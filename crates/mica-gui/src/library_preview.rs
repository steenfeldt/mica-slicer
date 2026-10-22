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
//!
//! A project with several plates shows its first plate and a switcher; another plate is made
//! from the project as read (kept while the preview shows it, so a switch does not read and
//! unzip the file again) through the same pipeline, and fades in over the last picture once
//! its first frame is drawn. Only the shown plate's meshes are kept. The view (turn and zoom)
//! stays when switching: the zoom is relative to each plate's framing, so the new plate shows
//! from the same side at the same relative size.

use crate::theme;
use eframe::egui::{self, Align2, Pos2, Rect, Sense, Stroke, Vec2};
use mica_gcode::thumbnails::{Fit, Orbit};
use mica_library::Kind;
use mica_library::render::{PreviewMesh, PreviewProject};
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

/// How long the embedded thumbnail fades into the coloured 3D view (and one plate into
/// another).
const CROSSFADE: Duration = Duration::from_millis(250);

/// A plate that takes longer than this to show says it is loading.
const SAY_LOADING: Duration = Duration::from_millis(150);

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
    /// A mesh of this plate at least this detailed (cells along the largest side, see
    /// [`Meshes`]); ignored once another plate is drawn.
    Detail(usize, u32),
    /// Show this plate (zero-based) from now on.
    Plate(usize),
}

/// The facts, the detail of the first mesh drawn, and the model as read (held by the
/// thread; tests check it is freed).
type Loaded = (Facts, Option<u32>, std::sync::Weak<PreviewMesh>);

