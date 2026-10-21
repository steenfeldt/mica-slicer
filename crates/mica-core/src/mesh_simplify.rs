// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: prusaslicer@b028299c77 src/libslic3r/QuadricEdgeCollapse.cpp :: its_quadric_edge_collapse, init, calculate_error, calculate_vertex, is_flipped, degenerate, create_no_volume, find_triangle_index1, reorder_edges, remove_triangle, change_neighbors, compact
// Source: prusaslicer@b028299c77 src/slic3r/GUI/Gizmos/GLGizmoSimplify.cpp :: GLGizmoSimplify::Configuration::fix_count_by_ratio, on_render_input_window
// Changes: Rust port on Mica's f32 indexed mesh; serial instead of TBB loops; a binary indexed heap instead of the 32-ary mutable mini-heap (equal errors may pop in another order); an AtomicBool cancel flag instead of throw_on_cancel and no status callback; the gizmo's ratio → triangle count rule and its five detail levels (max error) as helpers.
// Upstream attribution (QuadricEdgeCollapse.cpp):
// Copyright (c) Prusa Research 2021 - 2022 Pavel Mikuš @Godrak, Filip Sykala @Jony01, Lukáš Hejl @hejllukas, Lukáš Matěna @lukasmatena
//
// PrusaSlicer is released under the terms of the AGPLv3 or higher
//
// Paper: Garland & Heckbert, "Surface Simplification Using Quadric Error Metrics";
// inspiration: https://github.com/sp4cerat/Fast-Quadric-Mesh-Simplification

//! Mesh simplification by quadric edge collapse (PrusaSlicer's `its_quadric_edge_collapse`):
//! edges are collapsed cheapest first, by the summed plane quadrics of their end vertices,
//! until the triangle count reaches the target or the next collapse would cost more than
//! the allowed error. Collapses that would flip a triangle, create a degenerate one or a
//! zero-volume fin are skipped, so a closed mesh stays closed.

use crate::mesh::TriangleMesh;
use std::sync::atomic::{AtomicBool, Ordering};

/// The gizmo's "Level of detail" choices and their quadric error limits.
pub const DETAIL_LEVELS: [(&str, f32); 5] = [
    ("Extra high", 1e-3),
    ("High", 1e-2),
    ("Medium", 0.1),
    ("Low", 0.5),
    ("Extra low", 1.0),
];

/// Triangles left after removing `decimate_ratio` percent of `triangle_count`
/// (`GLGizmoSimplify::Configuration::fix_count_by_ratio`).
pub fn count_by_ratio(triangle_count: usize, decimate_ratio: f32) -> usize {
    if decimate_ratio <= 0. {
        triangle_count
    } else if decimate_ratio >= 100. {
        0
    } else {
        (triangle_count as f32 * (100. - decimate_ratio) / 100.).round() as usize
    }
}

/// Simplifies `mesh` towards `triangle_count` triangles, stopping early at `max_error`
/// (`None`: no limit). Returns `None` when cancelled.
pub fn simplify(
    mesh: &TriangleMesh,
    triangle_count: usize,
    max_error: Option<f32>,
    cancel: Option<&AtomicBool>,
) -> Option<TriangleMesh> {
    let mut m = mesh.clone();
    quadric_edge_collapse(&mut m, triangle_count as u32, max_error, cancel)?;
    Some(m)
}

#[derive(Clone, Copy, Default)]
struct SymMat([f64; 10]);

impl SymMat {
    fn plane(a: f64, b: f64, c: f64, d: f64) -> Self {
        SymMat([
            a * a,
            a * b,
            a * c,
            a * d,
            b * b,
            b * c,
            b * d,
            c * c,
            c * d,
            d * d,
        ])
    }
    #[allow(clippy::too_many_arguments)]
    fn det(
        &self,
        a11: usize,
        a12: usize,
        a13: usize,
        a21: usize,
        a22: usize,
        a23: usize,
        a31: usize,
        a32: usize,
        a33: usize,
    ) -> f64 {
        let m = &self.0;
        m[a11] * m[a22] * m[a33] + m[a13] * m[a21] * m[a32] + m[a12] * m[a23] * m[a31]
            - m[a13] * m[a22] * m[a31]
            - m[a11] * m[a23] * m[a32]
            - m[a12] * m[a21] * m[a33]
    }
    fn add(&mut self, o: &SymMat) {
        for i in 0..10 {
            self.0[i] += o.0[i];
        }
    }
}

