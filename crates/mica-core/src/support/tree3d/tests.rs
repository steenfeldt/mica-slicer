// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/Support/TreeSupportCommon.cpp :: TreeSupportSettings
// Source: prusaslicer@b028299c77 src/libslic3r/Support/TreeModelVolumes.cpp :: ceilRadius, calculateCollision, calculateAvoidance, calculatePlaceables, calculateWallRestrictions
// Source: orca@8500fcdcca src/libslic3r/Support/TreeSupportCommon.hpp :: TreeSupportMeshGroupSettings, layer_idx_ceil, layer_idx_floor
// Source: orca@8500fcdcca src/libslic3r/Support/TreeSupport3D.hpp :: propagate_down
// Changes: Hand-derived arithmetic and synthetic polygon assertions; config fixtures record CORE One and U1 organic values.
// Copyright (c) Prusa Research 2022 - 2023 Vojtěch Bubník @bubnikv, Pavel Mikuš @Godrak
// PrusaSlicer is released under the terms of the AGPLv3 or higher.
// Tree supports by Thomas Rahm, losely based on Tree Supports by CuraEngine.
// Original source of Thomas Rahm's tree supports: https://github.com/ThomasRahm/CuraEngine
// Copyright (c) 2021 Ultimaker B.V.
// CuraEngine is released under the terms of the AGPLv3 or higher.

use super::*;
use crate::{
    config::Config,
    geometry::{ExPolygon, Point, clip},
    rules::Rules,
};

