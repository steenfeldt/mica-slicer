// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/Support/TreeSupport.cpp :: generate_initial_areas, create_layer_pathing, create_nodes_from_area
// Source: orca@8500fcdcca src/libslic3r/Support/TreeSupport3D.cpp :: generate_initial_areas, create_layer_pathing, create_nodes_from_area
// Changes: Analytical synthetic fixtures, documented upstream formula derivations, deterministic graph and rule-switch assertions.
// Copyright (c) Prusa Research 2022 - 2023 Vojtěch Bubník @bubnikv, Lukáš Matěna @lukasmatena, Tomáš Mészáros @tamasmeszaros
// PrusaSlicer is released under the terms of the AGPLv3 or higher.
// Tree supports by Thomas Rahm, losely based on Tree Supports by CuraEngine.
// Original source of Thomas Rahm's tree supports: https://github.com/ThomasRahm/CuraEngine
// Copyright (c) 2021 Ultimaker B.V.
// CuraEngine is released under the terms of the AGPLv3 or higher.

use super::{
    geometry::{area, bbox, circle, contains, move_inside_if_outside},
    *,
};
use crate::{
    geometry::{ExPolygon, Point, clip},
    rules::Rules,
};

fn rect(x: f64, y: f64, w: f64, h: f64) -> Areas {
    vec![ExPolygon {
        contour: [(x, y), (x + w, y), (x + w, y + h), (x, y + h)]
            .into_iter()
            .map(|(x, y)| Point::new(scaled(x), scaled(y)))
            .collect(),
        holes: vec![],
    }]
}
fn config(rules: Rules) -> TreeSupportSettings {
    let m = TreeSupportMeshGroupSettings {
        rules,
        layer_height: scaled(0.2),
        support_xy_distance: scaled(0.1),
        support_xy_distance_overhang: scaled(0.1),
        support_top_distance: 0,
        support_bottom_distance: 0,
        support_roof_layers: 0,
        support_tree_branch_diameter: scaled(0.4),
        support_tree_tip_diameter: scaled(0.4),
        resolution: 0,
        ..Default::default()
    };
    let mut c = TreeSupportSettings::new(&m, &TreeSlicingParameters::new(0.2, 0.2), false);
    c.tip_layers = 1;
    c.branch_radius_increase_per_layer = 0.;
    c.layer_start_bp_radius = 0;
    c.maximum_move_distance = scaled(0.1);
    c.maximum_move_distance_slow = scaled(0.1);
    c.increase_radius_until_radius = c.min_radius;
    c.increase_radius_until_layer = 1;
    c
}
fn empty_volumes(c: &TreeSupportSettings, n: usize) -> TreeModelVolumes {
    TreeModelVolumes::new(c.clone(), vec![vec![]; n], vec![], vec![])
}
fn tip(layer: usize, x: f64, y: f64) -> SupportElement {
    let p = Point::new(scaled(x), scaled(y));
    SupportElement {
        state: SupportElementState {
            to_buildplate: true,
            to_model_gracious: true,
            can_use_safe_radius: true,
            target_height: layer as i32,
            layer_idx: layer as i32,
            target_position: p,
            next_position: p,
            result_on_layer: Some(p),
            ..Default::default()
        },
        parents: vec![],
        influence_area: circle(p, scaled(0.01)),
    }
}
fn verify_graph(bounds: &[SupportElements]) {
    for (l, layer) in bounds.iter().enumerate() {
        for e in layer {
            assert_eq!(e.state.layer_idx, l as i32);
            assert!(!e.state.deleted);
            assert!(e.state.result_on_layer.is_some());
            for &p in &e.parents {
                assert!(p >= 0);
                assert!(bounds[l + 1][p as usize].state.result_on_layer.is_some());
            }
        }
    }
}
#[test]
fn upstream_polyline_samples_endpoints_midpoint_and_closed_extrema() {
    let line = vec![Point::from_mm(0., 0.), Point::from_mm(10., 0.)];
    // Long open lines retain both ends, then samples 2.5, 5, 7.5mm.
    let out = ensure_maximum_distance_polyline(&[line], scaled(2.5) as f64, 1);
    assert_eq!(
        out[0],
        vec![0., 10., 2.5, 5., 7.5]
            .into_iter()
            .map(|x| Point::from_mm(x, 0.))
            .collect::<Vec<_>>()
    );
    // L<2*d and min_points<=1 produces the arc-length midpoint.
    let line = vec![
        Point::from_mm(0., 0.),
        Point::from_mm(1., 0.),
        Point::from_mm(1., 1.),
    ];
    assert_eq!(
        ensure_maximum_distance_polyline(&[line], scaled(2.) as f64, 1),
        vec![vec![Point::from_mm(1., 0.)]]
    );
    let closed = vec![
        Point::from_mm(0., 0.),
        Point::from_mm(4., 0.),
        Point::from_mm(4., 0.1),
        Point::from_mm(0., 0.1),
        Point::from_mm(0., 0.),
    ];
    let out = ensure_maximum_distance_polyline(&[closed], scaled(3.) as f64, 2);
    assert_eq!(out[0][0], Point::from_mm(0., 0.));
    assert_eq!(out[0][1], Point::from_mm(4., 0.1));
}
#[test]
fn tips_on_t_arm_hanging_blob_and_ledge_have_derived_counts() {
    for rules in [Rules::PrusaSlicer, Rules::OrcaSlicer] {
        let c = config(rules);
        let p = TreeSupportInfill::new(&c);
        // width .4, rate15 => connect_length=2.666666mm. The thin arm
        // (.2mm) cannot contain a flow line (.357mm spacing). Collision splits
        // it into two <2*connect_length boundaries, each sampled at its midpoint.
        let mut outlines = vec![rect(-0.2, -0.2, 0.4, 0.4); 22];
        outlines[21] = rect(-2., -0.1, 4., 0.2);
        let mut overhangs = vec![vec![]; 22];
        overhangs[21] = outlines[21].clone();
        let mut v = TreeModelVolumes::new(c.clone(), outlines, vec![], vec![]);
        let initial = generate_initial_areas(&mut v, &c, &overhangs, &p, None);
        assert_eq!(initial.move_bounds[20].len(), 2, "{rules:?}");
        assert_eq!(
            initial.move_bounds.iter().map(|l| l.len()).sum::<usize>(),
            2
        );
        // Tiny blob and ledge each have perimeter below 2*connect_length and
        // min_support_points=clamp(floor(perimeter/connect_length),1,3)=1.
        for overhang in [
            circle(Point::from_mm(4., 0.), scaled(0.07)),
            rect(4., 0., 0.15, 0.15),
        ] {
            let mut overhangs = vec![vec![]; 22];
            overhangs[21] = overhang;
            let mut v = empty_volumes(&c, 22);
            let initial = generate_initial_areas(&mut v, &c, &overhangs, &p, None);
            assert_eq!(initial.move_bounds[20].len(), 1);
            let full = create_tree_pathing(&mut v, &c, &overhangs, &p, None);
            assert_eq!(full.move_bounds[0].len(), 1);
            verify_graph(&full.move_bounds);
        }
    }
}
#[test]
fn nearby_tips_merge_on_sixth_layer_and_keep_full_reachable_area() {
    for rules in [Rules::PrusaSlicer, Rules::OrcaSlicer] {
        let c = config(rules);
        let v = empty_volumes(&c, 21);
        let mut bounds = vec![SupportElements::new(); 21];
        bounds[20].extend([tip(20, -0.5, 0.), tip(20, 0.5, 0.)]);
        create_layer_pathing(&v, &c, &mut bounds);
        // Constant radius => extra_speed=5 scaled units. Each seed grows from
        // .01mm by .100005mm/layer. At n=5 overlap width=.02005mm,
        // erased by upstream's -.025mm merge guard. At n=6 it is .22006mm.
        assert_eq!(bounds[15].len(), 2);
        assert_eq!(bounds[14].len(), 1);
        assert_eq!(bounds[14][0].parents.len(), 2);
        assert_eq!(bounds[0].len(), 1);
        assert!(bbox(&bounds[0][0].influence_area).unwrap().max.y > scaled(1.));
        create_nodes_from_area(&v, &c, &mut bounds);
        verify_graph(&bounds);
        let again = {
            let v = empty_volumes(&c, 21);
            let mut b = vec![SupportElements::new(); 21];
            b[20].extend([tip(20, -0.5, 0.), tip(20, 0.5, 0.)]);
            create_layer_pathing(&v, &c, &mut b);
            create_nodes_from_area(&v, &c, &mut b);
            b
        };
        assert_eq!(bounds, again);
    }
}
#[test]
fn model_landing_vs_build_plate_and_small_branch_culling() {
    for rules in [Rules::PrusaSlicer, Rules::OrcaSlicer] {
        let mut c = config(rules);
        c.min_dtt_to_model = 2;
        let mut outlines = vec![vec![]; 21];
        for layer in &mut outlines[..4] {
            *layer = rect(-10., -10., 20., 20.);
        }
        let mut v = TreeModelVolumes::new(c.clone(), outlines.clone(), vec![], vec![]);
        let status = get_avoidance_status(&mut v, &c, Point::default(), c.min_radius, 20);
        assert!(matches!(
            status,
            LineStatus::ToModelGracious | LineStatus::ToModelGraciousSafe
        ));
        let mut b = vec![SupportElements::new(); 21];
        let mut roofs = TreeRoofs::new(21);
        let p = TreeSupportInfill::new(&c);
        RichInterfacePlacer::new(&mut v, &c, &p, false, &mut b, &mut roofs).add_points_along_lines(
            vec![vec![(Point::default(), status)]],
            20,
            0,
            0,
            0,
        );
        create_layer_pathing(&v, &c, &mut b);
        create_nodes_from_area(&v, &c, &mut b);
        // With bottom gap0, placeables start immediately above the platform:
        // layer4 = outline[3] - outline[4], shrunk by radius+XY.
        assert!(b[..4].iter().all(|l| l.is_empty()));
        assert_eq!(b[4].len(), 1);
        // increase_areas_one_layer clears gracious when layer3 is blocked.
        assert!(!b[4][0].state.to_model_gracious);
        assert!(!b[4][0].state.to_buildplate);
        verify_graph(&b);
        // A buildplate-only tree at the same central position has no route
        // through the wide platform and thus produces no valid tips.
        c.support_rests_on_model = false;
        let mut v = TreeModelVolumes::new(c.clone(), outlines, vec![], vec![]);
        assert_eq!(
            get_avoidance_status(&mut v, &c, Point::default(), c.min_radius, 20),
            LineStatus::Invalid
        );
        let mut b = vec![SupportElements::new(); 3];
        let mut short = tip(2, 0., 0.);
        short.state.to_buildplate = false;
        short.state.to_model_gracious = false;
        short.state.result_on_layer = None;
        short.state.distance_to_top = 1;
        b[2].push_back(short);
        create_nodes_from_area(&v, &c, &mut b);
        assert!(b.iter().all(|l| l.is_empty()));
    }
}
#[test]
fn locked_tips_release_and_attempt_order_matches_upstream() {
    let c = config(Rules::PrusaSlicer);
    let v = empty_volumes(&c, 10);
    let mut t = tip(9, 0., 0.);
    t.state.dont_move_until = 4;
    let order = area_increase_order(&v, &c, &t, 9, true, 100, 200);
    assert_eq!(
        order[0],
        AreaIncreaseSettings {
            avoidance_type: AvoidanceType::Slow,
            increase_speed: 0,
            increase_radius: true,
            no_error: true,
            use_min_distance: false,
            move_area: false
        }
    );
    let mut b = vec![SupportElements::new(); 10];
    b[9].push_back(t);
    create_layer_pathing(&v, &c, &mut b);
    for layer in &b[6..9] {
        assert!(!layer[0].state.last_area_increase.move_area);
        assert_eq!(layer[0].state.result_on_layer, Some(Point::default()));
    }
    assert!(b[5][0].state.last_area_increase.move_area);
    assert_eq!(b[5][0].state.dont_move_until, 0);
    let mut hole_tip = tip(9, 0., 0.);
    hole_tip.state.can_use_safe_radius = false;
    hole_tip.state.use_min_xy_dist = true;
    let order = area_increase_order(&v, &c, &hole_tip, 9, true, 100, 200);
    assert_eq!(order[0].avoidance_type, AvoidanceType::Slow);
    assert!(!order[0].move_area);
    assert!(!order[0].use_min_distance);
    assert!(order[1].use_min_distance);
    assert_eq!(order.last().unwrap().avoidance_type, AvoidanceType::Fast);
    assert!(!order.last().unwrap().no_error);
}
#[test]
fn radius_growth_and_fractional_elephant_foot_follow_formulas() {
    let mut c = config(Rules::PrusaSlicer);
    c.branch_radius = scaled(0.6);
    c.tip_layers = 4;
    c.increase_radius_until_layer = 4;
    c.increase_radius_until_radius = scaled(0.6);
    c.branch_radius_increase_per_layer = scaled(0.02) as f64;
    let v = empty_volumes(&c, 15);
    let mut b = vec![SupportElements::new(); 15];
    b[14].push_back(tip(14, 0., 0.));
    create_layer_pathing(&v, &c, &mut b);
    // getRadius(d)=.2+(.6-.2)*d/4 through tip, then .6+(d-4)*.02.
    for d in 0..=14 {
        let expected = if d <= 4 {
            scaled(0.2) + scaled(0.4) * d / 4
        } else {
            scaled(0.6) + scaled(0.02) * (d - 4)
        };
        assert_eq!(b[14 - d as usize][0].radius(&c), expected);
    }
    // Foot growth contribution is (bp_growth-branch_growth)*foot count.
    c.bp_radius_increase_per_layer = scaled(0.12) as f64;
    let first = SupportElementState {
        effective_radius_height: 5,
        distance_to_top: 5,
        elephant_foot_increases: 1.5,
        ..Default::default()
    };
    let second = SupportElementState {
        effective_radius_height: 8,
        distance_to_top: 8,
        ..Default::default()
    };
    let merged = merge_support_element_states(&c, &first, &second, Point::default(), 0);
    assert_eq!(c.get_collision_radius(&first), scaled(0.77));
    assert_eq!(c.get_collision_radius(&second), scaled(0.68));
    assert!((merged.elephant_foot_increases - 0.9).abs() < 1e-9);
    assert_eq!(c.get_collision_radius(&merged), scaled(0.77));
}
#[test]
fn roofs_continuous_tips_and_orca_recovery_switches() {
    for rules in [Rules::PrusaSlicer, Rules::OrcaSlicer] {
        let mut c = config(rules);
        c.settings.support_roof_layers = 3;
        let mut p = TreeSupportInfill::new(&c);
        let mut v = empty_volumes(&c, 10);
        let mut overhangs = vec![vec![]; 10];
        overhangs[9] = rect(2., 2., 5., 5.);
        let out = generate_initial_areas(&mut v, &c, &overhangs, &p, None);
        // Three unobstructed roofs at layers8,7,6, tips at5.
        assert!(!out.roofs.top_contacts[8].is_empty());
        assert!(!out.roofs.top_interfaces[7].is_empty());
        assert!(!out.roofs.top_interfaces[6].is_empty());
        assert!(!out.move_bounds[5].is_empty());
        assert!(
            out.move_bounds[5]
                .iter()
                .all(|e| e.state.supports_roof && e.state.dont_move_until == 0)
        );
        let mut bounds = vec![SupportElements::new(); 10];
        let mut roofs = TreeRoofs::new(10);
        // Seven sampled points on a simulated support line disable ovalisation.
        let line = (0..7)
            .map(|i| (Point::from_mm(i as f64, 0.), LineStatus::ToBpSafe))
            .collect();
        RichInterfacePlacer::new(&mut v, &c, &p, false, &mut bounds, &mut roofs)
            .add_points_along_lines(vec![line], 8, 0, 0, 3);
        assert_eq!(bounds[8].len(), 7);
        assert!(bounds[8].iter().all(|e| e.state.skip_ovalisation));
        let a = SupportElementState {
            missing_roof_layers: 2,
            orca: OrcaElementMetadata {
                roof_recovery_dtt: 4,
                ..Default::default()
            },
            ..Default::default()
        };
        let b = SupportElementState {
            missing_roof_layers: 5,
            orca: OrcaElementMetadata {
                roof_recovery_dtt: 2,
                ..Default::default()
            },
            ..Default::default()
        };
        let merged = merge_support_element_states(&c, &a, &b, Point::default(), 0);
        assert_eq!(
            merged.missing_roof_layers,
            if rules.is_orca() { 5 } else { 2 }
        );
        assert_eq!(
            merged.orca.roof_recovery_dtt,
            if rules.is_orca() { 4 } else { 0 }
        );
        // Explicitly distinguish Prusa density from Orca's top density.
        p.interface_density = 0.2;
        p.top_interface_density = 1.;
        let lines = generate_support_infill_lines(&rect(0., 0., 10., 10.), &c, &p, true, 2, 0);
        assert!(!lines.is_empty());
    }
}
#[test]
fn safe_offsets_do_not_tunnel_and_projection_respects_holes() {
    let ring = clip::diff_ex(&rect(-2., -2., 4., 4.), &rect(-1., -1., 2., 2.));
    let grown = safe_offset_inc(
        &rect(-0.1, -0.1, 0.2, 0.2),
        scaled(4.),
        &ring,
        scaled(0.25),
        0,
        2,
    );
    assert!(bbox(&grown).unwrap().max.x <= scaled(1.));
    assert!(area(&grown) > 0.);
    let inside = move_inside_if_outside(&ring, Point::default());
    assert!(contains(&ring, inside));
    assert_eq!(inside.distance_to(Point::default()), scaled(1.) as f64);
    assert_eq!(
        safe_offset_inc(&rect(0., 0., 1., 1.), 0, &[], 10, 0, 1),
        clip::union_ex(&rect(0., 0., 1., 1.))
    );
}
#[test]
fn deleted_elements_remap_swapped_parents() {
    let mut b = vec![SupportElements::new(); 4];
    b[3].extend([tip(3, 0., 0.), tip(3, 1., 0.), tip(3, 2., 0.)]);
    b[3][0].state.deleted = true;
    let mut e = tip(2, 0., 0.);
    e.parents = vec![2, 1];
    b[2].push_back(e);
    remove_deleted_elements(&mut b);
    assert_eq!(b[3].len(), 2);
    assert_eq!(b[2][0].parents, vec![0, 1]);
}

