// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/GCode.cpp :: GCodeGenerator::process_layer
// Source: orca@8500fcdcca src/libslic3r/GCode.cpp :: GCode::process_layer, GCode::get_highest_bed_temperature
// Changes: Small two-object jobs exercise second-layer temperatures, Klipper startup tracking, command order and absence of a bed wait; Marlin tests retain Prusa deduplication.

use mica_core::{
    config::{Config, ConfigValue, Value},
    mesh::TriangleMesh,
    print_job::{PreparedPrint, PrintObject, ToolAssignments},
    settings::PrintSettings,
};
use mica_gcode::export::export_gcode;

fn config(flavor: &str) -> Config {
    let mut c = Config::defaults();
    for (key, value) in [
        ("printer_model", "XL5"),
        ("gcode_flavor", flavor),
        // The Klipper variant models the U1: Klipper firmware sliced with Orca's rules.
        (
            "mica_slicing_rules",
            if flavor == "klipper" {
                "orcaslicer"
            } else {
                "prusaslicer"
            },
        ),
        ("bed_shape", "0x0,360x0,360x360,0x360"),
        ("nozzle_diameter", "0.4,0.4,0.4,0.4,0.4"),
        ("layer_height", "0.2"),
        ("first_layer_height", "0.2"),
        ("first_layer_temperature", "220,230,240,240,240"),
        ("temperature", "200,210,215,215,215"),
        ("first_layer_bed_temperature", "60,60,60,60,60"),
        ("bed_temperature", "60,60,60,60,60"),
        ("chamber_temperature", "40,45,45,45,45"),
        ("ooze_prevention", "1"),
        ("idle_temperature", "170,175,180,180,180"),
        ("standby_temperature_delta", "-30"),
        ("single_extruder_multi_material", "0"),
        ("wipe_tower", "0"),
        ("skirts", "0"),
        ("support_material_extruder", "0"),
        ("perimeter_generator", "classic"),
        ("use_relative_e_distances", "1"),
        ("start_gcode", "M190 S60"),
        ("toolchange_gcode", "T{next_extruder}"),
        ("before_layer_gcode", ""),
        ("layer_gcode", ""),
        ("end_gcode", ""),
    ] {
        c.set_raw(key, value).unwrap();
    }
    c
}

fn job(c: &Config, second_tool_height: f32) -> String {
    let objects = [0.6, second_tool_height]
        .into_iter()
        .enumerate()
        .map(|(tool, height)| {
            let mut mesh = TriangleMesh::load_stl(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../tests/models/cube20.stl"
            ))
            .unwrap();
            for vertex in &mut mesh.vertices {
                vertex[2] *= height / 20.;
            }
            mesh.place_on_bed(80. + 40. * tool as f32, 80.);
            PrintObject::prepare(
                format!("cube {tool}"),
                tool,
                0,
                mesh,
                ToolAssignments([Some(tool), Some(tool), Some(tool), None, None]),
                c,
            )
            .unwrap()
        })
        .collect();
    let print = PreparedPrint::new(objects, c).unwrap();
    assert_eq!(print.ordering.used, [0, 1]);
    export_gcode(
        &print,
        c,
        &PrintSettings::from_config(c).unwrap(),
        "XL two tools",
    )
    .unwrap()
    .gcode
}

fn layer(gcode: &str, index: usize) -> &str {
    gcode
        .split("; CONFIG_BLOCK_START")
        .next()
        .unwrap()
        .split("; prusaslicer_config = begin")
        .next()
        .unwrap()
        .split(";LAYER_CHANGE\n")
        .nth(index + 1)
        .unwrap()
}

fn nozzle_changes(gcode: &str) -> Vec<&str> {
    layer(gcode, 1)
        .lines()
        .filter(|line| line.starts_with("M104 ") && line.ends_with("; set temperature"))
        .collect()
}

fn bed_changes(gcode: &str) -> Vec<&str> {
    layer(gcode, 1)
        .lines()
        .filter(|line| line.starts_with("M140 "))
        .collect()
}

#[test]
fn xl_second_layer_only_updates_the_active_tool_with_ooze_prevention() {
    for flavor in ["marlin2", "klipper"] {
        for idle in ["170,175", "nil,nil"] {
            let mut c = config(flavor);
            c.set_raw("idle_temperature", idle).unwrap();
            let gcode = job(&c, 0.6);
            // The layer transition precedes layer 2's tool selection: T1 remains active.
            assert_eq!(
                layer(&gcode, 0).lines().rfind(|l| *l == "T0" || *l == "T1"),
                Some("T1")
            );
            assert_eq!(
                nozzle_changes(&gcode),
                ["M104 S210 T1 ; set temperature"],
                "{flavor}, idle={idle}"
            );
            if flavor == "klipper" {
                assert_eq!(bed_changes(&gcode), ["M140 S60 ; set bed temperature"]);
            } else {
                assert!(bed_changes(&gcode).is_empty());
            }
            assert!(
                !layer(&gcode, 1)
                    .lines()
                    .any(|l| l.starts_with("M141 ") || l.starts_with("M191 "))
            );
            assert!(nozzle_changes(&gcode).iter().all(|l| !l.contains("S230")));
        }
    }
}

#[test]
fn second_layer_without_ooze_prevention_updates_all_used_tools() {
    for flavor in ["marlin2", "klipper"] {
        let mut c = config(flavor);
        c.set_raw("ooze_prevention", "0").unwrap();
        assert_eq!(
            nozzle_changes(&job(&c, 0.6)),
            [
                "M104 S200 T0 ; set temperature",
                "M104 S210 T1 ; set temperature"
            ]
        );
    }
}

