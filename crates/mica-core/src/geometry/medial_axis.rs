// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: prusaslicer@b028299c77 src/libslic3r/Geometry/MedialAxis.cpp :: MedialAxis::build, MedialAxis::process_edge_neighbors, MedialAxis::validate_edge
// Source: prusaslicer@b028299c77 src/libslic3r/Geometry/VoronoiOffset.cpp :: annotate_inside_outside
// Source: prusaslicer@b028299c77 src/libslic3r/ExPolygon.cpp :: ExPolygon::medial_axis, ExPolygon::on_boundary
// Source: prusaslicer@b028299c77 src/libslic3r/Polyline.hpp :: ThickPolyline
// Source: prusaslicer@b028299c77 src/libslic3r/Polygon.cpp :: Polygon::intersection
// Source: prusaslicer@b028299c77 src/libslic3r/Line.hpp :: line_alg::intersection
// Changes: Rust port. Voronoi edge annotations live in vectors indexed by edge / vertex / cell instead of Boost's
// Changes: colour bits; the debug asserts and SVG dumps are left out.

//! Medial axis ("skeleton") of narrow ExPolygons, with the local width along it.
//!
//! Used for gap fill (the slivers left between wall loops) and thin walls (features too narrow for a wall
//! loop): the centreline is extruded with a width that follows the shape.

use super::clip::closing_ex;
use super::voronoi::{self, CellSource, Diagram, Line};
use super::{ExPolygon, Point, Polygon, polyline_length, signed_area};

/// PrusaSlicer's `SCALED_EPSILON`: 1e-4 mm in scaled units.
pub const SCALED_EPSILON: f64 = 100.0;

/// A polyline with a width at both ends of every segment.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ThickPolyline {
    pub points: Vec<Point>,
    /// Start and end width of each segment (scaled units): `w(a), w(b), w(b), w(c), …`;
    /// always `2 * (points.len() - 1)` long.
    pub width: Vec<f64>,
    /// Whether the first / last point is a free end of the skeleton (not a junction or a loop).
    pub endpoints: (bool, bool),
}

impl ThickPolyline {
    pub fn first_point(&self) -> Point {
        self.points[0]
    }
    pub fn last_point(&self) -> Point {
        *self.points.last().unwrap()
    }
    pub fn length(&self) -> f64 {
        polyline_length(&self.points)
    }
    pub fn reverse(&mut self) {
        self.points.reverse();
        self.width.reverse();
        std::mem::swap(&mut self.endpoints.0, &mut self.endpoints.1);
    }
    /// The segments with their end widths.
    pub fn thicklines(&self) -> Vec<ThickLine> {
        self.points
            .windows(2)
            .enumerate()
            .map(|(i, w)| ThickLine {
                a: w[0],
                b: w[1],
                a_width: self.width[2 * i],
                b_width: self.width[2 * i + 1],
            })
            .collect()
    }
}

/// One segment of a [`ThickPolyline`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ThickLine {
    pub a: Point,
    pub b: Point,
    pub a_width: f64,
    pub b_width: f64,
}

