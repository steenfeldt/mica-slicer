// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Offscreen GPU tests of the G-code preview renderer: a synthetic preview drawn through the
//! same egui paint-callback path as the app. Each test skips (passes) on a machine without a
//! GPU adapter, and on Windows with only a software adapter (WARP) unless `MICA_GPU_TESTS=1`:
//! the GitHub Windows runner's in-box WARP took the test process down with an access
//! violation (STATUS_ACCESS_VIOLATION, no panic, no backtrace). The tests run one at a time
//! (one device at a time), whatever the test harness's thread count.

use super::{FORMAT, Frame, Renderer};
use crate::{
    preview::{Attr, Layer, Preview, Segment, Stats, View},
    scene::{Bed, camera::Camera, plates},
    worker::TempJob,
};
use eframe::egui;
use egui_wgpu::wgpu;
use glam::Vec3;
use std::{
    io::Write,
    sync::{Arc, Mutex, MutexGuard},
};

pub struct Gpu {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub egui: egui_wgpu::Renderer,
    ctx: egui::Context,
    /// Held until the device above is dropped (fields drop in order): one GPU test at a time.
    _turn: MutexGuard<'static, ()>,
}
/// Serializes the GPU tests: several devices made and used at once on one adapter is not
/// what the app does, and a software adapter is the most likely to trip over it.
static TURN: Mutex<()> = Mutex::new(());

/// A headless device with the renderer installed, or `None` (the test skips) without a
/// usable adapter.
pub fn gpu() -> Option<Gpu> {
    // A test that failed while holding the turn leaves nothing behind but its device.
    let turn = TURN.lock().unwrap_or_else(|e| e.into_inner());
    super::screenshot::block_on(async {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let Ok(adapter) = instance
            .request_adapter(&wgpu::RequestAdapterOptions::default())
            .await
        else {
            eprintln!("skipped: no GPU adapter");
            return None;
        };
        let info = adapter.get_info();
        eprintln!(
            "GPU tests on {:?} ({:?}, {:?}, driver {:?} {:?})",
            info.name, info.backend, info.device_type, info.driver, info.driver_info
        );
        if cfg!(windows)
            && info.device_type == wgpu::DeviceType::Cpu
            && std::env::var_os("MICA_GPU_TESTS").is_none_or(|v| v != "1")
        {
            eprintln!(
                "skipped: software adapter on Windows (WARP crashed the test process on CI); \
                 MICA_GPU_TESTS=1 runs it anyway"
            );
            return None;
        }
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor::default())
            .await
            .inspect_err(|e| eprintln!("skipped: no device: {e}"))
            .ok()?;
        let mut egui = egui_wgpu::Renderer::new(&device, FORMAT, Default::default());
        egui.callback_resources
            .insert(Renderer::new(&device, FORMAT));
        Some(Gpu {
            device,
            queue,
            egui,
            ctx: egui::Context::default(),
            _turn: turn,
        })
    })
}

