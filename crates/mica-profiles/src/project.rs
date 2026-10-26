// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original
// Source (original): Mica project-settings parsing and shared preset selection.
// Changes: Retain inherited printer names and nozzle fallback for GUI and CLI project matching.
// Changes: Slice with the whole embedded configuration (a layer over the presets it names).
// Changes: A PrusaSlicer project's stale merged Orca keys give way to their PrusaSlicer counterparts (mica_core::merged).

//! Settings embedded in 3MF projects (object, part and height-range metadata), mapped onto
//! Mica's canonical names: PrusaSlicer projects use them already; Orca and Snapmaker projects go
//! through the Orca option mapping. Used by the CLI and the GUI through
//! `mica_core::input::SettingsMapper`.
//!
//! [`ProjectSettings`] reads the project-level printer/process/filament settings (Orca's
//! `project_settings.config`, Prusa's `Slic3r_PE.config`) into preset names, filament colours,
//! the settings the project lists as changed relative to its system presets, and the whole
//! embedded configuration. A project slices with its embedded values, as in OrcaSlicer and
//! PrusaSlicer: [`ProjectSettings::layer`] gives them as a [`Layer`] over the presets the
//! project is based on (which only supply options the project does not set).

use std::collections::{BTreeMap, BTreeSet};

mod layer;
mod save;
pub use layer::{Difference, Layer, excluded, scope_of};
pub use save::{SaveSettings, orca_absolutes, selected_orca};

mod selection;
mod tower;
pub use selection::{
    Decision, decide, match_filament, match_name, match_printer, same_machine, split_printer_name,
    strip_copy,
};

use mica_core::config::{Config, ConfigValue, Scope, option_defs, parse_value, serialize_value};
use mica_core::input::SettingsMapper;
use mica_core::threemf::{RawConfig, Settings};

use crate::orca::{
    Dialect,
    mapping::{Disposition, map_config, map_metadata, map_sparse},
};

/// Maps PrusaSlicer or Orca project settings; rejected Orca values are errors, keys that only
/// survive as extras are reported through `ignored`.
#[derive(Clone, Copy, Debug, Default)]
pub struct Mapper {
    /// Resolve object tools against the project's own filament count when it exceeds the
    /// selected printer's, instead of failing the import (the GUI, which then offers the
    /// project's printer), and keep tool assignments beyond the selected printer's tools (the
    /// GUI keeps them after a printer switch and clamps when slicing), and report an
    /// object or part value Orca's schema rejects instead of failing the whole import. The
    /// CLI keeps the strict checks.
    pub project_tools: bool,
}

impl SettingsMapper for Mapper {
    fn tower_placements(
        &self,
        project: &mica_core::threemf::Project,
    ) -> Result<BTreeMap<u32, [f64; 3]>, String> {
        let mut placements = tower::placements(project.config.as_ref(), project.plates.len())?;
        // Explicit model metadata has the same precedence as in the Prusa importer.
        placements.extend(project.wipe_towers.clone());
        Ok(placements)
    }

    fn plate_dimensions(&self, project: &mica_core::threemf::Project) -> Option<(f64, f64)> {
        let raw = project.config.as_ref()?;
        if !raw.path.ends_with("project_settings.config") {
            return None;
        }
        let json: serde_json::Value = serde_json::from_str(&raw.text).ok()?;
        let points: Option<Vec<(f64, f64)>> = json
            .get("printable_area")?
            .as_array()?
            .iter()
            .map(|v| {
                let (x, y) = v.as_str()?.split_once('x')?;
                Some((x.parse().ok()?, y.parse().ok()?))
            })
            .collect();
        let points = points?;
        if points.is_empty() || points.iter().any(|(x, y)| !x.is_finite() || !y.is_finite()) {
            return None;
        }
        let width = points.iter().map(|p| p.0).fold(f64::NEG_INFINITY, f64::max)
            - points.iter().map(|p| p.0).fold(f64::INFINITY, f64::min);
        let depth = points.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max)
            - points.iter().map(|p| p.1).fold(f64::INFINITY, f64::min);
        (width > 0. && depth > 0.).then_some((width, depth))
    }

    fn lenient_tools(&self) -> bool {
        self.project_tools
    }

    fn tool_count(&self, project: &mica_core::threemf::Project) -> Option<usize> {
        if !self.project_tools {
            return None;
        }
        let settings = ProjectSettings::parse(project.config.as_ref()?)?;
        let n = settings.filaments.len().max(settings.colours.len());
        (n > 0).then_some(n)
    }

    fn patch(
        &self,
        raw: &Settings,
        orca: bool,
        ignored: &mut BTreeSet<String>,
    ) -> Result<Config, String> {
        if !orca {
            return mica_core::input::prusa_patch(raw, ignored);
        }
        let mapped = map_metadata(raw, Dialect::Orca242);
        ignored.extend(mapped.report.notes.iter().cloned());
        for entry in &mapped.report.entries {
            match entry.disposition {
                // The editor opens the project anyway and says what it left out (a project
                // that fails as a whole for one bad object value cannot even be repaired).
                Disposition::Rejected if self.project_tools => {
                    ignored.insert(format!(
                        "setting {} (rejected: {})",
                        entry.key, entry.reason
                    ));
                }
                Disposition::Rejected => {
                    return Err(format!("3MF {}: {}", entry.key, entry.reason));
                }
                Disposition::ExtrasOnly if entry.key != "extruder" => {
                    ignored.insert(format!("setting {} ({})", entry.key, entry.reason));
                }
                _ => {}
            }
        }
        Ok(mapped.config)
    }
}

