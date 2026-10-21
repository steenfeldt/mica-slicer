// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

use crate::{
    preview::{Builder, Preview},
    scene::{Bed, Object},
};
use mica_core::{
    config::Config,
    geometry::{BoundingBox, ExPolygon, Point},
    print::LayerSource,
    print_job::{PreparedPrint, ScheduledLayer, tool_config},
    progress::{Observer, Stage},
    settings::PrintSettings,
    toolpath::LayerToolpaths,
};
use mica_gcode::writer::MoveStats;
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{BufRead, BufReader, BufWriter, Write},
    ops::Range,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
};

static TEMP_ID: AtomicU64 = AtomicU64::new(0);
pub struct TempJob {
    pub path: PathBuf,
}
impl TempJob {
    pub fn new() -> std::io::Result<Self> {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "mica-gui-{}-{stamp}-{}",
            std::process::id(),
            TEMP_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path)?;
        Ok(Self { path })
    }
}
impl Drop for TempJob {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}
#[derive(Clone)]
pub struct Finished {
    pub preview: Arc<Preview>,
    pub stats: MoveStats,
    pub binary: bool,
    pub summary: Summary,
    /// The exact prime / wipe tower of the job, when it printed one.
    pub tower: Option<mica_core::wipe_tower::TowerShape>,
    /// Problems the slice reports without failing (PrusaSlicer's sequential collision notice).
    pub warnings: Vec<String>,
}

/// Filament use of one tool in the sliced plate.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ToolUsage {
    pub tool: usize,
    pub filament_mm: f64,
    pub grams: f64,
    pub cost: f64,
    pub filament: String,
}

/// What the summary panel shows after a slice.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Summary {
    pub time_s: f64,
    pub silent_time_s: Option<f64>,
    pub layers: usize,
    pub filament_mm: f64,
    pub grams: f64,
    pub cost: f64,
    pub tools: Vec<ToolUsage>,
}

impl Summary {
    /// Weight from density and diameter, cost from `filament_cost` (money per kg), per tool.
    pub fn compute(
        stats: MoveStats,
        layers: usize,
        per_tool_mm: &[(usize, f64)],
        config: &Config,
    ) -> Self {
        // `filament_settings_id` is preset bookkeeping, kept as raw text by the config.
        let names = config
            .raw("filament_settings_id")
            .and_then(|raw| mica_core::config::unescape_strings(&raw).ok())
            .unwrap_or_default();
        let mut tools = Vec::new();
        for &(tool, mm) in per_tool_mm {
            if mm <= 0. {
                continue;
            }
            let d = config.float_at("filament_diameter", tool).unwrap_or(1.75);
            let cm3 = mm * std::f64::consts::PI * (d / 2.).powi(2) / 1000.;
            let grams = cm3 * config.float_at("filament_density", tool).unwrap_or(0.);
            let cost = grams / 1000. * config.float_at("filament_cost", tool).unwrap_or(0.);
            tools.push(ToolUsage {
                tool,
                filament_mm: mm,
                grams,
                cost,
                filament: names
                    .get(tool)
                    .or_else(|| names.first())
                    .cloned()
                    .unwrap_or_default(),
            });
        }
        Self {
            time_s: stats.time_s,
            silent_time_s: stats.silent_time_s,
            layers,
            filament_mm: stats.filament_mm,
            grams: tools.iter().map(|t| t.grams).sum(),
            cost: tools.iter().map(|t| t.cost).sum(),
            tools,
        }
    }
}

/// Per-tool filament from the G-code footer. Exact `T<n> [mm]` totals win; otherwise the
/// per-role lines are summed; a single-tool file without tool lines attributes everything to
/// tool 0.
pub fn per_tool_filament(gcode_tail: &str, total_mm: f64) -> Vec<(usize, f64)> {
    let mut exact: BTreeMap<usize, f64> = BTreeMap::new();
    let mut by_role: BTreeMap<usize, f64> = BTreeMap::new();
    for line in gcode_tail.lines() {
        let Some(rest) = line.strip_prefix("; filament used T") else {
            continue;
        };
        let Some((tool, rest)) = rest.split_once(' ') else {
            continue;
        };
        let Ok(tool) = tool.parse::<usize>() else {
            continue;
        };
        let Some((unit, value)) = rest.rsplit_once(" = ") else {
            continue;
        };
        let Ok(value) = value.trim().parse::<f64>() else {
            continue;
        };
        if unit.trim() == "[mm]" {
            exact.insert(tool, value);
        } else if unit.trim().ends_with("[mm]") {
            *by_role.entry(tool).or_default() += value;
        }
    }
    let mut out: Vec<(usize, f64)> = if !exact.is_empty() {
        exact.into_iter().collect()
    } else {
        by_role.into_iter().collect()
    };
    if out.is_empty() && total_mm > 0. {
        out.push((0, total_mm));
    }
    out
}

/// Reads the last few kilobytes of the text G-code (where the statistics live).
fn gcode_tail(path: &std::path::Path) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = File::open(path) else {
        return String::new();
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let start = len.saturating_sub(64 * 1024);
    if f.seek(SeekFrom::Start(start)).is_err() {
        return String::new();
    }
    let mut buf = Vec::new();
    let _ = f.read_to_end(&mut buf);
    String::from_utf8_lossy(&buf).into_owned()
}

/// Things worth telling the user before slicing a plate.
pub fn plate_warnings(objects: &[Object], bed: &Bed, config: &Config) -> Vec<String> {
    let mut out = mica_core::safety::config_warnings(config);
    if objects.is_empty() {
        out.push("The plate is empty".into());
        return out;
    }
    for o in objects {
        let (min, _) = o.bounds();
        // Where and how far it reaches outside the printable volume, in one sentence.
        if let Some(outside) = crate::scene::outside::outside(o, bed) {
            out.push(crate::scene::outside::describe(&o.name, &outside));
        }
        if min.z > 0.05 {
            out.push(format!("{} floats {:.1} mm above the bed", o.name, min.z));
        }
        if min.z < -0.05 {
            out.push(format!("{} sinks {:.1} mm into the bed", o.name, -min.z));
        }
    }
    for (i, a) in objects.iter().enumerate() {
        for b in &objects[i + 1..] {
            let (amin, amax) = a.bounds();
            let (bmin, bmax) = b.bounds();
            if amin.x < bmax.x && bmin.x < amax.x && amin.y < bmax.y && bmin.y < amax.y {
                out.push(format!("{} and {} overlap", a.name, b.name));
            }
        }
    }
    out
}

/// The pre-slice skirt and brim check of one object (`mica_core::adhesion_check::preflight`,
/// which the print builder runs too): its first-layer section grown by its brim and skirt
/// against the bed, with the object's own settings on `config` (the plate's).
pub fn adhesion_overrun(
    object: &Object,
    config: &Config,
) -> Option<mica_core::adhesion_check::Overrun> {
    let config = object_config(config, object).ok()?;
    let t = object.transform();
    mica_core::adhesion_check::preflight(
        &object.mesh,
        &|v| {
            let p = t.transform_point3(glam::Vec3::from(v));
            [f64::from(p.x), f64::from(p.y), f64::from(p.z)]
        },
        &config,
    )
}

/// A plate printed one object after another: its clearance check, and each object's
/// clearance outline (`outlines[i]` belongs to `objects[i]`; empty unless sequential).
pub struct SequentialCheck {
    pub report: mica_core::sequential::Report,
    pub outlines: Vec<(Vec<[f32; 2]>, bool)>,
}

