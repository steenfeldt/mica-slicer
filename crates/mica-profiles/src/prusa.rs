// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/PresetBundle.cpp :: flatten_configbundle_hierarchy, PresetBundle::full_fff_config
// Source: prusaslicer@b028299c77 src/libslic3r/Preset.cpp :: is_compatible_with_printer, is_compatible_with_print
// Source: prusaslicer@b028299c77 src/libslic3r/PrintConfig.cpp :: obsolete_keys (wipe tower placement moved to Model in 2.9)
// Changes: Inheritance is resolved lazily per requested preset instead of flattening the whole tree;
// Changes: same precedence (own keys, then parents with later parents winning, "renamed_from" not inherited).
// Changes: Custom printers (user printers without a system parent) default to OrcaSlicer rules.
// Changes: The compatibility filter flattens only the compatibility keys of each preset (same inheritance walk and errors).

//! PrusaSlicer vendor bundles (`PrusaResearch.ini`): presets with multiple inheritance.

use std::collections::{BTreeMap, HashMap};
use std::fmt;

use mica_core::config::{Config, ParseError, Scope, option_defs};

use crate::ini::{self, Section};

/// Preset types in a bundle.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    Print,
    Filament,
    Printer,
}

impl Kind {
    fn prefix(self) -> &'static str {
        match self {
            Kind::Print => "print",
            Kind::Filament => "filament",
            Kind::Printer => "printer",
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum BundleError {
    UnknownPreset {
        kind: Kind,
        name: String,
    },
    Cycle {
        kind: Kind,
        name: String,
    },
    Value(ParseError),
    Incompatible {
        kind: Kind,
        name: String,
        with: String,
    },
}

impl fmt::Display for BundleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BundleError::UnknownPreset { kind, name } => {
                write!(f, "unknown {} preset \"{name}\"", kind.prefix())
            }
            BundleError::Cycle { kind, name } => write!(
                f,
                "{} preset \"{name}\" inherits from itself",
                kind.prefix()
            ),
            BundleError::Value(e) => write!(f, "{e}"),
            BundleError::Incompatible { kind, name, with } => {
                write!(
                    f,
                    "{} preset \"{name}\" is not compatible with \"{with}\"",
                    kind.prefix()
                )
            }
        }
    }
}

impl std::error::Error for BundleError {}

/// A loaded vendor bundle.
pub struct Bundle {
    /// `[vendor]` section entries (name, config_version, …).
    pub vendor: BTreeMap<String, String>,
    /// `[printer_model:ID]` sections.
    pub printer_models: BTreeMap<String, BTreeMap<String, String>>,
    presets: HashMap<(Kind, String), Section>,
    /// User printer presets with no system parent (custom printers): they slice like
    /// OrcaSlicer unless they record `mica_slicing_rules` themselves.
    custom_printers: std::collections::HashSet<String>,
}

/// Raw key → value pairs of a preset after inheritance.
pub type Flat = BTreeMap<String, String>;

/// The keys [`Bundle::compatible_presets`] reads from a flattened preset.
const COMPATIBILITY_KEYS: [&str; 4] = [
    "compatible_printers",
    "compatible_printers_condition",
    "compatible_prints",
    "compatible_prints_condition",
];

impl Bundle {
    pub fn parse(text: &str) -> Self {
        let mut vendor = BTreeMap::new();
        let mut printer_models = BTreeMap::new();
        let mut presets = HashMap::new();
        for section in ini::parse(text) {
            let (head, name) = section
                .name
                .split_once(':')
                .unwrap_or((section.name.as_str(), ""));
            let kind = match head {
                "print" => Some(Kind::Print),
                "filament" => Some(Kind::Filament),
                "printer" => Some(Kind::Printer),
                "vendor" => {
                    vendor.extend(section.entries.iter().cloned());
                    None
                }
                "printer_model" => {
                    printer_models
                        .insert(name.to_string(), section.entries.iter().cloned().collect());
                    None
                }
                _ => None,
            };
            if let Some(kind) = kind {
                presets.insert((kind, name.to_string()), section);
            }
        }
        Self {
            vendor,
            printer_models,
            presets,
            custom_printers: Default::default(),
        }
    }

