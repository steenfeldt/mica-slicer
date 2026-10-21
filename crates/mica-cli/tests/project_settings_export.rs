// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Exercise the actual CLI, including its export-only and export-with-slice paths.
use mica_core::threemf::{self, Flavour};
use std::{
    path::{Path, PathBuf},
    process::Command,
};

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "mica-cli-export-test-{}-{}",
            std::process::id(),
            N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn model(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/models")
        .join(name)
}

/// The pinned Orca profiles with the Snapmaker U1 (a reported skip without them).
fn u1_available() -> bool {
    mica_testkit::require_orca_profiles()
}

fn run(source: &Path, output: &Path, flavour: &str, extra: &[&str]) {
    let result = Command::new(env!("CARGO_BIN_EXE_mica"))
        .env("MICA_ISOLATED", "1")
        .arg("slice")
        .arg(source)
        .args([
            "--vendor",
            "Snapmaker",
            "--printer",
            "Snapmaker U1 (0.4 nozzle)",
            "--print",
            "0.20 Standard @Snapmaker U1 (0.4 nozzle)",
            "--filament",
            "Snapmaker PLA @U1",
            "--bed",
            "High Temp Plate",
            "--plate",
            "1",
            "--export-3mf",
        ])
        .arg(output)
        .args(["--flavour", flavour])
        .args(extra)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn native_fixture_settings_are_verbatim_with_or_without_slicing() {
    if !u1_available() {
        return;
    }
    let dir = Temp::new();
    for (name, flavour) in [("plates-orca.3mf", "orca"), ("painted-cube.3mf", "prusa")] {
        let mut source = model(name);
        if threemf::load_config(&source).unwrap().is_none() {
            let mut project = threemf::load_3mf(&source).unwrap();
            project.config = Some(threemf::RawConfig {
                path: "Metadata/Slic3r_PE.config".into(),
                text: "; fixture source formatting\r\n; printer_settings_id = Snapmaker U1 (0.4 nozzle)\r\n; perimeters = 2\r\n; fill_density = 8%\r\n; bridge_acceleration = 50%\r\n".into(),
            });
            source = dir.0.join(name);
            threemf::write(&source, &project, Flavour::Prusa).unwrap();
        }
        let original = threemf::load_config(&source).unwrap().unwrap();
        for slice in [false, true] {
            let output = dir.0.join(format!("{name}-{slice}.3mf"));
            let gcode = dir.0.join(format!("{name}.gcode"));
            // Cover both explicit use and automatic use for a matching printer.
            let mut extra = if slice && flavour == "orca" {
                vec![]
            } else {
                vec!["--project-settings", "use"]
            };
            if slice {
                extra.extend(["-o", gcode.to_str().unwrap()]);
            }
            run(&source, &output, flavour, &extra);
            let saved = threemf::load_config(&output).unwrap().unwrap();
            assert_eq!(saved.path, original.path);
            assert_eq!(
                saved.text.as_bytes(),
                original.text.as_bytes(),
                "{name}, slice={slice}"
            );
            if slice {
                // plates-orca.3mf prints Generic PLA/PLA/PETG/PLA @System; the validator's
                // single-filament profile needs the PETG tool's temperatures.
                let per_tool: &[&str] = if name == "plates-orca.3mf" {
                    &[
                        "--set",
                        "temperature=220,220,255,220",
                        "--set",
                        "first_layer_temperature=220,220,255,220",
                        "--set",
                        "nozzle_temperature_range_low=190,190,220,190",
                        "--set",
                        "nozzle_temperature_range_high=240,240,260,240",
                    ]
                } else {
                    &[]
                };
                let result = Command::new(env!("CARGO_BIN_EXE_mica"))
                    .env("MICA_ISOLATED", "1")
                    .arg("validate")
                    .arg(&gcode)
                    .args(["--vendor", "Snapmaker", "--json"])
                    .args(per_tool)
                    .output()
                    .unwrap();
                assert!(
                    result.status.success(),
                    "{}",
                    String::from_utf8_lossy(&result.stdout)
                );
                assert!(
                    String::from_utf8_lossy(&result.stdout)
                        .replace(' ', "")
                        .contains("\"errors\":0"),
                    "{}",
                    String::from_utf8_lossy(&result.stdout)
                );
            }
        }
    }
}

#[test]
fn ignoring_project_settings_exports_native_selected_profiles() {
    if !u1_available() {
        return;
    }
    let dir = Temp::new();
    // Add source-only data and values which would be corrupted by a canonical round trip.
    let mut project = threemf::load_3mf(model("plates-orca.3mf")).unwrap();
    project.config.as_mut().unwrap().text = r#"{"printer_settings_id":"Snapmaker U1 (0.4 nozzle)","source_only":"sentinel","wall_loops":"9"}"#.into();
    let source = dir.0.join("source.3mf");
    threemf::write(&source, &project, Flavour::Orca).unwrap();
    let output = dir.0.join("ignored.3mf");
    run(
        &source,
        &output,
        "orca",
        &["--project-settings", "ignore", "--set", "perimeters=5"],
    );
    let text = threemf::load_config(&output).unwrap().unwrap().text;
    assert!(!text.contains("source_only"));
    assert!(text.contains(r#""wall_loops":"5""#));
    assert!(!text.contains(r#""perimeters":"#));
    assert!(text.contains(r#""ensure_vertical_shell_thickness":"ensure_all""#));
    assert!(text.contains(r#""curr_bed_type":"High Temp Plate""#));
    let parsed = mica_profiles::project::ProjectSettings::parse(&threemf::RawConfig {
        path: "Metadata/project_settings.config".into(),
        text,
    })
    .unwrap();
    assert_eq!(parsed.filaments.len(), 4);
    assert_eq!(
        parsed.changes.process.get("perimeters").map(String::as_str),
        Some("5")
    );
}

#[test]
#[ignore = "needs a private model: set MICA_CAT_PROJECT (or MICA_PRIVATE_MODELS), cargo test -- --ignored"]
fn private_cat_no_change_export_is_verbatim() {
    let source = std::env::var_os("MICA_CAT_PROJECT")
        .map(PathBuf::from)
        .unwrap_or_else(|| mica_testkit::private_file("cat.3mf"));
    assert!(source.is_file(), "{} is missing", source.display());
    assert!(u1_available(), "the Snapmaker U1 profiles are missing");
    let dir = Temp::new();
    let original = threemf::load_config(&source).unwrap().unwrap();
    let output = dir.0.join("cat.3mf");
    run(&source, &output, "orca", &["--project-settings", "use"]);
    assert_eq!(
        threemf::load_config(output)
            .unwrap()
            .unwrap()
            .text
            .as_bytes(),
        original.text.as_bytes()
    );
}
