// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: prusaslicer@b028299c77 src/libslic3r/Measure.cpp :: MeasuringImpl::update_planes, MeasuringImpl::extract_features, MeasuringImpl::get_feature, get_center_and_radius
// Source: prusaslicer@b028299c77 src/libslic3r/Geometry/Circle.cpp :: circle_ransac
// Changes: Rust port on a welded copy of the mesh in world coordinates. Plane borders are chained from boundary half-edges by vertex instead of SurfaceMesh's next_around_target walk (a plane whose border does not close, or an open/non-manifold edge, gets no border features, as upstream's PLANE_FAILURE); features of every plane are extracted up front in parallel (rayon) instead of lazily; the circle RANSAC samples evenly spaced point triples instead of std::sample with mt19937; a plane without borders takes its facets' centroid as origin (upstream divides by zero).
// Upstream attribution (Measure.cpp):
// Copyright (c) Prusa Research 2022 - 2023 Lukáš Matěna @lukasmatena, Enrico Turri @enricoturri1966, Vojtěch Bubník @bubnikv, Pavel Mikuš @Godrak
//
// PrusaSlicer is released under the terms of the AGPLv3 or higher
//

//! Planes, edges and circles of a mesh, and the feature under a point on a facet.

use super::{EPSILON, Feature, math};
use glam::{DMat3, DQuat, DVec2, DVec3, Mat4};
use mica_core::mesh::TriangleMesh;
use rayon::prelude::*;
use std::{collections::HashMap, sync::Arc};

/// How close (mm) to a feature the pointer must be to pick it rather than the plane.
pub const FEATURE_HOVER_LIMIT: f64 = 0.5;

struct PlaneData {
    facets: Vec<u32>,
    /// The plane's features; the last one is the plane itself.
    features: Vec<Feature>,
}

/// The measurable features of one mesh in world coordinates.
pub struct Measuring {
    vertices: Vec<DVec3>,
    faces: Vec<[u32; 3]>,
    face_to_plane: Vec<usize>,
    planes: Vec<PlaneData>,
}

fn is_approx(a: f64, b: f64, eps: f64) -> bool {
    (a - b).abs() < eps
}

/// Circumcentre of three points (`Geometry::circle_center`); `None` when nearly collinear.
fn circle_center(a: DVec2, b: DVec2, c: DVec2) -> Option<DVec2> {
    let (b, c) = (b - a, c - a);
    let d = 2. * (b.x * c.y - b.y * c.x);
    if d.abs() < EPSILON {
        return None;
    }
    let (b2, c2) = (b.length_squared(), c.length_squared());
    Some(a + DVec2::new((c.y * b2 - b.y * c2) / d, (b.x * c2 - c.x * b2) / d))
}

/// Best of `iterations` three-point circles: centre, mean radius and the largest deviation.
fn circle_ransac(points: &[DVec2], iterations: usize) -> Option<(DVec2, f64, f64)> {
    let n = points.len();
    if n < 3 {
        return None;
    }
    let mut best: Option<(DVec2, f64, f64)> = None;
    for k in 0..iterations {
        let offset = k * n / (3 * iterations).max(1);
        let pick = |j: usize| points[(offset + j * n / 3) % n];
        let Some(center) = circle_center(pick(0), pick(1), pick(2)) else {
            continue;
        };
        let radius = points.iter().map(|p| (*p - center).length()).sum::<f64>() / n as f64;
        let err = points
            .iter()
            .map(|p| ((*p - center).length() - radius).abs())
            .fold(0., f64::max);
        if best.is_none_or(|(_, _, e)| err < e) {
            best = Some((center, radius, err));
        }
    }
    best
}

/// Fits a circle to border points lying in a plane rotated onto XY by `rot`.
fn center_and_radius(points: &[DVec3], rot: DQuat) -> Option<(DVec3, f64, f64)> {
    let mut z = 0.;
    let flat: Vec<DVec2> = points
        .iter()
        .map(|p| {
            let t = rot * *p;
            z = t.z;
            DVec2::new(t.x, t.y)
        })
        .collect();
    let iterations = if points.len() < 10 {
        2
    } else if points.len() < 100 {
        4
    } else {
        6
    };
    let (c, r, err) = circle_ransac(&flat, iterations)?;
    Some((rot.inverse() * DVec3::new(c.x, c.y, z), r, err))
}

