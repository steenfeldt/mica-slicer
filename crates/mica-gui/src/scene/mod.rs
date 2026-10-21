// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

pub mod bed;
pub mod bed_assets;
pub mod camera;
pub mod outside;
pub mod paint;
pub mod picking;
pub mod plates;
pub use bed::{Bed, Placement};
use glam::{Mat4, Quat, Vec2, Vec3};
use mica_core::{
    config::Config, mesh::TriangleMesh, painting::FacetPainting, print_job::ToolAssignments,
    threemf::PartKind,
};
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// A transform change produced by a gizmo or a panel; applied with [`Object::apply`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Delta {
    Translate(Vec3),
    /// Rotation about a world axis through the object's pivot, in radians.
    Rotate {
        axis: Vec3,
        angle: f32,
    },
    /// Per-axis factors (uniform scaling passes the same factor three times).
    Scale(Vec3),
}

/// A modifier, negative volume or support enforcer / blocker that belongs to an object and is
/// edited in the GUI. Its mesh is in the volume's own frame (primitives are centred at the
/// origin); `position`, `rotation` and `scale` place it in the **object's local frame** (the
/// frame of `Object::mesh`), so it follows every move, turn and scale of the object.
#[derive(Clone, Debug)]
pub struct Volume {
    pub id: u64,
    pub name: String,
    pub kind: PartKind,
    pub mesh: Arc<TriangleMesh>,
    pub local_bounds: ([f32; 3], [f32; 3]),
    pub position: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
    /// This volume's own setting overrides (modifiers only in practice; support volumes and
    /// negative volumes carry none).
    pub overrides: BTreeMap<String, String>,
    /// The editable text / SVG source of an embossed volume (`crate::emboss`).
    pub emboss: Option<Arc<crate::emboss::Emboss>>,
    /// The user group this volume sits in within its kind in the object tree ("Hull
    /// details"); empty: none. Display only, saved in Mica's own metadata (`GROUP_KEY`).
    pub group: String,
}
impl PartialEq for Volume {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
            && self.name == other.name
            && self.kind == other.kind
            && Arc::ptr_eq(&self.mesh, &other.mesh)
            && self.position == other.position
            && self.rotation == other.rotation
            && self.scale == other.scale
            && self.overrides == other.overrides
            && self.emboss == other.emboss
            && self.group == other.group
    }
}
impl Volume {
    pub fn new(name: impl Into<String>, kind: PartKind, mesh: TriangleMesh) -> Self {
        Self {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            name: name.into(),
            kind,
            local_bounds: mesh.bounds(),
            mesh: Arc::new(mesh),
            position: Vec3::ZERO,
            rotation: Quat::IDENTITY,
            scale: Vec3::ONE,
            overrides: BTreeMap::new(),
            emboss: None,
            group: String::new(),
        }
    }
    /// A copy with a fresh id (for duplicates and pasted objects); the mesh stays shared.
    pub fn fresh(&self) -> Self {
        Self {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            ..self.clone()
        }
    }
    /// Placement in the object's local frame.
    pub fn local_transform(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(self.scale, self.rotation, self.position)
    }
    /// Short label for lists and the sidebar scope: `Modifier`, `Negative volume`, …
    pub fn kind_label(&self) -> &'static str {
        kind_label(&self.kind)
    }
    /// Sparse engine configuration from the overrides (invalid values are skipped).
    pub fn config(&self) -> Config {
        config_from(&self.overrides)
    }
}

/// A layer-height range of an object with its own overrides. `min_z..max_z` are millimetres
/// **above the object's lowest point** (which rests on the bed), so "5 to 10 mm" reads as the
/// user expects whatever the object's transform is.
#[derive(Clone, Debug, PartialEq)]
pub struct HeightRange {
    pub id: u64,
    pub min_z: f32,
    pub max_z: f32,
    pub overrides: BTreeMap<String, String>,
    /// The user group of the range in the object tree (see [`Volume::group`]).
    pub group: String,
}
impl HeightRange {
    pub fn new(min_z: f32, max_z: f32) -> Self {
        Self {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            min_z,
            max_z,
            overrides: BTreeMap::new(),
            group: String::new(),
        }
    }
    pub fn fresh(&self) -> Self {
        Self {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            ..self.clone()
        }
    }
    /// `5.0–10.0 mm`
    pub fn label(&self) -> String {
        format!("{:.1}–{:.1} mm", self.min_z, self.max_z)
    }
    pub fn config(&self) -> Config {
        config_from(&self.overrides)
    }
}

pub fn kind_label(kind: &PartKind) -> &'static str {
    match kind {
        PartKind::Model => "Part",
        PartKind::Modifier => "Modifier",
        PartKind::NegativeVolume => "Negative volume",
        PartKind::SupportEnforcer => "Support enforcer",
        PartKind::SupportBlocker => "Support blocker",
        PartKind::Unknown(_) => "Volume",
    }
}

/// A sparse config from text overrides; values the config refuses are skipped (the worker
/// reports such values for the object's own overrides).
pub fn config_from(overrides: &BTreeMap<String, String>) -> Config {
    let mut c = Config::new();
    for (k, v) in overrides {
        let _ = c.set_raw(k, v);
    }
    c
}

/// Text overrides from a sparse config (an imported modifier's settings).
pub fn overrides_from(config: &Config) -> BTreeMap<String, String> {
    let mut out: BTreeMap<String, String> = config
        .values()
        .filter_map(|(k, _)| config.raw(k).map(|v| (k.to_string(), v)))
        .collect();
    for (k, v) in config.extras() {
        out.entry(k.to_string()).or_insert_with(|| v.to_string());
    }
    out
}

/// A GUI transform as the 3MF writer stores it (row-major, f64), so that a saved project
/// places its meshes with exactly the arithmetic the editor slices with.
pub fn to_threemf(t: Mat4) -> mica_core::threemf::Transform {
    let mut out = mica_core::threemf::Transform::IDENTITY;
    for (c, col) in t.to_cols_array_2d().iter().enumerate() {
        for (r, v) in col.iter().take(3).enumerate() {
            out.0[r][c] = f64::from(*v);
        }
    }
    out
}

/// The mesh with `t` applied; winding flipped back when `t` mirrors. Vertices go through the
/// f64 transform the 3MF reader applies, so a project saved and reopened slices identically.
pub fn transform_mesh(mesh: &TriangleMesh, t: Mat4) -> TriangleMesh {
    let t64 = to_threemf(t);
    TriangleMesh {
        vertices: mesh.vertices.iter().map(|v| t64.apply(*v)).collect(),
        faces: if t.determinant() < 0. {
            mesh.faces.iter().map(|f| [f[0], f[2], f[1]]).collect()
        } else {
            mesh.faces.clone()
        },
    }
}

/// PrusaSlicer's model of an imported plain object (`mica_core::input::Frame`): the mesh as
/// loaded, in object coordinates, and the instance transform that placed it as the object's
/// `mesh`. The editor moves, turns and scales `mesh` as it always has, but the object slices
/// and saves as `frame.source` under the editor's transform composed with `frame.trafo`
/// ([`Object::model_frame`]), exactly what the CLI builds from the same file, or from the
/// project the editor saves. It describes `placed` only: replacing the mesh (repair,
/// simplify, a cut) leaves the object with its mesh as its own frame.
#[derive(Debug)]
pub struct ImportFrame {
    placed: Arc<TriangleMesh>,
    frame: mica_core::input::Frame,
}

impl ImportFrame {
    /// The frame of `placed`, the object's mesh (`placed == place(frame)`).
    pub fn new(placed: Arc<TriangleMesh>, frame: mica_core::input::Frame) -> Arc<Self> {
        Arc::new(Self { placed, frame })
    }
}

/// Which part of an object is being edited besides the object itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sub {
    Volume(u64),
    Range(u64),
}