/// Runs the engine's sequential-printing check on the plate's objects in print order
/// (object, then instance, as the slicer orders them).
pub fn sequential_check(objects: &[Object], config: &Config) -> SequentialCheck {
    let none = SequentialCheck {
        report: mica_core::sequential::Report::default(),
        outlines: Vec::new(),
    };
    if !mica_core::sequential::enabled(config) || objects.len() < 2 {
        return none;
    }
    let mut order: Vec<usize> = (0..objects.len()).collect();
    order.sort_by_key(|&i| (objects[i].object_id, objects[i].instance_id));
    let meshes: Vec<mica_core::mesh::TriangleMesh> =
        order.iter().map(|&i| objects[i].placed_mesh()).collect();
    let parts: Vec<[&mica_core::mesh::TriangleMesh; 1]> = meshes.iter().map(|m| [m]).collect();
    let instances: Vec<mica_core::sequential::Instance<'_>> = order
        .iter()
        .zip(&parts)
        .map(|(&i, p)| mica_core::sequential::Instance {
            name: &objects[i].name,
            parts: p,
        })
        .collect();
    let report = mica_core::sequential::check(&instances, config);
    let mut outlines = vec![(Vec::new(), false); objects.len()];
    for (k, (&i, instance)) in order.iter().zip(&instances).enumerate() {
        outlines[i] = (
            mica_core::sequential::clearance_outline(instance, config)
                .into_iter()
                .map(|p| [p[0] as f32, p[1] as f32])
                .collect(),
            report.colliding.get(k).copied().unwrap_or(false),
        );
    }
    SequentialCheck { report, outlines }
}

/// The plate configuration with one object's overrides and its tool assignment applied.
/// The tool keys are written the way the CLI does (`mica_core::input`): the object's
/// default tool is what its unpainted surfaces and supports print with, so leaving the
/// plate's keys in place printed an object assigned to tool 3 with tool 1 wherever it
/// was not painted.
pub fn object_config(config: &Config, object: &Object) -> Result<Config, String> {
    let mut c = config.clone();
    for (k, v) in &object.overrides {
        c.set_raw(k, v)
            .map_err(|e| format!("{}: {e}", object.name))?;
    }
    for (key, tool) in TOOL_KEYS.into_iter().zip(object.tools.0) {
        c.set_raw(key, &tool.map_or(0, |t| t + 1).to_string())
            .map_err(|e| format!("{}: {e}", object.name))?;
    }
    Ok(c)
}

/// The per-object tool keys, in `ToolAssignments` order.
const TOOL_KEYS: [&str; 5] = [
    "perimeter_extruder",
    "infill_extruder",
    "solid_infill_extruder",
    "support_material_extruder",
    "support_material_interface_extruder",
];
/// Where a slice job is, for the header and the hint line.
#[derive(Clone, Debug, PartialEq)]
pub struct Progress {
    /// Fraction of the whole job, 0 ..= 1: the real overall percentage.
    pub overall: f32,
    /// The stage as the header names it: `Preparing · seam planning`, `Toolpaths`.
    pub stage: String,
    /// Fraction done of that stage, 0 ..= 1.
    pub fraction: f32,
    /// The hint-line text: `Preparing 3DBenchy · seam planning 42 %`.
    pub detail: String,
}
impl Progress {
    /// A stage without sub-steps: its fraction is the overall one.
    fn plain(overall: f32, stage: &str, detail: impl Into<String>) -> Self {
        Self {
            overall,
            stage: stage.into(),
            fraction: overall,
            detail: detail.into(),
        }
    }
    /// `Preparing · seam planning 42 %`
    pub fn header(&self) -> String {
        format!("{} {:.0} %", self.stage, self.fraction * 100.)
    }
}
pub type Report<'a> = &'a (dyn Fn(Progress) + Sync);

/// Part of the job spent preparing objects (the rest is scheduling, toolpaths and output).
const PREPARE_SPAN: f32 = 0.30;

/// The engine's preparation observer for one object: maps each stage's own fraction into
/// the job's overall progress (stages in the order the engine runs them) and answers the
/// cancel flag, which the engine checks per layer, volume, shell and batch.
struct Preparation<'a> {
    name: String,
    /// Overall progress where this object's preparation starts, and its share.
    base: f32,
    span: f32,
    cancel: &'a AtomicBool,
    report: Report<'a>,
    /// Furthest fraction reported so far per stage (`Self::index`). Parallel layers tick out of
    /// order, and seam planning runs concurrently with the surfaces, so the overall fraction is
    /// the weighted sum of every stage's furthest fraction: it never goes back, and an event only
    /// goes out when it advances its own stage.
    reported: Mutex<[f32; 5]>,
}
impl Preparation<'_> {
    /// Weight of a stage within one object's preparation (they sum to one).
    fn weight_of(stage: Stage) -> f32 {
        match stage {
            Stage::Slicing | Stage::Regions => 0.15,
            Stage::Seams | Stage::Supports => 0.25,
            Stage::Surfaces => 0.20,
            Stage::Toolpaths | Stage::Export => 0.,
        }
    }
    fn index(stage: Stage) -> Option<usize> {
        match stage {
            Stage::Slicing => Some(0),
            Stage::Regions => Some(1),
            Stage::Seams => Some(2),
            Stage::Supports => Some(3),
            Stage::Surfaces => Some(4),
            Stage::Toolpaths | Stage::Export => None,
        }
    }
}
impl Observer for Preparation<'_> {
    fn progress(&self, stage: Stage, fraction: f32) {
        let fraction = fraction.clamp(0., 1.);
        let Some(index) = Self::index(stage) else {
            return;
        };
        let mut reported = self.reported.lock().unwrap_or_else(|p| p.into_inner());
        if fraction < reported[index] {
            return;
        }
        reported[index] = fraction;
        let done: f32 = [
            Stage::Slicing,
            Stage::Regions,
            Stage::Seams,
            Stage::Supports,
            Stage::Surfaces,
        ]
        .into_iter()
        .map(|s| Self::weight_of(s) * reported[Self::index(s).unwrap()].max(0.))
        .sum();
        let overall = self.base + self.span * done;
        (self.report)(Progress {
            overall,
            stage: format!("Preparing · {}", stage.label()),
            fraction,
            detail: format!(
                "Preparing {} · {} {:.0} %",
                self.name,
                stage.label(),
                fraction * 100.
            ),
        });
    }
    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

pub enum Event {
    Progress(Progress),
    Done(Box<Result<Finished, String>>),
}
static JOBS_STARTED: AtomicU64 = AtomicU64::new(0);
/// How many slice jobs this process has started (tests assert that requests coalesce).
#[cfg_attr(not(test), allow(dead_code))]
pub fn jobs_started() -> u64 {
    JOBS_STARTED.load(Ordering::Relaxed)
}
/// What a slice job prints: the plate's objects, or a calibration test regenerated from its
/// parameters with the engine's own toolpath controls (`mica_core::calib`).
pub enum Job {
    /// The plate's objects and its per-layer custom G-code (colour changes, pauses…).
    Objects(Vec<Object>, mica_core::custom_gcode::Info),
    Calibration(mica_core::calib::Parameters),
}

pub struct Worker {
    pub receiver: mpsc::Receiver<Event>,
    pub cancel: Arc<AtomicBool>,
    /// The slice request this job answers (the editor numbers them).
    pub request: u64,
}
impl Worker {
    pub fn start(job: Job, config: Config, ctx: eframe::egui::Context, request: u64) -> Self {
        JOBS_STARTED.fetch_add(1, Ordering::Relaxed);
        let (tx, receiver) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let token = cancel.clone();
        std::thread::spawn(move || {
            let boundary = mica_core::job::Boundary::new("slice setup");
            let report = |p: Progress| {
                boundary.stage(&p.stage);
                let _ = tx.send(Event::Progress(p));
                ctx.request_repaint();
            };
            let result = boundary.run(|| match job {
                Job::Objects(objects, custom) => {
                    slice_with_custom_gcode(objects, custom, config, &token, &report)
                }
                Job::Calibration(params) => slice_calibration(&params, config, &token, &report),
            });
            let _ = tx.send(Event::Done(Box::new(result)));
            ctx.request_repaint();
        });
        Self {
            receiver,
            cancel,
            request,
        }
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}
fn check(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) {
        Err("Canceled".into())
    } else {
        Ok(())
    }
}

