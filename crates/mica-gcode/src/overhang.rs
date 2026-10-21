// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/ExtrusionProcessor.hpp :: estimate_points_properties
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/ExtrusionProcessor.cpp :: calculate_and_split_overhanging_extrusions, calculate_overhang_speed
// Source: prusaslicer@b028299c77 src/libslic3r/SupportSpotsGenerator.cpp :: estimate_malformations, estimate_curled_up_height
// Source: prusaslicer@b028299c77 src/libslic3r/PrintObject.cpp :: PrintObject::calculate_overhanging_perimeters
// Changes: Parallel path annotation with an ordered curl-history fold; one-layer curl history, compact point-range attributes, independent line index; preserve scarf samples when inserting points.
// Changes: Match upstream curvature indexing/f32 windows; restore clipped uniform loops before estimating curl and support.
// Source: orca@8500fcdcca src/libslic3r/GCode/ExtrusionProcessor.hpp :: ExtrusionQualityEstimator::prepare_for_new_layer
// Source: orca@8500fcdcca src/libslic3r/Support/SupportSpotsGenerator.cpp :: estimate_malformations
// Changes: Orca rules keep the previous layer's outline and curled external walls for orca_overhang.
// Copyright (c) Prusa Research 2022 - 2023 Pavel Mikuš @Godrak, Vojtěch Bubník @bubnikv

//! Dynamic support/curl annotation. The state survives export batch boundaries, not the whole print.
use crate::boundary::{Line, Lines, XY, cross, distance, lerp, point, sub, xy};
use mica_core::{
    config::Config,
    geometry::ExPolygon,
    seam::ScarfSample,
    toolpath::{ExtrusionPath, LayerToolpaths, OverhangSection, Role},
};

#[derive(Clone, Copy)]
struct Sample {
    position: XY,
    distance: f64,
    source: usize,
    t: f64,
}

// Upstream annotates closed loops before choosing their seam and clipping the seam gap.
// A uniform, non-scarfed loop still carries its seam metadata here. Restore its last
// segment for the geometric estimate, then retain the clipped endpoint when emitting it.
fn closed_loop(path: &ExtrusionPath) -> Option<ExtrusionPath> {
    if path.points.len() < 4 || path.seam.as_ref().is_none_or(|s| !s.samples.is_empty()) {
        return None;
    }
    let first = xy(path.points[0]);
    let last = xy(*path.points.last()?);
    let before = xy(path.points[path.points.len() - 2]);
    let gap = distance(first, last);
    if gap > path.flow.width || cross(sub(first, last), sub(last, before)).abs() > 1e-6 {
        return None;
    }
    let mut closed = path.clone();
    *closed.points.last_mut()? = path.points[0];
    Some(closed)
}

fn properties(
    path: &ExtrusionPath,
    lines: &Lines,
    signed: bool,
    offset: f64,
    max_length: f64,
) -> Vec<Sample> {
    let dist = |p| {
        if signed {
            lines.signed_distance(p) + offset
        } else {
            lines.nearest(p).map_or(f64::INFINITY, |(_, d)| d) + offset
        }
    };
    let mut result = Vec::new();
    for (i, edge) in path.points.windows(2).enumerate() {
        let a = xy(edge[0]);
        let b = xy(edge[1]);
        let length = distance(a, b);
        if length < 1e-9 {
            continue;
        }
        let da = dist(a);
        let db = dist(b);
        let mut ts = vec![0., 1.];
        if (da > offset + 1e-6) != (db > offset + 1e-6) {
            ts.extend(lines.intersections(Line { a, b }));
        }
        ts.sort_by(f64::total_cmp);
        ts.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
        let initial = ts.clone();
        for ends in initial.windows(2) {
            let p = lerp(a, b, ends[0]);
            let q = lerp(a, b, ends[1]);
            let len = distance(p, q);
            let d0 = dist(p);
            let d1 = dist(q);
            if len > 4.
                && ((d0 > -offset && d0 < offset + 2.) || (d1 > -offset && d1 < offset + 2.))
            {
                for t in [(d0 + 3. * offset) / len, 1. - (d1 + 3. * offset) / len] {
                    ts.push(ends[0] + t.clamp(0., 1.) * (ends[1] - ends[0]));
                }
            }
        }
        ts.sort_by(f64::total_cmp);
        ts.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
        if max_length > 0. {
            let initial = ts.clone();
            for ends in initial.windows(2) {
                let step = max_length / length;
                let mut t = ends[0] + step;
                while t < ends[1] - 1e-9 {
                    ts.push(t);
                    t += step;
                }
            }
            ts.sort_by(f64::total_cmp);
        }
        for t in ts.into_iter().filter(|t| *t < 1. - 1e-9) {
            let position = lerp(a, b, t);
            result.push(Sample {
                position,
                distance: dist(position),
                source: i,
                t,
            });
        }
    }
    if let Some(&p) = path.points.last() {
        let position = xy(p);
        result.push(Sample {
            position,
            distance: dist(position),
            source: path.points.len().saturating_sub(2),
            t: 1.,
        });
    }
    result.dedup_by(|a, b| point(a.position) == point(b.position));
    result
}

