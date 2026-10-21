// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Mesh booleans (union, difference, intersection) of two closed triangle meshes, with
//! exact predicates.
//!
//! OrcaSlicer's mesh boolean gizmo calls CGAL or MCUT; neither is pure Rust, and the Rust
//! crates that exist either use float BSP trees (which crack on coplanar and touching
//! input) or wrap C++. This is a self-contained implementation of the classic exact
//! approach (as in Cherchi et al., "Interactive and Robust Mesh Booleans", 2022):
//!
//! 1. Vertices are snapped to an integer grid (2^-16 mm by default, which is lossless for
//!    the f32 coordinates of any printable object), so orientation tests on input vertices
//!    are exact `i128` determinants.
//! 2. Every pair of triangles with overlapping boxes is intersected. Each intersection point
//!    is named by the pair of simplices (vertex, edge or face of each mesh) whose interiors
//!    contain it, decided exactly; the name is canonical, so the two triangles around an
//!    edge, and both meshes, agree on every point and the result has no T-junctions. New
//!    points are line-plane intersections with homogeneous integer coordinates.
//! 3. Each touched triangle is re-triangulated with the points on it and the intersection
//!    segments as constraints ([`triangulate`]), still with exact predicates.
//! 4. The pieces of each mesh are grouped into patches bounded by the intersection curves,
//!    and each patch is classified inside / outside / on the other mesh: locally at an
//!    intersection edge against the other mesh's (one or two) planes there, or by an exact
//!    ray cast when a patch touches nothing.
//! 5. The operation keeps the patches its rule selects (coplanar overlaps once, with the
//!    right orientation), so the result is closed whenever the inputs are.
//!
//! Inputs must be closed and consistently oriented (every edge shared by exactly two
//! triangles, in opposite directions): run [`crate::mesh_repair`] first.

mod exact;
mod triangulate;

use crate::mesh::TriangleMesh;
use exact::{HPoint, P2, Plane, line_line, line_plane, orient2d, orient3d};
use rayon::prelude::*;
use std::collections::{BTreeSet, HashMap};
use std::fmt;

/// The boolean operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BooleanOp {
    /// Everything inside either mesh.
    Union,
    /// The first mesh with the second carved out.
    Difference,
    /// Only what is inside both.
    Intersection,
}

impl BooleanOp {
    pub const ALL: [BooleanOp; 3] = [
        BooleanOp::Union,
        BooleanOp::Difference,
        BooleanOp::Intersection,
    ];
    pub fn label(self) -> &'static str {
        match self {
            BooleanOp::Union => "Union",
            BooleanOp::Difference => "Difference",
            BooleanOp::Intersection => "Intersection",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum BooleanError {
    /// Input `0` (first) or `1` (second) is not a closed, consistently oriented mesh.
    NotClosed(usize),
    /// The inputs are too large for the integer grid.
    TooLarge,
    /// An inconsistency the exact predicates should rule out (self-intersecting input).
    Failed(String),
}

impl fmt::Display for BooleanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BooleanError::NotClosed(i) => write!(
                f,
                "the {} shape has holes or flipped faces; repair it first",
                if *i == 0 { "first" } else { "second" }
            ),
            BooleanError::TooLarge => write!(f, "the shapes are too large"),
            BooleanError::Failed(why) => write!(
                f,
                "the boolean failed ({why}); a shape probably intersects itself"
            ),
        }
    }
}

impl std::error::Error for BooleanError {}

/// `a` op `b`. The result is closed (every directed edge has its reverse) and outward
/// facing; vertices of the inputs keep their exact coordinates.
pub fn boolean(
    a: &TriangleMesh,
    b: &TriangleMesh,
    op: BooleanOp,
) -> Result<TriangleMesh, BooleanError> {
    let ctx = Ctx::new(a, b)?;
    ctx.run(op)
}

/// A feature (vertex, edge as a sorted vertex pair, face) of one of the two meshes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum Feat {
    V(u32),
    E(u32, u32),
    F(u32),
}

fn edge(a: u32, b: u32) -> Feat {
    Feat::E(a.min(b), a.max(b))
}

/// Canonical name of an intersection point: an input vertex, or the pair of features of
/// the first and second mesh whose interiors meet there.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum PKey {
    Vert(u32),
    New(Feat, Feat),
}

fn canon(fa: Feat, fb: Feat) -> PKey {
    match (fa, fb) {
        (Feat::V(g), _) => PKey::Vert(g),
        (_, Feat::V(h)) => PKey::Vert(h),
        _ => PKey::New(fa, fb),
    }
}

