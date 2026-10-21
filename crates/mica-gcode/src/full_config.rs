// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: prusaslicer@b028299c77 src/libslic3r/PresetBundle.cpp :: PresetBundle::full_fff_config
// Source: prusaslicer@b028299c77 src/libslic3r/Config.hpp :: ConfigOptionFloatTempl::serialize, ConfigOptionPercent::serialize, ConfigOptionPoint::serialize
// Source: orca@8500fcdcca src/libslic3r/Config.hpp :: ConfigOptionFloat::serialize, ConfigOptionPercent::serialize, ConfigOptionPoint::serialize
// Source: prusaslicer@b028299c77 src/libslic3r/Config.cpp :: escape_strings_cstyle
// Source: orca@8500fcdcca src/libslic3r/Config.cpp :: escape_string_cstyle, escape_strings_cstyle
// Source: prusaslicer@b028299c77 src/libslic3r/GCode.cpp :: GCodeGenerator::encode_full_config, GCodeGenerator::append_full_config, GCodeGenerator::_do_export
// Source: snapmaker-orca@b1831e5dcb src/libslic3r/GCode.cpp :: GCode::append_full_config, serialize_gcode_config_option, GCode::_do_export
// Changes: Rust config serialization; retain resolved Orca text until canonical edits replace it.
// Changes: No BBL path. Flow variants are selected at profile composition; one plate per export.
// Changes: Mica-only and foreign-schema options are not upstream configuration keys. Never export host credentials, including foreign host keys in Prusa configs.
use mica_core::config::Config;
use mica_core::threemf::{Flavour, translate};
use std::collections::BTreeMap;

fn banned(key: &str) -> bool {
    matches!(
        key,
        "compatible_printers"
            | "compatible_prints"
            | "print_host"
            | "print_host_webui"
            | "printhost_apikey"
            | "printhost_cafile"
            | "printhost_user"
            | "printhost_password"
            | "printhost_port"
    )
}

fn orca_types() -> BTreeMap<&'static str, (&'static str, bool)> {
    include_str!("orca_gcode_options.tsv")
        .lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| {
            let mut f = l.split('\t');
            Some((f.next()?, (f.next()?, f.next()? == "1")))
        })
        .collect()
}

// Both upstreams leave an empty element unquoted in multi-string vectors.
fn upstream_strings(text: &str) -> String {
    let Ok(strings) = mica_core::config::unescape_strings(text) else {
        return text.into();
    };
    strings
        .iter()
        .map(|s| {
            let quote = strings.len() == 1 && s.is_empty()
                || s.contains([' ', '\t', '\\', '"', '\r', '\n']);
            if !quote {
                return s.clone();
            }
            let mut out = String::from("\"");
            for c in s.chars() {
                match c {
                    '\\' | '"' => {
                        out.push('\\');
                        out.push(c);
                    }
                    '\r' => out.push_str("\\r"),
                    '\n' => out.push_str("\\n"),
                    c => out.push(c),
                }
            }
            out.push('"');
            out
        })
        .collect::<Vec<_>>()
        .join(";")
}

// ConfigOption numeric serializers use an unmodified C++ stream: six significant
// digits, fixed notation for exponents -4..5, otherwise a signed/padded exponent.
fn number(value: f64) -> String {
    let scientific = format!("{value:.5e}");
    let (mantissa, exponent) = scientific
        .split_once('e')
        .expect("finite scientific number");
    let exponent: i32 = exponent.parse().expect("numeric exponent");
    if !(-4..6).contains(&exponent) {
        let mantissa = mantissa.trim_end_matches('0').trim_end_matches('.');
        return format!("{mantissa}e{exponent:+03}");
    }
    let decimals = (5 - exponent).max(0) as usize;
    let fixed = format!("{value:.decimals$}");
    if fixed.contains('.') {
        fixed.trim_end_matches('0').trim_end_matches('.').into()
    } else {
        fixed
    }
}

fn numeric_text(text: &str, ty: Option<&str>) -> String {
    let Some(ty) = ty else { return text.into() };
    if !ty.starts_with("Float") && !ty.starts_with("Percent") && !ty.starts_with("Point") {
        return text.into();
    }
    let token = |s: &str| {
        let (s, percent) = s.strip_suffix('%').map_or((s, false), |s| (s, true));
        match s.parse::<f64>() {
            Ok(v) if v.is_finite() => format!("{}{}", number(v), if percent { "%" } else { "" }),
            _ => s.into(),
        }
    };
    let mut out = String::new();
    let mut start = 0;
    for (i, c) in text
        .char_indices()
        .filter(|(_, c)| matches!(c, ',' | ';' | '#' | 'x'))
    {
        out.push_str(&token(&text[start..i]));
        out.push(if ty == "Point" && c == 'x' { ',' } else { c });
        start = i + c.len_utf8();
    }
    out.push_str(&token(&text[start..]));
    out
}

