// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/slic3r/GUI/NotificationManager.hpp :: NotificationManager::NotificationLevel, NotificationManager::get_standard_duration
// Source: prusaslicer@b028299c77 src/slic3r/GUI/NotificationManager.cpp :: NotificationManager::push_slicing_error_notification, NotificationManager::push_plater_warning_notification, NotificationManager::close_plater_warning_notification, NotificationManager::render_notifications
// Source: orca@8500fcdcca src/slic3r/GUI/NotificationManager.cpp :: NotificationManager::render_notifications
// Changes: Three levels instead of upstream's eight (errors and warnings never fade, information fades after a few seconds, as get_standard_duration); a plate's errors and warnings belong to that plate and show while it is current; state-derived notifications (a slice error, plate warnings, guard rails) are re-synced every frame the way upstream re-pushes or closes plater warnings while their condition holds; actions are typed enums instead of hyperlink callbacks; egui cards stacked in the 3D view's bottom-right corner.

//! Notifications: the one place errors, warnings and short confirmations are shown, stacked in
//! the bottom-right corner of the 3D view as PrusaSlicer's and Orca's notification managers do.
//!
//! - **Error** (red): stays until its cause is resolved or it is dismissed (a failed slice, a
//!   save that failed).
//! - **Warning** (orange): stays while its condition holds (an object or its brim outside the
//!   bed, a guard rail).
//! - **Info**: fades after a few seconds (exported, uploaded).
//!
//! A notification can belong to a plate (`plate`, by `Plate::id`): it shows only while that
//! plate is current, and the plate's label carries its marker. Notifications can carry
//! actions ("Move inside", "Arrange", "Show details"), which the app runs
//! (`App::run_notification_action`). The viewport's hint line is for progress and hints only.

use crate::theme;
use eframe::egui::{self, Align2, Color32, Pos2, Rect, Stroke, Vec2};
use std::collections::HashSet;
use std::time::{Duration, Instant};

/// How long an information notification stays (upstream's `PrintInfoShortNotificationLevel`).
pub const INFO_LIFE: Duration = Duration::from_secs(6);
/// A notification from a printer (a job finished) stays longer.
pub const PRINTER_LIFE: Duration = Duration::from_secs(20);
/// At most this many cards are stacked; the rest are counted.
const SHOWN_MAX: usize = 5;
const WIDTH: f32 = 360.;
/// A notification text up to this many characters (about two lines of the card) is shown
/// whole, without "Show details".
/// Characters of a title that fit the card's title row.
const TITLE_FIT: usize = 44;
const SHORT_TEXT: usize = 110;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Level {
    Info,
    Warning,
    Error,
}

impl Level {
    pub fn colour(self) -> Color32 {
        match self {
            Level::Info => theme::ACCENT_BRIGHT,
            Level::Warning => theme::WARNING,
            Level::Error => theme::DANGER,
        }
    }
}

/// What a notification's link does.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// Move an object (by `Object::id`) of a plate (by `Plate::id`) just inside the bed.
    MoveInside {
        plate: u64,
        object: u64,
    },
    /// Scale an object (by `Object::id`) of a plate (by `Plate::id`) down uniformly until it
    /// fits the printable volume, centred on the bed.
    ScaleToFit {
        plate: u64,
        object: u64,
    },
    /// Arrange the current plate.
    Arrange,
    /// Expand the notification to its full text.
    Details,
    /// Show the Machine view for this printer (`Printer::id`).
    OpenMachine(u64),
    /// Apply a guard rail's settings fix.
    Fix(crate::checks::Fix),
    /// Slice the current plate again.
    Slice,
    /// The update on offer (`updater.rs`): its release notes, install it, not now, stop the
    /// download, restart into it, try a failed check or install again.
    UpdateNotes,
    UpdateInstall,
    UpdateLater,
    UpdateCancel,
    UpdateRestart,
    UpdateRetry,
    /// Open Settings on a tab.
    OpenPreferences(crate::panels::preferences::PrefsTab),
    /// Turn a feature back on (Settings › Features).
    EnableFeature(crate::state::Feature),
    /// Profiles found in other slicers: open the review window, or not now.
    ReviewImports,
    DismissImports,
    /// Show an exported file in Finder (the file manager), selected.
    Reveal(std::path::PathBuf),
}