    /// Adds a user preset: `overrides` on top of `inherits` (resolved like any other preset).
    /// Replaces an existing preset of the same kind and name.
    pub fn add_user_preset(
        &mut self,
        kind: Kind,
        name: &str,
        inherits: &str,
        overrides: &BTreeMap<String, String>,
    ) {
        let mut entries = Vec::with_capacity(overrides.len() + 1);
        if !inherits.is_empty() {
            entries.push((
                "inherits".to_string(),
                mica_core::config::escape_strings(&[inherits.to_string()]),
            ));
        }
        entries.extend(overrides.iter().map(|(k, v)| (k.clone(), v.clone())));
        let section = Section {
            name: format!("{}:{name}", kind.prefix()),
            entries,
        };
        if kind == Kind::Printer && inherits.is_empty() {
            self.custom_printers.insert(name.to_string());
        } else if kind == Kind::Printer {
            self.custom_printers.remove(name);
        }
        self.presets.insert((kind, name.to_string()), section);
    }

    /// Removes a preset added by `add_user_preset` (Mica user profiles only: callers never
    /// pass a vendor preset's name).
    pub fn remove_user_preset(&mut self, kind: Kind, name: &str) {
        self.presets.remove(&(kind, name.to_string()));
        if kind == Kind::Printer {
            self.custom_printers.remove(name);
        }
    }

    /// Names of the user-visible presets of `kind` (abstract `*name*` presets excluded).
    pub fn preset_names(&self, kind: Kind) -> Vec<&str> {
        let mut names: Vec<&str> = self
            .presets
            .keys()
            .filter(|(k, n)| *k == kind && !n.starts_with('*'))
            .map(|(_, n)| n.as_str())
            .collect();
        names.sort_unstable();
        names
    }

    /// The preset's keys after applying its inheritance chain.
    pub fn flatten(&self, kind: Kind, name: &str) -> Result<Flat, BundleError> {
        self.flatten_inner(kind, name, None, &mut Vec::new())
    }

    /// [`Bundle::flatten`] restricted to `keys`: the same inheritance walk and errors, but
    /// only these keys are collected (the compatibility filters read only their conditions).
    fn flatten_keys(&self, kind: Kind, name: &str, keys: &[&str]) -> Result<Flat, BundleError> {
        self.flatten_inner(kind, name, Some(keys), &mut Vec::new())
    }

    fn flatten_inner(
        &self,
        kind: Kind,
        name: &str,
        keys: Option<&[&str]>,
        stack: &mut Vec<String>,
    ) -> Result<Flat, BundleError> {
        let section = self.presets.get(&(kind, name.to_string())).ok_or_else(|| {
            BundleError::UnknownPreset {
                kind,
                name: name.to_string(),
            }
        })?;
        if stack.iter().any(|n| n == name) {
            return Err(BundleError::Cycle {
                kind,
                name: name.to_string(),
            });
        }
        stack.push(name.to_string());
        let mut flat = Flat::new();
        if let Some(inherits) = section.get("inherits") {
            let parents = mica_core::config::unescape_strings(inherits).unwrap_or_default();
            for parent in parents.iter().map(|p| p.trim()).filter(|p| !p.is_empty()) {
                // Later parents override earlier ones.
                for (k, v) in self.flatten_inner(kind, parent, keys, stack)? {
                    if k != "renamed_from" {
                        flat.insert(k, v);
                    }
                }
            }
        }
        for (k, v) in &section.entries {
            if k != "inherits" && keys.is_none_or(|keys| keys.contains(&k.as_str())) {
                flat.insert(k.clone(), v.clone());
            }
        }
        stack.pop();
        Ok(flat)
    }

    /// A preset's own config on top of the defaults (what PrusaSlicer evaluates conditions against).
    fn preset_config(&self, kind: Kind, name: &str) -> Result<Config, BundleError> {
        let mut config = Config::defaults();
        for (k, v) in self.flatten(kind, name)? {
            if !is_preset_attribute(&k) {
                config.set_raw(&k, &v).map_err(BundleError::Value)?;
            }
        }
        Ok(config)
    }

