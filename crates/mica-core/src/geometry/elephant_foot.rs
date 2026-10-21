// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: prusaslicer@b028299c77 src/libslic3r/ElephantFootCompensation.cpp :: elephant_foot_compensation, contour_distance, contour_distance2, resample_polygon, smooth_compensation, smooth_compensation_banded
// Source: prusaslicer@b028299c77 src/libslic3r/ClipperUtils.cpp :: mittered_offset_path_scaled, variable_offset_inner_ex, fix_after_inner_offset, fix_after_outer_offset
// Source: prusaslicer@b028299c77 src/libslic3r/ExPolygon.cpp :: ExPolygon::simplify
// Source: prusaslicer@b028299c77 src/libslic3r/MultiPoint.hpp :: douglas_peucker
// Changes: Rust port; segment bounding-box scans replace EdgeGrid; Clipper2 cleans variable offsets.
// Changes: Debug SVG output omitted; superseded ray metric and disabled unbanded smoother retained for tests.
// Copyright (c) Prusa Research 2019 - 2023 Vojtěch Bubník @bubnikv, Lukáš Hejl @hejllukas, Tomáš Mészáros @tamasmeszaros
// ClipperUtils: Copyright (c) Prusa Research 2016 - 2023 Tomáš Mészáros, Vojtěch Bubník,
// Pavel Mikuš, Lukáš Matěna, Lukáš Hejl, Filip Sykala; Slic3r 2013 - 2015 Alessandro Ranellucci;
// 2015 Maksim Derbasov. Ported from Clipper.pm: Prusa Research 2016 - 2022 Vojtěch Bubník;
// Slic3r 2011 - 2014 Alessandro Ranellucci; 2012 - 2013 Mike Sheldrake.
// ExPolygon / MultiPoint: Copyright (c) Prusa Research 2016 - 2023 Vojtěch Bubník, Lukáš Matěna,
// Lukáš Hejl, Tomáš Mészáros, Enrico Turri; Slic3r 2013 - 2016 Alessandro Ranellucci;
// 2015 Maksim Derbasov; 2014 Petr Ledvina. Ported from ExPolygon.pm: Prusa Research
// 2017 - 2022 Vojtěch Bubník; Slic3r 2011 - 2014 Alessandro Ranellucci; 2012 Mark Hindess.
// Upstream is released under the terms of the AGPLv3 or higher.

//! Thin-feature-preserving first-layer shrinkage, in PrusaSlicer's scaled coordinates.

use std::f64::consts::PI;

use clipper2_rust::{ClipType, Clipper64, FillRule, Point64, PolyTree64};

use super::clip::{diff_ex, union_ex_positive};
use super::{BoundingBox, ExPolygon, Point, Polygon, SCALE, signed_area};
use crate::flow::Flow;

const SCALED_EPSILON: f64 = 100.0;

#[derive(Clone, Copy)]
struct V(f64, f64);
impl V {
    fn dot(self, b: Self) -> f64 {
        self.0 * b.0 + self.1 * b.1
    }
    fn cross(self, b: Self) -> f64 {
        self.0 * b.1 - self.1 * b.0
    }
    fn norm(self) -> f64 {
        self.dot(self).sqrt()
    }
    fn unit(self) -> Self {
        self * (1.0 / self.norm())
    }
    fn perp(self) -> Self {
        Self(self.1, -self.0)
    }
    fn point(self) -> Point {
        Point::new(self.0 as i64, self.1 as i64)
    }
    fn rounded(self) -> Point {
        Point::new(self.0.round() as i64, self.1.round() as i64)
    }
}
impl From<Point> for V {
    fn from(p: Point) -> Self {
        Self(p.x as f64, p.y as f64)
    }
}
impl std::ops::Add for V {
    type Output = Self;
    fn add(self, b: Self) -> Self {
        Self(self.0 + b.0, self.1 + b.1)
    }
}
impl std::ops::Sub for V {
    type Output = Self;
    fn sub(self, b: Self) -> Self {
        Self(self.0 - b.0, self.1 - b.1)
    }
}
impl std::ops::Mul<f64> for V {
    type Output = Self;
    fn mul(self, b: f64) -> Self {
        Self(self.0 * b, self.1 * b)
    }
}

fn prev(i: usize, n: usize) -> usize {
    (i + n - 1) % n
}
fn next(i: usize, n: usize) -> usize {
    (i + 1) % n
}

