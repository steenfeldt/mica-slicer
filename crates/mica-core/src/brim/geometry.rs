// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/Brim.cpp :: top_level_outer_brim_area, make_brim, inner_brim_area, make_inner_brim
// Source: prusaslicer@b028299c77 src/libslic3r/Print.cpp :: Print::_make_skirt
// Source: orca@8500fcdcca src/libslic3r/Print.cpp :: Print::_make_skirt
// Source: orca@8500fcdcca src/libslic3r/GCode.cpp :: Skirt::find_start_point
// Changes: Clipper2 offsets; containment booleans instead of Z-tagged Clipper trees; deterministic nesting order.
// Source: prusaslicer@b028299c77 src/libslic3r/ClipperUtils.cpp :: union_pt_chained_outside_in
// Source: prusaslicer@b028299c77 src/libslic3r/ShortestPath.cpp :: chain_polylines
// Changes: Prusa brim loops are ordered outside in along a containment tree (chain_points per level)
// Changes: and each loop's clipped pieces by chain_polylines.
// Copyright (c) Prusa Research 2021 - 2023 Vojtěch Bubník, Lukáš Matěna, Lukáš Hejl
// Copyright (c) SuperSlicer 2023 Remi Durand

use super::*;
use crate::geometry::clip::{
    Join, diff_ex, intersection_ex, intersection_pl, offset_ex_with, simplify_ex, to_polygons,
    union_ex,
};
use crate::geometry::{Polyline, SCALE, polygon_length, signed_area};