    /// Template context of a printer preset for compatibility conditions (with PrusaSlicer's extra
    /// `printer_preset` and `num_extruders` variables).
    fn printer_condition_context(
        &self,
        printer: &str,
    ) -> Result<mica_template::Context, BundleError> {
        let printer_config = self.preset_config(Kind::Printer, printer)?;
        let mut ctx = mica_gcode::templates::config_context(&printer_config);
        ctx.set("printer_preset", mica_template::Var::string(printer));
        ctx.set(
            "num_extruders",
            mica_template::Var::int(printer_config.vector_len("nozzle_diameter") as i64),
        );
        Ok(ctx)
    }

    /// Shared rule: an explicit list wins; otherwise the condition decides (empty or unparsable
    /// conditions count as compatible, as in PrusaSlicer).
    fn listed_or_condition(
        flat: &Flat,
        list_key: &str,
        condition_key: &str,
        target: &str,
        eval: &mut dyn FnMut(&str) -> Result<bool, BundleError>,
    ) -> Result<bool, BundleError> {
        let list = flat.get(list_key).map(String::as_str).unwrap_or("");
        let list = mica_core::config::unescape_strings(list).unwrap_or_default();
        if !list.is_empty() {
            return Ok(list.iter().any(|p| p == target));
        }
        let condition = flat.get(condition_key).map(String::as_str).unwrap_or("");
        if condition.trim().is_empty() {
            return Ok(true);
        }
        eval(condition)
    }

    /// Whether a print or filament preset may be used with `printer`, following PrusaSlicer:
    /// an explicit `compatible_printers` list wins, otherwise `compatible_printers_condition` is
    /// evaluated against the printer. A condition that fails to parse counts as compatible, as in
    /// PrusaSlicer.
    pub fn is_compatible_with_printer(
        &self,
        kind: Kind,
        name: &str,
        printer: &str,
    ) -> Result<bool, BundleError> {
        let flat = self.flatten(kind, name)?;
        Self::listed_or_condition(
            &flat,
            "compatible_printers",
            "compatible_printers_condition",
            printer,
            &mut |cond| {
                let mut ctx = self.printer_condition_context(printer)?;
                Ok(mica_template::evaluate_boolean(cond, &mut ctx).unwrap_or(true))
            },
        )
    }

    /// Whether a filament preset may be used with `print` (`compatible_prints[_condition]`).
    pub fn is_compatible_with_print(
        &self,
        filament: &str,
        print: &str,
    ) -> Result<bool, BundleError> {
        let flat = self.flatten(Kind::Filament, filament)?;
        Self::listed_or_condition(
            &flat,
            "compatible_prints",
            "compatible_prints_condition",
            print,
            &mut |cond| {
                let mut ctx =
                    mica_gcode::templates::config_context(&self.preset_config(Kind::Print, print)?);
                Ok(mica_template::evaluate_boolean(cond, &mut ctx).unwrap_or(true))
            },
        )
    }

    /// Presets of `kind` usable with `printer` (and, for filaments, with `print` if given).
    /// The printer and print contexts are built once and reused for every candidate.
    pub fn compatible_presets(&self, kind: Kind, printer: &str, print: Option<&str>) -> Vec<&str> {
        let Ok(mut printer_ctx) = self.printer_condition_context(printer) else {
            return Vec::new();
        };
        let mut print_ctx = match print {
            Some(p) if kind == Kind::Filament => match self.preset_config(Kind::Print, p) {
                Ok(c) => Some(mica_gcode::templates::config_context(&c)),
                Err(_) => return Vec::new(),
            },
            _ => None,
        };
        self.preset_names(kind)
            .into_iter()
            .filter(|name| {
                let Ok(flat) = self.flatten_keys(kind, name, &COMPATIBILITY_KEYS) else {
                    return false;
                };
                let on_printer = Self::listed_or_condition(
                    &flat,
                    "compatible_printers",
                    "compatible_printers_condition",
                    printer,
                    &mut |cond| {
                        Ok(mica_template::evaluate_boolean(cond, &mut printer_ctx).unwrap_or(true))
                    },
                );
                let on_print = match (&mut print_ctx, print) {
                    (Some(ctx), Some(p)) => Self::listed_or_condition(
                        &flat,
                        "compatible_prints",
                        "compatible_prints_condition",
                        p,
                        &mut |cond| Ok(mica_template::evaluate_boolean(cond, ctx).unwrap_or(true)),
                    ),
                    _ => Ok(true),
                };
                on_printer.unwrap_or(false) && on_print.unwrap_or(false)
            })
            .collect()
    }

