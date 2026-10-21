// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: orca@8500fcdcca src/slic3r/GUI/PartPlate.cpp :: PartPlate::estimate_wipe_tower_size
// Source: prusaslicer@b028299c77 src/slic3r/GUI/GLCanvas3D.cpp :: GLCanvas3D::reload_scene
// Source: prusaslicer@b028299c77 src/libslic3r/Print.hpp :: WipeTowerData
// Changes: The estimate plans a synthetic job (every used tool on every layer up to the tallest object) with the real planner instead of a volume/area formula, so it carries the brim, cone and rib footprint; the exact shape is read from the job's plan after slicing.
// Changes: The synthetic job orders its tools as toolchange_ordering says.
// Changes: Engine choices read mica_slicing_rules (mica_core::rules), not gcode_flavor or the presence of Orca keys (docs/slicing-rules.md).

//! The tower as the GUI shows it: its body box and complete first-layer footprint, either
//! estimated before slicing from the tools a plate uses and its tallest object, or exact
//! from the sliced job's plan. Coordinates are bed millimetres.

use super::{TowerKind, TowerPlan};
use crate::config::Config;
use crate::tool_ordering::ToolOrdering;

/// Size and footprint of a planned tower.
#[derive(Clone, Debug, PartialEq)]
pub struct TowerShape {
    pub kind: TowerKind,
    /// `wipe_tower_x/y`: the body's corner the layout starts from, before `rotation`.
    pub position: [f64; 2],
    /// `wipe_tower_rotation_angle`, degrees.
    pub rotation: f64,
    pub width: f64,
    /// Depth of the deepest layer, outer boundary included.
    pub depth: f64,
    /// Top of the tower: the last tool change's layer.
    pub height: f64,
    /// Printed brim width (whole loops).
    pub brim: f64,
    /// Convex outline of the first layer including brim, cone and ribs, in bed millimetres.
    pub hull: Vec<[f64; 2]>,
    /// `false` for a pre-slice estimate, `true` for the sliced job's plan.
    pub exact: bool,
}

impl TowerShape {
    /// Axis-aligned bounds of the footprint (`hull`).
    pub fn bbox(&self) -> ([f64; 2], [f64; 2]) {
        let mut min = [f64::INFINITY; 2];
        let mut max = [f64::NEG_INFINITY; 2];
        for p in &self.hull {
            for i in 0..2 {
                min[i] = min[i].min(p[i]);
                max[i] = max[i].max(p[i]);
            }
        }
        (min, max)
    }
    /// The body's four corners at its position and rotation, counter-clockwise from the
    /// anchor corner.
    pub fn body_corners(&self) -> [[f64; 2]; 4] {
        let (sin, cos) = self.rotation.to_radians().sin_cos();
        [
            [0., 0.],
            [self.width, 0.],
            [self.width, self.depth],
            [0., self.depth],
        ]
        .map(|[x, y]| {
            [
                self.position[0] + cos * x - sin * y,
                self.position[1] + sin * x + cos * y,
            ]
        })
    }
    /// Centre of the body on the bed.
    pub fn centre(&self) -> [f64; 2] {
        let c = self.body_corners();
        [(c[0][0] + c[2][0]) / 2., (c[0][1] + c[2][1]) / 2.]
    }
    /// The same tower moved by `(dx, dy)`: the position and footprint shift, nothing else
    /// changes (the plan does not depend on where the tower stands).
    pub fn translated(&self, dx: f64, dy: f64) -> Self {
        let mut out = self.clone();
        out.position = [self.position[0] + dx, self.position[1] + dy];
        for p in &mut out.hull {
            p[0] += dx;
            p[1] += dy;
        }
        out
    }
}

impl TowerPlan {
    /// The planned tower's shape; `exact` says whether the plan came from a sliced job.
    pub fn shape(&self, exact: bool) -> TowerShape {
        let s = &self.settings;
        TowerShape {
            kind: s.kind,
            position: s.position,
            rotation: s.rotation,
            width: s.width,
            depth: self.depth,
            height: self.height,
            brim: self.brim_width(),
            hull: self
                .first_layer_hull()
                .iter()
                .map(|p| [crate::geometry::unscale(p.x), crate::geometry::unscale(p.y)])
                .collect(),
            exact,
        }
    }
}