impl Measuring {
    /// Builds the features of `mesh` placed by `transform` (object to world).
    pub fn new(mesh: &TriangleMesh, transform: Mat4) -> Self {
        let t = transform.as_dmat4();
        // Weld exactly coincident vertices so neighbouring facets share indices.
        let mut ids: HashMap<[u32; 3], u32> = HashMap::with_capacity(mesh.vertices.len());
        let mut remap = Vec::with_capacity(mesh.vertices.len());
        let mut vertices: Vec<DVec3> = Vec::with_capacity(mesh.vertices.len());
        for v in &mesh.vertices {
            let key = v.map(f32::to_bits);
            let id = *ids.entry(key).or_insert_with(|| {
                vertices.push(t.transform_point3(DVec3::new(
                    v[0] as f64,
                    v[1] as f64,
                    v[2] as f64,
                )));
                (vertices.len() - 1) as u32
            });
            remap.push(id);
        }
        let mirrored = DMat3::from_mat4(t).determinant() < 0.;
        let faces: Vec<[u32; 3]> = mesh
            .faces
            .iter()
            .map(|f| {
                let f = f.map(|i| remap.get(i as usize).copied().unwrap_or(0));
                if mirrored { [f[0], f[2], f[1]] } else { f }
            })
            .collect();
        let normals: Vec<DVec3> = faces
            .iter()
            .map(|f| {
                let [a, b, c] = f.map(|i| vertices[i as usize]);
                (b - a).cross(c - a).normalize_or_zero()
            })
            .collect();
        // Neighbour across each edge (v[j] → v[j + 1]): the one other facet using it, or none.
        let mut edges: HashMap<(u32, u32), [u32; 2]> = HashMap::with_capacity(faces.len() * 3 / 2);
        let mut counts: HashMap<(u32, u32), u8> = HashMap::with_capacity(faces.len() * 3 / 2);
        for (fi, f) in faces.iter().enumerate() {
            for j in 0..3 {
                let (a, b) = (f[j], f[(j + 1) % 3]);
                let key = (a.min(b), a.max(b));
                let c = counts.entry(key).or_insert(0);
                if *c < 2 {
                    edges.entry(key).or_insert([u32::MAX; 2])[*c as usize] = fi as u32;
                }
                *c = c.saturating_add(1);
            }
        }
        let neighbors: Vec<[i64; 3]> = faces
            .iter()
            .enumerate()
            .map(|(fi, f)| {
                let mut out = [-1i64; 3];
                for j in 0..3 {
                    let (a, b) = (f[j], f[(j + 1) % 3]);
                    let key = (a.min(b), a.max(b));
                    if counts.get(&key) == Some(&2) {
                        let pair = edges[&key];
                        out[j] = if pair[0] as usize == fi {
                            pair[1] as i64
                        } else {
                            pair[0] as i64
                        };
                    }
                }
                out
            })
            .collect();
        // Flood fill facets of the same normal (within 0.001 per component) into planes.
        let same = |a: DVec3, b: DVec3| (a - b).abs().max_element() < 0.001;
        let mut face_to_plane = vec![usize::MAX; faces.len()];
        let mut planes: Vec<(Vec<u32>, DVec3)> = Vec::with_capacity(faces.len() / 5);
        let mut queue: Vec<usize> = Vec::new();
        for seed in 0..faces.len() {
            if face_to_plane[seed] != usize::MAX {
                continue;
            }
            let id = planes.len();
            let normal = normals[seed];
            let mut facets = vec![];
            face_to_plane[seed] = id;
            queue.push(seed);
            while let Some(f) = queue.pop() {
                facets.push(f as u32);
                for &n in &neighbors[f] {
                    if n >= 0 {
                        let n = n as usize;
                        if face_to_plane[n] == usize::MAX && same(normals[n], normal) {
                            face_to_plane[n] = id;
                            queue.push(n);
                        }
                    }
                }
            }
            facets.sort_unstable();
            planes.push((facets, normal));
        }
        let planes = planes
            .into_par_iter()
            .enumerate()
            .map(|(id, (facets, normal))| {
                let borders = borders(&facets, &faces, &neighbors, &face_to_plane, &vertices, id);
                let centroid = || {
                    let sum: DVec3 = facets
                        .iter()
                        .map(|&f| {
                            let [a, b, c] = faces[f as usize].map(|i| vertices[i as usize]);
                            (a + b + c) / 3.
                        })
                        .sum();
                    sum / facets.len().max(1) as f64
                };
                let features = extract_features(id, normal, &borders, centroid);
                PlaneData { facets, features }
            })
            .collect();
        Self {
            vertices,
            faces,
            face_to_plane,
            planes,
        }
    }
    #[cfg(test)]
    pub fn plane_count(&self) -> usize {
        self.planes.len()
    }
    /// Triangles (world coordinates) of plane `index`, for highlighting it.
    pub fn plane_triangles(&self, index: usize) -> Vec<[DVec3; 3]> {
        self.planes.get(index).map_or_else(Vec::new, |p| {
            p.facets
                .iter()
                .map(|&f| self.faces[f as usize].map(|i| self.vertices[i as usize]))
                .collect()
        })
    }
    #[cfg(test)]
    /// The features of plane `index` (its edges and circles, then the plane itself).
    pub fn plane_features(&self, index: usize) -> &[Feature] {
        self.planes.get(index).map_or(&[], |p| &p.features)
    }
    /// The feature to highlight for the pointer at `point` on facet `face`: the nearest edge or
    /// circle of the facet's plane within 0.5 mm (an edge's end point when the pointer is near
    /// it), else the plane.
    pub fn feature_at(&self, face: usize, point: DVec3) -> Option<Feature> {
        let plane = self.planes.get(*self.face_to_plane.get(face)?)?;
        let (last, rest) = plane.features.split_last()?;
        let probe = Feature::Point(point);
        let mut best: Option<(f64, &Feature)> = None;
        for f in rest {
            if let Some(d) = math::measure(f, &probe).distance_strict
                && d.dist < FEATURE_HOVER_LIMIT
                && best.is_none_or(|(b, _)| d.dist < b)
            {
                best = Some((d.dist, f));
            }
        }
        if let Some((_, f)) = best {
            if let Feature::Edge { a, b, .. } = f {
                // Within 10 % of the edge length (0.025..0.5 mm) of an end: that end point.
                let len_sq = (*b - *a).length_squared();
                let limit_sq = (0.025f64 * 0.025).max((0.5f64 * 0.5).min(0.01 * len_sq));
                if (point - *a).length_squared() < limit_sq {
                    return Some(Feature::Point(*a));
                }
                if (point - *b).length_squared() < limit_sq {
                    return Some(Feature::Point(*b));
                }
            }
            return Some(f.clone());
        }
        Some(last.clone())
    }
}

