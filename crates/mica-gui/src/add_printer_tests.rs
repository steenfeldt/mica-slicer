// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The Add printer window and My printers' cards driven through the whole (headless) app:
//! each screen draws, the buttons move between them, Setup writes the same `my_printers` and
//! `printers.toml` entries the old flow did (the secret only in the store), a found printer is
//! added in one click, and My printers' cards poll only while the window is open. Headless
//! apps never search the network, and every address here is a closed loopback port.

use crate::app::{App, Options};
use crate::panels::add_printer::{Connect, Screen};
use crate::printhost::{
    catalog::Tools,
    config::{self, Kind},
};
use eframe::egui::{self, Event, PointerButton, Pos2, RawInput};
use mica_profiles::user::Store;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT: AtomicUsize = AtomicUsize::new(0);

fn app() -> Option<(egui::Context, App)> {
    let ctx = egui::Context::default();
    let dir = std::env::temp_dir().join(format!(
        "mica-add-printer-ui-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let app = App::create(
        &ctx,
        Options {
            prefs_path: None,
            store: Some(Store::open(dir)),
            files: vec![],
            headless: true,
        },
    );
    // Without the pinned profiles there are no models to pick.
    if !(mica_testkit::require_prusa_profiles() && mica_testkit::require_orca_profiles()) {
        return None;
    }
    assert!(
        app.profiles.vendors.len() >= 2,
        "the Prusa and Orca vendors"
    );
    Some((ctx, app))
}

fn frame(ctx: &egui::Context, app: &mut App, events: Vec<Event>) {
    let input = RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            Pos2::ZERO,
            egui::vec2(1440., 900.),
        )),
        events,
        ..Default::default()
    };
    let mut output = ctx.run_ui(input, |ui| app.show(ui));
    output.textures_delta.clear();
}

fn click(ctx: &egui::Context, app: &mut App, at: Pos2) {
    frame(ctx, app, vec![Event::PointerMoved(at)]);
    let button = |pressed| Event::PointerButton {
        pos: at,
        button: PointerButton::Primary,
        pressed,
        modifiers: Default::default(),
    };
    frame(ctx, app, vec![button(true)]);
    frame(ctx, app, vec![button(false)]);
    frame(ctx, app, vec![]);
}

/// The window's top-left corner (it opens centred in the 1440 × 900 screen).
fn origin() -> Pos2 {
    let size = crate::panels::add_printer::SIZE;
    Pos2::new((1440. - size.x) / 2., (900. - size.y) / 2.)
}

#[test]
fn the_three_screens_draw_and_lead_to_each_other() {
    let Some((ctx, mut app)) = app() else { return };
    app.open_add_printer();
    for _ in 0..3 {
        frame(&ctx, &mut app, vec![]);
    }
    // A headless app does not search the network: it says so instead of finding nothing.
    assert!(matches!(
        app.add_printer.discovery.state,
        crate::printhost::discover::Search::Off(_)
    ));
    assert_eq!(app.add_printer.screen, Screen::Found);
    let o = origin();
    let size = crate::panels::add_printer::SIZE;
    // "Pick a model", bottom right of the "not listed" box.
    click(
        &ctx,
        &mut app,
        Pos2::new(
            o.x + size.x - 28. - 16. - 136. - 8. - 56.,
            o.y + size.y - 24. - 32.,
        ),
    );
    assert_eq!(app.add_printer.screen, Screen::Gallery);
    // The first card (featured: CORE One) opens Setup.
    let catalog = app
        .add_printer
        .catalog
        .as_ref()
        .expect("built for the gallery");
    let first = catalog.models[0].name.clone();
    assert_eq!(first, "Prusa CORE One");
    click(&ctx, &mut app, Pos2::new(o.x + 28. + 140., o.y + 300.));
    assert_eq!(app.add_printer.screen, Screen::Setup);
    let setup = app.add_printer.setup.as_ref().expect("setup");
    assert_eq!(
        app.add_printer.catalog.as_ref().unwrap().models[setup.model].name,
        first
    );
    assert_eq!(setup.name, "CORE One");
    // "← Back" returns to the gallery; Esc closes the window.
    click(
        &ctx,
        &mut app,
        Pos2::new(o.x + 300. + 28. + 20., o.y + size.y - 24. - 20.),
    );
    assert_eq!(app.add_printer.screen, Screen::Gallery);
    // Brand filter and search narrow the cards.
    app.add_printer.brand = Some("Snapmaker".into());
    app.add_printer.search = "u1".into();
    frame(&ctx, &mut app, vec![]);
    let shown = app
        .add_printer
        .catalog
        .as_ref()
        .unwrap()
        .search("u1", Some("Snapmaker"));
    assert_eq!(shown.len(), 1);
    frame(
        &ctx,
        &mut app,
        vec![Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        }],
    );
    assert!(!app.dialogs.add_printer);
}

