// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

use mica_core::{
    config::Config,
    flow::Flow,
    geometry::{BoundingBox, Point},
    print::{SliceReport, SlicedObject},
    settings::PrintSettings,
    toolpath::{ExtrusionPath, LayerToolpaths, Role},
};

fn line(role: Role, a: f64, b: f64) -> ExtrusionPath {
    ExtrusionPath {
        role,
        points: vec![Point::from_mm(a, 100.), Point::from_mm(b, 100.)],
        flow: Flow::new(0.42, 0.2),
        perimeter_index: None,
        overhang: None,
        seam: None,
        brick: None,
    }
}

fn export(rules: &str, style: &str) -> String {
    let mut config = Config::defaults();
    for (key, value) in [
        ("gcode_flavor", "marlin2"),
        ("mica_slicing_rules", rules),
        ("support_material_style", style),
        ("start_gcode", ""),
        ("end_gcode", ""),
        ("cooling", "0"),
        ("wipe", "0"),
        ("retract_length", "0"),
        ("retract_lift", "0"),
        ("use_relative_e_distances", "1"),
    ] {
        config.set_raw(key, value).unwrap();
    }
    let settings = PrintSettings::from_config(&config).unwrap();
    let obj = SlicedObject {
        adhesion: Default::default(),
        outlines: Vec::new(),
        layers: vec![LayerToolpaths {
            index: 0,
            print_z: 0.2,
            height: 0.2,
            paths: vec![
                line(Role::Perimeter, 100., 101.),
                line(Role::SupportMaterial, 150., 151.),
                line(Role::SupportMaterial, 102., 103.),
                line(Role::SupportMaterialInterface, 105., 104.),
            ],
            ..Default::default()
        }],
        first_layer_bbox: Some(BoundingBox {
            min: Point::from_mm(100., 100.),
            max: Point::from_mm(151., 100.),
        }),
        report: SliceReport {
            layers: 1,
            dropped_chains: 0,
        },
    };
    mica_gcode::export::export_gcode(&obj, &config, &settings, "support order")
        .unwrap()
        .gcode
}

#[test]
fn emitted_normal_and_organic_support_tours_follow_the_selected_rules() {
    for (rules, style) in [
        ("prusaslicer", "grid"),
        ("prusaslicer", "snug"),
        ("prusaslicer", "organic"),
        ("orcaslicer", "grid"),
        ("orcaslicer", "organic"),
    ] {
        let output = export(rules, style);
        let mut x = 0.;
        let mut endpoints = Vec::new();
        let mut travel = 0.;
        let mut printed = false;
        for line in output.split(";LAYER_CHANGE").nth(1).unwrap().lines() {
            if !line.starts_with("G1 ") {
                continue;
            }
            let value = |key| {
                line.split(';')
                    .next()
                    .unwrap()
                    .split_whitespace()
                    .find_map(|t| t.strip_prefix(key).and_then(|v| v.parse::<f64>().ok()))
            };
            let nx = value("X").unwrap_or(x);
            let e = value("E").unwrap_or(0.);
            if e > 0. && nx != x {
                endpoints.push(nx);
                printed = true;
            } else if printed && endpoints.len() < 4 {
                travel += (nx - x).abs();
            }
            x = nx;
        }
        let normal_prusa = rules == "prusaslicer" && style != "organic";
        let (expected, expected_travel) = if normal_prusa {
            // Pinned get_support_extrusions passes no previous-position seed.
            ([101., 150., 104., 102.], 96.)
        } else {
            ([101., 103., 105., 151.], 47.)
        };
        assert_eq!(endpoints, expected, "{rules}/{style}");
        assert!(
            (travel - expected_travel).abs() < 0.001,
            "{rules}/{style}: {travel} mm"
        );
    }
}
