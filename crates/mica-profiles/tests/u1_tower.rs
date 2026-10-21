// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: orca@8500fcdcca src/libslic3r/GCode/WipeTower2.cpp :: plan_toolchange, plan_tower, save_on_last_wipe, generate_support_rib_wall
// Changes: Profile-to-plan regression against saved Orca 2.4.1 four-tool tower measurements; bed-local multi-plate import checks.

use mica_core::{
    config::Config,
    geometry::unscale,
    input::{PlateSelection, load_with_plate},
    tool_ordering::ToolOrdering,
    wipe_tower::{TowerKind, TowerPlan, Wall},
};
use mica_profiles::{
    orca::{Dialect, map_config},
    project::Mapper,
};

fn config() -> Config {
    let raw = serde_json::from_value(serde_json::json!({
        "gcode_flavor": "klipper", "nozzle_diameter": ["0.4", "0.4", "0.4", "0.4"],
        "printable_area": ["0x0", "256x0", "256x256", "0x256"],
        "enable_prime_tower": "1", "prime_tower_width": "35", "prime_volume": "30",
        "prime_tower_brim_width": "5", "wipe_tower_cone_angle": "15", "wipe_tower_wall_type": "rib",
        "wipe_tower_extra_spacing": "100%", "wipe_tower_extra_flow": "100%",
        "wipe_tower_rib_width": "8", "wipe_tower_extra_rib_length": "0", "wipe_tower_fillet_wall": "1",
        "wipe_tower_bridging": "10", "enable_filament_ramming": "0", "purge_in_prime_tower": "0",
        "filament_minimal_purge_on_wipe_tower": ["15", "15", "15", "15"],
        "filament_multitool_ramming": ["0", "0", "0", "0"],
        "filament_multitool_ramming_flow": ["10", "10", "10", "10"],
        "single_extruder_multi_material_priming": "0",
        "wipe_tower_x": ["165"], "wipe_tower_y": ["233.362"]
    })).unwrap();
    map_config(&raw, Dialect::Orca242).config
}

fn ordering(layers: usize) -> ToolOrdering {
    ToolOrdering::new(
        (0..layers)
            .map(|i| (0.25 + i as f64 * 0.2, (0..4).map(Some).collect()))
            .collect(),
        true,
    )
}

fn bounds(p: &TowerPlan) -> [f64; 4] {
    let b = p.first_layer_bbox().unwrap();
    [
        unscale(b.min.x),
        unscale(b.min.y),
        unscale(b.max.x),
        unscale(b.max.y),
    ]
}

#[test]
fn four_tool_u1_matches_saved_orca_size_and_filament() {
    let c = config();
    let p = TowerPlan::new(&c, &ordering(100)).unwrap().unwrap();
    assert_eq!(p.settings.kind, TowerKind::Orca);
    assert_eq!(p.settings.wall, Wall::Rib);
    assert_eq!(p.settings.width, 35.);
    assert_eq!(p.depth, 13.5);
    assert_eq!(p.settings.position, [165., 233.362]);
    assert_eq!(
        p.layers[0]
            .changes
            .iter()
            .map(|t| t.required_depth)
            .collect::<Vec<_>>(),
        [2., 4., 4.]
    );
    assert_eq!(
        p.layers[1]
            .changes
            .iter()
            .map(|t| t.required_depth)
            .collect::<Vec<_>>(),
        [3., 5., 5.]
    );
    for (actual, expected) in bounds(&p)
        .into_iter()
        .zip([159.055, 225.887, 205.945, 254.337])
    {
        assert!((actual - expected).abs() < 0.001, "{actual} vs {expected}");
    }
    let volume: f64 = (0..p.layers.len())
        .flat_map(|i| p.visits(i))
        .map(|v| {
            assert!(v.before.is_none(), "U1 defaults disable ramming");
            v.after.moves.iter().map(|m| m.volume).sum::<f64>()
        })
        .sum();
    let filament = volume / (std::f64::consts::PI * (1.75_f64 / 2.).powi(2));
    // Orca reference: u1-cubes (scripts/prepare-wipe-tower-oracles.py), 100 layers, four tools.
    assert!((filament / 3455.7281 - 1.).abs() < 0.03, "{filament}");
    assert!(
        p.layers
            .iter()
            .all(|l| l.changes.len() == 3 && l.depth <= 13.)
    );
}

#[test]
fn rib_tower_ignores_cone_angle_height_and_flush_matrix() {
    let mut c = config();
    let short = TowerPlan::new(&c, &ordering(100)).unwrap().unwrap();
    c.set_raw("wipe_tower_cone_angle", "45").unwrap();
    c.set_raw("flush_volumes_matrix", "9999").unwrap();
    c.set_raw("flush_volumes_vector", "9999").unwrap();
    c.set_raw("wiping_volumes_matrix", "9999").unwrap();
    let tall = TowerPlan::new(&c, &ordering(1250)).unwrap().unwrap();
    assert_eq!(short.depth, tall.depth);
    assert_eq!(bounds(&short), bounds(&tall));
    assert_eq!(
        short.layers[1].changes[0].required_depth,
        tall.layers[1].changes[0].required_depth
    );
}

#[test]
fn selected_orca_plate_imports_its_local_tower_position() {
    let paths = [std::path::PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/models/plates-orca.3mf"
    ))];
    for (plate, expected) in [(1, [165., 233., 0.]), (2, [165., 233., 0.])] {
        let loaded = load_with_plate(
            &paths,
            (128., 128.),
            false,
            true,
            0,
            &Default::default(),
            &config(),
            &Mapper::default(),
            PlateSelection::Plate(plate),
        )
        .unwrap();
        assert_eq!(loaded.tower_placement, Some(expected));
        assert_eq!(loaded.plates[plate - 1].tower, Some(expected));
    }
}
