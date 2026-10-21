// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: orca@8500fcdcca src/libslic3r/PrintConfig.cpp :: PrintConfigDef::init_fff_params
// Source: prusaslicer@b028299c77 src/libslic3r/Config.cpp :: unescape_string_cstyle, unescape_strings_cstyle, escape_string_cstyle, escape_strings_cstyle
// Source: prusaslicer@b028299c77 src/libslic3r/PrintConfig.cpp :: PrintConfigDef::init_fff_params
// Source: prusaslicer@b028299c77 src/libslic3r/Config.hpp :: ConfigOption*::serialize, ConfigOption*::deserialize
// Changes: One typed value enum instead of PrusaSlicer's class hierarchy; same text serialization so
// Changes: PrusaSlicer INI files and Mica configs read and write identically. Option types, scopes and
// Changes: defaults come from prusa_options.tsv (generated, see xtask gen-prusa-options).
// Changes: Mica's own options (mica_options.tsv, e.g. mica_slicing_rules) join the same table.
// Changes: Setting the PrusaSlicer counterpart of a merged Orca option drops the Orca key (crate::merged).
// Source: prusaslicer@b028299c77 src/libslic3r/Config.hpp :: ConfigOptionVector::resize
// Changes: Extending a non-empty vector repeats its first element; only an empty one takes the default.

//! Typed configuration values, using PrusaSlicer's option names and text format.
//!
//! Canonical option names are PrusaSlicer's (Mica's structure follows PrusaSlicer, and Prusa profile
//! templates use these names). Options from other slicers are mapped onto them at import; options
//! Mica doesn't know are kept verbatim as strings so templates can still read them.

pub mod native;

use std::collections::BTreeMap;
use std::fmt;
use std::sync::OnceLock;

/// Value type of an option, as in PrusaSlicer's `ConfigOptionType`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OptType {
    Float,
    Floats,
    Int,
    Ints,
    Bool,
    Bools,
    Percent,
    Percents,
    FloatOrPercent,
    FloatsOrPercents,
    String,
    Strings,
    Enum,
    Enums,
    Point,
    Points,
}

impl OptType {
    fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "Float" => Self::Float,
            "Floats" => Self::Floats,
            "Int" => Self::Int,
            "Ints" => Self::Ints,
            "Bool" => Self::Bool,
            "Bools" => Self::Bools,
            "Percent" => Self::Percent,
            "Percents" => Self::Percents,
            "FloatOrPercent" => Self::FloatOrPercent,
            "FloatsOrPercents" => Self::FloatsOrPercents,
            "String" => Self::String,
            "Strings" => Self::Strings,
            "Enum" => Self::Enum,
            "Enums" => Self::Enums,
            "Point" => Self::Point,
            "Points" => Self::Points,
            _ => return None,
        })
    }

    pub fn is_vector(self) -> bool {
        matches!(
            self,
            Self::Floats
                | Self::Ints
                | Self::Bools
                | Self::Percents
                | Self::FloatsOrPercents
                | Self::Strings
                | Self::Enums
                | Self::Points
        )
    }

    fn element(self) -> Self {
        match self {
            Self::Floats => Self::Float,
            Self::Ints => Self::Int,
            Self::Bools => Self::Bool,
            Self::Percents => Self::Percent,
            Self::FloatsOrPercents => Self::FloatOrPercent,
            Self::Strings => Self::String,
            Self::Enums => Self::Enum,
            Self::Points => Self::Point,
            scalar => scalar,
        }
    }
}

/// Which preset owns an option.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    Print,
    Filament,
    Printer,
    Other,
}

#[derive(Clone, Debug)]
pub struct OptionDef {
    pub key: &'static str,
    pub ty: OptType,
    /// Vector elements may be `nil` (filament overrides of printer settings).
    pub nullable: bool,
    pub scope: Scope,
    /// Default value in PrusaSlicer's text form.
    pub default: &'static str,
    /// For percentages: the option they are a percentage of.
    pub ratio_over: Option<&'static str>,
}

