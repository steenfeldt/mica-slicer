// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: orca@8500fcdcca src/libslic3r/Config.cpp :: ConfigBase::load_from_json
// Source: orca@8500fcdcca src/libslic3r/PrintConfig.cpp :: PrintConfigDef
// Source: snapmaker-orca@b1831e5dcb src/libslic3r/PrintConfig.cpp :: PrintConfigDef
// Changes: Checked JSON data tables, strict value diagnostics, and lossless unknown options.
// Source: orca@8500fcdcca src/libslic3r/Config.hpp :: ConfigOptionIntsTempl::deserialize, ConfigOptionInt::deserialize
// Changes: Integers keep the leading digits of a fractional value ("59.8" is 59), as istream does.
// Source: orca@8500fcdcca src/libslic3r/Config.hpp :: ConfigOptionFloat::deserialize, ConfigOptionFloatOrPercent::deserialize, ConfigOptionPoint::deserialize
// Changes: A float reads its leading number ("50%" for a plain float is 50, "2mm" 2), as istream does;
// Changes: a single point may be written "x,y" or "xXy".

use std::collections::BTreeMap;
use std::sync::OnceLock;

use mica_core::config::{
    ConfigValue, OptType, escape_string, escape_strings, parse_value, unescape_string,
    unescape_strings,
};
use serde_json::Value;

use super::{Dialect, RawConfig};

pub(super) fn schema(dialect: Dialect) -> &'static BTreeMap<String, Value> {
    static ORCA: OnceLock<BTreeMap<String, Value>> = OnceLock::new();
    static SNAP: OnceLock<BTreeMap<String, Value>> = OnceLock::new();
    let (cell, text) = match dialect {
        Dialect::Orca242 => (&ORCA, include_str!("orca_schema.json")),
        Dialect::Snapmaker240 => (&SNAP, include_str!("snapmaker-orca_schema.json")),
    };
    cell.get_or_init(|| serde_json::from_str(text).expect("checked-in Orca schema"))
}

/// Native scope for Orca options which Mica retains outside its canonical schema.
pub fn option_scope(key: &str, dialect: Dialect) -> Option<mica_core::config::Scope> {
    use mica_core::config::Scope;
    let scopes = schema(dialect).get(key)?.get("scope")?.as_array()?;
    Some(if scopes.iter().any(|s| s == "filament") {
        Scope::Filament
    } else if scopes.iter().any(|s| s == "machine") {
        Scope::Printer
    } else if scopes.iter().any(|s| s == "process") {
        Scope::Print
    } else {
        Scope::Other
    })
}

/// The Orca type name of an option (`Ints`, `Floats`, `String`, …).
pub fn option_type(key: &str, dialect: Dialect) -> Option<&'static str> {
    schema(dialect).get(key)?.get("type")?.as_str()
}

/// Whether the importer accepts `value` for the Orca option `key` (unknown options are
/// accepted: they survive as extras). The same check as reading a project.
pub fn accepts(key: &str, value: &str, dialect: Dialect) -> Result<(), String> {
    match schema(dialect).get(key) {
        Some(def) => normalized(&Value::String(value.into()), def).map(|_| ()),
        None => Ok(()),
    }
}

pub(super) fn defaults(dialect: Dialect, scope: Option<&str>) -> RawConfig {
    schema(dialect)
        .iter()
        .filter(|(_, d)| match scope {
            Some(scope) => d["scope"].as_array().unwrap().iter().any(|s| s == scope),
            None => d["full"] == true,
        })
        .map(|(k, d)| (k.clone(), d["default"].clone()))
        .collect()
}

/// Orca's JSON strings are serialized option text. Arrays of strings use semicolons and C-style
/// quoting for string vectors, commas for numeric/point vectors, and `#` for point groups.
pub fn option_text(value: &Value, ty: &str) -> Result<String, String> {
    match value {
        Value::String(s) => Ok(s.clone()),
        Value::Array(a) => {
            if a.iter().all(Value::is_string) {
                let strings: Vec<_> = a.iter().map(|s| s.as_str().unwrap().to_string()).collect();
                Ok(if ty == "Strings" {
                    escape_strings(&strings)
                } else {
                    strings.join(if ty == "PointsGroups" { "#" } else { "," })
                })
            } else if a.iter().all(Value::is_array) {
                a.iter()
                    .map(|v| option_text(v, ty))
                    .collect::<Result<Vec<_>, _>>()
                    .map(|v| v.join("#"))
            } else {
                Err("expected a homogeneous array of strings (or point groups)".into())
            }
        }
        _ => Err("expected a string or an array of strings".into()),
    }
}

