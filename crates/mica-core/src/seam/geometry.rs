// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/SeamGeometry.cpp :: get_vertex_angles, get_polygon_normal, bounding_box_distance, get_mapping, distance_to_segment_squared
// Source: prusaslicer@b028299c77 src/libslic3r/Line.hpp :: line_alg::distance_to_squared
// Changes: Scaled Mica points with millimetre vector calculations; linear edge queries and explicit empty/degenerate handling.

use crate::geometry::{BoundingBox, Point, Polygon, SCALE};
use std::ops::{Add, Mul, Sub};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct V {
    pub x: f64,
    pub y: f64,
}
impl V {
    pub fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
    pub fn dot(self, b: Self) -> f64 {
        self.x * b.x + self.y * b.y
    }
    pub fn cross(self, b: Self) -> f64 {
        self.x * b.y - self.y * b.x
    }
    pub fn norm(self) -> f64 {
        self.x.hypot(self.y)
    }
    pub fn unit(self) -> Self {
        let n = self.norm();
        if n > 1e-12 {
            self * (1.0 / n)
        } else {
            Self::default()
        }
    }
    pub fn normal(self) -> Self {
        Self::new(self.y, -self.x).unit()
    }
    pub fn point(self) -> Point {
        Point::from_mm(self.x, self.y)
    }
}
impl From<Point> for V {
    fn from(p: Point) -> Self {
        Self::new(p.x as f64 / SCALE, p.y as f64 / SCALE)
    }
}
impl Add for V {
    type Output = Self;
    fn add(self, b: Self) -> Self {
        Self::new(self.x + b.x, self.y + b.y)
    }
}
impl Sub for V {
    type Output = Self;
    fn sub(self, b: Self) -> Self {
        Self::new(self.x - b.x, self.y - b.y)
    }
}
impl Mul<f64> for V {
    type Output = Self;
    fn mul(self, b: f64) -> Self {
        Self::new(self.x * b, self.y * b)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Choice {
    pub point: V,
    pub previous: usize,
    pub next: usize,
}
impl Choice {
    pub fn vertex(points: &[V], index: usize) -> Self {
        Self {
            point: points[index],
            previous: index,
            next: index,
        }
    }
}

pub(super) fn on_segment(p: V, a: V, b: V) -> V {
    let edge = b - a;
    let len = edge.dot(edge);
    a + edge
        * if len > 1e-20 {
            ((p - a).dot(edge) / len).clamp(0.0, 1.0)
        } else {
            0.0
        }
}
/// `line_alg::distance_to_squared`: the closest point of segment `a`-`b` to `p` and the squared
/// distance, with PrusaSlicer's arithmetic (endpoints returned exactly).
pub(super) fn distance_to_segment_squared(a: V, b: V, p: V) -> (V, f64) {
    let v = b - a;
    let va = p - a;
    let sq = |w: V| w.x * w.x + w.y * w.y;
    let l2 = sq(v);
    if l2 == 0.0 {
        return (a, sq(va));
    }
    let t = va.x * v.x + va.y * v.y;
    if t <= 0.0 {
        (a, sq(va))
    } else if t >= l2 {
        (b, sq(p - b))
    } else {
        let w = v * (t / l2);
        (a + w, sq(w - va))
    }
}
pub(super) fn closest(points: &[V], p: V) -> Choice {
    let mut choice = Choice::default();
    let mut distance = f64::INFINITY;
    for i in 0..points.len() {
        let j = (i + 1) % points.len();
        let point = on_segment(p, points[i], points[j]);
        let d = (point - p).norm();
        if d < distance {
            distance = d;
            choice = if (point - points[i]).norm() < 1e-7 {
                Choice::vertex(points, i)
            } else if (point - points[j]).norm() < 1e-7 {
                Choice::vertex(points, j)
            } else {
                Choice {
                    point,
                    previous: i,
                    next: j,
                }
            };
        }
    }
    choice
}
fn arms(points: &[V], i: usize, min: f64) -> Option<(V, V)> {
    let n = points.len();
    let before = (1..n)
        .map(|k| points[(i + n - k) % n])
        .find(|p| (*p - points[i]).norm() > min)?;
    let after = (1..n)
        .map(|k| points[(i + k) % n])
        .find(|p| (*p - points[i]).norm() > min)?;
    Some((points[i] - before, after - points[i]))
}
pub(super) fn angles(points: &[V], min_arm: f64) -> Vec<f64> {
    (0..points.len())
        .map(|i| arms(points, i, min_arm).map_or(0.0, |(a, b)| -a.cross(b).atan2(a.dot(b))))
        .collect()
}
pub(super) fn normal(points: &[V], c: Choice) -> V {
    if c.previous != c.next {
        (points[c.next] - points[c.previous]).normal()
    } else {
        arms(points, c.previous, 0.1)
            .map_or(V::default(), |(a, b)| (a.normal() + b.normal()).unit())
    }
}
pub(super) fn bb_distance(a: BoundingBox, b: BoundingBox) -> f64 {
    a.min.distance_to(b.min).max(a.max.distance_to(b.max)) / SCALE
}

/// Strongest predecessor wins at a merge. Other branches start new shells at a split.
pub(super) fn mapping(
    sizes: &[usize],
    link: impl Fn(usize, usize) -> Option<(usize, f64)>,
) -> (Vec<Vec<usize>>, usize) {
    let Some(&first) = sizes.first() else {
        return (Vec::new(), 0);
    };
    let mut next_id = first;
    let mut result: Vec<Vec<usize>> = vec![(0..first).collect()];
    for layer in 0..sizes.len() - 1 {
        let mut links = vec![None; sizes[layer + 1]];
        for (i, &bucket) in result[layer].iter().enumerate() {
            if let Some((j, weight)) = link(layer, i)
                && links[j].is_none_or(|(_, w)| weight > w)
            {
                links[j] = Some((bucket, weight));
            }
        }
        result.push(
            links
                .into_iter()
                .map(|l| {
                    l.map_or_else(
                        || {
                            let id = next_id;
                            next_id += 1;
                            id
                        },
                        |(id, _)| id,
                    )
                })
                .collect(),
        );
    }
    (result, next_id)
}

pub(super) fn open(poly: &[Point], at: V) -> Polygon {
    if poly.len() < 2 {
        return Vec::new();
    }
    let points: Vec<V> = poly.iter().copied().map(V::from).collect();
    let c = closest(&points, at);
    let mut out = vec![c.point.point()];
    let start = if c.previous == c.next {
        (c.next + 1) % poly.len()
    } else {
        c.next
    };
    for k in 0..poly.len() {
        let p = poly[(start + k) % poly.len()];
        if out.last() != Some(&p) {
            out.push(p);
        }
    }
    if out.last() != out.first() {
        out.push(out[0]);
    }
    out
}

/// Point and edge on an open polyline at a distance in mm; clamped at the end.
pub(super) fn along(points: &[Point], mut distance: f64) -> (usize, Point) {
    for (i, edge) in points.windows(2).enumerate() {
        let length = edge[0].distance_to(edge[1]) / SCALE;
        if length > distance && length > 0.0 {
            return (
                i,
                (V::from(edge[0]) + (V::from(edge[1]) - V::from(edge[0])) * (distance / length))
                    .point(),
            );
        }
        distance -= length;
    }
    (
        points.len().saturating_sub(2),
        points.last().copied().unwrap_or_default(),
    )
}

pub(super) fn contains(poly: &[Point], p: V) -> bool {
    let mut inside = false;
    for i in 0..poly.len() {
        let a = V::from(poly[i]);
        let b = V::from(poly[(i + 1) % poly.len()]);
        if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
            inside = !inside;
        }
    }
    inside
}

/// Arclength to the closest projection on an open path, in mm.
pub(super) fn distance_along(points: &[Point], target: V) -> f64 {
    let mut distance = 0.0;
    let mut best = (f64::INFINITY, 0.0);
    for w in points.windows(2) {
        let a = V::from(w[0]);
        let b = V::from(w[1]);
        let p = on_segment(target, a, b);
        let d = (p - target).norm();
        if d < best.0 {
            best = (d, distance + (p - a).norm());
        }
        distance += (b - a).norm();
    }
    best.1
}
