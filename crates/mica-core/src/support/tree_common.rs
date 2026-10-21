// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/Support/TreeSupportCommon.cpp :: TreeSupportSettings
// Source: prusaslicer@b028299c77 src/libslic3r/Support/TreeSupportCommon.hpp :: getRadius, recommendedMinRadius
// Source: prusaslicer@b028299c77 src/libslic3r/Support/TreeSupport.hpp :: SupportElement
// Changes: Millimetre settings and stable index arena; single object, no global mutable settings.
// Copyright (c) Prusa Research 2023 Vojtěch Bubník @bubnikv
// PrusaSlicer is released under the terms of the AGPLv3 or higher.
// Tree supports by Thomas Rahm, losely based on Tree Supports by CuraEngine.
// Original source of Thomas Rahm's tree supports:
// https://github.com/ThomasRahm/CuraEngine
//
// Original CuraEngine copyright:
// Copyright (c) 2021 Ultimaker B.V.
// CuraEngine is released under the terms of the AGPLv3 or higher.

use super::{SupportConfig, SupportError};
use crate::geometry::{ExPolygon, Point, SCALE};

#[derive(Clone, Debug, PartialEq)]
pub struct TreeConfig {
    pub angle: f64,
    pub angle_slow: f64,
    pub branch_diameter: f64,
    pub branch_diameter_angle: f64,
    /// Zero disables the second wall; otherwise compare the island area with a circle this wide.
    pub branch_diameter_double_wall: f64,
    pub branch_distance: f64,
    pub tip_diameter: f64,
    pub top_rate: f64,
}
impl TreeConfig {
    pub(super) fn validate(&self) -> Result<(), SupportError> {
        if ![self.angle, self.angle_slow, self.branch_diameter_angle]
            .iter()
            .all(|v| v.is_finite() && (0.0..90.0).contains(v))
            || ![
                self.branch_diameter,
                self.branch_distance,
                self.tip_diameter,
            ]
            .iter()
            .all(|v| v.is_finite() && *v > 0.0)
            || !self.branch_diameter_double_wall.is_finite()
            || self.branch_diameter_double_wall < 0.0
            || !self.top_rate.is_finite()
            || self.top_rate <= 0.0
            || self.top_rate > 1.0
        {
            return Err(SupportError("tree angles must be in [0, 90), diameters/distance positive, double-wall diameter nonnegative and top rate in (0, 100%]".into()));
        }
        Ok(())
    }
}

pub(super) struct TreeSettings {
    pub tip_radius: f64,
    pub branch_radius: f64,
    pub tip_height: f64,
    pub growth: f64,
    pub fast: f64,
    pub slow: f64,
    pub foot_growth: f64,
    pub connect_length: f64,
}
impl TreeSettings {
    pub fn new(c: &SupportConfig) -> Self {
        let t = &c.tree;
        let tip_radius = 0.5 * t.tip_diameter.min(t.branch_diameter).max(c.width);
        let branch_radius = (0.5 * t.branch_diameter).max(tip_radius);
        Self {
            tip_radius,
            branch_radius,
            tip_height: (((branch_radius - tip_radius) / (c.width / 3.0))
                .max(branch_radius / c.layer_height)
                .floor()
                .max(1.0))
                * c.layer_height,
            growth: t.branch_diameter_angle.to_radians().tan(),
            fast: t.angle.to_radians().tan(),
            slow: t.angle_slow.min(t.angle).to_radians().tan(),
            foot_growth: 0.7f64.tan().min(0.5 * c.width / c.layer_height),
            connect_length: c.width / t.top_rate + (2.0 * tip_radius - c.width).max(0.0),
        }
    }
    pub fn radius(&self, distance: f64) -> f64 {
        if distance <= self.tip_height {
            self.tip_radius + (self.branch_radius - self.tip_radius) * distance / self.tip_height
        } else {
            self.branch_radius + (distance - self.tip_height) * self.growth
        }
    }
    /// Advance the effective radius by one layer. A branch that was unable to grow in a
    /// passage resumes from its frozen radius, without catching up its entire distance to top.
    pub fn next_radius(&self, radius: f64, dz: f64) -> f64 {
        let tip_slope = (self.branch_radius - self.tip_radius) / self.tip_height;
        if radius < self.branch_radius && tip_slope > 0.0 {
            let tip_distance = ((self.branch_radius - radius) / tip_slope).min(dz);
            radius + tip_distance * tip_slope + (dz - tip_distance) * self.growth
        } else {
            radius + dz * self.growth
        }
    }
    pub fn recommended_radius(&self, height: f64) -> f64 {
        (3.75 - height * self.foot_growth).max(self.branch_radius)
    }
}

/// Counts before flange expansion. Roots may share a first-layer flange.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TreeStatistics {
    pub tips: usize,
    pub supported_tips: usize,
    pub bed_trunks: usize,
    pub model_trunks: usize,
}

