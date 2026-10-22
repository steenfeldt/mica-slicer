// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The 48 px header: wordmark, File / Edit / View menus (`menu.rs`; in the macOS menu bar
//! instead when it could be set up), the centred Prepare | Preview
//! segmented control, and on the right the time · weight read-out (or slicing progress),
//! Export and the primary "Print…" button. There is no status bar: transient status text is
//! shown by the viewport's hint line, totals live in the bottom-right card.

use super::{duration, widgets};
use crate::app::App;
use crate::keymap::Act;
use crate::theme;
use eframe::egui::{self, Align2, Pos2, Rect, Sense, Stroke, Vec2};

/// Width of the slicing progress read-out in the header, independent of the stage name.
const PROGRESS_WIDTH: f32 = 260.;

pub const HEADER_H: f32 = 48.;

impl App {
    pub(crate) fn topbar(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let p = theme::of(ui);
        egui::Panel::top("header")
            .exact_size(HEADER_H)
            .show_separator_line(false)
            .resizable(false)
            .frame(egui::Frame::NONE.fill(p.header))
            .show(ui, |ui| {
                let rect = ui.max_rect();
                ui.painter().line_segment(
                    [rect.left_bottom(), rect.right_bottom()],
                    Stroke::new(1., p.border),
                );
                // Left: wordmark and menus.
                let left = Rect::from_min_max(
                    Pos2::new(rect.left() + 16., rect.top()),
                    Pos2::new(rect.center().x - 150., rect.bottom()),
                );
                ui.scope_builder(
                    egui::UiBuilder::new()
                        .max_rect(left)
                        .layout(egui::Layout::left_to_right(egui::Align::Center)),
                    |ui| {
                        ui.spacing_mut().item_spacing.x = 2.;
                        ui.label(
                            egui::RichText::new("mica")
                                .font(theme::semibold(15.))
                                .color(p.text),
                        );
                        ui.add_space(8.);
                        // On the Mac the menus are in the system menu bar.
                        if !self.has_native_menu() {
                            flat_menu_style(ui);
                            self.menu_bar(ui);
                        }
                        self.document_label(ui);
                    },
                );
                // Centre: Prepare | Preview.
                self.mode_switch(ui, Pos2::new(rect.center().x, rect.center().y));
                // Right: read-out, Export, Print….
                let right = Rect::from_min_max(
                    Pos2::new(rect.center().x + 150., rect.top()),
                    Pos2::new(rect.right() - 12., rect.bottom()),
                );
                let cluster = ui.scope_builder(
                    egui::UiBuilder::new()
                        .max_rect(right)
                        .layout(egui::Layout::right_to_left(egui::Align::Center)),
                    |ui| {
                        ui.spacing_mut().item_spacing.x = 8.;
                        let busy = self.busy();
                        let sliced = self.finished().is_some();
                        let doc = self.viewing_gcode();
                        // Why Export and Print are off: a failed slice (a toolpath outside the
                        // printable area, as PrusaSlicer and Orca block export) says so.
                        let blocked = self.blocked_reason(busy);
                        if widgets::primary_button(ui, "Print…", sliced && !busy)
                            .on_hover_text(if doc {
                                "Send the opened G-code file to a printer, unchanged"
                            } else {
                                "Send the sliced plate to a printer (PrusaLink, Klipper, Snapmaker U1)"
                            })
                            .on_disabled_hover_text(&blocked)
                            .clicked()
                        {
                            self.open_print_dialog();
                        }
                        if !doc && self.plates.len() > 1 {
                            self.export_button(ui, &ctx, sliced && !busy);
                        } else if widgets::button(ui, if doc { "Save copy…" } else { "Export" }, sliced && !busy)
                            .on_hover_text(if doc {
                                "Save the opened file's bytes, unchanged, under another name (⌘G)"
                            } else {
                                "Save the sliced plate as G-code (⌘G)"
                            })
                            .on_disabled_hover_text(&blocked)
                            .clicked()
                        {
                            self.export(&ctx);
                        }
                        self.readout(ui, &ctx);
                    },
                );
                crate::tour::anchor(ui, crate::tour::Anchor::SliceExport, cluster.response.rect);
            });
    }

