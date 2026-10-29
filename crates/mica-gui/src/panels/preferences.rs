// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: orca@8500fcdcca src/slic3r/GUI/Preferences.cpp :: PreferencesDialog::create_items
// Changes: Same idea (app-level settings in titled sections, one row per item) with far fewer
// items, plain labels and a short description under each instead of tooltips; the delay sits
// under its auto-slice toggle; no networking items; the sections grouped on tabs with icons,
// like the macOS Settings windows; written from scratch for egui.

//! Settings (⌘,; File › Settings… off the Mac): the app-level settings that were spread over
//! the File and View menus or only lived in `mica.toml`, on five tabs. Every change applies
//! at once and is saved with the other preferences, and so is the open tab.

use super::{settings_icons, widgets};
use crate::app::App;
use crate::state::{AUTO_SLICE_DELAY_RANGE_MS, MAX_RECENT};
use crate::theme::{self, Choice};
use eframe::egui::{self, Align, Key, Layout, Modifiers, Pos2, Rect, Sense, Vec2};

/// Width of the control column.
const CONTROL_W: f32 = 250.;
const WINDOW_W: f32 = 620.;
/// One tab of the tab row.
const TAB_SIZE: Vec2 = Vec2::new(80., 50.);
/// The tallest the window gets; a tab with more scrolls.
const MAX_HEIGHT: f32 = 640.;

/// The tabs of the Settings window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrefsTab {
    /// Appearance and the 3D view.
    General,
    /// Automatic slicing and what new projects start with.
    Slicing,
    /// Profiles from other slicers: importing, removing, merging, re-importing.
    Profiles,
    /// Recent files, updates, crash reports and the model library's folders.
    Files,
    /// Mica's own features, each with a switch.
    Features,
}

impl PrefsTab {
    pub const ALL: [Self; 5] = [
        Self::General,
        Self::Slicing,
        Self::Profiles,
        Self::Files,
        Self::Features,
    ];

    /// The name kept in `mica.toml` (`preferences_tab`).
    pub fn key(self) -> &'static str {
        match self {
            Self::General => "general",
            Self::Slicing => "slicing",
            Self::Profiles => "profiles",
            Self::Files => "files",
            Self::Features => "features",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Slicing => "Slicing",
            Self::Profiles => "Profiles",
            Self::Files => "Files",
            Self::Features => "Features",
        }
    }

    /// The tab a remembered name stands for; anything unknown is General.
    pub fn from_key(key: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|t| t.key() == key.trim())
            .unwrap_or(Self::General)
    }

    /// The tab `step` places further along the row, wrapping around at either end.
    pub(crate) fn step(self, step: isize) -> Self {
        let n = Self::ALL.len() as isize;
        let i = Self::ALL.iter().position(|&t| t == self).unwrap_or(0) as isize;
        Self::ALL[(i + step).rem_euclid(n) as usize]
    }
}

impl App {
    /// Opens the Settings window, on `tab` when given (else on the tab open last).
    pub(crate) fn open_preferences(&mut self, tab: Option<PrefsTab>) {
        if let Some(t) = tab {
            self.set_preferences_tab(t);
        }
        self.dialogs.preferences = true;
        // Settings › Profiles looks at the other slicers' files again.
        self.import_ui.manage = None;
    }

    /// The Settings window's open tab (remembered across launches).
    pub(crate) fn preferences_tab(&self) -> PrefsTab {
        PrefsTab::from_key(&self.prefs.preferences_tab)
    }

    pub(crate) fn set_preferences_tab(&mut self, tab: PrefsTab) {
        if self.prefs.preferences_tab != tab.key() {
            self.prefs.preferences_tab = tab.key().into();
            self.prefs_dirty = true;
        }
    }

