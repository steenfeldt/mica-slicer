// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/ModelVisibility.cpp :: Visibility::Visibility, raycast_visibility, Frame, sample_hemisphere_uniform, calculate_point_visibility
// Source: prusaslicer@b028299c77 src/libslic3r/TriangleSetSampling.cpp :: sample_its_uniform_parallel
// Source: prusaslicer@b028299c77 src/libslic3r/AABBTreeIndirect.hpp :: build_aabb_tree_over_indexed_triangle_set, intersect_ray_first_hit, ray_box_intersect_invdir, intersect_triangle
// Source: prusaslicer@b028299c77 src/libslic3r/KDTreeIndirect.hpp :: KDTreeIndirect, find_nearby_points
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/SeamPlacer.cpp :: precalculate_seams, Placer::get_params
// Changes: Rust port with the upstream f32/f64 mix and Eigen 3.3 evaluation order (float three-term sums as c0 + (c1 + c2), double ones as (c0 + c1) + c2, contracted cross products).
// Changes: The placed bed-frame mesh is decimated (translation invariant for exactly translated vertices) and moved to PrusaSlicer's object frame by its XY bounding-box centre.
// Changes: Model parts only (no negative-volume ray parity); rayon over samples; boost uniform_real_distribution<double> reimplemented.
// Copyright (c) Prusa Research 2022 Pavel Mikuš @Godrak (upstream visibility and sampling).

use super::decimate::{self, V3f, cross, dot, face_normal, norm, normalized, sub};
use super::random::Mt19937;
use crate::mesh::TriangleMesh;
use rayon::prelude::*;

const SAMPLES: usize = 30_000;
const DECIMATION_TARGET: usize = 16_000;
const SQR_RAYS: usize = 5;

