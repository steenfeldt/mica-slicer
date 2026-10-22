// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The Library details panel's 3D preview: a small orbit view of the selected model. The mesh
//! is read and every frame is drawn on a background thread (the CPU renderer of the library
//! thumbnails, `mica_library::render`), so the panel never waits; the UI only uploads the
//! newest picture into one texture. Dropping the preview (another selection, the Library
//! closed) ends the thread and frees the meshes and the texture.
//!
//! A very detailed model is first drawn simplified (vertex clustering finer than a pixel of
//! the first view). Zoomed in (or in a larger panel) past that, a cell would span more than
//! a pixel and a flat face would show creases where a cell also holds a vertex of the next
//! face, so once the zoom has settled the thread draws a finer mesh: the full one when it is
//! light enough to turn smoothly, else one clustered fine enough for the zoom. The model as
//! read stays in memory for that while it is shown.

use crate::theme;
use eframe::egui::{self, Align2, Pos2, Rect, Sense, Stroke, Vec2};
use mica_gcode::thumbnails::{Fit, Orbit};
use mica_library::Kind;
use mica_library::render::PreviewMesh;
use std::path::PathBuf;
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

/// Meshes with more triangles are simplified for the preview's first view (it redraws
/// while dragging).
const PREVIEW_TRIANGLES: usize = 250_000;
/// The picture's height to width (220 × 288 at the details panel's default width).
const PICTURE_ASPECT: f32 = 220. / 288.;
/// The largest picture side rendered, physical pixels: the thumbnail renderer's limit
/// (`mica_gcode::thumbnails`, 1..=999 per side). A wider panel scales the picture up.
const MAX_PICTURE: u16 = 999;
/// How long the wanted picture size must stay put (a window or panel being resized) before
/// it is rendered at that size; the last picture scales meanwhile.
pub const RESIZE_SETTLE: Duration = Duration::from_millis(150);
/// Cells along the largest side when simplifying for the preview's first view.
const PREVIEW_CELLS: u32 = 400;
/// Zoomed in past the simplified mesh's detail, the full mesh is drawn when it has at most
/// this many triangles; a heavier one is clustered finer, to about this many. Measured at the
/// largest picture (999 × 763): a 654k-triangle project draws in 23 ms at the first view and
/// 40 ms zoomed in, 1.3M triangles in 39 ms and 74 ms; a million stays near 15 frames a
/// second while turning a zoomed-in view.
const FULL_DETAIL: usize = 1_000_000;
/// How long the zoom (and the picture size) must stay put before a finer mesh is made.
pub const ZOOM_SETTLE: Duration = Duration::from_millis(150);
/// Each finer mesh is this much finer than the zoom needs, so zooming a little further
/// needs no new one.
const DETAIL_HEADROOM: f32 = 1.25;

/// When the preview simplifies, how far, and how detailed it may get.
#[derive(Clone, Copy, Debug)]
struct Limits {
    simplify_above: usize,
    cells: u32,
    full_detail: usize,
}

const LIMITS: Limits = Limits {
    simplify_above: PREVIEW_TRIANGLES,
    cells: PREVIEW_CELLS,
    full_detail: FULL_DETAIL,
};

/// How long the embedded thumbnail fades into the coloured 3D view.
const CROSSFADE: Duration = Duration::from_millis(250);

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
    /// Drawn in the project's filament colours (else in the accent, like an STL).
    pub coloured: bool,
}

enum State {
    Loading,
    Ready(Facts),
    Failed(String),
}

/// What the UI asks of the drawing thread.
enum Request {
    /// A picture of this view and size.
    Draw(View, [u16; 2]),
    /// A mesh at least this detailed (cells along the largest side, see [`Meshes`]).
    Detail(u32),
}

/// The facts, the detail of the first mesh drawn, and the model as read (held by the
/// thread; tests check it is freed).
type Loaded = (Facts, Option<u32>, std::sync::Weak<PreviewMesh>);

enum Reply {
    Loaded(Result<Loaded, String>),
    /// The mesh drawn now has this detail; `true`: the finest it will get.
    Detail(Option<u32>, bool),
    Frame(View, [u16; 2], egui::ColorImage),
    /// The picture for this view and size could not be drawn.
    Failed(View, [u16; 2], String),
}

