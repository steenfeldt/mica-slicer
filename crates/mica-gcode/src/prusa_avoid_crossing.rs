// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/AvoidCrossingPerimeters.cpp :: AvoidCrossingPerimeters::travel_to, AvoidCrossingPerimeters::init_layer, avoid_perimeters, avoid_perimeters_inner, extend_for_closest_lines, get_closest_lines_in_radius, get_shortest_direction, simplify_travel, need_wipe, any_expolygon_contains
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/AvoidCrossingPerimeters.cpp :: inner_offset, contour_distance, resample_polygon, precompute_polygon_distances, get_boundary, get_boundary_external, init_boundary
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/AvoidCrossingPerimeters.cpp :: find_first_different_vertex, three_points_inward_normal, get_polygon_vertex_offset, get_middle_point_offset
// Source: prusaslicer@b028299c77 src/libslic3r/Line.hpp :: line_alg::intersection, line_alg::distance_to_squared
// Source: prusaslicer@b028299c77 src/libslic3r/Geometry.hpp :: segments_intersect, liang_barsky_line_clipping_interval
// Source: prusaslicer@b028299c77 src/clipper/clipper.cpp :: PointInPolygon
// Changes: Rust port for the Prusa motion path. EdgeGrid is replaced by a uniform segment grid with the
// Changes: same cell sizes; every query applies the upstream exact predicate, so only the input order of
// Changes: equal-key sorts can differ (the sorts themselves follow libc++). Boundaries are built lazily,
// Changes: as upstream: the internal one from the layer the generator is on at the first query (the
// Changes: previous layer for the layer-change travel). Support layers do not join the boundary; the
// Changes: external boundary uses the plate outlines of this print layer. Clipper2 replaces Clipper.
// Copyright (c) Prusa Research 2020 - 2022 Lukáš Hejl @hejllukas, Vojtěch Bubník @bubnikv

//! Travel planning that follows the inside of the layer's islands instead of crossing perimeters.

use mica_core::{
    config::Config,
    cxx_sort,
    geometry::{
        BoundingBox, ExPolygon, Point, Polygon,
        clip::{Join, diff_ex, offset, offset_ex, to_polygons, union_ex},
        elephant_foot::variable_offset_inner_ex,
    },
};
use std::cell::OnceCell;

const SCALED_EPSILON: i64 = 100;
/// libslic3r `EPSILON`.
const EPSILON: f64 = 1e-4;
const SCALE: f64 = 1_000_000.0;

fn perp(p: (f64, f64)) -> (f64, f64) {
    (-p.1, p.0)
}
fn normalized(p: (f64, f64)) -> (f64, f64) {
    let n = p.0.hypot(p.1);
    if n > 0.0 { (p.0 / n, p.1 / n) } else { p }
}
fn fsub(a: Point, b: Point) -> (f64, f64) {
    ((a.x - b.x) as f64, (a.y - b.y) as f64)
}
/// `(a - b).cast<float>().norm()`.
fn norm_f32(a: Point, b: Point) -> f32 {
    let (x, y) = ((a.x - b.x) as f32, (a.y - b.y) as f32);
    (x * x + y * y).sqrt()
}
fn sq_f32(a: Point, b: Point) -> f32 {
    let (x, y) = ((a.x - b.x) as f32, (a.y - b.y) as f32);
    x * x + y * y
}
fn cast(x: f64) -> i64 {
    // Eigen's cast<coord_t>() truncates towards zero.
    x as i64
}

/// Clipper's `PointInPolygon`: 0 outside, 1 inside, -1 on the boundary.
fn point_in_polygon(pt: Point, path: &[Point]) -> i32 {
    let cnt = path.len();
    if cnt < 3 {
        return 0;
    }
    let mut result = 0;
    let mut ip = path[0];
    for i in 1..=cnt {
        let next = if i == cnt { path[0] } else { path[i] };
        if next.y == pt.y
            && (next.x == pt.x || (ip.y == pt.y && ((next.x > pt.x) == (ip.x < pt.x))))
        {
            return -1;
        }
        if (ip.y < pt.y) != (next.y < pt.y) {
            let d = || {
                i128::from(ip.x - pt.x) * i128::from(next.y - pt.y)
                    - i128::from(next.x - pt.x) * i128::from(ip.y - pt.y)
            };
            if ip.x >= pt.x {
                if next.x > pt.x {
                    result = 1 - result;
                } else {
                    let d = d();
                    if d == 0 {
                        return -1;
                    }
                    if (d > 0) == (next.y > ip.y) {
                        result = 1 - result;
                    }
                }
            } else if next.x > pt.x {
                let d = d();
                if d == 0 {
                    return -1;
                }
                if (d > 0) == (next.y > ip.y) {
                    result = 1 - result;
                }
            }
        }
        ip = next;
    }
    result
}
fn polygon_contains(poly: &[Point], p: Point, border: bool) -> bool {
    match point_in_polygon(p, poly) {
        -1 => border,
        n => n % 2 == 1,
    }
}
/// `ExPolygon::contains(point, border_result = true)`.
fn expolygon_contains(ex: &ExPolygon, p: Point) -> bool {
    polygon_contains(&ex.contour, p, true)
        && !ex.holes.iter().any(|h| polygon_contains(h, p, false))
}

/// `Geometry::segments_intersect`.
fn segments_intersect(ip1: Point, ip2: Point, jp1: Point, jp2: Point) -> bool {
    fn signs(a: Point, b: Point, c: Point, d: Point) -> (i32, i32) {
        let iv = (i128::from(b.x - a.x), i128::from(b.y - a.y));
        let t1 = iv.0 * i128::from(c.y - a.y) - iv.1 * i128::from(c.x - a.x);
        let t2 = iv.0 * i128::from(d.y - a.y) - iv.1 * i128::from(d.x - a.x);
        (t1.signum() as i32, t2.signum() as i32)
    }
    let s1 = signs(ip1, ip2, jp1, jp2);
    let s2 = signs(jp1, jp2, ip1, ip2);
    let test1 = s1.0 * s1.1;
    let test2 = s2.0 * s2.1;
    if test1 <= 0 && test2 <= 0 {
        if test1 != 0 || test2 != 0 {
            return true;
        }
        if s1.0 == 0 && s1.1 == 0 {
            let vi = (ip2.x - ip1.x, ip2.y - ip1.y);
            let axis = |p: Point| if vi.0.abs() > vi.1.abs() { p.x } else { p.y };
            let (mut i, mut j, mut k, mut l) = (axis(ip1), axis(ip2), axis(jp1), axis(jp2));
            if i > j {
                std::mem::swap(&mut i, &mut j);
            }
            if k > l {
                std::mem::swap(&mut k, &mut l);
            }
            return (k >= i && k <= j) || (i >= k && i <= l);
        }
    }
    false
}

