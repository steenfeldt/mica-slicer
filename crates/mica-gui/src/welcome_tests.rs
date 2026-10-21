// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The Welcome and What's new windows in a headless app: which one a launch opens (first
//! launch, same version, an update, settings from before the Welcome window, development
//! builds), reopening from Help, Esc and Get started, the three start cards (printer gallery,
//! Benchy, import only when another slicer has presets: a fake home, never the user's), and
//! the About window's Alpha line.

use crate::app::{App, Options};
use crate::menu::Action;
use crate::panels::add_printer::Screen;
use crate::panels::welcome::Start;
use crate::panels::whats_new::VERSION;
use eframe::egui::{self, Event, Key, Modifiers, Pos2, RawInput};
use mica_profiles::user::Store;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mica-welcome-{name}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A headless app on its own store, looking for other slicers in an empty fake home.
fn app() -> (egui::Context, App) {
    let ctx = egui::Context::default();
    let mut app = App::create(
        &ctx,
        Options {
            prefs_path: None,
            store: Some(Store::open(temp("store"))),
            files: vec![],
            headless: true,
        },
    );
    app.profiles.look_in(&temp("home"), None);
    (ctx, app)
}

/// One whole-window frame; the texts drawn, joined by newlines.
fn frame(ctx: &egui::Context, app: &mut App, events: Vec<Event>) -> String {
    fn texts(shape: egui::Shape, out: &mut Vec<String>) {
        match shape {
            egui::Shape::Text(t) => out.push(t.galley.text().to_string()),
            egui::Shape::Vec(shapes) => shapes.into_iter().for_each(|s| texts(s, out)),
            _ => {}
        }
    }
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
    let mut out = vec![];
    for s in output.shapes {
        texts(s.shape, &mut out);
    }
    out.join("\n")
}

/// The texts of a settled frame (a new window sizes itself in its first frame, unseen).
fn shown(ctx: &egui::Context, app: &mut App) -> String {
    frame(ctx, app, vec![]);
    frame(ctx, app, vec![])
}

fn escape(ctx: &egui::Context, app: &mut App) {
    let ev = |pressed| Event::Key {
        key: Key::Escape,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers: Modifiers::NONE,
    };
    frame(ctx, app, vec![ev(true)]);
    frame(ctx, app, vec![ev(false)]);
}

#[test]
fn headless_apps_open_neither_window() {
    let (ctx, mut app) = app();
    frame(&ctx, &mut app, vec![]);
    assert!(app.dialogs.welcome.is_none());
    assert!(!app.dialogs.whats_new);
}

#[test]
fn welcome_once_then_whats_new_once_per_version() {
    let (ctx, mut app) = app();
    // First launch: Welcome, never What's new; the version is remembered at once.
    app.launch_windows(false, false, false, false);
    assert!(app.dialogs.welcome.is_some());
    assert!(!app.dialogs.whats_new, "no What's new on the first launch");
    assert_eq!(app.prefs.last_version, VERSION);
    assert!(!app.prefs.welcome_seen, "seen only once closed");
    let drawn = shown(&ctx, &mut app);
    for text in [
        "Welcome to Mica Slicer",
        "Alpha (pre-release)",
        "Mica is alpha (pre-release) software.",
        "Add your printer",
        "Try a Benchy",
        "Get started",
    ] {
        assert!(drawn.contains(text), "{text} in\n{drawn}");
    }
    assert!(
        !drawn.contains("Import from PrusaSlicer"),
        "no presets, no Import card"
    );
    escape(&ctx, &mut app);
    assert!(app.dialogs.welcome.is_none(), "Esc closes it");
    assert!(app.prefs.welcome_seen);

    // The next launch of the same version: nothing.
    app.launch_windows(true, false, false, false);
    assert!(app.dialogs.welcome.is_none());
    assert!(!app.dialogs.whats_new);

    // After an update: What's new, once.
    app.prefs.last_version = "0.0.0-older".into();
    app.launch_windows(true, false, false, false);
    assert!(app.dialogs.welcome.is_none());
    assert!(app.dialogs.whats_new);
    assert_eq!(app.prefs.last_version, VERSION);
    let drawn = shown(&ctx, &mut app);
    assert!(
        drawn.contains(&format!("What's new in Mica {VERSION}")),
        "{drawn}"
    );
    assert!(drawn.contains("The first tester build") || drawn.contains("Version"));
    escape(&ctx, &mut app);
    assert!(!app.dialogs.whats_new, "Esc closes What's new");
    app.launch_windows(true, false, false, false);
    assert!(!app.dialogs.whats_new, "once per version");

    // A development build shows it only when asked.
    app.prefs.last_version = "0.0.0-older".into();
    app.launch_windows(true, true, false, false);
    assert!(!app.dialogs.whats_new);
    app.launch_windows(true, true, false, true);
    assert!(app.dialogs.whats_new);
    app.dialogs.whats_new = false;

    // Help reopens both.
    app.run_menu(Action::Welcome, &ctx);
    assert!(app.dialogs.welcome.is_some());
    app.run_menu(Action::WhatsNew, &ctx);
    assert!(app.dialogs.whats_new);
    frame(&ctx, &mut app, vec![]);
}

