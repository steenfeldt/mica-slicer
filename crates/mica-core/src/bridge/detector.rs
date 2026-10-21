// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/BridgeDetector.cpp :: BridgeDetector
// Source: prusaslicer@b028299c77 src/libslic3r/BridgeDetector.hpp :: detect_bridging_direction
// Source: prusaslicer@b028299c77 src/libslic3r/PrincipalComponents2D.cpp :: compute_principal_components
// Changes: Clipper2 clipping; deterministic angle ties; polygon area moments in f64 instead of Eigen.
// Changes: Production support coverage uses sampled two-ended spans to avoid legacy trapezoid overcoverage.
// Attribution from src/libslic3r/BridgeDetector.cpp:
// Copyright (c) Prusa Research 2016 - 2021 Vojtěch Bubník @bubnikv
// Copyright (c) Slic3r 2014 - 2016 Alessandro Ranellucci @alranel
// Copyright (c) 2015 Maksim Derbasov @ntfshard
//
// PrusaSlicer is released under the terms of the AGPLv3 or higher
//
// Attribution from src/libslic3r/BridgeDetector.hpp:
// Copyright (c) Prusa Research 2016 - 2023 Pavel Mikuš @Godrak, Vojtěch Bubník @bubnikv
// Copyright (c) Slic3r 2014 - 2015 Alessandro Ranellucci @alranel
//
// PrusaSlicer is released under the terms of the AGPLv3 or higher
//
// Attribution from src/libslic3r/PrincipalComponents2D.cpp:
// Copyright (c) Prusa Research 2022 - 2023 Pavel Mikuš @Godrak
//
// PrusaSlicer is released under the terms of the AGPLv3 or higher
//
//! The legacy visibility detector and the active unanchored-edge direction estimator.
use crate::geometry::clip::{
    Join, closing_ex, diff_ex, intersection_ex, intersection_pl, offset_ex, union_ex,
    union_safety_offset_ex,
};
use crate::geometry::{BoundingBox, ExPolygon, Point, Polyline, SCALE};
use rayon::prelude::*;
use std::f64::consts::{FRAC_PI_2, PI};

pub(crate) fn offset(ex: &[ExPolygon], delta: f64) -> Vec<ExPolygon> {
    offset_ex(ex, delta, Join::Miter)
}
pub(crate) fn boundaries(ex: &[ExPolygon]) -> Vec<Polyline> {
    ex.iter()
        .flat_map(|e| e.polygons())
        .filter(|p| p.len() >= 3)
        .map(|p| {
            let mut line = p.clone();
            line.push(p[0]);
            line
        })
        .collect()
}
pub(crate) fn segments(lines: &[Polyline]) -> Vec<[Point; 2]> {
    lines
        .iter()
        .flat_map(|l| l.windows(2).map(|w| [w[0], w[1]]))
        .filter(|w| w[0] != w[1])
        .collect()
}
pub(crate) fn rotate_point(p: Point, angle: f64) -> Point {
    let (s, c) = angle.sin_cos();
    Point::new(
        (c * p.x as f64 - s * p.y as f64).round() as i64,
        (s * p.x as f64 + c * p.y as f64).round() as i64,
    )
}
pub(crate) fn rotate(ex: &[ExPolygon], angle: f64) -> Vec<ExPolygon> {
    ex.iter()
        .map(|e| ExPolygon {
            contour: e.contour.iter().map(|&p| rotate_point(p, angle)).collect(),
            holes: e
                .holes
                .iter()
                .map(|h| h.iter().map(|&p| rotate_point(p, angle)).collect())
                .collect(),
        })
        .collect()
}
fn direction(a: Point, b: Point) -> f64 {
    ((b.y - a.y) as f64)
        .atan2((b.x - a.x) as f64)
        .rem_euclid(PI)
}
fn parallel(a: f64, b: f64, tolerance: f64) -> bool {
    let d = (a - b).abs().rem_euclid(PI);
    d.min(PI - d) < tolerance
}
fn contains(ex: &[ExPolygon], p: Point) -> bool {
    use clipper2_rust::{Point64, PointInPolygonResult, point_in_polygon};
    let inside = |ring: &[Point]| {
        point_in_polygon(
            Point64::new(p.x, p.y),
            &ring.iter().map(|p| Point64::new(p.x, p.y)).collect(),
        )
    };
    ex.iter().any(|e| {
        inside(&e.contour) != PointInPolygonResult::IsOutside
            && !e
                .holes
                .iter()
                .any(|h| inside(h) == PointInPolygonResult::IsInside)
    })
}

