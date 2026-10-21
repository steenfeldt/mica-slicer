// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/ClipperUtils.hpp :: union_ex, diff_ex, intersection_pl, diff_pl, offset_ex, offset, to_polygons, offset2_ex, opening_ex, closing_ex, union_safety_offset_ex
// Changes: Same operation names and semantics, implemented on Clipper2 (clipper2-rust) instead of
// Changes: PrusaSlicer's modified Clipper 6.4.2. Only the subset the engine uses.
// Changes: `diff_pl_is_empty` / `intersection_pl_is_empty` answer without clipping when no segment touches the clip.
// Changes: Per-job exact-input memoization of tree booleans and offset/cleanup results.
// Changes: `offset_ex_half_up` / `offset2_ex_half_up` round offset vertices like Clipper 6.4's `FRound`.
// Source: prusaslicer@b028299c77 src/libslic3r/ClipperUtils.cpp :: PolyTreeToExPolygons
// Changes: Tree results list an outer polygon before the islands nested in its holes (pre-order).
// Changes: Differences and intersections leave out operand rings entirely above or below the other operand (same sweep where the result can be); safety-offset variants with a pre-grown clip; the polyline contact test skips edges whose bounding box misses the line.
// Changes: The clip rings' runs of edges beyond the subject's Y range are shortened to single edges (their end vertices kept).

//! Polygon booleans and offsets, mirroring PrusaSlicer's `ClipperUtils` API.

use clipper2_rust::{
    ClipType, Clipper64, EndType, FillRule, JoinType, Path64, Paths64, Point64, PolyTree64,
    inflate_paths_64_rounding, simplify_path,
};

use super::{ExPolygon, Point, Polygon, Polyline};

/// Miter limit used for offsets unless a caller says otherwise (PrusaSlicer's `DefaultMiterLimit`).
pub const DEFAULT_MITER_LIMIT: f64 = 3.0;

/// Small extra offset that keeps touching or nearly touching shapes apart (PrusaSlicer's
/// `ClipperSafetyOffset`), in scaled units.
pub const CLIPPER_SAFETY_OFFSET: f64 = 10.0;

/// How corners are joined when offsetting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Join {
    Miter,
    Round,
    Square,
}

impl From<Join> for JoinType {
    fn from(j: Join) -> Self {
        match j {
            Join::Miter => JoinType::Miter,
            Join::Round => JoinType::Round,
            Join::Square => JoinType::Square,
        }
    }
}

fn to_path(points: &[Point]) -> Path64 {
    points.iter().map(|p| Point64::new(p.x, p.y)).collect()
}

fn from_path(path: &Path64) -> Vec<Point> {
    path.iter().map(|p| Point::new(p.x, p.y)).collect()
}

fn polygons_to_paths<'a>(polys: impl IntoIterator<Item = &'a Polygon>) -> Paths64 {
    polys.into_iter().map(|p| to_path(p)).collect()
}

fn expolygons_to_paths(ex: &[ExPolygon]) -> Paths64 {
    ex.iter()
        .flat_map(|e| e.polygons())
        .map(|p| to_path(p))
        .collect()
}

/// Converts a Clipper2 tree (outer, hole, outer, … nesting) into ExPolygons.
pub(super) fn tree_to_expolygons(tree: &PolyTree64) -> Vec<ExPolygon> {
    // PolyTreeToExPolygonsRecursive: an outer polygon precedes the islands nested in its holes.
    fn walk_outer(tree: &PolyTree64, idx: usize, out: &mut Vec<ExPolygon>) {
        let node = &tree.nodes[idx];
        let at = out.len();
        out.push(ExPolygon {
            contour: from_path(node.polygon()),
            holes: Vec::new(),
        });
        for &hole_idx in node.children() {
            let hole = &tree.nodes[hole_idx];
            out[at].holes.push(from_path(hole.polygon()));
            for &island_idx in hole.children() {
                walk_outer(tree, island_idx, out);
            }
        }
    }
    let mut out = Vec::new();
    for &idx in tree.root().children() {
        walk_outer(tree, idx, &mut out);
    }
    out
}

/// The lowest and highest Y of all points of `paths` (`None` when there are none).
pub(super) fn y_range(paths: &[Path64]) -> Option<(i64, i64)> {
    paths.iter().flatten().fold(None, |r, p| match r {
        None => Some((p.y, p.y)),
        Some((lo, hi)) => Some((lo.min(p.y), hi.max(p.y))),
    })
}

/// Which vertices of a closed ring [`retain_y_overlapping`] keeps for the Y range `(lo, hi)`;
/// `None` when the ring lies entirely beyond it (every edge strictly above `hi` or strictly
/// below `lo`).
///
/// Every edge that reaches into the range is kept with both its ends. Of each run of edges
/// beyond the range (one side: both ends strictly above `hi`, or both strictly below `lo`) the
/// first vertices after either end are kept too, up to and including the first one reached by
/// a non-horizontal step: the sweep reads them for the kept edges (the next vertex of a bound
/// and whether an edge's top is a local maximum, which depends on the direction of the next
/// non-horizontal step). The vertices between become one edge, which lies on that side too.
fn kept_vertices(n: usize, y: impl Fn(usize) -> i64, (lo, hi): (i64, i64)) -> Option<Vec<bool>> {
    let beyond = |a: i64, b: i64| (a > hi && b > hi) || (a < lo && b < lo);
    // Edge i runs from vertex i to vertex i + 1 (wrapping).
    let out: Vec<bool> = (0..n).map(|i| beyond(y(i), y((i + 1) % n))).collect();
    if out.iter().all(|&o| o) {
        return None;
    }
    let mut keep: Vec<bool> = (0..n).map(|i| !(out[i] && out[(i + n - 1) % n])).collect();
    if keep.iter().all(|&k| k) {
        return Some(keep);
    }
    for start in 0..n {
        // A run of edges beyond the range starts at `start` (its first vertex) ...
        if !out[start] || out[(start + n - 1) % n] {
            continue;
        }
        let mut end = start;
        while out[end] {
            end = (end + 1) % n;
        }
        // ... and ends at `end`; keep the vertices the sweep reads from both ends.
        let mut j = start;
        while j != end {
            let next = (j + 1) % n;
            keep[next] = true;
            if y(next) != y(j) {
                break;
            }
            j = next;
        }
        let mut j = end;
        while j != start {
            let prev = (j + n - 1) % n;
            keep[prev] = true;
            if y(prev) != y(j) {
                break;
            }
            j = prev;
        }
    }
    Some(keep)
}