struct ResampledPoint {
    idx_src: usize,
    interpolated: bool,
    curve_parameter: f64,
}

fn resample_polygon(contour: &[Point], dist: f64) -> (Polygon, Vec<ResampledPoint>) {
    let mut out = Vec::new();
    let mut parameters = Vec::new();
    if contour.len() > 2 {
        let mut pt_prev = V::from(*contour.last().unwrap());
        for (idx_src, &pt) in contour.iter().enumerate() {
            let pt_this = V::from(pt);
            let v = pt_this - pt_prev;
            let length = v.norm();
            let n = (length / dist).ceil().max(1.0) as usize;
            let step = length / n as f64;
            for i in 1..n {
                out.push((pt_prev + v * (i as f64 / n as f64)).point());
                parameters.push(ResampledPoint {
                    idx_src,
                    interpolated: true,
                    curve_parameter: step,
                });
            }
            out.push(pt);
            parameters.push(ResampledPoint {
                idx_src,
                interpolated: false,
                curve_parameter: step,
            });
            pt_prev = pt_this;
        }
        for i in 1..parameters.len() {
            parameters[i].curve_parameter += parameters[i - 1].curve_parameter;
        }
    }
    (out, parameters)
}

struct Segment {
    contour: usize,
    index: usize,
    a: V,
    v: V,
    length_sq: f64,
    bbox: BoundingBox,
}

/// Same candidates as EdgeGrid after distance clamping, without its repeated cell visits.
fn segments(input: &ExPolygon) -> Vec<Segment> {
    input
        .polygons()
        .enumerate()
        .flat_map(|(contour, poly)| {
            poly.iter().enumerate().map(move |(index, &a)| {
                let b = poly[next(index, poly.len())];
                let v = V::from(b) - V::from(a);
                Segment {
                    contour,
                    index,
                    a: a.into(),
                    v,
                    length_sq: v.dot(v),
                    bbox: BoundingBox {
                        min: Point::new(a.x.min(b.x), a.y.min(b.y)),
                        max: Point::new(a.x.max(b.x), a.y.max(b.y)),
                    },
                }
            })
        })
        .collect()
}

fn inside_corner(contour: &[Point], i: usize, opposite: Point) -> bool {
    let a = V::from(contour[prev(i, contour.len())]);
    let b = V::from(contour[i]);
    let c = V::from(contour[next(i, contour.len())]);
    let p = V::from(opposite);
    let v1 = b - a;
    let v2 = c - b;
    let left1 = v1.cross(p - a) > 0.0;
    let left2 = v2.cross(p - b) > 0.0;
    if v1.cross(v2) > 0.0 {
        left1 && left2
    } else {
        left1 || left2
    }
}

fn contour_distance2(
    input: &ExPolygon,
    segments: &[Segment],
    idx_contour: usize,
    contour: &[Point],
    parameters: &[ResampledPoint],
    compensation: f64,
    search_radius: f64,
) -> Vec<f32> {
    let original = if idx_contour == 0 {
        &input.contour
    } else {
        &input.holes[idx_contour - 1]
    };
    let original_parameters: Vec<f64> = parameters
        .iter()
        .filter(|p| !p.interpolated)
        .enumerate()
        .map(|(i, p)| {
            debug_assert_eq!(i, p.idx_src);
            p.curve_parameter
        })
        .collect();
    let param_end = parameters.last().unwrap().curve_parameter;
    let accept_distance = 0.5 * compensation * PI;
    contour
        .iter()
        .enumerate()
        .map(|(i, &pt)| {
            let point = V::from(pt);
            // Unnormalised sum of the inward normals of the adjoining edges.
            let dir_inside = (V::from(contour[next(i, contour.len())])
                - V::from(contour[prev(i, contour.len())]))
            .perp()
                * -1.0;
            let mut distance = search_radius;
            for s in segments {
                if (s.bbox.max.x as f64) < point.0 - search_radius
                    || s.bbox.min.x as f64 > point.0 + search_radius
                    || (s.bbox.max.y as f64) < point.1 - search_radius
                    || s.bbox.min.y as f64 > point.1 + search_radius
                {
                    continue;
                }
                let va = point - s.a;
                let t = if s.length_sq == 0.0 {
                    0.0
                } else {
                    (va.dot(s.v) / s.length_sq).clamp(0.0, 1.0)
                };
                let foot = s.a + s.v * t;
                let bisector = foot - point;
                let dist = bisector.norm();
                if dist >= distance || dir_inside.dot(bisector) <= 0.0 {
                    continue;
                }
                if s.contour == idx_contour {
                    let param_lo = parameters[i].curve_parameter;
                    let param_hi = t * s.length_sq.sqrt()
                        + if s.index + 1 < original.len() {
                            original_parameters[s.index]
                        } else {
                            0.0
                        };
                    let difference = (param_hi - param_lo).abs();
                    let along = difference.min(param_end - difference);
                    if along < accept_distance {
                        continue;
                    }
                    if dist < search_radius + SCALED_EPSILON {
                        let inside = if t == 0.0 {
                            inside_corner(original, s.index, pt)
                        } else if t == 1.0 {
                            inside_corner(original, next(s.index, original.len()), pt)
                        } else {
                            s.v.cross(point - s.a) > 0.0
                        };
                        if !inside || along <= 0.6 * PI * dist {
                            continue;
                        }
                    }
                }
                distance = dist;
            }
            distance as f32
        })
        .collect()
}

