// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Absolute extrusion (`use_relative_e_distances = 0`) is refused with one message on every
//! export path, before any slicing: Mica's writer only writes relative E values, and an
//! absolute program from them would extrude almost nothing (review of 2026-10-21: the single
//! object path wrote `M82` and per-move deltas; several stock Orca printers ship absolute E).
use mica_core::safety::ABSOLUTE_E_UNSUPPORTED;
use std::path::{Path, PathBuf};

fn model_path(name: &str) -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/models")
        .join(name)
        .to_string_lossy()
        .into_owned()
}

fn out_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("mica-abs-e-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Slices `models` with `extra` arguments; `None` when the profiles are not installed.
fn slice(name: &str, models: &[&str], extra: &[&str]) -> Option<Result<(), String>> {
    let out = out_dir(name).join("out.gcode");
    let mut args = vec!["slice".to_string()];
    args.extend(models.iter().map(|m| model_path(m)));
    args.extend(extra.iter().map(|s| s.to_string()));
    args.extend(["-o".into(), out.to_string_lossy().into_owned()]);
    if !mica_testkit::require_profiles_for(&args) {
        return None;
    }
    let result = mica_cli::run(&args);
    assert!(
        result.is_ok() || !out.exists(),
        "{name}: a refused slice left {}",
        out.display()
    );
    Some(result)
}

const ABSOLUTE: &[&str] = &["--set", "use_relative_e_distances=0"];

fn refused(name: &str, models: &[&str], extra: &[&str]) {
    let mut args = extra.to_vec();
    args.extend(ABSOLUTE);
    if let Some(result) = slice(name, models, &args) {
        assert_eq!(result.unwrap_err(), ABSOLUTE_E_UNSUPPORTED, "{name}");
    }
}

#[test]
fn single_object_and_plates_refuse_absolute_extrusion() {
    // CORE One: one object (the path that used to write M82 with relative values) and a
    // plate of two (the scheduler used to call it a "multi-tool export").
    refused("coreone-single", &["cube20.stl"], &[]);
    refused("coreone-plate", &["cube20.stl", "cube20.stl"], &[]);
    // XL: several tools.
    refused(
        "xl",
        &["cube20.stl"],
        &[
            "--printer",
            "Original Prusa XL - 5T Input Shaper 0.4 nozzle",
            "--print",
            "0.20mm SPEED @XLIS 0.4",
            "--filament",
            "Prusament PLA @XLIS",
        ],
    );
    // U1: Klipper.
    refused(
        "u1",
        &["cube20.stl"],
        &[
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
        ],
    );
    // Relative extrusion still slices.
    if let Some(result) = slice("coreone-relative", &["cube20.stl"], &[]) {
        result.unwrap();
    }
}

/// A stock Orca printer that ships absolute extrusion (Marlin 2) is refused without any
/// `--set`, in the same words.
#[test]
fn stock_absolute_extrusion_profile_is_refused() {
    if let Some(result) = slice(
        "artillery",
        &["cube20.stl"],
        &[
            "--vendor",
            "Artillery",
            "--printer",
            "Artillery Sidewinder X3 Pro 0.4 nozzle",
        ],
    ) {
        assert_eq!(result.unwrap_err(), ABSOLUTE_E_UNSUPPORTED);
    }
}