    pub(crate) fn preferences_window(&mut self, ctx: &egui::Context) {
        let mut open = self.dialogs.preferences;
        let mut theme_pick: Option<Choice> = None;
        let mut done = false;
        // Esc closes the window, unless it only closes a dropdown or ends typing in a field.
        let escape = !widgets::field_editing(ctx)
            && !egui::Popup::is_any_open(ctx)
            && !ctx.egui_wants_keyboard_input()
            && ctx.input(|i| i.key_pressed(Key::Escape));
        // Ctrl+Tab / Ctrl+Shift+Tab and ⌘] / ⌘[ step through the tabs.
        let step = ctx.input_mut(|i| {
            if i.consume_key(Modifiers::CTRL | Modifiers::SHIFT, Key::Tab)
                || i.consume_key(Modifiers::COMMAND, Key::OpenBracket)
            {
                -1
            } else if i.consume_key(Modifiers::CTRL, Key::Tab)
                || i.consume_key(Modifiers::COMMAND, Key::CloseBracket)
            {
                1
            } else {
                0
            }
        });
        if step != 0 {
            self.set_preferences_tab(self.preferences_tab().step(step));
        }
        // The same size on every tab: each tab scrolls on its own when it needs more room.
        let height = (ctx.content_rect().height() - 120.).clamp(320., MAX_HEIGHT);
        egui::Window::new("Settings")
            .id(egui::Id::unique("mica-preferences"))
            .open(&mut open)
            .default_pos(ctx.content_rect().center_top() + egui::vec2(-WINDOW_W / 2., 70.))
            .fixed_size([WINDOW_W, height])
            .collapsible(false)
            .show(ctx, |ui| {
                let p = theme::of(ui);
                if let Some(t) = tab_row(ui, self.preferences_tab()) {
                    self.set_preferences_tab(t);
                }
                let tab = self.preferences_tab();
                ui.separator();
                let footer = 40.;
                egui::ScrollArea::vertical()
                    .id_salt(("mica-preferences-tab", tab.key()))
                    .max_height((ui.available_height() - footer).max(80.))
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width() - 8.);
                        match tab {
                            PrefsTab::General => {
                                self.appearance_section(ui, &mut theme_pick);
                                self.view_section(ui);
                            }
                            PrefsTab::Slicing => {
                                self.slicing_section(ui);
                                self.new_project_section(ui);
                            }
                            PrefsTab::Profiles => self.profiles_section(ui),
                            PrefsTab::Files => {
                                self.files_section(ui);
                                self.library_section(ui);
                            }
                            PrefsTab::Features => self.features_section(ui),
                        }
                    });
                ui.add_space(6.);
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(
                            "Changes apply right away and are remembered the next time Mica \
                             starts.",
                        )
                        .font(theme::sans(12.))
                        .color(p.muted),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if widgets::primary_button(ui, "Done", true).clicked() {
                            done = true;
                        }
                    });
                });
            });
        if let Some(c) = theme_pick {
            self.set_theme_choice(ctx, c);
        }
        self.dialogs.preferences = open && !done && !escape;
    }

    fn appearance_section(&mut self, ui: &mut egui::Ui, theme_pick: &mut Option<Choice>) {
        widgets::group_title(ui, "Appearance");
        let current = Choice::from_name(&self.prefs.theme);
        item(
            ui,
            "Theme",
            "Dark is easiest on the eyes next to a printer. Follow the system switches with \
             your computer's light and dark setting.",
            |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.;
                    for c in Choice::ALL {
                        if widgets::chip(ui, None, short_label(c), c == current, true).clicked()
                            && c != current
                        {
                            *theme_pick = Some(c);
                        }
                    }
                });
            },
        );
        let right = self.prefs.sidebar_right;
        let mut pick = None;
        item(
            ui,
            "Settings sidebar",
            "Which side of the window the printer, profiles and settings sit on.",
            |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.;
                    if widgets::chip(ui, None, "Left", !right, true).clicked() {
                        pick = Some(false);
                    }
                    if widgets::chip(ui, None, "Right", right, true).clicked() {
                        pick = Some(true);
                    }
                });
            },
        );
        if let Some(r) = pick
            && r != right
        {
            self.prefs.sidebar_right = r;
            self.prefs_dirty = true;
        }
    }

    /// Mica's own features, each with a switch (`[features]` in mica.toml).
    fn features_section(&mut self, ui: &mut egui::Ui) {
        widgets::group_title(ui, "Features");
        for f in crate::state::Feature::ALL {
            // Brick layers is a hidden opt-in: its switch shows only once it was turned on by
            // hand (`brick_layers = true` under [features] in mica.toml).
            if f.hidden() && !self.prefs.features.on(f) {
                continue;
            }
            // A feature that is not built yet shows its switch off and disabled.
            let available = f.available();
            let on = available && self.prefs.features.on(f);
            let mut flip = None;
            item(ui, f.title(), f.description(), |ui| {
                flip = switch(ui, &format!("prefs-feature-{}", f.key()), on, available)
            });
            if let Some(on) = flip {
                self.prefs.features.set(f, on);
                self.prefs_dirty = true;
                self.settings_changed();
            }
        }
    }

    fn slicing_section(&mut self, ui: &mut egui::Ui) {
        widgets::group_title(ui, "Slicing");
        let auto = self.prefs.auto_slice;
        let mut flip = None;
        item(
            ui,
            "Slice automatically",
            "Off: press Slice or open Preview when you are ready. On: Mica slices the \
             plate shortly after every change.",
            |ui| flip = switch(ui, "prefs-auto-slice", auto, true),
        );
        if let Some(on) = flip {
            self.prefs.auto_slice = on;
            self.prefs_dirty = true;
            if on {
                self.dirty_since = Some(std::time::Instant::now());
            }
        }
        if self.prefs.auto_slice {
            // The delay only matters with auto-slice on, so it sits right under it.
            let mut seconds = self.prefs.auto_slice_delay_ms as f32 / 1000.;
            let mut changed = false;
            indented(ui, |ui| {
                item(
                    ui,
                    "Wait before slicing",
                    "How long Mica waits after your last change. Longer saves work while you \
                     are still adjusting things.",
                    |ui| {
                        let lo = *AUTO_SLICE_DELAY_RANGE_MS.start() as f32 / 1000.;
                        let hi = *AUTO_SLICE_DELAY_RANGE_MS.end() as f32 / 1000.;
                        changed = ui
                            .add(
                                egui::Slider::new(&mut seconds, lo..=hi)
                                    .step_by(0.1)
                                    .fixed_decimals(1)
                                    .suffix(" s"),
                            )
                            .changed();
                    },
                );
            });
            if changed {
                self.prefs.auto_slice_delay_ms = (seconds * 1000.).round() as u64;
                self.prefs_dirty = true;
            }
        }
    }

    fn new_project_section(&mut self, ui: &mut egui::Ui) {
        widgets::group_title(ui, "New projects");
        // Printer: the remembered printers (My printers in the sidebar) or the last one used.
        let mut printers: Vec<(String, String)> = vec![(String::new(), "Last used".into())];
        for (vi, name) in self.profiles.my_printers(&self.prefs) {
            let vendor = self.profiles.vendors[vi].0.clone();
            printers.push((crate::state::printer_id(&vendor, &name), name));
        }
        let mut pick_printer = None;
        item(
            ui,
            "Printer",
            "The printer Mica starts with. Last used picks up where you left off.",
            |ui| {
                pick_printer = dropdown(
                    ui,
                    "prefs-default-printer",
                    &self.prefs.defaults.printer,
                    &printers,
                );
            },
        );
        let printer_label = super::sidebar::split_printer_name(&self.profiles.printer).0;
        let mut prints: Vec<(String, String)> =
            vec![(String::new(), "The printer's default".into())];
        let labels =
            crate::preset_names::labels(self.profiles.processes.iter().map(String::as_str));
        prints.extend(
            self.profiles
                .processes
                .iter()
                .zip(labels)
                .map(|(n, l)| (n.clone(), l.text())),
        );
        let mut pick_print = None;
        let print_hint = format!(
            "What New project starts with. The list shows the {printer_label}'s profiles; a \
             printer without the chosen one uses its own default."
        );
        item(ui, "Print profile", &print_hint, |ui| {
            pick_print = dropdown(
                ui,
                "prefs-default-print",
                &self.prefs.defaults.print,
                &prints,
            );
        });
        let mut filaments: Vec<(String, String)> =
            vec![(String::new(), "The printer's default".into())];
        let labels =
            crate::preset_names::labels(self.profiles.materials.iter().map(String::as_str));
        filaments.extend(
            self.profiles
                .materials
                .iter()
                .zip(labels)
                .map(|(n, l)| (n.clone(), l.text())),
        );
        let mut pick_filament = None;
        item(
            ui,
            "Filament",
            "Loaded on every tool of a new project, when the printer offers it.",
            |ui| {
                pick_filament = dropdown(
                    ui,
                    "prefs-default-filament",
                    &self.prefs.defaults.filament,
                    &filaments,
                );
            },
        );
        for (pick, field) in [
            (pick_printer, &mut self.prefs.defaults.printer),
            (pick_print, &mut self.prefs.defaults.print),
            (pick_filament, &mut self.prefs.defaults.filament),
        ] {
            if let Some(v) = pick
                && *field != v
            {
                *field = v;
                self.prefs_dirty = true;
            }
        }
    }

    fn view_section(&mut self, ui: &mut egui::Ui) {
        widgets::group_title(ui, "3D view");
        let mut perspective = None;
        item(
            ui,
            "Perspective camera",
            "Off shows the plate without perspective (orthographic), which makes sizes easy to \
             compare. The P key switches too.",
            |ui| {
                perspective = switch(
                    ui,
                    "prefs-perspective",
                    self.viewport.camera.perspective,
                    true,
                )
            },
        );
        if let Some(on) = perspective {
            self.viewport.camera.perspective = on;
        }
        let mut towards = None;
        item(
            ui,
            "Zoom towards the pointer",
            "Scrolling zooms in on the spot under the mouse. Off zooms on the middle of the view.",
            |ui| towards = switch(ui, "prefs-zoom-pointer", self.prefs.zoom_to_pointer, true),
        );
        if let Some(on) = towards {
            self.prefs.zoom_to_pointer = on;
            self.prefs_dirty = true;
        }
        let mut screen_center = None;
        item(
            ui,
            "Orbit around the screen center",
            &crate::keymap::localise(
                "Like PrusaSlicer: the view turns around what's in the middle of the screen, \
                 also after panning. Off: around the selected object or the plate's center, \
                 like OrcaSlicer. Cmd-drag always turns around the middle of the screen.",
            ),
            |ui| {
                screen_center = switch(
                    ui,
                    "prefs-orbit-center",
                    self.prefs.orbit_screen_center,
                    true,
                )
            },
        );
        if let Some(on) = screen_center {
            self.prefs.orbit_screen_center = on;
            self.prefs_dirty = true;
        }
        let mut reverse = None;
        item(
            ui,
            "Reverse scroll zoom",
            "Scrolling up zooms out instead of in.",
            |ui| reverse = switch(ui, "prefs-reverse-zoom", self.prefs.reverse_zoom, true),
        );
        if let Some(on) = reverse {
            self.prefs.reverse_zoom = on;
            self.prefs_dirty = true;
        }
        let current = self.prefs.toolbar_size;
        let mut size = None;
        item(
            ui,
            "Toolbar size",
            "How big the tool buttons over the 3D view and the plate bar are. Larger buttons \
             leave less room, so more tools move into the More menu on a narrow window.",
            |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.;
                    for s in crate::state::ToolbarSize::ALL {
                        if widgets::chip(ui, None, s.label(), s == current, true).clicked() {
                            size = Some(s);
                        }
                    }
                });
            },
        );
        if let Some(s) = size
            && s != current
        {
            self.prefs.toolbar_size = s;
            self.viewport.toolbar_size = s;
            self.prefs_dirty = true;
        }
    }

    fn files_section(&mut self, ui: &mut egui::Ui) {
        widgets::group_title(ui, "Files and updates");
        let mut n = self.prefs.recent_limit;
        let mut changed = false;
        item(
            ui,
            "Recent files",
            "How many files File › Open recent remembers. 0 keeps no list.",
            |ui| {
                changed = ui
                    .add(egui::Slider::new(&mut n, 0..=MAX_RECENT).suffix(" files"))
                    .changed();
            },
        );
        if changed {
            self.prefs.set_recent_limit(n);
            self.prefs_dirty = true;
        }
        let mut auto = None;
        let mut check_now = false;
        let hint = format!(
            "Once a day at start, Mica asks {} whether a newer {} version exists (it sends \
             only its version and platform); updates download from GitHub. You have {}. \
             Help › Check for Updates… (the Mica menu on a Mac) asks at any time.{}",
            mica_update::UPDATE_BASE_URL.trim_start_matches("https://"),
            mica_update::CHANNEL,
            mica_update::current_version(),
            if self.updater.enabled() {
                String::new()
            } else {
                " This build does not update itself.".into()
            }
        );
        item(ui, "Check for Mica updates", &hint, |ui| {
            ui.horizontal(|ui| {
                auto = switch(ui, "prefs-app-updates", self.prefs.check_updates, true);
                check_now = widgets::button(ui, "Check now", !self.updater.working()).clicked();
            });
        });
        if let Some(on) = auto {
            self.prefs.check_updates = on;
            self.prefs_dirty = true;
        }
        if check_now {
            let ctx = ui.ctx().clone();
            self.check_for_updates(&ctx);
        }
        item(
            ui,
            "Check for profile updates",
            "Off. Mica never goes online for profiles: new printer, print and filament \
             profiles arrive with Mica updates.",
            |ui| {
                switch(ui, "prefs-profile-updates", false, false);
            },
        );
        let dir = mica_core::job::log_directory();
        let mut open_dir = false;
        let p = theme::of(ui);
        let hint = format!(
            "If slicing fails unexpectedly, Mica writes a report here. Attach it when you \
             report the problem.\n{}",
            dir.display()
        );
        item(ui, "Crash reports", &hint, |ui| {
            let exists = dir.is_dir();
            open_dir = widgets::button(ui, "Show folder", exists && !self.headless)
                .on_hover_text(if exists {
                    "Open the folder in Finder"
                } else {
                    "No reports yet; the folder is made with the first one"
                })
                .clicked();
            if !exists {
                ui.label(
                    egui::RichText::new("No reports yet")
                        .font(theme::sans(11.))
                        .color(p.muted),
                );
            }
        });
        if open_dir {
            let opener = if cfg!(target_os = "macos") {
                "open"
            } else if cfg!(target_os = "windows") {
                "explorer"
            } else {
                "xdg-open"
            };
            if let Err(e) = std::process::Command::new(opener).arg(&dir).spawn() {
                self.notify_error(
                    "Could not open the folder",
                    format!("Could not open {}: {e}", dir.display()),
                );
            }
        }
    }
}

