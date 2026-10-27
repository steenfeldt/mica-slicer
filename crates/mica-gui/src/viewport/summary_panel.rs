// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The project summary card (`crate::project_summary`), opened from the header of Preview's
//! plate list and drawn beside the list, over the 3D view: the totals of every sliced plate,
//! the filaments to have on hand (hover a row for the plates using it), the plates as a
//! timeline in print order (hover a plate for its time, weight, colors and picture; click to
//! show it), where the time goes by feature, what is not sliced yet with "Slice the other N",
//! and Copy summary. It sits beside the plate list because it is about the plates: the right
//! sidebar stays the current plate's.

use super::{Action, Scene, Viewport};
use crate::plate_batch::PlateStatus;
use crate::project_summary::{
    ProjectSummary, duration, feature_name, filament_label, length, tools_text, weight,
};
use crate::theme;
use eframe::egui::{self, Align2, Color32, Pos2, Rect, Sense, Stroke, Vec2};

/// The card's width (narrow views get less).
const WIDTH: f32 = 340.;
const ROW: f32 = 22.;

fn open_id() -> egui::Id {
    egui::Id::unique("mica project summary open")
}
/// The card's area (the plate list does not make room for it).
pub fn card_id() -> egui::Id {
    egui::Id::unique("mica project summary card")
}
fn copied_id() -> egui::Id {
    egui::Id::unique("mica project summary copied")
}

/// Whether the project summary is open.
pub fn is_open(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp(open_id())).unwrap_or(false)
}
pub fn set_open(ctx: &egui::Context, open: bool) {
    ctx.data_mut(|d| d.insert_temp(open_id(), open));
}

/// Where the card was drawn last frame, and its controls (UI tests).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SummaryProbe {
    pub card: Option<Rect>,
    pub close: Option<Rect>,
    pub copy: Option<Rect>,
    pub slice_rest: Option<Rect>,
    /// `(plate index, segment)` of the timeline.
    pub segments: Vec<(usize, Rect)>,
    /// `(tool, row)` of the filament rows.
    pub filaments: Vec<(usize, Rect)>,
}

fn probe_id() -> egui::Id {
    egui::Id::unique("mica project summary probe")
}
#[cfg(test)]
pub fn probe(ctx: &egui::Context) -> SummaryProbe {
    ctx.data(|d| d.get_temp(probe_id())).unwrap_or_default()
}

impl Viewport {
    /// The card beside the plate list (`list`), inside the 3D view (`view`).
    pub(super) fn project_summary(
        &mut self,
        ui: &mut egui::Ui,
        list: Rect,
        view: Rect,
        scene: &Scene<'_>,
        summary: &ProjectSummary,
        actions: &mut Vec<Action>,
    ) {
        let ctx = ui.ctx().clone();
        if !is_open(&ctx) {
            ctx.data_mut(|d| d.insert_temp(probe_id(), SummaryProbe::default()));
            return;
        }
        let width = WIDTH.min(view.width() - 32.).max(220.);
        // Beside the list; over it when the view is too narrow.
        let mut left = list.right() + 8.;
        if left + width > view.right() - 12. {
            left = list.left();
        }
        let top = list.top();
        // Clear of the move slider along the view's bottom.
        let max_h = (view.bottom() - 70. - top).max(120.);
        let mut probe = SummaryProbe::default();
        let area = egui::Area::new(card_id())
            .order(egui::Order::Middle)
            .fixed_pos(Pos2::new(left, top))
            .pivot(Align2::LEFT_TOP)
            .show(&ctx, |ui| {
                theme::overlay_frame(ui)
                    .inner_margin(egui::Margin::same(12))
                    .show(ui, |ui| {
                        ui.set_width(width - 24.);
                        card(ui, scene, summary, max_h - 24., &mut probe, actions);
                    });
            });
        probe.card = Some(area.response.rect);
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            set_open(&ctx, false);
        }
        ctx.data_mut(|d| d.insert_temp(probe_id(), probe));
    }
}