/// Prusa's active direction rule: minimize bridge ends in air; enclosed holes use their short axis.
pub fn direction_from_anchors(area: &[ExPolygon], anchors: &[ExPolygon]) -> f64 {
    direction_from_offset_anchors(area, &offset(anchors, 100.0))
}

fn direction_from_offset_anchors(area: &[ExPolygon], anchors: &[ExPolygon]) -> f64 {
    let floating = crate::geometry::clip::diff_pl(&boundaries(area), anchors);
    let edges = segments(&floating);
    if edges.is_empty() {
        return short_axis(area);
    }
    let mut candidates = Vec::new();
    let mut quantized = std::collections::BTreeSet::new();
    for &[a, b] in &edges {
        let angle = direction(a, b);
        if quantized.insert(((angle + FRAC_PI_2) * 1000.0).ceil() as i64) {
            candidates.push(angle);
        }
    }
    // The first candidate of least cost (the summed edge lengths across the direction).
    // Every term is non-negative, so a partial sum above the best cost so far can only grow:
    // that candidate is dropped early, with the same sums and the same winner as summing all.
    let terms: Vec<(f64, f64)> = edges
        .iter()
        .map(|[a, b]| (-(b.x - a.x) as f64, (b.y - a.y) as f64))
        .collect();
    let mut best: Option<(f64, f64)> = None;
    for angle in candidates {
        let (s, c) = angle.sin_cos();
        let bound = best.map_or(f64::INFINITY, |(cost, _)| cost);
        let mut sum = -0.0;
        let mut beaten = false;
        for &(x, y) in &terms {
            sum += (x * s + y * c).abs();
            if sum > bound {
                beaten = true;
                break;
            }
        }
        if !beaten && sum < bound {
            best = Some((sum, angle));
        }
    }
    best.map_or(0.0, |(_, angle)| angle)
}

fn short_axis(area: &[ExPolygon]) -> f64 {
    let origin = area
        .first()
        .and_then(|e| e.contour.first())
        .copied()
        .unwrap_or_default();
    let (mut a, mut mx, mut my, mut xx, mut yy, mut xy) = (0., 0., 0., 0., 0., 0.);
    for line in boundaries(area) {
        for w in line.windows(2) {
            let (x, y) = (
                (w[0].x - origin.x) as f64 / SCALE,
                (w[0].y - origin.y) as f64 / SCALE,
            );
            let (u, v) = (
                (w[1].x - origin.x) as f64 / SCALE,
                (w[1].y - origin.y) as f64 / SCALE,
            );
            let cross = x * v - u * y;
            a += cross / 2.;
            mx += (x + u) * cross / 6.;
            my += (y + v) * cross / 6.;
            xx += (x * x + x * u + u * u) * cross / 12.;
            yy += (y * y + y * v + v * v) * cross / 12.;
            xy += (2. * x * y + x * v + u * y + 2. * u * v) * cross / 24.;
        }
    }
    if a <= 0. {
        return 0.;
    }
    let vx = xx / a - (mx / a).powi(2);
    let vy = yy / a - (my / a).powi(2);
    let cov = xy / a - mx * my / (a * a);
    (0.5 * (2. * cov).atan2(vx - vy) + FRAC_PI_2).rem_euclid(PI)
}

