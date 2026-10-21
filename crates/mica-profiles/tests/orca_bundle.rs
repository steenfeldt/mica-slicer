// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

use mica_profiles::orca::mapping::Disposition;
use mica_profiles::orca::{Bundle, Dialect, RawConfig, map_config};
const PRINTER: &str = "Snapmaker U1 (0.4 nozzle)";
fn upstream(dialect: Dialect) -> Option<Bundle> {
    let (root, available) = match dialect {
        Dialect::Orca242 => (
            mica_testkit::orca_profiles(),
            mica_testkit::require_orca_profiles(),
        ),
        Dialect::Snapmaker240 => (
            mica_testkit::snapmaker_orca_profiles(),
            mica_testkit::require_snapmaker_orca_profiles(),
        ),
    };
    if !available {
        return None;
    }
    let mut b = Bundle::new(dialect);
    if dialect == Dialect::Orca242 {
        b.add_vendor(root.join("OrcaFilamentLibrary.json")).unwrap();
    }
    b.add_vendor(root.join("Snapmaker.json")).unwrap();
    Some(b)
}
fn composed(dialect: Dialect) -> Option<RawConfig> {
    let b = upstream(dialect)?;
    let (process, filament) = match dialect {
        Dialect::Orca242 => (
            "0.20 Standard @Snapmaker U1 (0.4 nozzle)",
            "Generic PLA @System",
        ),
        Dialect::Snapmaker240 => ("0.20mm Standard @Snapmaker U1 (0.4 nozzle)", "Generic PLA"),
    };
    Some(b.full_config(PRINTER, process, &[filament; 4]).unwrap())
}
#[test]
fn snapmaker_u1_matches_project_with_every_difference_pinned() {
    let expected: RawConfig =
        serde_json::from_str(include_str!("fixtures/u1/project_settings.config")).unwrap();
    let differences: std::collections::BTreeMap<String, serde_json::Value> =
        serde_json::from_str(include_str!("fixtures/u1/snapmaker240_differences.json")).unwrap();
    let Some(got) = composed(Dialect::Snapmaker240) else {
        return;
    };
    let keys: std::collections::BTreeSet<_> = expected.keys().chain(got.keys()).collect();
    let mut matched = 0;
    let mut accounted = std::collections::BTreeSet::new();
    for key in keys {
        if expected.get(key) == got.get(key) {
            assert!(
                !differences.contains_key(key),
                "stale difference exemption: {key}"
            );
            matched += 1;
        } else {
            let d = differences.get(key).unwrap_or_else(|| {
                panic!(
                    "unexpected difference {key}: project {:?}, composed {:?}",
                    expected.get(key),
                    got.get(key)
                )
            });
            let reason = d["reason"].as_str().filter(|s| !s.is_empty()).unwrap();
            let actual = got.get(key).cloned().unwrap_or(serde_json::Value::Null);
            assert_eq!(
                actual, d["resolved"],
                "changed known difference {key}: {reason}"
            );
            eprintln!("{key}: {reason}");
            accounted.insert(key.clone());
        }
    }
    assert_eq!(
        accounted.len(),
        differences.len(),
        "stale/missing difference exemption"
    );
    assert_eq!(matched, 570);
    eprintln!(
        "Snapmaker: {matched}/584 project keys match exactly; 14 project differences and 23 additional retained keys, all individually pinned."
    );
}

#[test]
fn report_orca242_versus_snapmaker240() {
    let (Some(orca), Some(snap)) = (composed(Dialect::Orca242), composed(Dialect::Snapmaker240))
    else {
        return;
    };
    let keys: std::collections::BTreeSet<_> = orca.keys().chain(snap.keys()).collect();
    let mut changed = 0;
    for key in keys {
        if orca.get(key) != snap.get(key) {
            changed += 1;
            let summary = |v: Option<&serde_json::Value>| {
                v.map_or_else(
                    || "<absent>".to_string(),
                    |v| {
                        let s = v.to_string();
                        if s.chars().count() > 180 {
                            format!(
                                "{}… ({} bytes)",
                                s.chars().take(140).collect::<String>(),
                                s.len()
                            )
                        } else {
                            s
                        }
                    },
                )
            };
            eprintln!(
                "{key}: Snapmaker {} -> Orca {}",
                summary(snap.get(key)),
                summary(orca.get(key))
            );
        }
    }
    eprintln!(
        "Orca versus Snapmaker: {changed} differing keys; {} versus {} total keys (report only).",
        orca.len(),
        snap.len()
    );
}

