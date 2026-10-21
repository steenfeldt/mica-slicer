// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Preview sliders (Preview mockup): a vertical double-handle layer slider on the right with a
//! label card (Z, layer n / N, layer time) and tool-change markers, and a horizontal move
//! slider for the top layer with its `move n / N` label. Labels always stay inside the view.
//! The value↔pixel mapping is kept in plain functions so it can be tested without a UI.
//! With a plate's custom G-code the upper handle gets a "+" and the track its markers
//! (`layer_gcode`).

use super::{Preview, View};
use crate::theme;
use eframe::egui::{self, Align2, FontId, Pos2, Rect, Sense, Stroke, Vec2};

/// Maps a value in `0..=max` onto a pixel coordinate along `[start, end]`
/// (for the vertical slider `start` is the bottom, so larger values go up).
pub fn to_pixel(value: usize, max: usize, start: f32, end: f32) -> f32 {
    if max == 0 {
        return start;
    }
    start + (end - start) * value as f32 / max as f32
}
/// Inverse of [`to_pixel`], clamped and rounded to the nearest value.
pub fn to_value(pixel: f32, max: usize, start: f32, end: f32) -> usize {
    if max == 0 || (end - start).abs() < 1e-6 {
        return 0;
    }
    let t = ((pixel - start) / (end - start)).clamp(0., 1.);
    (t * max as f32).round() as usize
}
/// Keeps `low <= high` after one handle moved; returns the corrected pair.
pub fn order(low: usize, high: usize, moved_low: bool) -> (usize, usize) {
    if low <= high {
        (low, high)
    } else if moved_low {
        (low, low)
    } else {
        (high, high)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Handle {
    Low,
    High,
}

const TRACK_W: f32 = 4.;
const HANDLE_R: f32 = 10.;

/// Draws the vertical layer slider inside `rect` (`bounds` is the whole viewport, for
/// clamping labels). Returns true when the view changed.
pub fn layer_slider(
    ui: &mut egui::Ui,
    rect: Rect,
    bounds: Rect,
    preview: &Preview,
    view: &mut View,
    gcode: Option<&super::layer_gcode::LayerGCode<'_>>,
    edits: &mut Vec<super::layer_gcode::Edit>,
) -> bool {
    let p = theme::of(ui);
    let max = preview.layers.len().saturating_sub(1);
    view.high = view.high.min(max);
    view.low = view.low.min(view.high);
    let id = ui.scope_id().with("layer slider");
    let response = ui.interact(rect, id, Sense::click_and_drag());
    let painter = ui.painter_at(bounds);
    let track_x = rect.right() - 18. - TRACK_W / 2.;
    let margin = HANDLE_R + 2.;
    let (bottom, top) = (rect.bottom() - margin, rect.top() + margin);
    let y_of = |v: usize| to_pixel(v, max, bottom, top);
    painter.rect_filled(
        Rect::from_min_max(
            Pos2::new(track_x - TRACK_W / 2., top),
            Pos2::new(track_x + TRACK_W / 2., bottom),
        ),
        2.,
        p.border_soft,
    );
    painter.rect_filled(
        Rect::from_min_max(
            Pos2::new(track_x - TRACK_W / 2., y_of(view.high)),
            Pos2::new(track_x + TRACK_W / 2., y_of(view.low)),
        ),
        2.,
        p.accent,
    );
    // Tool-change markers.
    for &layer in &preview.stats.tool_changes {
        let y = y_of(layer.min(max));
        painter.line_segment(
            [Pos2::new(track_x - 10., y), Pos2::new(track_x - 5., y)],
            Stroke::new(2., theme::WARNING),
        );
    }
    let mut changed = false;
    let mut active: Option<Handle> = ui.data(|d| d.get_temp(id));
    if (response.drag_started() || response.clicked())
        && let Some(pt) = response.interact_pointer_pos()
    {
        let dl = (pt.y - y_of(view.low)).abs();
        let dh = (pt.y - y_of(view.high)).abs();
        let grabbed = if dl < dh || (dl == dh && pt.y > y_of(view.high)) {
            Handle::Low
        } else {
            Handle::High
        };
        active = Some(grabbed);
        // Stored as `Handle` (not `Option<Handle>`): `get_temp` above reads it back by that type, so a
        // mismatch made every drag stop after its first frame.
        ui.data_mut(|d| d.insert_temp(id, grabbed));
    }
    if (response.dragged() || response.clicked())
        && let (Some(h), Some(pt)) = (active, response.interact_pointer_pos())
    {
        let v = to_value(pt.y, max, bottom, top);
        let (low, high) = match h {
            Handle::Low => order(v, view.high, true),
            Handle::High => order(view.low, v, false),
        };
        if (low, high) != (view.low, view.high) {
            view.low = low;
            view.high = high;
            view.moves = u64::MAX;
            changed = true;
        }
    }
    if response.drag_stopped() {
        ui.data_mut(|d| d.remove::<Handle>(id));
    }
    // The arrow keys move the handles through `keymap::preview_keys`.
    for (handle, value) in [(Handle::Low, view.low), (Handle::High, view.high)] {
        let y = y_of(value);
        let c = Pos2::new(track_x, y);
        let is_high = handle == Handle::High;
        if is_high {
            painter.circle_filled(c, HANDLE_R + 3., p.accent);
        }
        painter.circle_filled(c, HANDLE_R, p.text);
        let layer = preview.layers.get(value);
        let z = layer.map_or(0., |l| l.z);
        if is_high {
            // Label card: Z, layer n / N, layer time; right-aligned, left of the handle.
            let lines = [
                (format!("{z:.2} mm"), p.text),
                (format!("layer {} / {}", value + 1, max + 1), p.muted),
                (
                    super::stats::format_seconds(layer.map_or(0., |l| l.time_s as f64)),
                    p.muted,
                ),
            ];
            let font = theme::mono(12.);
            let galleys: Vec<_> = lines
                .iter()
                .map(|(t, c)| painter.layout_no_wrap(t.clone(), font.clone(), *c))
                .collect();
            let w = galleys.iter().map(|g| g.size().x).fold(0f32, f32::max) + 16.;
            let h: f32 = galleys.iter().map(|g| g.size().y + 1.).sum::<f32>() + 8.;
            // Room for the "+" between the handle and the card.
            let gap = if gcode.is_some() { 46. } else { 22. };
            let mut card =
                Rect::from_min_size(Pos2::new(track_x - gap - w, y - 8.), Vec2::new(w, h));
            card = card.translate(crate::viewport::clamp_shift(bounds, card));
            painter.rect_filled(card, 6., p.panel);
            painter.rect_stroke(
                card,
                6.,
                Stroke::new(1., p.border_soft),
                egui::StrokeKind::Inside,
            );
            let mut yy = card.top() + 4.;
            for (g, (_, c)) in galleys.into_iter().zip(lines.iter()) {
                let gw = g.size().x;
                let gh = g.size().y;
                painter.galley(Pos2::new(card.right() - 8. - gw, yy), g, *c);
                yy += gh + 1.;
            }
        } else {
            let text = format!("{z:.2} mm");
            let g = painter.layout_no_wrap(text, theme::mono(12.), p.muted);
            let mut r = Rect::from_min_size(
                Pos2::new(track_x - 22. - g.size().x, y - g.size().y / 2.),
                g.size(),
            );
            r = r.translate(crate::viewport::clamp_shift(bounds, r));
            painter.galley(r.min, g, p.muted);
        }
    }
    if let Some(g) = gcode {
        let zs: Vec<f64> = preview.layer_zs();
        let mut high = view.high;
        let before = view.layer_ui.clone();
        let mut state = before.clone();
        if super::layer_gcode::ui(
            ui, bounds, track_x, &zs, &y_of, &mut high, &mut state, g, edits,
        ) {
            view.high = high.max(view.low);
            view.moves = u64::MAX;
            changed = true;
        }
        if state != before {
            changed = true;
        }
        view.layer_ui = state;
    }
    changed
}

/// Horizontal move slider for the top visible layer. Returns true when the view changed.
pub fn move_slider(
    ui: &mut egui::Ui,
    rect: Rect,
    bounds: Rect,
    preview: &Preview,
    view: &mut View,
) -> bool {
    let p = theme::of(ui);
    let count = preview
        .layers
        .get(view.high.min(preview.layers.len().saturating_sub(1)))
        .map_or(0, |l| l.count);
    let max = count as usize;
    let current = (view.moves.min(count)) as usize;
    let id = ui.scope_id().with("move slider");
    let response = ui.interact(rect, id, Sense::click_and_drag());
    let painter = ui.painter_at(bounds);
    let y = rect.center().y;
    let margin = HANDLE_R;
    let (left, right) = (rect.left() + margin, rect.right() - margin);
    let x_of = |v: usize| to_pixel(v, max, left, right);
    painter.rect_filled(
        Rect::from_min_max(
            Pos2::new(left, y - TRACK_W / 2.),
            Pos2::new(right, y + TRACK_W / 2.),
        ),
        2.,
        p.border_soft,
    );
    painter.rect_filled(
        Rect::from_min_max(
            Pos2::new(left, y - TRACK_W / 2.),
            Pos2::new(x_of(current), y + TRACK_W / 2.),
        ),
        2.,
        p.accent,
    );
    let mut changed = false;
    if (response.dragged() || response.clicked())
        && let Some(pt) = response.interact_pointer_pos()
    {
        let v = to_value(pt.x, max, left, right) as u64;
        if v != view.moves.min(count) {
            view.moves = v;
            changed = true;
        }
    }
    let hx = x_of(current);
    painter.circle_filled(Pos2::new(hx, y), HANDLE_R, p.text);
    let g = painter.layout_no_wrap(format!("move {current} / {max}"), theme::mono(12.), p.label);
    let mut r = Rect::from_min_size(Pos2::new(hx - g.size().x / 2., y - 36. + 6.), g.size());
    r = r.translate(crate::viewport::clamp_shift(bounds, r));
    painter.galley(r.min, g, p.label);
    let _ = (Align2::CENTER_CENTER, FontId::default());
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mapping_roundtrip_and_clamping() {
        // Vertical: bottom at 300, top at 20.
        assert_eq!(to_pixel(0, 100, 300., 20.), 300.);
        assert_eq!(to_pixel(100, 100, 300., 20.), 20.);
        assert_eq!(to_pixel(50, 100, 300., 20.), 160.);
        for v in [0, 1, 37, 99, 100] {
            assert_eq!(to_value(to_pixel(v, 100, 300., 20.), 100, 300., 20.), v);
        }
        assert_eq!(to_value(1000., 100, 300., 20.), 0);
        assert_eq!(to_value(-1000., 100, 300., 20.), 100);
        assert_eq!(to_value(50., 0, 300., 20.), 0);
        assert_eq!(to_pixel(5, 0, 300., 20.), 300.);
    }
    #[test]
    fn handles_never_cross() {
        assert_eq!(order(3, 7, true), (3, 7));
        assert_eq!(order(9, 7, true), (9, 9));
        assert_eq!(order(9, 7, false), (7, 7));
    }
    #[test]
    fn labels_are_pulled_inside_the_viewport() {
        let bounds = Rect::from_min_max(Pos2::new(0., 0.), Pos2::new(800., 600.));
        // A label hanging off the right edge and above the top moves back in.
        let label = Rect::from_min_size(Pos2::new(760., -10.), Vec2::new(100., 30.));
        let shift = crate::viewport::clamp_shift(bounds, label);
        let moved = label.translate(shift);
        assert!(bounds.contains_rect(moved), "{moved:?}");
        // A label already inside does not move.
        let inside = Rect::from_min_size(Pos2::new(100., 100.), Vec2::new(50., 20.));
        assert_eq!(crate::viewport::clamp_shift(bounds, inside), Vec2::ZERO);
    }
}
