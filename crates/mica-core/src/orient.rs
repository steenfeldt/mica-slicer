// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: orca@8500fcdcca src/libslic3r/Orient.cpp :: AutoOrienter::process, AutoOrienter::area_cumulation, AutoOrienter::add_supplements, AutoOrienter::get_cost_items, AutoOrienter::target_function
// Source: orca@8500fcdcca src/libslic3r/Orient.hpp :: OrientParams, OrientParamsArea
// Changes: Own f64 vector math and quickhull instead of Eigen/igl and TriangleMesh::convex_hull_3d; normals quantised with
// Changes: round() rather than floor(); duplicate candidates merged within 1e-4 instead of 1e-7; cost ties within EPSILON pick the
// Changes: candidate nearest the current "down" (Orca only prefers the identity); no appearance-face penalty (no face types here);
// Changes: candidates are scored in parallel and returned sorted; reported bottom/overhang areas are the plain face-area sums.
// Upstream attribution (Orient.cpp, Orient.hpp): the upstream files carry no copyright header. They are part of OrcaSlicer
// (SoftFever and contributors; inherited from BambuStudio, Bambu Lab), released under the terms of the AGPLv3 or higher.
// The cost model and its parameter names (TAR_A, RELATIVE_F, CONTOUR_F, BOTTOM_F, ASCENT, PLAFOND_ADV, FIRST_LAY_H, ...)
// follow Christoph Schranz's Tweaker-3 auto-orientation (MIT), which the upstream code was ported from.

//! Auto-orientation: pick the face that should rest on the bed.
//!
//! The search works like OrcaSlicer's FDM "Auto orient": candidate "down" directions come from
//! area-weighted clusters of the mesh face normals, the same for the convex hull, plus a fixed set
//! of axis and diagonal directions. Every candidate is scored by rotating it to −Z and summing the
//! face areas that become bottom contact, overhang, hull contact and near-horizontal ("low angle")
//! faces; the weighted combination (`Params`) is Orca's and the lowest cost wins.
//!
//! All geometry is on the mesh as the caller passes it; the result is a rotation about a world axis.

use rayon::prelude::*;
use std::collections::HashMap;

/// Upstream `EPSILON` (libslic3r.h); used for the first-layer band and cost ties.
const EPSILON: f64 = 1e-4;
/// Mesh-normal clusters taken as candidates (upstream `area_cumulation_accurate(..., 10)`).
const MESH_CLUSTERS: usize = 10;
/// Hull-normal clusters taken as candidates (upstream `area_cumulation_accurate(..., 14)`).
const HULL_CLUSTERS: usize = 14;
/// Candidates closer than this (Euclidean, on unit vectors) are one candidate.
const DUPLICATE_TOL: f64 = 1e-4;
const DOWN: [f64; 3] = [0.0, 0.0, -1.0];

/// Rotate the mesh about this world axis (through any point) by `angle` radians so the chosen
/// face lies flat on the bed. `axis` is unit, `angle` in [0, π]; the identity has `angle` 0.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rotation {
    pub axis: [f64; 3],
    pub angle: f64,
}

/// One scored "down" direction. `normal` is the direction (in the input frame) that is rotated
/// onto −Z; `overhang_area` and `bottom_area` are plain face-area sums in mm², `height` is the
/// object height in that orientation.
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub normal: [f64; 3],
    pub cost: f64,
    pub overhang_area: f64,
    pub bottom_area: f64,
    pub height: f64,
}

#[derive(Clone, Debug)]
pub struct Orientation {
    pub rotation: Rotation,
    pub best: Candidate,
    /// Every scored candidate, sorted by cost (ties by rotation angle). `best` is the cheapest up
    /// to `EPSILON`, preferring the smallest rotation.
    pub candidates: Vec<Candidate>,
}

/// Cost weights. Names follow upstream `OrientParams` / `OrientParamsArea` (Orient.hpp).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Params {
    /// Faces facing down within this angle of horizontal count as overhang (upstream
    /// `OrientMesh::overhang_angle`, fed from `support_threshold_angle`, default 30°).
    pub overhang_angle_deg: f64,
    /// Upstream `min_volume`: weight overhang area by steepness and height ("minimise support
    /// volume") instead of plain area ("minimise support area").
    pub min_volume: bool,
    pub tar_a: f64,
    pub tar_b: f64,
    pub relative_f: f64,
    pub contour_f: f64,
    pub bottom_f: f64,
    pub bottom_hull_f: f64,
    pub tar_c: f64,
    pub tar_d: f64,
    pub tar_e: f64,
    pub first_lay_h: f64,
    pub laf_max: f64,
    pub laf_min: f64,
    pub tar_laf: f64,
    pub tar_proj_area: f64,
    pub bottom_min: f64,
    pub use_low_angle_face: bool,
}

impl Params {
    /// Upstream `OrientParamsArea` with `min_volume = false`: what Orca's GUI uses by default
    /// (`OrientSettings::min_area = true`).
    pub const fn min_area() -> Self {
        Params {
            overhang_angle_deg: 30.0,
            min_volume: false,
            tar_a: 0.015,
            tar_b: 0.177,
            relative_f: 20.0,
            contour_f: 0.5,
            bottom_f: 2.5,
            bottom_hull_f: 0.1,
            tar_c: 0.1,
            tar_d: 1.0,
            tar_e: 0.0115,
            first_lay_h: 0.2,
            laf_max: 0.999,
            laf_min: 0.97,
            tar_laf: 0.001,
            tar_proj_area: 0.1,
            bottom_min: 0.1,
            use_low_angle_face: true,
        }
    }

