// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! What an opened G-code file adds to the window: the "Viewing G-code file" chip over the 3D
//! view (with Close), the file card on top of the Preview sidebar (what made it, which printer
//! it is for, Mica's check with its findings, Print… / Save copy… / Close) and the opening
//! progress in the header.

use super::{
    preview_side::{note, section},
    widgets,
};
use crate::app::App;
use crate::gcode_doc::{Check, app::plural};
use crate::theme;
use eframe::egui::{self, Align2, Sense, Stroke, Vec2};
use mica_gcode::validate::Severity;

impl App {
    /// The chip at the top of the 3D view: the file is shown read-only; Close returns to the
    /// scene.
    pub(crate) fn gcode_chip(&mut self, ctx: &egui::Context) {
        let Some(doc) = &self.gcode else { return };
        let name = doc.name.clone();
        let r = self.viewport_rect;
        let mut close = false;
        egui::Area::new(egui::Id::unique("gcode-chip"))
            .order(egui::Order::Foreground)
            .fixed_pos(egui::pos2(r.center().x, r.top() + 12.))
            .pivot(Align2::CENTER_TOP)
            .show(ctx, |ui| {
                let p = theme::of(ui);
                egui::Frame::new()
                    .fill(p.raised)
                    .stroke(Stroke::new(1., p.accent))
                    .corner_radius(16.)
                    .inner_margin(egui::Margin {
                        left: 14,
                        right: 6,
                        top: 4,
                        bottom: 4,
                    })
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 8.;
                            ui.label(
                                egui::RichText::new("Viewing G-code file")
                                    .font(theme::semibold(12.5))
                                    .color(p.accent_hi),
                            );
                            let mut job = egui::text::LayoutJob::simple_singleline(
                                name.clone(),
                                theme::medium(12.5),
                                p.text,
                            );
                            job.wrap = egui::text::TextWrapping::truncate_at_width(
                                (r.width() * 0.4).max(80.),
                            );
                            ui.label(job).on_hover_text(
                                "Read-only: the file is never changed. Close it to get back to your plate.",
                            );
                            ui.label(
                                egui::RichText::new("read-only")
                                    .font(theme::sans(11.5))
                                    .color(p.muted),
                            );
                            if widgets::button(ui, "Close", true)
                                .on_hover_text("Close the G-code file and return to your plate")
                                .clicked()
                            {
                                close = true;
                            }
                        });
                    });
            });
        if close {
            self.close_gcode();
        }
    }

    /// The file card on top of the Preview sidebar.
    pub(crate) fn gcode_card(&mut self, ui: &mut egui::Ui) {
        let Some(doc) = &self.gcode else { return };
        let p = theme::of(ui);
        let name = doc.name.clone();
        let generator = doc.origin.generator.clone();
        let printer = doc.printer.label(&doc.origin);
        let known = doc.origin.known();
        let format = if doc.finished.binary {
            "Binary G-code (.bgcode)"
        } else {
            "Text G-code"
        };
        let size = std::fs::metadata(&doc.path).map(|m| m.len()).ok();
        let lines = doc.scan.lines;
        let check = doc.check.clone();
        let path = doc.path.clone();
        let busy = self.busy();
        let mut print = false;
        let mut save = false;
        let mut close = false;
        ui.add_space(8.);
        section(ui, "G-code file", |ui| {
            ui.spacing_mut().item_spacing.y = 4.;
            ui.add(
                egui::Label::new(
                    egui::RichText::new(&name)
                        .font(theme::semibold(14.))
                        .color(p.text),
                )
                .wrap(),
            )
            .on_hover_text(path.display().to_string());
            let row = |ui: &mut egui::Ui, label: &str, value: &str| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        egui::RichText::new(label)
                            .font(theme::sans(12.))
                            .color(p.muted),
                    );
                    ui.label(
                        egui::RichText::new(value)
                            .font(theme::sans(12.))
                            .color(p.label),
                    );
                });
            };
            row(
                ui,
                "Made with",
                generator.as_deref().unwrap_or("an unknown slicer"),
            );
            row(
                ui,
                "For",
                &if known {
                    printer
                } else {
                    "the file does not name its printer".into()
                },
            );
            let size = size.map_or(String::new(), |b| {
                format!(" · {:.1} MB", b as f64 / 1_048_576.)
            });
            row(ui, "Format", &format!("{format}{size} · {lines} lines"));
            ui.add_space(6.);
            note(
                ui,
                "Read-only: Mica shows and sends this file exactly as it is. Slicing and editing come back when you close it.",
            );
            ui.add_space(8.);
            check_card(ui, &check);
            ui.add_space(10.);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.;
                print = widgets::primary_button(ui, "Print…", !busy)
                    .on_hover_text("Send this file to a printer, unchanged")
                    .clicked();
                save = widgets::button(ui, "Save copy…", !busy)
                    .on_hover_text("Save the file's bytes, unchanged, under another name")
                    .clicked();
                close = widgets::button(ui, "Close", true)
                    .on_hover_text("Close the G-code file and return to your plate")
                    .clicked();
            });
        });
        let ctx = ui.ctx().clone();
        if print {
            self.open_print_dialog();
        }
        if save {
            self.save_gcode_copy(&ctx);
        }
        if close {
            self.close_gcode();
        }
    }

    /// The header read-out while a G-code file opens: its progress and Cancel.
    pub(crate) fn gcode_progress(&mut self, ui: &mut egui::Ui) -> bool {
        let Some(l) = &self.gcode_loading else {
            return false;
        };
        let p = theme::of(ui);
        let fraction = l.progress.clamp(0., 1.);
        let text = format!("{} {:.0} %", l.stage, fraction * 100.);
        let name = l
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut cancel = false;
        if widgets::button(ui, "Cancel", true).clicked() {
            cancel = true;
        }
        let (slot, response) = ui.allocate_exact_size(Vec2::new(220., 22.), Sense::hover());
        ui.painter().text(
            slot.left_top(),
            Align2::LEFT_TOP,
            text,
            theme::mono(12.),
            p.muted_alt,
        );
        let bar = egui::Rect::from_min_size(
            egui::pos2(slot.left(), slot.bottom() - 2.),
            Vec2::new(slot.width(), 3.),
        );
        ui.painter().rect_filled(bar, 1.5, p.field_border);
        ui.painter().rect_filled(
            egui::Rect::from_min_size(bar.min, Vec2::new(bar.width() * fraction, 3.)),
            1.5,
            p.accent,
        );
        response.on_hover_text(format!("Opening {name}"));
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(100));
        if cancel {
            self.gcode_loading = None;
            self.status = "Canceled".into();
        }
        true
    }
}

