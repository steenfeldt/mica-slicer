// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: orca@8500fcdcca src/libslic3r/GCode.cpp :: GCode::_extrude
// Source: orca@8500fcdcca src/libslic3r/GCode/CoolingBuffer.cpp :: CoolingBuffer::apply_layer_cooldown
// Changes: The role-based fan markers (`;_OVERHANG_FAN_START` and friends) are not written and parsed
// Changes: back: the exporter applies each marker's CoolingBuffer effect where the marker would be.
// Changes: change_extruder_set_fan and ;_FORCE_RESUME_FAN_SPEED keep CoolingBuffer's fan, auxiliary fan
// Changes: and current fan across layers and write them per filament (layer start, before the T line).

//! Orca's per-feature part fan: `_extrude` marks overhang/bridge, internal bridge, support
//! interface and ironing regions; `apply_layer_cooldown` raises the fan for a region whose fan
//! speed is above the layer's, and writes the layer fan again at the end of every overhang or
//! internal-bridge region (and of a requested support-interface or ironing region).
//! `change_extruder_set_fan` writes each filament's layer fan and auxiliary fan
//! (`additional_cooling_fan_speed`) at the start of a layer and at every toolchange.

use crate::writer::Writer;
use mica_core::{config::Config, toolpath::Role};

const OVERHANG: usize = 0;
const INTERNAL_BRIDGE: usize = 1;
const SUPPORT_INTERFACE: usize = 2;
const IRONING: usize = 3;

/// A filament option kept as Orca text (`get_at(tool)`: the tool's entry, else the first).
fn entry<'a>(c: &'a Config, key: &str, tool: usize) -> Option<&'a str> {
    let text = c.extra(key)?;
    let mut values = text.split(',');
    let first = values.clone().next();
    values.nth(tool).or(first).map(str::trim)
}
fn float(c: &Config, key: &str, tool: usize) -> Option<f64> {
    entry(c, key, tool)
        .and_then(|s| s.trim_end_matches('%').parse().ok())
        .or_else(|| c.float_at(key, tool))
}
/// `enable_overhang_bridge_fan`, on by default (Orca's schema default).
fn bridge_fan_enabled(c: &Config, tool: usize) -> bool {
    entry(c, "enable_overhang_bridge_fan", tool)
        .map(|s| matches!(s, "1" | "true"))
        .or_else(|| c.bool_at("enable_overhang_bridge_fan", tool))
        .unwrap_or(true)
}

/// `overhang_fan_threshold` (default 95%) as the largest supported fraction of the line width
/// that still counts as an overhang (`check_overhang_fan`); `None` is `Overhang_threshold_none`.
fn threshold(c: &Config, tool: usize) -> Option<f64> {
    let percent = float(c, "overhang_fan_threshold", tool).unwrap_or(95.);
    match percent.round() as i64 {
        0 => None,
        10 => Some(0.9),
        25 => Some(0.75),
        50 => Some(0.5),
        75 => Some(0.25),
        _ => Some(0.05),
    }
}

/// The role-based fan marker decisions of one path section: `Some(on)` where `_extrude`
/// evaluates a marker, per overhang, internal bridge, support interface and ironing.
pub(crate) type Markers = [Option<bool>; 4];

/// `GCode::_extrude`'s marker evaluation for a path (`variable`: Orca prints it with variable
/// overhang speeds) or one of its sections, whose start and end support fractions are given.
pub(crate) fn markers(
    c: &Config,
    tool: usize,
    role: Role,
    variable: bool,
    overlap: (f64, f64),
) -> Markers {
    let mut out = [None; 4];
    if bridge_fan_enabled(c, tool) {
        let limit = threshold(c, tool);
        let check = |overlap: f64| {
            matches!(role, Role::BridgeInfill | Role::OverhangPerimeter)
                || limit.map_or(role == Role::ExternalPerimeter, |l| overlap <= l)
        };
        out[OVERHANG] = Some(if variable {
            check(overlap.0) && check(overlap.1)
        } else {
            (limit.is_none() && role == Role::ExternalPerimeter)
                || matches!(role, Role::BridgeInfill | Role::OverhangPerimeter)
        });
        out[INTERNAL_BRIDGE] = Some(role == Role::InternalBridgeInfill);
    }
    out[SUPPORT_INTERFACE] = Some(
        float(c, "support_material_interface_fan_speed", tool).is_some_and(|v| v >= 0.)
            && role == Role::SupportMaterialInterface,
    );
    out[IRONING] =
        Some(float(c, "ironing_fan_speed", tool).is_some_and(|v| v >= 0.) && role == Role::Ironing);
    out
}

/// One layer's fan speeds for the current filament (`change_extruder_set_fan`).
#[derive(Clone, Copy, Debug, Default)]
struct Speeds {
    fan: f64,
    region: [f64; 4],
    control: [bool; 4],
}