    /// Why Export and Print are disabled: the current plate's failed slice, a running job, or
    /// no slice yet.
    pub(crate) fn blocked_reason(&self, busy: bool) -> String {
        if busy {
            return "Wait for the current job to finish".into();
        }
        match &self.plates.current().slice_error {
            Some(e) => format!("Can't export or print: {}", e.message),
            None => "Slice the plate first".into(),
        }
    }

    /// Export with several plates: a split button, the menu has Export plate… and Export all
    /// plates….
    fn export_button(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, can_export: bool) {
        let all = self.can_export_all();
        let (main, menu) = widgets::split_button(ui, "Export", can_export, all, false);
        probe(ui, "export menu", menu.rect);
        open_requested(ui, "export menu", &menu);
        let blocked = self.blocked_reason(self.busy());
        if main
            .on_hover_text("Save the sliced plate as G-code (⌘G)")
            .on_disabled_hover_text(blocked)
            .clicked()
        {
            self.export(ctx);
        }
        let menu = menu.on_hover_text("Export this plate, or every plate into a folder");
        egui::Popup::menu(&menu).gap(6.).show(|ui| {
            ui.set_min_width(230.);
            if menu_item(ui, "Export plate…", Some(Act::ExportGcode), can_export) {
                self.export(ctx);
            }
            if menu_item(ui, "Export all plates…", None, all) {
                self.export_all_dialog(ctx);
            }
        });
    }

    /// The Slice button; with several plates a split button whose menu offers Slice plate
    /// (⌘R) and Slice all plates (⇧⌘R).
    fn slice_button(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, hover: &str) {
        if self.plates.len() < 2 {
            if widgets::primary_button(ui, "Slice", true)
                .on_hover_text(hover)
                .clicked()
            {
                self.slice_current(ctx);
            }
            return;
        }
        let (main, menu) = widgets::split_button(ui, "Slice", true, true, true);
        probe(ui, "slice menu", menu.rect);
        open_requested(ui, "slice menu", &menu);
        if main.on_hover_text(hover).clicked() {
            self.slice_current(ctx);
        }
        let menu = menu.on_hover_text("Slice this plate, or all plates (⇧⌘R)");
        let can_all = self
            .menu_state(crate::menu::Action::Act(Act::SliceAll), false)
            .enabled;
        egui::Popup::menu(&menu).gap(6.).show(|ui| {
            ui.set_min_width(230.);
            if menu_item(ui, "Slice plate", Some(Act::Slice), true) {
                self.slice_current(ctx);
            }
            if menu_item(ui, "Slice all plates", Some(Act::SliceAll), can_all) {
                self.slice_all(ctx);
            }
        });
    }

    /// The document after the menus: its name, a `•` while there are unsaved changes (the
    /// window title carries the same), truncated to the room left before the mode switch.
    fn document_label(&mut self, ui: &mut egui::Ui) {
        let p = theme::of(ui);
        if let Some(doc) = &self.gcode {
            let text = format!("{} · read-only", doc.name);
            let tip = doc.path.display().to_string();
            ui.add_space(10.);
            let width = ui.available_width() - 8.;
            if width >= 40. {
                let mut job =
                    egui::text::LayoutJob::simple_singleline(text, theme::medium(12.), p.muted_alt);
                job.wrap = egui::text::TextWrapping::truncate_at_width(width);
                ui.label(job).on_hover_text(tip);
            }
            return;
        }
        let modified = self.is_modified();
        let name = self.document_name();
        let text = if modified {
            format!("{name} •")
        } else {
            name
        };
        ui.add_space(10.);
        let width = ui.available_width() - 8.;
        if width < 40. {
            return;
        }
        let mut job = egui::text::LayoutJob::simple_singleline(
            text,
            theme::medium(12.),
            if modified { p.text } else { p.muted_alt },
        );
        job.wrap = egui::text::TextWrapping::truncate_at_width(width);
        let galley = ui.painter().layout_job(job);
        let (rect, response) = ui.allocate_exact_size(
            Vec2::new(galley.size().x, ui.available_height()),
            Sense::hover(),
        );
        ui.painter().galley(
            Pos2::new(rect.left(), rect.center().y - galley.size().y / 2.),
            galley,
            p.text,
        );
        response.on_hover_text(match (&self.document.path, modified) {
            (Some(path), true) => format!("{} — unsaved changes (⌘S saves)", path.display()),
            (Some(path), false) => path.display().to_string(),
            (None, true) => "Unsaved changes; ⌘S saves the project".to_string(),
            (None, false) => "No project file yet".to_string(),
        });
    }

