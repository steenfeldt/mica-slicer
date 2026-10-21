// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Mica's own user profiles: one TOML file per profile, storing only what differs from the
//! profile it inherits from.
//!
//! ```toml
//! # Mica Slicer profile
//! format = 1
//! kind = "filament"
//! name = "Extrudr GreenTEC Pro @COREONE HF0.4 Tweaked"
//! inherits = "Extrudr GreenTEC Pro @COREONE HF0.4"
//! inherits_source = "PrusaResearch"
//! inherits_version = "2.5.11"
//! imported_from = "PrusaSlicer: …/filament/Extrudr GreenTEC Pro @COREONE HF0.4 Tweaked.ini"
//!
//! [import]
//! source = "prusaslicer"
//! path = "…/filament/Extrudr GreenTEC Pro @COREONE HF0.4 Tweaked.ini"
//! name = "Extrudr GreenTEC Pro @COREONE HF0.4 Tweaked"
//! date = 1792000000
//! hash = "5f1d…"
//!
//! [settings]
//! first_layer_temperature = "225"
//! ```
//!
//! Setting values are kept in PrusaSlicer's text form, exactly as PrusaSlicer would write them.
//! A profile imported from OrcaSlicer or Snapmaker Orca also keeps the preset's own JSON values
//! in an `[orca]` table (strings and arrays of strings, as Orca writes them); it is registered in
//! the Orca bundle as the user preset it was, and `[settings]` holds Mica's changes on top.
//! `[import]` records where an imported profile came from (see `crate::import`).

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use mica_core::config::{Config, serialize_value};

use crate::import::Source;
use crate::orca;
use crate::prusa::{Bundle, BundleError, Flat, Kind};

pub const FORMAT_VERSION: i64 = 1;

#[derive(Debug)]
pub enum ProfileError {
    Io(PathBuf, std::io::Error),
    Parse(PathBuf, String),
    Bundle(BundleError),
    NotFound(Kind, String),
}

impl fmt::Display for ProfileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProfileError::Io(p, e) => write!(f, "{}: {e}", p.display()),
            ProfileError::Parse(p, e) => write!(f, "{}: {e}", p.display()),
            ProfileError::Bundle(e) => write!(f, "{e}"),
            ProfileError::NotFound(kind, name) => {
                write!(f, "no user {} profile named \"{name}\"", kind_name(*kind))
            }
        }
    }
}

impl std::error::Error for ProfileError {}

impl From<BundleError> for ProfileError {
    fn from(e: BundleError) -> Self {
        ProfileError::Bundle(e)
    }
}

pub fn kind_name(kind: Kind) -> &'static str {
    match kind {
        Kind::Printer => "printer",
        Kind::Print => "print",
        Kind::Filament => "filament",
    }
}

fn kind_from_name(s: &str) -> Option<Kind> {
    match s {
        "printer" => Some(Kind::Printer),
        "print" => Some(Kind::Print),
        "filament" => Some(Kind::Filament),
        _ => None,
    }
}

/// A user profile: overrides on top of a parent profile.
#[derive(Clone, Debug, PartialEq)]
pub struct UserProfile {
    pub kind: Kind,
    pub name: String,
    /// Parent profile name (a vendor profile or another user profile).
    pub inherits: String,
    /// Where the parent comes from, e.g. "PrusaResearch".
    pub inherits_source: String,
    /// Version of the parent's vendor bundle when this profile was made, to spot later changes.
    pub inherits_version: String,
    /// Free-form note about where the profile was imported from, if anywhere.
    pub imported_from: Option<String>,
    /// Overrides, PrusaSlicer option names → PrusaSlicer text values.
    pub settings: BTreeMap<String, String>,
    /// Where an imported profile came from (source slicer, file, date, content hash).
    pub import: Option<ImportInfo>,
    /// An Orca preset's own values (JSON strings / arrays of strings), for profiles imported
    /// from OrcaSlicer or Snapmaker Orca: registered in the Orca bundle as a user preset.
    pub orca: Option<orca::RawConfig>,
}