type V3d = [f64; 3];
fn to_d(a: V3f) -> V3d {
    a.map(f64::from)
}
fn sub_d(a: V3d, b: V3d) -> V3d {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn cross_d(a: V3d, b: V3d) -> V3d {
    [
        a[1].mul_add(b[2], -(a[2] * b[1])),
        a[2].mul_add(b[0], -(a[0] * b[2])),
        a[0].mul_add(b[1], -(a[1] * b[0])),
    ]
}
/// Eigen vectorizes double three-term reductions on arm64 as `(c0 + c1) + c2`.
fn dot_d(a: V3d, b: V3d) -> f64 {
    (a[0] * b[0] + a[1] * b[1]) + a[2] * b[2]
}

/// `boost::random::uniform_real_distribution<double>` on [0, 1) from the 64-bit engine.
fn uniform_real(engine: &mut Mt19937) -> f64 {
    loop {
        let r = engine.next() as f64 / 18_446_744_073_709_551_616.0;
        if r < 1.0 {
            return r;
        }
    }
}

#[derive(Clone, Copy)]
struct Bbox {
    min: V3f,
    max: V3f,
}
impl Bbox {
    fn extend(&mut self, o: &Bbox) {
        for k in 0..3 {
            self.min[k] = self.min[k].min(o.min[k]);
            self.max[k] = self.max[k].max(o.max[k]);
        }
    }
}
const NPOS: usize = usize::MAX;
const INNER: usize = usize::MAX - 1;
#[derive(Clone, Copy)]
struct Node {
    idx: usize,
    bbox: Bbox,
}
struct Primitive {
    idx: usize,
    bbox: Bbox,
    centroid: V3f,
}

/// The implicit balanced AABB tree of `AABBTreeIndirect::Tree<3, float>`, one triangle per leaf.
struct AabbTree {
    nodes: Vec<Node>,
}
impl AabbTree {
    fn new(vertices: &[V3f], faces: &[[u32; 3]]) -> Self {
        let third = (1.0f64 / 3.0) as f32;
        let mut input: Vec<Primitive> = faces
            .iter()
            .enumerate()
            .map(|(i, f)| {
                let [a, b, c] = f.map(|v| vertices[v as usize]);
                let centroid = std::array::from_fn(|k| third * ((a[k] + b[k]) + c[k]));
                let mut bbox = Bbox { min: a, max: a };
                for p in [b, c] {
                    bbox.extend(&Bbox { min: p, max: p });
                }
                Primitive {
                    idx: i,
                    bbox,
                    centroid,
                }
            })
            .collect();
        let empty = Node {
            idx: NPOS,
            bbox: Bbox {
                min: [0.0; 3],
                max: [0.0; 3],
            },
        };
        let mut tree = Self { nodes: Vec::new() };
        if !input.is_empty() {
            tree.nodes = vec![empty; input.len().next_power_of_two() * 2 - 1];
            let right = input.len() - 1;
            tree.build(&mut input, 0, 0, right);
        }
        tree
    }
    fn build(&mut self, input: &mut [Primitive], node: usize, left: usize, right: usize) {
        if left == right {
            self.nodes[node] = Node {
                idx: input[left].idx,
                bbox: input[left].bbox,
            };
            return;
        }
        let mut bbox = input[left].bbox;
        for p in &input[left + 1..=right] {
            bbox.extend(&p.bbox);
        }
        let diagonal = sub(bbox.max, bbox.min);
        let mut dimension = 0;
        for k in 1..3 {
            if diagonal[k] > diagonal[dimension] {
                dimension = k;
            }
        }
        let center = (left + right) / 2;
        partition(input, |p| p.centroid[dimension], left, right, center);
        self.nodes[node] = Node { idx: INNER, bbox };
        self.build(input, node * 2 + 1, left, center);
        self.build(input, node * 2 + 2, center + 1, right);
    }
}

/// The quickselect shared by `AABBTreeIndirect::Tree` and `KDTreeIndirect::partition_input`.
fn partition<T>(
    input: &mut [T],
    key: impl Fn(&T) -> f32,
    mut left: usize,
    mut right: usize,
    k: usize,
) {
    while left < right {
        let center = (left + right) / 2;
        let pivot;
        {
            let mut left_value = key(&input[left]);
            let mut center_value = key(&input[center]);
            let mut right_value = key(&input[right]);
            if left_value > center_value {
                input.swap(left, center);
                std::mem::swap(&mut left_value, &mut center_value);
            }
            if left_value > right_value {
                input.swap(left, right);
                right_value = left_value;
            }
            if center_value > right_value {
                input.swap(center, right);
                center_value = right_value;
            }
            pivot = center_value;
        }
        if right <= left + 2 {
            break;
        }
        let mut i = left;
        let mut j = right - 1;
        input.swap(center, j);
        loop {
            loop {
                i += 1;
                if key(&input[i]) >= pivot {
                    break;
                }
            }
            loop {
                j -= 1;
                if !(key(&input[j]) > pivot && i < j) {
                    break;
                }
            }
            if i >= j {
                break;
            }
            input.swap(i, j);
        }
        input.swap(i, right - 1);
        if k < i {
            right = i - 1;
        } else if k == i {
            break;
        } else {
            left = i + 1;
        }
    }
}

struct Ray<'a> {
    vertices: &'a [V3f],
    faces: &'a [[u32; 3]],
    tree: &'a AabbTree,
    origin: V3d,
    dir: V3d,
    invdir: V3d,
}
struct Hit {
    id: usize,
    t: f32,
}
impl Ray<'_> {
    fn hit_box(&self, b: &Bbox, t0: f64, t1: f64) -> bool {
        let mut min = to_d(b.min);
        let mut max = to_d(b.max);
        let (o, inv) = (self.origin, self.invdir);
        if inv[0] < 0.0 {
            std::mem::swap(&mut min[0], &mut max[0]);
        }
        if inv[1] < 0.0 {
            std::mem::swap(&mut min[1], &mut max[1]);
        }
        let mut tmin = (min[0] - o[0]) * inv[0];
        let tymax = (max[1] - o[1]) * inv[1];
        if tmin > tymax {
            return false;
        }
        let mut tmax = (max[0] - o[0]) * inv[0];
        let tymin = (min[1] - o[1]) * inv[1];
        if tymin > tmax {
            return false;
        }
        if tymin > tmin {
            tmin = tymin;
        }
        if tymax < tmax {
            tmax = tymax;
        }
        if inv[2] < 0.0 {
            std::mem::swap(&mut min[2], &mut max[2]);
        }
        let tzmin = (min[2] - o[2]) * inv[2];
        if tzmin > tmax {
            return false;
        }
        let tzmax = (max[2] - o[2]) * inv[2];
        if tmin > tzmax {
            return false;
        }
        if tzmin > tmin {
            tmin = tzmin;
        }
        if tzmax < tmax {
            tmax = tzmax;
        }
        tmin < t1 && tmax > t0
    }
    /// `intersect_triangle` (Möller–Trumbore, `raytri.c` `intersect_triangle1`) in double.
    fn triangle(&self, face: usize) -> Option<f64> {
        const EPS: f64 = 0.000_001;
        let [v0, v1, v2] = self.faces[face].map(|v| to_d(self.vertices[v as usize]));
        let edge1 = sub_d(v1, v0);
        let edge2 = sub_d(v2, v0);
        let pvec = cross_d(self.dir, edge2);
        let det = dot_d(edge1, pvec);
        let qvec;
        if det > EPS {
            let tvec = sub_d(self.origin, v0);
            let u = dot_d(tvec, pvec);
            if u < 0.0 || u > det {
                return None;
            }
            qvec = cross_d(tvec, edge1);
            let v = dot_d(self.dir, qvec);
            if v < 0.0 || u + v > det {
                return None;
            }
        } else if det < -EPS {
            let tvec = sub_d(self.origin, v0);
            let u = dot_d(tvec, pvec);
            if u > 0.0 || u < det {
                return None;
            }
            qvec = cross_d(tvec, edge1);
            let v = dot_d(self.dir, qvec);
            if v > 0.0 || u + v < det {
                return None;
            }
        } else {
            return None;
        }
        let inv_det = 1.0 / det;
        Some(dot_d(edge2, qvec) * inv_det)
    }
    /// `intersect_ray_recursive_first_hit`: hit distances are compared as rounded floats.
    fn first_hit(&self, node_idx: usize, mut min_t: f64) -> Option<Hit> {
        let node = &self.tree.nodes[node_idx];
        if !self.hit_box(&node.bbox, 0.0, min_t) {
            return None;
        }
        if node.idx != INNER {
            return match self.triangle(node.idx) {
                Some(t) if t > 0.0 => Some(Hit {
                    id: node.idx,
                    t: t as f32,
                }),
                _ => None,
            };
        }
        let left = node_idx * 2 + 1;
        let mut hit = None;
        if let Some(h) = self.first_hit(left, min_t)
            && f64::from(h.t) < min_t
        {
            min_t = f64::from(h.t);
            hit = Some(h);
        }
        if let Some(h) = self.first_hit(left + 1, min_t)
            && f64::from(h.t) < min_t
        {
            hit = Some(h);
        }
        hit
    }
}

