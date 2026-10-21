// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/PrintApply.cpp :: print_objects_from_model_object
// Source: prusaslicer@b028299c77 src/libslic3r/GCode.cpp :: GCodeGenerator::process_layer_single_object
// Source: orca@8500fcdcca src/libslic3r/PrintApply.cpp :: print_objects_from_model_object
// Changes: Instances are compared on their placed geometry, settings, tools, modifiers and painting instead of one ModelObject's transformation matrices, so CLI inputs, 3MF build items and GUI copies share alike; the shift is the difference of the objects' scaled centres; paths are shifted when a layer is scheduled, not by a G-code origin.

//! Instances that differ only by an XY translation share one prepared object.
//!
//! PrusaSlicer and Orca build one `PrintObject` per distinct instance transformation with the
//! XY translation removed, and list the instances with their scaled `shift`. Slicing, walls,
//! infill and supports happen once; G-code generation moves the origin to each instance.
//! Mica's inputs are already placed on the bed, so equality is decided on the placed data: the
//! same faces, model and modifier volumes, painting, layer-height ranges, tools and resolved
//! configuration, with every vertex displaced by the same XY vector. A rotation about Z, a
//! scale or a mirror changes the transformation, so such copies are prepared on their own, as
//! upstream does.

use crate::config::Config;
use crate::geometry::{ExPolygon, Point, unscale};
use crate::mesh::TriangleMesh;
use crate::print_job::ToolAssignments;
use crate::regions::RegionModel;
use crate::toolpath::{ExtrusionPath, LayerToolpaths};

/// Everything that decides whether two instances can share one prepared object.
pub struct Candidate<'a> {
    /// Combined model volumes, placed on the bed.
    pub mesh: &'a TriangleMesh,
    pub region: Option<&'a RegionModel>,
    pub tools: &'a ToolAssignments,
    /// The resolved configuration the object is prepared with.
    pub config: &'a Config,
}

/// The scaled centre preparation aligns infill to (`print::prepare_with_regions`' origin).
fn centre(mesh: &TriangleMesh) -> Point {
    let (min, max) = mesh.bounds();
    Point::from_mm(
        (f64::from(min[0]) + f64::from(max[0])) / 2.0,
        (f64::from(min[1]) + f64::from(max[1])) / 2.0,
    )
}

/// Largest deviation from a pure translation still treated as the same geometry: well below
/// the slicing resolution, above the f32 rounding of placing one mesh at two positions.
fn tolerance(a: &TriangleMesh) -> f64 {
    let (min, max) = a.bounds();
    let extent = min
        .iter()
        .chain(&max)
        .fold(0f32, |m, v| m.max(v.abs()))
        .max(1.);
    (1e-4f64).max(16. * f64::from(f32::EPSILON) * f64::from(extent))
}

/// Whether `b` is `a` moved by `d` (mm, XY) within `tol`, vertex for vertex.
fn translated(a: &TriangleMesh, b: &TriangleMesh, d: (f64, f64), tol: f64) -> bool {
    a.faces == b.faces
        && a.vertices.len() == b.vertices.len()
        && a.vertices.iter().zip(&b.vertices).all(|(p, q)| {
            (f64::from(q[0]) - f64::from(p[0]) - d.0).abs() <= tol
                && (f64::from(q[1]) - f64::from(p[1]) - d.1).abs() <= tol
                && (f64::from(q[2]) - f64::from(p[2])).abs() <= tol
        })
}

