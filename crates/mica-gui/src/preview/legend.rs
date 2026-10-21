// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/slic3r/GUI/GCodeViewer.cpp :: GCodeViewer::render_legend
// Source: prusaslicer@b028299c77 src/libvgcode/include/Types.hpp :: EViewType
// Changes: The feature table's columns (time, share, used filament, click to hide) and the gradient legend with linear / logarithmic layer-time ticks follow render_legend, the scheme menu follows EViewType's order (acceleration added after flow); filament per feature is the export's total split by extruded volume; the rows, ticks and colour print rows are drawn with egui in Mica's own layout inside the Preview sidebar; linear scales get round interior ticks and a mode with one value over the whole print shows a single swatch instead of a range.

//! Preview legend, drawn in the Preview sidebar (`panels/preview_side.rs`): the "Color by"
//! combo, one row per feature with its time, share and filament (click to hide), and for the
//! numeric modes a gradient scale with the real value range and ticks (logarithmic for the log
//! layer-time view), as PrusaSlicer's and Orca's legends show them. A mode whose values do not
//! differ within the labels' precision (all lines at 220 °C) shows that one value instead of a
//! made-up range.

use super::{ColorMode, Preview, TRAVEL, View, stats::format_time};
use crate::theme;
use eframe::egui::{self, Align2, Color32, Pos2, Rect, Sense, Stroke, Vec2};
pub const NAMES: [&str; 16] = [
    "Skirt",
    "Brim",
    "Outer wall",
    "Overhang wall",
    "Inner wall",
    "Sparse infill",
    "Solid infill",
    "Bridge",
    "Internal bridge",
    "Top surface",
    "Ironing",
    "Gap infill",
    "Support",
    "Support interface",
    "Travel",
    "Prime tower",
];
pub const COLORS: [[u8; 3]; 16] = [
    [70, 190, 90],
    [70, 190, 90],
    [255, 125, 56],
    [60, 100, 255],
    [255, 230, 77],
    [176, 48, 41],
    [150, 84, 204],
    [80, 150, 255],
    [40, 210, 235],
    [240, 64, 64],
    [60, 210, 220],
    [255, 255, 255],
    [75, 180, 105],
    [75, 210, 220],
    [110, 135, 160],
    [210, 170, 120],
];
pub const TOOLS: [[u8; 3]; 8] = [
    [255, 125, 56],
    [65, 180, 240],
    [130, 215, 100],
    [215, 105, 225],
    [245, 210, 65],
    [65, 220, 200],
    [230, 90, 115],
    [175, 180, 235],
];
/// Fallback tool colour when the profile has none.
pub fn tool_color(i: usize) -> [f32; 3] {
    TOOLS[i % 8].map(|c| c as f32 / 255.)
}
/// Same five-stop ramp as `segments.wgsl`.
pub fn gradient(t: f32) -> [f32; 3] {
    const STOPS: [[f32; 3]; 5] = [
        [0.16, 0.30, 0.90],
        [0.15, 0.78, 0.92],
        [0.22, 0.80, 0.30],
        [0.98, 0.84, 0.18],
        [0.92, 0.22, 0.18],
    ];
    let x = t.clamp(0., 1.) * 4.;
    let i = (x.floor() as usize).min(3);
    let f = x - i as f32;
    let (a, b) = (STOPS[i], STOPS[i + 1]);
    [
        a[0] + (b[0] - a[0]) * f,
        a[1] + (b[1] - a[1]) * f,
        a[2] + (b[2] - a[2]) * f,
    ]
}

/// Weight and length of the slice, for the footer, and the weight per tool for the rows.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Totals {
    pub grams: f64,
    pub filament_mm: f64,
    pub tool_grams: Vec<(usize, f64)>,
}

/// Per-feature times scaled so that they add up to the exported print time (the estimator's
/// own total includes travel and is slightly off from the export). Returns `(time, share)` for
/// a role: share in percent of the total.
pub fn feature_time(preview: &Preview, role: usize) -> (f64, f64) {
    let estimated = preview.stats.total_time();
    let total = preview.total_time();
    let scale = if estimated > 0. {
        total / estimated
    } else {
        1.
    };
    let t = preview.stats.roles[role].time_s * scale;
    let share = if total > 0. { 100. * t / total } else { 0. };
    (t, share)
}
pub fn tool_time(preview: &Preview, tool: usize) -> (f64, f64) {
    let estimated = preview.stats.total_time();
    let total = preview.total_time();
    let scale = if estimated > 0. {
        total / estimated
    } else {
        1.
    };
    let t = preview.stats.tools[tool].time_s * scale;
    let share = if total > 0. { 100. * t / total } else { 0. };
    (t, share)
}

