// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The Text / SVG tool driven through the whole app (keys, clicks, menus, the object list):
//! every way to add a text puts one on the model at once; a text opens for editing again
//! after Done (object list, T, double-click, "Edit text", from a saved project); typing shows
//! in the 3D view as it goes; Cancel (Esc) puts the text back; undo / redo follow the edits.

use super::Kind;
use crate::app::{App, Options};
use crate::commands::Command;
use crate::scene::Sub;
use eframe::egui::{self, Event, Key, Modifiers, PointerButton, RawInput};
use glam::Vec3;
use mica_core::emboss::EmbossSource;
use mica_core::threemf::{Flavour, PartKind};
use mica_profiles::user::Store;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT: AtomicUsize = AtomicUsize::new(0);

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mica-emboss-ui-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

fn model(name: &str) -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/models/")).join(name)
}

/// The whole app in a 1440 × 900 window with its own clock (double-clicks need one).
struct Ui {
    ctx: egui::Context,
    app: App,
    time: f64,
}

impl Ui {
    fn new(files: Vec<PathBuf>) -> Self {
        let ctx = egui::Context::default();
        let app = App::create(
            &ctx,
            Options {
                prefs_path: None,
                store: Some(Store::open(scratch("store"))),
                files,
                headless: true,
            },
        );
        let mut u = Self { ctx, app, time: 0. };
        for _ in 0..40 {
            u.frame(vec![]);
            if u.app.loading.is_none() && !u.app.plates.is_blank() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        u.frame(vec![]);
        u
    }

    fn models(names: &[&str]) -> Self {
        Self::new(names.iter().map(|n| model(n)).collect())
    }

    /// A saved project opened in a fresh app.
    fn open(path: &std::path::Path) -> Self {
        let u = Self::new(vec![path.to_path_buf()]);
        assert!(!u.app.plates.is_blank(), "{}", u.app.status);
        u
    }

    fn frame(&mut self, events: Vec<Event>) {
        self.time += 1. / 60.;
        let input = RawInput {
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

    fn press(&mut self, key: Key) {
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

    fn button(at: egui::Pos2, pressed: bool) -> Event {
        Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        }
    }

    fn click(&mut self, at: egui::Pos2) {
        self.frame(vec![Event::PointerMoved(at)]);
        self.frame(vec![Self::button(at, true)]);
        self.frame(vec![Self::button(at, false)]);
        self.frame(vec![]);
    }

    fn double_click(&mut self, at: egui::Pos2) {
        // Well after any earlier click (a third quick click is a triple click to egui).
        self.time += 1.;
        self.frame(vec![Event::PointerMoved(at)]);
        self.frame(vec![Self::button(at, true)]);
        self.frame(vec![Self::button(at, false)]);
        self.frame(vec![Self::button(at, true)]);
        self.frame(vec![Self::button(at, false)]);
        self.frame(vec![]);
    }

    fn type_text(&mut self, text: &str) {
        for c in text.chars() {
            self.frame(vec![Event::Text(c.to_string())]);
        }
        self.frame(vec![]);
    }

    /// Where a world point shows on screen.
    fn on_screen(&self, p: Vec3) -> egui::Pos2 {
        let rect = self.app.viewport_rect;
        let uv = self
            .app
            .viewport
            .camera
            .project(p, rect.aspect_ratio())
            .expect("in front of the camera");
        egui::pos2(
            rect.left() + uv.x * rect.width(),
            rect.top() + uv.y * rect.height(),
        )
    }

    /// The object list's row labelled `label`.
    fn row(&self, label: &str) -> egui::Pos2 {
        self.app
            .editor
            .tree
            .probe
            .iter()
            .find(|r| r.label == label)
            .unwrap_or_else(|| {
                panic!(
                    "no row {label}: {:?}",
                    self.app
                        .editor
                        .tree
                        .probe
                        .iter()
                        .map(|r| &r.label)
                        .collect::<Vec<_>>()
                )
            })
            .rect
            .center()
    }

    /// The text field of the open card (the card sits at the 3D view's top right).
    fn text_field(&self) -> egui::Pos2 {
        let r = self.app.viewport_rect;
        egui::pos2(r.right() - 14. - 160., r.top() + 14. + 75.)
    }

    fn target(&self) -> Option<(usize, Option<u64>)> {
        self.app.emboss_target()
    }

    fn volume(&self, i: usize, id: u64) -> &crate::scene::Volume {
        self.app.plates.current().objects[i]
            .volume(id)
            .expect("volume")
    }
}

fn text_of(v: &crate::scene::Volume) -> String {
    v.emboss
        .as_ref()
        .and_then(|e| e.config.text())
        .map(|t| t.text.clone())
        .unwrap_or_default()
}

fn set_text(a: &mut App, text: &str) {
    let text = text.to_string();
    a.edit_emboss(
        move |c| {
            if let EmbossSource::Text(t) = &mut c.source {
                t.text = text;
            }
        },
        false,
    );
}

/// A text volume on object `i` whose mesh is built and lies on (touches) the object.
fn assert_text_on(a: &App, i: usize, id: u64) {
    let o = &a.plates.current().objects[i];
    let v = o.volume(id).expect("text volume");
    assert!(v.emboss.is_some());
    assert_eq!(v.kind, PartKind::Model, "raised by default");
    assert!(!v.mesh.faces.is_empty(), "placed at once, not empty");
    let (a0, a1) = o.volume_bounds(v);
    let (b0, b1) = o.bounds();
    let overlap = (a1.min(b1) - a0.max(b0)).min_element();
    assert!(overlap > -0.05, "on the model: {a0} {a1} vs {b0} {b1}");
}

/// A world point on the face of a letter of text `id` (the middle of its largest
/// triangle facing out of the surface): a click there hits the text, not a gap.
fn on_letter(a: &App, i: usize, id: u64) -> Vec3 {
    let o = &a.plates.current().objects[i];
    let v = o.volume(id).unwrap();
    let frame = v.emboss.as_ref().unwrap().frame;
    let up = frame.transform_vector3(Vec3::Z).normalize();
    let tri = |f: &[u32; 3]| f.map(|k| Vec3::from(v.mesh.vertices[k as usize]));
    let best = v
        .mesh
        .faces
        .iter()
        .map(tri)
        .filter(|[p, q, r]| (*q - *p).cross(*r - *p).normalize_or_zero().dot(up) > 0.99)
        .max_by(|x, y| {
            let area = |[p, q, r]: &[Vec3; 3]| (*q - *p).cross(*r - *p).length();
            area(x).total_cmp(&area(y))
        })
        .expect("a face on the surface");
    let local = (best[0] + best[1] + best[2]) / 3.;
    o.volume_transform(v).transform_point3(local)
}

/// The text volumes of object `i`.
fn texts(a: &App, i: usize) -> Vec<u64> {
    a.plates.current().objects[i]
        .volumes
        .iter()
        .filter(|v| v.emboss.is_some())
        .map(|v| v.id)
        .collect()
}

// ----- adding a text ----------------------------------------------------------------------

/// T with the plate's only model not selected: the text goes on that model (as the other
/// tools pick the only model), raised, placed at once, with the card open on it.
#[test]
fn t_on_a_single_unselected_model_adds_the_text_to_it() {
    let mut u = Ui::models(&["cube20.stl"]);
    u.app.select(None, false);
    u.frame(vec![]);
    u.press(Key::T);
    assert_eq!(u.app.plates.current().objects.len(), 1, "no text object");
    let Some((0, Some(id))) = u.target() else {
        panic!("a text on the cube: {:?}", u.target())
    };
    assert_text_on(&u.app, 0, id);
    // The toolbar button does the same.
    u.app.close_emboss();
    u.app.select(None, false);
    u.app
        .handle(crate::viewport::Action::Emboss, &[], &u.ctx.clone());
    assert_eq!(texts(&u.app, 0).len(), 2);
}

/// Several models: T adds to the selected one; with none selected it makes a text object.
#[test]
fn t_adds_to_the_selected_model_or_makes_a_text_object() {
    let mut u = Ui::models(&["cube20.stl", "hole_block.stl"]);
    u.app.select(Some(1), false);
    u.frame(vec![]);
    u.press(Key::T);
    let Some((1, Some(id))) = u.target() else {
        panic!("a text on the selected model: {:?}", u.target())
    };
    assert_text_on(&u.app, 1, id);
    u.press(Key::T);
    assert!(u.app.emboss.is_none(), "T again is Done");
    assert_eq!(texts(&u.app, 1), [id], "Done keeps it");
    u.app.select(None, false);
    u.frame(vec![]);
    u.press(Key::T);
    assert_eq!(u.app.plates.current().objects.len(), 3);
    assert!(matches!(u.target(), Some((2, None))), "a text object");
}

/// The right-click menu's "Add text…" and "Add SVG…" put a part on that object; the object
/// list's "+ Add" offers Text and SVG too; each add is one undo step.
#[test]
fn menus_add_text_and_svg_parts_to_the_object() {
    let mut u = Ui::models(&["cube20.stl", "hole_block.stl"]);
    let ctx = u.ctx.clone();
    u.app.select(None, false);
    u.app.handle(
        crate::viewport::Action::Command {
            index: Some(0),
            command: Command::AddText,
        },
        &[(0, 0), (0, 1)],
        &ctx,
    );
    let Some((0, Some(a))) = u.target() else {
        panic!("Add text… on the cube: {:?}", u.target())
    };
    assert_text_on(&u.app, 0, a);
    // The object list's + Add › Text on the selected object.
    u.app.close_emboss();
    u.app.select(Some(1), false);
    u.app.run_add_pick(crate::panels::tree::AddPick::Text, &ctx);
    let Some((1, Some(b))) = u.target() else {
        panic!("+ Add › Text: {:?}", u.target())
    };
    assert_text_on(&u.app, 1, b);
    // Add SVG… (after its file dialog): the file's shape on the object, one undo step.
    u.app.close_emboss();
    let svg = scratch("star.svg");
    std::fs::write(
        &svg,
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100"><path d="M50 5 L61 39 L97 39 L68 61 L79 95 L50 74 L21 95 L32 61 L3 39 L39 39 Z"/></svg>"#,
    )
    .unwrap();
    let before = texts(&u.app, 0).len();
    u.app.add_svg_volume(0, &svg);
    let Some((0, Some(c))) = u.target() else {
        panic!("Add SVG…: {:?}", u.target())
    };
    assert_text_on(&u.app, 0, c);
    let v = u.volume(0, c);
    assert!(v.emboss.as_ref().unwrap().config.svg().is_some());
    assert_eq!(v.name, "star.svg");
    u.app.close_emboss();
    u.app.undo();
    assert_eq!(texts(&u.app, 0).len(), before, "one undo step");
    // The menus list the entries.
    let info = |text| crate::commands::MenuInfo {
        tools: 1,
        tool_labels: vec![],
        tool: 0,
        plates: vec![],
        current_plate: 0,
        instances: 1,
        can_paste: false,
        selection: 1,
        locked: false,
        sub: None,
        move_inside: false,
        textures: false,
        text,
    };
    for text in [false, true] {
        let labels = menu_labels(|ui| {
            crate::commands::object_menu(ui, &info(text));
        });
        assert!(labels.contains(&"Add text…".to_string()), "{labels:?}");
        assert!(labels.contains(&"Add SVG…".to_string()), "{labels:?}");
        assert_eq!(
            labels.contains(&"Edit text".to_string()),
            text,
            "{labels:?}"
        );
    }
    let labels = menu_labels(|ui| {
        crate::panels::tree::add_menu(ui, Some("cube20"), &[]);
    });
    assert!(labels.contains(&"Text".to_string()) && labels.contains(&"SVG…".to_string()));
}

/// The button labels a menu body draws.
fn menu_labels(body: impl FnOnce(&mut egui::Ui)) -> Vec<String> {
    let ctx = egui::Context::default();
    let mut body = Some(body);
    let mut out = ctx.run_ui(RawInput::default(), |ui| {
        if let Some(b) = body.take() {
            b(ui)
        }
    });
    out.textures_delta.clear();
    let mut labels = vec![];
    for s in out.shapes {
        collect_text(&s.shape, &mut labels);
    }
    labels
}

fn collect_text(shape: &egui::epaint::Shape, out: &mut Vec<String>) {
    match shape {
        egui::epaint::Shape::Text(t) => out.push(t.galley.text().to_string()),
        egui::epaint::Shape::Vec(v) => v.iter().for_each(|s| collect_text(s, out)),
        _ => {}
    }
}

// ----- editing it again -------------------------------------------------------------------

/// After Done, the text opens again on itself (content, font, size, depth and kind kept)
/// from the object list, T on the selected part, a double-click, and "Edit text"; edits
/// change that same volume.
#[test]
fn a_text_reopens_after_done_from_every_entry() {
    let mut u = Ui::models(&["cube20.stl"]);
    let ctx = u.ctx.clone();
    u.app.select(Some(0), false);
    u.press(Key::T);
    let Some((0, Some(id))) = u.target() else {
        panic!()
    };
    set_text(&mut u.app, "Top");
    u.app.edit_emboss(
        |c| {
            c.depth = 1.5;
            if let EmbossSource::Text(t) = &mut c.source {
                t.size_mm = 12.;
            }
        },
        false,
    );
    u.app.set_emboss_kind(Kind::Engraved);
    let style = u.volume(0, id).emboss.clone().unwrap();
    // Done.
    u.app.close_emboss();
    u.frame(vec![]);
    let reopened = |u: &Ui, how: &str| {
        assert_eq!(u.target(), Some((0, Some(id))), "{how}");
        let v = u.volume(0, id);
        assert_eq!(v.emboss.as_ref().unwrap().config, style.config, "{how}");
        assert_eq!(v.kind, PartKind::NegativeVolume, "{how}");
        assert_eq!(texts(&u.app, 0), [id], "{how}: no second text");
    };
    // 1. The object list: a click on the text's row.
    u.app.select(None, false);
    u.frame(vec![]);
    u.app.select(Some(0), false);
    u.frame(vec![]);
    let row = u.row("Text 1");
    u.click(row);
    reopened(&u, "object list");
    u.app.close_emboss();
    // 2. T with the part selected.
    u.app.sub = Some(Sub::Volume(id));
    u.frame(vec![]);
    u.press(Key::T);
    reopened(&u, "T");
    u.app.close_emboss();
    // 3. A double-click on the text in the 3D view (nothing selected first).
    u.app.select(None, false);
    u.frame(vec![]);
    let at = u.on_screen(on_letter(&u.app, 0, id));
    u.double_click(at);
    reopened(&u, "double-click");
    u.app.close_emboss();
    // 4. "Edit text" from the right-click menu.
    u.app.select(Some(0), false);
    u.app.sub = Some(Sub::Volume(id));
    u.app.run_command(Command::EditText, &ctx);
    reopened(&u, "Edit text");
    // The edit changes that volume.
    set_text(&mut u.app, "Again");
    assert_eq!(text_of(u.volume(0, id)), "Again");
}

/// With the card open on one text, picking another text in the list moves the card to it;
/// picking a plain part or the object closes it (the edits stay).
#[test]
fn the_card_follows_the_selected_text() {
    let mut u = Ui::models(&["cube20.stl"]);
    u.app.select(Some(0), false);
    u.press(Key::T);
    let Some((0, Some(a))) = u.target() else {
        panic!()
    };
    set_text(&mut u.app, "One");
    u.app.close_emboss();
    u.app.add_text_volume(0, Kind::Raised);
    let Some((0, Some(b))) = u.target() else {
        panic!()
    };
    set_text(&mut u.app, "Two");
    u.frame(vec![]);
    let row = u.row("Text 1");
    u.click(row);
    assert_eq!(u.target(), Some((0, Some(a))));
    assert_eq!(text_of(u.volume(0, b)), "Two", "kept");
    let row = u.row("cube20");
    u.click(row);
    assert!(u.app.emboss.is_none());
}

// ----- live preview -----------------------------------------------------------------------

/// Typing in the card rebuilds the mesh at once (the 3D view draws the volume's current
/// mesh), the whole typing is one undo step, and Done just closes.
#[test]
fn typing_shows_as_it_goes_and_is_one_undo_step() {
    let mut u = Ui::models(&["cube20.stl"]);
    u.app.select(Some(0), false);
    u.press(Key::T);
    let Some((0, Some(id))) = u.target() else {
        panic!()
    };
    let before = u.volume(0, id).clone();
    let field = u.text_field();
    u.click(field);
    u.type_text("!!");
    let v = u.volume(0, id).clone();
    assert_eq!(text_of(&v), "Text!!", "typed into the card");
    assert!(
        !std::sync::Arc::ptr_eq(&v.mesh, &before.mesh),
        "the mesh is rebuilt while typing"
    );
    assert!(
        v.local_bounds.1[0] - v.local_bounds.0[0]
            > before.local_bounds.1[0] - before.local_bounds.0[0]
    );
    // What the 3D view is handed is that mesh.
    let (objects, _) = u.app.scene_objects();
    let shown = objects[0].volume(id).unwrap();
    assert!(std::sync::Arc::ptr_eq(&shown.mesh, &v.mesh));
    // Done changes nothing more.
    u.app.close_emboss();
    assert!(std::sync::Arc::ptr_eq(&u.volume(0, id).mesh, &v.mesh));
    // One undo step for the typing; redo brings it back.
    u.app.undo();
    assert_eq!(text_of(u.volume(0, id)), "Text");
    assert!(std::sync::Arc::ptr_eq(&u.volume(0, id).mesh, &before.mesh));
    u.app.redo();
    assert_eq!(text_of(u.volume(0, id)), "Text!!");
}

/// Wrapped onto a detailed surface the rebuild runs in the background: the edit returns at
/// once (before the mesh is rebuilt) and the mesh lands a few frames later. Deterministic
/// under load: it checks the order of events, not the clock, and waits for the rebuild
/// itself (the time limit only catches a rebuild that never ends).
#[test]
fn a_wrapped_text_rebuilds_in_the_background() {
    let mut u = Ui::models(&["3DBenchy.stl"]);
    u.app.select(Some(0), false);
    u.press(Key::T);
    let Some((0, Some(id))) = u.target() else {
        panic!()
    };
    u.app.edit_emboss(|c| c.use_surface = true, false);
    u.app.finish_emboss_rebuild();
    let wrapped = u.volume(0, id).mesh.clone();
    set_text(&mut u.app, "Mica Slicer 2026");
    // The edit returned without wrapping: the old mesh is still there, the rebuild pending.
    assert!(u.app.emboss.as_ref().unwrap().rebuild.is_some());
    assert!(
        std::sync::Arc::ptr_eq(&u.volume(0, id).mesh, &wrapped),
        "the edit itself does not rebuild the wrapped mesh"
    );
    // Frames apply the rebuild once it is done (the frame loop polls it, as the app does).
    let hang = std::time::Instant::now() + std::time::Duration::from_secs(120);
    while u.app.emboss.as_ref().unwrap().rebuild.is_some() {
        assert!(
            std::time::Instant::now() < hang,
            "the rebuild never finished"
        );
        u.frame(vec![]);
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    let v = u.volume(0, id);
    assert!(!std::sync::Arc::ptr_eq(&v.mesh, &wrapped), "landed");
    assert_eq!(text_of(v), "Mica Slicer 2026");
    assert!(
        v.local_bounds.1[0] - v.local_bounds.0[0] > 40.,
        "{:?}",
        v.local_bounds
    );
}

// ----- cancel, undo -----------------------------------------------------------------------

/// Esc (Cancel) puts an existing text back as it was when the card opened, in one undo
/// step (undo brings the edits back); for a text the card added, Cancel takes it away.
#[test]
fn cancel_restores_the_original_or_removes_a_new_text() {
    let mut u = Ui::models(&["cube20.stl"]);
    u.app.select(Some(0), false);
    u.press(Key::T);
    let Some((0, Some(id))) = u.target() else {
        panic!()
    };
    set_text(&mut u.app, "Keep");
    u.app.close_emboss();
    let kept = u.volume(0, id).clone();
    // Reopen, change text, depth, kind and place; Esc.
    u.app.sub = Some(Sub::Volume(id));
    u.press(Key::T);
    set_text(&mut u.app, "Drop");
    u.app.edit_emboss(|c| c.depth = 3., false);
    u.app.set_emboss_kind(Kind::Modifier);
    let (min, max) = u.app.plates.current().objects[0].bounds();
    let c = (min + max) * 0.5;
    u.app.emboss_place(0, Vec3::new(c.x, min.y, c.z), -Vec3::Y);
    u.frame(vec![]);
    u.press(Key::Escape);
    assert!(u.app.emboss.is_none());
    assert_eq!(*u.volume(0, id), kept, "back as it was");
    u.app.undo();
    assert_eq!(
        text_of(u.volume(0, id)),
        "Drop",
        "undo brings the edits back"
    );
    u.app.redo();
    assert_eq!(*u.volume(0, id), kept);
    // Cancel on a text the card just added removes it.
    u.app.select(Some(0), false);
    u.app.add_text_volume(0, Kind::Raised);
    assert_eq!(texts(&u.app, 0).len(), 2);
    u.frame(vec![]);
    u.press(Key::Escape);
    assert_eq!(texts(&u.app, 0), [id]);
    // Unchanged: Cancel adds no undo step.
    u.app.sub = Some(Sub::Volume(id));
    u.press(Key::T);
    let steps = u.app.undo.len();
    u.press(Key::Escape);
    assert_eq!(u.app.undo.len(), steps);
}

/// A text object: Esc puts its text and mesh back.
#[test]
fn cancel_restores_a_text_object() {
    let mut u = Ui::models(&["cube20.stl", "hole_block.stl"]);
    u.app.select(None, false);
    u.press(Key::T);
    let Some((i, None)) = u.target() else {
        panic!()
    };
    u.app.close_emboss();
    let kept = u.app.plates.current().objects[i].clone();
    u.app.select(Some(i), false);
    u.press(Key::T);
    assert_eq!(u.target(), Some((i, None)), "T on a text object edits it");
    set_text(&mut u.app, "Changed");
    u.frame(vec![]);
    u.press(Key::Escape);
    assert!(u.app.plates.current().objects[i] == kept);
}

// ----- saved projects ---------------------------------------------------------------------

/// Texts (raised, engraved, modifier and an SVG) saved by Mica and as PrusaSlicer writes
/// them (`slic3rpe:shape` / `slic3rpe:text` metadata) open for editing from the object list,
/// edit the same volume, undo, and Cancel back to the loaded state.
#[test]
fn texts_from_a_saved_project_edit_in_place() {
    let mut u = Ui::models(&["cube20.stl"]);
    u.app.select(Some(0), false);
    for (kind, text) in [
        (Kind::Raised, "Up"),
        (Kind::Engraved, "In"),
        (Kind::Modifier, "Mod"),
    ] {
        u.app.add_text_volume(0, kind);
        set_text(&mut u.app, text);
        u.app.close_emboss();
    }
    let svg = scratch("star.svg");
    std::fs::write(
        &svg,
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100"><path d="M50 5 L61 39 L97 39 L68 61 L79 95 L50 74 L21 95 L32 61 L3 39 L39 39 Z"/></svg>"#,
    )
    .unwrap();
    u.app.add_svg_volume(0, &svg);
    u.app.close_emboss();
    for flavour in [Flavour::Prusa, Flavour::Orca] {
        let path = scratch(&format!("texts-{}.3mf", flavour.name()));
        u.app.save_project_to(&path, flavour).unwrap();
        let mut b = Ui::open(&path);
        b.app.select(Some(0), false);
        b.frame(vec![]);
        for (name, kind) in [
            ("Text 2", PartKind::NegativeVolume),
            ("Text 3", PartKind::Modifier),
            ("star.svg", PartKind::Model),
        ] {
            let o = &b.app.plates.current().objects[0];
            let v = o
                .volumes
                .iter()
                .find(|v| v.name == name)
                .unwrap_or_else(|| panic!("{flavour:?}: {name}"));
            assert_eq!(v.kind, kind, "{flavour:?} {name}");
            let (id, loaded) = (v.id, v.clone());
            let row = b.row(name);
            b.click(row);
            assert_eq!(b.target(), Some((0, Some(id))), "{flavour:?} {name}");
            let is_text = loaded.emboss.as_ref().unwrap().config.text().is_some();
            if is_text {
                set_text(&mut b.app, "Edited");
                assert_eq!(text_of(b.volume(0, id)), "Edited");
            } else {
                b.app.edit_emboss(|c| c.depth = 2.5, false);
            }
            assert!(!std::sync::Arc::ptr_eq(&b.volume(0, id).mesh, &loaded.mesh));
            b.app.undo();
            assert_eq!(*b.volume(0, id), loaded, "{flavour:?} {name}: undo");
            b.app.redo();
            b.frame(vec![]);
            b.press(Key::Escape);
            assert_eq!(*b.volume(0, id), loaded, "{flavour:?} {name}: cancel");
            b.app.select(Some(0), false);
            b.frame(vec![]);
        }
    }
}
