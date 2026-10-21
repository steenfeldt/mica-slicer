// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: prusaslicer@b028299c77 src/libslic3r/Measure.cpp :: get_measurement, angle_edge_edge, angle_edge_plane, angle_plane_plane
// Changes: Rust port with glam f64 vectors and Eigen's hyperplane / line helpers written out. Circle–circle distance for non-parallel circles minimises the point-to-circle distance over the second circle's angle (dense sampling plus golden-section refinement) instead of GTE's degree-8 polynomial root finding; the parallel case is ported. Edge–plane and circle–plane distances use the border edges stored in the plane feature instead of asking the Measuring object.
// Upstream attribution (Measure.cpp):
// Copyright (c) Prusa Research 2022 - 2023 Lukáš Matěna @lukasmatena, Enrico Turri @enricoturri1966, Vojtěch Bubník @bubnikv, Pavel Mikuš @Godrak
//
// PrusaSlicer is released under the terms of the AGPLv3 or higher
//

//! Distances and angles between two surface features.

use super::{
    EPSILON, Feature, are_parallel, are_perpendicular, edge_direction, get_orthogonal,
    orthonormal_basis,
};
use glam::{DQuat, DVec2, DVec3};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DistAndPoints {
    pub dist: f64,
    pub from: DVec3,
    pub to: DVec3,
}