/// Where a point lies relative to a triangle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Loc {
    Inside,
    Edge(usize),
    Vertex(usize),
    Outside,
}

/// Classifies from the three edge-side signs (edge i runs from corner i to corner i + 1):
/// all of one sign is inside, zeros put the point on edges or corners.
fn classify(mut o: [i32; 3]) -> Loc {
    if let Some(&s) = o.iter().find(|&&s| s != 0)
        && s < 0
    {
        o = o.map(|x| -x);
    }
    if o.iter().any(|&s| s < 0) {
        return Loc::Outside;
    }
    match o.iter().filter(|&&s| s == 0).count() {
        0 => Loc::Inside,
        1 => Loc::Edge(o.iter().position(|&s| s == 0).unwrap()),
        2 => {
            let nz = o.iter().position(|&s| s != 0).unwrap();
            // Zeros at edges nz + 1 and nz + 2 meet at corner nz + 2.
            Loc::Vertex((nz + 2) % 3)
        }
        _ => Loc::Outside,
    }
}

fn feat_of(loc: Loc, tri: [u32; 3], face: u32) -> Option<Feat> {
    match loc {
        Loc::Inside => Some(Feat::F(face)),
        Loc::Edge(i) => Some(edge(tri[i], tri[(i + 1) % 3])),
        Loc::Vertex(i) => Some(Feat::V(tri[i])),
        Loc::Outside => None,
    }
}

fn proj_int(p: [i64; 3], k: usize) -> P2 {
    P2::project(&HPoint::int(p), k)
}

/// Sub-triangles of one input triangle: (input face, corners).
type Pieces = Vec<(u32, [u32; 3])>;

/// What one pair of triangles contributes.
#[derive(Default)]
struct PairHits {
    /// Intersection points as (feature of A, feature of B).
    hits: Vec<(Feat, Feat)>,
    /// Constraint segments for the A triangle and for the B triangle.
    cons_a: Vec<(PKey, PKey)>,
    cons_b: Vec<(PKey, PKey)>,
    error: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Class {
    In,
    Out,
    OnSame,
    OnOpposite,
}

struct Ctx {
    /// Grid vertices of both meshes (coincident ones merged) and their f32 originals.
    verts: Vec<[i64; 3]>,
    orig: Vec<[f32; 3]>,
    scale: f64,
    faces: [Vec<[u32; 3]>; 2],
    planes: [Vec<Plane>; 2],
}

impl Ctx {
    fn new(a: &TriangleMesh, b: &TriangleMesh) -> Result<Ctx, BooleanError> {
        let max = a
            .vertices
            .iter()
            .chain(&b.vertices)
            .flatten()
            .fold(0f64, |m, &c| m.max(f64::from(c).abs()));
        if !max.is_finite() {
            return Err(BooleanError::TooLarge);
        }
        let limit = (1u64 << (exact::COORD_BITS - 1)) as f64;
        let mut scale = 65536f64;
        while max * scale >= limit {
            scale /= 2.;
            if scale < 1e-3 {
                return Err(BooleanError::TooLarge);
            }
        }
        let mut index: HashMap<[i64; 3], u32> = HashMap::new();
        let mut verts = Vec::new();
        let mut orig = Vec::new();
        let mut faces: [Vec<[u32; 3]>; 2] = [Vec::new(), Vec::new()];
        for (side, mesh) in [a, b].into_iter().enumerate() {
            let ids: Vec<u32> = mesh
                .vertices
                .iter()
                .map(|v| {
                    let g = v.map(|c| (f64::from(c) * scale).round() as i64);
                    *index.entry(g).or_insert_with(|| {
                        verts.push(g);
                        orig.push(*v);
                        (verts.len() - 1) as u32
                    })
                })
                .collect();
            faces[side] = mesh
                .faces
                .iter()
                .map(|f| f.map(|i| ids[i as usize]))
                .collect();
            fix_degenerate(&mut faces[side], &verts).map_err(|_| BooleanError::NotClosed(side))?;
            if !closed_manifold(&faces[side]) {
                return Err(BooleanError::NotClosed(side));
            }
        }
        let planes = [0, 1].map(|s: usize| {
            faces[s]
                .iter()
                .map(|f| {
                    Plane::through(
                        verts[f[0] as usize],
                        verts[f[1] as usize],
                        verts[f[2] as usize],
                    )
                })
                .collect()
        });
        Ok(Ctx {
            verts,
            orig,
            scale,
            faces,
            planes,
        })
    }

    fn p(&self, g: u32) -> [i64; 3] {
        self.verts[g as usize]
    }

