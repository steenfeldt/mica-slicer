// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Secondary windows: the printer and filament editors (pages for the less frequent settings,
//! like Orca's), the "All printers" picker and the "Save as" dialog. The Print dialog lives
//! in `print.rs`, the My printers window in `printers.rs`.

use super::{editor, profiles::PresetKind};
use crate::app::{App, SaveAs};
use crate::overlays::Target;
use crate::settings::{self, Tab};
use eframe::egui;
use mica_profiles::prusa::Kind;

impl App {
    pub(crate) fn dialogs(&mut self, ctx: &egui::Context) {
        if self.dialogs.printer_editor {
            self.printer_editor(ctx);
        }
        if let Some(tool) = self.dialogs.filament_editor {
            self.filament_editor(ctx, tool);
        }
        if self.dialogs.all_printers {
            self.all_printers(ctx);
        }
        if self.dialogs.save_as.is_some() {
            self.save_as_dialog(ctx);
        }
        if self.dialogs.print {
            self.print_dialog(ctx);
        }
        if self.export_all.is_some() {
            self.export_all_window(ctx);
        }
        if self.dialogs.my_printers {
            self.my_printers_window(ctx);
        }
        self.add_printer_window(ctx);
        if self.dialogs.save_project.is_some() {
            self.save_project_dialog(ctx);
        }
        if self.dialogs.unsaved.is_some() {
            self.unsaved_prompt(ctx);
        }
        if self.dialogs.calibration.is_some() {
            self.calibration_dialog(ctx);
        }
        if self.dialogs.preferences {
            self.preferences_window(ctx);
        }
        if self.prefs.features.textures {
            self.texture_gallery(ctx);
        }
        if self.keys.dialog {
            self.shortcuts_window(ctx);
        }
        if self.dialogs.about.is_some() {
            self.about_window(ctx);
        }
        if self.import_ui.review.is_some() {
            self.import_review_window(ctx);
        }
        self.update_notes_window(ctx);
        self.whats_new_window(ctx);
        // Last, so it sits above What's new when both open at launch.
        self.welcome_window(ctx);
    }

