// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/TriangleMesh.cpp :: trianglemesh_repair_on_import
// Source: prusaslicer@b028299c77 bundled_deps/admesh/admesh/connect.cpp :: stl_check_facets_exact, stl_check_facets_nearby, stl_remove_unconnected_facets, stl_fill_holes
// Source: prusaslicer@b028299c77 bundled_deps/admesh/admesh/normals.cpp :: stl_fix_normal_directions
// Source: prusaslicer@b028299c77 bundled_deps/admesh/admesh/util.cpp :: stl_calculate_volume
// Changes: The repair-on-import sequence (exact vertex merge with degenerate facets dropped, nearby matching of open edges with a growing tolerance, unconnected facets removed, normal directions made consistent per part, all facets reversed when the volume is negative) re-implemented on Mica's indexed mesh instead of admesh's facet/neighbour tables: duplicate facets removed (both of an opposite pair), nearby matching requires both ends of a reversed open edge within the tolerance (capped at half the shortest edge), each part keeps its majority orientation, only un-nested parts are turned outward, and holes are filled (upstream disables stl_fill_holes on import) by ear clipping each boundary loop in its Newell plane with a centroid fan as fallback. A mesh that is already closed and outward is returned untouched.

//! Mesh repair, on import and on demand (PrusaSlicer's admesh repair): merge duplicate
//! vertices, drop degenerate and duplicate triangles, close cracks between nearly touching
//! open edges, drop loose triangles, make normals consistent and outward, and fill the holes
//! that are left. [`repair`] leaves a clean mesh alone and says what it fixed otherwise.

use crate::mesh::TriangleMesh;
use std::collections::{HashMap, HashSet};
use std::fmt;

/// What a repair changed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RepairReport {
    /// Vertices merged into an identical or nearby one.
    pub vertices_merged: usize,
    /// Triangles with two identical corners (needles and points) removed.
    pub degenerate_removed: usize,
    /// Triangles that duplicated another one removed.
    pub duplicates_removed: usize,
    /// Loose triangles with no neighbour removed.
    pub unconnected_removed: usize,
    /// Triangles turned around so the surface faces outwards consistently.
    pub faces_flipped: usize,
    /// Holes closed, and the triangles added for them.
    pub holes_filled: usize,
    pub faces_added: usize,
    /// Open edges that could not be closed.
    pub open_edges_left: usize,
}

impl RepairReport {
    /// Whether nothing was changed.
    pub fn is_empty(&self) -> bool {
        RepairReport {
            open_edges_left: 0,
            ..self.clone()
        } == RepairReport::default()
    }
    /// One plain-language line per fix ("Merged 12 vertices").
    pub fn lines(&self) -> Vec<String> {
        let plural =
            |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
        let mut out = Vec::new();
        if self.vertices_merged > 0 {
            out.push(format!(
                "Merged {}",
                plural(
                    self.vertices_merged,
                    "duplicate vertex",
                    "duplicate vertices"
                )
            ));
        }
        if self.degenerate_removed > 0 {
            out.push(format!(
                "Removed {}",
                plural(
                    self.degenerate_removed,
                    "degenerate face",
                    "degenerate faces"
                )
            ));
        }
        if self.duplicates_removed > 0 {
            out.push(format!(
                "Removed {}",
                plural(self.duplicates_removed, "duplicate face", "duplicate faces")
            ));
        }
        if self.unconnected_removed > 0 {
            out.push(format!(
                "Removed {}",
                plural(self.unconnected_removed, "loose face", "loose faces")
            ));
        }
        if self.faces_flipped > 0 {
            out.push(format!(
                "Flipped {}",
                plural(self.faces_flipped, "inverted face", "inverted faces")
            ));
        }
        if self.holes_filled > 0 {
            out.push(format!(
                "Filled {} ({})",
                plural(self.holes_filled, "hole", "holes"),
                plural(self.faces_added, "face", "faces")
            ));
        }
        if self.open_edges_left > 0 {
            out.push(format!(
                "{} could not be closed",
                plural(self.open_edges_left, "open edge", "open edges")
            ));
        }
        out
    }
}