/// A preview of `layers` (records per layer) written to a temporary job.
pub fn preview(layers: &[Vec<Segment>]) -> Arc<Preview> {
    let job = Arc::new(TempJob::new().unwrap());
    let mut records = std::fs::File::create(job.path.join("preview.bin")).unwrap();
    let mut attrs = std::fs::File::create(job.path.join("attrs.bin")).unwrap();
    let mut out = vec![];
    let mut start = 0;
    for (i, l) in layers.iter().enumerate() {
        records.write_all(bytemuck::cast_slice(l)).unwrap();
        let a: Vec<Attr> = l
            .iter()
            .map(|s| Attr::new(s.speed() as f64, 10., 215., 4000.))
            .collect();
        attrs.write_all(bytemuck::cast_slice(&a)).unwrap();
        out.push(Layer {
            start,
            count: l.len() as u64,
            z: l.first().map_or(0.2 * (i + 1) as f32, |s| s.b[2]),
            time_s: 10. + i as f32,
        });
        start += l.len() as u64;
    }
    Arc::new(Preview {
        job,
        layers: out,
        count: start,
        stats: Stats::default(),
        total_time_s: None,
        color_print: None,
        relabel: None,
    })
}
pub fn line(a: [f32; 3], b: [f32; 3], role: u32) -> Segment {
    Segment::new(a, b, [0.45, 0.2], role, 0, 120., 100.)
}
/// A walled cylinder: outer and inner wall circles and alternating infill per layer.
pub fn tower(layers: usize, center: [f32; 2], radius: f32) -> Vec<Vec<Segment>> {
    let ring = |r: f32, z: f32, role: u32| {
        let n = 120;
        (0..n)
            .map(|k| {
                let p = |k: usize| {
                    let t = k as f32 / n as f32 * std::f32::consts::TAU;
                    [center[0] + r * t.cos(), center[1] + r * t.sin(), z]
                };
                line(p(k), p(k + 1), role)
            })
            .collect::<Vec<_>>()
    };
    (0..layers)
        .map(|l| {
            let z = 0.2 * (l + 1) as f32;
            let mut v = ring(radius, z, 2);
            v.push(line(
                [center[0] + radius, center[1], z],
                [center[0] + radius - 0.45, center[1], z],
                crate::preview::TRAVEL,
            ));
            v.extend(ring(radius - 0.45, z, 4));
            let inner = radius - 0.9;
            let mut y = -inner + 0.75;
            while y < inner {
                let x = (inner * inner - y * y).sqrt();
                let (a, b) = if l % 2 == 0 {
                    (
                        [center[0] - x, center[1] + y, z],
                        [center[0] + x, center[1] + y, z],
                    )
                } else {
                    (
                        [center[0] + y, center[1] - x, z],
                        [center[0] + y, center[1] + x, z],
                    )
                };
                v.push(line(a, b, 5));
                y += 1.5;
            }
            v
        })
        .collect()
}
pub fn bed() -> (Bed, Vec<plates::Plate>) {
    let mut bed = Bed::from_config(&mica_core::config::Config::defaults());
    bed.assets = Default::default();
    let layout = crate::plates::Plates::new(bed.max - bed.min);
    let plates = plates::snapshot(&layout);
    (bed, plates)
}
/// A camera looking at `target` with `span` mm across the view height.
pub fn camera(target: [f32; 3], span: f32) -> Camera {
    let mut c = Camera::looking_at(Vec3::from(target), span);
    c.yaw = -1.05;
    c.pitch = 0.45;
    c
}
pub fn frame(preview: &Arc<Preview>, camera: Camera, view: View, size: [u32; 2]) -> Frame {
    let (bed, plates) = bed();
    Frame {
        camera,
        bed,
        plates,
        objects: vec![],
        plate_of: vec![],
        volumes: vec![],
        selected: None,
        selection: vec![],
        hovered: None,
        tool_colors: vec![[0.9, 0.5, 0.2]],
        overlays: vec![],
        preview: Some(preview.clone()),
        view,
        size,
        dark: true,
        clip: None,
        caps: vec![],
        clearance: vec![],
        warned: vec![],
        surface_textures: vec![],
        moving: false,
        pick: None,
        tags: vec![],
    }
}
/// Renders `frame` through egui's paint callback and returns the RGB pixels.
pub fn render(g: &mut Gpu, frame: Frame) -> Vec<u8> {
    let [w, h] = frame.size;
    let raw = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(w as f32, h as f32),
        )),
        ..Default::default()
    };
    let mut frame = Some(frame);
    let mut full = g.ctx.run_ui(raw, |ui| {
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ui, |ui| {
                let rect = ui.max_rect();
                if let Some(f) = frame.take() {
                    ui.painter()
                        .add(egui_wgpu::Callback::new_paint_callback(rect, f));
                }
            });
    });
    for (id, deltas) in &full.textures_delta.set {
        for delta in deltas {
            g.egui.update_texture(&g.device, &g.queue, *id, delta);
        }
    }
    full.textures_delta.clear();
    let primitives = g.ctx.tessellate(full.shapes, full.pixels_per_point);
    let screen = egui_wgpu::ScreenDescriptor {
        size_in_pixels: [w, h],
        pixels_per_point: 1.,
    };
    let target = g.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("test target"),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let mut encoder = g.device.create_command_encoder(&Default::default());
    let user = g
        .egui
        .update_buffers(&g.device, &g.queue, &mut encoder, &primitives, &screen);
    {
        let mut pass = encoder
            .begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("test UI"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                ..Default::default()
            })
            .forget_lifetime();
        g.egui.render(&mut pass, &primitives, &screen);
    }
    let stride = (w * 4).div_ceil(256) * 256;
    let buffer = g.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("test readback"),
        size: (stride * h) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        target.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(stride),
                rows_per_image: Some(h),
            },
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
    g.queue.submit(user.into_iter().chain([encoder.finish()]));
    buffer.slice(..).map_async(wgpu::MapMode::Read, |_| {});
    g.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    let data = buffer.slice(..).get_mapped_range().unwrap();
    let mut rgb = Vec::with_capacity((w * h * 3) as usize);
    for row in data.chunks_exact(stride as usize) {
        for pixel in row[..w as usize * 4].as_chunks::<4>().0 {
            rgb.extend_from_slice(&pixel[..3]);
        }
    }
    rgb
}

