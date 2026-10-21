// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/slic3r/GUI/Gizmos/GLGizmoCut.cpp :: add_connector, delete_selected_connectors, reset_connectors, apply_selected_connectors, render_connectors_input_window, apply_connectors_in_model, can_perform_cut
// Source: prusaslicer@b028299c77 src/libslic3r/CutUtils.cpp :: process_connector_cut, Cut::perform_with_plane
// Changes: Connectors only (the plane, keep and place-on-cut logic, "Place automatically" with its replace prompt, the no-connectors prompt on Cut and the Cut button's label are original Mica code): connectors live in the plane's own X / Y, so they follow the plane when it moves (upstream locks the plane once connectors exist); one selected connector instead of a multi-selection; the cut builds the halves directly: a plug or snap becomes a model part of the lower half's regions model (its mesh merged like split-to-parts) and a negative volume, widened by the tolerances, in the upper half; a dowel's widened hole is cut by the plane into a negative volume per half (shifted 0.05 mm through the cut face, as upstream) and the dowel itself becomes its own object; the half that carries plugs or snaps is never laid on its cut face. Connector edits are not undo steps (the tool's state is not part of the scene).

//! The cut tool: a plane through the selected object that the Move gizmo drags along Z (and
//! X / Y for tilted planes), tilted with the card's angle fields, and "Cut" splits the mesh
//! along it with `mica_core::mesh_cut` (watertight halves with capped faces). The upper,
//! lower or both halves are kept; "Place on cut face" lays each kept half on its cut.
//! "Add connectors" places plugs, dowels and snaps on the plane with clicks in the view
//! (`mica_core::cut_connectors` has their geometry); the cut then turns them into parts,
//! holes and dowel objects. "Place automatically" spreads them over the cut face; Cut with
//! none placed after "Add connectors" asks first instead of silently cutting plain.

use crate::{
    app::App,
    scene::{Delta, Object, Sub, Volume},
};
use glam::{Mat4, Quat, Vec2, Vec3};
use mica_core::{
    cut_connectors::{self as cc, Attributes, Conflict, ConnectorType, Sizes},
    threemf::PartKind,
};
use std::sync::Arc;

/// A connector on the cut plane.
#[derive(Clone, Debug, PartialEq)]
pub struct Connector {
    /// Position in the plane's own X / Y, mm from `CutState::center`.
    pub at: Vec2,
    pub attributes: Attributes,
    pub sizes: Sizes,
    /// Placed by "Place automatically" (which may replace these, never hand-placed ones).
    pub auto: bool,
}

/// A question the card asks inline before it acts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CutPrompt {
    /// Cut was pressed after "Add connectors" with none placed: place some or cut anyway.
    NoConnectors,
    /// "Place automatically" again while some are: replace those or add more.
    ReplaceAuto,
}

/// What "Place automatically" did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AutoPlaced {
    /// Connectors from an earlier run exist: the card asks first ([`CutPrompt::ReplaceAuto`]).
    Ask,
    Done {
        added: usize,
        /// Parts of the cut face with no room for one.
        too_small: usize,
        /// Spots dropped because the connector would conflict (reach out of the object).
        dropped: usize,
    },
}