#[test]
fn gracious_contact_walks_to_highest_placeable_parent() {
    let c = config(Rules::PrusaSlicer);
    let outlines = vec![
        rect(0., 0., 10., 10.),
        rect(0., 0., 8., 8.),
        rect(0., 0., 6., 6.),
        rect(0., 0., 4., 4.),
    ];
    let v = TreeModelVolumes::new(c.clone(), outlines, vec![], vec![]);
    let mut b = vec![SupportElements::new(); 4];
    // Each shrinking ledge leaves a different landing strip. The walk tests
    // the entire element, climbs single-parent chains and stops at a merge/tip.
    for (layer, x) in [(1, 9.), (2, 7.), (3, 5.)] {
        let mut e = tip(layer, x, 1.);
        e.state.to_buildplate = false;
        e.state.result_on_layer = None;
        if layer < 3 {
            e.parents = vec![0];
        }
        b[layer].push_back(e);
    }
    set_to_model_contact_to_model_gracious(&v, &c, &mut b, 1, 0);
    assert!(b[1][0].state.deleted);
    assert!(b[2][0].state.deleted);
    assert_eq!(b[3][0].state.result_on_layer, Some(Point::from_mm(5., 1.)));
    remove_deleted_elements(&mut b);
    assert!(b[1].is_empty() && b[2].is_empty());
    assert_eq!(b[3].len(), 1);
}

