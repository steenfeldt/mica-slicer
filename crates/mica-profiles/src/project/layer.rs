// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original
// Source (original): Mica project-settings layer between the selected presets and user edits.
// Changes: A project slices with its own embedded values; differences to the current presets are informational.

//! A project's own values: what its embedded configuration sets that differs from the
//! presets it is based on, per scope (process, printer) and per tool (filaments).
//!
//! OrcaSlicer and PrusaSlicer slice a project with the configuration it was saved with (Orca
//! loads `project_settings.config` over its full defaults; PrusaSlicer loads `Slic3r_PE.config`
//! as the project's modified presets), whatever the vendor profiles say today. Mica composes
//! the presets the project names (for display, the slice cache and as the source of options the
//! project does not set) and puts this layer on top. User edits go on top of the layer, so the
//! project's values never count as the user's own changes.

use std::collections::BTreeSet;

use mica_core::config::{
    Config, ConfigValue, Scope, escape_strings, option_defs, serialize_value, unescape_strings,
};

use crate::orca::{self, Dialect};

/// The project's values that differ from the composed presets.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Layer {
    /// Process (print) options, whole values; also options of unknown scope.
    pub process: Config,
    /// Printer options, whole values (per-extruder vectors as a whole).
    pub printer: Config,
    /// One configuration per tool: a filament vector holds only that tool's element.
    pub filaments: Vec<Config>,
    /// The dialect Orca-only options were read in (their scope and text form).
    pub dialect: Option<Dialect>,
    /// Native project values retained for the G-code settings footer.
    pub native: mica_core::config::native::NativeConfig,
}

/// One of the project's values that differs from the current preset (for display).
#[derive(Clone, Debug, PartialEq)]
pub struct Difference {
    pub scope: Scope,
    /// The tool of a filament value.
    pub tool: Option<usize>,
    pub key: String,
    /// The project's value, as option text.
    pub project: String,
    /// The current preset's value, if it has the option.
    pub preset: Option<String>,
}

/// Options that are never part of the layer: preset bookkeeping, the per-tool colours (project
/// state of their own), the bed selection (the presets are composed for the project's bed),
/// per-plate tower positions (plate placement) and print-host access.
pub fn excluded(key: &str) -> bool {
    matches!(
        key,
        "printer_settings_id"
            | "print_settings_id"
            | "filament_settings_id"
            | "inherits"
            | "inherits_group"
            | "different_settings_to_system"
            | "compatible_printers"
            | "compatible_printers_condition"
            | "compatible_prints"
            | "compatible_prints_condition"
            | "print_compatible_printers"
            | "compatible_machine_expression_group"
            | "compatible_process_expression_group"
            | "filament_ids"
            | "filament_id"
            | "name"
            | "from"
            | "version"
            | "setting_id"
            | "instantiation"
            | "is_custom_defined"
            | "upward_compatible_machine"
            | "filament_colour"
            | "curr_bed_type"
            | "wipe_tower_x"
            | "wipe_tower_y"
            | "printer_technology"
    ) || key.starts_with("print_host")
        || key.starts_with("printhost")
}

/// The scope an option belongs to: Mica's definition, else Orca's schema; unknown options
/// count as process options.
pub fn scope_of(key: &str, dialect: Option<Dialect>) -> Scope {
    let scope = option_defs().get(key).map(|d| d.scope).or_else(|| {
        dialect
            .into_iter()
            .chain([Dialect::Orca242, Dialect::Snapmaker240])
            .find_map(|d| orca::option_scope(key, d))
    });
    match scope {
        Some(Scope::Filament) => Scope::Filament,
        Some(Scope::Printer) => Scope::Printer,
        _ => Scope::Print,
    }
}

/// How an Orca-only option's text separates its elements: `,` lists, quoted `;` strings.
fn text_kind(key: &str, dialect: Option<Dialect>) -> Option<&'static str> {
    let ty = dialect
        .into_iter()
        .chain([Dialect::Orca242, Dialect::Snapmaker240])
        .find_map(|d| orca::option_type(key, d))?;
    match ty {
        "Strings" => Some("strings"),
        "Ints" | "Floats" | "Bools" | "Percents" | "FloatsOrPercents" | "Enums" => Some("list"),
        _ => None,
    }
}

fn split_text(text: &str, kind: &str) -> Option<Vec<String>> {
    match kind {
        "strings" => unescape_strings(text).ok(),
        _ => Some(text.split(',').map(str::to_string).collect()),
    }
}

