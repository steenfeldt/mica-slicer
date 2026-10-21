// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/CoolingBuffer.cpp :: CoolingLine, PerExtruderAdjustments, extruder_range_slow_down_proportional, extruder_range_slow_down_non_proportional, CoolingBuffer::calculate_layer_slowdown
// Source: orca@8500fcdcca src/libslic3r/GCode/CoolingBuffer.cpp :: CoolingBuffer::calculate_layer_slowdown
// Changes: Indexed structured blocks instead of parsed G-code; f64; guards for empty adjustable ranges and proportional saturation; per-tool transition distance.
// Copyright (c) Prusa Research 2017 - 2023 Lukáš Matěna @lukasmatena, Pavel Mikuš @Godrak, Vojtěch Bubník @bubnikv
// Copyright (c) Slic3r 2016 Alessandro Ranellucci @alranel
// Ported from lib/Slic3r/GCode/CoolingBuffer.pm:
// Copyright (c) Prusa Research 2016 - 2017 Vojtěch Bubník @bubnikv
// Copyright (c) Slic3r 2013 - 2016 Alessandro Ranellucci @alranel
// Copyright (c) 2016 Chow Loong Jin @hyperair

use super::{CoolingConfig, Move, SlowdownLogic};

const EPSILON: f64 = 1e-6;
const EXTERNAL: u8 = 1;
const FIRST_INTERNAL: u8 = 2;
const ALL: u8 = EXTERNAL | FIRST_INTERNAL;

/// One upstream EXTRUDE_SET_SPEED block. Parser-only flags (G92, fan markers, etc.)
/// are represented by Mica's writer/plan rather than stored as inert lines here.
#[derive(Clone, Debug)]
pub struct CoolingLine {
    pub move_index: usize,
    pub adjustable_length: f64,
    pub non_adjustable_length: f64,
    pub feedrate: f64,
    pub feedrate_original: f64,
    pub adjustable_time: f64,
    pub adjustable_time_max: f64,
    pub non_adjustable_time: f64,
    pub slowdown: bool,
    adjustable: bool,
    features: u8,
}
impl CoolingLine {
    fn new(index: usize, m: &Move, c: &CoolingConfig) -> Self {
        let adjustable = m.adjustable && !(c.dont_slow_down_outer_wall && m.external);
        let length = m.length_mm.max(0.);
        let time = if length > 0. && m.speed > 0. {
            length / m.speed
        } else {
            0.
        };
        Self {
            move_index: index,
            adjustable_length: if adjustable { length } else { 0. },
            non_adjustable_length: if adjustable { 0. } else { length },
            feedrate: m.speed,
            feedrate_original: m.speed,
            adjustable_time: if adjustable { time } else { 0. },
            adjustable_time_max: if !adjustable || length == 0. {
                0.
            } else if c.min_print_speed == 0. {
                f64::INFINITY
            } else {
                time.max(length / c.min_print_speed)
            },
            non_adjustable_time: if adjustable { 0. } else { time },
            slowdown: false,
            adjustable,
            features: if m.external {
                EXTERNAL
            } else if m.perimeter_index == Some(1) {
                FIRST_INTERNAL
            } else {
                0
            },
        }
    }
    fn adjustable(&self, features: u8) -> bool {
        self.adjustable
            && self.adjustable_length > 0.
            && self.adjustable_time < self.adjustable_time_max
            && (self.features == 0 || self.features & features != 0)
    }
    pub fn time(&self) -> f64 {
        self.adjustable_time + self.non_adjustable_time
    }
    fn time_max(&self) -> f64 {
        self.adjustable_time_max + self.non_adjustable_time
    }
    fn set_time(&mut self, time: f64) {
        self.slowdown = true;
        self.adjustable_time = time.min(self.adjustable_time_max);
        self.feedrate = self.adjustable_length / self.adjustable_time;
    }
}