#[test]
fn setup_writes_my_printers_and_a_printers_toml_entry_with_the_key_in_the_store() {
    let Some((ctx, mut app)) = app() else { return };
    app.open_add_printer();
    frame(&ctx, &mut app, vec![]);
    let catalog = app
        .add_printer
        .catalog
        .clone()
        .unwrap_or_else(|| crate::printhost::catalog::Catalog::build(&app.profiles));
    let xl = catalog
        .models
        .iter()
        .position(|m| m.name == "Prusa XL")
        .unwrap();
    app.add_printer.catalog = Some(catalog);
    let mut choice = app.add_printer.catalog.as_ref().unwrap().models[xl].default_choice();
    choice.tools = Tools {
        count: 5,
        mmu: None,
    };
    app.begin_setup(
        xl,
        Some(choice),
        Some(Connect {
            address: "127.0.0.1:9".into(),
            secret: "SECRET-KEY-1".into(),
            ..Connect::default()
        }),
        Screen::Gallery,
    );
    frame(&ctx, &mut app, vec![]);
    assert_eq!(app.add_printer.setup.as_ref().unwrap().name, "XL 5T");
    app.finish_setup();
    let preset = "Original Prusa XL - 5T Input Shaper 0.4 nozzle";
    assert!(
        app.prefs.has_printer("Prusa", preset),
        "{:?}",
        app.prefs.my_printers
    );
    assert_eq!(app.profiles.printer, preset);
    assert!(!app.dialogs.add_printer);
    assert!(app.dialogs.my_printers);
    let p = &app.printers.list[0];
    assert_eq!(p.name, "XL 5T");
    assert_eq!(p.kind, Kind::PrusaLink);
    assert_eq!(p.address, "127.0.0.1:9");
    assert_eq!(
        p.profile.as_deref(),
        Some(format!("Prusa/{preset}").as_str())
    );
    let reference = p.keychain.clone().expect("a Keychain reference");
    assert_eq!(
        app.printers.secrets.get(&reference).unwrap().as_deref(),
        Some("SECRET-KEY-1")
    );
    // What printers.toml would hold: the old flow's fields, never the key.
    let text = config::to_toml(&app.printers.list);
    assert!(!text.contains("SECRET-KEY-1"));
    let back = config::parse(&text).unwrap();
    assert_eq!(back[0].profile, p.profile);
    assert_eq!(back[0].kind, Kind::PrusaLink);
    // A model added without an address is slice only: My printers, no connection.
    let u1 = app
        .add_printer
        .catalog
        .as_ref()
        .unwrap()
        .models
        .iter()
        .position(|m| m.name == "Snapmaker U1")
        .unwrap();
    app.begin_setup(u1, None, None, Screen::Gallery);
    app.finish_setup();
    assert!(
        app.prefs
            .has_printer("Snapmaker", "Snapmaker U1 (0.4 nozzle)")
    );
    assert_eq!(app.printers.list.len(), 1);
    assert!(
        app.slice_only_profiles()
            .iter()
            .any(|(_, n)| n == "Snapmaker U1 (0.4 nozzle)")
    );
    // A second XL 5T gets a number.
    let mut choice = app.add_printer.catalog.as_ref().unwrap().models[xl].default_choice();
    choice.tools = Tools {
        count: 5,
        mmu: None,
    };
    app.begin_setup(xl, Some(choice), None, Screen::Gallery);
    assert_eq!(app.add_printer.setup.as_ref().unwrap().name, "XL 5T 2");
}

