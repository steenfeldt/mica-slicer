// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The left sidebar (360 px): Printer, Filament and Process cards. The Process card holds the
//! scope line ("Settings for All objects ▾" or the object-scope card) and, for all objects,
//! the topic rail with one topic at a time (`topics.rs`): a curated Essentials (ten rows plus
//! "Show N more settings"), then Quality, Strength, Speed, Support, Cooling & temperatures,
//! Multimaterial (multi-tool printers only) and Advanced. A fixed footer holds the "Show all
//! settings" switch.

use super::{
    editor::{self, FieldContext},
    import::{MenuPick, preset_row},
    profiles::PresetKind,
    swatch,
    widgets::{self, Glyph, MARGIN, ROW_H},
};
use crate::app::{App, SaveAs};
use crate::overlays::Target;
use crate::preset_names;
use crate::settings::{self, Tab};
use crate::theme;
use eframe::egui::{self, Align2, Pos2, Rect, Sense, Stroke, Vec2};
use mica_core::config::Config;

/// Height of a section header.
const SECTION_H: f32 = 34.;

/// One row of the settings list: label, key and which overlay it edits.
pub(super) struct RowDef {
    pub(super) label: String,
    pub(super) key: &'static str,
    pub(super) target: Target,
}
/// A section inside a topic (Orca's option group: Layer height, Seam, …).
pub(super) struct GroupDef {
    pub(super) name: String,
    /// Stable name for the remembered fold state (the cooling groups' filament name left out).
    pub(super) id: String,
    pub(super) rows: Vec<RowDef>,
}
/// A topic of the rail (Orca's settings page, plus Essentials and Cooling & temperatures).
pub(super) struct TopicDef {
    pub(super) name: &'static str,
    /// Rows always shown.
    pub(super) groups: Vec<GroupDef>,
    /// Rows behind "Show N more settings" (Essentials); other topics use Simple/Advanced.
    pub(super) more: Vec<GroupDef>,
}

/// A card: 14 / 16 / 12 px padding and a 1 px bottom border, no fill.
pub(super) fn card<R>(ui: &mut egui::Ui, body: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let p = theme::of(ui);
    let r = egui::Frame::new()
        .inner_margin(egui::Margin {
            left: 16,
            right: 16,
            top: 14,
            bottom: 12,
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            body(ui)
        });
    let rect = r.response.rect;
    ui.painter().line_segment(
        [
            Pos2::new(rect.left(), rect.bottom()),
            Pos2::new(rect.right(), rect.bottom()),
        ],
        Stroke::new(1., p.border),
    );
    r.inner
}

pub use mica_profiles::project::split_printer_name;

/// Height of the sidebar's fixed footer (rule + "Show all settings" row).
const FOOTER_H: f32 = ROW_H + 11.;

