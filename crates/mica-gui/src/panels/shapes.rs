// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The "Add shape" gallery: a compact grid of the built-in models (`crate::builtin`) with
//! their thumbnails and names, opened from the object list's "+ Add" and the 3D view
//! toolbar's Add button. With an object selected, a row of pills above the grid picks how the
//! shape joins: as a new object, or as a part, modifier, negative volume or support volume of
//! that object (remembered while the window is open).

use crate::builtin::{self, AddAs};
use crate::theme;
use eframe::egui::{self, Align2, Pos2, Rect, Sense, Stroke, Vec2};
use mica_core::builtin_models::BuiltinModel;

const COLUMNS: usize = 5;
const TILE: Vec2 = Vec2::new(62., 80.);
const PICTURE: f32 = 50.;
const GAP: f32 = 4.;

/// What the gallery picked.
pub type ShapePick = (BuiltinModel, AddAs);

fn add_as_id() -> egui::Id {
    egui::Id::unique("mica-shape-gallery-add-as")
}

/// The gallery's width.
pub fn width() -> f32 {
    COLUMNS as f32 * TILE.x + (COLUMNS - 1) as f32 * GAP
}

/// Draws the gallery; `object`: the selected object's name (offers adding to it); `header`:
/// an "Add shape" title above it (where no menu entry names it).
pub fn gallery(ui: &mut egui::Ui, object: Option<&str>, header: bool) -> Option<ShapePick> {
    let p = theme::of(ui);
    let w = width();
    ui.set_min_width(w);
    ui.set_max_width(w);
    let mut add_as = if object.is_some() {
        ui.ctx()
            .data(|d| d.get_temp::<AddAs>(add_as_id()))
            .unwrap_or_default()
    } else {
        AddAs::Object
    };
    if header {
        ui.label(
            egui::RichText::new(crate::panels::widgets::spaced_upper("Add shape"))
                .font(theme::semibold(11.))
                .color(p.muted),
        );
        ui.add_space(4.);
    }
    if let Some(name) = object {
        // Three equal pills a row: how the shape joins.
        let pill_w = (w - 2. * GAP) / 3.;
        for row in AddAs::ALL.chunks(3) {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = GAP;
                for &a in row {
                    let tip = match a {
                        AddAs::Object => a.hint().to_string(),
                        _ => format!("{}: {name}", a.hint()),
                    };
                    if pill(ui, a.label(), a == add_as, pill_w)
                        .on_hover_text(tip)
                        .clicked()
                    {
                        add_as = a;
                        ui.ctx().data_mut(|d| d.insert_temp(add_as_id(), a));
                    }
                }
            });
            ui.add_space(GAP);
        }
        ui.add_space(4.);
    }
    let mut out = None;
    let ctx = ui.ctx().clone();
    for row in BuiltinModel::ALL.chunks(COLUMNS) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = GAP;
            for &m in row {
                let (r, resp) = ui.allocate_exact_size(TILE, Sense::click());
                let hot = resp.hovered();
                if hot {
                    ui.painter().rect_filled(r, 6., p.accent_surface);
                    ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
                }
                let pic = Rect::from_center_size(
                    Pos2::new(r.center().x, r.top() + 6. + PICTURE / 2.),
                    Vec2::splat(PICTURE),
                );
                match builtin::thumbnail(&ctx, m) {
                    Some(tex) => {
                        ui.painter().image(
                            tex.id(),
                            pic,
                            Rect::from_min_max(Pos2::ZERO, Pos2::new(1., 1.)),
                            egui::Color32::WHITE,
                        );
                    }
                    None => {
                        ui.painter().rect_stroke(
                            pic.shrink(8.),
                            6.,
                            Stroke::new(1., p.border_soft),
                            egui::StrokeKind::Inside,
                        );
                    }
                }
                ui.painter().text(
                    Pos2::new(r.center().x, r.bottom() - 12.),
                    Align2::CENTER_CENTER,
                    m.label(),
                    theme::sans(12.),
                    if hot { p.mark_fg } else { p.text },
                );
                let tip = match m.credit() {
                    Some(c) => format!("{}\n{}", m.description(), c),
                    None => m.description().to_string(),
                };
                if resp.on_hover_text(tip).clicked() {
                    out = Some((m, add_as));
                    ui.close();
                }
            }
        });
        ui.add_space(GAP);
    }
    out
}

/// A small selectable pill, `width` wide, its text centred.
fn pill(ui: &mut egui::Ui, text: &str, on: bool, width: f32) -> egui::Response {
    let p = theme::of(ui);
    let g = ui
        .painter()
        .layout_no_wrap(text.to_string(), theme::sans(12.), p.text);
    let (r, resp) = ui.allocate_exact_size(Vec2::new(width, 24.), Sense::click());
    let fill = if on {
        p.accent_surface
    } else if resp.hovered() {
        p.hover
    } else {
        p.raised
    };
    ui.painter().rect_filled(r, 12., fill);
    ui.painter().rect_stroke(
        r,
        12.,
        Stroke::new(1., if on { p.accent } else { p.border_raised }),
        egui::StrokeKind::Inside,
    );
    if resp.hovered() {
        ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
    }
    ui.painter().galley(
        r.center() - g.size() / 2.,
        g,
        if on { p.mark_fg } else { p.text },
    );
    resp
}