/// Prusa FFF options plus neutral Orca additions, with types, scopes and defaults.
pub fn option_defs() -> &'static BTreeMap<&'static str, OptionDef> {
    static DEFS: OnceLock<BTreeMap<&'static str, OptionDef>> = OnceLock::new();
    DEFS.get_or_init(|| {
        include_str!("prusa_options.tsv")
            .lines()
            .chain(include_str!("orca_options.tsv").lines())
            .chain(include_str!("orca_options_optional.tsv").lines())
            .chain(include_str!("mica_options.tsv").lines())
            .filter(|l| !l.starts_with('#') && !l.is_empty())
            .map(|line| {
                let mut f = line.splitn(6, '\t');
                let (key, ty, nullable, scope, default) = (
                    f.next().unwrap(),
                    f.next().unwrap(),
                    f.next().unwrap(),
                    f.next().unwrap(),
                    f.next().unwrap_or(""),
                );
                let ratio_over = f.next().filter(|r| !r.is_empty());
                let def = OptionDef {
                    key,
                    ty: OptType::from_name(ty)
                        .unwrap_or_else(|| panic!("prusa_options.tsv: bad type {ty} for {key}")),
                    nullable: nullable == "1",
                    scope: match scope {
                        "print" => Scope::Print,
                        "filament" => Scope::Filament,
                        "printer" => Scope::Printer,
                        _ => Scope::Other,
                    },
                    default,
                    ratio_over,
                };
                (key, def)
            })
            .collect()
    })
}

/// Names in PrusaSlicer's FFF option table (excluding Mica and Orca additions).
pub fn prusa_option_keys() -> impl Iterator<Item = &'static str> {
    include_str!("prusa_options.tsv")
        .lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| l.split('\t').next())
}

/// Which Orca table an option comes from (`None`: PrusaSlicer's or Mica's own).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OrcaTable {
    /// `orca_options.tsv`: neutral additions with Prusa-preserving defaults.
    Neutral,
    /// `orca_options_optional.tsv`: Orca controls absent from `Config::defaults`.
    Optional,
}

pub fn orca_table(key: &str) -> Option<OrcaTable> {
    let has = |table: &str| table.lines().any(|l| l.split('\t').next() == Some(key));
    if has(include_str!("orca_options_optional.tsv")) {
        Some(OrcaTable::Optional)
    } else if has(include_str!("orca_options.tsv")) {
        Some(OrcaTable::Neutral)
    } else {
        None
    }
}

/// A single (scalar) value.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Float(f64),
    Int(i64),
    Bool(bool),
    /// Stored as written: 15 for "15%".
    Percent(f64),
    /// Either an absolute value or a percentage of something else.
    FloatOrPercent {
        value: f64,
        percent: bool,
    },
    String(String),
    Enum(String),
    Point(f64, f64),
}

/// A configuration value: one scalar, or a vector (per extruder) whose elements may be nil.
#[derive(Clone, Debug, PartialEq)]
pub enum ConfigValue {
    Scalar(Value),
    Vector(Vec<Option<Value>>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
    pub key: String,
    pub raw: String,
    pub reason: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} = {:?}: {}", self.key, self.raw, self.reason)
    }
}

impl std::error::Error for ParseError {}

fn parse_number(s: &str) -> Result<f64, String> {
    let value = s
        .trim()
        .parse::<f64>()
        .map_err(|_| format!("'{s}' is not a number"))?;
    if !value.is_finite() {
        return Err(format!("'{s}' is not a finite number"));
    }
    Ok(value)
}