#[derive(Clone)]
pub struct Object {
    pub id: u64,
    pub name: String,
    pub mesh: Arc<TriangleMesh>,
    pub local_bounds: ([f32; 3], [f32; 3]),
    pub position: Vec3,
    /// Rotation about Z, applied after `tilt`.
    pub angle: f32,
    /// Orientation from "lay on face" or free rotation, applied before `angle`.
    pub tilt: Quat,
    pub scale: Vec3,
    /// Mirror flags per axis (components are 1 or -1), applied with the scale.
    pub mirror: Vec3,
    pub tools: ToolAssignments,
    pub object_id: usize,
    pub instance_id: usize,
    /// Per-object setting overrides (PrusaSlicer option names → text values), applied on top
    /// of the plate's configuration when the object is prepared for slicing. Starts as the
    /// 3MF object's own settings.
    pub overrides: BTreeMap<String, String>,
    /// Parts of a 3MF object (name, kind) for the object tree; model parts are merged into `mesh`.
    pub parts: Vec<(String, String)>,
    /// Native source parts and f64 build placement, kept apart from the display mesh.
    pub source: Option<(
        Arc<mica_core::threemf::Object>,
        mica_core::threemf::Transform,
    )>,
    /// The **model** parts of a 3MF object (several volumes with their own settings and
    /// painting streams); `None` for a plain mesh. Modifiers, negative and support volumes of
    /// the import are converted into `volumes`, its layer ranges into `height_ranges`, so
    /// everything but the model parts is editable. `placed_region` assembles the engine's
    /// regions model from all of it.
    pub region: Option<Arc<mica_core::regions::RegionModel>>,
    /// Modifiers, negative volumes and support enforcers / blockers (editable).
    pub volumes: Vec<Volume>,
    /// Layer-height ranges with overrides (editable), sorted and non-overlapping.
    pub height_ranges: Vec<HeightRange>,
    /// Facet painting (supports, seams, colours, fuzzy skin) over `mesh`, mesh-local. Starts
    /// as the 3MF's own painting (the model volumes' streams, concatenated like the mesh) and
    /// is what the painting tools edit; `placed_region` hands it to the slicer.
    pub painting: Arc<FacetPainting>,
    /// Set once a tool edited `painting`, so an emptied painting still replaces the import.
    pub painting_edited: bool,
    /// What the mesh repair fixed (on import or on demand); the object list shows a warning
    /// badge with it. `None`: nothing was repaired.
    pub repair: Option<Arc<mica_core::mesh_repair::RepairReport>>,
    /// Variable layer height profile, `(z, height)` pairs from the object's bottom
    /// (`mica_core::layer_profile`); empty: the plate's layer height and the ranges.
    pub layer_profile: Vec<f64>,
    /// Hand-placed brim ears, mesh-local `[x, y, z, radius]` (used with a painted brim).
    pub brim_ears: Vec<[f32; 4]>,
    /// A text / SVG object's editable source (`crate::emboss`): the whole mesh is the text.
    pub emboss: Option<Arc<crate::emboss::Emboss>>,
    /// The model frame a plain object was imported with (see [`ImportFrame`]).
    pub frame: Option<Arc<ImportFrame>>,
    /// Exact world bounds of a tilted object for the transform they were measured with
    /// (shared by clones, which is harmless: the transform is the key).
    bounds_cache: Arc<std::sync::Mutex<Option<(Mat4, Vec3, Vec3)>>>,
    /// The project's mixed colors, set on the copies a slice job gets (`App::slice_job`);
    /// `None` on the plate's own objects.
    pub mixes: Option<Arc<mica_core::full_spectrum::FullSpectrum>>,
}
/// Objects compare by identity of their shared geometry and by value for everything the
/// editor changes, which is what undo snapshots need to decide whether a gesture did anything.
impl PartialEq for Object {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
            && self.name == other.name
            && Arc::ptr_eq(&self.mesh, &other.mesh)
            && self.position == other.position
            && self.angle == other.angle
            && self.tilt == other.tilt
            && self.scale == other.scale
            && self.mirror == other.mirror
            && self.tools.0 == other.tools.0
            && self.object_id == other.object_id
            && self.instance_id == other.instance_id
            && self.overrides == other.overrides
            && self.parts == other.parts
            && match (&self.region, &other.region) {
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                (None, None) => true,
                _ => false,
            }
            && self.volumes == other.volumes
            && self.height_ranges == other.height_ranges
            && (Arc::ptr_eq(&self.painting, &other.painting) || self.painting == other.painting)
            && self.painting_edited == other.painting_edited
            && self.layer_profile == other.layer_profile
            && self.brim_ears == other.brim_ears
            && self.emboss == other.emboss
            && match (&self.frame, &other.frame) {
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                (None, None) => true,
                _ => false,
            }
            && self.mixes == other.mixes
    }
}
/// What a slice job derives from an object's region model (a Settings › Features switch off).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Derived {
    WithoutTextures,
    WithoutBrickLayers,
}

/// `region` changed by `strip`, as the same `Arc` every time for the same `region`: slice
/// jobs compare regions by identity (`Object`'s `PartialEq`, the slice cache), and a fresh
/// copy per job made every slice look stale the moment it finished (GUI monkey test, seeds
/// 107 and 112: an imported part with Brick layers settings while the feature was off). The
/// entries keep their source alive, so an address is never reused for another region.
pub(crate) fn derived_region(
    kind: Derived,
    region: &Arc<mica_core::regions::RegionModel>,
    strip: impl FnOnce(&mut mica_core::regions::RegionModel),
) -> Arc<mica_core::regions::RegionModel> {
    type Entry = (
        Derived,
        Arc<mica_core::regions::RegionModel>,
        Arc<mica_core::regions::RegionModel>,
    );
    static MEMO: std::sync::Mutex<Vec<Entry>> = std::sync::Mutex::new(Vec::new());
    /// Regions with a derived copy kept (a few plates' worth of imported objects).
    const KEEP: usize = 32;
    let mut memo = MEMO.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(i) = memo
        .iter()
        .position(|(k, from, _)| *k == kind && Arc::ptr_eq(from, region))
    {
        let entry = memo.remove(i);
        let out = entry.2.clone();
        memo.push(entry);
        return out;
    }
    let mut copy = (**region).clone();
    strip(&mut copy);
    let out = Arc::new(copy);
    memo.push((kind, region.clone(), out.clone()));
    if memo.len() > KEEP {
        memo.remove(0);
    }
    out
}

impl Object {
    /// The fields in which `self` and `other` differ, as `PartialEq` compares them (the
    /// monkey's report of a stale slice).
    pub(crate) fn differences(&self, other: &Self) -> Vec<&'static str> {
        let opt_ptr = |a: Option<*const ()>, b: Option<*const ()>| a == b;
        let checks = [
            ("id", self.id == other.id),
            ("name", self.name == other.name),
            ("mesh", Arc::ptr_eq(&self.mesh, &other.mesh)),
            ("position", self.position == other.position),
            ("angle", self.angle == other.angle),
            ("tilt", self.tilt == other.tilt),
            ("scale", self.scale == other.scale),
            ("mirror", self.mirror == other.mirror),
            ("tools", self.tools.0 == other.tools.0),
            ("object_id", self.object_id == other.object_id),
            ("instance_id", self.instance_id == other.instance_id),
            ("overrides", self.overrides == other.overrides),
            ("parts", self.parts == other.parts),
            (
                "region",
                opt_ptr(
                    self.region.as_ref().map(|r| Arc::as_ptr(r).cast()),
                    other.region.as_ref().map(|r| Arc::as_ptr(r).cast()),
                ),
            ),
            ("volumes", self.volumes == other.volumes),
            ("height_ranges", self.height_ranges == other.height_ranges),
            (
                "painting",
                Arc::ptr_eq(&self.painting, &other.painting) || self.painting == other.painting,
            ),
            (
                "painting_edited",
                self.painting_edited == other.painting_edited,
            ),
            ("layer_profile", self.layer_profile == other.layer_profile),
            ("brim_ears", self.brim_ears == other.brim_ears),
            ("emboss", self.emboss == other.emboss),
            (
                "frame",
                opt_ptr(
                    self.frame.as_ref().map(|r| Arc::as_ptr(r).cast()),
                    other.frame.as_ref().map(|r| Arc::as_ptr(r).cast()),
                ),
            ),
            ("mixes", self.mixes == other.mixes),
        ];
        checks
            .into_iter()
            .filter(|(_, same)| !same)
            .map(|(name, _)| name)
            .collect()
    }
}

