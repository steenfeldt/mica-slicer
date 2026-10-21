// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: bumpmesh@a6d6d6e6df js/displacement.js :: applyDisplacementLayers, _buildFalloffField, _creaseMoves
// Changes: Texture on the outer walls while slicing instead of moving mesh vertices: each wall point is moved in the slice plane along the wall's outward normal by the projected grey value; the 3-D normal comes from the facet that made the nearest contour segment (smooth across shallow edges, split at creases like BumpMesh's 30° normal groups); angle masks fade the texture on flat and overhanging surfaces; corners are mitred like BumpMesh's crease moves, crossing points at inner corners are cut out; the soft border of painted areas is a distance to the paint edge at slice time, with BumpMesh's falloff curves.
// Copyright (c) 2026 CNCKitchen (Stefan Hermann) and contributors (BumpMesh, AGPL-3.0-only)

//! Texture skin: the outer walls carry a grey height map, laid onto the part by the
//! [`crate::texture`] projections. Like fuzzy skin it runs before seam placement and Arachne's
//! width split, so seams and overhang detection see the textured wall. Off unless
//! `mica_texture_skin` is set or a part has texture paint.
use crate::{
    arachne::{ExtrusionLine, Junction},
    flow::Flow,
    geometry::{Point, Polygon, SCALE, signed_area},
    mesh::TriangleMesh,
    texture::{
        Direction, Mode, TextureSettings,
        heightmap::HeightMap,
        projection::{Bounds, Projector},
    },
};
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

// ---------------------------------------------------------------------------------------------
// Surface normals: the facet behind each contour segment, per layer.

/// A slice segment with the surface normals at its ends.
#[derive(Clone, Copy)]
struct Seg {
    a: [f64; 2],
    b: [f64; 2],
    na: [f32; 3],
    nb: [f32; 3],
    /// The facet's outward direction in the slice plane.
    out: [f64; 2],
    /// A steep wall ([`crate::texture::displace::WALL_SIN`]).
    vertical: bool,
}

/// One layer's segments in a uniform grid.
struct LayerIndex {
    segs: Vec<Seg>,
    min: [f64; 2],
    cell: f64,
    nx: usize,
    ny: usize,
    starts: Vec<u32>,
    items: Vec<u32>,
}
impl LayerIndex {
    fn new(segs: Vec<Seg>, cell: f64) -> Self {
        let mut min = [f64::INFINITY; 2];
        let mut max = [f64::NEG_INFINITY; 2];
        for s in &segs {
            for p in [s.a, s.b] {
                for k in 0..2 {
                    min[k] = min[k].min(p[k]);
                    max[k] = max[k].max(p[k]);
                }
            }
        }
        if segs.is_empty() {
            return Self {
                segs,
                min: [0.; 2],
                cell,
                nx: 0,
                ny: 0,
                starts: vec![0],
                items: Vec::new(),
            };
        }
        let nx = (((max[0] - min[0]) / cell).floor() as usize + 1).min(4096);
        let ny = (((max[1] - min[1]) / cell).floor() as usize + 1).min(4096);
        let cell_of = |p: [f64; 2]| {
            (
                (((p[0] - min[0]) / cell) as usize).min(nx - 1),
                (((p[1] - min[1]) / cell) as usize).min(ny - 1),
            )
        };
        let mut counts = vec![0u32; nx * ny + 1];
        let ranges: Vec<_> = segs
            .iter()
            .map(|s| {
                let (x0, y0) = cell_of([s.a[0].min(s.b[0]), s.a[1].min(s.b[1])]);
                let (x1, y1) = cell_of([s.a[0].max(s.b[0]), s.a[1].max(s.b[1])]);
                (x0, y0, x1, y1)
            })
            .collect();
        for &(x0, y0, x1, y1) in &ranges {
            for y in y0..=y1 {
                for x in x0..=x1 {
                    counts[y * nx + x + 1] += 1;
                }
            }
        }
        for i in 1..counts.len() {
            counts[i] += counts[i - 1];
        }
        let mut fill = counts.clone();
        let mut items = vec![0u32; *counts.last().unwrap() as usize];
        for (i, &(x0, y0, x1, y1)) in ranges.iter().enumerate() {
            for y in y0..=y1 {
                for x in x0..=x1 {
                    let c = &mut fill[y * nx + x];
                    items[*c as usize] = i as u32;
                    *c += 1;
                }
            }
        }
        Self {
            segs,
            min,
            cell,
            nx,
            ny,
            starts: counts,
            items,
        }
    }

    /// Normal of the nearest segment facing the same way as `out` (so the far side of a thin
    /// wall is left out), interpolated along it.
    #[cfg(test)]
    fn normal(&self, p: [f64; 2], out: [f64; 2], radius: f64) -> Option<[f64; 3]> {
        self.nearest(p, out, radius).map(|n| n.normal)
    }