#[test]
fn second_layer_skips_unchanged_or_zero_active_nozzle_temperatures() {
    for flavor in ["marlin2", "klipper"] {
        for temperature in ["200,230", "200,0"] {
            let mut c = config(flavor);
            c.set_raw("temperature", temperature).unwrap();
            assert!(
                nozzle_changes(&job(&c, 0.6)).is_empty(),
                "{flavor}: {temperature}"
            );
        }
    }
}

#[test]
fn second_layer_bed_uses_the_first_printing_tool_or_explicit_prusa_override() {
    for (flavor, bed_tool, formula, expected) in [
        ("marlin2", "0", "by_first_filament", 55),
        ("marlin2", "2", "by_first_filament", 65),
        ("marlin2", "3", "by_first_filament", 90),
        ("marlin2", "6", "by_first_filament", 55),
        ("klipper", "2", "by_first_filament", 55),
        ("klipper", "0", "by_highest_temp", 65),
        ("klipper", "0", "", 65),
    ] {
        let mut c = config(flavor);
        c.set_raw("bed_temperature", "55,65,90,90,90").unwrap();
        c.set_raw("bed_temperature_extruder", bed_tool).unwrap();
        // Orca's profile mapper supplies a typed enum outside the Prusa option schema.
        if !formula.is_empty() {
            c.set(
                "bed_temperature_formula",
                ConfigValue::Scalar(Value::Enum(formula.into())),
            );
        }
        // T1 finishes on layer 1, so T0 prints first on layer 2. Highest-temp
        // Orca mode considers all used filaments, including T1, but excludes unused T2..4.
        let gcode = job(&c, 0.21);
        // Nozzle selection uses the still-active T1, even though only T0 prints on layer 2.
        assert_eq!(nozzle_changes(&gcode), ["M104 S210 T1 ; set temperature"]);
        assert_eq!(
            bed_changes(&gcode),
            [format!("M140 S{expected} ; set bed temperature")],
            "{flavor}: {bed_tool}, {formula}"
        );
    }
}

#[test]
fn second_layer_can_turn_the_bed_off_and_deduplicates_against_start_temperature() {
    for flavor in ["marlin2", "klipper"] {
        let mut c = config(flavor);
        c.set_raw("bed_temperature", "0,0,0,0,0").unwrap();
        let off = job(&c, 0.6);
        if flavor == "klipper" {
            // Klipper skips first-layer tracking, leaving the writer target at zero.
            assert!(bed_changes(&off).is_empty());
        } else {
            assert_eq!(bed_changes(&off), ["M140 S0 ; set bed temperature"]);
        }
        c.set_raw("bed_temperature", "55,55,55,55,55").unwrap();
        c.set_raw("start_gcode", "M190 S55").unwrap();
        let gcode = job(&c, 0.6);
        if flavor == "marlin2" {
            assert!(bed_changes(&gcode).is_empty());
        } else {
            // Orca's Klipper writer still has its initial target (0), since startup
            // temperature tracking is skipped even when the template uses M190.
            assert_eq!(bed_changes(&gcode), ["M140 S55 ; set bed temperature"]);
        }
    }
}

#[test]
fn orca_second_layer_bed_command_follows_layer_macro_fan_marker_and_acceleration() {
    for flavor in ["klipper", "marlin2"] {
        for (first, other, expected) in [(60, 60, Some(60)), (60, 55, Some(55)), (60, 0, None)] {
            let mut c = config(flavor);
            mica_core::rules::Rules::OrcaSlicer.set(&mut c);
            c.set_raw("first_layer_bed_temperature", &first.to_string())
                .unwrap();
            c.set_raw("bed_temperature", &other.to_string()).unwrap();
            c.set_raw("start_gcode", &format!("M140 S{first}\nM190 S{first}"))
                .unwrap();
            c.set_raw("layer_gcode", ";AFTER_LAYER_CHANGE\nLAYER_MACRO")
                .unwrap();
            c.set_raw("first_layer_acceleration", "500").unwrap();
            c.set_raw("default_acceleration", "1000").unwrap();
            let gcode = job(&c, 0.6);
            let second = layer(&gcode, 1);
            let expected = if flavor == "klipper" {
                expected
            } else if other != first {
                Some(other)
            } else {
                None
            };
            assert_eq!(
                bed_changes(&gcode),
                expected
                    .map(|t| format!("M140 S{t} ; set bed temperature"))
                    .into_iter()
                    .collect::<Vec<_>>()
            );
            assert!(!second.contains("M190"));
            assert_eq!(gcode.lines().filter(|l| l.starts_with("M190 ")).count(), 1);
            if let Some(temp) = expected {
                let at = second
                    .find(&format!("M140 S{temp} ; set bed temperature"))
                    .unwrap();
                let macro_at = second.find("LAYER_MACRO").unwrap();
                let fan_at = second.find(";_SET_FAN_SPEED_CHANGING_LAYER").unwrap();
                let accel = if flavor == "klipper" {
                    "SET_VELOCITY_LIMIT ACCEL=1000"
                } else {
                    "M204 P1000"
                };
                let accel_at = second.find(accel).unwrap();
                let nozzle_at = second.find("M104 S210 T1 ; set temperature").unwrap();
                assert!(
                    macro_at < fan_at
                        && fan_at < accel_at
                        && accel_at < nozzle_at
                        && nozzle_at < at,
                    "{second}"
                );
            }
            assert!(!layer(&gcode, 2).contains("M140"));
        }
    }
}
