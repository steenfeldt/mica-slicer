// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: prusaslicer@b028299c77 src/libslic3r/Support/TreeSupport.cpp :: generate_initial_areas, sample_overhang_area, RichInterfacePlacer, ensure_maximum_distance_polyline, generate_support_infill_lines
// Source: orca@8500fcdcca src/libslic3r/Support/TreeSupport3D.cpp :: generate_initial_areas, sample_overhang_area, RichInterfacePlacer, ensure_maximum_distance_polyline, generate_support_infill_lines
// Source: prusaslicer@b028299c77 src/libslic3r/Support/TreeSupportCommon.hpp :: InterfacePlacer
// Source: orca@8500fcdcca src/libslic3r/Support/TreeSupportCommon.hpp :: InterfacePlacer
// Source: prusaslicer@b028299c77 src/libslic3r/Support/SupportParameters.cpp :: SupportParameters
// Source: orca@8500fcdcca src/libslic3r/Support/SupportParameters.hpp :: SupportParameters
// Source: prusaslicer@b028299c77 src/libslic3r/Flow.cpp :: Flow::rounded_rectangle_extrusion_spacing
// Changes: Rust owned geometry, deterministic scheduling (per-layer overhang geometry in parallel, tips sampled in layer order) and Clipper2 substitutions.
// Copyright (c) Prusa Research 2022 - 2023 Vojtěch Bubník @bubnikv, Lukáš Matěna @lukasmatena, Tomáš Mészáros @tamasmeszaros
// Copyright (c) Prusa Research 2023 Vojtěch Bubník @bubnikv
// Copyright (c) Prusa Research 2016 - 2023 Pavel Mikuš @Godrak, Oleksandra Iushchenko @YuSanka, Vojtěch Bubník @bubnikv, Tomáš Mészáros @tamasmeszaros
// Copyright (c) Slic3r 2014 - 2015 Alessandro Ranellucci @alranel
// Copyright (c) 2014 Petr Ledvina @ledvinap
// Copyright (c) Prusa Research 2022 Vojtěch Bubník @bubnikv
// Copyright (c) Slic3r 2012 - 2014 Alessandro Ranellucci @alranel
// PrusaSlicer is released under the terms of the AGPLv3 or higher.
// Tree supports by Thomas Rahm, losely based on Tree Supports by CuraEngine.
// Original source of Thomas Rahm's tree supports: https://github.com/ThomasRahm/CuraEngine
// Copyright (c) 2021 Ultimaker B.V.
// CuraEngine is released under the terms of the AGPLv3 or higher.

use super::{geometry::*, *};
use crate::{
    fill::{self, FillParams, Pattern},
    geometry::{Point, Polyline, SCALE, clip, polygon_length, polyline_length},
};
use rayon::prelude::*;
use std::collections::BTreeSet;

