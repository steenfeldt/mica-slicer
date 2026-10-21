// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Resolves real Prusa presets from the pinned vendor bundle and compares every value with the
//! full config PrusaSlicer 2.9.6 wrote for the same selection. Skipped (reported; a failure with
//! MICA_REQUIRE_FIXTURES=1) when the upstream data is missing (scripts/fetch-upstream.sh).

use mica_profiles::prusa::Bundle;

fn bundle() -> Option<Bundle> {
    if !mica_testkit::require_prusa_profiles() {
        return None;
    }
    let text = std::fs::read_to_string(mica_testkit::prusa_bundle()).unwrap();
    Some(Bundle::parse(&text))
}

#[test]
fn core_one_speed_pla_matches_prusaslicer() {
    let Some(bundle) = bundle() else { return };
    let config = bundle
        .full_config(
            "Prusa CORE One HF0.4 nozzle",
            "0.20mm SPEED @COREONE HF0.4",
            "Prusament PLA @COREONE HF0.4",
        )
        .unwrap();
    // The reference was exported with `--no-binary-gcode`, overriding the printer's `binary_gcode = 1`.
    let mut config = config;
    config.set_raw("binary_gcode", "0").unwrap();
    let expected = mica_profiles::ini::parse(include_str!("fixtures/coreone_hf04_speed_pla.ini"));
    let mut mismatches = Vec::new();
    let mut checked = 0;
    for (key, want) in &expected[0].entries {
        checked += 1;
        let got = config.raw(key);
        if got.as_deref() != Some(want.as_str()) {
            mismatches.push(format!("{key}:\n   mica: {got:?}\n   prusa: {want:?}"));
        }
    }
    assert!(
        mismatches.is_empty(),
        "{} of {checked} keys differ:\n{}",
        mismatches.len(),
        mismatches.join("\n")
    );
}

#[test]
fn engine_settings_from_profiles_equal_the_m0_settings() {
    use mica_core::settings::PrintSettings;
    let Some(bundle) = bundle() else { return };
    let config = bundle
        .full_config(
            "Prusa CORE One HF0.4 nozzle",
            "0.20mm SPEED @COREONE HF0.4",
            "Prusament PLA @COREONE HF0.4",
        )
        .unwrap();
    let from_profiles = PrintSettings::from_config(&config).unwrap();
    // The baseline hard-codes these values from PrusaSlicer's output; resolving the real profiles must agree.
    // Pressure advance lives in the filament start G-code, not in a setting, so it is excluded.
    // The organic tree settings are derived from the same keys (and the bed shape) by
    // `tree3d::settings`, which has its own tests; the baseline does not carry them.
    assert!(from_profiles.tree3d.is_some());
    let expected = PrintSettings {
        pressure_advance: 0.0,
        tree3d: from_profiles.tree3d.clone(),
        ..PrintSettings::core_one_hf04_speed_pla()
    };
    assert_eq!(from_profiles, expected);
}

#[test]
fn compatibility_follows_prusaslicer() {
    use mica_profiles::prusa::{BundleError, Kind};
    let Some(bundle) = bundle() else { return };
    let xl = "Original Prusa XL - 5T Input Shaper 0.4 nozzle";
    // PrusaSlicer 2.9.6 refuses this filament for the 5T Input Shaper XL ("not compatible").
    assert!(
        !bundle
            .is_compatible_with_printer(Kind::Filament, "Prusament PLA @XL", xl)
            .unwrap()
    );
    assert!(
        bundle
            .is_compatible_with_printer(Kind::Filament, "Prusament PLA @XLIS", xl)
            .unwrap()
    );
    assert!(matches!(
        bundle.full_config(xl, "0.20mm SPEED @XLIS 0.4", "Prusament PLA @XL"),
        Err(BundleError::Incompatible { .. })
    ));
    let core_one = "Prusa CORE One HF0.4 nozzle";
    let prints = bundle.compatible_presets(Kind::Print, core_one, None);
    assert!(prints.contains(&"0.20mm SPEED @COREONE HF0.4"));
    assert!(!prints.iter().any(|p| p.contains("@XL")));
    let filaments = bundle.compatible_presets(
        Kind::Filament,
        core_one,
        Some("0.20mm SPEED @COREONE HF0.4"),
    );
    assert!(filaments.contains(&"Prusament PLA @COREONE HF0.4"));
}

#[test]
fn xl_5t_custom_gcode_matches_prusaslicer() {
    use mica_core::{mesh::TriangleMesh, print::slice_object, settings::PrintSettings};
    // Needs the bundle the reference was made with (profiles 2.5.11; the pinned copy, never the
    // user's PrusaSlicer install).
    let path = mica_testkit::upstream("prusa-profiles/PrusaResearch/2.5.11.ini");
    if !mica_testkit::require_fixture(
        &path,
        "Prusa vendor profiles: scripts/fetch-upstream.sh prusa-profiles",
    ) {
        return;
    }
    let bundle = Bundle::parse(&std::fs::read_to_string(&path).unwrap());
    assert_eq!(
        bundle.vendor.get("config_version").map(String::as_str),
        Some("2.5.11"),
        "{}",
        path.display()
    );
    let config = bundle
        .full_config(
            "Original Prusa XL - 5T Input Shaper 0.4 nozzle",
            "0.20mm SPEED @XLIS 0.4",
            "Prusament PLA @XLIS",
        )
        .unwrap();
    let settings = PrintSettings::from_config(&config).unwrap();
    let mut mesh = TriangleMesh::load_stl(mica_testkit::model("cube20.stl")).unwrap();
    mesh.place_on_bed(180.0, 180.0);
    let obj = slice_object(&mesh, &settings).unwrap();
    let gcode = mica_gcode::export::export_gcode(&obj, &config, &settings, "cube20.stl")
        .unwrap()
        .gcode;

    let non_empty = |s: &str| {
        s.lines()
            .filter(|l| !l.is_empty() && !l.starts_with("M73 "))
            .map(str::to_string)
            .collect::<Vec<_>>()
    };
    let start = non_empty(
        &gcode[gcode.find(";TYPE:Custom").unwrap()..gcode.find(";LAYER_CHANGE").unwrap()],
    );
    let end_at = gcode.find("; Filament-specific end gcode").unwrap();
    let end = non_empty(
        &gcode[end_at
            ..gcode[end_at..]
                .find("; filament used")
                .map_or(gcode.len(), |i| end_at + i)],
    );
    let fixture = include_str!("fixtures/xl5is_cube20_prusaslicer.gcode");
    let (expected_start, expected_end) = fixture.split_once("; ---- end ----\n").unwrap();
    let expected_start: Vec<String> = non_empty(expected_start)
        .into_iter()
        .filter(|l| {
            !l.starts_with("; PrusaSlicer 2.9.6")
                && !l.starts_with("; print '")
                && !l.starts_with("; layer and")
        })
        .collect();
    assert_eq!(start, expected_start);
    assert_eq!(end, non_empty(expected_end));
}
