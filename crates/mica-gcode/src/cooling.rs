// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/CoolingBuffer.cpp :: CoolingLine, CoolingBuffer::apply_layer_cooldown
// Source: orca@8500fcdcca src/libslic3r/GCode/CoolingBuffer.cpp :: CoolingBuffer::parse_layer_gcode, CoolingBuffer::apply_layer_cooldown
// Changes: Structured path blocks replace G-code parsing; per-tool slowdown and protected tails; f64 arithmetic.
// Changes: Orca interpolation preserves equal fan endpoints exactly, avoiding spurious 99% commands.
// Changes: PrusaSlicer interpolation computes t in f32 like upstream's float layer time.
// Copyright (c) Prusa Research 2017 - 2023 Lukáš Matěna @lukasmatena, Pavel Mikuš @Godrak, Vojtěch Bubník @bubnikv
// Copyright (c) Slic3r 2016 Alessandro Ranellucci @alranel
// Ported from lib/Slic3r/GCode/CoolingBuffer.pm:
// Copyright (c) Prusa Research 2016 - 2017 Vojtěch Bubník @bubnikv
// Copyright (c) Slic3r 2013 - 2016 Alessandro Ranellucci @alranel
// Copyright (c) 2016 Chow Loong Jin @hyperair
// Changes: Engine choices read mica_slicing_rules (mica_core::rules), not gcode_flavor or the presence of Orca keys (docs/slicing-rules.md).

//! Layer-local, per-extruder cooling. Durations use nominal feedrates, as upstream does.
mod adjustment;
pub use adjustment::{CoolingLine, PerExtruderAdjustments, slow_down_multi};
use mica_core::{config::Config, settings::PrintSettings};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SlowdownLogic {
    Proportional,
    #[default]
    UniformCooling,
    ConsistentSurface,
}

/// One constant-speed extrusion block, in printing order.
#[derive(Clone, Copy, Debug, Default)]
pub struct Move {
    pub length_mm: f64,
    pub speed: f64,
    pub external: bool,
    pub adjustable: bool,
    /// External = 0, first inner = 1; other paths have no perimeter index.
    pub perimeter_index: Option<usize>,
    pub tool: usize,
    /// Output: tail to emit at the original speed, in mm.
    pub non_adjustable_length: f64,
}

#[derive(Clone, Copy, Debug)]
pub struct CoolingConfig {
    pub enabled: bool,
    pub logic: SlowdownLogic,
    pub perimeter_transition_distance: f64,
    pub orca: bool,
    pub dont_slow_down_outer_wall: bool,
    pub slowdown_below_layer_time: f64,
    pub fan_below_layer_time: f64,
    pub min_print_speed: f64,
    pub min_fan_speed: f64,
    pub max_fan_speed: f64,
    pub disable_fan_first_layers: usize,
    pub full_fan_speed_layer: usize,
    pub fan_always_on: bool,
}
impl Default for CoolingConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            logic: SlowdownLogic::UniformCooling,
            perimeter_transition_distance: 0.,
            orca: false,
            dont_slow_down_outer_wall: false,
            slowdown_below_layer_time: 5.,
            fan_below_layer_time: 60.,
            min_print_speed: 10.,
            min_fan_speed: 35.,
            max_fan_speed: 100.,
            disable_fan_first_layers: 1,
            full_fan_speed_layer: 0,
            fan_always_on: false,
        }
    }
}
impl CoolingConfig {
    pub fn from_config(c: &Config, s: &PrintSettings) -> Self {
        // Orca's CoolingBuffer under Orca rules, PrusaSlicer's otherwise.
        let orca = mica_core::rules::Rules::of(c).is_orca();
        Self {
            enabled: c.bool("cooling").unwrap_or(true),
            logic: if orca {
                SlowdownLogic::UniformCooling
            } else {
                match c.string("cooling_slowdown_logic") {
                    Some("consistent_surface") => SlowdownLogic::ConsistentSurface,
                    Some("proportional") => SlowdownLogic::Proportional,
                    _ => SlowdownLogic::UniformCooling,
                }
            },
            perimeter_transition_distance: c
                .float("cooling_perimeter_transition_distance")
                .unwrap_or(0.),
            orca,
            dont_slow_down_outer_wall: orca && c.bool("dont_slow_down_outer_wall").unwrap_or(false),
            slowdown_below_layer_time: s.slowdown_below_layer_time,
            fan_below_layer_time: s.fan_below_layer_time,
            min_print_speed: s.min_print_speed,
            min_fan_speed: s.min_fan_speed,
            max_fan_speed: s.max_fan_speed,
            disable_fan_first_layers: s.disable_fan_first_layers,
            full_fan_speed_layer: s.full_fan_speed_layer,
            fan_always_on: c.bool("fan_always_on").unwrap_or(true),
        }
    }
}

