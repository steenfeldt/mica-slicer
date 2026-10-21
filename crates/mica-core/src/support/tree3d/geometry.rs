// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: prusaslicer@b028299c77 src/libslic3r/Support/TreeSupport.cpp :: safe_union, safe_offset_inc, move_inside
// Source: orca@8500fcdcca src/libslic3r/Support/TreeSupport3D.cpp :: safe_union, safe_offset_inc, move_inside
// Source: prusaslicer@b028299c77 src/libslic3r/Polygon.cpp :: make_circle, make_circle_num_segments
// Source: prusaslicer@b028299c77 src/libslic3r/Polygon.cpp :: polygons_simplify, simplify_polygon_impl
// Changes: Rust owned geometry and deterministic serial scheduling; ClipperUtils offsets on the Clipper 6.4 offsetter port, booleans with Clipper 6.4 intersection rounding, polygons_simplify as Douglas-Peucker + non-zero SimplifyPolygons.
// Copyright (c) Prusa Research 2022 - 2023 Vojtěch Bubník @bubnikv, Lukáš Matěna @lukasmatena, Tomáš Mészáros @tamasmeszaros
// Copyright (c) Prusa Research 2016 - 2023 Vojtěch Bubník @bubnikv, Filip Sykala @Jony01, Lukáš Matěna @lukasmatena, Tomáš Mészáros @tamasmeszaros, Enrico Turri @enricoturri1966
// Copyright (c) Slic3r 2013 - 2015 Alessandro Ranellucci @alranel
// Copyright (c) 2014 Petr Ledvina @ledvinap
// Copyright (c) Prusa Research 2017 - 2022 Vojtěch Bubník @bubnikv
// Copyright (c) Slic3r 2011 - 2014 Alessandro Ranellucci @alranel
// Copyright (c) 2012 Mark Hindess
// PrusaSlicer is released under the terms of the AGPLv3 or higher.
// Tree supports by Thomas Rahm, losely based on Tree Supports by CuraEngine.
// Original source of Thomas Rahm's tree supports: https://github.com/ThomasRahm/CuraEngine
// Copyright (c) 2021 Ultimaker B.V.
// CuraEngine is released under the terms of the AGPLv3 or higher.

use super::{Areas, SUPPORT_TREE_CIRCLE_RESOLUTION, scaled};
use crate::geometry::{
    BoundingBox, ExPolygon, Point, Polyline,
    clip::{self, Join},
    clip64, signed_area,
    simplify::douglas_peucker,
};

pub(super) const EPS: i64 = 100;