    /// The slice state, right of the Prepare | Preview switch: a violet **Slice** button while the
    /// plate is not sliced; progress and **Cancel** while slicing; `38 min · 11.6 g` once it is
    /// done; and, after a change, the old totals muted with "changed, slice again" next to the
    /// Slice button. With auto-slice on the button is not needed.
    fn readout(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let p = theme::of(ui);
        if self.gcode_progress(ui) {
            return;
        }
        if let Some(doc) = &self.gcode {
            let s = &doc.finished.summary;
            // A file without its filament's density gives no weight: its length instead.
            let amount = if s.grams > 0. {
                format!("{:.1} g", s.grams)
            } else {
                format!("{:.2} m", s.filament_mm / 1000.)
            };
            ui.label(
                egui::RichText::new(format!("{} · {amount}", short_duration(s.time_s)))
                    .font(theme::mono(12.))
                    .color(p.muted_alt),
            )
            .on_hover_text(format!(
                "{} · {:.2} m of filament (from the file)",
                duration(s.time_s),
                s.filament_mm / 1000.
            ));
            return;
        }
        if self.worker.is_some() {
            if widgets::button(ui, "Cancel", true).clicked() {
                self.cancel_slice();
            }
            // The stage and its own percentage, with the whole job as a thin bar under it.
            let text = crate::worker::Progress {
                overall: self.progress,
                stage: self.progress_stage.clone(),
                fraction: self.progress_fraction,
                detail: String::new(),
            }
            .header();
            // A fixed-size slot so the bar doesn't jump with the stage name: the stage on the
            // left (without its own percentage), the whole job's percentage on the right.
            let stage = match text.rsplit_once(' ') {
                Some((head, "%")) => head.rsplit_once(' ').map_or(head, |(h, _)| h).to_string(),
                _ => text.clone(),
            };
            // Slice all: which plate of how many.
            let batch = self.batch_position();
            let stage = match batch {
                Some((k, n)) => format!("Slicing plate {k} of {n} · {stage}"),
                None => stage,
            };
            let (slot, response) =
                ui.allocate_exact_size(Vec2::new(PROGRESS_WIDTH, 22.), egui::Sense::hover());
            let font = theme::mono(12.);
            let percent = format!("{:.0} %", self.progress.clamp(0., 1.) * 100.);
            let percent_galley = ui
                .painter()
                .layout_no_wrap(percent, font.clone(), p.muted_alt);
            let stage_width = slot.width() - percent_galley.size().x - 8.;
            let mut stage_job = egui::text::LayoutJob::simple_singleline(stage, font, p.muted_alt);
            stage_job.wrap = egui::text::TextWrapping::truncate_at_width(stage_width);
            let stage_galley = ui.painter().layout_job(stage_job);
            ui.painter()
                .galley(slot.left_top(), stage_galley, p.muted_alt);
            ui.painter().galley(
                Pos2::new(slot.right() - percent_galley.size().x, slot.top()),
                percent_galley,
                p.muted_alt,
            );
            let label = response;
            let bar = Rect::from_min_size(
                Pos2::new(slot.left(), slot.bottom() - 2.),
                Vec2::new(slot.width(), 3.),
            );
            ui.painter().rect_filled(bar, 1.5, p.field_border);
            ui.painter().rect_filled(
                Rect::from_min_size(
                    bar.min,
                    Vec2::new(bar.width() * self.progress.clamp(0., 1.), bar.height()),
                ),
                1.5,
                p.accent,
            );
            let plate = self
                .worker
                .as_ref()
                .and_then(|(id, _)| self.plates.index_of(*id))
                .map(|i| self.plates.plate(i).name.clone())
                .unwrap_or_default();
            label.on_hover_text(match batch {
                Some((k, n)) => format!(
                    "Slicing all plates: {plate} ({k} of {n}) · {:.0} % of this plate · {}\nCancel stops the remaining plates too.",
                    self.progress * 100.,
                    self.status
                ),
                None => format!(
                    "{:.0} % of the whole job · {}",
                    self.progress * 100.,
                    self.status
                ),
            });
            return;
        }
        if self.stopping.is_some() {
            let text = if self.pending_slice.is_some() {
                "Canceling… then slicing again"
            } else {
                "Canceling…"
            };
            ui.label(
                egui::RichText::new(text)
                    .font(theme::mono(12.))
                    .color(p.muted),
            );
            return;
        }
        let can_slice =
            !self.busy() && self.config.is_ok() && !self.plates.current().objects.is_empty();
        if let Some(e) = self.plates.current().slice_error.clone() {
            // A failed slice: no old totals, the reason in red; a click shows its notification.
            if can_slice && !self.prefs.auto_slice {
                self.slice_button(ui, ctx, "Slice the current plate again (⌘R)");
            }
            if failed_pill(ui, &format!("Slice failed: {}", e.short))
                .on_hover_text(format!("{}\n\nClick to show the notification.", e.message))
                .clicked()
            {
                self.focus_slice_error();
            }
            return;
        }
        if let Some(f) = self.finished() {
            let s = &f.summary;
            let text = format!("{} · {:.1} g", short_duration(s.time_s), s.grams);
            let hover = format!(
                "{} · {:.2} m of filament",
                duration(s.time_s),
                s.filament_mm / 1000.
            );
            // Other plates still to slice: Slice all next to this plate's totals.
            let waiting = (0..self.plates.len())
                .filter(|&i| {
                    i != self.plates.current
                        && self.sliceable(i)
                        && self.plates.plate(i).finished.is_none()
                })
                .count();
            if waiting > 0
                && !self.busy()
                && self.config.is_ok()
                && widgets::button(ui, "Slice all", true)
                    .on_hover_text(format!(
                        "Slice the {waiting} other {} not sliced yet (⇧⌘R)",
                        if waiting == 1 { "plate" } else { "plates" }
                    ))
                    .clicked()
            {
                self.slice_all(ctx);
            }
            ui.label(
                egui::RichText::new(text)
                    .font(theme::mono(12.))
                    .color(p.muted_alt),
            )
            .on_hover_text(hover);
            return;
        }
        if can_slice && !self.prefs.auto_slice {
            self.slice_button(ui, ctx, "Slice the current plate (⌘R)");
        }
        // Outdated: the previous totals stay readable but muted, with a note.
        if can_slice && let Some(prev) = &self.plates.current().previous {
            ui.label(
                egui::RichText::new(format!(
                    "{} · {:.1} g · changed, slice again",
                    short_duration(prev.time_s),
                    prev.grams
                ))
                .font(theme::mono(12.))
                .color(p.muted.gamma_multiply(0.8)),
            )
            .on_hover_text("Totals of the last slice; the plate has changed since");
        }
    }