    /// Upstream `OrientParams` with `min_volume = true` (Orca's "minimise support volume" mode).
    pub const fn min_volume() -> Self {
        Params {
            overhang_angle_deg: 30.0,
            min_volume: true,
            tar_a: 0.01,
            tar_b: 0.177,
            relative_f: 6.610_621_027_964_314,
            contour_f: 0.232_286_232_697_759_97,
            bottom_f: 1.167_152_017_941_474,
            bottom_hull_f: 0.1,
            tar_c: 0.243_080_704_769_247_26,
            tar_d: 0.628_451_550_816_087_1,
            tar_e: 0.0,
            first_lay_h: 0.2,
            laf_max: 0.999,
            laf_min: 0.9703,
            tar_laf: 0.01,
            tar_proj_area: 0.1,
            bottom_min: 0.1,
            use_low_angle_face: true,
        }
    }

    /// Upstream `ASCENT = cos(π − overhang_angle)`: faces with `normal · up` below it overhang.
    fn ascent(&self) -> f64 {
        (std::f64::consts::PI - self.overhang_angle_deg.to_radians()).cos()
    }
}

impl Default for Params {
    fn default() -> Self {
        Params::min_area()
    }
}

/// Best orientation of the mesh as placed in the world, with Orca's default parameters.
pub fn best_orientation(vertices: &[[f32; 3]], faces: &[[u32; 3]]) -> Orientation {
    best_orientation_with(vertices, faces, &Params::default())
}

/// Best orientation with explicit cost parameters.
pub fn best_orientation_with(
    vertices: &[[f32; 3]],
    faces: &[[u32; 3]],
    params: &Params,
) -> Orientation {
    let prepared = Prepared::new(vertices, faces);
    let directions = prepared.candidate_directions();
    let mut candidates: Vec<Candidate> = directions
        .par_iter()
        .map(|&n| prepared.score(n, params))
        .collect();
    candidates.sort_by(|a, b| {
        a.cost
            .total_cmp(&b.cost)
            .then_with(|| down_dot(b.normal).total_cmp(&down_dot(a.normal)))
    });
    // Upstream breaks exact ties towards the identity (no flipping); nearest-to-down generalises
    // that and also keeps a tilted object on the face it is already closest to.
    let best = candidates
        .iter()
        .take_while(|c| c.cost <= candidates[0].cost + EPSILON)
        .max_by(|a, b| down_dot(a.normal).total_cmp(&down_dot(b.normal)))
        .cloned()
        .unwrap_or_else(|| prepared.score(DOWN, params));
    Orientation {
        rotation: rotation_to_down(best.normal),
        best,
        candidates,
    }
}

/// Cost of one candidate "down" direction, with Orca's default parameters.
pub fn score(vertices: &[[f32; 3]], faces: &[[u32; 3]], normal: [f64; 3]) -> Candidate {
    score_with(vertices, faces, normal, &Params::default())
}

/// Cost of one candidate "down" direction with explicit parameters.
pub fn score_with(
    vertices: &[[f32; 3]],
    faces: &[[u32; 3]],
    normal: [f64; 3],
    params: &Params,
) -> Candidate {
    Prepared::new(vertices, faces).score(normal, params)
}

/// The rotation that maps `normal` onto (0, 0, −1). Handles the parallel (identity) and
/// antiparallel (half turn about X) cases; a zero normal gives the identity.
pub fn rotation_to_down(normal: [f64; 3]) -> Rotation {
    let Some(n) = normalize(normal) else {
        return Rotation {
            axis: [1.0, 0.0, 0.0],
            angle: 0.0,
        };
    };
    let c = dot(n, DOWN);
    let axis = cross(n, DOWN);
    let s = norm(axis);
    if s < 1e-12 {
        return Rotation {
            axis: [1.0, 0.0, 0.0],
            angle: if c > 0.0 { 0.0 } else { std::f64::consts::PI },
        };
    }
    Rotation {
        axis: scale(axis, 1.0 / s),
        angle: s.atan2(c),
    }
}

/// Rotate vertices about the origin (Rodrigues' formula).
pub fn apply_rotation(vertices: &[[f32; 3]], r: &Rotation) -> Vec<[f32; 3]> {
    let k = normalize(r.axis).unwrap_or([1.0, 0.0, 0.0]);
    let (s, c) = r.angle.sin_cos();
    vertices
        .iter()
        .map(|&v| {
            let v = to_f64(v);
            let kv = cross(k, v);
            let out = add(
                add(scale(v, c), scale(kv, s)),
                scale(k, dot(k, v) * (1.0 - c)),
            );
            out.map(|x| x as f32)
        })
        .collect()
}

/// Per-mesh data shared by all candidates: f64 vertices, valid faces with unit normals and
/// areas, and the convex hull as index triples into the same vertex array.
struct Prepared {
    verts: Vec<[f64; 3]>,
    faces: Vec<[u32; 3]>,
    normals: Vec<[f64; 3]>,
    areas: Vec<f64>,
    hull_faces: Vec<[u32; 3]>,
    hull_normals: Vec<[f64; 3]>,
    hull_areas: Vec<f64>,
}