#[derive(Clone, Debug)]
pub struct Notification {
    pub id: u64,
    pub level: Level,
    /// Identity of a state-derived notification (stable across frames); `None` for pushed ones.
    pub key: Option<String>,
    /// The plate it belongs to (`Plate::id`); `None`: global.
    pub plate: Option<u64>,
    /// The first line, short: "Slice failed: brim doesn't fit on the bed".
    pub title: String,
    /// The full message (may be the same as the title).
    pub text: String,
    pub actions: Vec<(String, Action)>,
    pub born: Instant,
    /// When it fades; `None` stays.
    pub life: Option<Duration>,
}

impl Notification {
    pub fn new(level: Level, title: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            id: 0,
            level,
            key: None,
            plate: None,
            title: title.into(),
            text: text.into(),
            actions: Vec::new(),
            born: Instant::now(),
            life: (level == Level::Info).then_some(INFO_LIFE),
        }
    }
    pub fn on_plate(mut self, plate: u64) -> Self {
        self.plate = Some(plate);
        self
    }
    pub fn keyed(mut self, key: impl Into<String>) -> Self {
        self.key = Some(key.into());
        self
    }
    pub fn action(mut self, label: impl Into<String>, action: Action) -> Self {
        self.actions.push((label.into(), action));
        self
    }
    pub fn life(mut self, life: Duration) -> Self {
        self.life = Some(life);
        self
    }
    fn expired(&self) -> bool {
        self.life.is_some_and(|l| self.born.elapsed() > l)
    }
}

#[derive(Default)]
pub struct Notifications {
    pub list: Vec<Notification>,
    next: u64,
    /// State-derived notifications the user closed, by `(plate, key)`; forgotten once the
    /// condition is gone, so it comes back if it happens again.
    dismissed: HashSet<(Option<u64>, String)>,
    /// Expanded cards (full text), by id.
    expanded: HashSet<u64>,
    /// A card to highlight (the header's failed-slice read-out was clicked) and since when.
    focus: Option<(u64, Instant)>,
    /// A focus request for a notification the next sync brings back.
    pending_focus: Option<(Option<u64>, String)>,
}

impl Notifications {
    /// Adds a pushed notification. An identical one already shown is renewed instead.
    pub fn push(&mut self, mut n: Notification) -> u64 {
        if let Some(old) = self.list.iter_mut().find(|o| {
            o.key.is_none() && o.level == n.level && o.plate == n.plate && o.text == n.text
        }) {
            old.born = Instant::now();
            return old.id;
        }
        self.next += 1;
        n.id = self.next;
        let id = n.id;
        self.list.push(n);
        // Old information goes first when too many pile up.
        while self.list.iter().filter(|n| n.key.is_none()).count() > 12 {
            let Some(i) = self.list.iter().position(|n| n.key.is_none()) else {
                break;
            };
            self.list.remove(i);
        }
        id
    }

    /// Replaces the state-derived notifications with `wanted` (each with a `key`): one that
    /// was already shown keeps its id and age, one whose condition is gone disappears (and its
    /// dismissal is forgotten), a dismissed one stays hidden.
    pub fn sync(&mut self, wanted: Vec<Notification>) {
        let keys: HashSet<(Option<u64>, String)> = wanted
            .iter()
            .filter_map(|n| n.key.clone().map(|k| (n.plate, k)))
            .collect();
        self.dismissed.retain(|k| keys.contains(k));
        let mut old: Vec<Notification> = Vec::new();
        self.list.retain(|n| {
            if n.key.is_some() {
                old.push(n.clone());
                false
            } else {
                true
            }
        });
        for mut n in wanted {
            let Some(key) = n.key.clone() else { continue };
            if self.dismissed.contains(&(n.plate, key.clone())) {
                continue;
            }
            match old
                .iter()
                .find(|o| o.key.as_deref() == Some(key.as_str()) && o.plate == n.plate)
            {
                Some(o) => {
                    n.id = o.id;
                    n.born = o.born;
                }
                None => {
                    self.next += 1;
                    n.id = self.next;
                }
            }
            self.list.push(n);
        }
    }

    /// Closes a notification: a pushed one is removed, a state-derived one hidden until its
    /// condition goes away.
    pub fn dismiss(&mut self, id: u64) {
        if let Some(i) = self.list.iter().position(|n| n.id == id) {
            let n = self.list.remove(i);
            if let Some(key) = n.key {
                self.dismissed.insert((n.plate, key));
            }
        }
        self.expanded.remove(&id);
    }

