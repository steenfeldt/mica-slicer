// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: orca@8500fcdcca src/libslic3r/PresetBundle.cpp :: PresetBundle::load_vendor_configs_from_json, PresetBundle::full_fff_config
// Source: orca@8500fcdcca src/libslic3r/Preset.cpp :: PresetCollection::load_presets, Preset::normalize
// Source: orca@8500fcdcca src/libslic3r/Preset.cpp :: is_compatible_with_printer, is_compatible_with_print
// Source: snapmaker-orca@b1831e5dcb src/libslic3r/PresetBundle.cpp :: PresetBundle::full_fff_config
// Changes: Read-only importer, lazy name-based inheritance, explicit errors, retained unknown keys;
// Changes: one filament per tool, no GUI/cloud state; Snapmaker flow variants reduce to each tool's selected segment.
// Source: snapmaker-orca@b1831e5dcb src/libslic3r/PrintConfig.cpp :: get_config_idx, filament_flow_variant_options
// Changes: default_raw (Orca's full defaults); an empty default_bed_type falls back to Cool Plate.
// Source: orca@8500fcdcca src/libslic3r/Preset.cpp :: Preset::get_default_bed_type
// Changes: a numeric default_bed_type names Orca's BedType plate.
// Changes: machine-only vendor loading for the printer gallery; compatibility lists flatten
// Changes: and map the printer / process once (original Mica code).

//! Orca vendor/user JSON profiles. See `orca/README.md` for source evidence and limits.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Component, Path};

use serde_json::Value;

pub mod discovery;
mod flow_variant;
mod gcode_config;
mod legacy;
pub mod mapping;
mod schema;
pub use mapping::{MappingReport, MappingResult, map_config};
pub use schema::{accepts, option_scope, option_text, option_type, set_option_text};

/// JSON-shaped option values: strings or arrays of strings. Unknown values remain lossless.
pub type RawConfig = BTreeMap<String, Value>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dialect {
    Orca242,
    Snapmaker240,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Machine,
    Process,
    Filament,
}
impl Kind {
    pub fn directory(self) -> &'static str {
        match self {
            Self::Machine => "machine",
            Self::Process => "process",
            Self::Filament => "filament",
        }
    }
}

#[derive(Debug)]
pub struct Error(pub String);
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for Error {}

/// A printer or process many presets are checked against: flattened once, mapped on first need.
struct Target<'a> {
    name: &'a str,
    /// A user machine's system parent (Orca accepts it in `compatible_printers`).
    parent: Option<&'a str>,
    raw: RawConfig,
    config: std::cell::OnceCell<mica_core::config::Config>,
    printer: bool,
}

/// Metadata is not inherited. `filament_id` is the one upstream exception, handled on resolution.
#[derive(Clone, Debug, PartialEq)]
pub struct Preset {
    pub name: String,
    pub vendor: String,
    pub kind: Kind,
    pub inherits: Option<String>,
    pub instantiated: bool,
    pub from: String,
    pub is_user: bool,
    pub metadata: RawConfig,
    /// Only the file's overrides, including unknown keys.
    pub values: RawConfig,
}

#[derive(Clone, Debug)]
pub struct Bundle {
    pub dialect: Dialect,
    pub vendors: BTreeMap<String, Value>,
    /// `machine_model_list` entries by model name (`bed_model`, `bed_texture`, nozzles…).
    pub machine_models: BTreeMap<String, Value>,
    presets: BTreeMap<(Kind, String, String), Preset>,
    primary_vendor: String,
}

fn read_json(path: &Path) -> Result<Value, Error> {
    let text =
        std::fs::read_to_string(path).map_err(|e| Error(format!("{}: {e}", path.display())))?;
    serde_json::from_str(&text).map_err(|e| Error(format!("{}: {e}", path.display())))
}
fn metadata(key: &str) -> bool {
    matches!(
        key.to_ascii_lowercase().as_str(),
        "name"
            | "type"
            | "from"
            | "version"
            | "instantiation"
            | "inherits"
            | "setting_id"
            | "filament_id"
            | "description"
            | "url"
            | "is_custom_defined"
            | "renamed_from"
    )
}

