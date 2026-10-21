// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/GCode.cpp :: SmoothPathCache::operator(), get_seam_gap_distance_value
// Source: orca@8500fcdcca src/libslic3r/GCode.cpp :: GCode::generate_skirt
// Changes: Small emission hook for adhesion loops; existing writer handles extrusion and travel.

use mica_core::{
    geometry::{Point, SCALE},
    settings::PrintSettings,
    toolpath::{LayerToolpaths, Role},
};

pub(crate) fn prepare_paths(layer: &mut LayerToolpaths, settings: &PrintSettings) {
    let mut previous = Point::default();
    for path in &mut layer.paths {
        if !matches!(path.role, Role::Skirt | Role::Brim) {
            continue;
        }
        if path.points.len() > 3 && path.points.first() == path.points.last() {
            path.points.pop();
            // Orca skirt loops already carry the explicitly requested start angle.
            if !(settings.adhesion.orca && path.role == Role::Skirt) {
                let start = path
                    .points
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, p)| p.distance_sq(previous))
                    .unwrap()
                    .0;
                path.points.rotate_left(start);
            }
            path.points.push(path.points[0]);
            let mut gap = settings
                .seam
                .gap
                .resolve(settings.nozzle_diameter, 1.0)
                .max(0.0)
                * SCALE;
            while gap > 0.0 && path.points.len() >= 2 {
                let n = path.points.len();
                let a = path.points[n - 2];
                let b = path.points[n - 1];
                let len = a.distance_to(b);
                if len > gap {
                    let t = (len - gap) / len;
                    path.points[n - 1] = Point::new(
                        a.x + ((b.x - a.x) as f64 * t).round() as i64,
                        a.y + ((b.y - a.y) as f64 * t).round() as i64,
                    );
                    break;
                }
                gap -= len;
                path.points.pop();
            }
        }
        if let Some(p) = path.points.last() {
            previous = *p;
        }
    }
    layer.paths.retain(|p| p.points.len() >= 2);
}
