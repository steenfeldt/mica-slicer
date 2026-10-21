// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Boolean tests on difficult inputs: coplanar faces, touching faces and edges, identical
//! meshes, nested and disjoint shapes, curved and rotated meshes. Every result must be
//! closed, and volumes must add up (|A ∪ B| + |A ∩ B| = |A| + |B|).

use super::*;
use crate::mesh_edit::{cuboid, cylinder, sphere, transformed};

fn boxm(lo: [f32; 3], hi: [f32; 3]) -> TriangleMesh {
    let mut m = cuboid([hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]]);
    m.translate([
        (lo[0] + hi[0]) / 2.,
        (lo[1] + hi[1]) / 2.,
        (lo[2] + hi[2]) / 2.,
    ]);
    m
}

/// Every directed edge is matched by its reverse (index-wise), counting multiplicity.
fn closed(m: &TriangleMesh) -> bool {
    let mut count: HashMap<(u32, u32), i32> = HashMap::new();
    for f in &m.faces {
        for k in 0..3 {
            let (u, v) = (f[k], f[(k + 1) % 3]);
            if u < v {
                *count.entry((u, v)).or_default() += 1;
            } else {
                *count.entry((v, u)).or_default() -= 1;
            }
        }
    }
    count.values().all(|&c| c == 0)
}

/// Closed and every directed edge used once (a 2-manifold).
fn manifold(m: &TriangleMesh) -> bool {
    closed(m) && closed_manifold(&m.faces)
}

fn run(a: &TriangleMesh, b: &TriangleMesh, op: BooleanOp) -> TriangleMesh {
    let r = boolean(a, b, op).unwrap_or_else(|e| panic!("{op:?}: {e}"));
    assert!(closed(&r), "{op:?} result is open");
    r
}

/// The three operations, their closedness and the volume identities.
fn check_all(a: &TriangleMesh, b: &TriangleMesh, tol: f64) -> [TriangleMesh; 3] {
    let u = run(a, b, BooleanOp::Union);
    let d = run(a, b, BooleanOp::Difference);
    let i = run(a, b, BooleanOp::Intersection);
    let (va, vb) = (a.volume(), b.volume());
    let (vu, vd, vi) = (u.volume(), d.volume(), i.volume());
    assert!(
        (vu + vi - va - vb).abs() < tol,
        "union {vu} + intersection {vi} != {va} + {vb}"
    );
    assert!(
        (vd - (va - vi)).abs() < tol,
        "difference {vd} != {va} - {vi}"
    );
    [u, d, i]
}

#[test]
fn overlapping_cubes_in_general_position() {
    let a = boxm([0., 0., 0.], [20., 20., 20.]);
    let b = boxm([10., 7., 5.], [30., 25., 15.]);
    let [u, d, i] = check_all(&a, &b, 1e-3);
    assert!((i.volume() - 10. * 13. * 10.).abs() < 1e-3);
    assert!((u.volume() - (8000. + 20. * 18. * 10. - 1300.)).abs() < 1e-3);
    assert!((d.volume() - (8000. - 1300.)).abs() < 1e-3);
    for m in [&u, &d, &i] {
        assert!(manifold(m));
    }
}

#[test]
fn coplanar_top_and_bottom_through_hole() {
    // B is flush with A's top and bottom faces: the difference is a tube.
    let a = boxm([0., 0., 0.], [20., 20., 20.]);
    let b = boxm([5., 5., 0.], [15., 15., 20.]);
    let [u, d, i] = check_all(&a, &b, 1e-3);
    assert!((u.volume() - 8000.).abs() < 1e-3);
    assert!((i.volume() - 2000.).abs() < 1e-3);
    assert!((d.volume() - 6000.).abs() < 1e-3);
    for m in [&u, &d, &i] {
        assert!(manifold(m));
    }
    // A through hole: genus 1, so V - E + F = 0.
    let edges = d.faces.len() * 3 / 2;
    assert_eq!(
        d.vertices.len() as i64 - edges as i64 + d.faces.len() as i64,
        0
    );
}

