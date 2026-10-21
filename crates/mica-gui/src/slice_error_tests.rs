// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! A failed slice and a skirt or brim past the bed, end to end in the headless editor: the
//! warning before slicing (with Move inside), the failed slice's error state (no old totals or
//! preview, the plate's marker and notification, cleared by the next edit), the same words as
//! the CLI, and Move inside as one undo step after which the plate slices.

use crate::app::{App, Options};
use crate::notifications::{Action, Level, SLICE_ERROR};
use crate::overlays::Target;
use crate::panels::editor::Edit;
use eframe::egui;
use mica_profiles::user::Store;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

static NEXT: AtomicU64 = AtomicU64::new(0);
const CORE_ONE: &str = "Prusa CORE One HF0.4 nozzle";

fn model(name: &str) -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/models/")).join(name)
}

/// The CORE One with a 10 mm brim and a cube whose centre is at bed X 12, Y 110: its left
/// side 2 mm from the edge, as `mica slice cube20.stl --center 12,110 --set brim_width=10`.
fn editor(ctx: &egui::Context) -> App {
    let dir = std::env::temp_dir().join(format!(
        "mica-slice-error-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let mut app = App::create(
        ctx,
        Options {
            prefs_path: None,
            store: Some(Store::open(dir)),
            files: vec![],
            headless: true,
        },
    );
    let (vendor, _, name) = app
        .profiles
        .all_printers()
        .into_iter()
        .find(|(_, _, n)| n == CORE_ONE)
        .expect("printer preset");
    app.profiles.select_printer(vendor, &name);
    app.settings_changed();
    let result = crate::app::load_files(vec![model("cube20.stl")], &app.config, app.bed.center());
    app.finish_load(result);
    app.apply_edit(
        Some(Target::Print),
        Edit::Set {
            key: "brim_width".into(),
            raw: "10".into(),
        },
    );
    app.end_gesture();
    app
}

fn place(app: &mut App, x: f32, y: f32) {
    let o = &mut app.plates.current_mut().objects[0];
    let (min, max) = o.bounds();
    let centre = (min + max) * 0.5;
    o.position.x += x - centre.x;
    o.position.y += y - centre.y;
    app.changed();
}

fn wait_idle(app: &mut App, ctx: &egui::Context) {
    let deadline = Instant::now() + Duration::from_secs(300);
    while app.busy() && Instant::now() < deadline {
        app.poll(ctx);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(!app.busy(), "the job did not finish: {}", app.status);
}

fn slice(app: &mut App, ctx: &egui::Context) {
    app.slice_current(ctx);
    wait_idle(app, ctx);
    app.sync_notifications();
}

/// Before slicing: the warning names the object, the edge and the distance in the CLI's words,
/// offers Move inside, tints the object and marks the plate; nothing moves by itself.
#[test]
fn a_brim_past_the_bed_is_warned_about_before_slicing() {
    let ctx = egui::Context::default();
    let mut app = editor(&ctx);
    place(&mut app, 12., 110.);
    let before = app.plates.current().objects[0].position;
    assert_eq!(app.adhesion.len(), 1, "{:?}", app.warnings);
    let warning = app
        .warnings
        .iter()
        .find(|w| w.starts_with("The brim of cube20.stl would go "))
        .cloned()
        .unwrap_or_else(|| panic!("{:?}", app.warnings));
    assert!(
        warning.contains("mm past the left edge of the bed"),
        "{warning}"
    );
    app.sync_notifications();
    let n = app
        .notifications
        .visible(Some(app.plates.current().id))
        .into_iter()
        .find(|n| n.text == warning)
        .cloned()
        .expect("a warning notification");
    assert_eq!(n.level, Level::Warning);
    assert!(
        n.actions
            .iter()
            .any(|(l, a)| l == "Move inside" && matches!(a, Action::MoveInside { .. })),
        "{:?}",
        n.actions
    );
    assert_eq!(app.plate_alert(0), Some(Level::Warning));
    assert_eq!(
        app.plates.current().objects[0].position,
        before,
        "never moved by itself"
    );
    // Inside, with room for the brim: no warning.
    place(&mut app, 125., 110.);
    assert!(app.adhesion.is_empty());
    assert!(!app.warnings.iter().any(|w| w.contains("would go")));
    assert_eq!(app.plate_alert(0), None);
}

/// The failed slice: the plate forgets its old slice, keeps the error until the next edit,
/// shows it (header state, notification with Move inside, plate marker), in the CLI's words.
#[test]
fn a_failed_slice_shows_its_reason_until_the_next_edit() {
    let ctx = egui::Context::default();
    let mut app = editor(&ctx);
    place(&mut app, 125., 110.);
    slice(&mut app, &ctx);
    assert!(
        app.finished().is_some(),
        "{:?}",
        app.plates.current().slice_error
    );
    app.preview = true;
    place(&mut app, 12., 110.);
    slice(&mut app, &ctx);
    let plate = app.plates.current();
    let error = plate
        .slice_error
        .clone()
        .expect("the plate shows why its slice failed");
    assert!(
        plate.finished.is_none(),
        "no old slice is presented as current"
    );
    assert!(!app.preview, "the preview of the old slice is not shown");
    assert_eq!(error.short, "brim doesn't fit on the bed");
    assert!(
        error.message.starts_with("The brim of cube20.stl goes ")
            && error
                .message
                .contains("mm past the left edge of the bed; move it "),
        "{}",
        error.message
    );
    assert_eq!(app.plate_alert(0), Some(Level::Error));
    assert!(
        app.notifications
            .shown_or_dismissed(Some(plate.id), SLICE_ERROR)
    );
    let n = app
        .notifications
        .visible(Some(plate.id))
        .into_iter()
        .find(|n| n.level == Level::Error)
        .cloned()
        .expect("an error notification");
    assert_eq!(n.title, "Slice failed: brim doesn't fit on the bed");
    assert_eq!(n.text, error.message);
    assert!(n.actions.iter().any(|(l, _)| l == "Move inside"));
    // The same words as `mica slice` (both go through `PreparedPlate::finish`).
    let out = std::env::temp_dir().join(format!("mica-slice-error-{}.gcode", std::process::id()));
    let cli = mica_cli::run(
        &[
            "slice",
            model("cube20.stl").to_str().unwrap(),
            "--vendor",
            "Prusa",
            "--printer",
            CORE_ONE,
            "--print",
            &app.profiles.process,
            "--filament",
            &app.profiles.filaments[0],
            "--center",
            "12,110",
            "--set",
            "brim_width=10",
            "-o",
            out.to_str().unwrap(),
        ]
        .map(String::from),
    );
    assert_eq!(cli, Err(error.message.clone()));
    // Dismissed, it stays an error state (header, marker) and comes back on a click.
    app.notifications.dismiss(n.id);
    app.sync_notifications();
    assert!(app.plate_alert(0) == Some(Level::Error));
    app.focus_slice_error();
    app.sync_notifications();
    assert!(
        app.notifications
            .visible(Some(app.plates.current().id))
            .iter()
            .any(|n| n.level == Level::Error)
    );
    // The next edit clears it.
    place(&mut app, 13., 110.);
    app.sync_notifications();
    assert!(app.plates.current().slice_error.is_none());
    assert!(
        !app.notifications
            .visible(Some(app.plates.current().id))
            .iter()
            .any(|n| n.level == Level::Error)
    );
}

/// Move inside: one undo step, just enough to fit, after which the plate slices.
#[test]
fn move_inside_is_one_undo_step_and_the_plate_then_slices() {
    let ctx = egui::Context::default();
    let mut app = editor(&ctx);
    place(&mut app, 12., 110.);
    slice(&mut app, &ctx);
    assert!(app.plates.current().slice_error.is_some());
    let before = app.plates.current().objects[0].position;
    let id = app.plates.current().id;
    let object = app.plates.current().objects[0].id;
    app.run_notification_action(0, Action::MoveInside { plate: id, object }, &ctx);
    let after = app.plates.current().objects[0].position;
    let moved = after - before;
    assert!(moved.y.abs() < 1e-4, "{moved:?}");
    assert!(moved.x > 5. && moved.x < 10., "just enough: {moved:?}");
    assert!(app.adhesion.is_empty(), "{:?}", app.warnings);
    assert!(
        app.plates.current().slice_error.is_none(),
        "an edit clears the error"
    );
    app.undo();
    assert_eq!(
        app.plates.current().objects[0].position,
        before,
        "one undo step"
    );
    app.redo();
    assert_eq!(app.plates.current().objects[0].position, after);
    slice(&mut app, &ctx);
    assert!(
        app.finished().is_some(),
        "{:?}",
        app.plates.current().slice_error
    );
    // The command from the object's menu does the same.
    place(&mut app, 238., 110.);
    app.select(Some(0), false);
    app.run_command(crate::commands::Command::MoveInside, &ctx);
    assert!(app.adhesion.is_empty());
    let (_, max) = app.plates.current().objects[0].bounds();
    assert!(max.x > 239. && max.x < 241., "{max:?}");
}

/// A toolpath off a non-rectangular bed (the export's printable-area check, as PrusaSlicer
/// and Orca block export): the slice fails, Export and Print stay off and say why, and moved
/// onto the bed the plate slices.
#[test]
fn a_brim_off_a_diamond_bed_blocks_export_and_print() {
    let ctx = egui::Context::default();
    let mut app = editor(&ctx);
    app.apply_edit(
        Some(Target::Printer),
        Edit::Set {
            key: "bed_shape".into(),
            raw: "125x0,250x125,125x250,0x125".into(),
        },
    );
    app.end_gesture();
    // On the bed, inside its bounding box; the 10 mm brim crosses the lower-left edge.
    place(&mut app, 65., 82.);
    slice(&mut app, &ctx);
    let plate = app.plates.current();
    let error = plate
        .slice_error
        .clone()
        .unwrap_or_else(|| panic!("sliced: {:?}", app.warnings));
    assert!(plate.finished.is_none() && app.finished().is_none());
    assert_eq!(error.short, "a toolpath leaves the printable area");
    assert!(
        error
            .message
            .contains("the brim of cube20.stl is outside the printable volume")
            && error.message.contains("outside the bed outline"),
        "{}",
        error.message
    );
    assert_eq!(app.plate_alert(0), Some(Level::Error));
    let blocked = app.blocked_reason(false);
    assert!(
        blocked.starts_with("Can't export or print: ") && blocked.contains(&error.message),
        "{blocked}"
    );
    place(&mut app, 125., 125.);
    slice(&mut app, &ctx);
    assert!(
        app.finished().is_some(),
        "{:?}",
        app.plates.current().slice_error
    );
}