/// Connector edits from the view and the card.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ConnectorEdit {
    /// A click on the plane (world point): selects the connector there, or adds one.
    Click(Vec3),
    DeleteSelected,
    Clear,
    /// Leaves "Add connectors" mode (Esc, Done).
    Finish,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Keep {
    Upper,
    Lower,
    #[default]
    Both,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CutState {
    /// The object being cut (its index on the current plate).
    pub object: usize,
    /// That object's `Object::id`: deleting or reordering objects can move another object to
    /// `object`, and the tool must never cut that one instead (see `App::cut_target`).
    pub object_id: u64,
    /// A point on the plane, world coordinates.
    pub center: Vec3,
    /// Tilt about X and Y in radians (0, 0 = horizontal).
    pub tilt_x: f32,
    pub tilt_y: f32,
    pub keep: Keep,
    pub place_on_cut: bool,
    /// Plugs, dowels and snaps between the halves.
    pub connectors: Vec<Connector>,
    /// "Add connectors" mode: clicks on the plane place connectors, the plane stays put.
    pub editing: bool,
    /// The connector the card edits (and Delete removes).
    pub selected_connector: Option<usize>,
    /// Type, shape and sizes for the next connector (and the selected one).
    pub connector: (Attributes, Sizes),
    /// Each connector's conflict, refreshed by [`CutState::check`].
    pub conflicts: Vec<Option<Conflict>>,
    /// What `conflicts` was computed for.
    checked: Option<String>,
    /// Half-size of the drawn plane quad (mm), from the object's footprint.
    pub extent: f32,
    /// "Add connectors" was opened or a connector type chosen: Cut without any asks first.
    pub wants_connectors: bool,
    /// The card's open inline question, if any.
    pub prompt: Option<CutPrompt>,
    /// The last "Place automatically" result for the card (text, whether it is a warning).
    pub note: Option<(String, bool)>,
}

impl CutState {
    /// A horizontal plane through the middle of the object.
    pub fn for_object(index: usize, o: &Object) -> Self {
        let (min, max) = o.bounds();
        let size = max - min;
        Self {
            object: index,
            object_id: o.id,
            center: (min + max) * 0.5,
            tilt_x: 0.,
            tilt_y: 0.,
            keep: Keep::Both,
            place_on_cut: true,
            connectors: vec![],
            editing: false,
            selected_connector: None,
            connector: Default::default(),
            conflicts: vec![],
            checked: None,
            extent: size.truncate().length() * 0.5 + 5.,
            wants_connectors: false,
            prompt: None,
            note: None,
        }
    }
    /// The plane's own X and Y axes in the world.
    pub fn axes(&self) -> (Vec3, Vec3) {
        let r = self.rotation();
        (r * Vec3::X, r * Vec3::Y)
    }
    /// A connector's point on the plane, world coordinates.
    pub fn connector_point(&self, c: &Connector) -> Vec3 {
        let (x, y) = self.axes();
        self.center + x * c.at.x + y * c.at.y
    }
    /// A world point in the plane's X / Y (projected onto the plane).
    pub fn plane_coords(&self, p: Vec3) -> Vec2 {
        let (x, y) = self.axes();
        let d = p - self.center;
        Vec2::new(d.dot(x), d.dot(y))
    }
    /// Where a view ray meets the plane.
    pub fn ray_point(&self, origin: Vec3, dir: Vec3) -> Option<Vec3> {
        let n = self.normal();
        let denom = dir.dot(n);
        if denom.abs() < 1e-6 {
            return None;
        }
        let t = (self.center - origin).dot(n) / denom;
        (t > 0.).then(|| origin + dir * t)
    }
    /// The world transform of a unit connector mesh placed with `fit` (upstream: translation
    /// · cut rotation · rotation about Z by −angle · scale).
    pub fn connector_transform(&self, c: &Connector, fit: cc::Fit) -> Mat4 {
        let (s, r, t) = self.connector_srt(c, fit);
        Mat4::from_scale_rotation_translation(s, r, t)
    }
    fn connector_srt(&self, c: &Connector, fit: cc::Fit) -> (Vec3, Quat, Vec3) {
        (
            Vec3::from(fit.scale),
            self.rotation() * Quat::from_rotation_z(-c.sizes.angle),
            self.connector_point(c) + self.normal() * fit.offset,
        )
    }
    /// The connector under a plane point: upstream picks by the rendered shape; here a
    /// point within a connector's radius of its centre (the nearest one).
    pub fn connector_at(&self, p: Vec3) -> Option<usize> {
        let at = self.plane_coords(p);
        self.connectors
            .iter()
            .enumerate()
            .filter(|(_, c)| c.at.distance(at) <= c.sizes.radius.max(0.5))
            .min_by(|a, b| a.1.at.distance(at).total_cmp(&b.1.at.distance(at)))
            .map(|(i, _)| i)
    }
    /// Applies a connector edit (the view's clicks, Delete, Clear).
    pub fn edit_connectors(&mut self, edit: ConnectorEdit) {
        if edit != ConnectorEdit::Finish {
            self.note = None;
        }
        match edit {
            ConnectorEdit::Click(p) => {
                let at = self.plane_coords(p);
                match self.connector_at(p) {
                    Some(i) => self.selected_connector = Some(i),
                    None => {
                        let (attributes, sizes) = self.connector;
                        self.connectors.push(Connector {
                            at,
                            attributes,
                            sizes,
                            auto: false,
                        });
                        self.selected_connector = Some(self.connectors.len() - 1);
                    }
                }
            }
            ConnectorEdit::DeleteSelected => {
                if let Some(i) = self.selected_connector.take()
                    && i < self.connectors.len()
                {
                    self.connectors.remove(i);
                }
            }
            ConnectorEdit::Clear => {
                self.connectors.clear();
                self.selected_connector = None;
                self.prompt = None;
            }
            ConnectorEdit::Finish => {
                self.editing = false;
                self.selected_connector = None;
            }
        }
    }
    /// Sets the type, shape and sizes for new connectors and the selected one
    /// (`apply_selected_connectors`).
    pub fn set_connector(&mut self, attributes: Attributes, sizes: Sizes) {
        self.connector = (attributes, sizes);
        if let Some(c) = self
            .selected_connector
            .and_then(|i| self.connectors.get_mut(i))
        {
            c.attributes = attributes;
            c.sizes = sizes;
        }
    }
    /// "Place automatically": connectors of the current type and size spread over the cut
    /// face of `o` (`cut_connectors::Section::auto_place`), each clear of the outline and of
    /// every connector already there. Earlier automatic ones are replaced only when
    /// `replace` is `Some(true)`; while some exist and there is no answer yet (`None`),
    /// nothing changes and the card asks ([`AutoPlaced::Ask`]). Hand-placed connectors
    /// always stay. A spot whose connector would still conflict (it reaches out of the
    /// object near its top or bottom) is left out.
    pub fn place_automatically(&mut self, o: &Object, replace: Option<bool>) -> AutoPlaced {
        if replace.is_none() && self.connectors.iter().any(|c| c.auto) {
            self.prompt = Some(CutPrompt::ReplaceAuto);
            return AutoPlaced::Ask;
        }
        self.prompt = None;
        self.wants_connectors = true;
        if replace == Some(true) {
            self.connectors.retain(|c| !c.auto);
            self.selected_connector = None;
        }
        let (x, y) = self.axes();
        let axes = [x.to_array(), y.to_array(), self.normal().to_array()];
        let section = cc::Section::new(&o.placed_mesh(), self.center.to_array(), axes);
        let existing: Vec<([f32; 2], f32)> = self
            .connectors
            .iter()
            .map(|c| (c.at.to_array(), c.sizes.radius))
            .collect();
        let (attributes, sizes) = self.connector;
        let found = section.auto_place(sizes.radius, &existing);
        let before = self.connectors.len();
        self.connectors
            .extend(found.points.iter().map(|&p| Connector {
                at: Vec2::from(p),
                attributes,
                sizes,
                auto: true,
            }));
        // The contour and the spacing hold by construction; the object's box may not.
        let conflicts = connector_conflicts(o, self);
        let mut k = 0;
        self.connectors.retain(|_| {
            k += 1;
            k <= before || conflicts.get(k - 1).copied().flatten().is_none()
        });
        let added = self.connectors.len() - before;
        if added > 0 {
            self.selected_connector = None;
        }
        AutoPlaced::Done {
            added,
            too_small: found.too_small,
            dropped: found.points.len() - added,
        }
    }
    /// Refreshes `conflicts` for the object being cut when the plane or a connector changed.
    pub fn check(&mut self, o: &Object) {
        let key = format!(
            "{} {:?} {} {} {:?}",
            o.id, self.center, self.tilt_x, self.tilt_y, self.connectors
        );
        if self.checked.as_deref() == Some(key.as_str()) {
            return;
        }
        self.conflicts = connector_conflicts(o, self);
        self.checked = Some(key);
    }
    /// The first conflict, for the card and the Cut button.
    pub fn conflict(&self) -> Option<Conflict> {
        self.conflicts.iter().flatten().next().copied()
    }
    pub fn rotation(&self) -> Quat {
        Quat::from_rotation_y(self.tilt_y) * Quat::from_rotation_x(self.tilt_x)
    }
    /// Unit normal; "upper" is the side it points to.
    pub fn normal(&self) -> Vec3 {
        (self.rotation() * Vec3::Z).normalize()
    }
    /// Height of the plane at its centre, for the card.
    pub fn z(&self) -> f32 {
        self.center.z
    }
    pub fn set_z(&mut self, z: f32) {
        self.center.z = z;
    }
    /// Gizmo deltas: translations move the plane, rotations tilt it; scale is ignored.
    pub fn apply(&mut self, delta: Delta) {
        match delta {
            Delta::Translate(d) => self.center += d,
            Delta::Rotate { axis, angle } => {
                let (x, y, z) = (self.rotation() * Quat::from_axis_angle(axis, angle))
                    .to_euler(glam::EulerRot::ZXY);
                let _ = z;
                self.tilt_x = x;
                self.tilt_y = y;
            }
            Delta::Scale(_) => {}
        }
    }
    /// World transform of a unit quad (the drawn plane): 2 × extent square, 0.2 mm thin.
    pub fn quad_transform(&self) -> glam::Mat4 {
        glam::Mat4::from_scale_rotation_translation(
            Vec3::new(self.extent * 2., self.extent * 2., 0.2),
            self.rotation(),
            self.center,
        )
    }
}

/// Each connector's conflict on `o` (`is_conflict_for_connector`): outside the cut face,
/// out of the object's box, or overlapping another.
pub fn connector_conflicts(o: &Object, cut: &CutState) -> Vec<Option<Conflict>> {
    if cut.connectors.is_empty() {
        return vec![];
    }
    let (x, y) = cut.axes();
    let axes = [x.to_array(), y.to_array(), cut.normal().to_array()];
    let section = cc::Section::new(&o.placed_mesh(), cut.center.to_array(), axes);
    let placed: Vec<cc::Placed> = cut
        .connectors
        .iter()
        .map(|c| cc::Placed {
            point: cut.connector_point(c).to_array(),
            attributes: c.attributes,
            sizes: c.sizes,
        })
        .collect();
    let (lo, hi) = o.bounds();
    cc::conflicts(&placed, axes, (lo.to_array(), hi.to_array()), |p| {
        let q = cut.plane_coords(Vec3::from(p));
        section.contains(q.x, q.y)
    })
}

/// "dowel" / "dowels" and so on.
pub fn kind_noun(kind: ConnectorType, n: usize) -> &'static str {
    match (kind, n == 1) {
        (ConnectorType::Plug, true) => "plug",
        (ConnectorType::Plug, false) => "plugs",
        (ConnectorType::Dowel, true) => "dowel",
        (ConnectorType::Dowel, false) => "dowels",
        (ConnectorType::Snap, true) => "snap",
        (ConnectorType::Snap, false) => "snaps",
    }
}

