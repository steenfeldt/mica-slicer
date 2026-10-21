// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/GCodeProcessor.cpp :: TimeBlock::calculate_trapezoid, TimeMachine::calculate_time, process_G1, calc_junction_acceleration
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/GCodeProcessor.hpp :: TimeProcessor::Planner
// Source: orca@8500fcdcca src/libslic3r/GCode/GCodeProcessor.cpp :: get_axis_max_jerk
// Changes: Klipper uses classic jerk as Orca does. Timing only; bounded blocks and streamed completion records replace preview and per-move caches.
// Source: orca@8500fcdcca src/libslic3r/GCode/GCodeProcessor.cpp :: GCodeProcessor::process_G1, GCodeProcessor::get_xyz_max_jerk
// Changes: Klipper (Orca) planning adds BBS's centripetal cruise limit at shallow corners, one XYZ jerk vector and the axis-limited previous feedrate.

use mica_core::config::Config;

pub(super) const QUEUE: usize = 64;
pub(super) const REFRESH: usize = 4 * QUEUE;

#[derive(Clone, Copy, Default)]
struct State {
    feed: f32,
    safe: f32,
    axis: [f32; 4],
    unit: [f32; 4],
    /// Orca's XYZ enter/exit direction (zero for an extruder-only move).
    direction: [f32; 3],
}

struct Block {
    line: u64,
    first_layer: bool,
    distance: f32,
    acceleration: f32,
    cruise: f32,
    entry: f32,
    exit: f32,
    safe: f32,
    max_entry: f32,
    nominal: bool,
    recalculate: bool,
    time: f32,
}

fn speed(initial: f32, acceleration: f32, distance: f32) -> f32 {
    (initial * initial + 2.0 * acceleration * distance)
        .max(0.0)
        .sqrt()
}

impl Block {
    fn trapezoid(&mut self) {
        let a = self.acceleration;
        if a <= 0.0 {
            self.time = self.distance / self.cruise;
            return;
        }
        let mut up = ((self.cruise * self.cruise - self.entry * self.entry) / (2.0 * a)).max(0.0);
        let down = ((self.cruise * self.cruise - self.exit * self.exit) / (2.0 * a)).max(0.0);
        let mut cruise_distance = self.distance - up - down;
        let mut cruise = self.cruise;
        if cruise_distance < 0.0 {
            up = ((2.0 * a * self.distance - self.entry * self.entry + self.exit * self.exit)
                / (4.0 * a))
                .clamp(0.0, self.distance);
            cruise_distance = 0.0;
            cruise = speed(self.entry, a, up);
        }
        self.time = (speed(self.entry, a, up) - self.entry) / a
            + if cruise > 0.0 {
                cruise_distance / cruise
            } else {
                0.0
            }
            + (speed(cruise, -a, self.distance - up - cruise_distance) - cruise) / -a;
    }
}

pub(super) struct Machine {
    pub enabled: bool,
    pub axis_accel: [f32; 4],
    pub axis_speed: [f32; 4],
    pub jerk: [f32; 4],
    pub acceleration: [f32; 3], // extrusion, retraction, travel
    pub max_acceleration: [f32; 3],
    pub min_rate: [f32; 2],
    pub junction_deviation: f32,
    pub flow: f32,
    pub time: f64,
    pub first_layer: f32,
    /// Orca's (BBS) planner for Klipper: centripetal corner limit and XYZ vector jerk.
    orca: bool,
    prev: State,
    blocks: Vec<Block>,
}

