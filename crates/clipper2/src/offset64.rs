// SPDX-License-Identifier: BSL-1.0
// Provenance: ported
// Source: prusaslicer@b028299c77 src/clipper/clipper.cpp :: ClipperOffset, GetUnitNormal, Area, Orientation, FRound, Round
// Changes: Rust port of the Clipper 6.4 offsetter as modified by PrusaSlicer (ShortestEdgeLength), on
// Changes: Clipper2's Point64 paths. The finishing union runs on this crate's Clipper64 with Clipper 6.4
// Changes: intersections; the negative-offset bounding rectangle is removed by its bounds instead of by
// Changes: output position. Multiply-adds are fused (`mul_add`) where the reference arm64 build fuses them.

//! Clipper 6.4's polygon offsetter (`ClipperLib::ClipperOffset`), for PrusaSlicer-compatible offsets.

use crate::core::{Path64, Paths64, Point64};
use crate::engine::{with_clipper64_intersections, ClipType};
use crate::engine_public::Clipper64;
use crate::offset::{EndType, JoinType};
use crate::FillRule;

const PI: f64 = std::f64::consts::PI;
const TWO_PI: f64 = PI * 2.;
const DEF_ARC_TOLERANCE: f64 = 0.25;
const TOLERANCE: f64 = 1.0e-20;

/// Clipper 6.4's `Round` (`FRound`: `floor(a + 0.5)`, except for the largest double below one half).
#[inline]
pub fn round64(a: f64) -> i64 {
    if a == 0.49999999999999994 {
        0
    } else {
        (a + 0.5).floor() as i64
    }
}

/// Clipper 6.4's `Area` (positive for counter-clockwise paths).
pub fn area64(poly: &Path64) -> f64 {
    let n = poly.len();
    if n < 3 {
        return 0.;
    }
    let mut a = 0.;
    let mut j = n - 1;
    for i in 0..n {
        a = (poly[j].x as f64 + poly[i].x as f64).mul_add(poly[j].y as f64 - poly[i].y as f64, a);
        j = i;
    }
    -a * 0.5
}

/// Clipper 6.4's `Orientation`: `Area >= 0`.
pub fn orientation64(poly: &Path64) -> bool {
    area64(poly) >= 0.
}

#[derive(Clone, Copy)]
struct Normal {
    x: f64,
    y: f64,
}

fn unit_normal(pt1: Point64, pt2: Point64) -> Normal {
    if pt2.x == pt1.x && pt2.y == pt1.y {
        return Normal { x: 0., y: 0. };
    }
    let mut dx = (pt2.x - pt1.x) as f64;
    let mut dy = (pt2.y - pt1.y) as f64;
    let f = 1.0 / dx.mul_add(dx, dy * dy).sqrt();
    dx *= f;
    dy *= f;
    Normal { x: dy, y: -dx }
}

struct Node {
    contour: Path64,
    join: JoinType,
    end: EndType,
}

/// `ClipperLib::ClipperOffset`.
pub struct ClipperOffset64 {
    pub miter_limit: f64,
    pub arc_tolerance: f64,
    pub shortest_edge_length: f64,
    nodes: Vec<Node>,
    lowest: Option<(usize, usize)>,
    // DoOffset state
    dest_polys: Paths64,
    src: Path64,
    dest: Path64,
    normals: Vec<Normal>,
    delta: f64,
    sin_a: f64,
    sin: f64,
    cos: f64,
    miter_lim: f64,
    steps_per_rad: f64,
}

impl ClipperOffset64 {
    pub fn new(miter_limit: f64, arc_tolerance: f64, shortest_edge_length: f64) -> Self {
        Self {
            miter_limit,
            arc_tolerance,
            shortest_edge_length,
            nodes: Vec::new(),
            lowest: None,
            dest_polys: Vec::new(),
            src: Vec::new(),
            dest: Vec::new(),
            normals: Vec::new(),
            delta: 0.,
            sin_a: 0.,
            sin: 0.,
            cos: 0.,
            miter_lim: 0.,
            steps_per_rad: 0.,
        }
    }

    pub fn clear(&mut self) {
        self.nodes.clear();
        self.lowest = None;
    }

