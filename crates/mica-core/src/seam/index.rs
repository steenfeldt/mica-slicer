// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! A uniform grid over a boundary's eligible seam candidates, answering the nearest-point query of
//! [`super::choice::nearest`] exactly: it visits every candidate whose `norm()` distance can be at
//! most the best distance found (plus a rounding margin), so the result is the same first-minimal
//! index a linear `min_by` scan would return.

use super::geometry::V;
use super::painting::Paint;
use super::perimeters::Class;

/// Rounding margin on distances in millimetres: far larger than any `hypot` or cell-bound rounding
/// error at bed scale, far smaller than the resolution of any seam decision.
const MARGIN: f64 = 1e-9;
const MAX_CELLS_PER_AXIS: usize = 1024;

pub(super) struct NearestIndex {
    /// The best (paint, class) pair; only points with this pair are indexed.
    pub best: (Paint, Class),
    min: V,
    cell: f64,
    nx: usize,
    ny: usize,
    /// CSR layout: `cells[c]..cells[c + 1]` indexes `members` for cell `c = y * nx + x`.
    cells: Vec<u32>,
    members: Vec<u32>,
}

impl NearestIndex {
    pub fn new(points: &[V], paint: &[Paint], classes: &[Class]) -> Self {
        let best = paint
            .iter()
            .copied()
            .zip(classes.iter().copied())
            .min()
            .unwrap_or((Paint::Common, Class::Common));
        let eligible: Vec<u32> = (0..points.len())
            .filter(|&i| (paint[i], classes[i]) == best)
            .map(|i| i as u32)
            .collect();
        let (mut min, mut max) = (
            V::new(f64::INFINITY, f64::INFINITY),
            V::new(f64::NEG_INFINITY, f64::NEG_INFINITY),
        );
        for &i in &eligible {
            let p = points[i as usize];
            min = V::new(min.x.min(p.x), min.y.min(p.y));
            max = V::new(max.x.max(p.x), max.y.max(p.y));
        }
        if eligible.is_empty() {
            return Self {
                best,
                min: V::default(),
                cell: 1.0,
                nx: 0,
                ny: 0,
                cells: vec![0],
                members: Vec::new(),
            };
        }
        let (w, h) = (max.x - min.x, max.y - min.y);
        // About two candidates per cell; degenerate (collinear) sets fall back to the longer side.
        let n = eligible.len() as f64;
        let mut cell = if w > 0.0 && h > 0.0 {
            (w * h / n * 2.0).sqrt()
        } else {
            w.max(h) / n * 2.0
        };
        if cell.is_nan() || cell <= 0.0 {
            cell = 1.0;
        }
        let limit = w.max(h) / MAX_CELLS_PER_AXIS as f64;
        if cell < limit {
            cell = limit;
        }
        let nx = ((w / cell).floor() as usize + 1).min(MAX_CELLS_PER_AXIS);
        let ny = ((h / cell).floor() as usize + 1).min(MAX_CELLS_PER_AXIS);
        let mut counts = vec![0u32; nx * ny + 1];
        let cell_of = |p: V| {
            let x = (((p.x - min.x) / cell) as usize).min(nx - 1);
            let y = (((p.y - min.y) / cell) as usize).min(ny - 1);
            y * nx + x
        };
        for &i in &eligible {
            counts[cell_of(points[i as usize]) + 1] += 1;
        }
        for c in 1..counts.len() {
            counts[c] += counts[c - 1];
        }
        let mut members = vec![0u32; eligible.len()];
        let mut next = counts.clone();
        // Ascending point order within a cell, so equal distances resolve to the lowest index first.
        for &i in &eligible {
            let c = cell_of(points[i as usize]);
            members[next[c] as usize] = i;
            next[c] += 1;
        }
        Self {
            best,
            min,
            cell,
            nx,
            ny,
            cells: counts,
            members,
        }
    }

    /// The eligible point nearest to `p` (by `norm()` of the difference, lowest index on ties), among
    /// those accepted by `filter`.
    pub fn nearest(
        &self,
        points: &[V],
        p: V,
        mut filter: impl FnMut(usize) -> bool,
    ) -> Option<usize> {
        if self.members.is_empty() {
            return None;
        }
        let clamp = |v: f64, n: usize| -> isize {
            if v.is_nan() || v < 0.0 {
                0
            } else {
                (v as usize).min(n - 1) as isize
            }
        };
        let cx = clamp((p.x - self.min.x) / self.cell, self.nx);
        let cy = clamp((p.y - self.min.y) / self.cell, self.ny);
        let mut best: Option<(f64, usize)> = None;
        let rings = self.nx.max(self.ny);
        for r in 0..=rings {
            if let Some((d, _)) = best
                && (r as f64 - 1.0) * self.cell > d + MARGIN
            {
                break;
            }
            let r = r as isize;
            let mut visit = |x: isize, y: isize| {
                if x < 0 || y < 0 || x >= self.nx as isize || y >= self.ny as isize {
                    return;
                }
                let c = y as usize * self.nx + x as usize;
                for &i in &self.members[self.cells[c] as usize..self.cells[c + 1] as usize] {
                    let i = i as usize;
                    if !filter(i) {
                        continue;
                    }
                    let d = (points[i] - p).norm();
                    if best.is_none_or(|(bd, bi)| d < bd || (d == bd && i < bi)) {
                        best = Some((d, i));
                    }
                }
            };
            if r == 0 {
                visit(cx, cy);
                continue;
            }
            for x in cx - r..=cx + r {
                visit(x, cy - r);
                visit(x, cy + r);
            }
            for y in cy - r + 1..cy + r {
                visit(cx - r, y);
                visit(cx + r, y);
            }
        }
        best.map(|(_, i)| i)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(points: &[V], p: V) -> Option<usize> {
        (0..points.len()).min_by(|&a, &c| (points[a] - p).norm().total_cmp(&(points[c] - p).norm()))
    }

    #[test]
    fn matches_the_linear_scan_including_ties() {
        let mut points = Vec::new();
        let mut seed = 12345u64;
        let mut next = || {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((seed >> 11) as f64 / (1u64 << 53) as f64) * 20.0 - 10.0
        };
        for _ in 0..400 {
            let (x, y) = (next(), next());
            points.push(V::new(x, y));
            // Duplicate and mirrored points create exact ties that must resolve to the lowest index.
            points.push(V::new(x, y));
            points.push(V::new(-x, y));
        }
        let paint = vec![Paint::Common; points.len()];
        let classes = vec![Class::Common; points.len()];
        let index = NearestIndex::new(&points, &paint, &classes);
        for k in 0..300 {
            let q = if k % 3 == 0 {
                points[k * 7 % points.len()]
            } else {
                V::new(next() * 2.0, next() * 2.0)
            };
            assert_eq!(index.nearest(&points, q, |_| true), scan(&points, q));
        }
    }

    #[test]
    fn only_eligible_points_are_indexed() {
        let points = vec![V::new(0.0, 0.0), V::new(1.0, 0.0), V::new(2.0, 0.0)];
        let paint = vec![Paint::Common, Paint::Enforcer, Paint::Common];
        let classes = vec![Class::Common; 3];
        let index = NearestIndex::new(&points, &paint, &classes);
        assert_eq!(index.best, (Paint::Enforcer, Class::Common));
        assert_eq!(index.nearest(&points, V::new(2.0, 0.0), |_| true), Some(1));
        assert_eq!(index.nearest(&points, V::new(2.0, 0.0), |_| false), None);
        let empty = NearestIndex::new(&[], &[], &[]);
        assert_eq!(empty.nearest(&[], V::new(0.0, 0.0), |_| true), None);
    }
}
