// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The Print dialog: choose the printer (the ones fitting the current profile first), see the
//! preflight (idle or busy, tools, the job's filament next to the loaded one, file format),
//! "Start printing after upload" and the file name, then Upload with a progress bar. Errors
//! are plain language with what to do; success closes the dialog and shows a toast with a
//! link to the Machine view.
//!
//! For a Snapmaker U1 the filament table becomes the slot map: per used tool, the project's
//! swatch → a dropdown of the printer's slots (proposed by type and nearest color), with
//! type mismatches marked and empty slots blocking the start; plus the start options
//! (bed leveling, flow calibration for chosen tools, timelapse).

use super::{
    machine::{state_colour, warn_card},
    widgets,
};
use crate::app::App;
use crate::printhost::{
    config::{Fit, Kind},
    explain::{JobTool, Verdict},
    job::{Dialog, JobFile, Stage},
};
use crate::theme;
use eframe::egui::{self, Align2, Pos2, Rect, Sense, Stroke, Vec2};
use mica_printhost::FileFormat;

const DIALOG_W: f32 = 560.;

fn megabytes(bytes: u64) -> String {
    let mb = bytes as f64 / 1_048_576.;
    if mb < 10. {
        format!("{mb:.1} MB")
    } else {
        format!("{mb:.0} MB")
    }
}

impl App {
    /// "Print…": builds the job from the sliced plate and opens the dialog on the profile's
    /// default printer.
    pub(crate) fn open_print_dialog(&mut self) {
        // An opened G-code file is sent as it is.
        if self.viewing_gcode() {
            self.open_gcode_print_dialog();
            return;
        }
        if self.reslice_for_colours() {
            self.dialogs.print = false;
            return;
        }
        let Some(f) = self.finished() else {
            self.status = "Slice the plate first".into();
            return;
        };
        let file = self.job_file(f);
        let mut dialog = Dialog::new(file, self.export_stem());
        // Several plates with objects: This plate / All plates.
        let count = (0..self.plates.len())
            .filter(|&i| self.sliceable(i))
            .count();
        if count > 1 {
            dialog.plates = Some(crate::printhost::job::Plates::new(count));
        }
        if let Some(i) = self.linked_printer() {
            dialog.choose(self.printers.target(i));
            dialog.u1_remembered = self.remembered_u1(dialog.target.as_ref());
        }
        self.print_job = Some(dialog);
        self.dialogs.print = true;
    }

