// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Object labels under PrusaSlicer rules (LabelObjects with one extruder): the label opens in
//! the travel to the object's first extrusion and stays open across layers.
use mica_core::{
    config::Config, mesh::TriangleMesh, print::prepare_object_from_source, settings::PrintSettings,
};
use mica_gcode::export::export_gcode;

fn cube(set: &[(&str, &str)]) -> String {
    let mut config = Config::defaults();
    for line in
        include_str!("../../mica-profiles/tests/fixtures/coreone_hf04_speed_pla.ini").lines()
    {
        if let Some((key, value)) = line.split_once(" = ") {
            config.set_raw(key, value).unwrap();
        }
    }
    config.set_raw("thumbnails", "").unwrap();
    for (key, value) in set {
        config.set_raw(key, value).unwrap();
    }
    let settings = PrintSettings::from_config(&config).unwrap();
    let loaded = TriangleMesh::load_stl(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/models/cube20.stl"
    ))
    .unwrap();
    let mut mesh = loaded.clone();
    mesh.place_on_bed(125., 110.);
    export_gcode(
        &prepare_object_from_source(&mesh, &loaded, &settings).unwrap(),
        &config,
        &settings,
        "cube20.stl",
    )
    .unwrap()
    .gcode
}

/// The label lines after the header (the first layer change).
fn labels<'a>(g: &'a str, prefix: &str) -> Vec<(usize, &'a str)> {
    let start = g.find(";LAYER_CHANGE").unwrap();
    g[start..]
        .lines()
        .enumerate()
        .filter(|(_, l)| l.starts_with(prefix))
        .collect()
}

#[test]
fn single_object_label_opens_once_in_the_first_travel() {
    let g = cube(&[("gcode_label_objects", "firmware")]);
    assert!(g.contains("\nM486 S0\nM486 Acube20.stl\nM486 S-1\n"));
    let body = &g[g.find(";LAYER_CHANGE").unwrap()..];
    let found = labels(&g, "M486");
    assert_eq!(
        found.iter().map(|(_, l)| *l).collect::<Vec<_>>(),
        ["M486 S0", "M486 S-1"],
        "PrusaSlicer keeps the label open across layers"
    );
    let lines: Vec<_> = body.lines().collect();
    // travel_to_first_position: retract, lift, label, XY, Z.
    let at = found[0].0;
    assert!(
        lines[at - 1].starts_with("G1 Z"),
        "{:?}",
        &lines[at - 3..at + 3]
    );
    assert!(
        lines[at + 1].starts_with("G1 X"),
        "{:?}",
        &lines[at - 3..at + 3]
    );
    // Closed after the last layer, before the end G-code's retraction.
    let end = found[1].0;
    assert!(
        lines[end + 1].starts_with("G1 E-"),
        "{:?}",
        &lines[end - 2..end + 3]
    );
    assert!(!body[..body.find("M486 S-1").unwrap()].contains("TYPE:Custom"));
}

#[test]
fn skirt_layers_close_the_label_before_the_skirt() {
    let g = cube(&[
        ("gcode_label_objects", "firmware"),
        ("skirts", "1"),
        ("skirt_height", "2"),
        ("skirt_distance", "3"),
        ("min_skirt_length", "0"),
    ]);
    let found: Vec<_> = labels(&g, "M486").into_iter().map(|(_, l)| l).collect();
    assert_eq!(found, ["M486 S0", "M486 S-1", "M486 S0", "M486 S-1"]);
    let mut open = false;
    for line in g[g.find(";LAYER_CHANGE").unwrap()..].lines() {
        match line {
            "M486 S0" => open = true,
            "M486 S-1" => open = false,
            ";TYPE:Skirt/Brim" => assert!(!open, "a skirt printed inside the object label"),
            _ => {}
        }
    }
}

#[test]
fn octoprint_labels_name_object_and_copy() {
    let g = cube(&[("gcode_label_objects", "octoprint")]);
    let start = labels(&g, "; printing object");
    let stop = labels(&g, "; stop printing object");
    assert_eq!(start.len(), 1);
    assert_eq!(stop.len(), 1);
    assert_eq!(start[0].1, "; printing object cube20.stl id:0 copy 0");
    assert!(g.contains(
        "; printing object cube20.stl id:0 copy 0\n; stop printing object cube20.stl id:0 copy 0\n"
    ));
}