    /// A complete single-extruder print configuration from a printer, a print and a filament preset,
    /// combined the way PrusaSlicer's `full_fff_config` does: defaults ← print ← printer ← filament,
    /// vectors normalised to the extruder count, plus the bookkeeping keys PrusaSlicer adds.
    pub fn full_config(
        &self,
        printer: &str,
        print: &str,
        filament: &str,
    ) -> Result<Config, BundleError> {
        let incompatible = |kind, name: &str, with: &str| BundleError::Incompatible {
            kind,
            name: name.to_string(),
            with: with.to_string(),
        };
        if !self.is_compatible_with_printer(Kind::Print, print, printer)? {
            return Err(incompatible(Kind::Print, print, printer));
        }
        if !self.is_compatible_with_printer(Kind::Filament, filament, printer)? {
            return Err(incompatible(Kind::Filament, filament, printer));
        }
        if !self.is_compatible_with_print(filament, print)? {
            return Err(incompatible(Kind::Filament, filament, print));
        }
        let print_flat = self.flatten(Kind::Print, print)?;
        let printer_flat = self.flatten(Kind::Printer, printer)?;
        let filament_flat = self.flatten(Kind::Filament, filament)?;

        let mut config = Config::defaults();
        for flat in [&print_flat, &printer_flat, &filament_flat] {
            for (k, v) in flat {
                // Prusa 2.9 moved placement from presets into ModelWipeTower. Old vendor
                // bundles still contain these ignored keys; explicit project/CLI values
                // are applied after profile composition.
                if !is_preset_attribute(k)
                    && !matches!(
                        k.as_str(),
                        "wipe_tower_x" | "wipe_tower_y" | "wipe_tower_rotation_angle"
                    )
                {
                    config.set_raw(k, v).map_err(BundleError::Value)?;
                }
            }
        }

        // A Prusa bundle printer slices like PrusaSlicer (the option's default); a custom
        // printer like OrcaSlicer, unless either records its own choice.
        if self.custom_printers.contains(printer)
            && !printer_flat.contains_key(mica_core::rules::KEY)
        {
            mica_core::rules::Rules::OrcaSlicer.set(&mut config);
        }

        // Per-extruder and per-filament vectors get one entry per nozzle (Preset::normalize).
        let extruders = config.vector_len("nozzle_diameter").max(1);
        let keys: Vec<String> = config.values().map(|(k, _)| k.to_string()).collect();
        for key in &keys {
            let def = option_defs().get(key.as_str());
            let per_extruder = EXTRUDER_OPTION_KEYS.contains(&key.as_str());
            let per_filament = def.is_some_and(|d| d.scope == Scope::Filament && d.ty.is_vector());
            if key == "default_filament_profile" {
                continue;
            }
            if per_filament {
                // One filament chosen for a multi-tool printer is used on every tool (as PrusaSlicer's
                // CLI does when a single filament preset is given).
                config.resize_vector_repeat(key, extruders);
            } else if per_extruder {
                config.resize_vector(key, extruders);
            }
        }
        // The project's wipe volumes: an n×n matrix, 0 on the diagonal.
        let matrix: Vec<String> = (0..extruders * extruders)
            .map(|i| if i % (extruders + 1) == 0 { "0" } else { "140" }.to_string())
            .collect();
        config
            .set_raw("wiping_volumes_matrix", &matrix.join(","))
            .map_err(BundleError::Value)?;

        let set = |config: &mut Config, key: &str, raw: &str| {
            config.set_raw(key, raw).map_err(BundleError::Value)
        };
        let strings = |items: &[&str]| {
            mica_core::config::escape_strings(
                &items.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            )
        };
        set(&mut config, "print_settings_id", print)?;
        set(&mut config, "filament_settings_id", &strings(&[filament]))?;
        set(&mut config, "printer_settings_id", printer)?;
        set(&mut config, "physical_printer_settings_id", "")?;

        // Conditions and parents of the selected presets, kept for reference in the G-code.
        let attr =
            |flat: &Flat, key: &str| flat.get(key).map(String::as_str).unwrap_or("").to_string();
        let cumulative = [
            (
                "compatible_printers_condition_cummulative",
                vec![
                    attr(&print_flat, "compatible_printers_condition"),
                    attr(&filament_flat, "compatible_printers_condition"),
                ],
            ),
            (
                "compatible_prints_condition_cummulative",
                vec![attr(&filament_flat, "compatible_prints_condition")],
            ),
        ];
        for (key, values) in cumulative {
            if values.iter().any(|v| !v.is_empty()) {
                let refs: Vec<&str> = values.iter().map(String::as_str).collect();
                set(&mut config, key, &strings(&refs))?;
            }
        }
        Ok(config)
    }
}

