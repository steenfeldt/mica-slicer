// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: orca@8500fcdcca src/libslic3r/PrintConfig.cpp :: DynamicPrintConfig::normalize_fdm_2
// Source: orca@8500fcdcca src/libslic3r/Print.cpp :: Print::extruders, Print::support_material_extruders
// Changes: Only the independent support layer height is normalized here; the tower itself is decided by the print job.

//! The one way the CLI and the GUI turn a plate's objects and its configuration into a print:
//! placement checks, tool clamping, instance sharing, parallel preparation, the single-object
//! or plate source the exporter reads, and the combined mesh it takes its thumbnails and
//! object hulls from. The CLI calls it on what [`super::load_with_plate`] returns; the GUI on
//! the objects it edits (`mica-gui`'s worker), so the same objects and configuration give the
//! same G-code byte for byte (`mica-gui/src/cli_parity_tests.rs`).

use super::InputObject;
use crate::config::Config;
use crate::custom_gcode::{Info, WithCustomGCode};
use crate::geometry::{BoundingBox, ExPolygon, Point};
use crate::mesh::TriangleMesh;
use crate::print::{LayerSource, PathEmission, PreparedObject};
use crate::print_job::{PreparedPrint, PrintObject, ScheduledLayer};
use crate::progress::Observer;
use crate::threemf::Transform;
use crate::toolpath::LayerToolpaths;
use rayon::prelude::*;

/// A plate ready to export.
pub struct PlatePrint {
    source: Source,
    /// The job's name, the first object's (an STL's object is named after its file): the
    /// exporter labels a single object with it.
    pub name: String,
    /// Every object's placed model mesh, appended: thumbnails, Klipper object hulls and the
    /// binary G-code's object list come from it.
    pub mesh: TriangleMesh,
    /// Problems that do not stop the print (placement, open contours, a collision notice).
    pub warnings: Vec<String>,
    /// The warnings a front end can know before slicing, from the objects' placement alone (an
    /// object below the bed, a skirt or brim past its edge): the GUI checks those live, in its
    /// own words and with its own actions, and leaves them out of the slice's warnings.
    pub preflight: Vec<String>,
    /// Informational lines: tree support statistics, where the custom G-code lands.
    pub notes: Vec<String>,
}

enum Source {
    /// One plain object on one tool: the object's own source, as the CLI has always exported
    /// it, with the plate's per-layer custom G-code attached when there is any.
    Single(Box<PreparedObject>, Info),
    Plate(Box<PreparedPrint>),
}

/// Which object [`build_print`] is about to prepare, for a progress observer: `index` of `count`
/// prepared objects (instances that only move share one preparation and are not counted).
pub struct Preparing<'a> {
    pub index: usize,
    pub count: usize,
    pub object: &'a InputObject,
}

/// A plate's objects, prepared ([`prepare_plate`]), before the plate's custom G-code is placed
/// on them ([`PreparedPlate::finish`]).
pub struct PreparedPlate {
    /// The prepared objects in input order (the custom G-code's tool count reads them).
    pub objects: Vec<PrintObject>,
    mesh: TriangleMesh,
    name: String,
    warnings: Vec<String>,
    preflight: Vec<String>,
    notes: Vec<String>,
}

/// [`prepare_plate`] and [`PreparedPlate::finish`] in one go.
pub fn build_print<'o>(
    objects: Vec<InputObject>,
    config: &Config,
    custom: Info,
    observe: &(dyn Fn(Preparing<'_>) -> Box<dyn Observer + 'o> + Sync),
) -> Result<PlatePrint, String> {
    prepare_plate(objects, config, observe)?.finish(config, custom)
}

