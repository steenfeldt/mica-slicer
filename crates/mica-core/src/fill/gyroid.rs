// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: prusaslicer@b028299c77 src/libslic3r/Fill/FillGyroid.cpp :: FillGyroid::_fill_surface_single
// Source: prusaslicer@b028299c77 src/libslic3r/Fill/FillGyroid.hpp :: FillGyroid
// Changes: Rust port; the wave construction (f, make_wave, make_one_period, make_gyroid_waves) is unchanged.

//! Gyroid infill: the gyroid minimal surface sliced at the layer's z, approximated by sine-like waves.

use std::f64::consts::{FRAC_PI_2, PI};

use super::base::{EPSILON, align_point_to_grid, bbox_of, chain_polylines, rotate_points};
use super::connect::connect_infill;
use crate::geometry::clip::intersection_pl;
use crate::geometry::{Coord, ExPolygon, Point, Polyline, SCALE, polyline_length};

/// Correction applied to the regular infill angle to maximise printing speed (degrees).
const CORRECTION_ANGLE: f64 = -45.0;
/// Density adjustment to get a good percentage of weight.
const DENSITY_ADJUST: f64 = 2.44;
/// Upper resolution tolerance of the pattern (mm).
const PATTERN_TOLERANCE: f64 = 0.2;

fn f(x: f64, z_sin: f64, z_cos: f64, vertical: bool, flip: bool) -> f64 {
    if vertical {
        let phase_offset = (if z_cos < 0.0 { PI } else { 0.0 }) + PI;
        let a = (x + phase_offset).sin();
        let b = -z_cos;
        let res = z_sin * (x + phase_offset + if flip { PI } else { 0.0 }).cos();
        let r = (a * a + b * b).sqrt();
        (a / r).asin() + (res / r).asin() + PI
    } else {
        let phase_offset = if z_sin < 0.0 { PI } else { 0.0 };
        let a = (x + phase_offset).cos();
        let b = -z_sin;
        let res = z_cos * (x + phase_offset + if flip { 0.0 } else { PI }).sin();
        let r = (a * a + b * b).sqrt();
        (a / r).asin() + (res / r).asin() + 0.5 * PI
    }
}

#[allow(clippy::too_many_arguments)]
fn make_wave(
    one_period: &[(f64, f64)],
    width: f64,
    height: f64,
    offset: f64,
    scale_factor: f64,
    z_cos: f64,
    z_sin: f64,
    vertical: bool,
    flip: bool,
) -> Polyline {
    let mut points = one_period.to_vec();
    let period = points.last().unwrap().0;
    if width != period {
        // Do not extend if already truncated.
        points.pop();
        let n = points.len();
        loop {
            let p = points[points.len() - n];
            points.push((p.0 + period, p.1));
            if points.last().unwrap().0 >= width - EPSILON {
                break;
            }
        }
        points.push((width, f(width, z_sin, z_cos, vertical, flip)));
    }
    points
        .into_iter()
        .map(|(x, y)| {
            let y = (y + offset).clamp(0.0, height);
            let (x, y) = if vertical { (y, x) } else { (x, y) };
            Point::new((x * scale_factor) as Coord, (y * scale_factor) as Coord)
        })
        .collect()
}

fn make_one_period(
    width: f64,
    z_cos: f64,
    z_sin: f64,
    vertical: bool,
    flip: bool,
    tolerance: f64,
) -> Vec<(f64, f64)> {
    let mut points = Vec::new();
    let dx = FRAC_PI_2; // exact coordinates on main inflexion lobes
    let limit = (2.0 * PI).min(width);
    let mut x = 0.0;
    while x < limit - EPSILON {
        points.push((x, f(x, z_sin, z_cos, vertical, flip)));
        x += dx;
    }
    points.push((limit, f(limit, z_sin, z_cos, vertical, flip)));

    // Piecewise increase in resolution up to the requested tolerance.
    loop {
        let size = points.len();
        for i in 1..size {
            let lp = points[i - 1];
            let rp = points[i];
            let x = lp.0 + (rp.0 - lp.0) / 2.0;
            let y = f(x, z_sin, z_cos, vertical, flip);
            let (a, b) = ((x - lp.0, y - lp.1), (x - rp.0, y - rp.1));
            if (a.0 * b.1 - a.1 * b.0).abs() > tolerance * tolerance {
                points.push((x, y));
            }
        }
        if size == points.len() {
            break;
        }
        points.sort_by(|l, r| l.0.total_cmp(&r.0));
    }
    points
}