#[test]
fn coplanar_side_faces_shared_partially() {
    // B shares A's side plane x = 20 from outside (opposite normals) and A's top plane.
    let a = boxm([0., 0., 0.], [20., 20., 20.]);
    let b = boxm([20., 5., 10.], [30., 15., 20.]);
    let [u, d, i] = check_all(&a, &b, 1e-3);
    assert!((u.volume() - 9000.).abs() < 1e-3);
    assert!(i.faces.is_empty() || i.volume().abs() < 1e-6);
    assert!((d.volume() - 8000.).abs() < 1e-3);
    assert!(manifold(&u));
}

#[test]
fn stacked_cubes_touching_faces() {
    let a = boxm([0., 0., 0.], [10., 10., 10.]);
    let b = boxm([3., 3., 10.], [13., 13., 20.]);
    let [u, d, i] = check_all(&a, &b, 1e-3);
    assert!((u.volume() - 2000.).abs() < 1e-3);
    assert!(i.volume().abs() < 1e-6);
    assert!((d.volume() - 1000.).abs() < 1e-3);
    assert!(manifold(&u));
    assert!(manifold(&d));
}

#[test]
fn cubes_touching_along_an_edge_and_a_corner() {
    let a = boxm([0., 0., 0.], [10., 10., 10.]);
    let edge = boxm([10., 10., 0.], [20., 20., 10.]);
    let [u, _, i] = check_all(&a, &edge, 1e-3);
    assert!((u.volume() - 2000.).abs() < 1e-3);
    assert!(i.volume().abs() < 1e-6);
    let corner = boxm([10., 10., 10.], [20., 20., 20.]);
    let [u, _, _] = check_all(&a, &corner, 1e-3);
    assert!((u.volume() - 2000.).abs() < 1e-3);
}

#[test]
fn identical_meshes() {
    let a = boxm([0., 0., 0.], [10., 10., 10.]);
    let [u, d, i] = check_all(&a, &a.clone(), 1e-3);
    assert!((u.volume() - 1000.).abs() < 1e-3);
    assert!((i.volume() - 1000.).abs() < 1e-3);
    assert!(d.faces.is_empty());
}

#[test]
fn nested_and_disjoint() {
    let a = boxm([0., 0., 0.], [20., 20., 20.]);
    let inner = boxm([5., 5., 5.], [10., 10., 10.]);
    let [u, d, i] = check_all(&a, &inner, 1e-3);
    assert!((u.volume() - 8000.).abs() < 1e-3);
    assert!((i.volume() - 125.).abs() < 1e-3);
    // A hollow box: the cavity's faces point inwards.
    assert!((d.volume() - 7875.).abs() < 1e-3);
    assert_eq!(d.faces.len(), 24);
    let far = boxm([50., 0., 0.], [60., 10., 10.]);
    let [u, d, i] = check_all(&a, &far, 1e-3);
    assert_eq!(u.faces.len(), 24);
    assert!((d.volume() - 8000.).abs() < 1e-3);
    assert!(i.faces.is_empty());
}

#[test]
fn inner_box_touching_the_floor_from_inside() {
    // Same-orientation coplanar overlap (B's bottom on A's bottom, B inside A).
    let a = boxm([0., 0., 0.], [20., 20., 20.]);
    let b = boxm([5., 5., 0.], [10., 10., 10.]);
    let [u, d, i] = check_all(&a, &b, 1e-3);
    assert!((u.volume() - 8000.).abs() < 1e-3);
    assert!((i.volume() - 250.).abs() < 1e-3);
    assert!((d.volume() - 7750.).abs() < 1e-3);
    for m in [&u, &d, &i] {
        assert!(manifold(m));
    }
}

#[test]
fn sphere_minus_cylinder_and_rotated_cubes() {
    let s = sphere(10., 16, 32);
    let mut c = cylinder(4., 30., 24);
    c.translate([1., 0.5, 0.]);
    let [_, d, i] = check_all(&s, &c, 1e-2);
    assert!(d.volume() > 0. && i.volume() > 0.);
    assert!(manifold(&d) && manifold(&i));

    let a = boxm([0., 0., 0.], [20., 20., 20.]);
    let r = glam_rotation(0.3, 0.5, 0.7, [10., 10., 10.]);
    let b = transformed(&boxm([-8., -8., -8.], [8., 8., 8.]), &r);
    let [u, _, _] = check_all(&a, &b, 1e-2);
    assert!(manifold(&u));
}

