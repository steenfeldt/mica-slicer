// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Behaviour tests for the fill patterns.

use super::*;
use crate::geometry::clip::{diff_ex, intersection_pl, union_ex_positive};
use crate::geometry::{Polygon, polyline_length, scale};

const SPACING: f64 = 0.40708;

#[test]
fn disconnected_monotonic_surfaces_are_chained_without_reordering_their_scanlines() {
    let far = ExPolygon {
        contour: rect(0., 0., 10., 10.),
        holes: vec![],
    };
    let near = ExPolygon {
        contour: rect(100., 0., 110., 10.),
        holes: vec![],
    };
    let s =
        crate::settings::PrintSettings::from_config(&crate::config::Config::defaults()).unwrap();
    let p = params(1.);
    let from = Point::from_mm(100., 0.);
    let make = |area: &[ExPolygon]| {
        toolpaths(
            Pattern::Monotonic,
            area,
            &p,
            crate::flow::Flow::new(0.45, 0.2),
            crate::toolpath::Role::TopSolidInfill,
            &s,
            None,
            from,
        )
    };
    let near_paths = make(std::slice::from_ref(&near));
    let far_paths = make(std::slice::from_ref(&far));
    let together = make(&[far, near]);
    assert_eq!(&together[..near_paths.len()], near_paths);
    assert_eq!(&together[near_paths.len()..], far_paths);
}

fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Polygon {
    vec![
        Point::from_mm(x0, y0),
        Point::from_mm(x1, y0),
        Point::from_mm(x1, y1),
        Point::from_mm(x0, y1),
    ]
}

fn square(size: f64) -> Vec<ExPolygon> {
    union_ex_positive(&[rect(0.0, 0.0, size, size)])
}

/// A 20 mm square with a 6 mm square hole off-centre.
fn holed() -> Vec<ExPolygon> {
    diff_ex(
        &square(20.0),
        &union_ex_positive(&[rect(5.0, 8.0, 11.0, 14.0)]),
    )
}

fn params(density: f64) -> FillParams {
    FillParams {
        spacing: SPACING,
        density,
        angle: 45.0,
        layer_id: 0,
        z: 1.0,
        anchor_length: 1000.0,
        anchor_length_max: 1000.0,
        dont_adjust: false,
        link_max_length: 0.0,
        origin: Point::default(),
    }
}

fn sparse(density: f64) -> FillParams {
    FillParams {
        anchor_length: 2.0,
        anchor_length_max: 12.0,
        ..params(density)
    }
}

fn lines(f: &[Filled]) -> Vec<Polyline> {
    f.iter().flat_map(|f| f.polylines.clone()).collect()
}

fn length_mm(pl: &[Polyline]) -> f64 {
    pl.iter().map(|p| polyline_length(p)).sum::<f64>() / SCALE
}

fn area_mm2(ex: &[ExPolygon]) -> f64 {
    ex.iter().map(ExPolygon::area_mm2).sum()
}

/// All extrusion lies inside `area` (with 1 µm tolerance).
fn assert_inside(pl: &[Polyline], area: &[ExPolygon]) {
    let grown = offset_ex(area, scale(0.001) as f64, Join::Miter);
    let inside = length_mm(&intersection_pl(pl, &grown));
    let total = length_mm(pl);
    assert!(
        (inside - total).abs() < 1e-3 * total.max(1.0),
        "{total} mm of lines, {inside} mm inside"
    );
}

#[test]
fn solid_patterns_cover_the_area() {
    for pattern in [
        Pattern::Rectilinear,
        Pattern::Monotonic,
        Pattern::MonotonicLines,
    ] {
        for area in [square(20.0), holed()] {
            let filled = fill(pattern, &area, &params(1.0));
            let pl = lines(&filled);
            // The spacing may have been widened to fit the area evenly.
            let spacing = filled[0].spacing;
            assert!((SPACING..SPACING * 1.05).contains(&spacing));
            // Lines are centred half a spacing inside the area: length × spacing ≈ area minus that rim.
            let covered = length_mm(&pl) * spacing;
            let a = area_mm2(&area);
            assert!(
                (covered - a).abs() < 0.05 * a,
                "{pattern:?}: covered {covered} of {a}"
            );
            assert_inside(&pl, &area);
        }
    }
}

#[test]
fn sparse_patterns_are_proportional_to_density() {
    let area = square(20.0);
    for pattern in [Pattern::Rectilinear, Pattern::Grid, Pattern::Gyroid] {
        let l15 = length_mm(&lines(&fill(pattern, &area, &sparse(0.15))));
        let l30 = length_mm(&lines(&fill(pattern, &area, &sparse(0.30))));
        let expected = 400.0 * 0.15 / SPACING;
        assert!(
            l15 > 0.8 * expected && l15 < 1.35 * expected,
            "{pattern:?}: {l15} mm at 15%, expected about {expected}"
        );
        let ratio = l30 / l15;
        assert!(
            (1.6..2.3).contains(&ratio),
            "{pattern:?}: 30% / 15% length ratio {ratio}"
        );
    }
}