pub fn slice(
    objects: Vec<Object>,
    config: Config,
    cancel: &AtomicBool,
    report: Report<'_>,
) -> Result<Finished, String> {
    slice_with_custom_gcode(
        objects,
        mica_core::custom_gcode::Info::default(),
        config,
        cancel,
        report,
    )
}

/// [`slice`] with the plate's per-layer custom G-code (colour changes, pauses, custom G-code),
/// which the exporter places on its layers and the preview colours by.
pub fn slice_with_custom_gcode(
    objects: Vec<Object>,
    custom_gcode: mica_core::custom_gcode::Info,
    config: Config,
    cancel: &AtomicBool,
    report: Report<'_>,
) -> Result<Finished, String> {
    if objects.is_empty() {
        return Err("Open a model first".into());
    }
    // Tools beyond the printer's count print with its last tool (mixed colors are tools
    // after the physical ones).
    let count = config.vector_len("nozzle_diameter").max(1);
    let objects: Vec<Object> = objects
        .into_iter()
        .map(|mut o| {
            let mixes = o.mixes.clone();
            for (role, tool) in o.tools.0.iter_mut().enumerate() {
                if let Some(tool) = tool
                    && !(role < 3 && mixes.as_ref().is_some_and(|m| m.contains(*tool + 1)))
                {
                    *tool = (*tool).min(count - 1);
                }
            }
            o
        })
        .collect();
    let job = Arc::new(TempJob::new().map_err(|e| e.to_string())?);
    // The objects as the print builder takes them (plain objects from their model frame),
    // built in parallel; the builder shares one preparation between instances that only move
    // and prepares the others in parallel, exactly as for the CLI.
    use rayon::prelude::*;
    let inputs = objects
        .par_iter()
        .map(|o| -> Result<_, String> {
            check(cancel)?;
            Ok(o.slicing_input(object_config(&config, o)?))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let observe = |p: mica_core::input::Preparing<'_>| -> Box<dyn Observer + '_> {
        let observer = Preparation {
            name: p.object.name.clone(),
            base: PREPARE_SPAN * p.index as f32 / p.count as f32,
            span: PREPARE_SPAN / p.count as f32,
            cancel,
            report,
            reported: Mutex::new([-1.; 5]),
        };
        observer.progress(Stage::Slicing, 0.);
        Box::new(observer)
    };
    let plate = mica_core::input::prepare_plate(inputs, &config, &observe)?;
    check(cancel)?;
    report(Progress::plain(0.32, "Scheduling", "Scheduling tools…"));
    let print = plate.finish(&config, custom_gcode)?;
    check(cancel)?;
    let warnings = slice_warnings(&print);
    let settings = (0..config.vector_len("nozzle_diameter").max(1))
        .map(|i| PrintSettings::from_config(&tool_config(&config, i)).map_err(|e| e.to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    // The exporter's settings, as the CLI passes them.
    let export_settings = PrintSettings::from_config(&config).map_err(|e| e.to_string())?;
    let source = Capture {
        source: print,
        builder: Mutex::new(Some(
            Builder::new(&job, &config, &settings).map_err(|e| e.to_string())?,
        )),
        error: Mutex::new(None),
        settings,
        pending: Mutex::new(BTreeMap::new()),
        cancel,
        report,
    };
    finish(
        &job,
        &source,
        &config,
        &export_settings,
        &source.source.name,
        Some(&source.source.mesh),
        warnings,
    )
}

/// What the print builder warned about, as the CLI prints it (open contours dropped, a model
/// smaller than 1 mm, a collision notice, what Mica limited in a texture), without the
/// placement warnings the plate view already shows in its own words while the objects are
/// placed so (`plate_warnings`, the skirt and brim check), and each once.
pub fn slice_warnings(print: &mica_core::input::PlatePrint) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for w in &print.warnings {
        if !print.preflight.contains(w) && !out.contains(w) {
            out.push(w.clone());
        }
    }
    out
}

/// A calibration plate: the test is regenerated from its parameters (including the tool it
/// prints on) on the plate's configuration and exported through the engine's calibration
/// source, exactly as `mica calibrate` does, so the per-path pressure advance, temperature,
/// retraction and flow controls reach the G-code. The user's settings are not changed; the
/// engine's overrides apply to this job alone.
pub fn slice_calibration(
    params: &mica_core::calib::Parameters,
    config: Config,
    cancel: &AtomicBool,
    report: Report<'_>,
) -> Result<Finished, String> {
    if !matches!(config.string("gcode_flavor"), Some("marlin2" | "klipper")) {
        return Err("Calibration supports Prusa Buddy and Klipper printers".into());
    }
    report(Progress::plain(
        0.05,
        "Preparing",
        "Generating the calibration test…",
    ));
    let generated = mica_core::calib::generate(params, &config)?;
    check(cancel)?;
    let print = generated.prepare(&config)?;
    check(cancel)?;
    report(Progress::plain(0.32, "Scheduling", "Scheduling the test…"));
    let job = Arc::new(TempJob::new().map_err(|e| e.to_string())?);
    let config = print.config.clone();
    let settings = vec![print.settings.clone()];
    let source = Capture {
        source: print,
        builder: Mutex::new(Some(
            Builder::new(&job, &config, &settings).map_err(|e| e.to_string())?,
        )),
        error: Mutex::new(None),
        settings,
        pending: Mutex::new(BTreeMap::new()),
        cancel,
        report,
    };
    let mesh = generated.combined_mesh();
    let name = format!("calibration {:?}", params.kind);
    let export_settings = source.settings[0].clone();
    let warnings = source
        .source
        .print_job()
        .map(|j| j.warnings.clone())
        .unwrap_or_default();
    finish(
        &job,
        &source,
        &config,
        &export_settings,
        &name,
        Some(&mesh),
        warnings,
    )
}

/// Exports the captured source to text (and Prusa binary) G-code in the job directory and
/// turns the capture into the preview, statistics and summary.
fn finish<S: LayerSource>(
    job: &Arc<TempJob>,
    source: &Capture<'_, S>,
    config: &Config,
    settings: &PrintSettings,
    name: &str,
    mesh: Option<&mica_core::mesh::TriangleMesh>,
    warnings: Vec<String>,
) -> Result<Finished, String> {
    let (cancel, report) = (source.cancel, source.report);
    let text_path = job.path.join("print.gcode");
    let exported =
        mica_gcode::export::export_file(source, config, settings, name, mesh, &text_path);
    if let Some(e) = source
        .error
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .take()
    {
        return Err(e);
    }
    let stats = exported.map_err(|e| e.to_string())?;
    check(cancel)?;
    let binary = config.string("gcode_flavor") == Some("marlin2");
    if binary {
        report(Progress::plain(0.96, "Packing", "Packing binary G-code…"));
        let info = source
            .source
            .print_job()
            .map(mica_gcode::multi::objects_info)
            .or_else(|| mesh.map(|m| mica_gcode::prusa_metadata::objects_info(name, m)));
        let thumbnails = mesh
            .map(|m| mica_gcode::thumbnails::generate(config, m))
            .transpose()
            .map_err(|e| e.to_string())?
            .unwrap_or_default();
        let stamp = mica_gcode::identity::epoch().map_err(|e| e.to_string())?;
        if source.layer_count() == 0 {
            return Err("Nothing to print: the plate has no layers".into());
        }
        let metadata = mica_gcode::prusa_metadata::bgcode_metadata(
            config,
            stats,
            mica_gcode::prusa_metadata::max_layer_z(config, source.max_print_z()),
            info.as_deref(),
            &mica_gcode::prusa_metadata::utc_timestamp(stamp),
            thumbnails,
        );
        let file =
            BufWriter::new(File::create(job.path.join("print.bgcode")).map_err(|e| e.to_string())?);
        let mut encoder = mica_gcode::bgcode::Encoder::new(file, &metadata, &Default::default())
            .map_err(|e| e.to_string())?;
        // Text export includes base64 thumbnails; binary export already has them as
        // blocks. Remove the exact generated wrapper/payload before feeding the same
        // program to the encoder (including its leading/trailing separator comments).
        let mut thumbnail_text = Vec::new();
        mica_gcode::thumbnails::write_text(&mut thumbnail_text, &metadata.thumbnails, false)
            .map_err(|e| e.to_string())?;
        let thumbnail_text = String::from_utf8(thumbnail_text).map_err(|e| e.to_string())?;
        let mut skipped_thumbnails = thumbnail_text.is_empty();
        let mut reader = BufReader::new(File::open(&text_path).map_err(|e| e.to_string())?);
        let mut line = String::new();
        loop {
            check(cancel)?;
            line.clear();
            if reader.read_line(&mut line).map_err(|e| e.to_string())? == 0 {
                break;
            }
            if !skipped_thumbnails && line == "\n" {
                for expected in thumbnail_text.split_inclusive('\n').skip(1) {
                    line.clear();
                    reader.read_line(&mut line).map_err(|e| e.to_string())?;
                    if line != expected {
                        return Err(
                            "text thumbnail block changed while packing binary G-code".into()
                        );
                    }
                }
                skipped_thumbnails = true;
                continue;
            }
            encoder.append_gcode(&line).map_err(|e| e.to_string())?;
        }
        encoder
            .finish()
            .map_err(|e| e.to_string())?
            .flush()
            .map_err(|e| e.to_string())?;
    }
    source.flush_pending();
    let mut preview = source
        .builder
        .lock()
        .unwrap()
        .take()
        .unwrap()
        .finish_with(job.clone(), Some(stats.time_s))
        .map_err(|e| e.to_string())?;
    if let Some(info) = source.custom_gcode() {
        let tools = config.vector_len("nozzle_diameter").max(1);
        let object_tools = source
            .source
            .print_job()
            .map_or(1, |j| PreparedPrint::object_tool_count(&j.objects));
        let zs: Vec<f64> = preview.layer_zs();
        preview.color_print = crate::preview::color_print::ColorPrint::build(
            info,
            &zs,
            tools,
            object_tools,
            crate::preview::color_print::dialect(config),
        );
    }
    check(cancel)?;
    let per_tool = per_tool_filament(&gcode_tail(&text_path), stats.filament_mm);
    let summary = Summary::compute(stats, preview.layers.len(), &per_tool, config);
    let tower = source
        .source
        .print_job()
        .and_then(|j| j.wipe_tower.as_ref())
        .map(|t| t.shape(true));
    report(Progress::plain(1., "Done", "Ready to export"));
    Ok(Finished {
        preview: Arc::new(preview),
        stats,
        binary,
        summary,
        tower,
        warnings,
    })
}

/// A layer source wrapped so the exporter's emitted speeds and times feed the preview.
struct Capture<'a, S: LayerSource> {
    source: S,
    builder: Mutex<Option<Builder>>,
    error: Mutex<Option<String>>,
    settings: Vec<PrintSettings>,
    /// Layers handed to the exporter and not yet reported back with their emitted speeds
    /// (at most one export batch).
    pending: Mutex<BTreeMap<usize, ScheduledLayer>>,
    cancel: &'a AtomicBool,
    report: Report<'a>,
}
impl<S: LayerSource> Capture<'_, S> {
    /// Writes a layer to the preview, with the exporter's speeds when it reported them.
    fn record(
        &self,
        layer: &ScheduledLayer,
        emitted: Option<(&[mica_core::print::PathEmission], f64)>,
    ) {
        if let Some(builder) = self
            .builder
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_mut()
        {
            // Calibration controls can set a nozzle temperature per path (temperature towers).
            builder.path_temperatures = (0..layer.layer.paths.len())
                .map(|i| {
                    self.source
                        .calibration_control(layer.layer.index, i)
                        .and_then(|c| c.temperature)
                })
                .collect();
            let result = match emitted {
                Some((paths, time)) => builder.push_emitted(layer, &self.settings, paths, time),
                None => builder.push(layer, &self.settings),
            }
            .and_then(|()| {
                if let Some(tower) = self.source.print_job().and_then(|j| j.wipe_tower.as_ref()) {
                    if layer.layer.index == 0 {
                        builder.tower(tower.priming_visits())?;
                    }
                    builder.tower(tower.visits(layer.layer.index))?;
                }
                Ok(())
            });
            if let Err(e) = result {
                *self.error.lock().unwrap_or_else(|p| p.into_inner()) = Some(e.to_string());
            }
        }
    }
    /// Layers the exporter never reported (none in practice) still reach the preview.
    fn flush_pending(&self) {
        let pending = std::mem::take(&mut *self.pending.lock().unwrap_or_else(|p| p.into_inner()));
        for layer in pending.values() {
            self.record(layer, None);
        }
    }
}
impl<S: LayerSource> LayerSource for Capture<'_, S> {
    fn calibration_control(&self, layer: usize, path: usize) -> Option<mica_core::calib::Control> {
        self.source.calibration_control(layer, path)
    }
    fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
            || self
                .error
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .is_some()
    }
    fn emitted(&self, layer: usize, paths: &[mica_core::print::PathEmission], layer_time_s: f64) {
        if let Some(l) = self
            .pending
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&layer)
        {
            self.record(&l, Some((paths, layer_time_s)));
        }
    }
    fn print_job(&self) -> Option<&PreparedPrint> {
        self.source.print_job()
    }
    fn custom_gcode(&self) -> Option<&mica_core::custom_gcode::Info> {
        self.source.custom_gcode()
    }
    fn layer_count(&self) -> usize {
        self.source.layer_count()
    }
    fn layer_z(&self, i: usize) -> (f64, f64) {
        self.source.layer_z(i)
    }
    fn first_layer_bbox(&self) -> Option<BoundingBox> {
        self.source.first_layer_bbox()
    }
    fn first_layer_hull(&self) -> Option<&[Point]> {
        self.source.first_layer_hull()
    }
    fn layer_outlines(&self, i: usize) -> Option<&[ExPolygon]> {
        self.source.layer_outlines(i)
    }
    fn support_islands(&self, i: usize) -> Option<Vec<ExPolygon>> {
        self.source.support_islands(i)
    }
    fn region_slices(&self, i: usize) -> Option<Vec<Vec<ExPolygon>>> {
        self.source.region_slices(i)
    }
    fn toolpaths(&self, range: Range<usize>) -> Vec<LayerToolpaths> {
        self.source.toolpaths(range)
    }
    fn scheduled_toolpaths(&self, range: Range<usize>) -> Vec<ScheduledLayer> {
        let fraction = range.start as f32 / self.layer_count().max(1) as f32;
        (self.report)(Progress {
            overall: 0.36 + 0.57 * fraction,
            stage: "Toolpaths".into(),
            fraction,
            detail: format!(
                "Toolpaths · layers {}–{} / {}",
                range.start + 1,
                range.end,
                self.layer_count()
            ),
        });
        // Anything still pending from the previous batch was never reported: keep it.
        self.flush_pending();
        let layers = self.source.scheduled_toolpaths(range.clone());
        let mut pending = self.pending.lock().unwrap_or_else(|p| p.into_inner());
        // Keyed by job layer: a sequential object's layers count from its own first layer.
        for (i, l) in range.zip(&layers) {
            pending.insert(i, l.clone());
        }
        layers
    }
}

