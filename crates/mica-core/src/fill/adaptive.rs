// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/Fill/FillAdaptive.cpp :: build_octree, Octree::insert_triangle, generate_infill_lines_recursive, make_cubes_properties
// Changes: Indexed arena instead of a pointer pool; SAT axis loops; immutable per-object octree.
// Changes: Boundary anchoring reuses Mica's graph; internal T-joint hook generation is not yet ported.
// Copyright (c) Prusa Research 2020 - 2022 Vojtěch Bubník, Lukáš Matěna, Lukáš Hejl.
// Adaptive cubic inspired upstream by mboerwinkle's Cura SubDivCube.

use crate::{
    geometry::{Point, Polyline, SCALE},
    mesh::TriangleMesh,
};
type V = [f64; 3];
fn add(a: V, b: V) -> V {
    std::array::from_fn(|i| a[i] + b[i])
}
fn sub(a: V, b: V) -> V {
    std::array::from_fn(|i| a[i] - b[i])
}
fn mul(a: V, b: f64) -> V {
    a.map(|v| v * b)
}
fn dot(a: V, b: V) -> f64 {
    (0..3).map(|i| a[i] * b[i]).sum()
}
fn cross(a: V, b: V) -> V {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn rotate(mut p: V, inverse: bool) -> V {
    let angles = [
        5. * std::f64::consts::PI / 4.,
        215.264f64.to_radians(),
        std::f64::consts::PI / 6.,
    ];
    for k in 0..3 {
        let axis = if inverse { 2 - k } else { k };
        let (s, c) = (angles[axis] * if inverse { -1. } else { 1. }).sin_cos();
        let i = (axis + 1) % 3;
        let j = (axis + 2) % 3;
        let (a, b) = (p[i], p[j]);
        p[i] = c * a - s * b;
        p[j] = s * a + c * b;
    }
    p
}
fn intersects(t: [V; 3], min: V, max: V) -> bool {
    let center = mul(add(min, max), 0.5);
    let half = mul(sub(max, min), 0.5);
    let t = t.map(|p| sub(p, center));
    let edges = [sub(t[1], t[0]), sub(t[2], t[0]), sub(t[2], t[1])];
    let axes = [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]];
    let separated = |axis: V| {
        let r: f64 = (0..3).map(|i| half[i] * axis[i].abs()).sum();
        let projections = t.map(|p| dot(p, axis));
        let lo = projections.into_iter().fold(f64::INFINITY, f64::min);
        let hi = projections.into_iter().fold(f64::NEG_INFINITY, f64::max);
        lo > r || hi < -r
    };
    !axes.into_iter().any(separated)
        && !separated(cross(edges[0], edges[1]))
        && !edges
            .into_iter()
            .any(|e| axes.into_iter().any(|axis| separated(cross(axis, e))))
}
#[derive(Clone, Debug)]
struct Cube {
    center: V,
    children: [u32; 8],
}
const NONE: u32 = u32::MAX;
/// A mesh-adaptive hierarchy shared by all layer workers. Only intersected child cells are allocated.
#[derive(Clone, Debug)]
pub struct Octree {
    cubes: Vec<Cube>,
    edges: Vec<f64>,
}
impl Octree {
    pub fn build(
        mesh: &TriangleMesh,
        origin: Point,
        line_spacing: f64,
        support_only: bool,
    ) -> Self {
        assert!(
            line_spacing.is_finite() && line_spacing > 0.,
            "adaptive spacing must be finite and positive"
        );
        if mesh.vertices.is_empty() {
            return Self {
                cubes: Vec::new(),
                edges: Vec::new(),
            };
        }
        let o = [origin.x as f64 / SCALE, origin.y as f64 / SCALE, 0.];
        // Upstream indexed_triangle_set stores transformed mesh vertices as floats.
        let vertices: Vec<V> = mesh
            .vertices
            .iter()
            .map(|v| rotate(sub(v.map(f64::from), o), true).map(|x| f64::from(x as f32)))
            .collect();
        let mut min = [f64::INFINITY; 3];
        let mut max = [f64::NEG_INFINITY; 3];
        for v in &vertices {
            for i in 0..3 {
                min[i] = min[i].min(v[i]);
                max[i] = max[i].max(v[i]);
            }
        }
        let size = sub(max, min).into_iter().fold(0., f64::max) + 0.0001;
        let mut edges = vec![line_spacing * 2.];
        while *edges.last().unwrap() <= size {
            edges.push(edges.last().unwrap() * 2.);
        }
        let center = mul(add(min, max), 0.5);
        let mut tree = Self {
            cubes: vec![Cube {
                center,
                children: [NONE; 8],
            }],
            edges,
        };
        let depth = tree.edges.len() - 1;
        if depth > 0 {
            let half = [tree.edges[depth] / 2.; 3];
            let up = rotate([0., 0., 1.], true);
            for f in &mesh.faces {
                let t = f.map(|i| vertices[i as usize]);
                let n = cross(sub(t[1], t[0]), sub(t[2], t[1]));
                if !support_only || dot(n, up) > 0.707 * dot(n, n).sqrt() {
                    tree.insert(t, 0, sub(center, half), add(center, half), depth);
                }
            }
        }
        for cell in &mut tree.cubes {
            cell.center = rotate(cell.center, false);
        }
        tree
    }
    pub fn node_count(&self) -> usize {
        self.cubes.len()
    }
    fn insert(&mut self, t: [V; 3], node: usize, min: V, max: V, depth: usize) {
        let center = self.cubes[node].center;
        for i in 0..8 {
            let negative = |k| i & (1usize << k) == 0;
            let lo = std::array::from_fn(|k| {
                if negative(k) {
                    min[k]
                } else {
                    center[k] - 0.0001
                }
            });
            let hi = std::array::from_fn(|k| {
                if negative(k) {
                    center[k] + 0.0001
                } else {
                    max[k]
                }
            });
            if !intersects(t, lo, hi) {
                continue;
            }
            let mut child = self.cubes[node].children[i];
            if child == NONE {
                child = self.cubes.len() as u32;
                self.cubes[node].children[i] = child;
                self.cubes.push(Cube {
                    center: std::array::from_fn(|k| {
                        center[k]
                            + if negative(k) {
                                -self.edges[depth - 1] / 2.
                            } else {
                                self.edges[depth - 1] / 2.
                            }
                    }),
                    children: [NONE; 8],
                });
            }
            if depth > 1 {
                self.insert(t, child as usize, lo, hi, depth - 1);
            }
        }
    }
    pub(super) fn lines(&self, z: f64) -> Vec<Polyline> {
        let mut out = Vec::new();
        if self.cubes.is_empty() {
            return out;
        }
        for direction in 0..3 {
            let mut pending = vec![None; (1 << self.edges.len()) - 1];
            self.visit(
                0,
                self.edges.len() - 1,
                0,
                z,
                direction,
                &mut pending,
                &mut out,
            );
            out.extend(pending.into_iter().flatten().map(|[a, b]| vec![a, b]));
        }
        out
    }
    #[allow(clippy::too_many_arguments)]
    fn visit(
        &self,
        node: usize,
        depth: usize,
        address: usize,
        z: f64,
        dir: usize,
        pending: &mut [Option<[Point; 2]>],
        out: &mut Vec<Polyline>,
    ) {
        const ORDER: [[usize; 8]; 3] = [
            [2, 3, 0, 1, 6, 7, 4, 5],
            [4, 0, 6, 2, 5, 1, 7, 3],
            [1, 5, 0, 4, 3, 7, 2, 6],
        ];
        let c = &self.cubes[node];
        let edge = self.edges[depth];
        let dz = z - c.center[2];
        if dz.abs() > edge * 3f64.sqrt() / 2. {
            return;
        }
        let zdist = edge / 3f64.sqrt();
        if dz.abs() < zdist {
            let x = 0.5 * edge * 2f64.sqrt() * (zdist - dz.abs()) / zdist;
            let y = edge / 6f64.sqrt() - (zdist + dz) / 2f64.sqrt();
            let angle = [0., std::f64::consts::TAU / 3., -std::f64::consts::TAU / 3.][dir];
            let (s, co) = angle.sin_cos();
            let point =
                |x: f64| Point::from_mm(co * x - s * y + c.center[0], s * x + co * y + c.center[1]);
            let line = [point(x), point(-x)];
            if let Some(last) = &mut pending[address] {
                if (last[1].x - line[0].x)
                    .abs()
                    .max((last[1].y - line[0].y).abs())
                    > 1000
                {
                    out.push(last.to_vec());
                    *last = line;
                } else {
                    last[1] = line[1];
                }
            } else {
                pending[address] = Some(line);
            }
        }
        if depth > 0 {
            for (i, &k) in ORDER[dir].iter().enumerate() {
                let child = c.children[k];
                if child != NONE {
                    self.visit(
                        child as usize,
                        depth - 1,
                        address * 2 + 1 + i / 4,
                        z,
                        dir,
                        pending,
                        out,
                    );
                }
            }
        }
    }
}