fn curvature(samples: &[Sample], _width: f64) -> Vec<f64> {
    let n = samples.len();
    let mut result = vec![0.; n];
    if n < 3 {
        return result;
    }
    let looped = samples[0].position == samples[n - 1].position;
    let prev = |i: usize| {
        if i > 0 {
            i - 1
        } else if looped {
            n - 1
        } else {
            i
        }
    };
    let next = |i: usize| {
        if i + 1 < n {
            i + 1
        } else if looped {
            0
        } else {
            i
        }
    };
    let lengths: Vec<f32> = (0..n)
        .map(|i| distance(samples[prev(i)].position, samples[i].position) as f32)
        .collect();
    if lengths.iter().sum::<f32>() <= 1e-4 {
        return result;
    }
    let walk = |index: usize, backwards: bool, half: f32| {
        let mut i = index;
        let mut walked = 0_f32;
        let mut position = samples[index].position;
        while walked < half {
            let j = if backwards { prev(i) } else { next(i) };
            if j == i {
                break;
            }
            // Preserve the upstream cumulative-distance indexing: its window is based on
            // the preceding edge even when interpolating along the following edge.
            let length = lengths[if backwards { j } else { i }];
            if walked + length > half {
                let a = samples[i].position;
                let b = samples[j].position;
                let actual = distance(a, b);
                if actual > 0. {
                    position = lerp(a, b, f64::from(half - walked) / actual);
                }
                break;
            }
            walked += length;
            i = j;
        }
        position
    };
    for (i, sample) in samples.iter().enumerate() {
        for window in [3_f32, 9., 16.] {
            let a = sub(sample.position, walk(i, true, window * 0.5));
            let b = sub(walk(i, false, window * 0.5), sample.position);
            let k = (cross(a, b).atan2(a[0] * b[0] + a[1] * b[1]) / f64::from(window)) as f32;
            if k.abs() > (result[i] as f32).abs() {
                result[i] = f64::from(k);
            }
        }
    }
    result
}

/// Upstream's geometric curl estimate, including propagation and decay from the preceding layer.
pub fn estimate_curled_height(
    distance: f64,
    curvature: f64,
    height: f64,
    width: f64,
    previous: f64,
) -> f64 {
    let mut curled = if distance.abs() < 3. * width {
        (previous - 0.75 * height).max(0.)
    } else {
        0.
    };
    if distance > 0.2 * width && distance < 1.1 * width {
        curled += ((height + distance) * 0.5 - height).max(0.) * 0.5;
        if curvature > 0.01 {
            curled += (distance * distance - width * width / (curvature * 100.))
                .max(0.)
                .sqrt();
        }
        curled = curled.min(10. * height);
    }
    curled
}