/// Visibility-based bridge detector, retained for upstream regression tests and support coverage.
pub struct BridgeDetector {
    area: Vec<ExPolygon>,
    lower: Vec<ExPolygon>,
    anchors: Vec<ExPolygon>,
    edges: Vec<Polyline>,
    pub spacing: f64,
    pub angle: Option<f64>,
}
impl BridgeDetector {
    /// `spacing` in millimetres; angles in radians. Geometry uses Mica's scaled coordinates.
    pub fn new(area: &[ExPolygon], lower: &[ExPolygon], spacing: f64) -> Self {
        assert!(spacing.is_finite() && spacing > 0.0);
        let grown = offset(area, spacing * SCALE);
        let contours: Vec<_> = lower
            .iter()
            .map(|e| ExPolygon {
                contour: e.contour.clone(),
                holes: Vec::new(),
            })
            .collect();
        Self {
            area: area.to_vec(),
            lower: lower.to_vec(),
            anchors: intersection_ex(&grown, &union_safety_offset_ex(lower)),
            edges: intersection_pl(&boundaries(&grown), &contours),
            spacing,
            angle: None,
        }
    }
    pub fn detect_angle(&mut self, override_angle: f64) -> Option<f64> {
        self.angle = None;
        if self.edges.is_empty() || self.anchors.is_empty() {
            return None;
        }
        let spacing = self.spacing * SCALE;
        let mut candidates: Vec<_> = if override_angle != 0. {
            vec![override_angle]
        } else {
            let mut angles: Vec<_> = (0..=36).map(|i| i as f64 * PI / 36.).collect();
            angles.extend(
                segments(&boundaries(&self.area))
                    .iter()
                    .map(|[a, b]| direction(*a, *b)),
            );
            angles.extend(
                self.edges
                    .iter()
                    .filter(|e| e.first() != e.last())
                    .map(|e| direction(e[0], *e.last().unwrap())),
            );
            angles.sort_by(f64::total_cmp);
            angles.dedup_by(|a, b| parallel(*a, *b, PI / 180.));
            if angles.len() > 1 && parallel(angles[0], *angles.last().unwrap(), PI / 180.) {
                angles.pop();
            }
            angles
        }
        .into_iter()
        .map(|a| (a, 0.0f64, 0.0f64))
        .collect();
        let clip = offset(&self.area, 0.5 * spacing);
        for (angle, coverage, max_length) in &mut candidates {
            let rotated = rotate(&self.anchors, -*angle);
            let Some(bb) = BoundingBox::from_points(rotated.iter().flat_map(|e| e.contour.iter()))
            else {
                continue;
            };
            let mut lines = Vec::new();
            let mut y = bb.min.y;
            while y <= bb.max.y {
                lines.push(vec![
                    rotate_point(Point::new(bb.min.x, y), *angle),
                    rotate_point(Point::new(bb.max.x, y), *angle),
                ]);
                y += spacing.round().max(1.) as i64;
            }
            for line in intersection_pl(&lines, &clip) {
                if contains(&self.anchors, line[0])
                    && contains(&self.anchors, *line.last().unwrap())
                {
                    let len = crate::geometry::polyline_length(&line);
                    *coverage += len;
                    *max_length = max_length.max(len);
                }
            }
        }
        candidates.sort_by(|a, b| b.1.total_cmp(&a.1));
        let mut best = *candidates.first()?;
        if best.1 == 0. {
            return None;
        }
        for candidate in candidates.into_iter().skip(1) {
            if best.1 - candidate.1 >= spacing {
                break;
            }
            if candidate.2 < best.2 {
                best = candidate;
            }
        }
        self.angle = Some(best.0.rem_euclid(PI));
        self.angle
    }
    /// Upstream trapezoid coverage, clipped back to the original bridge including its holes.
    pub fn coverage(&self, angle: f64) -> Vec<ExPolygon> {
        let turn = FRAC_PI_2 - angle;
        let anchors = rotate(&self.anchors, turn);
        let grown = offset(&rotate(&self.area, turn), self.spacing * SCALE * 0.5);
        let mut covered = Vec::new();
        for ex in grown {
            let Some(bb) = BoundingBox::from_points(ex.contour.iter()) else {
                continue;
            };
            let mut xs: Vec<_> = ex.polygons().flat_map(|p| p.iter().map(|p| p.x)).collect();
            xs.sort_unstable();
            xs.dedup();
            for pair in xs.windows(2) {
                let slab = ExPolygon {
                    contour: vec![
                        Point::new(pair[0], bb.min.y),
                        Point::new(pair[1], bb.min.y),
                        Point::new(pair[1], bb.max.y),
                        Point::new(pair[0], bb.max.y),
                    ],
                    holes: Vec::new(),
                };
                for trap in intersection_ex(std::slice::from_ref(&ex), &[slab]) {
                    let lines: Vec<_> = segments(&boundaries(std::slice::from_ref(&trap)))
                        .iter()
                        .map(|l| l.to_vec())
                        .collect();
                    let supported = intersection_pl(&lines, &anchors)
                        .iter()
                        .filter(|l| {
                            crate::geometry::polyline_length(l) >= self.spacing * SCALE - 1.
                        })
                        .count();
                    if supported >= 2 {
                        covered.push(trap);
                    }
                }
            }
        }
        intersection_ex(&self.area, &rotate(&union_ex(&covered), -turn))
    }
    /// Conservative production coverage from scanlines with both ends inside anchors.
    /// The legacy trapezoid method counts supported edges irrespective of their direction;
    /// it can overclaim a whole rectangle when its two supported edges run along the bridge.
    pub fn anchored_coverage(&self, angle: f64) -> Vec<ExPolygon> {
        anchored_coverage(&self.area, &self.anchors, self.spacing, angle)
    }

    pub fn unsupported_edges(&self, angle: f64) -> Vec<Polyline> {
        segments(&crate::geometry::clip::diff_pl(
            &boundaries(&self.area),
            &offset(&self.lower, self.spacing * SCALE),
        ))
        .into_iter()
        .filter(|[a, b]| !parallel(direction(*a, *b), angle, 1e-4))
        .map(|l| l.to_vec())
        .collect()
    }
}