/// Single-tool adapter. Multi-tool callers must supply every tool's configuration.
pub fn slow_down(moves: &mut [Move], fixed_time: f64, cfg: &CoolingConfig) -> f64 {
    slow_down_multi(moves, fixed_time, &[*cfg])
}

/// Fan speed in percent for a layer that takes `layer_time` seconds.
pub fn fan_speed(layer: usize, layer_time: f64, cfg: &CoolingConfig) -> f64 {
    let mut disable = cfg.disable_fan_first_layers;
    if !cfg.orca && disable == 0 && cfg.full_fan_speed_layer > 0 {
        disable = 1; // the ramp needs a zero-fan first layer
    }
    if layer < disable {
        return 0.0;
    }
    let mut fan = if cfg.fan_always_on {
        cfg.min_fan_speed
    } else {
        0.0
    };
    if (cfg.enabled || cfg.orca) && layer_time < cfg.slowdown_below_layer_time {
        fan = cfg.max_fan_speed;
    } else if (cfg.enabled || cfg.orca) && layer_time < cfg.fan_below_layer_time {
        let t = if cfg.orca {
            (layer_time - cfg.slowdown_below_layer_time)
                / (cfg.fan_below_layer_time - cfg.slowdown_below_layer_time)
        } else {
            // PrusaSlicer's layer time and limits are floats, so t is a float: with it,
            // t * min + (1 - t) * max is exact and equal limits stay at their value (f64
            // arithmetic made a 100/100 % filament drop to 99 % on longer layers).
            let (time, slow, fan) = (
                layer_time as f32,
                cfg.slowdown_below_layer_time as f32,
                cfg.fan_below_layer_time as f32,
            );
            f64::from((time - slow) / (fan - slow))
        };
        // Upstream applies floor before adding 0.5 and casting to int.
        fan = if cfg.orca && cfg.min_fan_speed == cfg.max_fan_speed {
            cfg.min_fan_speed.floor()
        } else {
            (t * cfg.min_fan_speed + (1.0 - t) * cfg.max_fan_speed).floor()
        };
    }
    if layer + 1 < cfg.full_fan_speed_layer {
        let factor = (layer + 1 - disable) as f64 / (cfg.full_fan_speed_layer - disable) as f64;
        fan = (fan * factor + 0.5).floor().clamp(0.0, 100.0);
    }
    fan
}

/// Resolve the upstream SET/RESET_FAN_SPEED request against cooling and the first-layer ramp.
/// Returns the final fan percent; the exporter emits M106/M107 directly, consuming the markers.
pub fn requested_fan_speed(
    layer: usize,
    time: f64,
    cfg: &CoolingConfig,
    requested: Option<f64>,
    bridge: Option<f64>,
) -> f64 {
    let normal = fan_speed(layer, time, cfg);
    let disabled = cfg
        .disable_fan_first_layers
        .max(usize::from(!cfg.orca && cfg.full_fan_speed_layer > 0));
    if layer < disabled {
        return 0.;
    }
    let maximum = if layer + 1 < cfg.full_fan_speed_layer {
        normal
    } else {
        100.
    };
    if let Some(fan) = requested {
        return fan.trunc().clamp(normal.min(maximum), maximum);
    }
    if let Some(mut fan) = bridge {
        if layer + 1 < cfg.full_fan_speed_layer {
            fan = (fan * (layer + 1 - disabled) as f64
                / (cfg.full_fan_speed_layer - disabled) as f64
                + 0.5)
                .floor();
        }
        return normal.max(fan).clamp(0., 100.);
    }
    normal
}

#[cfg(test)]
mod tests;
