// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/PrintObject.cpp :: PrintObject::bridge_over_infill
// Source: orca@8500fcdcca src/libslic3r/PrintObject.cpp :: PrintObject::bridge_over_infill
// Changes: Orca centered anchor columns and finer detail opening; shared support across speed-only regions.
// Changes: Single region; candidates gathered per layer in parallel, then layer clusters (as upstream) in
// Changes: parallel, bottom-up inside a cluster; on textured meshes all bridge columns are clipped in one sweep and candidates meet only the nearby unsupported area; sparse paths generated on demand, not retained
// Changes: for the whole print; preserve explicit periodic internal bridges. Object-dependent infills use the prepared context. Line queries use linear scans, not an AABB tree.
// Attribution from src/libslic3r/PrintObject.cpp:
// Copyright (c) Prusa Research 2016 - 2023 Lukáš Hejl @hejllukas, Pavel Mikuš @Godrak, Lukáš Matěna @lukasmatena, Vojtěch Bubník @bubnikv, Enrico Turri @enricoturri1966, Oleksandra Iushchenko @YuSanka, David Kocík @kocikdav, Roman Beránek @zavorka
// Copyright (c) 2021 Justin Schuh @jschuh
// Copyright (c) 2021 Ilya @xorza
// Copyright (c) 2016 Joseph Lenox @lordofhyphens
// Copyright (c) Slic3r 2014 - 2016 Alessandro Ranellucci @alranel
// Copyright (c) 2015 Maksim Derbasov @ntfshard
//
// PrusaSlicer is released under the terms of the AGPLv3 or higher
//
//! Internal bridges: unsupported solid filtering, deep sparse regions and actual infill line anchors.
use super::detector::{boundaries, offset, rotate, rotate_point, segments};
use super::{BridgeFlow, BridgeSurface};
use crate::fill::FillParams;
use crate::flow::Flow;
use crate::geometry::clip::{
    closing_ex, diff_ex, intersection_ex, intersection_pl, opening_ex, union_ex,
    union_safety_offset_ex,
};
use crate::geometry::{BoundingBox, ExPolygon, Point, Polyline, SCALE};
use crate::settings::PrintSettings;
use crate::shells::{FillSurfaces, ShellLayer};
use rayon::prelude::*;
use std::f64::consts::{FRAC_PI_2, PI};

fn all(f: &FillSurfaces) -> Vec<ExPolygon> {
    let mut areas = [
        f.top.as_slice(),
        &f.bottom,
        &f.bottom_bridge,
        &f.internal,
        &f.internal_solid,
    ]
    .concat();
    areas.extend(f.internal_bridge.iter().map(|b| b.polygon.clone()));
    areas
}
fn solids(f: &FillSurfaces) -> Vec<ExPolygon> {
    let mut areas = [
        f.top.as_slice(),
        &f.bottom,
        &f.bottom_bridge,
        &f.internal_solid,
    ]
    .concat();
    areas.extend(f.internal_bridge.iter().map(|b| b.polygon.clone()));
    areas
}
fn area(ex: &[ExPolygon]) -> f64 {
    ex.iter().map(|e| e.area_mm2() * SCALE * SCALE).sum()
}

pub(crate) fn bridge_over_infill(
    surfaces: &mut [FillSurfaces],
    layers: &[ShellLayer],
    s: &PrintSettings,
    origin: Point,
) {
    if s.fill_pattern.needs_object() {
        return;
    }
    bridge_over_infill_prepared(surfaces, layers, s, origin, None);
}

pub(crate) fn bridge_over_infill_prepared(
    surfaces: &mut [FillSurfaces],
    layers: &[ShellLayer],
    s: &PrintSettings,
    origin: Point,
    context: Option<&crate::fill::PreparedFill>,
) {
    bridge_over_infill_supported(surfaces, layers, s, origin, context, None, &|| false);
}