fn join_text(items: &[String], kind: &str) -> String {
    match kind {
        "strings" => escape_strings(items),
        _ => items.join(","),
    }
}

/// An option neither Mica nor Orca's schema knows, which the presets do not have either
/// (another fork's options): nothing reads it, so it is not one of the project's values.
fn unknown(key: &str, base: &Config, dialect: Option<Dialect>) -> bool {
    let dialects = match dialect {
        Some(d) => vec![d],
        None => vec![Dialect::Orca242, Dialect::Snapmaker240],
    };
    !option_defs().contains_key(key)
        && dialects
            .into_iter()
            .all(|d| orca::option_type(key, d).is_none())
        && base.get(key).is_none()
        && base.extra(key).is_none()
}

/// Whether two values are the same setting (equal, or the same text).
fn same(preset: Option<&ConfigValue>, project: &ConfigValue) -> bool {
    preset.is_some_and(|p| p == project || serialize_value(p) == serialize_value(project))
}

/// A vector of `n` elements from `items`: a flush matrix keeps its top-left `n`-filament
/// block (a project with more filaments than the printer has tools), other vectors are cut
/// or padded with their last element.
fn reshape<T: Clone>(key: &str, items: Vec<T>, n: usize) -> Vec<T> {
    let side = |len: usize| (1..=len).find(|s| s * s >= len).filter(|s| s * s == len);
    if matches!(key, "flush_volumes_matrix" | "wiping_volumes_matrix")
        && let (Some(m), Some(k)) = (side(items.len()), side(n))
        && m >= k
    {
        return (0..k)
            .flat_map(|row| items[row * m..row * m + k].to_vec())
            .collect();
    }
    let mut items = items;
    if let Some(last) = items.last().cloned() {
        items.resize(n, last);
    }
    items
}

/// `value` in the shape of the preset's: a scalar for a vector option repeats per element, a
/// vector of another length (per filament, from a project with more filaments) is reshaped.
fn conform(key: &str, value: &ConfigValue, preset: Option<&ConfigValue>) -> ConfigValue {
    match (value, preset) {
        (ConfigValue::Vector(items), Some(ConfigValue::Vector(theirs)))
            if !theirs.is_empty() && !items.is_empty() && items.len() != theirs.len() =>
        {
            ConfigValue::Vector(reshape(key, items.clone(), theirs.len()))
        }
        (ConfigValue::Scalar(v), Some(ConfigValue::Vector(theirs))) if !theirs.is_empty() => {
            ConfigValue::Vector(vec![Some(v.clone()); theirs.len()])
        }
        _ => value.clone(),
    }
}

/// Element `tool` of a vector option (the first element for a shorter vector).
fn element(value: &ConfigValue, tool: usize) -> Option<ConfigValue> {
    match value {
        ConfigValue::Vector(items) => {
            let item = items.get(tool).or_else(|| items.first())?;
            Some(ConfigValue::Vector(vec![item.clone()]))
        }
        scalar => Some(scalar.clone()),
    }
}

