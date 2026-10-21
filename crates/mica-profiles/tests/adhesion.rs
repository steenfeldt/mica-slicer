// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 tests/fff_print/test_skirt_brim.cpp :: Skirt height is honored, Original Slic3r Skirt/Brim tests
// Source: orca@8500fcdcca src/libslic3r/Brim.cpp :: make_brim, make_brim_ears_auto, configBrimWidthByVolumeGroups
// Changes: Profile-to-G-code integration and numerical U1 2.4.1 oracle expectations; no bundled profiles.

use mica_core::{
    brim::BrimType,
    config::Config,
    geometry::unscale,
    mesh::TriangleMesh,
    print::{LayerSource, prepare_object},
    settings::PrintSettings,
    toolpath::Role,
};
use mica_profiles::orca::{Bundle, Dialect, map_config};

fn cube() -> TriangleMesh {
    let mut mesh = TriangleMesh::load_stl(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/models/cube20.stl"
    ))
    .unwrap();
    mesh.place_on_bed(135.5, 136.);
    mesh
}

#[test]
fn all_brim_modes_survive_mapping_including_painted_ears() {
    for (mode, expected) in [
        ("no_brim", BrimType::None),
        ("outer_only", BrimType::Outer),
        ("inner_only", BrimType::Inner),
        ("outer_and_inner", BrimType::OuterAndInner),
        ("auto_brim", BrimType::Auto),
        ("brim_ears", BrimType::Ears),
        ("painted", BrimType::Painted),
    ] {
        let raw = serde_json::from_value(serde_json::json!({
            "brim_type": mode, "brim_object_gap":"0.2", "initial_layer_print_height":"0.25",
            "skirt_loops":"3", "skirt_type":"perobject", "skirt_speed":"37", "skirt_start_angle":"90",
            "brim_ears_max_angle":"91", "brim_ears_detection_length":"0.5"
        })).unwrap();
        let mapped = map_config(&raw, Dialect::Orca242);
        assert_eq!(mapped.config.string("brim_type"), Some(mode));
        let result = PrintSettings::from_config(&mapped.config);
        {
            let kind = expected;
            let settings = result.unwrap();
            let s = settings.adhesion;
            assert_eq!(s.brim.kind, kind);
            assert_eq!(s.brim.separation, 0.2);
            assert_eq!(s.skirts, 3);
            assert!(s.per_object);
            assert!(s.orca);
            assert_eq!(s.skirt_speed, 37.);
            assert_eq!(s.skirt_start_angle, 90.);
            assert_eq!(s.brim.ears_max_angle, 91.);
            assert_eq!(s.brim.ears_detection_length, 0.5);
        }
    }
}

#[test]
fn upstream_skirt_height_speed_and_first_layer_emission() {
    let mut config = Config::defaults();
    for (key, value) in [
        ("skirts", "1"),
        ("skirt_height", "5"),
        ("brim_width", "5"),
        ("perimeters", "0"),
        ("support_material_speed", "99"),
        ("cooling", "0"),
        ("first_layer_speed", "100%"),
        ("use_relative_e_distances", "1"),
        ("gcode_flavor", "marlin2"),
        (
            "start_gcode",
            "; FOOTPRINT {first_layer_print_min[0]},{first_layer_print_max[0]}",
        ),
    ] {
        config.set_raw(key, value).unwrap();
    }
    let settings = PrintSettings::from_config(&config).unwrap();
    let obj = prepare_object(&cube(), &settings).unwrap();
    let bbox = obj.first_layer_bbox().unwrap();
    let gcode = mica_gcode::export::export_gcode(&obj, &config, &settings, "cube20")
        .unwrap()
        .gcode;
    let bounds: Vec<f64> = gcode
        .lines()
        .find_map(|l| l.strip_prefix("; FOOTPRINT "))
        .unwrap()
        .split(',')
        .map(|v| v.parse().unwrap())
        .collect();
    assert!((bounds[0] - unscale(bbox.min.x)).abs() < 0.001);
    assert!((bounds[1] - unscale(bbox.max.x)).abs() < 0.001);
    let mut role = "";
    let mut layers = 0;
    let mut adhesion_layers = std::collections::BTreeSet::new();
    let mut first_type = None;
    for line in gcode.lines() {
        if line == ";LAYER_CHANGE" {
            layers += 1;
        }
        if let Some(value) = line.strip_prefix(";TYPE:") {
            role = value;
            if layers == 1 && first_type.is_none() {
                first_type = Some(value);
            }
        }
        if layers > 0
            && role == "Skirt/Brim"
            && line.starts_with("G1 ")
            && line.contains(" E")
            && line.contains(" X")
        {
            adhesion_layers.insert(layers);
        }
    }
    assert_eq!(first_type, Some("Skirt/Brim"));
    assert_eq!(adhesion_layers, (1..=5).collect());
    assert!(gcode.contains("F5940"));
    assert_eq!(
        obj.toolpaths(1..2)[0].paths.first().unwrap().role,
        Role::Skirt
    );
    assert!(
        obj.toolpaths(1..2)[0]
            .paths
            .iter()
            .all(|p| p.role != Role::Brim)
    );
}