#[test]
fn catch_up_uses_collision_buckets_without_exceeding_dtt() {
    let mut c = config(Rules::PrusaSlicer);
    c.branch_radius = scaled(0.6);
    c.tip_layers = 4;
    c.increase_radius_until_layer = 4;
    c.increase_radius_until_radius = scaled(0.6);
    let v = empty_volumes(&c, 10);
    let mut parent = tip(9, 0., 0.);
    parent.state.distance_to_top = 3;
    parent.state.effective_radius_height = 0;
    let s = AreaIncreaseSettings {
        increase_radius: true,
        no_error: true,
        avoidance_type: AvoidanceType::Slow,
        ..Default::default()
    };
    let result = increase_single_area(&v, &c, s, 9, &parent, &[], 0, true).unwrap();
    // .2mm min radius, .5mm collision bucket step => .3mm rounds to .7.
    // Catch-up can fit .4 and .5 in that bucket, but upstream's condition
    // resulting_eff_dtt+1<distance_to_top stops at 3 when DTT=4.
    assert_eq!(result.state.distance_to_top, 4);
    assert_eq!(result.state.effective_radius_height, 3);
    assert_eq!(c.get_collision_radius(&result.state), scaled(0.5));
    assert_eq!(c.get_radius_state(&result.state), scaled(0.6));
}
#[test]
fn merging_preserves_full_area_and_rejects_incompatible_states() {
    let c = config(Rules::PrusaSlicer);
    let v = empty_volumes(&c, 10);
    let candidate = || SupportElementMerging {
        state: tip(8, 2., 2.).state,
        parents: vec![0],
        areas: SupportElementInfluenceAreas {
            influence_areas: rect(0., 0., 10., 10.),
            to_bp_areas: rect(1., 1., 2., 2.),
            to_model_areas: rect(1., 1., 2., 2.),
        },
    };
    let mut dst = candidate();
    let mut src = candidate();
    src.parents = vec![1];
    assert!(merge_influence_areas_two_elements(
        &v, &c, 9, &mut dst, &mut src
    ));
    assert_eq!(dst.parents, vec![0, 1]);
    assert!(src.parents.is_empty());
    // Reachable center positions cover100mm²; avoidance currently allows only
    // 4mm² to the bed. Retaining only that subset would lose future routes.
    assert_eq!(
        area(&dst.areas.influence_areas),
        100. * crate::geometry::SCALE.powi(2)
    );
    assert_eq!(
        area(&dst.areas.to_bp_areas),
        4. * crate::geometry::SCALE.powi(2)
    );
    let mut dst = candidate();
    let mut src = candidate();
    src.state.to_model_gracious = false;
    assert!(!merge_influence_areas_two_elements(
        &v, &c, 9, &mut dst, &mut src
    ));
    src = candidate();
    src.state.use_min_xy_dist = true;
    assert!(!merge_influence_areas_two_elements(
        &v, &c, 9, &mut dst, &mut src
    ));
}
#[test]
fn deterministic_spatial_buckets_and_orca_zero_clearance() {
    let mut c = config(Rules::OrcaSlicer);
    c.xy_min_distance = 0;
    c.xy_distance = 0;
    let run = |reverse_cache| {
        let v = empty_volumes(&c, 16);
        if reverse_cache {
            for layer in (0..16).rev() {
                let _ = v.get_collision(c.min_radius, layer, false);
            }
        }
        let mut b = vec![SupportElements::new(); 16];
        for y in 0..3 {
            for x in 0..3 {
                b[15].push_back(tip(15, x as f64 * 0.5, y as f64 * 0.5));
            }
        }
        create_layer_pathing(&v, &c, &mut b);
        create_nodes_from_area(&v, &c, &mut b);
        verify_graph(&b);
        b
    };
    let first = run(false);
    assert_eq!(first[0].len(), 1);
    assert_eq!(first, run(true));
    assert_eq!(first, run(false));
}

