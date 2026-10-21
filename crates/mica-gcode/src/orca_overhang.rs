// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: orca@8500fcdcca src/libslic3r/GCode/ExtrusionProcessor.hpp :: estimate_points_properties, ExtrusionQualityEstimator::estimate_extrusion_quality
// Source: orca@8500fcdcca src/libslic3r/GCode.cpp :: GCode::_extrude
// Changes: Runs once per layer before planning, on the paths as the writer receives them (the seam
// Changes: gap already clipped); the speed table, the slowdown point list and the per-segment speed
// Changes: hysteresis of `_extrude`'s variable-speed branch are resolved into one overhang section per
// Changes: printed segment. The previous layer's outlines and curled external walls come from the
// Changes: Prusa-ported curl history (`overhang.rs`), as Orca's `estimate_malformations` is the same
// Changes: SupportSpotsGenerator code. Point positions stay in f64 millimetres.
// Copyright (c) Prusa Research 2022 - 2023 Pavel Mikuš @Godrak (ExtrusionProcessor, as adapted by OrcaSlicer)

//! Orca's overhang slowdown (`enable_overhang_speed`, `overhang_1_4_speed` … `overhang_4_4_speed`,
//! `slowdown_for_curled_perimeters`): every wall and bridge path is split where its distance to the
//! previous layer's outline changes the speed, and printed with straight moves at the
//! interpolated speeds when any of them differs from the path's speed.

use crate::boundary::{Line, Lines, XY, distance, lerp, point, sub, xy};
use mica_core::{
    config::Config,
    seam::ScarfSample,
    settings::PrintSettings,
    toolpath::{ExtrusionPath, OverhangSection, Role},
};

/// `EPSILON` of libslic3r.h.
const EPSILON: f64 = 1e-4;

/// The speed table of one path (`ConfigOptionPercents overhang_overlap_levels` against
/// `dynamic_overhang_speeds`): ascending distances past the lower layer's edge, in mm, and their
/// speeds in mm/s; `original` is the path's speed (`speed` in `_extrude`).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Speeds {
    pub sections: Vec<(f64, f64)>,
    pub original: f64,
    pub curled: bool,
}

/// An Orca option kept as text, the tool's entry of a per-extruder vector.
fn entry<'a>(c: &'a Config, key: &str, tool: usize) -> Option<&'a str> {
    let text = c.extra(key)?;
    let mut values = text.split(',');
    let first = values.clone().next();
    values.nth(tool).or(first).map(str::trim)
}

fn flag(c: &Config, key: &str, tool: usize, default: bool) -> bool {
    entry(c, key, tool)
        .map(|s| matches!(s, "1" | "true"))
        .or_else(|| c.bool(key))
        .unwrap_or(default)
}

/// `get_abs_value(key, base)` of a FloatOrPercent Orca speed (default 0).
fn abs_value(c: &Config, key: &str, tool: usize, base: f64) -> f64 {
    match entry(c, key, tool) {
        Some(text) => match text.strip_suffix('%') {
            Some(percent) => percent
                .trim()
                .parse::<f64>()
                .map_or(0., |p| base * p / 100.),
            None => text.parse().unwrap_or(0.),
        },
        None => c.abs_value(key, base).unwrap_or(0.),
    }
}