#[test]
fn settings_from_before_the_welcome_window_get_whats_new() {
    let (_ctx, mut app) = app();
    // mica.toml existed but remembers no version: an earlier user updating.
    app.launch_windows(true, false, false, false);
    assert!(app.dialogs.welcome.is_none());
    assert!(app.prefs.welcome_seen);
    assert!(app.dialogs.whats_new);
    // A first launch that quit with Welcome open shows it again (it was never closed).
    let (_ctx, mut fresh) = self::app();
    fresh.launch_windows(false, false, false, false);
    assert!(fresh.dialogs.welcome.is_some());
    fresh.dialogs.welcome = None;
    fresh.launch_windows(true, false, false, false);
    assert!(fresh.dialogs.welcome.is_some());
}

#[test]
fn get_started_closes_and_cards_start_things() {
    let (ctx, mut app) = app();
    app.open_welcome();
    // No presets in the fake home: no Import card.
    assert_eq!(app.dialogs.welcome.as_ref().unwrap().importable, None);
    frame(&ctx, &mut app, vec![]);
    app.welcome_start(Start::AddPrinter);
    assert!(app.dialogs.welcome.is_none());
    assert!(app.prefs.welcome_seen);
    assert!(app.dialogs.add_printer);
    assert_eq!(
        app.add_printer.screen,
        Screen::Gallery,
        "the printer gallery, not the network search"
    );
    app.dialogs.add_printer = false;

    app.open_welcome();
    let before = app.plates.current().objects.len();
    app.welcome_start(Start::Benchy);
    assert!(app.dialogs.welcome.is_none());
    let objects = &app.plates.current().objects;
    assert_eq!(objects.len(), before + 1);
    assert_eq!(objects.last().unwrap().name, "3DBenchy");
    frame(&ctx, &mut app, vec![]);
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

#[test]
fn import_card_only_with_presets_in_other_slicers() {
    if !mica_testkit::require_prusa_profiles() {
        return;
    }
    let (ctx, mut app) = app();
    let home = temp("home-prusa");
    let prusa = home.join("Library/Application Support/PrusaSlicer");
    write(&prusa.join("PrusaSlicer.ini"), "[presets]\n");
    write(
        &prusa.join("filament/PLA Tweaked.ini"),
        "inherits = Prusament PLA @COREONE HF0.4\ntemperature = 212\n",
    );
    app.profiles.look_in(&home, None);
    // The first launch: the Welcome window's Import card replaces the startup notice.
    let offers = |app: &App| {
        app.notifications
            .list
            .iter()
            .filter(|n| {
                n.actions
                    .iter()
                    .any(|(_, a)| *a == crate::notifications::Action::ReviewImports)
            })
            .count()
    };
    app.offer_imports();
    assert_eq!(offers(&app), 1);
    app.launch_windows(false, false, false, false);
    assert_eq!(offers(&app), 0);
    let found = app.dialogs.welcome.as_ref().unwrap().importable.clone();
    assert_eq!(
        found.as_deref(),
        Some("Found 1 filament profile in PrusaSlicer")
    );
    let drawn = shown(&ctx, &mut app);
    assert!(
        drawn.contains("Import from PrusaSlicer / OrcaSlicer"),
        "{drawn}"
    );
    app.welcome_start(Start::Import);
    assert!(app.dialogs.welcome.is_none());
    let review = app.import_ui.review.as_ref().expect("the review window");
    assert_eq!(review.rows.len(), 1);
    frame(&ctx, &mut app, vec![]);
}

#[test]
fn about_shows_the_alpha_status() {
    let (ctx, mut app) = app();
    app.open_about(crate::panels::about::AboutTab::About);
    let drawn = shown(&ctx, &mut app);
    assert!(drawn.contains("Version "), "{drawn}");
    // The badge beside the version, and the warranty line with its License link.
    assert!(drawn.contains("\nAlpha (pre-release)\n"), "{drawn}");
    assert!(
        drawn.contains("Alpha (pre-release) · Provided as is, without warranty · "),
        "{drawn}"
    );
    assert!(drawn.contains("\nLicense\n"), "{drawn}");
}