/// Compact history of external extrusions. Support-only output layers do not advance it.
#[derive(Default)]
pub struct OverhangProcessor {
    previous: Lines,
    heights: Vec<f64>,
    boundary: Option<Lines>,
    curled: Vec<(Line, f64)>,
}
impl OverhangProcessor {
    /// Orca rules (`ExtrusionQualityEstimator`): `speeds(i, path)` is path `i`'s overhang speed
    /// table, `None` where it prints at one speed. The lower layer's outline and curled external
    /// walls slow the paths down; see [`crate::orca_overhang`].
    pub(crate) fn process_orca(
        &mut self,
        layer: &mut LayerToolpaths,
        outlines: Option<&[ExPolygon]>,
        speeds: impl Fn(usize, &ExtrusionPath) -> Option<crate::orca_overhang::Speeds> + Sync,
    ) {
        let _timer = mica_core::timing::Span::new(mica_core::timing::Stage::ExportOverhang);
        // Support-only output layers do not advance the history.
        if !layer.paths.iter().any(|p| {
            matches!(
                p.role,
                Role::ExternalPerimeter | Role::OverhangPerimeter | Role::Perimeter
            )
        }) {
            return;
        }
        let Some(outlines) = outlines.filter(|o| !o.is_empty()) else {
            self.boundary = None;
            self.previous = Lines::default();
            self.heights.clear();
            self.curled.clear();
            return;
        };
        let tables: Vec<_> = mica_core::parallel::map(&layer.paths, |i, path| {
            path.overhang.is_none().then(|| speeds(i, path)).flatten()
        });
        // This layer's curled walls, for the layer above (`estimate_curled_extrusions`).
        let curl = tables.iter().flatten().any(|t| t.curled);
        let estimate = curl.then(|| self.curl(layer, true));
        if layer.index > 0
            && let Some(boundary) = &self.boundary
            && tables.iter().any(Option::is_some)
        {
            let lines = Lines::new(self.curled.iter().map(|(l, _)| *l).collect());
            let heights: Vec<f64> = self.curled.iter().map(|(_, h)| *h).collect();
            let lower = crate::orca_overhang::Lower {
                outline: boundary,
                curled: (!heights.is_empty()).then_some((&lines, heights.as_slice())),
            };
            let height = layer.height;
            let mut items: Vec<_> = layer.paths.iter_mut().zip(&tables).collect();
            mica_core::parallel::for_each_mut(&mut items, |(path, table)| {
                if let Some(table) = table {
                    crate::orca_overhang::annotate(path, table, &lower, height);
                }
            });
        }
        match estimate {
            Some((current, heights, curled)) => {
                self.previous = Lines::new(current);
                self.heights = heights;
                self.curled = curled;
            }
            None => {
                self.previous = Lines::default();
                self.heights.clear();
                self.curled.clear();
            }
        }
        self.boundary = Some(Lines::outlines(outlines));
    }

