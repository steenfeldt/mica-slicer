// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/SmoothPath.cpp :: SmoothPathCache::interpolate_add, resolve_or_fit, resolve_or_fit_split_with_seam
// Source: prusaslicer@b028299c77 src/libslic3r/GCode.cpp :: arc_welder_enabled, interpolation_parameters, GCodeGenerator::_extrude
// Source: orca@8500fcdcca src/libslic3r/GCode.cpp :: GCode::_extrude
// Changes: Bounded preformatted motion with original-order writer accounting; layer-local indexed cache; single-attribute paths already ordered/seamed by Mica; radius mode extension.
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/CoolingBuffer.cpp :: CoolingBuffer::apply_layer_cooldown, split_linear_segment, emit_extrusion
// Changes: Cooling tail splits re-read the emitted lines as floats and interpolate the crossing line like CoolingBuffer.

use crate::writer::{ArcCommand, Writer};
use mica_core::{
    config::Config,
    geometry::{
        Point, SCALE,
        arc_welder::{self as arc, Path},
    },
    toolpath::{ExtrusionPath, Role},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArcFitting {
    Disabled,
    EmitCenter,
    EmitRadius,
}
impl ArcFitting {
    pub fn from_config(c: &Config) -> Self {
        if c.bool("spiral_vase").unwrap_or(false)
            || c.float("max_volumetric_extrusion_rate_slope_positive")
                .unwrap_or(0.)
                != 0.
            || c.float("max_volumetric_extrusion_rate_slope_negative")
                .unwrap_or(0.)
                != 0.
        {
            return Self::Disabled;
        }
        match c.string("arc_fitting") {
            Some("emit_center") => Self::EmitCenter,
            Some("emit_radius") if c.string("gcode_flavor") != Some("klipper") => Self::EmitRadius,
            // Orca/Klipper arcs use I/J: Klipper's G2/G3 parser does not accept R.
            Some("emit_radius") => Self::EmitCenter,
            _ => Self::Disabled,
        }
    }
}

/// Owns only one layer's fits. Keys are indices in that layer, never raw pointers across batches.
pub struct SmoothPathCache {
    paths: Vec<Path>,
}
impl SmoothPathCache {
    pub fn new(paths: &[ExtrusionPath], tolerance: f64, mode: ArcFitting) -> Self {
        let length_tolerance = if mode == ArcFitting::Disabled {
            0.
        } else {
            arc::LENGTH_TOLERANCE
        };
        Self {
            paths: paths
                .iter()
                .map(|p| {
                    arc::fit_path(
                        &p.points,
                        tolerance
                            * if p.role == Role::InternalInfill {
                                3.
                            } else {
                                1.
                            },
                        length_tolerance,
                    )
                })
                .collect(),
        }
    }
    pub fn get(&self, index: usize) -> &Path {
        &self.paths[index]
    }
    pub fn resolve_or_fit(
        &self,
        index: usize,
        source: &[Point],
        reverse: bool,
        tolerance: f64,
    ) -> Path {
        let mut path = self
            .paths
            .get(index)
            .cloned()
            .unwrap_or_else(|| arc::fit_path(source, tolerance, 0.));
        if reverse {
            arc::reverse(&mut path);
        }
        path
    }
    /// Rotate a closed, single-attribute path to the projected seam, retaining arcs through the split.
    pub fn resolve_or_fit_split_with_seam(
        &self,
        index: usize,
        source: &[Point],
        reverse: bool,
        tolerance: f64,
        seam: (Point, f64),
    ) -> Path {
        let path = self.resolve_or_fit(index, source, reverse, tolerance);
        let (left, mut right) = arc::split_at(&path, seam.0, seam.1);
        if right.is_empty() {
            return left;
        }
        right.extend(left.into_iter().skip(1));
        right
    }
}

fn quantize(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}
/// Compute extrusion from analytic length after endpoint quantization, like Prusa's exporter.
pub fn emit(w: &mut Writer, path: &[arc::Segment], e_per_mm: f64, speed: f64, mode: ArcFitting) {
    for_each_move(path, e_per_mm, mode, |m| {
        if let Some(command) = m.command {
            w.extrude_to_xy_g2g3(m.xy, command, m.e, speed, m.length);
        } else {
            w.extrude_to(m.xy.0, m.xy.1, m.e, speed);
        }
    });
}

/// `CoolingBuffer::apply_layer_cooldown` splitting a slowed extrusion so that its last
/// `tail` mm keep `original` speed. CoolingBuffer re-parses the emitted lines as floats and splits
/// the crossing segment by float interpolation (`split_linear_segment`); emitting the same text keeps
/// the output identical. Returns false (nothing written) when the path contains arcs.
pub(crate) fn emit_split_tail(
    w: &mut Writer,
    path: &[arc::Segment],
    e_per_mm: f64,
    speed: f64,
    original: f64,
    tail: f64,
    mode: ArcFitting,
) -> bool {
    let mut moves = Vec::new();
    for_each_move(path, e_per_mm, mode, |m| moves.push(m));
    let Some((x0, y0)) = w.position() else {
        return false;
    };
    if moves.iter().any(|m| m.command.is_some()) || w.tool_offset() != (0.0, 0.0) {
        return false;
    }
    let parse = |v: f64, d: usize| crate::writer::num(v, d).parse::<f32>().unwrap_or(v as f32);
    // Float positions as CoolingBuffer reads them, and each line's float length.
    let mut start = [parse(x0, 3), parse(y0, 3)];
    let mut lines = Vec::with_capacity(moves.len());
    for m in &moves {
        let end = [parse(m.xy.0, 3), parse(m.xy.1, 3)];
        let (dx, dy) = (end[0] - start[0], end[1] - start[1]);
        lines.push((start, end, parse(m.e, 5), (dx * dx + dy * dy).sqrt()));
        start = end;
    }
    let total: f32 = lines.iter().fold(0.0f32, |sum, l| sum + l.3);
    let split = total - tail as f32;
    const EPS: f32 = 10.0 * 0.001;
    // The machine Z (with z_offset), as CoolingBuffer reads it from the text.
    let z = parse(w.z() + w.z_offset(), 3);
    let mut acc = 0.0f32;
    let mut i = 0;
    while i < lines.len() && acc + lines[i].3 <= split {
        w.extrude_to(moves[i].xy.0, moves[i].xy.1, moves[i].e, speed);
        acc += lines[i].3;
        i += 1;
    }
    if i < lines.len() {
        let (a, b, e, len) = lines[i];
        let at = split - acc;
        if at >= len - EPS {
            w.extrude_to(moves[i].xy.0, moves[i].xy.1, moves[i].e, speed);
            i += 1;
        } else if at <= EPS {
        } else {
            let t = (at / len).clamp(0.0, 1.0);
            let lerp = |p: f32, q: f32| (1.0 - t) * p + t * q;
            let mid = [lerp(a[0], b[0]), lerp(a[1], b[1])];
            // A level segment keeps its Z: PrusaSlicer's lerp of two equal floats (fused into
            // one multiply-add by the compiler) gives the same float back, so CoolingBuffer
            // writes no Z word; the unfused product may not (e.g. 1.1 with a 0.5 mm z_offset).
            let zm = lerp(z, z);
            let em = lerp(0.0, e);
            w.extrude_split_part(a, mid, z, zm, em, speed);
            w.feedrate_line(original);
            w.extrude_split_part(mid, b, zm, z, e - em, original);
            w.set_position(moves[i].xy);
            i += 1;
        }
    }
    for m in &moves[i..] {
        w.extrude_to(m.xy.0, m.xy.1, m.e, original);
    }
    true
}

pub(crate) struct Motion {
    pub xy: (f64, f64),
    pub command: Option<ArcCommand>,
    pub e: f64,
    pub length: f64,
}

/// Independent text and motion data. Feedrate and writer accounting stay ordered.
pub(crate) struct Prepared {
    text: String,
    moves: Vec<Motion>,
    offset: (f64, f64),
    e_per_mm: f64,
    mode: ArcFitting,
}
impl Prepared {
    pub fn new(path: &[arc::Segment], e_per_mm: f64, mode: ArcFitting, offset: (f64, f64)) -> Self {
        let mut text = String::new();
        let mut moves = Vec::with_capacity(path.len().saturating_sub(1));
        for_each_move(path, e_per_mm, mode, |m| {
            crate::writer::extrusion_line(&mut text, m.xy, m.command, m.e, offset);
            moves.push(m);
        });
        Self {
            text,
            moves,
            offset,
            e_per_mm,
            mode,
        }
    }
    pub fn emit(&self, w: &mut Writer, speed: f64, e_per_mm: f64, mode: ArcFitting) -> bool {
        self.e_per_mm.to_bits() == e_per_mm.to_bits()
            && self.mode == mode
            && w.prepared_extrusions(&self.moves, &self.text, self.offset, speed)
    }
}

fn for_each_move(
    path: &[arc::Segment],
    e_per_mm: f64,
    mode: ArcFitting,
    mut emit: impl FnMut(Motion),
) {
    let Some(first) = path.first() else {
        return;
    };
    let mut prev_exact = first.point;
    let mut prev = Point::from_mm(
        quantize(prev_exact.x as f64 / SCALE),
        quantize(prev_exact.y as f64 / SCALE),
    );
    for end in &path[1..] {
        let xy = (
            quantize(end.point.x as f64 / SCALE),
            quantize(end.point.y as f64 / SCALE),
        );
        let p = Point::from_mm(xy.0, xy.1);
        if p == prev {
            continue;
        }
        let mut command = None;
        if end.radius != 0.0 && mode != ArcFitting::Disabled {
            let c = arc::arc_center(prev_exact, end.point, end.radius, end.ccw);
            let ij = (
                quantize((c.0 - prev.x as f64) / SCALE),
                quantize((c.1 - prev.y as f64) / SCALE),
            );
            if ij != (0., 0.) {
                command = Some(if mode == ArcFitting::EmitRadius {
                    ArcCommand::Radius {
                        radius: end.radius.signum()
                            * ((end.radius.abs() / SCALE * 1e5).round() / 1e5)
                                .max((0.5 * prev.distance_to(p) / SCALE * 1e5).ceil() / 1e5),
                        ccw: end.ccw,
                    }
                } else {
                    ArcCommand::Center { ij, ccw: end.ccw }
                });
            }
        }
        if let Some(command) = command {
            let radius = match command {
                ArcCommand::Radius { radius, .. } => radius * SCALE,
                _ => end.radius,
            };
            let length = arc::arc_angle(prev, p, radius) * radius.abs() / SCALE;
            emit(Motion {
                xy,
                command: Some(command),
                e: length * e_per_mm,
                length,
            });
        } else {
            let length = prev.distance_to(p) / SCALE;
            emit(Motion {
                xy,
                command: None,
                e: length * e_per_mm,
                length,
            });
        }
        prev = p;
        prev_exact = end.point;
    }
}
