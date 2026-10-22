// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/GCode.cpp :: GCodeGenerator::_do_export, GCodeGenerator::print_machine_envelope, GCodeGenerator::_print_first_layer_extruder_temperatures
// Source: prusaslicer@b028299c77 src/libslic3r/GCode.cpp :: GCodeGenerator::process_layer, GCodeGenerator::_print_first_layer_bed_temperature
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/GCodeWriter.cpp :: GCodeWriter::set_bed_temperature
// Source: prusaslicer@b028299c77 src/libslic3r/GCode.cpp :: GCodeGenerator::_extrude
// Source: orca@8500fcdcca src/libslic3r/GCode.cpp :: GCode::_do_export, GCode::process_layer, GCode::set_extruder
// Source: orca@8500fcdcca src/libslic3r/GCode.cpp :: GCode::get_highest_bed_temperature, GCode::_print_first_layer_bed_temperature
// Source: orca@8500fcdcca src/libslic3r/GCode/ToolOrdering.cpp :: ToolOrdering::handle_dontcare_extruder
// Source: orca@8500fcdcca src/libslic3r/GCodeWriter.cpp :: GCodeWriter::set_bed_temperature
// Changes: Klipper object exclusion follows the one Label objects setting (gcode_label_objects = firmware), as PrusaSlicer's LabelObjects does.
// Changes: Second-layer nozzle transition skips parked tools; bed selection follows layer tools, Prusa override or Orca highest-used-filament mode. Klipper skips startup bed tracking, so layer 2 emits M140 after the fan marker and acceleration reset without M190. An empty first layer takes the first printing layer's first tool (Orca's handle_dontcare_extruder).
// Source: snapmaker-orca@b1831e5dcb src/libslic3r/Utils.cpp :: header_slic3r_generated
// Source: orca@8500fcdcca src/libslic3r/GCodeWriter.cpp :: GCodeWriter::set_exhaust_fan
// Source: snapmaker-orca@b1831e5dcb src/libslic3r/GCode.cpp :: GCode::_do_export
// Changes: Exhaust fan after start/end G-code for air-filtering filaments (Orca and Snapmaker gating); unconditional end-of-print auxiliary fan off.
// Source: prusaslicer@b028299c77 src/libslic3r/GCode.cpp :: GCodeGenerator::extrude_smooth_path, GCodeGenerator::extrude_perimeters
// Changes: Delegate seam planning and optional scarf/wipe handling to dedicated seam modules.
// Changes: Shared Marlin 2 / Klipper single-object T0 pipeline; Orca startup, layer and shutdown order.
// Changes: Single-object Prusa templates also see extruded_volume/weight(_total) and a 255-entry is_extruder_used, as GCodeGenerator::_do_export provides.
// Changes: PreparedPrint supplies merged layers, per-path tools and labels through multi.rs. The program layout (envelope, labels, temperatures
// Changes: around the start G-code, preamble, filament G-code, layer-change templates) follows _do_export;
// Changes: per-role speed and acceleration (incl. gap fill) follow _extrude; moves are written by Mica's
// Changes: Structured per-tool cooling blocks and Mica writer instead of post-parsing G-code.
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/CoolingBuffer.cpp :: CoolingBuffer::parse_layer_gcode
// Changes: Cooling includes fixed retract, unretract and XYZ travel time; analytic smooth path lengths.
// Source: prusaslicer@b028299c77 src/libslic3r/GCode.cpp :: GCodeGenerator::change_layer, GCodeGenerator::travel_to_first_position
// Changes: Ramp from the float previous layer Z; travel_to_first_position's arrival after every ramping layer change.
// Changes: z_offset as PrusaSlicer 2.9.6 applies it (print_z, `;Z:`, layer_z and max_layer_z include it; the first move lifts from 2 x z_offset) and as Orca does (only the machine Z); absolute extrusion refused up front (mica_core::safety).
// Source: orca@8500fcdcca src/libslic3r/GCode/CoolingBuffer.cpp :: CoolingBuffer::parse_layer_gcode, CoolingBuffer::apply_layer_cooldown
// Source: orca@8500fcdcca src/libslic3r/GCode.cpp :: GCode::travel_to, GCode::extrude_loop
// Changes: Orca layer time starts at the first path of any role; each path's starting feedrate line is timed with the deretraction or last segment E; per-layer fan marker state.
// Changes: Orca plans the travel from the path end, before the wipe and the "move inwards before travel".
// Source: prusaslicer@b028299c77 src/libslic3r/GCode.cpp :: GCodeGenerator::travel_to, GCodeGenerator::change_layer, GCodeGenerator::process_layer, GCodeGenerator::process_layer_single_object, GCodeGenerator::gcode_to_point
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/GCodeWriter.cpp :: GCodeWriter::set_temperature
// Changes: Prusa retract-and-wipe at travels, layer changes and the end; the layer-change ramp starts at the wipe end.
// Changes: AvoidCrossingPerimeters one-shot modifiers (disable_once after skirt/brim, use_external_mp_once on a new
// Changes: object, use_external_mp for skirt/brim) and need_wipe; routes are replanned from the wipe end, which
// Changes: gcode_to_point truncates. Tool numbers on temperature commands only with several used extruders.
// Source: prusaslicer@b028299c77 src/libslic3r/PrintApply.cpp :: print_config_diffs
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/LabelObjects.cpp :: LabelObjects::update, LabelObjects::maybe_change_instance, LabelObjects::maybe_stop_instance, LabelObjects::all_objects_header
// Source: prusaslicer@b028299c77 src/libslic3r/GCode.cpp :: GCodeGenerator::generate_travel_gcode, GCodeGenerator::extrude_smooth_path
// Changes: One extruder: the object label changes as the travel's insert_gcode (after the first lift, before a
// Changes: travel's second-to-last point) and stays open across layers; skirt/brim layers and the end close it.
// Changes: Every travel_to sets the travel acceleration (the first one always); the print acceleration resets
// Changes: once per extrusion entity (contiguous pieces of a loop share it).
// Changes: G-code and templates see printer retraction values with the filament overrides applied.
// Source: orca@8500fcdcca src/libslic3r/GCode.cpp :: GCode::change_layer
// Changes: Retract on layer change off: Prusa retracts a rising layer change only if its XY path needs_retraction, Orca not at all.
// Source: prusaslicer@b028299c77 src/libslic3r/GCode.cpp :: GCodeGenerator::_extrude, GCodeGenerator::change_layer, GCodeGenerator::set_extruder
// Changes: Multi-tool jobs: after a toolchange without a tower the next path starts "after unknown position"; with several
// Changes: extruders instance labels change at the layer change, start after the unretract and stay open across layers.
// Source: orca@8500fcdcca src/libslic3r/GCode.cpp :: GCode::_do_export
// Changes: Sequential objects: the move between objects (sequential.rs), the between-objects template, object-local first layers
// Changes: with job-wide template layer numbers, and PrusaSlicer's object label held over an object's layers.
// Changes: Engine choices read mica_slicing_rules (mica_core::rules), not gcode_flavor or the presence of Orca keys (docs/slicing-rules.md).
// Changes: process_layer's ;Z: and ;HEIGHT: markers with six significant digits (PrusaSlicer float_to_string_decimal_point, Orca %g).
// Source: snapmaker-orca@b1831e5dcb src/libslic3r/GCode.cpp :: DoExport::update_print_stats_and_format_filament_stats, GCode::_do_export
// Changes: Orca-rule footer statistics in upstream order; the second pass fills per-slot weight/cost and gates their lines. Keep Prusa-rule output unchanged.
// Changes: Brick layers: raised walls at their own Z; the writer learns where raised walls stand.
// Changes: The time analysis reads the first-pass text on its own thread while it is written, instead of reading the finished file again.
// Source: prusaslicer@b028299c77 src/libslic3r/BuildVolume.cpp :: BuildVolume::all_paths_inside
// Changes: The printable-area check tests extrusion centerlines against the bed outline, Orca exclusion areas and the height (mica_core::build_volume) and names the feature, its object and where.
// Source: prusaslicer@b028299c77 src/libslic3r/GCode.cpp :: GCodeGenerator::_do_export, GCodeGenerator::process_layer, GCodeGenerator::change_layer, GCodeGenerator::travel_to_first_position
// Source: orca@8500fcdcca src/libslic3r/GCode.cpp :: GCode::_do_export, GCode::process_layer, GCode::change_layer
// Changes: Spiral vase: each layer's G-code from its layer change passes the SpiralVase post-processor; process_layer's enable condition; no ramping layer change on spiral layers; the move to the first point without travel acceleration; Orca moves to the layer Z in change_layer; the last layer's label stop after the post-processor.

//! Turns sliced layers into a complete G-code program, running the profile's G-code templates.

use std::fmt;

use mica_core::config::Config;
use mica_core::geometry::{Point, unscale};
use mica_core::mesh::TriangleMesh;
use mica_core::print::LayerSource;
use mica_core::settings::PrintSettings;
use mica_core::toolpath::Role;
use mica_template::{Context, Var};

use crate::cooling::{CoolingConfig, fan_speed, slow_down_multi};
use crate::templates::{config_context, run, set_layer_vars, sets_temperature};
use crate::writer::{MoveStats, Writer, decimal_g6, num, num_g6};
use crate::{Flavor, orca};

/// Layers fetched from the source at a time (computed in parallel within a batch).
const LAYER_BATCH: usize = 32;

#[derive(Debug)]
pub enum ExportError {
    /// An extrusion would leave the printable volume (bed outline, exclusion areas, height).
    OutOfBounds {
        layer: usize,
        /// What prints there: "the brim of Cube", "the skirt", "an external perimeter of Cube".
        what: String,
        x: f64,
        y: f64,
        z: f64,
        /// Where: "2.1 mm past the left edge of the bed", "in the excluded area …".
        reason: String,
    },
    Empty,
    /// A profile G-code template failed to evaluate.
    Template(String),
    Unsupported(String),
    /// Writing the output failed.
    Io(std::io::Error),
}

impl fmt::Display for ExportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExportError::OutOfBounds {
                layer,
                what,
                x,
                y,
                z,
                reason,
            } => write!(
                f,
                "layer {}: {what} is outside the printable volume at X {x:.3} Y {y:.3} \
                 Z {z:.3} ({reason}); move the object inside the printable area",
                layer + 1,
            ),
            ExportError::Empty => write!(f, "nothing to print"),
            ExportError::Template(e) => write!(f, "G-code template error in {e}"),
            ExportError::Unsupported(e) => write!(f, "{e}"),
            ExportError::Io(e) => write!(f, "writing G-code: {e}"),
        }
    }
}

impl std::error::Error for ExportError {}

#[derive(Debug)]
pub struct Exported {
    pub gcode: String,
    pub stats: MoveStats,
}

fn cooling_config(config: &Config, s: &PrintSettings) -> CoolingConfig {
    CoolingConfig::from_config(config, s)
}

/// The print acceleration commanded for a role (also read by the GUI preview's acceleration view).
pub fn role_acceleration(s: &PrintSettings, role: Role, first_layer: bool) -> f64 {
    if first_layer {
        return s.first_layer_acceleration;
    }
    match role {
        Role::ExternalPerimeter => s.external_perimeter_acceleration,
        Role::Perimeter => s.perimeter_acceleration,
        Role::OverhangPerimeter | Role::BridgeInfill | Role::InternalBridgeInfill => {
            s.bridge_acceleration
        }
        Role::InternalInfill => s.infill_acceleration,
        Role::SolidInfill => s.solid_infill_acceleration,
        Role::TopSolidInfill | Role::Ironing => s.top_solid_infill_acceleration,
        // PrusaSlicer has no gap fill acceleration: gap fill is neither perimeter nor infill, so it
        // falls through to the default.
        Role::Skirt
        | Role::Brim
        | Role::GapFill
        | Role::SupportMaterial
        | Role::SupportMaterialInterface => s.default_acceleration,
    }
}

/// PrusaSlicer's `LabelObjects` for the one object of a single-object print (the multi-object
/// path keeps its labels in [`crate::multi::Multi`]): M486 S0 for firmware labels, comments for
/// OctoPrint.
struct SingleLabel {
    firmware: bool,
    enabled: bool,
    /// `LabelObjects::init` OctoPrint name: "<name> id:0 copy 0".
    comment: String,
    open: bool,
}

impl SingleLabel {
    fn new(config: &Config, object_name: &str) -> Self {
        let style = mica_core::merged::label_objects(config);
        Self {
            firmware: config.string("gcode_label_objects") == Some("firmware"),
            enabled: style != mica_core::merged::LabelObjects::Disabled,
            comment: format!("{} id:0 copy 0", crate::multi::label_name(object_name)),
            open: false,
        }
    }
    fn start(&self) -> String {
        if self.firmware {
            "M486 S0\n".into()
        } else {
            format!("; printing object {}\n", self.comment)
        }
    }
    fn stop(&self) -> String {
        if self.firmware {
            "M486 S-1\n".into()
        } else {
            format!("; stop printing object {}\n", self.comment)
        }
    }
    /// `LabelObjects::maybe_change_instance` towards the object (`open`) or no object.
    fn change(&mut self, open: bool) -> String {
        if !self.enabled || self.open == open {
            return String::new();
        }
        self.open = open;
        if open { self.start() } else { self.stop() }
    }
}

/// `LabelObjects::maybe_change_instance` towards the label of `object` (`None`: none open).
fn label_change(
    multi: &mut Option<crate::multi::Multi>,
    single: &mut SingleLabel,
    object: Option<usize>,
    config: &Config,
) -> String {
    match multi {
        Some(m) => m.label_change(object, config),
        None => single.change(object.is_some()),
    }
}

/// `GCodeWriter::set_temperature` names the tool only when an extruder other than the first is used
/// (`multiple_extruders`); a single-filament XL print addresses the active tool implicitly.
fn tool_word(m: &crate::multi::Multi, tool: usize) -> String {
    if m.job.ordering.used.iter().any(|&t| t > 0) {
        format!(" T{tool}")
    } else {
        String::new()
    }
}

/// Orca's exhaust fan command (`GCodeWriter::set_exhaust_fan`) after the start G-code
/// (`during`) or after the end G-code: the highest speed among the used filaments that
/// activate air filtration. OrcaSlicer also needs the printer's `support_air_filtration` and the
/// filament's per-phase switch; Snapmaker Orca (which defines `filament_is_high_temperature` and
/// has no phase switches) checks neither. Prusa configs have no `activate_air_filtration`.
fn exhaust_fan(config: &Config, tools: &[usize], during: bool) -> Option<String> {
    let snapmaker = config.get("filament_is_high_temperature").is_some();
    if !snapmaker && config.bool("support_air_filtration") == Some(false) {
        return None;
    }
    let (phase, speed) = if during {
        (
            "activate_air_filtration_during_print",
            "during_print_exhaust_fan_speed",
        )
    } else {
        (
            "activate_air_filtration_on_completion",
            "complete_print_exhaust_fan_speed",
        )
    };
    tools
        .iter()
        .filter(|&&t| {
            config.bool_at("activate_air_filtration", t) == Some(true)
                && (snapmaker || config.bool_at(phase, t) != Some(false))
        })
        .map(|&t| config.int_at(speed, t).unwrap_or(0))
        .max()
        .map(|s| format!("M106 P3 S{}", (s as f64 / 100.0 * 255.0) as i64))
}