/// The picture size rendered for a wanted `size`: within [`MAX_PICTURE`] on its longer side,
/// in the same aspect.
fn picture_size([w, h]: [u16; 2]) -> [u16; 2] {
    let k = (f32::from(MAX_PICTURE) / f32::from(w.max(h).max(1))).min(1.);
    [w, h].map(|v| ((f32::from(v) * k).round() as u16).clamp(16, MAX_PICTURE))
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
    /// A new picture size waiting to stay put for [`RESIZE_SETTLE`], and since when.
    resizing: Option<([u16; 2], Instant)>,
    /// Why the last picture could not be drawn (cleared by the next one that is).
    render_error: Option<String>,
    /// When the first 3D picture arrived (the thumbnail fades out from then).
    first_frame: Option<Instant>,
    /// The detail of the mesh drawn: `None` the full mesh, else the clustering grid's cells
    /// along the largest side.
    detail: Option<u32>,
    /// The finest detail asked for; `u32::MAX` once the thread has said it goes no finer.
    asked: u32,
    /// A finer mesh wanted for this zoom and picture size, since when: asked for once they
    /// have stayed put for [`ZOOM_SETTLE`].
    refine: Option<(f32, [u16; 2], Instant)>,
    #[cfg(test)]
    retained: std::sync::Weak<PreviewMesh>,
    tx: mpsc::Sender<Request>,
    rx: mpsc::Receiver<Reply>,
}

/// The meshes the drawing thread holds while the preview shows a model: the model as read
/// and the one drawn (the same one, or a simplified copy).
struct Meshes {
    full: Arc<PreviewMesh>,
    drawn: Arc<PreviewMesh>,
    /// `None`: `drawn` is `full`; else the cells along the largest side it was clustered to.
    detail: Option<u32>,
    limits: Limits,
}

impl Meshes {
    /// A detailed model is drawn simplified at first; the rest as it is.
    fn new(full: PreviewMesh, limits: Limits) -> Self {
        let full = Arc::new(full);
        if full.mesh.faces.len() > limits.simplify_above {
            Self {
                drawn: Arc::new(full.simplified(limits.cells)),
                full,
                detail: Some(limits.cells),
                limits,
            }
        } else {
            Self {
                drawn: full.clone(),
                full,
                detail: None,
                limits,
            }
        }
    }

    /// Draws at least `cells` detail from now on: the full mesh when it is within the
    /// full-detail budget, else the model clustered to `cells` (coarser when that would be
    /// well over the budget). Face colours are kept. Returns whether this is the finest the
    /// preview gets.
    fn refine(&mut self, cells: u32) -> bool {
        let Some(now) = self.detail else {
            return true;
        };
        if now >= cells {
            return false;
        }
        let budget = self.limits.full_detail;
        // The coarser mesh goes first: nothing draws while this thread works.
        self.drawn = self.full.clone();
        self.detail = None;
        if self.full.mesh.faces.len() <= budget {
            return true;
        }
        let mut cells = cells;
        let mut finer = self.full.simplified(cells);
        let mut finest = false;
        // Clustered triangles grow about with the area of a cell (cells squared) until the
        // cells get finer than the model's own triangles: a few coarser tries at most.
        for _ in 0..8 {
            if finer.mesh.faces.len() <= budget || cells <= now {
                break;
            }
            let k = (budget as f32 / finer.mesh.faces.len() as f32).sqrt() * 0.95;
            cells = ((cells as f32 * k.min(0.7)) as u32).max(now);
            drop(finer);
            finer = self.full.simplified(cells);
            finest = true;
        }
        self.drawn = Arc::new(finer);
        self.detail = Some(cells);
        finest
    }
}

/// One picture of the preview mesh: in its face colours, else in the accent.
pub fn render(
    m: &PreviewMesh,
    w: u16,
    h: u16,
    orbit: Orbit,
) -> std::io::Result<mica_gcode::thumbnails::Image> {
    match &m.face_colours {
        Some(c) => mica_gcode::thumbnails::render_orbit_colored(&m.mesh, w, h, c, orbit),
        None => {
            mica_gcode::thumbnails::render_orbit(&m.mesh, w, h, mica_library::render::VIOLET, orbit)
        }
    }
}

impl Preview {
    /// Starts reading `path` on a background thread (read-only).
    pub fn new(id: &str, kind: Kind, path: PathBuf, ctx: &egui::Context) -> Self {
        Self::with_limits(id, kind, path, ctx, LIMITS)
    }

