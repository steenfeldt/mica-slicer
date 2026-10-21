// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/GCode.cpp :: GCodeGenerator::_extrude
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/CoolingBuffer.cpp :: CoolingBuffer::apply_layer_cooldown
// Changes: Synthetic paths verify role output, bridge speed/acceleration, fan restoration and cooling-off behavior.
use mica_core::{
    config::Config,
    flow::Flow,
    geometry::{BoundingBox, Point},
    print::{SliceReport, SlicedObject},
    settings::PrintSettings,
    toolpath::{ExtrusionPath, LayerToolpaths, Role},
};

#[test]
fn bridge_roles_speed_acceleration_and_fan_restore() {
    let mut config = Config::defaults();
    for (key, value) in [
        ("gcode_flavor", "marlin2"),
        ("use_relative_e_distances", "1"),
        ("start_gcode", ""),
        ("end_gcode", ""),
        ("layer_height", "0.2"),
        ("first_layer_height", "0.2"),
        ("extrusion_width", "0.45"),
        ("bridge_speed", "37"),
        ("bridge_acceleration", "1234"),
        ("default_acceleration", "1000"),
        ("bridge_fan_speed", "80"),
        ("min_fan_speed", "20"),
        ("max_fan_speed", "20"),
        ("disable_fan_first_layers", "1"),
        ("full_fan_speed_layer", "0"),
        ("fan_always_on", "1"),
        ("cooling", "0"),
        ("filament_max_volumetric_speed", "0"),
        ("max_volumetric_speed", "0"),
    ] {
        config.set_raw(key, value).unwrap();
    }
    let settings = PrintSettings::from_config(&config).unwrap();
    let paths = |index| {
        [
            Role::SolidInfill,
            Role::BridgeInfill,
            Role::InternalBridgeInfill,
            Role::SolidInfill,
        ]
        .into_iter()
        .enumerate()
        .map(|(i, role)| ExtrusionPath {
            role,
            points: vec![
                Point::from_mm(100., 100. + i as f64),
                Point::from_mm(120., 100. + i as f64),
            ],
            flow: if role == Role::InternalBridgeInfill {
                Flow::new(0.4, 0.4)
            } else {
                Flow::new(0.45, 0.2)
            },
            seam: None,
            overhang: None,
            perimeter_index: None,
            brick: None,
        })
        .collect::<Vec<_>>()
        .into_iter()
        .filter(|p| index > 0 || p.role == Role::SolidInfill)
        .collect()
    };
    let obj = SlicedObject {
        adhesion: Default::default(),
        outlines: Vec::new(),
        layers: (0..3)
            .map(|index| LayerToolpaths {
                support_groups: Vec::new(),
                index,
                print_z: (index + 1) as f64 * 0.2,
                height: 0.2,
                paths: paths(index),
            })
            .collect(),
        first_layer_bbox: Some(BoundingBox {
            min: Point::from_mm(100., 100.),
            max: Point::from_mm(120., 104.),
        }),
        report: SliceReport {
            layers: 3,
            dropped_chains: 0,
        },
    };
    let output = mica_gcode::export::export_gcode(&obj, &config, &settings, "bridges")
        .unwrap()
        .gcode;
    let layer = output.split(";LAYER_CHANGE").nth(3).unwrap();
    assert!(layer.contains(";TYPE:Bridge infill"));
    assert!(layer.contains("F2220"), "bridge feedrate missing");
    assert!(layer.contains("M204 P1234"), "bridge acceleration missing");
    let fan = layer
        .find("M106 S204")
        .expect("bridge fan works with automatic cooling disabled");
    let restore = layer[fan..].find("M106 S51").expect("normal fan restored");
    assert!(restore > 0);
    assert!(
        !output
            .split(";LAYER_CHANGE")
            .nth(1)
            .unwrap()
            .contains("M106 S204")
    );
}
