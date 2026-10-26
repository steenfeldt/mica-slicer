// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: prusaslicer@b028299c77 src/libslic3r/Fill/FillConcentric.cpp :: FillConcentric::_fill_surface_single
// Changes: Clipper2 offsets; existing Mica Arachne and thick-path conversion; explicit loop clipping.
// Copyright (c) Prusa Research 2016 - 2023 Vojtěch Bubník, Lukáš Hejl;
// Copyright (c) Slic3r 2011 - 2015 Alessandro Ranellucci; Copyright (c) 2012 Mark Hindess.

use super::{
    FillParams, Filled,
    base::{adjust_solid_spacing, bbox_of},
};
use crate::geometry::clip::{Join, offset_ex, offset2_ex, to_polygons};
use crate::geometry::{ExPolygon, Point, Polyline, SCALE};
use crate::{
    flow::Flow,
    settings::PrintSettings,
    toolpath::{ExtrusionPath, Role},
};

pub(super) fn clip_end(line: &mut Polyline, mut distance: f64) {
    while line.len() > 1 && distance > 0. {
        let n = line.len();
        let a = line[n - 2];
        let b = line[n - 1];
        let len = a.distance_to(b);
        if len <= distance {
            distance -= len;
            line.pop();
        } else {
            line[n - 1] = super::base::lerp(a, b, (len - distance) / len);
            break;
        }
    }
}

pub(super) fn fill(ex: &ExPolygon, p: &FillParams) -> Filled {
    let min = (p.spacing * SCALE) as i64;
    let mut d = (min as f64 / p.density) as i64;
    let bb = bbox_of(&ex.contour).unwrap();
    if p.density > 0.9999 && !p.dont_adjust {
        d = adjust_solid_spacing(bb.max.x - bb.min.x, d);
    }
    let spacing = if p.density > 0.9999 && !p.dont_adjust {
        d as f64 / SCALE
    } else {
        p.spacing
    };
    let mut last = vec![ex.clone()];
    let mut lines = Vec::new();
    let mut pos = Point::default();
    while !last.is_empty() {
        for mut poly in to_polygons(&last) {
            let Some(start) = (0..poly.len()).min_by_key(|&i| poly[i].distance_sq(pos)) else {
                continue;
            };
            poly.rotate_left(start);
            poly.push(poly[0]);
            pos = poly[0];
            // Upstream loop_clipping = nozzle_diameter * 0.15, supplied in toolpath dispatch.
            lines.push(poly);
        }
        last = offset2_ex(&last, -(d + min / 2) as f64, (min / 2) as f64, Join::Miter);
    }
    Filled {
        skin_from: None,
        polylines: lines,
        spacing,
    }
}

pub(super) fn arachne(
    area: &[ExPolygon],
    p: &FillParams,
    flow: Flow,
    role: Role,
    s: &PrintSettings,
) -> Vec<ExtrusionPath> {
    let mut result = Vec::new();
    let spacing = p.spacing * SCALE;
    for ex in area {
        let bb = bbox_of(&ex.contour).unwrap();
        let count = ((bb.max.x - bb.min.x).max(bb.max.y - bb.min.y) as f64 / spacing) as usize + 1;
        // FillBase shrinks by half spacing and FillConcentric expands it back for Arachne.
        let walls = crate::arachne::WallToolPaths::generate(
            std::slice::from_ref(ex),
            spacing,
            spacing,
            count,
            flow.height,
            s.nozzle_diameter,
            &s.arachne,
        );
        let mut pos = p.origin;
        for mut line in walls.paths {
            if line.junctions.len() < 2 {
                continue;
            }
            if line.is_closed {
                if crate::geometry::signed_area(&line.polygon()) < 0. {
                    line.junctions.reverse();
                }
                line.junctions.pop();
                let start = (0..line.junctions.len())
                    .min_by_key(|&i| line.junctions[i].p.distance_sq(pos))
                    .unwrap();
                line.junctions.rotate_left(start);
                line.junctions.push(line.junctions[0]);
            }
            let mut thick = crate::geometry::medial_axis::ThickPolyline {
                points: line.polygon(),
                width: line
                    .junctions
                    .windows(2)
                    .flat_map(|j| [j[0].width, j[1].width])
                    .collect(),
                endpoints: (!line.is_closed, !line.is_closed),
            };
            pos = *thick.points.last().unwrap();
            if line.is_closed {
                let original = thick.points.clone();
                clip_end(&mut thick.points, s.nozzle_diameter * 0.15 * SCALE);
                let segments = thick.points.len().saturating_sub(1);
                thick.width.truncate(2 * segments);
                if segments > 0 {
                    let start = original[segments - 1];
                    let length = start.distance_to(original[segments]);
                    if length > 0. {
                        let t = start.distance_to(thick.points[segments]) / length;
                        let a = thick.width[2 * segments - 2];
                        let b = &mut thick.width[2 * segments - 1];
                        *b = a + (*b - a) * t;
                    }
                }
            }
            if thick.points.len() >= 2 {
                result.extend(crate::perimeters::thick_polyline_to_multi_path(
                    &thick,
                    role,
                    flow,
                    0.05 * SCALE,
                    100.,
                ));
            }
        }
    }
    result
}

pub(super) fn shrunk(area: &ExPolygon, p: &FillParams) -> Vec<ExPolygon> {
    offset_ex(
        std::slice::from_ref(area),
        -0.5 * p.spacing * SCALE,
        Join::Miter,
    )
}
