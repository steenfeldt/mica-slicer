// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Moving models by pressing and dragging on them, and an object outside the printable
//! volume, end to end in the headless editor with real pointer events: a click only selects,
//! a press-drag on an unselected model selects and moves it as one undo step, the whole
//! selection moves together, a locked plate does not move, and the plate warning says where
//! and how far a model sticks out, with Move inside / Scale to fit / Arrange. Also the orbit
//! pivot: Cmd-drag turns around the point under the middle of the view (also over a model
//! while the Move gizmo is on), and the "orbit around the screen center" preference turns
//! around the camera target whatever is selected.

use crate::app::{App, Options};
use crate::notifications::Action;
use eframe::egui;
use glam::Vec3;
use mica_profiles::user::Store;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);
const CORE_ONE: &str = "Prusa CORE One HF0.4 nozzle";

struct Editor {
    app: App,
    ctx: egui::Context,
    time: f64,
    /// Modifier keys held for the events and frames that follow.
    modifiers: egui::Modifiers,
    /// The modifiers egui was last told about.
    sent: egui::Modifiers,
}

impl Editor {
    /// The CORE One with `cube20.stl` in the middle of the bed, nothing selected.
    fn new() -> Self {
        let ctx = egui::Context::default();
        let dir = std::env::temp_dir().join(format!(
            "mica-move-drag-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let mut app = App::create(
            &ctx,
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
        let model = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/models/"))
            .join("cube20.stl");
        let result = crate::app::load_files(vec![model], &app.config, app.bed.center());
        app.finish_load(result);
        app.select(None, false);
        let mut e = Self {
            app,
            ctx,
            time: 0.,
            modifiers: egui::Modifiers::NONE,
            sent: egui::Modifiers::NONE,
        };
        // Let the camera settle on the plate.
        for _ in 0..30 {
            e.frame(vec![]);
        }
        e
    }

    fn frame(&mut self, mut events: Vec<egui::Event>) {
        self.time += 1. / 60.;
        if self.modifiers != self.sent {
            self.sent = self.modifiers;
            events.insert(0, egui::Event::ModifiersChanged(self.modifiers));
        }
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1440., 900.),
            )),
            time: Some(self.time),
            predicted_dt: 1. / 60.,
            events,
            ..Default::default()
        };
        let app = &mut self.app;
        let mut output = self.ctx.run_ui(input, |ui| app.show(ui));
        output.textures_delta.clear();
    }

    /// Where object `k`'s top centre shows on screen.
    fn on_screen(&self, k: usize) -> egui::Pos2 {
        let (min, max) = self.app.plates.current().objects[k].bounds();
        let top = ((min + max) * 0.5).with_z(max.z - 0.5);
        let rect = self.app.viewport_rect;
        let uv = self
            .app
            .viewport
            .camera
            .project(top, rect.aspect_ratio())
            .expect("in front of the camera");
        egui::pos2(
            rect.left() + uv.x * rect.width(),
            rect.top() + uv.y * rect.height(),
        )
    }

    /// A spot on object `k`'s front face near its bottom left corner, clear of the gizmo.
    fn on_face(&self, k: usize) -> egui::Pos2 {
        let (min, _) = self.app.plates.current().objects[k].bounds();
        let rect = self.app.viewport_rect;
        let uv = self.uv(min + Vec3::new(3., 0.05, 3.));
        egui::pos2(
            rect.left() + uv.x * rect.width(),
            rect.top() + uv.y * rect.height(),
        )
    }

    /// Where world point `p` shows in the 3D view, 0..1 (0.5, 0.5: the middle).
    fn uv(&self, p: Vec3) -> glam::Vec2 {
        let rect = self.app.viewport_rect;
        self.app
            .viewport
            .camera
            .project(p, rect.aspect_ratio())
            .expect("in front of the camera")
    }

    /// Pans the view so the middle of the screen is off the plate's center and the model.
    fn pan_away(&mut self) {
        self.app.viewport.camera.target += Vec3::new(-60., 45., 0.);
        self.frame(vec![]);
    }

    /// A spot in the 3D view clear of the model, the gizmo and the toolbars.
    fn empty_spot(&self) -> egui::Pos2 {
        self.app.viewport_rect.center() + egui::vec2(-300., 180.)
    }

    fn position(&self, k: usize) -> Vec3 {
        self.app.plates.current().objects[k].position
    }

    /// Presses at `from`, moves in `steps` to `to`, holds there for `hold` seconds, releases.
    fn drag(&mut self, from: egui::Pos2, to: egui::Pos2, steps: usize, hold: f64) {
        let modifiers = self.modifiers;
        let button = move |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers,
        };
        self.frame(vec![egui::Event::PointerMoved(from)]);
        self.frame(vec![button(from, true)]);
        for k in 1..=steps {
            self.frame(vec![egui::Event::PointerMoved(
                from.lerp(to, k as f32 / steps as f32),
            )]);
        }
        let frames = (hold * 60.).ceil() as usize;
        for _ in 0..frames {
            self.frame(vec![]);
        }
        self.frame(vec![button(to, false)]);
        self.frame(vec![]);
    }
}