impl fmt::Display for RepairReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let lines = self.lines();
        if lines.is_empty() {
            write!(f, "No errors found")
        } else {
            write!(f, "{}", lines.join(", "))
        }
    }
}

/// Topology counts of a mesh as it is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MeshCheck {
    /// Triangles with two identical corner indices.
    pub degenerate: usize,
    /// Edges whose two directions are not used equally often (holes, flipped neighbours).
    pub bad_edges: usize,
    /// Enclosed volume is negative (inside out).
    pub inverted: bool,
}

impl MeshCheck {
    pub fn is_clean(&self) -> bool {
        self.degenerate == 0 && self.bad_edges == 0 && !self.inverted
    }
}

/// Checks a mesh by index: closed means every edge is used as often in each direction.
pub fn check(mesh: &TriangleMesh) -> MeshCheck {
    let mut count: HashMap<(u32, u32), i32> = HashMap::with_capacity(mesh.faces.len() * 2);
    let mut degenerate = 0;
    for f in &mesh.faces {
        if f[0] == f[1] || f[1] == f[2] || f[0] == f[2] {
            degenerate += 1;
            continue;
        }
        for k in 0..3 {
            let (u, v) = (f[k], f[(k + 1) % 3]);
            if u < v {
                *count.entry((u, v)).or_default() += 1;
            } else {
                *count.entry((v, u)).or_default() -= 1;
            }
        }
    }
    MeshCheck {
        degenerate,
        bad_edges: count.values().filter(|&&c| c != 0).count(),
        inverted: mesh.volume() < 0.,
    }
}

/// Repairs `mesh` when it needs it: `None` for a clean mesh (left exactly as it is),
/// otherwise the repaired mesh and what changed.
pub fn repair(mesh: &TriangleMesh) -> Option<(TriangleMesh, RepairReport)> {
    if mesh.faces.is_empty() || check(mesh).is_clean() {
        return None;
    }
    Some(repair_always(mesh))
}

/// Runs every repair step (on demand); a clean mesh comes back unchanged with an empty
/// report.
pub fn repair_always(mesh: &TriangleMesh) -> (TriangleMesh, RepairReport) {
    let mut report = RepairReport::default();
    if mesh.faces.is_empty() || check(mesh).is_clean() {
        return (mesh.clone(), report);
    }
    let mut m = merge_exact(mesh, &mut report);
    remove_degenerate(&mut m, &mut report);
    remove_duplicates(&mut m, &mut report);
    orient_parts(&mut m, &mut report);
    if !open_half_edges(&m).is_empty() {
        let (lo, hi) = m.bounds();
        let diag = (0..3)
            .map(|k| f64::from(hi[k] - lo[k]).powi(2))
            .sum::<f64>()
            .sqrt();
        let shortest = shortest_edge(&m);
        let increment = diag / 10000.;
        for i in 0..2 {
            let tol = (increment * (i + 1) as f64).min(shortest * 0.5);
            if tol <= 0. || !merge_nearby(&mut m, tol, &mut report) {
                break;
            }
            remove_degenerate(&mut m, &mut report);
            remove_duplicates(&mut m, &mut report);
            orient_parts(&mut m, &mut report);
            if open_half_edges(&m).is_empty() {
                break;
            }
        }
    }
    remove_unconnected(&mut m, &mut report);
    fill_holes(&mut m, &mut report);
    orient_parts(&mut m, &mut report);
    outward(&mut m, &mut report);
    report.open_edges_left = open_half_edges(&m).len();
    compact(&mut m);
    (m, report)
}

fn key(v: [f32; 3]) -> [u32; 3] {
    v.map(|c| if c == 0. { 0f32 } else { c }.to_bits())
}

