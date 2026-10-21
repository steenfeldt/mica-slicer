// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

use std::path::{Path, PathBuf};

use mica_core::{config::Config, mesh::TriangleMesh, print::slice_object, settings::PrintSettings};
use mica_profiles::orca::{Bundle, Dialect, Kind};

const PRINTER: &str = "Snapmaker U1 (0.4 nozzle)";
const PROCESS: &str = "0.20 Standard @Snapmaker U1 (0.4 nozzle)";
const FILAMENT: &str = "Snapmaker PLA @U1";
const BED: &str = "High Temp Plate";
const APP: &str = "/Applications/OrcaSlicer.app/Contents/MacOS/OrcaSlicer";

/// The pinned OrcaSlicer 2.4.2 profiles; `None` (a reported skip) when they are not fetched.
fn pinned() -> Option<Bundle> {
    mica_testkit::require_orca_profiles().then(|| bundle(&mica_testkit::orca_profiles()))
}

fn bundle(root: &Path) -> Bundle {
    let mut b =
        Bundle::load_vendor(root.join("OrcaFilamentLibrary.json"), Dialect::Orca242).unwrap();
    b.add_vendor(root.join("Snapmaker.json")).unwrap();
    b
}

fn model() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/models/cube20.stl")
}

fn export(config: &Config) -> String {
    let s = PrintSettings::from_config(config).unwrap();
    let mut mesh = TriangleMesh::load_stl(model()).unwrap();
    mesh.place_on_bed(135.5, 136.0);
    let obj = slice_object(&mesh, &s).unwrap();
    assert_eq!(obj.layers.len(), 100);
    assert!((obj.layers[99].print_z - 20.05).abs() < 1e-8);
    let result =
        mica_gcode::export::export_gcode_with_mesh(&obj, config, &s, "cube20.stl", &mesh).unwrap();
    assert!(result.stats.time_s.is_finite());
    assert!(!result.gcode.lines().any(|s| s.ends_with(" F0")));
    result.gcode
}

fn lines(s: &str) -> Vec<&str> {
    s.lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with("M73 "))
        .collect()
}

#[test]
fn u1_custom_gcode_matches_orcaslicer() {
    let Some(b) = pinned() else {
        return;
    };
    let config = b
        .single_tool_config(PRINTER, PROCESS, FILAMENT, Some(BED))
        .unwrap();
    let g = export(&config);
    let headers: Vec<_> = g
        .lines()
        .filter(|l| l.starts_with("; thumbnail begin "))
        .map(|l| l.split_whitespace().nth(3).unwrap())
        .collect();
    assert_eq!(headers, ["48x48", "300x300"]);
    assert_eq!(g.matches("; THUMBNAIL_BLOCK_START").count(), 2);
    assert_eq!(g.matches("; THUMBNAIL_BLOCK_END").count(), 2);
    assert!(g.find("; HEADER_BLOCK_END").unwrap() < g.find("; THUMBNAIL_BLOCK_START").unwrap());
    assert!(
        g.rfind("; THUMBNAIL_BLOCK_END").unwrap()
            < g.find("; external perimeters extrusion width").unwrap()
    );
    let fixture = include_str!("fixtures/u1_cube20_orcaslicer.gcode");
    let (_, fixture) = fixture.split_once("; ---- start ----\n").unwrap();
    let (start, end) = fixture.split_once("; ---- end ----\n").unwrap();
    assert_eq!(
        lines(&g[g.find(";TYPE:Custom").unwrap()..g.find(";LAYER_CHANGE").unwrap()]),
        lines(start)
    );
    let end_at = g.rfind(";TYPE:Custom").unwrap() + ";TYPE:Custom\n".len();
    assert_eq!(
        lines(&g[end_at..g.find("; EXECUTABLE_BLOCK_END").unwrap()]),
        lines(end)
    );
    assert_eq!(
        g.matches("EXCLUDE_OBJECT_START NAME=cube20.stl_id_0_copy_0")
            .count(),
        100
    );
    assert_eq!(
        g.matches("EXCLUDE_OBJECT_END NAME=cube20.stl_id_0_copy_0")
            .count(),
        100
    );
    assert!(g.contains("POLYGON=[[125.5,126],[145.5,126],[145.5,146],[125.5,146],[125.5,126]]"));
    let layers = &g[g.find(";LAYER_CHANGE").unwrap()..end_at];
    assert!(!layers.contains("M204"));
    assert!(!g.contains("M486") && !g.contains("M201") && !g.contains("M205"));
    assert!(!g.contains("SQUARE_CORNER_VELOCITY")); // default_jerk=0 disables all role overrides
    assert!(g.contains("SET_VELOCITY_LIMIT ACCEL=500 ACCEL_TO_DECEL=250"));
    assert!(g.contains("SET_VELOCITY_LIMIT ACCEL=10000 ACCEL_TO_DECEL=5000"));
    assert!(g.contains("M106 S255") && g.contains("M106 P2 S178"));
    for i in 1..=100 {
        assert!(layers.contains(&format!("SET_PRINT_STATS_INFO CURRENT_LAYER={i}\n")));
    }
}

