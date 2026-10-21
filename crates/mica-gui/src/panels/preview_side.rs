// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The Preview sidebar: the same panel as Prepare's (side, width, resizing and Shift+Tab
//! collapse are shared) with the same topic rail (`topics.rs`, icon-only when narrow). In
//! Preview the rail has one more item on top, **Preview**, whose page replaces the old
//! floating legend card:
//!
//! 1. **Color by**: the scheme combo and its legend (`preview::legend::body`): feature or tool
//!    rows with time, share and filament (click to hide), or the gradient scale; a scheme with
//!    one value over the whole print shows that value (`220 °C (whole print)`).
//! 2. **Totals**: print time, filament weight and length (per tool on a multi-tool print) and
//!    the cost when the filament has a price.
//! 3. **Layer**: the top layer the layer slider shows: its number, Z, height and time, and
//!    on a multi-tool print what each tool extrudes on it.
//!
//! The other rail items are the settings topics, shown right there in the sidebar. An edit
//! makes the slice out of date exactly as in Prepare; the window stays in Preview (`hold`):
//! the Preview page then says the slice is out of date and offers to slice again (automatic
//! slicing does it by itself), and the new toolpaths keep the color scheme and layer range.
//!
//! Entering Preview always opens the Preview page (that is what one enters Preview for);
//! Prepare keeps its own last topic in the preferences, so switching back and forth never
//! loses either. With the sidebar collapsed, a small chip in the 3D view's top corner (on the
//! sidebar's side) names the color scheme and brings the sidebar back.

use super::widgets::{self, MARGIN};
use crate::app::App;
use crate::preview::{
    Preview, View,
    legend::{self, Totals},
    stats::{self, format_seconds, format_time},
};
use crate::theme;
use eframe::egui::{self, Align2, Color32, Pos2, Rect, Sense, Stroke, Vec2};
use std::sync::Arc;

/// Height of a value row (the settings column's control height).
const ROW: f32 = widgets::CONTROL_H;

/// Extruded volume per tool of one layer of one preview: (job, layer, (tool, mm³)).
type LayerTools = (usize, usize, Vec<(usize, f64)>);

/// The Preview sidebar's own state.
#[derive(Default)]
pub struct PreviewSide {
    /// The settings topic open in Preview; `None`: the Preview page.
    pub topic: Option<String>,
    /// The topic rail is being drawn for the Preview sidebar (adds the Preview item).
    pub in_rail: bool,
    /// A setting was changed in Preview: the window stays in Preview while the plate has no
    /// up-to-date slice (until Prepare is picked).
    pub hold: bool,
    /// The sidebar showed Preview last frame (entering Preview opens the Preview page).
    was_previewing: bool,
    /// Extruded volume per tool of one layer of one preview: (job, layer, (tool, mm³)).
    layer_tools: Option<LayerTools>,
    /// What the last frame drew, for tests.
    pub probe: Probe,
}

impl PreviewSide {
    /// Opens a settings topic in the Preview sidebar now (as a rail click would), so that
    /// entering Preview does not switch back to the Preview page (screenshots, tests).
    pub fn open_topic(&mut self, topic: &str) {
        self.topic = (topic != super::topics::PREVIEW_TOPIC).then(|| topic.to_string());
        self.was_previewing = true;
    }
}

/// What the Preview sidebar and its chip drew in the last frame.
#[derive(Clone, Debug, Default)]
#[cfg_attr(not(test), allow(dead_code))] // Read by the tests.
pub struct Probe {
    /// The sidebar was drawn (its panel rect).
    pub sidebar: Option<Rect>,
    /// The Preview page was drawn.
    pub page: bool,
    /// The page said the slice is out of date.
    pub outdated: bool,
    /// The scheme the Color by combo shows.
    pub color_by: String,
    /// The single value line of a degenerate scale (`220 °C (whole print)`).
    pub single: Option<String>,
    /// The Totals rows and the Layer rows as (label, value).
    pub totals: Vec<(String, String)>,
    pub layer: Vec<(String, String)>,
    /// The collapsed sidebar's chip in the 3D view.
    pub chip: Option<Rect>,
}

/// Extruded volume per tool of the records `start..start + count`, tools in order.
fn tool_volumes(preview: &Preview, start: u64, count: u64) -> Vec<(usize, f64)> {
    let mut v = [0f64; 16];
    let ok = preview.read_batches(start..start + count, |batch| {
        for s in batch {
            if s.role() != crate::preview::TRAVEL {
                v[s.tool()] += stats::extruded_volume(s);
            }
        }
    });
    if ok.is_err() {
        return Vec::new();
    }
    v.iter()
        .enumerate()
        .filter(|(_, x)| **x > 0.)
        .map(|(t, x)| (t, *x))
        .collect()
}