    /// Shows a dismissed state-derived notification again and highlights it.
    pub fn focus(&mut self, plate: Option<u64>, key: &str) {
        self.dismissed.remove(&(plate, key.to_string()));
        if let Some(n) = self
            .list
            .iter()
            .find(|n| n.plate == plate && n.key.as_deref() == Some(key))
        {
            self.expanded.insert(n.id);
            self.focus = Some((n.id, Instant::now()));
        } else {
            // It appears on the next sync; highlight it then.
            self.pending_focus = Some((plate, key.to_string()));
        }
    }

    /// Drops faded information.
    pub fn expire(&mut self) {
        self.list.retain(|n| !n.expired());
    }

    /// The notifications shown while `plate` is current (`None`: no plate view, only global
    /// ones), errors first, then warnings, then information, older first within a level.
    pub fn visible(&self, plate: Option<u64>) -> Vec<&Notification> {
        let mut out: Vec<&Notification> = self
            .list
            .iter()
            .filter(|n| !n.expired() && (n.plate.is_none() || n.plate == plate && plate.is_some()))
            .collect();
        out.sort_by(|a, b| b.level.cmp(&a.level).then(a.born.cmp(&b.born)));
        out
    }

    /// Whether the state-derived notification `key` of `plate` is shown, or was dismissed by
    /// the user while its condition holds.
    pub fn shown_or_dismissed(&self, plate: Option<u64>, key: &str) -> bool {
        self.list
            .iter()
            .any(|n| n.plate == plate && n.key.as_deref() == Some(key))
            || self.dismissed.contains(&(plate, key.to_string()))
    }

    pub fn any_fading(&self) -> bool {
        self.list.iter().any(|n| n.life.is_some())
    }