impl App {
    pub(crate) fn sidebar(&mut self, ui: &mut egui::Ui) {
        let busy = self.loading.is_some();
        ui.add_enabled_ui(!busy, |ui| {
            // The object tree on top (its own scrolling, resizable list), then the scrolling
            // cards above a fixed footer with the "Show all settings" switch.
            ui.spacing_mut().item_spacing.y = 0.;
            self.object_tree(ui);
            let spacing = ui.spacing().item_spacing.y;
            let scroll_h = (ui.available_height() - FOOTER_H - 2. * spacing - 6.).max(0.);
            egui::ScrollArea::vertical()
                .id_salt("sidebar-scroll")
                .auto_shrink([false, false])
                .max_height(scroll_h)
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(8., 0.);
                    self.project_card(ui);
                    if let Some(none) = self.no_printer() {
                        // No printer, no filament or print profiles: the user chooses one.
                        let card = ui.scope(|ui| self.no_printer_card(ui, &none));
                        crate::tour::anchor(
                            ui,
                            crate::tour::Anchor::PrinterFilament,
                            card.response.rect,
                        );
                    } else {
                        let cards = ui.scope(|ui| {
                            self.printer_card(ui);
                            self.filament_card(ui);
                        });
                        crate::tour::anchor(
                            ui,
                            crate::tour::Anchor::PrinterFilament,
                            cards.response.rect,
                        );
                        self.process_card(ui);
                    }
                    if !self.notices.is_empty() {
                        ui.add_space(8.);
                        ui.horizontal(|ui| {
                            ui.add_space(16.);
                            ui.collapsing(format!("Notices ({})", self.notices.len()), |ui| {
                                for n in &self.notices {
                                    ui.small(n);
                                }
                            });
                        });
                    }
                    ui.add_space(12.);
                });
            self.show_all_footer(ui);
        });
    }

    /// The sidebar's fixed footer: a rule and the "Show all settings" switch, always visible.
    fn show_all_footer(&mut self, ui: &mut egui::Ui) {
        let p = theme::of(ui);
        let (rule, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.), Sense::hover());
        ui.painter().line_segment(
            [rule.left_top(), rule.right_top()],
            Stroke::new(1., p.border),
        );
        let (row, _) = ui.allocate_exact_size(
            Vec2::new(ui.available_width(), FOOTER_H - 1.),
            Sense::hover(),
        );
        ui.painter().rect_filled(row, 0., p.panel);
        ui.painter().text(
            Pos2::new(row.left() + 16., row.center().y),
            Align2::LEFT_CENTER,
            "Show all settings",
            theme::sans(13.),
            p.label,
        );
        let id = ui.make_persistent_id("show-all");
        let switch = Rect::from_min_size(
            Pos2::new(row.right() - MARGIN - 34., row.center().y - 10.),
            Vec2::new(34., 20.),
        );
        if let Some(v) = widgets::toggle_at(ui, id, switch, self.prefs.advanced, true) {
            self.prefs.advanced = v;
            self.prefs_dirty = true;
        }
    }

    /// The opened project's own settings: offered (`Use project settings` / `Keep mine`) or,
    /// when the project was made for the selected printer, already in use. In use, the
    /// project's saved values are its own (not edits); the card says how many differ from the
    /// current presets (with the list) and offers "Update to current profile" and a revert.
    /// Non-modal; the project's filament colours apply either way.
    fn project_card(&mut self, ui: &mut egui::Ui) {
        use crate::project::State;
        let Some(prompt) = &self.project else {
            return;
        };
        if prompt.state == State::Dismissed {
            return;
        }
        let applied = matches!(prompt.state, State::Applied { .. });
        let updated = prompt.updated;
        let summary = prompt.summary();
        let differences = if applied {
            self.project_differences.clone()
        } else {
            Vec::new()
        };
        let process = preset_names::display_name(&self.profiles.process);
        let p = theme::of(ui);
        let mut action: Option<&str> = None;
        card(ui, |ui| {
            egui::Frame::new()
                .fill(p.accent_surface)
                .stroke(Stroke::new(1., p.accent))
                .corner_radius(6.)
                .inner_margin(egui::Margin::symmetric(12, 10))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.spacing_mut().item_spacing.y = 6.;
                    ui.label(
                        egui::RichText::new(if applied {
                            "Using this project's settings"
                        } else {
                            "This project has its own settings"
                        })
                        .font(theme::semibold(13.))
                        .color(p.text),
                    );
                    ui.label(
                        egui::RichText::new(&summary)
                            .font(theme::sans(12.))
                            .color(p.label),
                    );
                    ui.label(
                        egui::RichText::new(match (applied, updated) {
                            (true, false) => {
                                "Sliced with the values the project was saved with (they are \
                                 the project's own, not your edits); the filament colors come \
                                 from the project."
                            }
                            (true, true) => {
                                "Updated to the current profiles, keeping the values the \
                                 project marks as changed; the filament colors come from the \
                                 project."
                            }
                            _ => "The filament colors already come from the project.",
                        })
                        .font(theme::sans(11.))
                        .color(p.muted),
                    );
                    if applied && !differences.is_empty() {
                        let n = differences.len();
                        egui::CollapsingHeader::new(
                            egui::RichText::new(format!(
                                "{n} value{} differ{} from the current {process} profile",
                                if n == 1 { "" } else { "s" },
                                if n == 1 { "s" } else { "" },
                            ))
                            .font(theme::sans(12.))
                            .color(p.label),
                        )
                        .id_salt("project-differences")
                        .show(ui, |ui| {
                            egui::ScrollArea::vertical()
                                .max_height(220.)
                                .show(ui, |ui| {
                                    for d in &differences {
                                        difference_row(ui, d, p);
                                    }
                                });
                        });
                    }
                    ui.add_space(2.);
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.;
                        if applied {
                            if !updated
                                && !differences.is_empty()
                                && widgets::button(ui, "Update to current profile", true)
                                    .on_hover_text(
                                        "Use today's presets instead of the values the project \
                                         was saved with, keeping the values the project marks \
                                         as deliberately changed",
                                    )
                                    .clicked()
                            {
                                action = Some("update");
                            }
                            if widgets::button(ui, "Revert to my settings", true).clicked() {
                                action = Some("revert");
                            }
                            if widgets::button(ui, "OK", true).clicked() {
                                action = Some("keep");
                            }
                        } else {
                            if widgets::primary_button(ui, "Use project settings", true).clicked() {
                                action = Some("use");
                            }
                            if widgets::button(ui, "Keep mine", true).clicked() {
                                action = Some("keep");
                            }
                        }
                    });
                });
        });
        match action {
            Some("use") => self.use_project_settings(),
            Some("update") => self.update_project_to_current(),
            Some("keep") => self.keep_my_settings(),
            Some("revert") => self.revert_project_settings(),
            _ => {}
        }
    }

    /// The Printer card without a printer: "No printer selected" with Add your printer (the
    /// gallery) and the user's printers, or the remembered Prusa printer waiting for the Prusa
    /// printer profiles with the download's state and Try again. Never another printer.
    fn no_printer_card(&mut self, ui: &mut egui::Ui, none: &crate::app::NoPrinter) {
        let p = theme::of(ui);
        let mine = self.profiles.my_printers_in_picker_order(&self.prefs);
        let mut choice: Option<(usize, String)> = None;
        let mut open_gallery = false;
        card(ui, |ui| {
            widgets::card_header(ui, "Printer", |_| {});
            ui.spacing_mut().item_spacing.y = 6.;
            match none {
                crate::app::NoPrinter::Unchosen => {
                    ui.label(
                        egui::RichText::new("No printer selected")
                            .font(theme::semibold(14.))
                            .color(p.text),
                    );
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(
                                "Mica slices for the printer you choose. Add yours to see its \
                                 bed and its print and filament profiles.",
                            )
                            .font(theme::sans(12.))
                            .color(p.muted),
                        )
                        .wrap(),
                    );
                    ui.add_space(4.);
                    if widgets::primary_button(ui, "Add your printer", true)
                        .on_hover_text(
                            "Pick it from the printer gallery. Tested printers come first.",
                        )
                        .clicked()
                    {
                        open_gallery = true;
                    }
                    if !mine.is_empty() {
                        ui.add_space(4.);
                        ui.label(
                            egui::RichText::new("Or choose one of your printers:")
                                .font(theme::sans(12.))
                                .color(p.label),
                        );
                        for (vi, name) in &mine {
                            if ui.selectable_label(false, name).clicked() {
                                choice = Some((*vi, name.clone()));
                            }
                        }
                    }
                    if self.profiles.prusa_missing {
                        ui.add_space(4.);
                        self.prusa_download_ui(ui);
                    }
                }
                crate::app::NoPrinter::Waiting(name) => {
                    let (model, variant) = split_printer_name(name);
                    ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(Vec2::splat(18.), Sense::hover());
                        widgets::glyph(ui.painter(), r, Glyph::Printer, p.muted_alt);
                        ui.label(
                            egui::RichText::new(model)
                                .font(theme::semibold(14.))
                                .color(p.text),
                        );
                        if !variant.is_empty() {
                            ui.label(
                                egui::RichText::new(variant)
                                    .font(theme::sans(12.))
                                    .color(p.muted),
                            );
                        }
                    });
                    ui.label(
                        egui::RichText::new(match self.prusa_download.phase {
                            crate::prusa_download::Phase::Downloading(_) => {
                                "Profiles downloading… This printer is ready once they are here."
                            }
                            _ => "This printer needs the Prusa printer profiles.",
                        })
                        .font(theme::sans(12.))
                        .color(p.label),
                    );
                    self.prusa_download_ui(ui);
                    ui.add_space(4.);
                    if widgets::button(ui, "Choose another printer", true).clicked() {
                        open_gallery = true;
                    }
                }
            }
        });
        if let Some((vi, name)) = choice {
            self.switch_printer(vi, &name);
        }
        if open_gallery {
            self.open_printer_gallery();
        }
    }

    fn printer_card(&mut self, ui: &mut egui::Ui) {
        let p = theme::of(ui);
        let mine = self.profiles.my_printers_in_picker_order(&self.prefs);
        let current = self.profiles.printer.clone();
        let vendor = self.profiles.vendor;
        let mut choice: Option<(usize, String)> = None;
        let mut open_all = false;
        let mut open_add = false;
        let mut edit = false;
        let mut bed_choice = None;
        let printer_changed = !self.overlays.printer.is_empty();
        let rules = self
            .effective_config()
            .map(mica_core::rules::Rules::of)
            .unwrap_or_else(|| self.profiles.profile_rules());
        let mut rules_clicked = false;
        card(ui, |ui| {
            widgets::card_header(ui, "Printer", |ui| {
                let badge = widgets::badge(
                    ui,
                    crate::settings::rules_badge(rules),
                    rules != self.profiles.profile_rules(),
                )
                .on_hover_text(format!(
                    "G-code follows {}'s behavior for walls, seams, travel, cooling and tool \
                     changes. Change it in Printer settings → Basic information.",
                    rules.label()
                ));
                rules_clicked = badge.clicked();
                if widgets::icon_button(
                    ui,
                    Glyph::Pencil,
                    if printer_changed {
                        "Printer settings (changed)"
                    } else {
                        "Printer settings: bed, G-code, speeds, extruders"
                    },
                    true,
                )
                .clicked()
                {
                    edit = true;
                }
            });
            let (model, variant) = split_printer_name(&current);
            let id = ui.make_persistent_id("printer-combo");
            let response = widgets::combo_button(
                ui,
                id,
                |ui| {
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(18.), Sense::hover());
                    widgets::glyph(ui.painter(), r, Glyph::Printer, p.muted_alt);
                },
                &model,
                &variant,
                None,
            );
            egui::Popup::menu(&response)
                .id(id.with("menu"))
                .width(response.rect.width())
                .show(|ui| {
                    if mine.is_empty() {
                        ui.weak("No printers added yet");
                    }
                    for (vi, name) in &mine {
                        let label = if self.profiles.vendors.len() > 1 {
                            format!("{name}  ·  {}", self.profiles.vendors[*vi].0)
                        } else {
                            name.clone()
                        };
                        let clicked = ui
                            .horizontal(|ui| {
                                let clicked = ui
                                    .selectable_label(*vi == vendor && *name == current, label)
                                    .clicked();
                                super::import::preset_badge(
                                    ui,
                                    &self.profiles,
                                    PresetKind::Printer,
                                    name,
                                );
                                clicked
                            })
                            .inner;
                        if clicked {
                            choice = Some((*vi, name.clone()));
                            ui.close();
                        }
                    }
                    if self.profiles.prusa_missing {
                        ui.separator();
                        self.prusa_download_ui(ui);
                    }
                    ui.separator();
                    if ui.button("Add printer…").clicked() {
                        open_add = true;
                        ui.close();
                    }
                    if ui.button("All printers…").clicked() {
                        open_all = true;
                        ui.close();
                    }
                });
            ui.add_space(8.);
            // Second line: size on the left, the plate type (Snapmaker) or tools on the right.
            let (line, _) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), 16.), Sense::hover());
            let size_text = if let Some(c) = self.effective_config() {
                let bed = self.bed.max - self.bed.min;
                format!(
                    "{:.0} × {:.0} × {:.0} mm",
                    bed.x,
                    bed.y,
                    c.float("max_print_height").unwrap_or(0.)
                )
            } else {
                String::new()
            };
            ui.painter().text(
                Pos2::new(line.left(), line.center().y),
                Align2::LEFT_CENTER,
                size_text,
                theme::sans(12.),
                p.muted,
            );
            if self.profiles.is_orca() {
                let g = ui.painter().layout_no_wrap(
                    self.profiles.bed.clone(),
                    theme::sans(12.),
                    p.accent_bright,
                );
                let r = Rect::from_min_size(
                    Pos2::new(line.right() - g.size().x - 12., line.top()),
                    Vec2::new(g.size().x + 12., line.height()),
                );
                let resp = ui.interact(r, ui.make_persistent_id("bed-type"), Sense::click());
                let color = if resp.hovered() {
                    p.accent_hi
                } else {
                    p.accent_bright
                };
                ui.painter().galley(
                    Pos2::new(r.left(), line.center().y - g.size().y / 2.),
                    g,
                    color,
                );
                widgets::chevron(
                    ui.painter(),
                    Pos2::new(r.right() - 4., line.center().y + 1.),
                    true,
                    color,
                );
                egui::Popup::menu(&resp).show(|ui| {
                    for b in super::profiles::SNAPMAKER_BEDS {
                        if ui
                            .selectable_label(*b == self.profiles.bed, b.to_string())
                            .clicked()
                        {
                            bed_choice = Some(b.to_string());
                            ui.close();
                        }
                    }
                });
            } else if let Some(c) = self.effective_config() {
                let n = c.vector_len("nozzle_diameter").max(1);
                let text = if n > 1 {
                    format!("{n} tools")
                } else if printer_changed {
                    format!("{} changed", self.overlays.printer.len())
                } else {
                    String::new()
                };
                ui.painter().text(
                    Pos2::new(line.right(), line.center().y),
                    Align2::RIGHT_CENTER,
                    text,
                    theme::sans(12.),
                    p.muted,
                );
            }
        });
        if let Some((vi, name)) = choice
            && (vi != vendor || name != current)
        {
            self.switch_printer(vi, &name);
        }
        if let Some(b) = bed_choice
            && b != self.profiles.bed
        {
            self.profiles.bed = b;
            self.settings_changed();
        }
        if open_all {
            self.dialogs.all_printers = true;
        }
        if open_add {
            self.open_add_printer();
        }
        if edit {
            self.dialogs.printer_editor = !self.dialogs.printer_editor;
        }
        if rules_clicked {
            // The switch is on the printer's first page.
            self.dialogs.printer_editor = true;
            self.editor.printer_page = crate::settings::pages(crate::settings::Tab::Printer)
                .iter()
                .position(|p| p.name == "Basic information")
                .unwrap_or(0);
        }
    }

    fn filament_card(&mut self, ui: &mut egui::Ui) {
        let p = theme::of(ui);
        let tools = self.profiles.filaments.len();
        let colours = self.tool_colours_rgba();
        let mut change: Option<(usize, String)> = None;
        let mut show_all = None;
        // (tool, act) from a picker's visible-filament list.
        let mut choose_acts: Vec<(usize, super::import::ChooseAct)> = Vec::new();
        let mut edit: Option<usize> = None;
        // The physical printer this profile prints on, and what it says differs per tool.
        let linked = self
            .linked_printer()
            .and_then(|i| self.printers.list.get(i))
            .map(|p| p.name.clone());
        let printer_side: Vec<Option<super::sync::PrinterFilament>> =
            (0..tools).map(|t| self.printer_filament(t)).collect();
        let summary =
            super::sync::summary_lines(&printer_side.iter().flatten().cloned().collect::<Vec<_>>());
        let mut sync = false;
        let mut ignore_all = false;
        // (tool, action) from a printer swatch's popover.
        let mut popover_action: Option<(usize, PopoverAction)> = None;
        let mut colour_pick: Option<(usize, super::colour::Pick)> = None;
        let recent = self.recent_colours();
        let mut mix_draft = self.mixes.draft.clone();
        let mut mix_acts = Vec::new();
        card(ui, |ui| {
            widgets::card_header(ui, if tools > 1 { "Filaments" } else { "Filament" }, |ui| {
                if tools == 1
                    && widgets::icon_button(
                        ui,
                        Glyph::Pencil,
                        "Filament settings: temperatures, cooling, flow",
                        true,
                    )
                    .clicked()
                {
                    edit = Some(0);
                }
                if let Some(name) = &linked
                    && widgets::icon_button(
                        ui,
                        Glyph::Sync,
                        &format!("Sync from printer: take each tool's color and material from what {name} has loaded"),
                        true,
                    )
                    .clicked()
                {
                    sync = true;
                }
            });
            // One quiet line when anything differs: colors as information, types as a
            // warning; the rows carry the details.
            for (k, (text, warning)) in summary.iter().enumerate() {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.;
                    ui.label(
                        egui::RichText::new(text)
                            .font(theme::sans(12.))
                            .color(if *warning { p.warn_text } else { p.muted }),
                    );
                    if k + 1 == summary.len() {
                        ui.label(
                            egui::RichText::new("·")
                                .font(theme::sans(12.))
                                .color(p.muted),
                        );
                        if widgets::link(ui, "Sync all")
                            .on_hover_text("Take every tool's color and material from the printer")
                            .clicked()
                        {
                            sync = true;
                        }
                        ui.label(
                            egui::RichText::new("·")
                                .font(theme::sans(12.))
                                .color(p.muted),
                        );
                        if widgets::link(ui, "Ignore")
                            .on_hover_text(
                                "Hide these differences until the printer reports other filament",
                            )
                            .clicked()
                        {
                            ignore_all = true;
                        }
                    }
                });
            }
            if !summary.is_empty() {
                ui.add_space(4.);
            }
            for (tool, colour) in colours.iter().enumerate().take(tools) {
                if tool > 0 {
                    ui.add_space(6.);
                }
                let theirs = printer_side.get(tool).cloned().flatten();
                let name = self.profiles.filaments[tool].clone();
                let changed = self
                    .overlays
                    .filaments
                    .get(tool)
                    .is_some_and(|o| !o.is_empty());
                // As the picker's row for it reads (told apart from the presets listed with it).
                let label = preset_names::label_in(
                    &name,
                    self.profiles
                        .materials
                        .iter()
                        .filter(|m| !self.profiles.filament_hidden(&self.prefs, m))
                        .map(|m| {
                            let other = self.profiles.is_incompatible(PresetKind::Filament, m);
                            (m.as_str(), other)
                        }),
                );
                let head = label.name.clone();
                let mut tail = label.suffix.clone();
                if changed {
                    tail = if tail.is_empty() {
                        "changed".into()
                    } else {
                        format!("{tail} · changed")
                    };
                }
                let id = ui.make_persistent_id(("filament-combo", tool));
                let colour_id = id.with("colour");
                let colour_open = egui::Popup::is_id_open(ui.ctx(), colour_id);
                let mut square = None;
                let printer_id = id.with("printer");
                let mut printer_square: Option<egui::Response> = None;
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.;
                    let combo_w = if tools > 1 {
                        ui.available_width() - widgets::ICON_BUTTON - 6.
                    } else {
                        ui.available_width()
                    };
                    let response = ui
                        .allocate_ui_with_layout(
                            Vec2::new(combo_w, widgets::COMBO_H),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| {
                                widgets::combo_button(
                                    ui,
                                    id,
                                    |ui| {
                                        if tools > 1 {
                                            ui.label(
                                                egui::RichText::new(format!("T{}", tool + 1))
                                                    .font(theme::mono(12.))
                                                    .color(p.muted),
                                            );
                                        }
                                        square = Some(
                                            super::colour::swatch_button(
                                                ui,
                                                *colour,
                                                16.,
                                                colour_open,
                                            )
                                            .on_hover_text(format!(
                                                "{}. Change the color",
                                                super::colour::describe(*colour)
                                            )),
                                        );
                                        if let Some(f) = &theirs {
                                            printer_square = Some(printer_swatch(
                                                ui,
                                                f,
                                                egui::Popup::is_id_open(ui.ctx(), printer_id),
                                            ));
                                        }
                                    },
                                    &head,
                                    &tail,
                                    None,
                                )
                            },
                        )
                        .inner;
                    let response = match preset_names::tooltip(&name, &label) {
                        Some(full) => response.on_hover_text(full),
                        None => response,
                    };
                    if tools > 1
                        && widgets::icon_button(ui, Glyph::Pencil, "Filament settings", true)
                            .clicked()
                    {
                        edit = Some(tool);
                    }
                    if let Some(square) = &square {
                        let title = if tools > 1 {
                            format!("Color of T{}", tool + 1)
                        } else {
                            "Filament color".to_string()
                        };
                        if let Some(pick) =
                            super::colour::popup(square, colour_id, &title, *colour, &recent)
                        {
                            colour_pick = Some((tool, pick));
                        }
                    }
                    // The list closes on a pick, not on clicks inside it (the visible-filament
                    // list and "Show profiles for other printers" keep it open).
                    let menu_id = id.with("menu");
                    // The screenshot tool's `--visible-filaments`: T1's list, open.
                    if tool == 0
                        && let Some(search) = ui.data_mut(|d| {
                            d.remove_temp::<String>(egui::Id::unique(
                                super::import::OPEN_VISIBLE_FILAMENTS,
                            ))
                        })
                    {
                        egui::Popup::open_id(ui.ctx(), menu_id);
                        ui.data_mut(|d| {
                            d.insert_temp(id.with("choosing"), true);
                            d.insert_temp(id.with("choose-search"), search);
                        });
                    } else if !egui::Popup::is_id_open(ui.ctx(), menu_id) {
                        super::import::reset_chooser(ui.ctx(), id);
                    }
                    let choosing = ui
                        .data(|d| d.get_temp::<bool>(id.with("choosing")))
                        .unwrap_or(false);
                    egui::Popup::menu(&response)
                        .id(menu_id)
                        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                        .width(response.rect.width().max(320.))
                        .show(|ui| {
                            let profiles = &self.profiles;
                            let prefs = &self.prefs;
                            if choosing {
                                let system: Vec<String> = profiles
                                    .materials
                                    .iter()
                                    .filter(|m| {
                                        !profiles.is_incompatible(PresetKind::Filament, m)
                                            && !profiles.is_user_preset(PresetKind::Filament, m)
                                    })
                                    .cloned()
                                    .collect();
                                let list = profiles.visible_filaments(prefs);
                                let acts = super::import::visible_chooser(
                                    ui,
                                    id,
                                    profiles,
                                    &system,
                                    |m| list.is_none_or(|l| l.iter().any(|n| n == m)),
                                );
                                choose_acts.extend(acts.into_iter().map(|a| (tool, a)));
                                return;
                            }
                            let shown: Vec<String> = profiles
                                .materials
                                .iter()
                                .filter(|m| !profiles.filament_hidden(prefs, m))
                                .cloned()
                                .collect();
                            let hidden = profiles
                                .materials
                                .iter()
                                .filter(|m| {
                                    !profiles.is_incompatible(PresetKind::Filament, m)
                                        && profiles.filament_hidden(prefs, m)
                                })
                                .count();
                            let pick = super::import::grouped_presets(
                                ui,
                                id,
                                profiles,
                                PresetKind::Filament,
                                &shown,
                                &name,
                                Some(super::import::Visibility { hidden }),
                                |ui, m, label| {
                                    if let Some(c) = profiles.filament_colour_rgba(m) {
                                        swatch(ui, c, 12.);
                                    } else {
                                        ui.add_space(12.);
                                    }
                                    let on = m == name;
                                    preset_row(ui, profiles, PresetKind::Filament, m, label, on)
                                },
                            );
                            match pick {
                                Some(MenuPick::Choose(m)) => {
                                    change = Some((tool, m));
                                    ui.close();
                                }
                                Some(MenuPick::ShowAll(on)) => show_all = Some(on),
                                Some(MenuPick::ChooseVisible) => {
                                    ui.data_mut(|d| d.insert_temp(id.with("choosing"), true));
                                }
                                Some(MenuPick::AllFilaments) => {
                                    choose_acts.push((tool, super::import::ChooseAct::ShowAll));
                                }
                                None => {}
                            }
                        });
                });
                // The printer swatch's popover: what differs and what to do about it.
                if let (Some(r), Some(f)) = (&printer_square, &theirs)
                    && let Some(a) = egui::Popup::menu(r)
                        .id(printer_id)
                        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                        .width(300.)
                        .show(|ui| difference_popover(ui, f))
                        .and_then(|r| r.inner)
                {
                    popover_action = Some((tool, a));
                }
            }
            // ColorMix can be switched off in Settings › Features.
            if self.prefs.features.color_mix {
                self.mixed_colors_section(ui, &mut mix_draft, &mut mix_acts);
            }
        });
        self.mixes.draft = mix_draft;
        self.apply_mix_acts(mix_acts);
        if let Some((tool, action)) = popover_action {
            let id = ui
                .make_persistent_id(("filament-combo", tool))
                .with("printer");
            egui::Popup::close_id(ui.ctx(), id);
            match action {
                PopoverAction::UseColour => self.use_printer_colour(tool),
                PopoverAction::UseFilament => self.use_printer_filament(tool),
                PopoverAction::Ignore => self.ignore_filament_difference(tool),
                PopoverAction::Close => {}
            }
        }
        if ignore_all {
            for (t, f) in printer_side.iter().enumerate() {
                if f.as_ref().is_some_and(|f| f.differs()) {
                    self.ignore_filament_difference(t);
                }
            }
        }
        if sync {
            let ctx = ui.ctx().clone();
            self.sync_filaments_from_printer(&ctx);
        }
        if let Some((tool, pick)) = colour_pick {
            self.set_filament_colour(tool, pick);
        }
        if let Some((tool, name)) = change
            && self.profiles.filaments[tool] != name
        {
            self.switch_filament(tool, name);
        }
        if let Some(on) = show_all {
            self.show_presets_for_other_printers(on);
        }
        for (tool, act) in choose_acts {
            let id = ui.make_persistent_id(("filament-combo", tool));
            self.choose_visible_filament(ui.ctx(), id, act);
        }
        if let Some(tool) = edit {
            self.dialogs.filament_editor = match self.dialogs.filament_editor {
                Some(t) if t == tool => None,
                _ => Some(tool),
            };
        }
    }

    pub(super) fn process_card(&mut self, ui: &mut egui::Ui) {
        let p = theme::of(ui);
        let current = self.profiles.process.clone();
        let changes = self.overlays.print.len();
        let modified = changes > 0;
        let is_user = self.profiles.is_user_preset(PresetKind::Print, &current);
        let mut choice = None;
        let mut other_printers = None;
        let mut save = false;
        let mut save_as = false;
        let mut discard = false;
        let show_all = self.prefs.advanced;
        // A new selection puts the scope bar on its deepest level.
        self.follow_selection_scope();
        let tower = self.editor.tower_mode && self.tower().is_some();
        // With a selection the scope bar (Profile | object | part) replaces the scope line;
        // every scope shows the topic rail with its own search.
        let bar = !tower && self.selected.is_some();
        let global = !tower && (bar || !self.editor.plate_mode);
        // Where the scope line starts: the quick tour's settings step shows from there down.
        let mut scope_top = None;
        // Header + combo + scope + search: the card's own padding; the list runs edge to edge.
        egui::Frame::new()
            .inner_margin(egui::Margin {
                left: 16,
                right: 16,
                top: 14,
                bottom: 0,
            })
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                widgets::card_header(ui, "Process", |ui| {
                    let more = widgets::icon_button(
                        ui,
                        Glyph::More,
                        "Save as, discard, show all settings",
                        true,
                    );
                    egui::Popup::menu(&more).show(|ui| {
                        ui.set_min_width(220.);
                        if ui.button("Save as new profile…").clicked() {
                            save_as = true;
                            ui.close();
                        }
                        if ui
                            .add_enabled(modified, egui::Button::new("Discard changes"))
                            .clicked()
                        {
                            discard = true;
                            ui.close();
                        }
                        ui.separator();
                        let mut all = show_all;
                        if ui.checkbox(&mut all, "Show all settings").changed() {
                            self.prefs.advanced = all;
                            self.prefs_dirty = true;
                            ui.close();
                        }
                    });
                    if widgets::icon_button(
                        ui,
                        Glyph::Save,
                        if is_user {
                            "Save changes into this profile"
                        } else {
                            "System presets are read-only; use ⋯ › Save as"
                        },
                        modified && is_user,
                    )
                    .clicked()
                    {
                        save = true;
                    }
                });
                let label = preset_names::label_in(
                    &current,
                    self.profiles.processes.iter().map(|o| {
                        let other = self.profiles.is_incompatible(PresetKind::Print, o);
                        (o.as_str(), other)
                    }),
                );
                let head = label.text();
                let trailing = match changes {
                    0 => String::new(),
                    1 => "● 1 change".to_string(),
                    n => format!("● {n} changes"),
                };
                let id = ui.make_persistent_id("process-combo");
                let response =
                    widgets::combo_button(ui, id, |_| {}, &head, &trailing, Some(p.accent_bright));
                let response = match preset_names::tooltip(&current, &label) {
                    Some(full) => response.on_hover_text(full),
                    None => response,
                };
                egui::Popup::menu(&response)
                    .id(id.with("menu"))
                    .width(response.rect.width())
                    .show(|ui| {
                        let profiles = &self.profiles;
                        let pick = super::import::grouped_presets(
                            ui,
                            id,
                            profiles,
                            PresetKind::Print,
                            &profiles.processes,
                            &current,
                            None,
                            |ui, o, label| {
                                preset_row(ui, profiles, PresetKind::Print, o, label, o == current)
                            },
                        );
                        match pick {
                            Some(MenuPick::Choose(o)) => {
                                choice = Some(o);
                                ui.close();
                            }
                            Some(MenuPick::ShowAll(on)) => other_printers = Some(on),
                            // Filament pickers only.
                            Some(MenuPick::ChooseVisible | MenuPick::AllFilaments) | None => {}
                        }
                    });
                ui.add_space(10.);
                scope_top = Some(ui.cursor().top());
                if bar {
                    self.scope_bar(ui);
                } else {
                    self.scope_line(ui);
                }
                if global {
                    // The topic list has its own search field and Changed filter.
                    return;
                }
                ui.add_space(10.);
                let searching = !self.editor.search.trim().is_empty();
                let annotation = if searching {
                    let n = self.search_result_count();
                    format!("{n} result{}", if n == 1 { "" } else { "s" })
                } else {
                    String::new()
                };
                let id = ui.make_persistent_id("process-search");
                if std::mem::take(&mut self.keys.focus_settings_search) {
                    // ⌘F (keymap.rs): the search field takes the keyboard.
                    ui.memory_mut(|m| m.request_focus(id.with("search")));
                }
                widgets::search_field(
                    ui,
                    id,
                    &mut self.editor.search,
                    "Search all settings",
                    &annotation,
                );
            });
        ui.add_space(10.);
        if tower {
            self.tower_settings(ui);
        } else if !bar && self.editor.plate_mode {
            self.plate_overrides(ui);
        } else {
            self.settings_list(ui);
        }
        if let Some(top) = scope_top {
            let x = ui.max_rect().x_range();
            let r = Rect::from_x_y_ranges(x, (top - 4.)..=ui.cursor().top());
            crate::tour::anchor(ui, crate::tour::Anchor::Settings, r);
        }
        if save && let Err(e) = self.save_profile(PresetKind::Print, 0, &current) {
            self.notify_error("The profile was not saved", e);
        }
        if save_as {
            self.dialogs.save_as = Some(SaveAs {
                kind: PresetKind::Print,
                tool: 0,
                name: format!("{current} - Copy"),
                error: None,
            });
        }
        if discard {
            self.record("Discard changes");
            self.overlays.print.clear();
            self.settings_changed();
        }
        if let Some(pr) = choice
            && pr != current
        {
            self.profiles.process = pr;
            self.overlays.process_switched();
            self.profiles.refresh();
            self.settings_changed();
        }
        if let Some(on) = other_printers {
            self.show_presets_for_other_printers(on);
        }
    }

    /// A filament picker's visible-filament list: a star toggled, "Show all" or "Done". Only
    /// what the picker lists changes; the selection and the slice don't.
    pub(crate) fn choose_visible_filament(
        &mut self,
        ctx: &egui::Context,
        id: egui::Id,
        act: super::import::ChooseAct,
    ) {
        use super::import::ChooseAct;
        match act {
            ChooseAct::Toggle(name, on) => {
                self.profiles
                    .set_filament_visible(&mut self.prefs, &name, on);
                self.prefs_dirty = true;
            }
            ChooseAct::ShowAll => {
                self.profiles.show_all_filaments(&mut self.prefs);
                self.prefs_dirty = true;
            }
            ChooseAct::Done => super::import::reset_chooser(ctx, id),
        }
    }

    /// The print and filament pickers' "Show profiles for other printers".
    pub(crate) fn show_presets_for_other_printers(&mut self, on: bool) {
        let before = (
            self.profiles.process.clone(),
            self.profiles.filaments.clone(),
        );
        self.profiles.show_incompatible = on;
        self.profiles.refresh();
        if before
            != (
                self.profiles.process.clone(),
                self.profiles.filaments.clone(),
            )
        {
            self.settings_changed();
        }
    }

    /// "Settings for All objects ▾", or the object-scope card when one object is edited.
    fn scope_line(&mut self, ui: &mut egui::Ui) {
        let p = theme::of(ui);
        if self.editor.tower_mode
            && let Some(t) = self.tower()
        {
            // The tower's scope: its rows are process (and filament) settings, not overrides.
            let name = t.label();
            let mut back = false;
            egui::Frame::new()
                .fill(p.selected_row)
                .stroke(Stroke::new(1., p.scope_border))
                .corner_radius(8.)
                .inner_margin(egui::Margin::symmetric(12, 10))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.spacing_mut().item_spacing.y = 2.;
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.;
                        ui.label(
                            egui::RichText::new("Settings for ")
                                .font(theme::sans(13.))
                                .color(p.muted_alt),
                        );
                        ui.label(
                            egui::RichText::new(name)
                                .font(theme::semibold(13.))
                                .color(p.text),
                        );
                    });
                    ui.label(
                        egui::RichText::new(
                            "Size, brim and purge of the tower. These are process settings: they apply to every plate.",
                        )
                        .font(theme::sans(12.))
                        .color(p.scope_text),
                    );
                    ui.add_space(4.);
                    if widgets::link(ui, "← Back to all objects").clicked() {
                        back = true;
                    }
                });
            if back {
                self.editor.tower_mode = false;
            }
            return;
        }
        let objects: Vec<String> = self
            .plates
            .current()
            .objects
            .iter()
            .map(|o| o.name.clone())
            .collect();
        let selected = self.selected.filter(|&i| i < objects.len());
        let scoped = self.editor.object_mode && selected.is_some();
        if !scoped && self.editor.plate_mode {
            self.plate_scope_card(ui);
            return;
        }
        let (row, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 20.), Sense::hover());
        let painter = ui.painter();
        let lead = painter.layout_no_wrap("Settings for ".into(), theme::sans(13.), p.muted_alt);
        painter.galley(
            Pos2::new(row.left(), row.center().y - lead.size().y / 2.),
            lead.clone(),
            p.muted_alt,
        );
        let g = painter.layout_no_wrap("All objects".into(), theme::medium(13.), p.text);
        let r = Rect::from_min_size(
            Pos2::new(row.left() + lead.size().x + 6., row.top()),
            Vec2::new(g.size().x + 14., row.height()),
        );
        let resp = ui.interact(r, ui.make_persistent_id("scope"), Sense::click());
        let color = if resp.hovered() { p.accent_hi } else { p.text };
        ui.painter().galley(
            Pos2::new(r.left(), row.center().y - g.size().y / 2.),
            g,
            color,
        );
        widgets::chevron(
            ui.painter(),
            Pos2::new(r.right() - 5., row.center().y + 1.),
            true,
            color,
        );
        if resp.hovered() {
            ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
        }
        let mut pick: Option<Option<usize>> = None;
        let mut plate_pick = false;
        egui::Popup::menu(&resp).show(|ui| {
            ui.set_min_width(220.);
            if ui.selectable_label(true, "All objects").clicked() {
                pick = Some(None);
                ui.close();
            }
            let own = self.plates.current().overrides.len();
            let plate = if own == 0 {
                "This plate".to_string()
            } else {
                format!(
                    "This plate ({own} own setting{})",
                    if own == 1 { "" } else { "s" }
                )
            };
            if ui
                .selectable_label(false, plate)
                .on_hover_text(
                    "Settings only this plate prints with, as OrcaSlicer's plate settings",
                )
                .clicked()
            {
                plate_pick = true;
                ui.close();
            }
            if !objects.is_empty() {
                ui.separator();
                ui.weak("Overrides for one object");
            }
            for (i, n) in objects.iter().enumerate() {
                if ui.selectable_label(false, n).clicked() {
                    pick = Some(Some(i));
                    ui.close();
                }
            }
        });
        if plate_pick {
            self.editor.plate_mode = true;
            self.editor.object_mode = false;
        }
        match pick {
            Some(None) => {
                self.editor.object_mode = false;
                self.editor.plate_mode = false;
            }
            Some(Some(i)) => {
                self.selected = Some(i);
                self.editor.object_mode = true;
                self.editor.plate_mode = false;
            }
            None => {}
        }
    }

    /// How many rows the search would show (for the "N results" annotation).
    fn search_result_count(&self) -> usize {
        let Some(config) = self.effective_config() else {
            return 0;
        };
        let sections = self.sections();
        let ctx = |r: &RowDef| FieldContext {
            config,
            overlay: self.overlays.get(r.target),
            preset_overrides: None,
            tool: tool_of(r.target),
            advanced: true,
            search: &self.editor.search,
            id: "count",
            force: true,
            inherit: None,
        };
        sections
            .iter()
            .flat_map(|s| s.groups.iter().chain(s.more.iter()))
            .flat_map(|g| {
                let keys: Vec<&str> = g.rows.iter().map(|r| r.key).collect();
                g.rows.iter().map(move |r| (r, keys.clone()))
            })
            .map(|(r, group_keys)| {
                let c = ctx(r);
                let own = usize::from(c.visible(r.key));
                let deps = if settings::dependents_active(r.key, config) {
                    settings::dependents(r.key)
                        .iter()
                        .filter(|k| {
                            !group_keys.contains(k)
                                && c.visible(k)
                                && crate::brim_ears::option_shown(k, config)
                        })
                        .count()
                } else {
                    0
                };
                own + deps
            })
            .sum()
    }

    /// The section definitions: Essentials, then the Orca pages plus cooling.
    pub(super) fn sections(&self) -> Vec<TopicDef> {
        let mut out = Vec::new();
        let curated = GroupDef {
            name: String::new(),
            id: String::new(),
            rows: settings::ESSENTIALS_CURATED
                .iter()
                .map(|(k, label)| RowDef {
                    label: label.to_string(),
                    key: k,
                    target: Target::Print,
                })
                .collect(),
        };
        let more = settings::ESSENTIALS_MORE
            .iter()
            .map(|(group, keys)| GroupDef {
                name: group.to_string(),
                id: group.to_string(),
                rows: keys
                    .iter()
                    .map(|k| RowDef {
                        label: settings::label(k),
                        key: k,
                        target: Target::Print,
                    })
                    .collect(),
            })
            .collect();
        out.push(TopicDef {
            name: "Essentials",
            groups: vec![curated],
            more,
        });
        let tools = self.tool_count();
        let mut pages: Vec<TopicDef> = settings::pages(Tab::Print)
            .iter()
            .filter(|page| page.name != "Multimaterial" || tools > 1)
            // Textures can be switched off in Settings › Features.
            .filter(|page| page.name != "Texture" || self.prefs.features.textures)
            .map(|page| {
                let groups = page
                    .groups
                    .iter()
                    // Brick layers: hidden unless turned on by hand in mica.toml.
                    .filter(|g| {
                        g.name != crate::brick_layers::GROUP
                            || self.prefs.features.brick_layers_enabled()
                    })
                    .map(|g| GroupDef {
                        name: g.name.clone(),
                        id: g.name.clone(),
                        rows: g
                            .lines
                            .iter()
                            .flat_map(|l| {
                                l.keys.iter().map(move |k| RowDef {
                                    label: if l.label.is_empty() || l.keys.len() == 1 {
                                        settings::label(k)
                                    } else {
                                        format!("{} · {}", l.label, settings::label(k))
                                    },
                                    key: k,
                                    target: Target::Print,
                                })
                            })
                            .collect(),
                    })
                    .collect();
                let name: &'static str = match page.name.as_str() {
                    "Quality" => "Quality",
                    "Strength" => "Strength",
                    "Speed" => "Speed",
                    "Support" => "Support",
                    "Multimaterial" => "Multimaterial",
                    "Texture" => "Texture",
                    _ => "Advanced",
                };
                TopicDef {
                    name,
                    groups,
                    more: vec![],
                }
            })
            .collect();
        // Cooling & temperatures (per tool) after Support.
        let mut cooling = Vec::new();
        for tool in 0..tools {
            let suffix = if tools > 1 {
                format!(
                    " · tool {} · {}",
                    tool + 1,
                    preset_names::display_name(&self.profiles.filaments[tool])
                )
            } else {
                String::new()
            };
            let tool_id = if tools > 1 {
                format!(" · tool {}", tool + 1)
            } else {
                String::new()
            };
            cooling.push(GroupDef {
                name: format!("Temperatures{suffix}"),
                id: format!("Temperatures{tool_id}"),
                rows: settings::ESSENTIAL_FILAMENT
                    .iter()
                    .map(|k| RowDef {
                        label: match *k {
                            "first_layer_temperature" => "Nozzle, first layer".into(),
                            "temperature" => "Nozzle, other layers".into(),
                            "first_layer_bed_temperature" => "Bed, first layer".into(),
                            "bed_temperature" => "Bed, other layers".into(),
                            _ => settings::label(k),
                        },
                        key: k,
                        target: Target::Filament(tool),
                    })
                    .collect(),
            });
            cooling.push(GroupDef {
                name: format!("Cooling{suffix}"),
                id: format!("Cooling{tool_id}"),
                rows: settings::COOLING
                    .iter()
                    .map(|k| RowDef {
                        label: match *k {
                            "min_fan_speed" => "Fan, minimum".into(),
                            "max_fan_speed" => "Fan, maximum".into(),
                            "bridge_fan_speed" => "Fan on bridges".into(),
                            "disable_fan_first_layers" => "No fan for the first".into(),
                            "slowdown_below_layer_time" => "Slow down below layer time".into(),
                            _ => settings::label(k),
                        },
                        key: k,
                        target: Target::Filament(tool),
                    })
                    .collect(),
            });
        }
        let at = pages
            .iter()
            .position(|s| s.name == "Multimaterial" || s.name == "Advanced")
            .unwrap_or(pages.len());
        pages.insert(
            at,
            TopicDef {
                name: "Cooling & temperatures",
                groups: cooling,
                more: vec![],
            },
        );
        out.extend(pages);
        out
    }
}