/// Where an imported profile came from. The file is only ever read.
#[derive(Clone, Debug, PartialEq)]
pub struct ImportInfo {
    pub source: Source,
    /// The preset file in the other slicer's settings folder.
    pub path: PathBuf,
    /// The preset's name there (Mica may have renamed it to avoid a clash).
    pub original_name: String,
    /// When it was imported (seconds since 1970).
    pub date: u64,
    /// `crate::import::hash` of the file's bytes at import, to notice later changes.
    pub hash: String,
    /// A PrusaSlicer import's `settings` as they came from the file, so a re-import can tell
    /// Mica's own changes (kept) from the source's (`crate::import::carry_edits`). Orca imports
    /// keep the source's values in `UserProfile::orca` instead.
    pub settings: Option<BTreeMap<String, String>>,
}

/// A JSON option value (Orca: strings, arrays of strings, arrays of those) as TOML.
fn json_to_toml(v: &serde_json::Value) -> Option<toml::Value> {
    use serde_json::Value;
    Some(match v {
        Value::String(s) => toml::Value::String(s.clone()),
        Value::Array(a) => toml::Value::Array(a.iter().filter_map(json_to_toml).collect()),
        Value::Bool(b) => toml::Value::String(if *b { "1" } else { "0" }.into()),
        Value::Number(n) => toml::Value::String(n.to_string()),
        Value::Null | Value::Object(_) => return None,
    })
}

fn toml_to_json(v: &toml::Value) -> serde_json::Value {
    use serde_json::Value;
    match v {
        toml::Value::String(s) => Value::String(s.clone()),
        toml::Value::Array(a) => Value::Array(a.iter().map(toml_to_json).collect()),
        other => Value::String(other.to_string()),
    }
}

impl UserProfile {
    pub fn to_toml(&self) -> String {
        let mut doc = toml::Table::new();
        doc.insert("format".into(), FORMAT_VERSION.into());
        doc.insert("kind".into(), kind_name(self.kind).into());
        doc.insert("name".into(), self.name.clone().into());
        doc.insert("inherits".into(), self.inherits.clone().into());
        doc.insert(
            "inherits_source".into(),
            self.inherits_source.clone().into(),
        );
        doc.insert(
            "inherits_version".into(),
            self.inherits_version.clone().into(),
        );
        if let Some(from) = &self.imported_from {
            doc.insert("imported_from".into(), from.clone().into());
        }
        if let Some(i) = &self.import {
            let mut t = toml::Table::new();
            t.insert("source".into(), i.source.key().into());
            t.insert("path".into(), i.path.to_string_lossy().into_owned().into());
            t.insert("name".into(), i.original_name.clone().into());
            t.insert("date".into(), (i.date.min(i64::MAX as u64) as i64).into());
            t.insert("hash".into(), i.hash.clone().into());
            if let Some(settings) = &i.settings {
                let st: toml::Table = settings
                    .iter()
                    .map(|(k, v)| (k.clone(), toml::Value::from(v.clone())))
                    .collect();
                t.insert("settings".into(), st.into());
            }
            doc.insert("import".into(), t.into());
        }
        if let Some(values) = &self.orca {
            let t: toml::Table = values
                .iter()
                .filter_map(|(k, v)| Some((k.clone(), json_to_toml(v)?)))
                .collect();
            doc.insert("orca".into(), t.into());
        }
        let settings: toml::Table = self
            .settings
            .iter()
            .map(|(k, v)| (k.clone(), toml::Value::from(v.clone())))
            .collect();
        doc.insert("settings".into(), settings.into());
        format!(
            "# Mica Slicer profile\n{}",
            toml::to_string(&doc).expect("a TOML table always serializes")
        )
    }