    /// The file Print sends for a sliced plate, with what it needs of the printer.
    pub(crate) fn job_file(&self, f: &crate::worker::Finished) -> JobFile {
        let format = if f.binary {
            FileFormat::Bgcode
        } else {
            FileFormat::Gcode
        };
        let ext = match format {
            FileFormat::Bgcode => "bgcode",
            FileFormat::Gcode => "gcode",
        };
        let path = f.preview.job.path.join(format!("print.{ext}"));
        let hold = Some(f.preview.job.clone());
        let colours = self.tool_colours();
        let config = self.effective_config();
        let mut used: Vec<usize> = f.summary.tools.iter().map(|t| t.tool).collect();
        if used.is_empty() {
            used.push(0);
        }
        let tools: Vec<JobTool> = used
            .iter()
            .map(|&t| JobTool {
                tool: t as u32,
                preset: self
                    .profiles
                    .filaments
                    .get(t)
                    .map(|n| crate::preset_names::display_name(n).to_string())
                    .unwrap_or_default(),
                material: config
                    .and_then(|c| c.string_at("filament_type", t))
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty()),
                colour: colours.get(t).copied(),
            })
            .collect();
        JobFile {
            path,
            format,
            tools,
            bed_type: None,
            hold,
        }
    }

    /// The start options last sent to this U1, if any.
    pub(crate) fn remembered_u1(
        &self,
        target: Option<&crate::printhost::job::Target>,
    ) -> Option<crate::printhost::u1map::Remembered> {
        let t = target.filter(|t| t.kind == crate::printhost::config::Kind::SnapmakerU1)?;
        self.prefs.u1_start.get(&t.name).copied()
    }

    pub(crate) fn start_check(&mut self, ctx: &egui::Context) {
        let Some(d) = &mut self.print_job else { return };
        let Some(t) = &d.target else { return };
        let connect = match self.printers.position(t.id) {
            Some(i) => self.printers.connector(i),
            None => Err(format!("{} is no longer in My printers", t.name)),
        };
        let c = ctx.clone();
        d.check(connect, move || c.request_repaint());
    }

    pub(crate) fn start_upload(&mut self, ctx: &egui::Context) {
        // Remember the U1 start options for this printer, whatever the outcome.
        if let Some(d) = &self.print_job
            && d.start_after
            && let (Some(plan), Some(t)) = (&d.u1, &d.target)
        {
            let r = plan.remembered();
            if self.prefs.u1_start.get(&t.name) != Some(&r) {
                self.prefs.u1_start.insert(t.name.clone(), r);
                self.prefs_dirty = true;
            }
        }
        let reports = self.identity_reports.0.clone();
        let Some(d) = &mut self.print_job else { return };
        d.identity_reports = Some(reports);
        let Some(t) = &d.target else { return };
        let connect = match self.printers.position(t.id) {
            Some(i) => self.printers.connector(i),
            None => Err(format!("{} is no longer in My printers", t.name)),
        };
        let c = ctx.clone();
        d.upload(connect, move || c.request_repaint());
    }

    /// All plates: sends every plate's file that is not sent yet (or failed).
    pub(crate) fn start_upload_all(&mut self, ctx: &egui::Context) {
        let Some(d) = &mut self.print_job else { return };
        let Some(t) = &d.target else { return };
        let connect = match self.printers.position(t.id) {
            Some(i) => self.printers.connector(i),
            None => Err(format!("{} is no longer in My printers", t.name)),
        };
        let c = ctx.clone();
        d.upload_all(connect, move || c.request_repaint());
    }

    /// Every plate was sent (or skipped): the dialog closes with a notification.
    pub(crate) fn finish_upload_all(&mut self) {
        use crate::printhost::job::ItemState;
        let Some(d) = &self.print_job else { return };
        let (Some(target), Some(pl)) = (&d.target, &d.plates) else {
            return;
        };
        let sent = pl.done();
        let started = pl
            .items
            .iter()
            .find(|i| matches!(i.state, ItemState::Done { started: true, .. }))
            .map(|i| i.plate.clone());
        let queued = pl
            .items
            .iter()
            .filter(|i| matches!(i.state, ItemState::Done { queued: true, .. }))
            .count();
        let warning = pl
            .items
            .iter()
            .any(|i| matches!(i.state, ItemState::Done { warning: true, .. }));
        let skipped: Vec<String> = pl
            .items
            .iter()
            .filter(|i| i.state == ItemState::Skipped)
            .map(|i| i.plate.clone())
            .chain(pl.skipped.iter().cloned())
            .collect();
        let plates = if sent == 1 {
            "1 plate".to_string()
        } else {
            format!("{sent} plates")
        };
        let mut text = if target.kind == Kind::PrusaConnect {
            let mut t = format!("Prusa Connect accepted {plates} for {}", target.name);
            if d.start_after {
                t.push_str(" · print of the first requested");
            }
            if queued > 0 {
                t.push_str(&format!(" · {queued} in the queue"));
            }
            t.push_str(". Printing is not confirmed.");
            t
        } else {
            let mut t = format!("Uploaded {plates} to {}", target.name);
            match &started {
                Some(plate) => t.push_str(&format!(" · {plate} printing")),
                None if d.start_after => {
                    t.push_str(" · the printer did not confirm a start; check its screen")
                }
                None => {}
            }
            t
        };
        if !skipped.is_empty() {
            text.push_str(&format!(" · not sent: {}", skipped.join(", ")));
        }
        let id = target.id;
        let level = if warning || !skipped.is_empty() {
            crate::notifications::Level::Warning
        } else {
            crate::notifications::Level::Info
        };
        self.notify(
            crate::notifications::Notification::new(level, text.clone(), text).action(
                "Open Machine view",
                crate::notifications::Action::OpenMachine(id),
            ),
        );
        self.dialogs.print = false;
        self.print_job = None;
    }

    /// Upload / Upload and print: an opened G-code file with validation errors asks first
    /// (warnings never do); everything else uploads.
    pub(crate) fn request_upload(&mut self, ctx: &egui::Context) {
        let errors = self.gcode.as_ref().map_or(0, |d| d.check.errors());
        let Some(d) = &mut self.print_job else { return };
        if errors > 0 && !d.errors_accepted {
            d.asking_errors = true;
            return;
        }
        self.start_upload(ctx);
    }

    /// "Send anyway" after the errors question.
    pub(crate) fn send_despite_errors(&mut self, ctx: &egui::Context) {
        if let Some(d) = &mut self.print_job {
            d.asking_errors = false;
            d.errors_accepted = true;
        }
        self.start_upload(ctx);
    }

    /// The dialog itself.
    pub(crate) fn print_dialog(&mut self, ctx: &egui::Context) {
        if self.print_job.is_none() {
            self.open_print_dialog();
        }
        let Some(d) = &self.print_job else {
            self.dialogs.print = false;
            return;
        };
        // A chosen printer is checked as soon as the dialog shows it.
        if d.target.is_some() && d.stage == Stage::Pick {
            self.start_check(ctx);
        }
        let vendor = self.profiles.vendor_name().to_string();
        let preset = self.profiles.printer.clone();
        let ranked = self.printers.ranked(&vendor, &preset);
        // An opened G-code file: a warning when the chosen printer is another model, and the
        // validator's errors (sending asks first; warnings never block).
        let mismatch = self.print_job.as_ref().and_then(|d| {
            let t = d.target.as_ref()?;
            self.gcode_mismatch(t.id, d.reported_model.as_deref())
        });
        let (file_errors, checking) = self.gcode.as_ref().map_or((0, false), |doc| {
            (
                doc.check.errors(),
                matches!(doc.check, crate::gcode_doc::Check::Running),
            )
        });
        let batch = self.batch_position();
        let slice_progress = self.progress;
        let mut plates_choice: Option<bool> = None;
        let mut upload_all = false;
        let mut send_anyway = false;
        let mut open = self.dialogs.print;
        let mut pick: Option<Option<usize>> = None;
        let mut upload = false;
        let mut retry = false;
        let mut close = false;
        let mut add_printer = false;
        let d = self.print_job.as_mut().expect("dialog state");
        let uploading = matches!(d.stage, Stage::Uploading { .. });
        egui::Window::new("Send to printer")
            .id(egui::Id::unique("print-dialog"))
            .open(&mut open)
            .default_width(DIALOG_W)
            .default_pos(ctx.content_rect().center() - egui::vec2(DIALOG_W / 2., 260.))
            .resizable(false)
            .collapsible(false)
            .show(ctx, |ui| {
                let p = theme::of(ui);
                ui.set_width(DIALOG_W);
                ui.spacing_mut().item_spacing.y = 10.;
                ui.add_space(2.);
                // Printer picker.
                widgets::group_title(ui, "Printer");
                if self.printers.list.is_empty() {
                    ui.label(
                        egui::RichText::new(
                            "No printers yet. Add your PrusaLink, Klipper or Snapmaker U1 printer first.",
                        )
                        .font(theme::sans(13.))
                        .color(p.label),
                    );
                    if widgets::primary_button(ui, "Add a printer…", true).clicked() {
                        add_printer = true;
                    }
                } else {
                    let id = ui.make_persistent_id("print-printer");
                    let (name, trailing) = match &d.target {
                        Some(t) => (t.name.clone(), t.kind.label().to_string()),
                        None => ("Choose a printer".to_string(), String::new()),
                    };
                    let response = widgets::combo_button(
                        ui,
                        id,
                        |ui| {
                            let (r, _) = ui.allocate_exact_size(Vec2::splat(18.), Sense::hover());
                            widgets::glyph(ui.painter(), r, widgets::Glyph::Printer, p.muted_alt);
                        },
                        &name,
                        &trailing,
                        None,
                    );
                    egui::Popup::menu(&response)
                        .id(id.with("menu"))
                        .width(response.rect.width())
                        .show(|ui| {
                            let fitting: Vec<&(usize, Fit)> =
                                ranked.iter().filter(|(_, f)| *f != Fit::Other).collect();
                            let others: Vec<&(usize, Fit)> =
                                ranked.iter().filter(|(_, f)| *f == Fit::Other).collect();
                            if !fitting.is_empty() {
                                ui.weak(format!("For {}", super::sidebar::split_printer_name(&preset).0));
                            }
                            for (i, fit) in &fitting {
                                let pr = &self.printers.list[*i];
                                let label = match fit {
                                    Fit::Linked => format!("{}  ·  linked", pr.name),
                                    _ => format!("{}  ·  {}", pr.name, pr.kind.label()),
                                };
                                if ui
                                    .selectable_label(d.target.as_ref().is_some_and(|t| t.id == pr.id), label)
                                    .clicked()
                                {
                                    pick = Some(Some(*i));
                                    ui.close();
                                }
                            }
                            if !others.is_empty() {
                                if !fitting.is_empty() {
                                    ui.separator();
                                }
                                ui.weak("Other printers");
                                for (i, _) in others {
                                    let pr = &self.printers.list[*i];
                                    if ui
                                        .selectable_label(
                                            d.target.as_ref().is_some_and(|t| t.id == pr.id),
                                            format!("{}  ·  {}", pr.name, pr.kind.label()),
                                        )
                                        .clicked()
                                    {
                                        pick = Some(Some(*i));
                                        ui.close();
                                    }
                                }
                            }
                            ui.separator();
                            if ui.button("My printers…").clicked() {
                                add_printer = true;
                                ui.close();
                            }
                        });
                }
                if let Some(w) = &mismatch {
                    warn_card(ui, "Made for another printer", w);
                }
                // This plate / All plates.
                if let Some(pl) = &d.plates {
                    widgets::group_title(ui, "Plates");
                    let mut all = pl.all;
                    ui.add_enabled_ui(!uploading && !pl.sending, |ui| {
                        ui.horizontal(|ui| {
                            ui.selectable_value(&mut all, false, "This plate");
                            ui.selectable_value(&mut all, true, format!("All plates ({})", pl.count))
                                .on_hover_text("Send every plate's file to the printer, one after the other");
                        });
                    });
                    if all != pl.all {
                        plates_choice = Some(all);
                    }
                    if pl.all {
                        if pl.preparing {
                            ui.label(
                                egui::RichText::new(match batch {
                                    Some((k, n)) => format!("Slicing plate {k} of {n} · {:.0} %", slice_progress * 100.),
                                    None => "Preparing the plates' files…".into(),
                                })
                                .font(theme::sans(13.))
                                .color(p.label),
                            );
                        }
                        if let Some(why) = &pl.problem {
                            warn_card(ui, "The plates are not ready", why);
                        }
                        let connect = d.target.as_ref().is_some_and(|t| t.kind == Kind::PrusaConnect);
                        for (i, item) in pl.items.iter().enumerate() {
                            item_row(ui, item, i == 0 && d.start_after, connect);
                        }
                        if !pl.skipped.is_empty() {
                            warn_card(ui, "Not included", &pl.skipped.join("\n"));
                        }
                    }
                }
                let all_plates = d.sending_all() || d.plates.as_ref().is_some_and(|p| p.all);
                // Preflight.
                if d.target.is_some() {
                    widgets::group_title(
                        ui,
                        &match d.plates.as_ref().filter(|p| p.sending_all()) {
                            Some(p) => format!(
                                "Before sending · checked with {}",
                                p.items.first().map_or("the first plate", |i| i.plate.as_str())
                            ),
                            None => "Before sending".to_string(),
                        },
                    );
                    match &d.stage {
                        Stage::Pick | Stage::Checking => {
                            ui.label(
                                egui::RichText::new("Asking the printer…")
                                    .font(theme::sans(13.))
                                    .color(p.muted),
                            );
                        }
                        Stage::CheckFailed(x) => {
                            warn_card(ui, &x.headline, &x.advice);
                            if widgets::button(ui, "Try again", true).clicked() {
                                retry = true;
                            }
                        }
                        Stage::CloudReady => {
                            warn_card(ui, "Send via Connect", "Printer readiness, loaded tools, materials and bed are unknown. Verify them in Prusa Connect. Acceptance does not confirm that printing has started.");
                            if let Some(url) = d.target.as_ref().and_then(|t| self.printers.position(t.id)).and_then(|i| self.printers.list[i].connect_page())
                                && widgets::button(ui, "Open in Prusa Connect", true).clicked() { ui.ctx().open_url(egui::OpenUrl::new_tab(url)); }
                        }
                        Stage::Checked(pre) => preflight_card(ui, pre, &d.file.tools),
                        Stage::Uploading { .. } | Stage::Done { .. } | Stage::Failed(_) => {
                            if let Some(pre) = d.preflight() {
                                preflight_card(ui, pre, &d.file.tools);
                            }
                        }
                    }
                }
                if let Some(plan) = &mut d.u1 {
                    widgets::group_title(ui, "Filament slots");
                    ui.add_enabled_ui(!uploading && d.start_after, |ui| slot_map(ui, plan));
                }
                // Options.
                widgets::group_title(ui, "Options");
                let sending = d.plates.as_ref().is_some_and(|p| p.sending);
                ui.add_enabled_ui(!uploading && !sending, |ui| {
                    if all_plates {
                        if d.target.as_ref().is_some_and(|t| t.kind == Kind::PrusaConnect) {
                            ui.horizontal(|ui| {
                                ui.selectable_value(&mut d.start_after, true, "Print the first, queue the rest");
                                ui.selectable_value(&mut d.start_after, false, "Add all to the queue");
                            });
                        } else {
                            ui.checkbox(&mut d.start_after, "Start printing the first plate after upload")
                                .on_hover_text("The other plates' files are uploaded; start them from the printer or the Machine view");
                        }
                        if let Some(plan) = &mut d.u1 {
                            ui.add_enabled_ui(d.start_after, |ui| start_options(ui, plan));
                        }
                        return;
                    }
                    if d.target.as_ref().is_some_and(|t| t.kind == Kind::PrusaConnect) {
                        let mut action = if d.queue { 2 } else if d.start_after { 1 } else { 0 };
                        ui.horizontal(|ui| {
                            ui.selectable_value(&mut action, 0, "Upload only");
                            ui.selectable_value(&mut action, 1, "Upload and print");
                            ui.selectable_value(&mut action, 2, "Add to queue");
                        });
                        d.queue = action == 2;
                        d.start_after = action == 1;
                    } else {
                        ui.checkbox(&mut d.start_after, "Start printing after upload");
                    }
                    if let Some(plan) = &mut d.u1 {
                        ui.add_enabled_ui(d.start_after, |ui| start_options(ui, plan));
                    }
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new("File name")
                                .font(theme::sans(13.))
                                .color(p.label),
                        );
                        ui.add(
                            egui::TextEdit::singleline(&mut d.file_name)
                                .font(theme::mono(13.))
                                .desired_width(300.),
                        );
                        ui.label(
                            egui::RichText::new(match d.file.format {
                                FileFormat::Bgcode => ".bgcode",
                                FileFormat::Gcode => ".gcode",
                            })
                            .font(theme::mono(13.))
                            .color(p.muted),
                        );
                    });
                });
                // Progress / result.
                match &d.stage {
                    Stage::Uploading { sent, total } => {
                        let fraction = if *total > 0 {
                            *sent as f32 / *total as f32
                        } else {
                            0.
                        };
                        ui.add_space(2.);
                        ui.label(
                            egui::RichText::new(if *total > 0 {
                                format!(
                                    "Uploading {} · {} of {}",
                                    d.remote_name(),
                                    megabytes(*sent),
                                    megabytes(*total)
                                )
                            } else {
                                "Connecting…".to_string()
                            })
                            .font(theme::sans(13.))
                            .color(p.label),
                        );
                        let (row, _) =
                            ui.allocate_exact_size(Vec2::new(ui.available_width(), 8.), Sense::hover());
                        super::machine::bar(ui, row, fraction, p.accent);
                    }
                    Stage::Failed(x) => warn_card(ui, &x.headline, &x.advice),
                    _ => {}
                }
                if d.start_after
                    && let Some(plan) = &d.u1
                    && !plan.errors().is_empty()
                    && !uploading
                {
                    ui.label(
                        egui::RichText::new("Choose a loaded slot for every tool to start the print.")
                            .font(theme::sans(12.))
                            .color(theme::DANGER),
                    );
                }
                if d.asking_errors && !uploading {
                    warn_card(
                        ui,
                        &format!(
                            "This file has {file_errors} G-code {}",
                            if file_errors == 1 { "error" } else { "errors" }
                        ),
                        "Mica's check found problems that can spoil the print or harm the printer (the sidebar lists them). Send it anyway?",
                    );
                    ui.horizontal(|ui| {
                        if widgets::button(ui, "Send anyway", true).clicked() {
                            send_anyway = true;
                        }
                        if widgets::button(ui, "Don't send", true).clicked() {
                            d.asking_errors = false;
                        }
                    });
                }
                if checking && !uploading {
                    ui.label(
                        egui::RichText::new("Checking the file before it can be sent…")
                            .font(theme::sans(12.))
                            .color(p.muted),
                    );
                }
                ui.add_space(4.);
                if all_plates {
                    ui.horizontal(|ui| {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.spacing_mut().item_spacing.x = 8.;
                            let (n, failed) = d.plates.as_ref().map_or((0, 0), |p| (p.to_send(), p.failed()));
                            let connect = d.target.as_ref().is_some_and(|t| t.kind == Kind::PrusaConnect);
                            let label = if sending {
                                "Sending…".to_string()
                            } else if failed > 0 {
                                format!("Retry {}", if n == 1 { "1 file".to_string() } else { format!("{n} files") })
                            } else if connect {
                                format!("Send {n} plates via Connect")
                            } else if d.start_after {
                                format!("Upload {n} plates and print the first")
                            } else {
                                format!("Upload {n} plates")
                            };
                            if widgets::primary_button(ui, &label, d.can_upload_all() && !checking).clicked() {
                                upload_all = true;
                            }
                            if failed > 0 && !sending && widgets::button(ui, "Skip failed", true)
                                .on_hover_text("Leave the failed files out and finish")
                                .clicked()
                            {
                                d.skip_failed();
                            }
                            if widgets::button(ui, "Close", !sending).clicked() {
                                close = true;
                            }
                        });
                    });
                    ui.add_space(2.);
                    return;
                }
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 8.;
                        let label = if d.target.as_ref().is_some_and(|t| t.kind == Kind::PrusaConnect) { "Send via Connect" } else { match (&d.stage, d.start_after) {
                            (Stage::Failed(_), true) => "Try again",
                            (Stage::Failed(_), false) => "Try again",
                            (_, true) => "Upload and print",
                            (_, false) => "Upload",
                        }};
                        if widgets::primary_button(
                            ui,
                            label,
                            d.can_upload() && !uploading && !checking && !d.asking_errors,
                        )
                        .clicked()
                        {
                            upload = true;
                        }
                        if widgets::button(ui, "Close", !uploading).clicked() {
                            close = true;
                        }
                        if let Stage::Checked(pre) = &d.stage
                            && !pre.ready
                        {
                            ui.label(
                                egui::RichText::new("The printer is busy: the file is uploaded, printing starts only when it is free")
                                    .font(theme::sans(11.))
                                    .color(p.warn_dim),
                            );
                        }
                    });
                });
                ui.add_space(2.);
            });
        if let Some(choice) = pick {
            let target = choice.and_then(|i| self.printers.target(i));
            let remembered = self.remembered_u1(target.as_ref());
            if let Some(d) = &mut self.print_job {
                d.choose(target);
                d.u1_remembered = remembered;
            }
        }
        if retry {
            self.start_check(ctx);
        }
        if let Some(all) = plates_choice {
            if let Some(d) = &mut self.print_job {
                d.choose_all(all);
            }
            if all {
                self.prepare_send_all(ctx);
            } else if self
                .slice_batch
                .as_ref()
                .is_some_and(|b| b.then == crate::plate_batch::Then::Send)
            {
                self.cancel_slice();
            }
        }
        if upload_all {
            self.start_upload_all(ctx);
        }
        if send_anyway {
            self.send_despite_errors(ctx);
        } else if upload {
            self.request_upload(ctx);
        }
        if add_printer {
            self.dialogs.my_printers = true;
        }
        self.dialogs.print = open && !close;
        if !self.dialogs.print
            && self
                .slice_batch
                .as_ref()
                .is_some_and(|b| b.then == crate::plate_batch::Then::Send)
        {
            // Closing while the plates slice for it stops that.
            self.cancel_slice();
        }
        if !self.dialogs.print {
            // Closing while an upload runs is not offered; otherwise forget the dialog.
            self.print_job = None;
        }
    }
}

