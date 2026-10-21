// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The menu bar, described once. [`menus`] lists every menu and item; each item names an
//! [`Action`] (most are a keymap [`Act`], whose first binding is the item's shortcut). The same
//! description is drawn by egui in the header ([`App::menu_bar`], Windows and Linux, and the
//! Mac when the native menu could not be set up) and built into the macOS system menu bar
//! (`native.rs`, through `muda`). [`App::menu_state`] says what is enabled, checked and what an
//! item reads; [`App::run_menu`] carries an item out. Both menus call the same two functions,
//! so they cannot drift.
//!
//! Shortcuts on the Mac: a chord with Cmd that a native item shows is the native menu's (the
//! system delivers it to the menu, never to the window), and egui's keymap skips it
//! ([`native_owns`]); every other key stays with the keymap. While a text field has the
//! keyboard, the Edit items (undo, redo, cut, copy, paste, select all) go to the field.

use crate::app::App;
use crate::keymap::{self, Act, Mods};
use crate::theme;
use eframe::egui::{self, Key};
use mica_core::painting::PaintKind;
use std::sync::OnceLock;

#[cfg(target_os = "macos")]
pub mod native;

/// Off the Mac there is no native menu: the egui menu bar is the only one.
#[cfg(not(target_os = "macos"))]
pub mod native {
    pub struct NativeMenu;
}

/// What a menu item does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// A keyboard shortcut's act (the item shows its shortcut).
    Act(Act),
    /// Export all plates…: one file per plate into a folder.
    ExportAll,
    MyPrinters,
    AddPrinter,
    AutoSlice,
    DiscardProcessChanges,
    /// Dark (true) or light theme.
    Theme(bool),
    /// Sidebar on the right (true) or the left.
    SidebarRight(bool),
    PaintOverlay(PaintKind),
    ShowAllSettings,
    /// A calibration test, by index into `calibration::Test::ALL`.
    Calibrate(usize),
    /// A recent file, by index into `Prefs::recent_files`.
    Recent(usize),
    ClearRecent,
    /// Quit through the normal close path (unsaved-changes prompt).
    Quit,
    /// Ask the update server for a newer Mica (`updater.rs`).
    CheckForUpdates,
    /// The About window (version, links, credits).
    About,
    /// The Welcome window (the first launch shows it by itself).
    Welcome,
    /// This version's release notes (shown once after an update).
    WhatsNew,
}

impl Action {
    /// The act whose first binding is this item's shortcut.
    pub fn act(self) -> Option<Act> {
        match self {
            Action::Act(a) => Some(a),
            _ => None,
        }
    }

