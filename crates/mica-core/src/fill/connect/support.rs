// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: prusaslicer@b028299c77 src/libslic3r/Fill/FillBase.cpp :: Fill::connect_base_support, mark_boundary_segments_overlapping_infill, base_support_extend_infill_lines, emit_loops_in_band, evaluate_support_arches, evaluate_support_arch_cost
// Changes: Rust indices replace contour-intersection pointers; reuse the existing boundary graph.
// Changes: Costs are local to each fill (upstream's static thresholds accidentally use the first spacing).
// Copyright (c) Prusa Research 2016 - 2023 Vojtěch Bubník @bubnikv, Lukáš Hejl @hejllukas, Tomáš Mészáros @tamasmeszaros, Lukáš Matěna @lukasmatena
// Copyright (c) SuperSlicer 2018 - 2019 Remi Durand @supermerill
// ported from lib/Slic3r/Fill/Base.pm:
// Copyright (c) Prusa Research 2016 Vojtěch Bubník @bubnikv
// Copyright (c) Slic3r 2011 - 2014 Alessandro Ranellucci @alranel
// PrusaSlicer is released under the terms of the AGPLv3 or higher

//! Sparse support boundary arches, scanline extensions, zig-zags, caps and T-joints.

use super::*;

fn point(g: &BoundaryInfillGraph, i: usize) -> Point {
    let c = g.cps[i];
    g.boundary[c.contour_idx][c.point_idx]
}
fn xy(p: Point) -> V2 {
    (p.x as f64, p.y as f64)
}
fn vertical(g: &BoundaryInfillGraph, i: usize, prev: bool) -> bool {
    let c = g.cps[i];
    point(g, i).x
        == point(
            g,
            if prev {
                c.prev_on_contour
            } else {
                c.next_on_contour
            },
        )
        .x
}
fn could_take(g: &BoundaryInfillGraph, i: usize, prev: bool) -> bool {
    if prev {
        g.cps[i].could_take_prev()
    } else {
        g.cps[i].could_take_next()
    }
}
fn trim(g: &mut BoundaryInfillGraph, i: usize, prev: bool, len: f64) {
    if prev {
        g.cps[i].trim_prev(len);
    } else {
        g.cps[i].trim_next(len);
    }
}
fn available(c: ContourIntersectionPoint, prev: bool) -> f64 {
    if prev {
        c.contour_not_taken_length_prev
    } else {
        c.contour_not_taken_length_next
    }
}
fn neighbour(c: ContourIntersectionPoint, prev: bool) -> usize {
    if prev {
        c.prev_on_contour
    } else {
        c.next_on_contour
    }
}
fn trimmed(c: ContourIntersectionPoint, prev: bool) -> bool {
    if prev { c.prev_trimmed } else { c.next_trimmed }
}
fn distance(g: &BoundaryInfillGraph, i: usize, j: usize, prev: bool) -> f64 {
    let c = g.cps[i];
    let len = g.contour_length(c.contour_idx);
    if prev {
        closed_contour_distance_cw(c.param, g.cps[j].param, len)
    } else {
        closed_contour_distance_ccw(c.param, g.cps[j].param, len)
    }
}
fn step(i: usize, n: usize, prev: bool) -> usize {
    if prev { (i + n - 1) % n } else { (i + 1) % n }
}
fn take_full(pl: &mut Polyline, contour: &[Point], from: usize, to: usize, prev: bool) {
    if prev {
        take_cw_full(pl, contour, from, to);
    } else {
        take_ccw_full(pl, contour, from, to);
    }
}
fn take_part(pl: &mut Polyline, g: &BoundaryInfillGraph, i: usize, prev: bool, length: f64) {
    let c = g.cps[i];
    let j = neighbour(c, prev);
    let contour = &g.boundary[c.contour_idx];
    let params = &g.boundary_params[c.contour_idx];
    if prev {
        take_cw_limited(pl, contour, params, c.point_idx, g.cps[j].point_idx, length);
    } else {
        take_ccw_limited(pl, contour, params, c.point_idx, g.cps[j].point_idx, length);
    }
}
fn length(pl: &[Point]) -> f64 {
    pl.windows(2).map(|p| p[0].distance_to(p[1])).sum()
}
fn clip_end(pl: &mut Polyline, mut amount: f64) {
    while pl.len() > 1 && amount > 0. {
        let end = pl.pop().unwrap();
        let start = *pl.last().unwrap();
        let len = start.distance_to(end);
        if len > amount {
            pl.push(lerp(start, end, (len - amount) / len));
            break;
        }
        amount -= len;
    }
}

