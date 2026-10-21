// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! "My printers": a card per physical printer (product picture or live camera snapshot,
//! status badge, progress while printing, loaded spools, temperatures, Print here / Watch /
//! Retry / Settings) and per profile without a connection ("slice only"). Status comes from
//! the printers' monitors, which poll only while the cards are on screen (`poll_printers`);
//! camera snapshots refresh every few seconds only while the window is open.
//!
//! "Settings" opens the former window for one printer: add / edit / remove, the type
//! (PrusaLink, Klipper via Moonraker, Snapmaker U1), address, credentials (saved in the
//! macOS Keychain; `printers.toml` only gets a reference), the linked Mica profile, an
//! optional camera URL with its own login (password in the Keychain too), and "Test
//! connection" showing what the backend reports or a readable error.

use super::{
    add_printer::{self, Style, line},
    dashboard::Players,
    machine::{self, state_colour, state_word, warn_card},
    swatch,
    widgets::{self, Glyph},
};
use crate::app::App;
use crate::printhost::{
    Test,
    cameras::{CameraViews, Role},
    config::{FileHeader, Kind, Printer},
};
use crate::theme;
use eframe::egui::{self, Align2, Color32, Pos2, Rect, Sense, Stroke, Vec2};
use mica_printhost::{LoadedFilament, PrinterState};

const LIST_W: f32 = 240.;
const SIZE: Vec2 = add_printer::SIZE;
const PAD: f32 = 28.;
const GAP: f32 = 16.;
const IMAGE_H: f32 = 120.;
const CARD_H: f32 = 262.;

/// What a card's buttons asked for.
#[derive(Clone, Debug, PartialEq)]
enum CardAction {
    Settings(usize),
    Watch(usize),
    Retry(usize),
    PrintHere(usize),
    Connect(usize, String),
    Use(usize, String),
    Forget(usize, String),
}

/// A card's two action buttons at its bottom.
fn two_buttons(inner: Rect) -> (Rect, Rect) {
    let half = ((inner.width() - 8.) / 2.).floor();
    let y = inner.bottom() - 32.;
    (
        Rect::from_min_size(Pos2::new(inner.left(), y), Vec2::new(half, 32.)),
        Rect::from_min_size(Pos2::new(inner.right() - half, y), Vec2::new(half, 32.)),
    )
}

/// `just now`, `2 min ago`, `3 h ago`.
fn ago(seconds: u64) -> String {
    match seconds {
        0..=59 => "just now".into(),
        60..=3599 => format!("{} min ago", seconds / 60),
        _ => format!("{} h ago", seconds / 3600),
    }
}