pub fn convex_hull(mut points: Vec<Point>) -> Polygon {
    points.sort_by_key(|p| (p.x, p.y));
    points.dedup();
    if points.len() < 3 {
        return points;
    }
    let cross = |a: Point, b: Point, c: Point| {
        (b.x - a.x) as i128 * (c.y - a.y) as i128 - (b.y - a.y) as i128 * (c.x - a.x) as i128
    };
    let mut lower = Vec::new();
    let mut upper = Vec::new();
    for (p, q) in points.iter().zip(points.iter().rev()) {
        for (v, p) in [(&mut lower, *p), (&mut upper, *q)] {
            while v.len() >= 2 && cross(v[v.len() - 2], v[v.len() - 1], p) <= 0 {
                v.pop();
            }
            v.push(p);
        }
    }
    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

pub(super) fn solid(p: &Polygon) -> ExPolygon {
    let mut p = p.clone();
    if signed_area(&p) < 0.0 {
        p.reverse();
    }
    ExPolygon {
        contour: p,
        holes: Vec::new(),
    }
}
pub(super) fn closed(mut p: Polygon) -> Polyline {
    if let Some(&first) = p.first() {
        p.push(first);
    }
    p
}
pub(super) fn offset(area: &[ExPolygon], mm: f64, join: Join) -> Vec<ExPolygon> {
    offset_ex_with(area, mm * SCALE, join, 3.0, 0.0125 * SCALE)
}
fn simplify(area: &[ExPolygon], resolution: f64) -> Vec<ExPolygon> {
    simplify_ex(area, resolution * SCALE)
}
pub(super) fn loops_to_paths(
    mut loops: Vec<Polygon>,
    flow: Flow,
    inner_first: bool,
) -> Vec<ExtrusionPath> {
    // Containment depth is monotonic in absolute area; stable sort retains deterministic peer order.
    loops.sort_by(|a, b| {
        let ord = signed_area(b).abs().total_cmp(&signed_area(a).abs());
        if inner_first { ord.reverse() } else { ord }
    });
    loop_paths(loops, flow)
}

/// Closed CCW brim paths of `loops`, in the given order.
fn loop_paths(loops: Vec<Polygon>, flow: Flow) -> Vec<ExtrusionPath> {
    loops
        .into_iter()
        .filter(|p| p.len() >= 3)
        .map(|mut p| {
            if signed_area(&p) < 0.0 {
                p.reverse();
            }
            path(closed(p), Role::Brim, flow)
        })
        .collect()
}

/// `union_pt_chained_outside_in` for disjoint loops: the containment tree walked depth first, each
/// level's loops chained by the first point of their contours (`chain_clipper_polynodes`).
fn chained_outside_in(loops: Vec<Polygon>) -> Vec<Polygon> {
    let loops: Vec<Polygon> = loops.into_iter().filter(|p| p.len() >= 3).collect();
    let areas: Vec<f64> = loops.iter().map(|p| signed_area(p).abs()).collect();
    let inside = |p: Point, poly: &Polygon| {
        let mut odd = false;
        for (a, b) in poly.iter().zip(poly.iter().cycle().skip(1)) {
            if (a.y > p.y) != (b.y > p.y)
                && ((p.x - a.x) as f64)
                    < (b.x - a.x) as f64 * (p.y - a.y) as f64 / (b.y - a.y) as f64
            {
                odd = !odd;
            }
        }
        odd
    };
    let mut children = vec![Vec::new(); loops.len() + 1];
    for i in 0..loops.len() {
        let parent = (0..loops.len())
            .filter(|&j| areas[j] > areas[i] && inside(loops[i][0], &loops[j]))
            .min_by(|&a, &b| areas[a].total_cmp(&areas[b]));
        children[parent.unwrap_or(loops.len())].push(i);
    }
    fn walk(node: usize, loops: &[Polygon], children: &[Vec<usize>], out: &mut Vec<usize>) {
        let kids = &children[node];
        let firsts: Vec<Point> = kids.iter().map(|&k| loops[k][0]).collect();
        for k in crate::shortest_path::chain_points(&firsts, None) {
            out.push(kids[k]);
            walk(kids[k], loops, children, out);
        }
    }
    let mut order = Vec::with_capacity(loops.len());
    walk(loops.len(), &loops, &children, &mut order);
    let mut src: Vec<Option<Polygon>> = loops.into_iter().map(Some).collect();
    order.into_iter().filter_map(|i| src[i].take()).collect()
}

fn outer(kind: BrimType) -> bool {
    matches!(kind, BrimType::Outer | BrimType::OuterAndInner)
}
fn inner(kind: BrimType) -> bool {
    matches!(kind, BrimType::Inner | BrimType::OuterAndInner)
}

fn nested(input: &AdhesionInput<'_>, index: usize) -> bool {
    let island = &input.islands[index];
    let own = [solid(&island.polygon.contour)];
    input.islands.iter().enumerate().any(|(j, other)| {
        index != j
            && other.object_id != island.object_id
            && signed_area(&other.polygon.contour).abs()
                > signed_area(&island.polygon.contour).abs()
            && diff_ex(&own, &[solid(&other.polygon.contour)]).is_empty()
    })
}

pub(super) fn outer_brim(input: &AdhesionInput<'_>) -> (Vec<ExtrusionPath>, Vec<ExPolygon>) {
    let spacing = input.flow.spacing();
    let mut bases = Vec::new();
    let mut areas = Vec::new();
    let mut excluded = Vec::new();
    let mut width: f64 = 0.0;
    for (i, island) in input.islands.iter().enumerate() {
        let b = island.brim;
        let contour = [solid(&island.polygon.contour)];
        let base = offset(&contour, b.separation, Join::Square);
        if outer(b.kind) && !nested(input, i) && b.width >= spacing {
            bases.extend(simplify(&base, input.resolution));
            areas.extend(diff_ex(
                &offset(&contour, b.width + b.separation, Join::Square),
                &base,
            ));
            width = width.max(b.width);
        }
        // Contours are solid here: outer brim may not enter another object's islands or holes.
        excluded.extend(contour);
        if b.kind != BrimType::None {
            excluded.extend(base);
        }
        if matches!(b.kind, BrimType::Inner | BrimType::None) {
            excluded.extend(offset(
                std::slice::from_ref(island.polygon),
                spacing,
                Join::Square,
            ));
        }
    }
    let area = diff_ex(&areas, &excluded);
    let mut loops = Vec::new();
    let mut islands = union_ex(&bases);
    for _ in 0..(width / spacing).floor() as usize {
        islands = simplify(&offset(&islands, spacing, Join::Square), input.resolution);
        loops.extend(to_polygons(&offset(&islands, -0.5 * spacing, Join::Miter)));
    }
    // make_brim: loops chained outside in, each loop's pieces inside the brim area by chain_polylines.
    let paths = loop_paths(chained_outside_in(loops), input.flow)
        .into_iter()
        .flat_map(|p| {
            let pieces: Vec<Polyline> = intersection_pl(&[p.points], &area)
                .into_iter()
                .filter(|l| !l.is_empty())
                .collect();
            crate::shortest_path::chain_polylines(pieces, None)
                .into_iter()
                .map(|points| path(points, Role::Brim, input.flow))
                .collect::<Vec<_>>()
        })
        .collect();
    (connect_lines(paths, &area, input.flow), area)
}

pub(super) fn inner_brim(input: &AdhesionInput<'_>) -> Vec<ExtrusionPath> {
    let spacing = input.flow.spacing();
    let mut loops = Vec::new();
    let mut normal = Vec::new();
    let mut forbidden = Vec::new();
    let mut all_holes = Vec::new();
    for island in input.islands {
        let b = island.brim;
        forbidden.extend(offset(
            std::slice::from_ref(island.polygon),
            b.separation,
            Join::Square,
        ));
        if matches!(b.kind, BrimType::Inner | BrimType::None) {
            forbidden.extend(diff_ex(
                &offset(&[solid(&island.polygon.contour)], spacing, Join::Square),
                &island.polygon.holes.iter().map(solid).collect::<Vec<_>>(),
            ));
        }
        for hole in &island.polygon.holes {
            let hole = [solid(hole)];
            all_holes.extend(hole.clone());
            if !inner(b.kind) {
                forbidden.extend(diff_ex(&hole, &offset(&hole, -spacing, Join::Square)));
                continue;
            }
            let occupied = input.islands.iter().any(|other| {
                !intersection_ex(&hole, &[solid(&other.polygon.contour)]).is_empty()
                    && other.polygon != island.polygon
            });
            let start = offset(&hole, -b.separation, Join::Square);
            if occupied {
                normal.extend(diff_ex(
                    &start,
                    &offset(&hole, -b.separation - b.width, Join::Square),
                ));
            } else {
                let mut inset = offset(&start, -spacing * 0.5, Join::Square);
                for _ in 0..(b.width / spacing).floor() as usize {
                    inset = simplify(&inset, input.resolution);
                    loops.extend(to_polygons(&inset));
                    inset = offset(&inset, -spacing, Join::Square);
                    if inset.is_empty() {
                        break;
                    }
                }
            }
        }
    }
    // An object placed in a surrounding object's hole has its outer brim in the inner-brim pass.
    for (i, island) in input.islands.iter().enumerate() {
        if nested(input, i) && outer(island.brim.kind) {
            let p = [solid(&island.polygon.contour)];
            let b = island.brim;
            normal.extend(diff_ex(
                &offset(&p, b.width + b.separation, Join::Square),
                &offset(&p, b.separation, Join::Square),
            ));
        }
    }
    let normal = diff_ex(&intersection_ex(&normal, &all_holes), &forbidden);
    let mut inset = offset(&normal, -spacing * 0.5, Join::Square);
    while !inset.is_empty() {
        inset = simplify(&inset, input.resolution);
        loops.extend(to_polygons(&inset));
        inset = offset(&inset, -spacing, Join::Square);
    }
    // make_inner_brim: union_pt_chained_outside_in, reversed.
    let mut loops = chained_outside_in(loops);
    loops.reverse();
    loop_paths(loops, input.flow)
}

pub(super) fn skirt(
    input: &AdhesionInput<'_>,
    brim_area: &[ExPolygon],
) -> (Vec<ExtrusionPath>, Vec<ExPolygon>) {
    let s = input.settings;
    let spacing = input.flow.spacing();
    let count = if s.draft_shield == DraftShield::Enabled {
        s.skirts.max(1)
    } else {
        s.skirts
    };
    let enabled = count > 0 && (s.skirt_height > 0 || s.draft_shield == DraftShield::Enabled);
    // Orca 2.4.1's zero-loop auto-brim footprint still includes skirt distance.
    let zero_loop_footprint = s.orca && s.brim.kind != BrimType::None && !enabled;
    if !enabled && !zero_loop_footprint {
        return (Vec::new(), Vec::new());
    }
    let mut by_object = std::collections::BTreeMap::<u64, Vec<Point>>::new();
    for hull in input.skirt_hulls {
        by_object
            .entry(hull.object_id)
            .or_default()
            .extend_from_slice(&hull.points);
    }
    // The tower reserves space and joins colliding skirts, but never owns a per-object skirt.
    let mut emit: Vec<_> = by_object.keys().map(|&id| id != u64::MAX).collect();
    let mut groups: Vec<Vec<Point>> = by_object.into_values().collect();
    if groups.is_empty() {
        groups.push(Vec::new());
        emit.push(false);
    }
    if s.draft_shield == DraftShield::Disabled {
        // Assign each brim component to the closest object's hull before collision merging.
        for ex in brim_area {
            if let Some(&point) = ex.contour.first() {
                let i = groups
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, pts)| {
                        pts.iter()
                            .map(|p| p.distance_sq(point))
                            .min()
                            .unwrap_or(i128::MAX)
                    })
                    .map_or(0, |(i, _)| i);
                groups[i].extend_from_slice(&ex.contour);
            }
        }
    }
    if !s.per_object {
        groups = vec![groups.into_iter().flatten().collect()];
        emit = vec![emit.iter().any(|v| *v)];
    } else {
        loop {
            let envelopes: Vec<_> = groups
                .iter()
                .zip(&emit)
                .map(|(p, emit)| {
                    let hull = vec![solid(&convex_hull(p.clone()))];
                    if *emit {
                        offset(
                            &hull,
                            s.skirt_distance + count as f64 * spacing,
                            Join::Round,
                        )
                    } else {
                        hull
                    }
                })
                .collect();
            let pair = (0..groups.len()).find_map(|i| {
                ((i + 1)..groups.len())
                    .find(|&j| !intersection_ex(&envelopes[i], &envelopes[j]).is_empty())
                    .map(|j| (i, j))
            });
            let Some((i, j)) = pair else { break };
            let other = groups.remove(j);
            groups[i].extend(other);
            let other = emit.remove(j);
            emit[i] |= other;
        }
    }
    let mut paths = Vec::new();
    let mut area = Vec::new();
    for (points, emit) in groups.into_iter().zip(emit) {
        if !emit {
            continue;
        }
        let hull = convex_hull(points);
        if hull.len() < 3 {
            continue;
        }
        let base = [solid(&hull)];
        let mut distance = s.skirt_distance - spacing * 0.5;
        let mut group_paths = Vec::new();
        let mut tool = 0;
        let mut used = 0.0;
        if enabled {
            loop {
                distance += spacing;
                let ring = offset_ex_with(&base, distance * SCALE, Join::Round, 3.0, 0.1 * SCALE);
                let ring = simplify(&ring, 0.05);
                let Some(poly) = ring.first() else {
                    break;
                };
                let mut points = poly.contour.clone();
                if s.orca {
                    start_at_angle(&mut points, s.skirt_start_angle);
                }
                let len = polygon_length(&points) / SCALE;
                group_paths.push(path(closed(points), Role::Skirt, input.flow));
                let mut enough = true;
                if s.min_skirt_length > 0.0
                    && let Some(t) = input.tools.get(tool)
                {
                    used += len * input.flow.mm3_per_mm() * t.extrusion_multiplier
                        / (std::f64::consts::PI * (t.filament_diameter / 2.0).powi(2));
                    enough = used >= s.min_skirt_length;
                    if enough && tool + 1 < input.tools.len() {
                        tool += 1;
                        used = 0.0;
                        enough = false;
                    }
                }
                if group_paths.len() >= count && enough {
                    break;
                }
            }
        }
        group_paths.reverse();
        paths.extend(group_paths);
        area.extend(offset_ex_with(
            &base,
            (distance + spacing * 0.5) * SCALE,
            Join::Round,
            3.0,
            0.1 * SCALE,
        ));
    }
    (paths, area)
}

