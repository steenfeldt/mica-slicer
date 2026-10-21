// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/TriangleMeshSlicer.cpp :: cut_mesh, triangulate_expolygons_3d
// Changes: section_cap returns only the lower cap (no welding) for the GUI section view. Arbitrary plane (point + normal) instead of a horizontal z; vertices classified by signed distance with a 1e-5 mm on-plane band and crossing triangles clipped into upper/lower polygons; split points shared per edge through a sorted-pair map instead of upstream's edge ids and coord_t snapping; own loop stitching, contour/hole nesting and ear-clip cap triangulation with hole bridging (upstream uses Clipper + GLU tesselation); f32 indexed mesh; coincident vertices welded first.
// Upstream attribution (TriangleMeshSlicer.cpp):
// Copyright (c) Prusa Research 2021 - 2023 Vojtěch Bubník @bubnikv, Lukáš Matěna @lukasmatena, Pavel Mikuš @Godrak, Lukáš Hejl @hejllukas
//
// PrusaSlicer is released under the terms of the AGPLv3 or higher
//

//! Cutting a triangle mesh with an arbitrary plane, optionally capping both halves.

use std::collections::HashMap;

use crate::mesh::TriangleMesh;

/// Distance (mm) within which a vertex counts as lying on the cut plane.
const ON_PLANE: f64 = 1e-5;
/// Vertices closer than this (mm) are merged before cutting and before checking watertightness.
pub(crate) const WELD_TOLERANCE: f32 = 1e-5;

/// A cutting plane. The "upper" half is the side the normal points to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CutPlane {
    pub point: [f32; 3],
    /// Need not be unit length.
    pub normal: [f32; 3],
}