/// Time of layers `first..=last` scaled to the exported total, and its share in percent.
pub fn segment_time(preview: &Preview, first: usize, last: usize) -> (f64, f64) {
    let all: f64 = preview.layers.iter().map(|l| l.time_s as f64).sum();
    let total = preview.total_time();
    let part: f64 = preview
        .layers
        .iter()
        .skip(first)
        .take(last.saturating_sub(first) + 1)
        .map(|l| l.time_s as f64)
        .sum();
    if all <= 0. {
        return (0., 0.);
    }
    (part / all * total, 100. * part / all)
}

/// Filament (grams when the slice knows its weight, else metres) of each feature: the export's
/// total split by the features' extruded volume. Travel and empty roles get `None`.
pub fn feature_filament(preview: &Preview, role: usize, totals: Option<&Totals>) -> Option<String> {
    let totals = totals?;
    let volume: f64 = preview
        .stats
        .roles
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != TRAVEL as usize)
        .map(|(_, b)| b.volume_mm3)
        .sum();
    let v = preview.stats.roles.get(role)?.volume_mm3;
    if role == TRAVEL as usize || volume <= 0. || v <= 0. {
        return None;
    }
    let share = v / volume;
    Some(if totals.grams > 0. {
        format!("{:.1} g", totals.grams * share)
    } else {
        format!("{:.2} m", totals.filament_mm * share / 1000.)
    })
}

/// Tick values under a gradient bar: `n` evenly spaced values, or evenly spaced on a log scale.
pub fn ticks(lo: f32, hi: f32, n: usize, log: bool) -> Vec<f32> {
    (0..n)
        .map(|k| {
            let t = if n > 1 { k as f32 / (n - 1) as f32 } else { 0. };
            if log {
                let (a, b) = (lo.max(0.1).ln(), hi.max(lo.max(0.1) * 1.01).ln());
                (a + (b - a) * t).exp()
            } else {
                lo + (hi - lo) * t
            }
        })
        .collect()
}

/// A gradient value with its unit, at the precision the mode needs.
pub fn format_value(mode: ColorMode, v: f32) -> String {
    match mode {
        ColorMode::LayerTime | ColorMode::LayerTimeLog => super::stats::format_seconds(v as f64),
        ColorMode::Fan => format!("{}%", tick_text(mode, v)),
        _ => format!("{} {}", tick_text(mode, v), mode.unit()),
    }
}

/// Whether the mode's values are whole numbers on screen (speeds, fan, temperature,
/// acceleration): their labels never show decimals.
fn integer_mode(mode: ColorMode) -> bool {
    !matches!(
        mode,
        ColorMode::Width
            | ColorMode::Height
            | ColorMode::VolumetricFlow
            | ColorMode::LayerTime
            | ColorMode::LayerTimeLog
    )
}

/// A tick label without its unit: decimals only where the mode needs them.
pub fn tick_text(mode: ColorMode, v: f32) -> String {
    match mode {
        ColorMode::Width | ColorMode::Height => format!("{v:.2}"),
        ColorMode::VolumetricFlow => format!("{v:.1}"),
        ColorMode::LayerTime | ColorMode::LayerTimeLog => super::stats::format_seconds(v as f64),
        // `+ 0.` turns a rounded -0 into 0.
        _ => format!("{:.0}", v.round() + 0.),
    }
}

/// The smallest difference the mode's labels show: 0.01 mm, 0.1 mm³/s, 0.1 s below ten
/// seconds (1 s above), else 1.
pub fn display_precision(mode: ColorMode, v: f32) -> f32 {
    match mode {
        ColorMode::Width | ColorMode::Height => 0.01,
        ColorMode::VolumetricFlow => 0.1,
        ColorMode::LayerTime | ColorMode::LayerTimeLog if v.abs() < 10. => 0.1,
        _ => 1.,
    }
}

/// The recorded value range of a gradient mode, when it has values.
pub fn raw_range(mode: ColorMode, preview: &Preview) -> Option<super::stats::Range> {
    let s = &preview.stats;
    let r = match mode {
        ColorMode::Speed => s.speed,
        ColorMode::Fan => s.fan,
        ColorMode::Width => s.width,
        ColorMode::Height => s.height,
        ColorMode::LayerTime | ColorMode::LayerTimeLog => s.layer_time,
        ColorMode::ActualSpeed => s.actual,
        ColorMode::VolumetricFlow => s.flow,
        ColorMode::Temperature => s.temp,
        ColorMode::Acceleration => s.accel,
        ColorMode::Feature | ColorMode::Tool | ColorMode::ColorPrint => return None,
    };
    r.is_valid().then_some(r)
}

