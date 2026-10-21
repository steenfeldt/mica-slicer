// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: prusaslicer@b028299c77 src/libslic3r/TriangleSelector.cpp :: select_patch, select_triangle_recursive, split_triangle, perform_split, undivide_triangle, remove_useless_children, seed_fill_select_triangles, seed_fill_apply_on_triangles, serialize, deserialize, Cursor, Circle, Sphere, Capsule2D, Capsule3D, DoublePointCursor, test_line_inside_capsule, HeightRange
// Source: prusaslicer@b028299c77 src/libslic3r/TriangleSelector.hpp :: Triangle, TriangleStateType
// Changes: Mica addition: `world_leaves` (bed-space leaves with the original facet they lie in, for the `mica mcp` renderer's facet buffer).
// Changes: Mica addition: a spread limit for smart fill (`fill_within`, `Spread`, `fill_reach`, `geodesic`): an approximate geodesic distance from the clicked point over the fill region (Dijkstra over the region's vertices with a triangle-unfolding update, exact on flat patches) limits how far a fill reaches, splitting the facets on the limit as the brushes do; not in upstream.
// Changes: Rust port for the GUI brushes; the capsule cursors (a stroke segment between two pointer samples) test distances to the axis segment (point-segment and segment-segment, projected on the view plane for the circle) instead of upstream's separate ball, cylinder and rectangle tests; subdivision stops at a budget of added triangles (MAX_SUBDIVISION) and at a split depth of 64 (MAX_SPLIT_DEPTH, the wire format's limit), and a side whose f32 midpoint is one of its ends is not split. Cursor tests run in bed coordinates on a world-space copy of the vertices kept alongside the mesh-local one (no per-cursor transform branch); midpoint vertices are allocated per split and not shared with the neighbour's children (the renderer draws leaves, no T-joint stitching); invalid triangles are compacted by re-serialising; bucket fill, gap filling, clipping planes and the strict T-joint output are left out. Serialisation targets Mica's nibble codes (`FacetStates`), the same wire format.
// Copyright (c) Prusa Research 2019 - 2022 Lukáš Hejl @hejllukas, Vojtěch Bubník @bubnikv, Lukáš Matěna @lukasmatena
// PrusaSlicer is released under the terms of the AGPLv3 or higher.

//! Interactive facet painting: a per-triangle state buffer over a mesh whose triangles are
//! subdivided where a brush edge crosses them, so painted patches follow the brush rather
//! than the mesh's own triangles. The tree round-trips through [`FacetStates`], the 3MF
//! `TriangleSelector` format the engine slices from.

use super::FacetStates;
use crate::mesh::TriangleMesh;

type V3 = [f32; 3];

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn mul(a: V3, s: f32) -> V3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn norm2(a: V3) -> f32 {
    dot(a, a)
}
fn normalize(a: V3) -> V3 {
    let n = norm2(a).sqrt();
    if n > 0. { mul(a, 1. / n) } else { a }
}
fn mid(a: V3, b: V3) -> V3 {
    mul(add(a, b), 0.5)
}

/// Column-major 4 × 4 transform (glam's `to_cols_array_2d`).
pub type Transform = [[f32; 4]; 4];

fn apply(t: &Transform, p: V3) -> V3 {
    std::array::from_fn(|r| t[0][r] * p[0] + t[1][r] * p[1] + t[2][r] * p[2] + t[3][r])
}
fn determinant3(t: &Transform) -> f32 {
    let a = [t[0][0], t[0][1], t[0][2]];
    let b = [t[1][0], t[1][1], t[1][2]];
    let c = [t[2][0], t[2][1], t[2][2]];
    dot(a, cross(b, c))
}

/// Facet states: 0 unpainted, 1 enforcer / tool 1, 2 blocker / tool 2, 3.. further tools.
pub const NONE: u8 = 0;
pub const ENFORCER: u8 = 1;
pub const BLOCKER: u8 = 2;

/// Most triangles brush strokes may add to a mesh by subdividing (about 150 MB with their
/// vertices); see `TriangleSelector::split_triangle`.
pub const MAX_SUBDIVISION: usize = 2_000_000;

/// Deepest split below an original facet, the depth the wire format accepts
/// (`FacetStates::push_codes`). It bounds every recursive walk of the tree: brush strokes on
/// a model scaled 10,000× once nested a single facet 43,000 levels deep and overflowed the
/// stack.
pub const MAX_SPLIT_DEPTH: usize = 64;

#[derive(Clone, Copy, Debug)]
struct Tri {
    verts: [u32; 3],
    source: u32,
    children: [u32; 4],
    /// Number of split sides (0 = leaf).
    splits: u8,
    /// Vertex opposite the split edge (one split) or shared by the two split edges (two).
    side: u8,
    state: u8,
    valid: bool,
    seed: bool,
}
impl Tri {
    fn is_split(&self) -> bool {
        self.splits != 0
    }
    fn child_count(&self) -> usize {
        if self.splits == 0 {
            0
        } else {
            self.splits as usize + 1
        }
    }
}

/// The brush shape: a cylinder along the view direction (what the pointer covers on screen)
/// or a sphere around the hit point (paints through to the back as well).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Brush {
    Circle,
    Sphere,
}

/// A brush placed on the surface, in bed coordinates: one dab around `center`, or with
/// `second` a capsule swept from `center` to `second` (upstream's `DoublePointCursor`:
/// `Capsule2D` for the circle, `Capsule3D` for the sphere), so a fast drag paints the whole
/// segment between two pointer samples instead of two separate dabs.
#[derive(Clone, Copy, Debug)]
pub struct Cursor {
    pub brush: Brush,
    /// The hit point on the surface (a capsule's first end).
    pub center: V3,
    /// A capsule's second end.
    pub second: Option<V3>,
    /// Unit direction from the camera to the hit point.
    pub dir: V3,
    pub radius: f32,
}

impl Cursor {
    pub fn new(brush: Brush, center: V3, camera: V3, radius: f32) -> Self {
        Self {
            brush,
            center,
            second: None,
            dir: normalize(sub(center, camera)),
            radius: radius.max(1e-3),
        }
    }
    /// A capsule from `first` to `second` seen from `camera` (the view direction is taken at
    /// `first`, as upstream). Coincident ends give a plain dab (upstream asserts they differ).
    pub fn capsule(brush: Brush, first: V3, second: V3, camera: V3, radius: f32) -> Self {
        let mut c = Self::new(brush, first, camera, radius);
        if norm2(sub(second, first)) > 1e-12 {
            c.second = Some(second);
        }
        c
    }
    /// Edges shorter than this are not split further (a fifth of the radius, as upstream).
    fn edge_limit(&self) -> f32 {
        self.radius / 5.
    }
    /// `p` without its component along the view (the circle brush works on the screen plane).
    fn flat(&self, p: V3) -> V3 {
        sub(p, mul(self.dir, dot(p, self.dir)))
    }
    fn point_inside(&self, p: V3) -> bool {
        let r2 = self.radius * self.radius;
        if let Some(q) = self.second {
            // Distance to the axis segment: the end balls (discs on screen) and the cylinder
            // (band) between them, upstream's two-part Capsule3D / Capsule2D tests in one.
            return match self.brush {
                Brush::Sphere => point_segment_dist2(p, self.center, q) < r2,
                Brush::Circle => {
                    point_segment_dist2(self.flat(p), self.flat(self.center), self.flat(q)) < r2
                }
            };
        }
        let diff = sub(self.center, p);
        let d2 = match self.brush {
            Brush::Sphere => norm2(diff),
            Brush::Circle => norm2(sub(diff, mul(self.dir, dot(diff, self.dir)))),
        };
        d2 < r2
    }
    /// Whether the pointer's line (through the centre along the view) pierces the triangle;
    /// for a capsule, the line through either end.
    fn pointer_in_triangle(&self, p: [V3; 3]) -> bool {
        circle_pointer_in_triangle(self.center, self.dir, p)
            || self
                .second
                .is_some_and(|q| circle_pointer_in_triangle(q, self.dir, p))
    }
    fn any_edge_inside(&self, p: [V3; 3]) -> bool {
        if let Some(q) = self.second {
            let r2 = self.radius * self.radius;
            return (0..3).any(|side| {
                let a = p[side];
                let b = p[(side + 1) % 3];
                match self.brush {
                    Brush::Sphere => segment_segment_dist2(a, b, self.center, q) < r2,
                    Brush::Circle => {
                        let (a, b) = (self.flat(a), self.flat(b));
                        segment_segment_dist2(a, b, self.flat(self.center), self.flat(q)) < r2
                    }
                }
            });
        }
        (0..3).any(|side| {
            let a = p[side];
            let b = p[(side + 1) % 3];
            match self.brush {
                Brush::Sphere => line_inside_sphere(a, b, self.center, self.radius),
                Brush::Circle => {
                    let s = normalize(sub(b, a));
                    let t = dot(sub(self.center, a), s);
                    let v = sub(add(a, mul(s, t)), self.center);
                    let d2 = norm2(v) - dot(v, self.dir).powi(2);
                    d2 < self.radius * self.radius && t >= 0. && t <= norm2(sub(b, a)).sqrt()
                }
            }
        })
    }
    /// A circle paints only faces turned towards the camera; a sphere paints through.
    fn facet_visible(&self, normal: V3) -> bool {
        self.brush == Brush::Sphere || dot(normal, self.dir) < 0.
    }
}

/// Whether the line through `center` along `dir` pierces the triangle (upstream's
/// `is_circle_pointer_inside_triangle`).
fn circle_pointer_in_triangle(center: V3, dir: V3, p: [V3; 3]) -> bool {
    let q1 = add(center, dir);
    let q2 = sub(center, dir);
    let sign = |a: V3, b: V3, c: V3, d: V3| dot(cross(sub(b, a), sub(c, a)), sub(d, a)) > 0.;
    if sign(q1, p[0], p[1], p[2]) == sign(q2, p[0], p[1], p[2]) {
        return false;
    }
    let pos = sign(q1, q2, p[0], p[1]);
    sign(q1, q2, p[1], p[2]) == pos && sign(q1, q2, p[2], p[0]) == pos
}