    /// The segmented Library | Prepare | Preview | Machine control centred at `center`, in
    /// workflow order: find a model, set it up, check the toolpaths, print.
    fn mode_switch(&mut self, ui: &mut egui::Ui, center: Pos2) {
        const LABELS: [&str; 4] = ["Prepare", "Preview", "Machine", "Library"];
        /// The order the segments are drawn in (indices into `LABELS`).
        const ORDER: [usize; 4] = [3, 0, 1, 2];
        let p = theme::of(ui);
        let seg = Vec2::new(0., 28.);
        let font = theme::medium(13.);
        let painter = ui.painter();
        let widths: Vec<f32> = LABELS
            .iter()
            .map(|t| {
                painter
                    .layout_no_wrap(t.to_string(), font.clone(), p.text)
                    .size()
                    .x
                    + 32.
            })
            .collect();
        let total = widths.iter().sum::<f32>() + 6.;
        let outer = Rect::from_center_size(center, Vec2::new(total, seg.y + 6.));
        crate::tour::anchor(ui, crate::tour::Anchor::Tabs, outer);
        painter.rect_filled(outer, 8., p.search_bg.gamma_multiply(1.6));
        painter.rect_stroke(
            outer,
            8.,
            Stroke::new(1., p.border_soft),
            egui::StrokeKind::Inside,
        );
        let mut x = outer.left() + 3.;
        let can_preview = self.finished().is_some();
        // Preview of an unsliced plate slices it first (as PrusaSlicer and Orca do).
        let can_slice =
            !self.busy() && self.config.is_ok() && !self.plates.current().objects.is_empty();
        // An opened G-code file has no Prepare: closing it returns to the plate.
        let doc = self.viewing_gcode();
        let ctx = ui.ctx().clone();
        let current = if self.library.shown {
            3
        } else if self.machine {
            2
        } else if self.previewing() {
            1
        } else {
            0
        };
        for i in ORDER {
            let (label, w) = (&LABELS[i], widths[i]);
            let r = Rect::from_min_size(Pos2::new(x, outer.top() + 3.), Vec2::new(w, seg.y));
            x += w;
            if i == 2 {
                crate::tour::anchor(ui, crate::tour::Anchor::MachineTab, r);
            }
            let active = i == current;
            let enabled = (i != 1 || can_preview || can_slice) && !(doc && i == 0);
            let response = ui.interact(
                r,
                ui.make_persistent_id(("mode", i)),
                if enabled {
                    Sense::click()
                } else {
                    Sense::hover()
                },
            );
            if active {
                ui.painter().rect_filled(r, 6., p.accent_surface);
            } else if enabled && response.hovered() {
                ui.painter().rect_filled(r, 6., p.hover);
            }
            // A model dragged from the Library: Prepare opens under it.
            if i == 0 && enabled {
                self.library_drag_tab(ui, r);
            }
            let color = if active {
                p.accent_hi
            } else if enabled {
                p.muted_alt
            } else {
                p.muted.gamma_multiply(0.6)
            };
            ui.painter().text(
                r.center(),
                Align2::CENTER_CENTER,
                *label,
                font.clone(),
                color,
            );
            let response = response.on_hover_text(if i == 0 && doc {
                "Close the G-code file to arrange and set up the plate"
            } else if i == 0 {
                "Arrange and set up the plate (Tab)"
            } else if i == 2 {
                "Watch and control your printers"
            } else if i == 3 {
                "Find models, projects and G-code in your folders"
            } else if can_preview {
                "Show the sliced toolpaths (Tab)"
            } else if can_slice {
                "Slice the plate, then show the toolpaths (Tab)"
            } else {
                "Add a model to the plate, then slice it to see the toolpaths"
            });
            if enabled && response.clicked() {
                self.library.shown = i == 3;
                if i == 3 {
                    continue;
                }
                self.machine = i == 2;
                if i == 1 && !can_preview {
                    self.slice_current(&ctx);
                } else if i < 2 {
                    self.preview = i == 1;
                }
            }
        }
    }
}