/// Leaves out what of `paths` lies entirely above or entirely below `range`: rings entirely
/// beyond it are dropped, and the runs of edges beyond it are shortened (see
/// [`kept_vertices`]). Every vertex in the range and every edge reaching into it are kept,
/// unchanged and in order.
fn retain_y_overlapping(paths: &mut Paths64, range: (i64, i64)) {
    paths.retain_mut(|ring| {
        let n = ring.len();
        if n < 2 {
            return ring
                .first()
                .is_some_and(|p| p.y >= range.0 && p.y <= range.1);
        }
        let Some(keep) = kept_vertices(n, |i| ring[i].y, range) else {
            return false;
        };
        let mut i = 0;
        ring.retain(|_| {
            i += 1;
            keep[i - 1]
        });
        true
    });
}

/// The rings of `ex` as paths, with what lies entirely above or below `range` left out as
/// [`retain_y_overlapping`] does (`None`: nothing). Same result, without copying the parts
/// it leaves out: for a whole layer's outlines used against a small area.
pub(super) fn expolygons_to_paths_in(ex: &[ExPolygon], range: Option<(i64, i64)>) -> Paths64 {
    let Some(range) = range else {
        return Paths64::new();
    };
    let mut out = Paths64::new();
    for ring in ex.iter().flat_map(|e| e.polygons()) {
        let n = ring.len();
        if n < 2 {
            if ring
                .first()
                .is_some_and(|p| p.y >= range.0 && p.y <= range.1)
            {
                out.push(to_path(ring));
            }
            continue;
        }
        if let Some(keep) = kept_vertices(n, |i| ring[i].y, range) {
            out.push(
                ring.iter()
                    .zip(keep)
                    .filter(|(_, k)| *k)
                    .map(|(p, _)| Point64::new(p.x, p.y))
                    .collect(),
            );
        }
    }
    out
}

/// Drops the rings of `paths` that lie entirely above or entirely below `range`.
fn retain_rings_y_overlapping(paths: &mut Paths64, (lo, hi): (i64, i64)) {
    paths.retain(|ring| {
        y_range(std::slice::from_ref(ring)).is_some_and(|(a, b)| a <= hi && b >= lo)
    });
}

/// Removes the parts of the operands that cannot change a difference or intersection,
/// keeping the sweep exactly as it was where the result can be. The result lies within the
/// Y range of the subject (and, for an intersection, of the clip). Inside that range,
/// Clipper's sweep sees the same scanlines (they come from the vertices in it), the same
/// active edges with the same end points, and the same winding counts (a horizontal ray at a
/// Y in the range only crosses edges that reach it), so the output points, their order and
/// the nesting stay the same; what lies beyond the range only shapes the sweep there, where
/// nothing is output. So clip rings entirely above or below the subject are dropped, and the
/// stretches of the remaining clip rings beyond it are shortened (see
/// [`retain_y_overlapping`]); an intersection also drops the subject rings beyond the clip.
/// (Rings that are merely beside the subject, overlapping in Y, are kept: their vertices are
/// scanlines that round intersection points.) Only the work shrinks, e.g. a small branch
/// slice clipped by a whole layer's outlines.
///
/// The subject keeps its rings whole: the edges entering the range carry output, and where
/// two of them meet just beyond the range, the order in which the sweep brings them in follows
/// the edges they came along. (Shortening the subject of intersections changed results; a
/// temporary checker that ran every difference and intersection of the cat, cow and Benchy
/// jobs with and without the shortened clip found no difference in 2.7 million calls.)
pub(super) fn prune_far_rings(op: ClipType, subject: &mut Paths64, clip: &mut Paths64) {
    match op {
        ClipType::Difference => match y_range(subject) {
            Some(r) => retain_y_overlapping(clip, r),
            None => clip.clear(),
        },
        ClipType::Intersection => {
            match y_range(subject) {
                Some(r) => retain_y_overlapping(clip, r),
                None => clip.clear(),
            }
            match y_range(clip) {
                Some(r) => retain_rings_y_overlapping(subject, r),
                None => subject.clear(),
            }
        }
        _ => {}
    }
}

fn boolean_ex(
    op: ClipType,
    mut subject: Paths64,
    mut clip: Paths64,
    rule: FillRule,
) -> Vec<ExPolygon> {
    prune_far_rings(op, &mut subject, &mut clip);
    super::clip_cache::get_or_build(
        super::clip_cache::Operation([0, op as u64, rule as u64, 0, 0, 0, 0]),
        subject,
        clip,
        |subject, clip| boolean_ex_uncached(op, subject, clip, rule),
    )
}

fn boolean_ex_uncached(
    op: ClipType,
    subject: &Paths64,
    clip: &Paths64,
    rule: FillRule,
) -> Vec<ExPolygon> {
    let mut c = Clipper64::new();
    c.add_subject(subject);
    if !clip.is_empty() {
        c.add_clip(clip);
    }
    let mut tree = PolyTree64::new();
    let mut open = Paths64::new();
    c.execute_tree(op, rule, &mut tree, &mut open);
    tree_to_expolygons(&tree)
}

/// Union of polygons whose orientation encodes inside/outside (CCW adds, CW subtracts).
pub fn union_ex_positive(polys: &[Polygon]) -> Vec<ExPolygon> {
    boolean_ex(
        ClipType::Union,
        polygons_to_paths(polys),
        Paths64::new(),
        FillRule::Positive,
    )
}

