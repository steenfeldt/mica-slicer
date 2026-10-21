// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: orca@8500fcdcca src/libslic3r/Config.hpp :: ConfigOption::serialize
// Source: snapmaker-orca@b1831e5dcb src/libslic3r/GCode.cpp :: serialize_gcode_config_option
// Changes: Retain resolved native option text with its canonical baseline; flow variants
// Changes: have already been selected by Bundle::full_config_with_project.
use super::{Dialect, RawConfig, schema};
use mica_core::config::{Config, serialize_value};

pub(super) fn remember(config: &mut Config, raw: &RawConfig, dialect: Dialect) {
    let mut native = mica_core::config::native::NativeConfig::default();
    for (key, value) in raw {
        let Some(def) = schema::schema(dialect).get(key) else {
            continue;
        };
        let ty = def["type"].as_str().unwrap();
        let normalized = if ty == "String" {
            Ok(value.clone())
        } else if key == "small_perimeter_speed" {
            // The mapper and Orca accept zero as automatic despite the UI minimum.
            let mut def = def.clone();
            def["min"] = serde_json::Value::from(0);
            schema::normalized(value, &def)
        } else {
            schema::normalized(value, def)
        };
        if let Ok(value) = normalized
            && let Ok(text) = schema::serialize_normalized(&value, ty)
        {
            // Parse/serialize numbers like ConfigOption::serialize (removing JSON's
            // insignificant trailing zeroes); string escaping remains upstream's.
            let text = schema::typed(&value, ty).map_or(text, |v| serialize_value(&v));
            native.options.insert(key.clone(), text);
        }
    }
    native.baseline = mica_core::config::native::current(config);
    native.typed_baseline = config
        .values()
        .map(|(k, v)| (k.into(), serialize_value(v)))
        .collect();
    config.native = std::sync::Arc::new(native);
}