/// `KDTreeIndirect<3, float>` over the sample positions.
struct KdTree {
    nodes: Vec<usize>,
}
impl KdTree {
    fn new(points: &[V3f]) -> Self {
        let mut tree = Self { nodes: Vec::new() };
        if points.is_empty() {
            return tree;
        }
        let mut input: Vec<usize> = (0..points.len()).collect();
        tree.nodes = vec![NPOS; (points.len() + 1).next_power_of_two()];
        tree.build(points, &mut input, 0, 0, 0, points.len() - 1);
        tree
    }
    fn build(
        &mut self,
        points: &[V3f],
        input: &mut [usize],
        node: usize,
        dim: usize,
        left: usize,
        right: usize,
    ) {
        if left > right {
            return;
        }
        if left == right {
            self.nodes[node] = input[left];
            return;
        }
        let center = (left + right) / 2;
        partition(input, |&i| points[i][dim], left, right, center);
        self.nodes[node] = input[center];
        let next = if dim + 1 == 3 { 0 } else { dim + 1 };
        if center > left {
            self.build(points, input, node * 2 + 1, next, left, center - 1);
        }
        self.build(points, input, node * 2 + 2, next, center + 1, right);
    }
    /// `find_nearby_points`: indices strictly within `radius`, in tree visiting order.
    fn nearby(&self, points: &[V3f], center: V3f, radius: f32, out: &mut Vec<usize>) {
        out.clear();
        if self.nodes.is_empty() {
            return;
        }
        let max_sq = f64::from(radius) * f64::from(radius);
        self.visit(points, center, max_sq, 0, 0, out);
    }
    fn visit(
        &self,
        points: &[V3f],
        center: V3f,
        max_sq: f64,
        node: usize,
        dim: usize,
        out: &mut Vec<usize>,
    ) {
        if node >= self.nodes.len() || self.nodes[node] == NPOS {
            return;
        }
        let idx = self.nodes[node];
        let p = points[idx];
        let mut dist = 0.0f64;
        for k in 0..3 {
            let d = f64::from(center[k] - p[k]);
            dist += d * d;
        }
        if dist < max_sq {
            out.push(idx);
        }
        // descent_mask, with libslic3r's EPSILON = 1e-4.
        let d = center[dim] - p[dim];
        let (go_left, go_right) = if f64::from(d) * f64::from(d) < max_sq + 1e-4 {
            (true, true)
        } else if d > 0.0 {
            (false, true)
        } else {
            (true, false)
        };
        let next = if dim + 1 == 3 { 0 } else { dim + 1 };
        if go_left {
            self.visit(points, center, max_sq, node * 2 + 1, next, out);
        }
        if go_right {
            self.visit(points, center, max_sq, node * 2 + 2, next, out);
        }
    }
}

