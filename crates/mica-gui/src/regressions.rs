// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c770b8380ee81c921a2867d522f288123 src/slic3r-arrange-wrapper/src/SceneBuilder.cpp :: ArrangeableSlicerModel::for_each_arrangeable_
// Changes: Original GUI regressions; verify adapted arrange preserves instance identities on overflow, fixed towers and oversized transforms.

//! Regression tests for what the monkey test (`monkey.rs`) found: each one replays the
//! smallest sequence of editor actions that crashed, hung or corrupted the scene before its
//! fix (see docs/quality-safety.md, "GUI monkey test").

use crate::{
    app::{App, Options},
    commands::{Command, Shape},
    overlays::Target,
    panels::editor::Edit,
    viewport::Action,
};
use eframe::egui;
use glam::Vec3;
use mica_profiles::user::Store;
use std::{
    path::PathBuf,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::{Duration, Instant},
};

static NEXT_STORE: AtomicU64 = AtomicU64::new(0);

fn model(name: &str) -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/models/")).join(name)
}

fn temp_dir(what: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mica-regression-{what}-{}-{}",
        std::process::id(),
        NEXT_STORE.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A headless app on the given printer (`None`: the detected default) with these models.
fn app(ctx: &egui::Context, printer: Option<&str>, models: &[&str]) -> App {
    let mut app = App::create(
        ctx,
        Options {
            prefs_path: None,
            store: Some(Store::open(temp_dir("store"))),
            files: vec![],
            headless: true,
        },
    );
    if let Some(name) = printer {
        let (vendor, _, name) = app
            .profiles
            .all_printers()
            .into_iter()
            .find(|(_, _, n)| n == name)
            .expect("printer preset");
        app.profiles.select_printer(vendor, &name);
        app.settings_changed();
    }
    if !models.is_empty() {
        let result = crate::app::load_files(
            models.iter().map(|m| model(m)).collect(),
            &app.config,
            app.bed.center(),
        );
        app.finish_load(result);
    }
    app
}

const CORE_ONE: &str = "Prusa CORE One HF0.4 nozzle";

fn map(app: &App) -> Vec<(usize, usize)> {
    app.scene_objects().1
}

fn wait_idle(app: &mut App, ctx: &egui::Context) {
    let deadline = Instant::now() + Duration::from_secs(300);
    while app.busy() && Instant::now() < deadline {
        app.poll(ctx);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(!app.busy(), "the job did not finish: {}", app.status);
}

/// A slice job named its plate by index: removing an earlier plate while it ran sent the
/// result to the wrong plate, or (removing the last index) panicked with an out-of-bounds
/// index when the result arrived. Removing a plate before the current one also made the next
/// plate current.
#[test]
fn a_slice_finds_its_plate_after_an_earlier_plate_is_removed() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx, Some(CORE_ONE), &["cube20.stl"]);
    app.select(Some(0), false);
    app.copy_selection();
    for _ in 0..2 {
        app.plates.add();
        app.paste(None);
    }
    assert_eq!(app.plates.len(), 3);
    let (b, c) = (app.plates.plate(1).id, app.plates.plate(2).id);
    app.select_plate(1);
    app.slice_plate(2, false, &ctx);
    assert!(app.worker.is_some(), "{}", app.status);
    app.handle(Action::RemovePlate(0), &map(&app), &ctx);
    assert_eq!(
        app.plates.current().id,
        b,
        "the current plate stays current"
    );
    wait_idle(&mut app, &ctx);
    let i = app.plates.index_of(c).unwrap();
    assert_eq!(i, 1);
    assert!(app.plates.plate(i).finished.is_some(), "{}", app.status);
    assert!(app.plates.plate(0).finished.is_none());
}