// --- Inside / outside annotation of the Voronoi diagram (VoronoiOffset.cpp). ---

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum VertexCategory {
    Unknown,
    Inside,
    Outside,
    OnContour,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EdgeCategory {
    Unknown,
    PointsInside,
    PointsOutside,
    PointsToContour,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CellCategory {
    Unknown,
    Inside,
    Outside,
    Boundary,
}

/// Boost's `ulp_comparison<double>` equality with the Voronoi vertex tolerance of 128 ULPs.
fn ulp_equal(a: f64, b: f64) -> bool {
    const SIGN: u64 = 0x8000_0000_0000_0000;
    let map = |v: f64| {
        let bits = v.to_bits();
        if bits < SIGN { SIGN - bits } else { bits }
    };
    let (a, b) = (map(a), map(b));
    a.abs_diff(b) <= 128
}

fn vertex_equal_to_point(v: (f64, f64), p: Point) -> bool {
    ulp_equal(v.0, p.x as f64) && ulp_equal(v.1, p.y as f64)
}

fn contour_point(cell: &voronoi::Cell, lines: &[Line]) -> Point {
    voronoi::source_point(cell, lines)
}

fn on_site(lines: &[Line], cell: &voronoi::Cell, p: (f64, f64)) -> bool {
    let line = &lines[cell.source_index];
    if cell.contains_point() {
        vertex_equal_to_point(p, contour_point(cell, lines))
    } else {
        vertex_equal_to_point(p, line.a) || vertex_equal_to_point(p, line.b)
    }
}

struct Annotations {
    vertex: Vec<VertexCategory>,
    edge: Vec<EdgeCategory>,
    cell: Vec<CellCategory>,
}

impl Annotations {
    /// Marks a cell; a cell seen from both sides becomes a boundary cell. Returns whether it changed.
    fn annotate_cell(&mut self, cell: usize, new: CellCategory) -> bool {
        let cc = self.cell[cell];
        let new = match (cc, new) {
            (CellCategory::Boundary, _) => return false,
            (CellCategory::Outside, CellCategory::Inside)
            | (CellCategory::Inside, CellCategory::Outside) => CellCategory::Boundary,
            _ => new,
        };
        if cc != new {
            self.cell[cell] = new;
            true
        } else {
            false
        }
    }
}

/// Classifies the Voronoi vertices, edges and cells as inside, outside or on the input polygons
/// (contours counter-clockwise, holes clockwise). PrusaSlicer's `Voronoi::annotate_inside_outside`.
fn annotate_inside_outside(vd: &Diagram, lines: &[Line]) -> Annotations {
    use CellCategory as C;
    use EdgeCategory as E;
    use VertexCategory as V;
    let mut a = Annotations {
        vertex: vec![V::Unknown; vd.vertices.len()],
        edge: vec![E::Unknown; vd.edges.len()],
        cell: vec![C::Unknown; vd.cells.len()],
    };

    // Vertices on the input contour. Boost merges very close vertices, so test each one against its sites.
    for e in 0..vd.edges.len() {
        if let Some(v) = vd.edges[e].vertex0
            && on_site(lines, vd.cell_of(e), vd.vertex_point(v))
        {
            a.vertex[v] = V::OnContour;
        }
    }
    // One end of a secondary edge always lies on the input contour.
    for e in 0..vd.edges.len() {
        let Some(v0) = vd.edges[e].vertex0 else {
            continue;
        };
        if vd.edges[e].primary {
            continue;
        }
        let cell = vd.cell_of(e);
        let p = if cell.contains_point() {
            contour_point(cell, lines)
        } else {
            contour_point(vd.cell_of(vd.twin(e)), lines)
        };
        match vd.vertex1(e) {
            None => a.vertex[v0] = V::OnContour,
            Some(v1) => {
                if vertex_equal_to_point(vd.vertex_point(v0), p) {
                    a.vertex[v0] = V::OnContour;
                } else {
                    a.vertex[v1] = V::OnContour;
                }
            }
        }
    }

    for e in 0..vd.edges.len() {
        let twin = vd.twin(e);
        if vd.vertex1(e).is_none() {
            // Infinite edge: always outside.
            let Some(v0) = vd.edges[e].vertex0 else {
                continue;
            };
            let secondary = !vd.edges[e].primary;
            a.edge[e] = E::PointsOutside;
            a.edge[twin] = if secondary {
                E::PointsToContour
            } else {
                E::PointsOutside
            };
            a.vertex[v0] = if secondary { V::OnContour } else { V::Outside };
            let (mut cell, mut cell2) = (vd.edges[e].cell, vd.edges[twin].cell);
            if vd.cells[cell].contains_segment() {
                std::mem::swap(&mut cell, &mut cell2);
            }
            a.annotate_cell(cell, C::Outside);
            let c2 = if vd.cells[cell2].contains_point() {
                C::Outside
            } else {
                C::Boundary
            };
            a.annotate_cell(cell2, c2);
        } else if let Some(v0) = vd.edges[e].vertex0 {
            let (cell, cell2) = if vd.cell_of(e).contains_segment() {
                (vd.edges[e].cell, vd.edges[twin].cell)
            } else if vd.cell_of(twin).contains_segment() {
                (vd.edges[twin].cell, vd.edges[e].cell)
            } else {
                // Point–point edges are classified by the seed fill below.
                continue;
            };
            let line = &lines[vd.cells[cell].source_index];
            let v1 = vd.vertex1(e).unwrap();
            let v0c = a.vertex[v0];
            let v1c = a.vertex[v1];
            let on_contour = v0c == V::OnContour || v1c == V::OnContour;
            if on_contour && v1c == V::OnContour {
                // Secondary edge pointing to a contour point.
                a.edge[e] = E::PointsToContour;
            } else {
                // v0 is not on the input polygons; which side is v1 on?
                let (x, y) = vd.vertex_point(v1);
                let l0 = (line.a.x as f64, line.a.y as f64);
                let lv = ((line.b.x - line.a.x) as f64, (line.b.y - line.a.y) as f64);
                let side = (x - l0.0) * lv.1 - (y - l0.1) * lv.0;
                let vc = if side > 0.0 { V::Outside } else { V::Inside };
                a.vertex[v1] = vc;
                let ec = if vc == V::Outside {
                    E::PointsOutside
                } else {
                    E::PointsInside
                };
                a.edge[e] = ec;
                a.vertex[v0] = if on_contour { V::OnContour } else { vc };
                a.edge[twin] = if on_contour { E::PointsToContour } else { ec };
                let side_cat = if vc == V::Outside {
                    C::Outside
                } else {
                    C::Inside
                };
                a.annotate_cell(cell, if on_contour { C::Boundary } else { side_cat });
                let c2 = if on_contour && vd.cells[cell2].contains_segment() {
                    C::Boundary
                } else {
                    side_cat
                };
                a.annotate_cell(cell2, c2);
            }
        }
    }

    // One round of expansion next to the classified cells, for edges between two point sites.
    let mut queue = Vec::new();
    for e in 0..vd.edges.len() {
        if a.edge[e] != E::Unknown {
            continue;
        }
        let (cell, cell2) = (vd.edges[e].cell, vd.edges[vd.twin(e)].cell);
        let (cc, cc2) = (a.cell[cell], a.cell[cell2]);
        let mut cc_new = if cc == C::Unknown { cc2 } else { cc };
        if cc_new == C::Unknown
            && let Some(v0) = vd.edges[e].vertex0
        {
            match a.vertex[v0] {
                V::Outside => cc_new = C::Outside,
                V::Inside => cc_new = C::Inside,
                _ => {}
            }
        }
        if cc_new == C::Unknown || cc_new == C::Boundary {
            continue;
        }
        let vc = if cc_new == C::Outside {
            V::Outside
        } else {
            V::Inside
        };
        if let Some(v) = vd.edges[e].vertex0 {
            a.vertex[v] = vc;
        }
        if let Some(v) = vd.vertex1(e) {
            a.vertex[v] = vc;
        }
        let ec = if cc_new == C::Outside {
            E::PointsOutside
        } else {
            E::PointsInside
        };
        a.edge[e] = ec;
        a.edge[vd.twin(e)] = ec;
        if cc != cc_new {
            a.annotate_cell(cell, cc_new);
            queue.push(cell);
        }
        if cc2 != cc_new {
            a.annotate_cell(cell2, cc_new);
            queue.push(cell2);
        }
    }

    // Final seed fill over the remaining cells and edges.
    while let Some(cell) = queue.pop() {
        let cc = a.cell[cell];
        if cc != C::Outside && cc != C::Inside {
            continue;
        }
        let Some(first) = vd.cells[cell].incident_edge else {
            continue;
        };
        let ec_new = if cc == C::Outside {
            E::PointsOutside
        } else {
            E::PointsInside
        };
        let mut e = first;
        let mut guard = 0;
        loop {
            if a.edge[e] == E::Unknown {
                a.edge[e] = ec_new;
                a.edge[vd.twin(e)] = ec_new;
                let cell2 = vd.edges[vd.twin(e)].cell;
                if a.cell[cell2] != cc {
                    a.annotate_cell(cell2, cc);
                    queue.push(cell2);
                }
            }
            e = vd.edges[e].next;
            guard += 1;
            if e == first || guard > vd.edges.len() {
                break;
            }
        }
    }
    a
}

// --- MedialAxis. ---

#[derive(Clone, Copy, Debug, Default)]
struct EdgeData {
    active: bool,
    width_start: f64,
    width_end: f64,
}

struct MedialAxis<'a> {
    lines: Vec<Line>,
    vd: Diagram,
    min_width: f64,
    max_width: f64,
    edge_data: Vec<EdgeData>,
    ex: &'a ExPolygon,
}

fn to_lines(ex: &ExPolygon) -> Vec<Line> {
    let mut out = Vec::new();
    for poly in ex.polygons() {
        let n = poly.len();
        for i in 0..n {
            out.push(Line::new(poly[i], poly[(i + 1) % n]));
        }
    }
    out
}

fn round_point(p: (f64, f64)) -> Point {
    Point::new(p.0.round() as i64, p.1.round() as i64)
}

/// Distance from `p` to the segment `a`–`b`.
fn distance_to_segment(p: Point, a: Point, b: Point) -> f64 {
    let (px, py) = (p.x as f64, p.y as f64);
    let (ax, ay) = (a.x as f64, a.y as f64);
    let (dx, dy) = ((b.x - a.x) as f64, (b.y - a.y) as f64);
    let l2 = dx * dx + dy * dy;
    if l2 == 0.0 {
        return (px - ax).hypot(py - ay);
    }
    let t = (((px - ax) * dx + (py - ay) * dy) / l2).clamp(0.0, 1.0);
    (px - (ax + t * dx)).hypot(py - (ay + t * dy))
}

/// `Line::orientation`: direction angle in [0, 2π).
fn line_orientation(l: &Line) -> f64 {
    let a = ((l.b.y - l.a.y) as f64).atan2((l.b.x - l.a.x) as f64);
    if a < 0.0 {
        a + 2.0 * std::f64::consts::PI
    } else {
        a
    }
}

impl MedialAxis<'_> {
    /// Edge data shared by a half-edge and its twin, and whether `e` runs against it.
    fn edge_data(&mut self, e: usize) -> (&mut EdgeData, bool) {
        (&mut self.edge_data[e / 2], e & 1 != 0)
    }

    fn build(&mut self, polylines: &mut Vec<ThickPolyline>) {
        let (vd, valid) = voronoi::construct(&self.lines);
        self.vd = vd;
        if !valid {
            // Very thin slivers and holes (a few nm) can make the diagram invalid even after rotation;
            // close them and try again.
            let closed = closing_ex(std::slice::from_ref(self.ex), 2.0 * SCALED_EPSILON);
            self.lines = closed.iter().flat_map(to_lines).collect();
            self.vd = voronoi::construct(&self.lines).0;
        }
        let ann = annotate_inside_outside(&self.vd, &self.lines);

        // Valid skeleton edges.
        self.edge_data = vec![EdgeData::default(); self.vd.edges.len() / 2];
        for e in (0..self.vd.edges.len()).step_by(2) {
            if !self.vd.edges[e].primary || !self.vd.is_finite(e) {
                continue;
            }
            let v0 = self.vd.edges[e].vertex0.unwrap();
            let v1 = self.vd.vertex1(e).unwrap();
            if (ann.vertex[v0] == VertexCategory::Inside
                || ann.vertex[v1] == VertexCategory::Inside)
                && self.validate_edge(e)
            {
                self.edge_data(e).0.active = true;
            }
        }

        // Chain the valid edges into polylines.
        for seed in (0..self.vd.edges.len()).step_by(2) {
            let data = self.edge_data[seed / 2];
            if !data.active {
                continue;
            }
            self.edge_data[seed / 2].active = false;
            let v0 = self.vd.edges[seed].vertex0.unwrap();
            let v1 = self.vd.vertex1(seed).unwrap();
            let mut polyline = ThickPolyline {
                points: vec![
                    round_point(self.vd.vertex_point(v0)),
                    round_point(self.vd.vertex_point(v1)),
                ],
                width: vec![data.width_start, data.width_end],
                endpoints: (false, false),
            };
            // Forward.
            self.process_edge_neighbors(seed, &mut polyline);
            // Backward.
            let mut reverse = ThickPolyline::default();
            self.process_edge_neighbors(self.vd.twin(seed), &mut reverse);
            let mut points: Vec<Point> = reverse.points.iter().rev().copied().collect();
            points.extend_from_slice(&polyline.points);
            let mut width: Vec<f64> = reverse.width.iter().rev().copied().collect();
            width.extend_from_slice(&polyline.width);
            polyline.points = points;
            polyline.width = width;
            polyline.endpoints.0 = reverse.endpoints.1;
            // Loops don't get their ends extended.
            if polyline.first_point() == polyline.last_point() {
                polyline.endpoints = (false, false);
            }
            polylines.push(polyline);
        }
    }

    fn process_edge_neighbors(&mut self, mut edge: usize, polyline: &mut ThickPolyline) {
        loop {
            // rot_next works around the start point; we want neighbours at the end point.
            let twin = self.vd.twin(edge);
            let mut num_neighbors = 0;
            let mut first_neighbor = None;
            let mut n = self.vd.rot_next(twin);
            let mut guard = 0;
            while n != twin && guard <= self.vd.edges.len() {
                if self.edge_data[n / 2].active {
                    if num_neighbors == 0 {
                        first_neighbor = Some(n);
                    }
                    num_neighbors += 1;
                }
                n = self.vd.rot_next(n);
                guard += 1;
            }
            match (num_neighbors, first_neighbor) {
                (1, Some(next)) => {
                    let (data, reversed) = self.edge_data(next);
                    data.active = false;
                    let (ws, we) = (data.width_start, data.width_end);
                    let v1 = self.vd.vertex1(next).unwrap();
                    polyline.points.push(round_point(self.vd.vertex_point(v1)));
                    if reversed {
                        polyline.width.extend([we, ws]);
                    } else {
                        polyline.width.extend([ws, we]);
                    }
                    edge = next;
                }
                (0, _) => {
                    polyline.endpoints.1 = true;
                    return;
                }
                // T-shaped or star-shaped joint.
                _ => return,
            }
        }
    }

    fn validate_edge(&mut self, e: usize) -> bool {
        // Overflows and almost infinite edges.
        let v0 = self.vd.vertex_point(self.vd.edges[e].vertex0.unwrap());
        let v1 = self.vd.vertex_point(self.vd.vertex1(e).unwrap());
        const MAX: f64 = 1.0e12;
        if v0.0.abs() > MAX || v0.1.abs() > MAX || v1.0.abs() > MAX || v1.1.abs() > MAX {
            return false;
        }
        let (a, b) = (round_point(v0), round_point(v1));
        let cell_l = *self.vd.cell_of(e);
        let cell_r = *self.vd.cell_of(self.vd.twin(e));
        let segment_l = self.lines[cell_l.source_index];
        let segment_r = self.lines[cell_r.source_index];
        let endpoint = |cell: &voronoi::Cell, seg: &Line| {
            if cell.source == CellSource::SegmentStart {
                seg.a
            } else {
                seg.b
            }
        };

        // Width of the cross-section at both ends of this edge. Our edge runs counter-clockwise around
        // cell_l, so segment_r runs the same way as the edge and segment_l against it.
        let mut w0 = if cell_r.contains_segment() {
            distance_to_segment(a, segment_r.a, segment_r.b) * 2.0
        } else {
            a.distance_to(endpoint(&cell_r, &segment_r)) * 2.0
        };
        let mut w1 = if cell_l.contains_segment() {
            distance_to_segment(b, segment_l.a, segment_l.b) * 2.0
        } else {
            b.distance_to(endpoint(&cell_l, &segment_l)) * 2.0
        };

        if cell_l.contains_segment() && cell_r.contains_segment() {
            // Only keep edges between (almost) facing segments: a narrow, oriented area.
            let mut angle = (line_orientation(&segment_r) - line_orientation(&segment_l)).abs();
            if angle > std::f64::consts::PI {
                angle = 2.0 * std::f64::consts::PI - angle;
            }
            if std::f64::consts::PI - angle > std::f64::consts::PI / 8.0
                && (w0 < SCALED_EPSILON
                    || w1 < SCALED_EPSILON
                    || a.distance_to(b) >= self.min_width)
            {
                return false;
            }
        } else if w0 < SCALED_EPSILON || w1 < SCALED_EPSILON {
            return false;
        }

        if (w0 >= self.min_width || w1 >= self.min_width)
            && (w0 <= self.max_width || w1 <= self.max_width)
        {
            let (data, reversed) = self.edge_data(e);
            if reversed {
                std::mem::swap(&mut w0, &mut w1);
            }
            data.width_start = w0;
            data.width_end = w1;
            return true;
        }
        false
    }
}