pub(super) fn normalized(value: &Value, def: &Value) -> Result<Value, String> {
    let ty = def["type"].as_str().unwrap();
    let text = option_text(value, ty)?;
    if ty == "String" {
        return unescape_string(&text).map(Value::String);
    }
    if ty == "Strings" {
        return unescape_strings(&text)
            .map(|s| Value::Array(s.into_iter().map(Value::String).collect()));
    }
    let vector = ty.ends_with('s') || ty == "PointsGroups";
    let tokens: Vec<_> = if vector && text.is_empty() {
        vec![]
    } else if vector {
        text.split(if ty == "PointsGroups" { '#' } else { ',' })
            .collect()
    } else {
        vec![text.as_str()]
    };
    let values = tokens
        .into_iter()
        .map(|s| {
            let s = s.trim();
            if s == "nil" && def["nullable"] == true {
                return Ok(Value::String(s.into()));
            }
            if ty.starts_with("Enum") {
                if !def["enum"]
                    .as_array()
                    .is_some_and(|a| a.iter().any(|v| v == s))
                {
                    return Err(format!("unknown {ty} value {s:?}"));
                }
            } else if ty.starts_with("Bool") {
                return match s {
                    "1" | "true" => Ok(Value::String("1".into())),
                    "0" | "false" => Ok(Value::String("0".into())),
                    _ => Err(format!("invalid boolean {s:?}")),
                };
            } else if ty.starts_with("Point") {
                for point in s.split(';') {
                    // ConfigOptionPoint reads "x,y" and "x x y" (BBL's `best_object_pos`
                    // is "0.5x0.5"); point lists are "AxB".
                    let split = if ty == "Point" {
                        point.split_once(',').or_else(|| point.split_once('x'))
                    } else {
                        point.split_once('x')
                    };
                    let (x, y) = split.ok_or_else(|| format!("invalid point {point:?}"))?;
                    finite_number(x)?;
                    finite_number(y)?;
                }
            } else {
                let percent = s.ends_with('%');
                let floaty = ty.starts_with("Float") || ty.starts_with("Percent");
                let whole = finite_number(s.trim_end_matches('%')).is_ok()
                    && (!percent || ty.contains("Percent"));
                if floaty && !whole {
                    // Orca reads floats with `istream >> double` (ConfigOptionFloat /
                    // FloatOrPercent ::deserialize), which stops at the first character that
                    // is not part of a number: vendor profiles ship "50%" for an absolute
                    // speed, "2mm", "2,5" or "0,0", and Orca slices with 50, 2, 2 and 0.
                    let prefix =
                        float_prefix(s).ok_or_else(|| format!("invalid finite number {s:?}"))?;
                    let keep_percent = s.contains('%') && ty.contains("Percent");
                    let n = finite_number(&prefix)?;
                    if !keep_percent
                        && (def["min"].as_f64().is_some_and(|v| n < v)
                            || def["max"].as_f64().is_some_and(|v| n > v))
                    {
                        return Err(format!("value {s} outside option bounds"));
                    }
                    return Ok(Value::String(if keep_percent {
                        format!("{prefix}%")
                    } else {
                        prefix
                    }));
                }
                if percent && !ty.contains("Percent") {
                    return Err(format!("percent is not allowed for {ty}"));
                }
                let mut n = finite_number(s.trim_end_matches('%'))?;
                if ty.starts_with("Int") {
                    // Orca reads integers with `istream >> int` (ConfigOptionInt(s)::deserialize),
                    // which stops at the first non-digit: "59.8" is 59 and "1e3" is 1. Profiles
                    // ship such values (Polymaker's `temperature_vitrification`).
                    let int =
                        integer_prefix(s).ok_or_else(|| format!("expected integer, got {s:?}"))?;
                    let truncated = n.fract() != 0.0 || n != int as f64;
                    n = int as f64;
                    if def["min"].as_f64().is_some_and(|v| n < v)
                        || def["max"].as_f64().is_some_and(|v| n > v)
                    {
                        return Err(format!("value {s} outside option bounds"));
                    }
                    return Ok(Value::String(if truncated {
                        int.to_string()
                    } else {
                        s.to_string()
                    }));
                }
                // Bounds on FloatOrPercent apply to absolute values in the upstream UI, not percentages.
                if !percent
                    && (def["min"].as_f64().is_some_and(|v| n < v)
                        || def["max"].as_f64().is_some_and(|v| n > v))
                {
                    return Err(format!("value {s} outside option bounds"));
                }
            }
            Ok(Value::String(s.to_string()))
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(if vector {
        Value::Array(values)
    } else {
        values.into_iter().next().unwrap()
    })
}

/// The number `istream >> double` reads from the start of `s`: an optionally signed decimal
/// with an optional exponent (`"2mm"` is 2, `"0,0"` is 0).
fn float_prefix(s: &str) -> Option<String> {
    let b = s.trim().as_bytes();
    let mut i = 0;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        i += 1;
    }
    let int = b[i..].iter().take_while(|c| c.is_ascii_digit()).count();
    i += int;
    let mut frac = 0;
    if i < b.len() && b[i] == b'.' {
        frac = b[i + 1..].iter().take_while(|c| c.is_ascii_digit()).count();
        i += 1 + frac;
    }
    if int + frac == 0 {
        return None;
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        let mut j = i + 1;
        if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        let exp = b[j..].iter().take_while(|c| c.is_ascii_digit()).count();
        if exp > 0 {
            i = j + exp;
        }
    }
    Some(String::from_utf8_lossy(&b[..i]).into_owned())
}

/// The value `istream >> int` reads from a finite number: its leading optionally signed digits.
fn integer_prefix(s: &str) -> Option<i64> {
    let s = s.trim();
    let digits = s.strip_prefix(['+', '-']).unwrap_or(s);
    let len = digits.bytes().take_while(u8::is_ascii_digit).count();
    if len == 0 {
        return None;
    }
    let sign = &s[..s.len() - digits.len()];
    format!("{sign}{}", &digits[..len]).parse().ok()
}

pub(super) fn finite_number(s: &str) -> Result<f64, String> {
    s.trim()
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite())
        .ok_or_else(|| format!("invalid finite number {s:?}"))
}