impl Machine {
    pub fn new(config: &Config, mode: usize, klipper: bool) -> Self {
        let defaults = Config::defaults();
        let limits = if config.string("machine_limits_usage") == Some("ignore") {
            &defaults
        } else {
            config
        };
        let get = |key: &str| {
            limits
                .float_at(key, mode)
                .or_else(|| defaults.float_at(key, mode))
                .unwrap_or(0.0) as f32
        };
        let axes = |prefix: &str| ['x', 'y', 'z', 'e'].map(|a| get(&format!("{prefix}_{a}")));
        let mut max_acceleration = [
            get("machine_max_acceleration_extruding"),
            get("machine_max_acceleration_retracting"),
            get("machine_max_acceleration_travel"),
        ];
        if klipper || config.string("machine_limits_usage") != Some("emit_to_gcode") {
            max_acceleration[2] = 0.0;
        }
        let acceleration = std::array::from_fn(|i| {
            if max_acceleration[i] > 0.0 {
                max_acceleration[i]
            } else {
                [1500.0, 1500.0, 1250.0][i]
            }
        });
        Self {
            enabled: mode == 0 || (!klipper && config.bool("silent_mode").unwrap_or(false)),
            axis_accel: axes("machine_max_acceleration"),
            axis_speed: axes("machine_max_feedrate"),
            jerk: axes("machine_max_jerk"),
            acceleration,
            max_acceleration,
            min_rate: [
                get("machine_min_extruding_rate"),
                get("machine_min_travel_rate"),
            ],
            junction_deviation: if klipper {
                0.0
            } else {
                get("machine_max_junction_deviation")
            },
            flow: 1.0,
            time: 0.0,
            first_layer: 0.0,
            orca: klipper,
            prev: State::default(),
            blocks: Vec::with_capacity(REFRESH + 1),
        }
    }

    pub fn set_acceleration(&mut self, kind: usize, value: f32) {
        self.acceleration[kind] = if self.max_acceleration[kind] > 0.0 {
            value.min(self.max_acceleration[kind])
        } else {
            value
        };
    }

    pub fn len(&self) -> usize {
        self.blocks.len()
    }

