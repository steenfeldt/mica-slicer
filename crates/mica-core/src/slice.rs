// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/TriangleMeshSlicer.cpp :: slice_facet, chain_lines_by_triangle_connectivity, make_expolygons
// Changes: Re-implemented in Rust for regular non-zero and organic-branch positive slicing: no colour/MMU slicing, no cut-mesh
// Changes: mode, no horizontal-facet output. Open chains are joined by nearest endpoints instead of
// Changes: PrusaSlicer's multi-stage gap closing. Facets are bucketed per layer before slicing.
// Source: prusaslicer@b028299c77 src/libslic3r/TriangleMeshSlicer.cpp :: v3f_scaled_to_contour_point, make_trafo_for_slicing, transform_mesh_vertices_for_slicing
// Source: prusaslicer@b028299c77 src/libslic3r/PrintObject.cpp :: PrintObject::PrintObject
// Source: prusaslicer@b028299c77 src/libslic3r/Model.cpp :: ModelVolume::center_geometry_after_creation
// Changes: Prusa profiles with a plain object's model frame (an STL or one-volume 3MF object as loaded, and its
// Changes: instance transform, from the CLI or the GUI): float slice heights, the double-half rounding of edge
// Changes: crossings, and the float object frame (`prusa_object_mesh`, instances that only move the volume);
// Changes: otherwise (Orca, regions, turned or scaled instances) nearest rounding.
// Source: prusaslicer@b028299c77 src/libslic3r/TriangleMeshSlicer.cpp :: make_expolygons
// Changes: with those Prusa roundings the gap closing runs on the Clipper 6.4 port (`clip64`).
// Source: orca@8500fcdcca src/libslic3r/TriangleMeshSlicer.cpp :: slice_facet, transform_mesh_vertices_for_slicing
// Changes: Opt-in branch meshes use Positive fill and upstream float-scaled XY under both rules.

//! Cutting a mesh into per-layer outlines.
//!
//! Each triangle that crosses a layer plane yields one segment. Rotating the triangle so its lowest
//! vertex comes first gives every segment the same orientation (outside on the right), so chaining
//! segments end to end produces counter-clockwise outer contours and clockwise holes. Segments are
//! chained by the mesh edge or vertex they pass through, which is exact for a watertight mesh.

use std::collections::HashMap;

use rayon::prelude::*;

use crate::geometry::clip::{Join, offset_ex, union_ex_positive};
use crate::geometry::{ExPolygon, Point, Polygon, SCALE, scale};
use crate::mesh::TriangleMesh;

/// Default gap-closing radius in mm (PrusaSlicer's `slice_closing_radius`).
pub const DEFAULT_CLOSING_RADIUS: f64 = 0.049;

/// The configuration's `slice_closing_radius` (PrintObjectSlice's `params_base.closing_radius`,
/// both slicers), [`DEFAULT_CLOSING_RADIUS`] when it is missing or not a non-negative number.
pub fn closing_radius(c: &crate::config::Config) -> f64 {
    c.float("slice_closing_radius")
        .filter(|v| v.is_finite() && *v >= 0.)
        .unwrap_or(DEFAULT_CLOSING_RADIUS)
}

/// Where a segment endpoint lies on the mesh: on an edge, or exactly on a vertex.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Anchor {
    Edge(u32),
    Vertex(u32),
}

#[derive(Clone, Copy, Debug)]
struct Segment {
    a: Point,
    b: Point,
    a_at: Anchor,
    b_at: Anchor,
}