#[test]
fn continuous_tips_and_failed_tip_roofs_retain_recovery_depth() {
    for rules in [Rules::PrusaSlicer, Rules::OrcaSlicer] {
        let mut c = config(rules);
        c.settings.support_roof_layers = 3;
        c.settings.minimum_roof_area = 100. * crate::geometry::SCALE.powi(2);
        let p = TreeSupportInfill::new(&c);
        let mut v = empty_volumes(&c, 10);
        let mut bounds = vec![SupportElements::new(); 10];
        let mut roofs = TreeRoofs::new(10);
        // 25mm² <100mm² minimum roof: no interface fits, so continuous tips
        // use min_radius/2=.1mm rather than connect_length=2.666mm.
        let mut placer = RichInterfacePlacer::new(&mut v, &c, &p, false, &mut bounds, &mut roofs);
        sample_overhang_area(
            rect(0., 0., 5., 5.),
            true,
            8,
            3,
            scaled(0.4 * 100. / 15.),
            &c.settings,
            &mut placer,
        );
        assert!(roofs.top_contacts.iter().all(Vec::is_empty));
        assert!(bounds[8].len() > 20);
        assert!(bounds[8].iter().any(|e| e.state.skip_ovalisation));
        assert!(bounds[8].iter().all(|e| e.state.dont_move_until == 3));
        let mut outlines = vec![vec![]; 10];
        outlines[7] = rect(-10., -10., 20., 20.);
        let mut v = TreeModelVolumes::new(c.clone(), outlines, vec![], vec![]);
        let mut bounds = vec![SupportElements::new(); 10];
        let mut roofs = TreeRoofs::new(10);
        RichInterfacePlacer::new(&mut v, &c, &p, true, &mut bounds, &mut roofs)
            .add_points_along_lines(
                vec![vec![(Point::default(), LineStatus::ToBpSafe)]],
                8,
                3,
                2,
                3,
            );
        // Collision on7 makes the point invalid one layer below. It is
        // released immediately at8, with all3 requested tip roofs pending.
        assert_eq!(bounds[8].len(), 1);
        let e = &bounds[8][0].state;
        assert_eq!(e.missing_roof_layers, 3);
        assert_eq!(e.dont_move_until, 3);
        assert_eq!(
            e.orca.roof_recovery_dtt,
            if rules.is_orca() { 2 } else { 0 }
        );
    }
}

