// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: orca@8500fcdcca src/libslic3r/GCode.cpp :: GCode::_extrude
// Changes: Optional Orca batch B wall, bridge or scarf controls; existing Prusa defaults retained.
// Source: prusaslicer@b028299c77 src/libslic3r/GCode.cpp :: GCodeGenerator::extrude_smooth_path, GCodeGenerator::extrude_perimeters
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/CoolingBuffer.cpp :: split_linear_segment
// Changes: Emit Mica's optional per-point scarf fractions without XY simplification; preserve Z ramps, extrusion factors and inward travel in streaming output.
// Changes: Brick layers: a raised wall starts at its raised Z.

use crate::writer::Writer;
use mica_core::geometry::{SCALE, unscale};
use mica_core::toolpath::{ExtrusionPath, LayerToolpaths};

/// Z of the path's first point: the layer's, a scarf ramp's start, or a brick-layers wall's
/// raised Z.
pub(crate) fn start_z(path: &ExtrusionPath, layer: &LayerToolpaths) -> f64 {
    if let Some(brick) = path.brick.filter(|b| b.raise > 0.0) {
        return layer.print_z + brick.raise;
    }
    layer.print_z
        + path
            .seam
            .as_ref()
            .and_then(|s| s.samples.first())
            .map_or(0.0, |s| (s.height - 1.0) * layer.height)
}
/// Returns false for an ordinary path, which can use the exporter's usual simplification.
#[cfg(test)]
pub(crate) fn extrude(
    w: &mut Writer,
    path: &ExtrusionPath,
    layer: &LayerToolpaths,
    speed: f64,
    e_per_mm: f64,
    z_speed: f64,
) -> bool {
    extrude_range(
        w,
        path,
        layer,
        0..path.points.len().saturating_sub(1),
        [speed, e_per_mm, z_speed],
    )
}

#[cfg(test)]
pub(crate) fn extrude_range(
    w: &mut Writer,
    path: &ExtrusionPath,
    layer: &LayerToolpaths,
    range: std::ops::Range<usize>,
    motion: [f64; 3],
) -> bool {
    extrude_range_with_cooling(w, path, layer, range, motion, [0., motion[0]])
}

