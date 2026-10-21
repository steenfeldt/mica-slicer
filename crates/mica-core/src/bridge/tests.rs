// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 tests/fff_print/test_bridges.cpp :: Bridge detector, Bridging integration
// Source: prusaslicer@b028299c77 tests/fff_print/test_flow.cpp :: Bridge flow specifics, Flow math for bridges
// Source: orca@8500fcdcca src/libslic3r/Fill/Fill.cpp :: Layer::make_fills
// Changes: Fixed-flow bridge regression and independent internal thickness.
// Changes: Rust geometry fixtures; additional flow, rotation, support and batch regressions.
use super::detector::{BridgeDetector, rotate};
use super::*;
use crate::geometry::clip::diff_ex;
use crate::geometry::{SCALE, polyline_length};
fn polygon(points: &[(f64, f64)]) -> ExPolygon {
    ExPolygon {
        contour: points.iter().map(|&(x, y)| Point::from_mm(x, y)).collect(),
        holes: Vec::new(),
    }
}
fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> ExPolygon {
    polygon(&[(x0, y0), (x1, y0), (x1, y1), (x0, y1)])
}
fn angle_distance(a: f64, b: f64) -> f64 {
    let d = (a - b).abs().rem_euclid(180.);
    d.min(180. - d)
}
fn check(bridge: ExPolygon, lower: Vec<ExPolygon>, expected: f64, tolerance: f64, fraction: f64) {
    let mut detector = BridgeDetector::new(std::slice::from_ref(&bridge), &lower, 0.5);
    let angle = detector.detect_angle(0.).expect("anchored bridge");
    assert!(
        angle_distance(angle.to_degrees(), expected) <= tolerance,
        "angle {} expected {expected}",
        angle.to_degrees()
    );
    let coverage: f64 = detector
        .coverage(angle)
        .iter()
        .map(ExPolygon::area_mm2)
        .sum();
    assert!(
        (coverage - bridge.area_mm2() * fraction).abs() < 0.02,
        "coverage {coverage}, expected {}",
        bridge.area_mm2() * fraction
    );
}
#[test]
fn upstream_o_shaped_bridges() {
    for (x, y, rotation, expected, tolerance) in [
        (20., 10., 0.0f64, 90., 5.01),
        (10., 20., 0., 0., 5.01),
        (20., 10., 45., 135., 20.),
        (20., 10., 135., 45., 20.),
    ] {
        let bridge = rect(0., 0., x, y);
        let lower = diff_ex(
            &[rect(-2., -2., x + 2., y + 2.)],
            std::slice::from_ref(&bridge),
        );
        check(
            rotate(&[bridge], rotation.to_radians()).remove(0),
            rotate(&lower, rotation.to_radians()),
            expected,
            tolerance,
            1.,
        );
    }
}
#[test]
fn upstream_two_sided_c_shaped_and_partial_l_coverage() {
    check(
        rect(0., 0., 20., 10.),
        vec![rect(-2., 0., 0., 10.), rect(20., 0., 22., 10.)],
        0.,
        5.01,
        1.,
    );
    check(
        polygon(&[(0., 0.), (20., 0.), (10., 10.), (0., 10.)]),
        vec![polygon(&[
            (0., 0.),
            (0., 10.),
            (10., 10.),
            (10., 12.),
            (-2., 12.),
            (-2., -2.),
            (22., -2.),
            (22., 0.),
        ])],
        135.,
        5.01,
        1.,
    );
    check(
        rect(10., 10., 20., 20.),
        vec![polygon(&[
            (10., 10.),
            (10., 20.),
            (20., 20.),
            (30., 30.),
            (0., 30.),
            (0., 0.),
        ])],
        45.,
        5.01,
        0.5,
    );
}
#[test]
fn detached_and_cantilever_are_not_fully_supported() {
    let bridge = rect(0., 0., 20., 10.);
    let mut floating = BridgeDetector::new(std::slice::from_ref(&bridge), &[], 0.5);
    assert_eq!(floating.detect_angle(0.), None);
    let mut cantilever = BridgeDetector::new(&[bridge], &[rect(-2., 0., 0., 10.)], 0.5);
    if let Some(angle) = cantilever.detect_angle(0.) {
        assert!(
            cantilever
                .coverage(angle)
                .iter()
                .map(ExPolygon::area_mm2)
                .sum::<f64>()
                < 1.0
        );
    }
}
#[test]
fn rotated_active_direction_and_fill_ignore_layer_alternation() {
    let s = PrintSettings::core_one_hf04_speed_pla();
    for degrees in [0.0f64, 30., 90.] {
        let angle = degrees.to_radians();
        let area = rotate(&[rect(-1., 0., 21., 10.)], angle);
        let anchors = rotate(&[rect(-1., 0., 0., 10.), rect(20., 0., 21., 10.)], angle);
        let direction = detector::direction_from_anchors(&area, &anchors);
        assert!(angle_distance(direction.to_degrees(), degrees) < 0.01);
        let surface = BridgeSurface {
            polygon: area[0].clone(),
            angle: direction,
        };
        let paths = toolpaths(
            &[surface],
            &area,
            &s,
            Flow::new(0.45, 0.2),
            Point::default(),
            false,
        );
        let longest = paths
            .iter()
            .flat_map(|p| p.points.windows(2))
            .max_by_key(|l| l[0].distance_sq(l[1]))
            .unwrap();
        let angle = ((longest[1].y - longest[0].y) as f64)
            .atan2((longest[1].x - longest[0].x) as f64)
            .to_degrees();
        assert!(
            angle_distance(angle, degrees) < 0.01,
            "{angle} != {degrees}"
        );
    }
}
#[test]
fn upstream_flow_ratios_and_constant_spacing() {
    let mut s = PrintSettings::core_one_hf04_speed_pla();
    let normal = Flow::new(0.45, 0.2);
    for ratio in [0.1f64, 0.5, 1., 2.] {
        s.bridge_flow_ratio = ratio;
        for thick in [false, true] {
            s.thick_bridges = thick;
            let flow = BridgeFlow::new(&s, normal, false);
            let want = if thick {
                std::f64::consts::PI / 4. * s.nozzle_diameter.powi(2) * ratio
            } else {
                normal.mm3_per_mm() * ratio
            };
            assert!((flow.extrusion.mm3_per_mm() - want).abs() < 1e-12);
            let spacing = if thick {
                s.nozzle_diameter * ratio.sqrt() + 0.05
            } else {
                normal.spacing()
            };
            assert!((flow.spacing - spacing).abs() < 1e-12);
        }
        s.thick_bridges = false;
        assert!(
            (BridgeFlow::new(&s, normal, true).extrusion.height - s.nozzle_diameter * ratio.sqrt())
                .abs()
                < 1e-12
        );
    }
}
#[test]
fn bridge_paths_have_the_requested_volume_and_role() {
    let s = PrintSettings {
        thick_bridges: true,
        bridge_flow_ratio: 0.5,
        ..PrintSettings::core_one_hf04_speed_pla()
    };
    let area = vec![rect(0., 0., 20., 10.)];
    let paths = toolpaths(
        &[BridgeSurface {
            polygon: area[0].clone(),
            angle: 0.,
        }],
        &area,
        &s,
        Flow::new(0.45, 0.2),
        Point::default(),
        false,
    );
    assert!(!paths.is_empty());
    for p in paths {
        assert_eq!(p.role, Role::BridgeInfill);
        let filament = polyline_length(&p.points) / SCALE * p.flow.mm3_per_mm()
            / (std::f64::consts::PI * (s.filament_diameter / 2.).powi(2));
        let expected = polyline_length(&p.points) / SCALE
            * (s.nozzle_diameter / s.filament_diameter).powi(2)
            * s.bridge_flow_ratio;
        assert!((filament - expected).abs() < 0.1 * expected);
    }
}

