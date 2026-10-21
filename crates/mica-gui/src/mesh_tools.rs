// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: orca@8500fcdcca src/slic3r/GUI/Gizmos/GLGizmoMeshBoolean.cpp :: GLGizmoMeshBoolean::on_render_input_window, generate_new_volume
// Source: prusaslicer@b028299c77 src/slic3r/GUI/Gizmos/GLGizmoSimplify.cpp :: GLGizmoSimplify::on_render_input_window, process, apply_simplify, init_model
// Source: orca@8500fcdcca src/slic3r/GUI/GUI_ObjectList.cpp :: ObjectList::fix_through_cgal, get_mesh_errors_info
// Changes: The tools' state and actions only (their cards are in panels/mesh_tools.rs): the boolean takes the selected object and a second shape (another object, or one of the object's own volumes) with union / difference / intersection, "keep the other shape" defaulting like upstream's "Delete input" (union always consumes it) plus "keep the original object" (the result becomes a new object); results replace the object as a plain mesh in bed coordinates. Simplify follows PrusaSlicer's level-of-detail / decimate-ratio configuration with a background preview, applied to every instance (per part for multi-part objects, triangle share by ratio), and drops painting like upstream. Repair runs Mica's admesh-style repair on import (only for meshes that need it) and on demand; the object list's warning badge shows what it fixed instead of Orca's error counts.

//! Mesh tools on the selected object: the boolean card (union, difference, intersection
//! with another shape), the simplify card (fewer triangles, with a live preview) and mesh
//! repair (on import and from the menu). Every operation is one undo entry.

use crate::{
    app::App,
    scene::{self, Object, Volume},
};
use eframe::egui;
use glam::{Mat4, Vec3};
use mica_core::{
    mesh::TriangleMesh,
    mesh_boolean::{self, BooleanOp},
    mesh_repair::{self, RepairReport},
    mesh_simplify::{self, DETAIL_LEVELS},
    threemf::PartKind,
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};

/// The second shape of a boolean.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Other {
    /// Another object on the plate (`Object::id`).
    Object(u64),
    /// One of the object's own volumes (`Volume::id`).
    Volume(u64),
}

#[derive(Clone, Debug, PartialEq)]
pub struct BooleanState {
    /// The object operated on (`Object::id`).
    pub object_id: u64,
    pub op: BooleanOp,
    pub other: Option<Other>,
    /// Add the result as a new object and keep the original.
    pub keep_original: bool,
    /// Keep the second shape (upstream's "Delete input" unticked).
    pub keep_other: bool,
    /// Why the last attempt failed.
    pub message: Option<String>,
}

impl BooleanState {
    /// Upstream's default: a union always consumes the second part, difference and
    /// intersection keep it.
    pub fn default_keep_other(op: BooleanOp) -> bool {
        op != BooleanOp::Union
    }
    /// What the operation does, in plain words.
    pub fn explain(op: BooleanOp) -> &'static str {
        match op {
            BooleanOp::Union => "Joins both shapes into one solid.",
            BooleanOp::Difference => "Cuts the second shape out of this object.",
            BooleanOp::Intersection => "Keeps only the part where both shapes overlap.",
        }
    }
    /// The apply button's verb.
    pub fn verb(op: BooleanOp) -> &'static str {
        match op {
            BooleanOp::Union => "Join",
            BooleanOp::Difference => "Subtract",
            BooleanOp::Intersection => "Intersect",
        }
    }
}

/// Simplify parameters: a level of detail (quadric error limit) or a triangle count.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SimplifyParams {
    pub use_count: bool,
    /// Index into `DETAIL_LEVELS`.
    pub level: usize,
    /// Percent of triangles to remove (upstream's "decimate ratio").
    pub ratio: f32,
}

impl Default for SimplifyParams {
    fn default() -> Self {
        // Upstream opens with a 50 % ratio, the "Medium" level selected.
        Self {
            use_count: true,
            level: 2,
            ratio: 50.,
        }
    }
}

struct Job {
    params: SimplifyParams,
    cancel: Arc<AtomicBool>,
    rx: mpsc::Receiver<Option<Simplified>>,
}

/// A simplified object mesh: the merged mesh and, for multi-part objects, each part.
#[derive(Clone)]
pub struct Simplified {
    pub mesh: Arc<TriangleMesh>,
    pub parts: Option<Vec<TriangleMesh>>,
}

pub struct SimplifyState {
    pub object_id: u64,
    pub params: SimplifyParams,
    pub original: usize,
    pub wireframe: bool,
    /// The preview for `params` once computed.
    pub preview: Option<(SimplifyParams, Simplified)>,
    job: Option<Job>,
}

impl SimplifyState {
    pub fn computing(&self) -> bool {
        self.job.is_some()
    }
    /// Triangles the preview has (or will aim for).
    pub fn result_count(&self) -> Option<usize> {
        self.preview
            .as_ref()
            .filter(|(p, _)| *p == self.params)
            .map(|(_, s)| s.mesh.faces.len())
    }
    pub fn wanted_count(&self) -> usize {
        mesh_simplify::count_by_ratio(self.original, self.params.ratio)
    }
}

impl Drop for SimplifyState {
    fn drop(&mut self) {
        if let Some(j) = &self.job {
            j.cancel.store(true, Ordering::Relaxed);
        }
    }
}

pub enum MeshTool {
    Boolean(BooleanState),
    Simplify(SimplifyState),
}

/// Simplifies an object's mesh (each model part on its own, triangles shared out by ratio).
fn simplify_object(
    mesh: &TriangleMesh,
    region: Option<&mica_core::regions::RegionModel>,
    params: SimplifyParams,
    cancel: Option<&AtomicBool>,
) -> Option<Simplified> {
    let run = |m: &TriangleMesh| {
        if params.use_count {
            let target = mesh_simplify::count_by_ratio(m.faces.len(), params.ratio);
            mesh_simplify::simplify(m, target, None, cancel)
        } else {
            mesh_simplify::simplify(m, 0, Some(DETAIL_LEVELS[params.level].1), cancel)
        }
    };
    let models: Vec<&TriangleMesh> = region
        .map(|r| {
            r.volumes
                .iter()
                .filter(|v| v.kind == PartKind::Model)
                .map(|v| &v.mesh)
                .collect()
        })
        .unwrap_or_default();
    if models.len() > 1 {
        let parts: Vec<TriangleMesh> = models.iter().map(|m| run(m)).collect::<Option<_>>()?;
        let merged = mica_core::mesh_edit::merge(&parts.iter().collect::<Vec<_>>());
        return Some(Simplified {
            mesh: Arc::new(merged),
            parts: Some(parts),
        });
    }
    Some(Simplified {
        mesh: Arc::new(run(mesh)?),
        parts: None,
    })
}

