// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

use mica_core::config::Config;
use mica_core::mesh::TriangleMesh;
use mica_core::print::LayerSource;
use mica_core::print_job::{PreparedPrint, PrintObject, ToolAssignments};
use mica_core::settings::PrintSettings;
use mica_core::threemf::Settings;
use mica_gcode::export::export_gcode;

fn config() -> Config {
    let mut c = Config::defaults();
    for (k, v) in [
        ("nozzle_diameter", "0.4,0.4,0.4"),
        ("first_layer_speed", "30"),
        ("layer_height", "0.2"),
        ("first_layer_height", "0.2"),
        ("support_material_extruder", "0"),
        ("perimeter_generator", "classic"),
        ("bed_shape", "0x0,250x0,250x210,0x210"),
        ("gcode_flavor", "marlin2"),
        ("use_relative_e_distances", "1"),
        ("first_layer_temperature", "220,230,240"),
        ("temperature", "200,210,215"),
        ("retract_length", "0.6,0.8,1"),
        ("retract_length_toolchange", "3,4,5"),
        ("retract_speed", "40,50,60"),
        ("gcode_label_objects", "firmware"),
        ("wipe_tower", "0"),
        (
            "start_gcode",
            "; used {is_extruder_used[0]} {is_extruder_used[1]} {is_extruder_used[2]} initial {initial_tool}",
        ),
        (
            "toolchange_gcode",
            "; switch {previous_extruder} {next_extruder}\nT{next_extruder}\n{position[2]=position[2]+2;}",
        ),
    ] {
        c.set_raw(k, v).unwrap();
    }
    c
}
fn object(
    c: &Config,
    id: usize,
    instance: usize,
    x: f32,
    tools: ToolAssignments,
    height: f32,
) -> PrintObject {
    let mut m = TriangleMesh::load_stl(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/models/cube20.stl"
    ))
    .unwrap();
    for v in &mut m.vertices {
        v[2] *= height / 20.;
    }
    m.place_on_bed(x, 80.);
    PrintObject::prepare(format!("cube {id}"), id, instance, m, tools, c).unwrap()
}
fn assigned(tool: usize) -> ToolAssignments {
    ToolAssignments([Some(tool), Some(tool), Some(tool), None, None])
}

#[test]
fn merged_z_schedule_is_batch_independent_and_keeps_instances() {
    let c = config();
    let p = PreparedPrint::new(
        vec![
            object(&c, 0, 0, 40., assigned(0), 0.6),
            object(&c, 0, 1, 75., assigned(2), 1.),
        ],
        &c,
    )
    .unwrap();
    assert_eq!(p.ordering.used, [0, 2]);
    assert_eq!(p.layer_count(), 5);
    let all = p.scheduled_toolpaths(0..5);
    for (i, a) in all.iter().enumerate() {
        let one = p.scheduled_toolpaths(i..i + 1).remove(0);
        assert_eq!(a.layer.paths, one.layer.paths);
        assert_eq!(a.assignments.len(), a.layer.paths.len());
        if i >= 3 {
            assert!(a.assignments.iter().all(|a| a.tool == 2));
        }
    }
    let g = export_gcode(&p, &c, &PrintSettings::from_config(&c).unwrap(), "job")
        .unwrap()
        .gcode;
    assert!(g.contains("; used true false true initial 0"));
    assert!(g.contains("M486 S1"));
    assert!(g.contains("M109 S240 T2"));
    assert!(g.contains("M109 S215 T2"));
    assert!(!g.lines().any(|l| l == "T1"));
    assert!(g.contains("; filament used T2 [External perimeter]"));
}

#[test]
fn role_tools_and_metadata_precedence_are_validated() {
    let mut c = config();
    c.set_raw("infill_extruder", "3").unwrap();
    c.set_raw("support_material_interface_extruder", "2")
        .unwrap();
    let object = Settings::from([("extruder".into(), "2".into())]);
    let part = Settings::from([
        ("extruder".into(), "3".into()),
        ("perimeter_extruder".into(), "1".into()),
    ]);
    let tools = ToolAssignments::resolve(&c, &object, &part, 0, 3).unwrap();
    assert_eq!(tools.0, [Some(0), Some(2), Some(2), None, Some(1)]);
    let bad = Settings::from([("extruder".into(), "4".into())]);
    assert!(ToolAssignments::resolve(&c, &bad, &Settings::new(), 0, 3).is_err());
}