/// Unique ids for undirected mesh edges, per face (edge k runs from vertex k to k+1).
fn face_edge_ids(mesh: &TriangleMesh) -> Vec<[u32; 3]> {
    // Vertex → incident faces as two flat arrays (compressed rows), like PrusaSlicer's
    // its_face_edge_ids; far smaller than a hash map over all edges for multi-million-face meshes.
    let nv = mesh.vertices.len();
    let mut offsets = vec![0u32; nv + 1];
    for f in &mesh.faces {
        for &v in f {
            offsets[v as usize + 1] += 1;
        }
    }
    for i in 0..nv {
        offsets[i + 1] += offsets[i];
    }
    let mut fill = offsets.clone();
    let mut vertex_faces = vec![0u32; mesh.faces.len() * 3];
    for (fi, f) in mesh.faces.iter().enumerate() {
        for &v in f {
            vertex_faces[fill[v as usize] as usize] = fi as u32;
            fill[v as usize] += 1;
        }
    }
    drop(fill);

    const UNSET: u32 = u32::MAX;
    let mut ids = vec![[UNSET; 3]; mesh.faces.len()];
    let mut next = 0u32;
    for fi in 0..mesh.faces.len() {
        for k in 0..3 {
            if ids[fi][k] != UNSET {
                continue;
            }
            let f = mesh.faces[fi];
            let (a, b) = (f[k], f[(k + 1) % 3]);
            ids[fi][k] = next;
            // Every other face with the edge a–b (either direction) gets the same id.
            let (lo, hi) = (
                offsets[a as usize] as usize,
                offsets[a as usize + 1] as usize,
            );
            for &g in &vertex_faces[lo..hi] {
                if g as usize == fi {
                    continue;
                }
                let gf = mesh.faces[g as usize];
                for j in 0..3 {
                    let (c, d) = (gf[j], gf[(j + 1) % 3]);
                    if ((c == a && d == b) || (c == b && d == a)) && ids[g as usize][j] == UNSET {
                        ids[g as usize][j] = next;
                    }
                }
            }
            next += 1;
        }
    }
    ids
}

fn to_point(v: [f64; 3]) -> Point {
    Point::new((v[0] * SCALE).round() as i64, (v[1] * SCALE).round() as i64)
}

/// How intersection points snap to the scaled grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SliceRounding {
    /// Nearest grid point (Orca's `floor(x + 0.5)` up to the halfway rule).
    #[default]
    Nearest,
    /// PrusaSlicer 2.9's `slice_facet`: a vertex on the plane goes through
    /// `v3f_scaled_to_contour_point` (`floor(v + 0.5)`), but an edge crossing is interpolated as
    /// `a·(1 − t) + b·t + (0.5, 0.5)` and then passed through the same function, so it is rounded
    /// with a second half added: `floor(p + 1)`. Every general intersection lands one unit up
    /// in X and Y (a cube's ±10 mm walls slice to 10000001 / −9999999).
    Prusa,
    /// [`SliceRounding::Prusa`] on vertices that are already scaled in XY, as `float`s
    /// (`transform_mesh_vertices_for_slicing`); see [`prusa_object_mesh`].
    PrusaScaled,
    /// Orca branch meshes after float scaling in XY; nearest crossing rounding.
    OrcaScaled,
}

impl SliceRounding {
    fn xy_scale(self) -> f64 {
        if matches!(self, Self::PrusaScaled | Self::OrcaScaled) {
            1.0
        } else {
            SCALE
        }
    }

    fn vertex(self, v: [f64; 3]) -> Point {
        match self {
            Self::Nearest => to_point(v),
            Self::Prusa => Point::new(
                (v[0] * SCALE + 0.5).floor() as i64,
                (v[1] * SCALE + 0.5).floor() as i64,
            ),
            // `v3f_scaled_to_contour_point` on a float vertex adds the half in float.
            Self::PrusaScaled | Self::OrcaScaled => Point::new(
                (v[0] as f32 + 0.5).floor() as i64,
                (v[1] as f32 + 0.5).floor() as i64,
            ),
        }
    }

    fn crossing(self, a: [f64; 3], b: [f64; 3], t: f64) -> Point {
        if self == Self::Nearest {
            return to_point([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, 0.]);
        }
        if self == Self::OrcaScaled {
            return Point::new(
                (a[0] + (b[0] - a[0]) * t + 0.5).floor() as i64,
                (a[1] + (b[1] - a[1]) * t + 0.5).floor() as i64,
            );
        }
        let s = self.xy_scale();
        let along = |i: usize| (a[i] * s) * (1. - t) + (b[i] * s) * t + 0.5;
        Point::new(
            (along(0) + 0.5).floor() as i64,
            (along(1) + 0.5).floor() as i64,
        )
    }
}