/// Repairs a freshly loaded object when its mesh needs it (every model part of a 3MF
/// object on its own); returns what was fixed. Clean meshes are not touched.
pub fn repair_on_import(o: &mut mica_core::input::InputObject) -> Option<RepairReport> {
    let mut total = RepairReport::default();
    let mut changed = false;
    match &mut o.region {
        None => {
            if let Some((m, r)) = mesh_repair::repair(&o.mesh) {
                o.mesh = m;
                // The repaired mesh is no longer the loaded one placed.
                o.frame = None;
                add(&mut total, &r);
                changed = true;
            }
        }
        Some(model) => {
            for v in &mut model.volumes {
                if let Some((m, r)) = mesh_repair::repair(&v.mesh) {
                    v.mesh = m;
                    // The faces changed: the part's painting no longer fits.
                    v.painting = Default::default();
                    add(&mut total, &r);
                    changed = true;
                }
            }
            if changed {
                let models: Vec<&TriangleMesh> = model
                    .volumes
                    .iter()
                    .filter(|v| v.kind == PartKind::Model)
                    .map(|v| &v.mesh)
                    .collect();
                o.mesh = mica_core::mesh_edit::merge(&models);
            }
        }
    }
    if changed {
        o.source = None;
    }
    changed.then_some(total)
}

/// Loaded objects for the scene, each repaired on import when it needs it; one notice per
/// repaired object (instances share theirs).
pub fn import_objects(
    objects: Vec<mica_core::input::InputObject>,
    notices: &mut Vec<String>,
) -> Vec<(Option<usize>, Object)> {
    let mut noted = std::collections::BTreeSet::new();
    objects
        .into_iter()
        .map(|mut o| {
            let repair = repair_on_import(&mut o);
            let plate = o.plate;
            let mut object = Object::new(o);
            if let Some(r) = &repair
                && noted.insert(object.object_id)
            {
                notices.push(format!("{} was repaired: {r}", object.name));
            }
            object.repair = repair.map(Arc::new);
            (plate, object)
        })
        .collect()
}

fn add(total: &mut RepairReport, r: &RepairReport) {
    total.vertices_merged += r.vertices_merged;
    total.degenerate_removed += r.degenerate_removed;
    total.duplicates_removed += r.duplicates_removed;
    total.unconnected_removed += r.unconnected_removed;
    total.faces_flipped += r.faces_flipped;
    total.holes_filled += r.holes_filled;
    total.faces_added += r.faces_added;
    total.open_edges_left += r.open_edges_left;
}

/// The object's model parts in bed coordinates, unioned into one solid.
fn placed_solid(o: &Object) -> Result<TriangleMesh, String> {
    let models: Vec<TriangleMesh> = match &o.region {
        Some(r)
            if r.volumes
                .iter()
                .filter(|v| v.kind == PartKind::Model)
                .count()
                > 1 =>
        {
            let t = o.transform();
            r.volumes
                .iter()
                .filter(|v| v.kind == PartKind::Model)
                .map(|v| scene::transform_mesh(&v.mesh, t))
                .collect()
        }
        _ => vec![o.placed_mesh()],
    };
    let mut it = models.into_iter();
    let mut acc = it.next().ok_or("the object has no mesh")?;
    for m in it {
        acc = mesh_boolean::boolean(&acc, &m, BooleanOp::Union).map_err(|e| e.to_string())?;
    }
    Ok(acc)
}

fn volume_world(o: &Object, v: &Volume) -> TriangleMesh {
    scene::transform_mesh(
        &scene::transform_mesh(&v.mesh, v.local_transform()),
        o.transform(),
    )
}

impl App {
    /// Index of the object a mesh tool works on (by id: deleting or reordering objects must
    /// never retarget it).
    pub(crate) fn mesh_tool_target(&self) -> Option<usize> {
        let id = match self.mesh_tool.as_ref()? {
            MeshTool::Boolean(b) => b.object_id,
            MeshTool::Simplify(s) => s.object_id,
        };
        self.plates
            .current()
            .objects
            .iter()
            .position(|o| o.id == id)
    }

    /// Closes the tool when its object is gone or no longer the primary selection.
    pub(crate) fn validate_mesh_tool(&mut self) {
        if self.mesh_tool.is_some()
            && (self.mesh_tool_target().is_none() || self.mesh_tool_target() != self.selected)
        {
            self.mesh_tool = None;
        }
    }

    /// B: opens (or closes) the boolean card on the primary object.
    pub(crate) fn toggle_boolean(&mut self) {
        if matches!(self.mesh_tool, Some(MeshTool::Boolean(_))) {
            self.mesh_tool = None;
            return;
        }
        let Some(i) = self.selected else { return };
        if self.plate_locked() || i >= self.plates.current().objects.len() {
            return;
        }
        self.cut = None;
        self.close_emboss();
        let objects = &self.plates.current().objects;
        let o = &objects[i];
        // Default second shape: another selected object, else the nearest other object, else
        // the object's first volume.
        let other = self
            .selection()
            .into_iter()
            .find(|&k| k != i)
            .map(|k| Other::Object(objects[k].id))
            .or_else(|| {
                let c = o.pivot_world();
                objects
                    .iter()
                    .enumerate()
                    .filter(|(k, _)| *k != i)
                    .min_by(|a, b| {
                        let da = a.1.pivot_world().distance(c);
                        let db = b.1.pivot_world().distance(c);
                        da.total_cmp(&db)
                    })
                    .map(|(_, q)| Other::Object(q.id))
            })
            .or_else(|| o.volumes.first().map(|v| Other::Volume(v.id)));
        let op = BooleanOp::Union;
        self.mesh_tool = Some(MeshTool::Boolean(BooleanState {
            object_id: o.id,
            op,
            other,
            keep_original: false,
            keep_other: BooleanState::default_keep_other(op),
            message: None,
        }));
        self.select(Some(i), false);
    }