/// `line_alg::intersection(l1, l2, &point)` for integer lines.
fn line_intersection(a1: Point, b1: Point, a2: Point, b2: Point) -> Option<Point> {
    let v1 = fsub(b1, a1);
    let v2 = fsub(b2, a2);
    let denom = v1.0 * v2.1 - v1.1 * v2.0;
    if denom.abs() < EPSILON {
        return None;
    }
    let v12 = fsub(a1, a2);
    let nume_a = v2.0 * v12.1 - v2.1 * v12.0;
    let nume_b = v1.0 * v12.1 - v1.1 * v12.0;
    let t1 = nume_a / denom;
    let t2 = nume_b / denom;
    ((0.0..=1.0).contains(&t1) && (0.0..=1.0).contains(&t2))
        .then(|| Point::new(cast(a1.x as f64 + t1 * v1.0), cast(a1.y as f64 + t1 * v1.1)))
}

/// `line_alg::distance_to_squared(line, point, &nearest)`.
fn distance_to_squared(a: Point, b: Point, p: Point) -> (f64, Point) {
    let v = fsub(b, a);
    let va = fsub(p, a);
    let l2 = v.0 * v.0 + v.1 * v.1;
    if l2 == 0.0 {
        return (va.0 * va.0 + va.1 * va.1, a);
    }
    let t = va.0 * v.0 + va.1 * v.1;
    if t <= 0.0 {
        return (va.0 * va.0 + va.1 * va.1, a);
    }
    if t >= l2 {
        let d = fsub(p, b);
        return (d.0 * d.0 + d.1 * d.1, b);
    }
    let w = (t / l2 * v.0, t / l2 * v.1);
    let nearest = Point::new(cast(a.x as f64 + w.0), cast(a.y as f64 + w.1));
    ((w.0 - va.0).powi(2) + (w.1 - va.1).powi(2), nearest)
}

/// Segment index of a set of closed contours (EdgeGrid replacement).
struct Grid {
    min: Point,
    cell: i64,
    cols: i64,
    rows: i64,
    /// Segment ids per cell, in contour/segment creation order.
    cells: Vec<Vec<u32>>,
    /// (contour, segment) of every segment id.
    segments: Vec<(u32, u32)>,
    bbox: BoundingBox,
    /// Visit stamps for one query at a time.
    marks: std::cell::RefCell<(u32, Vec<u32>)>,
}
impl Grid {
    fn new(polygons: &[&[Point]], bbox: BoundingBox, cell: i64) -> Self {
        let cell = cell.max(1);
        let cols = (bbox.max.x - bbox.min.x) / cell + 1;
        let rows = (bbox.max.y - bbox.min.y) / cell + 1;
        let mut grid = Self {
            min: bbox.min,
            cell,
            cols,
            rows,
            cells: vec![Vec::new(); (cols * rows) as usize],
            segments: Vec::new(),
            bbox,
            marks: std::cell::RefCell::new((0, Vec::new())),
        };
        for (c, poly) in polygons.iter().enumerate() {
            for i in 0..poly.len() {
                let a = poly[i];
                let b = poly[(i + 1) % poly.len()];
                let id = grid.segments.len() as u32;
                grid.segments.push((c as u32, i as u32));
                let (x0, x1) = (grid.cx(a.x.min(b.x)), grid.cx(a.x.max(b.x)));
                let (y0, y1) = (grid.cy(a.y.min(b.y)), grid.cy(a.y.max(b.y)));
                for y in y0..=y1 {
                    for x in x0..=x1 {
                        if grid.cell_touches_segment(x, y, a, b) {
                            grid.cells[(y * cols + x) as usize].push(id);
                        }
                    }
                }
            }
        }
        grid.marks.get_mut().1 = vec![0; grid.segments.len()];
        grid
    }
    fn cx(&self, x: i64) -> i64 {
        ((x - self.min.x).div_euclid(self.cell)).clamp(0, self.cols - 1)
    }
    fn cy(&self, y: i64) -> i64 {
        ((y - self.min.y).div_euclid(self.cell)).clamp(0, self.rows - 1)
    }
    /// Conservative: the cell's corners do not all lie strictly on one side of the segment's line.
    fn cell_touches_segment(&self, x: i64, y: i64, a: Point, b: Point) -> bool {
        let x0 = self.min.x + x * self.cell;
        let y0 = self.min.y + y * self.cell;
        let side = |cx: i64, cy: i64| {
            (i128::from(b.x - a.x) * i128::from(cy - a.y)
                - i128::from(b.y - a.y) * i128::from(cx - a.x))
            .signum()
        };
        let s = [
            side(x0, y0),
            side(x0 + self.cell, y0),
            side(x0, y0 + self.cell),
            side(x0 + self.cell, y0 + self.cell),
        ];
        !(s.iter().all(|&v| v > 0) || s.iter().all(|&v| v < 0))
    }
    fn collect(&self, cells: impl Iterator<Item = (i64, i64)>, out: &mut Vec<(u32, u32)>) {
        out.clear();
        let mut marks = self.marks.borrow_mut();
        marks.0 = marks.0.wrapping_add(1);
        if marks.0 == 0 {
            marks.1.iter_mut().for_each(|m| *m = 0);
            marks.0 = 1;
        }
        let stamp = marks.0;
        for (x, y) in cells {
            for &id in &self.cells[(y * self.cols + x) as usize] {
                let m = &mut marks.1[id as usize];
                if *m != stamp {
                    *m = stamp;
                    out.push(self.segments[id as usize]);
                }
            }
        }
    }
    /// Segments in cells touched by the segment `a`–`b`, in first-visit order along the line.
    fn along_line(&self, a: Point, b: Point, out: &mut Vec<(u32, u32)>) {
        let (x0, x1) = (self.cx(a.x.min(b.x)), self.cx(a.x.max(b.x)));
        let (y0, y1) = (self.cy(a.y.min(b.y)), self.cy(a.y.max(b.y)));
        let d = fsub(b, a);
        let len2 = d.0 * d.0 + d.1 * d.1;
        let mut cells: Vec<(i64, i64, f64)> = Vec::new();
        for y in y0..=y1 {
            for x in x0..=x1 {
                if a == b || self.cell_touches_segment(x, y, a, b) {
                    let cxm = (self.min.x + x * self.cell + self.cell / 2 - a.x) as f64;
                    let cym = (self.min.y + y * self.cell + self.cell / 2 - a.y) as f64;
                    let t = if len2 > 0.0 {
                        (cxm * d.0 + cym * d.1) / len2
                    } else {
                        0.0
                    };
                    cells.push((x, y, t));
                }
            }
        }
        cells.sort_by(|p, q| p.2.total_cmp(&q.2));
        self.collect(cells.into_iter().map(|(x, y, _)| (x, y)), out);
    }
    /// Segments in cells overlapping the box, row-major cell order.
    fn in_box(&self, min: Point, max: Point, out: &mut Vec<(u32, u32)>) {
        if max.x < self.bbox.min.x
            || max.y < self.bbox.min.y
            || min.x > self.bbox.max.x
            || min.y > self.bbox.max.y
        {
            out.clear();
            return;
        }
        let (x0, x1, y0, y1) = (
            self.cx(min.x),
            self.cx(max.x),
            self.cy(min.y),
            self.cy(max.y),
        );
        self.collect((y0..=y1).flat_map(|y| (x0..=x1).map(move |x| (x, y))), out);
    }
}

