// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Setting rows: a label on the left, one control in the fixed control column on the right,
//! a changed marker (tinted row, accent bar, inline reset), greyed-out dependents with a
//! reason, search highlights and plain-language tooltips. The editor never mutates the
//! configuration directly; it returns `Edit`s for the app to apply, so the same rows serve
//! global, per-object, printer and filament editing.

use super::widgets::{self, CONTROL_W, Glyph, LEFT, MARGIN, RESET_W, ROW_H};
use crate::overlays::{Overlay, tool_raw};
use crate::settings::{self, Page};
use crate::theme;
use eframe::egui::{self, Align2, Pos2, Rect, Sense, Vec2};
use mica_core::config::{
    Config, OptType, Scope, Value, escape_string, escape_strings, option_defs, serialize_value,
    unescape_string, unescape_strings,
};
use std::collections::BTreeMap;

pub use crate::settings::{SUPPORT_MODE, TOP_BOTTOM};
/// `complete_objects` (Orca's print sequence), shown as a two-way choice.
const PRINT_ORDER: &str = "complete_objects";

#[derive(Clone, Debug, PartialEq)]
pub enum Edit {
    Set { key: String, raw: String },
    Reset(String),
}

/// Everything a row needs to draw itself.
pub struct FieldContext<'a> {
    /// The effective configuration (presets + overlays).
    pub config: &'a Config,
    /// Unsaved changes of the target being edited; keys here show the changed marker.
    pub overlay: &'a Overlay,
    /// The user preset's own overrides relative to its system parent, if the selected preset
    /// is a user profile (shown with a second marker, not resettable here).
    pub preset_overrides: Option<&'a BTreeMap<String, String>>,
    /// Tool / extruder whose vector element is edited.
    pub tool: usize,
    /// Show every setting, not only the common ones.
    pub advanced: bool,
    pub search: &'a str,
    /// Salt distinguishing editors that show the same keys (global vs object, tools).
    pub id: &'a str,
    /// Show every key regardless of Simple/Advanced (curated lists).
    pub force: bool,
    /// Editing an object's or a part's overrides (the scope bar): what the rows inherit.
    pub inherit: Option<Inherit<'a>>,
}

/// What a row edited at an object or part scope inherits: the configuration without the
/// scope's own overrides and the words for its line under an override.
#[derive(Clone, Copy)]
pub struct Inherit<'a> {
    pub config: &'a Config,
    /// The scope's name (`3DBenchy`, `Hull infill`).
    pub scope: &'a str,
    /// A part: the line reads `was 2` (the object's value) instead of `3DBenchy · profile: 2`.
    pub part: bool,
}

impl FieldContext<'_> {
    /// The vector element of `key` this editor shows (`None`: the whole value). The printer
    /// editor's extruder selection only applies to per-extruder options
    /// (`overlays::printer_element`), as `App::apply_edit` writes them.
    pub fn element(&self, key: &str) -> Option<usize> {
        if self.id == "printer" {
            crate::overlays::printer_element(key, self.tool)
        } else {
            Some(self.tool)
        }
    }

    /// `element`, with the first element for a whole value.
    fn tool_of(&self, key: &str) -> usize {
        self.element(key).unwrap_or(0)
    }

    /// Whether a key would be shown without search: common, forced, advanced mode, or changed;
    /// and it applies under the printer's slicing rules (a changed one stays, to be reset).
    pub fn shown(&self, key: &str) -> bool {
        // A merged setting's PrusaSlicer counterpart never shows: the Orca key stands for it
        // (its edits change both).
        // Settings Mica does not implement are hidden too (`settings::unimplemented`).
        !settings::merged_away(key)
            && ((settings::applies(key, mica_core::rules::Rules::of(self.config))
                && settings::unimplemented::implemented(key, self.config))
                || self.modified(key))
            && (self.force || self.advanced || self.modified(key) || !is_advanced(key))
    }
    pub fn visible(&self, key: &str) -> bool {
        matches(key, self.search) && self.shown(key)
    }
    /// Whether the key (or any real key behind a synthetic one) carries an unsaved change.
    pub fn modified(&self, key: &str) -> bool {
        let real = settings::synthetic_keys(key);
        if real.is_empty() {
            self.overlay.contains_key(key)
                || counterpart(key).is_some_and(|k| self.overlay.contains_key(k))
        } else {
            real.iter().any(|k| self.overlay.contains_key(*k))
        }
    }
    fn preset_modified(&self, key: &str) -> bool {
        let real = settings::synthetic_keys(key);
        self.preset_overrides.is_some_and(|o| {
            if real.is_empty() {
                o.contains_key(key) || counterpart(key).is_some_and(|k| o.contains_key(k))
            } else {
                real.iter().any(|k| o.contains_key(*k))
            }
        })
    }
}

/// The PrusaSlicer counterpart a merged setting's row also stands for (`mica_core::merged`).
fn counterpart(key: &str) -> Option<&'static str> {
    mica_core::merged::by_orca(key).map(|p| p.prusa)
}

/// Search match for real and synthetic keys.
pub fn matches(key: &str, query: &str) -> bool {
    let real = settings::synthetic_keys(key);
    if real.is_empty() {
        settings::matches(key, query)
    } else {
        let q = query.trim().to_lowercase();
        q.is_empty()
            || real.iter().any(|k| settings::matches(k, query))
            || ["supports", "top / bottom layers"]
                .iter()
                .any(|l| l.contains(&q))
    }
}

/// Hidden in Simple mode.
pub fn is_advanced(key: &str) -> bool {
    settings::text().get(key).is_some_and(|t| t.advanced)
}

