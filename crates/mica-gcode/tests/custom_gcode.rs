// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/GCode.cpp :: ProcessLayer::emit_custom_gcode_per_print_z
// Changes: Mica export checks of the per-layer custom G-code blocks (tags, order, the layer they land on, the
// Changes: tool a colour change picks, the stop countdown) against the behaviour verified with PrusaSlicer 2.9.6.

use mica_core::config::Config;
use mica_core::custom_gcode::{Info, Item, Kind, Mode, WithCustomGCode};
use mica_core::mesh::TriangleMesh;
use mica_core::print::{LayerSource, prepare_object};
use mica_core::print_job::{PreparedPrint, PrintObject, ToolAssignments};
use mica_core::settings::PrintSettings;
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

fn config(tools: usize) -> Config {
    let mut c = Config::defaults();
    let per = |v: &str| vec![v; tools].join(",");
    for (key, value) in [
        ("gcode_flavor", "marlin2".to_string()),
        ("start_gcode", String::new()),
        ("end_gcode", String::new()),
        ("layer_height", "0.2".into()),
        ("first_layer_height", "0.2".into()),
        ("thumbnails", String::new()),
        ("perimeter_generator", "classic".into()),
        ("bed_shape", "0x0,250x0,250x210,0x210".into()),
        ("nozzle_diameter", per("0.4")),
        ("retract_length", per("0.8")),
        ("retract_speed", per("35")),
        ("deretract_speed", per("25")),
        ("wipe_tower", "0".into()),
        ("use_relative_e_distances", "1".into()),
        ("remaining_times", "1".into()),
        (
            "color_change_gcode",
            "M600\nG1 E0.3 F1500 ; prime after color change".into(),
        ),
        ("pause_print_gcode", "M601".into()),
        ("template_custom_gcode", "; template at {layer_z}".into()),
        ("toolchange_gcode", "T{next_extruder}".into()),
    ] {
        c.set_raw(key, &value).unwrap();
    }
    c
}

fn info(items: Vec<Item>, mode: Mode) -> Info {
    let mut info = Info {
        mode,
        ..Info::default()
    };
    for item in items {
        info.put(item);
    }
    info
}

/// The lines of a layer block, from its `;Z:` marker to the next layer.
fn layer<'a>(gcode: &'a str, z: &str) -> Vec<&'a str> {
    gcode
        .lines()
        .skip_while(|l| *l != format!(";Z:{z}"))
        .skip(1)
        .take_while(|l| !l.starts_with(";LAYER_CHANGE"))
        .collect()
}

#[test]
fn single_extruder_colour_change_pause_and_custom_blocks() {
    let c = config(1);
    let s = PrintSettings::from_config(&c).unwrap();
    let object = prepare_object(&cube(100., 6.), &s).unwrap();
    let plain = export_gcode(&object, &c, &s, "cube").unwrap().gcode;
    let custom = info(
        vec![
            Item {
                color: "#E67E22".into(),
                ..Item::new(1.0, Kind::ColorChange, 1)
            },
            Item {
                extra: "Insert magnets".into(),
                ..Item::new(2.1, Kind::PausePrint, 1)
            },
            Item {
                extra: "M117 hello\nG4 S1".into(),
                ..Item::new(3.0, Kind::Custom, 1)
            },
            Item::new(4.0, Kind::Template, 1),
        ],
        Mode::SingleExtruder,
    );
    let source = WithCustomGCode {
        source: &object,
        info: &custom,
    };
    let g = export_gcode(&source, &c, &s, "cube").unwrap().gcode;

    // The colour change waits for the layer's first extrusion: after the travel and Z move, and
    // M600 primes the nozzle, so the layer's own unretract disappears.
    let l1 = layer(&g, "1");
    let at = l1
        .iter()
        .position(|l| *l == ";COLOR_CHANGE,T0,#E67E22")
        .expect("colour change on Z=1");
    assert_eq!(l1[at + 1], "M600");
    assert_eq!(l1[at + 2], "G1 E0.3 F1500 ; prime after color change");
    assert!(l1[..at].iter().any(|l| l.starts_with("G1 Z1 ")), "{l1:#?}");
    assert!(l1[at + 3].starts_with(";TYPE:"), "{l1:#?}");
    // Generated at the layer start, the colour change counts the nozzle as primed (upstream's
    // discarded unretract), so the travel to the first point retracts again.
    let retracts = |l: &[&str]| l.iter().filter(|l| l.starts_with("G1 E-.8")).count();
    let p1 = layer(&plain, "1");
    let first = p1.iter().position(|l| l.starts_with(";TYPE:")).unwrap();
    assert_eq!(retracts(&l1[..at]), retracts(&p1[..first]) + 1);

    // Between layers 2.0 and 2.2 the pause moves up to 2.2, before its first extrusion.
    let l22 = layer(&g, "2.2");
    let p = l22.iter().position(|l| *l == ";PAUSE_PRINT").unwrap();
    assert_eq!(
        &l22[p..p + 3],
        &[";PAUSE_PRINT", "M117 Insert magnets", "M601"]
    );
    assert!(!l22[..p].iter().any(|l| l.starts_with(";TYPE:")));
    assert!(!layer(&g, "2").contains(&";PAUSE_PRINT"));

    let l3 = layer(&g, "3");
    let p = l3.iter().position(|l| *l == ";CUSTOM_GCODE").unwrap();
    assert_eq!(&l3[p + 1..p + 3], &["M117 hello", "G4 S1"]);
    let l4 = layer(&g, "4");
    let p = l4.iter().position(|l| *l == ";CUSTOM_GCODE").unwrap();
    assert_eq!(l4[p + 1], "; template at 4");

    // Remaining time to the next stop: whole minutes, then the last minute as a fraction (as
    // upstream, a stop less than a minute after the previous one gets no new countdown).
    let stops: Vec<_> = g.lines().filter(|l| l.starts_with("M73 C")).collect();
    assert!(stops.len() >= 2, "{stops:?}");
    assert!(stops.iter().any(|l| l.starts_with("M73 C0.")), "{stops:?}");
    assert!(!plain.contains("M73 C"));
    // Layers without items print the same moves.
    let moves = |l: Vec<&str>| -> Vec<String> {
        l.into_iter()
            .filter(|l| !l.starts_with("M73"))
            .map(str::to_string)
            .collect()
    };
    assert_eq!(moves(layer(&g, "0.6")), moves(layer(&plain, "0.6")));
}

