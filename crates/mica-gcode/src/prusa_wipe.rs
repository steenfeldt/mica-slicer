// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/Wipe.cpp :: Wipe::init, Wipe::set_path, Wipe::wipe
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/Wipe.hpp :: Wipe::calc_wipe_speed, Wipe::calc_xy_to_e_ratio
// Source: prusaslicer@b028299c77 src/libslic3r/GCode.cpp :: GCodeGenerator::retract_and_wipe, GCodeGenerator::extrude_smooth_path
// Source: prusaslicer@b028299c77 src/libslic3r/Geometry/ArcWelder.hpp :: arc_center, arc_angle
// Source: prusaslicer@b028299c77 src/libslic3r/Geometry/ArcWelder.cpp :: reverse
// Changes: Rust port for the Prusa (non-Orca) motion path. The wipe source is the smoothed geometry
// Changes: of the last extruded entity; Mica emits one upstream entity as consecutive contiguous paths
// Changes: (e.g. a loop split at overhangs), so pieces are collected until a travel. Planning is a pure
// Changes: function so the cooling estimate and the layer-change ramp see the same wipe as the writer.
// Changes: Single extruder per call (the active tool's settings).
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/Wipe.hpp :: Wipe::set_path
// Changes: Toolchange wipes (retract_and_wipe(true)) use the toolchange length; the wipe tower's
// Changes: wipe_path is a plain forward polyline.

//! Retract-and-wipe: retract part of the filament, then move back along the last extrusion while
//! retracting the rest, so ooze is wiped onto the printed line instead of strung across a travel.

use crate::writer::{ArcCommand, Writer};
use mica_core::{
    config::Config,
    geometry::{SCALE, arc_welder::Segment},
    settings::PrintSettings,
    toolpath::ExtrusionPath,
};

/// libslic3r `EPSILON`.
const EPSILON: f64 = 1e-4;

fn quantize_xy(v: f64) -> f64 {
    // Same rounding as the writer's coordinates (smooth_path::quantize), so equal points compare equal.
    (v * 1000.0).round() / 1000.0
}
fn quantize_e(v: f64) -> f64 {
    (v * 100000.0).round() * (1.0 / 100000.0)
}
fn quantize(p: (f64, f64)) -> (f64, f64) {
    (quantize_xy(p.0), quantize_xy(p.1))
}

/// One smoothed path point in G-code millimetres; `radius` (signed, mm) and `ccw` describe the arc
/// ending at this point, zero radius is a line.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct WipePoint {
    pub xy: (f64, f64),
    pub radius: f64,
    pub ccw: bool,
}

/// A planned wipe: the emitted moves and where it ends.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct WipePlan {
    /// Retraction before the wipe (quantized, may be zero).
    pub before: f64,
    pub moves: Vec<WipeMove>,
    /// Retraction after the wipe to reach the full length (quantized, may be zero).
    pub after: f64,
    pub speed: f64,
    /// A wipe was started (`;WIPE_START`), possibly without any move after quantization.
    pub started: bool,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct WipeMove {
    pub xy: (f64, f64),
    pub ij: Option<((f64, f64), bool)>,
    pub e: f64,
    pub length: f64,
}
impl WipePlan {
    pub fn end(&self) -> Option<(f64, f64)> {
        self.moves.last().map(|m| m.xy)
    }
    /// Non-adjustable time of the wipe moves (CoolingBuffer counts them as ordinary motion).
    pub fn duration(&self, retract_speed: f64) -> f64 {
        self.moves.iter().map(|m| m.length).sum::<f64>() / self.speed
            + (self.before + self.after) / retract_speed
    }
}

/// `Wipe` state of GCodeGenerator for the Prusa motion path.
#[derive(Default)]
pub(crate) struct Wipe {
    /// Pieces of the last extruded entity: smoothed geometry and bridge flag.
    pieces: Vec<(Vec<WipePoint>, bool)>,
    is_loop: bool,
}

