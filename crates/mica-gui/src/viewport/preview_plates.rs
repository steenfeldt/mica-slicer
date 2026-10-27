// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Preview's plate list: with two or more plates, a column at the 3D view's left edge with one
//! row per plate (its number and name, its slice status as a mark and a few words,
//! a warning marker) under a header with how many are sliced and what the sliced plates add up
//! to ("all 15 sliced", "72 h 10 min · 2.31 kg"). Clicking the header opens the project
//! summary beside the list (`summary_panel`). The plate shown is highlighted; a click shows
//! another plate (its preview, or its models with "Slice this plate" in the sidebar when it
//! has no slice). Long lists scroll; the column stays above the axis triad and below any card
//! hanging from the view's top-left corner, and never reaches the layer slider.

use super::{Action, Scene, Viewport};
use crate::plate_batch::{PlateStatus, status_mark};
use crate::project_summary::ProjectSummary;
use crate::scene::plates::Plate;
use crate::theme;
use eframe::egui::{self, Align2, Pos2, Rect, Sense, Vec2 as EVec2};

/// Where the list was drawn last frame (UI tests click its rows).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PreviewPlatesProbe {
    pub list: Option<Rect>,
    /// `(plate index, row)` for the rows drawn (scrolled-away rows are left out).
    pub rows: Vec<(usize, Rect)>,
    /// The header (a click opens the project summary).
    pub header: Option<Rect>,
}

const EDGE: f32 = 14.;
const ROW_H: f32 = 36.;
/// The column's widest; narrow views get less.
const MAX_WIDTH: f32 = 216.;

/// The row's two lines: `Plate 3 · Name` and the status words.
pub fn row_text(plate: &Plate) -> (String, String) {
    let mut title = format!("Plate {}", plate.index + 1);
    if let Some(name) = &plate.name {
        title = format!("{title} · {name}");
    }
    let status = match &plate.status {
        PlateStatus::Empty => "no objects".to_string(),
        s => s.text(),
    };
    (title, status)
}

/// Notes listed in a row's hover; more are counted.
const NOTES_SHOWN: usize = 5;

/// Whether the row carries the warning marker: warnings or an error besides a failed slice.
fn warns(plate: &Plate) -> bool {
    (plate.alert > 0 || !plate.notes.is_empty())
        && !matches!(plate.status, PlateStatus::Failed { .. })
}

/// The row's hover text: the status explained, the plate's own warnings, then what a click
/// does.
pub fn row_tooltip(plate: &Plate) -> String {
    let mut lines = vec![plate.status.tooltip()];
    if !plate.notes.is_empty() {
        lines.push(String::new());
        lines.push(match plate.notes.len() {
            1 => "1 warning on this plate:".into(),
            n => format!("{n} warnings on this plate:"),
        });
        for note in plate.notes.iter().take(NOTES_SHOWN) {
            lines.push(format!("• {note}"));
        }
        if plate.notes.len() > NOTES_SHOWN {
            lines.push(format!("and {} more", plate.notes.len() - NOTES_SHOWN));
        }
        lines.push(String::new());
    } else if warns(plate) {
        // Only the current plate's warnings are worked out in full.
        lines.push(if plate.current {
            "This plate has warnings; see its notifications".into()
        } else {
            "This plate has warnings: show it to see them".into()
        });
    }
    lines.push(if plate.current {
        "Shown now".into()
    } else if matches!(plate.status, PlateStatus::Sliced { .. }) {
        "Click to show its preview".into()
    } else {
        "Click to show this plate".into()
    });
    lines.join("\n")
}

