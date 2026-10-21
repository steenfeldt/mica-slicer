// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

use mica_core::{
    config::Config,
    geometry::{Point, signed_area},
    mesh::TriangleMesh,
    print::slice_object,
    seam::ScarfPlacement,
    settings::{FloatOrPercent, PerimeterGenerator, PrintSettings},
    toolpath::Role,
};

fn cube() -> TriangleMesh {
    let mut mesh = TriangleMesh::load_stl(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/models/cube20.stl"
    ))
    .unwrap();
    mesh.place_on_bed(125., 110.);
    mesh
}

#[test]
fn sandwich_and_alternating_reversal_both_generators() {
    for generator in [PerimeterGenerator::Classic, PerimeterGenerator::Arachne] {
        let mut s = PrintSettings::core_one_hf04_speed_pla();
        s.perimeters = 3;
        s.perimeter_generator = generator;
        s.orca_walls.inner_outer_inner = true;
        s.orca_walls.overhang_reverse = true;
        s.orca_walls.threshold = FloatOrPercent::Mm(0.);
        let sliced = slice_object(&cube(), &s).unwrap();
        for i in 0..4 {
            let walls: Vec<_> = sliced.layers[i]
                .paths
                .iter()
                .filter(|p| matches!(p.role, Role::Perimeter | Role::ExternalPerimeter))
                .collect();
            let depths: Vec<_> = walls.iter().map(|p| p.perimeter_index.unwrap()).collect();
            assert_eq!(
                depths,
                if i == 0 { vec![2, 1, 0] } else { vec![2, 0, 1] },
                "{generator:?} layer {i}"
            );
            for p in walls {
                assert_eq!(
                    signed_area(&p.points) < 0.,
                    i % 2 == 1,
                    "{generator:?} layer {i}"
                );
            }
        }
        s.orca_walls.internal_only = true;
        let sliced = slice_object(&cube(), &s).unwrap();
        let walls: Vec<_> = sliced.layers[1]
            .paths
            .iter()
            .filter(|p| matches!(p.role, Role::Perimeter | Role::ExternalPerimeter))
            .collect();
        for p in walls {
            assert_eq!(signed_area(&p.points) < 0., p.perimeter_index != Some(0));
        }
        s.orca_walls.threshold = FloatOrPercent::Percent(50.);
        let sliced = slice_object(&cube(), &s).unwrap();
        for p in &sliced.layers[1].paths {
            if matches!(p.role, Role::Perimeter | Role::ExternalPerimeter) {
                assert!(signed_area(&p.points) > 0.);
            }
        }
    }
}

#[test]
fn optional_settings_do_not_inject_orca_defaults_and_reject_bad_values() {
    let mut c = Config::defaults();
    for key in [
        "wall_sequence",
        "bridge_density",
        "seam_slope_type",
        "overhang_reverse",
    ] {
        assert!(c.get(key).is_none());
    }
    let default = PrintSettings::from_config(&c).unwrap();
    assert!(!default.orca_walls.inner_outer_inner && !default.orca_walls.overhang_reverse);
    c.set_raw("wall_sequence", "inner-outer-inner wall")
        .unwrap();
    c.set_raw("overhang_reverse_threshold", "0.2").unwrap();
    c.set_raw("internal_bridge_angle", "37").unwrap();
    c.set_raw("bridge_density", "80%").unwrap();
    let s = PrintSettings::from_config(&c).unwrap();
    assert!(s.orca_walls.inner_outer_inner);
    assert_eq!(s.orca_walls.threshold, FloatOrPercent::Mm(0.2));
    assert_eq!(s.internal_bridge_angle, 37.);
    assert_eq!(s.bridge_density, 0.8);
    for (key, value) in [
        ("wall_sequence", "bogus"),
        ("overhang_reverse_threshold", "-1%"),
        ("internal_bridge_angle", "181"),
        ("enable_extra_bridge_layer", "true"),
        ("bridge_density", "0%"),
        ("internal_bridge_density", "126%"),
    ] {
        let mut bad = c.clone();
        bad.set_raw(key, value).unwrap();
        assert!(PrintSettings::from_config(&bad).is_err(), "{key}");
    }
}