    /// Draws the stack with its bottom-right corner at `corner` and returns the clicked
    /// actions (with the notification's id).
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        corner: Pos2,
        plate: Option<u64>,
    ) -> Vec<(u64, Action)> {
        self.resolve_pending_focus();
        let shown: Vec<Notification> = self.visible(plate).into_iter().cloned().collect();
        if shown.is_empty() {
            return Vec::new();
        }
        let hidden = shown.len().saturating_sub(SHOWN_MAX);
        let mut actions = Vec::new();
        let mut close = None;
        let mut toggle = None;
        let focus = self
            .focus
            .filter(|(_, at)| at.elapsed() < Duration::from_millis(1600))
            .map(|(id, _)| id);
        egui::Area::new(egui::Id::unique("notifications"))
            .order(egui::Order::Foreground)
            .fixed_pos(corner)
            .pivot(Align2::RIGHT_BOTTOM)
            .show(ctx, |ui| {
                let p = theme::of(ui);
                ui.set_width(WIDTH);
                ui.spacing_mut().item_spacing.y = 8.;
                if hidden > 0 {
                    ui.label(
                        egui::RichText::new(format!("{hidden} more notification(s)"))
                            .font(theme::sans(11.))
                            .color(p.muted_alt),
                    );
                }
                for n in shown.iter().skip(hidden) {
                    let expanded = self.expanded.contains(&n.id);
                    let fade = n.life.map_or(1., |l| {
                        let left = l.as_secs_f32() - n.born.elapsed().as_secs_f32();
                        left.clamp(0., 1.)
                    });
                    let colour = n.level.colour();
                    let focused = focus == Some(n.id);
                    let r = egui::Frame::new()
                        .fill(p.panel.gamma_multiply(fade))
                        .stroke(Stroke::new(
                            if focused { 2. } else { 1. },
                            if n.level == Level::Info {
                                p.border_raised
                            } else {
                                colour
                            }
                            .gamma_multiply(if focused { 1. } else { 0.7 } * fade),
                        ))
                        .corner_radius(8.)
                        .inner_margin(egui::Margin {
                            left: 16,
                            right: 8,
                            top: 8,
                            bottom: 8,
                        })
                        .show(ui, |ui| {
                            ui.set_width(WIDTH - 24.);
                            ui.spacing_mut().item_spacing.y = 4.;
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 6.;
                                let label = match n.level {
                                    Level::Error => "Error",
                                    Level::Warning => "Warning",
                                    Level::Info => "",
                                };
                                if !label.is_empty() {
                                    ui.label(
                                        egui::RichText::new(label)
                                            .font(theme::semibold(12.))
                                            .color(colour.gamma_multiply(fade)),
                                    );
                                }
                                let w = ui.available_width() - 26.;
                                ui.allocate_ui_with_layout(
                                    Vec2::new(w, 16.),
                                    egui::Layout::left_to_right(egui::Align::Center),
                                    |ui| {
                                        ui.set_width(w);
                                        ui.add(
                                            egui::Label::new(
                                                egui::RichText::new(&n.title)
                                                    .font(theme::semibold(12.))
                                                    .color(p.text.gamma_multiply(fade)),
                                            )
                                            .truncate(),
                                        );
                                    },
                                );
                                if crate::panels::widgets::icon_button_sized(
                                    ui,
                                    crate::panels::widgets::Glyph::Close,
                                    "",
                                    true,
                                    20.,
                                )
                                .on_hover_text(match n.level {
                                    Level::Info => "Close",
                                    _ => "Dismiss",
                                })
                                .clicked()
                                {
                                    close = Some(n.id);
                                }
                            });
                            // A text of a line or two shows whole; a longer one is cut to a
                            // line until "Show details".
                            let short = n.text.chars().count() <= SHORT_TEXT;
                            // A one-line message (text = title) longer than the title row is
                            // cut there; "Show details" shows it whole below.
                            let long_title =
                                n.text == n.title && n.title.chars().count() > TITLE_FIT;
                            let long = (n.text != n.title && !short) || long_title;
                            if n.text != n.title || (long_title && expanded) {
                                let text = egui::RichText::new(&n.text)
                                    .font(theme::sans(12.))
                                    .color(p.muted_alt.gamma_multiply(fade));
                                if expanded || short {
                                    ui.add(egui::Label::new(text).wrap());
                                } else {
                                    ui.add(egui::Label::new(text).truncate());
                                }
                            }
                            if !n.actions.is_empty() || long {
                                ui.horizontal_wrapped(|ui| {
                                    ui.spacing_mut().item_spacing.x = 14.;
                                    for (label, a) in &n.actions {
                                        if crate::panels::widgets::link(ui, label).clicked() {
                                            actions.push((n.id, a.clone()));
                                        }
                                    }
                                    if long
                                        && crate::panels::widgets::link(
                                            ui,
                                            if expanded { "Less" } else { "Show details" },
                                        )
                                        .clicked()
                                    {
                                        toggle = Some(n.id);
                                    }
                                });
                            }
                        });
                    // The level's colour as a bar along the left edge.
                    let rect = r.response.rect;
                    ui.painter().rect_filled(
                        Rect::from_min_max(
                            rect.left_top() + Vec2::new(1., 1.),
                            Pos2::new(rect.left() + 5., rect.bottom() - 1.),
                        ),
                        egui::CornerRadius {
                            nw: 7,
                            sw: 7,
                            ne: 0,
                            se: 0,
                        },
                        colour.gamma_multiply(fade),
                    );
                }
            });
        for (id, a) in &actions {
            if *a == Action::Details {
                toggle = Some(*id);
            }
        }
        actions.retain(|(_, a)| *a != Action::Details);
        if let Some(id) = toggle
            && !self.expanded.remove(&id)
        {
            self.expanded.insert(id);
        }
        if let Some(id) = close {
            self.dismiss(id);
        }
        actions
    }

    fn resolve_pending_focus(&mut self) {
        if let Some((plate, key)) = self.pending_focus.take()
            && let Some(n) = self
                .list
                .iter()
                .find(|n| n.plate == plate && n.key.as_deref() == Some(key.as_str()))
        {
            self.expanded.insert(n.id);
            self.focus = Some((n.id, Instant::now()));
        }
    }
}

// ----- the app's side ------------------------------------------------------------------------

use crate::app::App;

impl App {
    /// Adds a pushed notification.
    pub(crate) fn notify(&mut self, n: Notification) -> u64 {
        self.notifications.push(n)
    }

    /// An error that stays until dismissed: `title` in a few words, `text` the whole message.
    pub(crate) fn notify_error(&mut self, title: impl Into<String>, text: impl Into<String>) {
        self.notify(Notification::new(Level::Error, title, text));
    }

    /// A warning that stays until dismissed.
    pub(crate) fn notify_warning(&mut self, text: impl Into<String>) {
        let text = text.into();
        self.notify(Notification::new(Level::Warning, text.clone(), text));
    }