#[test]
fn per_role_tool_offsets_and_no_labels() {
    let mut c = config();
    c.set_raw("extruder_offset", "0x0,2x3,0x0").unwrap();
    c.set_raw("gcode_label_objects", "disabled").unwrap();
    let p = PreparedPrint::new(
        vec![object(
            &c,
            0,
            0,
            50.,
            ToolAssignments([Some(0), Some(1), Some(1), None, None]),
            0.4,
        )],
        &c,
    )
    .unwrap();
    assert_eq!(p.ordering.used, [0, 1]);
    let g = export_gcode(&p, &c, &PrintSettings::from_config(&c).unwrap(), "roles")
        .unwrap()
        .gcode;
    assert!(g.contains("T1"));
    assert!(!g.contains("M486"));
    // The second tool's physical XY coordinates subtract its configured offset.
    let first = p.scheduled_toolpaths(0..1).remove(0);
    let path = first
        .layer
        .paths
        .iter()
        .zip(first.assignments)
        .find(|(_, a)| a.tool == 1)
        .unwrap()
        .0;
    let point = path.points[0];
    let xy = format!(
        "X{} Y{}",
        mica_gcode::writer::num(mica_core::geometry::unscale(point.x) - 2., 3),
        mica_gcode::writer::num(mica_core::geometry::unscale(point.y) - 3., 3)
    );
    assert!(g.contains(&xy), "missing {xy}");
}

#[test]
fn klipper_machine_change_time_excludes_initial_selection() {
    let mut c = config();
    c.set_raw("gcode_flavor", "klipper").unwrap();
    c.set_raw("machine_tool_change_time", "5").unwrap();
    let mut src = std::io::Cursor::new(b"M83\nT2\nT0\nT2\n");
    let estimate = mica_gcode::time_estimate::estimate(&mut src, &c).unwrap();
    assert_eq!(estimate.normal, 10.);
}