    /// [`Self::normal`] with the nearest point of the segment and whether its facet is a steep
    /// wall.
    fn nearest(&self, p: [f64; 2], out: [f64; 2], radius: f64) -> Option<Nearest> {
        if self.nx == 0 {
            return None;
        }
        let reach = (radius / self.cell).ceil() as i64;
        let cx = ((p[0] - self.min[0]) / self.cell).floor() as i64;
        let cy = ((p[1] - self.min[1]) / self.cell).floor() as i64;
        let mut best: Option<(f64, usize, f64)> = None;
        for y in (cy - reach).max(0)..=(cy + reach).min(self.ny as i64 - 1) {
            for x in (cx - reach).max(0)..=(cx + reach).min(self.nx as i64 - 1) {
                let c = y as usize * self.nx + x as usize;
                for &i in &self.items[self.starts[c] as usize..self.starts[c + 1] as usize] {
                    let s = &self.segs[i as usize];
                    if s.out[0] * out[0] + s.out[1] * out[1] < 0.3 {
                        continue;
                    }
                    let (dx, dy) = (s.b[0] - s.a[0], s.b[1] - s.a[1]);
                    let l2 = dx * dx + dy * dy;
                    let t = if l2 > 0. {
                        (((p[0] - s.a[0]) * dx + (p[1] - s.a[1]) * dy) / l2).clamp(0., 1.)
                    } else {
                        0.
                    };
                    let (qx, qy) = (s.a[0] + dx * t - p[0], s.a[1] + dy * t - p[1]);
                    let d = qx * qx + qy * qy;
                    if d <= radius * radius && best.is_none_or(|b| d < b.0) {
                        best = Some((d, i as usize, t));
                    }
                }
            }
        }
        let (_, i, t) = best?;
        let s = &self.segs[i];
        let n: [f64; 3] =
            std::array::from_fn(|k| f64::from(s.na[k]) * (1. - t) + f64::from(s.nb[k]) * t);
        let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        (l > 1e-9).then(|| Nearest {
            normal: n.map(|c| c / l),
            foot: [
                s.a[0] + (s.b[0] - s.a[0]) * t,
                s.a[1] + (s.b[1] - s.a[1]) * t,
            ],
            vertical: s.vertical,
        })
    }
}

/// The surface behind a wall point: its normal, the point on it, and whether it is a steep wall.
struct Nearest {
    normal: [f64; 3],
    foot: [f64; 2],
    vertical: bool,
}

/// Facets with smooth corner normals and the layers each one crosses.
struct SurfaceNormals {
    tris: Vec<[[f32; 3]; 3]>,
    corners: Vec<[[f32; 3]; 3]>,
    /// Segment tree over layer indices: node → facets spanning it.
    tree: Vec<Vec<u32>>,
    size: usize,
    zs: Vec<f64>,
    layers: Vec<OnceLock<LayerIndex>>,
}

/// Facets meeting at a sharper angle than this keep separate normals (BumpMesh's normal groups).
const CREASE_COS: f32 = 0.866;

impl SurfaceNormals {
    fn new(mesh: &TriangleMesh, zs: &[f64]) -> Self {
        // Weld by position, so smoothing works on meshes stored as separate triangles.
        let mut ids: HashMap<[i64; 3], u32> = HashMap::new();
        let welded: Vec<u32> = mesh
            .vertices
            .iter()
            .map(|v| {
                let key = v.map(|c| (f64::from(c) * 1e4).round() as i64);
                let n = ids.len() as u32;
                *ids.entry(key).or_insert(n)
            })
            .collect();
        let mut tris = Vec::with_capacity(mesh.faces.len());
        let mut normals = Vec::with_capacity(mesh.faces.len());
        let mut corner_ids = Vec::with_capacity(mesh.faces.len());
        for f in &mesh.faces {
            let t = f.map(|i| mesh.vertices[i as usize]);
            let n = cross(sub(t[1], t[0]), sub(t[2], t[0]));
            if dot(n, n) <= 1e-20 {
                continue;
            }
            tris.push(t);
            normals.push(n);
            corner_ids.push(f.map(|i| welded[i as usize]));
        }
        // Vertex → facets.
        let mut start = vec![0u32; ids.len() + 1];
        for c in &corner_ids {
            for &v in c {
                start[v as usize + 1] += 1;
            }
        }
        for i in 1..start.len() {
            start[i] += start[i - 1];
        }
        let mut fill = start.clone();
        let mut around = vec![0u32; *start.last().unwrap() as usize];
        for (f, c) in corner_ids.iter().enumerate() {
            for &v in c {
                around[fill[v as usize] as usize] = f as u32;
                fill[v as usize] += 1;
            }
        }
        let unit = |n: [f32; 3]| {
            let l = dot(n, n).sqrt();
            n.map(|c| c / l)
        };
        let corners = corner_ids
            .iter()
            .enumerate()
            .map(|(f, c)| {
                let own = unit(normals[f]);
                c.map(|v| {
                    let mut acc = [0f32; 3];
                    for &g in &around[start[v as usize] as usize..start[v as usize + 1] as usize] {
                        let n = normals[g as usize];
                        if dot(unit(n), own) >= CREASE_COS {
                            // Area weighted: the cross product's length is twice the area.
                            acc = [acc[0] + n[0], acc[1] + n[1], acc[2] + n[2]];
                        }
                    }
                    let l = dot(acc, acc).sqrt();
                    if l > 1e-20 { acc.map(|x| x / l) } else { own }
                })
            })
            .collect();
        let size = zs.len().next_power_of_two().max(1);
        let mut tree = vec![Vec::new(); 2 * size];
        for (f, t) in tris.iter().enumerate() {
            let lo = t
                .iter()
                .map(|p| f64::from(p[2]))
                .fold(f64::INFINITY, f64::min);
            let hi = t
                .iter()
                .map(|p| f64::from(p[2]))
                .fold(f64::NEG_INFINITY, f64::max);
            let first = zs.partition_point(|&z| z < lo);
            let last = zs.partition_point(|&z| z <= hi);
            if first >= last {
                continue;
            }
            let (mut l, mut r) = (first + size, last + size);
            while l < r {
                if l & 1 == 1 {
                    tree[l].push(f as u32);
                    l += 1;
                }
                if r & 1 == 1 {
                    r -= 1;
                    tree[r].push(f as u32);
                }
                l >>= 1;
                r >>= 1;
            }
        }
        Self {
            tris,
            corners,
            tree,
            size,
            zs: zs.to_vec(),
            layers: (0..zs.len()).map(|_| OnceLock::new()).collect(),
        }
    }