/// Undo rebuilds every plate: a job still running for the old plate is cancelled instead of
/// delivering into whatever plate now has its index.
#[test]
fn undo_cancels_a_job_whose_plate_was_replaced() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx, Some(CORE_ONE), &["cube20.stl"]);
    app.select(Some(0), false);
    app.copy_selection();
    app.plates.add();
    app.paste(None);
    app.select_plate(0);
    app.slice_plate(1, false, &ctx);
    assert!(app.worker.is_some());
    app.undo();
    app.poll(&ctx);
    assert!(app.worker.is_none(), "the orphaned job is cancelled");
    wait_idle(&mut app, &ctx);
    assert!(app.plates.plates.iter().all(|p| p.finished.is_none()));
}

/// An object scaled past the bed never fits a plate: Arrange spilled it onto a new plate,
/// where it did not fit either, forever (the window hung).
#[test]
fn arrange_leaves_an_object_larger_than_the_bed_alone() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx, Some(CORE_ONE), &["cube20.stl", "hole_block.stl"]);
    app.plates.current_mut().objects[0].scale = Vec3::splat(100.);
    let position = app.plates.current().objects[0].position;
    let rotation = app.plates.current().objects[0].rotation();
    app.arrange();
    assert_eq!(app.plates.current().objects[0].position, position);
    assert_eq!(app.plates.current().objects[0].rotation(), rotation);
    assert_eq!(app.plates.len(), 1, "no plate per attempt");
    assert!(
        app.notifications
            .list
            .iter()
            .any(|n| n.level == crate::notifications::Level::Warning
                && n.text.contains("larger than the bed")),
        "a warning says so"
    );
    app.arrange_all();
    assert_eq!(app.plates.len(), 1);
}

/// Arrange all renumbered every object: instances stopped being instances, and objects from
/// different plates with the same id merged into one.
#[test]
fn arrange_all_keeps_instances_and_plate_identities() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx, Some(CORE_ONE), &["cube20.stl"]);
    app.select(Some(0), false);
    app.add_instances();
    let mut other = app.plates.plate(0).objects[0].fresh();
    other.object_id = app.plates.plate(0).objects[0].object_id;
    app.plates.add();
    app.plates.current_mut().objects.push(other);
    app.arrange_all();
    let objects: Vec<_> = app.plates.plates.iter().flat_map(|p| &p.objects).collect();
    assert_eq!(objects.len(), 3);
    assert_eq!(objects[0].object_id, objects[1].object_id, "instances");
    assert_ne!(
        objects[0].object_id, objects[2].object_id,
        "another plate's object"
    );
}

#[test]
fn arrange_spill_keeps_instance_groups_and_plate_settings() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx, Some(CORE_ONE), &["cube20.stl"]);
    app.plates.current_mut().objects[0].scale = Vec3::splat(3.);
    let prototype = app.plates.current().objects[0].clone();
    app.plates.current_mut().objects = vec![prototype; 20];
    app.plates
        .current_mut()
        .overrides
        .insert("duplicate_distance".into(), "8".into());
    app.arrange();
    assert!(app.plates.len() > 1);
    assert_eq!(
        app.plates
            .plates
            .iter()
            .map(|p| p.objects.len())
            .sum::<usize>(),
        20
    );
    assert!(app.plates.plates.iter().any(|p| p.objects.len() > 1));
    for p in &app.plates.plates {
        assert_eq!(
            p.overrides.get("duplicate_distance").map(String::as_str),
            Some("8")
        );
        for o in &p.objects {
            assert_eq!(o.object_id, p.objects[0].object_id);
            assert_eq!(o.rotation(), p.objects[0].rotation());
        }
    }
}