/// Union with even-odd fill, for Arachne marker loops with arbitrary orientation.
pub fn union_ex_even_odd(polys: &[Polygon]) -> Vec<ExPolygon> {
    boolean_ex(
        ClipType::Union,
        polygons_to_paths(polys),
        Paths64::new(),
        FillRule::EvenOdd,
    )
}

/// Union of ExPolygons.
pub fn union_ex(ex: &[ExPolygon]) -> Vec<ExPolygon> {
    boolean_ex(
        ClipType::Union,
        expolygons_to_paths(ex),
        Paths64::new(),
        FillRule::NonZero,
    )
}

/// `subject` minus `clip`.
pub fn diff_ex(subject: &[ExPolygon], clip: &[ExPolygon]) -> Vec<ExPolygon> {
    let subject = expolygons_to_paths(subject);
    let clip = expolygons_to_paths_in(clip, y_range(&subject));
    boolean_ex(ClipType::Difference, subject, clip, FillRule::NonZero)
}

/// Area covered by both `subject` and `clip`.
pub fn intersection_ex(subject: &[ExPolygon], clip: &[ExPolygon]) -> Vec<ExPolygon> {
    let subject = expolygons_to_paths(subject);
    let clip = expolygons_to_paths_in(clip, y_range(&subject));
    boolean_ex(ClipType::Intersection, subject, clip, FillRule::NonZero)
}

/// Offsets ExPolygons by `delta` (scaled units; negative shrinks). Holes grow when the contour shrinks.
pub fn offset_ex(ex: &[ExPolygon], delta: f64, join: Join) -> Vec<ExPolygon> {
    if ex.is_empty() {
        return Vec::new();
    }
    offset_paths(expolygons_to_paths(ex), delta, join, DEFAULT_MITER_LIMIT)
}

/// The contours (counter-clockwise) and holes (clockwise) of ExPolygons as plain polygons,
/// in PrusaSlicer's `to_polygons` order: each contour followed by its holes.
pub fn to_polygons(ex: &[ExPolygon]) -> Vec<Polygon> {
    let oriented = |poly: &Polygon, ccw: bool| {
        let mut p = poly.clone();
        if (super::signed_area(&p) > 0.0) != ccw {
            p.reverse();
        }
        p
    };
    let mut out = Vec::new();
    for e in ex {
        out.push(oriented(&e.contour, true));
        out.extend(e.holes.iter().map(|h| oriented(h, false)));
    }
    out
}

fn offset_paths(paths: Paths64, delta: f64, join: Join, miter_limit: f64) -> Vec<ExPolygon> {
    if paths.is_empty() {
        return Vec::new();
    }
    offset_cached(
        paths,
        delta,
        join.into(),
        EndType::Polygon,
        miter_limit,
        0.0,
    )
}

fn offset_cached(
    paths: Paths64,
    delta: f64,
    join: JoinType,
    end: EndType,
    miter_limit: f64,
    arc_tolerance: f64,
) -> Vec<ExPolygon> {
    offset_cached_rounding(paths, delta, join, end, miter_limit, arc_tolerance, false)
}

fn offset_cached_rounding(
    paths: Paths64,
    delta: f64,
    join: JoinType,
    end: EndType,
    miter_limit: f64,
    arc_tolerance: f64,
    half_up: bool,
) -> Vec<ExPolygon> {
    super::clip_cache::get_or_build(
        super::clip_cache::Operation([
            1,
            delta.to_bits(),
            join as u64,
            end as u64,
            miter_limit.to_bits(),
            arc_tolerance.to_bits(),
            u64::from(half_up),
        ]),
        paths,
        Paths64::new(),
        |paths, _| {
            let offset = inflate_paths_64_rounding(
                paths,
                delta,
                join,
                end,
                miter_limit,
                arc_tolerance,
                half_up,
            );
            boolean_ex_uncached(
                ClipType::Union,
                &offset,
                &Paths64::new(),
                FillRule::Positive,
            )
        },
    )
}

/// Offsets polygons whose orientation encodes holes (clockwise polygons are holes) by `delta`
/// (scaled units; negative shrinks), returning polygons. PrusaSlicer's `offset(Polygons, …)` / `shrink`.
pub fn offset(polys: &[Polygon], delta: f64, join: Join, miter_limit: f64) -> Vec<Polygon> {
    to_polygons(&offset_paths(
        polygons_to_paths(polys),
        delta,
        join,
        miter_limit,
    ))
}

/// Offsets one ExPolygon, returning polygons. PrusaSlicer's `offset(ExPolygon, …)`.
pub fn offset_expolygon(ex: &ExPolygon, delta: f64, join: Join, miter_limit: f64) -> Vec<Polygon> {
    to_polygons(&offset_paths(
        expolygons_to_paths(std::slice::from_ref(ex)),
        delta,
        join,
        miter_limit,
    ))
}

/// Offsets ExPolygons with an explicit join, miter limit and arc tolerance (scaled; 0 picks Clipper's
/// default), PrusaSlicer's `offset_ex(…, joinType, miterLimit)`.
pub fn offset_ex_with(
    ex: &[ExPolygon],
    delta: f64,
    join: Join,
    miter_limit: f64,
    arc_tolerance: f64,
) -> Vec<ExPolygon> {
    if ex.is_empty() {
        return Vec::new();
    }
    offset_cached(
        expolygons_to_paths(ex),
        delta,
        join.into(),
        EndType::Polygon,
        miter_limit,
        arc_tolerance,
    )
}

/// The area within `delta` of open or closed polylines, with round joins and round ends
/// (ClipperLib's `jtRound` / `etOpenRound`).
pub fn offset_polylines_round(
    lines: &[Polyline],
    delta: f64,
    arc_tolerance: f64,
) -> Vec<ExPolygon> {
    let paths: Paths64 = lines
        .iter()
        .filter(|l| l.len() >= 2)
        .map(|l| to_path(l))
        .collect();
    if paths.is_empty() || delta <= 0.0 {
        return Vec::new();
    }
    offset_cached(
        paths,
        delta,
        JoinType::Round,
        EndType::Round,
        2.0,
        arc_tolerance,
    )
}