/// A plain object's model frame (the mesh as loaded and its instance transform, with `placed`
/// the frame placed on the bed) in PrusaSlicer's slicing frame, for an instance that only
/// moves the volume: the volume centred in float (`ModelVolume::center_geometry_after_creation`),
/// then `make_trafo_for_slicing(trafo_centered() * volume.get_matrix())` applied in float: XY
/// scaled by 10^6 about the object's centre (its bounding-box centre truncated to the 1 nm
/// grid), Z on the bed. The XY coordinates of the result are scaled units. The offset is read
/// off the placed mesh (`placed` must be a translated copy of `source`).
///
/// A turning, scaling or mirroring instance gives `None` (nearest rounding of the placed mesh):
/// PrusaSlicer's float products there depend on how its build contracts them, and a direct port
/// did not bring a turned cube closer to PrusaSlicer 2.9.6.
pub fn prusa_object_mesh(
    frame: &crate::input::Frame,
    placed: &TriangleMesh,
) -> Option<TriangleMesh> {
    let t = frame.trafo.0;
    let linear_identity = (0..3).all(|r| (0..3).all(|c| t[r][c] == f64::from(u8::from(r == c))));
    if !linear_identity {
        return None;
    }
    let source = frame.source.as_ref();
    if source.faces != placed.faces || source.vertices.len() != placed.vertices.len() {
        return None;
    }
    let (smin, smax) = source.bounds();
    let (pmin, pmax) = placed.bounds();
    let mid = |a: f32, b: f32| (f64::from(a) + f64::from(b)) / 2.0;
    let d = [
        mid(pmin[0], pmax[0]) - mid(smin[0], smax[0]),
        mid(pmin[1], pmax[1]) - mid(smin[1], smax[1]),
        f64::from(pmin[2]) - f64::from(smin[2]),
    ];
    let translated = source
        .vertices
        .iter()
        .zip(&placed.vertices)
        .all(|(s, p)| (0..3).all(|k| (f64::from(s[k]) + d[k] - f64::from(p[k])).abs() < 1e-3));
    if !translated {
        return None;
    }
    // Volume: vertices centred in float, the double shift kept as the volume offset.
    let shift: [f64; 3] = std::array::from_fn(|k| mid(smin[k], smax[k]));
    let centred: Vec<[f32; 3]> = source
        .vertices
        .iter()
        .map(|v| std::array::from_fn(|k| v[k] + -(shift[k] as f32)))
        .collect();
    // Raw bounding box in the instance frame (no translation): centred vertices plus the offset.
    let mut bmin = [f64::INFINITY; 3];
    let mut bmax = [f64::NEG_INFINITY; 3];
    for v in &centred {
        for k in 0..3 {
            let p = f64::from(v[k]) + shift[k];
            bmin[k] = bmin[k].min(p);
            bmax[k] = bmax[k].max(p);
        }
    }
    // `PrintObject::m_center_offset = Point::new_scale(bbox_center)`, removed by `trafo_centered`.
    let centre: [f64; 2] = std::array::from_fn(|k| {
        let c = (bmin[k] + bmax[k]) / 2.0;
        (c / 0.000_001) as i64 as f64 * 0.000_001
    });
    let t = [
        ((shift[0] - centre[0]) / 0.000_001) as f32,
        ((shift[1] - centre[1]) / 0.000_001) as f32,
        // Z on the bed, or as high as the placed mesh floats (Mica keeps such objects in place).
        (shift[2] - bmin[2] + f64::from(pmin[2])) as f32,
    ];
    let s = (1.0 / 0.000_001) as f32;
    let vertices = centred
        .iter()
        .map(|v| [s * v[0] + t[0], s * v[1] + t[1], v[2] + t[2]])
        .collect();
    Some(TriangleMesh {
        vertices,
        faces: source.faces.clone(),
    })
}