/// The row of tabs at the top, centered: an icon over each name, the open one highlighted.
/// The tab clicked, if any.
fn tab_row(ui: &mut egui::Ui, current: PrefsTab) -> Option<PrefsTab> {
    let tabs: Vec<IconTab> = PrefsTab::ALL
        .into_iter()
        .map(|tab| IconTab {
            key: tab.key(),
            label: tab.label(),
            hint: match tab {
                PrefsTab::General => "Appearance and the 3D view",
                PrefsTab::Slicing => "Automatic slicing and new projects",
                PrefsTab::Profiles => "Profiles imported from other slicers",
                PrefsTab::Files => "Recent files, updates and the model library",
                PrefsTab::Features => "Turn Mica's own features on or off",
            },
        })
        .collect();
    let at = PrefsTab::ALL
        .iter()
        .position(|&t| t == current)
        .unwrap_or(0);
    icon_tab_row(ui, "prefs-tab", &tabs, at).map(|k| PrefsTab::ALL[k])
}

/// One tab of an [`icon_tab_row`]: `key` picks its icon (`settings_icons::settings_tab`).
pub(crate) struct IconTab {
    pub key: &'static str,
    pub label: &'static str,
    pub hint: &'static str,
}

/// A centered row of icon tabs, like the macOS Settings windows (the Settings and About
/// windows). `current` is the open tab's index; returns the index of a tab clicked.
pub(crate) fn icon_tab_row(
    ui: &mut egui::Ui,
    id: &str,
    tabs: &[IconTab],
    current: usize,
) -> Option<usize> {
    let p = theme::of(ui);
    let gap = 4.;
    let n = tabs.len() as f32;
    let (row, _) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), TAB_SIZE.y + 4.),
        Sense::hover(),
    );
    let left = row.center().x - (n * TAB_SIZE.x + (n - 1.) * gap) / 2.;
    let mut picked = None;
    for (k, tab) in tabs.iter().enumerate() {
        let r = Rect::from_min_size(
            Pos2::new(left + k as f32 * (TAB_SIZE.x + gap), row.top() + 2.),
            TAB_SIZE,
        );
        let response = ui
            .interact(r, egui::Id::unique((id, tab.key)), Sense::click())
            .on_hover_text(tab.hint);
        let active = k == current;
        let painter = ui.painter();
        if active {
            painter.rect_filled(r, 8., p.accent_surface);
        } else if response.hovered() {
            painter.rect_filled(r, 8., p.hover);
        }
        let color = if active {
            p.mark_fg
        } else if response.hovered() {
            p.text
        } else {
            p.muted_alt
        };
        settings_icons::settings_tab(
            painter,
            Rect::from_center_size(Pos2::new(r.center().x, r.top() + 17.), Vec2::splat(20.)),
            tab.key,
            color,
        );
        let label = painter.layout_no_wrap(tab.label.into(), theme::sans(11.5), color);
        painter.galley(
            Pos2::new(r.center().x - label.size().x / 2., r.top() + 32.),
            label,
            color,
        );
        if response.hovered() {
            ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
        }
        if response.clicked() && !active {
            picked = Some(k);
        }
    }
    picked
}