/// `15 %`, `2 loops`, `aligned`: a raw setting value for the "Plate: …" line.
pub(crate) fn pretty_value(key: &str, raw: &str) -> String {
    let unit = settings::text()
        .get(key)
        .map(|t| editor::unit_for(key, &t.unit))
        .unwrap_or("");
    let v = raw.trim();
    if let Some(pct) = v.strip_suffix('%') {
        return format!("{} %", pct.trim());
    }
    if v.parse::<f64>().is_ok() {
        return if unit.is_empty() {
            v.to_string()
        } else {
            format!("{v} {unit}")
        };
    }
    settings::text()
        .get(key)
        .and_then(|t| t.enums.iter().find(|(val, _)| val == v))
        .map_or(v.to_string(), |(_, l)| l.clone())
}

pub(super) fn dashed_rect(painter: &egui::Painter, r: Rect, radius: f32, stroke: Stroke) {
    let _ = radius;
    for (a, b) in [
        (r.left_top(), r.right_top()),
        (r.right_top(), r.right_bottom()),
        (r.right_bottom(), r.left_bottom()),
        (r.left_bottom(), r.left_top()),
    ] {
        painter.add(egui::Shape::dashed_line(&[a, b], stroke, 4., 4.));
    }
}

/// One of the project's values in the card's list: `T2 Nozzle temperature  225 (profile 215)`.
fn difference_row(ui: &mut egui::Ui, d: &mica_profiles::project::Difference, p: &theme::Palette) {
    let short = |t: &str| {
        let one_line = t.replace('\n', " ⏎ ");
        if one_line.chars().count() > 40 {
            format!("{}…", one_line.chars().take(40).collect::<String>())
        } else {
            one_line
        }
    };
    let name = match d.tool {
        Some(t) => format!("T{} {}", t + 1, crate::settings::label(&d.key)),
        None => crate::settings::label(&d.key),
    };
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 4.;
        ui.label(
            egui::RichText::new(name)
                .font(theme::sans(11.))
                .color(p.text),
        )
        .on_hover_text(&d.key);
        ui.label(
            egui::RichText::new(short(&d.project))
                .font(theme::sans(11.))
                .color(p.label),
        )
        .on_hover_text(&d.project);
        ui.label(
            egui::RichText::new(format!(
                "(profile {})",
                d.preset.as_deref().map_or_else(|| "—".to_string(), short)
            ))
            .font(theme::sans(11.))
            .color(p.muted),
        );
    });
}