// ClipperUtils' `union_` / `diff` / `intersection` on Polygons (holes clockwise, non-zero
// fill) run on Clipper 6.4, which rounds edge intersections where Clipper2 truncates them.
pub(super) fn union64(a: &[ExPolygon]) -> Areas {
    clip64::boolean_ex(clipper2_rust::ClipType::Union, a, &[])
}
pub(super) fn diff64(a: &[ExPolygon], b: &[ExPolygon]) -> Areas {
    clip64::boolean_ex(clipper2_rust::ClipType::Difference, a, b)
}
pub(super) fn intersection64(a: &[ExPolygon], b: &[ExPolygon]) -> Areas {
    clip64::boolean_ex(clipper2_rust::ClipType::Intersection, a, b)
}
pub(super) fn area(a: &[ExPolygon]) -> f64 {
    a.iter()
        .map(|p| signed_area(&p.contour) + p.holes.iter().map(|h| signed_area(h)).sum::<f64>())
        .sum()
}
pub(super) fn miter(a: &[ExPolygon], d: f64) -> Areas {
    clip64::offset_polygons_ex(a, d, Join::Miter, 1.2)
}
pub(super) fn round(a: &[ExPolygon], d: i64) -> Areas {
    clip64::offset_polygons_ex(a, d as f64, Join::Round, scaled(0.01) as f64)
}
pub(super) fn outlines(a: &[ExPolygon]) -> Vec<Polyline> {
    a.iter()
        .flat_map(|ex| ex.polygons())
        .map(|p| {
            let mut line = p.clone();
            if let Some(&first) = p.first() {
                line.push(first);
            }
            line
        })
        .collect()
}
pub(super) fn circle(center: Point, radius: i64) -> Areas {
    let segments = (std::f64::consts::PI
        / (1. - SUPPORT_TREE_CIRCLE_RESOLUTION as f64 / radius as f64).acos())
    .ceil()
    .max(3.) as usize;
    vec![ExPolygon {
        contour: (0..segments)
            .map(|i| {
                let angle = std::f64::consts::TAU * i as f64 / segments as f64;
                Point::new(
                    center.x + (angle.cos() * radius as f64) as i64,
                    center.y + (angle.sin() * radius as f64) as i64,
                )
            })
            .collect(),
        holes: vec![],
    }]
}
/// `polygons_simplify(polygons, tolerance, strictly_simple = false)` (Polygon.cpp
/// `simplify_polygon_impl`): Douglas-Peucker on each ring closed by repeating its first
/// point, then `ClipperLib::SimplifyPolygons(pftNonZero)` of the decimated ring, holes
/// kept clockwise. The resulting polygon set is read back with the non-zero rule, as
/// every upstream consumer of these `Polygons` does.
pub fn polygons_simplify(a: &[ExPolygon], tolerance: f64) -> Areas {
    use clipper2_rust::{ClipType, FillRule, Paths64, Point64};
    let mut out: Vec<crate::geometry::Polygon> = vec![];
    for ring in a.iter().flat_map(|ex| ex.polygons()) {
        if ring.is_empty() {
            continue;
        }
        let mut closed = ring.clone();
        closed.push(ring[0]);
        let mut simplified = douglas_peucker(&closed, tolerance);
        simplified.pop();
        let ccw = signed_area(&simplified) > 0.;
        let path: Paths64 = vec![simplified.iter().map(|p| Point64::new(p.x, p.y)).collect()];
        for mut p in clip64::execute(ClipType::Union, &path, &Paths64::new(), FillRule::NonZero) {
            if !ccw {
                p.reverse();
            }
            out.push(p.into_iter().map(|q| Point::new(q.x, q.y)).collect());
        }
    }
    clip64::union_ex(&out, clipper2_rust::FillRule::NonZero)
}

pub(super) fn safe_union(a: &[ExPolygon], b: &[ExPolygon]) -> Areas {
    let all: Areas = a.iter().chain(b).cloned().collect();
    let result = union64(&all);
    if !all.is_empty() && result.is_empty() {
        // Buffer surviving boundary lines with the upstream jtMiter/1.2.
        union64(&offset_lines_miter(&outlines(&all), scaled(0.002) as f64))
    } else {
        result
    }
}

/// Incremental round offsets, subtracting obstacles between steps so an area
/// cannot tunnel through a wall. `last_unchecked` is followed by the caller's
/// collision/avoidance subtraction, unless zero requests it here.
pub fn safe_offset_inc(
    a: &[ExPolygon],
    distance: i64,
    collision: &[ExPolygon],
    safe_step: i64,
    last_unchecked: i64,
    min_offsets: usize,
) -> Areas {
    let mut final_difference = last_unchecked == 0;
    let mut ret = safe_union(a, &[]);
    if distance == 0 || safe_step < 0 || last_unchecked < 0 {
        return if final_difference {
            diff64(&ret, collision)
        } else {
            union64(&ret)
        };
    }
    // Negative expansion produces no offset in the pinned implementation.
    if distance < 0 {
        ret = polygons_simplify(&ret, scaled(0.015) as f64);
        return union64(&if final_difference {
            diff64(&ret, collision)
        } else {
            ret
        });
    }
    // Prusa's zero divisor is undefined; Orca guards its callers. Reject other
    // invalid direct requests explicitly instead of silently tunneling.
    assert!(
        safe_step > 0 && distance > 0,
        "safe_offset_inc needs a positive step and nonnegative distance"
    );
    let mut step = safe_step;
    let mut steps = if distance > last_unchecked {
        (distance - last_unchecked) / step
    } else {
        0
    };
    if distance - steps * step > last_unchecked {
        if (steps + 1) * step <= distance {
            steps += 1;
        } else {
            final_difference = true;
        }
    }
    if steps + i64::from(distance < last_unchecked || distance % step != 0) < min_offsets as i64
        && min_offsets > 1
    {
        step = distance / min_offsets as i64;
        // Defined behavior for integer-scale requests smaller than min_offsets.
        step = step.max(1);
        if step >= safe_step {
            step = safe_step;
            steps = min_offsets as i64;
        } else {
            steps = distance / step;
        }
    }
    for i in 0..steps {
        ret = diff64(&round(&ret, step), collision);
        if i % 10 == 7 {
            ret = polygons_simplify(&ret, scaled(0.015) as f64);
        }
    }
    let remainder = distance - steps * step;
    if remainder > EPS {
        ret = round(&ret, remainder);
    }
    ret = polygons_simplify(&ret, scaled(0.015) as f64);
    if final_difference {
        ret = diff64(&ret, collision);
    }
    union64(&ret)
}