/// The closed border loops of one plane: boundary half-edges (to a facet of another plane)
/// chained end to start. Empty when the plane touches an open edge or a loop does not close.
fn borders(
    facets: &[u32],
    faces: &[[u32; 3]],
    neighbors: &[[i64; 3]],
    face_to_plane: &[usize],
    vertices: &[DVec3],
    id: usize,
) -> Vec<Vec<DVec3>> {
    let mut half_edges: Vec<(u32, u32)> = vec![];
    for &f in facets {
        for j in 0..3 {
            let n = neighbors[f as usize][j];
            if n < 0 {
                return vec![];
            }
            if face_to_plane[n as usize] != id {
                let face = faces[f as usize];
                half_edges.push((face[j], face[(j + 1) % 3]));
            }
        }
    }
    let mut outgoing: HashMap<u32, Vec<usize>> = HashMap::with_capacity(half_edges.len());
    for (i, (a, _)) in half_edges.iter().enumerate() {
        outgoing.entry(*a).or_default().push(i);
    }
    let mut used = vec![false; half_edges.len()];
    let mut loops = vec![];
    for start in 0..half_edges.len() {
        if used[start] {
            continue;
        }
        used[start] = true;
        let first = half_edges[start].0;
        let mut current = half_edges[start].1;
        let mut chain = vec![vertices[first as usize]];
        let mut closed = false;
        while chain.len() <= half_edges.len() {
            if current == first {
                closed = true;
                break;
            }
            chain.push(vertices[current as usize]);
            let next = outgoing
                .get(&current)
                .and_then(|list| list.iter().copied().find(|&i| !used[i]));
            match next {
                Some(i) => {
                    used[i] = true;
                    current = half_edges[i].1;
                }
                None => break,
            }
        }
        if !closed {
            return vec![];
        }
        if chain.len() > 1 {
            loops.push(chain);
        }
    }
    loops
}

