// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 tests/fff_print/test_avoid_crossing_perimeters.cpp :: Avoid crossing perimeters
// Changes: Two 20 mm cubes as one multi-island mesh, plus batched/cached, missing-outline and scheduled-support regressions.
use mica_core::{
    config::Config,
    geometry::Point,
    mesh::TriangleMesh,
    print::{LayerSource, prepare_object},
    settings::PrintSettings,
};
use mica_gcode::export::export_gcode;

fn config() -> Config {
    let mut c = Config::defaults();
    for line in
        include_str!("../../mica-profiles/tests/fixtures/coreone_hf04_speed_pla.ini").lines()
    {
        if let Some((k, v)) = line.split_once(" = ") {
            c.set_raw(k, v).unwrap();
        }
    }
    c.set_raw("avoid_crossing_perimeters", "1").unwrap();
    c
}
#[test]
fn upstream_two_twenty_mm_cubes_and_batch_equivalence() {
    let mut mesh = TriangleMesh::load_stl(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/models/cube20.stl"
    ))
    .unwrap();
    let mut other = mesh.clone();
    mesh.place_on_bed(100., 110.);
    other.place_on_bed(140., 110.);
    let offset = mesh.vertices.len() as u32;
    mesh.vertices.extend(other.vertices);
    mesh.faces.extend(
        other
            .faces
            .into_iter()
            .map(|f| [f[0] + offset, f[1] + offset, f[2] + offset]),
    );
    let c = config();
    let s = PrintSettings::from_config(&c).unwrap();
    let prepared = prepare_object(&mesh, &s).unwrap();
    let batched = export_gcode(&prepared, &c, &s, "two cubes").unwrap();
    assert!(batched.gcode.contains(";TYPE:External perimeter"));
    assert_eq!(batched.gcode.matches(";LAYER_CHANGE").count(), 100);
    let cached = prepared.into_sliced();
    assert_eq!(cached.layer_outlines(50).unwrap().len(), 2);
    let all = export_gcode(&cached, &c, &s, "two cubes").unwrap();
    assert_eq!(batched.gcode, all.gcode);
}
#[test]
fn independent_support_z_schedule_has_matching_outlines() {
    let mut c = config();
    c.set_raw("raft_layers", "2").unwrap();
    c.set_raw("support_material", "1").unwrap();
    let s = PrintSettings::from_config(&c).unwrap();
    let mut mesh = TriangleMesh::load_stl(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/models/cube20.stl"
    ))
    .unwrap();
    mesh.place_on_bed(125., 110.);
    let prepared = prepare_object(&mesh, &s).unwrap();
    let expected: Vec<_> = (0..prepared.layer_count())
        .map(|i| {
            prepared
                .layer_outlines(i)
                .map(|p| p.to_vec())
                .unwrap_or_default()
        })
        .collect();
    assert!(expected.iter().any(Vec::is_empty));
    let cached = prepared.into_sliced();
    for (i, outline) in expected.iter().enumerate() {
        assert_eq!(cached.layer_outlines(i).unwrap_or_default(), outline);
    }
    assert!(cached.layer_outlines(cached.layer_count() + 1).is_none());
}
#[test]
fn travel_lift_follows_the_route_without_dropping_between_corners() {
    use mica_core::{geometry::ExPolygon, toolpath::LayerToolpaths};
    use mica_gcode::travels::{TravelPlanner, duration};
    let mut c = config();
    c.set_raw("retract_lift", "0.6").unwrap();
    c.set_raw("travel_ramping_lift", "0").unwrap();
    c.set_raw("filament_travel_ramping_lift", "0").unwrap();
    c.set_raw("filament_retract_lift", "0.6").unwrap();
    let polygon = |p: &[(f64, f64)]| p.iter().map(|p| Point::from_mm(p.0, p.1)).collect();
    let outline = vec![ExPolygon {
        contour: polygon(&[(0., 0.), (20., 0.), (20., 20.), (0., 20.)]),
        holes: vec![polygon(&[(8., 8.), (8., 12.), (12., 12.), (12., 8.)])],
    }];
    let layer = LayerToolpaths {
        index: 1,
        print_z: 0.4,
        height: 0.2,
        ..Default::default()
    };
    // The Prusa path plans avoidance with the ported AvoidCrossingPerimeters planner.
    let geometry = mica_gcode::prusa_avoid_crossing::LayerGeometry {
        lslices: &outline,
        tops: &[],
        perimeter_spacing: 407079.0,
    };
    let planner = TravelPlanner::new(&c, &layer, Some(&outline), Some(&outline))
        .with_prusa_avoid_crossing(&c, Some(geometry), Some(geometry), 449999.0);
    let (xy, _) = planner
        .prusa_route((2., 10.), (18., 10.), false, false)
        .unwrap();
    let route = planner.plan_route(&xy, 0, true);
    assert!(route.len() > 3);
    assert!(
        route[..route.len() - 1]
            .iter()
            .all(|p| (p[2] - 1.).abs() < 2e-6)
    );
    // PrusaSlicer's scaled Z is truncated to whole micrometres.
    let last = *route.last().unwrap();
    assert!(
        (last[0] - 18.).abs() < 1e-9
            && (last[1] - 10.).abs() < 1e-9
            && (last[2] - 0.4).abs() < 2e-6
    );
    assert!(mica_gcode::travels::route_length(&xy) > 16.);
    assert!(duration([2., 10., 0.4], &route, 200., 12.) > 16. / 200.);
    let flat = planner.plan_route(&xy, 0, false);
    assert!(flat.len() > 2);
    assert!(flat.iter().all(|p| p[2] == 0.4));
}

