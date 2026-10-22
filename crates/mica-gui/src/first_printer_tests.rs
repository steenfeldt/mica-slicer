// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Mica never picks a printer the user didn't choose. A packaged Mica before the Prusa printer
//! profiles are downloaded (`WITHOUT_PRUSA`: no Prusa bundle, only the Orca vendors) once
//! started on the first printer of the Snapmaker bundle (a Snapmaker A250) and sliced with it.
//! Now a fresh settings folder starts with no printer: the sidebar says so and offers Add your
//! printer (the gallery), Slice / Export / Print are off and say why, and Welcome's Try a
//! Benchy asks for a printer first. A remembered Prusa printer stays selected while its
//! profiles download and is usable once they are here.

use crate::app::{App, NoPrinter, Options};
use crate::keymap::Act;
use crate::menu::Action;
use crate::panels::add_printer::Screen;
use crate::panels::profiles::WITHOUT_PRUSA;
use crate::panels::welcome::Start;
use eframe::egui::{self, Event, Pos2, RawInput, Rect};
use mica_profiles::user::Store;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mica-first-printer-{name}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A headless app on a settings file in `dir` (as the real window reads one), discovering
/// profiles as a packaged Mica without the Prusa bundle.
fn packaged_app(dir: &Path) -> (egui::Context, App) {
    WITHOUT_PRUSA.set(true);
    let ctx = egui::Context::default();
    let mut app = App::create(
        &ctx,
        Options {
            prefs_path: Some(dir.join("mica.toml")),
            store: Some(Store::open(dir.join("store"))),
            files: vec![],
            headless: true,
        },
    );
    app.profiles.look_in(&dir.join("home"), None);
    (ctx, app)
}

/// One whole-window frame: the texts drawn with their rectangles.
fn frame(ctx: &egui::Context, app: &mut App, events: Vec<Event>) -> Vec<(String, Rect)> {
    fn texts(shape: egui::Shape, out: &mut Vec<(String, Rect)>) {
        match shape {
            egui::Shape::Text(t) => {
                let rect = t.galley.rect.translate(t.pos.to_vec2());
                out.push((t.galley.text().to_string(), rect));
            }
            egui::Shape::Vec(shapes) => shapes.into_iter().for_each(|s| texts(s, out)),
            _ => {}
        }
    }
    let input = RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(1440., 900.))),
        events,
        ..Default::default()
    };
    let mut output = ctx.run_ui(input, |ui| app.show(ui));
    output.textures_delta.clear();
    let mut out = vec![];
    for s in output.shapes {
        texts(s.shape, &mut out);
    }
    out
}