/// The one value of a `[min, max]` range when its spread is below what the labels show.
pub fn degenerate(mode: ColorMode, min: f32, max: f32) -> Option<f32> {
    (max - min < display_precision(mode, max)).then_some((min + max) * 0.5)
}

/// The one value a gradient mode has over the whole print, when its spread is below what the
/// labels can show (all 220 °C, every line 0.45 mm): the legend shows a single swatch instead
/// of a range, and the shader paints every line in the scale's middle color. `None` for a real
/// range, a mode without values or a discrete mode.
pub fn single_value(mode: ColorMode, preview: &Preview) -> Option<f32> {
    let r = raw_range(mode, preview)?;
    degenerate(mode, r.min, r.max)
}

/// The labelled values under a gradient bar, ends included: the logarithmic view keeps its
/// five log-spaced ticks; a linear scale adds round values between the ends (steps of 1, 2
/// or 5 × 10ⁿ, at least one unit for whole-number modes, never finer than the labels show),
/// leaving out those too close to an end for their labels to fit.
pub fn scale_ticks(mode: ColorMode, lo: f32, hi: f32) -> Vec<f32> {
    if mode == ColorMode::LayerTimeLog {
        return ticks(lo, hi, 5, true);
    }
    let span = hi - lo;
    if span.is_nan() || span <= 0. {
        return vec![lo];
    }
    let raw = (span / 4.).max(display_precision(mode, lo));
    let magnitude = 10f32.powf(raw.log10().floor());
    let mut step = [1., 2., 5., 10.]
        .iter()
        .map(|m| m * magnitude)
        .find(|s| *s >= raw * 0.999)
        .unwrap_or(10. * magnitude);
    if integer_mode(mode) {
        step = step.round().max(1.);
    }
    let mut out = vec![lo];
    let mut k = (lo / step).ceil();
    loop {
        let v = k * step;
        if v >= hi {
            break;
        }
        let t = (v - lo) / span;
        if t > 0.2 + 1e-3 && t < 0.8 - 1e-3 {
            out.push(v);
        }
        k += 1.;
    }
    out.push(hi);
    out
}

/// What a gradient view shows, in plain words.
pub fn note(mode: ColorMode, preview: &Preview) -> &'static str {
    match mode {
        ColorMode::Speed => {
            "Commanded speed after layer-time slowdown; travel routing not simulated"
        }
        ColorMode::ActualSpeed => {
            "Simulated with the printer's acceleration and jerk limits: the mean speed of each line"
        }
        ColorMode::Fan => "After cooling and first-layer ramp",
        ColorMode::Temperature => {
            "Nozzle temperature: first layer, then the filament's (and calibration bands)"
        }
        ColorMode::VolumetricFlow => {
            "Melted plastic per second: line cross-section × commanded speed"
        }
        ColorMode::Acceleration if preview.stats.accel.max <= 0. => {
            "This printer profile sends no acceleration commands"
        }
        ColorMode::Acceleration => "Acceleration the G-code commands per feature (mm/s²)",
        ColorMode::LayerTime => "Per layer, after cooling slowdown",
        ColorMode::LayerTimeLog => "Per layer, logarithmic scale: short layers stand out",
        _ => "",
    }
}

/// Widths of the legend's number columns (monospace 12 px: `1h 02m`, `100%`, `12.3 g`) and
/// the gap between columns.
const TIME_W: f32 = 48.;
const PCT_W: f32 = 30.;
const AMOUNT_W: f32 = 46.;
const GAP: f32 = 8.;

/// Right edges of the time, share and amount columns of a row ending at `right`.
fn columns(right: f32, amount: bool) -> [f32; 3] {
    let amount_x = right - 4.;
    let pct_x = if amount {
        amount_x - AMOUNT_W - GAP
    } else {
        amount_x
    };
    [pct_x - PCT_W - GAP, pct_x, amount_x]
}

