// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/slic3r/GUI/GLCanvas3D.cpp :: GLCanvas3D::LayersEditing::render_overlay, GLCanvas3D::LayersEditing::render_profile, GLCanvas3D::LayersEditing::get_tooltip
// Changes: An egui card (top right of the view) instead of the ImGui overlay and the thickness bar: the profile graph with its colour strip sits in the card and is dragged sideways; Adaptive / Smooth / Reset as upstream.
// Upstream attribution (GLCanvas3D.cpp):
// Copyright (c) Prusa Research 2015 - 2023 Enrico Turri @enricoturri1966, Vojtěch Bubník @bubnikv, Lukáš Matěna @lukasmatena, Oleksandra Iushchenko @YuSanka
//
// PrusaSlicer is released under the terms of the AGPLv3 or higher
//

//! The variable layer height card and the App side of the object tools: switching them on
//! and off, the per-frame view (gradient overlay, band, ears) and the edits.

use super::widgets::{self, Glyph, NumberSpec};
use crate::app::App;
use crate::layer_edit::{self as le, Mode};
use crate::scene::{Object, paint::PaintOverlay};
use crate::theme;
use crate::viewport::{ObjectTool, ObjectToolView};
use eframe::egui::{self, Align2, Color32, Pos2, Rect, Sense, Stroke, Vec2};
use mica_core::layer_profile::{self as lp, SlicingParams};
use std::sync::Arc;

const CARD_W: f32 = 300.;
const FIELD_W: f32 = 64.;
const FIELD_H: f32 = 26.;
const LABEL_W: f32 = 96.;
const GRAPH_H: f32 = 240.;

/// A label, a number field and (optionally) a slider track after it, on one row.
pub(super) fn number_row(
    ui: &mut egui::Ui,
    label: &str,
    id: &str,
    value: f64,
    spec: &NumberSpec<'_>,
    tip: &str,
    slider: Option<(f64, f64)>,
) -> Option<f64> {
    let p = theme::of(ui);
    let (row, _) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), FIELD_H + 2.),
        Sense::hover(),
    );
    ui.painter().text(
        Pos2::new(row.left(), row.center().y),
        Align2::LEFT_CENTER,
        label,
        theme::sans(12.),
        p.muted_alt,
    );
    let field = Rect::from_min_size(
        Pos2::new(row.left() + LABEL_W, row.center().y - FIELD_H / 2.),
        Vec2::new(FIELD_W, FIELD_H),
    );
    let mut out = widgets::number_with(
        ui,
        ui.make_persistent_id(id),
        field,
        value,
        spec,
        true,
        false,
    );
    ui.interact(field, ui.make_persistent_id((id, "tip")), Sense::hover())
        .on_hover_text(tip);
    if let Some((lo, hi)) = slider {
        let track = Rect::from_min_max(
            Pos2::new(field.right() + 12., row.center().y - 8.),
            Pos2::new(row.right() - 4., row.center().y + 8.),
        );
        let r = ui
            .interact(
                track,
                ui.make_persistent_id((id, "slider")),
                Sense::click_and_drag(),
            )
            .on_hover_text(tip);
        let t = ((value - lo) / (hi - lo)).clamp(0., 1.) as f32;
        let y = track.center().y;
        ui.painter().line_segment(
            [Pos2::new(track.left(), y), Pos2::new(track.right(), y)],
            Stroke::new(2., p.field_border),
        );
        let knob = track.left() + t * track.width();
        ui.painter().line_segment(
            [Pos2::new(track.left(), y), Pos2::new(knob, y)],
            Stroke::new(2., p.accent),
        );
        ui.painter()
            .circle_filled(Pos2::new(knob, y), 6., p.accent_hi);
        if (r.dragged() || r.clicked())
            && let Some(pos) = r.interact_pointer_pos()
        {
            let t = f64::from(((pos.x - track.left()) / track.width()).clamp(0., 1.));
            out = Some(lo + t * (hi - lo));
        }
    }
    out
}

/// A small text line in the card's muted colour.
pub(super) fn note(ui: &mut egui::Ui, text: &str, color: Color32) {
    ui.add(
        egui::Label::new(
            egui::RichText::new(text)
                .font(theme::sans(11.5))
                .color(color),
        )
        .wrap(),
    );
}