    pub fn from_toml(text: &str) -> Result<Self, String> {
        let doc: toml::Table = text.parse().map_err(|e: toml::de::Error| e.to_string())?;
        let s = |key: &str| {
            doc.get(key)
                .and_then(toml::Value::as_str)
                .map(str::to_string)
        };
        let format = doc
            .get("format")
            .and_then(toml::Value::as_integer)
            .unwrap_or(0);
        if format != FORMAT_VERSION {
            return Err(format!(
                "unsupported profile format {format} (expected {FORMAT_VERSION})"
            ));
        }
        let kind = s("kind")
            .and_then(|k| kind_from_name(&k))
            .ok_or("missing or invalid `kind`")?;
        let mut settings = BTreeMap::new();
        if let Some(table) = doc.get("settings").and_then(toml::Value::as_table) {
            for (k, v) in table {
                let v = v.as_str().ok_or_else(|| {
                    format!("setting `{k}` must be a string (PrusaSlicer text form)")
                })?;
                settings.insert(k.clone(), v.to_string());
            }
        }
        let import = match doc.get("import").and_then(toml::Value::as_table) {
            Some(t) => {
                let get = |k: &str| t.get(k).and_then(toml::Value::as_str).unwrap_or_default();
                let source = Source::from_key(get("source"))
                    .ok_or_else(|| format!("unknown import source {:?}", get("source")))?;
                Some(ImportInfo {
                    source,
                    path: PathBuf::from(get("path")),
                    original_name: get("name").to_string(),
                    date: t
                        .get("date")
                        .and_then(toml::Value::as_integer)
                        .unwrap_or(0)
                        .max(0) as u64,
                    hash: get("hash").to_string(),
                    settings: t.get("settings").and_then(toml::Value::as_table).map(|st| {
                        st.iter()
                            .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string())))
                            .collect()
                    }),
                })
            }
            None => None,
        };
        let orca = doc.get("orca").and_then(toml::Value::as_table).map(|t| {
            t.iter()
                .map(|(k, v)| (k.clone(), toml_to_json(v)))
                .collect::<orca::RawConfig>()
        });
        Ok(Self {
            kind,
            name: s("name").ok_or("missing `name`")?,
            inherits: s("inherits").unwrap_or_default(),
            inherits_source: s("inherits_source").unwrap_or_default(),
            inherits_version: s("inherits_version").unwrap_or_default(),
            imported_from: s("imported_from"),
            settings,
            import,
            orca,
        })
    }

    /// The slicer an imported profile came from: the `[import]` record, or (profiles imported
    /// before it existed) the `imported_from` note. `None` for Mica's own profiles.
    pub fn source(&self) -> Option<Source> {
        self.import
            .as_ref()
            .map(|i| i.source)
            .or_else(|| self.legacy_import().map(|(s, _)| s))
    }

    /// The other slicer's file an imported profile came from.
    pub fn source_path(&self) -> Option<PathBuf> {
        self.import
            .as_ref()
            .map(|i| i.path.clone())
            .or_else(|| self.legacy_import().map(|(_, p)| p))
    }

    /// `imported_from = "PrusaSlicer: <path>"` (written before `[import]` existed).
    fn legacy_import(&self) -> Option<(Source, PathBuf)> {
        let (app, path) = self.imported_from.as_deref()?.split_once(": ")?;
        let source = Source::ALL.into_iter().find(|s| s.app_name() == app)?;
        Some((source, PathBuf::from(path)))
    }

    /// The preset an Orca bundle resolves for this profile: the imported Orca preset itself
    /// (registered under this name, see [`register_orca`]), else the system parent with
    /// `settings` laid on top.
    pub fn orca_base(&self) -> &str {
        if self.orca.is_some() {
            &self.name
        } else {
            &self.inherits
        }
    }

    /// The same content as `other` (kind, parent and every value), whatever the names and
    /// import records: a duplicate.
    pub fn same_content(&self, other: &UserProfile) -> bool {
        self.kind == other.kind
            && self.inherits == other.inherits
            && self.inherits_source == other.inherits_source
            && self.settings == other.settings
            && orca_content(self.orca.as_ref()) == orca_content(other.orca.as_ref())
    }
}

/// An Orca preset's values without its bookkeeping (the name it was saved under, the app
/// version that saved it), for duplicate detection.
fn orca_content(values: Option<&orca::RawConfig>) -> Option<orca::RawConfig> {
    values.map(|v| {
        v.iter()
            .filter(|(k, _)| {
                !matches!(
                    k.as_str(),
                    "version"
                        | "from"
                        | "print_settings_id"
                        | "filament_settings_id"
                        | "printer_settings_id"
                        | "setting_id"
                        | "updated_time"
                )
            })
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    })
}

