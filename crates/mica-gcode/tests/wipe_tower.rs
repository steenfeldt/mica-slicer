// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 tests/fff_print/test_multi.cpp :: Basic tests, Ooze prevention
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/WipeTowerIntegration.cpp :: append_tcr
// Source: prusaslicer@b028299c77 src/libslic3r/PrintApply.cpp :: print_config_diffs
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/WipeTower.cpp :: WipeTower::toolchange_Change
// Changes: Nonconsecutive tool and template tests extended to tower labels, offsets, sparse Z, priming, statistics and batch-independent geometry.
// Source: prusaslicer@b028299c77 src/libslic3r/GCode.cpp :: GCodeGenerator::set_extruder, OozePrevention::post_toolchange
// Changes: Tower-exit wipe and the no-tower toolchange wipe, ooze-prevention wait and unknown-position travel.
// Source: orca@8500fcdcca src/libslic3r/GCode.cpp :: GCode::set_extruder, WipeTowerIntegration::append_tcr2
// Changes: Orca toolchange sequence (wipe during the toolchange retraction, eager lift, ooze wait, tower Z restore and deretraction).
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/WipeTower.cpp :: WipeTower::tool_change, merge_tcr
// Changes: Prusa tower exit sequence: toolchange trailer before the joined finish_layer, single retract_length retraction plus wipe on leaving.

use mica_core::{
    config::Config,
    mesh::TriangleMesh,
    print::LayerSource,
    print_job::{PreparedPrint, PrintObject, ToolAssignments},
    settings::PrintSettings,
};
use mica_gcode::export::export_gcode;

fn config() -> Config {
    let mut c = Config::defaults();
    for (k, v) in [
        ("nozzle_diameter", "0.4,0.4,0.4,0.4,0.4"),
        ("bed_shape", "0x0,300x0,300x300,0x300"),
        ("wipe_tower", "1"),
        ("wipe_tower_x", "20"),
        ("wipe_tower_y", "160"),
        ("wipe_tower_width", "35"),
        ("wipe_tower_brim_width", "2"),
        ("wipe_tower_extra_spacing", "150%"),
        ("layer_height", "0.2"),
        ("first_layer_height", "0.2"),
        ("gcode_flavor", "marlin2"),
        ("use_relative_e_distances", "1"),
        ("single_extruder_multi_material_priming", "0"),
        ("perimeter_generator", "classic"),
        ("gcode_label_objects", "firmware"),
        ("retract_length_toolchange", "5,5,5,5,5"),
        ("filament_multitool_ramming", "1,1,1,1,1"),
        ("filament_multitool_ramming_flow", "40,40,40,40,40"),
        ("filament_multitool_ramming_volume", "10,10,10,10,10"),
        (
            "toolchange_gcode",
            "; CHANGE {previous_extruder} {next_extruder}\nT{next_extruder}\n{position[2]=position[2]+2;}",
        ),
        (
            "start_gcode",
            "; tower {has_wipe_tower} prime {has_single_extruder_multi_material_priming}\n; bounds {first_layer_print_min[0]} {first_layer_print_max[0]} {first_layer_print_max[1]}",
        ),
    ] {
        c.set_raw(k, v).unwrap();
    }
    c
}
fn object(c: &Config, tool: usize, id: usize, h: f32) -> PrintObject {
    let mut mesh = TriangleMesh::load_stl(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/models/cube20.stl"
    ))
    .unwrap();
    for v in &mut mesh.vertices {
        v[2] *= h / 20.;
    }
    mesh.place_on_bed(70. + id as f32 * 40., 70.);
    PrintObject::prepare(
        format!("cube{id}"),
        id,
        0,
        mesh,
        ToolAssignments([Some(tool), Some(tool), Some(tool), None, None]),
        c,
    )
    .unwrap()
}
fn job(c: &Config) -> PreparedPrint {
    PreparedPrint::new(vec![object(c, 0, 0, 0.8), object(c, 4, 1, 0.8)], c).unwrap()
}
fn gcode(p: &PreparedPrint, c: &Config) -> String {
    export_gcode(p, c, &PrintSettings::from_config(c).unwrap(), "tower test")
        .unwrap()
        .gcode
}
fn axis(line: &str, key: char) -> Option<f64> {
    line.split_whitespace()
        .find_map(|w| w.strip_prefix(key)?.parse().ok())
}

