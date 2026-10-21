// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/Emboss.cpp :: Emboss::polygons2model
// Source: prusaslicer@b028299c77 src/libslic3r/Emboss.cpp :: Emboss::heal_polygons
// Source: prusaslicer@b028299c77 src/libslic3r/Emboss.cpp :: remove_small_islands
// Source: prusaslicer@b028299c77 src/slic3r/GUI/Jobs/EmbossJob.cpp :: try_create_mesh
// Source: prusaslicer@b028299c77 src/slic3r/GUI/Jobs/EmbossJob.cpp :: create_emboss_projection
// Changes: Front/back faces plus side quads as in polygons2model, with ear-clip triangulation (mesh_cut) instead of CGAL; healing is a Clipper2 non-zero / even-odd union with duplicate and tiny-island removal instead of the iterative heal_dupl_inter; the surface option keeps the flat outline but refines its triangulation and lifts every vertex onto the surface under it along the text Z (PrusaSlicer cuts the surface with CGAL and moves the cut along Z by the same front/back offsets); f64 maths, f32 mesh.
// Upstream attribution (Emboss.cpp, EmbossJob.cpp):
// Copyright (c) Prusa Research 2021 - 2023 Filip Sykala @Jony01, Lukáš Matěna @lukasmatena, Vojtěch Bubník @bubnikv, Oleksandra Iushchenko @YuSanka
//
// PrusaSlicer is released under the terms of the AGPLv3 or higher

//! Outline → closed triangle mesh.

use std::collections::HashMap;

use super::surface::SurfaceProbe;
use crate::geometry::{ExPolygon, Point, Polygon, SCALE, clip, signed_area};
use crate::mesh::TriangleMesh;

/// PrusaSlicer's `SAFE_SURFACE_OFFSET`: how far (mm) a shape reaches past the surface into the
/// other side, so a raised part fuses with the body and an engraving opens to the outside.
pub const SAFE_SURFACE_OFFSET: f64 = 0.015;

/// Islands and holes smaller than this (mm²) are dropped.
const MIN_AREA_MM2: f64 = 1e-4;

/// Where the prism sits along the text frame's Z, relative to the surface (or z = 0).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EmbossRange {
    pub back: f64,
    pub front: f64,
}
impl EmbossRange {
    /// A raised part (`outside`) spans `-0.015..depth-0.015`; an engraving or modifier
    /// `0.015-depth..0.015`, as PrusaSlicer's `try_create_mesh` offsets its `ProjectZ`.
    pub fn new(outside: bool, depth: f64) -> Self {
        let depth = depth.max(0.01);
        if outside {
            Self {
                back: -SAFE_SURFACE_OFFSET,
                front: depth - SAFE_SURFACE_OFFSET,
            }
        } else {
            Self {
                back: SAFE_SURFACE_OFFSET - depth,
                front: SAFE_SURFACE_OFFSET,
            }
        }
    }
}

/// Removes repeated points and collinear spikes; `None` when fewer than three points remain.
fn clean(poly: &Polygon) -> Option<Polygon> {
    let mut out: Polygon = Vec::with_capacity(poly.len());
    for &p in poly {
        if out.last() != Some(&p) {
            out.push(p);
        }
    }
    while out.len() > 1 && out.first() == out.last() {
        out.pop();
    }
    (out.len() >= 3).then_some(out)
}

/// A healed shape: the union of the outlines under the non-zero (TrueType, most SVG fills) or
/// even-odd fill rule, without self-intersections, with tiny islands and holes removed.
pub fn heal(polys: &[Polygon], non_zero: bool) -> Vec<ExPolygon> {
    let cleaned: Vec<Polygon> = polys.iter().filter_map(clean).collect();
    if cleaned.is_empty() {
        return Vec::new();
    }
    let mut ex = if non_zero {
        let as_ex: Vec<ExPolygon> = cleaned
            .into_iter()
            .map(|contour| ExPolygon {
                contour,
                holes: Vec::new(),
            })
            .collect();
        clip::union_ex(&as_ex)
    } else {
        clip::union_ex_even_odd(&cleaned)
    };
    let min = MIN_AREA_MM2 * SCALE * SCALE;
    ex.retain(|e| signed_area(&e.contour).abs() >= min);
    for e in &mut ex {
        e.holes.retain(|h| signed_area(h).abs() >= min);
    }
    ex
}

fn mm(p: Point) -> [f64; 2] {
    [p.x as f64 / SCALE, p.y as f64 / SCALE]
}