#[test]
fn u1_labels_and_pressure_advance_respect_profile_switches() {
    let Some(b) = pinned() else {
        return;
    };
    let mut config = b
        .single_tool_config(PRINTER, PROCESS, FILAMENT, Some(BED))
        .unwrap();
    use mica_core::config::{ConfigValue, Value};
    // One Label objects setting: off means no EXCLUDE_OBJECT on Klipper either.
    config.set_raw("gcode_label_objects", "disabled").unwrap();
    config.set(
        "enable_pressure_advance",
        ConfigValue::Vector(vec![Some(Value::Bool(false)); 4]),
    );
    let g = export(&config);
    assert!(!g.contains("EXCLUDE_OBJECT"));
    assert!(!g.contains("SET_PRESSURE_ADVANCE"));
    assert!(!g.contains("M486"));
}

/// Live oracle: all commands before the first layer and the custom end, plus each layer's macros.
/// Needs the installed OrcaSlicer 2.4.1 app (or a saved reference). Everything Orca writes stays
/// in the system temp dir.
#[test]
#[ignore = "needs OrcaSlicer 2.4.1 in /Applications (or MICA_U1_ORCA_REFERENCE): cargo test -- --ignored"]
fn u1_live_orcaslicer_start_end_and_layer_structure() {
    assert!(
        Path::new(APP).is_file(),
        "{APP} is missing: the live U1 oracle needs OrcaSlicer 2.4.1"
    );
    let root = Path::new("/Applications/OrcaSlicer.app/Contents/Resources/profiles");
    assert!(
        root.join("Snapmaker.json").is_file(),
        "{} has no Snapmaker profiles",
        root.display()
    );
    let b = bundle(root);
    // Reuse a saved successful oracle when another application launch is forbidden or unavailable.
    // The same startup, end, layer and timing assertions still run below.
    let reference = if let Ok(path) = std::env::var("MICA_U1_ORCA_REFERENCE") {
        let reference = std::fs::read_to_string(path).unwrap();
        assert!(reference.contains("OrcaSlicer 2.4.1"));
        reference
    } else {
        let version = std::process::Command::new(APP)
            .arg("--help")
            .output()
            .unwrap();
        assert!(
            String::from_utf8_lossy(&version.stdout).starts_with("OrcaSlicer-2.4.1:"),
            "the live U1 oracle needs OrcaSlicer 2.4.1 (or MICA_U1_ORCA_REFERENCE)"
        );
        let temp = std::env::temp_dir().join(format!("mica-u1-live-oracle-{}", std::process::id()));
        std::fs::create_dir_all(&temp).unwrap();
        // Flatten the installed preset files, retaining only overrides (the application supplies defaults).
        // Loading pinned 2.4.2 defaults into 2.4.1 would hide actual version differences.
        for (kind, name, file) in [
            (Kind::Machine, PRINTER, "machine"),
            (Kind::Process, PROCESS, "process"),
            (Kind::Filament, FILAMENT, "filament"),
        ] {
            fn overrides(b: &Bundle, kind: Kind, name: &str) -> mica_profiles::orca::RawConfig {
                let p = b.preset(kind, name).unwrap();
                let mut raw = p
                    .inherits
                    .as_deref()
                    .map(|parent| overrides(b, kind, parent))
                    .unwrap_or_default();
                raw.extend(p.values.clone());
                raw
            }
            let mut raw = overrides(&b, kind, name);
            raw.insert("name".into(), name.into());
            raw.insert("type".into(), file.into());
            raw.insert("from".into(), "system".into());
            raw.insert("instantiation".into(), "true".into());
            if kind == Kind::Machine {
                raw.insert("curr_bed_type".into(), BED.into());
            }
            std::fs::write(
                temp.join(format!("{file}.json")),
                serde_json::to_vec_pretty(&raw).unwrap(),
            )
            .unwrap();
        }
        let filament = temp.join("filament.json").to_string_lossy().into_owned();
        let output = std::process::Command::new(APP)
            .arg("--datadir")
            .arg(temp.join("data"))
            .arg("--load-settings")
            .arg(format!(
                "{};{}",
                temp.join("machine.json").display(),
                temp.join("process.json").display()
            ))
            .arg("--load-filaments")
            .arg([filament.as_str(); 4].join(";"))
            .args(["--slice", "0", "--arrange", "1", "--outputdir"])
            .arg(&temp)
            .arg(model())
            .current_dir(&temp)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "Orca: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let reference = std::fs::read_to_string(temp.join("plate_1.gcode")).unwrap();
        std::fs::remove_dir_all(temp).unwrap();
        reference
    };
    let config = b
        .single_tool_config(PRINTER, PROCESS, FILAMENT, Some(BED))
        .unwrap();
    let estimated =
        mica_gcode::time_estimate::estimate(&mut std::io::Cursor::new(&reference), &config)
            .unwrap();
    // OrcaSlicer 2.4.1, same U1 / Standard / Snapmaker PLA / High Temp Plate cube.
    assert!(reference.contains("; estimated printing time (normal mode) = 15m 8s"));
    assert!(
        (estimated.normal / 908.0 - 1.0).abs() < 0.01,
        "{estimated:?}"
    );
    assert_eq!(estimated.silent, None);
    eprintln!(
        "U1 Orca 2.4.1: 908s; Mica estimator on Orca text: {:.3}s",
        estimated.normal
    );
    let g = export(&config);
    fn startup(g: &str) -> Vec<&str> {
        let mut thumbnail = false;
        lines(&g[..g.find(";LAYER_CHANGE").unwrap()])
            .into_iter()
            // Orca's STL CLI has no thumbnail callback. Compare commands and other metadata;
            // Mica's thumbnail payloads and profile placement have separate regressions.
            .filter(|l| {
                if *l == "; THUMBNAIL_BLOCK_START" {
                    thumbnail = true;
                    return false;
                }
                if *l == "; THUMBNAIL_BLOCK_END" {
                    thumbnail = false;
                    return false;
                }
                !thumbnail && !l.starts_with("; generated by ")
            })
            .collect()
    }
    assert_eq!(startup(&g), startup(&reference));
    fn shutdown(g: &str) -> Vec<&str> {
        lines(&g[g.rfind(";TYPE:Custom\n").unwrap()..g.find("; EXECUTABLE_BLOCK_END").unwrap()])
    }
    assert_eq!(shutdown(&g), shutdown(&reference));
    fn structure(g: &str) -> Vec<&str> {
        lines(g)
            .into_iter()
            .filter(|l| {
                l.starts_with(";LAYER_CHANGE")
                    || l.starts_with(";Z:")
                    || l.starts_with(";BEFORE_LAYER_CHANGE")
                    || l.starts_with(";AFTER_LAYER_CHANGE")
                    || l.starts_with("SET_PRINT_STATS_INFO")
                    || *l == "TIMELAPSE_TAKE_FRAME"
                    || *l == "DEFECT_DETECTION_DETECT"
                    || l.starts_with("EXCLUDE_OBJECT_")
            })
            .collect()
    }
    assert_eq!(structure(&g), structure(&reference));
    // Orca stores layer height in float32 and emits extra HEIGHT markers for thick infill.
    for (a, b) in g
        .split(";LAYER_CHANGE\n")
        .skip(1)
        .zip(reference.split(";LAYER_CHANGE\n").skip(1))
    {
        let height = |s: &str| {
            s.lines()
                .nth(1)
                .unwrap()
                .strip_prefix(";HEIGHT:")
                .unwrap()
                .parse::<f64>()
                .unwrap()
        };
        assert!((height(a) - height(b)).abs() < 0.00001);
    }
}