    fn layer(&self, i: usize) -> Option<&LayerIndex> {
        let z = *self.zs.get(i)?;
        Some(self.layers[i].get_or_init(|| {
            let mut segs = Vec::new();
            let mut node = i + self.size;
            while node >= 1 {
                for &f in &self.tree[node] {
                    if let Some(s) = self.cut(f as usize, z) {
                        segs.push(s);
                    }
                }
                node >>= 1;
            }
            LayerIndex::new(segs, 1.)
        }))
    }

    /// Where facet `f` crosses the plane at `z`.
    fn cut(&self, f: usize, z: f64) -> Option<Seg> {
        let t = &self.tris[f];
        let c = &self.corners[f];
        let mut pts = [([0f64; 2], [0f32; 3]); 2];
        let mut n = 0;
        for (i, j) in [(0, 1), (1, 2), (2, 0)] {
            let (za, zb) = (f64::from(t[i][2]), f64::from(t[j][2]));
            if (za < z) == (zb < z) || n == 2 {
                continue;
            }
            let s = (z - za) / (zb - za);
            let p = [
                f64::from(t[i][0]) + (f64::from(t[j][0]) - f64::from(t[i][0])) * s,
                f64::from(t[i][1]) + (f64::from(t[j][1]) - f64::from(t[i][1])) * s,
            ];
            let sf = s as f32;
            let nn = std::array::from_fn(|k| c[i][k] * (1. - sf) + c[j][k] * sf);
            pts[n] = (p, nn);
            n += 1;
        }
        if n < 2 {
            return None;
        }
        let fn_ = cross(sub(t[1], t[0]), sub(t[2], t[0]));
        let l = f64::from(fn_[0]).hypot(f64::from(fn_[1]));
        // As the mesh displacement decides it (f64 from the same vertices).
        let vertical = {
            let p = t.map(|v| v.map(f64::from));
            let (u, v) = (
                [p[1][0] - p[0][0], p[1][1] - p[0][1], p[1][2] - p[0][2]],
                [p[2][0] - p[0][0], p[2][1] - p[0][1], p[2][2] - p[0][2]],
            );
            let c = [
                u[1] * v[2] - u[2] * v[1],
                u[2] * v[0] - u[0] * v[2],
                u[0] * v[1] - u[1] * v[0],
            ];
            let len = (c[0] * c[0] + c[1] * c[1] + c[2] * c[2]).sqrt();
            len > 1e-30 && (c[2] * (1. / len)).abs() < crate::texture::displace::WALL_SIN
        };
        let out = if l > 1e-12 {
            [f64::from(fn_[0]) / l, f64::from(fn_[1]) / l]
        } else {
            [0., 0.]
        };
        Some(Seg {
            a: pts[0].0,
            b: pts[1].0,
            na: pts[0].1,
            nb: pts[1].1,
            out,
            vertical,
        })
    }
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

// ---------------------------------------------------------------------------------------------
// Texture paint: which slot covers a point, and how far it is from the paint's edge.

/// A BVH node: bounds, the leaf range it covers and its children.
type LeafNode = (([f64; 3], [f64; 3]), usize, usize, Option<(usize, usize)>);

/// Every leaf triangle of the texture paint (unpainted ones as state 0) in a small BVH.
struct Leaves {
    tris: Vec<(u8, [[f64; 3]; 3])>,
    nodes: Vec<LeafNode>,
}
impl Leaves {
    fn new(mut tris: Vec<(u8, [[f64; 3]; 3])>) -> Self {
        let mut nodes = Vec::new();
        if !tris.is_empty() {
            let n = tris.len();
            Self::build(&mut tris, &mut nodes, 0, n);
        }
        Self { tris, nodes }
    }
    fn build(
        tris: &mut [(u8, [[f64; 3]; 3])],
        nodes: &mut Vec<LeafNode>,
        start: usize,
        end: usize,
    ) -> usize {
        let mut min = [f64::INFINITY; 3];
        let mut max = [f64::NEG_INFINITY; 3];
        for (_, t) in &tris[start..end] {
            for p in t {
                for k in 0..3 {
                    min[k] = min[k].min(p[k]);
                    max[k] = max[k].max(p[k]);
                }
            }
        }
        let index = nodes.len();
        nodes.push(((min, max), start, end, None));
        if end - start > 8 {
            let axis = (0..3)
                .max_by(|&a, &b| (max[a] - min[a]).total_cmp(&(max[b] - min[b])))
                .unwrap();
            let mid = (start + end) / 2;
            tris[start..end].select_nth_unstable_by(mid - start, |a, b| {
                let c = |t: &[[f64; 3]; 3]| t[0][axis] + t[1][axis] + t[2][axis];
                c(&a.1).total_cmp(&c(&b.1))
            });
            let l = Self::build(tris, nodes, start, mid);
            let r = Self::build(tris, nodes, mid, end);
            nodes[index].3 = Some((l, r));
        }
        index
    }
    /// Calls `f(state, squared distance)` for every leaf within `radius` of `p`.
    fn near(&self, p: [f64; 3], radius: f64, mut f: impl FnMut(u8, f64)) {
        if self.nodes.is_empty() {
            return;
        }
        let mut stack = vec![0];
        while let Some(i) = stack.pop() {
            let ((min, max), start, end, children) = self.nodes[i];
            let d = (0..3)
                .map(|k| (p[k] - p[k].clamp(min[k], max[k])).powi(2))
                .sum::<f64>();
            if d > radius * radius {
                continue;
            }
            if let Some((a, b)) = children {
                stack.extend([a, b]);
                continue;
            }
            for (state, t) in &self.tris[start..end] {
                let d = closest_distance_sq(p, t);
                if d <= radius * radius {
                    f(*state, d);
                }
            }
        }
    }
}

/// Squared distance from `p` to triangle `t` (Ericson, Real-Time Collision Detection 5.1.5).
fn closest_distance_sq(p: [f64; 3], t: &[[f64; 3]; 3]) -> f64 {
    let s = |a: [f64; 3], b: [f64; 3]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    let d = |a: [f64; 3], b: [f64; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let [a, b, c] = *t;
    let (ab, ac, ap) = (s(b, a), s(c, a), s(p, a));
    let (d1, d2) = (d(ab, ap), d(ac, ap));
    let along =
        |o: [f64; 3], e: [f64; 3], k: f64| [o[0] + e[0] * k, o[1] + e[1] * k, o[2] + e[2] * k];
    let q = if d1 <= 0. && d2 <= 0. {
        a
    } else {
        let bp = s(p, b);
        let (d3, d4) = (d(ab, bp), d(ac, bp));
        let cp = s(p, c);
        let (d5, d6) = (d(ab, cp), d(ac, cp));
        let vc = d1 * d4 - d3 * d2;
        let vb = d5 * d2 - d1 * d6;
        let va = d3 * d6 - d5 * d4;
        if d3 >= 0. && d4 <= d3 {
            b
        } else if vc <= 0. && d1 >= 0. && d3 <= 0. {
            along(a, ab, d1 / (d1 - d3))
        } else if d6 >= 0. && d5 <= d6 {
            c
        } else if vb <= 0. && d2 >= 0. && d6 <= 0. {
            along(a, ac, d2 / (d2 - d6))
        } else if va <= 0. && (d4 - d3) >= 0. && (d5 - d6) >= 0. {
            along(b, s(c, b), (d4 - d3) / ((d4 - d3) + (d5 - d6)))
        } else {
            let den = 1. / (va + vb + vc);
            let (v, w) = (vb * den, vc * den);
            [
                a[0] + ab[0] * v + ac[0] * w,
                a[1] + ab[1] * v + ac[1] * w,
                a[2] + ab[2] * v + ac[2] * w,
            ]
        }
    };
    let e = s(p, q);
    d(e, e)
}

/// Points along the edges of painted areas, in a uniform 3-D grid.
struct EdgePoints {
    cell: f64,
    cells: HashMap<[i64; 3], Vec<[f64; 3]>>,
}
impl EdgePoints {
    fn distance(&self, p: [f64; 3], limit: f64) -> f64 {
        let c = p.map(|v| (v / self.cell).floor() as i64);
        let reach = (limit / self.cell).ceil() as i64;
        let mut best = limit * limit;
        for dz in -reach..=reach {
            for dy in -reach..=reach {
                for dx in -reach..=reach {
                    if let Some(pts) = self.cells.get(&[c[0] + dx, c[1] + dy, c[2] + dz]) {
                        for q in pts {
                            let d = (p[0] - q[0]).powi(2)
                                + (p[1] - q[1]).powi(2)
                                + (p[2] - q[2]).powi(2);
                            best = best.min(d);
                        }
                    }
                }
            }
        }
        best.sqrt()
    }
}

/// The texture paint of an object: slots and the soft border.
pub(crate) struct PaintField {
    leaves: Leaves,
    edges: EdgePoints,
}
impl PaintField {
    /// `None` when no model volume has texture paint.
    pub(crate) fn new(model: &crate::regions::RegionModel, falloff: f64) -> Option<Self> {
        let volumes: Vec<_> = model
            .volumes
            .iter()
            .filter(|v| v.kind == crate::threemf::PartKind::Model)
            .collect();
        if volumes.iter().all(|v| v.painting.texture.is_empty()) {
            return None;
        }
        let mut tris = Vec::new();
        for v in &volumes {
            v.painting
                .texture
                .visit(&v.mesh, true, |state, t| tris.push((state, t)));
        }
        // Edges of painted leaves that no leaf of the same state shares exactly are where the
        // paint may end (or where a neighbour is split finer); sample them and keep the points
        // that touch a leaf of another state.
        let key = |p: [f64; 3]| p.map(|c| (c * 1e5).round() as i64);
        let mut count: HashMap<(u8, [i64; 3], [i64; 3]), u32> = HashMap::new();
        for (s, t) in tris.iter().filter(|(s, _)| *s > 0) {
            for (a, b) in [(0, 1), (1, 2), (2, 0)] {
                let (ka, kb) = (key(t[a]), key(t[b]));
                *count.entry((*s, ka.min(kb), ka.max(kb))).or_default() += 1;
            }
        }
        let leaves = Leaves::new(tris.clone());
        let step = (falloff / 4.).clamp(0.1, 0.5);
        let cell = falloff.max(0.5);
        let mut cells: HashMap<[i64; 3], Vec<[f64; 3]>> = HashMap::new();
        for (s, t) in tris.iter().filter(|(s, _)| *s > 0) {
            for (a, b) in [(0, 1), (1, 2), (2, 0)] {
                let (ka, kb) = (key(t[a]), key(t[b]));
                if count[&(*s, ka.min(kb), ka.max(kb))] > 1 {
                    continue;
                }
                let len = (0..3)
                    .map(|k| (t[b][k] - t[a][k]).powi(2))
                    .sum::<f64>()
                    .sqrt();
                let n = (len / step).ceil().max(1.) as usize;
                for i in 0..=n {
                    let f = i as f64 / n as f64;
                    let q: [f64; 3] = std::array::from_fn(|k| t[a][k] + (t[b][k] - t[a][k]) * f);
                    let mut other = false;
                    leaves.near(q, 1e-4, |state, _| other |= state != *s);
                    if other {
                        cells
                            .entry(q.map(|v| (v / cell).floor() as i64))
                            .or_default()
                            .push(q);
                    }
                }
            }
        }
        Some(Self {
            leaves,
            edges: EdgePoints { cell, cells },
        })
    }

    /// Distance from `p` to the nearest edge of a painted area, up to `limit`.
    pub(crate) fn edge_distance(&self, p: [f64; 3], limit: f64) -> f64 {
        self.edges.distance(p, limit)
    }

    /// The slot nearest to `p` within `radius` (0 when the nearest facet is unpainted).
    pub(crate) fn slot(&self, p: [f64; 3], radius: f64) -> u8 {
        let mut best = (f64::INFINITY, 0u8);
        self.leaves.near(p, radius, |state, d| {
            if d < best.0 || (d == best.0 && state > best.1) {
                best = (d, state);
            }
        });
        best.1
    }
}

// ---------------------------------------------------------------------------------------------
// The object's texture context.

/// Everything one object needs to texture its outer walls.
pub struct TextureSkin {
    normals: SurfaceNormals,
    paint: Option<PaintField>,
    /// Slice plane of every layer, bed frame.
    zs: Vec<f64>,
    bounds: Bounds,
    /// The steep walls the mesh displacement left to this texture, and their rims.
    walls: Option<Arc<crate::texture::displace::Rims>>,
}

impl TextureSkin {
    /// The object's texture context when any of `settings` (the object's and its regions')
    /// textures a wall or the model has texture paint.
    pub fn prepare(
        mesh: &TriangleMesh,
        settings: &[&TextureSettings],
        model: Option<&crate::regions::RegionModel>,
        zs: &[f64],
    ) -> Result<Option<Arc<Self>>, String> {
        Self::prepare_with_walls(mesh, settings, model, zs, None)
    }

    /// [`Self::prepare`], also texturing the steep walls the mesh displacement left to the wall
    /// texture (`walls`, settings on all surfaces).
    pub fn prepare_with_walls(
        mesh: &TriangleMesh,
        settings: &[&TextureSettings],
        model: Option<&crate::regions::RegionModel>,
        zs: &[f64],
        walls_left: Option<Arc<crate::texture::displace::Rims>>,
    ) -> Result<Option<Arc<Self>>, String> {
        let falloff = settings
            .iter()
            .flat_map(|s| {
                std::iter::once(s.falloff).chain(
                    s.slots
                        .keys()
                        .filter_map(|&n| s.slot(n).ok().map(|s| s.falloff)),
                )
            })
            .fold(0., f64::max);
        // Settings on all surfaces are textured in the mesh ([`crate::texture::displace`]);
        // the walls are left to this texture where walls-only settings apply, and the steep
        // walls the displacement left to it (`walls_left`).
        let walls = |s: &TextureSettings| s.surfaces == crate::texture::Surfaces::Walls;
        let painted = model.is_some_and(|m| {
            m.volumes.iter().any(|v| {
                v.kind == crate::threemf::PartKind::Model && !v.painting.texture.is_empty()
            })
        });
        let slots_on_walls = |s: &TextureSettings| {
            walls(s)
                || s.slots
                    .keys()
                    .any(|&n| s.slot(n).is_ok_and(|ss| walls(&ss)))
        };
        if walls_left.is_none()
            && !settings
                .iter()
                .any(|s| (s.enabled() && walls(s)) || (painted && slots_on_walls(s)))
        {
            return Ok(None);
        }
        let paint = model.and_then(|m| PaintField::new(m, falloff.max(0.5)));
        if paint.is_none() && !settings.iter().any(|s| s.enabled()) {
            return Ok(None);
        }
        // Resolve every map now, so a broken image fails the slice instead of a wall.
        for s in settings {
            if s.enabled() {
                s.map()?;
            }
            if paint.is_some() {
                for &n in s.slots.keys() {
                    s.slot(n)?.map()?;
                }
            }
        }
        let (min, max) = mesh.bounds();
        Ok(Some(Arc::new(Self {
            normals: SurfaceNormals::new(mesh, zs),
            paint,
            zs: zs.to_vec(),
            bounds: Bounds {
                min: min.map(f64::from),
                max: max.map(f64::from),
            },
            walls: walls_left,
        })))
    }
}

/// Warnings about texture depths for one region (shown with the slice result).
pub fn warnings(
    s: &TextureSettings,
    external: &Flow,
    arachne: bool,
    perimeters: usize,
) -> Vec<String> {
    let mut out = Vec::new();
    // On all surfaces the texture is in the mesh and walls are made around it: no wall limits.
    if (!s.enabled() && s.slots.is_empty()) || s.surfaces != crate::texture::Surfaces::Walls {
        return out;
    }
    let (outward, inward) = s.direction.reach(s.amplitude);
    let cap = inward_cap(external);
    if inward > cap + 1e-9 {
        out.push(format!(
            "Texture: {:.2} mm deep inward is more than half an outer wall ({cap:.2} mm); Mica limits it so the wall stays whole.",
            inward
        ));
    }
    if inward > 0. && perimeters <= 1 {
        out.push(
            "Texture: an inward texture with one wall can cut through the wall; use at least two walls.".into(),
        );
    }
    // With Arachne a deeper outward texture widens the outer wall instead (the Depth help says
    // so); only the Classic generator has to limit it.
    if outward > 0.5 * external.width + 1e-9 && !arachne {
        out.push(format!(
            "Texture: {outward:.2} mm outward is more than half the outer wall width ({:.2} mm); with the Classic wall generator Mica limits it. Use Arachne for deeper textures.",
            0.5 * external.width
        ));
    }
    out
}

/// Points closer than this (mm) to the line through their neighbours are dropped: far below
/// what a nozzle prints, it removes most points of flat runs and straight texture slopes.
const SIMPLIFY: f64 = 0.02;

/// Steps of the combined mode's widening, mm.
const WIDEN_STEP: f64 = 0.1;

/// How far a texture may reach into the part: half the outer wall's spacing, so the outer wall
/// never prints into the next one.
fn inward_cap(external: &Flow) -> f64 {
    0.5 * external.spacing()
}

/// One region's (or slot's) texture ready to evaluate.
struct Active {
    s: TextureSettings,
    map: Arc<HeightMap>,
    /// Depth after the direction's caps, mm.
    depth: f64,
    /// Widen the line instead of moving it where the texture rises (Arachne).
    combined: bool,
    /// Settings on all surfaces: only on the steep walls the mesh left to this texture, by
    /// their share ([`crate::texture::displace::Rims`]).
    all_surfaces: bool,
}
impl Active {
    /// `None` for settings on all surfaces (their texture is in the mesh already) unless the
    /// mesh left steep walls to this texture (`walls_left`).
    fn new(s: TextureSettings, external: &Flow, arachne: bool, walls_left: bool) -> Option<Self> {
        let all_surfaces = s.surfaces != crate::texture::Surfaces::Walls;
        if all_surfaces && !walls_left {
            return None;
        }
        let map = s.map().ok()?;
        let cap = inward_cap(external);
        let depth = match s.direction {
            Direction::Outward if !arachne => s.amplitude.min(0.5 * external.width),
            Direction::Outward => s.amplitude,
            Direction::Inward => s.amplitude.min(cap),
            Direction::Both if !arachne => s.amplitude.min(2. * cap).min(external.width),
            Direction::Both => s.amplitude.min(2. * cap),
        };
        let combined = arachne && s.direction.reach(depth).0 > 0.5 * external.width;
        Some(Self {
            s,
            map,
            depth,
            combined,
            all_surfaces,
        })
    }

    /// Displacement (mm, positive outward) at `p` with unit surface normal `n`.
    fn offset(&self, projector: &Projector, p: [f64; 3], n: [f64; 3]) -> f64 {
        let fade = angle_fade(&self.s, n);
        if fade <= 0. {
            return 0.;
        }
        let g = projector.grey(p, n);
        let mut d = self.s.direction.offset(g, self.depth) * fade;
        if d > 0. && self.s.overhang_clamp && n[2] < 0. {
            d *= smoothstep(35., 60., surface_angle(n));
        }
        d
    }
}

/// A painted slot's soft border and, when it textures walls only, its wall texture.
struct Slot {
    falloff: f64,
    curve: crate::texture::Curve,
    first_layer: bool,
    active: Option<Active>,
}

/// The surface's angle above horizontal, degrees (90 on a vertical wall).
fn surface_angle(n: [f64; 3]) -> f64 {
    n[2].abs().clamp(0., 1.).acos().to_degrees()
}
fn smoothstep(a: f64, b: f64, x: f64) -> f64 {
    let t = ((x - a) / (b - a)).clamp(0., 1.);
    t * t * (3. - 2. * t)
}
/// No texture on surfaces flatter than the top / bottom angle limit, fading in over 10°; walls
/// flatter than 8° never get one (sideways displacement there would run away).
fn angle_fade(s: &TextureSettings, n: [f64; 3]) -> f64 {
    let a = surface_angle(n);
    let limit = if n[2] >= 0. {
        s.top_angle
    } else {
        s.bottom_angle
    };
    let user = if limit > 0. {
        smoothstep(limit, limit + 10., a)
    } else {
        1.
    };
    user * smoothstep(3., 8., a)
}

/// The object's texture context, the region's settings and the shift from wall to bed coordinates.
#[derive(Clone, Copy)]
pub struct Surface<'a> {
    pub skin: &'a TextureSkin,
    pub offset: Point,
}

/// A displaced point and its line width (scaled; 0 for classic walls).
type Out = (Point, f64);

impl Surface<'_> {
    fn z(&self, layer: usize) -> Option<f64> {
        self.skin.zs.get(layer).copied()
    }

