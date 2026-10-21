// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/PrintObject.cpp :: PrintObject::project_and_append_custom_facets, project_triangles_to_slabs
// Source: prusaslicer@b028299c77 src/libslic3r/TriangleMeshSlicer.cpp :: slice_mesh_slabs
// Changes: Stream decoded facets into layer slabs using plane clipping; downward support projections share normal/tree volume masks.

use super::{FacetStates, Triangle};
use crate::geometry::clip::union_ex;
use crate::geometry::{ExPolygon, Point, signed_area};
use crate::mesh::TriangleMesh;

pub fn normal_z(t: Triangle) -> f64 {
    (t[1][0] - t[0][0]) * (t[2][1] - t[0][1]) - (t[1][1] - t[0][1]) * (t[2][0] - t[0][0])
}

fn clip_z(poly: &[[f64; 3]], z: f64, above: bool) -> Vec<[f64; 3]> {
    let mut out = Vec::new();
    let Some(&last) = poly.last() else {
        return out;
    };
    let inside = |p: [f64; 3]| if above { p[2] >= z } else { p[2] <= z };
    let mut a = last;
    for &b in poly {
        if inside(a) != inside(b) {
            let f = (z - a[2]) / (b[2] - a[2]);
            out.push(std::array::from_fn(|i| a[i] + f * (b[i] - a[i])));
        }
        if inside(b) {
            out.push(b);
        }
        a = b;
    }
    out
}

/// XY projection of the portion of a triangle between two horizontal planes.
pub fn slab(triangle: Triangle, low: f64, high: f64) -> Option<ExPolygon> {
    let points = clip_z(&clip_z(&triangle, low, true), high, false);
    let mut contour: Vec<_> = points.iter().map(|p| Point::from_mm(p[0], p[1])).collect();
    contour.dedup();
    let area = signed_area(&contour);
    if contour.len() < 3 || area.abs() < 1. {
        return None;
    }
    if area < 0. {
        contour.reverse();
    }
    Some(ExPolygon {
        contour,
        holes: vec![],
    })
}

/// Downward facets are projected upward to the first slicing plane above them.
pub fn support_masks(
    mesh: &TriangleMesh,
    states: &FacetStates,
    zs: &[f64],
    enforcers: &mut [Vec<ExPolygon>],
    blockers: &mut [Vec<ExPolygon>],
) {
    let _timing = crate::timing::Span::new(crate::timing::Stage::PaintingProjection);
    states.visit(mesh, false, |state, t| {
        if normal_z(t) >= 0. || !(state == 1 || state == 2) {
            return;
        }
        let min = t.iter().map(|p| p[2]).fold(f64::INFINITY, f64::min);
        let max = t.iter().map(|p| p[2]).fold(f64::NEG_INFINITY, f64::max);
        let first = zs.partition_point(|z| *z < min - 1e-6);
        let last = zs
            .partition_point(|z| *z <= max + 1e-6)
            .min(zs.len().saturating_sub(1));
        for i in first..=last {
            if i >= zs.len() {
                break;
            }
            let low = if i == 0 { f64::NEG_INFINITY } else { zs[i - 1] };
            if let Some(poly) = slab(t, low, zs[i]) {
                if state == 1 {
                    enforcers[i].push(poly);
                } else {
                    blockers[i].push(poly);
                }
            }
        }
    });
    for polygons in enforcers.iter_mut().chain(blockers) {
        *polygons = union_ex(polygons);
    }
}

/// Intersection of a nonhorizontal facet with a slicing plane. Half-open edges
/// avoid duplicate intersections at split-tree vertices.
pub fn section(t: Triangle, z: f64) -> Option<[Point; 2]> {
    let mut points = Vec::with_capacity(2);
    for i in 0..3 {
        let (a, b) = (t[i], t[(i + 1) % 3]);
        if (a[2] <= z && b[2] > z) || (b[2] <= z && a[2] > z) {
            let f = (z - a[2]) / (b[2] - a[2]);
            points.push(Point::from_mm(
                a[0] + f * (b[0] - a[0]),
                a[1] + f * (b[1] - a[1]),
            ));
        }
    }
    if points.len() == 2 && points[0] != points[1] {
        Some([points[0], points[1]])
    } else {
        None
    }
}