fn tool_object(c: &Config, id: usize, x: f32, tool: usize) -> PrintObject {
    PrintObject::prepare(
        format!("cube {id}"),
        id,
        0,
        cube(x, 3.),
        ToolAssignments([Some(tool), Some(tool), Some(tool), None, None]),
        c,
    )
    .unwrap()
}

#[test]
fn multi_tool_colour_change_picks_its_tool_first() {
    let c = config(2);
    let s = PrintSettings::from_config(&c).unwrap();
    let objects = || vec![tool_object(&c, 0, 60., 0), tool_object(&c, 1, 120., 1)];
    let plain = PreparedPrint::new(objects(), &c).unwrap();
    // Without the change, layer 1.0 keeps the last tool of layer 0.8 first.
    let li = (0..plain.layer_count())
        .find(|&i| (plain.layer_z(i).0 - 1.0).abs() < 1e-6)
        .unwrap();
    let last = *plain.ordering.layers[li - 1].extruders.last().unwrap();
    assert_eq!(plain.ordering.layers[li].extruders[0], last);
    let other = 1 - last;
    let custom = info(
        vec![Item {
            color: "#00FF00".into(),
            ..Item::new(1.0, Kind::ColorChange, other + 1)
        }],
        Mode::MultiExtruder,
    );
    let p = PreparedPrint::with_custom_gcode(objects(), &c, custom).unwrap();
    assert_eq!(p.ordering.layers[li].extruders[0], other);
    assert_eq!(p.ordering.layers[li].color_change_tool, Some(other));
    let g = export_gcode(&p, &c, &s, "two").unwrap().gcode;
    let l = layer(&g, "1");
    let t = l.iter().position(|x| *x == format!("T{other}")).unwrap();
    let cc = l
        .iter()
        .position(|x| *x == format!(";COLOR_CHANGE,T{other},#00FF00"))
        .unwrap();
    assert!(t < cc && l[cc + 1] == "M600", "{l:#?}");
}

#[test]
fn tool_change_items_switch_a_single_tool_plate() {
    let c = config(2);
    let s = PrintSettings::from_config(&c).unwrap();
    let custom = info(
        vec![Item::new(1.6, Kind::ToolChange, 2)],
        Mode::MultiAsSingle,
    );
    let p =
        PreparedPrint::with_custom_gcode(vec![tool_object(&c, 0, 100., 0)], &c, custom).unwrap();
    for (i, l) in p.ordering.layers.iter().enumerate() {
        let z = p.layer_z(i).0;
        assert_eq!(l.extruders, vec![usize::from(z > 1.5)], "layer {i} at {z}");
    }
    let g = export_gcode(&p, &c, &s, "one").unwrap().gcode;
    assert!(layer(&g, "1.6").contains(&"T1"));
    assert!(!g.contains(";COLOR_CHANGE"));
}

/// Printing object by object, PrusaSlicer and Orca ignore per-layer custom G-code (their
/// `assign_custom_gcodes` runs for layer-by-layer prints only): Mica drops it with a warning.
#[test]
fn sequential_prints_ignore_layer_gcode_with_a_warning() {
    let mut c = config(1);
    c.set_raw("complete_objects", "1").unwrap();
    c.set_raw("extruder_clearance_radius", "20").unwrap();
    c.set_raw("extruder_clearance_height", "20").unwrap();
    let s = PrintSettings::from_config(&c).unwrap();
    let custom = info(
        vec![
            Item::new(1.0, Kind::ColorChange, 1),
            Item::new(2.0, Kind::PausePrint, 1),
        ],
        Mode::SingleExtruder,
    );
    let p = PreparedPrint::with_custom_gcode(
        vec![tool_object(&c, 0, 60., 0), tool_object(&c, 1, 140., 0)],
        &c,
        custom,
    )
    .unwrap();
    assert!(p.is_sequential());
    assert!(p.custom_gcode.is_empty());
    assert!(p.warnings.iter().any(|w| w.contains("one after another")));
    let g = export_gcode(&p, &c, &s, "two").unwrap().gcode;
    assert!(!g.contains(";COLOR_CHANGE") && !g.contains(";PAUSE_PRINT"));
}