fn start_at_angle(poly: &mut Polygon, angle: f64) {
    let Some(bb) = BoundingBox::from_points(poly.iter()) else {
        return;
    };
    let cx = (bb.min.x + bb.max.x) as f64 * 0.5;
    let cy = (bb.min.y + bb.max.y) as f64 * 0.5;
    let r = ((bb.max.x - bb.min.x) as f64).hypot((bb.max.y - bb.min.y) as f64) * 0.5;
    let target = Point::new(
        (cx + r * angle.to_radians().cos()) as i64,
        (cy + r * angle.to_radians().sin()) as i64,
    );
    // Project onto edges, like ExtrusionLoop::split_at, rather than snapping to a distant vertex.
    let mut best = (i128::MAX, 0, Point::default());
    for i in 0..poly.len() {
        let a = poly[i];
        let b = poly[(i + 1) % poly.len()];
        let dx = (b.x - a.x) as f64;
        let dy = (b.y - a.y) as f64;
        let t = (((target.x - a.x) as f64 * dx + (target.y - a.y) as f64 * dy)
            / (dx * dx + dy * dy))
            .clamp(0.0, 1.0);
        let p = Point::new(a.x + (t * dx).round() as i64, a.y + (t * dy).round() as i64);
        if p.distance_sq(target) < best.0 {
            best = (p.distance_sq(target), i, p);
        }
    }
    let n = poly.len();
    poly.rotate_left((best.1 + 1) % n);
    if poly.first() != Some(&best.2) {
        poly.insert(0, best.2);
    }
    poly.dedup();
    if poly.first() == poly.last() {
        poly.pop();
    }
}