fn mark_overlapping(g: &mut BoundaryInfillGraph, infill: &[Polyline], spacing: f64) {
    let radius = 0.5 * (spacing + SCALED_EPSILON as f64);
    for i in 0..g.cps.len() {
        let c = g.cps[i];
        if c.contour_idx == NONE {
            continue;
        }
        let line = &infill[i / 2];
        for prev in [false, true] {
            if !could_take(g, i, prev) {
                trim(g, i, prev, 0.);
                continue;
            }
            let c = g.cps[i];
            let end = g.cps[neighbour(c, prev)].point_idx;
            let contour = &g.boundary[c.contour_idx];
            let params = &g.boundary_params[c.contour_idx];
            let mut k = c.point_idx;
            let mut inside = true;
            while k != end {
                let j = step(k, contour.len(), prev);
                if distance_to_segment_squared(xy(line[0]), xy(line[1]), xy(contour[j]))
                    >= radius * radius
                {
                    let overlap = line_rounded_thick_segment_collision(
                        xy(contour[k]),
                        xy(contour[j]),
                        xy(line[0]),
                        xy(line[1]),
                        radius,
                    );
                    let len = if prev {
                        closed_contour_distance_cw(
                            params[c.point_idx],
                            params[k],
                            *params.last().unwrap(),
                        )
                    } else {
                        closed_contour_distance_ccw(
                            params[c.point_idx],
                            params[k],
                            *params.last().unwrap(),
                        )
                    } + overlap.map_or(0., |v| v.1);
                    if len < available(c, prev) {
                        inside = false;
                        break;
                    }
                }
                let len = if prev {
                    closed_contour_distance_cw(
                        params[c.point_idx],
                        params[j],
                        *params.last().unwrap(),
                    )
                } else {
                    closed_contour_distance_ccw(
                        params[c.point_idx],
                        params[j],
                        *params.last().unwrap(),
                    )
                };
                if len >= available(c, prev) {
                    break;
                }
                k = j;
            }
            if inside {
                if !trimmed(c, prev) {
                    trim(g, neighbour(c, prev), !prev, 0.);
                }
                trim(g, i, prev, 0.);
            }
        }
    }
}