/// Which slicer wrote the project settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProjectDialect {
    /// Orca / Bambu / Snapmaker `Metadata/project_settings.config` (JSON).
    Orca,
    /// PrusaSlicer `Metadata/Slic3r_PE.config` (INI).
    Prusa,
}

/// The settings a project changed relative to its system presets, as canonical
/// (PrusaSlicer) option names → text, by preset scope. Filament entries hold one tool's value.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Changes {
    pub process: BTreeMap<String, String>,
    pub filaments: Vec<BTreeMap<String, String>>,
    pub printer: BTreeMap<String, String>,
}

impl Changes {
    pub fn count(&self) -> usize {
        self.process.len()
            + self.printer.len()
            + self.filaments.iter().map(BTreeMap::len).sum::<usize>()
    }
}

/// A project's own printer/process/filament selection and the settings it changed.
#[derive(Clone, Debug, PartialEq)]
pub struct ProjectSettings {
    pub dialect: ProjectDialect,
    /// `printer_settings_id`: the preset name the project was sliced with.
    pub printer: Option<String>,
    /// `printer_model` and `printer_variant` (nozzle), for matching a renamed preset.
    pub printer_model: Option<String>,
    pub printer_variant: Option<String>,
    /// Base printer preset names for a renamed user preset (`inherits`).
    pub printer_inherits: Vec<String>,
    /// `print_settings_id`.
    pub process: Option<String>,
    /// `filament_settings_id`, one per tool.
    pub filaments: Vec<String>,
    /// `filament_colour`, one per tool, as the project keeps it (`mica_core::color::project_text`:
    /// `#RRGGBB`, or the project's own `#RRGGBBAA` text); `None` when not a hex colour.
    pub colours: Vec<Option<String>>,
    /// `filament_type`, one per tool (PLA, PETG, …).
    pub filament_types: Vec<String>,
    /// Orca's `curr_bed_type`: the plate type the project was set up for.
    pub bed: Option<String>,
    /// Changes the project lists itself (Orca's `different_settings_to_system`), mapped to
    /// canonical names. Prusa projects do not list theirs. Informational and for
    /// [`Mode::Listed`]; slicing uses the whole embedded configuration ([`Self::layer`]).
    pub changes: Changes,
    /// Orca's per-plate prime tower position (`wipe_tower_x[i]`, `wipe_tower_y[i]`, bed
    /// coordinates of the plate), one entry per plate in plate order. Empty for Prusa
    /// projects, whose tower placement is model metadata the importer reads.
    pub towers: Vec<Option<[f64; 2]>>,
    /// Mica's own options from Mica's sidecar (an Orca project's `mica_slicing_rules`); set by
    /// the loader, applied as printer values of the layer.
    pub mica: std::collections::BTreeMap<String, String>,
    /// The whole Prusa project configuration in canonical form.
    full: Option<Config>,
    /// The whole Orca project configuration as stored (mapped when a layer is made, in the
    /// dialect of the bundle it is used with).
    orca: Option<crate::orca::RawConfig>,
    /// The Orca keys `different_settings_to_system` lists: process, one entry per filament,
    /// printer.
    listed: Vec<BTreeSet<String>>,
}

/// Which of a project's values a [`Layer`] holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Everything the project's embedded configuration sets (how OrcaSlicer and PrusaSlicer
    /// slice a project).
    Embedded,
    /// Only what the project marks as deliberately changed (Orca's
    /// `different_settings_to_system`) over today's presets: "Update to current profile".
    Listed,
}

impl ProjectSettings {
    /// Reads the embedded project settings; `None` for unknown or unreadable files.
    pub fn parse(raw: &RawConfig) -> Option<Self> {
        if raw.path.ends_with("project_settings.config") {
            Self::parse_orca(&raw.text)
        } else if raw.path.ends_with("Slic3r_PE.config") {
            Self::parse_prusa(&raw.text)
        } else {
            None
        }
    }

