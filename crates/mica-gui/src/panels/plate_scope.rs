// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The sidebar's "Settings for This plate" scope: the current plate's own settings
//! (`Plate::overrides`, see `plate_settings`), listed and edited like an object's overrides,
//! each with the changed marker, a remove button and the global value under it, plus
//! "+ Add plate setting" with the plate options of OrcaSlicer's plate settings dialog first.
//! An empty list means the plate prints with the global settings.

use super::{
    editor::{self, Edit, FieldContext},
    sidebar::{dashed_rect, pretty_value, section_header},
    widgets::{self, Glyph, LEFT, MARGIN, ROW_H},
};
use crate::app::App;
use crate::plate_settings::{self, BED_TYPE};
use crate::settings::{self, Tab};
use crate::theme;
use eframe::egui::{self, Align2, Pos2, Rect, Sense, Stroke, Vec2};

impl App {
    /// The scope card while the plate's settings are shown.
    pub(crate) fn plate_scope_card(&mut self, ui: &mut egui::Ui) {
        let p = theme::of(ui);
        let i = self.plates.current;
        let name = self.plates.current().name.clone();
        let count = self.plates.current().overrides.len();
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
                        egui::RichText::new(&name)
                            .font(theme::semibold(13.))
                            .color(p.text),
                    );
                    if name != format!("Plate {}", i + 1) {
                        ui.label(
                            egui::RichText::new(format!("  plate {}", i + 1))
                                .font(theme::sans(12.))
                                .color(p.muted),
                        );
                    }
                });
                ui.label(
                    egui::RichText::new(if count == 0 {
                        "This plate prints with the global settings. Add a setting to change it for this plate only."
                    } else {
                        "Only this plate's own settings are listed. Everything else follows the global settings."
                    })
                    .font(theme::sans(12.))
                    .color(p.scope_text),
                );
                ui.add_space(4.);
                if widgets::link(ui, "← Back to all objects").clicked() {
                    back = true;
                }
            });
        if back {
            self.editor.plate_mode = false;
        }
    }

    /// The plate's own settings, each with the global value under it.
    pub(crate) fn plate_overrides(&mut self, ui: &mut egui::Ui) {
        let p = theme::of(ui);
        let i = self.plates.current;
        let overrides = self.plates.current().overrides.clone();
        let Some(global) = self.effective_config().cloned() else {
            return;
        };
        let config = match self.plate_config(i) {
            Ok(c) => c,
            Err(e) => {
                ui.colored_label(ui.visuals().error_fg_color, e);
                global.clone()
            }
        };
        let beds = self.profiles.is_orca();
        let searching = !self.editor.search.trim().is_empty();
        let ctx = FieldContext {
            config: &config,
            overlay: &overrides,
            preset_overrides: None,
            tool: 0,
            advanced: true,
            search: &self.editor.search,
            id: "plate",
            force: true,
            inherit: None,
        };
        let mut edits: Vec<Edit> = Vec::new();
        let mut remove: Option<String> = None;
        section_header(
            ui,
            "This plate's settings",
            true,
            &if overrides.is_empty() {
                "none".to_string()
            } else {
                overrides.len().to_string()
            },
            true,
        );
        if overrides.is_empty() && !searching {
            ui.horizontal(|ui| {
                ui.add_space(LEFT);
                ui.label(
                    egui::RichText::new("None yet: this plate prints with the global settings.")
                        .font(theme::sans(12.))
                        .color(p.muted),
                );
            });
            ui.add_space(6.);
        }
        // Orca's plate options first, in its dialog's order; then the rest by label.
        let mut keys: Vec<&str> = overrides.keys().map(String::as_str).collect();
        keys.sort_by_key(|k| {
            (
                plate_settings::PLATE_OPTIONS
                    .iter()
                    .position(|o| o == k)
                    .unwrap_or(usize::MAX),
                plate_settings::label(k),
            )
        });
        for key in keys {
            if !settings::matches(key, &self.editor.search) && key != BED_TYPE {
                continue;
            }
            let (row_rect, _) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H), Sense::hover());
            let inner = Rect::from_min_max(
                row_rect.min,
                Pos2::new(row_rect.right() - 28., row_rect.bottom()),
            );
            let mut child = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(inner)
                    .layout(egui::Layout::top_down(egui::Align::Min)),
            );
            if key == BED_TYPE {
                bed_row(&mut child, &overrides[key], beds, &mut edits);
            } else {
                editor::row(&mut child, key, None, &ctx, &mut edits);
            }
            let x = Rect::from_center_size(
                Pos2::new(row_rect.right() - MARGIN - 2., row_rect.center().y),
                Vec2::splat(24.),
            );
            let mut xb = ui.new_child(egui::UiBuilder::new().max_rect(x));
            if widgets::icon_button_sized(
                &mut xb,
                Glyph::Close,
                "Back to the global setting",
                true,
                24.,
            )
            .clicked()
            {
                remove = Some(key.to_string());
            }
            let global_text = if key == BED_TYPE {
                if beds {
                    self.profiles.bed.clone()
                } else {
                    "this printer has no bed types; not applied".into()
                }
            } else {
                let raw = crate::overlays::tool_raw(&global, key, 0).unwrap_or_default();
                let boolean = mica_core::config::option_defs().get(key).is_some_and(|d| {
                    matches!(
                        d.ty,
                        mica_core::config::OptType::Bool | mica_core::config::OptType::Bools
                    )
                });
                if boolean {
                    if matches!(raw.trim(), "1" | "true") {
                        "on".into()
                    } else {
                        "off".into()
                    }
                } else {
                    pretty_value(key, &raw)
                }
            };
            let (line, _) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), 16.), Sense::hover());
            ui.painter().text(
                Pos2::new(line.left() + LEFT, line.center().y),
                Align2::LEFT_CENTER,
                format!("All plates: {global_text}"),
                theme::sans(11.),
                p.muted,
            );
            ui.add_space(2.);
        }
        if searching {
            widgets::group_title(ui, "Add a plate setting");
            let mut n = 0;
            for page in settings::pages(Tab::Print) {
                for g in &page.groups {
                    for l in &g.lines {
                        for key in &l.keys {
                            if !overrides.contains_key(*key)
                                && settings::matches(key, &self.editor.search)
                                && (!crate::brick_layers::is_option(key)
                                    || self.prefs.features.brick_layers_enabled())
                                && n < 40
                            {
                                editor::row(ui, key, None, &ctx, &mut edits);
                                n += 1;
                            }
                        }
                    }
                }
            }
            if n == 0 {
                ui.horizontal(|ui| {
                    ui.add_space(LEFT);
                    ui.weak("No setting matches");
                });
            }
        }
        ui.add_space(12.);
        let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 32.), Sense::hover());
        let r = Rect::from_min_max(
            Pos2::new(r.left() + 16., r.top()),
            Pos2::new(r.right() - 16., r.bottom()),
        );
        let resp = ui.interact(
            r,
            ui.make_persistent_id("add-plate-setting"),
            Sense::click(),
        );
        let stroke = Stroke::new(
            1.,
            if resp.hovered() {
                p.accent
            } else {
                p.border_raised
            },
        );
        dashed_rect(ui.painter(), r, 6., stroke);
        ui.painter().text(
            r.center(),
            Align2::CENTER_CENTER,
            "+ Add plate setting",
            theme::sans(13.),
            p.label,
        );
        egui::Popup::menu(&resp).show(|ui| {
            ui.set_min_width(220.);
            for key in plate_settings::PLATE_OPTIONS {
                if key == BED_TYPE && !beds {
                    continue;
                }
                let present = overrides.contains_key(key);
                if ui
                    .add_enabled(!present, egui::Button::new(plate_settings::label(key)))
                    .clicked()
                {
                    let raw = if key == BED_TYPE {
                        self.profiles.bed.clone()
                    } else {
                        crate::overlays::tool_raw(&config, key, 0).unwrap_or_default()
                    };
                    edits.push(Edit::Set {
                        key: key.to_string(),
                        raw,
                    });
                    ui.close();
                }
            }
            ui.separator();
            for (group, keys) in settings::OBJECT_OVERRIDES {
                ui.menu_button(*group, |ui| {
                    for key in *keys {
                        let present = overrides.contains_key(*key);
                        if ui
                            .add_enabled(!present, egui::Button::new(settings::label(key)))
                            .clicked()
                        {
                            let raw =
                                crate::overlays::tool_raw(&config, key, 0).unwrap_or_default();
                            edits.push(Edit::Set {
                                key: key.to_string(),
                                raw,
                            });
                            ui.close();
                        }
                    }
                });
            }
        });
        ui.add_space(8.);
        ui.horizontal(|ui| {
            ui.add_space(16.);
            ui.label(
                egui::RichText::new("Search above to add any other setting for this plate.")
                    .font(theme::sans(12.))
                    .color(p.muted),
            );
        });
        ui.add_space(18.);
        if let Some(key) = remove {
            edits.push(Edit::Reset(key));
        }
        for e in edits {
            self.apply_plate_edit(e);
        }
    }

    /// One edit of the current plate's own settings: one undo step per gesture; only this
    /// plate's slice goes. A locked plate keeps its layout, not its settings (as in Orca).
    pub(crate) fn apply_plate_edit(&mut self, edit: Edit) {
        let label = match &edit {
            Edit::Set { key, .. } => format!("Change plate {}", plate_settings::label(key)),
            Edit::Reset(key) => format!("Reset plate {}", plate_settings::label(key)),
        };
        self.begin_gesture(&label);
        let overrides = &mut self.plates.current_mut().overrides;
        match edit {
            Edit::Set { key, raw } => {
                overrides.insert(key, raw);
            }
            Edit::Reset(key) => {
                overrides.remove(&key);
            }
        }
        self.changed();
    }
}