    /// The Edit items that act on a focused text field instead of the plate.
    pub fn text_edit(self) -> bool {
        matches!(
            self,
            Action::Act(
                Act::Undo
                    | Act::Redo
                    | Act::CutToClipboard
                    | Act::Copy
                    | Act::Paste
                    | Act::SelectAll
            )
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Button,
    Check,
    Radio,
}

#[derive(Clone, Debug)]
pub struct Item {
    pub label: String,
    pub action: Action,
    pub kind: Kind,
    /// Hover text in the egui menu (none in the native one).
    pub hint: String,
    /// Hover text while disabled.
    pub disabled_hint: &'static str,
}

/// macOS items with no Mica action: AppKit carries them out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Native {
    Services,
    Hide,
    HideOthers,
    ShowAll,
    Minimize,
    Zoom,
    Fullscreen,
    BringAllToFront,
}

impl Native {
    /// The chord AppKit gives the item.
    pub fn chord(self) -> Option<(Key, Mods)> {
        match self {
            Native::Hide => Some((Key::H, Mods::CMD)),
            Native::HideOthers => Some((Key::H, Mods::CMD_ALT)),
            Native::Minimize => Some((Key::M, Mods::CMD)),
            Native::Fullscreen => Some((Key::F, Mods::CMD_CTRL)),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub enum Entry {
    Item(Item),
    Separator,
    /// The "Open recent" submenu: one item per recent file, then "Clear list".
    Recent,
    /// A line of muted text.
    Note(&'static str),
    Native(Native),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// The Mac application menu ("Mica").
    App,
    Normal,
    /// The Mac Window menu (the system adds the window list).
    Window,
    Help,
}

#[derive(Clone, Debug)]
pub struct Menu {
    pub title: &'static str,
    /// Read by the native Mac menu (and the tests); the egui bar draws every menu alike.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub role: Role,
    /// Minimum width of the egui menu.
    pub width: f32,
    pub entries: Vec<Entry>,
}

/// What an item looks like this frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct State {
    pub enabled: bool,
    pub checked: bool,
    /// Replaces the label ("Undo Move").
    pub label: Option<String>,
}

fn item(label: &str, action: Action) -> Entry {
    Entry::Item(Item {
        label: label.into(),
        action,
        kind: Kind::Button,
        hint: String::new(),
        disabled_hint: "",
    })
}
fn hinted(label: &str, action: Action, hint: &str) -> Entry {
    match item(label, action) {
        Entry::Item(mut i) => {
            i.hint = hint.into();
            Entry::Item(i)
        }
        e => e,
    }
}
fn check(label: &str, action: Action, hint: &str) -> Entry {
    Entry::Item(Item {
        label: label.into(),
        action,
        kind: Kind::Check,
        hint: hint.into(),
        disabled_hint: "",
    })
}
fn radio(label: &str, action: Action) -> Entry {
    Entry::Item(Item {
        label: label.into(),
        action,
        kind: Kind::Radio,
        hint: String::new(),
        disabled_hint: "",
    })
}

const AUTO_SLICE_HINT: &str = "Off: press Slice (⌘R) or open Preview when you are ready";

/// Every menu, in menu-bar order. `mac`: the macOS layout (application and Window menus,
/// Preferences as "Settings…" in the application menu).
pub fn menus(mac: bool) -> Vec<Menu> {
    use Action as A;
    use Entry::Separator as Sep;
    let mut out = vec![];
    if mac {
        out.push(Menu {
            title: "Mica",
            role: Role::App,
            width: 200.,
            entries: vec![
                item("About Mica Slicer", A::About),
                item("Check for Updates…", A::CheckForUpdates),
                Sep,
                item("Settings…", A::Act(Act::Preferences)),
                Sep,
                Entry::Native(Native::Services),
                Sep,
                Entry::Native(Native::Hide),
                Entry::Native(Native::HideOthers),
                Entry::Native(Native::ShowAll),
                Sep,
                item("Quit Mica", A::Quit),
            ],
        });
    }
    let mut file = vec![
        hinted(
            "New project",
            A::Act(Act::NewProject),
            "Start over with an empty plate and the printer's default profiles",
        ),
        item("Open…", A::Act(Act::Open)),
        Entry::Recent,
        Sep,
        hinted(
            "Save",
            A::Act(Act::Save),
            "Save the project (every plate, objects, painting and settings)",
        ),
        hinted(
            "Save As…",
            A::Act(Act::SaveAs),
            "Save as an OrcaSlicer / Snapmaker or a PrusaSlicer project",
        ),
        Sep,
        item("Slice plate", A::Act(Act::Slice)),
        item("Slice all plates", A::Act(Act::SliceAll)),
        item("Export G-code…", A::Act(Act::ExportGcode)),
        hinted(
            "Export all plates…",
            A::ExportAll,
            "One G-code file per plate into a folder; plates not sliced yet are sliced first",
        ),
        item("Print…", A::Act(Act::Print)),
        item("Add printer…", A::AddPrinter),
        item("My printers…", A::MyPrinters),
        Sep,
        check(
            "Slice automatically after changes",
            A::AutoSlice,
            AUTO_SLICE_HINT,
        ),
    ];
    if !mac {
        file.push(item("Settings…", A::Act(Act::Preferences)));
    }
    out.push(Menu {
        title: "File",
        role: Role::Normal,
        width: 260.,
        entries: file,
    });
    out.push(Menu {
        title: "Edit",
        role: Role::Normal,
        width: 240.,
        entries: vec![
            item("Undo", A::Act(Act::Undo)),
            item("Redo", A::Act(Act::Redo)),
            Sep,
            item("Cut", A::Act(Act::CutToClipboard)),
            item("Copy", A::Act(Act::Copy)),
            item("Paste", A::Act(Act::Paste)),
            item("Duplicate", A::Act(Act::Clone)),
            item("Select all", A::Act(Act::SelectAll)),
            Sep,
            item("Arrange plate", A::Act(Act::ArrangePlate)),
            item("Arrange all plates", A::Act(Act::ArrangeAll)),
            item("Delete", A::Act(Act::Delete)),
            item("Delete all", A::Act(Act::DeleteAll)),
            Sep,
            item("Discard process changes", A::DiscardProcessChanges),
        ],
    });
    let mut view = vec![
        radio("Dark theme", A::Theme(true)),
        radio("Light theme", A::Theme(false)),
        Sep,
        radio("Sidebar on the left", A::SidebarRight(false)),
        radio("Sidebar on the right", A::SidebarRight(true)),
        check("Hide the sidebar", A::Act(Act::CollapseSidebar), ""),
        Sep,
        check("Perspective camera", A::Act(Act::TogglePerspective), ""),
    ];
    for (n, name) in [
        "Isometric",
        "Top",
        "Bottom",
        "Front",
        "Rear",
        "Left",
        "Right",
    ]
    .into_iter()
    .enumerate()
    {
        view.push(item(&format!("View: {name}"), A::Act(Act::Camera(n as u8))));
    }
    view.push(item("Zoom to plate", A::Act(Act::ZoomFit { all: false })));
    view.push(Sep);
    const PAINT_HINT: &str = "Off: shown while its painting tool is active (support and fuzzy skin also on the selected object)";
    for (kind, label) in [
        (PaintKind::Supports, "Always show support paint"),
        (PaintKind::Seam, "Always show seam paint"),
        (PaintKind::Fuzzy, "Always show fuzzy skin paint"),
    ] {
        view.push(check(label, A::PaintOverlay(kind), PAINT_HINT));
    }
    view.push(Sep);
    view.push(check("Show all settings", A::ShowAllSettings, ""));
    view.push(check(
        "Slice automatically after changes",
        A::AutoSlice,
        AUTO_SLICE_HINT,
    ));
    out.push(Menu {
        title: "View",
        role: Role::Normal,
        width: 240.,
        entries: view,
    });
    let mut calibrate = vec![];
    for (i, test) in crate::calibration::Test::ALL.into_iter().enumerate() {
        if matches!(i, 2 | 5 | 6 | 7) {
            calibrate.push(Sep);
        }
        let first_sentence = test
            .explanation()
            .split_once(". ")
            .map_or(test.explanation(), |(s, _)| s);
        calibrate.push(Entry::Item(Item {
            label: test.title().into(),
            action: A::Calibrate(i),
            kind: Kind::Button,
            hint: format!("{}.", first_sentence.trim_end_matches('.')),
            disabled_hint: "Calibration prints need a Prusa Buddy or Klipper printer",
        }));
    }
    calibrate.push(Sep);
    calibrate.push(Entry::Note(
        "Order for a new filament: temperature, flow (pass 1, pass 2), pressure advance, retraction.",
    ));
    out.push(Menu {
        title: "Calibrate",
        role: Role::Normal,
        width: 250.,
        entries: calibrate,
    });
    if mac {
        out.push(Menu {
            title: "Window",
            role: Role::Window,
            width: 200.,
            entries: vec![
                Entry::Native(Native::Minimize),
                Entry::Native(Native::Zoom),
                Entry::Native(Native::Fullscreen),
                Sep,
                Entry::Native(Native::BringAllToFront),
            ],
        });
    }
    out.push(Menu {
        title: "Help",
        role: Role::Help,
        width: 240.,
        entries: if mac {
            vec![
                item("Keyboard shortcuts", A::Act(Act::ShowShortcuts)),
                Sep,
                item("Welcome to Mica…", A::Welcome),
                item("What's new…", A::WhatsNew),
            ]
        } else {
            // On the Mac it sits in the application menu, under About.
            vec![
                item("Keyboard shortcuts", A::Act(Act::ShowShortcuts)),
                Sep,
                item("Welcome to Mica…", A::Welcome),
                item("What's new…", A::WhatsNew),
                Sep,
                item("Check for Updates…", A::CheckForUpdates),
                item("About Mica Slicer", A::About),
            ]
        },
    });
    out
}

/// The menus of this platform's egui bar (the non-Mac layout, also the Mac's fallback).
fn egui_menus() -> &'static [Menu] {
    static M: OnceLock<Vec<Menu>> = OnceLock::new();
    M.get_or_init(|| menus(false))
}

/// The binding a menu item shows: its act's first binding on this platform.
pub fn shortcut(action: Action) -> Option<&'static keymap::Binding> {
    action.act().and_then(keymap::first_binding)
}

/// The chord a native item takes over: shortcuts with Cmd only. A bare key (A, Delete, Tab,
/// `?`) as a menu key equivalent would be taken from text fields too, so those stay egui's.
pub fn native_chord(action: Action) -> Option<(Key, Mods)> {
    if action == Action::Quit {
        return Some((Key::Q, Mods::CMD));
    }
    shortcut(action)
        .filter(|b| b.mods.has(Mods::CMD))
        .map(|b| (b.key, b.mods))
}

/// Every chord the native (Mac) menu bar owns.
pub fn native_chords() -> &'static [(Key, Mods)] {
    static C: OnceLock<Vec<(Key, Mods)>> = OnceLock::new();
    C.get_or_init(|| {
        let mut out = vec![];
        for m in menus(true) {
            for e in &m.entries {
                let chord = match e {
                    Entry::Item(i) => native_chord(i.action),
                    Entry::Native(n) => n.chord(),
                    _ => None,
                };
                out.extend(chord);
            }
        }
        out
    })
}

/// With the native menu up, whether a key chord belongs to it (egui's keymap skips it).
pub fn native_owns(key: Key, mods: Mods) -> bool {
    native_chords().contains(&(key, mods))
}

impl App {
    /// What a menu item looks like now. `text_focus`: a text field has the keyboard (the Edit
    /// items then act on the field and are always enabled).
    pub(crate) fn menu_state(&self, action: Action, text_focus: bool) -> State {
        let busy = self.busy();
        let has_objects = !self.plates.current().objects.is_empty();
        let has_selection = self.selected.is_some();
        let doc = self.viewing_gcode();
        let can_slice = !busy && self.config.is_ok() && has_objects && !doc;
        // Slice all may start while a slice runs: it waits for it.
        let can_slice_all = !doc
            && self.config.is_ok()
            && self.loading.is_none()
            && (0..self.plates.len()).any(|i| self.sliceable(i));
        let sliced = !busy && self.finished().is_some();
        let on = |enabled: bool| State {
            enabled,
            checked: false,
            label: None,
        };
        let checked = |checked: bool| State {
            enabled: true,
            checked,
            label: None,
        };
        if text_focus && action.text_edit() {
            return on(true);
        }
        // An opened G-code file is read-only: nothing edits the plate under it.
        if doc
            && matches!(
                action,
                Action::Act(
                    Act::Undo
                        | Act::Redo
                        | Act::CutToClipboard
                        | Act::Copy
                        | Act::Paste
                        | Act::Clone
                        | Act::SelectAll
                        | Act::ArrangePlate
                        | Act::ArrangeAll
                        | Act::Delete
                        | Act::DeleteAll
                ) | Action::Calibrate(_)
            )
        {
            return on(false);
        }
        match action {
            Action::Act(act) => match act {
                Act::NewProject => on(self.loading.is_none()),
                Act::Open => on(!busy),
                Act::Save | Act::SaveAs => on(!busy && !self.plates.is_blank()),
                Act::Slice => on(can_slice),
                Act::SliceAll => on(can_slice_all),
                Act::ExportGcode if doc => State {
                    enabled: sliced,
                    checked: false,
                    label: Some("Save copy…".into()),
                },
                Act::ExportGcode | Act::Print => on(sliced),
                Act::Undo => State {
                    enabled: self.undo.can_undo(),
                    checked: false,
                    label: self.undo.undo_label().map(|l| format!("Undo {l}")),
                },
                Act::Redo => State {
                    enabled: self.undo.can_redo(),
                    checked: false,
                    label: self.undo.redo_label().map(|l| format!("Redo {l}")),
                },
                Act::CutToClipboard | Act::Copy | Act::Clone | Act::Delete => on(has_selection),
                Act::Paste => on(!self.clipboard.is_empty()),
                Act::SelectAll | Act::ArrangePlate | Act::DeleteAll => on(has_objects),
                Act::ArrangeAll => on(self.plates.len() > 1 || has_objects),
                Act::CollapseSidebar => checked(self.keys.sidebar_hidden),
                Act::TogglePerspective => checked(self.viewport.camera.perspective),
                _ => on(true),
            },
            Action::ExportAll => on(self.can_export_all()),
            Action::DiscardProcessChanges => on(!self.overlays.print.is_empty()),
            Action::AutoSlice => checked(self.prefs.auto_slice),
            Action::Theme(dark) => checked((self.viewport.theme_mode == theme::Mode::Dark) == dark),
            Action::SidebarRight(right) => checked(self.prefs.sidebar_right == right),
            Action::PaintOverlay(kind) => checked(self.show_paint.strength(kind) > 0.),
            Action::ShowAllSettings => checked(self.prefs.advanced),
            Action::Calibrate(_) => on(self.calibration_supported()),
            Action::Recent(_) => on(!busy),
            Action::MyPrinters
            | Action::AddPrinter
            | Action::ClearRecent
            | Action::Quit
            | Action::About
            | Action::Welcome
            | Action::WhatsNew => on(true),
            Action::CheckForUpdates => on(!self.updater.working()),
        }
    }