impl App {
    /// The slicing parameters of current-plate object `i`.
    pub(crate) fn layer_params(&self, i: usize) -> Option<SlicingParams> {
        let c = self.plate_config(self.plates.current).ok()?;
        let o = self.plates.current().objects.get(i)?;
        Some(le::params(&c, o))
    }

    /// An object tool was switched on or off from the view.
    pub(crate) fn object_tool_changed(&mut self, tool: Option<ObjectTool>) {
        self.layer_overlay = None;
        self.ears_loose = None;
        self.paint_session = None;
        match tool {
            Some(ObjectTool::LayerHeights) => {
                self.status = "Layer heights: drag up on the model for thicker layers, down for thinner · Shift: back to normal · Alt: smooth".into();
            }
            Some(ObjectTool::BrimEars) => {
                self.status =
                    "Brim ears: click the model to place an ear, click an ear to remove it".into();
            }
            None => {}
        }
    }

    /// Alt-scroll in an object tool: the band width, or the size of new ears.
    pub(crate) fn object_tool_size(&mut self, delta: f32) {
        match self.viewport.object_tool {
            Some(ObjectTool::LayerHeights) => {
                let w = self.layer_edit.band_width;
                self.layer_edit
                    .set_band_width(w * (1. + delta.clamp(-0.5, 0.5)));
            }
            Some(ObjectTool::BrimEars) => {
                let d = self.brim_ear.diameter_for(self.effective_config());
                self.brim_ear
                    .set_diameter(d * (1. + delta.clamp(-0.5, 0.5)));
            }
            None => {}
        }
    }

    /// What the view needs this frame for the active object tool; the layer height tool's
    /// gradient replaces the object's paint overlays in `overlays`.
    pub(crate) fn object_tool_view(
        &mut self,
        objects: &[Object],
        selected: Option<usize>,
        overlays: &mut Vec<PaintOverlay>,
    ) -> Option<ObjectToolView> {
        let tool = self.viewport.object_tool?;
        let i = selected?;
        let o = objects.get(i)?;
        let mut view = ObjectToolView {
            tool,
            band_width: self.layer_edit.band_width,
            ear_diameter: self.brim_ear.diameter_for(self.effective_config()),
            loose_ears: Vec::new(),
        };
        match tool {
            ObjectTool::LayerHeights => {
                let p = self.layer_params(i)?;
                let profile = le::profile(&p, o);
                let key = le::overlay_key(o, &profile, &p);
                if self.layer_overlay.as_ref().is_none_or(|x| x.key != key) {
                    self.layer_overlay = Some(PaintOverlay {
                        object: o.id,
                        key,
                        vertices: Arc::new(le::overlay_vertices(&p, o, &profile)),
                    });
                }
                overlays.retain(|x| x.object != o.id);
                overlays.extend(self.layer_overlay.clone());
            }
            ObjectTool::BrimEars => {
                use std::hash::{Hash, Hasher};
                let mut h = std::collections::hash_map::DefaultHasher::new();
                o.id.hash(&mut h);
                for v in o.transform().to_cols_array() {
                    v.to_bits().hash(&mut h);
                }
                for e in o.brim_ears.iter().flatten() {
                    e.to_bits().hash(&mut h);
                }
                let key = h.finish();
                if self.ears_loose.as_ref().is_none_or(|(k, _)| *k != key) {
                    let layer = crate::brim_ears::first_layer(o);
                    self.ears_loose = Some((key, crate::brim_ears::loose(o, &layer)));
                }
                view.loose_ears = self
                    .ears_loose
                    .as_ref()
                    .map(|(_, l)| l.clone())
                    .unwrap_or_default();
            }
        }
        Some(view)
    }

    /// One layer height drag step on the selected object (`z` above its bottom).
    pub(crate) fn layer_edit_step(&mut self, z: f64, amount: f64, shift: bool, alt: bool) {
        let Some(i) = self.selected else { return };
        if self.plate_locked() {
            return;
        }
        let Some(p) = self.layer_params(i) else {
            return;
        };
        let mode = Mode::of(amount, shift, alt);
        let band = self.layer_edit.band_width;
        let mut o = self.plates.current().objects[i].clone();
        if le::edit(&p, &mut o, z, amount, band, mode) {
            self.begin_gesture("Layer heights");
            self.plates.current_mut().objects[i] = o;
            self.sync_instances(i);
            self.changed();
        }
    }

