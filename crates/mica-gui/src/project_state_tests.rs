// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! App-level tests of project state: a plate's own settings (`plate_settings`), filament
//! colours as project state (`Overlays::colours`) and the slice cache (`slice_cache`).

use crate::app::{App, Options};
use crate::overlays::Target;
use crate::panels::editor::Edit;
use crate::slice_cache::{Fit, Job};
use eframe::egui;
use mica_core::threemf::Flavour;
use mica_profiles::user::Store;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);

fn app(ctx: &egui::Context) -> App {
    let dir = std::env::temp_dir().join(format!(
        "mica-state-store-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    App::create(
        ctx,
        Options {
            prefs_path: None,
            store: Some(Store::open(dir)),
            files: vec![],
            headless: true,
        },
    )
}

fn models() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/models/"))
}

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mica-state-test-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

fn open(app: &mut App, file: &str) {
    let result = crate::app::load_files(vec![models().join(file)], &app.config, app.bed.center());
    app.finish_load(result);
    assert!(!app.plates.is_blank(), "{}", app.status);
}

/// Selects printer `name`; `false` (a reported skip) when the pinned profiles of its vendor
/// are not fetched.
fn select(app: &mut App, name: &str) -> bool {
    let vendor = if name.starts_with("Snapmaker") {
        "Snapmaker"
    } else {
        "Prusa"
    };
    if !mica_testkit::require_vendor(vendor) {
        return false;
    }
    let (vi, _, found) = app
        .profiles
        .all_printers()
        .into_iter()
        .find(|(_, _, n)| n == name)
        .unwrap_or_else(|| panic!("{name} is not among the loaded printers"));
    app.profiles.select_printer(vi, &found);
    app.settings_changed();
    true
}

fn xl(app: &mut App) -> String {
    let name = app
        .profiles
        .all_printers()
        .into_iter()
        .map(|(_, _, n)| n)
        .find(|n| n.contains("XL") && n.contains("5T") && n.contains("0.4") && n.contains("Input"))
        .expect("XL 5T preset");
    assert!(select(app, &name));
    name
}

fn reopen(ctx: &egui::Context, path: &Path, printer: &str) -> App {
    let mut again = app(ctx);
    assert!(select(&mut again, printer));
    let result =
        crate::app::load_files(vec![path.to_path_buf()], &again.config, again.bed.center());
    again.finish_load(result);
    assert!(!again.plates.is_blank(), "{}", again.status);
    again
}

/// Slices plate `i` synchronously and hands the result over as a finished job would.
fn slice_now(a: &mut App, i: usize) {
    let inputs = Arc::new(a.slice_inputs(i).unwrap());
    let cancel = AtomicBool::new(false);
    let f = match &inputs.job {
        Job::Objects(objects, custom) => crate::worker::slice_with_custom_gcode(
            objects.clone(),
            custom.clone(),
            inputs.config.clone(),
            &cancel,
            &|_| {},
        ),
        Job::Calibration(p) => {
            crate::worker::slice_calibration(p, inputs.config.clone(), &cancel, &|_| {})
        }
    }
    .unwrap();
    a.store_slice(i, Some(inputs), f);
}

fn shown(a: &App, i: usize) -> Option<*const crate::preview::Preview> {
    a.plates
        .plate(i)
        .finished
        .as_ref()
        .map(|f| Arc::as_ptr(&f.preview))
}

fn move_object(a: &mut App, dx: f32) {
    a.record("Move");
    a.plates.current_mut().objects[0].position.x += dx;
    a.changed();
}

