// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/Feature/Interlocking/InterlockingGenerator.cpp :: generate_interlocking_structure, addBoundaryCells, generateMicrostructure, handleThinAreas, applyMicrostructureToOutlines
// Source: prusaslicer@b028299c77 src/libslic3r/Feature/Interlocking/VoxelUtils.cpp :: DilationKernel, walkLine, walkDilatedPolygons, walkDilatedAreas
// Source: prusaslicer@b028299c77 src/libslic3r/Fill/FillRectilinear.cpp :: sample_grid_pattern
// Changes: Deterministic sparse voxel sets and vertical scanline area sampling; one material pair at a time, object-centred grid, bounds-checked settings.
// Copyright (c) 2022 Ultimaker B.V.
// Copyright (c) 2023 UltiMaker
// CuraEngine is released under the terms of the AGPLv3 or higher.

use super::segmentation::LayerColors;
use crate::geometry::clip::{
    Join, closing_ex, diff_ex, intersection_ex, offset_ex, opening_ex, union_ex,
};
use crate::{
    config::Config,
    geometry::{ExPolygon, Point, SCALE},
};
use std::collections::BTreeSet;

type Cell = [i64; 3];
type Cells = BTreeSet<Cell>;
struct Grid {
    width: i64,
    height: i64,
}