fn card(
    ui: &mut egui::Ui,
    scene: &Scene<'_>,
    s: &ProjectSummary,
    max_h: f32,
    probe: &mut SummaryProbe,
    actions: &mut Vec<Action>,
) {
    let p = theme::of(ui);
    let ctx = ui.ctx().clone();
    // Title and close.
    let (head, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 22.), Sense::hover());
    ui.painter().text(
        Pos2::new(head.left(), head.center().y),
        Align2::LEFT_CENTER,
        "Project summary",
        theme::semibold(14.),
        p.text,
    );
    let close = Rect::from_center_size(
        Pos2::new(head.right() - 9., head.center().y),
        Vec2::splat(20.),
    );
    let r = ui
        .interact(close, ui.scope_id().with("close"), Sense::click())
        .on_hover_text("Close (Esc)");
    if r.hovered() {
        ui.painter().rect_filled(close, 5., p.raised);
    }
    let c = close.center();
    for (a, b) in [(-4., 4.), (4., -4.)] {
        ui.painter().line_segment(
            [c + Vec2::new(-4., a), c + Vec2::new(4., b)],
            Stroke::new(1.4, p.muted_alt),
        );
    }
    if r.clicked() {
        set_open(&ctx, false);
    }
    probe.close = Some(close);
    let subtitle = if s.plates == 0 {
        "No plate has objects".to_string()
    } else if s.complete() {
        format!(
            "{} {} sliced",
            if s.plates > 1 { "All" } else { "The" },
            if s.plates == 1 {
                "plate".to_string()
            } else {
                format!("{} plates", s.plates)
            }
        )
    } else {
        format!("{} of {} plates sliced", s.sliced, s.plates)
    };
    ui.label(
        egui::RichText::new(subtitle)
            .font(theme::sans(12.))
            .color(p.muted),
    );
    ui.add_space(8.);
    // A scroll bar that takes its own room instead of covering the values' column.
    ui.spacing_mut().scroll = egui::style::ScrollStyle::solid();
    egui::ScrollArea::vertical()
        .id_salt("project summary scroll")
        .max_height((max_h - 100.).max(80.))
        // The area's room is last frame's size: let the card grow into the height it may take.
        .min_scrolled_height((max_h - 100.).max(80.))
        .auto_shrink([false, true])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 2.;
            if !s.complete() {
                incomplete(ui, scene, s, probe, actions);
            }
            if s.sliced > 0 {
                totals(ui, s);
                filaments(ui, scene, s, probe);
                timeline(ui, scene, s, probe, actions);
                features(ui, s);
            }
        });
    ui.add_space(8.);
    // Copy summary, with a moment of "Copied".
    let now = ui.input(|i| i.time);
    let copied_at: Option<f64> = ctx.data(|d| d.get_temp(copied_id()));
    let copied = copied_at.is_some_and(|t| now - t < 1.5);
    ui.horizontal(|ui| {
        let r = crate::panels::widgets::button(
            ui,
            if copied { "Copied" } else { "Copy summary" },
            s.sliced > 0,
        )
        .on_hover_text("Copy the totals, filaments and plates as text");
        probe.copy = Some(r.rect);
        if r.clicked() {
            ctx.copy_text(s.text(scene.project_name));
            ctx.data_mut(|d| d.insert_temp(copied_id(), now));
        }
    });
    if copied {
        ctx.request_repaint_after(std::time::Duration::from_millis(300));
    }
}

/// A section title with a hairline under it.
fn heading(ui: &mut egui::Ui, title: &str) {
    let p = theme::of(ui);
    ui.add_space(8.);
    let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 20.), Sense::hover());
    ui.painter().text(
        Pos2::new(r.left(), r.center().y),
        Align2::LEFT_CENTER,
        title,
        theme::semibold(12.5),
        p.text,
    );
    ui.painter().line_segment(
        [
            Pos2::new(r.left(), r.bottom()),
            Pos2::new(r.right(), r.bottom()),
        ],
        Stroke::new(1., p.border_soft),
    );
    ui.add_space(4.);
}

/// Label left, value right (monospace).
fn value_row(ui: &mut egui::Ui, label: &str, value: &str, hover: Option<&str>) {
    let p = theme::of(ui);
    let (rect, r) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW), Sense::hover());
    let painter = ui.painter();
    let v = painter.layout_no_wrap(value.to_string(), theme::mono(12.), p.text);
    let vw = v.size().x;
    painter.galley(
        Pos2::new(rect.right() - vw, rect.center().y - v.size().y / 2.),
        v,
        p.text,
    );
    let l = crate::preview::legend::truncated(
        painter,
        label.to_string(),
        theme::sans(12.5),
        p.label,
        rect.width() - vw - 12.,
    );
    painter.galley(
        Pos2::new(rect.left(), rect.center().y - l.size().y / 2.),
        l,
        p.label,
    );
    if let Some(h) = hover {
        r.on_hover_text(h);
    }
}