    /// Classic outer wall loop (counter-clockwise contours, clockwise holes).
    pub fn displace_polygon(
        &self,
        poly: &mut Polygon,
        layer: usize,
        depth: usize,
        contour: bool,
        s: &TextureSettings,
        external: &Flow,
    ) {
        if depth != 0 || poly.len() < 3 {
            return;
        }
        let Some(z) = self.z(layer) else { return };
        let pts: Vec<Out> = poly.iter().map(|&p| (p, 0.)).collect();
        if let Some(out) = self.displace_closed(&pts, layer, z, contour, s, external, false) {
            *poly = out.into_iter().map(|(p, _)| p).collect();
        }
    }

    /// Arachne outer wall (closed lines repeat their first junction at the end).
    pub fn displace_line(
        &self,
        line: &mut ExtrusionLine,
        layer: usize,
        s: &TextureSettings,
        external: &Flow,
    ) {
        if line.inset != 0 || !line.is_closed || line.junctions.len() < 4 {
            return;
        }
        let Some(z) = self.z(layer) else { return };
        let mut js = line.junctions.clone();
        if js.first().map(|j| j.p) == js.last().map(|j| j.p) {
            js.pop();
        }
        let contour = signed_area(&js.iter().map(|j| j.p).collect::<Vec<_>>()) > 0.;
        let pts: Vec<Out> = js.iter().map(|j| (j.p, j.width)).collect();
        let Some(moved) = self.displace_closed(&pts, layer, z, contour, s, external, true) else {
            return;
        };
        let perimeter = js[0].perimeter;
        let mut out: Vec<Junction> = moved
            .into_iter()
            .map(|(p, width)| Junction {
                p,
                width,
                perimeter,
            })
            .collect();
        out.push(out[0]);
        line.junctions = out;
    }