    fn printer_editor(&mut self, ctx: &egui::Context) {
        let mut open = self.dialogs.printer_editor;
        let title = format!("Printer · {}", self.profiles.printer);
        let tools = self.tool_count();
        let pages = settings::pages(Tab::Printer);
        let mut edits = Vec::new();
        let mut save = None;
        let note = self.rules_note();
        // Taller on a tall window, so a page's groups (Extruder › Travel lift) fit.
        let height = (ctx.content_rect().height() * 0.7).clamp(540., 1000.);
        egui::Window::new(title)
            .id(egui::Id::unique("printer-editor"))
            .open(&mut open)
            .default_size([760., height])
            .min_size([560., 360.])
            .default_pos(ctx.content_rect().center() - egui::vec2(380., height / 2.))
            .resizable(true)
            .collapsible(false)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.weak(format!(
                        "{} · changes apply to the next slice",
                        self.profiles.vendor_name()
                    ));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let name = self.profiles.printer.clone();
                        let modified = !self.overlays.printer.is_empty();
                        if ui.button("Save as…").clicked() {
                            save = Some(SaveAs {
                                kind: PresetKind::Printer,
                                tool: 0,
                                name: format!("{name} - Copy"),
                                error: None,
                            });
                        }
                        let is_user = self.profiles.is_user_preset(PresetKind::Printer, &name);
                        if ui
                            .add_enabled(modified && is_user, egui::Button::new("Save"))
                            .clicked()
                            && let Err(e) = self.save_profile(PresetKind::Printer, 0, &name)
                        {
                            self.notify_error("The printer profile was not saved", e);
                        }
                        if ui
                            .add_enabled(modified, egui::Button::new("Discard changes"))
                            .clicked()
                        {
                            self.overlays.printer.clear();
                            self.settings_changed();
                        }
                        let mut advanced = self.prefs.advanced;
                        if ui.toggle_value(&mut advanced, "Advanced").changed() {
                            self.prefs.advanced = advanced;
                            self.prefs_dirty = true;
                        }
                        ui.add(
                            egui::TextEdit::singleline(&mut self.editor.printer_search)
                                .hint_text("Search…")
                                .desired_width(160.),
                        );
                    });
                });
                ui.separator();
                if let Some(note) = &note {
                    edits.extend(rules_note_ui(ui, note));
                }
                ui.horizontal_top(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width(170.);
                        let mut n = 0;
                        for (pi, p) in pages.iter().enumerate() {
                            if p.name == "Extruder" {
                                for t in 0..tools {
                                    let selected = self.editor.printer_page == pi
                                        && self.editor.printer_extruder == t;
                                    let label = if tools > 1 {
                                        format!("Extruder {}", t + 1)
                                    } else {
                                        "Extruder".to_string()
                                    };
                                    if ui.selectable_label(selected, label).clicked() {
                                        self.editor.printer_page = pi;
                                        self.editor.printer_extruder = t;
                                    }
                                    n += 1;
                                }
                            } else if ui
                                .selectable_label(self.editor.printer_page == pi, &p.name)
                                .clicked()
                            {
                                self.editor.printer_page = pi;
                                n += 1;
                            }
                        }
                        let _ = n;
                    });
                    ui.separator();
                    ui.vertical(|ui| {
                        ui.set_min_width(ui.available_width());
                        let Some(config) = self.effective_config() else {
                            if let Err(e) = &self.config {
                                ui.colored_label(ui.visuals().error_fg_color, e);
                            }
                            return;
                        };
                        let preset_overrides = self
                            .profiles
                            .store
                            .load(Kind::Printer, &self.profiles.printer)
                            .ok()
                            .map(|p| p.settings);
                        let fctx = editor::FieldContext {
                            config,
                            overlay: &self.overlays.printer,
                            preset_overrides: preset_overrides.as_ref(),
                            tool: self.editor.printer_extruder,
                            advanced: self.prefs.advanced,
                            search: &self.editor.printer_search,
                            id: "printer",
                            force: false,
                            inherit: None,
                        };
                        egui::ScrollArea::vertical()
                            .id_salt("printer-pages")
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                let searching = !self.editor.printer_search.trim().is_empty();
                                let mut shown = 0;
                                if searching {
                                    for p in pages {
                                        let (e, n) = editor::page(ui, p, &fctx);
                                        shown += n;
                                        edits.extend(e);
                                    }
                                } else if let Some(p) = pages.get(self.editor.printer_page) {
                                    if p.name == "Basic information" {
                                        // The slicing-rules switch opens this page; say what it
                                        // means in one line.
                                        ui.weak(settings::RULES_DESCRIPTION);
                                        ui.add_space(2.);
                                    }
                                    let (e, n) = editor::page(ui, p, &fctx);
                                    shown = n;
                                    edits.extend(e);
                                    if shown == 0 && !fctx.advanced {
                                        // A page with only advanced settings would be empty in
                                        // simple mode: show them rather than a blank page.
                                        ui.weak("Advanced settings (this page has no basic ones)");
                                        ui.add_space(4.);
                                        let all = editor::FieldContext {
                                            advanced: true,
                                            ..fctx
                                        };
                                        let (e, n) = editor::page(ui, p, &all);
                                        shown = n;
                                        edits.extend(e);
                                    }
                                }
                                if shown == 0 {
                                    ui.weak("No setting matches");
                                }
                            });
                    });
                });
            });
        self.dialogs.printer_editor = open;
        for e in edits {
            self.apply_edit(Some(Target::Printer), e);
        }
        if save.is_some() {
            self.dialogs.save_as = save;
        }
    }

    /// The note for a profile sliced with the other slicer's rules, if it is.
    fn rules_note(&self) -> Option<RulesNote> {
        use mica_core::rules::Rules;
        let config = self.effective_config()?;
        let rules = Rules::of(config);
        let tuned = self.profiles.profile_rules();
        if rules == tuned {
            return None;
        }
        let counterpart = (rules.is_orca() && tuned.is_prusa())
            .then(|| mica_profiles::rules_fill::has_orca_counterpart(config, None))
            .flatten();
        Some(RulesNote {
            rules,
            tuned,
            counterpart: counterpart.map(|s| match s {
                mica_profiles::rules_fill::Source::OrcaProfile { printer, .. } => printer,
                mica_profiles::rules_fill::Source::OrcaDefaults => String::new(),
            }),
            use_profile: config.string(mica_profiles::rules_fill::SOURCE_KEY)
                != Some("orca_defaults"),
        })
    }

    fn filament_editor(&mut self, ctx: &egui::Context, tool: usize) {
        let tool = tool.min(self.profiles.filaments.len() - 1);
        let mut open = true;
        let name = self.profiles.filaments[tool].clone();
        let shown = crate::preset_names::display_name(&name);
        let title = if self.tool_count() > 1 {
            format!("Filament · Tool {} · {shown}", tool + 1)
        } else {
            format!("Filament · {shown}")
        };
        let pages = settings::pages(Tab::Filament);
        let mut edits = Vec::new();
        let mut save = None;
        egui::Window::new(title)
            .id(egui::Id::unique("filament-editor"))
            .open(&mut open)
            .default_size([720., 520.])
            .min_size([540., 360.])
            .default_pos(ctx.content_rect().center() - egui::vec2(360., 260.))
            .resizable(true)
            .collapsible(false)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.weak("Changes apply to the next slice");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let modified = !self.overlays.filaments[tool].is_empty();
                        if ui.button("Save as…").clicked() {
                            save = Some(SaveAs {
                                kind: PresetKind::Filament,
                                tool,
                                name: format!("{name} - Copy"),
                                error: None,
                            });
                        }
                        let is_user = self.profiles.is_user_preset(PresetKind::Filament, &name);
                        if ui
                            .add_enabled(modified && is_user, egui::Button::new("Save"))
                            .clicked()
                            && let Err(e) = self.save_profile(PresetKind::Filament, tool, &name)
                        {
                            self.notify_error("The filament profile was not saved", e);
                        }
                        if ui
                            .add_enabled(modified, egui::Button::new("Discard changes"))
                            .clicked()
                        {
                            self.overlays.filaments[tool].clear();
                            self.settings_changed();
                        }
                        let mut advanced = self.prefs.advanced;
                        if ui.toggle_value(&mut advanced, "Advanced").changed() {
                            self.prefs.advanced = advanced;
                            self.prefs_dirty = true;
                        }
                        ui.add(
                            egui::TextEdit::singleline(&mut self.editor.filament_search)
                                .hint_text("Search…")
                                .desired_width(160.),
                        );
                    });
                });
                ui.separator();
                ui.horizontal_top(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width(150.);
                        for (pi, p) in pages.iter().enumerate() {
                            if ui
                                .selectable_label(self.editor.filament_page == pi, &p.name)
                                .clicked()
                            {
                                self.editor.filament_page = pi;
                            }
                        }
                    });
                    ui.separator();
                    ui.vertical(|ui| {
                        ui.set_min_width(ui.available_width());
                        let Some(config) = self.effective_config() else {
                            return;
                        };
                        let preset_overrides = self
                            .profiles
                            .store
                            .load(Kind::Filament, &name)
                            .ok()
                            .map(|p| p.settings);
                        let fctx = editor::FieldContext {
                            config,
                            overlay: &self.overlays.filaments[tool],
                            preset_overrides: preset_overrides.as_ref(),
                            tool,
                            advanced: self.prefs.advanced,
                            search: &self.editor.filament_search,
                            id: "filament",
                            force: false,
                            inherit: None,
                        };
                        egui::ScrollArea::vertical()
                            .id_salt("filament-pages")
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                let searching = !self.editor.filament_search.trim().is_empty();
                                let mut shown = 0;
                                if searching {
                                    for p in pages {
                                        let (e, n) = editor::page(ui, p, &fctx);
                                        shown += n;
                                        edits.extend(e);
                                    }
                                } else if let Some(p) = pages.get(self.editor.filament_page) {
                                    let (e, n) = editor::page(ui, p, &fctx);
                                    shown = n;
                                    edits = e;
                                    if shown == 0 && !fctx.advanced {
                                        // A page with only advanced settings would be empty in
                                        // simple mode: show them rather than a blank page.
                                        ui.weak("Advanced settings (this page has no basic ones)");
                                        ui.add_space(4.);
                                        let all = editor::FieldContext {
                                            advanced: true,
                                            ..fctx
                                        };
                                        let (e, n) = editor::page(ui, p, &all);
                                        shown = n;
                                        edits = e;
                                    }
                                }
                                if shown == 0 {
                                    ui.weak("No setting matches");
                                }
                            });
                    });
                });
            });
        if !open {
            self.dialogs.filament_editor = None;
        }
        for e in edits {
            self.apply_edit(Some(Target::Filament(tool)), e);
        }
        if save.is_some() {
            self.dialogs.save_as = save;
        }
    }

    fn all_printers(&mut self, ctx: &egui::Context) {
        let mut open = self.dialogs.all_printers;
        let all = self.profiles.all_printers();
        let mut toggle: Option<(usize, String, bool)> = None;
        let mut select: Option<(usize, String)> = None;
        egui::Window::new("All printers")
            .id(egui::Id::unique("all-printers"))
            .open(&mut open)
            .default_size([520., 480.])
            .default_pos(ctx.content_rect().center() - egui::vec2(260., 240.))
            .collapsible(false)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.dialogs.all_printers_filter)
                            .hint_text("Filter printers…")
                            .desired_width(ui.available_width() - 8.),
                    );
                });
                ui.weak(
                    "Tick a printer to keep it in “My printers”; double-click to select it now.",
                );
                ui.separator();
                let filter = self.dialogs.all_printers_filter.to_lowercase();
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let mut last_vendor = String::new();
                        for (vi, vendor, name) in &all {
                            if !filter.is_empty() && !name.to_lowercase().contains(&filter) {
                                continue;
                            }
                            if *vendor != last_vendor {
                                ui.add_space(4.);
                                ui.label(egui::RichText::new(vendor).strong());
                                last_vendor = vendor.clone();
                            }
                            let mut mine = self.prefs.has_printer(vendor, name);
                            ui.horizontal(|ui| {
                                if ui.checkbox(&mut mine, "").changed() {
                                    toggle = Some((*vi, name.clone(), mine));
                                }
                                let current =
                                    *vi == self.profiles.vendor && *name == self.profiles.printer;
                                let r = ui.selectable_label(current, name);
                                if r.double_clicked() {
                                    select = Some((*vi, name.clone()));
                                }
                                super::import::preset_badge(
                                    ui,
                                    &self.profiles,
                                    PresetKind::Printer,
                                    name,
                                );
                            });
                        }
                    });
            });
        self.dialogs.all_printers = open;
        if let Some((vi, name, mine)) = toggle {
            let vendor = self.profiles.vendors[vi].0.clone();
            if mine {
                self.prefs.add_printer(&vendor, &name);
            } else {
                self.prefs.remove_printer(&vendor, &name);
            }
            self.prefs_dirty = true;
        }
        if let Some((vi, name)) = select {
            let vendor = self.profiles.vendors[vi].0.clone();
            self.prefs.add_printer(&vendor, &name);
            self.switch_printer(vi, &name);
            self.dialogs.all_printers = false;
        }
    }

    fn save_as_dialog(&mut self, ctx: &egui::Context) {
        let Some(dialog) = &mut self.dialogs.save_as else {
            return;
        };
        let kind = dialog.kind;
        let tool = dialog.tool;
        let what = match kind {
            PresetKind::Print => "process",
            PresetKind::Printer => "printer",
            PresetKind::Filament => "filament",
        };
        let mut action: Option<bool> = None;
        let mut open = true;
        egui::Window::new(format!("Save {what} profile as"))
            .id(egui::Id::unique("save-as"))
            .open(&mut open)
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0., 120.))
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.set_width(380.);
                ui.add_space(6.);
                ui.weak("Only your changes are stored, on top of the selected system preset.");
                let r = ui
                    .add(egui::TextEdit::singleline(&mut dialog.name).desired_width(f32::INFINITY));
                if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    action = Some(true);
                }
                if let Some(e) = &dialog.error {
                    ui.colored_label(ui.visuals().error_fg_color, e);
                }
                ui.add_space(6.);
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Save").clicked() {
                            action = Some(true);
                        }
                        if ui.button("Cancel").clicked() {
                            action = Some(false);
                        }
                    });
                });
            });
        if !open {
            action = Some(false);
        }
        match action {
            Some(true) => {
                let name = dialog.name.trim().to_string();
                if name.is_empty() {
                    dialog.error = Some("Give the profile a name".into());
                    return;
                }
                match self.save_profile(kind, tool, &name) {
                    Ok(()) => self.dialogs.save_as = None,
                    Err(e) => {
                        if let Some(d) = &mut self.dialogs.save_as {
                            d.error = Some(e);
                        }
                    }
                }
            }
            Some(false) => self.dialogs.save_as = None,
            None => {}
        }
    }
}

