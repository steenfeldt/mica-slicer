// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/MultiMaterialSegmentation.cpp :: segmentation_by_painting, cut_segmented_layers, segmentation_top_and_bottom_layers, merge_segmented_layers, filter_color_of_small_segments
// Changes: Spatial boundary projection, upstream Voronoi partition, width/depth cuts and top/bottom propagation per volume.
// Changes: Deterministic parallel layer/cap processing and shared colour-independent cap offsets.
// Copyright (c) Prusa Research 2021 - 2023 Vojtěch Bubník @bubnikv, Lukáš Hejl @hejllukas
// PrusaSlicer is released under the terms of the AGPLv3 or higher.

use super::{FacetStates, projection, voronoi};
use crate::config::Config;
use crate::geometry::clip::{
    Join, closing_ex, diff_ex, intersection_ex, offset_ex, opening_ex, simplify_ex, union_ex,
};
use crate::geometry::{ExPolygon, Point, SCALE};
use crate::mesh::TriangleMesh;
use rayon::prelude::*;
use std::collections::{BTreeMap, HashMap};

pub type LayerColors = Vec<Vec<ExPolygon>>;
#[derive(Clone, Copy)]
struct ColorLine {
    a: Point,
    b: Point,
    color: u8,
}

pub fn segment(
    mesh: &TriangleMesh,
    states: &FacetStates,
    zs: &[f64],
    outlines: &[Vec<ExPolygon>],
    config: &Config,
) -> Result<Vec<LayerColors>, String> {
    segment_with_virtuals(mesh, states, zs, outlines, config, &Default::default())
}