// Superseded upstream Shape Diameter Function, retained with its original fan/rejection rules.
// It is deliberately not used for compensation: it can pinch small pieces off a contour.
#[cfg(test)]
fn contour_distance(
    input: &ExPolygon,
    edges: &[Segment],
    idx_contour: usize,
    contour: &[Point],
    parameters: &[ResampledPoint],
    search_radius: f64,
) -> Vec<f32> {
    let original = if idx_contour == 0 {
        &input.contour
    } else {
        &input.holes[idx_contour - 1]
    };
    let original_parameters: Vec<_> = parameters
        .iter()
        .filter(|p| !p.interpolated)
        .map(|p| p.curve_parameter)
        .collect();
    let param_end = parameters.last().unwrap().curve_parameter;
    let bb = BoundingBox::from_points(input.polygons().flatten()).unwrap();
    let mut out = Vec::with_capacity(contour.len());
    for i in 0..contour.len() {
        let pt = V::from(contour[i]);
        let vprev = (pt - V::from(contour[prev(i, contour.len())])).unit();
        let vnext = (V::from(contour[next(i, contour.len())]) - pt).unit();
        let direction = (vprev.perp() + vnext.perp()).unit() * -1.0;
        let cross = vprev.cross(vnext);
        let dot = vprev.dot(vnext);
        let a = if cross < 0.0 || dot > 0.5 {
            PI / 3.0
        } else {
            0.48 * (-dot).min(1.0).acos()
        };
        let mut minimum = 1.0f64;
        for ray in -14..15 {
            let angle = a * f64::from(ray) / 15.0;
            let dir_unit = direction * angle.cos() + direction.perp() * angle.sin();
            let start = pt + dir_unit * SCALED_EPSILON;
            let dir = dir_unit * search_radius;
            // EdgeGrid's bounding box is enlarged by SCALED_EPSILON; then rays are trimmed back
            // by SCALED_EPSILON, leaving precisely the original bounding box here.
            let mut extent = 1.0f64;
            for (p, d, lo, hi) in [
                (start.0, dir.0, bb.min.x, bb.max.x),
                (start.1, dir.1, bb.min.y, bb.max.y),
            ] {
                if d.abs() >= 1e-4 {
                    let edge = if d > 0.0 {
                        hi as f64 - p
                    } else {
                        p - lo as f64
                    };
                    extent = extent.min(edge / d.abs());
                }
            }
            let mut t_min = 1.0;
            for edge in edges {
                let denominator = dir.cross(edge.v);
                if denominator.abs() < 1e-4 {
                    continue;
                }
                let delta = start - edge.a;
                let t = edge.v.cross(delta) / denominator;
                let t2 = dir.cross(delta) / denominator;
                if t < 0.0 || t > extent || !(0.0..=1.0).contains(&t2) {
                    continue;
                }
                let mut valid = true;
                if edge.contour == idx_contour {
                    let param_lo = parameters[i].curve_parameter;
                    let param_hi = t2 * edge.length_sq.sqrt()
                        + if edge.index + 1 < original.len() {
                            original_parameters[edge.index]
                        } else {
                            0.0
                        };
                    let distance = (param_hi - param_lo).abs();
                    valid = distance > search_radius && param_end - distance > search_radius;
                }
                if t < t_min {
                    t_min = if valid { t } else { 1.0 };
                }
            }
            minimum = minimum.min(t_min);
        }
        out.push((minimum * search_radius) as f32);
    }
    out
}

