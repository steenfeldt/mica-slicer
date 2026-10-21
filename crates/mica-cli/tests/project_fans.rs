// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! A U1 project's per-filament fan values reach the G-code per tool, as OrcaSlicer's
//! CoolingBuffer writes them (`change_extruder_set_fan` at every layer start and toolchange).
use mica_core::threemf::{self, Flavour};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    process::Command,
};

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "mica-cli-project-fans-{}-{}",
            std::process::id(),
            N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The pinned Orca profiles with the Snapmaker U1 (a reported skip without them).
fn u1_available() -> bool {
    mica_testkit::require_orca_profiles()
}

/// plates-orca.3mf (plate 1: one object on each of the first two tools) with per-filament
/// fan values in its project settings.
fn project(dir: &Path, name: &str, extra: &[(&str, &str)]) -> PathBuf {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/models/plates-orca.3mf");
    let mut project = threemf::load_3mf(&source).unwrap();
    let config = project.config.as_mut().unwrap();
    let mut settings: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(&config.text).unwrap();
    let per_tool = |values: [&str; 4]| serde_json::json!(values);
    for (key, values) in [
        ("additional_cooling_fan_speed", ["30", "90", "0", "0"]),
        ("close_fan_the_first_x_layers", ["1", "3", "1", "1"]),
        ("fan_min_speed", ["20", "60", "20", "20"]),
        ("fan_max_speed", ["100", "60", "100", "100"]),
        ("fan_cooling_layer_time", ["100", "100", "100", "100"]),
        ("full_fan_speed_layer", ["0", "0", "0", "0"]),
    ] {
        settings.insert(key.into(), per_tool(values));
    }
    for (key, value) in extra {
        settings.insert((*key).into(), serde_json::json!(value));
    }
    config.text = serde_json::to_string(&settings).unwrap();
    let path = dir.join(format!("{name}.3mf"));
    threemf::write(&path, &project, Flavour::Orca).unwrap();
    path
}

fn slice(source: &Path, output: &Path) -> String {
    let result = Command::new(env!("CARGO_BIN_EXE_mica"))
        .env("MICA_ISOLATED", "1")
        .arg("slice")
        .arg(source)
        .args([
            "--vendor",
            "Snapmaker",
            "--printer",
            "Snapmaker U1 (0.4 nozzle)",
            "--bed",
            "High Temp Plate",
            "--plate",
            "1",
            "-o",
        ])
        .arg(output)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    std::fs::read_to_string(output).unwrap()
}

/// (layer, tool, part fan PWM, auxiliary fan PWM) at every extrusion.
fn fan_states(gcode: &str) -> BTreeSet<(usize, usize, u32, u32)> {
    let (mut layer, mut tool, mut fan, mut aux) = (0, 0, 0, 0);
    let mut out = BTreeSet::new();
    for line in gcode.lines() {
        if line.starts_with(";LAYER_CHANGE") {
            layer += 1;
        }
        let code = line.split(';').next().unwrap_or("").trim();
        let words: Vec<_> = code.split_whitespace().collect();
        match words.first() {
            Some(t) if t.len() > 1 && t.starts_with('T') && t[1..].parse::<usize>().is_ok() => {
                tool = t[1..].parse().unwrap();
            }
            Some(&"M106") => {
                let p = words.iter().find_map(|w| w.strip_prefix('P'));
                let s = words
                    .iter()
                    .find_map(|w| w.strip_prefix('S'))
                    .map_or(255, |s| s.parse::<f64>().unwrap() as u32);
                if p == Some("2") {
                    aux = s;
                } else {
                    fan = s;
                }
            }
            Some(&"M107") => {
                if words.contains(&"P2") {
                    aux = 0;
                } else {
                    fan = 0;
                }
            }
            Some(&"G1") | Some(&"G2") | Some(&"G3")
                if layer > 0
                    && words
                        .iter()
                        .any(|w| w.starts_with('X') || w.starts_with('Y'))
                    && words
                        .iter()
                        .any(|w| w.strip_prefix('E').is_some_and(|e| !e.starts_with('-'))) =>
            {
                out.insert((layer, tool, fan, aux));
            }
            _ => {}
        }
    }
    out
}

#[test]
fn per_filament_fan_values_reach_each_tool() {
    if !u1_available() {
        return;
    }
    let dir = Temp::new();
    let gcode = slice(&project(&dir.0, "fans", &[]), &dir.0.join("fans.gcode"));
    let states = fan_states(&gcode);
    let at = |layer: usize, tool: usize| -> BTreeSet<(u32, u32)> {
        states
            .iter()
            .filter(|s| s.0 == layer && s.1 == tool)
            .map(|s| (s.2, s.3))
            .collect()
    };
    assert!(!at(1, 0).is_empty() && !at(1, 1).is_empty(), "{states:?}");
    // Layer 1: both tools closed.
    assert_eq!(at(1, 0), [(0, 0)].into());
    assert_eq!(at(1, 1), [(0, 0)].into());
    // Layers 2 and 3: tool 1 keeps both fans off (close_fan_the_first_x_layers = 3), tool 0
    // has its auxiliary fan at 30% (additional_cooling_fan_speed).
    for layer in [2, 3] {
        assert!(at(layer, 0).iter().all(|&(_, aux)| aux == 76), "{states:?}");
        assert_eq!(at(layer, 1), [(0, 0)].into(), "layer {layer}");
    }
    // From layer 4 tool 1 runs at its own 60% (fan_min_speed = fan_max_speed) with its 90%
    // auxiliary fan; tool 0 interpolates by layer time between 20% and 100%.
    for layer in 4..=8 {
        assert_eq!(at(layer, 1), [(153, 229)].into(), "layer {layer}");
        assert!(
            at(layer, 0).iter().all(|&(fan, aux)| aux == 76 && fan > 51),
            "layer {layer}: {states:?}"
        );
    }
    // CoolingBuffer writes the new filament's fans right before the line that selects it.
    let lines: Vec<_> = gcode.lines().collect();
    let fourth = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| l.starts_with(";LAYER_CHANGE"))
        .nth(3)
        .unwrap()
        .0;
    let select = (fourth..lines.len()).find(|&i| lines[i] == "T1").unwrap();
    assert_eq!(
        lines[select - 2..=select],
        ["M106 S153", "M106 P2 S229", "T1"],
        "{:?}",
        &lines[select - 6..=select]
    );
}

/// Orca's CoolingBuffer times the layer's moves and dwells only: the tool change time of the
/// print time estimate does not shorten the layer fan's interpolation.
#[test]
fn tool_change_time_does_not_change_layer_fans() {
    if !u1_available() {
        return;
    }
    let dir = Temp::new();
    let fans = |extra: &[(&str, &str)], name: &str| {
        let gcode = slice(
            &project(&dir.0, name, extra),
            &dir.0.join(format!("{name}.gcode")),
        );
        gcode
            .lines()
            .filter(|l| l.starts_with("M106") || l.starts_with("M107"))
            .map(str::to_string)
            .collect::<Vec<_>>()
    };
    let short = fans(&[("machine_tool_change_time", "0")], "short");
    let long = fans(&[("machine_tool_change_time", "40")], "long");
    assert_eq!(short, long);
}
