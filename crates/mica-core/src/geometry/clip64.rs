// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: prusaslicer@b028299c77 src/libslic3r/ClipperUtils.cpp :: raw_offset, expand_paths, shrink_paths, offset_paths, offset_expolygon_inner, expolygons_offset, expolygons_offset_pt, offset, offset_ex, offset2_ex, clipper_union, clipper_do
// Changes: PrusaSlicer's ClipperUtils compositions on the Clipper 6.4 offsetter port (`ClipperOffset64`)
// Changes: and this workspace's Clipper engine with Clipper 6.4 intersections. Trees are built by a
// Changes: non-zero union of the composed paths (the same polygons PolyTreeToExPolygons yields).
// Changes: `offset_polygons_ex` takes jtRound's arc tolerance in the miter-limit slot, as ClipperUtils' `raw_offset`; `boolean_ex` is `clipper_do` with non-zero fills.
// Changes: `boolean_ex` leaves out operand rings entirely above or below the other operand (`clip::prune_far_rings`, the same result).
// Changes: `boolean_ex` builds the clip paths without their stretches beyond the subject's Y range (`clip::expolygons_to_paths_in`).

//! PrusaSlicer-compatible offsets ("Clipper 6.4 flavour"), used for PrusaSlicer profiles.
//!
//! The geometry of Mica's default offsets (Clipper2) differs from PrusaSlicer's (Clipper 6.4) by a unit
//! here and there: Clipper2 truncates edge intersections where Clipper 6.4 rounds them, and the two
//! offsetters build their joins differently. Those units decide exact ties downstream, for example
//! which of two equally long boundary arcs a grid infill connects first.

use clipper2_rust::{
    ClipType, Clipper64, EndType, FillRule, JoinType, Paths64, Point64, PolyTree64, raw_offset64,
    with_clipper64_intersections,
};

use super::clip::Join;
use super::{ExPolygon, Point, Polygon};

fn to_path(points: &[Point]) -> Vec<Point64> {
    points.iter().map(|p| Point64::new(p.x, p.y)).collect()
}

/// `ClipperUtils`' float offsets: deltas are passed as `float`.
fn f(delta: f64) -> f64 {
    f64::from(delta as f32)
}

fn join(j: Join) -> JoinType {
    match j {
        Join::Miter => JoinType::Miter,
        Join::Round => JoinType::Round,
        Join::Square => JoinType::Square,
    }
}

/// A union (or other boolean) on the Clipper 6.4-compatible engine.
pub(crate) fn execute(op: ClipType, subject: &Paths64, clip: &Paths64, rule: FillRule) -> Paths64 {
    with_clipper64_intersections(true, || {
        let mut c = Clipper64::new();
        c.set_preserve_collinear(false);
        c.add_subject(subject);
        if !clip.is_empty() {
            c.add_clip(clip);
        }
        let mut out = Paths64::new();
        c.execute(op, rule, &mut out, None);
        out
    })
}

/// [`execute`] into ExPolygons (`PolyTreeToExPolygons`).
pub(crate) fn execute_tree(
    op: ClipType,
    subject: &Paths64,
    clip: &Paths64,
    rule: FillRule,
) -> Vec<ExPolygon> {
    with_clipper64_intersections(true, || {
        let mut c = Clipper64::new();
        c.set_preserve_collinear(false);
        c.add_subject(subject);
        if !clip.is_empty() {
            c.add_clip(clip);
        }
        let mut tree = PolyTree64::new();
        let mut open = Paths64::new();
        c.execute_tree(op, rule, &mut tree, &mut open);
        super::clip::tree_to_expolygons(&tree)
    })
}

fn raw(paths: &Paths64, delta: f64, j: Join, miter: f64, arc: f64) -> Paths64 {
    raw_offset64(paths, delta, join(j), EndType::Polygon, miter, arc)
}

/// `expand_paths` / `shrink_paths` (`offset_paths`), as paths.
fn offset_paths(paths: &Paths64, delta: f64, j: Join, miter: f64) -> Paths64 {
    offset_paths_arc(paths, delta, j, miter, 0.25)
}

/// [`offset_paths`] with an explicit round-join arc tolerance.
fn offset_paths_arc(paths: &Paths64, delta: f64, j: Join, miter: f64, arc: f64) -> Paths64 {
    if delta > 0. {
        execute(
            ClipType::Union,
            &raw(paths, delta, j, miter, arc),
            &Paths64::new(),
            FillRule::NonZero,
        )
    } else {
        let raw = raw(paths, delta, j, miter, arc);
        if raw.is_empty() {
            return raw;
        }
        let r = clipper2_rust::get_bounds_paths(&raw);
        let (l, t, rr, b) = (r.left - 10, r.top - 10, r.right + 10, r.bottom + 10);
        let outer = vec![
            Point64::new(l, b),
            Point64::new(rr, b),
            Point64::new(rr, t),
            Point64::new(l, t),
        ];
        let mut subject = raw;
        subject.push(outer);
        let mut out = with_clipper64_intersections(true, || {
            let mut c = Clipper64::new();
            c.set_preserve_collinear(false);
            c.add_subject(&subject);
            c.set_reverse_solution(true);
            let mut out = Paths64::new();
            c.execute(ClipType::Union, FillRule::Negative, &mut out, None);
            out
        });
        if let Some(i) = out.iter().position(|p| {
            p.len() == 4
                && p.iter()
                    .all(|q| (q.x == l || q.x == rr) && (q.y == t || q.y == b))
        }) {
            out.remove(i);
        }
        out
    }
}

fn tree(paths: &Paths64) -> Vec<ExPolygon> {
    execute_tree(ClipType::Union, paths, &Paths64::new(), FillRule::NonZero)
}