fn config(rules: Rules) -> Config {
    let mut c = Config::defaults();
    rules.set(&mut c);
    // Flattened 0.20mm SPEED @COREONE HF0.4 organic settings. U1 values below
    // follow fdm_process_U1 -> _common -> _0.20 -> 0.20 Standard (0.4 nozzle).
    for (k, v) in [
        ("layer_height", "0.2"),
        ("first_layer_height", "0.2"),
        ("nozzle_diameter", "0.4"),
        ("extrusion_width", "0.45"),
        ("external_perimeter_extrusion_width", "0.45"),
        ("gcode_resolution", "0.008"),
        ("support_material_extrusion_width", "0.4"),
        ("support_material_style", "organic"),
        ("support_material", "1"),
        ("support_material_auto", "0"),
        ("support_material_threshold", "35"),
        ("support_material_spacing", "2"),
        ("support_tree_branch_diameter_double_wall", "8"),
        ("support_material_interface_pattern", "auto"),
        ("support_material_xy_spacing", "80%"),
        ("support_material_interface_spacing", "0.2"),
        ("support_material_contact_distance", "0.2"),
        ("support_material_bottom_contact_distance", "0"),
        ("support_material_bottom_interface_layers", "0"),
        ("support_material_interface_layers", "3"),
        ("support_tree_angle", "40"),
        ("support_tree_angle_slow", "25"),
        ("support_tree_branch_diameter", "2"),
        ("support_tree_branch_diameter_angle", "5"),
        ("support_tree_tip_diameter", "0.8"),
        ("support_tree_top_rate", "30%"),
    ] {
        c.set_raw(k, v).unwrap();
    }
    if rules.is_orca() {
        for (k, v) in [
            ("initial_layer_print_height", "0.25"),
            ("support_line_width", "0.42"),
            ("line_width", "0.42"),
            ("external_perimeter_extrusion_width", "0.42"),
            ("resolution", "0.012"),
            ("support_object_xy_distance", "0.35"),
            ("support_object_first_layer_gap", "0.2"),
            ("support_top_z_distance", "0.2"),
            ("support_bottom_z_distance", "0.2"),
            ("support_interface_top_layers", "2"),
            ("support_interface_bottom_layers", "2"),
            ("support_interface_spacing", "0.5"),
            ("support_base_pattern_spacing", "2.5"),
            ("support_threshold_angle", "30"),
            ("support_base_pattern", "default"),
            ("support_interface_pattern", "auto"),
            ("tree_support_branch_angle", "45"),
            ("tree_support_branch_diameter_organic", "2"),
            ("tree_support_branch_angle_organic", "40"),
            ("tree_support_angle_slow", "25"),
            ("tree_support_branch_diameter_angle", "5"),
            ("tree_support_tip_diameter", "0.8"),
            ("tree_support_branch_distance_organic", "1"),
            ("tree_support_top_rate", "30%"),
            ("tree_support_wall_count", "0"),
        ] {
            c.set_raw(k, v).unwrap();
        }
    }
    c
}
fn settings(rules: Rules) -> TreeSupportSettings {
    let m = TreeSupportMeshGroupSettings::from_config(&config(rules)).unwrap();
    TreeSupportSettings::new(
        &m,
        &TreeSlicingParameters::new(0.2, if rules.is_orca() { 0.25 } else { 0.2 }),
        false,
    )
}
fn near(value: f64, expected: f64) {
    assert!((value - expected).abs() < 1e-6, "{value} != {expected}");
}
fn rect(x: f64, y: f64, w: f64, h: f64) -> Areas {
    vec![ExPolygon {
        contour: [(x, y), (x + w, y), (x + w, y + h), (x, y + h)]
            .into_iter()
            .map(|(x, y)| Point::new(scaled(x), scaled(y)))
            .collect(),
        holes: vec![],
    }]
}
// jtMiter limit1.2 falls back to square joins at a right-angle convex corner.
// Each of four corner cuts is a triangle with legs (2-sqrt(2))*delta.
// Integer offset vertices introduce at most a few scaled-square units of error.
/// Area of an axis-aligned square grown by a jtMiter offset. ClipperUtils passes miter limit
/// 1.2, but Clipper 6.4 (`ClipperOffset::Execute`) treats limits up to 2 as 2
/// (`m_miterLim = 0.5`), so right angles keep their full mitered corners.
fn square_offset_area(side: f64, delta: f64) -> f64 {
    (side + 2.0 * delta).powi(2)
}
fn area(a: &[ExPolygon]) -> f64 {
    a.iter().map(ExPolygon::area_mm2).sum()
}
fn has(a: &[ExPolygon], x: f64, y: f64) -> bool {
    // Probe a tiny rectangle instead of depending on the old tree module's
    // point-in-polygon helper. All probes sit well away from boundaries.
    area(&clip::intersection_ex(
        a,
        &rect(x - 0.0001, y - 0.0001, 0.0002, 0.0002),
    )) > 1e-9
}
fn synthetic(mut s: TreeSupportSettings, outlines: Vec<Areas>) -> TreeModelVolumes {
    s.resolution = 0; // analytical fixtures don't simplify away round corners
    TreeModelVolumes::new(s, outlines, vec![], rect(-50.0, -50.0, 100.0, 100.0))
}
#[test]
fn profile_settings_derived_values() {
    for rules in Rules::ALL {
        let s = settings(rules);
        // r_branch=2/2=1mm, r_tip=.8/2=.4mm. Integer tip ramp is
        // max(floor(.6/(width/3)),floor(1/.2)) = max(4,5) = 5 layers.
        assert_eq!(
            (s.branch_radius, s.min_radius, s.tip_layers),
            (1_000_000, 400_000, 5)
        );
        assert_eq!(
            (s.maximum_move_distance, s.maximum_move_distance_slow),
            (167_819, 93_261)
        );
        near(s.branch_radius_increase_per_layer, 17_497.7327051848);
        near(s.bp_radius_increase_per_layer, 168_457.6760926159);
        assert_eq!(
            (s.max_to_model_radius_increase, s.min_dtt_to_model),
            (500_000, 5)
        );
        assert_eq!((s.bp_radius, s.layer_start_bp_radius), (3_750_000, 16));
        assert_eq!(s.recommended_min_radius(0), 3_695_322);
        assert_eq!(s.recommended_min_radius(16), 0);
        assert_eq!(s.increase_radius_until_layer, 5);
        assert_eq!(s.z_distance_top_layers, 1);
        assert_eq!(s.z_distance_bottom_layers, 1);
        if rules.is_orca() {
            assert_eq!(s.support_line_width, 419_999); // float Flow::scaled_width
            assert_eq!(s.support_bottom_layers, 2);
            assert_eq!(s.xy_distance, 350_000);
            assert_eq!(s.xy_min_distance, 209_999);
            assert_eq!(s.support_roof_line_distance, 919_999);
            assert_eq!(s.support_line_spacing, 2_500_000);
            assert_eq!(s.resolution, 12_000);
            assert_eq!(s.settings.support_xy_distance_1st_layer, 200_000);
        } else {
            assert_eq!(s.support_line_width, 400_000);
            assert_eq!(s.support_bottom_layers, 0);
            assert_eq!(s.xy_distance, 359_999);
            assert_eq!(s.xy_min_distance, 224_999);
            assert_eq!(s.support_roof_line_distance, 600_000);
            assert_eq!(s.resolution, 8_000);
            assert_eq!(s.support_line_spacing, 2_000_000);
        }
        assert_eq!(s.settings.support_tree_top_rate, 30.0);
        assert_eq!(s.support_wall_count, 1);
    }
}
#[test]
fn radius_tip_growth_foot_and_frozen_collision() {
    let s = settings(Rules::PrusaSlicer);
    for (d, r) in [
        (0, 400_000),
        (1, 520_000),
        (2, 640_000),
        (5, 1_000_000),
        (6, 1_017_497),
        (10, 1_087_488),
    ] {
        assert_eq!(s.get_radius(d, 0.0), r);
    }
    // At DTT6 one regular growth plus one foot growth difference = .168457mm.
    assert_eq!(s.get_radius(6, 1.0), 1_168_457);
    assert_eq!(s.get_radius(0, 0.5), 475_479);
    let mut e = SupportElementState {
        distance_to_top: 10,
        effective_radius_height: 2,
        elephant_foot_increases: 0.5,
        ..Default::default()
    };
    // Drawing catches up to the forced branch radius at effective DTT=min(10,5).
    assert_eq!(get_effective_dtt(&s, &e), 5);
    assert_eq!(s.get_radius_state(&e), 1_075_479);
    assert_eq!(s.get_collision_radius(&e), 715_479);
    e.effective_radius_height = 6;
    assert_eq!(get_effective_dtt(&s, &e), 6);
    let mut capped = s.clone();
    capped.branch_radius_increase_per_layer = capped.bp_radius_increase_per_layer + 1.0;
    assert_eq!(capped.get_radius(0, 2.0), 400_000); // max(foot-growth,0)
}
#[test]
fn state_propagation_and_orca_roof_recovery() {
    let e = SupportElementState {
        layer_idx: 8,
        distance_to_top: 2,
        dont_move_until: 4,
        missing_roof_layers: 2,
        result_on_layer: Some(Point::new(4, 7)),
        skip_ovalisation: true,
        orca: OrcaElementMetadata {
            roof_recovery_dtt: 3,
            ..Default::default()
        },
        ..Default::default()
    };
    for rules in Rules::ALL {
        let d = e.propagate_down(rules);
        assert_eq!((d.layer_idx, d.distance_to_top), (7, 3));
        assert!(!d.result_on_layer_is_set() && !d.skip_ovalisation && d.locked());
        assert_eq!(d.missing_roof_layers, if rules.is_orca() { 1 } else { 2 });
        assert_eq!(
            d.orca.roof_recovery_dtt,
            if rules.is_orca() { 4 } else { 3 }
        );
    }
    let mut d = e;
    d.set_pending_roof_recovery(0, 5, Rules::OrcaSlicer);
    assert!(!d.has_pending_roof_recovery());
    assert_eq!(d.orca.roof_recovery_dtt, 0);
}
#[test]
fn radius_buckets_and_xy_delta() {
    let v = synthetic(settings(Rules::PrusaSlicer), vec![vec![]]);
    // r0=.4, initial delta=1.5-.4=1.1, ceil(1.1/1.5)=1 linear
    // step. After testing 1.5 the upstream loop adds ANOTHER 1.1,
    // giving 2.6 before the exponential sequence 3.9,5.85,...
    for (r, b) in [
        (0, 0),
        (1, 400_000),
        (400_000, 400_000),
        (400_001, 1_500_000),
        (1_500_000, 1_500_000),
        (1_500_001, 2_600_000),
        (2_600_001, 3_900_000),
        (3_900_001, 5_850_000),
    ] {
        assert_eq!(v.ceil_radius_raw(r), b);
    }
    assert_eq!(v.min_xy_dist_delta(), 135_000);
    assert_eq!(v.ceil_radius(0, false), 135_000);
    assert_eq!(v.ceil_radius(0, true), 0);
    assert_eq!(v.ceil_radius(400_000, false), 1_500_000);
    assert_eq!(v.get_radius_next_ceil(400_000, false), 1_365_000);
}
#[test]
fn slab_and_pillar_collision_with_top_taper() {
    let mut s = settings(Rules::PrusaSlicer);
    s.xy_min_distance = scaled(0.2);
    s.xy_distance = scaled(0.4);
    s.z_distance_bottom_layers = 0;
    s.z_distance_top_layers = 1;
    let mut v = synthetic(
        s,
        vec![
            rect(-1.0, -1.0, 2.0, 2.0),
            vec![],
            rect(-5.0, -5.0, 10.0, 10.0),
        ],
    );
    // At layer1 the next-layer slab contributes .5*xy_min=.1mm
    // (not .2), with no own outline: a mitered square of side 10.2.
    near(
        area(&v.get_collision(0, 1, true)),
        square_offset_area(10.0, 0.1),
    );
    assert!(has(&v.get_collision(0, 1, true), 5.05, 0.0));
    assert!(!has(&v.get_collision(0, 1, true), 5.15, 0.0));
    // Normal radius-zero bypasses bucketing: add delta=.2, total=.3.
    near(
        area(&v.get_collision(0, 1, false)),
        square_offset_area(10.0, 0.3),
    );
    near(
        area(&v.get_collision(0, 0, true)),
        square_offset_area(2.0, 0.2),
    ); // pillar side2.4
    // 2-layer top gap: first slab clearance=xy/2; final=0.
    v.settings.z_distance_top_layers = 2;
    v.clear();
    near(area(&v.get_collision(0, 0, true)), 100.0);
    // Bottom distance includes the preceding model layer at full clearance.
    v.settings.z_distance_top_layers = 0;
    v.settings.z_distance_bottom_layers = 1;
    v.clear();
    near(
        area(&v.get_collision(0, 1, true)),
        square_offset_area(2.0, 0.2),
    );
}
#[test]
fn avoidance_speeds_to_model_and_placeables() {
    let mut s = settings(Rules::PrusaSlicer);
    s.xy_distance = 0;
    s.xy_min_distance = 0;
    s.z_distance_bottom_layers = 0;
    s.z_distance_top_layers = 0;
    // Keep query radius .4 bucketed exactly, and disable holefree conversion.
    s.increase_radius_until_radius = 0;
    s.maximum_move_distance = scaled(0.5) + 2;
    s.maximum_move_distance_slow = scaled(0.25) + 2;
    let v = synthetic(
        s,
        vec![
            rect(-5.0, -5.0, 10.0, 10.0),
            rect(-1.0, -1.0, 2.0, 2.0),
            vec![],
        ],
    );
    // At radius .4 the first slab collision is a square halfwidth5.4.
    // A_fast(1) = C(1) union shrink(C(0),.5): halfwidth4.9.
    // A_slow(1) uses .25 instead: halfwidth5.15. No corner approximation
    // enters the mid-edge probes; negative round offsets of squares are exact.
    near(
        area(&v.get_avoidance(400_000, 1, AvoidanceType::Fast, false, true)),
        9.8 * 9.8,
    );
    // Shrinking the .4-offset mitered square by .25 leaves a .15-offset square.
    near(
        area(&v.get_avoidance(400_000, 1, AvoidanceType::Slow, false, true)),
        square_offset_area(10.0, 0.15),
    );
    assert!(has(
        &v.get_avoidance(400_000, 1, AvoidanceType::Slow, false, true),
        5.0,
        0.0
    ));
    assert!(!has(
        &v.get_avoidance(400_000, 1, AvoidanceType::Fast, false, true),
        5.0,
        0.0
    ));
    // Placeable0(1)=slab - pillar; .4 inward offset gives an annulus:
    // outer halfwidth4.6 and hole halfwidth1.4 (beveled at miterlimit1.2).
    assert!(has(&v.get_placeable_areas(400_000, 1), 3.0, 0.0));
    assert!(!has(&v.get_placeable_areas(400_000, 1), 4.8, 0.0));
    assert!(has(
        &v.get_avoidance(400_000, 1, AvoidanceType::Fast, false, true),
        3.0,
        0.0
    ));
    for kind in [
        AvoidanceType::Slow,
        AvoidanceType::FastSafe,
        AvoidanceType::Fast,
    ] {
        assert!(!has(
            &v.get_avoidance(400_000, 1, kind, true, true),
            3.0,
            0.0
        ));
        assert!(has(
            &v.get_avoidance(400_000, 1, kind, true, true),
            0.0,
            0.0
        ));
        assert_eq!(
            v.get_avoidance(400_000, 0, kind, true, true).clone(),
            v.get_collision(400_000, 0, true).clone()
        );
    }
}
#[test]
fn holefree_avoids_a_narrow_model_hole() {
    let mut s = settings(Rules::PrusaSlicer);
    s.xy_distance = 0;
    s.xy_min_distance = 0;
    s.z_distance_top_layers = 0;
    s.z_distance_bottom_layers = 0;
    let ring = clip::diff_ex(&rect(-5.0, -5.0, 10.0, 10.0), &rect(-0.8, -0.8, 1.6, 1.6));
    let v = synthetic(s, vec![ring.clone(), ring]);
    // r=.4 leaves an .8mm-wide hole in normal collision. Forced-growth
    // radius1 rounds to1.5, closing the 1.6mm hole. Holefree shrinks that
    // collision by1.5-.4-5/SCALE=1.099995 without reopening the hole.
    assert!(!has(&v.get_collision(400_000, 1, true), 0.0, 0.0));
    assert!(has(&v.get_collision_holefree(400_000, 1), 0.0, 0.0));
    assert!(!has(
        &v.get_avoidance(400_000, 1, AvoidanceType::Fast, false, true),
        0.0,
        0.0
    ));
    for kind in [AvoidanceType::Slow, AvoidanceType::FastSafe] {
        for model in [false, true] {
            assert!(has(
                &v.get_avoidance(400_000, 1, kind, model, true),
                0.0,
                0.0
            ));
        }
    }
}
#[test]
fn wall_minimum_and_normal_and_cache_clearing() {
    let mut s = settings(Rules::PrusaSlicer);
    s.xy_min_distance = scaled(0.2);
    s.xy_distance = scaled(0.4);
    s.z_distance_bottom_layers = 0;
    s.z_distance_top_layers = 0;
    let outline = rect(-2.0, -2.0, 4.0, 4.0);
    let mut v = synthetic(s, vec![outline.clone(), outline]);
    // W_normal(r,1)=C_normal(0,1) intersect C_min(r,0).
    // At r=.4: min upper extent2.2, normal upper2.4, lower2.6.
    assert!(has(&v.get_wall_restriction(400_000, 1, false), 2.3, 0.0));
    assert!(!has(&v.get_wall_restriction(400_000, 1, true), 2.3, 0.0));
    v.get_collision(0, 1, true);
    v.get_collision(400_000, 1, true);
    assert_eq!(
        v.get_collision_lower_bound_area(1, 500_000).unwrap().0,
        400_000
    );
    v.clear_all_but_object_collision();
    assert!(v.get_collision_lower_bound_area(1, 500_000).is_some());
    v.clear();
    assert!(v.get_collision_lower_bound_area(1, 500_000).is_none());
}
#[test]
fn precalculate_matches_lazy_and_request_order() {
    for rules in Rules::ALL {
        let mut s = settings(rules);
        s.z_distance_top_layers = 0;
        s.z_distance_bottom_layers = 0;
        let layers = vec![
            rect(-5.0, -5.0, 10.0, 10.0),
            rect(-1.0, -1.0, 2.0, 2.0),
            vec![],
            vec![],
        ];
        let lazy = synthetic(s.clone(), layers.clone());
        let mut eager = synthetic(s, layers);
        let first = (*lazy.get_avoidance(400_000, 3, AvoidanceType::FastSafe, false, true)).clone();
        eager.precalculate(3);
        assert!(eager.precalculated);
        assert_eq!(
            first,
            *eager.get_avoidance(400_000, 3, AvoidanceType::FastSafe, false, true)
        );
        for r in [0, 400_000, 1_500_000] {
            for l in (0..4).rev() {
                assert_eq!(
                    lazy.get_collision(r, l, false),
                    eager.get_collision(r, l, false)
                );
                for k in [
                    AvoidanceType::Fast,
                    AvoidanceType::Slow,
                    AvoidanceType::FastSafe,
                ] {
                    for model in [true, false] {
                        assert_eq!(
                            lazy.get_avoidance(r, l, k, model, true),
                            eager.get_avoidance(r, l, k, model, true)
                        );
                    }
                }
            }
        }
    }
}
#[test]
fn rule_switches_options_floor_and_zero_gap() {
    // Changing rules on a Prusa profile must retain its resolved percent XY
    // option when there is no raw Orca distance (80% is not 80 millimetres).
    let mut prusa_profile = config(Rules::PrusaSlicer);
    Rules::OrcaSlicer.set(&mut prusa_profile);
    assert_eq!(
        TreeSupportMeshGroupSettings::from_config(&prusa_profile)
            .unwrap()
            .support_xy_distance,
        359_999
    );
    for rules in Rules::ALL {
        let mut c = config(rules);
        c.set_raw("tree_support_branch_angle_organic", "15")
            .unwrap();
        c.set_raw("tree_support_wall_count", "2").unwrap();
        c.set_raw("support_material_bottom_interface_layers", "-1")
            .unwrap();
        c.set_raw("support_interface_bottom_layers", "-1").unwrap();
        let m = TreeSupportMeshGroupSettings::from_config(&c).unwrap();
        near(
            m.support_tree_angle.to_degrees(),
            if rules.is_orca() { 15.0 } else { 40.0 },
        );
        assert_eq!(m.support_wall_count, if rules.is_orca() { 2 } else { 1 });
        assert_eq!(m.support_floor_enable, rules.is_orca());
        assert_eq!(m.support_floor_layers, if rules.is_orca() { 2 } else { 0 });
        let mut m = m;
        m.support_xy_distance = 0;
        m.support_xy_distance_overhang = 0;
        let p = TreeSlicingParameters::new(0.2, 0.2);
        assert_eq!(
            TreeSupportSettings::new(&m, &p, true).xy_min_distance,
            100_000
        );
        assert_eq!(TreeSupportSettings::new(&m, &p, false).xy_min_distance, 0);
    }
}
#[test]
fn raft_layer_grid_and_interface_reservation() {
    let p = TreeSlicingParameters {
        object_print_z_min: 0.900_01,
        base_raft_layers: 1,
        interface_raft_layers: 1,
        raft_contact_top_z: 0.5,
        max_support_layer_height: 0.2,
        ..TreeSlicingParameters::new(0.2, 0.2)
    };
    for rules in Rules::ALL {
        let m = TreeSupportMeshGroupSettings::from_config(&config(rules)).unwrap();
        let s = TreeSupportSettings::new(&m, &p, false);
        // Remaining .40001mm: Prusa ceil(.40001/.2)=3, Orca
        // ceil((.40001-.0001)/.2)=2. First+contact precede these.
        assert_eq!(s.raft_layers.len(), if rules.is_orca() { 4 } else { 5 });
        near(s.layer_z(&p, 0), 0.2);
        near(s.layer_z(&p, s.raft_layers.len()), 1.100_01);
        assert_eq!(
            s.layer_idx_ceil(&p, 0.49),
            if rules.is_orca() { 1 } else { 5 }
        );
        assert_eq!(
            s.layer_idx_floor(&p, 0.49),
            if rules.is_orca() { 0 } else { 5 }
        );
        assert_eq!(s.roof_is_interface(2, 2, 1), !rules.is_orca());
    }
}

