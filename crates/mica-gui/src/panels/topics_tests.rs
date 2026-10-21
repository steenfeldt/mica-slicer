// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The process card's topic rail, driven headless through whole-window frames: picking a
//! topic, searching across topics, the Changed filter, folding sections and the sticky
//! section header.

use crate::app::{App, Options};
use crate::overlays::Target;
use crate::panels::editor::Edit;
use eframe::egui::{self, Pos2};
use mica_profiles::user::Store;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);
const CORE_ONE: &str = "Prusa CORE One HF0.4 nozzle";

struct Window {
    ctx: egui::Context,
    app: App,
    time: f64,
}

impl Window {
    fn new() -> Self {
        Self::with_printer(CORE_ONE)
    }

    fn with_printer(printer: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "mica-topics-{}-{}",
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
            .find(|(_, _, n)| n == printer)
            .expect("printer preset");
        app.profiles.select_printer(vendor, &name);
        app.settings_changed();
        let mut w = Self { ctx, app, time: 0. };
        w.frames(2);
        w
    }

    fn frame(&mut self, events: Vec<egui::Event>) {
        self.time += 0.05;
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                Pos2::ZERO,
                egui::vec2(1440., 900.),
            )),
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

    fn click_topic(&mut self, topic: &str) {
        let rect = self
            .probe()
            .rail
            .iter()
            .find(|(t, _)| t == topic)
            .map(|(_, r)| *r)
            .unwrap_or_else(|| panic!("no rail button {topic}"));
        self.click(rect.center());
    }

    fn click_header(&mut self, id: &str) {
        let rect = self
            .probe()
            .headers
            .iter()
            .find(|h| h.id == id)
            .map(|h| h.rect)
            .unwrap_or_else(|| panic!("no section header {id}"));
        self.click(rect.center());
    }

    fn probe(&self) -> &super::Probe {
        &self.app.editor.sidebar_probe
    }

    fn keys(&self) -> Vec<&'static str> {
        self.probe().rows.iter().map(|r| r.key).collect()
    }

    fn topics_shown(&self) -> Vec<&'static str> {
        let mut out: Vec<&'static str> = Vec::new();
        for r in &self.probe().rows {
            if !out.contains(&r.topic) {
                out.push(r.topic);
            }
        }
        out
    }

    fn set(&mut self, key: &str, raw: &str) {
        self.app.apply_edit(
            Some(Target::Print),
            Edit::Set {
                key: key.into(),
                raw: raw.into(),
            },
        );
        self.frames(2);
    }

    /// Scrolls the sidebar with the wheel over the settings list.
    fn scroll(&mut self, dy: f32) {
        let over = self.probe().changed_chip.expect("chip").center();
        self.frame(vec![egui::Event::PointerMoved(over)]);
        self.frame(vec![egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0., -dy),
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Wheel,
            modifiers: egui::Modifiers::NONE,
        }]);
        self.frames(30);
    }
}

#[test]
fn the_rail_switches_topics_one_at_a_time() {
    let mut w = Window::new();
    assert_eq!(w.probe().topic, "Essentials");
    assert!(!w.probe().filtering);
    assert!(w.keys().contains(&"layer_height"));
    assert!(w.probe().rail.len() >= 7, "{:?}", w.probe().rail);
    w.click_topic("Quality");
    assert_eq!(w.app.prefs.settings_topic, "Quality");
    assert_eq!(w.probe().topic, "Quality");
    assert_eq!(w.topics_shown(), ["Quality"]);
    assert!(w.keys().contains(&"seam_position"));
    // Section headers carry the topic: one level only.
    assert!(
        w.probe()
            .headers
            .iter()
            .all(|h| h.id.starts_with("Quality/"))
    );
    w.click_topic("Strength");
    assert_eq!(w.topics_shown(), ["Strength"]);
    assert!(!w.keys().contains(&"seam_position"));
    assert!(w.app.prefs_dirty, "the picked topic is remembered");
}

#[test]
fn search_lists_matches_of_every_topic_and_clearing_returns_to_the_topic() {
    let mut w = Window::new();
    w.click_topic("Quality");
    w.app.editor.search = "speed".into();
    w.frames(2);
    assert!(w.probe().filtering);
    let topics = w.topics_shown();
    assert!(topics.contains(&"Speed"), "{topics:?}");
    assert!(
        topics.len() > 1,
        "matches come from several topics: {topics:?}"
    );
    assert!(w.keys().contains(&"perimeter_speed"));
    // Nothing that does not match is listed.
    assert!(!w.keys().contains(&"layer_height"));
    // Grouped by topic: each topic's rows are together, in rail order.
    let rail: Vec<String> = w.probe().rail.iter().map(|(t, _)| t.clone()).collect();
    let order: Vec<usize> = topics
        .iter()
        .map(|t| rail.iter().position(|r| r == t).unwrap())
        .collect();
    assert!(order.windows(2).all(|p| p[0] < p[1]), "{topics:?}");
    w.app.editor.search.clear();
    w.frames(2);
    assert!(!w.probe().filtering);
    assert_eq!(w.topics_shown(), ["Quality"]);
}

