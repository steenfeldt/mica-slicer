// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Preview benchmark (development tool): frame times while orbiting, the hover pick and
//! level-of-detail screenshots, offscreen through the same egui paint-callback path as the
//! app. Run it through the 3D screenshot tool, always with a throwaway `MICA_CONFIG_DIR`:
//!
//! `mica-gui --screenshot-3d x.png MODEL --preview --size 3420x2000 --bench DIR [--tag NAME]
//! [--frames N] [--only orbit,hover,shots] [--printer NAME] [--first-plate]`
//!
//! It writes `DIR/NAME.md` (tables) and `DIR/NAME-<view>-<variant>.png`. Frame times are wall
//! clock from the UI pass to the GPU finishing (submit and wait), so on large previews they
//! are the GPU time.

use super::{FORMAT, Renderer, screenshot::Shot};
use crate::{
    theme,
    viewport::{Scene, Viewport},
};
use eframe::egui;
use egui_wgpu::wgpu;
use glam::Vec3;
use std::{fmt::Write as _, path::PathBuf, time::Instant};

#[derive(Clone, Debug, Default)]
pub struct Options {
    pub dir: PathBuf,
    pub tag: String,
    pub frames: usize,
    /// Comma-separated parts to run (`orbit`, `hover`, `shots`); empty runs all.
    pub only: String,
}
impl Options {
    fn runs(&self, part: &str) -> bool {
        self.only.is_empty() || self.only.split(',').any(|p| p.trim() == part)
    }
}

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    egui: egui_wgpu::Renderer,
    ctx: egui::Context,
}
#[derive(Default, Clone, Copy)]
struct Times {
    /// The UI pass (viewport, sliders, hover).
    ui: f64,
    /// The renderer's prepare (`update_buffers`: records the 3D frame, picks).
    prepare: f64,
    total: f64,
}
/// What a frame draws: the viewport with its overlays, or the 3D view alone.
enum Content {
    Ui(Option<[f32; 2]>),
    Frame(Box<super::Frame>),
}