    fn parse_orca(text: &str) -> Option<Self> {
        let json: serde_json::Value = serde_json::from_str(text).ok()?;
        let map = json.as_object()?;
        let string = |k: &str| map.get(k).and_then(|v| v.as_str()).map(str::to_string);
        let strings = |k: &str| -> Vec<String> {
            map.get(k)
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .map(|v| v.as_str().unwrap_or_default().to_string())
                        .collect()
                })
                .unwrap_or_default()
        };
        let filaments = strings("filament_settings_id");
        let colours: Vec<Option<String>> = strings("filament_colour")
            .iter()
            .map(|c| mica_core::color::project_text(c))
            .collect();
        let tools = filaments.len().max(colours.len());
        // `different_settings_to_system`: process, then one entry per filament, then printer;
        // each a `;`-separated key list.
        let diff = strings("different_settings_to_system");
        let keys = |entry: Option<&String>| -> Vec<String> {
            entry
                .map(|s| {
                    s.split(';')
                        .map(str::trim)
                        .filter(|k| !k.is_empty())
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default()
        };
        let orca: crate::orca::RawConfig =
            map.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        let mapped = map_config(&orca, Dialect::Orca242);
        let canonical_of = |key: &str| -> Vec<String> {
            mapped
                .report
                .entries
                .iter()
                .filter(|e| e.key == key)
                .filter_map(|e| e.canonical.clone())
                .chain(
                    crate::orca::mapping::also_sets(key)
                        .iter()
                        .map(|k| k.to_string()),
                )
                .chain(option_defs().contains_key(key).then(|| key.to_string()))
                .collect()
        };
        let mut changes = Changes {
            filaments: vec![BTreeMap::new(); tools],
            ..Default::default()
        };
        for key in keys(diff.first()) {
            for target in canonical_of(&key) {
                if let Some(v) = mapped.config.raw(&target) {
                    changes.process.insert(target, v);
                }
            }
        }
        for key in keys(diff.get(tools + 1)) {
            for target in canonical_of(&key) {
                if let Some(v) = mapped.config.raw(&target) {
                    changes.printer.insert(target, v);
                }
            }
        }
        for tool in 0..tools {
            for key in keys(diff.get(tool + 1)) {
                for target in canonical_of(&key) {
                    if let Some(v) = tool_value(&mapped.config, &target, tool) {
                        changes.filaments[tool].insert(target, v);
                    }
                }
            }
        }
        let mut listed: Vec<BTreeSet<String>> = diff
            .iter()
            .map(|entry| keys(Some(entry)).into_iter().collect())
            .collect();
        listed.resize(tools + 2, BTreeSet::new());
        let xs = strings("wipe_tower_x");
        let ys = strings("wipe_tower_y");
        let towers = xs
            .iter()
            .zip(&ys)
            .map(|(x, y)| Some([x.trim().parse().ok()?, y.trim().parse().ok()?]))
            .collect();
        Some(Self {
            dialect: ProjectDialect::Orca,
            towers,
            printer: string("printer_settings_id"),
            printer_model: string("printer_model"),
            printer_variant: string("printer_variant")
                .filter(|v| !v.trim().is_empty())
                .or_else(|| strings("nozzle_diameter").into_iter().next())
                .or_else(|| string("nozzle_diameter")),
            printer_inherits: string("inherits")
                .map(|s| vec![s])
                .unwrap_or_else(|| strings("inherits")),
            process: string("print_settings_id"),
            filaments,
            colours,
            filament_types: strings("filament_type"),
            bed: string("curr_bed_type").filter(|b| !b.trim().is_empty()),
            changes,
            mica: Default::default(),
            full: None,
            orca: Some(orca),
            listed,
        })
    }

    fn parse_prusa(text: &str) -> Option<Self> {
        // PrusaSlicer writes every option of `Slic3r_PE.config` as a `; key = value` comment
        // line; the INI reader would skip those, so the marker comes off first.
        let text: String = text
            .lines()
            .map(|l| {
                let trimmed = l.trim_start();
                match trimmed.strip_prefix(';') {
                    Some(rest) if rest.contains(" = ") => rest.trim_start(),
                    _ => l,
                }
            })
            .fold(String::new(), |mut s, l| {
                s.push_str(l);
                s.push('\n');
                s
            });
        let sections = crate::ini::parse(&text);
        let entries: Vec<(String, String)> = sections.into_iter().flat_map(|s| s.entries).collect();
        if entries.is_empty() {
            return None;
        }
        let get = |k: &str| {
            entries
                .iter()
                .rev()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v)
        };
        let list = |k: &str| -> Vec<String> {
            get(k)
                .and_then(|v| mica_core::config::unescape_strings(v).ok())
                .unwrap_or_default()
        };
        // A merged setting's Orca key that disagrees with its PrusaSlicer counterpart was
        // left behind by an edit in PrusaSlicer: the counterpart wins (`mica_core::merged`).
        let mut layer: BTreeMap<String, String> = entries
            .iter()
            .filter(|(k, _)| option_defs().contains_key(k.as_str()))
            .cloned()
            .collect();
        mica_core::merged::reconcile_prusa_layer(&mut layer, &Config::defaults());
        let mut full = Config::new();
        for (k, v) in &layer {
            let _ = full.set_raw(k, v);
        }
        let filaments = list("filament_settings_id");
        let colours = list("filament_colour")
            .iter()
            .map(|c| mica_core::color::project_text(c.trim().trim_matches('"')))
            .collect();
        Some(Self {
            dialect: ProjectDialect::Prusa,
            towers: Vec::new(),
            printer: get("printer_settings_id").cloned(),
            printer_model: get("printer_model").cloned(),
            printer_variant: get("printer_variant").cloned(),
            printer_inherits: list("inherits"),
            process: get("print_settings_id").cloned(),
            filaments,
            colours,
            filament_types: list("filament_type"),
            bed: None,
            changes: Changes::default(),
            mica: Default::default(),
            full: Some(full),
            orca: None,
            listed: Vec::new(),
        })
    }

    /// Number of tools the project prints with.
    pub fn tool_count(&self) -> usize {
        self.filaments.len().max(self.colours.len())
    }

    /// The embedded configuration in Mica's form, holding only what the project sets (Orca
    /// options mapped through the option table in `dialect`), with a note for every value the
    /// mapping rejects (those keep the presets' value) and for every Snapmaker Orca
    /// flow-variant list Mica reads as one value for all filaments.
    pub fn source(&self, dialect: Option<Dialect>) -> (Config, Vec<String>) {
        if let Some(full) = &self.full {
            return (full.clone(), Vec::new());
        }
        let Some(raw) = &self.orca else {
            return (Config::new(), Vec::new());
        };
        let dialect = dialect.unwrap_or(Dialect::Orca242);
        let mapped = map_sparse(raw, dialect);
        let rejected: Vec<_> = mapped
            .report
            .entries
            .iter()
            .filter(|e| e.disposition == Disposition::Rejected)
            .collect();
        if rejected.is_empty() {
            return (mapped.config, mapped.report.notes);
        }
        let notes = mapped
            .report
            .notes
            .iter()
            .cloned()
            .chain(rejected.iter().filter(|e| !excluded(&e.key)).map(|e| {
                format!(
                    "project value {} rejected ({}); the preset's value is used",
                    e.key, e.reason
                )
            }))
            .collect();
        let mut kept = raw.clone();
        for e in &rejected {
            kept.remove(&e.key);
        }
        (map_sparse(&kept, dialect).config, notes)
    }

    /// The project's values over `base`, the composed configuration of the presets it is
    /// based on: everything the embedded configuration sets that differs from them, or (in
    /// [`Mode::Listed`]) only the options the project lists as changed. Filament values are
    /// per tool, for the tools `base` has. The notes name rejected values.
    pub fn layer(
        &self,
        base: &Config,
        dialect: Option<Dialect>,
        mode: Mode,
    ) -> (Layer, Vec<String>) {
        // An Orca project on a PrusaSlicer printer (`dialect` is `None`) goes through the
        // lossy dialect translation: only the changes it lists are taken, as before.
        let mode = if self.orca.is_some() && dialect.is_none() {
            Mode::Listed
        } else {
            mode
        };
        let (source, notes) = self.source(dialect);
        let dialect = self
            .orca
            .as_ref()
            .map(|_| dialect.unwrap_or(Dialect::Orca242));
        let tools = base.vector_len("nozzle_diameter").max(1);
        let layer = match mode {
            Mode::Embedded => Layer::between(&source, base, tools, dialect, |_, _, _| true),
            Mode::Listed => {
                let targets = self.listed_targets(dialect.unwrap_or(Dialect::Orca242));
                let printer = self.tool_count() + 1;
                Layer::between(&source, base, tools, dialect, |_, tool, key| {
                    let mut lists = match tool {
                        Some(t) => vec![targets.get(t + 1)],
                        None => vec![targets.first(), targets.get(printer)],
                    };
                    lists.retain(Option::is_some);
                    lists.into_iter().flatten().any(|l| l.contains(key))
                })
            }
        };
        let mut layer = layer;
        let mut notes = notes;
        // A bed that cannot be one (a corner listed twice, no area) is never taken: the
        // printer's own bed is used, with a note (GUI monkey test, seed 3004).
        for scope in [&mut layer.printer, &mut layer.process] {
            if let Some(problem) = scope
                .points("bed_shape")
                .and_then(|bed| mica_core::safety::bed_shape_problem(&bed))
            {
                scope.remove("bed_shape");
                notes.push(format!(
                    "project bed shape rejected ({problem}); the printer's bed is used"
                ));
            }
        }
        self.default_rules(base, &mut layer);
        (layer, notes)
    }

    /// A project slices like the slicer that saved it: without its own `mica_slicing_rules`
    /// (Mica writes it into Prusa projects and into its sidecar for Orca ones), a PrusaSlicer
    /// project follows PrusaSlicer's rules and an OrcaSlicer project Orca's.
    fn default_rules(&self, base: &Config, layer: &mut Layer) {
        use mica_core::rules::{KEY, Rules};
        for (key, value) in &self.mica {
            if key.starts_with("mica_") && option_defs().contains_key(key.as_str()) {
                let _ = layer.printer.set_raw(key, value);
            }
        }
        let explicit = self.full.as_ref().is_some_and(|f| f.get(KEY).is_some())
            || self.orca.as_ref().is_some_and(|o| o.contains_key(KEY))
            || self.mica.contains_key(KEY);
        let wanted = match self.dialect {
            ProjectDialect::Prusa => Rules::PrusaSlicer,
            ProjectDialect::Orca => Rules::OrcaSlicer,
        };
        if !explicit && Rules::of(base) != wanted {
            wanted.set(&mut layer.printer);
        }
    }

    /// The listed Orca keys with the Mica options each of them sets.
    fn listed_targets(&self, dialect: Dialect) -> Vec<BTreeSet<String>> {
        let Some(raw) = &self.orca else {
            return Vec::new();
        };
        let report = map_sparse(raw, dialect).report;
        self.listed
            .iter()
            .map(|keys| {
                let mut out = BTreeSet::new();
                for key in keys {
                    out.insert(key.clone());
                    out.extend(
                        report
                            .entries
                            .iter()
                            .filter(|e| &e.key == key)
                            .filter_map(|e| e.canonical.clone()),
                    );
                    out.extend(
                        crate::orca::mapping::also_sets(key)
                            .iter()
                            .map(|k| k.to_string()),
                    );
                }
                out
            })
            .collect()
    }
}

