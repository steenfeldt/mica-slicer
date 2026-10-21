// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

use mica_profiles::orca::mapping::{Disposition, mapping_table};
use mica_profiles::orca::{Bundle, Dialect, Kind, RawConfig, map_config, option_text};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

fn raw(value: Value) -> RawConfig {
    serde_json::from_value(value).unwrap()
}

#[test]
fn single_wall_top_and_independent_infill_speeds_reach_the_engine() {
    use mica_core::settings::{PrintSettings, TopOnePerimeter};
    let mapped = map_config(
        &raw(json!({
            "only_one_wall_top": "1", "only_one_wall_first_layer": "1",
            "min_width_top_surface": "300%", "inner_wall_line_width": "0.45",
            "initial_layer_infill_speed": "105", "bridge_speed": "50",
            "internal_bridge_speed": "150%", "minimum_sparse_infill_area": "15"
        })),
        Dialect::Orca242,
    );
    assert_eq!(mapped.report.count(Disposition::Rejected), 0);
    let s = PrintSettings::from_config(&mapped.config).unwrap();
    assert_eq!(s.top_one_perimeter, TopOnePerimeter::TopSurfaces);
    assert!(s.only_one_perimeter_first_layer);
    assert!((s.top_surface_min_width.resolve(0.45, 1.) - 1.35).abs() < 1e-8);
    assert_eq!(s.top_surface_wall_extension, 0.85);
    assert_eq!(s.minimum_sparse_infill_area, Some(15.));
    assert_eq!(mapped.config.float("first_layer_infill_speed"), Some(105.));
    assert_eq!(mapped.config.float("internal_bridge_speed"), Some(75.));
    let prusa = PrintSettings::from_config(&mica_core::config::Config::defaults()).unwrap();
    assert_eq!(prusa.top_one_perimeter, TopOnePerimeter::None);
    assert_eq!(prusa.top_surface_wall_extension, 1.);
    assert_eq!(prusa.minimum_sparse_infill_area, None);
}
#[test]
fn batch_a_options_map_independently_and_keep_the_flow_curve() {
    use mica_core::settings::PrintSettings;
    for dialect in [Dialect::Orca242, Dialect::Snapmaker240] {
        let vector = dialect == Dialect::Snapmaker240;
        let mapped = map_config(
            &raw(json!({
                "xy_contour_compensation": "-0.1", "xy_hole_compensation": "0.2",
                "initial_layer_min_bead_width": "110%", "filter_out_gap_fill": "0.5",
                "small_perimeter_threshold": if vector {json!(["4"])} else {json!("4")},
                "small_perimeter_speed": if vector {json!(["25%"])} else {json!("25%")},
                "outer_wall_speed": "80", "small_area_infill_flow_compensation": "1",
                "small_area_infill_flow_compensation_model": ["0,0", "\n10,1"]
            })),
            dialect,
        );
        assert_eq!(
            mapped.report.count(Disposition::Rejected),
            0,
            "{:?}",
            mapped.report
        );
        assert_eq!(mapped.config.float("xy_size_compensation"), Some(0.));
        assert_eq!(mapped.config.float("small_perimeter_speed"), Some(20.));
        let tool = mica_core::print_job::tool_config(&mapped.config, 0);
        let s = PrintSettings::from_config(&tool).unwrap();
        assert_eq!(s.small_features.xy_hole_compensation, 0.2);
        assert_eq!(s.small_features.xy_contour_compensation, -0.1);
        assert_eq!(s.small_features.small_perimeter_threshold, 4.);
        assert_eq!(s.small_features.filter_out_gap_fill, 0.5);
        assert_eq!(
            s.small_features.initial_layer_min_bead_width,
            Some(mica_core::settings::FloatOrPercent::Percent(110.))
        );
        assert!((s.small_features.flow_compensation.unwrap().multiplier(5.) - 0.5).abs() < 1e-12);
    }
}