impl Prepared {
    fn new(vertices: &[[f32; 3]], faces: &[[u32; 3]]) -> Self {
        let verts: Vec<[f64; 3]> = vertices.iter().map(|&v| to_f64(v)).collect();
        let mut valid = Vec::with_capacity(faces.len());
        let mut normals = Vec::with_capacity(faces.len());
        let mut areas = Vec::with_capacity(faces.len());
        for &f in faces {
            if f.iter().any(|&i| i as usize >= verts.len()) {
                continue;
            }
            let [a, b, c] = f.map(|i| verts[i as usize]);
            let (n, area) = normal_and_area(a, b, c);
            if area <= 0.0 {
                continue;
            }
            valid.push(f);
            normals.push(n);
            areas.push(area);
        }
        let hull_faces = convex_hull(&verts);
        let mut hull_normals = Vec::with_capacity(hull_faces.len());
        let mut hull_areas = Vec::with_capacity(hull_faces.len());
        for f in &hull_faces {
            let [a, b, c] = f.map(|i| verts[i as usize]);
            let (n, area) = normal_and_area(a, b, c);
            hull_normals.push(n);
            hull_areas.push(area);
        }
        Prepared {
            verts,
            faces: valid,
            normals,
            areas,
            hull_faces,
            hull_normals,
            hull_areas,
        }
    }

    /// Upstream `process`: the identity, the largest mesh-normal clusters, the largest
    /// hull-normal clusters, the fixed supplements; duplicates removed (first occurrence kept).
    fn candidate_directions(&self) -> Vec<[f64; 3]> {
        let mut dirs = vec![DOWN];
        dirs.extend(area_cumulation(&self.normals, &self.areas, MESH_CLUSTERS));
        dirs.extend(area_cumulation(
            &self.hull_normals,
            &self.hull_areas,
            HULL_CLUSTERS,
        ));
        dirs.extend(SUPPLEMENTS.iter().map(|&v| normalize(v).unwrap_or(DOWN)));
        let mut unique: Vec<[f64; 3]> = Vec::with_capacity(dirs.len());
        for d in dirs {
            let Some(d) = normalize(d) else { continue };
            if !unique.iter().any(|u| norm(sub(*u, d)) < DUPLICATE_TOL) {
                unique.push(d);
            }
        }
        unique
    }

    /// Upstream `get_features` + `target_function` for one candidate.
    fn score(&self, normal: [f64; 3], params: &Params) -> Candidate {
        let normal = normalize(normal).unwrap_or(DOWN);
        let up = scale(normal, -1.0);
        let z: Vec<f64> = self.verts.iter().map(|&v| dot(v, up)).collect();
        let (mut min_z, mut max_z) = (f64::INFINITY, f64::NEG_INFINITY);
        for &zi in &z {
            min_z = min_z.min(zi);
            max_z = max_z.max(zi);
        }
        if !min_z.is_finite() {
            return Candidate {
                normal,
                cost: 0.0,
                overhang_area: 0.0,
                bottom_area: 0.0,
                height: 0.0,
            };
        }
        let ascent = params.ascent();
        let first = min_z + params.first_lay_h - EPSILON;
        let half = min_z + params.first_lay_h / 2.0 - EPSILON;
        let (mut bottom_full, mut bottom_half) = (0.0, 0.0);
        let (mut overhang_area, mut overhang_weighted, mut laf) = (0.0, 0.0, 0.0);
        for (i, f) in self.faces.iter().enumerate() {
            let zs = f.map(|k| z[k as usize]);
            let z_max = zs[0].max(zs[1]).max(zs[2]);
            let area = self.areas[i];
            let is_bottom_half = z_max < half;
            if z_max < first {
                bottom_full += area;
            }
            if is_bottom_half {
                bottom_half += area;
            }
            let np = dot(self.normals[i], up);
            if np < ascent && !is_bottom_half {
                overhang_area += area;
                let steepness = (ascent - np).max(0.0);
                let z_mean = (zs[0] + zs[1] + zs[2]) / 3.0 - min_z;
                overhang_weighted += z_mean * area * steepness;
            }
            let np_abs = np.abs();
            if np_abs < params.laf_max
                && np_abs > params.laf_min
                && z_max > min_z + params.first_lay_h
            {
                laf += area;
            }
        }
        // Upstream: the first layer is sliced at half its height, so the contact area is the
        // half-height band plus half of the full first-layer band.
        let bottom = 0.5 * bottom_full + bottom_half;
        let contour = 4.0 * bottom.sqrt();
        let bottom_hull: f64 = self
            .hull_faces
            .iter()
            .zip(&self.hull_areas)
            .filter(|(f, _)| f.iter().all(|&k| z[k as usize] < first))
            .map(|(_, a)| a)
            .sum();
        // Upstream never fills `area_projected`; it stays zero.
        let area_projected = 0.0;
        let laf = if params.use_low_angle_face { laf } else { 0.0 };
        let mut cost = if params.min_volume {
            let overhang = overhang_weighted / 25.0;
            params.tar_a * (overhang + params.tar_b)
                + params.relative_f
                    * (overhang * params.tar_c + params.tar_d + params.tar_laf * laf)
                    / (params.tar_d
                        + params.contour_f * contour
                        + params.bottom_f * bottom
                        + params.bottom_hull_f * bottom_hull
                        + params.tar_e * overhang
                        + params.tar_proj_area * area_projected)
        } else {
            params.relative_f * (overhang_area * params.tar_c + params.tar_d + params.tar_laf * laf)
                / (params.tar_d
                    + params.contour_f * contour
                    + params.bottom_f * bottom
                    + params.bottom_hull_f * bottom_hull
                    + params.tar_proj_area * area_projected)
        };
        if bottom < params.bottom_min {
            cost += 100.0;
        }
        Candidate {
            normal,
            cost,
            overhang_area,
            bottom_area: bottom_full,
            height: max_z - min_z,
        }
    }
}

