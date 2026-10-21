// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The Library details panel's 3D preview: a small orbit view of the selected model. The mesh
//! is read and every frame is drawn on a background thread (the CPU renderer of the library
//! thumbnails, `mica_library::render`), so the panel never waits; the UI only uploads the
//! newest picture into one texture. Dropping the preview (another selection, the Library
//! closed) ends the thread and frees the mesh and the texture.

use crate::theme;
use eframe::egui::{self, Align2, Pos2, Rect, Sense, Stroke, Vec2};
use mica_core::mesh::TriangleMesh;
use mica_gcode::thumbnails::{Fit, Orbit};
use mica_library::Kind;
use std::path::PathBuf;
use std::sync::mpsc;

/// Meshes with more triangles are simplified for the preview (it redraws while dragging).
const PREVIEW_TRIANGLES: usize = 250_000;
/// Cells along the largest side when simplifying for the preview.
const PREVIEW_CELLS: u32 = 400;

/// The orbit view: degrees and a magnification.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    pub azimuth: f32,
    pub elevation: f32,
    pub zoom: f32,
}

impl Default for View {
    /// The thumbnail's view, so the preview starts where the card's picture is.
    fn default() -> Self {
        let v = mica_library::render::VIEW;
        Self {
            azimuth: v.azimuth,
            elevation: v.elevation,
            zoom: 1.,
        }
    }
}

/// What the background thread found in the file.
#[derive(Clone, Debug, PartialEq)]
pub struct Facts {
    /// Bounding box size, mm.
    pub size: [f32; 3],
    pub triangles: usize,
}

enum State {
    Loading,
    Ready(Facts),
    Failed(String),
}

enum Reply {
    Loaded(Result<Facts, String>),
    Frame(View, [u16; 2], egui::ColorImage),
}

pub struct Preview {
    /// The model (content hash) shown.
    pub id: String,
    pub view: View,
    state: State,
    texture: Option<egui::TextureHandle>,
    /// The view and size of the last request and of the picture shown.
    requested: Option<(View, [u16; 2])>,
    shown: Option<(View, [u16; 2])>,
    tx: mpsc::Sender<(View, [u16; 2])>,
    rx: mpsc::Receiver<Reply>,
}

fn simplified(mesh: TriangleMesh) -> TriangleMesh {
    if mesh.faces.len() > PREVIEW_TRIANGLES {
        mica_library::render::simplify(&mesh, PREVIEW_CELLS)
    } else {
        mesh
    }
}

impl Preview {
    /// Starts reading `path` on a background thread (read-only).
    pub fn new(id: &str, kind: Kind, path: PathBuf, ctx: &egui::Context) -> Self {
        let (tx, requests) = mpsc::channel::<(View, [u16; 2])>();
        let (replies, rx) = mpsc::channel();
        let ctx = ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("mica-library-preview".into())
            .spawn(move || {
                let mesh = mica_library::render::load_mesh_file(kind, &path);
                let facts = mesh.as_ref().map_err(Clone::clone).map(|m| {
                    let (lo, hi) = m.bounds();
                    Facts {
                        size: std::array::from_fn(|i| hi[i] - lo[i]),
                        triangles: m.faces.len(),
                    }
                });
                let failed = facts.is_err();
                if replies.send(Reply::Loaded(facts)).is_err() || failed {
                    return;
                }
                ctx.request_repaint();
                let Ok(mesh) = mesh.map(simplified) else {
                    return;
                };
                // Draw the newest request; end when the preview is dropped.
                while let Ok(mut req) = requests.recv() {
                    while let Ok(newer) = requests.try_recv() {
                        req = newer;
                    }
                    let (view, [w, h]) = req;
                    let orbit = Orbit {
                        azimuth: view.azimuth,
                        elevation: view.elevation,
                        zoom: view.zoom,
                        fit: Fit::Sphere,
                    };
                    let Ok(img) = mica_gcode::thumbnails::render_orbit(
                        &mesh,
                        w,
                        h,
                        mica_library::render::VIOLET,
                        orbit,
                    ) else {
                        continue;
                    };
                    let image = egui::ColorImage::from_rgba_unmultiplied(
                        [usize::from(img.width), usize::from(img.height)],
                        &img.rgba,
                    );
                    if replies.send(Reply::Frame(view, [w, h], image)).is_err() {
                        return;
                    }
                    ctx.request_repaint();
                }
            });
        let mut p = Self {
            id: id.to_string(),
            view: View::default(),
            state: State::Loading,
            texture: None,
            requested: None,
            shown: None,
            tx,
            rx,
        };
        if let Err(e) = spawned {
            p.state = State::Failed(e.to_string());
        }
        p
    }