    pub fn process(
        &mut self,
        layer: &mut LayerToolpaths,
        outlines: Option<&[ExPolygon]>,
        config: &Config,
    ) {
        let _timer = mica_core::timing::Span::new(mica_core::timing::Stage::ExportOverhang);
        let speed = config
            .bool("enable_dynamic_overhang_speeds")
            .unwrap_or(false);
        let fan = config.bool("enable_dynamic_fan_speeds").unwrap_or(false);
        let curl = speed;
        self.curled.clear();
        if !(speed || fan) {
            return;
        }
        if !layer.paths.iter().any(|p| {
            matches!(
                p.role,
                Role::ExternalPerimeter | Role::OverhangPerimeter | Role::Perimeter
            )
        }) {
            return;
        }
        let Some(outlines) = outlines.filter(|o| !o.is_empty()) else {
            self.boundary = None;
            self.previous = Lines::default();
            self.heights.clear();
            return;
        };
        let (mut current, mut heights) = (Vec::new(), Vec::new());
        if curl {
            let (lines, values, curled) = self.curl(layer, false);
            self.curled = curled;
            current = lines;
            heights = values;
        }
        if (speed || fan)
            && layer.index > 0
            && let Some(boundary) = &self.boundary
        {
            let curled = Lines::new(self.curled.iter().map(|(l, _)| *l).collect());
            mica_core::parallel::for_each_mut(&mut layer.paths, |path| {
                // LayerSource implementations may supply already annotated paths.
                if path.overhang.is_some() {
                    return;
                }
                if !matches!(
                    path.role,
                    Role::ExternalPerimeter
                        | Role::Perimeter
                        | Role::OverhangPerimeter
                        | Role::GapFill
                ) {
                    return;
                }
                let closed = closed_loop(path);
                let mut samples = properties(
                    closed.as_ref().unwrap_or(path),
                    boundary,
                    true,
                    path.flow.width * 0.5,
                    0.,
                );
                if samples.len() < 2 {
                    return;
                }
                let values: Vec<_> = samples
                    .iter()
                    .enumerate()
                    .map(|(i, sample)| {
                        let next = &samples[(i + 1).min(samples.len() - 1)];
                        (
                            sample.distance.max(next.distance),
                            proximity(
                                Line {
                                    a: sample.position,
                                    b: next.position,
                                },
                                path.flow.width,
                                layer.height,
                                &curled,
                                &self.curled,
                            ),
                        )
                    })
                    .collect();
                let mut sections = Vec::new();
                let mut begin = 0;
                for i in 1..samples.len() {
                    if i + 1 == samples.len()
                        || (values[begin].0 - values[i].0).abs() >= 0.001 * path.flow.width
                        || (values[begin].1 - values[i].1).abs() >= 0.001
                    {
                        sections.push(OverhangSection {
                            end: i as u32,
                            start_distance: values[begin].0 as f32,
                            end_distance: samples[i].distance as f32,
                            curled_proximity: values[begin].1 as f32,
                            speed: 0.,
                        });
                        begin = i;
                    }
                }
                if closed.is_some() {
                    let endpoint = xy(*path.points.last().unwrap());
                    let mut gap = distance(samples.last().unwrap().position, endpoint);
                    while samples.len() > 1 && gap > 0. {
                        let n = samples.len();
                        let length = distance(samples[n - 2].position, samples[n - 1].position);
                        if length <= gap {
                            gap -= length;
                            samples.pop();
                        } else {
                            samples[n - 1].position = endpoint;
                            break;
                        }
                    }
                    let end = (samples.len() - 1) as u32;
                    if let Some(last) = sections.iter().position(|s| s.end >= end) {
                        sections.truncate(last + 1);
                        sections[last].end = end;
                    }
                }
                // Interpolate scarf Z, retaining each original segment's extrusion multiplier.
                if let Some(seam) = path.seam.as_mut().filter(|s| !s.samples.is_empty()) {
                    if let Some([ramp_end, return_start]) = seam.scarf_ranges {
                        let remap = |old: usize| {
                            samples
                                .iter()
                                .position(|p| p.source as f64 + p.t >= old as f64 - 1e-9)
                                .unwrap_or(samples.len() - 1)
                        };
                        seam.scarf_ranges = Some([remap(ramp_end), remap(return_start)]);
                    }
                    let old = &seam.samples;
                    seam.samples = samples
                        .iter()
                        .map(|p| {
                            let a = old[p.source];
                            let b = old[p.source + 1];
                            ScarfSample {
                                height: a.height + (b.height - a.height) * p.t,
                                extrusion: if p.t < 1e-9 { a.extrusion } else { b.extrusion },
                            }
                        })
                        .collect();
                }
                path.points = samples.iter().map(|p| point(p.position)).collect();
                path.overhang = Some(sections.into_boxed_slice());
            });
        }
        self.previous = Lines::new(current);
        self.heights = heights;
        self.boundary = Some(Lines::outlines(outlines));
    }

    /// The curl estimate of the layer's external walls (`estimate_malformations`) against the
    /// previous layer's: every sampled line with its curled height, and the lines curled by more
    /// than 0.1 mm. Orca estimates external perimeters only, PrusaSlicer overhang walls too.
    fn curl(&self, layer: &LayerToolpaths, orca: bool) -> (Vec<Line>, Vec<f64>, Vec<(Line, f64)>) {
        let contributions = mica_core::parallel::map(&layer.paths, |_, path| {
            let mut current = Vec::new();
            let mut heights = Vec::new();
            let external = match path.role {
                Role::ExternalPerimeter => true,
                Role::OverhangPerimeter => !orca,
                _ => false,
            };
            if !external {
                return (current, heights);
            }
            let mut closed = closed_loop(path);
            if let Some(original) = &mut closed
                && path.seam.as_ref().is_some_and(|s| s.original_clockwise)
                    != (mica_core::geometry::signed_area(&original.points) < 0.)
            {
                original.points.reverse();
            }
            let samples = properties(
                closed.as_ref().unwrap_or(path),
                &self.previous,
                false,
                0.,
                16.,
            );
            let curvatures = curvature(&samples, path.flow.width);
            for i in 0..samples.len() {
                let previous = i.saturating_sub(1);
                let line = Line {
                    a: samples[previous].position,
                    b: samples[i].position,
                };
                let middle = lerp(line.a, line.b, 0.5);
                let height = if let Some((idx, d)) = self.previous.nearest(middle) {
                    let sign = if self
                        .boundary
                        .as_ref()
                        .is_some_and(|b| b.signed_distance(middle) + path.flow.width * 0.5 < 0.)
                    {
                        -1.
                    } else {
                        1.
                    };
                    estimate_curled_height(
                        d * sign,
                        (curvatures[previous] + curvatures[i]) * 0.5,
                        layer.height,
                        path.flow.width,
                        self.heights[idx],
                    )
                } else {
                    0.
                };
                current.push(line);
                heights.push(height);
            }
            (current, heights)
        });
        let mut current = Vec::new();
        let mut heights = Vec::new();
        let mut curled = Vec::new();
        for (lines, values) in contributions {
            curled.extend(
                lines
                    .iter()
                    .copied()
                    .zip(values.iter().copied())
                    .filter(|(_, height)| *height > 0.1),
            );
            current.extend(lines);
            heights.extend(values);
        }
        (current, heights, curled)
    }
}