fn extend_lines(infill: &mut [Polyline], g: &mut BoundaryInfillGraph, line_spacing: f64) {
    let max_x = (line_spacing * 0.33) as i64;
    let min_y = (line_spacing * 0.5) as i64;
    for i in 0..g.cps.len() {
        let c = g.cps[i];
        if c.contour_idx == NONE {
            continue;
        }
        let contour = &g.boundary[c.contour_idx];
        let params = &g.boundary_params[c.contour_idx];
        let p = point(g, i);
        let mut candidates = [None, None];
        for (side, prev) in [false, true].into_iter().enumerate() {
            if vertical(g, i, prev) {
                continue;
            }
            let mut k = c.point_idx;
            let mut j = step(k, contour.len(), prev);
            while j != g.cps[neighbour(c, prev)].point_idx {
                if (contour[j].x - p.x).abs() > max_x {
                    break;
                }
                k = j;
                j = step(j, contour.len(), prev);
            }
            if k == c.point_idx {
                continue;
            }
            let dy = (contour[k].y - p.y) * if i & 1 == 0 { -1 } else { 1 };
            if dy <= min_y {
                continue;
            }
            let arc = if prev {
                closed_contour_distance_cw(params[c.point_idx], params[k], *params.last().unwrap())
            } else {
                closed_contour_distance_ccw(params[c.point_idx], params[k], *params.last().unwrap())
            };
            if arc < available(c, prev) {
                candidates[side] = Some((k, dy, arc));
            }
        }
        // In the pinned implementation the conditional assignment leaves the previous
        // candidate valid in both branches, and it is taken first when both qualify.
        let choice = candidates[1]
            .map(|v| (true, v))
            .or_else(|| candidates[0].map(|v| (false, v)));
        if let Some((prev, (k, _, arc))) = choice {
            let line = &mut infill[i / 2];
            if i & 1 == 0 {
                line.reverse();
            }
            take_full(line, contour, c.point_idx, k, prev);
            if i & 1 == 0 {
                line.reverse();
            }
            g.cps[i].point_idx = k;
            if trimmed(c, prev) {
                if prev {
                    g.cps[i].contour_not_taken_length_prev -= arc;
                } else {
                    g.cps[i].contour_not_taken_length_next -= arc;
                }
            } else {
                let other = neighbour(c, prev);
                let len = if prev {
                    closed_contour_distance_ccw(
                        params[g.cps[other].point_idx],
                        params[k],
                        *params.last().unwrap(),
                    )
                } else {
                    closed_contour_distance_ccw(
                        params[k],
                        params[g.cps[other].point_idx],
                        *params.last().unwrap(),
                    )
                };
                if prev {
                    g.cps[i].contour_not_taken_length_prev = len;
                    g.cps[other].contour_not_taken_length_next = len;
                } else {
                    g.cps[i].contour_not_taken_length_next = len;
                    g.cps[other].contour_not_taken_length_prev = len;
                }
            }
            trim(g, i, !prev, 0.);
            if prev {
                g.cps[c.next_on_contour].prev_trimmed = true;
            } else {
                g.cps[c.prev_on_contour].next_trimmed = true;
            }
        }
    }
}

/// The contour may leave/reenter the vertical band; retain outward-facing connecting segments.
#[allow(clippy::too_many_arguments)]
fn emit_in_band(
    left: i64,
    right: i64,
    contour: &[Point],
    params: &[f64],
    begin: f64,
    end: f64,
    min_length: f64,
    out: &mut Vec<Polyline>,
) {
    let mut ibegin = params.partition_point(|&v| v < begin);
    if params[ibegin] != begin {
        ibegin -= 1;
    }
    let iend = params.partition_point(|&v| v < end) % contour.len();
    ibegin %= contour.len();
    let pbegin = lerp(
        contour[ibegin],
        contour[(ibegin + 1) % contour.len()],
        (begin - params[ibegin]) / (params[ibegin + 1] - params[ibegin]),
    );
    let prev = step(iend, contour.len(), true);
    let tprev = if iend == 0 {
        *params.last().unwrap()
    } else {
        params[prev]
    };
    let pend = lerp(
        contour[iend],
        contour[prev],
        (end - params[iend]) / (tprev - params[iend]),
    );
    let side = |p: Point| {
        if p.x < left {
            -1
        } else if p.x > right {
            1
        } else {
            0
        }
    };
    let mut pl = Vec::new();
    let mut pl_end = 0;
    let finalize = |pl: &mut Polyline, pl_end: usize, out: &mut Vec<Polyline>| {
        pl.truncate(pl_end);
        if !pl.is_empty() {
            if let Some(last) = out.last_mut().filter(|p| {
                let a = *p.last().unwrap();
                let b = pl[0];
                let dx = (a.x - b.x) as i128;
                let dy = (a.y - b.y) as i128;
                dx * dx + dy * dy < SCALED_EPSILON as i128
            }) {
                last.extend_from_slice(&pl[1..]);
            } else if length(pl) > min_length {
                out.push(std::mem::take(pl));
            }
        }
        pl.clear();
    };
    let mut p1 = pbegin;
    if side(p1) == 0 {
        pl.push(p1);
    }
    let mut k = ibegin;
    while k != iend {
        let next = (k + 1) % contour.len();
        let p2 = if next == iend { pend } else { contour[next] };
        let (s1, s2) = (side(p1), side(p2));
        let mut interpolate = |s: i32, entering: bool| {
            let x = if s < 0 { left } else { right };
            let y = p1.y + ((x - p1.x) as f64 * (p2.y - p1.y) as f64 / (p2.x - p1.x) as f64) as i64;
            if !entering {
                pl_end = pl.len();
                pl.push(Point::new(x, y));
            } else if pl_end > 0 {
                if (s1 < 0) == (y - pl[pl_end].y < 0) {
                    pl.remove(pl_end);
                } else {
                    finalize(&mut pl, pl_end, out);
                    pl.push(Point::new(x, y));
                }
                pl_end = 0;
            } else {
                pl.push(Point::new(x, y));
            }
        };
        if s1 == 0 {
            if s2 == 0 {
                pl.push(p2);
            } else {
                interpolate(s2, false);
                if pl_end > 0 {
                    pl.push(p2);
                }
            }
        } else if s2 == 0 {
            interpolate(s1, true);
            pl.push(p2);
        } else if s1 != s2 {
            interpolate(s1, true);
            interpolate(s2, false);
        } else if pl_end > 0 {
            pl.push(p2);
        }
        p1 = p2;
        k = next;
    }
    finalize(&mut pl, pl_end, out);
}