/// `_extrude`'s overhang speed table for `path`, or `None` where Orca prints it at one speed:
/// overhang speeds off, the first layer, or a role other than a wall or a bridge.
pub(crate) fn speeds(
    c: &Config,
    s: &PrintSettings,
    path: &ExtrusionPath,
    tool: usize,
    first_layer: bool,
) -> Option<Speeds> {
    if first_layer
        || !matches!(
            path.role,
            Role::Perimeter
                | Role::ExternalPerimeter
                | Role::OverhangPerimeter
                | Role::BridgeInfill
                | Role::InternalBridgeInfill
        )
        || !flag(c, "enable_overhang_speed", tool, true)
    {
        return None;
    }
    let original = crate::extrusion_speed::speed(c, s, path, false);
    let external = path.role == Role::ExternalPerimeter;
    let base = if external {
        s.external_perimeter_speed
    } else {
        s.perimeter_speed
    };
    // `ref_speed`: the wall speed, capped by the volumetric limit (0: the volumetric limit).
    let mm3 = path.mm3_per_mm();
    let reference = if base == 0. && s.filament_max_volumetric_speed > 0. && mm3 > 0. {
        s.filament_max_volumetric_speed / mm3
    } else {
        crate::extrusion_speed::cap(c, s, path, base)
    };
    if reference <= 0. {
        return None;
    }
    let level = |key: &str| {
        let v = abs_value(c, key, tool, reference);
        if v < 0.5 { reference } else { v }
    };
    let curled = flag(c, "slowdown_for_curled_perimeters", tool, false);
    let quarter = level("overhang_4_4_speed");
    let last = if curled { quarter } else { s.bridge_speed };
    let speeds = [
        reference,
        level("overhang_1_4_speed"),
        level("overhang_2_4_speed"),
        level("overhang_3_4_speed"),
        quarter,
        last,
    ];
    let width = path.flow.width as f32;
    let mut sections: Vec<(f64, f64)> = [90_f32, 75., 50., 25., 13., 0.]
        .iter()
        .zip(speeds)
        .map(|(overlap, speed)| (f64::from(width * (1. - overlap / 100.)), speed))
        .collect();
    // The table sorts by distance, equal distances by decreasing speed, and an equal distance
    // takes the first (fastest) speed.
    sections.sort_by(|a, b| a.0.total_cmp(&b.0).then(b.1.total_cmp(&a.1)));
    for k in 1..sections.len() {
        if sections[k].0 == sections[k - 1].0 {
            sections[k].1 = sections[k - 1].1;
        }
    }
    Some(Speeds {
        sections,
        original,
        curled,
    })
}

#[derive(Clone, Copy, Debug)]
struct Sample {
    position: XY,
    /// Signed distance to the lower layer's outline plus half the width.
    distance: f64,
    /// Input edge the sample lies on and its parameter there.
    source: usize,
    t: f64,
}

/// `estimate_points_properties<true, true, true, true>` without a maximum line length: the
/// path's points, the crossings of the lower layer's outline, and the points where the distance
/// reaches the slowdown band, each with its distance (`min_distance`: the smallest distance with a
/// lower speed, or -1 to segment every line longer than 4 mm near the edge).
fn properties(points: &[XY], lower: &Lines, width: f64, min_distance: f64) -> Vec<Sample> {
    let min_spacing = width * 0.25;
    let offset = f64::from(0.5 * width as f32);
    let signed = |p: XY| lower.signed_distance(p);
    let mut samples = Vec::with_capacity(points.len() + points.len() / 2);
    let Some(&first) = points.first() else {
        return samples;
    };
    samples.push(Sample {
        position: first,
        distance: signed(first) + offset,
        source: 0,
        t: 0.,
    });
    for (i, &p) in points.iter().enumerate().skip(1) {
        let next = Sample {
            position: p,
            distance: signed(p) + offset,
            source: i - 1,
            t: 1.,
        };
        let prev = *samples.last().unwrap();
        if (prev.distance > offset + EPSILON) != (next.distance > offset + EPSILON) {
            let line = Line {
                a: prev.position,
                b: next.position,
            };
            for t in lower.intersections(line) {
                let position = lerp(line.a, line.b, t);
                if distance(position, prev.position) > min_spacing
                    && distance(next.position, position) > min_spacing
                {
                    samples.push(Sample {
                        position,
                        distance: offset,
                        source: i - 1,
                        t,
                    });
                }
            }
        }
        samples.push(next);
    }
    // Segmentation near the edge.
    let mut out = Vec::with_capacity(samples.len() * 2);
    out.push(samples[0]);
    for pair in samples.windows(2) {
        let (curr, next) = (pair[0], pair[1]);
        let near = |d: f64| d > -offset && d < offset + 2.;
        if near(curr.distance) || near(next.distance) {
            let length = distance(curr.position, next.position);
            let wanted = if min_distance > 0. {
                (curr.distance.abs() > min_distance || next.distance.abs() > min_distance)
                    && length >= 2.
            } else {
                length > 4.
            };
            if wanted {
                let a0 = ((curr.distance + 3. * offset) / length).clamp(0., 1.);
                let a1 = (1. - (next.distance + 3. * offset) / length).clamp(0., 1.);
                let (t0, t1) = (a0.min(a1), a0.max(a1));
                // The parameter of `curr` on `next`'s input edge.
                let tc = if curr.source == next.source {
                    curr.t
                } else {
                    0.
                };
                for (t, keep) in [(t0, t0 < 1.), (t1, t1 > 0.)] {
                    if !keep {
                        continue;
                    }
                    let position = lerp(curr.position, next.position, t);
                    let raw = signed(position);
                    if (raw.abs() > min_distance || min_distance <= 0.)
                        && distance(position, curr.position) > min_spacing
                        && distance(next.position, position) > min_spacing
                    {
                        out.push(Sample {
                            position,
                            distance: f64::from((raw + offset) as f32),
                            source: next.source,
                            t: tc + t * (next.t - tc),
                        });
                    }
                }
            }
        }
        out.push(next);
    }
    out
}

