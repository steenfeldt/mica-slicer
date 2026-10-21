// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/Geometry/ArcWelder.cpp :: fit_path, try_create_circle, try_create_arc, arc_discretize, reverse, clip_start, clip_end, point_to_path_projection, split_at
// Source: prusaslicer@b028299c77 src/libslic3r/Geometry/ArcWelder.hpp :: Segment, arc_center, arc_angle, arc_fit_center_gauss_newton_ls
// Source: prusaslicer@b028299c77 src/libslic3r/Geometry/Circle.hpp :: try_circle_center
// Changes: f64 scaled coordinates, slice/index APIs, greedy extension with full validation; no scarf fractions.
// Changes: Bisector fallback intersects the source segment; finite/degenerate input guards added.
// Arc Welder: Anti-Stutter Library, Copyright (C) 2021 Brad Hochgesang, FormerLurker@pm.me.
// Upstream merging algorithm originates in https://github.com/FormerLurker/ArcWelderLib (AGPL-3.0-or-later).

// The following code for merging circles into arches originates from https://github.com/FormerLurker/ArcWelderLib

////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////
// Arc Welder: Anti-Stutter Library
//
// Compresses many G0/G1 commands into G2/G3(arc) commands where possible, ensuring the tool paths stay within the specified resolution.
// This reduces file size and the number of gcodes per second.
//
// Uses the 'Gcode Processor Library' for gcode parsing, position processing, logging, and other various functionality.
//
// Copyright(C) 2021 - Brad Hochgesang
////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////
// This program is free software : you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published
// by the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.See the
// GNU Affero General Public License for more details.
//
//
// You can contact the author at the following email address:
// FormerLurker@pm.me
////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////

//! Smooth paths in scaled units. Radius is zero for a line, positive for a minor arc,
//! negative for a major arc. The first segment supplies only the starting point.

use super::{Point, SCALE, polyline_length, simplify::douglas_peucker};
use std::f64::consts::{PI, TAU};

pub const MAX_RADIUS: f64 = 600.0 * SCALE;
pub const LENGTH_TOLERANCE: f64 = 0.05;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Segment {
    pub point: Point,
    pub radius: f64,
    pub ccw: bool,
}
impl Segment {
    pub fn line(point: Point) -> Self {
        Self {
            point,
            radius: 0.0,
            ccw: true,
        }
    }
}
pub type Path = Vec<Segment>;

#[derive(Clone, Copy, Debug)]
struct V(f64, f64);
impl V {
    fn point(self) -> Point {
        Point::new(self.0.round() as i64, self.1.round() as i64)
    }
    fn dot(self, b: Self) -> f64 {
        self.0 * b.0 + self.1 * b.1
    }
    fn cross(self, b: Self) -> f64 {
        self.0 * b.1 - self.1 * b.0
    }
    fn norm(self) -> f64 {
        self.0.hypot(self.1)
    }
    fn perp(self) -> Self {
        Self(-self.1, self.0)
    }
    fn rotate(self, angle: f64) -> Self {
        let (s, c) = angle.sin_cos();
        Self(c * self.0 - s * self.1, s * self.0 + c * self.1)
    }
}
impl From<Point> for V {
    fn from(p: Point) -> Self {
        Self(p.x as f64, p.y as f64)
    }
}
impl std::ops::Add for V {
    type Output = Self;
    fn add(self, b: Self) -> Self {
        Self(self.0 + b.0, self.1 + b.1)
    }
}
impl std::ops::Sub for V {
    type Output = Self;
    fn sub(self, b: Self) -> Self {
        Self(self.0 - b.0, self.1 - b.1)
    }
}
impl std::ops::Mul<f64> for V {
    type Output = Self;
    fn mul(self, b: f64) -> Self {
        Self(self.0 * b, self.1 * b)
    }
}