    /// Resamples a closed loop and moves every point along the wall's outward normal. Points
    /// keep their widths (scaled), which grow where a combined texture rises. `None` when no
    /// point of the loop is textured.
    #[allow(clippy::too_many_arguments)]
    fn displace_closed(
        &self,
        pts: &[Out],
        layer: usize,
        z: f64,
        contour: bool,
        s: &TextureSettings,
        external: &Flow,
        arachne: bool,
    ) -> Option<Vec<Out>> {
        let skin = self.skin;
        let walls_left = skin.walls.is_some();
        // On all surfaces the mesh textures holes too ("Outer walls" and "All outer walls" are
        // one on a mesh), so the steep walls left to this texture are textured on holes too.
        let all_surfaces = s.surfaces != crate::texture::Surfaces::Walls;
        let base = (s.mode == Mode::All
            || (s.mode == Mode::External && (contour || (walls_left && all_surfaces))))
            .then(|| Active::new(s.clone(), external, arachne, walls_left))
            .flatten()
            .filter(|_| layer > 0 || s.first_layer);
        if base.is_none() && skin.paint.is_none() {
            return None;
        }
        let mut slots: Vec<Option<Option<Slot>>> = (0..16).map(|_| None).collect();
        let base_projector = base
            .as_ref()
            .map(|b| Projector::new(&b.map, b.s.mapping, skin.bounds));
        let normals = skin.normals.layer(layer);
        let n = pts.len();
        let step = s.point_distance.max(0.05) * SCALE;
        let right = |a: Point, b: Point| {
            let (dx, dy) = ((b.x - a.x) as f64, (b.y - a.y) as f64);
            let l = dx.hypot(dy);
            if l > 0. { [dy / l, -dx / l] } else { [0., 0.] }
        };
        let radius = 0.5 * external.width + 0.4;
        let mut out: Vec<Out> = Vec::with_capacity(n * 2);
        let mut any = false;
        let mut level = 0f64;
        for i in 0..n {
            let (a, wa) = pts[i];
            let (b, wb) = pts[(i + 1) % n];
            let prev = pts[(i + n - 1) % n].0;
            let edge = right(a, b);
            let before = right(prev, a);
            let len = a.distance_to(b);
            let k = (len / step).ceil().max(1.) as usize;
            for j in 0..k {
                let t = j as f64 / k as f64;
                let q = Point::new(
                    a.x + ((b.x - a.x) as f64 * t).round() as i64,
                    a.y + ((b.y - a.y) as f64 * t).round() as i64,
                );
                let width = wa + (wb - wa) * t;
                // At a vertex both edges move by the same amount: the mitre.
                let dir = if j == 0 { mitre(before, edge) } else { edge };
                let along = if j == 0 { normalize(dir) } else { edge };
                let bed = [
                    (q.x + self.offset.x) as f64 / SCALE,
                    (q.y + self.offset.y) as f64 / SCALE,
                    z,
                ];
                let near = normals.and_then(|l| l.nearest([bed[0], bed[1]], along, radius));
                let normal = near.as_ref().map_or([along[0], along[1], 0.], |n| n.normal);
                // The share of settings on all surfaces here: on a steep wall the mesh left to
                // this texture, what the mesh does not carry near its rims; elsewhere none.
                let share = match (&skin.walls, &near) {
                    (Some(r), Some(n)) if n.vertical => r.wall_share([n.foot[0], n.foot[1], z]),
                    _ => 0.,
                };
                let weight = |a: &Active| if a.all_surfaces { share } else { 1. };
                let mut d = 0.;
                let mut combined = false;
                if let (Some(b), Some(p)) = (&base, &base_projector) {
                    d = b.offset(p, bed, normal) * weight(b);
                    combined = b.combined;
                }
                if let Some(paint) = &skin.paint {
                    let slot = paint.slot(bed, external.width.max(0.3));
                    if slot > 0 {
                        let entry = slots[slot as usize].get_or_insert_with(|| {
                            s.slot(slot).ok().map(|ss| Slot {
                                falloff: ss.falloff,
                                curve: ss.falloff_curve,
                                first_layer: ss.first_layer,
                                active: Active::new(ss, external, arachne, walls_left),
                            })
                        });
                        if let Some(sl) = entry.as_ref().filter(|a| layer > 0 || a.first_layer) {
                            let w = if sl.falloff > 0. {
                                sl.curve
                                    .apply(paint.edges.distance(bed, sl.falloff) / sl.falloff)
                            } else {
                                1.
                            };
                            // A slot on all surfaces is in the mesh: here it only fades the
                            // region's wall texture out (and textures its share of the steep
                            // walls left to this texture).
                            let v = sl.active.as_ref().map_or(0., |act| {
                                let p = Projector::new(&act.map, act.s.mapping, skin.bounds);
                                act.offset(&p, bed, normal) * weight(act)
                            });
                            d = d * (1. - w) + v * w;
                            combined |= sl.active.as_ref().is_some_and(|a| a.combined);
                        }
                    }
                }
                any |= d != 0.;
                // Combined (as Orca's fuzzy skin mode): above half the line width the wall
                // widens instead of moving, so its inner edge stays on the next wall. The
                // widening goes in steps with some hysteresis, so the wall splits into few
                // paths of one width; the outer edge still follows the texture exactly.
                let free = 0.5 * external.width;
                let raw = if combined {
                    (d - free) / WIDEN_STEP
                } else {
                    0.
                };
                if raw <= 0. {
                    level = 0.;
                } else if (raw - level).abs() > 0.75 {
                    level = raw.round();
                }
                let extra = level * WIDEN_STEP;
                let (shift, width) = (d - extra / 2., width + extra * SCALE);
                out.push((
                    Point::new(
                        q.x + (dir[0] * shift * SCALE).round() as i64,
                        q.y + (dir[1] * shift * SCALE).round() as i64,
                    ),
                    width,
                ));
            }
        }
        if !any {
            return None;
        }
        // Points from both sides of an inner corner cross when the texture is deeper than the
        // corner allows; cut those loops out (twice, so a loop across the start is caught too).
        let window = (((4. * s.amplitude + 1.) * SCALE / step).ceil() as usize + 4).min(64);
        untangle(&mut out, window);
        let half = out.len() / 2;
        out.rotate_left(half);
        untangle(&mut out, window);
        let back = half.min(out.len());
        out.rotate_right(back);
        simplify(&mut out, SIMPLIFY * SCALE);
        (out.len() >= 3).then_some(out)
    }
}

