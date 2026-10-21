// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: prusaslicer@b028299c77 src/slic3r/GUI/Gizmos/GLGizmoFlatten.cpp :: GLGizmoFlatten::update_planes
// Changes: Rust port on f64 arrays; Mica's quickhull (orient.rs) instead of TriangleMesh::convex_hull_3d; the hull is built
// Changes: from the scaled mesh only (rotation does not change which planes exist, so results are cached per mesh and scale);
// Changes: planes are flattened onto an orthonormal in-plane basis instead of a two-vector quaternion, and hulled with a
// Changes: monotone chain instead of the rescaled fixed-point Geometry::convex_hull; a queued facet is skipped once visited;
// Changes: outlines stay in the scaled mesh frame (no 0.1 mm lift, no VBO / raycaster), with the unrounded hull kept for picking.

//! "Lay on face" planes, as PrusaSlicer's Place on face gizmo finds them.
//!
//! The candidates are the faces of the object's convex hull: anything the object can rest on
//! is a supporting plane of the hull, so every candidate is stable by construction and
//! concave or overhanging regions never become one. Neighbouring hull facets whose normals
//! agree (each component within 0.001) are flood-filled into one plane; its outline is the
//! 2D convex hull of the merged facets' vertices. Planes under 5 mm², single triangles with a
//! side under 1 mm and slivers with an inner angle under 10° are dropped, the rest sorted by
//! area and capped at 254, all upstream's numbers.
//!
//! Everything is computed in the *scaled* mesh frame (`vertex × scale`, mirror included), so
//! the thresholds are in real millimetres and the caller only rotates and translates the
//! result into the world.

use crate::orient::convex_hull;
use std::collections::HashMap;

/// Upstream `minimal_area`: smaller planes are not offered (mm²).
pub const MINIMAL_AREA: f64 = 5.;
/// Upstream `minimal_side`: a lone hull triangle with a shorter side is dropped (mm).
pub const MINIMAL_SIDE: f64 = 1.;
/// Upstream per-component tolerance for "the same normal".
pub const NORMAL_TOLERANCE: f64 = 0.001;
/// Upstream inner-angle threshold (degrees): sharper outlines are slivers, not faces.
pub const MINIMAL_ANGLE_DEG: f64 = 10.;
/// Upstream cap (a picking-pass limit there; kept so the same planes are offered).
pub const MAX_PLANES: usize = 254;

/// One plane the object can be laid on.
#[derive(Clone, Debug, PartialEq)]
pub struct Plane {
    /// Outward unit normal in the scaled mesh frame: laying on the plane turns it to −Z.
    pub normal: [f64; 3],
    /// Area of the plane's convex outline (mm²).
    pub area: f64,
    /// The convex outline (counter-clockwise seen from outside), in the scaled mesh frame.
    pub hull: Vec<[f64; 3]>,
    /// The outline as upstream draws it: shrunk 10 % towards its centroid and with rounded
    /// corners, so neighbouring planes do not touch.
    pub outline: Vec<[f64; 3]>,
}

/// The planes of a mesh scaled by `scale` (per axis, mirror as a negative factor), largest
/// first. Empty for meshes without volume (fewer than four non-coplanar points).
pub fn planes(vertices: &[[f32; 3]], scale: [f64; 3]) -> Vec<Plane> {
    let points: Vec<[f64; 3]> = vertices
        .iter()
        .map(|v| {
            [
                v[0] as f64 * scale[0],
                v[1] as f64 * scale[1],
                v[2] as f64 * scale[2],
            ]
        })
        .collect();
    let hull = convex_hull(&points);
    planes_of_hull(&points, &hull)
}