/// The slowdown over curled external walls of the layer below (`artificial_distance_to_curled_lines`).
fn curled_distance(a: XY, b: XY, width: f64, height: f64, curled: &Lines, heights: &[f64]) -> f64 {
    let limit = 10. * width;
    let middle = lerp(a, b, 0.5);
    let mut indices = curled.in_radius(middle, limit);
    if indices.is_empty() {
        return 0.;
    }
    let len = distance(a, b);
    if len > 2. {
        let dir = [(b[0] - a[0]) / len, (b[1] - a[1]) / len];
        let right = [-dir[1], dir[0]];
        // Each curled line clipped to the box of influence, projected on the direction.
        let projected: f64 = indices
            .iter()
            .map(|&i| {
                let local = |p: XY| {
                    let p = sub(p, a);
                    [
                        p[0] * dir[0] + p[1] * dir[1],
                        p[0] * right[0] + p[1] * right[1],
                    ]
                };
                let (p, q) = (local(curled.lines[i].a), local(curled.lines[i].b));
                let (mut lo, mut hi) = (0_f64, 1_f64);
                for (k, min, max) in [(0, 0., len), (1, -limit, limit)] {
                    let d = q[k] - p[k];
                    if d.abs() < 1e-12 {
                        if p[k] < min || p[k] > max {
                            return 0.;
                        }
                    } else {
                        let (x, y) = ((min - p[k]) / d, (max - p[k]) / d);
                        lo = lo.max(x.min(y));
                        hi = hi.min(x.max(y));
                    }
                }
                (hi - lo).max(0.) * (q[0] - p[0]).abs()
            })
            .sum();
        if projected < 0.4 * len {
            indices.clear();
        }
    }
    indices
        .iter()
        .map(|&i| {
            let d = curled.lines[i].distance(middle);
            width * (1. - d / limit) * (1. - d / limit) * heights[i] / (height * 10.)
        })
        .fold(0., f64::max)
}

/// `calculate_speed`: the table's speed at `distance`, rounded to whole mm/s.
fn speed_at(speeds: &Speeds, distance: f64) -> f64 {
    let table = &speeds.sections;
    let speed = if distance <= table[0].0 {
        speeds.original
    } else if distance >= table[table.len() - 1].0 {
        table[table.len() - 1].1
    } else {
        let mut k = 0;
        while distance > table[k + 1].0 {
            k += 1;
        }
        let t = ((distance - table[k].0) / (table[k + 1].0 - table[k].0)).clamp(0., 1.);
        (1. - t) * table[k].1 + t * table[k + 1].1
    };
    // `round` of a float.
    f64::from((speed as f32).round())
}

/// The previous layer, as `ExtrusionQualityEstimator::prepare_for_new_layer` keeps it.
pub(crate) struct Lower<'a> {
    pub outline: &'a Lines,
    /// Curled external walls of the layer below and their curled heights.
    pub curled: Option<(&'a Lines, &'a [f64])>,
}