/// Intersects one triangle with the plane `z`. Returns `None` if it doesn't cross.
fn slice_facet(
    z: f64,
    verts: [[f64; 3]; 3],
    ids: [u32; 3],
    edge_ids: [u32; 3],
    rounding: SliceRounding,
) -> Option<Segment> {
    // Start from the lowest vertex so all segments come out with the same orientation.
    let lowest = (0..3)
        .min_by(|&i, &j| verts[i][2].total_cmp(&verts[j][2]))
        .unwrap();
    let mut points: [(Point, Anchor); 3] = [(Point::default(), Anchor::Edge(0)); 3];
    let mut n = 0;
    let mut on_plane: Option<usize> = None;

    for j in 0..3 {
        let k = (lowest + j) % 3;
        let l = (k + 1) % 3;
        let (mut a, mut b) = (verts[k], verts[l]);
        let (mut a_id, mut b_id) = (ids[k], ids[l]);

        if a[2] == z && b[2] == z {
            // Edge lies in the plane. It belongs to the slice only if the third vertex is below,
            // i.e. it is the top edge of this triangle; then it runs b → a.
            let third_below = verts.iter().any(|v| v[2] < z);
            if !third_below {
                return None;
            }
            std::mem::swap(&mut a, &mut b);
            std::mem::swap(&mut a_id, &mut b_id);
            return Some(Segment {
                a: rounding.vertex(a),
                b: rounding.vertex(b),
                a_at: Anchor::Vertex(a_id),
                b_at: Anchor::Vertex(b_id),
            });
        }

        if a[2] == z || b[2] == z {
            let (v, id) = if a[2] == z { (a, a_id) } else { (b, b_id) };
            if on_plane.is_none_or(|i| points[i].1 != Anchor::Vertex(id)) {
                on_plane = Some(n);
                points[n] = (rounding.vertex(v), Anchor::Vertex(id));
                n += 1;
            }
        } else if (a[2] < z) != (b[2] < z) {
            // Order the edge by vertex id so both neighbouring triangles compute the same point.
            if a_id > b_id {
                std::mem::swap(&mut a, &mut b);
            }
            let t = (z - a[2]) / (b[2] - a[2]);
            points[n] = (rounding.crossing(a, b, t), Anchor::Edge(edge_ids[k]));
            n += 1;
        }
    }

    // A triangle crosses a plane at two points, or merely touches it at one vertex.
    (n == 2).then(|| Segment {
        a: points[1].0,
        b: points[0].0,
        a_at: points[1].1,
        b_at: points[0].1,
    })
}

/// Chains segments into closed loops. Unclosed chains are returned separately.
fn chain_segments(segments: &[Segment]) -> (Vec<Polygon>, Vec<Vec<Point>>) {
    let mut by_start: HashMap<Anchor, Vec<usize>> = HashMap::with_capacity(segments.len());
    for (i, s) in segments.iter().enumerate() {
        by_start.entry(s.a_at).or_default().push(i);
    }
    let mut used = vec![false; segments.len()];
    let mut loops = Vec::new();
    let mut open = Vec::new();

    for start in 0..segments.len() {
        if used[start] {
            continue;
        }
        used[start] = true;
        let first = segments[start];
        let mut pts = vec![first.a];
        let mut end = first;
        let closed = loop {
            pts.push(end.b);
            if end.b_at == first.a_at {
                break true;
            }
            let next = by_start
                .get(&end.b_at)
                .and_then(|cands| cands.iter().copied().find(|&i| !used[i]));
            match next {
                Some(i) => {
                    used[i] = true;
                    end = segments[i];
                }
                None => break false,
            }
        };
        if closed {
            pts.pop(); // last point repeats the first
            // Segments that rounded to a single point still link the chain; drop their duplicates now.
            pts.dedup();
            while pts.len() > 1 && pts.first() == pts.last() {
                pts.pop();
            }
            if pts.len() >= 3 {
                loops.push(pts);
            }
        } else {
            open.push(pts);
        }
    }
    (loops, open)
}

/// Joins open chains whose ends nearly meet into loops (nearest-endpoint matching).
fn close_open_chains(mut open: Vec<Vec<Point>>, max_gap: f64) -> Vec<Polygon> {
    let mut loops = Vec::new();
    while let Some(mut chain) = open.pop() {
        loop {
            let (first, last) = (chain[0], *chain.last().unwrap());
            if chain.len() >= 3 && last.distance_to(first) <= max_gap {
                loops.push(chain);
                break;
            }
            let best = open
                .iter()
                .enumerate()
                .map(|(i, c)| (i, last.distance_to(c[0])))
                .min_by(|a, b| a.1.total_cmp(&b.1));
            match best {
                Some((i, d)) if d <= max_gap => {
                    let next = open.swap_remove(i);
                    chain.extend(next);
                }
                _ => break, // give up on this chain
            }
        }
    }
    loops
}

/// Result of slicing one layer.
#[derive(Clone, Debug, Default)]
pub struct LayerSlice {
    pub islands: Vec<ExPolygon>,
    /// Segments that could not be chained into loops (non-manifold input).
    pub dropped_chains: usize,
}

