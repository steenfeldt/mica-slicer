// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Dragging models from the Library onto the plate, end to end in the headless app with real
//! pointer events: a card dragged over the Prepare tab switches to Prepare and lands where it
//! is dropped, a drop onto an object adds a part (its kind chosen afterwards, still one undo
//! step), picked cards are dragged together, and G-code opens in Preview instead.

use crate::app::{App, Options};
use eframe::egui;
use glam::Vec3;
use mica_core::threemf::PartKind;
use mica_library::testkit::{TempDir, repo_root, sample_gcode};
use mica_profiles::user::Store;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

static NEXT: AtomicU64 = AtomicU64::new(0);
const CORE_ONE: &str = "Prusa CORE One HF0.4 nozzle";

struct Harness {
    app: App,
    ctx: egui::Context,
    time: f64,
    /// Texts drawn in the last frame and their centres.
    texts: Vec<(String, egui::Pos2)>,
    _dirs: Vec<TempDir>,
}

fn texts(shape: egui::Shape, out: &mut Vec<(String, egui::Pos2)>) {
    match shape {
        egui::Shape::Text(t) => {
            out.push((t.galley.text().to_string(), t.pos + t.galley.size() / 2.))
        }
        egui::Shape::Vec(shapes) => {
            for s in shapes {
                texts(s, out);
            }
        }
        _ => {}
    }
}

fn button(pos: egui::Pos2, pressed: bool, modifiers: egui::Modifiers) -> egui::Event {
    egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers,
    }
}

impl Harness {
    /// The CORE One with `cube20.stl` on the plate, and a library of a few models (in a
    /// temporary folder) open in the Library tab.
    fn new() -> Self {
        Self::with(false)
    }