fn merge_exact(mesh: &TriangleMesh, report: &mut RepairReport) -> TriangleMesh {
    let mut index: HashMap<[u32; 3], u32> = HashMap::with_capacity(mesh.vertices.len());
    let mut vertices = Vec::with_capacity(mesh.vertices.len());
    let mut used = vec![false; mesh.vertices.len()];
    for f in &mesh.faces {
        for &i in f {
            used[i as usize] = true;
        }
    }
    let remap: Vec<u32> = mesh
        .vertices
        .iter()
        .enumerate()
        .map(|(i, &v)| {
            let len = vertices.len() as u32;
            let id = *index.entry(key(v)).or_insert(len);
            if id == len {
                vertices.push(v);
            } else if used[i] {
                report.vertices_merged += 1;
            }
            id
        })
        .collect();
    TriangleMesh {
        vertices,
        faces: mesh
            .faces
            .iter()
            .map(|f| f.map(|i| remap[i as usize]))
            .collect(),
    }
}

fn remove_degenerate(m: &mut TriangleMesh, report: &mut RepairReport) {
    let before = m.faces.len();
    m.faces
        .retain(|f| f[0] != f[1] && f[1] != f[2] && f[0] != f[2]);
    report.degenerate_removed += before - m.faces.len();
}

/// Same corner set twice: one of two equally oriented copies goes, an opposite pair (a
/// zero-thickness fin) goes entirely.
fn remove_duplicates(m: &mut TriangleMesh, report: &mut RepairReport) {
    let canon = |f: &[u32; 3]| {
        let mut s = *f;
        s.sort_unstable();
        s
    };
    let rot = |f: &[u32; 3]| {
        let k = (0..3).min_by_key(|&k| f[k]).unwrap();
        [f[k], f[(k + 1) % 3], f[(k + 2) % 3]]
    };
    let mut groups: HashMap<[u32; 3], Vec<usize>> = HashMap::new();
    for (i, f) in m.faces.iter().enumerate() {
        groups.entry(canon(f)).or_default().push(i);
    }
    let mut drop = vec![false; m.faces.len()];
    for list in groups.values().filter(|l| l.len() > 1) {
        let mut forward = Vec::new();
        let mut backward = Vec::new();
        let first = rot(&m.faces[list[0]]);
        for &i in list {
            if rot(&m.faces[i]) == first {
                forward.push(i);
            } else {
                backward.push(i);
            }
        }
        // Opposite copies cancel; of what is left, one stays.
        let pairs = forward.len().min(backward.len());
        let more = if forward.len() >= backward.len() {
            forward
        } else {
            backward
        };
        let keep = more.get(pairs).copied();
        for &i in list {
            drop[i] = Some(i) != keep;
        }
    }
    let before = m.faces.len();
    let mut k = 0;
    m.faces.retain(|_| {
        k += 1;
        !drop[k - 1]
    });
    report.duplicates_removed += before - m.faces.len();
}

/// Directed edges whose reverse is missing (with multiplicity).
fn open_half_edges(m: &TriangleMesh) -> Vec<(u32, u32)> {
    let mut count: HashMap<(u32, u32), i32> = HashMap::with_capacity(m.faces.len() * 3);
    for f in &m.faces {
        for k in 0..3 {
            *count.entry((f[k], f[(k + 1) % 3])).or_default() += 1;
        }
    }
    let mut out = Vec::new();
    for (&(u, v), &c) in &count {
        let back = count.get(&(v, u)).copied().unwrap_or(0);
        for _ in 0..(c - back).max(0) {
            out.push((u, v));
        }
    }
    out.sort_unstable();
    out
}

fn shortest_edge(m: &TriangleMesh) -> f64 {
    let mut best = f64::INFINITY;
    for f in &m.faces {
        for k in 0..3 {
            let (a, b) = (
                m.vertices[f[k] as usize],
                m.vertices[f[(k + 1) % 3] as usize],
            );
            let d = (0..3)
                .map(|i| f64::from(a[i] - b[i]).powi(2))
                .sum::<f64>()
                .sqrt();
            if d > 0. {
                best = best.min(d);
            }
        }
    }
    if best.is_finite() { best } else { 0. }
}