fn short_label(c: Choice) -> &'static str {
    match c {
        Choice::System => "System",
        _ => c.label(),
    }
}

/// One preference: its name and a short description on the left, the control on the right.
pub(crate) fn item(
    ui: &mut egui::Ui,
    title: &str,
    description: &str,
    control: impl FnOnce(&mut egui::Ui),
) {
    let p = theme::of(ui);
    let width = ui.available_width();
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 12.;
        ui.allocate_ui_with_layout(
            Vec2::new(width - CONTROL_W - 12., 0.),
            Layout::top_down(Align::Min),
            |ui| {
                ui.set_width(width - CONTROL_W - 12.);
                ui.spacing_mut().item_spacing.y = 2.;
                ui.label(
                    egui::RichText::new(title)
                        .font(theme::medium(13.))
                        .color(p.text),
                );
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(description)
                            .font(theme::sans(11.5))
                            .color(p.muted),
                    )
                    .wrap(),
                );
            },
        );
        ui.allocate_ui_with_layout(
            Vec2::new(CONTROL_W, 30.),
            Layout::top_down(Align::Min),
            control,
        );
    });
    ui.add_space(10.);
}

/// A dependent setting: indented under the toggle it belongs to, with a guide line.
fn indented(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    let p = theme::of(ui);
    let top = ui.cursor().top();
    ui.horizontal_top(|ui| {
        ui.add_space(14.);
        ui.vertical(|ui| add(ui));
    });
    let bottom = ui.cursor().top() - 10.;
    let x = ui.min_rect().left() + 4.;
    ui.painter().line_segment(
        [egui::pos2(x, top), egui::pos2(x, bottom)],
        egui::Stroke::new(2., p.border_raised),
    );
}

