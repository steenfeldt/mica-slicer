// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: prusaslicer@b028299c77 src/libslic3r/GCode.cpp :: ProcessLayer::emit_custom_color_change_gcode_per_print_z, ProcessLayer::emit_custom_gcode_per_print_z, GCodeGenerator::process_layer
// Source: orca@8500fcdcca src/libslic3r/GCode.cpp :: ProcessLayer::emit_custom_gcode_per_print_z, GCode::process_layer
// Changes: The layer each item lands on is planned once per export from the schedule (`mica_core::custom_gcode::assign`);
// Changes: templates run through the exporter's own template runner; the writer's discarded unretract becomes a state reset.
// Changes: Engine choices read mica_slicing_rules (mica_core::rules), not gcode_flavor or the presence of Orca keys (docs/slicing-rules.md).

// Copyright (c) Prusa Research 2016 - 2023 Vojtěch Bubník, Lukáš Matěna, Pavel Mikuš, Oleksandra Iushchenko,
// Enrico Turri, Lukáš Hejl, Filip Sykala, Tomáš Mészáros and others.
// Upstream is released under the AGPLv3 or higher; this port is AGPL-3.0-only.

//! Per-layer custom G-code in the exported program: colour changes (`M600`), pauses, custom and
//! template G-code, with the `;COLOR_CHANGE`, `;PAUSE_PRINT` and `;CUSTOM_GCODE` tags the time
//! estimate reads. PrusaSlicer defers a colour change to just before the layer's first extrusion
//! (GH #2672) or, on a tool changer, runs it right after picking the tool it is for; Orca emits
//! every item at the start of the layer and has no `M600` colour change.

use mica_core::config::Config;
use mica_core::custom_gcode::{Dialect, Item, Kind, assign};
use mica_core::print::LayerSource;
use mica_template::{Context, Var};

use crate::writer::Writer;

/// Runs a named template with the exporter's template runner.
pub(crate) type Process<'a> =
    dyn Fn(&str, &str, &mut Context, &mut Writer) -> Result<String, String> + 'a;

/// Which item each layer emits, and the tool a colour change must pick first (tool changers).
pub(crate) struct Plan {
    items: Vec<Option<Item>>,
    needed: Vec<Option<usize>>,
}

impl Plan {
    pub(crate) fn new(obj: &dyn LayerSource, config: &Config, orca: bool) -> Self {
        let n = obj.layer_count();
        let Some(info) = obj.custom_gcode().filter(|i| !i.is_empty()) else {
            return Self {
                items: vec![None; n],
                needed: vec![None; n],
            };
        };
        let job = obj.print_job();
        // Orca counts filaments, PrusaSlicer extruders.
        let tools = if orca {
            config
                .vector_len("filament_diameter")
                .max(config.vector_len("nozzle_diameter"))
                .max(1)
        } else {
            config.vector_len("nozzle_diameter").max(1)
        };
        let layers: Vec<(f64, Vec<usize>)> = (0..n)
            .map(|i| {
                (
                    obj.layer_z(i).0,
                    job.map_or_else(|| vec![0], |j| j.ordering.layers[i].extruders.clone()),
                )
            })
            .collect();
        let object_tools = job.map_or(1, |j| {
            mica_core::print_job::PreparedPrint::object_tool_count(&j.objects).max(1)
        });
        let dialect = if orca { Dialect::Orca } else { Dialect::Prusa };
        let items = assign(info, &layers, tools, object_tools, dialect)
            .into_iter()
            .map(|i| i.map(|i| info.items[i].clone()))
            .collect();
        let needed = (0..n)
            .map(|i| {
                job.and_then(|j| j.ordering.layers[i].color_change_tool)
                    .filter(|_| !orca)
            })
            .collect();
        Self { items, needed }
    }

    /// The item emitted on layer `li` and, for a colour change that needs a tool picked first,
    /// that zero-based tool.
    pub(crate) fn at(&self, li: usize) -> (Option<&Item>, Option<usize>) {
        (self.items[li].as_ref(), self.needed[li])
    }
}

/// PrusaSlicer `emit_custom_color_change_gcode_per_print_z`.
fn color_change(
    item: &Item,
    current: usize,
    first: usize,
    config: &Config,
    ctx: &mut Context,
    w: &mut Writer,
    process: &Process<'_>,
) -> Result<String, String> {
    let semm = config
        .bool("single_extruder_multi_material")
        .unwrap_or(false);
    let single_extruder_printer = config.vector_len("nozzle_diameter").max(1) == 1;
    let mut extruder: i64 = -1;
    if item.kind == Kind::ColorChange && item.extruder > 0 {
        extruder = if single_extruder_printer {
            0
        } else {
            item.extruder as i64 - 1
        };
    }
    // Colour change or tool change as colour change; the tag is for the G-code processor.
    let mut gcode = format!(";COLOR_CHANGE,T{extruder},{}\n", item.color);
    ctx.set_current_extruder(current);
    ctx.set("color_change_extruder", Var::int(extruder));
    if semm && !single_extruder_printer && extruder >= 0 && first as i64 != extruder {
        gcode += &process(
            "pause_print_gcode",
            config.string("pause_print_gcode").unwrap_or(""),
            ctx,
            w,
        )?;
        gcode += "\n";
        gcode += &format!("M117 Change filament for Extruder {extruder}\n");
    } else {
        gcode += &process(
            "color_change_gcode",
            config.string("color_change_gcode").unwrap_or(""),
            ctx,
            w,
        )?;
        gcode += "\n";
        // M600 refills the extruder: the writer counts it as unretracted (GH #6362).
        w.assume_unretracted();
    }
    Ok(gcode)
}