// The support prepass needs only anchors and the footprint, not the visibility detector's
// cloned lower layer and floating boundary edges. Share the identical coverage calculation.
fn anchored_coverage(
    area: &[ExPolygon],
    anchors: &[ExPolygon],
    spacing: f64,
    angle: f64,
) -> Vec<ExPolygon> {
    let rotated = rotate(area, -angle);
    let Some(bb) = BoundingBox::from_points(rotated.iter().flat_map(|e| e.contour.iter())) else {
        return Vec::new();
    };
    if anchors.is_empty() {
        return Vec::new();
    }
    let spacing = spacing * SCALE;
    let count = ((bb.max.y - bb.min.y) as f64 / spacing).ceil().max(1.0) as usize;
    let step = (bb.max.y - bb.min.y) as f64 / count as f64;
    let clip = offset(area, 0.5 * spacing);
    let mut covered = Vec::new();
    for i in 0..count {
        let y = (bb.min.y as f64 + (i as f64 + 0.5) * step).round() as i64;
        let line = vec![
            rotate_point(Point::new(bb.min.x - spacing.ceil() as i64, y), angle),
            rotate_point(Point::new(bb.max.x + spacing.ceil() as i64, y), angle),
        ];
        for span in intersection_pl(&[line], &clip) {
            if contains(anchors, span[0]) && contains(anchors, *span.last().unwrap()) {
                covered.extend(crate::geometry::clip::offset_polylines(
                    &[span],
                    step * 0.5 + 1.0,
                ));
            }
        }
    }
    intersection_ex(area, &union_ex(&covered))
}

/// Full-outline bridge coverage for supports. Only two-ended anchored spans can exclude support.
#[cfg(test)]
pub(crate) fn support_coverage(
    outlines: &[Vec<ExPolygon>],
    spacing: f64,
    override_angle: f64,
) -> Vec<Vec<ExPolygon>> {
    (0..outlines.len())
        .into_par_iter()
        .map(|i| coverage_layer(outlines, i, spacing, override_angle))
        .collect()
}

/// [`support_coverage`] of one layer (none for the first).
fn coverage_layer(
    outlines: &[Vec<ExPolygon>],
    i: usize,
    spacing: f64,
    override_angle: f64,
) -> Vec<ExPolygon> {
    if i == 0 {
        return Vec::new();
    }
    let _timer = crate::timing::Span::new(crate::timing::Stage::SupportBridges);
    let overhangs = diff_ex(&outlines[i], &outlines[i - 1]);
    if overhangs.is_empty() {
        return Vec::new();
    }
    // Every overhang island sees the same lower layer. Reuse its safety union
    // and direction-rule offset in source order.
    let lower = &outlines[i - 1];
    let safe_lower = union_safety_offset_ex(lower);
    let direction_lower = offset(lower, 100.0);
    // Islands are independent; a large overhang layer otherwise keeps one worker busy
    // after the others finish. Collected in source order.
    let parts: Vec<Vec<ExPolygon>> = overhangs
        .par_iter()
        .map(|ex| {
            let one = std::slice::from_ref(ex);
            let anchors = intersection_ex(&offset(one, spacing * SCALE), &safe_lower);
            let angle = if override_angle > 0.0 {
                override_angle.to_radians()
            } else {
                direction_from_offset_anchors(one, &direction_lower)
            };
            anchored_coverage(one, &anchors, spacing, angle)
        })
        .collect();
    let coverage: Vec<ExPolygon> = parts.into_iter().flatten().collect();
    closing_ex(&coverage, 10.)
}

/// [`support_coverage`] computed per layer on first use: supports subtract a layer's bridge
/// coverage only from a non-empty overhang, so layers without one never need it.
pub struct LazyCoverage<'a> {
    outlines: &'a [Vec<ExPolygon>],
    spacing: f64,
    override_angle: f64,
    layers: Vec<std::sync::OnceLock<Vec<ExPolygon>>>,
}
impl<'a> LazyCoverage<'a> {
    pub(crate) fn new(outlines: &'a [Vec<ExPolygon>], spacing: f64, override_angle: f64) -> Self {
        Self {
            outlines,
            spacing,
            override_angle,
            layers: (0..outlines.len()).map(|_| Default::default()).collect(),
        }
    }
    pub(crate) fn layer(&self, i: usize) -> &[ExPolygon] {
        self.layers[i]
            .get_or_init(|| coverage_layer(self.outlines, i, self.spacing, self.override_angle))
    }
}