/// Prepares `objects` for `config`, the plate configuration. `observe` hands out the progress
/// observer (which may cancel) for each prepared object.
pub fn prepare_plate<'o>(
    objects: Vec<InputObject>,
    config: &Config,
    observe: &(dyn Fn(Preparing<'_>) -> Box<dyn Observer + 'o> + Sync),
) -> Result<PreparedPlate, String> {
    if objects.is_empty() {
        return Err("no printable model parts".into());
    }
    // Before any geometry work: the exporter would refuse the print at its start
    // (`mica_gcode::export`, same check and words).
    if let Some(problem) = crate::safety::extrusion_mode_problem(config) {
        return Err(problem);
    }
    // Tools beyond the printer's count print with its last tool.
    let count = config.vector_len("nozzle_diameter").max(1);
    let objects: Vec<InputObject> = objects
        .into_iter()
        .map(|mut o| {
            for tool in o.tools.0.iter_mut().flatten() {
                *tool = (*tool).min(count - 1);
            }
            o
        })
        .collect();
    let mut objects = objects;
    normalize_orca_tower(&mut objects, config);
    let mut warnings = Vec::new();
    let mut preflight = Vec::new();
    let mut notes = Vec::new();
    // The placement preflight: an object outside the build volume (scaled past the bed, too
    // tall, off the plate) is refused before any geometry work.
    for o in &objects {
        let found =
            crate::safety::placement(&o.mesh, &o.config).map_err(|e| format!("{}: {e}", o.name))?;
        for warning in found {
            let warning = format!("{}: {warning}", o.name);
            if warning.contains(crate::safety::BELOW_BED) {
                preflight.push(warning.clone());
            }
            warnings.push(warning);
        }
        // A skirt or brim that would not fit on the bed: the export refuses it later
        // (`PreparedPlate::finish`); say so now, as upstream warns before slicing.
        if let Some(overrun) =
            crate::adhesion_check::preflight(&o.mesh, &|v| v.map(f64::from), &o.config)
        {
            let warning = crate::adhesion_check::explain(&o.name, &overrun, true);
            preflight.push(warning.clone());
            warnings.push(warning);
        }
    }
    let name = objects[0].name.clone();
    let mut mesh = TriangleMesh::default();
    for o in &objects {
        super::append(&mut mesh, &o.mesh, Transform::IDENTITY);
    }
    // Instances that differ from an earlier one only by an XY translation share its prepared
    // object at their shift ([`crate::instances::group`]).
    let groups = {
        let candidates: Vec<_> = objects.iter().map(InputObject::candidate).collect();
        crate::instances::group(&candidates)
    };
    let total = objects.len();
    let (leaders, copies): (Vec<_>, Vec<_>) = objects
        .into_iter()
        .zip(groups)
        .enumerate()
        .partition(|(i, (_, (leader, _)))| leader == i);
    let prepared_count = leaders.len();
    let mut out: Vec<Option<PrintObject>> = (0..total).map(|_| None).collect();
    for (i, prepared) in leaders
        .into_par_iter()
        .enumerate()
        .map(|(k, (i, (o, _)))| {
            let observer = observe(Preparing {
                index: k,
                count: prepared_count,
                object: &o,
            });
            crate::progress::check(observer.as_ref())?;
            o.prepare_observed(observer.as_ref()).map(|p| (i, p))
        })
        .collect::<Result<Vec<_>, _>>()?
    {
        out[i] = Some(prepared);
    }
    // A copy's leader comes before it, so it is already prepared.
    for (i, (o, (leader, shift))) in copies {
        let first = out[leader].as_mut().expect("leaders precede their copies");
        out[i] = Some(first.instance(o.name, o.object_id, o.instance_id, o.mesh, shift));
    }
    let objects: Vec<PrintObject> = out.into_iter().flatten().collect();
    for object in &objects {
        if object.prepared.report.dropped_chains > 0 {
            warnings.push(format!(
                "{}: {} open contour(s) dropped; the mesh may not be watertight",
                object.name, object.prepared.report.dropped_chains
            ));
        }
        if let Some(stats) = object.prepared.tree_support_statistics() {
            notes.push(format!(
                "tree supports: {}: {}/{} tips connected, {} bed trunks, {} model trunks",
                object.name, stats.supported_tips, stats.tips, stats.bed_trunks, stats.model_trunks
            ));
        }
    }
    Ok(PreparedPlate {
        objects,
        mesh,
        name,
        warnings,
        preflight,
        notes,
    })
}

/// OrcaSlicer's `DynamicPrintConfig::normalize_fdm_2`, applied when the print is set up
/// (`Print::apply`): a prime tower that stays on (several used filaments, or a smooth timelapse
/// or wrapping detection, and not one object after another) turns
/// `independent_support_layer_height` off, so the supports print on the object layers.
/// The used filaments are `Print::extruders`: the objects' tools and their support tools.
fn normalize_orca_tower(objects: &mut [InputObject], config: &Config) {
    if !crate::rules::Rules::of(config).is_orca() || !config.bool("wipe_tower").unwrap_or(false) {
        return;
    }
    let mut used = Vec::new();
    for o in objects.iter() {
        used.extend(crate::print_job::object_extruders(
            &o.tools,
            o.region.as_ref(),
            &o.config,
        ));
        if o.config.bool("support_material").unwrap_or(false)
            || o.config.int("raft_layers").unwrap_or(0) > 0
        {
            used.extend(o.tools.0[3..].iter().flatten());
        }
    }
    used.sort_unstable();
    used.dedup();
    let raw_on = |key: &str| {
        config
            .raw(key)
            .is_some_and(|v| matches!(v.trim(), "1" | "true"))
    };
    let keep = raw_on("timelapse_type") || raw_on("enable_wrapping_detection");
    let mut ids: Vec<usize> = objects.iter().map(|o| o.object_id).collect();
    ids.sort_unstable();
    ids.dedup();
    let by_object = crate::sequential::enabled(config) && ids.len() > 1;
    if !keep && (used.len() == 1 || by_object) {
        return;
    }
    const KEY: &str = "independent_support_layer_height";
    for o in objects.iter_mut() {
        // An imported Orca option keeps its typed value next to the raw text.
        if o.config.get(KEY).is_some() {
            o.config.set(
                KEY,
                crate::config::ConfigValue::Scalar(crate::config::Value::Bool(false)),
            );
        }
        o.config.set_raw(KEY, "0").expect("boolean option");
    }
}