impl std::fmt::Debug for Object {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Object")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("position", &self.position)
            .field("angle", &self.angle)
            .field("tilt", &self.tilt)
            .field("scale", &self.scale)
            .field("mirror", &self.mirror)
            .finish_non_exhaustive()
    }
}
impl Object {
    /// Heap bytes of the shared buffers this object keeps alive (meshes, painting, the
    /// import's parts and regions), each counted once per `seen` set; for the undo history's
    /// memory bound. Small per-object fields are not counted.
    pub fn retained(&self, seen: &mut std::collections::HashSet<usize>) -> usize {
        fn once<T>(
            seen: &mut std::collections::HashSet<usize>,
            a: &Arc<T>,
            bytes: impl FnOnce(&T) -> usize,
        ) -> usize {
            if seen.insert(Arc::as_ptr(a) as *const () as usize) {
                bytes(a)
            } else {
                0
            }
        }
        let mut total = once(seen, &self.mesh, TriangleMesh::heap_bytes)
            + once(seen, &self.painting, FacetPainting::heap_bytes);
        for v in &self.volumes {
            total += once(seen, &v.mesh, TriangleMesh::heap_bytes);
        }
        if let Some((source, _)) = &self.source {
            total += once(seen, source, |s| {
                s.parts
                    .iter()
                    .map(|p| p.mesh.heap_bytes() + p.painting.heap_bytes())
                    .sum()
            });
        }
        if let Some(region) = &self.region {
            total += once(seen, region, |r| {
                r.volumes
                    .iter()
                    .map(|v| v.mesh.heap_bytes() + v.painting.heap_bytes())
                    .sum()
            });
        }
        total
    }

    pub fn new(mut o: mica_core::input::InputObject) -> Self {
        // Embossed text / SVG parts keep their source; embossed model parts beside other model
        // parts become editable volumes.
        let imported = crate::emboss::take_imported(&mut o);
        let painting = o
            .region
            .as_ref()
            .map(|r| Self::painting_of_volumes(r, o.mesh.faces.len()))
            .unwrap_or_default();
        // The import's modifiers, negative and support volumes become editable volumes and
        // its layer ranges editable ranges; the model parts stay in the regions model.
        let mut volumes = Vec::new();
        let mut height_ranges = Vec::new();
        let mut layer_profile = Vec::new();
        let mut brim_ears = Vec::new();
        let region = o.region.map(|mut r| {
            // The import's mesh is placed where its instance stands: ears in bed coordinates
            // are mesh-local until the object moves.
            layer_profile = std::mem::take(&mut r.layer_height_profile);
            brim_ears = std::mem::take(&mut r.brim_ears)
                .into_iter()
                .map(|e| [e.x as f32, e.y as f32, e.z as f32, e.radius as f32])
                .collect();
            let mut n = 0;
            for v in r.volumes.iter().filter(|v| v.kind != PartKind::Model) {
                n += 1;
                let name = o
                    .parts
                    .iter()
                    .filter(|(_, kind)| kind != "Model")
                    .nth(n - 1)
                    .map(|(name, _)| name.clone())
                    .unwrap_or_else(|| format!("{} {n}", kind_label(&v.kind)));
                let mut vol = Volume::new(name, v.kind.clone(), v.mesh.clone());
                vol.overrides = overrides_from(&v.config);
                vol.emboss = imported.volumes.get(n - 1).cloned().flatten();
                volumes.push(vol);
            }
            r.volumes.retain(|v| v.kind == PartKind::Model);
            for hr in r.height_ranges.drain(..) {
                let mut range = HeightRange::new(hr.min_z as f32, hr.max_z as f32);
                range.overrides = overrides_from(&hr.config);
                height_ranges.push(range);
            }
            r
        });
        // The tree's user groups from Mica's own metadata of a saved project.
        if let Some((source, _)) = &o.source {
            crate::tree::restore(source, &mut volumes, &mut height_ranges);
        }
        volumes.extend(imported.model_volumes);
        let mut parts = o.parts;
        parts.retain(|(_, kind)| kind == "Model");
        let mesh = Arc::new(o.mesh);
        // A text object regenerates its mesh from its source; it saves as its mesh.
        let frame = o
            .frame
            .filter(|_| imported.object.is_none())
            .map(|f| ImportFrame::new(mesh.clone(), f));
        Self {
            frame,
            painting: Arc::new(painting),
            painting_edited: false,
            layer_profile,
            brim_ears,
            local_bounds: mesh.bounds(),
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            name: o.name,
            mesh,
            position: Vec3::ZERO,
            angle: 0.,
            tilt: Quat::IDENTITY,
            scale: Vec3::ONE,
            mirror: Vec3::ONE,
            tools: crate::mixes::imported_tools(o.tools, &o.config, region.as_ref()),
            object_id: o.object_id,
            instance_id: o.instance_id,
            // Tool keys are carried by `tools`, not shown as settings.
            overrides: o
                .overrides
                .into_iter()
                .filter(|(k, _)| k != "extruder" && !k.ends_with("_extruder"))
                .collect(),
            parts,
            source: o.source,
            region: region.map(Arc::new),
            volumes,
            height_ranges,
            repair: None,
            emboss: imported.object,
            bounds_cache: Default::default(),
            mixes: None,
        }
    }

    /// A plain object from a mesh in its own coordinates (cut halves, split components).
    /// Settings, tools, volumes and ranges are copied from `like`; painting is not (the
    /// faces changed), nor are the ranges when `keep_ranges` is false.
    pub fn from_mesh(name: impl Into<String>, mesh: TriangleMesh, like: &Object) -> Self {
        Self {
            painting: Arc::new(FacetPainting::default()),
            painting_edited: false,
            layer_profile: vec![],
            brim_ears: vec![],
            local_bounds: mesh.bounds(),
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            name: name.into(),
            mesh: Arc::new(mesh),
            position: Vec3::ZERO,
            angle: 0.,
            tilt: Quat::IDENTITY,
            scale: Vec3::ONE,
            mirror: Vec3::ONE,
            tools: like.tools.clone(),
            object_id: like.object_id,
            instance_id: 0,
            overrides: like.overrides.clone(),
            parts: vec![],
            source: None,
            region: None,
            volumes: vec![],
            height_ranges: vec![],
            repair: None,
            emboss: None,
            frame: None,
            bounds_cache: Default::default(),
            mixes: None,
        }
    }

    /// Replaces the mesh (simplify, repair) keeping the object where it is: the pivot moves
    /// with the new local bounds, and the position makes up for it.
    pub fn set_mesh(&mut self, mesh: TriangleMesh) {
        let old = self.center();
        self.local_bounds = mesh.bounds();
        self.mesh = Arc::new(mesh);
        self.frame = None;
        self.bounds_cache = Default::default();
        let d = self.center() - old;
        let rs = Mat4::from_quat(self.rotation()) * Mat4::from_scale(self.scale * self.mirror);
        self.position += rs.transform_vector3(d) - d;
    }