    fn run(&self, op: BooleanOp) -> Result<TriangleMesh, BooleanError> {
        // 1. Candidate pairs and their intersections.
        let bvh = Bvh::build(&self.faces[1], &self.verts);
        let results: Vec<(u32, u32, PairHits)> = (0..self.faces[0].len() as u32)
            .into_par_iter()
            .flat_map_iter(|fa| {
                let (lo, hi) = tri_box(self.faces[0][fa as usize], &self.verts);
                let mut found = Vec::new();
                bvh.query(lo, hi, &mut found);
                found.sort_unstable();
                found
                    .into_iter()
                    .filter_map(|fb| self.pair(fa, fb).map(|h| (fa, fb, h)))
                    .collect::<Vec<_>>()
            })
            .collect();
        if let Some((_, _, h)) = results.iter().find(|(_, _, h)| h.error.is_some()) {
            return Err(BooleanError::Failed(h.error.clone().unwrap()));
        }

        // 2. Points: input vertices keep their ids, new points follow in key order.
        let mut new_keys: BTreeSet<PKey> = BTreeSet::new();
        for (_, _, h) in &results {
            for &(fa, fb) in &h.hits {
                if let k @ PKey::New(..) = canon(fa, fb) {
                    new_keys.insert(k);
                }
            }
        }
        let nv = self.verts.len() as u32;
        let new_keys: Vec<PKey> = new_keys.into_iter().collect();
        let mut pts: Vec<HPoint> = self.verts.iter().map(|&v| HPoint::int(v)).collect();
        let computed: Vec<Option<HPoint>> =
            new_keys.par_iter().map(|k| self.point_of(*k)).collect();
        for c in computed {
            pts.push(c.ok_or_else(|| BooleanError::Failed("parallel crossing".into()))?);
        }
        let id_of: HashMap<PKey, u32> = new_keys
            .iter()
            .enumerate()
            .map(|(i, k)| (*k, nv + i as u32))
            .collect();
        let id = |k: PKey| match k {
            PKey::Vert(g) => g,
            k => id_of[&k],
        };

        // 3. Attach points and constraints to the features they lie on.
        let mut face_pts: [HashMap<u32, Vec<u32>>; 2] = Default::default();
        let mut edge_pts: [HashMap<(u32, u32), Vec<u32>>; 2] = Default::default();
        let mut face_cons: [HashMap<u32, Vec<(u32, u32)>>; 2] = Default::default();
        for (fa, fb, h) in &results {
            for &(xa, xb) in &h.hits {
                let p = id(canon(xa, xb));
                for (side, f) in [(0usize, xa), (1, xb)] {
                    match f {
                        Feat::V(_) => {}
                        Feat::E(u, v) => edge_pts[side].entry((u, v)).or_default().push(p),
                        Feat::F(face) => face_pts[side].entry(face).or_default().push(p),
                    }
                }
            }
            for &(x, y) in &h.cons_a {
                face_cons[0].entry(*fa).or_default().push((id(x), id(y)));
            }
            for &(x, y) in &h.cons_b {
                face_cons[1].entry(*fb).or_default().push((id(x), id(y)));
            }
        }

        // 4. Re-triangulate the touched faces.
        let mut subs: [Vec<(u32, [u32; 3])>; 2] = [Vec::new(), Vec::new()];
        for side in 0..2 {
            let pieces: Vec<Result<Pieces, String>> = (0..self.faces[side].len() as u32)
                .into_par_iter()
                .map(|f| {
                    let tri = self.faces[side][f as usize];
                    let mut points: Vec<u32> = face_pts[side].get(&f).cloned().unwrap_or_default();
                    for k in 0..3 {
                        let (u, v) = (tri[k], tri[(k + 1) % 3]);
                        if let Some(list) = edge_pts[side].get(&(u.min(v), u.max(v))) {
                            points.extend(list);
                        }
                    }
                    points.retain(|p| !tri.contains(p));
                    points.sort_unstable();
                    points.dedup();
                    let mut cons: Vec<(u32, u32)> =
                        face_cons[side].get(&f).cloned().unwrap_or_default();
                    cons.retain(|&(x, y)| x != y);
                    for c in &mut cons {
                        *c = (c.0.min(c.1), c.0.max(c.1));
                    }
                    cons.sort_unstable();
                    cons.dedup();
                    let trivial = cons
                        .iter()
                        .all(|&(x, y)| tri.contains(&x) && tri.contains(&y));
                    if points.is_empty() && trivial {
                        return Ok(vec![(f, tri)]);
                    }
                    let plane = &self.planes[side][f as usize];
                    let axis = plane.dominant_axis();
                    let sign = plane.n[axis].signum() as i32;
                    triangulate::subdivide(tri, &points, &cons, axis, sign, &pts)
                        .map(|ts| ts.into_iter().map(|t| (f, t)).collect())
                })
                .collect();
            for p in pieces {
                subs[side].extend(p.map_err(BooleanError::Failed)?);
            }
        }

        // 5. Classify patches and keep what the operation selects.
        let maps: [HashMap<(u32, u32), usize>; 2] = [0, 1].map(|s: usize| {
            let mut m = HashMap::with_capacity(subs[s].len() * 3);
            for (i, (_, t)) in subs[s].iter().enumerate() {
                for k in 0..3 {
                    m.insert((t[k], t[(k + 1) % 3]), i);
                }
            }
            m
        });
        let mut out_faces: Vec<[u32; 3]> = Vec::new();
        for side in 0..2 {
            let classes = self.classify(side, &subs, &maps, &pts)?;
            for ((_, t), c) in subs[side].iter().zip(classes) {
                let keep = match (op, side, c) {
                    (BooleanOp::Union, _, Class::Out) => Some(false),
                    (BooleanOp::Union, 0, Class::OnSame) => Some(false),
                    (BooleanOp::Intersection, _, Class::In) => Some(false),
                    (BooleanOp::Intersection, 0, Class::OnSame) => Some(false),
                    (BooleanOp::Difference, 0, Class::Out | Class::OnOpposite) => Some(false),
                    (BooleanOp::Difference, 1, Class::In) => Some(true),
                    _ => None,
                };
                match keep {
                    Some(false) => out_faces.push(*t),
                    Some(true) => out_faces.push([t[0], t[2], t[1]]),
                    None => {}
                }
            }
        }

        // 6. Compact into an f32 mesh.
        let mut remap: HashMap<u32, u32> = HashMap::new();
        let mut mesh = TriangleMesh::default();
        for f in &out_faces {
            let face = f.map(|p| {
                *remap.entry(p).or_insert_with(|| {
                    mesh.vertices.push(if p < nv {
                        self.orig[p as usize]
                    } else {
                        pts[p as usize].to_f64().map(|c| (c / self.scale) as f32)
                    });
                    (mesh.vertices.len() - 1) as u32
                })
            });
            mesh.faces.push(face);
        }
        Ok(mesh)
    }

