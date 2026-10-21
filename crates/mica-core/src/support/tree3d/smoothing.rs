// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: prusaslicer@b028299c77 src/libslic3r/MutablePolygon.cpp :: smooth_outward, clip_narrow_corner, remove_duplicates
// Source: prusaslicer@b028299c77 src/libslic3r/MutablePolygon.hpp :: MutablePolygon, range
// Changes: Indexed circular arena replaces mutable iterators; wide arithmetic avoids scaled-coordinate overflow.
// Copyright (c) Prusa Research 2021 - 2022 Lukáš Hejl, Lukáš Matěna, Vojtěch Bubník
// Adapted from Cura ConstPolygonRef::smooth_outward / smooth_corner_complex by Tim Kuipers.

use super::Areas;
use crate::geometry::{Point, Polygon};
const EPS: f64 = 100.;
fn sub(a: Point, b: Point) -> [f64; 2] {
    [(a.x - b.x) as f64, (a.y - b.y) as f64]
}
fn dot(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}
fn cross(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}
fn norm(a: [f64; 2]) -> f64 {
    dot(a, a)
}
fn add(p: Point, v: [f64; 2], t: f64) -> Point {
    Point::new(p.x + (v[0] * t) as i64, p.y + (v[1] * t) as i64)
}
struct Node {
    p: Point,
    prev: usize,
    next: usize,
}
struct Ring {
    nodes: Vec<Node>,
    head: usize,
    len: usize,
    range: Option<(usize, usize)>,
}
impl Ring {
    fn new(p: &[Point]) -> Self {
        Self {
            nodes: p
                .iter()
                .enumerate()
                .map(|(i, &point)| Node {
                    p: point,
                    prev: (i + p.len() - 1) % p.len(),
                    next: (i + 1) % p.len(),
                })
                .collect(),
            head: 0,
            len: p.len(),
            range: None,
        }
    }
    fn p(&self, i: usize) -> Point {
        self.nodes[i].p
    }
    fn prev(&self, i: usize) -> usize {
        self.nodes[i].prev
    }
    fn next(&self, i: usize) -> usize {
        self.nodes[i].next
    }
    fn remove(&mut self, i: usize) -> usize {
        let (prev, next) = (self.prev(i), self.next(i));
        self.nodes[prev].next = next;
        self.nodes[next].prev = prev;
        if self.head == i {
            self.head = next;
        }
        self.len -= 1;
        next
    }
    fn front_remove(&mut self, i: usize) -> usize {
        if let Some((a, b)) = self.range
            && a == i
        {
            self.range = if a == b {
                None
            } else {
                Some((self.next(a), b))
            };
        }
        self.remove(i)
    }
    fn back_remove(&mut self, i: usize) -> usize {
        if let Some((a, b)) = self.range
            && b == i
        {
            self.range = if a == b {
                None
            } else {
                Some((a, self.prev(b)))
            };
        }
        self.remove(i)
    }
    fn insert(&mut self, i: usize, p: Point) -> usize {
        let j = self.nodes.len();
        let prev = self.prev(i);
        self.nodes.push(Node { p, prev, next: i });
        self.nodes[prev].next = j;
        self.nodes[i].prev = j;
        self.len += 1;
        j
    }
    fn polygon(&self) -> Polygon {
        if self.len < 3 {
            return vec![];
        }
        let mut out = Vec::with_capacity(self.len);
        let mut i = self.head;
        for _ in 0..self.len {
            out.push(self.p(i));
            i = self.next(i);
        }
        out
    }
}
#[derive(Clone, Copy, PartialEq)]
enum Status {
    Free,
    Blocked,
    Far,
}
fn narrow(
    r: &mut Ring,
    p1: Point,
    a: &mut usize,
    b: &mut usize,
    mut current: f64,
    length: f64,
) -> bool {
    use Status::*;
    let l2 = length * length;
    let (mut f, mut back) = (Free, Free);
    let (mut p0, mut p2) = (r.p(*a), r.p(*b));
    let (mut p02, mut p22) = (p0, p2);
    let mut next_dist = 0.;
    while r.len >= 3 {
        if f == Far && back == Far {
            p02 = r.p(r.prev(*a));
            p22 = r.p(r.next(*b));
            let d = norm(sub(p22, p02));
            if d <= l2 {
                let n = r.back_remove(*a);
                *a = r.prev(n);
                *b = r.front_remove(*b);
                if r.len <= 2 {
                    r.len = 0;
                    return true;
                }
                f = Free;
                back = Free;
                current = d;
                p0 = p02;
                p2 = p22;
            } else {
                next_dist = d;
                break;
            }
        } else if f != Free && back != Free {
            break;
        }
        if f == Free && (back != Free || norm(sub(p2, p1)) < norm(sub(p0, p1))) {
            p22 = r.p(r.next(*b));
            if cross(sub(p2, p0), sub(p22, p0)) > 0. {
                f = Blocked;
            } else {
                let d = norm(sub(p22, p0));
                if d > l2 {
                    f = Far;
                    next_dist = d;
                } else {
                    *b = r.front_remove(*b);
                    p2 = p22;
                    current = d;
                }
            }
        } else {
            p02 = r.p(r.prev(*a));
            if cross(sub(p02, p2), sub(p0, p2)) > 0. {
                back = Blocked;
            } else {
                let d = norm(sub(p2, p02));
                if d > l2 {
                    back = Far;
                    next_dist = d;
                } else {
                    let n = r.back_remove(*a);
                    *a = r.prev(n);
                    p0 = p02;
                    current = d;
                }
            }
        }
    }
    if r.len <= 3 {
        if r.len < 3 || (f == Far && back == Far) {
            r.len = 0;
        }
        return true;
    }
    if (f == Blocked && back == Blocked) || current > (length - EPS).powi(2) {
    } else if next_dist < (length - EPS).powi(2) {
        if f == Far {
            let n = r.back_remove(*a);
            *a = r.prev(n);
        }
        if back == Far {
            *b = r.front_remove(*b);
        }
        if r.len <= 2 {
            r.len = 0;
            return true;
        }
    } else if f == Blocked || back == Blocked {
        if f == Far {
            std::mem::swap(&mut p0, &mut p2);
            std::mem::swap(&mut p02, &mut p22);
        }
        let v = sub(p02, p0);
        let d = sub(p0, p2);
        let aa = norm(v);
        let bb = 2. * dot(d, v);
        let disc = (bb * bb - 4. * aa * (norm(d) - l2)).max(0.);
        if aa > 0. {
            let t = (-bb + disc.sqrt()) / (2. * aa);
            let i = if back == Far { *b } else { *a };
            r.nodes[i].p = add(r.p(i), v, t);
        }
    } else {
        let d = current.sqrt();
        let t = (length - d) / (next_dist.sqrt() - d);
        r.nodes[*a].p = add(r.p(*a), sub(p02, p0), t);
        r.nodes[*b].p = add(r.p(*b), sub(p22, p2), t);
    }
    false
}
fn smooth_polygon(p: &[Point], length: i64) -> Polygon {
    if p.len() < 3 {
        return vec![];
    }
    let mut r = Ring::new(p);
    // remove_duplicates(polygon, scaled(0.01)), including the closing edge.
    let mut i = r.next(r.head);
    while i != r.head && r.len > 1 {
        if norm(sub(r.p(i), r.p(r.prev(i)))) < 10000_f64.powi(2) {
            i = r.remove(i);
        } else {
            i = r.next(i);
        }
    }
    if r.len < 3 {
        return vec![];
    }
    let length = length as f64;
    let l2 = length * length;
    r.range = Some((r.head, r.prev(r.head)));
    while let Some((i, end)) = r.range {
        if r.len <= 2 {
            break;
        }
        r.range = if i == end {
            None
        } else {
            Some((r.next(i), end))
        };
        let (mut a, mut b) = (r.prev(i), r.next(i));
        let (p0, p1, p2) = (r.p(a), r.p(i), r.p(b));
        let (mut v1, mut v2) = (sub(p0, p1), sub(p2, p1));
        if cross(v1, v2) > 0. {
            let d = dot(v1, v2);
            let (mut l1, mut l2v) = (norm(v1), norm(v2));
            if d > 0. || 2. * d * d < l1 * l2v {
                let dist = norm(sub(p2, p0));
                r.remove(i);
                if dist < l2 {
                    if narrow(&mut r, p1, &mut a, &mut b, dist, length) {
                        return r.polygon();
                    }
                } else if dist > (length + EPS).powi(2) {
                    let swap = l1 > l2v;
                    if swap {
                        std::mem::swap(&mut v1, &mut v2);
                        std::mem::swap(&mut l1, &mut l2v);
                    }
                    let (lv1, lv2) = (l1.sqrt(), l2v.sqrt());
                    let bis = [v1[0] / lv1 + v2[0] / lv2, v1[1] / lv1 + v2[1] / lv2];
                    let d2 = l1 - dot(v1, bis).powi(2) / norm(bis);
                    if d2 < EPS * EPS {
                    } else if d2 < 0.25 * l2 + EPS {
                        let bb = -2. * dot(v1, v2);
                        let u = (bb * bb - 4. * l2v * (l1 - l2)).max(0.);
                        let t = (-bb + u.sqrt()) / (2. * l2v);
                        r.insert(b, add(p1, v2, t));
                    } else {
                        let t = (0.25 * l2 / d2).sqrt();
                        let t2 = t * lv1 / lv2;
                        let (mut q0, mut q2) = (add(p1, v1, t), add(p1, v2, t2));
                        if swap {
                            std::mem::swap(&mut q0, &mut q2);
                        }
                        let ins = r.insert(b, q2);
                        r.insert(ins, q0);
                    }
                }
            }
        }
    }
    if r.len == 3 {
        let p0 = r.p(r.prev(r.head));
        let p1 = r.p(r.head);
        let p2 = r.p(r.next(r.head));
        let (mut v1, mut v2) = (sub(p0, p1), sub(p2, p1));
        if cross(v1, v2) > 0. {
            let v3 = sub(p2, p0);
            let (mut l1, mut l2v) = (norm(v1), norm(v2));
            let l3 = norm(v3);
            if l2v > l1 && l2v > l3 {
                std::mem::swap(&mut v1, &mut v2);
                std::mem::swap(&mut l1, &mut l2v);
            } else if l3 > l1 && l3 > l2v {
                v1 = v3;
                l1 = l3;
            }
            if l2v - dot(v1, v2).powi(2) / l1 < l2 {
                r.len = 0;
            }
        }
    }
    r.polygon()
}
pub(super) fn smooth_outward(areas: Areas, distance: i64) -> Areas {
    areas
        .into_iter()
        .filter_map(|mut ex| {
            ex.contour = smooth_polygon(&ex.contour, distance);
            ex.holes = ex
                .holes
                .iter()
                .map(|p| smooth_polygon(p, distance))
                .filter(|p| p.len() >= 3)
                .collect();
            (ex.contour.len() >= 3).then_some(ex)
        })
        .collect()
}