/// Squared distance from `p` to the segment `a`–`b`.
fn point_segment_dist2(p: V3, a: V3, b: V3) -> f32 {
    let ab = sub(b, a);
    let len2 = norm2(ab);
    let t = if len2 > 0. {
        (dot(sub(p, a), ab) / len2).clamp(0., 1.)
    } else {
        0.
    };
    norm2(sub(p, add(a, mul(ab, t))))
}

/// Squared distance between the segments `p1`–`q1` and `p2`–`q2` (Ericson, Real-Time
/// Collision Detection 5.1.9). An edge touches a capsule when it comes closer to the axis
/// than the radius, which covers upstream's sphere and cylinder tests
/// (`test_line_inside_capsule`, and the edge-through-rectangle checks of `Capsule2D`).
fn segment_segment_dist2(p1: V3, q1: V3, p2: V3, q2: V3) -> f32 {
    let d1 = sub(q1, p1);
    let d2 = sub(q2, p2);
    let r = sub(p1, p2);
    let a = norm2(d1);
    let e = norm2(d2);
    let f = dot(d2, r);
    let eps = 1e-12;
    let (s, t) = if a <= eps && e <= eps {
        (0., 0.)
    } else if a <= eps {
        (0., (f / e).clamp(0., 1.))
    } else {
        let c = dot(d1, r);
        if e <= eps {
            ((-c / a).clamp(0., 1.), 0.)
        } else {
            let b = dot(d1, d2);
            let denom = a * e - b * b;
            let mut s = if denom > eps {
                ((b * f - c * e) / denom).clamp(0., 1.)
            } else {
                0.
            };
            let mut t = (b * s + f) / e;
            if t < 0. {
                t = 0.;
                s = (-c / a).clamp(0., 1.);
            } else if t > 1. {
                t = 1.;
                s = ((b - c) / a).clamp(0., 1.);
            }
            (s, t)
        }
    };
    norm2(sub(add(p1, mul(d1, s)), add(p2, mul(d2, t))))
}

/// Whole line inside or crossing a sphere (Ericson, Real-Time Collision Detection 5.3.2).
fn line_inside_sphere(a: V3, b: V3, c: V3, r: f32) -> bool {
    let r2 = r * r;
    let n = sub(b, a);
    let m = sub(a, c);
    let mm = dot(m, m);
    if mm <= r2 || norm2(sub(b, c)) <= r2 {
        return true;
    }
    let nn = dot(n, n);
    let mn = dot(m, n);
    let discr = mn * mn - nn * (mm - r2);
    if discr < 0. || nn <= 0. {
        return false;
    }
    let s = discr.sqrt();
    let t1 = (-mn - s) / nn;
    if (0. ..=1.).contains(&t1) {
        return true;
    }
    let t2 = (-mn + s) / nn;
    (0. ..=1.).contains(&t2) && s > 0.
}

/// A horizontal band of the model (height-range painting), in bed coordinates.
#[derive(Clone, Copy, Debug)]
pub struct HeightRange {
    pub bottom: f32,
    pub top: f32,
}
impl HeightRange {
    pub fn around(z: f32, height: f32) -> Self {
        Self {
            bottom: z - height / 2.,
            top: z + height / 2.,
        }
    }
    fn point_inside(&self, p: V3) -> bool {
        p[2] >= self.bottom && p[2] <= self.top
    }
    fn any_edge_inside(&self, p: [V3; 3]) -> bool {
        !(p.iter().all(|v| v[2] < self.bottom) || p.iter().all(|v| v[2] > self.top))
    }
}

/// How far a smart fill may spread from where it started, measured along the surface.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spread {
    /// The clicked point, in bed coordinates, on the start facet.
    pub origin: V3,
    /// Largest distance along the surface, mm; zero or less means no limit.
    pub max_distance: f32,
}

impl Spread {
    fn limit(self) -> Option<Self> {
        (self.max_distance.is_finite() && self.max_distance > 0.).then_some(self)
    }
}

/// A source of distance in one facet's plane: the surface distance of a point `p` in the
/// facet is `|p - at| + offset`, valid where the line from `at` to `p` crosses `through`
/// (the facet edge a virtual source was unfolded over), if any.
#[derive(Clone, Copy, Debug)]
struct Source {
    at: V3,
    offset: f32,
    through: Option<(V3, V3)>,
}

/// The surface distance field over one facet (see [`Geodesic`]).
struct FacetField {
    sources: Vec<Source>,
    max: f32,
}

impl FacetField {
    fn point(&self, p: V3) -> f32 {
        let mut best = f32::INFINITY;
        for s in &self.sources {
            if let Some((a, b)) = s.through
                && !crosses(s.at, p, a, b)
            {
                continue;
            }
            best = best.min(norm2(sub(p, s.at)).sqrt() + s.offset);
        }
        best
    }
    /// A lower bound of the distance over the triangle (sources skip their edge test).
    fn lower_bound(&self, t: [V3; 3]) -> f32 {
        self.sources
            .iter()
            .map(|s| point_triangle_dist2(s.at, t).sqrt() + s.offset)
            .fold(f32::INFINITY, f32::min)
    }
    fn point_inside(&self, p: V3) -> bool {
        self.point(p) <= self.max
    }
}

/// Whether the segment `s`-`p` crosses the segment `a`-`b` (all in one plane), with a little
/// slack at the ends so a line through a vertex counts.
fn crosses(s: V3, p: V3, a: V3, b: V3) -> bool {
    let e = sub(b, a);
    let d = sub(p, s);
    let n = cross(e, d);
    let nn = norm2(n);
    if nn < 1e-20 {
        return false;
    }
    // s + d t = a + e u: crossing d t - e u = a - s with e and with d.
    let w = sub(a, s);
    let t = -dot(cross(w, e), n) / nn;
    let u = -dot(cross(w, d), n) / nn;
    let slack = 1e-3;
    (-slack..=1. + slack).contains(&t) && (-slack..=1. + slack).contains(&u)
}

/// Squared distance from `p` to the triangle `t`.
fn point_triangle_dist2(p: V3, t: [V3; 3]) -> f32 {
    let n = cross(sub(t[1], t[0]), sub(t[2], t[0]));
    let nn = norm2(n);
    if nn > 1e-20 {
        // Over the triangle: the distance to its plane.
        let inside = (0..3).all(|k| {
            let (u, v) = (t[k], t[(k + 1) % 3]);
            dot(cross(sub(v, u), sub(p, u)), n) >= 0.
        });
        if inside {
            return dot(sub(p, t[0]), n).powi(2) / nn;
        }
    }
    (0..3)
        .map(|k| point_segment_dist2(p, t[k], t[(k + 1) % 3]))
        .fold(f32::INFINITY, f32::min)
}

/// The virtual source of a wave that reached `a` at `da` and `b` at `db`, in the plane of the
/// triangle (`a`, `b`, `c`) on the far side of `a`-`b` from `c`: the facets the wave came
/// over, unfolded into this one's plane.
fn unfold(a: V3, b: V3, c: V3, da: f32, db: f32) -> Option<V3> {
    let e = sub(b, a);
    let len = norm2(e).sqrt();
    if !(len > 1e-9 && da.is_finite() && db.is_finite()) {
        return None;
    }
    let u = mul(e, 1. / len);
    let ac = sub(c, a);
    let perp = sub(ac, mul(u, dot(ac, u)));
    let h = norm2(perp).sqrt();
    if h <= 1e-9 || h.is_nan() {
        return None;
    }
    let away = mul(perp, -1. / h);
    let x = (da * da - db * db + len * len) / (2. * len);
    let y2 = da * da - x * x;
    // A source on the edge's line (the click on a facet edge) can come out a rounding error
    // below zero.
    if y2 < -1e-4 * da * da {
        return None;
    }
    Some(add(add(a, mul(u, x)), mul(away, y2.max(0.).sqrt())))
}

/// Approximate surface (geodesic) distances from a point over a set of facets.
pub struct Geodesic {
    /// Per original vertex (`INFINITY` where the walk did not reach).
    dist: Vec<f32>,
    start: usize,
    origin: V3,
}

impl Geodesic {
    /// The distance field over one facet of `sel`: the origin (on the start facet), the
    /// facet's vertices at their distances, and per edge the source unfolded over it.
    fn facet(&self, sel: &TriangleSelector, facet: usize, max: f32) -> FacetField {
        let v = sel.tris[facet].verts;
        let p = v.map(|i| sel.world[i as usize]);
        let d = v.map(|i| self.dist[i as usize]);
        let mut sources = Vec::with_capacity(7);
        if facet == self.start {
            sources.push(Source {
                at: self.origin,
                offset: 0.,
                through: None,
            });
        }
        for k in 0..3 {
            if d[k].is_finite() {
                sources.push(Source {
                    at: p[k],
                    offset: d[k],
                    through: None,
                });
            }
            let (i, j, o) = (k, (k + 1) % 3, (k + 2) % 3);
            if let Some(s) = unfold(p[i], p[j], p[o], d[i], d[j]) {
                sources.push(Source {
                    at: s,
                    offset: 0.,
                    through: Some((p[i], p[j])),
                });
            }
        }
        FacetField { sources, max }
    }

    /// The surface distance of a point `p` (bed coordinates) on `facet`.
    pub fn at(&self, sel: &TriangleSelector, facet: usize, p: V3) -> f32 {
        self.facet(sel, facet, f32::INFINITY).point(p)
    }
}

