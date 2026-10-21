// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The plate bar and the plate menu driven through the whole window: the menu opens from the
//! bar's ⋯ button (no hovering over a plate), renaming in place (click, F2, Enter, Esc), moving
//! plates as one undo step (menu and plate list drag), and the object list's plate menu.

use crate::app::{App, Options};
use crate::plate_menu;
use eframe::egui::{self, Event, Key, Modifiers, PointerButton, Pos2, RawInput};
use mica_profiles::user::Store;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT: AtomicUsize = AtomicUsize::new(0);

/// A window with cube20 on plate 1 and an empty plate 2, plate 1 current.
fn app() -> (egui::Context, App) {
    let ctx = egui::Context::default();
    let dir = std::env::temp_dir().join(format!(
        "mica-plates-ui-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let mut app = App::create(
        &ctx,
        Options {
            prefs_path: None,
            store: Some(Store::open(dir)),
            files: vec![
                PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/models/"))
                    .join("cube20.stl"),
            ],
            headless: true,
        },
    );
    for _ in 0..40 {
        frame(&ctx, &mut app, vec![]);
        if app.loading.is_none() && !app.plates.current().objects.is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    app.plates.add();
    app.select_plate(0);
    frame(&ctx, &mut app, vec![]);
    frame(&ctx, &mut app, vec![]);
    (ctx, app)
}

fn frame(ctx: &egui::Context, app: &mut App, events: Vec<Event>) {
    let input = RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1440., 900.),
        )),
        events,
        ..Default::default()
    };
    let mut output = ctx.run_ui(input, |ui| app.show(ui));
    output.textures_delta.clear();
}

fn click_with(ctx: &egui::Context, app: &mut App, at: Pos2, button: PointerButton) {
    frame(ctx, app, vec![Event::PointerMoved(at)]);
    let ev = |pressed| Event::PointerButton {
        pos: at,
        button,
        pressed,
        modifiers: Modifiers::NONE,
    };
    frame(ctx, app, vec![ev(true)]);
    frame(ctx, app, vec![ev(false)]);
    frame(ctx, app, vec![]);
}

fn click(ctx: &egui::Context, app: &mut App, at: Pos2) {
    click_with(ctx, app, at, PointerButton::Primary);
}

fn press(ctx: &egui::Context, app: &mut App, key: Key) {
    let ev = |pressed| Event::Key {
        key,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers: Modifiers::NONE,
    };
    frame(ctx, app, vec![ev(true)]);
    frame(ctx, app, vec![ev(false)]);
}

fn type_text(ctx: &egui::Context, app: &mut App, text: &str) {
    frame(ctx, app, vec![Event::Text(text.into())]);
}

/// Picks `label` from the plate menu that is open now.
fn pick(ctx: &egui::Context, app: &mut App, label: &str) {
    let rect = plate_menu::probe(ctx, label).unwrap_or_else(|| panic!("no menu row {label}"));
    click(ctx, app, rect.center());
}

fn names(app: &App) -> Vec<String> {
    app.plates.plates.iter().map(|p| p.name.clone()).collect()
}

#[test]
fn the_plate_menu_opens_from_the_plate_bar_without_hovering_a_plate() {
    let (ctx, mut a) = app();
    let menu = a
        .viewport
        .plate_probe
        .menu
        .expect("the plate bar's ⋯ button");
    plate_menu::clear_probe(&ctx);
    click(&ctx, &mut a, menu.center());
    // The pointer is on the bar, nowhere near a plate label.
    assert_eq!(a.viewport.hovered_plate, None);
    for label in [
        "Rename plate…",
        "Plate settings…",
        "Arrange objects on this plate",
        "Duplicate plate",
        "Move plate right",
        "Reorder plates…",
        "Delete plate",
    ] {
        assert!(plate_menu::probe(&ctx, label).is_some(), "{label}");
    }
    pick(&ctx, &mut a, "Duplicate plate");
    assert_eq!(names(&a), ["Plate 1", "Plate 2", "Plate 3"]);
    assert_eq!(a.plates.current, 1);
    assert_eq!(a.plates.plate(1).objects.len(), 1);
    assert_ne!(
        a.plates.plate(1).objects[0].object_id,
        a.plates.plate(0).objects[0].object_id
    );
    // Plate settings opens the sidebar's "This plate" scope.
    plate_menu::clear_probe(&ctx);
    click(&ctx, &mut a, menu.center());
    pick(&ctx, &mut a, "Plate settings…");
    assert!(a.editor.plate_mode);
}