/// Union grown by the safety offset (PrusaSlicer's `union_safety_offset_ex`).
pub fn union_safety_offset_ex(ex: &[ExPolygon]) -> Vec<ExPolygon> {
    offset_ex(ex, CLIPPER_SAFETY_OFFSET, Join::Miter)
}

/// `subject` minus `clip` grown by the safety offset (`diff_ex(…, ApplySafetyOffset::Yes)`).
pub fn diff_ex_safety(subject: &[ExPolygon], clip: &[ExPolygon]) -> Vec<ExPolygon> {
    if subject.is_empty() || clip.is_empty() {
        return diff_ex(subject, clip);
    }
    diff_ex(
        subject,
        &offset_ex(clip, CLIPPER_SAFETY_OFFSET, Join::Miter),
    )
}

/// `clip` grown by the safety offset, as [`diff_ex_safety`] and [`intersection_ex_safety`]
/// grow it, for callers that clip many subjects by the same `clip`.
pub fn safety_offset_ex(clip: &[ExPolygon]) -> Vec<ExPolygon> {
    if clip.is_empty() {
        return Vec::new();
    }
    offset_ex(clip, CLIPPER_SAFETY_OFFSET, Join::Miter)
}

/// [`diff_ex_safety`] with the grown clip computed beforehand: `grown` must be
/// [`safety_offset_ex`]`(clip)`. Same result, without growing `clip` again per call.
pub fn diff_ex_safety_grown(
    subject: &[ExPolygon],
    clip: &[ExPolygon],
    grown: &[ExPolygon],
) -> Vec<ExPolygon> {
    if subject.is_empty() || clip.is_empty() {
        return diff_ex(subject, clip);
    }
    diff_ex(subject, grown)
}

/// [`intersection_ex_safety`] with the grown clip computed beforehand (see
/// [`diff_ex_safety_grown`]).
pub fn intersection_ex_safety_grown(
    subject: &[ExPolygon],
    clip: &[ExPolygon],
    grown: &[ExPolygon],
) -> Vec<ExPolygon> {
    if subject.is_empty() || clip.is_empty() {
        return Vec::new();
    }
    intersection_ex(subject, grown)
}

/// `subject` within `clip` grown by the safety offset (`intersection_ex(…, ApplySafetyOffset::Yes)`).
pub fn intersection_ex_safety(subject: &[ExPolygon], clip: &[ExPolygon]) -> Vec<ExPolygon> {
    if subject.is_empty() || clip.is_empty() {
        return Vec::new();
    }
    intersection_ex(
        subject,
        &offset_ex(clip, CLIPPER_SAFETY_OFFSET, Join::Miter),
    )
}

/// Removes vertices that deviate less than `epsilon` (scaled) from the outline (Clipper2's `SimplifyPaths`).
/// Rings that collapse are dropped.
pub fn simplify_ex(ex: &[ExPolygon], epsilon: f64) -> Vec<ExPolygon> {
    let ring = |r: &Polygon| from_path(&simplify_path(&to_path(r), epsilon, true));
    ex.iter()
        .filter_map(|e| {
            let contour = ring(&e.contour);
            (contour.len() >= 3).then(|| ExPolygon {
                contour,
                holes: e.holes.iter().map(ring).filter(|h| h.len() >= 3).collect(),
            })
        })
        .collect()
}

/// One closed ring with vertices closer than `epsilon` (scaled) to the outline removed
/// (Clipper2's `SimplifyPath`).
pub fn simplify_ring(ring: &[Point], epsilon: f64) -> Polygon {
    from_path(&simplify_path(&to_path(ring), epsilon, true))
}

/// Offsets by `delta1`, then the result by `delta2` (PrusaSlicer's `offset2_ex`).
pub fn offset2_ex(ex: &[ExPolygon], delta1: f64, delta2: f64, join: Join) -> Vec<ExPolygon> {
    offset_ex(&offset_ex(ex, delta1, join), delta2, join)
}

/// [`offset_ex`] rounding the offset vertices like PrusaSlicer's Clipper 6.4 (`FRound`: halves
/// upwards, where Clipper2 rounds them away from zero). Differs only for deltas that put vertices
/// on exact halves, such as `float(-min_perimeter_infill_spacing / 2.)` with an odd spacing.
pub fn offset_ex_half_up(ex: &[ExPolygon], delta: f64, join: Join) -> Vec<ExPolygon> {
    if ex.is_empty() {
        return Vec::new();
    }
    offset_cached_rounding(
        expolygons_to_paths(ex),
        delta,
        join.into(),
        EndType::Polygon,
        DEFAULT_MITER_LIMIT,
        0.0,
        true,
    )
}

/// [`offset2_ex`] with [`offset_ex_half_up`] rounding.
pub fn offset2_ex_half_up(
    ex: &[ExPolygon],
    delta1: f64,
    delta2: f64,
    join: Join,
) -> Vec<ExPolygon> {
    offset_ex_half_up(&offset_ex_half_up(ex, delta1, join), delta2, join)
}

/// Morphological opening: shrink by `delta`, then grow back. Drops parts narrower than `2 * delta`.
pub fn opening_ex(ex: &[ExPolygon], delta: f64) -> Vec<ExPolygon> {
    offset2_ex(ex, -delta, delta, Join::Miter)
}

/// Morphological closing: grow by `delta`, then shrink back. Fills gaps narrower than `2 * delta`.
pub fn closing_ex(ex: &[ExPolygon], delta: f64) -> Vec<ExPolygon> {
    offset2_ex(ex, delta, -delta, Join::Miter)
}

/// The area within `delta` of open polylines, with square joins and butt ends
/// (PrusaSlicer's `offset(Polylines, delta)` with its line defaults).
pub fn offset_polylines(lines: &[Polyline], delta: f64) -> Vec<ExPolygon> {
    let paths: Paths64 = lines
        .iter()
        .filter(|l| l.len() >= 2)
        .map(|l| to_path(l))
        .collect();
    if paths.is_empty() || delta <= 0.0 {
        return Vec::new();
    }
    offset_cached(paths, delta, JoinType::Square, EndType::Butt, 2.0, 0.0)
}