/// One legend row: swatch, name, time, share and an optional amount. Dimmed and suffixed
/// "(hidden)" when off.
fn row(
    ui: &mut egui::Ui,
    color: Color32,
    name: &str,
    time: f64,
    percent: f64,
    on: &mut bool,
) -> bool {
    row_with(ui, color, name, time, percent, None, on)
}
fn row_with(
    ui: &mut egui::Ui,
    color: Color32,
    name: &str,
    time: f64,
    percent: f64,
    amount: Option<&str>,
    on: &mut bool,
) -> bool {
    let p = theme::of(ui);
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 26.), Sense::click());
    let alpha = if *on { 1. } else { 0.45 };
    if response.hovered() {
        ui.painter().rect_filled(rect, 5., p.hover);
        ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
    }
    let painter = ui.painter();
    let sw = Rect::from_center_size(
        Pos2::new(rect.left() + 4. + 6., rect.center().y),
        Vec2::splat(12.),
    );
    painter.rect_filled(sw, 3., color.gamma_multiply(alpha));
    let label = if *on {
        name.to_string()
    } else {
        format!("{name} (hidden)")
    };
    let [time_x, pct_x, amount_x] = columns(rect.right(), amount.is_some());
    let name_right = time_x - TIME_W - GAP;
    if let Some(a) = amount {
        painter.text(
            Pos2::new(amount_x, rect.center().y),
            Align2::RIGHT_CENTER,
            a,
            theme::mono(12.),
            p.label.gamma_multiply(alpha),
        );
    }
    painter.text(
        Pos2::new(pct_x, rect.center().y),
        Align2::RIGHT_CENTER,
        format!("{percent:.0}%"),
        theme::mono(12.),
        p.muted.gamma_multiply(alpha),
    );
    painter.text(
        Pos2::new(time_x, rect.center().y),
        Align2::RIGHT_CENTER,
        format_time(time),
        theme::mono(12.),
        p.label.gamma_multiply(alpha),
    );
    // One line, cut with an ellipsis where the columns begin (narrow sidebar).
    let g = truncated(
        painter,
        label,
        theme::sans(13.),
        p.text.gamma_multiply(alpha),
        (name_right - sw.right() - 8.).max(20.),
    );
    let name_rect = Rect::from_min_max(
        Pos2::new(sw.right() + 8., rect.top()),
        Pos2::new(name_right, rect.bottom()),
    );
    let elided = g.elided;
    painter.galley(
        Pos2::new(sw.right() + 8., rect.center().y - g.size().y / 2.),
        g,
        p.text.gamma_multiply(alpha),
    );
    if elided && response.hovered() && ui.rect_contains_pointer(name_rect) {
        response.clone().on_hover_text(name);
    }
    if response.clicked() {
        *on = !*on;
        return true;
    }
    false
}

/// One line of text cut with an ellipsis at `width`.
pub fn truncated(
    painter: &egui::Painter,
    text: String,
    font: egui::FontId,
    color: Color32,
    width: f32,
) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text, font, color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(width.max(10.));
    painter.layout_job(job)
}

/// The "Color by" combo of the Preview sidebar: the current scheme as a full-width combo
/// button (as the Printer / Filament / Process presets) with every scheme in its menu.
/// Returns true when the scheme changed.
pub fn color_by(ui: &mut egui::Ui, view: &mut View) -> bool {
    let mut changed = false;
    let id = ui.make_persistent_id("legend color");
    let response =
        crate::panels::widgets::combo_button(ui, id, |_| {}, view.color.label(), "", None);
    egui::Popup::menu(&response)
        .id(id.with("menu"))
        .width(response.rect.width())
        .show(|ui| {
            for mode in ColorMode::ALL {
                if ui
                    .selectable_label(view.color == mode, mode.label())
                    .clicked()
                {
                    if view.color != mode {
                        view.color = mode;
                        changed = true;
                    }
                    ui.close();
                }
            }
        });
    changed
}