/// `cancelled` is polled per layer; once it says so the surfaces are left as they were (the
/// caller drops the cancelled result).
pub(crate) fn bridge_over_infill_supported(
    surfaces: &mut [FillSurfaces],
    layers: &[ShellLayer],
    s: &PrintSettings,
    origin: Point,
    context: Option<&crate::fill::PreparedFill>,
    support: Option<&[FillSurfaces]>,
    cancelled: &(dyn Fn() -> bool + Sync),
) {
    if s.fill_density >= 1.0 {
        return;
    }
    // Gather all candidates before updating any surfaces, like upstream. Each layer reads only
    // itself and the immutable layer below, so the layers are gathered in parallel.
    let n = surfaces.len();
    let view: &[FillSurfaces] = surfaces;
    let source = support.unwrap_or(view);
    let candidates: Vec<Vec<Vec<ExPolygon>>> = (0..n)
        .into_par_iter()
        .map(|i| {
            let mut candidates = Vec::new();
            if i == 0 || cancelled() {
                return candidates;
            }
            let spacing = layers[i].solid.scaled_spacing() as f64;
            let below = &source[i - 1];
            let unsupported = diff_ex(
                &offset(&closing_ex(&all(below), 100.), -3. * spacing),
                &offset(&offset(&solids(below), -spacing), 4. * spacing),
            );
            // Textured meshes: thousands of solid pieces, each against the layer's unsupported
            // area cut to its neighbourhood.
            let cropper = s
                .textured_mesh
                .then(|| crate::geometry::clip::Cropper::new(&unsupported));
            for ex in &view[i].internal_solid {
                let one = std::slice::from_ref(ex);
                let unsupported = match (&cropper, BoundingBox::from_points(ex.contour.iter())) {
                    (Some(c), Some(bb)) => {
                        let near = c.crop(BoundingBox {
                            min: Point::new(bb.min.x - 1000, bb.min.y - 1000),
                            max: Point::new(bb.max.x + 1000, bb.max.y + 1000),
                        });
                        intersection_ex(one, &near)
                    }
                    _ => intersection_ex(one, &unsupported),
                };
                if unsupported.is_empty()
                    || (area(&unsupported) < area(one) - 1e-4
                        && area(&unsupported) <= 9. * spacing * spacing)
                {
                    continue;
                }
                let mut worth = intersection_ex(one, &offset(&unsupported, 4. * spacing));
                for piece in diff_ex(one, &offset(&worth, spacing)) {
                    let a = area(std::slice::from_ref(&piece));
                    if a < spacing * 12. * SCALE && a > spacing * spacing {
                        worth.push(piece);
                    }
                }
                let worth = intersection_ex(&closing_ex(&worth, 100.), one);
                if !worth.is_empty() {
                    candidates.push(worth);
                }
            }
            candidates
        })
        .collect();
    // A layer's bridges are clipped by those of the candidate layers within its bridge height
    // below it, so such layers form one cluster, processed bottom-up. Clusters are independent
    // and run in parallel (upstream clusters its threads the same way).
    let mut clusters: Vec<Vec<(usize, Vec<Vec<ExPolygon>>)>> = Vec::new();
    let mut last: Option<usize> = None;
    for (i, layer_candidates) in candidates.into_iter().enumerate() {
        if i == 0 || layer_candidates.is_empty() {
            continue;
        }
        let target_height = BridgeFlow::for_support(s, layers[i].solid).extrusion.height * 0.9;
        let depends =
            // Exactly the negation of the `break` condition in `bridge_layer` (NaN included).
            last.is_some_and(|p| {
                layers[p]
                    .print_z
                    .partial_cmp(&(layers[i].print_z - target_height - 1e-4))
                    != Some(std::cmp::Ordering::Less)
            });
        match clusters.last_mut() {
            Some(cluster) if depends => cluster.push((i, layer_candidates)),
            _ => clusters.push(vec![(i, layer_candidates)]),
        }
        last = Some(i);
    }
    let computed: Vec<Vec<(usize, Vec<BridgeSurface>)>> = clusters
        .into_par_iter()
        .map(|cluster| {
            let mut done: Vec<(usize, Vec<BridgeSurface>)> = Vec::new();
            for (i, mut layer_candidates) in cluster {
                if cancelled() {
                    break;
                }
                let own = bridge_layer(
                    i,
                    &mut layer_candidates,
                    view,
                    source,
                    layers,
                    s,
                    origin,
                    context,
                    |j| {
                        done.iter()
                            .find(|(k, _)| *k == j)
                            .map_or(&[][..], |(_, b)| b.as_slice())
                    },
                );
                done.push((i, own));
            }
            done
        })
        .collect();
    if cancelled() {
        return;
    }
    let mut bridges: Vec<Vec<BridgeSurface>> = vec![Vec::new(); n];
    for (i, b) in computed.into_iter().flatten() {
        bridges[i] = b;
    }
    // Each layer is trimmed by its own and the next layer's bridges only.
    surfaces.par_iter_mut().enumerate().for_each(|(i, f)| {
        if bridges[i].is_empty() && bridges.get(i + 1).is_none_or(|b| b.is_empty()) {
            return;
        }
        let spacing = layers[i].solid.scaled_spacing() as f64;
        let fill = union_safety_offset_ex(&all(f));
        let near = diff_ex(&fill, &offset(&fill, -spacing));
        let mut additional = Vec::new();
        if let Some(above) = bridges.get(i + 1) {
            for b in above {
                let one = std::slice::from_ref(&b.polygon);
                additional.extend(diff_ex(
                    one,
                    &offset(one, -layers[i + 1].solid.scaled_spacing() as f64),
                ));
            }
        }
        let additional = intersection_ex(&additional, &near);
        let cut: Vec<_> = bridges[i].iter().map(|b| b.polygon.clone()).collect();
        f.internal = diff_ex(&diff_ex(&f.internal, &cut), &additional);
        f.internal_solid.extend(additional);
        f.internal_solid = union_safety_offset_ex(&diff_ex(&f.internal_solid, &cut));
    });
    for (f, b) in surfaces.iter_mut().zip(bridges) {
        f.internal_bridge.extend(b);
    }
    super::extra::internal(surfaces, layers, s);
}

