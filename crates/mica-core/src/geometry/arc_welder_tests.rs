// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 tests/libslic3r/test_arc_welder.cpp :: arc basics, arc discretization, arc fitting, SmoothPath clipping test, arc quantization
// Changes: Rust parameterized cases, plus degenerate/reversal/length and splitting regressions.
use super::*;

fn p(x: f64, y: f64) -> Point {
    Point::from_mm(x, y)
}
fn near(a: f64, b: f64, tolerance: f64) {
    assert!((a - b).abs() < tolerance, "{a} != {b}");
}

#[test]
fn discretize_and_fit_minor_and_major_both_directions() {
    for (a, b, r, ccw) in [
        (p(2., 1.), p(1., 2.), SCALE, true),
        (p(1., 2.), p(2., 1.), -SCALE, true),
        (p(1., 2.), p(2., 1.), SCALE, false),
        (p(2., 1.), p(1., 2.), -SCALE, false),
    ] {
        let (x, y) = arc_center(a, b, r, ccw);
        near(x, SCALE, 1.0);
        near(y, SCALE, 1.0);
        let pts = arc_discretize(a, b, r, ccw, 0.002 * SCALE);
        for q in &pts {
            near(q.distance_to(p(1., 1.)), SCALE, 2.0);
        }
        let path = fit_path(&pts, 0.0021 * SCALE, LENGTH_TOLERANCE);
        assert_eq!(path.len(), 2, "{path:?}");
        assert_eq!(path[0].point, a);
        assert_eq!(path[1].point, b);
        assert_eq!(path[1].ccw, ccw);
        assert_eq!(path[1].radius.signum(), r.signum());
        near(
            path_length(&path),
            if r > 0.0 {
                PI / 2.0 * SCALE
            } else {
                1.5 * PI * SCALE
            },
            0.01 * SCALE,
        );
        for q in pts {
            assert!(
                point_to_path_projection(&path, q).unwrap().distance2 < (0.0021 * SCALE).powi(2)
            );
        }
    }
}

#[test]
fn tangential_arcs_stay_separate() {
    for (a, b, c, r, ccw) in [
        (p(2., 1.), p(1., 2.), p(0., 3.), SCALE, true),
        (p(0., 3.), p(1., 2.), p(2., 1.), -SCALE, true),
        (p(0., 3.), p(1., 2.), p(2., 1.), SCALE, false),
        (p(2., 1.), p(1., 2.), p(0., 3.), -SCALE, false),
    ] {
        let mut pts = arc_discretize(a, b, r, ccw, 0.002 * SCALE);
        pts.extend(
            arc_discretize(b, c, -r, !ccw, 0.002 * SCALE)
                .into_iter()
                .skip(1),
        );
        let path = fit_path(&pts, 0.0021 * SCALE, LENGTH_TOLERANCE);
        assert_eq!(path.len(), 3, "{path:?}");
        assert_eq!(path[1].point, b);
        assert_eq!(path[2].point, c);
        assert_eq!(path[1].ccw, ccw);
        assert_eq!(path[2].ccw, !ccw);
    }
}

#[test]
fn clip_split_reverse_major_arc_and_empty_paths() {
    let original = vec![
        Segment::line(p(1., 0.)),
        Segment {
            point: p(0., 1.),
            radius: -SCALE,
            ccw: false,
        },
    ];
    let length = path_length(&original);
    let mut rev = original.clone();
    reverse(&mut rev);
    near(path_length(&rev), length, 1.0);
    reverse(&mut rev);
    assert_eq!(rev, original);
    let mut clipped = original.clone();
    assert_eq!(clip_end(&mut clipped, PI * SCALE), 0.0);
    assert!(clipped[1].radius > 0.0);
    near(path_length(&clipped), length - PI * SCALE, 2.0);
    let (left, right) = split_at(&original, p(-1., 0.), 1.0);
    near(path_length(&left) + path_length(&right), length, 2.0);
    assert_eq!(left.last().unwrap().point, right[0].point);
    assert!(right[1].radius > 0.0);
    let mut clipped = original.clone();
    near(clip_start(&mut clipped, length + 10.), 10., 1.0);
    assert!(clipped.is_empty());
    assert_eq!(clip_end(&mut clipped, 5.), 5.);
    assert_eq!(split_at(&original, p(1., 0.), 1.).0.len(), 0);
}

#[test]
fn disabled_collinear_reversing_duplicate_and_closed_inputs() {
    assert!(fit_path(&[], 100., 0.05).is_empty());
    let pts = [p(0., 0.), p(1., 0.), p(2., 0.)];
    assert_eq!(fit_path(&pts, 0., 0.05).len(), 3);
    assert_eq!(fit_path(&pts, 100., 0.05).len(), 2);
    let mut pts = arc_discretize(p(1., 0.), p(0., 1.), SCALE, true, 1000.);
    let disabled = fit_path(&pts, 1000., 0.);
    assert!(disabled.iter().all(|s| s.radius == 0.));
    pts.extend(pts.clone().into_iter().rev());
    let fitted = fit_path(&pts, 1100., 0.05);
    assert!(fitted.len() >= 3);
    near(path_length(&fitted), PI * SCALE, 0.01 * SCALE);
    let pts: Vec<_> = (0..=360)
        .map(|i| {
            let a = (i as f64).to_radians();
            p(a.cos(), a.sin())
        })
        .collect();
    let path = fit_path(&pts, 0.005 * SCALE, 0.05);
    assert_eq!(path.first().unwrap().point, pts[0]);
    assert_eq!(path.last().unwrap().point, *pts.last().unwrap());
    assert!(
        path.windows(2)
            .all(|s| s[1].radius == 0.0 || s[0].point.distance_to(s[1].point) >= 0.0011 * SCALE)
    );
    near(path_length(&path), TAU * SCALE, 0.01 * SCALE);
}