fn parse_scalar(ty: OptType, s: &str) -> Result<Value, String> {
    let t = s.trim();
    Ok(match ty {
        OptType::Float => Value::Float(parse_number(t)?),
        OptType::Int => Value::Int(match t.parse::<i64>() {
            Ok(i) => i,
            // PrusaSlicer accepts "3.0" for ints in older profiles.
            Err(_) => parse_number(t)?.round() as i64,
        }),
        OptType::Bool => Value::Bool(match t {
            "1" | "true" => true,
            "0" | "false" => false,
            _ => return Err(format!("'{t}' is not a boolean")),
        }),
        OptType::Percent => Value::Percent(parse_number(t.trim_end_matches('%'))?),
        OptType::FloatOrPercent => {
            let percent = t.ends_with('%');
            Value::FloatOrPercent {
                value: parse_number(t.trim_end_matches('%'))?,
                percent,
            }
        }
        OptType::String => Value::String(unescape_string(s)?),
        OptType::Enum => Value::Enum(t.to_string()),
        OptType::Point => {
            let (x, y) = t
                .split_once('x')
                .ok_or_else(|| format!("'{t}' is not a point (XxY)"))?;
            Value::Point(parse_number(x)?, parse_number(y)?)
        }
        vector => unreachable!("parse_scalar called with vector type {vector:?}"),
    })
}

/// Parses `raw` (PrusaSlicer text form) as an option of type `ty`.
pub fn parse_value(ty: OptType, raw: &str) -> Result<ConfigValue, String> {
    if !ty.is_vector() {
        return parse_scalar(ty, raw).map(ConfigValue::Scalar);
    }
    let elem = ty.element();
    let items: Vec<String> = match ty {
        OptType::Strings => unescape_strings(raw)?,
        _ if raw.trim().is_empty() => Vec::new(),
        _ => raw.split(',').map(str::to_string).collect(),
    };
    items
        .iter()
        .map(|item| {
            if elem != OptType::String && item.trim() == "nil" {
                Ok(None)
            } else {
                parse_scalar(elem, item).map(Some)
            }
        })
        .collect::<Result<Vec<_>, _>>()
        .map(ConfigValue::Vector)
}

fn format_float(v: f64) -> String {
    // PrusaSlicer writes floats with at most 15 significant digits (so 27.99 stays "27.99"),
    // without trailing zeros.
    if v == 0.0 || !v.is_finite() {
        return if v.is_finite() {
            "0".into()
        } else {
            v.to_string()
        };
    }
    let magnitude = v.abs().log10().floor() as i32;
    let decimals = (14 - magnitude).clamp(0, 17) as usize;
    let s = format!("{v:.decimals$}");
    let s = if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.')
    } else {
        &s
    };
    if s == "-0" { "0".into() } else { s.into() }
}

fn serialize_scalar(v: &Value) -> String {
    match v {
        Value::Float(f) => format_float(*f),
        Value::Int(i) => i.to_string(),
        Value::Bool(b) => if *b { "1" } else { "0" }.into(),
        Value::Percent(p) => format!("{}%", format_float(*p)),
        Value::FloatOrPercent { value, percent } => {
            if *percent {
                format!("{}%", format_float(*value))
            } else {
                format_float(*value)
            }
        }
        Value::String(s) => escape_string(s),
        Value::Enum(e) => e.clone(),
        Value::Point(x, y) => format!("{}x{}", format_float(*x), format_float(*y)),
    }
}

/// PrusaSlicer text form of a value.
pub fn serialize_value(v: &ConfigValue) -> String {
    match v {
        ConfigValue::Scalar(s) => serialize_scalar(s),
        ConfigValue::Vector(items) => {
            if items.iter().any(|i| matches!(i, Some(Value::String(_)))) {
                let strs: Vec<String> = items
                    .iter()
                    .map(|i| match i {
                        Some(Value::String(s)) => s.clone(),
                        _ => String::new(),
                    })
                    .collect();
                return escape_strings(&strs);
            }
            items
                .iter()
                .map(|i| {
                    i.as_ref()
                        .map_or_else(|| "nil".to_string(), serialize_scalar)
                })
                .collect::<Vec<_>>()
                .join(",")
        }
    }
}