/// Why a setting currently has no effect, if so (greyed out with this tooltip).
pub fn disabled_reason(key: &str, c: &Config) -> Option<String> {
    let off = |k: &str| c.bool(k) == Some(false);
    let zero = |k: &str| c.float(k).is_some_and(|v| v <= 0.);
    let spiral = mica_core::spiral_vase::enabled(c);
    let reason = match key {
        // Spiral vase forces these while it is on (mica_core::spiral_vase::normalize).
        "perimeters" if spiral => "Spiral vase prints one wall while it is on",
        "top_solid_layers" if spiral => "Spiral vase prints no top layers while it is on",
        "fill_density" if spiral => "Spiral vase prints no infill while it is on",
        k if spiral
            && (k == "fill_pattern" || k == "fill_angle" || k.starts_with("infill_anchor")) =>
        {
            "No infill while Spiral vase is on"
        }
        "retract_layer_change" if spiral => {
            "Spiral vase never retracts at a layer change: the wall runs on into the next layer"
        }
        "brim_width" | "brim_separation" if c.string("brim_type") == Some("no_brim") => {
            "No effect while Brim type is “No brim”"
        }
        // Orca: placed ears have their own size, and the auto brim works out its width.
        "brim_width" if matches!(c.string("brim_type"), Some("painted" | "auto_brim")) => {
            "Not used by this brim type"
        }
        "brim_ears_max_angle" | "brim_ears_detection_length" if zero("brim_width") => {
            "No effect while the brim width is 0"
        }
        k if k.starts_with("support_material_")
            && k != "support_material"
            && off("support_material") =>
        {
            "No effect while supports are off"
        }
        k if k.starts_with("support_tree_") && off("support_material") => {
            "No effect while supports are off"
        }
        "support_material_threshold" if off("support_material_auto") => {
            "Only automatic supports use the overhang angle; enforcers ignore it"
        }
        k if k.starts_with("support_tree_")
            && c.string("support_material_style")
                .is_some_and(|s| s != "organic" && s != "tree") =>
        {
            "Only the Organic support style uses tree settings"
        }
        k if (k.starts_with("skirt_") || k == "min_skirt_length" || k == "draft_shield")
            && c.int("skirts").is_some_and(|n| n <= 0) =>
        {
            "No effect while Skirt loops is 0"
        }
        k if (k == "fill_pattern" || k == "fill_angle" || k.starts_with("infill_anchor"))
            && zero("fill_density") =>
        {
            "No effect while Sparse infill density is 0 %"
        }
        "top_fill_pattern" if c.int("top_solid_layers") == Some(0) => {
            "No effect while Top shell layers is 0"
        }
        "bottom_fill_pattern" if c.int("bottom_solid_layers") == Some(0) => {
            "No effect while Bottom shell layers is 0"
        }
        k if k.starts_with("ironing_") && off("ironing") => "No effect while Ironing is off",
        k if k.starts_with("fuzzy_skin_") && c.string("fuzzy_skin") == Some("none") => {
            "No effect while Fuzzy skin is “None”"
        }
        k if k.starts_with("wipe_tower_") && off("wipe_tower") => {
            "No effect while the prime tower is off"
        }
        k if k.starts_with("raft_") && k != "raft_layers" && c.int("raft_layers") == Some(0) => {
            "No effect while Raft layers is 0"
        }
        _ => return None,
    };
    Some(reason.to_string())
}

/// The choices a dropdown offers and the label of the current value: the choices Mica
/// implements (`settings::unimplemented`). A current value Mica does not support keeps its
/// label (the row's hint says it is not supported), so nothing changes until the user picks
/// another.
pub fn enum_choices(
    key: &str,
    raw: &str,
    mut choices: Vec<(String, String)>,
    c: &Config,
) -> (Vec<(String, String)>, String) {
    let label = choices
        .iter()
        .find(|(v, _)| v == raw)
        .map_or_else(|| prettify(raw), |(_, l)| l.clone());
    choices.retain(|(v, _)| settings::unimplemented::choice_supported(key, v, c));
    (choices, label)
}

/// Whether the current value of an enum setting is one Mica does not support.
fn unsupported_choice(key: &str, c: &Config) -> bool {
    option_defs()
        .get(key)
        .is_some_and(|d| d.ty == OptType::Enum)
        && c.string(key)
            .is_some_and(|v| !v.is_empty() && !settings::unimplemented::choice_supported(key, v, c))
}

/// A hint shown after the label when a value has a surprising meaning.
pub fn hint(key: &str, c: &Config) -> Option<&'static str> {
    if unsupported_choice(key, c) {
        return Some(settings::unimplemented::NOTE);
    }
    match key {
        "brim_width"
            if c.float("brim_width") == Some(0.) && c.string("brim_type") != Some("no_brim") =>
        {
            Some("0 mm prints no brim")
        }
        "support_material"
            if c.bool("support_material") == Some(true)
                && c.bool("support_material_auto") == Some(false) =>
        {
            Some("enforcers only")
        }
        _ => None,
    }
}

/// The unit shown next to a value: Orca's, with the mockup's plain words where Orca has none.
pub fn unit_for(key: &str, orca_unit: &str) -> &'static str {
    match key {
        "perimeters" => "loops",
        "top_solid_layers" | "bottom_solid_layers" | TOP_BOTTOM => "layers",
        _ => {
            if orca_unit.chars().count() <= 6 && !orca_unit.starts_with('(') {
                // Leaked to 'static through the settings table: units there are static strs.
                settings::text()
                    .get(key)
                    .map(|t| t.unit.as_str())
                    .filter(|u| *u == orca_unit)
                    .map_or("", |u| {
                        // SAFETY-free: the table lives for the program; copy the &str.
                        let s: &'static str = Box::leak(u.to_string().into_boxed_str());
                        s
                    })
            } else {
                ""
            }
        }
    }
}

