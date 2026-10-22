// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The Welcome window, the first thing a new user sees (and Help › Welcome to Mica…): what
//! Mica is, that it is alpha (pre-release) software, and three ways to start: add a printer,
//! import profiles from PrusaSlicer / OrcaSlicer (only when there are some), or put a Benchy on
//! the plate. Closing it (Get started, Esc, one of the starts, or Take a quick tour, which
//! starts the tour in `tour.rs`) marks it seen in mica.toml.
//!
//! At launch, [`App::launch_windows`] picks at most one window: Welcome on the first launch,
//! else What's new once per new version (`whats_new.rs`). Only the real window calls it; tests,
//! screenshots and the monkey test open these windows only when asked.

use super::{whats_new, widgets};
use crate::app::App;
use crate::builtin::AddAs;
use crate::theme;
use eframe::egui::{self, Align, Color32, Key, Layout, Pos2, Rect, RichText, Sense, Stroke, Vec2};
use mica_core::builtin_models::BuiltinModel;

const WINDOW_W: f32 = 580.;
const PAD: f32 = 28.;

/// The alpha notice (the README says the same at more length).
pub const ALPHA_NOTICE: &str = "Mica is alpha (pre-release) software. It's tested against \
     PrusaSlicer and OrcaSlicer and on real printers, but check the preview before printing and \
     stay near your printer for the first layers. Provided as is, without warranty.";

/// Windows the command line asks for at launch (`mica-gui --welcome`, `--whats-new`; a
/// development build shows What's new only this way).
#[derive(Clone, Copy, Debug, Default)]
pub struct Launch {
    pub welcome: bool,
    pub whats_new: bool,
}

/// The open Welcome window.
#[derive(Clone, Debug, Default)]
pub struct Welcome {
    /// "Found 2 printer profiles in PrusaSlicer" when other slicers have presets Mica does
    /// not have yet (the Import card shows only then); read once when the window opens.
    pub importable: Option<String>,
}

/// A way to start, from the window's cards.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Start {
    AddPrinter,
    Import,
    Benchy,
}

impl App {
    pub(crate) fn open_welcome(&mut self) {
        let found = self.profiles.importable();
        if !found.is_empty() {
            // The Import card replaces the startup notice about other slicers' presets (not
            // answered: it comes back next launch if nothing was imported).
            let offer: Vec<u64> = self
                .notifications
                .list
                .iter()
                .filter(|n| {
                    n.actions
                        .iter()
                        .any(|(_, a)| *a == crate::notifications::Action::ReviewImports)
                })
                .map(|n| n.id)
                .collect();
            for id in offer {
                self.notifications.dismiss(id);
            }
        }
        self.dialogs.welcome = Some(Welcome {
            importable: (!found.is_empty()).then(|| crate::import::found_phrase(&found)),
        });
    }

    /// Closes the window; it never opens by itself again.
    pub(crate) fn close_welcome(&mut self) {
        self.dialogs.welcome = None;
        if !self.prefs.welcome_seen {
            self.prefs.welcome_seen = true;
            self.prefs_dirty = true;
        }
    }

    /// One of the cards: closes the window, then opens what it names.
    pub(crate) fn welcome_start(&mut self, start: Start) {
        self.close_welcome();
        match start {
            Start::AddPrinter => {
                // The model gallery (tested printers first), not the network search.
                self.open_add_printer();
                self.add_printer.screen = super::add_printer::Screen::Gallery;
            }
            Start::Import => self.open_import_review(),
            Start::Benchy => self.add_builtin(BuiltinModel::Benchy, AddAs::Object),
        }
    }

