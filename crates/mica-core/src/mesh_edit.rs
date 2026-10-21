// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Mesh editing helpers: connected components, binary STL export, primitives, merging and
//! transformation.

use std::io::Write;

use crate::mesh::TriangleMesh;
use crate::mesh_cut::{WELD_TOLERANCE, compact_faces, weld_vertices};

/// Splits a mesh into its connected components (faces linked through shared vertices, after
/// merging vertices closer than 1e-5 mm). Each component is a compacted mesh; the result is sorted
/// by face count, largest first. A single-component mesh returns one clone of the input.
pub fn connected_components(mesh: &TriangleMesh) -> Vec<TriangleMesh> {
    if mesh.faces.is_empty() {
        return Vec::new();
    }
    let welded = weld_vertices(mesh, WELD_TOLERANCE);
    let mut uf = UnionFind::new(welded.vertices.len());
    for f in &welded.faces {
        uf.union(f[0] as usize, f[1] as usize);
        uf.union(f[0] as usize, f[2] as usize);
    }
    // Group faces by root, in order of first appearance.
    let mut group_of_root: Vec<Option<usize>> = vec![None; welded.vertices.len()];
    let mut groups: Vec<Vec<[u32; 3]>> = Vec::new();
    for f in &welded.faces {
        let root = uf.find(f[0] as usize);
        let g = *group_of_root[root].get_or_insert_with(|| {
            groups.push(Vec::new());
            groups.len() - 1
        });
        groups[g].push(*f);
    }
    if groups.len() == 1 {
        return vec![mesh.clone()];
    }
    groups.sort_by_key(|g| std::cmp::Reverse(g.len()));
    groups
        .iter()
        .map(|g| compact_faces(&welded.vertices, g))
        .collect()
}

struct UnionFind {
    parent: Vec<usize>,
}

impl UnionFind {
    fn new(n: usize) -> Self {
        Self {
            parent: (0..n).collect(),
        }
    }

    fn find(&mut self, mut i: usize) -> usize {
        while self.parent[i] != i {
            self.parent[i] = self.parent[self.parent[i]];
            i = self.parent[i];
        }
        i
    }

    fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            self.parent[ra.max(rb)] = ra.min(rb);
        }
    }
}

/// Writes `mesh` as binary STL (little-endian). The 80-byte header holds `name`, truncated; face
/// normals are computed from the vertex winding.
pub fn write_stl(mesh: &TriangleMesh, name: &str, out: &mut impl Write) -> std::io::Result<()> {
    let mut header = [0u8; 80];
    let bytes = name.as_bytes();
    let n = bytes.len().min(80);
    header[..n].copy_from_slice(&bytes[..n]);
    out.write_all(&header)?;
    out.write_all(&(mesh.faces.len() as u32).to_le_bytes())?;
    let mut record = [0u8; 50];
    for f in &mesh.faces {
        let [a, b, c] = f.map(|i| mesh.vertices[i as usize]);
        let normal = face_normal(a, b, c);
        for (k, v) in [normal, a, b, c].into_iter().enumerate() {
            for (j, coord) in v.into_iter().enumerate() {
                let o = k * 12 + j * 4;
                record[o..o + 4].copy_from_slice(&coord.to_le_bytes());
            }
        }
        out.write_all(&record)?;
    }
    Ok(())
}