/// Resolved SupportParameters subset. Flow spacings are millimetres and angles
/// radians, exactly as in upstream. Supply the object's resolved flow/tool and
/// interface/base layer counts; this API does not consult the old scheduler.
#[derive(Clone, Debug, PartialEq)]
pub struct TreeSupportInfill {
    pub base_spacing: f64,
    pub interface_spacing: f64,
    pub interface_density: f64,
    pub top_interface_density: f64,
    pub base_angle: f32,
    pub interface_angle: f32,
    pub base_pattern: Pattern,
    pub interface_pattern: Pattern,
    pub soluble_interface: bool,
    pub zero_gap_interface_top: bool,
    /// Upstream SupportParameters counts (Prusa excludes top contact, Orca includes it).
    pub num_top_interface_layers: usize,
    pub num_top_base_interface_layers: usize,
    /// Object centre on the bed. Upstream works in object coordinates centred on
    /// `PrintObject::m_center_offset`; the tip deduplication hash (truncating division)
    /// is taken relative to it. The sampling fill grid is anchored per ExPolygon.
    pub origin: Point,
}
impl TreeSupportInfill {
    /// Common single-material setup. Multi-material callers must supply the
    /// resolved layer counts, zero-gap flags, spacings and angles explicitly.
    pub fn new(c: &TreeSupportSettings) -> Self {
        let spacing = |w: i64| {
            f64::from(
                (w as f64 / SCALE) as f32
                    - (c.layer_height as f64 / SCALE) as f32
                        * (1. - std::f64::consts::PI / 4.) as f32,
            )
        };
        let base = spacing(c.support_line_width);
        let interface = spacing(c.support_roof_line_width);
        let density = (interface
            / (interface
                + (c.support_roof_line_distance - c.support_roof_line_width) as f64 / SCALE))
            .min(1.);
        let support_density = (base / (base + c.support_line_spacing as f64 / SCALE)).min(1.);
        Self {
            base_spacing: base,
            interface_spacing: interface,
            interface_density: density,
            top_interface_density: density,
            base_angle: 0.,
            interface_angle: std::f32::consts::FRAC_PI_2,
            base_pattern: if c.support_pattern == TreeSupportPattern::Honeycomb {
                Pattern::Honeycomb
            } else if support_density > 0.95 {
                Pattern::Rectilinear
            } else {
                Pattern::SupportBase
            },
            interface_pattern: if density > 0.95 {
                Pattern::Rectilinear
            } else {
                Pattern::SupportBase
            },
            soluble_interface: c.zero_top_z_gap,
            zero_gap_interface_top: c.zero_top_z_gap,
            num_top_interface_layers: if c.settings.rules.is_orca() {
                c.settings.support_roof_layers.max(0) as usize
            } else {
                (c.settings.support_roof_layers - 1).max(0) as usize
            },
            num_top_base_interface_layers: 0,
            origin: Point::default(),
        }
    }
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TreeRoofs {
    pub top_contacts: Vec<Areas>,
    pub top_interfaces: Vec<Areas>,
    pub top_base_interfaces: Vec<Areas>,
}
impl TreeRoofs {
    pub fn new(layers: usize) -> Self {
        Self {
            top_contacts: vec![vec![]; layers],
            top_interfaces: vec![vec![]; layers],
            top_base_interfaces: vec![vec![]; layers],
        }
    }
    pub(super) fn add(
        &mut self,
        c: &TreeSupportSettings,
        p: &TreeSupportInfill,
        area: Areas,
        layer: usize,
        dtt: usize,
    ) {
        let layers = if dtt == 0 {
            &mut self.top_contacts
        } else if c.roof_is_interface(
            dtt,
            p.num_top_interface_layers
                .saturating_sub(p.num_top_base_interface_layers),
            p.num_top_base_interface_layers,
        ) {
            &mut self.top_interfaces
        } else {
            &mut self.top_base_interfaces
        };
        layers[layer].extend(area);
    }
}

/// Preserve the upstream conditional's precedence: `interface_angle + parity`
/// is tested as a boolean, rather than added to the chosen +/-45 degree angle.
pub fn generate_support_infill_lines(
    area: &[crate::geometry::ExPolygon],
    c: &TreeSupportSettings,
    p: &TreeSupportInfill,
    roof: bool,
    layer: usize,
    distance: i64,
) -> Vec<Polyline> {
    let spacing = if roof {
        p.interface_spacing
    } else {
        p.base_spacing
    };
    let angle = if roof {
        if p.interface_angle + (layer & 1) as f32 != 0. {
            -std::f32::consts::FRAC_PI_4
        } else {
            std::f32::consts::FRAC_PI_4
        }
    } else {
        p.base_angle
    };
    let density = if roof {
        if c.settings.rules.is_orca() {
            p.top_interface_density
        } else {
            p.interface_density
        }
    } else {
        let scaled_spacing = (spacing / 0.000_001) as f32;
        f64::from(scaled_spacing / (scaled_spacing + distance as f32))
    };
    let mut params = FillParams {
        spacing,
        density: f64::from(density as f32),
        angle: f64::from(angle).to_degrees(),
        layer_id: layer,
        z: 0.,
        anchor_length: 1000.,
        anchor_length_max: 1000.,
        dont_adjust: true,
        link_max_length: 0.,
        origin: p.origin,
    };
    let pattern = if roof {
        p.interface_pattern
    } else {
        p.base_pattern
    };
    // The filler here never gets the object's bounding box, so `Fill::_infill_direction`
    // anchors the line grid at the centre of each ExPolygon's contour bounding box
    // (`BoundingBox::center()`, truncating, in object coordinates).
    let o = p.origin;
    let mut out = vec![];
    for expoly in union64(area) {
        let Some(bb) = crate::geometry::BoundingBox::from_points(&expoly.contour) else {
            continue;
        };
        params.origin = Point::new(
            o.x + (bb.min.x - o.x + bb.max.x - o.x) / 2,
            o.y + (bb.min.y - o.y + bb.max.y - o.y) / 2,
        );
        out.extend(
            fill::fill(pattern, std::slice::from_ref(&expoly), &params)
                .into_iter()
                .flat_map(|f| f.polylines),
        );
    }
    out
}
fn sample_next(line: &[Point], start: Point, idx: usize, distance: f64) -> Option<(Point, usize)> {
    let dist2 = distance * distance;
    let eps = scaled(0.01) as f64;
    for i in idx + 1..line.len() {
        let p1 = line[i];
        if p1.distance_sq(start) < dist2 as i128 {
            continue;
        }
        let p0 = line[i - 1];
        let vx = (p1.x - p0.x) as f64;
        let vy = (p1.y - p0.y) as f64;
        let l2 = vx * vx + vy * vy;
        if l2 < eps * eps {
            let mid = Point::new((p0.x + p1.x) / 2, (p0.y + p1.y) / 2);
            if (mid.distance_to(start) - distance).abs() < eps {
                return Some((mid, i - 1));
            } else {
                continue;
            }
        }
        let px = (start.x - p0.x) as f64;
        let py = (start.y - p0.y) as f64;
        let t = (px * vx + py * vy) / l2;
        let fx = vx * t;
        let fy = vy * t;
        let remaining = dist2 - (px - fx).powi(2) - (py - fy).powi(2);
        if remaining > -(EPS as f64) {
            let remaining = remaining.max(0.);
            if (vx - fx).powi(2) + (vy - fy).powi(2) >= remaining {
                let t = (remaining / l2).sqrt();
                return Some((
                    Point::new(p0.x + (fx + vx * t) as i64, p0.y + (fy + vy * t) as i64),
                    i - 1,
                ));
            }
        }
    }
    None
}
fn at_length(line: &[Point], mut distance: f64) -> Point {
    for pair in line.windows(2) {
        let len = pair[0].distance_to(pair[1]);
        if len >= distance && len > 0. {
            let t = distance / len;
            return Point::new(
                pair[0].x + ((pair[1].x - pair[0].x) as f64 * t) as i64,
                pair[0].y + ((pair[1].y - pair[0].y) as f64 * t) as i64,
            );
        }
        distance -= len;
    }
    *line.last().unwrap()
}
pub fn ensure_maximum_distance_polyline(
    input: &[Polyline],
    distance: f64,
    min_points: usize,
) -> Vec<Polyline> {
    let mut result = vec![];
    for part in input.iter().filter(|p| !p.is_empty()) {
        let mut part = part.clone();
        let len = polyline_length(&part);
        let mut line = vec![];
        let mut current_distance = distance.max(scaled(0.1) as f64);
        if len < 2. * distance && min_points <= 1 {
            line.push(at_length(&part, len / 2.));
        } else {
            let mut end = part.len() - 1;
            if part.first() == part.last() && part.len() > 1 {
                let mut start = 0;
                let mut max_dist2 = 0i64;
                for i in 0..part.len() - 1 {
                    for j in 0..part.len() - 1 {
                        let dist = part[i].distance_sq(part[j]) as f64;
                        if dist > max_dist2 as f64 {
                            start = i;
                            end = j;
                            max_dist2 = dist as i64;
                        }
                    }
                }
                let n = part.len() - 1;
                part[..n].rotate_left(start);
                part[n] = part[0];
                end = (part.len() + end - start - 1) % n;
            }
            while line.len() < min_points && current_distance >= scaled(0.1) as f64 {
                line.clear();
                let mut current = part[0];
                line.push(current);
                if min_points > 1 || current.distance_to(part[end]) > current_distance {
                    line.push(part[end]);
                }
                let mut index = 0;
                let mut next_distance = current_distance;
                while let Some((next, segment)) = sample_next(&part, current, index, next_distance)
                {
                    let min_dist = line
                        .iter()
                        .map(|p| p.distance_to(next))
                        .fold(f64::MAX, f64::min);
                    if min_dist >= current_distance {
                        line.push(next);
                        current = next;
                        index = segment;
                        next_distance = current_distance;
                    } else {
                        if current == next {
                            if next_distance > 2. * current_distance {
                                break;
                            }
                            next_distance += current_distance;
                            continue;
                        }
                        next_distance = (current_distance - min_dist).max(scaled(0.1) as f64);
                        current = next;
                        index = segment;
                    }
                }
                current_distance *= 0.9;
            }
        }
        result.push(line);
    }
    result
}
pub fn get_avoidance_status(
    v: &mut TreeModelVolumes,
    c: &TreeSupportSettings,
    point: Point,
    radius: i64,
    layer: usize,
) -> LineStatus {
    let min = c.xy_distance > c.xy_min_distance;
    if !contains(
        &v.get_avoidance(radius, layer, AvoidanceType::FastSafe, false, min),
        point,
    ) {
        LineStatus::ToBpSafe
    } else if !contains(
        &v.get_avoidance(radius, layer, AvoidanceType::Fast, false, min),
        point,
    ) {
        LineStatus::ToBp
    } else if c.support_rests_on_model
        && !contains(
            &v.get_avoidance(radius, layer, AvoidanceType::FastSafe, true, min),
            point,
        )
    {
        LineStatus::ToModelGraciousSafe
    } else if c.support_rests_on_model
        && !contains(
            &v.get_avoidance(radius, layer, AvoidanceType::Fast, true, min),
            point,
        )
    {
        LineStatus::ToModelGracious
    } else if c.support_rests_on_model && !contains(&v.get_collision(radius, layer, min), point) {
        LineStatus::ToModel
    } else {
        LineStatus::Invalid
    }
}
pub fn convert_lines_to_internal(
    v: &mut TreeModelVolumes,
    c: &TreeSupportSettings,
    lines: &[Polyline],
    layer: usize,
) -> LineInformations {
    let mut out = vec![];
    for line in lines {
        let mut result = vec![];
        for &point in line {
            let status = get_avoidance_status(v, c, point, c.get_radius(0, 0.), layer);
            if status != LineStatus::Invalid {
                result.push((point, status));
            } else if !result.is_empty() {
                out.push(std::mem::take(&mut result));
            }
        }
        if !result.is_empty() {
            out.push(result);
        }
    }
    out
}
fn evaluate_next(
    v: &mut TreeModelVolumes,
    c: &TreeSupportSettings,
    layer: usize,
    p: &(Point, LineStatus),
) -> bool {
    let min = c.xy_distance > c.xy_min_distance;
    if !contains(
        &v.get_avoidance(
            c.get_radius(0, 0.),
            layer - 1,
            if p.1 == LineStatus::ToBpSafe {
                AvoidanceType::FastSafe
            } else {
                AvoidanceType::Fast
            },
            false,
            min,
        ),
        p.0,
    ) {
        return true;
    }
    if c.support_rests_on_model && !matches!(p.1, LineStatus::ToBp | LineStatus::ToBpSafe) {
        let forbidden = if matches!(
            p.1,
            LineStatus::ToModelGracious | LineStatus::ToModelGraciousSafe
        ) {
            v.get_avoidance(
                c.get_radius(0, 0.),
                layer - 1,
                if p.1 == LineStatus::ToModelGraciousSafe {
                    AvoidanceType::FastSafe
                } else {
                    AvoidanceType::Fast
                },
                true,
                min,
            )
        } else {
            v.get_collision(c.get_radius(0, 0.), layer - 1, min)
        };
        return !contains(&forbidden, p.0);
    }
    false
}
fn split_lines(
    lines: LineInformations,
    mut evaluate: impl FnMut(&(Point, LineStatus)) -> bool,
) -> (LineInformations, LineInformations) {
    let mut keep = vec![];
    let mut release = vec![];
    for line in lines {
        let mut current = true;
        let mut result = vec![];
        for p in line {
            if evaluate(&p) != current {
                if !result.is_empty() {
                    (if current { &mut keep } else { &mut release })
                        .push(std::mem::take(&mut result));
                }
                current = !current;
            }
            result.push(p);
        }
        if !result.is_empty() {
            (if current { &mut keep } else { &mut release }).push(result);
        }
    }
    (keep, release)
}

pub struct RichInterfacePlacer<'a> {
    pub volumes: &'a mut TreeModelVolumes,
    pub config: &'a TreeSupportSettings,
    pub support_parameters: &'a TreeSupportInfill,
    pub force_tip_to_roof: bool,
    pub move_bounds: &'a mut [SupportElements],
    pub roofs: &'a mut TreeRoofs,
    already_inserted: Vec<BTreeSet<(i64, i64)>>,
}
impl<'a> RichInterfacePlacer<'a> {
    pub fn new(
        volumes: &'a mut TreeModelVolumes,
        config: &'a TreeSupportSettings,
        support_parameters: &'a TreeSupportInfill,
        force_tip_to_roof: bool,
        move_bounds: &'a mut [SupportElements],
        roofs: &'a mut TreeRoofs,
    ) -> Self {
        let already_inserted = vec![BTreeSet::new(); move_bounds.len()];
        Self {
            volumes,
            config,
            support_parameters,
            force_tip_to_roof,
            move_bounds,
            roofs,
            already_inserted,
        }
    }
    fn add_point(
        &mut self,
        p: (Point, LineStatus),
        layer: usize,
        dont_move: usize,
        roof: bool,
        recovery: usize,
        skip: bool,
    ) {
        let bp = matches!(p.1, LineStatus::ToBp | LineStatus::ToBpSafe);
        let gracious = bp
            || matches!(
                p.1,
                LineStatus::ToModelGracious | LineStatus::ToModelGraciousSafe
            );
        let safe = matches!(p.1, LineStatus::ToBpSafe | LineStatus::ToModelGraciousSafe);
        if !self.config.support_rests_on_model && !bp {
            return;
        }
        let divisor = ((self.config.min_radius + 1) / 10).max(1);
        // Hashed in object coordinates (truncating division), as upstream.
        let o = self.support_parameters.origin;
        if !self.already_inserted[layer].insert(((p.0.x - o.x) / divisor, (p.0.y - o.y) / divisor))
        {
            return;
        }
        let mut state = SupportElementState {
            target_height: layer as i32,
            target_position: p.0,
            next_position: p.0,
            layer_idx: layer as i32,
            to_buildplate: bp,
            result_on_layer: Some(p.0),
            to_model_gracious: gracious,
            use_min_xy_dist: self.config.xy_distance > self.config.xy_min_distance,
            supports_roof: roof,
            dont_move_until: dont_move as u32,
            can_use_safe_radius: safe,
            skip_ovalisation: skip,
            ..Default::default()
        };
        state.set_pending_roof_recovery(
            if self.force_tip_to_roof {
                dont_move as u32
            } else {
                0
            },
            recovery as u32,
            self.config.settings.rules,
        );
        self.move_bounds[layer].push_back(SupportElement {
            state,
            parents: vec![],
            influence_area: circle(p.0, scaled(0.01)),
        });
    }
    pub fn add_points_along_lines(
        &mut self,
        mut lines: LineInformations,
        insert_layer: usize,
        roof_tip_layers: usize,
        supports_roof_layers: usize,
        dont_move_until: usize,
    ) {
        let mut dtt = 0;
        while dtt < roof_tip_layers && insert_layer > dtt {
            let layer = insert_layer - dtt;
            let depth = dtt + supports_roof_layers;
            let (keep, release) = split_lines(lines, |p| {
                evaluate_next(self.volumes, self.config, layer, p)
            });
            lines = keep;
            for p in release.into_iter().flatten() {
                self.add_point(p, layer, roof_tip_layers - dtt, depth > 0, depth, false);
            }
            let mut roofs = vec![];
            for (p, _) in lines.iter().flatten() {
                // Upstream scales the pre-discretized 0.01mm circle, not an
                // independently discretized full-size circle.
                let mut tip = circle(*p, scaled(0.01));
                for q in &mut tip[0].contour {
                    q.x = p.x
                        + ((q.x - p.x) as f64 * self.config.min_radius as f64 / scaled(0.01) as f64)
                            as i64;
                    q.y = p.y
                        + ((q.y - p.y) as f64 * self.config.min_radius as f64 / scaled(0.01) as f64)
                            as i64;
                }
                roofs.extend(tip);
            }
            self.roofs
                .add(self.config, self.support_parameters, roofs, layer, depth);
            dtt += 1;
        }
        let depth = dtt + supports_roof_layers;
        for line in lines {
            let skip = self.config.min_radius < 3 * self.config.support_line_width
                && roof_tip_layers == 0
                && dtt == 0
                && line.len() > 5;
            for p in line {
                self.add_point(
                    p,
                    insert_layer - dtt,
                    dont_move_until.saturating_sub(dtt),
                    depth > 0,
                    depth,
                    skip,
                );
            }
        }
    }
}