fn classify(outlines: &[Vec<ExPolygon>], s: &PrintSettings) -> Vec<crate::shells::FillSurfaces> {
    let flow = Flow::new(0.45, 0.2);
    let layers: Vec<_> = (0..outlines.len())
        .map(|i| crate::shells::ShellLayer {
            print_z: (i + 1) as f64 * 0.2,
            height: 0.2,
            external: flow,
            perimeter: flow,
            solid: flow,
        })
        .collect();
    let fill = outlines
        .iter()
        .map(|ex| detector::offset(ex, -0.8 * SCALE))
        .collect();
    crate::shells::classify(outlines, fill, &layers, s)
}

#[test]
fn internal_bridge_depth_density_and_partition() {
    let outlines = vec![vec![rect(0., 0., 40., 30.)]; 40];
    for density in [0., 0.15, 1.] {
        let s = PrintSettings {
            fill_density: density,
            ..PrintSettings::core_one_hf04_speed_pla()
        };
        let layers = classify(&outlines, &s);
        let bridges: Vec<_> = layers
            .iter()
            .enumerate()
            .filter(|(_, f)| !f.internal_bridge.is_empty())
            .collect();
        assert_eq!(bridges.is_empty(), density == 1.0);
        if density < 1.0 {
            assert_eq!(bridges.len(), 1);
            assert_eq!(bridges[0].0, 35);
            let f = bridges[0].1;
            let bridged: Vec<_> = f
                .internal_bridge
                .iter()
                .map(|b| b.polygon.clone())
                .collect();
            assert!(
                crate::geometry::clip::intersection_ex(&bridged, &f.internal)
                    .iter()
                    .map(ExPolygon::area_mm2)
                    .sum::<f64>()
                    < 0.01
            );
            assert!(
                crate::geometry::clip::intersection_ex(&bridged, &f.internal_solid)
                    .iter()
                    .map(ExPolygon::area_mm2)
                    .sum::<f64>()
                    < 0.01
            );
        }
    }
    let s = PrintSettings {
        top_solid_layers: 0,
        solid_infill_every_layers: 10,
        ..PrintSettings::core_one_hf04_speed_pla()
    };
    let layers = classify(&outlines, &s);
    assert!(!layers[10].internal_bridge.is_empty());
    let shallow = classify(&outlines[..5], &PrintSettings::core_one_hf04_speed_pla());
    assert!(shallow.iter().all(|f| f.internal_bridge.is_empty()));
}

