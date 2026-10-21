// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: prusaslicer@b028299c77 src/libslic3r/Support/OrganicSupport.cpp :: organic_smooth_branches_avoid_collisions
// Source: orca@8500fcdcca src/libslic3r/Support/TreeSupport3D.cpp :: organic_smooth_branches_avoid_collisions
// Changes: Rayon-parallel Jacobi iteration and indexed links; bounded line scan (narrowed by a uniform grid, same order) replaces AABB-tree nearest query; zero-length shifts remain stationary; the unsigned layer difference (no collisions below a sphere's own layer) and object coordinates are kept.
// Copyright (c) Prusa Research 2023 Vojtěch Bubník

use super::{
    branch_mesh::{NodeId, V, position},
    *,
};
use crate::geometry::{Point, SCALE};
use rayon::prelude::*;
#[derive(Clone)]
struct Sphere {
    id: NodeId,
    below: Option<usize>,
    parents: Vec<usize>,
    locked: bool,
    radius: f64,
    pos: V,
    min_z: f64,
    max_z: f64,
    begin: usize,
    end: usize,
}
fn xy(p: V) -> V {
    V([p.0[0], p.0[1], 0.])
}
/// One layer's collision outline as segments, bucketed in a uniform grid. Upstream asks an
/// AABB tree for the nearest segment; Mica's scan visits the segments in their original order
/// and keeps the first strictly closer one, so the grid only narrows that scan to the segments
/// whose bounding box is within reach, still in the original order (same hit, same ties).
struct LayerLines {
    lines: Vec<(V, V)>,
    grid: Option<Grid>,
}
struct Grid {
    x0: f64,
    y0: f64,
    cell: f64,
    nx: usize,
    ny: usize,
    /// Segment indices per cell (CSR): `items[starts[c]..starts[c + 1]]`, ascending.
    starts: Vec<u32>,
    items: Vec<u32>,
}
/// Below this many segments a plain scan is as fast as the grid.
const GRID_MIN_LINES: usize = 64;
fn bounds(a: V, b: V) -> [f64; 4] {
    [
        a.0[0].min(b.0[0]),
        a.0[0].max(b.0[0]),
        a.0[1].min(b.0[1]),
        a.0[1].max(b.0[1]),
    ]
}
impl Grid {
    fn span(&self, lo: f64, hi: f64, origin: f64, n: usize) -> (usize, usize) {
        let f = |x: f64| ((x - origin) / self.cell).floor().clamp(0., (n - 1) as f64) as usize;
        (f(lo), f(hi))
    }
    fn new(lines: &[(V, V)]) -> Option<Self> {
        if lines.len() < GRID_MIN_LINES {
            return None;
        }
        let mut b = [f64::MAX, f64::MIN, f64::MAX, f64::MIN];
        for &(p, q) in lines {
            let l = bounds(p, q);
            b = [
                b[0].min(l[0]),
                b[1].max(l[1]),
                b[2].min(l[2]),
                b[3].max(l[3]),
            ];
        }
        let (w, h) = ((b[1] - b[0]).max(1e-3), (b[3] - b[2]).max(1e-3));
        // About two segments per cell, never finer than 0.25 mm.
        let cell = (w * h / (lines.len() as f64 / 2.)).sqrt().max(0.25);
        let nx = ((w / cell).floor() as usize + 1).min(4096);
        let ny = ((h / cell).floor() as usize + 1).min(4096);
        let mut grid = Self {
            x0: b[0],
            y0: b[2],
            cell,
            nx,
            ny,
            starts: vec![0; nx * ny + 1],
            items: Vec::new(),
        };
        let spans: Vec<_> = lines
            .iter()
            .map(|&(p, q)| {
                let l = bounds(p, q);
                (
                    grid.span(l[0], l[1], grid.x0, nx),
                    grid.span(l[2], l[3], grid.y0, ny),
                )
            })
            .collect();
        for &((x0, x1), (y0, y1)) in &spans {
            for y in y0..=y1 {
                for x in x0..=x1 {
                    grid.starts[y * nx + x + 1] += 1;
                }
            }
        }
        for c in 0..nx * ny {
            grid.starts[c + 1] += grid.starts[c];
        }
        let mut fill = grid.starts.clone();
        grid.items = vec![0; grid.starts[nx * ny] as usize];
        for (i, &((x0, x1), (y0, y1))) in spans.iter().enumerate() {
            for y in y0..=y1 {
                for x in x0..=x1 {
                    let c = y * nx + x;
                    grid.items[fill[c] as usize] = i as u32;
                    fill[c] += 1;
                }
            }
        }
        Some(grid)
    }
    /// Every segment whose bounding box lies within `reach` of `p` (and possibly a few more),
    /// ascending and without duplicates.
    fn candidates(&self, p: V, reach: f64, out: &mut Vec<u32>) {
        out.clear();
        let (x0, x1) = self.span(p.0[0] - reach, p.0[0] + reach, self.x0, self.nx);
        let (y0, y1) = self.span(p.0[1] - reach, p.0[1] + reach, self.y0, self.ny);
        for y in y0..=y1 {
            let row = y * self.nx;
            for c in row + x0..=row + x1 {
                out.extend_from_slice(
                    &self.items[self.starts[c] as usize..self.starts[c + 1] as usize],
                );
            }
        }
        if (y1 > y0) || (x1 > x0) {
            out.sort_unstable();
            out.dedup();
        }
    }
}
impl LayerLines {
    fn new(lines: Vec<(V, V)>) -> Self {
        let grid = Grid::new(&lines);
        Self { lines, grid }
    }
}
fn nearest_in(layer: &LayerLines, p: V, limit: f64, scratch: &mut Vec<u32>) -> Option<(f64, V)> {
    let Some(grid) = &layer.grid else {
        return nearest(layer.lines.iter().copied(), p, limit);
    };
    // A segment the scan does not skip has bounding-box distance² <= best <= limit; the margin
    // covers the rounding of the square root.
    let reach = limit.sqrt() * (1. + 1e-9) + 1e-6;
    grid.candidates(p, reach, scratch);
    nearest(scratch.iter().map(|&i| layer.lines[i as usize]), p, limit)
}
fn nearest(lines: impl Iterator<Item = (V, V)>, p: V, limit: f64) -> Option<(f64, V)> {
    let mut best = limit;
    let mut hit = None;
    for (a, b) in lines {
        let minx = a.0[0].min(b.0[0]);
        let maxx = a.0[0].max(b.0[0]);
        let miny = a.0[1].min(b.0[1]);
        let maxy = a.0[1].max(b.0[1]);
        let dx = (minx - p.0[0]).max(0.).max(p.0[0] - maxx);
        let dy = (miny - p.0[1]).max(0.).max(p.0[1] - maxy);
        if dx * dx + dy * dy > best {
            continue;
        }
        let d = b - a;
        let t = if d.norm2() > 0. {
            ((p - a).0[0] * d.0[0] + (p - a).0[1] * d.0[1]) / d.norm2()
        } else {
            0.
        };
        let h = a + d * t.clamp(0., 1.);
        let ds = (p - h).norm2();
        if ds < best {
            best = ds;
            hit = Some((best.sqrt(), h));
        }
    }
    hit
}
/// Layers a collision sphere is tested against. Both pins compute `dz` from
/// `uint32_t layer_id - LayerIndex layer_idx`, i.e. in unsigned arithmetic: below the sphere's
/// own layer the difference wraps to ~4.3e9, `r2 < 0`, and that layer never collides. So only
/// the own layer and the layers above are tested, although `layer_begin` reaches down to
/// `min_z`.
fn collision_layers(begin: usize, end: usize, own: usize) -> std::ops::Range<usize> {
    begin.max(own)..end
}
fn constrain(candidate: V, current: V, anchor: V, allowed: f64) -> V {
    let delta = candidate - anchor;
    let dist = delta.norm();
    let allowed = allowed.max((current - anchor).norm());
    if dist > allowed && dist > 1e-4 {
        anchor + delta * (allowed / dist)
    } else {
        candidate
    }
}
fn linked(
    mut candidate: V,
    current: V,
    s: &Sphere,
    all: &[Sphere],
    old: &[V],
    c: &TreeSupportSettings,
) -> V {
    if let Some(i) = s.below {
        candidate = constrain(
            candidate,
            current,
            xy(old[i]),
            (all[i].radius - s.radius).max(0.) + c.maximum_move_distance_slow as f64 / SCALE,
        );
    }
    for &i in &s.parents {
        candidate = constrain(
            candidate,
            current,
            xy(old[i]),
            (s.radius - all[i].radius).max(0.) + c.maximum_move_distance_slow as f64 / SCALE,
        );
    }
    candidate
}
pub(super) fn organic_smooth_branches_avoid_collisions<E>(
    v: &mut TreeModelVolumes,
    c: &TreeSupportSettings,
    p: &TreeSlicingParameters,
    bounds: &mut [SupportElements],
    cancel: &mut impl FnMut() -> Result<(), E>,
) -> Result<(), E> {
    let n = bounds.len();
    let mut offsets = vec![0; n + 1];
    for i in 0..n {
        offsets[i + 1] = offsets[i] + bounds[i].len();
    }
    let mut below = vec![None; offsets[n]];
    for (layer, elements) in bounds.iter().enumerate() {
        for (idx, e) in elements.iter().enumerate() {
            for &parent in &e.parents {
                below[offsets[layer + 1] + parent as usize] = Some(offsets[layer] + idx);
            }
        }
    }
    let mut spheres: Vec<Sphere> = Vec::with_capacity(offsets[n]);
    for (l, elements) in bounds.iter().enumerate() {
        for (i, e) in elements.iter().enumerate() {
            let pos = position(bounds, (l, i), c, p).float();
            let down = below[offsets[l] + i];
            let min_z = down.map_or(pos.0[2], |j| spheres[j].min_z);
            spheres.push(Sphere {
                id: (l, i),
                below: down,
                parents: e
                    .parents
                    .iter()
                    .map(|&i| offsets[l + 1] + i as usize)
                    .collect(),
                locked: e.parents.is_empty() || (down.is_none() && l > 0),
                radius: f64::from((e.radius(c) as f64 / SCALE) as f32),
                pos,
                min_z,
                max_z: f64::MAX,
                begin: 0,
                end: 0,
            });
        }
    }
    for i in (0..spheres.len()).rev() {
        spheres[i].max_z = if spheres[i].parents.is_empty() {
            spheres[i].pos.0[2]
        } else {
            spheres[i]
                .parents
                .iter()
                .map(|&j| spheres[j].max_z)
                .fold(f64::MAX, f64::min)
        };
    }
    let mut lines = vec![vec![]; n];
    for (l, elements) in bounds.iter().enumerate() {
        if !elements.is_empty() {
            // Upstream forces min_element_radius=0 before its lower-bound lookup.
            v.get_collision(0, l, true);
            if let Some((_, a)) = v.get_collision_lower_bound_area(l, 0) {
                for ex in a.iter() {
                    for poly in ex.polygons() {
                        for i in 0..poly.len() {
                            // Object coordinates, as the sphere positions.
                            let pt = |j: usize| {
                                V([
                                    (poly[j].x - p.origin.x) as f64 / SCALE,
                                    (poly[j].y - p.origin.y) as f64 / SCALE,
                                    0.,
                                ])
                            };
                            lines[l].push((pt(i), pt((i + 1) % poly.len())));
                        }
                    }
                }
            }
        }
        cancel()?;
    }
    let lines: Vec<LayerLines> = lines.into_par_iter().map(LayerLines::new).collect();
    for s in &mut spheres {
        s.min_z = s.min_z.max(s.pos.0[2] - s.radius);
        s.max_z = s.max_z.min(s.pos.0[2] + s.radius);
        s.begin = s.id.0.min(c.layer_idx_ceil(p, s.min_z));
        s.end = (s.id.0.max(c.layer_idx_floor(p, s.max_z)) + 1).min(n);
    }
    for _ in 0..100 {
        cancel()?;
        let old: Vec<_> = spheres.iter().map(|s| s.pos).collect();
        // Jacobi iteration: every update reads only the previous positions (upstream
        // runs it as a TBB parallel_for), so the parallel map is exact.
        let updated: Vec<(V, bool)> = spheres
            .par_iter()
            .map(|s| {
                if s.locked {
                    return (s.pos, false);
                }
                let mut moved = false;
                let mut depth = -f64::MAX;
                let mut hit = V::default();
                let mut pos = xy(s.pos);
                let mut scratch = Vec::new();
                let range = collision_layers(s.begin, s.end, s.id.0);
                for (l, layer_lines) in lines.iter().enumerate().take(range.end).skip(range.start) {
                    let dz = (l as f64 - s.id.0 as f64) * p.layer_height;
                    let r2 = s.radius * s.radius - dz * dz;
                    if r2 > 0.
                        && let Some((dist, h)) = nearest_in(layer_lines, pos, r2, &mut scratch)
                    {
                        let d = r2.sqrt() - dist;
                        if d > depth {
                            depth = d;
                            hit = h;
                        }
                    }
                }
                if depth > 0. {
                    moved = depth > 1e-4;
                    let distance = (depth + 0.1).clamp(0., 0.5);
                    let candidate = pos + (pos - hit).unit() * (distance * distance);
                    pos = if c.settings.rules.is_orca() {
                        linked(candidate, pos, s, &spheres, &old, c)
                    } else {
                        candidate
                    };
                    pos = pos.float();
                }
                let mut avg = V::default();
                let mut weight = 0.;
                for &j in &s.parents {
                    avg = avg + xy(old[j]) * s.radius;
                    weight += s.radius;
                }
                if let Some(j) = s.below {
                    avg = avg + xy(old[j]) * weight;
                    weight *= 2.;
                }
                if weight > 0. {
                    let shift = (avg / weight - pos) * 0.5;
                    let length = shift.norm();
                    if length > 0. {
                        let candidate = pos + shift * (length.min(0.2) / length);
                        pos = if c.settings.rules.is_orca() {
                            linked(candidate, pos, s, &spheres, &old, c)
                        } else {
                            candidate
                        };
                    }
                }
                (V([pos.0[0], pos.0[1], s.pos.0[2]]).float(), moved)
            })
            .collect();
        let mut moved = 0;
        for (s, (pos, m)) in spheres.iter_mut().zip(updated) {
            s.pos = pos;
            moved += usize::from(m);
        }
        if moved == 0 {
            break;
        }
    }
    for s in spheres {
        bounds[s.id.0][s.id.1].state.result_on_layer = Some(Point::new(
            (s.pos.0[0] * SCALE) as i64 + p.origin.x,
            (s.pos.0[1] * SCALE) as i64 + p.origin.y,
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::collision_layers;

    #[test]
    fn spheres_only_collide_with_their_own_layer_and_above() {
        // A sphere on layer 5 spanning layers 2..9 is tested on 5..9 only (unsigned wrap).
        assert_eq!(collision_layers(2, 9, 5), 5..9);
        assert_eq!(collision_layers(5, 9, 5), 5..9);
        // A tip whose span ends at its own layer.
        assert_eq!(collision_layers(3, 6, 5), 5..6);
    }
}