// Upstream retains this alternative under #if 0. Production uses the banded smoother below.
#[cfg(test)]
fn smooth_compensation(compensation: &mut Vec<f32>, strength: f32, iterations: usize) {
    let n = compensation.len();
    let mut out = compensation.clone();
    for _ in 0..iterations {
        for i in 0..n {
            let laplacian = compensation[i] * (1.0 - strength)
                + 0.5 * strength * (compensation[prev(i, n)] + compensation[next(i, n)]);
            out[i] = laplacian.max(compensation[i]);
        }
        std::mem::swap(compensation, &mut out);
    }
}

fn smooth_compensation_banded(
    contour: &[Point],
    band: f32,
    compensation: &mut Vec<f32>,
    strength: f32,
    iterations: usize,
) {
    let n = contour.len();
    debug_assert_eq!(n, compensation.len());
    debug_assert!(n > 2);
    // Keep upstream's f32 coordinate arithmetic as well as its f32 compensation values.
    let length_sq = |a: Point, b: Point| {
        let x = a.x as f32 - b.x as f32;
        let y = a.y as f32 - b.y as f32;
        x * x + y * y
    };
    let mut out = compensation.clone();
    for _ in 0..iterations {
        for i in 0..n {
            let sample = |step: fn(usize, usize) -> usize| {
                let mut j = step(i, n);
                let mut value = compensation[j];
                let l2 = length_sq(contour[i], contour[j]);
                if l2 < band * band {
                    let mut length = l2.sqrt();
                    let mut jprev = j;
                    j = step(j, n);
                    while j != i {
                        let this_length = length_sq(contour[j], contour[jprev]).sqrt();
                        let next_length = length + this_length;
                        if next_length > band {
                            let t = (band - length) / this_length;
                            value = compensation[jprev] * (1.0 - t) + compensation[j] * t;
                            break;
                        }
                        value = compensation[j];
                        length = next_length;
                        jprev = j;
                        j = step(j, n);
                    }
                }
                value
            };
            let laplacian =
                compensation[i] * (1.0 - strength) + 0.5 * strength * (sample(prev) + sample(next));
            // Negative offsets: smoothing may reduce shrinkage, never increase it.
            out[i] = laplacian.max(compensation[i]);
        }
        std::mem::swap(compensation, &mut out);
    }
}

/// Closed Douglas-Peucker reduction, including upstream's integer projection rounding.
fn simplify_polygon(contour: &[Point]) -> Polygon {
    if contour.len() < 3 {
        return contour.to_vec();
    }
    let mut points = contour.to_vec();
    points.push(points[0]);
    let mut out = vec![points[0]];
    let mut anchor = 0;
    let mut stack = vec![points.len() - 1];
    while let Some(&floater) = stack.last() {
        let a = points[anchor];
        let v = V::from(points[floater]) - V::from(a);
        let l2 = v.dot(v);
        let mut furthest = anchor;
        let mut max_dist_sq = -1i128;
        for (i, &p) in points.iter().enumerate().take(floater).skip(anchor + 1) {
            let va = V::from(p) - V::from(a);
            let t = if l2 == 0.0 {
                0.0
            } else {
                (va.dot(v) / l2).clamp(0.0, 1.0)
            };
            let projection = (v * t).point();
            let dist_sq = Point::new(p.x - a.x, p.y - a.y).distance_sq(projection);
            if dist_sq > max_dist_sq {
                max_dist_sq = dist_sq;
                furthest = i;
            }
        }
        if floater == anchor + 1 || max_dist_sq <= (SCALED_EPSILON * SCALED_EPSILON) as i128 {
            out.push(points[floater]);
            anchor = floater;
            stack.pop();
        } else {
            stack.push(furthest);
        }
    }
    out.pop();
    out
}

