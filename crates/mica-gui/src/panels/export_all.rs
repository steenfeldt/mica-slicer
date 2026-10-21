// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The Export all plates window: the folder and the file each plate gets; when files of those
//! names are already there it asks before replacing them; then "Slicing plate 2 of 4" with
//! Cancel, and the result ("Exported 4 files to …", Show in Finder) or what went wrong. The
//! work itself is in `plate_batch`.

use super::widgets;
use crate::app::App;
use crate::plate_batch::{ExportStage, reveal_label};
use crate::theme;
use eframe::egui;

const WIDTH: f32 = 460.;

impl App {
    pub(crate) fn export_all_window(&mut self, ctx: &egui::Context) {
        let Some(e) = &self.export_all else { return };
        let dir = e.dir.clone();
        let plan = e.plan.clone();
        let position = self.batch_position();
        let progress = self.progress;
        let mut replace = false;
        let mut cancel = false;
        let mut close = false;
        let mut reveal = None;
        let mut open = true;
        let busy = e.stage.busy();
        egui::Window::new("Export all plates")
            .id(egui::Id::unique("export-all"))
            .open(&mut open)
            .default_width(WIDTH)
            .default_pos(ctx.content_rect().center() - egui::vec2(WIDTH / 2., 180.))
            .resizable(false)
            .collapsible(false)
            .show(ctx, |ui| {
                let p = theme::of(ui);
                ui.set_width(WIDTH);
                ui.spacing_mut().item_spacing.y = 8.;
                let line = |ui: &mut egui::Ui, text: &str, color: egui::Color32| {
                    ui.label(
                        egui::RichText::new(text)
                            .font(theme::sans(13.))
                            .color(color),
                    );
                };
                widgets::group_title(ui, "Folder");
                ui.label(
                    egui::RichText::new(dir.display().to_string())
                        .font(theme::mono(12.))
                        .color(p.text),
                );
                match &e.stage {
                    ExportStage::Confirm { existing } => {
                        files(ui, &plan, existing);
                        super::machine::warn_card(
                            ui,
                            &format!(
                                "{} {} already in this folder",
                                count(existing.len(), "file"),
                                if existing.len() == 1 { "is" } else { "are" }
                            ),
                            &format!(
                                "Replace {} with the new {}? Other files in the folder stay.",
                                if existing.len() == 1 { "it" } else { "them" },
                                if existing.len() == 1 { "file" } else { "files" }
                            ),
                        );
                        ui.horizontal(|ui| {
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if widgets::primary_button(ui, "Replace", true).clicked() {
                                        replace = true;
                                    }
                                    if widgets::button(ui, "Cancel", true).clicked() {
                                        close = true;
                                    }
                                },
                            );
                        });
                    }
                    ExportStage::Slicing => {
                        files(ui, &plan, &[]);
                        let text = match position {
                            Some((k, n)) => {
                                format!("Slicing plate {k} of {n} · {:.0} %", progress * 100.)
                            }
                            None => "Preparing…".into(),
                        };
                        line(ui, &text, p.label);
                        let (row, _) = ui.allocate_exact_size(
                            egui::vec2(ui.available_width(), 6.),
                            egui::Sense::hover(),
                        );
                        let fraction = match position {
                            Some((k, n)) => ((k - 1) as f32 + progress) / n.max(1) as f32,
                            None => 0.,
                        };
                        super::machine::bar(ui, row, fraction, p.accent);
                        ui.horizontal(|ui| {
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if widgets::button(ui, "Cancel", true)
                                        .on_hover_text("Stop slicing; nothing is written")
                                        .clicked()
                                    {
                                        cancel = true;
                                    }
                                },
                            );
                        });
                    }
                    ExportStage::Writing { .. } => {
                        files(ui, &plan, &[]);
                        line(ui, "Writing the files…", p.label);
                    }
                    ExportStage::Done { files, skipped } => {
                        line(
                            ui,
                            &format!("Exported {}:", count(files.len(), "file")),
                            p.text,
                        );
                        for f in files {
                            ui.label(
                                egui::RichText::new(
                                    f.file_name()
                                        .map(|n| n.to_string_lossy().into_owned())
                                        .unwrap_or_default(),
                                )
                                .font(theme::mono(12.))
                                .color(p.label),
                            );
                        }
                        if !skipped.is_empty() {
                            super::machine::warn_card(
                                ui,
                                &format!("{} not exported", count(skipped.len(), "plate")),
                                &skipped.join("\n"),
                            );
                        }
                        ui.horizontal(|ui| {
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if widgets::primary_button(ui, "Done", true).clicked() {
                                        close = true;
                                    }
                                    if let Some(first) = files.first()
                                        && widgets::button(ui, reveal_label(), true).clicked()
                                    {
                                        reveal = Some(first.clone());
                                    }
                                },
                            );
                        });
                    }
                    ExportStage::Failed(why) => {
                        super::machine::warn_card(ui, "Nothing was exported", why);
                        ui.horizontal(|ui| {
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if widgets::button(ui, "Close", true).clicked() {
                                        close = true;
                                    }
                                },
                            );
                        });
                    }
                    ExportStage::Canceled => {
                        line(ui, "Canceled; no file was written.", p.label);
                        ui.horizontal(|ui| {
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if widgets::button(ui, "Close", true).clicked() {
                                        close = true;
                                    }
                                },
                            );
                        });
                    }
                }
            });
        if replace {
            self.export_all_start(ctx);
        }
        if cancel {
            self.cancel_slice();
        }
        if let Some(path) = reveal {
            self.run_notification_action(0, crate::notifications::Action::Reveal(path), ctx);
        }
        // Closing the window while it slices cancels; while it writes, the copy finishes.
        if (!open || close) && !busy {
            self.export_all = None;
        } else if !open && busy {
            self.cancel_slice();
        }
    }
}

/// "1 file", "3 files".
fn count(n: usize, what: &str) -> String {
    format!("{n} {what}{}", if n == 1 { "" } else { "s" })
}

/// The files the export writes, one per plate; those that replace a file are marked.
fn files(ui: &mut egui::Ui, plan: &[(usize, String)], existing: &[String]) {
    let p = theme::of(ui);
    widgets::group_title(
        ui,
        &format!("{} · one per plate", count(plan.len(), "file")),
    );
    for (i, name) in plan {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(format!("{}", i + 1))
                    .font(theme::sans(12.))
                    .color(p.muted),
            );
            ui.label(
                egui::RichText::new(name)
                    .font(theme::mono(12.))
                    .color(p.text),
            );
            if existing.contains(name) {
                ui.label(
                    egui::RichText::new("replaces a file")
                        .font(theme::sans(11.5))
                        .color(p.warn_text),
                );
            }
        });
    }
}