    /// At launch: Welcome on the first launch, else What's new once after an update. `existed`:
    /// mica.toml was there before this launch. `dev`: a development build, which shows What's
    /// new only when `force_whats_new` asks. Remembers the running version either way.
    pub(crate) fn launch_windows(
        &mut self,
        existed: bool,
        dev: bool,
        force_welcome: bool,
        force_whats_new: bool,
    ) {
        let current = whats_new::VERSION;
        // Settings from before the Welcome window (no version remembered) belong to someone
        // who has used Mica: no Welcome, but What's new for this version.
        let earlier_user = existed && self.prefs.last_version.is_empty();
        let last = std::mem::replace(&mut self.prefs.last_version, current.to_string());
        let updated = last != current;
        if updated {
            self.prefs_dirty = true;
        }
        if force_welcome || (!self.prefs.welcome_seen && !earlier_user) {
            self.open_welcome();
            if !force_whats_new {
                return;
            }
        }
        if earlier_user && !self.prefs.welcome_seen {
            self.prefs.welcome_seen = true;
            self.prefs_dirty = true;
        }
        if force_whats_new || (updated && !dev && whats_new::notes().is_some()) {
            self.open_whats_new();
        }
    }

    pub(crate) fn welcome_window(&mut self, ctx: &egui::Context) {
        let Some(state) = self.dialogs.welcome.clone() else {
            return;
        };
        let p = theme::of_ctx(ctx);
        let mut close = ctx.input(|i| i.key_pressed(Key::Escape))
            && !widgets::field_editing(ctx)
            && !ctx.egui_wants_keyboard_input();
        let mut start = None;
        let mut tour = false;
        egui::Window::new("Welcome to Mica Slicer")
            .id(egui::Id::unique("mica-welcome"))
            .title_bar(false)
            .resizable(false)
            .collapsible(false)
            .default_pos(ctx.content_rect().center_top() + egui::vec2(-WINDOW_W / 2., 70.))
            .frame(
                egui::Frame::window(&ctx.global_style())
                    .fill(p.bg)
                    .stroke(Stroke::new(1., p.border_soft))
                    .corner_radius(12)
                    .inner_margin(PAD as i8),
            )
            .show(ctx, |ui| {
                ui.set_width(WINDOW_W - 2. * PAD);
                header(ui);
                ui.add_space(16.);
                notice(ui);
                ui.add_space(18.);
                ui.label(
                    RichText::new("Start with")
                        .font(theme::semibold(12.))
                        .color(p.muted),
                );
                ui.add_space(6.);
                if card(
                    ui,
                    Card::Glyph(widgets::Glyph::Printer),
                    "Add your printer",
                    "Pick it from the printer gallery. Tested printers come first.",
                ) {
                    start = Some(Start::AddPrinter);
                }
                if self.profiles.prusa_missing {
                    ui.add_space(6.);
                    self.prusa_download_ui(ui);
                }
                if let Some(found) = &state.importable {
                    ui.add_space(8.);
                    if card(
                        ui,
                        Card::Glyph(widgets::Glyph::Sync),
                        "Import from PrusaSlicer / OrcaSlicer",
                        &format!("{found}. You choose what Mica copies."),
                    ) {
                        start = Some(Start::Import);
                    }
                }
                ui.add_space(8.);
                if card(
                    ui,
                    Card::Benchy,
                    "Try a Benchy",
                    "Put the 3DBenchy test model on the plate and slice it.",
                ) {
                    start = Some(Start::Benchy);
                }
                ui.add_space(20.);
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Help › Welcome to Mica… opens this again.")
                            .font(theme::sans(11.5))
                            .color(p.muted),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if widgets::primary_button(ui, "Get started", true).clicked() {
                            close = true;
                        }
                        if widgets::button(ui, "Take a quick tour", true)
                            .on_hover_text(
                                "A short walk through the main window, on a sample plate",
                            )
                            .clicked()
                        {
                            tour = true;
                        }
                    });
                });
            });
        if tour {
            self.start_tour(ctx);
        } else if let Some(s) = start {
            self.welcome_start(s);
        } else if close {
            self.close_welcome();
        }
    }
}