fn frame(
    g: &mut Gpu,
    shot: &mut Shot,
    viewport: &mut Viewport,
    content: Content,
    target: &wgpu::Texture,
    readback: bool,
) -> (Times, Option<Vec<u8>>) {
    let [w, h] = [target.width(), target.height()];
    let ppp = 2.;
    let view = target.create_view(&Default::default());
    let t0 = Instant::now();
    let mut viewports = egui::ViewportIdMap::default();
    viewports.insert(
        egui::ViewportId::ROOT,
        egui::ViewportInfo {
            native_pixels_per_point: Some(ppp),
            ..Default::default()
        },
    );
    let (pointer, mut direct) = match content {
        Content::Ui(p) => (p, None),
        Content::Frame(f) => (None, Some(*f)),
    };
    let raw = egui::RawInput {
        viewports,
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(w as f32 / ppp, h as f32 / ppp),
        )),
        focused: true,
        events: pointer
            .map(|p| vec![egui::Event::PointerMoved(egui::pos2(p[0], p[1]))])
            .unwrap_or_default(),
        ..Default::default()
    };
    let mut full = g.ctx.run_ui(raw, |ui| {
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ui, |ui| {
                if let Some(f) = direct.take() {
                    let rect = ui.max_rect();
                    ui.painter()
                        .add(egui_wgpu::Callback::new_paint_callback(rect, f));
                    return;
                }
                let preview = shot.preview.as_ref().map(|p| (p, &mut shot.view));
                let _ = viewport.show(
                    ui,
                    Scene {
                        textures: true,
                        objects: &shot.objects,
                        plate_of: &[],
                        selected: None,
                        selection: &[],
                        plate_objects: shot.objects.len(),
                        only_object: (shot.objects.len() == 1).then_some(0),
                        sub: None,
                        towers: &[],
                        tower_selected: false,
                        cut: None,
                        mesh_tool: None,
                        can_paste: false,
                        bed: &shot.bed,
                        plates: &shot.plates,
                        tool_colors: &shot.tool_colors,
                        tool_alphas: &[],
                        tool_labels: &[],
                        tool_rgba: &[],
                        project_name: "",
                        overlays: &[],
                        paint: None,
                        object_tool: None,
                        preview,
                        previewing: false,
                        editable: true,
                        can_slice_all: false,
                        status: None,
                        totals: None,
                        progress: None,
                        layer_gcode: None,
                        clearance: &[],
                        warned: &[],
                        failed: None,
                        movable: &[],
                        adhesion_outlines: &[],
                    },
                );
            });
    });
    for (id, deltas) in &full.textures_delta.set {
        for delta in deltas {
            g.egui.update_texture(&g.device, &g.queue, *id, delta);
        }
    }
    full.textures_delta.clear();
    let ui_ms = t0.elapsed().as_secs_f64() * 1e3;
    let primitives = g.ctx.tessellate(full.shapes, full.pixels_per_point);
    let screen = egui_wgpu::ScreenDescriptor {
        size_in_pixels: [w, h],
        pixels_per_point: ppp,
    };
    let t1 = Instant::now();
    let mut encoder = g.device.create_command_encoder(&Default::default());
    let user = g
        .egui
        .update_buffers(&g.device, &g.queue, &mut encoder, &primitives, &screen);
    let prepare = t1.elapsed().as_secs_f64() * 1e3;
    {
        let mut pass = encoder
            .begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("bench UI"),
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
    let buffer = readback.then(|| {
        let buffer = g.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("bench readback"),
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
        buffer
    });
    g.queue.submit(user.into_iter().chain([encoder.finish()]));
    let _ = g.device.poll(wgpu::PollType::wait_indefinitely());
    let total = t0.elapsed().as_secs_f64() * 1e3;
    let rgb = buffer.and_then(|buffer| {
        buffer.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        let _ = g.device.poll(wgpu::PollType::wait_indefinitely());
        let data = buffer.slice(..).get_mapped_range().ok()?;
        let mut rgb = Vec::with_capacity((w * h * 3) as usize);
        for row in data.chunks_exact(stride as usize) {
            for pixel in row[..w as usize * 4].as_chunks::<4>().0 {
                rgb.extend_from_slice(&pixel[..3]);
            }
        }
        Some(rgb)
    });
    (
        Times {
            ui: ui_ms,
            prepare,
            total,
        },
        rgb,
    )
}

fn target(device: &wgpu::Device, [w, h]: [u32; 2]) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("bench target"),
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
    })
}
fn pct(v: &mut [f64], p: f64) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(|a, b| a.total_cmp(b));
    v[((v.len() - 1) as f64 * p).round() as usize]
}
fn renderer(g: &mut Gpu) -> &mut Renderer {
    g.egui
        .callback_resources
        .get_mut::<Renderer>()
        .expect("renderer installed")
}