/// `AvoidCrossingPerimeters::Boundary`.
struct Boundary {
    boundaries: Vec<Polygon>,
    bbox: (f64, f64, f64, f64),
    params: Vec<Vec<f32>>,
    grid: Grid,
}
fn precompute_polygon_distances(polygon: &[Point]) -> Vec<f32> {
    let n = polygon.len();
    let mut out = vec![0.0f32; n + 1];
    for i in 1..n {
        let d = fsub(polygon[i], polygon[i - 1]);
        out[i] = out[i - 1] + d.0.hypot(d.1) as f32;
    }
    if n > 0 {
        let d = fsub(polygon[n - 1], polygon[0]);
        out[n] = out[n - 1] + d.0.hypot(d.1) as f32;
    }
    out
}
impl Boundary {
    fn new(boundaries: Vec<Polygon>) -> Option<Self> {
        let bb = BoundingBox::from_points(boundaries.iter().flatten())?;
        let bbox = BoundingBox {
            min: Point::new(bb.min.x - SCALED_EPSILON, bb.min.y - SCALED_EPSILON),
            max: Point::new(bb.max.x + SCALED_EPSILON, bb.max.y + SCALED_EPSILON),
        };
        let refs: Vec<&[Point]> = boundaries.iter().map(Vec::as_slice).collect();
        let grid = Grid::new(&refs, bbox, 1_000_000);
        let params = boundaries
            .iter()
            .map(|p| precompute_polygon_distances(p))
            .collect();
        Some(Self {
            bbox: (
                bbox.min.x as f64,
                bbox.min.y as f64,
                bbox.max.x as f64,
                bbox.max.y as f64,
            ),
            boundaries,
            params,
            grid,
        })
    }
    fn segment(&self, s: (u32, u32)) -> (Point, Point) {
        let p = &self.boundaries[s.0 as usize];
        let i = s.1 as usize;
        (p[i], p[(i + 1) % p.len()])
    }
}

#[derive(Clone, Copy, Debug)]
struct TravelPoint {
    point: Point,
    do_not_remove: bool,
}
#[derive(Clone, Copy, Debug)]
struct Intersection {
    border_idx: usize,
    line_idx: usize,
    point: Point,
    distance: f32,
    do_not_remove: bool,
}
#[derive(Clone, Copy, Debug)]
struct ClosestLine {
    border_idx: usize,
    line_idx: usize,
    point: Point,
}

fn get_closest_lines_in_radius(b: &Boundary, center: Point, radius: f32) -> Vec<ClosestLine> {
    let r = Point::new(
        f64::from(radius).round() as i64,
        f64::from(radius).round() as i64,
    );
    let max_d2 = f64::from(radius * radius);
    let mut segs = Vec::new();
    b.grid.in_box(
        Point::new(center.x - r.x, center.y - r.y),
        Point::new(center.x + r.x, center.y + r.y),
        &mut segs,
    );
    let mut out: Vec<ClosestLine> = segs
        .iter()
        .filter_map(|&s| {
            let (p, q) = b.segment(s);
            let (d2, point) = distance_to_squared(p, q, center);
            (d2 <= max_d2).then_some(ClosestLine {
                border_idx: s.0 as usize,
                line_idx: s.1 as usize,
                point,
            })
        })
        .collect();
    let d2 = |l: &ClosestLine| {
        let d = fsub(center, l.point);
        d.0 * d.0 + d.1 * d.1
    };
    cxx_sort::sort_by(&mut out, |l, r| d2(l) < d2(r));
    out
}