    /// Coordinates of a new point from its canonical name.
    fn point_of(&self, k: PKey) -> Option<HPoint> {
        match k {
            PKey::Vert(g) => Some(HPoint::int(self.p(g))),
            PKey::New(Feat::E(p, q), Feat::F(f)) => {
                line_plane(self.p(p), self.p(q), &self.planes[1][f as usize])
            }
            PKey::New(Feat::F(f), Feat::E(p, q)) => {
                line_plane(self.p(p), self.p(q), &self.planes[0][f as usize])
            }
            PKey::New(Feat::E(p, q), Feat::E(r, s)) => {
                line_line(self.p(p), self.p(q), self.p(r), self.p(s))
            }
            PKey::New(..) => None,
        }
    }

    /// Intersects triangle `fa` of the first mesh with triangle `fb` of the second.
    fn pair(&self, fa: u32, fb: u32) -> Option<PairHits> {
        let a = self.faces[0][fa as usize];
        let b = self.faces[1][fb as usize];
        let pa = a.map(|g| self.p(g));
        let pb = b.map(|g| self.p(g));
        let oa = pa.map(|p| orient3d(pb[0], pb[1], pb[2], p));
        if oa.iter().all(|&s| s > 0) || oa.iter().all(|&s| s < 0) {
            return None;
        }
        let ob = pb.map(|p| orient3d(pa[0], pa[1], pa[2], p));
        if ob.iter().all(|&s| s > 0) || ob.iter().all(|&s| s < 0) {
            return None;
        }
        let mut out = PairHits::default();
        if oa == [0, 0, 0] {
            self.coplanar(fa, fb, a, b, pa, pb, &mut out);
        } else {
            // Points of each triangle on the other's plane, located in the other triangle.
            let ka = self.planes[0][fa as usize].dominant_axis();
            let kb = self.planes[1][fb as usize].dominant_axis();
            let locate_vertex = |v: [i64; 3], t: [[i64; 3]; 3], k: usize| {
                let pv = proj_int(v, k);
                let pt = t.map(|p| proj_int(p, k));
                classify([0, 1, 2].map(|i| orient2d(&pt[i], &pt[(i + 1) % 3], &pv)))
            };
            let locate_line = |p: [i64; 3], q: [i64; 3], t: [[i64; 3]; 3]| {
                classify([0, 1, 2].map(|i| orient3d(p, q, t[i], t[(i + 1) % 3])))
            };
            for i in 0..3 {
                let j = (i + 1) % 3;
                if oa[i] == 0
                    && let Some(f) = feat_of(locate_vertex(pa[i], pb, kb), b, fb)
                {
                    out.hits.push((Feat::V(a[i]), f));
                }
                if oa[i] * oa[j] < 0
                    && let Some(f) = feat_of(locate_line(pa[i], pa[j], pb), b, fb)
                {
                    out.hits.push((edge(a[i], a[j]), f));
                }
                if ob[i] == 0
                    && let Some(f) = feat_of(locate_vertex(pb[i], pa, ka), a, fa)
                {
                    out.hits.push((f, Feat::V(b[i])));
                }
                if ob[i] * ob[j] < 0
                    && let Some(f) = feat_of(locate_line(pb[i], pb[j], pa), a, fa)
                {
                    out.hits.push((f, edge(b[i], b[j])));
                }
            }
            let mut keys: Vec<PKey> = out.hits.iter().map(|&(x, y)| canon(x, y)).collect();
            keys.sort_unstable();
            keys.dedup();
            match keys.len() {
                0 => return None,
                1 => {}
                2 => {
                    out.cons_a.push((keys[0], keys[1]));
                    out.cons_b.push((keys[0], keys[1]));
                }
                _ => out.error = Some("more than two points on one crossing".into()),
            }
        }
        (!out.hits.is_empty() || out.error.is_some()).then_some(out)
    }

