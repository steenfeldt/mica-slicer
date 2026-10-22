// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The quick tour in a headless app: every step finds its control in the default layout, Next
//! / Back / Skip by keyboard, clicks outside the card do nothing, the window behind takes no
//! keys or menu commands, the sample plate (an empty project gets the Benchy, a project with
//! something in it is left alone), "tour seen" in the settings, never starting by itself,
//! the Reduce motion crossfade, and the card always inside the window.

use crate::app::{App, Options};
use crate::menu::Action;
use crate::state::Prefs;
use crate::tour::{self, Anchor, Cutout, Nav, TOUR_STEPS, View};
use eframe::egui::{self, Event, Key, Modifiers, Pos2, RawInput, Rect, Vec2};
use mica_core::builtin_models::BuiltinModel;
use mica_profiles::user::Store;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mica-tour-{name}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

struct World {
    ctx: egui::Context,
    app: App,
    time: f64,
}

impl World {
    fn new() -> Self {
        let ctx = egui::Context::default();
        let app = App::create(
            &ctx,
            Options {
                prefs_path: None,
                store: Some(Store::open(temp("store"))),
                files: vec![],
                headless: true,
            },
        );
        Self { ctx, app, time: 0. }
    }

    /// One 1440 × 900 frame, a second later than the last (moves are settled).
    fn frame(&mut self, events: Vec<Event>) {
        self.time += 1.;
        self.frame_at(self.time, events);
    }

    fn frame_at(&mut self, time: f64, events: Vec<Event>) {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1440., 900.))),
            time: Some(time),
            events,
            ..Default::default()
        };
        let app = &mut self.app;
        let mut out = self.ctx.run_ui(input, |ui| app.show(ui));
        out.textures_delta.clear();
    }

    fn settle(&mut self) {
        for _ in 0..3 {
            self.frame(vec![]);
        }
    }

    fn key(&mut self, key: Key) {
        let ev = |pressed| Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: Modifiers::NONE,
        };
        self.frame(vec![ev(true)]);
        self.frame(vec![ev(false)]);
    }

    fn click(&mut self, at: Pos2) {
        let button = |pressed| Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        self.frame(vec![Event::PointerMoved(at)]);
        self.frame(vec![button(true)]);
        self.frame(vec![button(false)]);
    }

    fn step(&self) -> Option<usize> {
        self.app.tour.as_ref().map(|t| t.step)
    }
}

#[test]
fn every_step_finds_its_control_in_the_default_layout() {
    let mut w = World::new();
    w.frame(vec![]);
    w.app.start_tour(&w.ctx);
    for (i, step) in TOUR_STEPS.iter().enumerate() {
        w.app.tour_jump(i, &w.ctx);
        w.settle();
        let rect = tour::drawn_last_frame(&w.ctx, step.anchor);
        let rect = rect.unwrap_or_else(|| panic!("step {}: no {:?} on screen", i + 1, step.anchor));
        assert!(
            rect.width() >= 20. && rect.height() >= 20.,
            "step {}: {:?} is {rect:?}",
            i + 1,
            step.anchor
        );
        assert!(
            Rect::from_min_size(Pos2::ZERO, Vec2::new(1440., 900.)).contains_rect(rect),
            "step {}: {rect:?}",
            i + 1
        );
        // The step's view is the one showing.
        match step.view {
            View::Prepare => assert!(!w.app.machine && !w.app.preview, "step {}", i + 1),
            View::Preview => assert!(!w.app.machine),
            View::Machine => assert!(w.app.machine),
        }
        assert!(!w.app.library.shown);
    }
    // The Preview step started a slice of the sample (it opens Preview once done).
    w.app.cancel_slice();
    // Every anchor belongs to a step, and every step's text fits the card's style.
    for a in [
        Anchor::Tabs,
        Anchor::PrinterFilament,
        Anchor::Settings,
        Anchor::PlateBar,
        Anchor::Toolbar,
        Anchor::SliceExport,
        Anchor::MachineTab,
    ] {
        assert!(TOUR_STEPS.iter().any(|s| s.anchor == a), "{a:?}");
    }
    for s in &TOUR_STEPS {
        assert!((1..=4).contains(&s.body.len()), "{}", s.title);
        for b in s.body {
            let b = tour::body_text(b);
            assert_eq!(b.matches("**").count() % 2, 0, "unpaired ** in {b}");
            assert!(!b.contains('{'), "placeholder left in {b}");
            // The bundled fonts have no glyphs for these.
            for c in ['⌘', '⇧', '⌥', '⋯'] {
                assert!(!b.contains(c), "{c} in {b}");
            }
        }
    }
}

