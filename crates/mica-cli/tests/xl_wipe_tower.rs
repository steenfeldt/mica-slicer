// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The XL's wipe tower on the corpus painted cube (`--set wipe_tower=1`), checked against
//! PrusaSlicer 2.9.6's G-code for the same project and presets: the first layer has T1 alone,
//! so its tower is a finish_layer visited first, by the print's first travel.

fn model(name: &str) -> String {
    format!("{}/../../tests/models/{name}", env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn painted_cube_first_layer_tower_starts_like_prusaslicer() {
    let out = std::env::temp_dir().join(format!("mica-xl-tower-{}.gcode", std::process::id()));
    let all: Vec<String> = [
        "slice",
        &model("painted-cube.3mf"),
        "--printer",
        "Original Prusa XL - 5T Input Shaper 0.4 nozzle",
        "--print",
        "0.20mm SPEED @XLIS 0.4",
        "--filament",
        "Prusament PLA @XLIS",
        "--set",
        "wipe_tower=1",
        "-o",
        &out.to_string_lossy(),
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    if !mica_testkit::require_profiles_for(&all) {
        return;
    }
    match mica_cli::run(&all) {
        Ok(()) => {}
        Err(e) => panic!("mica {all:?}: {e}"),
    }
    let g = std::fs::read_to_string(&out).unwrap();
    let _ = std::fs::remove_file(&out);
    let layer: Vec<&str> = g
        .lines()
        .skip_while(|l| *l != ";LAYER_CHANGE")
        .skip(1)
        .take_while(|l| *l != ";LAYER_CHANGE")
        .collect();
    let tower = layer
        .iter()
        .position(|l| *l == ";TYPE:Wipe tower")
        .expect("first layer tower");
    let moves: Vec<&str> = layer[..tower]
        .iter()
        .copied()
        .filter(|l| l.starts_with("G1 Z") || l.starts_with("G1 X"))
        .collect();
    // travel_to_first_position: lift by the ramping lift's maximum (filament 0.6 mm), across
    // to the tower's start, down. PrusaSlicer: G1 Z.8, G1 X239.5 Y140.5, G1 Z.2.
    assert_eq!(moves.first().copied(), Some("G1 Z.8 F720"), "{moves:#?}");
    assert!(
        moves.iter().any(|l| l.starts_with("G1 X239.5 Y140.5")),
        "{moves:#?}"
    );
    // finish_layer's sparse rectangle starts in the corner save_on_last_wipe's m_left_to_right
    // gives: PrusaSlicer extrudes up the right edge first (G1 X239.5 Y143.75).
    let first_extrusion = layer[tower..]
        .iter()
        .find(|l| l.starts_with("G1 X") && l.contains(" E"))
        .unwrap();
    assert!(
        first_extrusion.starts_with("G1 X239.5 Y143.75 "),
        "{first_extrusion}"
    );
}