pub(super) fn apply(
    layers: &mut [LayerColors],
    config: &Config,
    origin: Point,
) -> Result<(), String> {
    if !config.bool("interlocking_beam").unwrap_or(false) || layers.is_empty() {
        return Ok(());
    }
    let nozzle = (0..config.vector_len("nozzle_diameter").max(1))
        .filter_map(|i| config.float_at("nozzle_diameter", i))
        .fold(f64::INFINITY, f64::min);
    let w = config
        .float("interlocking_beam_width")
        .unwrap_or(0.8)
        .max(nozzle);
    let n = config.int("interlocking_beam_layer_count").unwrap_or(2);
    let depth = config.int("interlocking_depth").unwrap_or(2);
    let avoidance = config.int("interlocking_boundary_avoidance").unwrap_or(2);
    let angle = config
        .float("interlocking_orientation")
        .unwrap_or(22.5)
        .to_radians();
    if !w.is_finite() || w <= 0. || n <= 0 || depth <= 0 || avoidance < 0 || !angle.is_finite() {
        return Err(
            "invalid interlocking beam width, layer count, depth, avoidance or orientation".into(),
        );
    }
    // Bound cubic dilation work before accepting untrusted project settings.
    if depth > 64 || avoidance > 64 || n > layers.len().max(1) as i64 * 2 {
        return Err("interlocking voxel kernel exceeds supported bounds".into());
    }
    let grid = Grid {
        width: (2. * w * SCALE).round() as i64,
        height: 2 * n,
    };
    let min_line = (0..config.vector_len("nozzle_diameter").max(1))
        .map(|tool| {
            let c = crate::print_job::tool_config(config, tool);
            let s = crate::settings::PrintSettings::from_config(&c).map_err(|e| e.to_string())?;
            Ok(crate::print::LayerFlows::new(&s, 1, 0.1).external.width * SCALE)
        })
        .collect::<Result<Vec<_>, String>>()?;
    let default = config.int("perimeter_extruder").unwrap_or(1).max(1) as usize;
    for layer in layers.iter_mut() {
        if default < layer.len() {
            let zero = std::mem::take(&mut layer[0]);
            layer[default].extend(zero);
            layer[default] = union_ex(&layer[default]);
        }
    }
    for a in 1..layers[0].len() {
        for b in a + 1..layers[0].len() {
            if !layers.iter().any(|l| !l[a].is_empty()) || !layers.iter().any(|l| !l[b].is_empty())
            {
                continue;
            }
            let pair: Vec<_> = [a, b]
                .iter()
                .map(|&color| {
                    layers
                        .iter()
                        .map(|l| rotate(&l[color], angle, origin, false))
                        .collect::<Vec<_>>()
                })
                .collect();
            let cells_a = grid.boundary(&pair[0], depth);
            let cells_b = grid.boundary(&pair[1], depth);
            let mut shared: Cells = cells_a.intersection(&cells_b).copied().collect();
            if shared.is_empty() {
                continue;
            }
            let mut total: Vec<_> = pair[0]
                .iter()
                .zip(&pair[1])
                .map(|(a, b)| closing_ex(&[a.clone(), b.clone()].concat(), 100.))
                .collect();
            total.push(vec![]); // The ghost layer supplies the top skin for air filtering.
            if avoidance > 0 {
                let air = grid.boundary(&total, avoidance);
                shared.retain(|c| !air.contains(c));
                if shared.is_empty() {
                    continue;
                }
                thin_areas(
                    layers,
                    [a, b],
                    &shared,
                    &grid,
                    w,
                    avoidance,
                    angle,
                    origin,
                    min_line[a - 1].min(min_line[b - 1]),
                );
            }
            let mut structure = vec![[Vec::new(), Vec::new()]; layers.len().div_ceil(n as usize)];
            for [x, y, z] in shared {
                for i in (z * grid.height).max(0)..((z + 1) * grid.height).min(layers.len() as i64)
                {
                    if i % n != 0 {
                        continue;
                    }
                    let row = i as usize / n as usize;
                    for (color, area) in structure[row].iter_mut().enumerate() {
                        let shift = if color == 0 { 0 } else { grid.width / 2 };
                        let mut p = rectangle(
                            x * grid.width + shift,
                            y * grid.width,
                            x * grid.width + shift + grid.width / 2,
                            (y + 1) * grid.width,
                        );
                        if row % 2 == 1 {
                            for p in &mut p.contour {
                                let dx = p.x - x * grid.width;
                                let dy = p.y - y * grid.width;
                                p.x = x * grid.width + dy;
                                p.y = y * grid.width + dx;
                            }
                            p.contour.reverse();
                        }
                        area.push(p);
                    }
                }
            }
            for pair in &mut structure {
                for area in pair {
                    *area = rotate(&union_ex(area), angle, origin, true);
                }
            }
            for (i, layer) in layers.iter_mut().enumerate() {
                let bounds = rotate(&total[i], angle, origin, true);
                for (side, color) in [a, b].into_iter().enumerate() {
                    let area = intersection_ex(&structure[i / n as usize][side], &bounds);
                    layer[color] = union_ex(
                        &[
                            diff_ex(&layer[color], &structure[i / n as usize][1 - side]),
                            area,
                        ]
                        .concat(),
                    );
                }
            }
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn thin_areas(
    layers: &mut [LayerColors],
    colors: [usize; 2],
    cells: &Cells,
    grid: &Grid,
    w: f64,
    avoidance: i64,
    angle: f64,
    origin: Point,
    min_line: f64,
) {
    let detect = w * SCALE * avoidance as f64 + 5.;
    let expand = w * SCALE * (avoidance - 1) as f64 + 5.;
    let mut near = vec![vec![]; layers.len()];
    for &[x, y, z] in cells {
        for i in (z * grid.height).max(0)..((z + 1) * grid.height).min(layers.len() as i64) {
            near[i as usize].push(rectangle(
                x * grid.width,
                y * grid.width,
                (x + 1) * grid.width,
                (y + 1) * grid.width,
            ));
        }
    }
    for (i, layer) in layers.iter_mut().enumerate() {
        if near[i].is_empty() {
            continue;
        }
        let near = rotate(
            &offset_ex(&closing_ex(&near[i], 5.), detect, Join::Miter),
            angle,
            origin,
            true,
        );
        let a = &layer[colors[0]];
        let b = &layer[colors[1]];
        let shrunk = offset_ex(
            &union_ex(
                &[
                    offset_ex(a, min_line, Join::Miter),
                    offset_ex(b, min_line, Join::Miter),
                ]
                .concat(),
            ),
            -2. * min_line,
            Join::Miter,
        );
        let mut border_a = diff_ex(a, &shrunk);
        let mut border_b = diff_ex(b, &shrunk);
        for _ in 0..(detect / min_line) as usize + 2 {
            let next_a = offset_ex(&border_a, min_line, Join::Miter);
            let next_b = offset_ex(&border_b, min_line, Join::Miter);
            border_a = diff_ex(&next_a, &next_b);
            border_b = diff_ex(&next_b, &next_a);
        }
        let large_a = opening_ex(a, detect);
        let large_b = opening_ex(b, detect);
        let grow = |small: &[ExPolygon],
                    large: &[ExPolygon],
                    other: &[ExPolygon],
                    border: &[ExPolygon]| {
            offset_ex(
                &intersection_ex(
                    &intersection_ex(
                        &intersection_ex(
                            other,
                            &offset_ex(&diff_ex(small, large), expand, Join::Miter),
                        ),
                        &near,
                    ),
                    border,
                ),
                5.,
                Join::Miter,
            )
        };
        let ea = grow(a, &large_a, &large_b, &border_a);
        let eb = grow(b, &large_b, &large_a, &border_b);
        let new_a = closing_ex(
            &diff_ex(&[a.clone(), ea.clone()].concat(), &eb),
            min_line / 4.,
        );
        let new_b = closing_ex(&diff_ex(&[b.clone(), eb].concat(), &ea), min_line / 4.);
        layer[colors[0]] = new_a;
        layer[colors[1]] = new_b;
    }
}

impl Grid {
    fn boundary(&self, layers: &[Vec<ExPolygon>], size: i64) -> Cells {
        let mut out = Cells::new();
        let kernel = kernel(size);
        let shift = if size % 2 == 0 { self.width / 2 } else { 0 };
        let zshift = if size % 2 == 0 { self.height / 2 } else { 0 };
        let emit = |base: Cell, out: &mut Cells| {
            for k in &kernel {
                let p = std::array::from_fn(|i| base[i] + k[i]);
                if p[2] >= 0 {
                    out.insert(p);
                }
            }
        };
        for (i, area) in layers.iter().enumerate() {
            let z = (i as i64 + zshift).div_euclid(self.height);
            for ring in area.iter().flat_map(ExPolygon::polygons) {
                for j in 0..ring.len() {
                    let a = Point::new(ring[j].x + shift, ring[j].y + shift);
                    let b = Point::new(
                        ring[(j + 1) % ring.len()].x + shift,
                        ring[(j + 1) % ring.len()].y + shift,
                    );
                    for [x, y] in self.line(a, b) {
                        emit([x, y, z], &mut out);
                    }
                }
            }
            let skin = if i == 0 {
                area.clone()
            } else {
                union_ex(&[diff_ex(area, &layers[i - 1]), diff_ex(&layers[i - 1], area)].concat())
            };
            let skin = opening_ex(&skin, self.width as f64 / 2.);
            // Upstream samples vertical lines anchored to each polygon's minimum X,
            // then maps the sampled points (with half-cell offset) onto the voxel grid.
            let translation = shift - self.width / 2;
            let area_z = grid_coord(i as i64 + zshift - self.height / 2, self.height);
            for ex in &skin {
                let minx = ex.contour.iter().map(|p| p.x).min().unwrap_or(0);
                let maxx = ex.contour.iter().map(|p| p.x).max().unwrap_or(0);
                for x in (minx..maxx).step_by(self.width as usize) {
                    let mut crossings = vec![];
                    for ring in ex.polygons() {
                        for j in 0..ring.len() {
                            let a = ring[j];
                            let b = ring[(j + 1) % ring.len()];
                            if (a.x <= x && x < b.x) || (b.x <= x && x < a.x) {
                                crossings.push(
                                    a.y + (((x - a.x) as f64 / (b.x - a.x) as f64)
                                        * (b.y - a.y) as f64)
                                        .round() as i64
                                        + translation,
                                );
                            }
                        }
                    }
                    crossings.sort_unstable();
                    for pair in crossings.as_chunks::<2>().0 {
                        let [a, b] = [pair[0], pair[1]];
                        let mut y = a - a % self.width - self.width;
                        while y < b {
                            if y > a {
                                emit(
                                    [
                                        grid_coord(x + translation + self.width / 2, self.width),
                                        grid_coord(y + self.width / 2, self.width),
                                        area_z,
                                    ],
                                    &mut out,
                                );
                            }
                            y += self.width;
                        }
                    }
                }
            }
        }
        out
    }
    fn line(&self, a: Point, b: Point) -> Vec<[i64; 2]> {
        let start = [a.x, a.y];
        let diff = [b.x - a.x, b.y - a.y];
        let coord = |v: i64| grid_coord(v, self.width);
        let mut cell = start.map(coord);
        let mut out = vec![];
        loop {
            out.push(cell);
            let mut crossing = f64::INFINITY;
            let mut dim = None;
            for i in 0..2 {
                if diff[i] == 0 {
                    continue;
                }
                let boundary = (cell[i] + i64::from(diff[i] > 0)) * self.width;
                let t = (boundary - start[i]) as f64 / diff[i] as f64;
                if t < crossing {
                    crossing = t;
                    dim = Some(i);
                }
            }
            if crossing > 1. {
                break;
            }
            let Some(i) = dim else {
                break;
            };
            cell[i] += diff[i].signum();
        }
        out
    }
}
fn kernel(size: i64) -> Vec<Cell> {
    let start = -size / 2;
    let end = size - size / 2;
    let mut out = vec![];
    for x in start..end {
        for y in start..end {
            for z in start..end {
                let norm = |v: i64| {
                    v as f64
                        / if v < 0 {
                            start.min(-1)
                        } else {
                            (end - 1).max(1)
                        } as f64
                };
                if norm(x) + norm(y) <= 1. + 1e-12 {
                    out.push([x, y, z]);
                }
            }
        }
    }
    out
}
fn rectangle(x: i64, y: i64, u: i64, v: i64) -> ExPolygon {
    ExPolygon {
        contour: vec![
            Point::new(x, y),
            Point::new(u, y),
            Point::new(u, v),
            Point::new(x, v),
        ],
        holes: vec![],
    }
}
fn rotate(polys: &[ExPolygon], angle: f64, origin: Point, inverse: bool) -> Vec<ExPolygon> {
    let (s, c) = if inverse { -angle } else { angle }.sin_cos();
    polys
        .iter()
        .map(|p| {
            let mut p = p.clone();
            for v in p.contour.iter_mut().chain(p.holes.iter_mut().flatten()) {
                let x = (v.x - if inverse { 0 } else { origin.x }) as f64;
                let y = (v.y - if inverse { 0 } else { origin.y }) as f64;
                *v = Point::new(
                    (c * x - s * y).round() as i64 + if inverse { origin.x } else { 0 },
                    (s * x + c * y).round() as i64 + if inverse { origin.y } else { 0 },
                );
            }
            p
        })
        .collect()
}
fn grid_coord(value: i64, width: i64) -> i64 {
    value / width - i64::from(value < 0)
}