fn make_gyroid_waves(
    grid_z: f64,
    density_adjusted: f64,
    line_spacing: f64,
    mut width: f64,
    mut height: f64,
) -> Vec<Polyline> {
    let scale_factor = line_spacing * SCALE / density_adjusted;
    // Tolerance in scaled units, clamped: no speed benefit beyond a certain point.
    let tolerance = (line_spacing / 2.0).min(PATTERN_TOLERANCE) / (scale_factor / SCALE);
    let z = grid_z / scale_factor;
    let (z_sin, z_cos) = z.sin_cos();

    let vertical = z_sin.abs() <= z_cos.abs();
    let mut lower_bound = 0.0;
    let mut upper_bound = height;
    let mut flip = true;
    if vertical {
        flip = false;
        lower_bound = -PI;
        upper_bound = width - FRAC_PI_2;
        std::mem::swap(&mut width, &mut height);
    }

    let one_period_odd = make_one_period(width, z_cos, z_sin, vertical, flip, tolerance);
    flip = !flip; // even polylines are a bit shifted
    let one_period_even = make_one_period(width, z_cos, z_sin, vertical, flip, tolerance);
    let mut result = Vec::new();
    let mut y0 = lower_bound;
    while y0 < upper_bound + EPSILON {
        result.push(make_wave(
            &one_period_odd,
            width,
            height,
            y0,
            scale_factor,
            z_cos,
            z_sin,
            vertical,
            flip,
        ));
        y0 += PI;
        if y0 < upper_bound + EPSILON {
            result.push(make_wave(
                &one_period_even,
                width,
                height,
                y0,
                scale_factor,
                z_cos,
                z_sin,
                vertical,
                flip,
            ));
        }
        y0 += PI;
    }
    result
}

/// Parameters of a gyroid fill of one ExPolygon.
#[derive(Clone, Copy, Debug)]
pub(crate) struct GyroidParams {
    /// mm.
    pub spacing: f64,
    pub density: f64,
    /// Base infill angle, radians (`fill_angle`, without per-layer alternation).
    pub angle: f64,
    /// Print z of the layer, mm.
    pub z: f64,
    pub anchor_length: f64,
    pub anchor_length_max: f64,
}

/// `FillGyroid::_fill_surface_single` for an ExPolygon already shrunk by half a line.
pub(crate) fn fill_gyroid(expolygon: &ExPolygon, p: &GyroidParams) -> Vec<Polyline> {
    let mut expolygon = expolygon.clone();
    let infill_angle = p.angle + CORRECTION_ANGLE.to_radians();
    if infill_angle.abs() >= EPSILON {
        rotate_points(&mut expolygon.contour, -infill_angle);
        for h in &mut expolygon.holes {
            rotate_points(h, -infill_angle);
        }
    }
    let Some(mut bb) = bbox_of(expolygon.contour.iter()) else {
        return Vec::new();
    };
    let density_adjusted = (p.density * DENSITY_ADJUST).max(0.0);
    if density_adjusted <= 0.0 {
        return Vec::new();
    }
    // Distance between the waves, scaled.
    let distance = (p.spacing * SCALE / density_adjusted) as Coord;
    if distance <= 0 {
        return Vec::new();
    }
    // Align the bounding box to a multiple of the grid module.
    let module = (2.0 * PI * distance as f64).round() as Coord;
    let aligned = align_point_to_grid(bb.min, Point::new(module, module), Point::default());
    bb.min.x = bb.min.x.min(aligned.x);
    bb.min.y = bb.min.y.min(aligned.y);

    let mut polylines = make_gyroid_waves(
        p.z * SCALE,
        density_adjusted,
        p.spacing,
        ((bb.max.x - bb.min.x) / distance) as f64 + 1.0,
        ((bb.max.y - bb.min.y) / distance) as f64 + 1.0,
    );
    // Shift the polylines to the grid origin.
    for pl in &mut polylines {
        for pt in pl.iter_mut() {
            pt.x += bb.min.x;
            pt.y += bb.min.y;
        }
    }
    let mut polylines = intersection_pl(&polylines, std::slice::from_ref(&expolygon));
    // Remove very small bits, but keep lines connecting thin walls.
    let minlength = 0.8 * p.spacing * SCALE;
    polylines.retain(|pl| polyline_length(pl) >= minlength);
    if polylines.is_empty() {
        return polylines;
    }
    let mut out = if p.anchor_length_max < 0.05 {
        chain_polylines(polylines)
    } else {
        let mut boundary = vec![expolygon.contour.clone()];
        boundary.extend(expolygon.holes.iter().cloned());
        let bbox = bbox_of(expolygon.contour.iter()).unwrap();
        connect_infill(
            polylines,
            &boundary,
            bbox,
            p.spacing,
            p.anchor_length,
            p.anchor_length_max,
        )
    };
    if infill_angle.abs() >= EPSILON {
        for pl in &mut out {
            rotate_points(pl, infill_angle);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_period_is_sorted_and_spans_two_pi() {
        let pts = make_one_period(10.0, 1.0, 0.0, true, false, 0.01);
        assert!(pts.windows(2).all(|w| w[0].0 < w[1].0));
        assert!((pts.last().unwrap().0 - 2.0 * PI).abs() < 1e-12);
    }
}