fn oriented(poly: &Polygon, ccw: bool) -> Polygon {
    let mut p: Polygon = poly.clone();
    p.dedup();
    while p.len() > 1 && p.first() == p.last() {
        p.pop();
    }
    if (signed_area(&p) > 0.0) != ccw {
        p.reverse();
    }
    p
}

/// `line_alg::intersection`: the point where segments `l1` and `l2` cross, truncated to integers.
fn segment_intersection(l1: (Point, Point), l2: (Point, Point)) -> Option<Point> {
    let v1 = ((l1.1.x - l1.0.x) as f64, (l1.1.y - l1.0.y) as f64);
    let v2 = ((l2.1.x - l2.0.x) as f64, (l2.1.y - l2.0.y) as f64);
    let denom = v1.0 * v2.1 - v1.1 * v2.0;
    if denom.abs() < 1e-4 {
        return None;
    }
    let v12 = ((l1.0.x - l2.0.x) as f64, (l1.0.y - l2.0.y) as f64);
    let t1 = (v2.0 * v12.1 - v2.1 * v12.0) / denom;
    let t2 = (v1.0 * v12.1 - v1.1 * v12.0) / denom;
    if (0.0..=1.0).contains(&t1) && (0.0..=1.0).contains(&t2) {
        Some(Point::new(
            (l1.0.x as f64 + t1 * v1.0) as i64,
            (l1.0.y as f64 + t1 * v1.1) as i64,
        ))
    } else {
        None
    }
}

