// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/ModelVisibility.cpp :: Visibility, raycast_visibility, calculate_point_visibility
// Source: prusaslicer@b028299c77 src/libslic3r/TriangleSetSampling.cpp :: sample_its_uniform_parallel
// Changes: Borrowed full mesh, independent median BVH and spatial sample grid; no decimation or negative-volume boolean evaluation.
// Changes: Retain seeded MT19937-64 area sampling, 30,000 samples, 25 hemisphere rays and upstream interpolation weights.
// Changes: Leaf triangles are stored contiguously in leaf order; traversal order and arithmetic unchanged.
// Changes: Kept for Orca profiles; Prusa profiles use the exact decimated port in model_visibility.rs through PointVisibility.
// Copyright (c) Prusa Research 2022 Pavel Mikuš @Godrak (upstream sampling).

use crate::mesh::TriangleMesh;
use rayon::prelude::*;
use std::collections::HashMap;

pub(super) type Vec3 = [f64; 3];
pub(super) fn add(a: Vec3, b: Vec3) -> Vec3 {
    std::array::from_fn(|i| a[i] + b[i])
}
pub(super) fn sub(a: Vec3, b: Vec3) -> Vec3 {
    std::array::from_fn(|i| a[i] - b[i])
}
pub(super) fn mul(a: Vec3, b: f64) -> Vec3 {
    a.map(|x| x * b)
}
pub(super) fn dot(a: Vec3, b: Vec3) -> f64 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}
pub(super) fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
pub(super) fn norm(a: Vec3) -> f64 {
    dot(a, a).sqrt()
}
pub(super) fn unit(a: Vec3) -> Vec3 {
    let n = norm(a);
    if n > 1e-15 { mul(a, 1.0 / n) } else { [0.0; 3] }
}
pub(super) fn triangle(mesh: &TriangleMesh, face: usize) -> [Vec3; 3] {
    mesh.faces[face].map(|v| mesh.vertices[v as usize].map(f64::from))
}