#[test]
fn binary_job_metadata_contains_each_instance() {
    let c = config();
    let p = PreparedPrint::new(
        vec![
            object(&c, 0, 0, 40., assigned(0), 0.4),
            object(&c, 0, 1, 75., assigned(2), 0.4),
        ],
        &c,
    )
    .unwrap();
    let path = std::env::temp_dir().join(format!("mica-m3-binary-{}.bgcode", std::process::id()));
    mica_gcode::export::export_file(
        &p,
        &c,
        &PrintSettings::from_config(&c).unwrap(),
        "job",
        None,
        &path,
    )
    .unwrap();
    let bytes = std::fs::read(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    let decoded = mica_gcode::bgcode::decode(&bytes).unwrap();
    assert!(decoded.gcode.contains("T2"));
    assert!(decoded.gcode.contains("M486 S1"));
    assert!(format!("{:?}", decoded.metadata.printer).contains("Instance 2"));
}

#[test]
fn nearly_equal_object_heights_merge_at_the_extreme_midpoint() {
    let c = config();
    let mut shifted = c.clone();
    shifted.set_raw("first_layer_height", "0.20005").unwrap();
    let p = PreparedPrint::new(
        vec![
            object(&c, 0, 0, 40., assigned(0), 0.6),
            object(&shifted, 1, 0, 75., assigned(2), 0.6),
        ],
        &c,
    )
    .unwrap();
    assert_eq!(p.layer_count(), 3);
    assert!((p.layer_z(0).0 - 0.200025).abs() < 1e-8);
    assert_eq!(p.ordering.layers[0].extruders, [0, 2]);
}

#[test]
fn region_speeds_and_physical_tool_settings_reach_export() {
    use mica_core::regions::{RegionModel, Volume};
    use mica_core::threemf::PartKind;
    let mut c = config();
    c.set_raw("slowdown_below_layer_time", "0").unwrap();
    c.set_raw("cooling", "0").unwrap();
    c.set_raw("skirts", "0").unwrap();
    let mesh = object(&c, 0, 0, 100., assigned(0), 1.).mesh;
    let mut override_config = Config::new();
    for (k, v) in [
        ("extruder", "3"),
        ("perimeter_speed", "12"),
        ("external_perimeter_speed", "12"),
        ("infill_speed", "12"),
        ("solid_infill_speed", "12"),
        ("top_solid_infill_speed", "12"),
    ] {
        override_config.set_raw(k, v).unwrap();
    }
    let model = RegionModel {
        full_spectrum: Default::default(),
        volumes: vec![
            Volume {
                painting: Default::default(),
                mesh: mesh.clone(),
                kind: PartKind::Model,
                config: Config::new(),
            },
            Volume {
                painting: Default::default(),
                mesh: mesh.clone(),
                kind: PartKind::Modifier,
                config: override_config,
            },
        ],
        height_ranges: vec![],
        layer_height_profile: vec![],
        brim_ears: vec![],
    };
    let object =
        PrintObject::prepare_regions("slow".into(), 0, 0, mesh, &model, assigned(0), &c).unwrap();
    let job = PreparedPrint::new(vec![object], &c).unwrap();
    assert_eq!(job.ordering.used, [2]);
    let g = export_gcode(&job, &c, &PrintSettings::from_config(&c).unwrap(), "region")
        .unwrap()
        .gcode;
    assert!(g.contains("M109 S240 T2"));
    assert!(
        g.lines().any(|l| l == "G1 F720"),
        "region speed was lost at export"
    );
    assert!(g.contains("E-1 F3600"), "physical tool retraction was lost");
}

fn two_tool_job(c: &Config) -> PreparedPrint {
    PreparedPrint::new(
        vec![
            object(c, 0, 0, 40., assigned(0), 1.),
            object(c, 1, 0, 110., assigned(1), 1.),
        ],
        c,
    )
    .unwrap()
}

fn layer_tools(p: &PreparedPrint) -> Vec<Vec<usize>> {
    p.ordering
        .layers
        .iter()
        .map(|l| l.extruders.clone())
        .collect()
}

#[test]
fn cyclic_toolchange_ordering_keeps_the_tool_order_on_every_layer() {
    // PrusaSlicer 2.9.6 (ToolOrdering::reorder_extruders): Optimized starts each layer with the
    // tool that ended the layer below, Cyclic prints 1, 2, ... on every layer. Two cubes on
    // tools 1 and 2 of the XL gave [[0,1],[1,0],[0,1],...] and [[0,1],[0,1],...] there.
    let mut c = config();
    let optimized = two_tool_job(&c);
    assert_eq!(
        layer_tools(&optimized),
        [vec![0, 1], vec![1, 0], vec![0, 1], vec![1, 0], vec![0, 1]]
    );
    c.set_raw("toolchange_ordering", "cyclic").unwrap();
    let cyclic = two_tool_job(&c);
    assert_eq!(layer_tools(&cyclic), vec![vec![0, 1]; 5]);
    assert_eq!(
        (optimized.ordering.changes, cyclic.ordering.changes),
        (5, 9)
    );
    let settings = PrintSettings::from_config(&c).unwrap();
    let tools = |p: &PreparedPrint| {
        export_gcode(p, &c, &settings, "order")
            .unwrap()
            .gcode
            .lines()
            .filter(|l| matches!(*l, "T0" | "T1"))
            .count()
    };
    assert!(tools(&cyclic) > tools(&optimized));
    // OrcaSlicer has no such option: its rules keep the optimized order.
    c.set_raw("mica_slicing_rules", "orcaslicer").unwrap();
    assert_eq!(layer_tools(&two_tool_job(&c)), layer_tools(&optimized));
}

/// Layer changes that retract between `;LAYER_CHANGE` and the layer's Z move.
fn layer_change_retractions(g: &str) -> usize {
    let mut count = 0;
    let mut in_change = false;
    for line in g.lines() {
        if line == ";LAYER_CHANGE" {
            in_change = true;
        } else if in_change && line.starts_with("G1 E-") {
            count += 1;
            in_change = false;
        } else if in_change && line.starts_with("G1 ") && line.contains('Z') {
            in_change = false;
        }
    }
    count
}

#[test]
fn retract_layer_change_off_retracts_only_when_the_move_needs_it() {
    // GCodeGenerator::change_layer: with retract_layer_change off, a rising layer change
    // retracts only if its travel is at least retract_before_travel long; the filament's
    // filament_retract_layer_change overrides the printer value.
    let slice = |printer: &str, filament: &str| {
        let mut c = config();
        c.set_raw("nozzle_diameter", "0.4").unwrap();
        c.set_raw("retract_layer_change", printer).unwrap();
        c.set_raw("filament_retract_layer_change", filament)
            .unwrap();
        let p = PreparedPrint::new(vec![object(&c, 0, 0, 60., assigned(0), 2.)], &c).unwrap();
        export_gcode(&p, &c, &PrintSettings::from_config(&c).unwrap(), "rlc")
            .unwrap()
            .gcode
    };
    let on = slice("1", "nil");
    let off = slice("0", "nil");
    let layers = on.lines().filter(|l| *l == ";LAYER_CHANGE").count();
    assert_eq!(layer_change_retractions(&on), layers);
    // Only the first layer change (no known position) retracts unconditionally.
    let fewer = layer_change_retractions(&off);
    assert!(fewer < layers, "{fewer} of {layers} layer changes retract");
    let program = |s: String| {
        s.split("; prusaslicer_config = begin")
            .next()
            .unwrap()
            .lines()
            .filter(|l| !l.starts_with("; generated by "))
            .collect::<Vec<_>>()
            .join("\n")
    };
    assert_eq!(
        program(slice("0", "1")),
        program(on),
        "the filament override turns it on"
    );
    assert_eq!(
        program(slice("1", "0")),
        program(off),
        "the filament override turns it off"
    );
}