/// One file of All plates: its state's mark, the plate and file name, and where it stands
/// (waiting, the upload's progress, printing / in the queue / uploaded, failed with the
/// reason on hover, skipped).
fn item_row(
    ui: &mut egui::Ui,
    item: &crate::printhost::job::BatchItem,
    starts: bool,
    connect: bool,
) {
    use crate::plate_batch::{PlateStatus, status_mark};
    use crate::printhost::job::ItemState;
    let p = theme::of(ui);
    let (row, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 24.), Sense::hover());
    let mark = match &item.state {
        ItemState::Waiting => PlateStatus::Queued,
        ItemState::Uploading { sent, total } => PlateStatus::Slicing {
            progress: if *total > 0 {
                *sent as f32 / *total as f32
            } else {
                0.
            },
        },
        ItemState::Done { .. } => PlateStatus::Sliced {
            time_s: 0.,
            grams: 0.,
            filament_mm: 0.,
        },
        ItemState::Failed(_) => PlateStatus::Failed {
            short: String::new(),
            message: String::new(),
        },
        ItemState::Skipped => PlateStatus::NotSliced,
    };
    status_mark(
        ui.painter(),
        Pos2::new(row.left() + 7., row.center().y),
        &mark,
    );
    let (text, colour, tip) = match &item.state {
        ItemState::Waiting if starts => ("waiting · will print".to_string(), p.muted_alt, None),
        ItemState::Waiting if connect => {
            ("waiting · will be queued".to_string(), p.muted_alt, None)
        }
        ItemState::Waiting => ("waiting".to_string(), p.muted_alt, None),
        ItemState::Uploading { sent, total } if *total > 0 => (
            format!("{} of {}", megabytes(*sent), megabytes(*total)),
            p.label,
            None,
        ),
        ItemState::Uploading { .. } => ("connecting…".to_string(), p.label, None),
        ItemState::Done {
            started,
            queued,
            message,
            warning,
        } => (
            format!(
                "{}{}",
                if *started {
                    "printing"
                } else if *queued {
                    "in the queue"
                } else if starts && connect {
                    "print requested"
                } else {
                    "uploaded"
                },
                if *warning { " · warning" } else { "" }
            ),
            p.text,
            message.clone(),
        ),
        ItemState::Failed(x) => (
            format!("failed: {}", x.headline),
            theme::DANGER,
            Some(x.advice.clone()),
        ),
        ItemState::Skipped => ("skipped".to_string(), p.muted, None),
    };
    let status_w = 210.;
    let name_rect = Rect::from_min_max(
        Pos2::new(row.left() + 20., row.top()),
        Pos2::new(row.right() - status_w - 8., row.bottom()),
    );
    let clipped = ui.painter().with_clip_rect(name_rect);
    let g = clipped.layout_no_wrap(item.plate.clone(), theme::sans(13.), p.text);
    let name_w = g.size().x;
    clipped.galley(
        Pos2::new(name_rect.left(), row.center().y - g.size().y / 2.),
        g,
        p.text,
    );
    clipped.text(
        Pos2::new(name_rect.left() + name_w + 8., row.center().y),
        Align2::LEFT_CENTER,
        &item.remote,
        theme::mono(11.5),
        p.muted,
    );
    let mut job = egui::text::LayoutJob::simple_singleline(text, theme::sans(12.), colour);
    job.wrap = egui::text::TextWrapping::truncate_at_width(status_w);
    let g = ui.painter().layout_job(job);
    ui.painter().galley(
        Pos2::new(row.right() - g.size().x, row.center().y - g.size().y / 2.),
        g,
        colour,
    );
    if let ItemState::Uploading { sent, total } = &item.state
        && *total > 0
    {
        let bar = Rect::from_min_size(
            Pos2::new(name_rect.left(), row.bottom() - 3.),
            Vec2::new(row.width() - 20., 2.),
        );
        super::machine::bar(ui, bar, *sent as f32 / *total as f32, p.accent);
    }
    if let Some(tip) = tip {
        response.on_hover_text(tip);
    }
}

