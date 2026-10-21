// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/GCodeProcessor.cpp :: GCodeProcessor::process_G1
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/GCodeProcessor.cpp :: TimeMachine::calculate_time
// Changes: Junction speed (classic per-axis jerk, or junction deviation when the printer sets one / Klipper's square corner velocity) and the forward / backward trapezoid passes re-implemented over one preview layer's records instead of a streamed G-code block queue; E axis, minimum segment time and the 64-block lookahead window are not modelled; the result is the mean speed of each record (length / trapezoid time). Record packing, temperature, emitted acceleration and volumetric flow per record are original.

//! Extra per-record attributes for the preview colour modes that the 32-byte record cannot hold:
//! simulated actual speed, volumetric flow, nozzle temperature and the emitted acceleration.
//! They live in `attrs.bin` next to `preview.bin`, one 8-byte [`Attr`] per record in the same
//! order, and are read with the records batch by batch (never kept whole in CPU memory; the
//! renderer holds both on the GPU, `render::toolpaths`), so switching colour modes needs no
//! re-slice or re-parse.

use super::{Segment, TOWER, TRAVEL};
use bytemuck::{Pod, Zeroable};
use mica_core::{config::Config, settings::PrintSettings, toolpath::Role};

/// 8 bytes per record. Fixed-point: `actual` 0.1 mm/s, `flow` 0.01 mm³/s, `temp` 0.1 °C,
/// `accel` mm/s² (0 = the printer gets no acceleration commands).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Pod, Zeroable)]
pub struct Attr {
    pub actual: u16,
    pub flow: u16,
    pub temp: u16,
    pub accel: u16,
}
impl Attr {
    pub fn new(actual: f64, flow: f64, temp: f64, accel: f64) -> Self {
        let q = |v: f64, scale: f64| (v * scale).round().clamp(0., 65535.) as u16;
        Self {
            actual: q(actual, 10.),
            flow: q(flow, 100.),
            temp: q(temp, 10.),
            accel: q(accel, 1.),
        }
    }
    pub fn actual(&self) -> f32 {
        self.actual as f32 / 10.
    }
    pub fn flow(&self) -> f32 {
        self.flow as f32 / 100.
    }
    pub fn temp(&self) -> f32 {
        self.temp as f32 / 10.
    }
    pub fn accel(&self) -> f32 {
        self.accel as f32
    }
}
pub const SIZE: usize = std::mem::size_of::<Attr>();

/// `Role as u32` back to the role (records 14/15 are travel and tower).
pub fn role_of(index: u32) -> Option<Role> {
    use Role::*;
    Some(match index {
        0 => Skirt,
        1 => Brim,
        2 => ExternalPerimeter,
        3 => OverhangPerimeter,
        4 => Perimeter,
        5 => InternalInfill,
        6 => SolidInfill,
        7 => BridgeInfill,
        8 => InternalBridgeInfill,
        9 => TopSolidInfill,
        10 => Ironing,
        11 => GapFill,
        12 => SupportMaterial,
        13 => SupportMaterialInterface,
        _ => return None,
    })
}

/// Cross-section area × speed: what the hot end melts, in mm³/s (the rounded-rectangle model of
/// `mica_core::flow::Flow::mm3_per_mm`). Travel has none.
pub fn volumetric(s: &Segment) -> f64 {
    if s.role() == TRAVEL {
        return 0.;
    }
    let w = (s.size & 65535) as f64 / 1000.;
    let h = (s.size >> 16) as f64 / 1000.;
    let area = (h * (w - h * (1. - std::f64::consts::FRAC_PI_4))).max(0.);
    area * s.speed() as f64
}