pub fn segment_with_virtuals(
    mesh: &TriangleMesh,
    states: &FacetStates,
    zs: &[f64],
    outlines: &[Vec<ExPolygon>],
    config: &Config,
    spectrum: &crate::full_spectrum::FullSpectrum,
) -> Result<Vec<LayerColors>, String> {
    let physical = config.vector_len("nozzle_diameter").max(1);
    let count = physical.max(spectrum.max_id()) + 1;
    let mut lines = vec![Vec::new(); zs.len()];
    let mut top = vec![vec![vec![]; count]; zs.len()];
    let mut bottom = top.clone();
    let mut invalid = None;
    let projection_timer = crate::timing::Span::new(crate::timing::Stage::PaintingProjection);
    states.visit(mesh, true, |color, t| {
        if color as usize > physical && !spectrum.contains(color as usize) {
            invalid = Some(color);
            return;
        }
        let lo = t.iter().map(|p| p[2]).fold(f64::INFINITY, f64::min);
        let hi = t.iter().map(|p| p[2]).fold(f64::NEG_INFINITY, f64::max);
        let first = zs.partition_point(|z| *z < lo);
        let last = zs.partition_point(|z| *z <= hi);
        for i in first..last {
            if let Some([a, b]) = projection::section(t, zs[i]) {
                lines[i].push(ColorLine { a, b, color });
            }
        }
        let normal = projection::normal_z(t);
        if normal.abs() < 1e-12 {
            return;
        }
        let start = first.saturating_sub(1);
        let end = last.min(zs.len().saturating_sub(1));
        for i in start..=end {
            if i >= zs.len() {
                break;
            }
            let (low, high) = if normal > 0. {
                (zs[i], zs.get(i + 1).copied().unwrap_or(f64::INFINITY))
            } else {
                (if i == 0 { f64::NEG_INFINITY } else { zs[i - 1] }, zs[i])
            };
            if let Some(p) = projection::slab(t, low, high) {
                if normal > 0. {
                    top[i][color as usize].push(p);
                } else {
                    bottom[i][color as usize].push(p);
                }
            }
        }
    });
    if let Some(color) = invalid {
        return Err(format!(
            "painted tool {color} exceeds printer's {} tools",
            physical
        ));
    }
    let width = config.float("mmu_segmented_region_max_width").unwrap_or(0.);
    drop(projection_timer);
    let _timing = crate::timing::Span::new(crate::timing::Stage::Segmentation);
    let depth = config
        .float("mmu_segmented_region_interlocking_depth")
        .unwrap_or(0.);
    if !width.is_finite() || width < 0. || !depth.is_finite() || depth < 0. {
        return Err("painting width/depth must be finite and nonnegative".into());
    }
    let beam = config.bool("interlocking_beam").unwrap_or(false);
    let mut result: Vec<LayerColors> = outlines
        .par_iter()
        .enumerate()
        .map(|(i, outlines)| {
            let index = LineIndex::new(&lines[i]);
            let mut layer = vec![vec![]; count];
            let cleaned = simplify_ex(&closing_ex(outlines, 0.001 * SCALE), 0.0005 * SCALE);
            for outline in &cleaned {
                let rings: Vec<_> = outline.polygons().map(|p| index.project(p)).collect();
                let first = rings.first().and_then(|p| p.first()).map_or(0, |p| p.1);
                let mut colors = if rings.iter().flatten().all(|p| p.1 == first) {
                    let mut c = vec![vec![]; count];
                    c[first as usize].push(outline.clone());
                    c
                } else {
                    voronoi::partition(&rings, count)?
                };
                // Preserve a disjoint, complete partition even with nanometre roundoff at graph vertices.
                let mut assigned = vec![];
                for area in colors.iter_mut().skip(1) {
                    *area = diff_ex(
                        &intersection_ex(area, std::slice::from_ref(outline)),
                        &assigned,
                    );
                    assigned.extend(area.clone());
                }
                colors[0] = diff_ex(std::slice::from_ref(outline), &assigned);
                if !beam {
                    let cut = if i % 2 == 0 && depth > 0. && width > depth {
                        width - depth
                    } else {
                        width
                    };
                    if cut > 0. {
                        let dominant = dominant_color(&colors);
                        let interior =
                            offset_ex(std::slice::from_ref(outline), -cut * SCALE, Join::Miter);
                        for area in &mut colors {
                            *area = diff_ex(area, &interior);
                        }
                        colors[dominant].extend(interior);
                    }
                }
                for (dst, src) in layer.iter_mut().zip(colors) {
                    dst.extend(src);
                }
            }
            for area in &mut layer {
                *area = union_ex(area);
            }
            Ok(layer)
        })
        .collect::<Result<_, String>>()?;
    drop(lines);
    let _caps_timer = crate::timing::Span::new(crate::timing::Stage::Caps);
    let top_count = config.int("top_solid_layers").unwrap_or(3).max(0) as usize;
    let bottom_count = config.int("bottom_solid_layers").unwrap_or(3).max(0) as usize;
    // Use the resolved perimeter flow when available, including percent/automatic widths.
    let settings =
        crate::settings::PrintSettings::from_config(config).map_err(|e| e.to_string())?;
    let flow = crate::print::LayerFlows::new(&settings, 1, settings.layer_height).perimeter;
    let threshold = if settings.gap_fill_enabled && settings.gap_fill_speed > 0. {
        flow.width * 0.25
    } else {
        (flow.width + 0.7 * flow.spacing()) * 0.5
    };
    top.par_iter_mut()
        .zip(bottom.par_iter_mut())
        .enumerate()
        .for_each(|(i, (top, bottom))| {
            for c in 0..count {
                top[c] = if top_count == 0 {
                    vec![]
                } else {
                    opening_ex(
                        &diff_ex(
                            &union_ex(&top[c]),
                            outlines.get(i + 1).map_or(&[], Vec::as_slice),
                        ),
                        threshold * SCALE,
                    )
                };
                bottom[c] = if bottom_count == 0 {
                    vec![]
                } else {
                    opening_ex(
                        &diff_ex(
                            &union_ex(&bottom[c]),
                            i.checked_sub(1).map_or(&[], |j| outlines[j].as_slice()),
                        ),
                        threshold * SCALE,
                    )
                };
            }
        });
    // Each source layer is independent. Gather in source-layer order so Clipper
    // receives exactly the same polygon order as the serial propagation pass.
    let contributions: Vec<_> = (0..zs.len())
        .into_par_iter()
        .map(|i| {
            let mut contributions = Vec::new();
            for c in 0..count {
                let mut seed = top[i][c].clone();
                seed.extend_from_slice(&bottom[i][c]);
                contributions.push((i, c, seed));
            }
            for (direction, n, seeds, opposite) in [
                (-1, top_count, &top[i], &bottom),
                (1, bottom_count.saturating_sub(1), &bottom[i], &top),
            ] {
                let mut active: Vec<_> = seeds.iter().map(|s| !s.is_empty()).collect();
                if n == 0 || !active.iter().any(|v| *v) {
                    continue;
                }
                let mut trim = outlines[i].clone();
                for step in 1..=n {
                    if !active.iter().any(|v| *v) {
                        break;
                    }
                    let j = i as isize + direction * step as isize;
                    if j < 0 || j >= zs.len() as isize {
                        break;
                    }
                    let j = j as usize;
                    // These two operations depend on layer and depth, not colour.
                    trim = intersection_ex(&trim, &outlines[j]);
                    let inset = offset_ex(&trim, -(step as f64) * flow.width * SCALE, Join::Miter);
                    let opposite: Vec<_> = opposite[j].iter().flatten().cloned().collect();
                    for c in 0..count {
                        if !active[c] {
                            continue;
                        }
                        let area = opening_ex(
                            &diff_ex(&intersection_ex(&seeds[c], &inset), &opposite),
                            threshold * SCALE,
                        );
                        if area.is_empty() {
                            active[c] = false;
                        } else {
                            contributions.push((j, c, area));
                        }
                    }
                }
            }
            contributions
        })
        .collect();
    let mut caps = vec![vec![vec![]; count]; zs.len()];
    for source in contributions {
        for (i, c, area) in source {
            caps[i][c].extend(area);
        }
    }
    result
        .par_iter_mut()
        .zip(caps.par_iter_mut())
        .enumerate()
        .for_each(|(i, (result, caps))| {
            let mut occupied = vec![];
            for cap in caps.iter_mut() {
                *cap = diff_ex(&intersection_ex(&union_ex(cap), &outlines[i]), &occupied);
                occupied.extend(cap.clone());
            }
            for c in 0..count {
                let side = diff_ex(&result[c], &occupied);
                let cap = std::mem::take(&mut caps[c]);
                let remove_dimples = !side.is_empty() && !cap.is_empty();
                let merged = union_ex(&[side, cap].concat());
                result[c] = if remove_dimples {
                    closing_ex(&merged, 0.0001 * SCALE)
                } else {
                    merged
                };
            }
        });
    let (min, max) = mesh.bounds();
    let origin = Point::from_mm(
        (f64::from(min[0]) + f64::from(max[0])) * 0.5,
        (f64::from(min[1]) + f64::from(max[1])) * 0.5,
    );
    super::interlocking::apply(&mut result, config, origin)?;
    Ok(result)
}