/// The near-view reference scene: a walled cylinder whose 0.2 mm layers project to about
/// 2–3 px, seen still.
fn near_scene() -> (Arc<Preview>, Camera) {
    let p = preview(&tower(30, [100., 100.], 6.));
    (p, camera([100., 100., 3.], 20.))
}
const NEAR_SIZE: [u32; 2] = [320, 240];
/// Made by the renderer before the levels of detail (every move a streamed tube, main at
/// 086eb00): the look near views must keep.
const NEAR_REFERENCE: &str = "src/render/testdata/preview-near-tubes.png";

fn decode(png: &[u8]) -> Vec<u8> {
    image::load_from_memory(png).unwrap().to_rgb8().into_raw()
}
/// Mean absolute channel difference and the share of pixels differing by more than 24 in
/// any channel.
fn difference(a: &[u8], b: &[u8]) -> (f64, f64) {
    assert_eq!(a.len(), b.len());
    let sum: u64 = a.iter().zip(b).map(|(x, y)| x.abs_diff(*y) as u64).sum();
    let off = a
        .chunks(3)
        .zip(b.chunks(3))
        .filter(|(p, q)| p.iter().zip(*q).any(|(x, y)| x.abs_diff(*y) > 24))
        .count();
    (
        sum as f64 / a.len() as f64,
        off as f64 / (a.len() / 3) as f64,
    )
}
/// Saves a test image to `$MICA_TEST_DUMP/<name>.png` when that is set (for looking at it).
fn dump(name: &str, rgb: &[u8]) {
    if let Ok(dir) = std::env::var("MICA_TEST_DUMP") {
        let png = mica_gcode::thumbnails::encode_png_rgb(NEAR_SIZE[0], NEAR_SIZE[1], rgb).unwrap();
        std::fs::write(std::path::Path::new(&dir).join(format!("{name}.png")), png).unwrap();
    }
}
fn renderer(g: &mut Gpu) -> &mut Renderer {
    g.egui.callback_resources.get_mut::<Renderer>().unwrap()
}

/// The legacy look (`Renderer::set_legacy`: every move a full tube with tube shading) is the
/// renderer before the levels of detail, pixel for pixel; and a near view, whose moves make
/// stripes of 3 px and more on screen (still or moving), looks like it. Below 3 px the tube
/// shading fades into the flat look.
#[test]
fn near_views_match_the_tube_rendering() {
    let Some(mut g) = gpu() else {
        return;
    };
    let reference = decode(
        &std::fs::read(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(NEAR_REFERENCE))
            .unwrap(),
    );
    let (p, c) = near_scene();
    renderer(&mut g).set_legacy(true);
    let legacy = render(&mut g, frame(&p, c.clone(), View::default(), NEAR_SIZE));
    let (mean, share) = difference(&legacy, &reference);
    eprintln!("legacy vs the old renderer: mean {mean:.3}, pixels off {share:.4}");
    assert!(mean < 0.1 && share < 0.001, "{mean} {share}");
    // Layers of about 2.4 px (stripes of about 2.2 px seen from above): partly flat.
    renderer(&mut g).set_legacy(false);
    let rgb = render(&mut g, frame(&p, c.clone(), View::default(), NEAR_SIZE));
    dump("near-2px", &rgb);
    let (mean, share) = difference(&rgb, &reference);
    eprintln!("2.4 px layers vs the old renderer: mean {mean:.3}, pixels off {share:.4}");
    assert!(mean < 4.0, "{mean} {share}");
    // Layers of 5 px (every stripe 3 px and more): identical, still and moving.
    let mut close = c.clone();
    close.span = 9.;
    close.span_goal = 9.;
    renderer(&mut g).set_legacy(true);
    let legacy = render(&mut g, frame(&p, close.clone(), View::default(), NEAR_SIZE));
    renderer(&mut g).set_legacy(false);
    for moving in [false, true] {
        let mut f = frame(&p, close.clone(), View::default(), NEAR_SIZE);
        f.moving = moving;
        let rgb = render(&mut g, f);
        dump(&format!("near-{moving}"), &rgb);
        let (mean, share) = difference(&rgb, &legacy);
        eprintln!("near, moving {moving}: mean {mean:.3}, pixels off {share:.4}");
        assert!(
            mean < 0.2 && share < 0.002,
            "moving {moving}: {mean} {share}"
        );
    }
}