/// Slices `mesh` at each height in `zs` (mm). Layers are processed in parallel.
pub fn slice_mesh(mesh: &TriangleMesh, zs: &[f64], closing_radius: f64) -> Vec<LayerSlice> {
    slice_mesh_observed(
        mesh,
        zs,
        closing_radius,
        SliceRounding::Nearest,
        &crate::progress::Silent,
    )
}

/// [`slice_mesh`] reporting one tick per layer; layers after a cancel come back empty (the
/// caller checks the observer and discards them).
pub fn slice_mesh_observed(
    mesh: &TriangleMesh,
    zs: &[f64],
    closing_radius: f64,
    rounding: SliceRounding,
    observer: &dyn crate::progress::Observer,
) -> Vec<LayerSlice> {
    slice_mesh_with_mode(mesh, zs, closing_radius, rounding, observer, false, 0.)
}

/// [`slice_mesh_observed`] (nearest rounding) for dense meshes: every chained loop is first
/// simplified by `tolerance` (mm), before the union and closing offsets, which would otherwise
/// work on every facet crossing of a finely refined mesh.
pub fn slice_mesh_simplified(
    mesh: &TriangleMesh,
    zs: &[f64],
    closing_radius: f64,
    tolerance: f64,
    observer: &dyn crate::progress::Observer,
) -> Vec<LayerSlice> {
    slice_mesh_with_mode(
        mesh,
        zs,
        closing_radius,
        SliceRounding::Nearest,
        observer,
        false,
        tolerance,
    )
}

/// Organic branch meshes use upstream's Positive slicing mode (all loops reoriented CCW).
pub(crate) fn slice_branch_mesh(
    mesh: &TriangleMesh,
    zs: &[f64],
    rounding: SliceRounding,
) -> Vec<LayerSlice> {
    let mut mesh = mesh.clone();
    for v in &mut mesh.vertices {
        v[0] *= SCALE as f32;
        v[1] *= SCALE as f32;
    }
    let rounding = if rounding == SliceRounding::Nearest {
        SliceRounding::OrcaScaled
    } else {
        SliceRounding::PrusaScaled
    };
    slice_mesh_with_mode(&mesh, zs, 0., rounding, &crate::progress::Silent, true, 0.)
}

fn slice_mesh_with_mode(
    mesh: &TriangleMesh,
    zs: &[f64],
    closing_radius: f64,
    rounding: SliceRounding,
    observer: &dyn crate::progress::Observer,
    positive: bool,
    simplify: f64,
) -> Vec<LayerSlice> {
    // PrusaSlicer slices at `float` heights (`slice_mesh` takes `std::vector<float> zs`).
    let prusa_zs: Vec<f64>;
    let zs = if rounding != SliceRounding::Nearest {
        prusa_zs = zs.iter().map(|&z| f64::from(z as f32)).collect();
        &prusa_zs[..]
    } else {
        zs
    };
    let ticker = crate::progress::Ticker::new(observer, crate::progress::Stage::Slicing, zs.len());
    let _timing = crate::timing::Span::new(crate::timing::Stage::MeshSlicing);
    let edge_ids = face_edge_ids(mesh);
    let vert = |i: u32| mesh.vertices[i as usize].map(f64::from);

    // Bucket facets by the layers they span, so each layer only looks at its own facets.
    let mut buckets: Vec<Vec<u32>> = vec![Vec::new(); zs.len()];
    for (fi, f) in mesh.faces.iter().enumerate() {
        let fz = f.map(|i| vert(i)[2]);
        let (lo, hi) = (fz[0].min(fz[1]).min(fz[2]), fz[0].max(fz[1]).max(fz[2]));
        if lo == hi {
            continue; // horizontal facets never produce a segment in regular slicing
        }
        let start = zs.partition_point(|&z| z < lo);
        let end = zs.partition_point(|&z| z <= hi);
        for b in &mut buckets[start..end] {
            b.push(fi as u32);
        }
    }

    let max_gap = 2.0 * scale(closing_radius.max(0.01)) as f64;
    zs.par_iter()
        .zip(buckets.par_iter())
        .map(|(&z, facets)| {
            if ticker.cancelled() {
                return LayerSlice {
                    islands: Vec::new(),
                    dropped_chains: 0,
                };
            }
            ticker.tick();
            let segments: Vec<Segment> = facets
                .iter()
                .filter_map(|&fi| {
                    let f = mesh.faces[fi as usize];
                    slice_facet(z, f.map(vert), f, edge_ids[fi as usize], rounding)
                })
                .collect();
            let (mut loops, open) = chain_segments(&segments);
            let open_count = open.len();
            let recovered = close_open_chains(open, max_gap);
            let dropped_chains = open_count.saturating_sub(recovered.len());
            loops.extend(recovered);
            if simplify > 0. {
                let eps = scale(simplify) as f64;
                for l in &mut loops {
                    *l = crate::geometry::clip::simplify_ring(l, eps);
                }
                loops.retain(|l| l.len() >= 3);
            }
            LayerSlice {
                // PrusaSlicer's slicing rules close gaps with its Clipper 6.4 geometry.
                islands: if positive {
                    // TriangleMeshSlicer Positive mode: every loop is made CCW, so inverted
                    // lobes and inner loops of a self-overlapping tube count as solid.
                    for l in &mut loops {
                        if crate::geometry::signed_area(l) < 0.0 {
                            l.reverse();
                        }
                    }
                    crate::geometry::clip64::union_ex(&loops, clipper2_rust::FillRule::Positive)
                } else if rounding != SliceRounding::Nearest {
                    make_expolygons_prusa(&loops, closing_radius)
                } else {
                    make_expolygons(&loops, closing_radius)
                },
                dropped_chains,
            }
        })
        .collect()
}