/// Parts of open lines outside the clip, retaining contour and hole segments.
pub fn diff_pl(lines: &[Polyline], clip: &[ExPolygon]) -> Vec<Polyline> {
    if lines.is_empty() {
        return Vec::new();
    }
    if clip.is_empty() {
        return lines.to_vec();
    }
    open_boolean(ClipType::Difference, lines, clip)
}

/// Clips open `lines` by `clip` (non-zero), without the clip rings entirely above or below
/// the lines (as [`prune_far_rings`]: the same result).
fn open_boolean(op: ClipType, lines: &[Polyline], clip_ex: &[ExPolygon]) -> Vec<Polyline> {
    let subject = polygons_to_paths(lines);
    let mut clip = expolygons_to_paths(clip_ex);
    match y_range(&subject) {
        Some(r) => retain_y_overlapping(&mut clip, r),
        None => clip.clear(),
    }
    if clip.is_empty() && op == ClipType::Intersection {
        return Vec::new();
    }
    let mut c = Clipper64::new();
    c.add_open_subject(&subject);
    c.add_clip(&clip);
    let mut closed = Paths64::new();
    let mut open = Paths64::new();
    c.execute(op, FillRule::NonZero, &mut closed, Some(&mut open));
    open.iter().map(from_path).collect()
}

/// The rings of some ExPolygons with their bounding boxes, cut quickly to many small rectangles:
/// where a whole layer's surface is clipped once per small island, only the rings near the
/// island take part (rectangle clipping, linear in the rings it touches) instead of the whole
/// layer in a sweep.
pub struct Cropper {
    rings: Vec<(Path64, super::BoundingBox, bool)>,
}

impl Cropper {
    pub fn new(ex: &[ExPolygon]) -> Self {
        Self {
            rings: ex
                .iter()
                .flat_map(|e| e.polygons())
                .filter_map(|r| {
                    let bb = super::BoundingBox::from_points(r.iter())?;
                    (r.len() >= 3).then(|| (to_path(r), bb, super::signed_area(r) > 0.))
                })
                .collect(),
        }
    }

    /// The area inside `rect` (the same region as an intersection with the rectangle; points
    /// on the rectangle's edges may round differently).
    pub fn crop(&self, rect: super::BoundingBox) -> Vec<ExPolygon> {
        let r = clipper2_rust::Rect64::new(rect.min.x, rect.min.y, rect.max.x, rect.max.y);
        let mut paths = Paths64::new();
        for (ring, bb, ccw) in &self.rings {
            if bb.max.x < rect.min.x
                || bb.min.x > rect.max.x
                || bb.max.y < rect.min.y
                || bb.min.y > rect.max.y
            {
                continue;
            }
            if bb.min.x >= rect.min.x
                && bb.max.x <= rect.max.x
                && bb.min.y >= rect.min.y
                && bb.max.y <= rect.max.y
            {
                paths.push(ring.clone());
                continue;
            }
            for mut piece in clipper2_rust::rect_clip_path_64(&r, ring) {
                // A piece of a ring turns the way the ring does (a hole around the whole
                // rectangle comes back as the rectangle).
                if (clipper2_rust::area(&piece) > 0.) != *ccw {
                    piece.reverse();
                }
                paths.push(piece);
            }
        }
        if paths.is_empty() {
            return Vec::new();
        }
        boolean_ex_uncached(ClipType::Union, &paths, &Paths64::new(), FillRule::NonZero)
    }
}

/// The parts of open `lines` inside `rect` (rectangle clipping; lines entirely inside are kept
/// as they are).
pub fn crop_lines(lines: &[&Polyline], rect: super::BoundingBox) -> Vec<Polyline> {
    let r = clipper2_rust::Rect64::new(rect.min.x, rect.min.y, rect.max.x, rect.max.y);
    let mut out = Vec::new();
    for line in lines {
        let Some(bb) = super::BoundingBox::from_points(line.iter()) else {
            continue;
        };
        if bb.min.x >= rect.min.x
            && bb.max.x <= rect.max.x
            && bb.min.y >= rect.min.y
            && bb.max.y <= rect.max.y
        {
            out.push((*line).clone());
        } else {
            out.extend(
                clipper2_rust::rect_clip_line_64(&r, &to_path(line))
                    .iter()
                    .map(from_path),
            );
        }
    }
    out
}

/// Parts of open `lines` that lie inside `clip`.
pub fn intersection_pl(lines: &[Polyline], clip: &[ExPolygon]) -> Vec<Polyline> {
    if lines.is_empty() || clip.is_empty() {
        return Vec::new();
    }
    open_boolean(ClipType::Intersection, lines, clip)
}

/// `diff_pl(lines, clip).is_empty()`: whether no part of any line lies outside `clip`.
///
/// Decided without clipping when no segment of a line touches or crosses a clip edge: such a line
/// is entirely inside or entirely outside, which the exact winding number of its first point tells
/// (Clipper's NonZero rule). Any contact falls back to [`diff_pl`], so the answer is always
/// Clipper's.
pub fn diff_pl_is_empty(lines: &[Polyline], clip: &[ExPolygon]) -> bool {
    if lines.is_empty() {
        return true;
    }
    if clip.is_empty() {
        return false;
    }
    for line in lines {
        match polyline_side(line, clip) {
            Side::Ignored | Side::Inside => {}
            Side::Outside => return false,
            Side::Contact => return diff_pl(lines, clip).is_empty(),
        }
    }
    true
}

/// `intersection_pl(lines, clip).is_empty()`: whether no part of any line lies inside `clip`, with
/// the exact no-contact fast path of [`diff_pl_is_empty`].
pub fn intersection_pl_is_empty(lines: &[Polyline], clip: &[ExPolygon]) -> bool {
    if lines.is_empty() || clip.is_empty() {
        return true;
    }
    for line in lines {
        match polyline_side(line, clip) {
            Side::Ignored | Side::Outside => {}
            Side::Inside => return false,
            Side::Contact => return intersection_pl(lines, clip).is_empty(),
        }
    }
    true
}

