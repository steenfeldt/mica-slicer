// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Every Snapmaker U1 filament preset of both pinned profile sets loads, composes and maps.

use std::path::Path;

use mica_profiles::orca::mapping::Disposition;
use mica_profiles::orca::{Bundle, Dialect, Kind, RawConfig, discovery, map_config};

const PRINTER: &str = "Snapmaker U1 (0.4 nozzle)";

fn bundle(repo: &str) -> Option<Bundle> {
    let root = mica_testkit::upstream(repo).join("resources/profiles");
    let available = match repo {
        "orca" => mica_testkit::require_orca_profiles(),
        _ => mica_testkit::require_snapmaker_orca_profiles(),
    };
    if !available {
        return None;
    }
    Some(
        discovery::load_installed(Some(&root), "Snapmaker")
            .unwrap()
            .0,
    )
}

/// Each Snapmaker filament for `printer` with the first process it is compatible with.
fn u1_filaments(b: &Bundle, printer: &str) -> Vec<(String, String)> {
    let processes = b.compatible_presets(Kind::Process, printer, None);
    b.compatible_presets(Kind::Filament, printer, None)
        .into_iter()
        .filter(|f| b.preset(Kind::Filament, f).unwrap().vendor == "Snapmaker")
        .filter_map(|f| {
            let process = processes
                .iter()
                .find(|p| b.is_compatible_with_print(f, p).unwrap())?;
            Some((f.to_string(), process.to_string()))
        })
        .collect()
}

/// Every vendor filament of every U1 nozzle composes and maps without a rejected value.
fn loads_every_u1_filament(repo: &str, dialect: Dialect, printers: &[(&str, usize)]) {
    let Some(b) = bundle(repo) else {
        return;
    };
    assert_eq!(b.dialect, dialect);
    for (printer, min) in printers {
        let filaments = u1_filaments(&b, printer);
        assert!(filaments.len() >= *min, "{printer}: {}", filaments.len());
        let failures: Vec<_> = filaments
            .iter()
            .filter_map(|(f, p)| {
                b.single_tool_config(printer, p, f, Some("High Temp Plate"))
                    .err()
                    .map(|e| format!("{printer} / {f}: {e}"))
            })
            .collect();
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }
}

#[test]
fn orca_u1_filaments_all_load() {
    loads_every_u1_filament(
        "orca",
        Dialect::Orca242,
        &[
            (PRINTER, 65),
            ("Snapmaker U1 (0.2 nozzle)", 0), // OrcaSlicer ships no Snapmaker filament for 0.2 mm
            ("Snapmaker U1 (0.6 nozzle)", 1),
            ("Snapmaker U1 (0.8 nozzle)", 1),
        ],
    );
}

#[test]
fn snapmaker_orca_u1_filaments_all_load() {
    loads_every_u1_filament(
        "snapmaker-orca",
        Dialect::Snapmaker240,
        &[
            (PRINTER, 43),
            ("Snapmaker U1 (0.2 nozzle)", 1),
            ("Snapmaker U1 (0.6 nozzle)", 1),
            ("Snapmaker U1 (0.8 nozzle)", 1),
        ],
    );
}

/// Regression: Polymaker's U1 presets give the integer `temperature_vitrification` as "59.8".
/// Orca reads integers with `istream >> int`, keeping the leading digits.
#[test]
fn fractional_integers_truncate_like_orca() {
    for (text, expected) in [
        ("59.8", 59),
        ("71.24", 71),
        ("-3.7", -3),
        ("1e3", 1),
        ("45", 45),
    ] {
        let raw = RawConfig::from([(
            "temperature_vitrification".to_string(),
            serde_json::json!([text]),
        )]);
        let mapped = map_config(&raw, Dialect::Orca242);
        assert_eq!(
            mapped.report.count(Disposition::Rejected),
            0,
            "{text}: {:?}",
            mapped.report
        );
        assert_eq!(
            mapped.config.int_at("temperature_vitrification", 0),
            Some(expected),
            "{text}"
        );
    }
    let raw = RawConfig::from([(
        "temperature_vitrification".to_string(),
        serde_json::json!(["warm"]),
    )]);
    let mapped = map_config(&raw, Dialect::Orca242);
    assert_eq!(mapped.report.count(Disposition::Rejected), 1);
    let Some(b) = bundle("orca") else {
        return;
    };
    for (filament, expected) in [
        ("Polymaker HT-PLA @Snapmaker U1", 59),
        ("Polymaker PETG @Snapmaker U1", 71),
        ("Fiberon ASA-CF08 @Snapmaker U1", 110),
    ] {
        let c = b
            .single_tool_config(
                PRINTER,
                "0.20 Standard @Snapmaker U1 (0.4 nozzle)",
                filament,
                Some("High Temp Plate"),
            )
            .unwrap();
        assert_eq!(c.int_at("temperature_vitrification", 0), Some(expected));
    }
}