/// Turns loops into ExPolygons, closing gaps up to `closing_radius` mm (grow, then shrink back).
fn make_expolygons(loops: &[Polygon], closing_radius: f64) -> Vec<ExPolygon> {
    let ex = union_ex_positive_nonzero(loops);
    if closing_radius <= 0.0 || ex.is_empty() {
        return ex;
    }
    let r = scale(closing_radius) as f64;
    offset_ex(&offset_ex(&ex, r, Join::Miter), -r, Join::Miter)
}

/// PrusaSlicer's `make_expolygons` (TriangleMeshSlicer.cpp) with its Clipper 6.4 geometry:
/// `offset2_ex(union_ex(loops, pftNonZero), +r, -r)` with the radius as a `float`.
fn make_expolygons_prusa(loops: &[Polygon], closing_radius: f64) -> Vec<ExPolygon> {
    use crate::geometry::clip64;
    let ex = clip64::union_ex(loops, clipper2_rust::FillRule::NonZero);
    if closing_radius <= 0.0 || ex.is_empty() {
        return ex;
    }
    let r = f64::from(closing_radius as f32) / 1e-6;
    clip64::offset2_ex(
        &ex,
        r,
        -r,
        Join::Miter,
        crate::geometry::clip::DEFAULT_MITER_LIMIT,
    )
}

