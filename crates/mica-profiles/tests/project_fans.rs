// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! An Orca project's per-filament fan values arrive per tool in the configuration Mica slices.
use mica_core::{print_job::tool_config, threemf::RawConfig};
use mica_profiles::{
    orca::{Dialect, mapping::map_sparse},
    project::{Mode, ProjectSettings},
};
use serde_json::{Value, json};

const FANS: [(&str, [&str; 4]); 12] = [
    ("fan_min_speed", ["20", "40", "60", "30"]),
    ("fan_max_speed", ["100", "80", "90", "70"]),
    ("fan_cooling_layer_time", ["60", "30", "100", "20"]),
    ("slow_down_layer_time", ["8", "4", "10", "6"]),
    ("full_fan_speed_layer", ["0", "3", "5", "0"]),
    ("close_fan_the_first_x_layers", ["1", "2", "3", "1"]),
    ("overhang_fan_speed", ["100", "70", "90", "50"]),
    ("overhang_fan_threshold", ["50%", "25%", "10%", "95%"]),
    ("enable_overhang_bridge_fan", ["1", "1", "0", "1"]),
    (
        "support_material_interface_fan_speed",
        ["-1", "50", "80", "-1"],
    ),
    ("additional_cooling_fan_speed", ["70", "30", "0", "100"]),
    ("dont_slow_down_outer_wall", ["0", "1", "0", "1"]),
];

#[test]
fn project_fan_values_arrive_per_tool() {
    // The presets: every filament alike, as four copies of one filament preset.
    let mut preset = serde_json::Map::new();
    preset.insert("nozzle_diameter".into(), json!(vec!["0.4"; 4]));
    preset.insert("auxiliary_fan".into(), json!("1"));
    for (key, values) in FANS {
        preset.insert(key.into(), json!(vec![values[0]; 4]));
    }
    let base = map_sparse(&preset.clone().into_iter().collect(), Dialect::Orca242).config;
    // The project: its own value per filament.
    let mut project: serde_json::Map<String, Value> = serde_json::Map::new();
    project.insert(
        "filament_settings_id".into(),
        json!(vec!["Generic PLA @System"; 4]),
    );
    for (key, values) in FANS {
        project.insert(key.into(), json!(values));
    }
    let settings = ProjectSettings::parse(&RawConfig {
        path: "Metadata/project_settings.config".into(),
        text: serde_json::to_string(&project).unwrap(),
    })
    .unwrap();
    let (layer, _) = settings.layer(&base, Some(Dialect::Orca242), Mode::Embedded);
    let mut config = base.clone();
    layer.apply(&mut config);
    for (key, values) in FANS {
        let canonical = match key {
            "fan_min_speed" => "min_fan_speed",
            "fan_max_speed" => "max_fan_speed",
            "fan_cooling_layer_time" => "fan_below_layer_time",
            "slow_down_layer_time" => "slowdown_below_layer_time",
            "close_fan_the_first_x_layers" => "disable_fan_first_layers",
            other => other,
        };
        for (tool, value) in values.iter().enumerate() {
            let want: f64 = value.trim_end_matches('%').parse().unwrap();
            // The G-code exporter slices with each tool's configuration.
            let c = tool_config(&config, tool);
            // An enum (`overhang_fan_threshold`) is read from its text only.
            if key != "overhang_fan_threshold" {
                assert_eq!(c.float_at(canonical, tool), Some(want), "{key} tool {tool}");
            }
            // Orca-only options also keep their text per filament.
            if !mica_core::config::option_defs().contains_key(key) {
                let text = config.extra(key).unwrap_or_else(|| panic!("{key} text"));
                assert_eq!(text.split(',').nth(tool), Some(*value), "{key} text");
            }
        }
    }
    assert_eq!(config.bool("auxiliary_fan"), Some(true));
}