/// Per-extruder wipe parameters (`Wipe::init`, `calc_wipe_speed`, `calc_xy_to_e_ratio`).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Params {
    pub speed: f64,
    pub xy_to_e: f64,
    pub before_wipe: f64,
    pub len_max: f64,
}
fn filament_number(c: &Config, key: &str) -> Option<f64> {
    c.float(&format!("filament_{key}")).or_else(|| c.float(key))
}
impl Params {
    /// `None` when wiping is disabled for this extruder (or would have no length).
    pub fn new(c: &Config, s: &PrintSettings) -> Option<Self> {
        if !s.seam.wipe || crate::orca_motion::enabled(c) {
            return None;
        }
        let speed = s.travel_speed * 0.8;
        let xy_to_e = 0.95 * (s.retract_speed + 0.5).floor() / speed;
        let len_max = s.retract_length / xy_to_e;
        (speed > 0.0 && len_max > 0.0 && len_max.is_finite()).then_some(Self {
            speed,
            xy_to_e,
            before_wipe: (filament_number(c, "retract_before_wipe").unwrap_or(0.0) / 100.0)
                .clamp(0.0, 1.0),
            len_max,
        })
    }
    /// `Wipe::init` for a multi-tool job also accumulates paths long enough for a toolchange
    /// retraction (`retract_length_toolchange`).
    pub fn with_toolchange_length(mut self, length: f64) -> Self {
        self.len_max = self.len_max.max(length / self.xy_to_e);
        self
    }
}

fn arc_center(a: (f64, f64), b: (f64, f64), radius: f64, ccw: bool) -> (f64, f64) {
    let v = (b.0 - a.0, b.1 - a.1);
    let q2 = v.0 * v.0 + v.1 * v.1;
    let t2 = radius * radius / q2 - 0.25;
    let t = if t2 > 0.0 { t2.sqrt() } else { 0.0 };
    let mid = (0.5 * (a.0 + b.0), 0.5 * (a.1 + b.1));
    let vp = (-v.1 * t, v.0 * t);
    if (radius > 0.0) == ccw {
        (mid.0 + vp.0, mid.1 + vp.1)
    } else {
        (mid.0 - vp.0, mid.1 - vp.1)
    }
}
fn arc_angle(a: (f64, f64), b: (f64, f64), radius: f64) -> f32 {
    let a = 0.5 * (b.0 - a.0).hypot(b.1 - a.1) / radius;
    // The upstream wipe stores the angle as float.
    (if radius > 0.0 {
        if a > 1.0 {
            std::f64::consts::PI
        } else {
            2.0 * a.asin()
        }
    } else if a < -1.0 {
        std::f64::consts::PI
    } else {
        2.0 * std::f64::consts::PI + 2.0 * a.asin()
    }) as f32
}