#[test]
fn arrange_keeps_xl_wipe_and_u1_prime_towers_fixed() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx, None, &[]);
    for printer in ["XL", "Snapmaker U1"] {
        let (vendor, _, name) = app
            .profiles
            .all_printers()
            .into_iter()
            .find(|(_, _, n)| {
                n.contains(printer) && n.contains("0.4") && (printer != "XL" || n.contains("5T"))
            })
            .expect("tower printer preset");
        app.profiles.select_printer(vendor, &name);
        app.settings_changed();
        let result = crate::app::load_files(
            vec![model("cube20.stl"), model("cube20.stl")],
            &app.config,
            app.bed.center(),
        );
        app.finish_load(result);
        app.plates.current_mut().objects[0].set_tool(0);
        app.plates.current_mut().objects[1].set_tool(1);
        app.changed();
        let before = app.towers[0]
            .as_ref()
            .expect("multi-tool tower")
            .shape
            .clone();
        app.arrange();
        let after = &app.towers[0].as_ref().expect("tower after arrange").shape;
        assert_eq!(before.position, after.position, "{printer}");
        assert_eq!(before.rotation, after.rotation, "{printer}");
        for o in &app.plates.current().objects {
            assert!(
                !mica_core::arrange::polygons_overlap(&o.footprint_hull(), &after.hull),
                "{printer} tower overlap"
            );
        }
        app.plates.current_mut().objects.clear();
    }
}

/// A tool selector set past the printer's tools (`perimeter_extruder = 8` on a CORE One, or
/// tools left by a switch from the XL) made every model fail to open ("perimeter_extruder
/// selects tool 8, printer has 1 tools"), and a project saved then could not be reopened.
#[test]
fn tool_selectors_stay_within_the_printer_and_models_still_open() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx, Some(CORE_ONE), &[]);
    app.apply_edit(
        Some(Target::Print),
        Edit::Set {
            key: "perimeter_extruder".into(),
            raw: "8".into(),
        },
    );
    app.end_gesture();
    // Clamped to tool 1, which (the profile's own) leaves nothing changed.
    assert_eq!(
        app.effective_config()
            .and_then(|c| c.int("perimeter_extruder")),
        Some(1)
    );
    assert!(
        app.overlays
            .print
            .get("perimeter_extruder")
            .is_none_or(|v| v == "1")
    );
    // As a printer switch from the XL leaves them: models open, the slice clamps.
    app.overlays
        .print
        .insert("infill_extruder".into(), "3".into());
    app.recompute_config();
    let result = crate::app::load_files(vec![model("cube20.stl")], &app.config, app.bed.center());
    assert!(result.is_ok(), "{:?}", result.err());
    app.finish_load(result);
    let dir = temp_dir("tools");
    let path = dir.join("tools.3mf");
    app.plates.current_mut().objects[0].set_tool(3);
    app.save_project_to(&path, mica_core::threemf::Flavour::Prusa)
        .unwrap();
    let reopened = crate::app::load_files(vec![path], &app.config, app.bed.center());
    assert!(reopened.is_ok(), "{:?}", reopened.err());
}

/// The GUI sliced objects outside the build volume (scaled 10,000×): minutes of geometry
/// work on a 200 m model, between which cancel was not checked. The CLI's placement preflight
/// refuses them at once.
#[test]
fn slicing_refuses_an_object_outside_the_build_volume_at_once() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx, Some(CORE_ONE), &["cube20.stl"]);
    app.plates.current_mut().objects[0].scale = Vec3::splat(1000.);
    let config = app.plate_config(0).unwrap();
    let objects = app.plates.plate(0).objects.clone();
    let started = Instant::now();
    let result = crate::worker::slice(objects, config, &AtomicBool::new(false), &|_| {});
    let error = result.err().expect("refused");
    assert!(error.contains("build volume"), "{error}");
    assert!(started.elapsed() < Duration::from_secs(10));
}

/// The cut tool named its object by index: after Select all (or anything that moved the
/// primary selection or the object order) it stayed open and Cut cut an object that was not
/// the selected one.
#[test]
fn the_cut_tool_never_cuts_another_object() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx, Some(CORE_ONE), &["cube20.stl", "hole_block.stl"]);
    app.select(Some(1), false);
    app.toggle_cut();
    assert_eq!(app.cut_target(), Some(1));
    app.run_command(Command::SelectAll, &ctx);
    assert!(app.cut.is_none(), "a new primary selection closes the cut");
    // The object order shifts under an open cut: the tool closes instead of cutting another.
    app.select(Some(1), false);
    app.toggle_cut();
    let cube = app.plates.current().objects[0].clone();
    app.plates.current_mut().objects.swap(0, 1);
    assert_eq!(app.cut_target(), None);
    app.apply_cut();
    assert_eq!(app.plates.current().objects.len(), 2, "nothing was cut");
    assert_eq!(app.plates.current().objects[1].id, cube.id);
}