enum Reply {
    /// A project's plates by name (only for more than one), and the project as read (held by
    /// the thread; tests check it is freed).
    Plates(Vec<String>, std::sync::Weak<PreviewProject>),
    /// This plate's mesh (zero-based; 0 for a model without plates) is what is drawn now.
    Loaded(usize, Result<Loaded, String>),
    /// The mesh drawn now has this detail; `true`: the finest it will get.
    Detail(Option<u32>, bool),
    /// A picture of this plate.
    Frame(usize, View, [u16; 2], egui::ColorImage),
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
    /// A project's plates by name (empty: one plate, or not a project).
    plates: Vec<String>,
    /// The plate wanted (zero-based), the one the thread's mesh is of (what `state` and
    /// `detail` describe), and the one of the picture shown with its facts.
    plate: usize,
    thread_plate: usize,
    shown_plate: Option<usize>,
    shown_facts: Option<Facts>,
    /// The last plate's picture while the new one fades in over it, and since when.
    previous: Option<egui::TextureHandle>,
    switched: Option<Instant>,
    /// When the plate wanted was asked for.
    plate_asked: Option<Instant>,
    #[cfg(test)]
    retained_project: std::sync::Weak<PreviewProject>,
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
                // A project with several plates is kept as read, to make another plate.
                let (first, project) = match kind {
                    Kind::ThreeMf => match PreviewProject::read_file(&path) {
                        Ok(p) => {
                            let first = p.plate(0);
                            let names = p.plate_names().to_vec();
                            if names.len() > 1 {
                                let p = Arc::new(p);
                                let weak = Arc::downgrade(&p);
                                if replies.send(Reply::Plates(names, weak)).is_err() {
                                    return;
                                }
                                (first, Some(p))
                            } else {
                                (first, None)
                            }
                        }
                        Err(e) => (Err(e), None),
                    },
                    _ => (mica_library::render::load_preview_file(kind, &path), None),
                };
                // Sends what was loaded; the meshes to draw, if any.
                let loaded = |plate: usize, m: Result<PreviewMesh, String>| {
                    let (meshes, reply) = match m {
                        Ok(m) => {
                            let meshes = Meshes::new(m, limits);
                            let m = &meshes.full;
                            let (lo, hi) = m.mesh.bounds();
                            let facts = Facts {
                                size: std::array::from_fn(|i| hi[i] - lo[i]),
                                triangles: m.triangles,
                                coloured: m.face_colours.is_some(),
                            };
                            let reply = Ok((facts, meshes.detail, Arc::downgrade(m)));
                            (Some(meshes), reply)
                        }
                        Err(e) => (None, Err(e)),
                    };
                    let sent = replies.send(Reply::Loaded(plate, reply)).is_ok();
                    ctx.request_repaint();
                    sent.then_some(meshes)
                };
                let Some(mut meshes) = loaded(0, first) else {
                    return;
                };
                if meshes.is_none() && project.is_none() {
                    return;
                }
                let mut plate = 0;
                // Draw the newest request (after another plate or a finer mesh, if one was
                // asked for); end when the preview is dropped.
                let mut last = None;
                while let Ok(first) = requests.recv() {
                    let (mut draw, mut details, mut switch) = (None, vec![], None);
                    for req in std::iter::once(first).chain(requests.try_iter()) {
                        match req {
                            Request::Draw(view, size) => draw = Some((view, size)),
                            Request::Detail(p, cells) => details.push((p, cells)),
                            Request::Plate(p) => switch = Some(p),
                        }
                    }
                    if let (Some(p), Some(project)) = (switch, &project)
                        && (p != plate || meshes.is_none())
                    {
                        // Only the shown plate's meshes are kept: the last ones go first.
                        drop(meshes.take());
                        plate = p;
                        match loaded(p, project.plate(p)) {
                            Some(m) => meshes = m,
                            None => return,
                        }
                    }
                    let Some(meshes) = meshes.as_mut() else {
                        continue;
                    };
                    let detail = details
                        .into_iter()
                        .filter(|&(p, _)| p == plate)
                        .map(|(_, cells)| cells)
                        .max();
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
                    if replies
                        .send(Reply::Frame(plate, view, [w, h], image))
                        .is_err()
                    {
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
            plates: vec![],
            plate: 0,
            thread_plate: 0,
            shown_plate: None,
            shown_facts: None,
            previous: None,
            switched: None,
            plate_asked: None,
            #[cfg(test)]
            retained: std::sync::Weak::new(),
            #[cfg(test)]
            retained_project: std::sync::Weak::new(),
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
                Reply::Plates(names, _project) => {
                    self.plates = names;
                    #[cfg(test)]
                    {
                        self.retained_project = _project;
                    }
                }
                Reply::Loaded(plate, loaded) => {
                    self.thread_plate = plate;
                    self.refine = None;
                    match loaded {
                        Ok((f, detail, _full)) => {
                            self.state = State::Ready(f);
                            self.detail = detail;
                            self.asked = detail.unwrap_or(u32::MAX);
                            #[cfg(test)]
                            {
                                self.retained = _full;
                            }
                        }
                        Err(e) => self.state = State::Failed(e),
                    }
                }
                Reply::Detail(detail, finest) => {
                    // The thread draws the shown view again with the new mesh.
                    self.shown = None;
                    self.detail = detail;
                    if finest || detail.is_none() {
                        self.asked = u32::MAX;
                    }
                }
                // A plate switched away from.
                Reply::Frame(plate, ..) if plate != self.plate => {}
                Reply::Frame(plate, view, size, image) => {
                    if self.shown_plate.is_some_and(|s| s != plate) {
                        // Another plate: it fades in over the last one's picture.
                        self.previous = self.texture.take();
                        self.switched = Some(Instant::now());
                    }
                    match &mut self.texture {
                        Some(t) => t.set(image, egui::TextureOptions::LINEAR),
                        None => {
                            self.texture = Some(ctx.load_texture(
                                format!("lib-preview-{}-{plate}", self.id),
                                image,
                                egui::TextureOptions::LINEAR,
                            ))
                        }
                    }
                    self.shown = Some((view, size));
                    self.shown_plate = Some(plate);
                    self.shown_facts = self.facts().cloned();
                    self.render_error = None;
                    self.first_frame.get_or_insert_with(Instant::now);
                }
                Reply::Failed(view, size, e) => {
                    self.shown = Some((view, size));
                    self.render_error = Some(e);
                }
            }
        }
        if self.switch_fade() >= 1. {
            self.previous = None;
            self.switched = None;
        }
    }

    /// A project's plates by name (empty for a model with one plate, or none).
    pub fn plates(&self) -> &[String] {
        &self.plates
    }

    /// The plate wanted (zero-based).
    pub fn plate(&self) -> usize {
        self.plate
    }

    /// Shows plate `plate` (zero-based) of a project with several plates: the thread makes
    /// its mesh; the last picture stays until its first frame. The view is kept.
    pub fn set_plate(&mut self, plate: usize) {
        if plate == self.plate || plate >= self.plates.len() {
            return;
        }
        self.plate = plate;
        self.plate_asked = Some(Instant::now());
        self.requested = None;
        self.resizing = None;
        self.refine = None;
        let _ = self.tx.send(Request::Plate(plate));
    }

    /// Another plate is on its way (the last one's picture shows meanwhile).
    pub fn switching(&self) -> bool {
        !self.plates.is_empty()
            && self.shown_plate.is_some_and(|s| s != self.plate)
            && self.plate_error().is_none()
    }

    /// The plate on its way (zero-based) once it takes long enough to say so (most take a
    /// few frames).
    pub fn loading_plate(&self) -> Option<usize> {
        (self.switching() && self.plate_asked.is_some_and(|t| t.elapsed() >= SAY_LOADING))
            .then_some(self.plate)
    }

    /// Why the wanted plate of a project with several plates cannot be shown.
    pub fn plate_error(&self) -> Option<&str> {
        match &self.state {
            State::Failed(e) if !self.plates.is_empty() && self.thread_plate == self.plate => {
                Some(e)
            }
            _ => None,
        }
    }

    /// The facts of the plate whose picture shows (before the first picture: of the mesh
    /// read), and that plate (zero-based).
    pub fn shown(&self) -> Option<(&Facts, usize)> {
        if self.plate_error().is_some() {
            return None;
        }
        match (&self.shown_facts, self.shown_plate) {
            (Some(f), Some(p)) => Some((f, p)),
            _ => self.facts().map(|f| (f, self.thread_plate)),
        }
    }

    /// The last plate's picture fading out under the new one, and how far the new one has
    /// faded in (0 to 1).
    pub fn previous(&self) -> Option<(&egui::TextureHandle, f32)> {
        self.previous.as_ref().map(|t| (t, self.switch_fade()))
    }

    fn switch_fade(&self) -> f32 {
        self.switched.map_or(1., |t| {
            (t.elapsed().as_secs_f32() / CROSSFADE.as_secs_f32()).min(1.)
        })
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

    /// The picture shown is the newest one asked for, of the plate wanted, faded in.
    pub fn settled(&self) -> bool {
        self.shown.is_some()
            && self.shown == self.requested
            && self.resizing.is_none()
            && self.shown_plate == Some(self.plate)
            && self.previous.is_none()
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
        let _ = self.tx.send(Request::Detail(self.thread_plate, cells));
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

/// "Plate 2 of 4 · Name" between a previous and a next button (disabled at the ends);
/// returns the plate (zero-based) clicked to.
fn plate_switcher(ui: &mut egui::Ui, plates: &[String], plate: usize) -> Option<usize> {
    let p = theme::of(ui);
    let mut to = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.;
        let back = plate > 0;
        if chevron_button(ui, false, back, "Previous plate", "This is the first plate").clicked() {
            to = Some(plate - 1);
        }
        let mut text = format!("Plate {} of {}", plate + 1, plates.len());
        if let Some(name) = plates.get(plate).filter(|n| !n.is_empty()) {
            text = format!("{text} · {name}");
        }
        let w = (ui.available_width() - CHEVRON - ui.spacing().item_spacing.x).max(0.);
        let (r, resp) = ui.allocate_exact_size(Vec2::new(w, CHEVRON), Sense::hover());
        let mut job =
            egui::text::LayoutJob::simple_singleline(text.clone(), theme::medium(12.), p.label);
        job.wrap = egui::text::TextWrapping::truncate_at_width(w);
        let g = ui.painter().layout_job(job);
        let elided = g.elided;
        ui.painter().galley(r.center() - g.size() / 2., g, p.label);
        if elided {
            resp.on_hover_text(text);
        }
        let next = plate + 1 < plates.len();
        if chevron_button(ui, true, next, "Next plate", "This is the last plate").clicked() {
            to = Some(plate + 1);
        }
    });
    to
}