#[test]
fn tower_is_not_an_object_and_tracks_tools_templates_and_filament() {
    let c = config();
    let p = job(&c);
    let g = gcode(&p, &c);
    assert!(g.contains("; tower true prime false"));
    assert!(p.first_layer_bbox().unwrap().max.y > p.object_first_layer_bbox().unwrap().max.y);
    let mut label = -1;
    let mut tower = false;
    let mut changes = 0;
    let mut e = 0.;
    for line in g.lines() {
        if line.starts_with("M486 S") {
            label = axis(line, 'S').unwrap() as i32;
        }
        if line.starts_with(";TYPE:") {
            tower = line == ";TYPE:Wipe tower";
        }
        if line.starts_with("; CHANGE") {
            changes += 1;
        }
        if tower
            && line.starts_with("G1 ")
            && axis(line, 'E').unwrap_or(0.) > 0.
            && axis(line, 'X').is_some()
        {
            assert_eq!(label, -1, "tower has active cancellation label: {line}");
            e += axis(line, 'E').unwrap();
        }
    }
    assert_eq!(changes, p.ordering.changes + 1); // initial call uses previous=-1
    let reported: f64 = g
        .lines()
        .filter(|l| l.starts_with("; filament used T") && l.contains("[Wipe tower] [mm] ="))
        .map(|l| l.rsplit_once('=').unwrap().1.trim().parse::<f64>().unwrap())
        .sum();
    assert!((e - reported).abs() < 0.002, "{e} vs {reported}");
    assert!(g.contains("; filament used T4 [cm3]"));
    assert!(!g.lines().any(|l| l == "T1" || l == "T2" || l == "T3"));
    assert_eq!(g.matches("M220 B\n").count(), g.matches("M220 R\n").count());
    for i in 0..p.layer_count() {
        assert_eq!(
            p.scheduled_toolpaths(i..i + 1)[0].layer.paths,
            p.scheduled_toolpaths(0..p.layer_count())[i].layer.paths
        );
    }
}
#[test]
fn tower_only_tool_and_offsets_do_not_create_object_labels() {
    let mut c = config();
    c.set_raw("wipe_tower_extruder", "3").unwrap();
    c.set_raw("extruder_offset", "0x0,0x0,2x3,0x0,0x0").unwrap();
    let p = job(&c);
    assert_eq!(p.ordering.used, [0, 2, 4]);
    let g = gcode(&p, &c);
    assert!(g.contains("T2\n"));
    assert!(!g.contains("M486 S2\n"));
    assert!(g.contains("; filament used T2 [Wipe tower]"));
    assert!(!g.contains("; filament used T2 [Perimeter]"));
}
#[test]
fn one_used_tool_stays_byte_identical_with_tower_enabled() {
    let mut c = config();
    let a = PreparedPrint::new(vec![object(&c, 4, 0, 0.4)], &c).unwrap();
    let before = gcode(&a, &c);
    c.set_raw("wipe_tower", "0").unwrap();
    let b = PreparedPrint::new(vec![object(&c, 4, 0, 0.4)], &c).unwrap();
    let program = |s: String| {
        s.split("; prusaslicer_config = begin")
            .next()
            .unwrap()
            .lines()
            .filter(|l| !l.starts_with("; generated by "))
            .collect::<Vec<_>>()
            .join("\n")
    };
    assert_eq!(program(before), program(gcode(&b, &c)));
}
#[test]
fn front_edge_priming_returns_to_first_printing_tool() {
    let mut c = config();
    c.set_raw("single_extruder_multi_material_priming", "1")
        .unwrap();
    let p = job(&c);
    let tower = p.wipe_tower.as_ref().unwrap();
    assert_eq!(tower.priming_tools, [4, 0]);
    let visits = tower.priming_visits();
    assert_eq!(visits.last().unwrap().tool, 0);
    assert!(
        visits
            .iter()
            .flat_map(|v| &v.after.moves)
            .filter(|m| m.volume > 0.)
            .all(|m| m.xy[1] < 20.)
    );
    let g = gcode(&p, &c);
    assert!(g.contains("; tower true prime true"));
    assert!(g[..g.find(";LAYER_CHANGE").unwrap()].contains(";TYPE:Wipe tower"));
}
#[test]
fn u1_zero_z_speed_falls_back_and_uses_firmware_speed_backup() {
    let mut c = config();
    for (k, v) in [
        ("gcode_flavor", "klipper"),
        ("mica_slicing_rules", "orcaslicer"),
        ("printer_model", "Snapmaker U1"),
        ("travel_speed_z", "0"),
        ("filament_multitool_ramming", "0,0,0,0,0"),
        ("exclude_object", "1"),
    ] {
        c.set_raw(k, v).unwrap();
    }
    let p = job(&c);
    let g = gcode(&p, &c);
    assert!(g.contains(";TYPE:Prime tower"));
    assert!(g.contains("M220 B\n"));
    assert!(
        !g.lines()
            .any(|l| l.starts_with("G1 Z") && l.ends_with(" F0"))
    );
    let mut active = false;
    for l in g.lines() {
        if l.starts_with("EXCLUDE_OBJECT_START ") {
            active = true;
        }
        if l.starts_with("EXCLUDE_OBJECT_END ") {
            active = false;
        }
        if l == ";TYPE:Prime tower" {
            assert!(!active);
        }
    }
}
/// Orca `GCode::set_extruder` inside `append_tcr2`: the toolchange retraction wipes along the
/// last path, the eager spiral lift precedes the toolchange G-code, OozePrevention waits for
/// the new tool, and the tower restores Z and deretracts before wiping.
#[test]
fn orca_toolchange_wipes_spiral_lifts_and_restores_on_the_tower() {
    let mut c = config();
    for (k, v) in [
        ("gcode_flavor", "klipper"),
        ("mica_slicing_rules", "orcaslicer"),
        ("printer_model", "Snapmaker U1"),
        ("exclude_object", "1"),
        ("filament_multitool_ramming", "0,0,0,0,0"),
        // Orca-only keys outside the Prusa schema stay scalar text here.
        ("z_hop_types", "Auto Lift"),
        ("travel_slope", "3"),
        ("wipe_distance", "2"),
        ("retract_lift", "0.4,0.4,0.4,0.4,0.4"),
        ("wipe", "1,1,1,1,1"),
        ("retract_before_wipe", "0%,0%,0%,0%,0%"),
        ("retract_length", "0.8,0.8,0.8,0.8,0.8"),
        ("retract_length_toolchange", "10,10,10,10,10"),
        ("enable_arc_fitting", "1"),
        ("ooze_prevention", "1"),
        ("standby_temperature_delta", "-150"),
        ("temperature", "220,220,220,220,220"),
        ("first_layer_temperature", "220,220,220,220,220"),
    ] {
        c.set_raw(k, v).unwrap();
    }
    let g = gcode(&job(&c), &c);
    let (mut changes, mut restored) = (0, 0);
    for block in g.split("; CP WIPE TOWER START").skip(1) {
        let block = &block[..block.find("; CP WIPE TOWER END").unwrap()];
        let Some(change) = block.find("; CHANGE ") else {
            continue;
        };
        changes += 1;
        let before = &block[..change];
        // retract_for_toolchange: before-wipe part plus the wipe's own retraction is 10 mm.
        let retracted: f64 = before
            .lines()
            .filter_map(|l| l.split(" E-").nth(1))
            .map(|e| e.split_whitespace().next().unwrap().parse::<f64>().unwrap())
            .sum();
        assert!((retracted - 10.).abs() < 1e-3, "{retracted}\n{before}");
        assert!(before.contains(";WIPE_START"), "{before}");
        assert!(before.contains("M104 S70 T"), "{before}");
        // The helix (linear segments without arc fitting) rises after the wipe.
        let wiped = &before[before.find(";WIPE_END").unwrap()..];
        let z: Vec<f64> = wiped
            .lines()
            .filter_map(|l| l.split(" Z").nth(1))
            .map(|z| z.split_whitespace().next().unwrap().parse().unwrap())
            .collect();
        assert!(z.len() >= 4, "{wiped}");
        let rise = z[z.len() - 1] - z[0] + (z[1] - z[0]);
        assert!((rise - 0.4).abs() < 2e-3, "{wiped}");
        let after = &block[change..];
        let wait = after
            .find("; set nozzle temperature and wait for it to be reached")
            .unwrap();
        // A tool's first selection has nothing to deretract (Extruder::unretract).
        let Some(restore) = after.find("\nG1 E10 ") else {
            continue;
        };
        restored += 1;
        assert!(wait < restore);
        assert!(after[..restore].contains("\nG1 Z"));
        let mut next = after[restore + 1..].lines();
        assert!(next.next().unwrap().starts_with("G1 E10 F"));
        assert_eq!(next.next(), Some("G4 S0"));
    }
    assert!(
        changes >= 2 && restored + 1 >= changes,
        "{changes} {restored}"
    );
}
#[test]
fn raft_gap_tower_layers_obey_max_height() {
    let mut c = config();
    c.set_raw("raft_layers", "2").unwrap();
    c.set_raw("support_material_contact_distance", "0.6")
        .unwrap();
    // The raft prints with the current tool: PrusaSlicer's wipe tower refuses non-soluble
    // supports on their own tool (Print::validate), and the option defaults are tool 1.
    c.set_raw("support_material_extruder", "0").unwrap();
    c.set_raw("support_material_interface_extruder", "0")
        .unwrap();
    let p = job(&c);
    let tower = p.wipe_tower.as_ref().unwrap();
    assert!(tower.layers.iter().all(|l| l.height <= 0.3 + 1e-6));
    let g = gcode(&p, &c);
    assert!(g.contains(";TYPE:Wipe tower"));
}