#[derive(Clone, Copy, Default)]
struct Arch {
    self_loop: bool,
    cost: f64,
}
fn arch_cost(pl: &[Point]) -> f64 {
    let a = pl[0];
    let b = *pl.last().unwrap();
    let (min_y, max_y) = (a.y.min(b.y), a.y.max(b.y));
    pl.iter()
        .map(|&p| {
            distance_to_segment_squared(xy(a), xy(b), xy(p))
                .sqrt()
                .max((p.y - max_y).max(min_y - p.y) as f64)
        })
        .fold(0., f64::max)
}
fn evaluate_arches(g: &BoundaryInfillGraph) -> Vec<Arch> {
    let mut out = vec![Arch::default(); g.cps.len() * 2];
    for i in 0..g.cps.len() {
        let c = g.cps[i];
        if c.contour_idx == NONE {
            continue;
        }
        for (side, prev) in [true, false].into_iter().enumerate() {
            let other = neighbour(c, prev);
            let arch = &mut out[i * 2 + side];
            arch.self_loop = other == (i ^ 1);
            if available(c, prev) > SCALED_EPSILON as f64 {
                let mut pl = vec![point(g, i)];
                if trimmed(c, prev) {
                    take_part(&mut pl, g, i, prev, available(c, prev));
                } else {
                    take_full(
                        &mut pl,
                        &g.boundary[c.contour_idx],
                        c.point_idx,
                        g.cps[other].point_idx,
                        prev,
                    );
                }
                arch.cost = arch_cost(&pl);
            }
        }
    }
    out
}
fn merged(links: &mut [usize], i: usize) -> usize {
    let mut k = i;
    while links[k] != k {
        k = links[k];
    }
    links[i] = k;
    k
}
fn take_next(
    g: &mut BoundaryInfillGraph,
    infill: &mut [Polyline],
    links: &mut [usize],
    i: usize,
    first: bool,
    half_width: f64,
    trim_length: f64,
) {
    let j = g.cps[i].next_on_contour;
    if g.cps[if first { i } else { j }].consumed {
        return;
    }
    let a = merged(links, i / 2);
    let b = merged(links, j / 2);
    let mut is_trimmed = trimmed(g.cps[if first { i } else { j }], !first);
    if !is_trimmed {
        is_trimmed = i == j || a == b || g.cps[if first { j } else { i }].consumed || j == (i ^ 1);
        if is_trimmed {
            let len = if i == j {
                g.contour_length(g.cps[i].contour_idx)
            } else {
                distance(g, i, j, false)
            };
            trim(
                g,
                i,
                false,
                if first {
                    (len - trim_length - SCALED_EPSILON as f64).max(0.)
                } else {
                    0.
                },
            );
            trim(
                g,
                j,
                true,
                if first {
                    0.
                } else {
                    (len - trim_length - SCALED_EPSILON as f64).max(0.)
                },
            );
        }
    }
    if is_trimmed {
        let (idx, start, end, clockwise) = if first {
            (a, i, j, false)
        } else {
            (b, j, i, true)
        };
        take_limited(g, &mut infill[idx], start, end, clockwise, 1e10, half_width);
    } else if !g.cps[i].consumed && !g.cps[j].consumed {
        let mut pl1 = std::mem::take(&mut infill[a]);
        let mut pl2 = std::mem::take(&mut infill[b]);
        if point(g, i) == pl1[0] {
            pl1.reverse();
        }
        if point(g, j) == *pl2.last().unwrap() {
            pl2.reverse();
        }
        take(g, &mut pl1, &pl2, i, j, false);
        infill[a.min(b)] = pl1;
        links[a.max(b)] = links[a.min(b)];
    }
}