fn check_engine_values(
    raw: &RawConfig,
    dialect: Dialect,
    pattern: &str,
    retraction: f64,
    retract_speed: f64,
) {
    let r = map_config(raw, dialect);
    let c = &r.config;
    assert_eq!(
        r.report.count(Disposition::Rejected),
        0,
        "{:?}",
        r.report
            .entries
            .iter()
            .filter(|e| e.disposition == Disposition::Rejected)
            .collect::<Vec<_>>()
    );
    assert_eq!(c.float("layer_height"), Some(0.2));
    assert_eq!(c.float("first_layer_height"), Some(0.25));
    assert_eq!(c.int("perimeters"), Some(2));
    assert_eq!(c.int("top_solid_layers"), Some(5));
    assert_eq!(c.int("bottom_solid_layers"), Some(3));
    assert_eq!(c.float("fill_density"), Some(15.0));
    assert_eq!(c.string("fill_pattern"), Some(pattern));
    for (key, expected) in [
        ("extrusion_width", 0.42),
        ("perimeter_extrusion_width", 0.45),
        ("external_perimeter_extrusion_width", 0.42),
        ("first_layer_extrusion_width", 0.5),
        ("infill_extrusion_width", 0.45),
        ("solid_infill_extrusion_width", 0.42),
        ("top_infill_extrusion_width", 0.42),
        ("support_material_extrusion_width", 0.42),
        ("perimeter_speed", 300.0),
        ("external_perimeter_speed", 200.0),
        ("infill_speed", 270.0),
        ("solid_infill_speed", 250.0),
        ("top_solid_infill_speed", 200.0),
        ("first_layer_speed", 50.0),
        ("small_perimeter_speed", 100.0),
        ("travel_speed", 500.0),
    ] {
        assert_eq!(c.float(key), Some(expected), "{key}");
    }
    for tool in 0..4 {
        assert_eq!(c.int_at("temperature", tool), Some(220));
        assert_eq!(c.int_at("first_layer_temperature", tool), Some(220));
        assert_eq!(c.float_at("nozzle_diameter", tool), Some(0.4));
        assert_eq!(c.float_at("retract_length", tool), Some(retraction));
        assert_eq!(c.float_at("retract_lift", tool), Some(0.4));
        assert_eq!(c.float_at("retract_speed", tool), Some(retract_speed));
    }
    assert!(
        matches!(c.get("nozzle_diameter"),Some(mica_core::config::ConfigValue::Vector(v)) if v.len()==4)
    );
    assert_eq!(
        c.points("bed_shape").unwrap(),
        vec![(0.5, 1.0), (270.5, 1.0), (270.5, 271.0), (0.5, 271.0)]
    );
    assert_eq!(c.string("gcode_flavor"), Some("klipper"));
    assert_eq!(r.orca, *raw);
    assert_eq!(r.report.entries.len(), raw.len());
    eprintln!(
        "{dialect:?} mapping mapped/converted/extras/rejected: {:?}",
        [
            Disposition::Mapped,
            Disposition::Converted,
            Disposition::ExtrasOnly,
            Disposition::Rejected
        ]
        .map(|d| r.report.count(d))
    );
}

#[test]
fn vendor_configs_map_engine_values() {
    if let Some(raw) = composed(Dialect::Snapmaker240) {
        check_engine_values(&raw, Dialect::Snapmaker240, "grid", 1.5, 30.0);
    }
    if let Some(raw) = composed(Dialect::Orca242) {
        check_engine_values(&raw, Dialect::Orca242, "crosshatch", 0.8, 40.0);
    }
}

#[test]
fn saved_project_maps_without_upstream_checkout() {
    let raw: RawConfig =
        serde_json::from_str(include_str!("fixtures/u1/project_settings.config")).unwrap();
    check_engine_values(&raw, Dialect::Snapmaker240, "grid", 1.5, 30.0);
    let r = map_config(&raw, Dialect::Snapmaker240);
    for i in 0..4 {
        assert_eq!(r.config.int_at("bed_temperature", i), Some(65));
        assert_eq!(r.config.int_at("first_layer_bed_temperature", i), Some(65));
    }
}
