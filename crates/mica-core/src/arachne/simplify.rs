// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: prusaslicer@b028299c77 src/libslic3r/Arachne/utils/ExtrusionLine.cpp :: ExtrusionLine::simplify, calculateExtrusionAreaDeviationError
// Source: prusaslicer@b028299c77 src/libslic3r/Arachne/WallToolPaths.cpp :: simplify
// Source: prusaslicer@b028299c77 src/libslic3r/MultiPoint.hpp :: douglas_peucker
// Source: prusaslicer@b028299c77 src/libslic3r/ExPolygon.cpp :: ExPolygon::simplify_p
// Changes: Shared geometry helpers; i128 area accumulation avoids coordinate-product overflow.
// Copyright (c) 2020-2022 Ultimaker B.V.
// CuraEngine is released under the terms of the AGPLv3 or higher.
use super::{ExtrusionLine, Junction};
use crate::geometry::{Point, Polygon};
const RESOLUTION_SQ: f64 = 500_000. * 500_000.;
const DEVIATION_SQ: f64 = 25_000. * 25_000.;
fn cross(a: Point, b: Point) -> i128 {
    a.x as i128 * b.y as i128 - a.y as i128 * b.x as i128
}
fn distance_sq(p: Point, a: Point, b: Point) -> f64 {
    let dx = (b.x - a.x) as f64;
    let dy = (b.y - a.y) as f64;
    let d = dx * dx + dy * dy;
    if d == 0. {
        return p.distance_sq(a) as f64;
    }
    let cross = (p.x - a.x) as f64 * dy - (p.y - a.y) as f64 * dx;
    cross * cross / d
}
fn intersection(a: Point, b: Point, c: Point, d: Point) -> Option<Point> {
    let u = ((b.x - a.x) as f64, (b.y - a.y) as f64);
    let v = ((d.x - c.x) as f64, (d.y - c.y) as f64);
    let denominator = u.0 * v.1 - u.1 * v.0;
    if denominator.abs() < 1e-10 {
        return None;
    }
    let t = ((c.x - a.x) as f64 * v.1 - (c.y - a.y) as f64 * v.0) / denominator;
    Some(super::graph::lerp(a, b, t))
}
fn area_error(a: Junction, b: Junction, c: Junction) -> f64 {
    let ab = a.p.distance_to(b.p).trunc();
    let bc = b.p.distance_to(c.p).trunc();
    let diff = (b.width - a.width).abs().max((c.width - b.width).abs());
    if diff > 1. {
        let aw = ((a.width + b.width) / 2.).trunc();
        let bw = ((b.width + c.width) / 2.).trunc();
        let average = ((ab * aw + bc * bw) / (ab + bc)).trunc();
        (aw * ab + bw * bc - average * a.p.distance_to(c.p).trunc()).abs()
    } else {
        diff * ab.min(bc)
    }
}
pub(crate) fn line(line: &mut ExtrusionLine) {
    let n = line.junctions.len();
    if n <= if line.is_closed { 3 } else { 2 } {
        return;
    }
    let mut out = vec![line.junctions[0]];
    let mut previous = line.junctions[0];
    let mut previous_previous = if line.is_closed {
        line.junctions[n - 2]
    } else {
        previous
    };
    let mut accumulated = cross(previous.p, line.junctions[1].p);
    for i in 1..n - usize::from(!line.is_closed) {
        let current = if i + 1 == n {
            out[0]
        } else {
            line.junctions[i]
        };
        if line.is_closed && out.len() + n - i <= 3 {
            out.push(current);
            continue;
        }
        let spill = line.is_closed && i + 2 >= n && i + 2 - n < out.len();
        let next = if spill {
            out[i + 2 - n]
        } else {
            line.junctions[i + 1]
        };
        let removed = cross(current.p, next.p);
        accumulated += removed;
        let length = current.p.distance_sq(previous.p) as f64;
        if length < 25_000. {
            continue;
        }
        let base = next.p.distance_sq(previous.p) as f64;
        if base == 0. {
            continue;
        }
        let area = (accumulated + cross(next.p, previous.p)) as f64;
        let height_sq = area * area / base;
        if height_sq <= 1_000.
            && distance_sq(current.p, previous.p, next.p) <= 1_000. * 1_000.
            && area_error(previous, current, next) <= 2_000_000.
        {
            continue;
        }
        if length < RESOLUTION_SQ && height_sq <= DEVIATION_SQ {
            if current.p.distance_sq(next.p) as f64 > 4. * RESOLUTION_SQ {
                if let Some(p) = intersection(previous_previous.p, previous.p, current.p, next.p)
                    && distance_sq(p, previous.p, current.p) <= DEVIATION_SQ
                    && p.distance_sq(previous.p) as f64 <= RESOLUTION_SQ
                    && p.distance_sq(current.p) as f64 <= RESOLUTION_SQ
                {
                    if !out.is_empty() {
                        out.pop();
                        previous = previous_previous;
                    }
                    accumulated = removed;
                    previous_previous = previous;
                    previous = Junction { p, ..current };
                    out.push(previous);
                    continue;
                }
            } else {
                continue;
            }
        }
        accumulated = removed;
        previous_previous = previous;
        previous = current;
        out.push(current);
    }
    if line.is_closed {
        out[0].p = out.last().unwrap().p;
    } else {
        out.push(line.junctions[n - 1]);
    }
    line.junctions = out;
}