    /// A copy with fresh ids (the object's and its volumes' and ranges'); meshes, the regions
    /// model and the painting stay shared. `object_id` is kept: an instance; callers give a
    /// duplicate a new one.
    pub fn fresh(&self) -> Self {
        Self {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            volumes: self.volumes.iter().map(Volume::fresh).collect(),
            height_ranges: self.height_ranges.iter().map(HeightRange::fresh).collect(),
            ..self.clone()
        }
    }

    /// Copies everything instances share (settings, painting, volumes and ranges) from
    /// `self` onto `other`, keeping `other`'s own transform and ids.
    pub fn sync_shared(&self, other: &mut Object) {
        other.overrides = self.overrides.clone();
        other.painting = self.painting.clone();
        other.painting_edited = self.painting_edited;
        other.tools = self.tools.clone();
        other.region = self.region.clone();
        other.parts = self.parts.clone();
        other.layer_profile = self.layer_profile.clone();
        other.brim_ears = self.brim_ears.clone();
        // Volumes and ranges keep their ids on the sibling when they match by position.
        other.volumes = self
            .volumes
            .iter()
            .enumerate()
            .map(|(i, v)| match other.volumes.get(i) {
                Some(mine) if mine.kind == v.kind => Volume {
                    id: mine.id,
                    ..v.clone()
                },
                _ => v.fresh(),
            })
            .collect();
        other.height_ranges = self
            .height_ranges
            .iter()
            .enumerate()
            .map(|(i, r)| match other.height_ranges.get(i) {
                Some(mine) => HeightRange {
                    id: mine.id,
                    ..r.clone()
                },
                None => r.fresh(),
            })
            .collect();
    }

    /// Prints the object's walls and infills with tool `t` (zero-based). The support roles
    /// stay as they are: "the current tool" (`None`) is what PrusaSlicer's wipe tower needs for
    /// non-soluble supports, and an explicit support tool is a setting of its own.
    pub fn set_tool(&mut self, t: usize) {
        for role in &mut self.tools.0[..3] {
            *role = Some(t);
        }
    }

    /// The volume with this id.
    pub fn volume(&self, id: u64) -> Option<&Volume> {
        self.volumes.iter().find(|v| v.id == id)
    }
    pub fn volume_mut(&mut self, id: u64) -> Option<&mut Volume> {
        self.volumes.iter_mut().find(|v| v.id == id)
    }
    pub fn range(&self, id: u64) -> Option<&HeightRange> {
        self.height_ranges.iter().find(|r| r.id == id)
    }
    pub fn range_mut(&mut self, id: u64) -> Option<&mut HeightRange> {
        self.height_ranges.iter_mut().find(|r| r.id == id)
    }
    /// World transform of a volume: the object's transform after the volume's local one.
    pub fn volume_transform(&self, v: &Volume) -> Mat4 {
        self.transform() * v.local_transform()
    }
    /// World bounds of a volume.
    pub fn volume_bounds(&self, v: &Volume) -> (Vec3, Vec3) {
        let t = self.volume_transform(v);
        let (a, b) = v.local_bounds;
        let mut min = Vec3::splat(f32::INFINITY);
        let mut max = -min;
        for x in [a[0], b[0]] {
            for y in [a[1], b[1]] {
                for z in [a[2], b[2]] {
                    let p = t.transform_point3(Vec3::new(x, y, z));
                    min = min.min(p);
                    max = max.max(p);
                }
            }
        }
        (min, max)
    }
    /// Applies a world-space delta to a volume: translations and rotation axes are taken into
    /// the object's frame, so the volume moves as the arrows show whatever the object's turn.
    pub fn apply_to_volume(&mut self, id: u64, delta: Delta) {
        let rot = self.rotation();
        let scale = self.scale * self.mirror;
        let Some(v) = self.volume_mut(id) else { return };
        match delta {
            Delta::Translate(d) => {
                let local = rot.inverse() * d;
                v.position += local / scale;
            }
            Delta::Rotate { axis, angle } => {
                let local_axis = (rot.inverse() * axis).normalize_or_zero();
                if local_axis != Vec3::ZERO {
                    v.rotation =
                        (Quat::from_axis_angle(local_axis, angle) * v.rotation).normalize();
                }
            }
            Delta::Scale(f) => v.scale = (v.scale * f).max(Vec3::splat(0.001)),
        }
    }
    /// Sorts the ranges and trims overlaps so the engine's "sorted, non-overlapping" rule
    /// holds; empty ranges are dropped.
    pub fn normalize_ranges(&mut self) {
        self.height_ranges
            .sort_by(|a, b| a.min_z.total_cmp(&b.min_z));
        let mut end = 0f32;
        for r in &mut self.height_ranges {
            r.min_z = r.min_z.max(end);
            end = r.max_z.max(r.min_z);
        }
        self.height_ranges.retain(|r| r.max_z > r.min_z + 1e-3);
    }
    /// Whether the object carries anything the engine's regions path needs.
    pub fn has_regions(&self) -> bool {
        self.region.is_some()
            || !self.volumes.is_empty()
            || !self.height_ranges.is_empty()
            || !self.layer_profile.is_empty()
            || !self.brim_ears.is_empty()
    }
    /// The 3MF's painting over the merged mesh: the model volumes' streams, shifted by the
    /// faces before them (the mesh is their concatenation). Streams with mixed windings
    /// cannot be joined and leave the object unpainted (the volumes keep theirs for slicing).
    fn painting_of_volumes(model: &mica_core::regions::RegionModel, faces: usize) -> FacetPainting {
        let mut out = FacetPainting::default();
        let mut offset = 0usize;
        for v in model.volumes.iter().filter(|v| v.kind == PartKind::Model) {
            if out.append_shifted(&v.painting, offset as u32).is_err() {
                return FacetPainting::default();
            }
            offset += v.mesh.faces.len();
        }
        if offset != faces {
            return FacetPainting::default();
        }
        out
    }

    /// Whether the slicer should see `painting` (something was painted or edited).
    pub(crate) fn paints(&self) -> bool {
        self.painting_edited || !self.painting.is_empty()
    }

    /// `painting` split back onto the model volumes by their face ranges, moved like the mesh.
    fn volume_painting(&self, first: usize, faces: usize, flip: bool) -> FacetPainting {
        // A 3MF may carry a model part without triangles: it has no painting to split off.
        if faces == 0 {
            return FacetPainting::default();
        }
        let mut p = self.painting.triangle_range(first, first + faces - 1);
        if flip {
            p.reverse_winding();
        }
        p
    }