/// The two halves of a cut; a side is `None` when nothing of the mesh lies there.
#[derive(Clone, Debug, Default)]
pub struct CutResult {
    pub upper: Option<TriangleMesh>,
    pub lower: Option<TriangleMesh>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Side {
    Above,
    On,
    Below,
}

/// Cuts `mesh` with `plane`. With `cap`, the open cross-sections are triangulated (outer contours
/// with their holes) so that watertight input yields two watertight halves; the cap of the upper
/// half faces `-normal`, that of the lower half `+normal`. Triangles lying exactly in the plane are
/// re-created by the caps when capping, and otherwise assigned to the half they face away from.
/// A plane that misses the mesh returns the whole mesh (cloned) on one side.
pub fn cut_mesh(mesh: &TriangleMesh, plane: &CutPlane, cap: bool) -> CutResult {
    if mesh.faces.is_empty() {
        return CutResult::default();
    }
    let Some(normal) = normalize(plane.normal.map(f64::from)) else {
        return CutResult {
            upper: Some(mesh.clone()),
            lower: None,
        };
    };
    let origin = plane.point.map(f64::from);
    let welded = weld_vertices(mesh, WELD_TOLERANCE);
    let dist: Vec<f64> = welded
        .vertices
        .iter()
        .map(|v| dot(sub(v.map(f64::from), origin), normal))
        .collect();
    let side: Vec<Side> = dist
        .iter()
        .map(|&d| {
            if d > ON_PLANE {
                Side::Above
            } else if d < -ON_PLANE {
                Side::Below
            } else {
                Side::On
            }
        })
        .collect();
    let mut used_sides = welded.faces.iter().flatten().map(|&i| side[i as usize]);
    let has_above = used_sides.clone().any(|s| s == Side::Above);
    let has_below = used_sides.any(|s| s == Side::Below);
    if !has_below {
        return CutResult {
            upper: Some(mesh.clone()),
            lower: None,
        };
    }
    if !has_above {
        return CutResult {
            upper: None,
            lower: Some(mesh.clone()),
        };
    }

    let mut verts = welded.vertices.clone();
    let mut splits: HashMap<(u32, u32), u32> = HashMap::new();
    let mut upper_faces: Vec<[u32; 3]> = Vec::new();
    let mut lower_faces: Vec<[u32; 3]> = Vec::new();
    // Directed in-plane edges of each half, already reversed so the cap pairs with the half.
    let mut upper_segs: Vec<[u32; 2]> = Vec::new();
    let mut lower_segs: Vec<[u32; 2]> = Vec::new();

    for face in &welded.faces {
        let cls = face.map(|i| side[i as usize]);
        let n_above = cls.iter().filter(|&&c| c == Side::Above).count();
        let n_below = cls.iter().filter(|&&c| c == Side::Below).count();
        if n_above == 0 && n_below == 0 {
            // Coplanar: dropped when capping (the cap re-creates it), else kept by facing.
            if !cap {
                let [a, b, c] = face.map(|i| verts[i as usize].map(f64::from));
                let fn_ = cross(sub(b, a), sub(c, a));
                if dot(fn_, normal) < 0.0 {
                    upper_faces.push(*face);
                } else {
                    lower_faces.push(*face);
                }
            }
            continue;
        }
        if n_below == 0 {
            upper_faces.push(*face);
            push_on_edge(face, &cls, &mut upper_segs);
            continue;
        }
        if n_above == 0 {
            lower_faces.push(*face);
            push_on_edge(face, &cls, &mut lower_segs);
            continue;
        }
        // Crossing triangle: clip into an upper and a lower polygon (3 or 4 vertices each).
        let mut up: Vec<(u32, bool)> = Vec::with_capacity(4);
        let mut low: Vec<(u32, bool)> = Vec::with_capacity(4);
        for k in 0..3 {
            let a = face[k];
            let b = face[(k + 1) % 3];
            match cls[k] {
                Side::Above => up.push((a, false)),
                Side::Below => low.push((a, false)),
                Side::On => {
                    up.push((a, true));
                    low.push((a, true));
                }
            }
            let crossing = matches!(
                (cls[k], cls[(k + 1) % 3]),
                (Side::Above, Side::Below) | (Side::Below, Side::Above)
            );
            if crossing {
                let s = split_point(a, b, &mut verts, &dist, &mut splits);
                up.push((s, true));
                low.push((s, true));
            }
        }
        emit_polygon(&up, &mut upper_faces, &mut upper_segs);
        emit_polygon(&low, &mut lower_faces, &mut lower_segs);
    }

    if cap {
        let (u, v) = plane_basis(normal);
        // Upper cap faces -normal: use a basis whose cross product is -normal.
        upper_faces.extend(cap_loops(&stitch_loops(&upper_segs), &verts, origin, v, u));
        lower_faces.extend(cap_loops(&stitch_loops(&lower_segs), &verts, origin, u, v));
    }

    CutResult {
        upper: (!upper_faces.is_empty()).then(|| compact_faces(&verts, &upper_faces)),
        lower: (!lower_faces.is_empty()).then(|| compact_faces(&verts, &lower_faces)),
    }
}

/// The filled cross-section of `mesh` in `plane`: the cap `cut_mesh` would put on the lower half,
/// facing `+normal`, on its own (the GUI's section view fills cut faces with it). Vertices are
/// not welded first, so the loops close through shared vertex indices (every Mica loader merges
/// bit-identical vertices); a plane that misses the mesh gives an empty mesh.
pub fn section_cap(mesh: &TriangleMesh, plane: &CutPlane) -> TriangleMesh {
    let empty = TriangleMesh {
        vertices: vec![],
        faces: vec![],
    };
    let Some(normal) = normalize(plane.normal.map(f64::from)) else {
        return empty;
    };
    let origin = plane.point.map(f64::from);
    let dist: Vec<f64> = mesh
        .vertices
        .iter()
        .map(|v| dot(sub(v.map(f64::from), origin), normal))
        .collect();
    let side = |i: u32| {
        let d = dist[i as usize];
        if d > ON_PLANE {
            Side::Above
        } else if d < -ON_PLANE {
            Side::Below
        } else {
            Side::On
        }
    };
    // Cap vertices: on-plane mesh vertices (key (i, i)) and edge crossings (key (lo, hi)).
    let mut verts: Vec<[f32; 3]> = Vec::new();
    let mut ids: HashMap<(u32, u32), u32> = HashMap::new();
    let mut point = |a: u32, b: u32| -> u32 {
        let key = (a.min(b), a.max(b));
        *ids.entry(key).or_insert_with(|| {
            let p = mesh.vertices[key.0 as usize].map(f64::from);
            let s = if key.0 == key.1 {
                p
            } else {
                let q = mesh.vertices[key.1 as usize].map(f64::from);
                let (dp, dq) = (dist[key.0 as usize], dist[key.1 as usize]);
                let t = dp / (dp - dq);
                [
                    p[0] + t * (q[0] - p[0]),
                    p[1] + t * (q[1] - p[1]),
                    p[2] + t * (q[2] - p[2]),
                ]
            };
            verts.push(s.map(|c| c as f32));
            (verts.len() - 1) as u32
        })
    };
    let mut segs: Vec<[u32; 2]> = Vec::new();
    for face in &mesh.faces {
        let cls = face.map(side);
        let n_above = cls.iter().filter(|&&c| c == Side::Above).count();
        let n_below = cls.iter().filter(|&&c| c == Side::Below).count();
        if n_below == 0 {
            // Above or coplanar: not part of the lower half's border.
            continue;
        }
        if n_above == 0 {
            for k in 0..3 {
                let j = (k + 1) % 3;
                if cls[k] == Side::On && cls[j] == Side::On {
                    segs.push([point(face[j], face[j]), point(face[k], face[k])]);
                }
            }
            continue;
        }
        // Crossing: the lower polygon's in-plane edge, reversed (as `emit_polygon`).
        let mut low: Vec<(u32, bool)> = Vec::with_capacity(4);
        for k in 0..3 {
            let (a, b) = (face[k], face[(k + 1) % 3]);
            match cls[k] {
                Side::Below => low.push((u32::MAX, false)),
                Side::On => low.push((point(a, a), true)),
                Side::Above => {}
            }
            if matches!(
                (cls[k], cls[(k + 1) % 3]),
                (Side::Above, Side::Below) | (Side::Below, Side::Above)
            ) {
                low.push((point(a, b), true));
            }
        }
        for k in 0..low.len() {
            let (p, p_on) = low[k];
            let (q, q_on) = low[(k + 1) % low.len()];
            if p_on && q_on && p != q {
                segs.push([q, p]);
            }
        }
    }
    if segs.is_empty() {
        return empty;
    }
    let (u, v) = plane_basis(normal);
    let faces = cap_loops(&stitch_loops(&segs), &verts, origin, u, v);
    TriangleMesh {
        vertices: verts,
        faces,
    }
}

/// True when every edge is shared by exactly two faces running in opposite directions, after
/// merging vertices closer than [`WELD_TOLERANCE`].
pub fn is_watertight(mesh: &TriangleMesh) -> bool {
    if mesh.faces.is_empty() {
        return false;
    }
    let welded = weld_vertices(mesh, WELD_TOLERANCE);
    let mut edges: HashMap<(u32, u32), [u32; 2]> = HashMap::with_capacity(welded.faces.len() * 3);
    for face in &welded.faces {
        for k in 0..3 {
            let a = face[k];
            let b = face[(k + 1) % 3];
            let counts = edges.entry((a.min(b), a.max(b))).or_default();
            counts[usize::from(a > b)] += 1;
        }
    }
    edges.values().all(|&c| c == [1, 1])
}

/// Merges vertices closer than `tol` and drops faces that collapse. Positions of the surviving
/// (first-seen) vertices are kept exactly.
pub(crate) fn weld_vertices(mesh: &TriangleMesh, tol: f32) -> TriangleMesh {
    let tol = f64::from(tol);
    let inv = 1.0 / tol;
    let tol2 = tol * tol;
    let mut cells: HashMap<[i64; 3], Vec<u32>> = HashMap::with_capacity(mesh.vertices.len());
    let mut vertices: Vec<[f32; 3]> = Vec::with_capacity(mesh.vertices.len());
    let mut remap: Vec<u32> = Vec::with_capacity(mesh.vertices.len());
    for v in &mesh.vertices {
        let p = v.map(f64::from);
        let cell = p.map(|c| (c * inv).floor() as i64);
        let mut found = None;
        'search: for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    let key = [cell[0] + dx, cell[1] + dy, cell[2] + dz];
                    let Some(list) = cells.get(&key) else {
                        continue;
                    };
                    for &j in list {
                        let q = vertices[j as usize].map(f64::from);
                        let d = sub(p, q);
                        if dot(d, d) <= tol2 {
                            found = Some(j);
                            break 'search;
                        }
                    }
                }
            }
        }
        let idx = found.unwrap_or_else(|| {
            let j = vertices.len() as u32;
            vertices.push(*v);
            cells.entry(cell).or_default().push(j);
            j
        });
        remap.push(idx);
    }
    let faces = mesh
        .faces
        .iter()
        .map(|f| f.map(|i| remap[i as usize]))
        .filter(|f| f[0] != f[1] && f[1] != f[2] && f[0] != f[2])
        .collect();
    TriangleMesh { vertices, faces }
}