/// Machine limits for the actual-speed simulation, from the printer profile.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Machine {
    /// Classic jerk per axis (X, Y, Z), mm/s.
    pub jerk: [f64; 3],
    /// Junction deviation (mm); replaces jerk when positive.
    pub junction_deviation: f64,
    /// Firmware caps on printing / travel acceleration (0 = none).
    pub max_extruding: f64,
    pub max_travel: f64,
    /// The acceleration the printer runs at when the G-code sets none.
    pub fallback: f64,
    /// Square corner velocity (Klipper), turned into a junction deviation per acceleration.
    pub square_corner_velocity: f64,
}
impl Default for Machine {
    fn default() -> Self {
        Self {
            jerk: [10., 10., 0.4],
            junction_deviation: 0.,
            max_extruding: 0.,
            max_travel: 0.,
            fallback: 1500.,
            square_corner_velocity: 0.,
        }
    }
}
impl Machine {
    pub fn from_config(config: &Config) -> Self {
        let positive = |key: &str| config.float(key).filter(|v| v.is_finite() && *v > 0.);
        let d = Self::default();
        let klipper = config.string("gcode_flavor") == Some("klipper");
        let max_extruding = positive("machine_max_acceleration_extruding").unwrap_or(0.);
        Self {
            jerk: [
                positive("machine_max_jerk_x").unwrap_or(d.jerk[0]),
                positive("machine_max_jerk_y").unwrap_or(d.jerk[1]),
                positive("machine_max_jerk_z").unwrap_or(d.jerk[2]),
            ],
            junction_deviation: positive("machine_max_junction_deviation").unwrap_or(0.),
            max_extruding,
            max_travel: positive("machine_max_acceleration_travel").unwrap_or(0.),
            fallback: positive("machine_max_acceleration_x")
                .or(Some(max_extruding).filter(|v| *v > 0.))
                .unwrap_or(d.fallback),
            square_corner_velocity: if klipper {
                positive("machine_max_square_corner_velocity")
                    .or_else(|| positive("square_corner_velocity"))
                    .unwrap_or(5.)
            } else {
                0.
            },
        }
    }
}

/// The acceleration the exporter commands for a record (`M204` / `SET_VELOCITY_LIMIT`), or 0
/// when the profile emits none (`default_acceleration` 0). Mirrors `mica_gcode::export`.
pub fn emitted_acceleration(
    s: &PrintSettings,
    config: &Config,
    role: u32,
    first_layer: bool,
) -> f64 {
    if s.default_acceleration <= 0. {
        return 0.;
    }
    match role_of(role) {
        Some(r) => mica_gcode::export::role_acceleration(s, r, first_layer),
        // Travel and the tower: the travel acceleration, else the default.
        None if role == TRAVEL => config
            .float(if first_layer {
                "initial_layer_travel_acceleration"
            } else {
                "travel_acceleration"
            })
            .filter(|a| *a > 0.)
            .unwrap_or(s.default_acceleration),
        None => s.default_acceleration,
    }
}

/// One move for the planner.
#[derive(Clone, Copy, Debug)]
pub struct Move {
    pub a: [f32; 3],
    pub b: [f32; 3],
    /// Commanded speed, mm/s.
    pub feed: f64,
    /// Acceleration, mm/s² (0 = the machine's fallback).
    pub accel: f64,
    pub travel: bool,
}

fn length(m: &Move) -> f64 {
    let d = [
        (m.b[0] - m.a[0]) as f64,
        (m.b[1] - m.a[1]) as f64,
        (m.b[2] - m.a[2]) as f64,
    ];
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}

/// Speed reachable over `distance` from `v` with `accel` (`max_allowable_speed`).
fn reachable(accel: f64, v: f64, distance: f64) -> f64 {
    (v * v + 2. * accel * distance).sqrt()
}

/// Time of a trapezoid / triangle profile from `v0` to `v1` cruising at most at `vc`.
pub fn trapezoid_time(length: f64, v0: f64, v1: f64, vc: f64, accel: f64) -> f64 {
    if length <= 0. {
        return 0.;
    }
    let vc = vc.max(1e-3);
    let (v0, v1) = (v0.min(vc), v1.min(vc));
    if accel <= 0. {
        return length / vc;
    }
    let d_acc = (vc * vc - v0 * v0) / (2. * accel);
    let d_dec = (vc * vc - v1 * v1) / (2. * accel);
    if d_acc + d_dec <= length {
        (vc - v0) / accel + (vc - v1) / accel + (length - d_acc - d_dec) / vc
    } else {
        // Never reaches the cruise speed: peak where acceleration meets deceleration.
        let peak = ((2. * accel * length + v0 * v0 + v1 * v1) / 2.)
            .sqrt()
            .max(v0.max(v1));
        (peak - v0) / accel + (peak - v1) / accel
    }
}