/// Splits `path` into one overhang section per printed segment with its speed when Orca prints it
/// with variable speeds; otherwise leaves it unchanged.
pub(crate) fn annotate(path: &mut ExtrusionPath, speeds: &Speeds, lower: &Lower<'_>, height: f64) {
    if path.points.len() < 2 || speeds.sections.is_empty() {
        return;
    }
    let width = path.flow.width;
    // The smallest overhang distance where the speed drops (-1: none).
    let min_distance = speeds
        .sections
        .iter()
        .filter(|(_, speed)| *speed <= speeds.original)
        .map(|(d, _)| *d)
        .reduce(f64::min)
        .unwrap_or(-1.);
    let points: Vec<XY> = path.points.iter().map(|&p| xy(p)).collect();
    let samples = properties(&points, lower.outline, width, min_distance);
    let n = samples.len();
    if n < 2 {
        return;
    }
    // Per point: speed, and the distance the overhang fan check sees (`overlap` as a distance).
    let mut speed = Vec::with_capacity(n);
    let mut fan = Vec::with_capacity(n);
    for i in 0..n {
        let (curr, next) = (samples[i], samples[(i + 1).min(n - 1)]);
        let artificial = match lower.curled {
            Some((lines, heights)) if speeds.curled => {
                curled_distance(curr.position, next.position, width, height, lines, heights)
            }
            _ => 0.,
        };
        let mut v = speed_at(speeds, curr.distance)
            .min(speed_at(speeds, next.distance))
            .min(speeds.original);
        if speeds.curled {
            v = v.min(speed_at(speeds, artificial));
        }
        speed.push(v);
        fan.push(curr.distance.max(next.distance) + artificial);
    }
    // `variable_speed`: any point more than 1 mm/s off the path's speed.
    if !speed.iter().any(|v| (v - speeds.original).abs() > 1.) {
        return;
    }
    // `_extrude`'s variable-speed loop: a segment takes its start point's speed, changed only by
    // more than 1 mm/s, or reset to the path's speed within 1 mm/s of it. Zero-length segments
    // (at G-code resolution) are skipped without touching that state.
    let quantum = |p: XY| [(p[0] * 1000.).round(), (p[1] * 1000.).round()];
    let mut kept = vec![samples[0]];
    let mut sections: Vec<OverhangSection> = Vec::with_capacity(n);
    let mut last = speed[0];
    for i in 1..n {
        if quantum(samples[i].position) == quantum(kept.last().unwrap().position) {
            continue;
        }
        let new = speed[i - 1];
        if (last - new).abs() > 1. {
            last = new;
        } else if (speeds.original - new).abs() <= 1. {
            last = speeds.original;
        }
        kept.push(samples[i]);
        sections.push(OverhangSection {
            end: (kept.len() - 1) as u32,
            start_distance: fan[i - 1] as f32,
            end_distance: fan[i] as f32,
            curled_proximity: 0.,
            speed: last as f32,
        });
    }
    if sections.is_empty() {
        return;
    }
    // Interpolate scarf Z, retaining each original segment's extrusion multiplier.
    if let Some(seam) = path.seam.as_mut().filter(|s| !s.samples.is_empty()) {
        if let Some([ramp_end, return_start]) = seam.scarf_ranges {
            let remap = |old: usize| {
                kept.iter()
                    .position(|p| p.source as f64 + p.t >= old as f64 - 1e-9)
                    .unwrap_or(kept.len() - 1)
            };
            seam.scarf_ranges = Some([remap(ramp_end), remap(return_start)]);
        }
        let old = &seam.samples;
        seam.samples = kept
            .iter()
            .map(|p| {
                let a = old[p.source];
                let b = old[(p.source + 1).min(old.len() - 1)];
                ScarfSample {
                    height: a.height + (b.height - a.height) * p.t,
                    extrusion: if p.t < 1e-9 { a.extrusion } else { b.extrusion },
                }
            })
            .collect();
    }
    path.points = kept.iter().map(|p| point(p.position)).collect();
    path.overhang = Some(sections.into_boxed_slice());
}

#[cfg(test)]
mod tests {
    use super::*;
    use mica_core::{flow::Flow, geometry::ExPolygon, geometry::Point};