pub(crate) fn extrude_range_with_cooling(
    w: &mut Writer,
    path: &ExtrusionPath,
    layer: &LayerToolpaths,
    range: std::ops::Range<usize>,
    motion: [f64; 3],
    cooling_tail: [f64; 2],
) -> bool {
    let [speed, e_per_mm, z_speed] = motion;
    let Some(seam) = &path.seam else {
        return false;
    };
    if seam.samples.is_empty() {
        return false;
    }
    assert_eq!(path.points.len(), seam.samples.len());
    if range.start == 0 {
        w.comment("SCARF_SEAM");
    }
    let total: f64 = path.points[range.start..=range.end]
        .windows(2)
        .map(|e| e[0].distance_to(e[1]) / SCALE)
        .sum();
    let split = total - cooling_tail[0];
    let mut elapsed = 0.;
    for i in range.clone() {
        let edge = &path.points[i..=i + 1];
        let sample = seam.samples[i + 1];
        let sloped = seam
            .scarf_ranges
            .is_some_and(|[end, start]| i < end || i >= start);
        let limit = if sloped {
            seam.scarf_speed.unwrap_or(f64::INFINITY)
        } else {
            f64::INFINITY
        };
        let speed = speed.min(limit);
        let length = edge[0].distance_to(edge[1]) / SCALE;
        let z = layer.print_z + (sample.height - 1.0) * layer.height;
        let dz = z - w.z();
        let distance = length.hypot(dz);
        if distance < 1e-6 {
            continue;
        }
        let speed = if dz.abs() > 1e-9 {
            speed.min(z_speed * distance / dz.abs())
        } else {
            speed
        };
        let tail_speed = if dz.abs() > 1e-9 {
            cooling_tail[1]
                .min(limit)
                .min(z_speed * distance / dz.abs())
        } else {
            cooling_tail[1].min(limit)
        };
        if cooling_tail[0] > 0. && split > elapsed + 0.01 && split < elapsed + length - 0.01 {
            let t = (split - elapsed) / length;
            let x = unscale(edge[0].x) + t * unscale(edge[1].x - edge[0].x);
            let y = unscale(edge[0].y) + t * unscale(edge[1].y - edge[0].y);
            w.extrude_to_xyz(
                x,
                y,
                w.z() + t * dz,
                t * length * e_per_mm * sample.extrusion,
                speed,
            );
            w.extrude_to_xyz(
                unscale(edge[1].x),
                unscale(edge[1].y),
                z,
                (1. - t) * length * e_per_mm * sample.extrusion,
                tail_speed,
            );
        } else {
            w.extrude_to_xyz(
                unscale(edge[1].x),
                unscale(edge[1].y),
                z,
                length * e_per_mm * sample.extrusion,
                if cooling_tail[0] > 0. && elapsed >= split - 0.01 {
                    tail_speed
                } else {
                    speed
                },
            );
        }
        elapsed += length;
    }
    // A very large positive seam gap may clip the path while it is still on the rising ramp.
    if range.end + 1 == path.points.len() {
        w.move_z(layer.print_z, z_speed);
    }
    true
}
pub(crate) fn wipe(w: &mut Writer, path: &ExtrusionPath, speed: f64) {
    if let Some(p) = path.seam.as_ref().and_then(|s| s.wipe) {
        w.comment("move inwards before travel");
        w.travel_to(unscale(p.x), unscale(p.y), speed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mica_core::flow::Flow;
    use mica_core::geometry::Point;
    use mica_core::seam::{PathSeam, ScarfSample};
    use mica_core::toolpath::Role;

    #[test]
    fn scarf_emits_simultaneous_xyz_and_preserves_extrusion_fractions() {
        let layer = LayerToolpaths {
            print_z: 0.4,
            height: 0.2,
            ..LayerToolpaths::default()
        };
        let path = ExtrusionPath {
            role: Role::ExternalPerimeter,
            points: vec![
                Point::from_mm(0.0, 0.0),
                Point::from_mm(1.0, 0.0),
                Point::from_mm(2.0, 0.0),
                Point::from_mm(3.0, 0.0),
            ],
            flow: Flow::new(0.45, 0.2),
            overhang: None,
            perimeter_index: None,
            seam: Some(Box::new(PathSeam {
                samples: vec![
                    ScarfSample {
                        height: 0.0,
                        extrusion: 0.0,
                    },
                    ScarfSample {
                        height: 0.5,
                        extrusion: 0.5,
                    },
                    ScarfSample {
                        height: 1.0,
                        extrusion: 1.0,
                    },
                    ScarfSample {
                        height: 1.0,
                        extrusion: 0.5,
                    },
                ],
                wipe: None,
                original_clockwise: false,
                loop_length_mm: None,
                ..Default::default()
            })),
            brick: None,
        };
        let mut w = Writer::new();
        w.travel_to(0.0, 0.0, 100.0);
        w.move_z(start_z(&path, &layer), 12.0);
        assert!(extrude(&mut w, &path, &layer, 50.0, 0.1, 12.0));
        assert!(w.as_str().contains("G1 X1 Y0 Z.3 E.05"));
        assert!(w.as_str().contains("G1 X2 Y0 Z.4 E.1"));
        assert!(w.as_str().contains("G1 X3 Y0 Z.4 E.05"));
        assert!((w.stats.filament_mm - 0.2).abs() < 1e-12);
        assert_eq!(w.z(), 0.4);
    }
    #[test]
    fn inward_wipe_is_a_travel_without_retraction_or_extrusion() {
        let path = ExtrusionPath {
            role: Role::ExternalPerimeter,
            points: Vec::new(),
            flow: Flow::new(0.45, 0.2),
            overhang: None,
            perimeter_index: None,
            seam: Some(Box::new(PathSeam {
                wipe: Some(Point::from_mm(2.0, 3.0)),
                ..PathSeam::default()
            })),
            brick: None,
        };
        let mut w = Writer::new();
        wipe(&mut w, &path, 100.0);
        assert_eq!(w.as_str(), ";move inwards before travel\nG1 X2 Y3 F6000\n");
        assert_eq!(w.stats.filament_mm, 0.0);
    }
}