#[test]
fn zero_small_perimeter_speed_imports_as_orca_auto() {
    for dialect in [Dialect::Orca242, Dialect::Snapmaker240] {
        let speed = if dialect == Dialect::Orca242 {
            json!("0")
        } else {
            json!(["0"])
        };
        let mapped = map_config(
            &raw(json!({"outer_wall_speed":"80","small_perimeter_speed":speed})),
            dialect,
        );
        assert_eq!(mapped.report.count(Disposition::Rejected), 0);
        assert_eq!(mapped.config.float("small_perimeter_speed"), Some(0.));
    }
}

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let p = std::env::temp_dir().join(format!(
            "mica-orca-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }
    fn write(&self, path: &str, value: Value) {
        let p = self.0.join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, serde_json::to_string_pretty(&value).unwrap()).unwrap();
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn vendor() -> (Temp, Bundle) {
    let t = Temp::new();
    t.write("Test.json",json!({"name":"Test","version":"1.0.0","machine_list":[
        {"name":"Printer","sub_path":"machine/child.json"},{"name":"Base","sub_path":"machine/base.json"}],
        "process_list":[{"name":"Process","sub_path":"process/process.json"}],
        "filament_list":[{"name":"PLA","sub_path":"filament/pla.json"},{"name":"PETG","sub_path":"filament/petg.json"}]}));
    t.write("Test/machine/base.json",json!({"name":"Base","instantiation":"false","from":"system","type":"machine","nozzle_diameter":["0.4","0.4"],"single_extruder_multi_material":"0","retraction_length":["0.8"],"machine_start_gcode":"G28\\nM117 \\\"hello\\\"","custom":"base"}));
    t.write("Test/machine/child.json",json!({"name":"Printer","instantiation":"true","inherits":"Base","from":"system","gcode_flavor":"klipper","custom":"child"}));
    t.write("Test/process/process.json",json!({"name":"Process","instantiation":"true","type":"process","layer_height":"0.2","wall_loops":"2","wall_infill_order":"infill/outer wall/inner wall"}));
    t.write("Test/filament/pla.json",json!({"name":"PLA","instantiation":"true","type":"filament","filament_id":"p","nozzle_temperature":["210"],"filament_start_gcode":["M117 PLA; okay\nM400"],"filament_retraction_length":["nil"]}));
    t.write("Test/filament/petg.json",json!({"name":"PETG","instantiation":"true","type":"filament","inherits":"PLA","filament_id":"q","nozzle_temperature":["240"],"filament_retraction_length":["1.2"]}));
    let b = Bundle::load_vendor(t.0.join("Test.json"), Dialect::Orca242).unwrap();
    (t, b)
}

#[test]
fn inheritance_defaults_metadata_and_per_tool_composition() {
    let (_t, b) = vendor();
    assert_eq!(b.preset_names(Kind::Machine), vec!["Printer"]);
    let printer = b.flatten(Kind::Machine, "Printer").unwrap();
    assert_eq!(printer["custom"], "child");
    assert!(!printer.contains_key("from"));
    assert_eq!(printer["retraction_length"], json!(["0.8", "0.8"]));
    assert_eq!(printer["machine_start_gcode"], "G28\nM117 \"hello\"");
    assert!(b.full_config("Base", "Process", &["PLA", "PLA"]).is_err());
    assert!(b.full_config("Printer", "Process", &["PLA"]).is_err());
    assert!(b.full_config("Printer", "Process", &[]).is_err());
    let cfg = b
        .full_config("Printer", "Process", &["PLA", "PETG"])
        .unwrap();
    assert_eq!(cfg["nozzle_temperature"], json!(["210", "240"]));
    assert_eq!(cfg["filament_ids"], json!(["p", "q"]));
    assert_eq!(cfg["wall_sequence"], "outer wall/inner wall");
    assert_eq!(cfg["is_infill_first"], "1");
    assert!(cfg.contains_key("wall_infill_order"));
    let mapped = map_config(&cfg, Dialect::Orca242);
    assert_eq!(mapped.config.int_at("temperature", 1), Some(240));
    assert_eq!(mapped.config.float_at("retract_length", 0), Some(0.8));
    assert_eq!(mapped.config.float_at("retract_length", 1), Some(1.2));
    assert_eq!(
        mapped.config.string("start_gcode"),
        Some("G28\nM117 \"hello\"")
    );
    assert_eq!(
        mapped.config.string_at("start_filament_gcode", 1),
        Some("M117 PLA; okay\nM400")
    );
}

#[test]
fn users_are_deltas_and_load_transactionally() {
    let (t, mut b) = vendor();
    t.write("app/user/default/machine/custom.json",json!({"name":"Custom","from":"user","inherits":"Printer","retraction_length":["1.1","1.3"],"custom":"user override"}));
    b.load_user_presets(t.0.join("app")).unwrap();
    let user = b.preset(Kind::Machine, "Custom").unwrap();
    assert_eq!(user.values.len(), 2);
    assert_eq!(user.from, "user");
    assert!(user.is_user);
    let flat = b.flatten(Kind::Machine, "Custom").unwrap();
    assert_eq!(flat["nozzle_diameter"], json!(["0.4", "0.4"]));
    assert_eq!(flat["retraction_length"], json!(["1.1", "1.3"]));
    assert_eq!(flat["inherits"], "Printer");
    t.write("broken/process/a.json", json!({"name":"A","inherits":"B"}));
    t.write("broken/process/b.json", json!({"name":"B","inherits":"A"}));
    assert!(
        b.load_user_presets(t.0.join("broken"))
            .unwrap_err()
            .0
            .contains("cycle")
    );
    assert!(b.preset(Kind::Process, "A").is_none());
    t.write(
        "missing/process/a.json",
        json!({"name":"A","inherits":"missing"}),
    );
    assert!(
        b.load_user_presets(t.0.join("missing"))
            .unwrap_err()
            .0
            .contains("unknown")
    );
}

#[test]
fn maps_units_enums_percent_bases_and_preserves_all_input() {
    let input = raw(
        json!({"nozzle_diameter":["0.4","0.4"],"layer_height":"0.2","line_width":"110%","initial_layer_line_width":"125%",
        "inner_wall_speed":"300","outer_wall_speed":"200","small_perimeter_speed":"50%","bridge_acceleration":"50%","outer_wall_acceleration":"5000",
        "sparse_infill_pattern":"tri-hexagon","top_surface_pattern":"monotonicline","seam_position":"back","seam_gap":"27%","print_sequence":"by object","gcode_flavor":"reprapfirmware",
        "filament_shrink":["98%","100%"],"filament_shrinkage_compensation_z":["99%","100%"],"min_bead_width":"85",
        "wall_transition_length":"100","infill_wall_overlap":"15","curr_bed_type":"Textured PEI Plate","textured_plate_temp":["60","70"],"cool_plate_temp":["35","35"],
        "unknown_vendor_key":["a;b","c\\d"]}),
    );
    let r = map_config(&input, Dialect::Orca242);
    assert_eq!(r.report.count(Disposition::Rejected), 0);
    assert_eq!(r.config.float("extrusion_width"), Some(0.44));
    assert_eq!(r.config.float("first_layer_extrusion_width"), Some(0.5));
    assert_eq!(r.config.float("small_perimeter_speed"), Some(100.0));
    assert_eq!(r.config.float("bridge_acceleration"), Some(2500.0));
    assert_eq!(r.config.string("fill_pattern"), Some("stars"));
    assert_eq!(r.config.string("top_fill_pattern"), Some("monotoniclines"));
    assert_eq!(r.config.string("seam_position"), Some("rear"));
    assert_eq!(r.config.raw("seam_gap_distance").as_deref(), Some("27%"));
    assert_eq!(r.config.bool("complete_objects"), Some(true));
    assert_eq!(r.config.string("gcode_flavor"), Some("reprapfirmware"));
    assert_eq!(r.config.float_at("bed_temperature", 1), Some(70.0));
    assert_eq!(
        r.config.float_at("filament_shrinkage_compensation_xy", 0),
        Some(2.0)
    );
    assert_eq!(r.config.abs_value("min_bead_width", 0.4), Some(0.34));
    assert_eq!(r.config.abs_value("wall_transition_length", 0.4), Some(0.4));
    assert_eq!(r.config.abs_value("infill_overlap", 0.4), Some(0.06));
    assert_eq!(r.orca, input);
    assert_eq!(r.extras.len(), input.len());
    assert_eq!(r.report.entries.len(), input.len());
}

#[test]
fn malformed_values_are_rejected_and_unsupported_modes_are_extras() {
    let input = raw(
        json!({"wall_loops":"many","layer_height":"NaN","nozzle_temperature":["200","oops"],"use_relative_e_distances":"maybe","line_width":"110%","sparse_infill_pattern":"tpmsd","seam_position":"aligned_back","unknown_bad":42,"unknown_ok":"future"}),
    );
    let r = map_config(&input, Dialect::Orca242);
    let outcomes: BTreeMap<_, _> = r
        .report
        .entries
        .iter()
        .map(|e| (e.key.as_str(), e.disposition))
        .collect();
    for key in [
        "wall_loops",
        "layer_height",
        "nozzle_temperature",
        "use_relative_e_distances",
        "line_width",
        "unknown_bad",
    ] {
        assert_eq!(outcomes[key], Disposition::Rejected, "{key}");
    }
    for key in ["sparse_infill_pattern", "seam_position", "unknown_ok"] {
        assert_eq!(outcomes[key], Disposition::ExtrasOnly, "{key}");
    }
    assert_eq!(r.orca, input);
    assert_eq!(r.extras.len(), input.len());
}

#[test]
fn json_arrays_use_upstream_serialization_and_mapping_table_is_checked() {
    assert_eq!(
        option_text(&json!(["0x0", "270x0"]), "Points").unwrap(),
        "0x0,270x0"
    );
    let text = option_text(&json!(["a;b", "line\n\\path", "\"quote\""]), "Strings").unwrap();
    assert_eq!(
        mica_core::config::unescape_strings(&text).unwrap(),
        vec!["a;b", "line\n\\path", "\"quote\""]
    );
    assert!(option_text(&json!(["1", 2]), "Floats").is_err());
    for (target, op) in mapping_table().values() {
        if *op == "engine" {
            let schema: Value =
                serde_json::from_str(include_str!("../src/orca/orca_schema.json")).unwrap();
            assert!(schema.get(*target).is_some());
            continue;
        }
        assert!(mica_core::config::option_defs().contains_key(target));
    }
}

#[test]
fn user_u1_preset_overrides_two_keys() {
    if !mica_testkit::require_orca_profiles() {
        return;
    }
    let index = mica_testkit::orca_profiles().join("Snapmaker.json");
    let mut b = Bundle::load_vendor(index, Dialect::Orca242).unwrap();
    let t = Temp::new();
    t.write("user/default/machine/u1-copy.json",json!({"name":"U1 user copy","from":"user","inherits":"Snapmaker U1 (0.4 nozzle)","retraction_length":["0.9"],"printer_notes":"User test"}));
    b.load_user_presets(&t.0).unwrap();
    assert_eq!(
        b.preset(Kind::Machine, "U1 user copy")
            .unwrap()
            .values
            .len(),
        2
    );
    let flat = b.flatten(Kind::Machine, "U1 user copy").unwrap();
    assert_eq!(flat["printer_notes"], "User test");
    assert_eq!(
        flat["retraction_length"],
        json!(["0.9", "0.9", "0.9", "0.9"])
    );
    assert_eq!(flat["gcode_flavor"], "klipper");
    assert_eq!(flat["nozzle_diameter"], json!(["0.4", "0.4", "0.4", "0.4"]));
}

#[test]
fn legacy_composites_and_index_paths_are_explicit() {
    let (t, mut bundle) = vendor();
    t.write("legacy/machine/new.json",json!({"name":"Legacy","inherits":"Printer","thumbnail_size":"32x32, 64x64","thumbnails_format":"PNG","enable_wipe_tower":"1","wiping_volumes_matrix":["0","140","140","0"]}));
    bundle.load_user_presets(t.0.join("legacy")).unwrap();
    let flat = bundle.flatten(Kind::Machine, "Legacy").unwrap();
    assert_eq!(flat["thumbnails"], "32x32/PNG, 64x64/PNG");
    assert_eq!(flat["wiping_volumes_use_custom_matrix"], "0");
    assert_eq!(flat["enable_prime_tower"], "1");
    assert!(flat.contains_key("enable_wipe_tower"));
    t.write(
        "Bad.json",
        json!({"machine_list":[{"name":"Bad","sub_path":"../outside.json"}]}),
    );
    assert!(
        Bundle::load_vendor(t.0.join("Bad.json"), Dialect::Orca242)
            .unwrap_err()
            .0
            .contains("unsafe")
    );
}

#[test]
fn invalid_vectors_and_heterogeneous_nozzles_do_not_silently_choose_values() {
    let raw = raw(
        json!({"nozzle_diameter":["0.4","0.6"],"line_width":"110%","fan_min_speed":["22.5"],"retraction_length":[],"filament_retraction_length":["1"]}),
    );
    let result = map_config(&raw, Dialect::Orca242);
    for key in ["line_width", "fan_min_speed", "filament_retraction_length"] {
        assert_eq!(
            result
                .report
                .entries
                .iter()
                .find(|e| e.key == key)
                .unwrap()
                .disposition,
            Disposition::Rejected,
            "{key}"
        );
    }
    assert_eq!(result.extras["nozzle_diameter"], "0.4,0.6");
    assert_eq!(result.orca, raw);
}

#[test]
fn orca_template_options_keep_numeric_boolean_and_vector_types() {
    let r = map_config(
        &raw(json!({
            "nozzle_diameter": ["0.4", "0.6"],
            "nozzle_temperature_initial_layer": ["220", "235"],
            "enable_pressure_advance": ["1", "0"],
            "pressure_advance": ["0.02", "0.03"],
            "initial_layer_travel_acceleration": "50%",
            "travel_acceleration": "10000",
            "curr_bed_type": "High Temp Plate",
            "filament_start_gcode": ["M117 hello", "M117 world"],
            "filament_retraction_distances_when_ec": ["nil", "5"]
        })),
        Dialect::Orca242,
    );
    assert_eq!(r.report.count(Disposition::Rejected), 0);
    assert_eq!(
        r.config.extra("nozzle_temperature_initial_layer"),
        Some("220,235")
    );
    let mut ctx = mica_gcode::templates::config_context(&r.config);
    let result = mica_template::process(
        "{nozzle_temperature_initial_layer[1] - 90}|{if enable_pressure_advance[0]}{pressure_advance[1]*2}{endif}|{initial_layer_travel_acceleration}|{nozzle_diameter[1]*2}|{curr_bed_type}|{filament_start_gcode[1]}",
        &mut ctx,
    ).unwrap();
    assert_eq!(result, "145|0.06|5000|1.2|High Temp Plate|M117 world");
}

#[test]
fn compatibility_checks_lists_conditions_and_user_machine_parent() {
    let (t, _) = vendor();
    t.write("Test/process/process.json", json!({"name":"Process","instantiation":"true","compatible_printers":["Other"],"compatible_printers_condition":"true"}));
    let b = Bundle::load_vendor(t.0.join("Test.json"), Dialect::Orca242).unwrap();
    assert!(
        !b.is_compatible_with_printer(Kind::Process, "Process", "Printer")
            .unwrap()
    );
    assert!(
        b.full_config("Printer", "Process", &["PLA", "PLA"])
            .unwrap_err()
            .to_string()
            .contains("not compatible")
    );
    t.write("Test/process/process.json", json!({"name":"Process","instantiation":"true","compatible_printers_condition":"printer_preset == \"Printer\" and num_extruders == 2 and nozzle_diameter[1] == 0.4"}));
    t.write("Test/filament/pla.json", json!({"name":"PLA","instantiation":"true","compatible_prints_condition":"layer_height < 0.1"}));
    let b = Bundle::load_vendor(t.0.join("Test.json"), Dialect::Orca242).unwrap();
    assert!(
        b.is_compatible_with_printer(Kind::Process, "Process", "Printer")
            .unwrap()
    );
    assert!(!b.is_compatible_with_print("PLA", "Process").unwrap());
    assert!(
        b.full_config("Printer", "Process", &["PLA", "PLA"])
            .is_err()
    );
    t.write(
        "Test/process/process.json",
        json!({"name":"Process","instantiation":"true","compatible_printers":["Printer"]}),
    );
    t.write("Test/filament/pla.json", json!({"name":"PLA","instantiation":"true","compatible_prints_condition":"invalid syntax ("}));
    t.write(
        "user/default/machine/copy.json",
        json!({"name":"Copy","inherits":"Printer"}),
    );
    let mut b = Bundle::load_vendor(t.0.join("Test.json"), Dialect::Orca242).unwrap();
    b.load_user_presets(t.0.join("user/default")).unwrap();
    assert!(
        b.is_compatible_with_printer(Kind::Process, "Process", "Copy")
            .unwrap()
    );
    assert!(b.is_compatible_with_print("PLA", "Process").unwrap());
    assert!(b.full_config("Copy", "Process", &["PLA", "PLA"]).is_ok());
}

#[test]
fn arc_fitting_boolean_maps_to_center_mode() {
    for dialect in [Dialect::Orca242, Dialect::Snapmaker240] {
        for (value, expected) in [("0", "disabled"), ("1", "emit_center")] {
            let input = raw(json!({"enable_arc_fitting":value}));
            let mapped = map_config(&input, dialect);
            assert_eq!(mapped.config.string("arc_fitting"), Some(expected));
            assert_eq!(mapped.report.count(Disposition::Rejected), 0);
        }
    }
}

#[test]
fn u1_crosshatch_has_an_engine_pattern() {
    let r = map_config(
        &raw(json!({"sparse_infill_pattern":"crosshatch"})),
        Dialect::Orca242,
    );
    assert_eq!(r.config.string("fill_pattern"), Some("crosshatch"));
    assert_eq!(
        mica_core::fill::Pattern::from_config_name("crosshatch"),
        Some(mica_core::fill::Pattern::CrossHatch)
    );
}

#[test]
fn batch_b_source_options_reach_engine_without_enum_loss() {
    use mica_core::{
        bridge::ExtraBridgeLayer,
        settings::{FloatOrPercent, PrintSettings},
    };
    let input = raw(
        json!({"wall_sequence":"inner-outer-inner wall", "overhang_reverse":"1", "overhang_reverse_internal_only":"1", "overhang_reverse_threshold":"30%", "enable_extra_bridge_layer":"apply_to_all", "internal_bridge_angle":"32", "bridge_density":"80%", "internal_bridge_density":"85%", "seam_slope_type":"all", "seam_slope_conditional":"1", "scarf_angle_threshold":"160", "scarf_overhang_threshold":"35%", "scarf_joint_speed":"42", "scarf_joint_flow_ratio":"0.92", "seam_slope_start_height":"10%", "seam_slope_steps":"12"}),
    );
    let mapped = map_config(&input, Dialect::Orca242);
    assert_eq!(mapped.report.count(Disposition::Rejected), 0);
    assert_eq!(mapped.report.count(Disposition::ExtrasOnly), 0);
    let s = PrintSettings::from_config(&mapped.config).unwrap();
    assert!(
        s.orca_walls.inner_outer_inner
            && s.orca_walls.overhang_reverse
            && s.orca_walls.internal_only
    );
    assert_eq!(s.orca_walls.threshold, FloatOrPercent::Percent(30.));
    assert_eq!(s.extra_bridge_layer, ExtraBridgeLayer::All);
    assert_eq!(s.internal_bridge_angle, 32.);
    assert_eq!(s.bridge_density, 0.8);
    assert_eq!(s.internal_bridge_density, 0.85);
    assert!(s.seam.scarf.conditional);
    assert_eq!(s.seam.scarf.speed, FloatOrPercent::Mm(42.));
    assert_eq!(s.seam.scarf.flow_ratio, 0.92);
    assert_eq!(s.seam.scarf.start_height, 0.1);
    assert_eq!(s.seam.scarf.steps, 12);
    assert_eq!(mapped.orca, input);
}

#[test]
fn imported_bridge_density_keeps_orca_policy_when_it_is_not_a_cli_override() {
    use mica_core::{bridge::BridgeStyle, settings::PrintSettings};
    let mapped = map_config(
        &raw(json!({"bridge_density":"80%","internal_bridge_density":"85%"})),
        Dialect::Orca242,
    );
    let settings = PrintSettings::from_config(&mapped.config).unwrap();
    assert_eq!(settings.bridge_style, BridgeStyle::Orca);
    assert_eq!(settings.bridge_density, 0.8);
    assert_eq!(settings.internal_bridge_density, 0.85);
}