#[test]
fn safe_avoidance_steps_cannot_tunnel_through_ring_wall() {
    let mut s = settings(Rules::PrusaSlicer);
    s.xy_distance = 0;
    s.xy_min_distance = 0;
    s.z_distance_top_layers = 0;
    s.z_distance_bottom_layers = 0;
    s.increase_radius_until_radius = 0;
    s.maximum_move_distance = scaled(2.0) + 2;
    let bottom = rect(-5.0, -5.0, 10.0, 10.0);
    let ring = clip::diff_ex(&bottom, &rect(-4.0, -4.0, 8.0, 8.0));
    let v = synthetic(s, vec![bottom, ring]);
    // Radius .4 gives the ring a hole halfwidth3.6. Shrinking previous
    // avoidance by the full2mm at once gives halfwidth3.4, exposing a false
    // reachable gap at x=3.5 INSIDE the closed ring. Upstream's .76,.76,.48
    // steps union the wall after EACH shrink; overlap keeps the interior filled.
    assert!(!has(&v.get_collision(400_000, 1, true), 3.5, 0.0));
    assert!(has(
        &v.get_avoidance(400_000, 1, AvoidanceType::Fast, false, true),
        3.5,
        0.0
    ));
}

#[test]
fn drawing_wall_thresholds_and_tool_flow_follow_rules() {
    for rules in Rules::ALL {
        let mut c = config(rules);
        for count in [0, 1, 2] {
            c.set_raw("tree_support_wall_count", &count.to_string())
                .unwrap();
            let m = TreeSupportMeshGroupSettings::from_config(&c).unwrap();
            let threshold = m.tree_branch_diameter_double_wall_area_scaled;
            if rules.is_orca() {
                match count {
                    0 => near(
                        threshold / (crate::geometry::SCALE.powi(2)),
                        6.25 * std::f64::consts::PI,
                    ),
                    1 => assert_eq!(threshold, f64::MAX),
                    _ => assert_eq!(threshold, 0.1),
                }
            } else {
                near(
                    threshold / crate::geometry::SCALE.powi(2),
                    16.0 * std::f64::consts::PI,
                );
            }
        }
        // Width=0 falls back to generic width=0, then support nozzle. Tool 0
        // uses the minimum nozzle under Prusa rules and the first under Orca.
        for (k, v) in [
            ("nozzle_diameter", "0.6,0.4"),
            ("extrusion_width", "0"),
            ("support_material_extrusion_width", "0"),
            ("line_width", "0"),
            ("support_line_width", "0"),
            ("support_material_extruder", "0"),
            ("support_filament", "0"),
            ("support_material_interface_extruder", "2"),
            ("support_interface_filament", "2"),
        ] {
            c.set_raw(k, v).unwrap();
        }
        let m = TreeSupportMeshGroupSettings::from_config(&c).unwrap();
        assert_eq!(
            m.support_line_width,
            if rules.is_orca() { 600_000 } else { 400_000 }
        );
        assert_eq!(m.support_roof_line_width, 400_000);
    }
}