fn dist(a: [f32; 3], b: [f32; 3]) -> f64 {
    (0..3)
        .map(|i| f64::from(a[i] - b[i]).powi(2))
        .sum::<f64>()
        .sqrt()
}

/// Matches each open edge with a reversed open edge whose ends are both within `tol` and
/// merges those vertices (admesh's "check nearby"). Returns whether anything merged.
fn merge_nearby(m: &mut TriangleMesh, tol: f64, report: &mut RepairReport) -> bool {
    let open = open_half_edges(m);
    if open.is_empty() {
        return false;
    }
    let cell = tol.max(1e-9);
    let cell_of = |v: [f32; 3]| v.map(|c| (f64::from(c) / cell).floor() as i64);
    // Open edges by the cell of their start vertex.
    let mut grid: HashMap<[i64; 3], Vec<usize>> = HashMap::new();
    for (i, &(u, _)) in open.iter().enumerate() {
        grid.entry(cell_of(m.vertices[u as usize]))
            .or_default()
            .push(i);
    }
    let mut parent: Vec<u32> = (0..m.vertices.len() as u32).collect();
    fn find(p: &mut [u32], mut i: u32) -> u32 {
        while p[i as usize] != i {
            p[i as usize] = p[p[i as usize] as usize];
            i = p[i as usize];
        }
        i
    }
    let mut used = vec![false; open.len()];
    let mut merged = 0;
    for i in 0..open.len() {
        if used[i] {
            continue;
        }
        let (u, v) = open[i];
        // The partner runs v' → u' with v' near v.
        let c = cell_of(m.vertices[v as usize]);
        let mut best: Option<(f64, usize)> = None;
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    let Some(list) = grid.get(&[c[0] + dx, c[1] + dy, c[2] + dz]) else {
                        continue;
                    };
                    for &j in list {
                        if j == i || used[j] {
                            continue;
                        }
                        let (x, y) = open[j];
                        if x == v && y == u {
                            continue;
                        }
                        let d1 = dist(m.vertices[x as usize], m.vertices[v as usize]);
                        let d2 = dist(m.vertices[y as usize], m.vertices[u as usize]);
                        if d1 <= tol && d2 <= tol && best.is_none_or(|(b, _)| d1 + d2 < b) {
                            best = Some((d1 + d2, j));
                        }
                    }
                }
            }
        }
        if let Some((_, j)) = best {
            used[i] = true;
            used[j] = true;
            let (x, y) = open[j];
            for (keep, gone) in [(v, x), (u, y)] {
                let (a, b) = (find(&mut parent, keep), find(&mut parent, gone));
                if a != b {
                    parent[b as usize] = a;
                    merged += 1;
                }
            }
        }
    }
    if merged == 0 {
        return false;
    }
    for f in &mut m.faces {
        for i in f.iter_mut() {
            *i = find(&mut parent, *i);
        }
    }
    report.vertices_merged += merged;
    true
}

/// Faces of each connected part (through edges used by exactly two faces) turned to agree
/// with their neighbours, keeping the part's majority orientation.
fn orient_parts(m: &mut TriangleMesh, report: &mut RepairReport) {
    let n = m.faces.len();
    let mut by_edge: HashMap<(u32, u32), Vec<usize>> = HashMap::with_capacity(n * 2);
    for (i, f) in m.faces.iter().enumerate() {
        for k in 0..3 {
            let (u, v) = (f[k], f[(k + 1) % 3]);
            by_edge.entry((u.min(v), u.max(v))).or_default().push(i);
        }
    }
    let has = |f: &[u32; 3], u: u32, v: u32| (0..3).any(|k| f[k] == u && f[(k + 1) % 3] == v);
    let mut flip = vec![false; n];
    let mut seen = vec![false; n];
    for start in 0..n {
        if seen[start] {
            continue;
        }
        let mut part = vec![start];
        seen[start] = true;
        let mut stack = vec![start];
        while let Some(f) = stack.pop() {
            let face = m.faces[f];
            for k in 0..3 {
                let (u, v) = (face[k], face[(k + 1) % 3]);
                let list = &by_edge[&(u.min(v), u.max(v))];
                if list.len() != 2 {
                    continue;
                }
                let g = if list[0] == f { list[1] } else { list[0] };
                if g == f || seen[g] {
                    continue;
                }
                // As oriented so far: f holds u→v when not flipped.
                let (fu, fv) = if flip[f] { (v, u) } else { (u, v) };
                let same_dir = has(&m.faces[g], fu, fv);
                flip[g] = same_dir;
                seen[g] = true;
                part.push(g);
                stack.push(g);
            }
        }
        let flipped = part.iter().filter(|&&f| flip[f]).count();
        if flipped * 2 > part.len() {
            for &f in &part {
                flip[f] = !flip[f];
            }
        }
    }
    for (f, &fl) in m.faces.iter_mut().zip(&flip) {
        if fl {
            f.swap(1, 2);
            report.faces_flipped += 1;
        }
    }
}

