// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: snapmaker-orca@b1831e5dcb src/libslic3r/PrintConfig.cpp :: process_flow_variant_options, get_config_idx, flow_variant_index
// Source: snapmaker-orca@b1831e5dcb src/libslic3r/PrintConfig.hpp :: get_value_at
// Source: orca@8500fcdcca src/libslic3r/Config.hpp :: ConfigOptionBool::deserialize
// Changes: Snapmaker Orca reads a flow-variant vector per extruder at print time; Mica's options of
// Changes: this kind are single values, so the vector becomes the element every filament reads, and
// Changes: the standard (first) element with a note when filaments would read different elements.

//! Snapmaker Orca keeps many process options (speeds, accelerations, jerks, overhang slowdown)
//! as vectors with one value per declared flow variant (`process_flow_support`, e.g.
//! `["standard", "high_flow"]`). While printing it reads the element of the current
//! filament's flow type (`filament_volume_type`), and the first (standard) one when the type
//! is not declared (`get_config_idx`, Process domain). OrcaSlicer 2.4.2 defines the same
//! options as single values and reads the first element of such a list
//! (`ConfigOptionBool::deserialize`: "take the first value if input is an array"; the numeric
//! options read the leading number). Mica's options are single values too, so a list becomes
//! the one element the filaments read; filaments of different flow types reading different
//! values is a per-tool setting Mica cannot hold: the standard value is used and a note says so.

use serde_json::Value;

use super::{Dialect, RawConfig, schema};

/// Snapmaker Orca's process options with one value per declared flow variant
/// (snapmaker-orca `process_flow_variant_options`).
pub(super) const PROCESS_FLOW_VARIANT_OPTIONS: [&str; 46] = [
    "outer_wall_speed",
    "inner_wall_speed",
    "sparse_infill_speed",
    "internal_solid_infill_speed",
    "top_surface_speed",
    "gap_infill_speed",
    "bridge_speed",
    "internal_bridge_speed",
    "ironing_speed",
    "small_perimeter_speed",
    "small_perimeter_threshold",
    "support_speed",
    "support_interface_speed",
    "initial_layer_speed",
    "initial_layer_infill_speed",
    "initial_layer_travel_speed",
    "travel_speed",
    "slow_down_layers",
    "overhang_1_4_speed",
    "overhang_2_4_speed",
    "overhang_3_4_speed",
    "overhang_4_4_speed",
    "enable_overhang_speed",
    "slowdown_for_curled_perimeters",
    "default_acceleration",
    "outer_wall_acceleration",
    "inner_wall_acceleration",
    "top_surface_acceleration",
    "initial_layer_acceleration",
    "bridge_acceleration",
    "travel_acceleration",
    "sparse_infill_acceleration",
    "internal_solid_infill_acceleration",
    "accel_to_decel_enable",
    "accel_to_decel_factor",
    "default_jerk",
    "outer_wall_jerk",
    "inner_wall_jerk",
    "infill_jerk",
    "top_surface_jerk",
    "initial_layer_jerk",
    "travel_jerk",
    "default_junction_deviation",
    "max_volumetric_extrusion_rate_slope",
    "max_volumetric_extrusion_rate_slope_segment_length",
    "extrusion_rate_smoothing_external_perimeter_only",
];

/// Single-value option types whose text a flow-variant list can stand in for.
fn scalar(ty: &str) -> bool {
    matches!(ty, "Bool" | "Int" | "Float" | "Percent" | "FloatOrPercent")
}

/// The elements of a list value: a JSON array, or `,` / `;`-separated text (quoted strings
/// as Orca writes string vectors).
fn items(value: &Value) -> Vec<String> {
    match value {
        Value::Array(a) => a
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
        Value::String(s) if s.trim_start().starts_with('"') => {
            mica_core::config::unescape_strings(s).unwrap_or_default()
        }
        Value::String(s) if s.trim().is_empty() => Vec::new(),
        Value::String(s) => s.split([',', ';']).map(|t| t.trim().to_string()).collect(),
        _ => Vec::new(),
    }
}

/// The flow-variant index each filament reads (snapmaker-orca `get_config_idx`, Process
/// domain): its `filament_volume_type` in `process_flow_support`, 0 when either is missing
/// or the type is not declared.
fn indices(raw: &RawConfig) -> Vec<usize> {
    let support = raw
        .get("process_flow_support")
        .map(items)
        .unwrap_or_default();
    let types = raw
        .get("filament_volume_type")
        .map(items)
        .unwrap_or_default();
    if support.is_empty() || types.is_empty() {
        return vec![0];
    }
    types
        .iter()
        .map(|t| support.iter().position(|s| s == t).unwrap_or(0))
        .collect()
}