impl Layer {
    /// The values of `source` (the project's embedded configuration in Mica's form, holding
    /// only what the project sets) that differ from `base` (the composed presets). Filament
    /// vectors are compared per tool for `tools` tools; a project with fewer filaments leaves
    /// the remaining tools to their presets. `keep` filters by scope, tool and key.
    pub fn between(
        source: &Config,
        base: &Config,
        tools: usize,
        dialect: Option<Dialect>,
        keep: impl Fn(Scope, Option<usize>, &str) -> bool,
    ) -> Self {
        let tools = tools.max(1);
        let mut out = Self {
            filaments: vec![Config::new(); tools],
            dialect,
            ..Default::default()
        };
        for (key, value) in source.values() {
            if excluded(key) || unknown(key, base, dialect) {
                continue;
            }
            let scope = scope_of(key, dialect);
            // Filament vectors are compared per tool; other vectors take the preset's shape.
            let value = match (scope, value) {
                (Scope::Filament, ConfigValue::Vector(_)) => value.clone(),
                _ => conform(key, value, base.get(key)),
            };
            match (scope, &value) {
                (Scope::Filament, ConfigValue::Vector(items)) => {
                    for (tool, item) in items.iter().enumerate().take(tools) {
                        let mine = ConfigValue::Vector(vec![item.clone()]);
                        let theirs = base.get(key).and_then(|v| element(v, tool));
                        if !same(theirs.as_ref(), &mine) && keep(scope, Some(tool), key) {
                            out.filaments[tool].set(key, mine);
                        }
                    }
                }
                _ => {
                    if !same(base.get(key), &value) && keep(scope, None, key) {
                        out.scope_mut(scope).set(key, value);
                    }
                }
            }
        }
        for (key, text) in source.extras() {
            if excluded(key) || unknown(key, base, dialect) {
                continue;
            }
            let scope = scope_of(key, dialect);
            let kind = text_kind(key, dialect);
            let preset = base.extra(key);
            // The text twin of a typed value follows the typed comparison (Orca writes `1`
            // where a preset has `1.0`).
            let typed = source.get(key).is_some();
            let differs = |out: &Self, tool: Option<usize>, text_differs: bool| match tool {
                _ if !typed => text_differs,
                Some(t) => out.filaments[t].get(key).is_some(),
                None => out.process.get(key).is_some() || out.printer.get(key).is_some(),
            };
            let list = kind.and_then(|k| Some((k, split_text(text, k)?)));
            if let Some((kind, mut items)) = list {
                let theirs = preset.and_then(|t| split_text(t, kind));
                if scope != Scope::Filament
                    && let Some(theirs) = theirs.as_ref().filter(|t| !t.is_empty())
                {
                    if items.len() == 1 && theirs.len() > 1 {
                        items = vec![items[0].clone(); theirs.len()];
                    } else if items.len() != theirs.len() {
                        items = reshape(key, items, theirs.len());
                    }
                }
                if scope == Scope::Filament {
                    for (tool, mine) in items.iter().enumerate().take(tools) {
                        let preset = theirs
                            .as_ref()
                            .and_then(|t| t.get(tool).or_else(|| t.first()));
                        if differs(&out, Some(tool), preset != Some(mine))
                            && keep(scope, Some(tool), key)
                        {
                            out.filaments[tool]
                                .set_raw(key, &join_text(std::slice::from_ref(mine), kind))
                                .expect("unknown keys are kept verbatim");
                        }
                    }
                    continue;
                }
                if differs(&out, None, theirs.as_ref() != Some(&items)) && keep(scope, None, key) {
                    out.scope_mut(scope)
                        .set_raw(key, &join_text(&items, kind))
                        .expect("unknown keys are kept verbatim");
                }
                continue;
            }
            if differs(&out, None, preset != Some(text)) && keep(scope, None, key) {
                out.scope_mut(scope)
                    .set_raw(key, text)
                    .expect("unknown keys are kept verbatim");
            }
        }
        let accepted = |key: &str| {
            let canonical = orca::mapping::mapping_table()
                .get(key)
                .map_or(key, |(k, _)| *k);
            let scope = scope_of(canonical, dialect);
            !excluded(key)
                && !excluded(canonical)
                && if scope == Scope::Filament {
                    (0..tools).any(|t| keep(scope, Some(t), canonical))
                } else {
                    keep(scope, None, canonical)
                }
        };
        out.native.options = source
            .native
            .options
            .iter()
            .filter(|(k, _)| accepted(k))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        out.native.baseline = source
            .native
            .baseline
            .iter()
            .filter(|(k, _)| accepted(k))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        out.native.typed_baseline = source
            .native
            .typed_baseline
            .iter()
            .filter(|(k, _)| accepted(k))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        out
    }