    pub fn add_path(&mut self, path: &Path64, join: JoinType, end: EndType) {
        if path.is_empty() {
            return;
        }
        let mut high = path.len() - 1;
        let shortest = self.shortest_edge_length > 0.;
        let shortest2 = if shortest {
            self.shortest_edge_length * self.shortest_edge_length
        } else {
            0.
        };
        let same = |a: Point64, b: Point64| {
            if shortest {
                let dx = (a.x - b.x) as f64;
                let dy = (a.y - b.y) as f64;
                dx.mul_add(dx, dy * dy) < shortest2
            } else {
                a.x == b.x && a.y == b.y
            }
        };
        if matches!(end, EndType::Joined | EndType::Polygon) {
            while high > 0 && same(path[high], path[0]) {
                high -= 1;
            }
        }
        let mut contour: Path64 = Vec::with_capacity(high + 1);
        contour.push(path[0]);
        let (mut j, mut k) = (0usize, 0usize);
        for &p in &path[1..=high] {
            if same(p, contour[j]) {
                continue;
            }
            j += 1;
            contour.push(p);
            if p.y > contour[k].y || (p.y == contour[k].y && p.x < contour[k].x) {
                k = j;
            }
        }
        if end == EndType::Polygon && j < 2 {
            return;
        }
        self.nodes.push(Node { contour, join, end });
        if end != EndType::Polygon {
            return;
        }
        let idx = self.nodes.len() - 1;
        match self.lowest {
            None => self.lowest = Some((idx, k)),
            Some((li, lk)) => {
                let ip = self.nodes[li].contour[lk];
                let c = self.nodes[idx].contour[k];
                if c.y > ip.y || (c.y == ip.y && c.x < ip.x) {
                    self.lowest = Some((idx, k));
                }
            }
        }
    }

    fn fix_orientations(&mut self) {
        let reverse_all = self
            .lowest
            .is_some_and(|(i, _)| !orientation64(&self.nodes[i].contour));
        for node in &mut self.nodes {
            if reverse_all {
                if node.end == EndType::Polygon
                    || (node.end == EndType::Joined && orientation64(&node.contour))
                {
                    node.contour.reverse();
                }
            } else if node.end == EndType::Joined && !orientation64(&node.contour) {
                node.contour.reverse();
            }
        }
    }

    /// `Execute(Paths&, delta)`.
    pub fn execute(&mut self, delta: f64) -> Paths64 {
        self.fix_orientations();
        self.do_offset(delta);
        with_clipper64_intersections(true, || {
            let mut c = Clipper64::new();
            c.set_preserve_collinear(false);
            c.add_subject(&self.dest_polys);
            let mut solution = Paths64::new();
            if delta > 0. {
                c.execute(ClipType::Union, FillRule::Positive, &mut solution, None);
            } else {
                let r = crate::core::get_bounds_paths(&self.dest_polys);
                let outer = vec![
                    Point64::new(r.left - 10, r.bottom + 10),
                    Point64::new(r.right + 10, r.bottom + 10),
                    Point64::new(r.right + 10, r.top - 10),
                    Point64::new(r.left - 10, r.top - 10),
                ];
                c.add_subject(&vec![outer]);
                c.set_reverse_solution(true);
                c.execute(ClipType::Union, FillRule::Negative, &mut solution, None);
                remove_outer_rect(
                    &mut solution,
                    r.left - 10,
                    r.top - 10,
                    r.right + 10,
                    r.bottom + 10,
                );
            }
            solution
        })
    }