pub async fn run(b: Options, mut shot: Shot, mut viewport: Viewport) -> Result<(), String> {
    let frames = if b.frames == 0 { 120 } else { b.frames };
    let tag = if b.tag.is_empty() { "bench" } else { &b.tag };
    std::fs::create_dir_all(&b.dir).map_err(|e| e.to_string())?;
    let preview = shot.preview.clone().ok_or("--bench needs --preview")?;
    let size = shot.size;
    let mut rep = String::new();
    let _ = writeln!(
        rep,
        "# {tag} · {}×{} px (2× points) · {} records, {} layers",
        size[0],
        size[1],
        preview.count,
        preview.layers.len()
    );
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions::default())
        .await
        .map_err(|e| e.to_string())?;
    let _ = writeln!(rep, "GPU: {}", adapter.get_info().name);
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor::default())
        .await
        .map_err(|e| e.to_string())?;
    let mut egui_r = egui_wgpu::Renderer::new(&device, FORMAT, Default::default());
    egui_r
        .callback_resources
        .insert(Renderer::new(&device, FORMAT));
    let ctx = egui::Context::default();
    theme::apply(&ctx, theme::Mode::Dark);
    let mut g = Gpu {
        device,
        queue,
        egui: egui_r,
        ctx,
    };
    let tgt = target(&g.device, size);
    let base_view = shot.view.clone();
    let cam0 = viewport.camera.clone();
    let layers = preview.layers.len();
    // The first frame uploads the records.
    let t = Instant::now();
    let _ = frame(
        &mut g,
        &mut shot,
        &mut viewport,
        Content::Ui(None),
        &tgt,
        false,
    );
    let _ = writeln!(
        rep,
        "first frame (uploads the records): {:.1} ms",
        t.elapsed().as_secs_f64() * 1e3
    );
    if let Some(e) = renderer(&mut g).error.clone() {
        return Err(e);
    }

    if b.runs("orbit") {
        type Apply = Box<dyn Fn(&mut crate::preview::View)>;
        let scenarios: Vec<(&str, Option<bool>, bool, Apply)> = vec![
            ("moving, full", Some(true), false, Box::new(|_| {})),
            (
                "still (refined), full",
                Some(false),
                false,
                Box::new(|_| {}),
            ),
            (
                "moving, 50 % layers",
                Some(true),
                false,
                Box::new(move |v| v.high = layers / 2),
            ),
            (
                "moving, outer walls only",
                Some(true),
                false,
                Box::new(|v| v.mask = 1 << 2),
            ),
            (
                "still, outer walls only",
                Some(false),
                false,
                Box::new(|v| v.mask = 1 << 2),
            ),
            (
                "moving, nothing visible",
                Some(true),
                false,
                Box::new(|v| v.mask = 0),
            ),
            (
                "legacy look (all full tubes), resident",
                Some(false),
                true,
                Box::new(|_| {}),
            ),
            (
                "empty range",
                Some(true),
                false,
                Box::new(|v| {
                    v.high = 0;
                    v.moves = 0;
                }),
            ),
        ];
        let _ = writeln!(
            rep,
            "\n## Orbit, {frames} frames, 360° (median / p95 ms)\n\n| scenario | total | UI pass | renderer prepare |\n|---|---|---|---|"
        );
        for (name, moving, legacy, apply) in &scenarios {
            shot.view = base_view.clone();
            apply(&mut shot.view);
            viewport.force_moving = *moving;
            renderer(&mut g).set_legacy(*legacy);
            let mut all = vec![];
            for f in 0..frames + 3 {
                viewport.camera = cam0.clone();
                viewport.camera.yaw = cam0.yaw + f as f32 * std::f32::consts::TAU / frames as f32;
                let (t, _) = frame(
                    &mut g,
                    &mut shot,
                    &mut viewport,
                    Content::Ui(None),
                    &tgt,
                    false,
                );
                if f >= 3 {
                    all.push(t);
                }
            }
            let col = |f: fn(&Times) -> f64| {
                let mut v: Vec<f64> = all.iter().map(f).collect();
                format!("{:.1} / {:.1}", pct(&mut v, 0.5), pct(&mut v, 0.95))
            };
            let _ = writeln!(
                rep,
                "| {name} | {} | {} | {} |",
                col(|t| t.total),
                col(|t| t.ui),
                col(|t| t.prepare)
            );
            eprintln!("bench: {name}");
        }
        renderer(&mut g).set_legacy(false);
        viewport.force_moving = None;
    }

    if b.runs("hover") {
        shot.view = base_view.clone();
        viewport.camera = cam0.clone();
        viewport.force_moving = Some(false);
        let [w, h] = [size[0] as f32 / 2., size[1] as f32 / 2.];
        // The index buffer is drawn with the first hover of a still view.
        let (first, _) = frame(
            &mut g,
            &mut shot,
            &mut viewport,
            Content::Ui(Some([w * 0.5, h * 0.5])),
            &tgt,
            false,
        );
        let mut prepare = vec![];
        let mut ui = vec![];
        let mut latency = vec![];
        let mut hits = 0;
        let n = 200;
        for k in 0..n {
            let p = [
                w * (0.3 + 0.4 * (k as f32 / n as f32)),
                h * (0.35 + 0.3 * ((k * 7 % n) as f32 / n as f32)),
            ];
            // Frames until the answer for this pointer position is in.
            for f in 1..=10 {
                let (t, _) = frame(
                    &mut g,
                    &mut shot,
                    &mut viewport,
                    Content::Ui(Some(p)),
                    &tgt,
                    false,
                );
                prepare.push(t.prepare);
                ui.push(t.ui);
                let done = viewport.hover_answer();
                if let Some(found) = done {
                    latency.push(f as f64);
                    hits += found as usize;
                    break;
                }
            }
        }
        let _ = writeln!(
            rep,
            "\n## Hover pick (index buffer), {n} pointer positions, camera still\n\nfirst hover of a still view (draws the index buffer): {:.1} ms frame\nhits {hits}/{n}; frames until answered: median {:.0}, max {:.0}\nper hover frame: renderer prepare (pick read-back and answer) median {:.3} ms, p95 {:.3} ms; UI pass incl. hover card median {:.3} ms",
            first.total,
            pct(&mut latency.clone(), 0.5),
            latency.iter().cloned().fold(0., f64::max),
            pct(&mut prepare.clone(), 0.5),
            pct(&mut prepare, 0.95),
            pct(&mut ui, 0.5),
        );
        viewport.force_moving = None;
    }

    if b.runs("shots") {
        let (mut lo, mut hi) = (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY));
        for o in &shot.objects {
            let (a, c) = o.bounds();
            lo = lo.min(a);
            hi = hi.max(c);
        }
        let center = (lo + hi) * 0.5;
        let views: Vec<(&str, f32, bool, bool)> = vec![
            // name, span factor, single layer, section
            ("far", 2.2, false, false),
            ("mid", 1.0, false, false),
            ("near", 0.45, false, false),
            ("close", 0.2, false, false),
            ("single-layer", 1.0, true, false),
            ("section", 0.6, false, true),
        ];
        for (vname, k, single, section) in views {
            for variant in ["still", "moving", "legacy"] {
                let mut camera = cam0.clone();
                camera.span *= k;
                camera.span_goal = camera.span;
                let mut view = base_view.clone();
                if single {
                    view.high = layers / 2;
                    view.low = layers / 2;
                }
                let clip = section.then(|| {
                    let (right, _) = camera.basis();
                    let n = -right;
                    [n.x, n.y, n.z, n.dot(center)]
                });
                renderer(&mut g).set_legacy(variant == "legacy");
                let f = super::Frame {
                    camera,
                    bed: shot.bed.clone(),
                    plates: shot.plates.clone(),
                    objects: shot.objects.clone(),
                    plate_of: vec![],
                    volumes: vec![],
                    selected: None,
                    selection: vec![],
                    hovered: None,
                    tool_colors: shot.tool_colors.clone(),
                    tool_alphas: vec![],
                    overlays: vec![],
                    preview: shot.preview.clone(),
                    view,
                    size,
                    dark: shot.dark,
                    clip,
                    caps: vec![],
                    clearance: vec![],
                    warned: vec![],
                    surface_textures: vec![],
                    moving: variant == "moving",
                    pick: None,
                    tags: vec![],
                };
                let (_, rgb) = frame(
                    &mut g,
                    &mut shot,
                    &mut viewport,
                    Content::Frame(Box::new(f)),
                    &tgt,
                    true,
                );
                let rgb = rgb.ok_or("read-back failed")?;
                let png = mica_gcode::thumbnails::encode_png_rgb(size[0], size[1], &rgb)
                    .map_err(|e| e.to_string())?;
                let path = b.dir.join(format!("{tag}-{vname}-{variant}.png"));
                std::fs::write(&path, png).map_err(|e| e.to_string())?;
                eprintln!("bench: {}", path.display());
            }
        }
        renderer(&mut g).set_legacy(false);
    }
    let path = b.dir.join(format!("{tag}.md"));
    std::fs::write(&path, &rep).map_err(|e| e.to_string())?;
    println!("{rep}");
    Ok(())
}