/// What decides whether a (sub)triangle is inside, crossed or outside the painted region.
enum Shape<'a> {
    Cursor(&'a Cursor),
    Band(&'a HeightRange),
    /// A fill's reach within one facet.
    Spread(&'a FacetField),
}
impl Shape<'_> {
    fn point_inside(&self, p: V3) -> bool {
        match self {
            Self::Cursor(c) => c.point_inside(p),
            Self::Band(b) => b.point_inside(p),
            Self::Spread(f) => f.point_inside(p),
        }
    }
    fn pointer_in_triangle(&self, p: [V3; 3]) -> bool {
        match self {
            Self::Cursor(c) => c.pointer_in_triangle(p),
            Self::Band(_) | Self::Spread(_) => false,
        }
    }
    fn any_edge_inside(&self, p: [V3; 3]) -> bool {
        match self {
            Self::Cursor(c) => c.any_edge_inside(p),
            Self::Band(b) => b.any_edge_inside(p),
            Self::Spread(f) => f.lower_bound(p) <= f.max,
        }
    }
    fn edge_limit(&self) -> f32 {
        match self {
            Self::Cursor(c) => c.edge_limit(),
            // Upstream splits down to 0.1 mm along a height-range boundary.
            Self::Band(_) => 0.1,
            Self::Spread(f) => spread_edge_limit(f.max),
        }
    }
}

/// Facets on a spread limit are split down to a fifteenth of the distance (a brush splits to
/// a fifth of its radius), between 0.1 and 1 mm.
fn spread_edge_limit(max: f32) -> f32 {
    (max / 15.).clamp(0.1, 1.)
}

/// The painting state of one mesh: original triangles plus the subdivisions painting made.
#[derive(Clone)]
pub struct TriangleSelector {
    /// Mesh-local and bed-space positions, index-aligned (midpoints are appended to both).
    local: Vec<V3>,
    world: Vec<V3>,
    tris: Vec<Tri>,
    /// Neighbour across each edge of every original triangle (`-1` at a boundary).
    neighbors: Vec<[i32; 3]>,
    /// Outward unit normals of the original triangles in bed space.
    normals: Vec<V3>,
    orig_tris: usize,
    orig_verts: usize,
    edge_limit_sqr: f32,
    invalid: usize,
    /// The facet streams this selector loads and saves use the swapped winding (b, c).
    reversed: bool,
}

impl TriangleSelector {
    /// `transform` places the mesh on the bed; `reversed` matches the facet streams' winding
    /// flag (see [`FacetStates::reversed`]).
    pub fn new(mesh: &TriangleMesh, transform: &Transform, reversed: bool) -> Self {
        let local: Vec<V3> = mesh.vertices.clone();
        let world: Vec<V3> = local.iter().map(|&p| apply(transform, p)).collect();
        let flip = determinant3(transform) < 0.;
        let mut tris = Vec::with_capacity(mesh.faces.len());
        let mut normals = Vec::with_capacity(mesh.faces.len());
        for (i, f) in mesh.faces.iter().enumerate() {
            let verts = if reversed { [f[0], f[2], f[1]] } else { *f };
            tris.push(Tri {
                verts,
                source: i as u32,
                children: [0; 4],
                splits: 0,
                side: 0,
                state: NONE,
                valid: true,
                seed: false,
            });
            let [a, b, c] = f.map(|v| world[v as usize]);
            let mut n = normalize(cross(sub(b, a), sub(c, a)));
            if flip {
                n = mul(n, -1.);
            }
            normals.push(n);
        }
        Self {
            neighbors: face_neighbors(mesh),
            local,
            world,
            tris,
            normals,
            orig_tris: mesh.faces.len(),
            orig_verts: mesh.vertices.len(),
            edge_limit_sqr: 1.,
            invalid: 0,
            reversed,
        }
    }

    pub fn reversed(&self) -> bool {
        self.reversed
    }
    pub fn facet_count(&self) -> usize {
        self.orig_tris
    }
    /// Bed-space outward normal of an original triangle.
    pub fn normal(&self, facet: usize) -> V3 {
        self.normals[facet]
    }

    /// Drops every split and state.
    pub fn clear(&mut self) {
        self.tris.truncate(self.orig_tris);
        self.local.truncate(self.orig_verts);
        self.world.truncate(self.orig_verts);
        for t in &mut self.tris {
            t.splits = 0;
            t.state = NONE;
            t.valid = true;
            t.seed = false;
        }
        self.invalid = 0;
    }

    /// Whether any leaf carries a state other than `NONE`.
    pub fn has_painting(&self) -> bool {
        self.tris
            .iter()
            .any(|t| t.valid && !t.is_split() && t.state != NONE)
    }

    /// Leaf triangles with their state, in mesh-local coordinates (`include_none` adds the
    /// unpainted leaves), in the mesh's own winding.
    pub fn leaves(&self, include_none: bool, mut f: impl FnMut(u8, [V3; 3])) {
        for t in &self.tris {
            if t.valid && !t.is_split() && (include_none || t.state != NONE) {
                let mut tri = t.verts.map(|v| self.local[v as usize]);
                if self.reversed {
                    tri.swap(1, 2);
                }
                f(t.state, tri);
            }
        }
    }

    /// Leaf triangles in bed coordinates with the original facet each one lies in and its
    /// state (`include_none` adds the unpainted leaves), for drawing and picking.
    pub fn world_leaves(&self, include_none: bool, mut f: impl FnMut(usize, u8, [V3; 3])) {
        for t in &self.tris {
            if t.valid && !t.is_split() && (include_none || t.state != NONE) {
                let mut tri = t.verts.map(|v| self.world[v as usize]);
                if self.reversed {
                    tri.swap(1, 2);
                }
                f(t.source as usize, t.state, tri);
            }
        }
    }

    // ----- painting ----------------------------------------------------------------------

    /// Paints everything the brush covers around the facet under the pointer: a breadth-first
    /// walk over neighbouring facets, splitting triangles the brush edge crosses when
    /// `splitting` is on. With `overhang_cos`, only facets whose bed-space normal points down
    /// at least that steeply (`-n.z >= cos`) take paint.
    pub fn paint(
        &mut self,
        facet_start: usize,
        cursor: &Cursor,
        state: u8,
        splitting: bool,
        overhang_cos: Option<f32>,
    ) {
        if facet_start >= self.orig_tris {
            return;
        }
        let shape = Shape::Cursor(cursor);
        self.edge_limit_sqr = shape.edge_limit() * shape.edge_limit();
        let mut queue = vec![facet_start];
        let mut visited = vec![false; self.orig_tris];
        let mut head = 0;
        while head < queue.len() {
            let facet = queue[head];
            head += 1;
            if visited[facet] {
                continue;
            }
            visited[facet] = true;
            let steep = overhang_cos.is_none_or(|c| -self.normals[facet][2] >= c);
            if steep && self.select_triangle(facet, &shape, state, splitting) {
                for k in 0..3 {
                    let n = self.neighbors[facet][k];
                    if n >= 0 && cursor.facet_visible(self.normals[n as usize]) {
                        queue.push(n as usize);
                    }
                }
            }
        }
        self.maybe_compact();
    }

    /// Paints every facet within a horizontal band of the bed (height-range brush).
    pub fn paint_height_range(&mut self, range: &HeightRange, state: u8) {
        let shape = Shape::Band(range);
        self.edge_limit_sqr = shape.edge_limit() * shape.edge_limit();
        let side = |z: f32| {
            if z < range.bottom {
                -1
            } else if z > range.top {
                1
            } else {
                0
            }
        };
        for facet in 0..self.orig_tris {
            let s = self.tris[facet]
                .verts
                .map(|v| side(self.world[v as usize][2]));
            if s[0] * s[1] <= 0 || s[1] * s[2] <= 0 || s[0] * s[2] <= 0 {
                self.select_triangle(facet, &shape, state, true);
            }
        }
        self.maybe_compact();
    }

    /// Smart fill: paints the facet under the pointer and every neighbour reachable over
    /// edges whose facets differ by at most `angle_deg`, whole triangles only (split
    /// children of a reached facet follow their parent).
    pub fn fill(
        &mut self,
        facet_start: usize,
        angle_deg: f32,
        state: u8,
        overhang_cos: Option<f32>,
    ) {
        for facet in self.fill_region(facet_start, angle_deg, overhang_cos) {
            self.set_facet(facet, state);
        }
    }

    /// Smart fill with a spread limit: as [`Self::fill`], but only as far as `spread` allows
    /// from its origin, measured along the filled surface. Facets wholly within reach take the
    /// state whole; facets the limit crosses are split along it (to a fifteenth of the
    /// distance, 0.1-1 mm). Without a limit (`None`, or a distance of zero) this is `fill`.
    pub fn fill_within(
        &mut self,
        facet_start: usize,
        angle_deg: f32,
        state: u8,
        overhang_cos: Option<f32>,
        spread: Option<Spread>,
    ) {
        let Some(spread) = spread.and_then(Spread::limit) else {
            self.fill(facet_start, angle_deg, state, overhang_cos);
            return;
        };
        let region = self.fill_region(facet_start, angle_deg, overhang_cos);
        if region.is_empty() {
            return;
        }
        let max = spread.max_distance;
        let field = self.geodesic(&region, facet_start, spread.origin, max);
        self.edge_limit_sqr = spread_edge_limit(max).powi(2);
        for facet in region {
            let f = field.facet(self, facet, max);
            let pts = self.world_tri(facet);
            if pts.iter().all(|&p| f.point_inside(p)) {
                self.set_facet(facet, state);
            } else if f.lower_bound(pts) <= max {
                self.select_triangle(facet, &Shape::Spread(&f), state, true);
            }
        }
        self.maybe_compact();
    }

    /// How far a smart fill from `origin` on `facet_start` reaches: per facet of its region,
    /// the surface distance of the facet's centroid and the facet's area (mm, mm²), nearest
    /// first. The running sum of the areas tells which spread limit covers how much.
    pub fn fill_reach(
        &self,
        facet_start: usize,
        angle_deg: f32,
        overhang_cos: Option<f32>,
        origin: V3,
    ) -> Vec<(f32, f32)> {
        let region = self.fill_region(facet_start, angle_deg, overhang_cos);
        if region.is_empty() {
            return vec![];
        }
        let field = self.geodesic(&region, facet_start, origin, f32::INFINITY);
        let mut out: Vec<(f32, f32)> = region
            .iter()
            .map(|&f| {
                let t = self.world_tri(f);
                let c = mul(add(add(t[0], t[1]), t[2]), 1. / 3.);
                let area = 0.5 * norm2(cross(sub(t[1], t[0]), sub(t[2], t[0]))).sqrt();
                (field.at(self, f, c), area)
            })
            .collect();
        out.sort_by(|a, b| a.0.total_cmp(&b.0));
        out
    }

    /// An unpainted selector over the same mesh and placement (previews and dry runs).
    pub fn blank(&self) -> Self {
        let mut out = Self {
            local: self.local[..self.orig_verts].to_vec(),
            world: self.world[..self.orig_verts].to_vec(),
            tris: self.tris[..self.orig_tris].to_vec(),
            neighbors: self.neighbors.clone(),
            normals: self.normals.clone(),
            orig_tris: self.orig_tris,
            orig_verts: self.orig_verts,
            edge_limit_sqr: 1.,
            invalid: 0,
            reversed: self.reversed,
        };
        out.clear();
        out
    }

    /// Approximate surface distances from `origin` (on facet `start`) over `region`'s facets:
    /// a Dijkstra walk over the region's vertices, each step taking the shorter of the edge
    /// and the straight line from the facet's unfolded virtual source (exact over flat
    /// patches, and across folds the wave crosses straight). Stops a longest edge past `max`,
    /// so the facets on the limit have their far vertices too (on a coarse mesh every vertex
    /// may lie beyond the limit while the limit runs through the facets).
    pub fn geodesic(&self, region: &[usize], start: usize, origin: V3, max: f32) -> Geodesic {
        use std::cmp::Reverse;
        use std::collections::BinaryHeap;
        let mut dist = vec![f32::INFINITY; self.orig_verts];
        // Vertex -> region facets, as sorted pairs.
        let mut incident: Vec<(u32, u32)> = Vec::with_capacity(region.len() * 3);
        let mut longest = 0f32;
        for &f in region {
            let t = self.world_tri(f);
            for k in 0..3 {
                longest = longest.max(norm2(sub(t[k], t[(k + 1) % 3])));
            }
            for v in self.tris[f].verts {
                incident.push((v, f as u32));
            }
        }
        let stop = max + longest.sqrt();
        incident.sort_unstable();
        let mut done = vec![false; self.orig_verts];
        // Non-negative f32 distances order like their bit patterns.
        let mut heap = BinaryHeap::new();
        if start < self.orig_tris {
            for v in self.tris[start].verts {
                let d = norm2(sub(self.world[v as usize], origin)).sqrt();
                if d < dist[v as usize] {
                    dist[v as usize] = d;
                    heap.push(Reverse((d.to_bits(), v)));
                }
            }
        }
        while let Some(Reverse((bits, a))) = heap.pop() {
            let da = f32::from_bits(bits);
            if done[a as usize] || da > dist[a as usize] {
                continue;
            }
            done[a as usize] = true;
            if da > stop {
                break;
            }
            let pa = self.world[a as usize];
            let lo = incident.partition_point(|e| e.0 < a);
            let hi = incident.partition_point(|e| e.0 <= a);
            for &(_, f) in &incident[lo..hi] {
                let verts = self.tris[f as usize].verts;
                for &x in &verts {
                    if x == a || done[x as usize] {
                        continue;
                    }
                    let px = self.world[x as usize];
                    let mut d = da + norm2(sub(px, pa)).sqrt();
                    // Over the facet's third vertex y: unfold the wave over the edge a-y.
                    if let Some(y) = verts.iter().copied().find(|&y| y != a && y != x)
                        && done[y as usize]
                    {
                        let py = self.world[y as usize];
                        if let Some(s) = unfold(pa, py, px, da, dist[y as usize])
                            && crosses(s, px, pa, py)
                        {
                            d = d.min(norm2(sub(px, s)).sqrt());
                        }
                    }
                    if d < dist[x as usize] {
                        dist[x as usize] = d;
                        heap.push(Reverse((d.to_bits(), x)));
                    }
                }
            }
        }
        Geodesic {
            dist,
            start,
            origin,
        }
    }

    /// The facets a smart fill from `facet_start` reaches: neighbours over edges whose
    /// facets differ by at most `angle_deg`, through facets the overhang filter accepts.
    pub fn fill_region(
        &self,
        facet_start: usize,
        angle_deg: f32,
        overhang_cos: Option<f32>,
    ) -> Vec<usize> {
        if facet_start >= self.orig_tris {
            return vec![];
        }
        let limit = angle_deg.to_radians().cos() - 1e-6;
        let mut visited = vec![false; self.orig_tris];
        let mut queue = vec![facet_start];
        let mut head = 0;
        let mut chosen = vec![];
        while head < queue.len() {
            let facet = queue[head];
            head += 1;
            if visited[facet] {
                continue;
            }
            visited[facet] = true;
            if overhang_cos.is_some_and(|c| -self.normals[facet][2] < c) {
                continue;
            }
            chosen.push(facet);
            for k in 0..3 {
                let n = self.neighbors[facet][k];
                if n >= 0 && !visited[n as usize] {
                    let d = dot(self.normals[facet], self.normals[n as usize]).clamp(0., 1.);
                    if d >= limit {
                        queue.push(n as usize);
                    }
                }
            }
        }
        chosen
    }

    /// Sets an original facet as a whole (any subdivision is dropped).
    pub fn set_facet(&mut self, facet: usize, state: u8) {
        if facet < self.orig_tris {
            self.undivide(facet);
            self.tris[facet].state = state;
        }
    }

    /// Paints every facet steeper than the overhang threshold (`-n.z >= cos`) that does not
    /// lie on the bed (all vertices at or below `bed_z`): "Paint overhangs as enforcers".
    pub fn paint_overhangs(&mut self, overhang_cos: f32, bed_z: f32, state: u8) -> usize {
        let mut count = 0;
        for facet in 0..self.orig_tris {
            if -self.normals[facet][2] >= overhang_cos
                && !self.tris[facet]
                    .verts
                    .iter()
                    .all(|&v| self.world[v as usize][2] <= bed_z + 1e-3)
            {
                self.set_facet(facet, state);
                count += 1;
            }
        }
        count
    }

    /// Facets steeper than the threshold, for highlighting (bed-space test as above).
    pub fn overhang_facets(&self, overhang_cos: f32, bed_z: f32) -> Vec<usize> {
        (0..self.orig_tris)
            .filter(|&facet| {
                -self.normals[facet][2] >= overhang_cos
                    && !self.tris[facet]
                        .verts
                        .iter()
                        .all(|&v| self.world[v as usize][2] <= bed_z + 1e-3)
            })
            .collect()
    }

    /// The original facet's triangle in bed coordinates.
    pub fn world_triangle(&self, facet: usize) -> [V3; 3] {
        let t = &self.tris[facet];
        let mut tri = t.verts.map(|v| self.world[v as usize]);
        if self.reversed {
            tri.swap(1, 2);
        }
        tri
    }

    fn select_triangle(
        &mut self,
        facet: usize,
        shape: &Shape<'_>,
        state: u8,
        splitting: bool,
    ) -> bool {
        if !self.tris[facet].valid {
            return false;
        }
        if !self.select_recursive(facet, shape, state, splitting, 0) {
            return false;
        }
        self.remove_useless_children(facet);
        true
    }

    fn world_tri(&self, idx: usize) -> [V3; 3] {
        self.tris[idx].verts.map(|v| self.world[v as usize])
    }

    /// `depth` is `idx`'s depth below its original facet: nothing is split below
    /// `MAX_SPLIT_DEPTH`, which bounds this recursion (and every other walk of the tree).
    fn select_recursive(
        &mut self,
        idx: usize,
        shape: &Shape<'_>,
        state: u8,
        splitting: bool,
        depth: usize,
    ) -> bool {
        if !self.tris[idx].valid {
            return false;
        }
        let pts = self.world_tri(idx);
        let inside = pts.iter().filter(|&&p| shape.point_inside(p)).count();
        if inside == 0 && !shape.pointer_in_triangle(pts) && !shape.any_edge_inside(pts) {
            return false;
        }
        if inside == 3 {
            // Wholly inside: drop any subdivision and take the state as one triangle.
            self.undivide(idx);
            self.tris[idx].state = state;
            return true;
        }
        // Partly inside: subdivide (unless already) and try the children.
        let t = self.tris[idx];
        if !t.is_split() && t.state == state {
            return true;
        }
        if splitting {
            // At the depth limit the triangle stays as it is, as one whose sides are all
            // under the edge limit does (upstream's "shall be unselected").
            if depth < MAX_SPLIT_DEPTH {
                self.split_triangle(idx);
            }
        } else if !self.tris[idx].is_split() {
            self.tris[idx].state = state;
        }
        let t = self.tris[idx];
        for k in 0..t.child_count() {
            self.select_recursive(t.children[k] as usize, shape, state, splitting, depth + 1);
        }
        true
    }

    /// Decides which sides to split (those longer than the edge limit) and splits.
    fn split_triangle(&mut self, idx: usize) {
        if self.tris[idx].is_split() {
            return;
        }
        // Subdivision budget: a brush edge along a huge triangle (a model scaled thousands of
        // times, seen edge-on by the circle brush) otherwise splits it into tens of millions
        // of children, gigabytes for one stroke. Past the budget, strokes stop refining.
        if self.tris.len() >= self.orig_tris + MAX_SUBDIVISION {
            return;
        }
        let pts = self.world_tri(idx);
        let sides = [
            norm2(sub(pts[2], pts[1])),
            norm2(sub(pts[0], pts[2])),
            norm2(sub(pts[1], pts[0])),
        ];
        let mut to_split = vec![];
        let mut keep = 0;
        // A side whose f32 midpoint is one of its ends cannot get shorter (a model scaled
        // 10,000×, its vertices ulps apart compared with a fine brush's edge limit): splitting
        // it would only copy the triangle, level after level.
        let halves = |a: V3, b: V3| {
            let m = mid(a, b);
            m != a && m != b
        };
        let ends = [(1, 2), (2, 0), (0, 1)];
        for (i, &s) in sides.iter().enumerate() {
            if s > self.edge_limit_sqr && halves(pts[ends[i].0], pts[ends[i].1]) {
                to_split.push(i as u8);
            } else {
                keep = i as u8;
            }
        }
        if to_split.is_empty() {
            return;
        }
        let special = if to_split.len() == 2 {
            keep
        } else {
            to_split[0]
        };
        self.tris[idx].splits = to_split.len() as u8;
        self.tris[idx].side = if to_split.len() == 3 { 0 } else { special };
        let old = self.tris[idx].state;
        self.perform_split(idx, old);
    }

    fn push_vertex(&mut self, a: u32, b: u32) -> u32 {
        let i = self.local.len() as u32;
        self.local
            .push(mid(self.local[a as usize], self.local[b as usize]));
        self.world
            .push(mid(self.world[a as usize], self.world[b as usize]));
        i
    }

    fn push_triangle(&mut self, verts: [u32; 3], source: u32, state: u8) -> u32 {
        self.tris.push(Tri {
            verts,
            source,
            children: [0; 4],
            splits: 0,
            side: 0,
            state,
            valid: true,
            seed: false,
        });
        (self.tris.len() - 1) as u32
    }

    /// Splits `idx` as its `splits` / `side` say, allocating midpoints and children in the
    /// upstream order (which the wire format depends on).
    fn perform_split(&mut self, idx: usize, old_state: u8) {
        let t = self.tris[idx];
        let source = t.source;
        // Vertices rotated so the special side comes first.
        let v: Vec<u32> = (0..3).map(|j| t.verts[(t.side as usize + j) % 3]).collect();
        let mut children = [0u32; 4];
        match t.splits {
            1 => {
                let m = self.push_vertex(v[2], v[1]);
                children[0] = self.push_triangle([v[0], v[1], m], source, old_state);
                children[1] = self.push_triangle([m, v[2], v[0]], source, old_state);
            }
            2 => {
                let m01 = self.push_vertex(v[1], v[0]);
                let m02 = self.push_vertex(v[0], v[2]);
                children[0] = self.push_triangle([v[0], m01, m02], source, old_state);
                children[1] = self.push_triangle([m01, v[1], m02], source, old_state);
                children[2] = self.push_triangle([v[1], v[2], m02], source, old_state);
            }
            3 => {
                let m01 = self.push_vertex(v[1], v[0]);
                let m12 = self.push_vertex(v[2], v[1]);
                let m20 = self.push_vertex(v[0], v[2]);
                children[0] = self.push_triangle([v[0], m01, m20], source, old_state);
                children[1] = self.push_triangle([m01, v[1], m12], source, old_state);
                children[2] = self.push_triangle([m12, v[2], m20], source, old_state);
                children[3] = self.push_triangle([m01, m12, m20], source, old_state);
            }
            _ => {}
        }
        self.tris[idx].children = children;
    }

    /// Removes all subdivision below `idx` (children become invalid slots).
    fn undivide(&mut self, idx: usize) {
        let t = self.tris[idx];
        if t.is_split() {
            for k in 0..t.child_count() {
                let c = t.children[k] as usize;
                self.undivide(c);
                self.tris[c].valid = false;
                self.invalid += 1;
            }
            self.tris[idx].splits = 0;
            self.tris[idx].side = 0;
        }
    }

    /// Merges children that all ended up as leaves of one state back into their parent.
    fn remove_useless_children(&mut self, idx: usize) -> bool {
        let t = self.tris[idx];
        if !t.is_split() {
            return false;
        }
        let mut removed = false;
        for k in 0..t.child_count() {
            if self.tris[t.children[k] as usize].is_split() {
                removed |= self.remove_useless_children(t.children[k] as usize);
            }
        }
        let first = self.tris[t.children[0] as usize];
        for k in 0..t.child_count() {
            let c = self.tris[t.children[k] as usize];
            if c.is_split() || (k > 0 && c.state != first.state) {
                return removed;
            }
        }
        self.undivide(idx);
        self.tris[idx].state = first.state;
        true
    }

    /// Compacts the triangle and vertex arrays once more than half the slots are dead, by
    /// rebuilding from the serialised tree (cheaper to get right than free lists).
    fn maybe_compact(&mut self) {
        if self.invalid * 2 > self.tris.len() && self.invalid > 1024 {
            let states = self.serialize();
            self.clear();
            self.load(&states);
        }
    }

    // ----- wire format -------------------------------------------------------------------

    /// Loads the facet streams (replacing the current state).
    pub fn load(&mut self, states: &FacetStates) {
        self.clear();
        for (triangle, codes) in states.entries() {
            let facet = triangle as usize;
            if facet >= self.orig_tris {
                break;
            }
            let mut pos = 0;
            self.load_node(facet, codes, &mut pos);
        }
    }

    fn load_node(&mut self, idx: usize, codes: &[u8], pos: &mut usize) {
        let Some(&code) = codes.get(*pos) else { return };
        *pos += 1;
        let count = code & 3;
        if count == 0 {
            self.tris[idx].state = super::state(code, codes, pos).unwrap_or(NONE);
            return;
        }
        self.tris[idx].splits = count;
        self.tris[idx].side = if count == 3 { 0 } else { code >> 2 };
        self.perform_split(idx, NONE);
        let children = self.tris[idx].children;
        // Children are stored last to first, as PrusaSlicer 2.3.1 wrote them.
        for k in (0..=count as usize).rev() {
            self.load_node(children[k] as usize, codes, pos);
        }
    }

    /// The whole painting as facet streams (the 3MF `TriangleSelector` nibble codes).
    pub fn serialize(&self) -> FacetStates {
        let mut out = FacetStates::default().with_reversed(self.reversed);
        let mut codes = Vec::new();
        for facet in 0..self.orig_tris {
            let t = &self.tris[facet];
            if t.is_split() || t.state != NONE {
                codes.clear();
                self.serialize_node(facet, &mut codes);
                out.push_codes(facet as u32, &codes)
                    .expect("selector trees are well formed");
            }
        }
        out
    }

    fn serialize_node(&self, idx: usize, codes: &mut Vec<u8>) {
        let t = &self.tris[idx];
        if t.is_split() {
            codes.push(t.splits | (t.side << 2));
            for k in (0..t.child_count()).rev() {
                self.serialize_node(t.children[k] as usize, codes);
            }
        } else {
            encode_state(t.state, codes);
        }
    }
}

/// Leaf state nibbles: 0..=2 directly, 3..=16 with one extra nibble, 17..=255 with three.
pub fn encode_state(state: u8, codes: &mut Vec<u8>) {
    match state {
        0..=2 => codes.push(state << 2),
        3..=16 => {
            codes.push(0b1100);
            codes.push(state - 3);
        }
        _ => {
            let n = state - 17;
            codes.push(0b1100);
            codes.push(14);
            codes.push(n & 15);
            codes.push(n >> 4);
        }
    }
}

/// Neighbour across each edge of every face (`-1` at boundaries or non-manifold edges past
/// the first pair), from a sorted edge list rather than a hash map.
pub fn face_neighbors(mesh: &TriangleMesh) -> Vec<[i32; 3]> {
    let mut edges: Vec<(u32, u32, u32, u8)> = Vec::with_capacity(mesh.faces.len() * 3);
    for (fi, f) in mesh.faces.iter().enumerate() {
        for k in 0..3 {
            let a = f[k];
            let b = f[(k + 1) % 3];
            edges.push((a.min(b), a.max(b), fi as u32, k as u8));
        }
    }
    edges.sort_unstable();
    let mut out = vec![[-1i32; 3]; mesh.faces.len()];
    let mut i = 0;
    while i + 1 < edges.len() {
        let (a, b, f, k) = edges[i];
        let (a2, b2, g, j) = edges[i + 1];
        if a == a2 && b == b2 && f != g {
            out[f as usize][k as usize] = g as i32;
            out[g as usize][j as usize] = f as i32;
            i += 2;
        } else {
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cube(size: f32) -> TriangleMesh {
        let s = size;
        TriangleMesh {
            vertices: vec![
                [0., 0., 0.],
                [s, 0., 0.],
                [s, s, 0.],
                [0., s, 0.],
                [0., 0., s],
                [s, 0., s],
                [s, s, s],
                [0., s, s],
            ],
            faces: vec![
                [0, 2, 1],
                [0, 3, 2],
                [4, 5, 6],
                [4, 6, 7],
                [0, 1, 5],
                [0, 5, 4],
                [1, 2, 6],
                [1, 6, 5],
                [2, 3, 7],
                [2, 7, 6],
                [3, 0, 4],
                [3, 4, 7],
            ],
        }
    }
    const ID: Transform = [
        [1., 0., 0., 0.],
        [0., 1., 0., 0.],
        [0., 0., 1., 0.],
        [0., 0., 0., 1.],
    ];
    fn area(t: [V3; 3]) -> f32 {
        0.5 * norm2(cross(sub(t[1], t[0]), sub(t[2], t[0]))).sqrt()
    }
    fn painted_area(sel: &TriangleSelector, state: u8) -> f32 {
        let mut a = 0.;
        sel.leaves(false, |s, t| {
            if s == state {
                a += area(t);
            }
        });
        a
    }

    #[test]
    fn neighbours_and_normals_of_a_cube() {
        let m = cube(20.);
        let sel = TriangleSelector::new(&m, &ID, false);
        let n = face_neighbors(&m);
        for (i, f) in n.iter().enumerate() {
            assert!(f.iter().all(|&g| g >= 0 && g as usize != i), "{i}: {f:?}");
        }
        // Bottom faces point down, top faces up, the front (y = 0) faces towards -Y.
        assert!(sel.normal(0)[2] < -0.99 && sel.normal(1)[2] < -0.99);
        assert!(sel.normal(2)[2] > 0.99);
        assert!(sel.normal(4)[1] < -0.99);
        // A mirrored placement keeps outward normals.
        let mut mirror = ID;
        mirror[0][0] = -1.;
        let sel = TriangleSelector::new(&m, &mirror, false);
        assert!(sel.normal(0)[2] < -0.99);
        assert!(sel.normal(4)[1] < -0.99);
    }

    #[test]
    fn brush_hit_test_circle_and_sphere() {
        // A circle brush seen from +Y, aimed at the front face centre.
        let c = Cursor::new(Brush::Circle, [10., 0., 10.], [10., -100., 10.], 3.);
        assert!(c.point_inside([11., 0., 11.]));
        assert!(!c.point_inside([14., 0., 10.]));
        // Depth does not matter for a circle (a cylinder along the view)...
        assert!(c.point_inside([10., 20., 10.]));
        // ...but it does for a sphere.
        let s = Cursor::new(Brush::Sphere, [10., 0., 10.], [10., -100., 10.], 3.);
        assert!(!s.point_inside([10., 20., 10.]));
        assert!(s.point_inside([10., 2., 10.]));
        // The pointer pierces a triangle around the centre, not one beside it.
        let tri = [[8., 0., 8.], [12., 0., 8.], [10., 0., 13.]];
        assert!(c.pointer_in_triangle(tri));
        let beside = [[20., 0., 8.], [24., 0., 8.], [22., 0., 13.]];
        assert!(!c.pointer_in_triangle(beside));
        // An edge passing through the brush counts, with no vertex inside.
        let through = [[0., 0., 10.], [20., 0., 10.], [10., 0., 30.]];
        assert!(c.any_edge_inside(through));
        assert!(s.any_edge_inside(through));
        assert!(!c.any_edge_inside(beside));
        // Facets turned away from the camera are invisible to a circle, not to a sphere.
        assert!(c.facet_visible([0., -1., 0.]));
        assert!(!c.facet_visible([0., 1., 0.]));
        assert!(s.facet_visible([0., 1., 0.]));
        assert!(line_inside_sphere(
            [0., 0., 0.],
            [10., 0., 0.],
            [5., 1., 0.],
            2.
        ));
        assert!(!line_inside_sphere(
            [0., 0., 0.],
            [10., 0., 0.],
            [5., 3., 0.],
            2.
        ));
    }

    #[test]
    fn split_on_paint_follows_the_brush_edge() {
        let m = cube(20.);
        let mut sel = TriangleSelector::new(&m, &ID, false);
        // Facet 4 is on the front face (y = 0); a 4 mm circle at its centre.
        let c = Cursor::new(Brush::Circle, [10., 0., 10.], [10., -100., 10.], 4.);
        sel.paint(4, &c, ENFORCER, true, None);
        let painted = painted_area(&sel, ENFORCER);
        let disc = std::f32::consts::PI * 16.;
        // The painted leaves approximate the disc (subdivided to radius / 5 edges), and
        // nothing else on the cube took paint.
        assert!(
            painted > disc * 0.8 && painted < disc * 1.35,
            "{painted} vs {disc}"
        );
        let mut leaves = 0;
        sel.leaves(false, |_, t| {
            leaves += 1;
            assert!(
                t.iter().all(|p| p[1].abs() < 1e-4),
                "paint left the front face"
            );
        });
        assert!(
            leaves > 20,
            "{leaves} leaves: the triangles were not subdivided"
        );
        // Without splitting the two whole facets of the face take the state.
        let mut whole = TriangleSelector::new(&m, &ID, false);
        whole.paint(4, &c, ENFORCER, false, None);
        assert!((painted_area(&whole, ENFORCER) - 400.).abs() < 1e-3);
        // Painting the same spot again with the same state changes nothing.
        let before = sel.serialize();
        sel.paint(4, &c, ENFORCER, true, None);
        assert_eq!(sel.serialize(), before);
        // Erasing (state NONE) over the whole face merges the children back into facets.
        let big = Cursor::new(Brush::Circle, [10., 0., 10.], [10., -100., 10.], 40.);
        sel.paint(4, &big, NONE, true, None);
        assert!(!sel.has_painting());
        assert!(sel.serialize().is_empty());
        let mut count = 0;
        sel.leaves(true, |_, _| count += 1);
        assert_eq!(count, 12, "children were not merged back");
    }

    #[test]
    fn sphere_paints_through_and_overhang_filter_keeps_walls_clean() {
        let m = cube(20.);
        let mut sel = TriangleSelector::new(&m, &ID, false);
        // A sphere at a bottom edge reaches the bottom and the front face.
        let s = Cursor::new(Brush::Sphere, [10., 0., 0.], [10., -100., 20.], 5.);
        sel.paint(0, &s, ENFORCER, true, None);
        let mut faces = std::collections::BTreeSet::new();
        sel.leaves(false, |_, t| {
            if t.iter().all(|p| p[2].abs() < 1e-4) {
                faces.insert("bottom");
            }
            if t.iter().all(|p| p[1].abs() < 1e-4) {
                faces.insert("front");
            }
        });
        assert_eq!(faces.len(), 2, "{faces:?}");
        // With the overhang filter (45°) only the bottom (straight down) takes paint.
        let mut sel = TriangleSelector::new(&m, &ID, false);
        sel.paint(0, &s, ENFORCER, true, Some(45f32.to_radians().cos()));
        sel.leaves(false, |_, t| assert!(t.iter().all(|p| p[2].abs() < 1e-4)));
        assert!(sel.has_painting());
        // "Paint overhangs": the bottom lies on the bed, so a plain cube has none; lifted, it has.
        let mut sel = TriangleSelector::new(&m, &ID, false);
        assert_eq!(
            sel.paint_overhangs(45f32.to_radians().cos(), 0., ENFORCER),
            0
        );
        let mut lifted = ID;
        lifted[3][2] = 5.;
        let mut sel = TriangleSelector::new(&m, &lifted, false);
        assert_eq!(sel.overhang_facets(45f32.to_radians().cos(), 0.).len(), 2);
        assert_eq!(
            sel.paint_overhangs(45f32.to_radians().cos(), 0., ENFORCER),
            2
        );
        assert!((painted_area(&sel, ENFORCER) - 400.).abs() < 1e-3);
    }

    #[test]
    fn height_range_and_fill() {
        let m = cube(20.);
        let mut sel = TriangleSelector::new(&m, &ID, false);
        sel.paint_height_range(&HeightRange::around(10., 2.), BLOCKER);
        let mut zs = (f32::INFINITY, f32::NEG_INFINITY);
        let mut painted = 0.;
        sel.leaves(false, |_, t| {
            for p in t {
                zs.0 = zs.0.min(p[2]);
                zs.1 = zs.1.max(p[2]);
            }
            painted += area(t);
        });
        // A 2 mm band around the four side faces: 4 × 20 × 2 = 160 mm², within 9 .. 11.
        assert!(zs.0 >= 9. - 1e-3 && zs.1 <= 11. + 1e-3, "{zs:?}");
        assert!((painted - 160.).abs() < 160. * 0.15, "{painted}");
        // World leaves cover the cube, each split child naming the side facet it lies in.
        let mut covered = 0.;
        sel.world_leaves(true, |facet, state, t| {
            covered += area(t);
            if state == BLOCKER {
                assert!((4..12).contains(&facet), "{facet}");
            }
            let o = sel.world_triangle(facet);
            let n = sel.normal(facet);
            assert!(
                dot(sub(t[0], o[0]), n).abs() < 1e-3,
                "leaf off its facet's plane"
            );
        });
        assert!((covered - 2400.).abs() < 2400. * 1e-3, "{covered}");
        // Smart fill at 30° takes one face (two facets), at 100° the whole cube.
        let mut sel = TriangleSelector::new(&m, &ID, false);
        sel.fill(4, 30., ENFORCER, None);
        assert!((painted_area(&sel, ENFORCER) - 400.).abs() < 1e-3);
        sel.fill(4, 100., BLOCKER, None);
        assert!((painted_area(&sel, BLOCKER) - 2400.).abs() < 1e-3);
        sel.clear();
        assert!(!sel.has_painting());
    }

    /// A closed cylinder around the z axis: `segments` sides, one ring at each end (long, thin
    /// side facets: a coarse mesh) or `rings` rings along the height.
    fn cylinder(radius: f32, height: f32, segments: u32, rings: u32) -> TriangleMesh {
        let mut vertices = vec![];
        for r in 0..=rings {
            let z = height * r as f32 / rings as f32;
            for k in 0..segments {
                let a = k as f32 / segments as f32 * std::f32::consts::TAU;
                vertices.push([radius * a.cos(), radius * a.sin(), z]);
            }
        }
        let bottom = vertices.len() as u32;
        vertices.push([0., 0., 0.]);
        let top = vertices.len() as u32;
        vertices.push([0., 0., height]);
        let mut faces = vec![];
        for r in 0..rings {
            for k in 0..segments {
                let (a, b) = (r * segments + k, r * segments + (k + 1) % segments);
                let (c, d) = (a + segments, b + segments);
                faces.push([a, b, d]);
                faces.push([a, d, c]);
            }
        }
        for k in 0..segments {
            let (a, b) = (k, (k + 1) % segments);
            faces.push([bottom, b, a]);
            faces.push([top, rings * segments + a, rings * segments + b]);
        }
        TriangleMesh { vertices, faces }
    }

    /// A UV sphere of `radius` around the origin.
    fn sphere(radius: f32, segments: u32, stacks: u32) -> TriangleMesh {
        let mut vertices = vec![[0., 0., radius]];
        for s in 1..stacks {
            let polar = s as f32 / stacks as f32 * std::f32::consts::PI;
            for k in 0..segments {
                let a = k as f32 / segments as f32 * std::f32::consts::TAU;
                vertices.push([
                    radius * polar.sin() * a.cos(),
                    radius * polar.sin() * a.sin(),
                    radius * polar.cos(),
                ]);
            }
        }
        let south = vertices.len() as u32;
        vertices.push([0., 0., -radius]);
        let ring = |s: u32, k: u32| 1 + (s - 1) * segments + k % segments;
        let mut faces = vec![];
        for k in 0..segments {
            faces.push([0, ring(1, k), ring(1, k + 1)]);
            faces.push([south, ring(stacks - 1, k + 1), ring(stacks - 1, k)]);
        }
        for s in 1..stacks - 1 {
            for k in 0..segments {
                let (a, b) = (ring(s, k), ring(s, k + 1));
                let (c, d) = (ring(s + 1, k), ring(s + 1, k + 1));
                faces.push([a, c, d]);
                faces.push([a, d, b]);
            }
        }
        TriangleMesh { vertices, faces }
    }

    #[test]
    fn spread_limit_on_a_sphere_paints_a_cap() {
        let r = 10.;
        let m = sphere(r, 64, 48);
        let mut sel = TriangleSelector::new(&m, &ID, false);
        // The north pole: every facet around it touches the pole vertex; start on one.
        let origin = [0., 0., r];
        let start = 0;
        let unlimited = {
            let mut s = TriangleSelector::new(&m, &ID, false);
            s.fill(start, 30., ENFORCER, None);
            painted_area(&s, ENFORCER)
        };
        let sphere_area = 4. * std::f32::consts::PI * r * r;
        assert!(unlimited > sphere_area * 0.95, "{unlimited}");
        for d in [3f32, 8., 15.] {
            sel.clear();
            let spread = Spread {
                origin,
                max_distance: d,
            };
            sel.fill_within(start, 30., ENFORCER, None, Some(spread));
            // A cap of geodesic radius d: area 2 pi r^2 (1 - cos(d / r)).
            let cap = 2. * std::f32::consts::PI * r * r * (1. - (d / r).cos());
            let got = painted_area(&sel, ENFORCER);
            assert!((got - cap).abs() < cap * 0.08, "d {d}: {got} vs cap {cap}");
            // Every painted point lies within the distance (on the sphere, by angle).
            sel.leaves(false, |_, t| {
                for p in t {
                    let geo = r * (p[2] / r).clamp(-1., 1.).acos();
                    assert!(geo <= d * 1.03 + 0.2, "d {d}: point at {geo}");
                }
            });
        }
        // The reach report: the nearest facets first, all of the sphere in total.
        let reach = sel.fill_reach(start, 30., None, origin);
        assert!(reach.windows(2).all(|w| w[0].0 <= w[1].0));
        let total: f32 = reach.iter().map(|r| r.1).sum();
        assert!((total - unlimited).abs() < 1e-1 * unlimited);
        let far = reach.last().unwrap().0;
        assert!(
            (far - std::f32::consts::PI * r).abs() < 0.08 * std::f32::consts::PI * r,
            "{far}"
        );
    }

    #[test]
    fn spread_limit_on_a_coarse_cylinder_is_a_disc_of_the_unrolled_side() {
        // 24 sides, one facet pair per side over the whole 40 mm height: long thin facets.
        let (radius, height) = (15., 40.);
        let m = cylinder(radius, height, 24, 1);
        let mut sel = TriangleSelector::new(&m, &ID, false);
        // A point half way up the side facet 0 (between segments 0 and 1).
        let t = sel.world_triangle(0);
        let mid_edge = mul(add(t[0], t[1]), 0.5);
        let origin = [mid_edge[0], mid_edge[1], height / 2.];
        let n = sel.normal(0);
        // Move onto the facet's plane (the chord between the two segment vertices).
        let origin = sub(origin, mul(n, dot(sub(origin, t[0]), n)));
        let start = (0..m.faces.len())
            .find(|&f| {
                let t = sel.world_triangle(f);
                point_triangle_dist2(origin, t) < 1e-6
            })
            .unwrap();
        let d = 10.;
        // Vertex distances are the unrolled side's straight lines: half way round (a 24-gon
        // of 15 mm, 1.95 mm from the click's vertical edge) and 20 mm down.
        let region = sel.fill_region(start, 30., None);
        assert_eq!(region.len(), 48, "the fill stays on the side");
        let g = sel.geodesic(&region, start, origin, f32::INFINITY);
        let side = 2. * radius * (std::f32::consts::PI / 24.).sin();
        let far = (12. * side - side / 2.).hypot(height / 2.);
        assert!((g.dist[12] - far).abs() < 0.05, "{} vs {far}", g.dist[12]);
        assert!(crosses(
            [0., -1., 0.],
            [0., 1., 0.],
            [-1., 0., 0.],
            [1., 0., 0.]
        ));
        assert!(!crosses(
            [0., -1., 0.],
            [3., 1., 0.],
            [-1., 0., 0.],
            [1., 0., 0.]
        ));
        sel.fill_within(
            start,
            30.,
            ENFORCER,
            None,
            Some(Spread {
                origin,
                max_distance: d,
            }),
        );
        // The side unrolls flat: a disc of radius d, nothing on the caps.
        let disc = std::f32::consts::PI * d * d;
        let got = painted_area(&sel, ENFORCER);
        assert!((got - disc).abs() < disc * 0.06, "{got} vs {disc}");
        sel.leaves(false, |_, t| {
            for p in t {
                assert!(
                    p[2] > 1. && p[2] < height - 1.,
                    "paint reached a cap: {p:?}"
                );
                assert!((p[2] - height / 2.).abs() <= d + 0.7, "{p:?}");
            }
        });
        // Paint elsewhere stays: a second limited fill around the other side keeps the first.
        let before = painted_area(&sel, ENFORCER);
        let far_side = (0..m.faces.len())
            .find(|&f| {
                let n = sel.normal(f);
                n[0] < -0.98
            })
            .unwrap();
        let tf = sel.world_triangle(far_side);
        let c = mul(add(add(tf[0], tf[1]), tf[2]), 1. / 3.);
        sel.fill_within(
            far_side,
            30.,
            BLOCKER,
            None,
            Some(Spread {
                origin: c,
                max_distance: 4.,
            }),
        );
        assert!((painted_area(&sel, ENFORCER) - before).abs() < 1e-2);
        assert!(painted_area(&sel, BLOCKER) > 0.8 * std::f32::consts::PI * 16.);
        // The wire format round-trips the split facets.
        let states = sel.serialize();
        let mut again = TriangleSelector::new(&m, &ID, false);
        again.load(&states);
        assert_eq!(again.serialize(), states);
    }

    #[test]
    fn no_spread_limit_is_the_plain_fill() {
        let m = cylinder(10., 30., 32, 6);
        for spread in [
            None,
            Some(Spread {
                origin: [10., 0., 15.],
                max_distance: 0.,
            }),
        ] {
            let mut a = TriangleSelector::new(&m, &ID, false);
            let mut b = TriangleSelector::new(&m, &ID, false);
            a.fill(3, 30., ENFORCER, None);
            b.fill_within(3, 30., ENFORCER, None, spread);
            assert_eq!(a.serialize(), b.serialize());
        }
        // A blank copy has no paint but the same facets.
        let mut a = TriangleSelector::new(&m, &ID, false);
        a.fill(3, 30., ENFORCER, None);
        let blank = a.blank();
        assert!(!blank.has_painting());
        assert_eq!(blank.facet_count(), a.facet_count());
    }

    #[test]
    fn round_trip_through_3mf_strings_and_the_engine_decoder() {
        let m = cube(20.);
        let mut sel = TriangleSelector::new(&m, &ID, false);
        let c = Cursor::new(Brush::Circle, [10., 0., 10.], [10., -100., 10.], 4.);
        sel.paint(4, &c, ENFORCER, true, None);
        sel.set_facet(2, 7); // tool 7: extended state
        sel.set_facet(3, 40); // tool 40: two extra nibbles
        let states = sel.serialize();
        assert!(states.painted_triangles() >= 3);
        // The wire strings decode with the engine's parser to the same records...
        let mut parsed = FacetStates::default();
        for (triangle, codes) in states.entries() {
            parsed
                .push(triangle, &FacetStates::wire_string(codes))
                .unwrap();
        }
        assert_eq!(parsed, states);
        // ...and the engine's leaf geometry equals the selector's leaves.
        let mut from_engine = vec![];
        parsed.visit(&m, false, |s, t| from_engine.push((s, t)));
        let mut from_selector = vec![];
        sel.leaves(false, |s, t| {
            from_selector.push((s, t.map(|p| p.map(f64::from))))
        });
        assert_eq!(from_engine.len(), from_selector.len());
        let key = |(s, t): &(u8, [[f64; 3]; 3])| {
            let c: Vec<i64> = (0..3)
                .map(|k| ((t[0][k] + t[1][k] + t[2][k]) * 1000.).round() as i64)
                .collect();
            (*s, c)
        };
        let mut a: Vec<_> = from_engine.iter().map(key).collect();
        let mut b: Vec<_> = from_selector.iter().map(key).collect();
        a.sort();
        b.sort();
        assert_eq!(a, b);
        // Loading the strings back gives the same tree again.
        let mut again = TriangleSelector::new(&m, &ID, false);
        again.load(&states);
        assert_eq!(again.serialize(), states);
        assert!((painted_area(&again, ENFORCER) - painted_area(&sel, ENFORCER)).abs() < 1e-4);
        assert!((painted_area(&again, 40) - 200.).abs() < 1e-3);
        // Reversed streams load and save with the swapped winding flag.
        let rev = states.clone().with_reversed(true);
        let mut r = TriangleSelector::new(&m, &ID, true);
        r.load(&rev);
        assert!(r.serialize().reversed());
        assert_eq!(r.serialize(), rev);
    }

    #[test]
    fn state_nibbles_match_the_decoder() {
        for state in [0u8, 1, 2, 3, 9, 16, 17, 100, 255] {
            let mut codes = vec![];
            encode_state(state, &mut codes);
            let mut pos = 0;
            let code = codes[pos];
            pos += 1;
            assert_eq!(super::super::state(code, &codes, &mut pos).unwrap(), state);
            assert_eq!(pos, codes.len());
        }
    }

    /// GUI monkey test (seed 5): far from the origin (a model scaled 10,000×) neighbouring f32
    /// coordinates are further apart than a fine brush's edge limit, so a side's midpoint is
    /// one of its ends and splitting copied the triangle level after level: 43,000 nested
    /// `select_recursive` calls overflowed the stack. Sides that cannot be halved are not
    /// split, the tree stays within the wire format's depth and serialises.
    #[test]
    fn a_fine_brush_far_from_the_origin_stays_shallow() {
        let m = cube(20.);
        let mut far = ID;
        far[3][0] = 1.0e7;
        let mut sel = TriangleSelector::new(&m, &far, false);
        let centre = [1.0e7 + 7., 9., 20.];
        let c = Cursor::new(Brush::Circle, centre, [1.0e7 + 7., 9., 70.], 0.001);
        for facet in 0..m.faces.len() {
            sel.paint(facet, &c, ENFORCER, true, None);
        }
        let states = sel.serialize();
        assert!(
            sel.tris.len() < m.faces.len() + 100_000,
            "{}",
            sel.tris.len()
        );
        let mut again = TriangleSelector::new(&m, &far, false);
        again.load(&states);
        assert_eq!(again.serialize(), states);
    }

    /// GUI monkey test: one circle-brush stroke along the top of a box scaled 10,000× in X,
    /// seen almost edge-on, split the 200 m facets into tens of millions of children (3 GB).
    /// Subdivision stops at the budget and the stroke still paints.
    #[test]
    fn a_stroke_along_a_huge_facet_stays_within_the_subdivision_budget() {
        let m = cube(20.);
        let mut long = ID;
        long[0][0] = 10_000.;
        let mut sel = TriangleSelector::new(&m, &long, false);
        let centre = [100_000., 10., 20.];
        let c = Cursor::new(Brush::Circle, centre, [0., 10., 20.5], 5.);
        let started = std::time::Instant::now();
        sel.paint(2, &c, ENFORCER, true, None);
        // The last split may add its four children past the budget.
        assert!(
            sel.tris.len() <= m.faces.len() + MAX_SUBDIVISION + 4,
            "{}",
            sel.tris.len()
        );
        assert!(started.elapsed() < std::time::Duration::from_secs(60));
        assert!(painted_area(&sel, ENFORCER) > 0.);
    }

    /// Painted leaves of `state` grouped into edge- or vertex-touching islands.
    fn islands(sel: &TriangleSelector, state: u8) -> usize {
        let mut tris = vec![];
        sel.leaves(false, |s, t| {
            if s == state {
                tris.push(t);
            }
        });
        let touch = |a: &[V3; 3], b: &[V3; 3]| {
            a.iter()
                .any(|p| b.iter().any(|q| norm2(sub(*p, *q)) < 1e-6))
                // A vertex of one on an edge of the other (unshared midpoints, T-joints).
                || a.iter().any(|p| {
                    (0..3).any(|k| point_segment_dist2(*p, b[k], b[(k + 1) % 3]) < 1e-8)
                })
                || b.iter().any(|p| {
                    (0..3).any(|k| point_segment_dist2(*p, a[k], a[(k + 1) % 3]) < 1e-8)
                })
        };
        let mut group: Vec<usize> = (0..tris.len()).collect();
        fn root(g: &mut [usize], mut i: usize) -> usize {
            while g[i] != i {
                g[i] = g[g[i]];
                i = g[i];
            }
            i
        }
        for i in 0..tris.len() {
            for j in i + 1..tris.len() {
                if touch(&tris[i], &tris[j]) {
                    let (a, b) = (root(&mut group, i), root(&mut group, j));
                    group[a] = b;
                }
            }
        }
        (0..tris.len())
            .filter(|&i| root(&mut group, i) == i)
            .count()
    }

    /// A fast drag samples the pointer far apart (here about 20 mm steps with a 2 mm brush,
    /// zig-zagging over both facets of the front face). Dabs alone leave islands; capsules
    /// between consecutive samples paint one connected stroke that covers the whole path,
    /// at either brush.
    #[test]
    fn a_stroke_sampled_at_sparse_points_paints_a_connected_region() {
        let m = cube(40.);
        // The front face y = 0: facet 5 above its diagonal (x < z), facet 4 below.
        let samples: [(usize, V3, V3); 4] = [
            (5, [3., 0., 30.], [3., -200., 30.]),
            (4, [15., 0., 10.], [15., -200., 10.]),
            (4, [30., 0., 25.], [30., -200., 25.]),
            (4, [37., 0., 5.], [37., -200., 5.]),
        ];
        for brush in [Brush::Circle, Brush::Sphere] {
            let r = 2.;
            let mut dabs = TriangleSelector::new(&m, &ID, false);
            for &(f, p, cam) in &samples {
                dabs.paint(f, &Cursor::new(brush, p, cam, r), ENFORCER, true, None);
            }
            assert_eq!(islands(&dabs, ENFORCER), 4, "dabs alone leave gaps");
            let mut stroke = TriangleSelector::new(&m, &ID, false);
            for w in samples.windows(2) {
                let (f, a, cam) = w[0];
                let (_, b, _) = w[1];
                let c = Cursor::capsule(brush, a, b, cam, r);
                stroke.paint(f, &c, ENFORCER, true, None);
            }
            assert_eq!(islands(&stroke, ENFORCER), 1, "{brush:?}");
            // Every point along the path lies on a painted leaf.
            let mut leaves = vec![];
            stroke.leaves(false, |_, t| leaves.push(t));
            let on_leaf = |p: V3| {
                leaves.iter().any(|t| {
                    let n = cross(sub(t[1], t[0]), sub(t[2], t[0]));
                    let inside = (0..3)
                        .all(|k| dot(cross(sub(t[(k + 1) % 3], t[k]), sub(p, t[k])), n) >= -1e-4);
                    inside && dot(sub(p, t[0]), normalize(n)).abs() < 1e-3
                })
            };
            for w in samples.windows(2) {
                let (_, a, _) = w[0];
                let (_, b, _) = w[1];
                for k in 0..=20 {
                    let p = add(a, mul(sub(b, a), k as f32 / 20.));
                    assert!(on_leaf(p), "{brush:?}: gap at {p:?}");
                }
            }
            // The stroke stays a stroke: well under the face, around the path's width.
            let area = painted_area(&stroke, ENFORCER);
            assert!(area > 150. && area < 600., "{brush:?}: {area}");
            // Capsules keep the depth bound.
            fn depth(sel: &TriangleSelector, i: usize) -> usize {
                let t = sel.tris[i];
                (0..t.child_count())
                    .map(|k| 1 + depth(sel, t.children[k] as usize))
                    .max()
                    .unwrap_or(0)
            }
            assert!((0..stroke.orig_tris).all(|f| depth(&stroke, f) <= MAX_SPLIT_DEPTH));
        }
    }

    #[test]
    fn capsule_hit_tests() {
        let c = Cursor::capsule(
            Brush::Circle,
            [0., 0., 0.],
            [10., 0., 0.],
            [0., -100., 0.],
            1.,
        );
        assert!(c.point_inside([5., 0., 0.5]));
        assert!(
            c.point_inside([5., 30., 0.5]),
            "a circle capsule ignores depth"
        );
        assert!(!c.point_inside([5., 0., 1.5]));
        assert!(c.point_inside([10.5, 0., 0.]));
        assert!(!c.point_inside([11.5, 0., 0.]));
        let s = Cursor::capsule(
            Brush::Sphere,
            [0., 0., 0.],
            [10., 0., 0.],
            [0., -100., 0.],
            1.,
        );
        assert!(!s.point_inside([5., 3., 0.]));
        // An edge crossing the middle of the segment, both ends outside.
        let tri = [[5., 0., -5.], [5., 0., 5.], [30., 0., 30.]];
        assert!(c.any_edge_inside(tri) && s.any_edge_inside(tri));
        assert!(
            segment_segment_dist2([0., 0., 0.], [1., 0., 0.], [0., 1., 0.], [1., 1., 0.]) > 0.99
        );
        // Coincident ends are a dab.
        assert!(
            Cursor::capsule(Brush::Circle, [1.; 3], [1.; 3], [0.; 3], 1.)
                .second
                .is_none()
        );
    }
}
