// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The other engine's settings, when a profile slices with the other slicer's rules.
//!
//! A PrusaSlicer profile has no values for Orca-only options (Z-hop type, scarf joint speed, Orca's
//! bridge density, …). Under OrcaSlicer rules they are taken from OrcaSlicer's own profile for
//! the same printer, process and filament when Orca ships one (its `Prusa` vendor has the
//! CORE One and the XL 5T), otherwise from OrcaSlicer's defaults. Only options PrusaSlicer has
//! no equivalent for are filled; everything the Prusa profile sets stays as it is.
//!
//! The other direction needs no lookup: an Orca profile is mapped onto `Config::defaults`, so
//! every PrusaSlicer-only option (ramping travel lift, cooling slowdown logic, …) already holds
//! PrusaSlicer's default. [`prusa_only_options`] lists them for the GUI note and the docs.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use mica_core::config::{Config, ConfigValue, OrcaTable, option_defs, orca_table};

use crate::orca::{self, Dialect, Kind, MappingResult, mapping::Disposition};

/// Where the filled-in values came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// OrcaSlicer's matching printer, process and filament profiles.
    OrcaProfile {
        printer: String,
        process: String,
        filament: String,
    },
    /// OrcaSlicer's built-in defaults (no matching Orca profile).
    OrcaDefaults,
}

impl std::fmt::Display for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OrcaProfile {
                printer,
                process,
                filament,
            } => write!(
                f,
                "OrcaSlicer's profiles {printer} / {process} / {filament}"
            ),
            Self::OrcaDefaults => f.write_str("OrcaSlicer's defaults"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Filled {
    pub source: Source,
    /// The options that were added, sorted.
    pub keys: Vec<String>,
}

/// Name tokens for matching a PrusaSlicer preset name with OrcaSlicer's spelling of it
/// ("Original Prusa XL - 5T Input Shaper 0.4 nozzle" ~ "Prusa XL 5T 0.4 nozzle",
/// "0.20mm SPEED @COREONE HF0.4" ~ "0.20mm SPEED @CORE One HF 0.4").
fn tokens(name: &str) -> BTreeSet<String> {
    let lower = name
        .to_ascii_lowercase()
        .replace("coreone", "core one")
        .replace("xlis", "xl")
        .replace("mk4is", "mk4")
        .replace("mk3.9is", "mk3.9");
    let mut spaced = String::new();
    let mut previous: Option<char> = None;
    for c in lower.chars() {
        // "hf0.4" → "hf 0.4"
        if let Some(p) = previous
            && p.is_ascii_alphabetic()
            && c.is_ascii_digit()
        {
            spaced.push(' ');
        }
        spaced.push(if c.is_ascii_alphanumeric() || c == '.' {
            c
        } else {
            ' '
        });
        previous = Some(c);
    }
    spaced
        .split_whitespace()
        .filter(|t| {
            !matches!(
                *t,
                "original" | "prusa" | "input" | "shaper" | "nozzle" | "is"
            )
        })
        .map(str::to_string)
        .collect()
}

fn similarity(a: &str, b: &str) -> f64 {
    let (a, b) = (tokens(a), tokens(b));
    let union = a.union(&b).count();
    if union == 0 {
        return 0.;
    }
    a.intersection(&b).count() as f64 / union as f64
}

/// The best-matching name among `candidates` (similarity at least `min`).
pub fn best_match<'a>(wanted: &str, candidates: &[&'a str], min: f64) -> Option<&'a str> {
    candidates
        .iter()
        .map(|c| (similarity(wanted, c), *c))
        .filter(|(s, _)| *s >= min)
        .max_by(|a, b| a.0.total_cmp(&b.0).then_with(|| b.1.cmp(a.1)))
        .map(|(_, c)| c)
}

/// OrcaSlicer's printer, process and filament presets for a PrusaSlicer selection.
pub fn orca_counterpart(
    bundle: &orca::Bundle,
    printer: &str,
    process: &str,
    filament: &str,
) -> Option<(String, String, String)> {
    let machines: Vec<&str> = bundle
        .preset_names(Kind::Machine)
        .into_iter()
        .filter(|m| {
            bundle
                .preset(Kind::Machine, m)
                .is_some_and(|p| p.instantiated)
        })
        .collect();
    let machine = best_match(printer, &machines, 0.75)?;
    // The printer decides; a process or filament (a user's own, say) without an Orca
    // counterpart falls back to the Orca printer's default one.
    let flat = bundle.flatten(Kind::Machine, machine).ok()?;
    let default = |key: &str| {
        flat.get(key).and_then(|v| match v {
            serde_json::Value::String(s) => Some(s.clone()),
            serde_json::Value::Array(a) => a.first().and_then(|s| s.as_str()).map(str::to_string),
            _ => None,
        })
    };
    let processes = bundle.compatible_presets(Kind::Process, machine, None);
    let process = best_match(process, &processes, 0.5)
        .map(str::to_string)
        .or_else(|| default("default_print_profile").filter(|p| processes.contains(&p.as_str())))?;
    let filaments = bundle.compatible_presets(Kind::Filament, machine, Some(&process));
    let filament = best_match(filament, &filaments, 0.5)
        .map(str::to_string)
        .or_else(|| {
            default("default_filament_profile").filter(|f| filaments.contains(&f.as_str()))
        })?;
    Some((machine.into(), process, filament))
}