#[test]
fn cancellation_reaches_all_pathing_stages() {
    let c = config(Rules::PrusaSlicer);
    let p = TreeSupportInfill::new(&c);
    let mut v = empty_volumes(&c, 10);
    let mut overhangs = vec![vec![]; 10];
    overhangs[9] = rect(2., 2., 0.1, 0.1);
    assert_eq!(
        generate_initial_areas_with_cancel(&mut v, &c, &overhangs, &p, None, || Err("tips")),
        Err("tips")
    );
    let mut b = vec![SupportElements::new(); 10];
    b[9].push_back(tip(9, 0., 0.));
    assert_eq!(
        create_layer_pathing_with_cancel(&v, &c, &mut b, || Err("spread")),
        Err("spread")
    );
    create_layer_pathing(&v, &c, &mut b);
    assert_eq!(
        create_nodes_from_area_with_cancel(&v, &c, &mut b, || Err("nodes")),
        Err("nodes")
    );
    assert_eq!(
        create_tree_pathing_with_cancel(&mut v, &c, &overhangs, &p, None, || Ok::<_, &str>(())),
        Ok(create_tree_pathing(&mut v, &c, &overhangs, &p, None))
    );
}

#[test]
fn top_density_and_interface_classification_use_rule_switches() {
    let mut c = config(Rules::PrusaSlicer);
    c.settings.support_roof_layers = 4;
    let mut p = TreeSupportInfill::new(&c);
    p.interface_density = 0.2;
    p.top_interface_density = 1.;
    let polygon = rect(0., 0., 10., 10.);
    let length = |c: &TreeSupportSettings| {
        generate_support_infill_lines(&polygon, c, &p, true, 0, 0)
            .iter()
            .map(|l| crate::geometry::polyline_length(l))
            .sum::<f64>()
    };
    let prusa = length(&c);
    c.settings.rules = Rules::OrcaSlicer;
    // Five times density; inset/turns perturb the finite-square line length.
    assert!(length(&c) > 3. * prusa);
    p.num_top_interface_layers = 3;
    p.num_top_base_interface_layers = 1;
    for rules in [Rules::PrusaSlicer, Rules::OrcaSlicer] {
        c.settings.rules = rules;
        let mut v = empty_volumes(&c, 8);
        let mut b = vec![SupportElements::new(); 8];
        let mut roofs = TreeRoofs::new(8);
        RichInterfacePlacer::new(&mut v, &c, &p, true, &mut b, &mut roofs).add_points_along_lines(
            vec![vec![(Point::default(), LineStatus::ToBpSafe)]],
            7,
            1,
            2,
            0,
        );
        // DTT2: Prusa threshold3-1=2; Orca reserves one => threshold1.
        assert_eq!(!roofs.top_interfaces[7].is_empty(), !rules.is_orca());
        assert_eq!(!roofs.top_base_interfaces[7].is_empty(), rules.is_orca());
    }
}
