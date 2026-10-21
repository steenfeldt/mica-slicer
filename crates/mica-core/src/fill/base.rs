// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: prusaslicer@b028299c77 src/libslic3r/Fill/FillBase.cpp :: Fill::_adjust_solid_spacing
// Source: prusaslicer@b028299c77 src/libslic3r/Point.hpp :: align_to_grid, lerp
// Source: prusaslicer@b028299c77 src/libslic3r/MultiPoint.cpp :: MultiPoint::rotate, MultiPoint::remove_duplicate_points
// Source: prusaslicer@b028299c77 src/libslic3r/Polygon.cpp :: remove_sticks, remove_small
// Source: prusaslicer@b028299c77 src/libslic3r/ShortestPath.cpp :: chain_polylines
// Changes: Rust port of the small helpers the fillers share. chain_polylines forwards to the full port in
// Changes: shortest_path; greedy_chain keeps Mica's earlier nearest-end chaining for the callers that
// Changes: order lines before connect_infill or from a start point (the Orca path).

//! Helpers shared by the fill patterns.

use crate::geometry::{BoundingBox, Coord, Point, Polygon, Polyline, signed_area};

/// PrusaSlicer's `EPSILON` (unscaled).
pub(crate) const EPSILON: f64 = 1e-4;
/// PrusaSlicer's `SCALED_EPSILON`: `EPSILON` in scaled units.
pub(crate) const SCALED_EPSILON: Coord = 100;

/// Rotates a point about the origin, rounding like `MultiPoint::rotate(cos, sin)`.
pub(crate) fn rotate_point(p: Point, cos: f64, sin: f64) -> Point {
    let (x, y) = (p.x as f64, p.y as f64);
    Point::new(
        cos.mul_add(x, -sin * y).round() as Coord,
        cos.mul_add(y, sin * x).round() as Coord,
    )
}

/// Rotates points in place about the origin by `angle` radians.
pub(crate) fn rotate_points(points: &mut [Point], angle: f64) {
    let (sin, cos) = angle.sin_cos();
    for p in points {
        *p = rotate_point(*p, cos, sin);
    }
}

/// `MultiPoint::remove_duplicate_points` for an open polyline: drops consecutive duplicates.
pub(crate) fn remove_duplicate_points(pl: &mut Polyline) {
    pl.dedup();
}

/// `Polygon::remove_duplicate_points`: also drops a last point equal to the first.
pub(crate) fn remove_duplicate_points_polygon(poly: &mut Polygon) {
    poly.dedup();
    while poly.len() >= 2 && poly.first() == poly.last() {
        poly.pop();
    }
}

fn is_stick(p1: Point, p2: Point, p3: Point) -> bool {
    let (v1x, v1y) = (p2.x - p1.x, p2.y - p1.y);
    let (v2x, v2y) = (p3.x - p2.x, p3.y - p2.y);
    let dir = v1x as i128 * v2x as i128 + v1y as i128 * v2y as i128;
    if dir > 0 {
        return false;
    }
    let l2_1 = v1x as f64 * v1x as f64 + v1y as f64 * v1y as f64;
    let l2_2 = v2x as f64 * v2x as f64 + v2y as f64 * v2y as f64;
    if dir == 0 {
        return l2_1 == 0.0 || l2_2 == 0.0;
    }
    let cross = v1x as f64 * v2y as f64 - v2x as f64 * v1y as f64;
    let dist2 = cross * cross / l2_1.max(l2_2);
    dist2 < EPSILON * EPSILON
}

/// Removes spikes (vertices where the outline turns straight back). Returns whether anything changed.
pub(crate) fn remove_sticks(poly: &mut Polygon) -> bool {
    let mut modified = false;
    let mut j = 1;
    let mut i = 1;
    while i + 1 < poly.len() {
        if !is_stick(poly[j - 1], poly[i], poly[i + 1]) {
            if j < i {
                poly[j] = poly[i];
            }
            j += 1;
        }
        i += 1;
    }
    j += 1;
    if j < poly.len() {
        let last = *poly.last().unwrap();
        poly[j - 1] = last;
        poly.truncate(j);
        modified = true;
    }
    while poly.len() >= 3 && is_stick(poly[poly.len() - 2], poly[poly.len() - 1], poly[0]) {
        poly.pop();
        modified = true;
    }
    while poly.len() >= 3 && is_stick(poly[poly.len() - 1], poly[0], poly[1]) {
        poly.remove(0);
    }
    modified
}

/// `remove_sticks(Polygons&)`: also drops polygons left with fewer than three points.
pub(crate) fn remove_sticks_all(polys: &mut Vec<Polygon>) -> bool {
    let mut modified = false;
    for p in polys.iter_mut() {
        modified |= remove_sticks(p);
    }
    polys.retain(|p| p.len() >= 3);
    modified
}