#[test]
fn dynamic_history_survives_two_export_batch_boundaries() {
    use mica_core::{
        flow::Flow,
        geometry::{BoundingBox, ExPolygon},
        print::SlicedObject,
        toolpath::{ExtrusionPath, LayerToolpaths, Role},
    };
    let mut object = SlicedObject {
        adhesion: Default::default(),
        outlines: Vec::new(),
        layers: Vec::new(),
        first_layer_bbox: None,
        report: Default::default(),
    };
    for i in 0..65 {
        let x = 80. + i as f64 * 0.16;
        let corners = vec![
            Point::from_mm(x, 80.),
            Point::from_mm(x + 10., 80.),
            Point::from_mm(x + 10., 90.),
            Point::from_mm(x, 90.),
        ];
        object.outlines.push(vec![ExPolygon {
            contour: corners.clone(),
            holes: Vec::new(),
        }]);
        let mut points = corners;
        points.push(points[0]);
        object.layers.push(LayerToolpaths {
            support_groups: Vec::new(),
            index: i,
            print_z: (i + 1) as f64 * 0.2,
            height: 0.2,
            paths: vec![ExtrusionPath {
                role: Role::ExternalPerimeter,
                points,
                flow: Flow::new(0.4, 0.2),
                seam: None,
                overhang: None,
                perimeter_index: None,
                brick: None,
            }],
        });
    }
    object.first_layer_bbox = BoundingBox::from_points(&object.outlines[0][0].contour);
    let c = config();
    let settings = PrintSettings::from_config(&c).unwrap();
    let expected = export_gcode(&object, &c, &settings, "overhang").unwrap();
    // Annotation supplied by a caller across the entire object must match the streaming exporter.
    let mut processor = mica_gcode::overhang::OverhangProcessor::default();
    for (i, layer) in object.layers.iter_mut().enumerate() {
        processor.process(layer, Some(&object.outlines[i]), &c);
    }
    assert!(object.layers[32].paths[0].overhang.is_some());
    assert!(object.layers[64].paths[0].overhang.is_some());
    let actual = export_gcode(&object, &c, &settings, "overhang").unwrap();
    assert!(
        actual.gcode == expected.gcode,
        "precomputed annotations differ from batched output"
    );
}

#[test]
fn repeated_fan_requests_are_consumed_and_custom_fan_is_restored() {
    let mut writer = mica_gcode::writer::Writer::new();
    writer.set_fan(100.);
    writer.set_fan(100.);
    assert_eq!(writer.as_str().matches("M106").count(), 1);
    writer.raw("M107 ; custom template");
    writer.set_fan(100.);
    assert_eq!(writer.as_str().matches("M106").count(), 2);
}