fn mittered_offset_path_scaled(contour: &[Point], deltas: &[f32], miter_limit: f64) -> Polygon {
    debug_assert_eq!(contour.len(), deltas.len());
    let mut out = Vec::new();
    if contour.len() <= 2 {
        return out;
    }
    let miter_limit = if miter_limit > 2.0 {
        2.0 / (miter_limit * miter_limit)
    } else {
        0.5
    };
    let lmin = f64::from(deltas.iter().copied().fold(f32::NEG_INFINITY, f32::max)) * 0.005;
    let l2min = lmin * lmin;
    let mut pt = V::from(contour[0]);
    let mut iprev = contour.len() - 1;
    while iprev > 0 && (V::from(contour[iprev]) - pt).dot(V::from(contour[iprev]) - pt) <= l2min {
        iprev -= 1;
    }
    if iprev == 0 {
        return out;
    }
    let ilast = iprev;
    let mut nprev = (pt - V::from(contour[iprev])).perp().unit();
    let mut i = 0;
    loop {
        let mut j = i + 1;
        while j <= ilast && (V::from(contour[j]) - pt).dot(V::from(contour[j]) - pt) <= l2min {
            j += 1;
        }
        let ptnext = if j > ilast {
            i = ilast;
            V::from(contour[0])
        } else {
            V::from(contour[j])
        };
        let nnext = (ptnext - pt).perp().unit();
        let delta = f64::from(deltas[i]);
        let sin_a = nprev.cross(nnext).clamp(-1.0, 1.0);
        let convex = sin_a * delta;
        if convex <= -1.0 {
            out.extend([
                (pt + nprev * delta).rounded(),
                pt.rounded(),
                (pt + nnext * delta).rounded(),
            ]);
        } else {
            let dot = nprev.dot(nnext);
            if convex < 1.0 && dot > 0.0 {
                out.push((pt + nprev * delta).rounded());
            } else {
                let r = 1.0 + dot;
                if r >= miter_limit {
                    out.push((pt + (nprev + nnext) * (delta / r)).rounded());
                } else {
                    let dx = (sin_a.atan2(dot) / 4.0).tan();
                    out.push((pt + (nprev - nprev.perp() * dx) * delta).rounded());
                    out.push((pt + (nnext + nnext.perp() * dx) * delta).rounded());
                }
            }
        }
        if i == ilast {
            break;
        }
        nprev = nnext;
        pt = ptnext;
        i = j;
    }
    out
}

/// `variable_offset_inner_ex`: inner offset of each contour point by its own (negative) delta.
pub fn variable_offset_inner_ex(input: &ExPolygon, deltas: &[Vec<f32>]) -> Vec<ExPolygon> {
    // Inner offsets are cleaned as holes in a negative enclosing rectangle. A positive union of the
    // raw path alone would keep the little inverted loops at convex corners.
    let raw = mittered_offset_path_scaled(&input.contour, &deltas[0], 2.0);
    let Some(bb) = BoundingBox::from_points(&raw) else {
        return Vec::new();
    };
    let mut clipper = Clipper64::new();
    let path = |p: &[Point]| p.iter().map(|p| Point64::new(p.x, p.y)).collect();
    clipper.add_subject(&vec![
        path(&raw),
        vec![
            Point64::new(bb.min.x - 10, bb.max.y + 10),
            Point64::new(bb.max.x + 10, bb.max.y + 10),
            Point64::new(bb.max.x + 10, bb.min.y - 10),
            Point64::new(bb.min.x - 10, bb.min.y - 10),
        ],
    ]);
    let mut tree = PolyTree64::new();
    clipper.execute_tree(
        ClipType::Union,
        FillRule::Negative,
        &mut tree,
        &mut Vec::new(),
    );
    let mut contours = Vec::new();
    for &outer in tree.root().children() {
        for &hole in tree.nodes[outer].children() {
            let contour = tree.nodes[hole]
                .polygon()
                .iter()
                .rev()
                .map(|p| Point::new(p.x, p.y))
                .collect();
            contours.push(ExPolygon {
                contour,
                holes: Vec::new(),
            });
        }
    }
    // Reverse CW hole paths to use the existing positive-fill union. Preserve holes-in-holes,
    // which may arise when an expanded C-shaped hole closes up.
    let holes: Vec<_> = input
        .holes
        .iter()
        .zip(&deltas[1..])
        .flat_map(|(hole, ds)| {
            let mut raw = mittered_offset_path_scaled(hole, ds, 2.0);
            raw.reverse();
            union_ex_positive(&[raw])
        })
        .collect();
    if holes.is_empty() {
        contours
    } else {
        diff_ex(&contours, &holes)
    }
}