fn normalize(v: [f64; 2]) -> [f64; 2] {
    let l = v[0].hypot(v[1]);
    if l > 1e-12 { [v[0] / l, v[1] / l] } else { v }
}

/// The vector that moves a corner so both edges move by one unit along their normals, at most
/// twice as long as a normal (BumpMesh caps its crease mitres at twice the depth).
fn mitre(n1: [f64; 2], n2: [f64; 2]) -> [f64; 2] {
    let c = 1. + n1[0] * n2[0] + n1[1] * n2[1];
    if c < 1e-6 {
        return n2;
    }
    let m = [(n1[0] + n2[0]) / c, (n1[1] + n2[1]) / c];
    let l = m[0].hypot(m[1]);
    if l > 2. {
        [m[0] * 2. / l, m[1] * 2. / l]
    } else {
        m
    }
}

fn cross2(o: Point, a: Point, b: Point) -> f64 {
    (a.x - o.x) as f64 * (b.y - o.y) as f64 - (a.y - o.y) as f64 * (b.x - o.x) as f64
}

/// Where segments `ab` and `cd` cross properly, with the parameter along `ab`.
fn crossing(a: Point, b: Point, c: Point, d: Point) -> Option<(Point, f64)> {
    let (d1, d2) = (cross2(c, d, a), cross2(c, d, b));
    let (d3, d4) = (cross2(a, b, c), cross2(a, b, d));
    if (d1 > 0.) == (d2 > 0.)
        || (d3 > 0.) == (d4 > 0.)
        || d1 == 0.
        || d2 == 0.
        || d3 == 0.
        || d4 == 0.
    {
        return None;
    }
    let t = d1 / (d1 - d2);
    Some((
        Point::new(
            a.x + ((b.x - a.x) as f64 * t).round() as i64,
            a.y + ((b.y - a.y) as f64 * t).round() as i64,
        ),
        t,
    ))
}