/// Registers the imported Orca presets among `profiles` in `bundle` as user presets (what
/// OrcaSlicer itself does with its `user/` folder). Returns a message per preset that could not
/// be registered (e.g. its parent is missing); the others are registered.
pub fn register_orca<'a>(
    bundle: &mut orca::Bundle,
    profiles: impl IntoIterator<Item = &'a UserProfile>,
) -> Vec<String> {
    let mut errors = Vec::new();
    for p in profiles {
        let Some(values) = &p.orca else {
            continue;
        };
        if let Err(e) = bundle.add_user_preset(orca_kind(p.kind), &p.name, &p.inherits, values) {
            errors.push(format!("{} profile \"{}\": {e}", kind_name(p.kind), p.name));
        }
    }
    errors
}

/// Whether `p` is a filament built directly on one of Orca's shared library filaments
/// (`Generic PLA @System`, …; `bundle` is any Orca vendor's, which all carry the library).
/// OrcaSlicer lists such filaments for every printer, so Mica does too, whichever vendor the
/// profile was made on.
pub fn is_library_filament(p: &UserProfile, bundle: &orca::Bundle) -> bool {
    p.kind == Kind::Filament
        && (p.inherits_source == crate::import::ORCA_LIBRARY
            || (!p.inherits_source.is_empty()
                && bundle
                    .preset(orca::Kind::Filament, &p.inherits)
                    .is_some_and(|q| !q.is_user && q.vendor == crate::import::ORCA_LIBRARY)))
}

/// Whether `p` belongs with the Orca vendor `key` (its `bundle`): built on that vendor, or a
/// filament on the shared library ([`is_library_filament`]).
pub fn on_orca_vendor(p: &UserProfile, key: &str, bundle: &orca::Bundle) -> bool {
    p.inherits_source == key || is_library_filament(p, bundle)
}

/// The Orca preset kind of a Mica profile kind.
pub fn orca_kind(kind: Kind) -> orca::Kind {
    match kind {
        Kind::Printer => orca::Kind::Machine,
        Kind::Print => orca::Kind::Process,
        Kind::Filament => orca::Kind::Filament,
    }
}

/// The configuration of an Orca selection whose printer, process and filament may be Mica user
/// profiles: the bundle composes each one's base (`UserProfile::orca_base`), then each
/// profile's own `settings` go on top. The GUI and the CLI both compose this way, so they slice
/// alike.
pub fn orca_config(
    bundle: &orca::Bundle,
    printer: (&str, Option<&UserProfile>),
    process: (&str, Option<&UserProfile>),
    filament: (&str, Option<&UserProfile>),
    bed: Option<&str>,
) -> Result<mica_core::config::Config, String> {
    let base = |(name, p): (&str, Option<&UserProfile>)| -> String {
        p.map_or(name.to_string(), |p| p.orca_base().to_string())
    };
    let mut c = bundle
        .single_tool_config(&base(printer), &base(process), &base(filament), bed)
        .map_err(|e| e.to_string())?;
    for p in [printer.1, process.1, filament.1].into_iter().flatten() {
        for (k, raw) in &p.settings {
            // Orca-only keys (`pressure_advance`…) are typed as well as kept as text, so the
            // templates and the exporter see the change.
            orca::set_option_text(&mut c, k, raw, bundle.dialect)?;
        }
    }
    Ok(c)
}

/// Normalises a raw value through Mica's typed config so formatting differences ("0.20" vs "0.2")
/// don't count as changes. Unknown keys compare verbatim.
fn normalized(key: &str, raw: &str) -> String {
    let mut c = Config::new();
    match c.set_raw(key, raw) {
        Ok(()) => c.get(key).map_or_else(|| raw.to_string(), serialize_value),
        Err(_) => raw.to_string(),
    }
}

/// Keys that describe a preset file rather than a setting the user changed.
fn is_bookkeeping(key: &str) -> bool {
    matches!(key, "inherits" | "renamed_from" | "alias")
        || key.ends_with("_settings_id")
        || key == "compatible_printers"
        || key == "compatible_prints"
}