/// Loose triangles: none of their edges is shared.
fn remove_unconnected(m: &mut TriangleMesh, report: &mut RepairReport) {
    let mut count: HashMap<(u32, u32), u32> = HashMap::with_capacity(m.faces.len() * 2);
    for f in &m.faces {
        for k in 0..3 {
            let (u, v) = (f[k], f[(k + 1) % 3]);
            *count.entry((u.min(v), u.max(v))).or_default() += 1;
        }
    }
    let before = m.faces.len();
    m.faces.retain(|f| {
        (0..3).any(|k| {
            let (u, v) = (f[k], f[(k + 1) % 3]);
            count[&(u.min(v), u.max(v))] > 1
        })
    });
    report.unconnected_removed += before - m.faces.len();
}

/// Closes each loop of open edges with triangles facing the same way as the surface.
fn fill_holes(m: &mut TriangleMesh, report: &mut RepairReport) {
    let open = open_half_edges(m);
    if open.is_empty() {
        return;
    }
    let mut next: HashMap<u32, Vec<u32>> = HashMap::new();
    for &(u, v) in &open {
        next.entry(u).or_default().push(v);
    }
    let mut starts: Vec<u32> = open.iter().map(|&(u, _)| u).collect();
    starts.sort_unstable();
    starts.dedup();
    for s in starts {
        while next.get(&s).is_some_and(|l| !l.is_empty()) {
            // Walk until a vertex repeats; the cycle from that vertex is one loop.
            let mut path = vec![s];
            let mut at = s;
            let mut pos: HashMap<u32, usize> = HashMap::from([(s, 0)]);
            let mut closed = None;
            while let Some(list) = next.get_mut(&at) {
                let Some(to) = list.pop() else { break };
                if let Some(&p) = pos.get(&to) {
                    closed = Some(p);
                    break;
                }
                pos.insert(to, path.len());
                path.push(to);
                at = to;
            }
            match closed {
                Some(p) => {
                    let lp: Vec<u32> = path[p..].to_vec();
                    // Edges before the cycle go back for later walks.
                    for w in path[..=p].windows(2) {
                        next.entry(w[0]).or_default().push(w[1]);
                    }
                    if lp.len() >= 3 {
                        let added = fill_loop(m, &lp);
                        report.holes_filled += 1;
                        report.faces_added += added;
                    }
                }
                None => break,
            }
        }
    }
}

