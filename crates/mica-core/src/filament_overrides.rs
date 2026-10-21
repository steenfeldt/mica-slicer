// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: prusaslicer@b028299c77 src/libslic3r/PrintApply.cpp :: print_config_diffs
// Source: prusaslicer@b028299c77 src/libslic3r/Config.hpp :: ConfigOptionVector::apply_override, ConfigOptionVector::is_nil
// Source: prusaslicer@b028299c77 src/libslic3r/PrintConfig.cpp :: PrintConfigDef::init_extruder_option_keys
// Changes: Returns an overridden copy of the whole configuration (Prusa layers the copies over the
// Changes: placeholder parser's full config and stores them in the print's PrintConfig). The
// Changes: `filament_*` keys themselves stay unchanged, as upstream.

//! Filament presets override the printer's per-extruder retraction values.
//!
//! PrusaSlicer declares a nullable `filament_<key>` for every printer retraction key. A non-nil
//! element replaces the printer's value for that extruder; a `nil` element keeps the printer value.
//! The overridden values are what G-code generation (`GCodeWriter`, `Extruder`) and custom G-code
//! templates see, e.g. `retract_length_toolchange[1]` in the XL start G-code.

use crate::config::{Config, ConfigValue};

/// `PrintConfigDef::m_extruder_retract_keys` (sorted, as upstream asserts).
pub const EXTRUDER_RETRACT_KEYS: [&str; 17] = [
    "deretract_speed",
    "retract_before_travel",
    "retract_before_wipe",
    "retract_layer_change",
    "retract_length",
    "retract_length_toolchange",
    "retract_lift",
    "retract_lift_above",
    "retract_lift_below",
    "retract_restart_extra",
    "retract_restart_extra_toolchange",
    "retract_speed",
    "travel_lift_before_obstacle",
    "travel_max_lift",
    "travel_ramping_lift",
    "travel_slope",
    "wipe",
];

/// Element-wise override of a printer vector by a nullable filament vector
/// (`ConfigOptionVector::apply_override`). Returns whether anything changed.
fn apply_override(
    printer: &mut Vec<Option<crate::config::Value>>,
    filament: &[Option<crate::config::Value>],
) -> bool {
    let mut modified = false;
    let count = printer.len().min(filament.len());
    for i in 0..count {
        if let Some(v) = &filament[i]
            && printer[i].as_ref() != Some(v)
        {
            printer[i] = Some(v.clone());
            modified = true;
        }
    }
    for (i, item) in filament.iter().enumerate().skip(count) {
        if let Some(v) = item {
            let front = printer.first().cloned().flatten();
            printer.resize(i + 1, front);
            printer[i] = Some(v.clone());
            modified = true;
        }
    }
    modified
}

/// The configuration G-code generation uses: every printer retraction key with the non-nil
/// filament overrides applied (`print_config_diffs`). Configurations without overrides are
/// returned unchanged.
pub fn apply(c: &Config) -> Config {
    let mut out = c.clone();
    apply_in_place(&mut out);
    out
}

/// In-place variant of [`apply`]; returns whether any value changed.
pub fn apply_in_place(c: &mut Config) -> bool {
    let mut changed = false;
    for key in EXTRUDER_RETRACT_KEYS {
        let Some(ConfigValue::Vector(filament)) = c.get(&format!("filament_{key}")) else {
            continue;
        };
        // An all-nil override is no override (`ConfigOptionVector::is_nil`).
        if filament.iter().all(Option::is_none) {
            continue;
        }
        let filament = filament.clone();
        let mut printer = match c.get(key) {
            Some(ConfigValue::Vector(v)) => v.clone(),
            Some(ConfigValue::Scalar(v)) => vec![Some(v.clone())],
            None => continue,
        };
        if apply_override(&mut printer, &filament) {
            c.set(key, ConfigValue::Vector(printer));
            changed = true;
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_nil_elements_override_per_extruder() {
        let mut c = Config::defaults();
        c.set_raw("retract_length_toolchange", "1.2,1.2,1.2")
            .unwrap();
        c.set_raw("filament_retract_length_toolchange", "20,nil,6")
            .unwrap();
        c.set_raw("retract_length", "0.8,0.8,0.8").unwrap();
        c.set_raw("filament_retract_length", "nil,nil,nil").unwrap();
        c.set_raw("wipe", "1,1,1").unwrap();
        c.set_raw("filament_wipe", "nil,0,nil").unwrap();
        let o = apply(&c);
        assert_eq!(o.raw("retract_length_toolchange").unwrap(), "20,1.2,6");
        assert_eq!(o.raw("retract_length").unwrap(), "0.8,0.8,0.8");
        assert_eq!(o.raw("wipe").unwrap(), "1,0,1");
        // The override keys themselves are kept for templates such as is_nil(filament_...).
        assert_eq!(
            o.raw("filament_retract_length_toolchange").unwrap(),
            "20,nil,6"
        );
    }

    #[test]
    fn longer_filament_vector_extends_from_the_first_printer_value() {
        let mut c = Config::defaults();
        c.set_raw("retract_restart_extra_toolchange", "0.5")
            .unwrap();
        c.set_raw("filament_retract_restart_extra_toolchange", "nil,nil,2")
            .unwrap();
        assert_eq!(
            apply(&c).raw("retract_restart_extra_toolchange").unwrap(),
            "0.5,0.5,2"
        );
    }

    #[test]
    fn all_nil_is_no_override() {
        let mut c = Config::defaults();
        c.set_raw("retract_length_toolchange", "4").unwrap();
        c.set_raw("filament_retract_length_toolchange", "nil")
            .unwrap();
        assert!(!apply_in_place(&mut c));
        assert_eq!(c.raw("retract_length_toolchange").unwrap(), "4");
    }
}
