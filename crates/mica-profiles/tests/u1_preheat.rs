// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: orca@8500fcdcca src/libslic3r/GCode/GCodeProcessor.cpp :: GCodeProcessor::run_post_process
// Source: orca@8500fcdcca src/libslic3r/GCode.cpp :: OozePrevention::pre_toolchange, GCode::_do_export
// Changes: Four-tool U1 job (PLA, PETG) through the full export: preheat lines never retarget the
// Changes: printing tool, only heat tools selected later, and agree with the toolchange M109 waits.

use std::collections::BTreeMap;

use mica_core::{
    config::Config,
    mesh::TriangleMesh,
    print_job::{PreparedPrint, PrintObject, ToolAssignments},
    settings::PrintSettings,
};
use mica_profiles::orca::{Bundle, Dialect};

const PRINTER: &str = "Snapmaker U1 (0.4 nozzle)";
const PROCESS: &str = "0.20 Standard @Snapmaker U1 (0.4 nozzle)";
const PLA: &str = "Snapmaker PLA @U1";
const PETG: &str = "Snapmaker PETG @U1";
const BED: &str = "High Temp Plate";

fn bundle() -> Option<Bundle> {
    if !mica_testkit::require_orca_profiles() {
        return None;
    }
    let root = mica_testkit::orca_profiles();
    let mut b =
        Bundle::load_vendor(root.join("OrcaFilamentLibrary.json"), Dialect::Orca242).unwrap();
    b.add_vendor(root.join("Snapmaker.json")).unwrap();
    Some(b)
}

/// T0, T2 and T3 print PLA, T1 PETG, like a project with per-tool filaments.
fn config(b: &Bundle) -> (Config, Config) {
    let mut c = b
        .single_tool_config(PRINTER, PROCESS, PLA, Some(BED))
        .unwrap();
    let petg = b
        .single_tool_config(PRINTER, PROCESS, PETG, Some(BED))
        .unwrap();
    mica_profiles::project::merge_filament(&mut c, &petg, 1).unwrap();
    (c, petg)
}

fn cube(c: &Config, id: usize, x: f32, tool: usize) -> PrintObject {
    let mut m = TriangleMesh::load_stl(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/models/cube20.stl"
    ))
    .unwrap();
    for v in &mut m.vertices {
        v[0] *= 0.4;
        v[1] *= 0.4;
        v[2] *= 0.15;
    }
    m.place_on_bed(x, 120.);
    let t = Some(tool);
    PrintObject::prepare(
        format!("cube {id}"),
        id,
        0,
        m,
        ToolAssignments([t, t, t, None, None]),
        c,
    )
    .unwrap()
}

fn command(line: &str) -> &str {
    line.split(';')
        .next()
        .unwrap_or("")
        .split_whitespace()
        .next()
        .unwrap_or("")
}

fn word(line: &str, key: char) -> Option<f64> {
    line.split(';')
        .next()
        .unwrap_or("")
        .split_whitespace()
        .find_map(|w| w.strip_prefix(key).and_then(|v| v.parse().ok()))
}

#[test]
fn u1_four_tool_preheat_avoids_the_printing_tool_and_matches_waits() {
    let Some(b) = bundle() else {
        return;
    };
    let (c, petg_config) = config(&b);
    let petg = [
        c.float_at("first_layer_temperature", 1).unwrap(),
        c.float_at("temperature", 1).unwrap(),
    ];
    assert_ne!(Some(petg[1]), c.float_at("temperature", 0));
    // Orca-only filament vectors follow the merged tool too (templates and validator read them).
    let slot = |key: &str, tool: usize| -> Option<String> {
        Some(c.extra(key)?.split(',').nth(tool)?.to_string())
    };
    for key in ["nozzle_temperature", "nozzle_temperature_range_high"] {
        let own = petg_config
            .extra(key)
            .and_then(|v| v.split(',').next())
            .map(str::to_string);
        assert!(own.is_some(), "{key}");
        assert_eq!(slot(key, 1), own, "{key}");
        assert_ne!(slot(key, 0), own, "{key}");
        // The typed copy too: templates read it, not the text.
        assert_eq!(c.float_at(key, 1), petg_config.float_at(key, 0), "{key}");
        assert_ne!(c.float_at(key, 0), c.float_at(key, 1), "{key}");
    }
    // Short per-tool segments: every 30 s preheat window reaches back into the
    // tool's own previous segment, the case upstream inserts into.
    let p = PreparedPrint::new(
        vec![
            cube(&c, 0, 90., 0),
            cube(&c, 1, 120., 1),
            cube(&c, 2, 150., 2),
            cube(&c, 3, 180., 3),
        ],
        &c,
    )
    .unwrap();
    assert_eq!(p.ordering.used, [0, 1, 2, 3]);
    let g =
        mica_gcode::export::export_gcode(&p, &c, &PrintSettings::from_config(&c).unwrap(), "job")
            .unwrap()
            .gcode;
    let lines: Vec<&str> = g.lines().collect();
    let mut active = None;
    let mut pending: BTreeMap<usize, (f64, usize)> = BTreeMap::new();
    let mut preheats = 0;
    for (i, line) in lines.iter().enumerate() {
        if let Some(t) = command(line)
            .strip_prefix('T')
            .and_then(|s| s.parse::<usize>().ok())
        {
            active = Some(t);
            pending.remove(&t);
            continue;
        }
        if line.contains("; preheat T") {
            preheats += 1;
            let tool = word(line, 'T').unwrap() as usize;
            assert_ne!(
                active,
                Some(tool),
                "line {}: preheat for the printing tool",
                i + 1
            );
            pending.insert(tool, (word(line, 'S').unwrap(), i));
        }
        if command(line) == "M109"
            && let (Some(t), Some(s)) = (word(line, 'T'), word(line, 'S'))
            && let Some(&(preheat, at)) = pending.get(&(t as usize))
        {
            assert_eq!(
                s,
                preheat,
                "line {}: M109 contradicts preheat at {}",
                i + 1,
                at + 1
            );
        }
        if command(line) == "M109" && word(line, 'T') == Some(1.) {
            let s = word(line, 'S').unwrap();
            assert!(petg.contains(&s), "line {}: PETG wait at {s}", i + 1);
        }
    }
    assert!(preheats > 10, "job never exercised the preheat pass");
    assert!(
        pending.is_empty(),
        "preheat for a tool never selected: {pending:?}"
    );
    assert!(g.contains("; removed M104"), "no cooldown was suppressed");
    let profile = mica_gcode::validate::Profile::from_config(&c).unwrap();
    let report = mica_gcode::validate::validate_text(g.as_bytes(), &profile);
    assert_eq!(report.errors, 0, "{:?}", report.diagnostics);
}