/// The thick bridges of candidate layer `i` (`candidates` are its unsupported solid areas, sorted
/// here as upstream does). `below(j)` returns the finished bridges of an earlier layer `j`.
#[allow(clippy::too_many_arguments)]
fn bridge_layer<'a>(
    i: usize,
    candidates: &mut [Vec<ExPolygon>],
    view: &[FillSurfaces],
    source: &[FillSurfaces],
    layers: &[ShellLayer],
    s: &PrintSettings,
    origin: Point,
    context: Option<&crate::fill::PreparedFill>,
    below: impl Fn(usize) -> &'a [BridgeSurface],
) -> Vec<BridgeSurface> {
    let mut own: Vec<BridgeSurface> = Vec::new();
    let flow = BridgeFlow::for_support(s, layers[i].solid);
    let spacing = flow.spacing * SCALE;
    let target_height = flow.extrusion.height * 0.9;
    let mut sparse = Vec::new();
    let mut not_sparse = Vec::new();
    for j in (0..i).rev() {
        if j < i - 1 && layers[j].print_z < layers[i].print_z - target_height * 0.9 - 1e-4 {
            break;
        }
        sparse.extend_from_slice(&source[j].internal);
        not_sparse.extend(solids(&source[j]));
    }
    let mut deep = diff_ex(
        &closing_ex(&union_ex(&sparse), 100.),
        &closing_ex(&union_ex(&not_sparse), 100.),
    );
    for j in (0..i).rev() {
        if layers[j].print_z < layers[i].print_z - target_height - 1e-4 {
            break;
        }
        let prior: Vec<_> = below(j).iter().map(|b| b.polygon.clone()).collect();
        deep = diff_ex(&deep, &prior);
    }
    deep = offset(&deep, spacing * 1.5);
    let current = &view[i];
    let total = closing_ex(&all(current), 100.);
    let mut expansion = intersection_ex(
        &closing_ex(
            &[current.internal.as_slice(), &current.internal_solid].concat(),
            100.,
        ),
        &deep,
    );
    let internal_unsupported = offset(&deep, -4.5 * spacing);
    let sparse_spacing = Flow::new(s.infill_extrusion_width, s.layer_height).spacing();
    let params = FillParams {
        spacing: sparse_spacing,
        density: s.fill_density,
        angle: s.fill_angle,
        layer_id: i - 1,
        z: layers[i - 1].print_z,
        anchor_length: s.infill_anchor.resolve(sparse_spacing, 1.0),
        anchor_length_max: s.infill_anchor_max.resolve(sparse_spacing, 1.0),
        dont_adjust: false,
        link_max_length: 0.0,
        origin,
    };
    let infill: Vec<_> = if let Some(context) = context {
        crate::fill::toolpaths(
            s.fill_pattern,
            &source[i - 1].internal,
            &params,
            Flow::new(s.infill_extrusion_width, layers[i - 1].height),
            crate::toolpath::Role::InternalInfill,
            s,
            Some(context),
            origin,
        )
        .into_iter()
        .map(|p| p.points)
        .collect()
    } else {
        crate::fill::fill(s.fill_pattern, &source[i - 1].internal, &params)
            .into_iter()
            .flat_map(|f| f.polylines)
            .collect()
    };
    let anchors = intersection_pl(&infill, &offset(&expansion, -spacing));
    candidates.sort_by_key(|a| {
        BoundingBox::from_points(a.iter().flat_map(|e| e.contour.iter()))
            .map(|b| (b.min.x, b.min.y))
    });
    if candidates.len() > 2 {
        let origin = BoundingBox::from_points(candidates[0].iter().flat_map(|e| e.contour.iter()))
            .unwrap()
            .max;
        candidates[1..].sort_by_key(|a| {
            BoundingBox::from_points(a.iter().flat_map(|e| e.contour.iter()))
                .unwrap()
                .min
                .distance_sq(origin)
        });
    }
    for candidate in candidates.iter() {
        let mut area = intersection_ex(&offset(candidate, spacing), &deep);
        area.retain(|ex| {
            !intersection_ex(std::slice::from_ref(ex), &internal_unsupported).is_empty()
        });
        if area.is_empty() {
            continue;
        }
        let limit = union_ex(&[area.as_slice(), &expansion].concat());
        let mut boundary = boundaries(&offset(&total, 1.3 * spacing));
        // Upstream uses unscaled flow.spacing() here (a sub-grid offset), intentionally retained.
        boundary.extend(boundaries(&offset(&limit, 0.3 * flow.spacing)));
        let mut angle = determine_angle(
            &area,
            &segments(if anchors.is_empty() {
                &boundary
            } else {
                &anchors
            }),
            s.bridge_style,
        );
        if s.internal_bridge_angle > 0. {
            angle = s.internal_bridge_angle.to_radians();
        }
        boundary.extend(anchors.iter().cloned());
        let boundary = segments(&boundary);
        let mut bridged = construct_anchored(
            &area,
            &boundary,
            flow,
            angle,
            s.bridge_style,
            s.textured_mesh,
        );
        let inflated = offset(&bridged, 3. * spacing);
        if let Some(other) = own
            .iter()
            .find(|b| !intersection_ex(std::slice::from_ref(&b.polygon), &inflated).is_empty())
        {
            angle = other.angle;
            bridged = construct_anchored(
                &area,
                &boundary,
                flow,
                angle,
                s.bridge_style,
                s.textured_mesh,
            );
        }
        let opening = if s.bridge_style == super::BridgeStyle::Orca {
            0.75
        } else {
            1.
        };
        bridged = closing_ex(&opening_ex(&bridged, spacing * opening), spacing);
        bridged = intersection_ex(&intersection_ex(&bridged, &limit), &total);
        bridged = diff_ex(&bridged, &current.top);
        // Adjacent candidates can share anchor polygons: keep their output disjoint.
        let used: Vec<_> = own.iter().map(|b| b.polygon.clone()).collect();
        bridged = diff_ex(&bridged, &used);
        expansion = diff_ex(&expansion, &bridged);
        own.extend(
            bridged
                .into_iter()
                .map(|polygon| BridgeSurface { polygon, angle }),
        );
    }
    own
}