/// Portable, safe process memory query. Kept off the render thread (once per second).
///
/// On macOS this is the physical footprint (`footprint`, what Activity Monitor shows): `ps`'s
/// resident size also counts pages the allocator has already given back (reusable until the
/// kernel reclaims them), so after one heavy slice it stays gigabytes above what the process
/// holds. Elsewhere, and if `footprint` fails, the resident set size.
pub fn memory_mb() -> Option<f64> {
    #[cfg(target_os = "macos")]
    if let Some(mb) = footprint_mb() {
        return Some(mb);
    }
    #[cfg(unix)]
    {
        let out = std::process::Command::new("ps")
            .args(["-o", "rss=", "-p", &std::process::id().to_string()])
            .output()
            .ok()?;
        String::from_utf8(out.stdout)
            .ok()?
            .trim()
            .parse::<f64>()
            .ok()
            .map(|kb| kb / 1024.)
    }
    #[cfg(not(unix))]
    {
        None
    }
}

/// `phys_footprint` of this process in MB, from `/usr/bin/footprint` (about 40 ms).
#[cfg(target_os = "macos")]
fn footprint_mb() -> Option<f64> {
    let out = std::process::Command::new("/usr/bin/footprint")
        .args(["--pid", &std::process::id().to_string()])
        .args(["--noCategories", "-f", "bytes"])
        .output()
        .ok()?;
    parse_footprint(&String::from_utf8_lossy(&out.stdout)).map(|b| b / 1048576.)
}