fn mm(p: Point) -> (f64, f64) {
    (unscale(p.x), unscale(p.y))
}

/// Whether the layer change retracts, for the active tool's (filament-overridden) configuration.
/// PrusaSlicer (`GCodeGenerator::change_layer`): with `retract_layer_change` off, a rising layer
/// change from a known position (`from`) retracts only when its XY path to the layer's first
/// point (`to`; avoid crossing perimeters on the layer below via `route`, else straight) is at
/// least `retract_before_travel` long (`needs_retraction`); otherwise it always retracts.
/// OrcaSlicer (`GCode::change_layer`): `retract_when_changing_layer` alone decides.
fn layer_change_retracts(
    (c, st): (&Config, &PrintSettings),
    from: Option<(f64, f64)>,
    to: Option<(f64, f64)>,
    route: impl FnOnce((f64, f64), (f64, f64)) -> Option<Vec<(f64, f64)>>,
) -> bool {
    if c.bool("retract_layer_change").unwrap_or(true) {
        return true;
    }
    if crate::orca_motion::enabled(c) {
        return false;
    }
    let (Some(from), Some(to)) = (from, to) else {
        return true;
    };
    let length = route(from, to).map_or_else(
        || (to.0 - from.0).hypot(to.1 - from.1),
        |r| crate::travels::route_length(&r),
    );
    length >= st.retract_before_travel
}

/// Bed rectangle (min x, min y, max x, max y) from `bed_shape`.
fn bed_bounds(config: &Config) -> (f64, f64, f64, f64) {
    let pts = config.points("bed_shape").unwrap_or_default();
    let fold = |f: fn(&(f64, f64)) -> f64, init: f64, op: fn(f64, f64) -> f64| {
        pts.iter().map(f).fold(init, op)
    };
    (
        fold(|p| p.0, f64::MAX, f64::min),
        fold(|p| p.1, f64::MAX, f64::min),
        fold(|p| p.0, f64::MIN, f64::max),
        fold(|p| p.1, f64::MIN, f64::max),
    )
}

/// Checks a layer's extrusions against the printable volume before its G-code is written,
/// as PrusaSlicer and Orca test the sliced toolpaths (`BuildVolume::all_paths_inside`): every
/// extrusion centerline point on the bed outline (rectangle, circle, polygon) within
/// `BedEpsilon`, at most the maximum print height, and under Orca rules outside the bed's
/// exclusion areas. `object_of` names the object a path (by index) belongs to.
fn check_bounds<'a>(
    layer: &mica_core::toolpath::LayerToolpaths,
    volume: &mica_core::build_volume::BuildVolume,
    object_of: impl Fn(usize) -> Option<&'a str>,
) -> Result<(), ExportError> {
    for (i, path) in layer.paths.iter().enumerate() {
        let z = layer.print_z + path.brick.map_or(0., |b| b.raise);
        for &p in &path.points {
            let (x, y) = mm(p);
            if let Err(outside) = volume.check_path_point(x, y, z) {
                return Err(ExportError::OutOfBounds {
                    layer: layer.index,
                    what: path_description(path.role, object_of(i)),
                    x,
                    y,
                    z,
                    reason: outside.describe(),
                });
            }
        }
    }
    Ok(())
}

/// "the brim of Cube", "the skirt", "an external perimeter of Cube".
fn path_description(role: Role, object: Option<&str>) -> String {
    let kind = match role {
        Role::Skirt => return "the skirt".into(),
        Role::Brim => "the brim".to_string(),
        Role::SupportMaterial => "support".into(),
        Role::SupportMaterialInterface => "a support interface".into(),
        other => {
            let name = other.name().to_lowercase();
            let article = if name.starts_with(['a', 'e', 'i', 'o', 'u']) {
                "an"
            } else {
                "a"
            };
            format!("{article} {name}")
        }
    };
    match object {
        Some(o) if !o.is_empty() => format!("{kind} of {o}"),
        _ => kind,
    }
}

/// Machine limits header (`print_machine_envelope`), for Marlin 2 when limits are emitted to G-code.
fn machine_envelope(c: &Config) -> String {
    if c.string("gcode_flavor") != Some("marlin2")
        || c.string("machine_limits_usage") != Some("emit_to_gcode")
    {
        return String::new();
    }
    let f = |k: &str| c.float(k).unwrap_or(0.0);
    let i = |k: &str| (f(k) + 0.5) as i64;
    let mut s = format!(
        "M201 X{} Y{} Z{} E{} ; sets maximum accelerations, mm/sec^2\n",
        i("machine_max_acceleration_x"),
        i("machine_max_acceleration_y"),
        i("machine_max_acceleration_z"),
        i("machine_max_acceleration_e")
    );
    s += &format!(
        "M203 X{} Y{} Z{} E{} ; sets maximum feedrates, mm / sec\n",
        i("machine_max_feedrate_x"),
        i("machine_max_feedrate_y"),
        i("machine_max_feedrate_z"),
        i("machine_max_feedrate_e")
    );
    s += &format!(
        "M204 P{} R{} T{} ; sets acceleration (P, T) and retract acceleration (R), mm/sec^2\n",
        i("machine_max_acceleration_extruding"),
        i("machine_max_acceleration_retracting"),
        i("machine_max_acceleration_travel")
    );
    s += &format!(
        "M205 X{:.2} Y{:.2} Z{:.2} E{:.2} ; sets the jerk limits, mm/sec\n",
        f("machine_max_jerk_x"),
        f("machine_max_jerk_y"),
        f("machine_max_jerk_z"),
        f("machine_max_jerk_e")
    );
    let jd = f("machine_max_junction_deviation");
    if jd > 0.0 {
        s += &format!("M205 J{jd:.3} ; sets the junction deviation, mm\n");
    }
    s += &format!(
        "M205 S{} T{} ; sets the minimum extruding and travel feed rate, mm/sec\n",
        i("machine_min_extruding_rate"),
        i("machine_min_travel_rate")
    );
    s
}

/// Variables PrusaSlicer sets before the start G-code (`_do_export`).
fn set_print_vars(ctx: &mut Context, config: &Config, obj: &dyn LayerSource) {
    let bed = bed_bounds(config);
    let extruders = config.vector_len("nozzle_diameter").max(1);
    ctx.set("initial_tool", Var::int(0));
    ctx.set("initial_extruder", Var::int(0));
    ctx.set("current_extruder", Var::int(0));
    ctx.set("total_layer_count", Var::int(obj.layer_count() as i64));
    ctx.set("current_object_idx", Var::int(0));
    ctx.set("has_wipe_tower", Var::bool(false));
    ctx.set(
        "has_single_extruder_multi_material_priming",
        Var::bool(false),
    );
    ctx.set("total_toolchanges", Var::int(0));
    ctx.set("print_bed_min", Var::floats([bed.0, bed.1]));
    ctx.set("print_bed_max", Var::floats([bed.2, bed.3]));
    ctx.set(
        "print_bed_size",
        Var::floats([bed.2 - bed.0, bed.3 - bed.1]),
    );
    if let Some(bb) = obj.first_layer_bbox() {
        let (min, max) = (mm(bb.min), mm(bb.max));
        ctx.set("first_layer_print_min", Var::floats([min.0, min.1]));
        ctx.set("first_layer_print_max", Var::floats([max.0, max.1]));
        ctx.set(
            "first_layer_print_size",
            Var::floats([max.0 - min.0, max.1 - min.1]),
        );
    }
    if let Some(hull) = obj.first_layer_hull() {
        ctx.set(
            "first_layer_print_convex_hull",
            Var::strings(
                hull.iter()
                    .map(|p| format!("[{}, {}]", unscale(p.x), unscale(p.y))),
            ),
        );
    }
    ctx.set("num_extruders", Var::int(extruders as i64));
    // Current Z lift (read-only in PrusaSlicer); no lift is active around custom G-code.
    ctx.set("zhop", Var::float(0.0));
    ctx.set(
        "is_extruder_used",
        crate::multi::is_extruder_used(config, extruders, |i| i == 0),
    );
    // Writable outputs, read back by PrusaSlicer after custom G-code.
    ctx.set_output("position", Var::floats([0.0, 0.0, 0.0]));
    ctx.set_output("e_retracted", Var::floats(vec![0.0; extruders]));
    ctx.set_output("e_restart_extra", Var::floats(vec![0.0; extruders]));
}

/// Writes a complete program for one object printed with the first extruder, in memory
/// (for tests and small prints; `export_file` streams to disk).
pub fn export_gcode(
    obj: &dyn LayerSource,
    config: &Config,
    s: &PrintSettings,
    object_name: &str,
) -> Result<Exported, ExportError> {
    export_in_memory(obj, config, s, object_name, None)
}

fn export_in_memory(
    obj: &dyn LayerSource,
    config: &Config,
    s: &PrintSettings,
    object_name: &str,
    mesh: Option<&TriangleMesh>,
) -> Result<Exported, ExportError> {
    let thumbnails = mesh
        .map(|m| crate::thumbnails::generate(config, m))
        .transpose()
        .map_err(ExportError::Io)?
        .unwrap_or_default();
    let mut first = Vec::new();
    let stats = export(obj, config, s, object_name, mesh, &thumbnails, &mut first)?;
    let mut gcode = Vec::with_capacity(first.len());
    let stats = postprocess(&mut std::io::Cursor::new(first), &mut gcode, stats, config)
        .map_err(ExportError::Io)?;
    Ok(Exported {
        gcode: String::from_utf8(gcode).expect("G-code is UTF-8"),
        stats,
    })
}

/// Supply the positioned source mesh for profile thumbnails and Orca's EXCLUDE_OBJECT polygon.
pub fn export_gcode_with_mesh(
    obj: &dyn LayerSource,
    config: &Config,
    s: &PrintSettings,
    object_name: &str,
    mesh: &TriangleMesh,
) -> Result<Exported, ExportError> {
    export_in_memory(obj, config, s, object_name, Some(mesh))
}

/// The profile mapper has already resolved Orca's plate-specific temperatures to these keys.
fn layer_bed_temperature(
    config: &Config,
    first_tool: usize,
    used_tools: &[usize],
    first_layer: bool,
) -> f64 {
    let key = if first_layer {
        "first_layer_bed_temperature"
    } else {
        "bed_temperature"
    };
    let temperature = |tool| config.float_at(key, tool).unwrap_or(0.);
    if mica_core::rules::Rules::of(config).is_orca() {
        if config
            .string("bed_temperature_formula")
            .unwrap_or("by_highest_temp")
            == "by_highest_temp"
        {
            return used_tools
                .iter()
                .map(|&tool| temperature(tool))
                .fold(0., f64::max);
        }
    } else {
        let tool = config.int("bed_temperature_extruder").unwrap_or(0);
        if tool > 0 && tool as usize <= config.vector_len("nozzle_diameter") {
            return temperature(tool as usize - 1);
        }
    }
    temperature(first_tool)
}

