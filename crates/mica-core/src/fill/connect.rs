// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: prusaslicer@b028299c77 src/libslic3r/Fill/FillBase.cpp :: Fill::connect_infill
// Source: prusaslicer@b028299c77 src/libslic3r/Fill/FillBase.cpp :: create_boundary_infill_graph
// Source: prusaslicer@b028299c77 src/libslic3r/Fill/FillBase.cpp :: mark_boundary_segments_touching_infill
// Source: prusaslicer@b028299c77 src/libslic3r/EdgeGrid.cpp :: EdgeGrid::Grid::closest_point_signed_distance
// Source: prusaslicer@b028299c77 src/libslic3r/Geometry.hpp :: liang_barsky_line_clipping_interval
// Changes: Rust port. ContourIntersectionPoint pointers become indices. The EdgeGrid spatial index is
// Changes: replaced by a bounding-box filtered scan over the boundary segments (same hits, as the grid
// Changes: only accelerates the search). The disabled (#if 0) connection strategy is not ported.
// Changes: The support-specific connector is in connect/support.rs and shares this boundary graph.

//! Connecting infill line ends along the boundary ("anchoring").
//!
//! Each infill line end is a T-joint on the boundary. Neighbouring T-joints whose boundary arc is shorter
//! than `anchor_length_max` are joined into one extrusion along the boundary, shortest arcs first. Ends
//! that stay unconnected get a hook of up to `anchor_length` along the boundary, as long as that part of
//! the boundary is not already covered by another infill line.

use super::base::{SCALED_EPSILON, lerp};
use crate::geometry::{BoundingBox, Point, Polygon, Polyline, SCALE};

mod support;
pub(crate) use support::connect_base_support;

const NONE: usize = usize::MAX;

/// One T-joint of an infill line end with the boundary.
#[derive(Clone, Copy, Debug)]
struct ContourIntersectionPoint {
    contour_idx: usize,
    point_idx: usize,
    /// Euclidean parameter of `point_idx` along its contour.
    param: f64,
    /// Neighbouring T-joints on the same contour (itself if it is the only one).
    prev_on_contour: usize,
    next_on_contour: usize,
    /// Contour length not yet taken by an extrusion or masked by an overlapping infill line.
    contour_not_taken_length_prev: f64,
    contour_not_taken_length_next: f64,
    /// The infill line end was connected, or could not be connected.
    consumed: bool,
    prev_trimmed: bool,
    next_trimmed: bool,
}

impl ContourIntersectionPoint {
    fn unconnected() -> Self {
        Self {
            contour_idx: NONE,
            point_idx: NONE,
            param: 0.0,
            prev_on_contour: NONE,
            next_on_contour: NONE,
            contour_not_taken_length_prev: f64::MAX,
            contour_not_taken_length_next: f64::MAX,
            consumed: false,
            prev_trimmed: false,
            next_trimmed: false,
        }
    }
    fn consume_prev(&mut self) {
        self.contour_not_taken_length_prev = 0.0;
        self.prev_trimmed = true;
        self.consumed = true;
    }
    fn consume_next(&mut self) {
        self.contour_not_taken_length_next = 0.0;
        self.next_trimmed = true;
        self.consumed = true;
    }
    fn trim_prev(&mut self, new_len: f64) {
        if new_len < self.contour_not_taken_length_prev {
            self.contour_not_taken_length_prev = new_len;
            self.prev_trimmed = true;
        }
    }
    fn trim_next(&mut self, new_len: f64) {
        if new_len < self.contour_not_taken_length_next {
            self.contour_not_taken_length_next = new_len;
            self.next_trimmed = true;
        }
    }
    fn could_take_prev(&self) -> bool {
        !self.consumed && self.contour_not_taken_length_prev > SCALED_EPSILON as f64
    }
    fn could_take_next(&self) -> bool {
        !self.consumed && self.contour_not_taken_length_next > SCALED_EPSILON as f64
    }
}

/// The boundary with the T-joints inserted, its parametrisation and the T-joints
/// (two per infill line: index `2 * line` is its first point, `2 * line + 1` its last).
struct BoundaryInfillGraph {
    boundary: Vec<Vec<Point>>,
    boundary_params: Vec<Vec<f64>>,
    cps: Vec<ContourIntersectionPoint>,
}

impl BoundaryInfillGraph {
    fn could_connect_prev(&self, i: usize) -> bool {
        let c = &self.cps[i];
        let p = &self.cps[c.prev_on_contour];
        !c.consumed && c.prev_on_contour != i && !p.consumed && !c.prev_trimmed && !p.next_trimmed
    }
    fn could_connect_next(&self, i: usize) -> bool {
        let c = &self.cps[i];
        let n = &self.cps[c.next_on_contour];
        !c.consumed && c.next_on_contour != i && !n.consumed && !c.next_trimmed && !n.prev_trimmed
    }
    fn contour_length(&self, contour_idx: usize) -> f64 {
        *self.boundary_params[contour_idx].last().unwrap()
    }
}

/// Distance from param1 to param2 going counter-clockwise.
fn closed_contour_distance_ccw(param1: f64, param2: f64, contour_length: f64) -> f64 {
    let d = param2 - param1;
    if d < 0.0 { d + contour_length } else { d }
}