pub(crate) fn connect_base_support(
    mut infill: Vec<Polyline>,
    boundary: &[Polygon],
    bbox: BoundingBox,
    spacing: f64,
    density: f64,
) -> Vec<Polyline> {
    infill.retain(|p| p.len() >= 2);
    let mut g = create_boundary_infill_graph(&infill, boundary, bbox, spacing);
    let half_width = 0.5 * spacing * SCALE;
    let line_spacing = spacing * SCALE / density;
    let trim_length = half_width * 0.3;
    let mut out = Vec::new();
    mark_overlapping(&mut g, &infill, spacing * SCALE);
    let mut counts = vec![0; g.boundary.len()];
    for c in &g.cps {
        if c.contour_idx != NONE {
            counts[c.contour_idx] += 1;
        }
    }
    for (i, &count) in counts.iter().enumerate() {
        if count == 0 && g.contour_length(i) > trim_length + 0.5 * line_spacing {
            let mut pl = g.boundary[i].clone();
            pl.push(pl[0]);
            clip_end(&mut pl, trim_length);
            if pl.len() > 1 {
                out.push(pl);
            }
        }
    }
    for i in 0..g.cps.len() {
        let c = g.cps[i];
        if c.contour_idx == NONE {
            continue;
        }
        let next = g.cps[c.next_on_contour];
        if c.next_trimmed && next.prev_trimmed {
            let x = point(&g, i).x;
            let (left, right) = if i & 1 == 0 {
                (
                    (x as f64 + half_width) as i64,
                    (x as f64 + line_spacing - half_width) as i64,
                )
            } else {
                (
                    (x as f64 - line_spacing + half_width) as i64,
                    (x as f64 - half_width) as i64,
                )
            };
            let len = g.contour_length(c.contour_idx);
            let mut begin = c.param + c.contour_not_taken_length_next;
            let mut end = next.param - next.contour_not_taken_length_prev;
            if begin >= len {
                begin -= len;
            }
            if end < 0. {
                end += len;
            }
            emit_in_band(
                left,
                right,
                &g.boundary[c.contour_idx],
                &g.boundary_params[c.contour_idx],
                begin,
                end,
                0.5 * line_spacing,
                &mut out,
            );
        }
    }
    extend_lines(&mut infill, &mut g, line_spacing);
    let mut links: Vec<_> = (0..infill.len()).collect();
    for i in 0..g.cps.len() {
        let c = g.cps[i];
        if c.contour_idx == NONE || c.consumed {
            continue;
        }
        let prev = could_take(&g, i, true)
            && vertical(&g, i, true)
            && !g.cps[c.prev_on_contour].consumed
            && c.prev_on_contour != (i ^ 1);
        let next = could_take(&g, i, false)
            && vertical(&g, i, false)
            && !g.cps[c.next_on_contour].consumed
            && c.next_on_contour != (i ^ 1);
        let prefer_prev = if c.prev_trimmed == c.next_trimmed {
            c.contour_not_taken_length_prev > c.contour_not_taken_length_next
        } else {
            !c.prev_trimmed && c.next_trimmed
        };
        if prev && (!next || prefer_prev) {
            if !c.prev_trimmed || c.contour_not_taken_length_prev > 1.3 * line_spacing {
                take_next(
                    &mut g,
                    &mut infill,
                    &mut links,
                    c.prev_on_contour,
                    false,
                    half_width,
                    trim_length,
                );
            }
        } else if next && (!c.next_trimmed || c.contour_not_taken_length_next > 1.3 * line_spacing)
        {
            take_next(
                &mut g,
                &mut infill,
                &mut links,
                i,
                true,
                half_width,
                trim_length,
            );
        }
    }
    let arches = evaluate_arches(&g);
    let low = line_spacing * 1.3;
    let high = line_spacing * 2.;
    let mut selected = Vec::new();
    for (i, c) in g.cps.iter().enumerate() {
        if c.contour_idx == NONE || c.consumed {
            continue;
        }
        let (a, b) = (arches[2 * i].cost, arches[2 * i + 1].cost);
        if a.max(b) < low || a.min(b) > high || (a - b).abs() / a.max(b) < 0.25 {
            continue;
        }
        if a > low {
            selected.push(2 * i);
        }
        if b > low {
            selected.push(2 * i + 1);
        }
    }
    selected.sort_unstable_by(|&a, &b| arches[b].cost.total_cmp(&arches[a].cost));
    for a in selected {
        let i = a / 2;
        if !g.cps[i].consumed {
            let prev = a & 1 == 0;
            let k = if prev { g.cps[i].prev_on_contour } else { i };
            take_next(
                &mut g,
                &mut infill,
                &mut links,
                k,
                !prev,
                half_width,
                trim_length,
            );
        }
    }
    // Left-to-right zig-zag, unlike the general infill connector's shortest-arch ordering.
    for i in 0..g.cps.len() {
        let c = g.cps[i];
        if c.contour_idx == NONE || c.consumed {
            continue;
        }
        let first = i & 1 == 0;
        let j = neighbour(c, !first);
        if merged(&mut links, i / 2) != merged(&mut links, j / 2) {
            take_next(
                &mut g,
                &mut infill,
                &mut links,
                if first { i } else { j },
                first,
                half_width,
                trim_length,
            );
        }
    }
    for i in 0..g.cps.len() {
        if g.cps[i].contour_idx == NONE {
            continue;
        }
        for prev in [true, false] {
            let c = g.cps[i];
            if neighbour(c, prev) == (i ^ 1) && could_take(&g, i, prev) {
                take_next(
                    &mut g,
                    &mut infill,
                    &mut links,
                    if prev { c.prev_on_contour } else { i },
                    !prev,
                    half_width,
                    trim_length,
                );
            }
        }
    }
    let mut candidates = Vec::new();
    for i in 0..g.cps.len() {
        if g.cps[i].contour_idx == NONE {
            continue;
        }
        if could_take(&g, i, true) {
            candidates.push(2 * i);
        }
        if could_take(&g, i, false) {
            candidates.push(2 * i + 1);
        }
    }
    candidates.sort_unstable_by(|&a, &b| arches[b].cost.total_cmp(&arches[a].cost));
    for a in candidates {
        let (i, prev) = (a / 2, a & 1 == 0);
        let j = neighbour(g.cps[i], prev);
        if could_take(&g, i, prev)
            && (merged(&mut links, i / 2) != merged(&mut links, j / 2) || arches[a].cost > high)
        {
            take_next(
                &mut g,
                &mut infill,
                &mut links,
                if prev { j } else { i },
                !prev,
                half_width,
                trim_length,
            );
        }
    }
    for i in 0..g.cps.len() {
        if g.cps[i].contour_idx == NONE {
            continue;
        }
        for (side, prev) in [true, false].into_iter().enumerate() {
            let arch = arches[2 * i + side];
            let c = g.cps[i];
            if available(c, prev) > SCALED_EPSILON as f64
                && arch.cost
                    > if arch.self_loop {
                        0.5 * line_spacing
                    } else {
                        3. * line_spacing
                    }
            {
                let mut pl = vec![point(&g, i)];
                if !trimmed(c, prev) {
                    trim(&mut g, i, prev, available(c, prev) - half_width);
                    trim(&mut g, neighbour(c, prev), !prev, 0.);
                }
                let len = available(g.cps[i], prev);
                if len > SCALED_EPSILON as f64 {
                    take_part(&mut pl, &g, i, prev, len);
                    trim(&mut g, i, prev, 0.);
                    pl.reverse();
                    clip_end(&mut pl, half_width);
                    pl.reverse();
                    out.push(pl);
                }
            }
        }
    }
    out.extend(infill.into_iter().filter(|p| !p.is_empty()));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn support_zigzag_and_caps_stay_inside_the_boundary() {
        let boundary: Vec<_> = [(0., 0.), (12., 0.), (12., 10.), (0., 10.)]
            .map(|p| Point::from_mm(p.0, p.1))
            .to_vec();
        let lines: Vec<_> = [2., 6., 10.]
            .map(|x| vec![Point::from_mm(x, 0.), Point::from_mm(x, 10.)])
            .to_vec();
        let out = connect_base_support(
            lines,
            std::slice::from_ref(&boundary),
            BoundingBox::from_points(&boundary).unwrap(),
            0.4,
            0.1,
        );
        assert_eq!(out.len(), 1, "{out:?}");
        assert!(out[0].iter().any(|p| p.x == Point::from_mm(2., 0.).x));
        assert!(out[0].iter().any(|p| p.x == Point::from_mm(10., 0.).x));
        let area = crate::geometry::ExPolygon {
            contour: boundary,
            holes: vec![],
        };
        // Clipper's open-path difference may retain a path collinear with the clip
        // boundary. Expand by one scaled unit to include that boundary in the check.
        let inclusive =
            crate::geometry::clip::offset_ex(&[area], 1., crate::geometry::clip::Join::Miter);
        assert!(
            crate::geometry::clip::diff_pl_is_empty(&out, &inclusive),
            "{out:?}"
        );
    }
    #[test]
    fn a_contour_without_scanlines_still_gets_a_clipped_support_loop() {
        let boundary: Vec<_> = [(0., 0.), (1., 0.), (1., 10.), (0., 10.)]
            .map(|p| Point::from_mm(p.0, p.1))
            .to_vec();
        let out = connect_base_support(
            vec![],
            std::slice::from_ref(&boundary),
            BoundingBox::from_points(&boundary).unwrap(),
            0.4,
            0.1,
        );
        assert_eq!(out.len(), 1);
        assert!((length(&out[0]) / SCALE - (22. - 0.06)).abs() < 1e-6);
        assert_ne!(out[0].first(), out[0].last());
    }
    #[test]
    fn vertical_boundary_arches_are_taken_before_the_zig_zag() {
        // Three lines 2 mm apart in a 10 mm square (spacing 0.4, density 0.2): the square's
        // vertical sides are consumed first, the remaining ends are joined left to right and
        // the free arcs stop half a line width short of the next line.
        let boundary: Vec<_> = [(0., 0.), (10., 0.), (10., 10.), (0., 10.)]
            .map(|p| Point::from_mm(p.0, p.1))
            .to_vec();
        let lines: Vec<_> = [3., 5., 7.]
            .map(|x| vec![Point::from_mm(x, 0.), Point::from_mm(x, 10.)])
            .to_vec();
        let out = connect_base_support(
            lines,
            std::slice::from_ref(&boundary),
            BoundingBox::from_points(&boundary).unwrap(),
            0.4,
            0.2,
        );
        assert_eq!(out.len(), 2, "{out:?}");
        let total: f64 = out.iter().map(|p| length(p)).sum::<f64>() / SCALE;
        assert!((total - 67.2).abs() < 1e-3, "length {total}");
        for side in [0., 10.] {
            let x = Point::from_mm(side, 0.).x;
            assert!(out.iter().any(|p| p.windows(2).any(|w| w[0].x == x
                && w[1].x == x
                && (w[0].y - w[1].y).abs() == Point::from_mm(0., 10.).y)));
        }
    }
}