/// A plate's own settings (U1: bed type, spiral vase, any other setting) apply to that plate
/// only, are edited like overrides, and survive an Orca save (in Orca's own plate keys, which
/// OrcaSlicer reads natively) and a Prusa save (Mica's marker).
#[test]
fn plate_settings_apply_to_their_plate_and_round_trip() {
    let ctx = egui::Context::default();
    let mut a = app(&ctx);
    let u1 = "Snapmaker U1 (0.4 nozzle)";
    if !select(&mut a, u1) {
        return;
    }
    open(&mut a, "cube20.stl");
    a.plates.add();
    open(&mut a, "cube20.stl");
    assert_eq!(a.plates.current, 1);
    let global_bed = a.profiles.bed.clone();
    let bed = crate::panels::profiles::SNAPMAKER_BEDS
        .iter()
        .find(|b| **b != global_bed)
        .unwrap()
        .to_string();
    a.editor.plate_mode = true;
    for (key, raw) in [
        ("curr_bed_type", bed.as_str()),
        ("spiral_vase", "1"),
        ("layer_height", "0.12"),
    ] {
        a.apply_plate_edit(Edit::Set {
            key: key.into(),
            raw: raw.into(),
        });
        a.end_gesture();
    }
    assert_eq!(a.plates.current().overrides.len(), 3);
    // The plate slices with them; the other plate and the global settings do not change.
    let own = a.plate_config(1).unwrap();
    let other = a.plate_config(0).unwrap();
    assert_eq!(own.float("layer_height"), Some(0.12));
    assert_eq!(own.bool("spiral_vase"), Some(true));
    assert_ne!(other.float("layer_height"), Some(0.12));
    assert_eq!(other.bool("spiral_vase"), Some(false));
    let on_bed = a.profiles.config_for_bed(&bed).unwrap();
    assert_eq!(
        own.raw("first_layer_bed_temperature"),
        on_bed.raw("first_layer_bed_temperature")
    );
    assert_eq!(
        other.raw("first_layer_bed_temperature"),
        a.config
            .as_ref()
            .unwrap()
            .raw("first_layer_bed_temperature")
    );
    // An undo step per setting; undo takes the last one back.
    a.undo();
    assert!(!a.plates.current().overrides.contains_key("layer_height"));
    a.redo();
    assert!(a.is_modified());
    // Orca flavour: Orca's own plate keys (`bed_type`, `spiral_mode`), the rest prefixed.
    let path = temp("plates-own.3mf");
    a.save_project_to(&path, Flavour::Orca).unwrap();
    let saved = mica_core::threemf::load_3mf(&path).unwrap();
    let meta = &saved.plates[1].metadata;
    assert_eq!(meta.get("bed_type"), Some(&bed));
    assert_eq!(meta.get("spiral_mode").map(String::as_str), Some("true"));
    assert_eq!(
        meta.get("mica_override:layer_height").map(String::as_str),
        Some("0.12")
    );
    assert!(!saved.plates[0].metadata.contains_key("bed_type"));
    for flavour in [Flavour::Orca, Flavour::Prusa] {
        let path = temp(&format!("plates-own-{}.3mf", flavour.name()));
        a.save_project_to(&path, flavour).unwrap();
        let b = reopen(&ctx, &path, u1);
        assert_eq!(b.plates.len(), 2, "{}", flavour.name());
        assert_eq!(
            b.plates.plate(1).overrides,
            a.plates.plate(1).overrides,
            "{}",
            flavour.name()
        );
        assert!(b.plates.plate(0).overrides.is_empty());
        assert!(
            !b.plates.plate(1).metadata.contains_key("bed_type"),
            "applied keys are not kept twice"
        );
        assert_eq!(b.plate_config(1).unwrap().float("layer_height"), Some(0.12));
    }
}