    /// Opens (or closes) the simplify card on the primary object.
    pub(crate) fn toggle_simplify(&mut self) {
        if matches!(self.mesh_tool, Some(MeshTool::Simplify(_))) {
            self.mesh_tool = None;
            return;
        }
        let Some(i) = self.selected else { return };
        if self.plate_locked() {
            return;
        }
        if i >= self.plates.current().objects.len() {
            return;
        }
        self.cut = None;
        self.close_emboss();
        let o = &self.plates.current().objects[i];
        self.mesh_tool = Some(MeshTool::Simplify(SimplifyState {
            object_id: o.id,
            params: SimplifyParams::default(),
            original: o.mesh.faces.len(),
            wireframe: true,
            preview: None,
            job: None,
        }));
        self.select(Some(i), false);
    }

    /// The second shapes the boolean card offers: (choice, label).
    pub(crate) fn boolean_choices(&self, i: usize) -> Vec<(Other, String)> {
        let objects = &self.plates.current().objects;
        let mut out = Vec::new();
        let stem = |n: &str| {
            std::path::Path::new(n)
                .file_stem()
                .map_or(n.to_string(), |s| s.to_string_lossy().into_owned())
        };
        for (k, q) in objects.iter().enumerate() {
            if k != i {
                out.push((Other::Object(q.id), format!("Object · {}", stem(&q.name))));
            }
        }
        if let Some(o) = objects.get(i) {
            for v in &o.volumes {
                out.push((
                    Other::Volume(v.id),
                    format!("Part · {} ({})", v.name, v.kind_label().to_lowercase()),
                ));
            }
        }
        out
    }

    /// Runs the boolean card's operation.
    pub(crate) fn apply_boolean(&mut self) {
        let Some(MeshTool::Boolean(b)) = &self.mesh_tool else {
            return;
        };
        let b = b.clone();
        let Some(other) = b.other else {
            self.set_boolean_message("Pick the second shape first");
            return;
        };
        match self.boolean_now(b.op, other, b.keep_original, b.keep_other) {
            Ok(()) => {
                // Stay open on the result for the next operation.
                if let Some(i) = self.selected {
                    let id = self.plates.current().objects[i].id;
                    let gone = b.other.is_some_and(|o| !self.boolean_exists(i, o));
                    if let Some(MeshTool::Boolean(state)) = self.mesh_tool.as_mut() {
                        state.object_id = id;
                        state.message = None;
                        if gone {
                            state.other = None;
                        }
                    }
                }
            }
            Err(e) => self.set_boolean_message(&e),
        }
    }

    fn boolean_exists(&self, i: usize, other: Other) -> bool {
        let objects = &self.plates.current().objects;
        match other {
            Other::Object(id) => objects
                .iter()
                .enumerate()
                .any(|(k, o)| k != i && o.id == id),
            Other::Volume(id) => objects.get(i).is_some_and(|o| o.volume(id).is_some()),
        }
    }

    fn set_boolean_message(&mut self, m: &str) {
        if let Some(MeshTool::Boolean(b)) = &mut self.mesh_tool {
            b.message = Some(m.to_string());
        }
        self.status = m.to_string();
    }

    /// `op` between the primary object and `other`; one undo entry. The result replaces the
    /// object (or, with `keep_original`, joins the plate as a new object).
    pub(crate) fn boolean_now(
        &mut self,
        op: BooleanOp,
        other: Other,
        keep_original: bool,
        keep_other: bool,
    ) -> Result<(), String> {
        let Some(i) = self.selected else {
            return Err("Select an object first".into());
        };
        if self.plate_locked() {
            return Err("The plate is locked".into());
        }
        let objects = &self.plates.current().objects;
        let o = objects.get(i).ok_or("No object")?.clone();
        let (tool_mesh, tool_index) = match other {
            Other::Object(id) => {
                let k = objects
                    .iter()
                    .position(|q| q.id == id && q.id != o.id)
                    .ok_or("The second object is gone")?;
                (placed_solid(&objects[k])?, Some(k))
            }
            Other::Volume(id) => {
                let v = o.volume(id).ok_or("The part is gone")?;
                (volume_world(&o, v), None)
            }
        };
        let source = placed_solid(&o)?;
        let result = mesh_boolean::boolean(&source, &tool_mesh, op)
            .map_err(|e| format!("{}: {e}", op.label()))?;
        if result.faces.is_empty() {
            return Err(match op {
                BooleanOp::Intersection => "The shapes do not overlap: nothing would be left",
                _ => "Nothing would be left of the object",
            }
            .into());
        }
        let label = match op {
            BooleanOp::Union => "union",
            BooleanOp::Difference => "difference",
            BooleanOp::Intersection => "intersection",
        };
        // A changed mesh is no longer an instance of the others.
        let shared = objects
            .iter()
            .filter(|q| q.object_id == o.object_id)
            .count()
            > 1;
        self.record(&format!("Mesh boolean ({label})"));
        let stem = std::path::Path::new(&o.name)
            .file_stem()
            .map_or(o.name.clone(), |s| s.to_string_lossy().into_owned());
        let name = if keep_original {
            format!("{stem} {label}")
        } else {
            o.name.clone()
        };
        let mut n = Object::from_mesh(name, result, &o);
        if keep_original || shared {
            n.object_id = self.next_free_object_id();
        }
        // The object's other volumes go along, moved into the new (bed) frame.
        let consumed = match other {
            Other::Volume(id) if !keep_other => Some(id),
            _ => None,
        };
        n.volumes = o
            .volumes
            .iter()
            .filter(|v| Some(v.id) != consumed)
            .map(|v| {
                // Volumes keep their ids when the object is replaced (the card's choice
                // stays valid); a kept original needs fresh ones for the copy.
                let mut v = if keep_original { v.fresh() } else { v.clone() };
                let t = o.transform() * v.local_transform();
                let (s, r, p) = t.to_scale_rotation_translation();
                v.position = p;
                v.rotation = r;
                v.scale = s;
                v
            })
            .collect();
        n.height_ranges = o.height_ranges.iter().map(|r| r.fresh()).collect();
        if n.lowest_z() > 1e-3 {
            n.drop_to_bed();
        }
        let plate = self.plates.current_mut();
        let target = if keep_original {
            plate.objects.push(n);
            plate.objects.len() - 1
        } else {
            plate.objects[i] = n;
            i
        };
        let mut target = target;
        if let (Some(k), false) = (tool_index, keep_other) {
            plate.objects.remove(k);
            if k < target {
                target -= 1;
            }
        }
        // An open card follows the result.
        let new_id = self.plates.current().objects[target].id;
        if let Some(MeshTool::Boolean(b)) = &mut self.mesh_tool {
            b.object_id = new_id;
        }
        self.select(Some(target), false);
        self.changed();
        self.status = format!(
            "{} done: {} triangles",
            op.label(),
            self.plates.current().objects[target].mesh.faces.len()
        );
        Ok(())
    }