#[test]
fn override_supported_bottom_and_bridge_support_masks() {
    let outlines: Vec<_> = (0..40)
        .map(|i| {
            if i < 30 {
                vec![rect(0., 0., 5., 12.), rect(25., 0., 30., 12.)]
            } else {
                vec![rect(0., 0., 30., 12.)]
            }
        })
        .collect();
    let mut s = PrintSettings {
        bridge_angle: 30.,
        ..PrintSettings::core_one_hf04_speed_pla()
    };
    let classified = classify(&outlines, &s);
    assert!(!classified[30].bridges.is_empty());
    assert!(
        classified[30]
            .bridges
            .iter()
            .all(|b| (b.angle.to_degrees() - 30.).abs() < 1e-6)
    );
    s.support_material_contact_distance = 0.;
    assert!(classify(&outlines, &s).iter().all(|f| f.bridges.is_empty()));
    let masks = detector::support_coverage(&outlines, 0.4, 0.0);
    assert!((masks[30].iter().map(ExPolygon::area_mm2).sum::<f64>() - 240.).abs() < 0.01);
    let heights = vec![0.2; outlines.len()];
    let tops: Vec<_> = (0..outlines.len()).map(|i| (i + 1) as f64 * 0.2).collect();
    let mut config = crate::support::SupportConfig::from(&s);
    config.automatic = true;
    config.top_gap = 0.2;
    config.dont_support_bridges = false;
    config.bridge_angle = 0.0;
    let (normal, _) =
        crate::support::schedule::prepare(&tops, &heights, &outlines, &config, Point::default())
            .unwrap();
    assert!(!normal.layers.is_empty());
    config.dont_support_bridges = true;
    let (bridged, _) =
        crate::support::schedule::prepare(&tops, &heights, &outlines, &config, Point::default())
            .unwrap();
    assert!(bridged.layers.is_empty());
    config.bridge_angle = 90.0;
    let (wrong_direction, _) =
        crate::support::schedule::prepare(&tops, &heights, &outlines, &config, Point::default())
            .unwrap();
    assert!(!wrong_direction.layers.is_empty());
    config.bridge_angle = 0.0;
    let cantilever: Vec<_> = (0..40)
        .map(|i| {
            if i < 30 {
                vec![rect(0., 0., 5., 12.)]
            } else {
                vec![rect(0., 0., 30., 12.)]
            }
        })
        .collect();
    let (supported, _) =
        crate::support::schedule::prepare(&tops, &heights, &cantilever, &config, Point::default())
            .unwrap();
    assert!(!supported.layers.is_empty());
}