/// Builds a mesh from a subset of faces, keeping only the vertices they reference (in order of
/// first use).
pub(crate) fn compact_faces(vertices: &[[f32; 3]], faces: &[[u32; 3]]) -> TriangleMesh {
    let mut remap: HashMap<u32, u32> = HashMap::with_capacity(faces.len() * 3 / 2);
    let mut out = TriangleMesh {
        vertices: Vec::with_capacity(faces.len() * 3 / 2),
        faces: Vec::with_capacity(faces.len()),
    };
    for face in faces {
        let f = face.map(|i| {
            *remap.entry(i).or_insert_with(|| {
                out.vertices.push(vertices[i as usize]);
                (out.vertices.len() - 1) as u32
            })
        });
        out.faces.push(f);
    }
    out
}

/// For a face entirely on one side with an edge in the plane, records that edge reversed as a cap
/// segment.
fn push_on_edge(face: &[u32; 3], cls: &[Side; 3], segs: &mut Vec<[u32; 2]>) {
    for k in 0..3 {
        let j = (k + 1) % 3;
        if cls[k] == Side::On && cls[j] == Side::On {
            segs.push([face[j], face[k]]);
        }
    }
}

/// Fan-triangulates a clipped polygon and records its in-plane edge (reversed) as a cap segment.
fn emit_polygon(poly: &[(u32, bool)], faces: &mut Vec<[u32; 3]>, segs: &mut Vec<[u32; 2]>) {
    if poly.len() < 3 {
        return;
    }
    for i in 1..poly.len() - 1 {
        let f = [poly[0].0, poly[i].0, poly[i + 1].0];
        if f[0] != f[1] && f[1] != f[2] && f[0] != f[2] {
            faces.push(f);
        }
    }
    for k in 0..poly.len() {
        let (p, p_on) = poly[k];
        let (q, q_on) = poly[(k + 1) % poly.len()];
        if p_on && q_on && p != q {
            segs.push([q, p]);
        }
    }
}

/// Index of the point where edge `a`-`b` crosses the plane, created once per undirected edge.
fn split_point(
    a: u32,
    b: u32,
    verts: &mut Vec<[f32; 3]>,
    dist: &[f64],
    splits: &mut HashMap<(u32, u32), u32>,
) -> u32 {
    let (lo, hi) = (a.min(b), a.max(b));
    *splits.entry((lo, hi)).or_insert_with(|| {
        let p = verts[lo as usize].map(f64::from);
        let q = verts[hi as usize].map(f64::from);
        let (dp, dq) = (dist[lo as usize], dist[hi as usize]);
        let t = dp / (dp - dq);
        let s = [
            p[0] + t * (q[0] - p[0]),
            p[1] + t * (q[1] - p[1]),
            p[2] + t * (q[2] - p[2]),
        ];
        verts.push(s.map(|c| c as f32));
        (verts.len() - 1) as u32
    })
}