#[test]
fn sparse_free_tower_descends_outside_objects_and_restores_model_z() {
    let mut c = config();
    let start = format!("{}\nG1 Z0.2", c.string("start_gcode").unwrap());
    c.set_raw("start_gcode", &start).unwrap();
    c.set_raw("wipe_tower_no_sparse_layers", "1").unwrap();
    let low = object(&c, 4, 1, 0.4);
    let mut high = object(&c, 4, 2, 0.4);
    // A second occurrence begins after a gap in this tool's use; the main object spans it.
    high.mesh.translate([0., 0., 0.8]);
    high = PrintObject::prepare(high.name, high.object_id, 0, high.mesh, high.tools, &c).unwrap();
    let p = PreparedPrint::new(vec![object(&c, 0, 0, 1.4), low, high], &c).unwrap();
    let tower = p.wipe_tower.as_ref().unwrap();
    assert!(tower.layers.iter().any(|l| l.skipped));
    let g = gcode(&p, &c);
    let mut z = 0.;
    let mut layer_z = 0.;
    let mut role = "";
    let mut lower_tower = false;
    for line in g.lines() {
        if let Some(v) = line.strip_prefix(";Z:") {
            layer_z = v.parse().unwrap();
        }
        if let Some(v) = line.strip_prefix(";TYPE:") {
            role = v;
        }
        if line.starts_with("G1 ") {
            z = axis(line, 'Z').unwrap_or(z);
            if axis(line, 'E').unwrap_or(0.) > 0. && axis(line, 'X').is_some() {
                if role == "Wipe tower" {
                    lower_tower |= z < layer_z - 1e-4;
                } else {
                    assert!(
                        (z - layer_z).abs() < 1e-4,
                        "{line} model Z{z}, layer {layer_z}"
                    );
                }
            }
        }
    }
    assert!(lower_tower);
}

