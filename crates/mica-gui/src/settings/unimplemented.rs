// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Settings and choices Mica's engine does not implement. The editor hides them (pages,
//! search, the object and part scopes) and leaves their values alone: profiles and projects
//! load and save them unchanged. A profile that sets one to a value with an effect upstream
//! gets a guard-rail notification (`checks::guard_rails`), and a selected unsupported choice
//! still shows, with a "not supported by Mica" note, so nothing changes silently.
//!
//! Each entry was checked against the engine: no reader in `mica-core` / `mica-gcode` (or a
//! reader that refuses the value or falls back).

use mica_core::config::Config;
use mica_core::rules::Rules;

/// Where an unimplemented setting or choice is hidden.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hidden {
    /// On every printer.
    Everywhere,
    /// On Klipper printers (`gcode_flavor = klipper`), where Mica's writer ignores it.
    Klipper,
}

impl Hidden {
    fn holds(self, c: &Config) -> bool {
        match self {
            Hidden::Everywhere => true,
            Hidden::Klipper => c.string("gcode_flavor") == Some("klipper"),
        }
    }
}

/// Under which slicing rules a value other than Mica's behavior changes the upstream G-code
/// (whether a profile value is worth a warning).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Upstream {
    /// Both slicers have the option.
    Both,
    /// Only PrusaSlicer has it; OrcaSlicer's rules ignore it as Mica does.
    PrusaSlicer,
    /// Neither slicer uses it any more (PrusaSlicer 2.6 removed it).
    Neither,
    /// The engine warns about a value with an effect itself, in the CLI's and the slice's
    /// words (`mica_core::safety::unsupported_settings`): no second notice here.
    Warned,
    /// Hidden without a notice: the value only matters where Mica already refuses the print
    /// (single-extruder multi-material), with automatic (zero) speeds, or only in upstream's
    /// own interface and file naming; or only changes one speed (see the entry).
    Quiet,
}

/// A setting Mica does not implement.
#[derive(Clone, Copy, Debug)]
pub struct Setting {
    pub key: &'static str,
    /// The value Mica prints as (the setting's raw text).
    pub inert: &'static str,
    pub hidden: Hidden,
    pub upstream: Upstream,
}

const fn s(key: &'static str, inert: &'static str, hidden: Hidden, upstream: Upstream) -> Setting {
    Setting {
        key,
        inert,
        hidden,
        upstream,
    }
}

use Hidden::{Everywhere, Klipper};