fn extend_for_closest_lines(
    intersections: Vec<Intersection>,
    b: &Boundary,
    start: Point,
    end: Point,
    search_radius: f32,
) -> Vec<Intersection> {
    let start_lines = get_closest_lines_in_radius(b, start, search_radius);
    let end_lines = get_closest_lines_in_radius(b, end, search_radius);
    let compute_distance = |cl: &ClosestLine| -> f32 {
        b.params[cl.border_idx][cl.line_idx]
            + norm_f32(cl.point, b.boundaries[cl.border_idx][cl.line_idx])
    };
    let make = |cl: &ClosestLine| Intersection {
        border_idx: cl.border_idx,
        line_idx: cl.line_idx,
        point: cl.point,
        distance: compute_distance(cl),
        do_not_remove: true,
    };
    if !start_lines.is_empty() && !end_lines.is_empty() {
        let starts: std::collections::HashSet<usize> =
            start_lines.iter().map(|l| l.border_idx).collect();
        for cl_end in &end_lines {
            if starts.contains(&cl_end.border_idx)
                && let Some(cl_start) = start_lines
                    .iter()
                    .find(|l| l.border_idx == cl_end.border_idx)
            {
                return vec![make(cl_start), make(cl_end)];
            }
        }
    }
    let r2 = f64::from(search_radius * search_radius);
    let get_closer = |lines: &[ClosestLine], i: &Intersection, close_to: Point| -> Option<usize> {
        lines.iter().position(|cl| {
            let old = f64::from(sq_f32(close_to, i.point));
            cl.border_idx == i.border_idx
                && old <= r2
                && f64::from(sq_f32(close_to, cl.point)) < old
        })
    };
    let same_boundary =
        |lines: &[ClosestLine], xs: &[Intersection], reverse: bool| -> Option<usize> {
            let indices: std::collections::HashSet<usize> =
                lines.iter().map(|l| l.border_idx).collect();
            let find = |i: &Intersection| lines.iter().position(|l| l.border_idx == i.border_idx);
            if reverse {
                xs.iter()
                    .rev()
                    .find(|i| indices.contains(&i.border_idx))
                    .and_then(find)
            } else {
                xs.iter()
                    .find(|i| indices.contains(&i.border_idx))
                    .and_then(find)
            }
        };
    let mut out = intersections;
    if !out.is_empty() && !start_lines.is_empty() {
        if let Some(i) = get_closer(&start_lines, &out[0], start) {
            out[0] = make(&start_lines[i]);
        } else {
            let i = same_boundary(&start_lines, &out, true).unwrap_or(0);
            out.insert(0, make(&start_lines[i]));
        }
    }
    if !out.is_empty() && !end_lines.is_empty() {
        let last = out.len() - 1;
        if let Some(i) = get_closer(&end_lines, &out[last], end) {
            out[last] = make(&end_lines[i]);
        } else {
            let i = same_boundary(&end_lines, &out, false).unwrap_or(0);
            out.push(make(&end_lines[i]));
        }
    }
    out
}

fn find_first_different_vertex(
    polygon: &[Point],
    point_idx: usize,
    point: Point,
    forward: bool,
) -> Point {
    if point != polygon[point_idx] {
        return polygon[point_idx];
    }
    let n = polygon.len() as i64;
    let mut line_idx = (point_idx as i64 + 1) % n;
    while point == polygon[line_idx as usize] && line_idx != point_idx as i64 {
        line_idx = if forward {
            if line_idx + 1 < n { line_idx + 1 } else { 0 }
        } else if line_idx > 0 {
            line_idx - 1
        } else {
            n - 1
        };
    }
    polygon[line_idx as usize]
}
fn three_points_inward_normal(left: Point, middle: Point, right: Point) -> (f64, f64) {
    let a = normalized(perp(fsub(middle, left)));
    let b = normalized(perp(fsub(right, middle)));
    normalized((a.0 + b.0, a.1 + b.1))
}
fn offset_point(p: Point, n: (f64, f64), offset: i64) -> Point {
    Point::new(
        p.x + cast(n.0 * offset as f64),
        p.y + cast(n.1 * offset as f64),
    )
}
fn get_polygon_vertex_offset(polygon: &[Point], idx: usize, offset: i64) -> Point {
    let n = polygon.len();
    let middle = polygon[idx];
    let left = find_first_different_vertex(polygon, (idx + n - 1) % n, middle, false);
    let right = find_first_different_vertex(polygon, (idx + 1) % n, middle, true);
    offset_point(
        middle,
        three_points_inward_normal(left, middle, right),
        offset,
    )
}
fn get_middle_point_offset(
    polygon: &[Point],
    left_idx: usize,
    right_idx: usize,
    middle: Point,
    offset: i64,
) -> Point {
    let left = find_first_different_vertex(polygon, left_idx, middle, false);
    let right = find_first_different_vertex(polygon, right_idx, middle, true);
    offset_point(
        middle,
        three_points_inward_normal(left, middle, right),
        offset,
    )
}

fn get_shortest_direction(
    b: &Boundary,
    first: &Intersection,
    second: &Intersection,
    contour_length: f32,
) -> bool {
    let poly = &b.boundaries[first.border_idx];
    let (mut d1, mut d2) = (first.distance, second.distance);
    let mut reversed = false;
    if d1 > d2 {
        std::mem::swap(&mut d1, &mut d2);
        reversed = true;
    }
    let mut forward = d2 - d1;
    let mut backward = d1 + contour_length - d2;
    if reversed {
        std::mem::swap(&mut forward, &mut backward);
    }
    let n = poly.len();
    forward -= norm_f32(first.point, poly[first.line_idx]);
    backward -= norm_f32(poly[(first.line_idx + 1) % n], first.point);
    forward -= norm_f32(poly[(second.line_idx + 1) % n], second.point);
    backward -= norm_f32(second.point, poly[second.line_idx]);
    forward < backward
}

fn first_intersection(b: &Boundary, a: Point, c: Point, segs: &mut Vec<(u32, u32)>) -> bool {
    b.grid.along_line(a, c, segs);
    segs.iter().any(|&s| {
        let (p, q) = b.segment(s);
        p != q && a != c && segments_intersect(p, q, a, c)
    })
}

fn simplify_travel(b: &Boundary, travel: &[TravelPoint]) -> Vec<TravelPoint> {
    let mut out = Vec::with_capacity(travel.len());
    out.push(travel[0]);
    let mut segs = Vec::new();
    let mut point_idx = 1;
    while point_idx < travel.len() {
        let current = travel[point_idx - 1].point;
        let mut next = travel[point_idx];
        if !next.do_not_remove {
            let mut idx2 = point_idx + 1;
            while idx2 < travel.len() {
                if travel[idx2].do_not_remove {
                    break;
                }
                if travel[idx2].point == current {
                    next = travel[idx2];
                    point_idx = idx2;
                    idx2 += 1;
                    continue;
                }
                if !first_intersection(b, current, travel[idx2].point, &mut segs) {
                    next = travel[idx2];
                    point_idx = idx2;
                }
                idx2 += 1;
            }
        }
        out.push(next);
        point_idx += 1;
    }
    out
}