/// A toggle switch; the new state when clicked.
fn switch(ui: &mut egui::Ui, id: &str, on: bool, enabled: bool) -> Option<bool> {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(34., 20.), Sense::hover());
    widgets::toggle_at(ui, egui::Id::unique(id), rect, on, enabled)
}

/// A full-width dropdown of `(value, label)` items; the chosen value.
fn dropdown(
    ui: &mut egui::Ui,
    id: &str,
    current: &str,
    items: &[(String, String)],
) -> Option<String> {
    let label = items
        .iter()
        .find(|(v, _)| v == current)
        .map_or_else(
            || crate::preset_names::display_name(current),
            |(_, l)| l.as_str(),
        )
        .to_string();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(CONTROL_W, 30.), Sense::hover());
    widgets::dropdown_with(
        ui,
        egui::Id::unique(id),
        rect,
        current,
        &label,
        None,
        items,
        true,
        false,
    )
}

#[cfg(test)]
mod tests {
    use super::PrefsTab;
    use crate::app::{App, Options};
    use crate::notifications::Action;
    use eframe::egui::{self, Key, Modifiers, Pos2};
    use mica_profiles::user::Store;
    use std::path::{Path, PathBuf};

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mica-prefs-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn app(ctx: &egui::Context, dir: &Path) -> App {
        App::create(
            ctx,
            Options {
                prefs_path: None,
                store: Some(Store::open(dir.join("store"))),
                files: vec![],
                headless: true,
            },
        )
    }