fn export(
    obj: &dyn LayerSource,
    config: &Config,
    s: &PrintSettings,
    object_name: &str,
    mesh: Option<&TriangleMesh>,
    thumbnails: &[crate::bgcode::Thumbnail],
    sink: &mut dyn std::io::Write,
) -> Result<MoveStats, ExportError> {
    if obj.layer_count() == 0 {
        return Err(ExportError::Empty);
    }
    // One answer for every path (one object, a plate, Klipper, several tools): the writer
    // only writes relative E.
    if let Some(problem) = mica_core::safety::extrusion_mode_problem(config) {
        return Err(ExportError::Unsupported(problem));
    }
    // Like PrusaSlicer's print_config_diffs, G-code generation and templates see the printer
    // retraction values with the filament overrides applied.
    let full_config = config;
    let overridden = mica_core::filament_overrides::apply(config);
    let config = &overridden;
    let flavor = match config.string("gcode_flavor").unwrap_or("") {
        "marlin2" => Flavor::PrusaBuddy,
        "klipper" => Flavor::Klipper,
        flavor => {
            return Err(ExportError::Unsupported(format!(
                "G-code flavor '{flavor}' is not supported (Marlin 2 and Klipper only)"
            )));
        }
    };
    // Firmware dialect (commands, file layout) follows gcode_flavor; engine behaviour follows
    // mica_slicing_rules. docs/slicing-rules.md classifies every branch below.
    let klipper = flavor == Flavor::Klipper;
    let orca = mica_core::rules::Rules::of(config).is_orca();
    // Orca-format templates (Orca variable names, writer-backed placeholders): Orca rules or a
    // Klipper printer (whose profiles come from Orca).
    let orca_templates = klipper || orca;
    let mut multi = obj
        .print_job()
        .map(|job| crate::multi::Multi::new(job, config))
        .transpose()?;
    let is_multi = multi.is_some();
    // Objects printed one after another (complete_objects / "by object").
    let sequential = obj.print_job().is_some_and(|j| j.is_sequential());
    let initial = multi.as_ref().map_or(0, |m| m.active);
    let used_tools = multi
        .as_ref()
        .map_or(&[0][..], |m| m.job.ordering.used.as_slice());
    // An empty first layer (an object resting on an edge or a point slices to nothing there)
    // takes the first tool of the first layer that prints, as Orca's
    // ToolOrdering::handle_dontcare_extruder fills layer 0 from the first layer with slices.
    // Without it the first-layer bed temperature over no tools was 0 °C (GUI monkey test).
    let first_layer_tools = multi.as_ref().map_or(&[0][..], |m| {
        let layers = &m.job.ordering.layers;
        match layers.first() {
            Some(l) if l.extruders.is_empty() => layers
                .iter()
                .find_map(|l| l.extruders.first())
                .map_or(&[][..], std::slice::from_ref),
            Some(l) => l.extruders.as_slice(),
            None => &[][..],
        }
    });
    let first_layer_bed_temperature =
        layer_bed_temperature(config, initial, first_layer_tools, true);
    // One Label objects setting: firmware-specific labels are EXCLUDE_OBJECT on Klipper.
    let exclude = !is_multi
        && klipper
        && mica_core::merged::label_objects(config) == mica_core::merged::LabelObjects::Firmware;
    let label = orca::object_name(object_name);
    // PrusaSlicer rules with one extruder (GCodeWriter::multiple_extruders false): an object's
    // label opens in the travel to its first extrusion (LabelObjects::maybe_change_instance as
    // the travel's insert_gcode) and stays open across layers until another object, a skirt or
    // brim, or the end of the print.
    let travel_labels = !orca
        && !klipper
        && mica_core::merged::label_objects(config) != mica_core::merged::LabelObjects::Disabled
        && multi.as_ref().is_none_or(|m| !m.multiple_extruders());
    let mut single_label = SingleLabel::new(config, object_name);
    let hull = if exclude {
        let mesh = mesh.ok_or_else(|| {
            ExportError::Unsupported(
                "Klipper object exclusion requires the source mesh; use export_gcode_with_mesh"
                    .into(),
            )
        })?;
        let hull = orca::object_hull(mesh);
        if hull.len() < 3 {
            return Err(ExportError::Empty);
        }
        hull
    } else {
        Vec::new()
    };
    let volume = mica_core::build_volume::BuildVolume::from_config(config);
    let job_objects = obj.print_job().map(|j| &j.objects);

    let mut ctx = config_context(config);
    set_print_vars(&mut ctx, config, obj);
    if orca_templates {
        orca::set_print_vars(&mut ctx, config, obj, object_name, mesh, s);
        // Orca templates branch on print_sequence; Mica edits it as complete_objects.
        if config.raw("print_sequence").is_some() {
            ctx.set(
                "print_sequence",
                Var::string(if mica_core::sequential::enabled(config) {
                    "by object"
                } else {
                    "by layer"
                }),
            );
        }
    }
    if let Some(m) = &multi {
        m.set_vars(&mut ctx, config);
    }
    if orca_templates {
        ctx.set(
            "bed_temperature_initial_layer_single",
            Var::int(first_layer_bed_temperature as i64),
        );
    }
    let template = |key: &str| config.string(key).unwrap_or("").to_string();
    let template_at = |key: &str| config.string_at(key, initial).unwrap_or("").to_string();
    let tpl_err = ExportError::Template;
    let gcode_resolution =
        mica_core::settings::gcode_resolution(config, 0.0125) * mica_core::geometry::SCALE;
    let arc_mode = crate::smooth_path::ArcFitting::from_config(config);
    let process = |name: &str, tpl: &str, ctx: &mut Context, writer: &mut Writer| {
        if is_multi {
            crate::multi::run(name, tpl, ctx, writer, config)
        } else if orca_templates {
            orca::run(name, tpl, ctx, writer, config)
        } else {
            let n = config.vector_len("nozzle_diameter").max(1);
            crate::multi::set_extruded(ctx, writer, config, &writer.tool_states(n));
            run(name, tpl, ctx)
        }
    };

    let filament_area = std::f64::consts::PI * (s.filament_diameter / 2.0).powi(2);
    let cooling = cooling_config(config, s);

    let mut w = Writer::with_config(flavor, config);
    if is_multi {
        w.select_tool(initial, config);
    }
    if klipper {
        w.raw("; HEADER_BLOCK_START");
        w.raw(&crate::identity::line(config).map_err(ExportError::Io)?);
        w.raw(&format!("; total layer number: {}", obj.layer_count()));
        for (key, label) in [
            ("filament_density", "filament_density"),
            ("filament_diameter", "filament_diameter"),
        ] {
            w.raw(&format!(
                "; {label}: {}",
                config.raw(key).unwrap_or_default()
            ));
        }
        w.raw(&format!(
            "; max_z_height: {:.2}\n; filament: 1\n; HEADER_BLOCK_END",
            obj.max_print_z()
        ));
    } else {
        w.raw(&crate::identity::line(config).map_err(ExportError::Io)?);
        w.raw(";");
    }
    if !thumbnails.is_empty() {
        let mut blocks = Vec::new();
        crate::thumbnails::write_text(&mut blocks, thumbnails, klipper).map_err(ExportError::Io)?;
        w.raw(std::str::from_utf8(&blocks).expect("thumbnail comments are ASCII"));
    }
    for (label, width) in [
        ("external perimeters", s.external_perimeter_extrusion_width),
        ("perimeters", s.perimeter_extrusion_width),
        ("infill", s.infill_extrusion_width),
        ("solid infill", s.solid_infill_extrusion_width),
        ("top infill", s.top_infill_extrusion_width),
        ("first layer", s.first_layer_extrusion_width),
    ] {
        w.raw(&format!("; {label} extrusion width = {width:.2}mm"));
    }
    w.raw("");
    w.raw(crate::time_estimate::FIRST_M73);
    let envelope = machine_envelope(config);
    if !envelope.is_empty() {
        w.raw(&envelope);
    }
    if klipper {
        w.raw("; EXECUTABLE_BLOCK_START");
        if exclude {
            w.raw(&orca::object_definition(&label, &hull));
        }
        w.set_fan(0.0);
        if config.bool("auxiliary_fan").unwrap_or(false) {
            w.set_auxiliary_fan(0.0);
        }
    } else if !is_multi && config.string("gcode_label_objects") == Some("firmware") {
        let name = crate::multi::label_name(object_name);
        w.raw(&format!("M486 S0\nM486 A{name}\nM486 S-1\n"));
    } else if !is_multi && travel_labels {
        // LabelObjects::all_objects_header, OctoPrint comments.
        w.raw(&format!("{}{}", single_label.start(), single_label.stop()));
    }

    if let Some(m) = &multi {
        m.definitions(&mut w, config);
    }

    // Start G-code, with PrusaSlicer's automatic temperature commands around it where the start
    // G-code doesn't set them itself.
    let start =
        process("start_gcode", &template("start_gcode"), &mut ctx, &mut w).map_err(tpl_err)?;
    let autoemit = !orca && config.bool("autoemit_temperature_commands").unwrap_or(true);
    let (bed_set, bed_by_gcode) = sets_temperature(&start, 140, 190);
    // Prusa records the custom startup target when autoemit is enabled. Orca's
    // corresponding override is disabled (#if 0), so it remembers the planned target.
    let last_bed_temperature = if orca && klipper {
        // _do_export skips _print_first_layer_bed_temperature for Klipper; the
        // writer keeps its initial target (0), even when custom startup sets the bed.
        0.0
    } else if autoemit {
        bed_by_gcode
            .filter(|&t| (0..1000).contains(&t))
            .map_or(first_layer_bed_temperature, |t| t as f64)
    } else {
        first_layer_bed_temperature
    };
    let (nozzle_set, _) = sets_temperature(&start, 104, 109);
    if autoemit && !bed_set && first_layer_bed_temperature > 0.0 {
        w.raw(&format!(
            "M190 S{} ; set bed temperature and wait for it to be reached",
            num(first_layer_bed_temperature, 0)
        ));
    }
    if autoemit && !nozzle_set && s.first_layer_temperature > 0.0 {
        if let Some(m) = &multi {
            for &tool in &m.job.ordering.used {
                w.raw(&format!(
                    "M104 S{}{} ; set temperature",
                    num(m.tools[tool].settings.first_layer_temperature, 0),
                    tool_word(m, tool)
                ));
            }
        } else {
            w.raw(&format!(
                "M104 S{} ; set temperature",
                num(s.first_layer_temperature, 0)
            ));
        }
    }
    w.raw(";TYPE:Custom");
    w.raw(&start);
    if autoemit && !nozzle_set && s.first_layer_temperature > 0.0 {
        if let Some(m) = &multi {
            w.raw(&format!(
                "M109 S{} T{initial} ; set temperature and wait for it to be reached",
                num(m.tools[initial].settings.first_layer_temperature, 0)
            ));
        } else {
            w.raw(&format!(
                "M109 S{} ; set temperature and wait for it to be reached",
                num(s.first_layer_temperature, 0)
            ));
        }
    }
    if orca && let Some(fan) = exhaust_fan(config, used_tools, true) {
        w.raw(&fan);
    }
    w.raw(if klipper {
        "G90\nG21"
    } else {
        "G21 ; set units to millimeters\nG90 ; use absolute coordinates"
    });
    w.raw(if config.bool("use_relative_e_distances").unwrap_or(true) {
        "M83 ; use relative distances for extrusion"
    } else {
        "M82 ; use absolute distances for extrusion"
    });
    if let Some(m) = &multi {
        m.initialize(&mut ctx, &mut w, config)?;
    }
    ctx.set("filament_extruder_id", Var::int(initial as i64));
    set_layer_vars(&mut ctx, if orca_templates { -1 } else { 0 }, 0.0, 0.0);
    let filament_start = process(
        "start_filament_gcode",
        &template_at("start_filament_gcode"),
        &mut ctx,
        &mut w,
    )
    .map_err(tpl_err)?;
    w.raw(&filament_start);
    if obj.calibration_control(0, 0).is_some() {
        crate::calib::after_start(
            &mut w,
            &start,
            &filament_start,
            config.bool("use_relative_e_distances").unwrap_or(true),
            initial,
        );
    }
    if let Some(m) = &multi
        && !orca
        && config.bool("ooze_prevention").unwrap_or(false)
    {
        w.raw(&format!(
            "M109 S{} T{initial} ; set temperature and wait for it to be reached",
            num(m.tools[initial].settings.first_layer_temperature, 0)
        ));
    }
    if klipper
        && config
            .bool_at("enable_pressure_advance", initial)
            .unwrap_or(false)
    {
        w.set_pressure_advance(config.float_at("pressure_advance", initial).unwrap_or(0.0));
    }
    if orca {
        // The first layer's change_extruder_set_fan writes both fans next (CoolingBuffer
        // starts without a fan speed).
        w.raw(&format!("T{initial}"));
    } else {
        w.set_fan(0.0);
    }
    // GCodeGenerator::preamble's silent travel_to_z(z_offset), then the first change_layer's
    // `position.z += z_offset`: PrusaSlicer's first move starts from 2 × z_offset when that is
    // above zero, else from the first layer's Z (process_layer's `previous_z`).
    let z_offset = w.z_offset();
    let z_after_start = if !orca && 2. * z_offset > f64::EPSILON {
        z_offset
    } else {
        obj.layer_z(0).0
    };
    w.forget_position(z_after_start);
    // PrusaSlicer's print_z includes z_offset (process_layer): the `;Z:` marker, the first
    // layer's `;HEIGHT:`, `layer_z` and `max_layer_z` in templates and the metadata carry it.
    // Orca's print_z does not (only the writer's Z moves do).
    let print_z_shift = if orca { 0. } else { z_offset };
    if let Some(m) = &mut multi {
        m.tower_prime(&mut ctx, &mut w, config)?;
    }

    let mut last_type: Option<Role> = None;
    let mut last_width: Option<f64> = None;
    let mut max_layer_z = 0.0f64;
    // GCodeGenerator::m_last_layer_z: the previous layer's print Z.
    let mut last_layer_z = 0.0f64;
    // The bed target last written (a sequential object's first layer sets it back).
    let mut bed_target = s.first_layer_bed_temperature;
    // Layers arrive in batches from the source; only one batch of toolpaths exists at a time.
    let layer_count = obj.layer_count();
    let mut batch: Vec<mica_core::print_job::ScheduledLayer> = Vec::new();
    let mut prepared_batch = Vec::new();
    let overhang_pairs = crate::orca_motion::OverhangPairs::new(layer_count);
    let batch_parameters = crate::export_batch::Parameters {
        source: obj,
        config,
        settings: s,
        tolerance: gcode_resolution,
        mode: arc_mode,
        overhang_pairs: &overhang_pairs,
    };
    let mut batch_start = 0usize;
    let mut any_paths = false;
    // Per-layer custom G-code (colour changes, pauses, custom and template G-code).
    let custom_plan = crate::custom_gcode::Plan::new(obj, config, orca);
    let mut dynamic_fan = None;
    // GCodeGenerator::last_position retains the unquantized toolpath endpoint, whereas the
    // writer tracks the rounded extrusion coordinates. Layer-change ramps start at the former.
    let mut last_path_position = None;
    let mut orca_wipe = crate::orca_motion::Wipe::default();
    // The last path's end and the position after its "move inwards before travel".
    let mut inward: Option<((f64, f64), (f64, f64))> = None;
    // PrusaSlicer's retract-and-wipe along the last extrusion (Prusa motion path only).
    let mut prusa_wipe = crate::prusa_wipe::Wipe::default();
    // AvoidCrossingPerimeters' one-shot modifiers and GCodeGenerator::last_position (unquantized).
    let mut acp = AcpState::default();
    let mut acp_from: Option<(f64, f64)> = None;
    // Spiral vase: every layer's G-code passes through the post-processor (SpiralVase).
    let mut spiral = crate::spiral_vase::SpiralVase::new(config);
    // The next batch is fetched and prepared on a helper thread while this thread emits the current
    // one. Preparation only depends on the immutable source and on the overhang (curl) history as
    // left by the previous batch, which travels with the task; emission never touches either, so the
    // batches are prepared in the same order with the same state as a serial fetch.
    let spiral_print = spiral.is_some();
    let fetch = |range: std::ops::Range<usize>, mut overhangs: crate::export_batch::Carry| {
        let mut batch = obj.scheduled_toolpaths(range.clone());
        let prepared = batch_parameters.prepare(&mut batch, range.start, &mut overhangs);
        if spiral_print {
            // The spiral layers as they print (their loops start where the layer below
            // ended), for the preview.
            for (k, scheduled) in batch.iter().enumerate() {
                let li = range.start + k;
                if spiral_candidate(s, scheduled, obj.layer_outlines(li)) {
                    obj.spiral_layer(li, scheduled, li + 1 == layer_count);
                }
            }
        }
        (batch, prepared, overhangs)
    };
    let fetch = &fetch;
    std::thread::scope(|scope| -> Result<(), ExportError> {
        let mut next = Some(scope.spawn(move || {
            fetch(
                0..LAYER_BATCH.min(layer_count),
                crate::export_batch::Carry::default(),
            )
        }));
        for li in 0..layer_count {
            if obj.is_cancelled() {
                return Err(ExportError::Io(std::io::Error::new(
                    std::io::ErrorKind::Interrupted,
                    "slicing canceled",
                )));
            }
            if li >= batch_start + batch.len() {
                batch_start = li;
                let (fetched, prepared, overhangs) = next
                    .take()
                    .expect("a batch is always prefetched")
                    .join()
                    .expect("batch preparation panicked");
                batch = fetched;
                prepared_batch = prepared;
                let end = li + batch.len();
                if end < layer_count {
                    next = Some(scope.spawn(move || {
                        fetch(end..(end + LAYER_BATCH).min(layer_count), overhangs)
                    }));
                }
            }
            let scheduled = &batch[li - batch_start];
            let prepared = std::mem::take(&mut prepared_batch[li - batch_start]);
            let layer = &scheduled.layer;
            if let Some(volume) = &volume {
                check_bounds(layer, volume, |i| match job_objects {
                    Some(objects) => scheduled
                        .assignments
                        .get(i)
                        .and_then(|a| objects.get(a.object))
                        .map(|o| o.name.as_str()),
                    None => Some(object_name),
                })?;
            }
            any_paths |= !layer.paths.is_empty();
            w.drain_to(sink).map_err(ExportError::Io)?;
            let active_settings = multi.as_ref().map(|m| m.tools[m.active].settings.clone());
            let s = active_settings.as_ref().unwrap_or(s);
            let calibrated_layer_settings =
                crate::calib::layer_settings(s, obj.calibration_control(li, 0));
            let s = calibrated_layer_settings.as_ref();
            let first = layer.index == 0;
            // Template layer number: layers printed before this one, over all objects.
            let layer_num = if sequential { li } else { layer.index } as i64;
            let z = layer.print_z;
            let travel_speed = if orca && first {
                config
                    .float("initial_layer_travel_speed")
                    .unwrap_or(s.travel_speed)
            } else {
                s.travel_speed
            };
            let travel_speed_z = if s.travel_speed_z == 0.0 {
                travel_speed
            } else {
                s.travel_speed_z
            };
            // Sequential printing: the next object starts from its own first layer once the
            // head is above everything printed (GCode::_do_export, complete_objects).
            let object_start = obj
                .print_job()
                .and_then(|j| j.object_start(li))
                .filter(|start| start.ordinal > 0);
            if let (Some(start), Some(m)) = (object_start, &mut multi) {
                // _do_export: retract_and_wipe before the move to the next object, then
                // disable_once for the travel to its first point.
                if let Some(plan) = tower_wipe_plan(m, &prusa_wipe, &w) {
                    let t = &m.tools[m.active];
                    crate::prusa_wipe::emit(&mut w, &plan, t.settings.retract_speed);
                }
                prusa_wipe.reset();
                acp.disabled_once = true;
                acp_from = None;
                let t = &m.tools[m.active];
                if crate::orca_motion::enabled(&t.config) {
                    orca_wipe.retract(&mut w, &t.config, &t.settings);
                } else {
                    m.retract_current(&mut w);
                }
                m.end_label(&mut w, config);
                let center = m.job.instance_center(start.object);
                let t = &m.tools[m.active];
                crate::sequential::travel_to_next_object(
                    &mut w,
                    &t.config,
                    &t.settings,
                    orca,
                    center,
                    max_layer_z,
                );
                last_path_position = None;
                ctx.set("current_object_idx", Var::int(start.ordinal as i64));
                // Orca calls it printing_by_object_gcode.
                let tpl = config
                    .string("between_objects_gcode")
                    .filter(|t| !t.is_empty())
                    .or_else(|| config.extra("printing_by_object_gcode"))
                    .unwrap_or("")
                    .to_string();
                let between =
                    process("between_objects_gcode", &tpl, &mut ctx, &mut w).map_err(tpl_err)?;
                let first = m.job.ordering.layers[li]
                    .extruders
                    .first()
                    .copied()
                    .unwrap_or(m.active);
                crate::sequential::first_layer_temperatures(
                    &mut w,
                    config,
                    &between,
                    first,
                    &m.job.ordering.used,
                    orca,
                    &mut bed_target,
                );
                if !between.is_empty() {
                    w.raw(&between);
                }
            }

            // process_layer: whether this layer becomes a spiral.
            let spiral_on = spiral.is_some()
                && spiral_layer_enabled(s, scheduled, obj.layer_outlines(li), multi.as_ref(), li);
            let travel_timer = mica_core::timing::Span::new(mica_core::timing::Stage::ExportTravel);
            let mut travel_geometry = prepared.travel;
            let acp_geometry = |i: usize| acp_layer_geometry(obj, i, s);
            let acp_current = acp_geometry(li);
            let acp_previous = li.checked_sub(1).and_then(acp_geometry);
            let acp_external_width = acp_flows(s, li, layer.height).1;
            let travels = crate::travels::TravelPlanner::new(
                config,
                layer,
                obj.previous_layer(li).and_then(|i| obj.layer_outlines(i)),
                obj.layer_outlines(li),
            )
            .with_source_cached(obj, li, &mut travel_geometry, Some(&overhang_pairs))
            .with_prusa_avoid_crossing(config, acp_current, acp_previous, acp_external_width)
            .spiral_layer(spiral_on);
            let tool_travels = multi.as_ref().map(|m| {
                m.tools
                    .iter()
                    .map(|t| {
                        crate::travels::TravelPlanner::new(
                            &t.config,
                            layer,
                            obj.previous_layer(li).and_then(|i| obj.layer_outlines(i)),
                            obj.layer_outlines(li),
                        )
                        .with_source_cached(obj, li, &mut travel_geometry, Some(&overhang_pairs))
                        .with_prusa_avoid_crossing(
                            &t.config,
                            acp_current,
                            acp_previous,
                            acp_external_width,
                        )
                        .spiral_layer(spiral_on)
                    })
                    .collect::<Vec<_>>()
            });
            drop(travel_timer);
            let mut plan = crate::overhang_plan::LayerPlan::new(
                layer,
                |i| {
                    multi.as_ref().map_or((config, s, 0), |m| {
                        let a = scheduled.assignments[i];
                        let (c, s) = m.job.objects[a.object].path_config(a.region, a.tool);
                        (c, s, a.tool)
                    })
                },
                prepared.curves,
                &mut dynamic_fan,
            );
            plan.calibrate(obj, li);
            let _emit_timer = mica_core::timing::Span::new(mica_core::timing::Stage::ExportEmit);
            // process_layer: a new object starts with the external planner (use_external_mp_once).
            let path_object = |i: usize| scheduled.assignments.get(i).map_or(0, |a| a.object);
            if !layer.paths.is_empty() && acp.current_object != Some(path_object(0)) {
                acp.external_once = true;
                acp.current_object = Some(path_object(0));
            }
            let previous_layer_z = std::mem::replace(&mut last_layer_z, z);
            let layer_retract = layer_change_retracts(
                multi.as_ref().map_or((config, s), |m| {
                    (&m.tools[m.active].config, &m.tools[m.active].settings)
                }),
                // last_position: unknown on the first layer.
                last_path_position
                    .or_else(|| w.position())
                    .filter(|_| !first && z > previous_layer_z),
                layer.paths.first().map(|p| mm(p.points[0])),
                |from, to| {
                    (!acp.disabled_once)
                        .then(|| travels.prusa_route(from, to, acp.external_once, true))
                        .flatten()
                        .map(|r| r.0)
                },
            );
            // GCodeGenerator::change_layer: retract_and_wipe first, then the ramp starts where the
            // wipe ended.
            let (layer_wipe, wipe_retract_speed, wipe_retract_length) = {
                let (c, st) = multi.as_ref().map_or((config, s), |m| {
                    (&m.tools[m.active].config, &m.tools[m.active].settings)
                });
                let plan = crate::prusa_wipe::Params::new(c, st)
                    .filter(|_| layer_retract && prusa_wipe.has_path())
                    .zip(w.position())
                    .map(|(p, from)| prusa_wipe.plan(from, w.retracted(), st.retract_length, &p));
                (plan, st.retract_speed, st.retract_length)
            };
            let wipe_end = layer_wipe.as_ref().and_then(|plan| plan.end());
            let layer_change = wipe_end
                .or(last_path_position)
                .or_else(|| w.position())
                .and_then(|(x, y)| {
                    layer.paths.first().and_then(|p| {
                        let to = mm(p.points[0]);
                        // get_layer_change_xy_path: avoidance planned on the previous layer, from
                        // last_position (after a wipe, its G-code position converted back).
                        let start = if wipe_end.is_some() {
                            gcode_to_point((x, y), obj.object_origin(path_object(0)))
                        } else {
                            (x, y)
                        };
                        let route = (!acp.disabled_once)
                            .then(|| travels.prusa_route(start, to, acp.external_once, true))
                            .flatten()
                            .map(|mut r| {
                                r.0[0] = (x, y);
                                r.0
                            });
                        // change_layer ramps from previous_layer_z (m_last_layer_z, a float).
                        let from_z = if layer.index == 0 || travels.is_orca() {
                            w.z()
                        } else {
                            previous_layer_z
                        };
                        travels.layer_change_along([x, y, from_z], to, route)
                    })
                });
            // CoolingBuffer includes all non-adjustable motion, including E-only moves and Z.
            let mut travel_estimate =
                if let (Some(points), Some((x, y))) = (&layer_change, wipe_end.or(w.position())) {
                    crate::travels::duration([x, y, w.z()], points, travel_speed, travel_speed_z)
                } else {
                    (z - w.z()).abs() / travel_speed_z
                };
            if let Some(plan) = &layer_wipe {
                // The wipe replaces part of the layer-change retraction counted below.
                travel_estimate +=
                    plan.duration(wipe_retract_speed) - wipe_retract_length / wipe_retract_speed;
            }
            // The cooling estimate replays the wipes of this layer's travels.
            let mut wipe_estimate = crate::prusa_wipe::Wipe::default();
            let mut cooling_started = !cooling.orca;
            if !cooling_started {
                travel_estimate = 0.;
            }
            let mut previous = if layer_change.is_some() {
                layer.paths.first().map(|p| mm(p.points[0]))
            } else {
                w.position()
            };
            let mut previous_role = orca_wipe.role;
            let mut orca_last_e = 0.;
            // Avoid-crossing routes planned for the estimate, reused by the writer when it starts
            // from the same position.
            let mut acp_routes: Vec<Option<AcpRoute>> = vec![None; layer.paths.len()];
            let mut acp_sim = acp;
            for (i, path) in layer.paths.iter().enumerate() {
                let (path_config, path_settings, planner) =
                    multi.as_ref().map_or((config, s, &travels), |m| {
                        let tool = scheduled.assignments[i].tool;
                        (
                            &m.tools[tool].config,
                            &m.tools[tool].settings,
                            &tool_travels.as_ref().unwrap()[tool],
                        )
                    });
                let path_wipe = crate::prusa_wipe::Params::new(path_config, path_settings);
                let to = mm(path.points[0]);
                let far = previous.is_none_or(|p| {
                    planner.needs_retraction(
                        p,
                        to,
                        path.role,
                        previous_role,
                        path_settings.retract_before_travel,
                    )
                });
                // The first path deretracts after the layer-change retraction.
                let retracted = (i == 0 && layer_retract) || far;
                if cooling_started && retracted && path_settings.retract_length > 0.0 {
                    travel_estimate += path_settings.retract_length / path_settings.retract_speed
                        + path_settings.retract_length / path_settings.deretract_speed;
                }
                let deretract = retracted && path_settings.retract_length > 0.0;
                if let Some(last) = plan.orca_phantom(
                    i,
                    if deretract {
                        path_settings.retract_length
                            + path_config
                                .float("filament_retract_restart_extra")
                                .or_else(|| path_config.float("retract_restart_extra"))
                                .unwrap_or(0.)
                    } else {
                        orca_last_e
                    },
                ) {
                    orca_last_e = last;
                }
                let use_external = acp_sim.before_path(path.role, path_object(i));
                let acp_route = |from: (f64, f64)| {
                    (far && i > 0 && !acp_sim.disabled_once)
                        .then(|| planner.prusa_route(from, to, use_external, false))
                        .flatten()
                };
                let mut route = previous.and_then(|from| acp_route(from).map(|r| (from, r)));
                if route.as_ref().is_some_and(|(_, r)| r.1) {
                    wipe_estimate.reset();
                }
                if i > 0
                    && far
                    && path_settings.retract_length > 0.0
                    && let (Some(p), Some(from)) = (&path_wipe, previous)
                    && wipe_estimate.has_path()
                {
                    let from = ((from.0 * 1e3).round() / 1e3, (from.1 * 1e3).round() / 1e3);
                    let plan = wipe_estimate.plan(from, 0.0, path_settings.retract_length, p);
                    if cooling_started {
                        travel_estimate += plan.duration(path_settings.retract_speed)
                            - path_settings.retract_length / path_settings.retract_speed;
                    }
                    if let Some(end) = plan.end() {
                        previous = Some(end);
                        let end = gcode_to_point(end, obj.object_origin(path_object(i)));
                        route = acp_route(end).map(|r| (end, r));
                    }
                }
                if !far {
                    wipe_estimate.reset();
                }
                if previous.is_some_and(|p| (p.0 - to.0).hypot(p.1 - to.1) > 0.002) {
                    acp_sim.after_travel();
                }
                acp_routes[i] = route.clone();
                if cooling_started && let Some(from) = previous {
                    let retract = far && path_settings.retract_length > 0.0;
                    let points = match &route {
                        Some((_, (r, _))) => planner.plan_route(r, i, retract),
                        None => planner.plan(from, to, i, retract),
                    };
                    travel_estimate += crate::travels::duration(
                        [from.0, from.1, z],
                        &points,
                        travel_speed,
                        travel_speed_z,
                    );
                }
                if cooling_started
                    && let (Some(end), Some(wipe)) = (
                        path.points.last(),
                        path.seam.as_ref().and_then(|seam| seam.wipe),
                    )
                {
                    travel_estimate +=
                        end.distance_to(wipe) / mica_core::geometry::SCALE / travel_speed;
                }
                if path_wipe.is_some() {
                    wipe_estimate.extruded(path, plan.curves(i));
                }
                acp_sim.after_path(path.role, layer.paths.get(i + 1).map(|p| p.role), first);
                previous = path.end_position().map(mm);
                previous_role = Some(path.role);
                // Orca marks every path, bridges included, with `;_EXTRUDE_SET_SPEED`.
                cooling_started |= cooling.orca || !path.role.is_bridge();
            }
            if let Some(m) = &mut multi {
                let paths: Vec<_> = layer
                    .paths
                    .iter()
                    .enumerate()
                    .filter_map(|(i, p)| {
                        let start = mm(*p.points.first()?);
                        let end = p.end_position().map_or(start, mm);
                        Some((
                            scheduled.assignments[i].tool,
                            [start.0, start.1],
                            [end.0, end.1],
                        ))
                    })
                    .collect();
                travel_estimate += m.tower_fixed_time(li, config, &paths);
                travel_estimate += m.cooling_toolchange_time(li, config);
            }
            // PrusaSlicer's cooling buffer also times the layer's custom G-code moves.
            if !orca && let (Some(item), picked) = custom_plan.at(li) {
                let first = multi.as_ref().map_or(s, |m| {
                    &m.tools[scheduled.assignments.first().map_or(m.active, |a| a.tool)].settings
                });
                let deretract = if first.retract_length > 0. && !layer.paths.is_empty() {
                    first.retract_length / first.deretract_speed
                } else {
                    0.
                };
                travel_estimate +=
                    crate::custom_gcode::cooling_time(item, picked.is_some(), config, deretract);
            }
            let cooling_tools = multi.as_ref().map_or_else(
                || vec![cooling],
                |m| {
                    m.tools
                        .iter()
                        .map(|t| cooling_config(&t.config, &t.settings))
                        .collect()
                },
            );
            let layer_time = slow_down_multi(&mut plan.moves, travel_estimate, &cooling_tools);
            if let Some(m) = &mut multi {
                m.layer_fans = cooling_tools
                    .iter()
                    .map(|cfg| fan_speed(layer.index, layer_time, cfg))
                    .collect();
            }
            obj.emitted(
                li,
                &plan.emissions(|i, requested| {
                    let (cfg, bridge) =
                        multi.as_ref().map_or((&cooling, s.bridge_fan_speed), |m| {
                            let tool = scheduled.assignments[i].tool;
                            (
                                &cooling_tools[tool],
                                m.tools[tool].settings.bridge_fan_speed,
                            )
                        });
                    crate::cooling::requested_fan_speed(
                        layer.index,
                        layer_time,
                        cfg,
                        requested,
                        layer.paths[i].role.is_bridge().then_some(bridge),
                    )
                }),
                layer_time,
            );

            if !orca {
                w.set_fan(fan_speed(layer.index, layer_time, &cooling));
            } else {
                // CoolingBuffer::apply_layer_cooldown starts the layer's G-code (before
                // ;LAYER_CHANGE) with change_extruder_set_fan for the current filament and no
                // fan requests.
                let tool = multi.as_ref().map_or(0, |m| m.active);
                let c = multi.as_ref().map_or(config, |m| &m.tools[tool].config);
                let fan = fan_speed(layer.index, layer_time, &cooling_tools[tool]);
                w.with_orca_fan(|f, w| f.change_extruder(w, c, tool, layer.index, fan, true));
            }

            w.begin_cooling_layer();
            // PrusaSlicer's first layer height marker is its print_z (with z_offset).
            let marker_height = if first {
                layer.height + print_z_shift
            } else {
                layer.height
            };
            w.raw(&format!(
                ";LAYER_CHANGE\n;Z:{}\n;HEIGHT:{}",
                // Six significant digits, as PrusaSlicer's float_to_string_decimal_point and
                // Orca's %g write them: two layers 0.0003 mm apart (variable layer heights on
                // two objects) keep distinct markers instead of both reading 0.8.
                if klipper {
                    decimal_g6(z + print_z_shift)
                } else {
                    num_g6(z + print_z_shift)
                },
                if klipper {
                    decimal_g6(marker_height)
                } else {
                    num_g6(marker_height)
                }
            ));
            if !first || orca {
                set_layer_vars(
                    &mut ctx,
                    layer_num,
                    z + print_z_shift,
                    if orca {
                        max_layer_z.max(z)
                    } else {
                        max_layer_z + print_z_shift
                    },
                );
                let before = process(
                    "before_layer_gcode",
                    &template("before_layer_gcode"),
                    &mut ctx,
                    &mut w,
                )
                .map_err(tpl_err)?;
                w.raw(&before);
            }
            // change_layer: with multiple extruders the layer's first instance starts before
            // the layer-change retraction (maybe_change_instance).
            if let Some(m) = &mut multi
                && !orca
                && m.multiple_extruders()
                && let Some(p) = layer.paths.first()
                && !matches!(p.role, Role::Skirt | Role::Brim)
            {
                m.begin_label(scheduled.assignments[0].object, &mut w, config);
            }
            if let Some(plan) = &layer_wipe {
                crate::prusa_wipe::emit(&mut w, plan, wipe_retract_speed);
            } else if !layer_retract {
                // Retract on layer change is off and the move does not need a retraction.
            } else if let Some(m) = &multi {
                let t = &m.tools[m.active];
                if crate::orca_motion::enabled(&t.config) {
                    orca_wipe.retract(&mut w, &t.config, &t.settings);
                } else {
                    m.retract_current(&mut w);
                }
            } else {
                orca_wipe.retract(&mut w, config, s);
            }
            // Wiping after raising Z is pointless.
            prusa_wipe.reset();
            if let Some(points) = &layer_change {
                // Unlift at the start of extrusion, after the layer template/temperature commands.
                for &point in &points[..points.len() - 1] {
                    w.travel_to_xyz_force(point, travel_speed, travel_speed_z);
                }
            } else if object_start.is_none()
                && (travels.is_orca() == first || (travels.is_orca() && spiral.is_some()))
            {
                // GCode::change_layer (Orca): a spiral vase print moves to the layer's Z at once.
                // GCodeGenerator::change_layer moves no Z on the first layer (it only adds
                // z_offset to the writer's position); the move to the first point lowers it.
                w.move_z(z, travel_speed_z);
            }
            if klipper && !first && exclude {
                w.raw(&format!("EXCLUDE_OBJECT_END NAME={label}"));
            }
            max_layer_z = max_layer_z.max(z);
            let mut last_height = Some(marker_height);
            if !first || orca {
                set_layer_vars(
                    &mut ctx,
                    layer_num,
                    z + print_z_shift,
                    if orca {
                        max_layer_z.max(z)
                    } else {
                        max_layer_z + print_z_shift
                    },
                );
                let after = process("layer_gcode", &template("layer_gcode"), &mut ctx, &mut w)
                    .map_err(tpl_err)?;
                w.raw(&after);
            }
            if orca {
                w.raw(";_SET_FAN_SPEED_CHANGING_LAYER");
                if first {
                    w.set_acceleration(s.first_layer_acceleration);
                } else if layer.index == 1
                    && s.default_acceleration > 0.
                    && s.first_layer_acceleration > 0.
                {
                    w.set_acceleration(s.default_acceleration);
                }
            }
            if layer.index == 1 {
                if let Some(m) = &multi {
                    for &tool in &m.job.ordering.used {
                        // Match both process_layer implementations: standby tools are left
                        // untouched until the normal toolchange/preheat path needs them.
                        if tool != m.active
                            && (config.bool("ooze_prevention").unwrap_or(false)
                                || config
                                    .bool("single_extruder_multi_material")
                                    .unwrap_or(false))
                        {
                            continue;
                        }
                        let t = &m.tools[tool].settings;
                        if t.temperature != t.first_layer_temperature && t.temperature > 0. {
                            w.raw(&format!(
                                "M104 S{}{} ; set temperature",
                                num(t.temperature, 0),
                                tool_word(m, tool)
                            ));
                        }
                    }
                }
                if !is_multi && s.temperature != s.first_layer_temperature && s.temperature > 0.0 {
                    w.raw(&format!(
                        "M104 S{} ; set temperature",
                        num(s.temperature, 0)
                    ));
                }
                // A second layer with nothing to print (GUI monkey test) keeps the active tool.
                let first_tool = multi.as_ref().map_or(0, |m| {
                    m.job.ordering.layers[li]
                        .extruders
                        .first()
                        .copied()
                        .unwrap_or(m.active)
                });
                let bed_temperature = layer_bed_temperature(config, first_tool, used_tools, false);
                if bed_temperature != last_bed_temperature {
                    w.raw(&format!(
                        "M140 S{} ; set bed temperature",
                        num(bed_temperature, 0)
                    ));
                }
                if s.bed_temperature > 0.0 {
                    bed_target = s.bed_temperature;
                }
            }
            // PrusaSlicer runs a layer's custom G-code here, after the layer change and the
            // second-layer temperatures, except a colour change, which waits for the layer's first
            // extrusion (or, on a tool changer, for the tool it is for). Orca runs it after its
            // first-layer acceleration, below.
            let (custom_item, custom_tool) = custom_plan.at(li);
            let first_tool = multi
                .as_ref()
                .and_then(|m| m.job.ordering.layers[li].extruders.first().copied())
                .unwrap_or(0);
            let mut pending_custom = None;
            let mut custom_after_tool = None;
            if !orca && let Some(item) = custom_item {
                if custom_tool.is_some() {
                    custom_after_tool = custom_tool.map(|t| (t, item));
                } else {
                    let current = multi.as_ref().map_or(0, |m| m.active);
                    let text = crate::custom_gcode::prusa(
                        item, current, first_tool, config, &mut ctx, &mut w, &process,
                    )
                    .map_err(tpl_err)?;
                    if item.kind == mica_core::custom_gcode::Kind::ColorChange {
                        pending_custom = Some(text);
                    } else {
                        w.raw(&text);
                    }
                }
            }
            if orca && let Some(item) = custom_item {
                let current = multi.as_ref().map_or(0, |m| m.active);
                let text =
                    crate::custom_gcode::orca(item, current, config, &mut ctx, &mut w, &process)
                        .map_err(tpl_err)?;
                w.raw(&text);
            }
            if klipper {
                if exclude {
                    w.raw(&format!("EXCLUDE_OBJECT_START NAME={label}"));
                }
            } else if travel_labels {
                // process_layer: maybe_stop_instance before the layer's skirt and brim.
                if layer
                    .paths
                    .iter()
                    .any(|p| matches!(p.role, Role::Skirt | Role::Brim))
                {
                    let text = label_change(&mut multi, &mut single_label, None, config);
                    w.raw_nonempty(&text);
                }
            } else if !is_multi && config.string("gcode_label_objects") == Some("firmware") {
                w.raw("M486 S0");
            }
            // The object a path belongs to for its label: none for skirt and brim.
            let path_label = |index: usize| -> Option<usize> {
                let path = &layer.paths[index];
                (!matches!(path.role, Role::Skirt | Role::Brim))
                    .then(|| scheduled.assignments.get(index).map_or(0, |a| a.object))
            };

            // travel_to_first_position after a ramping layer change: XY to the current position
            // (CoolingBuffer drops it at the current F), then Z to the first point's
            // scaled(print_z + (height_fraction - 1) * height). With several tools only when the
            // ramp already ended at the layer (the tool change comes first otherwise).
            let arrive = layer_change.as_ref().is_some_and(|p| {
                p.len() > 1
                    && (multi.as_ref().is_none_or(|m| m.job.ordering.used.len() < 2)
                        || (p[p.len() - 2][2] - z).abs() < 0.0005)
            });
            // travel_to_first_position on a spiral vase layer of a ramping printer: change_layer
            // did not ramp, so the nozzle moves straight to the first point (a move the spiral
            // drops) at the layer's Z (already there: CoolingBuffer drops the repeated Z).
            let spiral_arrival = spiral_on && !orca && travels.ramps() && w.position().is_some();
            for (index, path) in layer.paths.iter().enumerate() {
                // Raised walls of an earlier layer stand below this layer: travels may drop again.
                w.settle_raised(layer.print_z);
                if index == 0 && spiral_arrival {
                    if travel_labels {
                        let text =
                            label_change(&mut multi, &mut single_label, path_label(0), config);
                        w.raw_nonempty(&text);
                    }
                    let (sx, sy) = mm(path.points[0]);
                    w.travel_to_xy_force_line(sx, sy, travel_speed);
                    let first_z = crate::travels::scaled_z(crate::seam::start_z(path, layer));
                    if (w.z() - first_z).abs() > 1e-9 {
                        w.travel_to_z_force(first_z, travel_speed_z);
                    }
                }
                if index == 0 && arrive {
                    // travel_to_first_position: insert_gcode before the forced XY and Z moves.
                    if travel_labels {
                        let text =
                            label_change(&mut multi, &mut single_label, path_label(0), config);
                        w.raw_nonempty(&text);
                    }
                    let first_z = crate::travels::scaled_z(crate::seam::start_z(path, layer));
                    w.confirm_layer_arrival(
                        mm(path.points[0]),
                        travel_speed,
                        travel_speed_z,
                        first_z,
                    );
                }
                let mut deferred_label = None;
                if let Some(m) = &mut multi {
                    let a = scheduled.assignments[index];
                    let route = crate::wipe_tower::Route {
                        planners: tool_travels.as_deref(),
                        printed: index,
                    };
                    let switch = m.tower_before_tool(
                        a.tool,
                        li,
                        z,
                        route,
                        &mut prusa_wipe,
                        &mut ctx,
                        &mut w,
                        config,
                        &mut orca_wipe,
                    )?;
                    if switch.changed && !switch.tower && !orca {
                        // set_extruder forgets last_position: _extrude moves to the first point
                        // "after unknown position" (retract_and_wipe, XY travel, forced layer Z)
                        // without lift, ramp or avoid crossing.
                        let t = &m.tools[a.tool];
                        w.retract_with_extra(
                            t.settings.retract_length,
                            t.settings.retract_speed,
                            t.config.float("retract_restart_extra").unwrap_or(0.),
                        );
                        let (sx, sy) = mm(path.points[0]);
                        w.travel_to(sx, sy, travel_speed);
                        w.travel_to_z_force(z, travel_speed_z);
                        acp.after_travel();
                        acp_from = None;
                    }
                    if switch.tower {
                        // append_tcr: the next travel plans between objects and wipes along the
                        // tower's wipe path.
                        acp.external_once = true;
                        acp_from = None;
                        last_type = None;
                        last_width = None;
                        w.raw(&format!(";HEIGHT:{}", num(layer.height, 6)));
                        last_height = Some(layer.height);
                    }
                    if let Some((tool, item)) = custom_after_tool
                        && tool == a.tool
                    {
                        // The tool the colour change is for is picked: run it now.
                        custom_after_tool = None;
                        let text = crate::custom_gcode::prusa(
                            item, m.active, first_tool, config, &mut ctx, &mut w, &process,
                        )
                        .map_err(tpl_err)?;
                        w.raw(&text);
                        // After the tool change PrusaSlicer moves to the first point from an
                        // unknown position ("move to print after unknown position"): retract
                        // (M600 left the nozzle primed) and lift, travel, then force the layer Z.
                        let t = &m.tools[a.tool];
                        if t.settings.retract_length > 0. {
                            w.retract_with_extra(
                                t.settings.retract_length,
                                t.settings.retract_speed,
                                t.config.float("retract_restart_extra").unwrap_or(0.),
                            );
                            w.lift(t.config.float("retract_lift").unwrap_or(0.), travel_speed_z);
                        }
                        let (sx, sy) = mm(path.points[0]);
                        let lifted_z = w.z();
                        w.forget_position(lifted_z);
                        w.travel_to(sx, sy, travel_speed);
                        w.travel_to_z_force(crate::seam::start_z(path, layer), travel_speed_z);
                    }
                    ctx.set(
                        "current_object_idx",
                        Var::int(
                            m.job
                                .sequential_object(li)
                                .map_or(m.job.objects[a.object].object_id, |o| o.ordinal)
                                as i64,
                        ),
                    );
                    if travel_labels {
                        // The label changes in the travel below.
                    } else if matches!(path.role, Role::Skirt | Role::Brim) {
                        m.end_label(&mut w, config);
                    } else if orca || m.has_label() || !m.multiple_extruders() {
                        m.begin_label(a.object, &mut w, config);
                    } else {
                        // _extrude: with multiple extruders (GCodeWriter::multiple_extruders) and
                        // no active instance, e.g. after a toolchange or the tower, the instance
                        // starts after the unretract.
                        deferred_label = Some(a.object);
                    }
                }
                let (config, s) = multi.as_ref().map_or((config, s), |m| {
                    let a = scheduled.assignments[index];
                    m.job.objects[a.object].path_config(a.region, a.tool)
                });
                let control = obj.calibration_control(li, index);
                let calibrated_path_settings = crate::calib::settings(s, control);
                let s = calibrated_path_settings.as_ref();
                crate::calib::emit(
                    &mut w,
                    flavor,
                    control,
                    is_multi.then(|| scheduled.assignments[index].tool),
                );
                let travels = tool_travels
                    .as_ref()
                    .map_or(&travels, |v| &v[scheduled.assignments[index].tool]);
                if is_multi
                    && (index == 0
                        || scheduled.assignments[index - 1].tool
                            != scheduled.assignments[index].tool)
                {
                    // Orca's CoolingBuffer sets the new filament's fans at its T line.
                    if !cooling.orca {
                        let fan = fan_speed(layer.index, layer_time, &cooling_config(config, s));
                        w.set_fan(fan);
                    }
                }
                let filament_area = if is_multi {
                    std::f64::consts::PI * (s.filament_diameter / 2.).powi(2)
                } else {
                    filament_area
                };
                let (sx, sy) = mm(path.points[0]);
                // GCode::travel_to plans (and Orca's Auto Lift probes) the travel from here,
                // before the retraction's wipe moves the nozzle.
                // Orca's "move inwards before travel" does not update GCode::last_pos.
                let travel_from = match inward {
                    Some((end, after)) if travels.is_orca() && w.position() == Some(after) => {
                        Some(end)
                    }
                    _ => w.position(),
                };
                let far = travel_from.is_none_or(|p| {
                    travels.needs_retraction(
                        p,
                        (sx, sy),
                        path.role,
                        orca_wipe.role,
                        s.retract_before_travel,
                    )
                });
                let mut short = travel_from
                    .is_some_and(|p| travels.distance(p, (sx, sy)) < s.retract_before_travel);
                let path_wipe = crate::prusa_wipe::Params::new(config, s);
                // GCodeGenerator::generate_travel_xy_path: avoid crossing perimeters only for a
                // travel that needs retraction, from the unquantized last position.
                let use_external = acp.before_path(path.role, path_object(index));
                let acp_enabled =
                    far && index > 0 && !acp.disabled_once && travels.has_prusa_avoid_crossing();
                let plan_acp = |from: (f64, f64)| -> Option<AcpRoute> {
                    if !acp_enabled {
                        return None;
                    }
                    if let Some(cached) = &acp_routes[index]
                        && cached.0 == from
                    {
                        return Some(cached.clone());
                    }
                    travels
                        .prusa_route(from, (sx, sy), use_external, false)
                        .map(|r| (from, r))
                };
                let mut acp_route = acp_from.or(w.position()).and_then(plan_acp);
                if acp_route.as_ref().is_some_and(|(_, r)| r.1) {
                    // need_wipe(): the travel stays inside, wiping is pointless.
                    prusa_wipe.reset();
                }
                if far
                    && let Some(p) = &path_wipe
                    && prusa_wipe.has_path()
                    && let Some(from) = w.position()
                {
                    // GCodeGenerator::travel_to: retract_and_wipe, then route from the wipe end.
                    if is_multi {
                        w.set_restart_extra(
                            config
                                .float("filament_retract_restart_extra")
                                .or_else(|| config.float("retract_restart_extra"))
                                .unwrap_or(0.),
                        );
                    }
                    let plan = prusa_wipe.plan(from, w.retracted(), s.retract_length, p);
                    crate::prusa_wipe::emit(&mut w, &plan, s.retract_speed);
                    if let Some(end) = plan.end() {
                        // Wipe::wipe: last_position = gcode_to_point(quantized end).
                        let end = gcode_to_point(end, obj.object_origin(path_object(index)));
                        acp_route = plan_acp(end);
                    }
                    short = w.position().is_some_and(|p| {
                        acp_route.as_ref().map_or_else(
                            || travels.distance(p, (sx, sy)),
                            |(_, (r, _))| crate::travels::route_length(r),
                        ) < s.retract_before_travel
                    });
                } else if far {
                    if is_multi {
                        let extra = config
                            .float("filament_retract_restart_extra")
                            .or_else(|| config.float("retract_restart_extra"))
                            .unwrap_or(0.);
                        if crate::orca_motion::enabled(config) {
                            w.set_restart_extra(extra);
                            orca_wipe.retract(&mut w, config, s);
                        } else {
                            w.retract_with_extra(s.retract_length, s.retract_speed, extra);
                        }
                    } else {
                        orca_wipe.retract(&mut w, config, s);
                    }
                }
                // A real travel, not a joint between pieces of one variable-width wall (the writer's
                // position is rounded to 1 µm).
                let moves = w
                    .position()
                    .is_some_and(|p| travels.distance(p, (sx, sy)) > 0.002);
                if far || moves {
                    prusa_wipe.reset();
                }
                if orca {
                    let key = if first {
                        "initial_layer_travel_acceleration"
                    } else if short && path.role == Role::ExternalPerimeter {
                        "outer_wall_acceleration"
                    } else {
                        "travel_acceleration"
                    };
                    let accel = if s.default_acceleration > 0.0 {
                        config.float(key).unwrap_or(s.default_acceleration)
                    } else {
                        0.0
                    };
                    w.set_accel_and_jerk(accel, orca::jerk(config, path.role, first, true, short));
                } else if moves {
                    w.set_acceleration(s.default_acceleration);
                }
                // PrusaSlicer GCodeGenerator::generate_travel_gcode: every travel_to sets the travel
                // acceleration (the writer drops repeats; the first one is always written), with a
                // gentle one for the short hop onto an external perimeter, restoring the travel
                // acceleration afterwards. travel_to_first_position and a travel from an unknown
                // position write their moves directly.
                let travel_accel = config.float("travel_acceleration").unwrap_or(0.0);
                let short_accel = (!orca && short && moves && path.role == Role::ExternalPerimeter)
                    .then(|| config.float("travel_short_distance_acceleration"))
                    .flatten()
                    .filter(|a| *a > 0.0);
                let travel_to = !orca
                    && moves
                    && !(index == 0 && object_start.is_some())
                    && w.position().is_some();
                if travel_to {
                    w.set_travel_acceleration(short_accel.unwrap_or(travel_accel));
                    if travel_labels {
                        let text =
                            label_change(&mut multi, &mut single_label, path_label(index), config);
                        w.set_travel_insert(text);
                    }
                }
                if moves {
                    acp.after_travel();
                }
                if index == 0 && object_start.is_some() {
                    // travel_to_first_position resets the one-shot modifiers.
                    acp.after_travel();
                    // travel_to_first_position: across at the clearance height, then down
                    // (Orca descends in the same move).
                    if orca {
                        let z = crate::seam::start_z(path, layer);
                        w.travel_to_xyz_force([sx, sy, z], travel_speed, 0.);
                    } else {
                        if travel_labels {
                            let text = label_change(
                                &mut multi,
                                &mut single_label,
                                path_label(index),
                                config,
                            );
                            w.raw_nonempty(&text);
                        }
                        w.travel_to(sx, sy, travel_speed);
                    }
                } else if let (Some((_, (route, _))), Some(position)) = (&acp_route, w.position()) {
                    let mut route = route.clone();
                    route[0] = position;
                    travels.emit_route(
                        &mut w,
                        &route,
                        index,
                        far && s.retract_length > 0.0,
                        travel_speed,
                        travel_speed_z,
                    );
                } else if w.position().is_some() {
                    travels.emit_probed(
                        &mut w,
                        (sx, sy),
                        travel_from,
                        index,
                        (far || (travels.is_orca() && index == 0 && layer_retract))
                            && s.retract_length > 0.0,
                        travel_speed,
                        travel_speed_z,
                    );
                } else {
                    // travel_to_first_position resets the one-shot modifiers.
                    acp.after_travel();
                    if far && s.retract_length > 0.0 {
                        w.lift(travels.lift_height(), travel_speed_z);
                    }
                    // travel_to_first_position (and _extrude after an unknown position): the
                    // label changes before the XY move.
                    if travel_labels {
                        let text =
                            label_change(&mut multi, &mut single_label, path_label(index), config);
                        w.raw_nonempty(&text);
                    }
                    w.travel_to(sx, sy, travel_speed);
                    if orca {
                        w.unlift(travel_speed_z);
                    } else {
                        // travel_to_first_position: travel_to_z_force(first point's Z) straight
                        // from the lift (which started at 2 × z_offset with a Z offset).
                        w.unlift_to(crate::seam::start_z(path, layer), travel_speed_z);
                    }
                }
                // A travel that wrote no waypoint still ran its insert_gcode.
                w.flush_travel_insert();
                if travel_to && short_accel.is_some() {
                    w.set_travel_acceleration(travel_accel);
                }
                let start_z = crate::seam::start_z(path, layer);
                if first && index == 0 && control.is_some() {
                    // Startup macros may leave the nozzle lifted. A zero-retraction calibration
                    // has no travel hop to establish Z, so explicitly arrive even if cached Z agrees.
                    w.travel_to_xyz_force([sx, sy, start_z], travel_speed_z, travel_speed_z);
                } else {
                    w.move_z(start_z, travel_speed_z);
                }
                w.unretract(s.deretract_speed);
                if let (Some(object), Some(m)) = (deferred_label.take(), &mut multi) {
                    m.begin_label(object, &mut w, config);
                }
                if let Some(text) = pending_custom.take() {
                    // Due before the layer's first extrusion (PrusaSlicer GH #2672).
                    w.raw(&text);
                }

                if orca {
                    w.set_accel_and_jerk(
                        if s.default_acceleration > 0.0 {
                            role_acceleration(s, path.role, first)
                        } else {
                            0.0
                        },
                        orca::jerk(config, path.role, first, false, false),
                    );
                } else {
                    w.set_acceleration(role_acceleration(s, path.role, first));
                }
                if last_type != Some(path.role) {
                    let name = if klipper {
                        match path.role {
                            Role::Skirt => "Skirt",
                            Role::Brim => "Brim",
                            Role::ExternalPerimeter => "Outer wall",
                            Role::Perimeter => "Inner wall",
                            Role::OverhangPerimeter => "Overhang wall",
                            Role::BridgeInfill => "Bridge",
                            Role::InternalBridgeInfill => "Internal Bridge",
                            Role::InternalInfill => "Sparse infill",
                            Role::SolidInfill => "Internal solid infill",
                            Role::TopSolidInfill => "Top surface",
                            Role::Ironing => "Ironing",
                            Role::GapFill => "Gap infill",
                            Role::SupportMaterial => "Support",
                            Role::SupportMaterialInterface => "Support interface",
                        }
                    } else {
                        path.role.name()
                    };
                    w.raw(&format!(";TYPE:{name}"));
                    last_type = Some(path.role);
                }
                if last_width != Some(path.flow.width) {
                    w.raw(&format!(";WIDTH:{}", num(path.flow.width, 6)));
                    last_width = Some(path.flow.width);
                }
                // GCodeGenerator::_extrude: a new height only past EPSILON (both slicers).
                if last_height.is_none_or(|h| (h - path.flow.height).abs() > 1e-4) {
                    w.raw(&format!(";HEIGHT:{}", num(path.flow.height, 6)));
                    last_height = Some(path.flow.height);
                }
                let e_per_mm = path.mm3_per_mm() * s.extrusion_multiplier / filament_area;
                let before = w.stats.filament_mm;
                let tool_cooling = is_multi.then(|| cooling_config(config, s));
                plan.emit(
                    &mut w,
                    index,
                    path,
                    layer,
                    tool_cooling.as_ref().unwrap_or(&cooling),
                    layer_time,
                    config.float("bridge_fan_speed").unwrap_or(0.0),
                    e_per_mm,
                    travel_speed_z,
                    arc_mode,
                    cooling.orca,
                );
                // GCodeGenerator::extrude_smooth_path resets the acceleration once per extrusion
                // entity: the pieces of one loop (an overhang stretch of a perimeter, the
                // variable-width pieces of an Arachne wall) join end to start.
                if !orca
                    && layer.paths.get(index + 1).is_none_or(|next| {
                        path.points.last().unwrap().distance_to(next.points[0]) > 2_000.
                    })
                {
                    w.set_acceleration(s.default_acceleration);
                }
                orca_wipe.remember(
                    path,
                    config,
                    w.active_tool(),
                    plan.final_original_speed(index),
                );
                if path_wipe.is_some() {
                    prusa_wipe.extruded(path, plan.curves(index));
                }
                let end = w.position();
                crate::seam::wipe(&mut w, path, travel_speed);
                if path.z_raise() > 0.0 {
                    // Brick layers: travels after a raised wall keep above it.
                    let lift = if s.brick.travel_lift > 0.0 {
                        s.brick.travel_lift
                    } else {
                        s.retract_lift.max(0.2)
                    };
                    w.printed_raised(start_z, lift, travel_speed_z);
                }
                inward = end.zip(w.position()).filter(|(a, b)| a != b);
                last_path_position = path.end_position().map(mm);
                acp_from = last_path_position;
                acp.after_path(path.role, layer.paths.get(index + 1).map(|p| p.role), first);
                if let Some(m) = &mut multi {
                    m.record(
                        scheduled.assignments[index].tool,
                        path.role,
                        w.stats.filament_mm - before,
                    );
                }
            }
            if !orca {
                w.set_acceleration(s.default_acceleration);
            }
            // _do_export's label stop after the last layer comes after the spiral vase
            // post-processor (outside the layer's G-code).
            let last_of_spiral = spiral.is_some() && li + 1 == layer_count;
            if travel_labels {
                // _do_export: maybe_stop_instance after the last layer.
                if li + 1 == layer_count && !is_multi && !last_of_spiral {
                    let text = label_change(&mut multi, &mut single_label, None, config);
                    w.raw_nonempty(&text);
                }
            } else if !klipper
                && !is_multi
                && config.string("gcode_label_objects") == Some("firmware")
            {
                w.raw("M486 S-1");
            }
            if let Some(m) = &mut multi {
                let route = crate::wipe_tower::Route {
                    planners: tool_travels.as_deref(),
                    printed: layer.paths.len(),
                };
                if m.tower_finish_layer(
                    li,
                    z,
                    route,
                    &mut prusa_wipe,
                    &mut ctx,
                    &mut w,
                    config,
                    &mut orca_wipe,
                )? {
                    acp.external_once = true;
                    acp_from = None;
                    last_type = None;
                    last_width = None;
                    last_path_position = None;
                }
                // PrusaSlicer keeps the open label across layers (maybe_change_instance only
                // switches it), closing it after the last layer; Orca closes it every layer.
                if orca || (li + 1 == layer_count && !last_of_spiral) {
                    m.end_label(&mut w, config);
                }
            }
            if let Some(spiral) = &mut spiral {
                // The layer's G-code from its layer change on (the cooling buffer's fan lines and
                // a move to the next object come before it, outside process_layer's result).
                spiral.enable(spiral_on);
                let text = w.take_output();
                let start = text
                    .find(";LAYER_CHANGE\n")
                    .filter(|&i| i == 0 || text.as_bytes()[i - 1] == b'\n')
                    .unwrap_or(text.len());
                let (head, body) = text.split_at(start);
                spiral.follow(head);
                let processed = spiral.process_layer(body, li + 1 == layer_count);
                let added =
                    crate::spiral_vase::extruded(&processed) - crate::spiral_vase::extruded(body);
                w.restore_output(format!("{head}{processed}"), added);
                if last_of_spiral {
                    if travel_labels && !is_multi {
                        let text = label_change(&mut multi, &mut single_label, None, config);
                        w.raw_nonempty(&text);
                    }
                    if let Some(m) = &mut multi
                        && !orca
                    {
                        m.end_label(&mut w, config);
                    }
                }
            }
        }
        Ok(())
    })?;

    crate::calib::finish(&mut w, flavor, obj.calibration_control(0, 0));
    {
        let (c, st) = multi.as_ref().map_or((config, s), |m| {
            (&m.tools[m.active].config, &m.tools[m.active].settings)
        });
        if let Some(plan) = crate::prusa_wipe::Params::new(c, st)
            .filter(|_| prusa_wipe.has_path())
            .zip(w.position())
            .map(|(p, from)| prusa_wipe.plan(from, w.retracted(), st.retract_length, &p))
        {
            crate::prusa_wipe::emit(&mut w, &plan, st.retract_speed);
        }
    }
    if let Some(m) = &mut multi {
        let (z, h) = obj.layer_z(layer_count - 1);
        m.tower_finalize(z, h, &mut ctx, &mut w, config)?;
        let t = &m.tools[m.active];
        if crate::orca_motion::enabled(&t.config) {
            orca_wipe.retract(&mut w, &t.config, &t.settings);
        } else {
            m.retract_current(&mut w);
        }
    } else {
        orca_wipe.retract(&mut w, config, s);
    }
    if klipper && exclude {
        w.raw(&format!("EXCLUDE_OBJECT_END NAME={label}"));
    }
    if orca {
        w.set_fan(0.0);
        // Orca's end-of-print `set_additional_fan(0)` is unconditional (a static writer helper
        // without state), so it is emitted even when the auxiliary fan is already off.
        if config.bool("auxiliary_fan").unwrap_or(false) {
            w.force_auxiliary_fan(0.0);
        }
        w.raw(";TYPE:Custom");
    } else {
        w.raw("M107\n;TYPE:Custom");
    }
    if !any_paths {
        return Err(ExportError::Empty);
    }
    let last = obj.layer_count().saturating_sub(1) as i64;
    // layer_z: the writer's Z without z_offset (both slicers); max_layer_z: PrusaSlicer's
    // includes it, so its CORE One end G-code `G1 Z{z_offset+min(max_layer_z+1, …)}` parks
    // z_offset higher than the last layer's emitted Z plus 1 mm, as upstream does.
    set_layer_vars(&mut ctx, last, max_layer_z, max_layer_z + print_z_shift);
    // Every extruder's filament end G-code, unless one nozzle is shared by several filaments
    // (single_extruder_multi_material), where only the active filament's runs.
    let filament_slots = if let Some(m) = &multi {
        m.tools.len()
    } else if config
        .bool("single_extruder_multi_material")
        .unwrap_or(false)
    {
        1
    } else {
        config.vector_len("end_filament_gcode").max(1)
    };
    for slot in 0..filament_slots {
        if let Some(m) = &multi
            && !m.job.ordering.used.contains(&slot)
        {
            continue;
        }
        ctx.set("filament_extruder_id", Var::int(slot as i64));
        if orca_templates || is_multi {
            ctx.set_current_extruder(slot);
        }
        let tpl = config
            .string_at("end_filament_gcode", slot)
            .unwrap_or("")
            .to_string();
        let end = process("end_filament_gcode", &tpl, &mut ctx, &mut w).map_err(tpl_err)?;
        w.raw(&end);
    }
    if orca_templates || is_multi {
        ctx.set_current_extruder(multi.as_ref().map_or(0, |m| m.active));
    }
    let end = process("end_gcode", &template("end_gcode"), &mut ctx, &mut w).map_err(tpl_err)?;
    w.raw(&end);
    if orca && let Some(fan) = exhaust_fan(config, used_tools, false) {
        w.raw(&fan);
    }
    if klipper {
        w.raw("; EXECUTABLE_BLOCK_END");
    }
    w.raw(crate::time_estimate::LAST_M73);
    let stats = w.stats;
    if !klipper && config.string("gcode_label_objects") == Some("firmware") {
        if let Some(job) = obj.print_job() {
            w.raw(&format!(
                "; objects_info = {}",
                crate::multi::objects_info(job)
            ));
        } else if let Some(mesh) = mesh {
            w.raw(&format!(
                "; objects_info = {}",
                crate::prusa_metadata::objects_info(object_name, mesh)
            ));
        }
    }
    if !orca {
        w.raw(&format!(
            "; filament used [mm] = {:.2}\n; filament used [cm3] = {:.2}",
            stats.filament_mm,
            stats.filament_mm * filament_area / 1000.0,
        ));
    }
    w.raw(crate::time_estimate::FILAMENT_MARKER);
    if let Some(m) = &multi {
        m.statistics(&mut w);
    }
    if orca {
        // Like Orca's run_post_process, fill every configured slot from explicit G-code,
        // including startup purge and tower moves. The weight/cost lines are conditional.
        w.raw("; filament used [mm] = 0\n; filament used [cm3] = 0\n; filament used [g] = 0\n; filament cost = 0\n; total filament used [g] = 0\n; total filament cost = 0");
        if let Some(m) = &multi
            && m.changes > 0
        {
            w.raw(&format!("; total filament change = {}", m.changes));
        }
        w.raw(&format!("; total layers count = {}", obj.layer_count()));
    }
    w.raw(crate::time_estimate::TIME_PLACEHOLDER);
    w.raw(&crate::full_config::text(full_config, used_tools));
    w.drain_to(sink).map_err(ExportError::Io)?;
    Ok(stats)
}