fn avoid_perimeters_inner(
    b: &Boundary,
    start: Point,
    end: Point,
    perimeter_spacing: f32,
) -> (Vec<TravelPoint>, usize) {
    let mut segs = Vec::new();
    b.grid.along_line(start, end, &mut segs);
    let mut intersections: Vec<Intersection> = segs
        .iter()
        .filter_map(|&s| {
            let (p, q) = b.segment(s);
            line_intersection(start, end, p, q).map(|point| Intersection {
                border_idx: s.0 as usize,
                line_idx: s.1 as usize,
                point,
                distance: b.params[s.0 as usize][s.1 as usize]
                    + norm_f32(point, b.boundaries[s.0 as usize][s.1 as usize]),
                do_not_remove: false,
            })
        })
        .collect();
    let dir = fsub(end, start);
    cxx_sort::sort_by(&mut intersections, |l, r| {
        let d = fsub(r.point, l.point);
        d.0 * dir.0 + d.1 * dir.1 > 0.0
    });
    let search_radius = 2.0 * perimeter_spacing;
    let intersections = extend_for_closest_lines(intersections, b, start, end, search_radius);

    let boundaries = &b.boundaries;
    let mut result = vec![TravelPoint {
        point: start,
        do_not_remove: false,
    }];
    let mut i = 0;
    while i < intersections.len() {
        let first = intersections[i];
        let poly = &boundaries[first.border_idx];
        let n = poly.len();
        let left = first.line_idx;
        let right = if first.line_idx + 1 == n {
            0
        } else {
            first.line_idx + 1
        };
        result.push(TravelPoint {
            point: get_middle_point_offset(poly, left, right, first.point, SCALED_EPSILON),
            do_not_remove: first.do_not_remove,
        });
        // The farthest later intersection with the same border.
        if let Some(j) = (i + 1..intersections.len())
            .rev()
            .find(|&j| intersections[j].border_idx == first.border_idx)
        {
            let second = intersections[j];
            let forward = get_shortest_direction(
                b,
                &first,
                &second,
                *b.params[first.border_idx].last().unwrap(),
            );
            let mut line_idx = first.line_idx as i64;
            let n_i = n as i64;
            if forward {
                while line_idx != second.line_idx as i64 {
                    let v = if line_idx + 1 == n_i { 0 } else { line_idx + 1 };
                    result.push(TravelPoint {
                        point: get_polygon_vertex_offset(poly, v as usize, SCALED_EPSILON),
                        do_not_remove: false,
                    });
                    line_idx = if line_idx + 1 < n_i { line_idx + 1 } else { 0 };
                }
            } else {
                while line_idx != second.line_idx as i64 {
                    result.push(TravelPoint {
                        point: get_polygon_vertex_offset(
                            &boundaries[second.border_idx],
                            line_idx as usize,
                            SCALED_EPSILON,
                        ),
                        do_not_remove: false,
                    });
                    line_idx = if line_idx > 0 { line_idx - 1 } else { n_i - 1 };
                }
            }
            let poly2 = &boundaries[second.border_idx];
            let left = second.line_idx;
            let right = if second.line_idx >= poly2.len() - 1 {
                0
            } else {
                second.line_idx + 1
            };
            result.push(TravelPoint {
                point: get_middle_point_offset(poly2, left, right, second.point, SCALED_EPSILON),
                do_not_remove: second.do_not_remove,
            });
            i = j;
        }
        i += 1;
    }
    result.push(TravelPoint {
        point: end,
        do_not_remove: false,
    });
    if !intersections.is_empty() {
        result = simplify_travel(b, &result);
    }
    (result, intersections.len())
}

/// `liang_barsky_line_clipping`.
fn clip_line(
    a: (f64, f64),
    b: (f64, f64),
    bbox: (f64, f64, f64, f64),
) -> Option<((f64, f64), (f64, f64))> {
    let v = (b.0 - a.0, b.1 - a.1);
    let (mut t0, mut t1) = (0.0f64, 1.0f64);
    let mut clip = |p: f64, q: f64| -> bool {
        if p == 0.0 {
            return q >= 0.0;
        }
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
        true
    };
    if clip(-v.0, -bbox.0 + a.0)
        && clip(v.0, bbox.2 - a.0)
        && clip(-v.1, -bbox.1 + a.1)
        && clip(v.1, bbox.3 - a.1)
    {
        Some((
            (a.0 + t0 * v.0, a.1 + t0 * v.1),
            (a.0 + t1 * v.0, a.1 + t1 * v.1),
        ))
    } else {
        None
    }
}

// ---------------------------------------------------------------------------------------------
// Boundary construction.

fn resample_polygon(polygon: &[Point], dist_from_vertex: f64, max_allowed: f64) -> Polygon {
    let n = polygon.len();
    let mut out = Vec::with_capacity(3 * n);
    for i in 0..n {
        out.push(polygon[i]);
        let p1 = polygon[i];
        let p2 = polygon[(i + 1) % n];
        let v = fsub(p2, p1);
        let length = v.0.hypot(v.1);
        let u = normalized(v);
        let off = Point::new(cast(u.0 * dist_from_vertex), cast(u.1 * dist_from_vertex));
        if length > 2.0 * dist_from_vertex && off != Point::new(0, 0) {
            out.push(Point::new(p1.x + off.x, p1.y + off.y));
            let nv = (
                (p2.x - p1.x - 2 * off.x) as f64,
                (p2.y - p1.y - 2 * off.y) as f64,
            );
            let nl = nv.0.hypot(nv.1);
            if nl > max_allowed {
                let prev = *out.last().unwrap();
                let parts = (nl / max_allowed).ceil() as usize;
                for part in 1..parts {
                    let t = part as f64 / parts as f64;
                    out.push(Point::new(
                        cast(prev.x as f64 + nv.0 * t),
                        cast(prev.y as f64 + nv.1 * t),
                    ));
                }
            }
            out.push(Point::new(p2.x - off.x, p2.y - off.y));
        }
    }
    out
}