/// `raw` with every flow-variant list of a single-value option (in `dialect`'s schema) replaced
/// by the element the filaments read, and a note for each list whose filaments would read
/// different values.
pub(super) fn resolved(raw: &RawConfig, dialect: Dialect) -> (RawConfig, Vec<String>) {
    let mut out = raw.clone();
    let mut notes = Vec::new();
    let mut index = None;
    for (key, value) in raw {
        if !schema::option_type(key, dialect).is_some_and(scalar) {
            continue;
        }
        let flow = PROCESS_FLOW_VARIANT_OPTIONS.contains(&key.as_str());
        // Text lists are only flow variants for the flow options: elsewhere "2,5" is a
        // decimal comma, which the value parser reads as Orca does (2).
        let list = match value {
            Value::Array(a) if !a.is_empty() => items(value).len() == a.len(),
            Value::String(s) => flow && s.contains(',') && !s.contains('"'),
            _ => false,
        };
        if !list {
            continue;
        }
        let values = items(value);
        let Some(first) = values.first().cloned() else {
            continue;
        };
        let at = |i: usize| values.get(i).cloned().unwrap_or_else(|| first.clone());
        let picked = if flow {
            let index = index.get_or_insert_with(|| indices(raw));
            let mut read: Vec<usize> = index.clone();
            read.sort_unstable();
            read.dedup();
            if let [only] = read[..] {
                at(only)
            } else {
                let mut seen: Vec<String> = read.iter().map(|&i| at(i)).collect();
                seen.dedup();
                if seen.len() > 1 {
                    notes.push(format!(
                        "{key}: Snapmaker Orca uses a different value for each filament flow type \
                         ({}); Mica has one value for all filaments and uses the standard one, {first}",
                        seen.join(", ")
                    ));
                }
                first.clone()
            }
        } else {
            // OrcaSlicer reads the first value of a list for a single-value option.
            first.clone()
        };
        out.insert(key.clone(), Value::String(picked));
    }
    (out, notes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn raw(v: Value) -> RawConfig {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn the_list_matches_snapmaker_orca_vector_options_scalar_in_orca() {
        for key in PROCESS_FLOW_VARIANT_OPTIONS {
            let orca = schema::option_type(key, Dialect::Orca242).unwrap();
            let snap = schema::option_type(key, Dialect::Snapmaker240).unwrap();
            assert!(scalar(orca), "{key}: {orca}");
            assert_eq!(
                snap,
                format!("{}s", orca.replace("FloatOrPercent", "FloatsOrPercent"))
            );
        }
    }

    #[test]
    fn uniform_lists_read_as_their_value() {
        let (out, notes) = resolved(
            &raw(json!({
                "enable_overhang_speed": ["1", "1"],
                "outer_wall_speed": "60,60",
                "slow_down_layers": ["2", "2"],
                "overhang_1_4_speed": ["50%", "50%"],
                "accel_to_decel_factor": "50%,50%",
            })),
            Dialect::Orca242,
        );
        assert!(notes.is_empty());
        assert_eq!(out["enable_overhang_speed"], "1");
        assert_eq!(out["outer_wall_speed"], "60");
        assert_eq!(out["slow_down_layers"], "2");
        assert_eq!(out["overhang_1_4_speed"], "50%");
        assert_eq!(out["accel_to_decel_factor"], "50%");
    }

    #[test]
    fn filaments_read_their_flow_variant() {
        let base = json!({
            "outer_wall_speed": ["200", "300"],
            "process_flow_support": ["standard", "high_flow"],
        });
        // No flow types: every filament reads the standard value.
        let (out, notes) = resolved(&raw(base.clone()), Dialect::Orca242);
        assert_eq!(
            (out["outer_wall_speed"].as_str(), notes.len()),
            (Some("200"), 0)
        );
        // All high flow: the high-flow value.
        let mut all = base.clone();
        all["filament_volume_type"] = json!(["high_flow", "high_flow"]);
        let (out, notes) = resolved(&raw(all), Dialect::Orca242);
        assert_eq!(
            (out["outer_wall_speed"].as_str(), notes.len()),
            (Some("300"), 0)
        );
        // Mixed: the standard value, with a note.
        let mut mixed = base;
        mixed["filament_volume_type"] = json!("standard,high_flow");
        let (out, notes) = resolved(&raw(mixed), Dialect::Orca242);
        assert_eq!(out["outer_wall_speed"], "200");
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(notes[0].contains("200, 300"), "{notes:?}");
    }

    /// The value parser reads a list for a single value as Orca does: its first value.
    #[test]
    fn single_values_read_the_first_of_a_list() {
        let read = |key: &str, text: &str| {
            let def = &schema::schema(Dialect::Orca242)[key];
            schema::normalized(&Value::String(text.into()), def)
        };
        assert_eq!(read("enable_overhang_speed", "1,1"), Ok(json!("1")));
        assert_eq!(read("enable_overhang_speed", "0,1"), Ok(json!("0")));
        assert!(read("enable_overhang_speed", "yes").is_err());
        assert_eq!(read("slow_down_layers", "3,3"), Ok(json!("3")));
        // Decimal commas keep their meaning (istream reads 2 from "2,5").
        assert_eq!(read("outer_wall_speed", "2,5"), Ok(json!("2")));
        assert_eq!(read("overhang_1_4_speed", "50%,60%"), Ok(json!("50%")));
        // A point's comma is its coordinate separator.
        assert_eq!(read("best_object_pos", "0.5,0.5"), Ok(json!("0.5,0.5")));
        // A user profile's text is typed the same way.
        let mut c = mica_core::config::Config::new();
        schema::set_option_text(&mut c, "enable_overhang_speed", "1,1", Dialect::Orca242).unwrap();
        assert_eq!(c.bool("enable_overhang_speed"), Some(true));
        assert_eq!(c.extra("enable_overhang_speed"), Some("1"));
    }

    #[test]
    fn decimal_commas_and_vector_options_are_left_alone() {
        let input = raw(json!({
            // Not a flow option: "2,5" is a decimal comma Orca reads as 2.
            "wall_loops": "2,5",
            "nozzle_diameter": ["0.4", "0.4"],
            "outer_wall_speed": "80",
        }));
        let (out, notes) = resolved(&input, Dialect::Orca242);
        assert_eq!(out, input);
        assert!(notes.is_empty());
        // In Snapmaker Orca's own schema the flow options are vectors already.
        let snap = raw(json!({"enable_overhang_speed": ["1", "0"]}));
        assert_eq!(resolved(&snap, Dialect::Snapmaker240).0, snap);
    }
}