#[test]
fn keyboard_moves_through_the_steps_and_esc_skips() {
    let mut w = World::new();
    w.frame(vec![]);
    w.app.start_tour(&w.ctx);
    w.settle();
    assert_eq!(w.step(), Some(0));
    w.key(Key::ArrowLeft);
    assert_eq!(w.step(), Some(0), "no Back on the first step");
    w.key(Key::ArrowRight);
    assert_eq!(w.step(), Some(1));
    w.key(Key::Enter);
    assert_eq!(w.step(), Some(2));
    w.key(Key::ArrowLeft);
    assert_eq!(w.step(), Some(1));
    // Clicks outside the card do nothing: no dismiss, nothing behind reacts.
    let objects = w.app.plates.current().objects.len();
    for at in [
        Pos2::new(700., 450.),
        Pos2::new(8., 8.),
        Pos2::new(760., 24.),
    ] {
        w.click(at);
        assert_eq!(w.step(), Some(1), "click at {at:?}");
    }
    assert!(!w.app.machine && !w.app.preview && !w.app.library.shown);
    assert_eq!(w.app.plates.current().objects.len(), objects);
    // Keys don't reach the window behind (Delete would remove the selected Benchy).
    w.app.tour_jump(4, &w.ctx);
    w.settle();
    assert!(w.app.selected.is_some(), "the tools step selects an object");
    w.key(Key::Delete);
    w.key(Key::Backspace);
    assert_eq!(w.app.plates.current().objects.len(), objects);
    // Menu commands are off while the tour runs (Quit stays).
    assert!(
        !w.app
            .menu_state(Action::Act(crate::keymap::Act::Slice), false)
            .enabled
    );
    assert!(!w.app.menu_state(Action::Tour, false).enabled);
    assert!(w.app.menu_state(Action::Quit, false).enabled);
    w.key(Key::Escape);
    assert_eq!(w.step(), None, "Esc skips the tour");
    assert!(
        w.app
            .menu_state(Action::Act(crate::keymap::Act::Slice), false)
            .enabled
    );
    assert!(!w.app.machine && !w.app.preview, "back where it started");
}

#[test]
fn the_last_step_ends_with_done_and_returns() {
    let mut w = World::new();
    w.frame(vec![]);
    w.app.start_tour(&w.ctx);
    w.app.tour_jump(TOUR_STEPS.len() - 1, &w.ctx);
    w.settle();
    assert!(w.app.machine, "the last step shows the Machine tab");
    w.app.tour_nav(Nav::Next, &w.ctx);
    assert!(w.app.tour.is_none(), "Done ends it");
    assert!(!w.app.machine, "back to Prepare, where it started");
    // Skip from the middle, and from the menu again.
    w.app.run_menu(Action::Tour, &w.ctx);
    assert_eq!(w.step(), Some(0));
    w.app.tour_nav(Nav::Next, &w.ctx);
    w.app.tour_nav(Nav::Skip, &w.ctx);
    assert!(w.app.tour.is_none());
}

#[test]
fn an_empty_project_gets_the_benchy_and_others_are_left_alone() {
    let mut w = World::new();
    w.frame(vec![]);
    assert!(w.app.plates.current().objects.is_empty());
    w.app.start_tour(&w.ctx);
    let names: Vec<_> = w
        .app
        .plates
        .current()
        .objects
        .iter()
        .map(|o| o.name.clone())
        .collect();
    assert_eq!(names.len(), 1);
    assert!(names[0].contains("Benchy"), "{names:?}");
    w.app.end_tour();

    let mut w = World::new();
    w.frame(vec![]);
    w.app
        .add_builtin(BuiltinModel::Cube, crate::builtin::AddAs::Object);
    let before: Vec<_> = w
        .app
        .plates
        .current()
        .objects
        .iter()
        .map(|o| o.name.clone())
        .collect();
    w.app.start_tour(&w.ctx);
    w.settle();
    let after: Vec<_> = w
        .app
        .plates
        .current()
        .objects
        .iter()
        .map(|o| o.name.clone())
        .collect();
    assert_eq!(
        before, after,
        "a project with something in it is not changed"
    );
}

#[test]
fn tour_seen_is_remembered_and_nothing_starts_it_by_itself() {
    // Headless apps (tests, screenshots, the monkey test) and the first launch's windows
    // never start it.
    let mut w = World::new();
    w.app.launch_windows(false, false, false, false);
    w.settle();
    assert!(w.app.tour.is_none());
    assert!(w.app.dialogs.welcome.is_some());
    assert!(!w.app.prefs.tour_seen);
    // The Welcome window's button starts it and closes the window.
    w.app.start_tour(&w.ctx);
    assert!(w.app.tour.is_some());
    assert!(w.app.dialogs.welcome.is_none());
    assert!(w.app.prefs.welcome_seen);
    assert!(w.app.prefs.tour_seen);
    assert!(w.app.prefs_dirty);
    // Saved and read back.
    let path = temp("prefs").join("mica.toml");
    w.app.prefs.save(&path).unwrap();
    let (prefs, warning) = Prefs::load(&path);
    assert!(warning.is_none(), "{warning:?}");
    assert!(prefs.tour_seen);
    assert!(!Prefs::default().tour_seen);
    // Settings from before the tour read as not seen.
    std::fs::write(&path, "welcome_seen = true\n").unwrap();
    assert!(!Prefs::load(&path).0.tour_seen);
}