/// Far away the moving view (ribbons) looks like the still one (tubes), so nothing pops when
/// the view refines; both differ from the old look only by the sub-pixel shading.
#[test]
fn far_ribbons_look_like_the_refined_tubes() {
    let Some(mut g) = gpu() else {
        return;
    };
    let p = preview(&tower(60, [100., 100.], 6.));
    let c = camera([100., 100., 6.], 50.);
    let draw = |g: &mut Gpu, moving| {
        let mut f = frame(&p, c.clone(), View::default(), NEAR_SIZE);
        f.moving = moving;
        render(g, f)
    };
    let still = draw(&mut g, false);
    let moving = draw(&mut g, true);
    dump("far-still", &still);
    dump("far-moving", &moving);
    let (mean, share) = difference(&still, &moving);
    eprintln!("far, ribbons vs tubes: mean {mean:.3}, pixels off {share:.4}");
    assert!(mean < 0.5 && share < 0.005, "{mean} {share}");
    renderer(&mut g).set_legacy(true);
    let legacy = draw(&mut g, false);
    renderer(&mut g).set_legacy(false);
    dump("far-legacy", &legacy);
    let (mean, share) = difference(&still, &legacy);
    eprintln!("far, refined vs legacy: mean {mean:.3}, pixels off {share:.4}");
    assert!(mean < 4.0, "{mean}");
}

/// The hover pick searches the whole range, takes the move in front and skips moves that
/// are hidden by the legend, cut away by the section view or above the layer slider.
#[test]
fn pick_finds_the_front_move_anywhere_in_the_range() {
    let Some(mut g) = gpu() else {
        return;
    };
    // Record 0: an outer wall line across the view (x = 110; the camera looks along −x);
    // then 60 000 infill records off to the side; last, an infill line right behind
    // record 0 (x = 90).
    let z = 0.2;
    let mut layers = vec![vec![line([110., 95., z], [110., 105., z], 2)]];
    for l in 1..=60 {
        let zl = 0.2 * (l + 1) as f32;
        layers.push(
            (0..1000)
                .map(|k| {
                    let x = 20. + (k % 100) as f32 * 0.5;
                    line([x, 20., zl], [x + 0.4, 20., zl], 5)
                })
                .collect(),
        );
    }
    if let Some(top) = layers.last_mut() {
        top.push(line([90., 95., z], [90., 105., z], 5));
    }
    let p = preview(&layers);
    let last = p.count - 1;
    let mut c = Camera::looking_at(Vec3::new(100., 100., z - 0.1), 30.);
    c.yaw = 0.;
    c.pitch = 0.;
    let pick = |g: &mut Gpu, view: View, clip: Option<[f32; 4]>| {
        let shared = super::toolpaths::SharedPick::default();
        shared.lock().unwrap().request = Some(super::toolpaths::PickRequest {
            px: [NEAR_SIZE[0] / 2, NEAR_SIZE[1] / 2],
            radius: 10,
        });
        // Draw (and the index buffer), read back, take the answer.
        for _ in 0..3 {
            let mut f = frame(&p, c.clone(), view.clone(), NEAR_SIZE);
            f.clip = clip;
            f.pick = Some(shared.clone());
            render(g, f);
        }
        let result = shared.lock().unwrap().result;
        result.expect("answered").record
    };
    assert_eq!(pick(&mut g, View::default(), None), Some(0));
    // Outer walls hidden: the line behind it.
    let walls_hidden = View {
        mask: View::default().mask & !(1 << 2),
        ..View::default()
    };
    assert_eq!(pick(&mut g, walls_hidden, None), Some(last));
    // Cut away by the section plane (x > 100 removed).
    assert_eq!(
        pick(&mut g, View::default(), Some([1., 0., 0., 100.])),
        Some(last)
    );
    // The layer slider on the first layer: only record 0 is drawn.
    let first = View {
        high: 0,
        ..View::default()
    };
    assert_eq!(pick(&mut g, first, None), Some(0));
    let nothing = View {
        high: 0,
        mask: 1 << 5,
        ..View::default()
    };
    assert_eq!(pick(&mut g, nothing, None), None);
}

/// Writes the near-view reference image. Only for a deliberate change of the tube look: it
/// replaces the reference the levels of detail are checked against.
#[test]
#[ignore]
fn write_near_reference() {
    let Some(mut g) = gpu() else {
        return;
    };
    let (p, c) = near_scene();
    let rgb = render(&mut g, frame(&p, c, View::default(), NEAR_SIZE));
    let png = mica_gcode::thumbnails::encode_png_rgb(NEAR_SIZE[0], NEAR_SIZE[1], &rgb).unwrap();
    std::fs::write(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(NEAR_REFERENCE),
        png,
    )
    .unwrap();
}