    fn next_free_object_id(&self) -> usize {
        self.plates
            .plates
            .iter()
            .flat_map(|p| p.objects.iter().map(|o| o.object_id))
            .max()
            .map_or(0, |n| n + 1)
    }

    /// Per frame: collects a finished preview and starts one for changed parameters.
    pub(crate) fn poll_simplify(&mut self, ctx: &egui::Context) {
        let target = self.mesh_tool_target();
        let Some(MeshTool::Simplify(s)) = &mut self.mesh_tool else {
            return;
        };
        let Some(i) = target else { return };
        if let Some(job) = &s.job {
            match job.rx.try_recv() {
                Ok(Some(result)) => {
                    s.preview = Some((job.params, result));
                    s.job = None;
                }
                Ok(None) | Err(mpsc::TryRecvError::Disconnected) => s.job = None,
                Err(mpsc::TryRecvError::Empty) => {
                    if job.params != s.params {
                        job.cancel.store(true, Ordering::Relaxed);
                        s.job = None;
                    } else {
                        ctx.request_repaint_after(std::time::Duration::from_millis(30));
                    }
                }
            }
        }
        let fresh = s.preview.as_ref().is_some_and(|(p, _)| *p == s.params);
        if s.job.is_none() && !fresh {
            let o = &self.plates.current().objects[i];
            let mesh = o.mesh.clone();
            let region = o.region.clone();
            let params = s.params;
            let cancel = Arc::new(AtomicBool::new(false));
            let (tx, rx) = mpsc::channel();
            let flag = cancel.clone();
            let repaint = ctx.clone();
            std::thread::spawn(move || {
                let r = simplify_object(&mesh, region.as_deref(), params, Some(&flag));
                let _ = tx.send(r);
                repaint.request_repaint();
            });
            s.job = Some(Job { params, cancel, rx });
        }
    }

    /// The mesh the view shows for object `o` of plate `plate` while the simplify card
    /// previews it.
    pub(crate) fn simplify_preview(&self, plate: usize, o: &Object) -> Option<Arc<TriangleMesh>> {
        let Some(MeshTool::Simplify(s)) = &self.mesh_tool else {
            return None;
        };
        if plate != self.plates.current || o.id != s.object_id {
            return None;
        }
        s.preview.as_ref().map(|(_, r)| r.mesh.clone())
    }

    /// Applies the simplify card's preview (computed now if it is not ready) to the object
    /// and its instances.
    pub(crate) fn apply_simplify(&mut self) {
        let Some(i) = self.mesh_tool_target() else {
            return;
        };
        let Some(MeshTool::Simplify(s)) = &self.mesh_tool else {
            return;
        };
        let params = s.params;
        let ready = s
            .preview
            .as_ref()
            .filter(|(p, _)| *p == params)
            .map(|(_, r)| r.clone());
        let o = &self.plates.current().objects[i];
        let result = match ready {
            Some(r) => r,
            None => match simplify_object(&o.mesh, o.region.as_deref(), params, None) {
                Some(r) => r,
                None => return,
            },
        };
        let before = o.mesh.faces.len();
        self.set_object_mesh(i, &result, "Simplify");
        let after = self.plates.current().objects[i].mesh.faces.len();
        self.status = format!("Simplified: {before} → {after} triangles");
        self.mesh_tool = None;
    }

    /// Simplifies the primary object to about `triangles` triangles (monkey, tests).
    pub(crate) fn simplify_now(&mut self, ratio: f32) {
        let Some(i) = self.selected else { return };
        if self.plate_locked() {
            return;
        }
        let o = &self.plates.current().objects[i];
        let params = SimplifyParams {
            use_count: true,
            level: 2,
            ratio,
        };
        if let Some(r) = simplify_object(&o.mesh, o.region.as_deref(), params, None) {
            self.set_object_mesh(i, &r, "Simplify");
        }
    }

    /// Puts a new mesh on object `i` and every instance of it (one undo entry); painting
    /// goes, since the faces changed.
    fn set_object_mesh(&mut self, i: usize, result: &Simplified, label: &str) {
        self.record(label);
        let object_id = self.plates.current().objects[i].object_id;
        let plate = self.plates.current_mut();
        let region = plate.objects[i].region.as_ref().map(|r| {
            let mut r = (**r).clone();
            if let Some(parts) = &result.parts {
                let mut it = parts.iter();
                for v in r.volumes.iter_mut().filter(|v| v.kind == PartKind::Model) {
                    if let Some(m) = it.next() {
                        v.mesh = m.clone();
                    }
                    v.painting = Default::default();
                }
            } else {
                for v in r.volumes.iter_mut().filter(|v| v.kind == PartKind::Model) {
                    v.mesh = (*result.mesh).clone();
                    v.painting = Default::default();
                }
            }
            Arc::new(r)
        });
        for o in plate
            .objects
            .iter_mut()
            .filter(|o| o.object_id == object_id)
        {
            o.set_mesh((*result.mesh).clone());
            o.mesh = result.mesh.clone();
            o.region = region.clone();
            o.source = None;
            o.painting = Arc::new(Default::default());
            o.painting_edited = true;
        }
        self.changed();
    }