    /// Applies a whole-profile change (Adaptive, Smooth, Reset) as one undo step.
    fn layer_profile_command(&mut self, label: &str, f: impl FnOnce(&SlicingParams, &mut Object)) {
        let Some(i) = self.selected else { return };
        if self.plate_locked() {
            return;
        }
        let Some(p) = self.layer_params(i) else {
            return;
        };
        let mut o = self.plates.current().objects[i].clone();
        f(&p, &mut o);
        if o != self.plates.current().objects[i] {
            self.record(label);
            self.plates.current_mut().objects[i] = o;
            self.sync_instances(i);
            self.changed();
        }
    }

    pub(crate) fn layer_adaptive(&mut self) {
        let q = self.layer_edit.quality;
        self.layer_profile_command("Adaptive layer heights", |p, o| le::adaptive(p, o, q));
    }
    pub(crate) fn layer_smooth(&mut self) {
        let (r, keep) = (self.layer_edit.smooth_radius, self.layer_edit.keep_min);
        self.layer_profile_command("Smooth layer heights", |p, o| le::smooth(p, o, r, keep));
    }
    pub(crate) fn layer_reset(&mut self) {
        self.layer_profile_command("Reset layer heights", |_, o| o.layer_profile.clear());
    }

    /// The variable layer height card for the selected object.
    pub(crate) fn layer_window(&mut self, ctx: &egui::Context) {
        let Some(i) = self.selected else { return };
        let Some(o) = self.plates.current().objects.get(i) else {
            return;
        };
        let Some(p) = self.layer_params(i) else {
            return;
        };
        let name = std::path::Path::new(&o.name)
            .file_stem()
            .map_or(o.name.clone(), |s| s.to_string_lossy().into_owned());
        let profile = le::profile(&p, o);
        let own = !o.layer_profile.is_empty();
        let fits = le::fits(&p, o);
        let (layers, thinnest, thickest) = le::summary(&p, &profile);
        let hover_z = self.viewport_layer_z(i);
        let mut settings = self.layer_edit.clone();
        let (mut adaptive, mut smooth, mut reset, mut leave) = (false, false, false, false);
        let mut graph_edit: Option<(f64, f64, bool, bool)> = None;
        let mut graph_state: Option<bool> = None;
        let viewport = self.viewport_rect;
        egui::Area::new(egui::Id::unique("layer-heights-card"))
            .order(egui::Order::Middle)
            .fixed_pos(viewport.right_top() + egui::vec2(-14. - CARD_W, 14.))
            .show(ctx, |ui| {
                let pal = theme::of(ui);
                theme::overlay_frame(ui)
                    .inner_margin(egui::Margin::symmetric(12, 10))
                    .show(ui, |ui| {
                        ui.set_width(CARD_W - 24.);
                        ui.spacing_mut().item_spacing = Vec2::new(4., 4.);
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(widgets::spaced_upper(&format!(
                                    "{name} · Layer heights"
                                )))
                                .font(theme::semibold(11.))
                                .color(pal.muted),
                            );
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if widgets::icon_button_sized(ui, Glyph::Close, "Leave the tool (Esc)", true, 22.)
                                    .clicked()
                                {
                                    leave = true;
                                }
                            });
                        });
                        ui.add_space(4.);
                        // The graph: Z up, layer height to the right, coloured like the model.
                        let (rect, response) = ui.allocate_exact_size(
                            Vec2::new(ui.available_width(), GRAPH_H),
                            Sense::click_and_drag(),
                        );
                        let x_max = 1.12 * p.max_layer_height;
                        let to_x = |h: f64| rect.left() + (h / x_max) as f32 * rect.width();
                        let to_y = |z: f64| {
                            rect.bottom() - (z / p.object_height.max(1e-6)) as f32 * rect.height()
                        };
                        let painter = ui.painter_at(rect.expand(2.));
                        painter.rect_filled(rect, 4., pal.field);
                        // Colour strip on the left, row by row.
                        let strip = Rect::from_min_max(rect.left_top(), Pos2::new(rect.left() + 10., rect.bottom()));
                        let rows = rect.height().max(1.) as usize;
                        for r in 0..rows {
                            let z = (r as f64 + 0.5) / rows as f64 * p.object_height;
                            let h = lp::height_at(&profile, z).unwrap_or(p.layer_height);
                            let c = le::color(&p, h);
                            let y = rect.bottom() - r as f32 - 1.;
                            painter.rect_filled(
                                Rect::from_min_max(Pos2::new(strip.left(), y), Pos2::new(strip.right(), y + 1.)),
                                0.,
                                Color32::from_rgb((c[0] * 255.) as u8, (c[1] * 255.) as u8, (c[2] * 255.) as u8),
                            );
                        }
                        // Min, base and max heights.
                        for (h, strong) in [
                            (p.min_layer_height, false),
                            (p.layer_height, true),
                            (p.max_layer_height, false),
                        ] {
                            let x = to_x(h);
                            painter.line_segment(
                                [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
                                Stroke::new(1., if strong { pal.muted } else { pal.border_soft }),
                            );
                            painter.text(
                                Pos2::new(x, rect.bottom() - 3.),
                                Align2::CENTER_BOTTOM,
                                widgets::format_number(h, 2),
                                theme::sans(10.),
                                pal.muted,
                            );
                        }
                        painter.text(
                            Pos2::new(rect.right() - 4., rect.top() + 3.),
                            Align2::RIGHT_TOP,
                            format!("{} mm", widgets::format_number(p.object_height, 1)),
                            theme::sans(10.),
                            pal.muted,
                        );
                        // The edited band, under the pointer here or on the model.
                        let pointer_z = response.hover_pos().map(|q| {
                            f64::from((rect.bottom() - q.y) / rect.height()) * p.object_height
                        });
                        let band_z = self.layer_graph_z.or(pointer_z).or(hover_z);
                        if let Some(z) = band_z {
                            let (a, b) = (
                                to_y((z - f64::from(settings.band_width) / 2.).max(0.)),
                                to_y((z + f64::from(settings.band_width) / 2.).min(p.object_height)),
                            );
                            painter.rect_filled(
                                Rect::from_min_max(Pos2::new(rect.left(), b), Pos2::new(rect.right(), a)),
                                0.,
                                theme::ACCENT.gamma_multiply(0.14),
                            );
                            if let Some(h) = lp::height_at(&profile, z) {
                                painter.text(
                                    Pos2::new(rect.right() - 4., to_y(z) - 2.),
                                    Align2::RIGHT_BOTTOM,
                                    format!(
                                        "{} mm at {}",
                                        widgets::format_number(h, 3),
                                        widgets::format_number(z, 1)
                                    ),
                                    theme::sans(10.5),
                                    pal.text,
                                );
                            }
                        }
                        // The profile.
                        let line: Vec<Pos2> = profile
                            .chunks(2)
                            .filter(|c| c.len() == 2)
                            .map(|c| Pos2::new(to_x(c[1]), to_y(c[0])))
                            .collect();
                        if line.len() > 1 {
                            painter.add(egui::Shape::line(line, Stroke::new(2., theme::ACCENT_BRIGHT)));
                        }
                        // Dragging sideways in the graph moves the curve under the pointer.
                        let mods = ui.input(|i| i.modifiers);
                        if response.drag_started()
                            && let Some(q) = response.interact_pointer_pos()
                        {
                            graph_state = Some(true);
                            self.layer_graph_z = Some(
                                f64::from((rect.bottom() - q.y) / rect.height()).clamp(0., 1.)
                                    * p.object_height,
                            );
                        }
                        if response.dragged()
                            && let Some(z) = self.layer_graph_z
                        {
                            let dx = f64::from(response.drag_delta().x);
                            if dx != 0. {
                                let mm = dx * x_max / f64::from(rect.width());
                                graph_edit = Some((z, mm, mods.shift, mods.alt));
                            }
                        }
                        if response.drag_stopped() {
                            graph_state = Some(false);
                            self.layer_graph_z = None;
                        }
                        response.on_hover_text(
                            "Drag right for thicker layers, left for thinner · Shift: back to normal · Alt: smooth",
                        );
                        ui.add_space(4.);
                        let summary = format!(
                            "{layers} layers · {}–{} mm{}",
                            widgets::format_number(thinnest, 2),
                            widgets::format_number(thickest, 2),
                            if own { "" } else { " · not changed yet" }
                        );
                        note(ui, &summary, pal.muted_alt);
                        if !fits {
                            note(
                                ui,
                                "These layer heights were made for another object height (scaled or turned), so the slicer ignores them. Reset or press Adaptive to start again.",
                                theme::WARNING,
                            );
                        }
                        ui.add_space(4.);
                        if let Some(v) = number_row(
                            ui,
                            "Band width",
                            "vlh-band",
                            f64::from(settings.band_width),
                            &NumberSpec {
                                unit: "mm",
                                decimals: 1,
                                min: f64::from(le::BAND_MIN),
                                max: f64::from(le::BAND_MAX),
                                speed: 0.05,
                            },
                            "How tall a stretch one drag changes · Alt-scroll in the view changes it too",
                            Some((f64::from(le::BAND_MIN), f64::from(le::BAND_MAX))),
                        ) {
                            settings.set_band_width(v as f32);
                        }
                        ui.add_space(2.);
                        ui.horizontal(|ui| {
                            if widgets::styled_button(ui, "Adaptive", true, false, 26.)
                                .on_hover_text("Thin layers on slopes and curves, thick ones on walls (cusp height), at the detail below")
                                .clicked()
                            {
                                adaptive = true;
                            }
                        });
                        if let Some(v) = number_row(
                            ui,
                            "Detail ↔ speed",
                            "vlh-quality",
                            f64::from(settings.quality),
                            &NumberSpec {
                                unit: "",
                                decimals: 2,
                                min: 0.,
                                max: 1.,
                                speed: 0.01,
                            },
                            "0 = finest detail, 1 = fastest print (PrusaSlicer's Quality / Speed)",
                            Some((0., 1.)),
                        ) {
                            settings.quality = v.clamp(0., 1.) as f32;
                        }
                        ui.add_space(2.);
                        ui.horizontal(|ui| {
                            if widgets::styled_button(ui, "Smooth", true, false, 26.)
                                .on_hover_text("Even out sudden changes over the radius below")
                                .clicked()
                            {
                                smooth = true;
                            }
                            ui.add_space(8.);
                            let (r, _) = ui.allocate_exact_size(Vec2::new(34., 20.), Sense::hover());
                            if let Some(v) = widgets::toggle_at(
                                ui,
                                ui.make_persistent_id("vlh-keep-min"),
                                r,
                                settings.keep_min,
                                true,
                            ) {
                                settings.keep_min = v;
                            }
                            ui.label(
                                egui::RichText::new("Keep thin layers thin")
                                    .font(theme::sans(12.))
                                    .color(pal.label),
                            );
                        });
                        if let Some(v) = number_row(
                            ui,
                            "Smooth radius",
                            "vlh-radius",
                            f64::from(settings.smooth_radius),
                            &NumberSpec {
                                unit: "",
                                decimals: 0,
                                min: 1.,
                                max: 10.,
                                speed: 0.05,
                            },
                            "How many profile points each layer is averaged over",
                            Some((1., 10.)),
                        ) {
                            settings.smooth_radius = (v.round() as u32).clamp(1, 10);
                        }
                        ui.add_space(4.);
                        ui.horizontal(|ui| {
                            if widgets::styled_button(ui, "Reset", own, false, 26.)
                                .on_hover_text("Back to the plate's layer height (and height ranges)")
                                .clicked()
                            {
                                reset = true;
                            }
                        });
                        ui.add_space(2.);
                        note(
                            ui,
                            "Drag on the model: up = thicker, down = thinner · Shift: back to normal · Alt: smooth",
                            pal.muted,
                        );
                    });
            });
        if settings != self.layer_edit {
            self.layer_edit = settings;
        }
        match graph_state {
            Some(true) => self.begin_gesture("Layer heights"),
            Some(false) => self.end_gesture(),
            None => {}
        }
        if let Some((z, mm, shift, alt)) = graph_edit {
            self.layer_edit_step(z, mm, shift, alt);
        }
        if adaptive {
            self.layer_adaptive();
        }
        if smooth {
            self.layer_smooth();
        }
        if reset {
            self.layer_reset();
        }
        if leave {
            self.viewport.object_tool = None;
            self.object_tool_changed(None);
        }
    }

    /// Height above the bottom of object `i` the view's layer tool points at, if any.
    fn viewport_layer_z(&self, i: usize) -> Option<f64> {
        let o = self.plates.current().objects.get(i)?;
        let bottom = o.bounds().0.z;
        let z = self.viewport.layer_tool_z(bottom)?;
        Some(f64::from(z - bottom))
    }
}