fn distance_sq(p: Point, l: [Point; 2]) -> f64 {
    let dx = (l[1].x - l[0].x) as f64;
    let dy = (l[1].y - l[0].y) as f64;
    let x = (p.x - l[0].x) as f64;
    let y = (p.y - l[0].y) as f64;
    let t = ((x * dx + y * dy) / (dx * dx + dy * dy)).clamp(0., 1.);
    (x - t * dx).powi(2) + (y - t * dy).powi(2)
}
fn determine_angle(area: &[ExPolygon], anchors: &[[Point; 2]], style: super::BridgeStyle) -> f64 {
    if anchors.is_empty() {
        return 0.001;
    }
    let mut directions = Vec::<(f64, usize)>::new();
    for p in area.iter().flat_map(|e| e.polygons()) {
        let mut acc = 0.;
        // Match upstream's open vertex iteration (no closing segment).
        for w in p.windows(2) {
            let distance = w[0].distance_to(w[1]);
            acc += distance;
            if acc <= 2. * SCALE {
                continue;
            }
            acc = 0.;
            let count = (distance / (2. * SCALE)).ceil() as usize;
            for i in 0..count {
                let t = i as f64 / count as f64;
                let q = Point::new(
                    (w[0].x as f64 + t * (w[1].x - w[0].x) as f64) as i64,
                    (w[0].y as f64 + t * (w[1].y - w[0].y) as f64) as i64,
                );
                let line = anchors
                    .iter()
                    .min_by(|a, b| distance_sq(q, **a).total_cmp(&distance_sq(q, **b)))
                    .unwrap();
                let angle = ((line[1].y - line[0].y) as f64)
                    .atan2((line[1].x - line[0].x) as f64)
                    .rem_euclid(PI)
                    + FRAC_PI_2;
                if let Some(d) = directions.iter_mut().find(|d| d.0 == angle) {
                    d.1 += 1;
                } else {
                    directions.push((angle, 1));
                }
            }
        }
    }
    directions.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut best = (0., 0);
    for &(angle, _) in &directions {
        let (mut sum, mut count) = (0., 0);
        for &(a, n) in &directions {
            if (a - angle).abs() < PI * 0.1
                || (angle - PI * 0.1 < FRAC_PI_2
                    && a >= 1.5 * PI - (FRAC_PI_2 - (angle - PI * 0.1)))
            {
                let a = if style == super::BridgeStyle::Orca && (a - angle).abs() >= PI * 0.1 {
                    a - PI
                } else {
                    a
                };
                sum += a * n as f64;
                count += n;
            }
        }
        if count > best.1 {
            best = (sum / count as f64, count);
        }
    }
    if best.0 == 0. { 0.001 } else { best.0 }
}