fn incomplete(
    ui: &mut egui::Ui,
    scene: &Scene<'_>,
    s: &ProjectSummary,
    probe: &mut SummaryProbe,
    actions: &mut Vec<Action>,
) {
    let p = theme::of(ui);
    egui::Frame::new()
        .fill(p.warn_surface)
        .stroke(Stroke::new(1., p.warn_border))
        .corner_radius(8)
        .inner_margin(egui::Margin::same(10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let missing = s.missing.len();
            let text = if s.sliced == 0 {
                "No plate is sliced yet: slice the plates to see the totals.".to_string()
            } else {
                let mut t = s.missing_text();
                if let Some(first) = t.get_mut(0..1) {
                    first.make_ascii_uppercase();
                }
                format!(
                    "{t}; the totals leave {} out.",
                    if missing == 1 { "it" } else { "them" }
                )
            };
            ui.add(
                egui::Label::new(
                    egui::RichText::new(text)
                        .font(theme::sans(12.))
                        .color(p.warn_text),
                )
                .wrap(),
            );
            ui.add_space(6.);
            let busy = scene
                .plates
                .iter()
                .any(|pl| matches!(pl.status, PlateStatus::Queued | PlateStatus::Slicing { .. }));
            let label = if busy {
                "Slicing…".to_string()
            } else if s.sliced == 0 {
                "Slice all plates".to_string()
            } else {
                format!("Slice the other {missing}")
            };
            let r =
                crate::panels::widgets::primary_button(ui, &label, scene.can_slice_all && !busy)
                    .on_hover_text("Slice every plate that is not sliced yet or changed")
                    .on_disabled_hover_text(if busy {
                        "Slicing the plates now"
                    } else {
                        "Slicing is not available now"
                    });
            probe.slice_rest = Some(r.rect);
            if r.clicked() {
                actions.push(Action::SliceAll);
            }
        });
}

fn totals(ui: &mut egui::Ui, s: &ProjectSummary) {
    heading(ui, "Totals");
    value_row(
        ui,
        "Print time",
        &duration(s.time_s),
        Some("One plate after the other, as the plates' own estimates add up"),
    );
    value_row(
        ui,
        "Filament",
        &format!("{} · {}", weight(s.grams), length(s.mm)),
        None,
    );
    if s.cost > 0. {
        value_row(
            ui,
            "Cost",
            &format!("{:.2}", s.cost),
            Some("From the filament price per kg (Filament settings › Price)"),
        );
    }
    if s.tool_changes > 0 {
        value_row(ui, "Tool changes", &s.tool_changes.to_string(), None);
    }
    if let Some(tower) = &s.tower
        && s.tower_grams > 0.
    {
        value_row(
            ui,
            tower,
            &format!(
                "{} · {:.0} %",
                weight(s.tower_grams),
                s.share(s.tower_grams)
            ),
            Some(&format!(
                "Of the filament above, what the {} took: priming and purging at tool changes",
                tower.to_lowercase()
            )),
        );
    }
}

/// The swatch colour of `tool`: the filament's colour with its alpha.
fn tool_rgba(scene: &Scene<'_>, tool: usize) -> [u8; 4] {
    scene.tool_rgba.get(tool).copied().unwrap_or_else(|| {
        let c = scene
            .tool_colors
            .get(tool)
            .copied()
            .unwrap_or_else(|| crate::preview::legend::tool_color(tool));
        let [r, g, b] = c.map(|v| (v * 255.).round() as u8);
        [r, g, b, 255]
    })
}