    pub fn push(&mut self, delta: [f64; 4], feed: f32, line: u64, first_layer: bool, wiping: bool) {
        if !self.enabled {
            return;
        }
        let xyz = delta[..3].iter().map(|v| v * v).sum::<f64>();
        let distance = if xyz > 0.0 {
            xyz.sqrt()
        } else {
            delta[3].abs()
        } as f32;
        if distance == 0.0 || feed <= 0.0 {
            return;
        }
        let inv = 1.0 / distance;
        let mut curr = State {
            feed: feed.max(self.min_rate[usize::from(delta[3] == 0.0)]),
            ..State::default()
        };
        if self.orca {
            if xyz > 0.0 {
                let norm = delta[..3]
                    .iter()
                    .map(|d| (*d as f32).powi(2))
                    .sum::<f32>()
                    .sqrt();
                curr.direction = std::array::from_fn(|i| delta[i] as f32 / norm);
            }
            // BBS: limit the cruise by the centripetal acceleration of a shallow corner.
            let (p, c) = (self.prev.direction, curr.direction);
            if (p[0] != 0.0 || p[1] != 0.0) && (c[0] != 0.0 || c[1] != 0.0) {
                let unit = |v: [f32; 3]| {
                    let n = (v[0] * v[0] + v[1] * v[1]).sqrt();
                    [v[0] / n, v[1] / n]
                };
                let (v1, v2) = (unit(p), unit(c));
                let norm_diff = ((v2[0] - v1[0]).powi(2) + (v2[1] - v1[1]).powi(2)).sqrt();
                if norm_diff < 0.5 && norm_diff > 0.00001 {
                    let dot = v1[0] * v2[0] + v1[1] * v2[1];
                    let cross = v1[0] * v2[1] - v1[1] * v2[0];
                    let angle = f64::from(cross).atan2(f64::from(dot)) as f32;
                    let sin_theta_2 = ((1.0 - angle.cos()) * 0.5).sqrt();
                    let r = ((delta[0] * delta[0] + delta[1] * delta[1]).sqrt() * 0.5) as f32
                        / sin_theta_2;
                    curr.feed = curr.feed.min((self.acceleration[0] * r).sqrt());
                }
            }
        }
        let mut factor = 1.0f32;
        for (i, d) in delta.iter().enumerate() {
            curr.axis[i] = (f64::from(curr.feed) * d * f64::from(inv)) as f32;
            if i == 3 {
                curr.axis[i] *= self.flow;
            }
            if curr.axis[i] != 0.0 && self.axis_speed[i] != 0.0 {
                factor = factor.min(self.axis_speed[i] / curr.axis[i].abs());
            }
        }
        let cruise = factor * curr.feed;
        for v in &mut curr.axis {
            *v *= factor;
        }
        if self.orca {
            // Orca keeps the axis-limited feedrate as the previous move's feedrate.
            curr.feed = cruise;
        }
        let travel = xyz > 0.0 && (delta[3] <= 0.0 || (delta[0] == 0.0 && delta[1] == 0.0));
        let mut accel = self.acceleration[if travel && !wiping {
            2
        } else if xyz == 0.0 {
            1
        } else {
            0
        }];
        for (i, d) in delta.iter().enumerate() {
            let scale = (d.abs() * f64::from(inv)) as f32;
            if accel * scale > self.axis_accel[i] {
                accel = self.axis_accel[i] / scale;
            }
        }
        let prev = self.prev;
        let has_prev = !self.blocks.is_empty() && prev.feed > 0.0001;
        let mut junction = 0.0f32;
        if self.junction_deviation > 0.0 {
            curr.unit = delta.map(|d| d as f32 / distance);
            if has_prev {
                let mut cos = -prev
                    .unit
                    .iter()
                    .zip(curr.unit)
                    .map(|(a, b)| a * b)
                    .sum::<f32>();
                if cos <= 0.999999 {
                    cos = cos.max(-0.999999);
                    let mut vector: [f32; 4] = std::array::from_fn(|i| curr.unit[i] - prev.unit[i]);
                    let norm = vector.iter().map(|v| v * v).sum::<f32>().sqrt();
                    for v in &mut vector {
                        *v /= norm;
                    }
                    let mut ja = accel;
                    for (i, v) in vector.iter().enumerate() {
                        if *v != 0.0 {
                            ja = ja.min((self.axis_accel[i] / v).abs());
                        }
                    }
                    let sin = (0.5 * (1.0 - cos)).sqrt();
                    let mut squared = ja * self.junction_deviation * sin / (1.0 - sin);
                    if distance < 1.0 && cos < -std::f32::consts::FRAC_1_SQRT_2 {
                        // W. Randolph Franklin's MinMax acos approximation, as used upstream:
                        // https://wrf.ecse.rpi.edu/Research/Short_Notes/arcsin/onlyelem.html
                        let t = cos.abs();
                        let asin = 0.032843707
                            + t * (-1.4518384
                                + t * (29.66154
                                    + t * (-131.11235
                                        + t * (262.81305 + t * (-242.71996 + t * 84.31466)))));
                        let theta = std::f32::consts::FRAC_PI_2 + cos.signum() * asin;
                        squared = squared.min(distance * ja / theta);
                    }
                    junction = squared
                        .min(cruise * cruise)
                        .min(prev.feed * prev.feed)
                        .sqrt();
                }
            }
        } else {
            curr.safe = cruise;
            for i in 0..4 {
                if curr.axis[i].abs() > self.jerk[i] {
                    curr.safe = curr.safe.min(self.jerk[i]);
                }
            }
            junction = curr.safe;
            if has_prev {
                let larger = prev.feed > cruise;
                let smaller = if larger {
                    cruise / prev.feed
                } else {
                    prev.feed / cruise
                };
                junction = cruise.min(prev.feed);
                let mut factor = 1.0f32;
                let mut limited = false;
                if self.orca {
                    // BBS: one XYZ jerk vector, each component against its axis limit; the
                    // running factor also scales the components still to be checked.
                    let mut jerk: [f32; 3] = std::array::from_fn(|i| {
                        let exit =
                            prev.feed * prev.direction[i] * if larger { smaller } else { 1.0 };
                        (cruise * curr.direction[i] - exit).abs()
                    });
                    for i in 0..3 {
                        if jerk[i] > self.jerk[i] {
                            factor *= self.jerk[i] / jerk[i];
                            for v in &mut jerk {
                                *v *= factor;
                            }
                            limited = true;
                        }
                    }
                }
                for i in if self.orca { 3..4 } else { 0..4 } {
                    let mut exit = prev.axis[i];
                    let mut entry = curr.axis[i];
                    if larger {
                        exit *= smaller;
                    }
                    if limited {
                        exit *= factor;
                        entry *= factor;
                    }
                    let jerk = if exit > entry {
                        if entry > 0.0 || exit < 0.0 {
                            exit - entry
                        } else {
                            exit.max(-entry)
                        }
                    } else if entry < 0.0 || exit > 0.0 {
                        entry - exit
                    } else {
                        (-exit).max(entry)
                    };
                    if jerk > self.jerk[i] {
                        factor *= self.jerk[i] / jerk;
                        limited = true;
                    }
                }
                if limited {
                    junction *= factor;
                }
                if prev.safe > junction * 0.99 && curr.safe > junction * 0.99 {
                    junction = curr.safe;
                }
            }
        }
        let allowable = speed(curr.safe, accel, distance);
        let mut block = Block {
            line,
            first_layer,
            distance,
            acceleration: accel,
            cruise,
            entry: junction.min(allowable),
            exit: curr.safe,
            safe: curr.safe,
            max_entry: junction,
            nominal: cruise <= allowable,
            recalculate: true,
            time: 0.0,
        };
        block.trapezoid();
        self.blocks.push(block);
        self.prev = curr;
    }