/// Distance from param1 to param2 going clockwise.
fn closed_contour_distance_cw(param1: f64, param2: f64, contour_length: f64) -> f64 {
    closed_contour_distance_ccw(param2, param1, contour_length)
}

/// Adds contour points of the interval (idx_start, idx_end] going clockwise.
fn take_cw_full(pl: &mut Polyline, contour: &[Point], idx_start: usize, idx_end: usize) {
    let mut i = if idx_start == 0 {
        contour.len() - 1
    } else {
        idx_start - 1
    };
    while i != idx_end {
        pl.push(contour[i]);
        if i == 0 {
            i = contour.len();
        }
        i -= 1;
    }
    pl.push(contour[i]);
}

/// Adds contour points going clockwise from idx_start, at most `length_to_take` long.
fn take_cw_limited(
    pl: &mut Polyline,
    contour: &[Point],
    params: &[f64],
    idx_start: usize,
    idx_end: usize,
    length_to_take: f64,
) -> f64 {
    let length = *params.last().unwrap();
    let p0 = params[idx_start];
    let mut i = if idx_start == 0 {
        contour.len() - 1
    } else {
        idx_start - 1
    };
    let mut iprev = idx_start;
    let mut lprev = 0.0;
    loop {
        let l = closed_contour_distance_cw(p0, params[i], length);
        if l >= length_to_take {
            let t = (length_to_take - lprev) / (l - lprev);
            pl.push(lerp(contour[iprev], contour[i], t));
            return length_to_take;
        }
        pl.push(contour[i]);
        if i == idx_end {
            return l;
        }
        iprev = i;
        lprev = l;
        if i == 0 {
            i = contour.len();
        }
        i -= 1;
    }
}

/// Adds contour points of the interval (idx_start, idx_end] going counter-clockwise.
fn take_ccw_full(pl: &mut Polyline, contour: &[Point], idx_start: usize, idx_end: usize) {
    let mut i = idx_start + 1;
    if i == contour.len() {
        i = 0;
    }
    while i != idx_end {
        pl.push(contour[i]);
        i += 1;
        if i == contour.len() {
            i = 0;
        }
    }
    pl.push(contour[i]);
}

/// Adds contour points going counter-clockwise from idx_start, at most `length_to_take` long.
fn take_ccw_limited(
    pl: &mut Polyline,
    contour: &[Point],
    params: &[f64],
    idx_start: usize,
    idx_end: usize,
    length_to_take: f64,
) -> f64 {
    let length = *params.last().unwrap();
    let p0 = params[idx_start];
    let mut i = idx_start + 1;
    if i == contour.len() {
        i = 0;
    }
    let mut iprev = idx_start;
    let mut lprev = 0.0;
    loop {
        let l = closed_contour_distance_ccw(p0, params[i], length);
        if l >= length_to_take {
            let t = (length_to_take - lprev) / (l - lprev);
            pl.push(lerp(contour[iprev], contour[i], t));
            return length_to_take;
        }
        pl.push(contour[i]);
        if i == idx_end {
            return l;
        }
        iprev = i;
        lprev = l;
        i += 1;
        if i == contour.len() {
            i = 0;
        }
    }
}

/// Connects the end of `pl1` to the start of `pl2` along the contour from `cp_start` to `cp_end`,
/// marking the contour in between as consumed.
fn take(
    graph: &mut BoundaryInfillGraph,
    pl1: &mut Polyline,
    pl2: &Polyline,
    mut cp_start: usize,
    mut cp_end: usize,
    clockwise: bool,
) {
    let contour = &graph.boundary[graph.cps[cp_start].contour_idx];
    let (idx_start, idx_end) = (graph.cps[cp_start].point_idx, graph.cps[cp_end].point_idx);
    if clockwise {
        take_cw_full(pl1, contour, idx_start, idx_end);
    } else {
        take_ccw_full(pl1, contour, idx_start, idx_end);
    }
    pl1.extend_from_slice(&pl2[1..]);

    if clockwise {
        std::mem::swap(&mut cp_start, &mut cp_end);
    }
    let cps = &mut graph.cps;
    if cps[cp_start].next_on_contour != cp_end {
        let mut cp = cps[cp_start].next_on_contour;
        while cps[cp].next_on_contour != cp_end {
            cps[cp].consume_prev();
            cps[cp].consume_next();
            cp = cps[cp].next_on_contour;
        }
    }
    cps[cp_start].consume_next();
    cps[cp_end].consume_prev();
}