/// The `phys_footprint: N B` line of `footprint -f bytes`.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn parse_footprint(text: &str) -> Option<f64> {
    text.lines().find_map(|l| {
        l.trim()
            .strip_prefix("phys_footprint:")?
            .trim()
            .strip_suffix(" B")?
            .trim()
            .parse()
            .ok()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;
    use mica_core::print_job::PrintObject;

    #[test]
    fn an_object_assigned_to_tool_3_prints_unpainted_surfaces_with_tool_3() {
        let mut c = config();
        c.set_raw("nozzle_diameter", "0.4,0.4,0.4,0.4").unwrap();
        let mut m = mica_core::mesh::TriangleMesh::load_stl(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/models/cube20.stl"),
        )
        .unwrap();
        m.place_on_bed(125., 110.);
        let o = Object::new(mica_core::input::InputObject::plain(
            "cube",
            0,
            0,
            m,
            mica_core::print_job::ToolAssignments([Some(2), Some(2), Some(2), None, None]),
            c.clone(),
        ));
        let oc = object_config(&c, &o).unwrap();
        assert_eq!(oc.raw("perimeter_extruder").as_deref(), Some("3"));
        assert_eq!(oc.raw("infill_extruder").as_deref(), Some("3"));
        assert_eq!(oc.raw("support_material_extruder").as_deref(), Some("0"));
        let f = slice(vec![o], c, &AtomicBool::new(false), &|_| {}).unwrap();
        let used: Vec<usize> = f.summary.tools.iter().map(|t| t.tool).collect();
        assert_eq!(used, vec![2], "{:?}", f.summary.tools);
    }
    /// cube20 at `center` with a loose wall of two triangles inside it: every layer up to
    /// 10 mm slices the wall to an open contour, which the slicer drops.
    fn cube_with_open_wall(center: (f32, f32)) -> mica_core::mesh::TriangleMesh {
        let mut m = mica_core::mesh::TriangleMesh::load_stl(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/models/cube20.stl"),
        )
        .unwrap();
        m.place_on_bed(center.0, center.1);
        let base = m.vertices.len() as u32;
        let (x, y) = center;
        m.vertices.extend([
            [x - 5., y, 0.],
            [x + 5., y, 0.],
            [x + 5., y, 10.],
            [x - 5., y, 10.],
        ]);
        m.faces
            .extend([[base, base + 1, base + 2], [base, base + 2, base + 3]]);
        m
    }

    fn plain(name: &str, id: usize, mesh: mica_core::mesh::TriangleMesh, c: &Config) -> Object {
        Object::new(mica_core::input::InputObject::plain(
            name,
            id,
            0,
            mesh,
            mica_core::print_job::ToolAssignments([None; 5]),
            c.clone(),
        ))
    }

    /// The slice's own warnings reach the GUI as the CLI prints them: a single plain object
    /// (whose print has no plate job) and a plate of several objects, where the GUI used to
    /// read only the plate scheduler's warnings.
    #[test]
    fn open_contours_dropped_while_slicing_are_reported() {
        let c = config();
        let open = |f: &Finished| {
            f.warnings
                .iter()
                .filter(|w| w.contains("open contour(s) dropped"))
                .count()
        };
        let single = vec![plain("open.stl", 0, cube_with_open_wall((125., 110.)), &c)];
        let f = slice(single, c.clone(), &AtomicBool::new(false), &|_| {}).unwrap();
        assert_eq!(open(&f), 1, "{:?}", f.warnings);
        assert!(f.warnings[0].starts_with("open.stl: "), "{:?}", f.warnings);
        let mut cube = mica_core::mesh::TriangleMesh::load_stl(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/models/cube20.stl"),
        )
        .unwrap();
        cube.place_on_bed(125., 170.);
        let plate = vec![
            plain("open.stl", 0, cube_with_open_wall((125., 110.)), &c),
            plain("cube.stl", 1, cube, &c),
        ];
        let f = slice(plate, c, &AtomicBool::new(false), &|_| {}).unwrap();
        assert_eq!(open(&f), 1, "{:?}", f.warnings);
        // Each warning once, and none about cube.stl, which is fine.
        let mut seen = f.warnings.clone();
        seen.dedup();
        assert_eq!(seen.len(), f.warnings.len(), "{:?}", f.warnings);
        assert!(f.warnings.iter().all(|w| !w.starts_with("cube.stl")));
    }

    /// What the plate view already says live (an object below the bed, a skirt or brim past
    /// the edge) is not repeated by the slice.
    #[test]
    fn placement_warnings_the_plate_view_shows_are_not_repeated() {
        let c = config();
        let mut m = cube_with_open_wall((125., 110.));
        for v in &mut m.vertices {
            v[2] -= 1.;
        }
        let f = slice(
            vec![plain("sunk.stl", 0, m, &c)],
            c,
            &AtomicBool::new(false),
            &|_| {},
        )
        .unwrap();
        assert!(
            f.warnings
                .iter()
                .all(|w| !w.contains(mica_core::safety::BELOW_BED)),
            "{:?}",
            f.warnings
        );
        assert!(
            f.warnings.iter().any(|w| w.contains("open contour")),
            "{:?}",
            f.warnings
        );
    }

    #[test]
    fn footprint_output_parses_the_current_not_the_peak_value() {
        let text = "Finder [2145]: 64-bit    Footprint: 323192296 B (16384 bytes per page)\n\n\
                    Auxiliary data:\n    phys_footprint_peak: 659113640 B\n    \
                    phys_footprint: 323192296 B\n";
        assert_eq!(parse_footprint(text), Some(323192296.));
        assert_eq!(parse_footprint("no such process"), None);
    }

    fn config() -> Config {
        let mut c = Config::defaults();
        for line in include_str!("../../mica-profiles/tests/fixtures/coreone_hf04_speed_pla.ini")
            .lines()
            .filter(|l| !l.starts_with('#'))
        {
            if let Some((k, v)) = line
                .split_once(" = ")
                .or_else(|| line.strip_suffix(" =").map(|k| (k, "")))
            {
                c.set_raw(k, v).unwrap();
            }
        }
        c
    }
    fn objects(c: &Config) -> Vec<Object> {
        mica_core::input::load(
            &[PathBuf::from(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../tests/models/cube20.stl"
            ))],
            (125., 110.),
            false,
            true,
            0,
            &Default::default(),
            c,
            &mica_profiles::project::Mapper::default(),
        )
        .unwrap()
        .objects
        .into_iter()
        .map(Object::new)
        .collect()
    }
    /// A two-tool config for the colour-painted fixture (no tower, so the per-tool totals
    /// are the painted geometry alone).
    fn two_tool_config() -> Config {
        let mut c = config();
        for (k, v) in [
            ("nozzle_diameter", "0.4,0.4"),
            ("filament_colour", "#FF8800;#2266FF"),
            ("extruder_colour", ";"),
            ("wipe_tower", "0"),
        ] {
            c.set_raw(k, v).unwrap();
        }
        c
    }
    fn load_painted(c: &Config) -> mica_core::input::Loaded {
        mica_core::input::load(
            &[PathBuf::from(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../tests/models/painted-cube.3mf"
            ))],
            (125., 110.),
            false,
            true,
            0,
            &Default::default(),
            c,
            &mica_profiles::project::Mapper::default(),
        )
        .unwrap()
    }

    /// The loaded 3MF's colour painting reaches the slicer through the GUI's objects: the
    /// painted faces print with tool 2, and the per-tool filament equals the CLI's path
    /// (`InputObject::prepare` + `export_file`) to the millimetre.
    #[test]
    fn colour_painted_3mf_slices_like_the_cli_with_two_tools() {
        let c = two_tool_config();
        let objects: Vec<Object> = load_painted(&c)
            .objects
            .into_iter()
            .map(Object::new)
            .collect();
        assert_eq!(objects.len(), 1);
        assert!(
            !objects[0].painting.material.is_empty(),
            "painting not read"
        );
        assert!(objects[0].placed_region().is_some());
        let gui = slice(objects, c.clone(), &AtomicBool::new(false), &|_| {}).unwrap();
        let gui_tools: Vec<(usize, f64)> = gui
            .summary
            .tools
            .iter()
            .map(|t| (t.tool, t.filament_mm))
            .collect();
        assert_eq!(gui_tools.len(), 2, "{gui_tools:?}");
        assert!(
            gui_tools[1].1 > 100.,
            "tool 2 printed nothing: {gui_tools:?}"
        );
        // The CLI path on the same input.
        let prepared = load_painted(&c)
            .objects
            .into_iter()
            .map(mica_core::input::InputObject::prepare)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let print = PreparedPrint::new(prepared, &c).unwrap();
        let settings = PrintSettings::from_config(&tool_config(&c, 0)).unwrap();
        let job = TempJob::new().unwrap();
        let out = job.path.join("cli.gcode");
        let stats =
            mica_gcode::export::export_file(&print, &c, &settings, "painted", None, &out).unwrap();
        let cli_tools = per_tool_filament(&gcode_tail(&out), stats.filament_mm);
        assert_eq!(cli_tools.len(), 2, "{cli_tools:?}");
        for ((gt, gm), (ct, cm)) in gui_tools.iter().zip(&cli_tools) {
            assert_eq!(gt, ct);
            assert!((gm - cm).abs() < 1e-6, "tool {gt}: GUI {gm} vs CLI {cm}");
        }
        // The same object without its painting prints with tool 1 alone.
        let mut plain: Vec<Object> = load_painted(&c)
            .objects
            .into_iter()
            .map(Object::new)
            .collect();
        plain[0].painting = Arc::new(Default::default());
        plain[0].painting_edited = true;
        let plain = slice(plain, c, &AtomicBool::new(false), &|_| {}).unwrap();
        assert_eq!(plain.summary.tools.len(), 1);
    }

    /// A 20 × 20 × 10 slab floating on a 4 × 4 pillar: its underside is an overhang.
    fn slab_on_pillar() -> mica_core::mesh::TriangleMesh {
        fn boxed(min: [f32; 3], max: [f32; 3]) -> Vec<[[f32; 3]; 3]> {
            let v = |x: usize, y: usize, z: usize| {
                [
                    if x == 0 { min[0] } else { max[0] },
                    if y == 0 { min[1] } else { max[1] },
                    if z == 0 { min[2] } else { max[2] },
                ]
            };
            vec![
                [v(0, 0, 0), v(1, 1, 0), v(1, 0, 0)],
                [v(0, 0, 0), v(0, 1, 0), v(1, 1, 0)],
                [v(0, 0, 1), v(1, 0, 1), v(1, 1, 1)],
                [v(0, 0, 1), v(1, 1, 1), v(0, 1, 1)],
                [v(0, 0, 0), v(1, 0, 0), v(1, 0, 1)],
                [v(0, 0, 0), v(1, 0, 1), v(0, 0, 1)],
                [v(1, 0, 0), v(1, 1, 0), v(1, 1, 1)],
                [v(1, 0, 0), v(1, 1, 1), v(1, 0, 1)],
                [v(1, 1, 0), v(0, 1, 0), v(0, 1, 1)],
                [v(1, 1, 0), v(0, 1, 1), v(1, 1, 1)],
                [v(0, 1, 0), v(0, 0, 0), v(0, 0, 1)],
                [v(0, 1, 0), v(0, 0, 1), v(0, 1, 1)],
            ]
        }
        let mut t = boxed([0., 0., 10.], [20., 20., 20.]);
        t.extend(boxed([8., 8., 0.], [12., 12., 10.]));
        mica_core::mesh::TriangleMesh::from_triangles(&t)
    }

    /// Painting made in the GUI slices: a support enforcer stroke under a floating slab gives
    /// support paths where nothing was generated before ("Enforcers only"), and a seam stripe
    /// on a cube face pulls every layer's seam into the stripe.
    #[test]
    fn painted_supports_and_seams_reach_the_slicer() {
        use crate::paint::{BrushKind, Hit, Session, Settings};
        use mica_core::painting::PaintKind;
        let mut c = config();
        c.set_raw("support_material", "1").unwrap();
        c.set_raw("support_material_auto", "0").unwrap();
        let object = |mesh: mica_core::mesh::TriangleMesh| {
            let mut m = mesh;
            m.place_on_bed(125., 110.);
            Object::new(mica_core::input::InputObject::plain(
                "slab",
                0,
                0,
                m,
                mica_core::print_job::ToolAssignments([Some(0); 5]),
                c.clone(),
            ))
        };
        let support_length = |o: Object| {
            let f = slice(vec![o], c.clone(), &AtomicBool::new(false), &|_| {}).unwrap();
            f.preview.stats.roles[12].length_mm + f.preview.stats.roles[13].length_mm
        };
        let plain = object(slab_on_pillar());
        assert_eq!(
            support_length(plain.clone()),
            0.,
            "enforcers-only prints no support"
        );
        // Fill the slab's underside (facet 0 of the slab box) with an enforcer.
        let mut painted = plain.clone();
        let mut session = Session::new(&painted, PaintKind::Supports);
        let settings = Settings {
            brush: BrushKind::Fill,
            ..Settings::default()
        };
        let (min, _) = painted.bounds();
        assert!(session.stroke(
            &settings,
            Hit {
                face: 0,
                point: Vec3::new(min.x + 3., min.y + 3., 10.),
                camera: Vec3::new(min.x + 3., min.y + 3., -100.),
            },
            false,
        ));
        session.store(&mut painted);
        assert!(support_length(painted) > 50.);

        // Seams: a stripe on the +X face of a cube, around y = centre.
        let cube = object(
            mica_core::mesh::TriangleMesh::load_stl(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../tests/models/cube20.stl"
            ))
            .unwrap(),
        );
        let (min, max) = cube.bounds();
        let centre_y = (min.y + max.y) / 2.;
        let mut striped = cube.clone();
        let mut session = Session::new(&striped, PaintKind::Seam);
        let settings = Settings {
            brush: BrushKind::Circle,
            radius: 1.5,
            ..Settings::default()
        };
        for z in (1..20).step_by(2).map(|z| z as f32) {
            // The facet under the pointer, as the viewport finds it: a ray from +X.
            let camera = Vec3::new(max.x + 100., centre_y, z);
            let hit = crate::scene::picking::pick_face(
                std::slice::from_ref(&striped),
                camera,
                Vec3::NEG_X,
            )
            .unwrap();
            assert!(session.stroke(
                &settings,
                Hit {
                    face: hit.face,
                    point: camera + Vec3::NEG_X * hit.distance,
                    camera,
                },
                false,
            ));
        }
        session.store(&mut striped);
        let mut leaves = 0;
        striped
            .painting
            .seam
            .visit(&striped.mesh, false, |_, _| leaves += 1);
        assert!(leaves > 10, "the seam stripe painted {leaves} leaves");
        let seams = |o: &Object| -> Vec<(f64, f64)> {
            let prepared = match o.placed_region() {
                Some(model) => PrintObject::prepare_regions(
                    "cube".into(),
                    0,
                    0,
                    o.placed_mesh(),
                    &model,
                    o.tools.clone(),
                    &c,
                ),
                None => {
                    PrintObject::prepare("cube".into(), 0, 0, o.placed_mesh(), o.tools.clone(), &c)
                }
            }
            .unwrap()
            .prepared;
            (1..prepared.layer_count().saturating_sub(1))
                .flat_map(|l| prepared.seams.points(l).collect::<Vec<_>>())
                .map(|p| {
                    (
                        p.x as f64 / mica_core::geometry::SCALE,
                        p.y as f64 / mica_core::geometry::SCALE,
                    )
                })
                .collect()
        };
        let before = seams(&cube);
        let after = seams(&striped);
        assert!(!after.is_empty());
        // Every seam of the striped cube sits on the +X face inside the stripe.
        for (x, y) in &after {
            assert!(
                (x - f64::from(max.x)).abs() < 0.6,
                "seam x {x} vs face {}",
                max.x
            );
            assert!(
                (y - f64::from(centre_y)).abs() < 2.5,
                "seam y {y} outside the stripe"
            );
        }
        // Unpainted, the aligned seam sits in a corner, not mid-face.
        assert!(
            before
                .iter()
                .any(|(_, y)| (y - f64::from(centre_y)).abs() > 5.),
            "{before:?}"
        );
    }

    /// A calibration plate's job exports exactly what the engine's calibration source does
    /// (`mica calibrate`): the per-height pressure advance commands and sample comments are in
    /// the G-code, and the preview, totals and binary file are made from it.
    #[test]
    fn calibration_plate_exports_like_the_cli() {
        let c = config();
        let params =
            mica_core::calib::Parameters::new(mica_core::calib::Kind::PressureAdvanceTower);
        let gui = slice_calibration(&params, c.clone(), &AtomicBool::new(false), &|_| {}).unwrap();
        let gui_text = fs::read_to_string(gui.preview.job.path.join("print.gcode")).unwrap();
        assert!(gui.binary && gui.preview.job.path.join("print.bgcode").is_file());
        assert_eq!(gui.summary.layers, gui.preview.layers.len());
        assert!(gui.summary.grams > 1., "{}", gui.summary.grams);
        // The engine's own export of the same parameters.
        let generated = mica_core::calib::generate(&params, &c).unwrap();
        let print = generated.prepare(&c).unwrap();
        let mesh = generated.combined_mesh();
        let job = TempJob::new().unwrap();
        let out = job.path.join("cli.gcode");
        mica_gcode::export::export_file(
            &print,
            &print.config,
            &print.settings,
            "calibration PressureAdvanceTower",
            Some(&mesh),
            &out,
        )
        .unwrap();
        let cli_text = fs::read_to_string(&out).unwrap();
        assert_eq!(
            gui_text, cli_text,
            "the GUI job's G-code differs from the engine's"
        );
        // One sample comment per layer (0.2 mm layers over 51 bands); 51 band values plus the
        // documented final full band one step above `end` (docs/calibration.md).
        let samples: Vec<&str> = gui_text
            .lines()
            .filter(|l| l.starts_with("; calibration sample="))
            .collect();
        assert_eq!(samples.len(), 255);
        let distinct: std::collections::BTreeSet<&str> = samples.iter().copied().collect();
        assert_eq!(distinct.len(), 52, "one value per band");
        assert!(distinct.contains("; calibration sample=.102"));
        assert!(gui_text.contains("M572 S0.1 ; calibration pressure advance"));
        assert!(gui_text.contains("M572 S0 ; calibration pressure advance"));
        // Ordinary plates are untouched by the job kind.
        let plain = slice(objects(&c), c, &AtomicBool::new(false), &|_| {}).unwrap();
        assert!(plain.summary.layers > 50);
    }

    #[test]
    fn worker_exports_both_formats_and_removes_temporary_files() {
        let c = config();
        let objects = objects(&c);
        let result = slice(objects, c, &AtomicBool::new(false), &|_| {}).unwrap();
        assert_eq!(result.preview.layers.len(), 100);
        let dir = result.preview.job.path.clone();
        let text = fs::read_to_string(dir.join("print.gcode")).unwrap();
        let decoded =
            mica_gcode::bgcode::decode(&fs::read(dir.join("print.bgcode")).unwrap()).unwrap();
        // Binary packing strips comments; every motion command must survive unchanged.
        let moves = |text: &str| {
            text.lines()
                .filter(|l| l.starts_with("G1 ") || l.starts_with("G2 ") || l.starts_with("G3 "))
                .map(|line| line.split(';').next().unwrap().trim().to_owned())
                .collect::<Vec<_>>()
        };
        let a = moves(&text);
        let b = moves(&decoded.gcode);
        assert_eq!(a.len(), b.len());
        for (i, (a, b)) in a.iter().zip(&b).enumerate() {
            assert_eq!(a, b, "motion {i}");
        }
        assert!(result.stats.filament_mm > 600.);
        let mut segments = 0;
        result
            .preview
            .read_batches(0..result.preview.count, |batch| segments += batch.len())
            .unwrap();
        assert_eq!(segments as u64, result.preview.count);
        drop(result);
        assert!(!dir.exists());
    }
    #[test]
    fn summary_has_per_tool_weight_and_cost() {
        let mut c = Config::defaults();
        c.set_raw("nozzle_diameter", "0.4,0.4").unwrap();
        c.set_raw("filament_diameter", "1.75,2.85").unwrap();
        c.set_raw("filament_density", "1.24,1.27").unwrap();
        c.set_raw("filament_cost", "25,40").unwrap();
        c.set_raw("filament_settings_id", "\"PLA\";\"PETG\"")
            .unwrap();
        let stats = MoveStats {
            filament_mm: 3000.,
            time_s: 3600.,
            ..Default::default()
        };
        let tail = "; filament used [mm] = 3000.00\n; filament used T0 [Perimeter] [mm] = 1000.00000\n; filament used T0 [Solid infill] [mm] = 1000.00000\n; filament used T1 [Perimeter] [mm] = 1000.00000\n";
        let per_tool = per_tool_filament(tail, stats.filament_mm);
        assert_eq!(per_tool, vec![(0, 2000.), (1, 1000.)]);
        let s = Summary::compute(stats, 100, &per_tool, &c);
        assert_eq!(s.layers, 100);
        assert_eq!(s.tools.len(), 2);
        // 2000 mm of 1.75 mm filament = 4.8106 cm³ → 5.965 g → 0.149 money.
        let t0 = &s.tools[0];
        assert!((t0.grams - 5.965).abs() < 0.01, "{}", t0.grams);
        assert!((t0.cost - 0.14913).abs() < 0.001, "{}", t0.cost);
        assert_eq!(t0.filament, "PLA");
        let t1 = &s.tools[1];
        assert!((t1.grams - 8.098).abs() < 0.01, "{}", t1.grams);
        assert!((s.grams - (t0.grams + t1.grams)).abs() < 1e-9);
        // Exact per-tool totals win over role sums; single-tool files fall back to tool 0.
        let exact = "; filament used T1 [Perimeter] [mm] = 5\n; filament used T1 [mm] = 42\n";
        assert_eq!(per_tool_filament(exact, 42.), vec![(1, 42.)]);
        assert_eq!(per_tool_filament("; nothing\n", 7.), vec![(0, 7.)]);
    }
    #[test]
    fn warnings_cover_bed_height_and_overlap() {
        let c = config();
        let mut objects = objects(&c);
        objects.push(objects[0].clone());
        objects[1].name = "second".into();
        let bed = Bed::from_config(&c);
        let w = plate_warnings(&objects, &bed, &c);
        assert_eq!(w, ["cube20.stl and second overlap"]);
        objects[1].position.x += 300.;
        objects[0].scale.z = 20.;
        objects[0].drop_to_bed();
        objects[0].position.z += 2.;
        let w = plate_warnings(&objects, &bed, &c);
        assert!(
            w.iter()
                .any(|w| w.starts_with("second sticks out ") && w.contains("past the right edge")),
            "{w:?}"
        );
        assert!(
            w.iter().any(|w| w.starts_with("cube20.stl sticks out ")
                && w.contains("mm above the maximum height")),
            "{w:?}"
        );
        assert!(w.iter().any(|w| w.contains("floats")), "{w:?}");
        assert_eq!(plate_warnings(&[], &bed, &c), ["The plate is empty"]);
    }
    #[test]
    fn per_object_overrides_change_that_object_only() {
        let c = config();
        let mut objects = objects(&c);
        objects.push(objects[0].clone());
        objects[1].name = "dense".into();
        objects[1].position.x += 40.;
        objects[1]
            .overrides
            .insert("fill_density".into(), "100%".into());
        objects[1].overrides.insert("perimeters".into(), "5".into());
        let plain = slice(
            objects[..1].to_vec(),
            c.clone(),
            &AtomicBool::new(false),
            &|_| {},
        )
        .unwrap();
        let both = slice(objects, c, &AtomicBool::new(false), &|_| {}).unwrap();
        assert_eq!(both.preview.layers.len(), plain.preview.layers.len());
        // The dense cube uses far more than twice the plain cube's filament.
        assert!(
            both.stats.filament_mm > plain.stats.filament_mm * 2.5,
            "{} vs {}",
            both.stats.filament_mm,
            plain.stats.filament_mm
        );
        assert_eq!(both.summary.tools.len(), 1);
        assert!(both.summary.grams > 0.);
    }
    #[test]
    fn cancellation_during_export_never_returns_a_finished_job() {
        let c = config();
        let objects = objects(&c);
        let cancelled = AtomicBool::new(false);
        let result = slice(objects, c, &cancelled, &|p| {
            if p.overall >= 0.36 {
                cancelled.store(true, Ordering::Relaxed);
            }
        });
        assert!(result.is_err());
        assert!(cancelled.load(Ordering::Relaxed));
    }

    #[test]
    fn moved_copies_are_prepared_once_and_rotated_ones_again() {
        let c = config();
        let single = slice(objects(&c), c.clone(), &AtomicBool::new(false), &|_| {})
            .unwrap()
            .summary
            .filament_mm;
        let mut scene = objects(&c);
        let mut copy = scene[0].clone();
        copy.name = "moved copy".into();
        copy.instance_id = 1;
        copy.position.x += 37.3;
        copy.position.y -= 21.9;
        let mut turned = scene[0].clone();
        turned.name = "turned copy".into();
        turned.instance_id = 2;
        turned.position.y += 40.;
        turned.angle = 0.5;
        scene.push(copy);
        scene.push(turned);
        let seen = Mutex::new(Vec::<String>::new());
        let result = slice(scene, c, &AtomicBool::new(false), &|p| {
            seen.lock()
                .unwrap_or_else(|p| p.into_inner())
                .push(p.detail);
        })
        .unwrap();
        let seen = seen.into_inner().unwrap();
        // The moved copy shares the first cube's preparation; the turned one has its own.
        assert!(!seen.iter().any(|d| d.starts_with("Preparing moved copy")));
        assert!(seen.iter().any(|d| d.starts_with("Preparing turned copy")));
        // Three cubes; the purge line and the plate's skirt are printed once.
        let total = result.summary.filament_mm;
        assert!(
            (total - 3. * single).abs() < 0.03 * 3. * single,
            "{total} vs 3 × {single}"
        );
    }

    #[test]
    fn preparation_reports_every_stage_with_a_rising_overall_fraction() {
        let c = config();
        let objects = objects(&c);
        let seen = Mutex::new(Vec::<Progress>::new());
        let result = slice(objects, c, &AtomicBool::new(false), &|p| {
            seen.lock().unwrap_or_else(|p| p.into_inner()).push(p);
        })
        .unwrap();
        assert!(result.preview.layers.len() > 10);
        let seen = seen.into_inner().unwrap();
        let stages: std::collections::BTreeSet<&str> =
            seen.iter().map(|p| p.stage.as_str()).collect();
        for stage in [
            "Preparing · slicing layers",
            "Preparing · seam planning",
            "Preparing · supports",
            "Preparing · walls and surfaces",
            "Scheduling",
            "Toolpaths",
            "Done",
        ] {
            assert!(stages.contains(stage), "{stage} missing from {stages:?}");
        }
        // Stage fractions run 0 → 1 inside each stage; the overall fraction never goes back.
        assert!(seen.iter().all(|p| (0. ..=1.).contains(&p.fraction)));
        assert!(
            seen.windows(2).all(|w| w[0].overall <= w[1].overall + 1e-6),
            "overall progress went backwards"
        );
        assert_eq!(seen.last().map(|p| p.overall), Some(1.));
        let seams = seen
            .iter()
            .filter(|p| p.stage == "Preparing · seam planning")
            .count();
        assert!(seams >= 3, "seam planning reported {seams} times");
        let p = seen
            .iter()
            .find(|p| p.stage == "Preparing · seam planning" && p.fraction > 0.)
            .unwrap();
        assert!(p.header().starts_with("Preparing · seam planning "));
        assert!(p.header().ends_with(" %"));
        assert!(p.detail.starts_with("Preparing cube20"), "{}", p.detail);
        assert!(p.overall < PREPARE_SPAN);
    }

    #[test]
    fn cancellation_during_preparation_stops_before_scheduling() {
        let c = config();
        let objects = objects(&c);
        let cancelled = AtomicBool::new(false);
        let reached_scheduling = AtomicBool::new(false);
        let result = slice(objects, c, &cancelled, &|p| {
            if p.stage == "Preparing · seam planning" {
                cancelled.store(true, Ordering::Relaxed);
            }
            if p.stage == "Scheduling" {
                reached_scheduling.store(true, Ordering::Relaxed);
            }
        });
        assert_eq!(
            result.err().as_deref(),
            Some(mica_core::progress::CANCELLED)
        );
        assert!(!reached_scheduling.load(Ordering::Relaxed));
    }
}