#[test]
fn sparse_patterns_stay_inside() {
    for pattern in [
        Pattern::Rectilinear,
        Pattern::Grid,
        Pattern::Triangles,
        Pattern::Cubic,
        Pattern::Gyroid,
    ] {
        let area = holed();
        let pl = lines(&fill(pattern, &area, &sparse(0.2)));
        assert!(!pl.is_empty(), "{pattern:?} is empty");
        assert_inside(&pl, &area);
    }
}

#[test]
fn rectilinear_alternates_direction_per_layer() {
    let area = square(20.0);
    let direction = |layer_id: usize| {
        let p = FillParams {
            layer_id,
            angle: 0.0,
            ..params(1.0)
        };
        // The longest straight segment of the fill gives the line direction.
        let pl = lines(&fill(Pattern::Rectilinear, &area, &p));
        let (a, b) = pl
            .iter()
            .flat_map(|p| p.windows(2).map(|w| (w[0], w[1])))
            .max_by_key(|(a, b)| a.distance_sq(*b))
            .unwrap();
        ((b.y - a.y) as f64)
            .atan2((b.x - a.x) as f64)
            .to_degrees()
            .rem_euclid(180.0)
    };
    // fill_angle 0: horizontal on even layers, vertical on odd ones.
    assert!(direction(0).min(180.0 - direction(0)) < 1.0);
    assert!((direction(1) - 90.0).abs() < 1.0);
}

#[test]
fn grid_lines_cross_and_keep_their_angle() {
    let area = square(20.0);
    for layer_id in [0, 1] {
        let p = FillParams {
            layer_id,
            ..sparse(0.15)
        };
        let pl = lines(&fill(Pattern::Grid, &area, &p));
        let mut dirs: Vec<i64> = pl
            .iter()
            .flat_map(|p| p.windows(2))
            // Long segments off the boundary (the joints along the square's edges are axis-aligned).
            .filter(|w| {
                (w[0].x - w[1].x).abs() > scale(2.0) && (w[0].y - w[1].y).abs() > scale(2.0)
            })
            .map(|w| {
                let d = ((w[1].y - w[0].y) as f64).atan2((w[1].x - w[0].x) as f64);
                (d.to_degrees().rem_euclid(180.0)).round() as i64
            })
            .collect();
        dirs.sort_unstable();
        dirs.dedup();
        // Two sweeps at 45° and 135°, on every layer.
        assert_eq!(dirs, [45, 135], "layer {layer_id}");
    }
}

#[test]
fn monotonic_lines_follow_left_to_right_order() {
    // fill_angle 0, layer 0: lines are horizontal and must be printed bottom to top, each line only
    // after the overlapping line just below it.
    for pattern in [Pattern::Monotonic, Pattern::MonotonicLines] {
        let area = holed();
        let p = FillParams {
            angle: 0.0,
            ..params(1.0)
        };
        let pl = lines(&fill(pattern, &area, &p));
        // Horizontal runs in print order: (y, x_min, x_max).
        let mut runs = Vec::new();
        for p in &pl {
            for w in p.windows(2) {
                // Upstream's f32 angle can tilt an axis-aligned run by two scaled units.
                if (w[0].y - w[1].y).abs() <= 2 && (w[0].x - w[1].x).abs() > scale(1.0) {
                    runs.push((w[0].y, w[0].x.min(w[1].x), w[0].x.max(w[1].x)));
                }
            }
        }
        assert!(runs.len() > 40);
        let step = scale(SPACING * 1.1);
        for (k, &(y, x0, x1)) in runs.iter().enumerate() {
            for &(y2, a0, a1) in &runs[k + 1..] {
                // A later run directly below and overlapping this one would break monotonicity.
                let below = y2 < y && y - y2 <= step;
                let overlaps = a0 < x1 && a1 > x0;
                assert!(
                    !(below && overlaps),
                    "{pattern:?}: run at y {y2} printed after y {y}"
                );
            }
        }
    }
}

#[test]
fn monotonic_lines_are_not_linked() {
    let area = square(20.0);
    let pl = lines(&fill(Pattern::MonotonicLines, &area, &params(1.0)));
    assert!(pl.iter().all(|p| p.len() == 2));
    let linked = lines(&fill(Pattern::Monotonic, &area, &params(1.0)));
    // A convex area is one monotonic region: one zig-zag.
    assert_eq!(linked.len(), 1);
}

#[test]
fn gyroid_is_continuous() {
    let area = square(20.0);
    let p = FillParams {
        z: 3.3,
        ..sparse(0.2)
    };
    let pl = lines(&fill(Pattern::Gyroid, &area, &p));
    // Waves are smooth: no long jumps between consecutive points, except for the joints along the
    // (axis-aligned) boundary.
    for p in &pl {
        for w in p.windows(2) {
            let along_boundary = w[0].x == w[1].x || w[0].y == w[1].y;
            assert!(along_boundary || w[0].distance_to(w[1]) < scale(1.0) as f64);
        }
    }
    // Connected along the boundary into a few long paths.
    let total = length_mm(&pl);
    assert!(pl.len() <= 4, "{} gyroid paths", pl.len());
    assert!(total > 100.0);
    // Different layers are different slices of the surface.
    let q = FillParams {
        z: 3.5,
        ..sparse(0.2)
    };
    assert_ne!(pl, lines(&fill(Pattern::Gyroid, &area, &q)));
}