/// The ring in mm with the wanted orientation (counter-clockwise when `ccw`), without
/// collinear in-between points (they would make zero-area ears).
fn ring(poly: &Polygon, ccw: bool) -> Vec<[f64; 2]> {
    let mut pts: Vec<[f64; 2]> = poly.iter().map(|&p| mm(p)).collect();
    if (signed_area(poly) > 0.) != ccw {
        pts.reverse();
    }
    let mut i = 0;
    while pts.len() > 3 && i < pts.len() {
        let n = pts.len();
        let (a, b, c) = (pts[(i + n - 1) % n], pts[i], pts[(i + 1) % n]);
        let cross = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
        let scale = len2(a, b).max(len2(b, c)).max(1e-24);
        if cross.abs() <= 1e-12 * scale {
            pts.remove(i);
            i = i.saturating_sub(1);
        } else {
            i += 1;
        }
    }
    pts
}

fn len2(a: [f64; 2], b: [f64; 2]) -> f64 {
    (b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)
}

fn key(a: u32, b: u32) -> (u32, u32) {
    (a.min(b), a.max(b))
}

/// Splits edges longer than `max_edge` at their midpoints, longest edge of a triangle first;
/// an inner edge splits both triangles that share it, a boundary edge its one triangle, so the
/// triangulation stays conforming and its outline (the side walls) stays closed.
fn refine(pts: &mut Vec<[f64; 2]>, tris: &mut Vec<[u32; 3]>, max_edge: f64, limit: usize) {
    let mut edges: HashMap<(u32, u32), Vec<usize>> = HashMap::with_capacity(tris.len() * 3);
    for (t, f) in tris.iter().enumerate() {
        for k in 0..3 {
            edges.entry(key(f[k], f[(k + 1) % 3])).or_default().push(t);
        }
    }
    let max2 = max_edge * max_edge;
    let mut stack: Vec<usize> = (0..tris.len()).collect();
    while let Some(t) = stack.pop() {
        if tris.len() >= limit {
            break;
        }
        let f = tris[t];
        let (k, l) = (0..3)
            .map(|k| (k, len2(pts[f[k] as usize], pts[f[(k + 1) % 3] as usize])))
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .unwrap_or((0, 0.));
        if l <= max2 {
            continue;
        }
        let (a, b, c) = (f[k], f[(k + 1) % 3], f[(k + 2) % 3]);
        let shared = edges.get(&key(a, b)).map_or(0, Vec::len);
        let neighbour = edges
            .get(&key(a, b))
            .and_then(|list| list.iter().copied().find(|&o| o != t));
        let neighbour = match (shared, neighbour) {
            (1, _) => None,
            (2, Some(n)) => {
                let g = tris[n];
                match (0..3).find(|&j| g[j] == b && g[(j + 1) % 3] == a) {
                    Some(j) => Some((n, g[(j + 2) % 3])),
                    None => continue, // inconsistent winding (degenerate input): leave it
                }
            }
            _ => continue, // non-manifold edge: leave it
        };
        let m = pts.len() as u32;
        let (pa, pb) = (pts[a as usize], pts[b as usize]);
        pts.push([(pa[0] + pb[0]) * 0.5, (pa[1] + pb[1]) * 0.5]);
        let olds: Vec<usize> = std::iter::once(t)
            .chain(neighbour.map(|(n, _)| n))
            .collect();
        for &old in &olds {
            let f = tris[old];
            for k in 0..3 {
                if let Some(list) = edges.get_mut(&key(f[k], f[(k + 1) % 3])) {
                    list.retain(|&x| x != old);
                }
            }
        }
        let mut news = vec![t, tris.len()];
        tris[t] = [a, m, c];
        tris.push([m, b, c]);
        if let Some((n, d)) = neighbour {
            news.push(n);
            news.push(tris.len());
            tris[n] = [b, m, d];
            tris.push([m, a, d]);
        }
        for new in news {
            let f = tris[new];
            for k in 0..3 {
                edges
                    .entry(key(f[k], f[(k + 1) % 3]))
                    .or_default()
                    .push(new);
            }
            stack.push(new);
        }
    }
}