    /// A confirmation that fades after a few seconds.
    pub(crate) fn notify_info(&mut self, text: impl Into<String>) {
        let text = text.into();
        self.notify(Notification::new(Level::Info, text.clone(), text));
    }

    /// The state-derived notifications, rebuilt every frame: each plate's failed slice, and
    /// the current plate's warnings and guard rails (with their fixes).
    pub(crate) fn sync_notifications(&mut self) {
        let mut wanted = Vec::new();
        let current = self.plates.current;
        for i in 0..self.plates.len() {
            let plate = self.plates.plate(i);
            let Some(e) = &plate.slice_error else {
                continue;
            };
            let mut n = Notification::new(
                Level::Error,
                format!("Slice failed: {}", e.short),
                e.message.clone(),
            )
            .keyed(SLICE_ERROR)
            .on_plate(plate.id);
            if i == current {
                for &(k, _) in &self.adhesion {
                    if self.move_inside_shift(i, k).is_some() {
                        n = n.action(
                            move_inside_label(&self.adhesion, &plate.objects, k),
                            Action::MoveInside {
                                plate: plate.id,
                                object: plate.objects[k].id,
                            },
                        );
                    }
                }
            }
            // Absolute extrusion: the setting that fixes it, one click away.
            if e.message
                .contains(mica_core::safety::ABSOLUTE_E_UNSUPPORTED)
            {
                let fix = crate::checks::relative_e_fix();
                n = n.action(fix.label.clone(), Action::Fix(fix));
            }
            if !self.prefs.auto_slice {
                n = n.action("Slice again", Action::Slice);
            }
            wanted.push(n);
        }
        let plate = self.plates.current();
        let id = plate.id;
        // What the slice itself warned about (a collision, an open contour), while it shows.
        for w in plate.finished.iter().flat_map(|f| &f.warnings) {
            wanted.push(
                Notification::new(Level::Warning, short_warning(w), w.clone())
                    .keyed(format!("slice-warning:{w}"))
                    .on_plate(id),
            );
        }
        let failed = plate.slice_error.is_some();
        for w in &self.warnings {
            // The failed slice's error already says it, with the same Move inside.
            if failed
                && self.adhesion.iter().any(|(k, ov)| {
                    *w == mica_core::adhesion_check::explain(&plate.objects[*k].name, ov, true)
                })
            {
                continue;
            }
            let mut n = Notification::new(Level::Warning, w.clone(), w.clone())
                .keyed(format!("warning:{w}"))
                .on_plate(id);
            // The object a warning is about: its skirt or brim, or the object itself outside
            // the printable volume, can be moved inside.
            let outside = |o: &crate::scene::Object| is_outside_warning(w, &o.name);
            let about = plate
                .objects
                .iter()
                .enumerate()
                .find(|(k, o)| {
                    self.adhesion.iter().any(|(j, ov)| {
                        j == k && *w == mica_core::adhesion_check::explain(&o.name, ov, true)
                    }) || outside(o)
                })
                .map(|(k, _)| k);
            if let Some(k) = about
                && self.move_inside_shift(current, k).is_some()
            {
                n = n.action(
                    "Move inside",
                    Action::MoveInside {
                        plate: id,
                        object: plate.objects[k].id,
                    },
                );
            }
            // Larger than the volume: moving cannot help, scaling can.
            if let Some(k) = about
                && outside(&plate.objects[k])
                && crate::scene::outside::fit_scale(&plate.objects[k], &self.bed).is_some()
            {
                n = n.action(
                    "Scale to fit",
                    Action::ScaleToFit {
                        plate: id,
                        object: plate.objects[k].id,
                    },
                );
            }
            if w.ends_with(" overlap") || about.is_some_and(|k| outside(&plate.objects[k])) {
                n = n.action("Arrange", Action::Arrange);
            }
            n.title = short_warning(w);
            wanted.push(n);
        }
        for g in &self.guards {
            if self.dismissed_guards.contains(&g.message) {
                continue;
            }
            // The failed slice already says it, with the same fix.
            if plate
                .slice_error
                .as_ref()
                .is_some_and(|e| g.message.starts_with(&e.message))
            {
                continue;
            }
            let mut n = Notification::new(Level::Warning, g.message.clone(), g.message.clone())
                .keyed(format!("guard:{}", g.message))
                .on_plate(id);
            if let Some(f) = &g.fix {
                n = n.action(f.label.clone(), Action::Fix(f.clone()));
            }
            wanted.push(n);
        }
        wanted.extend(self.feature_off_notifications());
        wanted.extend(self.update_notification());
        self.notifications.sync(wanted);
        self.notifications.expire();
    }