    pub fn calculate(&mut self, keep: usize, additional: f32) -> Vec<(u64, f64)> {
        if !self.enabled {
            return Vec::new();
        }
        // Upstream leaves a lone block queued. At EOF Mica also accounts for one-block programs.
        if self.blocks.is_empty() {
            self.time += f64::from(additional);
            return Vec::new();
        }
        for i in (1..self.blocks.len()).rev() {
            let (left, right) = self.blocks.split_at_mut(i);
            let curr = &mut left[i - 1];
            let next = &right[0];
            if curr.entry != curr.max_entry || next.recalculate {
                let entry = if curr.nominal {
                    curr.max_entry
                } else {
                    curr.max_entry
                        .min(speed(next.entry, curr.acceleration, curr.distance))
                };
                if curr.entry != entry {
                    curr.entry = entry;
                    curr.recalculate = true;
                }
            }
        }
        for i in 1..self.blocks.len() {
            let (left, right) = self.blocks.split_at_mut(i);
            let prev = &left[i - 1];
            let curr = &mut right[0];
            if !prev.nominal && prev.entry < curr.entry {
                let entry = speed(prev.entry, prev.acceleration, prev.distance);
                if entry < curr.entry {
                    curr.entry = entry;
                    curr.recalculate = true;
                }
            }
        }
        for i in 0..self.blocks.len() {
            let (left, right) = self.blocks.split_at_mut(i + 1);
            let curr = &mut left[i];
            if right.is_empty() || curr.recalculate || right[0].recalculate {
                curr.exit = right.first().map_or(curr.safe, |b| b.entry);
                curr.trapezoid();
                curr.recalculate = false;
            }
        }
        let n = self.blocks.len() - keep;
        let mut records = Vec::with_capacity(n);
        for (i, b) in self.blocks.drain(..n).enumerate() {
            let time = b.time + if i + 1 == n { additional } else { 0.0 };
            self.time += f64::from(time);
            if b.first_layer {
                self.first_layer += time;
            }
            records.push((b.line, self.time));
        }
        if let Some(b) = self.blocks.first_mut() {
            b.max_entry = b.entry;
        }
        records
    }
}