#[test]
fn a_click_selects_without_moving() {
    let mut e = Editor::new();
    let before = e.position(0);
    let undo = e.app.undo.len();
    let p = e.on_screen(0);
    e.drag(p, p, 0, 0.);
    assert_eq!(e.app.selected, Some(0), "the click selects the model");
    assert_eq!(e.position(0), before, "a click never nudges");
    // A jitter under the threshold, even held long enough for egui to call it a drag.
    e.app.select(None, false);
    e.drag(p, p + egui::vec2(4., -3.), 2, 1.2);
    assert_eq!(e.position(0), before, "movement under 5 px does not move");
    assert_eq!(e.app.undo.len(), undo, "nothing to undo");
}

#[test]
fn press_drag_on_an_unselected_model_moves_it_in_one_undo_step() {
    let mut e = Editor::new();
    assert_eq!(e.app.selected, None);
    let before = e.position(0);
    let undo = e.app.undo.len();
    let p = e.on_screen(0);
    e.drag(p, p + egui::vec2(80., 0.), 8, 0.);
    assert_eq!(e.app.selected, Some(0), "the press selected it");
    let after = e.position(0);
    assert!(
        (after - before).truncate().length() > 5.,
        "it moved: {before} → {after}"
    );
    assert_eq!(after.z, before.z, "it stays on the bed");
    assert_eq!(e.app.undo.len(), undo + 1, "one undo step for the drag");
    assert_eq!(e.app.undo.undo_label(), Some("Move"));
    e.app.undo();
    assert_eq!(e.position(0), before);
}

#[test]
fn the_selection_moves_together_and_another_model_moves_alone() {
    let mut e = Editor::new();
    e.app.select(Some(0), false);
    e.app
        .run_command(crate::commands::Command::Duplicate, &e.ctx.clone());
    assert_eq!(e.app.plates.current().objects.len(), 2);
    for _ in 0..5 {
        e.frame(vec![]);
    }
    e.app.select_many(&[0, 1], false);
    let (a, b) = (e.position(0), e.position(1));
    let p = e.on_screen(1);
    e.drag(p, p + egui::vec2(0., 60.), 6, 0.);
    let (da, db) = (e.position(0) - a, e.position(1) - b);
    assert!(da.length() > 5., "the other selected model moved too");
    assert!(
        (da - db).length() < 1e-3,
        "by the same amount: {da} vs {db}"
    );
    // Only model 0 selected: pressing model 1 selects it alone and moves only it.
    e.app.select(Some(0), false);
    let (a, b) = (e.position(0), e.position(1));
    let p = e.on_screen(1);
    e.drag(p, p + egui::vec2(-60., 0.), 6, 0.);
    assert_eq!(e.app.selection(), vec![1]);
    assert_eq!(e.position(0), a, "the formerly selected model stays");
    assert!((e.position(1) - b).length() > 5.);
}

#[test]
fn a_locked_plate_does_not_move() {
    let mut e = Editor::new();
    let current = e.app.plates.current;
    e.app.plates.plates[current].locked = true;
    let before = e.position(0);
    let p = e.on_screen(0);
    e.drag(p, p + egui::vec2(80., 0.), 8, 0.);
    assert_eq!(e.position(0), before);
}