fn dominant_color(colors: &LayerColors) -> usize {
    let area = |p: &Vec<ExPolygon>| p.iter().map(ExPolygon::area_mm2).sum::<f64>();
    let mut best = 0;
    for i in 1..colors.len() {
        if area(&colors[i]) > area(&colors[best]) + 1e-12 {
            best = i;
        }
    }
    best
}

const GRID: f64 = 2. * SCALE;
struct LineIndex<'a> {
    lines: &'a [ColorLine],
    cells: HashMap<(i64, i64), Vec<usize>>,
}
impl<'a> LineIndex<'a> {
    fn new(lines: &'a [ColorLine]) -> Self {
        let mut out = Self {
            lines,
            cells: HashMap::new(),
        };
        for (i, l) in lines.iter().enumerate() {
            for x in bin(l.a.x.min(l.b.x) - 400_000)..=bin(l.a.x.max(l.b.x) + 400_000) {
                for y in bin(l.a.y.min(l.b.y) - 400_000)..=bin(l.a.y.max(l.b.y) + 400_000) {
                    out.cells.entry((x, y)).or_default().push(i);
                }
            }
        }
        out
    }
    fn project(&self, ring: &[Point]) -> Vec<(Point, u8)> {
        let mut out: Vec<(Point, u8)> = vec![];
        for i in 0..ring.len() {
            let a = ring[i];
            let b = ring[(i + 1) % ring.len()];
            if a == b {
                continue;
            }
            let mut ids = vec![];
            for x in bin(a.x.min(b.x))..=bin(a.x.max(b.x)) {
                for y in bin(a.y.min(b.y))..=bin(a.y.max(b.y)) {
                    if let Some(indices) = self.cells.get(&(x, y)) {
                        ids.extend_from_slice(indices);
                    }
                }
            }
            ids.sort_unstable();
            ids.dedup();
            let mut cuts = vec![0., 1.];
            let mut ranges = vec![];
            for id in ids {
                let l = self.lines[id];
                let (ta, da) = project(l.a, a, b);
                let (tb, db) = project(l.b, a, b);
                // Ignore transverse neighbouring faces: colour travels along its own boundary.
                if da.min(db) > (0.4 * SCALE).powi(2) || (ta - tb).abs() < 1e-10 {
                    continue;
                }
                let length = a.distance_to(b);
                let snap = |t: f64| {
                    if t * length < 0.01 * SCALE {
                        0.
                    } else if (1. - t) * length < 0.01 * SCALE {
                        1.
                    } else {
                        t
                    }
                };
                let low = snap(ta.min(tb).clamp(0., 1.));
                let high = snap(ta.max(tb).clamp(0., 1.));
                if high - low < 1e-10 {
                    continue;
                }
                cuts.extend([low, high]);
                ranges.push((low, high, l));
            }
            cuts.sort_by(f64::total_cmp);
            cuts.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
            for pair in cuts.windows(2) {
                let mid = (pair[0] + pair[1]) * 0.5;
                let p = lerp(a, b, mid);
                let mut distance = f64::INFINITY;
                let mut color = 0;
                for &(low, high, l) in &ranges {
                    if mid >= low && mid <= high {
                        let (t, _) = project(p, l.a, l.b);
                        let q = lerp(l.a, l.b, t.clamp(0., 1.));
                        let d = p.distance_to(q);
                        if d < distance {
                            distance = d;
                            color = l.color;
                        }
                    }
                }
                if distance > 0.4 * SCALE {
                    color = 0;
                }
                let p = lerp(a, b, pair[0]);
                if out.last().is_none_or(|v| v.0 != p) {
                    out.push((p, color));
                }
            }
        }
        filter_small_color_runs(&mut out, 0.2 * SCALE);
        // Linear stack simplification retains colour transitions and avoids quadratic
        // vertex removal on finely tessellated painted models.
        let mut reduced: Vec<(Point, u8)> = Vec::with_capacity(out.len());
        for point in out {
            while reduced.len() >= 2 {
                let n = reduced.len();
                if reduced[n - 2].1 != reduced[n - 1].1
                    || project(reduced[n - 1].0, reduced[n - 2].0, point.0).1
                        > (0.001 * SCALE).powi(2)
                {
                    break;
                }
                reduced.pop();
            }
            reduced.push(point);
        }
        while reduced.len() > 3 {
            let n = reduced.len();
            if reduced[n - 2].1 == reduced[n - 1].1
                && project(reduced[n - 1].0, reduced[n - 2].0, reduced[0].0).1
                    <= (0.001 * SCALE).powi(2)
            {
                reduced.pop();
            } else {
                break;
            }
        }
        reduced
    }
}
fn bin(v: i64) -> i64 {
    (v as f64 / GRID).floor() as i64
}
fn lerp(a: Point, b: Point, t: f64) -> Point {
    Point::new(
        a.x + ((b.x - a.x) as f64 * t).round() as i64,
        a.y + ((b.y - a.y) as f64 * t).round() as i64,
    )
}
fn project(p: Point, a: Point, b: Point) -> (f64, f64) {
    let dx = (b.x - a.x) as f64;
    let dy = (b.y - a.y) as f64;
    let l = dx * dx + dy * dy;
    if l == 0. {
        return (0., p.distance_to(a).powi(2));
    }
    let t = ((p.x - a.x) as f64 * dx + (p.y - a.y) as f64 * dy) / l;
    let q = lerp(a, b, t);
    (t, p.distance_to(q).powi(2))
}