/// A filament colour is project state: no changed marker, no change of the filament
/// profile, not written into one, saved with the project and not "changed" after reopening;
/// the slice stays, and Export re-slices only for the colours.
#[test]
fn filament_colour_is_project_state() {
    let ctx = egui::Context::default();
    let mut a = app(&ctx);
    let printer = xl(&mut a);
    open(&mut a, "cube20.stl");
    slice_now(&mut a, 0);
    let first = shown(&a, 0);
    let teal = [0x1E, 0x9C, 0xA0];
    a.apply_edit(
        Some(Target::Filament(1)),
        Edit::Set {
            key: "filament_colour".into(),
            raw: crate::project::hex(teal),
        },
    );
    a.end_gesture();
    assert!(
        a.overlays.filaments.iter().all(|o| o.is_empty()),
        "no filament change (no changed marker, nothing for the profile)"
    );
    assert_eq!(a.overlays.colour(1), Some("#1E9CA0"));
    assert_eq!(a.tool_colours()[1], teal);
    assert_eq!(
        a.config.as_ref().unwrap().string_at("filament_colour", 1),
        Some("#1E9CA0")
    );
    assert!(a.is_modified(), "the project changed");
    // The slice stays; only its colours lag.
    assert_eq!(shown(&a, 0), first);
    assert_eq!(a.slice_fit(0), Some(Fit::Colours));
    // Saving the filament as a profile does not take the colour into it.
    let parent = a.profiles.filament_colour(&a.profiles.filaments[1]);
    assert_ne!(parent, Some(teal));
    let name = format!("Colour test {}", std::process::id());
    a.save_profile(crate::panels::profiles::PresetKind::Filament, 1, &name)
        .unwrap();
    assert_eq!(a.overlays.colour(1), Some("#1E9CA0"), "the colour stays");
    assert_eq!(
        a.profiles.filament_colour(&a.profiles.filaments[1]),
        parent,
        "the profile keeps its own colour"
    );
    assert_eq!(a.tool_colours()[1], teal);
    // Save, reopen: the colours come back, nothing reads as changed.
    for flavour in [Flavour::Prusa, Flavour::Orca] {
        let path = temp(&format!("colour-{}.3mf", flavour.name()));
        a.save_project_to(&path, flavour).unwrap();
        let b = reopen(&ctx, &path, &printer);
        assert_eq!(b.tool_colours()[1], teal, "{}", flavour.name());
        assert!(
            b.overlays.filaments.iter().all(|o| o.is_empty()),
            "{}: a reopened project's colours are not filament changes: {:?}",
            flavour.name(),
            b.overlays.filaments
        );
        assert!(!b.is_modified(), "{}", flavour.name());
    }
    // Another preset for the tool: the colour stays unless that preset has its own.
    let other = a
        .profiles
        .materials
        .iter()
        .find(|m| **m != a.profiles.filaments[1])
        .cloned()
        .unwrap();
    let has_colour = a.profiles.filament_colour(&other).is_some();
    a.switch_filament(1, other);
    assert_eq!(a.overlays.colour(1).is_some(), !has_colour);
}

/// Undo and redo get a completed slice back from the cache; a change on one plate keeps the
/// others' slices; a colour-only change keeps the slice and Export slices once more.
/// Per-layer custom G-code is part of the slice key: adding a colour change makes the slice
/// stale, and undo brings back the slice without it (never the other one).
#[test]
fn layer_custom_gcode_is_part_of_the_slice_key() {
    use mica_core::custom_gcode::{Item, Kind, Mode};
    let ctx = egui::Context::default();
    let mut a = app(&ctx);
    open(&mut a, "cube20.stl");
    slice_now(&mut a, 0);
    let plain = shown(&a, 0).unwrap();
    let before = a.slice_inputs(0).unwrap();
    a.edit_layer_gcode(
        crate::preview::layer_gcode::Edit::Put {
            item: Item::new(5.0, Kind::ColorChange, 1),
            mode: Mode::SingleExtruder,
            label: "Add color change",
        },
        &ctx,
    );
    assert_ne!(a.slice_inputs(0).unwrap(), before);
    assert_ne!(a.slice_fit(0), Some(Fit::Exact));
    slice_now(&mut a, 0);
    let coloured = shown(&a, 0).unwrap();
    assert_ne!(coloured, plain);
    a.undo();
    assert!(a.plates.plate(0).custom_gcode.is_empty());
    assert_eq!(shown(&a, 0), Some(plain));
    a.redo();
    assert_eq!(a.plates.plate(0).custom_gcode.items.len(), 1);
    assert_eq!(shown(&a, 0), Some(coloured));
}