#[test]
fn filament_toolchange_retraction_overrides_the_printer_value_per_tool() {
    // PrintApply.cpp print_config_diffs + ConfigOptionVector::apply_override: a non-nil
    // filament_retract_length_toolchange element wins for its tool, nil keeps the printer's
    // value, and templates (e.g. the XL start G-code's e_retracted bookkeeping) see the result.
    let mut c = config();
    c.set_raw("retract_length_toolchange", "1.2,1.2,1.2,1.2,1.2")
        .unwrap();
    c.set_raw("filament_retract_length_toolchange", "20,nil,nil,nil,nil")
        .unwrap();
    c.set_raw(
        "start_gcode",
        "; tc {retract_length_toolchange[0]} {retract_length_toolchange[4]} {is_nil(filament_retract_length_toolchange[4])}",
    )
    .unwrap();
    let p = job(&c);
    let g = gcode(&p, &c);
    assert!(g.contains("; tc 20 1.2 true"), "{}", &g[..2000]);
    let mut last_retract = None;
    let mut checked = 0;
    for line in g.lines() {
        if line.starts_with("G1 E-") {
            last_retract = axis(line, 'E');
        }
        if let Some(rest) = line.strip_prefix("; CHANGE ") {
            let from: i64 = rest.split_whitespace().next().unwrap().parse().unwrap();
            if from >= 0 {
                let expected = if from == 0 { -20. } else { -1.2 };
                assert_eq!(last_retract, Some(expected), "{line}");
                checked += 1;
            }
        }
    }
    assert!(checked >= 2);
}