/// Whether a mapped Orca key is an Orca-only option (no PrusaSlicer equivalent).
fn orca_only(mapped: &MappingResult) -> BTreeSet<String> {
    mapped
        .report
        .entries
        .iter()
        .filter(|e| e.disposition != Disposition::Rejected)
        .filter(|e| {
            let own = e.canonical.as_deref().is_none_or(|c| c == e.key);
            // A merged setting (wall order, scarf seams) is the PrusaSlicer profile's own value,
            // translated (`mica_core::merged`), not OrcaSlicer's.
            own && mica_core::merged::by_orca(&e.key).is_none()
                && (!option_defs().contains_key(e.key.as_str())
                    || orca_table(&e.key) == Some(OrcaTable::Optional))
        })
        .map(|e| e.key.clone())
        .collect()
}

/// Copies the Orca-only options `config` lacks from `mapped`; returns their keys.
pub fn fill_from(config: &mut Config, mapped: &MappingResult) -> Vec<String> {
    let tools = config.vector_len("nozzle_diameter").max(1);
    let mut filled = Vec::new();
    let keys = orca_only(mapped);
    for key in &keys {
        if config.get(key).is_some() || config.extra(key).is_some() {
            continue;
        }
        // An Orca filament override of an option filled here (filament_z_hop_types over
        // z_hop_types) stays unset ("nil"): the printer value applies, as for a Prusa filament
        // that does not override it.
        if let Some(base) = key.strip_prefix("filament_")
            && keys.contains(base)
            && matches!(mapped.config.get(key), Some(ConfigValue::Vector(_)))
        {
            config.set(key, ConfigValue::Vector(vec![None; tools]));
            config
                .set_raw(key, &vec!["nil"; tools].join(","))
                .expect("unknown keys are kept as text");
            filled.push(key.clone());
            continue;
        }
        let key = key.clone();
        if !option_defs().contains_key(key.as_str())
            && let Some(text) = mapped.extras.get(&key)
        {
            // The text form for templates and the Orca motion code, then the typed value.
            config
                .set_raw(&key, text)
                .expect("unknown keys are kept as text");
        }
        if let Some(value) = mapped.config.get(&key) {
            config.set(&key, value.clone());
            if config.vector_len(&key) > 1 || tools > 1 {
                config.resize_vector_repeat(&key, tools);
            }
        } else if config.extra(&key).is_none() {
            continue;
        }
        filled.push(key);
    }
    filled
}

/// The (first) preset name a `*_settings_id` option records; these are text extras.
fn preset_name(config: &Config, key: &str) -> String {
    config
        .raw(key)
        .and_then(|raw| mica_core::config::unescape_strings(&raw).ok())
        .and_then(|names| names.into_iter().next())
        .unwrap_or_default()
}

/// OrcaSlicer's own defaults, mapped like a profile.
pub fn orca_defaults() -> Arc<MappingResult> {
    static DEFAULTS: OnceLock<Arc<MappingResult>> = OnceLock::new();
    DEFAULTS
        .get_or_init(|| {
            Arc::new(orca::map_config(
                &orca::default_raw(Dialect::Orca242),
                Dialect::Orca242,
            ))
        })
        .clone()
}

/// `mica_orca_values`: where the Orca-only values come from.
pub const SOURCE_KEY: &str = "mica_orca_values";

type Matched = Option<(Arc<MappingResult>, String, String, String)>;

/// OrcaSlicer's matching profiles for a PrusaSlicer selection, mapped (cached: the GUI asks on
/// every settings change).
fn matched(profiles: Option<&Path>, printer: &str, process: &str, filament: &str) -> Matched {
    type Key = (Option<std::path::PathBuf>, String, String, String);
    static CACHE: OnceLock<Mutex<HashMap<Key, Matched>>> = OnceLock::new();
    let key = (
        profiles.map(Path::to_path_buf),
        printer.to_string(),
        process.to_string(),
        filament.to_string(),
    );
    let cache = CACHE.get_or_init(Default::default);
    if let Some(hit) = cache.lock().expect("cache lock").get(&key) {
        return hit.clone();
    }
    let found = orca::discovery::load_installed(profiles, "Prusa")
        .ok()
        .and_then(|(bundle, _)| {
            let (m, p, f) = orca_counterpart(&bundle, printer, process, filament)?;
            let raw = bundle.full_config(&m, &p, &[f.as_str()]).ok()?;
            Some((Arc::new(orca::map_config(&raw, bundle.dialect)), m, p, f))
        });
    cache.lock().expect("cache lock").insert(key, found.clone());
    found
}