/// Chains directed segments into closed loops by exact index matching. Open chains are dropped.
fn stitch_loops(segs: &[[u32; 2]]) -> Vec<Vec<u32>> {
    let mut outgoing: HashMap<u32, Vec<usize>> = HashMap::with_capacity(segs.len());
    for (i, s) in segs.iter().enumerate() {
        outgoing.entry(s[0]).or_default().push(i);
    }
    let mut used = vec![false; segs.len()];
    let mut loops = Vec::new();
    for start in 0..segs.len() {
        if used[start] {
            continue;
        }
        used[start] = true;
        let first = segs[start][0];
        let mut cur = segs[start][1];
        let mut chain = vec![first];
        let mut closed = false;
        while chain.len() <= segs.len() {
            if cur == first {
                closed = true;
                break;
            }
            chain.push(cur);
            let next = outgoing
                .get(&cur)
                .and_then(|list| list.iter().copied().find(|&i| !used[i]));
            match next {
                Some(i) => {
                    used[i] = true;
                    cur = segs[i][1];
                }
                None => break,
            }
        }
        if closed && chain.len() >= 3 {
            loops.push(chain);
        }
    }
    loops
}

#[derive(Clone, Copy, Debug)]
struct Pt {
    idx: u32,
    x: f64,
    y: f64,
}

/// Triangulates closed loops lying in the plane through `origin` spanned by `u`, `v`. Output
/// triangles are counter-clockwise in the (`u`, `v`) basis, i.e. their normal is `u × v`.
fn cap_loops(
    loops: &[Vec<u32>],
    verts: &[[f32; 3]],
    origin: [f64; 3],
    u: [f64; 3],
    v: [f64; 3],
) -> Vec<[u32; 3]> {
    let polys: Vec<Vec<Pt>> = loops
        .iter()
        .map(|l| {
            l.iter()
                .map(|&i| {
                    let d = sub(verts[i as usize].map(f64::from), origin);
                    Pt {
                        idx: i,
                        x: dot(d, u),
                        y: dot(d, v),
                    }
                })
                .collect()
        })
        .collect();
    let areas: Vec<f64> = polys.iter().map(|p| signed_area(p)).collect();

    // Nest: depth = number of other loops containing a point of this loop; a hole belongs to the
    // smallest loop containing it.
    let mut parent: Vec<Option<usize>> = vec![None; polys.len()];
    let mut depth: Vec<usize> = vec![0; polys.len()];
    for i in 0..polys.len() {
        let p = &polys[i];
        let (tx, ty) = ((p[0].x + p[1].x) / 2.0, (p[0].y + p[1].y) / 2.0);
        let mut best: Option<usize> = None;
        for j in 0..polys.len() {
            if i != j && point_in_polygon(&polys[j], tx, ty) {
                depth[i] += 1;
                if best.is_none_or(|b| areas[j].abs() < areas[b].abs()) {
                    best = Some(j);
                }
            }
        }
        parent[i] = best;
    }

    let mut tris = Vec::new();
    for i in 0..polys.len() {
        if !depth[i].is_multiple_of(2) {
            continue;
        }
        let holes: Vec<Vec<Pt>> = (0..polys.len())
            .filter(|&h| !depth[h].is_multiple_of(2) && parent[h] == Some(i))
            .map(|h| polys[h].clone())
            .collect();
        tris.extend(triangulate_with_holes(polys[i].clone(), holes));
    }
    tris
}

fn signed_area(p: &[Pt]) -> f64 {
    let n = p.len();
    (0..n)
        .map(|i| {
            let (a, b) = (p[i], p[(i + 1) % n]);
            a.x * b.y - b.x * a.y
        })
        .sum::<f64>()
        / 2.0
}

fn point_in_polygon(p: &[Pt], x: f64, y: f64) -> bool {
    let n = p.len();
    let mut inside = false;
    for i in 0..n {
        let (a, b) = (p[i], p[(i + 1) % n]);
        if (a.y > y) != (b.y > y) {
            let ix = a.x + (y - a.y) / (b.y - a.y) * (b.x - a.x);
            if ix > x {
                inside = !inside;
            }
        }
    }
    inside
}

fn cross2(o: Pt, a: Pt, b: Pt) -> f64 {
    (a.x - o.x) * (b.y - o.y) - (a.y - o.y) * (b.x - o.x)
}

/// Inclusive point-in-triangle for a counter-clockwise triangle.
fn in_triangle(p: Pt, a: Pt, b: Pt, c: Pt) -> bool {
    cross2(a, b, p) >= 0.0 && cross2(b, c, p) >= 0.0 && cross2(c, a, p) >= 0.0
}

fn same_pos(a: Pt, b: Pt) -> bool {
    a.x == b.x && a.y == b.y
}

/// Triangulates one planar polygon with holes (mm). Indices run over `outer` and then each hole
/// in order; triangles are counter-clockwise. Used by the emboss tool for its flat faces.
pub(crate) fn triangulate_2d(outer: &[[f64; 2]], holes: &[Vec<[f64; 2]>]) -> Vec<[u32; 3]> {
    let mut idx = 0u32;
    let mut ring = |r: &[[f64; 2]]| {
        r.iter()
            .map(|p| {
                let pt = Pt {
                    idx,
                    x: p[0],
                    y: p[1],
                };
                idx += 1;
                pt
            })
            .collect::<Vec<_>>()
    };
    let outer = ring(outer);
    let holes = holes.iter().map(|h| ring(h)).collect();
    triangulate_with_holes(outer, holes)
}