#[test]
fn grid_line_ends_are_anchored_along_the_boundary() {
    let area = square(20.0);
    let unconnected = FillParams {
        anchor_length: 0.0,
        anchor_length_max: 0.0,
        ..sparse(0.15)
    };
    let plain = lines(&fill(Pattern::Grid, &area, &unconnected));
    assert!(plain.iter().all(|p| p.len() == 2));

    // Short anchors only: line ends get hooks of at most 1 mm along the boundary, or are joined with a
    // neighbour less than 1 mm away. Either way, each line end adds at most 1 mm.
    let hooks = FillParams {
        anchor_length: 1.0,
        anchor_length_max: 1.0,
        ..sparse(0.15)
    };
    let hooked = lines(&fill(Pattern::Grid, &area, &hooks));
    assert!(hooked.len() <= plain.len());
    let added = length_mm(&hooked) - length_mm(&plain);
    assert!(
        added > 0.5 * plain.len() as f64 && added <= 2.0 * plain.len() as f64 + 1e-3,
        "{added} mm added to {} lines",
        plain.len()
    );

    // With the CORE One anchors (2 mm, connect up to 12 mm) neighbouring lines join into long paths.
    let joined = lines(&fill(Pattern::Grid, &area, &sparse(0.15)));
    assert!(joined.len() < plain.len() / 2);
    assert!(length_mm(&joined) > length_mm(&plain));
    // Every joint runs along the boundary of the shrunk area (half a line in).
    let rim = offset_ex(&area, -0.5 * SPACING * SCALE, Join::Miter);
    for p in &joined {
        for w in p.windows(2) {
            let along_edge = |a: Point, b: Point| {
                let on = |c: i64, lo: i64, hi: i64| (c - lo).abs() < 10 || (c - hi).abs() < 10;
                let bb = crate::geometry::BoundingBox::from_points(rim[0].contour.iter()).unwrap();
                ((a.x - b.x).abs() < 10 && on(a.x, bb.min.x, bb.max.x))
                    || ((a.y - b.y).abs() < 10 && on(a.y, bb.min.y, bb.max.y))
            };
            let diagonal = (w[0].x - w[1].x).abs() > 10 && (w[0].y - w[1].y).abs() > 10;
            assert!(diagonal || along_edge(w[0], w[1]), "segment {:?}", w);
        }
    }
}

/// Random star-shaped areas with rectangular holes: fills stay inside, solid fills cover the area.
#[test]
fn random_shapes_fill_inside_and_cover() {
    let mut seed: u64 = 12345;
    let mut rnd = || {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (seed >> 11) as f64 / (1u64 << 53) as f64
    };
    for iter in 0..100 {
        // Random star polygon with a random hole cut.
        let n = 5 + (rnd() * 40.0) as usize;
        let poly: Polygon = (0..n)
            .map(|i| {
                let a = i as f64 / n as f64 * std::f64::consts::TAU;
                let r = 3.0 + rnd() * 12.0;
                Point::from_mm(20.0 + r * a.cos(), 20.0 + r * a.sin())
            })
            .collect();
        let mut area = union_ex_positive(&[poly]);
        for _ in 0..(rnd() * 4.0) as usize {
            let (x, y, w) = (10.0 + rnd() * 20.0, 10.0 + rnd() * 20.0, 0.5 + rnd() * 5.0);
            area = diff_ex(
                &area,
                &union_ex_positive(&[rect(x, y, x + w, y + w * (0.3 + rnd()))]),
            );
        }
        for pattern in [
            Pattern::Rectilinear,
            Pattern::Monotonic,
            Pattern::MonotonicLines,
            Pattern::Grid,
            Pattern::Gyroid,
            Pattern::Cubic,
        ] {
            let p = FillParams {
                angle: rnd() * 180.0,
                layer_id: iter,
                z: rnd() * 30.0,
                ..if rnd() < 0.5 {
                    params(1.0)
                } else {
                    sparse(0.05 + rnd() * 0.5)
                }
            };
            let p = if matches!(pattern, Pattern::Grid | Pattern::Gyroid | Pattern::Cubic)
                && p.density >= 1.0
            {
                sparse(0.2)
            } else {
                p
            };
            let pl = lines(&fill(pattern, &area, &p));
            assert_inside(&pl, &area);
            if p.density >= 1.0 {
                let cov = length_mm(&pl) * SPACING / area_mm2(&area);
                assert!(
                    (0.85..1.1).contains(&cov),
                    "shape {iter} {pattern:?}: coverage {cov:.3}"
                );
            }
        }
    }
}