    fn do_offset(&mut self, delta: f64) {
        self.dest_polys.clear();
        self.delta = delta;
        if delta > -TOLERANCE && delta < TOLERANCE {
            for node in &self.nodes {
                if node.end == EndType::Polygon {
                    self.dest_polys.push(node.contour.clone());
                }
            }
            return;
        }
        self.miter_lim = if self.miter_limit > 2. {
            2. / (self.miter_limit * self.miter_limit)
        } else {
            0.5
        };
        let y = if self.arc_tolerance <= 0. {
            DEF_ARC_TOLERANCE
        } else if self.arc_tolerance > delta.abs() * DEF_ARC_TOLERANCE {
            delta.abs() * DEF_ARC_TOLERANCE
        } else {
            self.arc_tolerance
        };
        let mut steps = PI / (1. - y / delta.abs()).acos();
        if steps > delta.abs() * PI {
            steps = delta.abs() * PI;
        }
        self.sin = (TWO_PI / steps).sin();
        self.cos = (TWO_PI / steps).cos();
        self.steps_per_rad = steps / TWO_PI;
        if delta < 0. {
            self.sin = -self.sin;
        }
        let nodes = std::mem::take(&mut self.nodes);
        for node in &nodes {
            self.src = node.contour.clone();
            let len = self.src.len();
            if len == 0 || (delta <= 0. && (len < 3 || node.end != EndType::Polygon)) {
                continue;
            }
            self.dest.clear();
            if len == 1 {
                let s = self.src[0];
                if node.join == JoinType::Round {
                    let (mut x, mut yy) = (1.0f64, 0.0f64);
                    let mut j = 1.;
                    while j <= steps {
                        self.dest.push(Point64::new(
                            round64(x.mul_add(delta, s.x as f64)),
                            round64(yy.mul_add(delta, s.y as f64)),
                        ));
                        let x2 = x;
                        x = x.mul_add(self.cos, -(self.sin * yy));
                        yy = x2.mul_add(self.sin, yy * self.cos);
                        j += 1.;
                    }
                } else {
                    let (mut x, mut yy) = (-1.0f64, -1.0f64);
                    for _ in 0..4 {
                        self.dest.push(Point64::new(
                            round64(x.mul_add(delta, s.x as f64)),
                            round64(yy.mul_add(delta, s.y as f64)),
                        ));
                        if x < 0. {
                            x = 1.;
                        } else if yy < 0. {
                            yy = 1.;
                        } else {
                            x = -1.;
                        }
                    }
                }
                self.dest_polys.push(std::mem::take(&mut self.dest));
                continue;
            }
            self.normals.clear();
            for j in 0..len - 1 {
                self.normals.push(unit_normal(self.src[j], self.src[j + 1]));
            }
            if matches!(node.end, EndType::Joined | EndType::Polygon) {
                self.normals
                    .push(unit_normal(self.src[len - 1], self.src[0]));
            } else {
                self.normals.push(self.normals[len - 2]);
            }
            match node.end {
                EndType::Polygon => {
                    let mut k = len - 1;
                    for j in 0..len {
                        self.offset_point(j, &mut k, node.join);
                    }
                    self.dest_polys.push(std::mem::take(&mut self.dest));
                }
                EndType::Joined => {
                    let mut k = len - 1;
                    for j in 0..len {
                        self.offset_point(j, &mut k, node.join);
                    }
                    self.dest_polys.push(std::mem::take(&mut self.dest));
                    let n = self.normals[len - 1];
                    for j in (1..len).rev() {
                        self.normals[j] = Normal {
                            x: -self.normals[j - 1].x,
                            y: -self.normals[j - 1].y,
                        };
                    }
                    self.normals[0] = Normal { x: -n.x, y: -n.y };
                    k = 0;
                    for j in (0..len).rev() {
                        self.offset_point(j, &mut k, node.join);
                    }
                    self.dest_polys.push(std::mem::take(&mut self.dest));
                }
                _ => {
                    let mut k = 0;
                    for j in 1..len - 1 {
                        self.offset_point(j, &mut k, node.join);
                    }
                    if node.end == EndType::Butt {
                        let j = len - 1;
                        let (s, n) = (self.src[j], self.normals[j]);
                        self.dest.push(Point64::new(
                            round64(n.x.mul_add(delta, s.x as f64)),
                            round64(n.y.mul_add(delta, s.y as f64)),
                        ));
                        self.dest.push(Point64::new(
                            round64((-n.x).mul_add(delta, s.x as f64)),
                            round64((-n.y).mul_add(delta, s.y as f64)),
                        ));
                    } else {
                        let j = len - 1;
                        let k2 = len - 2;
                        self.sin_a = 0.;
                        self.normals[j] = Normal {
                            x: -self.normals[j].x,
                            y: -self.normals[j].y,
                        };
                        if node.end == EndType::Square {
                            self.do_square(j, k2);
                        } else {
                            self.do_round(j, k2);
                        }
                    }
                    for j in (1..len).rev() {
                        self.normals[j] = Normal {
                            x: -self.normals[j - 1].x,
                            y: -self.normals[j - 1].y,
                        };
                    }
                    self.normals[0] = Normal {
                        x: -self.normals[1].x,
                        y: -self.normals[1].y,
                    };
                    k = len - 1;
                    for j in (1..k).rev() {
                        self.offset_point(j, &mut k, node.join);
                    }
                    if node.end == EndType::Butt {
                        let (s, n) = (self.src[0], self.normals[0]);
                        self.dest.push(Point64::new(
                            round64((-n.x).mul_add(delta, s.x as f64)),
                            round64((-n.y).mul_add(delta, s.y as f64)),
                        ));
                        self.dest.push(Point64::new(
                            round64(n.x.mul_add(delta, s.x as f64)),
                            round64(n.y.mul_add(delta, s.y as f64)),
                        ));
                    } else {
                        self.sin_a = 0.;
                        if node.end == EndType::Square {
                            self.do_square(0, 1);
                        } else {
                            self.do_round(0, 1);
                        }
                    }
                    self.dest_polys.push(std::mem::take(&mut self.dest));
                }
            }
        }
        self.nodes = nodes;
    }

    fn push_normal(&mut self, j: usize, n: Normal) {
        let s = self.src[j];
        self.dest.push(Point64::new(
            round64(n.x.mul_add(self.delta, s.x as f64)),
            round64(n.y.mul_add(self.delta, s.y as f64)),
        ));
    }

