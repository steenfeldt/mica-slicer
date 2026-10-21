// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

use mica_core::threemf::{Flavour, Project, RawConfig, Settings, translate};
use mica_profiles::project::ProjectSettings;

#[test]
fn every_support_mode_roundtrips_through_native_orca_settings() {
    for auto in ["0", "1"] {
        for style in ["grid", "snug", "tree", "organic"] {
            let canonical = Settings::from([
                ("support_material_auto".into(), auto.into()),
                ("support_material_style".into(), style.into()),
            ]);
            let native = translate(&canonical, Flavour::Prusa, Flavour::Orca);
            assert!(native["support_type"].ends_with(if auto == "0" {
                "(manual)"
            } else {
                "(auto)"
            }));
            assert_eq!(translate(&native, Flavour::Orca, Flavour::Prusa), canonical);
            let mut p = Project {
                config: Some(RawConfig {
                    path: "Metadata/project_settings.config".into(),
                    text: r#"{"filament_settings_id":["PLA"],"different_settings_to_system":["support_type;support_style","",""]}"#.into(),
                }),
                ..Default::default()
            };
            p.update_settings(&canonical).unwrap();
            let parsed = ProjectSettings::parse(p.config.as_ref().unwrap()).unwrap();
            assert_eq!(parsed.changes.process["support_material_auto"], auto);
            assert_eq!(parsed.changes.process["support_material_style"], style);
        }
    }
}

#[test]
fn editing_one_support_setting_keeps_the_other_native_setting() {
    let mut p = Project {
        config: Some(RawConfig {
            path: "Metadata/project_settings.config".into(),
            text: r#"{"support_type":"tree(manual)","support_style":"default"}"#.into(),
        }),
        ..Default::default()
    };
    p.update_settings(&Settings::from([(
        "support_material_auto".into(),
        "1".into(),
    )]))
    .unwrap();
    assert!(p.config.as_ref().unwrap().text.contains("tree(auto)"));
    p.update_settings(&Settings::from([(
        "support_material_auto".into(),
        "0".into(),
    )]))
    .unwrap();
    p.update_settings(&Settings::from([(
        "support_material_style".into(),
        "snug".into(),
    )]))
    .unwrap();
    let raw: serde_json::Value = serde_json::from_str(&p.config.unwrap().text).unwrap();
    assert_eq!(raw["support_type"], "normal(manual)");
    assert_eq!(raw["support_style"], "snug");
}