    /// Calibration prints need Marlin 2 (Prusa Buddy) or Klipper G-code.
    fn calibration_supported(&self) -> bool {
        self.effective_config()
            .is_some_and(|c| matches!(c.string("gcode_flavor"), Some("marlin2" | "klipper")))
    }

    /// Carries out a menu item, from either menu bar.
    pub(crate) fn run_menu(&mut self, action: Action, ctx: &egui::Context) {
        use crate::commands::Command;
        let state = self.menu_state(action, false);
        if !state.enabled {
            return;
        }
        match action {
            Action::Act(act) => match act {
                Act::NewProject => self.new_project(),
                Act::Open => self.open_dialog(ctx),
                Act::Save => self.save(),
                Act::SaveAs => self.save_as(None),
                Act::Slice => self.slice_current(ctx),
                Act::SliceAll => self.slice_all(ctx),
                Act::ExportGcode => self.export(ctx),
                Act::Print => self.open_print_dialog(),
                Act::Preferences => self.open_preferences(None),
                Act::Undo => self.undo(),
                Act::Redo => self.redo(),
                Act::CutToClipboard => {
                    self.copy_selection();
                    self.delete_selected();
                }
                Act::Copy => self.run_command(Command::Copy, ctx),
                Act::Paste => self.run_command(Command::Paste, ctx),
                Act::Clone => self.run_command(Command::Duplicate, ctx),
                Act::SelectAll => self.run_command(Command::SelectAll, ctx),
                Act::ArrangePlate => self.arrange(),
                Act::ArrangeAll => self.arrange_all(),
                Act::Delete => self.delete_selected(),
                Act::DeleteAll => self.clear_plate(),
                Act::CollapseSidebar => self.keys.sidebar_hidden = !self.keys.sidebar_hidden,
                Act::TogglePerspective => {
                    self.viewport.camera.perspective = !self.viewport.camera.perspective;
                }
                Act::Camera(n) => self.viewport.camera.preset(n as usize),
                Act::ZoomFit { .. } => self.fit_current_plate(false),
                Act::ShowShortcuts => {
                    self.keys.dialog = true;
                    self.keys.focus_dialog_search = true;
                }
                // Not in a menu; the keymap's own path.
                other => {
                    if let Some(view) = self.run_key(other, ctx) {
                        self.viewport.keys.push(view);
                    }
                }
            },
            Action::ExportAll => self.export_all_dialog(ctx),
            Action::MyPrinters => self.dialogs.my_printers = true,
            Action::AddPrinter => self.open_add_printer(),
            Action::AutoSlice => {
                self.prefs.auto_slice = !self.prefs.auto_slice;
                self.prefs_dirty = true;
                if self.prefs.auto_slice {
                    self.dirty_since = Some(std::time::Instant::now());
                }
            }
            Action::DiscardProcessChanges => {
                self.record("Discard changes");
                self.overlays.print.clear();
                self.settings_changed();
            }
            Action::Theme(dark) => {
                let mode = if dark {
                    theme::Mode::Dark
                } else {
                    theme::Mode::Light
                };
                if self.viewport.theme_mode != mode {
                    self.set_theme(ctx, mode);
                }
            }
            Action::SidebarRight(right) => {
                if self.prefs.sidebar_right != right {
                    self.prefs.sidebar_right = right;
                    self.prefs_dirty = true;
                }
            }
            Action::PaintOverlay(kind) => {
                let on = self.show_paint.strength(kind) > 0.;
                self.show_paint.set(kind, if on { 0. } else { 1. });
            }
            Action::ShowAllSettings => {
                self.prefs.advanced = !self.prefs.advanced;
                self.prefs_dirty = true;
            }
            Action::Calibrate(i) => {
                if let Some(test) = crate::calibration::Test::ALL.get(i) {
                    self.open_calibration(*test);
                }
            }
            Action::Recent(i) => {
                if let Some(path) = self.prefs.recent_files.get(i).cloned() {
                    self.load(vec![path], ctx);
                }
            }
            Action::ClearRecent => {
                self.prefs.recent_files.clear();
                self.prefs_dirty = true;
            }
            Action::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            Action::CheckForUpdates => self.check_for_updates(ctx),
            Action::About => self.open_about(crate::panels::about::AboutTab::About),
            Action::Welcome => self.open_welcome(),
            Action::WhatsNew => self.open_whats_new(),
        }
    }