impl PreparedPlate {
    /// Problems found while preparing (placement, a skirt or brim past the bed, open contours).
    #[cfg(test)]
    pub(crate) fn warnings_for_test(&self) -> &[String] {
        &self.warnings
    }

    /// The print with the plate's per-layer custom G-code.
    pub fn finish(self, config: &Config, custom: Info) -> Result<PlatePrint, String> {
        let Self {
            objects,
            mesh,
            name,
            mut warnings,
            preflight,
            mut notes,
        } = self;
        // Names and model bounds of the objects, for naming the owner of a skirt or brim
        // that passes the bed's edge.
        let owners: Vec<(String, [f64; 2], [f64; 2])> = objects
            .iter()
            .map(|o| {
                let (min, max) = o.mesh.bounds();
                (
                    o.name.clone(),
                    [f64::from(min[0]), f64::from(min[1])],
                    [f64::from(max[0]), f64::from(max[1])],
                )
            })
            .collect();
        let source = source(objects, &mut warnings, &mut notes, config, custom)?;
        adhesion_fits(&source, &owners, config)?;
        Ok(PlatePrint {
            source,
            mesh,
            name,
            warnings,
            preflight,
            notes,
        })
    }
}

/// The generated skirt and brim against the bed, by the exporter's bounds rule: a print the
/// exporter would refuse at its first layer fails here, naming the object, the edge and the
/// distance (`adhesion_check::explain`), in the same words for the CLI and the GUI.
fn adhesion_fits(
    source: &Source,
    owners: &[(String, [f64; 2], [f64; 2])],
    config: &Config,
) -> Result<(), String> {
    let plan = match source {
        Source::Single(object, _) => &object.adhesion,
        Source::Plate(print) => &print.adhesion,
    };
    let Some(volume) = crate::build_volume::BuildVolume::from_config(config) else {
        return Ok(());
    };
    let owners: Vec<crate::adhesion_check::Owner<'_>> = owners
        .iter()
        .map(|(name, min, max)| crate::adhesion_check::Owner {
            name,
            min: *min,
            max: *max,
        })
        .collect();
    match crate::adhesion_check::plan_overrun(plan, &owners, &volume) {
        Some((owner, overrun)) => Err(crate::adhesion_check::explain(
            owners[owner].name,
            &overrun,
            false,
        )),
        None => Ok(()),
    }
}

fn source(
    objects: Vec<PrintObject>,
    warnings: &mut Vec<String>,
    notes: &mut Vec<String>,
    config: &Config,
    mut custom: Info,
) -> Result<Source, String> {
    let count = config.vector_len("nozzle_diameter").max(1);
    if crate::sequential::enabled(config) && !custom.is_empty() {
        // As PrusaSlicer and Orca: no per-layer custom G-code when printing object by object.
        warnings.push(
            "color changes, pauses and other layer G-code are not used when objects print one \
             after another (as in PrusaSlicer and OrcaSlicer)"
                .into(),
        );
        custom = Info::default();
    }
    // One plain object on the first tool of a single-tool printer prints from its own
    // prepared source; everything else through the plate scheduler.
    let single = objects.len() == 1
        && !objects[0].prepared.has_regions()
        && objects[0].tools.0.iter().all(|t| t.is_none_or(|t| t == 0))
        && count <= 1;
    let source = if single {
        let object = objects.into_iter().next().expect("one object");
        let name = object.name.clone();
        let prepared = object.into_prepared()?;
        // As the plate scheduler: what Mica limited or widened in the object's textures.
        warnings.extend(prepared.texture_warnings());
        warnings.extend(prepared.brick_warnings());
        // As the plate scheduler (`PreparedPrint::new`): no object with an empty first layer.
        if !PrintObject::first_layer_prints(&prepared) {
            return Err(crate::print_job::empty_first_layer(&name, config));
        }
        Source::Single(Box::new(prepared), custom.clone())
    } else {
        let print = PreparedPrint::with_custom_gcode(objects, config, custom.clone())?;
        warnings.extend(print.warnings.iter().cloned());
        Source::Plate(Box::new(print))
    };
    for item in &custom.items {
        notes.push(format!("at {:.2} mm: {}", item.print_z, item.describe()));
    }
    Ok(source)
}