/// Undoes PrusaSlicer's C-style escaping: `\n`, `\r`, and `\x` → `x`.
pub fn unescape_string(s: &str) -> Result<String, String> {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('r') => out.push('\r'),
                Some(other) => out.push(other),
                None => return Err("dangling backslash".into()),
            }
        } else {
            out.push(c);
        }
    }
    Ok(out)
}

/// Escapes newlines and backslashes so a string fits on one INI line.
pub fn escape_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
    out
}

/// Splits a `;`-separated list where items may be quoted (`"a;b";c`).
pub fn unescape_strings(s: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    if s.is_empty() {
        return Ok(out);
    }
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    loop {
        while i < chars.len() && (chars[i] == ' ' || chars[i] == '\t') {
            i += 1;
        }
        if i == chars.len() {
            return Ok(out);
        }
        let mut buf = String::new();
        if chars[i] == '"' {
            i += 1;
            loop {
                let Some(&c) = chars.get(i) else {
                    return Err("unterminated quoted string".into());
                };
                if c == '"' {
                    i += 1;
                    break;
                }
                if c == '\\' {
                    i += 1;
                    match chars.get(i) {
                        Some('n') => buf.push('\n'),
                        Some('r') => buf.push('\r'),
                        Some(&other) => buf.push(other),
                        None => return Err("dangling backslash".into()),
                    }
                } else {
                    buf.push(c);
                }
                i += 1;
            }
        } else {
            while i < chars.len() && chars[i] != ';' {
                buf.push(chars[i]);
                i += 1;
            }
        }
        out.push(buf);
        while i < chars.len() && (chars[i] == ' ' || chars[i] == '\t') {
            i += 1;
        }
        if i == chars.len() {
            return Ok(out);
        }
        if chars[i] != ';' {
            return Err("expected ';' between strings".into());
        }
        i += 1;
    }
}

/// Joins strings with `;`, quoting those that need it.
pub fn escape_strings(strs: &[String]) -> String {
    strs.iter()
        .map(|s| {
            let needs_quotes = s.is_empty() || s.contains([';', '"', '\\', '\n', '\r', ' ', '\t']);
            if !needs_quotes {
                return s.clone();
            }
            let mut q = String::from("\"");
            for c in s.chars() {
                match c {
                    '"' => q.push_str("\\\""),
                    '\\' => q.push_str("\\\\"),
                    '\n' => q.push_str("\\n"),
                    '\r' => q.push_str("\\r"),
                    c => q.push(c),
                }
            }
            q.push('"');
            q
        })
        .collect::<Vec<_>>()
        .join(";")
}

/// A set of typed options plus any unknown keys, verbatim.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Config {
    /// Source-format values for full-configuration export, separate from template extras.
    pub native: std::sync::Arc<native::NativeConfig>,
    values: BTreeMap<String, ConfigValue>,
    /// Keys Mica has no definition for (other slicers' options, preset bookkeeping), raw text.
    extras: BTreeMap<String, String>,
}

impl Config {
    pub fn new() -> Self {
        Self::default()
    }

    /// Every known option at its Prusa-compatible default.
    pub fn defaults() -> Self {
        let mut c = Self::new();
        for def in option_defs().values().filter(|d| {
            // Optional Orca controls stay absent so Prusa configs are unchanged; neutral
            // additions (orca_options.tsv) are part of the defaults.
            !include_str!("orca_options_optional.tsv")
                .lines()
                .any(|l| l.split('\t').next() == Some(d.key))
        }) {
            let v = parse_value(def.ty, def.default)
                .unwrap_or_else(|e| panic!("default of {} does not parse: {e}", def.key));
            c.values.insert(def.key.to_string(), v);
        }
        c
    }

