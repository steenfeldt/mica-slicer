// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: orca@8500fcdcca src/libslic3r/Fill/FillBase.cpp :: Fill::_create_gap_fill
// Source: prusaslicer@b028299c77 src/libslic3r/Fill/FillEnsuring.cpp :: make_fill_polylines
// Source: prusaslicer@b028299c77 src/libslic3r/Algorithm/PathSorting.hpp :: sort_paths
// Source: prusaslicer@b028299c77 src/libslic3r/Polyline.cpp :: ThickPolyline::clip_end
// Changes: Rust scanline/reconstruction port, Clipper2 intersections and existing Arachne;
// Changes: linear distance searches instead of AABB/KD trees; vibration graph in ensuring_filter.rs.
// Source: prusaslicer@b028299c77 src/libslic3r/Fill/Fill.cpp :: Layer::make_fills
// Changes: `entities` keeps one multi-path per variable-width line and one list per ExPolygon (a no-sort
// Changes: collection upstream).
// Attribution from src/libslic3r/Fill/FillEnsuring.cpp:
// Copyright (c) Prusa Research 2023 Vojtěch Bubník @bubnikv, Pavel Mikuš @Godrak, Lukáš Hejl @hejllukas
// PathSorting.hpp: Copyright (c) Prusa Research 2023 Pavel Mikuš @Godrak
// PrusaSlicer is released under the terms of the AGPLv3 or higher

//! Fixed-spacing internal solid sweeps with reconstructed coverage and Arachne gap filling.
use super::{FillParams, Pattern, base::rotate_points};
use crate::arachne::WallToolPaths;
use crate::flow::Flow;
use crate::geometry::clip::{
    Join, diff_ex, intersection_ex, intersection_pl, offset_ex, offset2_ex, union_ex_positive,
    union_safety_offset_ex,
};
use crate::geometry::medial_axis::ThickPolyline;
use crate::geometry::{BoundingBox, ExPolygon, Point, Polygon, SCALE};
use crate::settings::PrintSettings;
use crate::toolpath::{ExtrusionPath, Role};