/// Upstream `update_planes` from the hull facets on.
fn planes_of_hull(points: &[[f64; 3]], hull: &[[u32; 3]]) -> Vec<Plane> {
    let n = hull.len();
    let normals: Vec<[f64; 3]> = hull
        .iter()
        .map(|f| {
            let [a, b, c] = f.map(|i| points[i as usize]);
            normalize(cross(sub(b, a), sub(c, a))).unwrap_or([0.; 3])
        })
        .collect();
    // its_face_neighbors: the facet across each edge.
    let mut edges: HashMap<(u32, u32), usize> = HashMap::with_capacity(n * 3);
    for (fi, f) in hull.iter().enumerate() {
        for k in 0..3 {
            edges.insert((f[k], f[(k + 1) % 3]), fi);
        }
    }
    let neighbours: Vec<[Option<usize>; 3]> = hull
        .iter()
        .map(|f| std::array::from_fn(|k| edges.get(&(f[(k + 1) % 3], f[k])).copied()))
        .collect();

    // Flood fill facets sharing the seed's normal.
    let mut groups: Vec<([f64; 3], Vec<[f64; 3]>)> = Vec::new();
    let mut visited = vec![false; n];
    let mut queue: Vec<usize> = Vec::new();
    for seed in 0..n {
        if visited[seed] {
            continue;
        }
        let normal = normals[seed];
        queue.push(seed);
        let mut verts: Vec<[f64; 3]> = Vec::new();
        while let Some(fi) = queue.pop() {
            if visited[fi] {
                continue;
            }
            let this = normals[fi];
            if (0..3).all(|k| (this[k] - normal[k]).abs() < NORMAL_TOLERANCE) {
                verts.extend(hull[fi].map(|i| points[i as usize]));
                visited[fi] = true;
                for nb in neighbours[fi].iter().flatten() {
                    if !visited[*nb] {
                        queue.push(*nb);
                    }
                }
            }
        }
        // A very small lone triangle would be rejected later anyway.
        if verts.len() == 3
            && (dist(verts[0], verts[1]) < MINIMAL_SIDE
                || dist(verts[0], verts[2]) < MINIMAL_SIDE
                || dist(verts[1], verts[2]) < MINIMAL_SIDE)
        {
            continue;
        }
        groups.push((normal, verts));
    }

    let cos_threshold = MINIMAL_ANGLE_DEG.to_radians().cos();
    let mut planes: Vec<Plane> = groups
        .into_iter()
        .filter_map(|(normal, verts)| {
            // Flatten into the plane's own 2D frame and drop the inner points.
            let (u, v) = basis(normal);
            let height = verts.iter().map(|p| dot(*p, normal)).sum::<f64>() / verts.len() as f64;
            let flat: Vec<[f64; 2]> = verts.iter().map(|p| [dot(*p, u), dot(*p, v)]).collect();
            let polygon = hull2d(flat);
            if polygon.len() < 3 {
                return None;
            }
            let area = shoelace(&polygon).abs();
            if area < MINIMAL_AREA {
                return None;
            }
            let m = polygon.len();
            for i in 0..m {
                let prev = polygon[(i + m - 1) % m];
                let curr = polygon[i];
                let next = polygon[(i + 1) % m];
                let a = normalize2(sub2(prev, curr));
                let b = normalize2(sub2(next, curr));
                if a[0] * b[0] + a[1] * b[1] > cos_threshold {
                    return None;
                }
            }
            let lift =
                |q: [f64; 2]| add(add(scale(u, q[0]), scale(v, q[1])), scale(normal, height));
            let outline = round_corners(&shrink(&polygon));
            Some(Plane {
                normal,
                area,
                hull: polygon.iter().map(|&q| lift(q)).collect(),
                outline: outline.into_iter().map(lift).collect(),
            })
        })
        .collect();
    planes.sort_by(|a, b| b.area.total_cmp(&a.area));
    planes.truncate(MAX_PLANES);
    planes
}

/// Upstream: "shrink the polygon a little bit so it does not touch the object edges".
fn shrink(polygon: &[[f64; 2]]) -> Vec<[f64; 2]> {
    let n = polygon.len() as f64;
    let c = polygon
        .iter()
        .fold([0., 0.], |s, p| [s[0] + p[0] / n, s[1] + p[1] / n]);
    polygon
        .iter()
        .map(|p| [0.9 * p[0] + 0.1 * c[0], 0.9 * p[1] + 0.1 * c[1]])
        .collect()
}

/// Upstream corner rounding: each vertex moves towards the average of points on its two
/// sides, `K` times, the side points walking towards the vertex (2·K·N points out).
fn round_corners(polygon: &[[f64; 2]]) -> Vec<[f64; 2]> {
    const K: usize = 10;
    const AGGRESSIVITY: f64 = 0.2;
    let n = polygon.len();
    let mut out = vec![[0.; 2]; 2 * K * n];
    let mut neighbours: Vec<(usize, usize)> = Vec::with_capacity(n);
    let lerp =
        |r: f64, a: [f64; 2], b: [f64; 2]| [r * a[0] + (1. - r) * b[0], r * a[1] + (1. - r) * b[1]];
    for (j, p) in polygon.iter().enumerate() {
        out[j * 2 * K] = *p;
        let first = if j == 0 {
            (n - 1) * 2 * K + K
        } else {
            j * 2 * K - K
        };
        neighbours.push((first, j * 2 * K + K));
    }
    for i in 0..K {
        for j in 0..n {
            if i == 0 {
                out[j * 2 * K + K] = lerp(0.5, out[j * 2 * K], out[((j + 1) % n) * 2 * K]);
            } else {
                let r = 0.2 + 0.3 / (K - 1) as f64 * i as f64;
                let (first, second) = neighbours[j];
                out[first] = lerp(r, out[j * 2 * K], out[first - 1]);
                out[second] = lerp(r, out[j * 2 * K], out[second + 1]);
            }
        }
        for j in 0..n {
            let (first, second) = neighbours[j];
            let mid = lerp(0.5, out[first], out[second]);
            out[2 * K * j] = lerp(1. - AGGRESSIVITY, out[2 * K * j], mid);
        }
        for nb in &mut neighbours {
            nb.0 += 1;
            nb.1 -= 1;
        }
    }
    out
}