/// A muted note under the legend.
fn note_label(ui: &mut egui::Ui, text: &str) {
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

/// The legend under the "Color by" combo: one row per feature or tool (click to hide) with
/// time, share and filament, the color print segments, or for the numeric modes the gradient
/// scale with its ticks and unit (a single swatch when the whole print has one value) and the
/// travel row. `tool_colors` come from the active profile. Returns true on change.
pub fn body(
    ui: &mut egui::Ui,
    view: &mut View,
    preview: &Preview,
    tool_colors: &[[f32; 3]],
    totals: Option<&Totals>,
) -> bool {
    let p = theme::of(ui);
    let mut changed = false;
    ui.spacing_mut().item_spacing = Vec2::new(8., 0.);
    let stats = &preview.stats;
    match view.color {
        ColorMode::Feature => {
            // Column heads of the time / filament table.
            let (head, _) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), 18.), Sense::hover());
            let [time_x, pct_x, amount_x] = columns(head.right(), true);
            for (x, text) in [
                (amount_x, "Filament"),
                (pct_x, "Share"),
                (time_x, "Time"),
                (head.left() + 4., "Feature"),
            ] {
                ui.painter().text(
                    Pos2::new(x, head.center().y),
                    if text == "Feature" {
                        Align2::LEFT_CENTER
                    } else {
                        Align2::RIGHT_CENTER
                    },
                    text,
                    theme::sans(10.5),
                    p.muted,
                );
            }
            for (i, c) in COLORS.into_iter().enumerate() {
                let name = preview.role_name(i);
                if stats.roles[i].count == 0 {
                    continue;
                }
                let mut on = view.mask & (1 << i) != 0;
                let (t, share) = feature_time(preview, i);
                let amount = feature_filament(preview, i, totals);
                if row_with(
                    ui,
                    Color32::from_rgb(c[0], c[1], c[2]),
                    name,
                    t,
                    share,
                    Some(amount.as_deref().unwrap_or("")),
                    &mut on,
                ) {
                    view.mask ^= 1 << i;
                    changed = true;
                }
            }
            ui.add_space(6.);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 12.;
                let extrusions: u32 = ((1u32 << 16) - 1) & !(1 << TRAVEL);
                if crate::panels::widgets::link(ui, "Show all").clicked() {
                    view.mask |= extrusions;
                    changed = true;
                }
                if crate::panels::widgets::link(ui, "Hide all").clicked() {
                    view.mask &= !extrusions;
                    changed = true;
                }
            });
            ui.add_space(6.);
            note_label(ui, "Click a row to hide it.");
        }
        ColorMode::ColorPrint if preview.color_print.is_some() => {
            let cp = preview.color_print.as_ref().unwrap();
            for seg in &cp.segments {
                let (t, share) = segment_time(preview, seg.first, seg.last);
                let z = |i: usize| preview.layers.get(i).map_or(0., |l| l.z);
                let lo = z(seg.first);
                let name = match seg.tool {
                    Some(tool) => format!("Tool {} from {:.2} mm", tool + 1, lo),
                    None => match seg.swatch {
                        crate::preview::color_print::Swatch::Tool(t) if cp.tools > 1 => {
                            format!("Tool {} · {:.2}–{:.2} mm", t + 1, lo, z(seg.last))
                        }
                        _ => format!("{:.2}–{:.2} mm", lo, z(seg.last)),
                    },
                };
                let mut on = true;
                row(
                    ui,
                    theme::to_color32(cp.swatch_color(seg.swatch, tool_colors)),
                    &name,
                    t,
                    share,
                    &mut on,
                );
            }
            for e in &cp.events {
                let z = preview.layers.get(e.layer).map_or(0., |l| l.z);
                ui.label(
                    egui::RichText::new(format!("At {z:.2} mm: {}", e.label))
                        .font(theme::sans(12.))
                        .color(p.label),
                );
            }
        }
        ColorMode::Tool | ColorMode::ColorPrint => {
            for (i, bucket) in stats.tools.iter().enumerate() {
                if bucket.count == 0 {
                    continue;
                }
                let mut on = view.tool_mask & (1 << i) != 0;
                let c = tool_colors.get(i).copied().unwrap_or_else(|| tool_color(i));
                let (t, share) = tool_time(preview, i);
                let grams = totals
                    .and_then(|t| t.tool_grams.iter().find(|(tool, _)| *tool == i))
                    .map(|(_, g)| format!("{g:.1} g"));
                if row_with(
                    ui,
                    theme::to_color32(c),
                    &format!("Tool {}", i + 1),
                    t,
                    share,
                    Some(grams.as_deref().unwrap_or("")),
                    &mut on,
                ) {
                    view.tool_mask ^= 1 << i;
                    changed = true;
                }
            }
            if stats.roles[TRAVEL as usize].count > 0 {
                let mut on = view.travel_visible();
                let c = COLORS[TRAVEL as usize];
                let (t, share) = feature_time(preview, TRAVEL as usize);
                if row_with(
                    ui,
                    Color32::from_rgb(c[0], c[1], c[2]),
                    "Travel",
                    t,
                    share,
                    Some(""),
                    &mut on,
                ) {
                    view.mask ^= 1 << TRAVEL;
                    changed = true;
                }
            }
            ui.add_space(6.);
            note_label(ui, "Click a row to hide it.");
        }
        mode => {
            debug_assert!(mode.is_gradient());
            if let Some(v) = single_value(mode, preview) {
                single_swatch(ui, mode, v);
            } else {
                scale(ui, mode, view.gradient_range(preview));
            }
            let note = note(mode, preview);
            if !note.is_empty() {
                ui.add_space(6.);
                note_label(ui, note);
            }
            ui.add_space(6.);
            let mut travel = view.travel_visible();
            let c = COLORS[TRAVEL as usize];
            let (t, share) = feature_time(preview, TRAVEL as usize);
            if row(
                ui,
                Color32::from_rgb(c[0], c[1], c[2]),
                "Travel",
                t,
                share,
                &mut travel,
            ) {
                view.mask ^= 1 << TRAVEL;
                changed = true;
            }
        }
    }
    changed
}