/// Filament a tool uses on a layer: the export's total for the tool, split by extruded
/// volume (as the feature rows split it), in grams and millimetres.
pub fn layer_usage(
    preview: &Preview,
    tool: usize,
    volume: f64,
    summary: &crate::worker::Summary,
) -> Option<(f64, f64)> {
    let total = preview.stats.tools.get(tool)?.volume_mm3;
    let usage = summary.tools.iter().find(|t| t.tool == tool)?;
    (total > 0.).then(|| {
        let share = volume / total;
        (usage.grams * share, usage.filament_mm * share)
    })
}

/// `12.3 g · 4.12 m` (or the length alone when the filament has no density).
pub fn amount(grams: f64, mm: f64) -> String {
    if grams > 0. {
        format!("{grams:.1} g · {:.2} m", mm / 1000.)
    } else {
        format!("{:.2} m", mm / 1000.)
    }
}

/// A layer's small amount: `0.04 g · 14 mm` (two decimals of grams, whole millimetres below a
/// metre).
pub fn layer_amount(grams: f64, mm: f64) -> String {
    let length = if mm < 1000. {
        format!("{mm:.0} mm")
    } else {
        format!("{:.2} m", mm / 1000.)
    };
    if grams > 0. {
        format!("{grams:.2} g · {length}")
    } else {
        length
    }
}

/// A row of the Totals and Layer sections: optional swatch, label on the left, value in the
/// right-hand column (monospace), both on one line; the label is cut with an ellipsis rather
/// than wrapped when the sidebar is narrow.
fn value_row(
    ui: &mut egui::Ui,
    swatch: Option<Color32>,
    label: &str,
    value: &str,
    hover: Option<&str>,
) {
    let p = theme::of(ui);
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW), Sense::hover());
    let painter = ui.painter();
    let mut left = rect.left();
    if let Some(c) = swatch {
        let sw =
            Rect::from_center_size(Pos2::new(left + 4. + 6., rect.center().y), Vec2::splat(12.));
        painter.rect_filled(sw, 3., c);
        left = sw.right() + 8.;
    }
    let value_g = painter.layout_no_wrap(value.to_string(), theme::mono(12.), p.text);
    let value_w = value_g.size().x;
    let right = rect.right() - 4.;
    painter.galley(
        Pos2::new(right - value_w, rect.center().y - value_g.size().y / 2.),
        value_g,
        p.text,
    );
    let label_g = legend::truncated(
        painter,
        label.to_string(),
        theme::sans(13.),
        p.label,
        right - value_w - 12. - left,
    );
    painter.galley(
        Pos2::new(left, rect.center().y - label_g.size().y / 2.),
        label_g,
        p.label,
    );
    if let Some(h) = hover {
        response.on_hover_text(h);
    }
}

/// A muted note line.
pub(super) fn note(ui: &mut egui::Ui, text: &str) {
    let p = theme::of(ui);
    ui.add(
        egui::Label::new(
            egui::RichText::new(text)
                .font(theme::sans(11.))
                .color(p.muted),
        )
        .wrap(),
    );
}

/// A section of the Preview page: the settings pages' section header (icon, bold title,
/// rule), then the body with the rows' margins.
pub(super) fn section<R>(
    ui: &mut egui::Ui,
    title: &str,
    body: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    let (rect, _) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), super::topics::SECTION_H),
        Sense::hover(),
    );
    super::topics::paint_section_header(ui, rect, title, None, 0, false);
    ui.add_space(4.);
    let r = egui::Frame::new()
        .inner_margin(egui::Margin {
            left: MARGIN as i8,
            right: MARGIN as i8,
            top: 0,
            bottom: 0,
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            body(ui)
        });
    ui.add_space(12.);
    r.inner
}

impl App {
    /// Whether the window shows Preview: a slice to show, or a setting changed in Preview
    /// and its new slice not there yet.
    pub(crate) fn previewing(&self) -> bool {
        self.preview && (self.finished().is_some() || self.preview_side.hold)
    }

    /// Once a frame before the panels: entering Preview opens the Preview page; leaving it
    /// ends the hold.
    pub(crate) fn sync_preview_side(&mut self) {
        if !self.preview {
            self.preview_side.hold = false;
        }
        let now = self.previewing();
        if now && !self.preview_side.was_previewing {
            self.preview_side.topic = None;
        }
        self.preview_side.was_previewing = now;
        // Set again when the Preview sidebar draws this frame.
        self.preview_side.probe.sidebar = None;
    }