#[test]
fn undo_restores_cached_slices() {
    let ctx = egui::Context::default();
    let mut a = app(&ctx);
    open(&mut a, "cube20.stl");
    slice_now(&mut a, 0);
    let first = shown(&a, 0).unwrap();
    move_object(&mut a, 10.);
    assert!(a.finished().is_none());
    let hits = a.slice_cache.hits;
    a.undo();
    assert_eq!(shown(&a, 0), Some(first), "the slice is back at once");
    assert_eq!(a.slice_cache.hits, hits + 1);
    assert_eq!(a.slice_fit(0), Some(Fit::Exact));
    a.redo();
    assert!(a.finished().is_none(), "the moved state was never sliced");
    slice_now(&mut a, 0);
    let moved = shown(&a, 0).unwrap();
    a.undo();
    assert_eq!(shown(&a, 0), Some(first));
    a.redo();
    assert_eq!(shown(&a, 0), Some(moved));

    // Two plates: undoing on plate 2 keeps plate 1's slice (the same one, not a copy).
    a.plates.add();
    open(&mut a, "cube20.stl");
    slice_now(&mut a, 1);
    let second = shown(&a, 1).unwrap();
    move_object(&mut a, 5.);
    assert_eq!(shown(&a, 0), Some(moved));
    a.undo();
    assert_eq!(shown(&a, 0), Some(moved));
    assert_eq!(shown(&a, 1), Some(second));

    // A settings change and back: every plate gets its slice back.
    a.apply_edit(
        Some(Target::Print),
        Edit::Set {
            key: "perimeters".into(),
            raw: "5".into(),
        },
    );
    a.end_gesture();
    assert!(shown(&a, 0).is_none() && shown(&a, 1).is_none());
    a.undo();
    assert_eq!(shown(&a, 0), Some(moved));
    assert_eq!(shown(&a, 1), Some(second));

    // Colour only: the slices stay; Export slices again for the colours, and undo makes the
    // kept slice exact again.
    a.apply_edit(
        Some(Target::Filament(0)),
        Edit::Set {
            key: "filament_colour".into(),
            raw: "#FF00FF".into(),
        },
    );
    a.end_gesture();
    assert_eq!(shown(&a, 1), Some(second));
    assert_eq!(a.slice_fit(1), Some(Fit::Colours));
    assert!(a.reslice_for_colours());
    assert!(a.finished().is_none() && a.reslice_requested);
    a.undo();
    assert_eq!(shown(&a, 1), Some(second));
    assert_eq!(a.slice_fit(1), Some(Fit::Exact));

    // Bounded: of the slices no plate shows, at most `MAX_SPARE` stay.
    a.plates.current = 0;
    for k in 0..6 {
        move_object(&mut a, 3. + k as f32);
        slice_now(&mut a, 0);
    }
    assert!(a.slice_cache.len() <= crate::slice_cache::MAX_SPARE + 2);
    assert!(a.slice_cache.spare_bytes() <= crate::slice_cache::MAX_SPARE_BYTES);
}

/// Settings the editor hides because Mica does not implement them (`settings::unimplemented`)
/// are still project state: a value set in a project is said (a guard rail), saved and read
/// back unchanged in both flavours.
#[test]
fn hidden_unimplemented_settings_round_trip() {
    let ctx = egui::Context::default();
    let mut a = app(&ctx);
    let printer = xl(&mut a);
    open(&mut a, "cube20.stl");
    let values = [
        ("interface_shells", "1"),
        ("gcode_comments", "1"),
        ("slicing_mode", "even_odd"),
    ];
    for (key, raw) in values {
        a.apply_edit(
            Some(Target::Print),
            Edit::Set {
                key: key.into(),
                raw: raw.into(),
            },
        );
    }
    a.end_gesture();
    a.refresh_warnings();
    let guard = a
        .guards
        .iter()
        .find(|g| g.message.starts_with("Not supported by Mica"))
        .expect("the unsupported values are reported");
    assert!(guard.message.contains("Even-odd"), "{}", guard.message);
    for flavour in [Flavour::Prusa, Flavour::Orca] {
        let path = temp(&format!("hidden-{}.3mf", flavour.name()));
        a.save_project_to(&path, flavour).unwrap();
        let b = reopen(&ctx, &path, &printer);
        let c = b.config.as_ref().unwrap();
        for (key, raw) in values {
            assert_eq!(
                c.raw(key).as_deref(),
                Some(raw),
                "{}: {key}",
                flavour.name()
            );
        }
    }
}