#[test]
fn outside_warning_says_where_and_offers_the_fixes() {
    let mut e = Editor::new();
    let bed = e.app.bed.clone();
    // 12.4 mm past the back edge.
    {
        let o = &mut e.app.plates.current_mut().objects[0];
        let (_, max) = o.bounds();
        o.position.y += bed.max.y + 12.4 - max.y;
    }
    e.app.changed();
    e.app.refresh_warnings();
    e.app.sync_notifications();
    let plate = e.app.plates.current().id;
    let n = e
        .app
        .notifications
        .visible(Some(plate))
        .into_iter()
        .find(|n| n.text.starts_with("cube20.stl sticks out"))
        .cloned()
        .expect("the outside warning");
    assert_eq!(n.text, "cube20.stl sticks out 12.4 mm past the back edge");
    let labels: Vec<&str> = n.actions.iter().map(|(l, _)| l.as_str()).collect();
    assert_eq!(labels, ["Move inside", "Arrange"]);

    // Larger than the volume: Scale to fit, one undo step, and then it fits.
    {
        let o = &mut e.app.plates.current_mut().objects[0];
        o.scale = Vec3::splat(14.);
        o.drop_to_bed();
    }
    e.app.changed();
    e.app.refresh_warnings();
    e.app.sync_notifications();
    let n = e
        .app
        .notifications
        .visible(Some(plate))
        .into_iter()
        .find(|n| n.text.starts_with("cube20.stl sticks out"))
        .cloned()
        .expect("the outside warning");
    assert!(n.text.contains("mm above the maximum height"), "{}", n.text);
    assert!(
        n.text.ends_with("; it is larger than the bed"),
        "{}",
        n.text
    );
    let scale = n
        .actions
        .iter()
        .find(|(l, _)| l == "Scale to fit")
        .map(|(_, a)| a.clone())
        .expect("Scale to fit");
    assert!(matches!(scale, Action::ScaleToFit { .. }));
    let undo = e.app.undo.len();
    let ctx = e.ctx.clone();
    e.app.run_notification_action(n.id, scale, &ctx);
    let o = &e.app.plates.current().objects[0];
    assert_eq!(crate::scene::outside::outside(o, &bed), None, "it fits now");
    assert!((o.scale.x - o.scale.y).abs() < 1e-6 && (o.scale.y - o.scale.z).abs() < 1e-6);
    let (min, max) = o.bounds();
    assert!(min.z.abs() < 1e-3, "on the bed");
    let centre = (min + max) * 0.5;
    let bed_centre = (bed.min + bed.max) * 0.5;
    assert!((centre.x - bed_centre.x).abs() < 0.01 && (centre.y - bed_centre.y).abs() < 0.01);
    assert_eq!(e.app.undo.len(), undo + 1);
    assert_eq!(e.app.undo.undo_label(), Some("Scale to fit"));
    e.app.undo();
    assert_eq!(e.app.plates.current().objects[0].scale, Vec3::splat(14.));
}

#[test]
fn cmd_drag_orbits_around_the_screen_center_even_over_the_selected_model() {
    let mut e = Editor::new();
    e.app.select(Some(0), false);
    e.frame(vec![]);
    e.pan_away();
    let rect = e.app.viewport_rect;
    let camera = e.app.viewport.camera.clone();
    let pivot = crate::viewport::orbit::under_screen_centre(
        &camera,
        &e.app.plates.current().objects,
        rect.aspect_ratio(),
    );
    assert_eq!(pivot.z, 0.);
    assert!(e.uv(pivot).abs_diff_eq(glam::Vec2::splat(0.5), 1e-3));
    let before = e.position(0);
    let undo = e.app.undo.len();
    e.modifiers = egui::Modifiers::COMMAND;
    // A press on the model with the Move gizmo on: Orca turns the view instead of moving.
    let p = e.on_face(0);
    e.drag(p, p + egui::vec2(120., -40.), 8, 0.);
    e.modifiers = egui::Modifiers::NONE;
    assert_eq!(e.position(0), before, "Cmd-drag does not move the model");
    assert_eq!(e.app.undo.len(), undo, "nothing to undo");
    let after = &e.app.viewport.camera;
    assert!((after.yaw - camera.yaw).abs() > 0.1, "the view turned");
    let uv = e.uv(pivot);
    assert!(
        uv.abs_diff_eq(glam::Vec2::splat(0.5), 2e-3),
        "the point that was under the middle stays there: {uv}"
    );
    // Without Cmd the same press moves the model on the bed again.
    let p = e.on_face(0);
    e.drag(p, p + egui::vec2(80., 0.), 8, 0.);
    let moved = e.position(0) - before;
    assert!(moved.length() > 5. && moved.z == 0., "{moved}");
}

#[test]
fn the_orbit_pivot_follows_the_preference() {
    // Off (default): around the selected model, like Orca.
    let mut e = Editor::new();
    assert!(!e.app.prefs.orbit_screen_center);
    e.app.select(Some(0), false);
    e.frame(vec![]);
    e.pan_away();
    let model = e.app.plates.current().objects[0].pivot_world();
    let at = e.uv(model);
    let spot = e.empty_spot();
    e.drag(spot, spot + egui::vec2(100., 30.), 8, 0.);
    assert!(e.uv(model).abs_diff_eq(at, 2e-3), "the model stays put");

    // On: around the middle of the view (the camera target), whatever is selected.
    let mut e = Editor::new();
    e.app.prefs.orbit_screen_center = true;
    e.app.select(Some(0), false);
    e.frame(vec![]);
    e.pan_away();
    let target = e.app.viewport.camera.target;
    let yaw = e.app.viewport.camera.yaw;
    let model = e.app.plates.current().objects[0].pivot_world();
    let at = e.uv(model);
    let spot = e.empty_spot();
    e.drag(spot, spot + egui::vec2(100., 30.), 8, 0.);
    let camera = &e.app.viewport.camera;
    assert!((camera.yaw - yaw).abs() > 0.1, "the view turned");
    assert!(
        camera.target.distance(target) < 1e-3,
        "{target} → {}",
        camera.target
    );
    assert!(e.uv(target).abs_diff_eq(glam::Vec2::splat(0.5), 1e-3));
    assert!(
        !e.uv(model).abs_diff_eq(at, 0.01),
        "the selection is not the pivot"
    );
}