    /// Picks up what the thread sent: the facts, the newest picture.
    pub fn poll(&mut self, ctx: &egui::Context) {
        while let Ok(reply) = self.rx.try_recv() {
            match reply {
                Reply::Loaded(Ok(f)) => self.state = State::Ready(f),
                Reply::Loaded(Err(e)) => self.state = State::Failed(e),
                Reply::Frame(view, size, image) => {
                    match &mut self.texture {
                        Some(t) => t.set(image, egui::TextureOptions::LINEAR),
                        None => {
                            self.texture = Some(ctx.load_texture(
                                format!("lib-preview-{}", self.id),
                                image,
                                egui::TextureOptions::LINEAR,
                            ))
                        }
                    }
                    self.shown = Some((view, size));
                }
            }
        }
    }

    pub fn facts(&self) -> Option<&Facts> {
        match &self.state {
            State::Ready(f) => Some(f),
            _ => None,
        }
    }

    pub fn error(&self) -> Option<&str> {
        match &self.state {
            State::Failed(e) => Some(e),
            _ => None,
        }
    }

    pub fn loading(&self) -> bool {
        matches!(self.state, State::Loading)
    }

    pub fn texture(&self) -> Option<&egui::TextureHandle> {
        self.texture.as_ref()
    }

    /// The picture shown is the newest one asked for.
    pub fn settled(&self) -> bool {
        self.shown.is_some() && self.shown == self.requested
    }

    /// Asks for a picture of `size` physical pixels in the current view (once per change).
    pub fn request(&mut self, size: [u16; 2]) {
        if self.facts().is_none() {
            return;
        }
        let size = size.map(|v| v.clamp(16, 999));
        let want = (self.view, size);
        if self.requested != Some(want) {
            self.requested = Some(want);
            let _ = self.tx.send(want);
        }
    }

    /// Drag: turn the view (degrees per point).
    pub fn orbit(&mut self, delta: egui::Vec2) {
        self.view.azimuth = (self.view.azimuth - delta.x * 0.6).rem_euclid(360.);
        self.view.elevation = (self.view.elevation + delta.y * 0.6).clamp(-89., 89.);
    }

    /// Scroll: zoom in (positive) or out.
    pub fn zoom(&mut self, scroll: f32) {
        self.view.zoom = (self.view.zoom * (scroll * 0.004).exp()).clamp(0.4, 8.);
    }

    pub fn reset(&mut self) {
        self.view = View::default();
    }
}

/// Whether a model of `size` (mm) fits a build volume, turned on the bed if need be.
pub fn fits(size: [f32; 3], volume: [f32; 3]) -> bool {
    let [x, y, z] = size;
    let [bx, by, bz] = volume;
    let e = 1e-3;
    z <= bz + e && ((x <= bx + e && y <= by + e) || (y <= bx + e && x <= by + e))
}

/// A printer preset as a short name: `Prusa CORE One HF0.4 nozzle` → `CORE One`,
/// `Snapmaker U1 (0.4 nozzle)` → `U1`.
pub fn short_printer(preset: &str) -> String {
    let mut s = preset.trim();
    for prefix in ["Original Prusa ", "Prusa ", "Snapmaker "] {
        s = s.strip_prefix(prefix).unwrap_or(s);
    }
    let words: Vec<&str> = s.split_whitespace().collect();
    let words = if words
        .last()
        .is_some_and(|w| w.trim_end_matches(')') == "nozzle")
        && words.len() > 2
    {
        &words[..words.len() - 2]
    } else {
        &words[..]
    };
    words.join(" ").trim_end_matches([' ', '-']).to_string()
}