/// Ear clipping with hole bridging: the outer contour is made counter-clockwise, holes clockwise,
/// each hole is joined to the enclosing polygon at mutually visible vertices (Eberly), and the
/// resulting weakly simple polygon is ear-clipped.
fn triangulate_with_holes(mut outer: Vec<Pt>, mut holes: Vec<Vec<Pt>>) -> Vec<[u32; 3]> {
    if signed_area(&outer) < 0.0 {
        outer.reverse();
    }
    for h in &mut holes {
        if signed_area(h) > 0.0 {
            h.reverse();
        }
    }
    let max_x = |h: &[Pt]| h.iter().map(|p| p.x).fold(f64::MIN, f64::max);
    holes.sort_by(|a, b| max_x(b).total_cmp(&max_x(a)));
    for hole in holes {
        outer = bridge_hole(outer, hole);
    }
    ear_clip(&outer)
}

fn is_reflex(poly: &[Pt], i: usize) -> bool {
    let n = poly.len();
    cross2(poly[(i + n - 1) % n], poly[i], poly[(i + 1) % n]) < 0.0
}

fn bridge_hole(outer: Vec<Pt>, hole: Vec<Pt>) -> Vec<Pt> {
    let n = outer.len();
    let mi = (0..hole.len())
        .max_by(|&a, &b| hole[a].x.total_cmp(&hole[b].x))
        .unwrap_or(0);
    let m = hole[mi];
    // Closest intersection of the ray from m towards +x with an outer edge.
    let mut hit: Option<(f64, usize)> = None;
    for i in 0..n {
        let (a, b) = (outer[i], outer[(i + 1) % n]);
        if (a.y <= m.y && b.y > m.y) || (b.y <= m.y && a.y > m.y) {
            let ix = a.x + (m.y - a.y) / (b.y - a.y) * (b.x - a.x);
            if ix >= m.x && hit.is_none_or(|(hx, _)| ix < hx) {
                hit = Some((ix, i));
            }
        }
    }
    let Some((ix, i)) = hit else {
        // Hole not actually enclosed; leave it out rather than fail.
        return outer;
    };
    let (a, b) = (outer[i], outer[(i + 1) % n]);
    let ipt = Pt {
        idx: u32::MAX,
        x: ix,
        y: m.y,
    };
    let mut best = if same_pos(a, ipt) {
        i
    } else if same_pos(b, ipt) {
        (i + 1) % n
    } else if a.x > b.x {
        i
    } else {
        (i + 1) % n
    };
    let score = |q: Pt| {
        let (dx, dy) = (q.x - m.x, q.y - m.y);
        let len = (dx * dx + dy * dy).sqrt();
        if len == 0.0 {
            (1.0, 0.0)
        } else {
            (dx / len, len)
        }
    };
    let (mut best_cos, mut best_len) = score(outer[best]);
    let p = outer[best];
    for j in 0..n {
        if j == best || !is_reflex(&outer, j) {
            continue;
        }
        let q = outer[j];
        if same_pos(q, p) || !in_triangle(q, m, ipt, p) {
            continue;
        }
        let (c, l) = score(q);
        if c > best_cos || (c == best_cos && l < best_len) {
            best = j;
            best_cos = c;
            best_len = l;
        }
    }
    let mut merged = Vec::with_capacity(n + hole.len() + 2);
    merged.extend_from_slice(&outer[..=best]);
    merged.extend_from_slice(&hole[mi..]);
    merged.extend_from_slice(&hole[..=mi]);
    merged.extend_from_slice(&outer[best..]);
    merged
}