/// The preflight as a card: state, tools, format, the filament table and the warnings.
fn preflight_card(ui: &mut egui::Ui, pre: &crate::printhost::explain::Preflight, _job: &[JobTool]) {
    let p = theme::of(ui);
    egui::Frame::new()
        .fill(p.raised)
        .stroke(Stroke::new(1., p.border_raised))
        .corner_radius(8.)
        .inner_margin(egui::Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 6.;
            let line = |ui: &mut egui::Ui,
                        label: &str,
                        text: &str,
                        colour: egui::Color32,
                        dot: Option<egui::Color32>| {
                let (row, _) =
                    ui.allocate_exact_size(Vec2::new(ui.available_width(), 20.), Sense::hover());
                ui.painter().text(
                    Pos2::new(row.left(), row.center().y),
                    Align2::LEFT_CENTER,
                    label,
                    theme::sans(12.),
                    p.muted,
                );
                let mut x = row.left() + 72.;
                if let Some(c) = dot {
                    ui.painter()
                        .circle_filled(Pos2::new(x + 4., row.center().y), 4., c);
                    x += 14.;
                }
                ui.painter().text(
                    Pos2::new(x, row.center().y),
                    Align2::LEFT_CENTER,
                    text,
                    theme::sans(13.),
                    colour,
                );
            };
            let state_dot = Some(if pre.ready {
                p.accent_bright
            } else {
                state_colour(p, Some(pre.state))
            });
            line(
                ui,
                "Printer",
                &pre.state_text,
                if pre.ready { p.text } else { p.warn_text },
                state_dot,
            );
            line(ui, "Tools", &pre.tools_text, p.text, None);
            line(
                ui,
                "File",
                &pre.format_text,
                if pre.format_ok { p.text } else { theme::DANGER },
                None,
            );
            if !pre.rows.is_empty() {
                ui.add_space(4.);
                // Table header.
                let (h, _) =
                    ui.allocate_exact_size(Vec2::new(ui.available_width(), 16.), Sense::hover());
                let col_job = h.left() + 48.;
                let col_loaded = h.left() + (h.width() - 48.) / 2. + 48.;
                for (x, text) in [
                    (h.left(), "Tool"),
                    (col_job, "This job"),
                    (col_loaded, "Loaded on the printer"),
                ] {
                    ui.painter().text(
                        Pos2::new(x, h.center().y),
                        Align2::LEFT_CENTER,
                        widgets::spaced_upper(text),
                        theme::semibold(10.5),
                        p.muted,
                    );
                }
                for r in &pre.rows {
                    let (row, _) = ui
                        .allocate_exact_size(Vec2::new(ui.available_width(), 24.), Sense::hover());
                    ui.painter().text(
                        Pos2::new(row.left(), row.center().y),
                        Align2::LEFT_CENTER,
                        format!("T{}", r.tool + 1),
                        theme::mono(12.),
                        p.label,
                    );
                    // The job's side.
                    let mut x = col_job;
                    if let Some(c) = r.job.colour {
                        swatch_at(ui, Pos2::new(x, row.center().y), c);
                        x += 20.;
                    }
                    let job_text = match (&r.job.material, r.job.preset.is_empty()) {
                        (Some(m), false) => format!("{m} · {}", r.job.preset),
                        (Some(m), true) => m.clone(),
                        (None, false) => r.job.preset.clone(),
                        (None, true) => "—".into(),
                    };
                    let mut job = egui::text::LayoutJob::simple_singleline(
                        job_text,
                        theme::sans(12.5),
                        p.text,
                    );
                    job.wrap = egui::text::TextWrapping::truncate_at_width(col_loaded - x - 10.);
                    let g = ui.painter().layout_job(job);
                    ui.painter()
                        .galley(Pos2::new(x, row.center().y - g.size().y / 2.), g, p.text);
                    // The printer's side.
                    let mut x = col_loaded;
                    if let Some(c) = r.loaded_colour {
                        swatch_at(ui, Pos2::new(x, row.center().y), c);
                        x += 20.;
                    }
                    let (text, colour) = match r.verdict {
                        Verdict::Missing => ("no such tool".to_string(), theme::DANGER),
                        Verdict::Unknown => ("not reported".to_string(), p.muted),
                        Verdict::Match | Verdict::Differs => (
                            crate::printhost::explain::loaded_text(
                                r.loaded_material.as_deref(),
                                r.loaded_colour,
                            ),
                            p.text,
                        ),
                    };
                    ui.painter().text(
                        Pos2::new(x, row.center().y),
                        Align2::LEFT_CENTER,
                        &text,
                        theme::sans(12.5),
                        colour,
                    );
                    // Verdict mark at the right edge.
                    match r.verdict {
                        Verdict::Match => {
                            let c = Pos2::new(row.right() - 10., row.center().y);
                            let s = Stroke::new(1.8, p.accent_bright);
                            ui.painter().line_segment(
                                [Pos2::new(c.x - 5., c.y), Pos2::new(c.x - 1.5, c.y + 3.5)],
                                s,
                            );
                            ui.painter().line_segment(
                                [
                                    Pos2::new(c.x - 1.5, c.y + 3.5),
                                    Pos2::new(c.x + 5., c.y - 3.5),
                                ],
                                s,
                            );
                        }
                        Verdict::Differs => {
                            let g = ui.painter().layout_no_wrap(
                                "differs".into(),
                                theme::medium(11.),
                                p.warn_text,
                            );
                            let chip = Rect::from_min_size(
                                Pos2::new(row.right() - g.size().x - 14., row.center().y - 9.),
                                Vec2::new(g.size().x + 14., 18.),
                            );
                            ui.painter().rect_filled(chip, 9., p.warn_surface);
                            ui.painter().rect_stroke(
                                chip,
                                9.,
                                Stroke::new(1., p.warn_border),
                                egui::StrokeKind::Inside,
                            );
                            ui.painter().galley(
                                Pos2::new(chip.left() + 7., chip.center().y - g.size().y / 2.),
                                g,
                                p.warn_text,
                            );
                        }
                        _ => {}
                    }
                }
            }
            if !pre.warnings.is_empty() {
                ui.add_space(4.);
                egui::Frame::new()
                    .fill(p.warn_surface)
                    .stroke(Stroke::new(1., p.warn_border))
                    .corner_radius(6.)
                    .inner_margin(egui::Margin::symmetric(10, 8))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.spacing_mut().item_spacing.y = 3.;
                        for w in &pre.warnings {
                            ui.label(
                                egui::RichText::new(format!("•  {w}"))
                                    .font(theme::sans(12.))
                                    .color(p.warn_text),
                            );
                        }
                    });
            }
        });
}

