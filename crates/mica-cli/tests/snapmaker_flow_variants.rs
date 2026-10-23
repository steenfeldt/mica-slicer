// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! A project saved by Snapmaker Orca stores its flow-variant process options as lists (one
//! value per flow type: `"enable_overhang_speed": ["1", "1"]`), where OrcaSlicer 2.4.2 has
//! single values. Such a project opens and slices like the same project with single values:
//! a uniform list is its value, and filaments of different flow types read the standard value
//! with a note.
use mica_core::threemf::{self, Flavour};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    process::Command,
};

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "mica-cli-flow-variants-{}-{}",
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

/// plates-orca.3mf with `values` in its project settings, written to `dir/project.3mf`.
fn project(dir: &Path, values: &[(&str, Value)]) -> PathBuf {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/models/plates-orca.3mf");
    let mut project = threemf::load_3mf(&source).unwrap();
    let config = project.config.as_mut().unwrap();
    let mut settings: serde_json::Map<String, Value> = serde_json::from_str(&config.text).unwrap();
    for (key, value) in values {
        settings.insert((*key).into(), value.clone());
    }
    config.text = serde_json::to_string(&settings).unwrap();
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join("project.3mf");
    threemf::write(&path, &project, Flavour::Orca).unwrap();
    path
}

/// The G-code and the notes of slicing `values`' project on the U1.
fn slice(dir: &Path, values: &[(&str, Value)]) -> (String, String) {
    let source = project(dir, values);
    let output = dir.join("project.gcode");
    let result = Command::new(env!("CARGO_BIN_EXE_mica"))
        .env("MICA_ISOLATED", "1")
        .env("SOURCE_DATE_EPOCH", "0")
        .arg("slice")
        .arg(&source)
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
        .arg(&output)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&result.stderr).into_owned();
    assert!(result.status.success(), "{stderr}");
    (std::fs::read_to_string(output).unwrap(), stderr)
}

const SINGLE: [(&str, &str); 5] = [
    ("enable_overhang_speed", "0"),
    ("slowdown_for_curled_perimeters", "1"),
    ("outer_wall_speed", "123"),
    ("slow_down_layers", "3"),
    ("overhang_2_4_speed", "40%"),
];

#[test]
fn snapmaker_orca_lists_slice_like_single_values() {
    if !mica_testkit::require_orca_profiles() {
        return;
    }
    let single: Vec<_> = SINGLE.iter().map(|(k, v)| (*k, json!(v))).collect();
    let lists: Vec<_> = SINGLE.iter().map(|(k, v)| (*k, json!([v, v]))).collect();
    let tmp = Temp::new();
    let (want, notes) = slice(&tmp.0.join("single"), &single);
    assert!(!notes.contains("rejected"), "{notes}");
    // The values are the project's, not the preset's.
    assert!(want.contains("F7380"), "outer wall speed 123 mm/s");
    let (got, notes) = slice(&tmp.0.join("lists"), &lists);
    assert!(
        !notes.contains("Snapmaker Orca") && !notes.contains("rejected"),
        "{notes}"
    );
    assert!(got == want, "a uniform list slices like its value");
}

#[test]
fn filaments_of_another_flow_type_read_their_variant() {
    if !mica_testkit::require_orca_profiles() {
        return;
    }
    let tmp = Temp::new();
    let flows = |types: [&str; 4]| {
        vec![
            ("process_flow_support", json!(["standard", "high_flow"])),
            ("filament_volume_type", json!(types)),
            ("outer_wall_speed", json!(["123", "234"])),
        ]
    };
    let speed = |v: &str| vec![("outer_wall_speed", json!(v))];
    // Every filament high flow: Snapmaker Orca's high-flow value.
    let (high, notes) = slice(&tmp.0.join("high"), &flows(["high_flow"; 4]));
    assert!(
        !notes.contains("Snapmaker Orca") && !notes.contains("rejected"),
        "{notes}"
    );
    assert!(high == slice(&tmp.0.join("234"), &speed("234")).0);
    // Mixed flow types: one value for all filaments, the standard one, and a note.
    let (mixed, notes) = slice(
        &tmp.0.join("mixed"),
        &flows(["standard", "high_flow", "standard", "standard"]),
    );
    assert!(
        notes.contains("3MF note: outer_wall_speed: Snapmaker Orca uses a different value")
            && notes.contains("uses the standard one, 123"),
        "{notes}"
    );
    assert!(mixed == slice(&tmp.0.join("123"), &speed("123")).0);
}