impl Viewport {
    pub(super) fn preview_plates(
        &mut self,
        ui: &mut egui::Ui,
        rect: Rect,
        scene: &Scene<'_>,
        actions: &mut Vec<Action>,
    ) {
        let own = ui.scope_id().with("preview plates");
        let width = (rect.width() * 0.3).clamp(150., MAX_WIDTH);
        // Below any card hanging from the top-left corner (the collapsed sidebar's legend
        // chip), as the toolbar keeps clear of the cards on its row.
        let top_edge = rect.top() + EDGE;
        let x_span = (rect.left() + EDGE, rect.left() + EDGE + width);
        let below: f32 = ui.ctx().memory(|m| {
            m.areas()
                .visible_layer_ids()
                .into_iter()
                .filter(|l| {
                    l.order == egui::Order::Middle
                        && l.id != own
                        && l.id != super::summary_panel::card_id()
                })
                .filter_map(|l| m.area_rect(l.id))
                .filter(|r| (r.top() - top_edge).abs() < 1. && r.width() > 0.)
                .filter(|r| r.left() < x_span.1 && r.right() > x_span.0)
                .map(|r| r.bottom() + 8.)
                .fold(top_edge, f32::max)
        });
        // Above the axis triad (bottom left), as the measure card is.
        let bottom = rect.bottom() - 100.;
        let header_h = 46.;
        let frame_pad = 12.;
        let max_rows_h = (bottom - below - header_h - frame_pad).max(ROW_H);
        let current = scene.plates.iter().position(|p| p.current);
        let scroll_to_current = self.preview_plates_shown != current;
        self.preview_plates_shown = current;
        let summary = ProjectSummary::of(scene.plates);
        let mut probe = PreviewPlatesProbe::default();
        let area = egui::Area::new(own)
            .order(egui::Order::Middle)
            .fixed_pos(Pos2::new(rect.left() + EDGE, below))
            .pivot(Align2::LEFT_TOP)
            .show(ui.ctx(), |ui| {
                theme::overlay_frame(ui)
                    .inner_margin(egui::Margin::same(6))
                    .show(ui, |ui| {
                        ui.set_width(width);
                        // Header: "Plates", how many are sliced and their total; a click opens
                        // the project summary.
                        let (head, r) = ui.allocate_exact_size(
                            EVec2::new(width, header_h - 4.),
                            Sense::click(),
                        );
                        probe.header = Some(head);
                        header(ui, head, &summary, r.hovered());
                        if r.hovered() {
                            ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
                        }
                        let open = super::summary_panel::is_open(ui.ctx());
                        if r.clicked() {
                            super::summary_panel::set_open(ui.ctx(), !open);
                        }
                        r.on_hover_text(if open {
                            "Close the project summary"
                        } else {
                            "Show the project summary: totals, filaments, plates and where the time goes"
                        });
                        ui.add_space(2.);
                        egui::ScrollArea::vertical()
                            .id_salt("preview plates scroll")
                            // The area's room is last frame's size: `min_scrolled_height` lets
                            // the list grow into the height it may take.
                            .max_height(max_rows_h)
                            .min_scrolled_height(max_rows_h)
                            .auto_shrink([false, true])
                            .show(ui, |ui| {
                                ui.spacing_mut().item_spacing.y = 2.;
                                for plate in scene.plates {
                                    let (row, r) = ui.allocate_exact_size(
                                        EVec2::new(ui.available_width(), ROW_H),
                                        Sense::click(),
                                    );
                                    if plate.current && scroll_to_current {
                                        ui.scroll_to_rect(row, None);
                                    }
                                    if !ui.is_rect_visible(row) {
                                        continue;
                                    }
                                    probe.rows.push((plate.index, row));
                                    draw_row(ui, row, plate, r.hovered());
                                    if r.hovered() && !plate.current {
                                        ui.output_mut(|o| {
                                            o.cursor_icon = egui::CursorIcon::PointingHand
                                        });
                                    }
                                    let clicked = r.clicked();
                                    r.on_hover_text(row_tooltip(plate));
                                    if clicked && !plate.current {
                                        actions.push(Action::SelectPlate(plate.index));
                                    }
                                }
                            });
                    });
            });
        probe.list = Some(area.response.rect);
        let list = area.response.rect;
        self.preview_plates_probe = probe;
        self.project_summary(ui, list, rect, scene, &summary, actions);
    }
}

/// The header: "Plates" and how many are sliced over the total of the sliced plates and the
/// way to the summary. Long texts give way: the total first keeps its time.
fn header(ui: &egui::Ui, head: Rect, s: &ProjectSummary, hovered: bool) {
    let p = theme::of(ui);
    let painter = ui.painter();
    if hovered {
        painter.rect_filled(head, 6., p.raised);
    }
    let top = head.top() + 11.;
    let bottom = head.bottom() - 11.;
    painter.text(
        Pos2::new(head.left() + 6., top),
        Align2::LEFT_CENTER,
        "Plates",
        theme::semibold(12.5),
        p.text,
    );
    painter.text(
        Pos2::new(head.right() - 6., top),
        Align2::RIGHT_CENTER,
        s.note(),
        theme::sans(11.5),
        p.muted,
    );
    // The way to the summary, then the total in the room left.
    let open = super::summary_panel::is_open(ui.ctx());
    let link = painter.text(
        Pos2::new(head.right() - 6., bottom),
        Align2::RIGHT_CENTER,
        if open { "Summary ‹" } else { "Summary ›" },
        theme::sans(11.5),
        p.accent_bright,
    );
    let room = link.left() - 10. - (head.left() + 6.);
    let candidates = if s.sliced > 0 {
        vec![s.total(), crate::project_summary::duration(s.time_s)]
    } else {
        vec!["no totals yet".to_string()]
    };
    let font = theme::sans(11.5);
    let color = if s.sliced > 0 { p.label } else { p.muted };
    let text = candidates
        .iter()
        .find(|t| {
            painter
                .layout_no_wrap((*t).clone(), font.clone(), color)
                .size()
                .x
                <= room
        })
        .cloned()
        .unwrap_or_default();
    painter.text(
        Pos2::new(head.left() + 6., bottom),
        Align2::LEFT_CENTER,
        text,
        font,
        color,
    );
}