/// A material's lines across every use of its tool on this layer, in source order
/// until sorting. Fixed travel/retraction/toolchange time is supplied separately.
#[derive(Clone, Debug)]
pub struct PerExtruderAdjustments {
    pub extruder_id: usize,
    pub config: CoolingConfig,
    pub lines: Vec<CoolingLine>,
    n_lines_adjustable: usize,
    time_maximum: f64,
}
impl PerExtruderAdjustments {
    fn active(&self) -> bool {
        self.config.enabled && !self.lines.is_empty()
    }
    pub fn elapsed_time_total(&self) -> f64 {
        self.lines.iter().map(CoolingLine::time).sum()
    }
    fn maximum_time_after_slowdown(&self, features: u8) -> f64 {
        self.lines
            .iter()
            .map(|l| {
                if l.adjustable(features) {
                    l.time_max()
                } else {
                    l.time()
                }
            })
            .sum()
    }
    fn adjustable_time(&self, features: u8) -> f64 {
        self.lines
            .iter()
            .filter(|l| l.adjustable(features))
            .map(|l| l.adjustable_time)
            .sum()
    }
    fn non_adjustable_time(&self, features: u8) -> f64 {
        self.lines
            .iter()
            .map(|l| {
                if l.adjustable(features) {
                    l.non_adjustable_time
                } else {
                    l.time()
                }
            })
            .sum()
    }
    fn slowdown_to_minimum_feedrate(&mut self, features: u8) -> f64 {
        for l in &mut self.lines {
            if l.adjustable(features) {
                l.set_time(l.adjustable_time_max);
            }
        }
        self.elapsed_time_total()
    }
    fn slow_down_proportional(&mut self, factor: f64, features: u8) -> f64 {
        for l in &mut self.lines {
            if l.adjustable(features) {
                l.set_time(l.adjustable_time * factor.max(1.));
            }
        }
        self.elapsed_time_total()
    }
    fn sort_lines_by_decreasing_feedrate(&mut self) {
        self.lines.sort_by(|a, b| {
            b.adjustable(ALL)
                .cmp(&a.adjustable(ALL))
                .then_with(|| b.feedrate.total_cmp(&a.feedrate))
                .then_with(|| a.move_index.cmp(&b.move_index))
        });
        self.n_lines_adjustable = self.lines.iter().take_while(|l| l.adjustable(ALL)).count();
    }
    fn time_stretch(&self, speed: f64, features: u8) -> f64 {
        self.lines[..self.n_lines_adjustable]
            .iter()
            .filter(|l| l.adjustable(features) && l.feedrate > speed)
            .map(|l| l.adjustable_time * (l.feedrate / speed - 1.))
            .sum()
    }
    fn slow_down_to_feedrate(&mut self, speed: f64, features: u8) {
        for l in &mut self.lines[..self.n_lines_adjustable] {
            if l.adjustable(features) && l.feedrate > speed {
                l.set_time(l.adjustable_length / speed);
            }
        }
    }
    fn create_non_adjustable_segments(&mut self) {
        let mut remaining = self.config.perimeter_transition_distance;
        for l in self.lines.iter_mut().rev() {
            if remaining <= 0. {
                break;
            }
            if l.adjustable(0) {
                let length = remaining.min(l.adjustable_length);
                remaining -= length;
                l.adjustable_length -= length;
                l.non_adjustable_length += length;
                l.adjustable_time = l.adjustable_length / l.feedrate;
                l.non_adjustable_time = l.non_adjustable_length / l.feedrate;
                l.adjustable_time_max = if l.adjustable_length == 0. {
                    0.
                } else if self.config.min_print_speed == 0. {
                    f64::INFINITY
                } else {
                    l.adjustable_time
                        .max(l.adjustable_length / self.config.min_print_speed)
                };
            }
        }
    }
}