    /// The regions model moved with the object (same transform as `placed_mesh`): the model
    /// parts (the 3MF's, or the mesh itself), then the editable volumes in archive order, and
    /// the height ranges in bed Z. `None` when the object has nothing the regions path needs
    /// (no parts, volumes, ranges or painting), so a plain mesh takes the plain path.
    pub fn placed_region(&self) -> Option<mica_core::regions::RegionModel> {
        // With mixed colors every object goes through the regions model, as the CLI does.
        let mixes = self.mixes.as_ref().filter(|m| !m.extruders().is_empty());
        if !self.has_regions() && !self.paints() && mixes.is_none() {
            return None;
        }
        let t = self.transform();
        let flip = t.determinant() < 0.;
        let mut volumes = Vec::new();
        match self.region.as_ref() {
            None => volumes.push(mica_core::regions::Volume {
                mesh: self.placed_mesh(),
                kind: PartKind::Model,
                config: Config::new(),
                painting: self.volume_painting(0, self.mesh.faces.len(), flip),
            }),
            Some(model) => {
                let mut first = 0usize;
                for v in &model.volumes {
                    let painting = if v.kind == PartKind::Model && self.paints() {
                        let p = self.volume_painting(first, v.mesh.faces.len(), flip);
                        first += v.mesh.faces.len();
                        p
                    } else {
                        let mut p = v.painting.clone();
                        if flip {
                            p.reverse_winding();
                        }
                        p
                    };
                    volumes.push(mica_core::regions::Volume {
                        mesh: transform_mesh(&v.mesh, t),
                        kind: v.kind.clone(),
                        config: v.config.clone(),
                        painting,
                    });
                }
            }
        }
        for v in &self.volumes {
            // Two steps (into the object's frame, then the object's placement): the same
            // arithmetic a saved project goes through, where the volume is a part in object
            // coordinates placed by the instance transform.
            volumes.push(mica_core::regions::Volume {
                mesh: transform_mesh(&transform_mesh(&v.mesh, v.local_transform()), t),
                kind: v.kind.clone(),
                config: v.config(),
                painting: FacetPainting::default(),
            });
        }
        let bottom = self.lowest_z() as f64;
        let mut ranges: Vec<HeightRange> = self.height_ranges.clone();
        ranges.sort_by(|a, b| a.min_z.total_cmp(&b.min_z));
        Some(mica_core::regions::RegionModel {
            full_spectrum: match &self.mixes {
                Some(m) => (**m).clone(),
                None => self
                    .region
                    .as_ref()
                    .map(|r| r.full_spectrum.clone())
                    .unwrap_or_default(),
            },
            volumes,
            height_ranges: ranges
                .iter()
                .filter(|r| r.max_z > r.min_z)
                .map(|r| mica_core::regions::HeightRange {
                    min_z: r.min_z as f64 + bottom,
                    max_z: r.max_z as f64 + bottom,
                    config: r.config(),
                })
                .collect(),
            layer_height_profile: self.layer_profile.clone(),
            brim_ears: self
                .brim_ears
                .iter()
                .map(|e| {
                    let p = t.transform_point3(Vec3::new(e[0], e[1], e[2]));
                    mica_core::brim::BrimEar {
                        x: f64::from(p.x),
                        y: f64::from(p.y),
                        z: f64::from(p.z),
                        radius: f64::from(e[3]),
                    }
                })
                .collect(),
        })
    }
    /// Local pivot: the untransformed bounding-box centre.
    pub fn center(&self) -> Vec3 {
        let (a, b) = self.local_bounds;
        (Vec3::from(a) + Vec3::from(b)) * 0.5
    }
    pub fn rotation(&self) -> Quat {
        Quat::from_rotation_z(self.angle) * self.tilt
    }
    pub fn transform(&self) -> Mat4 {
        let pivot = self.center();
        Mat4::from_translation(self.position + pivot)
            * Mat4::from_quat(self.rotation())
            * Mat4::from_scale(self.scale * self.mirror)
            * Mat4::from_translation(-pivot)
    }
    /// Euler angles (X, Y, Z, radians) of the world rotation, for editing in fields.
    pub fn euler(&self) -> Vec3 {
        let (z, y, x) = self.rotation().to_euler(glam::EulerRot::ZYX);
        Vec3::new(x, y, z)
    }
    /// Sets the world rotation from Euler angles (X, Y, Z, radians): Z stays the editable
    /// `angle`, X and Y fold into `tilt`.
    pub fn set_euler(&mut self, e: Vec3) {
        self.angle = e.z;
        self.tilt = Quat::from_euler(glam::EulerRot::ZYX, 0., e.y, e.x).normalize();
    }
    /// World position of the pivot (where gizmos attach).
    pub fn pivot_world(&self) -> Vec3 {
        self.position + self.center()
    }
    pub fn placed_mesh(&self) -> TriangleMesh {
        // Mirroring flips winding; `transform_mesh` keeps outward normals for the slicer.
        transform_mesh(&self.mesh, self.transform())
    }
    /// What prints: the placed mesh plus the editable model-part volumes (a raised text), so
    /// the layers reach the top of everything. Equal to `placed_mesh` without such volumes.
    pub fn print_mesh(&self) -> TriangleMesh {
        let parts: Vec<&Volume> = self
            .volumes
            .iter()
            .filter(|v| v.kind == PartKind::Model)
            .collect();
        let mesh = self.placed_mesh();
        if parts.is_empty() {
            return mesh;
        }
        let t = self.transform();
        let placed: Vec<TriangleMesh> = parts
            .iter()
            .map(|v| transform_mesh(&transform_mesh(&v.mesh, v.local_transform()), t))
            .collect();
        let mut all = vec![&mesh];
        all.extend(placed.iter());
        mica_core::mesh_edit::merge(&all)
    }
    /// Whether the object prints as a plain mesh, from its model frame: nothing the regions
    /// path needs, no painting ([`Self::placed_region`] is `None`).
    pub fn is_plain(&self) -> bool {
        !self.has_regions() && !self.paints()
    }

    /// The plain object's model frame: the import's mesh as loaded with the editor's transform
    /// composed onto the import's instance transform, or (without an import frame) the mesh
    /// under the editor's transform. What it slices from and what a saved project stores.
    pub fn model_frame(&self) -> mica_core::input::Frame {
        let editor = to_threemf(self.transform());
        match self
            .frame
            .as_ref()
            .filter(|f| Arc::ptr_eq(&f.placed, &self.mesh))
        {
            Some(f) => mica_core::input::Frame {
                source: f.frame.source.clone(),
                trafo: editor.compose(f.frame.trafo),
            },
            None => mica_core::input::Frame {
                source: self.mesh.clone(),
                trafo: editor,
            },
        }
    }

    /// The object as the print builder (`mica_core::input::build_print`) takes it, with
    /// `config` its own configuration (`crate::worker::object_config`): a plain object from
    /// its model frame, as the CLI loads a model, else the placed mesh with its regions model.
    pub fn slicing_input(&self, config: Config) -> mica_core::input::InputObject {
        use mica_core::input::InputObject;
        match self.placed_region() {
            // `placed_region` is `None` exactly for plain objects (`is_plain`).
            None => InputObject::from_frame(
                self.name.clone(),
                self.object_id,
                self.instance_id,
                self.model_frame(),
                self.tools.clone(),
                config,
            ),
            Some(region) => {
                // A mixed color's tool is its layer-0 physical tool, as the CLI passes it.
                let tools = mica_core::print_job::ToolAssignments(
                    self.tools
                        .0
                        .map(|t| t.map(|t| region.full_spectrum.resolve(t + 1, 0) - 1)),
                );
                let mut o = InputObject::plain(
                    self.name.clone(),
                    self.object_id,
                    self.instance_id,
                    self.print_mesh(),
                    tools,
                    config,
                );
                o.region = Some(region);
                o
            }
        }
    }