    /// The egui menu bar (File, Edit, View, Calibrate, Help) for the header.
    pub(crate) fn menu_bar(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let mut chosen = None;
        for menu in egui_menus() {
            ui.menu_button(menu.title, |ui| {
                ui.set_min_width(menu.width);
                for entry in &menu.entries {
                    if let Some(a) = self.menu_entry(ui, entry) {
                        chosen = Some(a);
                    }
                }
            });
        }
        if let Some(action) = chosen {
            self.run_menu(action, &ctx);
        }
    }

    /// One entry of an egui menu; returns the action when it was chosen.
    fn menu_entry(&self, ui: &mut egui::Ui, entry: &Entry) -> Option<Action> {
        match entry {
            Entry::Separator => {
                ui.separator();
                None
            }
            Entry::Note(text) => {
                ui.label(
                    egui::RichText::new(*text)
                        .font(theme::sans(11.))
                        .color(theme::of(ui).muted),
                );
                None
            }
            Entry::Native(_) => None,
            Entry::Recent => {
                let mut out = None;
                ui.menu_button("Open recent", |ui| {
                    ui.set_min_width(320.);
                    if self.prefs.recent_files.is_empty() {
                        ui.weak("No recent files");
                        return;
                    }
                    let enabled = self.menu_state(Action::Recent(0), false).enabled;
                    for (i, path) in self.prefs.recent_files.iter().enumerate() {
                        if ui
                            .add_enabled(enabled, egui::Button::new(recent_label(path)))
                            .on_hover_text(path.display().to_string())
                            .clicked()
                        {
                            ui.close();
                            out = Some(Action::Recent(i));
                        }
                    }
                    ui.separator();
                    if ui.button("Clear list").clicked() {
                        ui.close();
                        out = Some(Action::ClearRecent);
                    }
                });
                out
            }
            Entry::Item(item) => {
                let state = self.menu_state(item.action, false);
                let label = state.label.as_deref().unwrap_or(&item.label);
                let key = shortcut(item.action)
                    .map(keymap::Binding::chord)
                    .unwrap_or_default();
                let with_hints = |r: egui::Response| {
                    let r = if !item.disabled_hint.is_empty() {
                        r.on_disabled_hover_text(item.disabled_hint)
                    } else {
                        r
                    };
                    if !item.hint.is_empty() {
                        r.on_hover_text(&item.hint)
                    } else {
                        r
                    }
                };
                match item.kind {
                    Kind::Button => {
                        let r = ui.add_enabled(
                            state.enabled,
                            egui::Button::new(label).shortcut_text(key),
                        );
                        if with_hints(r).clicked() {
                            ui.close();
                            return Some(item.action);
                        }
                    }
                    Kind::Check => {
                        let mut on = state.checked;
                        let r = ui.add_enabled(state.enabled, egui::Checkbox::new(&mut on, label));
                        let r = if item.hint.is_empty() && !key.is_empty() {
                            r.on_hover_text(format!("{key} toggles"))
                        } else {
                            with_hints(r)
                        };
                        if r.changed() {
                            return Some(item.action);
                        }
                    }
                    Kind::Radio => {
                        let r = ui.add_enabled(
                            state.enabled,
                            egui::RadioButton::new(state.checked, label),
                        );
                        if with_hints(r).clicked() && !state.checked {
                            ui.close();
                            return Some(item.action);
                        }
                    }
                }
                None
            }
        }
    }