fn proportional(adj: &mut [PerExtruderAdjustments], fixed: f64, target: f64) {
    let maximum = fixed
        + adj
            .iter()
            .map(|a| a.maximum_time_after_slowdown(0))
            .sum::<f64>();
    let features = if maximum > target {
        0
    } else {
        for a in adj.iter_mut() {
            a.slowdown_to_minimum_feedrate(0);
        }
        ALL
    };
    let non_adjustable = fixed
        + adj
            .iter()
            .map(|a| a.non_adjustable_time(features))
            .sum::<f64>();
    // Recompute after saturating the inner moves. Upstream's stale total can yield
    // a negative factor with mixed minimum speeds; that is never a valid slowdown.
    let mut total = fixed
        + adj
            .iter()
            .map(PerExtruderAdjustments::elapsed_time_total)
            .sum::<f64>();
    for _ in 0..5 {
        if total >= target || total <= non_adjustable {
            break;
        }
        let factor = (target - non_adjustable) / (total - non_adjustable);
        total = fixed
            + adj
                .iter_mut()
                .map(|a| a.slow_down_proportional(factor, features))
                .sum::<f64>();
        if total > 0.95 * target {
            break;
        }
    }
}

fn new_feedrate(
    adj: &[PerExtruderAdjustments],
    order: &[usize],
    mut minimum: f64,
    stretch: f64,
    features: u8,
) -> f64 {
    let mut speed = minimum;
    for _ in 0..20 {
        let mut numerator = 0.;
        let mut denominator = stretch;
        for &i in order {
            for l in &adj[i].lines[..adj[i].n_lines_adjustable] {
                if l.adjustable(features) && l.feedrate > minimum {
                    numerator += l.adjustable_time * l.feedrate;
                    denominator += l.adjustable_time;
                }
            }
        }
        if numerator <= 0. || denominator <= EPSILON {
            return minimum;
        }
        speed = numerator / denominator;
        if speed < minimum + EPSILON
            || !order.iter().any(|&i| {
                adj[i].lines[..adj[i].n_lines_adjustable]
                    .iter()
                    .any(|l| l.adjustable(features) && l.feedrate > minimum && l.feedrate < speed)
            })
        {
            break;
        }
        minimum = speed;
    }
    speed
}

fn non_proportional(adj: &mut [PerExtruderAdjustments], mut stretch: f64, features: u8) -> f64 {
    let mut order: Vec<_> = (0..adj.len())
        .filter(|&i| adj[i].n_lines_adjustable > 0)
        .collect();
    order.sort_by(|&a, &b| {
        adj[b]
            .config
            .min_print_speed
            .total_cmp(&adj[a].config.min_print_speed)
    });
    let mut begin = vec![0; adj.len()];
    let mut end = begin.clone();
    let mut feedrate = order
        .iter()
        .map(|&i| adj[i].lines[0].feedrate)
        .fold(0., f64::max);
    if feedrate == 0. || stretch <= EPSILON {
        return stretch;
    }
    loop {
        for &i in &order {
            end[i] = begin[i];
            while end[i] < adj[i].n_lines_adjustable
                && adj[i].lines[end[i]].feedrate > feedrate - EPSILON
            {
                end[i] += 1;
            }
        }
        let next = order
            .iter()
            .filter(|&&i| end[i] < adj[i].n_lines_adjustable)
            .map(|&i| adj[i].lines[end[i]].feedrate)
            .fold(0., f64::max);
        let mut j = 0;
        while j < order.len() {
            let min_speed = adj[order[j]].config.min_print_speed;
            let range = &order[j..];
            if min_speed == 0. {
                let time: f64 = range
                    .iter()
                    .map(|&i| adj[i].adjustable_time(features))
                    .sum();
                if time > 0. {
                    for &i in range {
                        adj[i].slow_down_proportional((time + stretch) / time, features);
                    }
                    return 0.;
                }
            } else {
                let mut limit = next.max(min_speed);
                let maximum: f64 = range
                    .iter()
                    .map(|&i| adj[i].time_stretch(limit, features))
                    .sum();
                let done = maximum >= stretch;
                if done {
                    limit = new_feedrate(adj, range, limit, stretch, features);
                } else {
                    stretch -= maximum;
                }
                for &i in range {
                    adj[i].slow_down_to_feedrate(limit, features);
                }
                if done {
                    return 0.;
                }
            }
            j += 1;
            while j < order.len() && adj[order[j]].config.min_print_speed > min_speed - EPSILON {
                j += 1;
            }
        }
        if next == 0. {
            break;
        }
        begin.clone_from(&end);
        feedrate = next;
    }
    stretch
}

