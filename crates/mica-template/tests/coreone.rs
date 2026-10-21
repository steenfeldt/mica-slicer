// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Acceptance tests: the Prusa CORE One HF0.4 custom G-code (vendor profiles 2.5.12) must expand
//! exactly as PrusaSlicer 2.9.6 expands it. The templates come from the resolved-config fixture
//! `mica-profiles/tests/fixtures/coreone_hf04_speed_pla.ini` (derived from PrusaSlicer's AGPL-3.0
//! profiles; see its header).

use mica_template::{Context, Value, ValueType, Var, process};

/// The `key` custom G-code template of the resolved CORE One config, `\n` escapes expanded.
fn template(key: &str) -> String {
    let path = format!(
        "{}/../mica-profiles/tests/fixtures/coreone_hf04_speed_pla.ini",
        env!("CARGO_MANIFEST_DIR")
    );
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let prefix = format!("{key} = ");
    let value = text
        .lines()
        .find_map(|l| l.strip_prefix(&prefix))
        .unwrap_or_else(|| panic!("{path}: no {key}"));
    assert!(
        !value.contains("\\\\"),
        "{key}: unexpected escaped backslash"
    );
    value.replace("\\n", "\n")
}

fn non_blank_lines(s: &str) -> Vec<&str> {
    s.lines().filter(|l| !l.trim().is_empty()).collect()
}

/// Variables PrusaSlicer passes when slicing tests/models/cube20.stl (centred at 125,110) with
/// Prusament PLA @COREONE HF0.4.
fn cube20_context() -> Context {
    let mut ctx = Context::with_seed(0);
    ctx.set("nozzle_diameter", Var::floats([0.4]));
    ctx.set("filament_abrasive", Var::bools([false]));
    ctx.set("nozzle_high_flow", Var::bools([true]));
    ctx.set("first_layer_print_min", Var::floats([115.0, 100.0]));
    ctx.set("first_layer_print_max", Var::floats([135.0, 120.0]));
    ctx.set("print_bed_max", Var::floats([250.0, 220.0]));
    ctx.set("chamber_minimal_temperature", Var::ints([0]));
    ctx.set("chamber_temperature", Var::ints([20]));
    ctx.set("initial_tool", 0);
    ctx.set("first_layer_bed_temperature", Var::ints([60]));
    ctx.set("first_layer_temperature", Var::ints([230]));
    ctx.set("filament_notes", Var::strings([""]));
    ctx.set("filament_type", Var::strings(["PLA"]));
    // A nullable option (PLA sets it; other filaments may leave it nil).
    ctx.set(
        "idle_temperature",
        Var::nullable(ValueType::Int, [Some(Value::Int(170))]),
    );
    // Printer settings used by the end G-code.
    ctx.set("max_print_height", 270.0);
    ctx.set("z_offset", 0.0);
    ctx
}

#[test]
fn coreone_start_gcode_matches_prusaslicer() {
    let mut ctx = cube20_context();
    let out = process(&template("start_gcode"), &mut ctx).unwrap_or_else(|e| panic!("{e}"));

    let expected_path = format!(
        "{}/../mica-gcode/tests/fixtures/coreone_cube20_start.gcode",
        env!("CARGO_MANIFEST_DIR")
    );
    let expected_text = std::fs::read_to_string(&expected_path).unwrap();
    // Skip the 4 comment header lines; drop the G21/G90/M83 lines PrusaSlicer appends after the template.
    let mut expected: Vec<&str> = non_blank_lines(&expected_text)[4..].to_vec();
    let appended = expected.split_off(expected.len() - 3);
    assert!(appended[0].starts_with("G21 ") && appended[1].starts_with("G90 "));
    assert!(appended[2].starts_with("M83 "));

    assert_eq!(non_blank_lines(&out), expected);
}

#[test]
fn coreone_start_gcode_nil_idle_temperature_and_chamber_heating() {
    let mut ctx = cube20_context();
    ctx.set("idle_temperature", Var::nullable(ValueType::Int, [None]));
    ctx.set("chamber_minimal_temperature", Var::ints([35]));
    ctx.set("filament_type", Var::strings(["PC"]));
    // The chamber-heating branch reads idle_temperature, which is nil: PrusaSlicer fails the same way.
    let e = process(&template("start_gcode"), &mut ctx).unwrap_err();
    assert!(e.message.contains("nil"), "{e}");

    ctx.set("chamber_minimal_temperature", Var::ints([0]));
    let out = process(&template("start_gcode"), &mut ctx).unwrap();
    assert!(out.contains("\nM104 T0 S100\n"), "{out}");
    assert!(out.contains("\nM109 R205 ; wait for temp\n"), "{out}");
}

#[test]
fn coreone_end_gcode() {
    let mut ctx = cube20_context();
    ctx.set("layer_z", 20.0);
    ctx.set("max_layer_z", 20.0);
    let out = process(&template("end_gcode"), &mut ctx).unwrap();
    let lines = non_blank_lines(&out);
    assert_eq!(lines[0], "G1 Z21 F720 ; Move print head up");
    assert_eq!(lines[1], "M104 S0 ; turn off temperature");
    assert_eq!(*lines.last().unwrap(), "; max_layer_z = 20");

    // At the top of the build volume the head is not lifted.
    ctx.set("layer_z", 270.0);
    ctx.set("max_layer_z", 270.0);
    let out = process(&template("end_gcode"), &mut ctx).unwrap();
    assert_eq!(non_blank_lines(&out)[0], "M104 S0 ; turn off temperature");
}

#[test]
fn coreone_before_layer_gcode() {
    let mut ctx = cube20_context();
    ctx.set("max_layer_z", 20.0);

    ctx.set("layer_z", 20.0);
    let out = process(&template("before_layer_gcode"), &mut ctx).unwrap();
    assert_eq!(
        non_blank_lines(&out),
        [";BEFORE_LAYER_CHANGE", "G92 E0.0", ";20"]
    );

    ctx.set("layer_z", 175.0);
    let out = process(&template("before_layer_gcode"), &mut ctx).unwrap();
    assert_eq!(
        non_blank_lines(&out),
        [
            ";BEFORE_LAYER_CHANGE",
            "G92 E0.0",
            ";175",
            "M201 X5500 Y5500"
        ]
    );

    ctx.set("layer_z", 0.2);
    let out = process(&template("before_layer_gcode"), &mut ctx).unwrap();
    assert_eq!(non_blank_lines(&out)[2], ";0.2");
}