/// Overrides in `full` (a complete preset, as PrusaSlicer saves user presets) relative to `parent`.
/// A key the parent doesn't set compares against the option's default, which is what the parent
/// effectively uses.
pub fn diff_against(full: &Flat, parent: &Flat) -> BTreeMap<String, String> {
    let defaults = mica_core::config::option_defs();
    full.iter()
        .filter(|(k, _)| !is_bookkeeping(k))
        .filter(|(k, v)| {
            let effective = parent
                .get(*k)
                .map(String::as_str)
                .or_else(|| defaults.get(k.as_str()).map(|d| d.default));
            effective.is_none_or(|p| normalized(k, p) != normalized(k, v))
        })
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

/// Imports a PrusaSlicer user preset file (`<datadir>/{print,filament,printer}/<name>.ini`). PrusaSlicer
/// stores the full flattened config plus `inherits`; only the real changes are kept.
pub fn import_prusa_user_preset(
    path: &Path,
    kind: Kind,
    bundle: &Bundle,
) -> Result<UserProfile, ProfileError> {
    let text =
        std::fs::read_to_string(path).map_err(|e| ProfileError::Io(path.to_path_buf(), e))?;
    let mut full: Flat = crate::ini::parse(&text)
        .into_iter()
        .flat_map(|s| s.entries)
        .collect();
    // A merged setting's Orca key (which Mica writes next to the PrusaSlicer one) that
    // disagrees with its counterpart was left behind by an edit in PrusaSlicer.
    mica_core::merged::reconcile_prusa_layer(&mut full, &mica_core::config::Config::defaults());
    let name = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let inherits = full.get("inherits").cloned().unwrap_or_default();
    let parent = if inherits.is_empty() {
        Flat::new()
    } else {
        bundle.flatten(kind, &inherits)?
    };
    Ok(UserProfile {
        kind,
        name,
        inherits,
        inherits_source: bundle.vendor.get("name").cloned().unwrap_or_default(),
        inherits_version: bundle
            .vendor
            .get("config_version")
            .cloned()
            .unwrap_or_default(),
        imported_from: Some(format!("PrusaSlicer: {}", path.display())),
        settings: diff_against(&full, &parent),
        import: None,
        orca: None,
    })
}

/// A user profile as a PrusaSlicer user preset file (`<datadir>/{print,filament,printer}/<name>.ini`):
/// the parent's flattened settings with the profile's overrides on top, plus `inherits`, as
/// PrusaSlicer saves one. A merged setting's PrusaSlicer counterpart follows its Orca key
/// (`mica_core::merged`); the Orca key stays (PrusaSlicer ignores it, Mica reads it back).
pub fn export_prusa_user_preset(
    profile: &UserProfile,
    bundle: &Bundle,
) -> Result<String, ProfileError> {
    let mut full = if profile.inherits.is_empty() {
        Flat::new()
    } else {
        bundle.flatten(profile.kind, &profile.inherits)?
    };
    for (k, v) in &profile.settings {
        full.insert(k.clone(), v.clone());
    }
    let mut context = mica_core::config::Config::defaults();
    for (k, v) in &full {
        let _ = context.set_raw(k, v);
    }
    for pair in mica_core::merged::PAIRS {
        if let Some(orca) = full.get(pair.orca)
            && let Some(v) = mica_core::merged::to_prusa(&context, pair, orca)
        {
            full.insert(pair.prusa.to_string(), v);
        }
    }
    full.insert("inherits".into(), profile.inherits.clone());
    let mut text = format!("# generated by Mica Slicer {}\n", mica_core::VERSION);
    for (k, v) in &full {
        text.push_str(&format!("{k} = {v}\n"));
    }
    Ok(text)
}

/// A folder of user profiles: `<root>/{printer,print,filament}/<name>.toml`, deleted ones in `<root>/trash`.
pub struct Store {
    pub root: PathBuf,
}

/// File name for a profile name: keep it readable, replace only characters file systems reject.
fn file_name(name: &str) -> String {
    let safe: String = name
        .chars()
        .map(|c| {
            if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '_'
            } else {
                c
            }
        })
        .collect();
    format!("{safe}.toml")
}

