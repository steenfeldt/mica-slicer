// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: prusaslicer@b028299c77 src/libslic3r/ShortEdgeCollapse.cpp :: its_short_edge_collpase, stable_shuffle
// Source: prusaslicer@b028299c77 src/libslic3r/NormalUtils.cpp :: create_triangle_normal, create_normals_nelson_weighted
// Source: prusaslicer@b028299c77 src/libslic3r/MeshSplitImpl.hpp :: create_face_neighbors_index
// Source: prusaslicer@b028299c77 src/libslic3r/TriangleMesh.cpp :: VertexFaceIndex::create, its_face_normals
// Changes: Rust port in f32 with Eigen 3.3 evaluation order (three-term sums as a0 + (a1 + a2), cross products with the contracted multiply-add of Apple clang arm64 builds).
// Changes: Sequential face-neighbour index (identical to the TBB version on manifold meshes); boost uniform_int_distribution<int> on the MT19937-64 engine reimplemented.
// Copyright (c) Prusa Research 2022 - 2023 Vojtěch Bubník @bubnikv, Pavel Mikuš @Godrak

use super::random::Mt19937;
use crate::mesh::TriangleMesh;
use std::collections::HashMap;

pub(super) type V3f = [f32; 3];

pub(super) fn sub(a: V3f, b: V3f) -> V3f {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
/// Eigen's `cross` evaluates `a1 * b2 - a2 * b1` as one C++ expression, which clang contracts
/// into a fused multiply-add of the first product and the rounded second one.
pub(super) fn cross(a: V3f, b: V3f) -> V3f {
    [
        a[1].mul_add(b[2], -(a[2] * b[1])),
        a[2].mul_add(b[0], -(a[0] * b[2])),
        a[0].mul_add(b[1], -(a[1] * b[0])),
    ]
}
/// Eigen's unrolled reduction of three coefficients splits them as `c0 + (c1 + c2)`.
pub(super) fn dot(a: V3f, b: V3f) -> f32 {
    a[0] * b[0] + (a[1] * b[1] + a[2] * b[2])
}
pub(super) fn squared_norm(a: V3f) -> f32 {
    dot(a, a)
}
pub(super) fn norm(a: V3f) -> f32 {
    squared_norm(a).sqrt()
}
pub(super) fn normalized(a: V3f) -> V3f {
    let z = squared_norm(a);
    if z > 0.0 {
        let s = z.sqrt();
        [a[0] / s, a[1] / s, a[2] / s]
    } else {
        a
    }
}

/// `its_face_normal`: `(b - a) x (c - b)`, normalized twice like upstream.
pub(super) fn face_normal([a, b, c]: [V3f; 3]) -> V3f {
    normalized(normalized(cross(sub(b, a), sub(c, b))))
}

fn corners(mesh: &TriangleMesh, face: [u32; 3]) -> [V3f; 3] {
    face.map(|v| mesh.vertices[v as usize])
}

/// Neighbouring face across each edge (`edge i` runs from vertex `i` to `i + 1`), or -1.
fn face_neighbors(mesh: &TriangleMesh) -> Vec<[i32; 3]> {
    let faces = &mesh.faces;
    // VertexFaceIndex: faces incident to each vertex, in face order.
    let mut start = vec![0usize; mesh.vertices.len() + 1];
    for f in faces {
        for &v in f {
            start[v as usize + 1] += 1;
        }
    }
    for i in 2..start.len() {
        start[i] += start[i - 1];
    }
    let mut all = vec![0usize; *start.last().unwrap_or(&0)];
    let mut cursor = start.clone();
    for (face, f) in faces.iter().enumerate() {
        for &v in f {
            all[cursor[v as usize]] = face;
            cursor[v as usize] += 1;
        }
    }
    let mut neighbors = vec![[-1i32; 3]; faces.len()];
    for face in 0..faces.len() {
        let tri = faces[face];
        for edge in 0..3 {
            if neighbors[face][edge] != -1 {
                continue;
            }
            let (a, b) = (tri[edge], tri[if edge == 2 { 0 } else { edge + 1 }]);
            for &other in &all[start[a as usize]..start[a as usize + 1]] {
                if other <= face {
                    continue;
                }
                let o = faces[other];
                let Some(k) = o.iter().position(|&v| v == b) else {
                    continue;
                };
                if a != o[(k + 1) % 3] {
                    continue;
                }
                neighbors[face][edge] = other as i32;
                neighbors[other][k] = face as i32;
                break;
            }
        }
    }
    neighbors
}

/// `NormalUtils::create_normals` with the default Nelson-Max weighting (not renormalized).
fn vertex_normals(mesh: &TriangleMesh) -> Vec<V3f> {
    let mut normals = vec![[0.0f32; 3]; mesh.vertices.len()];
    let mut count = vec![0.0f32; mesh.vertices.len()];
    for &f in &mesh.faces {
        let [v0, v1, v2] = corners(mesh, f);
        let normal = normalized(cross(sub(v1, v0), sub(v2, v0)));
        let e0 = norm(sub(v0, v1));
        let e1 = norm(sub(v1, v2));
        let e2 = norm(sub(v2, v0));
        let coefs = [e0 * e2, e0 * e1, e1 * e2];
        for i in 0..3 {
            let n = &mut normals[f[i] as usize];
            for k in 0..3 {
                n[k] += normal[k] * coefs[i];
            }
            count[f[i] as usize] += coefs[i];
        }
    }
    for (n, c) in normals.iter_mut().zip(count) {
        for x in n {
            *x /= c;
        }
    }
    normals
}

/// `boost::random::uniform_int_distribution<int>{min, max}` drawn from a 64-bit engine.
fn uniform_int(generator: &mut Mt19937, min: i32, max: i32) -> i32 {
    let range = max.wrapping_sub(min) as u32 as u64;
    if range == 0 {
        return min;
    }
    let brange = u64::MAX;
    let mut bucket = brange / (range + 1);
    if brange % (range + 1) == range {
        bucket += 1;
    }
    loop {
        let result = generator.next() / bucket;
        if result <= range {
            return (result as u32).wrapping_add(min as u32) as i32;
        }
    }
}

fn stable_shuffle(range: &mut [usize], generator: &mut Mt19937) {
    let n = range.len() as i32;
    let mut i = 0;
    while i < n - 2 {
        let j = uniform_int(generator, i, n - 1);
        range.swap(i as usize, j as usize);
        i += 1;
    }
}

/// Decimates `mesh` by collapsing short edges until at most `target` faces remain, exactly like
/// PrusaSlicer's seam visibility preprocessing. Only coordinate differences enter the arithmetic,
/// so an exactly translated copy of a mesh decimates identically.
pub(super) fn short_edge_collapse(mesh: &mut TriangleMesh, target: usize) {
    let mut mapping: Vec<usize> = (0..mesh.vertices.len()).collect();
    let mut flatten = Vec::new();
    let mut final_index = |mapping: &mut Vec<usize>, orig: usize| {
        flatten.clear();
        let mut idx = orig;
        while mapping[idx] != idx {
            flatten.push(idx);
            idx = mapping[idx];
        }
        for &i in &flatten {
            mapping[i] = idx;
        }
        idx
    };
    let mut removed = vec![false; mesh.faces.len()];
    let mut neighbors = face_neighbors(mesh);

    let mut min_dot = vec![1.0f32; mesh.vertices.len()];
    {
        let vertex_normals = vertex_normals(mesh);
        for &t in &mesh.faces {
            let n = face_normal(corners(mesh, t));
            for v in t {
                let v = v as usize;
                min_dot[v] = min_dot[v].min(dot(n, vertex_normals[v]));
            }
        }
    }

    let remove_face = |neighbors: &mut [[i32; 3]], removed: &mut [bool], face: i32, other: i32| {
        if face < 0 {
            return;
        }
        removed[face as usize] = true;
        let n = neighbors[face as usize];
        let n_a = if n[0] != other { n[0] } else { n[1] };
        let n_b = if n[2] != other { n[2] } else { n[1] };
        // Upstream skips face 0 here (`> 0`), kept for identical results.
        if n_a > 0
            && let Some(x) = neighbors[n_a as usize].iter_mut().find(|x| **x == face)
        {
            *x = n_b;
        }
        if n_b > 0
            && let Some(x) = neighbors[n_b as usize].iter_mut().find(|x| **x == face)
        {
            *x = n_a;
        }
    };

    let mut generator = Mt19937::new(27_644_437);
    let mut face_indices: Vec<usize> = (0..mesh.faces.len()).collect();
    let mut kept = Vec::with_capacity(face_indices.len());
    let mut decimation_ratio = 1.0f32;
    let mut edge_len = 0.2f32;
    while face_indices.len() > target {
        edge_len = (f64::from(edge_len) * (2.0 - f64::from(decimation_ratio))) as f32;
        let max_edge_len_squared = edge_len * edge_len;
        stable_shuffle(&mut face_indices, &mut generator);
        let mut allowed = face_indices.len() as i32 - target as i32;
        for &face in &face_indices {
            if removed[face] {
                continue;
            }
            for edge in 0..3 {
                let mut keep = final_index(&mut mapping, mesh.faces[face][edge] as usize);
                let mut remove =
                    final_index(&mut mapping, mesh.faces[face][(edge + 1) % 3] as usize);
                if squared_norm(sub(mesh.vertices[keep], mesh.vertices[remove]))
                    > max_edge_len_squared
                {
                    continue;
                }
                if min_dot[remove] < min_dot[keep] {
                    std::mem::swap(&mut keep, &mut remove);
                }
                mapping[remove] = mapping[keep];
                let other = neighbors[face][edge];
                remove_face(&mut neighbors, &mut removed, face as i32, other);
                remove_face(&mut neighbors, &mut removed, other, face as i32);
                allowed -= 2;
                break;
            }
            if allowed <= 0 {
                break;
            }
        }
        let previous = face_indices.len();
        kept.clear();
        kept.extend(face_indices.iter().copied().filter(|&f| !removed[f]));
        std::mem::swap(&mut face_indices, &mut kept);
        decimation_ratio = (previous - face_indices.len()) as f32 / previous as f32;
    }

    let mut final_vertices_mapping: HashMap<usize, u32> = HashMap::new();
    let mut vertices = Vec::new();
    let mut faces = Vec::with_capacity(face_indices.len());
    for &idx in &face_indices {
        let f = mesh.faces[idx].map(|v| final_index(&mut mapping, v as usize));
        if f[0] == f[1] || f[1] == f[2] || f[2] == f[0] {
            continue;
        }
        faces.push(f.map(|v| {
            *final_vertices_mapping.entry(v).or_insert_with(|| {
                vertices.push(mesh.vertices[v]);
                (vertices.len() - 1) as u32
            })
        }));
    }
    mesh.vertices = vertices;
    mesh.faces = faces;
}