/// The plate switcher's buttons, points square.
const CHEVRON: f32 = 26.;

/// A frameless ‹ or › button: keyboard-focusable (a ring shows the focus), a tooltip,
/// disabled at the ends.
fn chevron_button(
    ui: &mut egui::Ui,
    next: bool,
    enabled: bool,
    tip: &str,
    disabled_tip: &str,
) -> egui::Response {
    let p = theme::of(ui);
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (r, resp) = ui.allocate_exact_size(Vec2::splat(CHEVRON), sense);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, tip));
    let hot = enabled && resp.hovered();
    if hot {
        ui.painter().rect_filled(r, 6., p.hover);
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    if resp.has_focus() {
        ui.painter().rect_stroke(
            r.shrink(1.),
            6.,
            Stroke::new(1.5, p.accent),
            egui::StrokeKind::Inside,
        );
    }
    let color = if !enabled {
        p.muted.gamma_multiply(0.5)
    } else if hot {
        p.text
    } else {
        p.muted_alt
    };
    let (c, s) = (r.center(), 4.);
    let x = if next { -s / 2. } else { s / 2. };
    ui.painter().add(egui::Shape::line(
        vec![
            Pos2::new(c.x + x, c.y - s),
            Pos2::new(c.x - x, c.y),
            Pos2::new(c.x + x, c.y + s),
        ],
        Stroke::new(1.6, color),
    ));
    resp.on_hover_text(if enabled { tip } else { disabled_tip })
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
        let plate_error = self
            .library
            .preview
            .as_ref()
            .filter(|pv| pv.id == m.id)
            .and_then(|pv| Some((pv.plate_error()?.to_string(), pv.plate())));
        if let Some((e, plate)) = &plate_error {
            // Neither the thumbnail (the first plate's) nor the last plate's picture.
            ui.painter().text(
                rect.center(),
                Align2::CENTER_CENTER,
                format!("Can't show plate {}: {e}", plate + 1),
                theme::sans(11.5),
                p.muted,
            );
        } else if fade < 1. {
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
            let picture = |t: &egui::TextureHandle, alpha: f32| {
                let s = t.size_vec2() / ppp;
                let k = (rect.width() / s.x).min(rect.height() / s.y);
                ui.painter().image(
                    t.id(),
                    Rect::from_center_size(rect.center(), s * k),
                    Rect::from_min_max(Pos2::ZERO, Pos2::new(1., 1.)),
                    egui::Color32::WHITE.gamma_multiply(alpha),
                );
            };
            if fade > 0.
                && plate_error.is_none()
                && let Some(t) = pv.texture()
            {
                // Another plate fades in over the last one.
                let mut alpha = fade;
                if let Some((prev, k)) = pv.previous() {
                    picture(prev, fade * (1. - k));
                    alpha *= k;
                }
                picture(t, alpha);
            }
            if let Some(e) = pv.render_error() {
                ui.painter().text(
                    rect.right_bottom() - Vec2::new(10., 10.),
                    Align2::RIGHT_BOTTOM,
                    format!("3D view failed: {e}"),
                    theme::sans(10.5),
                    p.warn_text,
                );
            } else if pv.loading() || pv.loading_plate().is_some() {
                ui.painter().text(
                    rect.right_bottom() - Vec2::new(10., 10.),
                    Align2::RIGHT_BOTTOM,
                    match pv.loading_plate() {
                        Some(plate) if !pv.loading() => format!("Loading plate {}…", plate + 1),
                        _ => "Loading 3D view…".to_string(),
                    },
                    theme::sans(10.5),
                    p.muted,
                );
            }
        }
        // A project with several plates: which one shows, and the previous and next.
        if let Some(pv) = self
            .library
            .preview
            .as_mut()
            .filter(|pv| pv.id == m.id && pv.plates().len() > 1)
            && let Some(plate) = plate_switcher(ui, pv.plates(), pv.plate())
        {
            pv.set_plate(plate);
            ctx.request_repaint();
        }
        if self
            .library
            .preview
            .as_ref()
            .is_some_and(|pv| pv.facts().is_some())
        {
            resp.on_hover_text("Drag to turn, scroll to zoom, double-click to reset the view");
        }
        // Size, triangles and the printers it fits: of the plate shown (the index's facts,
        // of the first plate, until the preview has read the file).
        let shown = self
            .library
            .preview
            .as_ref()
            .filter(|pv| pv.id == m.id)
            .and_then(|pv| pv.shown().map(|(f, plate)| (f.clone(), plate)));
        let (size_mm, triangles, plate) = match &shown {
            Some((f, plate)) => (Some(f.size), Some(f.triangles as u64), *plate),
            None => (
                m.info.size_mm,
                (m.info.triangles > 0).then_some(m.info.triangles),
                0,
            ),
        };
        if let Some(s) = size_mm.filter(|_| plate_error.is_none()) {
            let mut line = crate::library::dims(s);
            if let Some(t) = triangles {
                line = format!("{line} · {} triangles", group_thousands(t));
            }
            if m.kind == mica_library::Kind::ThreeMf && m.info.plates > 1 {
                line.push_str(&format!(" · plate {}", plate + 1));
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
    fn plates_switch_with_their_own_mesh_colours_size_and_fits() {
        let ctx = egui::Context::default();
        let dir = mica_library::testkit::TempDir::new("preview-plates");
        let path = dir.path().join("plates.3mf");
        std::fs::write(&path, mica_library::testkit::four_plates_3mf()).unwrap();
        let project = PreviewProject::read_file(&path).unwrap();
        let size = |plate| {
            let m = project.plate(plate).unwrap();
            let (lo, hi) = m.mesh.bounds();
            std::array::from_fn::<f32, 3, _>(|i| hi[i] - lo[i])
        };
        let mut p = Preview::new("plates", Kind::ThreeMf, path, &ctx);
        assert!(wait(&mut p, &ctx, |p| p.facts().is_some()));
        assert_eq!(p.plates(), ["", "Lid", "Loose", ""]);
        p.request([200, 150]);
        assert!(wait(&mut p, &ctx, Preview::settled));
        let (f, plate) = p.shown().unwrap();
        assert_eq!((f.size, plate), (size(0), 0));
        // Turned and zoomed: the view stays when switching.
        p.orbit(egui::vec2(30., 10.));
        p.zoom(80.);
        let view = p.view;
        p.request([200, 150]);
        assert!(wait(&mut p, &ctx, Preview::settled));
        p.set_plate(1);
        assert!(p.switching() && !p.settled());
        assert_eq!(
            p.shown().unwrap().1,
            0,
            "the last plate shows until the new one"
        );
        p.request([200, 150]);
        assert!(wait(&mut p, &ctx, |p| p.shown().is_some_and(|s| s.1 == 1)));
        assert!(
            p.previous().is_some(),
            "the new plate fades in over the last"
        );
        assert!(wait(&mut p, &ctx, Preview::settled));
        assert!(p.previous().is_none() && !p.switching());
        assert_eq!(p.view, view);
        let (f, _) = p.shown().unwrap();
        assert_eq!(f.size, size(1));
        assert_eq!((f.triangles, f.coloured), (12, true));
        // The lid's mesh in its tool's colour, and only that plate's mesh is held.
        let lid = p.retained.upgrade().unwrap();
        let c = lid.face_colours.as_ref().unwrap();
        assert!(c.iter().all(|&x| x == [0xFF; 3]), "{c:?}");
        drop(lid);
        // Fits: the shown plate's size against a volume only one of the plates fits.
        let (a, b) = (size(0), size(1));
        assert_ne!(a, b);
        // The lid fits a volume just its size; the first plate does not.
        let volume = b;
        assert!(fits(b, volume) && !fits(a, volume), "{a:?} {b:?}");
        assert_eq!(fits(f.size, volume), fits(b, volume));
        // Next: the loose cube in red; then the empty plate says so.
        p.set_plate(2);
        p.request([200, 150]);
        assert!(wait(&mut p, &ctx, Preview::settled));
        let loose = p.retained.upgrade().unwrap();
        assert!(
            loose
                .face_colours
                .as_ref()
                .unwrap()
                .iter()
                .all(|&x| x == [0xFB, 0x02, 0x07])
        );
        drop(loose);
        p.set_plate(3);
        assert!(wait(&mut p, &ctx, |p| p.plate_error().is_some()));
        assert_eq!(p.plate_error(), Some("nothing on this plate"));
        assert!(p.shown().is_none() && !p.switching());
        // Out of range: nothing happens. Back to the first plate.
        p.set_plate(4);
        assert_eq!(p.plate(), 3);
        p.set_plate(0);
        // (The panel asks for a picture every frame; one is drawn once the plate is read.)
        assert!(wait(&mut p, &ctx, |p| p.facts().is_some()));
        p.request([200, 150]);
        assert!(wait(&mut p, &ctx, Preview::settled));
        assert_eq!(p.shown().unwrap().0.size, size(0));
        // Another selection drops the preview: the project as read and the mesh go.
        let (mesh, project) = (p.retained.clone(), p.retained_project.clone());
        assert!(project.upgrade().is_some());
        drop(p);
        let start = Instant::now();
        while (mesh.strong_count() > 0 || project.strong_count() > 0)
            && start.elapsed() < Duration::from_secs(10)
        {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!((mesh.strong_count(), project.strong_count()), (0, 0));
    }

    #[test]
    fn single_plate_projects_and_stls_have_no_plates_to_switch() {
        let ctx = egui::Context::default();
        let root = mica_library::testkit::repo_root();
        for (kind, file) in [
            (Kind::ThreeMf, "tests/models/painted-cube.3mf"),
            (Kind::Stl, "tests/models/cube20.stl"),
        ] {
            let mut p = Preview::new(file, kind, root.join(file), &ctx);
            assert!(wait(&mut p, &ctx, |p| p.facts().is_some()));
            assert!(p.plates().is_empty(), "{file}");
            p.set_plate(1);
            assert_eq!(p.plate(), 0);
            p.request([64, 64]);
            assert!(wait(&mut p, &ctx, Preview::settled));
            assert_eq!(p.shown().unwrap().1, 0);
            assert!(
                p.retained_project.upgrade().is_none(),
                "nothing kept to switch"
            );
        }
    }

    /// The switcher's buttons are reached with Tab and pressed with Enter; the first plate's
    /// previous button is disabled (so Tab lands on next), the last plate's next one too.
    #[test]
    fn the_plate_switcher_works_from_the_keyboard() {
        let ctx = egui::Context::default();
        let plates = ["".to_string(), "Lid".to_string(), String::new()];
        let frame = |plate: usize, events: Vec<egui::Event>| {
            let mut to = None;
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(320., 200.))),
                events,
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| to = plate_switcher(ui, &plates, plate));
            output.textures_delta.clear();
            to
        };
        let key = |key: egui::Key| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        };
        for (plate, want) in [(0, Some(1)), (2, Some(1))] {
            ctx.memory_mut(|m| m.surrender_focus(m.focused().unwrap_or(egui::Id::NULL)));
            assert_eq!(frame(plate, vec![]), None);
            assert_eq!(frame(plate, vec![key(egui::Key::Tab)]), None);
            assert!(
                ctx.memory(|m| m.focused().is_some()),
                "a button has the focus"
            );
            let pressed =
                frame(plate, vec![key(egui::Key::Enter)]).or_else(|| frame(plate, vec![]));
            assert_eq!(pressed, want, "plate {plate}");
        }
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
