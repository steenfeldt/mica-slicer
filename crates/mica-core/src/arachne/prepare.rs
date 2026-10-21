// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: prusaslicer@b028299c77 src/libslic3r/Arachne/WallToolPaths.cpp :: fixSelfIntersections, removeDegenerateVerts, removeColinearEdges, removeSmallAreas
// Changes: Bounding-box scan replaces SparseLineGrid (a uniform grid for large inputs, same tests in scan order); Clipper2 even-odd union repairs intersections.
// Copyright (c) 2022 Ultimaker B.V.
// CuraEngine is released under the terms of the AGPLv3 or higher.
use super::graph::project;
use crate::geometry::clip::{to_polygons, union_ex_even_odd, union_ex_positive};
use crate::geometry::{ExPolygon, Point, Polygon, signed_area};

fn fix_intersections(polys: &mut Vec<Polygon>, epsilon: i64) {
    let total: usize = polys.iter().map(Vec::len).sum();
    if total <= GRID_FROM {
        nudge_scan(polys, epsilon);
    } else {
        nudge_grid(polys, epsilon);
    }
    *polys = to_polygons(&union_ex_even_odd(polys));
}

/// Vertices from which the nudge pass uses a grid instead of the all-pairs scan.
const GRID_FROM: usize = 2048;

/// One nudge test of vertex `j` of polygon `i` against segment `l` of polygon `k`: moves the
/// vertex off a segment it lies within `half` of. Returns whether it moved.
#[inline]
fn nudge(polys: &mut [Polygon], i: usize, j: usize, k: usize, l: usize, epsilon: i64) -> bool {
    let half = (epsilon + 1) / 2;
    let move_dist = (half - 2).max(2);
    let n = polys[i].len();
    let next = (l + 1) % polys[k].len();
    if i == k && (j == l || j == next) {
        return false;
    }
    let p = polys[i][j];
    let a = polys[k][l];
    let b = polys[k][next];
    if p.x < a.x.min(b.x) - epsilon
        || p.x > a.x.max(b.x) + epsilon
        || p.y < a.y.min(b.y) - epsilon
        || p.y > a.y.max(b.y) + epsilon
    {
        return false;
    }
    let q = project(p, a, b);
    if p.distance_sq(q) <= i128::from(half) * i128::from(half) {
        let other = polys[i][(j + 1) % n];
        let left = (b.x - a.x) as i128 * (other.y - a.y) as i128
            - (b.y - a.y) as i128 * (other.x - a.x) as i128
            > 0;
        let (dx, dy) = if left {
            (b.x - a.x, b.y - a.y)
        } else {
            (a.x - b.x, a.y - b.y)
        };
        let length = (dx as f64).hypot(dy as f64).trunc();
        if length > 0. {
            let before = polys[i][j];
            polys[i][j].x += (-dy as f64 * move_dist as f64 / length) as i64;
            polys[i][j].y += (dx as f64 * move_dist as f64 / length) as i64;
            return polys[i][j] != before;
        }
    }
    false
}

/// Every vertex against every segment, in order (the reference the grid must match).
fn nudge_scan(polys: &mut [Polygon], epsilon: i64) {
    for i in 0..polys.len() {
        for j in 0..polys[i].len() {
            for k in 0..polys.len() {
                for l in 0..polys[k].len() {
                    nudge(polys, i, j, k, l, epsilon);
                }
            }
        }
    }
}