/// Editor state of the window (not persisted).
#[derive(Default)]
pub struct Editor {
    /// The highlighted printer.
    pub selected: Option<usize>,
    /// The entry being edited: its index (`None` for a new one) and a working copy.
    pub editing: Option<(Option<usize>, Printer)>,
    /// The typed secret; `secret_touched` tells an untouched field from a cleared one.
    pub secret: String,
    pub secret_touched: bool,
    pub show_secret: bool,
    /// PrusaLink: log in with user name and password instead of an API key.
    pub login: bool,
    /// The typed camera password; `camera_secret_touched` as for `secret`.
    pub camera_secret: String,
    pub camera_secret_touched: bool,
    pub error: Option<String>,
    /// The cards, or one printer's settings (the list, details and form).
    pub view: View,
    /// The cards' camera snapshots (running only while the window shows them).
    pub cameras: CameraViews,
    /// The window was open last frame.
    pub was_open: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum View {
    #[default]
    Cards,
    Settings,
}

impl App {
    /// "My printers": a card per printer (status, job, spools, camera) and per profile that
    /// has no connection; "Settings" opens the list / details / form of one printer.
    pub(crate) fn my_printers_window(&mut self, ctx: &egui::Context) {
        if !self.dialogs.my_printers {
            if self.printers_editor.was_open {
                self.printers_editor.was_open = false;
                // No snapshots while the window is closed.
                self.printers_editor.cameras.clear();
            }
            return;
        }
        self.printers_editor.was_open = true;
        let p = theme::of_ctx(ctx);
        let mut keep = true;
        let mut test: Option<usize> = None;
        let mut remove: Option<usize> = None;
        self.printers_editor.cameras.begin_frame();
        self.add_printer.images.begin_frame();
        egui::Window::new("My printers")
            .id(egui::Id::unique("my-printers"))
            .title_bar(false)
            .resizable(false)
            .collapsible(false)
            .fixed_size(SIZE)
            .default_pos(ctx.content_rect().center() - SIZE / 2.)
            .frame(
                egui::Frame::window(&ctx.global_style())
                    .fill(p.bg)
                    .stroke(Stroke::new(1., p.border_soft))
                    .corner_radius(12)
                    .inner_margin(0),
            )
            .show(ctx, |ui| {
                let (rect, _) = ui.allocate_exact_size(SIZE, Sense::hover());
                let left = rect.left() + PAD;
                // Header: the title (or Back from a printer's settings), Add printer, close.
                let close = Rect::from_min_size(
                    Pos2::new(rect.right() - PAD - 32., rect.top() + 18.),
                    Vec2::splat(32.),
                );
                let cr = ui.interact(close, egui::Id::unique("my-printers-close"), Sense::click());
                ui.painter()
                    .rect_filled(close, 8., if cr.hovered() { p.hover } else { p.raised });
                widgets::glyph(ui.painter(), close.shrink(9.), Glyph::Close, p.muted_alt);
                if cr.on_hover_text("Close").clicked() {
                    keep = false;
                }
                match self.printers_editor.view {
                    View::Cards => {
                        ui.painter().text(
                            Pos2::new(left, close.center().y),
                            Align2::LEFT_CENTER,
                            "My printers",
                            theme::semibold(22.),
                            p.text,
                        );
                        let add = Rect::from_min_size(
                            Pos2::new(close.left() - 12. - 132., close.top() - 2.),
                            Vec2::new(132., 36.),
                        );
                        if add_printer::button_in(
                            ui,
                            add,
                            egui::Id::unique("my-printers-add"),
                            "+ Add printer",
                            Style::Primary,
                            true,
                        )
                        .clicked()
                        {
                            self.open_add_printer();
                        }
                        let body = Rect::from_min_max(
                            Pos2::new(left, rect.top() + 70.),
                            Pos2::new(rect.right() - PAD, rect.bottom() - 24.),
                        );
                        self.printer_cards(ui, body, &mut keep);
                    }
                    View::Settings => {
                        let g = ui.painter().layout_no_wrap(
                            "← My printers".into(),
                            theme::sans(13.),
                            p.muted_alt,
                        );
                        let back = Rect::from_min_size(
                            Pos2::new(left, close.center().y - g.size().y / 2.),
                            g.size(),
                        );
                        let br = ui.interact(
                            back.expand(4.),
                            egui::Id::unique("my-printers-back"),
                            Sense::click(),
                        );
                        ui.painter().galley(
                            back.min,
                            g,
                            if br.hovered() { p.text } else { p.muted_alt },
                        );
                        if br.hovered() {
                            ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
                        }
                        if br.clicked() {
                            self.printers_editor.view = View::Cards;
                            self.printers_editor.editing = None;
                            self.printers_editor.error = None;
                        }
                        ui.painter().text(
                            Pos2::new(back.right() + 16., close.center().y),
                            Align2::LEFT_CENTER,
                            "Printer settings",
                            theme::semibold(20.),
                            p.text,
                        );
                        let body = Rect::from_min_max(
                            Pos2::new(left, rect.top() + 66.),
                            Pos2::new(rect.right() - PAD, rect.bottom() - 20.),
                        );
                        let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(body));
                        self.printer_settings_view(&mut ui, &mut test, &mut remove);
                    }
                }
            });
        self.printers_editor.cameras.end_frame();
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) && !widgets::field_editing(ctx) {
            keep = false;
        }
        self.dialogs.my_printers = keep;
        if let Some(i) = test {
            let c = ctx.clone();
            self.printers.test(i, move || c.request_repaint());
        }
        if let Some(i) = remove {
            self.printers.remove(i);
            self.printers_editor.selected = None;
            self.printers_editor.editing = None;
            self.printers_editor.view = View::Cards;
        }
    }

    /// The former window: the printer list on the left, its details or the form.
    fn printer_settings_view(
        &mut self,
        ui: &mut egui::Ui,
        test: &mut Option<usize>,
        remove: &mut Option<usize>,
    ) {
        let p = theme::of(ui);
        ui.label(
            egui::RichText::new(format!(
                "Printers Mica can send to and watch. Keys and passwords go to the {}; \
                 printers.toml only keeps a reference.",
                self.printers.secrets.label()
            ))
            .font(theme::sans(12.))
            .color(p.muted),
        );
        if let Some(w) = &self.printers.warning {
            warn_card(ui, "The printer list could not be read", w);
        }
        ui.add_space(6.);
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 16.;
            ui.allocate_ui_with_layout(
                Vec2::new(LIST_W, ui.available_height()),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.spacing_mut().item_spacing.y = 6.;
                    self.printer_list(ui);
                },
            );
            let sep = ui.available_rect_before_wrap();
            ui.painter().line_segment(
                [sep.left_top(), sep.left_bottom()],
                Stroke::new(1., p.border),
            );
            ui.add_space(4.);
            ui.vertical(|ui| {
                ui.set_width(ui.available_width());
                egui::ScrollArea::vertical()
                    .id_salt("printer-settings")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 8.;
                        if self.printers_editor.editing.is_some() {
                            self.printer_form(ui, test);
                        } else {
                            self.printer_details(ui, test, remove);
                        }
                    });
            });
        });
    }

    /// The profiles in My printers that no connection links to: `(vendor index, preset)`.
    pub(crate) fn slice_only_profiles(&self) -> Vec<(usize, String)> {
        self.profiles
            .my_printers(&self.prefs)
            .into_iter()
            .filter(|(vi, name)| {
                let id = format!("{}/{name}", self.profiles.vendors[*vi].0);
                !self
                    .printers
                    .list
                    .iter()
                    .any(|p| p.profile.as_deref() == Some(id.as_str()))
            })
            .collect()
    }

    /// The model picture and variant summary ("HF 0.4", "4 tools · 0.4") of a profile.
    fn profile_look(
        &mut self,
        ctx: &egui::Context,
        vendor: usize,
        preset: &str,
    ) -> (Option<egui::TextureHandle>, Option<String>) {
        let (texture, summary, _) = self.profile_look_named(ctx, vendor, preset);
        (texture, summary)
    }

    /// `profile_look` with the model's name ("Prusa XL").
    fn profile_look_named(
        &mut self,
        ctx: &egui::Context,
        vendor: usize,
        preset: &str,
    ) -> (Option<egui::TextureHandle>, Option<String>, Option<String>) {
        let key = self
            .profiles
            .vendors
            .get(vendor)
            .map(|(k, _)| k.clone())
            .unwrap_or_default();
        let catalog = self.catalog();
        let Some((mi, choice)) = catalog.locate(&key, preset) else {
            return (None, None, None);
        };
        let m = catalog.models[mi].clone();
        let v = m
            .resolve(&choice)
            .cloned()
            .unwrap_or_else(|| m.default_variant().clone());
        let summary = add_printer::choice_summary(&m, &choice).replace(" nozzle", "");
        (
            self.add_printer
                .images
                .get(ctx, &m.asset_dir, v.image.as_deref()),
            Some(summary),
            Some(m.name.clone()),
        )
    }

    /// `(vendor, preset)` of the profile a printer that names no profile most likely uses,
    /// from the model it reports (empty strings: none fits).
    fn profile_for_model(
        &mut self,
        kind: Kind,
        model: Option<&str>,
        tools: Option<usize>,
    ) -> (String, String) {
        let Some(model) = model else {
            return Default::default();
        };
        let catalog = self.catalog();
        let Some((mi, choice)) =
            catalog.match_printer(add_printer::family(kind), model, tools, None)
        else {
            return Default::default();
        };
        let m = &catalog.models[mi];
        match m.resolve(&choice) {
            Some(v) => (v.vendor.clone(), v.preset.clone()),
            None => Default::default(),
        }
    }

    fn printer_cards(&mut self, ui: &mut egui::Ui, body: Rect, keep: &mut bool) {
        let p = theme::of(ui);
        let footer_h = 44.;
        let list = Rect::from_min_max(
            body.min,
            Pos2::new(body.right(), body.bottom() - footer_h - 16.),
        );
        let connected = self.printers.list.len();
        let loose = self.slice_only_profiles();
        let n = connected + loose.len();
        if let Some(w) = &self.printers.warning {
            line(
                ui,
                Pos2::new(body.left(), body.top() - 18.),
                &format!("The printer list could not be read: {w}"),
                theme::sans(12.),
                p.warn_text,
                body.width(),
            );
        }
        if n == 0 {
            ui.painter().text(
                Pos2::new(list.center().x, list.top() + 80.),
                Align2::CENTER_TOP,
                "No printers yet. Add one: Mica finds the printers on your network.",
                theme::sans(13.),
                p.label,
            );
        }
        let mut action: Option<CardAction> = None;
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(list));
        egui::ScrollArea::vertical()
            .id_salt("my-printers-cards")
            .auto_shrink([false, false])
            .show(&mut child, |ui| {
                let cols = 3usize;
                let w = ((list.width() - GAP * (cols as f32 - 1.)) / cols as f32).floor();
                let rows = n.div_ceil(cols);
                let (area, _) = ui.allocate_exact_size(
                    Vec2::new(list.width(), (rows as f32 * (CARD_H + GAP) - GAP).max(0.)),
                    Sense::hover(),
                );
                for k in 0..n {
                    let (row, col) = (k / cols, k % cols);
                    let card = Rect::from_min_size(
                        Pos2::new(
                            area.left() + col as f32 * (w + GAP),
                            area.top() + row as f32 * (CARD_H + GAP),
                        ),
                        Vec2::new(w, CARD_H),
                    );
                    if !ui.is_rect_visible(card) {
                        continue;
                    }
                    let a = if k < connected {
                        self.connected_card(ui, card, k)
                    } else {
                        let (vi, name) = loose[k - connected].clone();
                        self.slice_only_card(ui, card, vi, &name)
                    };
                    if a.is_some() {
                        action = a;
                    }
                }
            });
        // Footer.
        let foot = Rect::from_min_size(
            Pos2::new(body.left(), body.bottom() - footer_h),
            Vec2::new(body.width(), footer_h),
        );
        ui.painter().rect_filled(foot, 10., p.panel);
        ui.painter().text(
            Pos2::new(foot.left() + 16., foot.center().y),
            Align2::LEFT_CENTER,
            "Profiles without a connection (slice only) show here too, without a status.",
            theme::sans(13.),
            p.muted_alt,
        );
        ui.painter().text(
            Pos2::new(foot.right() - 16., foot.center().y),
            Align2::RIGHT_CENTER,
            format!("Keys stay in the {}.", self.printers.secrets.label()),
            theme::sans(13.),
            p.muted_alt,
        );
        self.card_action(action, keep);
    }

    fn card_action(&mut self, action: Option<CardAction>, keep: &mut bool) {
        match action {
            None => {}
            Some(CardAction::Settings(i)) => {
                self.printers_editor.view = View::Settings;
                self.printers_editor.selected = Some(i);
                self.printers_editor.editing = None;
                self.printers_editor.error = None;
            }
            Some(CardAction::Watch(i)) => {
                if let Some(id) = self.printers.id_at(i) {
                    self.machine = true;
                    self.machine_printer = Some(id);
                    self.machine_view.confirm_cancel = false;
                    *keep = false;
                }
            }
            Some(CardAction::Retry(i)) => {
                // Ask now instead of waiting out the back-off.
                if let Some(w) = self
                    .printers
                    .list
                    .get(i)
                    .and_then(|p| self.printers.watchers.get(&p.name))
                {
                    w.poke();
                }
            }
            Some(CardAction::PrintHere(i)) => {
                self.open_print_dialog();
                let target = self.printers.target(i);
                if let Some(d) = &mut self.print_job {
                    d.choose(target);
                }
                *keep = false;
            }
            Some(CardAction::Connect(vi, preset)) => {
                let key = self
                    .profiles
                    .vendors
                    .get(vi)
                    .map(|(k, _)| k.clone())
                    .unwrap_or_default();
                if let Some((mi, choice)) = self.catalog().locate(&key, &preset) {
                    self.open_add_printer();
                    self.begin_setup(mi, Some(choice), None, add_printer::Screen::Gallery);
                }
            }
            Some(CardAction::Use(vi, preset)) => {
                self.switch_printer(vi, &preset);
            }
            Some(CardAction::Forget(vi, preset)) => {
                let vendor = self.profiles.vendors[vi].0.clone();
                self.prefs.remove_printer(&vendor, &preset);
                self.prefs_dirty = true;
            }
        }
    }

    /// The image well at the top of a card with its status badge: the camera's snapshot
    /// (`camera`) or the product picture. True when clicked (a connected printer's opens the
    /// Machine view).
    fn card_top(
        &mut self,
        ui: &mut egui::Ui,
        card: Rect,
        index: Option<usize>,
        texture: Option<&egui::TextureHandle>,
        camera: bool,
        badge: (&str, Color32, Color32),
    ) -> bool {
        let p = theme::of(ui);
        let top = Rect::from_min_size(card.min, Vec2::new(card.width(), IMAGE_H));
        let rounding = egui::CornerRadius {
            nw: 12,
            ne: 12,
            sw: 0,
            se: 0,
        };
        ui.painter().rect_filled(top, rounding, p.raised);
        let mut clicked = false;
        if let Some(i) = index {
            let r = ui.interact(top, egui::Id::unique(("card-top", i)), Sense::click());
            if r.hovered() {
                ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
            }
            clicked = r
                .on_hover_text("Open this printer in the Machine view")
                .clicked();
        }
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(top));
        child.set_clip_rect(top.shrink(1.).intersect(ui.clip_rect()));
        match index {
            Some(i) if camera => {
                self.camera_picture_in(&mut child, top, i, Role::Thumbnail, Players::MyPrinters);
            }
            _ => add_printer::picture(&child, top, 0., texture),
        }
        let (text, fg, bg) = badge;
        let g = ui
            .painter()
            .layout_no_wrap(text.to_string(), theme::medium(11.), fg);
        let at = Pos2::new(top.right() - 10. - g.size().x - 16., top.top() + 10.);
        add_printer::pill(ui, at, text, fg, bg, false, false);
        clicked
    }

    fn connected_card(&mut self, ui: &mut egui::Ui, card: Rect, i: usize) -> Option<CardAction> {
        let p = theme::of(ui);
        let printer = self.printers.list.get(i)?.clone();
        ui.painter().rect_filled(card, 12., p.panel);
        let live = self.printers.live(i);
        let status = live.and_then(|l| l.status.clone());
        let failed = live.is_some_and(|l| l.failed());
        let state = status.as_ref().map(|s| s.state);
        let known = live.is_some() && (state.is_some() || failed);
        let online = known && !failed;
        let has_camera = printer.camera.is_some()
            || live
                .and_then(|l| l.cameras.as_ref())
                .is_some_and(|c| !c.is_empty());
        let filaments: Vec<LoadedFilament> = live
            .filter(|l| l.filaments_known)
            .map(|l| l.filaments.clone())
            .unwrap_or_default();
        let tools = live
            .and_then(|l| l.info.as_ref())
            .and_then(|i| i.tools.as_ref())
            .map(Vec::len);
        let updated = live.and_then(|l| l.updated);
        let why = live.and_then(|l| {
            l.unavailable.clone().or_else(|| {
                l.error.as_ref().map(|e| {
                    crate::printhost::explain::explain(
                        e,
                        printer.kind,
                        &printer.name,
                        &printer.address,
                    )
                    .headline
                })
            })
        });
        let (vendor, preset) = printer
            .profile
            .as_deref()
            .and_then(|s| s.split_once('/'))
            .map(|(v, n)| (v.to_string(), n.to_string()))
            .unwrap_or_default();
        // The linked profile's picture; without a link, the model the printer reports.
        let reported = live
            .and_then(|l| l.info.as_ref())
            .and_then(|i| i.model.clone())
            .or_else(|| printer.model.clone());
        let (vendor, preset) = match self.profiles.vendors.iter().position(|(n, _)| *n == vendor) {
            Some(_) => (vendor, preset),
            None => self.profile_for_model(printer.kind, reported.as_deref(), tools),
        };
        let (texture, summary) = match self.profiles.vendors.iter().position(|(n, _)| *n == vendor)
        {
            Some(vi) => self.profile_look(ui.ctx(), vi, &preset),
            None => (None, None),
        };
        let (badge, fg, bg) = if printer.kind == Kind::PrusaConnect {
            ("Send via Connect".to_string(), p.muted_alt, p.panel)
        } else if !known {
            ("Connecting…".to_string(), p.muted_alt, p.panel)
        } else {
            let (fg, bg) = add_printer::state_colours(p, state, failed);
            let text = if failed {
                "Offline".to_string()
            } else {
                add_printer::state_text(state, status.as_ref().and_then(|s| s.progress), "Ready")
            };
            (text, fg, if failed { p.panel } else { bg })
        };
        let open = self.card_top(
            ui,
            card,
            Some(i),
            texture.as_ref(),
            has_camera && online,
            (&badge, fg, bg),
        );
        let inner = Rect::from_min_max(
            Pos2::new(card.left() + 14., card.top() + IMAGE_H + 14.),
            card.max - Vec2::splat(14.),
        );
        let mut y = inner.top();
        let printing =
            online && matches!(state, Some(PrinterState::Printing | PrinterState::Paused));
        let right_text = if printing {
            status
                .as_ref()
                .and_then(|s| s.remaining)
                .map(|r| format!("{} left", super::duration(r)))
                .unwrap_or_default()
        } else {
            summary.clone().unwrap_or_default()
        };
        let rg = ui
            .painter()
            .layout_no_wrap(right_text, theme::sans(12.), p.muted_alt);
        let rw = rg.size().x;
        ui.painter()
            .galley(Pos2::new(inner.right() - rw, y + 3.), rg, p.muted_alt);
        line(
            ui,
            Pos2::new(inner.left(), y),
            &printer.name,
            theme::semibold(15.),
            p.text,
            inner.width() - rw - 10.,
        );
        y += 26.;
        if printing {
            let fraction = status.as_ref().and_then(|s| s.progress).unwrap_or(0.) as f32 / 100.;
            machine::bar(
                ui,
                Rect::from_min_size(Pos2::new(inner.left(), y), Vec2::new(inner.width(), 4.)),
                fraction,
                if state == Some(PrinterState::Paused) {
                    p.muted_alt
                } else {
                    theme::WARNING
                },
            );
            y += 12.;
        }
        if online {
            if !filaments.is_empty() && (printing || filaments.len() > 1) {
                let mut x = inner.left();
                for f in &filaments {
                    let r = Rect::from_min_size(Pos2::new(x, y + 1.), Vec2::splat(14.));
                    match f
                        .colour
                        .as_deref()
                        .and_then(crate::printhost::explain::parse_colour)
                    {
                        Some(c) => {
                            ui.painter()
                                .rect_filled(r, 4., Color32::from_rgb(c[0], c[1], c[2]));
                        }
                        None => {
                            ui.painter().rect_filled(r, 4., p.raised);
                        }
                    }
                    ui.painter().rect_stroke(
                        r,
                        4.,
                        Stroke::new(1., p.border_raised),
                        egui::StrokeKind::Inside,
                    );
                    x = r.right() + 6.;
                }
                let words = match (tools, &summary) {
                    (Some(t), Some(s)) if t > 1 && !s.contains("tools") => {
                        format!("{t} tools · {s}")
                    }
                    (_, Some(s)) => s.clone(),
                    _ => String::new(),
                };
                line(
                    ui,
                    Pos2::new(x + 4., y + 1.),
                    &words,
                    theme::sans(12.),
                    p.muted_alt,
                    inner.right() - x - 4.,
                );
                y += 24.;
            }
            if !printing {
                let mut parts = vec![];
                if let Some(s) = &status {
                    if let Some(b) = s.bed.as_ref().and_then(|b| b.current) {
                        parts.push(format!("Bed {b:.0} °C"));
                    }
                    if s.hotends.len() == 1
                        && let Some(n) = s.hotends[0].temperature.current
                    {
                        parts.push(format!("Nozzle {n:.0} °C"));
                    }
                }
                if filaments.len() == 1
                    && let Some(m) = &filaments[0].material
                {
                    parts.push(format!("{m} loaded"));
                }
                line(
                    ui,
                    Pos2::new(inner.left(), y),
                    &parts.join(" · "),
                    theme::sans(13.),
                    p.muted_alt,
                    inner.width(),
                );
            } else if let Some(job) = status.as_ref().and_then(|s| s.job_name.clone()) {
                line(
                    ui,
                    Pos2::new(inner.left(), y),
                    &job,
                    theme::sans(12.),
                    p.muted_alt,
                    inner.width(),
                );
            }
        } else if failed {
            let text = match updated {
                Some(t) => format!(
                    "No answer {} at {}",
                    ago(t.elapsed().as_secs()),
                    printer.address
                ),
                None => format!("No answer at {}", printer.address),
            };
            line(
                ui,
                Pos2::new(inner.left(), y),
                &text,
                theme::sans(13.),
                p.muted_alt,
                inner.width(),
            );
            if let Some(w) = why {
                line(
                    ui,
                    Pos2::new(inner.left(), y + 20.),
                    &w,
                    theme::sans(12.),
                    p.warn_text,
                    inner.width(),
                );
            }
        } else {
            line(
                ui,
                Pos2::new(inner.left(), y),
                &if printer.kind == Kind::PrusaConnect {
                    "Status available in Prusa Connect".into()
                } else {
                    format!("Asking {}…", printer.address)
                },
                theme::sans(13.),
                p.muted,
                inner.width(),
            );
        }
        // Actions.
        let (a, b) = two_buttons(inner);
        let mut out = None;
        if failed {
            if add_printer::button_in(
                ui,
                a,
                egui::Id::unique(("card-retry", i)),
                "Retry",
                Style::Secondary,
                true,
            )
            .on_hover_text("Ask the printer again now")
            .clicked()
            {
                out = Some(CardAction::Retry(i));
            }
        } else if printing {
            if add_printer::button_in(
                ui,
                a,
                egui::Id::unique(("card-watch", i)),
                "Watch",
                Style::Secondary,
                true,
            )
            .on_hover_text("Open the Machine view: camera, temperatures, pause and cancel")
            .clicked()
            {
                out = Some(CardAction::Watch(i));
            }
        } else {
            let sliced = self.finished().is_some();
            let r = add_printer::button_in(
                ui,
                a,
                egui::Id::unique(("card-print", i)),
                if printer.kind == Kind::PrusaConnect {
                    "Send via Connect"
                } else {
                    "Print here"
                },
                Style::Soft,
                sliced && (online || printer.kind == Kind::PrusaConnect),
            );
            let r = if sliced {
                r.on_hover_text("Send the sliced plate to this printer")
            } else {
                r.on_hover_text("Slice the plate first")
            };
            if r.clicked() {
                out = Some(CardAction::PrintHere(i));
            }
        }
        if add_printer::button_in(
            ui,
            b,
            egui::Id::unique(("card-settings", i)),
            "Settings",
            Style::Secondary,
            true,
        )
        .on_hover_text("Address, key, profile, camera; test, edit or remove")
        .clicked()
        {
            out = Some(CardAction::Settings(i));
        }
        if open {
            out = Some(CardAction::Watch(i));
        }
        ui.painter().rect_stroke(
            card,
            12.,
            Stroke::new(1., p.border_raised),
            egui::StrokeKind::Inside,
        );
        out
    }

    fn slice_only_card(
        &mut self,
        ui: &mut egui::Ui,
        card: Rect,
        vi: usize,
        preset: &str,
    ) -> Option<CardAction> {
        let p = theme::of(ui);
        ui.painter().rect_filled(card, 12., p.panel);
        let (texture, summary, model_name) = self.profile_look_named(ui.ctx(), vi, preset);
        self.card_top(
            ui,
            card,
            None,
            texture.as_ref(),
            false,
            ("Slice only", p.muted_alt, p.panel),
        );
        let inner = Rect::from_min_max(
            Pos2::new(card.left() + 14., card.top() + IMAGE_H + 14.),
            card.max - Vec2::splat(14.),
        );
        // The model's name ("Prusa XL"); a profile of the user's own keeps its name.
        let model = model_name.unwrap_or_else(|| preset.to_string());
        let summary = summary.unwrap_or_default();
        let rg = ui
            .painter()
            .layout_no_wrap(summary, theme::sans(12.), p.muted_alt);
        let rw = rg.size().x;
        ui.painter().galley(
            Pos2::new(inner.right() - rw, inner.top() + 3.),
            rg,
            p.muted_alt,
        );
        line(
            ui,
            inner.min,
            &model,
            theme::semibold(15.),
            p.text,
            inner.width() - rw - 10.,
        );
        let current = vi == self.profiles.vendor && preset == self.profiles.printer;
        line(
            ui,
            Pos2::new(inner.left(), inner.top() + 26.),
            if current {
                "The current printer. Connect it to send prints."
            } else {
                "Mica slices for it. Connect it to send prints."
            },
            theme::sans(13.),
            p.muted_alt,
            inner.width(),
        );
        // Connect… | Use | Remove
        let w = ((inner.width() - 16.) / 3.).floor();
        let y = inner.bottom() - 32.;
        let r = |k: f32| {
            Rect::from_min_size(Pos2::new(inner.left() + k * (w + 8.), y), Vec2::new(w, 32.))
        };
        let id = egui::Id::unique(("slice-only", vi, preset));
        let mut out = None;
        if add_printer::button_in(ui, r(0.), id.with("connect"), "Connect…", Style::Soft, true)
            .on_hover_text("Add its network address to send prints and watch it")
            .clicked()
        {
            out = Some(CardAction::Connect(vi, preset.to_string()));
        }
        if add_printer::button_in(ui, r(1.), id.with("use"), "Use", Style::Secondary, !current)
            .on_hover_text("Make it the current printer")
            .clicked()
        {
            out = Some(CardAction::Use(vi, preset.to_string()));
        }
        // The last remaining printer cannot be removed: the printer card needs one.
        let last = self.profiles.my_printers(&self.prefs).len() <= 1;
        if add_printer::button_in(
            ui,
            r(2.),
            id.with("remove"),
            "Remove",
            Style::Secondary,
            !current && !last,
        )
        .on_hover_text("Remove it from My printers (the profile stays available)")
        .clicked()
        {
            out = Some(CardAction::Forget(vi, preset.to_string()));
        }
        ui.painter().rect_stroke(
            card,
            12.,
            Stroke::new(1., p.border_raised),
            egui::StrokeKind::Inside,
        );
        out
    }
    fn printer_list(&mut self, ui: &mut egui::Ui) {
        let p = theme::of(ui);
        let mut pick = None;
        for (i, printer) in self.printers.list.iter().enumerate() {
            let selected =
                self.printers_editor.selected == Some(i) && self.printers_editor.editing.is_none();
            let (row, r) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), 48.), Sense::click());
            let fill = if selected {
                p.accent_surface
            } else if r.hovered() {
                p.hover
            } else {
                p.raised
            };
            ui.painter().rect_filled(row, 8., fill);
            ui.painter().rect_stroke(
                row,
                8.,
                Stroke::new(1., if selected { p.accent } else { p.border_raised }),
                egui::StrokeKind::Inside,
            );
            ui.painter().text(
                Pos2::new(row.left() + 12., row.top() + 16.),
                Align2::LEFT_CENTER,
                &printer.name,
                theme::medium(13.),
                if selected { p.accent_hi } else { p.text },
            );
            let mut job = egui::text::LayoutJob::simple_singleline(
                format!("{} · {}", printer.kind.label(), printer.address),
                theme::sans(11.),
                p.muted,
            );
            job.wrap = egui::text::TextWrapping::truncate_at_width(row.width() - 40.);
            let g = ui.painter().layout_job(job);
            ui.painter()
                .galley(Pos2::new(row.left() + 12., row.top() + 27.), g, p.muted);
            let dot = match self.printers.tests.get(&printer.name) {
                Some(Test::Done(r)) => Some(state_colour(p, Some(r.state))),
                Some(Test::Failed(_)) => Some(theme::DANGER),
                Some(Test::Running(_)) => Some(p.muted),
                None => None,
            };
            if let Some(c) = dot {
                ui.painter()
                    .circle_filled(Pos2::new(row.right() - 14., row.top() + 16.), 4., c);
            }
            if r.clicked() {
                pick = Some(i);
            }
            if r.hovered() {
                ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
            }
        }
        if self.printers.list.is_empty() {
            ui.label(
                egui::RichText::new("No printers yet.")
                    .font(theme::sans(12.))
                    .color(p.muted),
            );
        }
        ui.add_space(4.);
        if widgets::button(ui, "+ Add manually", true)
            .on_hover_text(
                "Any address and login, for a printer the Add printer window does not find",
            )
            .clicked()
        {
            let kind = if self.profiles.is_orca() {
                Kind::SnapmakerU1
            } else {
                Kind::PrusaLink
            };
            let mut printer = Printer::new(kind);
            // Link the new printer to the current profile by default.
            printer.profile = Some(format!(
                "{}/{}",
                self.profiles.vendor_name(),
                self.profiles.printer
            ));
            self.begin_edit(None, printer);
        }
        if let Some(i) = pick {
            self.printers_editor.selected = Some(i);
            self.printers_editor.editing = None;
            self.printers_editor.error = None;
        }
    }

    fn begin_edit(&mut self, index: Option<usize>, printer: Printer) {
        let e = &mut self.printers_editor;
        e.login = printer.username.is_some();
        e.editing = Some((index, printer));
        e.secret.clear();
        e.secret_touched = false;
        e.show_secret = false;
        e.camera_secret.clear();
        e.camera_secret_touched = false;
        e.error = None;
    }

    /// The selected printer: its facts, Edit / Test / Remove, and the test result.
    fn printer_details(
        &mut self,
        ui: &mut egui::Ui,
        test: &mut Option<usize>,
        remove: &mut Option<usize>,
    ) {
        let p = theme::of(ui);
        let Some(i) = self
            .printers_editor
            .selected
            .filter(|i| *i < self.printers.list.len())
        else {
            ui.add_space(8.);
            ui.label(
                egui::RichText::new(if self.printers.list.is_empty() {
                    "Add your first printer with the button on the left. You need its address \
                     (name or IP) and, for PrusaLink, the API key from the printer's settings."
                } else {
                    "Choose a printer on the left to see its details, test the connection or edit it."
                })
                .font(theme::sans(13.))
                .color(p.label),
            );
            return;
        };
        let printer = self.printers.list[i].clone();
        ui.label(
            egui::RichText::new(&printer.name)
                .font(theme::semibold(18.))
                .color(p.text),
        );
        let fact = |ui: &mut egui::Ui, label: &str, value: &str| {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(format!("{label:<10}"))
                        .font(theme::mono(12.))
                        .color(p.muted),
                );
                ui.label(
                    egui::RichText::new(value)
                        .font(theme::sans(13.))
                        .color(p.text),
                );
            });
        };
        fact(ui, "Type", printer.kind.label());
        fact(ui, "Address", &printer.address);
        if let Some(url) = printer.connect_page()
            && widgets::button(ui, "Open in Prusa Connect", true).clicked()
        {
            ui.ctx().open_url(egui::OpenUrl::new_tab(url));
        }
        let credential = match (&printer.username, printer.has_secret()) {
            (Some(u), true) => format!("user {u} + password ({})", where_kept(&printer)),
            (Some(u), false) => format!("user {u}, no password saved"),
            (None, true) => format!("API key ({})", where_kept(&printer)),
            (None, false) => "none".to_string(),
        };
        fact(ui, "Login", &credential);
        fact(
            ui,
            "Profile",
            &printer
                .profile
                .as_deref()
                .and_then(|p| p.split_once('/').map(|(_, name)| name))
                .map_or_else(
                    || "automatic (first matching profile)".to_string(),
                    str::to_string,
                ),
        );
        if let Some(m) = &printer.model {
            fact(ui, "Model", m);
        }
        let camera = match (&printer.camera, &printer.camera_username) {
            (Some(url), Some(user)) => format!(
                "{url} (user {user}{})",
                if printer.camera_keychain.is_some() {
                    ", password in the Keychain"
                } else {
                    ""
                }
            ),
            (Some(url), None) => url.clone(),
            (None, _) => "the one the printer reports, if any".into(),
        };
        fact(ui, "Camera", &camera);
        if printer.is_moonraker() {
            fact(ui, "File header", &file_header_text(&printer));
        }
        ui.add_space(4.);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.;
            let testing = matches!(
                self.printers.tests.get(&printer.name),
                Some(Test::Running(_))
            );
            if widgets::primary_button(
                ui,
                if testing {
                    "Testing…"
                } else {
                    "Test connection"
                },
                !testing,
            )
            .clicked()
            {
                *test = Some(i);
            }
            if widgets::button(ui, "Edit", true).clicked() {
                self.begin_edit(Some(i), printer.clone());
            }
            if widgets::button(ui, "Remove", true)
                .on_hover_text("Removes the printer and its saved key")
                .clicked()
            {
                *remove = Some(i);
            }
        });
        ui.add_space(6.);
        match self.printers.tests.get(&printer.name) {
            None => {}
            Some(Test::Running(_)) => {
                ui.label(
                    egui::RichText::new("Asking the printer…")
                        .font(theme::sans(13.))
                        .color(p.muted),
                );
            }
            Some(Test::Failed(x)) => warn_card(ui, &x.headline, &x.advice),
            Some(Test::Done(r)) => {
                egui::Frame::new()
                    .fill(p.raised)
                    .stroke(Stroke::new(1., p.border_raised))
                    .corner_radius(8.)
                    .inner_margin(egui::Margin::symmetric(12, 10))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.spacing_mut().item_spacing.y = 5.;
                        ui.horizontal(|ui| {
                            ui.painter().circle_filled(
                                ui.cursor().left_top() + Vec2::new(5., 10.),
                                4.,
                                state_colour(p, Some(r.state)),
                            );
                            ui.add_space(14.);
                            ui.label(
                                egui::RichText::new(if printer.kind == Kind::PrusaConnect {
                                    "Connect API key verified · readiness unknown".into()
                                } else {
                                    format!("Connected · {}", state_word(Some(r.state)))
                                })
                                .font(theme::medium(13.))
                                .color(p.text),
                            );
                        });
                        let line = |ui: &mut egui::Ui, label: &str, value: String| {
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new(format!("{label:<10}"))
                                        .font(theme::mono(12.))
                                        .color(p.muted),
                                );
                                ui.label(
                                    egui::RichText::new(value)
                                        .font(theme::sans(13.))
                                        .color(p.label),
                                );
                            });
                        };
                        line(
                            ui,
                            "Model",
                            r.model.clone().unwrap_or_else(|| "not reported".into()),
                        );
                        line(
                            ui,
                            "Firmware",
                            r.firmware.clone().unwrap_or_else(|| "not reported".into()),
                        );
                        line(
                            ui,
                            "Tools",
                            match &r.tools {
                                Some(t) if t.len() == 1 => "1 tool".into(),
                                Some(t) if !t.is_empty() => {
                                    let mut s = t.clone();
                                    s.sort_unstable();
                                    format!(
                                        "{} tools (T{}–T{})",
                                        t.len(),
                                        s[0] + 1,
                                        s[s.len() - 1] + 1
                                    )
                                }
                                _ => "not reported".into(),
                            },
                        );
                        line(ui, "Can", r.capability_words().join(", "));
                        if !r.filaments.is_empty() {
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new(format!("{:<10}", "Loaded"))
                                        .font(theme::mono(12.))
                                        .color(p.muted),
                                );
                                ui.spacing_mut().item_spacing.x = 6.;
                                for f in &r.filaments {
                                    if let Some(c) = f
                                        .colour
                                        .as_deref()
                                        .and_then(crate::printhost::explain::parse_colour)
                                    {
                                        swatch(ui, c, 12.);
                                    }
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "T{} {}",
                                            f.tool + 1,
                                            f.material.as_deref().unwrap_or("?")
                                        ))
                                        .font(theme::sans(12.))
                                        .color(p.label),
                                    );
                                    ui.add_space(4.);
                                }
                            });
                        }
                    });
            }
        }
    }

    /// The add / edit form.
    fn printer_form(&mut self, ui: &mut egui::Ui, test: &mut Option<usize>) {
        let p = theme::of(ui);
        let store_label = self.printers.secrets.label();
        let store_ok = self.printers.secrets.available();
        let my_presets: Vec<String> = self
            .profiles
            .my_printers(&self.prefs)
            .into_iter()
            .map(|(vi, name)| format!("{}/{name}", self.profiles.vendors[vi].0))
            .collect();
        let Some((index, printer)) = &mut self.printers_editor.editing else {
            return;
        };
        let is_new = index.is_none();
        let editor_login = &mut self.printers_editor.login;
        let secret = &mut self.printers_editor.secret;
        let secret_touched = &mut self.printers_editor.secret_touched;
        let show_secret = &mut self.printers_editor.show_secret;
        let camera_secret = &mut self.printers_editor.camera_secret;
        let camera_secret_touched = &mut self.printers_editor.camera_secret_touched;
        let mut save = false;
        let mut cancel = false;
        let mut forget = false;
        ui.label(
            egui::RichText::new(if is_new {
                "Add a printer"
            } else {
                "Edit printer"
            })
            .font(theme::semibold(18.))
            .color(p.text),
        );
        let field = |ui: &mut egui::Ui, label: &str, body: &mut dyn FnMut(&mut egui::Ui)| {
            ui.horizontal(|ui| {
                ui.allocate_ui_with_layout(
                    Vec2::new(90., 26.),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.label(
                            egui::RichText::new(label)
                                .font(theme::sans(13.))
                                .color(p.label),
                        );
                    },
                );
                body(ui);
            });
        };
        field(ui, "Name", &mut |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut printer.name)
                    .hint_text("e.g. XL in the workshop")
                    .desired_width(320.),
            );
        });
        field(ui, "Type", &mut |ui| {
            let old = printer.kind;
            egui::ComboBox::from_id_salt("connection-kind")
                .selected_text(printer.kind.label())
                .show_ui(ui, |ui| {
                    for kind in Kind::ALL {
                        ui.selectable_value(&mut printer.kind, kind, kind.label());
                    }
                });
            if printer.kind != old {
                if printer.kind != Kind::PrusaLink {
                    *editor_login = false;
                    printer.username = None;
                }
                if printer.kind == Kind::PrusaConnect {
                    printer.address = mica_printhost::CONNECT_URL.into();
                } else if old == Kind::PrusaConnect {
                    printer.address.clear();
                }
                // Credentials for different services are never reused implicitly.
                printer.keychain = None;
                printer.api_key_env = None;
                printer.password_env = None;
                secret.clear();
                *secret_touched = true;
            }
        });
        if printer.kind == Kind::PrusaConnect {
            printer.username = None;
            *editor_login = false;
            ui.label("Create the per-printer API key in Prusa Connect → select printer → Settings → API key.");
            ui.weak("Uploads use https://connect.prusa3d.com. The LAN PrusaLink key does not work here.");
        } else {
            field(ui, "Address", &mut |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut printer.address)
                        .hint_text(if printer.kind == Kind::PrusaLink {
                            "prusa-xl.local or 192.0.2.50"
                        } else {
                            "u1.local:7125 or 192.0.2.60:7125"
                        })
                        .font(theme::mono(13.))
                        .desired_width(320.),
                );
            });
        }
        if matches!(printer.kind, Kind::PrusaConnect | Kind::PrusaLink) {
            let mut id = printer.connect_printer_id.clone().unwrap_or_default();
            field(ui, "Connect ID", &mut |ui| {
                if ui
                    .add(
                        egui::TextEdit::singleline(&mut id)
                            .hint_text("optional: ID from /printer/ID/dashboard")
                            .desired_width(320.),
                    )
                    .changed()
                {
                    printer.connect_printer_id =
                        Some(id.trim().into()).filter(|id: &String| !id.is_empty());
                }
            });
            ui.weak("Copy the printer ID from its Prusa Connect page URL to enable Open in Prusa Connect.");
        }
        if printer.kind == Kind::PrusaLink {
            field(ui, "Log in with", &mut |ui| {
                ui.spacing_mut().item_spacing.x = 6.;
                if widgets::chip(ui, None, "API key", !*editor_login, true).clicked() {
                    *editor_login = false;
                }
                if widgets::chip(ui, None, "User name + password", *editor_login, true).clicked() {
                    *editor_login = true;
                }
            });
            if *editor_login {
                let mut user = printer.username.clone().unwrap_or_default();
                field(ui, "User name", &mut |ui| {
                    if ui
                        .add(
                            egui::TextEdit::singleline(&mut user)
                                .hint_text("maker")
                                .desired_width(320.),
                        )
                        .changed()
                    {
                        printer.username = Some(user.clone());
                    }
                });
                if printer.username.is_none() {
                    printer.username = Some(user);
                }
            } else {
                printer.username = None;
            }
        }
        let secret_label = if *editor_login {
            "Password"
        } else {
            printer.kind.secret_label()
        };
        let has_saved = printer.has_secret();
        field(ui, secret_label, &mut |ui| {
            ui.spacing_mut().item_spacing.x = 6.;
            let hint = if has_saved && !*secret_touched {
                "•••••••• saved; type to replace"
            } else if matches!(printer.kind, Kind::Moonraker | Kind::SnapmakerU1) {
                "leave empty if Moonraker needs no key"
            } else {
                "paste the key from the printer's settings"
            };
            let r = ui.add(
                egui::TextEdit::singleline(secret)
                    .password(!*show_secret)
                    .hint_text(hint)
                    .font(theme::mono(13.))
                    .desired_width(250.),
            );
            if r.changed() {
                *secret_touched = true;
            }
            if widgets::button(ui, if *show_secret { "Hide" } else { "Show" }, true).clicked() {
                *show_secret = !*show_secret;
            }
            if has_saved
                && widgets::button(ui, "Forget", true)
                    .on_hover_text("Remove the saved key or password")
                    .clicked()
            {
                forget = true;
            }
        });
        ui.horizontal(|ui| {
            ui.add_space(96.);
            ui.label(
                egui::RichText::new(if store_ok {
                    format!("Saved in the {store_label}; never written to Mica's files.")
                } else {
                    format!(
                        "Secret storage is {store_label}; use api_key_env in printers.toml instead."
                    )
                })
                .font(theme::sans(11.))
                .color(if store_ok { p.muted } else { p.warn_text }),
            );
        });
        field(ui, "Profile", &mut |ui| {
            let current = printer
                .profile
                .clone()
                .unwrap_or_else(|| "Automatic (first matching profile)".into());
            let shown = current
                .split_once('/')
                .map_or(current.clone(), |(_, n)| n.to_string());
            egui::ComboBox::from_id_salt("printer-profile")
                .width(320.)
                .selected_text(shown)
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_label(
                            printer.profile.is_none(),
                            "Automatic (first matching profile)",
                        )
                        .clicked()
                    {
                        printer.profile = None;
                    }
                    for preset in &my_presets {
                        let name = preset.split_once('/').map_or(preset.as_str(), |(_, n)| n);
                        if ui
                            .selectable_label(printer.profile.as_deref() == Some(preset), name)
                            .clicked()
                        {
                            printer.profile = Some(preset.clone());
                        }
                    }
                });
        });
        ui.horizontal(|ui| {
            ui.add_space(96.);
            ui.label(
                egui::RichText::new("Several physical printers can share one profile; the Print dialog lists the matching ones first.")
                    .font(theme::sans(11.))
                    .color(p.muted),
            );
        });
        if printer.is_moonraker() {
            field(ui, "File header", &mut |ui| {
                let shown = printer.file_header.map_or(AUTOMATIC, FileHeader::label);
                egui::ComboBox::from_id_salt("file-header")
                    .width(320.)
                    .selected_text(shown)
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(printer.file_header.is_none(), AUTOMATIC)
                            .clicked()
                            && printer.file_header.is_some()
                        {
                            // Back to Automatic: it learns again from the next upload.
                            printer.file_header = None;
                            printer.file_header_detected = None;
                        }
                        for h in [FileHeader::Mica, FileHeader::OrcaCompatible] {
                            if ui
                                .selectable_label(printer.file_header == Some(h), h.label())
                                .clicked()
                            {
                                printer.file_header = Some(h);
                            }
                        }
                    });
            });
            ui.horizontal(|ui| {
                ui.add_space(96.);
                ui.label(
                    egui::RichText::new(file_header_hint(printer))
                        .font(theme::sans(11.))
                        .color(p.muted),
                );
            });
        }
        let mut camera = printer.camera.clone().unwrap_or_default();
        field(ui, "Camera", &mut |ui| {
            if ui
                .add(
                    egui::TextEdit::singleline(&mut camera)
                        .hint_text("optional: rtsp://…, http://…/stream.mjpg or …/snapshot.jpg")
                        .font(theme::mono(13.))
                        .desired_width(320.),
                )
                .changed()
            {
                printer.camera = Some(camera.clone()).filter(|c| !c.trim().is_empty());
            }
        });
        ui.horizontal(|ui| {
            ui.add_space(96.);
            ui.label(
                egui::RichText::new(match printer.kind {
                    Kind::PrusaConnect => "Open Prusa Connect to view this printer’s camera.",
                    Kind::PrusaLink => "PrusaLink printers (MK4, XL, CORE One) do not share their camera over the network; enter an IP camera here.",
                    _ => "Leave empty to use the camera Klipper reports (Mainsail / Fluidd webcam settings).",
                })
                .font(theme::sans(11.))
                .color(p.muted),
            );
        });
        if printer.camera.is_some() {
            let mut user = printer.camera_username.clone().unwrap_or_default();
            let has_camera_secret = printer.camera_keychain.is_some();
            field(ui, "Camera login", &mut |ui| {
                ui.spacing_mut().item_spacing.x = 6.;
                if ui
                    .add(
                        egui::TextEdit::singleline(&mut user)
                            .hint_text("user (if the camera asks)")
                            .desired_width(150.),
                    )
                    .changed()
                {
                    printer.camera_username = Some(user.clone()).filter(|u| !u.trim().is_empty());
                }
                let r = ui.add(
                    egui::TextEdit::singleline(camera_secret)
                        .password(true)
                        .hint_text(if has_camera_secret && !*camera_secret_touched {
                            "•••••• saved"
                        } else {
                            "password"
                        })
                        .desired_width(160.),
                );
                if r.changed() {
                    *camera_secret_touched = true;
                }
            });
        }
        if let Some(e) = &self.printers_editor.error {
            warn_card(ui, "Not saved", e);
        }
        ui.add_space(4.);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.;
            if widgets::primary_button(ui, if is_new { "Add" } else { "Save" }, true).clicked() {
                save = true;
            }
            if widgets::button(ui, "Cancel", true).clicked() {
                cancel = true;
            }
            if widgets::button(ui, "Save and test", true).clicked() {
                save = true;
                *test = Some(usize::MAX);
            }
        });
        if forget {
            *secret = String::new();
            *secret_touched = true;
            printer.api_key_env = None;
            printer.password_env = None;
        }
        if cancel {
            self.printers_editor.editing = None;
            self.printers_editor.error = None;
            if test.is_some() {
                *test = None;
            }
        }
        if save {
            let (index, mut printer) = self.printers_editor.editing.clone().expect("editing");
            let typed = self
                .printers_editor
                .secret_touched
                .then(|| self.printers_editor.secret.clone());
            // A camera URL pasted with `user:password@` is split: the user name stays, the
            // password goes to the Keychain, the URL is saved without them.
            let mut camera_typed = self
                .printers_editor
                .camera_secret_touched
                .then(|| self.printers_editor.camera_secret.clone());
            if let Some(url) = printer.camera.clone()
                && let Some((clean, user, pass)) = split_credentials(&url)
            {
                printer.camera = Some(clean);
                printer.camera_username = Some(user).filter(|u| !u.is_empty());
                if pass.is_some() {
                    camera_typed = pass;
                }
            }
            if printer.camera.is_none() {
                printer.camera_username = None;
                if printer.camera_keychain.is_some() {
                    camera_typed = Some(String::new());
                }
            }
            let id = index.and_then(|i| self.printers.id_at(i));
            let identity_before = self.printer_gcode_identity();
            let saved = self
                .printers
                .put(index, printer, typed.as_deref())
                .and_then(|i| {
                    if let Some(c) = &camera_typed {
                        self.printers.put_camera_secret(i, c)?;
                    }
                    Ok(i)
                });
            if let Some(id) = id {
                self.machine_view.cameras.forget(id);
            }
            self.printers_editor.camera_secret.clear();
            self.printers_editor.camera_secret_touched = false;
            match saved {
                Ok(i) => {
                    // A new file header changes the G-code: slice again.
                    if self.printer_gcode_identity() != identity_before {
                        self.settings_changed();
                    }
                    self.printers_editor.editing = None;
                    self.printers_editor.error = None;
                    self.printers_editor.secret.clear();
                    self.printers_editor.selected = Some(i);
                    if *test == Some(usize::MAX) {
                        *test = Some(i);
                    }
                }
                Err(e) => {
                    self.printers_editor.error = Some(e);
                    *test = None;
                }
            }
        }
    }
}