    /// One whole-window frame; the texts drawn, with their centers.
    fn frame(ctx: &egui::Context, app: &mut App, events: Vec<egui::Event>) -> Vec<(String, Pos2)> {
        fn texts(shape: egui::Shape, out: &mut Vec<(String, Pos2)>) {
            match shape {
                egui::Shape::Text(t) => {
                    out.push((t.galley.text().to_string(), t.pos + t.galley.size() / 2.));
                }
                egui::Shape::Vec(shapes) => shapes.into_iter().for_each(|s| texts(s, out)),
                _ => {}
            }
        }
        let input = egui::RawInput {
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
        out
    }

    fn key(key: Key, modifiers: Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    #[test]
    fn tab_names_round_trip_and_unknown_ones_open_general() {
        for t in PrefsTab::ALL {
            assert_eq!(PrefsTab::from_key(t.key()), t);
        }
        assert_eq!(PrefsTab::from_key("advanced"), PrefsTab::General);
        assert_eq!(PrefsTab::from_key(""), PrefsTab::General);
        assert_eq!(PrefsTab::General.step(-1), PrefsTab::Features);
        assert_eq!(PrefsTab::Features.step(1), PrefsTab::General);
        let defaults = crate::state::Prefs::default();
        assert_eq!(
            PrefsTab::from_key(&defaults.preferences_tab),
            PrefsTab::General
        );
    }

    #[test]
    fn the_open_tab_is_remembered_across_launches() {
        let dir = temp_dir("tab-persists");
        let ctx = egui::Context::default();
        let mut first = app(&ctx, &dir);
        assert_eq!(first.preferences_tab(), PrefsTab::General);
        first.open_preferences(None);
        frame(&ctx, &mut first, vec![]);
        // Clicking a tab switches to it.
        let texts = frame(&ctx, &mut first, vec![]);
        let at = texts
            .iter()
            .find(|(t, _)| t == "Files")
            .map(|(_, p)| *p)
            .expect("the Files tab is drawn");
        for e in [
            egui::Event::PointerMoved(at),
            egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::NONE,
            },
            egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Modifiers::NONE,
            },
        ] {
            frame(&ctx, &mut first, vec![e]);
        }
        assert_eq!(first.preferences_tab(), PrefsTab::Files);
        let texts = frame(&ctx, &mut first, vec![]);
        assert!(texts.iter().any(|(t, _)| t == "Recent files"));
        assert!(
            !texts.iter().any(|(t, _)| t == "Theme"),
            "one tab at a time"
        );
        assert!(first.prefs_dirty);
        // What the next launch reads back from mica.toml.
        let saved = crate::state::Prefs::from_toml(&first.prefs.to_toml()).unwrap();
        drop(first);
        let ctx = egui::Context::default();
        let mut second = app(&ctx, &dir);
        second.prefs = saved;
        assert_eq!(second.preferences_tab(), PrefsTab::Files);
        second.open_preferences(None);
        frame(&ctx, &mut second, vec![]);
        let texts = frame(&ctx, &mut second, vec![]);
        assert!(texts.iter().any(|(t, _)| t == "Settings"));
        assert!(texts.iter().any(|(t, _)| t == "Recent files"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn links_open_settings_on_their_tab_and_keys_switch_and_close() {
        let dir = temp_dir("open-on-tab");
        let ctx = egui::Context::default();
        let mut app = app(&ctx, &dir);
        frame(&ctx, &mut app, vec![]);
        // The disabled-feature notice's Settings link.
        app.run_notification_action(0, Action::OpenPreferences(PrefsTab::Features), &ctx);
        assert!(app.dialogs.preferences);
        assert_eq!(app.preferences_tab(), PrefsTab::Features);
        frame(&ctx, &mut app, vec![]);
        let texts = frame(&ctx, &mut app, vec![]);
        let first_feature = crate::state::Feature::ALL[0].title();
        assert!(
            texts.iter().any(|(t, _)| t == first_feature),
            "the Features tab shows the switches"
        );
        // Ctrl+Tab wraps around to General, ⌘[ goes back.
        frame(&ctx, &mut app, vec![key(Key::Tab, Modifiers::CTRL)]);
        assert_eq!(app.preferences_tab(), PrefsTab::General);
        frame(
            &ctx,
            &mut app,
            vec![key(Key::OpenBracket, Modifiers::COMMAND)],
        );
        assert_eq!(app.preferences_tab(), PrefsTab::Features);
        frame(
            &ctx,
            &mut app,
            vec![key(Key::CloseBracket, Modifiers::COMMAND)],
        );
        assert_eq!(app.preferences_tab(), PrefsTab::General);
        frame(
            &ctx,
            &mut app,
            vec![key(Key::Tab, Modifiers::CTRL | Modifiers::SHIFT)],
        );
        assert_eq!(app.preferences_tab(), PrefsTab::Features);
        assert!(app.dialogs.preferences);
        // Esc closes; ⌘, opens again on the tab open last.
        frame(&ctx, &mut app, vec![key(Key::Escape, Modifiers::NONE)]);
        assert!(!app.dialogs.preferences);
        frame(&ctx, &mut app, vec![key(Key::Comma, Modifiers::COMMAND)]);
        assert!(app.dialogs.preferences);
        assert_eq!(app.preferences_tab(), PrefsTab::Features);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