/// Menus in the header are flat text buttons (`.ibtn` with auto width).
fn flat_menu_style(ui: &mut egui::Ui) {
    let p = theme::of(ui);
    let v = ui.visuals_mut();
    v.widgets.inactive.weak_bg_fill = egui::Color32::TRANSPARENT;
    v.widgets.inactive.bg_stroke = Stroke::NONE;
    v.widgets.inactive.fg_stroke = Stroke::new(1., p.muted_alt);
    v.widgets.hovered.weak_bg_fill = p.hover;
    v.widgets.hovered.bg_stroke = Stroke::NONE;
    v.widgets.hovered.fg_stroke = Stroke::new(1., p.text);
    v.widgets.active.weak_bg_fill = p.hover;
    v.widgets.active.bg_stroke = Stroke::NONE;
    v.widgets.open.weak_bg_fill = p.hover;
    v.widgets.open.bg_stroke = Stroke::NONE;
    v.override_text_color = None;
    ui.spacing_mut().button_padding = Vec2::new(10., 6.);
}

/// `38 min`, `1 h 12 min`, `45 s`: the compact header form.
/// One row of the Slice / Export split buttons' menus, with its shortcut; true when picked.
fn menu_item(ui: &mut egui::Ui, label: &str, act: Option<Act>, enabled: bool) -> bool {
    let key = act.map(crate::keymap::shortcut_text).unwrap_or_default();
    let r = ui.add_enabled(enabled, egui::Button::new(label).shortcut_text(key));
    probe(ui, label, r.rect);
    if r.clicked() {
        ui.close();
        true
    } else {
        false
    }
}