/// Second pipeline stage: analyze the text stream, rewind, then fill times, progress and headers.
/// The bounded planner and disk-backed timeline live in `time_estimate`.
pub fn postprocess<R: std::io::BufRead + std::io::Seek>(
    src: &mut R,
    dst: &mut dyn std::io::Write,
    stats: MoveStats,
    config: &Config,
) -> std::io::Result<MoveStats> {
    let _timer = mica_core::timing::Span::new(mica_core::timing::Stage::Postprocess);
    crate::time_estimate::Analysis::new(src, config)?.write(src, dst, stats)
}

/// Writes the G-code to `path` without holding it in memory: the first pass streams to a temporary
/// file next to it, the second pass post-processes that into `path`.
pub fn export_file(
    obj: &dyn LayerSource,
    config: &Config,
    s: &PrintSettings,
    object_name: &str,
    mesh: Option<&TriangleMesh>,
    path: &std::path::Path,
) -> Result<MoveStats, ExportError> {
    let _timing = mica_core::timing::Span::new(mica_core::timing::Stage::Export);
    use std::io::{BufReader, BufWriter, Write};
    let binary = path.extension().is_some_and(|e| e == "bgcode");
    let thumbnails = mesh
        .map(|m| crate::thumbnails::generate(config, m))
        .transpose()
        .map_err(ExportError::Io)?
        .unwrap_or_default();
    let tmp = path.with_extension("gcode.part");
    // The time analysis reads the text as it is written, on its own thread, instead of
    // reading the finished file again afterwards (same bytes in the same order).
    let (mut stats, analysis) = std::thread::scope(|scope| {
        let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(32);
        let analysis = scope.spawn(move || {
            let mut reader = ChunkReader {
                rx,
                chunk: Vec::new(),
                at: 0,
            };
            let analysis = crate::time_estimate::Analysis::from_stream(&mut reader, config, 0);
            // Drain what an error left unread, so the writer never blocks on a full channel.
            while reader.rx.recv().is_ok() {}
            analysis
        });
        let written = (|| -> Result<MoveStats, ExportError> {
            let mut first = Tee {
                file: BufWriter::new(std::fs::File::create(&tmp).map_err(ExportError::Io)?),
                tx: Some(tx),
                pending: Vec::with_capacity(TEE_CHUNK),
            };
            let stats = export(
                obj,
                config,
                s,
                object_name,
                mesh,
                if binary { &[] } else { &thumbnails },
                &mut first,
            )?;
            first.finish().map_err(ExportError::Io)?;
            Ok(stats)
        })();
        // An error drops the sender above, which ends the analysis early.
        let analysis = analysis.join().expect("time analysis thread panicked");
        written.map(|stats| (stats, analysis))
    })
    .inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })?;
    if binary && config.string("gcode_flavor") != Some("marlin2") {
        let _ = std::fs::remove_file(&tmp);
        return Err(ExportError::Unsupported(
            "binary G-code (.bgcode) is for Prusa printers only".into(),
        ));
    }
    let result = (|| -> std::io::Result<()> {
        let _timer = mica_core::timing::Span::new(mica_core::timing::Stage::Postprocess);
        let mut src = BufReader::new(std::fs::File::open(&tmp)?);
        let mut timing = analysis?;
        timing.update_stats(&mut stats);
        let dst = BufWriter::new(std::fs::File::create(path)?);
        if !binary {
            let mut dst = dst;
            timing.write(&mut src, &mut dst, stats)?;
            return dst.flush();
        }
        let max_layer_z = crate::prusa_metadata::max_layer_z(config, obj.max_print_z());
        let info = obj
            .print_job()
            .map(crate::multi::objects_info)
            .or_else(|| mesh.map(|m| crate::prusa_metadata::objects_info(object_name, m)));
        let now = crate::identity::epoch()?;
        let metadata = crate::prusa_metadata::bgcode_metadata(
            config,
            stats,
            max_layer_z,
            info.as_deref(),
            &crate::prusa_metadata::utc_timestamp(now),
            thumbnails,
        );
        let encoder =
            crate::bgcode::Encoder::new(dst, &metadata, &crate::bgcode::BinarizerConfig::default())
                .map_err(std::io::Error::other)?;
        let mut sink = EncoderSink(Some(encoder));
        timing.write(&mut src, &mut sink, stats)?;
        let mut dst = sink
            .0
            .take()
            .expect("encoder present")
            .finish()
            .map_err(std::io::Error::other)?;
        dst.flush()
    })();
    let _ = std::fs::remove_file(&tmp);
    result.map_err(ExportError::Io)?;
    Ok(stats)
}

