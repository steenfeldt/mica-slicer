// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Which slicer's behaviour Mica follows: PrusaSlicer 2.9.6 or OrcaSlicer.
//!
//! Mica carries two engines in one codebase. Every place that picks one of them asks
//! [`Rules::of`], which reads the printer option `mica_slicing_rules`. Firmware commands
//! (EXCLUDE_OBJECT, SET_VELOCITY_LIMIT, macros, M486, arc syntax) follow `gcode_flavor` instead.
//! The classification of every decision site is in `docs/slicing-rules.md`.
//!
//! The option's default comes from where the printer profile was loaded from: Prusa bundle
//! printers start with PrusaSlicer rules, Orca vendor profiles (and custom printers) with
//! OrcaSlicer rules (`mica_profiles::orca::map_config` sets it). A config without the key
//! (hand-built tests, `Config::defaults`) follows PrusaSlicer.

use crate::config::Config;

/// The option key, stored in printer presets and projects.
pub const KEY: &str = "mica_slicing_rules";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Rules {
    #[default]
    PrusaSlicer,
    OrcaSlicer,
}

impl Rules {
    pub const ALL: [Rules; 2] = [Rules::PrusaSlicer, Rules::OrcaSlicer];

    /// The rules `c` slices with.
    pub fn of(c: &Config) -> Self {
        c.string(KEY).and_then(Self::parse).unwrap_or_default()
    }

    pub fn is_orca(self) -> bool {
        self == Self::OrcaSlicer
    }

    pub fn is_prusa(self) -> bool {
        self == Self::PrusaSlicer
    }

    /// The option value (`prusaslicer` / `orcaslicer`).
    pub fn name(self) -> &'static str {
        match self {
            Self::PrusaSlicer => "prusaslicer",
            Self::OrcaSlicer => "orcaslicer",
        }
    }

    /// The slicer's display name.
    pub fn label(self) -> &'static str {
        match self {
            Self::PrusaSlicer => "PrusaSlicer",
            Self::OrcaSlicer => "OrcaSlicer",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "prusaslicer" | "prusa" => Some(Self::PrusaSlicer),
            "orcaslicer" | "orca" => Some(Self::OrcaSlicer),
            _ => None,
        }
    }

    /// Records these rules in `c`.
    pub fn set(self, c: &mut Config) {
        c.set_raw(KEY, self.name())
            .expect("mica_slicing_rules is a known enum option");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_prusaslicer_and_values_round_trip() {
        assert_eq!(Rules::of(&Config::new()), Rules::PrusaSlicer);
        assert_eq!(Rules::of(&Config::defaults()), Rules::PrusaSlicer);
        let mut c = Config::defaults();
        Rules::OrcaSlicer.set(&mut c);
        assert_eq!(c.raw(KEY).as_deref(), Some("orcaslicer"));
        assert!(Rules::of(&c).is_orca());
        assert_eq!(Rules::parse("Orca"), Some(Rules::OrcaSlicer));
        assert_eq!(Rules::parse("klipper"), None);
    }

    #[test]
    fn orca_only_keys_do_not_select_the_engine() {
        let mut c = Config::defaults();
        for (k, v) in [
            ("seam_slope_type", "none"),
            ("support_remove_small_overhang", "1"),
            ("bridge_density", "100%"),
            ("gcode_flavor", "klipper"),
        ] {
            c.set_raw(k, v).unwrap();
        }
        assert_eq!(Rules::of(&c), Rules::PrusaSlicer);
    }
}