/// Removes small loops: when segment `i` crosses a segment up to `window` points later, the
/// points in between go and the crossing takes their place.
fn untangle(pts: &mut Vec<Out>, window: usize) {
    let n = pts.len();
    if n < 4 {
        return;
    }
    let mut out = Vec::with_capacity(n);
    let mut i = 0;
    while i < n {
        out.push(pts[i]);
        if i + 1 >= n {
            break;
        }
        let (a, b) = (pts[i].0, pts[i + 1].0);
        let mut cut = None;
        for j in (i + 2..(i + window).min(n - 1)).rev() {
            if let Some((x, t)) = crossing(a, b, pts[j].0, pts[j + 1].0) {
                cut = Some((j, x, t));
                break;
            }
        }
        match cut {
            Some((j, x, t)) => {
                let w = pts[i].1 + (pts[i + 1].1 - pts[i].1) * t;
                out.push((x, w));
                i = j + 1;
            }
            None => i += 1,
        }
    }
    *pts = out;
}

/// Drops points that lie within `tolerance` of the line through their neighbours and keep the
/// width (flat, untextured runs need no extra points).
fn simplify(pts: &mut Vec<Out>, tolerance: f64) {
    if pts.len() < 4 {
        return;
    }
    let mut out: Vec<Out> = Vec::with_capacity(pts.len());
    out.push(pts[0]);
    for i in 1..pts.len() {
        let (b, wb) = pts[i];
        let (c, wc) = pts[(i + 1) % pts.len()];
        let (a, wa) = *out.last().unwrap();
        let (dx, dy) = ((c.x - a.x) as f64, (c.y - a.y) as f64);
        let l = dx.hypot(dy);
        let off = if l > 0. {
            ((b.x - a.x) as f64 * dy - (b.y - a.y) as f64 * dx).abs() / l
        } else {
            f64::INFINITY
        };
        let along = if l > 0. {
            ((b.x - a.x) as f64 * dx + (b.y - a.y) as f64 * dy) / (l * l)
        } else {
            0.
        };
        let width_kept =
            (wb - (wa + (wc - wa) * along.clamp(0., 1.))).abs() <= 0.01 * wb.abs().max(1.);
        if !(off <= tolerance && (0. ..=1.).contains(&along) && width_kept) {
            out.push((b, wb));
        }
    }
    if out.len() >= 3 {
        *pts = out;
    }
}

/// Carries an object's texture skin into the wall generators, with no fuzzy-skin painting.
pub struct Painting<'a>(pub &'a TextureSkin);
impl crate::fuzzy_skin::FuzzySkinPainting for Painting<'_> {
    fn needs_sampling(&self) -> bool {
        false
    }
    fn settings_at(&self, _: Point, _: usize) -> Option<crate::fuzzy_skin::FuzzySkinSettings> {
        None
    }
    fn texture_skin(&self) -> Option<Surface<'_>> {
        Some(Surface {
            skin: self.0,
            offset: Point::default(),
        })
    }
}

#[cfg(test)]
mod tests;