    #[allow(clippy::too_many_arguments)]
    fn coplanar(
        &self,
        fa: u32,
        fb: u32,
        a: [u32; 3],
        b: [u32; 3],
        pa: [[i64; 3]; 3],
        pb: [[i64; 3]; 3],
        out: &mut PairHits,
    ) {
        let k = self.planes[0][fa as usize].dominant_axis();
        let qa = pa.map(|p| proj_int(p, k));
        let qb = pb.map(|p| proj_int(p, k));
        let locate =
            |v: &P2, t: &[P2; 3]| classify([0, 1, 2].map(|i| orient2d(&t[i], &t[(i + 1) % 3], v)));
        for i in 0..3 {
            if let Some(f) = feat_of(locate(&qa[i], &qb), b, fb) {
                out.hits.push((Feat::V(a[i]), f));
            }
            if let Some(f) = feat_of(locate(&qb[i], &qa), a, fa) {
                out.hits.push((f, Feat::V(b[i])));
            }
        }
        for i in 0..3 {
            let (a0, a1) = (&qa[i], &qa[(i + 1) % 3]);
            for j in 0..3 {
                let (b0, b1) = (&qb[j], &qb[(j + 1) % 3]);
                if orient2d(a0, a1, b0) * orient2d(a0, a1, b1) < 0
                    && orient2d(b0, b1, a0) * orient2d(b0, b1, a1) < 0
                {
                    out.hits
                        .push((edge(a[i], a[(i + 1) % 3]), edge(b[j], b[(j + 1) % 3])));
                }
            }
        }
        // Each edge of one triangle, clipped to the other, constrains the other.
        let on_edge =
            |f: Feat, u: u32, v: u32| f == edge(u, v) || f == Feat::V(u) || f == Feat::V(v);
        for j in 0..3 {
            let (u, v) = (b[j], b[(j + 1) % 3]);
            let mut keys: Vec<PKey> = out
                .hits
                .iter()
                .filter(|&&(_, fb)| on_edge(fb, u, v))
                .map(|&(x, y)| canon(x, y))
                .collect();
            keys.sort_unstable();
            keys.dedup();
            if keys.len() == 2 {
                out.cons_a.push((keys[0], keys[1]));
            } else if keys.len() > 2 {
                out.error = Some("more than two points on a coplanar edge".into());
            }
        }
        for i in 0..3 {
            let (u, v) = (a[i], a[(i + 1) % 3]);
            let mut keys: Vec<PKey> = out
                .hits
                .iter()
                .filter(|&&(fa, _)| on_edge(fa, u, v))
                .map(|&(x, y)| canon(x, y))
                .collect();
            keys.sort_unstable();
            keys.dedup();
            if keys.len() == 2 {
                out.cons_b.push((keys[0], keys[1]));
            } else if keys.len() > 2 {
                out.error = Some("more than two points on a coplanar edge".into());
            }
        }
    }

