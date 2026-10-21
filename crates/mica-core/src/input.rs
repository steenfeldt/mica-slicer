// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/PrintObjectSlice.cpp :: slices_to_regions
// Changes: Mica's own loader (original); plain objects keep PrusaSlicer's model frame (`Frame`: the volume in
// Changes: object coordinates and the instance transform), and a one-volume 3MF object slices as a plain object,
// Changes: as `slices_to_regions` hands a single model volume's slices to its region unchanged.
// Changes: Loaded::mica_settings carries the project sidecar's Mica options.

//! The one model loader for the CLI and the GUI: STL and 3MF geometry with placement, tool
//! assignments and the regions model (modifiers, negative and support volumes, height ranges,
//! per-object/part settings). Loading does not slice; `InputObject::prepare` does, so a GUI can
//! edit placement and overrides first. Diagnostics come back as warnings instead of being
//! printed, and dialect-specific settings mapping (Orca) is supplied by the caller, because the
//! engine only knows PrusaSlicer's option names.

mod build;
mod plates;
pub use build::{PlatePrint, PreparedPlate, Preparing, build_print, prepare_plate};
pub use plates::{PlateSelection, grid_origin, nearest_plate};

use crate::config::{Config, option_defs};
use crate::mesh::TriangleMesh;
use crate::print_job::{PrintObject, ToolAssignments};
use crate::regions::{HeightRange, RegionModel, Volume, apply_config};
use crate::threemf::{PartKind, Settings, Transform, load_3mf};
use rayon::prelude::*;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

/// Most tools a lenient load (`SettingsMapper::lenient_tools`) keeps an assignment to.
pub const LENIENT_TOOLS: usize = 64;

/// Maps a project's raw object, part or height-range settings onto a sparse canonical patch.
/// `orca` says whether the project came from Orca/Snapmaker (their option names) or
/// PrusaSlicer. Keys that cannot be applied go to `ignored` (reported, never dropped silently).
pub trait SettingsMapper {
    /// Per-bed tower placements, including source-specific project settings. These coordinates
    /// are already bed-local and must not receive the model's multi-plate grid translation.
    fn tower_placements(
        &self,
        project: &crate::threemf::Project,
    ) -> Result<BTreeMap<u32, [f64; 3]>, String> {
        Ok(project.wipe_towers.clone())
    }

    /// Original archive bed dimensions used for Orca's global plate grid. A different
    /// selected printer must not change the source placement. None uses the selected bed.
    fn plate_dimensions(&self, _project: &crate::threemf::Project) -> Option<(f64, f64)> {
        None
    }

    /// How many tools the archive's own project settings use. When that exceeds the selected
    /// printer's count, tool assignments are resolved against it instead of failing, so an
    /// editor can open the project and offer the project's printer. None keeps the strict
    /// check (the CLI).
    fn tool_count(&self, _project: &crate::threemf::Project) -> Option<usize> {
        None
    }

    /// Whether tool assignments beyond the selected printer's tools are kept instead of
    /// failing the load (up to [`LENIENT_TOOLS`]). An editor keeps them, as it does after a
    /// printer switch, and clamps them when slicing; the CLI keeps the strict check.
    fn lenient_tools(&self) -> bool {
        false
    }

    fn patch(
        &self,
        raw: &Settings,
        orca: bool,
        ignored: &mut BTreeSet<String>,
    ) -> Result<Config, String>;
}

/// PrusaSlicer option names only; Orca names are reported as unknown.
pub struct PrusaMapper;
impl SettingsMapper for PrusaMapper {
    fn patch(
        &self,
        raw: &Settings,
        _orca: bool,
        ignored: &mut BTreeSet<String>,
    ) -> Result<Config, String> {
        prusa_patch(raw, ignored)
    }
}

/// Sparse patch from PrusaSlicer-named settings.
pub fn prusa_patch(raw: &Settings, ignored: &mut BTreeSet<String>) -> Result<Config, String> {
    let mut config = Config::new();
    for (key, value) in raw {
        if !option_defs().contains_key(key.as_str()) && key != "extruder" {
            ignored.insert(format!("unknown setting {key}"));
        }
        config.set_raw(key, value).map_err(|e| format!("3MF {e}"))?;
    }
    Ok(config)
}

pub struct Loaded {
    pub objects: Vec<InputObject>,
    pub warnings: Vec<String>,
    pub tower_placement: Option<[f64; 3]>,
    /// The archives' explicit plates, in archive order (several projects concatenate);
    /// `InputObject::plate` indexes this. Empty without plate metadata.
    pub plates: Vec<LoadedPlate>,
    /// The first archive's embedded printer/process/filament settings, unparsed.
    pub config: Option<crate::threemf::RawConfig>,
    /// That archive's Mica options kept in Mica's sidecar (`mica_slicing_rules` of an Orca
    /// project).
    pub mica_settings: Settings,
    /// Per-layer custom G-code (colour changes, pauses…) of the selected plate (the first
    /// archive's plate 1 when none is selected).
    pub custom_gcode: crate::custom_gcode::Info,
    /// The first project's mixed colors (ColorMix) and their file text, if it has any.
    pub full_spectrum: Option<(crate::full_spectrum::FullSpectrum, String)>,
}

