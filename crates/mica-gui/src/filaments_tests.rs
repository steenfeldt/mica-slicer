// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! More filaments than tools, end to end on the Snapmaker U1 (four tools): a plate prints
//! the filaments it uses on the printer's tools, its configuration takes each tool's
//! filament settings, a plate with more filaments than tools says so, and a printer change
//! or a save keeps the project's filaments.

use crate::app::{App, Options};
use eframe::egui;
use mica_profiles::user::Store;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

fn temp(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mica-filaments-{tag}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The U1 with two 20 mm cubes on plate 1 and six filaments, F6 blue at 235 °C.
fn u1_six(ctx: &egui::Context) -> Option<App> {
    if !mica_testkit::require_orca_profiles() {
        return None;
    }
    let mut app = App::create(
        ctx,
        Options {
            prefs_path: None,
            store: Some(Store::open(temp("store"))),
            files: vec![],
            headless: true,
        },
    );
    let (vi, _, name) = app
        .profiles
        .all_printers()
        .into_iter()
        .find(|(_, _, n)| n == "Snapmaker U1 (0.4 nozzle)")
        .expect("the Snapmaker U1 among the loaded printers");
    app.switch_printer(vi, &name);
    let cube = PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/models/cube20.stl"
    ));
    let loaded = crate::app::load_files(vec![cube.clone(), cube], &app.config, app.bed.center());
    app.finish_load(loaded);
    assert!(app.grow_filaments(6));
    app.overlays.set_colour(5, Some("#3C7DD9".into()));
    app.overlays.filaments[5].insert("temperature".into(), "235".into());
    app.settings_changed();
    Some(app)
}

fn objects(app: &App) -> Vec<crate::scene::Object> {
    match app.slice_job(0) {
        crate::slice_cache::Job::Objects(o, _) => o,
        crate::slice_cache::Job::Calibration(_) => panic!("an ordinary plate"),
    }
}

#[test]
fn a_plate_prints_its_filaments_on_free_tools() {
    let ctx = egui::Context::default();
    let Some(mut app) = u1_six(&ctx) else {
        return;
    };
    assert_eq!((app.profiles.tool_count(), app.filament_count()), (4, 6));
    app.plates.current_mut().objects[0].set_tool(0);
    app.plates.current_mut().objects[1].set_tool(5);
    app.changed();
    // F1 stays on T1; F6 takes the first free tool, T2.
    let tools = app.plate_tools(0).unwrap();
    assert_eq!(tools.slots[..2], [0, 5]);
    let o = objects(&app);
    assert_eq!(o[0].tools.0[0], Some(0));
    assert_eq!(o[1].tools.0[0], Some(1), "F6 prints as T2");
    // The scene keeps naming the filament.
    assert_eq!(app.plates.current().objects[1].tools.0[0], Some(5));
    let c = app.job_config(0).unwrap();
    assert_eq!(c.vector_len("nozzle_diameter"), 4);
    assert_eq!(c.vector_len("filament_colour"), 4, "one per tool");
    assert_eq!(
        c.int_at("temperature", 1),
        Some(235),
        "T2 has F6's settings"
    );
    let colours = c.raw("filament_colour").unwrap();
    assert_eq!(
        colours.split(';').nth(1).map(|s| s.trim_matches('"')),
        Some("#3C7DD9"),
        "{colours}"
    );
}

#[test]
fn a_plate_with_more_filaments_than_tools_does_not_slice() {
    let ctx = egui::Context::default();
    let Some(mut app) = u1_six(&ctx) else {
        return;
    };
    let p = &mut app.plates.current_mut().objects[0];
    p.set_tool(0);
    p.tools.0[3] = Some(1);
    p.tools.0[4] = Some(2);
    let q = &mut app.plates.current_mut().objects[1];
    q.set_tool(3);
    q.overrides
        .insert("solid_infill_extruder".into(), "5".into());
    app.changed();
    let e = app.job_config(0).unwrap_err();
    assert!(
        e.contains("uses 5 filaments") && e.contains("4 tools"),
        "{e}"
    );
}

#[test]
fn a_printer_change_and_a_save_keep_the_projects_filaments() {
    let ctx = egui::Context::default();
    let Some(mut app) = u1_six(&ctx) else {
        return;
    };
    app.plates.current_mut().objects[1].set_tool(5);
    app.changed();
    let (vi, name) = (app.profiles.vendor, app.profiles.printer.clone());
    app.switch_printer(vi, "Snapmaker U1 (0.2 nozzle)");
    assert_eq!(app.filament_count(), 6, "F6 is used");
    app.switch_printer(vi, &name);
    assert_eq!(app.filament_count(), 6);
    app.overlays.set_colour(5, Some("#3C7DD9".into()));
    app.colours_changed();
    let path = temp("save").join("six.3mf");
    app.save_project_to(&path, mica_core::threemf::Flavour::Orca)
        .unwrap();
    let saved = mica_core::threemf::load_3mf(&path).unwrap();
    let json: serde_json::Value =
        serde_json::from_str(&saved.config.as_ref().unwrap().text).unwrap();
    let colours = json["filament_colour"].as_array().unwrap();
    assert_eq!(colours.len(), 6, "{colours:?}");
    assert_eq!(colours[5], "#3C7DD9");
    // Opened again: six filaments, the cube on F6.
    let mut again = u1_six(&ctx).unwrap();
    let loaded = crate::app::load_files(vec![path], &again.config, again.bed.center());
    again.finish_load(loaded);
    assert_eq!(again.filament_count(), 6);
    assert!(
        again
            .plates
            .plates
            .iter()
            .flat_map(|p| &p.objects)
            .any(|o| o.tools.0[0] == Some(5))
    );
}