/// `contour_distance` of AvoidCrossingPerimeters (a variant of the elephant-foot one).
fn contour_distance(
    ex: &ExPolygon,
    distances: &[Vec<f32>],
    idx_contour: usize,
    compensation: f64,
    search_radius: f64,
) -> Vec<f32> {
    let polygons: Vec<&[Point]> = std::iter::once(ex.contour.as_slice())
        .chain(ex.holes.iter().map(Vec::as_slice))
        .collect();
    let polygon = polygons[idx_contour];
    if polygon.len() <= 2 {
        return Vec::new();
    }
    let params = &distances[idx_contour];
    let accept = 0.5 * compensation * std::f64::consts::PI;
    let reject = search_radius;
    let n = polygon.len();
    let prev = |c: &[Point], i: usize| c[(i + c.len() - 1) % c.len()];
    let next = |c: &[Point], i: usize| c[(i + 1) % c.len()];
    let inside_corner = |c: &[Point], i: usize, opposite: Point| {
        let pt = (opposite.x as f64, opposite.y as f64);
        let (pp, pt_this, pn) = (prev(c, i), c[i], next(c, i));
        let v1 = fsub(pt_this, pp);
        let v2 = fsub(pn, pt_this);
        let left1 = v1.0 * (pt.1 - pp.y as f64) - v1.1 * (pt.0 - pp.x as f64) > 0.0;
        let left2 = v2.0 * (pt.1 - pt_this.y as f64) - v2.1 * (pt.0 - pt_this.x as f64) > 0.0;
        if v1.0 * v2.1 - v1.1 * v2.0 > 0.0 {
            left1 && left2
        } else {
            left1 || left2
        }
    };
    let mut out = Vec::with_capacity(n);
    for (idx_point, &pt) in polygon.iter().enumerate() {
        let v1 = fsub(pt, prev(polygon, idx_point));
        let v2 = fsub(next(polygon, idx_point), pt);
        let dir_inside = (-v1.1 - v2.1, v1.0 + v2.0);
        let mut found = false;
        let mut distance = f64::MAX;
        for (ci, contour) in polygons.iter().enumerate() {
            for ipt in 0..contour.len() {
                let a = contour[ipt];
                let b = next(contour, ipt);
                if (a.x.min(b.x) as f64) > pt.x as f64 + search_radius
                    || (a.x.max(b.x) as f64) < pt.x as f64 - search_radius
                    || (a.y.min(b.y) as f64) > pt.y as f64 + search_radius
                    || (a.y.max(b.y) as f64) < pt.y as f64 - search_radius
                {
                    continue;
                }
                let v = fsub(b, a);
                let va = fsub(pt, a);
                let l2 = v.0 * v.0 + v.1 * v.1;
                let t = if l2 == 0.0 {
                    0.0
                } else {
                    ((va.0 * v.0 + va.1 * v.1) / l2).clamp(0.0, 1.0)
                };
                let foot = (a.x as f64 + t * v.0, a.y as f64 + t * v.1);
                let bis = (foot.0 - pt.x as f64, foot.1 - pt.y as f64);
                let dist = bis.0.hypot(bis.1);
                if (!found || dist < distance) && dir_inside.0 * bis.0 + dir_inside.1 * bis.1 > 0.0
                {
                    let mut ok = true;
                    if ci == idx_contour {
                        let mut lo = f64::from(params[idx_point]);
                        let mut hi = t * l2.sqrt();
                        let end = f64::from(*params.last().unwrap());
                        if ipt + 1 < contour.len() {
                            hi += f64::from(params[ipt]);
                        }
                        if lo > hi {
                            std::mem::swap(&mut lo, &mut hi);
                        }
                        let along = (hi - lo).min(lo + end - hi);
                        if along < accept {
                            ok = false;
                        } else if dist < reject + SCALED_EPSILON as f64 {
                            let inside = if t == 0.0 {
                                inside_corner(contour, ipt, pt)
                            } else if t == 1.0 {
                                inside_corner(contour, (ipt + 1) % contour.len(), pt)
                            } else {
                                v.0 * (pt.y as f64 - a.y as f64) - v.1 * (pt.x as f64 - a.x as f64)
                                    > 0.0
                            };
                            ok = inside && along > 0.6 * std::f64::consts::PI * dist;
                        }
                    }
                    if ok && (!found || dist < distance) {
                        distance = dist;
                        found = true;
                    }
                }
            }
        }
        out.push(if found {
            distance.min(search_radius) as f32
        } else {
            search_radius as f32
        });
    }
    out
}

/// `inner_offset`: offset that falls back to the original polygon where it would break apart.
fn inner_offset(ex_polygons: &[ExPolygon], offset: f64) -> Vec<ExPolygon> {
    let widths = [offset / 2.0, offset, 2.0 * offset + SCALED_EPSILON as f64];
    let mut result: Vec<ExPolygon> = ex_polygons
        .iter()
        .map(|ex| ExPolygon {
            contour: resample_polygon(&ex.contour, offset / 2.0, 0.5 * SCALE),
            holes: ex
                .holes
                .iter()
                .map(|h| resample_polygon(h, offset / 2.0, 0.5 * SCALE))
                .collect(),
        })
        .collect();
    let min_area = (f64::from(0.1f32) * SCALE).powi(2);
    for ex in &mut result {
        let Some(bb) = BoundingBox::from_points(ex.contour.iter().chain(ex.holes.iter().flatten()))
        else {
            continue;
        };
        let size = (
            (bb.max.x - bb.min.x + 2 * SCALED_EPSILON) as f64,
            (bb.max.y - bb.min.y + 2 * SCALED_EPSILON) as f64,
        );
        if size.0 * size.1 < min_area {
            continue;
        }
        for (wi, &min_width) in widths.iter().enumerate() {
            let search_radius = 2.0 * (offset + min_width);
            let distances: Vec<Vec<f32>> = std::iter::once(&ex.contour)
                .chain(ex.holes.iter())
                .map(|p| precompute_polygon_distances(p))
                .collect();
            let offsets: Vec<Vec<f32>> = (0..=ex.holes.len())
                .map(|c| {
                    contour_distance(ex, &distances, c, offset, search_radius)
                        .into_iter()
                        .map(|d| {
                            if f64::from(d) < min_width {
                                0.0
                            } else if f64::from(d) > min_width + 2.0 * offset {
                                -(offset as f32)
                            } else {
                                -(d - min_width as f32) / 2.0
                            }
                        })
                        .collect()
                })
                .collect();
            let mut offset_ex = variable_offset_inner_ex(ex, &offsets);
            if offset_ex.len() == 1 && offset_ex[0].holes.len() == ex.holes.len() {
                *ex = offset_ex.remove(0);
                break;
            } else if wi + 1 < widths.len() {
                continue;
            } else if offset_ex.len() == 1 {
                *ex = offset_ex.remove(0);
                break;
            } else if offset_ex.len() > 1 {
                let mut best = 0;
                let mut best_area = offset_ex[0].area_mm2();
                for (i, e) in offset_ex.iter().enumerate().skip(1) {
                    if best_area < e.area_mm2() {
                        best_area = e.area_mm2();
                        best = i;
                    }
                }
                *ex = offset_ex.swap_remove(best);
                break;
            }
        }
    }
    result
}