/// One plate of a project, without its objects.
#[derive(Clone, Debug, PartialEq)]
pub struct LoadedPlate {
    /// The archive's own name for the plate; empty when unnamed.
    pub name: String,
    /// Orca's `locked` flag.
    pub locked: bool,
    /// The project's bed-local tower placement, if any.
    pub tower: Option<[f64; 3]>,
    /// The plate's raw metadata (Orca's `filament_maps` and friends), not applied.
    pub metadata: Settings,
    /// The plate's per-layer custom G-code.
    pub custom_gcode: crate::custom_gcode::Info,
}

/// Geometry, assignments and settings of one printable instance, not yet sliced.
pub struct InputObject {
    pub name: String,
    pub object_id: usize,
    pub instance_id: usize,
    /// Combined model volumes, placed on the bed.
    pub mesh: TriangleMesh,
    pub tools: ToolAssignments,
    /// Base configuration with the object's own settings and tool keys applied.
    pub config: Config,
    /// The object's sparse settings (PrusaSlicer names → text), for display and editing.
    pub overrides: Settings,
    /// Volumes, modifiers and height ranges of a 3MF object; `None` for plain meshes.
    pub region: Option<RegionModel>,
    /// 3MF parts (name, kind), for display. Empty for STL input.
    pub parts: Vec<(String, String)>,
    /// Editable source geometry and the bed-local build transform, before f32 placement.
    /// Shared between instances; a GUI can retain this for a native project save.
    pub source: Option<(std::sync::Arc<crate::threemf::Object>, Transform)>,
    /// Index into `Loaded::plates` of the plate this instance belongs to; `None` for STL
    /// input and archives without plate metadata.
    pub plate: Option<usize>,
    /// A plain object's model: its mesh in object coordinates and the instance transform,
    /// with `mesh == place(frame)`. PrusaSlicer slices the object in this frame (and computes
    /// aligned-seam visibility in it); `None` for objects with a regions model.
    pub frame: Option<Frame>,
}

/// PrusaSlicer's model of a plain object: one model volume in object coordinates (an STL as
/// loaded, a 3MF object's mesh) and the `ModelInstance` transform that puts it on the bed.
/// The CLI and the GUI both slice a plain object from its frame ([`place`]); the GUI composes
/// its own edits onto `trafo`, and a saved project stores exactly `source` and `trafo`.
#[derive(Clone, Debug)]
pub struct Frame {
    pub source: std::sync::Arc<TriangleMesh>,
    pub trafo: Transform,
}

impl Frame {
    /// The frame moved by `d`: an `f32` instance offset stays an `f32` sum (as an STL's
    /// offset is computed), any other transform composes in f64.
    pub fn translated(&self, d: [f32; 3]) -> Self {
        let linear_identity =
            (0..3).all(|r| (0..3).all(|c| self.trafo.0[r][c] == f64::from(u8::from(r == c))));
        let offset: [f64; 3] = std::array::from_fn(|r| self.trafo.0[r][3]);
        let trafo = if linear_identity && offset.iter().all(|&v| f64::from(v as f32) == v) {
            translation(std::array::from_fn(|r| offset[r] as f32 + d[r]))
        } else {
            translation(d).compose(self.trafo)
        };
        Self {
            source: self.source.clone(),
            trafo,
        }
    }
}

/// The frame's mesh placed on the bed: every vertex through the f64 transform once (as the 3MF
/// reader places its objects), winding kept outward under a mirroring transform. A pure
/// translation by `f32` offsets gives exactly the `f32` sums [`TriangleMesh::translate`] does.
pub fn place(frame: &Frame) -> TriangleMesh {
    let mut out = TriangleMesh::default();
    append(&mut out, &frame.source, frame.trafo);
    out
}

/// Where a model is placed when nothing else says (`--center` in the CLI): the centre of the
/// bed's bounding box, computed in f64 from `bed_shape` (a 250 × 220 mm bed without one). The
/// CLI and the GUI place STLs by this one rule.
pub fn bed_center(c: &Config) -> (f32, f32) {
    let bed = c
        .points("bed_shape")
        .filter(|p| p.len() >= 3 && p.iter().all(|(x, y)| x.is_finite() && y.is_finite()))
        .unwrap_or_else(|| vec![(0., 0.), (250., 0.), (250., 220.), (0., 220.)]);
    let mid = |f: fn(&(f64, f64)) -> f64| {
        ((bed.iter().map(f).fold(f64::INFINITY, f64::min)
            + bed.iter().map(f).fold(f64::NEG_INFINITY, f64::max))
            / 2.0) as f32
    };
    (mid(|p| p.0), mid(|p| p.1))
}

/// A translation by `f32` offsets, as a transform.
pub fn translation(d: [f32; 3]) -> Transform {
    let mut t = Transform::IDENTITY;
    for (row, v) in t.0.iter_mut().zip(d) {
        row[3] = f64::from(v);
    }
    t
}