/// The Cut button's label, naming what the cut makes: "Cut", "Cut with 3 dowels", "Cut with
/// 1 plug", or "Cut with 4 connectors" for mixed types.
pub fn cut_label(connectors: &[Connector]) -> String {
    let Some(first) = connectors.first() else {
        return "Cut".into();
    };
    let n = connectors.len();
    let kind = first.attributes.kind;
    if connectors.iter().all(|c| c.attributes.kind == kind) {
        format!("Cut with {n} {}", kind_noun(kind, n))
    } else {
        format!("Cut with {n} connectors")
    }
}

/// The card's and the status line's words for a "Place automatically" result, and whether
/// they are a warning (nothing placed, or parts of the face left without).
pub fn auto_message(placed: AutoPlaced, kind: ConnectorType, sizes: &Sizes) -> (String, bool) {
    let AutoPlaced::Done {
        added,
        too_small,
        dropped,
    } = placed
    else {
        return (String::new(), false);
    };
    let one = kind_noun(kind, 1);
    let across = format!("{:.1} mm", 2. * sizes.radius);
    if added == 0 {
        let text = if dropped > 0 {
            format!("No {one} fits: it would reach out of the object. Try a shallower depth")
        } else if too_small > 0 {
            let needs = 2. * (sizes.radius + cc::AUTO_MARGIN);
            format!(
                "No room for a {across} {one}: the cut face needs {needs:.1} mm across. Try a smaller size"
            )
        } else {
            "The cut plane does not cross the object".to_string()
        };
        return (text, true);
    }
    let mut text = format!("Placed {added} {}", kind_noun(kind, added));
    if too_small == 1 {
        text += &format!(" · 1 part of the cut face is too small for a {across} {one}");
    } else if too_small > 1 {
        text += &format!(" · {too_small} parts of the cut face are too small for a {across} {one}");
    }
    if dropped > 0 {
        text += &format!(" · {dropped} left out (would reach out of the object)");
    }
    (text, too_small > 0 || dropped > 0)
}

