// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 tests/fff_print/test_gcode.cpp :: Extrusion, travels, temperatures
// Source: prusaslicer@b028299c77 tests/fff_print/test_printgcode.cpp :: layer_num represents the layer's index from z=0
// Changes: Mica's job API and its own cubes; also the clearance moves and templates between objects, labels and the validator.

use mica_core::{
    config::Config,
    mesh::TriangleMesh,
    print::LayerSource,
    print_job::{PreparedPrint, PrintObject, ToolAssignments},
    settings::PrintSettings,
};
use mica_gcode::export::export_gcode;

fn config() -> Config {
    let mut c = Config::defaults();
    for (k, v) in [
        ("bed_shape", "0x0,250x0,250x220,0x220"),
        ("layer_height", "0.2"),
        ("first_layer_height", "0.2"),
        ("gcode_flavor", "marlin2"),
        ("use_relative_e_distances", "1"),
        ("perimeter_generator", "classic"),
        ("gcode_label_objects", "firmware"),
        ("complete_objects", "1"),
        ("extruder_clearance_radius", "20"),
        ("extruder_clearance_height", "20"),
        ("retract_length", "0.8"),
        ("retract_lift", "0.4"),
        (
            "between_objects_gcode",
            "; next object {current_object_idx}",
        ),
        ("before_layer_gcode", "; before {layer_num}"),
        ("layer_gcode", ";Layer:[layer_num] ([layer_z] mm)"),
        ("first_layer_temperature", "230"),
        ("temperature", "220"),
        ("first_layer_bed_temperature", "65"),
        ("bed_temperature", "60"),
    ] {
        c.set_raw(k, v).unwrap();
    }
    c
}

fn cube(c: &Config, id: usize, x: f32, height: f32) -> PrintObject {
    let mut mesh = TriangleMesh::load_stl(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/models/cube20.stl"
    ))
    .unwrap();
    for v in &mut mesh.vertices {
        v[2] *= height / 20.;
    }
    mesh.place_on_bed(x, 110.);
    PrintObject::prepare(
        format!("cube{id}"),
        id,
        0,
        mesh,
        ToolAssignments([Some(0), Some(0), Some(0), None, None]),
        c,
    )
    .unwrap()
}

fn axis(line: &str, key: char) -> Option<f64> {
    line.split(';')
        .next()?
        .split_whitespace()
        .find_map(|w| w.strip_prefix(key)?.parse().ok())
}

#[test]
fn objects_print_one_after_another_above_everything_printed() {
    let c = config();
    // Model order (object id) decides, not the placement: the taller cube prints first.
    let job = PreparedPrint::new(vec![cube(&c, 1, 150., 2.), cube(&c, 0, 60., 4.)], &c).unwrap();
    assert!(job.is_sequential());
    assert_eq!(job.layer_count(), 20 + 10);
    assert!(job.object_start(0).is_some_and(|s| s.ordinal == 0));
    let second = job.object_start(20).unwrap();
    assert_eq!(
        (second.ordinal, job.objects[second.object].object_id),
        (1, 1)
    );
    assert_eq!((job.previous_layer(20), job.next_layer(19)), (None, None));
    assert_eq!(job.previous_layer(21), Some(20));
    assert_eq!(job.max_print_z(), job.layer_z(19).0);
    assert_eq!(job.scheduled(19..21)[1].layer.index, 0);

    let g = export_gcode(&job, &c, &PrintSettings::from_config(&c).unwrap(), "seq")
        .unwrap()
        .gcode;
    let mut markers = Vec::new();
    let mut z = 0.;
    let mut top = 0f64;
    let mut cleared = false;
    let mut between = None;
    for (n, line) in g.lines().enumerate() {
        if let Some(v) = line.strip_prefix(";Z:") {
            let v: f64 = v.parse().unwrap();
            if markers.last().is_some_and(|&last| v < last) {
                assert!(cleared, "the next object starts before clearing the first");
                between = Some(n);
            }
            markers.push(v);
        }
        if let Some(v) = axis(line, 'Z') {
            z = v;
        }
        let xy = axis(line, 'X').is_some() || axis(line, 'Y').is_some();
        if line.starts_with("G1") && xy {
            if axis(line, 'E').is_some_and(|e| e > 0.) {
                top = top.max(z);
                cleared = false;
            } else if z >= top - 1e-6 && top > 1. {
                cleared = true;
            }
        }
    }
    assert_eq!(markers.len(), 30);
    let n = between.expect("Z restarts for the second object");
    let before: Vec<&str> = g.lines().take(n).collect();
    // The between-objects template runs with the next object's index, after the travel to
    // its centre (150, 110) and the first-layer temperatures.
    let at = before
        .iter()
        .rposition(|l| *l == "; next object 1")
        .unwrap();
    let travel = before
        .iter()
        .rposition(|l| l.starts_with("G1 X150 Y110"))
        .unwrap();
    assert!(travel < at);
    assert!(before[travel..at].contains(&"M104 S230 ; set temperature"));
    assert!(before[travel..at].contains(&"M140 S65 ; set bed temperature"));
    // No before-layer template on an object's first layer; layer numbers keep counting.
    assert!(!g.contains("; before 20"));
    assert!(g.contains("; before 21"));
    // layer_num counts every layer printed so far (test_printgcode.cpp): the second
    // object's last layer is the job's last.
    assert!(
        g.contains(";Layer:19 (4 mm)"),
        "{}",
        &g[g.find(";Layer:19").unwrap_or(0)..][..30]
    );
    assert!(g.contains(";Layer:29 (2 mm)"));
    // First-layer, other-layer, first-layer again, other-layer, off (test_gcode.cpp).
    let mut temps: Vec<f64> = Vec::new();
    for line in g.lines() {
        if (line.starts_with("M104") || line.starts_with("M109"))
            && let Some(t) = axis(line, 'S')
            && temps.last() != Some(&t)
        {
            temps.push(t);
        }
    }
    assert_eq!(temps, [230., 220., 230., 220., 0.]);
    // The label stays on during an object and changes only between objects.
    assert_eq!(g.matches("M486 S-1").count(), 2 + 2);
    let report = mica_gcode::validate::validate_text(
        g.as_bytes(),
        &mica_gcode::validate::Profile::from_config(&c).unwrap(),
    );
    assert_eq!(report.errors, 0, "{:?}", report.diagnostics);
}

#[test]
fn layer_by_layer_jobs_are_unchanged_and_clearance_is_checked() {
    let mut c = config();
    c.set_raw("complete_objects", "0").unwrap();
    let job = PreparedPrint::new(vec![cube(&c, 0, 60., 4.), cube(&c, 1, 150., 2.)], &c).unwrap();
    assert!(!job.is_sequential());
    assert_eq!(job.layer_count(), 20);
    assert_eq!(job.previous_layer(10), Some(9));
    // A generic (unlisted) printer: a 20 mm clearance cylinder; 25 mm apart is too close.
    let c = config();
    let job = PreparedPrint::new(vec![cube(&c, 0, 60., 4.), cube(&c, 1, 85., 2.)], &c).unwrap();
    assert_eq!(
        job.warnings,
        ["Extruder will crash into cube0 while printing cube1."]
    );
}