/// Slow the whole layer, finalizing tools in increasing layer-time threshold order.
/// Moves retain source order; tool indices address `configs`, including unused slots.
/// `fixed_time` includes all non-adjustable motion, waits and toolchange estimates.
pub fn slow_down_multi(moves: &mut [Move], fixed_time: f64, configs: &[CoolingConfig]) -> f64 {
    let mut adjustments: Vec<_> = configs
        .iter()
        .enumerate()
        .map(|(extruder_id, &config)| PerExtruderAdjustments {
            extruder_id,
            config,
            lines: Vec::new(),
            n_lines_adjustable: 0,
            time_maximum: 0.,
        })
        .collect();
    let mut layer_had_extrusion = configs.first().is_none_or(|c| !c.orca);
    for (i, m) in moves.iter_mut().enumerate() {
        m.non_adjustable_length = 0.;
        let a = &mut adjustments[m.tool];
        layer_had_extrusion |= m.adjustable;
        let mut line = CoolingLine::new(i, m, &a.config);
        if !layer_had_extrusion {
            line.non_adjustable_time = 0.;
        }
        a.lines.push(line);
    }
    let mut fixed = fixed_time;
    for a in &mut adjustments {
        if a.config.logic == SlowdownLogic::ConsistentSurface {
            a.create_non_adjustable_segments();
        }
        a.time_maximum = a.maximum_time_after_slowdown(ALL);
        // Sort every tool so mixed logic values are safe when a range is equalized.
        a.sort_lines_by_decreasing_feedrate();
        if !a.active() {
            fixed += a.elapsed_time_total();
        }
    }
    adjustments.sort_by(|a, b| {
        b.active()
            .cmp(&a.active())
            .then_with(|| {
                a.config
                    .slowdown_below_layer_time
                    .total_cmp(&b.config.slowdown_below_layer_time)
            })
            .then_with(|| a.extruder_id.cmp(&b.extruder_id))
    });
    let count = adjustments.iter().take_while(|a| a.active()).count();
    for i in 0..count {
        let adj = &mut adjustments[i..count];
        let target = adj[0].config.slowdown_below_layer_time * 1.001;
        let total = fixed
            + adj
                .iter()
                .map(PerExtruderAdjustments::elapsed_time_total)
                .sum::<f64>();
        if total < target {
            let maximum = fixed + adj.iter().map(|a| a.time_maximum).sum::<f64>();
            if maximum <= target {
                for a in adj.iter_mut() {
                    a.slowdown_to_minimum_feedrate(ALL);
                }
            } else {
                match adj[0].config.logic {
                    SlowdownLogic::Proportional => proportional(adj, fixed, target),
                    SlowdownLogic::UniformCooling => {
                        non_proportional(adj, target - total, ALL);
                    }
                    SlowdownLogic::ConsistentSurface => {
                        let remaining = non_proportional(adj, target - total, 0);
                        if remaining > EPSILON {
                            non_proportional(adj, remaining, ALL);
                        }
                    }
                }
            }
        }
        fixed += adj[0].elapsed_time_total();
    }
    for a in adjustments {
        for l in a.lines {
            moves[l.move_index].speed = l.feedrate;
            if l.slowdown {
                moves[l.move_index].non_adjustable_length = l.non_adjustable_length;
            }
        }
    }
    fixed
}