    /// Inside / outside / on for every piece of mesh `side`, one decision per patch.
    fn classify(
        &self,
        side: usize,
        subs: &[Vec<(u32, [u32; 3])>; 2],
        maps: &[HashMap<(u32, u32), usize>; 2],
        pts: &[HPoint],
    ) -> Result<Vec<Class>, BooleanError> {
        let other = 1 - side;
        let mine = &subs[side];
        let is_cross =
            |x: u32, y: u32| maps[other].contains_key(&(x, y)) || maps[other].contains_key(&(y, x));
        let mut uf = UnionFind::new(mine.len());
        for (i, (_, t)) in mine.iter().enumerate() {
            for k in 0..3 {
                let (x, y) = (t[k], t[(k + 1) % 3]);
                if !is_cross(x, y)
                    && let Some(&j) = maps[side].get(&(y, x))
                {
                    uf.union(i, j);
                }
            }
        }
        let mut patch_class: HashMap<usize, Class> = HashMap::new();
        let mut members: HashMap<usize, Vec<usize>> = HashMap::new();
        for i in 0..mine.len() {
            members.entry(uf.find(i)).or_default().push(i);
        }
        let mut roots: Vec<usize> = members.keys().copied().collect();
        roots.sort_unstable();
        for root in roots {
            let list = &members[&root];
            let mut class = None;
            'search: for &i in list {
                let (orig, t) = mine[i];
                for k in 0..3 {
                    let (x, y) = (t[k], t[(k + 1) % 3]);
                    if is_cross(x, y) {
                        let o = t[(k + 2) % 3];
                        class = Some(self.local_class(side, orig, x, y, o, subs, maps, pts)?);
                        break 'search;
                    }
                }
            }
            let class = match class {
                Some(c) => c,
                None => self.ray_class(side, list.iter().map(|&i| mine[i].1))?,
            };
            patch_class.insert(root, class);
        }
        Ok((0..mine.len()).map(|i| patch_class[&uf.find(i)]).collect())
    }

    /// Class of the piece of `side` with edge x→y on the other mesh's surface and third
    /// vertex `o`, decided against the other mesh's planes around that edge.
    #[allow(clippy::too_many_arguments)]
    fn local_class(
        &self,
        side: usize,
        orig: u32,
        x: u32,
        y: u32,
        o: u32,
        subs: &[Vec<(u32, [u32; 3])>; 2],
        maps: &[HashMap<(u32, u32), usize>; 2],
        pts: &[HPoint],
    ) -> Result<Class, BooleanError> {
        let other = 1 - side;
        let (Some(&t1), Some(&t2)) = (maps[other].get(&(x, y)), maps[other].get(&(y, x))) else {
            return Err(BooleanError::Failed("open intersection edge".into()));
        };
        let b1 = subs[other][t1].0;
        let b2 = subs[other][t2].0;
        let p1 = &self.planes[other][b1 as usize];
        let p2 = &self.planes[other][b2 as usize];
        let n_mine = self.planes[side][orig as usize].n;
        let on = |plane: &Plane| {
            if exact::dot(n_mine, plane.n) > 0 {
                Class::OnSame
            } else {
                Class::OnOpposite
            }
        };
        let po = &pts[o as usize];
        let s1 = p1.side(po);
        // Side of a triangle's corners against a plane: the first that is off it.
        let side_of = |face: u32, plane: &Plane| {
            self.faces[other][face as usize]
                .iter()
                .map(|&g| plane.side(&HPoint::int(self.p(g))))
                .find(|&s| s != 0)
                .unwrap_or(0)
        };
        let c = if b1 == b2 { 0 } else { side_of(b2, p1) };
        if c == 0 {
            // One plane (inside a face, or across a flat edge).
            return Ok(match s1 {
                0 => on(p1),
                s if s < 0 => Class::In,
                _ => Class::Out,
            });
        }
        let s2 = p2.side(po);
        let t1 = side_of(b1, p2);
        if s1 == 0 && s2 == t1 {
            return Ok(on(p1));
        }
        if s2 == 0 && s1 == c {
            return Ok(on(p2));
        }
        let convex = c < 0;
        let inside = if convex {
            s1 < 0 && s2 < 0
        } else {
            s1 < 0 || s2 < 0
        };
        Ok(if inside { Class::In } else { Class::Out })
    }