/// The bed type row: the plate's bed in a menu of the printer's beds, with the changed
/// marker (it is set, or it would not be listed).
fn bed_row(ui: &mut egui::Ui, bed: &str, beds: bool, edits: &mut Vec<Edit>) {
    let p = theme::of(ui);
    let (row, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(row, 0., p.changed_row);
    painter.rect_filled(
        Rect::from_min_max(
            Pos2::new(row.left(), row.top() + 4.),
            Pos2::new(row.left() + 3., row.bottom() - 4.),
        ),
        2.,
        p.accent,
    );
    painter.text(
        Pos2::new(row.left() + LEFT, row.center().y),
        Align2::LEFT_CENTER,
        "Bed type",
        theme::sans(13.),
        if beds { p.text } else { p.muted },
    );
    let control = widgets::control_rect(row);
    let resp = ui.interact(control, ui.make_persistent_id("plate-bed"), Sense::click());
    let color = if resp.hovered() { p.accent_hi } else { p.text };
    ui.painter().rect_stroke(
        control,
        6.,
        Stroke::new(1., p.border_raised),
        egui::StrokeKind::Inside,
    );
    ui.painter().text(
        Pos2::new(control.left() + 8., control.center().y),
        Align2::LEFT_CENTER,
        bed,
        theme::sans(12.5),
        color,
    );
    widgets::chevron(
        ui.painter(),
        Pos2::new(control.right() - 10., control.center().y + 1.),
        true,
        color,
    );
    if beds {
        egui::Popup::menu(&resp).show(|ui| {
            for b in super::profiles::SNAPMAKER_BEDS {
                if ui.selectable_label(b == bed, b).clicked() {
                    edits.push(Edit::Set {
                        key: BED_TYPE.into(),
                        raw: b.into(),
                    });
                    ui.close();
                }
            }
        });
    }
}
