// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: prusaslicer@b028299c77 src/libslic3r/Support/OrganicSupport.cpp :: extrude_branch, discretize_circle, triangulate_fan, triangulate_strip
// Source: orca@8500fcdcca src/libslic3r/Support/TreeSupport3D.cpp :: extrude_branch
// Changes: Owned indexed TriangleMesh; defined minimum tessellation for sub-resolution caps; Orca flat bed foot selected by Rules; float vertices in object coordinates.
// Copyright (c) Prusa Research 2023 Vojtěch Bubník

use super::{SupportElements, TreeSlicingParameters, TreeSupportSettings};
use crate::{geometry::SCALE, mesh::TriangleMesh};
use std::{
    f64::consts::{FRAC_PI_2, TAU},
    ops::{Add, Div, Mul, Sub},
};
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct V(pub [f64; 3]);
impl Add for V {
    type Output = Self;
    fn add(self, b: Self) -> Self {
        Self(std::array::from_fn(|i| self.0[i] + b.0[i]))
    }
}
impl Sub for V {
    type Output = Self;
    fn sub(self, b: Self) -> Self {
        Self(std::array::from_fn(|i| self.0[i] - b.0[i]))
    }
}
impl Mul<f64> for V {
    type Output = Self;
    fn mul(self, b: f64) -> Self {
        Self(self.0.map(|v| v * b))
    }
}
impl Div<f64> for V {
    type Output = Self;
    fn div(self, b: f64) -> Self {
        self * (1. / b)
    }
}
impl V {
    pub fn norm2(self) -> f64 {
        self.0.iter().map(|x| x * x).sum()
    }
    pub fn norm(self) -> f64 {
        self.norm2().sqrt()
    }
    pub fn unit(self) -> Self {
        let l = self.norm();
        if l > 0. { self / l } else { Self::default() }
    }
    pub fn cross(self, b: Self) -> Self {
        let [x, y, z] = self.0;
        let [a, b, c] = b.0;
        Self([y * c - z * b, z * a - x * c, x * b - y * a])
    }
    pub fn float(self) -> Self {
        Self(self.0.map(|x| f64::from(x as f32)))
    }
}
pub(super) type NodeId = (usize, usize);
pub(super) fn position(
    bounds: &[SupportElements],
    id: NodeId,
    c: &TreeSupportSettings,
    p: &TreeSlicingParameters,
) -> V {
    let pt = bounds[id.0][id.1]
        .state
        .result_on_layer
        .expect("placed tree node");
    // Object coordinates (upstream's unscaled<float> of the object-frame point).
    V([
        (pt.x - p.origin.x) as f64 / SCALE,
        (pt.y - p.origin.y) as f64 / SCALE,
        c.layer_z(p, id.0),
    ])
}
fn push(m: &mut TriangleMesh, p: V) -> u32 {
    let i = m.vertices.len() as u32;
    m.vertices.push(p.0.map(|x| x as f32));
    i
}
fn vertex(m: &TriangleMesh, i: u32) -> V {
    V(m.vertices[i as usize].map(f64::from))
}
fn triangulate_fan(m: &mut TriangleMesh, center: u32, (begin, end): (u32, u32), flip: bool) {
    let mut u = end - 1;
    for v in begin..end {
        m.faces
            .push(if flip { [center, u, v] } else { [center, v, u] });
        u = v;
    }
}
fn triangulate_strip(m: &mut TriangleMesh, (b1, e1): (u32, u32), (b2, e2): (u32, u32)) {
    let (mut n1, mut n2) = (e1 - b1, e2 - b2);
    let start = (b2..e2)
        .min_by(|&a, &b| {
            (vertex(m, a) - vertex(m, b1))
                .norm2()
                .total_cmp(&(vertex(m, b) - vertex(m, b1)).norm2())
        })
        .unwrap();
    let (mut u, mut v) = (b1, start);
    while n1 > 0 || n2 > 0 {
        let u2 = if u + 1 == e1 { b1 } else { u + 1 };
        let v2 = if v + 1 == e2 { b2 } else { v + 1 };
        let first = if n1 == 0 {
            false
        } else if n2 == 0 {
            true
        } else {
            (vertex(m, u2) - vertex(m, v)).norm2() < (vertex(m, v2) - vertex(m, u)).norm2()
        };
        if first {
            m.faces.push([u, u2, v]);
            n1 -= 1;
            u = u2;
        } else {
            m.faces.push([u, v2, v]);
            n2 -= 1;
            v = v2;
        }
    }
}
fn discretize_circle(m: &mut TriangleMesh, center: V, normal: V, r: f32) -> (u32, u32) {
    let step = (2. * (1. - 0.015_f32 / r).clamp(-1., 1.).acos()) as f64;
    let n = (TAU / step).ceil().max(3.) as usize;
    let step = (TAU / n as f64) as f32;
    let x = normal.cross(V([0., -1., 0.])).unit() * f64::from(r);
    let y = normal.cross(x).unit() * f64::from(r);
    let begin = m.vertices.len() as u32;
    let mut angle = 0_f32;
    for _ in 0..n {
        push(
            m,
            (center + x * f64::from(angle.cos()) + y * f64::from(angle.sin())).float(),
        );
        angle += step;
    }
    (begin, m.vertices.len() as u32)
}
fn cap_steps(radius: f32) -> usize {
    let step = 2. * (1. - 0.015_f32 / radius).clamp(-1., 1.).acos();
    (std::f32::consts::PI / (2. * step)).ceil().max(2.) as usize
}
pub(super) fn extrude_branch(
    bounds: &[SupportElements],
    path: &[NodeId],
    root: bool,
    c: &TreeSupportSettings,
    p: &TreeSlicingParameters,
) -> (TriangleMesh, (f32, f32)) {
    let mut m = TriangleMesh::default();
    let mut previous = None;
    let (mut zmin, mut zmax) = (0., 0.);
    for i in 1..path.len() {
        let a = position(bounds, path[i - 1], c, p);
        let b = position(bounds, path[i], c, p);
        let v = (b - a).unit();
        if i == 1 {
            let r = (bounds[path[0].0][path[0].1].radius(c) as f64 / SCALE) as f32;
            if c.settings.rules.is_orca() && root && path[0].0 == 0 {
                let normal = V([0., 0., 1.]);
                let foot = V([a.0[0], a.0[1], 0.]);
                let center = push(&mut m, foot);
                let bottom = discretize_circle(&mut m, foot, normal, r);
                triangulate_fan(&mut m, center, bottom, false);
                let top = discretize_circle(&mut m, a.float(), normal, r);
                triangulate_strip(&mut m, bottom, top);
                previous = Some(top);
                zmin = 0.;
            } else {
                let steps = cap_steps(r);
                let step = (FRAC_PI_2 / steps as f64) as f32;
                let center = push(&mut m, a - v * f64::from(r));
                zmin = m.vertices[center as usize][2];
                let mut angle = step;
                for _ in 1..steps {
                    let strip = discretize_circle(
                        &mut m,
                        (a - v * f64::from(r * angle.cos())).float(),
                        v.float(),
                        r * angle.sin(),
                    );
                    if let Some(prev) = previous {
                        triangulate_strip(&mut m, prev, strip);
                    } else {
                        triangulate_fan(&mut m, center, strip, false);
                    }
                    previous = Some(strip);
                    angle += step;
                }
            }
        }
        if i + 1 == path.len() {
            let r = (bounds[path[i].0][path[i].1].radius(c) as f64 / SCALE) as f32;
            let steps = cap_steps(r);
            let step = (FRAC_PI_2 / steps as f64) as f32;
            let mut angle = std::f32::consts::FRAC_PI_2;
            for _ in 0..steps {
                let strip = discretize_circle(
                    &mut m,
                    (b + v * f64::from(r * angle.cos())).float(),
                    v.float(),
                    r * angle.sin(),
                );
                triangulate_strip(&mut m, previous.unwrap(), strip);
                previous = Some(strip);
                angle -= step;
            }
            let center = push(&mut m, b + v * f64::from(r));
            zmax = m.vertices[center as usize][2];
            triangulate_fan(&mut m, center, previous.unwrap(), true);
        } else {
            let next = position(bounds, path[i + 1], c, p);
            let normal = (v + (next - b).unit()).unit();
            let r = (bounds[path[i].0][path[i].1].radius(c) as f64 / SCALE) as f32;
            let strip = discretize_circle(&mut m, b.float(), normal.float(), r);
            triangulate_strip(&mut m, previous.unwrap(), strip);
            previous = Some(strip);
        }
    }
    (m, (zmin, zmax))
}
