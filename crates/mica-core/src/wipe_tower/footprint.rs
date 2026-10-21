// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/Print.cpp :: Print::first_layer_wipe_tower_corners
// Source: orca@8500fcdcca src/libslic3r/Print.cpp :: Print::first_layer_wipe_tower_corners
// Changes: Prusa nominal brim/cone envelope; Orca first-layer bounds in tower-local coordinates, rotated once.

use super::*;

impl TowerPlan {
    /// The brim as printed: the requested width rounded to whole first-layer loops.
    pub fn brim_width(&self) -> f64 {
        let Some(first) = self.layers.first() else {
            return 0.;
        };
        let s = &self.settings;
        let spacing = s.line_width - first.height * (1. - std::f64::consts::FRAC_PI_4);
        ((s.brim + spacing / 2.) / spacing).floor() * spacing
    }
    /// Startup/skirt envelope, distinct from the bounds of emitted extrusion centrelines.
    pub fn first_layer_hull(&self) -> Vec<Point> {
        if self.layers.is_empty() {
            return Vec::new();
        }
        let s = &self.settings;
        let (sin, cos) = s.rotation.to_radians().sin_cos();
        let brim = self.brim_width();
        let (lo, hi) = if s.kind == TowerKind::Prusa {
            ([-brim, -brim], [s.width + brim, self.depth + brim])
        } else {
            // Undo placement before taking the box, so a rotated tower does not grow twice.
            let mut min = [f64::INFINITY; 2];
            let mut max = [f64::NEG_INFINITY; 2];
            for visit in self.visits(0) {
                for block in visit.before.iter().chain(std::iter::once(&visit.after)) {
                    for m in &block.moves {
                        if m.volume <= 0. {
                            continue;
                        }
                        let x = m.xy[0] - s.position[0];
                        let y = m.xy[1] - s.position[1];
                        let p = [cos * x + sin * y, -sin * x + cos * y];
                        for i in 0..2 {
                            min[i] = min[i].min(p[i]);
                            max[i] = max[i].max(p[i]);
                        }
                    }
                }
            }
            if !min[0].is_finite() {
                return Vec::new();
            }
            (min, max)
        };
        let mut pts = vec![lo, [hi[0], lo[1]], hi, [lo[0], hi[1]]];
        let radius = (s.cone_angle.to_radians() / 2.).tan() * self.height;
        let fake_width = 0.66 * s.width;
        let diagonal = (fake_width / 2.).hypot(self.depth / 2.);
        let scale_x = if radius > diagonal {
            let t = (radius - diagonal) * 0.5 * self.depth / diagonal;
            let tan = self.depth / fake_width;
            (fake_width / 2. + t / tan + t * tan) / (fake_width / 2.)
        } else {
            1.
        };
        for i in 0..40 {
            let angle = std::f64::consts::TAU * i as f64 / 40.;
            pts.push([
                (lo[0] + hi[0]) / 2. + (radius + brim) * angle.cos() / scale_x,
                (lo[1] + hi[1]) / 2. + (radius + brim) * angle.sin(),
            ]);
        }
        crate::brim::convex_hull(
            pts.into_iter()
                .map(|p| {
                    Point::from_mm(
                        s.position[0] + cos * p[0] - sin * p[1],
                        s.position[1] + sin * p[0] + cos * p[1],
                    )
                })
                .collect(),
        )
    }
}
