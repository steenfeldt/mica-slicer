// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The 3D view: camera interaction, picking, gizmos, floating toolbars, plate labels and the
//! preview overlays (layer and move sliders, hover card). It owns no scene data; the editor hands
//! in a [`Scene`] each frame and receives [`Action`]s back.

pub mod axes;
pub mod drag;
pub mod flatten;
pub mod gizmo;
pub mod icons;
pub mod measure;
mod plate_bar;
pub mod plate_tags;
mod preview_plates;
pub mod section;
mod toolbar;
use crate::{
    commands::{self, Command, MenuInfo},
    cut::CutState,
    preview::{Preview, Segment, View, slider},
    render,
    scene::{
        Bed, Delta, Object, Sub,
        camera::Camera,
        picking,
        plates::{self, Plate},
    },
    theme,
    tower::{TowerCommand, TowerView},
};
use eframe::egui::{self, Align2, Color32, Key, PointerButton, Pos2, Rect, Vec2 as EVec2};
use glam::{Vec2, Vec3};
use icons::Icon;
use mica_core::painting::PaintKind;
pub use plate_bar::{PlateProbe, PlateRename};
use std::sync::Arc;

/// Totals of the current plate's slice for the bottom-right card.
#[derive(Clone, Debug, PartialEq)]
pub struct Totals {
    pub time_s: f64,
    pub grams: f64,
    pub filament_mm: f64,
    /// `+2 min · +0.8 g`: the change since the previous slice.
    pub delta: Option<String>,
    /// What changed before this slice, `infill → 20 %`, shown under the delta.
    pub cause: Option<String>,
}

/// What the editor shows this frame.
pub struct Scene<'a> {
    pub objects: &'a [Object],
    /// The plate each object belongs to (same length as `objects`), so placement warnings
    /// check an object against its own plate's bed. Empty: every object is on plate 0.
    pub plate_of: &'a [usize],
    /// The primary selected object (gizmo, tool card) and every selected object (halo,
    /// group moves).
    pub selected: Option<usize>,
    pub selection: &'a [usize],
    /// How many objects the current plate has (the per-object tools need one).
    pub plate_objects: usize,
    /// The scene index of the current plate's object when it is the only one: a tool that
    /// needs a selection picks it instead of opening with nothing to work on.
    pub only_object: Option<usize>,
    /// The selected volume or height range of the primary object.
    pub sub: Option<Sub>,
    /// The prime / wipe towers of the plates that print one (world placement).
    pub towers: &'a [TowerView],
    /// The current plate's tower is selected (its gizmo and card show instead of an object's).
    pub tower_selected: bool,
    /// The cut tool's plane while it is open (the gizmo moves it instead of the object).
    pub cut: Option<&'a CutState>,
    /// The open mesh tool, as its toolbar icon (highlighted).
    pub mesh_tool: Option<icons::Icon>,
    /// Whether Paste has something to paste (context menus).
    pub can_paste: bool,
    pub bed: &'a Bed,
    pub plates: &'a [Plate],
    pub tool_colors: &'a [[f32; 3]],
    /// The Textures feature is on (Settings › Features): Texture painting is offered.
    pub textures: bool,
    /// The tools' names for menus, `tool_colors` order (physical tools, then mixed colors).
    pub tool_labels: &'a [String],
    /// Painted facets to draw over the objects.
    pub overlays: &'a [crate::scene::paint::PaintOverlay],
    /// The active painting tool's brush, for the cursor on the surface.
    pub paint: Option<PaintView>,
    /// The active object tool (variable layer height, brim ears) and what its cursor needs.
    pub object_tool: Option<ObjectToolView>,
    /// `Some` switches to preview mode.
    pub preview: Option<(&'a Arc<Preview>, &'a mut View)>,
    /// The window shows Preview, also while the current plate has no slice to show (`preview`
    /// is then `None`): with several plates, the plate list stays at the view's left edge.
    pub previewing: bool,
    /// False while slicing: camera only.
    pub editable: bool,
    /// Slice all plates can start (the plate list's button).
    pub can_slice_all: bool,
    /// A fresh status message to show in the hint line instead of the mode hint.
    pub status: Option<String>,
    /// Slice totals of the current plate, when it is sliced.
    pub totals: Option<Totals>,
    /// Slicing progress of the current plate, while it runs.
    pub progress: Option<f32>,
    /// The current plate's per-layer custom G-code, for the preview's layer slider.
    pub layer_gcode: Option<crate::preview::layer_gcode::LayerGCode<'a>>,
    /// Sequential printing: each object's clearance outline (world mm) and whether it
    /// collides, drawn on the bed. Skirts and brims that do not fit come along as colliding
    /// outlines (red).
    pub clearance: &'a [(Vec<[f32; 2]>, bool)],
    /// Objects (scene indices) whose skirt or brim would not fit on the bed, with the label
    /// shown over them: tinted like an object outside the bed.
    pub warned: &'a [(usize, String)],
    /// The current plate's slice failed: the reason in a few words, for the totals card.
    pub failed: Option<String>,
    /// Objects (scene indices) past the bed's edge, or with a skirt or brim past it, that fit
    /// when moved: their menu offers "Move inside".
    pub movable: &'a [usize],
    /// The skirt and brim outlines (world mm, on the bed) that do not fit: drawn in red.
    pub adhesion_outlines: &'a [Vec<[f32; 2]>],
}
/// The painting tool as the viewport shows it: brush shape and radius (mm) for the cursor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PaintView {
    pub kind: PaintKind,
    pub brush: crate::paint::BrushKind,
    pub radius: f32,
    pub erase: bool,
}

/// A tool that edits the selected object from the view; gizmos are off while one is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObjectTool {
    /// Variable layer height: drag on the object to thicken or thin layers in a band.
    LayerHeights,
    /// Brim ears: click the model to place an ear, click an ear to remove it.
    BrimEars,
}

/// The object tool as the viewport shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct ObjectToolView {
    pub tool: ObjectTool,
    /// Edited band (mm) of the layer height tool.
    pub band_width: f32,
    /// Diameter of a new ear (mm).
    pub ear_diameter: f32,
    /// Ears that touch neither the model nor an attached ear (drawn as invalid).
    pub loose_ears: Vec<usize>,
}