/// Extends `pl1` from `cp_start` along the contour towards `cp_end` by at most `take_max_length`,
/// stopping half a line width short of other infill lines.
#[allow(clippy::too_many_arguments)]
fn take_limited(
    graph: &mut BoundaryInfillGraph,
    pl1: &mut Polyline,
    cp_start: usize,
    cp_end: usize,
    clockwise: bool,
    take_max_length: f64,
    line_half_width: f64,
) {
    let start = graph.cps[cp_start];
    if pl1.is_empty() {
        return;
    }
    if !(if clockwise {
        start.could_take_prev()
    } else {
        start.could_take_next()
    }) {
        return;
    }
    let contour_idx = start.contour_idx;
    let contour = &graph.boundary[contour_idx];
    let params = &graph.boundary_params[contour_idx];
    let cps = &mut graph.cps;

    let add_at_start = pl1[0] == contour[start.point_idx];
    let pl_tmp = if add_at_start {
        std::mem::take(pl1)
    } else {
        Vec::new()
    };

    let length = *params.last().unwrap();
    let mut length_to_go = take_max_length;
    cps[cp_start].consumed = true;
    if cp_start == cp_end {
        length_to_go = length_to_go.min(length - line_half_width).max(0.0);
        length_to_go = length_to_go.min(if clockwise {
            cps[cp_start].contour_not_taken_length_prev
        } else {
            cps[cp_start].contour_not_taken_length_next
        });
        cps[cp_start].consume_prev();
        cps[cp_start].consume_next();
        if length_to_go > SCALED_EPSILON as f64 {
            let p = cps[cp_start].point_idx;
            if clockwise {
                take_cw_limited(pl1, contour, params, p, p, length_to_go);
            } else {
                take_ccw_limited(pl1, contour, params, p, p, length_to_go);
            }
        }
    } else if clockwise {
        let mut cp = cp_start;
        while cp != cp_end {
            let prev = cps[cp].prev_on_contour;
            let l = closed_contour_distance_cw(cps[cp].param, cps[prev].param, length);
            length_to_go = length_to_go.min(cps[cp].contour_not_taken_length_prev);
            // Don't overlap with an already extruded infill line.
            length_to_go = length_to_go.min(l - line_half_width).max(0.0);
            cps[cp].consume_prev();
            if l >= length_to_go {
                if length_to_go > SCALED_EPSILON as f64 {
                    cps[prev].trim_next(l - length_to_go);
                    take_cw_limited(
                        pl1,
                        contour,
                        params,
                        cps[cp].point_idx,
                        cps[prev].point_idx,
                        length_to_go,
                    );
                }
                break;
            }
            cps[prev].trim_next(0.0);
            take_cw_full(pl1, contour, cps[cp].point_idx, cps[prev].point_idx);
            length_to_go -= l;
            cp = prev;
        }
    } else {
        let mut cp = cp_start;
        while cp != cp_end {
            let next = cps[cp].next_on_contour;
            let l = closed_contour_distance_ccw(cps[cp].param, cps[next].param, length);
            length_to_go = length_to_go.min(cps[cp].contour_not_taken_length_next);
            length_to_go = length_to_go.min(l - line_half_width).max(0.0);
            cps[cp].consume_next();
            if l >= length_to_go {
                if length_to_go > SCALED_EPSILON as f64 {
                    cps[next].trim_prev(l - length_to_go);
                    take_ccw_limited(
                        pl1,
                        contour,
                        params,
                        cps[cp].point_idx,
                        cps[next].point_idx,
                        length_to_go,
                    );
                }
                break;
            }
            cps[next].trim_prev(0.0);
            take_ccw_full(pl1, contour, cps[cp].point_idx, cps[next].point_idx);
            length_to_go -= l;
            cp = next;
        }
    }

    if add_at_start {
        pl1.reverse();
        pl1.extend(pl_tmp);
    }
}

/// A point at `distance` along a polyline from its start (or its end): segment index, parameter on the
/// segment (from the segment start) and the point.
#[derive(Clone, Copy)]
struct SegmentPoint {
    idx_segment: usize,
    t: f64,
    point: (f64, f64),
}

fn clip_start_segment_and_point(polyline: &[Point], mut distance: f64) -> Option<SegmentPoint> {
    let mut prev = (polyline[0].x as f64, polyline[0].y as f64);
    for (i, p) in polyline.iter().enumerate().skip(1) {
        let pt = (p.x as f64, p.y as f64);
        let v = (pt.0 - prev.0, pt.1 - prev.1);
        let l = (v.0 * v.0 + v.1 * v.1).sqrt();
        if l > distance {
            let t = distance / l;
            return Some(SegmentPoint {
                idx_segment: i - 1,
                t,
                point: (prev.0 + t * v.0, prev.1 + t * v.1),
            });
        }
        distance -= l;
        prev = pt;
    }
    None
}

fn clip_end_segment_and_point(polyline: &[Point], mut distance: f64) -> Option<SegmentPoint> {
    let last = polyline[polyline.len() - 1];
    let mut next = (last.x as f64, last.y as f64);
    for i in (0..polyline.len() - 1).rev() {
        let pt = (polyline[i].x as f64, polyline[i].y as f64);
        let v = (pt.0 - next.0, pt.1 - next.1);
        let l = (v.0 * v.0 + v.1 * v.1).sqrt();
        if l > distance {
            let t = distance / l;
            return Some(SegmentPoint {
                idx_segment: i,
                t: 1.0 - t,
                point: (next.0 + t * v.0, next.1 + t * v.1),
            });
        }
        distance -= l;
        next = pt;
    }
    None
}

type V2 = (f64, f64);

fn sub(a: V2, b: V2) -> V2 {
    (a.0 - b.0, a.1 - b.1)
}
fn dot(a: V2, b: V2) -> f64 {
    a.0 * b.0 + a.1 * b.1
}