/// `offset_expolygon_inner`: the contour, and each hole on its own, by a fresh offsetter.
fn offset_expolygon_inner(
    ex: &ExPolygon,
    delta: f64,
    j: Join,
    miter: f64,
    out: &mut Paths64,
) -> bool {
    let off = |p: &Polygon, d: f64| {
        let mut co = clipper2_rust::ClipperOffset64::new(miter, 0.25, (d * 0.005).abs());
        co.add_path(&to_path(p), join(j), EndType::Polygon);
        co.execute(d)
    };
    let contours = off(&ex.contour, delta);
    if contours.is_empty() {
        return false;
    }
    let mut holes = Paths64::new();
    for h in &ex.holes {
        holes.extend(off(h, -delta));
    }
    if holes.is_empty() {
        out.extend(contours);
    } else if delta < 0. {
        let diff = execute(ClipType::Difference, &contours, &holes, FillRule::NonZero);
        if diff.is_empty() {
            return false;
        }
        out.extend(diff);
    } else {
        out.extend(contours);
        for mut h in holes {
            h.reverse();
            out.push(h);
        }
    }
    true
}

fn expolygons_offset_raw(ex: &[ExPolygon], delta: f64, j: Join, miter: f64) -> (Paths64, usize) {
    let mut out = Paths64::new();
    let mut n = 0;
    for e in ex {
        n += usize::from(offset_expolygon_inner(e, delta, j, miter, &mut out));
    }
    (out, n)
}

/// `expolygons_offset`: united only for an outward offset of several ExPolygons.
fn expolygons_offset(ex: &[ExPolygon], delta: f64, j: Join, miter: f64) -> Paths64 {
    let (out, n) = expolygons_offset_raw(ex, delta, j, miter);
    if n > 1 && delta > 0. {
        execute(ClipType::Union, &out, &Paths64::new(), FillRule::NonZero)
    } else {
        out
    }
}

/// `offset_ex(const ExPolygons&, float delta, …)`.
pub fn offset_ex(ex: &[ExPolygon], delta: f64, j: Join, miter: f64) -> Vec<ExPolygon> {
    if ex.is_empty() {
        return Vec::new();
    }
    tree(&expolygons_offset_raw(ex, f(delta), j, miter).0)
}

/// `offset2_ex(const ExPolygons&, float delta1, float delta2, …)`.
pub fn offset2_ex(
    ex: &[ExPolygon],
    delta1: f64,
    delta2: f64,
    j: Join,
    miter: f64,
) -> Vec<ExPolygon> {
    if ex.is_empty() {
        return Vec::new();
    }
    let first = expolygons_offset(ex, f(delta1), j, miter);
    if first.is_empty() {
        return Vec::new();
    }
    tree(&offset_paths(&first, f(delta2), j, miter))
}

/// `offset(const Polygons&, float delta, …)` (orientation encodes holes).
pub fn offset(polys: &[Polygon], delta: f64, j: Join, miter: f64) -> Vec<Polygon> {
    let paths: Paths64 = polys.iter().map(|p| to_path(p)).collect();
    if paths.is_empty() {
        return Vec::new();
    }
    offset_paths(&paths, f(delta), j, miter)
        .iter()
        .map(|p| p.iter().map(|q| Point::new(q.x, q.y)).collect())
        .collect()
}

/// `union_ex(const Polygons&, fill_type)`.
pub fn union_ex(polys: &[Polygon], rule: FillRule) -> Vec<ExPolygon> {
    let paths: Paths64 = polys.iter().map(|p| to_path(p)).collect();
    execute_tree(ClipType::Union, &paths, &Paths64::new(), rule)
}

/// `offset(const Polygons&, float delta, joinType, miterLimit)` into ExPolygons, where for
/// `jtRound` the last argument is ClipperUtils' arc tolerance (`raw_offset`), as the tree
/// support code passes it. The polygons of `ex` (holes clockwise) are offset path by path.
pub fn offset_polygons_ex(
    ex: &[ExPolygon],
    delta: f64,
    j: Join,
    miter_or_arc: f64,
) -> Vec<ExPolygon> {
    let paths: Paths64 = ex
        .iter()
        .flat_map(|e| e.polygons())
        .map(|p| to_path(p))
        .collect();
    if paths.is_empty() {
        return Vec::new();
    }
    let (miter, arc) = if j == Join::Round {
        (2., miter_or_arc)
    } else {
        (miter_or_arc, 0.)
    };
    tree(&offset_paths_arc(&paths, f(delta), j, miter, arc))
}

/// Boolean of ExPolygons (holes clockwise) on the Clipper 6.4-compatible engine, non-zero
/// fill for both operands (ClipperUtils `clipper_do`), as ExPolygons.
pub fn boolean_ex(op: ClipType, subject: &[ExPolygon], clip: &[ExPolygon]) -> Vec<ExPolygon> {
    let paths = |a: &[ExPolygon]| -> Paths64 {
        a.iter()
            .flat_map(|e| e.polygons())
            .map(|p| to_path(p))
            .collect()
    };
    if subject.is_empty() {
        return Vec::new();
    }
    let mut subject = paths(subject);
    let mut clip = match op {
        ClipType::Difference | ClipType::Intersection => {
            super::clip::expolygons_to_paths_in(clip, super::clip::y_range(&subject))
        }
        _ => paths(clip),
    };
    // The same engine, so the same exact pruning as Mica's own booleans.
    super::clip::prune_far_rings(op, &mut subject, &mut clip);
    if subject.is_empty() {
        return Vec::new();
    }
    execute_tree(op, &subject, &clip, FillRule::NonZero)
}