    /// Sets an option from PrusaSlicer text form. Unknown keys are kept verbatim.
    pub fn set_raw(&mut self, key: &str, raw: &str) -> Result<(), ParseError> {
        // Prusa's legacy/CLI aliases address both visible faces, not the internal solid shells.
        if matches!(key, "solid_fill_pattern" | "external_fill_pattern") {
            self.set_raw("top_fill_pattern", raw)?;
            return self.set_raw("bottom_fill_pattern", raw);
        }
        if key == "mica_gcode_identity" && !matches!(raw, "mica" | "orca_compatible") {
            return Err(ParseError {
                key: key.into(),
                raw: raw.into(),
                reason: "expected mica or orca_compatible".into(),
            });
        }
        match option_defs().get(key) {
            Some(def) => {
                let v = parse_value(def.ty, raw).map_err(|reason| ParseError {
                    key: key.into(),
                    raw: raw.into(),
                    reason,
                })?;
                self.values.insert(key.to_string(), v);
                // A PrusaSlicer value replaces the merged Orca setting it stands for.
                if let Some(pair) = crate::merged::by_prusa(key) {
                    self.values.remove(pair.orca);
                }
            }
            None => {
                self.extras.insert(key.to_string(), raw.to_string());
            }
        }
        Ok(())
    }

    pub fn set(&mut self, key: &str, value: ConfigValue) {
        self.values.insert(key.to_string(), value);
    }

    /// Drops the value of `key` (a defined option), returning it.
    pub fn remove(&mut self, key: &str) -> Option<ConfigValue> {
        self.values.remove(key)
    }

    pub fn get(&self, key: &str) -> Option<&ConfigValue> {
        self.values.get(key)
    }

    pub fn extra(&self, key: &str) -> Option<&str> {
        self.extras.get(key).map(String::as_str)
    }

    pub fn values(&self) -> impl Iterator<Item = (&str, &ConfigValue)> {
        self.values.iter().map(|(k, v)| (k.as_str(), v))
    }

    pub fn extras(&self) -> impl Iterator<Item = (&str, &str)> {
        self.extras.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }

    /// Overlays `other` on top of `self` (later wins), including unknown keys.
    /// A layer that sets the PrusaSlicer counterpart of a merged Orca option without the Orca
    /// key replaces that Orca value (`crate::merged`).
    pub fn apply(&mut self, other: &Config) {
        if !other.native.options.is_empty() {
            let native = std::sync::Arc::make_mut(&mut self.native);
            native.options.extend(other.native.options.clone());
            native.baseline.extend(other.native.baseline.clone());
            native
                .typed_baseline
                .extend(other.native.typed_baseline.clone());
        }
        for pair in crate::merged::PAIRS {
            if other.values.contains_key(pair.prusa) && !other.values.contains_key(pair.orca) {
                self.values.remove(pair.orca);
            }
        }
        for (k, v) in &other.values {
            self.values.insert(k.clone(), v.clone());
        }
        for (k, v) in &other.extras {
            self.extras.insert(k.clone(), v.clone());
        }
    }

    /// Text form of an option (known or unknown).
    pub fn raw(&self, key: &str) -> Option<String> {
        self.values
            .get(key)
            .map(serialize_value)
            .or_else(|| self.extras.get(key).cloned())
    }

    /// Scalar or first vector element of `key`; for per-extruder options use `at`.
    fn scalar(&self, key: &str, index: usize) -> Option<&Value> {
        match self.values.get(key)? {
            ConfigValue::Scalar(v) => Some(v),
            // PrusaSlicer falls back to the first element if a vector is shorter than the extruder count.
            ConfigValue::Vector(items) => items.get(index).or_else(|| items.first())?.as_ref(),
        }
    }

    /// Numeric value (float, int, or the number part of a percent).
    pub fn float_at(&self, key: &str, index: usize) -> Option<f64> {
        match self.scalar(key, index)? {
            Value::Float(f) | Value::Percent(f) => Some(*f),
            Value::FloatOrPercent { value, .. } => Some(*value),
            Value::Int(i) => Some(*i as f64),
            Value::Bool(b) => Some(f64::from(u8::from(*b))),
            _ => None,
        }
    }