/// Squared distance from `p` to the segment a–b.
fn distance_to_segment_squared(a: V2, b: V2, p: V2) -> f64 {
    let v = sub(b, a);
    let va = sub(p, a);
    let l2 = dot(v, v);
    if l2 == 0.0 {
        return dot(va, va);
    }
    let t = dot(va, v);
    if t <= 0.0 {
        dot(va, va)
    } else if t >= l2 {
        let vb = sub(p, b);
        dot(vb, vb)
    } else {
        let w = (t / l2 * v.0, t / l2 * v.1);
        let d = sub(va, w);
        dot(d, d)
    }
}

/// `Geometry::liang_barsky_line_clipping_interval` against an axis-aligned box.
fn liang_barsky_interval(x0: V2, v: V2, min: V2, max: V2) -> Option<(f64, f64)> {
    let mut t0 = 0.0f64;
    let mut t1 = 1.0f64;
    let mut clip_side = |p: f64, q: f64| -> bool {
        if p == 0.0 {
            if q < 0.0 {
                return false;
            }
        } else {
            let r = q / p;
            if p < 0.0 {
                if r > t1 {
                    return false;
                }
                if r > t0 {
                    t0 = r;
                }
            } else {
                if r < t0 {
                    return false;
                }
                if r < t1 {
                    t1 = r;
                }
            }
        }
        true
    };
    if clip_side(-v.0, -min.0 + x0.0)
        && clip_side(v.0, max.0 - x0.0)
        && clip_side(-v.1, -min.1 + x0.1)
        && clip_side(v.1, max.1 - x0.1)
    {
        Some((t0, t1))
    } else {
        None
    }
}

/// Intersections of the ray line_pt + t·line_vec with a circle; `Geometry::ray_circle_intersections_r2_lv2_c`.
fn ray_circle_interval(segment_pt: V2, r2: f64, line_pt: V2, line_vec: V2) -> Option<(f64, f64)> {
    let p0 = sub(line_pt, segment_pt);
    let lv2 = dot(line_vec, line_vec);
    let (a, b) = (line_vec.1, -line_vec.0);
    let c = -line_vec.1 * p0.0 + line_vec.0 * p0.1;
    let x0 = -a * c;
    let y0 = -b * c;
    let d2 = r2 * lv2 - c * c;
    if d2 < 0.0 {
        return None;
    }
    let d = d2.sqrt();
    let first = ((x0 + b * d) / lv2, (y0 - a * d) / lv2);
    let second = ((x0 - b * d) / lv2, (y0 + a * d) / lv2);
    let mut tmin = dot(sub(first, p0), line_vec) / lv2;
    let mut tmax = dot(sub(second, p0), line_vec) / lv2;
    if tmin > tmax {
        std::mem::swap(&mut tmin, &mut tmax);
    }
    let tmin = tmin.max(0.0);
    let tmax = tmax.min(1.0);
    (tmin <= tmax).then_some((tmin, tmax))
}

/// Overlap of the line a–b with the segment thickened by `offset` (with round caps), as Euclidean
/// distances along the line.
fn line_rounded_thick_segment_collision(
    line_a: V2,
    line_b: V2,
    segment_a: V2,
    segment_b: V2,
    offset: f64,
) -> Option<(f64, f64)> {
    let line_v0 = sub(line_b, line_a);
    let lv = dot(line_v0, line_v0);
    let segment_v = sub(segment_b, segment_a);
    let segment_l = dot(segment_v, segment_v).sqrt();
    let offset2 = offset * offset;
    let eps = SCALED_EPSILON as f64;
    if lv < eps * eps {
        // Very short line: test whether its centre is inside the thick segment.
        let lpt = (0.5 * (line_a.0 + line_b.0), 0.5 * (line_a.1 + line_b.1));
        let inside = if segment_l > eps {
            distance_to_segment_squared(segment_a, segment_b, lpt) < offset2
        } else {
            let mid = (
                0.5 * (segment_a.0 + segment_b.0),
                0.5 * (segment_a.1 + segment_b.1),
            );
            let d = sub(mid, lpt);
            dot(d, d) < offset2
        };
        return inside.then(|| (0.0, lv.sqrt()));
    }
    let mut tmin = f64::MAX;
    let mut tmax = -f64::MAX;
    let mut extend = |i: Option<(f64, f64)>| {
        if let Some((a, b)) = i {
            tmin = tmin.min(a);
            tmax = tmax.max(b);
        }
    };
    if segment_l > eps {
        extend(ray_circle_interval(segment_a, offset2, line_a, line_v0));
        extend(ray_circle_interval(segment_b, offset2, line_a, line_v0));
        // Clip the line in the coordinate space of the segment, spanning (0, 0) to (segment_l, 0).
        let dir_x = (segment_v.0 / segment_l, segment_v.1 / segment_l);
        let dir_y = (-dir_x.1, dir_x.0);
        let p0 = sub(line_a, segment_a);
        extend(liang_barsky_interval(
            (dot(p0, dir_x), dot(p0, dir_y)),
            (dot(line_v0, dir_x), dot(line_v0, dir_y)),
            (0.0, -offset),
            (segment_l, offset),
        ));
    } else {
        // Upstream passes the radius (not its square) here.
        let mid = (
            0.5 * (segment_a.0 + segment_b.0),
            0.5 * (segment_a.1 + segment_b.1),
        );
        extend(ray_circle_interval(mid, offset, line_a, line_v0));
    }
    if tmin <= tmax {
        let l = lv.sqrt();
        Some((tmin * l, tmax * l))
    } else {
        None
    }
}