/// Circles, polygon edges and plain edges of a plane's borders, then the plane itself.
fn extract_features(
    index: usize,
    normal: DVec3,
    borders: &[Vec<DVec3>],
    centroid: impl Fn() -> DVec3,
) -> Vec<Feature> {
    let mut out: Vec<Feature> = vec![];
    let rot = DQuat::from_rotation_arc(normal.normalize_or(DVec3::Z), DVec3::Z);
    let mut plane_edges: Vec<(DVec3, DVec3)> = vec![];
    for border in borders {
        let n = border.len();
        if n <= 1 {
            continue;
        }
        let mut done = false;
        if n > 4
            && let Some((center, radius, err)) = center_and_radius(border, rot)
            && err < 0.05
        {
            // The whole border is one circle (or a regular polygon of 5–8 sides).
            let is_polygon = n <= 8;
            let lengths_match = (2..n).all(|i| {
                is_approx(
                    (border[i] - border[i - 1]).length_squared(),
                    (border[i - 1] - border[i - 2]).length_squared(),
                    0.01,
                )
            });
            if lengths_match {
                if is_polygon {
                    for j in 0..n {
                        let (a, b) = (border[if j == 0 { n - 1 } else { j - 1 }], border[j]);
                        out.push(Feature::Edge {
                            a,
                            b,
                            center: Some(center),
                        });
                        plane_edges.push((a, b));
                    }
                } else {
                    out.push(Feature::Circle {
                        center,
                        radius,
                        normal,
                    });
                }
                done = true;
            }
        }
        if done {
            continue;
        }
        // Not one circle: look for circular segments, then add the remaining edges.
        let same_angle = |a: f64, b: f64| is_approx(a, b, 0.01);
        let same_length = |a: f64, b: f64| is_approx(a, b, 0.01);
        let at = |idx: usize, offset: i64| -> usize {
            let out = idx as i64 + offset;
            out.rem_euclid(n as i64) as usize
        };
        let mut angles = Vec::with_capacity(n);
        let mut lengths = Vec::with_capacity(n);
        let mut first_different = 0usize;
        for i in 0..n {
            let v2 = border[i] - border[if i == 0 { n - 1 } else { i - 1 }];
            let v1 = border[if i == n - 1 { 0 } else { i + 1 }] - border[i];
            let mut angle = (-normal.dot(v1.cross(v2))).atan2(-v1.dot(v2)) + std::f64::consts::PI;
            if angle > std::f64::consts::PI {
                angle = 2. * std::f64::consts::PI - angle;
            }
            angles.push(angle);
            lengths.push(v2.length());
            if first_different == 0
                && angles.len() > 1
                && !same_angle(angle, angles[angles.len() - 2])
            {
                first_different = angles.len() - 1;
            }
        }
        let mut start_idx: Option<usize> = None;
        let mut circle = false;
        let mut first_iter = true;
        let mut circles: Vec<Feature> = vec![];
        let mut circle_idxs: Vec<(usize, usize)> = vec![];
        let mut single: Vec<DVec3> = vec![];
        let mut single_length = 0.;
        let first_pt = at(first_different, 1);
        let mut i = first_pt;
        while i != first_pt || first_iter {
            if same_angle(angles[i], angles[at(i, -1)])
                && i != at(first_pt, -1)
                && Some(i) != start_idx
            {
                if !circle {
                    circle = true;
                    single.clear();
                    single_length = 0.;
                    let s = at(i, -2);
                    start_idx = Some(s);
                    single.push(border[s]);
                    single.push(border[at(s, 1)]);
                    single_length += lengths[at(i, -1)];
                }
                single.push(border[i]);
                single_length += lengths[i];
            } else {
                if circle && single.len() >= 5 {
                    single.push(border[i]);
                    single_length += lengths[i];
                    let s = start_idx.unwrap_or(0);
                    // The internal edges of the segment must have matching lengths.
                    let mut accept = true;
                    let mut j = at(s, 3);
                    while j != i {
                        if !same_length(lengths[at(j, -1)], lengths[j]) {
                            accept = false;
                            break;
                        }
                        j = at(j, 1);
                    }
                    if accept
                        && let Some((center, radius, err)) = center_and_radius(&single, rot)
                        // A good fit subtending at least ~90 degrees.
                        && err < 0.05
                        && single_length / radius > 0.9 * std::f64::consts::FRAC_PI_2
                    {
                        circle_idxs.push((s, i));
                        circles.push(Feature::Circle {
                            center,
                            radius,
                            normal,
                        });
                    }
                }
                circle = false;
            }
            first_iter = false;
            i = at(i, 1);
        }
        let mut edges: Vec<(DVec3, DVec3)> = vec![];
        if circle_idxs.is_empty() {
            for i in 1..n {
                edges.push((border[i - 1], border[i]));
            }
            edges.push((border[0], border[n - 1]));
        } else if circle_idxs.len() > 1 || circle_idxs[0].0 != circle_idxs[0].1 {
            // Start at the end of the first circular segment, add edges up to the next one.
            let mut i = circle_idxs[0].1;
            let mut next_circle = 1;
            let mut guard = 0;
            loop {
                i = at(i, 1);
                edges.push((border[at(i, -1)], border[i]));
                if next_circle < circle_idxs.len() && i == circle_idxs[next_circle].0 {
                    i = circle_idxs[next_circle].1;
                    next_circle += 1;
                }
                guard += 1;
                if i == circle_idxs[0].0 || guard > 2 * n {
                    break;
                }
            }
        }
        // Merge consecutive collinear edges that share a point.
        let mut k = edges.len();
        while k > 0 && edges.len() > 1 {
            k -= 1;
            if k >= edges.len() {
                continue;
            }
            let prev = if k == 0 { edges.len() - 1 } else { k - 1 };
            let (fs, fe) = edges[prev];
            let (ss, se) = edges[k];
            if (fe - ss).length() < 1e-9
                && is_approx(
                    (fe - fs)
                        .normalize_or_zero()
                        .dot((se - ss).normalize_or_zero()),
                    1.,
                    1e-9,
                )
            {
                edges[prev] = (fs, se);
                edges.remove(k);
            }
        }
        out.extend(circles);
        for (a, b) in edges {
            plane_edges.push((a, b));
            out.push(Feature::Edge { a, b, center: None });
        }
    }
    // The plane itself, at the centre of its border points.
    let (sum, count) = borders.iter().fold((DVec3::ZERO, 0usize), |(s, c), b| {
        (
            s + b.iter().skip(1).sum::<DVec3>(),
            c + b.len().saturating_sub(1),
        )
    });
    let origin = if count > 0 {
        sum / count as f64
    } else {
        centroid()
    };
    out.push(Feature::Plane {
        index,
        normal,
        origin,
        edges: Arc::new(plane_edges),
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use mica_core::mesh_edit;

    fn hole_block() -> TriangleMesh {
        TriangleMesh::load_stl(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../tests/models/hole_block.stl"),
        )
        .unwrap()
    }

    #[test]
    fn cube_planes_edges_and_points() {
        let cube = mesh_edit::cuboid([20., 20., 20.]);
        let m = Measuring::new(&cube, Mat4::IDENTITY);
        assert_eq!(m.plane_count(), 6);
        // Each face: four edges and the plane.
        for p in 0..6 {
            let f = m.plane_features(p);
            assert_eq!(f.len(), 5, "{f:?}");
            assert!(matches!(f.last(), Some(Feature::Plane { .. })));
        }
        // The top face's centre picks the plane, near an edge the edge, near a corner the point.
        let top = (0..cube.faces.len())
            .find(|&i| {
                let [a, b, c] = cube.faces[i].map(|k| cube.vertices[k as usize][2]);
                a > 9.9 && b > 9.9 && c > 9.9
            })
            .unwrap();
        assert!(matches!(
            m.feature_at(top, DVec3::new(0., 0., 10.)),
            Some(Feature::Plane { .. })
        ));
        match m.feature_at(top, DVec3::new(0., 9.8, 10.)) {
            Some(Feature::Edge { a, b, .. }) => assert!(((b - a).length() - 20.).abs() < 1e-6),
            other => panic!("{other:?}"),
        }
        match m.feature_at(top, DVec3::new(9.9, 9.9, 10.)) {
            Some(Feature::Point(p)) => assert!((p - DVec3::new(10., 10., 10.)).length() < 1e-6),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn detects_cylinder_caps_and_holes_as_circles() {
        let cylinder = mesh_edit::cylinder(4., 10., 64);
        let m = Measuring::new(
            &cylinder,
            Mat4::from_translation(glam::Vec3::new(100., 50., 5.)),
        );
        let circles = |m: &Measuring| -> Vec<(DVec3, f64)> {
            (0..m.plane_count())
                .flat_map(|p| m.plane_features(p).to_vec())
                .filter_map(|f| match f {
                    Feature::Circle { center, radius, .. } => Some((center, radius)),
                    _ => None,
                })
                .collect()
        };
        // One circle on the top and one on the bottom cap.
        let c = circles(&m);
        assert_eq!(c.len(), 2, "{c:?}");
        for (centre, r) in c {
            assert!((r - 4.).abs() < 0.01, "{r}");
            assert!((centre.truncate() - glam::DVec2::new(100., 50.)).length() < 0.01);
        }
        // The test block's hole: circles on both faces it opens into, on the hole's axis.
        let block = Measuring::new(&hole_block(), Mat4::IDENTITY);
        let c = circles(&block);
        assert!(c.len() >= 2, "{c:?}");
        let first = c[0].0.truncate();
        assert!(
            c.iter()
                .all(|(centre, r)| (centre.truncate() - first).length() < 0.05
                    && (r - c[0].1).abs() < 0.05)
        );
    }

    #[test]
    fn regular_polygons_give_edges_with_centre() {
        // A hexagonal prism: both caps are regular polygons whose edges carry the centre.
        let prism = mesh_edit::cylinder(4., 10., 6);
        let m = Measuring::new(&prism, Mat4::IDENTITY);
        let with_centre: Vec<DVec3> = (0..m.plane_count())
            .flat_map(|p| m.plane_features(p).to_vec())
            .filter_map(|f| match f {
                Feature::Edge {
                    center: Some(c), ..
                } => Some(c),
                _ => None,
            })
            .collect();
        assert_eq!(with_centre.len(), 12);
        assert!(with_centre.iter().all(|c| c.truncate().length() < 1e-6));
    }

    #[test]
    fn ransac_fits_points_on_a_circle() {
        let pts: Vec<DVec2> = (0..40)
            .map(|k| {
                let a = k as f64 / 40. * std::f64::consts::TAU;
                DVec2::new(3. + 7. * a.cos(), -2. + 7. * a.sin())
            })
            .collect();
        let (c, r, err) = circle_ransac(&pts, 4).unwrap();
        assert!((c - DVec2::new(3., -2.)).length() < 1e-9);
        assert!((r - 7.).abs() < 1e-9 && err < 1e-9);
        assert!(circle_ransac(&pts[..2], 4).is_none());
    }
}
