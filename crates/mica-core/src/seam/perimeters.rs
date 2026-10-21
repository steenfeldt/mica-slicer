// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/SeamPerimeters.cpp :: Perimeter::create, classify_points, merge_angle_types, offset_along_perimeter
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/SeamChoice.cpp :: maybe_choose_seam_point
// Changes: Boundary-based planning with signed previous-layer distances; no extrusion-role bridge intervals or embedded regions until region perimeters exist.
// Changes: Linear queries replace KD trees; simplify before painting, preserve paint transitions and classify inserted points.
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/SeamPerimeters.cpp :: classify_overhangs, get_overhang_angle, oversample_painted, get_point_types, remove_redundant_points
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/SeamGeometry.cpp :: get_vertex_angles, oversample_edge
// Source: prusaslicer@b028299c77 src/libslic3r/MultiPoint.hpp :: douglas_peucker
// Changes: Prusa profiles (Boundary::new_prusa) follow Perimeter::create's order and arithmetic: classify, oversample painted edges, point types, then simplify each type/classification run; Orca keeps the earlier simplify-first model.

use super::geometry::{self, Choice, V};
use super::index::NearestIndex;
use super::painting::{Paint, SeamPainting};
use crate::geometry::{BoundingBox, ExPolygon, Point, Polygon, SCALE, signed_area};
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Class {
    Common,
    Overhang,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Angle {
    Convex,
    Concave,
    Smooth,
}
fn angle_type(a: f64) -> Angle {
    if a > 10.0f64.to_radians() {
        Angle::Convex
    } else if a < -15.0f64.to_radians() {
        Angle::Concave
    } else {
        Angle::Smooth
    }
}

pub(super) struct Boundary {
    pub points: Vec<V>,
    pub angles: Vec<f64>,
    pub kinds: Vec<Angle>,
    pub paint: Vec<Paint>,
    pub classes: Vec<Class>,
    pub bbox: BoundingBox,
    pub hole: bool,
    pub z: f64,
    /// Built by [`Self::new_prusa`]: candidate searches follow PrusaSlicer exactly.
    pub parity: bool,
    /// Grid over the eligible candidates, built on the first nearest-point query.
    index: OnceLock<NearestIndex>,
}
impl Boundary {
    /// The eligible candidates' spatial index (see [`NearestIndex`]). It reflects `paint` and
    /// `classes` as of the first query, so every later change must go through [`Self::set_paint`]
    /// or [`Self::mark_overhang`], which discard it.
    pub fn index(&self) -> &NearestIndex {
        self.index
            .get_or_init(|| NearestIndex::new(&self.points, &self.paint, &self.classes))
    }
    #[cfg(test)]
    pub fn set_paint(&mut self, i: usize, paint: Paint) {
        self.paint[i] = paint;
        self.index = OnceLock::new();
    }
    /// An unsupported floating start: every candidate counts as an overhang.
    pub fn mark_overhang(&mut self) {
        self.classes.fill(Class::Overhang);
        self.index = OnceLock::new();
    }
    pub fn new(
        poly: &Polygon,
        previous: &[ExPolygon],
        z: f64,
        height: f64,
        painting: &dyn SeamPainting,
    ) -> Option<Self> {
        let bbox = BoundingBox::from_points(poly.iter())?;
        let simplified = crate::geometry::simplify::douglas_peucker(poly, 0.001 * SCALE);
        let mut points = Vec::new();
        for i in 0..simplified.len() {
            let a = V::from(simplified[i]);
            let b = V::from(simplified[(i + 1) % simplified.len()]);
            points.push(a);
            let mid = (a + b) * 0.5;
            let length = (b - a).norm();
            if painting.intersects([mid.x, mid.y, z], length * 0.5) {
                let n = (length / 0.2).ceil() as usize;
                for k in 1..n {
                    points.push(a + (b - a) * (k as f64 / n as f64));
                }
            }
        }
        let paint = points
            .iter()
            .map(|p| painting.classify([p.x, p.y, z], 0.05))
            .collect();
        let lower: Vec<Vec<V>> = previous
            .iter()
            .flat_map(ExPolygon::polygons)
            .map(|p| p.iter().copied().map(V::from).collect())
            .collect();
        let classes = points
            .iter()
            .map(|&p| {
                let inside = previous.iter().any(|e| {
                    geometry::contains(&e.contour, p)
                        && !e.holes.iter().any(|h| geometry::contains(h, p))
                });
                let distance = if inside {
                    0.0
                } else {
                    lower
                        .iter()
                        .map(|poly| (geometry::closest(poly, p).point - p).norm())
                        .fold(f64::INFINITY, f64::min)
                };
                if !previous.is_empty() && distance > height * 55.0f64.to_radians().tan() {
                    Class::Overhang
                } else {
                    Class::Common
                }
            })
            .collect();
        Some(Self::finish(points, paint, classes, bbox, poly, z, false))
    }

    /// Angles, angle types and the remaining fields shared by both constructors. With `exact`,
    /// angles use PrusaSlicer's arithmetic (`get_vertex_angles`, `merge_angle_types`).
    fn finish(
        points: Vec<V>,
        paint: Vec<Paint>,
        classes: Vec<Class>,
        bbox: BoundingBox,
        poly: &Polygon,
        z: f64,
        exact: bool,
    ) -> Self {
        let (angles, smooth) = if exact {
            (vertex_angles(&points, 0.25), vertex_angles(&points, 0.5))
        } else {
            (
                geometry::angles(&points, 0.25),
                geometry::angles(&points, 0.5),
            )
        };
        let distance = |a: V, b: V| {
            if exact { norm(a - b) } else { (a - b).norm() }
        };
        let mut kinds: Vec<Angle> = angles.iter().copied().map(angle_type).collect();
        let sharp = kinds.clone();
        // Use the longer arm's angle only when no genuine sharp corner of that type is nearby.
        for i in 0..points.len() {
            let smooth_kind = angle_type(smooth[i]);
            if smooth_kind != sharp[i] && smooth_kind != Angle::Smooth {
                let n = points.len();
                let mut nearby = false;
                for forward in [false, true] {
                    for k in 0..n {
                        let j = if forward {
                            (i + k) % n
                        } else {
                            (i + n - 1 - k) % n
                        };
                        if distance(points[j], points[i]) > 0.5 {
                            break;
                        }
                        if sharp[j] == smooth_kind {
                            nearby = true;
                        }
                    }
                }
                if !nearby {
                    kinds[i] = smooth_kind;
                }
            }
        }
        Self {
            points,
            angles,
            kinds,
            paint,
            classes,
            bbox,
            hole: signed_area(poly) < 0.0,
            z,
            parity: exact,
            index: OnceLock::new(),
        }
    }

    /// PrusaSlicer 2.9 `Perimeter::create` on a boundary polygon: classify overhangs against the
    /// lower layer, oversample painted edges, classify painting, then simplify each run of equal
    /// type and classification separately (upstream order and arithmetic).
    pub fn new_prusa(
        poly: &Polygon,
        previous: &[ExPolygon],
        z: f64,
        height: f64,
        painting: &dyn SeamPainting,
    ) -> Option<Self> {
        let bbox = BoundingBox::from_points(poly.iter())?;
        let raw: Vec<V> = poly.iter().map(|&p| unscaled(p)).collect();
        let n = raw.len();
        let lower: Vec<Vec<V>> = previous
            .iter()
            .flat_map(ExPolygon::polygons)
            .map(|p| p.iter().map(|&q| unscaled(q)).collect())
            .collect();
        let threshold = 55.0f64.to_radians();
        let classes: Vec<Class> = raw
            .iter()
            .map(|&p| {
                if previous.is_empty() {
                    return Class::Common;
                }
                let inside = previous.iter().any(|e| {
                    geometry::contains(&e.contour, p)
                        && !e.holes.iter().any(|h| geometry::contains(h, p))
                });
                if inside {
                    return Class::Common;
                }
                let distance = lower
                    .iter()
                    .map(|poly| segment_distance(poly, p))
                    .fold(f64::INFINITY, f64::min);
                let angle = if distance > 0.0 {
                    std::f64::consts::FRAC_PI_2 - (height / distance).atan()
                } else {
                    0.0
                };
                if angle > threshold {
                    Class::Overhang
                } else {
                    Class::Common
                }
            })
            .collect();
        let to3 = |p: V| {
            [
                f64::from(p.x as f32),
                f64::from(p.y as f32),
                f64::from(z as f32),
            ]
        };
        let mut points = Vec::with_capacity(n);
        let mut point_classes = Vec::with_capacity(n);
        for i in 0..n {
            points.push(raw[i]);
            point_classes.push(classes[i]);
            let next = if i == n - 1 { 0 } else { i + 1 };
            let length = norm(raw[i] - raw[next]) as f32;
            let middle = (raw[i] + raw[next]) * 0.5;
            if painting.intersects(to3(middle), f64::from(length) / 2.0) {
                for p in oversample_edge(raw[i], raw[next], 0.2) {
                    let mut class = Class::Common;
                    if classes[next] != Class::Common {
                        class = classes[next];
                    }
                    if classes[i] != Class::Common {
                        class = classes[i];
                    }
                    points.push(p);
                    point_classes.push(class);
                }
            }
        }
        let paint: Vec<Paint> = points
            .iter()
            .map(|&p| painting.classify(to3(p), 0.05))
            .collect();
        // remove_redundant_points
        let mut kept = Vec::with_capacity(points.len());
        let mut start = 0;
        for i in 0..points.len() {
            if i + 1 == points.len()
                || paint[i] != paint[i + 1]
                || point_classes[i] != point_classes[i + 1]
            {
                douglas_peucker(&points, start, i, 0.001, &mut kept);
                start = i + 1;
            }
        }
        let points: Vec<V> = kept.iter().map(|&i| points[i]).collect();
        let paint = kept.iter().map(|&i| paint[i]).collect();
        let classes = kept.iter().map(|&i| point_classes[i]).collect();
        Some(Self::finish(points, paint, classes, bbox, poly, z, true))
    }
    /// Prusa's strict type-first search: enforcer > common > blocker; common > overhang.
    pub fn best(&self) -> (Paint, Class) {
        self.paint
            .iter()
            .copied()
            .zip(self.classes.iter().copied())
            .min()
            .unwrap_or((Paint::Common, Class::Common))
    }
    pub fn smooth(&self, c: Choice) -> bool {
        c.previous != c.next || self.kinds[c.previous] == Angle::Smooth
    }
    pub fn overhang(&self, c: Choice) -> bool {
        self.classes[c.previous] == Class::Overhang || self.classes[c.next] == Class::Overhang
    }
    pub fn angle(&self, c: Choice) -> f64 {
        if c.previous == c.next {
            self.angles[c.previous]
        } else {
            0.0
        }
    }
    pub fn normal(&self, c: Choice) -> V {
        geometry::normal(&self.points, c)
    }
    /// Walk the model boundary, preserving an edge's orientation for normal projection.
    pub fn offset(&self, c: Choice, distance: f64, forward: bool) -> Option<Choice> {
        if distance > self.allowed_distance(c, forward) {
            return None;
        }
        if distance <= 1e-9 {
            return Some(c);
        }
        let n = self.points.len();
        let start = if forward {
            if c.previous == c.next {
                (c.next + 1) % n
            } else {
                c.next
            }
        } else if c.previous == c.next {
            (c.previous + n - 1) % n
        } else {
            c.previous
        };
        let mut p = c.point;
        let mut remaining = distance;
        for k in 0..=n {
            // Starting inside an edge leaves one partial edge after visiting all vertices.
            if k == n {
                let edge = c.point - p;
                let length = edge.norm();
                if length > 0.0 && remaining <= length {
                    return Some(Choice {
                        point: p + edge * (remaining / length),
                        ..c
                    });
                }
                return None;
            }
            let i = if forward {
                (start + k) % n
            } else {
                (start + n - k) % n
            };
            let edge = self.points[i] - p;
            let length = edge.norm();
            if length > 0.0 && remaining <= length {
                if (remaining - length).abs() < 1e-9 {
                    return Some(Choice::vertex(&self.points, i));
                }
                return Some(Choice {
                    point: p + edge * (remaining / length),
                    previous: if forward { (i + n - 1) % n } else { i },
                    next: if forward { i } else { (i + 1) % n },
                });
            }
            remaining -= length;
            p = self.points[i];
        }
        None
    }
    /// Distance until entering a worse blocked/overhanging region; one complete turn at most.
    pub fn allowed_distance(&self, c: Choice, forward: bool) -> f64 {
        let n = self.points.len();
        if n == 0 {
            return 0.0;
        }
        let value = (self.paint[c.previous], self.classes[c.previous])
            .min((self.paint[c.next], self.classes[c.next]));
        let mut prev = c.point;
        let mut distance = 0.0;
        let start = if forward {
            if c.previous == c.next {
                (c.next + 1) % n
            } else {
                c.next
            }
        } else {
            c.previous
        };
        for k in 0..=n {
            let i = if forward {
                (start + k) % n
            } else {
                (start + n - k % n) % n
            };
            let worse = (self.paint[i], self.classes[i]) > value;
            if worse && (self.paint[i] == Paint::Blocker || self.classes[i] == Class::Overhang) {
                return distance;
            }
            distance += (self.points[i] - prev).norm();
            prev = self.points[i];
        }
        f64::INFINITY
    }
}

pub(super) fn project(poly: &[Point], point: V, normal: V) -> Choice {
    let points: Vec<V> = poly.iter().copied().map(V::from).collect();
    let depth = (geometry::closest(&points, point).point - point).norm();
    geometry::closest(&points, point - normal * depth)
}

/// `unscaled`: scaled integer coordinates times PrusaSlicer's `SCALING_FACTOR`.
fn unscaled(p: Point) -> V {
    V::new(p.x as f64 * 0.000_001, p.y as f64 * 0.000_001)
}
/// Eigen's `Vec2d::norm`.
fn norm(v: V) -> f64 {
    (v.x * v.x + v.y * v.y).sqrt()
}
fn squared_norm(v: V) -> f64 {
    v.x * v.x + v.y * v.y
}
/// `cross2`, one C++ expression that Apple clang contracts into a fused multiply-add.
fn cross2(a: V, b: V) -> f64 {
    a.x.mul_add(b.y, -(a.y * b.x))
}
/// `Geometry::get_vertex_angles`.
fn vertex_angles(points: &[V], min_arm_length: f64) -> Vec<f64> {
    let n = points.len();
    (0..n)
        .map(|index| {
            // visit_forward from `index` (skipping it) and visit_backward from `index - 1`.
            let next = (1..=n)
                .map(|k| (index + k) % n)
                .find(|&j| j != index && norm(points[j] - points[index]) > min_arm_length);
            let previous = (1..=n)
                .map(|k| (index + n - k) % n)
                .find(|&j| norm(points[j] - points[index]) > min_arm_length);
            match (previous, next) {
                (Some(p), Some(q)) => {
                    let a = points[index] - points[p];
                    let b = points[q] - points[index];
                    -cross2(a, b).atan2(a.x * b.x + a.y * b.y)
                }
                _ => 0.0,
            }
        })
        .collect()
}
/// `Geometry::oversample_edge`.
fn oversample_edge(from: V, to: V, max_distance: f64) -> Vec<V> {
    let total = norm(from - to);
    let count = (total / max_distance).ceil() as usize + 1;
    if count < 3 {
        return Vec::new();
    }
    let step = total / (count - 1) as f64;
    let direction = to - from;
    let length = squared_norm(direction);
    let unit = if length > 0.0 {
        let s = length.sqrt();
        V::new(direction.x / s, direction.y / s)
    } else {
        direction
    };
    let step_vector = unit * step;
    (1..count - 1)
        .map(|i| from + step_vector * i as f64)
        .collect()
}
/// Unsigned distance to the nearest edge of a closed polygon (`AABBTreeLines` distancer).
fn segment_distance(poly: &[V], p: V) -> f64 {
    let n = poly.len();
    let mut best = f64::INFINITY;
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        let v = b - a;
        let va = p - a;
        let l2 = squared_norm(v);
        let t = if l2 > 0.0 {
            ((va.x * v.x + va.y * v.y) / l2).clamp(0.0, 1.0)
        } else {
            0.0
        };
        best = best.min(squared_norm(va - v * t));
    }
    best.sqrt()
}
/// `douglas_peucker<double>` over `points[first..=last]` as an open polyline, appending the
/// indices it keeps.
fn douglas_peucker(points: &[V], first: usize, last: usize, tolerance: f64, out: &mut Vec<usize>) {
    out.push(first);
    if first == last {
        return;
    }
    if first + 1 == last {
        out.push(last);
        return;
    }
    let tolerance_sq = tolerance * tolerance;
    let mut anchor = first;
    let mut floater = last;
    let mut stack = vec![floater];
    loop {
        let take_floater = if anchor + 1 == floater {
            true
        } else {
            let (a, f) = (points[anchor], points[floater]);
            let v = f - a;
            let l2 = squared_norm(v);
            let mut max_dist_sq: Option<f64> = None;
            let mut furthest = anchor;
            for (k, &p) in points.iter().enumerate().take(floater).skip(anchor + 1) {
                let va = p - a;
                let dist_sq = if l2 == 0.0 {
                    squared_norm(va)
                } else {
                    let t = va.x * v.x + va.y * v.y;
                    if t <= 0.0 {
                        squared_norm(va)
                    } else if t >= l2 {
                        squared_norm(p - f)
                    } else {
                        let dt = t / l2;
                        if dt <= 0.0 {
                            squared_norm(va)
                        } else if dt >= 1.0 {
                            squared_norm(p - f)
                        } else {
                            squared_norm(v * dt - va)
                        }
                    }
                };
                if max_dist_sq.is_none_or(|m| dist_sq > m) {
                    max_dist_sq = Some(dist_sq);
                    furthest = k;
                }
            }
            if max_dist_sq.is_some_and(|m| m <= tolerance_sq) {
                true
            } else {
                floater = furthest;
                stack.push(floater);
                false
            }
        };
        if take_floater {
            out.push(floater);
            anchor = floater;
            stack.pop();
            match stack.last() {
                Some(&f) => floater = f,
                None => break,
            }
        }
    }
}