    /// The Preview's sidebar: the topic rail with the Preview page on top.
    pub(crate) fn preview_sidebar(&mut self, ui: &mut egui::Ui) {
        self.preview_side.probe = Probe {
            sidebar: Some(ui.max_rect()),
            ..Default::default()
        };
        let busy = self.loading.is_some();
        ui.spacing_mut().item_spacing.y = 0.;
        // An opened G-code file: its card and the Preview page; no settings (they would edit
        // the hidden plate's profile, not the file).
        if self.gcode.is_some() {
            egui::ScrollArea::vertical()
                .id_salt("gcode-sidebar-scroll")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(8., 0.);
                    self.gcode_card(ui);
                    self.preview_page(ui);
                    ui.add_space(12.);
                });
            return;
        }
        egui::ScrollArea::vertical()
            .id_salt("preview-sidebar-scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing = Vec2::new(8., 0.);
                // Preview edits the profile: no object, part or tower scope here.
                let modes = (self.editor.object_mode, self.editor.tower_mode);
                self.editor.object_mode = false;
                self.editor.tower_mode = false;
                self.preview_side.in_rail = true;
                let generation = self.config_generation;
                let shown = self.finished().map(|f| Arc::as_ptr(&f.preview));
                let view = self.view.clone();
                ui.add_enabled_ui(!busy, |ui| self.settings_list(ui));
                self.preview_side.in_rail = false;
                (self.editor.object_mode, self.editor.tower_mode) = modes;
                let now = self.finished().map(|f| Arc::as_ptr(&f.preview));
                if self.config_generation != generation || (shown.is_some() && now != shown) {
                    // The edit made the slice out of date (and left Preview): stay, and keep
                    // the scheme and layer range for the next slice.
                    self.preview = true;
                    self.preview_side.hold = true;
                    if shown.is_some() {
                        self.keep_view = Some(view);
                    }
                }
                ui.add_space(12.);
            });
    }

    /// The Preview page (inside the rail's content column).
    pub(crate) fn preview_page(&mut self, ui: &mut egui::Ui) {
        let p = theme::of(ui);
        self.preview_side.probe.page = true;
        // Title row, as a settings topic's.
        ui.add_space(12.);
        let (title_row, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 24.), Sense::hover());
        ui.painter().text(
            Pos2::new(title_row.left() + 12., title_row.center().y),
            Align2::LEFT_CENTER,
            "Preview",
            theme::semibold(18.),
            p.text,
        );
        ui.add_space(12.);
        let (rule, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.), Sense::hover());
        ui.painter().line_segment(
            [rule.left_top(), rule.right_top()],
            Stroke::new(1., p.border),
        );
        ui.add_space(4.);
        let Some(f) = self.finished() else {
            self.outdated_page(ui);
            return;
        };
        let preview: Arc<Preview> = f.preview.clone();
        let summary = f.summary.clone();
        let tool_colors = self.tool_colors_f32();
        self.color_section(ui, &preview, &summary, &tool_colors);
        self.totals_section(ui, &preview, &summary, &tool_colors);
        self.layer_section(ui, &preview, &summary, &tool_colors);
    }

    /// A setting changed in Preview: the slice is out of date until the plate slices again.
    fn outdated_page(&mut self, ui: &mut egui::Ui) {
        self.preview_side.probe.outdated = true;
        let slicing = self
            .worker
            .as_ref()
            .is_some_and(|(id, _)| *id == self.plates.current().id);
        let can_slice =
            !self.busy() && self.config.is_ok() && !self.plates.current().objects.is_empty();
        let mut slice = false;
        let progress = self.progress;
        section(ui, "Out of date", |ui| {
            note(
                ui,
                if slicing {
                    "Settings changed: slicing again…"
                } else {
                    "Settings changed: this slice is out of date."
                },
            );
            ui.add_space(10.);
            if slicing {
                ui.add(
                    egui::ProgressBar::new(progress.clamp(0., 1.))
                        .desired_height(6.)
                        .desired_width(ui.available_width()),
                );
            } else {
                slice = widgets::primary_button(ui, "Slice now", can_slice).clicked();
            }
        });
        if slice {
            let ctx = ui.ctx().clone();
            self.slice_current(&ctx);
        }
    }

    fn color_section(
        &mut self,
        ui: &mut egui::Ui,
        preview: &Preview,
        summary: &crate::worker::Summary,
        tool_colors: &[[f32; 3]],
    ) {
        let totals = Totals {
            grams: summary.grams,
            filament_mm: summary.filament_mm,
            tool_grams: summary.tools.iter().map(|t| (t.tool, t.grams)).collect(),
        };
        let view: &mut View = &mut self.view;
        let probe = &mut self.preview_side.probe;
        section(ui, "Color by", |ui| {
            legend::color_by(ui, view);
            ui.add_space(10.);
            legend::body(ui, view, preview, tool_colors, Some(&totals));
            probe.color_by = view.color.label().to_string();
            probe.single = view
                .color
                .is_gradient()
                .then(|| legend::single_value(view.color, preview))
                .flatten()
                .map(|v| legend::single_text(view.color, v));
        });
    }

    fn totals_section(
        &mut self,
        ui: &mut egui::Ui,
        preview: &Preview,
        summary: &crate::worker::Summary,
        tool_colors: &[[f32; 3]],
    ) {
        let mut rows: Vec<(Option<Color32>, String, String, Option<String>)> = vec![
            (
                None,
                "Print time".into(),
                format_time(preview.total_time()),
                None,
            ),
            (
                None,
                "Filament".into(),
                amount(summary.grams, summary.filament_mm),
                None,
            ),
        ];
        if summary.tools.len() > 1 {
            for t in &summary.tools {
                let c = tool_colors
                    .get(t.tool)
                    .copied()
                    .unwrap_or_else(|| legend::tool_color(t.tool));
                let name = crate::preset_names::display_name(&t.filament);
                let label = if name.is_empty() {
                    format!("Tool {}", t.tool + 1)
                } else {
                    format!("Tool {} · {name}", t.tool + 1)
                };
                rows.push((
                    Some(theme::to_color32(c)),
                    label,
                    amount(t.grams, t.filament_mm),
                    (!t.filament.is_empty()).then(|| t.filament.clone()),
                ));
            }
        }
        if summary.cost > 0. {
            rows.push((
                None,
                "Cost".into(),
                format!("{:.2}", summary.cost),
                Some("From the filament price per kg (Filament settings › Price)".into()),
            ));
        }
        let estimated = preview.total_time_s.is_none();
        let probe = &mut self.preview_side.probe;
        section(ui, "Totals", |ui| {
            for (swatch, label, value, hover) in &rows {
                value_row(ui, *swatch, label, value, hover.as_deref());
                probe.totals.push((label.clone(), value.clone()));
            }
            ui.add_space(6.);
            note(
                ui,
                if estimated {
                    "Times are estimated."
                } else {
                    "Times match the exported G-code."
                },
            );
        });
    }

    fn layer_section(
        &mut self,
        ui: &mut egui::Ui,
        preview: &Preview,
        summary: &crate::worker::Summary,
        tool_colors: &[[f32; 3]],
    ) {
        let n = preview.layers.len();
        if n == 0 {
            return;
        }
        let i = self.view.high.min(n - 1);
        let layer = &preview.layers[i];
        let below = if i > 0 { preview.layers[i - 1].z } else { 0. };
        // The layer's own estimate, as the layer slider and the Layer time scheme show it.
        let time = layer.time_s as f64;
        let mut rows: Vec<(Option<Color32>, String, String)> = vec![
            (None, "Layer".into(), format!("{} of {n}", i + 1)),
            (None, "Z height".into(), format!("{:.2} mm", layer.z)),
            (
                None,
                "Layer height".into(),
                format!("{:.2} mm", (layer.z - below).max(0.)),
            ),
            (None, "Layer time".into(), format_seconds(time)),
        ];
        if summary.tools.len() > 1 {
            let key = Arc::as_ptr(&preview.job) as usize;
            let cached = self
                .preview_side
                .layer_tools
                .as_ref()
                .filter(|(k, l, _)| *k == key && *l == i)
                .map(|(.., v)| v.clone());
            let volumes = cached.unwrap_or_else(|| {
                let v = tool_volumes(preview, layer.start, layer.count);
                self.preview_side.layer_tools = Some((key, i, v.clone()));
                v
            });
            if volumes.is_empty() {
                rows.push((None, "Tools".into(), "No extrusion".into()));
            }
            for (tool, volume) in volumes {
                let c = tool_colors
                    .get(tool)
                    .copied()
                    .unwrap_or_else(|| legend::tool_color(tool));
                let value = layer_usage(preview, tool, volume, summary)
                    .map_or_else(|| "–".to_string(), |(g, mm)| layer_amount(g, mm));
                rows.push((
                    Some(theme::to_color32(c)),
                    format!("Tool {} on this layer", tool + 1),
                    value,
                ));
            }
        }
        let probe = &mut self.preview_side.probe;
        section(ui, "Layer", |ui| {
            for (swatch, label, value) in &rows {
                value_row(ui, *swatch, label, value, None);
                probe.layer.push((label.clone(), value.clone()));
            }
        });
    }

    /// The collapsed sidebar's chip in the 3D view's top corner (the sidebar's side): the
    /// color scheme with its colors; a click brings the sidebar back.
    pub(crate) fn preview_legend_chip(&mut self, ctx: &egui::Context) {
        let Some(f) = self.finished() else {
            return;
        };
        let preview = f.preview.clone();
        let mode = self.view.color;
        let tool_colors = self.tool_colors_f32();
        let label = format!("Color by {}", mode.label());
        let r = self.viewport_rect;
        let (pos, pivot) = if self.prefs.sidebar_right {
            (Pos2::new(r.right() - 14., r.top() + 14.), Align2::RIGHT_TOP)
        } else {
            (Pos2::new(r.left() + 14., r.top() + 14.), Align2::LEFT_TOP)
        };
        let mut open = false;
        let mut chip = None;
        egui::Area::new(egui::Id::unique("preview legend chip"))
            .order(egui::Order::Middle)
            .fixed_pos(pos)
            .pivot(pivot)
            .show(ctx, |ui| {
                let p = theme::of(ui);
                theme::overlay_frame(ui)
                    .inner_margin(egui::Margin::same(0))
                    .show(ui, |ui| {
                        let g =
                            ui.painter()
                                .layout_no_wrap(label.clone(), theme::sans(13.), p.text);
                        let size = Vec2::new(10. + 28. + 8. + g.size().x + 12. + 14., 30.);
                        let (rect, response) = ui.allocate_exact_size(size, Sense::click());
                        if response.hovered() {
                            ui.painter().rect_filled(rect.shrink(1.), 9., p.hover);
                            ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
                        }
                        let strip = Rect::from_min_size(
                            Pos2::new(rect.left() + 10., rect.center().y - 5.),
                            Vec2::new(28., 10.),
                        );
                        chip_colors(ui.painter(), strip, mode, &preview, &tool_colors);
                        ui.painter().galley(
                            Pos2::new(strip.right() + 8., rect.center().y - g.size().y / 2.),
                            g,
                            p.text,
                        );
                        widgets::chevron(
                            ui.painter(),
                            Pos2::new(rect.right() - 14., rect.center().y),
                            true,
                            p.muted_alt,
                        );
                        open = response
                            .on_hover_text("Show the sidebar with the legend (Shift+Tab)")
                            .clicked();
                        chip = Some(rect);
                    });
            });
        self.preview_side.probe.chip = chip;
        if open {
            self.keys.sidebar_hidden = false;
        }
    }
}