/// Non-zero union: loops of either orientation add, and clockwise loops nested in counter-clockwise
/// ones cancel out to form holes.
fn union_ex_positive_nonzero(loops: &[Polygon]) -> Vec<ExPolygon> {
    // For consistently oriented slicer output, positive and non-zero fill agree; positive also drops
    // stray clockwise loops that are not inside anything (e.g. inverted shells).
    union_ex_positive(loops)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::tests::box_mesh;

    /// A square with a 64-gon hole whose chords carry an extra point at 37 %, like the edge
    /// crossings a cylinder's diagonal facet edges leave in a slice.
    fn square_with_round_hole() -> Vec<Polygon> {
        let r = |v: f64| (v + 0.5).floor() as i64;
        let square = [
            (-10000000, -10000000),
            (1000, -10000000),
            (10000000, -10000000),
            (10000000, 3000),
            (10000000, 10000000),
            (-10000000, 10000000),
        ];
        let mut hole = Vec::new();
        let n = 64;
        for i in 0..n {
            let a0 = 2. * std::f64::consts::PI * i as f64 / n as f64;
            let a1 = 2. * std::f64::consts::PI * (i + 1) as f64 / n as f64;
            let p0 = (4000000. * a0.cos() + 1., 4000000. * a0.sin() + 1.);
            let p1 = (4000000. * a1.cos() + 1., 4000000. * a1.sin() + 1.);
            hole.push(Point::new(r(p0.0), r(p0.1)));
            hole.push(Point::new(
                r(p0.0 + (p1.0 - p0.0) * 0.37),
                r(p0.1 + (p1.1 - p0.1) * 0.37),
            ));
        }
        hole.reverse();
        vec![
            square.iter().map(|&(x, y)| Point::new(x, y)).collect(),
            hole,
        ]
    }

    /// Σx², Σy², Σxy of a ring: a start-independent fingerprint of its exact vertices.
    fn moments(ring: &[Point]) -> (i128, i128, i128) {
        ring.iter().fold((0, 0, 0), |(a, b, c), p| {
            let (x, y) = (i128::from(p.x), i128::from(p.y));
            (a + x * x, b + y * y, c + x * y)
        })
    }

    #[test]
    fn prusa_gap_closing_matches_clipper64_reference() {
        // Reference: PrusaSlicer 2.9.6's make_expolygons (offset2_ex(union_ex(loops), +r, -r) on
        // its Clipper 6.4, built from the pinned sources) on the same loops: the collinear
        // square points vanish, and the hole keeps 128 vertices with these exact moments.
        let loops = square_with_round_hole();
        let ex = make_expolygons_prusa(&loops, DEFAULT_CLOSING_RADIUS);
        assert_eq!(ex.len(), 1);
        assert_eq!(ex[0].contour.len(), 4);
        assert_eq!(
            moments(&ex[0].contour),
            (400000000000000, 400000000000000, 0)
        );
        assert_eq!(ex[0].holes.len(), 1);
        assert_eq!(ex[0].holes[0].len(), 128);
        assert_eq!(
            moments(&ex[0].holes[0]),
            (1022850561235856, 1022850561235856, 128)
        );
        // Clipper2's truncated intersections shrink the hole by a unit here and there.
        let clipper2 = make_expolygons(&loops, DEFAULT_CLOSING_RADIUS);
        assert_ne!(moments(&clipper2[0].holes[0]), moments(&ex[0].holes[0]));
    }

    fn areas(layer: &LayerSlice) -> f64 {
        layer.islands.iter().map(ExPolygon::area_mm2).sum()
    }

    #[test]
    fn box_slices_to_one_ccw_square() {
        let m = box_mesh([0.0; 3], [20.0, 10.0, 5.0]);
        let layers = slice_mesh(&m, &[0.1, 2.5, 4.9], DEFAULT_CLOSING_RADIUS);
        for l in &layers {
            assert_eq!(l.islands.len(), 1);
            assert_eq!(l.dropped_chains, 0);
            assert!((areas(l) - 200.0).abs() < 1e-6, "area {}", areas(l));
            assert!(crate::geometry::signed_area(&l.islands[0].contour) > 0.0);
        }
    }

    fn bounds_of(islands: &[ExPolygon]) -> (Point, Point) {
        let pts = islands.iter().flat_map(|e| e.contour.iter());
        let min = pts.clone().fold(Point::new(i64::MAX, i64::MAX), |m, p| {
            Point::new(m.x.min(p.x), m.y.min(p.y))
        });
        let max = pts.fold(Point::new(i64::MIN, i64::MIN), |m, p| {
            Point::new(m.x.max(p.x), m.y.max(p.y))
        });
        (min, max)
    }

    #[test]
    fn prusa_rounding_moves_edge_crossings_one_unit_up() {
        // PrusaSlicer 2.9 adds the rounding half twice to an edge crossing: a 20 mm cube centred on
        // the origin slices to 10000001 / -9999999, not +-10000000.
        let m = box_mesh([-10.0, -10.0, 0.0], [10.0, 10.0, 20.0]);
        let nearest = slice_mesh(&m, &[10.5], 0.0);
        assert_eq!(
            bounds_of(&nearest[0].islands),
            (
                Point::new(-10_000_000, -10_000_000),
                Point::new(10_000_000, 10_000_000)
            )
        );
        let prusa = slice_mesh_observed(
            &m,
            &[10.5],
            0.0,
            SliceRounding::Prusa,
            &crate::progress::Silent,
        );
        assert_eq!(
            bounds_of(&prusa[0].islands),
            (
                Point::new(-9_999_999, -9_999_999),
                Point::new(10_000_001, 10_000_001)
            )
        );
    }

    #[test]
    fn prusa_object_mesh_centres_and_scales_the_loaded_mesh() {
        // Loaded at 0..20, placed at 115..135 / 100..120: the slicing frame is centred and scaled.
        let source = box_mesh([0.0; 3], [20.0, 20.0, 20.0]);
        let placed = box_mesh([115.0, 100.0, 0.0], [135.0, 120.0, 20.0]);
        let frame = crate::input::Frame {
            source: std::sync::Arc::new(source.clone()),
            trafo: crate::threemf::Transform::IDENTITY,
        };
        let object = prusa_object_mesh(&frame, &placed).unwrap();
        let (min, max) = object.bounds();
        assert_eq!(min, [-10_000_000.0, -10_000_000.0, 0.0]);
        assert_eq!(max, [10_000_000.0, 10_000_000.0, 20.0]);
        let layers = slice_mesh_observed(
            &object,
            &[10.5],
            0.0,
            SliceRounding::PrusaScaled,
            &crate::progress::Silent,
        );
        assert_eq!(
            bounds_of(&layers[0].islands),
            (
                Point::new(-9_999_999, -9_999_999),
                Point::new(10_000_001, 10_000_001)
            )
        );
        // Not a translated copy: no slicing frame.
        let other = box_mesh([115.0, 100.0, 0.0], [136.0, 120.0, 20.0]);
        assert!(prusa_object_mesh(&frame, &other).is_none());
        // A turned instance: no float frame either.
        let turned = crate::input::Frame {
            source: std::sync::Arc::new(source.clone()),
            trafo: crate::threemf::Transform([
                [0.0, -1.0, 0.0, 125.0],
                [1.0, 0.0, 0.0, 105.0],
                [0.0, 0.0, 1.0, 0.0],
            ]),
        };
        assert!(prusa_object_mesh(&turned, &crate::input::place(&turned)).is_none());
    }

    #[test]
    fn plane_through_vertices_still_slices() {
        // z = 0 and z = 5 hit vertices; the in-between plane hits the diagonal edges' vertices too.
        let m = box_mesh([0.0; 3], [10.0, 10.0, 5.0]);
        let layers = slice_mesh(&m, &[2.5], 0.0);
        assert!((areas(&layers[0]) - 100.0).abs() < 1e-6);
        // A plane exactly through the top face: triangles below own their top edges.
        let top = slice_mesh(&m, &[5.0], 0.0);
        assert!((areas(&top[0]) - 100.0).abs() < 1e-6, "{}", areas(&top[0]));
    }

    #[test]
    fn hole_block_has_a_hole() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/models/");
        let m = TriangleMesh::load_stl(format!("{dir}hole_block.stl")).unwrap();
        let layers = slice_mesh(&m, &[0.1, 5.0, 9.9], DEFAULT_CLOSING_RADIUS);
        for l in &layers {
            assert_eq!(l.islands.len(), 1);
            assert_eq!(l.islands[0].holes.len(), 1);
            // 400 mm² minus a 64-gon of radius 4 (50.185 mm²).
            assert!((areas(l) - 349.815).abs() < 1e-2, "area {}", areas(l));
        }
    }

    #[test]
    fn benchy_slices_without_open_contours() {
        // Regression: planes passing a hair below vertices (0.2·22 + 0.1 = 4.499999999999999 vs 4.5)
        // create segments that round to a single point; dropping them broke 66 of 240 layers.
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/models/");
        let mut m = TriangleMesh::load_stl(format!("{dir}3DBenchy.stl")).unwrap();
        m.place_on_bed(125.0, 110.0);
        let zs: Vec<f64> = (0..240).map(|i| 0.2 + i as f64 * 0.2 - 0.1).collect();
        let layers = slice_mesh(&m, &zs, DEFAULT_CLOSING_RADIUS);
        let dropped: usize = layers.iter().map(|l| l.dropped_chains).sum();
        assert_eq!(dropped, 0);
    }

    #[test]
    fn two_separate_boxes_give_two_islands() {
        let mut tris = Vec::new();
        for m in [
            box_mesh([0.0; 3], [5.0, 5.0, 5.0]),
            box_mesh([10.0, 0.0, 0.0], [15.0, 5.0, 5.0]),
        ] {
            for f in &m.faces {
                tris.push(f.map(|i| m.vertices[i as usize]));
            }
        }
        let both = TriangleMesh::from_triangles(&tris);
        let layers = slice_mesh(&both, &[2.0], DEFAULT_CLOSING_RADIUS);
        assert_eq!(layers[0].islands.len(), 2);
    }
}