    fn offset_point(&mut self, j: usize, k: &mut usize, join: JoinType) {
        let (nk, nj) = (self.normals[*k], self.normals[j]);
        self.sin_a = nk.x.mul_add(nj.y, -(nj.x * nk.y));
        if (self.sin_a * self.delta).abs() < 1.0 {
            let cos_a = nk.x.mul_add(nj.x, nj.y * nk.y);
            if cos_a > 0. {
                self.push_normal(j, nk);
                return;
            }
        } else {
            self.sin_a = self.sin_a.clamp(-1.0, 1.0);
        }
        if self.sin_a * self.delta < 0. {
            self.push_normal(j, nk);
            self.dest.push(self.src[j]);
            self.push_normal(j, nj);
        } else {
            match join {
                JoinType::Miter => {
                    let r = 1. + nj.x.mul_add(nk.x, nj.y * nk.y);
                    if r >= self.miter_lim {
                        self.do_miter(j, *k, r);
                    } else {
                        self.do_square(j, *k);
                    }
                }
                JoinType::Square | JoinType::Bevel => self.do_square(j, *k),
                JoinType::Round => self.do_round(j, *k),
            }
        }
        *k = j;
    }

    fn do_square(&mut self, j: usize, k: usize) {
        let (nk, nj, s, d) = (self.normals[k], self.normals[j], self.src[j], self.delta);
        let dx = (self.sin_a.atan2(nk.x.mul_add(nj.x, nk.y * nj.y)) / 4.).tan();
        self.dest.push(Point64::new(
            round64(d.mul_add((-nk.y).mul_add(dx, nk.x), s.x as f64)),
            round64(d.mul_add(nk.x.mul_add(dx, nk.y), s.y as f64)),
        ));
        self.dest.push(Point64::new(
            round64(d.mul_add(nj.y.mul_add(dx, nj.x), s.x as f64)),
            round64(d.mul_add((-nj.x).mul_add(dx, nj.y), s.y as f64)),
        ));
    }

    fn do_miter(&mut self, j: usize, k: usize, r: f64) {
        let (nk, nj, s) = (self.normals[k], self.normals[j], self.src[j]);
        let q = self.delta / r;
        self.dest.push(Point64::new(
            round64((nk.x + nj.x).mul_add(q, s.x as f64)),
            round64((nk.y + nj.y).mul_add(q, s.y as f64)),
        ));
    }

    fn do_round(&mut self, j: usize, k: usize) {
        let (nk, nj, s, d) = (self.normals[k], self.normals[j], self.src[j], self.delta);
        let a = self.sin_a.atan2(nk.x.mul_add(nj.x, nk.y * nj.y));
        let steps = round64(self.steps_per_rad * a.abs()).max(1);
        let (mut x, mut y) = (nk.x, nk.y);
        for _ in 0..steps {
            self.dest.push(Point64::new(
                round64(x.mul_add(d, s.x as f64)),
                round64(y.mul_add(d, s.y as f64)),
            ));
            let x2 = x;
            x = x.mul_add(self.cos, -(self.sin * y));
            y = x2.mul_add(self.sin, y * self.cos);
        }
        self.dest.push(Point64::new(
            round64(nj.x.mul_add(d, s.x as f64)),
            round64(nj.y.mul_add(d, s.y as f64)),
        ));
    }
}

/// Drops the bounding rectangle the negative offset added around its input (6.4 erases the first
/// output path, which is that rectangle).
fn remove_outer_rect(solution: &mut Paths64, l: i64, t: i64, r: i64, b: i64) {
    if let Some(i) = solution.iter().position(|p| {
        p.len() == 4
            && p.iter()
                .all(|q| (q.x == l || q.x == r) && (q.y == t || q.y == b))
    }) {
        solution.remove(i);
    }
}

/// PrusaSlicer's `raw_offset` (ClipperUtils.cpp): every path offset on its own by a fresh
/// `ClipperOffset`, outward for counter-clockwise paths and inward for clockwise ones (whose
/// results are reversed back to clockwise).
pub fn raw_offset64(
    paths: &Paths64,
    delta: f64,
    join: JoinType,
    end: EndType,
    miter_limit: f64,
    arc_tolerance: f64,
) -> Paths64 {
    let mut out = Paths64::new();
    let mut co = ClipperOffset64::new(
        if join == JoinType::Round {
            2.
        } else {
            miter_limit
        },
        if join == JoinType::Round {
            arc_tolerance
        } else {
            0.
        },
        (delta * 0.005).abs(),
    );
    for path in paths {
        co.clear();
        co.add_path(path, join, end);
        let ccw = if end == EndType::Polygon {
            orientation64(path)
        } else {
            true
        };
        let mut this = co.execute(if ccw { delta } else { -delta });
        if !ccw {
            for p in &mut this {
                p.reverse();
            }
        }
        out.extend(this);
    }
    out
}