/// The settings Mica hides (see the module comment).
pub const SETTINGS: &[Setting] = &[
    // Quality.
    s(
        "automatic_extrusion_widths",
        "0",
        Everywhere,
        Upstream::PrusaSlicer,
    ),
    s("extra_perimeters", "0", Everywhere, Upstream::PrusaSlicer),
    s(
        "extra_perimeters_on_overhangs",
        "0",
        Everywhere,
        Upstream::Both,
    ),
    // Strength.
    s(
        "infill_only_where_needed",
        "0",
        Everywhere,
        Upstream::Neither,
    ),
    s(
        "automatic_infill_combination",
        "0",
        Everywhere,
        Upstream::PrusaSlicer,
    ),
    // Only used with automatic infill combination.
    s(
        "automatic_infill_combination_max_layer_height",
        "100%",
        Everywhere,
        Upstream::Neither,
    ),
    // Multimaterial.
    s("interface_shells", "0", Everywhere, Upstream::Both),
    // Others.
    s(
        "avoid_crossing_curled_overhangs",
        "0",
        Everywhere,
        Upstream::PrusaSlicer,
    ),
    s(
        "only_retract_when_crossing_perimeters",
        "0",
        Everywhere,
        Upstream::PrusaSlicer,
    ),
    // Verbose G-code: only one Klipper comment is written, none of the rest.
    s("gcode_comments", "0", Everywhere, Upstream::Both),
    // Printer.
    s("use_firmware_retraction", "0", Everywhere, Upstream::Both),
    s(
        "high_current_on_filament_swap",
        "0",
        Everywhere,
        Upstream::Both,
    ),
    // Klipper: progress follows `disable_m73`, and there is no silent mode.
    s("remaining_times", "0", Klipper, Upstream::Both),
    s("silent_mode", "1", Klipper, Upstream::Both),
    // Scripts are not run (on purpose for now); the slice says so.
    s("post_process", "", Everywhere, Upstream::Warned),
    s("gcode_substitutions", "", Everywhere, Upstream::Warned),
    s(
        "filament_shrinkage_compensation_xy",
        "0%",
        Everywhere,
        Upstream::Warned,
    ),
    s(
        "filament_shrinkage_compensation_z",
        "0%",
        Everywhere,
        Upstream::Warned,
    ),
    // PrusaSlicer's first object layer over a raft; the slice warns when there is a raft.
    s(
        "first_layer_speed_over_raft",
        "30",
        Everywhere,
        Upstream::Warned,
    ),
    s(
        "first_layer_acceleration_over_raft",
        "0",
        Everywhere,
        Upstream::Warned,
    ),
    // PrusaSlicer's speed of solid infill over bridges (its InfillOverBridge role, which Mica
    // prints as solid infill at the solid infill speed): a speed only, set by most Prusa
    // profiles, so no notice.
    s("over_bridge_speed", "0", Everywhere, Upstream::Quiet),
    // Only with automatic (zero) speeds.
    s("max_print_speed", "80", Everywhere, Upstream::Quiet),
    // Single-extruder multi-material only (the export refuses MMU prints).
    s("cooling_tube_length", "5", Everywhere, Upstream::Quiet),
    s(
        "cooling_tube_retraction",
        "91.5",
        Everywhere,
        Upstream::Quiet,
    ),
    s("extra_loading_move", "-2", Everywhere, Upstream::Quiet),
    s("parking_pos_retraction", "92", Everywhere, Upstream::Quiet),
    s("multimaterial_purging", "140", Everywhere, Upstream::Quiet),
    s("filament_cooling_moves", "4", Everywhere, Upstream::Quiet),
    s(
        "filament_cooling_initial_speed",
        "2.2",
        Everywhere,
        Upstream::Quiet,
    ),
    s(
        "filament_cooling_final_speed",
        "3.4",
        Everywhere,
        Upstream::Quiet,
    ),
    s("filament_loading_speed", "28", Everywhere, Upstream::Quiet),
    s(
        "filament_loading_speed_start",
        "3",
        Everywhere,
        Upstream::Quiet,
    ),
    s(
        "filament_unloading_speed",
        "90",
        Everywhere,
        Upstream::Quiet,
    ),
    s(
        "filament_unloading_speed_start",
        "100",
        Everywhere,
        Upstream::Quiet,
    ),
    s(
        "filament_ramming_parameters",
        "",
        Everywhere,
        Upstream::Quiet,
    ),
    s(
        "filament_toolchange_delay",
        "0",
        Everywhere,
        Upstream::Quiet,
    ),
    s(
        "filament_stamping_distance",
        "0",
        Everywhere,
        Upstream::Quiet,
    ),
    s(
        "filament_stamping_loading_speed",
        "20",
        Everywhere,
        Upstream::Quiet,
    ),
    s(
        "filament_purge_multiplier",
        "100%",
        Everywhere,
        Upstream::Quiet,
    ),
    s("filament_flush_speed", "0", Everywhere, Upstream::Quiet),
    s("filament_flush_volume", "0", Everywhere, Upstream::Quiet),
    // Upstream's interface and file naming only: Mica's bed and export names do not use them.
    s("bed_custom_model", "", Everywhere, Upstream::Quiet),
    s("bed_custom_texture", "", Everywhere, Upstream::Quiet),
    s(
        "output_filename_format",
        "[input_filename_base].gcode",
        Everywhere,
        Upstream::Quiet,
    ),
    s("variable_layer_height", "1", Everywhere, Upstream::Quiet),
];

/// A choice of an enum setting Mica does not implement.
#[derive(Clone, Copy, Debug)]
pub struct Choice {
    pub key: &'static str,
    pub value: &'static str,
    pub hidden: Hidden,
}

const fn v(key: &'static str, value: &'static str, hidden: Hidden) -> Choice {
    Choice { key, value, hidden }
}

/// The choices Mica hides. A selected one still shows, noted as not supported.
pub const CHOICES: &[Choice] = &[
    // Slices as Regular: nothing reads the mode.
    v("slicing_mode", "even_odd", Everywhere),
    v("slicing_mode", "close_holes", Everywhere),
    // The slice stops with "honeycomb supports are not implemented".
    v("support_material_pattern", "honeycomb", Everywhere),
    // The slice stops with "JPG thumbnails are not supported".
    v("thumbnails_format", "JPG", Everywhere),
    // Klipper gets no machine limits in the G-code (time estimate only).
    v("machine_limits_usage", "emit_to_gcode", Klipper),
];

/// The note next to an unsupported choice that is selected.
pub const NOTE: &str = "not supported by Mica";