/// Requests back to the editor; the viewport never mutates scene data itself.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Open,
    /// A built-in model from the toolbar's Add menu ("Add shape").
    AddShape(
        mica_core::builtin_models::BuiltinModel,
        crate::builtin::AddAs,
    ),
    /// A paint stroke frame on object `index` (the brush is where the pointer hits it). With
    /// a `trail` (the earlier points of this frame's segment, oldest first) the brush sweeps
    /// from the trail's first point through to `hit`, so fast drags leave no gaps.
    Paint {
        index: usize,
        hit: crate::paint::Hit,
        trail: Vec<crate::paint::Hit>,
        /// Shift held: erase instead of paint.
        erase: bool,
    },
    /// Alt-scroll in a painting tool: change the brush radius by this many mm.
    BrushSize(f32),
    /// A painting tool was picked (or dropped with `None`) from the toolbar.
    PaintTool(Option<PaintKind>),
    /// A line for the hint bar: why a tool's shortcut did nothing.
    Hint(String),
    /// An object tool was picked (or dropped with `None`).
    ObjectTool(Option<ObjectTool>),
    /// A layer height drag step on object `index` at `z` mm above its bottom: `amount` mm of
    /// layer height (positive thickens); Shift resets towards the base height, Alt smooths.
    LayerEdit {
        index: usize,
        z: f64,
        amount: f64,
        shift: bool,
        alt: bool,
    },
    /// Place a brim ear on object `index` at `at` (world XY).
    AddBrimEar {
        index: usize,
        at: Vec2,
    },
    /// Remove brim ear `ear` of object `index`.
    RemoveBrimEar {
        index: usize,
        ear: usize,
    },
    Arrange,
    /// Click selection; `toggle` (Shift / Cmd held) adds to or removes from the selection.
    Select {
        index: Option<usize>,
        toggle: bool,
    },
    /// Shift-drag box: every object whose box projects into the rectangle.
    BoxSelect {
        indices: Vec<usize>,
        toggle: bool,
    },
    /// A volume or height range of object `index` was picked (or dropped with `None`).
    SelectSub {
        index: usize,
        sub: Option<Sub>,
    },
    /// A context-menu command on object `index` (None: the bed menu).
    Command {
        index: Option<usize>,
        command: Command,
    },
    /// The cut plane was dragged (translate) or tilted (rotate).
    CutPlane(Delta),
    /// A cut connector was placed, picked or removed (the cut tool's "Add connectors" mode).
    CutConnector(crate::cut::ConnectorEdit),
    /// The current plate's prime tower was clicked.
    SelectTower,
    /// The prime tower was dragged, nudged or turned (incremental, like `Transform`).
    TowerTransform(Delta),
    /// A command from the tower's context menu.
    TowerCommand(TowerCommand),
    Transform {
        index: usize,
        delta: Delta,
    },
    LayOnFace {
        index: usize,
        face: usize,
    },
    /// Lay the object on one of its flat planes (`flatten`): `normal` is the plane's world
    /// normal when it was clicked.
    LayOnPlane {
        index: usize,
        normal: Vec3,
    },
    /// Sets the selected object's world rotation (X, Y, Z Euler angles, radians) from a field.
    SetRotation {
        index: usize,
        euler: Vec3,
    },
    /// A drag began: the editor snapshots the scene for one undo step.
    GestureBegin(&'static str),
    /// A drag ended; the editor commits the undo step (if anything changed).
    Edited,
    /// A drag was cancelled with Esc; the editor restores the state from `GestureBegin`.
    GestureCancel,
    Cut,
    /// Add, replace or remove a colour change, pause or custom G-code at a layer.
    LayerGCode(crate::preview::layer_gcode::Edit),
    /// The Text / SVG tool was toggled (toolbar button).
    Emboss,
    /// Esc with the Text / SVG tool open: its edits are put back and it closes.
    EmbossCancel,
    /// Open the Text / SVG tool on a text: volume `volume` of object `index`, or the object
    /// itself when it is a text object (`None`); a double-click or a click with the tool open.
    EditText {
        index: usize,
        volume: Option<u64>,
    },
    /// With the Text / SVG tool open, the model was clicked at `point` (world) on a face with
    /// outward `normal`: place the text there.
    EmbossPlace {
        index: usize,
        point: Vec3,
        normal: Vec3,
    },
    AddPlate,
    /// The plate list's Slice all plates.
    SliceAll,
    RemovePlate(usize),
    SelectPlate(usize),
    PlateArrange(usize),
    PlateOrient(usize),
    PlateLock(usize),
    /// A pick from the plate menu (plate bar, 3D view, object list, plate list).
    Plate(usize, crate::plate_menu::PlateCommand),
    /// The plate bar's name field was confirmed (empty: `Plate N` again).
    RenamePlate(usize, String),
    /// A plate was dragged to another place in the plate list.
    MovePlate {
        from: usize,
        to: usize,
    },
    /// Preview legend/slider changed.
    ViewChanged,
}
#[derive(Clone, Debug, PartialEq)]
enum Drag {
    None,
    Orbit,
    Pan,
    /// A press on a model that moves it (and the rest of the selection) on the bed. `last` is
    /// the bed point the object has followed to; nothing moves until the pointer left the
    /// `press` point by more than [`drag::MOVE_THRESHOLD_PX`] (`armed`).
    Object {
        index: usize,
        last: Vec3,
        press: Pos2,
        armed: bool,
    },
    /// The prime tower dragged on the bed.
    Tower {
        last: Vec3,
    },
    Gizmo {
        index: usize,
        drag: gizmo::Drag,
    },
    /// A paint stroke on the selected object; `last` is the previous pointer sample and its
    /// hit, where the next frame's stroke segment starts (`None` after the pointer left the
    /// model).
    Paint {
        index: usize,
        last: Option<(Pos2, crate::paint::Hit)>,
    },
    /// A layer height drag at `z` (mm above the object's bottom), last pointer Y.
    LayerEdit {
        index: usize,
        z: f64,
        last_y: f32,
    },
    /// Shift-drag rectangle selection from `start` (screen).
    Box {
        start: Pos2,
        toggle: bool,
    },
    /// A handle of the section plane (`section::Section` keeps the grab).
    Section,
}
/// `Drag::Gizmo { index }` for the cut plane rather than an object.
const CUT_INDEX: usize = usize::MAX;
/// `Drag::Gizmo { index }` for the prime tower.
const TOWER_INDEX: usize = usize::MAX - 1;
/// The tower box and its footprint slab (RGBA).
pub const TOWER_COLOR: [f32; 4] = [0.56, 0.62, 0.8, 0.34];
pub const TOWER_FOOTPRINT_COLOR: [f32; 4] = [0.56, 0.62, 0.8, 0.22];
/// A hovered preview move: record index, record and attributes.
type HoveredMove = (u64, Segment, crate::preview::Attr);
pub struct Viewport {
    /// Open the toolbar's Add menu on the next frame (screenshots).
    pub open_add_menu: bool,
    pub camera: Camera,
    pub mode: gizmo::Mode,
    pub lay_on_face: bool,
    /// The Text / SVG tool is open on a text volume: clicks on a model place the text (set by
    /// the editor).
    pub emboss: bool,
    /// The Text / SVG tool is open (on a volume or a text object): Esc cancels it.
    pub emboss_open: bool,
    /// The flat planes "Lay on face" offers, per mesh and scale.
    pub planes: flatten::PlaneCache,
    /// The active painting tool; gizmos are off while one is.
    pub paint: Option<PaintKind>,
    /// The active object tool (layer heights, brim ears); gizmos are off while one is.
    pub object_tool: Option<ObjectTool>,
    /// Where the object tool points on the selected object (world).
    tool_hover: Option<Vec3>,
    /// Where the brush would land: (hit, face normal) in bed coordinates.
    paint_hover: Option<(Vec3, Vec3)>,
    /// The stroke event a click there would start (the Fill brush's preview).
    paint_hover_hit: Option<crate::paint::Hit>,
    pub theme_mode: theme::Mode,
    pub hovered: Option<usize>,
    pub hovered_plate: Option<usize>,
    /// The hovered plate's label plus icon strip (screen), last frame: the pointer keeps the
    /// plate hovered there, so the icons stay reachable after leaving the plate itself.
    plate_hover_zone: Option<(usize, Rect)>,
    /// This frame's plate tags on the ground (drawn by the renderer, clicked here).
    plate_tags: Vec<plate_tags::PlateTag>,
    tag_cache: plate_tags::TagCache,
    /// The plate name being edited in the plate bar.
    pub plate_rename: Option<PlateRename>,
    /// Open the plate list (or the plate menu) on the next frame.
    pub open_plate_list: bool,
    pub open_plate_menu: bool,
    /// Where the plate bar's parts were drawn (UI tests).
    pub plate_probe: PlateProbe,
    /// Where Preview's plate list was drawn (UI tests), and the plate it last showed as
    /// current (a newly shown plate scrolls into view).
    pub preview_plates_probe: preview_plates::PreviewPlatesProbe,
    preview_plates_shown: Option<usize>,
    /// The plate the right-clicked bed belongs to (the bed menu's plate section).
    menu_plate: Option<usize>,
    /// A volume of the selected object under the pointer.
    pub hovered_volume: Option<u64>,
    /// The current plate's prime tower is under the pointer.
    pub hovered_tower: bool,
    /// Where the pointer meets the bed plane (world), for pasting at the cursor.
    pub bed_hover: Option<Vec3>,
    /// What the context menu was opened on: `Some(None)` = the bed, `Some(Some(i))` = an object.
    menu_target: Option<Option<usize>>,
    /// The context menu was opened on the prime tower.
    menu_tower: bool,
    hover_handle: Option<gizmo::Handle>,
    drag: Drag,
    feedback: Option<String>,
    /// Preview move under the pointer: the pick answer it came from (`PickResult::serial`)
    /// and the record with its attributes (`None` = no move there).
    hover_segment: Option<(u64, Option<HoveredMove>)>,
    /// The preview's hover pick, answered by the renderer from its index buffer.
    pick: render::toolpaths::SharedPick,
    /// When the camera or the preview's sliders last changed (hash of camera and range, and
    /// the time; `None` = not since the view opened): distant layers draw as ribbons until
    /// the view has been still for `toolpaths::REFINE_DELAY`.
    interaction: Option<(u64, Option<std::time::Instant>)>,
    /// Tools and benchmarks: force the preview's moving (`Some(true)`) or still look.
    pub force_moving: Option<bool>,
    /// This frame's preview is drawn as moving (no hover card then).
    moving: bool,
    /// Typing a Z angle after double-clicking the ring handle.
    angle_edit: Option<String>,
    /// Scrolling zooms towards the pointer (else the centre of the view).
    pub zoom_to_pointer: bool,
    /// Scrolling up zooms out.
    pub reverse_zoom: bool,
    /// How big the floating toolbars are (Settings › General).
    pub toolbar_size: icons::ToolbarSize,
    /// The measure tool is open (Prepare); its selection and cached features.
    pub measure_on: bool,
    pub measure: measure::MeasureTool,
    /// The section view (a clipping plane per mode).
    pub section: section::Section,
    /// This frame's 3D-view shortcuts, queued by `App::shortcuts` (`keymap.rs`).
    pub keys: Vec<crate::keymap::Act>,
    /// Preview's one-layer mode (L): the low layer handle follows the high one.
    pub one_layer: bool,
}
impl Default for Viewport {
    fn default() -> Self {
        Self::new(Camera::default())
    }
}
/// Pointer position in viewport coordinates 0..1.
fn uv(rect: Rect, p: Pos2) -> Vec2 {
    Vec2::new(
        (p.x - rect.left()) / rect.width(),
        (p.y - rect.top()) / rect.height(),
    )
}
impl Viewport {
    pub fn new(camera: Camera) -> Self {
        Self {
            open_add_menu: false,
            camera,
            mode: gizmo::Mode::Move,
            lay_on_face: false,
            emboss: false,
            emboss_open: false,
            planes: flatten::PlaneCache::default(),
            paint: None,
            paint_hover: None,
            paint_hover_hit: None,
            object_tool: None,
            tool_hover: None,
            theme_mode: theme::Mode::default(),
            hovered: None,
            hovered_plate: None,
            plate_hover_zone: None,
            plate_tags: vec![],
            tag_cache: Default::default(),
            plate_rename: None,
            open_plate_list: false,
            open_plate_menu: false,
            plate_probe: PlateProbe::default(),
            preview_plates_probe: Default::default(),
            preview_plates_shown: None,
            menu_plate: None,
            hovered_volume: None,
            hovered_tower: false,
            bed_hover: None,
            menu_target: None,
            menu_tower: false,
            hover_handle: None,
            drag: Drag::None,
            feedback: None,
            hover_segment: None,
            pick: Default::default(),
            interaction: None,
            force_moving: None,
            moving: false,
            angle_edit: None,
            zoom_to_pointer: true,
            reverse_zoom: false,
            toolbar_size: icons::ToolbarSize::default(),
            measure_on: false,
            measure: measure::MeasureTool::default(),
            section: section::Section::default(),
            keys: vec![],
            one_layer: false,
        }
    }
    /// The flat plane "Lay on face" would use under a pointer ray (`flatten`).
    fn plane_under(
        planes: &mut flatten::PlaneCache,
        objects: &[Object],
        origin: Vec3,
        dir: Vec3,
        ctx: &egui::Context,
    ) -> Option<flatten::PlaneHit> {
        let face = picking::pick_face(objects, origin, dir).map(|h| (h.object, h.distance));
        let all: Vec<usize> = (0..objects.len()).collect();
        planes.pick(objects, &all, origin, dir, face, ctx)
    }
    /// Opens or closes the measure tool (it replaces the gizmos, lay-on-face, painting and the
    /// layer-height / brim-ear tools); returns the actions that close those.
    pub fn toggle_measure(&mut self) -> Vec<Action> {
        self.measure_on = !self.measure_on;
        self.measure.unselect();
        let mut out = vec![];
        if self.measure_on {
            self.lay_on_face = false;
            if self.paint.take().is_some() {
                out.push(Action::PaintTool(None));
            }
            if self.object_tool.take().is_some() {
                out.push(Action::ObjectTool(None));
            }
        }
        out
    }
    /// The scene bounds the section plane spans: the current plate's objects (else the bed).
    pub fn section_bounds(&self, scene: &Scene<'_>) -> (Vec3, Vec3) {
        let current = scene.plates.iter().position(|p| p.current).unwrap_or(0);
        let mine = scene
            .objects
            .iter()
            .enumerate()
            .filter(|(i, _)| scene.plate_of.get(*i).copied().unwrap_or(0) == current)
            .map(|(_, o)| o);
        let origin = scene
            .plates
            .get(current)
            .map_or(Vec3::ZERO, |p| p.origin.extend(0.));
        section::scene_bounds(
            mine,
            (
                scene.bed.min + origin,
                scene.bed.max.with_z(scene.bed.height.min(250.)) + origin,
            ),
        )
    }
    /// Scripted measure click at a screen point (headless screenshots, the monkey).
    pub fn measure_pick(&mut self, objects: &[Object], rect: Rect, at: Pos2, shift: bool) -> bool {
        let aspect = rect.width() / rect.height().max(1.);
        let ray = self.camera.ray(uv(rect, at), aspect);
        let painter =
            egui::Painter::new(egui::Context::default(), egui::LayerId::background(), rect);
        let gp = gizmo::Painter {
            painter: &painter,
            rect,
            camera: &self.camera,
        };
        self.measure_on = true;
        self.measure.pick_at(objects, &gp, ray, at, shift)
    }
    /// Frames the current plate (or all plates with `all`).
    pub fn fit_plate(&mut self, bed: &Bed, plates: &[Plate], all: bool, aspect: f32) {
        let (min, max) = if all {
            plates::all_bounds(bed, plates)
        } else {
            let origin = plates
                .iter()
                .find(|p| p.current)
                .map_or(Vec2::ZERO, |p| p.origin);
            plates::plate_bounds(bed, origin)
        };
        // Fit the plate footprint plus a third of the height so the bed fills the view.
        self.camera.fit(min, max.with_z(max.z * 0.35), aspect);
    }
    /// Frames one object with some air around it.
    pub fn focus(&mut self, o: &Object, aspect: f32) {
        let (min, max) = o.bounds();
        let pad = (max - min).max_element() * 0.5;
        self.camera.fit(min - pad, max + pad, aspect);
    }
    /// The gizmo's geometry: on the cut plane while the cut tool is open, on the selected
    /// volume when one is picked, else on the selected object.
    fn gizmo_layout(&self, scene: &Scene<'_>, height_px: f32) -> Option<gizmo::Layout> {
        if scene.preview.is_some()
            || !scene.editable
            || self.lay_on_face
            || self.paint.is_some()
            || self.measure_on
            || self.object_tool.is_some()
        {
            return None;
        }
        let mm = self.camera.mm_per_pixel(height_px);
        let size = mm * 80.;
        if scene.tower_selected && scene.selected.is_none() {
            // The tower: move on the bed or turn about its centre; no scaling.
            let t = current_tower(scene)?;
            if let Drag::Gizmo { drag, .. } = &self.drag {
                return Some(*drag.layout());
            }
            let centre = t.centre();
            return Some(match self.mode {
                // Only the bed ring: PrusaSlicer disables the X / Y grabbers for the tower.
                gizmo::Mode::Rotate => gizmo::Layout::rotate(
                    centre,
                    t.radius(),
                    t.radius(),
                    size,
                    t.shape.rotation.to_radians() as f32,
                    mm * 22.,
                )
                .z_only(),
                gizmo::Mode::None => return None,
                _ => gizmo::Layout::new(centre, size, gizmo::Mode::Move),
            });
        }
        let o = scene.objects.get(scene.selected?)?;
        if let Some(cut) = scene.cut {
            // Placing connectors: clicks go to the plane, the plane stays put.
            if cut.editing {
                return None;
            }
            if let Drag::Gizmo { drag, .. } = &self.drag {
                return Some(*drag.layout());
            }
            return Some(match self.mode {
                gizmo::Mode::Rotate => {
                    gizmo::Layout::rotate(cut.center, cut.extent, cut.extent, size, 0., mm * 22.)
                }
                gizmo::Mode::Scale | gizmo::Mode::None => {
                    gizmo::Layout::new(cut.center, size, gizmo::Mode::Move)
                }
                m => gizmo::Layout::new(cut.center, size, m),
            });
        }
        // A drag keeps the geometry it began with (ring radii and centres); the dragged ring's
        // handle is drawn moved by the sweep.
        if let Drag::Gizmo { drag, .. } = &self.drag {
            return Some(*drag.layout());
        }
        if let Some(Sub::Volume(id)) = scene.sub
            && let Some(v) = o.volume(id)
        {
            let (min, max) = o.volume_bounds(v);
            let centre = (min + max) * 0.5;
            let radius = (max - min).truncate().length() * 0.5;
            let sphere = (max - min).length() * 0.5;
            return Some(if self.mode == gizmo::Mode::Rotate {
                gizmo::Layout::rotate(centre, radius, sphere, size, 0., mm * 22.)
            } else {
                gizmo::Layout::new(centre, size, self.mode)
            });
        }
        if matches!(scene.sub, Some(Sub::Range(_))) {
            return None;
        }
        Some(if self.mode == gizmo::Mode::Rotate {
            Self::object_rotate_layout(o, mm)
        } else {
            gizmo::Layout::new(o.pivot_world(), size, self.mode)
        })
    }
    /// The rotate gizmo of an object at `mm` millimetres per pixel: the bed ring outside the
    /// footprint, the tilt rings outside the sphere around the pivot, 22 px clear of the model
    /// and at least 80 px.
    fn object_rotate_layout(o: &Object, mm: f32) -> gizmo::Layout {
        gizmo::Layout::rotate(
            o.pivot_world(),
            o.footprint_radius(),
            o.pivot_radius(),
            mm * 80.,
            o.angle,
            mm * 22.,
        )
    }
    /// Screen positions of ring `axis`'s handle and of the point at `angle` (radians in the
    /// ring's frame: absolute on the bed for Z, from the handle's rest for X / Y) and `ratio` ×
    /// ring radius, for scripted drags in headless screenshots.
    pub fn ring_screen_points(
        &self,
        o: &Object,
        rect: Rect,
        axis: usize,
        angle: f32,
        ratio: f32,
    ) -> Option<(Pos2, Pos2)> {
        let mm = self.camera.mm_per_pixel(rect.height());
        let ring = Self::object_rotate_layout(o, mm).ring(axis.min(2));
        let gp = gizmo::Painter {
            painter: &egui::Painter::new(
                egui::Context::default(),
                egui::LayerId::background(),
                rect,
            ),
            rect,
            camera: &self.camera,
        };
        Some((
            gp.project(ring.point(ring.rest, 1.))?,
            gp.project(ring.point(angle, ratio))?,
        ))
    }
    /// The 3D view's shortcuts (camera, tools, nudges, Esc), matched in `keymap.rs`.
    fn shortcuts(
        &mut self,
        keys: &[crate::keymap::Act],
        scene: &Scene<'_>,
        aspect: f32,
        actions: &mut Vec<Action>,
    ) {
        use crate::keymap::{Act, Tool, ZOOM_STEP, toggled};
        for &act in keys {
            match act {
                Act::Camera(n) => self.camera.preset(n as usize),
                Act::ZoomFit { all } => {
                    if let (Some(i), false) = (scene.selected, all)
                        && let Some(o) = scene.objects.get(i)
                    {
                        self.focus(o, aspect);
                    } else {
                        self.fit_plate(scene.bed, scene.plates, all, aspect);
                    }
                }
                Act::ZoomIn => self.camera.zoom(ZOOM_STEP),
                Act::ZoomOut => self.camera.zoom(-ZOOM_STEP),
                Act::TogglePerspective => self.camera.perspective = !self.camera.perspective,
                // In Preview, Esc closes the section view.
                Act::Escape if scene.preview.is_some() && self.section.active(true) => {
                    self.section.toggle(true, self.camera.direction());
                    actions.push(Action::ViewChanged);
                }
                _ => {}
            }
        }
        if scene.preview.is_some() || !scene.editable {
            return;
        }
        let mut has_selection = scene.selected.is_some();
        for &act in keys {
            match act {
                Act::Tool(Tool::Measure) if scene.objects.is_empty() => {
                    actions.push(Action::Hint(format!("Measure: {}", toolbar::ADD_FIRST)));
                }
                Act::Tool(Tool::Measure) => actions.extend(self.toggle_measure()),
                Act::Tool(tool) => {
                    // The shortcut follows its toolbar button: off without a model (or, with
                    // several, without a selection), the only object picked otherwise.
                    if let Some(icon) = toolbar::icon_for(tool)
                        && toolbar::needs_object(icon)
                    {
                        if let Err(why) = Self::tool_state(icon, scene) {
                            if !why.is_empty() {
                                actions.push(Action::Hint(format!("{}: {why}", icon.text().0)));
                            }
                            continue;
                        }
                        if !has_selection && let Some(only) = scene.only_object {
                            actions.push(Action::Select {
                                index: Some(only),
                                toggle: false,
                            });
                            has_selection = true;
                        }
                    }
                    let leave_paint = !matches!(tool, Tool::Paint(_));
                    // Any other tool closes the measure tool.
                    self.measure_on = false;
                    match tool {
                        Tool::Move | Tool::Rotate | Tool::Scale if has_selection => {
                            let mode = match tool {
                                Tool::Move => gizmo::Mode::Move,
                                Tool::Rotate => gizmo::Mode::Rotate,
                                _ => gizmo::Mode::Scale,
                            };
                            let other = self.lay_on_face || self.paint.is_some();
                            self.mode = toggled(self.mode, mode, other);
                            self.lay_on_face = false;
                        }
                        Tool::LayOnFace if has_selection => {
                            self.lay_on_face = !self.lay_on_face;
                        }
                        Tool::Paint(kind) => {
                            if kind == PaintKind::Material && scene.tool_colors.len() < 2 {
                                continue;
                            }
                            self.paint = if self.paint == Some(kind) {
                                None
                            } else {
                                Some(kind)
                            };
                            self.lay_on_face = false;
                            actions.push(Action::PaintTool(self.paint));
                        }
                        Tool::LayerHeights | Tool::BrimEars if has_selection => {
                            let t = if tool == Tool::LayerHeights {
                                ObjectTool::LayerHeights
                            } else {
                                ObjectTool::BrimEars
                            };
                            self.object_tool = (self.object_tool != Some(t)).then_some(t);
                            self.lay_on_face = false;
                            self.measure_on = false;
                            actions.push(Action::ObjectTool(self.object_tool));
                        }
                        _ => continue,
                    }
                    if leave_paint && self.paint.take().is_some() {
                        actions.push(Action::PaintTool(None));
                    }
                    if !matches!(tool, Tool::LayerHeights | Tool::BrimEars)
                        && self.object_tool.take().is_some()
                    {
                        actions.push(Action::ObjectTool(None));
                    }
                }
                // Arrow keys: 10 mm, 1 mm with Shift, along the view with ⌘ (Orca); in the
                // cut tool ↑ ↓ move the cut plane by 1 mm. Each press is one undo step.
                Act::Nudge {
                    dx,
                    dy,
                    fine,
                    camera,
                } if !self.dragging() => {
                    if scene.cut.is_some() {
                        if dx == 0 {
                            actions.push(Action::CutPlane(Delta::Translate(Vec3::Z * dy as f32)));
                        }
                        continue;
                    }
                    let step = if fine { 1. } else { 10. };
                    let dir = if camera {
                        let (right, up) = self.camera.basis();
                        let flat = |v: Vec3| v.with_z(0.).try_normalize().unwrap_or(Vec3::ZERO);
                        flat(right) * dx as f32 + flat(up) * dy as f32
                    } else {
                        Vec3::new(dx as f32, dy as f32, 0.)
                    };
                    if dir != Vec3::ZERO {
                        self.push_transform(scene, Delta::Translate(dir * step), "Move", actions);
                    }
                }
                Act::Turn(sign) if !self.dragging() && scene.cut.is_none() => {
                    let delta = Delta::Rotate {
                        axis: Vec3::Z,
                        angle: std::f32::consts::FRAC_PI_4 * sign as f32,
                    };
                    self.push_transform(scene, delta, "Rotate", actions);
                }
                Act::Escape => self.escape(scene, actions),
                _ => {}
            }
        }
    }
    /// One keyboard step on the selection (or the selected prime tower).
    fn push_transform(
        &self,
        scene: &Scene<'_>,
        delta: Delta,
        label: &'static str,
        actions: &mut Vec<Action>,
    ) {
        if scene.tower_selected && scene.selected.is_none() {
            actions.push(Action::GestureBegin(label));
            actions.push(Action::TowerTransform(delta));
            actions.push(Action::Edited);
        } else if let Some(i) = scene.selected
            && scene.sub.is_none()
        {
            actions.push(Action::GestureBegin(label));
            actions.push(Action::Transform { index: i, delta });
            actions.push(Action::Edited);
        }
    }
    /// Esc: abandon a drag, else close the open tool (cut, painting, lay on face, rotate or
    /// scale back to move), else drop the selected part, else clear the selection.
    fn escape(&mut self, scene: &Scene<'_>, actions: &mut Vec<Action>) {
        if self.dragging() {
            // Abandon the drag: the editor restores the start state, no undo entry.
            self.drag = Drag::None;
            self.feedback = None;
            actions.push(Action::GestureCancel);
        } else if scene.cut.is_some_and(|c| c.editing) {
            // "Add connectors" mode: Esc finishes the connectors before the cut tool closes.
            actions.push(Action::CutConnector(crate::cut::ConnectorEdit::Finish));
        } else if scene.cut.is_some() {
            actions.push(Action::Cut);
        } else if let Some(icon) = scene.mesh_tool {
            // The boolean / simplify card closes (its command toggles it).
            actions.push(Action::Command {
                index: None,
                command: if icon == icons::Icon::Simplify {
                    Command::Simplify
                } else {
                    Command::MeshBoolean
                },
            });
        } else if self.emboss_open {
            actions.push(Action::EmbossCancel);
        } else if self.paint.is_some() {
            self.paint = None;
            actions.push(Action::PaintTool(None));
        } else if self.measure_on {
            // Esc drops the measure selection first, then leaves the tool.
            if !self.measure.unselect() {
                self.measure_on = false;
            }
        } else if self.object_tool.is_some() {
            self.object_tool = None;
            actions.push(Action::ObjectTool(None));
        } else if self.lay_on_face {
            self.lay_on_face = false;
        } else if matches!(self.mode, gizmo::Mode::Rotate | gizmo::Mode::Scale)
            && scene.selected.is_some()
        {
            self.mode = gizmo::Mode::Move;
        } else if self.section.active(false) {
            // The section view closes before the selection is dropped.
            self.section.toggle(false, self.camera.direction());
        } else if let (Some(_), Some(i)) = (scene.sub, scene.selected) {
            actions.push(Action::SelectSub {
                index: i,
                sub: None,
            });
        } else {
            actions.push(Action::Select {
                index: None,
                toggle: false,
            });
        }
    }
    /// World Z the layer height tool points at: the pressed height while dragging, else the
    /// model under the pointer (`bottom` is the selected object's lowest Z).
    pub fn layer_tool_z(&self, bottom: f32) -> Option<f32> {
        if self.object_tool != Some(ObjectTool::LayerHeights) {
            return None;
        }
        match self.drag {
            Drag::LayerEdit { z, .. } => Some(bottom + z as f32),
            _ => self.tool_hover.map(|h| h.z),
        }
    }
    /// Whether an object or gizmo drag is in progress.
    pub fn dragging(&self) -> bool {
        matches!(
            self.drag,
            Drag::Object { .. }
                | Drag::Tower { .. }
                | Drag::Gizmo { .. }
                | Drag::Paint { .. }
                | Drag::LayerEdit { .. }
        )
    }
    /// Objects whose projected bounding box meets the screen rectangle (any corner inside the
    /// rectangle, or the rectangle inside the projected box).
    fn objects_in_rect(&self, gp: &gizmo::Painter<'_>, objects: &[Object], r: Rect) -> Vec<usize> {
        let mut out = vec![];
        for (i, o) in objects.iter().enumerate() {
            let (min, max) = o.bounds();
            let mut screen = Rect::NOTHING;
            let mut any_inside = false;
            for x in [min.x, max.x] {
                for y in [min.y, max.y] {
                    for z in [min.z, max.z] {
                        if let Some(p) = gp.project(Vec3::new(x, y, z)) {
                            screen = screen.union(Rect::from_min_max(p, p));
                            any_inside |= r.contains(p);
                        }
                    }
                }
            }
            if any_inside || (screen.is_positive() && screen.contains_rect(r)) {
                out.push(i);
            }
        }
        out
    }
    /// Picks the paint target: the selected object's face under the pointer.
    /// The stroke event a click at the pointer would start, while a painting tool hovers the
    /// selected object (from the last frame drawn).
    pub fn paint_hover_hit(&self) -> Option<crate::paint::Hit> {
        self.paint_hover_hit
    }