/// PrusaSlicer 2.9 `ModelInfo::Visibility` of one object, in its object frame.
pub(super) struct ModelVisibility {
    /// Bed frame minus this offset is the object frame the samples live in.
    offset: [f64; 3],
    positions: Vec<V3f>,
    normals: Vec<V3f>,
    visibility: Vec<f32>,
    tree: KdTree,
    radius: f32,
}

impl ModelVisibility {
    /// Builds the visibility field from the placed (bed-frame) mesh, reporting the ray casting as
    /// 0.5 .. 0.7 of the seam stage; samples after a cancel are skipped.
    pub fn new(
        mesh: &TriangleMesh,
        source: Option<&TriangleMesh>,
        observer: &dyn crate::progress::Observer,
    ) -> Self {
        let ticker = crate::progress::Ticker::span(
            observer,
            crate::progress::Stage::Seams,
            SAMPLES,
            0.5,
            0.7,
        );
        // The model frame: the mesh as loaded when known, else the placed mesh itself.
        let (model, to_bed) = match source.and_then(|s| translation(s, mesh).map(|d| (s, d))) {
            Some((s, d)) => (s, d),
            None => (mesh, [0.0; 3]),
        };
        let mut its = super::shared_vertices::admesh_shared_vertices(model);
        let to_object = centre_volume(&mut its);
        decimate::short_edge_collapse(&mut its, DECIMATION_TARGET);
        for v in &mut its.vertices {
            *v = std::array::from_fn(|k| (f64::from(v[k]) + to_object[k]) as f32);
        }
        // Bed frame -> model frame -> object frame.
        let offset = std::array::from_fn(|k| to_bed[k] - to_object[k]);
        let (positions, normals, total_area) = sample(&its);
        let tree = KdTree::new(&positions);
        let density = SAMPLES as f32 / total_area;
        let search_area = 4.0f32 / (-std::hint::black_box(0.9f32).ln() * density);
        let radius = (f64::from(search_area) / std::f64::consts::PI).sqrt() as f32;
        let aabb = AabbTree::new(&its.vertices, &its.faces);
        let directions = directions();
        let visibility: Vec<f32> = (0..positions.len())
            .into_par_iter()
            .map(|i| {
                ticker.tick();
                if i.is_multiple_of(256) && ticker.cancelled() {
                    return 1.0;
                }
                raycast(&its, &aabb, &directions, positions[i], normals[i])
            })
            .collect();
        Self {
            offset,
            positions,
            normals,
            visibility,
            tree,
            radius,
        }
    }

    /// Sample positions and normals (object frame), sample visibilities and the search radius.
    #[cfg(test)]
    pub fn samples(&self) -> (&[V3f], &[V3f], &[f32], f32) {
        (
            &self.positions,
            &self.normals,
            &self.visibility,
            self.radius,
        )
    }