#[test]
fn cylinder_through_a_cube_face_with_shared_vertex_plane() {
    // The cylinder's caps are coplanar with the cube's top and bottom, its axis vertices on
    // the cube's diagonal: coplanar overlaps, touching vertices and edges in one go.
    let a = boxm([-10., -10., 0.], [10., 10., 20.]);
    let mut c = cylinder(10., 20., 4);
    c.translate([0., 0., 10.]);
    let [u, d, i] = check_all(&a, &c, 1e-3);
    // A square of circumradius 10 inscribed in the cube's footprint touches its sides.
    assert!((i.volume() - 200. * 20.).abs() < 1e-2);
    assert!((u.volume() - 8000.).abs() < 1e-2);
    assert!((d.volume() - (8000. - 4000.)).abs() < 1e-2);
    assert!(closed(&u) && closed(&d));
}

#[test]
fn open_input_is_rejected() {
    let a = boxm([0., 0., 0.], [10., 10., 10.]);
    let mut open = a.clone();
    open.faces.pop();
    assert_eq!(
        boolean(&open, &a, BooleanOp::Union).unwrap_err(),
        BooleanError::NotClosed(0)
    );
    assert_eq!(
        boolean(&a, &open, BooleanOp::Union).unwrap_err(),
        BooleanError::NotClosed(1)
    );
}

#[test]
fn chained_booleans_stay_closed() {
    // Results feed further booleans: a plate with three pockets, then a union with a rib.
    let mut m = boxm([0., 0., 0.], [40., 20., 5.]);
    for k in 0..3 {
        let x = 5. + k as f32 * 12.;
        let mut c = cylinder(3., 10., 20);
        c.translate([x + 2.5, 10., 5.]);
        m = run(&m, &c, BooleanOp::Difference);
        assert!(manifold(&m));
    }
    let rib = boxm([0., 9., 0.], [40., 11., 12.]);
    let u = run(&m, &rib, BooleanOp::Union);
    assert!(manifold(&u));
    assert!(u.volume() > m.volume());
}

/// Seeded random boxes on a coarse 1 mm grid (so faces, edges and corners coincide all the
/// time) and random rotated spheres: every result closed, volumes consistent.
#[test]
fn random_grid_boxes_and_spheres() {
    let mut state = 0x2545_f491_4f6c_dd1du64;
    let mut next = |n: u64| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state % n
    };
    for _ in 0..150 {
        let mut corner = || {
            let lo = [next(8) as f32, next(8) as f32, next(8) as f32];
            let hi = [
                lo[0] + 1. + next(6) as f32,
                lo[1] + 1. + next(6) as f32,
                lo[2] + 1. + next(6) as f32,
            ];
            boxm(lo, hi)
        };
        let (a, b) = (corner(), corner());
        check_all(&a, &b, 1e-3);
    }
    for k in 0..12 {
        let a = sphere(6., 8 + k % 3, 12 + k % 5);
        let r = glam_rotation(
            0.1 * k as f32,
            0.37,
            0.05 * k as f32,
            [3. + k as f32 * 0.25, 1., 0.5],
        );
        let b = transformed(&sphere(5., 9, 14), &r);
        check_all(&a, &b, 1e-2);
    }
}

#[test]
fn larger_meshes_are_fast_enough() {
    let a = sphere(20., 64, 128);
    let mut b = sphere(15., 60, 120);
    b.translate([12., 3., 1.]);
    let t = std::time::Instant::now();
    let d = run(&a, &b, BooleanOp::Difference);
    assert!(manifold(&d));
    assert!(t.elapsed().as_secs_f64() < 20., "took {:?}", t.elapsed());
}

fn glam_rotation(ax: f32, ay: f32, az: f32, t: [f32; 3]) -> [[f32; 4]; 4] {
    let (sx, cx) = ax.sin_cos();
    let (sy, cy) = ay.sin_cos();
    let (sz, cz) = az.sin_cos();
    // R = Rz * Ry * Rx, column-major.
    let r = [
        [cz * cy, sz * cy, -sy],
        [cz * sy * sx - sz * cx, sz * sy * sx + cz * cx, cy * sx],
        [cz * sy * cx + sz * sx, sz * sy * cx - cz * sx, cy * cx],
    ];
    [
        [r[0][0], r[0][1], r[0][2], 0.],
        [r[1][0], r[1][1], r[1][2], 0.],
        [r[2][0], r[2][1], r[2][2], 0.],
        [t[0], t[1], t[2], 1.],
    ]
}