/// `225154` → `225,154`.
pub fn group_thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// A printer chip: green when the model fits, grey when it does not.
fn fit_chip(ui: &mut egui::Ui, name: &str, ok: bool) -> egui::Response {
    let p = theme::of(ui);
    let color = if ok { p.success } else { p.muted };
    let g = ui
        .painter()
        .layout_no_wrap(name.to_string(), theme::medium(11.), color);
    let (r, resp) = ui.allocate_exact_size(Vec2::new(g.size().x + 14., 20.), Sense::hover());
    ui.painter()
        .rect_filled(r, 10., color.gamma_multiply(if ok { 0.16 } else { 0.1 }));
    ui.painter().rect_stroke(
        r,
        10.,
        Stroke::new(1., color.gamma_multiply(0.6)),
        egui::StrokeKind::Inside,
    );
    ui.painter().galley(
        Pos2::new(r.left() + 7., r.center().y - g.size().y / 2.),
        g,
        color,
    );
    resp
}

impl crate::app::App {
    /// The user's printers (My printers, else the current one) with their build volumes.
    pub(crate) fn library_printer_volumes(&mut self) -> Vec<(String, Option<[f32; 3]>)> {
        let mut out: Vec<(String, Option<[f32; 3]>)> = vec![];
        let list: Vec<(String, Option<String>)> = self
            .printers
            .list
            .iter()
            .map(|p| (p.name.clone(), p.profile.clone()))
            .collect();
        for (name, profile) in list {
            if out.iter().any(|(n, _)| *n == name) {
                continue;
            }
            let volume = profile.and_then(|key| {
                if let Some(v) = self.library.volumes.get(&key) {
                    return *v;
                }
                let v = key.split_once('/').and_then(|(vendor, preset)| {
                    let vi = self.profiles.vendor_index(vendor)?;
                    let c = self.profiles.printer_defaults_config(vi, preset).ok()?;
                    let bed = crate::scene::bed::Bed::from_config(&c);
                    Some([bed.max.x - bed.min.x, bed.max.y - bed.min.y, bed.height])
                });
                self.library.volumes.insert(key, v);
                v
            });
            out.push((name, volume));
        }
        if out.is_empty() {
            let b = &self.bed;
            out.push((
                short_printer(&self.profiles.printer),
                Some([b.max.x - b.min.x, b.max.y - b.min.y, b.height]),
            ));
        }
        out
    }