#[test]
fn orca_scarf_resolves_units_and_applies_to_inner_walls() {
    let mut c = Config::defaults();
    for (k, v) in [
        ("mica_slicing_rules", "orcaslicer"),
        ("seam_slope_type", "all"),
        // A PrusaSlicer profile's "only on smooth perimeters" (on) is the merged Conditional
        // scarf joint; this test wants the scarf on the cube's corners.
        ("seam_slope_conditional", "0"),
        ("seam_slope_inner_walls", "1"),
        ("seam_slope_start_height", "0.04"),
        ("seam_slope_min_length", "6"),
        ("scarf_joint_speed", "40%"),
        ("scarf_joint_flow_ratio", "0.8"),
    ] {
        c.set_raw(k, v).unwrap();
    }
    let s = PrintSettings::from_config(&c).unwrap();
    assert!(s.seam.scarf.orca && s.seam.scarf.on_inner_perimeters);
    let sliced = slice_object(&cube(), &s).unwrap();
    assert!(
        sliced.layers[0]
            .paths
            .iter()
            .all(|p| p.seam.as_ref().is_none_or(|m| m.samples.is_empty()))
    );
    let walls: Vec<_> = sliced.layers[1]
        .paths
        .iter()
        .filter(|p| matches!(p.role, Role::ExternalPerimeter | Role::Perimeter))
        .collect();
    assert!(!walls.is_empty());
    for p in walls {
        let meta = p.seam.as_ref().unwrap();
        assert!(!meta.samples.is_empty());
        assert!((meta.samples[0].height - 0.04 / sliced.layers[1].height).abs() < 1e-8);
        let base = if p.role == Role::ExternalPerimeter {
            s.external_perimeter_speed
        } else {
            s.perimeter_speed
        };
        assert_eq!(meta.scarf_speed, Some(0.4 * base));
        assert_ne!(p.points[0], Point::default());
    }
    c.set_raw("seam_slope_conditional", "1").unwrap();
    let sliced = slice_object(&cube(), &PrintSettings::from_config(&c).unwrap()).unwrap();
    assert!(
        sliced.layers[1]
            .paths
            .iter()
            .all(|p| p.seam.as_ref().is_none_or(|m| m.samples.is_empty()))
    );
}

/// One scarf setting on every printer: PrusaSlicer rules take the values PrusaSlicer's scarf
/// has (Conditional scarf joint is "only on smooth perimeters") and leave Orca's extras
/// neutral; OrcaSlicer rules take them all.
#[test]
fn prusa_rules_take_only_the_scarf_values_prusaslicer_has() {
    let mut c = Config::defaults();
    for (key, value) in [
        ("seam_slope_type", "external"),
        ("seam_slope_conditional", "1"),
        ("scarf_angle_threshold", "170"),
        ("scarf_overhang_threshold", "20%"),
        ("scarf_joint_speed", "40%"),
        ("scarf_joint_flow_ratio", "0.9"),
    ] {
        c.set_raw(key, value).unwrap();
    }
    let s = PrintSettings::from_config(&c).unwrap();
    assert!(!s.seam.scarf.orca);
    assert_eq!(s.seam.scarf.placement, ScarfPlacement::Contours);
    assert!(s.seam.scarf.only_on_smooth && !s.seam.scarf.conditional);
    assert_eq!(s.seam.scarf.speed, FloatOrPercent::Percent(100.));
    assert_eq!(s.seam.scarf.flow_ratio, 1.);
    c.set_raw("mica_slicing_rules", "orcaslicer").unwrap();
    let s = PrintSettings::from_config(&c).unwrap();
    assert!(s.seam.scarf.orca && s.seam.scarf.conditional);
    assert_eq!(s.seam.scarf.angle_threshold, 170.);
    assert_eq!(s.seam.scarf.overhang_threshold, 0.2);
    assert_eq!(s.seam.scarf.speed, FloatOrPercent::Percent(40.));
    assert_eq!(s.seam.scarf.flow_ratio, 0.9);
}