    /// Class of a patch that touches nothing, by an exact ray cast from the centroid of one
    /// of its pieces with input vertices only.
    fn ray_class(
        &self,
        side: usize,
        pieces: impl Iterator<Item = [u32; 3]>,
    ) -> Result<Class, BooleanError> {
        let other = 1 - side;
        let nv = self.verts.len() as u32;
        const DIRS: [[i128; 3]; 6] = [
            [7, 3, 11],
            [-5, 13, 2],
            [3, -17, 5],
            [11, 7, -13],
            [-19, -2, 7],
            [2, 23, -3],
        ];
        for t in pieces.filter(|t| t.iter().all(|&p| p < nv)).take(8) {
            let c3 = t.iter().fold([0i128; 3], |acc, &g| {
                let p = self.p(g);
                [
                    acc[0] + p[0] as i128,
                    acc[1] + p[1] as i128,
                    acc[2] + p[2] as i128,
                ]
            });
            'dirs: for d in DIRS {
                let mut winding = 0i32;
                for f in &self.faces[other] {
                    let r = f.map(|g| {
                        let p = self.p(g);
                        [
                            3 * p[0] as i128 - c3[0],
                            3 * p[1] as i128 - c3[1],
                            3 * p[2] as i128 - c3[2],
                        ]
                    });
                    let n = exact::cross(exact::sub3(r[1], r[0]), exact::sub3(r[2], r[0]));
                    let num = exact::dot(n, r[0]).signum();
                    let den = exact::dot(n, d).signum();
                    let s = [0, 1, 2].map(|i| {
                        let (u, v) = (r[i], r[(i + 1) % 3]);
                        exact::dot(d, exact::cross(u, v)).signum()
                    });
                    let all_pos = s.iter().all(|&x| x >= 0);
                    let all_neg = s.iter().all(|&x| x <= 0);
                    if num == 0 {
                        // The start point is on this face's plane.
                        let inside = {
                            let k = Plane { n, d: 0 }.dominant_axis();
                            let q = r.map(|p| P2 {
                                u: p[(k + 1) % 3],
                                v: p[(k + 2) % 3],
                                w: 1,
                            });
                            let origin = P2 { u: 0, v: 0, w: 1 };
                            classify([0, 1, 2].map(|i| orient2d(&q[i], &q[(i + 1) % 3], &origin)))
                        };
                        if inside != Loc::Outside {
                            // On the surface: try another piece.
                            continue 'dirs;
                        }
                        continue;
                    }
                    if den == 0 || num != den {
                        continue;
                    }
                    if !(all_pos || all_neg) {
                        continue;
                    }
                    if s.contains(&0) {
                        // Through an edge or corner: another direction.
                        continue 'dirs;
                    }
                    winding += den as i32;
                }
                return Ok(if winding > 0 { Class::In } else { Class::Out });
            }
        }
        Err(BooleanError::Failed(
            "could not classify a detached part".into(),
        ))
    }
}

/// Removes collapsed triangles (two equal corners) and splits zero-area triangles with three
/// distinct collinear corners into their neighbour, keeping the mesh closed.
fn fix_degenerate(faces: &mut Vec<[u32; 3]>, verts: &[[i64; 3]]) -> Result<(), ()> {
    faces.retain(|f| f[0] != f[1] && f[1] != f[2] && f[0] != f[2]);
    let zero = |f: &[u32; 3]| {
        Plane::through(
            verts[f[0] as usize],
            verts[f[1] as usize],
            verts[f[2] as usize],
        )
        .is_degenerate()
    };
    let mut guard = 0;
    while let Some(i) = faces.iter().position(zero) {
        guard += 1;
        if guard > faces.len() + 64 {
            return Err(());
        }
        let f = faces[i];
        // The middle corner lies between the other two on their line.
        let p = f.map(|g| verts[g as usize]);
        let axis = (0..3)
            .max_by_key(|&k| {
                let lo = p.iter().map(|q| q[k]).min().unwrap();
                let hi = p.iter().map(|q| q[k]).max().unwrap();
                hi - lo
            })
            .unwrap();
        let mut order = [0usize, 1, 2];
        order.sort_by_key(|&k| p[k][axis]);
        let m = order[1];
        let (a, c) = (f[(m + 1) % 3], f[(m + 2) % 3]);
        let mid = f[m];
        // f = (mid, a, c) in cyclic order, long edge a→c; the neighbour holds c→a.
        let Some(j) = faces
            .iter()
            .position(|g| (0..3).any(|k| g[k] == c && g[(k + 1) % 3] == a))
        else {
            return Err(());
        };
        let g = faces[j];
        let k = (0..3).find(|&k| g[k] == c).unwrap();
        let x = g[(k + 2) % 3];
        // Neighbour (c, a, x) split at mid on edge c→a.
        let (lo, hi) = (i.min(j), i.max(j));
        faces.swap_remove(hi);
        faces.swap_remove(lo);
        faces.push([c, mid, x]);
        faces.push([mid, a, x]);
        faces.retain(|f| f[0] != f[1] && f[1] != f[2] && f[0] != f[2]);
    }
    Ok(())
}