/// A tower sized before slicing: every one of `tools` (distinct, zero based) prints on
/// every layer up to `height` (the tallest object), which is the deepest tower the job can
/// need and, for the usual multi-colour print, the one it gets. Layers follow the
/// configuration's first and regular layer height. `None` when the configuration prints no
/// tower or fewer than two tools are used; errors are the planner's own (invalid tower
/// settings). The tower may stand off the bed: the caller decides where it goes.
pub fn estimate(c: &Config, tools: &[usize], height: f64) -> Result<Option<TowerShape>, String> {
    let mut used: Vec<usize> = tools.to_vec();
    used.sort_unstable();
    used.dedup();
    if used.len() < 2 || !c.bool("wipe_tower").unwrap_or(false) {
        return Ok(None);
    }
    let layer = c.float("layer_height").unwrap_or(0.2).max(0.01);
    let first = c
        .abs_value("first_layer_height", layer)
        .filter(|h| *h > 0.)
        .unwrap_or(layer);
    let height = if height.is_finite() && height > first {
        height
    } else {
        first.max(10.)
    };
    // No tower is taller than the printer: a model scaled past the build volume (which
    // cannot be sliced) must not make the estimate plan a hundred thousand layers.
    let height = c
        .float("max_print_height")
        .filter(|h| h.is_finite() && *h > first)
        .map_or(height, |max| height.min(max));
    // Whole layers only: a thin last layer would need more purge rows and, as the depth
    // propagates downwards, deepen the entire tower.
    let count = ((height - first) / layer).ceil().max(0.) as usize + 1;
    let layers: Vec<(f64, Vec<Option<usize>>)> = (0..count.min(100_000))
        .map(|i| {
            (
                first + i as f64 * layer,
                used.iter().map(|&t| Some(t)).collect(),
            )
        })
        .collect();
    let ordering = ToolOrdering::new(layers, crate::print_job::optimized_tool_ordering(c));
    Ok(TowerPlan::plan(c, &ordering)?.map(|p| p.shape(false)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(kind: &str) -> Config {
        let mut c = Config::defaults();
        let pairs: &[(&str, &str)] = match kind {
            "xl" => &[
                ("bed_shape", "0x0,360x0,360x360,0x360"),
                ("layer_height", "0.2"),
                ("first_layer_height", "0.2"),
                ("nozzle_diameter", "0.4,0.4,0.4,0.4,0.4"),
                ("wipe_tower", "1"),
                ("wipe_tower_width", "60"),
                ("wipe_tower_brim_width", "3"),
                ("wipe_tower_cone_angle", "25"),
                ("wipe_tower_extra_spacing", "150%"),
                ("filament_multitool_ramming", "1,1,1,1,1"),
                ("filament_multitool_ramming_flow", "40,40,40,40,40"),
                ("filament_multitool_ramming_volume", "10,10,10,10,10"),
                ("filament_minimal_purge_on_wipe_tower", "15,15,15,15,15"),
            ],
            _ => &[
                ("bed_shape", "0x0,270x0,270x270,0x270"),
                ("layer_height", "0.2"),
                ("gcode_flavor", "klipper"),
                ("mica_slicing_rules", "orcaslicer"),
                ("nozzle_diameter", "0.4,0.4,0.4,0.4"),
                ("wipe_tower", "1"),
                ("wipe_tower_width", "35"),
                ("wipe_tower_brim_width", "5"),
                ("wipe_tower_cone_angle", "15"),
                ("prime_volume", "30"),
                ("first_layer_height", "0.25"),
                ("filament_minimal_purge_on_wipe_tower", "15,15,15,15"),
            ],
        };
        for (k, v) in pairs {
            c.set_raw(k, v).unwrap();
        }
        c
    }

    /// A job whose every layer prints all of `tools`, at the configuration's layer heights.
    fn real(c: &Config, tools: Vec<usize>, layers: usize) -> TowerShape {
        let layer = c.float("layer_height").unwrap();
        let first = c.abs_value("first_layer_height", layer).unwrap();
        let ordering = ToolOrdering::new(
            (0..layers)
                .map(|i| {
                    (
                        first + i as f64 * layer,
                        tools.iter().map(|&t| Some(t)).collect(),
                    )
                })
                .collect(),
            true,
        );
        TowerPlan::new(c, &ordering).unwrap().unwrap().shape(true)
    }

    #[test]
    fn estimate_matches_the_exact_tower_within_fifteen_percent() {
        for (c, tools) in [
            (config("xl"), vec![0, 1]),
            (config("xl"), vec![0, 2, 4]),
            (config("u1"), vec![0, 1, 2, 3]),
        ] {
            let exact = real(&c, tools.clone(), 100);
            let est = estimate(&c, &tools, 20.).unwrap().unwrap();
            assert!(!est.exact && exact.exact);
            assert_eq!(est.width, exact.width);
            assert!(
                (est.depth - exact.depth).abs() <= 0.15 * exact.depth,
                "depth {} vs {}",
                est.depth,
                exact.depth
            );
            assert!(
                (est.height - exact.height).abs() <= 0.15 * exact.height,
                "height {} vs {}",
                est.height,
                exact.height
            );
            let (emin, emax) = est.bbox();
            let (xmin, xmax) = exact.bbox();
            for i in 0..2 {
                let size = xmax[i] - xmin[i];
                assert!(
                    (emin[i] - xmin[i]).abs() <= 0.15 * size,
                    "{i}: {emin:?} {xmin:?}"
                );
                assert!(
                    (emax[i] - xmax[i]).abs() <= 0.15 * size,
                    "{i}: {emax:?} {xmax:?}"
                );
            }
        }
    }

    #[test]
    fn xl_estimate_has_the_documented_footprint() {
        // Two tools, 20 mm tall: 60 × 4.25 mm body, brim and cone circle as the oracle
        // table in docs/wipe-tower.md lists them (X 176.8–243.2, Y 134.5–149.7).
        let s = estimate(&config("xl"), &[0, 1], 20.).unwrap().unwrap();
        assert_eq!(s.kind, TowerKind::Prusa);
        assert!((s.depth - 4.25).abs() < 1e-6, "{}", s.depth);
        assert!((s.brim - 3.2).abs() < 0.01, "{}", s.brim);
        let (min, max) = s.bbox();
        assert!(
            (min[0] - 176.8).abs() < 0.01 && (max[0] - 243.2).abs() < 0.01,
            "{min:?} {max:?}"
        );
        assert!(
            (min[1] - 134.5).abs() < 0.1 && (max[1] - 149.7).abs() < 0.1,
            "{min:?} {max:?}"
        );
        let c = s.body_corners();
        assert_eq!(c[0], [180., 140.]);
        assert!((c[2][0] - 240.).abs() < 1e-9 && (c[2][1] - 144.25).abs() < 1e-9);
        assert_eq!(s.centre(), [210., 142.125]);
    }

    #[test]
    fn u1_estimate_is_the_four_tool_body_with_ribs_and_brim() {
        let s = estimate(&config("u1"), &[0, 1, 2, 3], 48.)
            .unwrap()
            .unwrap();
        assert_eq!(s.kind, TowerKind::Orca);
        assert!((s.width - 35.).abs() < 1e-9);
        assert!((s.depth - 13.5).abs() < 1e-6, "{}", s.depth);
        let (min, max) = s.bbox();
        // The complete footprint of the cat project's tower: 46.89 × 28.45 mm.
        assert!((max[0] - min[0] - 46.89).abs() < 0.05, "{min:?} {max:?}");
        assert!((max[1] - min[1] - 28.45).abs() < 0.05, "{min:?} {max:?}");
    }

    #[test]
    fn estimate_needs_two_tools_and_an_enabled_tower_and_may_stand_off_the_bed() {
        let c = config("xl");
        assert!(estimate(&c, &[1], 20.).unwrap().is_none());
        assert!(estimate(&c, &[1, 1], 20.).unwrap().is_none());
        let mut off = c.clone();
        off.set_raw("wipe_tower", "0").unwrap();
        assert!(estimate(&off, &[0, 1], 20.).unwrap().is_none());
        let mut outside = c.clone();
        outside.set_raw("wipe_tower_x", "340").unwrap();
        let s = estimate(&outside, &[0, 1], 20.).unwrap().unwrap();
        assert!(s.bbox().1[0] > 360.);
        let moved = s.translated(-100., 5.);
        assert_eq!(moved.position, [240., 145.]);
        assert!((moved.bbox().1[0] - (s.bbox().1[0] - 100.)).abs() < 1e-9);
        assert_eq!(moved.depth, s.depth);
    }

    #[test]
    fn estimate_height_is_the_tallest_object_and_rotation_turns_the_footprint() {
        let c = config("xl");
        let s = estimate(&c, &[0, 1], 33.3).unwrap().unwrap();
        // Whole layers: the top is the first layer at or above the object.
        assert!((s.height - 33.4).abs() < 1e-6, "{}", s.height);
        let mut r = c.clone();
        r.set_raw("wipe_tower_rotation_angle", "90").unwrap();
        let t = estimate(&r, &[0, 1], 33.3).unwrap().unwrap();
        let (a, b) = (s.bbox(), t.bbox());
        assert!(((a.1[0] - a.0[0]) - (b.1[1] - b.0[1])).abs() < 1e-6);
        assert!(((a.1[1] - a.0[1]) - (b.1[0] - b.0[0])).abs() < 1e-6);
    }

    /// Monkey-test regression: a model scaled far past the build volume made the GUI plan a
    /// 100,000-layer tower on its UI thread at every change (the window froze for minutes).
    /// The estimate stops at the printer's height.
    #[test]
    fn estimate_never_plans_past_the_printer_height() {
        let mut c = config("xl");
        c.set_raw("max_print_height", "360").unwrap();
        let started = std::time::Instant::now();
        let huge = estimate(&c, &[0, 1], 20_000.).unwrap().unwrap();
        assert_eq!(huge, estimate(&c, &[0, 1], 360.).unwrap().unwrap());
        assert!(huge.height <= 360. + 1e-6, "{}", huge.height);
        assert!(started.elapsed() < std::time::Duration::from_secs(20));
    }
}