/// The scaled XY shift that turns `a`'s prepared object into `b`'s, if they may share it.
pub fn shift_between(a: &Candidate, b: &Candidate) -> Option<Point> {
    if a.tools != b.tools
        || a.mesh.faces.len() != b.mesh.faces.len()
        || a.region.is_some() != b.region.is_some()
        || a.config != b.config
    {
        return None;
    }
    let (ca, cb) = (centre(a.mesh), centre(b.mesh));
    let shift = Point::new(cb.x - ca.x, cb.y - ca.y);
    let d = (unscale(shift.x), unscale(shift.y));
    let tol = tolerance(a.mesh).max(tolerance(b.mesh));
    if !translated(a.mesh, b.mesh, d, tol) {
        return None;
    }
    if let (Some(ra), Some(rb)) = (a.region, b.region) {
        let volumes = ra.volumes.len() == rb.volumes.len()
            && ra.volumes.iter().zip(&rb.volumes).all(|(va, vb)| {
                va.kind == vb.kind
                    && va.config == vb.config
                    && va.painting == vb.painting
                    && translated(&va.mesh, &vb.mesh, d, tol)
            });
        let ranges = ra.height_ranges.len() == rb.height_ranges.len()
            && ra
                .height_ranges
                .iter()
                .zip(&rb.height_ranges)
                .all(|(x, y)| x.min_z == y.min_z && x.max_z == y.max_z && x.config == y.config);
        let ears = ra.brim_ears.len() == rb.brim_ears.len()
            && ra.brim_ears.iter().zip(&rb.brim_ears).all(|(x, y)| {
                (y.x - x.x - d.0).abs() <= tol
                    && (y.y - x.y - d.1).abs() <= tol
                    && x.z == y.z
                    && x.radius == y.radius
            });
        if !volumes
            || !ranges
            || !ears
            || ra.layer_height_profile != rb.layer_height_profile
            || ra.full_spectrum != rb.full_spectrum
        {
            return None;
        }
    }
    Some(shift)
}

/// For every candidate, the index of the candidate whose prepared object it uses (itself for
/// the first of each group) and its shift from it. Groups keep input order, like upstream's
/// per-object instance lists, and the first instance is prepared where it stands, so a
/// single-instance plate is unchanged.
pub fn group(candidates: &[Candidate]) -> Vec<(usize, Point)> {
    let mut leaders: Vec<usize> = Vec::new();
    candidates
        .iter()
        .enumerate()
        .map(|(i, c)| {
            for &l in &leaders {
                if let Some(shift) = shift_between(&candidates[l], c) {
                    return (l, shift);
                }
            }
            leaders.push(i);
            (i, Point::default())
        })
        .collect()
}

fn shift_points(points: &mut [Point], shift: Point) {
    for p in points {
        p.x += shift.x;
        p.y += shift.y;
    }
}

/// `polygons` moved by `shift`.
pub fn shifted_polygons(polygons: &[ExPolygon], shift: Point) -> Vec<ExPolygon> {
    let mut out = polygons.to_vec();
    if shift != Point::default() {
        for ex in &mut out {
            shift_points(&mut ex.contour, shift);
            for hole in &mut ex.holes {
                shift_points(hole, shift);
            }
        }
    }
    out
}

/// A path of the shared object, as printed by the instance at `shift` (upstream sets the
/// G-code origin to the instance's shift before it emits the object's extrusions).
pub fn shift_path(path: &mut ExtrusionPath, shift: Point) {
    if shift == Point::default() {
        return;
    }
    shift_points(&mut path.points, shift);
    if let Some(wipe) = path.seam.as_mut().and_then(|s| s.wipe.as_mut()) {
        wipe.x += shift.x;
        wipe.y += shift.y;
    }
}

/// [`shift_path`] for a whole layer.
pub fn shift_layer(layer: &mut LayerToolpaths, shift: Point) {
    for path in &mut layer.paths {
        shift_path(path, shift);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cube(x: f32, y: f32) -> TriangleMesh {
        let mut m = TriangleMesh::load_stl(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/models/cube20.stl"),
        )
        .unwrap();
        m.place_on_bed(x, y);
        m
    }

    #[test]
    fn translated_copies_share_and_rotated_ones_do_not() {
        let c = Config::defaults();
        let tools = ToolAssignments([Some(0), Some(0), Some(0), None, None]);
        let a = cube(100., 100.);
        let b = cube(137.3, 61.9);
        let mut rotated = cube(100., 100.);
        for v in &mut rotated.vertices {
            *v = [200. - v[1], v[0], v[2]];
        }
        let mut other = c.clone();
        other.set_raw("perimeters", "7").unwrap();
        let cand = |mesh, config| Candidate {
            mesh,
            region: None,
            tools: &tools,
            config,
        };
        let groups = group(&[
            cand(&a, &c),
            cand(&b, &c),
            cand(&rotated, &c),
            cand(&b, &other),
        ]);
        assert_eq!(groups[0], (0, Point::default()));
        assert_eq!(groups[1].0, 0);
        assert!((unscale(groups[1].1.x) - 37.3).abs() < 1e-4);
        assert!((unscale(groups[1].1.y) + 38.1).abs() < 1e-4);
        assert_eq!(groups[2], (2, Point::default()));
        assert_eq!(groups[3], (3, Point::default()));
    }
}