/// The entry for `key`, if Mica does not implement it.
pub fn setting(key: &str) -> Option<&'static Setting> {
    SETTINGS.iter().find(|s| s.key == key)
}

/// Whether the editor shows `key` with the configuration `c` (false for a setting Mica does
/// not implement there).
pub fn implemented(key: &str, c: &Config) -> bool {
    setting(key).is_none_or(|s| !s.hidden.holds(c))
}

/// Whether `key` is hidden on every printer (the object and part scopes, which have no
/// printer of their own to ask).
pub fn hidden_everywhere(key: &str) -> bool {
    setting(key).is_some_and(|s| s.hidden == Everywhere)
}

/// Whether the choice `value` of `key` is one Mica implements with the configuration `c`.
/// G-code flavors other than Marlin 2 and Klipper are refused by the writer.
pub fn choice_supported(key: &str, value: &str, c: &Config) -> bool {
    if key == "gcode_flavor" {
        return matches!(value, "marlin2" | "klipper");
    }
    !CHOICES
        .iter()
        .any(|x| x.key == key && x.value == value && x.hidden.holds(c))
}

/// The labels of what the configuration sets that Mica does not implement, with the value
/// (`Verbose G-code: on`, `Slicing mode: Even-odd`): hidden settings at a value with an effect
/// upstream under the printer's rules, and unsupported choices that are selected.
pub fn in_use(c: &Config) -> Vec<String> {
    let rules = Rules::of(c);
    let mut out = Vec::new();
    for s in SETTINGS {
        let matters = match s.upstream {
            Upstream::Both => true,
            Upstream::PrusaSlicer => !rules.is_orca(),
            Upstream::Neither | Upstream::Warned | Upstream::Quiet => false,
        };
        if !matters || !s.hidden.holds(c) {
            continue;
        }
        let Some(raw) = c.raw(s.key) else { continue };
        // Per-tool vectors: any element.
        let values: Vec<&str> = raw.split(',').map(str::trim).collect();
        if let Some(other) = values.iter().find(|x| **x != s.inert) {
            out.push(format!(
                "{}: {}",
                super::label(s.key),
                shown_value(s.key, other)
            ));
        }
    }
    let enum_keys = CHOICES
        .iter()
        .map(|x| x.key)
        .chain(std::iter::once("gcode_flavor"));
    let mut seen = Vec::new();
    for key in enum_keys {
        if seen.contains(&key) {
            continue;
        }
        seen.push(key);
        if let Some(value) = c.string(key)
            && !value.is_empty()
            && !choice_supported(key, value, c)
        {
            out.push(format!(
                "{}: {}",
                super::label(key),
                shown_value(key, value)
            ));
        }
    }
    out
}

/// A value as the editor would show it: a switch's on/off, a choice's label.
fn shown_value(key: &str, raw: &str) -> String {
    let def = mica_core::config::option_defs().get(key);
    if def.is_some_and(|d| {
        matches!(
            d.ty,
            mica_core::config::OptType::Bool | mica_core::config::OptType::Bools
        )
    }) {
        return if raw == "1" { "on" } else { "off" }.into();
    }
    choice_label(key, raw)
}