/// Mica's check of the file: running, clean, or its errors and warnings with the first
/// findings (errors first).
fn check_card(ui: &mut egui::Ui, check: &Check) {
    let p = theme::of(ui);
    match check {
        Check::Running => {
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(12.));
                ui.label(
                    egui::RichText::new("Checking the file…")
                        .font(theme::sans(12.5))
                        .color(p.muted),
                );
            });
        }
        Check::Failed(e) => {
            super::machine::warn_card(ui, "The file could not be checked", e);
        }
        Check::Done {
            errors,
            warnings,
            details,
            against,
        } => {
            let (headline, colour) = match (*errors, *warnings) {
                (0, 0) => ("Mica's check found no problems".to_string(), p.success),
                (0, w) => (format!("{w} {}", plural(w, "warning")), p.warn_text),
                (e, 0) => (format!("{e} {}", plural(e, "error")), theme::DANGER),
                (e, w) => (
                    format!("{e} {}, {w} {}", plural(e, "error"), plural(w, "warning")),
                    theme::DANGER,
                ),
            };
            ui.label(
                egui::RichText::new(headline)
                    .font(theme::medium(13.))
                    .color(colour),
            );
            ui.label(
                egui::RichText::new(format!("Checked against {against}"))
                    .font(theme::sans(11.))
                    .color(p.muted),
            );
            if !details.is_empty() {
                egui::CollapsingHeader::new(
                    egui::RichText::new(format!("Details ({})", details.len()))
                        .font(theme::sans(12.))
                        .color(p.label),
                )
                .id_salt("gcode-check-details")
                .default_open(*errors > 0)
                .show(ui, |ui| {
                    for (severity, text) in details {
                        ui.label(egui::RichText::new(text).font(theme::sans(11.5)).color(
                            match severity {
                                Severity::Error => theme::DANGER,
                                Severity::Warning => p.warn_text,
                            },
                        ));
                    }
                });
            }
        }
    }
}
