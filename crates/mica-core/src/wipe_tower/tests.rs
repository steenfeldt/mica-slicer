// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 tests/fff_print/test_multi.cpp :: Basic tests, Ooze prevention
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/WipeTower.cpp :: plan_toolchange, extract_wipe_volumes, generate
// Changes: Nonconsecutive physical tools, sparse tower and numerical planning regressions. Upstream has no dedicated test_wipe_tower file at this pin.
// Changes: Wipe paths (toolchange_Wipe, finish_layer) checked against PrusaSlicer 2.9.6 XL output.
// Source: orca@8500fcdcca src/libslic3r/GCode/WipeTower2.cpp :: merge_tcr, WipeTower2::construct_tcr
// Changes: Joined finish index and toolchange entry (tool_change_start_pos) of visits.

use super::*;
fn config() -> Config {
    let mut c = Config::defaults();
    for (k, v) in [
        ("bed_shape", "0x0,360x0,360x360,0x360"),
        ("single_extruder_multi_material_priming", "0"),
        ("nozzle_diameter", "0.4,0.4,0.4,0.4,0.4"),
        ("wipe_tower", "1"),
        ("wipe_tower_width", "60"),
        ("wipe_tower_brim_width", "3"),
        ("wipe_tower_cone_angle", "25"),
        ("wipe_tower_extra_spacing", "150%"),
        ("filament_multitool_ramming", "1,1,1,1,1"),
        ("filament_multitool_ramming_flow", "40,40,40,40,40"),
        ("filament_multitool_ramming_volume", "10,10,10,10,10"),
        ("filament_minimal_purge_on_wipe_tower", "15,15,15,15,15"),
    ] {
        c.set_raw(k, v).unwrap();
    }
    c
}
fn order(tools: Vec<Vec<usize>>) -> ToolOrdering {
    ToolOrdering::new(
        tools
            .into_iter()
            .enumerate()
            .map(|(i, ts)| ((i + 1) as f64 * 0.2, ts.into_iter().map(Some).collect()))
            .collect(),
        true,
    )
}
#[test]
fn xl_profile_depth_and_nonconsecutive_tools() {
    let p = TowerPlan::new(&config(), &order(vec![vec![0, 4]; 100]))
        .unwrap()
        .unwrap();
    assert_eq!(p.layers.len(), 100);
    assert!((p.depth - 4.25).abs() < 1e-6);
    for (i, l) in p.layers.iter().enumerate() {
        assert_eq!(l.changes.len(), 1);
        let visits = p.visits(i);
        assert_eq!(visits.len(), 1);
        let ram: f64 = visits[0]
            .before
            .as_ref()
            .unwrap()
            .moves
            .iter()
            .filter(|m| m.ramming)
            .map(|m| m.volume)
            .sum();
        assert!((ram - 10.).abs() < 1e-8);
        assert!(visits[0].after.moves.iter().map(|m| m.volume).sum::<f64>() >= 15.);
    }
}
#[test]
fn independent_tools_ignore_color_matrix_and_single_tool_has_no_tower() {
    let mut c = config();
    let o = order(vec![vec![1, 3]; 3]);
    let a = TowerPlan::new(&c, &o).unwrap().unwrap();
    c.set_raw("wiping_volumes_matrix", "9999").unwrap();
    let b = TowerPlan::new(&c, &o).unwrap().unwrap();
    assert_eq!(a.depth, b.depth);
    assert!(
        TowerPlan::new(&c, &order(vec![vec![3]; 5]))
            .unwrap()
            .is_none()
    );
}
#[test]
fn sparse_layers_support_upper_changes_and_no_sparse_compresses_z() {
    let o = order(vec![vec![0, 1], vec![1], vec![1], vec![0, 1], vec![0]]);
    let mut c = config();
    let a = TowerPlan::new(&c, &o).unwrap().unwrap();
    assert_eq!(a.layers.len(), 4);
    assert!(!a.visits(1).is_empty());
    c.set_raw("wipe_tower_no_sparse_layers", "1").unwrap();
    let b = TowerPlan::new(&c, &o).unwrap().unwrap();
    assert!(b.visits(1).is_empty());
    assert!(b.visits(2).is_empty());
    assert!((b.visits(3)[0].z - 0.4).abs() < 1e-6);
}
#[test]
fn rotation_translates_all_moves_and_footprint() {
    let mut c = config();
    let o = order(vec![vec![0, 1]; 3]);
    let a = TowerPlan::new(&c, &o).unwrap().unwrap();
    c.set_raw("wipe_tower_x", "50").unwrap();
    c.set_raw("wipe_tower_y", "60").unwrap();
    c.set_raw("wipe_tower_rotation_angle", "90").unwrap();
    let b = TowerPlan::new(&c, &o).unwrap().unwrap();
    let av = a.visits(0);
    let bv = b.visits(0);
    for (a, b) in av[0].after.moves.iter().zip(&bv[0].after.moves) {
        assert!((b.xy[0] - (50. - (a.xy[1] - 140.))).abs() < 1e-6);
        assert!((b.xy[1] - (60. + a.xy[0] - 180.)).abs() < 1e-6);
        assert_eq!(a.volume, b.volume);
    }
    assert!(b.first_layer_bbox().unwrap().min.x > 0);
}
#[test]
fn invalid_parameters_are_errors() {
    for (k, v) in [
        ("wipe_tower_width", "1"),
        ("wipe_tower_extra_spacing", "0%"),
        ("wipe_tower_x", "NaN"),
        ("wipe_tower_cone_angle", "180"),
        ("wipe_tower_bridging", "0"),
        ("nozzle_diameter", "0.4,0.6"),
    ] {
        let mut c = config();
        c.set_raw(k, v).unwrap();
        assert!(
            TowerPlan::new(&c, &order(vec![vec![0, 1]; 3])).is_err(),
            "{k}"
        );
    }
}
/// GUI monkey test, seed 605: a brim wider than the bed is refused at once, by the plan the
/// GUI's tower estimate uses too, instead of being built (5 GB, a minute) and then refused.
#[test]
fn a_brim_wider_than_the_bed_is_refused_before_any_geometry() {
    let mut c = config();
    c.set_raw("wipe_tower_brim_width", "1000000").unwrap();
    let started = std::time::Instant::now();
    let e = TowerPlan::plan(&c, &order(vec![vec![0, 1]; 3])).unwrap_err();
    assert!(e.contains("wider than the print bed"), "{e}");
    assert!(started.elapsed().as_secs() < 2);
    c.set_raw("wipe_tower_brim_width", "50").unwrap();
    assert!(TowerPlan::plan(&c, &order(vec![vec![0, 1]; 3])).is_ok());
}

