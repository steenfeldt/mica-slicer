// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! hole_block's XY travel stays close to PrusaSlicer 2.9.6's (CORE One and XL, default CLI
//! placement), counted from the first layer change. Only the reference numbers are stored,
//! measured the same way on the unmodified reference app's output: CORE One 2398.6 mm, XL
//! 2571.9 mm. Before PrusaSlicer's Clipper 6.4 gap closing was ported for slicing, a unit of
//! geometry noise around the hole flipped the grid infill's corner connections on 24 of 50 layers
//! and Mica travelled 10.4 % / 10.8 % more (now about 2.6 %).

use std::path::PathBuf;

fn model(name: &str) -> String {
    format!("{}/../../tests/models/{name}", env!("CARGO_MANIFEST_DIR"))
}

/// Sum of XY travel (moves without positive extrusion) from the first layer change on, in mm.
fn travel(gcode: &str) -> f64 {
    let (mut x, mut y): (Option<f64>, Option<f64>) = (None, None);
    let mut total = 0.;
    let mut layers = false;
    for line in gcode.lines() {
        layers |= line.starts_with(";LAYER_CHANGE");
        let code = line.split(';').next().unwrap_or("").trim();
        let mut words = code.split_whitespace();
        let Some(cmd) = words.next() else { continue };
        if !matches!(cmd, "G0" | "G1" | "G2" | "G3") {
            continue;
        }
        let (mut nx, mut ny, mut e) = (x, y, 0.);
        let mut moves = false;
        for w in words {
            let (k, v) = w.split_at(1);
            let Ok(v) = v.parse::<f64>() else { continue };
            match k {
                "X" => (nx, moves) = (Some(v), true),
                "Y" => (ny, moves) = (Some(v), true),
                "E" => e = v,
                _ => {}
            }
        }
        if layers
            && matches!(cmd, "G0" | "G1")
            && moves
            && e <= 0.
            && let (Some(ax), Some(ay), Some(bx), Some(by)) = (x, y, nx, ny)
        {
            total += (bx - ax).hypot(by - ay);
        }
        (x, y) = (nx, ny);
    }
    total
}

fn slice(profile: &[&str], tag: &str) -> Option<String> {
    let path: PathBuf = std::env::temp_dir().join(format!(
        "mica-hole-travel-{}-{tag}.gcode",
        std::process::id()
    ));
    let mut args: Vec<String> = vec!["slice".into(), model("hole_block.stl")];
    args.extend(profile.iter().map(|s| s.to_string()));
    args.extend(["-o".into(), path.to_string_lossy().into_owned()]);
    if !mica_testkit::require_profiles_for(&args) {
        return None;
    }
    match mica_cli::run(&args) {
        Ok(()) => {}
        Err(e) => panic!("mica {args:?}: {e}"),
    }
    let text = std::fs::read_to_string(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    Some(text)
}

fn check(profile: &[&str], tag: &str, reference: f64) {
    let Some(gcode) = slice(profile, tag) else {
        return;
    };
    let mica = travel(&gcode);
    let error = mica / reference - 1.;
    assert!(
        error.abs() < 0.04,
        "{tag}: travel {mica:.1} mm vs PrusaSlicer {reference} mm ({:+.2} %)",
        error * 100.
    );
}

#[test]
fn core_one_travel_is_close_to_prusaslicer() {
    check(&[], "core", 2398.6);
}

#[test]
fn xl_travel_is_close_to_prusaslicer() {
    check(
        &[
            "--printer",
            "Original Prusa XL - 5T Input Shaper 0.4 nozzle",
            "--print",
            "0.20mm SPEED @XLIS 0.4",
            "--filament",
            "Prusament PLA @XLIS",
        ],
        "xl",
        2571.9,
    );
}
