// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The Preview sidebar driven headless through whole-window frames: the legend, totals and
//! layer in the sidebar's Preview page (nothing of it over the 3D view), the rail with the
//! Preview item on top (Prepare's without it), a settings topic opened and edited in Preview
//! (the slice goes out of date, the window stays in Preview), entering Preview reopening the
//! Preview page, the collapsed sidebar's chip, and the plate list's project summary.

use crate::app::{App, Options};
use eframe::egui::{self, Pos2, Rect};
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
    /// Every text drawn in the last frame, with its position.
    texts: Vec<(String, Pos2)>,
    /// What the last frames copied to the clipboard.
    copied: Option<String>,
}

fn collect(shape: &egui::Shape, out: &mut Vec<(String, Pos2)>) {
    match shape {
        egui::Shape::Text(t) => out.push((t.galley.text().to_string(), t.pos)),
        egui::Shape::Vec(v) => v.iter().for_each(|s| collect(s, out)),
        _ => {}
    }
}

impl Window {
    /// The CORE One with cube20 loaded, sliced and shown in Preview.
    fn sliced() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "mica-preview-side-{}-{}",
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
        let mut w = Self {
            ctx,
            app,
            time: 0.,
            texts: Vec::new(),
            copied: None,
        };
        w.slice();
        w.app.preview = true;
        w.frames(3);
        w
    }

    fn slice(&mut self) {
        let ctx = self.ctx.clone();
        self.app.slice_current(&ctx);
        let deadline = Instant::now() + Duration::from_secs(300);
        while self.app.busy() && Instant::now() < deadline {
            self.app.poll(&ctx);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(self.app.finished().is_some(), "{}", self.app.status);
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
        for c in &output.platform_output.commands {
            if let egui::OutputCommand::CopyText(text) = c {
                self.copied = Some(text.clone());
            }
        }
        self.texts.clear();
        for s in &output.shapes {
            collect(&s.shape, &mut self.texts);
        }
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

    fn rail(&self) -> Vec<String> {
        self.app
            .editor
            .sidebar_probe
            .rail
            .iter()
            .map(|(t, _)| t.clone())
            .collect()
    }

    fn click_topic(&mut self, topic: &str) {
        let rect = self
            .app
            .editor
            .sidebar_probe
            .rail
            .iter()
            .find(|(t, _)| t == topic)
            .map(|(_, r)| *r)
            .unwrap_or_else(|| panic!("no rail button {topic}: {:?}", self.rail()));
        self.click(rect.center());
    }

    /// Where a text was drawn, if it was.
    fn text_at(&self, text: &str) -> Option<Pos2> {
        self.texts.iter().find(|(t, _)| t == text).map(|(_, p)| *p)
    }
}

#[test]
fn the_legend_totals_and_layer_are_in_the_sidebar_not_over_the_3d_view() {
    let mut w = Window::sliced();
    let probe = w.app.preview_side.probe.clone();
    let sidebar = probe.sidebar.expect("the Preview has the sidebar");
    let view = w.app.viewport_rect;
    assert!(probe.page, "Preview opens on the Preview page");
    assert_eq!(w.app.editor.sidebar_probe.topic, "Preview");
    // Same panel as Prepare: on the right by default, with the rail's Preview item on top.
    assert!(sidebar.left() >= view.right() - 1., "{sidebar:?} {view:?}");
    assert_eq!(w.rail().first().map(String::as_str), Some("Preview"));
    assert!(w.rail().iter().any(|t| t == "Quality"));
    assert_eq!(probe.color_by, "Feature");
    // The legend's texts are drawn inside the sidebar, none over the 3D view.
    for text in [
        "Color by",
        "Outer wall",
        "Click a row to hide it.",
        "Totals",
        "Layer",
    ] {
        let at = w
            .text_at(text)
            .unwrap_or_else(|| panic!("{text} is not drawn"));
        assert!(
            sidebar.contains(at),
            "{text} at {at:?}, sidebar {sidebar:?}"
        );
        assert!(!view.contains(at), "{text} over the 3D view at {at:?}");
    }
    // Totals: the slice's time and filament; Layer: the top layer of the slider.
    let label = |rows: &[(String, String)], l: &str| {
        rows.iter()
            .find(|(k, _)| k == l)
            .map(|(_, v)| v.clone())
            .unwrap_or_else(|| panic!("no {l} row in {rows:?}"))
    };
    let f = w.app.finished().unwrap();
    assert_eq!(
        label(&probe.totals, "Print time"),
        crate::preview::stats::format_time(f.preview.total_time())
    );
    assert!(label(&probe.totals, "Filament").ends_with(" m"));
    let n = f.preview.layers.len();
    assert_eq!(label(&probe.layer, "Layer"), format!("{n} of {n}"));
    assert_eq!(label(&probe.layer, "Z height"), "20.00 mm");
    // Following the layer slider.
    w.app.view.high = 9;
    w.frames(2);
    let probe = w.app.preview_side.probe.clone();
    assert_eq!(label(&probe.layer, "Layer"), format!("10 of {n}"));
    // A scheme with one value over the whole print says so instead of a range.
    w.app.view.color = crate::preview::ColorMode::Width;
    w.frames(2);
    let f = w.app.finished().unwrap();
    let single = crate::preview::legend::single_value(w.app.view.color, &f.preview);
    assert_eq!(
        w.app.preview_side.probe.single,
        single.map(|v| crate::preview::legend::single_text(w.app.view.color, v))
    );
    // Prepare's rail has no Preview item.
    w.app.preview = false;
    w.frames(2);
    assert!(w.app.preview_side.probe.sidebar.is_none());
    assert_ne!(w.rail().first().map(String::as_str), Some("Preview"));
}

#[test]
fn a_settings_topic_opens_and_edits_in_preview_and_entering_preview_shows_the_page() {
    let mut w = Window::sliced();
    let prepare_topic = w.app.prefs.settings_topic.clone();
    w.click_topic("Quality");
    assert_eq!(w.app.preview_side.topic.as_deref(), Some("Quality"));
    assert_eq!(w.app.editor.sidebar_probe.topic, "Quality");
    assert!(!w.app.preview_side.probe.page);
    assert!(
        w.app
            .editor
            .sidebar_probe
            .rows
            .iter()
            .any(|r| r.key == "layer_height"),
        "the topic's settings are listed in the Preview sidebar"
    );
    assert!(w.app.previewing());
    // Prepare keeps its own topic.
    assert_eq!(w.app.prefs.settings_topic, prepare_topic);
    // Edit a setting in Preview: the "Only one wall on first layer" switch, the first row of
    // Walls and surfaces. The slice is out of date; the window stays in Preview.
    let header = w
        .app
        .editor
        .sidebar_probe
        .headers
        .iter()
        .find(|h| h.id == "Quality/Walls and surfaces")
        .map(|h| h.rect)
        .expect("Walls and surfaces section");
    let sidebar = w.app.preview_side.probe.sidebar.unwrap();
    let generation = w.app.config_generation;
    w.click(Pos2::new(
        sidebar.right() - crate::panels::widgets::MARGIN - crate::panels::widgets::CONTROL_W + 17.,
        header.bottom() + crate::panels::widgets::ROW_H / 2. - 2.,
    ));
    assert_ne!(w.app.config_generation, generation, "the switch was edited");
    assert!(w.app.preview && w.app.previewing(), "still in Preview");
    assert_eq!(w.app.preview_side.topic.as_deref(), Some("Quality"));
    // Back on the Preview page: the slice is out of date, with Slice now.
    w.click_topic("Preview");
    if w.app.finished().is_none() {
        assert!(w.app.preview_side.probe.outdated);
        assert!(w.text_at("Slice now").is_some());
    }
    // Leaving and entering Preview opens the Preview page again.
    w.slice();
    w.app.preview = true;
    w.frames(2);
    w.click_topic("Speed");
    assert_eq!(w.app.preview_side.topic.as_deref(), Some("Speed"));
    w.app.preview = false;
    w.frames(2);
    w.app.preview = true;
    w.frames(2);
    assert_eq!(w.app.preview_side.topic, None);
    assert!(w.app.preview_side.probe.page);
}

#[test]
fn a_collapsed_sidebar_leaves_a_chip_that_brings_it_back() {
    let mut w = Window::sliced();
    w.app.keys.sidebar_hidden = true;
    w.frames(2);
    let chip = w.app.preview_side.probe.chip.expect("the legend chip");
    assert!(w.app.viewport_rect.contains(chip.center()));
    assert!(w.text_at("Color by Feature").is_some());
    assert!(
        w.text_at("Outer wall").is_none(),
        "no legend rows while collapsed"
    );
    w.click(chip.center());
    assert!(!w.app.keys.sidebar_hidden);
    assert!(w.app.preview_side.probe.chip.is_none());
    assert!(w.app.preview_side.probe.sidebar.is_some());
}

#[test]
fn preview_lists_the_plates_and_switches_between_them() {
    let mut w = Window::sliced();
    // One plate: no list.
    assert!(w.app.viewport.preview_plates_probe.list.is_none());
    // A second plate, not sliced; back on the sliced first one in Preview.
    w.app
        .plate_command(0, crate::plate_menu::PlateCommand::Duplicate);
    w.app.select_plate(0);
    w.app.preview = true;
    w.frames(3);
    let probe = w.app.viewport.preview_plates_probe.clone();
    let list = probe.list.expect("the plate list in Preview");
    let view = w.app.viewport_rect;
    assert!(
        view.contains(list.min) && view.contains(list.max),
        "{list:?} {view:?}"
    );
    assert!(list.right() < view.center().x, "at the left edge: {list:?}");
    assert_eq!(probe.rows.iter().map(|r| r.0).collect::<Vec<_>>(), [0, 1]);
    assert!(w.text_at("1 of 2 sliced").is_some());
    assert!(w.text_at("not sliced").is_some());
    // The list replaces the plate bar.
    assert!(w.app.viewport.plate_probe.list.is_none());
    // A click on the unsliced plate shows it and stays in Preview, offering its slice.
    w.click(probe.rows[1].1.center());
    assert_eq!(w.app.plates.current, 1);
    assert!(w.app.previewing(), "still in Preview");
    assert!(w.app.preview_side.probe.outdated);
    assert!(w.text_at("Slice this plate").is_some());
    assert!(w.app.viewport.preview_plates_probe.list.is_some());
    // Back to the sliced plate: its preview.
    let row = w.app.viewport.preview_plates_probe.rows[0].1;
    w.click(row.center());
    assert_eq!(w.app.plates.current, 0);
    assert!(w.app.previewing() && w.app.finished().is_some());
    assert!(!w.app.preview_side.probe.outdated);
    // Sidebar collapsed on the left: the list sits below the legend chip.
    w.app.prefs.sidebar_right = false;
    w.app.keys.sidebar_hidden = true;
    w.frames(3);
    let chip = w.app.preview_side.probe.chip.expect("the legend chip");
    let list = w.app.viewport.preview_plates_probe.list.unwrap();
    assert!(list.top() >= chip.bottom(), "{list:?} under {chip:?}");
    // Prepare: no list, the plate bar is back.
    w.app.preview = false;
    w.frames(2);
    assert!(w.app.viewport.preview_plates_probe.list.is_none());
    assert!(w.app.viewport.plate_probe.list.is_some());
}

#[test]
fn the_plate_list_totals_open_the_project_summary() {
    use crate::plate_batch::PlateStatus;
    use crate::viewport::summary_panel;
    let mut w = Window::sliced();
    // Three plates, only the first sliced.
    for _ in 0..2 {
        w.app
            .plate_command(0, crate::plate_menu::PlateCommand::Duplicate);
    }
    w.app.select_plate(0);
    w.app.preview = true;
    w.frames(3);
    let one = w.app.finished().unwrap().summary.clone();
    assert!(w.text_at("1 of 3 sliced").is_some());
    let header = w.app.viewport.preview_plates_probe.header.expect("header");
    let expected = format!(
        "{} · {}",
        crate::project_summary::duration(one.time_s),
        crate::project_summary::weight(one.grams)
    );
    assert!(w.text_at(&expected).is_some(), "the total {expected}");
    // Every plate with objects has its picture.
    assert_eq!(w.app.plate_thumbs.list.len(), 3);
    assert!(
        w.app
            .plate_thumbs
            .list
            .iter()
            .all(|t| t.texture().is_some())
    );
    // The header opens the summary beside the list.
    assert!(!summary_panel::is_open(&w.ctx));
    w.click(header.center());
    assert!(summary_panel::is_open(&w.ctx));
    let probe = summary_panel::probe(&w.ctx);
    let card = probe.card.expect("the summary card");
    let list = w.app.viewport.preview_plates_probe.list.unwrap();
    assert!(
        card.left() >= list.right(),
        "beside the list: {card:?} {list:?}"
    );
    assert!(w.text_at("Project summary").is_some());
    assert!(
        w.text_at("Plates 2 and 3 not sliced; the totals leave them out.")
            .is_some()
            || w.texts
                .iter()
                .any(|(t, _)| t.starts_with("Plates 2 and 3 not sliced")),
        "{:?}",
        w.texts
    );
    // Copy summary.
    w.click(probe.copy.unwrap().center());
    let text = w.copied.clone().expect("copied");
    assert!(
        text.contains("1 of 3 plates sliced; plates 2 and 3 not sliced"),
        "{text}"
    );
    assert!(text.contains(&format!(
        "Print time: {}",
        crate::project_summary::duration(one.time_s)
    )));
    assert!(text.contains("Plate 2: not sliced"), "{text}");
    // Slice the other 2: then all three count, three times the one plate.
    let slice_rest = summary_panel::probe(&w.ctx)
        .slice_rest
        .expect("Slice the other 2");
    w.click(slice_rest.center());
    let deadline = Instant::now() + Duration::from_secs(600);
    while (w.app.busy() || w.app.slice_batch.is_some()) && Instant::now() < deadline {
        w.frames(1);
        std::thread::sleep(Duration::from_millis(5));
    }
    w.frames(3);
    assert!(
        w.app
            .plate_statuses()
            .iter()
            .all(|s| matches!(s, PlateStatus::Sliced { .. })),
        "{:?}",
        w.app
            .plate_statuses()
            .iter()
            .map(|s| s.text())
            .collect::<Vec<_>>()
    );
    assert!(w.text_at("all 3 sliced").is_some());
    let mut plates = crate::scene::plates::snapshot(&w.app.plates);
    for p in &mut plates {
        p.status = w.app.plate_status(p.index);
    }
    let s = crate::project_summary::ProjectSummary::of(&plates);
    assert!((s.time_s - 3. * one.time_s).abs() < 1e-6);
    assert!((s.grams - 3. * one.grams).abs() < 1e-6);
    assert_eq!(s.filaments.len(), 1);
    assert!((s.filaments[0].grams - s.grams).abs() < 1e-6);
    // A plate of the timeline shows that plate.
    let probe = summary_panel::probe(&w.ctx);
    assert_eq!(
        probe.segments.iter().map(|(i, _)| *i).collect::<Vec<_>>(),
        [0, 1, 2]
    );
    w.click(probe.segments[2].1.center());
    assert_eq!(w.app.plates.current, 2);
    // A plate changed since its slice leaves the totals.
    w.app.plates.plates[1].objects[0].position.x += 5.;
    w.app.plates.plates[1].invalidate();
    w.frames(2);
    assert!(w.text_at("2 of 3 sliced").is_some());
    // Close.
    let close = summary_panel::probe(&w.ctx).close.unwrap();
    w.click(close.center());
    assert!(!summary_panel::is_open(&w.ctx));
    assert!(summary_panel::probe(&w.ctx).card.is_none());
}