#[test]
fn reduce_motion_crossfades_instead_of_moving() {
    let a = Rect::from_min_size(Pos2::new(100., 100.), Vec2::new(200., 40.));
    let b = Rect::from_min_size(Pos2::new(900., 600.), Vec2::new(300., 300.));
    // Moving: the cutout is between the two, ease-out (past half way at half time).
    let Cutout::At(Some(mid)) = tour::plan(Some(a), Some(b), 0.5, false) else {
        panic!("a move");
    };
    assert!(mid.min.x > a.min.x && mid.min.x < b.min.x);
    assert!(mid.min.x - a.min.x > (b.min.x - a.min.x) / 2., "ease-out");
    assert!(
        mid.width() > a.width() && mid.width() < b.width(),
        "size too"
    );
    assert_eq!(tour::plan(Some(a), Some(b), 1., false), Cutout::At(Some(b)));
    // Reduce motion: never a cutout in between, only the two fading.
    for t in [0., 0.2, 0.5, 0.9] {
        match tour::plan(Some(a), Some(b), t, true) {
            Cutout::Crossfade { from, to, k } => {
                assert_eq!((from, to), (Some(a), Some(b)));
                assert!((0. ..=1.).contains(&k));
            }
            other => panic!("{other:?}"),
        }
    }
    assert_eq!(tour::plan(Some(a), Some(b), 1., true), Cutout::At(Some(b)));
    // A step without a control on screen fades too.
    assert!(matches!(
        tour::plan(Some(a), None, 0.5, false),
        Cutout::Crossfade { .. }
    ));

    // In the window: a Reduce motion tour changes steps through frames without errors, and
    // the move is over after its time.
    let mut w = World::new();
    w.frame(vec![]);
    w.app.start_tour(&w.ctx);
    w.app.tour.as_mut().unwrap().reduce_motion = true;
    w.settle();
    let t0 = w.time;
    w.app.tour_nav(Nav::Next, &w.ctx);
    w.frame_at(t0 + 0.1, vec![]);
    assert!(w.app.tour.as_ref().unwrap().moving(t0 + 0.1));
    w.frame_at(t0 + 0.2, vec![]);
    assert!(
        !w.app
            .tour
            .as_ref()
            .unwrap()
            .moving(t0 + tour::MOVE_S + 0.01)
    );
}

#[test]
fn the_card_stays_inside_the_window() {
    let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(1440., 900.));
    let size = Vec2::new(400., 260.);
    // No control on screen: the middle, no pointer.
    let (r, side) = tour::place(screen, None, size);
    assert_eq!(side, None);
    assert!((r.center() - screen.center()).length() < 0.5);
    // Controls all over the window, small and large.
    for x in (0..1440).step_by(120) {
        for y in (0..900).step_by(90) {
            for s in [
                Vec2::new(40., 30.),
                Vec2::new(360., 700.),
                Vec2::new(900., 60.),
            ] {
                let cut = Rect::from_min_size(Pos2::new(x as f32, y as f32), s).intersect(screen);
                let (r, side) = tour::place(screen, Some(cut), size);
                assert!(screen.shrink(15.).contains_rect(r), "{cut:?} → {r:?}");
                assert!(side.is_some());
            }
        }
    }
    // The header's switch: the card goes below it.
    let tabs = Rect::from_center_size(Pos2::new(720., 24.), Vec2::new(330., 40.));
    assert_eq!(
        tour::place(screen, Some(tabs), size).1,
        Some(tour::Side::Below)
    );
    // The header's right side (Slice, Export, Print…): below too, right-aligned in the window.
    let cluster = Rect::from_min_max(Pos2::new(1166., 2.), Pos2::new(1434., 52.));
    let (r, side) = tour::place(screen, Some(cluster), size);
    assert_eq!(side, Some(tour::Side::Below));
    assert!((r.right() - (1440. - 16.)).abs() < 0.5, "{r:?}");
    // The plate bar at the bottom: above it.
    let bar = Rect::from_min_max(Pos2::new(334., 830.), Pos2::new(738., 894.));
    assert_eq!(
        tour::place(screen, Some(bar), size).1,
        Some(tour::Side::Above)
    );
    // The sidebar's cards: beside them.
    let cards = Rect::from_min_max(Pos2::new(1066., 120.), Pos2::new(1438., 332.));
    assert_eq!(
        tour::place(screen, Some(cards), size).1,
        Some(tour::Side::Left)
    );
}