/// The label of a print with one value of a gradient mode: `220 °C (whole print)`.
pub fn single_text(mode: ColorMode, v: f32) -> String {
    format!("{} (whole print)", format_value(mode, v))
}

/// A gradient mode with one value over the whole print: one swatch in the scale's middle
/// color (the color the lines are painted in) and the value.
fn single_swatch(ui: &mut egui::Ui, mode: ColorMode, v: f32) {
    let p = theme::of(ui);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 26.), Sense::hover());
    let sw = Rect::from_center_size(
        Pos2::new(rect.left() + 4. + 6., rect.center().y),
        Vec2::splat(12.),
    );
    let painter = ui.painter();
    painter.rect_filled(sw, 3., theme::to_color32(gradient(0.5)));
    let g = painter.layout_no_wrap(format_value(mode, v), theme::sans(13.), p.text);
    let x = sw.right() + 8.;
    let w = g.size().x;
    painter.galley(Pos2::new(x, rect.center().y - g.size().y / 2.), g, p.text);
    painter.text(
        Pos2::new(x + w + 6., rect.center().y),
        Align2::LEFT_CENTER,
        "(whole print)",
        theme::sans(13.),
        p.muted,
    );
}

/// The gradient bar with its ticks, labels and unit.
fn scale(ui: &mut egui::Ui, mode: ColorMode, [lo, hi]: [f32; 2]) {
    let p = theme::of(ui);
    let log = mode == ColorMode::LayerTimeLog;
    ui.add_space(4.);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 12.), Sense::hover());
    let n = 48;
    for i in 0..n {
        let t0 = i as f32 / n as f32;
        let t1 = (i + 1) as f32 / n as f32;
        let c = gradient((t0 + t1) * 0.5);
        ui.painter().rect_filled(
            Rect::from_min_max(
                Pos2::new(rect.left() + rect.width() * t0, rect.top()),
                Pos2::new(rect.left() + rect.width() * t1 + 0.5, rect.bottom()),
            ),
            0.,
            theme::to_color32(c),
        );
    }
    // Ticks under the bar at their own positions: the ends and round values between (five
    // log-spaced ticks in the logarithmic view), then the unit once at the right.
    let (scale_rect, _) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 18.), Sense::hover());
    let values = scale_ticks(mode, lo, hi);
    let last = values.len().saturating_sub(1);
    for (k, v) in values.iter().enumerate() {
        let t = if log {
            k as f32 / last.max(1) as f32
        } else if hi > lo {
            ((v - lo) / (hi - lo)).clamp(0., 1.)
        } else {
            0.
        };
        let x = scale_rect.left() + scale_rect.width() * t;
        ui.painter().line_segment(
            [
                Pos2::new(x, scale_rect.top()),
                Pos2::new(x, scale_rect.top() + 3.),
            ],
            Stroke::new(1., p.muted),
        );
        let align = match k {
            0 => Align2::LEFT_TOP,
            k if k == last => Align2::RIGHT_TOP,
            _ => Align2::CENTER_TOP,
        };
        ui.painter().text(
            Pos2::new(x, scale_rect.top() + 4.),
            align,
            tick_text(mode, *v),
            theme::mono(11.),
            p.label,
        );
    }
    if !matches!(mode, ColorMode::LayerTime | ColorMode::LayerTimeLog) {
        let (unit_rect, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 14.), Sense::hover());
        ui.painter().text(
            Pos2::new(unit_rect.right(), unit_rect.center().y),
            Align2::RIGHT_CENTER,
            mode.unit(),
            theme::mono(11.),
            p.muted,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn gradient_endpoints_and_monotone_hue() {
        let close = |a: [f32; 3], b: [f32; 3]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-5);
        assert!(close(gradient(0.), [0.16, 0.30, 0.90]));
        assert!(close(gradient(1.), [0.92, 0.22, 0.18]));
        let mid = gradient(0.5);
        assert!(mid[1] > mid[0] && mid[1] > mid[2]);
        assert_eq!(gradient(-5.), gradient(0.));
        assert_eq!(tool_color(9), tool_color(1));
    }
    #[test]
    fn gradient_ticks_and_formats() {
        assert_eq!(ticks(0., 100., 5, false), vec![0., 25., 50., 75., 100.]);
        // Log ticks: the geometric series between the ends.
        let t = ticks(1., 100., 3, true);
        assert!(
            (t[0] - 1.).abs() < 1e-4 && (t[1] - 10.).abs() < 1e-3 && (t[2] - 100.).abs() < 1e-2
        );
        // Layer times below 0.1 s are clamped on the log scale.
        assert!((ticks(0., 10., 2, true)[0] - 0.1).abs() < 1e-6);
        assert_eq!(format_value(ColorMode::VolumetricFlow, 12.34), "12.3 mm³/s");
        assert_eq!(format_value(ColorMode::Temperature, 215.), "215 °C");
        assert_eq!(format_value(ColorMode::Width, 0.45), "0.45 mm");
        assert_eq!(format_value(ColorMode::LayerTimeLog, 6.21), "6.2 s");
        // Whole-number quantities never show decimals.
        assert_eq!(tick_text(ColorMode::Temperature, 226.25), "226");
        assert_eq!(tick_text(ColorMode::Temperature, 219.8), "220");
        assert_eq!(tick_text(ColorMode::Temperature, 230.), "230");
        assert_eq!(tick_text(ColorMode::Fan, -0.2), "0");
        assert_eq!(format_value(ColorMode::Acceleration, 4000.), "4000 mm/s²");
        for mode in ColorMode::ALL {
            assert_eq!(ColorMode::parse(mode.key()), Some(mode));
            assert!(!mode.label().is_empty());
        }
        assert_eq!(
            ColorMode::parse("colour-print"),
            Some(ColorMode::ColorPrint)
        );
        assert_eq!(ColorMode::parse("nope"), None);
    }
    #[test]
    fn a_range_below_the_display_precision_is_one_value() {
        use ColorMode as M;
        // All 220 °C (today's 219.5…220.5 was a made-up range).
        assert_eq!(degenerate(M::Temperature, 220., 220.), Some(220.));
        assert_eq!(single_text(M::Temperature, 220.), "220 °C (whole print)");
        assert_eq!(degenerate(M::Temperature, 219.6, 220.4), Some(220.));
        assert_eq!(degenerate(M::Temperature, 215., 230.), None);
        // Fan, speed and acceleration: whole units.
        assert_eq!(degenerate(M::Fan, 100., 100.), Some(100.));
        assert_eq!(single_text(M::Fan, 100.), "100% (whole print)");
        assert_eq!(degenerate(M::Speed, 40., 41.), None);
        assert_eq!(
            single_text(M::Acceleration, 4000.),
            "4000 mm/s² (whole print)"
        );
        // Width and height: hundredths of a millimetre.
        assert!(degenerate(M::Width, 0.45, 0.452).is_some_and(|v| (v - 0.451).abs() < 1e-5));
        assert_eq!(single_text(M::Width, 0.45), "0.45 mm (whole print)");
        assert_eq!(degenerate(M::Height, 0.2, 0.25), None);
        // Flow: tenths; layer time: tenths of a second below ten seconds, else seconds.
        assert!(degenerate(M::VolumetricFlow, 12.3, 12.35).is_some());
        assert!(degenerate(M::LayerTime, 5., 5.05).is_some());
        assert_eq!(degenerate(M::LayerTime, 5., 5.2), None);
        assert!(degenerate(M::LayerTime, 30., 30.6).is_some());
        assert_eq!(single_text(M::LayerTime, 6.2), "6.2 s (whole print)");
    }
    #[test]
    fn scale_ticks_are_round_values_between_the_ends() {
        use ColorMode as M;
        assert_eq!(
            scale_ticks(M::Speed, 20., 258.),
            vec![20., 100., 200., 258.]
        );
        assert_eq!(
            scale_ticks(M::Temperature, 200., 240.),
            vec![200., 210., 220., 230., 240.]
        );
        // Whole-number modes never get half steps, nor ticks crowding the ends.
        let t = scale_ticks(M::Temperature, 225., 230.);
        assert_eq!(t, vec![225., 228., 230.]);
        assert!(t.iter().all(|v| v.fract() == 0.));
        assert_eq!(scale_ticks(M::Fan, 0., 100.), vec![0., 50., 100.]);
        // Decimal modes step in their own precision.
        let w = scale_ticks(M::Width, 0.4, 0.5);
        assert_eq!(w.first(), Some(&0.4));
        assert_eq!(w.last(), Some(&0.5));
        assert!(w.len() >= 3);
        // The log view keeps its five log-spaced ticks; an empty span is one tick.
        assert_eq!(scale_ticks(M::LayerTimeLog, 1., 100.).len(), 5);
        assert_eq!(scale_ticks(M::Speed, 50., 50.), vec![50.]);
    }
    #[test]
    fn legend_ranges_come_from_the_records() {
        use crate::preview::{Attr, Builder, Segment};
        use crate::worker::TempJob;
        use std::sync::Arc;
        let job = Arc::new(TempJob::new().unwrap());
        let config = mica_core::config::Config::defaults();
        let settings = vec![mica_core::settings::PrintSettings::core_one_hf04_speed_pla()];
        let mut b = Builder::new(&job, &config, &settings).unwrap();
        for (speed, temp, accel) in [(50., 215., 1000.), (150., 230., 4000.)] {
            let s = Segment::new([0.; 3], [10., 0., 0.2], [0.45, 0.2], 2, 0, speed, 0.);
            b.stats_mut().add(&s, 0);
            b.stats_mut()
                .add_attr(&s, &Attr::new(speed * 0.8, 0., temp, accel));
        }
        let p = b.finish_with(job, Some(10.)).unwrap();
        let range = |mode| {
            View {
                color: mode,
                ..Default::default()
            }
            .gradient_range(&p)
        };
        assert_eq!(range(ColorMode::Speed), [50., 150.]);
        assert_eq!(range(ColorMode::ActualSpeed), [40., 120.]);
        assert_eq!(range(ColorMode::Temperature), [215., 230.]);
        assert_eq!(range(ColorMode::Acceleration), [1000., 4000.]);
        let flow = range(ColorMode::VolumetricFlow);
        let area = 0.2 * (0.45 - 0.2 * (1. - std::f32::consts::FRAC_PI_4));
        assert!((flow[0] - area * 50.).abs() < 1e-3 && (flow[1] - area * 150.).abs() < 1e-3);
        // The log view's shader range is the logarithm of the layer-time range.
        let v = View {
            color: ColorMode::LayerTimeLog,
            ..Default::default()
        };
        let [lo, hi] = v.gradient_range(&p);
        let [llo, lhi] = v.shader_range(&p);
        assert!((llo - lo.max(0.1).ln()).abs() < 1e-6 && (lhi - hi.ln()).abs() < 1e-6);
        // Filament per feature: all of it in the outer wall here.
        let totals = Totals {
            grams: 3.,
            filament_mm: 1000.,
            tool_grams: vec![],
        };
        assert_eq!(
            feature_filament(&p, 2, Some(&totals)).as_deref(),
            Some("3.0 g")
        );
        assert_eq!(feature_filament(&p, TRAVEL as usize, Some(&totals)), None);
    }
    #[test]
    fn feature_times_add_up_to_the_export_total() {
        use crate::preview::{Builder, Segment};
        use crate::worker::TempJob;
        use std::sync::Arc;
        let job = Arc::new(TempJob::new().unwrap());
        let config = mica_core::config::Config::defaults();
        let settings = vec![mica_core::settings::PrintSettings::core_one_hf04_speed_pla()];
        let mut b = Builder::new(&job, &config, &settings).unwrap();
        // Hand-made statistics: 100 mm outer wall at 50 mm/s (2 s), 100 mm infill at 100 mm/s
        // (1 s), a 10 mm travel at 200 mm/s (0.05 s): estimator total 3.05 s.
        b.stats_mut().add(
            &Segment::new([0.; 3], [100., 0., 0.2], [0.45, 0.2], 2, 0, 50., 100.),
            0,
        );
        b.stats_mut().add(
            &Segment::new([0.; 3], [100., 0., 0.2], [0.45, 0.2], 5, 0, 100., 30.),
            0,
        );
        b.stats_mut().add(
            &Segment::new([0.; 3], [10., 0., 0.2], [0.08, 0.04], TRAVEL, 0, 200., 0.),
            0,
        );
        // The exporter says 6.1 s: every feature is scaled by 2 and the shares still sum to 100.
        let p = b.finish_with(job, Some(6.1)).unwrap();
        let (outer, outer_pct) = feature_time(&p, 2);
        let (infill, infill_pct) = feature_time(&p, 5);
        let (travel, travel_pct) = feature_time(&p, TRAVEL as usize);
        assert!((outer - 4.).abs() < 1e-4, "{outer}");
        assert!((infill - 2.).abs() < 1e-4, "{infill}");
        assert!((travel - 0.1).abs() < 1e-4, "{travel}");
        assert!((outer + infill + travel - 6.1).abs() < 1e-4);
        assert!((outer_pct + infill_pct + travel_pct - 100.).abs() < 1e-4);
        assert!((tool_time(&p, 0).0 - 6.).abs() < 1e-4);
    }
}