#[test]
fn a_found_printer_is_added_in_one_click_unless_it_needs_its_key() {
    let Some((ctx, mut app)) = app() else { return };
    app.open_add_printer();
    frame(&ctx, &mut app, vec![]);
    let found = |name: &str, needs_key: bool| mica_printhost::discovery::Discovered {
        name: name.into(),
        address: "127.0.0.1:9".into(),
        kind: mica_printhost::PrinterKind::SnapmakerU1,
        model: Some("Snapmaker U1".into()),
        firmware: None,
        nozzle: Some(0.4),
        tools: Some(4),
        state: Some(mica_printhost::PrinterState::Idle),
        progress: None,
        needs_key,
        hostname: None,
    };
    app.add_printer.looked_up = vec![found("Workshop U1", true)];
    for _ in 0..2 {
        frame(&ctx, &mut app, vec![]);
    }
    // The card's button, bottom of the first card.
    let o = origin();
    click(
        &ctx,
        &mut app,
        Pos2::new(o.x + 28. + 140., o.y + 118. + 296. - 34.),
    );
    // It asked for its key: Setup, prefilled, nothing added yet.
    assert_eq!(app.add_printer.screen, Screen::Setup);
    let s = app.add_printer.setup.as_ref().unwrap();
    assert_eq!(s.name, "Workshop U1");
    assert_eq!(s.connect.address, "127.0.0.1:9");
    assert!(s.connect.needs_key);
    assert!(app.printers.list.is_empty());
    // Without a key prompt, one click adds the profile and the connection.
    app.add_printer.screen = Screen::Found;
    app.add_printer.looked_up = vec![found("Garage U1", false)];
    frame(&ctx, &mut app, vec![]);
    click(
        &ctx,
        &mut app,
        Pos2::new(o.x + 28. + 140., o.y + 118. + 296. - 34.),
    );
    assert_eq!(app.printers.list.len(), 1);
    let p = &app.printers.list[0];
    assert_eq!(p.name, "Garage U1");
    assert_eq!(p.kind, Kind::SnapmakerU1);
    assert_eq!(
        p.profile.as_deref(),
        Some("Snapmaker/Snapmaker U1 (0.4 nozzle)")
    );
    assert!(
        app.prefs
            .has_printer("Snapmaker", "Snapmaker U1 (0.4 nozzle)")
    );
    assert!(app.dialogs.my_printers && !app.dialogs.add_printer);
}

#[test]
fn my_printers_cards_poll_only_while_the_window_is_open() {
    let Some((ctx, mut app)) = app() else { return };
    for (name, port) in [("A", 9), ("B", 1)] {
        let mut p = config::Printer::new(Kind::PrusaLink);
        p.name = name.into();
        p.address = format!("127.0.0.1:{port}");
        app.printers.put(None, p, None).unwrap();
    }
    app.dialogs.my_printers = true;
    for _ in 0..3 {
        frame(&ctx, &mut app, vec![]);
    }
    assert_eq!(app.printers.watchers.len(), 2);
    assert!(
        app.printers
            .watchers
            .values()
            .all(|w| w.interval().is_some())
    );
    // Settings view of one printer: the cards are gone, so is their polling.
    app.printers_editor.view = crate::panels::printers::View::Settings;
    app.printers_editor.selected = Some(0);
    frame(&ctx, &mut app, vec![]);
    assert!(
        app.printers
            .watchers
            .values()
            .all(|w| w.interval().is_none())
    );
    app.printers_editor.view = crate::panels::printers::View::Cards;
    frame(&ctx, &mut app, vec![]);
    assert!(
        app.printers
            .watchers
            .values()
            .all(|w| w.interval().is_some())
    );
    // Closed: nothing polls (no job is running), no camera players are left.
    app.dialogs.my_printers = false;
    for _ in 0..2 {
        frame(&ctx, &mut app, vec![]);
    }
    assert!(
        app.printers
            .watchers
            .values()
            .all(|w| w.interval().is_none())
    );
    assert!(app.printers_editor.cameras.is_empty());
}

/// GUI monkey test (seed 1001): the Library opened over the Machine tab left the dashboard's
/// camera players streaming, unseen, until the Library closed.
#[test]
fn library_over_the_machine_tab_stops_its_cameras() {
    let Some((ctx, mut app)) = app() else { return };
    let mut p = config::Printer::new(Kind::PrusaLink);
    p.name = "Camera".into();
    p.address = "127.0.0.1:9".into();
    p.camera = Some("http://127.0.0.1:9/snapshot.jpg".into());
    app.printers.put(None, p, None).unwrap();
    app.machine = true;
    for _ in 0..3 {
        frame(&ctx, &mut app, vec![]);
    }
    assert!(
        !app.machine_view.cameras.is_empty(),
        "the dashboard plays the camera"
    );
    app.library.shown = true;
    frame(&ctx, &mut app, vec![]);
    assert!(app.machine_view.cameras.is_empty());
    // Back from the Library to the Machine tab: the camera plays again.
    app.library.shown = false;
    for _ in 0..2 {
        frame(&ctx, &mut app, vec![]);
    }
    assert!(!app.machine_view.cameras.is_empty());
}