impl Store {
    /// The default location: `MICA_CONFIG_DIR` if set, else the platform's application-support
    /// folder (never the real one in tests: `mica_core::user_data`).
    pub fn default_root() -> PathBuf {
        mica_core::user_data::config_dir().join("profiles")
    }

    pub fn open(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn dir(&self, kind: Kind) -> PathBuf {
        self.root.join(kind_name(kind))
    }

    pub fn save(&self, profile: &UserProfile) -> Result<PathBuf, ProfileError> {
        let dir = self.dir(profile.kind);
        std::fs::create_dir_all(&dir).map_err(|e| ProfileError::Io(dir.clone(), e))?;
        let path = dir.join(file_name(&profile.name));
        // Written aside and renamed, so a reader (the CLI, another window) never sees half a
        // file; `list` skips the `.tmp` file.
        let tmp = dir.join(format!(".{}.tmp", file_name(&profile.name)));
        std::fs::write(&tmp, profile.to_toml()).map_err(|e| ProfileError::Io(tmp.clone(), e))?;
        std::fs::rename(&tmp, &path).map_err(|e| ProfileError::Io(path.clone(), e))?;
        Ok(path)
    }

    pub fn list(&self, kind: Kind) -> Result<Vec<UserProfile>, ProfileError> {
        let dir = self.dir(kind);
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "toml") {
                let text = std::fs::read_to_string(&path)
                    .map_err(|e| ProfileError::Io(path.clone(), e))?;
                out.push(
                    UserProfile::from_toml(&text)
                        .map_err(|e| ProfileError::Parse(path.clone(), e))?,
                );
            }
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    pub fn load(&self, kind: Kind, name: &str) -> Result<UserProfile, ProfileError> {
        self.list(kind)?
            .into_iter()
            .find(|p| p.name == name)
            .ok_or_else(|| ProfileError::NotFound(kind, name.to_string()))
    }

    /// Moves a profile to `<root>/trash/<kind>/` instead of erasing it.
    pub fn delete(&self, kind: Kind, name: &str) -> Result<PathBuf, ProfileError> {
        let from = self.dir(kind).join(file_name(name));
        if !from.is_file() {
            return Err(ProfileError::NotFound(kind, name.to_string()));
        }
        let trash = self.root.join("trash").join(kind_name(kind));
        std::fs::create_dir_all(&trash).map_err(|e| ProfileError::Io(trash.clone(), e))?;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let to = trash.join(format!(
            "{}.{stamp}.toml",
            file_name(name).trim_end_matches(".toml")
        ));
        std::fs::rename(&from, &to).map_err(|e| ProfileError::Io(from.clone(), e))?;
        Ok(to)
    }

    /// Every stored profile, printers first.
    pub fn list_all(&self) -> Result<Vec<UserProfile>, ProfileError> {
        let mut out = Vec::new();
        for kind in [Kind::Printer, Kind::Print, Kind::Filament] {
            out.extend(self.list(kind)?);
        }
        Ok(out)
    }

    /// Registers every stored profile built on `bundle`'s vendor (or on no vendor) in
    /// `bundle`, so they can be selected like vendor presets. Profiles of Orca vendors are
    /// left out.
    pub fn register_all(&self, bundle: &mut Bundle) -> Result<usize, ProfileError> {
        let mut n = 0;
        let vendor = bundle.vendor.get("name").cloned().unwrap_or_default();
        for kind in [Kind::Printer, Kind::Print, Kind::Filament] {
            for p in self.list(kind)? {
                if p.orca.is_some()
                    || !(p.inherits_source.is_empty() || p.inherits_source == vendor)
                {
                    continue;
                }
                bundle.add_user_preset(kind, &p.name, &p.inherits, &p.settings);
                n += 1;
            }
        }
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BUNDLE: &str = "\
[vendor]
name = TestVendor
config_version = 1.2.3

[filament:Base PLA]
temperature = 215
first_layer_temperature = 220
filament_type = PLA
compatible_printers_condition = printer_model==\"X\"
";

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mica-user-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn toml_round_trip() {
        let p = UserProfile {
            kind: Kind::Filament,
            name: "My PLA".into(),
            inherits: "Base PLA".into(),
            inherits_source: "TestVendor".into(),
            inherits_version: "1.2.3".into(),
            imported_from: None,
            import: None,
            orca: None,
            settings: [
                ("temperature".to_string(), "210".to_string()),
                (
                    "start_filament_gcode".to_string(),
                    "\"M572 S0.03\\nM142 S36\"".to_string(),
                ),
            ]
            .into_iter()
            .collect(),
        };
        let text = p.to_toml();
        assert!(text.starts_with("# Mica Slicer profile\n"));
        assert_eq!(UserProfile::from_toml(&text).unwrap(), p);
    }

    #[test]
    fn import_keeps_only_real_changes() {
        let bundle = Bundle::parse(BUNDLE);
        let dir = tmp("import");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("Base PLA Tweaked.ini");
        // PrusaSlicer saves the full config: unchanged values (some formatted differently) plus changes.
        std::fs::write(&file, "# generated by PrusaSlicer\ninherits = Base PLA\ntemperature = 210\nfirst_layer_temperature = 220.0\nfilament_type = PLA\nfilament_settings_id = \"\"\ncompatible_printers_condition = printer_model==\"X\"\n").unwrap();
        let p = import_prusa_user_preset(&file, Kind::Filament, &bundle).unwrap();
        assert_eq!(p.name, "Base PLA Tweaked");
        assert_eq!(p.inherits_version, "1.2.3");
        assert_eq!(p.settings.len(), 1, "{:?}", p.settings);
        assert_eq!(p.settings["temperature"], "210");
    }

    #[test]
    fn store_save_list_delete_and_use() {
        let mut bundle = Bundle::parse(BUNDLE);
        let store = Store::open(tmp("store"));
        let p = UserProfile {
            kind: Kind::Filament,
            name: "Mine/odd:name".into(),
            inherits: "Base PLA".into(),
            inherits_source: "TestVendor".into(),
            inherits_version: "1.2.3".into(),
            imported_from: None,
            import: None,
            orca: None,
            settings: [("temperature".to_string(), "205".to_string())]
                .into_iter()
                .collect(),
        };
        store.save(&p).unwrap();
        assert_eq!(store.list(Kind::Filament).unwrap(), vec![p.clone()]);
        assert_eq!(store.register_all(&mut bundle).unwrap(), 1);
        let flat = bundle.flatten(Kind::Filament, "Mine/odd:name").unwrap();
        assert_eq!(flat["temperature"], "205");
        assert_eq!(flat["first_layer_temperature"], "220");
        let trashed = store.delete(Kind::Filament, "Mine/odd:name").unwrap();
        assert!(trashed.is_file());
        assert!(store.list(Kind::Filament).unwrap().is_empty());
    }

    /// An Orca user preset imported into Mica resolves exactly as OrcaSlicer's own loading of
    /// its `user/` folder does (what Mica did before imports were asked for), survives the
    /// profile file and is left out of the PrusaSlicer bundle.
    #[test]
    fn imported_orca_preset_resolves_like_orcas_user_folder() {
        use crate::import::{self, Source, SourceDir};
        if !mica_testkit::require_orca_profiles() {
            return;
        }
        let (system, _) =
            orca::discovery::load_installed(Some(&mica_testkit::orca_profiles()), "Snapmaker")
                .unwrap();
        let home = tmp("orca-fidelity");
        let user = home.join("user/default");
        std::fs::create_dir_all(user.join("process")).unwrap();
        std::fs::create_dir_all(user.join("machine")).unwrap();
        std::fs::write(
            user.join("process/Fine.json"),
            r#"{"from":"User","inherits":"0.20 Standard @Snapmaker U1 (0.4 nozzle)","name":"Fine","print_extruder_id":["1"],"sparse_infill_density":"10%","wall_loops":"3","version":"2.4.0.7"}"#,
        )
        .unwrap();
        std::fs::write(
            user.join("machine/My U1.json"),
            r#"{"from":"User","inherits":"Snapmaker U1 (0.4 nozzle)","name":"My U1","printer_extruder_id":["1","2","3","4"],"retraction_length":["0.6","0.6","0.6","0.6"]}"#,
        )
        .unwrap();
        let mut live = system.clone();
        live.load_user_presets(&home).unwrap();
        let (found, errors) = import::scan(&[SourceDir {
            source: Source::OrcaSlicer,
            root: user,
        }]);
        assert!(errors.is_empty(), "{errors:?}");
        let store = Store::open(tmp("orca-fidelity-store"));
        for f in &found {
            let p = import::orca_profile(f, "Snapmaker", "2.0", &f.name, 1).unwrap();
            store.save(&p).unwrap();
        }
        let stored = store.list_all().unwrap();
        assert_eq!(stored.len(), 2);
        let mut imported = system.clone();
        assert!(register_orca(&mut imported, &stored).is_empty());
        for (kind, name) in [
            (orca::Kind::Process, "Fine"),
            (orca::Kind::Machine, "My U1"),
        ] {
            assert_eq!(
                imported.flatten(kind, name).unwrap(),
                live.flatten(kind, name).unwrap(),
                "{name}"
            );
        }
        let filament = "Snapmaker PLA @U1";
        let by_orca = live
            .single_tool_config("My U1", "Fine", filament, Some("High Temp Plate"))
            .unwrap();
        let printer = stored.iter().find(|p| p.name == "My U1");
        let process = stored.iter().find(|p| p.name == "Fine");
        let by_mica = orca_config(
            &imported,
            ("My U1", printer),
            ("Fine", process),
            (filament, None),
            Some("High Temp Plate"),
        )
        .unwrap();
        assert_eq!(
            by_mica.values().collect::<Vec<_>>(),
            by_orca.values().collect::<Vec<_>>()
        );
        assert_eq!(by_mica.float("fill_density"), Some(10.));
        // Not a PrusaSlicer preset.
        let mut prusa = Bundle::parse(BUNDLE);
        assert_eq!(store.register_all(&mut prusa).unwrap(), 0);
        // Removing the import from the bundle leaves the system presets.
        imported.remove_user_preset(orca::Kind::Process, "Fine");
        assert!(imported.preset(orca::Kind::Process, "Fine").is_none());
        assert!(
            imported
                .preset(
                    orca::Kind::Process,
                    "0.20 Standard @Snapmaker U1 (0.4 nozzle)"
                )
                .is_some()
        );
    }

    #[test]
    fn import_records_round_trip() {
        let p = UserProfile {
            kind: Kind::Print,
            name: "Fine (Orca)".into(),
            inherits: "0.20 Standard".into(),
            inherits_source: "Snapmaker".into(),
            inherits_version: "2.0".into(),
            imported_from: Some("OrcaSlicer: /x/Fine.json".into()),
            settings: Default::default(),
            import: Some(ImportInfo {
                source: Source::OrcaSlicer,
                path: "/x/Fine.json".into(),
                original_name: "Fine".into(),
                date: 1_792_454_400,
                hash: "00ff".into(),
                settings: Some(
                    [("temperature".to_string(), "210".to_string())]
                        .into_iter()
                        .collect(),
                ),
            }),
            orca: Some(
                [
                    ("wall_loops".to_string(), serde_json::json!("3")),
                    ("print_extruder_id".to_string(), serde_json::json!(["1"])),
                ]
                .into_iter()
                .collect(),
            ),
        };
        let back = UserProfile::from_toml(&p.to_toml()).unwrap();
        assert_eq!(back, p);
        assert_eq!(back.source(), Some(Source::OrcaSlicer));
        assert_eq!(back.orca_base(), "Fine (Orca)");
        // A profile imported before `[import]` existed still knows its source.
        let legacy = UserProfile {
            import: None,
            orca: None,
            imported_from: Some("PrusaSlicer: /p/filament/A.ini".into()),
            ..p
        };
        assert_eq!(legacy.source(), Some(Source::PrusaSlicer));
        assert_eq!(
            legacy.source_path(),
            Some(PathBuf::from("/p/filament/A.ini"))
        );
        assert_eq!(legacy.orca_base(), "0.20 Standard");
    }
}