pub(super) fn serialize_normalized(value: &Value, ty: &str) -> Result<String, String> {
    if ty == "String" {
        return Ok(escape_string(value.as_str().ok_or("expected string")?));
    }
    option_text(value, ty)
}

/// Sets an option from its text form the way Mica's own settings do (`Config::set_raw`) and,
/// for an Orca-only option of this dialect (`pressure_advance`, `enable_pressure_advance`, …),
/// also as the typed value the templates and the exporter read, which `set_raw` alone would
/// keep as untyped text.
pub fn set_option_text(
    config: &mut mica_core::config::Config,
    key: &str,
    text: &str,
    dialect: Dialect,
) -> Result<(), String> {
    config.set_raw(key, text).map_err(|e| e.to_string())?;
    if mica_core::config::option_defs().contains_key(key) {
        return Ok(());
    }
    if let Some(ty) = schema(dialect).get(key).and_then(|d| d["type"].as_str()) {
        let typed = parse_value(opt_type(ty)?, text).map_err(|e| format!("{key}: {e}"))?;
        config.set(key, typed);
    }
    Ok(())
}

/// Preserve the source type as well as its raw spelling for options used by Orca templates.
pub(super) fn typed(value: &Value, ty: &str) -> Result<ConfigValue, String> {
    parse_value(opt_type(ty)?, &serialize_normalized(value, ty)?)
}

fn opt_type(ty: &str) -> Result<OptType, String> {
    let opt = match ty {
        "Float" => OptType::Float,
        "Floats" => OptType::Floats,
        "Int" => OptType::Int,
        "Ints" => OptType::Ints,
        "Bool" => OptType::Bool,
        "Bools" => OptType::Bools,
        "Percent" => OptType::Percent,
        "Percents" => OptType::Percents,
        "FloatOrPercent" => OptType::FloatOrPercent,
        "FloatsOrPercents" => OptType::FloatsOrPercents,
        "String" => OptType::String,
        "Strings" => OptType::Strings,
        "Enum" => OptType::Enum,
        "Enums" => OptType::Enums,
        "Point" => OptType::Point,
        "Points" => OptType::Points,
        _ => return Err(format!("unsupported template option type {ty}")),
    };
    Ok(opt)
}