    fn with_limits(
        id: &str,
        kind: Kind,
        path: PathBuf,
        ctx: &egui::Context,
        limits: Limits,
    ) -> Self {
        let (tx, requests) = mpsc::channel::<Request>();
        let (replies, rx) = mpsc::channel();
        let ctx = ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("mica-library-preview".into())
            .spawn(move || {
                let mut meshes = match mica_library::render::load_preview_file(kind, &path) {
                    Ok(m) => Meshes::new(m, limits),
                    Err(e) => {
                        let _ = replies.send(Reply::Loaded(Err(e)));
                        ctx.request_repaint();
                        return;
                    }
                };
                let m = &meshes.full;
                let (lo, hi) = m.mesh.bounds();
                let facts = Facts {
                    size: std::array::from_fn(|i| hi[i] - lo[i]),
                    triangles: m.triangles,
                    coloured: m.face_colours.is_some(),
                };
                let loaded = (facts, meshes.detail, Arc::downgrade(m));
                if replies.send(Reply::Loaded(Ok(loaded))).is_err() {
                    return;
                }
                ctx.request_repaint();
                // Draw the newest request (after a finer mesh, if one was asked for); end
                // when the preview is dropped.
                let mut last = None;
                while let Ok(first) = requests.recv() {
                    let (mut draw, mut detail) = (None, None);
                    for req in std::iter::once(first).chain(requests.try_iter()) {
                        match req {
                            Request::Draw(view, size) => draw = Some((view, size)),
                            Request::Detail(cells) => detail = detail.max(Some(cells)),
                        }
                    }
                    if let Some(cells) = detail {
                        let finest = meshes.refine(cells);
                        if replies.send(Reply::Detail(meshes.detail, finest)).is_err() {
                            return;
                        }
                        // Show the finer mesh in the view shown.
                        draw = draw.or(last);
                    }
                    let Some(req) = draw else {
                        continue;
                    };
                    last = Some(req);
                    let (view, [w, h]) = req;
                    let orbit = Orbit {
                        azimuth: view.azimuth,
                        elevation: view.elevation,
                        zoom: view.zoom,
                        fit: Fit::Sphere,
                    };
                    let img = match render(&meshes.drawn, w, h, orbit) {
                        Ok(img) => img,
                        Err(e) => {
                            if replies
                                .send(Reply::Failed(view, [w, h], e.to_string()))
                                .is_err()
                            {
                                return;
                            }
                            ctx.request_repaint();
                            continue;
                        }
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
            resizing: None,
            render_error: None,
            first_frame: None,
            detail: None,
            asked: 0,
            refine: None,
            #[cfg(test)]
            retained: std::sync::Weak::new(),
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
                Reply::Loaded(Ok((f, detail, _full))) => {
                    self.state = State::Ready(f);
                    self.detail = detail;
                    self.asked = detail.unwrap_or(u32::MAX);
                    #[cfg(test)]
                    {
                        self.retained = _full;
                    }
                }
                Reply::Loaded(Err(e)) => self.state = State::Failed(e),
                Reply::Detail(detail, finest) => {
                    // The thread draws the shown view again with the new mesh.
                    self.shown = None;
                    self.detail = detail;
                    if finest || detail.is_none() {
                        self.asked = u32::MAX;
                    }
                }
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
                    self.render_error = None;
                    self.first_frame.get_or_insert_with(Instant::now);
                }
                Reply::Failed(view, size, e) => {
                    self.shown = Some((view, size));
                    self.render_error = Some(e);
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

    /// Why the newest picture could not be drawn.
    pub fn render_error(&self) -> Option<&str> {
        self.render_error.as_deref()
    }

    /// The view and picture size last asked for.
    #[cfg(test)]
    pub fn requested(&self) -> Option<(View, [u16; 2])> {
        self.requested
    }

    pub fn loading(&self) -> bool {
        matches!(self.state, State::Loading)
    }

    pub fn texture(&self) -> Option<&egui::TextureHandle> {
        self.texture.as_ref()
    }

    /// How far the 3D view has faded in over the thumbnail (0 to 1). A coloured project
    /// fades in; a model drawn in the accent (an STL) replaces the thumbnail at once.
    pub fn fade(&self) -> f32 {
        match (self.first_frame, self.facts()) {
            (Some(t), Some(f)) if f.coloured => {
                (t.elapsed().as_secs_f32() / CROSSFADE.as_secs_f32()).min(1.)
            }
            (Some(_), _) => 1.,
            (None, _) => 0.,
        }
    }

    /// The picture shown is the newest one asked for.
    pub fn settled(&self) -> bool {
        self.shown.is_some() && self.shown == self.requested && self.resizing.is_none()
    }

    /// Asks for a picture of `size` physical pixels in the current view (once per change).
    /// A turn or zoom is asked for at once; a new size only once it has stayed put for
    /// [`RESIZE_SETTLE`] (the last picture scales meanwhile). A finer mesh, when the zoom
    /// and size need one, once they have stayed put for [`ZOOM_SETTLE`] (the current mesh
    /// draws meanwhile). Returns how long until something waiting is due, for the caller
    /// to repaint then.
    pub fn request(&mut self, size: [u16; 2]) -> Option<Duration> {
        self.request_at(size, Instant::now())
    }

    fn request_at(&mut self, size: [u16; 2], now: Instant) -> Option<Duration> {
        let model = self.facts()?.size;
        let size = picture_size(size);
        let refine = self.refine_at(model, size, now);
        let draw = self.draw_at(size, now);
        match (refine, draw) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// Asks for a finer mesh once a clustering cell of the mesh drawn would span more than
    /// a pixel at the current zoom and picture size, and they have stayed put.
    fn refine_at(&mut self, model: [f32; 3], size: [u16; 2], now: Instant) -> Option<Duration> {
        let wanted = mica_library::render::preview_cells(model, size, self.view.zoom);
        if self.detail.is_none() || wanted <= self.asked {
            self.refine = None;
            return None;
        }
        let since = match self.refine {
            Some((zoom, s, t)) if zoom == self.view.zoom && s == size => t,
            _ => now,
        };
        self.refine = Some((self.view.zoom, size, since));
        let left = ZOOM_SETTLE.saturating_sub(now.duration_since(since));
        if !left.is_zero() {
            return Some(left);
        }
        self.refine = None;
        let cells = (wanted as f32 * DETAIL_HEADROOM).ceil() as u32;
        self.asked = cells;
        let _ = self.tx.send(Request::Detail(cells));
        None
    }

    fn draw_at(&mut self, size: [u16; 2], now: Instant) -> Option<Duration> {
        // A new size of a picture already shown, the view unchanged: wait for it to settle.
        if let Some((view, last)) = self.requested
            && last != size
            && view == self.view
            && self.texture.is_some()
        {
            let since = match self.resizing {
                Some((s, t)) if s == size => t,
                _ => now,
            };
            self.resizing = Some((size, since));
            let left = RESIZE_SETTLE.saturating_sub(now.duration_since(since));
            if !left.is_zero() {
                return Some(left);
            }
        }
        self.resizing = None;
        let want = (self.view, size);
        if self.requested != Some(want) {
            self.requested = Some(want);
            let _ = self.tx.send(Request::Draw(want.0, want.1));
        }
        None
    }

    /// Asks at once for a finer mesh that waits for the zoom to settle (screenshots). Returns
    /// whether a finer mesh is still on its way.
    pub fn refine_pending(&mut self) -> bool {
        if let Some((_, size, since)) = self.refine
            && let Some(f) = self.facts()
        {
            let model = f.size;
            self.refine_at(model, size, since + ZOOM_SETTLE);
        }
        self.asked != u32::MAX && self.detail.is_some_and(|d| d < self.asked)
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
        let w = ui.available_width();
        let (rect, resp) = ui.allocate_exact_size(
            Vec2::new(w, (w * PICTURE_ASPECT).round()),
            Sense::click_and_drag(),
        );
        ui.painter().rect_filled(rect, 8., p.search_bg);
        let ppp = ctx.pixels_per_point();
        let size = [
            (rect.width() * ppp).round() as u16,
            (rect.height() * ppp).round() as u16,
        ];
        // The 3D view's opacity over the thumbnail (0: only the thumbnail).
        let mut fade = 0.;
        if let Some(pv) = self.library.preview.as_mut().filter(|pv| pv.id == m.id) {
            if resp.dragged() {
                pv.orbit(resp.drag_delta());
            }
            if resp.hovered() || resp.dragged() {
                // Scroll or pinch zooms; the details' scroll area must not also move under it.
                let (scroll, pinch) = ui.input_mut(|i| {
                    let s = i.smooth_scroll_delta.y;
                    if pv.facts().is_some() {
                        i.smooth_scroll_delta = Vec2::ZERO;
                    }
                    (s, i.zoom_delta())
                });
                if scroll != 0. {
                    pv.zoom(scroll);
                }
                if pinch != 1. && pinch > 0. {
                    pv.zoom(pinch.ln() / 0.004);
                }
            }
            if resp.double_clicked() {
                pv.reset();
            }
            // While the panel's edge is dragged the last picture scales; the new size renders
            // once it has stayed put a moment after the drag (also a resized window). A turn
            // or zoom is drawn at once, at the current size.
            let wait = if self.library.detail_resizing && pv.texture().is_some() {
                Some(crate::library_preview::RESIZE_SETTLE)
            } else {
                pv.request(size)
            };
            if let Some(wait) = wait {
                ctx.request_repaint_after(wait);
            }
            if pv.texture().is_some() {
                fade = pv.fade();
            }
            if pv.facts().is_some() && (resp.hovered() || resp.dragged()) {
                ctx.set_cursor_icon(if resp.dragged() {
                    egui::CursorIcon::Grabbing
                } else {
                    egui::CursorIcon::Grab
                });
            }
            if (!pv.settled() && pv.error().is_none()) || (fade > 0. && fade < 1.) {
                ctx.request_repaint_after(std::time::Duration::from_millis(16));
            }
        }
        if fade < 1. {
            let tint = egui::Color32::WHITE.gamma_multiply(1. - fade);
            match self.library.texture(&ctx, m) {
                Some(t) => {
                    let r = rect.shrink(10.);
                    let s = t.size_vec2();
                    let k = (r.width() / s.x).min(r.height() / s.y);
                    ui.painter().image(
                        t.id(),
                        Rect::from_center_size(r.center(), s * k),
                        Rect::from_min_max(Pos2::ZERO, Pos2::new(1., 1.)),
                        tint,
                    );
                }
                None if fade == 0. => crate::panels::library::thumb_placeholder(ui, rect, m),
                None => {}
            }
        }
        if let Some(pv) = self.library.preview.as_ref().filter(|pv| pv.id == m.id) {
            if fade > 0.
                && let Some(t) = pv.texture()
            {
                let s = t.size_vec2() / ppp;
                let k = (rect.width() / s.x).min(rect.height() / s.y);
                ui.painter().image(
                    t.id(),
                    Rect::from_center_size(rect.center(), s * k),
                    Rect::from_min_max(Pos2::ZERO, Pos2::new(1., 1.)),
                    egui::Color32::WHITE.gamma_multiply(fade),
                );
            }
            if let Some(e) = pv.render_error() {
                ui.painter().text(
                    rect.right_bottom() - Vec2::new(10., 10.),
                    Align2::RIGHT_BOTTOM,
                    format!("3D view failed: {e}"),
                    theme::sans(10.5),
                    p.warn_text,
                );
            } else if pv.loading() {
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
    fn projects_show_their_filament_colours_and_fade_in() {
        let ctx = egui::Context::default();
        let dir = mica_library::testkit::TempDir::new("preview-colours");
        let path = dir.path().join("coloured.3mf");
        std::fs::write(&path, mica_library::testkit::coloured_3mf()).unwrap();
        let mut p = Preview::new("coloured", Kind::ThreeMf, path.clone(), &ctx);
        assert!(wait(&mut p, &ctx, |p| p.facts().is_some()));
        let f = p.facts().unwrap().clone();
        assert!(f.coloured && f.triangles == 24, "{f:?}");
        assert_eq!(p.fade(), 0., "the thumbnail shows until the first picture");
        p.request([200, 120]);
        assert!(wait(&mut p, &ctx, Preview::settled));
        assert!(p.fade() < 1., "a coloured project fades in");
        std::thread::sleep(CROSSFADE);
        assert_eq!(p.fade(), 1.);
        // The picture: the project's tan, black and no accent violet.
        let m = mica_library::render::load_preview_file(Kind::ThreeMf, &path).unwrap();
        let img = render(&m, 200, 120, mica_library::render::VIEW).unwrap();
        let near = |c: [u8; 3]| {
            img.rgba.chunks(4).any(|px| {
                px[3] == 255
                    && (0..3).all(|i| {
                        // Shading scales the colour; compare the hue by ratios.
                        let k = f32::from(px[0].max(1)) / f32::from(c[0].max(1));
                        (f32::from(px[i]) - f32::from(c[i]) * k).abs() < 12.
                    })
            })
        };
        assert!(near([0xCB, 0x8F, 0x29]), "tool 1's colour shows");
        let violet = mica_library::render::VIOLET;
        assert!(
            !img.rgba
                .chunks(4)
                .any(|px| px[3] == 255 && px[2] > px[0] + 40 && px[2] > violet[1])
        );
        // A bare STL swaps at once, in the accent.
        let stl = mica_library::testkit::repo_root().join("tests/models/cube20.stl");
        let mut p = Preview::new("stl", Kind::Stl, stl, &ctx);
        assert!(wait(&mut p, &ctx, |p| p.facts().is_some()));
        assert!(!p.facts().unwrap().coloured);
        p.request([64, 64]);
        assert!(wait(&mut p, &ctx, Preview::settled));
        assert_eq!(p.fade(), 1.);
    }

    #[test]
    fn wide_panels_render_within_the_renderer_limit_after_settling() {
        let ctx = egui::Context::default();
        let path = mica_library::testkit::repo_root().join("tests/models/cube20.stl");
        let mut p = Preview::new("cube", Kind::Stl, path, &ctx);
        assert!(wait(&mut p, &ctx, |p| p.facts().is_some()));
        // The first picture is asked for at once.
        let t0 = Instant::now();
        assert_eq!(p.request_at([576, 440], t0), None);
        assert!(wait(&mut p, &ctx, Preview::settled));
        // A wider panel on a 2x screen: more than the renderer's 999 pixels. The size waits
        // until it stays put, then renders scaled into the limit, in the same aspect.
        let wide = [1800, 1375];
        assert!(p.request_at(wide, t0).is_some());
        assert!(p.request_at(wide, t0 + RESIZE_SETTLE / 2).is_some());
        assert!(!p.settled());
        assert_eq!(p.request_at(wide, t0 + RESIZE_SETTLE), None);
        assert!(wait(&mut p, &ctx, Preview::settled));
        assert_eq!(p.render_error(), None);
        let [w, h] = p.texture().unwrap().size();
        assert_eq!(w, 999);
        assert!(
            (h as f32 / w as f32 - 1375. / 1800.).abs() < 0.01,
            "{w}x{h}"
        );
        // Turning while a size settles draws at once, at the new size.
        p.request_at([1000, 800], t0 + RESIZE_SETTLE);
        p.orbit(egui::vec2(30., 0.));
        assert_eq!(p.request_at([1000, 800], t0 + RESIZE_SETTLE), None);
        assert_eq!(p.requested().unwrap().1, [999, 799]);
        assert!(wait(&mut p, &ctx, Preview::settled));
        assert_eq!(p.render_error(), None);
    }

    /// A block 12 mm square and 10 mm high whose flat top meets a 45° chamfer along +x (the
    /// side the preview's view looks at), tessellated at about 0.1 mm, the chamfer listed
    /// first (so a clustering cell straddling the edge keeps a chamfer vertex below the top),
    /// plus a dense
    /// patch of tiny triangles on the bottom that clustering removes. Triangles of the top,
    /// the chamfer and the patch.
    fn chamfered_block() -> (Vec<[[f32; 3]; 3]>, [usize; 3]) {
        let (edge, top, rows) = (9.65f32, 10f32, 120);
        let mut tris = vec![];
        // From the far side in: the chamfer's lowest vertex in a cell comes first.
        let quads = |nu: usize, at: &dyn Fn(f32, f32) -> [f32; 3], tris: &mut Vec<_>| {
            for i in (0..nu).rev() {
                for j in 0..rows {
                    let p = |i: usize, j: usize| at(i as f32 / nu as f32, j as f32 / rows as f32);
                    tris.push([p(i, j), p(i + 1, j), p(i + 1, j + 1)]);
                    tris.push([p(i, j), p(i + 1, j + 1), p(i, j + 1)]);
                }
            }
        };
        let side = 12. - edge;
        quads(
            24,
            &|u, v| [edge + u * side, v * 12., top - u * side],
            &mut tris,
        );
        let chamfer = tris.len();
        quads(97, &|u, v| [u * edge, v * 12., top], &mut tris);
        let flat = tris.len() - chamfer;
        quads(
            4,
            &|u, v| [12., v * 12., (top - side) * (1. - u)],
            &mut tris,
        );
        // The patch: 0.05 mm across, facing down.
        let before = tris.len();
        for i in 0..100 {
            for j in 0..100 {
                let p = |i: usize, j: usize| [3. + i as f32 * 5e-4, 3. + j as f32 * 5e-4, 0.];
                tris.push([p(i, j), p(i + 1, j + 1), p(i + 1, j)]);
                tris.push([p(i, j), p(i, j + 1), p(i + 1, j + 1)]);
            }
        }
        let patch = tris.len() - before;
        (tris, [flat, chamfer, patch])
    }

    fn stl(tris: &[[[f32; 3]; 3]]) -> Vec<u8> {
        let mut b = vec![0u8; 80];
        b.extend((tris.len() as u32).to_le_bytes());
        for t in tris {
            b.extend([0u8; 12]);
            for v in t.iter().flatten() {
                b.extend(v.to_le_bytes());
            }
            b.extend([0u8; 2]);
        }
        b
    }

    const RED: [u8; 3] = [200, 40, 40];
    const BLUE: [u8; 3] = [40, 40, 200];
    const GREEN: [u8; 3] = [40, 200, 40];

    /// The block in colours: the top red, the chamfer and the wall blue, the patch green.
    fn coloured_block() -> PreviewMesh {
        let (tris, [flat, chamfer, patch]) = chamfered_block();
        let mesh = mica_core::mesh::TriangleMesh::from_stl_bytes(&stl(&tris)).unwrap();
        let wall = tris.len() - flat - chamfer - patch;
        let colours: Vec<[u8; 3]> = std::iter::repeat_n(BLUE, chamfer)
            .chain(std::iter::repeat_n(RED, flat))
            .chain(std::iter::repeat_n(BLUE, wall))
            .chain(std::iter::repeat_n(GREEN, patch))
            .collect();
        assert_eq!(mesh.faces.len(), colours.len());
        PreviewMesh {
            triangles: mesh.faces.len(),
            mesh,
            face_colours: Some(colours),
        }
    }

    const PICTURE: [u16; 2] = [600, 450];

    /// Pixels of the flat top (exactly its shade in the full mesh's picture) shaded otherwise
    /// in `m`'s picture, and how many top pixels there are.
    fn creased(m: &PreviewMesh, full: &PreviewMesh, zoom: f32) -> (usize, usize) {
        let orbit = Orbit {
            zoom,
            fit: Fit::Sphere,
            ..mica_library::render::VIEW
        };
        let [w, h] = PICTURE;
        let reference = render(full, w, h, orbit).unwrap();
        let img = render(m, w, h, orbit).unwrap();
        // The top's shade: the most common opaque red-dominant colour of the reference.
        let mut counts = std::collections::HashMap::new();
        for px in reference.rgba.chunks(4) {
            if px[3] == 255 && px[0] > px[2] + 40 {
                *counts.entry([px[0], px[1], px[2]]).or_insert(0usize) += 1;
            }
        }
        let shade = counts.into_iter().max_by_key(|&(_, n)| n).unwrap().0;
        let mut top = 0;
        let mut off = 0;
        for (r, px) in reference.rgba.chunks(4).zip(img.rgba.chunks(4)) {
            if r[3] == 255 && r[..3] == shade {
                top += 1;
                if (0..3).any(|i| px[i].abs_diff(shade[i]) > 3) || px[3] != 255 {
                    off += 1;
                }
            }
        }
        (off, top)
    }

    #[test]
    fn zoomed_in_flat_faces_show_no_creases_after_refining() {
        let full = coloured_block();
        let limits = |full_detail| Limits {
            simplify_above: 1000,
            cells: 24,
            full_detail,
        };
        let [flat, chamfer, patch] = chamfered_block().1;
        let size = [12., 12., 10.];
        let zoom = 2.;
        let wanted = mica_library::render::preview_cells(size, PICTURE, zoom);
        // Before: the first view's mesh, its cells several pixels wide at this zoom.
        let first = Meshes::new(full.clone(), limits(usize::MAX));
        assert_eq!(first.detail, Some(24));
        let (before, top) = creased(&first.drawn, &full, zoom);
        assert!(top > 20_000, "{top} top pixels");
        assert!(
            before > 1000,
            "the first mesh shows creases: {before} of {top}"
        );
        // After, within the full-detail budget: the full mesh itself.
        let mut m = Meshes::new(full.clone(), limits(full.mesh.faces.len()));
        assert!(m.refine(wanted));
        assert_eq!(m.detail, None);
        assert!(Arc::ptr_eq(&m.drawn, &m.full));
        assert_eq!(creased(&m.drawn, &full, zoom).0, 0);
        // After, over the budget: clustered to at most a pixel, colours kept.
        let mut m = Meshes::new(full.clone(), limits(flat + chamfer + 5000));
        assert!(!m.refine(wanted), "within the budget at this detail");
        let cells = m.detail.unwrap();
        let cell_px =
            12. / cells as f32 * mica_library::render::preview_px_per_mm(size, PICTURE, zoom);
        assert!(
            cells >= wanted && cell_px <= 1.,
            "{cells} cells: {cell_px} px"
        );
        let drawn = &m.drawn;
        assert!(drawn.mesh.faces.len() < full.mesh.faces.len() - patch / 2);
        let c = drawn.face_colours.as_ref().unwrap();
        assert_eq!(c.len(), drawn.mesh.faces.len());
        let n = |k: [u8; 3]| c.iter().filter(|&&x| x == k).count();
        assert_eq!(n(RED), flat, "every top face keeps its colour");
        assert!(n(GREEN) < patch / 2);
        let (after, _) = creased(drawn, &full, zoom);
        assert!(
            after * 50 < before,
            "creases: {before} pixels before, {after} after"
        );
        // A budget too small for the zoom: as fine as the budget allows, and said so.
        let mut m = Meshes::new(full.clone(), limits(4000));
        assert!(m.refine(wanted));
        let cells = m.detail.unwrap();
        assert!(cells >= 24 && cells < wanted, "{cells}");
        assert!(m.drawn.mesh.faces.len() <= 4000);
        assert_eq!(
            m.drawn.face_colours.as_ref().unwrap().len(),
            m.drawn.mesh.faces.len()
        );
    }

    #[test]
    fn zooming_in_asks_for_a_finer_mesh_once_the_zoom_settles() {
        let ctx = egui::Context::default();
        let dir = mica_library::testkit::TempDir::new("preview-zoom");
        let path = dir.path().join("block.stl");
        std::fs::write(&path, stl(&chamfered_block().0)).unwrap();
        let limits = |full_detail| Limits {
            simplify_above: 1000,
            cells: 24,
            full_detail,
        };
        let mut p = Preview::with_limits("block", Kind::Stl, path.clone(), &ctx, limits(10));
        assert!(wait(&mut p, &ctx, |p| p.facts().is_some()));
        assert_eq!(p.detail, Some(24));
        // A small picture at the first view needs no finer mesh.
        let t0 = Instant::now();
        assert_eq!(p.request_at([40, 30], t0), None);
        assert!(p.refine.is_none());
        assert!(wait(&mut p, &ctx, Preview::settled));
        // Zooming in: each step draws at once, the finer mesh waits for the zoom to settle.
        let mut t = t0;
        for _ in 0..5 {
            p.zoom(100.);
            t += ZOOM_SETTLE / 2;
            assert!(p.request_at(PICTURE, t).is_some_and(|w| w <= ZOOM_SETTLE));
            assert_eq!(p.asked, 24, "no finer mesh while the zoom changes");
        }
        assert!(p.request_at(PICTURE, t + ZOOM_SETTLE / 2).is_some());
        assert_eq!(p.request_at(PICTURE, t + ZOOM_SETTLE), None);
        let wanted = mica_library::render::preview_cells([12., 12., 10.], PICTURE, p.view.zoom);
        assert!(p.asked >= wanted, "{} < {wanted}", p.asked);
        // The budget (10 triangles) stops it at its finest; it is not asked again.
        assert!(wait(&mut p, &ctx, |p| p.asked == u32::MAX));
        assert!(p.detail.unwrap() >= 24);
        assert!(wait(&mut p, &ctx, Preview::settled));
        p.zoom(200.);
        assert_eq!(p.request_at(PICTURE, t + ZOOM_SETTLE * 4), None);
        assert!(p.refine.is_none());
        // The model as read is held while shown and freed with the preview.
        let retained = p.retained.clone();
        assert!(retained.upgrade().is_some());
        drop(p);
        let start = Instant::now();
        while retained.strong_count() > 0 && start.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            retained.strong_count(),
            0,
            "another selection frees the model"
        );
        // Within the full-detail budget the full mesh is drawn, then nothing is asked again.
        let mut p = Preview::with_limits("block", Kind::Stl, path, &ctx, limits(1_000_000));
        assert!(wait(&mut p, &ctx, |p| p.facts().is_some()));
        p.zoom(300.);
        let t0 = Instant::now();
        assert!(p.request_at(PICTURE, t0).is_some());
        assert_eq!(p.request_at(PICTURE, t0 + ZOOM_SETTLE), None);
        assert!(wait(&mut p, &ctx, |p| p.detail.is_none()));
        assert_eq!(p.asked, u32::MAX);
        assert!(wait(&mut p, &ctx, Preview::settled));
        assert_eq!(p.render_error(), None);
        // Zooming back out keeps the full mesh.
        p.reset();
        assert_eq!(p.request_at(PICTURE, t0 + ZOOM_SETTLE * 2), None);
        assert!(wait(&mut p, &ctx, Preview::settled));
        assert_eq!(p.detail, None);
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