/// [`nudge_scan`] with the segments in a uniform grid: the same tests in the same order on
/// the segments whose box can hold the vertex (all others are skipped by the box test
/// anyway), so the result is identical. A moved vertex re-files its two segments and asks
/// the grid again for the segments after the current one. Stability campaign 2026-10-21: a
/// textured slab at 0.05 mm layers had layers of 100,000+ vertices, and the all-pairs scan
/// held one layer (and a cancel) for many minutes.
fn nudge_grid(polys: &mut [Polygon], epsilon: i64) {
    use std::collections::HashMap;
    let offsets: Vec<usize> = polys
        .iter()
        .scan(0, |acc, p| {
            let at = *acc;
            *acc += p.len();
            Some(at)
        })
        .collect();
    let total: usize = polys.iter().map(Vec::len).sum();
    let (mut lo, mut hi) = (
        Point::new(i64::MAX, i64::MAX),
        Point::new(i64::MIN, i64::MIN),
    );
    for p in polys.iter().flatten() {
        lo = Point::new(lo.x.min(p.x), lo.y.min(p.y));
        hi = Point::new(hi.x.max(p.x), hi.y.max(p.y));
    }
    let extent = (hi.x.saturating_sub(lo.x))
        .max(hi.y.saturating_sub(lo.y))
        .max(1) as f64;
    let cell = ((extent / (total as f64).sqrt()) as i64)
        .max(2 * epsilon + 1)
        .max(1);
    // Segments covering more cells than this are checked for every vertex.
    const WIDE: i64 = 64;
    let mut grid: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    let mut wide: Vec<usize> = Vec::new();
    let file = |polys: &[Polygon],
                grid: &mut HashMap<(i64, i64), Vec<usize>>,
                wide: &mut Vec<usize>,
                k: usize,
                l: usize| {
        let a = polys[k][l];
        let b = polys[k][(l + 1) % polys[k].len()];
        let (x0, x1) = (a.x.min(b.x) - epsilon, a.x.max(b.x) + epsilon);
        let (y0, y1) = (a.y.min(b.y) - epsilon, a.y.max(b.y) + epsilon);
        let (cx0, cx1) = (x0.div_euclid(cell), x1.div_euclid(cell));
        let (cy0, cy1) = (y0.div_euclid(cell), y1.div_euclid(cell));
        let id = offsets[k] + l;
        if (cx1 - cx0 + 1).saturating_mul(cy1 - cy0 + 1) > WIDE {
            wide.push(id);
            return;
        }
        for cx in cx0..=cx1 {
            for cy in cy0..=cy1 {
                grid.entry((cx, cy)).or_default().push(id);
            }
        }
    };
    for k in 0..polys.len() {
        for l in 0..polys[k].len() {
            file(polys, &mut grid, &mut wide, k, l);
        }
    }
    let locate = |id: usize| {
        let k = offsets.partition_point(|&o| o <= id) - 1;
        (k, id - offsets[k])
    };
    let mut candidates = Vec::new();
    for i in 0..polys.len() {
        let n = polys[i].len();
        for j in 0..n {
            let mut from = 0;
            loop {
                let p = polys[i][j];
                candidates.clear();
                if let Some(c) = grid.get(&(p.x.div_euclid(cell), p.y.div_euclid(cell))) {
                    candidates.extend(c.iter().copied().filter(|&id| id >= from));
                }
                candidates.extend(wide.iter().copied().filter(|&id| id >= from));
                candidates.sort_unstable();
                candidates.dedup();
                let mut moved = false;
                for &id in &candidates {
                    from = id + 1;
                    let (k, l) = locate(id);
                    if nudge(polys, i, j, k, l, epsilon) {
                        file(polys, &mut grid, &mut wide, i, j);
                        file(polys, &mut grid, &mut wide, i, (j + n - 1) % n);
                        moved = true;
                        break;
                    }
                }
                if !moved {
                    break;
                }
            }
        }
    }
}
fn degenerate(a: Point, b: Point, c: Point) -> bool {
    let u = (i128::from(b.x - a.x), i128::from(b.y - a.y));
    let v = (i128::from(c.x - b.x), i128::from(c.y - b.y));
    u.0 * v.1 - u.1 * v.0 == 0 && u.0 * v.0 + u.1 * v.1 <= 0
}
fn remove_degenerate(poly: &mut Polygon) {
    let mut out: Polygon = vec![];
    for i in 0..poly.len() {
        let last = out.last().copied().unwrap_or(*poly.last().unwrap());
        if i + 1 == poly.len() && out.is_empty() {
            break;
        }
        let next = if i + 1 == poly.len() {
            out[0]
        } else {
            poly[i + 1]
        };
        if degenerate(last, poly[i], next) {
            while out.len() > 1 && degenerate(out[out.len() - 2], out[out.len() - 1], next) {
                out.pop();
            }
        } else {
            out.push(poly[i]);
        }
    }
    *poly = out;
}
fn remove_collinear(poly: &mut Polygon) {
    loop {
        let before = poly.len();
        let mut process = vec![true; poly.len()];
        let mut again = true;
        while again {
            again = false;
            let n = poly.len();
            if n <= 3 {
                return;
            }
            let mut skip = vec![false; n];
            let mut out = vec![];
            let mut i = 0;
            while i < n {
                if !process[i] {
                    out.push(poly[i]);
                    i += 1;
                    continue;
                }
                if i + 1 == n && skip[0] {
                    skip[out.len()] = true;
                    again = true;
                    out.push(poly[i]);
                    break;
                }
                let a = poly[(i + n - 1) % n];
                let b = poly[i];
                let c = poly[(i + 1) % n];
                let u = ((a.x - b.x) as f64, (a.y - b.y) as f64);
                let v = ((c.x - b.x) as f64, (c.y - b.y) as f64);
                let angle = (u.0 * v.1 - u.1 * v.0).atan2(u.0 * v.0 + u.1 * v.1).abs();
                if angle > 0.005 && angle < std::f64::consts::PI - 0.005 {
                    out.push(b);
                } else if i + 1 < n {
                    skip[out.len()] = true;
                    again = true;
                    out.push(c);
                    i += 1;
                }
                i += 1;
            }
            *poly = out;
            process = skip;
        }
        if poly.len() == before {
            break;
        }
    }
}
pub(crate) fn clean(mut polygons: Vec<Polygon>, minimum_area: f64) -> Vec<ExPolygon> {
    let _timing = crate::timing::Span::new(crate::timing::Stage::WallCleanup);
    fix_intersections(&mut polygons, 12_499);
    for p in &mut polygons {
        remove_degenerate(p);
        remove_collinear(p);
    }
    polygons.retain(|p| p.len() >= 3);
    fix_intersections(&mut polygons, 12_499);
    for p in &mut polygons {
        remove_degenerate(p);
    }
    polygons.retain(|p| p.len() >= 3);
    // ExPolygon nesting removes the small contour together with its holes.
    union_ex_positive(&polygons)
        .into_iter()
        .filter(|e| signed_area(&e.contour).abs() >= minimum_area)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Many small, nearly touching rings (the layers of a textured top at fine layers): the
    /// grid nudges exactly like the all-pairs scan, and fast.
    #[test]
    fn the_grid_nudges_like_the_scan() {
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let mut moved = 0;
        for case in 0..6 {
            let mut polys: Vec<Polygon> = vec![];
            let rings = 40 + case * 60;
            for r in 0..rings {
                let cx = (r % 20) as i64 * 300_000 + (next() % 40_000) as i64;
                let cy = (r / 20) as i64 * 300_000 + (next() % 40_000) as i64;
                let n = 3 + (next() % 12) as usize;
                let radius = 120_000 + (next() % 60_000) as i64;
                polys.push(
                    (0..n)
                        .map(|t| {
                            let a = t as f64 / n as f64 * std::f64::consts::TAU;
                            let jitter = (next() % 20_000) as i64;
                            Point::new(
                                cx + ((radius + jitter) as f64 * a.cos()) as i64,
                                cy + ((radius + jitter) as f64 * a.sin()) as i64,
                            )
                        })
                        .collect(),
                );
            }
            // Long edges too (they go to the wide list).
            polys.push(vec![
                Point::new(0, -5_000),
                Point::new(6_000_000, -5_000),
                Point::new(6_000_000, 4_000_000),
            ]);
            let mut a = polys.clone();
            let mut b = polys.clone();
            nudge_scan(&mut a, 12_499);
            nudge_grid(&mut b, 12_499);
            assert_eq!(a, b, "case {case}");
            moved += a
                .iter()
                .flatten()
                .zip(polys.iter().flatten())
                .filter(|(x, y)| x != y)
                .count();
        }
        assert!(moved > 50, "the cases nudge vertices ({moved})");
    }

    #[test]
    fn a_large_layer_cleans_quickly() {
        let mut polys: Vec<Polygon> = vec![];
        for r in 0..20_000i64 {
            let (cx, cy) = ((r % 200) * 100_000, (r / 200) * 100_000);
            polys.push(vec![
                Point::new(cx, cy),
                Point::new(cx + 60_000, cy),
                Point::new(cx + 60_000, cy + 60_000),
                Point::new(cx, cy + 60_000),
            ]);
        }
        let started = std::time::Instant::now();
        let out = clean(polys, 0.);
        assert_eq!(out.len(), 20_000);
        assert!(started.elapsed().as_secs() < 20, "{:?}", started.elapsed());
    }
}