struct Node {
    min: Vec3,
    max: Vec3,
    start: usize,
    end: usize,
    children: Option<(usize, usize)>,
}
/// Independent BVH implementation over face indices. After the build, the triangles are also
/// copied in leaf order (`tris[k]` is face `faces[k]`, converted to f64 exactly as `triangle`
/// does), so a leaf's ray tests read one contiguous block instead of three vertex lookups per face.
pub(super) struct MeshTree<'a> {
    mesh: &'a TriangleMesh,
    faces: Vec<usize>,
    tris: Vec<[Vec3; 3]>,
    nodes: Vec<Node>,
}
impl<'a> MeshTree<'a> {
    pub fn new(mesh: &'a TriangleMesh) -> Self {
        let mut tree = Self {
            mesh,
            faces: (0..mesh.faces.len()).collect(),
            tris: Vec::new(),
            nodes: Vec::new(),
        };
        if !tree.faces.is_empty() {
            tree.build(0, tree.faces.len());
        }
        tree.tris = tree.faces.iter().map(|&f| triangle(mesh, f)).collect();
        tree
    }
    fn build(&mut self, start: usize, end: usize) -> usize {
        let mut min = [f64::INFINITY; 3];
        let mut max = [f64::NEG_INFINITY; 3];
        for &f in &self.faces[start..end] {
            for p in triangle(self.mesh, f) {
                for k in 0..3 {
                    min[k] = min[k].min(p[k]);
                    max[k] = max[k].max(p[k]);
                }
            }
        }
        let index = self.nodes.len();
        self.nodes.push(Node {
            min,
            max,
            start,
            end,
            children: None,
        });
        if end - start > 8 {
            let axis = (0..3)
                .max_by(|&a, &b| (max[a] - min[a]).total_cmp(&(max[b] - min[b])))
                .unwrap();
            let mid = (start + end) / 2;
            let center = |f: usize| triangle(self.mesh, f).iter().map(|p| p[axis]).sum::<f64>();
            self.faces[start..end].select_nth_unstable_by(mid - start, |&a, &b| {
                center(a).total_cmp(&center(b)).then(a.cmp(&b))
            });
            let left = self.build(start, mid);
            let right = self.build(mid, end);
            self.nodes[index].children = Some((left, right));
        }
        index
    }
    fn hit_box(n: &Node, o: Vec3, d: Vec3, max_distance: f64) -> bool {
        let mut lo: f64 = 0.0;
        let mut hi = max_distance;
        for k in 0..3 {
            if d[k].abs() < 1e-14 {
                if o[k] < n.min[k] || o[k] > n.max[k] {
                    return false;
                }
            } else {
                let a = (n.min[k] - o[k]) / d[k];
                let b = (n.max[k] - o[k]) / d[k];
                lo = lo.max(a.min(b));
                hi = hi.min(a.max(b));
                if hi < lo {
                    return false;
                }
            }
        }
        true
    }
    fn ray_triangle([a, b, c]: &[Vec3; 3], o: Vec3, d: Vec3) -> Option<f64> {
        let (a, b, c) = (*a, *b, *c);
        let e1 = sub(b, a);
        let e2 = sub(c, a);
        let h = cross(d, e2);
        let det = dot(e1, h);
        if det.abs() < 1e-12 {
            return None;
        }
        let s = sub(o, a);
        let u = dot(s, h) / det;
        if !(0.0..=1.0).contains(&u) {
            return None;
        }
        let q = cross(s, e1);
        let v = dot(d, q) / det;
        if v < 0.0 || u + v > 1.0 {
            return None;
        }
        let t = dot(e2, q) / det;
        (t > 1e-8).then_some(t)
    }
    pub fn occluded(&self, o: Vec3, d: Vec3) -> bool {
        if self.nodes.is_empty() {
            return false;
        }
        // Same right-first depth-first order as a `Vec` stack; the tree is at most a few dozen
        // levels deep, so a fixed array holds it (the median split halves every inner node).
        let mut stack = [0usize; 128];
        let mut len = 1;
        let mut distance = f64::INFINITY;
        let mut face = None;
        while len > 0 {
            len -= 1;
            let n = &self.nodes[stack[len]];
            if !Self::hit_box(n, o, d, distance) {
                continue;
            }
            if let Some((l, r)) = n.children {
                stack[len] = l;
                stack[len + 1] = r;
                len += 2;
            } else {
                for (k, tri) in self.tris[n.start..n.end].iter().enumerate() {
                    if let Some(t) = Self::ray_triangle(tri, o, d)
                        && t < distance
                    {
                        distance = t;
                        face = Some(n.start + k);
                    }
                }
            }
        }
        face.is_some_and(|k| {
            let [a, b, c] = self.tris[k];
            dot(cross(sub(b, a), sub(c, a)), d) <= 0.0
        })
    }
    pub fn in_radius(&self, p: Vec3, r: f64) -> bool {
        if self.nodes.is_empty() {
            return false;
        }
        let mut stack = vec![0];
        while let Some(i) = stack.pop() {
            let n = &self.nodes[i];
            let d = (0..3)
                .map(|k| (p[k] - p[k].clamp(n.min[k], n.max[k])).powi(2))
                .sum::<f64>();
            if d > r * r {
                continue;
            }
            if let Some((l, r)) = n.children {
                stack.push(l);
                stack.push(r);
            } else {
                for &tri in &self.tris[n.start..n.end] {
                    if triangle_distance_sq(p, tri) <= r * r {
                        return true;
                    }
                }
            }
        }
        false
    }
}
// Closest point on a triangle by plane projection plus three edge projections; also handles degenerate facets.
fn triangle_distance_sq(p: Vec3, [a, b, c]: [Vec3; 3]) -> f64 {
    let n = cross(sub(b, a), sub(c, a));
    let n2 = dot(n, n);
    if n2 > 1e-20 {
        let q = sub(p, mul(n, dot(sub(p, a), n) / n2));
        if [(a, b), (b, c), (c, a)]
            .iter()
            .all(|&(u, v)| dot(cross(sub(v, u), sub(q, u)), n) >= -1e-12)
        {
            return dot(sub(p, q), sub(p, q));
        }
    }
    [(a, b), (b, c), (c, a)]
        .iter()
        .map(|&(u, v)| {
            let e = sub(v, u);
            let l = dot(e, e);
            let t = if l > 0.0 {
                (dot(sub(p, u), e) / l).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let d = sub(p, add(u, mul(e, t)));
            dot(d, d)
        })
        .fold(f64::INFINITY, f64::min)
}

/// Interpolated mesh visibility at a bed-frame point (mm), 1 = fully visible.
pub(super) trait PointVisibility: Sync {
    fn at(&self, p: Vec3) -> f64;
}
impl PointVisibility for Visibility {
    fn at(&self, p: Vec3) -> f64 {
        Visibility::at(self, p)
    }
}
impl PointVisibility for super::model_visibility::ModelVisibility {
    fn at(&self, p: Vec3) -> f64 {
        super::model_visibility::ModelVisibility::at(self, p)
    }
}

struct Sample {
    p: Vec3,
    n: Vec3,
    visibility: f64,
}
pub(super) struct Visibility {
    samples: Vec<Sample>,
    grid: HashMap<[i64; 3], Vec<usize>>,
    radius: f64,
}
impl Visibility {
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn new(mesh: &TriangleMesh) -> Self {
        Self::new_observed(mesh, &crate::progress::Silent)
    }

    /// [`Self::new`] reporting the ray casting as 0.5 .. 0.7 of the seam stage; samples after
    /// a cancel are skipped (the caller discards the plan).
    pub fn new_observed(mesh: &TriangleMesh, observer: &dyn crate::progress::Observer) -> Self {
        const COUNT: usize = 30_000;
        let ticker =
            crate::progress::Ticker::span(observer, crate::progress::Stage::Seams, COUNT, 0.5, 0.7);
        let mut total = 0.0;
        let areas: Vec<f64> = (0..mesh.faces.len())
            .map(|i| {
                let [a, b, c] = triangle(mesh, i);
                total += 0.5 * norm(cross(sub(b, a), sub(c, a)));
                total
            })
            .collect();
        if total <= 0.0 {
            return Self {
                samples: Vec::new(),
                grid: HashMap::new(),
                radius: 1.0,
            };
        }
        let mut random = super::random::Mt19937::new(27_644_437);
        let random_samples: Vec<[f64; 3]> = (0..COUNT)
            .map(|_| [random.unit(), random.unit(), random.unit()])
            .collect();
        let tree = MeshTree::new(mesh);
        let directions: Vec<Vec3> = (0..5)
            .flat_map(|x| {
                (0..5).map(move |y| {
                    let u = (x as f64 + 0.5) / 5.0;
                    let v = (y as f64 + 0.5) / 5.0;
                    let phi = std::f64::consts::TAU * u;
                    let r = 2.0 * (v - v * v).sqrt();
                    [phi.cos() * r, phi.sin() * r, (1.0 - 2.0 * v).abs()]
                })
            })
            .collect();
        let samples: Vec<Sample> = (0..COUNT)
            .into_par_iter()
            .map(|i| {
                ticker.tick();
                if i.is_multiple_of(256) && ticker.cancelled() {
                    return Sample {
                        p: [0.0; 3],
                        n: [0.0, 0.0, 1.0],
                        visibility: 1.0,
                    };
                }
                let t = random_samples[i][0] * total;
                let f = areas.partition_point(|&x| x <= t).min(areas.len() - 1);
                let u = random_samples[i][1].sqrt();
                let v = random_samples[i][2];
                let [a, b, c] = triangle(mesh, f);
                let p = add(add(mul(a, 1.0 - u), mul(b, u * (1.0 - v))), mul(c, u * v));
                let n = unit(cross(sub(b, a), sub(c, b)));
                let temp = if n[0].abs() > 0.99 {
                    [0.0, 1.0, 0.0]
                } else {
                    [1.0, 0.0, 0.0]
                };
                let y = unit(cross(n, temp));
                let x = cross(y, n);
                let o = add(p, mul(n, 0.01));
                let hits = directions
                    .iter()
                    .filter(|&&d| {
                        tree.occluded(o, add(add(mul(x, d[0]), mul(y, d[1])), mul(n, d[2])))
                    })
                    .count();
                Sample {
                    p,
                    n,
                    visibility: 1.0 - hits as f64 / 25.0,
                }
            })
            .collect();
        let radius = (4.0 / (-0.9f64.ln() * (COUNT as f64 / total) * std::f64::consts::PI))
            .sqrt()
            .max(1e-6);
        let mut result = Self {
            samples,
            grid: HashMap::new(),
            radius,
        };
        for (i, s) in result.samples.iter().enumerate() {
            result.grid.entry(result.cell(s.p)).or_default().push(i);
        }
        result
    }
    fn cell(&self, p: Vec3) -> [i64; 3] {
        p.map(|v| (v / self.radius).floor() as i64)
    }
    pub fn at(&self, p: Vec3) -> f64 {
        let cell = self.cell(p);
        let mut weight = 0.0;
        let mut visibility = 0.0;
        for x in -1..=1 {
            for y in -1..=1 {
                for z in -1..=1 {
                    if let Some(ids) = self.grid.get(&[cell[0] + x, cell[1] + y, cell[2] + z]) {
                        for &i in ids {
                            let s = &self.samples[i];
                            let d = sub(p, s.p);
                            let dist = norm(d);
                            if dist > self.radius {
                                continue;
                            }
                            let w = 2.0 * self.radius - dot(d, s.n).abs() - dist;
                            weight += w;
                            visibility += w * s.visibility;
                        }
                    }
                }
            }
        }
        if weight > 1e-12 {
            visibility / weight
        } else {
            1.0
        }
    }
}