/// What the travel planner needs to know about one print layer.
#[derive(Clone, Copy)]
pub struct LayerGeometry<'a> {
    pub lslices: &'a [ExPolygon],
    /// Top fill surfaces (`stTop`), not crossed by internal travel.
    pub tops: &'a [ExPolygon],
    /// Average scaled perimeter spacing of the layer's regions (`get_perimeter_spacing`).
    pub perimeter_spacing: f32,
}

fn get_boundary(layer: &LayerGeometry<'_>) -> Vec<ExPolygon> {
    let spacing = layer.perimeter_spacing;
    let perimeter_offset = spacing / 2.0;
    let boundary = union_ex(&inner_offset(layer.lslices, 1.5 * f64::from(spacing)));
    if layer.tops.is_empty() {
        return boundary;
    }
    let tops = union_ex(layer.tops);
    diff_ex(
        &boundary,
        &offset_ex(&tops, -f64::from(perimeter_offset), Join::Miter),
    )
}

fn get_boundary_external(lslices: &[ExPolygon], spacing: f32) -> Vec<Polygon> {
    let perimeter_offset = spacing / 2.0;
    let mut holes: Vec<Polygon> = lslices
        .iter()
        .flat_map(|ex| ex.holes.iter().cloned())
        .map(|mut h| {
            h.reverse();
            h
        })
        .collect();
    if holes.is_empty() {
        return holes;
    }
    holes = offset(&holes, f64::from(perimeter_offset), Join::Miter, 3.0);
    for p in &mut holes {
        p.reverse();
    }
    holes
}

/// `AvoidCrossingPerimeters` for one print layer.
pub struct Planner {
    lslices_offset: Vec<ExPolygon>,
    lslices_offset_bboxes: Vec<BoundingBox>,
    /// `m_grid_lslices_offset`: segments of the offset slices, over the slices' bounding box.
    lslices_grid: Option<Grid>,
    internal: OnceCell<Option<Boundary>>,
    external: OnceCell<Option<Boundary>>,
    max_detour: f64,
    max_detour_percent: bool,
    z_lift: bool,
    /// Contours then holes of `lslices_offset`, indexed like `lslices_grid`.
    offset_polygons: Vec<Polygon>,
}

/// A planned travel and whether its wipe may be skipped (`could_be_wipe_disabled`).
pub struct Route {
    pub points: Vec<Point>,
    pub wipe_disabled: bool,
}

impl Planner {
    /// `init_layer`. `external_width` is the average scaled external perimeter width.
    pub fn new(
        config: &Config,
        layer: LayerGeometry<'_>,
        external_width: f32,
        z_lift: bool,
    ) -> Self {
        let lslices_offset =
            offset_ex(layer.lslices, -f64::from(external_width / 2.0), Join::Miter);
        let lslices_offset_bboxes = lslices_offset
            .iter()
            .map(|ex| {
                BoundingBox::from_points(ex.contour.iter()).unwrap_or(BoundingBox {
                    min: Point::default(),
                    max: Point::default(),
                })
            })
            .collect();
        let lslices_grid = BoundingBox::from_points(
            layer.lslices.iter().flat_map(|e| e.contour.iter()),
        )
        .map(|bb| {
            let bbox = BoundingBox {
                min: Point::new(bb.min.x - SCALED_EPSILON, bb.min.y - SCALED_EPSILON),
                max: Point::new(bb.max.x + SCALED_EPSILON, bb.max.y + SCALED_EPSILON),
            };
            let polys: Vec<&[Point]> = lslices_offset
                .iter()
                .flat_map(|ex: &ExPolygon| {
                    std::iter::once(ex.contour.as_slice()).chain(ex.holes.iter().map(Vec::as_slice))
                })
                .collect();
            Grid::new(&polys, bbox, 1_000_000)
        });
        let offset_polygons = to_polygons(&lslices_offset);
        let raw = config
            .raw("avoid_crossing_perimeters_max_detour")
            .unwrap_or_default();
        Self {
            lslices_offset,
            lslices_offset_bboxes,
            lslices_grid,
            internal: OnceCell::new(),
            external: OnceCell::new(),
            max_detour: config
                .float("avoid_crossing_perimeters_max_detour")
                .unwrap_or(0.0),
            max_detour_percent: raw.trim_end().ends_with('%'),
            z_lift,
            offset_polygons,
        }
    }

    fn offset_segment(&self, s: (u32, u32)) -> (Point, Point) {
        let p = &self.offset_polygons[s.0 as usize];
        let i = s.1 as usize;
        (p[i], p[(i + 1) % p.len()])
    }
    fn crosses_offset(&self, a: Point, b: Point) -> bool {
        let Some(grid) = &self.lslices_grid else {
            return false;
        };
        let mut segs = Vec::new();
        grid.along_line(a, b, &mut segs);
        segs.iter().any(|&s| {
            let (p, q) = self.offset_segment(s);
            p != q && a != b && segments_intersect(p, q, a, b)
        })
    }
    fn in_grid_bbox(&self, p: Point) -> bool {
        self.lslices_grid.as_ref().is_some_and(|g| {
            p.x >= g.bbox.min.x && p.x <= g.bbox.max.x && p.y >= g.bbox.min.y && p.y <= g.bbox.max.y
        })
    }
    fn bbox_contains(bb: &BoundingBox, p: Point) -> bool {
        p.x >= bb.min.x && p.x <= bb.max.x && p.y >= bb.min.y && p.y <= bb.max.y
    }
    /// `any_expolygon_contains` for a polyline (a line is a two-point polyline).
    fn contained(&self, points: &[Point]) -> bool {
        if points.iter().any(|&p| !self.in_grid_bbox(p)) {
            return false;
        }
        if points.windows(2).any(|w| self.crosses_offset(w[0], w[1])) {
            return false;
        }
        self.lslices_offset
            .iter()
            .zip(&self.lslices_offset_bboxes)
            .any(|(ex, bb)| {
                points.iter().all(|&p| Self::bbox_contains(bb, p))
                    && expolygon_contains(ex, points[0])
            })
    }