#[derive(Default)]
pub(crate) struct State {
    /// `GCode::m_is_role_based_fan_on` and `m_role_based_fan_marker_layer`.
    on: [bool; 4],
    marker_layer: [Option<usize>; 4],
    /// `apply_layer_cooldown`'s `fan_speed_change_requests` (per layer).
    requests: [bool; 4],
    speeds: Speeds,
    layer: usize,
    /// CoolingBuffer's `m_fan_speed`, `m_additional_fan_speed` and `m_current_fan_speed`
    /// (`None` is its -1), kept across layers.
    fan: Option<f64>,
    auxiliary: Option<f64>,
    current: Option<f64>,
}

/// `close_fan_the_first_x_layers` of a filament.
fn closed_layers(c: &Config, tool: usize) -> usize {
    c.float_at("disable_fan_first_layers", tool)
        .unwrap_or(0.)
        .max(0.) as usize
}

/// The auxiliary fan percent `change_extruder_set_fan` sets for a filament on a layer:
/// `additional_cooling_fan_speed`, off for the first `close_fan_the_first_x_layers` layers.
pub(crate) fn auxiliary_speed(c: &Config, tool: usize, layer: usize) -> f64 {
    if layer < closed_layers(c, tool) {
        0.
    } else {
        float(c, "additional_cooling_fan_speed", tool)
            .unwrap_or(0.)
            .trunc()
    }
}

impl State {
    /// Start a layer (or a filament change within it) with `fan`, the layer fan percent.
    pub fn begin(&mut self, c: &Config, tool: usize, layer: usize, fan: f64, new_layer: bool) {
        if new_layer {
            self.requests = [false; 4];
        }
        self.layer = layer;
        let typed = |key: &str| c.float_at(key, tool).unwrap_or(0.);
        let close = closed_layers(c, tool);
        let mut speeds = Speeds {
            fan,
            ..Speeds::default()
        };
        if layer >= close {
            let mut overhang = float(c, "overhang_fan_speed", tool).unwrap_or(100.);
            let full = typed("full_fan_speed_layer").max(0.) as usize;
            if layer + 1 < full {
                let factor = (layer + 1 - close) as f64 / (full - close) as f64;
                overhang = (overhang * factor + 0.5).floor().clamp(0., 255.);
            }
            speeds.region[OVERHANG] = overhang;
            speeds.control[OVERHANG] = overhang > fan;
            let internal = float(c, "internal_bridge_fan_speed", tool).unwrap_or(-1.);
            (
                speeds.region[INTERNAL_BRIDGE],
                speeds.control[INTERNAL_BRIDGE],
            ) = if internal < 0. {
                (overhang, speeds.control[OVERHANG])
            } else {
                (internal, true)
            };
            let support = float(c, "support_material_interface_fan_speed", tool).unwrap_or(-1.);
            speeds.region[SUPPORT_INTERFACE] = support;
            speeds.control[SUPPORT_INTERFACE] = support >= 0.;
            let ironing = float(c, "ironing_fan_speed", tool).unwrap_or(-1.);
            speeds.region[IRONING] = ironing;
            speeds.control[IRONING] = ironing >= 0.;
        }
        self.speeds = speeds;
    }

    /// `change_extruder_set_fan(true)` for `tool` (the layer's current filament at a layer start,
    /// or the new one at its T line): the region speeds, then the layer fan `fan` and the
    /// auxiliary fan where they differ from the ones CoolingBuffer set last.
    pub fn change_extruder(
        &mut self,
        w: &mut Writer,
        c: &Config,
        tool: usize,
        layer: usize,
        fan: f64,
        new_layer: bool,
    ) {
        self.begin(c, tool, layer, fan, new_layer);
        if self.fan != Some(fan) {
            self.fan = Some(fan);
            self.current = Some(fan);
            w.force_fan(fan);
        }
        let auxiliary = auxiliary_speed(c, tool, layer);
        if self.auxiliary != Some(auxiliary) {
            self.auxiliary = Some(auxiliary);
            if c.bool("auxiliary_fan").unwrap_or(false) {
                w.force_auxiliary_fan(auxiliary);
            }
        }
    }

    /// `;_FORCE_RESUME_FAN_SPEED` after custom toolchange G-code: the auxiliary fan again, then
    /// the current part fan unless a region request holds it.
    pub fn force_resume(&mut self, w: &mut Writer, c: &Config) {
        if let Some(auxiliary) = self.auxiliary
            && c.bool("auxiliary_fan").unwrap_or(false)
        {
            w.force_auxiliary_fan(auxiliary);
        }
        if self.fan.is_some()
            && !self.requests.iter().any(|&r| r)
            && let Some(current) = self.current
        {
            w.force_fan(current);
        }
    }