/// Marks the boundary pieces lying within `distance_colliding` of an infill line (ignoring the first and
/// last `clip_distance` of each line) as not available for anchors or connections.
fn mark_boundary_segments_touching_infill(
    graph: &mut BoundaryInfillGraph,
    boundary_intersections: &[Vec<usize>],
    infill: &[Polyline],
    clip_distance: f64,
    distance_colliding: f64,
) {
    let radius = distance_colliding;
    // Bounding boxes of the boundary segments, for the scan below.
    let seg_boxes: Vec<Vec<(V2, V2)>> = graph
        .boundary
        .iter()
        .map(|c| {
            (0..c.len())
                .map(|i| {
                    let (a, b) = (c[i], c[(i + 1) % c.len()]);
                    (
                        (a.x.min(b.x) as f64, a.y.min(b.y) as f64),
                        (a.x.max(b.x) as f64, a.y.max(b.y) as f64),
                    )
                })
                .collect()
        })
        .collect();

    for polyline in infill {
        if polyline.len() < 2 {
            continue;
        }
        let (Some(start), Some(end)) = (
            clip_start_segment_and_point(polyline, clip_distance),
            clip_end_segment_and_point(polyline, clip_distance),
        ) else {
            continue;
        };
        if !(start.idx_segment < end.idx_segment
            || (start.idx_segment == end.idx_segment && start.t < end.t))
        {
            continue;
        }
        for point_idx in start.idx_segment..=end.idx_segment {
            let pt1 = if point_idx == start.idx_segment {
                start.point
            } else {
                let p = polyline[point_idx];
                (p.x as f64, p.y as f64)
            };
            let pt2 = if point_idx == end.idx_segment {
                end.point
            } else {
                let p = polyline[point_idx + 1];
                (p.x as f64, p.y as f64)
            };
            let inflate = radius + SCALED_EPSILON as f64;
            let ib_min = (pt1.0.min(pt2.0) - inflate, pt1.1.min(pt2.1) - inflate);
            let ib_max = (pt1.0.max(pt2.0) + inflate, pt1.1.max(pt2.1) + inflate);
            for (contour_idx, intersections) in boundary_intersections.iter().enumerate() {
                if intersections.is_empty() {
                    // No infill line touches this contour.
                    continue;
                }
                let contour = &graph.boundary[contour_idx];
                for (seg_idx, (bmin, bmax)) in seg_boxes[contour_idx].iter().enumerate() {
                    if bmin.0 > ib_max.0
                        || bmax.0 < ib_min.0
                        || bmin.1 > ib_max.1
                        || bmax.1 < ib_min.1
                    {
                        continue;
                    }
                    let a = contour[seg_idx];
                    let b = contour[(seg_idx + 1) % contour.len()];
                    let seg_pt1 = (a.x as f64, a.y as f64);
                    let seg_pt2 = (b.x as f64, b.y as f64);
                    let Some(interval) =
                        line_rounded_thick_segment_collision(seg_pt1, seg_pt2, pt1, pt2, radius)
                    else {
                        continue;
                    };
                    let params = &graph.boundary_params[contour_idx];
                    let contour_length = *params.last().unwrap();
                    let param_seg_pt1 = params[seg_idx];
                    let param_seg_pt2 = params[seg_idx + 1];
                    let param_overlap1 = param_seg_pt2.min(param_seg_pt1 + interval.0);
                    let param_overlap2 = param_seg_pt2.min(param_seg_pt1 + interval.1);
                    // The span of T-joints trimmed by (param_overlap1, param_overlap2).
                    let cps = &mut graph.cps;
                    let (ip_low, ip_high) = if intersections.len() == 1 {
                        (intersections[0], intersections[0])
                    } else {
                        let lower = |param: f64| {
                            let k = intersections.partition_point(|&c| cps[c].param < param);
                            intersections.get(k).copied().unwrap_or(intersections[0])
                        };
                        let mut ip_low = lower(param_overlap1);
                        let ip_high = lower(param_overlap2);
                        if cps[ip_low].param != param_overlap1 {
                            ip_low = cps[ip_low].prev_on_contour;
                        }
                        (ip_low, ip_high)
                    };
                    if cps[ip_low].next_on_contour != ip_high {
                        let mut ip = cps[ip_low].next_on_contour;
                        while ip != ip_high {
                            cps[ip].consume_prev();
                            cps[ip].consume_next();
                            ip = cps[ip].next_on_contour;
                        }
                    }
                    let trim_l = closed_contour_distance_ccw(
                        cps[ip_low].param,
                        param_overlap1,
                        contour_length,
                    );
                    cps[ip_low].trim_next(trim_l);
                    let trim_l = closed_contour_distance_ccw(
                        param_overlap2,
                        cps[ip_high].param,
                        contour_length,
                    );
                    cps[ip_high].trim_prev(trim_l);
                }
            }
        }
    }
}