/// Triangulates the hole bounded by `lp` (open edges lp[i] → lp[i+1]); returns the faces
/// added. The fill runs against the loop, so its triangles hold the reversed edges.
fn fill_loop(m: &mut TriangleMesh, lp: &[u32]) -> usize {
    let poly: Vec<u32> = lp.iter().rev().copied().collect();
    let p: Vec<[f64; 3]> = poly
        .iter()
        .map(|&i| m.vertices[i as usize].map(f64::from))
        .collect();
    let n = p.len();
    if n == 3 {
        m.faces.push([poly[0], poly[1], poly[2]]);
        return 1;
    }
    // Newell normal and a plane basis.
    let mut nrm = [0f64; 3];
    for i in 0..n {
        let (a, b) = (p[i], p[(i + 1) % n]);
        nrm[0] += (a[1] - b[1]) * (a[2] + b[2]);
        nrm[1] += (a[2] - b[2]) * (a[0] + b[0]);
        nrm[2] += (a[0] - b[0]) * (a[1] + b[1]);
    }
    let len = (nrm[0] * nrm[0] + nrm[1] * nrm[1] + nrm[2] * nrm[2]).sqrt();
    let added = if len > 1e-12 && n <= 2000 {
        let nz = nrm.map(|c| c / len);
        let helper = if nz[0].abs() < 0.9 {
            [1., 0., 0.]
        } else {
            [0., 1., 0.]
        };
        let cr = |a: [f64; 3], b: [f64; 3]| {
            [
                a[1] * b[2] - a[2] * b[1],
                a[2] * b[0] - a[0] * b[2],
                a[0] * b[1] - a[1] * b[0],
            ]
        };
        let u = cr(helper, nz);
        let ul = (u[0] * u[0] + u[1] * u[1] + u[2] * u[2]).sqrt();
        let u = u.map(|c| c / ul);
        let v = cr(nz, u);
        let q: Vec<[f64; 2]> = p
            .iter()
            .map(|x| {
                [
                    x[0] * u[0] + x[1] * u[1] + x[2] * u[2],
                    x[0] * v[0] + x[1] * v[1] + x[2] * v[2],
                ]
            })
            .collect();
        ear_clip(&q).map(|tris| {
            for t in &tris {
                m.faces.push(t.map(|i| poly[i]));
            }
            tris.len()
        })
    } else {
        None
    };
    added.unwrap_or_else(|| {
        // Centroid fan.
        let mut c = [0f64; 3];
        for x in &p {
            for k in 0..3 {
                c[k] += x[k] / n as f64;
            }
        }
        m.vertices.push(c.map(|x| x as f32));
        let ci = (m.vertices.len() - 1) as u32;
        for i in 0..n {
            m.faces.push([poly[i], poly[(i + 1) % n], ci]);
        }
        n
    })
}

/// Ear clipping of a simple polygon given counter-clockwise; `None` when no ear is found
/// (self-overlapping loop).
fn ear_clip(q: &[[f64; 2]]) -> Option<Vec<[usize; 3]>> {
    let cross = |a: [f64; 2], b: [f64; 2], c: [f64; 2]| {
        (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
    };
    let mut idx: Vec<usize> = (0..q.len()).collect();
    let mut out = Vec::with_capacity(q.len() - 2);
    while idx.len() > 3 {
        let n = idx.len();
        let mut found = None;
        for k in 0..n {
            let (a, b, c) = (idx[(k + n - 1) % n], idx[k], idx[(k + 1) % n]);
            if cross(q[a], q[b], q[c]) <= 0. {
                continue;
            }
            let inside = idx.iter().any(|&o| {
                o != a
                    && o != b
                    && o != c
                    && cross(q[a], q[b], q[o]) >= 0.
                    && cross(q[b], q[c], q[o]) >= 0.
                    && cross(q[c], q[a], q[o]) >= 0.
            });
            if !inside {
                found = Some(k);
                break;
            }
        }
        let k = found?;
        out.push([idx[(k + n - 1) % n], idx[k], idx[(k + 1) % n]]);
        idx.remove(k);
    }
    out.push([idx[0], idx[1], idx[2]]);
    Some(out)
}

/// Turns inside-out parts outward: a part with negative volume that no other part
/// encloses (a cavity inside another part legitimately faces inwards).
fn outward(m: &mut TriangleMesh, report: &mut RepairReport) {
    let parts = parts(m);
    let volume = |faces: &[usize]| {
        faces
            .iter()
            .map(|&i| {
                let [a, b, c] = m.faces[i].map(|v| m.vertices[v as usize].map(f64::from));
                (a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0])
                    + a[2] * (b[0] * c[1] - b[1] * c[0]))
                    / 6.0
            })
            .sum::<f64>()
    };
    let mut to_flip = Vec::new();
    for (pi, part) in parts.iter().enumerate() {
        if volume(part) >= 0. {
            continue;
        }
        let probe = m.vertices[m.faces[part[0]][0] as usize].map(f64::from);
        let nested = parts
            .iter()
            .enumerate()
            .any(|(qi, other)| qi != pi && winding(m, other, probe).abs() > 0.5);
        if !nested {
            to_flip.extend(part.iter().copied());
        }
    }
    for i in to_flip {
        m.faces[i].swap(1, 2);
        report.faces_flipped += 1;
    }
}