/// Painting, laying on a face or picking on another plate moved the primary selection
/// directly and left the old object's selected modifier behind; undo/redo then restored a
/// different selection than the one recorded.
#[test]
fn a_new_primary_object_drops_the_old_ones_selected_part() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx, Some(CORE_ONE), &["cube20.stl", "hole_block.stl"]);
    app.select(Some(0), false);
    app.run_command(Command::AddModifier(Shape::Box), &ctx);
    assert!(app.sub.is_some());
    app.handle(Action::LayOnFace { index: 1, face: 0 }, &map(&app), &ctx);
    assert_eq!(app.selected, Some(1));
    assert_eq!(app.sub, None);
}

/// Orca has no percentages for several PrusaSlicer "mm/s or %" options: an object's
/// `top_solid_infill_speed = 50%` was written as Orca's `top_surface_speed = 50%`, which the
/// importer rejects, so the saved project could not be opened again.
#[test]
fn orca_projects_with_percent_speeds_open_again() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx, Some(CORE_ONE), &["cube20.stl"]);
    app.plates.current_mut().objects[0]
        .overrides
        .insert("top_solid_infill_speed".into(), "50%".into());
    let path = temp_dir("orca").join("percent.3mf");
    app.save_project_to(&path, mica_core::threemf::Flavour::Orca)
        .unwrap();
    let reopened = crate::app::load_files(vec![path], &app.config, app.bed.center());
    let scene = reopened.expect("the project opens again");
    let o = &scene.objects[0].1;
    let speed = o
        .overrides
        .get("top_solid_infill_speed")
        .or_else(|| o.overrides.get("top_surface_speed"))
        .expect("the speed survives");
    assert!(!speed.ends_with('%'), "{speed}");
    assert!(speed.parse::<f64>().is_ok_and(|v| v > 0.), "{speed}");
}

/// The settings table had PrusaSlicer's `def->max = 0; def->max = 300;` typo read as a
/// maximum of 0: the first-layer bed temperature field clamped every typed value to 0 °C.
#[test]
fn the_first_layer_bed_temperature_field_goes_up_to_300() {
    let text = &crate::settings::text()["first_layer_bed_temperature"];
    assert_eq!(text.max, Some(300.));
    assert_eq!(crate::settings::text()["min_bead_width"].max, None);
}

/// On the U1 every tool carried the first filament's Orca-only filament options
/// (`nozzle_temperature_range_low/high`, …): a PLA tool next to a PETG one was judged
/// against the PETG range, and `mica validate` failed its G-code as cold extrusion.
#[test]
fn every_u1_tool_keeps_its_own_filament_options() {
    let ctx = egui::Context::default();
    let mut u1 = app(&ctx, None, &[]);
    let Some((vendor, _, name)) = u1
        .profiles
        .all_printers()
        .into_iter()
        .find(|(_, _, n)| n.contains("Snapmaker U1") && n.contains("0.4"))
    else {
        eprintln!("no Snapmaker U1 profiles installed; skipped");
        return;
    };
    u1.profiles.select_printer(vendor, &name);
    u1.settings_changed();
    let materials = u1.profiles.materials.clone();
    let find = |want: &str, not: &str| {
        materials
            .iter()
            .find(|m| m.contains(want) && !m.contains(not))
            .cloned()
    };
    let (Some(petg), Some(pla)) = (find("PETG", "CF"), find("PLA", "PETG")) else {
        eprintln!("no PETG and PLA filaments for the U1; skipped");
        return;
    };
    u1.profiles.filaments[0] = petg.clone();
    u1.profiles.filaments[1] = pla.clone();
    u1.profiles.refresh();
    u1.settings_changed();
    let c = u1.config.clone().unwrap();
    let low = |tool: usize| -> Option<f64> {
        c.float_at("nozzle_temperature_range_low", tool)
            .or_else(|| {
                c.extra("nozzle_temperature_range_low")?
                    .split(',')
                    .nth(tool)?
                    .trim()
                    .parse()
                    .ok()
            })
    };
    let alone = |f: &str| {
        let mut solo = app(&ctx, None, &[]);
        solo.profiles.select_printer(vendor, &name);
        solo.profiles.filaments[0] = f.to_string();
        solo.profiles.refresh();
        solo.settings_changed();
        let c = solo.config.clone().unwrap();
        c.float_at("nozzle_temperature_range_low", 0).or_else(|| {
            c.extra("nozzle_temperature_range_low")?
                .split(',')
                .next()?
                .trim()
                .parse()
                .ok()
        })
    };
    assert_eq!(low(0), alone(&petg), "{petg}");
    assert_eq!(low(1), alone(&pla), "{pla}");
}