fn center(a: V, b: V, radius: f64, ccw: bool) -> V {
    let v = b - a;
    let t = (radius * radius / v.dot(v) - 0.25).max(0.0).sqrt();
    (a + b) * 0.5 + v.perp() * (if (radius > 0.0) == ccw { t } else { -t })
}
/// Centre of a non-degenerate arc (scaled coordinates, retained at floating precision).
pub fn arc_center(a: Point, b: Point, radius: f64, ccw: bool) -> (f64, f64) {
    let c = center(a.into(), b.into(), radius, ccw);
    (c.0, c.1)
}
pub fn arc_angle(a: Point, b: Point, radius: f64) -> f64 {
    let minor = 2.0 * (0.5 * a.distance_to(b) / radius.abs()).min(1.0).asin();
    if radius > 0.0 { minor } else { TAU - minor }
}
fn angle(a: V, b: V, ccw: bool) -> f64 {
    let t = a.cross(b).atan2(a.dot(b)) * if ccw { 1.0 } else { -1.0 };
    if t < 0.0 { t + TAU } else { t }
}
pub fn segment_length(start: &Segment, end: &Segment) -> f64 {
    if end.radius == 0.0 {
        start.point.distance_to(end.point)
    } else {
        end.radius.abs() * arc_angle(start.point, end.point, end.radius)
    }
}
pub fn path_length(path: &[Segment]) -> f64 {
    path.windows(2).map(|p| segment_length(&p[0], &p[1])).sum()
}
fn sample(start: Point, end: Segment, t: f64) -> Point {
    let a = V::from(start);
    let b = V::from(end.point);
    if end.radius == 0.0 {
        (a + (b - a) * t).point()
    } else {
        let c = center(a, b, end.radius, end.ccw);
        let theta = arc_angle(start, end.point, end.radius) * t * if end.ccw { 1.0 } else { -1.0 };
        (c + (a - c).rotate(theta)).point()
    }
}
/// Discretize to the specified maximum chord deviation. Endpoints are preserved exactly.
pub fn arc_discretize(a: Point, b: Point, radius: f64, ccw: bool, deviation: f64) -> Vec<Point> {
    assert!(radius != 0.0 && a != b && deviation > 0.0);
    let theta = arc_angle(a, b, radius);
    let r = radius.abs();
    let n = if r - deviation <= 0.0 {
        if theta < PI || r * (1.0 + (PI - 0.5 * theta).cos()) < deviation {
            1
        } else {
            2
        }
    } else {
        (theta / (2.0 * (1.0 - deviation / r).acos())).ceil() as usize
    }
    .max(1);
    let seg = Segment {
        point: b,
        radius,
        ccw,
    };
    let mut pts = Vec::with_capacity(n + 1);
    pts.push(a);
    pts.extend((1..n).map(|i| sample(a, seg, i as f64 / n as f64)));
    pts.push(b);
    pts
}