/// A printer profile sliced with the other slicer's rules (`mica_slicing_rules`).
pub(crate) struct RulesNote {
    rules: mica_core::rules::Rules,
    /// The slicer the profile was made for.
    tuned: mica_core::rules::Rules,
    /// OrcaSlicer's own printer profile for this printer, if Orca ships one.
    counterpart: Option<String>,
    /// The Orca-only values come from that profile (`mica_orca_values`).
    use_profile: bool,
}

/// The plain note above the printer pages, with the choice of where the other slicer's
/// settings come from. Returns the edit of that choice.
fn rules_note_ui(ui: &mut egui::Ui, note: &RulesNote) -> Vec<editor::Edit> {
    let p = crate::theme::of(ui);
    let mut edits = Vec::new();
    egui::Frame::new()
        .fill(p.selected_row)
        .stroke(egui::Stroke::new(1., p.scope_border))
        .corner_radius(8.)
        .inner_margin(egui::Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(format!(
                "This profile is tuned for {}. G-code will follow {}'s rules.",
                note.tuned.label(),
                note.rules.label()
            ));
            if note.rules.is_prusa() {
                ui.weak(
                    "PrusaSlicer-only settings (ramping travel lift, cooling slowdown logic, …) \
                     use PrusaSlicer's defaults unless you change them.",
                );
            } else if let Some(printer) = &note.counterpart {
                let mut use_profile = note.use_profile;
                if ui
                    .checkbox(
                        &mut use_profile,
                        format!(
                            "Take the Orca-only settings (Z-hop type, bridge line width, …) from \
                             OrcaSlicer's own \"{printer}\" profile"
                        ),
                    )
                    .on_hover_text(
                        "Off: OrcaSlicer's defaults. Settings this profile has stay as they are.",
                    )
                    .changed()
                {
                    edits.push(editor::Edit::Set {
                        key: mica_profiles::rules_fill::SOURCE_KEY.into(),
                        raw: if use_profile {
                            "orca_profile"
                        } else {
                            "orca_defaults"
                        }
                        .into(),
                    });
                }
            } else {
                ui.weak(
                    "OrcaSlicer has no profile for this printer: the Orca-only settings \
                     (Z-hop type, bridge line width, …) use OrcaSlicer's defaults.",
                );
            }
        });
    ui.add_space(6.);
    edits
}