/// Every directed edge exactly once, each with its reverse.
fn closed_manifold(faces: &[[u32; 3]]) -> bool {
    let mut seen: HashMap<(u32, u32), u32> = HashMap::with_capacity(faces.len() * 3);
    for f in faces {
        for k in 0..3 {
            let e = (f[k], f[(k + 1) % 3]);
            if seen.insert(e, 1).is_some() {
                return false;
            }
        }
    }
    seen.keys().all(|&(u, v)| seen.contains_key(&(v, u)))
}

fn tri_box(f: [u32; 3], verts: &[[i64; 3]]) -> ([i64; 3], [i64; 3]) {
    let mut lo = [i64::MAX; 3];
    let mut hi = [i64::MIN; 3];
    for g in f {
        let p = verts[g as usize];
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    (lo, hi)
}

/// Bounding-volume tree over triangles (closed integer boxes, so touching boxes overlap).
struct Bvh {
    nodes: Vec<Node>,
    items: Vec<u32>,
    boxes: Vec<([i64; 3], [i64; 3])>,
}

struct Node {
    lo: [i64; 3],
    hi: [i64; 3],
    /// Leaf: `items[start..start + count]`; inner (count 0): children `left`, `left + 1`.
    start: u32,
    count: u32,
    left: u32,
}

impl Bvh {
    fn build(faces: &[[u32; 3]], verts: &[[i64; 3]]) -> Bvh {
        let boxes: Vec<_> = faces.iter().map(|&f| tri_box(f, verts)).collect();
        let mut bvh = Bvh {
            nodes: Vec::new(),
            items: (0..faces.len() as u32).collect(),
            boxes,
        };
        if !faces.is_empty() {
            bvh.nodes.push(Node {
                lo: [0; 3],
                hi: [0; 3],
                start: 0,
                count: 0,
                left: 0,
            });
            bvh.split(0, 0, faces.len());
        }
        bvh
    }

    fn split(&mut self, node: usize, start: usize, end: usize) {
        let mut lo = [i64::MAX; 3];
        let mut hi = [i64::MIN; 3];
        for &i in &self.items[start..end] {
            let (a, b) = self.boxes[i as usize];
            for k in 0..3 {
                lo[k] = lo[k].min(a[k]);
                hi[k] = hi[k].max(b[k]);
            }
        }
        self.nodes[node].lo = lo;
        self.nodes[node].hi = hi;
        if end - start <= 8 {
            self.nodes[node].start = start as u32;
            self.nodes[node].count = (end - start) as u32;
            return;
        }
        let axis = (0..3).max_by_key(|&k| hi[k] - lo[k]).unwrap();
        let boxes = &self.boxes;
        self.items[start..end]
            .sort_unstable_by_key(|&i| boxes[i as usize].0[axis] + boxes[i as usize].1[axis]);
        let mid = (start + end) / 2;
        let left = self.nodes.len();
        for _ in 0..2 {
            self.nodes.push(Node {
                lo: [0; 3],
                hi: [0; 3],
                start: 0,
                count: 0,
                left: 0,
            });
        }
        self.nodes[node].left = left as u32;
        self.split(left, start, mid);
        self.split(left + 1, mid, end);
    }

    fn query(&self, lo: [i64; 3], hi: [i64; 3], out: &mut Vec<u32>) {
        if self.nodes.is_empty() {
            return;
        }
        let overlaps = |a: &[i64; 3], b: &[i64; 3]| (0..3).all(|k| a[k] <= hi[k] && lo[k] <= b[k]);
        let mut stack = vec![0usize];
        while let Some(n) = stack.pop() {
            let node = &self.nodes[n];
            if !overlaps(&node.lo, &node.hi) {
                continue;
            }
            if node.count > 0 {
                for &i in &self.items[node.start as usize..(node.start + node.count) as usize] {
                    let (a, b) = &self.boxes[i as usize];
                    if overlaps(a, b) {
                        out.push(i);
                    }
                }
            } else {
                stack.push(node.left as usize);
                stack.push(node.left as usize + 1);
            }
        }
    }
}

struct UnionFind {
    parent: Vec<usize>,
}

impl UnionFind {
    fn new(n: usize) -> Self {
        Self {
            parent: (0..n).collect(),
        }
    }
    fn find(&mut self, mut i: usize) -> usize {
        while self.parent[i] != i {
            self.parent[i] = self.parent[self.parent[i]];
            i = self.parent[i];
        }
        i
    }
    fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            self.parent[ra.max(rb)] = ra.min(rb);
        }
    }
}

#[cfg(test)]
mod tests;