/// Ray / plane-outline test for picking: the distance along `dir` from `origin` to the
/// outline's interior, all in one frame. Only planes facing the ray count.
pub fn ray_hits(
    polygon: &[[f64; 3]],
    normal: [f64; 3],
    origin: [f64; 3],
    dir: [f64; 3],
) -> Option<f64> {
    let facing = dot(normal, dir);
    if polygon.len() < 3 || facing >= 0. {
        return None;
    }
    let t = dot(sub(polygon[0], origin), normal) / facing;
    if t < 0. {
        return None;
    }
    let hit = add(origin, scale(dir, t));
    // Convex polygon, counter-clockwise around the normal: inside every edge.
    let m = polygon.len();
    for i in 0..m {
        let a = polygon[i];
        let b = polygon[(i + 1) % m];
        if dot(cross(sub(b, a), sub(hit, a)), normal) < -1e-9 {
            return None;
        }
    }
    Some(t)
}

/// An orthonormal basis (u, v) with u × v = n.
fn basis(n: [f64; 3]) -> ([f64; 3], [f64; 3]) {
    let helper = if n[0].abs() < 0.9 {
        [1., 0., 0.]
    } else {
        [0., 1., 0.]
    };
    let u = normalize(cross(helper, n)).unwrap_or([1., 0., 0.]);
    let v = cross(n, u);
    (u, v)
}