pub(super) type ElementId = usize;
#[derive(Clone, Debug)]
pub(super) struct Element {
    pub layer: usize,
    /// Parents are above this element; each has at most one child below.
    pub parents: Vec<ElementId>,
    pub child: Option<ElementId>,
    pub area: Vec<ExPolygon>,
    pub position: Point,
    pub radius: f64,
    pub distance_to_top: f64,
    pub to_buildplate: bool,
    pub valid: bool,
}
#[derive(Default)]
pub(super) struct Forest {
    pub elements: Vec<Element>,
    pub layers: Vec<Vec<ElementId>>,
}
impl Forest {
    pub fn insert(&mut self, element: Element) -> ElementId {
        let id = self.elements.len();
        self.layers[element.layer].push(id);
        for &parent in &element.parents {
            self.elements[parent].child = Some(id);
        }
        self.elements.push(element);
        id
    }
}

pub(super) fn circle(center: Point, radius: f64) -> Vec<ExPolygon> {
    vec![ExPolygon {
        contour: (0..25)
            .map(|i| {
                let a = std::f64::consts::TAU * i as f64 / 25.0;
                Point::new(
                    center.x + (radius * SCALE * a.cos()).round() as i64,
                    center.y + (radius * SCALE * a.sin()).round() as i64,
                )
            })
            .collect(),
        holes: Vec::new(),
    }]
}

pub(super) fn contains(area: &[ExPolygon], point: Point) -> bool {
    fn inside(poly: &[Point], p: Point) -> bool {
        let mut yes = false;
        for (a, b) in poly
            .iter()
            .zip(poly.iter().cycle().skip(1))
            .take(poly.len())
        {
            if (a.y > p.y) != (b.y > p.y)
                && (p.x as f64)
                    < a.x as f64 + (p.y - a.y) as f64 * (b.x - a.x) as f64 / (b.y - a.y) as f64
            {
                yes = !yes;
            }
        }
        yes
    }
    area.iter()
        .any(|ex| inside(&ex.contour, point) && !ex.holes.iter().any(|h| inside(h, point)))
}

/// Project onto the closest boundary if outside. All tie breaking follows polygon order.
pub(super) fn move_inside(area: &[ExPolygon], point: Point) -> Point {
    if contains(area, point) {
        return point;
    }
    let mut best = point;
    let mut distance = f64::INFINITY;
    for poly in area.iter().flat_map(ExPolygon::polygons) {
        for (a, b) in poly
            .iter()
            .zip(poly.iter().cycle().skip(1))
            .take(poly.len())
        {
            let dx = (b.x - a.x) as f64;
            let dy = (b.y - a.y) as f64;
            let t = (((point.x - a.x) as f64 * dx + (point.y - a.y) as f64 * dy)
                / (dx * dx + dy * dy).max(1.0))
            .clamp(0.0, 1.0);
            let q = Point::new(a.x + (t * dx).round() as i64, a.y + (t * dy).round() as i64);
            let d = q.distance_to(point);
            if d < distance {
                best = q;
                distance = d;
            }
        }
    }
    best
}

/// Tree influence regions use the upstream 15 µm simplification tolerance. Without this,
/// repeated offsets retain sub-micron edges and make the propagation cost grow every layer.
pub(super) fn simplify(mut area: Vec<ExPolygon>) -> Vec<ExPolygon> {
    let original = area.clone();
    for ex in &mut area {
        for poly in std::iter::once(&mut ex.contour).chain(&mut ex.holes) {
            let path: Vec<_> = poly
                .iter()
                .map(|p| clipper2_rust::Point64::new(p.x, p.y))
                .collect();
            *poly = clipper2_rust::simplify_path(&path, 0.015 * SCALE, true)
                .into_iter()
                .map(|p| Point::new(p.x, p.y))
                .collect();
        }
    }
    let out = crate::geometry::clip::union_ex(&area);
    if out.is_empty() { original } else { out }
}

pub(super) fn offset_ex(
    area: &[ExPolygon],
    delta: f64,
    join: crate::geometry::clip::Join,
) -> Vec<ExPolygon> {
    let paths: Vec<_> = area
        .iter()
        .flat_map(ExPolygon::polygons)
        .map(|p| {
            p.iter()
                .map(|p| clipper2_rust::Point64::new(p.x, p.y))
                .collect()
        })
        .collect();
    let paths = clipper2_rust::inflate_paths_64(
        &paths,
        delta,
        join.into(),
        clipper2_rust::EndType::Polygon,
        1.2,
        0.01 * SCALE,
    );
    let polygons: Vec<_> = paths
        .iter()
        .map(|p| p.iter().map(|p| Point::new(p.x, p.y)).collect())
        .collect();
    crate::geometry::clip::union_ex_positive(&polygons)
}