/// Connected parts (faces sharing a vertex).
fn parts(m: &TriangleMesh) -> Vec<Vec<usize>> {
    let mut parent: Vec<usize> = (0..m.vertices.len()).collect();
    fn find(p: &mut [usize], mut i: usize) -> usize {
        while p[i] != i {
            p[i] = p[p[i]];
            i = p[i];
        }
        i
    }
    for f in &m.faces {
        for k in 1..3 {
            let (a, b) = (
                find(&mut parent, f[0] as usize),
                find(&mut parent, f[k] as usize),
            );
            if a != b {
                parent[a.max(b)] = a.min(b);
            }
        }
    }
    let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
    for (i, f) in m.faces.iter().enumerate() {
        groups
            .entry(find(&mut parent, f[0] as usize))
            .or_default()
            .push(i);
    }
    let mut out: Vec<Vec<usize>> = groups.into_values().collect();
    out.sort_by_key(|g| g[0]);
    out
}

/// Generalised winding number of `p` with respect to the faces (solid angles / 4π).
fn winding(m: &TriangleMesh, faces: &[usize], p: [f64; 3]) -> f64 {
    let mut total = 0.;
    for &i in faces {
        let [a, b, c] = m.faces[i].map(|v| {
            let q = m.vertices[v as usize];
            [
                f64::from(q[0]) - p[0],
                f64::from(q[1]) - p[1],
                f64::from(q[2]) - p[2],
            ]
        });
        let l = |x: [f64; 3]| (x[0] * x[0] + x[1] * x[1] + x[2] * x[2]).sqrt();
        let d = |x: [f64; 3], y: [f64; 3]| x[0] * y[0] + x[1] * y[1] + x[2] * y[2];
        let det = a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0])
            + a[2] * (b[0] * c[1] - b[1] * c[0]);
        let (la, lb, lc) = (l(a), l(b), l(c));
        let den = la * lb * lc + d(a, b) * lc + d(b, c) * la + d(c, a) * lb;
        total += 2. * det.atan2(den);
    }
    total / (4. * std::f64::consts::PI)
}

/// Drops vertices no face uses (after merges), keeping the order of the rest.
fn compact(m: &mut TriangleMesh) {
    let mut used = vec![false; m.vertices.len()];
    for f in &m.faces {
        for &i in f {
            used[i as usize] = true;
        }
    }
    if used.iter().all(|&u| u) {
        return;
    }
    let mut remap = vec![u32::MAX; m.vertices.len()];
    let mut vertices = Vec::new();
    for (i, v) in m.vertices.iter().enumerate() {
        if used[i] {
            remap[i] = vertices.len() as u32;
            vertices.push(*v);
        }
    }
    m.vertices = vertices;
    for f in &mut m.faces {
        *f = f.map(|i| remap[i as usize]);
    }
}

