// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 tests/fff_print/test_skirt_brim.cpp :: Original Slic3r Skirt/Brim tests
// Changes: Geometry/API assertions plus nested islands, Orca ears and auto-width regressions.
use super::*;
use crate::geometry::Point;

fn rect(x: f64, y: f64, w: f64, h: f64) -> ExPolygon {
    ExPolygon {
        contour: vec![
            Point::from_mm(x, y),
            Point::from_mm(x + w, y),
            Point::from_mm(x + w, y + h),
            Point::from_mm(x, y + h),
        ],
        holes: Vec::new(),
    }
}
fn plan(polys: &[ExPolygon], s: &AdhesionSettings, height: f64) -> AdhesionPlan {
    let islands: Vec<_> = polys
        .iter()
        .enumerate()
        .map(|(id, p)| FirstLayerIsland {
            object_id: id as u64,
            polygon: p,
            brim: &s.brim,
            object_height: height,
            ears: &[],
        })
        .collect();
    generate(&AdhesionInput {
        islands: &islands,
        supports: &[],
        first_layer: polys,
        skirt_hulls: &polys
            .iter()
            .enumerate()
            .map(|(id, p)| SkirtHull {
                object_id: id as u64,
                points: p.contour.clone(),
            })
            .collect::<Vec<_>>(),
        settings: s,
        flow: Flow::new(0.5, 0.3),
        resolution: 0.008,
        tools: &[SkirtTool {
            id: 0,
            filament_diameter: 1.75,
            extrusion_multiplier: 1.0,
        }],
        max_speed: 250.0,
        filament_type: "PLA",
    })
}
#[test]
fn upstream_two_brim_lines() {
    let mut s = AdhesionSettings::default();
    s.brim.width = 1.0;
    let p = plan(&[rect(0., 0., 20., 20.)], &s, 20.);
    assert_eq!(p.brim.len(), 2);
}
#[test]
fn upstream_height_zero_and_minimum_length() {
    let mut s = AdhesionSettings {
        skirts: 2,
        skirt_height: 0,
        ..Default::default()
    };
    assert!(plan(&[rect(0., 0., 20., 20.)], &s, 20.).skirt.is_empty());
    s.skirt_height = 5;
    s.min_skirt_length = 20.0;
    let p = plan(&[rect(0., 0., 20., 20.)], &s, 20.);
    assert!(
        geometry::path_length(&p.skirt) * Flow::new(0.5, 0.3).mm3_per_mm()
            / (std::f64::consts::PI * 0.875_f64.powi(2))
            >= 20.0
    );
    for layer in 0..8 {
        assert_eq!(
            !p.paths_for_tool(layer, 0.2, &[0], 0, 0).is_empty(),
            layer < 5
        );
    }
}
#[test]
fn upstream_skirt_smaller_than_brim() {
    let mut s = AdhesionSettings {
        skirts: 1,
        skirt_distance: 1.0,
        ..Default::default()
    };
    s.brim.width = 10.0;
    let p = plan(&[rect(0., 0., 20., 20.)], &s, 20.);
    assert_eq!(p.skirt.len(), 1);
    assert!(p.first_layer_bbox().unwrap().min.x < -crate::geometry::scale(11.0));
}
#[test]
fn ears_angle_threshold_and_auto_tall_thin() {
    let mut s = AdhesionSettings {
        orca: true,
        ..Default::default()
    };
    s.brim.kind = BrimType::Ears;
    s.brim.width = 5.0;
    s.brim.ears_max_angle = 91.0;
    assert!(!plan(&[rect(0., 0., 20., 20.)], &s, 20.).brim.is_empty());
    s.brim.ears_max_angle = 89.0;
    assert!(plan(&[rect(0., 0., 20., 20.)], &s, 20.).brim.is_empty());
    assert_eq!(
        orca::auto_width(&[rect(0., 0., 20., 20.)], 20., 250., "PLA"),
        0.0
    );
    assert!(orca::auto_width(&[rect(0., 0., 2., 8.)], 100., 250., "PLA") > 5.0);
}
#[test]
fn painted_ears_only_where_placed() {
    let mut s = AdhesionSettings {
        orca: true,
        ..Default::default()
    };
    s.brim.kind = BrimType::Painted;
    s.brim.width = 0.;
    s.brim.separation = 0.1;
    let square = rect(0., 0., 20., 20.);
    let run = |ears: &[BrimEar]| {
        let islands = [FirstLayerIsland {
            object_id: 0,
            polygon: &square,
            brim: &s.brim,
            object_height: 20.,
            ears,
        }];
        generate(&AdhesionInput {
            islands: &islands,
            supports: &[],
            first_layer: std::slice::from_ref(&square),
            skirt_hulls: &[],
            settings: &s,
            flow: Flow::new(0.5, 0.3),
            resolution: 0.008,
            tools: &[SkirtTool {
                id: 0,
                filament_diameter: 1.75,
                extrusion_multiplier: 1.0,
            }],
            max_speed: 250.0,
            filament_type: "PLA",
        })
    };
    // No ears: no brim even with brim_type painted.
    assert!(run(&[]).brim.is_empty());
    let ear = |x, y, z| BrimEar {
        x,
        y,
        z,
        radius: 5.,
    };
    let one = run(&[ear(0., 0., -0.0001)]);
    assert!(!one.brim.is_empty());
    // Every brim point lies near the (0, 0) corner, within the ear radius.
    let r = crate::geometry::scale(5.);
    assert!(
        one.brim
            .iter()
            .flat_map(|p| &p.points)
            .all(|p| p.x.abs() <= r && p.y.abs() <= r)
    );
    // Ears above the bed are ignored; two corners give more brim than one.
    assert!(run(&[ear(0., 0., 0.5)]).brim.is_empty());
    // An ear away from the model would print loose: Orca drops brim islands without contact.
    assert!(run(&[ear(60., 60., -0.0001)]).brim.is_empty());
    assert_eq!(
        run(&[ear(0., 0., -0.0001), ear(60., 60., -0.0001)])
            .brim
            .len(),
        one.brim.len()
    );
    let length = |p: &AdhesionPlan| {
        p.brim
            .iter()
            .map(|p| crate::geometry::polyline_length(&p.points))
            .sum::<f64>()
    };
    let two = run(&[ear(0., 0., -0.0001), ear(20., 20., -0.0001)]);
    assert!(length(&two) > 1.9 * length(&one));
}
#[test]
fn tool_order_allocation() {
    let s = AdhesionSettings {
        skirts: 5,
        ..Default::default()
    };
    let mut p = plan(&[rect(0., 0., 20., 20.)], &s, 20.);
    assert_eq!(
        p.skirt_ranges(&[2, 0, 3]),
        vec![(2, 0..2), (0, 2..4), (3, 4..5)]
    );
    p.orca = true;
    assert_eq!(p.skirt_ranges(&[2, 0, 3]), vec![(2, 0..5)]);
}