/// Draws a page (groups of lines). Returns the edits and how many rows were shown.
pub fn page(ui: &mut egui::Ui, page: &Page, ctx: &FieldContext<'_>) -> (Vec<Edit>, usize) {
    let mut edits = Vec::new();
    let mut shown = 0;
    for group in &page.groups {
        let rows: Vec<(String, &'static str)> = group
            .lines
            .iter()
            .flat_map(|l| {
                l.keys.iter().filter(|k| ctx.visible(k)).map(move |k| {
                    let label = if l.label.is_empty() || l.keys.len() == 1 {
                        settings::label(k)
                    } else {
                        format!("{} · {}", l.label, settings::label(k))
                    };
                    (label, *k)
                })
            })
            .collect();
        if rows.is_empty() {
            continue;
        }
        if !group.name.is_empty() {
            widgets::group_title(ui, &group.name);
        }
        for (label, key) in rows {
            row(ui, key, Some(&label), ctx, &mut edits);
            shown += 1;
        }
    }
    (edits, shown)
}

/// Height of the line under an override at an object or part scope.
pub const SCOPE_LINE_H: f32 = 16.;

/// A value as the line under a scoped override shows it: `15 %`, `5 / 3`, `Off`.
pub fn inherited_text(key: &str, config: &Config, tool: usize) -> String {
    match key {
        TOP_BOTTOM => format!(
            "{} / {}",
            config.int("top_solid_layers").unwrap_or(0),
            config.int("bottom_solid_layers").unwrap_or(0)
        ),
        SUPPORT_MODE => {
            let on = config.bool("support_material").unwrap_or(false);
            let auto = config.bool("support_material_auto").unwrap_or(true);
            let plate = config
                .bool("support_material_buildplate_only")
                .unwrap_or(false);
            match (on, auto, plate) {
                (false, _, _) => "Off",
                (true, false, _) => "Enforcers only",
                (true, true, false) => "Everywhere",
                (true, true, true) => "Build plate only",
            }
            .to_string()
        }
        _ => {
            let raw = tool_raw(config, key, tool)
                .or_else(|| mica_core::merged::orca_raw(config, key))
                .or_else(|| option_defs().get(key).map(|d| d.default.to_string()))
                .unwrap_or_default();
            match option_defs().get(key).map(|d| d.ty) {
                Some(OptType::Bool | OptType::Bools) => {
                    if raw.trim() == "1" { "On" } else { "Off" }.to_string()
                }
                _ => super::sidebar::pretty_value(key, &raw),
            }
        }
    }
}

/// Indent of a dependent row under its parent.
pub const DEPENDENT_INDENT: f32 = 18.;

/// One settings row. `label` overrides the option label.
pub fn row(
    ui: &mut egui::Ui,
    key: &str,
    label: Option<&str>,
    ctx: &FieldContext<'_>,
    edits: &mut Vec<Edit>,
) {
    row_indented(ui, key, label, ctx, edits, 0.);
}

/// A dependent row: the same row, its label indented by `indent` behind a subtle guide line.
pub fn row_indented(
    ui: &mut egui::Ui,
    key: &str,
    label: Option<&str>,
    ctx: &FieldContext<'_>,
    edits: &mut Vec<Edit>,
    indent: f32,
) {
    let p = theme::of(ui);
    // An override at an object or part scope carries a line under its label.
    let scope_line = ctx.inherit.is_some() && ctx.modified(key);
    let height = if scope_line {
        ROW_H + SCOPE_LINE_H
    } else {
        ROW_H
    };
    let (row_rect, row_response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::hover());
    let control = widgets::control_rect(row_rect);
    if indent > 0. {
        // Guide line between the changed-marker bar and the indented label.
        let x = row_rect.left() + LEFT + indent * 0.45;
        ui.painter().line_segment(
            [
                Pos2::new(x, row_rect.top()),
                Pos2::new(x, row_rect.bottom()),
            ],
            egui::Stroke::new(1., p.border_raised),
        );
    }
    let text = settings::text().get(key);
    // "Print order" everywhere, also where callers pass the table's "Print sequence".
    let label = label.filter(|_| key != PRINT_ORDER);
    let name = label.map(str::to_string).unwrap_or_else(|| match key {
        SUPPORT_MODE => "Supports".to_string(),
        TOP_BOTTOM => "Top / bottom layers".to_string(),
        PRINT_ORDER => "Print order".to_string(),
        _ => text.map_or_else(|| key.to_string(), |t| t.label.clone()),
    });
    let modified = ctx.modified(key);
    let preset_mod = !modified && ctx.preset_modified(key);
    let disabled = disabled_reason(key, ctx.config);
    let enabled = disabled.is_none();
    let painter = ui.painter();
    // Changed value: tinted row with an accent bar; user-preset deviation: a muted bar.
    if modified {
        painter.rect_filled(row_rect, 0., p.changed_row);
    }
    if modified || preset_mod {
        painter.rect_filled(
            Rect::from_min_max(
                Pos2::new(row_rect.left(), row_rect.top() + 4.),
                Pos2::new(row_rect.left() + 3., row_rect.bottom() - 4.),
            ),
            2.,
            if modified {
                p.accent
            } else {
                theme::WARNING.gamma_multiply(0.7)
            },
        );
    }
    let label_color = if !enabled || (ctx.inherit.is_some() && !modified) {
        p.muted
    } else if modified {
        p.text
    } else {
        p.label
    };
    let reset_room = if modified { RESET_W } else { 0. };
    let label_right = control.left() - 8. - reset_room;
    let label_rect = Rect::from_min_max(
        Pos2::new(row_rect.left() + LEFT + indent, row_rect.top()),
        Pos2::new(
            label_right,
            row_rect.bottom() - if scope_line { SCOPE_LINE_H } else { 0. },
        ),
    );
    if scope_line && let Some(inherit) = ctx.inherit {
        // `3DBenchy · profile: 15 %` for an object, `was 15 %` for a part.
        let was = inherited_text(key, inherit.config, ctx.tool);
        let line = if inherit.part {
            format!("was {was}")
        } else {
            format!("{} · profile: {was}", inherit.scope)
        };
        let g = ui
            .painter()
            .layout_no_wrap(line, theme::sans(10.5), p.scope_text);
        ui.painter()
            .with_clip_rect(Rect::from_min_max(
                Pos2::new(label_rect.left(), label_rect.bottom() - 6.),
                Pos2::new(control.left() - 8., row_rect.bottom()),
            ))
            .galley(
                Pos2::new(label_rect.left(), label_rect.bottom() - 5.),
                g,
                p.scope_text,
            );
    }
    let job = widgets::highlighted(
        &name,
        ctx.search,
        theme::sans(13.),
        label_color,
        p.mark_bg,
        p.mark_fg,
    );
    let label_response = ui
        .scope_builder(
            egui::UiBuilder::new()
                .max_rect(label_rect)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
            |ui| ui.add(egui::Label::new(job).truncate().selectable(false)),
        )
        .inner;
    let label_end = label_rect.left() + label_response.rect.width().min(label_rect.width());
    if let Some(h) = hint(key, ctx.config) {
        let x = label_end + 6.;
        if x + 40. < label_right {
            ui.painter().text(
                Pos2::new(x, row_rect.center().y),
                Align2::LEFT_CENTER,
                h,
                theme::sans(10.5),
                p.muted,
            );
        }
    }
    label_response.on_hover_ui(|ui| {
        ui.set_max_width(320.);
        ui.label(egui::RichText::new(&name).font(theme::semibold(13.)));
        // Mica's help text (`settings/help_text.tsv`), also for the synthetic Essentials keys
        // and the brim ear extras.
        let help = settings::help(key);
        if !help.is_empty() {
            ui.label(help);
        }
        match key {
            SUPPORT_MODE | TOP_BOTTOM | PRINT_ORDER => {}
            _ => {
                if let Some(t) = text {
                    let mut extra = Vec::new();
                    if !t.unit.is_empty() {
                        extra.push(format!("Unit: {}", t.unit));
                    }
                    match (t.min, t.max) {
                        (Some(a), Some(b)) => extra.push(format!("Range: {a} – {b}")),
                        (Some(a), None) => extra.push(format!("Minimum: {a}")),
                        (None, Some(b)) => extra.push(format!("Maximum: {b}")),
                        _ => {}
                    }
                    if !extra.is_empty() {
                        ui.weak(extra.join(" · "));
                    }
                }
            }
        }
        if let Some(r) = &disabled {
            ui.colored_label(ui.visuals().warn_fg_color, r);
        }
        let real = settings::synthetic_keys(key);
        if real.is_empty() {
            ui.weak(egui::RichText::new(format!("Setting key: {key}")).font(theme::mono(11.5)));
        } else {
            ui.weak(
                egui::RichText::new(format!("Setting keys: {}", real.join(", ")))
                    .font(theme::mono(11.5)),
            );
        }
        if modified && let Some(inherit) = ctx.inherit {
            ui.weak(format!(
                "Changed for {}; ↺ removes the override.",
                inherit.scope
            ));
        } else if modified {
            ui.weak("Changed from the preset; ↺ restores the preset value.");
        } else if preset_mod {
            ui.weak("Your profile changes this value of the system preset.");
        }
    });
    // Inline reset on changed rows (always visible, as in the mockup).
    if modified {
        let reset = Rect::from_center_size(
            Pos2::new(label_end + 4. + 10., row_rect.center().y),
            Vec2::splat(20.),
        );
        let reset = if reset.right() > label_right + reset_room {
            Rect::from_center_size(
                Pos2::new(label_right + reset_room / 2., row_rect.center().y),
                Vec2::splat(20.),
            )
        } else {
            reset
        };
        let r = ui.interact(
            reset,
            ui.make_persistent_id(("reset", ctx.id, key, ctx.tool)),
            Sense::click(),
        );
        if r.hovered() {
            ui.painter().rect_filled(reset, 5., p.hover);
            ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
        }
        widgets::glyph(
            ui.painter(),
            reset.shrink(4.),
            Glyph::Reset,
            if r.hovered() { p.text } else { p.muted_alt },
        );
        let tip = if ctx.inherit.is_some() {
            "Remove the override"
        } else {
            "Back to the preset value"
        };
        if r.on_hover_text(tip).clicked() {
            let real = settings::synthetic_keys(key);
            if real.is_empty() {
                edits.push(Edit::Reset(key.to_string()));
            } else {
                edits.extend(real.iter().map(|k| Edit::Reset(k.to_string())));
            }
        }
    }
    if is_multiline(key) {
        multiline_field(ui, key, ctx, edits, enabled);
    } else {
        control_widget(ui, key, control, ctx, edits, enabled, modified);
        if ctx.inherit.is_some() && !modified {
            // Inherited: the value muted behind a dashed outline; editing it makes an
            // override at this scope.
            let veil = p.panel;
            ui.painter().rect_filled(
                control,
                5.,
                egui::Color32::from_rgba_unmultiplied(veil.r(), veil.g(), veil.b(), 120),
            );
            super::sidebar::dashed_rect(
                ui.painter(),
                control,
                5.,
                egui::Stroke::new(1., p.border_raised),
            );
        }
    }
    if let Some(r) = disabled {
        ui.interact(
            control,
            ui.make_persistent_id(("why", ctx.id, key, ctx.tool)),
            Sense::hover(),
        )
        .on_hover_text(r);
    }
    let _ = row_response;
}

/// Options PrusaSlicer and Orca mark `multiline` (G-code templates, notes, post-processing):
/// edited in a full-width code box under their label instead of the one-line control.
const MULTILINE: &[&str] = &[
    "adaptive_pressure_advance_model",
    "before_layer_change_gcode",
    "before_layer_gcode",
    "between_objects_gcode",
    "change_extrusion_role_gcode",
    "change_filament_gcode",
    "color_change_gcode",
    "custom_parameters_filament",
    "custom_parameters_print",
    "custom_parameters_printer",
    "end_filament_gcode",
    "end_gcode",
    "filament_change_extrusion_role_gcode",
    "filament_end_gcode",
    "filament_notes",
    "filament_start_gcode",
    "file_start_gcode",
    "layer_change_gcode",
    "layer_gcode",
    "machine_end_gcode",
    "machine_pause_gcode",
    "machine_start_gcode",
    "material_notes",
    "notes",
    "pause_print_gcode",
    "post_process",
    "printer_notes",
    "printing_by_object_gcode",
    "process_change_extrusion_role_gcode",
    "small_area_infill_flow_compensation_model",
    "start_filament_gcode",
    "start_gcode",
    "template_custom_gcode",
    "time_lapse_gcode",
    "toolchange_gcode",
    "wrapping_detection_gcode",
];

pub fn is_multiline(key: &str) -> bool {
    MULTILINE.contains(&key)
}

/// The full-width code box of a multiline option. The stored value is C-escaped (`\n`); the
/// box shows real line breaks and commits when it loses focus, so typing does not re-slice
/// or push an undo step per keystroke.
fn multiline_field(
    ui: &mut egui::Ui,
    key: &str,
    ctx: &FieldContext<'_>,
    edits: &mut Vec<Edit>,
    enabled: bool,
) {
    let def = option_defs().get(key);
    let vector = def.is_some_and(|d| d.ty == OptType::Strings);
    // A print-scope string list (flow compensation model points, post-processing scripts,
    // G-code substitutions) is one list, not one value per tool: show every entry, one per
    // line (Orca stores later points with a leading newline), and save the whole list.
    let list = vector && def.is_some_and(|d| d.scope == Scope::Print);
    let raw = raw_of(key, ctx);
    let shown = if list {
        list_entries(ctx.config, key).join("\n")
    } else if vector {
        unescape_strings(&raw)
            .ok()
            .and_then(|v| v.into_iter().next())
            .unwrap_or_default()
    } else {
        unescape_string(&raw).unwrap_or_else(|_| raw.clone())
    };
    let id = ui.make_persistent_id(("mica-multiline", ctx.id, key, ctx.tool));
    let mut buf = ui
        .data_mut(|d| d.get_temp::<String>(id))
        .unwrap_or_else(|| shown.clone());
    let rows = shown.lines().count().clamp(3, 18);
    let response = ui
        .horizontal(|ui| {
            ui.add_space(LEFT);
            ui.add_enabled(
                enabled,
                egui::TextEdit::multiline(&mut buf)
                    .code_editor()
                    .font(theme::mono(12.))
                    .desired_rows(rows)
                    .desired_width(ui.available_width() - MARGIN)
                    .lock_focus(true),
            )
        })
        .inner;
    if response.has_focus() {
        ui.data_mut(|d| d.insert_temp(id, buf));
    } else {
        ui.data_mut(|d| d.remove::<String>(id));
        if response.lost_focus() && buf != shown {
            let raw = if list {
                let entries: Vec<String> = buf
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .map(str::to_string)
                    .collect();
                escape_strings(&entries)
            } else if vector {
                escape_strings(&[buf])
            } else {
                escape_string(&buf)
            };
            edits.push(Edit::Set {
                key: key.into(),
                raw,
            });
        }
    }
    ui.add_space(6.);
}

/// Every entry of a string-list option, trimmed of Orca's leading newlines.
fn list_entries(config: &Config, key: &str) -> Vec<String> {
    match config.get(key) {
        Some(mica_core::config::ConfigValue::Vector(items)) => items
            .iter()
            .filter_map(|i| match i {
                Some(Value::String(s)) => Some(s.trim().to_string()),
                _ => None,
            })
            .filter(|s| !s.is_empty())
            .collect(),
        Some(mica_core::config::ConfigValue::Scalar(Value::String(s))) => s
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect(),
        _ => option_defs()
            .get(key)
            .and_then(|d| unescape_strings(d.default).ok())
            .unwrap_or_default(),
    }
}

/// `auto_brim` → `Auto brim`: a readable label for an enum value the text table does not
/// know (Orca-only vocabulary).
pub fn prettify(raw: &str) -> String {
    let mut s = raw.replace('_', " ");
    if let Some(first) = s.get(..1) {
        let upper = first.to_uppercase();
        s.replace_range(..1, &upper);
    }
    s
}

/// Current text of `key` for the edited tool.
fn raw_of(key: &str, ctx: &FieldContext<'_>) -> String {
    match ctx.element(key) {
        Some(tool) => tool_raw(ctx.config, key, tool),
        None => ctx.config.raw(key),
    }
    .unwrap_or_else(|| {
        // A merged setting a PrusaSlicer profile does not spell out shows its PrusaSlicer
        // counterpart's value, translated (`mica_core::merged`).
        mica_core::merged::orca_raw(ctx.config, key)
            .unwrap_or_else(|| option_defs().get(key).map_or("", |d| d.default).into())
    })
}

fn set(edits: &mut Vec<Edit>, key: &str, value: Value) {
    edits.push(Edit::Set {
        key: key.to_string(),
        raw: serialize_value(&mica_core::config::ConfigValue::Scalar(value)),
    });
}

fn control_widget(
    ui: &mut egui::Ui,
    key: &str,
    rect: Rect,
    ctx: &FieldContext<'_>,
    edits: &mut Vec<Edit>,
    enabled: bool,
    accent: bool,
) {
    let id = ui.make_persistent_id(("mica-field", ctx.id, key, ctx.tool));
    if key == SUPPORT_MODE {
        support_mode(ui, id, rect, ctx, edits, accent);
        return;
    }
    if key == PRINT_ORDER {
        let raw = if ctx.config.bool(key).unwrap_or(false) {
            "1"
        } else {
            "0"
        };
        let choices = [
            ("0".to_string(), "Layer by layer".to_string()),
            ("1".to_string(), "Object by object".to_string()),
        ];
        let label = &choices[usize::from(raw == "1")].1;
        if let Some(v) =
            widgets::dropdown_with(ui, id, rect, raw, label, None, &choices, enabled, accent)
            && v != raw
        {
            set(edits, key, Value::Bool(v == "1"));
        }
        return;
    }
    if key == TOP_BOTTOM {
        let values = [
            ctx.config.int("top_solid_layers").unwrap_or(0),
            ctx.config.int("bottom_solid_layers").unwrap_or(0),
        ];
        if let Some(next) =
            widgets::pair_number(ui, id, rect, values, "layers", (0, 1000), enabled, accent)
        {
            if next[0] != values[0] {
                set(edits, "top_solid_layers", Value::Int(next[0]));
            }
            if next[1] != values[1] {
                set(edits, "bottom_solid_layers", Value::Int(next[1]));
            }
        }
        return;
    }
    // Orca's automatic-ear options are engine extras without an option definition.
    if let Some(&(_, _, unit, min, max, _, _)) = crate::brim_ears::ear_option(key) {
        let v = crate::brim_ears::option_value(key, ctx.config);
        let spec = widgets::NumberSpec {
            unit,
            decimals: 1,
            min,
            max,
            speed: if unit == "°" { 0.5 } else { 0.05 },
        };
        if let Some(n) = widgets::number_with(ui, id, rect, v, &spec, enabled, accent) {
            edits.push(Edit::Set {
                key: key.into(),
                raw: widgets::format_number(n, 1),
            });
        }
        return;
    }
    if key == crate::textures::TEXTURE_KEY {
        // The texture row opens the gallery; a pick also sets its suggested tile size.
        if let Some((value, tile)) =
            crate::textures::picker(ui, id, rect, &raw_of(key, ctx), enabled)
        {
            edits.push(Edit::Set {
                key: key.into(),
                raw: value,
            });
            edits.push(Edit::Set {
                key: "mica_texture_skin_scale".into(),
                raw: widgets::format_number(tile, 1),
            });
        }
        return;
    }
    if super::colour::is_colour_key(key) {
        let title = settings::label(key);
        if let Some(v) =
            super::colour::option_control(ui, id, rect, &title, &raw_of(key, ctx), enabled)
        {
            edits.push(Edit::Set {
                key: key.into(),
                raw: v,
            });
        }
        return;
    }
    let Some(def) = option_defs().get(key) else {
        if let Some(v) = widgets::text_field(ui, id, rect, &raw_of(key, ctx), enabled) {
            edits.push(Edit::Set {
                key: key.into(),
                raw: v,
            });
        }
        return;
    };
    let text = settings::text().get(key);
    let orca_unit = text.map(|t| t.unit.as_str()).unwrap_or("");
    let unit = unit_for(key, orca_unit);
    let (min, max) = text.map(|t| (t.min, t.max)).unwrap_or((None, None));
    let element = match def.ty {
        OptType::Floats => OptType::Float,
        OptType::Ints => OptType::Int,
        OptType::Bools => OptType::Bool,
        OptType::Percents => OptType::Percent,
        OptType::FloatsOrPercents => OptType::FloatOrPercent,
        OptType::Strings => OptType::String,
        OptType::Enums => OptType::Enum,
        OptType::Points => OptType::Point,
        t => t,
    };
    match element {
        OptType::Bool => {
            let on = ctx
                .config
                .bool_at(key, ctx.tool_of(key))
                .or_else(|| match mica_core::merged::orca_value(ctx.config, key) {
                    Some(Value::Bool(b)) => Some(b),
                    _ => None,
                })
                .unwrap_or(def.default == "1");
            if let Some(v) = widgets::toggle(ui, id, rect, on, enabled) {
                set(edits, key, Value::Bool(v));
            }
        }
        OptType::Int => {
            let v = ctx
                .config
                .int_at(key, ctx.tool_of(key))
                .or_else(|| match mica_core::merged::orca_value(ctx.config, key) {
                    Some(Value::Int(i)) => Some(i),
                    _ => None,
                })
                .unwrap_or_else(|| def.default.parse().unwrap_or(0)) as f64;
            let spec = widgets::NumberSpec {
                unit,
                decimals: 0,
                min: min.unwrap_or(i64::MIN as f64),
                max: max.unwrap_or(i64::MAX as f64),
                speed: 0.1,
            };
            if let Some(n) = widgets::number_with(ui, id, rect, v, &spec, enabled, accent) {
                set(edits, key, Value::Int(n.round() as i64));
            }
        }
        OptType::Float | OptType::Percent | OptType::FloatOrPercent => {
            let raw = raw_of(key, ctx);
            let percent = element == OptType::Percent
                || (element == OptType::FloatOrPercent && raw.trim().ends_with('%'));
            let v = ctx
                .config
                .float_at(key, ctx.tool_of(key))
                .or_else(|| raw.trim().trim_end_matches('%').parse().ok())
                .unwrap_or_else(|| def.default.trim_end_matches('%').parse().unwrap_or(0.));
            // "mm or %" (line widths, overhang speeds): an absolute value shows the first unit
            // inside the field, as Orca does.
            let unit = if percent {
                "%"
            } else if unit.is_empty() && element == OptType::FloatOrPercent {
                orca_unit.strip_suffix(" or %").unwrap_or("")
            } else {
                unit
            };
            let decimals = widgets::decimals_for(unit, v);
            let spec = widgets::NumberSpec {
                unit,
                decimals,
                min: min.unwrap_or(f64::MIN),
                max: max.unwrap_or(f64::MAX),
                speed: if v.abs() < 1. {
                    0.005
                } else if v.abs() < 20. {
                    0.05
                } else {
                    0.5
                },
            };
            if let Some(n) = widgets::number_with(ui, id, rect, v, &spec, enabled, accent) {
                set(
                    edits,
                    key,
                    match element {
                        OptType::Percent => Value::Percent(n),
                        OptType::FloatOrPercent => Value::FloatOrPercent { value: n, percent },
                        _ => Value::Float(n),
                    },
                );
            }
        }
        OptType::Enum => {
            let raw = raw_of(key, ctx);
            let mut choices = text.map(|t| t.enums.clone()).unwrap_or_default();
            // An Orca printer (the U1) also offers the ear brims (see `crate::brim_ears`).
            if key == "brim_type" && crate::brim_ears::orca(ctx.config) {
                choices.extend(
                    crate::brim_ears::ORCA_BRIM_TYPES
                        .iter()
                        .map(|(v, l)| (v.to_string(), l.to_string())),
                );
            }
            if choices.is_empty() {
                if let Some(v) = widgets::text_field(ui, id, rect, &raw, enabled) {
                    edits.push(Edit::Set {
                        key: key.into(),
                        raw: v,
                    });
                }
                return;
            }
            let (choices, label) = enum_choices(key, &raw, choices, ctx.config);
            let icon_key = super::icons::has_icons(key).then_some(key);
            if let Some(v) = widgets::dropdown_with(
                ui, id, rect, &raw, &label, icon_key, &choices, enabled, accent,
            ) && v != raw
            {
                set(edits, key, Value::Enum(v));
            }
        }
        _ => {
            if let Some(v) = widgets::text_field(ui, id, rect, &raw_of(key, ctx), enabled) {
                edits.push(Edit::Set {
                    key: key.into(),
                    raw: v,
                });
            }
        }
    }
}

/// The combined supports mode (see `SUPPORT_MODE`).
fn support_mode(
    ui: &mut egui::Ui,
    id: egui::Id,
    rect: Rect,
    ctx: &FieldContext<'_>,
    edits: &mut Vec<Edit>,
    accent: bool,
) {
    let c = ctx.config;
    let on = c.bool("support_material").unwrap_or(false);
    let auto = c.bool("support_material_auto").unwrap_or(true);
    let plate_only = c.bool("support_material_buildplate_only").unwrap_or(false);
    let current = match (on, auto, plate_only) {
        (false, _, _) => "off",
        (true, false, _) => "enforcers",
        (true, true, false) => "everywhere",
        (true, true, true) => "plate",
    };
    let items = [
        ("off", "Off"),
        ("enforcers", "Enforcers only"),
        ("everywhere", "Everywhere"),
        ("plate", "Build plate only"),
    ];
    let label = items
        .iter()
        .find(|(v, _)| *v == current)
        .map_or("", |(_, l)| l);
    let choices: Vec<(String, String)> = items
        .iter()
        .map(|(v, l)| (v.to_string(), l.to_string()))
        .collect();
    if let Some(v) =
        widgets::dropdown_with(ui, id, rect, current, label, None, &choices, true, accent)
        && v != current
    {
        edits.extend(support_mode_edits(&v, ctx.overlay));
    }
}

/// The edits choosing supports mode `mode` (`off`, `enforcers`, `everywhere`, `plate`) makes,
/// with `overlay` the edits of the target.
pub(crate) fn support_mode_edits(mode: &str, overlay: &Overlay) -> Vec<Edit> {
    let (on, auto, plate) = match mode {
        "off" => (false, None, None),
        "enforcers" => (true, Some(false), None),
        "everywhere" => (true, Some(true), Some(false)),
        _ => (true, Some(true), Some(true)),
    };
    let mut edits = Vec::new();
    set(&mut edits, "support_material", Value::Bool(on));
    // What a mode does not decide (both for Off, build plate only for Enforcers, which the
    // choice does not show) is left as it is without an edit: what another mode changed in
    // it goes, so switching back leaves nothing changed.
    for (key, value) in [
        ("support_material_auto", auto),
        ("support_material_buildplate_only", plate),
    ] {
        match value {
            Some(b) => set(&mut edits, key, Value::Bool(b)),
            None if overlay.contains_key(key) => edits.push(Edit::Reset(key.into())),
            None => {}
        }
    }
    edits
}

/// A multi-line text (custom G-code, notes) with the label above.
pub fn long_text(ui: &mut egui::Ui, key: &str, ctx: &FieldContext<'_>, edits: &mut Vec<Edit>) {
    let id = ui.make_persistent_id(("mica-long", ctx.id, key, ctx.tool));
    let raw = ctx
        .config
        .string_at(key, ctx.tool_of(key))
        .unwrap_or("")
        .to_string();
    let focused = ui.memory(|m| m.has_focus(id));
    let mut buf: String = if focused {
        ui.data_mut(|d| d.get_temp::<String>(id))
            .unwrap_or_else(|| raw.clone())
    } else {
        raw.clone()
    };
    ui.add_space(4.);
    ui.horizontal(|ui| {
        ui.add_space(LEFT);
        ui.label(egui::RichText::new(settings::label(key)).size(13.));
        if ctx.modified(key) {
            ui.label(
                egui::RichText::new("changed")
                    .size(10.5)
                    .color(theme::ACCENT),
            );
        }
    });
    let r = ui.add(
        egui::TextEdit::multiline(&mut buf)
            .id(id)
            .code_editor()
            .desired_rows(5)
            .desired_width(ui.available_width() - MARGIN),
    );
    if r.lost_focus() && buf != raw {
        let def = option_defs().get(key);
        let raw = if def.is_some_and(|d| d.ty == OptType::Strings) {
            mica_core::config::escape_strings(&[buf.clone()])
        } else {
            mica_core::config::escape_string(&buf)
        };
        edits.push(Edit::Set {
            key: key.to_string(),
            raw,
        });
    }
    ui.data_mut(|d| d.insert_temp(id, buf));
    let _ = CONTROL_W;
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_print_scope_string_list_shows_every_entry() {
        // The flow compensation model is a list of points, not one value per tool: all ten
        // default points must show (the editor once showed only the first, "0,0").
        let c = Config::defaults();
        let points = super::list_entries(&c, "small_area_infill_flow_compensation_model");
        assert_eq!(points.len(), 10, "{points:?}");
        assert_eq!(points[0], "0,0");
        assert_eq!(points[9], "10,1");
        // Saving the edited text writes the whole list back.
        let raw = escape_strings(&points);
        let mut c2 = Config::defaults();
        c2.set_raw("small_area_infill_flow_compensation_model", &raw)
            .unwrap();
        assert_eq!(
            super::list_entries(&c2, "small_area_infill_flow_compensation_model"),
            points
        );
    }

    use super::*;
    #[test]
    fn visibility_follows_mode_search_and_overlay() {
        let config = Config::defaults();
        let empty = Overlay::new();
        let simple = FieldContext {
            config: &config,
            overlay: &empty,
            preset_overrides: None,
            tool: 0,
            advanced: false,
            search: "",
            id: "t",
            force: false,
            inherit: None,
        };
        assert!(simple.visible("layer_height"));
        // Scarf seam settings are advanced options in Orca.
        assert!(!simple.visible("seam_slope_min_length"));
        let mut overlay = Overlay::new();
        overlay.insert("seam_slope_min_length".into(), "15".into());
        // The GUI writes the PrusaSlicer counterpart with it; that row never shows.
        overlay.insert("scarf_seam_length".into(), "15".into());
        let with_edit = FieldContext {
            overlay: &overlay,
            ..simple
        };
        assert!(with_edit.visible("seam_slope_min_length"));
        assert!(with_edit.modified("seam_slope_min_length"));
        assert!(!with_edit.visible("scarf_seam_length"));
        // A merged row is found by its PrusaSlicer name too.
        let prusa_name = FieldContext {
            search: "scarf joint length",
            advanced: true,
            ..with_edit
        };
        assert!(prusa_name.visible("seam_slope_min_length"));
        let searching = FieldContext {
            search: "wall",
            advanced: true,
            ..with_edit
        };
        assert!(searching.visible("perimeters"));
        assert!(!searching.visible("layer_height"));
        assert!(is_advanced("scarf_seam_length") && !is_advanced("layer_height"));
    }

    /// Settings Mica does not implement show on no page and in no search, on Marlin and on
    /// Klipper printers, unless they carry an unsaved change (to be reset).
    #[test]
    fn unimplemented_settings_are_hidden_from_pages_and_search() {
        use settings::unimplemented::{Hidden, SETTINGS};
        let mut config = Config::defaults();
        config.set_raw("gcode_flavor", "klipper").unwrap();
        let empty = Overlay::new();
        let all = FieldContext {
            config: &config,
            overlay: &empty,
            preset_overrides: None,
            tool: 0,
            advanced: true,
            search: "",
            id: "t",
            force: false,
            inherit: None,
        };
        let on_pages: Vec<&str> = [
            settings::Tab::Print,
            settings::Tab::Filament,
            settings::Tab::Printer,
        ]
        .into_iter()
        .flat_map(settings::pages)
        .flat_map(|p| &p.groups)
        .flat_map(|g| &g.lines)
        .flat_map(|l| l.keys.iter().copied())
        .collect();
        for s in SETTINGS {
            assert!(on_pages.contains(&s.key), "{} is not on a page", s.key);
            assert!(!all.visible(s.key), "{}", s.key);
            let label = settings::label(s.key);
            let search = FieldContext {
                search: &label,
                ..all
            };
            assert!(!search.visible(s.key), "{}", s.key);
        }
        // The Klipper-only gaps show on a Marlin printer.
        let mut marlin = config.clone();
        marlin.set_raw("gcode_flavor", "marlin2").unwrap();
        let marlin_ctx = FieldContext {
            config: &marlin,
            ..all
        };
        for s in SETTINGS {
            assert_eq!(
                marlin_ctx.visible(s.key),
                s.hidden == Hidden::Klipper,
                "{}",
                s.key
            );
        }
        let mut overlay = Overlay::new();
        overlay.insert("interface_shells".into(), "1".into());
        let changed = FieldContext {
            overlay: &overlay,
            ..all
        };
        assert!(changed.visible("interface_shells"));
    }

    /// An unsupported choice is not offered, but a selected one still shows with its label and
    /// the "not supported by Mica" note, so the value never changes behind the user's back.
    #[test]
    fn unsupported_choices_are_not_offered_but_still_display() {
        let mut c = Config::defaults();
        c.set_raw("gcode_flavor", "marlin2").unwrap();
        let full = settings::text()["slicing_mode"].enums.clone();
        assert_eq!(full.len(), 3);
        let (choices, label) = enum_choices("slicing_mode", "regular", full.clone(), &c);
        assert_eq!(
            choices,
            vec![("regular".to_string(), "Regular".to_string())]
        );
        assert_eq!(label, "Regular");
        assert_eq!(hint("slicing_mode", &c), None);
        c.set_raw("slicing_mode", "even_odd").unwrap();
        let (choices, label) = enum_choices("slicing_mode", "even_odd", full, &c);
        assert_eq!(label, "Even-odd");
        assert!(!choices.iter().any(|(v, _)| v == "even_odd"));
        assert_eq!(hint("slicing_mode", &c), Some("not supported by Mica"));
        assert_eq!(c.string("slicing_mode"), Some("even_odd"));
        // Rule-dependent: machine limits are emitted on Marlin, not on Klipper.
        let limits = settings::text()["machine_limits_usage"].enums.clone();
        c.set_raw("machine_limits_usage", "emit_to_gcode").unwrap();
        assert_eq!(
            enum_choices("machine_limits_usage", "emit_to_gcode", limits.clone(), &c)
                .0
                .len(),
            3
        );
        assert_eq!(hint("machine_limits_usage", &c), None);
        c.set_raw("gcode_flavor", "klipper").unwrap();
        let (choices, label) = enum_choices("machine_limits_usage", "emit_to_gcode", limits, &c);
        assert_eq!((choices.len(), label.as_str()), (2, "Write to G-code"));
        assert_eq!(
            hint("machine_limits_usage", &c),
            Some("not supported by Mica")
        );
        // A flavor the writer refuses.
        c.set_raw("gcode_flavor", "reprapfirmware").unwrap();
        assert_eq!(hint("gcode_flavor", &c), Some("not supported by Mica"));
    }

    #[test]
    fn synthetic_keys_follow_their_real_keys() {
        let config = Config::defaults();
        let mut overlay = Overlay::new();
        overlay.insert("bottom_solid_layers".into(), "4".into());
        let ctx = FieldContext {
            config: &config,
            overlay: &overlay,
            preset_overrides: None,
            tool: 0,
            advanced: false,
            search: "",
            id: "t",
            force: true,
            inherit: None,
        };
        assert!(ctx.modified(TOP_BOTTOM));
        assert!(!ctx.modified(SUPPORT_MODE));
        assert!(matches(TOP_BOTTOM, "bottom"));
        assert!(matches(SUPPORT_MODE, "support"));
        assert!(!matches(TOP_BOTTOM, "seam"));
        assert_eq!(prettify("auto_brim"), "Auto brim");
        assert_eq!(prettify("crosshatch"), "Crosshatch");
        assert_eq!(unit_for("perimeters", "(minimum)"), "loops");
        assert_eq!(unit_for(TOP_BOTTOM, ""), "layers");
        assert_eq!(unit_for("layer_height", "mm"), "mm");
        assert_eq!(unit_for("fill_density", "a very long unit"), "");
    }

    /// Spiral vase forces one wall, no top layers and no infill: those rows are greyed out
    /// with the reason while it is on.
    #[test]
    fn spiral_vase_locks_what_it_forces() {
        let mut c = Config::defaults();
        for key in [
            "perimeters",
            "top_solid_layers",
            "fill_density",
            "fill_pattern",
        ] {
            assert!(disabled_reason(key, &c).is_none(), "{key}");
        }
        c.set_raw("spiral_vase", "1").unwrap();
        for key in [
            "perimeters",
            "top_solid_layers",
            "fill_density",
            "fill_pattern",
            "retract_layer_change",
        ] {
            assert!(
                disabled_reason(key, &c).is_some_and(|r| r.contains("Spiral vase")),
                "{key}"
            );
        }
        assert!(disabled_reason("bottom_solid_layers", &c).is_none());
    }

    #[test]
    fn dependents_and_hints() {
        let mut c = Config::defaults();
        c.set_raw("brim_type", "no_brim").unwrap();
        c.set_raw("support_material", "0").unwrap();
        assert!(disabled_reason("brim_width", &c).is_some());
        assert!(disabled_reason("support_material_style", &c).is_some());
        assert!(disabled_reason("support_material", &c).is_none());
        assert!(disabled_reason("layer_height", &c).is_none());
        c.set_raw("brim_type", "outer_only").unwrap();
        c.set_raw("brim_width", "0").unwrap();
        assert_eq!(hint("brim_width", &c), Some("0 mm prints no brim"));
        c.set_raw("support_material", "1").unwrap();
        c.set_raw("support_material_auto", "0").unwrap();
        assert_eq!(hint("support_material", &c), Some("enforcers only"));
        assert!(disabled_reason("support_material_threshold", &c).is_some());
    }
}