/// The closed prism of `shape` (text frame, scaled coordinates) between `range.back` and
/// `range.front` along Z, outward-facing. With a `surface`, every vertex is lifted onto the
/// surface under it (the outline is refined to `max_edge` mm so the faces follow curvature);
/// points the surface does not cover stay at z = 0.
pub fn emboss_mesh(
    shape: &[ExPolygon],
    range: EmbossRange,
    surface: Option<&SurfaceProbe>,
    max_edge: f64,
) -> TriangleMesh {
    let mut mesh = TriangleMesh::default();
    for ex in shape {
        let outer = ring(&ex.contour, true);
        let holes: Vec<Vec<[f64; 2]>> = ex.holes.iter().map(|h| ring(h, false)).collect();
        let mut tris = crate::mesh_cut::triangulate_2d(&outer, &holes);
        let mut pts: Vec<[f64; 2]> = outer;
        for h in &holes {
            pts.extend_from_slice(h);
        }
        if surface.is_some() {
            refine(&mut pts, &mut tris, max_edge.max(0.05), 400_000);
        }
        let heights: Vec<f64> = match surface {
            Some(s) => pts
                .iter()
                .map(|p| s.height(p[0], p[1]).unwrap_or(0.))
                .collect(),
            None => vec![0.; pts.len()],
        };
        let base = mesh.vertices.len() as u32;
        let count = pts.len() as u32;
        for (p, h) in pts.iter().zip(&heights) {
            mesh.vertices
                .push([p[0] as f32, p[1] as f32, (h + range.front) as f32]);
        }
        for (p, h) in pts.iter().zip(&heights) {
            mesh.vertices
                .push([p[0] as f32, p[1] as f32, (h + range.back) as f32]);
        }
        let mut uses: HashMap<(u32, u32), u32> = HashMap::with_capacity(tris.len() * 3);
        for t in &tris {
            mesh.faces.push(t.map(|i| base + i));
            mesh.faces
                .push([t[0], t[2], t[1]].map(|i| base + count + i));
            for k in 0..3 {
                *uses.entry(key(t[k], t[(k + 1) % 3])).or_default() += 1;
            }
        }
        // Side walls along the outline (edges of one triangle only). The faces are
        // counter-clockwise, so the material lies left of each boundary edge a → b and the quad
        // (a_back, b_back, b_front, a_front) faces away from it.
        for t in &tris {
            for k in 0..3 {
                let (a, b) = (t[k], t[(k + 1) % 3]);
                if uses.get(&key(a, b)) == Some(&1) {
                    let (af, bf) = (base + a, base + b);
                    let (ab, bb) = (af + count, bf + count);
                    mesh.faces.push([ab, bb, bf]);
                    mesh.faces.push([ab, bf, af]);
                }
            }
        }
    }
    mesh
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh_cut::is_watertight;

    fn square(x: f64, y: f64, s: f64) -> Polygon {
        [[x, y], [x + s, y], [x + s, y + s], [x, y + s]]
            .iter()
            .map(|p| Point::from_mm(p[0], p[1]))
            .collect()
    }

    #[test]
    fn ring_with_hole_is_a_closed_outward_prism() {
        let mut hole = square(2., 2., 6.);
        hole.reverse();
        let shape = heal(&[square(0., 0., 10.), hole], true);
        assert_eq!(shape.len(), 1);
        assert_eq!(shape[0].holes.len(), 1);
        let mesh = emboss_mesh(&shape, EmbossRange::new(true, 2.), None, 0.5);
        assert!(is_watertight(&mesh));
        // (100 - 36) mm² × 2 mm, positive: normals point outwards.
        assert!((mesh.volume() - 128.).abs() < 1e-3, "{}", mesh.volume());
        let (min, max) = mesh.bounds();
        assert!((min[2] + 0.015).abs() < 1e-5 && (max[2] - 1.985).abs() < 1e-5);
    }

    #[test]
    fn overlapping_outlines_union_under_non_zero() {
        let shape = heal(&[square(0., 0., 4.), square(2., 0., 4.)], true);
        assert_eq!(shape.len(), 1);
        let mesh = emboss_mesh(&shape, EmbossRange::new(false, 1.), None, 0.5);
        assert!(is_watertight(&mesh));
        assert!((mesh.volume() - 24.).abs() < 1e-3);
        let (min, max) = mesh.bounds();
        assert!((min[2] + 0.985).abs() < 1e-5 && (max[2] - 0.015).abs() < 1e-5);
        // Even-odd: the overlap becomes a hole region.
        let eo = heal(&[square(0., 0., 4.), square(2., 0., 4.)], false);
        let area: f64 = eo.iter().map(|e| e.area_mm2()).sum();
        assert!((area - 16.).abs() < 1e-6, "{area}");
    }

    #[test]
    fn surface_projection_follows_a_cylinder_and_stays_watertight() {
        // Cylinder of radius 20 along X, text frame at its top: z = sqrt(400 - y²) - 20.
        let cyl = crate::mesh_edit::cylinder(20., 40., 96);
        // cylinder() stands along Z; turn it to lie along X and put its top at z = 0.
        let t =
            crate::threemf::Transform([[0., 0., 1., 0.], [0., 1., 0., 0.], [-1., 0., 0., -20.]]);
        let probe = SurfaceProbe::new(&[(&cyl, t)], ([-12., -12.], [12., 12.]), 1.0);
        assert!(probe.height(0., 0.).is_some_and(|h| h.abs() < 0.1));
        let shape = heal(&[square(-10., -8., 16.)], true);
        let mesh = emboss_mesh(&shape, EmbossRange::new(true, 1.), Some(&probe), 0.5);
        assert!(is_watertight(&mesh));
        let (min, _) = mesh.bounds();
        // The corners at y = ±8 sit lower on the curve: 20 - sqrt(400 - 64) ≈ 1.67 mm.
        assert!(min[2] < -1.6, "{min:?}");
        assert!(mesh.volume() > 0.);
    }
}