fn filaments(ui: &mut egui::Ui, scene: &Scene<'_>, s: &ProjectSummary, probe: &mut SummaryProbe) {
    let p = theme::of(ui);
    heading(ui, "By filament");
    for f in &s.filaments {
        let (rect, r) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW + 8.), Sense::hover());
        probe.filaments.push((f.tool, rect));
        if r.hovered() {
            ui.painter().rect_filled(rect.expand(2.), 5., p.raised);
        }
        let painter = ui.painter();
        let line = Rect::from_min_size(rect.min, Vec2::new(rect.width(), ROW));
        let sw = Rect::from_center_size(
            Pos2::new(line.left() + 7., line.center().y),
            Vec2::splat(14.),
        );
        crate::panels::colour::paint_colour(painter, sw, 3., tool_rgba(scene, f.tool));
        painter.rect_stroke(
            sw,
            3.,
            Stroke::new(1., p.border_raised),
            egui::StrokeKind::Inside,
        );
        let mut value = format!("{} · {}", weight(f.grams), length(f.mm));
        if f.cost > 0. {
            value.push_str(&format!(" · {:.2}", f.cost));
        }
        let v = painter.layout_no_wrap(value, theme::mono(11.5), p.text);
        let vw = v.size().x;
        painter.galley(
            Pos2::new(line.right() - vw, line.center().y - v.size().y / 2.),
            v,
            p.text,
        );
        let label_left = sw.right() + 8.;
        let l = crate::preview::legend::truncated(
            painter,
            filament_label(f),
            theme::sans(12.5),
            p.label,
            line.right() - vw - 10. - label_left,
        );
        painter.galley(
            Pos2::new(label_left, line.center().y - l.size().y / 2.),
            l,
            p.label,
        );
        // Share bar under the label.
        let share = s.share(f.grams);
        let bar = Rect::from_min_size(
            Pos2::new(label_left, line.bottom() + 1.),
            Vec2::new(rect.right() - label_left, 4.),
        );
        painter.rect_filled(bar, 2., p.raised);
        let mut fill = bar;
        fill.set_width((bar.width() * (share as f32 / 100.)).max(2.));
        let [cr, cg, cb, _] = tool_rgba(scene, f.tool);
        painter.rect_filled(fill, 2., Color32::from_rgb(cr, cg, cb));
        painter.rect_stroke(
            fill,
            2.,
            Stroke::new(0.6, p.border_raised),
            egui::StrokeKind::Inside,
        );
        r.on_hover_ui(|ui| {
            ui.label(
                egui::RichText::new(format!(
                    "{} · {:.0} % of the filament",
                    filament_label(f),
                    share
                ))
                .font(theme::semibold(12.5)),
            );
            if f.tower_grams > 0. {
                ui.label(format!(
                    "{} of it for the {}",
                    weight(f.tower_grams),
                    s.tower.as_deref().unwrap_or("tower").to_lowercase()
                ));
            }
            ui.add_space(4.);
            ui.label(match f.plates.len() {
                1 => "Used on 1 plate:".to_string(),
                n => format!("Used on {n} plates:"),
            });
            for (index, grams, mm) in &f.plates {
                let title = s
                    .timeline
                    .iter()
                    .find(|p| p.index == *index)
                    .map_or_else(|| format!("Plate {}", index + 1), |p| p.title.clone());
                ui.label(format!("{title}: {} · {}", weight(*grams), length(*mm)));
            }
        });
    }
}

fn timeline(
    ui: &mut egui::Ui,
    scene: &Scene<'_>,
    s: &ProjectSummary,
    probe: &mut SummaryProbe,
    actions: &mut Vec<Action>,
) {
    let p = theme::of(ui);
    heading(ui, "By plate");
    let (bar, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 30.), Sense::hover());
    let total = s.time_s.max(1.);
    let gap = 2.;
    let n = s.timeline.len() as f32;
    let room = bar.width() - gap * (n - 1.).max(0.);
    let current = scene.plates.iter().find(|pl| pl.current).map(|pl| pl.index);
    let mut x = bar.left();
    for (k, plate) in s.timeline.iter().enumerate() {
        let w = (room * (plate.time_s / total) as f32).max(3.);
        let seg = Rect::from_min_max(
            Pos2::new(x, bar.top()),
            Pos2::new((x + w).min(bar.right()), bar.bottom()),
        );
        x = seg.right() + gap;
        probe.segments.push((plate.index, seg));
        let r = ui.interact(
            seg,
            ui.scope_id().with(("plate segment", k)),
            Sense::click(),
        );
        let painter = ui.painter();
        let is_current = current == Some(plate.index);
        painter.rect_filled(
            seg,
            4.,
            if is_current {
                p.selected_row
            } else if r.hovered() {
                p.border_raised
            } else {
                p.raised
            },
        );
        if is_current || r.hovered() {
            painter.rect_stroke(
                seg,
                4.,
                Stroke::new(1.5, if is_current { p.accent } else { p.muted_alt }),
                egui::StrokeKind::Inside,
            );
        }
        // The plate's colours as a strip along the bottom.
        if !plate.tools.is_empty() {
            let strip = Rect::from_min_max(
                Pos2::new(seg.left() + 2., seg.bottom() - 6.),
                Pos2::new(seg.right() - 2., seg.bottom() - 2.),
            );
            let each = strip.width() / plate.tools.len() as f32;
            for (j, &t) in plate.tools.iter().enumerate() {
                let [cr, cg, cb, _] = tool_rgba(scene, t);
                painter.rect_filled(
                    Rect::from_min_size(
                        Pos2::new(strip.left() + each * j as f32, strip.top()),
                        Vec2::new(each, strip.height()),
                    ),
                    0.,
                    Color32::from_rgb(cr, cg, cb),
                );
            }
        }
        let number = (plate.index + 1).to_string();
        let g = painter.layout_no_wrap(number, theme::sans(11.), p.text);
        if g.size().x + 4. < seg.width() {
            painter.galley(
                Pos2::new(
                    seg.center().x - g.size().x / 2.,
                    seg.top() + 3. + (seg.height() - 9. - g.size().y) / 2.,
                ),
                g,
                p.text,
            );
        }
        if r.hovered() {
            ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
        }
        let clicked = r.clicked();
        let thumb = scene.plate_thumbs.get(plate.index).cloned();
        r.on_hover_ui(|ui| {
            ui.horizontal(|ui| {
                if let Some(t) = thumb.as_ref().and_then(|t| t.texture()) {
                    ui.add(egui::Image::new(t).fit_to_exact_size(Vec2::splat(64.)));
                }
                ui.vertical(|ui| {
                    ui.label(egui::RichText::new(&plate.title).font(theme::semibold(12.5)));
                    ui.label(format!(
                        "{} · {} · {}",
                        duration(plate.time_s),
                        weight(plate.grams),
                        tools_text(&plate.tools)
                    ));
                    ui.label(format!(
                        "Starts after {}, done after {}",
                        duration(plate.start_s),
                        duration(plate.start_s + plate.time_s)
                    ));
                    ui.label(
                        egui::RichText::new(if is_current {
                            "Shown now"
                        } else {
                            "Click to show this plate"
                        })
                        .color(p.muted),
                    );
                });
            });
        });
        if clicked && !is_current {
            actions.push(Action::SelectPlate(plate.index));
        }
    }
    // Running total under the bar: start, where the middle plate starts, end.
    let (axis, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 16.), Sense::hover());
    let painter = ui.painter();
    let font = theme::sans(10.5);
    painter.text(
        Pos2::new(axis.left(), axis.center().y),
        Align2::LEFT_CENTER,
        "0",
        font.clone(),
        p.muted,
    );
    let end = painter.text(
        Pos2::new(axis.right(), axis.center().y),
        Align2::RIGHT_CENTER,
        duration(s.time_s),
        font.clone(),
        p.muted,
    );
    if s.timeline.len() > 1 {
        let mid = &s.timeline[s.timeline.len() / 2];
        let at = axis.left() + axis.width() * (mid.start_s / total) as f32;
        let g = painter.layout_no_wrap(duration(mid.start_s), font, p.muted);
        let half = g.size().x / 2.;
        if at - half > axis.left() + 20. && at + half < end.left() - 10. {
            painter.galley(
                Pos2::new(at - half, axis.center().y - g.size().y / 2.),
                g,
                p.muted,
            );
        }
    }
}