    /// `AvoidCrossingPerimeters::travel_to`. `generator_layer` is the layer GCodeGenerator is on (the
    /// previous one during a layer change); it seeds the internal boundary on first use.
    pub fn travel_to(
        &self,
        start: Point,
        end: Point,
        use_external: bool,
        generator_layer: &LayerGeometry<'_>,
    ) -> Route {
        let mut result: Vec<Point> = Vec::new();
        let mut count = 0;
        let startf = (start.x as f64, start.y as f64);
        let endf = (end.x as f64, end.y as f64);
        let boundary =
            if !use_external && !self.lslices_offset.is_empty() && !self.contained(&[start, end]) {
                self.internal
                    .get_or_init(|| Boundary::new(to_polygons(&get_boundary(generator_layer))))
                    .as_ref()
            } else if use_external {
                self.external
                    .get_or_init(|| {
                        Boundary::new(get_boundary_external(
                            generator_layer.lslices,
                            generator_layer.perimeter_spacing,
                        ))
                    })
                    .as_ref()
            } else {
                None
            };
        if let Some(b) = boundary
            && let Some((s, e)) = clip_line(startf, endf, b.bbox)
        {
            let (path, n) = avoid_perimeters_inner(
                b,
                Point::new(cast(s.0), cast(s.1)),
                Point::new(cast(e.0), cast(e.1)),
                generator_layer.perimeter_spacing,
            );
            count = n;
            result = path.iter().map(|t| t.point).collect();
            result[0] = start;
            *result.last_mut().unwrap() = end;
        }
        if result.is_empty() {
            result = vec![start, end];
            count = 0;
        }
        let length = |p: &[Point]| -> f64 {
            p.windows(2)
                .map(|w| {
                    let d = fsub(w[1], w[0]);
                    d.0.hypot(d.1)
                })
                .sum()
        };
        let mut exceeded = false;
        if self.max_detour > 0.0 {
            let direct = length(&[start, end]);
            let detour = length(&result) - direct;
            let max = if self.max_detour_percent {
                direct * 0.01 * self.max_detour
            } else {
                self.max_detour * SCALE
            };
            if detour > max {
                result = vec![start, end];
                exceeded = true;
            }
        }
        let wipe_disabled = if use_external || exceeded {
            false
        } else {
            !self.need_wipe(start, end, &result, count)
        };
        Route {
            points: result,
            wipe_disabled,
        }
    }

    fn need_wipe(&self, a: Point, b: Point, result: &[Point], count: usize) -> bool {
        if count == 0 {
            return false;
        }
        if self.z_lift {
            if self.contained(&[a, b]) {
                !(result.len() > 2 && result[0] == a && *result.last().unwrap() == b)
                    && !self.contained(result)
            } else {
                true
            }
        } else {
            !self.contained(result)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(x0: f64, y0: f64, size: f64) -> Polygon {
        [
            (x0, y0),
            (x0 + size, y0),
            (x0 + size, y0 + size),
            (x0, y0 + size),
        ]
        .iter()
        .map(|p| Point::from_mm(p.0, p.1))
        .collect()
    }

    #[test]
    fn point_in_polygon_matches_clipper_semantics() {
        let sq = square(0., 0., 10.);
        assert_eq!(point_in_polygon(Point::from_mm(5., 5.), &sq), 1);
        assert_eq!(point_in_polygon(Point::from_mm(15., 5.), &sq), 0);
        assert_eq!(point_in_polygon(Point::from_mm(10., 5.), &sq), -1);
        assert_eq!(point_in_polygon(Point::from_mm(0., 0.), &sq), -1);
    }

    #[test]
    fn travel_around_a_hole_follows_the_inside_and_keeps_its_ends() {
        let mut c = Config::defaults();
        c.set_raw("avoid_crossing_perimeters", "1").unwrap();
        // A 20 mm block with a 10 mm square hole in its middle.
        let mut hole = square(5., 5., 10.);
        hole.reverse();
        let lslices = vec![ExPolygon {
            contour: square(0., 0., 20.),
            holes: vec![hole],
        }];
        let layer = LayerGeometry {
            lslices: &lslices,
            tops: &[],
            perimeter_spacing: 450_000.0,
        };
        let planner = Planner::new(&c, layer, 450_000.0, true);
        let a = Point::from_mm(10., 2.);
        let b = Point::from_mm(10., 18.);
        let route = planner.travel_to(a, b, false, &layer);
        assert!(route.points.len() > 2, "{:?}", route.points);
        assert_eq!(route.points[0], a);
        assert_eq!(*route.points.last().unwrap(), b);
        // Never enters the hole (inset by the boundary offset).
        for w in route.points.windows(2) {
            let m = Point::new((w[0].x + w[1].x) / 2, (w[0].y + w[1].y) / 2);
            assert!(
                !polygon_contains(&square(5., 5., 10.), m, false),
                "{:?}",
                route.points
            );
        }
        // A travel inside the material stays straight.
        let straight = planner.travel_to(
            Point::from_mm(2., 2.),
            Point::from_mm(2., 18.),
            false,
            &layer,
        );
        assert_eq!(straight.points.len(), 2);
        assert!(straight.wipe_disabled);
    }

    #[test]
    fn wipe_end_on_the_external_perimeter_line_counts_as_inside() {
        // PrusaSlicer 2.9.6 XL cube20 layer 3: after the wipe along the external perimeter (centre line
        // 0.225 mm inside the slice) the travel to the infill stays straight. The external width
        // 0.45 is float, so scaled_width() truncates to 449999 and the offset slice edge lies half a
        // unit outside that centre line.
        let mut c = Config::defaults();
        c.set_raw("avoid_crossing_perimeters", "1").unwrap();
        let lslices = vec![ExPolygon {
            contour: square(170., 170., 20.),
            holes: vec![],
        }];
        let layer = LayerGeometry {
            lslices: &lslices,
            tops: &[],
            perimeter_spacing: 407079.0,
        };
        let planner = Planner::new(&c, layer, 449999.0, true);
        let a = Point::from_mm(188.295, 189.775);
        let b = Point::from_mm(189.022, 175.67);
        assert_eq!(planner.travel_to(a, b, false, &layer).points, vec![a, b]);
        // With an exact 0.45 mm width the start would lie on the boundary and be routed.
        let exact = Planner::new(&c, layer, 450000.0, true);
        assert!(exact.travel_to(a, b, false, &layer).points.len() > 2);
    }
}
