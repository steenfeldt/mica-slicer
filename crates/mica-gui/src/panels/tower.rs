// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The prime / wipe tower's tool card (X / Y / rotation, its size, a free spot, disable) and
//! its sidebar scope ("Settings for Prime tower": the tower rows of the process settings, the
//! purge volumes, and the Orca-only wall / rib options). One typed value is one undo entry.

use super::{
    editor::{self, Edit, FieldContext},
    subcards::{card, single},
    widgets::{self, LEFT, MARGIN, ROW_H},
};
use crate::app::App;
use crate::overlays::Target;
use crate::settings;
use crate::theme;
use crate::tower::TowerCommand;
use eframe::egui::{self, Align2, Pos2, Rect, Sense, Vec2};

/// Process rows of the tower, in the order the card and Orca show them.
const PROCESS_KEYS: &[&str] = &[
    "wipe_tower",
    "wipe_tower_width",
    "wipe_tower_brim_width",
    "wipe_tower_cone_angle",
    "wipe_tower_extra_spacing",
    "wipe_tower_extra_flow",
    "wipe_tower_bridging",
    "wipe_tower_no_sparse_layers",
    "wipe_tower_extruder",
];

/// Orca-only tower options the option tables do not type: `(key, label, unit)`.
const ORCA_NUMBERS: &[(&str, &str, &str)] = &[
    ("prime_volume", "Prime volume", "mm³"),
    ("wipe_tower_rib_width", "Rib width", "mm"),
    ("wipe_tower_extra_rib_length", "Extra rib length", "mm"),
];
const ORCA_WALL_TYPES: &[(&str, &str)] =
    &[("rectangle", "Rectangle"), ("cone", "Cone"), ("rib", "Rib")];

