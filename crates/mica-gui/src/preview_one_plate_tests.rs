// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Preview with several plates draws the current plate alone, driven headless through
//! whole-window frames: only its bed reaches the renderer, no plate strips lie on the ground,
//! showing another plate (the plate list, the plate bar's path) draws that plate and frames
//! its bed with the orbit angle kept, and a camera already close in on the plate is left as
//! it is. Prepare keeps the whole grid with its strips.

use crate::app::{App, Options};
use crate::viewport::Viewport;
use eframe::egui::{self, Pos2, Rect};
use glam::{Vec2, Vec3};
use mica_profiles::user::Store;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

static NEXT: AtomicU64 = AtomicU64::new(0);
const CORE_ONE: &str = "Prusa CORE One HF0.4 nozzle";

struct Window {
    ctx: egui::Context,
    app: App,
    time: f64,
}

impl Window {
    /// The CORE One with cube20 sliced on plate 1 and a copy of it on plate 2 (not sliced),
    /// plate 1 current, in Prepare.
    fn two_plates() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "mica-preview-one-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let ctx = egui::Context::default();
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
        app.slice_current(&ctx);
        let deadline = Instant::now() + Duration::from_secs(300);
        while app.busy() && Instant::now() < deadline {
            app.poll(&ctx);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(app.finished().is_some(), "{}", app.status);
        app.plate_command(0, crate::plate_menu::PlateCommand::Duplicate);
        app.select_plate(0);
        let mut w = Self { ctx, app, time: 0. };
        w.frames(3);
        w
    }

    fn frame(&mut self, events: Vec<egui::Event>) {
        self.time += 0.05;
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(1440., 900.))),
            time: Some(self.time),
            predicted_dt: 0.05,
            events,
            ..Default::default()
        };
        let app = &mut self.app;
        let mut output = self.ctx.run_ui(input, |ui| app.show(ui));
        output.textures_delta.clear();
    }

    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            self.frame(vec![]);
        }
    }

    fn click(&mut self, at: Pos2) {
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        self.frame(vec![egui::Event::PointerMoved(at)]);
        self.frame(vec![button(true)]);
        self.frame(vec![button(false)]);
        self.frames(2);
    }

    /// The printable area of plate `i` in world millimetres.
    fn bed_of(&self, i: usize) -> (Vec2, Vec2) {
        let o = self.app.plates.offset(i).truncate();
        (
            self.app.bed.min.truncate() + o,
            self.app.bed.max.truncate() + o,
        )
    }

    /// Whether the camera aims at a point over plate `i`'s printable area.
    fn aims_at(&self, i: usize) -> bool {
        let (lo, hi) = self.bed_of(i);
        let t = self.app.viewport.camera.target.truncate();
        t.cmpge(lo).all() && t.cmple(hi).all()
    }
}

#[test]
fn preview_draws_the_current_plate_alone_and_prepare_the_grid() {
    let mut w = Window::two_plates();
    // Prepare: both beds, and the strips in front of them.
    assert_eq!(w.app.viewport.drawn_plates, [0, 1]);
    assert!(
        w.app.viewport.plate_tag_count() > 0,
        "Prepare's plate strips"
    );
    // Preview: plate 1's bed alone, no strips (the plate list names the plates).
    w.app.preview = true;
    w.frames(3);
    assert!(w.app.previewing());
    assert_eq!(w.app.viewport.drawn_plates, [0]);
    assert_eq!(w.app.viewport.plate_tag_count(), 0);
    assert!(w.aims_at(0));
    // The plate list shows plate 2 (no slice: its models): its bed alone, framed.
    let row = w.app.viewport.preview_plates_probe.rows[1].1;
    w.click(row.center());
    assert_eq!(w.app.plates.current, 1);
    assert!(w.app.previewing(), "still in Preview");
    assert_eq!(w.app.viewport.drawn_plates, [1]);
    assert_eq!(w.app.viewport.plate_tag_count(), 0);
    assert!(w.aims_at(1), "the camera moved to plate 2's bed");
    // Back to plate 1 the way the plate bar and the keyboard do it.
    w.app.select_plate(0);
    w.frames(2);
    assert_eq!(w.app.viewport.drawn_plates, [0]);
    assert!(w.aims_at(0));
    // Prepare again: the grid.
    w.app.preview = false;
    w.frames(2);
    assert_eq!(w.app.viewport.drawn_plates, [0, 1]);
}