/// Opens the menu of split button `which` (`slice menu`, `export menu`) on this frame (the
/// screenshot tool's `--command slice-menu`).
pub fn request_menu(ctx: &egui::Context, which: &str) {
    ctx.data_mut(|d| d.insert_temp(egui::Id::unique("topbar open").with(which), true));
}

fn open_requested(ui: &egui::Ui, which: &str, menu: &egui::Response) {
    let id = egui::Id::unique("topbar open").with(which);
    if ui
        .ctx()
        .data_mut(|d| d.remove_temp::<bool>(id))
        .unwrap_or(false)
    {
        egui::Popup::open_id(ui.ctx(), egui::Popup::default_response_id(menu));
    }
}

/// Where the header's split buttons and their menu rows were drawn (UI tests click them).
#[derive(Clone, Default)]
pub struct Probe(pub Vec<(String, Rect)>);

fn probe(ui: &egui::Ui, label: &str, rect: Rect) {
    ui.ctx().data_mut(|d| {
        let probe = d.get_temp_mut_or_default::<Probe>(egui::Id::unique("topbar probe"));
        probe.0.retain(|(l, _)| l != label);
        probe.0.push((label.to_string(), rect));
    });
}

/// The rect `label` (a menu row, `slice menu`, `export menu`) was last drawn at.
#[cfg(test)]
pub fn probed(ctx: &egui::Context, label: &str) -> Option<Rect> {
    ctx.data(|d| {
        d.get_temp::<Probe>(egui::Id::unique("topbar probe"))
            .and_then(|p| p.0.iter().find(|(l, _)| l == label).map(|(_, r)| *r))
    })
}

pub fn short_duration(seconds: f64) -> String {
    let s = seconds.max(0.).round() as u64;
    let (h, m) = (s / 3600, (s % 3600) / 60);
    if h > 0 {
        format!("{h} h {m:02} min")
    } else if m > 0 {
        format!("{m} min")
    } else {
        format!("{s} s")
    }
}

/// The header's failed-slice state: a red-edged pill with the reason in a few words,
/// truncated to the room there is; clicking it shows the notification.
fn failed_pill(ui: &mut egui::Ui, text: &str) -> egui::Response {
    let p = theme::of(ui);
    let font = theme::medium(12.);
    let room = (ui.available_width() - 8.).max(80.);
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_string(), font, p.text);
    job.wrap = egui::text::TextWrapping::truncate_at_width(room - 30.);
    let galley = ui.painter().layout_job(job);
    let size = Vec2::new(galley.size().x + 30., 26.);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let hovered = response.hovered();
    ui.painter().rect_filled(
        rect,
        6.,
        theme::DANGER.gamma_multiply(if hovered { 0.28 } else { 0.18 }),
    );
    ui.painter().rect_stroke(
        rect,
        6.,
        Stroke::new(1., theme::DANGER),
        egui::StrokeKind::Inside,
    );
    ui.painter().circle_filled(
        Pos2::new(rect.left() + 12., rect.center().y),
        4.,
        theme::DANGER,
    );
    ui.painter().galley(
        Pos2::new(rect.left() + 22., rect.center().y - galley.size().y / 2.),
        galley,
        p.text,
    );
    if hovered {
        ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
    }
    response
}

#[cfg(test)]
mod tests {
    #[test]
    fn stages_and_durations() {
        assert_eq!(super::short_duration(38. * 60. + 28.), "38 min");
        assert_eq!(super::short_duration(3600. + 12. * 60.), "1 h 12 min");
        assert_eq!(super::short_duration(45.), "45 s");
    }
}