#[test]
fn u1_filament_lists_follow_orca_compatibility() {
    if let Some(b) = bundle("orca") {
        let standard = "0.20 Standard @Snapmaker U1 (0.4 nozzle)";
        let u1 = b.compatible_presets(Kind::Filament, PRINTER, Some(standard));
        assert!(u1.contains(&"Snapmaker PLA @U1"));
        assert!(u1.contains(&"Polymaker HT-PLA @Snapmaker U1"));
        // Library filaments have no compatible_printers, so they fit every printer.
        assert!(u1.contains(&"Generic PLA @System"));
        // Restricted to its own process (compatible_prints).
        assert!(!u1.contains(&"Snapmaker ABS Benchy @U1"));
        let benchy = b.compatible_presets(
            Kind::Filament,
            PRINTER,
            Some("0.25 Benchy @Snapmaker U1 (0.4 nozzle)"),
        );
        assert!(benchy.contains(&"Snapmaker ABS Benchy @U1"));
        // Other Snapmaker printers' filaments and other nozzles stay out.
        assert!(!u1.iter().any(|f| f.contains("@Snapmaker J1")));
        let big = b.compatible_presets(Kind::Filament, "Snapmaker U1 (0.6 nozzle)", None);
        assert!(!big.contains(&"Snapmaker PLA @U1"));
        assert!(big.contains(&"Snapmaker TPU @U1"));
    }
    if let Some(b) = bundle("snapmaker-orca") {
        let u1 = b.compatible_presets(
            Kind::Filament,
            PRINTER,
            Some("0.20mm Standard @Snapmaker U1 (0.4 nozzle)"),
        );
        assert!(u1.contains(&"Snapmaker ABS @U1 0.4 nozzle"));
        assert!(u1.contains(&"Generic PLA"));
        assert!(!u1.contains(&"Snapmaker ABS @U1 0.2 nozzle"));
        // Snapmaker Orca's OrcaFilamentLibrary index is empty.
        assert!(!u1.iter().any(|f| f.ends_with("@System")));
        let small = b.compatible_presets(Kind::Filament, "Snapmaker U1 (0.2 nozzle)", None);
        assert!(small.contains(&"Snapmaker ABS @U1 0.2 nozzle"));
        assert!(!small.contains(&"Snapmaker ABS @U1 0.4 nozzle"));
    }
}

/// Snapmaker Orca filaments declare `filament_flow_support = [standard, high_flow]` and one
/// value per variant; each tool reads the variant of its project flow type (standard default).
#[test]
fn snapmaker_flow_variants_select_the_tool_flow_type() {
    let Some(b) = bundle("snapmaker-orca") else {
        return;
    };
    let process = "0.20mm Standard @Snapmaker U1 (0.4 nozzle)";
    let matte = "Snapmaker PLA Matte @U1";
    let project = RawConfig::from([
        (
            "curr_bed_type".to_string(),
            serde_json::json!("High Temp Plate"),
        ),
        (
            "filament_volume_type".to_string(),
            serde_json::json!(["standard", "high_flow", "standard", "high_flow"]),
        ),
    ]);
    let raw = b
        .full_config_with_project(PRINTER, process, &[matte; 4], &project)
        .unwrap();
    let strings = |k: &str| -> Vec<String> {
        raw[k]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(strings("nozzle_temperature"), ["215", "220", "215", "220"]);
    assert_eq!(
        strings("additional_cooling_fan_speed"),
        ["80", "100", "80", "100"]
    );
    assert_eq!(
        strings("filament_flow_support"),
        ["standard", "high_flow", "standard", "high_flow"]
    );
    // Not a flow-variant option: the first value, as upstream composes it.
    assert_eq!(strings("filament_type"), ["PLA"; 4]);
    let c = b
        .single_tool_config(PRINTER, process, matte, Some("High Temp Plate"))
        .unwrap();
    assert_eq!(c.int_at("first_layer_temperature", 0), Some(215));
    assert_eq!(c.float_at("additional_cooling_fan_speed", 0), Some(80.));
}

fn cube(config: &mica_core::config::Config) -> String {
    use mica_core::{mesh::TriangleMesh, print::slice_object, settings::PrintSettings};
    let s = PrintSettings::from_config(config).unwrap();
    let mut mesh = TriangleMesh::load_stl(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/models/cube20.stl"),
    )
    .unwrap();
    mesh.place_on_bed(135.5, 136.0);
    let obj = slice_object(&mesh, &s).unwrap();
    mica_gcode::export::export_gcode_with_mesh(&obj, config, &s, "cube20.stl", &mesh)
        .unwrap()
        .gcode
}

/// Snapmaker Orca's start G-code branches on `chamber_cooling_mode`; filaments that activate
/// air filtration switch the exhaust fan on after the start and end G-code.
#[test]
fn snapmaker_orca_chamber_and_exhaust_fan() {
    let Some(b) = bundle("snapmaker-orca") else {
        return;
    };
    let process = "0.20mm Standard @Snapmaker U1 (0.4 nozzle)";
    let config = |f| {
        b.single_tool_config(PRINTER, process, f, Some("High Temp Plate"))
            .unwrap()
    };
    // Softens at 45 °C: strong cooling.
    let pla = cube(&config("Snapmaker PLA Basic @U1"));
    assert!(pla.contains("SET_PURIFIER_MODE MODE=1 DESIRE_TEMP=42"));
    assert!(!pla.contains("M106 P3"));
    // A high-temperature filament keeps the chamber warm.
    let abs = cube(&config("Snapmaker ABS @U1 0.4 nozzle"));
    assert!(abs.contains("SET_PURIFIER_MODE MODE=3 DESIRE_TEMP=45"));
    assert_eq!(abs.matches("\nM106 P3 S178\n").count(), 2);
    assert!(abs.contains("TIMELAPSE_STOP\nM106 P3 S178\n; EXECUTABLE_BLOCK_END"));
}
