// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: prusaslicer@b028299c77 src/libslic3r/Fill/FillHoneycomb.cpp :: FillHoneycomb::_fill_surface_single
// Source: prusaslicer@b028299c77 src/libslic3r/Fill/Fill3DHoneycomb.cpp :: makeNormalisedGrid, Fill3DHoneycomb::_fill_surface_single
// Changes: Shared clipping/connection helpers; compute the small hexagon constants without a cache.
// Copyright (c) Prusa Research 2016 - 2021 Vojtěch Bubník; Slic3r 2011 - 2015 Alessandro Ranellucci;
// Copyright (c) 2012 Mark Hindess; SuperSlicer 2019 Remi Durand. 3D tessellation: David Eccles (gringer).

use super::{
    FillParams,
    base::{bbox_of, rotate_points},
    clipped_connected,
};
use crate::geometry::{ExPolygon, Point, Polyline, SCALE};

pub(super) fn honeycomb(ex: &ExPolygon, p: &FillParams, angle: f64) -> Vec<Polyline> {
    let min = (p.spacing * SCALE) as i64;
    let distance = (min as f64 / p.density) as i64;
    let side = (distance as f64 / (3f64.sqrt() / 2.)) as i64;
    let width = distance * 2;
    let height = side * 3;
    let short = (distance as f64 * 3f64.sqrt() / 3.) as i64;
    let xo = min / 2;
    let yo = (xo as f64 * 3f64.sqrt() / 3.) as i64;
    let center = Point::new(width / 2, side);
    let bb = bbox_of(&ex.contour).unwrap();
    let mut corners = vec![
        bb.min,
        Point::new(bb.min.x, bb.max.y),
        bb.max,
        Point::new(bb.max.x, bb.min.y),
    ];
    for q in &mut corners {
        q.x -= center.x;
        q.y -= center.y;
    }
    rotate_points(&mut corners, angle);
    for q in &mut corners {
        q.x += center.x;
        q.y += center.y;
    }
    let bb = crate::geometry::BoundingBox::from_points(corners.iter()).unwrap();
    let mut x = bb.min.x.div_euclid(width) * width;
    let ymin = bb.min.y.div_euclid(height) * height;
    let mut lines = Vec::new();
    while x <= bb.max.x {
        let mut line = Vec::new();
        let mut ax = [x + xo, x + distance - xo];
        for _ in 0..2 {
            line.reverse();
            let mut y = ymin;
            while y <= bb.max.y {
                line.extend([
                    Point::new(ax[1], y + yo),
                    Point::new(ax[0], y + short - yo),
                    Point::new(ax[0], y + short + side + yo),
                    Point::new(ax[1], y + 2 * short + side - yo),
                    Point::new(ax[1], y + 2 * short + 2 * side + yo),
                ]);
                y += 2 * (short + side);
            }
            ax = [ax[1] + distance, ax[0] + distance];
            x += distance;
        }
        for q in &mut line {
            q.x -= center.x;
            q.y -= center.y;
        }
        rotate_points(&mut line, -angle);
        for q in &mut line {
            q.x += center.x;
            q.y += center.y;
        }
        lines.push(line);
    }
    clipped_connected(lines, ex, p)
}

pub(super) fn honeycomb_3d(ex: &ExPolygon, p: &FillParams) -> Vec<Polyline> {
    let bb = bbox_of(&ex.contour).unwrap();
    let d = (p.spacing * SCALE / p.density) as i64;
    let min = Point::new(
        bb.min.x.div_euclid(2 * d) * 2 * d,
        bb.min.y.div_euclid(2 * d) * 2 * d,
    );
    let nx = ((bb.max.x - min.x) / d + 1) as usize;
    let ny = ((bb.max.y - min.y) / d + 1) as usize;
    let period = 2f64.sqrt();
    let off = (((p.z * SCALE / d as f64) % period - period / 2.).abs() / period * 4. - 1.) * 0.5;
    let vertical = p.layer_id.is_multiple_of(2);
    let (width, height) = if vertical { (nx, ny) } else { (ny, nx) };
    let mut lines = Vec::new();
    for x in 0..=width {
        let side = |i: usize| if i.is_multiple_of(2) { -1. } else { 1. };
        let mut pairs = vec![(x as f64 - off / 2. * side(x), -off.abs() / 2.)];
        for y in 0..height {
            let a = x as f64 + off / 2. * side(x + y);
            pairs.push((a, y as f64 + off.abs() / 2.));
            pairs.push((a, y as f64 + 1. - off.abs() / 2.));
        }
        pairs.push((
            x as f64 - off / 2. * side(x + height - 1),
            height as f64 + off.abs() / 2.,
        ));
        let mut line: Polyline = pairs
            .into_iter()
            .map(|(a, b)| {
                let a = (a.clamp(0., width as f64) * d as f64) as i64;
                let b = (b.clamp(0., height as f64) * d as f64) as i64;
                let (a, b) = if vertical { (a, b) } else { (b, a) };
                Point::new(a + min.x, b + min.y)
            })
            .collect();
        if x & 1 == 1 {
            line.reverse();
        }
        line.dedup();
        lines.push(line);
    }
    clipped_connected(lines, ex, p)
}