/// Review find (external data): a 3MF model part without triangles underflowed
/// `first + faces - 1` when the object's painting was split back onto its parts for slicing.
#[test]
fn a_model_part_without_triangles_slices_its_painting_safely() {
    let ctx = egui::Context::default();
    let app = app(&ctx, Some(CORE_ONE), &["cube20.stl"]);
    let mut o = app.plates.plate(0).objects[0].clone();
    let part = |mesh: mica_core::mesh::TriangleMesh| mica_core::regions::Volume {
        mesh,
        kind: mica_core::threemf::PartKind::Model,
        config: mica_core::config::Config::new(),
        painting: Default::default(),
    };
    o.region = Some(std::sync::Arc::new(mica_core::regions::RegionModel {
        volumes: vec![
            part(mica_core::mesh::TriangleMesh::default()),
            part((*o.mesh).clone()),
        ],
        height_ranges: vec![],
        ..Default::default()
    }));
    o.painting_edited = true;
    let model = o.placed_region().expect("a regions model");
    assert_eq!(model.volumes.len(), 2);
}

/// GUI monkey test, seeds 107 and 112: an object whose regions model sets Brick layers (or
/// textures) while that feature is off was given a fresh copy of the model in every slice
/// job, and jobs compare regions by identity, so a finished slice never fitted its plate (it
/// looked stale at once and the slice cache never found it).
#[test]
fn a_slice_fits_its_plate_with_a_feature_off_that_its_parts_use() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx, Some(CORE_ONE), &["cube20.stl"]);
    let o = &mut app.plates.current_mut().objects[0];
    let mut config = mica_core::config::Config::new();
    config.set_raw("mica_brick_layers", "1").unwrap();
    config.set_raw("mica_texture_skin", "external").unwrap();
    o.region = Some(std::sync::Arc::new(mica_core::regions::RegionModel {
        volumes: vec![mica_core::regions::Volume {
            mesh: (*o.mesh).clone(),
            kind: mica_core::threemf::PartKind::Model,
            config,
            painting: Default::default(),
        }],
        ..Default::default()
    }));
    for (textures, brick_layers) in [(true, false), (false, true), (false, false)] {
        app.prefs.features.textures = textures;
        app.prefs.features.brick_layers = brick_layers;
        let a = app.slice_inputs(0).unwrap();
        let b = app.slice_inputs(0).unwrap();
        assert!(
            a.fit(&b) == Some(crate::slice_cache::Fit::Exact),
            "textures {textures}, brick layers {brick_layers}: {}",
            a.difference(&b)
        );
    }
}