/// PrusaSlicer `emit_custom_gcode_per_print_z`. `current` is the active tool, `first` the first
/// tool printing the layer (zero based).
pub(crate) fn prusa(
    item: &Item,
    current: usize,
    first: usize,
    config: &Config,
    ctx: &mut Context,
    w: &mut Writer,
    process: &Process<'_>,
) -> Result<String, String> {
    if matches!(item.kind, Kind::ColorChange | Kind::ToolChange) {
        return color_change(item, current, first, config, ctx, w, process);
    }
    let mut gcode = String::new();
    ctx.set_current_extruder(current);
    if item.kind == Kind::PausePrint {
        gcode += ";PAUSE_PRINT\n";
        if !item.extra.is_empty() {
            gcode += &format!("M117 {}\n", item.extra);
        }
        ctx.set("color_change_extruder", Var::int(current as i64));
        gcode += &process(
            "pause_print_gcode",
            config.string("pause_print_gcode").unwrap_or(""),
            ctx,
            w,
        )?;
    } else {
        gcode += ";CUSTOM_GCODE\n";
        if item.kind == Kind::Template {
            gcode += &process(
                "template_custom_gcode",
                config.string("template_custom_gcode").unwrap_or(""),
                ctx,
                w,
            )?;
        } else {
            gcode += &item.extra;
        }
    }
    gcode += "\n";
    Ok(gcode)
}

/// Non-adjustable motion time the item adds to its layer, for the cooling slowdown (PrusaSlicer's
/// `CoolingBuffer` parses the custom G-code with the layer): E-only `G0`/`G1` lines of the
/// template with their own feed rate. A colour change also repays the layer's first unretract
/// (`M600` refills the extruder), which the cooling estimate counted as `deretract` seconds.
/// Templates are read unexpanded; lines with placeholders are skipped.
pub(crate) fn cooling_time(item: &Item, picked_tool: bool, config: &Config, deretract: f64) -> f64 {
    let template = match item.kind {
        Kind::ColorChange | Kind::ToolChange => config.string("color_change_gcode"),
        Kind::PausePrint => config.string("pause_print_gcode"),
        Kind::Template => config.string("template_custom_gcode"),
        Kind::Custom => Some(item.extra.as_str()),
    }
    .unwrap_or("");
    let mut seconds = 0.;
    for line in template.lines() {
        let code = line.split(';').next().unwrap_or("").trim();
        if code.contains(['{', '[']) {
            continue;
        }
        let mut words = code.split_whitespace();
        if !matches!(words.next(), Some("G0" | "G1")) {
            continue;
        }
        let (mut e, mut f, mut other) = (None, None, false);
        for w in words {
            let (axis, value) = w.split_at(1);
            let value = value.parse::<f64>().ok();
            match axis {
                "E" => e = value,
                "F" => f = value,
                _ => other = true,
            }
        }
        if let (Some(f), false, Some(e)) = (f, other, e)
            && f > 0.
        {
            seconds += e.abs() / (f / 60.);
        }
    }
    let semm_pause = config
        .bool("single_extruder_multi_material")
        .unwrap_or(false)
        && config.vector_len("nozzle_diameter") > 1;
    if matches!(item.kind, Kind::ColorChange | Kind::ToolChange) && !picked_tool && !semm_pause {
        seconds -= deretract;
    }
    seconds
}

/// Orca `emit_custom_gcode_per_print_z`: no `M600` colour change (BBS removed it), the pause
/// message is not shown and `machine_pause_gcode` (Mica's `pause_print_gcode`) runs.
pub(crate) fn orca(
    item: &Item,
    current: usize,
    config: &Config,
    ctx: &mut Context,
    w: &mut Writer,
    process: &Process<'_>,
) -> Result<String, String> {
    let mut gcode = String::new();
    ctx.set_current_extruder(current);
    if item.kind == Kind::PausePrint {
        gcode += ";PAUSE_PRINT\n";
        gcode += &process(
            "machine_pause_gcode",
            config.string("pause_print_gcode").unwrap_or(""),
            ctx,
            w,
        )?;
        gcode += "\n";
    } else {
        gcode += ";CUSTOM_GCODE\n";
        if item.kind == Kind::Template {
            gcode += &process(
                "template_custom_gcode",
                config.string("template_custom_gcode").unwrap_or(""),
                ctx,
                w,
            )?;
        } else {
            gcode += &item.extra;
        }
    }
    gcode += "\n";
    Ok(gcode)
}