/// GUI monkey test (seed 1002): undo all + redo all lost both plates' prime tower placements.
/// Restoring a step whose printer overrides change the bed recomputed the configuration, and a
/// bed change drops every plate's tower placement, including the ones just restored.
#[test]
fn undo_across_a_bed_change_keeps_tower_placements() {
    let ctx = egui::Context::default();
    let mut a = app(&ctx);
    open(&mut a, "cube20.stl");
    // One step changes the bed and places the tower on it (as opening a project with its
    // printer settings and tower positions does).
    a.record("Open project");
    a.overlays
        .printer
        .insert("bed_shape".into(), "0x0,200x0,200x200,0x200".into());
    a.recompute_config();
    a.set_tower_placement(0, [120., 150., 0.]);
    let placed = Some([120., 150., 0.]);
    assert_eq!(a.plates.plates[0].tower_placement, placed);
    a.undo();
    assert!(!a.overlays.printer.contains_key("bed_shape"));
    a.redo();
    assert!(a.overlays.printer.contains_key("bed_shape"));
    assert_eq!(a.plates.plates[0].tower_placement, placed);
}

/// World bounds of the current plate's sliced extrusions as the renderer draws them
/// (`preview_model` of the scene's plate snapshot applied to the bed-coordinate records).
fn drawn_extrusion_bounds(a: &App) -> (glam::Vec3, glam::Vec3) {
    let snapshot = crate::scene::plates::snapshot(&a.plates);
    let model = crate::render::preview_model(&snapshot);
    let preview = &a.plates.current().finished.as_ref().unwrap().preview;
    let mut min = glam::Vec3::splat(f32::INFINITY);
    let mut max = -min;
    preview
        .read_batches(0..preview.count, |segments| {
            for s in segments
                .iter()
                .filter(|s| s.role() != crate::preview::TRAVEL)
            {
                for p in [s.a, s.b] {
                    let w = model.transform_point3(glam::Vec3::from(p));
                    min = min.min(w);
                    max = max.max(w);
                }
            }
        })
        .unwrap();
    assert!(min.x.is_finite(), "no extrusions in the preview");
    (min, max)
}

fn inside_plate(a: &App, i: usize, (min, max): (glam::Vec3, glam::Vec3)) -> bool {
    let origin = crate::scene::plates::snapshot(&a.plates)[i].origin;
    let (lo, hi) = crate::scene::plates::plate_bounds(&a.bed, origin);
    min.x >= lo.x - 0.01 && min.y >= lo.y - 0.01 && max.x <= hi.x + 0.01 && max.y <= hi.y + 0.01
}

/// The preview of plate N is drawn on plate N's bed (a U1 project with two plates once showed
/// plate 2's slice on plate 1's bed), also after the plate moves to another grid position.
#[test]
fn preview_draws_on_its_own_plate() {
    let ctx = egui::Context::default();
    let mut a = app(&ctx);
    if !select(&mut a, "Snapmaker U1 (0.4 nozzle)") {
        return;
    }
    open(&mut a, "plates-orca.3mf");
    assert_eq!(a.plates.len(), 2);
    // The project's unassigned "loose" object sits off the bed (its own test); plate 2 keeps
    // the lid.
    for p in &mut a.plates.plates {
        p.objects.retain(|o| o.name != "loose");
    }
    a.select_plate(1);
    slice_now(&mut a, 1);
    a.preview = true;
    let bounds = drawn_extrusion_bounds(&a);
    assert!(inside_plate(&a, 1, bounds), "{bounds:?} not on plate 2");
    assert!(!inside_plate(&a, 0, bounds), "{bounds:?} on plate 1");
    // Reordered into a 2×2 grid's last cell (behind, to the right): the preview follows.
    a.plates.add();
    a.plates.add();
    let moved = a.plates.plates.remove(1);
    a.plates.plates.push(moved);
    a.plates.current = 3;
    let bounds = drawn_extrusion_bounds(&a);
    assert!(inside_plate(&a, 3, bounds), "{bounds:?} not on plate 4");
    for i in 0..3 {
        assert!(
            !inside_plate(&a, i, bounds),
            "{bounds:?} on plate {}",
            i + 1
        );
    }
    // An opened G-code file shows on the single plate at the origin.
    assert_eq!(
        crate::render::preview_model(&crate::gcode_doc::single_plate()),
        glam::Mat4::IDENTITY
    );
}