impl InputObject {
    /// A plain mesh with the given configuration (STL input, tests).
    pub fn plain(
        name: impl Into<String>,
        object_id: usize,
        instance_id: usize,
        mesh: TriangleMesh,
        tools: ToolAssignments,
        config: Config,
    ) -> Self {
        Self {
            name: name.into(),
            object_id,
            instance_id,
            mesh,
            tools,
            config,
            overrides: Settings::new(),
            region: None,
            parts: Vec::new(),
            source: None,
            plate: None,
            frame: None,
        }
    }

    /// A plain object from its model frame, placed by [`place`].
    pub fn from_frame(
        name: impl Into<String>,
        object_id: usize,
        instance_id: usize,
        frame: Frame,
        tools: ToolAssignments,
        config: Config,
    ) -> Self {
        let mut o = Self::plain(name, object_id, instance_id, place(&frame), tools, config);
        o.frame = Some(frame);
        o
    }

    /// Slices the object: with its regions model when it has one.
    pub fn prepare(self) -> Result<PrintObject, String> {
        self.prepare_observed(&crate::progress::Silent)
    }

    /// [`Self::prepare`] reporting the stages to `observer`, which may cancel it.
    pub fn prepare_observed(
        self,
        observer: &dyn crate::progress::Observer,
    ) -> Result<PrintObject, String> {
        match self.region {
            Some(model) => PrintObject::prepare_regions_observed(
                self.name,
                self.object_id,
                self.instance_id,
                self.mesh,
                &model,
                self.tools,
                &self.config,
                observer,
            ),
            None => PrintObject::prepare_in_frame_observed(
                self.name,
                self.object_id,
                self.instance_id,
                self.mesh,
                self.tools,
                &self.config,
                self.frame.as_ref(),
                observer,
            ),
        }
    }
}

impl InputObject {
    /// What decides whether this instance can share another's prepared object.
    pub fn candidate(&self) -> crate::instances::Candidate<'_> {
        crate::instances::Candidate {
            mesh: &self.mesh,
            region: self.region.as_ref(),
            tools: &self.tools,
            config: &self.config,
        }
    }
}

/// Prepare independent instances in the shared Rayon pool. Indexed collection retains input
/// order (including deterministic error selection); nested layer jobs use the same workers.
/// Instances that differ from an earlier one only by an XY translation are not prepared
/// again: they share its prepared object at their shift ([`crate::instances::group`]).
pub fn prepare_objects(objects: Vec<InputObject>) -> Result<Vec<PrintObject>, String> {
    let groups = {
        let candidates: Vec<_> = objects.iter().map(InputObject::candidate).collect();
        crate::instances::group(&candidates)
    };
    let count = objects.len();
    let (leaders, copies): (Vec<_>, Vec<_>) = objects
        .into_iter()
        .zip(groups)
        .enumerate()
        .partition(|(i, (_, (leader, _)))| leader == i);
    let mut out: Vec<Option<PrintObject>> = (0..count).map(|_| None).collect();
    for (i, prepared) in leaders
        .into_par_iter()
        .map(|(i, (o, _))| o.prepare().map(|p| (i, p)))
        .collect::<Result<Vec<_>, _>>()?
    {
        out[i] = Some(prepared);
    }
    // A copy's leader comes before it, so it is already prepared.
    for (i, (o, (leader, shift))) in copies {
        let first = out[leader].as_mut().expect("leaders precede their copies");
        out[i] = Some(first.instance(o.name, o.object_id, o.instance_id, o.mesh, shift));
    }
    Ok(out.into_iter().flatten().collect())
}

/// Append transformed indexed geometry, preserving winding under mirrored transforms.
pub fn append(out: &mut TriangleMesh, mesh: &TriangleMesh, transform: Transform) {
    let base = out.vertices.len() as u32;
    out.vertices
        .extend(mesh.vertices.iter().map(|p| transform.apply(*p)));
    out.faces.extend(mesh.faces.iter().map(|f| {
        let mut f = f.map(|i| i + base);
        if transform.determinant() < 0.0 {
            f.swap(1, 2);
        }
        f
    }));
}

/// Text form of a sparse patch, for display.
fn raw_settings(patch: &Config) -> Settings {
    let mut out = Settings::new();
    for (k, _) in patch.values() {
        if let Some(raw) = patch.raw(k) {
            out.insert(k.to_string(), raw);
        }
    }
    for (k, v) in patch.extras() {
        out.insert(k.to_string(), v.to_string());
    }
    out
}

/// Loads printable model parts with tool assignments and region models, without slicing.
/// `overrides` uses one-based object/tool numbers; `default` is a zero-based physical tool.
/// STL inputs rest on the bed; 3MF build placements are retained unless explicitly centered.
#[allow(clippy::too_many_arguments)]
pub fn load(
    paths: &[PathBuf],
    center: (f32, f32),
    explicit_center: bool,
    all_objects: bool,
    default: usize,
    overrides: &BTreeMap<usize, usize>,
    c: &Config,
    mapper: &dyn SettingsMapper,
) -> Result<Loaded, String> {
    load_with_plate(
        paths,
        center,
        explicit_center,
        all_objects,
        default,
        overrides,
        c,
        mapper,
        PlateSelection::All,
    )
}