    /// "Bake texture into mesh": the selected object's texture on all surfaces (from its
    /// settings, modifiers and texture paint) becomes its mesh, as the slicer displaces it;
    /// then its texture settings and texture paint are turned off so it is not textured twice.
    /// One undo entry. Returns the new triangle count.
    pub(crate) fn bake_texture(&mut self) -> Option<usize> {
        let i = self.selected?;
        if self.plate_locked() || !self.prefs.features.textures {
            return None;
        }
        let result = (|| -> Result<Option<(Simplified, usize)>, String> {
            let config = self.job_config(self.plates.current)?;
            let o = &self.plates.current().objects[i];
            let input = o.slicing_input(crate::worker::object_config(&config, o)?);
            let settings = mica_core::settings::PrintSettings::from_config(
                &mica_core::print_job::tool_config(&input.config, 0),
            )
            .map_err(|e| e.to_string())?;
            let Some(d) = mica_core::texture::displace::bake(
                &input.mesh,
                &settings,
                input.region.as_ref().map(|m| (m, &input.config)),
            )?
            else {
                return Ok(None);
            };
            // Back from the bed into the object's own frame.
            let inverse = o.transform().inverse();
            let local = |m: &TriangleMesh| TriangleMesh {
                vertices: m
                    .vertices
                    .iter()
                    .map(|v| inverse.transform_point3(Vec3::from_array(*v)).to_array())
                    .collect(),
                faces: m.faces.clone(),
            };
            let parts = o.region.as_ref().map(|r| {
                let placed = input.region.as_ref();
                r.volumes
                    .iter()
                    .filter(|v| v.kind == PartKind::Model)
                    .zip(
                        placed
                            .map(|p| {
                                p.volumes
                                    .iter()
                                    .enumerate()
                                    .filter(|(_, v)| v.kind == PartKind::Model)
                                    .map(|(k, _)| d.volumes.get(k).cloned().flatten())
                                    .collect::<Vec<_>>()
                            })
                            .unwrap_or_default()
                            .into_iter()
                            .chain(std::iter::repeat(None)),
                    )
                    .map(|(v, baked)| baked.as_ref().map_or_else(|| v.mesh.clone(), local))
                    .collect()
            });
            let mesh = local(&d.mesh);
            let n = mesh.faces.len();
            Ok(Some((
                Simplified {
                    mesh: Arc::new(mesh),
                    parts,
                },
                n,
            )))
        })();
        match result {
            Ok(Some((baked, n))) => {
                self.set_object_mesh(i, &baked, "Bake texture");
                let object_id = self.plates.current().objects[i].object_id;
                let strip = |m: &mut std::collections::BTreeMap<String, String>| {
                    m.retain(|k, _| !k.starts_with("mica_texture"));
                };
                for o in self
                    .plates
                    .current_mut()
                    .objects
                    .iter_mut()
                    .filter(|o| o.object_id == object_id)
                {
                    strip(&mut o.overrides);
                    o.overrides
                        .insert("mica_texture_skin".into(), "none".into());
                    for v in &mut o.volumes {
                        strip(&mut v.overrides);
                    }
                    for r in &mut o.height_ranges {
                        strip(&mut r.overrides);
                    }
                    if let Some(r) = &mut o.region
                        && mica_core::features::region_uses_textures(r)
                    {
                        mica_core::features::region_without_textures(Arc::make_mut(r));
                    }
                }
                self.changed();
                self.notify_info(format!("Baked the texture into the mesh: {n} triangles"));
                Some(n)
            }
            Ok(None) => {
                self.notify_info(
                    "Nothing to bake: set Textured surfaces to All surfaces (or paint a slot on all surfaces) first".to_string(),
                );
                None
            }
            Err(e) => {
                self.notify_error("The texture was not baked", e);
                None
            }
        }
    }

    /// "Repair mesh": repairs every selected object (and its instances); one undo entry.
    pub(crate) fn repair_selection(&mut self) {
        let sel = self.selection();
        if sel.is_empty() || self.plate_locked() {
            return;
        }
        let objects = &self.plates.current().objects;
        let mut work: Vec<(usize, TriangleMesh, Option<Vec<TriangleMesh>>, RepairReport)> =
            Vec::new();
        let mut done = std::collections::BTreeSet::new();
        for &i in &sel {
            let o = &objects[i];
            if !done.insert(o.object_id) {
                continue;
            }
            let mut total = RepairReport::default();
            let models: Vec<&TriangleMesh> = o
                .region
                .as_ref()
                .map(|r| {
                    r.volumes
                        .iter()
                        .filter(|v| v.kind == PartKind::Model)
                        .map(|v| &v.mesh)
                        .collect()
                })
                .unwrap_or_default();
            if models.len() > 1 {
                let parts: Vec<TriangleMesh> = models
                    .iter()
                    .map(|m| {
                        let (r, rep) = mesh_repair::repair_always(m);
                        add(&mut total, &rep);
                        r
                    })
                    .collect();
                let merged = mica_core::mesh_edit::merge(&parts.iter().collect::<Vec<_>>());
                work.push((i, merged, Some(parts), total));
            } else {
                let (r, rep) = mesh_repair::repair_always(&o.mesh);
                work.push((i, r, None, rep));
            }
        }
        let fixed: Vec<_> = work.into_iter().filter(|w| !w.3.is_empty()).collect();
        if fixed.is_empty() {
            self.status = "Repair: no errors found".into();
            // Nothing to fix any more: the badges go.
            for &i in &sel {
                self.plates.current_mut().objects[i].repair = None;
            }
            return;
        }
        let summary = fixed
            .iter()
            .map(|w| w.3.to_string())
            .collect::<Vec<_>>()
            .join("; ");
        let mut first = true;
        for (i, mesh, parts, report) in fixed {
            if first {
                self.set_object_mesh(
                    i,
                    &Simplified {
                        mesh: Arc::new(mesh),
                        parts,
                    },
                    "Repair mesh",
                );
                first = false;
            } else {
                // Same undo entry: apply without recording again.
                let object_id = self.plates.current().objects[i].object_id;
                let mesh = Arc::new(mesh);
                for o in self
                    .plates
                    .current_mut()
                    .objects
                    .iter_mut()
                    .filter(|o| o.object_id == object_id)
                {
                    o.set_mesh((*mesh).clone());
                    o.mesh = mesh.clone();
                    if let (Some(parts), Some(r)) = (&parts, o.region.as_ref()) {
                        let mut r = (**r).clone();
                        for (v, m) in r
                            .volumes
                            .iter_mut()
                            .filter(|v| v.kind == PartKind::Model)
                            .zip(parts)
                        {
                            v.mesh = m.clone();
                            v.painting = Default::default();
                        }
                        o.region = Some(Arc::new(r));
                    }
                    o.source = None;
                    o.painting = Arc::new(Default::default());
                    o.painting_edited = true;
                }
                self.changed();
            }
            let object_id = self.plates.current().objects[i].object_id;
            let report = Arc::new(report);
            for o in self
                .plates
                .current_mut()
                .objects
                .iter_mut()
                .filter(|o| o.object_id == object_id)
            {
                o.repair = Some(report.clone());
            }
        }
        self.status = format!("Repaired: {summary}");
    }