/// The label of a choice (`Even-odd`), from the full upstream list.
pub fn choice_label(key: &str, raw: &str) -> String {
    super::text()
        .get(key)
        .and_then(|t| t.enums.iter().find(|(v, _)| v == raw))
        .map_or_else(|| crate::panels::editor::prettify(raw), |(_, l)| l.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mica_core::config::option_defs;

    #[test]
    fn entries_are_known_options_with_valid_values() {
        for s in SETTINGS {
            assert!(option_defs().contains_key(s.key), "{}", s.key);
            let mut c = Config::defaults();
            c.set_raw(s.key, s.inert)
                .unwrap_or_else(|e| panic!("{}: {e}", s.key));
            // Hidden settings are hidden everywhere they appear: on the pages and at the
            // object scopes.
            assert!(!implemented(s.key, &{
                let mut k = c.clone();
                k.set_raw("gcode_flavor", "klipper").unwrap();
                k
            }));
        }
        for x in CHOICES {
            let t = super::super::text().get(x.key).expect(x.key);
            assert!(
                t.enums.iter().any(|(v, _)| v == x.value),
                "{}={} is not a choice",
                x.key,
                x.value
            );
        }
    }

    /// PrusaSlicer's defaults on a Marlin 2 printer.
    fn marlin() -> Config {
        let mut c = Config::defaults();
        c.set_raw("gcode_flavor", "marlin2").unwrap();
        c
    }

    #[test]
    fn defaults_and_owner_printers_use_nothing_unimplemented() {
        // PrusaSlicer's defaults keep extra perimeters on; every owner preset turns it off,
        // and Orca rules ignore it.
        let mut c = marlin();
        assert_eq!(in_use(&c).len(), 1, "{:?}", in_use(&c));
        c.set_raw("extra_perimeters", "0").unwrap();
        assert!(in_use(&c).is_empty(), "{:?}", in_use(&c));
        let mut u1 = Config::defaults();
        u1.set_raw("gcode_flavor", "klipper").unwrap();
        Rules::OrcaSlicer.set(&mut u1);
        assert!(in_use(&u1).is_empty(), "{:?}", in_use(&u1));
    }

    /// Post-processing scripts and the other settings the engine warns about itself are
    /// hidden, and the slice's own warning (CLI and GUI alike) replaces the generic notice;
    /// the quiet ones get neither. Spiral vase is printed, so it is offered and not warned.
    #[test]
    fn engine_warned_settings_are_hidden_and_warned_once() {
        let mut c = marlin();
        c.set_raw("extra_perimeters", "0").unwrap();
        assert!(implemented("spiral_vase", &c));
        for key in [
            "post_process",
            "gcode_substitutions",
            "filament_shrinkage_compensation_xy",
            "over_bridge_speed",
            "max_print_speed",
            "cooling_tube_length",
            "filament_ramming_parameters",
            "output_filename_format",
        ] {
            assert!(!implemented(key, &c), "{key}");
        }
        c.set_raw("spiral_vase", "1").unwrap();
        c.set_raw("post_process", "/usr/local/bin/script.sh")
            .unwrap();
        c.set_raw("filament_shrinkage_compensation_xy", "0.5%")
            .unwrap();
        c.set_raw("over_bridge_speed", "35%").unwrap();
        c.set_raw("max_print_speed", "300").unwrap();
        assert!(in_use(&c).is_empty(), "{:?}", in_use(&c));
        let warnings = mica_core::safety::config_warnings(&c);
        assert!(
            !warnings.iter().any(|w| w.contains("Spiral vase")),
            "{warnings:?}"
        );
        for start in [
            "Post-processing scripts aren't run yet",
            "Filament shrinkage compensation isn't applied yet",
        ] {
            assert_eq!(
                warnings.iter().filter(|w| w.starts_with(start)).count(),
                1,
                "{warnings:?}"
            );
        }
        // Defaults warn about nothing.
        assert!(mica_core::safety::unsupported_settings(&marlin()).is_empty());
    }

    #[test]
    fn profile_values_and_choices_are_reported() {
        let mut c = marlin();
        c.set_raw("extra_perimeters", "0").unwrap();
        c.set_raw("gcode_comments", "1").unwrap();
        c.set_raw("slicing_mode", "even_odd").unwrap();
        assert_eq!(
            in_use(&c),
            vec![
                format!("{}: on", super::super::label("gcode_comments")),
                format!("{}: Even-odd", super::super::label("slicing_mode")),
            ]
        );
        // Klipper-only gaps follow the flavor.
        c.set_raw("gcode_comments", "0").unwrap();
        c.set_raw("slicing_mode", "regular").unwrap();
        c.set_raw("machine_limits_usage", "emit_to_gcode").unwrap();
        c.set_raw("remaining_times", "1").unwrap();
        assert!(in_use(&c).is_empty());
        assert!(implemented("remaining_times", &c));
        assert!(choice_supported(
            "machine_limits_usage",
            "emit_to_gcode",
            &c
        ));
        c.set_raw("gcode_flavor", "klipper").unwrap();
        assert_eq!(in_use(&c).len(), 2, "{:?}", in_use(&c));
        assert!(!implemented("remaining_times", &c));
        assert!(!choice_supported(
            "machine_limits_usage",
            "emit_to_gcode",
            &c
        ));
        // A PrusaSlicer-only option is not reported under Orca rules.
        let mut orca = marlin();
        Rules::OrcaSlicer.set(&mut orca);
        orca.set_raw("only_retract_when_crossing_perimeters", "1")
            .unwrap();
        assert!(in_use(&orca).is_empty());
        // Unsupported flavors.
        assert!(!choice_supported("gcode_flavor", "reprapfirmware", &orca));
        assert!(choice_supported("gcode_flavor", "marlin2", &orca));
    }
}