const FLOW_SUPPORT_KEYS: [&str; 3] = [
    "filament_flow_support",
    "process_flow_support",
    "printer_flow_support",
];

/// Snapmaker Orca's filament options that hold one value per declared flow variant
/// (snapmaker-orca `filament_flow_variant_options`).
const FILAMENT_FLOW_VARIANT_OPTIONS: [&str; 19] = [
    "filament_flow_ratio",
    "enable_pressure_advance",
    "pressure_advance",
    "nozzle_temperature_initial_layer",
    "nozzle_temperature",
    "filament_max_volumetric_speed",
    "fan_min_speed",
    "fan_max_speed",
    "additional_cooling_fan_speed",
    "filament_retraction_length",
    "filament_retraction_speed",
    "filament_deretraction_speed",
    "filament_z_hop_types",
    "filament_wipe_distance",
    "filament_retract_length_toolchange",
    "filament_multitool_ramming",
    "filament_multitool_ramming_volume",
    "filament_multitool_ramming_flow",
    "filament_minimal_purge_on_wipe_tower",
];

/// A machine's `default_bed_type` as a plate name: Elegoo and WonderMaker profiles give
/// Orca's `BedType` number ("4"), which `Preset::get_default_bed_type` reads with `atoi`.
pub fn bed_type_name(value: &str) -> &str {
    match value.trim() {
        "1" => "Cool Plate",
        "2" => "Engineering Plate",
        "3" => "High Temp Plate",
        "4" => "Textured PEI Plate",
        "5" => "Textured Cool Plate",
        "6" => "Supertack Plate",
        _ => value,
    }
}

/// OrcaSlicer's full default configuration (what a profile without values would hold).
pub fn default_raw(dialect: Dialect) -> RawConfig {
    let mut out = schema::defaults(dialect, None);
    out.extend(schema::defaults(dialect, Some("filament")));
    out
}

impl Bundle {
    pub fn new(dialect: Dialect) -> Self {
        Self {
            dialect,
            vendors: BTreeMap::new(),
            machine_models: BTreeMap::new(),
            presets: BTreeMap::new(),
            primary_vendor: String::new(),
        }
    }

    /// A vendor's `machine_model` description (the file named by `machine_model_list`).
    pub fn machine_model(&self, name: &str) -> Option<&Value> {
        self.machine_models.get(name)
    }

    /// Read `<profiles>/<Vendor>.json` and only the subfiles listed by that index.
    /// Load a shared library first with `add_vendor` when a vendor inherits library presets.
    pub fn load_vendor(index: impl AsRef<Path>, dialect: Dialect) -> Result<Self, Error> {
        let mut b = Self::new(dialect);
        b.add_vendor(index)?;
        Ok(b)
    }

    pub fn add_vendor(&mut self, index: impl AsRef<Path>) -> Result<(), Error> {
        self.add_vendor_kinds(index, &[Kind::Machine, Kind::Process, Kind::Filament])
    }

    /// Only a vendor's printers (`machine_list` and `machine_model_list`): what a printer
    /// gallery needs, without reading the vendor's process and filament files.
    pub fn load_vendor_machines(index: impl AsRef<Path>, dialect: Dialect) -> Result<Self, Error> {
        let mut b = Self::new(dialect);
        b.add_vendor_kinds(index, &[Kind::Machine])?;
        Ok(b)
    }

    /// The vendor whose index was added last (its file stem, e.g. `Ratrig`).
    pub fn primary_vendor(&self) -> &str {
        &self.primary_vendor
    }