#[test]
fn u1_all_six_template_calls_have_runtime_values_and_current_tool_index() {
    let Some(b) = pinned() else {
        return;
    };
    use mica_core::config::{ConfigValue, Value};
    let mut c = b
        .single_tool_config(PRINTER, PROCESS, FILAMENT, Some(BED))
        .unwrap();
    let mut set =
        |key: &str, value: &str| c.set(key, ConfigValue::Scalar(Value::String(value.into())));
    set(
        "start_gcode",
        ";MS {nozzle_temperature_initial_layer[0]-90} {bed_temperature_initial_layer_single} {initial_extruder} {curr_bed_type} {is_extruder_used[3]}\n;MESH {adaptive_bed_mesh_min[0]} {adaptive_bed_mesh_max[1]} {bed_mesh_probe_count[0]} {bed_mesh_algo}",
    );
    set(
        "before_layer_gcode",
        ";BL {layer_num} {layer_z} {max_layer_z}",
    );
    set(
        "layer_gcode",
        ";AL {layer_num+1} {layer_z} {most_used_physical_extruder_id}",
    );
    set(
        "end_gcode",
        ";ME {layer_num} {max_layer_z} [nozzle_temperature]\n;EST {print_time_sec} {used_filament_length}",
    );
    c.set(
        "nozzle_temperature",
        ConfigValue::Vector([220, 230, 240, 250].map(|v| Some(Value::Int(v))).into()),
    );
    c.set(
        "start_filament_gcode",
        ConfigValue::Vector(vec![
            Some(Value::String(
                ";FS {layer_num} {filament_extruder_id} [nozzle_temperature]".into()
            ));
            4
        ]),
    );
    c.set(
        "end_filament_gcode",
        ConfigValue::Vector(vec![
            Some(Value::String(
                ";FE {layer_num} {filament_extruder_id} [nozzle_temperature]".into()
            ));
            4
        ]),
    );
    let g = export(&c);
    for line in [
        ";MS 130 55 0 High Temp Plate false",
        ";MESH 123.5 148 4 bicubic",
        ";FS -1 0 220",
        ";BL 0 0.25 0.25",
        ";AL 1 0.25 0",
        ";FE 99 0 220",
        ";FE 99 1 230",
        ";FE 99 2 240",
        ";FE 99 3 250",
        ";ME 99 20.05 220",
    ] {
        assert!(g.lines().any(|l| l == line), "missing {line}");
    }
    assert!(!g.contains("__MICA_"));
}