    fn scope_mut(&mut self, scope: Scope) -> &mut Config {
        match scope {
            Scope::Printer => &mut self.printer,
            _ => &mut self.process,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.differences_raw().next().is_none()
    }

    /// Puts the layer over `config` (the composed presets it was computed against, or the same
    /// presets on another bed type).
    pub fn apply(&self, config: &mut Config) {
        if !self.native.options.is_empty() {
            let native = std::sync::Arc::make_mut(&mut config.native);
            native.options.extend(self.native.options.clone());
            native.baseline.extend(self.native.baseline.clone());
            native
                .typed_baseline
                .extend(self.native.typed_baseline.clone());
        }
        config.apply(&self.process);
        config.apply(&self.printer);
        for (tool, layer) in self.filaments.iter().enumerate() {
            for (key, value) in layer.values() {
                let ConfigValue::Vector(items) = value else {
                    config.set(key, value.clone());
                    continue;
                };
                let Some(item) = items.first() else { continue };
                let mut values = match config.get(key) {
                    Some(ConfigValue::Vector(v)) => v.clone(),
                    _ => Vec::new(),
                };
                if values.len() <= tool {
                    let pad = values.last().cloned().unwrap_or_else(|| item.clone());
                    values.resize(tool + 1, pad);
                }
                values[tool] = item.clone();
                config.set(key, ConfigValue::Vector(values));
            }
            for (key, text) in layer.extras() {
                let Some(kind) = text_kind(key, self.dialect) else {
                    let _ = config.set_raw(key, text);
                    continue;
                };
                let Some(element) = split_text(text, kind).and_then(|v| v.into_iter().next())
                else {
                    continue;
                };
                let mut values = config
                    .extra(key)
                    .and_then(|t| split_text(t, kind))
                    .unwrap_or_default();
                if values.len() <= tool {
                    let pad = values.last().cloned().unwrap_or_else(|| element.clone());
                    values.resize(tool + 1, pad);
                }
                values[tool] = element;
                let _ = config.set_raw(key, &join_text(&values, kind));
            }
        }
    }

    /// Drops `keys` from every scope (the bed temperatures when another bed type is chosen).
    pub fn drop_keys(&mut self, keys: &[&str]) {
        let keep = |k: &String, _: &mut String| {
            let canonical = orca::mapping::mapping_table()
                .get(k.as_str())
                .map_or(k.as_str(), |(k, _)| *k);
            !keys.contains(&k.as_str()) && !keys.contains(&canonical)
        };
        self.native.options.retain(keep);
        self.native.baseline.retain(keep);
        self.native.typed_baseline.retain(keep);
        let without = |c: &Config| {
            let mut out = Config::new();
            for (k, v) in c.values().filter(|(k, _)| !keys.contains(k)) {
                out.set(k, v.clone());
            }
            for (k, v) in c.extras().filter(|(k, _)| !keys.contains(k)) {
                out.set_raw(k, v).expect("unknown keys are kept verbatim");
            }
            out
        };
        self.process = without(&self.process);
        self.printer = without(&self.printer);
        for f in &mut self.filaments {
            *f = without(f);
        }
    }

    /// Drops the project's process values (another process preset was chosen).
    pub fn drop_process(&mut self) {
        let keep = |k: &String, _: &mut String| scope_of(k, self.dialect) != Scope::Print;
        self.native.options.retain(keep);
        self.native.baseline.retain(keep);
        self.native.typed_baseline.retain(keep);
        self.process = Config::new();
    }

    /// Drops the project's values for `tool` (another filament preset was chosen for it).
    pub fn drop_tool(&mut self, tool: usize) {
        // Dropping a tool requires rebuilding each native filament vector from the
        // effective canonical vector (which still includes the other project tools).
        let keep = |k: &String, _: &mut String| scope_of(k, self.dialect) != Scope::Filament;
        self.native.options.retain(keep);
        self.native.baseline.retain(keep);
        self.native.typed_baseline.retain(keep);
        if let Some(f) = self.filaments.get_mut(tool) {
            *f = Config::new();
        }
    }

    /// Every entry with its scope and tool, Orca-named copies and text twins included.
    fn differences_raw(&self) -> impl Iterator<Item = (Scope, Option<usize>, &Config, &str)> {
        let whole = [
            (Scope::Print, &self.process),
            (Scope::Printer, &self.printer),
        ]
        .into_iter()
        .map(|(s, c)| (s, None, c));
        let tools = self
            .filaments
            .iter()
            .enumerate()
            .map(|(t, c)| (Scope::Filament, Some(t), c));
        whole.chain(tools).flat_map(|(scope, tool, c)| {
            c.values()
                .map(|(k, _)| k)
                .chain(c.extras().map(|(k, _)| k))
                .map(move |k| (scope, tool, c, k))
        })
    }

    /// The project's values as the user sees them, one entry per option: an Orca option that
    /// the mapping also stores under Mica's name, and the text twin of a typed value, appear
    /// once (under Mica's name). `base` supplies the current preset's value.
    pub fn differences(&self, base: &Config) -> Vec<Difference> {
        let mut seen = BTreeSet::new();
        let mut out = Vec::new();
        for (scope, tool, c, key) in self.differences_raw() {
            if !seen.insert((tool, key.to_string())) {
                continue;
            }
            if let Some(&(canonical, _)) = orca::mapping::mapping_table().get(key)
                && canonical != key
                && (c.get(canonical).is_some() || c.extra(canonical).is_some())
            {
                continue;
            }
            let Some(project) = c.raw(key) else { continue };
            let preset = match (tool, base.get(key)) {
                (Some(t), Some(v)) => element(v, t).map(|v| serialize_value(&v)),
                (Some(t), None) => base.extra(key).map(|raw| {
                    match text_kind(key, self.dialect).and_then(|k| split_text(raw, k)) {
                        Some(items) => items
                            .get(t)
                            .or_else(|| items.first())
                            .cloned()
                            .unwrap_or_default(),
                        None => raw.to_string(),
                    }
                }),
                (None, _) => base.raw(key),
            };
            out.push(Difference {
                scope,
                tool,
                key: key.to_string(),
                project,
                preset,
            });
        }
        out
    }

    /// How many of the project's values differ from the current presets.
    pub fn count(&self, base: &Config) -> usize {
        self.differences(base).len()
    }

    /// The keys the layer sets, by scope and tool (for saving them as project changes).
    pub fn keys(&self) -> super::Changes {
        let keys = |c: &Config| {
            c.values()
                .map(|(k, _)| k)
                .chain(c.extras().map(|(k, _)| k))
                .map(|k| (k.to_string(), String::new()))
                .collect()
        };
        super::Changes {
            process: keys(&self.process),
            printer: keys(&self.printer),
            filaments: self.filaments.iter().map(keys).collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(pairs: &[(&str, &str)]) -> Config {
        let mut c = Config::new();
        for (k, v) in pairs {
            c.set_raw(k, v).unwrap();
        }
        c
    }

    #[test]
    fn layer_holds_only_differences_per_scope_and_tool() {
        let base = config(&[
            ("perimeters", "2"),
            ("layer_height", "0.2"),
            ("temperature", "210,210,210"),
            ("retract_length", "0.8,0.8,0.8"),
            ("filament_settings_id", "\"A\";\"B\";\"C\""),
            ("prime_volume", "30"),
        ]);
        let mut source = config(&[
            ("perimeters", "3"),
            ("layer_height", "0.2"),
            ("temperature", "210,225,230,240"),
            ("retract_length", "1.5,1.5,1.5"),
            ("filament_settings_id", "\"X\";\"Y\";\"Z\""),
            ("prime_volume", "45"),
            ("wipe_tower_x", "10"),
        ]);
        source
            .set_raw("nozzle_temperature_range_high", "240,260,250")
            .unwrap();
        let l = Layer::between(&source, &base, 3, None, |_, _, _| true);
        assert_eq!(l.process.int("perimeters"), Some(3));
        assert!(l.process.get("layer_height").is_none());
        assert!(l.process.get("wipe_tower_x").is_none());
        assert!(l.filaments[0].get("temperature").is_none());
        assert_eq!(l.filaments[1].float_at("temperature", 0), Some(225.));
        assert_eq!(l.filaments[2].float_at("temperature", 0), Some(230.));
        assert!(
            l.filaments
                .iter()
                .all(|f| f.get("filament_settings_id").is_none())
        );
        assert_eq!(l.printer.float_at("retract_length", 2), Some(1.5));
        assert_eq!(l.process.extra("prime_volume"), Some("45"));
        assert_eq!(
            l.filaments[1].extra("nozzle_temperature_range_high"),
            Some("260")
        );

        let mut c = base.clone();
        c.set_raw("nozzle_temperature_range_high", "240,240,240")
            .unwrap();
        l.apply(&mut c);
        assert_eq!(c.int("perimeters"), Some(3));
        assert_eq!(c.raw("temperature").as_deref(), Some("210,225,230"));
        assert_eq!(
            c.extra("nozzle_temperature_range_high"),
            Some("240,260,250")
        );
        assert_eq!(c.extra("prime_volume"), Some("45"));
        assert_eq!(l.count(&base), 8);

        let only = Layer::between(&source, &base, 3, None, |s, t, k| {
            s == Scope::Filament && t == Some(2) && k == "temperature"
        });
        assert_eq!(only.count(&base), 1);
        let mut dropped = l.clone();
        dropped.drop_tool(1);
        dropped.drop_process();
        assert_eq!(dropped.count(&base), 4);
    }
}