    fn square(size: f64) -> ExPolygon {
        ExPolygon {
            contour: vec![
                Point::from_mm(0., 0.),
                Point::from_mm(size, 0.),
                Point::from_mm(size, size),
                Point::from_mm(0., size),
            ],
            holes: vec![],
        }
    }
    fn table() -> Speeds {
        // U1 0.20 Standard: 200 mm/s outer wall, overhangs 0/50/30/10, bridges 50.
        let w = 0.42_f32;
        let d = |o: f32| f64::from(w * (1. - o / 100.));
        Speeds {
            sections: vec![
                (d(90.), 200.),
                (d(75.), 200.),
                (d(50.), 50.),
                (d(25.), 30.),
                (d(13.), 10.),
                (d(0.), 50.),
            ],
            original: 200.,
            curled: false,
        }
    }
    fn wall(points: &[(f64, f64)]) -> ExtrusionPath {
        ExtrusionPath {
            role: Role::ExternalPerimeter,
            points: points.iter().map(|&(x, y)| Point::from_mm(x, y)).collect(),
            flow: Flow::new(0.42, 0.2),
            perimeter_index: Some(0),
            seam: None,
            overhang: None,
            brick: None,
        }
    }

    #[test]
    fn a_supported_wall_keeps_its_points_and_speed() {
        let outline = Lines::outlines(&[square(20.)]);
        let lower = Lower {
            outline: &outline,
            curled: None,
        };
        let mut path = wall(&[(1., 1.), (19., 1.), (19., 19.)]);
        let before = path.clone();
        annotate(&mut path, &table(), &lower, 0.2);
        assert_eq!(path, before);
    }

    #[test]
    fn a_wall_leaving_the_lower_layer_slows_down_step_by_step() {
        let outline = Lines::outlines(&[square(10.)]);
        let lower = Lower {
            outline: &outline,
            curled: None,
        };
        // Along the edge, then 2 mm out over air and back.
        let mut path = wall(&[(1., 9.8), (20., 9.8), (20., 12.), (1., 12.)]);
        annotate(&mut path, &table(), &lower, 0.2);
        let sections = path.overhang.as_ref().expect("variable speed");
        assert_eq!(sections.len() + 1, path.points.len());
        let speeds: Vec<f32> = sections.iter().map(|s| s.speed).collect();
        // Fully supported start at the wall speed, the air part at the bridge speed.
        assert_eq!(speeds[0], 200.);
        assert!(speeds.contains(&50.));
        assert!(speeds.iter().all(|&v| v <= 200.));
        // The point list is ordered along the path.
        let xs: Vec<i64> = path.points.iter().take(4).map(|p| p.x).collect();
        assert!(xs.windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn speed_table_interpolates_and_rounds() {
        let t = table();
        assert_eq!(speed_at(&t, 0.), 200.);
        assert_eq!(speed_at(&t, 1.), 50.);
        let mid = f64::from(0.42_f32 * 0.375);
        assert_eq!(speed_at(&t, mid), 125.);
    }

    #[test]
    fn orca_options_build_the_table() {
        let mut c = Config::defaults();
        mica_core::rules::Rules::OrcaSlicer.set(&mut c);
        c.set_raw("external_perimeter_speed", "200").unwrap();
        c.set_raw("bridge_speed", "50").unwrap();
        c.set_raw("small_perimeter_speed", "0").unwrap();
        for (k, v) in [
            ("overhang_1_4_speed", "0"),
            ("overhang_2_4_speed", "50"),
            ("overhang_3_4_speed", "15%"),
            ("overhang_4_4_speed", "10"),
        ] {
            c.set_raw(k, v).unwrap();
        }
        let mut s = PrintSettings::from_config(&c).unwrap();
        s.filament_max_volumetric_speed = 0.;
        let path = wall(&[(0., 0.), (50., 0.)]);
        let t = speeds(&c, &s, &path, 0, false).unwrap();
        let v: Vec<f64> = t.sections.iter().map(|s| s.1).collect();
        assert_eq!(v, [200., 200., 50., 30., 10., 50.]);
        assert!(speeds(&c, &s, &path, 0, true).is_none());
        c.set_raw("enable_overhang_speed", "0").unwrap();
        assert!(speeds(&c, &s, &path, 0, false).is_none());
    }
}