/// An angle between two edges (or planes, through edges lying on them) meeting at `center`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AngleAndEdges {
    pub angle: f64,
    pub center: DVec3,
    pub e1: (DVec3, DVec3),
    pub e2: (DVec3, DVec3),
    /// Radius of the arc to draw.
    pub radius: f64,
    pub coplanar: bool,
}
impl AngleAndEdges {
    /// Parallel edges or planes: no angle to show (upstream's `Dummy`).
    pub const DUMMY: Self = Self {
        angle: 0.,
        center: DVec3::ZERO,
        e1: (DVec3::ZERO, DVec3::ZERO),
        e2: (DVec3::ZERO, DVec3::ZERO),
        radius: 0.,
        coplanar: true,
    };
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MeasurementResult {
    pub angle: Option<AngleAndEdges>,
    /// Distance between the infinite extensions (perpendicular distance).
    pub distance_infinite: Option<DistAndPoints>,
    /// Distance between the features themselves (direct distance).
    pub distance_strict: Option<DistAndPoints>,
    /// Per-axis absolute difference of two points.
    pub distance_xyz: Option<DVec3>,
}

fn dp(dist: f64, from: DVec3, to: DVec3) -> DistAndPoints {
    DistAndPoints { dist, from, to }
}
fn min_dist(v: &[DistAndPoints]) -> Option<DistAndPoints> {
    v.iter().copied().min_by(|a, b| {
        a.dist
            .partial_cmp(&b.dist)
            .unwrap_or(std::cmp::Ordering::Equal)
    })
}
/// Projection of `p` on the plane through `o` with unit normal `n`.
fn project_on_plane(p: DVec3, n: DVec3, o: DVec3) -> DVec3 {
    p - n * n.dot(p - o)
}
/// Where the line through `a` with direction `d` meets the plane (`n`, `o`).
fn line_plane(a: DVec3, d: DVec3, n: DVec3, o: DVec3) -> DVec3 {
    let denom = n.dot(d);
    if denom.abs() < 1e-15 {
        return a;
    }
    a + d * (n.dot(o - a) / denom)
}
/// Intersection of two 2D lines through `a1`,`a2` and `b1`,`b2`.
fn intersect_2d(a1: DVec2, a2: DVec2, b1: DVec2, b2: DVec2) -> DVec2 {
    let (r, s) = (a2 - a1, b2 - b1);
    let denom = r.perp_dot(s);
    if denom.abs() < 1e-15 {
        return a1;
    }
    a1 + r * ((b1 - a1).perp_dot(s) / denom)
}

pub fn angle_edge_edge(e1: (DVec3, DVec3), e2: (DVec3, DVec3)) -> AngleAndEdges {
    let mut u1 = edge_direction(e1.0, e1.1);
    let mut u2 = edge_direction(e2.0, e2.1);
    if are_parallel(u1, u2) {
        return AngleAndEdges::DUMMY;
    }
    // Project the edges on the plane they span.
    let normal = u1.cross(u2).normalize();
    let mut p11 = project_on_plane(e1.0, normal, e1.0);
    let mut p12 = project_on_plane(e1.1, normal, e1.0);
    let mut p21 = project_on_plane(e2.0, normal, e1.0);
    let mut p22 = project_on_plane(e2.1, normal, e1.0);
    let coplanar = (e2.0 - p21).length() < EPSILON && (e2.1 - p22).length() < EPSILON;
    // Rotate that plane onto XY and intersect the lines there.
    let q = DQuat::from_rotation_arc(normal, DVec3::Z);
    let r = |p: DVec3| q * p;
    let (r11, r12, r21, r22) = (r(p11), r(p12), r(p21), r(p22));
    let flat = |p: DVec3| DVec2::new(p.x, p.y);
    let c2 = intersect_2d(flat(r11), flat(r12), flat(r21), flat(r22));
    let center = q.inverse() * DVec3::new(c2.x, c2.y, r11.z);
    // Both edges pointing away from the centre.
    let (mut out1, mut out2) = (e1, e2);
    if (c2 - flat(r11)).length_squared() > (c2 - flat(r12)).length_squared() {
        std::mem::swap(&mut p11, &mut p12);
        out1 = (e1.1, e1.0);
        u1 = -u1;
    }
    if (c2 - flat(r21)).length_squared() > (c2 - flat(r22)).length_squared() {
        std::mem::swap(&mut p21, &mut p22);
        out2 = (e2.1, e2.0);
        u2 = -u2;
    }
    let angle = u1.dot(u2).clamp(-1., 1.).acos();
    let m1 = (p11 + p12) * 0.5;
    let m2 = (p21 + p22) * 0.5;
    let radius = (center - m1).length().min((center - m2).length());
    AngleAndEdges {
        angle,
        center,
        e1: out1,
        e2: out2,
        radius,
        coplanar,
    }
}

pub fn angle_edge_plane(e: (DVec3, DVec3), normal: DVec3, origin: DVec3) -> AngleAndEdges {
    let mut u = edge_direction(e.0, e.1);
    if are_perpendicular(u, normal) {
        return AngleAndEdges::DUMMY;
    }
    // Point the edge away from where its line meets the plane.
    let inters = line_plane(e.0, e.1 - e.0, normal, origin);
    let (mut e1, mut e2) = e;
    if (e1 - inters).length_squared() > (e2 - inters).length_squared() {
        std::mem::swap(&mut e1, &mut e2);
        u = -u;
    }
    if are_parallel(u, normal) {
        let basis = orthonormal_basis(u);
        let radius = ((e1 + e2) * 0.5 - inters).length();
        let dir = if basis[1].dot(origin - inters) >= 0. {
            basis[1]
        } else {
            -basis[1]
        };
        let mut on_plane = (inters, inters + dir * radius);
        let at_start = (inters - e1).length() < 1e-9;
        if !at_start {
            on_plane.0 += dir * radius;
            on_plane.1 += dir * radius;
        }
        return AngleAndEdges {
            angle: std::f64::consts::FRAC_PI_2,
            center: inters,
            e1: (e1, e2),
            e2: on_plane,
            radius,
            coplanar: at_start,
        };
    }
    let e1e2 = e2 - e1;
    let len = e1e2.length();
    // The second edge lies on the plane, under the first.
    let temp = normal.cross(e1e2);
    let on_unit = normal.cross(temp).normalize();
    let mut on_plane = (origin, origin + on_unit * len);
    let test = (on_plane.1 - on_plane.0).cross(e1e2);
    if test.dot(temp) < 0. {
        on_plane = (origin, origin - on_unit * len);
    }
    let mut ret = angle_edge_edge((e1, e2), on_plane);
    ret.radius = (inters - (e1 + e2) * 0.5).length();
    ret
}

pub fn angle_plane_plane(n1: DVec3, o1: DVec3, n2: DVec3, o2: DVec3) -> AngleAndEdges {
    if are_parallel(n1, n2) {
        return AngleAndEdges::DUMMY;
    }
    // Intersection line of the planes: direction n1 × n2 through the point closest to the origin
    // satisfying both plane equations (the minimum-norm solution, as a QR solve gives).
    let dir = n1.cross(n2).normalize();
    let (d1, d2) = (n1.dot(o1), n2.dot(o2));
    let n12 = n1.dot(n2);
    let det = 1. - n12 * n12;
    let c1 = (d1 - d2 * n12) / det;
    let c2 = (d2 - d1 * n12) / det;
    let line_o = n1 * c1 + n2 * c2;
    let proj = |p: DVec3| line_o + dir * dir.dot(p - line_o);
    let (p1, p2) = (proj(o1), proj(o2));
    let u1 = (o1 - p1).normalize_or_zero();
    let u2 = (o2 - p2).normalize_or_zero();
    let radius = 10f64.max((o1 - p1).length().max((o2 - p2).length()));
    let e1 = (p1 + u1 * radius, p1 + u1 * 2. * radius);
    let e2 = (p2 + u2 * radius, p2 + u2 * 2. * radius);
    let mut ret = angle_edge_edge(e1, e2);
    ret.radius = radius;
    ret
}

/// Point to circle: the nearest point of the circle (`get_measurement` point / circle case).
fn point_circle(p: DVec3, c: DVec3, radius: f64, n: DVec3) -> DistAndPoints {
    let proj = project_on_plane(p, n, c);
    if (proj - c).length() < 1e-9 {
        let on = c + get_orthogonal(n) * radius;
        // The point on the axis is equally far from the whole circle.
        let d = ((p - proj).length_squared() + radius * radius).sqrt();
        return dp(if (p - c).length() < 1e-9 { radius } else { d }, p, on);
    }
    let dist = (((proj - c).length() - radius).powi(2) + (p - proj).length_squared()).sqrt();
    dp(dist, p, c + (proj - c).normalize() * radius)
}

/// Closest points of two circles.
fn circle_circle(c0: DVec3, r0: f64, n0: DVec3, c1: DVec3, r1: f64, n1: DVec3) -> DistAndPoints {
    let d = c1 - c0;
    if !are_parallel(n0, n1) {
        // Minimise the distance from circle 1's point at angle t to circle 0.
        let [u1, v1, _] = orthonormal_basis(n1);
        let at = |t: f64| c1 + (u1 * t.cos() + v1 * t.sin()) * r1;
        let f = |t: f64| point_circle(at(t), c0, r0, n0).dist;
        let n = 720;
        let step = std::f64::consts::TAU / n as f64;
        let mut best = (0, f64::INFINITY);
        for k in 0..n {
            let v = f(k as f64 * step);
            if v < best.1 {
                best = (k, v);
            }
        }
        let (mut a, mut b) = ((best.0 as f64 - 1.) * step, (best.0 as f64 + 1.) * step);
        let g = (5f64.sqrt() - 1.) / 2.;
        for _ in 0..80 {
            let x1 = b - g * (b - a);
            let x2 = a + g * (b - a);
            if f(x1) < f(x2) {
                b = x2;
            } else {
                a = x1;
            }
        }
        let t = (a + b) / 2.;
        let p1 = at(t);
        let on0 = point_circle(p1, c0, r0, n0);
        return dp(on0.dist, on0.to, p1);
    }
    // Parallel circles: by how the projections onto the centre line overlap.
    let mut n0d = n0.dot(d);
    let comp = d - n0 * n0d;
    let dd = comp.length();
    let mut u = comp.normalize_or_zero();
    let dmr1 = dd - r1;
    let (distance, p0, p1);
    if dmr1 >= r0 {
        distance = dmr1 - r0;
        p0 = c0 + u * r0;
        p1 = c1 - u * r1;
    } else {
        let dpr1 = dd + r1;
        if dpr1 <= r0 {
            distance = r0 - dpr1;
            if dd <= 0. {
                u = get_orthogonal(n0);
            }
            p0 = c0 + u * r0;
            p1 = c1 + u * r1;
        } else if dmr1 <= -r0 {
            distance = -r0 - dmr1;
            if dd > 0. {
                p0 = c0 - u * r0;
                p1 = c1 - u * r1;
            } else {
                u = get_orthogonal(n0);
                p0 = c0 + u * r0;
                p1 = c1 + u * r1;
            }
        } else {
            distance = (c1 - c0).length();
            p0 = c0;
            p1 = c1;
            n0d = 0.;
        }
    }
    dp((distance * distance + n0d * n0d).sqrt(), p0, p1)
}

/// Measures between two features (`get_measurement`): distances and, for edges and planes,
/// the angle. The pair is ordered by type first, as upstream does.
pub fn measure(a: &Feature, b: &Feature) -> MeasurementResult {
    let (f1, f2) = if a.rank() > b.rank() { (b, a) } else { (a, b) };
    let mut result = MeasurementResult::default();
    match (f1, f2) {
        (Feature::Point(p), Feature::Point(q)) => {
            let diff = *q - *p;
            result.distance_strict = Some(dp(diff.length(), *p, *q));
            result.distance_xyz = Some(diff.abs());
        }
        (Feature::Point(p), Feature::Edge { a: s, b: e, .. }) => {
            let dir = (*e - *s).normalize_or_zero();
            let proj = *s + dir * dir.dot(*p - *s);
            let dist_inf = (*p - proj).length();
            let len_sq = (*e - *s).length_squared();
            let ds = (proj - *s).length_squared();
            let de = (proj - *e).length_squared();
            result.distance_strict = Some(if ds < len_sq && de < len_sq {
                dp(dist_inf, *p, proj)
            } else {
                let s_closer = ds < de;
                dp(
                    (ds.min(de) + dist_inf * dist_inf).sqrt(),
                    *p,
                    if s_closer { *s } else { *e },
                )
            });
            result.distance_infinite = Some(dp(dist_inf, *p, proj));
        }
        (
            Feature::Point(p),
            Feature::Circle {
                center,
                radius,
                normal,
            },
        ) => {
            let proj = project_on_plane(*p, *normal, *center);
            result.distance_strict = Some(if (proj - *center).length() < 1e-9 {
                dp(
                    *radius,
                    *center,
                    *center + get_orthogonal(*normal) * *radius,
                )
            } else {
                point_circle(*p, *center, *radius, *normal)
            });
        }
        (Feature::Point(p), Feature::Plane { normal, origin, .. }) => {
            let proj = project_on_plane(*p, *normal, *origin);
            result.distance_infinite = Some(dp(normal.dot(*p - *origin).abs(), *p, proj));
        }
        (Feature::Edge { a: a1, b: b1, .. }, Feature::Edge { a: a2, b: b2, .. }) => {
            let (e1, e2) = ((*a1, *b1), (*a2, *b2));
            let mut d = vec![
                dp((e2.0 - e1.0).length(), e1.0, e2.0),
                dp((e2.1 - e1.0).length(), e1.0, e2.1),
                dp((e2.0 - e1.1).length(), e1.1, e2.0),
                dp((e2.1 - e1.1).length(), e1.1, e2.1),
            ];
            let mut point_edge = |v: DVec3, e: (DVec3, DVec3)| {
                let m = measure(
                    &Feature::Point(v),
                    &Feature::Edge {
                        a: e.0,
                        b: e.1,
                        center: None,
                    },
                );
                if let Some(s) = m.distance_strict {
                    let (e1e2, e1v2) = (e.1 - e.0, s.to - e.0);
                    if e1v2.dot(e1e2) >= 0. && e1v2.length() < e1e2.length() {
                        d.push(dp(s.dist, v, s.to));
                    }
                }
            };
            point_edge(e1.0, e2);
            point_edge(e1.1, e2);
            point_edge(e2.0, e1);
            point_edge(e2.1, e1);
            result.distance_infinite = min_dist(&d);
            result.angle = Some(angle_edge_edge(e1, e2));
        }
        (
            Feature::Edge { a: s, b: e, .. },
            Feature::Circle {
                center,
                radius,
                normal,
            },
        ) => {
            let circle = |p: DVec3| point_circle(p, *center, *radius, *normal);
            let mut d = vec![circle(*s), circle(*e)];
            let e1e2 = *e - *s;
            let unit = e1e2.normalize_or_zero();
            let inter = line_plane(*s, e1e2, unit, *center);
            let e1i = inter - *s;
            if e1i.dot(e1e2) >= 0. && e1i.length() < e1e2.length() {
                d.push(circle(inter));
            }
            result.distance_infinite = min_dist(&d);
        }
        (
            Feature::Edge { a: from, b: to, .. },
            Feature::Plane {
                normal,
                origin,
                edges,
                ..
            },
        ) => {
            let unit = (*to - *from).normalize_or_zero();
            if are_perpendicular(unit, *normal) {
                let d = [
                    dp(
                        normal.dot(*from - *origin).abs(),
                        *from,
                        project_on_plane(*from, *normal, *origin),
                    ),
                    dp(
                        normal.dot(*to - *origin).abs(),
                        *to,
                        project_on_plane(*to, *normal, *origin),
                    ),
                ];
                result.distance_infinite = min_dist(&d);
            } else {
                let mut d = vec![];
                for (a, b) in edges.iter() {
                    let m = measure(
                        &Feature::Edge {
                            a: *a,
                            b: *b,
                            center: None,
                        },
                        f1,
                    );
                    match m.distance_infinite {
                        Some(x) => d.push(x),
                        None => {
                            d.clear();
                            break;
                        }
                    }
                }
                result.distance_infinite = min_dist(&d);
            }
            result.angle = Some(angle_edge_plane((*from, *to), *normal, *origin));
        }
        (
            Feature::Circle {
                center: c0,
                radius: r0,
                normal: n0,
            },
            Feature::Circle {
                center: c1,
                radius: r1,
                normal: n1,
            },
        ) => {
            result.distance_infinite = Some(circle_circle(*c0, *r0, *n0, *c1, *r1, *n1));
        }
        (
            Feature::Circle {
                center, normal: n1, ..
            },
            Feature::Plane {
                normal: n2,
                origin,
                edges,
                ..
            },
        ) => {
            let coplanar = are_parallel(*n1, *n2) && n1.dot(*origin - *center).abs() < EPSILON;
            if !coplanar {
                let mut d = vec![];
                for (a, b) in edges.iter() {
                    let m = measure(
                        &Feature::Edge {
                            a: *a,
                            b: *b,
                            center: None,
                        },
                        f1,
                    );
                    match m.distance_infinite {
                        Some(x) => d.push(x),
                        None => {
                            d.clear();
                            break;
                        }
                    }
                }
                result.distance_infinite = min_dist(&d);
            }
        }
        (
            Feature::Plane {
                normal: n1,
                origin: o1,
                ..
            },
            Feature::Plane {
                normal: n2,
                origin: o2,
                ..
            },
        ) => {
            if are_parallel(*n1, *n2) {
                result.distance_infinite = Some(dp(
                    n1.dot(*o2 - *o1).abs(),
                    *o2,
                    project_on_plane(*o2, *n1, *o1),
                ));
            } else {
                result.angle = Some(angle_plane_plane(*n1, *o1, *n2, *o2));
            }
        }
        _ => {}
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    fn v(x: f64, y: f64, z: f64) -> DVec3 {
        DVec3::new(x, y, z)
    }
    fn edge(a: DVec3, b: DVec3) -> Feature {
        Feature::Edge { a, b, center: None }
    }
    fn plane(normal: DVec3, origin: DVec3) -> Feature {
        Feature::Plane {
            index: 0,
            normal,
            origin,
            edges: Arc::new(vec![]),
        }
    }
    #[test]
    fn point_point_and_xyz() {
        let m = measure(
            &Feature::Point(v(0., 0., 0.)),
            &Feature::Point(v(3., -4., 12.)),
        );
        assert!((m.distance_strict.unwrap().dist - 13.).abs() < 1e-12);
        assert_eq!(m.distance_xyz, Some(v(3., 4., 12.)));
    }
    #[test]
    fn point_edge_strict_and_perpendicular() {
        let e = edge(v(0., 0., 0.), v(10., 0., 0.));
        // Above the middle: both distances are 5.
        let m = measure(&Feature::Point(v(5., 5., 0.)), &e);
        assert!((m.distance_strict.unwrap().dist - 5.).abs() < 1e-12);
        assert!((m.distance_infinite.unwrap().dist - 5.).abs() < 1e-12);
        // Beyond the end: perpendicular 3, direct to the end point 5.
        let m = measure(&e, &Feature::Point(v(14., 3., 0.)));
        assert!((m.distance_infinite.unwrap().dist - 3.).abs() < 1e-12);
        assert!((m.distance_strict.unwrap().dist - 5.).abs() < 1e-12);
        assert_eq!(m.distance_strict.unwrap().to, v(10., 0., 0.));
    }
    #[test]
    fn edge_edge_angle_and_distance() {
        let a = edge(v(0., 0., 0.), v(10., 0., 0.));
        let b = edge(v(0., 0., 0.), v(0., 10., 0.));
        let m = measure(&a, &b);
        let angle = m.angle.unwrap();
        assert!((angle.angle.to_degrees() - 90.).abs() < 1e-9);
        assert!(angle.coplanar);
        assert!(angle.center.length() < 1e-9);
        assert!(m.distance_infinite.unwrap().dist < 1e-12);
        // Edges that do not touch: the angle between them pointing away from where their lines
        // cross (here 135°, not the 45° of the lines).
        let c = edge(v(20., 0., 0.), v(30., 10., 0.));
        let m = measure(&a, &c);
        assert!((m.angle.unwrap().angle.to_degrees() - 135.).abs() < 1e-9);
        assert!((m.angle.unwrap().center - v(20., 0., 0.)).length() < 1e-9);
        assert!((m.distance_infinite.unwrap().dist - 10.).abs() < 1e-9);
        // Parallel edges: no angle, the distance between them.
        let d = edge(v(0., 4., 0.), v(10., 4., 0.));
        let m = measure(&a, &d);
        assert_eq!(m.angle, Some(AngleAndEdges::DUMMY));
        assert!((m.distance_infinite.unwrap().dist - 4.).abs() < 1e-12);
    }
    #[test]
    fn planes_parallel_distance_and_angle() {
        let p = plane(v(0., 0., 1.), v(0., 0., 0.));
        let q = plane(v(0., 0., -1.), v(5., 5., 20.));
        let m = measure(&p, &q);
        assert!((m.distance_infinite.unwrap().dist - 20.).abs() < 1e-12);
        let r = plane(v(1., 0., 0.), v(10., 0., 5.));
        let m = measure(&p, &r);
        assert!((m.angle.unwrap().angle.to_degrees() - 90.).abs() < 1e-9);
        let s = plane(v(1., 0., 1.).normalize(), v(10., 0., 5.));
        // The half-planes towards each plane's origin meet at 45° along x = 15.
        let m = measure(&p, &s);
        assert!((m.angle.unwrap().angle.to_degrees() - 45.).abs() < 1e-6);
        assert!((m.angle.unwrap().center.x - 15.).abs() < 1e-6);
        // Point to plane: the perpendicular distance.
        let m = measure(&Feature::Point(v(3., 4., 7.5)), &p);
        assert!((m.distance_infinite.unwrap().dist - 7.5).abs() < 1e-12);
    }
    #[test]
    fn circles_point_distance_and_diameter() {
        let c = Feature::Circle {
            center: v(0., 0., 0.),
            radius: 5.,
            normal: v(0., 0., 1.),
        };
        // From the centre: the radius.
        let m = measure(&Feature::Point(v(0., 0., 0.)), &c);
        assert!((m.distance_strict.unwrap().dist - 5.).abs() < 1e-12);
        let m = measure(&Feature::Point(v(8., 0., 4.)), &c);
        assert!((m.distance_strict.unwrap().dist - 5.).abs() < 1e-12);
        // Two parallel circles side by side: gap between the rims, and concentric ones.
        let d = Feature::Circle {
            center: v(20., 0., 0.),
            radius: 3.,
            normal: v(0., 0., 1.),
        };
        assert!((measure(&c, &d).distance_infinite.unwrap().dist - 12.).abs() < 1e-12);
        let e = Feature::Circle {
            center: v(0., 0., 10.),
            radius: 2.,
            normal: v(0., 0., 1.),
        };
        let gap = measure(&c, &e).distance_infinite.unwrap().dist;
        assert!((gap - (9f64 + 100.).sqrt()).abs() < 1e-9, "{gap}");
        // Perpendicular circles linked like a chain: they cross, distance zero.
        let f = Feature::Circle {
            center: v(8., 0., 0.),
            radius: 3.,
            normal: v(0., 1., 0.),
        };
        assert!(measure(&c, &f).distance_infinite.unwrap().dist < 1e-6);
        // Perpendicular circles apart: the nearest rim points.
        let g = Feature::Circle {
            center: v(20., 0., 0.),
            radius: 5.,
            normal: v(0., 1., 0.),
        };
        assert!((measure(&c, &g).distance_infinite.unwrap().dist - 10.).abs() < 1e-6);
    }
    #[test]
    fn edge_plane_angle() {
        let p = plane(v(0., 0., 1.), v(0., 0., 0.));
        // Rising at 45° from the plane: upstream reports the angle on the plane's side away from
        // the edge's footprint, 135°.
        let e = edge(v(0., 0., 0.), v(10., 0., 10.));
        let m = measure(&e, &p);
        assert!((m.angle.unwrap().angle.to_degrees() - 135.).abs() < 1e-6);
        let vertical = edge(v(0., 0., 2.), v(0., 0., 12.));
        let m = measure(&vertical, &p);
        assert!((m.angle.unwrap().angle.to_degrees() - 90.).abs() < 1e-9);
        // Parallel to the plane: the distance, no angle.
        let flat = edge(v(0., 0., 3.), v(10., 0., 3.));
        let m = measure(&flat, &p);
        assert_eq!(m.angle, Some(AngleAndEdges::DUMMY));
        assert!((m.distance_infinite.unwrap().dist - 3.).abs() < 1e-12);
    }
}
