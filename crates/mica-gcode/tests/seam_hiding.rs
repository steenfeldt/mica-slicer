// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/GCode.cpp :: GCodeGenerator::extrude_perimeters, GCodeGenerator::travel_to
// Changes: Regression for a CORE One cube test print: Arachne external loops must move inwards
// Changes: before retracting, and short hops onto external perimeters use travel_short_distance_acceleration.
use mica_core::{
    config::Config, mesh::TriangleMesh, print::prepare_object, settings::PrintSettings,
};
use mica_gcode::export::export_gcode;

fn core_one_cube() -> String {
    let mut c = Config::defaults();
    for l in include_str!("../../mica-profiles/tests/fixtures/coreone_hf04_speed_pla.ini").lines() {
        if let Some((k, v)) = l.split_once(" = ") {
            c.set_raw(k, v).unwrap();
        }
    }
    c.set_raw("thumbnails", "").unwrap();
    let settings = PrintSettings::from_config(&c).unwrap();
    let mut mesh = TriangleMesh::load_stl(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/models/cube20.stl"
    ))
    .unwrap();
    mesh.place_on_bed(125., 110.);
    export_gcode(
        &prepare_object(&mesh, &settings).unwrap(),
        &c,
        &settings,
        "cube",
    )
    .unwrap()
    .gcode
}

#[test]
fn arachne_external_loops_move_inwards_before_retracting() {
    let gcode = core_one_cube();
    let lines: Vec<&str> = gcode.lines().collect();
    let mut checked = 0;
    for (i, line) in lines.iter().enumerate() {
        if *line != ";TYPE:External perimeter" {
            continue;
        }
        // The first non-extruding G1 after the loop is the inward move, before any retraction.
        let after = lines[i + 1..]
            .iter()
            .filter(|l| l.starts_with("G1 ") && (l.contains('X') || l.contains("E-")))
            .find(|l| !l.contains(" E") || l.contains("E-"))
            .unwrap();
        assert!(
            !after.contains("E-"),
            "retracted on the outer surface: {after}"
        );
        checked += 1;
    }
    assert_eq!(checked, 100);
}

#[test]
fn short_hops_onto_external_perimeters_use_gentle_travel_acceleration() {
    let gcode = core_one_cube();
    // CORE One: travel_short_distance_acceleration = 250, travel_acceleration = 7000; once per layer on a cube.
    assert_eq!(gcode.matches("\nM204 T250\n").count(), 100);
    assert_eq!(gcode.matches("\nM204 T7000\n").count(), 100);
}