/// The monkey types random text into printer address fields: with the loopback restriction
/// on, nothing but this computer is ever contacted.
#[test]
fn loopback_restriction_refuses_every_other_address() {
    use crate::printhost::is_loopback;
    for a in [
        "127.0.0.1:18081",
        "localhost",
        "http://127.0.0.1/x",
        "[::1]:80",
        "LOCALHOST:1",
    ] {
        assert!(is_loopback(a), "{a}");
    }
    for a in [
        "printer.local",
        "198.51.100.5",
        "0",
        "abc",
        "http://example.com",
        "127.0.0.1.evil.com",
        "",
    ] {
        assert!(!is_loopback(a), "{a}");
    }
    crate::printhost::restrict_to_loopback();
    let ctx = egui::Context::default();
    let mut app = app(&ctx, None, &[]);
    let mut p = crate::printhost::config::Printer::new(crate::printhost::config::Kind::PrusaLink);
    p.name = "far".into();
    p.address = "198.51.100.5".into();
    let i = app.printers.put(None, p, None).unwrap();
    let error = app.printers.connector(i).err().expect("refused");
    assert!(
        error.contains("only talks to printers on this computer"),
        "{error}"
    );
}

/// Monkey seed 0: a temperature tower for tool 2's PETG on the U1 was judged against the wrong
/// filament (tool 1's PLA limits) and reported thousands of "hotend exceeds filament maximum"
/// errors. The test now prints on tool 2 itself (T1 picked up and heated, the others cold),
/// and its G-code validates cleanly against `App::job_config`, the plate's configuration.
#[test]
fn a_calibration_plate_is_validated_against_the_filament_it_tests() {
    let ctx = egui::Context::default();
    let mut u1 = app(&ctx, None, &[]);
    let Some((vendor, _, name)) = u1
        .profiles
        .all_printers()
        .into_iter()
        .find(|(_, _, n)| n.contains("Snapmaker U1") && n.contains("0.4"))
    else {
        eprintln!("no Snapmaker U1 profiles installed; skipped");
        return;
    };
    u1.profiles.select_printer(vendor, &name);
    u1.settings_changed();
    let materials = u1.profiles.materials.clone();
    let find = |want: &str, not: &str| {
        materials
            .iter()
            .find(|m| m.contains(want) && !m.contains(not))
            .cloned()
    };
    let (Some(petg), Some(pla)) = (find("PETG", "CF"), find("PLA", "PETG")) else {
        eprintln!("no PETG and PLA filaments for the U1; skipped");
        return;
    };
    u1.profiles.filaments[0] = pla;
    u1.profiles.filaments[1] = petg;
    u1.profiles.refresh();
    u1.settings_changed();
    u1.open_calibration(crate::calibration::Test::Temperature);
    let config = u1.config.clone().unwrap();
    u1.dialogs
        .calibration
        .as_mut()
        .unwrap()
        .set_tool(1, &config);
    let plate = u1
        .create_calibration_plate(&ctx)
        .expect("calibration plate");
    wait_idle(&mut u1, &ctx);
    let finished = u1.plates.plate(plate).finished.as_ref().expect(&u1.status);
    let gcode = finished.preview.job.path.join("print.gcode");
    let job = u1.job_config(plate).unwrap();
    assert_eq!(
        job.float_at("temperature", 0),
        config.float_at("temperature", 0)
    );
    let text = std::fs::read_to_string(&gcode).unwrap();
    let selects: std::collections::BTreeSet<&str> = text
        .lines()
        .filter(|l| l.len() == 2 && l.starts_with('T'))
        .collect();
    assert_eq!(selects, ["T1"].into(), "the PETG tool prints the tower");
    assert!(text.contains("T1 ; calibration temperature"));
    let profile = mica_gcode::validate::Profile::from_config(&job).unwrap();
    let report = mica_gcode::validate::validate_file(&gcode, &profile).unwrap();
    let errors: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| matches!(d.severity, mica_gcode::validate::Severity::Error))
        .map(|d| format!("line {}: {} {}", d.line, d.rule, d.message))
        .take(5)
        .collect();
    assert_eq!(report.errors, 0, "{errors:?}");
}

/// Applies `edits` to `target` as one user edit (one undo step).
fn edit(app: &mut App, target: Option<Target>, edits: Vec<Edit>) {
    for e in edits {
        app.apply_edit(target, e);
    }
    app.end_gesture();
}