/// Whether a plane point lies on the cut face of `o` (`unproject_on_cut_plane`): new
/// connectors are only placed there.
pub fn on_cut_face(o: &Object, cut: &CutState, p: Vec3) -> bool {
    let (x, y) = cut.axes();
    let axes = [x.to_array(), y.to_array(), cut.normal().to_array()];
    let q = cut.plane_coords(p);
    cc::Section::new(&o.placed_mesh(), cut.center.to_array(), axes).contains(q.x, q.y)
}

/// The unit body mesh of a connector kind for drawing, with a GPU mesh key (one per kind,
/// made once).
pub fn preview_mesh(a: Attributes) -> (u64, Arc<mica_core::mesh::TriangleMesh>) {
    static CACHE: std::sync::Mutex<Vec<(Attributes, u64, Arc<mica_core::mesh::TriangleMesh>)>> =
        std::sync::Mutex::new(Vec::new());
    let a = a.effective();
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, id, m)) = cache.iter().find(|(k, _, _)| *k == a) {
        return (*id, m.clone());
    }
    // A volume's id comes from the scene's id counter, so the key never meets an object's.
    let v = Volume::new("connector", PartKind::Model, cc::unit_mesh(a));
    cache.push((a, v.id, v.mesh.clone()));
    (v.id, v.mesh)
}

/// Connector colours (RGBA) by kind and state: plugs and snaps amber, dowels blue, conflicts
/// red (upstream's PLAG / DOWEL / CONNECTOR_ERR colours in Mica's palette).
pub fn connector_color(kind: ConnectorType, conflict: bool, selected: bool) -> [f32; 4] {
    let a = if selected { 0.9 } else { 0.7 };
    if conflict {
        return [0.92, 0.26, 0.22, a];
    }
    match kind {
        ConnectorType::Dowel => [0.3, 0.6, 0.98, a],
        ConnectorType::Plug | ConnectorType::Snap => [0.98, 0.74, 0.2, a],
    }
}

/// A unit connector mesh placed in the world.
fn placed_connector(
    cut: &CutState,
    c: &Connector,
    mesh: &mica_core::mesh::TriangleMesh,
    fit: cc::Fit,
) -> mica_core::mesh::TriangleMesh {
    crate::scene::transform_mesh(mesh, cut.connector_transform(c, fit))
}

/// A negative volume in a cut half's frame (which is the world at the cut).
fn hole_volume(
    name: String,
    mesh: mica_core::mesh::TriangleMesh,
    srt: Option<(Vec3, Quat, Vec3)>,
) -> Volume {
    let mut v = Volume::new(name, PartKind::NegativeVolume, mesh);
    if let Some((s, r, t)) = srt {
        v.scale = s;
        v.rotation = r;
        v.position = t;
    }
    v
}