#[test]
fn settings_resolve_bridge_controls_and_acceleration_fallback() {
    let mut config = crate::config::Config::defaults();
    for (k, v) in [
        ("bridge_angle", "30"),
        ("bridge_flow_ratio", "0.5"),
        ("bridge_speed", "37"),
        ("bridge_fan_speed", "72"),
        ("thick_bridges", "1"),
        ("dont_support_bridges", "1"),
        ("bridge_acceleration", "0"),
        ("default_acceleration", "1200"),
    ] {
        config.set_raw(k, v).unwrap();
    }
    let s = PrintSettings::from_config(&config).unwrap();
    assert_eq!(s.bridge_angle, 30.);
    assert_eq!(s.bridge_flow_ratio, 0.5);
    assert_eq!(s.bridge_speed, 37.);
    assert_eq!(s.bridge_fan_speed, 72.);
    assert!(s.thick_bridges && s.dont_support_bridges);
    assert_eq!(s.bridge_acceleration, 1200.);
    config.set_raw("bridge_flow_ratio", "0").unwrap();
    assert!(PrintSettings::from_config(&config).is_err());
}

#[test]
fn bridge_toolpaths_are_batch_invariant() {
    use crate::print::LayerSource;
    let mut mesh = crate::mesh::TriangleMesh::load_stl(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/models/cube20.stl"
    ))
    .unwrap();
    mesh.place_on_bed(125., 110.);
    let obj =
        crate::print::prepare_object(&mesh, &PrintSettings::core_one_hf04_speed_pla()).unwrap();
    let all = obj.toolpaths(90..100);
    let separate: Vec<_> = (90..100).flat_map(|i| obj.toolpaths(i..i + 1)).collect();
    assert!(
        all.iter()
            .flat_map(|l| &l.paths)
            .any(|p| p.role == Role::InternalBridgeInfill)
    );
    for (a, b) in all.iter().zip(&separate) {
        assert_eq!(a.paths, b.paths);
    }
}

#[test]
fn orca_bridges_keep_flow_when_spacing_does_not_fit_the_island() {
    let mut s = PrintSettings::core_one_hf04_speed_pla();
    s.bridge_style = BridgeStyle::Orca;
    s.thick_bridges = false;
    s.thick_internal_bridges = true;
    let normal = Flow::new(0.42, 0.2);
    let area = rect(0., 0., 13.7, 10.1);
    let surfaces = [BridgeSurface {
        polygon: area.clone(),
        angle: 0.,
    }];
    let paths = toolpaths(&surfaces, &[area], &s, normal, Point::default(), true);
    assert!(!paths.is_empty());
    let expected = BridgeFlow::new(&s, normal, true).extrusion;
    assert!(paths.iter().all(|p| p.flow == expected));
    assert_eq!(expected.width, s.nozzle_diameter);
    assert_eq!(
        BridgeFlow::new(&s, normal, false).extrusion.height,
        normal.height
    );
    s.thick_internal_bridges = false;
    assert_eq!(
        BridgeFlow::new(&s, normal, true).extrusion.height,
        normal.height
    );
}

