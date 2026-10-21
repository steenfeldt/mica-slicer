// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Strict command comparison uses the same dependency-free script as the live oracle runs.
use mica_core::{
    config::Config, mesh::TriangleMesh, print::prepare_object_from_source, settings::PrintSettings,
};
use mica_gcode::export::export_gcode;
use std::{
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

fn cube(generator: &str) -> String {
    let mut config = Config::defaults();
    for line in
        include_str!("../../mica-profiles/tests/fixtures/coreone_hf04_speed_pla.ini").lines()
    {
        if let Some((key, value)) = line.split_once(" = ") {
            config.set_raw(key, value).unwrap();
        }
    }
    config.set_raw("thumbnails", "").unwrap();
    config.set_raw("perimeter_generator", generator).unwrap();
    let settings = PrintSettings::from_config(&config).unwrap();
    let loaded = TriangleMesh::load_stl(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/models/cube20.stl"
    ))
    .unwrap();
    let mut mesh = loaded.clone();
    mesh.place_on_bed(125., 110.);
    // As the CLI: the loaded STL is sliced in PrusaSlicer's object frame.
    export_gcode(
        &prepare_object_from_source(&mesh, &loaded, &settings).unwrap(),
        &config,
        &settings,
        "cube20",
    )
    .unwrap()
    .gcode
}

struct Temporary(PathBuf);
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn compare(gcode: &str, layers: &[usize]) {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let path = Temporary(std::env::temp_dir().join(format!(
        "mica-exact-{}-{}.gcode",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )));
    std::fs::write(&path.0, gcode).unwrap();
    // Windows installs Python 3 as `python` (no `python3`).
    let mut command = Command::new(if cfg!(windows) { "python" } else { "python3" });
    command
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../scripts/gcode-exact-diff.py"
        ))
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/coreone_cube20_layer_moves.gcode"
        ))
        .arg(&path.0);
    for layer in layers {
        command.arg("--only-layer").arg(layer.to_string());
    }
    let result = command
        .output()
        .expect("python3 is required for the exact G-code oracle");
    assert!(
        result.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn cube_bottom_and_top_layers_match_prusaslicer_exactly() {
    compare(&cube("arachne"), &[0, 1, 2, 96, 97, 98, 99]);
}

#[test]
#[ignore = "Full acceptance oracle: remaining grid anchors, solid-ring ordering and cooling differences are documented in fixtures/README.md"]
fn cube_all_layers_match_prusaslicer_exactly() {
    compare(&cube("arachne"), &[]);
}

#[test]
fn normalized_oracle_round_trips_without_losing_commands() {
    compare(
        include_str!("fixtures/coreone_cube20_layer_moves.gcode"),
        &[],
    );
}