/// Splits `o` along the plane into new objects (upper first, then lower, then any dowels;
/// in world placement, dropped to the bed); `None` when the plane misses the mesh. Painting
/// is not carried over (face indices change); settings, tools, overrides and ranges are
/// copied; volumes are dropped. Connectors (`process_connector_cut`): a plug or snap is a
/// model part of the lower half and a widened hole in the upper half; a dowel is a hole in
/// both halves and an object of its own. With connectors both halves are kept.
pub fn cut_object(o: &Object, cut: &CutState) -> Option<Vec<Object>> {
    let placed = o.placed_mesh();
    let plane = mica_core::mesh_cut::CutPlane {
        point: cut.center.to_array(),
        normal: cut.normal().to_array(),
    };
    let result = mica_core::mesh_cut::cut_mesh(&placed, &plane, true);
    let (upper, lower) = (result.upper, result.lower);
    if upper.is_none() || lower.is_none() {
        return None;
    }
    let stem = std::path::Path::new(&o.name)
        .file_stem()
        .map_or(o.name.clone(), |s| s.to_string_lossy().into_owned());
    let normal = cut.normal();
    // Connector geometry, world coordinates.
    let mut upper_holes = vec![];
    let mut lower_holes = vec![];
    let mut plugs = vec![];
    let mut dowels = vec![];
    for (k, c) in cut.connectors.iter().enumerate() {
        let a = c.attributes.effective();
        let name = format!("Connector-{}", k + 1);
        let hole = cc::hole(a, &c.sizes);
        match a.kind {
            ConnectorType::Plug | ConnectorType::Snap => {
                plugs.push((
                    name.clone(),
                    placed_connector(cut, c, &cc::unit_mesh(a), cc::body(a, &c.sizes)),
                ));
                upper_holes.push(hole_volume(
                    name,
                    cc::unit_hole_mesh(a),
                    Some(cut.connector_srt(c, hole)),
                ));
            }
            ConnectorType::Dowel => {
                let widened = placed_connector(cut, c, &cc::unit_hole_mesh(a), hole);
                let halves = mica_core::mesh_cut::cut_mesh(&widened, &plane, true);
                let shift = |m: mica_core::mesh::TriangleMesh, d: f32| {
                    crate::scene::transform_mesh(&m, Mat4::from_translation(normal * d))
                };
                if let Some(m) = halves.upper {
                    upper_holes.push(hole_volume(
                        format!("{name}_A"),
                        shift(m, -cc::PREVIEW_SHIFT),
                        None,
                    ));
                }
                if let Some(m) = halves.lower {
                    lower_holes.push(hole_volume(
                        format!("{name}_B"),
                        shift(m, cc::PREVIEW_SHIFT),
                        None,
                    ));
                }
                // The pin itself, upright and unturned (upstream discards rotation and offset).
                let fit = cc::body(a, &c.sizes);
                let pin = crate::scene::transform_mesh(
                    &cc::unit_mesh(a),
                    Mat4::from_scale(Vec3::from(fit.scale)),
                );
                let mut d =
                    Object::from_mesh(format!("{stem} (dowel {})", dowels.len() + 1), pin, o);
                d.height_ranges = vec![];
                d.drop_to_bed();
                dowels.push(d);
            }
        }
    }
    let has_connectors = !cut.connectors.is_empty();
    let mut out = vec![];
    for (mesh, label, down) in [(upper, "upper", -normal), (lower, "lower", normal)] {
        let keep = has_connectors
            || match cut.keep {
                Keep::Both => true,
                Keep::Upper => label == "upper",
                Keep::Lower => label == "lower",
            };
        if !keep {
            continue;
        }
        let Some(mesh) = mesh else { continue };
        let name = format!("{stem} ({label})");
        let is_lower = label == "lower";
        let mut n = if is_lower && !plugs.is_empty() {
            // The half and its plugs as model parts of one object (as split-to-parts).
            let mut volumes = vec![mica_core::regions::Volume {
                mesh: mesh.clone(),
                kind: PartKind::Model,
                config: mica_core::config::Config::new(),
                painting: Default::default(),
            }];
            let mut parts = vec![(name.clone(), "Model".to_string())];
            for (pname, m) in &plugs {
                volumes.push(mica_core::regions::Volume {
                    mesh: m.clone(),
                    kind: PartKind::Model,
                    config: mica_core::config::Config::new(),
                    painting: Default::default(),
                });
                parts.push((pname.clone(), "Model".to_string()));
            }
            let merged =
                mica_core::mesh_edit::merge(&volumes.iter().map(|v| &v.mesh).collect::<Vec<_>>());
            let mut n = Object::from_mesh(name, merged, o);
            n.parts = parts;
            n.region = Some(Arc::new(mica_core::regions::RegionModel {
                volumes,
                height_ranges: vec![],
                ..Default::default()
            }));
            n
        } else {
            Object::from_mesh(name, mesh, o)
        };
        n.height_ranges = vec![];
        n.volumes = if is_lower {
            lower_holes.clone()
        } else {
            upper_holes.clone()
        };
        // The half with plugs or snaps standing on its cut face is never laid on it.
        if cut.place_on_cut && !(is_lower && !plugs.is_empty()) {
            // The cut face's outward normal is `down`; lay it on the bed.
            n.lay_on_face(down);
        }
        n.drop_to_bed();
        out.push(n);
    }
    out.extend(dowels);
    Some(out)
}