#[test]
fn orca_internal_angle_and_extra_layer_are_perpendicular_and_disjoint() {
    let outlines = vec![vec![rect(0., 0., 40., 30.)]; 40];
    let s = PrintSettings {
        internal_bridge_angle: 30.,
        extra_bridge_layer: ExtraBridgeLayer::Internal,
        bridge_style: BridgeStyle::Orca,
        ..PrintSettings::core_one_hf04_speed_pla()
    };
    let layers = classify(&outlines, &s);
    let bridges: Vec<_> = layers
        .iter()
        .enumerate()
        .filter(|(_, f)| !f.internal_bridge.is_empty())
        .collect();
    assert_eq!(
        bridges.iter().map(|(i, _)| *i).collect::<Vec<_>>(),
        [35, 36]
    );
    for (index, f) in bridges {
        for bridge in &f.internal_bridge {
            assert!(
                angle_distance(
                    bridge.angle.to_degrees(),
                    if index == 35 { 30. } else { 120. }
                ) < 1e-6
            );
            assert!(
                intersection_ex(std::slice::from_ref(&bridge.polygon), &f.internal_solid)
                    .iter()
                    .map(ExPolygon::area_mm2)
                    .sum::<f64>()
                    < 0.01
            );
        }
    }
}

#[test]
fn extra_external_bridge_is_classified_before_full_density_conversion() {
    let mut outlines = vec![vec![rect(0., 0., 5., 12.), rect(25., 0., 30., 12.)]; 10];
    outlines.extend(vec![vec![rect(0., 0., 30., 12.)]; 10]);
    let s = PrintSettings {
        fill_density: 1.,
        extra_bridge_layer: ExtraBridgeLayer::External,
        ..PrintSettings::core_one_hf04_speed_pla()
    };
    let layers = classify(&outlines, &s);
    assert!(!layers[10].bridges.is_empty());
    assert!(!layers[11].bridges.is_empty());
    assert!(layers[12].bridges.is_empty());
}
#[test]
fn orca_bridge_line_width_sets_the_bridge_flow() {
    let mut s = PrintSettings::core_one_hf04_speed_pla();
    let normal = Flow::new(0.45, 0.2);
    s.bridge_flow_ratio = 1.0;
    s.thick_bridges = false;
    s.orca_walls.bridge_line_width = Some(crate::settings::FloatOrPercent::Percent(100.));
    let flow = BridgeFlow::new(&s, normal, false);
    assert!((flow.extrusion.width - s.nozzle_diameter).abs() < 1e-12);
    assert!((flow.spacing - Flow::new(s.nozzle_diameter, 0.2).spacing()).abs() < 1e-12);
    s.thick_bridges = true;
    s.orca_walls.bridge_line_width = Some(crate::settings::FloatOrPercent::Mm(0.5));
    assert!((BridgeFlow::new(&s, normal, false).extrusion.width - 0.5).abs() < 1e-12);
}

#[test]
fn orca_keeps_small_top_bottom_cracks_as_top() {
    // A one-layer 0.5 mm ledge on top of a block: both top and bottom (a "crack").
    let outlines = vec![
        vec![rect(0., 0., 20., 20.)],
        vec![rect(0., 0., 20., 20.)],
        vec![rect(0., 0., 20.5, 20.)],
    ];
    let flow = Flow::new(0.45, 0.2);
    let layers: Vec<_> = (0..outlines.len())
        .map(|i| crate::shells::ShellLayer {
            print_z: (i + 1) as f64 * 0.2,
            height: 0.2,
            external: flow,
            perimeter: flow,
            solid: flow,
        })
        .collect();
    let ledge_top = |s: &PrintSettings| {
        let surfaces = crate::shells::classify(&outlines, outlines.clone(), &layers, s);
        let top: f64 = surfaces[2].top.iter().map(ExPolygon::area_mm2).sum();
        let bridge: f64 = surfaces[2]
            .bottom_bridge
            .iter()
            .map(ExPolygon::area_mm2)
            .sum();
        (top, bridge)
    };
    let mut s = PrintSettings::core_one_hf04_speed_pla();
    let (prusa_top, prusa_bridge) = ledge_top(&s);
    assert!(prusa_bridge > 5.0, "{prusa_bridge}");
    assert!(prusa_top < 400.0);
    s.minimum_sparse_infill_area = Some(15.0);
    let (orca_top, orca_bridge) = ledge_top(&s);
    assert!(orca_bridge < 0.01, "{orca_bridge}");
    assert!(orca_top > 409.0, "{orca_top}");
}