    /// The screenshot tool's mesh-tool commands (`--command boolean:difference`, …); `None`
    /// for other names.
    pub(crate) fn mesh_tool_script(&mut self, name: &str, arg: &str) -> Option<Result<(), String>> {
        let r = match name {
            "boolean" => {
                if !matches!(self.mesh_tool, Some(MeshTool::Boolean(_))) {
                    self.toggle_boolean();
                }
                let op = BooleanOp::ALL
                    .into_iter()
                    .find(|o| o.label().eq_ignore_ascii_case(arg));
                match (&mut self.mesh_tool, op) {
                    (Some(MeshTool::Boolean(b)), Some(op)) => {
                        b.op = op;
                        b.keep_other = BooleanState::default_keep_other(op);
                        Ok(())
                    }
                    (Some(MeshTool::Boolean(_)), None) if arg.is_empty() => Ok(()),
                    (Some(_), None) => Err(format!("--command boolean: unknown operation {arg}")),
                    _ => Err("--command boolean: select an object first".into()),
                }
            }
            // Moves object 1 so its box corner sits at object 0's corner + (dx, dy, dz).
            "overlap" => {
                let d: Vec<f32> = arg
                    .split(',')
                    .filter_map(|v| v.trim().parse().ok())
                    .collect();
                let objects = &self.plates.current().objects;
                if d.len() != 3 || objects.len() < 2 {
                    return Some(Err("--command overlap:DX,DY,DZ needs two objects".into()));
                }
                let (a, _) = objects[0].bounds();
                let (b, _) = objects[1].bounds();
                self.plates.current_mut().objects[1].position +=
                    a + Vec3::new(d[0], d[1], d[2]) - b;
                self.changed();
                Ok(())
            }
            "boolean-consume" => {
                if let Some(MeshTool::Boolean(b)) = &mut self.mesh_tool {
                    b.keep_other = false;
                }
                Ok(())
            }
            "boolean-apply" => {
                self.apply_boolean();
                match &self.mesh_tool {
                    Some(MeshTool::Boolean(b)) if b.message.is_some() => {
                        Err(b.message.clone().unwrap_or_default())
                    }
                    _ => Ok(()),
                }
            }
            "simplify" => {
                if !matches!(self.mesh_tool, Some(MeshTool::Simplify(_))) {
                    self.toggle_simplify();
                }
                if let Some(MeshTool::Simplify(s)) = &mut self.mesh_tool
                    && let Ok(ratio) = arg.parse::<f32>()
                {
                    s.params.use_count = true;
                    s.params.ratio = ratio.clamp(0., 99.);
                }
                // Wait for the preview so the shot shows it.
                let ctx = egui::Context::default();
                for _ in 0..2000 {
                    self.poll_simplify(&ctx);
                    match &self.mesh_tool {
                        Some(MeshTool::Simplify(s)) if s.result_count().is_none() => {
                            std::thread::sleep(std::time::Duration::from_millis(5))
                        }
                        _ => break,
                    }
                }
                Ok(())
            }
            "simplify-apply" => {
                self.apply_simplify();
                Ok(())
            }
            "repair" => {
                self.repair_selection();
                Ok(())
            }
            _ => return None,
        };
        Some(r)
    }