enum Side {
    /// Fewer than two distinct consecutive points: Clipper drops the open path.
    Ignored,
    Inside,
    Outside,
    /// A segment touches or crosses a clip edge (or vertex): only Clipper decides.
    Contact,
}

fn polyline_side(line: &[Point], clip: &[ExPolygon]) -> Side {
    let mut points: Vec<Point> = line.to_vec();
    points.dedup();
    if points.len() < 2 {
        return Side::Ignored;
    }
    let rings = || clip.iter().flat_map(|e| e.polygons());
    // An edge can touch a segment only where their bounding boxes overlap.
    let (lo, hi) = points.iter().fold((points[0], points[0]), |(lo, hi), p| {
        (
            Point::new(lo.x.min(p.x), lo.y.min(p.y)),
            Point::new(hi.x.max(p.x), hi.y.max(p.y)),
        )
    });
    for ring in rings() {
        for (c, d) in ring_edges(ring) {
            if c.x.max(d.x) < lo.x
                || c.x.min(d.x) > hi.x
                || c.y.max(d.y) < lo.y
                || c.y.min(d.y) > hi.y
            {
                continue;
            }
            if points.windows(2).any(|s| segments_touch(s[0], s[1], c, d)) {
                return Side::Contact;
            }
        }
    }
    // The first point is on no edge (the first segment touches none), so its winding number is
    // unambiguous: Dan Sunday's crossing rule with exact integer orientation tests.
    let p = points[0];
    let mut winding = 0i64;
    for ring in rings() {
        for (a, b) in ring_edges(ring) {
            if a.y <= p.y {
                if b.y > p.y && orientation(a, b, p) > 0 {
                    winding += 1;
                }
            } else if b.y <= p.y && orientation(a, b, p) < 0 {
                winding -= 1;
            }
        }
    }
    if winding != 0 {
        Side::Inside
    } else {
        Side::Outside
    }
}

fn ring_edges(ring: &[Point]) -> impl Iterator<Item = (Point, Point)> + '_ {
    let n = ring.len();
    (0..n).map(move |k| (ring[k], ring[(k + 1) % n]))
}

/// Sign of the cross product (b - a) × (p - a), exact.
fn orientation(a: Point, b: Point, p: Point) -> i32 {
    let cross = i128::from(b.x - a.x) * i128::from(p.y - a.y)
        - i128::from(b.y - a.y) * i128::from(p.x - a.x);
    cross.signum() as i32
}