#[test]
fn the_changed_filter_shows_only_changed_settings_across_topics() {
    let mut w = Window::new();
    w.click_topic("Quality");
    assert_eq!(w.probe().topic_changed, 0);
    w.set("seam_position", "rear");
    w.set("perimeter_speed", "123");
    assert_eq!(w.probe().topic_changed, 1, "Quality's own count");
    let chip = w.probe().changed_chip.expect("the Changed chip");
    w.click(chip.center());
    assert!(w.app.editor.changed_only);
    assert!(w.probe().filtering);
    let mut keys = w.keys();
    keys.sort_unstable();
    assert_eq!(keys, ["perimeter_speed", "seam_position"]);
    assert_eq!(w.topics_shown(), ["Quality", "Speed"]);
    // Resetting a setting drops it from the filter.
    w.app
        .apply_edit(Some(Target::Print), Edit::Reset("perimeter_speed".into()));
    w.frames(2);
    assert_eq!(w.keys(), ["seam_position"]);
    // Picking a topic ends the filter.
    w.click_topic("Strength");
    assert!(!w.app.editor.changed_only);
    assert_eq!(w.topics_shown(), ["Strength"]);
}

/// The topic header, the Changed chip and the process dropdown count the same changes:
/// flipping supports and their automatic mode in the Support topic is two changed settings
/// everywhere, not three because Essentials' "Supports" row stands for the same keys.
#[test]
fn changed_counts_agree_between_topic_chip_and_process_dropdown() {
    for printer in [CORE_ONE, "Snapmaker U1 (0.4 nozzle)"] {
        let mut w = Window::with_printer(printer);
        w.click_topic("Support");
        // Both switches flipped from the preset's values.
        for key in ["support_material", "support_material_auto"] {
            let on = w.app.effective_config().and_then(|c| c.bool(key)) == Some(true);
            w.set(key, if on { "0" } else { "1" });
        }
        let dropdown = w.app.overlays.print.len();
        assert_eq!(dropdown, 2, "{printer}: the process dropdown's changes");
        assert_eq!(w.probe().topic_changed, 2, "{printer}: Support's header");
        assert_eq!(w.probe().all_changed, 2, "{printer}: the Changed chip");
        let chip = w.probe().changed_chip.expect("the Changed chip");
        w.click(chip.center());
        assert_eq!(
            w.keys().len(),
            2,
            "{printer}: rows under the filter: {:?}",
            w.keys()
        );
        // A change in another topic counts once more everywhere.
        w.set("perimeter_speed", "123");
        assert_eq!(w.app.overlays.print.len(), 3);
        assert_eq!(w.probe().all_changed, 3, "{printer}");
    }
}

#[test]
fn a_folded_section_hides_its_rows_but_search_and_the_filter_reveal_them() {
    let mut w = Window::new();
    w.click_topic("Quality");
    assert!(w.keys().contains(&"seam_position"));
    w.click_header("Quality/Seam");
    assert!(
        w.app
            .prefs
            .collapsed_sections
            .contains(&"Quality/Seam".to_string())
    );
    assert!(!w.keys().contains(&"seam_position"));
    let header = |w: &Window| {
        w.probe()
            .headers
            .iter()
            .find(|h| h.id == "Quality/Seam")
            .cloned()
            .unwrap()
    };
    assert!(header(&w).collapsed);
    assert_eq!(header(&w).changed, 0);
    // A folded section with a changed setting says so on its header.
    w.set("seam_position", "rear");
    assert_eq!(header(&w).changed, 1);
    assert!(!w.keys().contains(&"seam_position"));
    // Folding survives a topic switch.
    w.click_topic("Strength");
    w.click_topic("Quality");
    assert!(!w.keys().contains(&"seam_position"));
    // Search shows the row inside the folded section, and the fold comes back after.
    w.app.editor.search = "seam pos".into();
    w.frames(2);
    assert!(w.keys().contains(&"seam_position"), "{:?}", w.keys());
    assert!(
        w.probe()
            .rows
            .iter()
            .any(|r| r.topic == "Quality" && r.section == "Seam" && r.key == "seam_position")
    );
    w.app.editor.search.clear();
    w.frames(2);
    assert!(!w.keys().contains(&"seam_position"));
    // So does the Changed filter.
    w.app.editor.changed_only = true;
    w.frames(2);
    assert_eq!(w.keys(), ["seam_position"]);
    w.app.editor.changed_only = false;
    w.frames(2);
    assert!(!w.keys().contains(&"seam_position"));
    assert!(
        w.app
            .prefs
            .collapsed_sections
            .contains(&"Quality/Seam".to_string())
    );
    // Unfold.
    w.click_header("Quality/Seam");
    assert!(w.keys().contains(&"seam_position"));
    assert!(w.app.prefs.collapsed_sections.is_empty());
}

