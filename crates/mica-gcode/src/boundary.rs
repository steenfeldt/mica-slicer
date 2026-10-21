// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Layer-local line queries in millimetres. The tree owns only indices, never toolpath copies.
use mica_core::geometry::{ExPolygon, Point, SCALE};

pub(crate) type XY = [f64; 2];
pub(crate) fn xy(p: Point) -> XY {
    [p.x as f64 / SCALE, p.y as f64 / SCALE]
}

pub(crate) fn point(p: XY) -> Point {
    Point::from_mm(p[0], p[1])
}
pub(crate) fn lerp(a: XY, b: XY, t: f64) -> XY {
    [a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1])]
}
pub(crate) fn distance(a: XY, b: XY) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}
pub(crate) fn cross(a: XY, b: XY) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}
pub(crate) fn sub(a: XY, b: XY) -> XY {
    [a[0] - b[0], a[1] - b[1]]
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Line {
    pub a: XY,
    pub b: XY,
}
impl Line {
    pub fn projection(self, p: XY) -> (f64, XY) {
        let v = sub(self.b, self.a);
        let w = sub(p, self.a);
        let norm = v[0] * v[0] + v[1] * v[1];
        let t = if norm > 0. {
            ((w[0] * v[0] + w[1] * v[1]) / norm).clamp(0., 1.)
        } else {
            0.
        };
        (t, lerp(self.a, self.b, t))
    }
    pub fn distance(self, p: XY) -> f64 {
        distance(p, self.projection(p).1)
    }
    pub fn intersection(self, other: Self) -> Option<f64> {
        let v = sub(self.b, self.a);
        let w = sub(other.b, other.a);
        let d = cross(v, w);
        if d.abs() < 1e-12 {
            return None;
        }
        let u = sub(other.a, self.a);
        let t = cross(u, w) / d;
        let s = cross(u, v) / d;
        ((-1e-9..=1. + 1e-9).contains(&t) && (-1e-9..=1. + 1e-9).contains(&s))
            .then_some(t.clamp(0., 1.))
    }
}

#[derive(Debug)]
struct Node {
    min: XY,
    max: XY,
    range: std::ops::Range<usize>,
    children: Option<(usize, usize)>,
}
#[derive(Default, Debug)]
pub(crate) struct Lines {
    pub lines: Vec<Line>,
    order: Vec<usize>,
    nodes: Vec<Node>,
}
impl Lines {
    pub fn new(lines: Vec<Line>) -> Self {
        let mut tree = Self {
            order: (0..lines.len()).collect(),
            lines,
            nodes: Vec::new(),
        };
        if !tree.lines.is_empty() {
            tree.build(0..tree.lines.len());
        }
        tree
    }
    pub fn outlines(polys: &[ExPolygon]) -> Self {
        Self::new(
            polys
                .iter()
                .flat_map(|p| p.polygons())
                .flat_map(|p| {
                    p.iter()
                        .zip(p.iter().cycle().skip(1))
                        .take(p.len())
                        .map(|(&a, &b)| Line { a: xy(a), b: xy(b) })
                })
                .collect(),
        )
    }
    fn build(&mut self, range: std::ops::Range<usize>) -> usize {
        let mut min = [f64::INFINITY; 2];
        let mut max = [f64::NEG_INFINITY; 2];
        for &i in &self.order[range.clone()] {
            for p in [self.lines[i].a, self.lines[i].b] {
                for k in 0..2 {
                    min[k] = min[k].min(p[k]);
                    max[k] = max[k].max(p[k]);
                }
            }
        }
        let idx = self.nodes.len();
        self.nodes.push(Node {
            min,
            max,
            range: range.clone(),
            children: None,
        });
        if range.len() > 8 {
            let k = usize::from(max[1] - min[1] > max[0] - min[0]);
            let mid = range.start + range.len() / 2;
            self.order[range.clone()].select_nth_unstable_by(range.len() / 2, |&a, &b| {
                (self.lines[a].a[k] + self.lines[a].b[k])
                    .total_cmp(&(self.lines[b].a[k] + self.lines[b].b[k]))
                    .then(a.cmp(&b))
            });
            let l = self.build(range.start..mid);
            let r = self.build(mid..range.end);
            self.nodes[idx].children = Some((l, r));
        }
        idx
    }
    fn box_distance(&self, idx: usize, p: XY) -> f64 {
        let n = &self.nodes[idx];
        distance(
            p,
            [
                p[0].clamp(n.min[0], n.max[0]),
                p[1].clamp(n.min[1], n.max[1]),
            ],
        )
    }
    pub fn nearest(&self, p: XY) -> Option<(usize, f64)> {
        if self.nodes.is_empty() {
            return None;
        }
        let mut best = (0, f64::INFINITY);
        self.search(0, p, &mut best);
        Some(best)
    }
    fn search(&self, idx: usize, p: XY, best: &mut (usize, f64)) {
        if self.box_distance(idx, p) > best.1 {
            return;
        }
        let n = &self.nodes[idx];
        if let Some((l, r)) = n.children {
            let (first, second) = if self.box_distance(l, p) <= self.box_distance(r, p) {
                (l, r)
            } else {
                (r, l)
            };
            self.search(first, p, best);
            self.search(second, p, best);
        } else {
            for &i in &self.order[n.range.clone()] {
                let d = self.lines[i].distance(p);
                if d < best.1 {
                    *best = (i, d);
                }
            }
        }
    }
    pub fn in_radius(&self, p: XY, radius: f64) -> Vec<usize> {
        let mut result = Vec::new();
        if !self.nodes.is_empty() {
            self.radius(0, p, radius, &mut result);
        }
        result
    }
    fn radius(&self, idx: usize, p: XY, r: f64, result: &mut Vec<usize>) {
        if self.box_distance(idx, p) > r {
            return;
        }
        let n = &self.nodes[idx];
        if let Some((l, h)) = n.children {
            self.radius(l, p, r, result);
            self.radius(h, p, r, result);
        } else {
            result.extend(
                self.order[n.range.clone()]
                    .iter()
                    .copied()
                    .filter(|&i| self.lines[i].distance(p) <= r),
            );
        }
    }
    pub fn intersections(&self, line: Line) -> Vec<f64> {
        let mut result = Vec::new();
        if !self.nodes.is_empty() {
            let min = [line.a[0].min(line.b[0]), line.a[1].min(line.b[1])];
            let max = [line.a[0].max(line.b[0]), line.a[1].max(line.b[1])];
            self.crossings(0, line, min, max, &mut result);
        }
        result.sort_by(f64::total_cmp);
        result.dedup_by(|a, b| (*a - *b).abs() < 1e-8);
        result
    }
    /// Intersection parameters along `line` with the lines of the subtree whose boxes overlap
    /// the line's box (a small tolerance keeps touching boxes).
    fn crossings(&self, idx: usize, line: Line, min: XY, max: XY, result: &mut Vec<f64>) {
        let n = &self.nodes[idx];
        let tolerance = 1e-6;
        if n.min[0] > max[0] + tolerance
            || n.max[0] < min[0] - tolerance
            || n.min[1] > max[1] + tolerance
            || n.max[1] < min[1] - tolerance
        {
            return;
        }
        if let Some((l, r)) = n.children {
            self.crossings(l, line, min, max, result);
            self.crossings(r, line, min, max, result);
        } else {
            result.extend(
                self.order[n.range.clone()]
                    .iter()
                    .filter_map(|&i| line.intersection(self.lines[i])),
            );
        }
    }
    /// Even/odd membership works for disjoint islands, holes and nested islands.
    pub fn contains(&self, p: XY) -> bool {
        !self.nodes.is_empty() && self.contains_node(0, p)
    }
    fn contains_node(&self, index: usize, p: XY) -> bool {
        let node = &self.nodes[index];
        // A crossing requires min_y <= y < max_y. Keep the original crossing arithmetic;
        // parity is independent of traversal order, including shared vertices and holes.
        if p[1] < node.min[1] || p[1] >= node.max[1] {
            return false;
        }
        if let Some((left, right)) = node.children {
            return self.contains_node(left, p) ^ self.contains_node(right, p);
        }
        let mut inside = false;
        for &i in &self.order[node.range.clone()] {
            let l = &self.lines[i];
            if (l.a[1] > p[1]) != (l.b[1] > p[1])
                && p[0] < (l.b[0] - l.a[0]) * (p[1] - l.a[1]) / (l.b[1] - l.a[1]) + l.a[0]
            {
                inside = !inside;
            }
        }
        inside
    }
    pub fn signed_distance(&self, p: XY) -> f64 {
        let d = self.nearest(p).map_or(f64::INFINITY, |(_, d)| d);
        if self.contains(p) { -d } else { d }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indexed_membership_matches_crossing_scan_at_vertices_and_holes() {
        let mut lines = Vec::new();
        for (center, radius) in [([0., 0.], 10.), ([0., 0.], 5.), ([15., 0.], 2.)] {
            let ring: Vec<_> = (0..64)
                .map(|i| {
                    let angle = i as f64 * std::f64::consts::TAU / 64.;
                    [
                        center[0] + radius * angle.cos(),
                        center[1] + radius * angle.sin(),
                    ]
                })
                .collect();
            lines.extend((0..ring.len()).map(|i| Line {
                a: ring[i],
                b: ring[(i + 1) % ring.len()],
            }));
        }
        let tree = Lines::new(lines);
        let points = tree.lines.iter().flat_map(|line| [line.a, line.b]).chain(
            (-40..=40).flat_map(|x| (-24..=24).map(move |y| [x as f64 * 0.5, y as f64 * 0.5])),
        );
        for p in points {
            let inside = tree
                .lines
                .iter()
                .filter(|l| {
                    (l.a[1] > p[1]) != (l.b[1] > p[1])
                        && p[0] < (l.b[0] - l.a[0]) * (p[1] - l.a[1]) / (l.b[1] - l.a[1]) + l.a[0]
                })
                .count()
                % 2
                != 0;
            assert_eq!(tree.contains(p), inside, "point {p:?}");
        }
        assert!(!Lines::default().contains([0., 0.]));
    }
}