pub(super) fn contains(a: &[ExPolygon], p: Point) -> bool {
    use clipper2_rust::{Point64, PointInPolygonResult, point_in_polygon};
    let test = |poly: &[Point]| {
        point_in_polygon(
            Point64::new(p.x, p.y),
            &poly.iter().map(|q| Point64::new(q.x, q.y)).collect(),
        )
    };
    a.iter().any(|ex| {
        let outer = test(&ex.contour);
        outer == PointInPolygonResult::IsOn
            || (outer == PointInPolygonResult::IsInside
                && !ex
                    .holes
                    .iter()
                    .any(|h| test(h) == PointInPolygonResult::IsInside))
    })
}

/// All active upstream callers use distance=0 and unlimited maxDist2. Preserve
/// boundary traversal, closest-point ties, and truncation of the projection.
pub(super) fn move_inside_if_outside(a: &[ExPolygon], from: Point) -> Point {
    if contains(a, from) {
        return from;
    }
    let mut best = from;
    let mut best_dist = i128::MAX;
    for poly in a
        .iter()
        .flat_map(|ex| ex.polygons())
        .filter(|p| p.len() >= 2)
    {
        let p0 = poly[poly.len() - 2];
        let mut p1 = poly[poly.len() - 1];
        let dot = |a: Point, b: Point, c: Point| {
            (b.x - a.x) as i128 * (c.x - a.x) as i128 + (b.y - a.y) as i128 * (c.y - a.y) as i128
        };
        let mut beyond_prev = dot(p0, p1, from) >= p0.distance_sq(p1);
        for &p2 in poly {
            let length = p1.distance_sq(p2);
            if length == 0 {
                p1 = p2;
                continue;
            }
            let projection = dot(p1, p2, from);
            let candidate = if projection <= 0 {
                if beyond_prev {
                    beyond_prev = false;
                    Some(p1)
                } else {
                    None
                }
            } else if projection >= length {
                beyond_prev = true;
                None
            } else {
                beyond_prev = false;
                let t = projection as f64 / length as f64;
                Some(Point::new(
                    p1.x + ((p2.x - p1.x) as f64 * t) as i64,
                    p1.y + ((p2.y - p1.y) as f64 * t) as i64,
                ))
            };
            if let Some(p) = candidate {
                let d = p.distance_sq(from);
                if d < best_dist {
                    best_dist = d;
                    best = p;
                }
            }
            p1 = p2;
        }
    }
    best
}
pub(super) fn bbox(a: &[ExPolygon]) -> Option<BoundingBox> {
    BoundingBox::from_points(a.iter().flat_map(|e| e.polygons()).flatten())
}

// Clipper2 adapter for the two upstream recovery buffers. Retain miter joins,
// 1.2 miter limit and butt ends rather than the general wrapper's square joins.
pub(super) fn offset_lines_miter(lines: &[Polyline], delta: f64) -> Areas {
    use clipper2_rust::{EndType, JoinType, Point64, inflate_paths_64};
    let paths = lines
        .iter()
        .filter(|p| p.len() >= 2)
        .map(|line| line.iter().map(|p| Point64::new(p.x, p.y)).collect())
        .collect();
    let paths = inflate_paths_64(&paths, delta, JoinType::Miter, EndType::Butt, 1.2, 0.0);
    let polygons: Vec<_> = paths
        .into_iter()
        .map(|line| line.into_iter().map(|p| Point::new(p.x, p.y)).collect())
        .collect();
    clip::union_ex_positive(&polygons)
}