/// Upstream `add_supplements`: the axes and the 45° diagonals.
const SUPPLEMENTS: [[f64; 3]; 18] = {
    const H: f64 = std::f64::consts::FRAC_1_SQRT_2;
    [
        [0.0, 0.0, -1.0],
        [H, 0.0, -H],
        [0.0, H, -H],
        [-H, 0.0, -H],
        [0.0, -H, -H],
        [1.0, 0.0, 0.0],
        [H, H, 0.0],
        [0.0, 1.0, 0.0],
        [-H, H, 0.0],
        [-1.0, 0.0, 0.0],
        [-H, -H, 0.0],
        [0.0, -1.0, 0.0],
        [H, -H, 0.0],
        [H, 0.0, H],
        [0.0, H, H],
        [-H, 0.0, H],
        [0.0, -H, H],
        [0.0, 0.0, 1.0],
    ]
};

/// Upstream `area_cumulation_accurate`: bin normals at 1e-3, rank bins by total area and return
/// the exact normal of the largest face in each of the `count` largest bins.
fn area_cumulation(normals: &[[f64; 3]], areas: &[f64], count: usize) -> Vec<[f64; 3]> {
    struct Bin {
        total: f64,
        largest: f64,
        normal: [f64; 3],
    }
    let mut bins: HashMap<[i32; 3], Bin> = HashMap::new();
    for (&n, &a) in normals.iter().zip(areas) {
        if a <= 0.0 {
            continue;
        }
        let key = n.map(|x| (x * 1000.0).round() as i32);
        let bin = bins.entry(key).or_insert(Bin {
            total: 0.0,
            largest: -1.0,
            normal: n,
        });
        bin.total += a;
        if a > bin.largest {
            bin.largest = a;
            bin.normal = n;
        }
    }
    let mut ranked: Vec<(&[i32; 3], &Bin)> = bins.iter().collect();
    ranked.sort_by(|(ka, a), (kb, b)| b.total.total_cmp(&a.total).then_with(|| ka.cmp(kb)));
    ranked
        .into_iter()
        .take(count)
        .map(|(_, bin)| bin.normal)
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Convex hull (quickhull). Replaces upstream TriangleMesh::convex_hull_3d (qhull).

/// Triangles of the convex hull of `points`, as index triples with outward CCW winding. Empty
/// for fewer than four non-coplanar points.
pub(crate) fn convex_hull(points: &[[f64; 3]]) -> Vec<[u32; 3]> {
    struct Face {
        v: [u32; 3],
        n: [f64; 3],
        d: f64,
        outside: Vec<u32>,
        alive: bool,
    }
    fn make_face(points: &[[f64; 3]], v: [u32; 3]) -> Face {
        let [a, b, c] = v.map(|i| points[i as usize]);
        let n = normalize(cross(sub(b, a), sub(c, a))).unwrap_or([0.0; 3]);
        Face {
            v,
            n,
            d: dot(n, a),
            outside: Vec::new(),
            alive: true,
        }
    }
    let dist = |f: &Face, p: [f64; 3]| dot(f.n, p) - f.d;

    if points.len() < 4 || points.len() > u32::MAX as usize {
        return Vec::new();
    }
    let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
    for p in points {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    let extent = (0..3).map(|k| hi[k] - lo[k]).fold(0.0, f64::max);
    if !extent.is_finite() || extent <= 0.0 {
        return Vec::new();
    }
    let eps = extent * 1e-6;

    // Initial simplex: the two extremes along the widest axis, the point farthest from their
    // line, the point farthest from that plane.
    let axis = (0..3)
        .max_by(|&a, &b| (hi[a] - lo[a]).total_cmp(&(hi[b] - lo[b])))
        .unwrap_or(0);
    let argmax = |score: &dyn Fn(usize) -> f64| -> (usize, f64) {
        let mut best = (0, f64::NEG_INFINITY);
        for i in 0..points.len() {
            let s = score(i);
            if s > best.1 {
                best = (i, s);
            }
        }
        best
    };
    let i0 = argmax(&|i| -points[i][axis]).0;
    let i1 = argmax(&|i| points[i][axis]).0;
    let p0 = points[i0];
    let dir = sub(points[i1], p0);
    let (i2, line_dist) = argmax(&|i| norm(cross(sub(points[i], p0), dir)) / norm(dir));
    if line_dist <= eps {
        return Vec::new();
    }
    let base_n = normalize(cross(dir, sub(points[i2], p0))).unwrap_or([0.0; 3]);
    let (i3, plane_dist) = argmax(&|i| dot(sub(points[i], p0), base_n).abs());
    if plane_dist <= eps {
        return Vec::new();
    }
    let (i0, i1, i2, i3) = (i0 as u32, i1 as u32, i2 as u32, i3 as u32);
    let mut faces: Vec<Face> = Vec::new();
    for (tri, opposite) in [
        ([i0, i1, i2], i3),
        ([i0, i1, i3], i2),
        ([i0, i2, i3], i1),
        ([i1, i2, i3], i0),
    ] {
        let mut f = make_face(points, tri);
        if dist(&f, points[opposite as usize]) > 0.0 {
            f = make_face(points, [tri[0], tri[2], tri[1]]);
        }
        faces.push(f);
    }
    let mut edges: HashMap<(u32, u32), usize> = HashMap::new();
    let link = |edges: &mut HashMap<(u32, u32), usize>, fi: usize, v: [u32; 3]| {
        for k in 0..3 {
            edges.insert((v[k], v[(k + 1) % 3]), fi);
        }
    };
    for (fi, f) in faces.iter().enumerate() {
        link(&mut edges, fi, f.v);
    }
    for (i, &p) in points.iter().enumerate() {
        let i = i as u32;
        if [i0, i1, i2, i3].contains(&i) {
            continue;
        }
        if let Some(f) = faces.iter_mut().find(|f| dist(f, p) > eps) {
            f.outside.push(i);
        }
    }
    let mut work: Vec<usize> = (0..4).filter(|&f| !faces[f].outside.is_empty()).collect();
    // 0 = unknown, 1 = visible from the apex, 2 = not visible.
    let mut mark: Vec<u8> = vec![0; 4];
    let mut touched: Vec<usize> = Vec::new();

    while let Some(fi) = work.pop() {
        if !faces[fi].alive || faces[fi].outside.is_empty() {
            continue;
        }
        let apex_i = *faces[fi]
            .outside
            .iter()
            .max_by(|&&a, &&b| {
                dist(&faces[fi], points[a as usize])
                    .total_cmp(&dist(&faces[fi], points[b as usize]))
            })
            .unwrap_or(&faces[fi].outside[0]);
        let apex = points[apex_i as usize];

        // Visible region by flood fill over shared edges; its boundary is the horizon.
        let mut visible = vec![fi];
        let mut horizon: Vec<(u32, u32)> = Vec::new();
        mark[fi] = 1;
        touched.push(fi);
        let mut cursor = 0;
        while cursor < visible.len() {
            let f = visible[cursor];
            cursor += 1;
            let v = faces[f].v;
            for k in 0..3 {
                let (a, b) = (v[k], v[(k + 1) % 3]);
                let Some(&g) = edges.get(&(b, a)) else {
                    horizon.push((a, b));
                    continue;
                };
                if mark[g] == 0 {
                    mark[g] = if dist(&faces[g], apex) > eps { 1 } else { 2 };
                    touched.push(g);
                    if mark[g] == 1 {
                        visible.push(g);
                    }
                }
                if mark[g] == 2 {
                    horizon.push((a, b));
                }
            }
        }
        let mut orphans: Vec<u32> = Vec::new();
        for &f in &visible {
            faces[f].alive = false;
            let v = faces[f].v;
            for k in 0..3 {
                edges.remove(&(v[k], v[(k + 1) % 3]));
            }
            orphans.extend(
                std::mem::take(&mut faces[f].outside)
                    .into_iter()
                    .filter(|&i| i != apex_i),
            );
        }
        let first_new = faces.len();
        for &(a, b) in &horizon {
            let fi = faces.len();
            let f = make_face(points, [a, b, apex_i]);
            link(&mut edges, fi, f.v);
            faces.push(f);
            mark.push(0);
        }
        for i in orphans {
            let p = points[i as usize];
            if let Some(f) = faces[first_new..].iter_mut().find(|f| dist(f, p) > eps) {
                f.outside.push(i);
            }
        }
        work.extend((first_new..faces.len()).filter(|&f| !faces[f].outside.is_empty()));
        for f in touched.drain(..) {
            mark[f] = 0;
        }
    }
    faces.iter().filter(|f| f.alive).map(|f| f.v).collect()
}

// ---------------------------------------------------------------------------------------------
// Small vector helpers.

fn to_f64(v: [f32; 3]) -> [f64; 3] {
    v.map(f64::from)
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
fn norm(a: [f64; 3]) -> f64 {
    dot(a, a).sqrt()
}
fn normalize(a: [f64; 3]) -> Option<[f64; 3]> {
    let n = norm(a);
    (n > 1e-300 && n.is_finite()).then(|| scale(a, 1.0 / n))
}
fn down_dot(n: [f64; 3]) -> f64 {
    dot(n, DOWN)
}
/// Unit normal and area of a CCW triangle; zero normal and area if degenerate.
fn normal_and_area(a: [f64; 3], b: [f64; 3], c: [f64; 3]) -> ([f64; 3], f64) {
    let n = cross(sub(b, a), sub(c, a));
    let len = norm(n);
    match normalize(n) {
        Some(unit) => (unit, 0.5 * len),
        None => ([0.0; 3], 0.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::TriangleMesh;
    use crate::mesh::tests::box_mesh;
    use std::f64::consts::{FRAC_PI_2, PI};

    fn close(a: [f64; 3], b: [f64; 3], tol: f64) -> bool {
        norm(sub(a, b)) < tol
    }

    /// Closed mesh from loose triangles, wound outward (flipped if the signed volume is negative).
    fn closed(tris: &[[[f32; 3]; 3]]) -> TriangleMesh {
        let mut m = TriangleMesh::from_triangles(tris);
        if m.volume() < 0.0 {
            for f in &mut m.faces {
                f.swap(1, 2);
            }
        }
        assert!(m.volume() > 0.0);
        m
    }

    /// Prism along Y between `y0` and `y1`: `outline` is the (x, z) boundary in CCW order, `caps`
    /// a triangulation of its interior.
    fn prism(outline: &[[f32; 2]], caps: &[[[f32; 2]; 3]], y0: f32, y1: f32) -> TriangleMesh {
        let at = |p: [f32; 2], y: f32| [p[0], y, p[1]];
        let mut tris = Vec::new();
        for i in 0..outline.len() {
            let (p, q) = (outline[i], outline[(i + 1) % outline.len()]);
            tris.push([at(p, y0), at(q, y0), at(q, y1)]);
            tris.push([at(p, y0), at(q, y1), at(p, y1)]);
        }
        for &[a, b, c] in caps {
            tris.push([at(a, y0), at(c, y0), at(b, y0)]);
            tris.push([at(a, y1), at(b, y1), at(c, y1)]);
        }
        closed(&tris)
    }

    fn pyramid(half: f32, height: f32) -> TriangleMesh {
        let (a, b, c, d) = (
            [-half, -half, 0.],
            [half, -half, 0.],
            [half, half, 0.],
            [-half, half, 0.],
        );
        let t = [0., 0., height];
        closed(&[
            [a, c, b],
            [a, d, c],
            [a, b, t],
            [b, c, t],
            [c, d, t],
            [d, a, t],
        ])
    }

    /// Unit normal of the mesh face with the lowest centroid.
    fn lowest_face_normal(vertices: &[[f32; 3]], faces: &[[u32; 3]]) -> [f64; 3] {
        let f = faces
            .iter()
            .min_by(|a, b| {
                let zc = |f: &[u32; 3]| f.iter().map(|&i| vertices[i as usize][2]).sum::<f32>();
                zc(a).total_cmp(&zc(b))
            })
            .unwrap();
        let [a, b, c] = f.map(|i| to_f64(vertices[i as usize]));
        normal_and_area(a, b, c).0
    }

    fn rotated_about_x(m: &TriangleMesh, angle: f64) -> Vec<[f32; 3]> {
        apply_rotation(
            &m.vertices,
            &Rotation {
                axis: [1., 0., 0.],
                angle,
            },
        )
    }

    #[test]
    fn flat_box_stays_flat() {
        let m = box_mesh([0.; 3], [20., 20., 20.]);
        let o = best_orientation(&m.vertices, &m.faces);
        let down = score(&m.vertices, &m.faces, DOWN);
        assert!((down.bottom_area - 400.0).abs() < 1e-6, "{down:?}");
        assert!(down.overhang_area.abs() < 1e-9);
        assert!((down.height - 20.0).abs() < 1e-6);
        assert!(o.candidates.iter().all(|c| c.cost >= down.cost - 1e-12));
        // Identity, or an equivalent quarter turn; here the tie-break keeps the identity.
        assert!(o.rotation.angle.abs() < 1e-9, "{:?}", o.rotation);
        assert!(close(o.best.normal, DOWN, 1e-9));
        let after = apply_rotation(&m.vertices, &o.rotation);
        assert!(close(lowest_face_normal(&after, &m.faces), DOWN, 1e-3));
    }

    #[test]
    fn tilted_box_comes_back_flat() {
        let m = box_mesh([0.; 3], [20., 20., 20.]);
        let tilted = rotated_about_x(&m, 30f64.to_radians());
        let o = best_orientation(&tilted, &m.faces);
        let after = apply_rotation(&tilted, &o.rotation);
        assert!(close(lowest_face_normal(&after, &m.faces), DOWN, 1e-3));
        let min_z = after
            .iter()
            .map(|v| v[2] as f64)
            .fold(f64::INFINITY, f64::min);
        for v in &after {
            let z = v[2] as f64 - min_z;
            assert!(z.abs() < 1e-3 || (z - 20.0).abs() < 1e-3, "z={z}");
        }
        // Nearest face wins among equal costs: a 30° correction about X, not a 60° or 90° one.
        assert!(
            (o.rotation.angle - 30f64.to_radians()).abs() < 1e-3,
            "{:?}",
            o.rotation
        );
        assert!(
            close(o.rotation.axis, [1., 0., 0.], 1e-3)
                || close(o.rotation.axis, [-1., 0., 0.], 1e-3)
        );
    }

    #[test]
    fn wedge_lies_on_its_largest_face_not_the_slope() {
        // Isosceles wedge: base 40 wide, 10 tall, 20 deep; the base is the largest face.
        let outline = [[0., 0.], [40., 0.], [20., 10.]];
        let m = prism(&outline, &[outline], 0., 20.);
        let o = best_orientation(&m.vertices, &m.faces);
        assert!(close(o.best.normal, DOWN, 1e-6), "{:?}", o.best);
        assert!(o.best.overhang_area.abs() < 1e-9);
        assert!((o.best.bottom_area - 800.0).abs() < 1e-3);
        let slope = normalize([-10., 0., 20.]).unwrap();
        let slanted = score(&m.vertices, &m.faces, slope);
        assert!(
            slanted.cost > o.best.cost * 1.5,
            "{slanted:?} vs {:?}",
            o.best
        );
        assert!(
            o.candidates
                .iter()
                .all(|c| !close(c.normal, slope, 1e-3) || c.cost > o.best.cost)
        );

        // Right-triangle wedge: resting on the base or on the (larger) hypotenuse both leave no
        // overhang and both are fine; the small back face and the triangular ends must lose.
        let outline = [[0., 0.], [30., 0.], [0., 10.]];
        let m = prism(&outline, &[outline], 0., 20.);
        let o = best_orientation(&m.vertices, &m.faces);
        let hyp = normalize([10., 0., 30.]).unwrap();
        assert!(
            close(o.best.normal, DOWN, 1e-6) || close(o.best.normal, hyp, 1e-6),
            "{:?}",
            o.best
        );
        assert!(o.best.overhang_area.abs() < 1e-9);
        assert!(o.best.bottom_area >= 600.0 - 1e-3);
        let back = score(&m.vertices, &m.faces, [-1., 0., 0.]);
        let end = score(&m.vertices, &m.faces, [0., -1., 0.]);
        assert!(back.cost > o.best.cost && end.cost > o.best.cost);
    }

    #[test]
    fn pyramid_prefers_base_over_apex() {
        let m = pyramid(5., 50.);
        let o = best_orientation(&m.vertices, &m.faces);
        let base = score(&m.vertices, &m.faces, DOWN);
        let apex = score(&m.vertices, &m.faces, [0., 0., 1.]);
        assert!(apex.cost > base.cost + 50.0, "{apex:?} vs {base:?}");
        assert!(apex.bottom_area < 1e-6);
        assert!(!close(o.best.normal, [0., 0., 1.], 1e-3), "{:?}", o.best);
        // A squat pyramid rests on its base outright.
        let m = pyramid(10., 30.);
        let o = best_orientation(&m.vertices, &m.faces);
        assert!(close(o.best.normal, DOWN, 1e-6), "{:?}", o.best);
    }

    #[test]
    fn t_shape_avoids_the_overhang() {
        // Upright T: stem 10 wide under a 40 wide bar, both 10 tall; 10 deep along Y.
        let outline = [
            [15., 0.],
            [25., 0.],
            [25., 10.],
            [40., 10.],
            [40., 20.],
            [0., 20.],
            [0., 10.],
            [15., 10.],
        ];
        let caps = [
            [[15., 0.], [25., 0.], [25., 10.]],
            [[15., 0.], [25., 10.], [15., 10.]],
            [[0., 10.], [40., 10.], [40., 20.]],
            [[0., 10.], [40., 20.], [0., 20.]],
        ];
        let m = prism(&outline, &caps, 0., 10.);
        let upright = score(&m.vertices, &m.faces, DOWN);
        assert!((upright.overhang_area - 300.0).abs() < 1e-3, "{upright:?}");
        assert!((upright.bottom_area - 100.0).abs() < 1e-3);
        let flipped = score(&m.vertices, &m.faces, [0., 0., 1.]);
        assert!(flipped.overhang_area.abs() < 1e-9);
        assert!(
            flipped.cost < upright.cost / 10.0,
            "{flipped:?} vs {upright:?}"
        );
        let o = best_orientation(&m.vertices, &m.faces);
        assert!(o.best.overhang_area.abs() < 1e-9, "{:?}", o.best);
        assert!(o.best.cost <= flipped.cost + 1e-12);
        assert!(!close(o.best.normal, DOWN, 1e-3));
    }

    #[test]
    fn rotation_to_down_maps_normals_onto_minus_z() {
        for n in [
            [0., 0., 1.],
            [0., 0., -1.],
            [1., 0., 0.],
            [0., -1., 0.],
            [1., 1., 1.],
            [-0.3, 0.2, 0.9],
            [0.001, 0., -1.],
            [2., -5., 0.5],
        ] {
            let r = rotation_to_down(n);
            assert!((norm(r.axis) - 1.0).abs() < 1e-12);
            assert!((0.0..=PI).contains(&r.angle));
            let unit = normalize(n).unwrap().map(|x| x as f32);
            let mapped = to_f64(apply_rotation(&[unit], &r)[0]);
            assert!(close(mapped, DOWN, 1e-5), "{n:?} -> {mapped:?} via {r:?}");
        }
        assert_eq!(rotation_to_down([0., 0., -1.]).angle, 0.0);
        assert!((rotation_to_down([0., 0., 1.]).angle - PI).abs() < 1e-12);
        assert!((rotation_to_down([1., 0., 0.]).angle - FRAC_PI_2).abs() < 1e-12);
        assert_eq!(rotation_to_down([0., 0., 0.]).angle, 0.0);
    }

    #[test]
    fn scoring_is_deterministic_and_sorted() {
        let outline = [[0., 0.], [30., 0.], [0., 10.]];
        let m = prism(&outline, &[outline], 0., 20.);
        let a = best_orientation(&m.vertices, &m.faces);
        let b = best_orientation(&m.vertices, &m.faces);
        assert_eq!(a.candidates, b.candidates);
        assert_eq!(a.best, b.best);
        assert!(a.candidates.windows(2).all(|w| w[0].cost <= w[1].cost));
        assert!(a.candidates.len() >= 18);
        for c in &a.candidates {
            assert!((norm(c.normal) - 1.0).abs() < 1e-9);
            assert!(c.cost.is_finite());
        }
        for w in a.candidates.windows(2) {
            assert!(!close(w[0].normal, w[1].normal, DUPLICATE_TOL));
        }
    }

    #[test]
    fn min_volume_params_agree_on_the_easy_cases() {
        let p = Params::min_volume();
        let m = box_mesh([0.; 3], [20., 20., 20.]);
        let o = best_orientation_with(&m.vertices, &m.faces, &p);
        assert!(o.rotation.angle.abs() < 1e-9);
        let outline = [[0., 0.], [40., 0.], [20., 10.]];
        let m = prism(&outline, &[outline], 0., 20.);
        let o = best_orientation_with(&m.vertices, &m.faces, &p);
        assert!(close(o.best.normal, DOWN, 1e-6), "{:?}", o.best);
        assert!(o.candidates.windows(2).all(|w| w[0].cost <= w[1].cost));
    }

    #[test]
    fn convex_hull_of_a_box_with_interior_points() {
        let mut m = box_mesh([0.; 3], [20., 20., 20.]);
        m.vertices.push([10., 10., 10.]);
        m.vertices.push([5., 7., 1.]);
        let pts: Vec<[f64; 3]> = m.vertices.iter().map(|&v| to_f64(v)).collect();
        let hull = convex_hull(&pts);
        assert_eq!(hull.len(), 12);
        let area: f64 = hull
            .iter()
            .map(|f| {
                let [a, b, c] = f.map(|i| pts[i as usize]);
                normal_and_area(a, b, c).1
            })
            .sum();
        assert!((area - 2400.0).abs() < 1e-6);
        // Outward: every face sees the centre on its inside.
        for f in &hull {
            let [a, b, c] = f.map(|i| pts[i as usize]);
            let (n, _) = normal_and_area(a, b, c);
            assert!(dot(n, sub([10., 10., 10.], a)) < 0.0);
        }
        assert!(hull.iter().all(|f| f.iter().all(|&i| i < 8)));
        assert!(convex_hull(&pts[..3]).is_empty());
        assert!(convex_hull(&[[0., 0., 0.], [1., 0., 0.], [0., 1., 0.], [1., 1., 0.]]).is_empty());
    }

    #[test]
    fn legs_rest_on_the_hull_bottom() {
        // A table: a 40×40×5 top on four 4×4×20 legs. No single large face at the leg tips, but
        // the convex hull's bottom spans them, so "legs down" beats "top down" only if the hull
        // term is in play; either way the result must not be a side or a diagonal.
        let mut tris = Vec::new();
        let push_box = |tris: &mut Vec<[[f32; 3]; 3]>, lo: [f32; 3], hi: [f32; 3]| {
            let b = box_mesh(lo, hi);
            for f in &b.faces {
                tris.push(f.map(|i| b.vertices[i as usize]));
            }
        };
        push_box(&mut tris, [0., 0., 20.], [40., 40., 25.]);
        for (x, y) in [(0., 0.), (36., 0.), (0., 36.), (36., 36.)] {
            push_box(&mut tris, [x, y, 0.], [x + 4., y + 4., 20.]);
        }
        let m = TriangleMesh::from_triangles(&tris);
        let legs_down = score(&m.vertices, &m.faces, DOWN);
        assert!((legs_down.bottom_area - 64.0).abs() < 1e-3, "{legs_down:?}");
        let o = best_orientation(&m.vertices, &m.faces);
        assert!(
            close(o.best.normal, DOWN, 1e-6) || close(o.best.normal, [0., 0., 1.], 1e-6),
            "{:?}",
            o.best
        );
    }

    #[test]
    fn large_mesh_is_fast_enough() {
        // ~100k triangles: a sphere-ish blob; must finish well within the test budget even in debug.
        let n = 224usize;
        let mut tris = Vec::with_capacity(2 * n * n);
        let p = |i: usize, j: usize| {
            let th = PI * i as f64 / n as f64;
            let ph = 2.0 * PI * j as f64 / n as f64;
            [
                (30.0 * th.sin() * ph.cos()) as f32,
                (20.0 * th.sin() * ph.sin()) as f32,
                (25.0 * th.cos()) as f32,
            ]
        };
        for i in 0..n {
            for j in 0..n {
                let (a, b, c, d) = (p(i, j), p(i + 1, j), p(i + 1, j + 1), p(i, j + 1));
                tris.push([a, b, c]);
                tris.push([a, c, d]);
            }
        }
        let m = TriangleMesh::from_triangles(&tris);
        assert!(m.faces.len() >= 100_000);
        let start = std::time::Instant::now();
        let o = best_orientation(&m.vertices, &m.faces);
        let elapsed = start.elapsed();
        assert!(o.best.cost.is_finite());
        assert!(elapsed.as_secs() < 20, "{elapsed:?}");
    }
}
