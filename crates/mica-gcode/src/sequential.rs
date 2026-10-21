// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/GCode.cpp :: GCodeGenerator::_do_export, GCodeGenerator::_print_first_layer_bed_temperature, GCodeGenerator::_print_first_layer_extruder_temperatures, GCodeGenerator::travel_to_first_position
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/GCodeWriter.cpp :: GCodeWriter::set_temperature, GCodeWriter::set_bed_temperature
// Source: orca@8500fcdcca src/libslic3r/GCode.cpp :: GCode::_do_export, GCode::_print_first_layer_extruder_temperatures
// Changes: The move between two sequentially printed objects as a writer helper: the exporter runs the
// Changes: between-objects template and the first object's state resets itself.
// Changes: Engine choices read mica_slicing_rules (mica_core::rules), not gcode_flavor or the presence of Orca keys (docs/slicing-rules.md).

//! Between two objects printed one after another: lift clear of everything printed, travel
//! to the next object's centre, and set its first-layer temperatures again.

use crate::writer::{Writer, num};
use mica_core::config::Config;
use mica_core::settings::PrintSettings;
use mica_core::toolpath::LayerToolpaths;

/// After the retraction and the label end: up to the highest printed Z and over to `center`.
///
/// PrusaSlicer writes the current Z again ("ensure z position"), rises to the highest layer
/// so far ("clear all already printed objects") and travels like any travel, so the ramping
/// lift applies. Orca lifts by its Z hop on retraction, rises to the highest layer plus the
/// Z hop (on top of the lift still pending) and moves there in one XYZ move.
pub(crate) fn travel_to_next_object(
    w: &mut Writer,
    c: &Config,
    s: &PrintSettings,
    orca: bool,
    center: (f64, f64),
    max_layer_z: f64,
) {
    let z_speed = if s.travel_speed_z > 0. {
        s.travel_speed_z
    } else {
        s.travel_speed
    };
    if orca {
        let hop = c
            .float("filament_z_hop")
            .or_else(|| c.float("z_hop"))
            .or_else(|| c.float("retract_lift"))
            .unwrap_or(0.)
            .max(0.);
        let lift = hop;
        if lift > 0. {
            w.travel_to_z_force(w.z() + lift, z_speed);
        }
        let z = max_layer_z + hop + lift;
        w.travel_to_z_force(z, z_speed);
        w.travel_to_xyz_force([center.0, center.1, z], s.travel_speed, 0.);
        return;
    }
    let last = w.z();
    w.travel_to_z_force(last, z_speed);
    let z = last.max(max_layer_z);
    w.travel_to_z_force(z, z_speed);
    let Some(from) = w.position() else {
        w.travel_to(center.0, center.1, s.travel_speed);
        return;
    };
    let layer = LayerToolpaths {
        support_groups: Vec::new(),
        index: 1,
        print_z: z,
        height: 0.,
        paths: Vec::new(),
    };
    let planner = crate::travels::TravelPlanner::new(c, &layer, None, None);
    for point in planner.plan(from, center, 0, s.retract_length > 0.) {
        w.travel_to_xyz(point, s.travel_speed, z_speed);
    }
}

/// First-layer bed and nozzle temperatures before the next object, without waiting, unless
/// the between-objects G-code sets them itself. `bed` is the bed target the writer last
/// set; it is updated. `first` prints first; `tools` are all printing tools (a T parameter
/// is written when more than one prints).
pub(crate) fn first_layer_temperatures(
    w: &mut Writer,
    c: &Config,
    between: &str,
    first: usize,
    tools: &[usize],
    orca: bool,
    bed: &mut f64,
) {
    let (bed_set, bed_value) = crate::templates::sets_temperature(between, 140, 190);
    let target = c
        .float_at("first_layer_bed_temperature", first)
        .unwrap_or(0.);
    if bed_set {
        if let Some(t) = bed_value {
            *bed = t as f64;
        }
    } else if target != *bed {
        *bed = target;
        w.raw(&format!("M140 S{} ; set bed temperature", num(target, 0)));
    }
    if crate::templates::sets_temperature(between, 104, 109).0 {
        return;
    }
    let ooze = !orca && c.bool("ooze_prevention").unwrap_or(false);
    for &tool in tools {
        let mut temp = c.float_at("first_layer_temperature", tool).unwrap_or(0.);
        if ooze && tool != first {
            temp = c
                .float_at("idle_temperature", tool)
                .filter(|v| *v > 0.)
                .unwrap_or(temp + c.float("standby_temperature_delta").unwrap_or(0.));
        }
        if temp <= 0. {
            continue;
        }
        let t = if tools.len() > 1 {
            format!(" T{tool}")
        } else {
            String::new()
        };
        let comment = if orca {
            "set nozzle temperature"
        } else {
            "set temperature"
        };
        w.raw(&format!("M104 S{}{t} ; {comment}", num(temp, 0)));
    }
}
