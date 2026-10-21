// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/ExtrusionProcessor.cpp :: calculate_overhang_speed
// Source: prusaslicer@b028299c77 src/libslic3r/SupportSpotsGenerator.cpp :: estimate_curled_up_height
// Changes: Numeric regression cases for interpolation, curl propagation, optional controls and streaming annotation.
use super::*;
use mica_core::{flow::Flow, geometry::Point};
fn config() -> Config {
    let mut c = Config::defaults();
    for (key, value) in [
        ("enable_dynamic_overhang_speeds", "1"),
        ("enable_dynamic_fan_speeds", "1"),
        ("overhang_speed_0", "15"),
        ("overhang_speed_1", "25"),
        ("overhang_speed_2", "50"),
        ("overhang_speed_3", "70%"),
        ("overhang_fan_speed_0", "100"),
        ("overhang_fan_speed_1", "100"),
        ("overhang_fan_speed_2", "0"),
        ("overhang_fan_speed_3", "0"),
    ] {
        c.set_raw(key, value).unwrap();
    }
    c
}
fn attrs(d: f64) -> OverhangSection {
    OverhangSection {
        end: 1,
        start_distance: d as f32,
        end_distance: d as f32,
        curled_proximity: 0.,
        speed: 0.,
    }
}
fn square(x: f64) -> ExPolygon {
    ExPolygon {
        contour: vec![
            Point::from_mm(x, 0.),
            Point::from_mm(x + 10., 0.),
            Point::from_mm(x + 10., 10.),
            Point::from_mm(x, 10.),
        ],
        holes: vec![],
    }
}
fn layer(index: usize, x: f64) -> LayerToolpaths {
    let mut points = square(x).contour;
    points.push(points[0]);
    LayerToolpaths {
        support_groups: Vec::new(),
        index,
        print_z: (index + 1) as f64 * 0.2,
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
    }
}
#[test]
fn support_knots_percent_zero_cap_and_fan_hysteresis() {
    let mut c = config();
    for (d, speed, fan) in [
        (0., 200., 0.),
        (0.1, 140., 0.),
        (0.2, 50., 0.),
        (0.3, 25., 100.),
        (0.4, 15., 100.),
        (2., 15., 100.),
    ] {
        let (s, f) = calculate_overhang_speed(&c, 0.4, attrs(d), 200., 200., None);
        assert!((s.unwrap() - speed).abs() < 1e-4, "{d}: {s:?}");
        assert!((f.unwrap() - fan).abs() < 1e-4);
    }
    let (_, fan) = calculate_overhang_speed(&c, 0.4, attrs(0.298), 200., 200., Some(100.));
    assert_eq!(fan, Some(100.));
    let (_, fan) = calculate_overhang_speed(&c, 0.4, attrs(0.296), 200., 200., Some(100.));
    assert!(fan.unwrap() < 97.);
    assert_eq!(
        calculate_overhang_speed(&c, 0.4, attrs(0.), 80., 0., None).0,
        Some(80.)
    );
    c.set_raw("overhang_speed_0", "0").unwrap();
    assert_eq!(
        calculate_overhang_speed(&c, 0.4, attrs(0.4), 80., 100., None).0,
        Some(80.)
    );
    c.set_raw("enable_dynamic_overhang_speeds", "0").unwrap();
    assert_eq!(
        calculate_overhang_speed(&c, 0.4, attrs(0.4), 80., 100., None),
        (None, Some(100.))
    );
    c.set_raw("enable_dynamic_fan_speeds", "0").unwrap();
    assert_eq!(
        calculate_overhang_speed(&c, 0.4, attrs(0.4), 80., 100., None),
        (None, None)
    );
}
#[test]
fn curl_propagates_decays_and_ignores_distant_bridges() {
    assert!((estimate_curled_height(0.3, 0., 0.2, 0.4, 0.) - 0.025).abs() < 1e-9);
    assert!((estimate_curled_height(0., 0., 0.2, 0.4, 1.) - 0.85).abs() < 1e-9);
    assert!(estimate_curled_height(0.3, 1., 0.2, 0.4, 0.) > 0.3);
    assert_eq!(estimate_curled_height(2., 1., 0.2, 0.4, 1.), 0.);
    assert_eq!(estimate_curled_height(0.3, 1., 0.2, 0.4, 10.), 2.);
    let mut a = attrs(0.);
    a.curled_proximity = 1.;
    let (s, f) = calculate_overhang_speed(&config(), 0.4, a, 200., 200., None);
    assert_eq!(s, Some(15.));
    assert_eq!(f, Some(0.));
}
#[test]
fn annotation_preserves_first_layer_skips_support_only_and_keeps_history() {
    let c = config();
    let mut processor = OverhangProcessor::default();
    let mut first = layer(0, 0.);
    processor.process(&mut first, Some(&[square(0.)]), &c);
    assert!(first.paths[0].overhang.is_none());
    let mut support = LayerToolpaths {
        index: 1,
        ..Default::default()
    };
    processor.process(&mut support, None, &c);
    let mut next = layer(2, 0.3);
    processor.process(&mut next, Some(&[square(0.3)]), &c);
    let attrs = next.paths[0].overhang.as_ref().unwrap();
    assert_eq!(
        attrs.last().unwrap().end as usize,
        next.paths[0].points.len() - 1
    );
    assert!(attrs.iter().any(|a| a.start_distance > 0.4));
    assert!(attrs.iter().any(|a| a.start_distance < 0.));
    for a in attrs.iter() {
        assert!(a.start_distance.is_finite() && a.curled_proximity.is_finite());
    }
}
#[test]
fn cooling_fan_limits_and_reset() {
    let c = crate::cooling::CoolingConfig {
        slowdown_below_layer_time: 8.,
        fan_below_layer_time: 40.,
        min_print_speed: 20.,
        min_fan_speed: 35.,
        max_fan_speed: 80.,
        disable_fan_first_layers: 1,
        full_fan_speed_layer: 3,
        fan_always_on: true,
        ..crate::cooling::CoolingConfig::default()
    };
    use crate::cooling::requested_fan_speed as fan;
    assert_eq!(fan(0, 100., &c, Some(100.), None), 0.);
    assert_eq!(fan(1, 100., &c, Some(100.), None), 18.);
    assert_eq!(fan(3, 100., &c, Some(0.), None), 35.);
    assert_eq!(fan(3, 100., &c, Some(100.), None), 100.);
    assert_eq!(fan(3, 100., &c, None, None), 35.);
    assert_eq!(fan(3, 1., &c, Some(10.), None), 80.);
    assert_eq!(fan(3, 100., &c, None, Some(100.)), 100.);
}
#[test]
fn long_line_curl_coverage_and_scarf_interpolation() {
    let short = Line {
        a: [4., 0.],
        b: [5., 0.],
    };
    let segment = Line {
        a: [0., 0.],
        b: [10., 0.],
    };
    assert_eq!(
        proximity(segment, 0.4, 0.2, &Lines::new(vec![short]), &[(short, 1.)]),
        0.
    );
    let long = Line {
        a: [0., 0.],
        b: [10., 0.],
    };
    assert_eq!(
        proximity(segment, 0.4, 0.2, &Lines::new(vec![long]), &[(long, 1.)]),
        0.5
    );
    let c = config();
    let mut proc = OverhangProcessor::default();
    let mut first = layer(0, 0.);
    proc.process(&mut first, Some(&[square(0.)]), &c);
    let mut next = layer(1, 0.3);
    next.paths[0].seam = Some(Box::new(mica_core::seam::PathSeam {
        samples: (0..5)
            .map(|i| ScarfSample {
                height: i as f64 / 4.,
                extrusion: i as f64 / 4.,
            })
            .collect(),
        wipe: None,
        original_clockwise: false,
        loop_length_mm: None,
        ..Default::default()
    }));
    let volume = |p: &ExtrusionPath| {
        p.points
            .windows(2)
            .zip(&p.seam.as_ref().unwrap().samples[1..])
            .map(|(edge, sample)| {
                edge[0].distance_to(edge[1]) / mica_core::geometry::SCALE * sample.extrusion
            })
            .sum::<f64>()
    };
    let before = volume(&next.paths[0]);
    proc.process(&mut next, Some(&[square(0.3)]), &c);
    let path = &next.paths[0];
    assert_eq!(path.points.len(), path.seam.as_ref().unwrap().samples.len());
    assert!(path.points.len() > 5);
    assert!((volume(path) - before).abs() < 1e-5);
}
