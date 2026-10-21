// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/Support/SupportCommon.cpp :: tree_supports_generate_paths
// Changes: Hollow organic loops with inward seam anchors and area-triggered double walls;
// Changes: Retain Prusa fixed-direction anchor / no-sort inner-outer collection ranges through scheduling.
// Changes: polygon clipping checks anchor visibility instead of per-vertex source-contour tags.
// Source: orca@8500fcdcca src/libslic3r/Support/SupportCommon.cpp :: tree_supports_generate_paths, generate_support_toolpaths
// Changes: Orca draws inner walls on every opened island and doubles walls above 100 mm.
// Copyright (c) Prusa Research 2023 Vojtěch Bubník @bubnikv
// PrusaSlicer is released under the terms of the AGPLv3 or higher.

use crate::flow::Flow;
use crate::geometry::clip::{Join, intersection_pl, offset_ex, offset2_ex};
use crate::geometry::{ExPolygon, Point, SCALE, polyline_length};
use crate::toolpath::{ExtrusionPath, Role};

pub(super) fn generate(
    polygons: &[ExPolygon],
    flow: Flow,
    double_wall_diameter: f64,
    orca: bool,
    groups: &mut Vec<std::ops::Range<usize>>,
    offset: usize,
) -> Vec<ExtrusionPath> {
    let spacing = flow.spacing();
    let width = flow.width * SCALE;
    let eps = 1e-4 * SCALE;
    let path = |points: Vec<Point>| ExtrusionPath {
        role: Role::SupportMaterial,
        points,
        flow,
        overhang: None,
        perimeter_index: None,
        seam: None,
        brick: None,
    };
    let mut out = Vec::new();
    // closing_ex(polygons, SCALED_EPSILON, SCALED_EPSILON + 0.5 * width): loop centrelines.
    for ex in offset2_ex(polygons, eps, -(eps + 0.5 * width), Join::Miter) {
        let start = offset + out.len();
        let double = double_wall_diameter > 0.0
            && ex.area_mm2() > std::f64::consts::PI * double_wall_diameter.powi(2) / 4.0;
        let mut regions = vec![ex.clone()];
        let mut outer = Vec::new();
        if double {
            // Another perimeter on the centreline; the anchored loop moves one width inward.
            // Prusa requires a single remaining island, Orca draws inner walls on all of them.
            let level2 = offset2_ex(
                std::slice::from_ref(&ex),
                -1.5 * width,
                0.5 * width,
                Join::Miter,
            );
            if (orca && !level2.is_empty()) || level2.len() == 1 {
                for poly in ex.polygons() {
                    if poly.len() < 3 {
                        continue;
                    }
                    let mut points = poly.clone();
                    points.push(points[0]);
                    clip_end(&mut points, spacing * 0.15 * SCALE);
                    if points.len() >= 2 {
                        outer.push(path(points));
                    }
                }
                regions = if orca {
                    level2
                } else {
                    vec![level2[0].clone()]
                };
            }
        }
        let visible = offset_ex(std::slice::from_ref(&ex), 0.5 * width, Join::Round);
        for region in &regions {
            let anchors = offset_ex(std::slice::from_ref(region), -width, Join::Miter);
            for poly in region.polygons() {
                if poly.len() < 3 {
                    continue;
                }
                let mut points = poly.clone();
                points.reverse();
                points.push(points[0]);
                clip_end(&mut points, spacing * 0.15 * SCALE);
                if points.len() < 2 {
                    continue;
                }
                append_anchor(
                    &mut points,
                    &anchors,
                    &visible,
                    spacing * 6.0 * SCALE,
                    width * 3.0,
                );
                points.reverse();
                out.push(path(points));
                if !orca && !double {
                    let end = offset + out.len();
                    groups.push(end - 1..end);
                }
            }
        }
        // Upstream reverses the double-wall collection: anchored inner loop first.
        out.extend(outer);
        if !orca && double && offset + out.len() > start {
            groups.push(start..offset + out.len());
        }
    }
    out
}
fn interpolate(a: Point, b: Point, t: f64) -> Point {
    Point::new(
        a.x + ((b.x - a.x) as f64 * t).round() as i64,
        a.y + ((b.y - a.y) as f64 * t).round() as i64,
    )
}
fn clip_end(points: &mut Vec<Point>, mut distance: f64) {
    while points.len() > 1 {
        let n = points.len();
        let length = points[n - 2].distance_to(points[n - 1]);
        if length > distance {
            points[n - 1] = interpolate(points[n - 1], points[n - 2], distance / length);
            break;
        }
        distance -= length;
        points.pop();
    }
}
fn append_anchor(
    points: &mut Vec<Point>,
    anchors: &[ExPolygon],
    region: &[ExPolygon],
    length: f64,
    max_distance: f64,
) {
    let seam = *points.last().unwrap();
    let mut best = None;
    let mut best_distance = max_distance;
    for poly in anchors.iter().flat_map(ExPolygon::polygons) {
        for i in 0..poly.len() {
            let a = poly[i];
            let b = poly[(i + 1) % poly.len()];
            let dx = (b.x - a.x) as f64;
            let dy = (b.y - a.y) as f64;
            let t = (((seam.x - a.x) as f64 * dx + (seam.y - a.y) as f64 * dy)
                / (dx * dx + dy * dy).max(1.0))
            .clamp(0.0, 1.0);
            let foot = interpolate(a, b, t);
            let distance = seam.distance_to(foot);
            if distance < best_distance {
                let clipped = intersection_pl(&[vec![seam, foot]], region);
                if clipped.iter().map(|p| polyline_length(p)).sum::<f64>() + 10.0 >= distance {
                    best = Some((poly, i, foot));
                    best_distance = distance;
                }
            }
        }
    }
    if let Some((poly, start, foot)) = best {
        points.push(foot);
        let mut remaining = length - best_distance;
        let mut from = foot;
        for step in 1..=poly.len() {
            let to = poly[(start + step) % poly.len()];
            let d = from.distance_to(to);
            if d > remaining {
                points.push(interpolate(from, to, remaining / d));
                break;
            }
            points.push(to);
            remaining -= d;
            from = to;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_anchor_and_double_wall_ranges_are_prusa_only() {
        let polygon = |x| ExPolygon {
            contour: [(x, 0.), (x + 10., 0.), (x + 10., 10.), (x, 10.)]
                .map(|(x, y)| Point::from_mm(x, y))
                .to_vec(),
            holes: Vec::new(),
        };
        let islands = [polygon(0.), polygon(30.)];
        for diameter in [0., 5.] {
            let mut prusa = Vec::new();
            let mut orca = Vec::new();
            let paths = generate(
                &islands,
                Flow::new(0.42, 0.2),
                diameter,
                false,
                &mut prusa,
                7,
            );
            let orca_paths = generate(&islands, Flow::new(0.42, 0.2), diameter, true, &mut orca, 7);
            assert_eq!(paths, orca_paths);
            assert!(orca.is_empty());
            let lengths: Vec<_> = prusa.iter().map(|g| g.len()).collect();
            assert_eq!(
                lengths,
                if diameter == 0. {
                    vec![1, 1]
                } else {
                    vec![2, 2]
                }
            );
            assert_eq!(prusa[0].start, 7);
            assert_eq!(prusa.last().unwrap().end, 7 + paths.len());
        }
    }
}