#[test]
fn the_sticky_header_sits_at_the_visible_top_of_the_section_scrolled_into() {
    let mut w = Window::new();
    w.app.prefs.advanced = true;
    w.click_topic("Quality");
    assert!(w.probe().sticky.is_none(), "nothing scrolled yet");
    w.scroll(700.);
    let probe = w.probe().clone();
    let (id, rect) = probe
        .sticky
        .clone()
        .expect("a sticky header after scrolling");
    assert!(
        (rect.top() - probe.visible_top).abs() < 0.5,
        "drawn at the scroll area's visible top: {} vs {}",
        rect.top(),
        probe.visible_top
    );
    // It belongs to the section whose own header scrolled away last.
    let real = probe.headers.iter().find(|h| h.id == id).unwrap();
    assert!(real.rect.top() < probe.visible_top);
    let next = probe
        .headers
        .iter()
        .filter(|h| h.rect.top() > real.rect.top())
        .map(|h| h.rect.top())
        .fold(f32::INFINITY, f32::min);
    assert!(next > probe.visible_top + super::SECTION_H, "{id}");
    assert!(
        probe
            .headers
            .iter()
            .filter(|h| h.rect.top() < probe.visible_top)
            .all(|h| h.rect.top() <= real.rect.top()),
        "{id} is the last header above the top"
    );
    // A folded section never sticks: folding it drops its rows from under the top.
    w.click(rect.center());
    assert!(w.app.prefs.collapsed_sections.contains(&id));
    let after = w.probe().sticky.clone().map(|(s, _)| s);
    assert_ne!(after.as_deref(), Some(id.as_str()));
}

#[test]
fn a_narrow_sidebar_keeps_the_rail_as_icons() {
    let mut w = Window::new();
    assert!(!w.probe().compact);
    w.app.prefs.window.sidebar_width = 320.;
    w.ctx.memory_mut(|m| *m = Default::default());
    w.frames(3);
    assert!(
        w.probe().compact,
        "a 320 px sidebar shows the icon-only rail"
    );
    w.click_topic("Support");
    assert_eq!(w.topics_shown(), ["Support"]);
}

/// Every topic of the rail lists settings: simple and "Show all settings", PrusaSlicer and
/// Orca rules, all objects and an object scope. (Speed used to open empty in simple mode:
/// OrcaSlicer files every speed as advanced.)
#[test]
fn no_rail_topic_opens_empty() {
    for printer in [CORE_ONE, "Snapmaker U1 (0.4 nozzle)"] {
        for all in [false, true] {
            for object in [false, true] {
                let mut w = Window::with_printer(printer);
                let model = std::path::PathBuf::from(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/../../tests/models/3DBenchy.stl"
                ));
                let result = crate::app::load_files(vec![model], &w.app.config, w.app.bed.center());
                w.app.finish_load(result);
                w.app.prefs.advanced = all;
                w.frames(3);
                w.app.selected = object.then_some(0);
                w.app.editor.object_mode = object;
                w.frames(3);
                assert_eq!(w.app.scope_env().is_some(), object);
                let rail: Vec<String> = w.probe().rail.iter().map(|(t, _)| t.clone()).collect();
                assert!(rail.len() >= 6, "{rail:?}");
                for t in rail {
                    w.app.prefs.settings_topic = t.clone();
                    w.frames(2);
                    let case = format!("{printer}, show all {all}, object scope {object}, {t}");
                    assert_eq!(w.probe().topic, t, "{case}");
                    assert!(!w.probe().rows.is_empty(), "{case}: no settings");
                    assert!(w.probe().note.is_none(), "{case}: {:?}", w.probe().note);
                }
            }
        }
    }
}

#[test]
fn simple_speed_shows_its_key_options_and_more_behind_the_link() {
    let mut w = Window::new();
    w.app.prefs.settings_topic = "Speed".into();
    w.frames(2);
    let keys = w.keys();
    for k in ["external_perimeter_speed", "infill_speed", "travel_speed"] {
        assert!(keys.contains(&k), "{k} in {keys:?}");
    }
    assert!(!keys.contains(&"default_acceleration"));
    // Texture: switch, image, depth and direction without "Show all settings".
    w.app.prefs.settings_topic = "Texture".into();
    w.frames(2);
    let keys = w.keys();
    for k in crate::settings::TOPIC_KEYS
        .iter()
        .find(|(t, _)| *t == "Texture")
        .unwrap()
        .1
    {
        assert!(keys.contains(k), "{k} in {keys:?}");
    }
}