pub(super) fn tool_of(target: Target) -> usize {
    match target {
        Target::Filament(t) => t,
        _ => 0,
    }
}

/// Collapsible section header (34 px): chevron, semibold title, muted summary on the right,
/// a 1 px rule above.
pub(super) fn section_header(
    ui: &mut egui::Ui,
    name: &str,
    open: bool,
    summary: &str,
    rule: bool,
) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), SECTION_H), Sense::click());
    paint_section_header(ui, rect, name, open, summary, response.hovered(), rule);
    if response.hovered() {
        ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
    }
    response
}

fn paint_section_header(
    ui: &egui::Ui,
    rect: Rect,
    name: &str,
    open: bool,
    summary: &str,
    hovered: bool,
    rule: bool,
) {
    let p = theme::of(ui);
    let painter = ui.painter();
    if rule {
        painter.line_segment(
            [rect.left_top(), rect.right_top()],
            Stroke::new(1., p.border),
        );
    }
    let color = if hovered { p.accent_hi } else { p.text };
    widgets::chevron(
        painter,
        Pos2::new(rect.left() + 16. + 6., rect.center().y),
        open,
        p.label,
    );
    let title = painter.layout_no_wrap(name.to_string(), theme::semibold(13.), color);
    let title_pos = Pos2::new(
        rect.left() + 16. + 12. + 8.,
        rect.center().y - title.size().y / 2.,
    );
    let title_w = title.size().x;
    painter.galley(title_pos, title, color);
    if !summary.is_empty() {
        let left = title_pos.x + title_w + 12.;
        let galley = painter.layout_no_wrap(summary.to_string(), theme::sans(12.), p.muted);
        let x = (rect.right() - MARGIN - galley.size().x).max(left);
        let clip = Rect::from_min_max(
            Pos2::new(left, rect.top()),
            Pos2::new(rect.right() - MARGIN, rect.bottom()),
        );
        painter.with_clip_rect(clip).galley(
            Pos2::new(x, rect.center().y - galley.size().y / 2.),
            galley,
            p.muted,
        );
    }
}