/// The chip's small color strip: the first features' colors, the tools' colors, or the
/// gradient (its middle color for a scheme with one value).
fn chip_colors(
    painter: &egui::Painter,
    strip: Rect,
    mode: crate::preview::ColorMode,
    p: &Preview,
    tool_colors: &[[f32; 3]],
) {
    use crate::preview::ColorMode;
    let colors: Vec<Color32> = match mode {
        ColorMode::Feature => (0..16)
            .filter(|&i| i != crate::preview::TRAVEL as usize && p.stats.roles[i].count > 0)
            .take(4)
            .map(|i| {
                let c = legend::COLORS[i];
                Color32::from_rgb(c[0], c[1], c[2])
            })
            .collect(),
        ColorMode::Tool | ColorMode::ColorPrint => (0..16)
            .filter(|&i| p.stats.tools[i].count > 0)
            .take(4)
            .map(|i| {
                theme::to_color32(
                    tool_colors
                        .get(i)
                        .copied()
                        .unwrap_or_else(|| legend::tool_color(i)),
                )
            })
            .collect(),
        m if legend::single_value(m, p).is_some() => {
            vec![theme::to_color32(legend::gradient(0.5))]
        }
        _ => (0..14)
            .map(|i| theme::to_color32(legend::gradient(i as f32 / 13.)))
            .collect(),
    };
    if colors.is_empty() {
        return;
    }
    let w = strip.width() / colors.len() as f32;
    for (k, c) in colors.iter().enumerate() {
        painter.rect_filled(
            Rect::from_min_size(
                Pos2::new(strip.left() + w * k as f32, strip.top()),
                Vec2::new(w + 0.5, strip.height()),
            ),
            0.,
            *c,
        );
    }
}

#[cfg(test)]
#[path = "preview_side_tests.rs"]
mod tests;