/// Text of one tool's element of `key` (the scalar itself for scalar options).
fn tool_value(config: &Config, key: &str, tool: usize) -> Option<String> {
    match config.get(key)? {
        ConfigValue::Vector(items) => {
            let item = items.get(tool).or_else(|| items.first())?;
            Some(serialize_value(&ConfigValue::Vector(vec![item.clone()])))
        }
        scalar => Some(serialize_value(scalar)),
    }
}

/// `#RRGGBB` (also `#RRGGBBAA`, alpha dropped), quoted or not, to RGB (`mica_core::color`);
/// anything else is `None`.
pub fn parse_colour(s: &str) -> Option<[u8; 3]> {
    mica_core::color::parse_hex(s.trim().trim_matches('"'))
}

/// Sets `key` for one tool: element `tool` of a vector option, or the whole value for scalars.
pub fn set_tool_value(
    config: &mut Config,
    key: &str,
    raw: &str,
    tool: usize,
) -> Result<(), String> {
    let Some(def) = option_defs().get(key) else {
        return config.set_raw(key, raw).map_err(|e| e.to_string());
    };
    if !def.ty.is_vector() {
        return config.set_raw(key, raw).map_err(|e| e.to_string());
    }
    let parsed = parse_value(def.ty, raw)?;
    let ConfigValue::Vector(items) = parsed else {
        return Err("expected a vector".into());
    };
    let element = items.first().cloned().unwrap_or(None);
    let mut values = match config.get(key) {
        Some(ConfigValue::Vector(v)) => v.clone(),
        _ => Vec::new(),
    };
    if values.len() <= tool {
        let pad = values.last().cloned().unwrap_or(None);
        values.resize(tool + 1, pad);
    }
    values[tool] = element;
    config.set(key, ConfigValue::Vector(values));
    Ok(())
}