/// Sparse colour list for integration with the indexed region table.
pub fn nonempty(colors: LayerColors) -> BTreeMap<u8, Vec<ExPolygon>> {
    colors
        .into_iter()
        .enumerate()
        .filter(|(_, p)| !p.is_empty())
        .map(|(c, p)| (c as u8, p))
        .collect()
}

// Remove short interruptions between matching colours, including across the ring seam.
// This is the upstream 0.2 mm colour filter, independent of geometric simplification.
fn filter_small_color_runs(points: &mut [(Point, u8)], limit: f64) {
    if points.len() < 3 {
        return;
    }
    let n = points.len();
    let mut runs: Vec<(usize, usize, u8, f64)> = vec![];
    for i in 0..n {
        let color = points[i].1;
        let len = points[i].0.distance_to(points[(i + 1) % n].0);
        if let Some(last) = runs.last_mut().filter(|r| r.2 == color) {
            last.1 = (i + 1) % n;
            last.3 += len;
        } else {
            runs.push((i, (i + 1) % n, color, len));
        }
    }
    if runs.len() > 2 && runs[0].2 == runs.last().unwrap().2 {
        let last = runs.pop().unwrap();
        runs[0].0 = last.0;
        runs[0].3 += last.3;
    }
    let mut from = 0;
    while from < runs.len() {
        let mut to = (from + 1) % runs.len();
        let mut length = 0.;
        let mut update = false;
        while to != from {
            if length > limit {
                break;
            }
            if runs[to].2 == runs[from].2 {
                update = true;
                break;
            }
            length += runs[to].3;
            to = (to + 1) % runs.len();
        }
        if !update {
            from += 1;
            continue;
        }
        let color = runs[from].2;
        let mut current = (from + 1) % runs.len();
        while current != to {
            let mut i = runs[current].0;
            while i != runs[current].1 {
                points[i].1 = color;
                i = (i + 1) % n;
            }
            points[runs[current].1].1 = color;
            runs[current].2 = color;
            current = (current + 1) % runs.len();
        }
        if from < to {
            from = to;
        } else {
            break;
        }
    }
}