pub(crate) fn outline(poly: &mut Polygon) {
    let n = poly.len();
    if n < 3 {
        poly.clear();
        return;
    }
    if n == 3 {
        return;
    }
    let mut out: Polygon = vec![];
    let mut prev = poly[n - 1];
    let mut prevprev = poly[n - 2];
    let mut accumulated = cross(prev, poly[0]);
    for i in 0..n {
        let mut current = poly[i];
        let next = if i + 1 < n {
            poly[i + 1]
        } else if out.len() > 1 {
            out[0]
        } else {
            poly[0]
        };
        let removed = cross(current, next);
        accumulated += removed;
        let length = current.distance_sq(prev) as f64;
        if length < 25_000_000. {
            continue;
        }
        let base = next.distance_sq(prev) as f64;
        if base == 0. {
            continue;
        }
        let area = (accumulated + cross(next, prev)) as f64;
        let height_sq = area * area / base;
        if height_sq <= 5_000. * 5_000. && distance_sq(current, prev, next) <= 5_000. * 5_000. {
            continue;
        }
        if length < RESOLUTION_SQ && height_sq <= DEVIATION_SQ {
            if current.distance_sq(next) as f64 > 4. * RESOLUTION_SQ {
                if let Some(p) = intersection(prevprev, prev, current, next)
                    && distance_sq(p, prev, current) <= DEVIATION_SQ
                    && p.distance_sq(prev) as f64 <= RESOLUTION_SQ
                    && p.distance_sq(next) as f64 <= RESOLUTION_SQ
                {
                    current = p;
                    if !out.is_empty() {
                        out.pop();
                        prev = prevprev;
                    }
                }
            } else {
                continue;
            }
        }
        accumulated = removed;
        prevprev = prev;
        prev = current;
        out.push(current);
    }
    *poly = out;
}

/// Closed-polygon Douglas–Peucker used by process_arachne before making the spacing outline.
pub(crate) fn resolution(poly: &Polygon, tolerance: f64) -> Polygon {
    if poly.len() < 3 || tolerance <= 0. {
        return poly.clone();
    }
    let mut points = poly.clone();
    points.push(points[0]);
    let mut keep = vec![false; points.len()];
    keep[0] = true;
    keep[points.len() - 1] = true;
    let mut stack = vec![(0, points.len() - 1)];
    while let Some((a, b)) = stack.pop() {
        if b <= a + 1 {
            continue;
        }
        let mut furthest = a;
        let mut distance = -1.;
        for i in a + 1..b {
            let d = points[i].distance_sq(super::graph::project(points[i], points[a], points[b]))
                as f64;
            if d > distance {
                distance = d;
                furthest = i;
            }
        }
        if distance > tolerance * tolerance {
            keep[furthest] = true;
            stack.push((furthest, b));
            stack.push((a, furthest));
        }
    }
    points.pop();
    points
        .into_iter()
        .zip(keep)
        .filter_map(|(p, keep)| keep.then_some(p))
        .collect()
}