    /// XY convex hull of the placed mesh (bed coordinates), for arranging and spacing.
    pub fn footprint_hull(&self) -> Vec<[f64; 2]> {
        let t = self.transform();
        let points: Vec<[f64; 2]> = self
            .mesh
            .vertices
            .iter()
            .map(|v| {
                let p = t.transform_point3(Vec3::from(*v));
                [p.x as f64, p.y as f64]
            })
            .collect();
        hull2d(points)
    }
    /// World bounding box. Untilted objects use the transformed local box (exact in Z, which
    /// is what the bed checks need); a tilted object's box corners would dip below its real
    /// lowest point, so those are measured on the vertices and cached per transform.
    pub fn bounds(&self) -> (Vec3, Vec3) {
        let t = self.transform();
        if self.tilt.abs_diff_eq(Quat::IDENTITY, 1e-6) {
            let (a, b) = self.local_bounds;
            let mut min = Vec3::splat(f32::INFINITY);
            let mut max = -min;
            for x in [a[0], b[0]] {
                for y in [a[1], b[1]] {
                    for z in [a[2], b[2]] {
                        let p = t.transform_point3(Vec3::new(x, y, z));
                        min = min.min(p);
                        max = max.max(p);
                    }
                }
            }
            return (min, max);
        }
        if let Some((cached, min, max)) = *self.bounds_cache.lock().unwrap()
            && cached == t
        {
            return (min, max);
        }
        let mut min = Vec3::splat(f32::INFINITY);
        let mut max = -min;
        for v in &self.mesh.vertices {
            let p = t.transform_point3(Vec3::from(*v));
            min = min.min(p);
            max = max.max(p);
        }
        *self.bounds_cache.lock().unwrap() = Some((t, min, max));
        (min, max)
    }
    /// Radius of the footprint around the pivot: the largest XY distance of any transformed
    /// vertex from the vertical axis through `pivot_world`. Invariant under translation and
    /// rotation about Z, so a ring sized by it does not change while the object turns.
    pub fn footprint_radius(&self) -> f32 {
        let t = self.transform();
        let c = self.pivot_world().truncate();
        self.mesh
            .vertices
            .iter()
            .map(|v| {
                t.transform_point3(Vec3::from(*v))
                    .truncate()
                    .distance_squared(c)
            })
            .fold(0f32, f32::max)
            .sqrt()
    }
    /// Radius of the sphere around the pivot that holds the whole object: the largest distance
    /// of any transformed vertex from `pivot_world`. Invariant under any rotation about the
    /// pivot, so the tilt rings sized by it keep their size while the object turns.
    pub fn pivot_radius(&self) -> f32 {
        let t = self.transform();
        let c = self.pivot_world();
        self.mesh
            .vertices
            .iter()
            .map(|v| t.transform_point3(Vec3::from(*v)).distance_squared(c))
            .fold(0f32, f32::max)
            .sqrt()
    }
    /// Exact Z of the lowest transformed vertex (bounding boxes overestimate after tilting).
    pub fn lowest_z(&self) -> f32 {
        let t = to_threemf(self.transform());
        self.mesh
            .vertices
            .iter()
            .map(|v| t.apply(*v)[2])
            .fold(f32::INFINITY, f32::min)
    }
    /// Moves the object down (or up) so it rests on the bed plane.
    pub fn drop_to_bed(&mut self) {
        let z = self.lowest_z();
        if z.is_finite() {
            self.position.z -= z;
        }
    }
    pub fn apply(&mut self, delta: Delta) {
        match delta {
            Delta::Translate(d) => self.position += d,
            Delta::Rotate { axis, angle } => {
                if axis.abs_diff_eq(Vec3::Z, 1e-4) {
                    self.angle += angle;
                } else if axis.abs_diff_eq(-Vec3::Z, 1e-4) {
                    self.angle -= angle;
                } else {
                    // Fold into tilt while keeping `angle` as the editable Z rotation.
                    let z = Quat::from_rotation_z(self.angle);
                    self.tilt = z.inverse()
                        * Quat::from_axis_angle(axis.normalize(), angle)
                        * z
                        * self.tilt;
                }
                self.tilt = self.tilt.normalize();
                self.drop_to_bed();
            }
            Delta::Scale(f) => {
                self.scale = (self.scale * f).max(Vec3::splat(0.001));
                self.drop_to_bed();
            }
        }
    }
    /// Rotates the object so the face with world normal `normal` points down, then drops it.
    pub fn lay_on_face(&mut self, normal: Vec3) {
        let n = normal.normalize_or_zero();
        if n == Vec3::ZERO {
            return;
        }
        // New world rotation R' = arc · R with R = Rz(angle) · tilt; keep `angle` editable and
        // fold the change into tilt: tilt' = Rz⁻¹ · arc · Rz · tilt.
        let arc = Quat::from_rotation_arc(n, -Vec3::Z);
        let z = Quat::from_rotation_z(self.angle);
        self.tilt = (z.inverse() * arc * z * self.tilt).normalize();
        self.drop_to_bed();
    }
    /// World-space normal of a mesh face.
    pub fn face_normal(&self, face: usize) -> Vec3 {
        let f = self.mesh.faces[face];
        let t = self.transform();
        let [a, b, c] = f.map(|i| t.transform_point3(Vec3::from(self.mesh.vertices[i as usize])));
        (b - a).cross(c - a).normalize_or_zero()
    }
}

/// Lays object `index` on the face hit by a pick; convenience for the viewport.
pub fn lay_on_face(objects: &mut [Object], index: usize, face: usize) {
    if let Some(o) = objects.get_mut(index)
        && face < o.mesh.faces.len()
    {
        let n = o.face_normal(face);
        o.lay_on_face(n);
    }
}

