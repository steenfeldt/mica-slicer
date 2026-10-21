// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 tests/fff_print/test_cooling.cpp :: Cooling integration tests
// Changes: Actual Mica cube export checks nominal layer duration and per-tool material minima; role-speed regression for the CORE One interior layers.
use mica_core::{
    config::Config,
    mesh::TriangleMesh,
    print::prepare_object,
    print_job::{PreparedPrint, PrintObject, ToolAssignments},
    settings::PrintSettings,
};
use mica_gcode::export::export_gcode;

fn cube(x: f32, height: f32) -> TriangleMesh {
    let mut mesh = TriangleMesh::load_stl(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/models/cube20.stl"
    ))
    .unwrap();
    for v in &mut mesh.vertices {
        v[2] *= height / 20.;
    }
    mesh.place_on_bed(x, 100.);
    mesh
}
fn config() -> Config {
    let mut c = Config::defaults();
    for (key, value) in [
        ("gcode_flavor", "marlin2"),
        ("start_gcode", ""),
        ("end_gcode", ""),
        ("layer_height", "0.2"),
        ("first_layer_height", "0.2"),
        ("thumbnails", ""),
        ("cooling", "1"),
        ("slowdown_below_layer_time", "10"),
        ("min_print_speed", "0"),
        ("first_layer_speed", "100%"),
        ("external_perimeter_speed", "99"),
        ("fan_below_layer_time", "0"),
        ("arc_fitting", "disabled"),
        ("skirts", "0"),
        ("bed_shape", "0x0,250x0,250x210,0x210"),
        ("use_relative_e_distances", "1"),
    ] {
        c.set_raw(key, value).unwrap();
    }
    c
}
#[derive(Default)]
struct Layer {
    time: f64,
    moves: Vec<(usize, String, f64, f64)>,
}
fn read(gcode: &str) -> Vec<Layer> {
    let (mut x, mut y, mut z, mut f) = (0_f64, 0_f64, 0_f64, 1_f64);
    let mut tool = 0;
    let mut role = String::new();
    let mut layers = Vec::<Layer>::new();
    for line in gcode.lines() {
        if line.starts_with(";LAYER_CHANGE") {
            layers.push(Layer::default());
        }
        if let Some(r) = line.strip_prefix(";TYPE:") {
            role = r.into();
        }
        let mut words = line.split(';').next().unwrap().split_whitespace();
        let Some(cmd) = words.next() else { continue };
        if let Some(t) = cmd.strip_prefix('T').and_then(|s| s.parse::<usize>().ok()) {
            tool = t;
        }
        if cmd != "G1" && cmd != "G0" {
            continue;
        }
        let (mut nx, mut ny, mut nz, mut de) = (x, y, z, 0_f64);
        for word in words {
            let Ok(v) = word[1..].parse::<f64>() else {
                continue;
            };
            match word.as_bytes()[0] {
                b'X' => nx = v,
                b'Y' => ny = v,
                b'Z' => nz = v,
                b'E' => de = v,
                b'F' => f = v / 60.,
                _ => {}
            }
        }
        let xy = (nx - x).hypot(ny - y);
        let xyz = xy.hypot(nz - z);
        if let Some(layer) = layers.last_mut() {
            layer.time += if xyz > 0. { xyz / f } else { de.abs() / f };
            if de > 0. && xy > 0. {
                layer.moves.push((tool, role.clone(), f, xy));
            }
        }
        (x, y, z) = (nx, ny, nz);
    }
    layers
}
#[test]
fn upstream_cube_minimum_layer_time_is_honored_with_zero_minimum_speed() {
    let c = config();
    let settings = PrintSettings::from_config(&c).unwrap();
    let object = prepare_object(&cube(125., 20.), &settings).unwrap();
    let output = export_gcode(&object, &c, &settings, "cube").unwrap();
    let layers = read(&output.gcode);
    assert_eq!(layers.len(), 100);
    for (i, layer) in layers.iter().enumerate() {
        assert!(layer.time > 9.5, "layer {i}: {}", layer.time);
    }
}
#[test]
fn nonconsecutive_tools_use_their_own_cooling_settings_in_export() {
    let mut c = config();
    for (k, v) in [
        ("nozzle_diameter", "0.4,0.4,0.4"),
        ("cooling", "1,1,1"),
        ("min_print_speed", "40,1,15"),
        ("slowdown_below_layer_time", "1000,1,1000"),
        ("perimeter_speed", "100"),
        ("external_perimeter_speed", "100"),
        ("solid_infill_speed", "100"),
        ("top_solid_infill_speed", "100"),
        ("small_perimeter_speed", "100"),
        ("fill_density", "0%"),
        ("filament_max_volumetric_speed", "0,0,0"),
        ("wipe_tower", "0"),
        ("toolchange_gcode", "T{next_extruder}"),
    ] {
        c.set_raw(k, v).unwrap();
    }
    let objects = [0, 2]
        .into_iter()
        .enumerate()
        .map(|(i, t)| {
            PrintObject::prepare(
                format!("cube{i}"),
                i,
                0,
                cube(80. + i as f32 * 60., 0.6),
                ToolAssignments([Some(t), Some(t), Some(t), None, None]),
                &c,
            )
            .unwrap()
        })
        .collect();
    let job = PreparedPrint::new(objects, &c).unwrap();
    let output = export_gcode(&job, &c, &PrintSettings::from_config(&c).unwrap(), "tools").unwrap();
    let layers = read(&output.gcode);
    for layer in &layers[1..] {
        for t in [0, 2] {
            let moves: Vec<_> = layer.moves.iter().filter(|m| m.0 == t).collect();
            assert!(!moves.is_empty());
            for m in moves {
                assert!((m.2 - if t == 0 { 40. } else { 15. }).abs() < 0.02, "{m:?}");
            }
        }
    }
}
#[test]
fn core_one_sparse_cube_layers_keep_both_walls_at_full_speed() {
    let mut c = Config::defaults();
    for l in include_str!("../../mica-profiles/tests/fixtures/coreone_hf04_speed_pla.ini").lines() {
        if let Some((k, v)) = l.split_once(" = ") {
            c.set_raw(k, v).unwrap();
        }
    }
    c.set_raw("arc_fitting", "disabled").unwrap();
    c.set_raw("thumbnails", "").unwrap();
    let settings = PrintSettings::from_config(&c).unwrap();
    let output = export_gcode(
        &prepare_object(&cube(125., 20.), &settings).unwrap(),
        &c,
        &settings,
        "cube",
    )
    .unwrap();
    for (i, layer) in read(&output.gcode).iter().enumerate().take(94).skip(3) {
        let mut infill = false;
        for m in &layer.moves {
            let speed = match m.1.as_str() {
                "External perimeter" => 200.,
                "Perimeter" => 250.,
                "Internal infill" => {
                    infill = true;
                    if m.2 > 100. { 200. } else { 21. }
                }
                _ => continue,
            };
            assert!((m.2 - speed).abs() < 1., "layer {i}: {m:?}");
        }
        assert!(infill);
    }
}