/// Fills the Orca-only options of a PrusaSlicer-profile `config` that slices with OrcaSlicer's
/// rules. `profiles` is Orca's `resources/profiles` (None: the installed OrcaSlicer, else the
/// pinned checkout); its `Prusa` vendor is searched for the printer, process and filament named
/// in `config` (`printer_settings_id`, `print_settings_id`, `filament_settings_id`).
pub fn fill_orca_rules(config: &mut Config, profiles: Option<&Path>) -> Filled {
    let name = |key: &str| preset_name(config, key);
    let (printer, process, filament) = (
        name("printer_settings_id"),
        name("print_settings_id"),
        name("filament_settings_id"),
    );
    let use_profile = config.string(SOURCE_KEY) != Some("orca_defaults");
    let matched = use_profile
        .then(|| matched(profiles, &printer, &process, &filament))
        .flatten();
    match matched {
        Some((mapped, printer, process, filament)) => Filled {
            keys: fill_from(config, &mapped),
            source: Source::OrcaProfile {
                printer,
                process,
                filament,
            },
        },
        None => Filled {
            keys: fill_from(config, &orca_defaults()),
            source: Source::OrcaDefaults,
        },
    }
}

/// PrusaSlicer-only options an Orca profile slices with under PrusaSlicer rules: options of
/// PrusaSlicer's table that no Orca option maps onto. They hold PrusaSlicer's defaults
/// (`Config::defaults`) unless the user changed them.
pub fn prusa_only_options() -> Vec<&'static str> {
    let mapped: BTreeSet<&str> = orca::mapping::mapping_table()
        .values()
        .map(|(target, _)| *target)
        .collect();
    option_defs()
        .keys()
        .copied()
        .filter(|k| !mapped.contains(k))
        .filter(|k| orca_table(k).is_none())
        .filter(|k| !k.starts_with("mica_"))
        .collect()
}

/// OrcaSlicer's matching profiles for the printer, process and filament `config` names, if
/// Orca ships them (the GUI offers to take the Orca-only settings from them).
pub fn has_orca_counterpart(config: &Config, profiles: Option<&Path>) -> Option<Source> {
    let name = |key: &str| preset_name(config, key);
    let (_, printer, process, filament) = matched(
        profiles,
        &name("printer_settings_id"),
        &name("print_settings_id"),
        &name("filament_settings_id"),
    )?;
    Some(Source::OrcaProfile {
        printer,
        process,
        filament,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prusa_names_match_orca_spelling() {
        let machines = [
            "Prusa CORE One HF 0.4 nozzle",
            "Prusa CORE One L HF 0.4 nozzle",
            "Prusa CORE One 0.4 nozzle",
            "Prusa XL 5T 0.4 nozzle",
            "Prusa XL 0.4 nozzle",
        ];
        assert_eq!(
            best_match("Prusa CORE One HF0.4 nozzle", &machines, 0.75),
            Some("Prusa CORE One HF 0.4 nozzle")
        );
        assert_eq!(
            best_match(
                "Original Prusa XL - 5T Input Shaper 0.4 nozzle",
                &machines,
                0.75
            ),
            Some("Prusa XL 5T 0.4 nozzle")
        );
        assert_eq!(best_match("Voron 2.4 350", &machines, 0.75), None);
        let processes = [
            "0.20mm SPEED @CORE One HF 0.4",
            "0.15mm SPEED @CORE One HF 0.4",
        ];
        assert_eq!(
            best_match("0.20mm SPEED @COREONE HF0.4", &processes, 0.5),
            Some("0.20mm SPEED @CORE One HF 0.4")
        );
    }

    #[test]
    fn orca_defaults_fill_only_orca_only_options() {
        let mut c = Config::defaults();
        let perimeter_speed = c.raw("external_perimeter_speed");
        let keys = fill_from(&mut c, &orca_defaults());
        assert!(keys.iter().any(|k| k == "z_hop_types"), "{keys:?}");
        // Merged settings (wall order, scarf seams) keep the PrusaSlicer profile's value.
        assert!(
            !keys
                .iter()
                .any(|k| k == "seam_slope_type" || k == "wall_sequence")
        );
        assert!(keys.iter().any(|k| k == "scarf_joint_speed"), "{keys:?}");
        // Orca options with a PrusaSlicer equivalent stay with the Prusa value.
        assert!(!keys.iter().any(|k| k == "outer_wall_speed"));
        assert!(!keys.iter().any(|k| k == "nozzle_temperature"));
        assert_eq!(c.raw("external_perimeter_speed"), perimeter_speed);
        assert_eq!(c.raw("z_hop_types").as_deref(), Some("Slope Lift"));
        // Idempotent.
        assert!(fill_from(&mut c, &orca_defaults()).is_empty());
    }

    #[test]
    fn prusa_only_options_include_travel_lift_switches() {
        let keys = prusa_only_options();
        for k in ["travel_ramping_lift", "cooling_slowdown_logic"] {
            assert!(keys.contains(&k), "{k}");
        }
        assert!(!keys.contains(&"perimeters"));
    }
}