    /// Draws the simplify preview's wireframe over the view (front-facing edges only).
    pub(crate) fn draw_simplify_wireframe(&self, ctx: &egui::Context) {
        const MAX_TRIANGLES: usize = 60_000;
        let Some(MeshTool::Simplify(s)) = &self.mesh_tool else {
            return;
        };
        if !s.wireframe || self.preview {
            return;
        }
        let (Some(i), Some((_, r))) = (self.mesh_tool_target(), s.preview.as_ref()) else {
            return;
        };
        let mesh = &r.mesh;
        if mesh.faces.len() > MAX_TRIANGLES {
            return;
        }
        let o = &self.plates.current().objects[i];
        let mut placed = o.clone();
        placed.position += self.plates.offset(self.plates.current);
        let t: Mat4 = placed.transform();
        let rect = self.viewport_rect;
        let aspect = rect.aspect_ratio().max(1e-3);
        let cam = &self.viewport.camera;
        let world: Vec<Vec3> = mesh
            .vertices
            .iter()
            .map(|v| t.transform_point3(Vec3::from(*v)))
            .collect();
        let screen: Vec<Option<egui::Pos2>> = world
            .iter()
            .map(|&w| {
                cam.project(w, aspect)
                    .map(|uv| rect.min + egui::vec2(uv.x * rect.width(), uv.y * rect.height()))
            })
            .collect();
        let eye = cam.eye();
        let flip = t.determinant() < 0.;
        let mut edges = std::collections::HashSet::new();
        for f in &mesh.faces {
            let [a, b, c] = f.map(|k| world[k as usize]);
            let mut n = (b - a).cross(c - a);
            if flip {
                n = -n;
            }
            let towards = if cam.perspective {
                eye - a
            } else {
                -cam.direction()
            };
            if n.dot(towards) <= 0. {
                continue;
            }
            for k in 0..3 {
                let (u, v) = (f[k], f[(k + 1) % 3]);
                edges.insert((u.min(v), u.max(v)));
            }
        }
        let p = crate::theme::of_ctx(ctx);
        // The view's own layer, after the 3D image and under the tool cards.
        let painter = ctx
            .layer_painter(egui::LayerId::background())
            .with_clip_rect(rect);
        let stroke = egui::Stroke::new(1., p.accent_hi.gamma_multiply(0.75));
        for (u, v) in edges {
            if let (Some(a), Some(b)) = (screen[u as usize], screen[v as usize]) {
                painter.line_segment([a, b], stroke);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{App, Options};
    use mica_profiles::user::Store;
    use std::path::PathBuf;
    use std::sync::atomic::AtomicU64;

    static NEXT: AtomicU64 = AtomicU64::new(0);

    fn app(models: &[&str]) -> App {
        let ctx = egui::Context::default();
        let dir = std::env::temp_dir().join(format!(
            "mica-meshtools-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        App::create(
            &ctx,
            Options {
                prefs_path: None,
                store: Some(Store::open(dir)),
                files: models
                    .iter()
                    .map(|m| {
                        PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/models/"))
                            .join(m)
                    })
                    .collect(),
                headless: true,
            },
        )
    }

    fn closed(m: &TriangleMesh) -> bool {
        mesh_repair::check(m).bad_edges == 0
    }

    /// Bake texture: the textured cube becomes a closed, finer mesh with the texture on top,
    /// its texture is off afterwards (so it is not textured twice), and undo brings it back.
    #[test]
    fn baking_a_texture_makes_it_the_mesh() {
        use crate::panels::editor::Edit;
        let mut a = app(&["cube20.stl"]);
        a.select(Some(0), false);
        assert_eq!(a.bake_texture(), None, "nothing to bake without a texture");
        for (key, raw) in [
            ("mica_texture_skin", "external"),
            ("mica_texture_skin_surfaces", "walls_and_tops"),
        ] {
            a.apply_edit(
                None,
                Edit::Set {
                    key: key.into(),
                    raw: raw.into(),
                },
            );
        }
        let before = a.plates.current().objects[0].mesh.faces.len();
        let (_, top) = a.plates.current().objects[0].bounds();
        let n = a.bake_texture().expect("baked");
        let o = &a.plates.current().objects[0];
        assert_eq!(o.mesh.faces.len(), n);
        assert!(n > 100 * before);
        assert!(closed(&o.mesh));
        assert!(o.bounds().1.z > top.z + 0.1, "the top carries the texture");
        assert_eq!(
            o.overrides.get("mica_texture_skin").map(String::as_str),
            Some("none")
        );
        a.undo();
        let o = &a.plates.current().objects[0];
        assert_eq!(o.mesh.faces.len(), before);
        assert_eq!(
            o.overrides
                .get("mica_texture_skin_surfaces")
                .map(String::as_str),
            Some("walls_and_tops")
        );
    }

    /// Two cubes side by side, moved to overlap: union, difference and intersection each
    /// make a closed object, undo restores both inputs, and "keep" options are honoured.
    #[test]
    fn boolean_ops_replace_keep_and_undo() {
        let mut a = app(&["cube20.stl", "cube20.stl"]);
        assert_eq!(a.plates.current().objects.len(), 2);
        // Overlap the second cube with the first by 10 mm in X and 5 mm in Y.
        let (lo0, _) = a.plates.current().objects[0].bounds();
        let (lo1, _) = a.plates.current().objects[1].bounds();
        a.plates.current_mut().objects[1].position += lo0 + Vec3::new(10., 5., 0.) - lo1;
        let other = a.plates.current().objects[1].id;
        a.select(Some(0), false);
        a.boolean_now(BooleanOp::Union, Other::Object(other), false, false)
            .unwrap();
        let objects = &a.plates.current().objects;
        assert_eq!(objects.len(), 1);
        assert!(closed(&objects[0].mesh));
        assert!((objects[0].mesh.volume() - (16000. - 10. * 15. * 20.)).abs() < 0.1);
        a.undo();
        assert_eq!(a.plates.current().objects.len(), 2);

        a.select(Some(0), false);
        a.boolean_now(BooleanOp::Difference, Other::Object(other), true, true)
            .unwrap();
        let objects = &a.plates.current().objects;
        assert_eq!(objects.len(), 3, "original, other and the result");
        let result = &objects[a.selected.unwrap()];
        assert!(closed(&result.mesh));
        assert!((result.mesh.volume() - (8000. - 3000.)).abs() < 0.1);
        a.undo();

        a.select(Some(0), false);
        a.boolean_now(BooleanOp::Intersection, Other::Object(other), false, true)
            .unwrap();
        let objects = &a.plates.current().objects;
        assert_eq!(objects.len(), 2);
        let result = &objects[a.selected.unwrap()];
        assert!((result.mesh.volume() - 3000.).abs() < 0.1);
        // The intersection rests on the bed.
        assert!(result.lowest_z().abs() < 1e-3);
    }

    #[test]
    fn boolean_with_own_negative_volume_and_project_round_trip() {
        let mut a = app(&["cube20.stl"]);
        a.select(Some(0), false);
        a.add_volume(PartKind::NegativeVolume, crate::commands::Shape::Cylinder);
        let vid = a.plates.current().objects[0].volumes[0].id;
        // Through the card: the object's own volume is the default second shape.
        a.toggle_boolean();
        let Some(MeshTool::Boolean(b)) = &mut a.mesh_tool else {
            panic!("card not open")
        };
        assert_eq!(b.other, Some(Other::Volume(vid)));
        b.op = BooleanOp::Difference;
        b.keep_other = false;
        a.apply_boolean();
        // The card stays open on the result.
        let new_id = a.plates.current().objects[0].id;
        assert!(
            matches!(&a.mesh_tool, Some(MeshTool::Boolean(b)) if b.object_id == new_id && b.message.is_none())
        );
        let o = a.plates.current().objects[0].clone();
        assert!(o.volumes.is_empty(), "the part was consumed");
        assert!(closed(&o.mesh));
        assert!(o.mesh.volume() < 8000. - 100.);
        // The edited mesh survives a project save and load.
        let dir = std::env::temp_dir().join(format!(
            "mica-meshtools-save-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bool.3mf");
        a.save_project_to(&path, mica_core::threemf::Flavour::Prusa)
            .unwrap();
        let b = app(&[]);
        let loaded = crate::app::load_files(vec![path], &b.config, Vec3::ZERO).unwrap();
        let (_, back) = &loaded.objects[0];
        assert_eq!(back.mesh.faces.len(), o.mesh.faces.len());
        assert!((back.mesh.volume() - o.mesh.volume()).abs() < 0.1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An edited copy stops sharing with untouched instances (instances compare placed
    /// meshes) and the slice cache sees new geometry (meshes compare by identity).
    #[test]
    fn edited_instances_stop_sharing_and_invalidate_the_slice_key() {
        let mut a = app(&["cube20.stl", "hole_block.stl"]);
        a.select(Some(0), false);
        a.add_instances();
        let objects = &a.plates.current().objects;
        let copies: Vec<usize> = (0..objects.len())
            .filter(|&k| objects[k].object_id == objects[0].object_id)
            .collect();
        assert_eq!(copies.len(), 2);
        let config = a.config.clone().unwrap();
        let shared = |a: &App| {
            let o = &a.plates.current().objects;
            let meshes: Vec<TriangleMesh> = copies.iter().map(|&k| o[k].placed_mesh()).collect();
            let tools: Vec<_> = copies.iter().map(|&k| o[k].tools.clone()).collect();
            let c: Vec<_> = meshes
                .iter()
                .zip(&tools)
                .map(|(m, t)| mica_core::instances::Candidate {
                    mesh: m,
                    region: None,
                    tools: t,
                    config: &config,
                })
                .collect();
            mica_core::instances::group(&c)[1].0 == 0
        };
        assert!(shared(&a), "untouched copies share");
        let before = a.plates.current().objects[copies[1]].clone();
        // Union the second copy with the other object, keeping that one.
        a.select(Some(copies[1]), false);
        let other = a
            .plates
            .current()
            .objects
            .iter()
            .find(|o| o.name.contains("hole"))
            .unwrap()
            .id;
        let (lo0, _) = a.plates.current().objects[copies[1]].bounds();
        let k = a
            .plates
            .current()
            .objects
            .iter()
            .position(|o| o.id == other)
            .unwrap();
        let (lo1, _) = a.plates.current().objects[k].bounds();
        a.plates.current_mut().objects[k].position += lo0 + Vec3::new(10., 0., 0.) - lo1;
        a.boolean_now(BooleanOp::Union, Other::Object(other), false, true)
            .unwrap();
        let after = &a.plates.current().objects[a.selected.unwrap()];
        assert_ne!(after.object_id, before.object_id, "no longer an instance");
        assert!(after != &before, "the slice key changes with the mesh");
        assert!(!shared(&a), "the edited copy is prepared on its own");
    }

    #[test]
    fn simplify_preview_apply_and_undo() {
        let mut a = app(&["cube20.stl"]);
        // A finer mesh: replace the cube by a sphere.
        let s = mica_core::mesh_edit::sphere(10., 24, 48);
        let n = s.faces.len();
        a.plates.current_mut().objects[0].set_mesh(s);
        a.select(Some(0), false);
        a.toggle_simplify();
        let ctx = egui::Context::default();
        for _ in 0..500 {
            a.poll_simplify(&ctx);
            if let Some(MeshTool::Simplify(st)) = &a.mesh_tool
                && st.result_count().is_some()
            {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let Some(MeshTool::Simplify(st)) = &a.mesh_tool else {
            panic!("no tool")
        };
        let count = st.result_count().expect("preview");
        assert!(count <= n / 2 && count + 2 > n / 2);
        assert!(
            a.simplify_preview(a.plates.current, &a.plates.current().objects[0])
                .is_some()
        );
        let before = a.plates.current().objects[0].clone();
        a.apply_simplify();
        let o = &a.plates.current().objects[0];
        assert!(o != &before, "the slice key changes with the mesh");
        assert_eq!(o.mesh.faces.len(), count);
        assert!(closed(&o.mesh));
        a.undo();
        assert_eq!(a.plates.current().objects[0].mesh.faces.len(), n);
    }

    #[test]
    fn repair_on_import_and_on_demand() {
        // A cube with one face missing, written as STL.
        let mut m = mica_core::mesh_edit::cuboid([20., 20., 20.]);
        m.faces.truncate(10);
        let dir = std::env::temp_dir().join(format!(
            "mica-meshtools-repair-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("open.stl");
        let mut f = std::fs::File::create(&path).unwrap();
        mica_core::mesh_edit::write_stl(&m, "open", &mut f).unwrap();
        drop(f);
        let b = app(&[]);
        let loaded = crate::app::load_files(vec![path], &b.config, Vec3::ZERO).unwrap();
        let (_, o) = &loaded.objects[0];
        assert!(closed(&o.mesh));
        let report = o.repair.as_ref().expect("repaired on import");
        assert_eq!(report.holes_filled, 1);
        // A clean model gets no badge.
        let clean = app(&["cube20.stl"]);
        assert!(clean.plates.current().objects[0].repair.is_none());
        // On demand: break the clean cube, repair, undo.
        let mut a = clean;
        let mut broken = (*a.plates.current().objects[0].mesh).clone();
        broken.faces[3].swap(1, 2);
        a.plates.current_mut().objects[0].set_mesh(broken);
        a.select(Some(0), false);
        a.repair_selection();
        let o = &a.plates.current().objects[0];
        assert!(closed(&o.mesh));
        assert_eq!(o.repair.as_ref().unwrap().faces_flipped, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