    /// Draws the notifications with the stack's bottom-right corner at `corner` (`plates`:
    /// the plate views are showing, so the current plate's notifications are shown) and runs
    /// the clicked actions.
    pub(crate) fn notifications_ui(&mut self, ctx: &egui::Context, corner: Pos2, plates: bool) {
        let plate = plates.then(|| self.plates.current().id);
        let actions = self.notifications.show(ctx, corner, plate);
        for (id, a) in actions {
            self.run_notification_action(id, a, ctx);
        }
        if self.notifications.any_fading() {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
    }

    pub(crate) fn run_notification_action(&mut self, id: u64, a: Action, ctx: &egui::Context) {
        match a {
            Action::MoveInside { plate, object } => {
                if let Some(i) = self.plates.index_of(plate)
                    && let Some(k) = self
                        .plates
                        .plate(i)
                        .objects
                        .iter()
                        .position(|o| o.id == object)
                {
                    self.move_inside(i, k);
                }
            }
            Action::ScaleToFit { plate, object } => {
                if let Some(i) = self.plates.index_of(plate)
                    && let Some(k) = self
                        .plates
                        .plate(i)
                        .objects
                        .iter()
                        .position(|o| o.id == object)
                {
                    self.scale_to_fit(i, k);
                }
            }
            Action::Arrange => self.arrange(),
            Action::Details => {}
            Action::OpenMachine(printer) => {
                self.machine_printer = Some(printer);
                self.machine = true;
                self.notifications.dismiss(id);
            }
            Action::Fix(f) => {
                for (key, raw) in f.settings {
                    // A printer setting (relative extrusion) is the printer's change.
                    let target = if crate::settings::on_printer_tab(&key) {
                        crate::overlays::Target::Printer
                    } else {
                        crate::overlays::Target::Print
                    };
                    self.apply_edit(Some(target), crate::panels::editor::Edit::Set { key, raw });
                }
            }
            Action::Slice => self.slice_current(ctx),
            Action::EnableFeature(f) => self.set_feature(f, true),
            Action::OpenPreferences(tab) => self.open_preferences(Some(tab)),
            Action::ReviewImports => {
                self.answer_import_offer(id);
                self.open_import_review();
            }
            Action::DismissImports => self.answer_import_offer(id),
            Action::Reveal(path) => {
                if let Err(e) = crate::library::reveal(&path) {
                    self.notify_error(
                        "Could not show the file",
                        format!("{}: {e}", path.display()),
                    );
                }
            }
            Action::UpdateNotes
            | Action::UpdateInstall
            | Action::UpdateLater
            | Action::UpdateCancel
            | Action::UpdateRestart
            | Action::UpdateRetry => self.run_update_action(&a, ctx),
        }
    }

    /// Turns a feature of Settings › Features on or off: its controls show or hide, and
    /// every plate slices again.
    pub(crate) fn set_feature(&mut self, f: crate::state::Feature, on: bool) {
        if self.prefs.features.on(f) == on {
            return;
        }
        self.prefs.features.set(f, on);
        self.prefs_dirty = true;
        self.settings_changed();
    }

    /// The features the project uses although they are turned off in Settings › Features.
    pub(crate) fn features_off_in_use(&self) -> Vec<crate::state::Feature> {
        use crate::state::Feature;
        let mut out = Vec::new();
        if !self.prefs.features.textures
            && let Some(c) = self.effective_config()
            && self
                .plates
                .plates
                .iter()
                .any(|p| crate::textures::objects_use_textures(&p.objects, c))
        {
            out.push(Feature::Textures);
        }
        if !self.prefs.features.color_mix && self.project_uses_mixes() {
            out.push(Feature::ColorMix);
        }
        if !self.prefs.features.brick_layers_enabled()
            && let Some(c) = self.effective_config()
            && self
                .plates
                .plates
                .iter()
                .any(|p| crate::brick_layers::objects_use_brick_layers(&p.objects, c))
        {
            out.push(Feature::BrickLayers);
        }
        out
    }

    /// A lasting notice for each feature the project uses while it is off, with a one-click
    /// Turn on (and a link to Settings › Features): the data stays in the project, the slice
    /// goes without it.
    fn feature_off_notifications(&self) -> Vec<Notification> {
        self.features_off_in_use()
            .into_iter()
            .map(|f| {
                if f == crate::state::Feature::BrickLayers {
                    // Hidden unless turned on by hand in mica.toml: no switch to offer.
                    return Notification::new(
                        Level::Warning,
                        "This project uses Brick layers, which are off in this Mica".to_string(),
                        "This project uses Brick layers, which are off in this Mica. Mica slices \
                         it without them and keeps them in the project when you save."
                            .to_string(),
                    )
                    .keyed(format!("feature-off:{}", f.key()));
                }
                Notification::new(
                    Level::Warning,
                    format!(
                        "This project uses {}, which is turned off in Settings › Features",
                        f.title()
                    ),
                    format!(
                        "This project uses {}, which is turned off in Settings › Features. \
                         Mica slices it without {} and keeps it in the project when you save.",
                        f.title(),
                        f.title()
                    ),
                )
                .keyed(format!("feature-off:{}", f.key()))
                .action("Turn on", Action::EnableFeature(f))
                .action(
                    "Settings",
                    Action::OpenPreferences(crate::panels::preferences::PrefsTab::Features),
                )
            })
            .collect()
    }

    /// The header's failed-slice read-out was clicked: show and highlight its notification.
    pub(crate) fn focus_slice_error(&mut self) {
        let plate = self.plates.current().id;
        self.notifications.focus(Some(plate), SLICE_ERROR);
        self.machine = false;
        self.library.shown = false;
    }

    /// The XY move (bed mm) that brings object `k` of plate `i` just inside the bed with its
    /// skirt and brim, or the object itself when it is outside; `None` when it fits already
    /// or cannot fit.
    pub(crate) fn move_inside_shift(&self, i: usize, k: usize) -> Option<[f32; 2]> {
        let plate = self.plates.plate(i);
        let o = plate.objects.get(k)?;
        let overrun = if i == self.plates.current {
            self.adhesion
                .iter()
                .find(|(j, _)| *j == k)
                .map(|(_, o)| o.clone())
        } else {
            self.adhesion_overruns(i)
                .into_iter()
                .find(|(j, _)| *j == k)
                .map(|(_, o)| o)
        };
        if let Some(overrun) = overrun {
            return overrun.fix.map(|f| [f[0] as f32, f[1] as f32]);
        }
        let (min, max) = o.bounds();
        let axis = |lo: f32, hi: f32, bed_lo: f32, bed_hi: f32| -> Option<f32> {
            let (left, right) = (bed_lo - lo, hi - bed_hi);
            if left > 0.01 && right > 0.01 || hi - lo > bed_hi - bed_lo {
                None
            } else if left > 0.01 {
                Some(left + 0.01)
            } else if right > 0.01 {
                Some(-(right + 0.01))
            } else {
                Some(0.)
            }
        };
        let dx = axis(min.x, max.x, self.bed.min.x, self.bed.max.x)?;
        let dy = axis(min.y, max.y, self.bed.min.y, self.bed.max.y)?;
        (dx != 0. || dy != 0.).then_some([dx, dy])
    }

    /// "Move inside": moves object `k` of plate `i` just enough that it and its skirt and
    /// brim fit on the bed, as one undo step. Never done without being asked.
    pub(crate) fn move_inside(&mut self, i: usize, k: usize) -> bool {
        if self.plates.plate(i).locked {
            self.status = "This plate is locked; unlock it from its plate label".into();
            return false;
        }
        let Some([dx, dy]) = self.move_inside_shift(i, k) else {
            return false;
        };
        if i != self.plates.current {
            self.select_plate(i);
        }
        self.record("Move inside");
        let o = &mut self.plates.current_mut().objects[k];
        o.position.x += dx;
        o.position.y += dy;
        let name = o.name.clone();
        self.changed();
        self.notify_info(format!(
            "Moved {name} {}",
            mica_core::adhesion_check::shift_text([f64::from(dx), f64::from(dy)])
        ));
        true
    }
}

impl App {
    /// "Scale to fit": scales object `k` of plate `i` down uniformly until it fits the
    /// printable volume, then centres it on the bed and drops it onto it, as one undo step
    /// (PrusaSlicer's `Selection::scale_to_fit_print_volume`, `scene::outside::fit_scale`).
    /// Never done without being asked; refused on a locked plate.
    pub(crate) fn scale_to_fit(&mut self, i: usize, k: usize) -> bool {
        if self.plates.plate(i).locked {
            self.status = "This plate is locked; unlock it from its plate label".into();
            return false;
        }
        let Some(s) = self
            .plates
            .plate(i)
            .objects
            .get(k)
            .and_then(|o| crate::scene::outside::fit_scale(o, &self.bed))
        else {
            return false;
        };
        if i != self.plates.current {
            self.select_plate(i);
        }
        self.record("Scale to fit");
        let bed_centre = ((self.bed.min + self.bed.max) * 0.5).truncate();
        let o = &mut self.plates.current_mut().objects[k];
        o.apply(crate::scene::Delta::Scale(glam::Vec3::splat(s)));
        let (min, max) = o.bounds();
        let shift = bed_centre - ((min + max) * 0.5).truncate();
        o.position += shift.extend(0.);
        o.drop_to_bed();
        let name = o.name.clone();
        self.changed();
        self.notify_info(format!("Scaled {name} to {:.1} % to fit", s * 100.));
        true
    }
}

/// Whether plate warning `w` is the outside-the-volume warning of the object named `name`
/// (`scene::outside::describe`).
fn is_outside_warning(w: &str, name: &str) -> bool {
    w.strip_prefix(name).is_some_and(|rest| {
        rest.starts_with(" sticks out ")
            || rest.starts_with(" reaches into an excluded area of the bed")
    })
}

/// The key of a plate's failed-slice notification.
pub const SLICE_ERROR: &str = "slice-error";

/// "Move Cube inside" when several objects can be moved, else "Move inside".
fn move_inside_label(
    adhesion: &[(usize, mica_core::adhesion_check::Overrun)],
    objects: &[crate::scene::Object],
    k: usize,
) -> String {
    if adhesion.len() > 1 {
        format!("Move {} inside", objects[k].name)
    } else {
        "Move inside".into()
    }
}

/// A short title for a placement warning: "The brim of Cube doesn't fit on the bed".
fn short_warning(w: &str) -> String {
    for (prefix, kind) in [("The brim of ", "brim"), ("The skirt around ", "skirt")] {
        if let Some(rest) = w.strip_prefix(prefix)
            && let Some((name, _)) = rest.split_once(" would go ")
        {
            return format!("The {kind} of {name} doesn't fit on the bed");
        }
    }
    // "Bracket sticks out 12.4 mm past the back edge…" (`scene::outside::describe`).
    if let Some((name, _)) = w.split_once(" sticks out ") {
        return format!("{name} is outside the printable area");
    }
    if let Some(name) = w.strip_suffix(" reaches into an excluded area of the bed") {
        return format!("{name} is in an excluded area of the bed");
    }
    match w.split_once("; ") {
        Some((head, _)) => head.to_string(),
        None => w.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_notifications_keep_identity_and_dismissals_until_resolved() {
        let mut n = Notifications::default();
        let warn = || {
            Notification::new(
                Level::Warning,
                "Cube sticks out 2.0 mm past the left edge",
                "",
            )
            .keyed("w")
            .on_plate(1)
        };
        n.sync(vec![warn()]);
        let id = n.visible(Some(1))[0].id;
        n.sync(vec![warn()]);
        assert_eq!(
            n.visible(Some(1))[0].id,
            id,
            "the same condition keeps its card"
        );
        assert!(
            n.visible(Some(2)).is_empty(),
            "another plate does not show it"
        );
        n.dismiss(id);
        n.sync(vec![warn()]);
        assert!(
            n.visible(Some(1)).is_empty(),
            "dismissed while the condition holds"
        );
        n.sync(vec![]);
        n.sync(vec![warn()]);
        assert_eq!(
            n.visible(Some(1)).len(),
            1,
            "it comes back once it happens again"
        );
    }

    #[test]
    fn errors_stay_and_information_fades() {
        let mut n = Notifications::default();
        n.push(Notification::new(
            Level::Error,
            "Saving failed",
            "disk full",
        ));
        let mut info = Notification::new(Level::Info, "Exported", "Exported");
        info.born = Instant::now() - INFO_LIFE - Duration::from_secs(1);
        n.push(info);
        n.expire();
        let shown = n.visible(None);
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].level, Level::Error);
    }
}