/// Bytes handed to the time analysis at once.
const TEE_CHUNK: usize = 256 * 1024;

/// Writes the first-pass text to its file and hands the same bytes, in order, to the time
/// analysis thread.
struct Tee<W: std::io::Write> {
    file: W,
    tx: Option<std::sync::mpsc::SyncSender<Vec<u8>>>,
    pending: Vec<u8>,
}

impl<W: std::io::Write> Tee<W> {
    fn send(&mut self) {
        let chunk = std::mem::replace(&mut self.pending, Vec::with_capacity(TEE_CHUNK));
        // A failed analysis stops receiving; its error is reported after the export.
        if let Some(tx) = &self.tx
            && tx.send(chunk).is_err()
        {
            self.tx = None;
        }
    }

    /// Flushes the file and ends the analysis input.
    fn finish(mut self) -> std::io::Result<()> {
        if !self.pending.is_empty() {
            self.send();
        }
        self.tx = None;
        self.file.flush()
    }
}

impl<W: std::io::Write> std::io::Write for Tee<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.file.write_all(buf)?;
        self.pending.extend_from_slice(buf);
        if self.pending.len() >= TEE_CHUNK {
            self.send();
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

/// The analysis thread's view of the chunks a [`Tee`] sends: ends when the sender is gone.
struct ChunkReader {
    rx: std::sync::mpsc::Receiver<Vec<u8>>,
    chunk: Vec<u8>,
    at: usize,
}

impl std::io::Read for ChunkReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        use std::io::BufRead;
        let available = self.fill_buf()?;
        let n = available.len().min(buf.len());
        buf[..n].copy_from_slice(&available[..n]);
        self.consume(n);
        Ok(n)
    }
}