impl App {
    /// The floating card for the selected tower, where an object's card would be.
    pub(crate) fn tower_window(&mut self, ctx: &egui::Context) {
        let Some(t) = self.tower().cloned() else {
            self.tower_selected = false;
            return;
        };
        let plate = self.plates.current;
        let placement = self.tower_placement(plate);
        let rotates = self
            .effective_config()
            .is_some_and(crate::tower::rotation_supported);
        let locked = self.plates.current().locked;
        let mode = self.viewport.mode;
        let title = match mode {
            crate::viewport::gizmo::Mode::Rotate => format!("{} · Rotate", t.label()),
            _ => format!("{} · Move", t.label()),
        };
        let mut edit: Option<(usize, f64)> = None;
        let mut command: Option<TowerCommand> = None;
        let viewport = self.viewport_rect;
        card(ctx, viewport, &title, |ui| {
            let p = theme::of(ui);
            let s = &t.shape;
            if let Some(v) = single(
                ui,
                "X mm",
                "tower-x",
                placement[0] as f32,
                "",
                1,
                -1e4,
                1e4,
                0.5,
            ) {
                edit = Some((0, v as f64));
            }
            if let Some(v) = single(
                ui,
                "Y mm",
                "tower-y",
                placement[1] as f32,
                "",
                1,
                -1e4,
                1e4,
                0.5,
            ) {
                edit = Some((1, v as f64));
            }
            if rotates
                && let Some(v) = single(
                    ui,
                    "Rotation °",
                    "tower-rot",
                    placement[2] as f32,
                    "",
                    0,
                    -1e4,
                    1e4,
                    1.,
                )
            {
                edit = Some((2, v as f64));
            }
            ui.add_space(4.);
            let (row, _) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), 16.), Sense::hover());
            ui.painter().text(
                Pos2::new(row.left(), row.center().y),
                Align2::LEFT_CENTER,
                format!(
                    "{:.1} × {:.1} mm · {:.1} mm tall · {}",
                    s.width,
                    s.depth,
                    s.height,
                    if s.exact {
                        "exact (sliced)"
                    } else {
                        "estimated before slicing"
                    }
                ),
                theme::sans(11.),
                p.muted,
            );
            let (min, max) = s.bbox();
            let (row, _) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), 16.), Sense::hover());
            ui.painter().text(
                Pos2::new(row.left(), row.center().y),
                Align2::LEFT_CENTER,
                format!(
                    "Footprint with brim: X {:.1}–{:.1}  Y {:.1}–{:.1}",
                    min[0], max[0], min[1], max[1]
                ),
                theme::sans(11.),
                p.muted,
            );
            for problem in &t.problems {
                let (row, _) =
                    ui.allocate_exact_size(Vec2::new(ui.available_width(), 16.), Sense::hover());
                ui.painter().text(
                    Pos2::new(row.left(), row.center().y),
                    Align2::LEFT_CENTER,
                    format!(
                        "! {} — slicing is blocked until it is moved",
                        problem.text()
                    ),
                    theme::sans(11.),
                    theme::WARNING,
                );
            }
            ui.add_space(6.);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.;
                if widgets::styled_button(ui, "Free spot", !locked, false, 26.)
                    .on_hover_text("Move the tower to a bed corner or edge clear of every object")
                    .clicked()
                {
                    command = Some(TowerCommand::FreeSpot);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if widgets::styled_button(ui, &format!("Disable {}", t.label().to_lowercase()), true, false, 26.)
                        .on_hover_text("Switch the tower off in the process settings; the objects then print without one")
                        .clicked()
                    {
                        command = Some(TowerCommand::Disable);
                    }
                });
            });
            ui.add_space(4.);
            if widgets::link(ui, &format!("{} settings…", t.label())).clicked() {
                command = Some(TowerCommand::Settings);
            }
        });
        if let Some((axis, v)) = edit {
            self.set_tower_value(axis, v);
        }
        if let Some(c) = command {
            self.run_tower_command(c);
        }
    }

    /// The sidebar in tower scope: the tower's rows of the process settings, the purge
    /// volumes per tool and, on Orca printers, the wall / rib options.
    pub(crate) fn tower_settings(&mut self, ui: &mut egui::Ui) {
        let p = theme::of(ui);
        let Some(config) = self.effective_config().cloned() else {
            return;
        };
        let orca = mica_core::rules::Rules::of(&config).is_orca();
        let tools = self.tool_count();
        let name = self.tower_name();
        let mut edits: Vec<(Target, Edit)> = Vec::new();
        let searching = !self.editor.search.trim().is_empty();
        let ctx_for = |target: Target, tool: usize| FieldContext {
            config: &config,
            overlay: self.overlays.get(target),
            preset_overrides: None,
            tool,
            advanced: true,
            search: &self.editor.search,
            id: "tower",
            force: true,
            inherit: None,
        };
        widgets::group_title(ui, name);
        let ctx = ctx_for(Target::Print, 0);
        for key in PROCESS_KEYS {
            if !ctx.visible(key) {
                continue;
            }
            let mut e = Vec::new();
            editor::row(ui, key, None, &ctx, &mut e);
            edits.extend(e.into_iter().map(|e| (Target::Print, e)));
        }
        if orca {
            // Orca extras are text in the configuration; plain number fields edit them.
            let mut extra_edits: Vec<(String, String)> = Vec::new();
            for (key, label, unit) in ORCA_NUMBERS {
                if !settings::matches(key, &self.editor.search)
                    && !label
                        .to_lowercase()
                        .contains(&self.editor.search.to_lowercase())
                {
                    continue;
                }
                let modified = self.overlays.print.contains_key(*key);
                let value = mica_core::wipe_tower::number(&config, key, 0.);
                let (row, _) =
                    ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H), Sense::hover());
                if modified {
                    ui.painter().rect_filled(row, 0., p.changed_row);
                    ui.painter().rect_filled(
                        Rect::from_min_max(
                            Pos2::new(row.left(), row.top() + 4.),
                            Pos2::new(row.left() + 3., row.bottom() - 4.),
                        ),
                        2.,
                        p.accent,
                    );
                }
                ui.painter().text(
                    Pos2::new(row.left() + LEFT, row.center().y),
                    Align2::LEFT_CENTER,
                    *label,
                    theme::sans(13.),
                    if modified { p.text } else { p.label },
                );
                let control = widgets::control_rect(row);
                let spec = widgets::NumberSpec {
                    unit,
                    decimals: 1,
                    min: 0.,
                    max: 1e4,
                    speed: 0.5,
                };
                let id = ui.make_persistent_id(("tower-extra", key));
                if let Some(v) = widgets::number_with(ui, id, control, value, &spec, true, false) {
                    extra_edits.push((key.to_string(), format!("{v}")));
                }
            }
            // Wall type and fillet: a combo and a switch over the extras.
            if settings::matches("wipe_tower_wall_type", &self.editor.search)
                || "wall type".contains(&self.editor.search.to_lowercase())
            {
                // Orca-only keys are text extras in the configuration.
                let current = config
                    .string("wipe_tower_wall_type")
                    .or_else(|| config.extra("wipe_tower_wall_type"))
                    .unwrap_or("rib")
                    .trim()
                    .to_string();
                let (row, _) =
                    ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H), Sense::hover());
                ui.painter().text(
                    Pos2::new(row.left() + LEFT, row.center().y),
                    Align2::LEFT_CENTER,
                    "Wall type",
                    theme::sans(13.),
                    p.label,
                );
                let control = widgets::control_rect(row);
                let shown = ORCA_WALL_TYPES
                    .iter()
                    .find(|(v, _)| *v == current)
                    .map_or(current.as_str(), |(_, l)| l);
                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(control));
                egui::ComboBox::from_id_salt("tower-wall-type")
                    .width(control.width())
                    .selected_text(shown)
                    .show_ui(&mut child, |ui| {
                        for (v, l) in ORCA_WALL_TYPES {
                            if ui.selectable_label(*v == current, *l).clicked() {
                                extra_edits.push(("wipe_tower_wall_type".into(), v.to_string()));
                            }
                        }
                    });
            }
            if settings::matches("wipe_tower_fillet_wall", &self.editor.search)
                || "fillet".contains(&self.editor.search.to_lowercase())
            {
                let on = config.bool("wipe_tower_fillet_wall").unwrap_or_else(|| {
                    config
                        .extra("wipe_tower_fillet_wall")
                        .is_none_or(|v| matches!(v.trim(), "1" | "true"))
                });
                let (row, _) =
                    ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H), Sense::hover());
                ui.painter().text(
                    Pos2::new(row.left() + LEFT, row.center().y),
                    Align2::LEFT_CENTER,
                    "Fillet the rib walls",
                    theme::sans(13.),
                    p.label,
                );
                let switch = Rect::from_min_size(
                    Pos2::new(row.right() - MARGIN - 34., row.center().y - 10.),
                    Vec2::new(34., 20.),
                );
                let id = ui.make_persistent_id("tower-fillet");
                if let Some(v) = widgets::toggle_at(ui, id, switch, on, true) {
                    extra_edits.push((
                        "wipe_tower_fillet_wall".into(),
                        if v { "1" } else { "0" }.into(),
                    ));
                }
            }
            for (key, raw) in extra_edits {
                edits.push((Target::Print, Edit::Set { key, raw }));
            }
        }
        // Purge volumes: per tool (filament), plus the Prusa printer-level purging volume.
        let mut purge_rows = 0;
        for tool in 0..tools {
            let key = "filament_minimal_purge_on_wipe_tower";
            let ctx = ctx_for(Target::Filament(tool), tool);
            if !ctx.visible(key) {
                continue;
            }
            if purge_rows == 0 {
                widgets::group_title(ui, "Purge volumes");
            }
            purge_rows += 1;
            let label = format!(
                "Minimal purge · tool {}{}",
                tool + 1,
                if tools > 1 {
                    format!(
                        " · {}",
                        crate::preset_names::display_name(&self.profiles.filaments[tool])
                    )
                } else {
                    String::new()
                }
            );
            let mut e = Vec::new();
            editor::row(ui, key, Some(&label), &ctx, &mut e);
            edits.extend(e.into_iter().map(|e| (Target::Filament(tool), e)));
        }
        if !orca {
            let key = "multimaterial_purging";
            let ctx = ctx_for(Target::Printer, 0);
            if mica_core::config::option_defs().contains_key(key) && ctx.visible(key) {
                if purge_rows == 0 {
                    widgets::group_title(ui, "Purge volumes");
                }
                let mut e = Vec::new();
                editor::row(ui, key, None, &ctx, &mut e);
                edits.extend(e.into_iter().map(|e| (Target::Printer, e)));
            }
        }
        if !searching {
            ui.add_space(6.);
            ui.horizontal(|ui| {
                ui.add_space(LEFT);
                ui.label(
                    egui::RichText::new(
                        "Position and rotation are on the tower's card and in the 3D view; they are kept per plate.",
                    )
                    .font(theme::sans(12.))
                    .color(p.muted),
                );
            });
        }
        ui.add_space(12.);
        for (target, e) in edits {
            self.apply_edit(Some(target), e);
        }
    }
}