pub(crate) fn fill(
    area: &[ExPolygon],
    params: &FillParams,
    flow: Flow,
    s: &PrintSettings,
) -> Vec<ExtrusionPath> {
    area.iter()
        .flat_map(|ex| {
            polylines(ex, params, flow, s)
                .iter()
                .flat_map(|thick| {
                    crate::perimeters::thick_polyline_to_multi_path(
                        thick,
                        Role::SolidInfill,
                        flow,
                        0.05 * SCALE,
                        100.,
                    )
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Per ExPolygon of `area`, the multi-paths of each variable-width line in generation order
/// (one no-sort `ExtrusionEntityCollection` each in `Layer::make_fills`).
pub(crate) fn entities(
    area: &[ExPolygon],
    params: &FillParams,
    flow: Flow,
    s: &PrintSettings,
) -> Vec<Vec<Vec<ExtrusionPath>>> {
    area.iter()
        .map(|ex| {
            polylines(ex, params, flow, s)
                .iter()
                .map(|thick| {
                    crate::perimeters::thick_polyline_to_multi_path(
                        thick,
                        Role::SolidInfill,
                        flow,
                        0.05 * SCALE,
                        100.,
                    )
                })
                .filter(|paths| !paths.is_empty())
                .collect()
        })
        .collect()
}

fn polylines(
    ex: &ExPolygon,
    params: &FillParams,
    flow: Flow,
    s: &PrintSettings,
) -> Vec<ThickPolyline> {
    let spacing = (params.spacing * SCALE) as i64;
    if spacing <= 0 {
        return Vec::new();
    }
    let direction = params.angle.to_radians() as f32
        + Pattern::Rectilinear.layer_angle(params.layer_id) as f32
        + std::f32::consts::FRAC_PI_2;
    let angle = std::f64::consts::PI - f64::from(direction);
    let mut ex = ex.clone();
    for polygon in std::iter::once(&mut ex.contour).chain(ex.holes.iter_mut()) {
        super::translate(polygon, -params.origin.x, -params.origin.y);
        rotate_points(polygon, angle);
    }
    let Some(bb) = BoundingBox::from_points(ex.contour.iter()) else {
        return Vec::new();
    };
    let area = [ex];
    let inner = offset_ex(
        &intersection_ex(
            &area,
            &offset2_ex(
                &area,
                -2. * spacing as f64,
                3. * spacing as f64,
                Join::Miter,
            ),
        ),
        -0.5 * spacing as f64,
        Join::Miter,
    );
    let count = ((bb.max.x - bb.min.x + spacing - 1) / spacing) as usize;
    let mut sections = Vec::with_capacity(count);
    for i in 0..count {
        let x = bb.min.x + i as i64 * spacing;
        let mut lines = intersection_pl(
            &[vec![Point::new(x, bb.min.y), Point::new(x, bb.max.y)]],
            &inner,
        );
        for line in &mut lines {
            line.sort_by_key(|p| p.y);
        }
        lines.retain(|l| l.last().unwrap().y - l[0].y > spacing);
        lines.sort_by_key(|l| l[0].y);
        sections.push(
            lines
                .into_iter()
                .map(|l| [l[0], *l.last().unwrap()])
                .collect::<Vec<_>>(),
        );
    }
    super::ensuring_filter::filter(&mut sections);
    let mut thick: Vec<_> = sections
        .iter()
        .flatten()
        .map(|line| ThickPolyline {
            points: line.to_vec(),
            width: vec![spacing as f64; 2],
            endpoints: (true, true),
        })
        .collect();
    let coverage = reconstruct(&sections, spacing);
    let gaps = diff_ex(
        &area,
        &union_safety_offset_ex(&union_ex_positive(&coverage)),
    );
    for gap in gaps {
        let Some(gap_bb) = BoundingBox::from_points(gap.contour.iter()) else {
            continue;
        };
        let loops = ((gap_bb.max.x - gap_bb.min.x).max(gap_bb.max.y - gap_bb.min.y) + spacing - 1)
            / spacing;
        let walls = WallToolPaths::generate(
            &[gap],
            spacing as f64,
            spacing as f64,
            loops.max(1) as usize,
            flow.height,
            s.nozzle_diameter,
            &s.arachne,
        );
        for line in walls.paths {
            if line.junctions.len() < 2 {
                continue;
            }
            let mut tp = ThickPolyline {
                points: line.polygon(),
                width: line
                    .junctions
                    .windows(2)
                    .flat_map(|j| [j[0].width, j[1].width])
                    .collect(),
                endpoints: (!line.is_closed, !line.is_closed),
            };
            if line.is_closed {
                if (crate::geometry::signed_area(&tp.points) < 0.)
                    != s.seam.prefer_clockwise_movements
                {
                    tp.reverse();
                }
                let n = tp.points.len() - 1;
                let at = (0..n)
                    .min_by_key(|&i| tp.points[i].distance_sq(gap_bb.min))
                    .unwrap();
                tp.points.pop();
                tp.points.rotate_left(at);
                tp.points.push(tp.points[0]);
                tp.width.rotate_left(2 * at);
                clip_end(&mut tp, 0.5 * spacing as f64);
            }
            if tp.points.len() > 1
                && tp.length() > 0.
                && tp.length() >= s.small_features.filter_out_gap_fill * SCALE
            {
                thick.push(tp);
            }
        }
    }
    thick.sort_by_key(|tp| {
        let bb = BoundingBox::from_points(tp.points.iter()).unwrap();
        (bb.min.x, bb.min.y)
    });
    connect_gaps(&mut thick, spacing as f64);
    thick.retain(|tp| {
        tp.points.len() > 1
            && !(tp.length() < spacing as f64 && tp.width.iter().all(|&w| w < spacing as f64))
    });
    let thick = sort_paths(thick, bb.min, spacing as f64 * 1.2);
    let mut connected: Vec<ThickPolyline> = Vec::new();
    for mut tp in thick {
        if let Some(tail) = connected.last_mut() {
            let last = tail.last_point();
            if last.distance_sq(tp.last_point()) < last.distance_sq(tp.first_point()) {
                tp.reverse();
            }
            if last.distance_to(tp.first_point()) < 2. * spacing as f64 {
                tail.points.extend(tp.points);
                tail.width.extend([spacing as f64; 2]);
                tail.width.extend(tp.width);
                continue;
            }
        }
        connected.push(tp);
    }
    for tp in &mut connected {
        rotate_points(&mut tp.points, -angle);
        super::translate(&mut tp.points, params.origin.x, params.origin.y);
    }
    connected
}

pub(super) fn clip_end(tp: &mut ThickPolyline, mut length: f64) {
    while tp.points.len() > 1 && length > 0. {
        let n = tp.points.len();
        let (a, b) = (tp.points[n - 2], tp.points[n - 1]);
        let len = a.distance_to(b);
        if len <= length {
            length -= len;
            tp.points.pop();
            tp.width.truncate(2 * (tp.points.len() - 1));
        } else {
            let t = length / len;
            tp.points[n - 1] = Point::new(
                (b.x as f64 + (a.x - b.x) as f64 * t) as i64,
                (b.y as f64 + (a.y - b.y) as f64 * t) as i64,
            );
            let w = tp.width.len();
            tp.width[w - 1] += (tp.width[w - 2] - tp.width[w - 1]) * t;
            length = 0.;
        }
    }
}

pub(super) fn reconstruct(sections: &[Vec<[Point; 2]>], spacing: i64) -> Vec<Polygon> {
    let mut active: Vec<(Polygon, Polygon)> = Vec::new();
    let mut out = Vec::new();
    let dx = spacing / 2;
    let shifted = |p: Point, x: i64| Point::new(p.x + x, p.y);
    for section in sections {
        let lines: Vec<_> = section
            .iter()
            .map(|[a, b]| [Point::new(a.x, a.y - dx), Point::new(b.x, b.y + dx)])
            .collect();
        let mut used = vec![false; lines.len()];
        let mut next = Vec::new();
        for (mut lows, mut highs) in active {
            let low = *lows.last().unwrap();
            let high = *highs.last().unwrap();
            if let Some((i, [a, b])) = lines
                .iter()
                .enumerate()
                .find(|(i, l)| !used[*i] && l[1].y > low.y && l[0].y <= high.y)
            {
                for (path, from, to) in [(&mut lows, low, *a), (&mut highs, high, *b)] {
                    if from.distance_to(to) >= 2. * spacing as f64 {
                        path.extend([shifted(from, dx), shifted(to, -dx)]);
                    }
                    path.push(to);
                }
                used[i] = true;
                next.push((lows, highs));
            } else {
                lows.push(shifted(low, dx));
                highs.push(shifted(high, dx));
                lows.extend(highs.into_iter().rev());
                out.push(lows);
            }
        }
        for (i, [a, b]) in lines.into_iter().enumerate() {
            if !used[i] {
                next.push((vec![shifted(a, -dx), a], vec![shifted(b, -dx), b]));
            }
        }
        active = next;
    }
    for (mut lows, highs) in active {
        lows.extend(highs.into_iter().rev());
        out.push(lows);
    }
    out
}

fn point_distance(p: Point, points: &[Point]) -> f64 {
    points
        .windows(2)
        .map(|e| {
            let (dx, dy) = ((e[1].x - e[0].x) as f64, (e[1].y - e[0].y) as f64);
            let (px, py) = ((p.x - e[0].x) as f64, (p.y - e[0].y) as f64);
            let t = if dx * dx + dy * dy > 0. {
                ((px * dx + py * dy) / (dx * dx + dy * dy)).clamp(0., 1.)
            } else {
                0.
            };
            (px - t * dx).hypot(py - t * dy)
        })
        .fold(f64::INFINITY, f64::min)
}

fn sort_paths(paths: Vec<ThickPolyline>, mut from: Point, touch: f64) -> Vec<ThickPolyline> {
    if paths.len() <= 1 {
        return paths;
    }
    let mut dependencies = vec![Vec::new(); paths.len()];
    for i in 0..paths.len() {
        for j in i + 1..paths.len() {
            if paths[i]
                .points
                .iter()
                .any(|&p| point_distance(p, &paths[j].points) < touch)
                || paths[j]
                    .points
                    .iter()
                    .any(|&p| point_distance(p, &paths[i].points) < touch)
            {
                dependencies[j].push(i);
            }
        }
    }
    let mut used = vec![false; paths.len()];
    let mut out = Vec::with_capacity(paths.len());
    while out.len() < paths.len() {
        let i = (0..paths.len())
            .filter(|&i| !used[i] && dependencies[i].iter().all(|&j| used[j]))
            .min_by(|&i, &j| {
                point_distance(from, &paths[i].points)
                    .total_cmp(&point_distance(from, &paths[j].points))
            })
            .unwrap();
        let mut tp = paths[i].clone();
        if from.distance_sq(tp.last_point()) < from.distance_sq(tp.first_point()) {
            tp.reverse();
        }
        from = tp.last_point();
        used[i] = true;
        out.push(tp);
    }
    out
}

fn connect_gaps(paths: &mut [ThickPolyline], spacing: f64) {
    let mut linear: Vec<_> = paths
        .iter()
        .map(|p| {
            p.points.len() == 2
                && p.points[0].x == p.points[1].x
                && p.width.iter().all(|&w| w == spacing)
        })
        .collect();
    for i in 0..paths.len() {
        for j in i + 1..paths.len() {
            if paths[i].points.len() < 2 || paths[j].points.len() < 2 || (linear[i] && linear[j]) {
                continue;
            }
            let pair = [(false, false), (false, true), (true, false), (true, true)]
                .into_iter()
                .find(|&(a, b)| {
                    let p = if a {
                        paths[i].last_point()
                    } else {
                        paths[i].first_point()
                    };
                    let q = if b {
                        paths[j].last_point()
                    } else {
                        paths[j].first_point()
                    };
                    p.distance_to(q) < spacing
                });
            let Some((a, b)) = pair else {
                continue;
            };
            let mut source = paths[i].clone();
            let mut target = paths[j].clone();
            if !b {
                target.reverse();
            }
            if a {
                source.reverse();
            }
            let n = target.points.len();
            let v = (
                target.points[n - 1].x - target.points[n - 2].x,
                target.points[n - 1].y - target.points[n - 2].y,
            );
            let w = (
                source.points[1].x - source.points[0].x,
                source.points[1].y - source.points[0].y,
            );
            let angle = ((v.0 as f64 * w.1 as f64 - v.1 as f64 * w.0 as f64)
                .atan2(v.0 as f64 * w.0 as f64 + v.1 as f64 * w.1 as f64))
            .abs();
            if angle > std::f64::consts::PI / 6. {
                continue;
            }
            target
                .width
                .extend([*target.width.last().unwrap(), source.width[0]]);
            target.width.extend(source.width);
            target.points.extend(source.points);
            paths[j] = target;
            paths[i].points.clear();
            paths[i].width.clear();
            linear[j] |= linear[i];
        }
    }
}