fn circle(a: V, b: V, c: V) -> Option<V> {
    let (b, c) = (b - a, c - a);
    let d = b.cross(c);
    if d.abs() < 100.0 {
        return None;
    }
    let center = a + (b * c.dot(c) - c * b.dot(b)).perp() * (0.5 / d);
    ((center - a).norm() <= MAX_RADIUS).then_some(center)
}
fn projection(a: V, b: V, p: V) -> V {
    let v = b - a;
    let l2 = v.dot(v);
    a + v * if l2 > 0.0 {
        ((p - a).dot(v) / l2).clamp(0.0, 1.0)
    } else {
        0.0
    }
}
fn sufficient(pts: &[Point], c: V, r: f64, tol: f64) -> bool {
    pts.iter()
        .all(|p| ((V::from(*p) - c).norm() - r).abs() <= tol)
        && pts
            .windows(2)
            .all(|w| ((projection(w[0].into(), w[1].into(), c) - c).norm() - r).abs() <= tol)
}
fn least_squares(pts: &[Point], initial: V, iterations: usize) -> Option<V> {
    let a = V::from(pts[0]);
    let b = V::from(*pts.last()?);
    let v = b - a;
    let len = v.norm();
    if len == 0.0 {
        return None;
    }
    let y = v * (1.0 / len);
    let x = y.perp();
    // Local origin avoids cancellation for short arcs far from the coordinate origin.
    let mid = (a + b) * 0.5;
    let mut cx = x.dot(initial - mid);
    let a2 = (len * 0.5).powi(2);
    for _ in 0..iterations {
        let r = (a2 + cx * cx).sqrt();
        let (mut num, mut denom) = (0.0, 0.0);
        for i in 1..pts.len() {
            let prev = V::from(pts[i - 1]);
            let p = V::from(pts[i]);
            for q in [Some((prev + p) * 0.5), (i + 1 < pts.len()).then_some(p)]
                .into_iter()
                .flatten()
            {
                let q = q - mid;
                let v = x.dot(q) - cx;
                let ri = v.hypot(y.dot(q));
                if ri > 1e-9 {
                    let j = -cx / r - v / ri;
                    num += j * (ri - r);
                    denom += j * j;
                }
            }
        }
        if denom == 0.0 {
            return None;
        }
        cx -= num / denom;
        if !cx.is_finite() {
            return None;
        }
    }
    Some(mid + x * cx)
}
fn try_arc(pts: &[Point], tol: f64, length_tol: f64) -> Option<Segment> {
    let a = V::from(pts[0]);
    let b = V::from(*pts.last()?);
    if (b - a).norm() < 1.0 {
        return None;
    }
    let mut c = circle(a, pts[pts.len() / 2].into(), b)
        .filter(|c| sufficient(pts, *c, (*c - a).norm(), 2.0 * tol));
    if c.is_none() && pts.len() > 3 {
        let v = b - a;
        let mid = (a + b) * 0.5;
        for w in pts.windows(2) {
            let p = V::from(w[0]);
            let q = V::from(w[1]);
            let dp = v.dot(p - mid);
            let dq = v.dot(q - mid);
            if dp * dq <= 0.0 && dp != dq {
                let intersection = p + (q - p) * (dp / (dp - dq));
                c = circle(a, intersection, b)
                    .filter(|c| sufficient(pts, *c, (*c - a).norm(), 2.0 * tol));
                break;
            }
        }
    }
    let c = least_squares(pts, c?, if pts.len() == 3 { 3 } else { 5 })?;
    let r = (c - a).norm();
    if r > MAX_RADIUS || !sufficient(pts, c, r, tol) {
        return None;
    }
    let mut direction = 0.0_f64;
    for w in pts.windows(2) {
        let cross = (V::from(w[0]) - c).cross(V::from(w[1]) - c);
        if cross.abs() > 1e-6 {
            if direction * cross < 0.0 {
                return None;
            }
            direction = cross;
        }
    }
    if direction == 0.0 {
        return None;
    }
    let ccw = direction > 0.0;
    let theta = angle(a - c, b - c, ccw);
    let length = polyline_length(pts);
    if (r * theta - length).abs() >= length_tol * length {
        return None;
    }
    Some(Segment {
        point: *pts.last()?,
        radius: if theta > PI { -r } else { r },
        ccw,
    })
}
fn decimate_tail(path: &mut Path, begin: usize, tolerance: f64) {
    if path.len() - begin > 2 {
        let points: Vec<_> = path[begin..].iter().map(|s| s.point).collect();
        let simplified = douglas_peucker(&points, tolerance);
        path.truncate(begin + 1); // Preserve the incoming arc at the start of the linear run.
        path.extend(simplified.into_iter().skip(1).map(Segment::line));
    }
}
/// Fit arcs with endpoint interpolation, radial/chord and relative length validation.
/// Zero length tolerance disables arcs, zero geometric tolerance disables all decimation.
pub fn fit_path(points: &[Point], tolerance: f64, length_tolerance: f64) -> Path {
    if points.len() <= 2 || tolerance <= 0.0 {
        return points.iter().copied().map(Segment::line).collect();
    }
    let fine = (0.03 * tolerance).max(0.000060 * SCALE);
    if length_tolerance <= 0.0 || fine > 0.5 * tolerance {
        return douglas_peucker(points, tolerance)
            .into_iter()
            .map(Segment::line)
            .collect();
    }
    let mut src = douglas_peucker(points, fine);
    src.dedup();
    if src.len() < 2 {
        return src.into_iter().map(Segment::line).collect();
    }
    let mut out = vec![Segment::line(src[0])];
    let mut linear_start = 0;
    let mut i = 1;
    while i < src.len() {
        let start = i - 1;
        let mut best = None;
        for end in i + 1..src.len() {
            match try_arc(&src[start..=end], tolerance, length_tolerance) {
                Some(arc) => best = Some((end, arc)),
                None => break,
            }
        }
        if let Some((end, arc)) = best {
            // Avoid quantizing both endpoints to the same G-code point; prefer a line for flat arcs.
            let a = V::from(src[start]);
            let b = V::from(arc.point);
            if (b - a).dot(b - a) >= 2.0 * (0.0011 * SCALE).powi(2)
                && src[start + 1..end].iter().any(|p| {
                    let p = V::from(*p);
                    (p - projection(a, b, p)).norm() > tolerance
                })
            {
                decimate_tail(&mut out, linear_start, tolerance);
                linear_start = out.len();
                out.push(arc);
                i = end + 1;
                continue;
            }
        }
        out.push(Segment::line(src[i]));
        i += 1;
    }
    decimate_tail(&mut out, linear_start, tolerance);
    out
}