    pub fn float(&self, key: &str) -> Option<f64> {
        self.float_at(key, 0)
    }

    pub fn int_at(&self, key: &str, index: usize) -> Option<i64> {
        match self.scalar(key, index)? {
            Value::Int(i) => Some(*i),
            Value::Float(f) => Some(f.round() as i64),
            Value::Bool(b) => Some(i64::from(*b)),
            _ => None,
        }
    }

    pub fn int(&self, key: &str) -> Option<i64> {
        self.int_at(key, 0)
    }

    pub fn bool_at(&self, key: &str, index: usize) -> Option<bool> {
        match self.scalar(key, index)? {
            Value::Bool(b) => Some(*b),
            Value::Int(i) => Some(*i != 0),
            _ => None,
        }
    }

    pub fn bool(&self, key: &str) -> Option<bool> {
        self.bool_at(key, 0)
    }

    pub fn string_at(&self, key: &str, index: usize) -> Option<&str> {
        match self.scalar(key, index)? {
            Value::String(s) | Value::Enum(s) => Some(s),
            _ => None,
        }
    }

    pub fn string(&self, key: &str) -> Option<&str> {
        self.string_at(key, 0)
    }

    /// Resolves a float-or-percent option: percentages are taken of `base`.
    pub fn abs_value(&self, key: &str, base: f64) -> Option<f64> {
        match self.scalar(key, 0)? {
            Value::FloatOrPercent {
                value,
                percent: true,
            }
            | Value::Percent(value) => Some(base * value / 100.0),
            Value::FloatOrPercent {
                value,
                percent: false,
            }
            | Value::Float(value) => Some(*value),
            Value::Int(i) => Some(*i as f64),
            _ => None,
        }
    }

    /// Number of elements of a vector option (1 for scalars, 0 if unset).
    pub fn vector_len(&self, key: &str) -> usize {
        match self.values.get(key) {
            Some(ConfigValue::Vector(items)) => items.len(),
            Some(ConfigValue::Scalar(_)) => 1,
            None => 0,
        }
    }

    /// Resizes a vector option to `n` elements like PrusaSlicer's `ConfigOptionVector::resize`:
    /// a non-empty vector repeats its first element, an empty one is filled with the option's
    /// default (its first element).
    pub fn resize_vector(&mut self, key: &str, n: usize) {
        if let Some(ConfigValue::Vector(items)) = self.values.get_mut(key) {
            if let Some(front) = items.first().cloned() {
                items.resize(n, front);
            } else {
                let pad = option_defs()
                    .get(key)
                    .and_then(|d| match parse_value(d.ty, d.default) {
                        Ok(ConfigValue::Vector(items)) => items.first().cloned(),
                        _ => None,
                    })
                    .flatten();
                items.resize(n, pad);
            }
        }
    }

    /// Resizes a vector option to `n` elements by repeating its last element (empty stays empty).
    pub fn resize_vector_repeat(&mut self, key: &str, n: usize) {
        if let Some(ConfigValue::Vector(items)) = self.values.get_mut(key)
            && let Some(last) = items.last().cloned()
        {
            items.resize(n, last);
        }
    }

