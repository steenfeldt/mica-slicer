// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Cut connectors end to end in the headless editor with real pointer events: in "Add
//! connectors" mode a click on the cut face places a connector even though the model is in
//! front of the plane (the top of the cube hides it), quick clicks (which egui reports as a
//! double click) place one each instead of refocusing the camera; "Place automatically"
//! fills the face and asks before replacing its own; Cut after "Add connectors" with none
//! placed asks first; the Cut button names the result; a cut with automatic dowels makes
//! holed halves and the dowels.

use crate::app::{App, Options};
use crate::cut::{AutoPlaced, CutPrompt};
use eframe::egui;
use glam::Vec3;
use mica_core::{
    cut_connectors::{Attributes, ConnectorType},
    threemf::PartKind,
};
use mica_profiles::user::Store;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);
const CORE_ONE: &str = "Prusa CORE One HF0.4 nozzle";

struct Editor {
    app: App,
    ctx: egui::Context,
    time: f64,
}

impl Editor {
    /// The CORE One with `model` in the middle of the bed, selected, the cut tool open.
    fn with_cut(model: &str) -> Self {
        let ctx = egui::Context::default();
        let dir = std::env::temp_dir().join(format!(
            "mica-cut-ui-{}-{}",
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
        let model =
            PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/models/")).join(model);
        let result = crate::app::load_files(vec![model], &app.config, app.bed.center());
        app.finish_load(result);
        app.select(Some(0), false);
        app.toggle_cut();
        let mut e = Self { app, ctx, time: 0. };
        for _ in 0..30 {
            e.frame(vec![]);
        }
        e
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

    /// A press and release at `at` without movement.
    fn click(&mut self, at: egui::Pos2) {
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        self.frame(vec![egui::Event::PointerMoved(at)]);
        self.frame(vec![button(true)]);
        self.frame(vec![button(false)]);
    }

    /// New connectors `d` mm across.
    fn set_connector_size(&mut self, d: f32) {
        let c = self.app.cut.as_mut().unwrap();
        let (a, mut s) = c.connector;
        s.radius = d / 2.;
        c.set_connector(a, s);
    }

    fn cut(&self) -> &crate::cut::CutState {
        self.app.cut.as_ref().expect("cut tool open")
    }
}

/// The plane runs through the middle of the cube, hidden under its top: a click over the cut
/// face still places a connector there, and the model is neither moved nor reselected.
#[test]
fn a_click_places_a_connector_on_the_plane_behind_the_model() {
    let mut e = Editor::with_cut("cube20.stl");
    e.app.cut.as_mut().unwrap().editing = true;
    let camera = e.app.viewport.camera.clone();
    let position = e.app.plates.current().objects[0].position;
    let c = e.cut().clone();
    let (x, y) = c.axes();
    let target = c.center + x * 4. + y * -3.;
    let p = e.on_screen(target);
    e.click(p);
    assert_eq!(e.cut().connectors.len(), 1, "{}", e.app.status);
    let placed = e.cut().connector_point(&e.cut().connectors[0]);
    // The click lands where the view ray meets the plane (a pixel is well under 1 mm).
    assert!((placed - target).length() < 1., "{placed} vs {target}");
    assert_eq!(e.app.plates.current().objects[0].position, position);
    assert_eq!(e.app.selected, Some(0));
    assert_eq!(e.app.viewport.camera, camera, "the view stays put");
}

/// Two quick clicks are a double click to egui; each must still place a connector (the
/// double click used to focus the camera on the model instead).
#[test]
fn quick_clicks_place_one_connector_each() {
    let mut e = Editor::with_cut("cube20.stl");
    e.app.cut.as_mut().unwrap().editing = true;
    let camera = e.app.viewport.camera.clone();
    let c = e.cut().clone();
    let (x, _) = c.axes();
    // 2 mm apart: a few pixels, close enough for egui to call the second click a double.
    let a = e.on_screen(c.center - x);
    let b = e.on_screen(c.center + x);
    assert!(a.distance(b) < 6., "{}", a.distance(b));
    e.set_connector_size(0.6);
    e.click(a);
    e.click(b);
    assert_eq!(e.cut().connectors.len(), 2, "{}", e.app.status);
    assert_eq!(e.app.viewport.camera, camera, "no focus jump");
}

fn dowels(e: &mut Editor) {
    let c = e.app.cut.as_mut().unwrap();
    let (_, s) = c.connector;
    c.set_connector(
        Attributes {
            kind: ConnectorType::Dowel,
            ..Attributes::default()
        },
        s,
    );
}

/// Cut after "Add connectors" with nothing placed asks in the card instead of silently
/// cutting plain; "Place automatically" there fills the face, and the Cut that follows
/// makes the halves with holes and the dowels as objects of their own.
#[test]
fn cut_with_none_placed_asks_then_auto_dowels_cut_into_holed_halves_and_pins() {
    let mut e = Editor::with_cut("cube20.stl");
    e.app.cut.as_mut().unwrap().editing = true;
    dowels(&mut e);
    e.frame(vec![]);
    assert_eq!(crate::cut::cut_label(&e.cut().connectors), "Cut");
    e.app.apply_cut();
    assert_eq!(e.cut().prompt, Some(CutPrompt::NoConnectors), "asks first");
    assert_eq!(e.app.plates.current().objects.len(), 1, "nothing cut yet");
    assert!(e.app.status.contains("won't join"), "{}", e.app.status);
    // The card shows the question (and draws without trouble).
    e.frame(vec![]);
    e.app.auto_place_connectors(None);
    let n = e.cut().connectors.len();
    assert!(
        (3..=4).contains(&n),
        "a 20 mm square takes 3 or 4: {}",
        e.app.status
    );
    assert!(e.cut().connectors.iter().all(|c| c.auto));
    assert_eq!(e.cut().prompt, None);
    assert_eq!(e.app.status, format!("Placed {n} dowels"));
    let o = e.app.plates.current().objects[0].clone();
    e.app.cut.as_mut().unwrap().check(&o);
    assert_eq!(e.cut().conflict(), None);
    assert_eq!(
        crate::cut::cut_label(&e.cut().connectors),
        format!("Cut with {n} dowels")
    );
    e.frame(vec![]);
    e.app.apply_cut();
    assert!(e.app.cut.is_none(), "{}", e.app.status);
    let objects = &e.app.plates.current().objects;
    assert_eq!(objects.len(), 2 + n, "two halves and {n} dowels");
    for half in &objects[..2] {
        assert_eq!(half.volumes.len(), n, "{}", half.name);
        assert!(
            half.volumes
                .iter()
                .all(|v| v.kind == PartKind::NegativeVolume)
        );
    }
    assert!(objects[2..].iter().all(|d| d.name.contains("(dowel ")));
}

/// "Place automatically" again asks before replacing its own connectors; hand-placed ones
/// always stay. "Cut anyway" cuts plain.
#[test]
fn place_automatically_again_asks_and_cut_anyway_cuts_plain() {
    let mut e = Editor::with_cut("cube20.stl");
    e.app.cut.as_mut().unwrap().editing = true;
    e.frame(vec![]);
    let o = e.app.plates.current().objects[0].clone();
    // One by hand in a corner first.
    let c = e.app.cut.as_mut().unwrap();
    let (x, y) = c.axes();
    let corner = c.center + x * 6. + y * 6.;
    c.edit_connectors(crate::cut::ConnectorEdit::Click(corner));
    assert!(matches!(
        c.place_automatically(&o, None),
        AutoPlaced::Done { added, .. } if added > 0
    ));
    let first = c.connectors.clone();
    assert!(!first[0].auto && first[1..].iter().all(|c| c.auto));
    assert_eq!(c.place_automatically(&o, None), AutoPlaced::Ask);
    assert_eq!(c.prompt, Some(CutPrompt::ReplaceAuto));
    assert_eq!(c.connectors, first, "asking changes nothing");
    e.frame(vec![]);
    e.app.auto_place_connectors(Some(true));
    let c = e.cut();
    assert_eq!(c.prompt, None);
    assert_eq!(
        c.connectors, first,
        "replaced by the same spots, the hand-placed one kept"
    );
    // "Add more" keeps them all and fills the gaps between them, still conflict-free.
    e.app.auto_place_connectors(Some(false));
    let c = e.app.cut.as_mut().unwrap();
    assert!(c.connectors.len() > first.len(), "{}", e.app.status);
    assert_eq!(c.connectors[..first.len()], first[..]);
    c.check(&o);
    assert_eq!(c.conflict(), None);
    assert_eq!(c.note.as_ref().map(|n| n.1), Some(false));
    // Cleared, Cut asks; "Cut anyway" cuts plain.
    e.app
        .cut
        .as_mut()
        .unwrap()
        .edit_connectors(crate::cut::ConnectorEdit::Clear);
    e.app.apply_cut();
    assert_eq!(e.cut().prompt, Some(CutPrompt::NoConnectors));
    e.frame(vec![]);
    e.app.cut_anyway();
    assert!(e.app.cut.is_none());
    assert_eq!(e.app.plates.current().objects.len(), 2);
    assert!(
        e.app
            .plates
            .current()
            .objects
            .iter()
            .all(|o| o.volumes.is_empty())
    );
}

/// A face too small for the connector places nothing and says so.
#[test]
fn place_automatically_on_a_small_face_says_so() {
    let mut e = Editor::with_cut("cube20.stl");
    let c = e.app.cut.as_mut().unwrap();
    let (a, mut s) = c.connector;
    s.radius = 9.;
    c.set_connector(a, s);
    e.app.auto_place_connectors(None);
    let c = e.cut();
    assert!(c.connectors.is_empty());
    let (note, warn) = c.note.clone().unwrap();
    assert!(warn, "{note}");
    assert_eq!(
        note,
        "No room for a 18.0 mm plug: the cut face needs 21.0 mm across. Try a smaller size"
    );
    assert_eq!(e.app.status, note);
}

#[test]
fn cut_button_names_the_connectors() {
    use crate::cut::{Connector, cut_label};
    let make = |kind| Connector {
        at: glam::Vec2::ZERO,
        attributes: Attributes {
            kind,
            ..Attributes::default()
        },
        sizes: Default::default(),
        auto: false,
    };
    assert_eq!(cut_label(&[]), "Cut");
    assert_eq!(cut_label(&[make(ConnectorType::Plug)]), "Cut with 1 plug");
    assert_eq!(
        cut_label(&[make(ConnectorType::Plug), make(ConnectorType::Plug)]),
        "Cut with 2 plugs"
    );
    assert_eq!(
        cut_label(&vec![make(ConnectorType::Dowel); 3]),
        "Cut with 3 dowels"
    );
    assert_eq!(
        cut_label(&vec![make(ConnectorType::Snap); 2]),
        "Cut with 2 snaps"
    );
    assert_eq!(
        cut_label(&[
            make(ConnectorType::Plug),
            make(ConnectorType::Dowel),
            make(ConnectorType::Snap),
            make(ConnectorType::Dowel)
        ]),
        "Cut with 4 connectors"
    );
}