/// Drops polygons with an absolute area below `min_area` (scaled units²).
pub(crate) fn remove_small(polys: &mut Vec<Polygon>, min_area: f64) {
    polys.retain(|p| signed_area(p).abs() >= min_area);
}

/// Calculates a new spacing to fill `width` with an integer number of lines, the first and last line
/// centred at the interval ends. Never decreases the spacing, and increases it by at most 20%.
pub(crate) fn adjust_solid_spacing(width: Coord, distance: Coord) -> Coord {
    let number_of_intervals = ((width as f64 - EPSILON) / distance as f64) as Coord;
    let mut distance_new = if number_of_intervals == 0 {
        distance
    } else {
        ((width as f64 - EPSILON) / number_of_intervals as f64) as Coord
    };
    let factor = distance_new as f64 / distance as f64;
    const FACTOR_MAX: f64 = 1.2;
    if factor > FACTOR_MAX {
        distance_new = (distance as f64 * FACTOR_MAX + 0.5).floor() as Coord;
    }
    distance_new
}

/// Aligns a coordinate down to a multiple of `spacing` (rounding towards minus infinity).
pub(crate) fn align_to_grid(coord: Coord, spacing: Coord) -> Coord {
    if coord < 0 {
        ((coord - spacing + 1) / spacing) * spacing
    } else {
        (coord / spacing) * spacing
    }
}

/// Aligns a point down to the grid `spacing` anchored at `base`.
pub(crate) fn align_point_to_grid(p: Point, spacing: Point, base: Point) -> Point {
    Point::new(
        base.x + align_to_grid(p.x - base.x, spacing.x),
        base.y + align_to_grid(p.y - base.y, spacing.y),
    )
}

/// Linear interpolation between two points, truncating like PrusaSlicer's `lerp(Point, Point, t)`.
pub(crate) fn lerp(a: Point, b: Point, t: f64) -> Point {
    Point::new(
        ((1.0 - t) * a.x as f64 + t * b.x as f64) as Coord,
        ((1.0 - t) * a.y as f64 + t * b.y as f64) as Coord,
    )
}

pub(crate) fn bbox_of<'a>(points: impl IntoIterator<Item = &'a Point>) -> Option<BoundingBox> {
    BoundingBox::from_points(points)
}

pub(crate) fn merge_point(bb: &mut BoundingBox, p: Point) {
    bb.min.x = bb.min.x.min(p.x);
    bb.min.y = bb.min.y.min(p.y);
    bb.max.x = bb.max.x.max(p.x);
    bb.max.y = bb.max.y.max(p.y);
}

/// `chain_polylines(polylines)`: PrusaSlicer's (and Orca's) greedy chaining with chain flipping and
/// two-exchange improvement; empty polylines are dropped.
pub(crate) fn chain_polylines(mut polylines: Vec<Polyline>) -> Vec<Polyline> {
    polylines.retain(|p| !p.is_empty());
    crate::shortest_path::chain_polylines(polylines, None)
}

/// Orders polylines greedily by the nearest end point, reversing them where that is shorter; starts at
/// `start` (or the first polyline's start). Not an upstream algorithm.
pub(crate) fn greedy_chain(mut polylines: Vec<Polyline>, start: Option<Point>) -> Vec<Polyline> {
    polylines.retain(|p| !p.is_empty());
    let mut out = Vec::with_capacity(polylines.len());
    let Some(mut from) = start.or_else(|| polylines.first().map(|p| p[0])) else {
        return out;
    };
    while !polylines.is_empty() {
        let mut best = (0, false, i128::MAX);
        for (i, l) in polylines.iter().enumerate() {
            let d0 = from.distance_sq(l[0]);
            if d0 < best.2 {
                best = (i, false, d0);
            }
            let d1 = from.distance_sq(*l.last().unwrap());
            if d1 < best.2 {
                best = (i, true, d1);
            }
        }
        let mut line = polylines.swap_remove(best.0);
        if best.1 {
            line.reverse();
        }
        from = *line.last().unwrap();
        out.push(line);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solid_spacing_adjustment() {
        // 10 mm wide, 0.45 mm lines: 22 intervals of 0.4545 mm.
        assert_eq!(adjust_solid_spacing(10_000_000, 450_000), 454_545);
        // Never more than 20% wider.
        assert_eq!(adjust_solid_spacing(800_000, 450_000), 540_000);
    }

    #[test]
    fn grid_alignment_rounds_down() {
        assert_eq!(align_to_grid(7, 5), 5);
        assert_eq!(align_to_grid(-7, 5), -10);
        assert_eq!(align_to_grid(-10, 5), -10);
    }

    #[test]
    fn sticks_are_removed() {
        let mut p = vec![
            Point::new(0, 0),
            Point::new(10, 0),
            Point::new(20, 0),
            Point::new(10, 0),
            Point::new(10, 10),
            Point::new(0, 10),
        ];
        assert!(remove_sticks(&mut p));
        assert!(!p.contains(&Point::new(20, 0)));
    }
}