    /// `calculate_point_visibility` at a bed-frame point (mm).
    pub fn at(&self, p: [f64; 3]) -> f64 {
        let position: V3f = std::array::from_fn(|k| (p[k] - self.offset[k]) as f32);
        let mut points = Vec::new();
        self.tree
            .nearby(&self.positions, position, self.radius, &mut points);
        if points.is_empty() {
            return 1.0;
        }
        let mut total_weight = 0.0f32;
        let mut total_visibility = 0.0f32;
        for &i in &points {
            let sample = self.positions[i];
            let plane = dot(sub(position, sample), self.normals[i]).abs();
            let mut weight = self.radius - plane;
            weight += self.radius - norm(sub(position, sample));
            total_visibility = weight.mul_add(self.visibility[i], total_visibility);
            total_weight += weight;
        }
        f64::from(total_visibility / total_weight)
    }
}
/// The translation from `source` to `placed` when the placed mesh is a translated copy of the
/// loaded one (same faces, every vertex within 1 µm of the bounding-box shift).
fn translation(source: &TriangleMesh, placed: &TriangleMesh) -> Option<[f64; 3]> {
    if source.faces != placed.faces || source.vertices.len() != placed.vertices.len() {
        return None;
    }
    let (smin, smax) = source.bounds();
    let (pmin, pmax) = placed.bounds();
    let centre = |a: f32, b: f32| (f64::from(a) + f64::from(b)) / 2.0;
    let d = [
        centre(pmin[0], pmax[0]) - centre(smin[0], smax[0]),
        centre(pmin[1], pmax[1]) - centre(smin[1], smax[1]),
        f64::from(pmin[2]) - f64::from(smin[2]),
    ];
    source
        .vertices
        .iter()
        .zip(&placed.vertices)
        .all(|(s, p)| (0..3).all(|k| (f64::from(s[k]) + d[k] - f64::from(p[k])).abs() < 1e-3))
        .then_some(d)
}

/// The import and print-object transforms of an STL model part, applied in place: the volume is
/// centred on its bounding box in float (`ModelVolume::center_geometry_after_creation`) and moved
/// back by its double volume offset. Returns the translation `PrintObject::trafo_centered` then
/// applies: minus the XY bounding-box centre truncated to the 1 nm grid, and Z onto the bed.
fn centre_volume(its: &mut TriangleMesh) -> [f64; 3] {
    let (min, max) = its.bounds();
    let shift: [f64; 3] = std::array::from_fn(|k| (f64::from(min[k]) + f64::from(max[k])) / 2.0);
    if shift != [0.0; 3] {
        let d = shift.map(|s| -(s as f32));
        for v in &mut its.vertices {
            for k in 0..3 {
                v[k] += d[k];
            }
        }
    }
    // Raw bounding box of the volume (centred float vertices plus the double offset).
    let mut bmin = [f64::INFINITY; 3];
    let mut bmax = [f64::NEG_INFINITY; 3];
    for v in &mut its.vertices {
        for k in 0..3 {
            let p = f64::from(v[k]) + shift[k];
            bmin[k] = bmin[k].min(p);
            bmax[k] = bmax[k].max(p);
            v[k] = p as f32;
        }
    }
    let centre: [f64; 2] = std::array::from_fn(|k| {
        let c = (bmin[k] + bmax[k]) / 2.0;
        (c / 0.000_001) as i64 as f64 * 0.000_001
    });
    [-centre[0], -centre[1], 0.0 - bmin[2]]
}