/// Ear clipping of a counter-clockwise, weakly simple polygon. Strictly convex, unblocked ears are
/// preferred; collinear ears are taken when nothing else is left, and as a last resort any convex
/// vertex is clipped so the loop always terminates.
fn ear_clip(poly: &[Pt]) -> Vec<[u32; 3]> {
    let n = poly.len();
    let mut tris = Vec::with_capacity(n.saturating_sub(2));
    if n < 3 {
        return tris;
    }
    let mut prev: Vec<usize> = (0..n).map(|i| (i + n - 1) % n).collect();
    let mut next: Vec<usize> = (0..n).map(|i| (i + 1) % n).collect();
    let turn =
        |prev: &[usize], next: &[usize], i: usize| cross2(poly[prev[i]], poly[i], poly[next[i]]);
    let mut reflex: Vec<bool> = (0..n).map(|i| turn(&prev, &next, i) < 0.0).collect();
    let mut remaining = n;
    let mut cur = 0;
    let mut level = 0; // 0: strictly convex ears, 1: collinear ears too, 2: clip anything convex
    let mut visited = 0;
    let push = |tris: &mut Vec<[u32; 3]>, a: usize, b: usize, c: usize| {
        let f = [poly[a].idx, poly[b].idx, poly[c].idx];
        if f[0] != f[1] && f[1] != f[2] && f[0] != f[2] {
            tris.push(f);
        }
    };
    while remaining > 3 {
        let (a, b, c) = (prev[cur], cur, next[cur]);
        let t = turn(&prev, &next, b);
        let convex = match level {
            0 => t > 0.0,
            1 => t >= 0.0,
            _ => !reflex[b],
        };
        let mut ear = convex;
        if ear && level < 2 {
            let mut k = next[c];
            while k != a {
                if reflex[k] {
                    let q = poly[k];
                    if !same_pos(q, poly[a])
                        && !same_pos(q, poly[b])
                        && !same_pos(q, poly[c])
                        && in_triangle(q, poly[a], poly[b], poly[c])
                    {
                        ear = false;
                        break;
                    }
                }
                k = next[k];
            }
        }
        if ear {
            push(&mut tris, a, b, c);
            next[a] = c;
            prev[c] = a;
            remaining -= 1;
            reflex[a] = turn(&prev, &next, a) < 0.0;
            reflex[c] = turn(&prev, &next, c) < 0.0;
            cur = c;
            level = 0;
            visited = 0;
        } else {
            cur = c;
            visited += 1;
            if visited >= remaining {
                visited = 0;
                level += 1;
                if level > 2 {
                    // Nothing convex at all (degenerate numerics): clip the current vertex.
                    push(&mut tris, a, b, c);
                    next[a] = c;
                    prev[c] = a;
                    remaining -= 1;
                    cur = c;
                    level = 0;
                }
            }
        }
    }
    push(&mut tris, prev[cur], cur, next[cur]);
    tris
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
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

fn normalize(v: [f64; 3]) -> Option<[f64; 3]> {
    let len = dot(v, v).sqrt();
    (len > 0.0 && len.is_finite()).then(|| v.map(|c| c / len))
}

/// Orthonormal `(u, v)` spanning the plane with unit normal `n`, such that `u × v = n`.
fn plane_basis(n: [f64; 3]) -> ([f64; 3], [f64; 3]) {
    let axis = if n[0].abs() <= n[1].abs() && n[0].abs() <= n[2].abs() {
        [1.0, 0.0, 0.0]
    } else if n[1].abs() <= n[2].abs() {
        [0.0, 1.0, 0.0]
    } else {
        [0.0, 0.0, 1.0]
    };
    let u = normalize(cross(n, axis)).unwrap_or([1.0, 0.0, 0.0]);
    let v = cross(n, u);
    (u, v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::tests::box_mesh;

    fn z_plane(z: f32) -> CutPlane {
        CutPlane {
            point: [0.0, 0.0, z],
            normal: [0.0, 0.0, 1.0],
        }
    }

    fn quad(tris: &mut Vec<[[f32; 3]; 3]>, q: [[f32; 3]; 4]) {
        tris.push([q[0], q[1], q[2]]);
        tris.push([q[0], q[2], q[3]]);
    }

    /// 20x20x10 block with a 6x6 through-hole along Z (from (7,7) to (13,13)), volume 3640.
    pub(crate) fn frame_mesh() -> TriangleMesh {
        let mut t = Vec::new();
        let (lo, hi, h) = (7.0, 13.0, 10.0);
        // Outer walls.
        quad(
            &mut t,
            [[0., 0., 0.], [20., 0., 0.], [20., 0., h], [0., 0., h]],
        );
        quad(
            &mut t,
            [[20., 0., 0.], [20., 20., 0.], [20., 20., h], [20., 0., h]],
        );
        quad(
            &mut t,
            [[20., 20., 0.], [0., 20., 0.], [0., 20., h], [20., 20., h]],
        );
        quad(
            &mut t,
            [[0., 20., 0.], [0., 0., 0.], [0., 0., h], [0., 20., h]],
        );
        // Hole walls (facing into the hole).
        quad(
            &mut t,
            [[lo, lo, 0.], [lo, hi, 0.], [lo, hi, h], [lo, lo, h]],
        );
        quad(
            &mut t,
            [[hi, lo, 0.], [hi, lo, h], [hi, hi, h], [hi, hi, 0.]],
        );
        quad(
            &mut t,
            [[lo, lo, 0.], [lo, lo, h], [hi, lo, h], [hi, lo, 0.]],
        );
        quad(
            &mut t,
            [[lo, hi, 0.], [hi, hi, 0.], [hi, hi, h], [lo, hi, h]],
        );
        // Top annulus (+z) as four trapezoids, bottom mirrored.
        let strips = [
            [[0., 0.], [20., 0.], [hi, lo], [lo, lo]],
            [[20., 0.], [20., 20.], [hi, hi], [hi, lo]],
            [[20., 20.], [0., 20.], [lo, hi], [hi, hi]],
            [[0., 20.], [0., 0.], [lo, lo], [lo, hi]],
        ];
        for s in strips {
            quad(&mut t, s.map(|p| [p[0], p[1], h]));
            let [a, b, c, d] = s.map(|p| [p[0], p[1], 0.0]);
            quad(&mut t, [d, c, b, a]);
        }
        TriangleMesh::from_triangles(&t)
    }

    #[test]
    fn frame_fixture_is_sane() {
        let m = frame_mesh();
        assert!(is_watertight(&m));
        assert!((m.volume() - 3640.0).abs() < 1e-3);
    }

    #[test]
    fn box_is_watertight_and_open_box_is_not() {
        let m = box_mesh([0.0; 3], [20.0; 3]);
        assert!(is_watertight(&m));
        let mut open = m.clone();
        open.faces.pop();
        assert!(!is_watertight(&open));
    }

    #[test]
    fn horizontal_cut_of_box() {
        let m = box_mesh([0.0; 3], [20.0; 3]);
        let r = cut_mesh(&m, &z_plane(7.0), true);
        let (upper, lower) = (r.upper.unwrap(), r.lower.unwrap());
        assert!(is_watertight(&upper), "upper not watertight");
        assert!(is_watertight(&lower), "lower not watertight");
        assert!((upper.volume() - 20.0 * 20.0 * 13.0).abs() < 1e-3);
        assert!((lower.volume() - 20.0 * 20.0 * 7.0).abs() < 1e-3);
        assert!(upper.faces.len() >= 12 && lower.faces.len() >= 12);
        let (umin, umax) = upper.bounds();
        assert!((umin[2] - 7.0).abs() < 1e-6 && (umax[2] - 20.0).abs() < 1e-6);
        let (lmin, lmax) = lower.bounds();
        assert!(lmin[2].abs() < 1e-6 && (lmax[2] - 7.0).abs() < 1e-6);
    }

    #[test]
    fn upper_is_the_side_the_normal_points_to() {
        let m = box_mesh([0.0; 3], [20.0; 3]);
        let plane = CutPlane {
            point: [3.0, 3.0, 7.0],
            normal: [0.0, 0.0, -2.5],
        };
        let r = cut_mesh(&m, &plane, true);
        assert!((r.upper.unwrap().volume() - 20.0 * 20.0 * 7.0).abs() < 1e-3);
        assert!((r.lower.unwrap().volume() - 20.0 * 20.0 * 13.0).abs() < 1e-3);
    }

    #[test]
    fn tilted_cut_through_vertices_keeps_volume() {
        let m = box_mesh([0.0; 3], [20.0; 3]);
        // x + z = 20 passes through four box corners.
        let plane = CutPlane {
            point: [10.0, 10.0, 10.0],
            normal: [1.0, 0.0, 1.0],
        };
        let r = cut_mesh(&m, &plane, true);
        let (upper, lower) = (r.upper.unwrap(), r.lower.unwrap());
        assert!(is_watertight(&upper));
        assert!(is_watertight(&lower));
        assert!((upper.volume() - 4000.0).abs() < 1e-3);
        assert!((lower.volume() - 4000.0).abs() < 1e-3);
    }

    #[test]
    fn tilted_cut_generic_keeps_volume() {
        let m = box_mesh([0.0; 3], [20.0; 3]);
        let plane = CutPlane {
            point: [7.3, 1.0, 2.0],
            normal: [1.0, 0.0, 1.0],
        };
        let r = cut_mesh(&m, &plane, true);
        let (upper, lower) = (r.upper.unwrap(), r.lower.unwrap());
        assert!(is_watertight(&upper));
        assert!(is_watertight(&lower));
        assert!(upper.volume() > 0.0 && lower.volume() > 0.0);
        assert!((upper.volume() + lower.volume() - 8000.0).abs() < 1e-3);
    }

    #[test]
    fn cut_through_hole_caps_with_hole() {
        let m = frame_mesh();
        let r = cut_mesh(&m, &z_plane(4.0), true);
        let (upper, lower) = (r.upper.unwrap(), r.lower.unwrap());
        assert!(is_watertight(&upper), "upper not watertight");
        assert!(is_watertight(&lower), "lower not watertight");
        assert!((lower.volume() - 364.0 * 4.0).abs() < 1e-3);
        assert!((upper.volume() - 364.0 * 6.0).abs() < 1e-3);
        // The hole must stay open: nothing of either half lies inside the hole column.
        for half in [&upper, &lower] {
            for f in &half.faces {
                let c = f.iter().fold([0.0f32; 3], |acc, &i| {
                    let v = half.vertices[i as usize];
                    [
                        acc[0] + v[0] / 3.0,
                        acc[1] + v[1] / 3.0,
                        acc[2] + v[2] / 3.0,
                    ]
                });
                assert!(
                    !(c[0] > 7.0 + 1e-3
                        && c[0] < 13.0 - 1e-3
                        && c[1] > 7.0 + 1e-3
                        && c[1] < 13.0 - 1e-3),
                    "triangle centroid {c:?} inside the hole"
                );
            }
        }
    }

    #[test]
    fn tilted_cut_of_frame_keeps_volume() {
        let m = frame_mesh();
        let plane = CutPlane {
            point: [10.0, 10.0, 5.0],
            normal: [0.3, 0.2, 1.0],
        };
        let r = cut_mesh(&m, &plane, true);
        let (upper, lower) = (r.upper.unwrap(), r.lower.unwrap());
        assert!(is_watertight(&upper));
        assert!(is_watertight(&lower));
        assert!((upper.volume() + lower.volume() - 3640.0).abs() < 1e-3);
    }

    #[test]
    fn plane_missing_the_mesh() {
        let m = box_mesh([0.0; 3], [20.0; 3]);
        let r = cut_mesh(&m, &z_plane(25.0), true);
        assert!(r.upper.is_none());
        let lower = r.lower.unwrap();
        assert_eq!(lower.faces, m.faces);
        assert_eq!(lower.vertices, m.vertices);
        let r = cut_mesh(&m, &z_plane(-1.0), true);
        assert!(r.lower.is_none());
        assert_eq!(r.upper.unwrap().faces.len(), 12);
        // Exactly touching a face counts as missing too.
        let r = cut_mesh(&m, &z_plane(20.0), true);
        assert!(r.upper.is_none());
        assert_eq!(r.lower.unwrap().faces.len(), 12);
    }

    #[test]
    fn uncapped_halves_are_open() {
        let m = box_mesh([0.0; 3], [20.0; 3]);
        let r = cut_mesh(&m, &z_plane(7.0), false);
        let (upper, lower) = (r.upper.unwrap(), r.lower.unwrap());
        assert!(!is_watertight(&upper));
        assert!(!is_watertight(&lower));
        // Open halves have no cap, so the signed volume is not the full prism.
        assert!(upper.faces.len() < 12 + 4);
    }

    #[test]
    fn cut_is_deterministic() {
        let m = frame_mesh();
        let plane = CutPlane {
            point: [9.0, 11.0, 5.0],
            normal: [0.1, -0.4, 1.0],
        };
        let a = cut_mesh(&m, &plane, true);
        let b = cut_mesh(&m, &plane, true);
        let (au, bu) = (a.upper.unwrap(), b.upper.unwrap());
        assert_eq!(au.vertices, bu.vertices);
        assert_eq!(au.faces, bu.faces);
        let (al, bl) = (a.lower.unwrap(), b.lower.unwrap());
        assert_eq!(al.vertices, bl.vertices);
        assert_eq!(al.faces, bl.faces);
    }

    fn assert_clean_cut(mesh: &TriangleMesh, plane: &CutPlane) {
        let r = cut_mesh(mesh, plane, true);
        let (upper, lower) = (r.upper.unwrap(), r.lower.unwrap());
        assert!(is_watertight(&upper), "upper not watertight for {plane:?}");
        assert!(is_watertight(&lower), "lower not watertight for {plane:?}");
        assert!(upper.volume() > 0.0 && lower.volume() > 0.0);
        let total = mesh.volume();
        assert!(
            (upper.volume() + lower.volume() - total).abs() < 1e-3 * total.max(1.0),
            "volumes {} + {} != {total} for {plane:?}",
            upper.volume(),
            lower.volume()
        );
    }

    #[test]
    fn dense_primitives_cut_cleanly() {
        let sphere = crate::mesh_edit::sphere(10.0, 24, 48);
        let cyl = crate::mesh_edit::cylinder(8.0, 20.0, 96);
        for (i, normal) in [
            [0.0, 0.0, 1.0],
            [1.0, 0.0, 1.0],
            [0.3, -0.7, 0.2],
            [1.0, 0.0, 0.0],
        ]
        .into_iter()
        .enumerate()
        {
            let point = [0.1 * i as f32, -0.2 * i as f32, 0.5 + 0.7 * i as f32];
            assert_clean_cut(&sphere, &CutPlane { point, normal });
            assert_clean_cut(&cyl, &CutPlane { point, normal });
        }
    }

    #[test]
    fn hole_block_model_cuts_cleanly() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/models/hole_block.stl"
        );
        let m = TriangleMesh::load_stl(path).unwrap();
        assert!(is_watertight(&m));
        let (min, max) = m.bounds();
        let mid = [
            (min[0] + max[0]) / 2.0,
            (min[1] + max[1]) / 2.0,
            (min[2] + max[2]) / 2.0,
        ];
        for normal in [[0.0, 0.0, 1.0], [0.0, 1.0, 0.0], [0.4, 0.3, 1.0]] {
            assert_clean_cut(&m, &CutPlane { point: mid, normal });
        }
    }

    #[test]
    fn welding_merges_near_vertices() {
        let mut m = box_mesh([0.0; 3], [20.0; 3]);
        // Duplicate a vertex with a tiny offset and point one face at the copy.
        let v = m.vertices[0];
        m.vertices.push([v[0] + 2e-6, v[1], v[2]]);
        let copy = (m.vertices.len() - 1) as u32;
        for f in &mut m.faces {
            if f[0] == 0 {
                f[0] = copy;
                break;
            }
        }
        assert!(is_watertight(&m));
        assert_eq!(weld_vertices(&m, WELD_TOLERANCE).vertices.len(), 8);
    }

    #[test]
    fn section_cap_fills_the_cross_section_facing_the_normal() {
        let m = box_mesh([0.0, 0.0, 0.0], [10.0, 20.0, 30.0]);
        let cap = section_cap(&m, &z_plane(12.0));
        let mut area = 0.0;
        for f in &cap.faces {
            let [a, b, c] = f.map(|i| cap.vertices[i as usize].map(f64::from));
            let n = cross(sub(b, a), sub(c, a));
            assert!(n[2] > 0.0, "cap faces +normal");
            area += 0.5 * n[2];
            for p in [a, b, c] {
                assert!((p[2] - 12.0).abs() < 1e-5);
            }
        }
        assert!((area - 200.0).abs() < 1e-6, "{area}");
        // A tilted plane through the middle, and a plane that misses.
        let tilted = section_cap(
            &m,
            &CutPlane {
                point: [5.0, 10.0, 15.0],
                normal: [0.0, 1.0, 1.0],
            },
        );
        assert!(!tilted.faces.is_empty());
        assert!(section_cap(&m, &z_plane(40.0)).faces.is_empty());
    }
}