/// The U1 slot map: one row per used tool, `T1 [swatch] PLA · preset → [slot dropdown]`,
/// then the mismatch warnings and empty-slot errors.
fn slot_map(ui: &mut egui::Ui, plan: &mut crate::printhost::u1map::Plan) {
    let p = theme::of(ui);
    let mut pick: Option<(usize, u32)> = None;
    let slot_label = slot_text;
    let slot_rgb =
        |s: &mica_printhost::U1Slot| s.colour.as_deref().and_then(mica_core::color::parse_hex);
    ui.spacing_mut().item_spacing.y = 6.;
    for (i, row) in plan.rows.iter().enumerate() {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.;
            let (r, _) = ui.allocate_exact_size(Vec2::new(26., widgets::COMBO_H), Sense::hover());
            ui.painter().text(
                Pos2::new(r.left(), r.center().y),
                Align2::LEFT_CENTER,
                format!("T{}", row.job.tool + 1),
                theme::mono(12.),
                p.label,
            );
            // The project's side.
            let (r, _) = ui.allocate_exact_size(Vec2::new(196., widgets::COMBO_H), Sense::hover());
            let mut x = r.left();
            if let Some(c) = row.job.colour {
                swatch_at(ui, Pos2::new(x, r.center().y), c);
                x += 20.;
            }
            let text = match (&row.job.material, row.job.preset.is_empty()) {
                (Some(m), false)
                    if !row
                        .job
                        .preset
                        .to_ascii_uppercase()
                        .contains(&m.to_ascii_uppercase()) =>
                {
                    format!("{m} · {}", row.job.preset)
                }
                (_, false) => row.job.preset.clone(),
                (Some(m), true) => m.clone(),
                (None, true) => "—".into(),
            };
            let mut job = egui::text::LayoutJob::simple_singleline(text, theme::sans(12.5), p.text);
            job.wrap = egui::text::TextWrapping::truncate_at_width(r.right() - x - 4.);
            let g = ui.painter().layout_job(job);
            ui.painter()
                .galley(Pos2::new(x, r.center().y - g.size().y / 2.), g, p.text);
            // The arrow.
            let (r, _) = ui.allocate_exact_size(Vec2::new(16., widgets::COMBO_H), Sense::hover());
            ui.painter().text(
                r.center(),
                Align2::CENTER_CENTER,
                "→",
                theme::sans(14.),
                p.muted,
            );
            // The printer's slot.
            let chosen = row
                .slot
                .and_then(|s| plan.slots.iter().find(|x| x.index == s));
            let (trailing, colour) = if row.issues.iter().any(|i| i.is_error()) {
                ("empty", Some(theme::DANGER))
            } else if !row.issues.is_empty() {
                ("type differs", Some(p.warn_text))
            } else {
                ("", None)
            };
            let id = ui.make_persistent_id(("u1-slot", row.job.tool));
            let response = ui
                .allocate_ui(Vec2::new(ui.available_width(), widgets::COMBO_H), |ui| {
                    widgets::combo_button(
                        ui,
                        id,
                        |ui| {
                            let (r, _) = ui.allocate_exact_size(Vec2::splat(14.), Sense::hover());
                            if let Some(c) = chosen.and_then(slot_rgb) {
                                swatch_at(ui, r.left_center(), c);
                            }
                        },
                        &chosen.map(slot_label).unwrap_or_else(|| "No slot".into()),
                        trailing,
                        colour,
                    )
                })
                .inner;
            egui::Popup::menu(&response)
                .id(id.with("menu"))
                .width(response.rect.width())
                .show(|ui| {
                    for s in &plan.slots {
                        let (r, resp) = ui.allocate_exact_size(
                            Vec2::new(ui.available_width(), 24.),
                            Sense::click(),
                        );
                        if resp.hovered() || row.slot == Some(s.index) {
                            ui.painter().rect_filled(r, 4., p.hover);
                        }
                        if let Some(c) = slot_rgb(s) {
                            swatch_at(ui, Pos2::new(r.left() + 6., r.center().y), c);
                        }
                        ui.painter().text(
                            Pos2::new(r.left() + 28., r.center().y),
                            Align2::LEFT_CENTER,
                            slot_label(s),
                            theme::sans(13.),
                            p.text,
                        );
                        let resp = match slot_rgb(s) {
                            Some(c) => resp.on_hover_text(mica_printhost::colour::hex(c)),
                            None => resp,
                        };
                        if resp.clicked() {
                            pick = Some((i, s.index));
                            ui.close();
                        }
                    }
                });
        });
    }
    if let Some((row, slot)) = pick {
        plan.choose(row, slot);
    }
    let (errors, warnings) = (plan.errors(), plan.warnings());
    if !errors.is_empty() || !warnings.is_empty() {
        egui::Frame::new()
            .fill(p.warn_surface)
            .stroke(Stroke::new(1., p.warn_border))
            .corner_radius(6.)
            .inner_margin(egui::Margin::symmetric(10, 8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 3.;
                for (text, colour) in errors
                    .iter()
                    .map(|e| (e, theme::DANGER))
                    .chain(warnings.iter().map(|w| (w, p.warn_text)))
                {
                    ui.label(
                        egui::RichText::new(format!("•  {text}"))
                            .font(theme::sans(12.))
                            .color(colour),
                    );
                }
            });
    }
}