/// [`load`] with explicit plate selection; the legacy API continues to load all plates.
#[allow(clippy::too_many_arguments)]
pub fn load_with_plate(
    paths: &[PathBuf],
    center: (f32, f32),
    explicit_center: bool,
    all_objects: bool,
    default: usize,
    overrides: &BTreeMap<usize, usize>,
    c: &Config,
    mapper: &dyn SettingsMapper,
    plate: PlateSelection,
) -> Result<Loaded, String> {
    let _timing = crate::timing::Span::new(crate::timing::Stage::Load);
    let mut warnings = Vec::new();
    let mut tower_placement = None;
    let mut loaded_plates: Vec<LoadedPlate> = Vec::new();
    let mut project_config = None;
    let mut project_mica = Settings::new();
    let mut project_spectrum = None;
    let mut custom_gcode: Option<crate::custom_gcode::Info> = None;
    let count = c.vector_len("nozzle_diameter").max(1);
    if default >= count {
        return Err(format!(
            "tool {} exceeds printer's {count} tools",
            default + 1
        ));
    }
    // The count tool assignments are checked against (see `SettingsMapper::lenient_tools`).
    let resolve_count = |count: usize| {
        if mapper.lenient_tools() {
            count.max(LENIENT_TOOLS)
        } else {
            count
        }
    };
    let mut inputs: Vec<InputObject> = Vec::new();
    let mut object_id = 0;
    let mut stl_x = 0.0;
    let only_stl = paths
        .iter()
        .all(|p| !p.extension().is_some_and(|s| s.eq_ignore_ascii_case("3mf")));
    for path in paths {
        if !path
            .extension()
            .is_some_and(|s| s.eq_ignore_ascii_case("3mf"))
        {
            if let PlateSelection::Plate(n) = plate
                && n != 1
            {
                return Err(format!("plate {n} is outside 1..=1 for STL input"));
            }
            let loaded =
                TriangleMesh::load_stl(path).map_err(|e| format!("{}: {e}", path.display()))?;
            // The instance offset: the bounding-box centre onto `center`, resting on the bed;
            // several STLs side by side, 10 mm apart.
            let (min, max) = loaded.bounds();
            let mut offset = [
                center.0 - (min[0] + max[0]) / 2.0,
                center.1 - (min[1] + max[1]) / 2.0,
                -min[2],
            ];
            if only_stl && paths.len() > 1 {
                let (min, max) = (min[0] + offset[0], max[0] + offset[0]);
                offset[0] += stl_x - min;
                stl_x += max - min + 10.;
            }
            let frame = Frame {
                source: std::sync::Arc::new(loaded),
                trafo: translation(offset),
            };
            let mut settings = Settings::new();
            if let Some(n) = overrides.get(&(object_id + 1)) {
                settings.insert("extruder".into(), n.to_string());
            }
            let tools = ToolAssignments::resolve(
                c,
                &settings,
                &Settings::new(),
                default,
                resolve_count(count),
            )?;
            inputs.push(InputObject::from_frame(
                path.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                object_id,
                0,
                frame,
                tools,
                c.clone(),
            ));
            object_id += 1;
            continue;
        }
        let mut project = load_3mf(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let dimensions = mapper.plate_dimensions(&project);
        let selected = plates::select(&mut project, plate, c, dimensions)?;
        let towers = mapper.tower_placements(&project)?;
        let count = match mapper.tool_count(&project) {
            Some(n) if n > count => {
                warnings.push(format!(
                    "{}: the project uses {n} tools, the selected printer has {count}",
                    path.file_name().unwrap_or_default().to_string_lossy()
                ));
                n
            }
            _ => count,
        };
        let plate_base = loaded_plates.len();
        loaded_plates.extend(project.plates.iter().enumerate().map(|(i, p)| {
            LoadedPlate {
                name: p.name.clone(),
                locked: p
                    .metadata
                    .get("locked")
                    .is_some_and(|v| v == "true" || v == "1"),
                tower: towers.get(&(i as u32)).copied(),
                metadata: p.metadata.clone(),
                custom_gcode: project
                    .custom_gcodes
                    .get(&(i as u32))
                    .cloned()
                    .unwrap_or_default(),
            }
        }));
        if custom_gcode.is_none() {
            custom_gcode = Some(
                project
                    .custom_gcodes
                    .get(&(selected.unwrap_or(0) as u32))
                    .cloned()
                    .unwrap_or_default(),
            );
        }
        if project_config.is_none() {
            project_config = project.config.clone();
            project_mica = project.mica_settings.clone();
        }
        if let Some(&p) = towers.get(&(selected.unwrap_or(0) as u32)) {
            if tower_placement.is_some_and(|old| old != p) {
                return Err("input projects have conflicting wipe tower placements".into());
            }
            tower_placement = Some(p);
        }
        let orca = project
            .config
            .as_ref()
            .is_some_and(|c| c.path.ends_with("project_settings.config"))
            || project.objects.iter().any(|o| {
                o.settings.contains_key("wall_loops")
                    || o.parts.iter().any(|p| p.metadata.contains_key("subtype"))
            });
        let spectrum = project
            .full_spectrum_json
            .as_deref()
            .map(|text| crate::full_spectrum::FullSpectrum::from_json(text, count))
            .transpose()?
            .unwrap_or_default();
        if project_spectrum.is_none()
            && let Some(text) = &project.full_spectrum_json
        {
            project_spectrum = Some((spectrum.clone(), text.clone()));
        }
        let mut ignored = project.ignored_features.clone();
        if project.plates.len() > 1 && selected.is_none() && plate != PlateSelection::Each {
            ignored.insert(format!(
                "multiple plates ({}): stored global coordinates retained",
                project.plates.len()
            ));
        }
        let mut selected_objects = 0;
        for object in &project.objects {
            if !object.instances.iter().any(|i| i.printable)
                || (!all_objects && selected_objects > 0)
            {
                object_id += 1;
                continue;
            }
            selected_objects += 1;
            let mut settings = object.settings.clone();
            let override_tool = overrides.get(&(object_id + 1));
            if let Some(n) = override_tool {
                settings.insert("extruder".into(), n.to_string());
            }
            let parts: Vec<(String, String)> = object
                .parts
                .iter()
                .map(|p| (p.name.clone(), format!("{:?}", p.kind)))
                .collect();
            let source = std::sync::Arc::new(object.clone());
            // One model part and nothing else (no part settings, painting, ranges, layer
            // profile or ears) is a plain object, as an STL is: PrusaSlicer's ModelObject with
            // one volume, sliced in its frame by the same path.
            let plain_source = match object.parts.as_slice() {
                [part]
                    if part.kind == PartKind::Model
                        && part.settings.is_empty()
                        && part.painting.is_empty()
                        && object.height_ranges.is_empty()
                        && object.layer_height_profile.is_empty()
                        && object.brim_points.is_empty()
                        && spectrum.extruders().is_empty() =>
                {
                    Some(std::sync::Arc::new(part.mesh.clone()))
                }
                _ => None,
            };
            for (ii, instance) in object
                .instances
                .iter()
                .enumerate()
                .filter(|(_, i)| i.printable)
            {
                let object_patch = mapper.patch(&settings, orca, &mut ignored)?;
                let mut config = apply_config(c, &object_patch);
                let tools = ToolAssignments::resolve_with_virtuals(
                    &config,
                    &settings,
                    &Settings::new(),
                    default,
                    resolve_count(count),
                    &spectrum,
                )?;
                for (key, tool) in [
                    "perimeter_extruder",
                    "infill_extruder",
                    "solid_infill_extruder",
                    "support_material_extruder",
                    "support_material_interface_extruder",
                ]
                .into_iter()
                .zip(tools.0)
                {
                    config
                        .set_raw(key, &tool.map_or(0, |t| t + 1).to_string())
                        .map_err(|e| e.to_string())?;
                }
                let tools =
                    ToolAssignments(tools.0.map(|t| t.map(|t| spectrum.resolve(t + 1, 0) - 1)));
                let mut model = RegionModel {
                    full_spectrum: spectrum.clone(),
                    ..RegionModel::default()
                };
                let mut mesh = TriangleMesh::default();
                for part in &object.parts {
                    if let PartKind::Unknown(kind) = &part.kind {
                        ignored.insert(format!("unknown part type {kind}"));
                        continue;
                    }
                    let mut part_settings = part.settings.clone();
                    if let Some(n) = override_tool {
                        part_settings.insert("extruder".into(), n.to_string());
                    }
                    let patch = mapper.patch(&part_settings, orca, &mut ignored)?;
                    let mut placed = TriangleMesh::default();
                    append(&mut placed, &part.mesh, instance.transform);
                    if part.kind == PartKind::Model {
                        append(&mut mesh, &placed, Transform::IDENTITY);
                    }
                    let mut painting = part.painting.clone();
                    if instance.transform.determinant() < 0. {
                        painting.reverse_winding();
                    }
                    model.volumes.push(Volume {
                        painting,
                        mesh: placed,
                        kind: part.kind.clone(),
                        config: patch,
                    });
                }
                for range in &object.height_ranges {
                    model.height_ranges.push(HeightRange {
                        min_z: range.min_z,
                        max_z: range.max_z,
                        config: mapper.patch(&range.settings, orca, &mut ignored)?,
                    });
                }
                model.layer_height_profile = object.layer_height_profile.clone();
                // Orca places ears with the instance's full transform (`BrimPoint::transform`),
                // in the frame of the object recentred on its bounding box.
                let origin = object.brim_frame_origin();
                model.brim_ears = object
                    .brim_points
                    .iter()
                    .map(|point| {
                        let t = instance.transform.0;
                        let p = [0, 1, 2].map(|a| f64::from(point[a]) + origin[a]);
                        let at = |r: [f64; 4]| r[0] * p[0] + r[1] * p[1] + r[2] * p[2] + r[3];
                        crate::brim::BrimEar {
                            x: at(t[0]),
                            y: at(t[1]),
                            z: f64::from(at(t[2]) as f32),
                            radius: f64::from(point[3]),
                        }
                    })
                    .collect();
                if !mesh.faces.is_empty() {
                    let plate = instance.plate.and_then(|id| {
                        project
                            .plates
                            .iter()
                            .position(|p| p.id == id)
                            .map(|i| i + plate_base)
                    });
                    // The object's mesh in object coordinates under its instance transform,
                    // wherever it stands: identical objects slice identically, moved.
                    let frame = plain_source.as_ref().map(|source| Frame {
                        source: source.clone(),
                        trafo: instance.transform,
                    });
                    inputs.push(InputObject {
                        name: object.name.clone(),
                        object_id,
                        instance_id: ii,
                        mesh,
                        tools,
                        config,
                        overrides: raw_settings(&object_patch),
                        region: plain_source.is_none().then_some(model),
                        parts: parts.clone(),
                        source: Some((source.clone(), instance.transform)),
                        plate,
                        frame,
                    });
                }
            }
            object_id += 1;
        }
        if !ignored.is_empty() {
            warnings.push(format!(
                "Parts of this project Mica doesn't use yet were skipped: {}",
                ignored.into_iter().collect::<Vec<_>>().join("; ")
            ));
        }
    }
    if let Some(i) = overrides.keys().find(|&&i| i == 0 || i > object_id) {
        return Err(format!(
            "--object-extruder object {i} is outside 1..={object_id}"
        ));
    }
    if inputs.is_empty() {
        return Err("no printable model parts".into());
    }
    if explicit_center || (only_stl && paths.len() > 1) {
        let mut mesh = TriangleMesh::default();
        for o in &inputs {
            append(&mut mesh, &o.mesh, Transform::IDENTITY);
        }
        let (before, _) = mesh.bounds();
        mesh.place_on_bed(center.0, center.1);
        let (after, _) = mesh.bounds();
        let d = std::array::from_fn(|i| after[i] - before[i]);
        for o in &mut inputs {
            // A plain object moves in its frame, placed again in one step.
            if let Some(frame) = &mut o.frame {
                *frame = frame.translated(d);
                o.mesh = place(frame);
            } else {
                o.mesh.translate(d);
            }
            // Explicit centering has edited the baked geometry.
            o.source = None;
            if let Some(model) = &mut o.region {
                for volume in &mut model.volumes {
                    volume.mesh.translate(d);
                }
                for ear in &mut model.brim_ears {
                    ear.x += f64::from(d[0]);
                    ear.y += f64::from(d[1]);
                    ear.z += f64::from(d[2]);
                }
            }
        }
    }
    Ok(Loaded {
        objects: inputs,
        warnings,
        tower_placement,
        plates: loaded_plates,
        config: project_config,
        mica_settings: project_mica,
        custom_gcode: custom_gcode.unwrap_or_default(),
        full_spectrum: project_spectrum,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parallel_objects_preserve_identity_and_scheduled_paths() {
        use crate::print::LayerSource;
        let config = Config::defaults();
        let make = || {
            (0..2)
                .map(|id| {
                    let mut mesh = TriangleMesh::load_stl(
                        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                            .join("../../tests/models/cube20.stl"),
                    )
                    .unwrap();
                    for p in &mut mesh.vertices {
                        for v in p {
                            *v *= 0.2;
                        }
                    }
                    mesh.place_on_bed(20. + id as f32 * 10., 20.);
                    let tools =
                        ToolAssignments::resolve(&config, &Settings::new(), &Settings::new(), 0, 1)
                            .unwrap();
                    let mut input = InputObject::plain(
                        format!("cube {id}"),
                        id,
                        id + 3,
                        mesh.clone(),
                        tools,
                        config.clone(),
                    );
                    if id == 1 {
                        input.region = Some(RegionModel {
                            volumes: vec![Volume {
                                mesh,
                                kind: PartKind::Model,
                                config: Config::new(),
                                painting: Default::default(),
                            }],
                            ..Default::default()
                        });
                    }
                    input
                })
                .collect()
        };
        let prepare = |threads| {
            rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .build()
                .unwrap()
                .install(|| {
                    let objects = prepare_objects(make()).unwrap();
                    for (i, o) in objects.iter().enumerate() {
                        assert_eq!((o.object_id, o.instance_id), (i, i + 3));
                        assert_eq!(o.name, format!("cube {i}"));
                    }
                    crate::print_job::PreparedPrint::new(objects, &config).unwrap()
                })
        };
        let a = prepare(1);
        let b = prepare(4);
        assert_eq!(a.layer_count(), b.layer_count());
        let a = a.toolpaths(0..a.layer_count());
        let b = b.toolpaths(0..b.layer_count());
        for (a, b) in a.iter().zip(&b) {
            assert_eq!(a.paths, b.paths);
        }
    }

    #[test]
    fn translated_instances_share_one_preparation() {
        use crate::geometry::unscale;
        use crate::print::LayerSource;
        use std::sync::Arc;
        let config = Config::defaults();
        let tools =
            ToolAssignments::resolve(&config, &Settings::new(), &Settings::new(), 0, 1).unwrap();
        let cube = |x: f32, y: f32, instance: usize, angle: f32| {
            let mut mesh = TriangleMesh::load_stl(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../tests/models/cube20.stl"),
            )
            .unwrap();
            let (sin, cos) = angle.to_radians().sin_cos();
            for p in &mut mesh.vertices {
                *p = [
                    0.5 * (p[0] * cos - p[1] * sin),
                    0.5 * (p[0] * sin + p[1] * cos),
                    0.5 * p[2],
                ];
            }
            mesh.place_on_bed(x, y);
            InputObject::plain("cube", 0, instance, mesh, tools.clone(), config.clone())
        };
        let objects = prepare_objects(vec![
            cube(40., 40., 0, 0.),
            cube(71.3, 52.9, 1, 0.),
            cube(40., 90., 2, 30.),
        ])
        .unwrap();
        assert!(Arc::ptr_eq(&objects[0].prepared, &objects[1].prepared));
        assert!(!Arc::ptr_eq(&objects[0].prepared, &objects[2].prepared));
        let shift = objects[1].shift;
        assert!((unscale(shift.x) - 31.3).abs() < 1e-4);
        assert!((unscale(shift.y) - 12.9).abs() < 1e-4);
        assert_eq!(objects[2].shift, crate::geometry::Point::default());
        let print = crate::print_job::PreparedPrint::new(objects, &config).unwrap();
        for layer in print.scheduled(0..print.layer_count()) {
            let of = |object: usize| -> Vec<_> {
                layer
                    .layer
                    .paths
                    .iter()
                    .zip(&layer.assignments)
                    // The plate's skirt and brim are assigned to an owner object.
                    .filter(|(p, a)| {
                        a.object == object
                            && !matches!(
                                p.role,
                                crate::toolpath::Role::Skirt | crate::toolpath::Role::Brim
                            )
                    })
                    .map(|(p, _)| p.clone())
                    .collect()
            };
            let mut moved = of(0);
            assert!(!moved.is_empty());
            for path in &mut moved {
                crate::instances::shift_path(path, shift);
            }
            assert_eq!(moved, of(1));
        }
        let outlines = print.layer_outlines(3).unwrap();
        let first = print.objects[0].prepared.layer_outlines(3).unwrap();
        assert_eq!(
            &outlines[first.len()..2 * first.len()],
            crate::instances::shifted_polygons(first, shift).as_slice()
        );
    }

    /// Every plain object's mesh is its model frame placed, bit for bit (what the GUI rebuilds
    /// from the frame after an edit); one-volume 3MF objects are plain, painted ones are not;
    /// 3MF objects keep their mesh in object coordinates.
    #[test]
    fn plain_objects_are_their_model_frame_placed() {
        let models = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/models");
        let mut config = Config::defaults();
        config
            .set_raw("nozzle_diameter", "0.4,0.4,0.4,0.4,0.4")
            .unwrap();
        let cases: [(&[&str], bool, PlateSelection); 5] = [
            (&["cube20.stl"], false, PlateSelection::All),
            (
                &["cube20.stl", "hole_block.stl"],
                false,
                PlateSelection::All,
            ),
            (&["3DBenchy.stl"], true, PlateSelection::All),
            (&["plates-orca.3mf"], false, PlateSelection::Each),
            (
                &["plates-orca.3mf", "painted-cube.3mf"],
                true,
                PlateSelection::All,
            ),
        ];
        for (files, centred, plate) in cases {
            let paths: Vec<PathBuf> = files.iter().map(|f| models.join(f)).collect();
            let loaded = load_with_plate(
                &paths,
                (101.3, 87.7),
                centred,
                true,
                0,
                &BTreeMap::new(),
                &config,
                &PrusaMapper,
                plate,
            )
            .unwrap();
            for o in &loaded.objects {
                let Some(frame) = &o.frame else {
                    assert!(o.region.is_some(), "{files:?} {}: no frame", o.name);
                    continue;
                };
                assert!(o.region.is_none());
                let placed = place(frame);
                assert_eq!(placed.faces, o.mesh.faces, "{files:?} {}", o.name);
                let bits = |m: &TriangleMesh| -> Vec<[u32; 3]> {
                    m.vertices.iter().map(|v| v.map(f32::to_bits)).collect()
                };
                assert_eq!(bits(&placed), bits(&o.mesh), "{files:?} {}", o.name);
            }
            if files.contains(&"painted-cube.3mf") {
                assert!(
                    loaded.objects.iter().any(|o| o.region.is_some()),
                    "a painted object keeps its regions model"
                );
            }
        }
    }

    /// PrusaSlicer slices every object in its own frame, so identical objects print identically
    /// wherever they stand: the same model prepared on its own at two bed positions (not shared
    /// as instances) gives the same paths on every layer, moved by the difference of their
    /// centres. Seams, walls and infill included.
    #[test]
    fn the_same_model_at_two_positions_prints_the_same_moved() {
        use crate::print::LayerSource;
        let models = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/models");
        let config = Config::defaults();
        for model in ["hole_block.stl", "thin_features.stl"] {
            let source = std::sync::Arc::new(TriangleMesh::load_stl(models.join(model)).unwrap());
            let (min, max) = source.bounds();
            let at = |x: f32, y: f32| {
                let frame = Frame {
                    source: source.clone(),
                    trafo: translation([
                        x - (min[0] + max[0]) / 2.0,
                        y - (min[1] + max[1]) / 2.0,
                        -min[2],
                    ]),
                };
                let tools =
                    ToolAssignments::resolve(&config, &Settings::new(), &Settings::new(), 0, 1)
                        .unwrap();
                InputObject::from_frame(model, 0, 0, frame, tools, config.clone())
                    .prepare()
                    .unwrap()
            };
            let a = at(61.337, 72.913);
            let b = at(158.071, 131.4419);
            let centre = |o: &PrintObject| {
                let (min, max) = o.mesh.bounds();
                crate::geometry::Point::from_mm(
                    (f64::from(min[0]) + f64::from(max[0])) / 2.0,
                    (f64::from(min[1]) + f64::from(max[1])) / 2.0,
                )
            };
            let (ca, cb) = (centre(&a), centre(&b));
            let shift = crate::geometry::Point::new(cb.x - ca.x, cb.y - ca.y);
            let n = a.prepared.layer_count();
            assert_eq!(n, b.prepared.layer_count(), "{model}");
            let (pa, pb) = (a.prepared.toolpaths(0..n), b.prepared.toolpaths(0..n));
            for (i, (la, lb)) in pa.iter().zip(&pb).enumerate() {
                let mut moved = la.clone();
                crate::instances::shift_layer(&mut moved, shift);
                assert_eq!(moved.paths, lb.paths, "{model}: layer {}", i + 1);
            }
        }
    }

    /// Three separate objects with the same mesh in a project (not instances of one object, so
    /// each is prepared on its own) print the same paths, moved, as PrusaSlicer prints them.
    #[test]
    fn identical_project_objects_print_the_same_moved() {
        use crate::print::LayerSource;
        use crate::threemf::{Flavour, Instance, Object, Part, Project, ResourceId};
        let models = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/models");
        let mesh = TriangleMesh::load_stl(models.join("hole_block.stl")).unwrap();
        let (min, _) = mesh.bounds();
        let object = |i: u32, x: f64, y: f64| Object {
            name: format!("block {i}"),
            source: ResourceId {
                path: "3D/3dmodel.model".into(),
                id: i,
            },
            parts: vec![Part {
                name: format!("block {i}"),
                kind: PartKind::Model,
                source: ResourceId {
                    path: "3D/3dmodel.model".into(),
                    id: i,
                },
                mesh: mesh.clone(),
                source_mesh: None,
                painting: Default::default(),
                transform: Transform::IDENTITY,
                settings: Settings::new(),
                metadata: Settings::new(),
            }],
            instances: vec![Instance {
                transform: Transform([
                    [1., 0., 0., x],
                    [0., 1., 0., y],
                    [0., 0., 1., -f64::from(min[2])],
                ]),
                printable: true,
                plate: None,
            }],
            settings: Settings::new(),
            metadata: Settings::new(),
            height_ranges: vec![],
            layer_height_profile: vec![],
            brim_points: vec![],
        };
        let dir = std::env::temp_dir().join(format!("mica-identical-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("three.3mf");
        let project = Project {
            objects: vec![
                object(1, 40.123, 50.77),
                object(2, 111.9, 52.031),
                object(3, 77.25, 140.6),
            ],
            ..Default::default()
        };
        crate::threemf::write(&path, &project, Flavour::Prusa).unwrap();
        let config = Config::defaults();
        let loaded = load(
            std::slice::from_ref(&path),
            (125., 110.),
            false,
            true,
            0,
            &BTreeMap::new(),
            &config,
            &PrusaMapper,
        )
        .unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(loaded.objects.len(), 3);
        assert!(loaded.objects.iter().all(|o| o.frame.is_some()));
        let prepared: Vec<PrintObject> = loaded
            .objects
            .into_iter()
            .map(|o| o.prepare().unwrap())
            .collect();
        let centre = |o: &PrintObject| {
            let (min, max) = o.mesh.bounds();
            crate::geometry::Point::from_mm(
                (f64::from(min[0]) + f64::from(max[0])) / 2.0,
                (f64::from(min[1]) + f64::from(max[1])) / 2.0,
            )
        };
        let n = prepared[0].prepared.layer_count();
        let first = prepared[0].prepared.toolpaths(0..n);
        for other in &prepared[1..] {
            let (ca, cb) = (centre(&prepared[0]), centre(other));
            let shift = crate::geometry::Point::new(cb.x - ca.x, cb.y - ca.y);
            let paths = other.prepared.toolpaths(0..n);
            for (i, (la, lb)) in first.iter().zip(&paths).enumerate() {
                let mut moved = la.clone();
                crate::instances::shift_layer(&mut moved, shift);
                assert_eq!(moved.paths, lb.paths, "{}: layer {}", other.name, i + 1);
            }
        }
    }

    #[test]
    fn prusa_patch_reports_unknown_keys() {
        let mut raw = Settings::new();
        raw.insert("perimeters".into(), "4".into());
        raw.insert("wall_loops".into(), "3".into());
        raw.insert("extruder".into(), "2".into());
        let mut ignored = BTreeSet::new();
        let patch = PrusaMapper.patch(&raw, false, &mut ignored).unwrap();
        assert_eq!(patch.int("perimeters"), Some(4));
        assert_eq!(patch.raw("extruder").as_deref(), Some("2"));
        assert_eq!(
            ignored.into_iter().collect::<Vec<_>>(),
            ["unknown setting wall_loops"]
        );
        let shown = raw_settings(&patch);
        assert_eq!(shown["perimeters"], "4");
        assert_eq!(shown["wall_loops"], "3");
    }
}