#[test]
fn prusa_toolchange_restores_xy_z_and_deretracts_inside_the_tower() {
    // WipeTower::toolchange_Change: the custom toolchange G-code may move the head (here it
    // lifts Z by 2 mm), so the tower travels back without skipping, forces the layer Z and
    // deretracts before the wipe, without an extra lift of its own.
    let c = config();
    let p = job(&c);
    let g = gcode(&p, &c);
    let lines: Vec<_> = g.lines().collect();
    let mut checked = 0;
    let mut deretracted = 0;
    for (i, line) in lines.iter().enumerate() {
        if line.starts_with("; CHANGE ") && !line.starts_with("; CHANGE -1") {
            let rest = &lines[i..];
            let travel = rest.iter().position(|l| l.starts_with("G1 F")).unwrap();
            assert!(rest[travel + 1].starts_with("G1 X"), "{}", rest[travel + 1]);
            assert!(rest[travel + 2].starts_with("G1 Z"), "{}", rest[travel + 2]);
            assert!(!rest[..travel].iter().any(|l| l.starts_with("G1 Z")));
            // A tool parked with its toolchange retraction is deretracted right there.
            deretracted += usize::from(rest[travel + 3].starts_with("G1 E"));
            checked += 1;
        }
    }
    assert!(checked >= 2);
    assert!(deretracted >= 1);
}

fn wiping(mut c: Config) -> Config {
    c.set_raw("wipe", "1,1,1,1,1").unwrap();
    c.set_raw("retract_before_wipe", "80%,80%,80%,80%,80%")
        .unwrap();
    c
}

#[test]
fn leaving_the_tower_wipes_along_the_tower_wipe_path() {
    // WipeTowerIntegration::append_tcr sets gcodegen.m_wipe to the tower's wipe_path when the
    // tower was printed at the object Z, so the next retraction wipes on the tower.
    let c = wiping(config());
    let g = gcode(&job(&c), &c);
    let lines: Vec<_> = g.lines().collect();
    let inside = |l: &str| {
        axis(l, 'X').is_none_or(|x| (19. ..=56.).contains(&x))
            && axis(l, 'Y').is_none_or(|y| (150. ..=175.).contains(&y))
    };
    let mut wiped = 0;
    for (i, line) in lines.iter().enumerate() {
        if *line != "; CP WIPE TOWER END" {
            continue;
        }
        let Some(r) = lines[i..]
            .iter()
            .take_while(|l| !l.starts_with(";TYPE:"))
            .position(|l| l.starts_with("G1 E-"))
        else {
            continue;
        };
        let rest = &lines[i + r..];
        if rest[1] == ";WIPE_START" {
            let moves: Vec<_> = rest
                .iter()
                .take_while(|l| **l != ";WIPE_END")
                .filter(|l| l.starts_with("G1 X") || l.starts_with("G1 Y"))
                .collect();
            assert!(
                !moves.is_empty() && moves.iter().all(|l| inside(l)),
                "{moves:?}"
            );
            wiped += 1;
        }
    }
    assert!(wiped >= 2, "{wiped}");
}

#[test]
fn toolchange_without_tower_wipes_and_ooze_prevention_waits() {
    // GCodeGenerator::set_extruder: retract_and_wipe(true) retracts retract_before_wipe of the
    // toolchange length, wipes along the last extrusion and tops up; OozePrevention::
    // post_toolchange waits for the new tool; the next travel starts "after unknown position"
    // (XY, then the layer Z) without a lift.
    for ooze in [true, false] {
        let mut c = wiping(config());
        c.set_raw("wipe_tower", "0").unwrap();
        c.set_raw("temperature", "215,215,215,215,215").unwrap();
        c.set_raw("first_layer_temperature", "215,215,215,215,215")
            .unwrap();
        c.set_raw("ooze_prevention", if ooze { "1" } else { "0" })
            .unwrap();
        let g = gcode(&job(&c), &c);
        let lines: Vec<_> = g.lines().collect();
        let mut checked = 0;
        for (i, line) in lines.iter().enumerate() {
            let Some(rest) = line.strip_prefix("; CHANGE ") else {
                continue;
            };
            let mut words = rest.split_whitespace();
            let from: i64 = words.next().unwrap().parse().unwrap();
            let to: i64 = words.next().unwrap().parse().unwrap();
            if from < 0 {
                continue;
            }
            let start = lines[..i]
                .iter()
                .rposition(|l| l.starts_with("G1 E-4 "))
                .unwrap();
            assert_eq!(
                lines[start + 1],
                ";WIPE_START",
                "{}",
                lines[start..i].join("\n")
            );
            let retracted: f64 = lines[start..i]
                .iter()
                .filter_map(|l| axis(l, 'E'))
                .map(|e| -e)
                .sum();
            assert!((retracted - 5.).abs() < 1e-4, "{retracted}");
            let after = &lines[i..];
            let deretract = after
                .iter()
                .position(|l| l.starts_with("G1 E") && !l.starts_with("G1 E-"))
                .unwrap();
            let wait = format!("M109 S215 T{to} ; set temperature and wait for it to be reached");
            assert_eq!(after[..deretract].contains(&wait.as_str()), ooze);
            let moves: Vec<_> = after[..deretract]
                .iter()
                .filter(|l| l.starts_with("G1 X") || l.starts_with("G1 Z"))
                .collect();
            assert_eq!(moves.len(), 2, "{moves:?}");
            assert!(moves[0].starts_with("G1 X") && !moves[0].contains(" Z"));
            assert!(moves[1].starts_with("G1 Z"));
            checked += 1;
        }
        assert!(checked >= 2);
    }
}