    /// Native menu: set it up for the window (macOS); on failure the egui menu stays.
    pub(crate) fn init_native_menu(&mut self, ctx: &egui::Context) {
        #[cfg(target_os = "macos")]
        match native::NativeMenu::new(ctx, &menus(true)) {
            Ok(menu) => self.native_menu = Some(menu),
            Err(e) => {
                eprintln!(
                    "mica-gui: the macOS menu bar could not be set up ({e}); using the in-window menu"
                );
                self.notify_warning(format!(
                    "The macOS menu bar could not be set up ({e}); the menus are in the window instead."
                ));
            }
        }
        #[cfg(not(target_os = "macos"))]
        let _ = ctx;
    }

    /// Native menu, start of a frame: carries out the items chosen since the last frame.
    pub(crate) fn native_menu_events(&mut self, ctx: &egui::Context) {
        #[cfg(target_os = "macos")]
        {
            let Some(menu) = &mut self.native_menu else {
                return;
            };
            let actions = menu.events();
            let typing = keymap::typing(ctx);
            for action in actions {
                if typing && action.text_edit() {
                    native::forward_to_text_field(ctx, action);
                } else {
                    self.run_menu(action, ctx);
                }
            }
        }
        #[cfg(not(target_os = "macos"))]
        let _ = ctx;
    }

