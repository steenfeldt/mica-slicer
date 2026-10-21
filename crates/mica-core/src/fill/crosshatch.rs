// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: orca@8500fcdcca src/libslic3r/Fill/FillCrossHatch.cpp :: generate_infill_layers, FillCrossHatch::_fill_surface_single
// Changes: Single-line fill, Clipper2 and Mica connections; no multiline replication.
// CrossHatch by Bambu Lab; transformation inspired by David Eccles (gringer).

use super::{
    FillParams,
    base::{bbox_of, rotate_points},
    clipped_connected,
};
use crate::geometry::{ExPolygon, Point, Polyline, SCALE, polyline_length};

pub(super) fn fill(ex: &ExPolygon, p: &FillParams) -> Vec<Polyline> {
    let mut ex = ex.clone();
    let angle = p.angle.to_radians();
    rotate_points(&mut ex.contour, -angle);
    for h in &mut ex.holes {
        rotate_points(h, -angle);
    }
    let mut d = (p.spacing * SCALE / p.density) as i64;
    if p.density < 0.999 {
        d = (d as f64 * 1.08) as i64;
    }
    let bb = bbox_of(&ex.contour).unwrap();
    let min = Point::new(
        bb.min.x.div_euclid(4 * d) * 4 * d,
        bb.min.y.div_euclid(4 * d) * 4 * d,
    );
    let d = d as f64;
    let ratio = if p.density < 0.3 {
        (1. - (-5. * p.density).exp()).clamp(0.2, 1.)
    } else {
        1.
    };
    let trans = d * 0.4;
    let repeat = d * ratio;
    let period = trans + repeat;
    let z = p.z * SCALE + repeat / 2. + trans;
    let tz = z.rem_euclid(period) - repeat;
    let mut vertical = z.rem_euclid(2. * period) - (period - 1.) <= 0.;
    let progress = if tz < 0. {
        None
    } else {
        let progress = tz.rem_euclid(trans) / trans;
        Some(if progress < 0.5 {
            (progress + 0.1) * 2.
        } else {
            vertical = !vertical;
            (1.1 - progress) * 2.
        })
    };
    let (w, h) = ((bb.max.x - min.x) as f64, (bb.max.y - min.y) as f64);
    let (w, h) = if vertical { (h, w) } else { (w, h) };
    let mut lines = Vec::new();
    let point = |x: f64, y: f64| {
        let (x, y) = if vertical { (y, x) } else { (x, y) };
        Point::new(x as i64 + min.x, y as i64 + min.y)
    };
    if let Some(progress) = progress {
        let grid = 2. * d;
        let off = progress / 8. * grid;
        for even in [false, true] {
            for y in 0..(h / grid) as usize + 2 {
                let mut line = Vec::new();
                for x in 0..(w / grid) as usize + 2 {
                    for (a, b) in [
                        (0.25 * grid - off, off),
                        (0.25 * grid + off, off),
                        (0.75 * grid - off, -off),
                        (0.75 * grid + off, -off),
                    ] {
                        line.push(point(
                            a + x as f64 * grid - if even { grid / 2. } else { 0. },
                            b + (y as f64 + if even { 0.5 } else { 0. }) * grid,
                        ));
                    }
                }
                lines.push(line);
            }
        }
    } else {
        for y in 0..(h / d) as usize + 1 {
            lines.push(vec![point(0., y as f64 * d), point(w, y as f64 * d)]);
        }
    }
    let mut lines = crate::geometry::clip::intersection_pl(&lines, std::slice::from_ref(&ex));
    lines.retain(|l| polyline_length(l) >= 0.8 * p.spacing * SCALE);
    let mut lines = clipped_connected(lines, &ex, p);
    for line in &mut lines {
        rotate_points(line, angle);
    }
    lines
}