/// The icon, the title with its Alpha badge, and one line on what Mica is.
fn header(ui: &mut egui::Ui) {
    let p = theme::of(ui);
    ui.horizontal(|ui| {
        if let Some(t) = super::about::icon(ui.ctx()) {
            ui.add(egui::Image::new(&t).fit_to_exact_size(Vec2::splat(56.)));
        }
        ui.add_space(10.);
        ui.vertical(|ui| {
            ui.add_space(4.);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Welcome to Mica Slicer")
                        .font(theme::semibold(22.))
                        .color(p.text),
                );
                ui.add_space(4.);
                widgets::status_badge(ui, "Alpha (pre-release)");
            });
            ui.label(
                RichText::new(
                    "A fast, native 3D-printing slicer that slices like PrusaSlicer and \
                     OrcaSlicer, so your profiles print the way you know.",
                )
                .font(theme::sans(13.))
                .color(p.label),
            );
        });
    });
}

/// The highlighted alpha notice.
fn notice(ui: &mut egui::Ui) {
    let p = theme::of(ui);
    egui::Frame::new()
        .fill(p.warn_surface)
        .stroke(Stroke::new(1., p.warn_border))
        .corner_radius(8.)
        .inner_margin(egui::Margin::symmetric(14, 11))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(
                RichText::new(ALPHA_NOTICE)
                    .font(theme::sans(12.5))
                    .color(p.warn_text),
            );
        });
}

enum Card {
    Glyph(widgets::Glyph),
    /// A small boat for the Benchy.
    Benchy,
}

/// A full-width start card: an icon tile, the title and a line under it. True when clicked.
fn card(ui: &mut egui::Ui, icon: Card, title: &str, text: &str) -> bool {
    let p = theme::of(ui);
    let w = ui.available_width();
    let text_w = w - 16. - 36. - 12. - 16. - 20.;
    let galley = ui
        .painter()
        .layout(text.to_string(), theme::sans(12.), p.muted, text_w);
    let h = (20. + 4. + galley.size().y + 24.).max(64.);
    let (rect, response) = ui.allocate_exact_size(Vec2::new(w, h), Sense::click());
    let hovered = response.hovered();
    if hovered {
        ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
    }
    let painter = ui.painter();
    painter.rect_filled(rect, 10., if hovered { p.hover } else { p.raised });
    painter.rect_stroke(
        rect,
        10.,
        Stroke::new(1., if hovered { p.accent } else { p.border_soft }),
        egui::StrokeKind::Inside,
    );
    let tile = Rect::from_min_size(
        Pos2::new(rect.left() + 16., rect.center().y - 18.),
        Vec2::splat(36.),
    );
    painter.rect_filled(tile, 8., p.accent_surface);
    match icon {
        Card::Glyph(g) => widgets::glyph(painter, tile.shrink(9.), g, p.accent_bright),
        Card::Benchy => boat(painter, tile.shrink(8.), p.accent_bright),
    }
    let x = tile.right() + 12.;
    let top = rect.top() + 12.;
    painter.text(
        Pos2::new(x, top),
        egui::Align2::LEFT_TOP,
        title,
        theme::semibold(14.),
        p.text,
    );
    painter.galley(Pos2::new(x, top + 24.), galley, p.muted);
    widgets::chevron(
        painter,
        Pos2::new(rect.right() - 20., rect.center().y),
        false,
        p.muted,
    );
    response.clicked()
}

/// A simple boat outline: hull, cabin and chimney.
fn boat(painter: &egui::Painter, r: Rect, color: Color32) {
    let s = Stroke::new(1.6, color);
    let at = |fx: f32, fy: f32| Pos2::new(r.left() + fx * r.width(), r.top() + fy * r.height());
    painter.add(egui::Shape::closed_line(
        vec![at(0.0, 0.62), at(1.0, 0.62), at(0.82, 0.9), at(0.16, 0.9)],
        s,
    ));
    painter.add(egui::Shape::closed_line(
        vec![at(0.3, 0.62), at(0.3, 0.36), at(0.68, 0.36), at(0.68, 0.62)],
        s,
    ));
    painter.line_segment([at(0.5, 0.36), at(0.5, 0.12)], s);
}