pub fn reverse(path: &mut Path) {
    for i in 1..path.len() {
        path[i - 1].radius = path[i].radius;
        path[i - 1].ccw = !path[i].ccw;
    }
    if let Some(last) = path.last_mut() {
        last.radius = 0.0;
    }
    path.reverse();
}
/// Remove length from the end, returning any distance beyond the path's length.
pub fn clip_end(path: &mut Path, mut distance: f64) -> f64 {
    while distance > 0.0 && path.len() > 1 {
        let end = path.pop().unwrap();
        let start = *path.last().unwrap();
        let len = segment_length(&start, &end);
        if distance < len {
            let mut clipped = end;
            clipped.point = sample(start.point, end, 1.0 - distance / len);
            if end.radius < 0.0 && (len - distance) / end.radius.abs() <= PI {
                clipped.radius = -end.radius;
            }
            path.push(clipped);
            return 0.0;
        }
        distance -= len;
    }
    if path.len() < 2 {
        path.clear();
    }
    distance
}
pub fn clip_start(path: &mut Path, distance: f64) -> f64 {
    reverse(path);
    let left = clip_end(path, distance);
    reverse(path);
    left
}
#[derive(Clone, Copy, Debug)]
pub struct PathSegmentProjection {
    pub segment_id: usize,
    pub point: Point,
    pub distance2: f64,
}
pub fn point_to_path_projection(path: &[Segment], point: Point) -> Option<PathSegmentProjection> {
    let p = V::from(point);
    let mut best: Option<PathSegmentProjection> = None;
    for (i, w) in path.windows(2).enumerate() {
        let a = V::from(w[0].point);
        let b = V::from(w[1].point);
        let q = if w[1].radius == 0.0 {
            projection(a, b, p)
        } else {
            let c = center(a, b, w[1].radius, w[1].ccw);
            let v = p - c;
            let theta = angle(a - c, v, w[1].ccw);
            if v.norm() > 0.0 && theta <= arc_angle(w[0].point, w[1].point, w[1].radius) {
                c + v * (w[1].radius.abs() / v.norm())
            } else if (p - a).norm() <= (p - b).norm() {
                a
            } else {
                b
            }
        };
        let d = (p - q).dot(p - q);
        if best.is_none_or(|b| d < b.distance2) {
            best = Some(PathSegmentProjection {
                segment_id: i,
                point: q.point(),
                distance2: d,
            });
        }
    }
    best
}
/// Split at the closest point, snapping to segment endpoints within `min_segment_length`.
pub fn split_at(path: &[Segment], point: Point, min_segment_length: f64) -> (Path, Path) {
    let Some(proj) = point_to_path_projection(path, point) else {
        return (path.to_vec(), Vec::new());
    };
    let i = proj.segment_id;
    let (start, end) = (path[i], path[i + 1]);
    let ds = proj.point.distance_to(start.point);
    let de = proj.point.distance_to(end.point);
    if ds == 0.0 || de == 0.0 || ds.min(de) < min_segment_length {
        let j = if ds <= de { i } else { i + 1 };
        if j == 0 {
            return (Vec::new(), path.to_vec());
        }
        if j + 1 == path.len() {
            return (path.to_vec(), Vec::new());
        }
        let mut right = path[j..].to_vec();
        right[0].radius = 0.0;
        return (path[..=j].to_vec(), right);
    }
    let mut left = path[..=i + 1].to_vec();
    let mut right = path[i..].to_vec();
    left[i + 1].point = proj.point;
    right[0] = Segment::line(proj.point);
    if end.radius < 0.0 {
        let a = V::from(start.point);
        let b = V::from(end.point);
        let c = center(a, b, end.radius, end.ccw);
        let q = V::from(proj.point);
        if angle(a - c, q - c, end.ccw) <= PI {
            left[i + 1].radius = -end.radius;
        }
        if angle(q - c, b - c, end.ccw) <= PI {
            right[1].radius = -end.radius;
        }
    }
    (left, right)
}

#[cfg(test)]
#[path = "arc_welder_tests.rs"]
mod tests;