fn features(ui: &mut egui::Ui, s: &ProjectSummary) {
    let p = theme::of(ui);
    if s.features.is_empty() {
        return;
    }
    heading(ui, "Where the time goes");
    for &(role, secs) in &s.features {
        let share = if s.time_s > 0. {
            100. * secs / s.time_s
        } else {
            0.
        };
        let (rect, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW), Sense::hover());
        let painter = ui.painter();
        let [r, g, b] = crate::preview::legend::COLORS[role.min(15)];
        let sw = Rect::from_center_size(
            Pos2::new(rect.left() + 6., rect.center().y),
            Vec2::splat(10.),
        );
        painter.rect_filled(sw, 2., Color32::from_rgb(r, g, b));
        let value = format!("{} · {:>3.0} %", duration(secs), share);
        let v = painter.layout_no_wrap(value, theme::mono(11.5), p.text);
        let vw = v.size().x;
        painter.galley(
            Pos2::new(rect.right() - vw, rect.center().y - v.size().y / 2.),
            v,
            p.text,
        );
        // A share bar between the name and the value.
        let name_left = sw.right() + 8.;
        let name =
            painter.layout_no_wrap(feature_name(role).to_string(), theme::sans(12.5), p.label);
        let name_w = name.size().x;
        painter.galley(
            Pos2::new(name_left, rect.center().y - name.size().y / 2.),
            name,
            p.label,
        );
        let bar_left = name_left + name_w.max(110.) + 8.;
        let bar_right = rect.right() - vw - 10.;
        if bar_right - bar_left > 20. {
            let bar = Rect::from_min_max(
                Pos2::new(bar_left, rect.center().y - 2.),
                Pos2::new(bar_right, rect.center().y + 2.),
            );
            painter.rect_filled(bar, 2., p.raised);
            let mut fill = bar;
            fill.set_width((bar.width() * (share as f32 / 100.)).max(1.));
            painter.rect_filled(fill, 2., p.muted_alt);
        }
    }
    ui.add_space(2.);
    let p = theme::of(ui);
    ui.label(
        egui::RichText::new("Estimated per feature, as Color by Feature shows each plate.")
            .font(theme::sans(11.))
            .color(p.muted),
    );
}
