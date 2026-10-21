// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: orca@8500fcdcca src/libslic3r/PrintConfig.cpp :: PrintConfigDef::handle_legacy, PrintConfigDef::handle_legacy_composite
// Source: orca@8500fcdcca src/libslic3r/Config.cpp :: ConfigBase::load_from_json
// Changes: Preserve old names alongside effective replacements; unsupported obsolete options remain
// Changes: extras. No forward-compatibility default substitution of invalid values.
// Changes: "zig-zag" patterns become rectilinear, a negative ironing_angle 0, wall_direction
// Changes: "auto" ccw, and old percentage speeds / XY distances are dropped, as handle_legacy does.
// Changes: BambuStudio's top_one_wall_type turns only_one_wall_top on; PrusaSlicer values older Mica
// Changes: wrote for enable_arc_fitting, ensure_vertical_shell_thickness and only_one_wall_top read as meant.

use super::{Dialect, RawConfig, schema};
use serde_json::Value;

/// Original file overrides remain in Preset::values. Effective values are parsed once per layer.
pub(super) fn parsed(raw: &RawConfig, dialect: Dialect) -> RawConfig {
    let mut out = RawConfig::new();
    for (key, value) in raw {
        let parsed = if let Some(d) = schema::schema(dialect).get(key) {
            schema::normalized(value, d).unwrap_or_else(|_| value.clone())
        } else {
            value.clone()
        };
        out.insert(key.clone(), parsed);
    }
    let aliases = [
        ("enable_wipe_tower", "enable_prime_tower"),
        ("wipe_tower_width", "prime_tower_width"),
        ("wiping_volume", "prime_volume"),
        ("wipe_tower_brim_width", "prime_tower_brim_width"),
        ("tool_change_gcode", "change_filament_gcode"),
        ("bridge_fan_speed", "overhang_fan_speed"),
        ("wipe_tower_extruder", "wipe_tower_filament"),
        ("support_material_extruder", "support_filament"),
        (
            "support_material_interface_extruder",
            "support_interface_filament",
        ),
        ("support_material_angle", "support_angle"),
        ("support_material_enforce_layers", "enforce_support_layers"),
        ("inherits_cummulative", "inherits_group"),
        (
            "compatible_printers_condition_cummulative",
            "compatible_machine_expression_group",
        ),
        (
            "compatible_prints_condition_cummulative",
            "compatible_process_expression_group",
        ),
        ("cooling", "slow_down_for_layer_cooling"),
        ("sparse_infill_anchor", "infill_anchor"),
        ("sparse_infill_anchor_max", "infill_anchor_max"),
        ("chamber_temperatures", "chamber_temperature"),
        ("thumbnail_size", "thumbnails"),
        ("initial_layer_flow_ratio", "bottom_solid_infill_flow_ratio"),
        ("ironing_direction", "ironing_angle"),
        ("counterbole_hole_bridging", "counterbore_hole_bridging"),
        (
            "prime_tower_extra_rib_length",
            "wipe_tower_extra_rib_length",
        ),
        ("prime_tower_rib_width", "wipe_tower_rib_width"),
        ("prime_tower_fillet_wall", "wipe_tower_fillet_wall"),
        ("extruder_clearance_max_radius", "extruder_clearance_radius"),
        ("machine_switch_extruder_time", "machine_tool_change_time"),
    ];
    for (old, new) in aliases {
        if let Some(v) = raw.get(old)
            && !raw.contains_key(new)
            && let Some(d) = schema::schema(dialect).get(new)
        {
            out.insert(
                new.into(),
                schema::normalized(v, d).unwrap_or_else(|_| v.clone()),
            );
        }
    }
    if let Some(s) = raw.get("wall_infill_order").and_then(Value::as_str) {
        let sequence = match s {
            "inner wall/outer wall/infill" | "infill/inner wall/outer wall" => {
                "inner wall/outer wall"
            }
            "outer wall/inner wall/infill" | "infill/outer wall/inner wall" => {
                "outer wall/inner wall"
            }
            "inner-outer-inner wall/infill" => "inner-outer-inner wall",
            _ => s,
        };
        if !raw.contains_key("wall_sequence") {
            out.insert("wall_sequence".into(), Value::String(sequence.into()));
        }
        if s.starts_with("infill/") {
            out.insert("is_infill_first".into(), Value::String("1".into()));
        }
    }
    for (key, old, new) in [
        ("support_type", "normal", "normal(manual)"),
        ("support_type", "tree", "tree(manual)"),
        ("support_type", "hybrid(auto)", "tree(auto)"),
        ("support_base_pattern", "none", "hollow"),
        ("ensure_vertical_shell_thickness", "1", "ensure_all"),
        ("ensure_vertical_shell_thickness", "0", "ensure_moderate"),
        ("curr_bed_type", "SuperTack Plate", "Supertack Plate"),
        // Mica before 2026-10 wrote PrusaSlicer's vocabulary for these (OrcaSlicer would
        // substitute its default or "off"); they read as what Mica meant.
        ("enable_arc_fitting", "emit_center", "1"),
        ("enable_arc_fitting", "disabled", "0"),
        ("ensure_vertical_shell_thickness", "enabled", "ensure_all"),
        (
            "ensure_vertical_shell_thickness",
            "partial",
            "ensure_moderate",
        ),
        ("ensure_vertical_shell_thickness", "disabled", "none"),
        ("only_one_wall_top", "none", "0"),
        ("only_one_wall_top", "top", "1"),
        ("only_one_wall_top", "topmost", "1"),
    ] {
        if raw.get(key).and_then(Value::as_str) == Some(old) {
            out.insert(key.into(), Value::String(new.into()));
        }
    }
    if raw.get("only_one_wall_top").and_then(Value::as_str) == Some("topmost")
        && !raw.contains_key("top_one_wall_type")
    {
        out.insert("top_one_wall_type".into(), Value::String("topmost".into()));
    }
    // BambuStudio's top_one_wall_type: any kind but none turns Orca's switch on (and the
    // mapping keeps Topmost surface only).
    if raw
        .get("top_one_wall_type")
        .and_then(Value::as_str)
        .is_some_and(|s| s != "none")
    {
        out.insert("only_one_wall_top".into(), Value::String("1".into()));
    }
    for key in [
        "sparse_infill_pattern",
        "top_surface_pattern",
        "bottom_surface_pattern",
        "internal_solid_infill_pattern",
        "ironing_pattern",
        "support_ironing_pattern",
    ] {
        if raw.get(key).and_then(Value::as_str) == Some("zig-zag") {
            out.insert(key.into(), Value::String("rectilinear".into()));
        }
    }
    // Old BBS profiles gave these as a percentage; Orca drops such a value (the inherited or
    // default one applies) rather than reading it as millimetres.
    for key in [
        "initial_layer_print_height",
        "initial_layer_speed",
        "internal_solid_infill_speed",
        "top_surface_speed",
        "support_interface_speed",
        "outer_wall_speed",
        "support_object_xy_distance",
    ] {
        if raw
            .get(key)
            .and_then(Value::as_str)
            .is_some_and(|s| s.contains('%'))
        {
            out.remove(key);
        }
    }
    if raw.get("wall_direction").and_then(Value::as_str) == Some("auto") {
        out.insert("wall_direction".into(), Value::String("ccw".into()));
    }
    if raw
        .get("ironing_angle")
        .and_then(Value::as_str)
        .is_some_and(|s| s.starts_with('-'))
    {
        out.insert("ironing_angle".into(), Value::String("0".into()));
    }
    if raw.get("support_type").and_then(Value::as_str) == Some("hybrid(auto)") {
        out.insert("support_style".into(), Value::String("tree_hybrid".into()));
    }
    // Composite thumbnail migration. Orca appends the separately selected format to bare WxH.
    if let Some(s) = out.get("thumbnails").and_then(Value::as_str) {
        let format = out
            .get("thumbnails_format")
            .and_then(Value::as_str)
            .unwrap_or("PNG");
        let value = s
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| {
                if s.contains('/') {
                    s.to_string()
                } else {
                    format!("{s}/{format}")
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
        out.insert("thumbnails".into(), Value::String(value));
    }
    if !out.contains_key("wiping_volumes_use_custom_matrix")
        && let Some(a) = out.get("wiping_volumes_matrix").and_then(Value::as_array)
    {
        let n = (a.len() as f64).sqrt().round() as usize;
        if n > 0 && n * n == a.len() {
            let custom = a.iter().enumerate().any(|(i, v)| {
                i / n != i % n && v.as_str().and_then(|s| s.parse::<f64>().ok()) != Some(140.0)
            });
            out.insert(
                "wiping_volumes_use_custom_matrix".into(),
                Value::String(if custom { "1" } else { "0" }.into()),
            );
        }
    }
    out
}