#[derive(Clone, Copy, Default)]
struct Error {
    value: f32,
    triangle_index: u32,
}

#[derive(Clone, Copy, Default)]
struct TriangleInfo {
    n: [f32; 3],
    min_index: u8,
}
impl TriangleInfo {
    fn is_deleted(&self) -> bool {
        self.n[0] > 2.
    }
    fn set_deleted(&mut self) {
        self.n[0] = 3.;
    }
}

#[derive(Clone, Copy, Default)]
struct VertexInfo {
    q: SymMat,
    start: u32,
    count: u32,
}
impl VertexInfo {
    fn is_deleted(&self) -> bool {
        self.count == 0
    }
}

#[derive(Clone, Copy, Default)]
struct EdgeInfo {
    t_index: u32,
    edge: u8,
}

struct CopyEdgeInfo {
    start: u32,
    count: u32,
    shift: u32,
}

const CHECK_CANCEL_PERIOD: u32 = 16;

type Vertices = Vec<[f32; 3]>;
type Indices = Vec<[u32; 3]>;

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn normalized(a: [f64; 3]) -> [f64; 3] {
    let l = dot(a, a).sqrt();
    if l > 0. { a.map(|c| c / l) } else { a }
}
fn d3(v: [f32; 3]) -> [f64; 3] {
    v.map(f64::from)
}
fn subf(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn normf(a: [f32; 3]) -> [f32; 3] {
    let l = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt();
    if l > 0. { a.map(|c| c / l) } else { a }
}
fn dotf(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn crossf(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn create_normal(t: &[u32; 3], vertices: &Vertices) -> [f64; 3] {
    let v0 = d3(vertices[t[0] as usize]);
    let v1 = d3(vertices[t[1] as usize]);
    let v2 = d3(vertices[t[2] as usize]);
    normalized(cross(sub(v1, v0), sub(v2, v0)))
}

fn calculate_determinant(q: &SymMat) -> f64 {
    q.det(0, 1, 2, 1, 4, 5, 2, 5, 7)
}

fn calculate_vertex_det(det: f64, q: &SymMat) -> [f64; 3] {
    let det_1 = -1. / det;
    let det_x = q.det(1, 2, 3, 4, 5, 6, 5, 7, 8);
    let det_y = q.det(0, 2, 3, 1, 5, 6, 2, 7, 8);
    let det_z = q.det(0, 1, 3, 1, 4, 6, 2, 5, 8);
    [det_1 * det_x, -det_1 * det_y, det_1 * det_z]
}

fn create_vertices(id_v1: u32, id_v2: u32, vertices: &Vertices) -> [[f64; 3]; 3] {
    let v0 = d3(vertices[id_v1 as usize]);
    let v1 = d3(vertices[id_v2 as usize]);
    let vm = [
        (v0[0] + v1[0]) / 2.,
        (v0[1] + v1[1]) / 2.,
        (v0[2] + v1[2]) / 2.,
    ];
    [v0, v1, vm]
}

fn vertex_error(q: &SymMat, v: [f64; 3]) -> f64 {
    let q = &q.0;
    let (x, y, z) = (v[0], v[1], v[2]);
    q[0] * x * x
        + 2. * q[1] * x * y
        + 2. * q[2] * x * z
        + 2. * q[3] * x
        + q[4] * y * y
        + 2. * q[5] * y * z
        + 2. * q[6] * y
        + q[7] * z * z
        + 2. * q[8] * z
        + q[9]
}

fn calculate_error(id_v1: u32, id_v2: u32, q: &SymMat, vertices: &Vertices) -> f64 {
    let det = calculate_determinant(q);
    if det.abs() < f64::EPSILON {
        let verts = create_vertices(id_v1, id_v2, vertices);
        return verts
            .iter()
            .map(|&v| vertex_error(q, v))
            .fold(f64::INFINITY, f64::min);
    }
    vertex_error(q, calculate_vertex_det(det, q))
}

fn calculate_vertex(id_v1: u32, id_v2: u32, q: &SymMat, vertices: &Vertices) -> [f32; 3] {
    let det = calculate_determinant(q);
    if det.abs() < f64::EPSILON {
        let verts = create_vertices(id_v1, id_v2, vertices);
        let errors = verts.map(|v| vertex_error(q, v));
        // First minimum, like std::min_element.
        let mut best = 0;
        for i in 1..3 {
            if errors[i] < errors[best] {
                best = i;
            }
        }
        return verts[best].map(|c| c as f32);
    }
    calculate_vertex_det(det, q).map(|c| c as f32)
}

fn create_quadric(t: &[u32; 3], n: [f64; 3], vertices: &Vertices) -> SymMat {
    let v0 = d3(vertices[t[0] as usize]);
    SymMat::plane(n[0], n[1], n[2], -dot(n, v0))
}

fn calculate_3errors(t: &[u32; 3], vertices: &Vertices, v_infos: &[VertexInfo]) -> [f64; 3] {
    let mut error = [0.; 3];
    for j in 0..3 {
        let j2 = if j == 2 { 0 } else { j + 1 };
        let (vi0, vi1) = (t[j], t[j2]);
        let mut q = v_infos[vi0 as usize].q;
        q.add(&v_infos[vi1 as usize].q);
        error[j] = calculate_error(vi0, vi1, &q, vertices);
    }
    error
}

fn calculate_error_t(
    ti: u32,
    t: &[u32; 3],
    vertices: &Vertices,
    v_infos: &[VertexInfo],
    min_index: &mut u8,
) -> Error {
    let error = calculate_3errors(t, vertices, v_infos);
    *min_index = if error[0] < error[1] {
        if error[0] < error[2] { 0 } else { 2 }
    } else if error[1] < error[2] {
        1
    } else {
        2
    };
    Error {
        value: error[*min_index as usize] as f32,
        triangle_index: ti,
    }
}

/// Min-heap of triangle errors with each triangle's heap slot tracked, so entries can be
/// updated and removed in place.
struct Mpq {
    heap: Vec<Error>,
    pos: Vec<usize>,
}

impl Mpq {
    fn new(n: usize) -> Self {
        Self {
            heap: Vec::with_capacity(n),
            pos: vec![0; n],
        }
    }
    fn set(&mut self, i: usize, e: Error) {
        self.pos[e.triangle_index as usize] = i;
        self.heap[i] = e;
    }
    fn swap(&mut self, a: usize, b: usize) {
        self.heap.swap(a, b);
        self.pos[self.heap[a].triangle_index as usize] = a;
        self.pos[self.heap[b].triangle_index as usize] = b;
    }
    fn up(&mut self, mut i: usize) {
        while i > 0 {
            let p = (i - 1) / 2;
            if self.heap[i].value < self.heap[p].value {
                self.swap(i, p);
                i = p;
            } else {
                break;
            }
        }
    }
    fn down(&mut self, mut i: usize) {
        loop {
            let (l, r) = (2 * i + 1, 2 * i + 2);
            let mut m = i;
            if l < self.heap.len() && self.heap[l].value < self.heap[m].value {
                m = l;
            }
            if r < self.heap.len() && self.heap[r].value < self.heap[m].value {
                m = r;
            }
            if m == i {
                break;
            }
            self.swap(i, m);
            i = m;
        }
    }
    fn push(&mut self, e: Error) {
        self.heap.push(e);
        let i = self.heap.len() - 1;
        self.pos[e.triangle_index as usize] = i;
        self.up(i);
    }
    fn top(&self) -> Option<Error> {
        self.heap.first().copied()
    }
    fn remove(&mut self, i: usize) {
        let last = self.heap.len() - 1;
        if i != last {
            self.swap(i, last);
        }
        self.heap.pop();
        if i < self.heap.len() {
            self.up(i);
            self.down(i);
        }
    }
    fn pop(&mut self) {
        self.remove(0);
    }
    fn update(&mut self, i: usize) {
        self.up(i);
        self.down(i);
    }
}

fn init(
    its: &TriangleMesh,
) -> (
    Vec<TriangleInfo>,
    Vec<VertexInfo>,
    Vec<EdgeInfo>,
    Vec<Error>,
) {
    let mut t_infos = vec![TriangleInfo::default(); its.faces.len()];
    let mut v_infos = vec![VertexInfo::default(); its.vertices.len()];
    {
        let mut triangle_quadrics = vec![SymMat::default(); its.faces.len()];
        for (i, t) in its.faces.iter().enumerate() {
            let normal = create_normal(t, &its.vertices);
            t_infos[i].n = normal.map(|c| c as f32);
            triangle_quadrics[i] = create_quadric(t, normal, &its.vertices);
        }
        for (i, t) in its.faces.iter().enumerate() {
            let q = triangle_quadrics[i];
            for e in 0..3 {
                let v = &mut v_infos[t[e] as usize];
                v.q.add(&q);
                v.count += 1;
            }
        }
    }
    let mut triangle_start = 0u32;
    for v in &mut v_infos {
        v.start = triangle_start;
        triangle_start += v.count;
        v.count = 0;
    }
    let mut errors = vec![Error::default(); its.faces.len()];
    for (i, t) in its.faces.iter().enumerate() {
        errors[i] = calculate_error_t(
            i as u32,
            t,
            &its.vertices,
            &v_infos,
            &mut t_infos[i].min_index,
        );
    }
    let mut e_infos = vec![EdgeInfo::default(); its.faces.len() * 3];
    for (i, t) in its.faces.iter().enumerate() {
        for j in 0..3 {
            let v = &mut v_infos[t[j] as usize];
            let ei = (v.start + v.count) as usize;
            e_infos[ei] = EdgeInfo {
                t_index: i as u32,
                edge: j as u8,
            };
            v.count += 1;
        }
    }
    (t_infos, v_infos, e_infos, errors)
}

fn find_triangle_index1(
    vi: u32,
    v_info: &VertexInfo,
    ti0: u32,
    e_infos: &[EdgeInfo],
    indices: &Indices,
) -> Option<u32> {
    let end = v_info.start + v_info.count;
    for ei in v_info.start..end {
        let e = e_infos[ei as usize];
        if e.t_index == ti0 {
            continue;
        }
        let t = &indices[e.t_index as usize];
        if t[((e.edge + 1) % 3) as usize] == vi || t[((e.edge + 2) % 3) as usize] == vi {
            return Some(e.t_index);
        }
    }
    None
}

fn reorder_edges(e_infos: &mut [EdgeInfo], v_info: &VertexInfo, ti0: u32, ti1: u32) {
    let end = (v_info.start + v_info.count - 2) as usize;
    let (i0, i1) = (end, end + 1);
    let mut swapped = false;
    for ei in v_info.start as usize..end {
        if e_infos[ei].t_index == ti0 {
            e_infos.swap(ei, i0);
            if swapped {
                return;
            }
            if e_infos[ei].t_index == ti1 {
                e_infos.swap(ei, i1);
                return;
            }
            swapped = true;
        } else if e_infos[ei].t_index == ti1 {
            e_infos.swap(ei, i1);
            if swapped {
                return;
            }
            if e_infos[ei].t_index == ti0 {
                e_infos.swap(ei, i0);
                return;
            }
            swapped = true;
        }
    }
}

fn is_flipped(
    new_vertex: [f32; 3],
    v_info: &VertexInfo,
    t_infos: &[TriangleInfo],
    e_infos: &[EdgeInfo],
    its: &TriangleMesh,
) -> bool {
    let triangle_beauty_threshold = 1.0f32 - f32::EPSILON;
    let dot_thr = 0.2f32;
    let end = v_info.start + v_info.count - 2;
    for ei in v_info.start..end {
        let e = e_infos[ei as usize];
        let t = &its.faces[e.t_index as usize];
        let normal = t_infos[e.t_index as usize].n;
        let vf = its.vertices[t[((e.edge + 1) % 3) as usize] as usize];
        let vs = its.vertices[t[((e.edge + 2) % 3) as usize] as usize];
        let d1 = normf(subf(vf, new_vertex));
        let d2 = normf(subf(vs, new_vertex));
        let d = dotf(d1, d2);
        if d > triangle_beauty_threshold || d < -triangle_beauty_threshold {
            let v_orig = its.vertices[t[(e.edge % 3) as usize] as usize];
            let d1o = normf(subf(vf, v_orig));
            let d2o = normf(subf(vs, v_orig));
            if dotf(d1o, d2o).abs() < d.abs() {
                return true;
            }
        }
        let n = normf(crossf(d1, d2));
        if dotf(n, normal) < dot_thr {
            return true;
        }
    }
    false
}

fn degenerate(vi: u32, v_info: &VertexInfo, e_infos: &[EdgeInfo], indices: &Indices) -> bool {
    let end = v_info.start + v_info.count - 2;
    for ei in v_info.start..end {
        let t = &indices[e_infos[ei as usize].t_index as usize];
        if t.contains(&vi) {
            return true;
        }
    }
    false
}

fn create_no_volume(
    vi0: u32,
    vi1: u32,
    v_info0: &VertexInfo,
    v_info1: &VertexInfo,
    e_infos: &[EdgeInfo],
    indices: &Indices,
) -> bool {
    let end0 = v_info0.start + v_info0.count - 2;
    let end1 = v_info1.start + v_info1.count - 2;
    for ei0 in v_info0.start..end0 {
        let t0 = &indices[e_infos[ei0 as usize].t_index as usize];
        let mut t0i = 0usize;
        let mut t0vi0 = t0[t0i];
        if t0vi0 == vi0 {
            t0i += 1;
            t0vi0 = t0[t0i];
        }
        t0i += 1;
        let mut t0vi1 = t0[t0i];
        if t0vi1 == vi0 {
            t0i += 1;
            t0vi1 = t0[t0i];
        }
        for ei1 in v_info1.start..end1 {
            let t1 = &indices[e_infos[ei1 as usize].t_index as usize];
            let Some(mut t1i) = t1.iter().position(|&v| v == t0vi1) else {
                continue;
            };
            t1i = (t1i + 1) % 3;
            if t1[t1i] == vi1 {
                t1i = (t1i + 1) % 3;
            }
            if t1[t1i] == t0vi0 {
                return true;
            }
        }
    }
    false
}

fn remove_triangle(e_infos: &mut [EdgeInfo], v_info: &mut VertexInfo, ti: u32) {
    let start = v_info.start as usize;
    let last = start + v_info.count as usize - 1;
    for ei in start..last {
        if e_infos[ei].t_index == ti {
            e_infos[ei] = e_infos[last];
            v_info.count -= 1;
            return;
        }
    }
    debug_assert_eq!(e_infos[last].t_index, ti);
    v_info.count -= 1;
}

#[allow(clippy::too_many_arguments)]
fn change_neighbors(
    e_infos: &mut [EdgeInfo],
    v_infos: &mut [VertexInfo],
    ti0: u32,
    ti1: u32,
    vi0: u32,
    vi1: u32,
    vi_top0: u32,
    t1: [u32; 3],
    infos: &mut Vec<CopyEdgeInfo>,
    e_infos1: &mut Vec<EdgeInfo>,
) {
    debug_assert!(vi0 < vi1);
    let mut vi_top1 = t1[0];
    if vi_top1 == vi0 || vi_top1 == vi1 {
        vi_top1 = t1[1];
        if vi_top1 == vi0 || vi_top1 == vi1 {
            vi_top1 = t1[2];
        }
    }
    remove_triangle(e_infos, &mut v_infos[vi_top0 as usize], ti0);
    remove_triangle(e_infos, &mut v_infos[vi_top1 as usize], ti1);

    let new_triangle_count = v_infos[vi0 as usize].count + v_infos[vi1 as usize].count - 4;
    remove_triangle(e_infos, &mut v_infos[vi0 as usize], ti0);
    remove_triangle(e_infos, &mut v_infos[vi0 as usize], ti1);

    // Move the second vertex's edge infos out, to free their space.
    e_infos1.clear();
    let v1 = v_infos[vi1 as usize];
    for ei in v1.start..v1.start + v1.count {
        let e = e_infos[ei as usize];
        if e.t_index == ti0 || e.t_index == ti1 {
            continue;
        }
        e_infos1.push(e);
    }
    v_infos[vi1 as usize].count = 0;

    let v0 = v_infos[vi0 as usize];
    let mut need = new_triangle_count.saturating_sub(v0.count);

    let mut act_vi = vi0 + 1;
    let mut act_start = v_infos[act_vi as usize].start;
    let mut last_end = v0.start + v0.count;

    infos.clear();
    loop {
        let save = act_start - last_end;
        if save > 0 {
            if save >= need {
                break;
            }
            need -= save;
            let a = v_infos[act_vi as usize];
            infos.push(CopyEdgeInfo {
                start: a.start,
                count: a.count,
                shift: need,
            });
        } else if let Some(last) = infos.last_mut() {
            // Contiguous with the previous block: it moves with it.
            last.count += v_infos[act_vi as usize].count;
        }
        let a = v_infos[act_vi as usize];
        last_end = a.start + a.count;
        v_infos[act_vi as usize].start += need;
        act_vi += 1;
        if (act_vi as usize) < v_infos.len() {
            act_start = v_infos[act_vi as usize].start;
        } else {
            act_start = e_infos.len() as u32;
        }
    }

    for c in infos.iter().rev() {
        for ei in (c.start..c.start + c.count).rev() {
            e_infos[(ei + c.shift) as usize] = e_infos[ei as usize];
        }
    }

    for &e in e_infos1.iter() {
        let v0 = &mut v_infos[vi0 as usize];
        let ei_f = v0.start + v0.count;
        e_infos[ei_f as usize] = e;
        v0.count += 1;
    }
}

fn compact(
    v_infos: &[VertexInfo],
    t_infos: &[TriangleInfo],
    e_infos: &[EdgeInfo],
    its: &mut TriangleMesh,
) {
    let mut vi_new = 0usize;
    for (vi, v) in v_infos.iter().enumerate() {
        if v.is_deleted() {
            continue;
        }
        for ei in v.start..v.start + v.count {
            let e = e_infos[ei as usize];
            its.faces[e.t_index as usize][e.edge as usize] = vi_new as u32;
        }
        its.vertices[vi_new] = its.vertices[vi];
        vi_new += 1;
    }
    its.vertices.truncate(vi_new);
    let mut ti_new = 0usize;
    for (ti, t) in t_infos.iter().enumerate() {
        if t.is_deleted() {
            continue;
        }
        its.faces[ti_new] = its.faces[ti];
        ti_new += 1;
    }
    its.faces.truncate(ti_new);
}

/// In place; `None` when cancelled (the mesh is then left half-done).
fn quadric_edge_collapse(
    its: &mut TriangleMesh,
    triangle_count: u32,
    max_error: Option<f32>,
    cancel: Option<&AtomicBool>,
) -> Option<()> {
    if triangle_count as usize >= its.faces.len() {
        return Some(());
    }
    let maximal_error = max_error.unwrap_or(f32::MAX);
    if maximal_error <= 0. {
        return Some(());
    }
    let cancelled = || cancel.is_some_and(|c| c.load(Ordering::Relaxed));
    let (mut t_infos, mut v_infos, mut e_infos, errors) = init(its);
    if cancelled() {
        return None;
    }
    let mut mpq = Mpq::new(its.faces.len());
    for e in errors {
        mpq.push(e);
    }
    let mut ceis: Vec<CopyEdgeInfo> = Vec::with_capacity(50);
    let mut e_infos_swap: Vec<EdgeInfo> = Vec::with_capacity(50);
    let mut changed: Vec<u32> = Vec::with_capacity(100);

    let mut actual_triangle_count = its.faces.len() as u32;
    let mut iteration_number = 0u32;
    while actual_triangle_count > triangle_count {
        let Some(mut e) = mpq.top() else { break };
        iteration_number += 1;
        if iteration_number.is_multiple_of(CHECK_CANCEL_PERIOD) && cancelled() {
            return None;
        }
        if e.value >= maximal_error {
            break;
        }
        mpq.pop();
        let ti0 = e.triangle_index;
        if t_infos[ti0 as usize].is_deleted() {
            continue;
        }
        let min_index = t_infos[ti0 as usize].min_index as usize;
        let t0 = its.faces[ti0 as usize];
        let mut vi0 = t0[min_index];
        let mut vi1 = t0[(min_index + 1) % 3];
        if vi0 > vi1 {
            std::mem::swap(&mut vi0, &mut vi1);
        }
        let mut q = v_infos[vi0 as usize].q;
        q.add(&v_infos[vi1 as usize].q);
        let new_vertex0 = calculate_vertex(vi0, vi1, &q, &its.vertices);
        let (vin0, vin1) = (v_infos[vi0 as usize], v_infos[vi1 as usize]);
        let ti1_opt = if vin0.count < vin1.count {
            find_triangle_index1(vi1, &vin0, ti0, &e_infos, &its.faces)
        } else {
            find_triangle_index1(vi0, &vin1, ti0, &e_infos, &its.faces)
        };
        let mut ti1 = u32::MAX;
        if let Some(t) = ti1_opt {
            ti1 = t;
            reorder_edges(&mut e_infos, &vin0, ti0, ti1);
            reorder_edges(&mut e_infos, &vin1, ti0, ti1);
        }
        let (vin0, vin1) = (v_infos[vi0 as usize], v_infos[vi1 as usize]);
        if ti1_opt.is_none()
            || degenerate(vi0, &vin1, &e_infos, &its.faces)
            || degenerate(vi1, &vin0, &e_infos, &its.faces)
            || create_no_volume(vi0, vi1, &vin0, &vin1, &e_infos, &its.faces)
            || is_flipped(new_vertex0, &vin0, &t_infos, &e_infos, its)
            || is_flipped(new_vertex0, &vin1, &t_infos, &e_infos, its)
        {
            // Try the triangle's other edges.
            let errors = calculate_3errors(&t0, &its.vertices, &v_infos);
            let ord: [u8; 3] = if errors[0] < errors[1] {
                if errors[0] < errors[2] {
                    if errors[1] < errors[2] {
                        [0, 1, 2]
                    } else {
                        [0, 2, 1]
                    }
                } else {
                    [2, 0, 1]
                }
            } else if errors[1] < errors[2] {
                if errors[0] < errors[2] {
                    [1, 0, 2]
                } else {
                    [1, 2, 0]
                }
            } else {
                [2, 1, 0]
            };
            let t_info0 = &mut t_infos[ti0 as usize];
            if t_info0.min_index == ord[0] {
                t_info0.min_index = ord[1];
                e.value = errors[t_info0.min_index as usize] as f32;
            } else if t_info0.min_index == ord[1] {
                t_info0.min_index = ord[2];
                e.value = errors[t_info0.min_index as usize] as f32;
            } else {
                t_info0.min_index = 3;
                e.value = maximal_error;
            }
            mpq.push(e);
            continue;
        }

        changed.clear();
        for di in vin0.start..vin0.start + vin0.count - 2 {
            changed.push(e_infos[di as usize].t_index);
        }
        for di in vin1.start..vin1.start + vin1.count - 2 {
            let ei = e_infos[di as usize];
            its.faces[ei.t_index as usize][ei.edge as usize] = vi0;
            changed.push(ei.t_index);
        }
        v_infos[vi0 as usize].q = q;

        let vi_top0 = t0[(min_index + 2) % 3];
        let t1 = its.faces[ti1 as usize];
        change_neighbors(
            &mut e_infos,
            &mut v_infos,
            ti0,
            ti1,
            vi0,
            vi1,
            vi_top0,
            t1,
            &mut ceis,
            &mut e_infos_swap,
        );
        its.vertices[vi0 as usize] = new_vertex0;

        mpq.remove(mpq.pos[ti1 as usize]);
        for &ti in &changed {
            let slot = mpq.pos[ti as usize];
            let t = its.faces[ti as usize];
            t_infos[ti as usize].n = create_normal(&t, &its.vertices).map(|c| c as f32);
            let mut mi = 0u8;
            let err = calculate_error_t(ti, &t, &its.vertices, &v_infos, &mut mi);
            t_infos[ti as usize].min_index = mi;
            mpq.set(slot, err);
            mpq.update(slot);
        }
        t_infos[ti0 as usize].set_deleted();
        t_infos[ti1 as usize].set_deleted();
        actual_triangle_count -= 2;
    }
    compact(&v_infos, &t_infos, &e_infos, its);
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh_edit::{cuboid, sphere};

    fn closed(m: &TriangleMesh) -> bool {
        let mut seen = std::collections::HashSet::new();
        for f in &m.faces {
            for k in 0..3 {
                if !seen.insert((f[k], f[(k + 1) % 3])) {
                    return false;
                }
            }
        }
        seen.iter().all(|&(u, v)| seen.contains(&(v, u)))
    }

    #[test]
    fn reaches_triangle_targets_and_stays_closed() {
        let s = sphere(20., 48, 96);
        let n = s.faces.len();
        for target in [n / 2, n / 4, n / 10, 200, 40] {
            let r = simplify(&s, target, None, None).unwrap();
            // Every collapse removes two triangles, so the count lands on or one below.
            assert!(
                r.faces.len() <= target && r.faces.len() + 2 > target,
                "{} for target {target}",
                r.faces.len()
            );
            assert!(closed(&r), "target {target}");
            let v = r.volume();
            let exact = 4. / 3. * std::f64::consts::PI * 8000.;
            let tol = if target >= 200 { 0.05 } else { 0.25 };
            assert!((v - exact).abs() / exact < tol, "volume {v} at {target}");
        }
    }

    #[test]
    fn ratio_and_detail_levels() {
        assert_eq!(count_by_ratio(1000, 0.), 1000);
        assert_eq!(count_by_ratio(1000, 50.), 500);
        assert_eq!(count_by_ratio(1000, 100.), 0);
        let s = sphere(10., 32, 64);
        let fine = simplify(&s, 0, Some(DETAIL_LEVELS[0].1), None).unwrap();
        let coarse = simplify(&s, 0, Some(DETAIL_LEVELS[4].1), None).unwrap();
        assert!(coarse.faces.len() < fine.faces.len());
        assert!(fine.faces.len() < s.faces.len());
        // Every collapse of a box's edge moves a corner off its planes: under a tight error
        // limit the box keeps all its triangles.
        let b = cuboid([10., 20., 30.]);
        let r = simplify(&b, 0, Some(DETAIL_LEVELS[0].1), None).unwrap();
        assert_eq!(r.faces.len(), 12);
        assert!((r.volume() - 6000.).abs() < 1e-3);
        // A finely tessellated flat face collapses without changing the shape.
        let mut grid = TriangleMesh::default();
        let n = 8u32;
        for j in 0..=n {
            for i in 0..=n {
                grid.vertices.push([i as f32, j as f32, 0.]);
            }
        }
        let at = |i: u32, j: u32| j * (n + 1) + i;
        for j in 0..n {
            for i in 0..n {
                grid.faces.push([at(i, j), at(i + 1, j), at(i + 1, j + 1)]);
                grid.faces.push([at(i, j), at(i + 1, j + 1), at(i, j + 1)]);
            }
        }
        let flat = simplify(&grid, 0, Some(1e-6), None).unwrap();
        assert!(flat.faces.len() < grid.faces.len() / 2);
        assert!(flat.vertices.iter().all(|v| v[2] == 0.));
    }

    #[test]
    fn cancel_stops() {
        let s = sphere(10., 32, 64);
        let flag = AtomicBool::new(true);
        assert!(simplify(&s, 10, None, Some(&flag)).is_none());
    }
}