#[test]
fn prusa_tower_exit_matches_tool_change_and_finish_layer_sequence() {
    // A small 2-tool XL-like job (filament monitoring, M220 backup, wipe on retraction).
    // WipeTower::tool_change ends with M220 R, a travel feed rate, G4 S0 and G92 E0 before
    // merge_tcr's joined finish_layer; the final unload does not re-enable monitoring;
    // leaving the tower retracts retract_length once (retract_before_wipe, then the wipe),
    // never the toolchange length, and repeats no acceleration the toolchange left alone.
    let mut c = wiping(config());
    for (k, v) in [
        ("printer_notes", "PRINTER_MODEL_XL"),
        ("retract_length", "0.8,0.8,0.8,0.8,0.8"),
        ("retract_length_toolchange", "20,20,20,20,20"),
        ("default_acceleration", "2500"),
        ("wipe_tower_acceleration", "0"),
        ("travel_speed", "400"),
    ] {
        c.set_raw(k, v).unwrap();
    }
    let g = gcode(&job(&c), &c);
    let lines: Vec<_> = g.lines().collect();
    let (mut exits, mut joined, mut finals) = (0, 0, 0);
    for (i, line) in lines.iter().enumerate() {
        if *line != "; CP WIPE TOWER START" {
            continue;
        }
        let end = i + lines[i..]
            .iter()
            .position(|l| *l == "; CP WIPE TOWER END")
            .unwrap();
        let visit = &lines[i..end];
        let text = visit.join("\n");
        let count = |s: &str| visit.iter().filter(|l| **l == s).count();
        assert_eq!(count("M220 B"), 1, "{text}");
        assert_eq!(count("M220 R"), 1, "{text}");
        assert_eq!(count("G92 E0"), 1, "{text}");
        let restore = visit.iter().position(|l| *l == "M220 R").unwrap();
        assert_eq!(
            visit[restore + 1..restore + 4],
            ["G1 F24000", "G4 S0", "G92 E0"],
            "{text}"
        );
        let change = visit.iter().any(|l| l.starts_with("; CHANGE "));
        assert_eq!(count("M591 R"), usize::from(change), "{text}");
        if !change {
            finals += 1;
            continue;
        }
        // The joined finish_layer comes after the toolchange's trailer.
        if visit[restore + 4..]
            .iter()
            .any(|l| l.starts_with("G1 X") && l.contains(" E"))
        {
            joined += 1;
        }
        // The exit: everything up to the next model extrusion.
        let next = lines[end..]
            .iter()
            .position(|l| l.starts_with("G1 X") && axis(l, 'E').is_some_and(|e| e > 0.))
            .unwrap();
        let exit = &lines[end..end + next];
        let text = exit.join("\n");
        let (mut retracts, mut retracted, mut wiped, mut wiping) = (0, 0., 0., false);
        for l in exit {
            match *l {
                ";WIPE_START" => wiping = true,
                ";WIPE_END" => wiping = false,
                _ => {}
            }
            if let Some(e) = axis(l, 'E').filter(|e| *e < 0.) {
                if wiping {
                    wiped -= e;
                } else {
                    retracts += 1;
                    retracted -= e;
                }
            }
        }
        assert_eq!(retracts, 1, "{text}");
        assert!((retracted - 0.64).abs() < 1e-4, "{text}");
        assert!((wiped - 0.16).abs() < 1e-4, "{text}");
        let retract = exit.iter().position(|l| l.starts_with("G1 E-")).unwrap();
        assert!(
            !exit[..retract].iter().any(|l| l.starts_with("M204")),
            "{text}"
        );
        exits += 1;
    }
    assert!(
        exits >= 3 && joined >= 3 && finals == 1,
        "{exits} {joined} {finals}"
    );
}
