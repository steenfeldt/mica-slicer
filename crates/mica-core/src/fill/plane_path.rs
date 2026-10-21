// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: prusaslicer@b028299c77 src/libslic3r/Fill/FillPlanePath.cpp :: FillPlanePath::_fill_surface_single, generate_archimedean_chords, hilbert_n_to_xy, generate_octagram_spiral
// Changes: Use Clipper2 and Mica connection helpers; discard outside points while generating the curve.
// Copyright (c) Prusa Research 2016 - 2022 Vojtěch Bubník; Slic3r 2011 - 2016 Alessandro Ranellucci.
// Hilbert state table adapted upstream from Kevin Ryde's Math::PlanePath::HilbertCurve.

use super::{
    FillParams, Pattern,
    base::{bbox_of, rotate_points},
    clipped_connected,
};
use crate::geometry::{BoundingBox, ExPolygon, Point, Polyline, SCALE};

fn hilbert(n: usize) -> (i64, i64) {
    const NEXT: [usize; 16] = [4, 0, 0, 12, 0, 4, 4, 8, 12, 8, 8, 4, 8, 12, 12, 0];
    const X: [i64; 16] = [0, 1, 1, 0, 0, 0, 1, 1, 1, 0, 0, 1, 1, 1, 0, 0];
    const Y: [i64; 16] = [0, 0, 1, 1, 0, 1, 1, 0, 1, 1, 0, 0, 1, 0, 0, 1];
    let digits = (usize::BITS - n.leading_zeros()).div_ceil(2) as usize;
    let mut state = if digits & 1 == 1 { 4 } else { 0 };
    let (mut x, mut y) = (0, 0);
    for i in (0..digits).rev() {
        state += (n >> (i * 2)) & 3;
        x |= X[state] << i;
        y |= Y[state] << i;
        state = NEXT[state];
    }
    (x, y)
}

pub(super) fn fill(
    ex: &ExPolygon,
    p: &FillParams,
    pattern: Pattern,
    object_bounds: Option<BoundingBox>,
) -> Vec<Polyline> {
    let angle = p.angle.to_radians() + std::f64::consts::FRAC_PI_2;
    let mut ex = ex.clone();
    rotate_points(&mut ex.contour, -angle);
    for h in &mut ex.holes {
        rotate_points(h, -angle);
    }
    let snug = bbox_of(&ex.contour).unwrap();
    let mut bb = snug;
    if p.density < 0.995
        && let Some(b) = object_bounds
    {
        let mut corners = vec![
            b.min,
            Point::new(b.min.x, b.max.y),
            b.max,
            Point::new(b.max.x, b.min.y),
        ];
        rotate_points(&mut corners, -angle);
        bb = BoundingBox::from_points(corners.iter()).unwrap();
    } else {
        bb.min.x -= 100;
        bb.min.y -= 100;
        bb.max.x += 100;
        bb.max.y += 100;
    }
    let shift = if pattern == Pattern::HilbertCurve {
        bb.min
    } else {
        Point::new((bb.min.x + bb.max.x) / 2, (bb.min.y + bb.max.y) / 2)
    };
    let d = p.spacing * SCALE / p.density;
    let maxx = ((bb.max.x - shift.x) as f64 / d).ceil();
    let maxy = ((bb.max.y - shift.y) as f64 / d).ceil();
    let minx = ((bb.min.x - shift.x) as f64 / d).ceil();
    let miny = ((bb.min.y - shift.y) as f64 / d).ceil();
    let mut curve: Polyline = Vec::new();
    // Same three-point rejection as upstream's InfillPolylineClipper: no huge outside Hilbert buffers.
    let sides = |v: Point| {
        u8::from(v.x < snug.min.x)
            | (u8::from(v.x > snug.max.x) << 1)
            | (u8::from(v.y < snug.min.y) << 2)
            | (u8::from(v.y > snug.max.y) << 3)
    };
    let mut add = |x: f64, y: f64| {
        let v = Point::new(
            (x * d + 0.5).floor() as i64 + shift.x,
            (y * d + 0.5).floor() as i64 + shift.y,
        );
        let n = curve.len();
        if n >= 2 && sides(curve[n - 2]) & sides(curve[n - 1]) & sides(v) != 0 {
            curve.pop();
        }
        curve.push(v);
    };
    match pattern {
        Pattern::HilbertCurve => {
            let size = ((maxx - minx + 1.).max(maxy - miny + 1.) as usize)
                .max(2)
                .next_power_of_two();
            for i in 0..size * size {
                let (x, y) = hilbert(i);
                add(x as f64 + minx, y as f64 + miny);
            }
        }
        Pattern::ArchimedeanChords => {
            let maxr = maxx.hypot(maxy) * 2f64.sqrt() + 1.5;
            let resolution = 0.0125 * SCALE / d;
            let (mut r, mut theta) = (1., 0.);
            add(0., 0.);
            add(1., 0.);
            while r < maxr {
                theta += 2. * (1. - resolution / r).clamp(-1., 1.).acos();
                r = 1. + theta / std::f64::consts::TAU;
                add(r * theta.cos(), r * theta.sin());
            }
        }
        Pattern::OctagramSpiral => {
            let maxr = maxx.hypot(maxy) * 2f64.sqrt() + 1.5;
            let inc = 2f64.sqrt();
            let mut r = 0.;
            add(0., 0.);
            while r < maxr {
                r += inc;
                let x = r / inc;
                let r2 = r + x;
                for (a, b) in [
                    (r, 0.),
                    (r2, x),
                    (x, x),
                    (x, r2),
                    (0., r),
                    (-x, r2),
                    (-x, x),
                    (-r2, x),
                    (-r, 0.),
                    (-r2, -x),
                    (-x, -x),
                    (-x, -r2),
                    (0., -r),
                    (x, -r2),
                    (x, -x),
                    (r2 + inc, -x),
                ] {
                    add(a, b);
                }
            }
        }
        _ => unreachable!(),
    }
    let mut cp = *p;
    if p.density > 0.5 {
        cp.anchor_length_max = 0.;
    }
    let mut lines = clipped_connected(vec![curve], &ex, &cp);
    for l in &mut lines {
        rotate_points(l, angle);
    }
    lines
}