impl std::io::BufRead for ChunkReader {
    fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
        while self.at >= self.chunk.len() {
            match self.rx.recv() {
                Ok(chunk) => {
                    self.chunk = chunk;
                    self.at = 0;
                }
                Err(_) => return Ok(&[]),
            }
        }
        Ok(&self.chunk[self.at..])
    }

    fn consume(&mut self, amount: usize) {
        self.at += amount;
    }
}

/// Feeds post-processed G-code lines into the binary encoder.
struct EncoderSink<W: std::io::Write>(Option<crate::bgcode::Encoder<W>>);

impl<W: std::io::Write> std::io::Write for EncoderSink<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let text = std::str::from_utf8(buf).map_err(std::io::Error::other)?;
        self.0
            .as_mut()
            .expect("encoder present")
            .append_gcode(text)
            .map_err(std::io::Error::other)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// `process_layer`'s spiral vase condition: the settings allow a spiral on this layer
/// (`mica_core::spiral_vase::spiral_layer`), it prints one object (`layers.size() == 1`) without
/// supports, its region printed one loop of one island and no fills, and (Orca) it changes no
/// tool.
fn spiral_layer_enabled(
    s: &PrintSettings,
    scheduled: &mica_core::print_job::ScheduledLayer,
    outlines: Option<&[mica_core::geometry::ExPolygon]>,
    multi: Option<&crate::multi::Multi>,
    li: usize,
) -> bool {
    let tool_change = multi.is_some_and(|m| {
        m.job.ordering.layers[li]
            .extruders
            .iter()
            .any(|&t| t != m.active)
    });
    spiral_candidate(s, scheduled, outlines) && !tool_change
}