    fn paint_hit(
        &self,
        scene: &Scene<'_>,
        rect: Rect,
        p: Pos2,
        aspect: f32,
    ) -> Option<(usize, crate::paint::Hit, Vec3)> {
        let i = scene.selected?;
        let o = scene.objects.get(i)?;
        let (origin, dir) = self.camera.ray(uv(rect, p), aspect);
        let hit = picking::pick_face(std::slice::from_ref(o), origin, dir)?;
        Some((
            i,
            crate::paint::Hit {
                face: hit.face,
                point: origin + dir * hit.distance,
                camera: origin,
            },
            o.face_normal(hit.face),
        ))
    }
    /// Where the pointer ray at `p` meets the horizontal plane at height `z` (world), in
    /// front of the camera; `None` when the ray runs (nearly) parallel to it.
    fn plane_point_at(camera: &Camera, rect: Rect, p: Pos2, aspect: f32, z: f32) -> Option<Vec3> {
        let (a, d) = camera.ray(uv(rect, p), aspect);
        if d.z.abs() < 1e-5 {
            return None;
        }
        let t = (z - a.z) / d.z;
        (t > 0.).then(|| a + d * t)
    }
    /// Whether a press-and-drag on object `i` (scene index) moves it: only with no tool that
    /// gives the drag its own meaning (painting, layer heights, brim ears, measure, cut, lay
    /// on face), never on a locked plate, and with the Text tool open only for a model that
    /// is selected already (a drag on another one turns the camera, as before).
    fn press_moves(&self, scene: &Scene<'_>, i: usize) -> bool {
        let plate = scene.plate_of.get(i).copied().unwrap_or(0);
        let locked = scene
            .plates
            .iter()
            .find(|p| p.index == plate)
            .is_some_and(|p| p.locked);
        !locked
            && !self.lay_on_face
            && !self.measure_on
            && self.paint.is_none()
            && self.object_tool.is_none()
            && scene.cut.is_none()
            && (!self.emboss || scene.selection.contains(&i))
    }
    /// Draws the whole view into `ui`'s available rect and returns editor actions.
    pub fn show(&mut self, ui: &mut egui::Ui, mut scene: Scene<'_>) -> Vec<Action> {
        let mut actions = vec![];
        icons::set_size(ui.ctx(), self.toolbar_size);
        let (rect, response) =
            ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());
        let ctx = ui.ctx().clone();
        let aspect = rect.width() / rect.height().max(1.);
        let keys = std::mem::take(&mut self.keys);
        if let Some((preview, view)) = scene.preview.as_mut()
            && crate::keymap::preview_keys(&keys, &preview.layers, view, &mut self.one_layer)
        {
            actions.push(Action::ViewChanged);
            ctx.request_repaint();
        }
        self.shortcuts(&keys, &scene, aspect, &mut actions);
        let dt = ctx.input(|i| i.stable_dt).min(0.1);
        if self.camera.animate(dt, aspect) {
            ctx.request_repaint();
        }
        let pointer = ctx.input(|i| i.pointer.latest_pos());
        if response.hovered() {
            let scroll = ctx.input(|i| i.smooth_scroll_delta.y);
            let alt = ctx.input(|i| i.modifiers.alt);
            // ⌘-scroll reaches egui as a zoom gesture; while painting it sizes the brush (Orca).
            let pinch = ctx.input(|i| i.zoom_delta());
            if self.paint.is_some() && (pinch - 1.).abs() > 1e-4 {
                actions.push(Action::BrushSize((pinch - 1.) * 2.));
                ctx.request_repaint();
            } else if scroll.abs() > 0.
                && alt
                && (self.paint.is_some() || self.object_tool.is_some())
            {
                actions.push(Action::BrushSize(scroll * 0.02));
                ctx.request_repaint();
            } else if scroll.abs() > 0. && alt && self.section.active(scene.preview.is_some()) {
                // Alt-scroll moves the section plane (PrusaSlicer's clipping plane).
                let preview = scene.preview.is_some();
                if self.section.nudge(preview, scroll * 0.0005) && preview {
                    actions.push(Action::ViewChanged);
                }
                ctx.request_repaint();
            } else if scroll.abs() > 0. {
                // Settings › General: zoom direction and whether it follows the pointer.
                let scroll = if self.reverse_zoom { -scroll } else { scroll };
                match pointer.filter(|_| self.zoom_to_pointer) {
                    Some(p) => self.camera.zoom_at(scroll, uv(rect, p), aspect),
                    None => self.camera.zoom(scroll),
                }
                ctx.request_repaint();
            }
        }
        let editing = scene.preview.is_none() && scene.editable;
        let in_preview = scene.preview.is_some();
        let measuring = self.measure_on && editing;
        // Section plane of this mode (Prepare cuts objects, Preview toolpaths).
        let section_bounds = self.section_bounds(&scene);
        let section_eq = self
            .section
            .plane(in_preview)
            .map(|p| p.equation(section_bounds));
        let probe = ui.painter_at(rect);
        self.planes.retain(scene.objects);
        if self.lay_on_face && editing {
            // Start finding every object's planes as soon as the tool is on.
            for o in scene.objects {
                self.planes.planes(o, &ctx);
            }
        }
        let layout = self.gizmo_layout(&scene, rect.height());
        let pick_radius = self.camera.mm_per_pixel(rect.height()) * 7.;
        self.measure.retain(scene.objects);
        // Painting: the brush follows the selected object's surface under the pointer.
        self.paint_hover = None;
        let hovered_before = self.paint_hover_hit.take();
        if self.paint.is_some()
            && editing
            && let Some(p) = pointer
            && response.hovered()
            && let Some((_, hit, normal)) = self.paint_hit(&scene, rect, p, aspect)
        {
            self.paint_hover = Some((hit.point, normal));
            self.paint_hover_hit = Some(hit);
        }
        if self.paint_hover_hit != hovered_before {
            // The editor builds the Fill preview from the hover of the frame before.
            ctx.request_repaint();
        }
        self.tool_hover = None;
        if self.object_tool.is_some()
            && editing
            && let Some(p) = pointer
            && response.hovered()
            && let Some((_, hit, _)) = self.paint_hit(&scene, rect, p, aspect)
        {
            self.tool_hover = Some(hit.point);
        }
        // Hover: gizmo handles first, then objects (and the tower), then plates.
        self.bed_hover = None;
        if matches!(self.drag, Drag::None) {
            self.hover_handle = None;
            self.hovered = None;
            // On the hovered plate's label or icons the plate stays hovered.
            self.hovered_plate = self
                .plate_hover_zone
                .filter(|(_, zone)| pointer.is_some_and(|p| zone.contains(p)))
                .map(|(i, _)| i);
            self.hovered_volume = None;
            self.hovered_tower = false;
            self.section.hover = None;
            let gp_probe = gizmo::Painter {
                painter: &probe,
                rect,
                camera: &self.camera,
            };
            if let Some(p) = pointer
                && response.hovered()
            {
                let (origin, dir) = self.camera.ray(uv(rect, p), aspect);
                if let Some(eq) = section_eq {
                    self.section.hover = self.section.hit(&gp_probe, eq, section_bounds, p);
                }
                if let Some(l) = &layout
                    && self.section.hover.is_none()
                {
                    self.hover_handle = l.hit(origin, dir, pick_radius);
                }
                if measuring && self.section.hover.is_none() {
                    self.measure
                        .update_hover(scene.objects, (origin, dir), &gp_probe, Some(p));
                } else {
                    self.measure
                        .update_hover(scene.objects, (origin, dir), &gp_probe, None);
                }
                if editing && self.hover_handle.is_none() && self.section.hover.is_none() {
                    let hit = picking::pick_face(scene.objects, origin, dir);
                    self.hovered = hit.map(|h| h.object);
                    if self.paint.is_none()
                        && let Some(d) = pick_tower(&scene, origin, dir)
                        && hit.is_none_or(|h| d < h.distance)
                    {
                        self.hovered_tower = true;
                        self.hovered = None;
                    }
                    // A part of the selected object under the pointer (x-ray, so a modifier
                    // inside the body is findable), or with Option held any part.
                    let direct = ctx.input(|i| i.modifiers.alt);
                    if (scene.selected.is_some() || direct)
                        && !self.hovered_tower
                        && let Some((i, Some(id))) = picking::click_target(
                            scene.objects,
                            scene.selected,
                            origin,
                            dir,
                            direct,
                        )
                    {
                        self.hovered_volume = Some(id);
                        self.hovered = Some(i);
                    }
                }
                if let Some(bp) = self.camera.bed_point(uv(rect, p), aspect) {
                    self.bed_hover = Some(bp);
                    if self.hovered.is_none()
                        && let Some(plate) =
                            plates::plate_at(scene.bed, scene.plates, bp.truncate())
                    {
                        self.hovered_plate = Some(plate);
                    }
                }
            } else {
                self.measure
                    .update_hover(scene.objects, (Vec3::ZERO, Vec3::Z), &gp_probe, None);
            }
        }
        // Drags.
        let delta = ctx.input(|i| i.pointer.delta());
        let (shift, command) = ctx.input(|i| (i.modifiers.shift, i.modifiers.command));
        if response.drag_started_by(PointerButton::Primary)
            && let Some(p) = ctx.input(|i| i.pointer.press_origin())
        {
            let (origin, dir) = self.camera.ray(uv(rect, p), aspect);
            self.drag = Drag::Orbit;
            // What was under the pointer where the button went down (a fast pointer may be
            // elsewhere by the time egui reports the drag), handles first, then objects.
            let pressed_handle = layout
                .as_ref()
                .and_then(|l| l.hit(origin, dir, pick_radius));
            let pressed_object = if editing && pressed_handle.is_none() {
                picking::pick(scene.objects, origin, dir)
            } else {
                None
            };
            let pressed_tower = editing
                && pressed_handle.is_none()
                && self.paint.is_none()
                && !measuring
                && scene.cut.is_none()
                && pick_tower(&scene, origin, dir).is_some_and(|d| {
                    picking::pick_face(scene.objects, origin, dir).is_none_or(|h| d < h.distance)
                });
            let section_grab = section_eq.and_then(|eq| {
                self.section.hit(
                    &gizmo::Painter {
                        painter: &probe,
                        rect,
                        camera: &self.camera,
                    },
                    eq,
                    section_bounds,
                    p,
                )
            });
            if let Some(h) = section_grab {
                let mm = self.camera.mm_per_pixel(rect.height());
                self.section
                    .begin(in_preview, h, section_bounds, (origin, dir), mm);
                self.drag = Drag::Section;
            } else if self.object_tool == Some(ObjectTool::LayerHeights)
                && editing
                && let Some((i, hit, _)) = self.paint_hit(&scene, rect, p, aspect)
                && let Some(o) = scene.objects.get(i)
            {
                actions.push(Action::GestureBegin("Layer heights"));
                self.drag = Drag::LayerEdit {
                    index: i,
                    z: f64::from(hit.point.z - o.bounds().0.z),
                    last_y: p.y,
                };
            } else if self.paint.is_some()
                && editing
                && let Some((i, hit, _)) = self.paint_hit(&scene, rect, p, aspect)
            {
                actions.push(Action::GestureBegin(
                    self.paint.map_or("Paint", crate::paint::tool_name),
                ));
                actions.push(Action::Paint {
                    index: i,
                    hit,
                    trail: vec![],
                    erase: shift,
                });
                self.drag = Drag::Paint {
                    index: i,
                    last: Some((p, hit)),
                };
            } else if let (Some(l), Some(h), Some(i)) = (&layout, pressed_handle, scene.selected)
                && let Some(drag) = gizmo::Drag::begin(*l, h, origin, dir)
            {
                actions.push(Action::GestureBegin(match (scene.cut.is_some(), h) {
                    (true, _) => "Cut plane",
                    (_, gizmo::Handle::Axis(_) | gizmo::Handle::Plane(_)) => "Move",
                    (_, gizmo::Handle::Ring(_)) => "Rotate",
                    (_, gizmo::Handle::ScaleAxis(_) | gizmo::Handle::Uniform) => "Scale",
                }));
                let index = if scene.cut.is_some() { CUT_INDEX } else { i };
                self.drag = Drag::Gizmo { index, drag };
            } else if let (Some(l), Some(h), true, None) = (
                &layout,
                pressed_handle,
                scene.tower_selected,
                scene.selected,
            ) && let Some(drag) = gizmo::Drag::begin(*l, h, origin, dir)
            {
                actions.push(Action::GestureBegin(match h {
                    gizmo::Handle::Ring(_) => "Turn prime tower",
                    _ => "Move prime tower",
                }));
                self.drag = Drag::Gizmo {
                    index: TOWER_INDEX,
                    drag,
                };
            } else if pressed_tower
                && !self.lay_on_face
                && !shift
                && let Some(point) = self.camera.bed_point(uv(rect, p), aspect)
            {
                // Dragging the tower selects it and moves it on the bed.
                if !scene.tower_selected {
                    actions.push(Action::SelectTower);
                }
                actions.push(Action::GestureBegin("Move prime tower"));
                self.drag = Drag::Tower { last: point };
            } else if let Some(i) = pressed_object
                && editing
                && !shift
                && self.press_moves(&scene, i)
                && let Some(point) = picking::pick_face(scene.objects, origin, dir)
                    .map(|h| (origin + dir * h.distance).z)
                    .and_then(|z| Self::plane_point_at(&self.camera, rect, p, aspect, z))
            {
                // Press and drag on a model selects it and moves it in one gesture (`drag`):
                // a model of the selection drags the whole selection, any other model is
                // selected alone first. The grab point follows the pointer in the horizontal
                // plane through it.
                if let Some(index) = drag::press_selects(scene.selection, i) {
                    actions.push(Action::Select {
                        index: Some(index),
                        toggle: false,
                    });
                }
                actions.push(Action::GestureBegin("Move"));
                self.drag = Drag::Object {
                    index: i,
                    last: point,
                    press: p,
                    armed: false,
                };
            } else if editing
                && shift
                && pressed_object.is_none()
                && !pressed_tower
                && self.paint.is_none()
                && self.object_tool.is_none()
                && !measuring
            {
                // Shift-drag over empty space: rectangle selection.
                self.drag = Drag::Box {
                    start: p,
                    toggle: command,
                };
            }
        }
        if response.drag_started_by(PointerButton::Secondary)
            || response.drag_started_by(PointerButton::Middle)
        {
            self.drag = Drag::Pan;
        }
        let pivot = scene
            .selected
            .filter(|_| scene.preview.is_none())
            .and_then(|i| scene.objects.get(i))
            .map(|o| o.pivot_world())
            .or_else(|| {
                (scene.tower_selected && scene.preview.is_none())
                    .then(|| current_tower(&scene).map(|t| t.centre()))
                    .flatten()
            })
            .unwrap_or_else(|| {
                let origin = scene
                    .plates
                    .iter()
                    .find(|p| p.current)
                    .map_or(Vec2::ZERO, |p| p.origin);
                scene.bed.center() + origin.extend(0.)
            });
        if let Drag::Paint { index, last } = self.drag
            && let Some(p) = pointer
            && last.is_none_or(|(at, _)| at != p)
        {
            // Pointer samples from the previous position to this one, each raycast; runs of
            // hits become capsule strokes from the previous hit on, a miss ends a run.
            let radius = scene.paint.map_or(1., |v| v.radius);
            let px = radius / self.camera.mm_per_pixel(rect.height()).max(1e-6);
            let samples = match last {
                Some((at, _)) => crate::paint::stroke_samples(
                    glam::Vec2::new(at.x, at.y),
                    glam::Vec2::new(p.x, p.y),
                    px * 0.5,
                ),
                None => vec![glam::Vec2::new(p.x, p.y)],
            };
            let mut chain: Vec<crate::paint::Hit> = last.map(|(_, h)| h).into_iter().collect();
            let mut runs = vec![];
            let mut end = None;
            for s in samples {
                let at = Pos2::new(s.x, s.y);
                match self.paint_hit(&scene, rect, at, aspect) {
                    Some((i, hit, _)) if i == index => {
                        chain.push(hit);
                        end = Some((at, hit));
                    }
                    _ => {
                        runs.push(std::mem::take(&mut chain));
                        end = None;
                    }
                }
            }
            runs.push(chain);
            let fresh = |run: &Vec<crate::paint::Hit>| {
                // A run that is only the previous hit paints nothing new.
                !(run.len() == 1 && last.is_some_and(|(_, h)| h == run[0]))
            };
            for run in runs.into_iter().filter(|r| !r.is_empty() && fresh(r)) {
                let mut run = crate::paint::simplify_stroke(&run, radius / 10.);
                let hit = run.pop().unwrap();
                actions.push(Action::Paint {
                    index,
                    hit,
                    trail: run,
                    erase: shift,
                });
            }
            self.drag = Drag::Paint { index, last: end };
        }
        if let Drag::LayerEdit { index, z, last_y } = &mut self.drag
            && let Some(p) = pointer
        {
            // Dragging up thickens the band's layers, down thins them.
            let dy = *last_y - p.y;
            if dy != 0. {
                *last_y = p.y;
                let alt = ctx.input(|i| i.modifiers.alt);
                actions.push(Action::LayerEdit {
                    index: *index,
                    z: *z,
                    amount: f64::from(dy) * crate::layer_edit::MM_PER_PIXEL,
                    shift,
                    alt,
                });
            }
        }
        match &mut self.drag {
            Drag::None | Drag::Paint { .. } | Drag::Box { .. } | Drag::LayerEdit { .. } => {}
            Drag::Section => {
                if let Some(p) = pointer {
                    let ray = self.camera.ray(uv(rect, p), aspect);
                    if self.section.update(in_preview, section_bounds, ray, shift) && in_preview {
                        actions.push(Action::ViewChanged);
                    }
                }
            }
            Drag::Orbit => {
                if response.dragged_by(PointerButton::Primary) {
                    self.camera.orbit_around(Vec2::new(delta.x, delta.y), pivot);
                }
            }
            Drag::Pan => {
                self.camera.pan(Vec2::new(delta.x, delta.y), rect.height());
            }
            Drag::Object {
                index,
                last,
                press,
                armed,
            } => {
                if let Some(p) = pointer
                    && !*armed
                    && drag::move_threshold_met(*press, p)
                {
                    *armed = true;
                }
                if *armed
                    && let Some(p) = pointer
                    && let Some(point) = Self::plane_point_at(&self.camera, rect, p, aspect, last.z)
                {
                    let mut d = (point - *last).with_z(0.);
                    if shift && let Some(o) = scene.objects.get(*index) {
                        // Snap the resulting position to whole millimetres.
                        d = (o.position + d).round() - o.position;
                    }
                    if d.length_squared() > 0. {
                        *last += d;
                        self.feedback = scene.objects.get(*index).map(|o| {
                            let p = o.position + d;
                            format!("X {:.1}  Y {:.1} mm", p.x, p.y)
                        });
                        actions.push(Action::Transform {
                            index: *index,
                            delta: Delta::Translate(d),
                        });
                    }
                }
            }
            Drag::Tower { last } => {
                if let Some(p) = pointer
                    && let Some(point) = self.camera.bed_point(uv(rect, p), aspect)
                {
                    let mut d = (point - *last).with_z(0.);
                    if shift && let Some(t) = current_tower(&scene) {
                        let pos = Vec2::new(t.shape.position[0] as f32, t.shape.position[1] as f32);
                        d = ((pos + d.truncate()).round() - pos).extend(0.);
                    }
                    if d.length_squared() > 0. {
                        *last += d;
                        self.feedback = current_tower(&scene).map(|t| {
                            format!(
                                "X {:.1}  Y {:.1} mm",
                                t.shape.position[0] + d.x as f64,
                                t.shape.position[1] + d.y as f64
                            )
                        });
                        actions.push(Action::TowerTransform(Delta::Translate(d)));
                    }
                }
            }
            Drag::Gizmo { index, drag } => {
                if let Some(p) = pointer {
                    let (origin, dir) = self.camera.ray(uv(rect, p), aspect);
                    if let Some(delta) = drag.update(origin, dir, shift) {
                        if *index == CUT_INDEX {
                            actions.push(Action::CutPlane(delta));
                        } else if *index == TOWER_INDEX {
                            actions.push(Action::TowerTransform(delta));
                        } else {
                            actions.push(Action::Transform {
                                index: *index,
                                delta,
                            });
                        }
                    }
                    self.feedback = Some(drag.feedback());
                }
            }
        }
        let mut box_rect: Option<(Rect, bool)> = None;
        if let (Drag::Box { start, toggle }, Some(p)) = (&self.drag, pointer) {
            box_rect = Some((Rect::from_two_pos(*start, p), *toggle));
        }
        let mut finished_box: Option<(Rect, bool)> = None;
        if response.drag_stopped() {
            if self.dragging() {
                actions.push(Action::Edited);
            }
            self.section.end();
            if let Some(b) = box_rect.take() {
                finished_box = Some(b);
            }
            self.drag = Drag::None;
            self.feedback = None;
        }
        // Brim ears: a click on an ear removes it, a click on the model places one.
        if self.object_tool == Some(ObjectTool::BrimEars)
            && editing
            && response.clicked()
            && let Some(p) = response.interact_pointer_pos()
            && let Some(i) = scene.selected
            && let Some(o) = scene.objects.get(i)
        {
            let on_bed = self.camera.bed_point(uv(rect, p), aspect);
            let ear = on_bed.and_then(|b| crate::brim_ears::ear_at(o, b.truncate()));
            if let Some(ear) = ear {
                actions.push(Action::RemoveBrimEar { index: i, ear });
            } else if let Some((i, hit, _)) = self.paint_hit(&scene, rect, p, aspect) {
                actions.push(Action::AddBrimEar {
                    index: i,
                    at: hit.point.truncate(),
                });
            }
        }
        // A click without movement in a painting tool is a dab.
        if self.paint.is_some()
            && editing
            && response.clicked()
            && let Some(p) = response.interact_pointer_pos()
            && let Some((i, hit, _)) = self.paint_hit(&scene, rect, p, aspect)
        {
            actions.push(Action::GestureBegin(
                self.paint.map_or("Paint", crate::paint::tool_name),
            ));
            actions.push(Action::Paint {
                index: i,
                hit,
                trail: vec![],
                erase: shift,
            });
            actions.push(Action::Edited);
        }
        // Clicks: type an angle, lay on face, select, focus, select plate. Placing cut
        // connectors, quick clicks are single clicks (a double click focused the model and
        // swallowed the second connector).
        let placing_connectors = editing && scene.cut.is_some_and(|c| c.editing);
        if response.double_clicked()
            && !placing_connectors
            && let Some(p) = response.interact_pointer_pos()
        {
            let (origin, dir) = self.camera.ray(uv(rect, p), aspect);
            let on_ring = layout.as_ref().is_some_and(|l| {
                l.mode == gizmo::Mode::Rotate
                    && l.hit(origin, dir, pick_radius * 1.5) == Some(gizmo::Handle::Ring(2))
            });
            if on_ring {
                let current = scene
                    .selected
                    .and_then(|i| scene.objects.get(i))
                    .map_or(0., |o| gizmo::wrap_degrees(o.angle.to_degrees()));
                self.angle_edit = Some(format!("{current:.0}"));
                // The double-click also started a drag: drop it without recording anything.
                if self.dragging() {
                    self.drag = Drag::None;
                    actions.push(Action::GestureCancel);
                }
            } else if editing && let Some((index, volume)) = text_under(&scene, origin, dir) {
                // A double-click on a text opens the Text tool on it. The press also began a
                // move: dropped first (its restore would undo the opening).
                if self.dragging() {
                    self.drag = Drag::None;
                    actions.push(Action::GestureCancel);
                }
                actions.push(Action::EditText { index, volume });
            } else {
                match picking::pick(scene.objects, origin, dir) {
                    Some(i) if scene.preview.is_none() => {
                        if let Some(o) = scene.objects.get(i) {
                            self.focus(o, aspect);
                        }
                        actions.push(Action::Select {
                            index: Some(i),
                            toggle: false,
                        });
                    }
                    _ => self.fit_plate(scene.bed, scene.plates, false, aspect),
                }
            }
        } else if response.clicked()
            && let Some(p) = response.interact_pointer_pos()
        {
            let (origin, dir) = self.camera.ray(uv(rect, p), aspect);
            let plate_under = || {
                self.camera
                    .bed_point(uv(rect, p), aspect)
                    .and_then(|bp| plates::plate_at(scene.bed, scene.plates, bp.truncate()))
                    .filter(|i| scene.plates.get(*i).is_some_and(|pl| !pl.current))
            };
            let toggle = shift || command;
            if measuring {
                // The measure tool owns clicks: they select features, never objects.
                self.measure.click(shift);
            } else if editing && (self.paint.is_some() || self.object_tool.is_some()) {
                // Painting: clicks on the model are dabs (handled above); elsewhere nothing.
                // Object tools handle their clicks above too.
            } else if editing && let Some(cut) = scene.cut.filter(|c| c.editing) {
                // "Add connectors": a click on the plane places or picks a connector.
                if let Some(at) = cut.ray_point(origin, dir) {
                    actions.push(Action::CutConnector(crate::cut::ConnectorEdit::Click(at)));
                }
            } else if editing && scene.cut.is_some() {
                // The cut tool keeps its object; clicks elsewhere do nothing.
            } else if editing && self.lay_on_face {
                if let Some(h) =
                    Self::plane_under(&mut self.planes, scene.objects, origin, dir, &ctx)
                {
                    actions.push(Action::LayOnPlane {
                        index: h.object,
                        normal: h.normal,
                    });
                    actions.push(Action::Edited);
                }
            } else if editing
                && !toggle
                && !self.lay_on_face
                && let Some(d) = pick_tower(&scene, origin, dir)
                && picking::pick_face(scene.objects, origin, dir).is_none_or(|h| d < h.distance)
            {
                actions.push(Action::SelectTower);
            } else if editing {
                let hit = picking::pick_face(scene.objects, origin, dir);
                // Drill-down: a part of the selected object under the click picks it (its
                // body again leaves the part); Option-click picks any part directly.
                let direct = ctx.input(|i| i.modifiers.alt);
                let volume = (!toggle && !self.lay_on_face && !self.emboss)
                    .then(|| {
                        picking::click_target(scene.objects, scene.selected, origin, dir, direct)
                    })
                    .flatten()
                    .and_then(|(i, v)| v.map(|id| (i, id)));
                // With the Text tool open, a click on another text moves the tool to it.
                let other_text = (self.emboss_open && !toggle)
                    .then(|| text_under(&scene, origin, dir))
                    .flatten()
                    .filter(|&(i, v)| scene.selected != Some(i) || scene.sub != v.map(Sub::Volume));
                match (hit, volume) {
                    (_, Some((i, id))) => actions.push(Action::SelectSub {
                        index: i,
                        sub: Some(Sub::Volume(id)),
                    }),
                    _ if other_text.is_some() => {
                        if let Some((index, volume)) = other_text {
                            actions.push(Action::EditText { index, volume });
                        }
                    }
                    (Some(h), _) if self.emboss && !toggle => {
                        if let Some(o) = scene.objects.get(h.object) {
                            let t = o.transform();
                            let [a, b, c] = o.mesh.faces[h.face].map(|k| {
                                t.transform_point3(Vec3::from(o.mesh.vertices[k as usize]))
                            });
                            let mut normal = (b - a).cross(c - a).normalize_or_zero();
                            if t.determinant() < 0. {
                                normal = -normal;
                            }
                            actions.push(Action::EmbossPlace {
                                index: h.object,
                                point: origin + dir * h.distance,
                                normal,
                            });
                        }
                    }
                    (Some(h), _) if scene.sub.is_some() && scene.selected == Some(h.object) => {
                        actions.push(Action::SelectSub {
                            index: h.object,
                            sub: None,
                        })
                    }
                    (Some(h), _) => actions.push(Action::Select {
                        index: Some(h.object),
                        toggle,
                    }),
                    (None, _) => match plate_under() {
                        Some(i) if !toggle => actions.push(Action::SelectPlate(i)),
                        _ if toggle => {}
                        _ => actions.push(Action::Select {
                            index: None,
                            toggle: false,
                        }),
                    },
                }
            } else if scene.preview.is_none()
                && let Some(i) = plate_under()
            {
                actions.push(Action::SelectPlate(i));
            }
        }
        // Right-click: remember what is under the pointer; the menu below acts on it.
        if editing
            && response.secondary_clicked()
            && let Some(p) = response.interact_pointer_pos()
        {
            let (origin, dir) = self.camera.ray(uv(rect, p), aspect);
            let hit = picking::pick_face(scene.objects, origin, dir);
            self.menu_tower =
                pick_tower(&scene, origin, dir).is_some_and(|d| hit.is_none_or(|h| d < h.distance));
            self.menu_target = Some(hit.map(|h| h.object).filter(|_| !self.menu_tower));
            if self.menu_tower && !scene.tower_selected {
                actions.push(Action::SelectTower);
            }
            // The bed menu pastes where it was opened.
            self.menu_plate = None;
            if let Some(bp) = self.camera.bed_point(uv(rect, p), aspect) {
                self.bed_hover = Some(bp);
                // On a plate's bed the menu is that plate's (it becomes the current one).
                if hit.is_none() && !self.menu_tower {
                    self.menu_plate = plates::plate_at(scene.bed, scene.plates, bp.truncate());
                    if let Some(i) = self.menu_plate
                        && scene.plates.get(i).is_some_and(|pl| !pl.current)
                    {
                        actions.push(Action::SelectPlate(i));
                    }
                }
            }
        }
        if editing {
            let target = self.menu_target;
            let info = menu_info(&scene, target.flatten());
            let tower_menu = self.menu_tower;
            let menu_plate = self.menu_plate;
            let mut plate_pick = None;
            let tower_name = current_tower(&scene).map_or("Prime tower", |t| t.label());
            response.context_menu(|ui| {
                if tower_menu {
                    if let Some(c) = tower_menu_ui(ui, tower_name, !info.locked) {
                        actions.push(Action::TowerCommand(c));
                    }
                    return;
                }
                let command = match target {
                    Some(Some(_)) => commands::object_menu(ui, &info),
                    _ => commands::bed_menu(ui, &info),
                };
                if let Some(command) = command {
                    actions.push(Action::Command {
                        index: target.flatten(),
                        command,
                    });
                }
                // The bed's plate: its whole menu, under the bed commands.
                if !matches!(target, Some(Some(_)))
                    && let Some(i) = menu_plate
                    && let Some(plate) = scene.plates.get(i)
                {
                    ui.separator();
                    let name = plate
                        .name
                        .clone()
                        .unwrap_or_else(|| crate::plates::default_name(i));
                    let pinfo = crate::plate_menu::PlateMenuInfo {
                        index: i,
                        count: scene.plates.len(),
                        name: name.clone(),
                        locked: plate.locked,
                        objects: plate.objects,
                        editable: scene.editable,
                    };
                    if let Some(c) = crate::plate_menu::plate_menu(ui, &pinfo) {
                        plate_pick = Some((i, c, name));
                    }
                }
            });
            if let Some((i, c, name)) = plate_pick {
                self.pick(i, c, &name, &mut actions);
            }
        } else {
            self.menu_target = None;
            self.menu_tower = false;
        }
        // 3D render.
        let scale = ctx.pixels_per_point();
        let moving = match scene.preview.as_ref() {
            Some((p, v)) => self.preview_moving(&ctx, aspect, v.range(p)),
            None => false,
        };
        self.moving = moving;
        let dark = ui.visuals().dark_mode;
        let caps = match section_eq {
            Some(eq) if !in_preview => self.section.caps(scene.objects, eq, |o| {
                let tool = o.tools.0[0].unwrap_or(0);
                section::cap_color(
                    scene
                        .tool_colors
                        .get(tool)
                        .copied()
                        .unwrap_or_else(|| crate::preview::legend::tool_color(tool)),
                )
            }),
            _ => vec![],
        };
        // Plate tags on the ground next to each plate, hovered unless an object stands in
        // front of the ground under the pointer.
        {
            let gp_tags = gizmo::Painter {
                painter: &probe,
                rect,
                camera: &self.camera,
            };
            let camera = &self.camera;
            let tag_pointer = pointer.filter(|p| rect.contains(*p));
            self.plate_tags = plate_tags::layout(
                &ctx,
                &mut self.tag_cache,
                scene.bed,
                scene.plates,
                dark,
                rect,
                |w| gp_tags.project(w),
                tag_pointer,
                || {
                    tag_pointer.is_some_and(|p| {
                        let (origin, dir) = camera.ray(uv(rect, p), aspect);
                        picking::pick_face(scene.objects, origin, dir).is_some()
                    })
                },
            );
        }
        let frame = render::Frame {
            camera: self.camera.clone(),
            bed: scene.bed.clone(),
            plates: scene.plates.to_vec(),
            objects: scene.objects.to_vec(),
            plate_of: scene.plate_of.to_vec(),
            volumes: if scene.preview.is_none() {
                self.volume_draws(&scene)
            } else {
                vec![]
            },
            selected: scene.selected.filter(|_| scene.preview.is_none()),
            selection: if scene.preview.is_none() {
                scene.selection.to_vec()
            } else {
                vec![]
            },
            hovered: self.hovered.filter(|_| editing),
            tool_colors: scene.tool_colors.to_vec(),
            overlays: if scene.preview.is_none() {
                scene.overlays.to_vec()
            } else {
                vec![]
            },
            preview: scene.preview.as_ref().map(|(p, _)| Arc::clone(p)),
            view: scene
                .preview
                .as_ref()
                .map_or_else(View::default, |(_, v)| (*v).clone()),
            size: [
                (rect.width() * scale).max(1.) as u32,
                (rect.height() * scale).max(1.) as u32,
            ],
            dark,
            clip: section_eq,
            caps,
            clearance: if scene.preview.is_none() {
                scene.clearance.to_vec()
            } else {
                vec![]
            },
            warned: if scene.preview.is_none() {
                scene.warned.iter().map(|(i, _)| *i).collect()
            } else {
                vec![]
            },
            surface_textures: if scene.preview.is_none() {
                crate::textures::published(ui.ctx())
            } else {
                vec![]
            },
            moving,
            pick: in_preview.then(|| self.pick.clone()),
            tags: self.plate_tags.iter().map(|t| t.draw.clone()).collect(),
        };
        ui.painter()
            .add(egui_wgpu::Callback::new_paint_callback(rect, frame));
        // Overlays drawn in 2D.
        let painter = ui.painter_at(rect);
        let gp = gizmo::Painter {
            painter: &painter,
            rect,
            camera: &self.camera,
        };
        let (zone, picks) = self.plate_labels(ui, &gp, &scene, &mut actions);
        self.plate_hover_zone = zone;
        for (index, pick, name) in picks {
            // As `pick`, field by field (the painter borrows the camera).
            match pick {
                None => {
                    actions.push(Action::SelectPlate(index));
                    self.open_plate_menu = true;
                }
                Some(crate::plate_menu::PlateCommand::Rename) => {
                    actions.push(Action::SelectPlate(index));
                    self.plate_rename = Some(PlateRename {
                        index,
                        text: name,
                        focus: true,
                    });
                }
                Some(crate::plate_menu::PlateCommand::Reorder) => self.open_plate_list = true,
                Some(c) => actions.push(Action::Plate(index, c)),
            }
        }
        if scene.preview.is_none() {
            self.object_labels(ui, &gp, &scene);
            self.tower_labels(ui, &gp, &scene);
        }
        // Rectangle selection: the box while dragging, the objects in it on release.
        if let Some((r, _)) = box_rect {
            painter.rect_filled(r, 2., theme::ACCENT.gamma_multiply(0.12));
            painter.rect_stroke(
                r,
                2.,
                egui::Stroke::new(1., theme::ACCENT_BRIGHT),
                egui::StrokeKind::Inside,
            );
        }
        if let Some((r, toggle)) = finished_box {
            let indices = self.objects_in_rect(&gp, scene.objects, r);
            actions.push(Action::BoxSelect { indices, toggle });
        }
        if self.lay_on_face
            && editing
            && let Some(p) = pointer
            && response.hovered()
        {
            // Show which flat plane a click would lay on the bed: every plane of the object
            // under the pointer that faces the camera faintly, the one under it in full.
            let (origin, dir) = self.camera.ray(uv(rect, p), aspect);
            let hit = Self::plane_under(&mut self.planes, scene.objects, origin, dir, &ctx);
            let object = hit
                .as_ref()
                .map(|h| h.object)
                .or_else(|| picking::pick_face(scene.objects, origin, dir).map(|h| h.object));
            if let Some(oi) = object
                && let Some(o) = scene.objects.get(oi)
                && let Some(planes) = self.planes.planes(o, &ctx)
            {
                let r = o.rotation();
                for (pi, plane) in planes.iter().enumerate() {
                    let hovered = hit.as_ref().is_some_and(|h| h.plane == pi);
                    let n = r * Vec3::from(plane.normal.map(|c| c as f32));
                    if !hovered && n.dot(dir) >= 0. {
                        continue;
                    }
                    let pts: Vec<Pos2> = flatten::world_outline(o, &plane.outline)
                        .into_iter()
                        .filter_map(|q| gp.project(q))
                        .collect();
                    if pts.len() != plane.outline.len() {
                        continue;
                    }
                    let (fill, stroke) = if hovered {
                        (
                            theme::ACCENT_BRIGHT.gamma_multiply(0.55),
                            egui::Stroke::new(1.5, theme::ACCENT_BRIGHT),
                        )
                    } else {
                        (
                            Color32::from_white_alpha(28),
                            egui::Stroke::new(1., Color32::from_white_alpha(60)),
                        )
                    };
                    painter.add(egui::Shape::convex_polygon(pts, fill, stroke));
                }
            }
        }
        if let (Some(view), Some((hit, normal))) = (scene.paint, self.paint_hover)
            && editing
        {
            self.brush_cursor(&painter, &gp, view, hit, normal, shift);
        }
        if measuring {
            self.measure.draw(&gp, scene.objects, shift);
        }
        if section_eq.is_some() {
            self.section.draw(&gp, in_preview, section_bounds);
        }
        if let Some(view) = &scene.object_tool
            && editing
        {
            self.object_tool_cursor(&painter, &gp, &scene, view, pointer);
        }
        if let Some(l) = &layout {
            let (active, sweep, zone) = match &self.drag {
                Drag::Gizmo { drag, .. } => (
                    Some(drag.handle),
                    matches!(drag.handle, gizmo::Handle::Ring(_)).then(|| drag.turned()),
                    drag.zone(),
                ),
                _ => (
                    None,
                    None,
                    if shift {
                        gizmo::SnapZone::Shift
                    } else {
                        gizmo::SnapZone::Fine
                    },
                ),
            };
            let state = gizmo::RotateState { sweep, zone };
            gp.draw(l, self.hover_handle, active, state);
            if l.mode == gizmo::Mode::Rotate {
                // The label follows the hot ring: the bed ring shows the yaw (and can be typed
                // into), a tilt ring the angle turned in this drag (PrusaSlicer's tooltip).
                // While an angle is being typed the label stays on the bed ring.
                let axis = match active.or(self.hover_handle) {
                    Some(gizmo::Handle::Ring(i)) if self.angle_edit.is_none() => i,
                    _ => 2,
                };
                let ring = l.ring(axis);
                let angle_deg = if axis == 2 {
                    scene
                        .selected
                        .and_then(|i| scene.objects.get(i))
                        .map(|o| o.angle.to_degrees())
                        .or_else(|| current_tower(&scene).map(|t| t.shape.rotation as f32))
                        .unwrap_or(0.)
                } else {
                    sweep.unwrap_or(0.).to_degrees()
                };
                let hot = active == Some(gizmo::Handle::Ring(axis))
                    || self.hover_handle == Some(gizmo::Handle::Ring(axis));
                let handle = gizmo::Painter::handle_angle(&ring, active, state);
                let label_rect = gp.angle_label(&ring, handle, angle_deg, zone, hot);
                if let (Some(text), Some(r)) = (self.angle_edit.as_mut(), label_rect) {
                    // Typing an angle: a mono field over the label; Enter applies, Esc cancels.
                    let id = ui.make_persistent_id("angle-edit");
                    let field =
                        Rect::from_min_size(r.min, EVec2::new(r.width().max(96.), r.height()));
                    painter.rect_filled(field, 6., theme::DARK.dark_box);
                    let resp = ui.put(
                        field,
                        egui::TextEdit::singleline(text)
                            .id(id)
                            .font(theme::mono(13.))
                            .desired_width(field.width() - 8.)
                            .margin(egui::Margin::symmetric(8, 4)),
                    );
                    resp.request_focus();
                    let (enter, escape) =
                        ctx.input(|i| (i.key_pressed(Key::Enter), i.key_pressed(Key::Escape)));
                    if escape {
                        self.angle_edit = None;
                    } else if enter {
                        let typed = text.trim().trim_end_matches('°').trim().replace(',', ".");
                        if let (Ok(deg), Some(i)) = (typed.parse::<f32>(), scene.selected)
                            && let Some(o) = scene.objects.get(i)
                        {
                            let mut euler = o.euler();
                            euler.z = deg.to_radians();
                            actions.push(Action::SetRotation { index: i, euler });
                            actions.push(Action::Edited);
                        } else if let (Ok(deg), true, Some(t)) = (
                            typed.parse::<f32>(),
                            scene.tower_selected,
                            current_tower(&scene),
                        ) {
                            actions.push(Action::GestureBegin("Turn prime tower"));
                            actions.push(Action::TowerTransform(Delta::Rotate {
                                axis: Vec3::Z,
                                angle: (deg - t.shape.rotation as f32).to_radians(),
                            }));
                            actions.push(Action::Edited);
                        }
                        self.angle_edit = None;
                    }
                }
            } else {
                self.angle_edit = None;
            }
        }
        if let Some(text) = &self.feedback
            && let Some(p) = pointer
            && !matches!(
                self.drag,
                Drag::Gizmo {
                    drag: gizmo::Drag {
                        handle: gizmo::Handle::Ring(_),
                        ..
                    },
                    ..
                }
            )
        {
            gp.readout(p + EVec2::new(16., 14.), text);
        } else if editing
            && let Some(i) = self.hovered
            && scene.selected != Some(i)
            && let Some(p) = pointer
            && let Some(o) = scene.objects.get(i)
        {
            tooltip(
                &painter,
                rect,
                p + EVec2::new(16., 12.),
                &format!("{} · click to select", o.name),
                true,
            );
        } else if editing
            && self.hovered_tower
            && !scene.tower_selected
            && let Some(p) = pointer
            && let Some(t) = current_tower(&scene)
        {
            tooltip(
                &painter,
                rect,
                p + EVec2::new(16., 12.),
                &format!("{} · click to select, drag to move", t.label()),
                true,
            );
        }
        if scene.objects.is_empty() && scene.preview.is_none() {
            let open = ctx.format_shortcut(&egui::KeyboardShortcut::new(
                egui::Modifiers::COMMAND,
                Key::O,
            ));
            let p = theme::of(ui);
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                "Drop an STL or 3MF here",
                theme::semibold(20.),
                p.text,
            );
            painter.text(
                rect.center() + EVec2::new(0., 28.),
                Align2::CENTER_CENTER,
                format!("or press {open} to open a model"),
                theme::sans(13.),
                p.muted_alt,
            );
        }
        self.hint(ui, rect, &scene);
        // Axis triad, bottom left.
        let triad_rect = Rect::from_min_size(
            Pos2::new(rect.left() + 10., rect.bottom() - 86.),
            EVec2::splat(76.),
        );
        if let Some(preset) = axes::triad(ui, triad_rect, &self.camera) {
            self.camera.preset(preset);
        }
        // Preview with several plates: the plate list at the left edge replaces the plate bar.
        let plate_list = (scene.preview.is_some() || scene.previewing) && scene.plates.len() > 1;
        if scene.preview.is_none() {
            // Preview without a slice to show (`previewing`): the models, but no editing tools.
            if !scene.previewing {
                self.toolbar(ui, rect, &scene, &mut actions);
            }
            if !plate_list {
                self.plate_bar(ui, rect, &scene, &mut actions);
            }
            self.totals_card(ui, rect, &scene);
        } else {
            self.preview_toolbar(ui, rect);
        }
        if plate_list {
            self.preview_plates(ui, rect, &scene, &mut actions);
        } else {
            self.preview_plates_probe = Default::default();
            self.preview_plates_shown = None;
        }
        if measuring {
            self.measure.card(ui, rect);
        }
        if self.section.active(in_preview)
            && self
                .section
                .card(ui, rect, in_preview, self.camera.direction())
            && in_preview
        {
            actions.push(Action::ViewChanged);
        }
        let preview_model = render::preview_model(scene.plates);
        if let Some((preview, view)) = scene.preview.as_mut() {
            let p: &Arc<Preview> = preview;
            self.preview_overlays(
                ui,
                rect,
                p,
                preview_model,
                view,
                pointer,
                section_eq,
                scene.layer_gcode.as_ref(),
                &mut actions,
            );
        }
        actions
    }
    /// The translucent volumes to draw: every object's modifiers, negative and support
    /// volumes (the selected one highlighted), the selected object's height ranges as slabs
    /// (brighter for the selected range) and the cut plane.
    fn volume_draws(&self, scene: &Scene<'_>) -> Vec<render::VolumeDraw> {
        let mut out = vec![];
        for (i, o) in scene.objects.iter().enumerate() {
            let primary = scene.selected == Some(i);
            for v in &o.volumes {
                let selected = primary && scene.sub == Some(Sub::Volume(v.id));
                let (color, cutout) = render::volume_color(&v.kind, selected);
                out.push(render::VolumeDraw {
                    mesh: Some((v.id, v.mesh.clone())),
                    // A text's mesh is rebuilt under the same id as it is edited.
                    tracked: true,
                    transform: o.volume_transform(v),
                    color,
                    cutout,
                    selected,
                    hovered: self.hovered_volume == Some(v.id) && !selected,
                });
            }
            if primary && !o.height_ranges.is_empty() {
                let (min, max) = o.bounds();
                let bottom = o.lowest_z();
                let pad = 3.;
                for r in &o.height_ranges {
                    let selected = scene.sub == Some(Sub::Range(r.id));
                    let lo = bottom + r.min_z;
                    let hi = bottom + r.max_z;
                    let size = Vec3::new(
                        max.x - min.x + 2. * pad,
                        max.y - min.y + 2. * pad,
                        (hi - lo).max(0.05),
                    );
                    let centre = Vec3::new(
                        (min.x + max.x) * 0.5,
                        (min.y + max.y) * 0.5,
                        (lo + hi) * 0.5,
                    );
                    let mut color = render::RANGE_COLOR;
                    if selected {
                        color[3] = 0.36;
                    }
                    out.push(render::VolumeDraw {
                        mesh: None,
                        tracked: false,
                        transform: glam::Mat4::from_scale_rotation_translation(
                            size,
                            glam::Quat::IDENTITY,
                            centre,
                        ),
                        color,
                        cutout: false,
                        selected,
                        hovered: false,
                    });
                }
            }
        }
        if let Some(cut) = scene.cut {
            out.push(render::VolumeDraw {
                mesh: None,
                tracked: false,
                transform: cut.quad_transform(),
                color: render::CUT_PLANE_COLOR,
                cutout: false,
                selected: true,
                hovered: false,
            });
            // Connectors as they will be built: plugs and snaps standing on the plane,
            // dowels through it.
            for (i, c) in cut.connectors.iter().enumerate() {
                let a = c.attributes.effective();
                let selected = cut.selected_connector == Some(i);
                let conflict = cut.conflicts.get(i).is_some_and(|c| c.is_some());
                out.push(render::VolumeDraw {
                    mesh: Some(crate::cut::preview_mesh(a)),
                    tracked: false,
                    transform: cut
                        .connector_transform(c, mica_core::cut_connectors::body(a, &c.sizes)),
                    color: crate::cut::connector_color(a.kind, conflict, selected),
                    cutout: false,
                    selected,
                    hovered: false,
                });
            }
        }
        // Prime towers: the body box and the brim / rib footprint slab under it.
        let current = scene.plates.iter().position(|p| p.current);
        for t in scene.towers {
            let own = current == Some(t.plate);
            let selected = own && scene.tower_selected && scene.selected.is_none();
            let hovered = own && self.hovered_tower;
            let mut color = TOWER_COLOR;
            let mut footprint = TOWER_FOOTPRINT_COLOR;
            if selected {
                color[3] = 0.5;
                footprint[3] = 0.34;
            }
            if !t.problems.is_empty() {
                // Warning tint, like the stripes on an object outside the bed.
                color = [0.93, 0.6, 0.3, color[3]];
                footprint = [0.93, 0.6, 0.3, footprint[3]];
            }
            out.push(render::VolumeDraw {
                mesh: None,
                tracked: false,
                transform: t.body_transform(),
                color,
                cutout: false,
                selected,
                hovered,
            });
            out.push(render::VolumeDraw {
                mesh: Some((t.footprint_id(), Arc::new(t.footprint_mesh()))),
                tracked: false,
                transform: glam::Mat4::IDENTITY,
                color: footprint,
                cutout: false,
                selected: false,
                hovered,
            });
        }
        out
    }
    /// The tower's name and size over its box, and its warning, like an object's labels.
    fn tower_labels(&self, ui: &egui::Ui, gp: &gizmo::Painter<'_>, scene: &Scene<'_>) {
        let current = scene.plates.iter().position(|p| p.current);
        for t in scene.towers {
            let own = current == Some(t.plate);
            let (min, max) = t.bounds();
            let centre = (min + max) * 0.5;
            if let Some(p) = t.problems.first()
                && let Some(at) = gp.project(centre.with_z(max.z + 6.))
                && gp.rect.contains(at)
            {
                tooltip(
                    gp.painter,
                    gp.rect,
                    at - EVec2::new(0., 30.),
                    &format!("! {}", p.text()),
                    false,
                );
            }
            if own
                && !matches!(self.drag, Drag::Gizmo { .. } | Drag::Tower { .. })
                && let Some(p) = gp.project(Vec3::new(centre.x, centre.y, 0.))
                && gp.rect.contains(p)
            {
                let pal = theme::of(ui);
                let s = &t.shape;
                let text = if scene.tower_selected && scene.selected.is_none() {
                    format!(
                        "{} · {:.1} × {:.1} mm · {:.1} mm tall{}",
                        t.label(),
                        s.width,
                        s.depth,
                        s.height,
                        if s.exact { "" } else { " · estimated" }
                    )
                } else {
                    t.label().to_string()
                };
                let galley = gp
                    .painter
                    .layout_no_wrap(text, theme::mono(11.), pal.dark_box_text);
                let r = Rect::from_center_size(
                    p + EVec2::new(0., 22.),
                    galley.size() + EVec2::new(12., 4.),
                );
                gp.painter.rect_filled(r, 4., pal.dark_box);
                gp.painter
                    .galley(r.min + EVec2::new(6., 2.), galley, pal.dark_box_text);
            }
        }
    }

    /// The brush on the surface: a ring of the brush radius in the plane of the hit facet
    /// (a sphere brush adds a faint second ring for its depth), in the kind's paint colour,
    /// red-tinted while erasing.
    fn brush_cursor(
        &self,
        painter: &egui::Painter,
        gp: &gizmo::Painter<'_>,
        view: PaintView,
        hit: Vec3,
        normal: Vec3,
        shift: bool,
    ) {
        use crate::scene::paint as colors;
        let erase = view.erase || shift;
        let c = if erase {
            [0.95, 0.4, 0.35, 1.]
        } else {
            match view.kind {
                PaintKind::Supports => colors::SUPPORT_ENFORCER,
                PaintKind::Seam => colors::SEAM_ENFORCER,
                PaintKind::Material => colors::SUPPORT_ENFORCER,
                PaintKind::Fuzzy => colors::FUZZY,
                PaintKind::Texture => colors::TEXTURE_SLOTS[0],
            }
        };
        let color = Color32::from_rgb(
            (c[0] * 255.) as u8,
            (c[1] * 255.) as u8,
            (c[2] * 255.) as u8,
        );
        let n = normal.normalize_or_zero();
        let n = if n == Vec3::ZERO { Vec3::Z } else { n };
        let helper = if n.z.abs() > 0.9 { Vec3::X } else { Vec3::Z };
        let u = n.cross(helper).normalize();
        let v = n.cross(u);
        let ring = |radius: f32, lift: f32| -> Vec<Pos2> {
            (0..=48)
                .filter_map(|k| {
                    let a = k as f32 / 48. * std::f32::consts::TAU;
                    gp.project(hit + n * lift + (u * a.cos() + v * a.sin()) * radius)
                })
                .collect()
        };
        let draw = |pts: Vec<Pos2>, width: f32, alpha: f32| {
            if pts.len() > 2 {
                painter.add(egui::Shape::line(
                    pts.clone(),
                    egui::Stroke::new(width + 2., theme::OUTLINE.gamma_multiply(alpha)),
                ));
                painter.add(egui::Shape::line(
                    pts,
                    egui::Stroke::new(width, color.gamma_multiply(alpha)),
                ));
            }
        };
        match view.brush {
            crate::paint::BrushKind::Circle | crate::paint::BrushKind::Sphere => {
                draw(ring(view.radius, 0.05), 2., 1.);
                if view.brush == crate::paint::BrushKind::Sphere {
                    draw(ring(view.radius * 0.7, view.radius * 0.7), 1., 0.5);
                }
            }
            // Fill and height-range brushes act on the facet under a small marker.
            crate::paint::BrushKind::Fill | crate::paint::BrushKind::Height => {
                draw(ring(view.radius.min(2.), 0.05), 2., 1.);
            }
        }
        if let Some(c) = gp.project(hit) {
            painter.circle_filled(c, 2.5, color);
        }
    }

    /// What the object tools draw: the edited band around the object (layer heights), the
    /// ears on the bed and the ear a click would place (brim ears).
    fn object_tool_cursor(
        &self,
        painter: &egui::Painter,
        gp: &gizmo::Painter<'_>,
        scene: &Scene<'_>,
        view: &ObjectToolView,
        pointer: Option<Pos2>,
    ) {
        let Some(o) = scene.selected.and_then(|i| scene.objects.get(i)) else {
            return;
        };
        let (min, max) = o.bounds();
        let ring = |centre: Vec3, r: f32| -> Vec<Pos2> {
            (0..=32)
                .filter_map(|k| {
                    let a = k as f32 / 32. * std::f32::consts::TAU;
                    gp.project(centre + Vec3::new(a.cos(), a.sin(), 0.) * r)
                })
                .collect()
        };
        match view.tool {
            ObjectTool::LayerHeights => {
                let z = match self.drag {
                    Drag::LayerEdit { z, .. } => Some(min.z + z as f32),
                    _ => self.tool_hover.map(|h| h.z),
                };
                let Some(z) = z else { return };
                // The band: two outlines of the box at its edges, the centre line brighter.
                let pad = 1.;
                for (dz, width, alpha) in [
                    (-view.band_width / 2., 1.2, 0.7),
                    (view.band_width / 2., 1.2, 0.7),
                    (0., 2., 1.),
                ] {
                    let zz = (z + dz).clamp(min.z, max.z);
                    let pts: Vec<Pos2> = [
                        Vec3::new(min.x - pad, min.y - pad, zz),
                        Vec3::new(max.x + pad, min.y - pad, zz),
                        Vec3::new(max.x + pad, max.y + pad, zz),
                        Vec3::new(min.x - pad, max.y + pad, zz),
                        Vec3::new(min.x - pad, min.y - pad, zz),
                    ]
                    .into_iter()
                    .filter_map(|p| gp.project(p))
                    .collect();
                    if pts.len() == 5 {
                        painter.add(egui::Shape::line(
                            pts,
                            egui::Stroke::new(width, theme::ACCENT_BRIGHT.gamma_multiply(alpha)),
                        ));
                    }
                }
            }
            ObjectTool::BrimEars => {
                let ears = crate::brim_ears::world(o);
                let hovered = pointer
                    .and_then(|p| {
                        self.camera
                            .bed_point(uv(gp.rect, p), gp.rect.width() / gp.rect.height().max(1.))
                    })
                    .and_then(|b| crate::brim_ears::ear_at(o, b.truncate()));
                for (i, (c, r)) in ears.iter().enumerate() {
                    let loose = view.loose_ears.contains(&i);
                    let color = if Some(i) == hovered {
                        Color32::from_rgb(240, 100, 90)
                    } else if loose {
                        theme::WARNING
                    } else {
                        theme::ACCENT_BRIGHT
                    };
                    let pts = ring(c.with_z(0.), *r);
                    if pts.len() > 2 {
                        painter.add(egui::Shape::convex_polygon(
                            pts.clone(),
                            color.gamma_multiply(0.22),
                            egui::Stroke::NONE,
                        ));
                        painter.add(egui::Shape::line(pts, egui::Stroke::new(1.6, color)));
                    }
                    if let Some(p) = gp.project(c.with_z(0.)) {
                        painter.circle_filled(p, 2.5, color);
                    }
                }
                if hovered.is_none()
                    && let Some(h) = self.tool_hover
                {
                    let pts = ring(h.with_z(0.), view.ear_diameter / 2.);
                    if pts.len() > 2 {
                        painter.add(egui::Shape::line(
                            pts,
                            egui::Stroke::new(1.2, theme::ACCENT_BRIGHT.gamma_multiply(0.8)),
                        ));
                    }
                }
            }
        }
    }

    /// One plain line above the plate bar that says what the current mode does (or shows a
    /// fresh status message).
    fn hint(&self, ui: &egui::Ui, rect: Rect, scene: &Scene<'_>) {
        let text: String = if let Some(s) = &scene.status {
            s.clone()
        } else {
            let t = if scene.preview.is_some() {
                "Drag the right slider to scrub layers · ↑ ↓ step one layer · hover a line for details"
            } else if scene.previewing && scene.progress.is_none() {
                if scene.plates.len() > 1 {
                    "No slice to show for this plate: slice it from the Preview page, or pick another plate"
                } else {
                    "No slice to show: slice the plate from the Preview page"
                }
            } else if !scene.editable {
                "Slicing… the camera still works: drag to orbit, right-drag to pan, scroll to zoom"
            } else if scene.cut.is_some_and(|c| c.editing) {
                "Connectors: click the cut face to place one · click one to select it · Delete removes it · Esc when done"
            } else if scene.cut.is_some() {
                "Cut: drag the arrows to move the plane · R tilts it with the rings · Cut in the card · Esc cancels"
            } else if self.measure_on {
                match self.measure.hover_name() {
                    Some(_) if self.measure.second.is_some() => {
                        "Click another feature to measure against the first · Esc unselects · U leaves"
                    }
                    Some(_) => {
                        "Click to select · Shift picks a point on it · Esc unselects · U leaves"
                    }
                    None => "Measure: hover a point, edge, circle or flat face · U or Esc leaves",
                }
            } else if self.emboss {
                "Text: changes show as you type · click the model to move the text there · Done keeps it · Esc cancels"
            } else if self.lay_on_face {
                "Lay on face: click a highlighted flat side to rest it on the bed · Esc cancels"
            } else if self.object_tool == Some(ObjectTool::LayerHeights) {
                "Drag up on the model for thicker layers, down for thinner · Shift: back to normal · Alt: smooth · Alt-scroll: band width · Esc leaves"
            } else if self.object_tool == Some(ObjectTool::BrimEars) {
                "Click the model to place a brim ear · click an ear to remove it · Alt-scroll: ear size · Esc leaves"
            } else if self.paint.is_some() {
                if scene.selected.is_some() {
                    "Left-drag to paint · Shift-drag to erase · Alt-scroll to change brush size · Esc leaves the tool"
                } else {
                    "Click a model to paint on it · Esc leaves the tool"
                }
            } else if matches!(self.drag, Drag::Tower { .. }) {
                "Shift snaps to 1 mm · the tower stays on the bed · Esc cancels · release to finish"
            } else if scene.tower_selected && scene.selected.is_none() {
                match self.mode {
                    gizmo::Mode::Rotate => {
                        "Drag the handle to turn the prime tower · double-click to type an angle · Esc deselects"
                    }
                    _ => {
                        "Drag the prime tower or its arrows · arrow keys nudge 1 mm (Shift 10) · right-click for actions · Esc deselects"
                    }
                }
            } else if matches!(self.drag, Drag::Gizmo { .. } | Drag::Object { .. }) {
                match self.mode {
                    gizmo::Mode::Rotate => {
                        "Ring: 5° steps · inside: snap 45° · outside: free · Shift 1° · Esc cancels"
                    }
                    gizmo::Mode::Scale => "Shift snaps to 5 % · Esc cancels · release to finish",
                    _ => "Shift snaps to 1 mm · Esc cancels · release to finish",
                }
            } else if scene.selection.len() > 1 {
                "Several objects selected: they move, turn and scale together · Shift-click adds or removes · right-click for actions"
            } else if matches!(scene.sub, Some(Sub::Volume(_))) {
                "Modifier selected: the gizmos move it inside its object · its settings are in the sidebar · Esc back to the object"
            } else if matches!(scene.sub, Some(Sub::Range(_))) {
                "Height range selected: edit From / To in the card, its settings in the sidebar · Esc back to the object"
            } else if scene.selected.is_some() {
                match self.mode {
                    gizmo::Mode::Move => {
                        "Drag the model or the arrows to move · Shift snaps to 1 mm · Shift-click adds to the selection · right-click for actions"
                    }
                    gizmo::Mode::Rotate => {
                        "Drag a ring or its handle (blue turns, red / green tilt) · ring: 5° steps · inside: snap 45° · outside: free · Shift 1° · double-click to type"
                    }
                    gizmo::Mode::Scale => {
                        "Drag an end to scale one axis, the center cube to scale uniformly · Shift snaps to 5 %"
                    }
                    gizmo::Mode::None => "M R S choose a tool · Esc deselects",
                }
            } else if scene.objects.is_empty() {
                ""
            } else {
                "Click a model to select it, drag it to move it · Shift-drag a box to select several · Double-click to focus · Right-click for actions"
            };
            t.to_owned()
        };
        if text.is_empty() {
            return;
        }
        let painter = ui.painter_at(rect);
        // Above the plate bar (14 px up, its height from the toolbar size) with a gap; in
        // Preview above the move slider label.
        let y = if scene.preview.is_some() {
            rect.bottom() - 20.
        } else {
            rect.bottom() - 14. - icons::metrics(ui.ctx()).bar_height() - 6. - 8.
        };
        let color = if scene.status.is_some() {
            theme::DARK.text
        } else {
            theme::DARK.label
        };
        let galley = painter.layout_no_wrap(text, theme::sans(12.), color);
        let size = galley.size() + EVec2::new(20., 8.);
        let mut pill = Rect::from_center_size(Pos2::new(rect.center().x, y), size);
        // Clear the axis triad at the bottom left, then stay inside the view.
        if pill.left() < rect.left() + 96. {
            pill = pill.translate(EVec2::new(rect.left() + 96. - pill.left(), 0.));
        }
        pill = pill.translate(clamp_shift(rect, pill));
        // A translucent dark pill keeps the line readable over the bed texture.
        painter.rect_filled(pill, size.y / 2., theme::DARK.dark_box.gamma_multiply(0.82));
        painter.galley(pill.min + EVec2::new(10., 4.), galley, color);
    }
    /// Bottom-right card: `38 min · 11.6 g · 3.85 m` and the change since the last slice.
    fn totals_card(&self, ui: &mut egui::Ui, rect: Rect, scene: &Scene<'_>) {
        let p = theme::of(ui);
        let cause = scene
            .totals
            .as_ref()
            .filter(|t| t.delta.is_some())
            .and_then(|t| t.cause.clone());
        let (line1, line2): (String, Option<String>) = if let Some(t) = &scene.totals {
            (
                format!(
                    "{} · {:.1} g · {:.2} m",
                    crate::panels::topbar::short_duration(t.time_s),
                    t.grams,
                    t.filament_mm / 1000.
                ),
                t.delta.as_ref().map(|d| format!("{d} from last change")),
            )
        } else if let Some(progress) = scene.progress {
            (format!("slicing… {:.0} %", progress * 100.), None)
        } else if let Some(reason) = &scene.failed {
            ("slice failed".into(), Some(reason.clone()))
        } else if scene.objects.is_empty() {
            return;
        } else {
            ("not sliced yet".into(), None)
        };
        egui::Area::new(ui.scope_id().with("totals"))
            .order(egui::Order::Middle)
            .fixed_pos(Pos2::new(rect.right() - 14., rect.bottom() - 14.))
            .pivot(Align2::RIGHT_BOTTOM)
            .show(ui.ctx(), |ui| {
                theme::overlay_frame(ui)
                    .inner_margin(egui::Margin::symmetric(12, 8))
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 2.;
                        // Never wrap: the card is pinned to the corner and grows to the left.
                        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                        ui.label(egui::RichText::new(line1).font(theme::mono(12.)).color(
                            if scene.totals.is_some() {
                                p.text
                            } else if scene.failed.is_some() {
                                theme::DANGER
                            } else {
                                p.muted
                            },
                        ));
                        if let Some(l2) = line2 {
                            ui.label(egui::RichText::new(l2).font(theme::mono(12.)).color(
                                if scene.failed.is_some() {
                                    theme::DANGER
                                } else {
                                    p.accent_bright
                                },
                            ));
                        }
                        if let Some(c) = cause {
                            ui.label(egui::RichText::new(c).font(theme::sans(11.)).color(p.muted));
                        }
                    })
                    .response
                    .on_hover_text(
                        "Estimated print time, filament weight and length. The colored line \
                         compares this slice with the one before your last change.",
                    );
            });
    }
    /// Preview's small toolbar (top centre): the section view toggle.
    fn preview_toolbar(&mut self, ui: &mut egui::Ui, rect: Rect) {
        egui::Area::new(ui.scope_id().with("preview toolbar"))
            .order(egui::Order::Middle)
            .fixed_pos(Pos2::new(rect.center().x, rect.top() + 14.))
            .pivot(Align2::CENTER_TOP)
            .show(ui.ctx(), |ui| {
                let m = icons::metrics(ui.ctx());
                theme::overlay_frame(ui)
                    .inner_margin(egui::Margin::same(m.margin as i8))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            if icons::button(ui, Icon::Section, self.section.active(true), true) {
                                self.section.toggle(true, self.camera.direction());
                            }
                        });
                    });
            });
    }
    /// Plate tags (number and name lying on the ground next to each plate, laid out by
    /// `plate_tags` and drawn by the renderer) as handles: a click shows the plate, a
    /// right-click opens its plate menu; plus hover icons for the plate under the pointer.
    /// The hover strip's ⋯ opens the plate bar's menu (`None` in the returned picks).
    #[allow(clippy::type_complexity)]
    fn plate_labels(
        &self,
        ui: &mut egui::Ui,
        _gp: &gizmo::Painter<'_>,
        scene: &Scene<'_>,
        actions: &mut Vec<Action>,
    ) -> (
        Option<(usize, Rect)>,
        Vec<(usize, Option<crate::plate_menu::PlateCommand>, String)>,
    ) {
        let mut picks = vec![];
        if scene.plates.len() < 2 {
            // A single plate needs no tag; the plate bar names it.
            return (None, picks);
        }
        let mut zone = None;
        for plate in scene.plates {
            let tags: Vec<&plate_tags::PlateTag> = self
                .plate_tags
                .iter()
                .filter(|t| t.plate == plate.index)
                .collect();
            // Too small to read (or out of view): no handle either.
            let Some(number) = tags.iter().find(|t| t.kind == plate_tags::TagKind::Number) else {
                continue;
            };
            let full_name = plate
                .name
                .clone()
                .unwrap_or_else(|| crate::plates::default_name(plate.index));
            let info = crate::plate_menu::PlateMenuInfo {
                index: plate.index,
                count: scene.plates.len(),
                name: full_name.clone(),
                locked: plate.locked,
                objects: plate.objects,
                editable: scene.editable,
            };
            let mut tip = format!("Plate {}", plate.index + 1);
            if let Some(name) = &plate.name {
                tip = format!("{tip} · {name}");
            }
            if plate.locked {
                tip.push_str(" · locked");
            }
            tip.push_str("\nClick to show this plate, right-click for the plate menu");
            for tag in &tags {
                // Only the quad itself is a handle (the pointer test is done on the projected
                // quad, objects in front win); elsewhere the viewport keeps the pointer.
                let bounds = tag.bounds();
                let r = if tag.hovered {
                    bounds
                } else {
                    Rect::from_min_size(bounds.min, EVec2::ZERO)
                };
                let label = ui
                    .interact(
                        r,
                        ui.scope_id().with(("plate tag", plate.index, tag.kind)),
                        egui::Sense::click(),
                    )
                    .on_hover_text(&tip);
                if label.clicked() && !plate.current {
                    actions.push(Action::SelectPlate(plate.index));
                }
                label.context_menu(|ui| {
                    if let Some(c) = crate::plate_menu::plate_menu(ui, &info) {
                        picks.push((plate.index, Some(c), full_name.clone()));
                    }
                });
            }
            let r = number.bounds();
            let hovered = self.hovered_plate == Some(plate.index) && scene.preview.is_none();
            if hovered {
                let id = ui.scope_id().with(("plate icons", plate.index));
                let icons = egui::Area::new(id)
                    .order(egui::Order::Middle)
                    .fixed_pos(Pos2::new(r.right() + 6., r.top() - 4.))
                    .show(ui.ctx(), |ui| {
                        theme::overlay_frame(ui)
                            .inner_margin(egui::Margin::same(2))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 0.;
                                    if icons::button_tip(
                                        ui,
                                        Icon::Arrange,
                                        "Arrange this plate",
                                        false,
                                        scene.editable,
                                    ) {
                                        actions.push(Action::PlateArrange(plate.index));
                                    }
                                    if icons::button_tip(
                                        ui,
                                        Icon::Orient,
                                        "Auto-orient every object on this plate",
                                        false,
                                        scene.editable,
                                    ) {
                                        actions.push(Action::PlateOrient(plate.index));
                                    }
                                    let icon = if plate.locked {
                                        Icon::Lock
                                    } else {
                                        Icon::Unlock
                                    };
                                    if icons::button(ui, icon, plate.locked, scene.editable) {
                                        actions.push(Action::PlateLock(plate.index));
                                    }
                                    if icons::button(ui, Icon::Settings, false, true) {
                                        actions.push(Action::Plate(
                                            plate.index,
                                            crate::plate_menu::PlateCommand::Settings,
                                        ));
                                    }
                                    if icons::button_tip(
                                        ui,
                                        Icon::More,
                                        "Plate menu: rename, duplicate, move, delete…",
                                        false,
                                        true,
                                    ) {
                                        picks.push((plate.index, None, full_name.clone()));
                                    }
                                });
                            });
                    });
                // The number, gap and icons (and a hovered name), with a small margin, keep
                // this plate hovered.
                let mut z = r.union(icons.response.rect);
                for t in tags.iter().filter(|t| t.hovered) {
                    z = z.union(t.bounds());
                }
                zone = Some((plate.index, z.expand(8.)));
            } else if let Some(t) = tags.iter().find(|t| t.hovered) {
                // On a tag in the gap the plate counts as hovered from the next frame on.
                zone = Some((plate.index, t.bounds()));
            }
        }
        (zone, picks)
    }
    /// Size read-out under the selection and an explicit warning on objects that cannot print.
    /// Each object is checked against the bed of **its own** plate (`Scene::plate_of`), and a
    /// warning is only drawn while its anchor is in view: a label clamped in from an
    /// off-screen object would appear to belong to whatever happens to be at the edge.
    fn object_labels(&self, ui: &egui::Ui, gp: &gizmo::Painter<'_>, scene: &Scene<'_>) {
        // Skirts and brims that do not fit: their outline on the bed, in red over a dark edge.
        for outline in scene.adhesion_outlines {
            let points: Vec<Pos2> = outline
                .iter()
                .filter_map(|p| gp.project(Vec3::new(p[0], p[1], 0.05)))
                .collect();
            if points.len() == outline.len() && points.len() > 2 {
                gp.painter.add(egui::Shape::closed_line(
                    points.clone(),
                    egui::Stroke::new(3.5, theme::OUTLINE.gamma_multiply(0.6)),
                ));
                gp.painter.add(egui::Shape::closed_line(
                    points,
                    egui::Stroke::new(2., theme::DANGER),
                ));
            }
        }
        for (i, o) in scene.objects.iter().enumerate() {
            let (min, max) = o.bounds();
            let center = (min + max) * 0.5;
            // An object outside the printable volume carries no label: its outside part is
            // striped and outlined in red, and the plate's notification says where and how far.
            let adhesion = scene
                .warned
                .iter()
                .find(|(k, _)| *k == i)
                .map(|(_, t)| t.as_str());
            if let Some(text) = adhesion
                && let Some(p) = gp.project(center.with_z(max.z + 6.))
                && gp.rect.contains(p)
            {
                tooltip(
                    gp.painter,
                    gp.rect,
                    p - EVec2::new(0., 30.),
                    &format!("! {text}"),
                    false,
                );
            }
            if scene.selected == Some(i)
                && !self.measure_on
                && !matches!(self.drag, Drag::Gizmo { .. } | Drag::Object { .. })
                && let Some(p) = gp.project(Vec3::new(center.x, center.y, min.z))
            {
                let size = max - min;
                let pal = theme::of(ui);
                let text = format!("{:.1} × {:.1} × {:.1} mm", size.x, size.y, size.z);
                let galley = gp
                    .painter
                    .layout_no_wrap(text, theme::mono(11.), pal.dark_box_text);
                let r = Rect::from_center_size(
                    p + EVec2::new(0., 22.),
                    galley.size() + EVec2::new(12., 4.),
                );
                gp.painter.rect_filled(r, 4., pal.dark_box);
                gp.painter
                    .galley(r.min + EVec2::new(6., 2.), galley, pal.dark_box_text);
            }
        }
    }
    /// Benchmarks: whether the hover pick has answered the current pointer position, and
    /// whether it found a move there.
    pub fn hover_answer(&self) -> Option<bool> {
        let pick = self.pick.lock().ok()?;
        let (request, result) = (pick.request?, pick.result?);
        (result.request == request).then_some(result.record.is_some())
    }
    /// Whether the preview is moving: the camera or the shown range changed within
    /// `toolpaths::REFINE_DELAY` (then a repaint is asked for when it has been still that long,
    /// so the view refines). The first frame of a view is still.
    fn preview_moving(
        &mut self,
        ctx: &egui::Context,
        aspect: f32,
        range: std::ops::Range<u64>,
    ) -> bool {
        use std::hash::{Hash, Hasher};
        if let Some(m) = self.force_moving {
            return m;
        }
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for v in self.camera.matrix(aspect).to_cols_array() {
            v.to_bits().hash(&mut h);
        }
        range.hash(&mut h);
        let key = h.finish();
        let now = std::time::Instant::now();
        let changed = match self.interaction {
            Some((k, at)) if k == key => at,
            Some(_) => Some(now),
            None => None,
        };
        self.interaction = Some((key, changed));
        let delay = render::toolpaths::REFINE_DELAY;
        match changed.map(|t| now.duration_since(t)) {
            Some(elapsed) if elapsed < delay => {
                ctx.request_repaint_after(delay - elapsed);
                true
            }
            _ => false,
        }
    }
    #[allow(clippy::too_many_arguments)]
    fn preview_overlays(
        &mut self,
        ui: &mut egui::Ui,
        rect: Rect,
        preview: &Arc<Preview>,
        model: glam::Mat4,
        view: &mut View,
        pointer: Option<Pos2>,
        clip: Option<[f32; 4]>,
        layer_gcode: Option<&crate::preview::layer_gcode::LayerGCode<'_>>,
        actions: &mut Vec<Action>,
    ) {
        let p = theme::of(ui);
        let mut changed = false;
        // Toolpaths are in bed coordinates; `model` puts them on the shown plate's bed (as the
        // renderer draws them).
        let world = |w: [f32; 3]| model.transform_point3(Vec3::from(w));
        // A line wholly on the cut-away side of the section plane cannot be hovered.
        let kept = |s: &Segment| {
            clip.is_none_or(|c| {
                let n = Vec3::new(c[0], c[1], c[2]);
                n.dot(world(s.a)) <= c[3] || n.dot(world(s.b)) <= c[3]
            })
        };
        // Layer slider on the right (34 px in from the edge), move slider along the bottom.
        let slider_rect = Rect::from_min_max(
            Pos2::new(rect.right() - 160., rect.top() + 70.),
            Pos2::new(rect.right() - 14., rect.bottom() - 90.),
        );
        let mut edits = Vec::new();
        changed |= slider::layer_slider(
            ui,
            slider_rect,
            rect,
            preview,
            view,
            layer_gcode,
            &mut edits,
        );
        actions.extend(edits.into_iter().map(Action::LayerGCode));
        let move_rect = Rect::from_min_max(
            Pos2::new(rect.left() + 110., rect.bottom() - 58.),
            Pos2::new(rect.right() - 180., rect.bottom() - 30.),
        );
        changed |= slider::move_slider(ui, move_rect, rect, preview, view);
        let gp = gizmo::Painter {
            painter: &ui.painter_at(rect),
            rect,
            camera: &self.camera,
        };
        // Tool head marker at the last visible move.
        if let Some(index) = view.head_index(preview)
            && let Ok(s) = preview.segment_at(index)
            && let Some(q) = gp.project(world(s.b))
        {
            let painter = ui.painter_at(rect);
            painter.circle_stroke(q, 7., egui::Stroke::new(2., p.accent_bright));
            painter.circle_filled(q, 2., p.accent_bright);
        }
        // Hovered move: the renderer answers from its index buffer (every drawn move of the
        // range, the front one where moves overlap) a frame or two later.
        if changed {
            self.hover_segment = None;
            if let Ok(mut pick) = self.pick.lock() {
                pick.result = None;
            }
        }
        if let Some(pt) = pointer
            && rect.contains(pt)
            && ui.rect_contains_pointer(rect)
            && !slider_rect.contains(pt)
            && !move_rect.expand(12.).contains(pt)
        {
            let ppp = ui.ctx().pixels_per_point();
            let request = crate::render::toolpaths::PickRequest {
                px: [
                    ((pt.x - rect.left()) * ppp).max(0.) as u32,
                    ((pt.y - rect.top()) * ppp).max(0.) as u32,
                ],
                radius: (5. * ppp).ceil() as u32,
            };
            let result = self.pick.lock().ok().and_then(|mut p| {
                p.request = Some(request);
                p.result
            });
            if let Some(r) = result
                && self
                    .hover_segment
                    .is_none_or(|(serial, _)| serial != r.serial)
            {
                let range = view.range(preview);
                let found = r.record.and_then(|i| {
                    let s = preview.segment_at(i).ok()?;
                    (range.contains(&i) && view.shows(&s) && kept(&s))
                        .then(|| (i, s, preview.attr_at(i).unwrap_or_default()))
                });
                self.hover_segment = Some((r.serial, found));
            }
            if result.is_none_or(|r| r.request != request) {
                // The answer for this pointer position is on its way.
                ui.ctx().request_repaint();
            }
            if let Some((_, Some((index, s, a)))) = self.hover_segment
                && !self.moving
            {
                let role = s.role() as usize;
                hover_card(
                    &ui.painter_at(rect),
                    rect,
                    pt + EVec2::new(16., 16.),
                    &format!(
                        "{} · layer {}",
                        preview.role_name(role),
                        preview.layer_of(index) + 1
                    ),
                    &format!(
                        "{:.0} mm/s · {:.2} × {:.2} mm · fan {} %",
                        s.speed(),
                        (s.size & 65535) as f32 / 1000.,
                        (s.size >> 16) as f32 / 1000.,
                        (s.meta >> 8) & 255
                    ),
                    &if role as u32 == crate::preview::TRAVEL {
                        format!("Z {:.2} mm · tool {}", s.b[2], s.tool() + 1)
                    } else {
                        format!(
                            "Z {:.2} mm · tool {} · {:.0} °C · {:.1} mm³/s · actual {:.0} mm/s",
                            s.b[2],
                            s.tool() + 1,
                            a.temp(),
                            a.flow(),
                            a.actual()
                        )
                    },
                );
            }
        } else {
            self.hover_segment = None;
            if let Ok(mut pick) = self.pick.lock() {
                pick.request = None;
            }
        }
        // The legend and the totals are in the Preview sidebar (`panels/preview_side.rs`).
        if changed {
            actions.push(Action::ViewChanged);
        }
    }
}
/// The text under the ray: an embossed volume in front of (or on) the surface hit, or a text
/// object hit (`(object, None)`).
fn text_under(scene: &Scene<'_>, origin: Vec3, dir: Vec3) -> Option<(usize, Option<u64>)> {
    let hit = picking::pick_face(scene.objects, origin, dir);
    let nearest = scene
        .objects
        .iter()
        .enumerate()
        .filter_map(|(i, o)| {
            let (id, d) = picking::pick_text_volume(o, origin, dir)?;
            Some((i, id, d))
        })
        .min_by(|a, b| a.2.total_cmp(&b.2));
    if let Some((i, id, d)) = nearest
        && hit.is_none_or(|h| h.object == i || d <= h.distance + 1e-3)
    {
        return Some((i, Some(id)));
    }
    let h = hit?;
    scene.objects[h.object]
        .emboss
        .is_some()
        .then_some((h.object, None))
}