fn proximity(segment: Line, width: f64, height: f64, lines: &Lines, curled: &[(Line, f64)]) -> f64 {
    let limit = 10. * width;
    let mid = lerp(segment.a, segment.b, 0.5);
    let indices = lines.in_radius(mid, limit);
    let len = distance(segment.a, segment.b);
    if len > 8. {
        let dir = [
            (segment.b[0] - segment.a[0]) / len,
            (segment.b[1] - segment.a[1]) / len,
        ];
        let right = [-dir[1], dir[0]];
        let projected: f64 = indices
            .iter()
            .map(|&i| {
                // Clip into the segment's rectangular influence region before measuring projection.
                let transform = |p| {
                    let p = sub(p, segment.a);
                    [
                        p[0] * dir[0] + p[1] * dir[1],
                        p[0] * right[0] + p[1] * right[1],
                    ]
                };
                let a = transform(lines.lines[i].a);
                let b = transform(lines.lines[i].b);
                let mut lo = 0_f64;
                let mut hi = 1_f64;
                for (k, min, max) in [(0, 0., len), (1, -limit, limit)] {
                    let d = b[k] - a[k];
                    if d.abs() < 1e-12 {
                        if a[k] < min || a[k] > max {
                            return 0.;
                        }
                    } else {
                        let x = (min - a[k]) / d;
                        let y = (max - a[k]) / d;
                        lo = lo.max(x.min(y));
                        hi = hi.min(x.max(y));
                    }
                }
                (hi - lo).max(0.) * (b[0] - a[0]).abs()
            })
            .sum();
        if projected < 0.4 * len {
            return 0.;
        }
    }
    indices
        .iter()
        .map(|&i| {
            (1. - lines.lines[i].distance(mid) / limit).powi(2) * curled[i].1 / (10. * height)
        })
        .fold(0., f64::max)
}

/// Speed and fan interpolation at 0/25/50/75/100 percent support. A zero speed means the reference speed.
pub fn calculate_overhang_speed(
    config: &Config,
    width: f64,
    attributes: OverhangSection,
    reference: f64,
    default: f64,
    current_fan: Option<f64>,
) -> (Option<f64>, Option<f64>) {
    let base = if reference > 0. { reference } else { default };
    let interpolate = |values: [f64; 5], distance: f64| {
        let t = (4. * distance / width).clamp(0., 4.);
        let i = (t.floor() as usize).min(3);
        values[i] + (values[i + 1] - values[i]) * (t - i as f64)
    };
    let speed = config
        .bool("enable_dynamic_overhang_speeds")
        .unwrap_or(false)
        .then(|| {
            let mut values = [if default < 1e-6 { base } else { default }; 5];
            for i in 0..4 {
                let v = config
                    .abs_value(&format!("overhang_speed_{i}"), base)
                    .unwrap_or(base);
                values[4 - i] = if v < 1e-6 { base } else { v };
            }
            interpolate(values, attributes.start_distance as f64)
                .min(interpolate(values, attributes.end_distance as f64))
                .min(interpolate(
                    values,
                    width * attributes.curled_proximity as f64,
                ))
        });
    let fan = config
        .bool("enable_dynamic_fan_speeds")
        .unwrap_or(false)
        .then(|| {
            let mut values = [0.; 5];
            for i in 0..4 {
                values[4 - i] = config
                    .float(&format!("overhang_fan_speed_{i}"))
                    .unwrap_or(0.);
            }
            let fan = interpolate(values, attributes.start_distance as f64)
                .min(interpolate(values, attributes.end_distance as f64));
            if let Some(current) = current_fan
                && current > fan
                && current - fan <= 3.
            {
                current
            } else {
                fan
            }
        });
    (speed, fan)
}

#[cfg(test)]
mod tests;
