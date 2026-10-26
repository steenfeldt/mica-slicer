// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Headless rendering of the 3D view with its overlays (toolbar, legend, sliders, gizmo) to a
//! PNG, for reviewing the look without a window. egui runs without a backend; its shapes are
//! drawn with `egui_wgpu::Renderer` into the same texture the viewport callback renders into.

use super::{FORMAT, Renderer};
use crate::{
    panels::profiles::Profiles,
    preview::{ColorMode, View},
    scene::{self, Bed, Object, camera::Camera, plates},
    theme,
    viewport::{Scene, Viewport, gizmo},
    worker,
};
use eframe::egui;
use egui_wgpu::wgpu;
use std::{
    future::Future,
    path::PathBuf,
    pin::pin,
    sync::{Arc, atomic::AtomicBool},
    task::{Context, Poll, Wake, Waker},
};

struct ThreadWake(std::thread::Thread);
impl Wake for ThreadWake {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.0.unpark();
    }
}
pub(crate) fn block_on<F: Future>(future: F) -> F::Output {
    let waker = Waker::from(Arc::new(ThreadWake(std::thread::current())));
    let mut cx = Context::from_waker(&waker);
    let mut f = pin!(future);
    loop {
        match f.as_mut().poll(&mut cx) {
            Poll::Ready(v) => return v,
            Poll::Pending => std::thread::park(),
        }
    }
}
struct Options {
    output: PathBuf,
    paths: Vec<PathBuf>,
    preview: bool,
    xl: bool,
    color: ColorMode,
    view: usize,
    select: Option<usize>,
    gizmo: gizmo::Mode,
    plates: usize,
    dark: bool,
    perspective: bool,
    layer: Option<usize>,
    moves: Option<u64>,
    ui: bool,
    size: [u32; 2],
    hover: Option<usize>,
    lay_on_face: bool,
    pointer: Option<[f32; 2]>,
    /// XY offset applied to the first object (to test out-of-bed warnings).
    offset: Option<[f32; 2]>,
    /// A printer preset by name instead of the default printer.
    printer: Option<String>,
    /// Keep only the objects whose centre lies on the first plate's bed (a multi-plate
    /// project's first plate, at its stored positions).
    first_plate: bool,
    /// `--bench DIR`: measure the preview (`render::bench`) instead of one screenshot.
    bench: Option<super::bench::Options>,
}
fn parse(args: &[String]) -> Result<Options, String> {
    let mut o = Options {
        output: PathBuf::new(),
        paths: vec![],
        preview: false,
        xl: false,
        color: ColorMode::Feature,
        view: 0,
        select: None,
        gizmo: gizmo::Mode::Move,
        plates: 1,
        dark: true,
        perspective: true,
        layer: None,
        moves: None,
        ui: true,
        size: [1400, 900],
        hover: None,
        lay_on_face: false,
        pointer: None,
        offset: None,
        printer: None,
        first_plate: false,
        bench: None,
    };
    let mut output = None;
    let mut iter = args.iter();
    let value = |iter: &mut std::slice::Iter<'_, String>, name: &str| {
        iter.next()
            .cloned()
            .ok_or_else(|| format!("{name} needs a value"))
    };
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--screenshot" => output = Some(PathBuf::from(value(&mut iter, arg)?)),
            "--preview" => o.preview = true,
            "--xl" => o.xl = true,
            "--light" => o.dark = false,
            "--ortho" => o.perspective = false,
            "--no-ui" => o.ui = false,
            "--ui" => o.ui = true,
            "--lay-on-face" => o.lay_on_face = true,
            "--printer" => o.printer = Some(value(&mut iter, arg)?),
            "--first-plate" => o.first_plate = true,
            "--bench" => {
                o.bench.get_or_insert_with(Default::default).dir =
                    PathBuf::from(value(&mut iter, arg)?)
            }
            "--tag" => o.bench.get_or_insert_with(Default::default).tag = value(&mut iter, arg)?,
            "--frames" => {
                o.bench.get_or_insert_with(Default::default).frames =
                    value(&mut iter, arg)?.parse().map_err(|_| "--frames N")?
            }
            "--only" => o.bench.get_or_insert_with(Default::default).only = value(&mut iter, arg)?,
            "--color" => {
                o.color = ColorMode::parse(&value(&mut iter, arg)?)
                    .ok_or_else(|| "Unknown color mode".to_string())?
            }
            "--view" => {
                o.view = match value(&mut iter, arg)?.as_str() {
                    "iso" => 0,
                    "top" => 1,
                    "bottom" => 2,
                    "front" => 3,
                    "rear" => 4,
                    "left" => 5,
                    "right" => 6,
                    _ => return Err("Unknown view".into()),
                }
            }
            "--gizmo" => {
                o.gizmo = match value(&mut iter, arg)?.as_str() {
                    "move" => gizmo::Mode::Move,
                    "rotate" => gizmo::Mode::Rotate,
                    "scale" => gizmo::Mode::Scale,
                    "none" => gizmo::Mode::None,
                    _ => return Err("Unknown gizmo".into()),
                }
            }
            "--select" => {
                o.select = Some(value(&mut iter, arg)?.parse().map_err(|_| "--select N")?)
            }
            "--hover" => o.hover = Some(value(&mut iter, arg)?.parse().map_err(|_| "--hover N")?),
            "--plates" => o.plates = value(&mut iter, arg)?.parse().map_err(|_| "--plates N")?,
            "--layer" => o.layer = Some(value(&mut iter, arg)?.parse().map_err(|_| "--layer N")?),
            "--moves" => o.moves = Some(value(&mut iter, arg)?.parse().map_err(|_| "--moves N")?),
            "--pointer" => {
                let v = value(&mut iter, arg)?;
                let (x, y) = v.split_once(',').ok_or("--pointer X,Y")?;
                o.pointer = Some([
                    x.parse().map_err(|_| "--pointer X,Y")?,
                    y.parse().map_err(|_| "--pointer X,Y")?,
                ]);
            }
            "--offset" => {
                let v = value(&mut iter, arg)?;
                let (x, y) = v.split_once(',').ok_or("--offset X,Y")?;
                o.offset = Some([
                    x.parse().map_err(|_| "--offset X,Y")?,
                    y.parse().map_err(|_| "--offset X,Y")?,
                ]);
            }
            "--size" => {
                let v = value(&mut iter, arg)?;
                let (w, h) = v.split_once('x').ok_or("--size WxH")?;
                o.size = [
                    w.parse().map_err(|_| "--size WxH")?,
                    h.parse().map_err(|_| "--size WxH")?,
                ];
            }
            x if x.starts_with('-') => return Err(format!("Unknown option {x}")),
            _ => o.paths.push(PathBuf::from(arg)),
        }
    }
    if o.paths.is_empty() {
        return Err("Supply at least one STL or 3MF".into());
    }
    o.output = output.ok_or("Missing screenshot path")?;
    if o.bench
        .as_ref()
        .is_some_and(|b| b.dir.as_os_str().is_empty())
    {
        return Err("--tag, --frames and --only need --bench DIR".into());
    }
    Ok(o)
}
pub fn run(args: &[String]) -> Result<(), String> {
    let o = parse(args)?;
    // Screenshots never see the user's own data (see `mica_core::user_data`): a throwaway
    // profile store and no other slicer's settings.
    mica_core::user_data::isolate();
    let store = std::env::temp_dir().join(format!("mica-shot3d-store-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&store);
    let mut profiles = Profiles::discover_with_store(mica_profiles::user::Store::open(store));
    if o.xl {
        profiles.printer = profiles
            .printers
            .iter()
            .find(|p| {
                p.contains("XL") && p.contains("5T") && p.contains("0.4") && !p.contains("Input")
            })
            .or_else(|| {
                profiles
                    .printers
                    .iter()
                    .find(|p| p.contains("XL") && p.contains("5T") && p.contains("0.4"))
            })
            .cloned()
            .ok_or("No XL 5-tool profile available")?;
        profiles.refresh();
        if let Some(p) = profiles
            .processes
            .iter()
            .find(|p| p.contains("0.20mm SPEED"))
        {
            profiles.process = p.clone();
            profiles.refresh();
        }
        if let Some(f) = profiles
            .materials
            .iter()
            .find(|f| f.starts_with("Prusament PLA"))
        {
            profiles.filaments.fill(f.clone());
        }
    }
    if let Some(name) = &o.printer {
        let (vendor, _, found) = profiles
            .all_printers()
            .into_iter()
            .find(|(_, _, n)| n == name)
            .ok_or_else(|| format!("No printer preset named {name}"))?;
        profiles.select_printer(vendor, &found);
    }
    let config = profiles.config()?;
    let bed = Bed::from_config(&config);
    let center = bed.center();
    eprintln!(
        "Printer: {} ({}, {})\nPrint: {}\nFilaments: {:?}",
        profiles.printer,
        bed.name,
        bed.size_label(),
        profiles.process,
        profiles.filaments
    );
    let loaded = mica_core::input::load(
        &o.paths,
        (center.x, center.y),
        false,
        true,
        0,
        &Default::default(),
        &config,
        &mica_profiles::project::Mapper::default(),
    )?;
    for w in loaded.warnings {
        eprintln!("{w}");
    }
    let mut objects: Vec<_> = loaded.objects.into_iter().map(Object::new).collect();
    if o.first_plate {
        objects.retain(|obj| {
            let (a, b) = obj.bounds();
            let c = (a + b) * 0.5;
            c.x >= bed.min.x && c.x <= bed.max.x && c.y >= bed.min.y && c.y <= bed.max.y
        });
        eprintln!("First plate: {} objects", objects.len());
    } else if objects.len() > 1 {
        scene::arrange_rows(&mut objects, &bed);
    }
    if o.xl {
        for (i, obj) in objects.iter_mut().enumerate() {
            obj.tools.0 = [Some(i % profiles.filaments.len()); 5];
        }
    }
    if let (Some(d), Some(obj)) = (o.offset, objects.first_mut()) {
        obj.position += glam::Vec3::new(d[0], d[1], 0.);
    }
    let mut tool_colors = scene::tool_colors(&config);
    if o.xl && tool_colors.iter().all(|c| *c == tool_colors[0]) {
        // XL profiles ship one colour per tool slot only in the GUI; use the fallback palette.
        tool_colors = (0..tool_colors.len())
            .map(crate::preview::legend::tool_color)
            .collect();
    }
    let triangles: usize = objects.iter().map(|obj| obj.mesh.faces.len()).sum();
    eprintln!(
        "Geometry: {triangles} triangles, memory {:?} MB",
        worker::memory_mb()
    );
    let result = if o.preview {
        Some(worker::slice(
            objects.clone(),
            config,
            &AtomicBool::new(false),
            &|p| eprintln!("{}", p.detail),
        )?)
    } else {
        None
    };
    if let Some(f) = &result {
        eprintln!(
            "Preview: {} layers, {} segments, {} bytes/segment, {:.1} MB disk; {:.1} min, {:.1} mm filament; {} tool changes; memory {:?} MB",
            f.preview.layers.len(),
            f.preview.count,
            std::mem::size_of::<crate::preview::Segment>(),
            f.preview.count as f64 * 32. / 1e6,
            f.stats.time_s / 60.,
            f.stats.filament_mm,
            f.preview.stats.tool_changes.len(),
            worker::memory_mb()
        );
    }
    let mut plates_layout = crate::plates::Plates::new(bed.max - bed.min);
    for _ in 1..o.plates {
        plates_layout.add();
    }
    plates_layout.current = 0;
    let plates = plates::snapshot(&plates_layout);
    let aspect = o.size[0] as f32 / o.size[1] as f32;
    let mut camera = Camera::looking_at(center, (bed.max - bed.min).max_element() * 1.15);
    camera.perspective = o.perspective;
    camera.preset(o.view);
    let mut viewport = Viewport::new(camera);
    viewport.mode = o.gizmo;
    viewport.lay_on_face = o.lay_on_face;
    viewport.hovered = o.hover;
    viewport.fit_plate(&bed, &plates, o.plates > 1, aspect);
    if let Some(i) = o.select
        && let Some(obj) = objects.get(i)
        && o.plates == 1
    {
        viewport.focus(obj, aspect);
    } else if o.preview && !objects.is_empty() {
        let mut min = glam::Vec3::splat(f32::INFINITY);
        let mut max = -min;
        for obj in &objects {
            let (a, b) = obj.bounds();
            min = min.min(a);
            max = max.max(b);
        }
        let pad = (max - min).max_element() * 0.45;
        viewport.camera.fit(min - pad, max + pad, aspect);
    }
    let mut view = View {
        color: o.color,
        ..Default::default()
    };
    if let Some(l) = o.layer {
        view.high = l;
    }
    if let Some(m) = o.moves {
        view.moves = m;
    }
    let scene = Shot {
        objects,
        selected: o.select,
        bed,
        plates,
        tool_colors,
        preview: result.as_ref().map(|r| r.preview.clone()),
        view,
        dark: o.dark,
        size: o.size,
        ui: o.ui,
        pointer: o.pointer,
    };
    if let Some(b) = o.bench {
        return block_on(super::bench::run(b, scene, viewport));
    }
    block_on(render(&o.output, scene, viewport))?;
    eprintln!(
        "Rendered {} · memory {:?} MB",
        o.output.display(),
        worker::memory_mb()
    );
    Ok(())
}
pub(crate) struct Shot {
    pub objects: Vec<Object>,
    pub selected: Option<usize>,
    pub bed: Bed,
    pub plates: Vec<plates::Plate>,
    pub tool_colors: Vec<[f32; 3]>,
    pub preview: Option<Arc<crate::preview::Preview>>,
    pub view: View,
    pub dark: bool,
    pub size: [u32; 2],
    pub ui: bool,
    pub pointer: Option<[f32; 2]>,
}
async fn render(
    path: &std::path::Path,
    mut shot: Shot,
    mut viewport: Viewport,
) -> Result<(), String> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions::default())
        .await
        .map_err(|e| e.to_string())?;
    eprintln!("GPU: {:?}", adapter.get_info().name);
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor::default())
        .await
        .map_err(|e| e.to_string())?;
    let [w, h] = shot.size;
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Screenshot"),
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
    let target_view = target.create_view(&Default::default());
    let mut egui_renderer = egui_wgpu::Renderer::new(&device, FORMAT, Default::default());
    egui_renderer
        .callback_resources
        .insert(Renderer::new(&device, FORMAT));
    let ctx = egui::Context::default();
    theme::apply(
        &ctx,
        if shot.dark {
            theme::Mode::Dark
        } else {
            theme::Mode::Light
        },
    );
    let screen = egui_wgpu::ScreenDescriptor {
        size_in_pixels: [w, h],
        pixels_per_point: 1.,
    };
    // Floating areas learn their size on the first pass and hover state needs a pass with
    // the pointer in place; render the third. A preview's hover card needs the renderer's
    // pick answer first: the 3D view is drawn and read back, then the passes run again.
    let mut out = None;
    let rounds = if shot.pointer.is_some() && shot.preview.is_some() {
        3
    } else {
        1
    };
    for pass in 0..3 * rounds {
        if pass > 0 && pass % 3 == 0 {
            let full: egui::FullOutput = out.take().expect("a pass ran");
            let primitives = ctx.tessellate(full.shapes, full.pixels_per_point);
            let mut encoder = device.create_command_encoder(&Default::default());
            let user =
                egui_renderer.update_buffers(&device, &queue, &mut encoder, &primitives, &screen);
            queue.submit(user.into_iter().chain([encoder.finish()]));
            device
                .poll(wgpu::PollType::wait_indefinitely())
                .map_err(|e| e.to_string())?;
        }
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(w as f32, h as f32),
            )),
            focused: true,
            events: shot
                .pointer
                .map(|p| vec![egui::Event::PointerMoved(egui::pos2(p[0], p[1]))])
                .unwrap_or_default(),
            ..Default::default()
        };
        let full = ctx.run_ui(raw, |ui| {
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ui, |ui| {
                    if shot.ui {
                        let preview = shot.preview.as_ref().map(|p| (p, &mut shot.view));
                        let _ = viewport.show(
                            ui,
                            Scene {
                                textures: true,
                                objects: &shot.objects,
                                plate_of: &[],
                                selected: shot.selected,
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
                                tool_labels: &[],
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
                    } else {
                        let rect = ui.available_rect_before_wrap();
                        let frame = super::Frame {
                            camera: viewport.camera.clone(),
                            bed: shot.bed.clone(),
                            plates: shot.plates.clone(),
                            objects: shot.objects.clone(),
                            plate_of: vec![],
                            volumes: vec![],
                            selected: shot.selected,
                            selection: shot.selected.into_iter().collect(),
                            hovered: None,
                            tool_colors: shot.tool_colors.clone(),
                            overlays: vec![],
                            preview: shot.preview.clone(),
                            view: shot.view.clone(),
                            size: [w, h],
                            dark: shot.dark,
                            clip: None,
                            caps: vec![],
                            clearance: vec![],
                            warned: vec![],
                            surface_textures: vec![],
                            moving: false,
                            pick: None,
                        };
                        ui.painter()
                            .add(egui_wgpu::Callback::new_paint_callback(rect, frame));
                    }
                });
        });
        // The font atlas arrives with the first pass; upload every pass's texture changes.
        for (id, deltas) in &full.textures_delta.set {
            for delta in deltas {
                egui_renderer.update_texture(&device, &queue, *id, delta);
            }
        }
        out = Some(full);
    }
    let out = out.unwrap();
    let primitives = ctx.tessellate(out.shapes, out.pixels_per_point);
    let mut encoder = device.create_command_encoder(&Default::default());
    let user = egui_renderer.update_buffers(&device, &queue, &mut encoder, &primitives, &screen);
    if let Some(e) = egui_renderer
        .callback_resources
        .get::<Renderer>()
        .and_then(|r| r.error.clone())
    {
        return Err(e);
    }
    {
        let mut pass = encoder
            .begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Screenshot UI"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target_view,
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
        egui_renderer.render(&mut pass, &primitives, &screen);
    }
    let stride = (w * 4).div_ceil(256) * 256;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Screenshot readback"),
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
    queue.submit(user.into_iter().chain([encoder.finish()]));
    let (tx, rx) = std::sync::mpsc::channel();
    buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .map_err(|e| e.to_string())?;
    rx.recv()
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    let data = buffer
        .slice(..)
        .get_mapped_range()
        .map_err(|e| e.to_string())?;
    let mut rgb = Vec::with_capacity((w * h * 3) as usize);
    for row in data.chunks_exact(stride as usize) {
        for pixel in row[..w as usize * 4].as_chunks::<4>().0 {
            rgb.extend_from_slice(&pixel[..3]);
        }
    }
    let png = mica_gcode::thumbnails::encode_png_rgb(w, h, &rgb).map_err(|e| e.to_string())?;
    std::fs::write(path, png).map_err(|e| e.to_string())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn arguments() {
        let o = parse(
            &[
                "--screenshot",
                "out.png",
                "a.stl",
                "--preview",
                "--color",
                "speed",
                "--view",
                "top",
                "--select",
                "0",
                "--gizmo",
                "rotate",
                "--plates",
                "3",
                "--light",
                "--ortho",
                "--layer",
                "12",
                "--size",
                "800x600",
            ]
            .map(String::from),
        )
        .unwrap();
        assert!(o.preview && !o.dark && !o.perspective);
        assert_eq!(
            (o.view, o.select, o.plates, o.layer),
            (1, Some(0), 3, Some(12))
        );
        assert_eq!(o.gizmo, gizmo::Mode::Rotate);
        assert_eq!(o.color, ColorMode::Speed);
        assert_eq!(o.size, [800, 600]);
        assert!(parse(&["--screenshot".into(), "x.png".into()]).is_err());
        assert!(
            parse(&[
                "--screenshot".into(),
                "x.png".into(),
                "a.stl".into(),
                "--bogus".into()
            ])
            .is_err()
        );
    }
}