/// Number of distinct directed edges whose reverse is missing.
pub fn open_edge_count(mesh: &TriangleMesh) -> usize {
    let set: HashSet<(u32, u32)> = open_half_edges(mesh).into_iter().collect();
    set.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh_edit::{cuboid, sphere, write_stl};

    /// A deliberately broken STL: a sphere written as loose triangles (so vertices merge on
    /// load), with one face missing (a hole), three faces flipped, one needle, a duplicated
    /// face, a stray triangle and a crack (a vertex nudged by 2 µm on one side).
    fn broken_stl() -> Vec<u8> {
        let s = sphere(10., 12, 24);
        let mut tris: Vec<[[f32; 3]; 3]> = s
            .faces
            .iter()
            .map(|f| f.map(|i| s.vertices[i as usize]))
            .collect();
        tris.remove(40); // hole
        for i in [5, 77, 150] {
            tris[i].swap(1, 2); // flipped
        }
        let dup = tris[90];
        tris.push(dup); // duplicate
        let a = tris[10][0];
        tris.push([a, a, tris[10][1]]); // needle (collapses on load)
        tris.push([[50., 50., 50.], [51., 50., 50.], [50., 51., 50.]]); // loose
        // Crack: every copy of one vertex in the faces of one side moves by 2 µm.
        let crack = tris[200][0];
        for t in tris.iter_mut().skip(200).take(1) {
            for v in t.iter_mut() {
                if *v == crack {
                    v[0] += 0.002;
                }
            }
        }
        let m = TriangleMesh {
            vertices: tris.iter().flatten().copied().collect(),
            faces: (0..tris.len() as u32)
                .map(|i| [3 * i, 3 * i + 1, 3 * i + 2])
                .collect(),
        };
        let mut out = Vec::new();
        write_stl(&m, "broken", &mut out).unwrap();
        out
    }

    #[test]
    fn repairs_a_broken_stl() {
        let bytes = broken_stl();
        let loaded = TriangleMesh::from_stl_bytes(&bytes).unwrap();
        assert!(!check(&loaded).is_clean());
        let (fixed, report) = repair(&loaded).expect("needs repair");
        assert!(check(&fixed).is_clean(), "{:?} {report}", check(&fixed));
        assert_eq!(report.open_edges_left, 0, "{report}");
        assert_eq!(report.faces_flipped, 3, "{report}");
        assert_eq!(report.holes_filled, 1, "{report}");
        assert!(report.duplicates_removed >= 1, "{report}");
        assert_eq!(report.unconnected_removed, 1, "{report}");
        assert!(report.vertices_merged >= 1, "{report}");
        let exact = 4. / 3. * std::f64::consts::PI * 1000.;
        assert!((fixed.volume() - exact).abs() / exact < 0.05);
        assert!(!report.lines().is_empty());
    }

    #[test]
    fn clean_meshes_are_left_alone() {
        let c = cuboid([10., 20., 30.]);
        assert!(repair(&c).is_none());
        let (same, report) = repair_always(&c);
        assert!(report.is_empty());
        assert_eq!(same.vertices, c.vertices);
        assert_eq!(same.faces, c.faces);
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/models/");
        for name in ["cube20.stl", "hole_block.stl"] {
            let m = TriangleMesh::load_stl(format!("{dir}{name}")).unwrap();
            assert!(repair(&m).is_none(), "{name} should be clean");
        }
    }

    #[test]
    fn inside_out_mesh_turns_outward_but_cavities_stay() {
        let mut c = cuboid([10., 10., 10.]);
        for f in &mut c.faces {
            f.swap(1, 2);
        }
        let (fixed, report) = repair(&c).unwrap();
        assert_eq!(report.faces_flipped, 12);
        assert!(fixed.volume() > 0.);
        // A hollow box (inner shell facing inwards) is clean as it is.
        let mut inner = cuboid([4., 4., 4.]);
        for f in &mut inner.faces {
            f.swap(1, 2);
        }
        let hollow = crate::mesh_edit::merge(&[&cuboid([10., 10., 10.]), &inner]);
        assert!(repair(&hollow).is_none());
    }

    #[test]
    fn open_box_gets_its_lid() {
        let mut c = cuboid([10., 10., 10.]);
        // Remove both triangles of one face.
        c.faces.truncate(10);
        let (fixed, report) = repair(&c).unwrap();
        assert_eq!(report.holes_filled, 1);
        assert_eq!(report.faces_added, 2);
        assert!(check(&fixed).is_clean());
        assert!((fixed.volume() - 1000.).abs() < 1e-3);
    }
}