/// What the context menus need, from the scene (the target object when there is one).
fn menu_info(scene: &Scene<'_>, target: Option<usize>) -> MenuInfo {
    let current = scene.plates.iter().position(|p| p.current).unwrap_or(0);
    let object = target.and_then(|i| scene.objects.get(i).map(|o| (i, o)));
    let instances = object.map_or(0, |(i, o)| {
        let plate = scene.plate_of.get(i).copied().unwrap_or(0);
        scene
            .objects
            .iter()
            .enumerate()
            .filter(|(k, p)| {
                p.object_id == o.object_id && scene.plate_of.get(*k).copied().unwrap_or(0) == plate
            })
            .count()
    });
    MenuInfo {
        tools: scene.tool_colors.len().max(1),
        tool_labels: scene.tool_labels.to_vec(),
        tool: object.map_or(0, |(_, o)| o.tools.0[0].unwrap_or(0)),
        plates: scene
            .plates
            .iter()
            .map(|p| p.name.clone().unwrap_or_default())
            .collect(),
        current_plate: current,
        instances,
        can_paste: scene.can_paste,
        selection: match target {
            Some(i) if scene.selection.contains(&i) => scene.selection.len().max(1),
            _ => 1,
        },
        locked: scene.plates.get(current).is_some_and(|p| p.locked),
        move_inside: target.is_some_and(|i| scene.movable.contains(&i)),
        textures: scene.textures,
        text: object.is_some_and(|(i, o)| {
            let part = match scene.sub {
                Some(Sub::Volume(id)) if scene.selected == Some(i) => o.volume(id),
                _ => None,
            };
            part.map_or(o.emboss.is_some(), |v| v.emboss.is_some())
        }),
        sub: scene.sub.map(|s| match s {
            Sub::Volume(_) => "Volume",
            Sub::Range(_) => "Height range",
        }),
    }
}