/// One line that says what a collapsed section currently does.
pub fn section_summary(name: &str, c: &Config) -> String {
    let f = |k: &str| c.float(k).unwrap_or(0.);
    let i = |k: &str| c.int(k).unwrap_or(0);
    let e = |k: &str| {
        let v = c.string(k).unwrap_or("");
        settings::text()
            .get(k)
            .and_then(|t| t.enums.iter().find(|(val, _)| val == v))
            .map_or(v.to_string(), |(_, l)| l.clone())
    };
    match name {
        "Essentials" => format!(
            "{:.2} mm · {} walls · {:.0} %",
            f("layer_height"),
            i("perimeters"),
            f("fill_density"),
        ),
        "Quality" => format!(
            "{} walls · {} seam",
            e("perimeter_generator"),
            e("seam_position").to_lowercase()
        ),
        "Strength" => format!(
            "{} walls · {:.0} % {}",
            i("perimeters"),
            f("fill_density"),
            e("fill_pattern").to_lowercase()
        ),
        "Speed" => format!("{:.0} mm/s outer wall", f("external_perimeter_speed")),
        "Support" => {
            if c.bool("support_material") != Some(true) {
                "Off".into()
            } else if c.bool("support_material_auto") == Some(false) {
                "Enforcers only".into()
            } else {
                format!(
                    "{} · {}{}",
                    if c.bool("support_material_buildplate_only") == Some(true) {
                        "Build plate only"
                    } else {
                        "Everywhere"
                    },
                    e("support_material_style").to_lowercase(),
                    if i("raft_layers") > 0 { " · raft" } else { "" }
                )
            }
        }
        "Cooling & temperatures" => format!(
            "{:.0} °C · {:.0} °C bed",
            c.float_at("temperature", 0).unwrap_or(0.),
            c.float_at("bed_temperature", 0).unwrap_or(0.)
        ),
        "Multimaterial" => {
            if c.bool("wipe_tower") == Some(true) {
                "Prime tower on".into()
            } else {
                "Prime tower off".into()
            }
        }
        _ => {
            let brim = if c.string("brim_type") == Some("no_brim") || f("brim_width") <= 0. {
                "no brim".to_string()
            } else {
                format!("{:.0} mm brim", f("brim_width"))
            };
            format!("{brim} · {} skirt loops · G-code output", i("skirts"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn summaries() {
        let mut c = Config::defaults();
        c.set_raw("layer_height", "0.2").unwrap();
        c.set_raw("perimeters", "2").unwrap();
        c.set_raw("fill_density", "15%").unwrap();
        c.set_raw("fill_pattern", "grid").unwrap();
        c.set_raw("support_material", "1").unwrap();
        c.set_raw("support_material_auto", "0").unwrap();
        c.set_raw("perimeter_generator", "arachne").unwrap();
        c.set_raw("seam_position", "aligned").unwrap();
        assert_eq!(
            section_summary("Essentials", &c),
            "0.20 mm · 2 walls · 15 %"
        );
        assert_eq!(
            section_summary("Quality", &c),
            "Arachne walls · aligned seam"
        );
        assert_eq!(section_summary("Strength", &c), "2 walls · 15 % grid");
        assert_eq!(section_summary("Support", &c), "Enforcers only");
        c.set_raw("support_material_auto", "1").unwrap();
        assert!(section_summary("Support", &c).starts_with("Everywhere"));
        c.set_raw("brim_type", "no_brim").unwrap();
        assert!(section_summary("Advanced", &c).starts_with("no brim"));
        c.set_raw("temperature", "215").unwrap();
        c.set_raw("bed_temperature", "60").unwrap();
        assert_eq!(
            section_summary("Cooling & temperatures", &c),
            "215 °C · 60 °C bed"
        );
    }
    #[test]
    fn names_split_like_the_mockup() {
        assert_eq!(
            split_printer_name("Prusa CORE One HF0.4 nozzle"),
            ("Prusa CORE One".into(), "HF 0.4".into())
        );
        assert_eq!(
            split_printer_name("Original Prusa XL - 5T Input Shaper 0.4 nozzle"),
            ("Original Prusa XL - 5T Input Shaper".into(), "0.4".into())
        );
        assert_eq!(
            split_printer_name("Snapmaker U1 (0.4 nozzle)"),
            ("Snapmaker U1".into(), "0.4".into())
        );
        assert_eq!(pretty_value("fill_density", "15%"), "15 %");
        assert_eq!(pretty_value("perimeters", "2"), "2 loops");
        assert_eq!(pretty_value("seam_position", "aligned"), "Aligned");
    }
}

/// What a printer-swatch popover asks the card to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PopoverAction {
    UseColour,
    UseFilament,
    Ignore,
    Close,
}

/// The printer's swatch beside the project's in a Filament row: smaller, with a dot when
/// the loaded filament differs (amber for another type, the accent for another color).
fn printer_swatch(
    ui: &mut egui::Ui,
    f: &super::sync::PrinterFilament,
    open: bool,
) -> egui::Response {
    let p = theme::of(ui);
    let differs = f.differs();
    // Room for the dot on every row, so the preset names line up.
    let w = 22.;
    let (rect, response) = ui.allocate_exact_size(Vec2::new(w, 16.), Sense::click());
    let sq = Rect::from_center_size(
        Pos2::new(rect.left() + 6., rect.center().y),
        Vec2::splat(11.),
    );
    match f.loaded_colour {
        Some(c) => {
            ui.painter()
                .rect_filled(sq, 2.5, egui::Color32::from_rgb(c[0], c[1], c[2]));
            ui.painter().rect_stroke(
                sq,
                2.5,
                Stroke::new(
                    1.,
                    if response.hovered() || open {
                        p.text
                    } else {
                        p.border_raised
                    },
                ),
                egui::StrokeKind::Inside,
            );
        }
        None => {
            ui.painter()
                .rect_stroke(sq, 2.5, Stroke::new(1., p.muted), egui::StrokeKind::Inside);
        }
    }
    if differs {
        let colour = if f.type_differs {
            p.warn_text
        } else {
            p.accent_bright
        };
        ui.painter()
            .circle_filled(Pos2::new(rect.right() - 3.5, rect.top() + 4.), 3.5, colour);
    }
    if response.hovered() {
        ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
    }
    response.on_hover_text(f.tooltip())
}

/// The popover's content: the differences in words and the choices.
fn difference_popover(
    ui: &mut egui::Ui,
    f: &super::sync::PrinterFilament,
) -> Option<PopoverAction> {
    let p = theme::of(ui);
    let mut action = None;
    ui.spacing_mut().item_spacing.y = 6.;
    let lines = f.lines();
    if lines.is_empty() {
        ui.label(
            egui::RichText::new(if f.ignored {
                "Difference ignored until the loaded filament changes"
            } else {
                "The printer has the project's filament loaded"
            })
            .font(theme::sans(12.5))
            .color(p.text),
        );
    }
    for l in &lines {
        ui.label(egui::RichText::new(l).font(theme::sans(12.5)).color(p.text));
    }
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.;
        if f.type_differs && !f.ignored {
            if widgets::primary_button(ui, "Use printer's filament", true).clicked() {
                action = Some(PopoverAction::UseFilament);
            }
        } else if f.colour_differs
            && !f.ignored
            && widgets::primary_button(ui, "Use printer's color", true).clicked()
        {
            action = Some(PopoverAction::UseColour);
        }
        if f.differs() {
            if widgets::button(ui, "Ignore", true)
                .on_hover_text("Until the printer reports other filament in this slot")
                .clicked()
            {
                action = Some(PopoverAction::Ignore);
            }
        } else if widgets::button(ui, "Close", true).clicked() {
            action = Some(PopoverAction::Close);
        }
    });
    action
}