pub fn sample_overhang_area(
    mut overhang: Areas,
    large_roof: bool,
    layer: usize,
    roof_layers: usize,
    connect: i64,
    m: &TreeSupportMeshGroupSettings,
    placer: &mut RichInterfacePlacer<'_>,
) {
    let c = placer.config;
    let p = placer.support_parameters;
    let generate_roof = |a: &[crate::geometry::ExPolygon], l| {
        generate_support_infill_lines(a, c, p, true, l, m.support_roof_line_distance)
    };
    let mut lines = vec![];
    let mut dtt = 0;
    let mut generation_dtt = 0;
    if large_roof {
        let mut added = vec![];
        let mut last = overhang.clone();
        while dtt < roof_layers && layer > dtt {
            let min = c.xy_distance > c.xy_min_distance;
            let raw = if c.support_rests_on_model {
                placer
                    .volumes
                    .get_collision(c.get_radius(0, 0.), layer - dtt - 1, min)
            } else {
                placer.volumes.get_avoidance(
                    c.get_radius(0, 0.),
                    layer - dtt - 1,
                    AvoidanceType::Fast,
                    false,
                    min,
                )
            };
            let forbidden = miter(&raw, scaled(0.005) as f64);
            let next = diff64(&overhang, &forbidden);
            if area(&next) < m.minimum_roof_area {
                if dtt > 0 {
                    let before = dtt - 1;
                    let samples = ensure_maximum_distance_polyline(
                        &generate_roof(&last, layer - before),
                        connect as f64,
                        1,
                    );
                    lines = split_lines(
                        convert_lines_to_internal(placer.volumes, c, &samples, layer - before),
                        |pt| evaluate_next(placer.volumes, c, layer - before, pt),
                    )
                    .0;
                }
                break;
            }
            added.push(overhang.clone());
            last = overhang;
            overhang = next;
            dtt += 1;
        }
        generation_dtt = dtt.max(1) - 1;
        if lines.is_empty()
            && dtt != 0
            && generate_roof(&overhang, layer - generation_dtt).is_empty()
        {
            for (idx, a) in added.iter().take(dtt).enumerate() {
                if generate_roof(a, layer - idx).is_empty() {
                    dtt = idx;
                    generation_dtt = dtt.max(1) - 1;
                    break;
                }
            }
        }
        for (idx, a) in added.into_iter().take(dtt).enumerate() {
            placer.roofs.add(c, p, a, layer - idx, idx);
        }
    }
    if lines.is_empty() {
        let supports_roof = dtt > 0;
        let continuous = !supports_roof && large_roof;
        let generated = generate_support_infill_lines(
            &overhang,
            c,
            p,
            supports_roof,
            layer - generation_dtt,
            if supports_roof {
                m.support_roof_line_distance
            } else {
                m.support_tree_branch_distance
            },
        );
        let mut samples = ensure_maximum_distance_polyline(
            &generated,
            if continuous {
                (c.min_radius / 2) as f64
            } else {
                connect as f64
            },
            1,
        );
        let count = samples.iter().map(Vec::len).sum::<usize>();
        let perimeter: f64 = overhang
            .iter()
            .flat_map(|ex| ex.polygons())
            .map(|poly| polygon_length(poly))
            .sum();
        let min_points = ((perimeter / connect as f64) as usize).clamp(1, 3);
        if count <= min_points {
            let reduced = miter(&overhang, -c.support_line_width as f64 / 2.2);
            let use_reduced = !reduced.is_empty()
                && area(&miter(
                    &diff64(&overhang, &reduced),
                    c.support_line_width.max(connect) as f64,
                )) < (scaled(0.001) as f64).powi(2);
            samples = ensure_maximum_distance_polyline(
                &outlines(if use_reduced { &reduced } else { &overhang }),
                connect as f64,
                min_points,
            );
        }
        lines = convert_lines_to_internal(placer.volumes, c, &samples, layer - dtt);
    }
    if dtt >= layer && large_roof {
        placer
            .roofs
            .add(c, p, overhang, 0, dtt.min(p.num_top_interface_layers));
    } else {
        placer.add_points_along_lines(
            lines,
            layer - dtt,
            if placer.force_tip_to_roof {
                roof_layers - dtt
            } else {
                0
            },
            dtt,
            if roof_layers > 0 {
                roof_layers - dtt
            } else {
                0
            },
        );
    }
}