/// Closest point on the boundary within `search_radius`: (contour, segment start index, t along it).
fn closest_point_on_boundary(
    boundary: &[Polygon],
    pt: Point,
    search_radius: f64,
) -> Option<(usize, usize, f64)> {
    let mut d_min = search_radius;
    let mut result = None;
    for (contour_idx, contour) in boundary.iter().enumerate() {
        let n = contour.len();
        for ipt in 0..n {
            let p1 = contour[ipt];
            let p2 = contour[(ipt + 1) % n];
            let v_seg = (p2.x - p1.x, p2.y - p1.y);
            let v_pt = (pt.x - p1.x, pt.y - p1.y);
            let t_pt = v_seg.0 as i128 * v_pt.0 as i128 + v_seg.1 as i128 * v_pt.1 as i128;
            let l2_seg = v_seg.0 as i128 * v_seg.0 as i128 + v_seg.1 as i128 * v_seg.1 as i128;
            if t_pt < 0 {
                // Closest to p1, if inside the wedge between the previous and this segment.
                let dabs = ((v_pt.0 as f64).powi(2) + (v_pt.1 as f64).powi(2)).sqrt();
                if dabs < d_min {
                    let p0 = contour[if ipt == 0 { n - 1 } else { ipt - 1 }];
                    let v_prev = (p1.x - p0.x, p1.y - p0.y);
                    let t2 = v_prev.0 as i128 * v_pt.0 as i128 + v_prev.1 as i128 * v_pt.1 as i128;
                    if t2 > 0 {
                        d_min = dabs;
                        result = Some((contour_idx, ipt, 0.0));
                    }
                }
            } else if t_pt > l2_seg {
                // Closest to p2, which starts the next segment.
                continue;
            } else {
                let d_seg = v_seg.1 as i128 * v_pt.0 as i128 - v_seg.0 as i128 * v_pt.1 as i128;
                let dabs = (d_seg as f64 / (l2_seg as f64).sqrt()).abs();
                if dabs < d_min {
                    d_min = dabs;
                    result = Some((contour_idx, ipt, t_pt as f64 / l2_seg as f64));
                }
            }
        }
    }
    result
}

fn create_boundary_infill_graph(
    infill_ordered: &[Polyline],
    boundary_src: &[Polygon],
    bbox: BoundingBox,
    spacing: f64,
) -> BoundaryInfillGraph {
    let _ = bbox;
    let mut out = BoundaryInfillGraph {
        boundary: vec![Vec::new(); boundary_src.len()],
        boundary_params: vec![Vec::new(); boundary_src.len()],
        cps: vec![ContourIntersectionPoint::unconnected(); infill_ordered.len() * 2],
    };

    // Project the infill end points onto the boundary.
    let mut intersection_points: Vec<((usize, usize, f64), usize)> = Vec::new();
    for (i, pl) in infill_ordered.iter().enumerate() {
        for (k, pt) in [(0, pl[0]), (1, *pl.last().unwrap())] {
            if let Some(cp) = closest_point_on_boundary(boundary_src, pt, SCALED_EPSILON as f64) {
                intersection_points.push((cp, i * 2 + k));
            }
        }
    }
    intersection_points.sort_by(|a, b| {
        (a.0.0, a.0.1)
            .cmp(&(b.0.0, b.0.1))
            .then(a.0.2.total_cmp(&b.0.2))
    });

    let mut it = 0;
    let mut boundary_intersections: Vec<Vec<usize>> = vec![Vec::new(); boundary_src.len()];
    for (idx_contour, contour_src) in boundary_src.iter().enumerate() {
        // Copy the contour while inserting the T-joints, and chain the T-joints along it.
        let contour_dst = &mut out.boundary[idx_contour];
        let mut pfirst = NONE;
        let mut pprev = NONE;
        for (idx_point, &ipt) in contour_src.iter().enumerate() {
            if contour_dst.last() != Some(&ipt) {
                contour_dst.push(ipt);
            }
            while it < intersection_points.len()
                && intersection_points[it].0.0 == idx_contour
                && intersection_points[it].0.1 == idx_point
            {
                let end_idx = intersection_points[it].1;
                let line = &infill_ordered[end_idx / 2];
                let pt = if end_idx & 1 == 1 {
                    *line.last().unwrap()
                } else {
                    line[0]
                };
                let mut idx_tjoint_pt = 0;
                if idx_point + 1 < contour_src.len() || Some(&pt) != contour_dst.first() {
                    if Some(&pt) != contour_dst.last() {
                        contour_dst.push(pt);
                    }
                    idx_tjoint_pt = contour_dst.len() - 1;
                }
                out.cps[end_idx] = ContourIntersectionPoint {
                    contour_idx: idx_contour,
                    point_idx: idx_tjoint_pt,
                    ..ContourIntersectionPoint::unconnected()
                };
                if pprev != NONE {
                    out.cps[pprev].next_on_contour = end_idx;
                    out.cps[end_idx].prev_on_contour = pprev;
                } else {
                    pfirst = end_idx;
                }
                boundary_intersections[idx_contour].push(end_idx);
                pprev = end_idx;
                it += 1;
            }
            if pfirst != NONE {
                out.cps[pprev].next_on_contour = pfirst;
                out.cps[pfirst].prev_on_contour = pprev;
            }
        }
        // Parametrise the new boundary by length.
        let contour_dst = &out.boundary[idx_contour];
        let mut params = vec![0.0; contour_dst.len() + 1];
        for i in 1..contour_dst.len() {
            params[i] = params[i - 1] + contour_dst[i].distance_to(contour_dst[i - 1]);
        }
        if let (Some(&first), Some(&last)) = (contour_dst.first(), contour_dst.last()) {
            let n = params.len();
            params[n - 1] = params[n - 2] + last.distance_to(first);
        }
        for &ip in &boundary_intersections[idx_contour] {
            out.cps[ip].param = params[out.cps[ip].point_idx];
        }
        let contour_length = *params.last().unwrap();
        for &ip in &boundary_intersections[idx_contour] {
            let c = out.cps[ip];
            if c.next_on_contour == ip {
                out.cps[ip].contour_not_taken_length_prev = contour_length;
                out.cps[ip].contour_not_taken_length_next = contour_length;
            } else {
                let prev = out.cps[c.prev_on_contour].param;
                let next = out.cps[c.next_on_contour].param;
                out.cps[ip].contour_not_taken_length_prev =
                    closed_contour_distance_ccw(prev, c.param, contour_length);
                out.cps[ip].contour_not_taken_length_next =
                    closed_contour_distance_ccw(c.param, next, contour_length);
            }
        }
        out.boundary_params[idx_contour] = params;
    }

    // Mark the boundary close to some infill line as consumed.
    let clip_distance = 1.7 * spacing * SCALE;
    // Slightly more than the overlap of the adaptive cubic anchors.
    let distance_colliding = 0.8 * spacing * SCALE;
    mark_boundary_segments_touching_infill(
        &mut out,
        &boundary_intersections,
        infill_ordered,
        clip_distance,
        distance_colliding,
    );
    out
}