    /// Apply a section's marker decisions in `_extrude`'s order, writing what the CoolingBuffer
    /// writes for each marker.
    pub fn apply(&mut self, w: &mut Writer, markers: Markers) {
        for (kind, on) in markers.into_iter().enumerate() {
            let Some(on) = on else { continue };
            let start = on && (!self.on[kind] || self.marker_layer[kind] != Some(self.layer));
            let end = !on && self.on[kind];
            self.on[kind] = on;
            self.marker_layer[kind] = on.then_some(self.layer);
            let control = self.speeds.control[kind];
            let need = if start {
                let need = control && !self.requests[kind];
                if need {
                    self.requests[kind] = true;
                }
                need
            } else if end {
                if kind == SUPPORT_INTERFACE && !self.requests[kind] {
                    // `TYPE_SUPPORT_INTERFACE_FAN_END` only acts on an active request.
                    false
                } else {
                    if control {
                        self.requests[kind] = false;
                    }
                    true
                }
            } else {
                false
            };
            if need {
                // A region speed becomes `m_current_fan_speed`; the layer fan does not.
                let speed = match (0..4).find(|&k| self.requests[k]) {
                    Some(k) => {
                        self.current = Some(self.speeds.region[k]);
                        self.speeds.region[k]
                    }
                    None => self.fan.unwrap_or(self.speeds.fan),
                };
                w.force_fan(speed);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn config() -> Config {
        let mut c = Config::defaults();
        for (k, v) in [
            ("enable_overhang_bridge_fan", "1"),
            ("overhang_fan_threshold", "0%"),
            ("overhang_fan_speed", "100"),
            ("disable_fan_first_layers", "1"),
            ("gcode_flavor", "klipper"),
        ] {
            c.set_raw(k, v).unwrap();
        }
        c
    }
    #[test]
    fn every_overhang_region_end_rewrites_the_layer_fan() {
        let c = config();
        let mut w = Writer::with_config(crate::Flavor::Klipper, &c);
        let mut fan = State::default();
        // First layer: fan off, each external wall end still writes M106 S0.
        fan.begin(&c, 0, 0, 0., true);
        for role in [
            Role::ExternalPerimeter,
            Role::Perimeter,
            Role::ExternalPerimeter,
            Role::SolidInfill,
        ] {
            fan.apply(&mut w, markers(&c, 0, role, false, (1., 1.)));
        }
        assert_eq!(w.as_str().matches("M106 S0").count(), 2);
        // Later layer at full fan: the overhang fan is not above it, but every end writes it again.
        fan.begin(&c, 0, 3, 100., true);
        for role in [
            Role::ExternalPerimeter,
            Role::Perimeter,
            Role::BridgeInfill,
            Role::SolidInfill,
        ] {
            fan.apply(&mut w, markers(&c, 0, role, false, (1., 1.)));
        }
        assert_eq!(w.as_str().matches("M106 S255").count(), 2);
    }

    /// Regression: `additional_cooling_fan_speed` and `close_fan_the_first_x_layers` are
    /// per-filament (`EXTRUDER_CONFIG`), not the first filament's for every tool.
    #[test]
    fn each_filament_sets_its_own_fans_when_they_change() {
        use mica_core::config::{ConfigValue, Value};
        let mut c = config();
        c.set_raw("disable_fan_first_layers", "1,3").unwrap();
        c.set_raw("additional_cooling_fan_speed", "30,90").unwrap();
        c.set("auxiliary_fan", ConfigValue::Scalar(Value::Bool(true)));
        assert_eq!(auxiliary_speed(&c, 0, 1), 30.);
        assert_eq!(auxiliary_speed(&c, 1, 1), 0.);
        assert_eq!(auxiliary_speed(&c, 1, 3), 90.);
        let mut w = Writer::with_config(crate::Flavor::Klipper, &c);
        let mut fan = State::default();
        // CoolingBuffer starts without fan speeds: the first layer writes both.
        fan.change_extruder(&mut w, &c, 0, 0, 0., true);
        assert_eq!(w.as_str(), "M106 S0\nM106 P2 S0\n");
        // Layer 2 on tool 0, then tool 1 (still closed) and back.
        fan.change_extruder(&mut w, &c, 0, 1, 40., true);
        fan.change_extruder(&mut w, &c, 1, 1, 0., false);
        fan.change_extruder(&mut w, &c, 0, 1, 40., false);
        // Unchanged fans are not written again.
        fan.change_extruder(&mut w, &c, 0, 1, 40., false);
        assert_eq!(
            w.as_str(),
            "M106 S0\nM106 P2 S0\nM106 S102\nM106 P2 S76\nM106 S0\nM106 P2 S0\nM106 S102\nM106 P2 S76\n"
        );
        // ;_FORCE_RESUME_FAN_SPEED writes the auxiliary fan, then the current fan.
        let mut w = Writer::with_config(crate::Flavor::Klipper, &c);
        fan.force_resume(&mut w, &c);
        assert_eq!(w.as_str(), "M106 P2 S76\nM106 S102\n");
    }
}