/// Convex hull (monotone chain, counter-clockwise, no duplicates) of 2D points.
pub fn hull2d(mut points: Vec<[f64; 2]>) -> Vec<[f64; 2]> {
    points.sort_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
    points.dedup();
    if points.len() < 3 {
        return points;
    }
    let cross = |o: [f64; 2], a: [f64; 2], b: [f64; 2]| {
        (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
    };
    let mut lower: Vec<[f64; 2]> = Vec::new();
    for p in &points {
        while lower.len() >= 2 && cross(lower[lower.len() - 2], lower[lower.len() - 1], *p) <= 0. {
            lower.pop();
        }
        lower.push(*p);
    }
    let mut upper: Vec<[f64; 2]> = Vec::new();
    for p in points.iter().rev() {
        while upper.len() >= 2 && cross(upper[upper.len() - 2], upper[upper.len() - 1], *p) <= 0. {
            upper.pop();
        }
        upper.push(*p);
    }
    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

/// Whether two objects' XY bounding boxes overlap (with a gap of `spacing`).
fn boxes_touch(a: (Vec3, Vec3), b: (Vec3, Vec3), spacing: f32) -> bool {
    a.0.x < b.1.x + spacing
        && b.0.x < a.1.x + spacing
        && a.0.y < b.1.y + spacing
        && b.0.y < a.1.y + spacing
}

/// A free XY position for an object of footprint `size` near `near` (its bottom-left corner
/// would be at the result): spots are tried in rings around `near`, right first, then
/// behind, left and in front, growing outwards, and the first one that is inside the bed and
/// clear of every object in `taken` wins. When nothing is clear the spot to the right is
/// returned anyway (the user sees the overlap warning).
pub fn free_spot(taken: &[Object], size: Vec2, near: Vec2, bed: &Bed, spacing: f32) -> Vec2 {
    let boxes: Vec<(Vec3, Vec3)> = taken.iter().map(Object::bounds).collect();
    let inside = |p: Vec2| {
        p.x >= bed.min.x - 1e-3
            && p.y >= bed.min.y - 1e-3
            && p.x + size.x <= bed.max.x + 1e-3
            && p.y + size.y <= bed.max.y + 1e-3
    };
    let clear = |p: Vec2| {
        let b = (p.extend(0.), (p + size).extend(0.));
        !boxes.iter().any(|t| boxes_touch(*t, b, spacing))
    };
    let step = Vec2::new(size.x + spacing, size.y + spacing);
    let fallback = near + Vec2::new(step.x, 0.);
    for ring in 1..=12 {
        let r = ring as f32;
        let mut candidates = vec![];
        for k in 0..=ring {
            let kf = k as f32;
            candidates.push(near + Vec2::new(step.x * r, step.y * kf));
            candidates.push(near + Vec2::new(step.x * r, -step.y * kf));
            candidates.push(near + Vec2::new(step.x * kf, step.y * r));
            candidates.push(near + Vec2::new(-step.x * kf, step.y * r));
            candidates.push(near + Vec2::new(-step.x * r, step.y * kf));
            candidates.push(near + Vec2::new(-step.x * r, -step.y * kf));
            candidates.push(near + Vec2::new(step.x * kf, -step.y * r));
            candidates.push(near + Vec2::new(-step.x * kf, -step.y * r));
        }
        if let Some(p) = candidates.into_iter().find(|p| inside(*p) && clear(*p)) {
            return p;
        }
    }
    // Nothing near the original: anywhere on the bed, scanning from the front left.
    let mut y = bed.min.y + spacing;
    while y + size.y <= bed.max.y {
        let mut x = bed.min.x + spacing;
        while x + size.x <= bed.max.x {
            let p = Vec2::new(x, y);
            if clear(p) {
                return p;
            }
            x += 5.;
        }
        y += 5.;
    }
    fallback
}

/// Places `o` with its footprint's bottom-left at `at` (XY); Z is untouched.
pub fn move_footprint_to(o: &mut Object, at: Vec2) {
    let (min, _) = o.bounds();
    o.position += (at - min.truncate()).extend(0.);
}

/// Row placement used as the fallback when the packing arrange cannot run (empty bed
/// polygon); the real arrange lives in `mica_core::arrange` and `App::arrange`.
pub fn arrange_rows(objects: &mut [Object], bed: &Bed) {
    let mut cursor = bed.min + Vec3::new(10., 10., 0.);
    let mut row = 0.0_f32;
    for o in objects.iter_mut() {
        let (min, max) = o.bounds();
        let size = max - min;
        if cursor.x + size.x > bed.max.x - 10. && cursor.x > bed.min.x + 10. {
            cursor.x = bed.min.x + 10.;
            cursor.y += row + 10.;
            row = 0.;
        }
        o.position += (cursor - min).with_z(0.);
        cursor.x += size.x + 10.;
        row = row.max(size.y);
    }
    if !objects.is_empty() {
        let mut min = Vec3::splat(f32::INFINITY);
        let mut max = -min;
        for o in objects.iter() {
            let (a, b) = o.bounds();
            min = min.min(a);
            max = max.max(b);
        }
        let offset = (bed.center() - (min + max) * 0.5).with_z(0.);
        for o in objects {
            o.position += offset;
        }
    }
}

/// Filament colour per tool from the profile (`filament_colour`, then `extruder_colour`).
pub fn tool_colors(c: &Config) -> Vec<[f32; 3]> {
    let n = c.vector_len("nozzle_diameter").max(1);
    (0..n)
        .map(|i| {
            ["filament_colour", "extruder_colour"]
                .into_iter()
                .filter(|k| c.vector_len(k) > i)
                .filter_map(|k| c.string_at(k, i))
                .find_map(crate::theme::parse_hex)
                .unwrap_or_else(|| crate::preview::legend::tool_color(i))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mica_core::input::InputObject;
    fn wedge() -> Object {
        // Right triangular prism: faces include a slanted one with normal (1,0,1)/√2.
        let mesh = TriangleMesh {
            vertices: vec![
                [0., 0., 0.],
                [10., 0., 0.],
                [0., 0., 10.],
                [0., 5., 0.],
                [10., 5., 0.],
                [0., 5., 10.],
            ],
            faces: vec![
                [0, 2, 1],
                [3, 4, 5],
                [0, 1, 4],
                [0, 4, 3],
                [1, 5, 2],
                [1, 4, 5],
                [0, 3, 5],
                [0, 5, 2],
            ],
        };
        Object::new(InputObject::plain(
            "wedge",
            0,
            0,
            mesh,
            ToolAssignments([Some(0); 5]),
            Config::defaults(),
        ))
    }
    #[test]
    fn footprint_radius_ignores_rotation_about_z() {
        let mut o = wedge();
        o.position = Vec3::new(30., -12., 0.);
        let base = o.footprint_radius();
        assert!(base > 0.);
        for angle in [0.3, 1.1, 2.5, -0.7] {
            o.angle = angle;
            assert!((o.footprint_radius() - base).abs() < 1e-4, "{angle}");
            // The axis-aligned box does change with the angle, which is why it is not used.
            let (min, max) = o.bounds();
            assert!(max.x - min.x > 0.);
        }
        // Scaling does change it.
        o.scale = Vec3::splat(2.);
        assert!((o.footprint_radius() - 2. * base).abs() < 1e-4);
    }
    #[test]
    fn tilt_ring_deltas_turn_a_turned_object_about_world_axes() {
        // The X / Y rings are world-aligned (PrusaSlicer's World coordinates): their deltas
        // pre-multiply the object's rotation whatever it already is, and Z stays editable.
        let mut o = wedge();
        o.angle = 0.7;
        o.tilt = Quat::from_rotation_y(0.4);
        let before = o.rotation();
        o.apply(Delta::Rotate {
            axis: Vec3::X,
            angle: 0.5,
        });
        let want = Quat::from_rotation_x(0.5) * before;
        assert!(o.rotation().dot(want).abs() > 1. - 1e-5);
        assert_eq!(o.angle, 0.7);
    }
    #[test]
    fn pivot_radius_ignores_any_rotation() {
        let mut o = wedge();
        o.position = Vec3::new(30., -12., 4.);
        let base = o.pivot_radius();
        assert!(base >= o.footprint_radius() - 1e-4);
        for (angle, tilt) in [
            (0.3, Quat::from_rotation_x(0.8)),
            (1.1, Quat::from_rotation_y(-1.3)),
            (-0.7, Quat::from_euler(glam::EulerRot::ZYX, 0.2, 0.5, 1.4)),
        ] {
            o.angle = angle;
            o.tilt = tilt;
            assert!((o.pivot_radius() - base).abs() < 1e-3, "{angle}");
        }
    }
    #[test]
    fn placed_region_moves_with_the_object() {
        let mut o = wedge();
        let mut vol = Volume::new(
            "mod",
            mica_core::threemf::PartKind::Modifier,
            (*o.mesh).clone(),
        );
        vol.overrides.insert("fill_density".into(), "100%".into());
        o.volumes.push(vol);
        o.height_ranges.push(HeightRange::new(0., 5.));
        o.position = Vec3::new(10., 20., 2.);
        o.scale = Vec3::new(1., 1., 2.);
        let placed = o.placed_region().unwrap();
        let expected = o.placed_mesh();
        // Model part first (the mesh itself), then the modifier moved like the mesh.
        assert_eq!(placed.volumes.len(), 2);
        assert_eq!(placed.volumes[0].kind, mica_core::threemf::PartKind::Model);
        assert_eq!(placed.volumes[1].mesh.vertices, expected.vertices);
        assert_eq!(
            placed.volumes[1].config.raw("fill_density").as_deref(),
            Some("100%")
        );
        // Ranges are above the object's lowest point: scaling ×2 about the pivot (Z 5) and
        // moving up 2 puts the bottom at Z = −3.
        assert_eq!(o.lowest_z(), -3.);
        assert_eq!(placed.height_ranges[0].min_z, -3.);
        assert_eq!(placed.height_ranges[0].max_z, 2.);
        // A plain object has no regions model at all.
        assert!(wedge().placed_region().is_none());
    }

    /// An imported regions model splits into editable volumes / ranges and the model parts.
    #[test]
    fn imported_modifiers_become_editable_volumes() {
        let w = wedge();
        let mut model = mica_core::regions::RegionModel::default();
        model.volumes.push(mica_core::regions::Volume {
            painting: Default::default(),
            mesh: (*w.mesh).clone(),
            kind: PartKind::Model,
            config: Config::new(),
        });
        let mut c = Config::new();
        c.set_raw("perimeters", "4").unwrap();
        model.volumes.push(mica_core::regions::Volume {
            painting: Default::default(),
            mesh: (*w.mesh).clone(),
            kind: PartKind::Modifier,
            config: c,
        });
        let mut rc = Config::new();
        rc.set_raw("layer_height", "0.1").unwrap();
        model.height_ranges.push(mica_core::regions::HeightRange {
            min_z: 2.,
            max_z: 6.,
            config: rc,
        });
        let mut input = mica_core::input::InputObject::plain(
            "w",
            0,
            0,
            (*w.mesh).clone(),
            ToolAssignments([Some(0); 5]),
            Config::defaults(),
        );
        input.region = Some(model);
        input.parts = vec![
            ("body".into(), "Model".into()),
            ("dense".into(), "Modifier".into()),
        ];
        let o = Object::new(input);
        assert_eq!(o.volumes.len(), 1);
        assert_eq!(o.volumes[0].name, "dense");
        assert_eq!(
            o.volumes[0].overrides.get("perimeters").map(String::as_str),
            Some("4")
        );
        assert_eq!(o.height_ranges.len(), 1);
        assert_eq!(o.height_ranges[0].label(), "2.0–6.0 mm");
        assert_eq!(o.region.as_ref().unwrap().volumes.len(), 1);
        assert_eq!(o.parts, vec![("body".to_string(), "Model".to_string())]);
        let placed = o.placed_region().unwrap();
        assert_eq!(placed.volumes.len(), 2);
        assert_eq!(placed.volumes[1].kind, PartKind::Modifier);
        assert_eq!(
            placed.height_ranges[0]
                .config
                .raw("layer_height")
                .as_deref(),
            Some("0.1")
        );
    }

    /// Volume deltas arrive in world space and land in the object's frame.
    #[test]
    fn volume_deltas_follow_the_objects_frame() {
        let mut o = wedge();
        o.angle = std::f32::consts::FRAC_PI_2;
        let v = Volume::new("m", PartKind::Modifier, (*o.mesh).clone());
        let id = v.id;
        o.volumes.push(v);
        // World +X is local -Y for an object turned 90° CCW.
        o.apply_to_volume(id, Delta::Translate(Vec3::new(4., 0., 0.)));
        let p = o.volume(id).unwrap().position;
        assert!(
            (p - Vec3::new(0., -4., 0.)).abs().max_element() < 1e-4,
            "{p}"
        );
        // And the volume's world bounds moved by +4 in X.
        let (min, _) = o.volume_bounds(o.volume(id).unwrap());
        let (omin, _) = o.bounds();
        assert!((min.x - omin.x - 4.).abs() < 1e-3);
        o.apply_to_volume(id, Delta::Scale(Vec3::splat(2.)));
        assert_eq!(o.volume(id).unwrap().scale, Vec3::splat(2.));
    }

    #[test]
    fn ranges_normalize_and_instances_sync() {
        let mut o = wedge();
        o.height_ranges.push(HeightRange::new(6., 9.));
        o.height_ranges.push(HeightRange::new(0., 7.));
        o.height_ranges.push(HeightRange::new(9., 9.));
        o.normalize_ranges();
        assert_eq!(o.height_ranges.len(), 2);
        assert_eq!(
            (o.height_ranges[0].min_z, o.height_ranges[0].max_z),
            (0., 7.)
        );
        assert_eq!(
            (o.height_ranges[1].min_z, o.height_ranges[1].max_z),
            (7., 9.)
        );
        let mut twin = o.fresh();
        twin.position.x = 50.;
        assert_ne!(twin.id, o.id);
        assert!(Arc::ptr_eq(&twin.mesh, &o.mesh));
        o.overrides.insert("perimeters".into(), "5".into());
        o.height_ranges.push(HeightRange::new(9., 12.));
        let twin_range_id = twin.height_ranges[0].id;
        o.sync_shared(&mut twin);
        assert_eq!(
            twin.overrides.get("perimeters").map(String::as_str),
            Some("5")
        );
        assert_eq!(twin.height_ranges.len(), 3);
        assert_eq!(twin.height_ranges[0].id, twin_range_id);
        assert_eq!(twin.position.x, 50.);
    }

    #[test]
    fn free_spot_avoids_taken_boxes_and_stays_on_the_bed() {
        let bed = Bed::default();
        let mut a = wedge();
        move_footprint_to(&mut a, Vec2::new(100., 100.));
        let size = Vec2::new(10., 5.);
        let spot = free_spot(
            std::slice::from_ref(&a),
            size,
            Vec2::new(100., 100.),
            &bed,
            6.,
        );
        assert_eq!(spot, Vec2::new(116., 100.));
        // The spot to the right is taken: the next ring position is used.
        let mut b = wedge();
        move_footprint_to(&mut b, spot);
        let spot2 = free_spot(
            &[a.clone(), b.clone()],
            size,
            Vec2::new(100., 100.),
            &bed,
            6.,
        );
        assert!(spot2 != spot);
        for t in [&a, &b] {
            let (min, max) = t.bounds();
            assert!(!boxes_touch(
                (min, max),
                (spot2.extend(0.), (spot2 + size).extend(0.)),
                6.
            ));
        }
        // Near the bed's far edge the spot stays inside.
        let spot3 = free_spot(&[], size, Vec2::new(245., 100.), &bed, 6.);
        assert!(spot3.x + size.x <= bed.max.x + 1e-3);
    }
    #[test]
    fn euler_round_trip_and_mirror() {
        let mut o = wedge();
        o.set_euler(Vec3::new(0.3, -0.2, 1.1));
        let e = o.euler();
        assert!(
            (e - Vec3::new(0.3, -0.2, 1.1)).abs().max_element() < 1e-5,
            "{e}"
        );
        assert!((o.angle - 1.1).abs() < 1e-6);
        o.mirror.x = -1.;
        let placed = o.placed_mesh();
        // Mirrored output keeps the original winding sense (faces flipped back).
        assert_eq!(placed.faces[0], [0, 1, 2]);
        let (lo, hi) = o.bounds();
        assert!(hi.x > lo.x);
    }
    #[test]
    fn lay_on_face_puts_face_down_and_rests_on_bed() {
        let mut o = wedge();
        assert!(
            o.face_normal(4)
                .abs_diff_eq(Vec3::new(1., 0., 1.).normalize(), 1e-5)
        );
        lay_on_face(std::slice::from_mut(&mut o), 0, 4);
        assert!(o.face_normal(4).abs_diff_eq(-Vec3::Z, 1e-4));
        assert!(o.lowest_z().abs() < 1e-4);
        // Laying on an already-down face is a no-op orientation-wise.
        let before = o.rotation();
        let n = o.face_normal(4);
        o.lay_on_face(n);
        assert!(o.rotation().abs_diff_eq(before, 1e-4));
    }
    #[test]
    fn deltas_compose() {
        let mut o = wedge();
        o.apply(Delta::Translate(Vec3::new(5., 0., 0.)));
        assert_eq!(o.position, Vec3::new(5., 0., 0.));
        o.apply(Delta::Rotate {
            axis: Vec3::Z,
            angle: 0.5,
        });
        assert!((o.angle - 0.5).abs() < 1e-6 && o.tilt == Quat::IDENTITY);
        o.apply(Delta::Scale(Vec3::splat(2.)));
        assert_eq!(o.scale, Vec3::splat(2.));
        let (min, max) = o.bounds();
        assert!((max.z - min.z - 20.).abs() < 1e-3 && min.z.abs() < 1e-3);
        o.apply(Delta::Rotate {
            axis: Vec3::X,
            angle: std::f32::consts::FRAC_PI_2,
        });
        assert!(o.lowest_z().abs() < 1e-3);
        assert!((o.angle - 0.5).abs() < 1e-6);
    }
    #[test]
    fn colours_from_profile() {
        let mut c = Config::defaults();
        c.set_raw("nozzle_diameter", "0.4,0.4,0.4").unwrap();
        c.set_raw("filament_colour", "#FF0000;#00FF00;").unwrap();
        c.set_raw("extruder_colour", ";;#0000FF").unwrap();
        let colors = tool_colors(&c);
        assert_eq!(colors.len(), 3);
        assert_eq!(colors[0], [1., 0., 0.]);
        assert_eq!(colors[1], [0., 1., 0.]);
        assert_eq!(colors[2], [0., 0., 1.]);
    }
}