/// The U1's start options: bed leveling, flow calibration (for chosen tools) and timelapse.
fn start_options(ui: &mut egui::Ui, plan: &mut crate::printhost::u1map::Plan) {
    let p = theme::of(ui);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 16.;
        ui.checkbox(&mut plan.bed_leveling, "Bed leveling");
        ui.checkbox(&mut plan.flow_calibration, "Flow calibration");
        ui.checkbox(&mut plan.timelapse, "Timelapse");
    });
    if plan.flow_calibration && plan.rows.len() > 1 {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 10.;
            ui.add_space(24.);
            ui.label(
                egui::RichText::new("Calibrate")
                    .font(theme::sans(12.))
                    .color(p.muted),
            );
            for row in &mut plan.rows {
                let label = match row.slot {
                    Some(s) => format!("T{} (slot {})", row.job.tool + 1, s + 1),
                    None => format!("T{}", row.job.tool + 1),
                };
                ui.checkbox(&mut row.calibrate, label);
            }
        });
        if plan.rows.iter().all(|r| !r.calibrate) {
            ui.label(
                egui::RichText::new("No tool is chosen, so no flow calibration runs.")
                    .font(theme::sans(11.))
                    .color(p.warn_dim),
            );
        }
    }
}

/// A U1 slot in the slot dropdown: `Slot 1 · PLA Basic · yellow`, or `Slot 4 · empty`.
/// Colors by name; the hex code is only in the tooltip.
pub(crate) fn slot_text(s: &mica_printhost::U1Slot) -> String {
    let empty = s.exists == Some(false)
        || s.material
            .as_deref()
            .is_none_or(|m| m.trim().is_empty() || m == "NONE");
    if empty {
        return format!("Slot {} · empty", s.index + 1);
    }
    let m = s.material.clone().unwrap_or_default();
    let mut text = match s
        .subtype
        .as_deref()
        .filter(|t| !t.is_empty() && *t != "NONE")
    {
        Some(sub) => format!("Slot {} · {m} {sub}", s.index + 1),
        None => format!("Slot {} · {m}", s.index + 1),
    };
    if let Some(c) = s.colour.as_deref().and_then(mica_core::color::parse_hex) {
        text.push_str(" · ");
        text.push_str(mica_printhost::colour::colour_name(c));
    }
    text
}

/// A 14 px swatch with its left edge at `at`.
fn swatch_at(ui: &egui::Ui, at: Pos2, colour: [u8; 3]) {
    let r = Rect::from_min_size(Pos2::new(at.x, at.y - 7.), Vec2::splat(14.));
    ui.painter().rect_filled(
        r,
        3.,
        egui::Color32::from_rgb(colour[0], colour[1], colour[2]),
    );
    ui.painter().rect_stroke(
        r,
        3.,
        Stroke::new(1., ui.visuals().widgets.noninteractive.bg_stroke.color),
        egui::StrokeKind::Inside,
    );
}