/// The current plate's tower, if it prints one.
fn current_tower<'a>(scene: &'a Scene<'_>) -> Option<&'a TowerView> {
    let current = scene.plates.iter().position(|p| p.current)?;
    scene.towers.iter().find(|t| t.plate == current)
}

/// Distance along the ray to the current plate's tower (box or footprint), if it is hit.
fn pick_tower(scene: &Scene<'_>, origin: Vec3, dir: Vec3) -> Option<f32> {
    current_tower(scene)?.pick(origin, dir)
}

/// The tower's context menu: no delete; the tower is switched off in the settings instead.
pub(crate) fn tower_menu_ui(ui: &mut egui::Ui, name: &str, edit: bool) -> Option<TowerCommand> {
    ui.set_min_width(220.);
    let mut out = None;
    let lower = name.to_lowercase();
    if ui
        .add_enabled(edit, egui::Button::new("Move to a free spot"))
        .on_hover_text("A corner or edge of the bed clear of every object")
        .clicked()
    {
        out = Some(TowerCommand::FreeSpot);
        ui.close();
    }
    if ui.button(format!("{name} settings…")).clicked() {
        out = Some(TowerCommand::Settings);
        ui.close();
    }
    ui.separator();
    if ui
        .button(format!("Disable {lower}"))
        .on_hover_text("Switches the tower off in the process settings (Multimaterial)")
        .clicked()
    {
        out = Some(TowerCommand::Disable);
        ui.close();
    }
    out
}