/// PrusaSlicer's `extruder_option_keys`: printer options with one value per extruder.
const EXTRUDER_OPTION_KEYS: &[&str] = &[
    "nozzle_diameter",
    "min_layer_height",
    "max_layer_height",
    "extruder_offset",
    "retract_length",
    "retract_lift",
    "retract_lift_above",
    "retract_lift_below",
    "retract_speed",
    "deretract_speed",
    "retract_before_wipe",
    "retract_restart_extra",
    "retract_before_travel",
    "wipe",
    "travel_slope",
    "travel_max_lift",
    "travel_ramping_lift",
    "travel_lift_before_obstacle",
    "retract_layer_change",
    "retract_length_toolchange",
    "retract_restart_extra_toolchange",
    "extruder_colour",
    "default_filament_profile",
    "nozzle_high_flow",
];

/// Preset attributes rather than print settings; PrusaSlicer keeps them out of the full config
/// (conditions reappear in the `*_cummulative` keys).
fn is_preset_attribute(key: &str) -> bool {
    matches!(
        key,
        "compatible_printers"
            | "compatible_printers_condition"
            | "compatible_prints"
            | "compatible_prints_condition"
            | "renamed_from"
            | "alias"
            | "inherits"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const BUNDLE: &str = "\
[vendor]
name = Test

[printer:*common*]
nozzle_diameter = 0.4
retract_length = 0.8
printer_model = X

[printer:*hf*]
retract_length = 0.7
renamed_from = Old

[printer:Real]
inherits = *common*; *hf*
bed_shape = 0x0,250x0,250x220,0x220
";

    #[test]
    fn multiple_inheritance_later_parent_wins() {
        let b = Bundle::parse(BUNDLE);
        let flat = b.flatten(Kind::Printer, "Real").unwrap();
        assert_eq!(flat["retract_length"], "0.7");
        assert_eq!(flat["nozzle_diameter"], "0.4");
        assert!(!flat.contains_key("renamed_from"));
        assert!(!flat.contains_key("inherits"));
        assert_eq!(b.preset_names(Kind::Printer), vec!["Real"]);
    }

    #[test]
    fn unknown_and_cyclic_presets_are_errors() {
        let b = Bundle::parse("[printer:A]\ninherits = B\n[printer:B]\ninherits = A\n");
        assert!(matches!(
            b.flatten(Kind::Printer, "A"),
            Err(BundleError::Cycle { .. })
        ));
        assert!(matches!(
            b.flatten(Kind::Printer, "Z"),
            Err(BundleError::UnknownPreset { .. })
        ));
    }
}