/// `Polygon::intersection`: the first edge of `poly` (starting with the closing edge) that `line` crosses.
fn polygon_intersection(poly: &Polygon, line: (Point, Point)) -> Option<Point> {
    if poly.len() < 2 {
        return None;
    }
    if let Some(p) = segment_intersection((poly[0], *poly.last().unwrap()), line) {
        return Some(p);
    }
    (1..poly.len()).find_map(|i| segment_intersection((poly[i - 1], poly[i]), line))
}

fn on_boundary(ex: &ExPolygon, p: Point, eps: f64) -> bool {
    ex.polygons().any(|poly| {
        let n = poly.len();
        (0..n).any(|i| distance_to_segment(p, poly[i], poly[(i + 1) % n]) < eps)
    })
}

/// Medial axis of `ex`: centrelines of the parts between `min_width` and `max_width` wide, with their
/// local width, extended to the boundary at free ends. PrusaSlicer's `ExPolygon::medial_axis`.
pub fn medial_axis(ex: &ExPolygon, min_width: f64, max_width: f64) -> Vec<ThickPolyline> {
    let ex = ExPolygon {
        contour: oriented(&ex.contour, true),
        holes: ex.holes.iter().map(|h| oriented(h, false)).collect(),
    };
    if ex.contour.len() < 3 {
        return Vec::new();
    }
    let mut ma = MedialAxis {
        lines: to_lines(&ex),
        vd: Diagram::default(),
        min_width,
        max_width,
        edge_data: Vec::new(),
        ex: &ex,
    };
    let mut pp = Vec::new();
    ma.build(&mut pp);

    let max_w = pp
        .iter()
        .flat_map(|p: &ThickPolyline| p.width.iter().copied())
        .fold(0.0f64, f64::max);

    // Extend free ends to the boundary, then drop polylines that are too short.
    let mut removed = false;
    let mut i = 0;
    while i < pp.len() {
        let polyline = &mut pp[i];
        let mut new_front = polyline.first_point();
        let mut new_back = polyline.last_point();
        if polyline.endpoints.0 && !on_boundary(&ex, new_front, SCALED_EPSILON) {
            let p1 = polyline.points[0];
            let mut p2 = (polyline.points[1].x as f64, polyline.points[1].y as f64);
            let p1f = (p1.x as f64, p1.y as f64);
            // Keep the extension from reaching the other side.
            if polyline.points.len() == 2 {
                p2 = ((p1f.0 + p2.0) * 0.5, (p1f.1 + p2.1) * 0.5);
            }
            let (dx, dy) = (p2.0 - p1f.0, p2.1 - p1f.1);
            let len = dx.hypot(dy);
            let ext = (p1f.0 - dx / len * max_width, p1f.1 - dy / len * max_width);
            let seg = (
                Point::new(ext.0 as i64, ext.1 as i64),
                Point::new(p2.0 as i64, p2.1 as i64),
            );
            if let Some(p) = polygon_intersection(&ex.contour, seg) {
                new_front = p;
            }
        }
        if polyline.endpoints.1 && !on_boundary(&ex, new_back, SCALED_EPSILON) {
            let n = polyline.points.len();
            let mut p1 = (
                polyline.points[n - 2].x as f64,
                polyline.points[n - 2].y as f64,
            );
            let p2 = (
                polyline.points[n - 1].x as f64,
                polyline.points[n - 1].y as f64,
            );
            if n == 2 {
                p1 = ((p1.0 + p2.0) * 0.5, (p1.1 + p2.1) * 0.5);
            }
            let (dx, dy) = (p2.0 - p1.0, p2.1 - p1.1);
            let len = dx.hypot(dy);
            let ext = (p2.0 + dx / len * max_width, p2.1 + dy / len * max_width);
            let seg = (
                Point::new(p1.0 as i64, p1.1 as i64),
                Point::new(ext.0 as i64, ext.1 as i64),
            );
            if let Some(p) = polygon_intersection(&ex.contour, seg) {
                new_back = p;
            }
        }
        polyline.points[0] = new_front;
        let last = polyline.points.len() - 1;
        polyline.points[last] = new_back;

        // Too short (only now known: the extension depends on the local thickness).
        if (polyline.endpoints.0 || polyline.endpoints.1) && polyline.length() < max_w * 2.0 {
            pp.remove(i);
            removed = true;
            continue;
        }
        i += 1;
    }

    // Reconnect polylines that met at a junction whose third branch was just removed.
    if removed {
        let mut i = 0;
        while i < pp.len() {
            if pp[i].endpoints.0 && pp[i].endpoints.1 {
                i += 1;
                continue;
            }
            let mut j = i + 1;
            while j < pp.len() {
                let (head, tail) = pp.split_at_mut(j);
                let polyline = &mut head[i];
                let other = &mut tail[0];
                if polyline.last_point() == other.last_point() {
                    other.reverse();
                } else if polyline.first_point() == other.last_point() {
                    polyline.reverse();
                    other.reverse();
                } else if polyline.first_point() == other.first_point() {
                    polyline.reverse();
                } else if polyline.last_point() != other.first_point() {
                    j += 1;
                    continue;
                }
                polyline.points.extend_from_slice(&other.points[1..]);
                polyline.width.extend_from_slice(&other.width);
                polyline.endpoints.1 = other.endpoints.1;
                pp.remove(j);
                j = i + 1;
            }
            i += 1;
        }
    }
    pp
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::{SCALE, scale};

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> ExPolygon {
        ExPolygon {
            contour: vec![
                Point::from_mm(x0, y0),
                Point::from_mm(x1, y0),
                Point::from_mm(x1, y1),
                Point::from_mm(x0, y1),
            ],
            holes: Vec::new(),
        }
    }

    fn widths_mm(p: &ThickPolyline) -> impl Iterator<Item = f64> + '_ {
        p.width.iter().map(|w| w / SCALE)
    }

    #[test]
    fn thin_rectangle_centreline() {
        // 10 × 0.5 mm: the centreline runs end to end along y = 0.25 with width 0.5.
        let ex = rect(0.0, 0.0, 10.0, 0.5);
        let pp = medial_axis(&ex, scale(0.1) as f64, scale(1.0) as f64);
        assert_eq!(pp.len(), 1, "{pp:?}");
        let p = &pp[0];
        let len = p.length() / SCALE;
        // The skeleton runs between the corner bisectors (0.25 mm in from each end) and its free ends
        // are extended to the boundary.
        assert!((len - 10.0).abs() < 0.01, "length {len}");
        for pt in &p.points {
            assert!((pt.y as f64 / SCALE - 0.25).abs() < 1e-3, "{pt:?}");
        }
        // Width 0.5 mm along the straight part; at the extended ends it is the width of the last edge.
        let ws: Vec<f64> = widths_mm(p).collect();
        assert!(ws.iter().all(|w| *w <= 0.5 + 1e-6 && *w > 0.0), "{ws:?}");
        assert!(ws.iter().filter(|w| (*w - 0.5).abs() < 1e-6).count() >= 2);
        assert_eq!(p.width.len(), 2 * (p.points.len() - 1));
    }

    #[test]
    fn too_wide_rectangle_has_no_axis() {
        let ex = rect(0.0, 0.0, 10.0, 3.0);
        assert!(medial_axis(&ex, scale(0.1) as f64, scale(1.0) as f64).is_empty());
    }

    #[test]
    fn wedge_widths_taper() {
        // A long thin wedge: 1 mm wide at x = 0, 0.2 mm at x = 20.
        let ex = ExPolygon {
            contour: vec![
                Point::from_mm(0.0, 0.0),
                Point::from_mm(20.0, 0.4),
                Point::from_mm(20.0, 0.6),
                Point::from_mm(0.0, 1.0),
            ],
            holes: Vec::new(),
        };
        let pp = medial_axis(&ex, scale(0.05) as f64, scale(2.0) as f64);
        assert_eq!(pp.len(), 1, "{pp:?}");
        let mut p = pp[0].clone();
        if p.first_point().x > p.last_point().x {
            p.reverse();
        }
        // Widths shrink monotonically from the wide to the narrow end, and match the wedge.
        let ws: Vec<f64> = widths_mm(&p).collect();
        assert!(ws.windows(2).all(|w| w[1] <= w[0] + 1e-6), "{ws:?}");
        assert!(ws[0] > 0.8 && ws[0] <= 1.0 + 1e-6, "{ws:?}");
        assert!(*ws.last().unwrap() < 0.3, "{ws:?}");
        for (i, line) in p.thicklines().iter().enumerate() {
            // The wedge is 1 − 0.04·x mm wide at x (measured perpendicular to the axis, ~equal here).
            let expect = |pt: Point| 1.0 - 0.04 * pt.x as f64 / SCALE;
            if i > 0 {
                assert!(
                    (line.a_width / SCALE - expect(line.a)).abs() < 0.01,
                    "{line:?}"
                );
            }
        }
    }

    #[test]
    fn narrow_ring_gives_a_closed_loop() {
        // A 10 mm square with a 9.4 mm square hole: a 0.3 mm wide ring.
        let mut ex = rect(0.0, 0.0, 10.0, 10.0);
        let mut hole = rect(0.3, 0.3, 9.7, 9.7).contour;
        hole.reverse();
        ex.holes.push(hole);
        let pp = medial_axis(&ex, scale(0.05) as f64, scale(1.0) as f64);
        assert_eq!(pp.len(), 1, "{pp:?}");
        let p = &pp[0];
        assert_eq!(p.first_point(), p.last_point());
        assert_eq!(p.endpoints, (false, false));
        // Centreline of the ring: 0.15 mm in from the outside along the sides; around each inner corner it
        // follows two parabolic arcs (between the corner point and the outer sides), approximated by their
        // chords from (0.3, 0.15) to the vertex on the diagonal at x = y = d.
        let d = (1.2 - 0.72f64.sqrt()) / 2.0;
        let chord = ((0.3 - d).powi(2) + (d - 0.15).powi(2)).sqrt();
        let expect = 4.0 * 9.4 + 8.0 * chord;
        assert!(
            (p.length() / SCALE - expect).abs() < 0.01,
            "{} vs {expect}",
            p.length() / SCALE
        );
        // 0.3 mm along the sides, up to 2·√2·d at the corner vertices.
        let corner = 2.0 * std::f64::consts::SQRT_2 * (0.3 - d);
        assert!(widths_mm(p).all(|w| w > 0.3 - 1e-3 && w < corner + 1e-3));
        assert!(widths_mm(p).filter(|w| (w - 0.3).abs() < 1e-3).count() >= 8);
    }
}