#[test]
fn zero_movement_and_precache_cancellation() {
    let mut s = settings(Rules::PrusaSlicer);
    s.maximum_move_distance = 0;
    s.increase_radius_until_radius = 0;
    s.z_distance_bottom_layers = 0;
    s.z_distance_top_layers = 0;
    let mut v = synthetic(s, vec![rect(-5.0, -5.0, 10.0, 10.0), vec![]]);
    let collision = (*v.get_collision(400_000, 0, true)).clone();
    assert_eq!(
        *v.get_avoidance(400_000, 1, AvoidanceType::Fast, false, true),
        collision
    );
    assert_eq!(
        v.precalculate_with_cancel(1, || Err("cancelled".into())),
        Err("cancelled".into())
    );
    assert!(!v.precalculated);
    v.precalculate(1);
    assert!(v.precalculated);
}

#[test]
fn orca_pattern_options_retain_the_full_upstream_enum() {
    let mut c = config(Rules::OrcaSlicer);
    for (raw, expected) in [
        ("default", TreeSupportPattern::Default),
        ("lightning", TreeSupportPattern::Lightning),
        ("hollow", TreeSupportPattern::Hollow),
    ] {
        c.set_raw("support_base_pattern", raw).unwrap();
        assert_eq!(
            TreeSupportMeshGroupSettings::from_config(&c)
                .unwrap()
                .support_pattern,
            expected
        );
    }
    for (raw, expected) in [
        (
            "rectilinear_interlaced",
            TreeInterfacePattern::RectilinearInterlaced,
        ),
        ("grid", TreeInterfacePattern::Grid),
    ] {
        c.set_raw("support_interface_pattern", raw).unwrap();
        assert_eq!(
            TreeSupportMeshGroupSettings::from_config(&c)
                .unwrap()
                .support_roof_pattern,
            expected
        );
    }
    Rules::PrusaSlicer.set(&mut c);
    let m = TreeSupportMeshGroupSettings::from_config(&c).unwrap();
    assert_eq!(m.support_pattern, TreeSupportPattern::Rectilinear);
    assert_eq!(m.support_roof_pattern, TreeInterfacePattern::Auto);
}