impl App {
    /// Opens the cut tool on the primary object, or closes it.
    pub(crate) fn toggle_cut(&mut self) {
        if self.cut.take().is_some() {
            self.status = "Cut canceled".into();
            return;
        }
        let Some(i) = self.selected else {
            self.status = "Select an object to cut".into();
            return;
        };
        if self.plate_locked() {
            return;
        }
        let Some(o) = self.plates.current().objects.get(i) else {
            return;
        };
        self.sub = None;
        self.viewport.paint = None;
        self.viewport.lay_on_face = false;
        self.viewport.mode = crate::viewport::gizmo::Mode::Move;
        self.cut = Some(CutState::for_object(i, o));
        self.status =
            "Cut: drag the plane along Z (arrows), tilt it in the card, then press Cut".into();
    }

    /// The index of the object the open cut tool cuts: still on the current plate, still the
    /// same object, still the selected one. `None` when the tool is closed or stale.
    pub(crate) fn cut_target(&self) -> Option<usize> {
        let c = self.cut.as_ref()?;
        let o = self.plates.current().objects.get(c.object)?;
        (o.id == c.object_id && self.selected == Some(c.object)).then_some(c.object)
    }

    /// Closes the cut tool when its object went away (deleted, undone, another plate).
    pub(crate) fn validate_cut(&mut self) {
        if self.cut.is_some() && self.cut_target().is_none() {
            self.cut = None;
        }
    }

    /// Cut, unless "Add connectors" was opened (or a type chosen) and none are placed: then
    /// the card asks first ([`CutPrompt::NoConnectors`]: place automatically or cut anyway).
    pub(crate) fn apply_cut(&mut self) {
        if let Some(c) = &mut self.cut
            && c.wants_connectors
            && c.connectors.is_empty()
        {
            c.prompt = Some(CutPrompt::NoConnectors);
            self.status = "No connectors placed: the halves won't join".into();
            return;
        }
        self.cut_anyway();
    }

    /// "Place automatically" from the card; `replace` answers whether earlier automatic
    /// connectors go (`None`: not asked yet).
    pub(crate) fn auto_place_connectors(&mut self, replace: Option<bool>) {
        let Some(o) = self
            .cut_target()
            .and_then(|i| self.plates.current().objects.get(i))
            .cloned()
        else {
            return;
        };
        let Some(c) = &mut self.cut else { return };
        let placed = c.place_automatically(&o, replace);
        if placed == AutoPlaced::Ask {
            self.status = "Replace the connectors placed automatically, or add more?".into();
            return;
        }
        let (attributes, sizes) = c.connector;
        let note = auto_message(placed, attributes.kind, &sizes);
        self.status = note.0.clone();
        c.note = Some(note);
    }