#[test]
#[ignore = "needs a saved OrcaSlicer U1 Benchy: set MICA_U1_BENCHY_REFERENCE, cargo test -- --ignored"]
fn saved_benchy_motion_estimator() {
    let path = std::env::var("MICA_U1_BENCHY_REFERENCE")
        .expect("set MICA_U1_BENCHY_REFERENCE to the saved OrcaSlicer U1 Benchy G-code");
    let Some(b) = pinned() else {
        return;
    };
    let config = b
        .single_tool_config(PRINTER, PROCESS, FILAMENT, Some(BED))
        .unwrap();
    let reference = std::fs::read_to_string(path).unwrap();
    let estimated =
        mica_gcode::time_estimate::estimate(&mut std::io::Cursor::new(&reference), &config)
            .unwrap();
    assert!(reference.contains("; estimated printing time (normal mode) = 40m 11s"));
    eprintln!(
        "Saved Orca Benchy 2411s, Mica estimator {:.3}s",
        estimated.normal
    );
    assert!(
        (estimated.normal / 2411. - 1.).abs() < 0.01,
        "{estimated:?}"
    );
}

#[test]
fn u1_round_two_settings_and_overrides() {
    use mica_core::settings::FloatOrPercent;
    let Some(b) = pinned() else {
        return;
    };
    let mut c = b
        .single_tool_config(PRINTER, PROCESS, FILAMENT, Some(BED))
        .unwrap();
    let s = PrintSettings::from_config(&c).unwrap();
    assert!(s.precise_outer_wall && s.detect_narrow_internal_solid_infill);
    assert_eq!(
        s.top_bottom_infill_overlap,
        Some(FloatOrPercent::Percent(25.))
    );
    assert_eq!(c.float("travel_slope"), Some(3.));
    assert_eq!(c.string("z_hop_types"), Some("Auto Lift"));
    c.set_raw("precise_outer_wall", "0").unwrap();
    c.set_raw("detect_narrow_internal_solid_infill", "0")
        .unwrap();
    c.set_raw("top_bottom_infill_wall_overlap", "20%").unwrap();
    let s = PrintSettings::from_config(&c).unwrap();
    assert!(!s.precise_outer_wall && !s.detect_narrow_internal_solid_infill);
    assert_eq!(
        s.top_bottom_infill_overlap,
        Some(FloatOrPercent::Percent(20.))
    );
    let prusa = PrintSettings::from_config(&Config::defaults()).unwrap();
    assert!(!prusa.precise_outer_wall && !prusa.detect_narrow_internal_solid_infill);
    assert_eq!(prusa.top_bottom_infill_overlap, None);
}