/// Compensates each island using the first layer's external perimeter width and spacing (mm).
pub fn elephant_foot_compensation(
    input: &[ExPolygon],
    external_perimeter_flow: Flow,
    compensation: f64,
) -> Vec<ExPolygon> {
    compensate_with_min_width(
        input,
        external_perimeter_flow.width + external_perimeter_flow.spacing(),
        compensation,
    )
}

/// Variant with an explicit minimum contour width, in mm.
pub fn compensate_with_min_width(
    input: &[ExPolygon],
    min_contour_width: f64,
    compensation: f64,
) -> Vec<ExPolygon> {
    input
        .iter()
        .map(|island| compensate_polygon(island, min_contour_width, compensation))
        .collect()
}

/// Compensates one island without deleting any of its contours or holes.
pub fn compensate_polygon(
    input: &ExPolygon,
    min_contour_width: f64,
    compensation: f64,
) -> ExPolygon {
    if compensation <= 0.0 || input.contour.len() < 3 {
        return input.clone();
    }
    debug_assert!(signed_area(&input.contour) > 0.0);
    debug_assert!(input.holes.iter().all(|h| signed_area(h) < 0.0));
    let compensation = compensation * SCALE;
    let min_width = min_contour_width * SCALE;
    let width_compensated = min_width + 2.0 * compensation;
    let search_radius = width_compensated + 0.5 * min_width;
    let bb = BoundingBox::from_points(&input.contour).unwrap();
    if ((bb.max.x - bb.min.x) as f64) < width_compensated + SCALED_EPSILON
        || ((bb.max.y - bb.min.y) as f64) < width_compensated + SCALED_EPSILON
        || input.area_mm2() * SCALE * SCALE < width_compensated * width_compensated * 5.0
    {
        return input.clone();
    }
    let simplified = union_ex_positive(
        &input
            .polygons()
            .map(|p| simplify_polygon(p))
            .collect::<Vec<_>>(),
    );
    let Some(simplified) = simplified.first() else {
        return input.clone();
    };
    let edges = segments(simplified);
    let mut resampled = simplified.clone();
    let mut deltas = Vec::new();
    for (i, poly) in std::iter::once(&mut resampled.contour)
        .chain(&mut resampled.holes)
        .enumerate()
    {
        let (points, parameters) = resample_polygon(poly, 0.5 * SCALE);
        *poly = points;
        let mut distances = contour_distance2(
            simplified,
            &edges,
            i,
            poly,
            &parameters,
            compensation,
            search_radius,
        );
        for d in &mut distances {
            *d = if f64::from(*d) < min_width {
                0.0
            } else if f64::from(*d) > width_compensated {
                -(compensation as f32)
            } else {
                -(*d - min_width as f32) / 2.0
            };
        }
        smooth_compensation_banded(poly, (0.8 * 0.5 * SCALE) as f32, &mut distances, 0.3, 3);
        deltas.push(distances);
    }
    let mut out = variable_offset_inner_ex(&resampled, &deltas);
    if out.len() == 1 && out[0].holes.len() == resampled.holes.len() {
        out.remove(0)
    } else {
        input.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Polygon {
        [(x0, y0), (x1, y0), (x1, y1), (x0, y1)]
            .into_iter()
            .map(|(x, y)| Point::from_mm(x, y))
            .collect()
    }

    #[test]
    fn square_shrinks_and_hole_grows_by_requested_amount() {
        let mut hole = rect(5.0, 5.0, 15.0, 15.0);
        hole.reverse();
        let input = ExPolygon {
            contour: rect(0.0, 0.0, 20.0, 20.0),
            holes: vec![hole],
        };
        let output = compensate_polygon(&input, 0.95708, 0.2);
        assert_eq!(output.holes.len(), 1);
        assert_eq!(
            BoundingBox::from_points(&output.contour),
            Some(BoundingBox {
                min: Point::from_mm(0.2, 0.2),
                max: Point::from_mm(19.8, 19.8)
            })
        );
        assert_eq!(
            BoundingBox::from_points(&output.holes[0]),
            Some(BoundingBox {
                min: Point::from_mm(4.8, 4.8),
                max: Point::from_mm(15.2, 15.2)
            })
        );
        assert!((output.area_mm2() - (19.6 * 19.6 - 10.4 * 10.4)).abs() < 1e-5);
    }

    #[test]
    fn tiny_islands_and_zero_compensation_are_unchanged() {
        for (w, h) in [(0.6, 20.0), (20.0, 0.6), (2.0, 2.0)] {
            let input = ExPolygon {
                contour: rect(0.0, 0.0, w, h),
                holes: vec![],
            };
            assert_eq!(compensate_polygon(&input, 0.95708, 0.2), input);
        }
        let input = ExPolygon {
            contour: rect(0.0, 0.0, 20.0, 20.0),
            holes: vec![],
        };
        assert_eq!(compensate_polygon(&input, 0.95708, 0.0), input);
    }

    #[test]
    fn superseded_ray_metric_measures_opposing_edges() {
        let input = ExPolygon {
            contour: rect(0.0, 0.0, 20.0, 20.0),
            holes: vec![],
        };
        let (points, parameters) = resample_polygon(&input.contour, 0.5 * SCALE);
        let distances = contour_distance(
            &input,
            &segments(&input),
            0,
            &points,
            &parameters,
            30.0 * SCALE,
        );
        assert_eq!(distances.len(), points.len());
        // At the midpoint of an edge, the shortest accepted ray crosses the square.
        let i = points
            .iter()
            .position(|p| *p == Point::from_mm(10.0, 0.0))
            .unwrap();
        assert!((f64::from(distances[i]) / SCALE - 20.0).abs() < 0.001);
    }

    #[test]
    fn narrow_neck_survives_between_wide_regions() {
        // Two large pads connected by a neck narrower than the minimum contour width.
        let input = union_ex_positive(&[
            rect(0.0, 0.0, 5.0, 5.0),
            rect(5.0, 2.2, 10.0, 2.8),
            rect(10.0, 0.0, 15.0, 5.0),
        ])
        .remove(0);
        let output = compensate_polygon(&input, 0.95708, 0.2);
        assert!(output.area_mm2() < input.area_mm2() - 2.0);
        assert!(output.holes.is_empty());
        let neck = super::super::clip::intersection_ex(
            &[output],
            &[ExPolygon {
                contour: rect(7.0, 0.0, 8.0, 5.0),
                holes: vec![],
            }],
        );
        assert!((neck.iter().map(ExPolygon::area_mm2).sum::<f64>() - 0.6).abs() < 1e-5);
    }

    #[test]
    fn partially_compensates_necks_down_to_the_external_flow_limit() {
        for (width, expected) in [(0.6, 0.6), (1.1, 0.95708), (2.0, 1.6)] {
            let input = union_ex_positive(&[
                rect(0.0, 0.0, 5.0, 5.0),
                rect(5.0, 2.5 - width / 2.0, 10.0, 2.5 + width / 2.0),
                rect(10.0, 0.0, 15.0, 5.0),
            ])
            .remove(0);
            let output = compensate_polygon(&input, 0.95708, 0.2);
            let middle = super::super::clip::intersection_ex(
                &[output],
                &[ExPolygon {
                    contour: rect(7.0, 0.0, 8.0, 5.0),
                    holes: vec![],
                }],
            );
            let width_after = middle.iter().map(ExPolygon::area_mm2).sum::<f64>();
            assert!(
                (width_after - expected).abs() < 1e-5,
                "neck {width}: {width_after}"
            );
        }
    }

    #[test]
    fn topology_change_falls_back_to_the_original_island() {
        let input = union_ex_positive(&[
            rect(0.0, 0.0, 20.0, 20.0),
            rect(20.0, 9.8, 30.0, 10.2),
            rect(30.0, 0.0, 50.0, 20.0),
        ])
        .remove(0);
        // An explicit zero minimum permits the neck to disappear; the topology guard must reject it.
        let output = compensate_polygon(&input, 0.0, 1.0);
        assert_eq!(output, input);
    }

    #[test]
    fn both_smoothers_only_reduce_shrinkage_and_wrap_the_contour() {
        let original = vec![0.0, -200_000.0, -200_000.0, -200_000.0];
        let mut simple = original.clone();
        smooth_compensation(&mut simple, 0.4, 1);
        assert_eq!(simple, vec![0.0, -160_000.0, -200_000.0, -160_000.0]);
        let contour = rect(0.0, 0.0, 0.1, 0.1);
        let mut banded = original.clone();
        smooth_compensation_banded(&contour, 150_000.0, &mut banded, 0.3, 3);
        for (a, b) in banded.iter().zip(&original) {
            assert!(*a >= *b && *a <= 0.0);
        }
        assert_eq!(banded[1], banded[3]);
        assert!(banded[2] > original[2]);
    }
}