#[test]
fn holes_nested_objects_and_separation_do_not_fill_model_material() {
    use crate::geometry::clip::{Join, intersection_pl, offset_ex};
    use crate::geometry::scale;
    let mut ring = rect(0., 0., 40., 40.);
    let mut hole = rect(5., 5., 30., 30.).contour;
    hole.reverse();
    ring.holes.push(hole);
    let center = rect(15., 15., 10., 10.);
    let mut s = AdhesionSettings::default();
    s.brim.width = 3.;
    s.brim.separation = 0.1;
    s.brim.kind = BrimType::OuterAndInner;
    let polys = [ring, center];
    let p = plan(&polys, &s, 20.);
    assert!(p.brim.len() > 10);
    let forbidden = offset_ex(&polys, scale(0.09) as f64, Join::Square);
    let hit = intersection_pl(
        &p.brim.iter().map(|p| p.points.clone()).collect::<Vec<_>>(),
        &forbidden,
    );
    assert!(hit.is_empty(), "brim crossed material/gap: {hit:?}");
    s.brim.kind = BrimType::Inner;
    let p = plan(&polys[..1], &s, 20.);
    assert!(!p.brim.is_empty());
    assert!(
        p.brim
            .iter()
            .flat_map(|p| &p.points)
            .all(|p| p.x > scale(5.) && p.x < scale(35.))
    );
}

#[test]
fn nearby_per_object_skirts_merge_but_distant_ones_do_not() {
    let s = AdhesionSettings {
        skirts: 1,
        per_object: true,
        skirt_distance: 3.,
        ..Default::default()
    };
    assert_eq!(
        plan(&[rect(0., 0., 20., 20.), rect(25., 0., 20., 20.)], &s, 20.)
            .skirt
            .len(),
        1
    );
    assert_eq!(
        plan(&[rect(0., 0., 20., 20.), rect(50., 0., 20., 20.)], &s, 20.)
            .skirt
            .len(),
        2
    );
}

#[test]
fn tower_is_an_unexpanded_skirt_obstacle_without_its_own_loops() {
    let s = AdhesionSettings {
        skirts: 1,
        per_object: true,
        skirt_distance: 3.,
        ..Default::default()
    };
    for (tower_x, merged) in [(26., false), (22., true)] {
        let objects = [rect(0., 0., 20., 20.)];
        let hulls = [
            SkirtHull {
                object_id: 0,
                points: objects[0].contour.clone(),
            },
            SkirtHull {
                object_id: u64::MAX,
                points: rect(tower_x, 0., 20., 20.).contour,
            },
        ];
        let p = generate(&AdhesionInput {
            islands: &[],
            supports: &[],
            first_layer: &objects,
            skirt_hulls: &hulls,
            settings: &s,
            flow: Flow::new(0.5, 0.3),
            resolution: 0.008,
            tools: &[],
            max_speed: 250.,
            filament_type: "PLA",
        });
        assert_eq!(p.skirt.len(), 1);
        let bb = BoundingBox::from_points(&p.skirt[0].points).unwrap();
        assert_eq!(bb.max.x > Point::from_mm(tower_x, 0.).x, merged);
    }
}