/// `SupportParameters`: a soluble interface on another tool over a non-soluble base gets base
/// interface layers. Each tool's resolved settings (`tool_config`) must still see the other
/// tools' `filament_soluble`, or the interface tool reads as non-soluble.
#[test]
fn soluble_interface_base_layers_read_every_tool() {
    for rules in [Rules::PrusaSlicer, Rules::OrcaSlicer] {
        let mut c = config(rules);
        for (k, v) in [
            ("nozzle_diameter", "0.4,0.4"),
            ("filament_soluble", "0,1"),
            ("support_material_extruder", "0"),
            ("support_filament", "0"),
            ("support_material_interface_extruder", "2"),
            ("support_interface_filament", "2"),
            ("support_material_contact_distance", "0"),
            ("support_top_z_distance", "0"),
            ("support_material_interface_layers", "3"),
            ("support_interface_top_layers", "3"),
        ] {
            c.set_raw(k, v).unwrap();
        }
        let s = crate::settings::PrintSettings::from_config(&crate::print_job::tool_config(&c, 0))
            .unwrap();
        let t = s.tree3d.unwrap();
        assert_eq!(t.top_base_layers, 1, "{rules:?}");
        // Object on tool 1 (zero based 0), interface on tool 2: no merge.
        assert!(!t.can_merge_support_regions, "{rules:?}");
    }
}