    pub fn points(&self, key: &str) -> Option<Vec<(f64, f64)>> {
        match self.values.get(key)? {
            ConfigValue::Vector(items) => items
                .iter()
                .map(|i| match i {
                    Some(Value::Point(x, y)) => Some((*x, *y)),
                    _ => None,
                })
                .collect(),
            ConfigValue::Scalar(Value::Point(x, y)) => Some(vec![(*x, *y)]),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_loads_and_defaults_parse() {
        let defs = option_defs();
        assert!(defs.len() > 300);
        assert_eq!(defs["perimeters"].ty, OptType::Int);
        assert_eq!(defs["perimeters"].scope, Scope::Print);
        assert!(defs["idle_temperature"].nullable);
        let c = Config::defaults();
        assert_eq!(c.int("perimeters"), Some(3));
        assert_eq!(c.float("fill_density"), Some(20.0));
        assert_eq!(
            c.get("idle_temperature"),
            Some(&ConfigValue::Vector(vec![None]))
        );
    }

    #[test]
    fn resized_vectors_repeat_their_first_element() {
        // ConfigOptionVector::resize: the XL 5T's one `retract_layer_change = 1` becomes
        // 1,1,1,1,1 (PrusaSlicer 2.9.6), not 1,0,0,0,0 padded with the option default.
        let mut c = Config::new();
        c.set_raw("retract_layer_change", "1").unwrap();
        c.set_raw("retract_length", "0.8,0.7").unwrap();
        c.set_raw("wipe", "").unwrap();
        for key in ["retract_layer_change", "retract_length", "wipe"] {
            c.resize_vector(key, 5);
        }
        assert_eq!(c.raw("retract_layer_change").as_deref(), Some("1,1,1,1,1"));
        assert_eq!(
            c.raw("retract_length").as_deref(),
            Some("0.8,0.7,0.8,0.8,0.8")
        );
        assert_eq!(c.raw("wipe").as_deref(), Some("0,0,0,0,0"));
    }

    #[test]
    fn vectors_percents_points_strings() {
        let mut c = Config::new();
        c.set_raw("nozzle_diameter", "0.4,0.6").unwrap();
        assert_eq!(c.float_at("nozzle_diameter", 1), Some(0.6));
        assert_eq!(c.float_at("nozzle_diameter", 5), Some(0.4)); // falls back to the first
        c.set_raw("bed_shape", "0x0,250x0,250x220,0x220").unwrap();
        assert_eq!(c.points("bed_shape").unwrap()[2], (250.0, 220.0));
        c.set_raw("infill_overlap", "15%").unwrap();
        assert_eq!(c.abs_value("infill_overlap", 2.0), Some(0.3));
        c.set_raw("start_filament_gcode", "\"M572 S0.036 ; PA\\n\\nM142 S36\"")
            .unwrap();
        assert_eq!(
            c.string("start_filament_gcode"),
            Some("M572 S0.036 ; PA\n\nM142 S36")
        );
        c.set_raw("start_gcode", "M17\\nG28 ; home").unwrap();
        assert_eq!(c.string("start_gcode"), Some("M17\nG28 ; home"));
        c.set_raw("filament_type", "PLA;PETG").unwrap();
        assert_eq!(c.string_at("filament_type", 1), Some("PETG"));
    }

    #[test]
    fn round_trips_prusaslicer_text() {
        for (key, raw) in [
            ("bed_shape", "0x0,250x0,250x220,0x220"),
            ("fill_density", "15%"),
            ("nozzle_diameter", "0.4"),
            ("idle_temperature", "nil"),
            ("temperature", "230"),
            ("start_gcode", "M17\\nG28 ; home"),
            ("extruder_colour", "\"\""),
            ("filament_type", "PLA"),
            ("filament_cost", "27.99"),
            ("first_layer_height", "0.2"),
        ] {
            let mut c = Config::new();
            c.set_raw(key, raw).unwrap();
            assert_eq!(c.raw(key).unwrap(), raw, "{key}");
        }
    }

    #[test]
    fn unknown_keys_are_kept() {
        let mut c = Config::new();
        c.set_raw("wall_loops", "3").unwrap();
        assert_eq!(c.extra("wall_loops"), Some("3"));
        assert_eq!(c.raw("wall_loops").as_deref(), Some("3"));
    }

    #[test]
    fn bad_values_are_errors() {
        let mut c = Config::new();
        assert!(c.set_raw("perimeters", "lots").is_err());
        assert!(c.set_raw("bed_shape", "250,220").is_err());
    }
}