/// The running state of `Wipe::wipe`'s `wipe_linear` / `wipe_arc` lambdas.
struct Cx<'a> {
    xy_to_e: f64,
    to_go: f64,
    moves: &'a mut Vec<WipeMove>,
    started: bool,
}
impl Cx<'_> {
    fn linear(&mut self, prev: (f64, f64), pt: &mut (f64, f64)) -> bool {
        let q = quantize(*pt);
        if q == prev {
            *pt = q;
            return false;
        }
        let segment = (q.0 - prev.0).hypot(q.1 - prev.1);
        // Quantize E as it is extruded as a whole segment.
        let mut de = quantize_e(self.xy_to_e * segment);
        let mut done = false;
        if de > self.to_go - EPSILON {
            *pt = if de > self.to_go + EPSILON {
                // Shorten the segment.
                let t = self.to_go / de;
                quantize((prev.0 + (pt.0 - prev.0) * t, prev.1 + (pt.1 - prev.1) * t))
            } else {
                q
            };
            de = self.to_go;
            done = true;
        } else {
            *pt = q;
        }
        self.moves.push(WipeMove {
            xy: *pt,
            ij: None,
            e: de,
            length: (pt.0 - prev.0).hypot(pt.1 - prev.1),
        });
        self.to_go -= de;
        done
    }
    fn arc(&mut self, prev: (f64, f64), pt: &mut (f64, f64), radius: f64, ccw: bool) -> bool {
        let q = quantize(*pt);
        if q == prev {
            *pt = q;
            return false;
        }
        if radius == 0.0 {
            // Degenerated arc after quantization: a line segment.
            return self.linear(prev, pt);
        }
        let mut center = arc_center(prev, q, radius, ccw);
        let mut angle = arc_angle(prev, q, radius);
        let segment = f64::from(angle) * radius.abs();
        let mut de = quantize_e(self.xy_to_e * segment);
        let mut done = false;
        if de > self.to_go - EPSILON {
            if de > self.to_go + EPSILON {
                // Shorten the segment; recalculate the arc from the unquantized end coordinate.
                center = arc_center(prev, *pt, radius, ccw);
                angle = arc_angle(prev, *pt, radius);
                de = self.xy_to_e * f64::from(angle) * radius.abs();
                let a = f64::from(if ccw { angle } else { -angle }) * (self.to_go / de);
                let (s, c) = a.sin_cos();
                let d = (prev.0 - center.0, prev.1 - center.1);
                *pt = quantize((center.0 + c * d.0 - s * d.1, center.1 + s * d.0 + c * d.1));
            } else {
                *pt = q;
            }
            de = self.to_go;
            done = true;
        } else {
            *pt = q;
        }
        // Quantized IJ circle centre offset.
        let ij = quantize((center.0 - prev.0, center.1 - prev.1));
        if ij == (0.0, 0.0) {
            return self.linear(prev, pt);
        }
        let r = (prev.0 - center.0).hypot(prev.1 - center.1);
        self.moves.push(WipeMove {
            xy: *pt,
            ij: Some((ij, ccw)),
            e: de,
            length: f64::from(angle) * r,
        });
        self.to_go -= de;
        done
    }
}

fn smoothed(curve: &[Segment]) -> Vec<WipePoint> {
    curve
        .iter()
        .map(|s| WipePoint {
            xy: (s.point.x as f64 / SCALE, s.point.y as f64 / SCALE),
            radius: s.radius / SCALE,
            ccw: s.ccw,
        })
        .collect()
}
/// `ArcWelder::reverse`: each arc's radius moves to its start point, with the orientation flipped.
fn reversed(mut path: Vec<WipePoint>) -> Vec<WipePoint> {
    for i in 1..path.len() {
        path[i - 1].radius = path[i].radius;
        path[i - 1].ccw = !path[i].ccw;
    }
    if let Some(last) = path.last_mut() {
        last.radius = 0.0;
    }
    path.reverse();
    path
}
fn length(path: &[WipePoint]) -> f64 {
    path.windows(2)
        .map(|w| {
            let chord = (w[1].xy.0 - w[0].xy.0).hypot(w[1].xy.1 - w[0].xy.1);
            if w[1].radius == 0.0 {
                chord
            } else {
                f64::from(arc_angle(w[0].xy, w[1].xy, w[1].radius)) * w[1].radius.abs()
            }
        })
        .sum()
}

impl Wipe {
    pub fn reset(&mut self) {
        self.pieces.clear();
    }
    pub fn has_path(&self) -> bool {
        !self.pieces.is_empty()
    }
    /// `Wipe::set_path(Path&&)` with a plain polyline (the wipe tower's `wipe_path`): wiped
    /// forward from its first point.
    pub fn set_path(&mut self, points: &[[f64; 2]]) {
        self.pieces.clear();
        if points.len() > 1 {
            self.is_loop = true;
            self.pieces.push((
                points
                    .iter()
                    .map(|p| WipePoint {
                        xy: (p[0], p[1]),
                        radius: 0.,
                        ccw: false,
                    })
                    .collect(),
                false,
            ));
        }
    }