#[test]
fn draft_shield_trims_brim_and_extends_to_all_layers() {
    use crate::geometry::clip::{intersection_pl, offset_polylines_round};
    let mut s = AdhesionSettings {
        skirts: 1,
        skirt_distance: 2.,
        draft_shield: DraftShield::Enabled,
        ..Default::default()
    };
    s.brim.width = 5.;
    let p = plan(&[rect(0., 0., 20., 20.)], &s, 20.);
    assert!(!p.paths_for_tool(90, 0.2, &[0], 0, 0).is_empty());
    let band = offset_polylines_round(
        &p.skirt.iter().map(|p| p.points.clone()).collect::<Vec<_>>(),
        Flow::new(0.5, 0.3).scaled_spacing() as f64 * 0.49,
        10000.,
    );
    assert!(
        intersection_pl(
            &p.brim.iter().map(|p| p.points.clone()).collect::<Vec<_>>(),
            &band
        )
        .is_empty()
    );
}

#[test]
fn auto_width_is_translation_invariant_and_holes_reduce_stiffness() {
    let a = orca::auto_width(&[rect(0., 0., 10., 4.)], 50., 250., "PLA");
    let b = orca::auto_width(&[rect(1000., 3000., 10., 4.)], 50., 250., "PLA");
    assert!((a - b).abs() < 1e-10);
    let mut hollow = rect(0., 0., 10., 4.);
    let mut hole = rect(1., 1., 8., 2.).contour;
    hole.reverse();
    hollow.holes.push(hole);
    assert!(orca::auto_width(&[hollow], 50., 250., "PLA") >= a);
}

#[test]
fn prepared_layers_prefix_adhesion_and_keep_batch_results_equal() {
    use crate::{
        mesh::TriangleMesh,
        print::{LayerSource, prepare_object},
        settings::PrintSettings,
    };
    let mut mesh = TriangleMesh::load_stl(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/models/cube20.stl"
    ))
    .unwrap();
    mesh.place_on_bed(125., 110.);
    let mut s = PrintSettings::core_one_hf04_speed_pla();
    s.adhesion.skirts = 1;
    s.adhesion.skirt_height = 5;
    s.adhesion.brim.width = 5.;
    let obj = prepare_object(&mesh, &s).unwrap();
    let all = obj.toolpaths(0..7);
    for (i, layer) in all.iter().enumerate() {
        assert_eq!(layer.paths, obj.toolpaths(i..i + 1)[0].paths);
        assert_eq!(layer.paths.first().unwrap().role == Role::Skirt, i < 5);
        assert_eq!(layer.paths.iter().any(|p| p.role == Role::Brim), i == 0);
    }
    assert!(obj.first_layer_bbox().unwrap().min.x < crate::geometry::scale(105.));
}

#[test]
fn support_footprint_is_inside_skirt_and_orca_brim_avoids_support() {
    use crate::geometry::clip::intersection_pl;
    let model = rect(0., 0., 20., 20.);
    let support = rect(19., 0., 8., 25.);
    let mut s = AdhesionSettings {
        skirts: 1,
        ..Default::default()
    };
    s.brim.width = 5.;
    let make = |s: &AdhesionSettings| {
        generate(&AdhesionInput {
            islands: &[FirstLayerIsland {
                object_id: 0,
                polygon: &model,
                brim: &s.brim,
                object_height: 20.,
                ears: &[],
            }],
            supports: std::slice::from_ref(&support),
            first_layer: std::slice::from_ref(&model),
            skirt_hulls: &[SkirtHull {
                object_id: 0,
                points: model
                    .contour
                    .iter()
                    .chain(&support.contour)
                    .copied()
                    .collect(),
            }],
            settings: s,
            flow: Flow::new(0.5, 0.3),
            resolution: 0.008,
            tools: &[],
            max_speed: 250.,
            filament_type: "PLA",
        })
    };
    let prusa = make(&s);
    assert!(prusa.first_layer_bbox().unwrap().max.x > crate::geometry::scale(33.));
    assert!(!prusa.brim.is_empty());
    // Prusa brims precede supports and may cross them; Orca masks the support bed-contact areas.
    s.orca = true;
    let orca = make(&s);
    assert!(
        intersection_pl(
            &orca
                .brim
                .iter()
                .map(|p| p.points.clone())
                .collect::<Vec<_>>(),
            &[support]
        )
        .is_empty()
    );
}

#[test]
fn resolution_rejects_invalid_settings_and_honors_width_fallback() {
    use crate::{config::Config, settings::PrintSettings};
    let mut c = Config::defaults();
    c.set_raw("first_layer_extrusion_width", "0").unwrap();
    c.set_raw("first_layer_height", "0.3").unwrap();
    c.set_raw("perimeter_extrusion_width", "200%").unwrap();
    assert_eq!(
        PrintSettings::from_config(&c)
            .unwrap()
            .adhesion
            .extrusion_width,
        0.6
    );
    for (key, val) in [
        ("brim_width", "-1"),
        ("brim_separation", "NaN"),
        ("skirts", "-1"),
        ("brim_ears_max_angle", "181"),
    ] {
        let mut bad = c.clone();
        assert!(
            bad.set_raw(key, val).is_err() || PrintSettings::from_config(&bad).is_err(),
            "{key}"
        );
    }
}