/// Convex hull (monotone chain, counter-clockwise, no collinear points).
fn hull2d(mut points: Vec<[f64; 2]>) -> Vec<[f64; 2]> {
    points.sort_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
    points.dedup();
    if points.len() < 3 {
        return points;
    }
    let cross2 = |o: [f64; 2], a: [f64; 2], b: [f64; 2]| {
        (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
    };
    let mut lower: Vec<[f64; 2]> = Vec::new();
    for p in &points {
        while lower.len() >= 2
            && cross2(lower[lower.len() - 2], lower[lower.len() - 1], *p) <= 1e-12
        {
            lower.pop();
        }
        lower.push(*p);
    }
    let mut upper: Vec<[f64; 2]> = Vec::new();
    for p in points.iter().rev() {
        while upper.len() >= 2
            && cross2(upper[upper.len() - 2], upper[upper.len() - 1], *p) <= 1e-12
        {
            upper.pop();
        }
        upper.push(*p);
    }
    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

fn shoelace(p: &[[f64; 2]]) -> f64 {
    let m = p.len();
    0.5 * (0..m)
        .map(|i| {
            let (a, b) = (p[i], p[(i + 1) % m]);
            a[0] * b[1] - b[0] * a[1]
        })
        .sum::<f64>()
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn scale(a: [f64; 3], s: f64) -> [f64; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn dist(a: [f64; 3], b: [f64; 3]) -> f64 {
    dot(sub(a, b), sub(a, b)).sqrt()
}
fn normalize(a: [f64; 3]) -> Option<[f64; 3]> {
    let l = dot(a, a).sqrt();
    (l > 1e-300 && l.is_finite()).then(|| scale(a, 1. / l))
}
fn sub2(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [a[0] - b[0], a[1] - b[1]]
}
fn normalize2(a: [f64; 2]) -> [f64; 2] {
    let l = (a[0] * a[0] + a[1] * a[1]).sqrt();
    if l > 0. { [a[0] / l, a[1] / l] } else { a }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An axis-aligned box whose every side is a `cells` × `cells` grid of quads (two
    /// triangles each), so each flat side is many coplanar triangles.
    fn grid_box(size: [f32; 3], cells: u32) -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
        let mut vertices = Vec::new();
        let mut faces = Vec::new();
        // (fixed axis, value 0 or 1, the two running axes in CCW order seen from outside)
        let sides = [
            (2, 0., 1, 0),
            (2, 1., 0, 1),
            (1, 0., 0, 2),
            (1, 1., 2, 0),
            (0, 0., 2, 1),
            (0, 1., 1, 2),
        ];
        for (axis, at, a, b) in sides {
            let base = vertices.len() as u32;
            for i in 0..=cells {
                for j in 0..=cells {
                    let mut p = [0f32; 3];
                    p[axis] = at * size[axis];
                    p[a] = i as f32 / cells as f32 * size[a];
                    p[b] = j as f32 / cells as f32 * size[b];
                    vertices.push(p);
                }
            }
            let idx = |i: u32, j: u32| base + i * (cells + 1) + j;
            for i in 0..cells {
                for j in 0..cells {
                    faces.push([idx(i, j), idx(i + 1, j), idx(i + 1, j + 1)]);
                    faces.push([idx(i, j), idx(i + 1, j + 1), idx(i, j + 1)]);
                }
            }
        }
        (vertices, faces)
    }

    #[test]
    fn a_cube_of_many_coplanar_triangles_has_six_planes() {
        let (vertices, faces) = grid_box([20., 20., 20.], 12);
        assert_eq!(faces.len(), 6 * 12 * 12 * 2);
        let planes = planes(&vertices, [1., 1., 1.]);
        assert_eq!(planes.len(), 6, "{planes:?}");
        let mut normals: Vec<[i32; 3]> = planes
            .iter()
            .map(|p| p.normal.map(|c| c.round() as i32))
            .collect();
        normals.sort();
        assert_eq!(
            normals,
            vec![
                [-1, 0, 0],
                [0, -1, 0],
                [0, 0, -1],
                [0, 0, 1],
                [0, 1, 0],
                [1, 0, 0]
            ]
        );
        for p in &planes {
            assert!((p.area - 400.).abs() < 1e-6, "{}", p.area);
            assert_eq!(p.hull.len(), 4);
            assert_eq!(p.outline.len(), 4 * 2 * 10);
            // Every outline point lies on its side, inside the hull.
            for q in &p.outline {
                let h = dot(*q, p.normal);
                assert!((h - dot(p.hull[0], p.normal)).abs() < 1e-9);
            }
        }
    }

    #[test]
    fn scale_changes_which_planes_qualify() {
        // 2 × 2 × 20: the 4 mm² ends are under the 5 mm² minimum, the long sides are not.
        let (vertices, _) = grid_box([2., 2., 20.], 3);
        assert_eq!(planes(&vertices, [1., 1., 1.]).len(), 4);
        // Scaled ×2 the ends are 16 mm² and qualify; areas are in scaled millimetres.
        let scaled = planes(&vertices, [2., 2., 2.]);
        assert_eq!(scaled.len(), 6);
        assert!((scaled[0].area - 160.).abs() < 1e-6);
        // A mirror keeps the planes, normals still outward.
        let mirrored = planes(&vertices, [-2., 2., 2.]);
        assert_eq!(mirrored.len(), 6);
        assert!(mirrored.iter().any(|p| p.normal[0] > 0.99));
    }

    #[test]
    fn concave_sides_are_not_planes() {
        // An L-shaped prism: the two inner faces of the notch cannot be rested on.
        let profile = [
            [0., 0.],
            [20., 0.],
            [20., 8.],
            [8., 8.],
            [8., 20.],
            [0., 20.],
        ];
        let mut vertices = Vec::new();
        for z in [0f32, 10.] {
            for p in profile {
                vertices.push([p[0], p[1], z]);
            }
        }
        let planes = planes(&vertices, [1., 1., 1.]);
        // Bottom, top, the four outer sides and the diagonal hull side over the notch.
        assert_eq!(planes.len(), 7, "{planes:?}");
        let diagonal = planes
            .iter()
            .find(|p| p.normal[0] > 0.5 && p.normal[1] > 0.5)
            .expect("the hull side bridging the notch");
        assert!((diagonal.area - (12f64 * 12. * 2.).sqrt() * 10.).abs() < 1e-6);
    }

    #[test]
    fn picking_hits_the_outline_from_the_front_only() {
        let (vertices, _) = grid_box([20., 20., 20.], 2);
        let planes = planes(&vertices, [1., 1., 1.]);
        let top = planes.iter().find(|p| p.normal[2] > 0.99).unwrap();
        let down = [0., 0., -1.];
        assert_eq!(
            ray_hits(&top.outline, top.normal, [10., 10., 50.], down),
            Some(30.)
        );
        // Outside the shrunk outline, and from behind.
        assert_eq!(
            ray_hits(&top.outline, top.normal, [0.5, 0.5, 50.], down),
            None
        );
        assert_eq!(
            ray_hits(&top.outline, top.normal, [10., 10., -50.], [0., 0., 1.]),
            None
        );
    }

    /// Time for the Benchy (run with `--release -- --ignored --nocapture`).
    #[test]
    #[ignore]
    fn benchy_timing() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/models/3DBenchy.stl"
        );
        let mesh = crate::mesh::TriangleMesh::load_stl(path).unwrap();
        let start = std::time::Instant::now();
        let planes = planes(&mesh.vertices, [1., 1., 1.]);
        eprintln!(
            "3DBenchy: {} facets, {} planes in {:?}; largest {:.0} mm²",
            mesh.faces.len(),
            planes.len(),
            start.elapsed(),
            planes[0].area
        );
        assert!(
            planes[0].normal[2] < -0.99,
            "the hull bottom is the largest plane"
        );
    }
}