/// Whether closed segments `ab` and `cd` share at least one point (exact).
fn segments_touch(a: Point, b: Point, c: Point, d: Point) -> bool {
    let on = |p: Point, q: Point, r: Point| {
        r.x >= p.x.min(q.x) && r.x <= p.x.max(q.x) && r.y >= p.y.min(q.y) && r.y <= p.y.max(q.y)
    };
    let (d1, d2) = (orientation(c, d, a), orientation(c, d, b));
    let (d3, d4) = (orientation(a, b, c), orientation(a, b, d));
    if d1 * d2 < 0 && d3 * d4 < 0 {
        return true;
    }
    (d1 == 0 && on(c, d, a))
        || (d2 == 0 && on(c, d, b))
        || (d3 == 0 && on(a, b, c))
        || (d4 == 0 && on(a, b, d))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::{SCALE, scale};

    fn square(a: i64) -> ExPolygon {
        ExPolygon {
            contour: vec![
                Point::new(a, a),
                Point::new(-a, a),
                Point::new(-a, -a),
                Point::new(a, -a),
            ],
            holes: vec![],
        }
    }

    #[test]
    fn cropping_matches_intersecting_with_the_rectangle() {
        // A big square with a grid of square holes, one hole around the whole rectangle and
        // one crossing its edge.
        let mut big = square(100_000);
        for i in -4..=4 {
            for j in -4..=4 {
                let (x, y) = (i * 20_000, j * 20_000);
                let mut h = vec![
                    Point::new(x - 5_000, y - 5_000),
                    Point::new(x - 5_000, y + 5_000),
                    Point::new(x + 5_000, y + 5_000),
                    Point::new(x + 5_000, y - 5_000),
                ];
                if (i, j) == (0, 0) {
                    h = vec![
                        Point::new(x - 9_000, y - 9_000),
                        Point::new(x - 9_000, y + 9_000),
                        Point::new(x + 9_000, y + 9_000),
                        Point::new(x + 9_000, y - 9_000),
                    ];
                }
                big.holes.push(h);
            }
        }
        let cropper = Cropper::new(std::slice::from_ref(&big));
        let area = |e: &[ExPolygon]| e.iter().map(|x| x.area_mm2()).sum::<f64>();
        for (min, max) in [
            ((-3_000, -3_000), (3_000, 3_000)),
            ((12_000, 12_000), (31_000, 27_000)),
            ((-150_000, -150_000), (150_000, 150_000)),
            ((90_000, -1_000), (120_000, 1_000)),
        ] {
            let rect = crate::geometry::BoundingBox {
                min: Point::new(min.0, min.1),
                max: Point::new(max.0, max.1),
            };
            let quad = ExPolygon {
                contour: vec![
                    Point::new(min.0, min.1),
                    Point::new(max.0, min.1),
                    Point::new(max.0, max.1),
                    Point::new(min.0, max.1),
                ],
                holes: vec![],
            };
            let want = intersection_ex(std::slice::from_ref(&big), &[quad]);
            let got = cropper.crop(rect);
            assert!((area(&want) - area(&got)).abs() < 1e-9, "{rect:?}");
            assert_eq!(want.len(), got.len(), "{rect:?}");
        }
        // Lines: the parts inside the rectangle, whole lines kept as they are.
        let line: Polyline = vec![Point::new(-50_000, 0), Point::new(50_000, 0)];
        let inner: Polyline = vec![Point::new(-1_000, 500), Point::new(1_000, 500)];
        let rect = crate::geometry::BoundingBox {
            min: Point::new(-2_000, -2_000),
            max: Point::new(2_000, 2_000),
        };
        let out = crop_lines(&[&line, &inner], rect);
        assert_eq!(out.len(), 2);
        assert_eq!(out[1], inner);
        let xs: Vec<i64> = out[0].iter().map(|p| p.x).collect();
        assert_eq!(
            (xs.iter().min(), xs.iter().max()),
            (Some(&-2_000), Some(&2_000))
        );
    }

    fn extent(ex: &[ExPolygon]) -> (i64, i64) {
        let xs = ex.iter().flat_map(|e| e.contour.iter().map(|p| p.x));
        (xs.clone().min().unwrap(), xs.max().unwrap())
    }

    #[test]
    fn half_up_offsets_round_like_clipper_6() {
        // The cube20 infill area (PrusaSlicer's upstream ClipperUtils on the same square gives
        // 9225445 / -9225443): the half-unit deltas of `min_perimeter_infill_spacing / 2.` round
        // upwards in Clipper 6.4, away from zero in Clipper2.
        let inner = [square(9_164_383)];
        let (d1, d2) = (-122_123.5, 183_184.5);
        assert_eq!(
            extent(&offset2_ex_half_up(&inner, d1, d2, Join::Miter)),
            (-9_225_443, 9_225_445)
        );
        assert_eq!(
            extent(&offset2_ex(&inner, d1, d2, Join::Miter)),
            (-9_225_445, 9_225_445)
        );
        // Whole deltas are unaffected.
        assert_eq!(
            offset_ex_half_up(&inner, -1000.0, Join::Miter),
            offset_ex(&inner, -1000.0, Join::Miter)
        );
    }

    #[test]
    fn polyline_emptiness_fast_paths_agree_with_clipper() {
        // A square with a square hole, both orientations of the hole, and lines that are inside,
        // outside, in the hole, crossing, touching edges and vertices, collinear with edges,
        // degenerate (one repeated point) and multi-segment.
        let mut hole = rect(4.0, 4.0, 6.0, 6.0);
        hole.reverse();
        let regions = [
            vec![ExPolygon {
                contour: rect(0.0, 0.0, 10.0, 10.0),
                holes: vec![hole.clone()],
            }],
            vec![ExPolygon {
                contour: rect(0.0, 0.0, 10.0, 10.0),
                holes: vec![rect(4.0, 4.0, 6.0, 6.0)],
            }],
            vec![
                ExPolygon {
                    contour: rect(0.0, 0.0, 10.0, 10.0),
                    holes: vec![],
                },
                ExPolygon {
                    contour: rect(8.0, 8.0, 12.0, 12.0),
                    holes: vec![],
                },
            ],
        ];
        let p = Point::from_mm;
        let lines: Vec<Polyline> = vec![
            vec![p(1.0, 1.0), p(2.0, 3.0)],
            vec![p(-1.0, 1.0), p(-2.0, 3.0)],
            vec![p(4.5, 4.5), p(5.5, 5.5)],
            vec![p(1.0, 1.0), p(11.0, 1.0)],
            vec![p(1.0, 1.0), p(10.0, 1.0)],
            vec![p(0.0, 0.0), p(2.0, 2.0)],
            vec![p(0.0, 2.0), p(0.0, 5.0)],
            vec![p(4.0, 4.0), p(4.0, 4.0)],
            vec![p(1.0, 1.0), p(3.0, 1.0), p(3.0, 3.0), p(9.0, 9.0)],
            vec![p(9.0, 9.0), p(11.0, 11.0)],
            vec![p(9.0, 9.0), p(9.5, 9.5)],
            vec![p(1.0, 1.0), p(1.0, 1.0), p(2.0, 2.0)],
        ];
        for clip in &regions {
            for line in &lines {
                let one = std::slice::from_ref(line);
                assert_eq!(
                    diff_pl_is_empty(one, clip),
                    diff_pl(one, clip).is_empty(),
                    "diff {line:?}"
                );
                assert_eq!(
                    intersection_pl_is_empty(one, clip),
                    intersection_pl(one, clip).is_empty(),
                    "intersection {line:?}"
                );
            }
            assert_eq!(
                diff_pl_is_empty(&lines, clip),
                diff_pl(&lines, clip).is_empty()
            );
            assert_eq!(
                intersection_pl_is_empty(&lines, clip),
                intersection_pl(&lines, clip).is_empty()
            );
        }
        // Random self-intersecting rings and lines on a coarse grid (many exact contacts).
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut next = |n: i64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % n as u64) as i64
        };
        for _ in 0..3000 {
            let mut ring = || -> Polygon {
                (0..3 + next(5))
                    .map(|_| Point::new(next(12) * 1000, next(12) * 1000))
                    .collect()
            };
            let contour = ring();
            let holes = if contour.len() % 2 == 0 {
                vec![ring()]
            } else {
                vec![]
            };
            let clip = vec![ExPolygon { contour, holes }];
            let line: Polyline = (0..2 + next(3))
                .map(|_| Point::new(next(12) * 1000 + next(2) * 500, next(12) * 1000))
                .collect();
            let one = std::slice::from_ref(&line);
            assert_eq!(diff_pl_is_empty(one, &clip), diff_pl(one, &clip).is_empty());
            assert_eq!(
                intersection_pl_is_empty(one, &clip),
                intersection_pl(one, &clip).is_empty()
            );
        }
        assert!(diff_pl_is_empty(&[], &regions[0]));
        assert!(!diff_pl_is_empty(&lines, &[]));
        assert!(intersection_pl_is_empty(&lines, &[]));
    }

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Polygon {
        vec![
            Point::from_mm(x0, y0),
            Point::from_mm(x1, y0),
            Point::from_mm(x1, y1),
            Point::from_mm(x0, y1),
        ]
    }

    fn total_area(ex: &[ExPolygon]) -> f64 {
        ex.iter().map(ExPolygon::area_mm2).sum()
    }

    #[test]
    fn positive_union_makes_holes_from_clockwise_loops() {
        let mut hole = rect(5.0, 5.0, 15.0, 15.0);
        hole.reverse();
        let ex = union_ex_positive(&[rect(0.0, 0.0, 20.0, 20.0), hole]);
        assert_eq!(ex.len(), 1);
        assert_eq!(ex[0].holes.len(), 1);
        assert!((total_area(&ex) - 300.0).abs() < 1e-9);
    }

    #[test]
    fn island_inside_hole_becomes_separate_expolygon() {
        let mut hole = rect(5.0, 5.0, 15.0, 15.0);
        hole.reverse();
        let island = rect(8.0, 8.0, 12.0, 12.0);
        let ex = union_ex_positive(&[rect(0.0, 0.0, 20.0, 20.0), hole, island]);
        assert_eq!(ex.len(), 2);
        assert!((total_area(&ex) - 316.0).abs() < 1e-9);
        // PolyTreeToExPolygons order: the outer polygon before the island in its hole.
        assert_eq!(ex[0].holes.len(), 1);
        assert!(ex[1].holes.is_empty());
    }

    #[test]
    fn inset_of_square_with_miter_is_exact() {
        let ex = union_ex_positive(&[rect(0.0, 0.0, 20.0, 20.0)]);
        let inset = offset_ex(&ex, -(scale(1.0) as f64), Join::Miter);
        assert!((total_area(&inset) - 324.0).abs() < 1e-6);
    }

    #[test]
    fn inset_too_far_vanishes() {
        let ex = union_ex_positive(&[rect(0.0, 0.0, 2.0, 2.0)]);
        assert!(offset_ex(&ex, -(scale(1.5) as f64), Join::Miter).is_empty());
    }

    #[test]
    fn diff_and_intersection() {
        let a = union_ex_positive(&[rect(0.0, 0.0, 10.0, 10.0)]);
        let b = union_ex_positive(&[rect(5.0, 0.0, 15.0, 10.0)]);
        assert!((total_area(&diff_ex(&a, &b)) - 50.0).abs() < 1e-9);
        assert!((total_area(&intersection_ex(&a, &b)) - 50.0).abs() < 1e-9);
        assert!((total_area(&union_ex(&[a[0].clone(), b[0].clone()])) - 150.0).abs() < 1e-9);
    }

    #[test]
    fn clipping_open_lines_keeps_inside_parts() {
        let mut hole = rect(4.0, 0.0, 6.0, 10.0);
        hole.reverse();
        let area = union_ex_positive(&[rect(0.0, 0.0, 10.0, 10.0), hole]);
        let line = vec![Point::from_mm(-5.0, 5.0), Point::from_mm(15.0, 5.0)];
        let parts = intersection_pl(&[line], &area);
        assert_eq!(parts.len(), 2);
        let len: f64 = parts
            .iter()
            .map(|p| crate::geometry::polyline_length(p))
            .sum::<f64>()
            / SCALE;
        assert!((len - 8.0).abs() < 1e-6);
    }

    #[test]
    fn runs_beyond_the_range_keep_their_end_vertices() {
        // 0..7: up the right side into a peak far above the range (-5, 15), down the left.
        let ys = [0, 0, 20, 30, 40, 50, 30, 20];
        let keep = kept_vertices(ys.len(), |i| ys[i], (-5, 15)).unwrap();
        // The run 2..7 keeps its ends and the vertex after/before each (3 and 6).
        assert_eq!(keep, [true, true, true, true, false, false, true, true]);
        // A horizontal step from a run's end keeps going to the next height change.
        let ys = [0, 0, 20, 20, 40, 50, 30, 20];
        let keep = kept_vertices(ys.len(), |i| ys[i], (-5, 15)).unwrap();
        assert_eq!(keep, [true, true, true, true, true, false, true, true]);
        // Entirely beyond the range: dropped.
        assert_eq!(kept_vertices(3, |i| [20, 30, 40][i], (-5, 15)), None);
    }

    #[test]
    fn shortened_clip_gives_the_same_booleans() {
        // A wavy whole-layer-like ring spanning +-50 mm with many vertices far above and below
        // a small subject, and a hole; both engines, difference and intersection.
        let wave: Polygon = (0..400)
            .map(|i| {
                let a = std::f64::consts::TAU * f64::from(i) / 400.;
                let r = 40. + 8. * (13. * a).sin();
                Point::from_mm(r * a.cos(), r * a.sin() * 1.25)
            })
            .collect();
        let mut hole: Polygon = (0..60)
            .map(|i| {
                let a = std::f64::consts::TAU * f64::from(i) / 60.;
                Point::from_mm(3. + 9. * a.cos(), -2. + 30. * a.sin())
            })
            .collect();
        hole.reverse();
        let clip = [ExPolygon {
            contour: wave,
            holes: vec![hole],
        }];
        let subject = [ExPolygon {
            contour: (0..24)
                .map(|i| {
                    let a = std::f64::consts::TAU * f64::from(i) / 24.;
                    Point::from_mm(35. + 12. * a.cos(), 4. + 6. * a.sin())
                })
                .collect(),
            holes: vec![],
        }];
        let s = expolygons_to_paths(&subject);
        let c = expolygons_to_paths(&clip);
        let shortened = expolygons_to_paths_in(&clip, y_range(&s));
        let count = |p: &Paths64| p.iter().map(Vec::len).sum::<usize>();
        assert!(count(&shortened) * 2 < count(&c));
        for op in [ClipType::Difference, ClipType::Intersection] {
            let full = boolean_ex_uncached(op, &s, &c, FillRule::NonZero);
            assert!(!full.is_empty());
            assert_eq!(
                boolean_ex_uncached(op, &s, &shortened, FillRule::NonZero),
                full
            );
            let full64 = super::super::clip64::execute_tree(op, &s, &c, FillRule::NonZero);
            assert_eq!(
                super::super::clip64::execute_tree(op, &s, &shortened, FillRule::NonZero),
                full64
            );
            assert_eq!(
                super::super::clip64::boolean_ex(op, &subject, &clip),
                full64
            );
        }
        assert_eq!(
            diff_ex(&subject, &clip),
            boolean_ex_uncached(ClipType::Difference, &s, &c, FillRule::NonZero)
        );
    }
}