fn construct_anchored(
    area: &[ExPolygon],
    anchors: &[[Point; 2]],
    flow: BridgeFlow,
    angle: f64,
    style: super::BridgeStyle,
    batch: bool,
) -> Vec<ExPolygon> {
    let turn = FRAC_PI_2 - angle;
    let area = rotate(area, turn);
    let anchors: Vec<_> = anchors
        .iter()
        .map(|l| [rotate_point(l[0], turn), rotate_point(l[1], turn)])
        .collect();
    let Some(bb) = BoundingBox::from_points(area.iter().flat_map(|e| e.contour.iter())) else {
        return Vec::new();
    };
    let Some(ab) = BoundingBox::from_points(anchors.iter().flatten()) else {
        return Vec::new();
    };
    let spacing = (flow.spacing * SCALE).round() as i64;
    let width = (flow.extrusion.width * SCALE).round() as i64;
    let mut columns = Vec::<Vec<[Point; 2]>>::new();
    let x0 = bb.min.x
        + if style == super::BridgeStyle::Orca {
            spacing / 2
        } else {
            0
        };
    let line = |x: i64| {
        vec![
            Point::new(x, ab.min.y - spacing),
            Point::new(x, ab.max.y + spacing),
        ]
    };
    let section = |x: i64, l: &Polyline| {
        let lo = l.iter().map(|p| p.y).min().unwrap();
        let hi = l.iter().map(|p| p.y).max().unwrap();
        [Point::new(x, lo), Point::new(x, hi)]
    };
    // On a textured mesh the area has thousands of points: every column is clipped in one
    // sweep, each piece going back to its column (they are a line spacing apart).
    let mut batched: Option<Vec<Vec<[Point; 2]>>> = None;
    if batch && spacing > 0 && bb.max.x > x0 {
        let count = ((bb.max.x - x0 - 1) / spacing + 1) as usize;
        let lines: Vec<Polyline> = (0..count).map(|k| line(x0 + k as i64 * spacing)).collect();
        let mut by_column = vec![Vec::new(); count];
        for l in intersection_pl(&lines, &area) {
            let Some(first) = l.first() else { continue };
            let k = ((first.x - x0) as f64 / spacing as f64).round();
            if k >= 0. && (k as usize) < count {
                let k = k as usize;
                by_column[k].push(section(x0 + k as i64 * spacing, &l));
            }
        }
        batched = Some(by_column);
    }
    let mut batched = batched.map(|b| b.into_iter());
    let mut x = x0;
    while x < bb.max.x {
        let mut sections: Vec<_> = match &mut batched {
            Some(columns) => columns.next().unwrap_or_default(),
            None => intersection_pl(&[line(x)], &area)
                .iter()
                .map(|l| section(x, l))
                .collect(),
        };
        let mut hits = Vec::new();
        for &[a, b] in &anchors {
            if a.x != b.x && x >= a.x.min(b.x) && x <= a.x.max(b.x) {
                let t = (x - a.x) as f64 / (b.x - a.x) as f64;
                hits.push((a.y as f64 + t * (b.y - a.y) as f64).round() as i64);
            }
        }
        hits.sort_unstable();
        hits.dedup();
        for section in &mut sections {
            if let Some(&y) = hits.iter().rev().find(|&&y| y < section[0].y) {
                section[0].y = y - width;
            }
            if let Some(&y) = hits.iter().find(|&&y| y > section[1].y) {
                section[1].y = y + width;
            }
        }
        sections.sort_by_key(|l| l[0].y);
        let mut merged = Vec::<[Point; 2]>::new();
        for l in sections {
            if let Some(last) = merged.last_mut()
                && last[1].y >= l[0].y
            {
                last[1].y = last[1].y.max(l[1].y);
            } else {
                merged.push(l);
            }
        }
        columns.push(merged);
        x += spacing;
    }
    // Trace consecutive sections, adding a half-spacing step at large changes in anchor distance.
    let mut active = Vec::<(Vec<Point>, Vec<Point>)>::new();
    let mut out = Vec::new();
    let close = |mut low: Vec<Point>, high: Vec<Point>, out: &mut Vec<ExPolygon>| {
        low.extend(high.into_iter().rev());
        if low.len() >= 3 {
            out.push(ExPolygon {
                contour: low,
                holes: Vec::new(),
            });
        }
    };
    for column in columns {
        let mut used = vec![false; column.len()];
        let mut next = Vec::new();
        for (mut low, mut high) in active {
            let a = *low.last().unwrap();
            let b = *high.last().unwrap();
            let found = column
                .iter()
                .enumerate()
                .find(|(j, l)| !used[*j] && l[0].y <= b.y && l[1].y >= a.y);
            if let Some((j, l)) = found {
                for (points, p) in [(&mut low, l[0]), (&mut high, l[1])] {
                    let last = *points.last().unwrap();
                    if last.distance_sq(p) as f64 >= 36. * (spacing as f64).powi(2) {
                        points.push(Point::new(last.x + spacing / 2, last.y));
                        points.push(Point::new(p.x - spacing / 2, p.y));
                    }
                    points.push(p);
                }
                used[j] = true;
                next.push((low, high));
            } else {
                low.push(Point::new(a.x + spacing / 2, a.y));
                high.push(Point::new(b.x + spacing / 2, b.y));
                close(low, high, &mut out);
            }
        }
        for (j, l) in column.iter().enumerate() {
            if !used[j] {
                next.push((
                    vec![Point::new(l[0].x - spacing / 2, l[0].y), l[0]],
                    vec![Point::new(l[1].x - spacing / 2, l[1].y), l[1]],
                ));
            }
        }
        active = next;
    }
    for (low, high) in active {
        close(low, high, &mut out);
    }
    // Traces run clockwise; normalize before the nonzero union.
    for ex in &mut out {
        if crate::geometry::signed_area(&ex.contour) < 0. {
            ex.contour.reverse();
        }
    }
    rotate(&union_safety_offset_ex(&out), -turn)
}