// Used by regression tests and footprint checks.
#[cfg(test)]
pub(super) fn path_length(paths: &[ExtrusionPath]) -> f64 {
    paths
        .iter()
        .map(|p| crate::geometry::polyline_length(&p.points) / SCALE)
        .sum()
}

/// Upstream's short open-path connections. A segment scan replaces its EdgeGrid acceleration;
/// the shortened connector must cross neither a brim boundary nor any generated centerline.
pub(super) fn connect_lines(
    mut paths: Vec<ExtrusionPath>,
    area: &[ExPolygon],
    flow: Flow,
) -> Vec<ExtrusionPath> {
    let mut previous = None;
    for p in &mut paths {
        if let Some(end) = previous
            && p.points.first() != p.points.last()
            && p.points.last().unwrap().distance_sq(end) < p.points[0].distance_sq(end)
        {
            p.points.reverse();
        }
        previous = p.points.last().copied();
    }
    let boundaries = offset(area, 0.0001, Join::Miter);
    let mut segments: Vec<_> = paths
        .iter()
        .flat_map(|p| p.points.windows(2).map(|p| (p[0], p[1])))
        .collect();
    for p in boundaries.iter().flat_map(ExPolygon::polygons) {
        segments.extend(
            p.iter()
                .zip(p.iter().cycle().skip(1))
                .take(p.len())
                .map(|(&a, &b)| (a, b)),
        );
    }
    let cross = |a: Point, b: Point, c: Point| {
        (b.x - a.x) as i128 * (c.y - a.y) as i128 - (b.y - a.y) as i128 * (c.x - a.x) as i128
    };
    let intersects = |a: Point, b: Point, c: Point, d: Point| {
        if a.x.min(b.x) > c.x.max(d.x)
            || c.x.min(d.x) > a.x.max(b.x)
            || a.y.min(b.y) > c.y.max(d.y)
            || c.y.min(d.y) > a.y.max(b.y)
        {
            return false;
        }
        cross(a, b, c).signum() * cross(a, b, d).signum() <= 0
            && cross(c, d, a).signum() * cross(c, d, b).signum() <= 0
    };
    let mut result: Vec<ExtrusionPath> = Vec::new();
    for p in paths {
        if let Some(prev) = result.last_mut()
            && prev.points.first() != prev.points.last()
            && p.points.first() != p.points.last()
        {
            let a = *prev.points.last().unwrap();
            let b = p.points[0];
            let len = a.distance_to(b);
            if len <= 2.0 * flow.scaled_spacing() as f64 {
                let fraction = (0.0001 * SCALE / len).min(0.49);
                let a1 = Point::new(
                    a.x + ((b.x - a.x) as f64 * fraction) as i64,
                    a.y + ((b.y - a.y) as f64 * fraction) as i64,
                );
                let b1 = Point::new(
                    b.x + ((a.x - b.x) as f64 * fraction) as i64,
                    b.y + ((a.y - b.y) as f64 * fraction) as i64,
                );
                if !segments.iter().any(|&(c, d)| intersects(a1, b1, c, d)) {
                    segments.push((a, b));
                    prev.points.extend(p.points);
                    continue;
                }
            }
        }
        result.push(p);
    }
    result
}