const AUTOMATIC: &str = "Automatic (recommended)";

/// The File header setting in the printer's details: `Automatic · Orca-compatible`.
fn file_header_text(printer: &Printer) -> String {
    match (printer.file_header, printer.file_header_detected) {
        (Some(h), _) => h.label().to_string(),
        (None, Some(h)) => format!("Automatic · {} (the firmware asked for it)", h.label()),
        (None, None) => "Automatic (the profile's default)".into(),
    }
}

/// The one-line explanation under the File header choice.
fn file_header_hint(printer: &Printer) -> &'static str {
    match (printer.file_header, printer.file_header_detected) {
        (None, Some(FileHeader::OrcaCompatible)) => {
            "This printer's firmware didn't recognize Mica Slicer files, so they get an Orca-compatible header."
        }
        (None, _) => {
            "How files name their slicer. Mica checks after uploads that the printer shows thumbnails and times."
        }
        (Some(FileHeader::Mica), _) => {
            "Files name Mica Slicer; older Klipper firmware then shows no thumbnail or print time."
        }
        (Some(FileHeader::OrcaCompatible), _) => {
            "Files also name OrcaSlicer, which older Klipper firmware (Snapmaker U1) recognizes."
        }
    }
}

/// `scheme://user:pass@host/…` → (`scheme://host/…`, user, password); `None` when the URL
/// carries no login.
fn split_credentials(url: &str) -> Option<(String, String, Option<String>)> {
    let u = url.trim();
    let (scheme, rest) = u.split_once("://")?;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(end);
    let (userinfo, host) = authority.rsplit_once('@')?;
    let (user, pass) = match userinfo.split_once(':') {
        Some((u, p)) => (u.to_string(), Some(p.to_string())),
        None => (userinfo.to_string(), None),
    };
    Some((format!("{scheme}://{host}{tail}"), user, pass))
}

fn where_kept(printer: &Printer) -> &'static str {
    if printer.keychain.is_some() {
        "in the Keychain"
    } else if printer.api_key_env.is_some() || printer.password_env.is_some() {
        "from an environment variable"
    } else {
        "not saved"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camera_urls_lose_their_login_to_separate_fields() {
        assert_eq!(
            split_credentials("rtsp://admin:pa:ss@198.51.100.9:554/stream1?x=1"),
            Some((
                "rtsp://198.51.100.9:554/stream1?x=1".into(),
                "admin".into(),
                Some("pa:ss".into())
            ))
        );
        assert_eq!(
            split_credentials("http://viewer@cam/stream.mjpg"),
            Some(("http://cam/stream.mjpg".into(), "viewer".into(), None))
        );
        assert_eq!(split_credentials("http://cam/a@b"), None);
        assert_eq!(split_credentials("http://cam/"), None);
    }
}