#[test]
fn u1_default_ears_and_tall_auto_match_reference_bounds() {
    if !mica_testkit::require_orca_profiles() {
        return;
    }
    let root = mica_testkit::orca_profiles();
    let mut b =
        Bundle::load_vendor(root.join("OrcaFilamentLibrary.json"), Dialect::Orca242).unwrap();
    b.add_vendor(root.join("Snapmaker.json")).unwrap();
    for (mode, tall, bounds, filament) in [
        ("auto_brim", false, [123.5, 124., 147.5, 148.], 0.),
        (
            "brim_ears",
            false,
            [119.498, 119.998, 151.502, 152.002],
            13.64198,
        ),
        (
            "auto_brim",
            true,
            [125.258, 122.758, 145.742, 149.242],
            31.22806,
        ),
    ] {
        let mut c = b
            .single_tool_config(
                "Snapmaker U1 (0.4 nozzle)",
                "0.20 Standard @Snapmaker U1 (0.4 nozzle)",
                "Snapmaker PLA @U1",
                Some("High Temp Plate"),
            )
            .unwrap();
        c.set_raw("brim_type", mode).unwrap();
        let s = PrintSettings::from_config(&c).unwrap();
        let mut mesh = cube();
        if tall {
            for v in &mut mesh.vertices {
                v[0] *= 0.1;
                v[1] *= 0.4;
                v[2] *= 5.;
            }
            mesh.place_on_bed(135.5, 136.);
        }
        let obj = prepare_object(&mesh, &s).unwrap();
        let bb = obj.first_layer_bbox().unwrap();
        let actual = [
            unscale(bb.min.x),
            unscale(bb.min.y),
            unscale(bb.max.x),
            unscale(bb.max.y),
        ];
        for (a, b) in actual.iter().zip(bounds) {
            assert!((a - b).abs() < 0.001, "{mode}: {actual:?}");
        }
        let gcode = mica_gcode::export::export_gcode_with_mesh(&obj, &c, &s, "cube20.stl", &mesh)
            .unwrap()
            .gcode;
        assert!(gcode.contains(&format!(
            "BED_MESH_CALIBRATE mesh_min={},{} mesh_max={},{}",
            bounds[0], bounds[1], bounds[2], bounds[3]
        )));
        let mut role = "";
        let mut e = 0.0;
        for line in gcode.lines() {
            if let Some(value) = line.strip_prefix(";TYPE:") {
                role = value;
            }
            if role == "Brim"
                && line.starts_with("G1 ")
                && (line.contains(" X") || line.contains(" Y"))
            {
                for word in line.split_whitespace() {
                    if let Some(value) = word.strip_prefix('E').and_then(|s| s.parse::<f64>().ok())
                    {
                        e += value.max(0.);
                    }
                }
            }
        }
        // Arc fitting may move brim extrusion to G2/G3; compare the complete core path volume too.
        let volume: f64 = obj
            .adhesion
            .brim
            .iter()
            .map(|p| {
                mica_core::geometry::polyline_length(&p.points) / mica_core::geometry::SCALE
                    * p.flow.mm3_per_mm()
            })
            .sum();
        let total_e = volume * s.extrusion_multiplier
            / (std::f64::consts::PI * (s.filament_diameter / 2.).powi(2));
        assert!(
            (total_e - filament).abs() < 0.02 * filament + 0.01,
            "{mode}: {total_e} vs {filament}, linear G-code {e}"
        );
    }
}