/// Unit normal of a triangle; zero for degenerate triangles.
fn face_normal(a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> [f32; 3] {
    let [a, b, c] = [a, b, c].map(|v| v.map(f64::from));
    let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
    let n = [
        u[1] * v[2] - u[2] * v[1],
        u[2] * v[0] - u[0] * v[2],
        u[0] * v[1] - u[1] * v[0],
    ];
    let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
    if len > 0.0 {
        n.map(|c| (c / len) as f32)
    } else {
        [0.0; 3]
    }
}

/// Axis-aligned box of the given size centred at the origin.
pub fn cuboid(size: [f32; 3]) -> TriangleMesh {
    let h = size.map(|s| s / 2.0);
    // Corner i: bit 0 -> x, bit 1 -> y, bit 2 -> z.
    let corner = |i: u32| {
        [
            if i & 1 != 0 { h[0] } else { -h[0] },
            if i & 2 != 0 { h[1] } else { -h[1] },
            if i & 4 != 0 { h[2] } else { -h[2] },
        ]
    };
    let vertices: Vec<[f32; 3]> = (0..8).map(corner).collect();
    // Each quad counter-clockwise seen from outside.
    let quads: [[u32; 4]; 6] = [
        [0, 2, 3, 1], // -z
        [4, 5, 7, 6], // +z
        [0, 1, 5, 4], // -y
        [1, 3, 7, 5], // +x
        [3, 2, 6, 7], // +y
        [2, 0, 4, 6], // -x
    ];
    let mut faces = Vec::with_capacity(12);
    for [a, b, c, d] in quads {
        faces.push([a, b, c]);
        faces.push([a, c, d]);
    }
    TriangleMesh { vertices, faces }
}

/// Thin box; identical to [`cuboid`], kept as a named helper for modifier slabs.
pub fn slab(size: [f32; 3]) -> TriangleMesh {
    cuboid(size)
}

/// Cylinder along Z centred at the origin, with `segments` (at least 3) sides.
pub fn cylinder(radius: f32, height: f32, segments: usize) -> TriangleMesh {
    let s = segments.max(3);
    let (zb, zt) = (-height / 2.0, height / 2.0);
    let mut vertices = Vec::with_capacity(2 * s + 2);
    for z in [zb, zt] {
        for i in 0..s {
            let (sin, cos) = (std::f64::consts::TAU * i as f64 / s as f64).sin_cos();
            vertices.push([
                (f64::from(radius) * cos) as f32,
                (f64::from(radius) * sin) as f32,
                z,
            ]);
        }
    }
    let cb = vertices.len() as u32;
    vertices.push([0.0, 0.0, zb]);
    let ct = vertices.len() as u32;
    vertices.push([0.0, 0.0, zt]);
    let mut faces = Vec::with_capacity(4 * s);
    for i in 0..s {
        let j = (i + 1) % s;
        let (bi, bj) = (i as u32, j as u32);
        let (ti, tj) = ((s + i) as u32, (s + j) as u32);
        faces.push([bi, bj, tj]);
        faces.push([bi, tj, ti]);
        faces.push([cb, bj, bi]);
        faces.push([ct, ti, tj]);
    }
    TriangleMesh { vertices, faces }
}

/// UV sphere centred at the origin with `rings` (at least 2) latitude bands and `segments`
/// (at least 3) longitude steps.
pub fn sphere(radius: f32, rings: usize, segments: usize) -> TriangleMesh {
    let (r, s) = (rings.max(2), segments.max(3));
    let rad = f64::from(radius);
    let mut vertices = Vec::with_capacity((r - 1) * s + 2);
    vertices.push([0.0, 0.0, radius]);
    for k in 1..r {
        let phi = std::f64::consts::PI * k as f64 / r as f64;
        let (ring_r, z) = (rad * phi.sin(), rad * phi.cos());
        for i in 0..s {
            let (sin, cos) = (std::f64::consts::TAU * i as f64 / s as f64).sin_cos();
            vertices.push([(ring_r * cos) as f32, (ring_r * sin) as f32, z as f32]);
        }
    }
    let south = vertices.len() as u32;
    vertices.push([0.0, 0.0, -radius]);
    let ring = |k: usize, i: usize| (1 + (k - 1) * s + i % s) as u32;
    let mut faces = Vec::with_capacity(2 * s * (r - 1));
    for i in 0..s {
        faces.push([0, ring(1, i), ring(1, i + 1)]);
        faces.push([south, ring(r - 1, i + 1), ring(r - 1, i)]);
    }
    for k in 1..r - 1 {
        for i in 0..s {
            let (ai, aj) = (ring(k, i), ring(k, i + 1));
            let (bi, bj) = (ring(k + 1, i), ring(k + 1, i + 1));
            faces.push([ai, bi, bj]);
            faces.push([ai, bj, aj]);
        }
    }
    TriangleMesh { vertices, faces }
}

/// Concatenates meshes, re-indexing faces. No vertices are merged.
pub fn merge(meshes: &[&TriangleMesh]) -> TriangleMesh {
    let mut out = TriangleMesh {
        vertices: Vec::with_capacity(meshes.iter().map(|m| m.vertices.len()).sum()),
        faces: Vec::with_capacity(meshes.iter().map(|m| m.faces.len()).sum()),
    };
    for m in meshes {
        let base = out.vertices.len() as u32;
        out.vertices.extend_from_slice(&m.vertices);
        out.faces
            .extend(m.faces.iter().map(|f| f.map(|i| i + base)));
    }
    out
}

/// Applies a column-major 4x4 affine matrix (glam `Mat4::to_cols_array_2d` layout: `m[col][row]`).
/// Face winding is flipped when the linear part has a negative determinant, so the result stays
/// outward-facing.
pub fn transformed(mesh: &TriangleMesh, m: &[[f32; 4]; 4]) -> TriangleMesh {
    let vertices = mesh
        .vertices
        .iter()
        .map(|v| {
            let mut p = [m[3][0], m[3][1], m[3][2]];
            for c in 0..3 {
                for (r, out) in p.iter_mut().enumerate() {
                    *out += m[c][r] * v[c];
                }
            }
            p
        })
        .collect();
    let det = f64::from(m[0][0])
        * (f64::from(m[1][1]) * f64::from(m[2][2]) - f64::from(m[2][1]) * f64::from(m[1][2]))
        - f64::from(m[1][0])
            * (f64::from(m[0][1]) * f64::from(m[2][2]) - f64::from(m[2][1]) * f64::from(m[0][2]))
        + f64::from(m[2][0])
            * (f64::from(m[0][1]) * f64::from(m[1][2]) - f64::from(m[1][1]) * f64::from(m[0][2]));
    let faces = if det < 0.0 {
        mesh.faces.iter().map(|f| [f[0], f[2], f[1]]).collect()
    } else {
        mesh.faces.clone()
    };
    TriangleMesh { vertices, faces }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::tests::box_mesh;
    use crate::mesh_cut::is_watertight;

    fn face_positions(m: &TriangleMesh) -> Vec<[[f32; 3]; 3]> {
        m.faces
            .iter()
            .map(|f| f.map(|i| m.vertices[i as usize]))
            .collect()
    }

    #[test]
    fn cuboid_is_watertight_with_volume() {
        let m = cuboid([10.0, 20.0, 30.0]);
        assert!(is_watertight(&m));
        assert!((m.volume() - 6000.0).abs() < 1e-3);
        let (min, max) = m.bounds();
        assert_eq!(min, [-5.0, -10.0, -15.0]);
        assert_eq!(max, [5.0, 10.0, 15.0]);
        assert_eq!(slab([10.0, 10.0, 0.2]).faces.len(), 12);
    }

    #[test]
    fn cylinder_is_watertight_with_volume() {
        let m = cylinder(5.0, 10.0, 64);
        assert!(is_watertight(&m));
        let exact = std::f64::consts::PI * 25.0 * 10.0;
        let polygon = 0.5 * 64.0 * 25.0 * (std::f64::consts::TAU / 64.0).sin() * 10.0;
        assert!((m.volume() - polygon).abs() < 1e-3);
        assert!((m.volume() - exact).abs() / exact < 0.01);
        let (min, max) = m.bounds();
        assert!((min[2] + 5.0).abs() < 1e-6 && (max[2] - 5.0).abs() < 1e-6);
        assert!(is_watertight(&cylinder(1.0, 1.0, 1)));
    }

    #[test]
    fn sphere_is_watertight_with_volume() {
        let m = sphere(5.0, 16, 32);
        assert!(is_watertight(&m));
        let exact = 4.0 / 3.0 * std::f64::consts::PI * 125.0;
        assert!(m.volume() > 0.0);
        assert!((m.volume() - exact).abs() / exact < 0.05);
        assert!(is_watertight(&sphere(1.0, 1, 1)));
        assert!(sphere(1.0, 1, 1).volume() > 0.0);
    }

    #[test]
    fn components_of_two_boxes() {
        let a = box_mesh([0.0; 3], [10.0; 3]);
        let b = box_mesh([20.0, 0.0, 0.0], [30.0, 10.0, 10.0]);
        let both = merge(&[&a, &b]);
        assert_eq!(both.faces.len(), 24);
        assert_eq!(both.vertices.len(), 16);
        let parts = connected_components(&both);
        assert_eq!(parts.len(), 2);
        for p in &parts {
            assert_eq!(p.faces.len(), 12);
            assert_eq!(p.vertices.len(), 8);
            assert!(is_watertight(p));
            assert!((p.volume() - 1000.0).abs() < 1e-3);
        }
        let back = merge(&[&parts[0], &parts[1]]);
        assert_eq!(back.faces.len(), both.faces.len());
        assert!((back.volume() - both.volume()).abs() < 1e-3);
    }

    #[test]
    fn components_sorted_largest_first_and_single_is_clone() {
        let small = box_mesh([0.0; 3], [1.0; 3]);
        let big = cylinder(3.0, 2.0, 12);
        let mut shifted = big.clone();
        shifted.translate([10.0, 0.0, 0.0]);
        let parts = connected_components(&merge(&[&small, &shifted]));
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].faces.len(), 48);
        assert_eq!(parts[1].faces.len(), 12);
        let one = connected_components(&small);
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].faces, small.faces);
        assert_eq!(one[0].vertices, small.vertices);
        assert!(connected_components(&TriangleMesh::default()).is_empty());
    }

    #[test]
    fn components_join_through_coincident_vertices() {
        // Two boxes sharing a face but with separate vertex lists are one component.
        let a = box_mesh([0.0; 3], [10.0; 3]);
        let b = box_mesh([10.0, 0.0, 0.0], [20.0, 10.0, 10.0]);
        assert_eq!(connected_components(&merge(&[&a, &b])).len(), 1);
    }

    #[test]
    fn stl_round_trip() {
        let m = merge(&[
            &cylinder(4.0, 3.0, 9),
            &box_mesh([7.0; 3], [9.0, 10.0, 11.0]),
        ]);
        let mut bytes = Vec::new();
        write_stl(&m, "mica test solid", &mut bytes).unwrap();
        assert_eq!(bytes.len(), 84 + 50 * m.faces.len());
        assert!(bytes[..80].starts_with(b"mica test solid"));
        let back = TriangleMesh::from_stl_bytes(&bytes).unwrap();
        assert_eq!(back.faces.len(), m.faces.len());
        assert_eq!(face_positions(&back), face_positions(&m));
        // A name longer than the header is truncated, not an error.
        let mut bytes = Vec::new();
        write_stl(&m, &"x".repeat(200), &mut bytes).unwrap();
        assert_eq!(&bytes[..80], "x".repeat(80).as_bytes());
    }

    #[test]
    fn stl_normals_point_outward() {
        let m = cuboid([2.0, 2.0, 2.0]);
        let mut bytes = Vec::new();
        write_stl(&m, "", &mut bytes).unwrap();
        for (i, rec) in bytes[84..].as_chunks::<50>().0.iter().enumerate() {
            let f = |o: usize| f32::from_le_bytes([rec[o], rec[o + 1], rec[o + 2], rec[o + 3]]);
            let n = [f(0), f(4), f(8)];
            let [a, b, c] = m.faces[i].map(|v| m.vertices[v as usize]);
            let centroid = [
                (a[0] + b[0] + c[0]) / 3.0,
                (a[1] + b[1] + c[1]) / 3.0,
                (a[2] + b[2] + c[2]) / 3.0,
            ];
            let d = n[0] * centroid[0] + n[1] * centroid[1] + n[2] * centroid[2];
            assert!(
                (d - 1.0).abs() < 1e-5,
                "face {i}: normal {n:?} at {centroid:?}"
            );
        }
    }

    #[test]
    fn transformed_mirror_flips_winding() {
        let m = cuboid([4.0, 6.0, 8.0]);
        let mut mirror = [[0.0f32; 4]; 4];
        mirror[0][0] = -1.0;
        mirror[1][1] = 1.0;
        mirror[2][2] = 1.0;
        mirror[3][3] = 1.0;
        let t = transformed(&m, &mirror);
        assert!((t.volume() - m.volume()).abs() < 1e-3);
        assert!(is_watertight(&t));
        assert_ne!(t.faces, m.faces);
        // Translation column and scaling.
        let mut scale = [[0.0f32; 4]; 4];
        scale[0][0] = 2.0;
        scale[1][1] = 2.0;
        scale[2][2] = 2.0;
        scale[3] = [10.0, 20.0, 30.0, 1.0];
        let s = transformed(&m, &scale);
        assert_eq!(s.faces, m.faces);
        assert!((s.volume() - 8.0 * m.volume()).abs() < 1e-3);
        let (min, max) = s.bounds();
        assert_eq!(min, [6.0, 14.0, 22.0]);
        assert_eq!(max, [14.0, 26.0, 38.0]);
    }
}