/// `Fill::connect_infill`: joins infill lines ending on `boundary` along the boundary and adds anchors.
/// `spacing`, `anchor_length` and `anchor_length_max` are in mm.
pub(crate) fn connect_infill(
    mut infill_ordered: Vec<Polyline>,
    boundary: &[Polygon],
    bbox: BoundingBox,
    spacing: f64,
    anchor_length: f64,
    anchor_length_max: f64,
) -> Vec<Polyline> {
    infill_ordered.retain(|p| p.len() >= 2);
    if infill_ordered.is_empty() {
        return infill_ordered;
    }
    let anchor_length = anchor_length * SCALE;
    let anchor_length_max = anchor_length_max * SCALE;
    let eps = SCALED_EPSILON as f64;

    let mut graph = create_boundary_infill_graph(&infill_ordered, boundary, bbox, spacing);

    // Union-find of merged polylines; the lower index survives.
    let mut merged_with: Vec<usize> = (0..infill_ordered.len()).collect();
    fn get_and_update(merged_with: &mut [usize], idx: usize) -> usize {
        let mut last = idx;
        loop {
            let lower = merged_with[last];
            if lower == last {
                merged_with[idx] = last;
                return last;
            }
            last = lower;
        }
    }

    let line_half_width = 0.5 * spacing * SCALE;

    // Connect neighbouring T-joints, shortest arcs first.
    let mut arches: Vec<(usize, f64)> = (0..graph.cps.len())
        .filter(|&i| {
            let c = &graph.cps[i];
            c.contour_idx != NONE && c.next_on_contour != i && graph.could_connect_next(i)
        })
        .map(|i| {
            let c = &graph.cps[i];
            let len = closed_contour_distance_ccw(
                c.param,
                graph.cps[c.next_on_contour].param,
                graph.contour_length(c.contour_idx),
            );
            (i, len)
        })
        .collect();
    arches.sort_by(|a, b| a.1.total_cmp(&b.1));

    for &(cp1, arc_length) in &arches {
        let cp2 = graph.cps[cp1].next_on_contour;
        if graph.cps[cp1].consumed || graph.cps[cp2].consumed {
            continue;
        }
        let idx1 = get_and_update(&mut merged_with, cp1 / 2);
        let idx2 = get_and_update(&mut merged_with, cp2 / 2);
        if idx1 == idx2 {
            continue;
        }
        let contour_idx = graph.cps[cp1].contour_idx;
        if arc_length < anchor_length_max {
            // Not closing a loop: connect the lines.
            let mut pl1 = std::mem::take(&mut infill_ordered[idx1]);
            let mut pl2 = std::mem::take(&mut infill_ordered[idx2]);
            let contour = &graph.boundary[contour_idx];
            if contour[graph.cps[cp1].point_idx] == pl1[0] {
                pl1.reverse();
            }
            if contour[graph.cps[cp2].point_idx] == *pl2.last().unwrap() {
                pl2.reverse();
            }
            take(&mut graph, &mut pl1, &pl2, cp1, cp2, false);
            if idx2 < idx1 {
                infill_ordered[idx2] = pl1;
                merged_with[idx1] = merged_with[idx2];
            } else {
                infill_ordered[idx1] = pl1;
                merged_with[idx2] = merged_with[idx1];
            }
        } else if anchor_length > eps {
            // Move along the perimeter, but don't take the whole arc.
            take_limited(
                &mut graph,
                &mut infill_ordered[idx1],
                cp1,
                cp2,
                false,
                anchor_length,
                line_half_width,
            );
            take_limited(
                &mut graph,
                &mut infill_ordered[idx2],
                cp2,
                cp1,
                true,
                anchor_length,
                line_half_width,
            );
        }
    }

    // Connect the remaining open line ends to the boundary if possible.
    for cp in 0..graph.cps.len() {
        let c = graph.cps[cp];
        if c.consumed || c.contour_idx == NONE {
            continue;
        }
        let contour_length = graph.contour_length(c.contour_idx);
        let lprev = if graph.could_connect_prev(cp) {
            closed_contour_distance_ccw(graph.cps[c.prev_on_contour].param, c.param, contour_length)
        } else {
            f64::MAX
        };
        let lnext = if graph.could_connect_next(cp) {
            closed_contour_distance_ccw(c.param, graph.cps[c.next_on_contour].param, contour_length)
        } else {
            f64::MAX
        };
        let idx = get_and_update(&mut merged_with, cp / 2);
        if infill_ordered[idx].is_empty() {
            continue;
        }
        let mut connected = false;
        for l in [lprev.min(lnext), lprev.max(lnext)] {
            if l == f64::MAX || l > anchor_length_max {
                break;
            }
            let reversed = l == lprev;
            let cp2 = if reversed {
                c.prev_on_contour
            } else {
                c.next_on_contour
            };
            let idx2 = get_and_update(&mut merged_with, cp2 / 2);
            if idx == idx2 {
                // Try the other side.
                continue;
            }
            let mut pl = std::mem::take(&mut infill_ordered[idx]);
            let mut pl2 = std::mem::take(&mut infill_ordered[idx2]);
            let contour = &graph.boundary[c.contour_idx];
            if contour[c.point_idx] == pl[0] {
                pl.reverse();
            }
            if contour[graph.cps[cp2].point_idx] == *pl2.last().unwrap() {
                pl2.reverse();
            }
            take(&mut graph, &mut pl, &pl2, cp, cp2, reversed);
            if idx < idx2 {
                merged_with[idx2] = idx;
                infill_ordered[idx] = pl;
            } else {
                merged_with[idx] = idx2;
                infill_ordered[idx2] = pl;
            }
            connected = true;
            break;
        }
        if !connected && anchor_length > eps {
            // Hook along the longer free side; that leaves room for a hook on the other side.
            let c = graph.cps[cp];
            let l = c
                .contour_not_taken_length_prev
                .max(c.contour_not_taken_length_next);
            if l > eps {
                let (to, clockwise) =
                    if c.contour_not_taken_length_prev > c.contour_not_taken_length_next {
                        (c.prev_on_contour, true)
                    } else {
                        (c.next_on_contour, false)
                    };
                take_limited(
                    &mut graph,
                    &mut infill_ordered[idx],
                    cp,
                    to,
                    clockwise,
                    anchor_length,
                    line_half_width,
                );
            }
        }
    }

    infill_ordered.retain(|p| !p.is_empty());
    infill_ordered
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::{polyline_length, scale};

    fn square_mm(size: f64) -> Polygon {
        vec![
            Point::from_mm(0.0, 0.0),
            Point::from_mm(size, 0.0),
            Point::from_mm(size, size),
            Point::from_mm(0.0, size),
        ]
    }

    #[test]
    fn neighbouring_lines_are_joined_along_the_boundary() {
        // Two vertical lines 1 mm apart in a 10 mm square: the 1 mm boundary arc joins them.
        let boundary = vec![square_mm(10.0)];
        let lines = vec![
            vec![Point::from_mm(4.0, 0.0), Point::from_mm(4.0, 10.0)],
            vec![Point::from_mm(5.0, 0.0), Point::from_mm(5.0, 10.0)],
        ];
        let bbox = BoundingBox::from_points(boundary[0].iter()).unwrap();
        let out = connect_infill(lines, &boundary, bbox, 0.4, 2.0, 12.0);
        assert_eq!(out.len(), 1);
        let len = polyline_length(&out[0]) / SCALE;
        // 2 × 10 mm lines + 1 mm connection, plus 2 mm anchors on the free ends (clipped by the corner
        // distance of 4 mm, so the full 2 mm fit).
        assert!((len - 25.0).abs() < 0.01, "length {len}");
    }

    #[test]
    fn far_apart_lines_get_limited_anchors() {
        // Lines 5 mm apart with anchor_length_max 3 mm: no connection, just 1 mm anchors.
        let boundary = vec![square_mm(20.0)];
        let lines = vec![
            vec![Point::from_mm(5.0, 0.0), Point::from_mm(5.0, 20.0)],
            vec![Point::from_mm(10.0, 0.0), Point::from_mm(10.0, 20.0)],
        ];
        let bbox = BoundingBox::from_points(boundary[0].iter()).unwrap();
        let out = connect_infill(lines, &boundary, bbox, 0.4, 1.0, 3.0);
        assert_eq!(out.len(), 2);
        for pl in &out {
            let len = polyline_length(pl) / SCALE;
            assert!((len - 22.0).abs() < 0.01, "length {len}");
            // The anchors run along the boundary.
            assert!(pl[0].y == 0 || pl[0].y == scale(20.0));
        }
    }
}
