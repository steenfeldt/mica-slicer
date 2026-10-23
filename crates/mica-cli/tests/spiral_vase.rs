// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Spiral vase through `mica slice`: on the CORE One (PrusaSlicer rules) and the U1 (Orca
//! rules) the hole block prints its solid bottom, then one wall whose Z rises within every
//! layer (the hole closes), a finishing loop at the top, and G-code the validator accepts; and
//! the prints spiral vase cannot make (supports, two objects at once) are refused.

use std::path::{Path, PathBuf};

const U1: &[&str] = &[
    "--vendor",
    "Snapmaker",
    "--printer",
    "Snapmaker U1 (0.4 nozzle)",
    "--print",
    "0.20 Standard @Snapmaker U1 (0.4 nozzle)",
    "--filament",
    "Snapmaker PLA @U1",
    "--bed",
    "High Temp Plate",
];

fn model(name: &str) -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/models")
        .join(name)
        .to_string_lossy()
        .into_owned()
}

/// Named, not the default printer: the default depends on the profiles found (CI differs).
const CORE_ONE: &[&str] = &[
    "--printer",
    "Prusa CORE One HF0.4 nozzle",
    "--print",
    "0.20mm SPEED @COREONE HF0.4",
    "--filament",
    "Prusament PLA @COREONE HF0.4",
];

fn out(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("mica-spiral-vase-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

fn slice(models: &[&str], profile: &[&str], extra: &[&str], name: &str) -> Result<String, String> {
    let path = out(name);
    let mut all = vec!["slice".to_string()];
    all.extend(models.iter().map(|m| model(m)));
    all.extend(profile.iter().map(|s| s.to_string()));
    all.extend(extra.iter().map(|s| s.to_string()));
    all.extend(["-o".into(), path.to_string_lossy().into_owned()]);
    mica_cli::run(&all)?;
    Ok(std::fs::read_to_string(&path).unwrap())
}

/// Per layer marker: the Z of each extrusion move, and whether the layer travels in XY.
fn layers(gcode: &str) -> Vec<(f64, Vec<f64>, bool)> {
    let mut out: Vec<(f64, Vec<f64>, bool)> = Vec::new();
    let mut z = 0.;
    for line in gcode.lines() {
        if let Some(v) = line.strip_prefix(";Z:") {
            out.push((v.parse().unwrap(), Vec::new(), false));
            continue;
        }
        if !line.starts_with("G1 ") {
            continue;
        }
        let word = |c: char| {
            line.split_whitespace()
                .find_map(|w| w.strip_prefix(c))
                .and_then(|v| v.parse::<f64>().ok())
        };
        if let Some(v) = word('Z') {
            z = v;
        }
        let Some(layer) = out.last_mut() else {
            continue;
        };
        match word('E') {
            Some(e) if e > 0. && (word('X').is_some() || word('Y').is_some()) => layer.1.push(z),
            None if word('X').is_some() || word('Y').is_some() => layer.2 = true,
            _ => {}
        }
    }
    out
}

fn check_spiral(gcode: &str, profile: &[&str], first_spiral: usize) {
    let layers = layers(gcode);
    // Solid bottom layers print flat.
    for (z, zs, _) in &layers[..first_spiral] {
        assert!(
            zs.iter().all(|v| (v - z).abs() < 1e-6),
            "layer {z} is not flat"
        );
    }
    // Every layer above rises from the layer below to its own Z, without a travel.
    let (spirals, finish) = layers[first_spiral..].split_at(layers.len() - first_spiral - 1);
    for (i, (z, zs, travels)) in spirals.iter().enumerate() {
        let below = layers[first_spiral + i - 1].0;
        assert!(!travels, "spiral layer {z} travels");
        assert!(zs.windows(2).all(|w| w[1] >= w[0]), "Z falls on layer {z}");
        assert!(
            zs[0] < below + 0.05 && zs[0] >= below,
            "layer {z} starts at {}",
            zs[0]
        );
        assert!(
            (zs.last().unwrap() - z).abs() < 1e-6,
            "layer {z} ends at its Z"
        );
    }
    // The finishing loop repeats the last layer's marker and stays at its height.
    let (z, zs, _) = &finish[0];
    assert_eq!(*z, spirals.last().unwrap().0);
    assert!(zs.iter().all(|v| (v - z).abs() < 1e-6));
    // The validator accepts it (it reads spiral vase from the file's configuration).
    let mut args = vec!["validate".to_string()];
    let path = out("validate.gcode");
    std::fs::write(&path, gcode).unwrap();
    args.push(path.to_string_lossy().into_owned());
    args.extend(profile.iter().map(|s| s.to_string()));
    mica_cli::run(&args).unwrap_or_else(|e| panic!("validation: {e}"));
}

#[test]
fn the_core_one_prints_one_rising_wall_after_the_solid_bottom() {
    if !mica_testkit::require_prusa_profiles() {
        return;
    }
    let gcode = slice(
        &["hole_block.stl"],
        CORE_ONE,
        &["--set", "spiral_vase=1", "--set", "support_material=0"],
        "core.gcode",
    )
    .unwrap();
    // Forced as PrusaSlicer's normalize_fdm does.
    assert!(gcode.contains("; perimeters = 1\n"));
    assert!(gcode.contains("; fill_density = 0%\n"));
    assert!(gcode.contains("; retract_layer_change = 0\n"));
    // Three bottom layers of 0.2 mm reach bottom_solid_min_thickness: the fourth spirals.
    check_spiral(&gcode, CORE_ONE, 3);
}

#[test]
fn the_u1_prints_one_rising_wall_with_orca_rules() {
    if !mica_testkit::require_orca_profiles() {
        return;
    }
    let gcode = slice(
        &["hole_block.stl"],
        U1,
        &["--set", "spiral_vase=1"],
        "u1.gcode",
    )
    .unwrap();
    check_spiral(&gcode, U1, 3);
    // Orca drops the retractions of a spiral layer.
    let spirals = gcode
        .split(";LAYER_CHANGE")
        .skip(4)
        .take(5)
        .collect::<String>();
    assert!(!spirals.contains("E-"), "a spiral layer retracts");
}

#[test]
fn supports_and_two_objects_at_once_are_refused() {
    if !mica_testkit::require_prusa_profiles() {
        return;
    }
    let error = slice(
        &["cube20.stl"],
        &[],
        &["--set", "spiral_vase=1", "--set", "support_material=1"],
        "supports.gcode",
    )
    .unwrap_err();
    assert!(error.contains("supports"), "{error}");
    let two = ["cube20.stl", "hole_block.stl"];
    let vase = ["--set", "spiral_vase=1", "--set", "support_material=0"];
    let error = slice(&two, &[], &vase, "two.gcode").unwrap_err();
    assert!(error.contains("one object at a time"), "{error}");
    // One after another they print.
    let mut sequential = vase.to_vec();
    sequential.extend(["--set", "complete_objects=1"]);
    slice(&two, &[], &sequential, "sequential.gcode").unwrap();
}