/// [`spiral_layer_enabled`] without the tool change (known only while writing).
fn spiral_candidate(
    s: &PrintSettings,
    scheduled: &mica_core::print_job::ScheduledLayer,
    outlines: Option<&[mica_core::geometry::ExPolygon]>,
) -> bool {
    let layer = &scheduled.layer;
    if !mica_core::spiral_vase::spiral_layer(s, layer.index) || layer.paths.is_empty() {
        return false;
    }
    let mut objects = scheduled.assignments.iter().map(|a| a.object);
    if let Some(first) = objects.next()
        && objects.any(|o| o != first)
    {
        return false;
    }
    let walls_only = layer.paths.iter().all(|p| {
        matches!(
            p.role,
            Role::ExternalPerimeter | Role::Perimeter | Role::OverhangPerimeter
        )
    });
    walls_only && outlines.is_none_or(|o| o.len() <= 1)
}

/// `GCodeGenerator::gcode_to_point`: a G-code position back to scaled object coordinates, which
/// truncates (`scaled<coord_t>`), returned in placed millimetres.
fn gcode_to_point(p: (f64, f64), origin: Option<Point>) -> (f64, f64) {
    let Some(o) = origin else {
        return p;
    };
    let (ox, oy) = (o.x as f64 * 1e-6, o.y as f64 * 1e-6);
    let x = o.x + ((p.0 - ox) / 1e-6) as i64;
    let y = o.y + ((p.1 - oy) / 1e-6) as i64;
    (
        x as f64 / mica_core::geometry::SCALE,
        y as f64 / mica_core::geometry::SCALE,
    )
}

/// The retract-and-wipe that precedes a travel to the wipe tower (`append_tcr`).
fn tower_wipe_plan(
    m: &crate::multi::Multi,
    wipe: &crate::prusa_wipe::Wipe,
    w: &crate::writer::Writer,
) -> Option<crate::prusa_wipe::WipePlan> {
    let t = &m.tools[m.active];
    crate::prusa_wipe::Params::new(&t.config, &t.settings)
        .filter(|_| wipe.has_path())
        .zip(w.position())
        .map(|(p, from)| wipe.plan(from, w.retracted(), t.settings.retract_length, &p))
}

/// One avoid-crossing travel: its start and (route, could the wipe be skipped).
type AcpRoute = ((f64, f64), (Vec<(f64, f64)>, bool));

/// GCodeGenerator's AvoidCrossingPerimeters modifiers: `disable_once`, `use_external_mp_once`,
/// `use_external_mp` around skirt/brim, and the current instance.
#[derive(Clone, Copy)]
struct AcpState {
    disabled_once: bool,
    external_once: bool,
    current_object: Option<usize>,
}
impl Default for AcpState {
    fn default() -> Self {
        // Enabled by default for the first travel move of the print.
        Self {
            disabled_once: true,
            external_once: false,
            current_object: None,
        }
    }
}
impl AcpState {
    /// Whether the travel to a path uses the external (between objects) planner.
    fn before_path(&mut self, role: Role, object: usize) -> bool {
        if self.current_object != Some(object) {
            self.external_once = true;
            self.current_object = Some(object);
        }
        matches!(role, Role::Skirt | Role::Brim) || self.external_once
    }
    /// `reset_once_modifiers`, at the end of every travel.
    fn after_travel(&mut self) {
        self.disabled_once = false;
        self.external_once = false;
    }
    /// The straight hop from the skirt (first layer only) or brim to the object.
    fn after_path(&mut self, role: Role, next: Option<Role>, first_layer: bool) {
        if next != Some(role)
            && ((role == Role::Skirt && first_layer) || role == Role::Brim)
            && next.is_some_and(|n| !matches!(n, Role::Skirt | Role::Brim))
        {
            self.disabled_once = true;
        }
    }
}