impl PlatePrint {
    /// The source the exporter reads: a single object carries the plate's custom G-code
    /// alongside it ([`WithCustomGCode`]).
    fn with<R>(&self, f: impl FnOnce(&dyn LayerSource) -> R) -> R {
        match &self.source {
            Source::Single(object, custom) if !custom.is_empty() => f(&WithCustomGCode {
                source: object.as_ref(),
                info: custom,
            }),
            Source::Single(object, _) => f(object.as_ref()),
            Source::Plate(print) => f(print.as_ref()),
        }
    }
}

impl LayerSource for PlatePrint {
    fn calibration_control(&self, layer: usize, path: usize) -> Option<crate::calib::Control> {
        self.with(|s| s.calibration_control(layer, path))
    }
    fn is_cancelled(&self) -> bool {
        self.with(|s| s.is_cancelled())
    }
    fn emitted(&self, layer: usize, paths: &[PathEmission], layer_time_s: f64) {
        self.with(|s| s.emitted(layer, paths, layer_time_s))
    }
    fn print_job(&self) -> Option<&PreparedPrint> {
        match &self.source {
            Source::Plate(print) => Some(print.as_ref()),
            Source::Single(..) => None,
        }
    }
    fn custom_gcode(&self) -> Option<&Info> {
        match &self.source {
            Source::Single(_, custom) => (!custom.is_empty()).then_some(custom),
            Source::Plate(print) => print.custom_gcode(),
        }
    }
    fn scheduled_toolpaths(&self, range: std::ops::Range<usize>) -> Vec<ScheduledLayer> {
        self.with(|s| s.scheduled_toolpaths(range))
    }
    fn first_layer_hull(&self) -> Option<&[Point]> {
        match &self.source {
            Source::Single(object, _) => object.first_layer_hull(),
            Source::Plate(print) => print.first_layer_hull(),
        }
    }
    fn layer_outlines(&self, index: usize) -> Option<&[ExPolygon]> {
        match &self.source {
            Source::Single(object, _) => object.layer_outlines(index),
            Source::Plate(print) => print.layer_outlines(index),
        }
    }
    // A single object with custom G-code answers as `WithCustomGCode` does: without top
    // surfaces or an origin.
    fn layer_top_surfaces(&self, index: usize) -> Option<&[ExPolygon]> {
        match &self.source {
            Source::Single(_, custom) if !custom.is_empty() => None,
            Source::Single(object, _) => object.layer_top_surfaces(index),
            Source::Plate(print) => print.layer_top_surfaces(index),
        }
    }
    fn object_origin(&self, object: usize) -> Option<Point> {
        match &self.source {
            Source::Single(_, custom) if !custom.is_empty() => None,
            Source::Single(o, _) => o.object_origin(object),
            Source::Plate(print) => print.object_origin(object),
        }
    }
    fn support_islands(&self, index: usize) -> Option<Vec<ExPolygon>> {
        self.with(|s| s.support_islands(index))
    }
    fn region_slices(&self, index: usize) -> Option<Vec<Vec<ExPolygon>>> {
        self.with(|s| s.region_slices(index))
    }
    fn layer_count(&self) -> usize {
        self.with(|s| s.layer_count())
    }
    fn layer_z(&self, i: usize) -> (f64, f64) {
        self.with(|s| s.layer_z(i))
    }
    fn previous_layer(&self, i: usize) -> Option<usize> {
        self.with(|s| s.previous_layer(i))
    }
    fn next_layer(&self, i: usize) -> Option<usize> {
        self.with(|s| s.next_layer(i))
    }
    fn max_print_z(&self) -> f64 {
        self.with(|s| s.max_print_z())
    }
    fn first_layer_bbox(&self) -> Option<BoundingBox> {
        self.with(|s| s.first_layer_bbox())
    }
    fn toolpaths(&self, range: std::ops::Range<usize>) -> Vec<LayerToolpaths> {
        self.with(|s| s.toolpaths(range))
    }
}
