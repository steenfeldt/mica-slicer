// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! CLI overrides must preserve imported Orca types, including non-Prusa preheat settings.
use mica_core::config::{Config, ConfigValue, OptType, Value, option_defs, parse_value};

pub(crate) fn apply(c: &mut Config, key: &str, raw: &str) -> Result<(), String> {
    if !option_defs().contains_key(key)
        && !matches!(
            key,
            "wipe_tower_x" | "wipe_tower_y" | "wipe_tower_rotation_angle"
        )
        && c.get(key).is_none()
        && c.extra(key).is_none()
        && !mica_profiles::orca::mapping::mapping_table().contains_key(key)
    {
        return Err(format!(
            "unknown profile setting `{key}`; check its spelling"
        ));
    }
    if !option_defs().contains_key(key)
        && let Some(existing) = c.get(key)
    {
        let (scalar, vector) = match existing {
            ConfigValue::Scalar(v) => (Some(v), false),
            ConfigValue::Vector(v) => (v.iter().flatten().next(), true),
        };
        let ty = match (scalar, vector) {
            (Some(Value::Float(_)), false) => OptType::Float,
            (Some(Value::Float(_)), true) => OptType::Floats,
            (Some(Value::Int(_)), false) => OptType::Int,
            (Some(Value::Int(_)), true) => OptType::Ints,
            (Some(Value::Bool(_)), false) => OptType::Bool,
            (Some(Value::Bool(_)), true) => OptType::Bools,
            (Some(Value::Percent(_)), false) => OptType::Percent,
            (Some(Value::Percent(_)), true) => OptType::Percents,
            (Some(Value::FloatOrPercent { .. }), false) => OptType::FloatOrPercent,
            (Some(Value::FloatOrPercent { .. }), true) => OptType::FloatsOrPercents,
            (Some(Value::Point(..)), false) => OptType::Point,
            (Some(Value::Point(..)), true) => OptType::Points,
            (Some(Value::Enum(_)), false) => OptType::Enum,
            (Some(Value::Enum(_)), true) => OptType::Enums,
            (_, false) => OptType::String,
            (_, true) => OptType::Strings,
        };
        c.set(
            key,
            parse_value(ty, raw).map_err(|e| format!("{key}: {e}"))?,
        );
        // Imported Orca options also retain raw text. Tower settings accept raw GUI edits,
        // so both representations must reflect a CLI edit (including per-plate X/Y vectors).
        c.set_raw(key, raw).map_err(|e| e.to_string())?;
    } else {
        c.set_raw(key, raw).map_err(|e| e.to_string())?;
    }
    if let Some(&(canonical, "identity")) = mica_profiles::orca::mapping::mapping_table().get(key)
        && canonical != key
    {
        c.set_raw(canonical, raw).map_err(|e| e.to_string())?;
    }
    // Orca's support type and style together select Mica's automatic mode and style.
    if matches!(key, "support_type" | "support_style") {
        let text = |k: &str| c.extra(k).map(str::to_string).or_else(|| c.raw(k));
        let kind = text("support_type").unwrap_or_default();
        let style = text("support_style").unwrap_or_else(|| "default".into());
        let Some((auto, style)) = mica_profiles::orca::mapping::support_mode(&kind, &style) else {
            return Err(format!("unknown support_type `{kind}`"));
        };
        c.set_raw("support_material_auto", if auto { "1" } else { "0" })
            .map_err(|e| e.to_string())?;
        c.set_raw("support_material_style", style)
            .map_err(|e| e.to_string())?;
    }
    if let Some(value) = c.get(key).cloned() {
        for (&source, &(canonical, conversion)) in mica_profiles::orca::mapping::mapping_table() {
            if canonical == key
                && source != key
                && conversion == "identity"
                && c.get(source).is_some()
            {
                c.set(source, value.clone());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_keys_are_reported() {
        assert!(
            apply(&mut Config::defaults(), "nonsense_profile_key", "1")
                .unwrap_err()
                .contains("unknown")
        );
    }

    #[test]
    fn tower_edits_replace_imported_raw_and_typed_values() {
        let mut c = Config::defaults();
        for (key, ty, before, after, expected) in [
            ("wipe_tower_x", OptType::Floats, "15", "165", 165.),
            ("wipe_tower_y", OptType::Floats, "220", "233.362", 233.362),
            ("prime_volume", OptType::Float, "30", "45", 45.),
        ] {
            c.set_raw(key, before).unwrap();
            c.set(key, parse_value(ty, before).unwrap());
            apply(&mut c, key, after).unwrap();
            assert_eq!(c.float(key), Some(expected));
            assert_eq!(mica_core::wipe_tower::number(&c, key, 0.), expected);
        }
    }
}
