// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: prusaslicer@b028299c77 src/libslic3r/Arachne/SkeletalTrapezoidationGraph.cpp :: SkeletalTrapezoidationGraph, STHalfEdge, STHalfEdgeNode
// Source: prusaslicer@b028299c77 src/libslic3r/Arachne/SkeletalTrapezoidation.cpp :: constructFromPolygons, transferEdge, discretize, separatePointyQuadEndNodes
// Source: prusaslicer@b028299c77 src/libslic3r/Geometry/VoronoiUtils.cpp :: compute_point_cell_range, discretize_parabola
// Changes: Stable index arenas with tombstones replace pointer lists; bounded graph walks reject malformed diagrams.
// Copyright (c) 2020-2022 Ultimaker B.V.
// CuraEngine is released under the terms of the AGPLv3 or higher.

use super::Junction;
use super::beading::Beading;
use crate::geometry::voronoi::{self, CellSource, Diagram, Line};
use crate::geometry::{Point, Polygon};
use std::collections::HashSet;

pub(crate) const NONE: usize = usize::MAX;
#[derive(Clone, Debug)]
pub(crate) struct Propagation {
    pub beading: Beading,
    pub bottom: f64,
    pub top: f64,
    pub upward_only: bool,
}
impl From<Beading> for Propagation {
    fn from(beading: Beading) -> Self {
        Self {
            beading,
            bottom: 0.,
            top: 0.,
            upward_only: false,
        }
    }
}
#[derive(Clone, Debug)]
pub(crate) struct Node {
    pub p: Point,
    pub radius: f64,
    pub count: i32,
    pub ratio: f64,
    pub incident: usize,
    pub beading: Option<Propagation>,
    pub alive: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EdgeType {
    Normal,
    Extra,
    Transition,
}
#[derive(Clone, Copy, Debug)]
pub(crate) struct Mid {
    pub pos: f64,
    pub count: usize,
    pub radius: f64,
}
#[derive(Clone, Copy, Debug)]
pub(crate) struct End {
    pub pos: f64,
    pub count: usize,
    pub lower: bool,
}
#[derive(Clone, Debug)]
pub(crate) struct Edge {
    pub from: usize,
    pub to: usize,
    pub twin: usize,
    pub prev: usize,
    pub next: usize,
    pub kind: EdgeType,
    pub central: bool,
    pub mids: Vec<Mid>,
    pub ends: Vec<End>,
    pub junctions: Vec<Junction>,
    pub alive: bool,
}
#[derive(Clone, Default, Debug)]
pub(crate) struct Graph {
    initial_edges: usize,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
}

pub(crate) fn lerp(a: Point, b: Point, t: f64) -> Point {
    Point::new(
        a.x + ((b.x - a.x) as f64 * t) as i64,
        a.y + ((b.y - a.y) as f64 * t) as i64,
    )
}
pub(crate) fn project(p: Point, a: Point, b: Point) -> Point {
    let dx = (b.x - a.x) as f64;
    let dy = (b.y - a.y) as f64;
    let d = dx * dx + dy * dy;
    if d == 0. {
        a
    } else {
        lerp(
            a,
            b,
            (((p.x - a.x) as f64 * dx + (p.y - a.y) as f64 * dy) / d).clamp(0., 1.),
        )
    }
}
fn vertex(vd: &Diagram, v: usize) -> Point {
    let v = &vd.vertices[v];
    Point::new(v.x.round() as i64, v.y.round() as i64)
}
fn cross(a: Point, b: Point, c: Point) -> i128 {
    (b.x - a.x) as i128 * (c.y - a.y) as i128 - (b.y - a.y) as i128 * (c.x - a.x) as i128
}

impl Graph {
    // Upstream builds with emplace_front, then appends transition ribs with emplace_back.
    // Preserve that processing order while keeping stable arena indices.
    pub fn edge_ids(&self) -> Vec<usize> {
        (0..self.initial_edges)
            .rev()
            .chain(self.initial_edges..self.edges.len())
            .collect()
    }
    pub fn node(&mut self, p: Point) -> usize {
        let id = self.nodes.len();
        self.nodes.push(Node {
            p,
            radius: -1.,
            count: -1,
            ratio: 0.,
            incident: NONE,
            beading: None,
            alive: true,
        });
        id
    }
    fn edge(&mut self, from: usize, to: usize, kind: EdgeType) -> usize {
        let id = self.edges.len();
        self.edges.push(Edge {
            from,
            to,
            twin: NONE,
            prev: NONE,
            next: NONE,
            kind,
            central: false,
            mids: vec![],
            ends: vec![],
            junctions: vec![],
            alive: true,
        });
        self.nodes[from].incident = id;
        id
    }
    fn pair(&mut self, a: usize, b: usize) {
        self.edges[a].twin = b;
        self.edges[b].twin = a;
    }
    fn link(&mut self, a: usize, b: usize) {
        if a != NONE {
            self.edges[a].next = b;
        }
        if b != NONE {
            self.edges[b].prev = a;
        }
    }
    pub fn len(&self, e: usize) -> f64 {
        let e = &self.edges[e];
        self.nodes[e.from].p.distance_to(self.nodes[e.to].p).trunc()
    }
    pub fn outgoing(&self, node: usize) -> Vec<usize> {
        let start = self.nodes[node].incident;
        if start == NONE {
            return vec![];
        }
        let mut out = vec![];
        let mut e = start;
        while e != NONE && out.len() <= self.edges.len() {
            if !self.edges[e].alive || out.contains(&e) {
                break;
            }
            out.push(e);
            let t = self.edges[e].twin;
            if t == NONE {
                break;
            }
            e = self.edges[t].next;
        }
        out
    }
    pub fn beyond(&self, e: usize) -> Vec<usize> {
        let stop = self.edges[e].twin;
        let mut x = self.edges[e].next;
        let mut out = vec![];
        while x != NONE && x != stop && out.len() <= self.edges.len() {
            if out.contains(&x) {
                break;
            }
            out.push(x);
            let twin = self.edges[x].twin;
            if twin == NONE {
                break;
            }
            x = self.edges[twin].next;
        }
        out
    }
    pub fn dist_up(&self, e: usize) -> Option<f64> {
        fn walk(g: &Graph, e: usize, visited: &mut HashSet<usize>) -> Option<f64> {
            let edge = &g.edges[e];
            let a = g.nodes[edge.from].radius;
            let b = g.nodes[edge.to].radius;
            if b > a {
                return Some(0.);
            }
            if b < a || !visited.insert(e) {
                return None;
            }
            let result = g
                .beyond(e)
                .into_iter()
                .filter_map(|x| walk(g, x, visited))
                .min_by(f64::total_cmp)
                .map(|d| d + g.len(e));
            visited.remove(&e);
            result
        }
        walk(self, e, &mut HashSet::new())
    }
    pub fn upward(&self, e: usize) -> bool {
        let edge = &self.edges[e];
        let a = &self.nodes[edge.from];
        let b = &self.nodes[edge.to];
        if a.radius != b.radius {
            return b.radius > a.radius;
        }
        match (self.dist_up(e), self.dist_up(edge.twin)) {
            (Some(a), Some(b)) => a < b,
            (Some(_), None) => true,
            (None, Some(_)) => false,
            _ => (b.p.x, b.p.y) < (a.p.x, a.p.y),
        }
    }
    pub fn local_max(&self, n: usize, strict: bool) -> bool {
        self.nodes[n].radius > 0.
            && self.outgoing(n).iter().all(|&e| {
                let edge = &self.edges[e];
                self.edges[edge.twin].next != NONE
                    && if strict {
                        self.nodes[edge.to].radius <= self.nodes[n].radius
                    } else {
                        self.dist_up(e).is_none()
                    }
            })
    }
    pub fn central_node(&self, n: usize) -> bool {
        self.outgoing(n).iter().any(|&e| self.edges[e].central)
    }
    pub fn multi(&self, n: usize) -> bool {
        self.outgoing(n)
            .iter()
            .filter(|&&e| self.edges[e].central)
            .count()
            > 2
    }
    pub fn end_central(&self, e: usize) -> bool {
        self.edges[e].central && self.beyond(e).iter().all(|&e| !self.edges[e].central)
    }
    fn rib(&mut self, prev: usize, a: Point, b: Point) -> usize {
        let peak = self.edges[prev].to;
        let p = project(self.nodes[peak].p, a, b);
        self.nodes[peak].radius = self.nodes[peak].p.distance_to(p).trunc();
        let boundary = self.node(p);
        self.nodes[boundary].radius = 0.;
        let forth = self.edge(peak, boundary, EdgeType::Extra);
        let back = self.edge(boundary, peak, EdgeType::Extra);
        self.pair(forth, back);
        self.link(prev, forth);
        back
    }
    pub fn source(&self, e: usize) -> (Point, Point) {
        let mut a = e;
        let mut b = e;
        while self.edges[a].prev != NONE {
            a = self.edges[a].prev;
        }
        while self.edges[b].next != NONE {
            b = self.edges[b].next;
        }
        (
            self.nodes[self.edges[a].from].p,
            self.nodes[self.edges[b].to].p,
        )
    }
    fn insert_rib(&mut self, e: usize, n: usize) -> (usize, usize) {
        let (a, b) = self.source(e);
        let p = project(self.nodes[n].p, a, b);
        self.nodes[n].radius = self.nodes[n].p.distance_to(p).trunc();
        self.nodes[n].ratio = 0.;
        let boundary = self.node(p);
        self.nodes[boundary].radius = 0.;
        let old = self.edges[e].clone();
        let second = self.edge(n, old.to, EdgeType::Normal);
        let out = self.edge(n, boundary, EdgeType::Transition);
        let inward = self.edge(boundary, n, EdgeType::Transition);
        self.link(e, out);
        self.link(inward, second);
        self.link(second, old.next);
        self.edges[e].to = n;
        self.edges[e].central = true;
        self.edges[second].central = true;
        self.pair(out, inward);
        (e, second)
    }
    pub fn insert_node(&mut self, e: usize, p: Point, count: i32) -> usize {
        let twin = self.edges[e].twin;
        let n = self.node(p);
        let (a, b) = self.insert_rib(e, n);
        let (c, d) = self.insert_rib(twin, n);
        self.pair(a, d);
        self.pair(b, c);
        self.nodes[n].count = count;
        b
    }
    pub fn construct(polys: &[Polygon], angle: f64) -> Option<Self> {
        let mut lines = vec![];
        let mut corners = vec![];
        for poly in polys {
            for i in 0..poly.len() {
                lines.push(Line::new(poly[i], poly[(i + 1) % poly.len()]));
                corners.push((
                    poly[(i + poly.len() - 1) % poly.len()],
                    poly[(i + 2) % poly.len()],
                ));
            }
        }
        let timer = crate::timing::Span::new(crate::timing::Stage::GraphVoronoi);
        let (vd, _valid) = voronoi::construct(&lines);
        drop(timer);
        let timer = crate::timing::Span::new(crate::timing::Stage::GraphTransfer);
        let mut g = Self::default();
        let mut node_map = vec![NONE; vd.vertices.len()];
        let mut edge_map = vec![NONE; vd.edges.len()];
        for cell in &vd.cells {
            let Some(first) = cell.incident_edge else {
                continue;
            };
            let line = lines[cell.source_index];
            let (a, b, begin, end) = if cell.contains_segment() {
                let Some((begin, end)) = voronoi::segment_cell_range(&vd, cell, &lines) else {
                    continue;
                };
                (line.b, line.a, begin, end)
            } else {
                if !vd.is_finite(first) {
                    continue;
                }
                let p = voronoi::source_point(cell, &lines);
                let v0 = vertex(&vd, vd.edges[first].vertex0?);
                let v1 = vertex(&vd, vd.vertex1(first)?);
                let q = if v0 == p { v1 } else { v0 };
                let (prev, next) = if cell.source == CellSource::SegmentStart {
                    (corners[cell.source_index].0, line.b)
                } else {
                    (line.a, corners[cell.source_index].1)
                };
                let l = cross(prev, p, q) >= 0;
                let r = cross(p, next, q) >= 0;
                let inside = if cross(prev, p, next) > 0 {
                    l && r
                } else {
                    l || r
                };
                if !inside {
                    continue;
                }
                let mut e = first;
                let mut range = None;
                loop {
                    if !vd.is_finite(e) {
                        range = None;
                        break;
                    }
                    if vertex(&vd, vd.vertex1(e)?) == p {
                        range = Some((vd.edges[e].next, e));
                    }
                    e = vd.edges[e].next;
                    if e == first {
                        break;
                    }
                }
                let Some((begin, end)) = range else {
                    continue;
                };
                (p, p, begin, end)
            };
            let mut prev = NONE;
            let mut e = begin;
            let mut steps = 0;
            loop {
                if !vd.is_finite(e) {
                    return None;
                }
                let start = if e == begin {
                    a
                } else {
                    vertex(&vd, vd.edges[e].vertex0?)
                };
                let finish = if e == end {
                    b
                } else {
                    vertex(&vd, vd.vertex1(e)?)
                };
                if edge_map[e ^ 1] != NONE {
                    let mut twin = edge_map[e ^ 1];
                    let dest = node_map[vd.vertex1(e)?];
                    loop {
                        let from = g.edges[twin].to;
                        let to = g.edges[twin].from;
                        let new = g.edge(from, to, EdgeType::Normal);
                        g.pair(new, twin);
                        g.link(prev, new);
                        prev = new;
                        if to == dest {
                            break;
                        }
                        let back = g.edges[twin].prev;
                        if back == NONE || g.edges[back].twin == NONE {
                            return None;
                        }
                        twin = g.edges[g.edges[back].twin].prev;
                        if twin == NONE {
                            return None;
                        }
                        prev = g.rib(prev, a, b);
                    }
                } else {
                    let points = discretize(&vd, e, &lines, angle);
                    let v0 = vd.edges[e].vertex0?;
                    let v1 = vd.vertex1(e)?;
                    let mut from = if prev != NONE {
                        g.edges[prev].to
                    } else if node_map[v0] != NONE {
                        node_map[v0]
                    } else {
                        let n = g.node(start);
                        node_map[v0] = n;
                        n
                    };
                    for (i, &p) in points.iter().enumerate().skip(1) {
                        let to = if i + 1 < points.len() {
                            g.node(p)
                        } else if node_map[v1] != NONE {
                            node_map[v1]
                        } else {
                            let n = g.node(finish);
                            node_map[v1] = n;
                            n
                        };
                        let new = g.edge(from, to, EdgeType::Normal);
                        g.link(prev, new);
                        prev = new;
                        from = to;
                        if i + 1 < points.len() {
                            prev = g.rib(prev, a, b);
                        }
                    }
                    edge_map[e] = prev;
                }
                if e == begin {
                    g.nodes[node_map[vd.edges[e].vertex0?]].radius = 0.;
                }
                if e == end {
                    g.nodes[g.edges[prev].to].radius = 0.;
                    break;
                }
                prev = g.rib(prev, a, b);
                e = vd.edges[e].next;
                steps += 1;
                if steps > vd.edges.len() {
                    return None;
                }
            }
        }
        if g.edges.iter().any(|e| e.twin == NONE) {
            return None;
        }
        let mut visited = HashSet::new();
        for e in 0..g.edges.len() {
            if g.edges[e].prev != NONE {
                continue;
            }
            let n = g.edges[e].from;
            if !visited.insert(n) {
                let new = g.nodes.len();
                g.nodes.push(g.nodes[n].clone());
                g.nodes[new].incident = e;
                g.edges[e].from = new;
                let t = g.edges[e].twin;
                g.edges[t].to = new;
            }
        }
        g.initial_edges = g.edges.len();
        drop(timer);
        let _timer = crate::timing::Span::new(crate::timing::Stage::GraphCollapse);
        g.collapse();
        for e in 0..g.edges.len() {
            if g.edges[e].alive && g.edges[e].prev == NONE {
                g.nodes[g.edges[e].from].incident = e;
            }
        }
        Some(g)
    }
    fn collapse(&mut self) {
        for start in self.edge_ids() {
            if !self.edges[start].alive || self.edges[start].prev != NONE {
                continue;
            }
            let mut end = start;
            while self.edges[end].next != NONE {
                end = self.edges[end].next;
            }
            let mid = self.edges[start].next;
            if mid != end && mid != NONE && self.len(mid) < 5. {
                let old = self.edges[mid].to;
                let new = self.edges[mid].from;
                let twin = self.edges[mid].twin;
                // Retarget the whole vertex fan before unlinking the pair.
                for e in &mut self.edges {
                    if !e.alive {
                        continue;
                    }
                    if e.from == old {
                        e.from = new;
                    }
                    if e.to == old {
                        e.to = new;
                    }
                }
                self.nodes[old].alive = false;
                if self.nodes[new].incident == mid {
                    self.nodes[new].incident = if self.edges[twin].next != NONE {
                        self.edges[twin].next
                    } else {
                        self.edges[self.edges[mid].prev].twin
                    };
                }
                self.link(self.edges[mid].prev, self.edges[mid].next);
                self.link(self.edges[twin].prev, self.edges[twin].next);
                self.edges[mid].alive = false;
                self.edges[twin].alive = false;
            }
            let a = self.edges[start].from;
            let b = self.edges[end].to;
            let c = self.edges[start].to;
            let d = self.edges[end].from;
            if self.nodes[a].p.distance_to(self.nodes[b].p) < 5.
                && self.nodes[c].p.distance_to(self.nodes[d].p) < 5.
            {
                let st = self.edges[start].twin;
                let et = self.edges[end].twin;
                self.edges[st].to = b;
                self.nodes[b].incident = et;
                if self.nodes[d].incident == end {
                    self.nodes[d].incident = if self.edges[et].next != NONE {
                        self.edges[et].next
                    } else {
                        self.edges[self.edges[end].prev].twin
                    };
                }
                self.nodes[a].alive = false;
                self.pair(st, et);
                self.edges[start].alive = false;
                self.edges[end].alive = false;
            }
        }
    }
}

fn discretize(vd: &Diagram, e: usize, lines: &[Line], angle: f64) -> Vec<Point> {
    let a = vertex(vd, vd.edges[e].vertex0.unwrap());
    let b = vertex(vd, vd.vertex1(e).unwrap());
    let left = vd.cell_of(e);
    let right = vd.cell_of(e ^ 1);
    if !vd.edges[e].primary || (left.contains_segment() && right.contains_segment()) {
        return vec![a, b];
    }
    if left.contains_point() != right.contains_point() {
        let (point, segment) = if left.contains_point() {
            (left, right)
        } else {
            (right, left)
        };
        let focus = voronoi::source_point(point, lines);
        let line = lines[segment.source_index];
        let dx = (line.b.x - line.a.x) as f64;
        let dy = (line.b.y - line.a.y) as f64;
        let length = dx.hypot(dy);
        let project_x = |p: Point| {
            (((p.x - line.a.x) as f64 * dx + (p.y - line.a.y) as f64 * dy) / length).trunc()
        };
        let px = project_x(focus);
        let foot = lerp(line.a, line.b, px / length);
        let d = focus.distance_to(foot).trunc();
        if d < 1. {
            return vec![a, b];
        }
        let sx = project_x(a) - px;
        let ex = project_x(b) - px;
        let direction = if ex > sx { 1. } else { -1. };
        let steps = ((ex - sx).abs() / 800_000.).round() as usize;
        let mut xs: Vec<f64> = (1..steps)
            .map(|i| sx + (ex - sx) * i as f64 / steps as f64)
            .collect();
        let bound = (angle / 2.).atan() * d;
        for x in [-bound, 0., bound] {
            if (x - sx) * direction > 0. && (ex - x) * direction > 0. {
                xs.push(x);
            }
        }
        xs.sort_by(|x, y| (x * direction).total_cmp(&(y * direction)));
        let cos = -(foot.y - focus.y) as f64 / d;
        let sin = (foot.x - focus.x) as f64 / d;
        let mut out = vec![a];
        for x in xs {
            let y = (x * x / (2. * d)).trunc() + (d / 2.).trunc();
            out.push(Point::new(
                foot.x + (cos * x - sin * y).round() as i64,
                foot.y + (sin * x + cos * y).round() as i64,
            ));
        }
        out.push(b);
        out
    } else {
        let lp = voronoi::source_point(left, lines);
        let rp = voronoi::source_point(right, lines);
        let middle = lerp(lp, rp, 0.5);
        let d = lp.distance_to(rp);
        if d == 0. {
            return vec![a, b];
        }
        let axis = (-(rp.y - lp.y) as f64 / d, (rp.x - lp.x) as f64 / d);
        let project_x =
            |p: Point| (p.x - middle.x) as f64 * axis.0 + (p.y - middle.y) as f64 * axis.1;
        let sx = project_x(a);
        let ex = project_x(b);
        let direction = if ex > sx { 1. } else { -1. };
        let steps = ((a.distance_to(b) + 400_000.) / 800_000.) as usize;
        let steps = steps + steps % 2;
        let mut xs: Vec<f64> = (1..steps)
            .map(|i| sx + (ex - sx) * i as f64 / steps as f64)
            .collect();
        let bound = d * 0.5 / ((std::f64::consts::PI - angle) * 0.5).tan();
        for x in [-bound, bound] {
            if (x - sx) * direction > 0. && (ex - x) * direction > 0. {
                xs.push(x);
            }
        }
        xs.sort_by(|x, y| (x * direction).total_cmp(&(y * direction)));
        let mut out = vec![a];
        for x in xs {
            out.push(Point::new(
                middle.x + (axis.0 * x) as i64,
                middle.y + (axis.1 * x) as i64,
            ));
        }
        out.push(b);
        out
    }
}