    /// The details panel's picture: the 3D preview once loaded (drag to turn, scroll to
    /// zoom, double-click to reset), the thumbnail until then; below it the size, the
    /// triangle count, which printers it fits, and a handle to drag it onto the plate.
    pub(crate) fn library_detail_preview(&mut self, ui: &mut egui::Ui, m: &mica_library::Model) {
        let p = theme::of(ui);
        let ctx = ui.ctx().clone();
        let (rect, resp) = ui.allocate_exact_size(
            Vec2::new(ui.available_width(), 220.),
            Sense::click_and_drag(),
        );
        ui.painter().rect_filled(rect, 8., p.search_bg);
        let ppp = ctx.pixels_per_point();
        let size = [
            (rect.width() * ppp).round() as u16,
            (rect.height() * ppp).round() as u16,
        ];
        let mut shown = false;
        if let Some(pv) = self.library.preview.as_mut().filter(|pv| pv.id == m.id) {
            if resp.dragged() {
                pv.orbit(resp.drag_delta());
            }
            if resp.hovered() {
                let scroll = ui.input(|i| i.smooth_scroll_delta.y);
                if scroll != 0. {
                    pv.zoom(scroll);
                }
            }
            if resp.double_clicked() {
                pv.reset();
            }
            pv.request(size);
            if let Some(t) = pv.texture() {
                let s = t.size_vec2() / ppp;
                let k = (rect.width() / s.x).min(rect.height() / s.y).min(1.);
                ui.painter().image(
                    t.id(),
                    Rect::from_center_size(rect.center(), s * k),
                    Rect::from_min_max(Pos2::ZERO, Pos2::new(1., 1.)),
                    egui::Color32::WHITE,
                );
                shown = true;
            }
            if pv.facts().is_some() && (resp.hovered() || resp.dragged()) {
                ctx.set_cursor_icon(if resp.dragged() {
                    egui::CursorIcon::Grabbing
                } else {
                    egui::CursorIcon::Grab
                });
            }
            if !pv.settled() && pv.error().is_none() {
                ctx.request_repaint_after(std::time::Duration::from_millis(30));
            }
            if pv.loading() {
                ui.painter().text(
                    rect.right_bottom() - Vec2::new(10., 10.),
                    Align2::RIGHT_BOTTOM,
                    "Loading 3D view…",
                    theme::sans(10.5),
                    p.muted,
                );
            }
        }
        if self
            .library
            .preview
            .as_ref()
            .is_some_and(|pv| pv.facts().is_some())
        {
            resp.on_hover_text("Drag to turn, scroll to zoom, double-click to reset the view");
        }
        if !shown {
            match self.library.texture(&ctx, m) {
                Some(t) => {
                    let r = rect.shrink(10.);
                    let s = t.size_vec2();
                    let k = (r.width() / s.x).min(r.height() / s.y);
                    ui.painter().image(
                        t.id(),
                        Rect::from_center_size(r.center(), s * k),
                        Rect::from_min_max(Pos2::ZERO, Pos2::new(1., 1.)),
                        egui::Color32::WHITE,
                    );
                }
                None => crate::panels::library::thumb_placeholder(ui, rect, m),
            }
        }
        // Size, triangles and the printers it fits.
        let facts = self
            .library
            .preview
            .as_ref()
            .filter(|pv| pv.id == m.id)
            .and_then(|pv| pv.facts().cloned());
        let size_mm = facts.as_ref().map(|f| f.size).or(m.info.size_mm);
        let triangles = facts
            .as_ref()
            .map(|f| f.triangles as u64)
            .or((m.info.triangles > 0).then_some(m.info.triangles));
        if let Some(s) = size_mm {
            let mut line = crate::library::dims(s);
            if let Some(t) = triangles {
                line = format!("{line} · {} triangles", group_thousands(t));
            }
            if m.kind == mica_library::Kind::ThreeMf && m.info.plates > 1 {
                line.push_str(" · first plate");
            }
            ui.label(
                egui::RichText::new(line)
                    .font(theme::sans(12.))
                    .color(p.label),
            );
            let printers = self.library_printer_volumes();
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = Vec2::new(4., 4.);
                ui.label(
                    egui::RichText::new("Fits")
                        .font(theme::sans(12.))
                        .color(p.muted),
                );
                for (name, volume) in printers {
                    let ok = volume.is_some_and(|v| fits(s, v));
                    let tip = match volume {
                        Some(v) if ok => format!(
                            "Fits the {name}'s build volume ({})",
                            crate::library::dims(v)
                        ),
                        Some(v) => {
                            format!("Too large for the {name} ({})", crate::library::dims(v))
                        }
                        None => format!("The {name}'s build volume is not known"),
                    };
                    fit_chip(ui, &name, ok).on_hover_text(tip);
                }
            });
        }
        // A handle to drag the model onto the plate (dragging the picture turns it).
        if m.kind.openable() || m.kind.is_gcode() {
            let label = if m.kind.is_gcode() {
                "Drag to Prepare to open in Preview"
            } else {
                "Drag onto the plate"
            };
            let g = ui
                .painter()
                .layout_no_wrap(label.into(), theme::sans(11.5), p.muted);
            let (r, handle) =
                ui.allocate_exact_size(Vec2::new(g.size().x + 34., 24.), Sense::click_and_drag());
            let hot = handle.hovered() || handle.dragged();
            ui.painter()
                .rect_filled(r, 7., if hot { p.hover } else { p.raised });
            ui.painter().rect_stroke(
                r,
                7.,
                Stroke::new(1., p.border_raised),
                egui::StrokeKind::Inside,
            );
            // Six grip dots.
            for k in 0..6 {
                let c = Pos2::new(
                    r.left() + 10. + (k % 2) as f32 * 4.,
                    r.center().y - 4. + (k / 2) as f32 * 4.,
                );
                ui.painter().circle_filled(c, 1.2, p.muted);
            }
            ui.painter().galley(
                Pos2::new(r.left() + 24., r.center().y - g.size().y / 2.),
                g,
                p.muted,
            );
            if hot {
                ctx.set_cursor_icon(egui::CursorIcon::Grab);
            }
            self.library_drag_source(&handle, m, None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn fit_and_numbers() {
        assert!(fits([200., 100., 50.], [250., 220., 270.]));
        assert!(
            fits([210., 240., 50.], [250., 220., 270.]),
            "turned on the bed"
        );
        assert!(!fits([260., 100., 50.], [250., 220., 270.]));
        assert!(!fits([100., 100., 280.], [250., 220., 270.]));
        assert_eq!(group_thousands(225_154), "225,154");
        assert_eq!(group_thousands(12), "12");
        assert_eq!(group_thousands(1_000_000), "1,000,000");
        assert_eq!(short_printer("Prusa CORE One HF0.4 nozzle"), "CORE One");
        assert_eq!(short_printer("Snapmaker U1 (0.4 nozzle)"), "U1");
        assert_eq!(short_printer("Custom"), "Custom");
    }

    fn wait(p: &mut Preview, ctx: &egui::Context, until: impl Fn(&Preview) -> bool) -> bool {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(30) {
            p.poll(ctx);
            if until(p) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        false
    }

    #[test]
    fn loads_off_the_ui_thread_and_frees_everything_when_dropped() {
        let ctx = egui::Context::default();
        let path = mica_library::testkit::repo_root().join("tests/models/3DBenchy.stl");
        let textures = || ctx.tex_manager().read().num_allocated();
        let before = textures();
        let mut p = Preview::new("benchy", Kind::Stl, path, &ctx);
        assert!(wait(&mut p, &ctx, |p| p.facts().is_some()));
        let f = p.facts().unwrap().clone();
        assert!(f.triangles > 100_000, "{f:?}");
        assert!(
            (f.size[0] - 60.).abs() < 2. && (f.size[2] - 48.).abs() < 2.,
            "{f:?}"
        );
        p.request([320, 220]);
        assert!(wait(&mut p, &ctx, Preview::settled));
        assert_eq!(p.texture().unwrap().size(), [320, 220]);
        assert_eq!(textures(), before + 1);
        // Turning the view asks for one new picture; the newest wins.
        p.orbit(egui::vec2(40., -10.));
        p.request([320, 220]);
        p.zoom(120.);
        p.request([320, 220]);
        assert!(wait(&mut p, &ctx, Preview::settled));
        assert!(p.view.zoom > 1.);
        p.reset();
        assert_eq!(p.view, View::default());
        drop(p);
        assert_eq!(textures(), before, "the texture is freed with the preview");
    }

    #[test]
    fn unreadable_files_say_why() {
        let ctx = egui::Context::default();
        let dir = mica_library::testkit::TempDir::new("preview-bad");
        let path = dir.path().join("broken.stl");
        std::fs::write(&path, b"not an stl").unwrap();
        let mut p = Preview::new("bad", Kind::Stl, path, &ctx);
        assert!(wait(&mut p, &ctx, |p| p.error().is_some()));
        p.request([100, 100]);
        assert!(p.texture().is_none());
    }
}