impl ProjectSettings {
    /// Slices with the project's own values: its embedded configuration over `config` (the
    /// composed presets it names), then its colours. Returns the notes for rejected values.
    /// Resolve its preset names first, then apply explicit caller overrides afterwards.
    pub fn apply(&self, config: &mut Config) -> Result<Vec<String>, String> {
        let (layer, notes) = self.layer(config, None, Mode::Embedded);
        layer.apply(config);
        self.apply_colours(config)?;
        Ok(notes)
    }

    /// The project's filament colours, one per tool of `config`.
    pub fn apply_colours(&self, config: &mut Config) -> Result<(), String> {
        let tools = config.vector_len("nozzle_diameter").max(1);
        for (tool, colour) in self.colours.iter().take(tools).enumerate() {
            if let Some(text) = colour {
                set_tool_value(config, "filament_colour", text, tool)?;
            }
        }
        Ok(())
    }
}

/// Overlay another resolved filament's vector values into one physical tool slot.
/// Used after resolving each project filament against the selected printer and process.
pub fn merge_filament(config: &mut Config, filament: &Config, tool: usize) -> Result<(), String> {
    for (key, _) in filament.values() {
        if (option_defs()
            .get(key)
            .is_some_and(|d| d.scope == Scope::Filament)
            || key == "filament_settings_id")
            && matches!(filament.get(key), Some(ConfigValue::Vector(_)))
            && let Some(raw) = tool_value(filament, key, 0)
        {
            set_tool_value(config, key, &raw, tool)?;
        }
    }
    // Orca-only filament vectors (`nozzle_temperature`, `nozzle_temperature_range_high`, …) are kept
    // typed for templates and as option text in extras; both follow the tool's own filament, or
    // every tool would keep the first filament's values there.
    let orca_filament = |key: &str| {
        !option_defs().contains_key(key)
            && [Dialect::Orca242, Dialect::Snapmaker240]
                .into_iter()
                .find_map(|d| crate::orca::option_scope(key, d))
                == Some(Scope::Filament)
    };
    for (key, value) in filament.values() {
        if let ConfigValue::Vector(items) = value
            && orca_filament(key)
            && let Some(element) = items.first()
        {
            let mut values = match config.get(key) {
                Some(ConfigValue::Vector(v)) => v.clone(),
                _ => Vec::new(),
            };
            if values.len() <= tool {
                let pad = values.last().cloned().unwrap_or_else(|| element.clone());
                values.resize(tool + 1, pad);
            }
            values[tool] = element.clone();
            config.set(key, ConfigValue::Vector(values));
        }
    }
    for (key, raw) in filament.extras() {
        let numeric = [Dialect::Orca242, Dialect::Snapmaker240]
            .into_iter()
            .find_map(|d| crate::orca::option_type(key, d))
            .is_some_and(|t| {
                matches!(
                    t,
                    "Ints" | "Floats" | "Bools" | "Percents" | "FloatsOrPercents"
                )
            });
        if !numeric || !orca_filament(key) {
            continue;
        }
        let Some(element) = raw.split(',').next().filter(|e| !e.is_empty()) else {
            continue;
        };
        let mut values: Vec<&str> = config
            .extra(key)
            .map(|v| v.split(',').collect())
            .unwrap_or_default();
        if values.len() <= tool {
            let pad = values.last().copied().unwrap_or(element);
            values.resize(tool + 1, pad);
        }
        values[tool] = element;
        let joined = values.join(",");
        config.set_raw(key, &joined).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plate_grid_uses_the_archive_bed_without_applying_embedded_presets() {
        let p = mica_core::threemf::Project {
            config: Some(mica_core::threemf::RawConfig {
                path: "Metadata/project_settings.config".into(),
                text: r#"{"printable_area":["0.5x1","270.5x1","270.5x271","0.5x271"]}"#.into(),
            }),
            ..Default::default()
        };
        assert_eq!(Mapper::default().plate_dimensions(&p), Some((270., 270.)));
    }

    #[test]
    fn project_filament_edits_reach_the_orca_named_copy() {
        let mut c = map_config(
            &serde_json::from_value(serde_json::json!({
                "nozzle_diameter": ["0.4", "0.4", "0.4", "0.4"],
                "nozzle_temperature": ["215", "215", "215", "215"],
            }))
            .unwrap(),
            Dialect::Orca242,
        )
        .config;
        assert_eq!(c.float_at("nozzle_temperature", 1), Some(215.));
        ProjectSettings::parse(&orca_config())
            .unwrap()
            .apply(&mut c)
            .unwrap();
        for key in ["temperature", "nozzle_temperature"] {
            assert_eq!(c.float_at(key, 0), Some(215.), "{key}");
            assert_eq!(c.float_at(key, 1), Some(220.), "{key}");
        }
    }

    #[test]
    fn merge_filament_moves_orca_only_vectors_into_the_tool_slot() {
        let map = |pairs: serde_json::Value| {
            map_config(&serde_json::from_value(pairs).unwrap(), Dialect::Orca242).config
        };
        let mut c = map(serde_json::json!({
            "nozzle_diameter": ["0.4", "0.4", "0.4"],
            "nozzle_temperature": ["220", "220", "220"],
            "nozzle_temperature_range_high": ["240", "240", "240"],
        }));
        let petg = map(serde_json::json!({
            "nozzle_diameter": ["0.4", "0.4", "0.4"],
            "nozzle_temperature": ["255", "255", "255"],
            "nozzle_temperature_range_high": ["260", "260", "260"],
        }));
        assert!(c.extra("nozzle_temperature_range_high").is_some());
        merge_filament(&mut c, &petg, 1).unwrap();
        assert_eq!(c.float_at("temperature", 1), Some(255.));
        for (key, pla, petg) in [
            ("nozzle_temperature", 220., 255.),
            ("nozzle_temperature_range_high", 240., 260.),
        ] {
            assert_eq!(c.float_at(key, 0), Some(pla), "{key}");
            assert_eq!(c.float_at(key, 1), Some(petg), "{key}");
            assert_eq!(c.float_at(key, 2), Some(pla), "{key}");
        }
        assert_eq!(c.extra("nozzle_temperature"), Some("220,255,220"));
        assert_eq!(
            c.extra("nozzle_temperature_range_high"),
            Some("240,260,240")
        );
    }

    #[test]
    fn orca_names_are_translated() {
        let mut raw = Settings::new();
        raw.insert("wall_loops".into(), "4".into());
        raw.insert("sparse_infill_density".into(), "30%".into());
        let mut ignored = BTreeSet::new();
        let patch = Mapper::default().patch(&raw, true, &mut ignored).unwrap();
        assert_eq!(patch.int("perimeters"), Some(4));
        assert_eq!(patch.float("fill_density"), Some(30.));
        let mut prusa = Settings::new();
        prusa.insert("perimeters".into(), "5".into());
        let patch = Mapper::default()
            .patch(&prusa, false, &mut ignored)
            .unwrap();
        assert_eq!(patch.int("perimeters"), Some(5));
    }

    const ORCA: &str = r##"{
        "printer_settings_id": "Snapmaker U1 (0.4 nozzle) - Copy",
        "printer_model": "Snapmaker U1",
        "printer_variant": "0.4",
        "print_settings_id": "0.20 Standard @Snapmaker U1 (0.4 nozzle)",
        "filament_settings_id": ["Generic PLA Silk @System", "Generic PLA @System", "Generic PLA @System", "Generic PLA @System"],
        "filament_colour": ["#CB8F29", "#000000", "#FFFFFF", "#FB0207"],
        "filament_type": ["PLA", "PLA", "PLA", "PLA"],
        "nozzle_temperature": ["215", "220", "210", "210"],
        "different_settings_to_system": ["enable_support;sparse_infill_density;support_type", "", "nozzle_temperature", "", "", "print_host"],
        "enable_support": "1",
        "sparse_infill_density": "8%",
        "support_type": "tree(manual)",
        "print_host": "192.0.2.5",
        "wipe_tower_x": ["165", "201.433", "x"],
        "wipe_tower_y": ["233.362", "221.855", "244.697"],
        "printable_area": ["0.5x1", "270.5x1", "270.5x271", "0.5x271"]
    }"##;

    fn orca_config() -> RawConfig {
        RawConfig {
            path: "Metadata/project_settings.config".into(),
            text: ORCA.into(),
        }
    }

    /// GUI monkey test, seed 3004: a project saved with a damaged bed
    /// (`0x0,0x0,360x360,0x360`) slices on the printer's bed, with a note.
    #[test]
    fn a_damaged_project_bed_is_rejected_with_a_note() {
        let text = ORCA.replace(
            r#""printable_area": ["0.5x1", "270.5x1", "270.5x271", "0.5x271"]"#,
            r#""printable_area": ["0x0", "0x0", "360x360", "0x360"]"#,
        );
        assert_ne!(text, ORCA);
        let s = ProjectSettings::parse(&RawConfig {
            path: "Metadata/project_settings.config".into(),
            text,
        })
        .unwrap();
        let mut base = Config::defaults();
        base.set_raw("bed_shape", "0.5x1,270.5x1,270.5x271,0.5x271")
            .unwrap();
        let (layer, notes) = s.layer(&base, Some(Dialect::Orca242), Mode::Embedded);
        assert!(layer.printer.get("bed_shape").is_none());
        assert!(layer.process.get("bed_shape").is_none());
        assert!(
            notes.iter().any(|n| n.contains("bed shape rejected")),
            "{notes:?}"
        );
        // A sound bed is taken.
        let s = ProjectSettings::parse(&orca_config()).unwrap();
        let mut other = Config::defaults();
        other
            .set_raw("bed_shape", "0x0,200x0,200x200,0x200")
            .unwrap();
        let (layer, notes) = s.layer(&other, Some(Dialect::Orca242), Mode::Embedded);
        assert!(
            layer.printer.get("bed_shape").is_some() || layer.process.get("bed_shape").is_some()
        );
        assert!(!notes.iter().any(|n| n.contains("bed shape")));
    }

    #[test]
    fn orca_project_settings_name_presets_colours_and_listed_changes() {
        let s = ProjectSettings::parse(&orca_config()).unwrap();
        assert_eq!(s.dialect, ProjectDialect::Orca);
        assert_eq!(
            s.printer.as_deref(),
            Some("Snapmaker U1 (0.4 nozzle) - Copy")
        );
        assert_eq!(s.printer_model.as_deref(), Some("Snapmaker U1"));
        assert_eq!(s.printer_variant.as_deref(), Some("0.4"));
        assert_eq!(
            s.process.as_deref(),
            Some("0.20 Standard @Snapmaker U1 (0.4 nozzle)")
        );
        assert_eq!(s.filaments.len(), 4);
        assert_eq!(s.tool_count(), 4);
        assert_eq!(
            s.colours,
            ["#CB8F29", "#000000", "#FFFFFF", "#FB0207"].map(|c| Some(c.to_string()))
        );
        assert_eq!(
            s.towers,
            [Some([165., 233.362]), Some([201.433, 221.855]), None]
        );
        let c = s.changes.clone();
        assert_eq!(
            c.process.get("fill_density").map(String::as_str),
            Some("8%")
        );
        assert_eq!(
            c.process.get("support_material").map(String::as_str),
            Some("1")
        );
        assert!(c.printer.contains_key("print_host"));
        assert_eq!(c.filaments.len(), 4);
        assert_eq!(
            c.filaments[1].get("temperature").map(String::as_str),
            Some("220")
        );
        assert!(c.filaments[0].is_empty() && c.filaments[2].is_empty());
        // `tree(manual)`: enforcers only, Orca's default tree style is organic.
        assert_eq!(
            c.process.get("support_material_auto").map(String::as_str),
            Some("0")
        );
        assert_eq!(
            c.process.get("support_material_style").map(String::as_str),
            Some("organic")
        );
        assert_eq!(c.count(), 6);
        // The tolerant mapper reports the project's tool count; the strict one does not.
        let project = mica_core::threemf::Project {
            config: Some(orca_config()),
            ..Default::default()
        };
        assert_eq!(Mapper::default().tool_count(&project), None);
        assert_eq!(
            Mapper {
                project_tools: true
            }
            .tool_count(&project),
            Some(4)
        );
    }

    #[test]
    fn prusa_project_settings_are_diffed_against_the_base_presets() {
        // PrusaSlicer's own form: every option as a `; key = value` line.
        let raw = RawConfig {
            path: "Metadata/Slic3r_PE.config".into(),
            text: "; generated by PrusaSlicer 2.9.6 on 2026-09-09 at 09:46:57 UTC\n\n; printer_settings_id = Original Prusa XL - 5T Input Shaper 0.4 nozzle\n; print_settings_id = 0.20mm SPEED @XLIS 0.4\n; filament_settings_id = \"Prusament PLA @XLIS\";\"Prusament PETG @XLIS\"\n; filament_colour = #FF8000;#0080FF\n; filament_type = PLA;PETG\n; perimeters = 3\n; temperature = 215,250\n; layer_height = 0.2\n; retract_length = 0.8,0.8\n".into(),
        };
        let s = ProjectSettings::parse(&raw).unwrap();
        assert_eq!(s.dialect, ProjectDialect::Prusa);
        assert_eq!(
            s.printer.as_deref(),
            Some("Original Prusa XL - 5T Input Shaper 0.4 nozzle")
        );
        assert_eq!(s.filaments, ["Prusament PLA @XLIS", "Prusament PETG @XLIS"]);
        assert_eq!(
            s.colours,
            [Some("#FF8000".to_string()), Some("#0080FF".to_string())]
        );
        let mut base = Config::defaults();
        base.set_raw("nozzle_diameter", "0.4,0.4").unwrap();
        base.set_raw("perimeters", "2").unwrap();
        base.set_raw("layer_height", "0.2").unwrap();
        base.set_raw("temperature", "215,215").unwrap();
        base.set_raw("retract_length", "0.8,0.8").unwrap();
        let (l, notes) = s.layer(&base, None, Mode::Embedded);
        assert!(notes.is_empty());
        assert_eq!(l.process.int("perimeters"), Some(3));
        assert!(l.process.get("layer_height").is_none());
        assert!(l.filaments[0].values().next().is_none());
        assert_eq!(l.filaments[1].float_at("temperature", 0), Some(250.));
        assert_eq!(l.filaments[1].string_at("filament_type", 0), Some("PETG"));
        assert!(l.printer.values().next().is_none());
        assert_eq!(l.count(&base), 3);
        // Prusa projects list no changes: updating to today's presets keeps none.
        assert!(s.layer(&base, None, Mode::Listed).0.is_empty());
        assert!(
            ProjectSettings::parse(&RawConfig {
                path: "Metadata/other.config".into(),
                text: String::new()
            })
            .is_none()
        );
    }

    /// One object value Orca's schema rejects failed the whole project in the GUI, which then
    /// could not even open it to repair it. The editor's mapper reports and skips it; the
    /// CLI's stays strict.
    #[test]
    fn the_editor_skips_a_rejected_object_value() {
        let raw = mica_core::threemf::Settings::from([
            // Orca reads "50%" as 50 (stream extraction stops at the '%'), so use a value
            // with no leading number, which Orca's schema really rejects.
            ("top_surface_speed".to_string(), "fast".to_string()),
            ("wall_loops".to_string(), "3".to_string()),
        ]);
        let mut ignored = BTreeSet::new();
        let strict = Mapper::default().patch(&raw, true, &mut ignored);
        assert!(strict.is_err());
        let mut ignored = BTreeSet::new();
        let config = Mapper {
            project_tools: true,
        }
        .patch(&raw, true, &mut ignored)
        .unwrap();
        assert_eq!(config.int("perimeters"), Some(3));
        assert!(
            ignored.iter().any(|i| i.contains("top_surface_speed")),
            "{ignored:?}"
        );
    }

    #[test]
    fn a_project_slices_like_the_slicer_that_saved_it_unless_it_records_its_rules() {
        use mica_core::rules::{KEY, Rules};
        // An Orca project on a printer that slices like PrusaSlicer: Orca's rules.
        let base = Config::defaults();
        let mut s = ProjectSettings::parse(&orca_config()).unwrap();
        let (layer, _) = s.layer(&base, None, Mode::Embedded);
        let mut c = base.clone();
        layer.apply(&mut c);
        assert_eq!(Rules::of(&c), Rules::OrcaSlicer);
        // Mica's sidecar records PrusaSlicer's rules: they win.
        s.mica.insert(KEY.into(), "prusaslicer".into());
        let (layer, _) = s.layer(&base, None, Mode::Embedded);
        let mut c = base.clone();
        Rules::OrcaSlicer.set(&mut c);
        layer.apply(&mut c);
        assert_eq!(Rules::of(&c), Rules::PrusaSlicer);
        // A PrusaSlicer project (no Mica key) on an Orca printer: PrusaSlicer's rules; with
        // the key Mica writes, the project's own choice.
        let mut orca_base = Config::defaults();
        Rules::OrcaSlicer.set(&mut orca_base);
        for (text, expected) in [
            ("; perimeters = 3\n", Rules::PrusaSlicer),
            (
                "; perimeters = 3\n; mica_slicing_rules = orcaslicer\n",
                Rules::OrcaSlicer,
            ),
        ] {
            let p = ProjectSettings::parse(&RawConfig {
                path: "Metadata/Slic3r_PE.config".into(),
                text: text.into(),
            })
            .unwrap();
            let (layer, _) = p.layer(&orca_base, None, Mode::Embedded);
            let mut c = orca_base.clone();
            layer.apply(&mut c);
            assert_eq!(Rules::of(&c), expected, "{text}");
        }
    }
}
