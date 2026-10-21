// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: orca@8500fcdcca src/slic3r/GUI/Gizmos/GLGizmoMeshBoolean.cpp :: GLGizmoMeshBoolean::on_render_input_window
// Source: prusaslicer@b028299c77 src/slic3r/GUI/Gizmos/GLGizmoSimplify.cpp :: GLGizmoSimplify::on_render_input_window
// Changes: Laid out as Mica tool cards (same place and look as the cut card): the boolean card has the three operations as chips, the second shape as a dropdown instead of click-to-select source / tool volumes, a plain-language line per operation, and two toggles ("Keep the other shape", upstream's "Delete input" inverted, and "Keep the original object"); the simplify card keeps upstream's two modes (level of detail, decimate ratio with the resulting count) with chips, a slider and the before → after triangle counts, the wireframe toggle, Apply and Close.

//! The boolean and simplify cards of the mesh tools (`crate::mesh_tools`).

use super::subcards::card;
use super::widgets;
use crate::app::App;
use crate::mesh_tools::{BooleanState, MeshTool, Other};
use crate::theme;
use eframe::egui::{self, Sense, Vec2};
use mica_core::mesh_boolean::BooleanOp;
use mica_core::mesh_simplify::DETAIL_LEVELS;

fn thousands(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn toggle_row(ui: &mut egui::Ui, id: &str, on: bool, label: &str, tip: &str) -> Option<bool> {
    let p = theme::of(ui);
    let mut out = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.;
        let (r, _) = ui.allocate_exact_size(Vec2::new(34., 20.), Sense::hover());
        out = widgets::toggle_at(ui, ui.make_persistent_id(id), r, on, true);
        ui.label(
            egui::RichText::new(label)
                .font(theme::sans(12.))
                .color(p.label),
        )
        .on_hover_text(tip);
    });
    out
}

fn muted(ui: &mut egui::Ui, text: &str) {
    let p = theme::of(ui);
    ui.label(
        egui::RichText::new(text)
            .font(theme::sans(11.5))
            .color(p.muted),
    );
}

impl App {
    /// The open mesh tool's card (instead of the object card).
    pub(crate) fn mesh_tool_window(&mut self, ctx: &egui::Context) {
        self.validate_mesh_tool();
        if self.mesh_tool.is_none() {
            return;
        }
        match &self.mesh_tool {
            Some(MeshTool::Boolean(_)) => self.boolean_card(ctx),
            Some(MeshTool::Simplify(_)) => {
                self.poll_simplify(ctx);
                self.simplify_card(ctx);
            }
            None => {}
        }
    }