    /// `extrude_smooth_path` → `set_path`: remember a path that was just extruded. A path that starts
    /// exactly where the previous one ended continues the same entity.
    pub fn extruded(&mut self, path: &ExtrusionPath, curves: Vec<Vec<Segment>>) {
        let mut geometry: Vec<WipePoint> = Vec::new();
        for curve in curves {
            let points = smoothed(&curve);
            if geometry
                .last()
                .is_some_and(|l| points.first().is_some_and(|p| p.xy == l.xy))
            {
                geometry.extend_from_slice(&points[1..]);
            } else {
                geometry.extend(points);
            }
        }
        if geometry.len() < 2 {
            return;
        }
        let continues = self
            .pieces
            .last()
            .and_then(|(p, _)| p.last())
            .is_some_and(|l| l.xy == geometry[0].xy);
        if !continues {
            self.pieces.clear();
            self.is_loop = path.seam.is_some();
        }
        self.pieces.push((geometry, path.role.is_bridge()));
    }

    /// `Wipe::set_path` on the remembered entity: loops wipe forward from their start, open paths
    /// backwards; whole pieces are appended until `len_max`, stopping at bridges and gaps.
    fn wipe_path(&self, len_max: f64) -> Vec<WipePoint> {
        let pieces: Vec<Vec<WipePoint>> = if self.is_loop {
            self.pieces.iter().map(|(p, _)| p.clone()).collect()
        } else {
            self.pieces
                .iter()
                .rev()
                .map(|(p, _)| reversed(p.clone()))
                .collect()
        };
        let bridges: Vec<bool> = if self.is_loop {
            self.pieces.iter().map(|p| p.1).collect()
        } else {
            self.pieces.iter().rev().map(|p| p.1).collect()
        };
        let mut out = pieces[0].clone();
        let mut len = length(&out);
        for (piece, bridge) in pieces.iter().zip(bridges).skip(1) {
            if len >= len_max || bridge || out.last().map(|p| p.xy) != piece.first().map(|p| p.xy) {
                break;
            }
            len += length(piece);
            out.extend_from_slice(&piece[1..]);
        }
        out
    }

    /// `retract_and_wipe` for retraction state `retracted` (mm) at writer position `from`.
    pub fn plan(
        &self,
        from: (f64, f64),
        retracted: f64,
        retract_length: f64,
        p: &Params,
    ) -> WipePlan {
        let mut plan = WipePlan {
            speed: p.speed,
            ..Default::default()
        };
        let mut retracted = retracted;
        if self.has_path() {
            plan.before = quantize_e(p.before_wipe * retract_length - retracted).max(0.0);
            retracted += plan.before;
            let mut to_go = quantize_e(retract_length - retracted).max(0.0);
            if to_go > 0.0 {
                let path = self.wipe_path(p.len_max);
                let mut prev = quantize(from);
                let mut cx = Cx {
                    xy_to_e: p.xy_to_e,
                    to_go,
                    moves: &mut plan.moves,
                    started: false,
                };
                // Start at the current position (the loop may have been clipped before its start).
                let mut pt = path[0].xy;
                let mut done = false;
                if pt != prev {
                    cx.started = true;
                    done = cx.linear(prev, &mut pt);
                }
                if !done {
                    prev = pt;
                    for point in &path[1..] {
                        let mut pt = point.xy;
                        if pt != prev {
                            cx.started = true;
                            if cx.arc(prev, &mut pt, point.radius, point.ccw) {
                                break;
                            }
                            prev = pt;
                        }
                    }
                }
                to_go = cx.to_go;
                plan.started = cx.started;
                let _ = to_go;
                for m in &plan.moves {
                    retracted += m.e;
                }
            }
        }
        plan.after = quantize_e(retract_length - retracted).max(0.0);
        plan
    }
}