/// Raft-specific contact inputs are supplied from the slicing grid; no classic
/// support generator or G-code objects are constructed by this isolated port.
#[derive(Clone, Debug, Default)]
pub struct TreeRaftContact {
    pub contact_layer: usize,
    pub first_tree_layer: usize,
    pub outline: Areas,
    pub expansion: i64,
}
#[derive(Clone, Debug, PartialEq)]
pub struct TreePathing {
    pub move_bounds: Vec<SupportElements>,
    pub roofs: TreeRoofs,
}

/// `overhangs` is on the combined raft/object grid, as returned by upstream
/// generate_overhangs. This step consumes overhang regions, not triangle meshes.
pub fn generate_initial_areas(
    v: &mut TreeModelVolumes,
    c: &TreeSupportSettings,
    overhangs: &[Areas],
    p: &TreeSupportInfill,
    raft: Option<&TreeRaftContact>,
) -> TreePathing {
    generate_initial_areas_with_cancel(v, c, overhangs, p, raft, || {
        Ok::<(), std::convert::Infallible>(())
    })
    .unwrap()
}

/// Fallible cancellation is checked between upstream work units.
pub fn generate_initial_areas_with_cancel<E>(
    v: &mut TreeModelVolumes,
    c: &TreeSupportSettings,
    overhangs: &[Areas],
    p: &TreeSupportInfill,
    raft: Option<&TreeRaftContact>,
    mut cancel: impl FnMut() -> Result<(), E>,
) -> Result<TreePathing, E> {
    let z_delta = c.z_distance_top_layers + 1;
    let layers = overhangs
        .iter()
        .rposition(|a| !a.is_empty())
        .map_or(0, |top| top.saturating_sub(c.z_distance_top_layers))
        .max(c.raft_layers.len());
    let mut result = TreePathing {
        move_bounds: vec![SupportElements::new(); layers],
        roofs: TreeRoofs::new(layers),
    };
    if layers == 0 {
        return Ok(result);
    }
    if let Some(r) = raft {
        result.roofs.add(
            c,
            p,
            if r.expansion > 0 {
                miter(&r.outline, r.expansion as f64)
            } else {
                r.outline.clone()
            },
            r.contact_layer,
            0,
        );
    }
    let m = &c.settings;
    let min = c.xy_distance > c.xy_min_distance;
    let connect = (c.support_line_width as f64 * 100. / m.support_tree_top_rate
        + (2. * c.min_radius as f64 - c.support_line_width as f64).max(0.))
        as i64;
    assert!(connect > 0, "tip density must yield positive spacing");
    let circle_length = if c.min_radius < c.support_line_width {
        c.min_radius / 2
    } else {
        ((c.min_radius as f64).powi(2) - ((c.min_radius - c.support_line_width / 2) as f64).powi(2))
            .sqrt() as i64
    };
    let extra = (c.min_radius - c.support_line_width / 2).max(0)
        + if min { c.support_line_width / 2 } else { 0 };
    let roof_layers = m.support_roof_layers.max(0) as usize;
    let roof_enabled = roof_layers > 0;
    let force = roof_enabled
        && ((if c.settings.rules.is_orca() {
            p.zero_gap_interface_top
        } else {
            p.soluble_interface
        }) || (c.min_radius as f64).powi(2) * std::f64::consts::PI > m.minimum_roof_area);
    let first = c.raft_layers.len().saturating_sub(z_delta).max(1);
    // The overhang geometry of each layer depends only on the volumes (upstream computes it
    // in a parallel loop too); the tips are then sampled into the placer in layer order.
    let volumes: &TreeModelVolumes = v;
    cancel()?;
    let areas: Vec<(usize, Areas, Areas)> = (first
        ..layers.min(overhangs.len().saturating_sub(z_delta)))
        .into_par_iter()
        .filter_map(|layer| {
            let raw = &overhangs[layer + z_delta];
            if raw.is_empty() {
                return None;
            }
            let forbidden_raw = if c.support_rests_on_model {
                volumes.get_collision(c.get_radius(0, 0.), layer, min)
            } else {
                volumes.get_avoidance(c.get_radius(0, 0.), layer, AvoidanceType::Fast, false, min)
            };
            let forbidden = miter(&forbidden_raw, scaled(0.005) as f64);
            let mut regular = safe_offset_inc(
                raw,
                m.support_offset,
                &forbidden,
                (c.min_radius as f64 * 1.75 + c.xy_min_distance as f64) as i64,
                0,
                1,
            );
            let expanded = if m.support_offset == 0 {
                raw.clone()
            } else {
                miter(raw, m.support_offset as f64)
            };
            let mut remaining = intersection64(
                &diff64(
                    &expanded,
                    &miter(&regular, c.support_line_width as f64 * 0.5),
                ),
                &forbidden,
            );
            let mut acc = 0;
            while !remaining.is_empty() && acc + c.support_line_width / 8 < extra {
                let step = (if acc + 2 * c.support_line_width > c.min_radius {
                    c.support_line_width / 8
                } else {
                    circle_length
                })
                .min(extra - acc);
                assert!(step > 0);
                acc += step;
                let collision = volumes.get_collision(0, layer, true);
                let offset_step = c.xy_min_distance + c.support_line_width;
                remaining = diff64(
                    &remaining,
                    &safe_offset_inc(
                        &regular,
                        (1.5 * acc as f64) as i64,
                        &collision,
                        offset_step,
                        0,
                        1,
                    ),
                );
                regular.extend(diff64(
                    &safe_offset_inc(&remaining, acc, &collision, offset_step, 0, 1),
                    &forbidden,
                ));
                regular = union64(&regular);
            }
            let mut roof_parts = Vec::new();
            if roof_enabled {
                let mut roofs = safe_offset_inc(
                    raw,
                    0,
                    &forbidden,
                    c.min_radius * 2 + c.xy_min_distance,
                    0,
                    1,
                );
                // Preserve upstream's minimum_support_area gate for roof filtering.
                if m.minimum_support_area > 0 {
                    remove_small(&mut roofs, m.minimum_roof_area);
                }
                regular = clip::diff_ex_safety(&regular, &roofs);
                roof_parts = union64(&roofs);
            }
            if m.minimum_support_area > 0 {
                remove_small(&mut regular, m.minimum_support_area as f64);
            }
            Some((layer, roof_parts, union64(&regular)))
        })
        .collect();
    let mut placer =
        RichInterfacePlacer::new(v, c, p, force, &mut result.move_bounds, &mut result.roofs);
    for (layer, roof_parts, regular_parts) in areas {
        cancel()?;
        for part in roof_parts {
            cancel()?;
            sample_overhang_area(
                vec![part],
                true,
                layer,
                roof_layers,
                connect,
                m,
                &mut placer,
            );
        }
        for part in regular_parts {
            cancel()?;
            sample_overhang_area(
                vec![part],
                false,
                layer,
                roof_layers,
                connect,
                m,
                &mut placer,
            );
        }
    }
    if let Some(r) = raft {
        for layer in 0..r.first_tree_layer.min(layers) {
            result.roofs.top_contacts[layer].clear();
            result.move_bounds[layer].clear();
        }
        if r.expansion > 0 && r.first_tree_layer < layers {
            let contact = &result.roofs.top_contacts[r.contact_layer];
            result.move_bounds[r.first_tree_layer]
                .retain(|e| !contains(contact, e.state.result_on_layer.unwrap()));
        }
    }
    Ok(result)
}
/// Complete step-2 output, deliberately separate from support generation used
/// by slicing. The same API serves Prusa and Orca rules.
pub fn create_tree_pathing(
    v: &mut TreeModelVolumes,
    c: &TreeSupportSettings,
    overhangs: &[Areas],
    p: &TreeSupportInfill,
    raft: Option<&TreeRaftContact>,
) -> TreePathing {
    create_tree_pathing_with_cancel(v, c, overhangs, p, raft, || {
        Ok::<(), std::convert::Infallible>(())
    })
    .unwrap()
}

/// Fallible cancellation is checked between upstream work units.
pub fn create_tree_pathing_with_cancel<E>(
    v: &mut TreeModelVolumes,
    c: &TreeSupportSettings,
    overhangs: &[Areas],
    p: &TreeSupportInfill,
    raft: Option<&TreeRaftContact>,
    mut cancel: impl FnMut() -> Result<(), E>,
) -> Result<TreePathing, E> {
    let mut result = generate_initial_areas_with_cancel(v, c, overhangs, p, raft, &mut cancel)?;
    create_layer_pathing_with_cancel(v, c, &mut result.move_bounds, &mut cancel)?;
    create_nodes_from_area_with_cancel(v, c, &mut result.move_bounds, &mut cancel)?;
    Ok(result)
}

// Upstream remove_small operates on individual contours and holes, before
// rebuilding ExPolygons. Keep the same hole filtering rather than net-area culling.
fn remove_small(areas: &mut Areas, minimum: f64) {
    let mut polygons = clip::to_polygons(areas);
    polygons.retain(|p| crate::geometry::signed_area(p).abs() >= minimum);
    *areas = clip::union_ex_positive(&polygons);
}