    /// `downloads`: also the synthetic downloads (project cards: Cable Organizer, Desk Hook, …).
    fn with(downloads: bool) -> Self {
        let ctx = egui::Context::default();
        let dir = std::env::temp_dir().join(format!(
            "mica-lib-drop-{}-{}",
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
        let models = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/models/"));
        let result = crate::app::load_files(
            vec![models.join("cube20.stl")],
            &app.config,
            app.bed.center(),
        );
        app.finish_load(result);
        app.select(None, false);
        let data = TempDir::new("drop-data");
        let lib = TempDir::new("drop-lib");
        for f in ["taper.stl", "thin_features.stl"] {
            std::fs::copy(repo_root().join("tests/models").join(f), lib.path().join(f)).unwrap();
        }
        std::fs::write(
            lib.path().join("sliced.gcode"),
            sample_gcode(CORE_ONE, &["#E5534B"], &[3.]),
        )
        .unwrap();
        if downloads {
            mica_library::testkit::synthetic_downloads(lib.path(), 0).unwrap();
        }
        app.library.open(
            data.path(),
            mica_library::Options {
                watch: false,
                ..Default::default()
            },
        );
        let l = app.library.lib.as_ref().unwrap();
        l.add_folder(lib.path()).unwrap();
        assert!(l.wait_idle(Duration::from_secs(60)));
        let mut h = Self {
            app,
            ctx,
            time: 0.,
            texts: vec![],
            _dirs: vec![data, lib],
        };
        for _ in 0..20 {
            h.frame(vec![]);
        }
        h
    }

    fn frame(&mut self, events: Vec<egui::Event>) {
        self.time += 1. / 60.;
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
        self.texts.clear();
        for s in output.shapes {
            texts(s.shape, &mut self.texts);
        }
    }

    fn show_library(&mut self) {
        self.app.library.shown = true;
        self.frame(vec![]);
        self.frame(vec![]);
    }

    fn text(&self, t: &str) -> egui::Pos2 {
        self.texts
            .iter()
            .find(|(s, _)| s == t)
            .unwrap_or_else(|| panic!("no {t:?} on screen"))
            .1
    }

    /// Where a plate-local point shows on screen.
    fn on_screen(&self, local: Vec3) -> egui::Pos2 {
        let world = local + self.app.plates.offset(self.app.plates.current);
        let rect = self.app.viewport_rect;
        let uv = self
            .app
            .viewport
            .camera
            .project(world, rect.aspect_ratio())
            .expect("in front of the camera");
        egui::pos2(
            rect.left() + uv.x * rect.width(),
            rect.top() + uv.y * rect.height(),
        )
    }

    /// Presses on the card named `card`, drags over the Prepare tab (Prepare opens) and on to
    /// the plate-local point `to` (computed once Prepare shows), and releases there.
    fn drag_card(&mut self, card: &str, to: impl Fn(&Self) -> egui::Pos2) {
        let from = self.text(card);
        let tab = self.text("Prepare");
        self.frame(vec![egui::Event::PointerMoved(from)]);
        self.frame(vec![button(from, true, egui::Modifiers::NONE)]);
        for k in 1..=8 {
            self.frame(vec![egui::Event::PointerMoved(
                from.lerp(tab, k as f32 / 8.),
            )]);
        }
        for _ in 0..3 {
            self.frame(vec![]);
        }
        assert!(
            !self.app.library.shown,
            "Prepare opened under the dragged card"
        );
        let to = to(self);
        for k in 1..=8 {
            self.frame(vec![egui::Event::PointerMoved(tab.lerp(to, k as f32 / 8.))]);
        }
        self.frame(vec![button(to, false, egui::Modifiers::NONE)]);
        self.frame(vec![]);
    }

    fn objects(&self) -> usize {
        self.app.plates.current().objects.len()
    }
}

#[test]
fn a_card_dropped_on_the_bed_lands_there_in_one_undo_step() {
    let mut h = Harness::new();
    h.show_library();
    let undo = h.app.undo.len();
    let spot = Vec3::new(60., 70., 0.);
    h.drag_card("taper.stl", |h| h.on_screen(spot));
    assert_eq!(h.objects(), 2, "the model was added");
    let o = &h.app.plates.current().objects[1];
    let (min, max) = o.bounds();
    let centre = (min + max) * 0.5;
    assert!(
        (centre.x - spot.x).abs() < 1.5 && (centre.y - spot.y).abs() < 1.5,
        "lands where it was dropped: {centre}"
    );
    assert!(min.z.abs() < 1e-3, "on the bed");
    assert_eq!(h.app.selected, Some(1), "the new model is selected");
    assert_eq!(h.app.undo.len(), undo + 1);
    assert_eq!(h.app.undo.undo_label(), Some("Add from library"));
    h.app.undo();
    assert_eq!(h.objects(), 1);
}

#[test]
fn a_card_dropped_onto_an_object_becomes_a_part_of_the_chosen_kind() {
    let mut h = Harness::new();
    h.show_library();
    let undo = h.app.undo.len();
    let (min, max) = h.app.plates.current().objects[0].bounds();
    let top = ((min + max) * 0.5).with_z(max.z - 0.5);
    h.drag_card("thin_features.stl", |h| h.on_screen(top));
    assert_eq!(h.objects(), 1, "no new object");
    let cube = &h.app.plates.current().objects[0];
    assert_eq!(cube.volumes.len(), 1, "a part was added to the cube");
    let v = &cube.volumes[0];
    assert_eq!(v.kind, PartKind::Model, "Part by default");
    assert_eq!(v.name, "thin_features");
    // A model, not a primitive: the part card calls it a mesh (Change shape still offered).
    assert_eq!(crate::intents::primitive_shape(&v.mesh), None);
    for s in [
        crate::commands::Shape::Box,
        crate::commands::Shape::Cylinder,
        crate::commands::Shape::Sphere,
    ] {
        let m = crate::intents::fitted(s, Vec3::new(10., 14., 6.));
        assert_eq!(crate::intents::primitive_shape(&m), Some(s));
    }
    // Centred where it was dropped (on the cube's top).
    let world = cube.transform().transform_point3(v.position);
    assert!((world - top).length() < 1.5, "{world} vs {top}");
    // The kind choice: Modifier, by clicking it.
    let popover = h
        .app
        .library
        .popover
        .clone()
        .expect("the part-kind choice is open");
    assert_eq!(popover.kind, PartKind::Model);
    h.frame(vec![]);
    let at = h.text("Modifier");
    h.frame(vec![egui::Event::PointerMoved(at)]);
    h.frame(vec![button(at, true, egui::Modifiers::NONE)]);
    h.frame(vec![button(at, false, egui::Modifiers::NONE)]);
    h.frame(vec![]);
    let v = &h.app.plates.current().objects[0].volumes[0];
    assert_eq!(v.kind, PartKind::Modifier);
    assert_eq!(
        v.overrides.get("fill_density").map(String::as_str),
        Some("100%")
    );
    assert_eq!(
        h.app.undo.len(),
        undo + 1,
        "the drop and its kind are one step"
    );
    assert_eq!(h.app.undo.undo_label(), Some("Add part from library"));
    h.app.undo();
    assert!(h.app.plates.current().objects[0].volumes.is_empty());
    h.frame(vec![]);
    assert!(
        h.app.library.popover.is_none(),
        "the choice closes with the undo"
    );
}

#[test]
fn picked_cards_are_dragged_together_and_gcode_opens_in_preview() {
    let mut h = Harness::new();
    h.show_library();
    // ⌘-click picks a second card.
    let a = h.text("taper.stl");
    let b = h.text("thin_features.stl");
    h.frame(vec![egui::Event::PointerMoved(a)]);
    h.frame(vec![button(a, true, egui::Modifiers::NONE)]);
    h.frame(vec![button(a, false, egui::Modifiers::NONE)]);
    h.frame(vec![
        egui::Event::PointerMoved(b),
        egui::Event::ModifiersChanged(egui::Modifiers::COMMAND),
    ]);
    h.frame(vec![button(b, true, egui::Modifiers::COMMAND)]);
    h.frame(vec![button(b, false, egui::Modifiers::COMMAND)]);
    h.frame(vec![egui::Event::ModifiersChanged(egui::Modifiers::NONE)]);
    assert_eq!(h.app.library.picked.len(), 1);
    let undo = h.app.undo.len();
    let spot = Vec3::new(170., 150., 0.);
    h.drag_card("thin_features.stl", |h| h.on_screen(spot));
    assert_eq!(h.objects(), 3, "both picked models were added");
    assert_eq!(h.app.undo.len(), undo + 1, "in one undo step");
    assert_eq!(h.app.selection(), vec![1, 2]);
    // G-code is not added to the plate: it opens in Preview.
    h.show_library();
    h.drag_card("sliced.gcode", |h| h.on_screen(Vec3::new(60., 60., 0.)));
    assert_eq!(h.objects(), 3);
    assert!(h.app.viewing_gcode(), "the G-code opened in Preview");
}

#[test]
fn the_details_preview_follows_the_selection_and_is_freed() {
    let mut h = Harness::new();
    h.show_library();
    let tex = |h: &Harness| h.ctx.tex_manager().read().num_allocated();
    let id = h
        .app
        .library
        .visible()
        .find(|m| m.name == "taper.stl")
        .unwrap()
        .id
        .clone();
    h.app.library.selected = Some(id.clone());
    let start = std::time::Instant::now();
    loop {
        h.frame(vec![]);
        let ready = h
            .app
            .library
            .preview
            .as_ref()
            .is_some_and(|p| p.id == id && p.settled());
        if ready {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "the preview never showed"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(h.texts.iter().any(|(t, _)| t.contains("triangles")));
    assert!(h.texts.iter().any(|(t, _)| t == "Fits"));
    let with_preview = tex(&h);
    // Another selection: the old preview (texture, mesh, thread) goes.
    h.app.library.selected = None;
    h.frame(vec![]);
    h.frame(vec![]);
    assert!(h.app.library.preview.is_none());
    assert!(tex(&h) < with_preview, "the preview's texture was freed");
}

#[test]
fn a_project_card_and_a_file_inside_a_project_drag_onto_the_plate() {
    let mut h = Harness::with(true);
    h.show_library();
    // The project card carries all its parts.
    let undo = h.app.undo.len();
    h.drag_card("Cable Organizer", |h| h.on_screen(Vec3::new(70., 150., 0.)));
    assert_eq!(h.objects(), 4, "the three parts were added");
    assert_eq!(h.app.undo.len(), undo + 1, "in one undo step");
    // A file inside the opened project: onto the cube as a part.
    let folder = h
        .app
        .library
        .projects()
        .list
        .iter()
        .find(|p| p.name == "Desk Hook")
        .unwrap()
        .folder
        .clone();
    h.app.library.open_project = Some(folder);
    h.show_library();
    let (min, max) = h.app.plates.current().objects[0].bounds();
    let top = ((min + max) * 0.5).with_z(max.z - 0.5);
    h.drag_card("desk-hook.stl", |h| h.on_screen(top));
    let cube = &h.app.plates.current().objects[0];
    assert_eq!(cube.volumes.len(), 1, "a part of the cube");
    assert_eq!(cube.volumes[0].name, "desk-hook");
    // The details panel works for a file inside a project.
    h.show_library();
    let id = h
        .app
        .library
        .snapshot()
        .models
        .iter()
        .find(|m| m.name == "desk-hook.stl")
        .unwrap()
        .id
        .clone();
    h.app.library.selected = Some(id.clone());
    let start = std::time::Instant::now();
    while !h
        .app
        .library
        .preview
        .as_ref()
        .is_some_and(|p| p.id == id && p.settled())
    {
        h.frame(vec![]);
        assert!(start.elapsed() < Duration::from_secs(30), "no preview");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(h.texts.iter().any(|(t, _)| t == "Fits"));
}