/// `sample_its_uniform_parallel`: positions, normals and the float total area.
fn sample(its: &TriangleMesh) -> (Vec<V3f>, Vec<V3f>, f32) {
    let corners = |f: [u32; 3]| f.map(|v| its.vertices[v as usize]);
    let mut keys: Vec<(f64, usize)> = Vec::with_capacity(its.faces.len());
    let mut area_sum = 0.0f32;
    for (i, &f) in its.faces.iter().enumerate() {
        let [a, b, c] = corners(f);
        let area = 0.5 * f64::from(norm(cross(sub(b, a), sub(c, a))));
        area_sum = (f64::from(area_sum) + area) as f32;
        let key = f64::from(area_sum);
        // std::map assignment: an equal key keeps its position and takes the later triangle.
        match keys.last_mut() {
            Some(last) if last.0 == key => last.1 = i,
            _ => keys.push((key, i)),
        }
    }
    let mut engine = Mt19937::new(27_644_437);
    let random: Vec<[f64; 3]> = (0..SAMPLES)
        .map(|_| {
            let x = uniform_real(&mut engine);
            let y = uniform_real(&mut engine);
            let z = uniform_real(&mut engine);
            [x, y, z]
        })
        .collect();
    if keys.is_empty() {
        return (Vec::new(), Vec::new(), area_sum);
    }
    let total = f64::from(area_sum);
    let (positions, normals) = random
        .iter()
        .map(|r| {
            let t_sample = r[0] * total;
            let k = keys
                .partition_point(|&(key, _)| key <= t_sample)
                .min(keys.len() - 1);
            let [a, b, c] = corners(its.faces[keys[k].1]);
            let sq_u = r[1].sqrt();
            let v = r[2];
            let wa = (1.0 - sq_u) as f32;
            let wb = (sq_u * (1.0 - v)) as f32;
            let wc = (v * sq_u) as f32;
            let p = std::array::from_fn(|k| (a[k] * wa + b[k] * wb) + c[k] * wc);
            let n = normalized(cross(sub(b, a), sub(c, b)));
            (p, n)
        })
        .unzip();
    (positions, normals, area_sum)
}

/// The 5 x 5 stratified hemisphere directions of `raycast_visibility`.
fn directions() -> Vec<V3f> {
    let step = 1.0f32 / SQR_RAYS as f32;
    let mut result = Vec::with_capacity(SQR_RAYS * SQR_RAYS);
    for x in 0..SQR_RAYS {
        let sx = (f64::from(std::hint::black_box(x as f32) * step) + f64::from(step) / 2.0) as f32;
        for y in 0..SQR_RAYS {
            let sy =
                (f64::from(std::hint::black_box(y as f32) * step) + f64::from(step) / 2.0) as f32;
            // sample_hemisphere_uniform, with clang's contraction of `y - y * y` and `1 - 2 * y`.
            let term1 = 2.0f32 * std::f32::consts::PI * sx;
            let term2 = 2.0f32 * (-sy).mul_add(sy, sy).sqrt();
            result.push([
                term1.cos() * term2,
                term1.sin() * term2,
                (-2.0f32).mul_add(sy, 1.0).abs(),
            ]);
        }
    }
    result
}

/// Visibility of one sample: 1 minus 1/25 per ray that first meets a back-facing triangle.
fn raycast(
    its: &TriangleMesh,
    tree: &AabbTree,
    directions: &[V3f],
    center: V3f,
    normal: V3f,
) -> f32 {
    let decrease = 1.0f32 / (SQR_RAYS * SQR_RAYS) as f32;
    let mut result = 1.0f32;
    if tree.nodes.is_empty() {
        return result;
    }
    // Frame::set_from_z
    let z = normalized(normal);
    let tmp = if z[0].abs() > 0.99 {
        [0.0, 1.0, 0.0]
    } else {
        [1.0, 0.0, 0.0]
    };
    let y = normalized(cross(z, tmp));
    let x = cross(y, z);
    let origin: V3d = std::array::from_fn(|k| f64::from(center[k] + normal[k] * 0.01f32));
    for d in directions {
        let dir: V3f = std::array::from_fn(|k| (d[0] * x[k] + d[1] * y[k]) + d[2] * z[k]);
        let dir_d = to_d(dir);
        let ray = Ray {
            vertices: &its.vertices,
            faces: &its.faces,
            tree,
            origin,
            dir: dir_d,
            invdir: dir_d.map(|v| 1.0 / v),
        };
        if let Some(hit) = ray.first_hit(0, f64::INFINITY) {
            let [a, b, c] = its.faces[hit.id].map(|v| its.vertices[v as usize]);
            if dot(face_normal([a, b, c]), dir) <= 0.0 {
                result -= decrease;
            }
        }
    }
    result
}