/// Mean speed of every move (length / time) after acceleration and junction limits, with the
/// chain starting and ending at the junction-safe speed, as a firmware planner would run it.
pub fn plan(moves: &[Move], machine: &Machine) -> Vec<f64> {
    let n = moves.len();
    if n == 0 {
        return vec![];
    }
    let accel_of = |m: &Move| {
        let cap = if m.travel {
            machine.max_travel
        } else {
            machine.max_extruding
        };
        let a = if m.accel > 0. {
            m.accel
        } else {
            machine.fallback
        };
        if cap > 0. { a.min(cap) } else { a }.max(1.)
    };
    let lengths: Vec<f64> = moves.iter().map(length).collect();
    let dirs: Vec<[f64; 3]> = moves
        .iter()
        .zip(&lengths)
        .map(|(m, &l)| {
            if l > 0. {
                [
                    (m.b[0] - m.a[0]) as f64 / l,
                    (m.b[1] - m.a[1]) as f64 / l,
                    (m.b[2] - m.a[2]) as f64 / l,
                ]
            } else {
                [0.; 3]
            }
        })
        .collect();
    let feeds: Vec<f64> = moves.iter().map(|m| m.feed.max(0.1)).collect();
    // Safe (from-rest) speed of each move: no axis may jump by more than its jerk.
    let safe: Vec<f64> = (0..n)
        .map(|i| {
            let mut v = feeds[i];
            for (axis, jerk) in machine.jerk.iter().enumerate() {
                if (dirs[i][axis] * feeds[i]).abs() > *jerk {
                    v = v.min(*jerk);
                }
            }
            v
        })
        .collect();
    // Maximum junction speed entering each move.
    let mut max_entry = vec![0.; n];
    for i in 0..n {
        if i == 0 {
            max_entry[i] = safe[i];
            continue;
        }
        let (p, c) = (i - 1, i);
        let accel = accel_of(&moves[c]);
        let deviation = if machine.junction_deviation > 0. {
            machine.junction_deviation
        } else if machine.square_corner_velocity > 0. {
            let scv = machine.square_corner_velocity;
            scv * scv * (std::f64::consts::SQRT_2 - 1.) / accel
        } else {
            0.
        };
        let vmax = if deviation > 0. {
            let cos_theta =
                -(dirs[p][0] * dirs[c][0] + dirs[p][1] * dirs[c][1] + dirs[p][2] * dirs[c][2]);
            let limit = feeds[c].min(feeds[p]);
            if cos_theta <= -0.999999 {
                // Straight on.
                limit
            } else {
                let cos_theta = cos_theta.min(0.999999);
                let sin_half = (0.5 * (1. - cos_theta)).sqrt();
                let v2 = accel * deviation * sin_half / (1. - sin_half);
                v2.sqrt().min(limit)
            }
        } else {
            // Classic jerk: the shared junction speed that keeps each axis jump within its jerk.
            let prev_larger = feeds[p] > feeds[c];
            let factor = if prev_larger {
                feeds[c] / feeds[p]
            } else {
                feeds[p] / feeds[c]
            };
            let mut vmax = if prev_larger { feeds[c] } else { feeds[p] };
            let mut v_factor = 1.;
            let mut limited = false;
            for (axis, jerk) in machine.jerk.iter().enumerate() {
                let mut v_exit = dirs[p][axis] * feeds[p];
                let mut v_entry = dirs[c][axis] * feeds[c];
                if prev_larger {
                    v_exit *= factor;
                }
                if limited {
                    v_exit *= v_factor;
                    v_entry *= v_factor;
                }
                let j = if v_exit > v_entry {
                    if v_entry > 0. || v_exit < 0. {
                        v_exit - v_entry
                    } else {
                        v_exit.max(-v_entry)
                    }
                } else if v_entry < 0. || v_exit > 0. {
                    v_entry - v_exit
                } else {
                    (-v_exit).max(v_entry)
                };
                if j > *jerk {
                    v_factor *= jerk / j;
                    limited = true;
                }
            }
            if limited {
                vmax *= v_factor;
            }
            let threshold = vmax * 0.99;
            if safe[p] > threshold && safe[c] > threshold {
                vmax = safe[c];
            }
            vmax
        };
        max_entry[i] = vmax.min(feeds[c]);
    }
    // Backward pass: each entry must allow decelerating to the next entry within the move.
    let mut entry = max_entry.clone();
    let mut exit_last = safe[n - 1];
    for i in (0..n).rev() {
        let next = if i + 1 < n { entry[i + 1] } else { exit_last };
        entry[i] = entry[i].min(reachable(accel_of(&moves[i]), next, lengths[i]));
    }
    // Forward pass: and accelerating from the previous entry.
    for i in 1..n {
        let from = reachable(accel_of(&moves[i - 1]), entry[i - 1], lengths[i - 1]);
        entry[i] = entry[i].min(from);
    }
    exit_last = exit_last.min(reachable(
        accel_of(&moves[n - 1]),
        entry[n - 1],
        lengths[n - 1],
    ));
    (0..n)
        .map(|i| {
            let exit = if i + 1 < n { entry[i + 1] } else { exit_last };
            let t = trapezoid_time(lengths[i], entry[i], exit, feeds[i], accel_of(&moves[i]));
            if t > 0. { lengths[i] / t } else { feeds[i] }
        })
        .collect()
}