/// Emit a planned retract-and-wipe.
pub(crate) fn emit(w: &mut Writer, plan: &WipePlan, retract_speed: f64) {
    w.retract_top_up(plan.before, retract_speed);
    if plan.started {
        w.begin_wipe();
        for m in &plan.moves {
            match m.ij {
                None => w.wipe_to(m.xy.0, m.xy.1, m.e, plan.speed),
                Some((ij, ccw)) => w.wipe_arc_to(
                    m.xy,
                    ArcCommand::Center { ij, ccw },
                    m.e,
                    plan.speed,
                    m.length,
                ),
            }
        }
        w.raw(";WIPE_END");
    }
    w.retract_top_up(plan.after, retract_speed);
}

#[cfg(test)]
mod tests {
    use super::*;
    use mica_core::geometry::Point;

    fn params() -> Params {
        // XL: retract 0.8 mm at 35 mm/s, travel 400 mm/s, 80 % before wipe.
        let speed = 400.0 * 0.8;
        let xy_to_e = 0.95 * 35.0 / speed;
        Params {
            speed,
            xy_to_e,
            before_wipe: 0.8,
            len_max: 0.8 / xy_to_e,
        }
    }

    #[test]
    fn loop_wipe_matches_prusaslicer_xl_cube() {
        // PrusaSlicer 2.9.6, XL cube20 layer 3 external perimeter, seam at (189.775, 189.775):
        // G1 E-.64 / G1 X189.775 Y189.775 E-.00623 / G1 X188.295 Y189.775 E-.15377
        let loop_points = [
            (189.775, 189.775),
            (170.225, 189.775),
            (170.225, 170.225),
            (189.775, 170.225),
            (189.775, 189.715),
        ];
        let path = ExtrusionPath {
            role: mica_core::toolpath::Role::ExternalPerimeter,
            perimeter_index: Some(0),
            points: loop_points
                .iter()
                .map(|p| Point::from_mm(p.0, p.1))
                .collect(),
            flow: mica_core::flow::Flow::new(0.45, 0.2),
            seam: Some(Box::default()),
            overhang: None,
            brick: None,
        };
        let mut wipe = Wipe::default();
        let curve = path.points.iter().map(|&p| Segment::line(p)).collect();
        wipe.extruded(&path, vec![curve]);
        let plan = wipe.plan((189.775, 189.715), 0.0, 0.8, &params());
        assert_eq!(plan.before, 0.64);
        assert_eq!(plan.moves.len(), 2);
        assert_eq!(plan.moves[0].xy, (189.775, 189.775));
        assert!((plan.moves[0].e - 0.00623).abs() < 1e-12);
        assert_eq!(plan.moves[1].xy, (188.295, 189.775));
        assert!((plan.moves[1].e - 0.15377).abs() < 1e-12);
        assert_eq!(plan.after, 0.0);
    }

    #[test]
    fn open_path_wipes_backwards_and_short_paths_top_up() {
        let path = ExtrusionPath {
            role: mica_core::toolpath::Role::InternalInfill,
            perimeter_index: None,
            points: vec![Point::from_mm(0., 0.), Point::from_mm(0.5, 0.)],
            flow: mica_core::flow::Flow::new(0.45, 0.2),
            seam: None,
            overhang: None,
            brick: None,
        };
        let mut wipe = Wipe::default();
        wipe.extruded(
            &path,
            vec![path.points.iter().map(|&p| Segment::line(p)).collect()],
        );
        let plan = wipe.plan((0.5, 0.), 0.0, 0.8, &params());
        assert_eq!(plan.moves.len(), 1);
        assert_eq!(plan.moves[0].xy, (0., 0.));
        // 0.5 mm of wipe retracts 0.05195 mm; the rest is retracted in place afterwards.
        assert!((plan.moves[0].e - 0.05195).abs() < 1e-12);
        assert!((plan.after - (0.16 - 0.05195)).abs() < 1e-9);
        // Already fully retracted: nothing to do.
        let plan = wipe.plan((0.5, 0.), 0.8, 0.8, &params());
        assert_eq!(
            plan,
            WipePlan {
                speed: plan.speed,
                ..Default::default()
            }
        );
    }
}