    /// Performs the cut as it stands: one undo entry, the halves selected.
    pub(crate) fn cut_anyway(&mut self) {
        let Some(cut) = self.cut.clone() else { return };
        if self.plate_locked() {
            return;
        }
        let Some(o) = self
            .cut_target()
            .and_then(|i| self.plates.current().objects.get(i))
            .cloned()
        else {
            self.cut = None;
            return;
        };
        if let Some(conflict) = connector_conflicts(&o, &cut).into_iter().flatten().next() {
            // Upstream's `can_perform_cut`: no cut while a connector is invalid.
            self.notify_warning(format!("{}: move or resize it first", conflict.message()));
            return;
        }
        let Some(halves) = cut_object(&o, &cut) else {
            self.notify_warning("The cut plane does not cross the object");
            return;
        };
        self.record("Cut");
        self.plates.current_mut().objects.remove(cut.object);
        // Numbered after the remaining objects (the cut one's id is free unless an instance
        // still uses it), so a saved and reopened project numbers them the same.
        let next = self
            .plates
            .plates
            .iter()
            .flat_map(|p| p.objects.iter().map(|o| o.object_id))
            .max()
            .map_or(0, |n| n + 1);
        let mut made = vec![];
        let bed = self.bed.clone();
        for (k, mut h) in halves.into_iter().enumerate() {
            h.object_id = next + k;
            if k > 0 {
                // The second half beside the first, not on top of it.
                let (min, max) = h.bounds();
                let size = (max - min).truncate();
                let spot = crate::scene::free_spot(
                    &self.plates.current().objects,
                    size,
                    min.truncate(),
                    &bed,
                    crate::commands::SPACING,
                );
                crate::scene::move_footprint_to(&mut h, spot);
            }
            let plate = self.plates.current_mut();
            plate.objects.push(h);
            made.push(plate.objects.len() - 1);
        }
        self.cut = None;
        self.sub = None;
        self.select_many(&made, false);
        self.changed();
        self.status = format!(
            "Cut into {} part{}",
            made.len(),
            if made.len() == 1 { "" } else { "s" }
        );
        let _ = Sub::Volume(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mica_core::{
        config::Config, input::InputObject, mesh::TriangleMesh, print_job::ToolAssignments,
    };

    fn cube() -> Object {
        let mesh = TriangleMesh {
            vertices: vec![
                [0., 0., 0.],
                [20., 0., 0.],
                [20., 20., 0.],
                [0., 20., 0.],
                [0., 0., 20.],
                [20., 0., 20.],
                [20., 20., 20.],
                [0., 20., 20.],
            ],
            faces: vec![
                [0, 2, 1],
                [0, 3, 2],
                [4, 5, 6],
                [4, 6, 7],
                [0, 1, 5],
                [0, 5, 4],
                [1, 2, 6],
                [1, 6, 5],
                [2, 3, 7],
                [2, 7, 6],
                [3, 0, 4],
                [3, 4, 7],
            ],
        };
        Object::new(InputObject::plain(
            "cube",
            0,
            0,
            mesh,
            ToolAssignments([Some(0); 5]),
            Config::defaults(),
        ))
    }

    #[test]
    fn plane_state_moves_and_tilts() {
        let o = cube();
        let mut c = CutState::for_object(0, &o);
        assert_eq!(c.z(), 10.);
        assert_eq!(c.normal(), Vec3::Z);
        c.apply(Delta::Translate(Vec3::new(0., 0., 3.)));
        assert_eq!(c.z(), 13.);
        c.tilt_x = 30f32.to_radians();
        let n = c.normal();
        assert!((n.length() - 1.).abs() < 1e-5 && n.z < 1. && n.y.abs() > 0.1);
    }

    #[test]
    fn cutting_a_cube_gives_two_resting_halves_with_the_right_volumes() {
        let mut o = cube();
        o.position = Vec3::new(50., 50., 0.);
        let mut c = CutState::for_object(0, &o);
        c.set_z(7.);
        c.place_on_cut = false;
        let halves = cut_object(&o, &c).unwrap();
        assert_eq!(halves.len(), 2);
        assert!(halves[0].name.ends_with("(upper)"));
        let upper_volume = halves[0].placed_mesh().volume();
        let lower_volume = halves[1].placed_mesh().volume();
        assert!(
            (upper_volume - 20. * 20. * 13.).abs() < 1e-2,
            "{upper_volume}"
        );
        assert!(
            (lower_volume - 20. * 20. * 7.).abs() < 1e-2,
            "{lower_volume}"
        );
        for h in &halves {
            assert!(h.lowest_z().abs() < 1e-4);
            assert!(mica_core::mesh_cut::is_watertight(&h.placed_mesh()));
        }
        // Keep only the lower half, placed on its cut face: it is 7 mm tall and flat.
        c.keep = Keep::Lower;
        c.place_on_cut = true;
        let lower = cut_object(&o, &c).unwrap();
        assert_eq!(lower.len(), 1);
        let (min, max) = lower[0].bounds();
        assert!((max.z - min.z - 7.).abs() < 1e-3);
        // A plane above the cube misses it.
        c.set_z(40.);
        assert!(cut_object(&o, &c).is_none());
    }

    fn sizes(radius: f32, height: f32) -> Sizes {
        Sizes {
            radius,
            height,
            radius_tolerance: 0.1,
            height_tolerance: 0.2,
            angle: 0.,
        }
    }

    #[test]
    fn connectors_are_placed_picked_edited_and_removed() {
        let mut o = cube();
        o.position = Vec3::new(50., 50., 0.);
        let mut c = CutState::for_object(0, &o);
        c.editing = true;
        c.set_connector(Attributes::default(), sizes(2., 4.));
        // Two clicks on the face place two connectors; a click on one selects it.
        c.edit_connectors(ConnectorEdit::Click(Vec3::new(55., 60., 10.)));
        c.edit_connectors(ConnectorEdit::Click(Vec3::new(65., 60., 10.)));
        assert_eq!(c.connectors.len(), 2);
        assert_eq!(c.selected_connector, Some(1));
        c.edit_connectors(ConnectorEdit::Click(Vec3::new(55.5, 60.5, 10.)));
        assert_eq!(c.connectors.len(), 2);
        assert_eq!(c.selected_connector, Some(0));
        assert!((c.connector_point(&c.connectors[0]) - Vec3::new(55., 60., 10.)).length() < 1e-4);
        // Settings apply to the selected connector only (and to the next one).
        let dowel = Attributes {
            kind: ConnectorType::Dowel,
            ..Attributes::default()
        };
        c.set_connector(dowel, sizes(1.5, 3.));
        assert_eq!(c.connectors[0].attributes.kind, ConnectorType::Dowel);
        assert_eq!(c.connectors[1].attributes.kind, ConnectorType::Plug);
        // Both lie on the cut face; one moved off it conflicts, one onto the other overlaps.
        c.check(&o);
        assert_eq!(c.conflicts, vec![None, None]);
        assert!(on_cut_face(&o, &c, Vec3::new(55., 60., 10.)));
        assert!(!on_cut_face(&o, &c, Vec3::new(75., 60., 10.)));
        c.connectors[1].at = Vec2::new(9., 0.);
        c.check(&o);
        assert_eq!(c.conflict(), Some(Conflict::OutsideContour));
        c.connectors[1].at = c.connectors[0].at + Vec2::new(1., 0.);
        c.check(&o);
        assert_eq!(c.conflict(), Some(Conflict::Overlap));
        // Connectors follow the plane.
        c.set_z(12.);
        assert!((c.connector_point(&c.connectors[0]).z - 12.).abs() < 1e-5);
        c.edit_connectors(ConnectorEdit::DeleteSelected);
        assert_eq!(c.connectors.len(), 1);
        assert_eq!(c.selected_connector, None);
        c.edit_connectors(ConnectorEdit::Clear);
        assert!(c.connectors.is_empty());
        c.edit_connectors(ConnectorEdit::Finish);
        assert!(!c.editing);
        // A view ray from above meets the plane at its height.
        let at = c.ray_point(Vec3::new(55., 60., 100.), Vec3::NEG_Z).unwrap();
        assert!((at - Vec3::new(55., 60., 12.)).length() < 1e-4);
    }

    /// A plug: a model part of the lower half standing on the cut face, a widened hole in
    /// the upper half; a dowel: a hole in each half and an object of its own. Volumes add up.
    #[test]
    fn cutting_with_plug_and_dowel_connectors() {
        let mut o = cube();
        o.position = Vec3::new(50., 50., 0.);
        let mut c = CutState::for_object(0, &o);
        c.place_on_cut = false;
        c.keep = Keep::Upper; // connectors keep both halves anyway
        let s = sizes(2., 4.);
        let dowel = Attributes {
            kind: ConnectorType::Dowel,
            ..Attributes::default()
        };
        c.connectors = vec![
            Connector {
                at: Vec2::new(-5., 0.),
                attributes: Attributes::default(),
                sizes: s,
                auto: false,
            },
            Connector {
                at: Vec2::new(5., 0.),
                attributes: dowel,
                sizes: s,
                auto: false,
            },
        ];
        let parts = cut_object(&o, &c).unwrap();
        assert_eq!(parts.len(), 3);
        let (upper, lower, pin) = (&parts[0], &parts[1], &parts[2]);
        assert!(upper.name.ends_with("(upper)") && lower.name.ends_with("(lower)"));
        assert!(pin.name.ends_with("(dowel 1)"));
        // The upper half: the plug's hole and the dowel's upper hole, negative volumes.
        assert_eq!(upper.volumes.len(), 2);
        assert!(
            upper
                .volumes
                .iter()
                .all(|v| v.kind == PartKind::NegativeVolume)
        );
        let hole = &upper.volumes[0];
        let (lo, hi) = upper.volume_bounds(hole);
        assert!((hi.z - (4.2 - cc::PREVIEW_SHIFT)).abs() < 1e-3, "{hi}");
        assert!((lo.z + cc::PREVIEW_SHIFT).abs() < 1e-3, "{lo}");
        assert!((hi.x - lo.x - 4.2).abs() < 1e-2);
        // The lower half: its body and the plug as model parts, the dowel's lower hole.
        let region = lower.region.as_ref().unwrap();
        assert_eq!(region.volumes.len(), 2);
        assert_eq!(lower.parts.len(), 2);
        assert_eq!(lower.parts[1].0, "Connector-1");
        let plug = lower.placed_region().unwrap();
        let plug_volume = plug.volumes[1].mesh.volume();
        assert!(
            (plug_volume - std::f64::consts::PI * 4. * 4.).abs() < 0.05,
            "{plug_volume}"
        );
        let (_, hi) = lower.bounds();
        assert!(
            (hi.z - 14.).abs() < 1e-3,
            "the plug stands 4 mm on the cut face"
        );
        assert_eq!(lower.volumes.len(), 1);
        assert_eq!(lower.volumes[0].name, "Connector-2_B");
        // The dowel's pin: twice the depth tall, upright on the bed.
        let (lo, hi) = pin.bounds();
        assert!((hi.z - lo.z - 8.).abs() < 1e-3 && lo.z.abs() < 1e-4);
        assert!(mica_core::mesh_cut::is_watertight(&pin.placed_mesh()));
        // Placed on the cut face, the upper half turns over (its holes with it); the lower
        // half keeps standing on its base so the plug points up.
        c.place_on_cut = true;
        let parts = cut_object(&o, &c).unwrap();
        let (upper, lower) = (&parts[0], &parts[1]);
        let (lo, hi) = upper.volume_bounds(&upper.volumes[0]);
        assert!(
            lo.z < 0.1 && hi.z < 5.,
            "the hole opens on the bed: {lo} {hi}"
        );
        assert!((lower.bounds().1.z - 14.).abs() < 1e-3);
    }
}