/// `can_merge_support_regions` keeps upstream's comparison of the zero-based object extruder
/// with the one-based support tool.
#[test]
fn support_regions_merge_as_upstream() {
    assert!(Tree3dConfig::can_merge(2, 2, &[0]));
    assert!(!Tree3dConfig::can_merge(0, 2, &[0]));
    assert!(Tree3dConfig::can_merge(0, 1, &[1]));
    assert!(!Tree3dConfig::can_merge(0, 1, &[0]));
    assert!(!Tree3dConfig::can_merge(0, 2, &[0, 2]));
    assert!(Tree3dConfig::can_merge(0, 0, &[0, 1]));
}

#[test]
fn tip_sampling_lines_follow_the_area_not_the_bed() {
    // The tip sampling filler never gets an object bounding box, so upstream anchors the line
    // grid at each ExPolygon's own bounding-box centre: moving an overhang moves its lines.
    for rules in Rules::ALL {
        let c = settings(rules);
        let p = TreeSupportInfill::new(&c);
        let lines = |dx: f64| {
            generate_support_infill_lines(
                &rect(1.0 + dx, 2.0, 7.3, 4.1),
                &c,
                &p,
                false,
                3,
                c.support_line_spacing,
            )
        };
        let base = lines(0.0);
        assert!(!base.is_empty());
        for dx in [0.37, 11.0, 123.45] {
            let shifted: Vec<Vec<Point>> = lines(dx)
                .into_iter()
                .map(|l| {
                    l.into_iter()
                        .map(|q| Point::new(q.x - scaled(dx), q.y))
                        .collect()
                })
                .collect();
            assert_eq!(shifted.len(), base.len());
            for (a, b) in shifted.iter().zip(&base) {
                assert_eq!(a.len(), b.len());
                for (q, r) in a.iter().zip(b) {
                    assert!(
                        (q.x - r.x).abs() <= 2 && (q.y - r.y).abs() <= 2,
                        "{q:?} {r:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn polygons_simplify_is_douglas_peucker_anchored_at_the_first_vertex() {
    // polygons_simplify (Polygon.cpp): Douglas-Peucker on the ring closed by repeating vertex 0,
    // so vertex 0 survives even when it lies on a straight edge; Clipper2's SimplifyPath would
    // drop it. Points within the tolerance elsewhere go, the hole stays a clockwise hole.
    let p = |x: f64, y: f64| Point::new(scaled(x), scaled(y));
    let ex = ExPolygon {
        contour: vec![
            p(5., 0.005),
            p(10., 0.),
            p(10., 5.),
            p(10.005, 7.),
            p(10., 10.),
            p(0., 10.),
            p(0., 0.),
        ],
        holes: vec![vec![
            p(3., 3.),
            p(3., 7.),
            p(5., 7.002),
            p(7., 7.),
            p(7., 3.),
        ]],
    };
    let out = polygons_simplify(&[ex], scaled(0.015) as f64);
    assert_eq!(out.len(), 1);
    let contour = &out[0].contour;
    assert!(contour.contains(&p(5., 0.005)), "{contour:?}");
    assert!(!contour.contains(&p(10., 5.)) && !contour.contains(&p(10.005, 7.)));
    assert_eq!(contour.len(), 5);
    assert_eq!(out[0].holes.len(), 1);
    let hole = &out[0].holes[0];
    assert!(crate::geometry::signed_area(hole) < 0.);
    assert!(!hole.contains(&p(5., 7.002)));
    assert_eq!(hole.len(), 4);
}

#[test]
fn round_offsets_use_clipper64_arc_tolerance_per_path() {
    // ClipperUtils::offset(Polygons, delta, jtRound, arc_tolerance): Clipper 6.4 rounds
    // m_StepsPerRad * angle per corner, steps = pi / acos(1 - arc / delta). For 1 mm and
    // 0.01 mm: 3.533 steps per radian, Round(5.55) = 6 steps, 7 vertices per right angle.
    // (A tiny square would vanish: ShortestEdgeLength = 0.005 * delta drops its vertices.)
    let p = |x: i64, y: i64| Point::new(x, y);
    let mm = scaled(1.);
    let square = ExPolygon {
        contour: vec![p(0, 0), p(mm, 0), p(mm, mm), p(0, mm)],
        holes: vec![],
    };
    let out = crate::geometry::clip64::offset_polygons_ex(
        &[square],
        scaled(1.) as f64,
        clip::Join::Round,
        scaled(0.01) as f64,
    );
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].contour.len(), 28, "{:?}", out[0].contour);
    // A hole is offset as its own clockwise path, so it shrinks.
    let s = |v: f64| scaled(v);
    let frame = ExPolygon {
        contour: vec![p(0, 0), p(s(10.), 0), p(s(10.), s(10.)), p(0, s(10.))],
        holes: vec![vec![
            p(s(2.), s(2.)),
            p(s(2.), s(8.)),
            p(s(8.), s(8.)),
            p(s(8.), s(2.)),
        ]],
    };
    let grown =
        crate::geometry::clip64::offset_polygons_ex(&[frame], s(1.) as f64, clip::Join::Miter, 1.2);
    assert_eq!(grown.len(), 1);
    assert_eq!(grown[0].holes.len(), 1);
    let hole = crate::geometry::BoundingBox::from_points(&grown[0].holes[0]).unwrap();
    assert_eq!((hole.min.x, hole.max.x), (s(3.), s(7.)));
}
