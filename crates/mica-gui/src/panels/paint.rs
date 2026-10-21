// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The painting tool card (top right of the 3D view while a painting tool is active): brush
//! shape, size, the mode (enforce / block, or the tool swatches for colour painting), Erase,
//! Clear all, and for support painting the overhang highlight with "Paint overhangs as
//! enforcers". The card edits `App::paint` (the brush settings); strokes come from the viewport.

use super::widgets::{self, Glyph, NumberSpec};
use crate::app::App;
use crate::paint::{BrushKind, RADIUS_MAX, RADIUS_MIN, SPREAD_MAX, SPREAD_SLIDER_MAX};
use crate::theme;
use eframe::egui::{self, Align2, Pos2, Rect, Sense, Stroke, Vec2};
use mica_core::painting::PaintKind;

const CARD_W: f32 = 300.;
const FIELD_W: f32 = 64.;
const FIELD_H: f32 = 26.;
const LABEL_W: f32 = 74.;

impl App {
    /// The painting card for the selected object and tool `kind`.
    pub(crate) fn paint_window(&mut self, ctx: &egui::Context, kind: PaintKind) {
        let Some(i) = self.selected else { return };
        let Some(o) = self.plates.current().objects.get(i) else {
            return;
        };
        let name = std::path::Path::new(&o.name)
            .file_stem()
            .map_or(o.name.clone(), |s| s.to_string_lossy().into_owned());
        let painted = !o.painting.get(kind).is_empty();
        // Physical tools, then the mixed colors (`mixes`).
        let choices = self.tool_choices();
        let mut settings = self.paint.clone();
        let mut clear = false;
        let mut overhangs = false;
        let mut leave = false;
        let viewport = self.viewport_rect;
        let title = format!("{name} · {}", crate::paint::tool_name(kind));
        egui::Area::new(egui::Id::unique("paint-card"))
            .order(egui::Order::Middle)
            .fixed_pos(viewport.right_top() + egui::vec2(-14. - CARD_W, 14.))
            .show(ctx, |ui| {
                let p = theme::of(ui);
                theme::overlay_frame(ui)
                    .inner_margin(egui::Margin::symmetric(12, 10))
                    .show(ui, |ui| {
                        ui.set_width(CARD_W - 24.);
                        ui.spacing_mut().item_spacing = Vec2::new(4., 4.);
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(widgets::spaced_upper(&title))
                                    .font(theme::semibold(11.))
                                    .color(p.muted),
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
                        // Brush shape.
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 6.;
                            for b in BrushKind::ALL {
                                let tip = match b {
                                    BrushKind::Circle => "Circle: paints what the pointer covers (front faces)",
                                    BrushKind::Sphere => "Sphere: paints around the hit point, through to the back",
                                    BrushKind::Fill => "Fill: the connected surface within the angle, up to the max spread",
                                    BrushKind::Height => "Height: a horizontal band of the model around the hit",
                                };
                                if widgets::chip(ui, None, b.label(), settings.brush == b, true)
                                    .on_hover_text(tip)
                                    .clicked()
                                {
                                    settings.brush = b;
                                }
                            }
                        });
                        ui.add_space(2.);
                        // Size / angle / height row.
                        let (label, value, spec, tip) = match settings.brush {
                            BrushKind::Fill => (
                                "Angle °",
                                settings.fill_angle,
                                NumberSpec {
                                    unit: "",
                                    decimals: 0,
                                    min: 1.,
                                    max: 90.,
                                    speed: 0.5,
                                },
                                "Neighboring facets within this angle are filled together",
                            ),
                            BrushKind::Height => (
                                "Height mm",
                                settings.height,
                                NumberSpec {
                                    unit: "",
                                    decimals: 1,
                                    min: 0.1,
                                    max: 200.,
                                    speed: 0.1,
                                },
                                "Height of the band around the hit",
                            ),
                            _ => (
                                "Size mm",
                                settings.radius,
                                NumberSpec {
                                    unit: "",
                                    decimals: 1,
                                    min: f64::from(RADIUS_MIN),
                                    max: f64::from(RADIUS_MAX),
                                    speed: 0.1,
                                },
                                "Brush radius · Alt-scroll in the view changes it too",
                            ),
                        };
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
                        if let Some(v) = widgets::number_with(
                            ui,
                            ui.make_persistent_id(("paint-size", settings.brush as u8)),
                            field,
                            f64::from(value),
                            &spec,
                            true,
                            false,
                        ) {
                            match settings.brush {
                                BrushKind::Fill => settings.fill_angle = v as f32,
                                BrushKind::Height => settings.height = v as f32,
                                _ => settings.set_radius(v as f32),
                            }
                        }
                        ui.interact(field, ui.make_persistent_id("paint-size-tip"), Sense::hover())
                            .on_hover_text(tip);
                        // A slider track for the radius next to the field.
                        if matches!(settings.brush, BrushKind::Circle | BrushKind::Sphere) {
                            let track = Rect::from_min_max(
                                Pos2::new(field.right() + 12., row.center().y - 8.),
                                Pos2::new(row.right() - 4., row.center().y + 8.),
                            );
                            let r = ui.interact(
                                track,
                                ui.make_persistent_id("paint-size-slider"),
                                Sense::click_and_drag(),
                            );
                            let t = ((settings.radius - RADIUS_MIN) / (RADIUS_MAX - RADIUS_MIN))
                                .clamp(0., 1.);
                            let line_y = track.center().y;
                            ui.painter().line_segment(
                                [Pos2::new(track.left(), line_y), Pos2::new(track.right(), line_y)],
                                Stroke::new(2., p.field_border),
                            );
                            let knob_x = track.left() + t * track.width();
                            ui.painter().line_segment(
                                [Pos2::new(track.left(), line_y), Pos2::new(knob_x, line_y)],
                                Stroke::new(2., p.accent),
                            );
                            ui.painter().circle_filled(Pos2::new(knob_x, line_y), 6., p.accent_hi);
                            if (r.dragged() || r.clicked())
                                && let Some(pos) = r.interact_pointer_pos()
                            {
                                let t = ((pos.x - track.left()) / track.width()).clamp(0., 1.);
                                settings.set_radius(RADIUS_MIN + t * (RADIUS_MAX - RADIUS_MIN));
                            }
                        }
                        // Fill: how far it may spread from the click, along the surface.
                        if settings.brush == BrushKind::Fill {
                            let (row, _) = ui.allocate_exact_size(
                                Vec2::new(ui.available_width(), FIELD_H + 2.),
                                Sense::hover(),
                            );
                            ui.painter().text(
                                Pos2::new(row.left(), row.center().y),
                                Align2::LEFT_CENTER,
                                "Max spread",
                                theme::sans(12.),
                                p.muted_alt,
                            );
                            let field = Rect::from_min_size(
                                Pos2::new(row.left() + LABEL_W, row.center().y - FIELD_H / 2.),
                                Vec2::new(FIELD_W, FIELD_H),
                            );
                            if let Some(v) = widgets::number_with(
                                ui,
                                ui.make_persistent_id("paint-fill-spread"),
                                field,
                                f64::from(settings.fill_spread),
                                &NumberSpec {
                                    unit: "mm",
                                    decimals: 1,
                                    min: 0.,
                                    max: f64::from(SPREAD_MAX),
                                    speed: 0.2,
                                },
                                true,
                                false,
                            ) {
                                settings.set_fill_spread(v as f32);
                            }
                            ui.interact(field, ui.make_persistent_id("paint-fill-spread-tip"), Sense::hover())
                                .on_hover_text(
                                    "How far a fill may spread from the click, measured along the surface · 0 = no limit (it stops only at sharp edges)",
                                );
                            let track = Rect::from_min_max(
                                Pos2::new(field.right() + 12., row.center().y - 8.),
                                Pos2::new(row.right() - 4., row.center().y + 8.),
                            );
                            let r = ui.interact(
                                track,
                                ui.make_persistent_id("paint-fill-spread-slider"),
                                Sense::click_and_drag(),
                            );
                            let t = (settings.fill_spread / SPREAD_SLIDER_MAX).clamp(0., 1.);
                            let line_y = track.center().y;
                            ui.painter().line_segment(
                                [Pos2::new(track.left(), line_y), Pos2::new(track.right(), line_y)],
                                Stroke::new(2., p.field_border),
                            );
                            let knob_x = track.left() + t * track.width();
                            ui.painter().line_segment(
                                [Pos2::new(track.left(), line_y), Pos2::new(knob_x, line_y)],
                                Stroke::new(2., p.accent),
                            );
                            ui.painter().circle_filled(Pos2::new(knob_x, line_y), 6., p.accent_hi);
                            if settings.fill_spread <= 0. {
                                ui.painter().text(
                                    Pos2::new(track.right(), line_y - 9.),
                                    Align2::RIGHT_BOTTOM,
                                    "no limit",
                                    theme::sans(10.),
                                    p.muted,
                                );
                            }
                            if (r.dragged() || r.clicked())
                                && let Some(pos) = r.interact_pointer_pos()
                            {
                                let t = ((pos.x - track.left()) / track.width()).clamp(0., 1.);
                                settings.set_fill_spread((t * SPREAD_SLIDER_MAX * 2.).round() / 2.);
                            }
                            r.on_hover_text("Max spread: drag to the left end for no limit");
                        }
                        ui.add_space(2.);
                        // Mode.
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing = Vec2::new(6., 6.);
                            match kind {
                                PaintKind::Supports | PaintKind::Seam => {
                                    let (enforce, block) = if kind == PaintKind::Supports {
                                        ("Enforce", "Block")
                                    } else {
                                        ("Seam here", "No seam")
                                    };
                                    if widgets::chip(ui, None, enforce, settings.enforce && !settings.erase, true)
                                        .on_hover_text(if kind == PaintKind::Supports {
                                            "Painted faces get support"
                                        } else {
                                            "The seam prefers painted faces"
                                        })
                                        .clicked()
                                    {
                                        settings.enforce = true;
                                        settings.erase = false;
                                    }
                                    if widgets::chip(ui, None, block, !settings.enforce && !settings.erase, true)
                                        .on_hover_text(if kind == PaintKind::Supports {
                                            "Painted faces never get support"
                                        } else {
                                            "The seam avoids painted faces"
                                        })
                                        .clicked()
                                    {
                                        settings.enforce = false;
                                        settings.erase = false;
                                    }
                                }
                                PaintKind::Material => {
                                    for choice in &choices {
                                        let (t, c) = (choice.tool, choice.colour);
                                        let on = settings.tool == t && !settings.erase;
                                        let (rect, r) =
                                            ui.allocate_exact_size(Vec2::new(44., 32.), Sense::click());
                                        ui.painter().rect_filled(
                                            rect,
                                            7.,
                                            if r.hovered() { p.hover } else { p.raised },
                                        );
                                        ui.painter().rect_stroke(
                                            rect,
                                            7.,
                                            Stroke::new(1., if on { p.accent } else { p.border_raised }),
                                            egui::StrokeKind::Inside,
                                        );
                                        let sw = Rect::from_center_size(
                                            Pos2::new(rect.left() + 13., rect.center().y),
                                            Vec2::splat(12.),
                                        );
                                        ui.painter().rect_filled(
                                            sw,
                                            3.,
                                            egui::Color32::from_rgb(c[0], c[1], c[2]),
                                        );
                                        ui.painter().text(
                                            Pos2::new(rect.right() - 8., rect.center().y),
                                            Align2::RIGHT_CENTER,
                                            &choice.short,
                                            theme::mono(12.),
                                            if on { p.accent_hi } else { p.text },
                                        );
                                        if r.on_hover_text(crate::mixes::paint_tip(choice)).clicked() {
                                            settings.tool = t;
                                            settings.erase = false;
                                        }
                                    }
                                }
                                PaintKind::Fuzzy => {
                                    ui.label(
                                        egui::RichText::new("Painted faces get fuzzy skin")
                                            .font(theme::sans(12.))
                                            .color(p.muted_alt),
                                    );
                                }
                                // The slots follow below the mode row.
                                PaintKind::Texture => {}
                            }
                            if widgets::chip(ui, Some(Glyph::Eraser), "Erase", settings.erase, true)
                                .on_hover_text("Remove paint under the brush · or hold Shift while painting")
                                .clicked()
                            {
                                settings.erase = !settings.erase;
                            }
                        });
                        if kind == PaintKind::Supports {
                            ui.add_space(4.);
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 8.;
                                let (r, _) = ui.allocate_exact_size(Vec2::new(34., 20.), Sense::hover());
                                if let Some(v) = widgets::toggle_at(
                                    ui,
                                    ui.make_persistent_id("paint-overhangs-toggle"),
                                    r,
                                    settings.show_overhangs,
                                    true,
                                ) {
                                    settings.show_overhangs = v;
                                }
                                ui.label(
                                    egui::RichText::new("Show overhangs steeper than")
                                        .font(theme::sans(12.))
                                        .color(p.label),
                                );
                                let (ar, _) = ui.allocate_exact_size(Vec2::new(52., FIELD_H), Sense::hover());
                                if let Some(v) = widgets::number_with(
                                    ui,
                                    ui.make_persistent_id("paint-overhang-angle"),
                                    ar,
                                    f64::from(settings.overhang_angle),
                                    &NumberSpec {
                                        unit: "°",
                                        decimals: 0,
                                        min: 1.,
                                        max: 89.,
                                        speed: 0.5,
                                    },
                                    settings.show_overhangs,
                                    false,
                                ) {
                                    settings.overhang_angle = v as f32;
                                }
                            });
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 8.;
                                let (r, _) = ui.allocate_exact_size(Vec2::new(34., 20.), Sense::hover());
                                if let Some(v) = widgets::toggle_at(
                                    ui,
                                    ui.make_persistent_id("paint-only-overhangs"),
                                    r,
                                    settings.only_overhangs,
                                    settings.show_overhangs,
                                ) {
                                    settings.only_overhangs = v;
                                }
                                ui.label(
                                    egui::RichText::new("Paint on overhangs only")
                                        .font(theme::sans(12.))
                                        .color(if settings.show_overhangs { p.label } else { p.muted }),
                                );
                            });
                            if widgets::styled_button(ui, "Paint overhangs as enforcers", settings.show_overhangs, false, 26.)
                                .on_hover_text("Every highlighted facet becomes a support enforcer (one undo step)")
                                .clicked()
                            {
                                overhangs = true;
                            }
                        }
                        if kind == PaintKind::Texture && !settings.erase {
                            ui.add_space(4.);
                            self.texture_paint_slots(ui, &mut settings.texture_slot);
                        }
                        ui.add_space(6.);
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 8.;
                            if widgets::styled_button(ui, "Clear all", painted, false, 26.)
                                .on_hover_text("Remove this tool's paint from the object")
                                .clicked()
                            {
                                clear = true;
                            }
                        });
                        ui.add_space(4.);
                        ui.label(
                            egui::RichText::new(
                                "Left-drag to paint · Shift-drag to erase · Alt-scroll to change brush size",
                            )
                            .font(theme::sans(11.))
                            .color(p.muted),
                        );
                    });
            });
        if settings != self.paint {
            self.paint = settings;
        }
        if clear {
            self.clear_painting();
        }
        if overhangs {
            self.paint_overhangs();
        }
        if leave {
            self.viewport.paint = None;
            self.paint_session = None;
        }
    }

    /// The painting card while no object is selected (the selection was cleared with the
    /// tool open): what to do instead of an empty tool.
    pub(crate) fn paint_pick_card(&mut self, ctx: &egui::Context, kind: PaintKind) {
        let viewport = self.viewport_rect;
        let empty = self.plates.current().objects.is_empty();
        let title = crate::paint::tool_name(kind).to_string();
        let mut leave = false;
        let mut add = false;
        egui::Area::new(egui::Id::unique("paint-pick-card"))
            .order(egui::Order::Middle)
            .fixed_pos(viewport.right_top() + egui::vec2(-14. - CARD_W, 14.))
            .show(ctx, |ui| {
                let p = theme::of(ui);
                theme::overlay_frame(ui)
                    .inner_margin(egui::Margin::symmetric(12, 10))
                    .show(ui, |ui| {
                        ui.set_width(CARD_W - 24.);
                        ui.spacing_mut().item_spacing = Vec2::new(4., 6.);
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(widgets::spaced_upper(&title))
                                    .font(theme::semibold(11.))
                                    .color(p.muted),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if widgets::icon_button_sized(
                                        ui,
                                        Glyph::Close,
                                        "Leave the tool (Esc)",
                                        true,
                                        22.,
                                    )
                                    .clicked()
                                    {
                                        leave = true;
                                    }
                                },
                            );
                        });
                        let text = if empty {
                            "Add a model to paint on."
                        } else {
                            "Select a model to paint: click one in the 3D view or the object list."
                        };
                        ui.label(
                            egui::RichText::new(text)
                                .font(theme::sans(12.5))
                                .color(p.text),
                        );
                        if empty && widgets::link(ui, "Add model…").clicked() {
                            add = true;
                        }
                    });
            });
        if add {
            self.open_dialog(ctx);
        }
        if leave {
            self.viewport.paint = None;
            self.paint_session = None;
        }
    }
}
