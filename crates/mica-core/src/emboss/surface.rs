// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/slic3r/GUI/Jobs/EmbossJob.cpp :: cut_surface_to_its
// Source: prusaslicer@b028299c77 src/libslic3r/CutSurface.cpp :: its_cut_AoI
// Changes: Only the "area of interest" idea is kept: the target meshes are taken into the text frame and the faces under the outline's bounding box that look out along the text Z are binned in a uniform XY grid; a query returns the surface height nearest the text plane, and the union of those faces is the coverage an outline is cut to (PrusaSlicer cuts the surface with the outline in CGAL instead).
// Upstream attribution (EmbossJob.cpp, CutSurface.cpp):
// Copyright (c) Prusa Research 2021 - 2023 Filip Sykala @Jony01, Lukáš Matěna @lukasmatena, Vojtěch Bubník @bubnikv
//
// PrusaSlicer is released under the terms of the AGPLv3 or higher

//! Surface heights under an outline, in the text frame.

use crate::mesh::TriangleMesh;
use crate::threemf::Transform;

/// Triangles of the surface around the text, binned for vertical ray queries.
pub struct SurfaceProbe {
    tris: Vec<[[f64; 3]; 3]>,
    cells: Vec<Vec<u32>>,
    min: [f64; 2],
    cell: f64,
    nx: usize,
    ny: usize,
    /// Hits farther than this from the text plane (mm) are ignored.
    reach: f64,
}

impl SurfaceProbe {
    /// `meshes` with the transform that takes each into the text frame; only triangles over
    /// `bounds` (text-frame XY min / max, widened by a cell) are kept. `cell` is the bin size.
    pub fn new(
        meshes: &[(&TriangleMesh, Transform)],
        bounds: ([f64; 2], [f64; 2]),
        cell: f64,
    ) -> Self {
        let cell = cell.max(0.05);
        let min = [bounds.0[0] - cell, bounds.0[1] - cell];
        let max = [bounds.1[0] + cell, bounds.1[1] + cell];
        let nx = (((max[0] - min[0]) / cell).ceil() as usize).clamp(1, 1024);
        let ny = (((max[1] - min[1]) / cell).ceil() as usize).clamp(1, 1024);
        let cell = ((max[0] - min[0]) / nx as f64).max((max[1] - min[1]) / ny as f64);
        let mut probe = Self {
            tris: Vec::new(),
            cells: vec![Vec::new(); nx * ny],
            min,
            cell,
            nx,
            ny,
            reach: (max[0] - min[0]).max(max[1] - min[1]).max(10.),
        };
        let reach = probe.reach;
        for (mesh, t) in meshes {
            let flip = t.determinant() < 0.;
            let v: Vec<[f64; 3]> = mesh
                .vertices
                .iter()
                .map(|p| {
                    let p = p.map(f64::from);
                    let r = t.0;
                    [0, 1, 2].map(|i| r[i][0] * p[0] + r[i][1] * p[1] + r[i][2] * p[2] + r[i][3])
                })
                .collect();
            for f in &mesh.faces {
                let mut tri = f.map(|i| v[i as usize]);
                if flip {
                    tri.swap(1, 2);
                }
                // Only faces looking out along the text Z (not edge-on, not the far side) carry
                // the text, and only near the text plane.
                let [a, b, c] = tri;
                let (u, w) = (sub(b, a), sub(c, a));
                let n = [
                    u[1] * w[2] - u[2] * w[1],
                    u[2] * w[0] - u[0] * w[2],
                    u[0] * w[1] - u[1] * w[0],
                ];
                let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
                if n[2] <= 0.05 * len || tri.iter().all(|p| p[2].abs() > reach) {
                    continue;
                }
                let lo = [0, 1].map(|k| tri.iter().map(|p| p[k]).fold(f64::INFINITY, f64::min));
                let hi = [0, 1].map(|k| tri.iter().map(|p| p[k]).fold(f64::NEG_INFINITY, f64::max));
                if hi[0] < min[0] || lo[0] > max[0] || hi[1] < min[1] || lo[1] > max[1] {
                    continue;
                }
                let id = probe.tris.len() as u32;
                probe.tris.push(tri);
                let (x0, y0) = probe.index(lo[0], lo[1]);
                let (x1, y1) = probe.index(hi[0], hi[1]);
                for y in y0..=y1 {
                    for x in x0..=x1 {
                        probe.cells[y * nx + x].push(id);
                    }
                }
            }
        }
        probe
    }

    fn index(&self, x: f64, y: f64) -> (usize, usize) {
        let ix = ((x - self.min[0]) / self.cell)
            .floor()
            .clamp(0., (self.nx - 1) as f64) as usize;
        let iy = ((y - self.min[1]) / self.cell)
            .floor()
            .clamp(0., (self.ny - 1) as f64) as usize;
        (ix, iy)
    }

    /// Where the surface lies under the text (the union of the kept faces seen along Z, in
    /// scaled coordinates): an outline is cut to it so nothing hangs past the surface's edge.
    pub fn coverage(&self) -> Vec<crate::geometry::ExPolygon> {
        use crate::geometry::{Point, clip};
        let polys: Vec<crate::geometry::Polygon> = self
            .tris
            .iter()
            .map(|t| t.iter().map(|p| Point::from_mm(p[0], p[1])).collect())
            .collect();
        clip::union_ex_positive(&polys)
    }

    /// Whether any surface lies under the outline's box at all.
    pub fn is_empty(&self) -> bool {
        self.tris.is_empty()
    }

    /// Height (text-frame Z) of the surface under `(x, y)` closest to z = 0, if one is within
    /// reach.
    pub fn height(&self, x: f64, y: f64) -> Option<f64> {
        if x < self.min[0]
            || y < self.min[1]
            || x > self.min[0] + self.cell * self.nx as f64
            || y > self.min[1] + self.cell * self.ny as f64
        {
            return None;
        }
        let (ix, iy) = self.index(x, y);
        let mut best: Option<f64> = None;
        for &t in &self.cells[iy * self.nx + ix] {
            let [a, b, c] = self.tris[t as usize];
            let d = (b[0] - a[0]) * (c[1] - a[1]) - (c[0] - a[0]) * (b[1] - a[1]);
            if d.abs() < 1e-14 {
                continue; // edge-on to the text plane
            }
            let u = ((x - a[0]) * (c[1] - a[1]) - (c[0] - a[0]) * (y - a[1])) / d;
            let v = ((b[0] - a[0]) * (y - a[1]) - (x - a[0]) * (b[1] - a[1])) / d;
            const E: f64 = -1e-9;
            if u < E || v < E || u + v > 1. - E {
                continue;
            }
            let z = a[2] + u * (b[2] - a[2]) + v * (c[2] - a[2]);
            if z.abs() <= self.reach && best.is_none_or(|h| z.abs() < h.abs()) {
                best = Some(z);
            }
        }
        best
    }
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