/// Small floating text box; `dark_box` draws the dark read-out style (`#0d0f11`, 1 px
/// border), otherwise the warning style. Always kept inside the viewport.
fn tooltip(painter: &egui::Painter, rect: Rect, at: Pos2, text: &str, dark_box: bool) {
    let color = if dark_box {
        theme::DARK.text
    } else {
        Color32::from_rgb(40, 24, 18)
    };
    let galley = painter.layout_no_wrap(text.to_owned(), theme::sans(12.), color);
    let size = galley.size() + EVec2::new(20., 14.);
    let mut r = if dark_box {
        Rect::from_min_size(at, size)
    } else {
        Rect::from_center_size(at, size)
    };
    r = r.translate(clamp_shift(rect, r));
    if dark_box {
        painter.rect_filled(r, 8., theme::DARK.dark_box);
        painter.rect_stroke(
            r,
            8.,
            egui::Stroke::new(1., theme::DARK.border_raised),
            egui::StrokeKind::Inside,
        );
    } else {
        painter.rect_filled(r, 8., theme::WARNING);
    }
    painter.galley(r.min + EVec2::new(10., 7.), galley, color);
}

/// The preview hover card: a bold title, a mono detail line and a muted third line, kept
/// inside the viewport.
fn hover_card(
    painter: &egui::Painter,
    rect: Rect,
    at: Pos2,
    title: &str,
    detail: &str,
    note: &str,
) {
    let d = &theme::DARK;
    let g1 = painter.layout_no_wrap(title.to_owned(), theme::semibold(12.), d.text);
    let g2 = painter.layout_no_wrap(detail.to_owned(), theme::mono(12.), d.label);
    let g3 = painter.layout_no_wrap(note.to_owned(), theme::sans(12.), d.muted);
    let w = [&g1, &g2, &g3]
        .iter()
        .map(|g| g.size().x)
        .fold(0f32, f32::max)
        + 20.;
    let line = g1.size().y + 6.5;
    let mut r = Rect::from_min_size(at, EVec2::new(w, line * 3. + 10.));
    r = r.translate(clamp_shift(rect, r));
    painter.rect_filled(r, 8., d.dark_box);
    painter.rect_stroke(
        r,
        8.,
        egui::Stroke::new(1., d.border_raised),
        egui::StrokeKind::Inside,
    );
    let x = r.left() + 10.;
    painter.galley(Pos2::new(x, r.top() + 7.), g1, d.text);
    painter.galley(Pos2::new(x, r.top() + 7. + line), g2, d.label);
    painter.galley(Pos2::new(x, r.top() + 7. + line * 2.), g3, d.muted);
}

/// The translation that moves `r` inside `bounds` (as far as it fits).
pub fn clamp_shift(bounds: Rect, r: Rect) -> EVec2 {
    EVec2::new(
        (bounds.right() - r.right()).min(0.) + (bounds.left() - r.left()).max(0.),
        (bounds.bottom() - r.bottom()).min(0.) + (bounds.top() - r.top()).max(0.),
    )
}

/// A plate's marker in the plate bar: orange for warnings, red for a failed slice.
fn alert_dot(ui: &mut egui::Ui, alert: u8) {
    let (r, response) = ui.allocate_exact_size(EVec2::new(10., 28.), egui::Sense::hover());
    ui.painter().circle_filled(
        r.center(),
        4.,
        if alert > 1 {
            theme::DANGER
        } else {
            theme::WARNING
        },
    );
    response.on_hover_text(if alert > 1 {
        "This plate's slice failed; see its notification"
    } else {
        "This plate has warnings; see its notifications"
    });
}