fn set(key: &str, raw: &str) -> Vec<Edit> {
    vec![Edit::Set {
        key: key.into(),
        raw: raw.into(),
    }]
}

/// Choosing a supports mode in the print settings, as the dropdown does.
fn supports(app: &mut App, mode: &str) {
    let edits = crate::panels::editor::support_mode_edits(mode, &app.overlays.print);
    edit(app, Some(Target::Print), edits);
}

/// Supports Everywhere (or Enforcers only) and back to Off left the settings changed: Off
/// kept what the on-mode had written, and a value set back to the profile's stayed an edit.
#[test]
fn settings_set_back_to_the_profile_are_no_longer_changed() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx, Some(CORE_ONE), &[]);
    let c = app.effective_config().unwrap();
    let flag = |k| c.bool(k).unwrap();
    let profile = match (
        flag("support_material"),
        flag("support_material_auto"),
        flag("support_material_buildplate_only"),
    ) {
        (false, _, _) => "off",
        (true, false, _) => "enforcers",
        (true, true, false) => "everywhere",
        (true, true, true) => "plate",
    };
    // Every mode and back to the profile's: nothing changed.
    for mode in ["off", "enforcers", "everywhere", "plate"] {
        if mode == profile {
            continue;
        }
        supports(&mut app, mode);
        assert!(!app.overlays.print.is_empty(), "{mode} is a change");
        supports(&mut app, profile);
        assert!(
            app.overlays.print.is_empty(),
            "{mode} to {profile}: {:?}",
            app.overlays.print
        );
    }
    // Off, an on-mode and Off again: only Off's own change stays.
    supports(&mut app, "off");
    let off = app.overlays.print.clone();
    for mode in ["enforcers", "everywhere", "plate"] {
        supports(&mut app, mode);
        supports(&mut app, "off");
        assert_eq!(app.overlays.print, off, "off, {mode}, off");
    }
    supports(&mut app, profile);
    assert!(app.overlays.print.is_empty());

    // A number typed back; undo still steps through it.
    let density = app.effective_config().unwrap().raw("fill_density").unwrap();
    edit(&mut app, Some(Target::Print), set("fill_density", "37%"));
    assert!(app.overlays.print.contains_key("fill_density"));
    edit(&mut app, Some(Target::Print), set("fill_density", &density));
    assert!(app.overlays.print.is_empty());
    app.undo();
    assert_eq!(app.overlays.print["fill_density"], "37%");
    app.undo();
    assert!(app.overlays.print.is_empty());

    // A merged setting: the Orca key and its PrusaSlicer counterpart go together (under
    // OrcaSlicer rules, where the setting shows).
    let rules = mica_core::rules::KEY;
    edit(&mut app, Some(Target::Printer), set(rules, "orcaslicer"));
    assert!(app.overlays.printer.contains_key(rules));
    let c = app.effective_config().unwrap();
    let (wall, outer_first) = (
        mica_core::merged::orca_raw(c, "wall_sequence").unwrap(),
        c.raw("external_perimeters_first").unwrap(),
    );
    let other = if wall == "outer wall/inner wall" {
        "inner wall/outer wall"
    } else {
        "outer wall/inner wall"
    };
    edit(&mut app, Some(Target::Print), set("wall_sequence", other));
    assert!(app.overlays.print.contains_key("wall_sequence"));
    assert!(app.overlays.print.contains_key("external_perimeters_first"));
    edit(&mut app, Some(Target::Print), set("wall_sequence", &wall));
    assert!(app.overlays.print.is_empty(), "{:?}", app.overlays.print);
    assert_eq!(
        app.effective_config()
            .unwrap()
            .raw("external_perimeters_first"),
        Some(outer_first)
    );
    edit(&mut app, Some(Target::Printer), set(rules, "prusaslicer"));
    assert!(app.overlays.printer.is_empty());

    // An object override set to the plate's value is no override.
    let result = crate::app::load_files(vec![model("cube20.stl")], &app.config, app.bed.center());
    app.finish_load(result);
    app.selected = Some(0);
    app.sub = None;
    let perimeters = app.effective_config().unwrap().raw("perimeters").unwrap();
    edit(&mut app, None, set("perimeters", "7"));
    assert_eq!(app.plates.current().objects[0].overrides["perimeters"], "7");
    edit(&mut app, None, set("perimeters", &perimeters));
    assert!(app.plates.current().objects[0].overrides.is_empty());
}