// Preset composition may expand a one-element vector per tool. That does not
// edit its native setting; preserve the original shape until an element changes.
fn same_value(key: &str, before: &str, after: &str) -> bool {
    if before == after {
        return true;
    }
    let Some(def) = mica_core::config::option_defs().get(key) else {
        return false;
    };
    use mica_core::config::{ConfigValue, parse_value};
    match (parse_value(def.ty, before), parse_value(def.ty, after)) {
        (Ok(ConfigValue::Vector(a)), Ok(ConfigValue::Vector(b)))
            if !a.is_empty() && !b.is_empty() =>
        {
            (0..a.len().max(b.len()))
                .all(|i| a.get(i).or_else(|| a.first()) == b.get(i).or_else(|| b.first()))
        }
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// Sorted upstream option pairs, shared by text output and the bgcode slicer metadata.
pub fn entries(config: &Config) -> Vec<(String, String)> {
    let orca = mica_core::rules::Rules::of(config).is_orca();
    let values = mica_core::config::native::current(config);
    let values = if orca {
        let types = orca_types();
        let mut result = config.native.options.clone();
        // Translate changes only: untouched native percentages, enum vocabulary and
        // plate-specific temperatures must survive the canonical mapping losslessly.
        let edits: BTreeMap<String, String> = values
            .iter()
            .filter(|(k, v)| {
                config
                    .native
                    .baseline
                    .get(*k)
                    .is_none_or(|before| !same_value(k, before, v))
            })
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let changed = translate(&edits, Flavour::Prusa, Flavour::Orca);
        // Scarf percentages and other merged values read neighbouring settings.
        // Supply their dependencies without letting stale native aliases mask an edit.
        let mut context = edits.clone();
        for key in [
            "layer_height",
            "scarf_seam_length",
            "seam_slope_scarf_length",
            "support_material_auto",
            "support_material_style",
            "curr_bed_type",
        ] {
            if let Some(value) = values.get(key) {
                context.entry(key.into()).or_insert_with(|| value.clone());
            }
        }
        let translated = translate(&context, Flavour::Prusa, Flavour::Orca);
        for key in changed.keys() {
            if types.contains_key(key.as_str())
                && let Some(value) = translated.get(key)
            {
                result.insert(key.clone(), value.clone());
            }
        }
        result.retain(|k, v| {
            // ConfigOptionVectorBase::is_nil: skip a nullable vector only when all
            // its elements are nil (including an empty nullable vector).
            types.get(k.as_str()).is_some_and(|(_, nullable)| {
                !nullable || (!v.is_empty() && !v.split(',').all(|v| v == "nil"))
            })
        });
        if let Some(colour) = result.get("filament_colour").cloned() {
            result.insert("extruder_colour".into(), colour);
        }
        for key in [
            "filament_flow_support",
            "process_flow_support",
            "printer_flow_support",
        ] {
            if let Some(v) = result.get_mut(key)
                && let Ok(strings) = mica_core::config::unescape_strings(v)
            {
                *v = strings.join(",");
            }
        }
        result
    } else {
        // FullPrintConfig has no preset-level conditions/parent; the old table
        // also contains infill_only_where_needed, commented out in PrintConfigDef.
        let keys: std::collections::BTreeSet<_> = mica_core::config::prusa_option_keys().collect();
        values
            .into_iter()
            .filter(|(k, _)| {
                keys.contains(k.as_str())
                    && !matches!(
                        k.as_str(),
                        "compatible_printers_condition"
                            | "compatible_prints_condition"
                            | "inherits"
                            | "infill_only_where_needed"
                    )
                    || matches!(
                        k.as_str(),
                        "print_settings_id"
                            | "printer_settings_id"
                            | "filament_settings_id"
                            | "physical_printer_settings_id"
                            | "compatible_printers_condition_cummulative"
                            | "compatible_prints_condition_cummulative"
                            | "inherits_cummulative"
                    )
            })
            .map(|(k, v)| {
                let v = mica_core::merged::prusa_raw(config, &k).unwrap_or(v);
                (k, v)
            })
            .collect()
    };
    let types = orca_types();
    values
        .into_iter()
        .filter(|(k, _)| !banned(k) && !k.starts_with("mica_"))
        .map(|(k, v)| {
            let ty = if orca {
                types.get(k.as_str()).map(|t| t.0)
            } else if matches!(
                k.as_str(),
                "filament_settings_id"
                    | "inherits_cummulative"
                    | "compatible_printers_condition_cummulative"
                    | "compatible_prints_condition_cummulative"
            ) {
                Some("Strings")
            } else {
                mica_core::config::option_defs()
                    .get(k.as_str())
                    .map(|d| match d.ty {
                        mica_core::config::OptType::Strings => "Strings",
                        mica_core::config::OptType::Float
                        | mica_core::config::OptType::Floats
                        | mica_core::config::OptType::Percent
                        | mica_core::config::OptType::Percents
                        | mica_core::config::OptType::FloatOrPercent
                        | mica_core::config::OptType::FloatsOrPercents => "Float",
                        mica_core::config::OptType::Point => "Point",
                        mica_core::config::OptType::Points => "Points",
                        _ => "",
                    })
            };
            let v = match ty {
                Some("String") if orca => v.replace('"', "\\\""),
                Some("Strings") => upstream_strings(&v),
                _ => v,
            };
            let v = if !orca
                && (k == "filament_settings_id"
                    || mica_core::config::option_defs()
                        .get(k.as_str())
                        .is_some_and(|d| {
                            d.scope == mica_core::config::Scope::Filament && d.ty.is_vector()
                        }))
            {
                let ty = if k == "filament_settings_id" {
                    mica_core::config::OptType::Strings
                } else {
                    mica_core::config::option_defs()[k.as_str()].ty
                };
                match mica_core::config::parse_value(ty, &v) {
                    Ok(mica_core::config::ConfigValue::Vector(mut items)) if !items.is_empty() => {
                        items.resize(
                            config.vector_len("nozzle_diameter").max(1),
                            items[0].clone(),
                        );
                        let text = mica_core::config::serialize_value(
                            &mica_core::config::ConfigValue::Vector(items),
                        );
                        if ty == mica_core::config::OptType::Strings {
                            upstream_strings(&text)
                        } else {
                            text
                        }
                    }
                    _ => v,
                }
            } else {
                v
            };
            let v = numeric_text(&v, ty);
            (k, v)
        })
        .collect()
}

/// Append the upstream footer block after statistics. Orca's four compatibility fields
/// are deliberately outside the sorted full config, matching its non-BBL call site.
pub fn text(config: &Config, used_tools: &[usize]) -> String {
    let orca = mica_core::rules::Rules::of(config).is_orca();
    let mut out = if orca {
        "\n; CONFIG_BLOCK_START\n"
    } else {
        "\n; prusaslicer_config = begin\n"
    }
    .to_string();
    for (key, value) in entries(config) {
        if orca && matches!(key.as_str(), "wipe_tower_x" | "wipe_tower_y") {
            // Upstream writes the selected plate with 3 decimals, then also the
            // serialized vector (the second `if` is independent, not `else if`).
            let first = value
                .split(',')
                .next()
                .and_then(|v| v.parse::<f64>().ok())
                .unwrap_or(0.);
            let first = mica_core::wipe_tower::number(config, &key, first);
            out.push_str(&format!("; {key} = {first:.3}\n"));
        }
        let value = if !orca && key == "binary_gcode" {
            "0"
        } else {
            &value
        };
        out.push_str(&format!("; {key} = {value}\n"));
    }
    if orca {
        let bed = used_tools
            .iter()
            .filter_map(|&tool| config.int_at("first_layer_bed_temperature", tool))
            .max()
            .unwrap_or(0);
        out.push_str(&format!("; first_layer_bed_temperature = {bed}\n; bed_shape = {}\n; first_layer_temperature = {}\n; first_layer_height = {:.3}\n; CONFIG_BLOCK_END\n\n",
            config.raw("bed_shape").unwrap_or_default(), config.int("first_layer_temperature").unwrap_or(0),
            config.float("first_layer_height").unwrap_or(0.)));
    } else {
        out.push_str("; prusaslicer_config = end\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn upstream_numeric_precision_and_point_spelling() {
        assert_eq!(
            numeric_text("0.123456789,1234567,0.00001,-0", Some("Floats")),
            "0.123457,1.23457e+06,1e-05,-0"
        );
        assert_eq!(
            numeric_text("12.3456789%", Some("FloatOrPercent")),
            "12.3457%"
        );
        assert_eq!(
            numeric_text("0.123456789x0.5", Some("Point")),
            "0.123457,0.5"
        );
        assert_eq!(numeric_text("0x0,250x220", Some("Points")), "0x0,250x220");
        assert_eq!(numeric_text("G1 X1234567", Some("String")), "G1 X1234567");
        let mut config = Config::new();
        config
            .set_raw("inherits_cummulative", "\"\";User;\"\"")
            .unwrap();
        assert_eq!(
            entries(&config),
            [("inherits_cummulative".into(), ";User;".into())]
        );
        mica_core::rules::Rules::OrcaSlicer.set(&mut config);
        config.set_raw("wipe_tower_x", "1.2354999").unwrap();
        assert!(text(&config, &[0]).contains("; wipe_tower_x = 1.235\n; wipe_tower_x = 1.2355\n"));
    }

    #[test]
    fn secrets_never_enter_text_or_metadata_and_prusa_is_sorted() {
        let mut c = Config::defaults();
        for k in [
            "print_host",
            "printhost_apikey",
            "printhost_cafile",
            "printhost_user",
            "printhost_password",
            "printhost_port",
            "print_host_webui",
        ] {
            c.set_raw(k, "SECRET").unwrap();
        }
        for rules in [
            mica_core::rules::Rules::PrusaSlicer,
            mica_core::rules::Rules::OrcaSlicer,
        ] {
            rules.set(&mut c);
            let e = entries(&c);
            assert!(e.windows(2).all(|p| p[0].0 < p[1].0));
            assert!(!text(&c, &[0]).contains("SECRET"));
            assert!(e.iter().all(|(k, _)| !k.starts_with("mica_")));
        }
    }
}