    fn boolean_card(&mut self, ctx: &egui::Context) {
        let Some(i) = self.mesh_tool_target() else {
            return;
        };
        let Some(MeshTool::Boolean(state)) = &self.mesh_tool else {
            return;
        };
        let state = state.clone();
        let o = &self.plates.current().objects[i];
        let name = std::path::Path::new(&o.name)
            .file_stem()
            .map_or(o.name.clone(), |s| s.to_string_lossy().into_owned());
        let choices = self.boolean_choices(i);
        let mut op = None;
        let mut other = None;
        let mut keep_other = None;
        let mut keep_original = None;
        let mut apply = false;
        let mut close = false;
        card(
            ctx,
            self.viewport_rect,
            &format!("{name} · Mesh boolean"),
            |ui| {
                let p = theme::of(ui);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.;
                    for k in BooleanOp::ALL {
                        if widgets::chip(ui, None, k.label(), state.op == k, true).clicked() {
                            op = Some(k);
                        }
                    }
                });
                ui.add_space(2.);
                muted(ui, BooleanState::explain(state.op));
                ui.add_space(6.);
                let (row, _) =
                    ui.allocate_exact_size(Vec2::new(ui.available_width(), 30.), Sense::hover());
                ui.painter().text(
                    egui::pos2(row.left(), row.center().y),
                    egui::Align2::LEFT_CENTER,
                    if state.op == BooleanOp::Difference {
                        "Subtract"
                    } else {
                        "With"
                    },
                    theme::sans(12.),
                    p.muted_alt,
                );
                let field = egui::Rect::from_min_max(
                    egui::pos2(row.left() + 74., row.top() + 2.),
                    egui::pos2(row.right(), row.bottom() - 2.),
                );
                let value = |c: Other| match c {
                    Other::Object(id) => format!("o{id}"),
                    Other::Volume(id) => format!("v{id}"),
                };
                let items: Vec<(String, String)> = choices
                    .iter()
                    .map(|(c, label)| (value(*c), label.clone()))
                    .collect();
                let current = state.other.map(value).unwrap_or_default();
                let current_label = choices
                    .iter()
                    .find(|(c, _)| Some(*c) == state.other)
                    .map_or("Choose a shape…".to_string(), |(_, l)| l.clone());
                if items.is_empty() {
                    ui.painter().text(
                        egui::pos2(field.left(), field.center().y),
                        egui::Align2::LEFT_CENTER,
                        "Add another object or a part first",
                        theme::sans(12.),
                        p.muted,
                    );
                } else if let Some(v) = widgets::dropdown_with(
                    ui,
                    ui.make_persistent_id("boolean-other"),
                    field,
                    &current,
                    &current_label,
                    None,
                    &items,
                    true,
                    false,
                ) {
                    other = choices
                        .iter()
                        .find(|(c, _)| value(*c) == v)
                        .map(|(c, _)| *c);
                }
                ui.add_space(4.);
                keep_other = toggle_row(
                    ui,
                    "boolean-keep-other",
                    state.keep_other,
                    "Keep the other shape",
                    "Off: the second object or part is used up by the operation",
                );
                keep_original = toggle_row(
                    ui,
                    "boolean-keep-original",
                    state.keep_original,
                    "Keep the original object",
                    "On: the result is added as a new object and this one stays as it is",
                );
                if let Some(m) = &state.message {
                    ui.add_space(4.);
                    ui.label(
                        egui::RichText::new(m)
                            .font(theme::sans(12.))
                            .color(p.warn_text),
                    );
                }
                ui.add_space(8.);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.;
                    if widgets::primary_button(
                        ui,
                        BooleanState::verb(state.op),
                        state.other.is_some(),
                    )
                    .clicked()
                    {
                        apply = true;
                    }
                    if widgets::styled_button(ui, "Close", true, false, 28.).clicked() {
                        close = true;
                    }
                    ui.label(
                        egui::RichText::new("B toggles · Esc closes")
                            .font(theme::sans(11.))
                            .color(p.muted),
                    );
                });
            },
        );
        if let Some(MeshTool::Boolean(s)) = &mut self.mesh_tool {
            if let Some(k) = op
                && k != s.op
            {
                s.op = k;
                s.keep_other = BooleanState::default_keep_other(k);
                s.message = None;
            }
            if let Some(c) = other {
                s.other = Some(c);
                s.message = None;
            }
            if let Some(v) = keep_other {
                s.keep_other = v;
            }
            if let Some(v) = keep_original {
                s.keep_original = v;
            }
        }
        if apply {
            self.apply_boolean();
        }
        if close {
            self.mesh_tool = None;
        }
    }

    fn simplify_card(&mut self, ctx: &egui::Context) {
        let Some(i) = self.mesh_tool_target() else {
            return;
        };
        let Some(MeshTool::Simplify(s)) = &self.mesh_tool else {
            return;
        };
        let params = s.params;
        let original = s.original;
        let wanted = s.wanted_count();
        let result = s.result_count();
        let computing = s.computing();
        let wireframe = s.wireframe;
        let o = &self.plates.current().objects[i];
        let painted = !o.painting.is_empty();
        let name = std::path::Path::new(&o.name)
            .file_stem()
            .map_or(o.name.clone(), |s| s.to_string_lossy().into_owned());
        let mut new_params = params;
        let mut new_wire = None;
        let mut apply = false;
        let mut close = false;
        card(
            ctx,
            self.viewport_rect,
            &format!("{name} · Simplify"),
            |ui| {
                let p = theme::of(ui);
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("Triangles")
                            .font(theme::sans(12.))
                            .color(p.muted_alt),
                    );
                    ui.add_space(8.);
                    let after = match result {
                        Some(n) => thousands(n),
                        None => "…".into(),
                    };
                    ui.label(
                        egui::RichText::new(format!("{} → {after}", thousands(original)))
                            .font(theme::mono(12.5))
                            .color(p.text),
                    );
                });
                ui.add_space(4.);
                ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.;
                if widgets::chip(ui, None, "Triangle count", params.use_count, true)
                    .on_hover_text("Remove a share of the triangles")
                    .clicked()
                {
                    new_params.use_count = true;
                }
                if widgets::chip(ui, None, "Detail level", !params.use_count, true)
                    .on_hover_text("Remove triangles until the shape would change more than the level allows")
                    .clicked()
                {
                    new_params.use_count = false;
                }
            });
                ui.add_space(4.);
                if params.use_count {
                    let mut ratio = params.ratio;
                    let r = ui.add(
                        egui::Slider::new(&mut ratio, 0.0..=99.0)
                            .text("removed")
                            .suffix(" %")
                            .fixed_decimals(0),
                    );
                    if r.changed() {
                        new_params.ratio = ratio;
                    }
                    muted(ui, &format!("Aim: {} triangles", thousands(wanted)));
                } else {
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = Vec2::new(4., 4.);
                        for (k, (label, _)) in DETAIL_LEVELS.iter().enumerate() {
                            if widgets::chip(ui, None, label, params.level == k, true).clicked() {
                                new_params.level = k;
                            }
                        }
                    });
                }
                ui.add_space(4.);
                new_wire = toggle_row(
                    ui,
                    "simplify-wire",
                    wireframe,
                    "Show wireframe",
                    "Draws the simplified triangles over the model",
                );
                if computing {
                    muted(ui, "Computing the preview…");
                }
                if painted {
                    ui.label(
                        egui::RichText::new("Painting on this object will be removed.")
                            .font(theme::sans(11.5))
                            .color(p.warn_text),
                    );
                }
                ui.add_space(8.);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.;
                    let useful = result.is_some_and(|n| n < original);
                    if widgets::primary_button(ui, "Apply", useful || computing).clicked() {
                        apply = true;
                    }
                    if widgets::styled_button(ui, "Close", true, false, 28.).clicked() {
                        close = true;
                    }
                    ui.label(
                        egui::RichText::new("Esc closes")
                            .font(theme::sans(11.))
                            .color(p.muted),
                    );
                });
            },
        );
        if let Some(MeshTool::Simplify(s)) = &mut self.mesh_tool {
            s.params = new_params;
            if let Some(w) = new_wire {
                s.wireframe = w;
            }
        }
        if apply {
            self.apply_simplify();
        }
        if close {
            self.mesh_tool = None;
        }
        self.draw_simplify_wireframe(ctx);
    }
}

/// A small warning triangle with an exclamation mark (the repair badge).
pub fn warning_glyph(painter: &egui::Painter, r: egui::Rect, c: egui::Color32) {
    let s = egui::Stroke::new(1.6, c);
    let top = egui::pos2(r.center().x, r.top() + 1.);
    let left = egui::pos2(r.left() + 0.5, r.bottom() - 1.);
    let right = egui::pos2(r.right() - 0.5, r.bottom() - 1.);
    painter.add(egui::Shape::closed_line(vec![top, right, left], s));
    painter.line_segment(
        [
            egui::pos2(r.center().x, r.top() + 5.),
            egui::pos2(r.center().x, r.bottom() - 5.),
        ],
        s,
    );
    painter.circle_filled(egui::pos2(r.center().x, r.bottom() - 3.), 1., c);
}