/// One row: the status mark, `Plate 3 · Name` over the status words, a warning marker.
fn draw_row(ui: &egui::Ui, row: Rect, plate: &Plate, hovered: bool) {
    let p = theme::of(ui);
    let painter = ui.painter();
    if plate.current {
        painter.rect_filled(row, 6., p.selected_row);
    } else if hovered {
        painter.rect_filled(row, 6., p.raised);
    }
    let y = row.center().y;
    if plate.status == PlateStatus::Empty {
        // No slice to have: a short dash where the mark goes.
        painter.line_segment(
            [
                Pos2::new(row.left() + 10., y),
                Pos2::new(row.left() + 18., y),
            ],
            egui::Stroke::new(1.4, p.muted),
        );
    } else {
        status_mark(painter, Pos2::new(row.left() + 14., y), &plate.status);
    }
    let text_left = row.left() + 28.;
    let (title, status) = row_text(plate);
    let warn = warns(plate);
    let text_right = row.right() - if warn { 22. } else { 8. };
    let text_width = (text_right - text_left).max(10.);
    let title_color = if plate.current { p.text } else { p.label };
    let mut job = egui::text::LayoutJob::simple_singleline(
        title,
        if plate.current {
            theme::semibold(12.5)
        } else {
            theme::sans(12.5)
        },
        title_color,
    );
    job.wrap = egui::text::TextWrapping::truncate_at_width(text_width);
    let g = painter.layout_job(job);
    painter.galley(
        Pos2::new(text_left, y - 8. - g.size().y / 2.),
        g,
        title_color,
    );
    let status_color = match plate.status {
        PlateStatus::Failed { .. } => theme::DANGER,
        PlateStatus::Sliced { .. } => p.muted_alt,
        _ => p.muted,
    };
    let mut job = egui::text::LayoutJob::simple_singleline(status, theme::sans(11.5), status_color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(text_width);
    let g = painter.layout_job(job);
    painter.galley(
        Pos2::new(text_left, y + 8. - g.size().y / 2.),
        g,
        status_color,
    );
    if warn {
        painter.circle_filled(
            Pos2::new(row.right() - 12., y - 8.),
            4.,
            if plate.alert > 1 {
                theme::DANGER
            } else {
                theme::WARNING
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plate(index: usize, status: PlateStatus) -> Plate {
        Plate {
            index,
            origin: glam::Vec2::ZERO,
            current: false,
            name: None,
            locked: false,
            objects: 1,
            alert: 0,
            notes: Vec::new(),
            status,
        }
    }

    #[test]
    fn rows_read_number_name_and_status() {
        let mut a = plate(
            2,
            PlateStatus::Sliced {
                time_s: 38. * 60.,
                grams: 11.6,
                filament_mm: 3850.,
                summary: Default::default(),
            },
        );
        a.name = Some("Brackets".into());
        let (title, status) = row_text(&a);
        assert_eq!(title, "Plate 3 · Brackets");
        assert!(
            status.contains("38 min") && status.contains("11.6 g"),
            "{status}"
        );
        assert!(row_tooltip(&a).ends_with("Click to show its preview"));

        let b = plate(0, PlateStatus::NotSliced);
        assert_eq!(row_text(&b), ("Plate 1".into(), "not sliced".into()));
        assert!(row_tooltip(&b).ends_with("Click to show this plate"));

        let c = plate(4, PlateStatus::Empty);
        assert_eq!(row_text(&c).1, "no objects");

        let d = plate(
            5,
            PlateStatus::Failed {
                short: "off the bed".into(),
                message: "An object is outside the bed".into(),
            },
        );
        assert_eq!(row_text(&d).1, "failed: off the bed");
        assert!(row_tooltip(&d).contains("An object is outside the bed"));

        let mut e = plate(1, PlateStatus::Queued);
        e.current = true;
        e.alert = 1;
        assert_eq!(row_text(&e).1, "waiting to slice");
        let tip = row_tooltip(&e);
        assert!(
            tip.contains("warnings") && tip.ends_with("Shown now"),
            "{tip}"
        );

        // The current plate's own warnings are listed by name; another plate's are only
        // known to be there.
        e.notes = vec!["Cube overlaps".into(), "Bracket is outside the bed".into()];
        let tip = row_tooltip(&e);
        assert!(
            tip.contains("2 warnings on this plate:\n• Cube overlaps\n• Bracket"),
            "{tip}"
        );
        let mut f = plate(3, PlateStatus::NotSliced);
        f.alert = 1;
        assert!(row_tooltip(&f).contains("show it to see them"));
        assert!(warns(&f) && warns(&e) && !warns(&b) && !warns(&d));
    }
}
