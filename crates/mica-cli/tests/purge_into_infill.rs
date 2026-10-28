// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Purging into infill and objects (`wipe_into_infill`, `wipe_into_objects`) on the XL, with the
//! sparse infill on tool 2. The filament per tool is checked against PrusaSlicer 2.9.6's G-code
//! for the same presets and settings (cube20 centered at 180,180; docs/wipe-tower.md).

fn model(name: &str) -> String {
    format!("{}/../../tests/models/{name}", env!("CARGO_MANIFEST_DIR"))
}

/// Slices cube20 on the XL with infill on tool 2 and `extra` settings; `None` without profiles.
fn slice(tag: &str, extra: &[&str]) -> Option<String> {
    let out = std::env::temp_dir().join(format!(
        "mica-purge-into-infill-{tag}-{}.gcode",
        std::process::id()
    ));
    let mut all: Vec<String> = [
        "slice",
        &model("cube20.stl"),
        "--printer",
        "Original Prusa XL - 5T 0.4 nozzle",
        "--print",
        "0.20mm SPEED @XL 0.4",
        "--filament",
        "Prusament PLA @XL",
        "--set",
        "infill_extruder=2",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    for e in extra {
        all.push("--set".into());
        all.push(e.to_string());
    }
    all.push("-o".into());
    all.push(out.to_string_lossy().into_owned());
    if !mica_testkit::require_profiles_for(&all) {
        return None;
    }
    if let Err(e) = mica_cli::run(&all) {
        panic!("mica {all:?}: {e}");
    }
    let g = std::fs::read_to_string(&out).unwrap();
    let _ = std::fs::remove_file(&out);
    Some(g)
}

/// `; filament used T<tool> [mm] = …` (0 when the tool is not used).
fn used(g: &str, tool: usize) -> f64 {
    let key = format!("; filament used T{tool} [mm] = ");
    g.lines()
        .find_map(|l| l.strip_prefix(&key))
        .map_or(0., |v| v.trim().parse().unwrap())
}

fn tool_changes(g: &str, tool: usize) -> usize {
    let t = format!("T{tool} ");
    g.lines()
        .skip_while(|l| *l != ";LAYER_CHANGE")
        .filter(|l| l.starts_with(&t))
        .count()
}

#[test]
fn infill_tool_gives_way_where_the_layer_has_no_solid_infill() {
    let Some(off) = slice("off", &[]) else {
        return;
    };
    let on = slice("on", &["wipe_into_infill=1"]).unwrap();
    // PrusaSlicer 2.9.6: T1 1659.95 / T2 1263.30 mm without, 2384.68 / 59.61 mm with.
    for (g, t0, t1) in [(&off, 1659.95, 1263.30), (&on, 2384.68, 59.61)] {
        assert!((used(g, 0) - t0).abs() < 1., "T1 {} != {t0}", used(g, 0));
        assert!((used(g, 1) - t1).abs() < 1., "T2 {} != {t1}", used(g, 1));
    }
    assert!(used(&on, 0) + used(&on, 1) < 0.9 * (used(&off, 0) + used(&off, 1)));
    // The sparse layers print their infill with the wall tool: almost no tool changes left.
    assert!(tool_changes(&off, 1) > 40, "{}", tool_changes(&off, 1));
    assert!(tool_changes(&on, 1) <= 3, "{}", tool_changes(&on, 1));
}

#[test]
fn purging_into_the_object_prints_it_with_one_tool() {
    let Some(g) = slice("objects", &["wipe_into_objects=1"]) else {
        return;
    };
    // PrusaSlicer 2.9.6: every wall and fill may absorb a purge, so tool 2 is never needed.
    assert!((used(&g, 0) - 1307.94).abs() < 1., "{}", used(&g, 0));
    assert_eq!(used(&g, 1), 0.);
    assert!(!g.contains(";TYPE:Wipe tower"));
}