fn shown(ctx: &egui::Context, app: &mut App) -> String {
    frame(ctx, app, vec![]);
    let drawn = frame(ctx, app, vec![]);
    drawn
        .into_iter()
        .map(|(t, _)| t)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Clicks the text `label` where it was drawn.
fn click(ctx: &egui::Context, app: &mut App, label: &str) {
    frame(ctx, app, vec![]);
    let drawn = frame(ctx, app, vec![]);
    let (_, rect) = drawn
        .iter()
        .find(|(t, _)| t == label)
        .unwrap_or_else(|| panic!("{label} is not drawn"));
    let pos = rect.center();
    let button = |pressed| Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    frame(ctx, app, vec![Event::PointerMoved(pos)]);
    frame(ctx, app, vec![button(true)]);
    frame(ctx, app, vec![button(false)]);
}

#[test]
fn a_fresh_packaged_mica_asks_for_a_printer() {
    if !mica_testkit::require_orca_profiles() {
        return;
    }
    let dir = temp("fresh");
    let (ctx, mut app) = packaged_app(&dir);
    assert!(app.profiles.prusa_missing);
    // No printer, and none of the Orca vendors' printers in its place.
    assert!(!app.profiles.has_printer(), "{}", app.profiles.printer);
    assert_eq!(app.no_printer(), Some(NoPrinter::Unchosen));
    assert!(
        app.prefs.my_printers.is_empty(),
        "{:?}",
        app.prefs.my_printers
    );
    assert!(app.prefs.selection.printer.is_empty());
    assert!(app.config.is_err());

    // A model on the plate: nothing slices, and every place says why.
    app.welcome_start(Start::Benchy);
    assert!(
        app.plates.current().objects.is_empty(),
        "Try a Benchy asks for the printer first"
    );
    assert!(app.dialogs.add_printer);
    assert_eq!(app.add_printer.screen, Screen::Gallery);
    app.dialogs.add_printer = false;
    app.benchy_waits_for_printer = false;
    app.add_builtin(
        mica_core::builtin_models::BuiltinModel::Cube,
        crate::builtin::AddAs::Object,
    );
    assert!(!app.menu_state(Action::Act(Act::Slice), false).enabled);
    assert!(!app.menu_state(Action::Act(Act::ExportGcode), false).enabled);
    assert!(!app.menu_state(Action::Act(Act::Print), false).enabled);
    let reason = app.blocked_reason(false);
    assert!(reason.contains("Choose your printer"), "{reason}");
    app.slice_current(&ctx);
    assert!(app.worker.is_none() && app.plates.current().finished.is_none());
    assert_eq!(app.status, reason, "a slice request says why");

    let drawn = shown(&ctx, &mut app);
    for text in ["No printer selected", "Add your printer", "Slice"] {
        assert!(drawn.contains(text), "{text} in\n{drawn}");
    }
    assert!(!drawn.contains("Snapmaker A250"), "{drawn}");

    // Add your printer opens the gallery; choosing a printer there makes it usable.
    click(&ctx, &mut app, "Add your printer");
    assert!(app.dialogs.add_printer);
    assert_eq!(app.add_printer.screen, Screen::Gallery);
    let vi = app.profiles.vendor_index("Snapmaker").unwrap();
    app.prefs
        .add_printer("Snapmaker", "Snapmaker U1 (0.4 nozzle)");
    app.switch_printer(vi, "Snapmaker U1 (0.4 nozzle)");
    assert_eq!(app.no_printer(), None);
    assert!(app.config.is_ok());
    assert!(app.menu_state(Action::Act(Act::Slice), false).enabled);
    assert_eq!(app.prefs.selection.printer, "Snapmaker U1 (0.4 nozzle)");
}

#[test]
fn try_a_benchy_without_a_printer_places_it_once_one_is_chosen() {
    if !mica_testkit::require_orca_profiles() {
        return;
    }
    let dir = temp("benchy");
    let (_ctx, mut app) = packaged_app(&dir);
    app.welcome_start(Start::Benchy);
    assert!(app.plates.current().objects.is_empty());
    let vi = app.profiles.vendor_index("Snapmaker").unwrap();
    app.switch_printer(vi, "Snapmaker U1 (0.4 nozzle)");
    let objects = &app.plates.current().objects;
    assert_eq!(objects.len(), 1);
    assert_eq!(objects[0].name, "3DBenchy");
}

#[test]
fn a_remembered_prusa_printer_waits_for_its_profiles() {
    if !mica_testkit::require_orca_profiles() || !mica_testkit::require_prusa_profiles() {
        return;
    }
    let dir = temp("waiting");
    std::fs::write(
        dir.join("mica.toml"),
        "detected = true\nwelcome_seen = true\nmy_printers = [\"Prusa/Prusa CORE One HF0.4 nozzle\"]\n\n\
         [selection]\nvendor = \"Prusa\"\nprinter = \"Prusa CORE One HF0.4 nozzle\"\n\
         print = \"0.20mm SPEED @COREONE HF0.4\"\nfilaments = [\"Prusament PLA @COREONE HF0.4\"]\n",
    )
    .unwrap();
    let (ctx, mut app) = packaged_app(&dir);
    // Still the CORE One, waiting: never another printer in its place.
    assert!(!app.profiles.has_printer(), "{}", app.profiles.printer);
    let waiting = NoPrinter::Waiting("Prusa CORE One HF0.4 nozzle".into());
    assert_eq!(app.no_printer(), Some(waiting.clone()));
    assert_eq!(app.prefs.selection.printer, "Prusa CORE One HF0.4 nozzle");
    assert_eq!(app.blocked_reason(false), waiting.reason());
    let drawn = shown(&ctx, &mut app);
    for text in [
        "Prusa CORE One",
        "This printer needs the Prusa printer profiles.",
        "Download",
    ] {
        assert!(drawn.contains(text), "{text} in\n{drawn}");
    }
    // Downloading, then a failure that offers Try again.
    app.prusa_download.phase = crate::prusa_download::Phase::Downloading(0.4);
    let drawn = shown(&ctx, &mut app);
    assert!(drawn.contains("Profiles downloading…"), "{drawn}");
    app.prusa_download.phase =
        crate::prusa_download::Phase::Failed("Prusa's profile server could not be reached".into());
    let drawn = shown(&ctx, &mut app);
    assert!(drawn.contains("Try again"), "{drawn}");
    assert_eq!(app.no_printer(), Some(waiting));
    // Settings saved meanwhile keep the remembered printer.
    app.recompute_config();
    assert_eq!(app.prefs.selection.vendor, "Prusa");

    // The bundle arrives: the CORE One is selected and usable.
    app.prusa_profiles_arrived(mica_profiles::vendor_profiles::prusa::Fetched {
        bundle: mica_testkit::prusa_bundle(),
        assets: 0,
        missing_assets: vec![],
    });
    assert_eq!(app.no_printer(), None);
    assert_eq!(app.profiles.vendor_name(), "Prusa");
    assert_eq!(app.profiles.printer, "Prusa CORE One HF0.4 nozzle");
    assert!(app.config.is_ok());
    let drawn = shown(&ctx, &mut app);
    assert!(
        !drawn.contains("needs the Prusa printer profiles"),
        "{drawn}"
    );
}

#[test]
fn headless_runs_keep_the_default_printer_only_when_it_is_there() {
    // Tests, screenshots and the monkey test (no settings file) start on the CLI's default
    // printer when the Prusa profiles are here; without them, on none (never a fallback).
    if !mica_testkit::require_orca_profiles() {
        return;
    }
    WITHOUT_PRUSA.set(true);
    let ctx = egui::Context::default();
    let app = App::create(
        &ctx,
        Options {
            prefs_path: None,
            store: Some(Store::open(temp("headless-store"))),
            files: vec![],
            headless: true,
        },
    );
    assert!(!app.profiles.has_printer(), "{}", app.profiles.printer);
    assert!(app.prefs.my_printers.is_empty());
    if mica_testkit::require_prusa_profiles() {
        WITHOUT_PRUSA.set(false);
        let app = App::create(
            &ctx,
            Options {
                prefs_path: None,
                store: Some(Store::open(temp("headless-store"))),
                files: vec![],
                headless: true,
            },
        );
        assert_eq!(
            app.profiles.printer,
            crate::panels::profiles::DEFAULT_PRINTER
        );
    }
}

#[test]
fn a_real_settings_file_without_a_choice_gets_no_printer_even_with_the_prusa_profiles() {
    if !mica_testkit::require_prusa_profiles() {
        return;
    }
    let dir = temp("dev");
    let ctx = egui::Context::default();
    let app = App::create(
        &ctx,
        Options {
            prefs_path: Some(dir.join("mica.toml")),
            store: Some(Store::open(dir.join("store"))),
            files: vec![],
            headless: true,
        },
    );
    assert!(!app.profiles.prusa_missing);
    assert_eq!(app.no_printer(), Some(NoPrinter::Unchosen));
    assert!(app.prefs.my_printers.is_empty());
}