/// A filament value set back to its profile's: no change for that tool, the other tools'
/// edits stay.
#[test]
fn a_filament_value_set_back_leaves_the_other_tools_alone() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx, None, &[]);
    let (vendor, _, name) = app
        .profiles
        .all_printers()
        .into_iter()
        .find(|(_, _, n)| n.contains("XL") && n.contains("5T") && n.contains("0.4"))
        .expect("XL 5T preset");
    app.profiles.select_printer(vendor, &name);
    app.settings_changed();
    assert!(app.tool_count() >= 2);
    let own = |app: &App, tool| {
        crate::overlays::tool_raw(app.effective_config().unwrap(), "temperature", tool).unwrap()
    };
    let second = own(&app, 1);
    edit(
        &mut app,
        Some(Target::Filament(0)),
        set("temperature", "211"),
    );
    edit(
        &mut app,
        Some(Target::Filament(1)),
        set("temperature", "212"),
    );
    assert_eq!(own(&app, 1), "212");
    edit(
        &mut app,
        Some(Target::Filament(1)),
        set("temperature", &second),
    );
    assert!(app.overlays.filaments[1].is_empty());
    assert_eq!(app.overlays.filaments[0]["temperature"], "211");
    assert_eq!(own(&app, 0), "211");
    assert_eq!(own(&app, 1), second);
}

/// GUI monkey test, seed 3004: a bed shape typed on the XL with extruder 2 selected in the
/// printer editor was written as that extruder's point (`0x0,0x0,360x360,0x360`, a triangle
/// that left the models outside the bed), and the monkey's printer switch kept it for the
/// CORE One. A bed shape is one value; a damaged one is refused with a notification; a
/// printer switch drops the old printer's edits.
#[test]
fn a_bed_shape_is_one_value_and_stays_with_its_printer() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx, Some("Original Prusa XL - 5T 0.4 nozzle"), &[]);
    app.editor.printer_extruder = 1;
    let bed = |app: &App| app.effective_config().unwrap().raw("bed_shape").unwrap();
    edit(
        &mut app,
        Some(Target::Printer),
        set("bed_shape", "0x0,250x0,250x220,0x220"),
    );
    assert_eq!(bed(&app), "0x0,250x0,250x220,0x220");
    // Per-extruder options still take the selected extruder's element.
    edit(
        &mut app,
        Some(Target::Printer),
        set("retract_length", "1.5"),
    );
    let c = app.effective_config().unwrap();
    assert_eq!(c.float_at("retract_length", 1), Some(1.5));
    assert_ne!(c.float_at("retract_length", 0), Some(1.5));

    // A damaged bed is refused, visibly.
    let shown = |app: &App| {
        app.notifications
            .visible(None)
            .iter()
            .any(|n| n.text.contains("bed shape was not changed"))
    };
    assert!(!shown(&app));
    edit(
        &mut app,
        Some(Target::Printer),
        set("bed_shape", "0x0,0x0,360x360,0x360"),
    );
    assert_eq!(bed(&app), "0x0,250x0,250x220,0x220");
    assert!(shown(&app));

    // Another printer: its own bed.
    let (vendor, _, core) = app
        .profiles
        .all_printers()
        .into_iter()
        .find(|(_, _, n)| n == "Prusa CORE One HF0.4 nozzle")
        .unwrap();
    app.switch_printer(vendor, &core);
    assert!(app.overlays.printer.is_empty());
    let own = app.profiles.config().unwrap().raw("bed_shape").unwrap();
    assert_eq!(bed(&app), own);
}