    fn add_vendor_kinds(&mut self, index: impl AsRef<Path>, kinds: &[Kind]) -> Result<(), Error> {
        let index = index.as_ref();
        let root = read_json(index)?;
        let vendor = index
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| Error("vendor index needs a filename".into()))?;
        let dir = index.with_extension("");
        let mut pending = Vec::new();
        for &kind in kinds {
            let key = format!("{}_list", kind.directory());
            if let Some(entries) = root.get(&key) {
                for entry in entries
                    .as_array()
                    .ok_or_else(|| Error(format!("{key}: expected array")))?
                {
                    let subpath = entry["sub_path"]
                        .as_str()
                        .ok_or_else(|| Error(format!("{key}: missing sub_path")))?;
                    if Path::new(subpath)
                        .components()
                        .any(|c| !matches!(c, Component::Normal(_)))
                    {
                        return Err(Error(format!("unsafe vendor sub_path: {subpath}")));
                    }
                    let mut preset =
                        Self::parse_preset(kind, &read_json(&dir.join(subpath))?, false)?;
                    preset.vendor = vendor.into();
                    if entry["name"].as_str() != Some(&preset.name) {
                        return Err(Error(format!("index/file name mismatch: {subpath}")));
                    }
                    pending.push(preset);
                }
            }
        }
        self.insert_presets(pending)?;
        if let Some(models) = root.get("machine_model_list").and_then(Value::as_array) {
            for entry in models {
                let Some(subpath) = entry["sub_path"].as_str() else {
                    continue;
                };
                if Path::new(subpath)
                    .components()
                    .any(|c| !matches!(c, Component::Normal(_)))
                {
                    return Err(Error(format!("unsafe vendor sub_path: {subpath}")));
                }
                let model = read_json(&dir.join(subpath))?;
                let name = model["name"]
                    .as_str()
                    .or(entry["name"].as_str())
                    .unwrap_or_default()
                    .to_string();
                if !name.is_empty() {
                    self.machine_models.insert(name, model);
                }
            }
        }
        self.vendors.insert(vendor.into(), root);
        self.primary_vendor = vendor.into();
        Ok(())
    }

    /// Accept an application directory, its `user/default`, or the directory containing the three
    /// preset folders. Files are never rewritten; `.info`/cloud credentials are not imported.
    pub fn load_user_presets(&mut self, path: impl AsRef<Path>) -> Result<(), Error> {
        let path = path.as_ref();
        let root = if path.join("user/default").is_dir() {
            path.join("user/default")
        } else {
            path.to_path_buf()
        };
        if !root.is_dir() {
            return Err(Error(format!("missing user directory {}", root.display())));
        }
        let mut pending = Vec::new();
        for kind in [Kind::Machine, Kind::Process, Kind::Filament] {
            let folder = root.join(kind.directory());
            if !folder.exists() {
                continue;
            }
            let mut paths = std::fs::read_dir(&folder)
                .map_err(|e| Error(e.to_string()))?
                .map(|e| e.map(|e| e.path()))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| Error(e.to_string()))?;
            paths.sort();
            for path in paths {
                if path.extension().is_some_and(|e| e == "json") {
                    pending.push(Self::parse_preset(kind, &read_json(&path)?, true)?);
                }
            }
        }
        // Transactional: bad references must not partially install a user bundle.
        let mut candidate = self.clone();
        candidate.insert_presets(pending.clone())?;
        for p in pending {
            candidate.flatten(p.kind, &p.name)?;
        }
        *self = candidate;
        Ok(())
    }

    /// Adds one user preset (a Mica profile imported from Orca's `user/` folder): `values` are
    /// the preset file's own keys, on top of `inherits`. Replaces a user preset of the same name;
    /// a system preset of that name, a missing parent or a bad value is refused and nothing
    /// changes.
    pub fn add_user_preset(
        &mut self,
        kind: Kind,
        name: &str,
        inherits: &str,
        values: &RawConfig,
    ) -> Result<(), Error> {
        let mut json: serde_json::Map<String, Value> = values
            .iter()
            .filter(|(k, _)| !matches!(k.as_str(), "name" | "inherits" | "type" | "instantiation"))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        json.insert("name".into(), Value::String(name.into()));
        if !inherits.is_empty() {
            json.insert("inherits".into(), Value::String(inherits.into()));
        }
        let preset = Self::parse_preset(kind, &Value::Object(json), true)?;
        let key = (kind, String::new(), name.to_string());
        if self
            .presets
            .get(&key)
            .or_else(|| self.preset(kind, name))
            .is_some_and(|p| !p.is_user)
        {
            return Err(Error(format!("a system preset is named {name}")));
        }
        let old = self.presets.insert(key.clone(), preset);
        if let Err(e) = self.flatten(kind, name) {
            match old {
                Some(old) => self.presets.insert(key, old),
                None => self.presets.remove(&key),
            };
            return Err(e);
        }
        Ok(())
    }

    /// Removes a user preset added by `load_user_presets` or `add_user_preset`.
    pub fn remove_user_preset(&mut self, kind: Kind, name: &str) {
        self.presets
            .retain(|(k, _, n), p| !(*k == kind && n == name && p.is_user));
    }

    fn insert_presets(&mut self, pending: Vec<Preset>) -> Result<(), Error> {
        let mut names = BTreeMap::new();
        for p in &pending {
            let key = (p.kind, p.vendor.clone(), p.name.clone());
            if self.presets.get(&key).is_some_and(|old| old != p)
                || names.insert(key, p).is_some_and(|old| old != p)
            {
                return Err(Error(format!("duplicate {:?} preset {}", p.kind, p.name)));
            }
        }
        for p in pending {
            self.presets
                .insert((p.kind, p.vendor.clone(), p.name.clone()), p);
        }
        Ok(())
    }

    fn parse_preset(kind: Kind, json: &Value, is_user: bool) -> Result<Preset, Error> {
        let obj = json
            .as_object()
            .ok_or_else(|| Error("preset must be a JSON object".into()))?;
        let name = obj
            .get("name")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| Error("preset missing name".into()))?
            .to_string();
        if let Some(ty) = obj.get("type")
            && ty != kind.directory()
        {
            return Err(Error(format!("{name}: wrong preset type {ty}")));
        }
        let instantiated = match obj.get("instantiation").and_then(Value::as_str) {
            Some("true") => true,
            Some("false") => false,
            None if is_user => true,
            _ => {
                return Err(Error(format!(
                    "{name}: instantiation must be string true/false"
                )));
            }
        };
        let mut md = RawConfig::new();
        let mut values = RawConfig::new();
        for (k, v) in obj {
            if metadata(k) {
                md.insert(k.to_ascii_lowercase(), v.clone());
            } else {
                values.insert(k.clone(), v.clone());
            }
        }
        let inherits = match obj.get("inherits") {
            None => None,
            Some(Value::String(s)) if s.is_empty() => None,
            Some(Value::String(s)) => Some(s.clone()),
            _ => return Err(Error(format!("{name}: inherits must be a single name"))),
        };
        let from = match obj.get("from") {
            None => if is_user { "user" } else { "system" }.to_string(),
            Some(Value::String(s)) => s.clone(),
            _ => return Err(Error(format!("{name}: from must be a string"))),
        };
        Ok(Preset {
            name,
            vendor: String::new(),
            kind,
            inherits,
            instantiated,
            from,
            is_user,
            metadata: md,
            values,
        })
    }

    /// Snapmaker Orca's profile format: presets declare their flow variants with
    /// `filament_flow_support` / `process_flow_support` / `printer_flow_support` and arrays of
    /// one value per variant (`docs/snapmaker-profile-format.md`). OrcaSlicer's own Snapmaker
    /// folder never uses these keys, so their presence identifies the Snapmaker fork's set.
    pub fn uses_flow_variants(&self) -> bool {
        self.presets
            .values()
            .any(|p| !p.is_user && FLOW_SUPPORT_KEYS.iter().any(|k| p.values.contains_key(*k)))
    }

    pub fn preset(&self, kind: Kind, name: &str) -> Option<&Preset> {
        self.presets
            .get(&(kind, String::new(), name.into()))
            .or_else(|| {
                self.presets
                    .get(&(kind, self.primary_vendor.clone(), name.into()))
            })
            .or_else(|| {
                self.presets
                    .values()
                    .find(|p| p.kind == kind && p.name == name)
            })
    }
    pub fn preset_names(&self, kind: Kind) -> Vec<&str> {
        self.presets
            .values()
            .filter(|p| p.kind == kind && p.instantiated)
            .map(|p| p.name.as_str())
            .collect()
    }

    pub fn flatten(&self, kind: Kind, name: &str) -> Result<RawConfig, Error> {
        self.flatten_inner(kind, name, None, &mut BTreeSet::new())
    }

    /// Explicit lists take precedence over conditions. Like Orca and Prusa, malformed conditions
    /// count as compatible. Orca also accepts a user machine's immediate system parent.
    pub fn is_compatible_with_printer(
        &self,
        kind: Kind,
        name: &str,
        printer: &str,
    ) -> Result<bool, Error> {
        let target = self.printer_target(printer)?;
        self.compatible(&self.flatten(kind, name)?, "compatible_printers", &target)
    }

    pub fn is_compatible_with_print(&self, filament: &str, process: &str) -> Result<bool, Error> {
        let target = self.process_target(process)?;
        self.compatible(
            &self.flatten(Kind::Filament, filament)?,
            "compatible_prints",
            &target,
        )
    }

    fn printer_target<'a>(&'a self, printer: &'a str) -> Result<Target<'a>, Error> {
        Ok(Target {
            name: printer,
            parent: self
                .preset(Kind::Machine, printer)
                .filter(|p| p.is_user)
                .and_then(|p| p.inherits.as_deref()),
            raw: self.flatten(Kind::Machine, printer)?,
            config: std::cell::OnceCell::new(),
            printer: true,
        })
    }

    fn process_target<'a>(&'a self, process: &'a str) -> Result<Target<'a>, Error> {
        Ok(Target {
            name: process,
            parent: None,
            raw: self.flatten(Kind::Process, process)?,
            config: std::cell::OnceCell::new(),
            printer: false,
        })
    }

    fn compatible(&self, preset: &RawConfig, key: &str, target: &Target) -> Result<bool, Error> {
        let list = preset.get(key).and_then(Value::as_array);
        if let Some(list) = list.filter(|a| !a.is_empty()) {
            return Ok(list.iter().any(|v| {
                v.as_str() == Some(target.name)
                    || target.parent.is_some_and(|p| v.as_str() == Some(p))
            }));
        }
        let condition = preset
            .get(&format!("{key}_condition"))
            .and_then(Value::as_str)
            .unwrap_or("");
        if condition.trim().is_empty() {
            return Ok(true);
        }
        let config = target
            .config
            .get_or_init(|| map_config(&target.raw, self.dialect).config);
        let mut ctx = mica_gcode::templates::config_context(config);
        if target.printer {
            ctx.set("printer_preset", mica_template::Var::string(target.name));
            ctx.set(
                "num_extruders",
                mica_template::Var::int(config.vector_len("nozzle_diameter") as i64),
            );
        }
        Ok(mica_template::evaluate_boolean(condition, &mut ctx).unwrap_or(true))
    }

    /// The instantiated presets of `kind` compatible with `printer` (and, for filaments, with
    /// `process`). The printer and process are flattened and mapped once for the whole list.
    pub fn compatible_presets(
        &self,
        kind: Kind,
        printer: &str,
        process: Option<&str>,
    ) -> Vec<&str> {
        let Ok(machine) = self.printer_target(printer) else {
            return vec![];
        };
        let print = match process.filter(|_| kind == Kind::Filament) {
            Some(p) => match self.process_target(p) {
                Ok(t) => Some(t),
                Err(_) => return vec![],
            },
            None => None,
        };
        self.preset_names(kind)
            .into_iter()
            .filter(|name| {
                let Ok(flat) = self.flatten(kind, name) else {
                    return false;
                };
                self.compatible(&flat, "compatible_printers", &machine)
                    .unwrap_or(false)
                    && print.as_ref().is_none_or(|t| {
                        self.compatible(&flat, "compatible_prints", t)
                            .unwrap_or(false)
                    })
            })
            .collect()
    }
    fn flatten_inner(
        &self,
        kind: Kind,
        name: &str,
        vendor: Option<&str>,
        visiting: &mut BTreeSet<String>,
    ) -> Result<RawConfig, Error> {
        let p = vendor
            .and_then(|v| self.presets.get(&(kind, v.into(), name.into())))
            .or_else(|| self.preset(kind, name))
            .ok_or_else(|| Error(format!("unknown {kind:?} preset {name}")))?;
        let marker = format!("{}::{name}", p.vendor);
        if !visiting.insert(marker.clone()) {
            return Err(Error(format!("inheritance cycle at {name}")));
        }
        let mut flat = match &p.inherits {
            Some(parent) => self.flatten_inner(kind, parent, Some(&p.vendor), visiting)?,
            None => schema::defaults(self.dialect, Some(kind.directory())),
        };
        flat.extend(legacy::parsed(&p.values, self.dialect));
        // Metadata is not inherited as option data. System inheritance is flattened by Orca;
        // user inheritance is retained for the cumulative project fields.
        flat.insert(
            "inherits".into(),
            Value::String(if p.is_user {
                p.inherits.clone().unwrap_or_default()
            } else {
                String::new()
            }),
        );
        if let Some(id) = p.metadata.get("filament_id").or_else(|| {
            if self.dialect == Dialect::Snapmaker240 && kind == Kind::Filament {
                p.metadata.get("setting_id")
            } else {
                None
            }
        }) {
            flat.insert("filament_id".into(), id.clone());
        }
        if kind == Kind::Machine {
            self.normalize_machine(&mut flat);
        }
        visiting.remove(&marker);
        Ok(flat)
    }

    fn normalize_machine(&self, raw: &mut RawConfig) {
        let n = raw
            .get("nozzle_diameter")
            .and_then(Value::as_array)
            .map_or(1, Vec::len);
        for (k, d) in schema::schema(self.dialect) {
            if d["extruder"] != true || k == "default_filament_profile" {
                continue;
            }
            if let Some(v) = raw.get_mut(k)
                && let Ok(Value::Array(mut a)) = schema::normalized(v, d)
            {
                let first = a
                    .first()
                    .cloned()
                    .or_else(|| d["default"].as_array()?.first().cloned());
                if let Some(first) = first {
                    a.resize(n, first);
                    *v = Value::Array(a);
                }
            }
        }
    }

    /// Orca precedence: full defaults → process → filament defaults → printer → filaments.
    /// `filaments` is ordered by tool; each vector contributes its first element per tool.
    pub fn full_config(
        &self,
        printer: &str,
        process: &str,
        filaments: &[&str],
    ) -> Result<RawConfig, Error> {
        self.full_config_with_project(printer, process, filaments, &RawConfig::new())
    }

    /// CLI's one-filament selection fills all slots, while the exporter only prints with T0.
    /// A supplied plate overrides the machine's default plate (Orca's application state).
    pub fn single_tool_config(
        &self,
        printer: &str,
        process: &str,
        filament: &str,
        bed: Option<&str>,
    ) -> Result<mica_core::config::Config, Error> {
        let machine = self.flatten(Kind::Machine, printer)?;
        let n = machine
            .get("nozzle_diameter")
            .and_then(Value::as_array)
            .map_or(1, Vec::len);
        let bed = bed
            .or_else(|| machine.get("default_bed_type").and_then(Value::as_str))
            .filter(|b| !b.is_empty())
            .map(bed_type_name)
            .unwrap_or("Cool Plate");
        let project = RawConfig::from([("curr_bed_type".into(), Value::String(bed.into()))]);
        let raw = self.full_config_with_project(printer, process, &vec![filament; n], &project)?;
        let mapped = map_config(&raw, self.dialect);
        let rejected: Vec<_> = mapped
            .report
            .entries
            .iter()
            .filter(|e| e.disposition == mapping::Disposition::Rejected)
            .map(|e| format!("{}: {}", e.key, e.reason))
            .collect();
        if !rejected.is_empty() {
            return Err(Error(rejected.join("\n")));
        }
        Ok(mapped.config)
    }

    /// Project state (bed selection, colors, placement, flush matrix) overlays the printer before filaments.
    pub fn full_config_with_project(
        &self,
        printer: &str,
        process: &str,
        filaments: &[&str],
        project: &RawConfig,
    ) -> Result<RawConfig, Error> {
        if filaments.is_empty() {
            return Err(Error("select at least one filament".into()));
        }
        for (kind, name) in [(Kind::Machine, printer), (Kind::Process, process)]
            .into_iter()
            .chain(filaments.iter().map(|n| (Kind::Filament, *n)))
        {
            if !self.preset(kind, name).is_some_and(|p| p.instantiated) {
                return Err(Error(format!("preset {name} is missing or abstract")));
            }
        }
        for (kind, name) in std::iter::once((Kind::Process, process))
            .chain(filaments.iter().map(|n| (Kind::Filament, *n)))
        {
            if !self.is_compatible_with_printer(kind, name, printer)? {
                return Err(Error(format!(
                    "{kind:?} preset {name:?} is not compatible with {printer:?}"
                )));
            }
            if kind == Kind::Filament && !self.is_compatible_with_print(name, process)? {
                return Err(Error(format!(
                    "filament preset {name:?} is not compatible with {process:?}"
                )));
            }
        }
        let machine = self.flatten(Kind::Machine, printer)?;
        let print = self.flatten(Kind::Process, process)?;
        let fs = filaments
            .iter()
            .map(|n| self.flatten(Kind::Filament, n))
            .collect::<Result<Vec<_>, _>>()?;
        let n = machine
            .get("nozzle_diameter")
            .and_then(Value::as_array)
            .map_or(1, Vec::len);
        if n > 1 && n != filaments.len() {
            return Err(Error(format!("{n} tools require {n} filament selections")));
        }
        // Bambu-style extruder variants need the upstream extruder mapping UI; do not silently
        // choose a segment. (OrcaSlicer's U1 presets declare a single variant.)
        for f in &fs {
            if f.get("filament_extruder_variant")
                .and_then(Value::as_array)
                .is_some_and(|a| a.len() > 1)
            {
                return Err(Error(
                    "filament_extruder_variant: multiple extruder variants are not supported"
                        .into(),
                ));
            }
        }
        // Snapmaker flow variants: each tool reads the segment of its project flow type
        // (`filament_volume_type`, "standard" unless the project says otherwise), falling back
        // to the filament's first variant (snapmaker-orca `get_config_idx`, Filament domain).
        let volume_types = project
            .get("filament_volume_type")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let flow_index: Vec<usize> = fs
            .iter()
            .enumerate()
            .map(|(i, f)| {
                let wanted = volume_types
                    .get(i)
                    .and_then(Value::as_str)
                    .unwrap_or("standard");
                f.get("filament_flow_support")
                    .and_then(Value::as_array)
                    .and_then(|a| a.iter().position(|v| v.as_str() == Some(wanted)))
                    .unwrap_or(0)
            })
            .collect();
        let flow_variants = self.dialect == Dialect::Snapmaker240;
        let mut out = schema::defaults(self.dialect, None);
        out.extend(print.clone());
        out.extend(schema::defaults(self.dialect, Some("filament")));
        out.extend(machine.clone());
        out.extend(project.clone());
        let keys: BTreeSet<_> = fs.iter().flat_map(|f| f.keys().cloned()).collect();
        for k in keys {
            if matches!(
                k.as_str(),
                "compatible_printers" | "compatible_prints" | "filament_id"
            ) {
                continue;
            }
            let def = schema::schema(self.dialect).get(&k);
            let vector = def.map_or_else(
                || fs[0].get(&k).is_some_and(Value::is_array),
                |d| d["default"].is_array(),
            );
            let flow_option = flow_variants
                && (FILAMENT_FLOW_VARIANT_OPTIONS.contains(&k.as_str())
                    || k == "filament_flow_support");
            if vector {
                let mut a = Vec::new();
                // Options this Orca version does not define never reach its G-code (Orca drops
                // them on load); Mica keeps them as extras, one element per tool, and an absent
                // one is empty, so they cannot fail the composition.
                let empty = Value::String(String::new());
                for (i, f) in fs.iter().enumerate() {
                    let v = f
                        .get(&k)
                        .or_else(|| def.map(|d| &d["default"]))
                        .unwrap_or(&empty);
                    let value = if let Some(d) = def {
                        schema::normalized(v, d).unwrap_or_else(|_| v.clone())
                    } else {
                        v.clone()
                    };
                    let index = if flow_option { flow_index[i] } else { 0 };
                    let element = value
                        .as_array()
                        .and_then(|a| a.get(index).or_else(|| a.first()))
                        .cloned()
                        .unwrap_or(value);
                    a.push(element);
                }
                out.insert(k, Value::Array(a));
            } else if let Some(v) = fs[0].get(&k) {
                out.insert(k, v.clone());
            }
        }
        for (source, target, values) in [
            (
                "compatible_printers_condition",
                "compatible_machine_expression_group",
                std::iter::once(&print).chain(fs.iter()).collect::<Vec<_>>(),
            ),
            (
                "compatible_prints_condition",
                "compatible_process_expression_group",
                fs.iter().collect(),
            ),
            (
                "inherits",
                "inherits_group",
                std::iter::once(&print)
                    .chain(fs.iter())
                    .chain(std::iter::once(&machine))
                    .collect(),
            ),
        ] {
            let a: Vec<_> = values
                .iter()
                .map(|f| {
                    f.get(source)
                        .cloned()
                        .unwrap_or(Value::String(String::new()))
                })
                .collect();
            if a.iter().any(|v| v.as_str().is_some_and(|s| !s.is_empty())) {
                out.insert(target.into(), Value::Array(a));
            }
        }
        if let Some(v) = print.get("compatible_printers") {
            out.insert("print_compatible_printers".into(), v.clone());
        }
        for k in [
            "compatible_printers",
            "compatible_prints",
            "compatible_printers_condition",
            "compatible_prints_condition",
            "inherits",
            "filament_id",
            "different_settings_to_system",
            "alias",
        ] {
            out.remove(k);
        }
        for k in [
            "support_filament",
            "support_interface_filament",
            "wipe_tower_filament",
        ] {
            if let Some(i) = out
                .get(k)
                .and_then(Value::as_str)
                .and_then(|s| s.parse::<i64>().ok())
            {
                out.insert(
                    k.into(),
                    Value::String(i.clamp(0, filaments.len() as i64).to_string()),
                );
            }
        }
        out.insert("printer_settings_id".into(), Value::String(printer.into()));
        out.insert("print_settings_id".into(), Value::String(process.into()));
        out.insert(
            "filament_settings_id".into(),
            Value::Array(
                filaments
                    .iter()
                    .map(|s| Value::String((*s).into()))
                    .collect(),
            ),
        );
        out.insert(
            "filament_ids".into(),
            Value::Array(
                fs.iter()
                    .map(|f| {
                        f.get("filament_id")
                            .cloned()
                            .unwrap_or(Value::String(String::new()))
                    })
                    .collect(),
            ),
        );
        if self.dialect == Dialect::Snapmaker240 {
            out.insert(
                "filament_flow_step_size".into(),
                Value::Array(vec![Value::String("1".into()); filaments.len()]),
            );
        }
        // Firmware compatibility is a printer-profile default; saved/user values win.
        if machine.get("printer_model").and_then(Value::as_str) == Some("Snapmaker U1") {
            out.entry("mica_gcode_identity".into())
                .or_insert_with(|| Value::String("orca_compatible".into()));
        }
        // PresetBundle::full_fff_config: one selected variant for each tool's filament.
        out.insert(
            "filament_self_index".into(),
            Value::Array(
                (1..=filaments.len())
                    .map(|i| Value::String(i.to_string()))
                    .collect(),
            ),
        );
        out.insert("printer_technology".into(), Value::String("FFF".into()));
        Ok(out)
    }
}