    /// Native menu, end of a frame: brings enabled, checked and labels up to date.
    pub(crate) fn native_menu_refresh(&mut self, ctx: &egui::Context) {
        #[cfg(target_os = "macos")]
        {
            let Some(mut menu) = self.native_menu.take() else {
                return;
            };
            let typing = keymap::typing(ctx);
            menu.refresh(|a| self.menu_state(a, typing), &self.prefs.recent_files);
            self.native_menu = Some(menu);
        }
        #[cfg(not(target_os = "macos"))]
        let _ = ctx;
    }

    /// The menus are in the system menu bar, not in the window.
    pub(crate) fn has_native_menu(&self) -> bool {
        self.native_menu.is_some()
    }
}

/// A recent file as the menu shows it: its file name.
pub fn recent_label(path: &std::path::Path) -> String {
    path.file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items(menus: &[Menu], skip_mac_only: bool) -> Vec<(String, Action)> {
        menus
            .iter()
            .filter(|m| !skip_mac_only || !matches!(m.role, Role::App | Role::Window))
            .flat_map(|m| m.entries.iter())
            .filter_map(|e| match e {
                Entry::Item(i) => Some((i.label.clone(), i.action)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn every_item_maps_to_an_action_with_its_shortcut() {
        for mac in [false, true] {
            for m in menus(mac) {
                assert!(!m.entries.is_empty(), "{} is empty", m.title);
                for e in &m.entries {
                    if let Entry::Item(i) = e {
                        assert!(!i.label.is_empty());
                        if let Some(act) = i.action.act() {
                            // A shortcut act in a menu is carried out by `run_menu` (which
                            // handles every act an item uses) and labelled from the keymap.
                            assert!(
                                keymap::BINDINGS.iter().any(|b| b.act == act),
                                "{}: {act:?} has no binding",
                                i.label
                            );
                            assert!(
                                shortcut(i.action).is_some(),
                                "{}: no binding on this platform",
                                i.label
                            );
                        }
                        if let Action::Calibrate(n) = i.action {
                            assert!(n < crate::calibration::Test::ALL.len());
                        }
                    }
                    if let Entry::Native(n) = e {
                        assert!(
                            matches!(m.role, Role::App | Role::Window),
                            "{n:?} outside the Mac menus"
                        );
                    }
                }
            }
        }
        // The Mac layout puts the app's own items in the application menu.
        let mac = menus(true);
        assert_eq!(mac.first().map(|m| m.role), Some(Role::App));
        assert!(items(&mac[..1], false).contains(&("Quit Mica".into(), Action::Quit)));
        assert!(
            !menus(false)
                .iter()
                .any(|m| m.role != Role::Normal && m.role != Role::Help)
        );
    }

    #[test]
    fn same_items_on_every_platform() {
        let other: Vec<Action> = items(&menus(false), false)
            .into_iter()
            .map(|x| x.1)
            .collect();
        let mut mac: Vec<Action> = items(&menus(true), true).into_iter().map(|x| x.1).collect();
        // Preferences moves to the Mac application menu as "Settings…", Check for Updates
        // from Help to under About.
        let app = items(&menus(true)[..1], false);
        assert!(app.contains(&("Settings…".into(), Action::Act(Act::Preferences))));
        assert!(app.contains(&("Check for Updates…".into(), Action::CheckForUpdates)));
        assert!(app.contains(&("About Mica Slicer".into(), Action::About)));
        let moved = [
            Action::About,
            Action::CheckForUpdates,
            Action::Act(Act::Preferences),
        ];
        mac.retain(|a| !moved.contains(a));
        let mut other = other;
        other.retain(|a| !moved.contains(a));
        assert!(items(&menus(false), false).contains(&("About Mica Slicer".into(), Action::About)));
        assert_eq!(other, mac);
        assert!(
            items(&menus(false), false)
                .contains(&("Check for Updates…".into(), Action::CheckForUpdates))
        );
        let titles = |mac| -> Vec<&str> {
            menus(mac)
                .into_iter()
                .filter(|m| m.role == Role::Normal || m.role == Role::Help)
                .map(|m| m.title)
                .collect()
        };
        assert_eq!(titles(false), titles(true));
        assert_eq!(titles(false), ["File", "Edit", "View", "Calibrate", "Help"]);
    }

    #[test]
    fn no_duplicate_shortcuts() {
        for mac in [false, true] {
            let mut seen: Vec<(Key, Mods, String)> = vec![];
            for m in menus(mac) {
                for e in &m.entries {
                    let (chord, label) = match e {
                        Entry::Item(i) => (
                            shortcut(i.action).map(|b| (b.key, b.mods)).or(if mac {
                                native_chord(i.action)
                            } else {
                                None
                            }),
                            i.label.clone(),
                        ),
                        Entry::Native(n) => (n.chord(), format!("{n:?}")),
                        _ => (None, String::new()),
                    };
                    let Some((key, mods)) = chord else { continue };
                    // The same act in two menus (auto-slice) has no shortcut; any other
                    // repeat is a clash.
                    if let Some(prev) = seen.iter().find(|s| s.0 == key && s.1 == mods) {
                        panic!("{label} and {} share a shortcut", prev.2);
                    }
                    seen.push((key, mods, label));
                }
            }
        }
    }

    #[test]
    fn native_chords_have_one_meaning() {
        // Every chord the native menu takes is bound in the keymap only to the act of the item
        // that shows it (or to nothing), so the keymap skipping it loses nothing.
        for m in menus(true) {
            for e in &m.entries {
                let (chord, act) = match e {
                    Entry::Item(i) => (native_chord(i.action), i.action.act()),
                    Entry::Native(Native::Minimize) => {
                        (Native::Minimize.chord(), Some(Act::Minimize))
                    }
                    Entry::Native(Native::Fullscreen) => {
                        (Native::Fullscreen.chord(), Some(Act::Fullscreen))
                    }
                    Entry::Native(n) => (n.chord(), None),
                    _ => (None, None),
                };
                let Some((key, mods)) = chord else { continue };
                assert!(mods.has(Mods::CMD), "{key:?}: a native chord without Cmd");
                for b in keymap::BINDINGS
                    .iter()
                    .filter(|b| b.key == key && b.mods == mods)
                {
                    assert_eq!(Some(b.act), act, "{key:?} {mods:?}");
                }
            }
        }
        // Only chords that need it: plain keys, Shift+Tab and `?` stay with the keymap.
        assert!(!native_owns(Key::A, Mods::NONE));
        assert!(!native_owns(Key::Delete, Mods::NONE));
        assert!(!native_owns(Key::Tab, Mods::SHIFT));
        assert!(native_owns(Key::S, Mods::CMD));
        assert!(native_owns(Key::Q, Mods::CMD));
        assert!(native_owns(Key::Z, Mods::CMD));
        assert!(native_owns(Key::Comma, Mods::CMD));
    }

    #[test]
    fn shortcuts_fire_once_with_the_native_menu() {
        // With the native menu up, a chord it owns reaches no keymap binding (the menu runs
        // it); without it, the keymap still does. Keys it does not own work either way.
        let mut cx = keymap::Context::default();
        let cmd = egui::Modifiers::COMMAND;
        for &(key, mods) in native_chords() {
            // Off the Mac Ctrl is the command key: a Cmd+Ctrl chord cannot be typed there.
            if !cfg!(target_os = "macos") && mods.has(Mods::CTRL) {
                continue;
            }
            let m = egui::Modifiers {
                alt: mods.has(Mods::ALT),
                shift: mods.has(Mods::SHIFT),
                ctrl: mods.has(Mods::CTRL),
                mac_cmd: cfg!(target_os = "macos"),
                command: true,
            };
            cx.native_menu = true;
            assert!(
                keymap::lookup(key, m, false, cx).is_none(),
                "{key:?} {mods:?}"
            );
        }
        cx.native_menu = false;
        assert_eq!(
            keymap::lookup(Key::S, cmd, false, cx).map(|b| b.act),
            Some(Act::Save)
        );
        cx.native_menu = true;
        assert_eq!(
            keymap::lookup(Key::A, egui::Modifiers::NONE, false, cx).map(|b| b.act),
            Some(Act::ArrangeAll)
        );
        assert_eq!(
            keymap::lookup(Key::I, cmd, false, cx).map(|b| b.act),
            Some(Act::Import),
            "not in a menu: still the keymap's"
        );
    }

    #[test]
    fn menu_state_and_actions_on_an_empty_plate() {
        let ctx = egui::Context::default();
        let mut app = crate::app::App::create(
            &ctx,
            crate::app::Options {
                prefs_path: None,
                store: Some(mica_profiles::user::Store::open(
                    std::env::temp_dir().join(format!("mica-menu-test-{}", std::process::id())),
                )),
                files: vec![],
                headless: true,
            },
        );
        let st = |app: &crate::app::App, a| app.menu_state(a, false);
        assert!(!st(&app, Action::Act(Act::Undo)).enabled);
        assert!(!st(&app, Action::Act(Act::Copy)).enabled);
        assert!(
            app.menu_state(Action::Act(Act::Copy), true).enabled,
            "text field"
        );
        assert!(!st(&app, Action::Act(Act::ExportGcode)).enabled);
        // Toggles flip their checked state through `run_menu`.
        for a in [
            Action::Act(Act::CollapseSidebar),
            Action::Act(Act::TogglePerspective),
            Action::ShowAllSettings,
            Action::AutoSlice,
            Action::PaintOverlay(PaintKind::Seam),
        ] {
            let before = st(&app, a).checked;
            app.run_menu(a, &ctx);
            assert_eq!(st(&app, a).checked, !before, "{a:?}");
        }
        app.run_menu(Action::SidebarRight(true), &ctx);
        assert!(st(&app, Action::SidebarRight(true)).checked);
        assert!(!st(&app, Action::SidebarRight(false)).checked);
        app.run_menu(Action::Act(Act::ShowShortcuts), &ctx);
        assert!(app.keys.dialog);
        // A disabled item does nothing.
        app.run_menu(Action::Act(Act::DeleteAll), &ctx);
    }
}