#[test]
fn the_name_renames_in_place_enter_keeps_esc_cancels_and_empty_resets() {
    let (ctx, mut a) = app();
    let name = a.viewport.plate_probe.name.expect("the plate bar's name");
    click(&ctx, &mut a, name.center());
    assert!(a.viewport.plate_rename.is_some());
    frame(&ctx, &mut a, vec![]);
    // The whole name is selected: typing replaces it.
    type_text(&ctx, &mut a, "Gift for Tom");
    press(&ctx, &mut a, Key::Enter);
    frame(&ctx, &mut a, vec![]);
    assert_eq!(a.plates.current().name, "Gift for Tom");
    assert!(a.viewport.plate_rename.is_none());
    // One undo step.
    a.undo();
    assert_eq!(a.plates.current().name, "Plate 1");
    a.redo();
    assert_eq!(a.plates.current().name, "Gift for Tom");
    // F2 opens the field; Esc leaves the name as it was (and keeps the selection).
    a.select(Some(0), false);
    frame(&ctx, &mut a, vec![]);
    press(&ctx, &mut a, Key::F2);
    frame(&ctx, &mut a, vec![]);
    assert!(a.viewport.plate_rename.is_some());
    type_text(&ctx, &mut a, "Something else");
    press(&ctx, &mut a, Key::Escape);
    frame(&ctx, &mut a, vec![]);
    assert!(a.viewport.plate_rename.is_none());
    assert_eq!(a.plates.current().name, "Gift for Tom");
    assert_eq!(a.selected, Some(0));
    // An empty name is the plate's number again.
    press(&ctx, &mut a, Key::F2);
    frame(&ctx, &mut a, vec![]);
    press(&ctx, &mut a, Key::Backspace);
    press(&ctx, &mut a, Key::Enter);
    frame(&ctx, &mut a, vec![]);
    assert_eq!(a.plates.current().name, "Plate 1");
    assert_eq!(
        a.plates.current().objects.len(),
        1,
        "Backspace deleted nothing"
    );
}

#[test]
fn moving_a_plate_is_one_undo_step_and_keeps_its_objects_and_settings() {
    let (ctx, mut a) = app();
    a.rename_plate(0, "Cube");
    a.plates.plates[0]
        .overrides
        .insert("curr_bed_type".into(), "Cool Plate".into());
    let id = a.plates.plate(0).id;
    let position = a.plates.plate(0).objects[0].position;
    let steps = a.undo.len();
    let menu = a.viewport.plate_probe.menu.unwrap();
    click(&ctx, &mut a, menu.center());
    pick(&ctx, &mut a, "Move plate right");
    assert_eq!(names(&a), ["Plate 1", "Cube"]);
    assert_eq!(a.undo.len(), steps + 1);
    // The moved plate stays current and keeps everything (positions are plate-local).
    assert_eq!(a.plates.current, 1);
    let moved = a.plates.plate(1);
    assert_eq!(moved.id, id);
    assert_eq!(moved.objects[0].position, position);
    assert_eq!(
        moved.overrides.get("curr_bed_type").map(String::as_str),
        Some("Cool Plate")
    );
    a.undo();
    assert_eq!(names(&a), ["Cube", "Plate 2"]);
    assert_eq!(a.plates.plate(0).objects.len(), 1);
    // An undo of a move keeps the untouched plates themselves (their slices too).
    assert_eq!(a.plates.plate(0).id, id);
    a.redo();
    assert_eq!(names(&a), ["Plate 1", "Cube"]);
}

#[test]
fn dragging_a_row_of_the_plate_list_reorders_the_plates() {
    let (ctx, mut a) = app();
    a.plates.add();
    a.rename_plate(2, "Last");
    a.select_plate(0);
    frame(&ctx, &mut a, vec![]);
    let list = a
        .viewport
        .plate_probe
        .list
        .expect("the plate bar's plate count");
    click(&ctx, &mut a, list.center());
    // The popup settles above the bar on its second frame.
    frame(&ctx, &mut a, vec![]);
    let rows = a.viewport.plate_probe.rows.clone();
    assert_eq!(rows.len(), 3, "the plate list is open");
    // Drag "Last" above plate 1.
    let from = rows[2].center();
    let to = Pos2::new(rows[0].center().x, rows[0].top() + 5.);
    let steps = a.undo.len();
    frame(&ctx, &mut a, vec![Event::PointerMoved(from)]);
    frame(
        &ctx,
        &mut a,
        vec![Event::PointerButton {
            pos: from,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        }],
    );
    for k in 1..=8 {
        let t = k as f32 / 8.;
        frame(&ctx, &mut a, vec![Event::PointerMoved(from.lerp(to, t))]);
    }
    frame(
        &ctx,
        &mut a,
        vec![Event::PointerButton {
            pos: to,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        }],
    );
    frame(&ctx, &mut a, vec![]);
    assert_eq!(names(&a), ["Last", "Plate 2", "Plate 3"]);
    assert_eq!(a.undo.len(), steps + 1);
    // Plate 1 (with the cube) is now second, and still current.
    assert_eq!(a.plates.current, 1);
    assert_eq!(a.plates.current().objects.len(), 1);
}

#[test]
fn the_object_list_header_has_the_plate_menu() {
    let (ctx, mut a) = app();
    let rect = a
        .editor
        .tree
        .plate_title
        .expect("the object list header is drawn");
    plate_menu::clear_probe(&ctx);
    click_with(&ctx, &mut a, rect.center(), PointerButton::Secondary);
    pick(&ctx, &mut a, "Lock plate");
    assert!(a.plates.plate(0).locked);
}

#[test]
fn the_export_and_print_file_name_carries_the_plate_name() {
    let (_ctx, mut a) = app();
    assert_eq!(a.export_stem(), "cube20_plate_1");
    a.rename_plate(0, "Gift / Tom");
    assert_eq!(a.export_stem(), "cube20_gift___tom");
}