#[test]
fn support_only_layers_neither_thin_nor_deepen_the_tower() {
    // Object layers every 0.2 mm alternate two tools; independent support layers sit 0.05 mm
    // below each object layer and keep the current tool (support tool "don't care").
    let objects: Vec<(f64, Vec<Option<usize>>)> = (1..=20)
        .map(|i| (i as f64 * 0.2, vec![Some(0), Some(1)]))
        .collect();
    let mut mixed = Vec::new();
    for (z, tools) in &objects {
        if *z > 0.3 {
            mixed.push((z - 0.05, vec![None]));
        }
        mixed.push((*z, tools.clone()));
    }
    let plain = ToolOrdering::new(objects, true);
    let mut interleaved = ToolOrdering::new(mixed, true);
    for l in &mut interleaved.layers {
        l.has_object = ((l.print_z / 0.2).round() * 0.2 - l.print_z).abs() < 1e-9;
    }
    let has = interleaved.tower_layers(0.3);
    assert_eq!(has.iter().filter(|h| **h).count(), 20);
    let c = config();
    let a = TowerPlan::new(&c, &plain).unwrap().unwrap();
    let b = TowerPlan::new(&c, &interleaved).unwrap().unwrap();
    assert_eq!(a.layers.len(), b.layers.len());
    assert!(
        (a.depth - b.depth).abs() < 1e-9,
        "{} vs {}",
        a.depth,
        b.depth
    );
    for (la, lb) in a.layers.iter().zip(&b.layers) {
        assert!((la.height - lb.height).abs() < 1e-9);
        assert!((la.depth - lb.depth).abs() < 1e-9);
    }
    for (j, l) in interleaved.layers.iter().enumerate() {
        assert_eq!(b.visits(j).is_empty(), !l.has_object, "layer {j}");
    }
    // Planning the support layers as tower layers is what thinned and deepened the tower.
    for l in &mut interleaved.layers {
        l.has_object = true;
    }
    let thin = TowerPlan::new(&c, &interleaved).unwrap().unwrap();
    assert!(thin.layers.iter().any(|l| l.height < 0.06));
    assert!(thin.depth > a.depth + 1.);
}
#[test]
fn support_only_layer_with_its_own_tool_prints_the_tower() {
    let mut o = order(vec![vec![0], vec![0], vec![1], vec![1], vec![0]]);
    o.layers[2].has_object = false;
    o.layers[3].has_object = false;
    // Layer 2 changes the tool; layer 3 keeps tool 1 and is skipped unless the tower would
    // then rise by more than the maximum layer height.
    assert_eq!(o.tower_layers(0.7), [true, true, true, false, true]);
    assert_eq!(o.tower_layers(0.3), [true; 5]);
    let mut c = config();
    c.set_raw("max_layer_height", "0.7,0.7,0.7,0.7,0.7")
        .unwrap();
    let p = TowerPlan::new(&c, &o).unwrap().unwrap();
    assert_eq!(p.job_layers, [Some(0), Some(1), Some(2), None, Some(3)]);
    assert!((p.layers[3].height - 0.4).abs() < 1e-9);
}
#[test]
fn finish_layer_starts_where_m_left_to_right_points_after_the_first_change() {
    // WipeTower::generate runs finish_layer right after the first non-soluble toolchange; the
    // end of toolchange_Wipe flips m_left_to_right unless that tool is the layer's last one.
    // PrusaSlicer 2.9.6 XL references: with one change per layer the finish starts next to the
    // wipe's end, with two it starts at the opposite end (a tower-width travel).
    for (tools, adjacent) in [(vec![0, 4], true), (vec![0, 1, 2], false)] {
        let p = TowerPlan::new(&config(), &order(vec![tools; 60]))
            .unwrap()
            .unwrap();
        for i in [10, 11, 50, 51] {
            let after = &p.visits(i)[0].after.moves;
            let k = after.iter().position(|m| m.width == 0.).unwrap();
            let (end, start) = (after[k - 1].xy, after[k].xy);
            let gap = (end[0] - start[0]).hypot(end[1] - start[1]);
            assert_eq!(gap < 5., adjacent, "layer {i}: {end:?} -> {start:?}");
        }
    }
}
#[test]
fn visits_carry_the_upstream_wipe_path_for_leaving_the_tower() {
    // ToolChangeResult::wipe_path: toolchange_Wipe adds three points (down one line, then
    // towards the side the last line started from); finish_layer, merged after the first
    // change, replaces it by two points back along the outer contour. PrusaSlicer 2.9.6 XL,
    // three cubes T1/T2/T3, layer 10 (the next retraction wipes 0.16 mm along these paths):
    // finish end (240, 140) -> X238.46 Y140, second change end (239, 140.75) -> Y141.5 -> X238.21.
    let p = TowerPlan::new(&config(), &order(vec![vec![0, 1, 2]; 60]))
        .unwrap()
        .unwrap();
    let close = |a: [f64; 2], b: [f64; 2]| (a[0] - b[0]).hypot(a[1] - b[1]) < 1e-3;
    let visits = p.visits(10);
    assert_eq!(visits.len(), 2);
    let finish = &visits[0].wipe_path;
    assert_eq!(finish.len(), 2);
    assert!(close(finish[0], visits[0].after.moves.last().unwrap().xy));
    assert!(close(finish[0], [240., 140.]), "{finish:?}");
    assert!(
        (finish[1][1] - 140.).abs() < 1e-3 && finish[1][0] < 238.46,
        "{finish:?}"
    );
    let change = &visits[1].wipe_path;
    assert_eq!(change.len(), 3);
    assert!(close(change[0], visits[1].after.moves.last().unwrap().xy));
    for (a, b) in change
        .iter()
        .zip([[239., 140.75], [239., 141.5], [180., 141.5]])
    {
        assert!(close(*a, b), "{change:?}");
    }
    // toolchange_Unload alone (the final purge) leaves no wipe path.
    assert!(p.final_ram(0, 12.2, 0.2).unwrap().wipe_path.is_empty());
}
#[test]
fn visits_carry_the_merge_point_and_toolchange_entry() {
    // The first non-soluble change joins the finish (merge_tcr); every change records
    // tool_change_start_pos.
    let p = TowerPlan::new(&config(), &order(vec![vec![0, 1, 2]; 4]))
        .unwrap()
        .unwrap();
    let visits = p.visits(1);
    assert_eq!(visits.len(), 2);
    let pw = p.settings.line_width;
    for v in &visits {
        let entry = v.after.entry.expect("toolchange entry");
        assert!(
            (entry[0] - v.after.start[0]).hypot(entry[1] - v.after.start[1]) > pw / 2.,
            "tool_change_start_pos differs from the wipe start"
        );
    }
    let joined = visits[0]
        .after
        .joined
        .expect("finish joined after the first change");
    assert!(joined > 0 && joined < visits[0].after.moves.len());
    assert_eq!(visits[1].after.joined, None);
}
#[test]
fn first_layer_without_changes_starts_where_save_on_last_wipe_leaves_m_left_to_right() {
    // WipeTower::save_on_last_wipe runs tool_change over every layer (five passes) before
    // generate, so a first layer without toolchanges finishes in the corner the topmost
    // layer's last wipe leaves. PrusaSlicer 2.9.6, painted cube on the XL with a wipe tower
    // (T1 alone on the first layer, T1/T2 above): the first layer's tower starts at X239.5
    // Y140.5, not X180.5.
    let mut layers = vec![vec![0]];
    layers.extend(vec![vec![0, 1]; 20]);
    let p = TowerPlan::new(&config(), &order(layers)).unwrap().unwrap();
    let visits = p.visits(0);
    assert_eq!(visits.len(), 1);
    let start = visits[0].after.start;
    assert!(
        (start[0] - 239.5).abs() < 1e-3 && (start[1] - 140.5).abs() < 1e-3,
        "{start:?}"
    );
}