#[test]
fn preview_frames_the_bed_keeping_the_orbit_and_a_close_look() {
    let mut w = Window::two_plates();
    let (yaw, pitch) = (-0.6, 0.5);
    w.app.viewport.camera.yaw = yaw;
    w.app.viewport.camera.pitch = pitch;
    // Prepare looking at the whole grid: entering Preview frames plate 1's bed.
    w.app.fit_current_plate(true);
    let grid_span = w.app.viewport.camera.span;
    w.app.preview = true;
    w.frames(2);
    let cam = w.app.viewport.camera.clone();
    assert!(w.aims_at(0));
    assert!(cam.span < grid_span, "{} < {grid_span}", cam.span);
    assert_eq!((cam.yaw, cam.pitch), (yaw, pitch), "the orbit angle stays");
    // A close look at a detail of the plate stays put, frame after frame.
    w.app.viewport.camera.span /= 3.;
    w.app.viewport.camera.span_goal = w.app.viewport.camera.span;
    w.app.viewport.camera.target += Vec3::new(20., 10., 5.);
    let close = w.app.viewport.camera.clone();
    w.frames(3);
    assert_eq!(w.app.viewport.camera, close);
    // Leaving Preview and coming back keeps it too.
    w.app.preview = false;
    w.frames(2);
    w.app.preview = true;
    w.frames(2);
    assert_eq!(w.app.viewport.camera, close);
    // Another plate from the plate list: its bed, framed as plate 1's was, at the same
    // orbit angle.
    let row = w.app.viewport.preview_plates_probe.rows[1].1;
    w.click(row.center());
    assert_eq!(w.app.plates.current, 1);
    let moved = &w.app.viewport.camera;
    assert!(w.aims_at(1));
    assert!(
        (moved.span - cam.span).abs() < 1e-3,
        "{} vs {}",
        moved.span,
        cam.span
    );
    assert_eq!((moved.yaw, moved.pitch), (yaw, pitch));
    let shift = moved.target - cam.target;
    assert!(
        (shift - w.app.plates.offset(1) + w.app.plates.offset(0)).length() < 1e-3,
        "the same framing moved by the plate offset: {shift:?}"
    );
}

#[test]
fn preview_framing_is_the_bed_with_its_sheet_and_single_plates_are_unchanged() {
    use crate::scene::plates::Plate;
    let mut w = Window::two_plates();
    w.frames(1);
    let plates = crate::scene::plates::snapshot(&w.app.plates);
    let bed = &w.app.bed;
    let (pre_min, pre_max) = Viewport::plate_framing(bed, &plates, false, false);
    let (pv_min, pv_max) = Viewport::plate_framing(bed, &plates, false, true);
    // Preview leaves out the strip and name line, but keeps the sheet in front of the bed.
    assert!(pv_min.y > pre_min.y);
    assert!(pv_min.y <= bed.min.y - bed.overhang().front);
    assert_eq!(pv_max, pre_max);
    // Fit all in Preview: the one plate shown.
    assert_eq!(
        Viewport::plate_framing(bed, &plates, true, true),
        (pv_min, pv_max)
    );
    // One plate: the same framing in both modes, as before.
    let one: Vec<Plate> = plates[..1].to_vec();
    assert_eq!(
        Viewport::plate_framing(bed, &one, false, true),
        Viewport::plate_framing(bed, &one, false, false)
    );
    assert_eq!(
        Viewport::plate_framing(bed, &one, false, false),
        crate::scene::plates::plate_bounds(bed, Vec2::ZERO)
    );
}
