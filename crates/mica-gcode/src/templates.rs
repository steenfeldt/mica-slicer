// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/PlaceholderParser.cpp :: MyContext::scalar_variable_reference, MyContext::vector_variable_reference
// Source: prusaslicer@b028299c77 src/libslic3r/GCode.cpp :: GCodeGenerator::_do_export, GCodeGenerator::placeholder_parser_process
// Changes: The configuration is converted to template variables up front (percentages resolved through
// Changes: ratio_over, widths through Flow rules) instead of being looked up lazily during parsing.

//! Running profile G-code templates (start, end, layer change, filament) with PrusaSlicer's variables.

use mica_core::config::{Config, ConfigValue, Value as CfgValue, serialize_value};
use mica_core::resolve::float_or_percent_value;
use mica_template::{Context, Value, ValueType, Var};

fn scalar(v: &CfgValue) -> Value {
    match v {
        CfgValue::Float(f) | CfgValue::Percent(f) => Value::Float(*f),
        CfgValue::FloatOrPercent { value, .. } => Value::Float(*value),
        CfgValue::Int(i) => Value::Int(*i),
        CfgValue::Bool(b) => Value::Bool(*b),
        CfgValue::String(s) | CfgValue::Enum(s) => Value::Str(s.clone()),
        CfgValue::Point(..) => Value::Str(serialize_value(&ConfigValue::Scalar(v.clone()))),
    }
}

fn value_type(v: &Value) -> Option<ValueType> {
    v.value_type()
}

/// Template variables for every typed option, including imported Orca options.
/// Unknown extras remain strings; raw extras never shadow a validated typed value.
pub fn config_context(config: &Config) -> Context {
    let mut ctx = Context::new();
    for (key, value) in config.values() {
        let var = match value {
            ConfigValue::Scalar(CfgValue::FloatOrPercent { .. }) => {
                match float_or_percent_value(config, key) {
                    Ok(v) => Var::float(v),
                    Err(_) => Var::Scalar(scalar(match value {
                        ConfigValue::Scalar(s) => s,
                        ConfigValue::Vector(_) => unreachable!(),
                    })),
                }
            }
            ConfigValue::Scalar(s) => Var::Scalar(scalar(s)),
            ConfigValue::Vector(items) => {
                let values: Vec<Option<Value>> =
                    items.iter().map(|i| i.as_ref().map(scalar)).collect();
                let ty = values
                    .iter()
                    .flatten()
                    .find_map(value_type)
                    .or_else(|| {
                        mica_core::config::option_defs()
                            .get(key)
                            .map(|d| match d.ty {
                                mica_core::config::OptType::Ints => ValueType::Int,
                                mica_core::config::OptType::Bools => ValueType::Bool,
                                mica_core::config::OptType::Strings
                                | mica_core::config::OptType::Enums
                                | mica_core::config::OptType::Points => ValueType::Str,
                                _ => ValueType::Float,
                            })
                    })
                    .unwrap_or(ValueType::Float);
                Var::nullable(ty, values)
            }
        };
        ctx.set(key, var);
    }
    for (key, raw) in config.extras() {
        if config.get(key).is_none() {
            ctx.set(key, Var::string(raw));
        }
    }
    ctx
}

/// Runs `template` and returns its output. Errors name the template so the user can find it.
pub fn run(name: &str, template: &str, ctx: &mut Context) -> Result<String, String> {
    mica_template::process(template, ctx).map_err(|e| format!("{name}: {e}"))
}

/// Per-call variables PrusaSlicer adds around custom G-code at a layer.
pub fn set_layer_vars(ctx: &mut Context, layer_num: i64, layer_z: f64, max_layer_z: f64) {
    ctx.set("layer_num", Var::int(layer_num));
    ctx.set("layer_z", Var::float(layer_z));
    ctx.set("max_layer_z", Var::float(max_layer_z));
}

/// Whether custom G-code sets a temperature itself (`M<set>` or `M<set_and_wait>` at a line start),
/// and the temperature it sets if that can be read (`custom_gcode_sets_temperature`).
pub fn sets_temperature(gcode: &str, m_set: u32, m_wait: u32) -> (bool, Option<i64>) {
    let mut found = false;
    let mut temp = None;
    for line in gcode.lines() {
        let line = line.trim_start();
        let Some(rest) = line.strip_prefix('M') else {
            continue;
        };
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        let Ok(code) = digits.parse::<u32>() else {
            continue;
        };
        if code != m_set && code != m_wait {
            continue;
        }
        found = true;
        let args = rest[digits.len()..].split(';').next().unwrap_or("");
        for word in args.split_whitespace() {
            if let Some(v) = word.strip_prefix('S').or_else(|| word.strip_prefix('R'))
                && let Ok(t) = v.parse::<f64>()
            {
                temp = Some(t.round() as i64);
            }
        }
    }
    (found, temp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_values_become_variables() {
        let mut c = Config::defaults();
        c.set_raw("nozzle_diameter", "0.4,0.6").unwrap();
        c.set_raw("idle_temperature", "170,nil").unwrap();
        c.set_raw("wall_loops", "3").unwrap(); // unknown (Orca) key
        let mut ctx = config_context(&c);
        assert_eq!(run("t", "{nozzle_diameter[1]}", &mut ctx).unwrap(), "0.6");
        assert_eq!(
            run("t", "{is_nil(idle_temperature[1])}", &mut ctx).unwrap(),
            "true"
        );
        assert_eq!(run("t", "[wall_loops]", &mut ctx).unwrap(), "3");
        assert_eq!(run("t", "{fill_density}", &mut ctx).unwrap(), "20");
    }

    #[test]
    fn detects_temperature_commands() {
        assert_eq!(
            sets_temperature("G28\nM104 S215 ; heat\n", 104, 109),
            (true, Some(215))
        );
        assert_eq!(
            sets_temperature("M109 R170 ; wait\n", 104, 109),
            (true, Some(170))
        );
        assert_eq!(
            sets_temperature("G28\n;M104 S200\n", 104, 109),
            (false, None)
        );
    }
}