/// Attributes of one layer's (or tower visit's) records in order. `temps` gives each record's
/// nozzle temperature and `first_layer` selects first-layer accelerations.
pub fn compute(
    records: &[Segment],
    temps: &[f64],
    settings: &[PrintSettings],
    config: &Config,
    machine: &Machine,
    first_layer: bool,
) -> Vec<Attr> {
    let setting = |tool: usize| &settings[tool.min(settings.len().saturating_sub(1))];
    let accels: Vec<f64> = records
        .iter()
        .map(|r| {
            let role = if r.role() == TOWER {
                u32::MAX
            } else {
                r.role()
            };
            emitted_acceleration(setting(r.tool()), config, role, first_layer)
        })
        .collect();
    let moves: Vec<Move> = records
        .iter()
        .zip(&accels)
        .map(|(r, &accel)| Move {
            a: r.a,
            b: r.b,
            feed: r.speed() as f64,
            accel,
            travel: r.role() == TRAVEL,
        })
        .collect();
    let actual = plan(&moves, machine);
    records
        .iter()
        .enumerate()
        .map(|(i, r)| {
            Attr::new(
                actual[i],
                volumetric(r),
                temps.get(i).copied().unwrap_or(0.),
                accels[i],
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn mv(a: [f32; 2], b: [f32; 2], feed: f64) -> Move {
        Move {
            a: [a[0], a[1], 0.2],
            b: [b[0], b[1], 0.2],
            feed,
            accel: 1000.,
            travel: false,
        }
    }
    #[test]
    fn packing_round_trips() {
        let a = Attr::new(123.45, 15.678, 215., 4000.);
        assert_eq!(SIZE, 8);
        assert!((a.actual() - 123.4).abs() < 0.11);
        assert!((a.flow() - 15.68).abs() < 0.006);
        assert_eq!(a.temp(), 215.);
        assert_eq!(a.accel(), 4000.);
        // Out of range values saturate instead of wrapping.
        assert_eq!(Attr::new(1e9, -5., 0., 1e9).actual, 65535);
        assert_eq!(Attr::new(0., -5., 0., 0.).flow, 0);
    }
    #[test]
    fn trapezoid_and_triangle_times() {
        // 100 mm at 100 mm/s from and to rest with 1000 mm/s²: 5 mm up, 5 mm down, 90 mm cruise.
        let t = trapezoid_time(100., 0., 0., 100., 1000.);
        assert!((t - (0.1 + 0.1 + 0.9)).abs() < 1e-9, "{t}");
        // 2 mm never reaches 100 mm/s: a triangle peaking at sqrt(2000) mm/s.
        let t = trapezoid_time(2., 0., 0., 100., 1000.);
        let peak = 2000f64.sqrt();
        assert!((t - 2. * peak / 1000.).abs() < 1e-9);
        // Constant speed without acceleration.
        assert!((trapezoid_time(10., 50., 50., 50., 1000.) - 0.2).abs() < 1e-9);
    }
    #[test]
    fn long_straight_moves_reach_their_speed_short_zigzags_do_not() {
        let m = Machine {
            jerk: [8., 8., 0.4],
            ..Default::default()
        };
        // A long straight line split in two: nearly the commanded speed throughout.
        let line = [
            mv([0., 0.], [100., 0.], 100.),
            mv([100., 0.], [200., 0.], 100.),
        ];
        let v = plan(&line, &m);
        assert!(v.iter().all(|s| *s > 90. && *s <= 100.), "{v:?}");
        // A zigzag of 1 mm moves reversing direction: far below the commanded speed.
        let zig: Vec<Move> = (0..20)
            .map(|i| {
                let x = if i % 2 == 0 { [0., 1.] } else { [1., 0.] };
                mv([x[0], i as f32 * 0.1], [x[1], i as f32 * 0.1 + 0.1], 200.)
            })
            .collect();
        let v = plan(&zig, &m);
        assert!(v.iter().all(|s| *s < 60.), "{v:?}");
        // Every speed stays within the commanded feed.
        assert!(plan(&zig, &m).iter().all(|s| *s <= 200. + 1e-9));
    }
    #[test]
    fn junction_deviation_limits_corners() {
        let m = Machine {
            junction_deviation: 0.02,
            ..Default::default()
        };
        // A square: 90° corners slow the moves below the straight-line case.
        let square = [
            mv([0., 0.], [10., 0.], 200.),
            mv([10., 0.], [10., 10.], 200.),
            mv([10., 10.], [0., 10.], 200.),
            mv([0., 10.], [0., 0.], 200.),
        ];
        let straight = [
            mv([0., 0.], [10., 0.], 200.),
            mv([10., 0.], [20., 0.], 200.),
            mv([20., 0.], [30., 0.], 200.),
            mv([30., 0.], [40., 0.], 200.),
        ];
        let a = plan(&square, &m);
        let b = plan(&straight, &m);
        assert!(a[1] < b[1], "{a:?} {b:?}");
    }
    #[test]
    fn volumetric_flow_from_the_record() {
        let s = Segment::new([0.; 3], [1., 0., 0.], [0.45, 0.2], 2, 0, 100., 0.);
        let expected = 0.2 * (0.45 - 0.2 * (1. - std::f64::consts::FRAC_PI_4)) * 100.;
        assert!((volumetric(&s) - expected).abs() < 1e-9);
        let t = Segment::new([0.; 3], [1., 0., 0.], [0.08, 0.04], TRAVEL, 0, 200., 0.);
        assert_eq!(volumetric(&t), 0.);
    }
    #[test]
    fn accelerations_follow_the_exporter() {
        let s = PrintSettings::core_one_hf04_speed_pla();
        let c = Config::defaults();
        assert_eq!(
            emitted_acceleration(&s, &c, Role::ExternalPerimeter as u32, false),
            s.external_perimeter_acceleration
        );
        assert_eq!(
            emitted_acceleration(&s, &c, Role::InternalInfill as u32, true),
            s.first_layer_acceleration
        );
        let mut none = s.clone();
        none.default_acceleration = 0.;
        assert_eq!(emitted_acceleration(&none, &c, 2, false), 0.);
        assert_eq!(role_of(13), Some(Role::SupportMaterialInterface));
        assert_eq!(role_of(TRAVEL), None);
    }
}