/// Scaled perimeter spacing and external perimeter width of a layer (`get_perimeter_spacing`,
/// `get_external_perimeter_width` for one region).
fn acp_flows(s: &PrintSettings, layer: usize, height: f64) -> (f32, f32) {
    let width = |w: f64| {
        if layer == 0 && s.raft_layers == 0 {
            s.first_layer_extrusion_width
        } else {
            w
        }
    };
    let flow = |w: f64| mica_core::flow::Flow::new(width(w), height);
    (
        flow(s.perimeter_extrusion_width).scaled_spacing() as f32,
        // Flow::scaled_width(): coord_t(scale_(float width)), which truncates (0.45 -> 449999).
        ((f64::from(width(s.external_perimeter_extrusion_width) as f32) / 1e-6) as i64) as f32,
    )
}

fn acp_layer_geometry<'a>(
    obj: &'a dyn LayerSource,
    i: usize,
    s: &PrintSettings,
) -> Option<crate::prusa_avoid_crossing::LayerGeometry<'a>> {
    let lslices = obj.layer_outlines(i).filter(|l| !l.is_empty())?;
    Some(crate::prusa_avoid_crossing::LayerGeometry {
        lslices,
        tops: obj.layer_top_surfaces(i).unwrap_or(&[]),
        perimeter_spacing: acp_flows(s, i, obj.layer_z(i).1).0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mica_core::mesh::TriangleMesh;
    use mica_core::print::slice_object;

    /// The CORE One HF0.4 / 0.20mm SPEED / Prusament PLA config, as resolved by PrusaSlicer.
    fn core_one_config() -> Config {
        let mut c = Config::defaults();
        let text = include_str!("../../mica-profiles/tests/fixtures/coreone_hf04_speed_pla.ini");
        for line in text.lines().filter(|l| !l.starts_with('#')) {
            if let Some((k, v)) = line
                .split_once(" = ")
                .or_else(|| line.strip_suffix(" =").map(|k| (k, "")))
            {
                c.set_raw(k, v).unwrap();
            }
        }
        c
    }

    fn cube_gcode(center: (f32, f32)) -> Result<Exported, ExportError> {
        cube_gcode_with(center, &[])
    }

    fn cube_gcode_with(center: (f32, f32), set: &[(&str, &str)]) -> Result<Exported, ExportError> {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/models/");
        let mut m = TriangleMesh::load_stl(format!("{dir}cube20.stl")).unwrap();
        m.place_on_bed(center.0, center.1);
        let mut config = core_one_config();
        for (k, v) in set {
            config.set_raw(k, v).unwrap();
        }
        let s = PrintSettings::from_config(&config).unwrap();
        let obj = slice_object(&m, &s).unwrap();
        export_gcode(&obj, &config, &s, "cube20.stl")
    }

    #[test]
    fn start_sequence_matches_prusaslicer() {
        // Everything PrusaSlicer writes between its header and the first layer, generated from the
        // profile's own templates (blank lines and PrusaSlicer's M73 progress lines aside).
        let out = cube_gcode((125.0, 110.0)).unwrap();
        let ours: Vec<&str> = out.gcode
            [out.gcode.find(";TYPE:Custom").unwrap()..out.gcode.find(";LAYER_CHANGE").unwrap()]
            .lines()
            .filter(|l| !l.is_empty() && !l.starts_with("M73 "))
            .collect();
        let expected: Vec<&str> = include_str!("../tests/fixtures/coreone_cube20_start.gcode")
            .lines()
            .skip(4)
            .filter(|l| !l.is_empty() && !l.starts_with("M73 "))
            .chain([
                "M572 S0.036 ; Filament gcode",
                "M142 S36 ; set heatbreak target temp",
                "M107",
            ])
            .collect();
        assert_eq!(ours[0], ";TYPE:Custom");
        assert_eq!(&ours[1..], &expected[..]);
        assert!(out.gcode.starts_with("; generated by Mica Slicer"));
        assert!(
            out.gcode
                .contains("M201 X10000 Y10000 Z400 E5000 ; sets maximum accelerations, mm/sec^2\n")
        );
        assert!(
            out.gcode
                .contains("M205 J0.010 ; sets the junction deviation, mm\n")
        );
    }

    #[test]
    fn layer_and_end_templates_run() {
        let g = cube_gcode((125.0, 110.0)).unwrap().gcode;
        assert_eq!(g.matches(";LAYER_CHANGE").count(), 100);
        assert!(g.contains(";BEFORE_LAYER_CHANGE\nG92 E0.0\n;0.4\n"));
        assert!(g.contains(";AFTER_LAYER_CHANGE\n;0.4"));
        assert!(g.contains("; Filament-specific end gcode"));
        assert!(g.contains("G1 Z21 F720 ; Move print head up"));
        assert!(g.contains("; max_layer_z = 20"));
        // CoolingBuffer prepends the fan update before the layer-change marker.
        assert!(g.contains("M106 S127.5\n;LAYER_CHANGE\n;Z:.4\n"));
    }

    /// The Z words of the travels and Z moves (no extrusions) from the second layer on.
    fn travel_zs(g: &str) -> Vec<f64> {
        let second = g.match_indices(";LAYER_CHANGE").nth(1).unwrap().0;
        let end = g.find("; Filament-specific end gcode").unwrap();
        g[second..end]
            .lines()
            .filter(|l| l.starts_with("G1 ") && !l.contains(" E"))
            .filter_map(|l| {
                l.split_whitespace()
                    .find_map(|w| w.strip_prefix('Z'))
                    .map(|z| z.parse().unwrap())
            })
            .collect()
    }

    #[test]
    fn z_offset_moves_every_z_as_prusaslicer() {
        // PrusaSlicer 2.9.6, CORE One HF0.4 / 0.20mm SPEED / Prusament PLA, cube20 at 125,110,
        // `--z-offset 0.5`: the first layer reads ";Z:0.7 ;HEIGHT:0.7", lifts from 2 × z_offset
        // (preamble + change_layer) to Z1.6, travels and lowers straight to Z.7; later layers
        // and their layer G-code carry print_z + 0.5; the end G-code
        // `G1 Z{z_offset+min(max_layer_z+1, …)}` parks at Z22 with max_layer_z = 20.5.
        let base = cube_gcode((125.0, 110.0)).unwrap().gcode;
        let g = cube_gcode_with((125.0, 110.0), &[("z_offset", "0.5")])
            .unwrap()
            .gcode;
        let first = &g[g.find(";LAYER_CHANGE").unwrap()..g.find(";TYPE:Perimeter").unwrap()];
        assert!(
            first.starts_with(";LAYER_CHANGE\n;Z:.7\n;HEIGHT:.7\n"),
            "{first}"
        );
        assert!(first.contains("G1 Z1.6 F720\n"), "{first}");
        assert!(
            first.ends_with("G1 Z.7 F720\nG1 E.7 F1500\nM204 P500\n"),
            "{first}"
        );
        assert!(!first.contains("G1 Z1 F720"), "{first}");
        // The extrusion height differs from the first layer's marker (as upstream writes it).
        assert!(g.contains(";TYPE:Perimeter\n;WIDTH:.499999\n;HEIGHT:.2\n"));
        assert!(g.contains(";Z:.9\n"));
        assert!(g.contains(";AFTER_LAYER_CHANGE\n;0.9"));
        assert!(g.contains("G1 Z22 F720 ; Move print head up"));
        assert!(g.contains("; max_layer_z = 20.5"));
        // Every later travel and Z move is the same move 0.5 mm higher.
        let (zs, zs0) = (travel_zs(&g), travel_zs(&base));
        assert_eq!(zs.len(), zs0.len());
        for (z, z0) in zs.iter().zip(&zs0) {
            assert!((z - z0 - 0.5).abs() < 1.5e-3, "{z} vs {z0}");
        }
        // No offset, no change.
        assert_eq!(
            cube_gcode_with((125.0, 110.0), &[("z_offset", "0")])
                .unwrap()
                .gcode,
            base
        );
    }

    #[test]
    fn every_move_is_on_the_object_and_extrusion_is_positive() {
        let out = cube_gcode((125.0, 110.0)).unwrap();
        let start = out.gcode.find(";LAYER_CHANGE").unwrap();
        let end = out.gcode.find("; Filament-specific end gcode").unwrap();
        for line in out.gcode[start..end]
            .lines()
            .filter(|l| l.starts_with("G1 X"))
        {
            let val = |k: char| {
                line.split_whitespace()
                    .find(|t| t.starts_with(k))
                    .map(|t| t[1..].parse::<f64>().unwrap())
            };
            let (x, y) = (val('X').unwrap(), val('Y').unwrap());
            assert!(
                (115.0..=135.0).contains(&x) && (100.0..=120.0).contains(&y),
                "{line}"
            );
            if let Some(e) = val('E') {
                assert!(e > 0.0 && e < 2.0, "{line}");
            }
        }
    }

    #[test]
    fn binary_export_decodes_to_the_text_program() {
        use crate::bgcode::{Metadata, decode};
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/models/");
        let mut m = TriangleMesh::load_stl(format!("{dir}cube20.stl")).unwrap();
        m.place_on_bed(125.0, 110.0);
        let config = core_one_config();
        let s = PrintSettings::from_config(&config).unwrap();
        let obj = slice_object(&m, &s).unwrap();
        let tmp = std::env::temp_dir().join(format!("mica-bgcode-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let (bin, text) = (tmp.join("cube.bgcode"), tmp.join("cube.gcode"));
        export_file(&obj, &config, &s, "cube20.stl", Some(&m), &bin).unwrap();
        export_file(&obj, &config, &s, "cube20.stl", Some(&m), &text).unwrap();
        let decoded = decode(&std::fs::read(&bin).unwrap()).unwrap();
        let expected = [(16, 16), (313, 173), (480, 240), (380, 285)];
        assert_eq!(decoded.metadata.thumbnails.len(), expected.len());
        for (thumbnail, (w, h)) in decoded.metadata.thumbnails.iter().zip(expected) {
            use crate::bgcode::ThumbnailFormat;
            use crate::thumbnails::tests::{decode_png, decode_qoi};
            assert_eq!((thumbnail.width, thumbnail.height), (w, h));
            assert_eq!(
                thumbnail.format,
                if w == 380 {
                    ThumbnailFormat::Png
                } else {
                    ThumbnailFormat::Qoi
                }
            );
            let (iw, ih, pixels) = if w == 380 {
                decode_png(&thumbnail.data)
            } else {
                decode_qoi(&thumbnail.data)
            };
            assert_eq!((iw, ih), (u32::from(w), u32::from(h)));
            assert!(pixels.as_chunks::<4>().0.iter().any(|p| p[3] != 0));
        }
        // Binary thumbnails are blocks, never a duplicate base64 payload in G-code blocks.
        assert!(!decoded.gcode.contains("thumbnail begin"));
        assert!(!decoded.gcode.contains("thumbnail_QOI begin"));
        let text_gcode = std::fs::read_to_string(&text).unwrap();
        let mut blocks = Vec::new();
        crate::thumbnails::write_text(&mut blocks, &decoded.metadata.thumbnails, false).unwrap();
        let blocks = String::from_utf8(blocks).unwrap();
        assert!(text_gcode.contains(&blocks));
        assert!(
            text_gcode.find("thumbnail_QOI begin").unwrap()
                < text_gcode.find("extrusion width").unwrap()
        );
        let text_without_thumbnails = text_gcode.replacen(&blocks, "", 1);
        let (text_program, text_config) = text_without_thumbnails
            .split_once("; prusaslicer_config = begin")
            .unwrap();
        assert!(!decoded.gcode.contains("prusaslicer_config"));
        let Metadata::Ini(slicer) = &decoded.metadata.slicer[0] else {
            panic!("INI slicer metadata")
        };
        let mut text_pairs: Vec<_> = text_config
            .lines()
            .filter_map(|l| l.strip_prefix("; ")?.split_once(" = "))
            .filter(|(k, _)| *k != "prusaslicer_config")
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        // The file-format selection is the only container-specific config value.
        for (k, v) in &mut text_pairs {
            if k == "binary_gcode" {
                *v = config.raw(k).unwrap();
            }
        }
        assert_eq!(&text_pairs, slicer);
        // MeatPack drops inline comments (PrusaSlicer's .bgcode does the same); everything else is equal.
        let strip = |g: &str| -> Vec<String> {
            g.lines()
                .map(|l| {
                    if l.starts_with(';') {
                        l.to_string()
                    } else {
                        l.split(';').next().unwrap().trim_end().to_string()
                    }
                })
                .filter(|l| !l.is_empty())
                .collect()
        };
        assert_eq!(strip(&decoded.gcode), strip(text_program));
        let Metadata::Ini(printer) = &decoded.metadata.printer else {
            panic!("INI printer metadata")
        };
        let get = |k: &str| {
            printer
                .iter()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(get("printer_model"), Some("COREONE"));
        assert_eq!(get("max_layer_z"), Some("20.00"));
        assert!(get("objects_info").unwrap().contains("[135.000,120.000]"));
        assert!(get("estimated printing time (normal mode)").is_some());
        assert!(get("estimated printing time (silent mode)").is_some());
        let Metadata::Ini(print) = &decoded.metadata.print else {
            panic!("INI print metadata")
        };
        for mode in ["normal", "silent"] {
            for label in [
                "estimated printing time",
                "estimated first layer printing time",
            ] {
                let key = format!("{label} ({mode} mode)");
                let value = &print.iter().find(|(k, _)| k == &key).unwrap().1;
                assert!(decoded.gcode.contains(&format!("; {key} = {value}")));
            }
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn batched_layers_give_the_same_program() {
        // All layers computed up front vs. fetched in batches of LAYER_BATCH: identical G-code.
        let config = core_one_config();
        let s = PrintSettings::from_config(&config).unwrap();
        for model in ["hole_block.stl", "thin_features.stl", "cube20.stl"] {
            let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/models/");
            let mut m = TriangleMesh::load_stl(format!("{dir}{model}")).unwrap();
            m.place_on_bed(125.0, 110.0);
            let all = slice_object(&m, &s).unwrap();
            let prepared = mica_core::print::prepare_object(&m, &s).unwrap();
            let mut a = Vec::new();
            let mut b = Vec::new();
            export(&all, &config, &s, model, Some(&m), &[], &mut a).unwrap();
            export(&prepared, &config, &s, model, Some(&m), &[], &mut b).unwrap();
            assert!(a == b, "{model}: batched output differs");
        }
    }

    #[test]
    fn rejects_objects_off_the_bed() {
        assert!(matches!(
            cube_gcode((5.0, 110.0)).unwrap_err(),
            ExportError::OutOfBounds { .. }
        ));
    }
}
