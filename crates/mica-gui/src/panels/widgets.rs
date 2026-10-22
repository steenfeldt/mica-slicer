// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The control vocabulary of the mockups: one fixed-width control column (132 × 26, radius 5,
//! mono numbers with the unit in a muted colour), toggle switches (34 × 20), dropdowns with
//! pictograms, 28 px icon buttons, the 32 px search field, uppercase 11 px card headers,
//! full-width combo buttons, chips and the two header button styles. Everything is sized by
//! the constants below so rows line up.

use super::icons;
use crate::theme;
use eframe::egui::{self, Align2, Color32, FontId, Id, Pos2, Rect, Sense, Stroke, Vec2};

/// Width and height of every control in the settings column.
pub const CONTROL_W: f32 = 132.;
pub const CONTROL_H: f32 = 26.;
/// Height of a settings row.
pub const ROW_H: f32 = 30.;
/// Inner margin between the control column and the panel edge.
pub const MARGIN: f32 = 16.;
/// Left padding of a row (room for the changed marker bar).
pub const LEFT: f32 = 18.;
/// Width reserved for the reset icon between label and control.
pub const RESET_W: f32 = 24.;
/// Height of combo buttons (presets) and of the search field.
pub const COMBO_H: f32 = 34.;
pub const SEARCH_H: f32 = 32.;
/// Icon button size (cards, toolbars).
pub const ICON_BUTTON: f32 = 28.;
const RADIUS: f32 = 5.;
const PAD: f32 = 8.;

/// The control rect of a row: right-aligned in the column, vertically centred.
pub fn control_rect(row: Rect) -> Rect {
    Rect::from_min_size(
        Pos2::new(
            row.right() - MARGIN - CONTROL_W,
            row.center().y - CONTROL_H / 2.,
        ),
        Vec2::new(CONTROL_W, CONTROL_H),
    )
}

fn body_font() -> FontId {
    theme::sans(13.)
}
fn number_font() -> FontId {
    theme::mono(13.)
}

/// Draws the standard field background. `accent` outlines a changed value.
pub fn frame(ui: &egui::Ui, rect: Rect, hot: bool, enabled: bool) {
    frame_with(ui, rect, hot, enabled, false);
}
pub fn frame_with(ui: &egui::Ui, rect: Rect, hot: bool, enabled: bool, accent: bool) {
    let p = theme::of(ui);
    let painter = ui.painter();
    let mut fill = p.field;
    if !enabled {
        fill = fill.gamma_multiply(0.6);
    }
    painter.rect_filled(rect, RADIUS, fill);
    let stroke = if hot || accent {
        Stroke::new(1., p.accent)
    } else {
        Stroke::new(1., p.field_border)
    };
    painter.rect_stroke(rect, RADIUS, stroke, egui::StrokeKind::Inside);
}

pub struct NumberSpec<'a> {
    pub unit: &'a str,
    pub decimals: usize,
    pub min: f64,
    pub max: f64,
    /// Value change per dragged pixel.
    pub speed: f64,
}

/// `0.20`, `15`, `35`: fixed decimals, no trailing noise beyond them.
pub fn format_number(v: f64, decimals: usize) -> String {
    // Values that round to zero print as `0`, never `-0`.
    let factor = 10f64.powi(decimals as i32);
    let v = if (v * factor).round() == 0. { 0. } else { v };
    if decimals == 0 {
        format!("{:.0}", v)
    } else {
        format!("{v:.decimals$}")
    }
}

/// Sensible precision for a value with a unit: millimetres get two decimals, percentages
/// and degrees whole numbers unless fractional, everything else what the value needs.
pub fn decimals_for(unit: &str, v: f64) -> usize {
    let fractional = (v - v.round()).abs() > 1e-6;
    match unit {
        "mm" => {
            if (v * 100. - (v * 100.).round()).abs() > 1e-6 {
                3
            } else {
                2
            }
        }
        "%" | "°" | "mm/s" | "mm/s²" | "°C" | "s" | "layers" | "loops" | "mm³/s" => {
            if fractional {
                1
            } else {
                0
            }
        }
        _ => {
            if !fractional {
                0
            } else if (v * 100. - (v * 100.).round()).abs() > 1e-6 {
                3
            } else {
                2
            }
        }
    }
}

/// Draws a right-aligned value and its unit inside `rect`, returning the x where the text ends
/// on the left.
fn paint_value(ui: &egui::Ui, rect: Rect, value: &str, unit: &str, enabled: bool) -> f32 {
    let p = theme::of(ui);
    let painter = ui.painter();
    let text_color = if enabled { p.text } else { p.muted };
    let mut x = rect.right() - PAD;
    if !unit.is_empty() {
        let galley = painter.layout_no_wrap(unit.to_string(), theme::sans(12.), p.muted);
        painter.galley(
            Pos2::new(x - galley.size().x, rect.center().y - galley.size().y / 2.),
            galley.clone(),
            p.muted,
        );
        x -= galley.size().x + 6.;
    }
    let galley = painter.layout_no_wrap(value.to_string(), number_font(), text_color);
    painter.galley(
        Pos2::new(x - galley.size().x, rect.center().y - galley.size().y / 2.),
        galley.clone(),
        text_color,
    );
    x - galley.size().x
}

/// The id of the number field whose inline editor is open, if any (one at a time).
fn editing_marker() -> Id {
    Id::unique("mica-field-editing")
}

/// Whether a number field is being typed into. Keyboard shortcuts (Esc deselects, M / R / S
/// pick tools) must stay out of the way then, also on the frame egui drops the focus on Esc.
pub fn field_editing(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp::<Id>(editing_marker())).is_some()
}

/// Closes an open inline editor without committing it (its card disappeared, for example).
pub fn cancel_field_edit(ctx: &egui::Context) {
    if let Some(id) = ctx.data(|d| d.get_temp::<Id>(editing_marker())) {
        ctx.data_mut(|d| {
            d.remove::<String>(id.with("editing"));
            d.remove::<bool>(id.with("opened"));
            d.remove::<Id>(editing_marker());
        });
    }
}

/// Opens the inline editor of field `id` with `text` as its buffer.
fn open_edit(ui: &egui::Ui, id: Id, text: String) {
    cancel_field_edit(ui.ctx());
    ui.data_mut(|d| {
        d.insert_temp(id.with("editing"), text);
        d.insert_temp(editing_marker(), id);
    });
    ui.ctx().request_repaint();
}

/// The inline text editor used by the number fields. `None` when the field is not being
/// edited; `Some(None)` while typing goes on or after Esc reverted it; `Some(Some(text))` when
/// Enter or a focus loss committed the buffer. The buffer lives in egui's memory, so the value
/// the caller passes every frame never overwrites what is being typed; the whole value is
/// selected when the editor opens, so typing replaces it.
fn inline_edit(ui: &mut egui::Ui, id: Id, rect: Rect) -> Option<Option<String>> {
    let edit_id = id.with("editing");
    let mut text = ui.data(|d| d.get_temp::<String>(edit_id))?;
    let te_id = id.with("text");
    let opened_id = id.with("opened");
    let fresh = !ui.data(|d| d.get_temp::<bool>(opened_id).unwrap_or(false));
    if fresh {
        // Focus first, then select everything: the TextEdit keeps a stored selection only
        // for the widget that already has focus when it is shown.
        ui.memory_mut(|m| m.request_focus(te_id));
        let mut state = egui::text_edit::TextEditState::load(ui.ctx(), te_id).unwrap_or_default();
        state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::select_all_str(&text)));
        state.store(ui.ctx(), te_id);
        ui.data_mut(|d| d.insert_temp(opened_id, true));
    }
    let response = ui.put(
        rect,
        egui::TextEdit::singleline(&mut text)
            .id(te_id)
            .font(number_font())
            .align(Align2::RIGHT_CENTER)
            .margin(egui::Margin::symmetric(PAD as i8, 4))
            .desired_width(rect.width()),
    );
    let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
    let escape = ui.input(|i| i.key_pressed(egui::Key::Escape));
    let done = |ui: &mut egui::Ui| {
        ui.data_mut(|d| {
            d.remove::<String>(edit_id);
            d.remove::<bool>(opened_id);
            d.remove::<Id>(editing_marker());
        });
        ui.memory_mut(|m| m.surrender_focus(te_id));
    };
    if escape {
        // egui already dropped the focus; the buffer is discarded and the field shows the
        // value it is given again.
        done(ui);
        return Some(None);
    }
    if response.lost_focus() || (enter && response.has_focus()) {
        done(ui);
        return Some(Some(text));
    }
    ui.data_mut(|d| d.insert_temp(edit_id, text));
    Some(None)
}

fn parse_number(text: &str, unit: &str, spec: &NumberSpec<'_>) -> Option<f64> {
    text.trim()
        .trim_end_matches(unit)
        .trim()
        .replace(',', ".")
        .parse::<f64>()
        .ok()
        // `nan`, `inf` and `1e400` parse as floats; none is a value a field can take.
        .filter(|v| v.is_finite())
        .map(|v| v.clamp(spec.min, spec.max))
}

/// A compact number field: right-aligned mono value with the unit in a muted colour; drag to
/// change, click to type. Returns the new value when it changed. `accent` outlines the field
/// (changed from the preset).
pub fn number_with(
    ui: &mut egui::Ui,
    id: Id,
    rect: Rect,
    value: f64,
    spec: &NumberSpec<'_>,
    enabled: bool,
    accent: bool,
) -> Option<f64> {
    if enabled && let Some(result) = inline_edit(ui, id, rect) {
        return result
            .and_then(|t| parse_number(&t, spec.unit, spec))
            .filter(|v| (v - value).abs() > 1e-9);
    }
    let response = ui.allocate_rect(
        rect,
        if enabled {
            Sense::click_and_drag()
        } else {
            Sense::hover()
        },
    );
    let hot = enabled && (response.hovered() || response.dragged());
    frame_with(ui, rect, hot, enabled, accent);
    paint_value(
        ui,
        rect,
        &format_number(value, spec.decimals),
        spec.unit,
        enabled,
    );
    if hot {
        ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::ResizeHorizontal);
    }
    if !enabled {
        return None;
    }
    if response.dragged() && response.drag_delta().x != 0. {
        let shift = ui.input(|i| i.modifiers.shift);
        let step = spec.speed * if shift { 0.1 } else { 1. };
        let next = (value + response.drag_delta().x as f64 * step).clamp(spec.min, spec.max);
        // Keep the shown precision so dragging never produces 0.20000001.
        let factor = 10f64.powi(spec.decimals.max(2) as i32);
        let next = (next * factor).round() / factor;
        return ((next - value).abs() > 1e-12).then_some(next);
    }
    if response.clicked() || response.double_clicked() {
        open_edit(ui, id, format_number(value, spec.decimals));
    }
    None
}

/// Two whole numbers in one field, shown as `5 / 3 layers` (top / bottom shell layers).
/// Dragging the left half changes the first value, the right half the second; a click opens
/// typing of `a / b`. Returns the changed pair.
#[allow(clippy::too_many_arguments)]
pub fn pair_number(
    ui: &mut egui::Ui,
    id: Id,
    rect: Rect,
    values: [i64; 2],
    unit: &str,
    range: (i64, i64),
    enabled: bool,
    accent: bool,
) -> Option<[i64; 2]> {
    let text = format!("{} / {}", values[0], values[1]);
    if enabled && let Some(result) = inline_edit(ui, id, rect) {
        return result.and_then(|t| {
            let t = t.trim().trim_end_matches(unit);
            let (a, b) = t.split_once('/').or_else(|| t.split_once(' '))?;
            let a: i64 = a.trim().parse().ok()?;
            let b: i64 = b.trim().parse().ok()?;
            let next = [a.clamp(range.0, range.1), b.clamp(range.0, range.1)];
            (next != values).then_some(next)
        });
    }
    let response = ui.allocate_rect(
        rect,
        if enabled {
            Sense::click_and_drag()
        } else {
            Sense::hover()
        },
    );
    let hot = enabled && (response.hovered() || response.dragged());
    frame_with(ui, rect, hot, enabled, accent);
    let text_left = paint_value(ui, rect, &text, unit, enabled);
    if !enabled {
        return None;
    }
    // Which half the pointer is on decides which value a drag changes.
    let half = ui.input(|i| i.pointer.latest_pos()).map_or(0, |p| {
        let split = (text_left + rect.right() - PAD) * 0.5;
        usize::from(p.x > split)
    });
    if hot {
        ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::ResizeHorizontal);
    }
    if response.drag_started() {
        ui.data_mut(|d| d.insert_temp(id.with("half"), half));
    }
    if response.dragged() && response.drag_delta().x != 0. {
        let which: usize = ui.data(|d| d.get_temp(id.with("half"))).unwrap_or(half);
        let acc_id = id.with("acc");
        let mut acc: f32 = ui.data(|d| d.get_temp(acc_id)).unwrap_or(0.);
        acc += response.drag_delta().x;
        let steps = (acc / 8.).trunc();
        acc -= steps * 8.;
        ui.data_mut(|d| d.insert_temp(acc_id, acc));
        if steps != 0. {
            let mut next = values;
            next[which] = (values[which] + steps as i64).clamp(range.0, range.1);
            return (next != values).then_some(next);
        }
        return None;
    }
    if response.clicked() || response.double_clicked() {
        open_edit(ui, id, text);
    }
    None
}

/// A toggle switch (34 × 20) at the left edge of `rect`, as the mockup places it in the
/// control column. Returns the new state when clicked.
pub fn toggle(ui: &mut egui::Ui, id: Id, rect: Rect, on: bool, enabled: bool) -> Option<bool> {
    let size = Vec2::new(34., 20.);
    let r = Rect::from_min_size(Pos2::new(rect.left(), rect.center().y - size.y / 2.), size);
    toggle_at(ui, id, r, on, enabled)
}
/// A toggle switch filling `r` (34 × 20 expected).
pub fn toggle_at(ui: &mut egui::Ui, id: Id, r: Rect, on: bool, enabled: bool) -> Option<bool> {
    let p = theme::of(ui);
    let response = ui.interact(
        r,
        id,
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    let t = ui.ctx().animate_bool(id.with("anim"), on);
    let painter = ui.painter();
    let mut fill = lerp_color(p.toggle_off, p.accent, t);
    if !enabled {
        fill = fill.gamma_multiply(0.5);
    }
    painter.rect_filled(r, r.height() / 2., fill);
    if enabled && response.hovered() {
        painter.rect_stroke(
            r,
            r.height() / 2.,
            Stroke::new(1., p.accent_bright),
            egui::StrokeKind::Inside,
        );
    }
    let knob_r = (r.height() - 6.) / 2.;
    let knob_x = egui::lerp((r.left() + 3. + knob_r)..=(r.right() - 3. - knob_r), t);
    painter.circle_filled(
        Pos2::new(knob_x, r.center().y),
        knob_r,
        lerp_color(p.toggle_knob, p.toggle_knob_on, t),
    );
    if enabled && response.hovered() {
        ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
    }
    (enabled && response.clicked()).then_some(!on)
}

fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

/// Height of one row of a dropdown popup.
pub const DROPDOWN_ROW_H: f32 = 28.;
/// Tallest popup before it scrolls.
pub const DROPDOWN_MAX_H: f32 = 360.;

/// A dropdown: the current label (with its pictogram when the option has one) and a chevron;
/// the popup lists `items` as `(value, label)` rows with pictograms in a fixed 16 px column.
/// Every row spans the popup's width and is clickable as a whole; the hovered row gets a
/// raised fill, the current value an accent-tinted row, a violet check mark and medium text, the
/// keyboard focus (↑ ↓, Enter, Esc) an accent outline. The popup is at least as wide as the
/// control and as its longest label, opens below the control (above when there is no room),
/// aligned with its left edge, scrolls beyond 360 px and scrolls the current value into view
/// when it opens. Returns the chosen value. `accent` outlines a changed value.
#[allow(clippy::too_many_arguments)]
pub fn dropdown_with(
    ui: &mut egui::Ui,
    id: Id,
    rect: Rect,
    current_value: &str,
    current_label: &str,
    icon_key: Option<&str>,
    items: &[(String, String)],
    enabled: bool,
    accent: bool,
) -> Option<String> {
    let p = theme::of(ui);
    let response = ui.allocate_rect(
        rect,
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    let popup_id = id.with("menu");
    let open = egui::Popup::is_id_open(ui.ctx(), popup_id);
    frame_with(
        ui,
        rect,
        enabled && (response.hovered() || open),
        enabled,
        accent,
    );
    let text_color = if enabled { p.text } else { p.muted };
    let mut left = rect.left() + PAD;
    if let Some(key) = icon_key {
        let icon_rect =
            Rect::from_center_size(Pos2::new(left + 7., rect.center().y), Vec2::splat(14.));
        if icons::paint(ui, icon_rect, key, current_value) {
            left += 20.;
        }
    }
    let right = rect.right() - PAD - 12.;
    let painter = ui.painter();
    let galley = painter.layout(
        current_label.to_string(),
        body_font(),
        text_color,
        (right - left).max(10.),
    );
    let clip = Rect::from_min_max(Pos2::new(left, rect.top()), Pos2::new(right, rect.bottom()));
    painter.with_clip_rect(clip).galley(
        Pos2::new(
            left,
            rect.center().y - galley.rows.first().map_or(7., |r| r.height() / 2.),
        ),
        galley,
        text_color,
    );
    chevron(
        painter,
        Pos2::new(rect.right() - PAD - 4., rect.center().y),
        true,
        p.muted,
    );
    if !enabled {
        return None;
    }
    if response.hovered() {
        ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
    }
    // Width: the control, or the longest label plus icon column, check mark and padding.
    let has_icons = icon_key.is_some();
    let longest = items
        .iter()
        .map(|(_, l)| {
            painter
                .layout_no_wrap(l.clone(), theme::medium(13.), p.text)
                .size()
                .x
        })
        .fold(0f32, f32::max);
    let width = rect
        .width()
        .max(longest + 2. * 10. + if has_icons { 22. } else { 0. } + 26.)
        .ceil();
    let selected_index = items.iter().position(|(v, _)| v == current_value);
    let focus_id = popup_id.with("focus");
    let was_open_id = popup_id.with("was-open");
    let mut chosen = None;
    egui::Popup::menu(&response)
        .id(popup_id)
        .align(egui::RectAlign::BOTTOM_START)
        .gap(4.)
        .width(width)
        .frame(
            egui::Frame::new()
                .fill(p.raised)
                .stroke(Stroke::new(1., p.border_raised))
                .corner_radius(egui::CornerRadius::same(8))
                .inner_margin(egui::Margin::symmetric(0, 4)),
        )
        .show(|ui| {
            let just_opened = !ui.data(|d| d.get_temp::<bool>(was_open_id).unwrap_or(false));
            ui.data_mut(|d| d.insert_temp(was_open_id, true));
            let mut focus: Option<usize> = if just_opened {
                selected_index
            } else {
                ui.data(|d| d.get_temp(focus_id)).flatten()
            };
            // Keyboard: ↑ ↓ move the focus, Enter picks it (Esc closes the popup itself).
            let (up, down, enter) = ui.input(|i| {
                (
                    i.key_pressed(egui::Key::ArrowUp),
                    i.key_pressed(egui::Key::ArrowDown),
                    i.key_pressed(egui::Key::Enter),
                )
            });
            if !items.is_empty() {
                if down {
                    focus = Some(focus.map_or(0, |f| (f + 1).min(items.len() - 1)));
                }
                if up {
                    focus = Some(focus.map_or(items.len() - 1, |f| f.saturating_sub(1)));
                }
            }
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            ui.set_width(width);
            egui::ScrollArea::vertical()
                .max_height(DROPDOWN_MAX_H)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.set_width(width);
                    for (n, (value, label)) in items.iter().enumerate() {
                        let selected = *value == current_value;
                        let (row, r) = ui.allocate_exact_size(
                            Vec2::new(ui.available_width(), DROPDOWN_ROW_H),
                            Sense::click(),
                        );
                        if r.hovered() {
                            ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
                            ui.data_mut(|d| d.insert_temp(focus_id, Some(n)));
                        }
                        let painter = ui.painter();
                        if selected {
                            painter.rect_filled(row, 0., p.accent.gamma_multiply(0.18));
                        } else if r.hovered() {
                            painter.rect_filled(row, 0., p.hover);
                        }
                        if focus == Some(n) && (up || down || just_opened) {
                            ui.scroll_to_rect(row, Some(egui::Align::Center));
                        }
                        if selected && just_opened {
                            ui.scroll_to_rect(row, Some(egui::Align::Center));
                        }
                        if focus == Some(n) && !r.hovered() {
                            painter.rect_stroke(
                                row.shrink2(Vec2::new(4., 1.)),
                                5.,
                                Stroke::new(1., p.accent.gamma_multiply(0.7)),
                                egui::StrokeKind::Inside,
                            );
                        }
                        let mut x = row.left() + 10.;
                        if let Some(key) = icon_key {
                            let icon = Rect::from_center_size(
                                Pos2::new(x + 8., row.center().y),
                                Vec2::splat(16.),
                            );
                            icons::paint(ui, icon, key, value);
                            x += 22.;
                        }
                        painter.text(
                            Pos2::new(x, row.center().y),
                            Align2::LEFT_CENTER,
                            label,
                            if selected {
                                theme::medium(13.)
                            } else {
                                body_font()
                            },
                            if selected { p.accent_hi } else { p.text },
                        );
                        if selected {
                            // Check mark at the right edge.
                            let c = Pos2::new(row.right() - 16., row.center().y);
                            let stroke = Stroke::new(1.8, p.accent_bright);
                            painter.line_segment(
                                [Pos2::new(c.x - 5., c.y), Pos2::new(c.x - 1.5, c.y + 3.5)],
                                stroke,
                            );
                            painter.line_segment(
                                [
                                    Pos2::new(c.x - 1.5, c.y + 3.5),
                                    Pos2::new(c.x + 5., c.y - 3.5),
                                ],
                                stroke,
                            );
                        }
                        if r.clicked() || (enter && focus == Some(n)) {
                            chosen = Some(value.clone());
                            ui.close();
                        }
                    }
                });
            ui.data_mut(|d| d.insert_temp(focus_id, focus));
        });
    if !egui::Popup::is_id_open(ui.ctx(), popup_id) {
        ui.data_mut(|d| d.remove_temp::<bool>(was_open_id));
    }
    chosen
}

/// A single-line text field in the control column; commits on Enter or focus loss.
pub fn text_field(
    ui: &mut egui::Ui,
    id: Id,
    rect: Rect,
    value: &str,
    enabled: bool,
) -> Option<String> {
    let buf_id = id.with("buffer");
    let te_id = id.with("text");
    let focused = ui.memory(|m| m.has_focus(te_id));
    let mut text = if focused {
        ui.data(|d| d.get_temp::<String>(buf_id))
            .unwrap_or_else(|| value.to_string())
    } else {
        value.to_string()
    };
    frame(ui, rect, focused, enabled);
    let response = ui.put(
        rect,
        egui::TextEdit::singleline(&mut text)
            .id(te_id)
            .font(body_font())
            .frame(egui::Frame::NONE)
            .interactive(enabled)
            .margin(egui::Margin::symmetric(PAD as i8, 4))
            .desired_width(rect.width()),
    );
    let out = (response.lost_focus() && text != value).then(|| text.clone());
    ui.data_mut(|d| d.insert_temp(buf_id, text));
    out
}

/// Small painter chevron (`▾` / `▸`) centred at `at`, 12 px, stroke 2.4 as in the mockups.
pub fn chevron(painter: &egui::Painter, at: Pos2, down: bool, color: Color32) {
    let s = 3.5;
    let stroke = Stroke::new(2., color);
    if down {
        painter.line_segment(
            [
                Pos2::new(at.x - s, at.y - s / 2.),
                Pos2::new(at.x, at.y + s / 2.),
            ],
            stroke,
        );
        painter.line_segment(
            [
                Pos2::new(at.x, at.y + s / 2.),
                Pos2::new(at.x + s, at.y - s / 2.),
            ],
            stroke,
        );
    } else {
        painter.line_segment(
            [
                Pos2::new(at.x - s / 2., at.y - s),
                Pos2::new(at.x + s / 2., at.y),
            ],
            stroke,
        );
        painter.line_segment(
            [
                Pos2::new(at.x + s / 2., at.y),
                Pos2::new(at.x - s / 2., at.y + s),
            ],
            stroke,
        );
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Glyph {
    Pencil,
    Save,
    More,
    Search,
    Reset,
    Close,
    Printer,
    Up,
    Down,
    /// Counter-clockwise quarter turn (↺ with a 90° hint).
    RotateCcw,
    RotateCw,
    LayOnFace,
    Star,
    /// A small brush (painted objects in the list, the paint card).
    Brush,
    /// A rubber (Erase).
    Eraser,
    /// Add / remove an instance.
    Plus,
    Minus,
    /// The prime / wipe tower: a block with its purge lines.
    Tower,
    /// Two arrows chasing each other (sync from the printer).
    Sync,
}

/// Paints a glyph into `rect` (about 15 px, stroke 1.8 like the mockup SVGs).
pub fn glyph(painter: &egui::Painter, rect: Rect, glyph: Glyph, color: Color32) {
    let c = rect.center();
    let s = rect.width().min(rect.height()) * 0.5;
    let stroke = Stroke::new(1.6, color);
    let at = |fx: f32, fy: f32| Pos2::new(c.x + (fx - 0.5) * 2. * s, c.y + (fy - 0.5) * 2. * s);
    match glyph {
        Glyph::Pencil => {
            // M4 20h4L19 9l-4-4L4 16v4z
            painter.add(egui::Shape::closed_line(
                vec![
                    at(4. / 24., 20. / 24.),
                    at(8. / 24., 20. / 24.),
                    at(19. / 24., 9. / 24.),
                    at(15. / 24., 5. / 24.),
                    at(4. / 24., 16. / 24.),
                ],
                stroke,
            ));
        }
        Glyph::Save => {
            painter.add(egui::Shape::closed_line(
                vec![
                    at(5. / 24., 4. / 24.),
                    at(16. / 24., 4. / 24.),
                    at(19. / 24., 7. / 24.),
                    at(19. / 24., 20. / 24.),
                    at(5. / 24., 20. / 24.),
                ],
                stroke,
            ));
            painter.add(egui::Shape::line(
                vec![
                    at(8. / 24., 4. / 24.),
                    at(8. / 24., 9. / 24.),
                    at(15. / 24., 9. / 24.),
                    at(15. / 24., 4. / 24.),
                ],
                stroke,
            ));
            painter.add(egui::Shape::line(
                vec![
                    at(8. / 24., 20. / 24.),
                    at(8. / 24., 14. / 24.),
                    at(16. / 24., 14. / 24.),
                    at(16. / 24., 20. / 24.),
                ],
                stroke,
            ));
        }
        Glyph::More => {
            for fx in [5. / 24., 0.5, 19. / 24.] {
                painter.circle_filled(at(fx, 0.5), s * 0.14, color);
            }
        }
        Glyph::Search => {
            let o = at(11. / 24., 11. / 24.);
            painter.circle_stroke(o, s * 7. / 12., Stroke::new(1.8, color));
            painter.line_segment(
                [at(16. / 24., 16. / 24.), at(20. / 24., 20. / 24.)],
                Stroke::new(1.8, color),
            );
        }
        Glyph::Reset | Glyph::RotateCcw => {
            // M4 12a8 8 0 1 0 3-6.2M4 4v5h5
            let r = s * 8. / 12.;
            let pts: Vec<Pos2> = (0..=22)
                .map(|i| {
                    let a = std::f32::consts::PI + i as f32 / 22. * (std::f32::consts::TAU - 0.7);
                    Pos2::new(c.x + r * a.cos(), c.y + r * a.sin())
                })
                .collect();
            painter.add(egui::Shape::line(pts, stroke));
            painter.add(egui::Shape::line(
                vec![
                    at(4. / 24., 4. / 24.),
                    at(4. / 24., 9. / 24.),
                    at(9. / 24., 9. / 24.),
                ],
                stroke,
            ));
        }
        Glyph::RotateCw => {
            let r = s * 8. / 12.;
            let pts: Vec<Pos2> = (0..=22)
                .map(|i| {
                    let a = -i as f32 / 22. * (std::f32::consts::TAU - 0.7);
                    Pos2::new(c.x + r * a.cos(), c.y + r * a.sin())
                })
                .collect();
            painter.add(egui::Shape::line(pts, stroke));
            painter.add(egui::Shape::line(
                vec![
                    at(20. / 24., 4. / 24.),
                    at(20. / 24., 9. / 24.),
                    at(15. / 24., 9. / 24.),
                ],
                stroke,
            ));
        }
        Glyph::Close => {
            painter.line_segment([at(6. / 24., 6. / 24.), at(18. / 24., 18. / 24.)], stroke);
            painter.line_segment([at(18. / 24., 6. / 24.), at(6. / 24., 18. / 24.)], stroke);
        }
        Glyph::Plus => {
            painter.line_segment([at(5. / 24., 12. / 24.), at(19. / 24., 12. / 24.)], stroke);
            painter.line_segment([at(12. / 24., 5. / 24.), at(12. / 24., 19. / 24.)], stroke);
        }
        Glyph::Minus => {
            painter.line_segment([at(5. / 24., 12. / 24.), at(19. / 24., 12. / 24.)], stroke);
        }
        Glyph::Sync => {
            // Two half circles with arrow heads: the top one clockwise, the bottom one back.
            let r = s * 8. / 12.;
            for (start, head) in [(std::f32::consts::PI, true), (0., false)] {
                let pts: Vec<Pos2> = (0..=14)
                    .map(|i| {
                        let a = start + i as f32 / 14. * (std::f32::consts::PI - 0.5);
                        Pos2::new(c.x + r * a.cos(), c.y + r * a.sin())
                    })
                    .collect();
                let end = *pts.last().unwrap();
                painter.add(egui::Shape::line(pts, stroke));
                let (dx, dy) = if head { (-1., 1.) } else { (1., -1.) };
                painter.add(egui::Shape::line(
                    vec![
                        Pos2::new(end.x + dx * s * 0.42, end.y),
                        end,
                        Pos2::new(end.x, end.y + dy * s * 0.42),
                    ],
                    stroke,
                ));
            }
        }
        Glyph::Tower => {
            // A tall block on a brim line, with three purge lines across it.
            painter.rect_stroke(
                Rect::from_min_max(at(7. / 24., 4. / 24.), at(17. / 24., 18. / 24.)),
                1.,
                stroke,
                egui::StrokeKind::Inside,
            );
            painter.line_segment([at(4. / 24., 20. / 24.), at(20. / 24., 20. / 24.)], stroke);
            for y in [8., 11., 14.] {
                painter.line_segment(
                    [at(9. / 24., y / 24.), at(15. / 24., y / 24.)],
                    Stroke::new(1.2, color),
                );
            }
        }
        Glyph::Printer => {
            // rect 3 4 18 16 r2 + M3 15h18M9 9h6
            painter.rect_stroke(
                Rect::from_min_max(at(3. / 24., 4. / 24.), at(21. / 24., 20. / 24.)),
                2.,
                stroke,
                egui::StrokeKind::Inside,
            );
            painter.line_segment([at(3. / 24., 15. / 24.), at(21. / 24., 15. / 24.)], stroke);
            painter.line_segment([at(9. / 24., 9. / 24.), at(15. / 24., 9. / 24.)], stroke);
        }
        Glyph::Up => {
            painter.add(egui::Shape::line(
                vec![
                    at(6. / 24., 15. / 24.),
                    at(0.5, 9. / 24.),
                    at(18. / 24., 15. / 24.),
                ],
                Stroke::new(2.2, color),
            ));
        }
        Glyph::Down => {
            painter.add(egui::Shape::line(
                vec![
                    at(6. / 24., 9. / 24.),
                    at(0.5, 15. / 24.),
                    at(18. / 24., 9. / 24.),
                ],
                Stroke::new(2.2, color),
            ));
        }
        Glyph::LayOnFace => {
            painter.line_segment([at(3. / 24., 20. / 24.), at(21. / 24., 20. / 24.)], stroke);
            painter.add(egui::Shape::closed_line(
                vec![
                    at(7. / 24., 16. / 24.),
                    at(0.5, 6. / 24.),
                    at(17. / 24., 16. / 24.),
                ],
                stroke,
            ));
        }
        Glyph::Brush => {
            painter.line_segment([at(20. / 24., 4. / 24.), at(11. / 24., 13. / 24.)], stroke);
            painter.add(egui::Shape::closed_line(
                vec![
                    at(11. / 24., 13. / 24.),
                    at(6. / 24., 14. / 24.),
                    at(4. / 24., 20. / 24.),
                    at(10. / 24., 18. / 24.),
                ],
                stroke,
            ));
        }
        Glyph::Eraser => {
            painter.add(egui::Shape::closed_line(
                vec![
                    at(4. / 24., 15. / 24.),
                    at(13. / 24., 6. / 24.),
                    at(20. / 24., 13. / 24.),
                    at(12. / 24., 21. / 24.),
                    at(7. / 24., 21. / 24.),
                ],
                stroke,
            ));
            painter.line_segment([at(8. / 24., 11. / 24.), at(16. / 24., 19. / 24.)], stroke);
        }
        Glyph::Star => {
            // M12 3l2.5 5.5L20 9l-4 4 1 6-5-3-5 3 1-6-4-4 5.5-.5z
            painter.add(egui::Shape::closed_line(
                vec![
                    at(0.5, 3. / 24.),
                    at(14.5 / 24., 8.5 / 24.),
                    at(20. / 24., 9. / 24.),
                    at(16. / 24., 13. / 24.),
                    at(17. / 24., 19. / 24.),
                    at(0.5, 16. / 24.),
                    at(7. / 24., 19. / 24.),
                    at(8. / 24., 13. / 24.),
                    at(4. / 24., 9. / 24.),
                    at(9.5 / 24., 8.5 / 24.),
                ],
                stroke,
            ));
        }
    }
}

/// [`Glyph::Star`] filled (a starred item): a triangle fan from its centre, which the star's
/// outline is visible from.
pub fn star_filled(painter: &egui::Painter, rect: Rect, color: Color32) {
    let c = rect.center();
    let s = rect.width().min(rect.height()) * 0.5;
    let at = |fx: f32, fy: f32| Pos2::new(c.x + (fx - 0.5) * 2. * s, c.y + (fy - 0.5) * 2. * s);
    let outline = [
        at(0.5, 3. / 24.),
        at(14.5 / 24., 8.5 / 24.),
        at(20. / 24., 9. / 24.),
        at(16. / 24., 13. / 24.),
        at(17. / 24., 19. / 24.),
        at(0.5, 16. / 24.),
        at(7. / 24., 19. / 24.),
        at(8. / 24., 13. / 24.),
        at(4. / 24., 9. / 24.),
        at(9.5 / 24., 8.5 / 24.),
    ];
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(at(0.5, 12. / 24.), color);
    for p in outline {
        mesh.colored_vertex(p, color);
    }
    let n = outline.len() as u32;
    for i in 0..n {
        mesh.add_triangle(0, 1 + i, 1 + (i + 1) % n);
    }
    painter.add(egui::Shape::mesh(mesh));
    glyph(painter, rect, Glyph::Star, color);
}

/// A 28 px frameless icon button (`.ibtn`): muted glyph, hover fill and brighter glyph.
pub fn icon_button(ui: &mut egui::Ui, g: Glyph, tooltip: &str, enabled: bool) -> egui::Response {
    icon_button_sized(ui, g, tooltip, enabled, ICON_BUTTON)
}
pub fn icon_button_sized(
    ui: &mut egui::Ui,
    g: Glyph,
    tooltip: &str,
    enabled: bool,
    size: f32,
) -> egui::Response {
    let p = theme::of(ui);
    let (rect, response) = ui.allocate_exact_size(
        Vec2::splat(size),
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    let hovered = enabled && response.hovered();
    let color = if !enabled {
        p.muted.gamma_multiply(0.5)
    } else if hovered {
        p.text
    } else {
        p.muted_alt
    };
    if hovered {
        ui.painter().rect_filled(rect, 6., p.hover);
        ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
    }
    glyph(
        ui.painter(),
        Rect::from_center_size(rect.center(), Vec2::splat(15.)),
        g,
        color,
    );
    if tooltip.is_empty() {
        response
    } else {
        response.on_hover_text(tooltip)
    }
}

/// Card header: 11 px uppercase semibold muted title on the left, icon buttons on the right.
pub fn card_header(ui: &mut egui::Ui, title: &str, right: impl FnOnce(&mut egui::Ui)) {
    let p = theme::of(ui);
    ui.horizontal(|ui| {
        ui.set_min_height(ICON_BUTTON);
        ui.label(
            egui::RichText::new(spaced_upper(title))
                .font(theme::semibold(11.))
                .color(p.muted),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 2.;
            right(ui);
        });
    });
}

/// Uppercase with hair spaces between letters, standing in for `letter-spacing: .06em`.
pub fn spaced_upper(text: &str) -> String {
    let mut out = String::new();
    for (i, c) in text.to_uppercase().chars().enumerate() {
        if i > 0 && c != ' ' {
            out.push('\u{200a}');
        }
        out.push(c);
    }
    out
}

/// Small, muted group title (`LAYERS`) with 12 px above.
pub fn group_title(ui: &mut egui::Ui, text: &str) {
    group_title_at(ui, text, LEFT);
}

/// [`group_title`] indented by `indent` (0: flush with the rows below, for panels whose
/// content has no inner indent).
pub fn group_title_at(ui: &mut egui::Ui, text: &str, indent: f32) {
    ui.add_space(10.);
    let p = theme::of(ui);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 16.), Sense::hover());
    ui.painter().text(
        Pos2::new(rect.left() + indent, rect.center().y),
        Align2::LEFT_CENTER,
        spaced_upper(text),
        theme::semibold(10.5),
        p.muted,
    );
}

/// A full-width `.combo` button (34 px, raised surface, radius 6): optional leading content,
/// the name, a muted trailing detail and a chevron. Returns the response of the whole button.
pub fn combo_button(
    ui: &mut egui::Ui,
    id: Id,
    leading: impl FnOnce(&mut egui::Ui),
    text: &str,
    trailing: &str,
    trailing_color: Option<Color32>,
) -> egui::Response {
    let p = theme::of(ui);
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), COMBO_H), Sense::click());
    let open = egui::Popup::is_id_open(ui.ctx(), id.with("menu"));
    let hot = response.hovered() || open;
    ui.painter()
        .rect_filled(rect, 6., if hot { p.hover } else { p.raised });
    ui.painter().rect_stroke(
        rect,
        6.,
        Stroke::new(1., if open { p.accent } else { p.border_raised }),
        egui::StrokeKind::Inside,
    );
    let inner = rect.shrink2(Vec2::new(10., 0.));
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(inner)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    child.spacing_mut().item_spacing.x = 10.;
    leading(&mut child);
    let used = child.min_rect().width();
    let left = inner.left() + if used > 0. { used + 10. } else { 0. };
    let painter = ui.painter();
    let mut right = inner.right() - 12.;
    chevron(
        painter,
        Pos2::new(inner.right() - 5., rect.center().y),
        true,
        p.muted_alt,
    );
    // A leading `● ` becomes a painted 6 px dot (the mockup's "1 change" badge).
    let (dot, trailing) = match trailing.strip_prefix("● ") {
        Some(t) => (true, t),
        None => (false, trailing),
    };
    if !trailing.is_empty() {
        let color = trailing_color.unwrap_or(p.muted);
        let g = painter.layout_no_wrap(trailing.to_string(), theme::sans(12.), color);
        right -= g.size().x + 10.;
        painter.galley(
            Pos2::new(right + 10., rect.center().y - g.size().y / 2.),
            g.clone(),
            color,
        );
        if dot {
            right -= 12.;
            painter.circle_filled(Pos2::new(right + 10. + 3., rect.center().y), 3., color);
        }
    }
    let galley = painter.layout(
        text.to_string(),
        body_font(),
        p.text,
        (right - left - 6.).max(20.),
    );
    let clip = Rect::from_min_max(Pos2::new(left, rect.top()), Pos2::new(right, rect.bottom()));
    painter.with_clip_rect(clip).galley(
        Pos2::new(
            left,
            rect.center().y - galley.rows.first().map_or(7., |r| r.height() / 2.),
        ),
        galley,
        p.text,
    );
    if response.hovered() {
        ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
    }
    response
}

/// A full-width search field (32 px) with a magnifier inside and an optional right-hand
/// annotation (`5 results`). Returns true when the text changed.
pub fn search_field(
    ui: &mut egui::Ui,
    id: Id,
    text: &mut String,
    hint: &str,
    annotation: &str,
) -> bool {
    let p = theme::of(ui);
    let (rect, _) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), SEARCH_H), Sense::hover());
    let te_id = id.with("search");
    let focused = ui.memory(|m| m.has_focus(te_id));
    let painter = ui.painter();
    painter.rect_filled(rect, 6., p.search_bg);
    painter.rect_stroke(
        rect,
        6.,
        Stroke::new(
            1.,
            if focused || !text.is_empty() {
                p.accent
            } else {
                p.border_soft
            },
        ),
        egui::StrokeKind::Inside,
    );
    glyph(
        painter,
        Rect::from_center_size(
            Pos2::new(rect.left() + 17., rect.center().y),
            Vec2::splat(14.),
        ),
        Glyph::Search,
        p.muted,
    );
    let mut right = rect.right();
    if !annotation.is_empty() {
        let g = painter.layout_no_wrap(annotation.to_string(), theme::sans(12.), p.muted);
        right -= g.size().x + 12.;
        painter.galley(
            Pos2::new(right + 2., rect.center().y - g.size().y / 2.),
            g.clone(),
            p.muted,
        );
    }
    if !text.is_empty() {
        right -= 22.;
    }
    // One text row, centred in the field: a taller text box puts the text at its top.
    let row = ui.fonts_mut(|f| f.row_height(&body_font()));
    let inner = Rect::from_min_max(
        Pos2::new(rect.left() + 30., rect.center().y - row / 2.),
        Pos2::new(right, rect.center().y + row / 2.),
    );
    let response = ui.put(
        inner,
        egui::TextEdit::singleline(text)
            .id(te_id)
            .font(body_font())
            .frame(egui::Frame::NONE)
            .hint_text(egui::RichText::new(hint).color(p.muted).font(body_font()))
            .margin(egui::Margin::symmetric(4, 0))
            .desired_width(inner.width()),
    );
    let changed = response.changed();
    if !text.is_empty() {
        let clear =
            Rect::from_center_size(Pos2::new(right + 11., rect.center().y), Vec2::splat(18.));
        let r = ui.interact(clear, id.with("clear"), Sense::click());
        glyph(
            ui.painter(),
            clear.shrink(3.),
            Glyph::Close,
            if r.hovered() { p.text } else { p.muted },
        );
        if r.clicked() {
            text.clear();
            return true;
        }
    }
    changed
}

/// `.btn`: 30 px, raised surface, 1 px border, radius 6.
pub fn button(ui: &mut egui::Ui, text: &str, enabled: bool) -> egui::Response {
    styled_button(ui, text, enabled, false, 30.)
}
/// The one primary action (violet fill, dark text).
pub fn primary_button(ui: &mut egui::Ui, text: &str, enabled: bool) -> egui::Response {
    styled_button(ui, text, enabled, true, 30.)
}
pub fn styled_button(
    ui: &mut egui::Ui,
    text: &str,
    enabled: bool,
    primary: bool,
    height: f32,
) -> egui::Response {
    let p = theme::of(ui);
    let font = if primary {
        theme::semibold(13.)
    } else {
        theme::sans(13.)
    };
    let galley = ui.painter().layout_no_wrap(text.to_string(), font, p.text);
    let size = Vec2::new(galley.size().x + 24., height);
    let (rect, response) = ui.allocate_exact_size(
        size,
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    let hovered = enabled && response.hovered();
    let (fill, stroke, color) = if primary {
        let fill = if hovered { p.accent_bright } else { p.accent };
        (fill, fill, p.on_accent)
    } else {
        (
            if hovered { p.hover } else { p.raised },
            p.border_raised,
            p.text,
        )
    };
    let alpha = if enabled { 1. } else { 0.45 };
    let painter = ui.painter();
    painter.rect_filled(rect, 6., fill.gamma_multiply(alpha));
    painter.rect_stroke(
        rect,
        6.,
        Stroke::new(1., stroke.gamma_multiply(alpha)),
        egui::StrokeKind::Inside,
    );
    painter.galley(
        Pos2::new(
            rect.center().x - galley.size().x / 2.,
            rect.center().y - galley.size().y / 2.,
        ),
        galley,
        color.gamma_multiply(alpha),
    );
    if hovered {
        ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
    }
    response
}

/// A split button: the main part does the usual thing (Slice the current plate), the narrow
/// chevron part on its right opens a menu of alternatives (Slice all plates). Returns the
/// main part's response and the chevron's (open a `Popup::menu` on it). The menu can be open
/// while the main part is disabled (Export all plates before the current plate is sliced).
pub fn split_button(
    ui: &mut egui::Ui,
    text: &str,
    enabled: bool,
    menu_enabled: bool,
    primary: bool,
) -> (egui::Response, egui::Response) {
    let p = theme::of(ui);
    let font = if primary {
        theme::semibold(13.)
    } else {
        theme::sans(13.)
    };
    let galley = ui.painter().layout_no_wrap(text.to_string(), font, p.text);
    const CHEVRON: f32 = 24.;
    let size = Vec2::new(galley.size().x + 24. + CHEVRON, 30.);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let main_rect = Rect::from_min_max(rect.min, Pos2::new(rect.right() - CHEVRON, rect.bottom()));
    let menu_rect = Rect::from_min_max(Pos2::new(rect.right() - CHEVRON, rect.top()), rect.max);
    let sense = |on: bool| if on { Sense::click() } else { Sense::hover() };
    let id = ui.make_persistent_id(("split button", text));
    let main = ui.interact(main_rect, id.with("main"), sense(enabled));
    let menu = ui.interact(menu_rect, id.with("menu"), sense(menu_enabled));
    let open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&menu));
    let (fill, hover_fill, stroke, color) = if primary {
        (p.accent, p.accent_bright, p.accent, p.on_accent)
    } else {
        (p.raised, p.hover, p.border_raised, p.text)
    };
    let alpha = if enabled { 1. } else { 0.45 };
    let menu_alpha = if menu_enabled { 1. } else { 0.45 };
    let painter = ui.painter();
    painter.rect_filled(rect, 6., fill.gamma_multiply(alpha));
    for (part, hot) in [
        (main_rect, enabled && main.hovered()),
        (menu_rect, menu_enabled && (menu.hovered() || open)),
    ] {
        if hot {
            let corner = if part == main_rect {
                egui::CornerRadius {
                    nw: 6,
                    sw: 6,
                    ne: 0,
                    se: 0,
                }
            } else {
                egui::CornerRadius {
                    nw: 0,
                    sw: 0,
                    ne: 6,
                    se: 6,
                }
            };
            painter.rect_filled(part, corner, hover_fill);
        }
    }
    painter.rect_stroke(
        rect,
        6.,
        Stroke::new(1., stroke.gamma_multiply(alpha)),
        egui::StrokeKind::Inside,
    );
    // The divider between the two parts.
    painter.line_segment(
        [
            Pos2::new(menu_rect.left(), rect.top() + 6.),
            Pos2::new(menu_rect.left(), rect.bottom() - 6.),
        ],
        Stroke::new(
            1.,
            if primary {
                p.on_accent
            } else {
                p.border_raised
            }
            .gamma_multiply(alpha * 0.35),
        ),
    );
    painter.galley(
        Pos2::new(
            main_rect.center().x - galley.size().x / 2.,
            main_rect.center().y - galley.size().y / 2.,
        ),
        galley,
        color.gamma_multiply(alpha),
    );
    chevron(
        painter,
        menu_rect.center(),
        true,
        color.gamma_multiply(menu_alpha),
    );
    if (enabled && main.hovered()) || (menu_enabled && menu.hovered()) {
        ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
    }
    (main, menu)
}

/// A small rounded label (the printer card's "Orca rules"): 18 px, clickable.
pub fn badge(ui: &mut egui::Ui, text: &str, accent: bool) -> egui::Response {
    let p = theme::of(ui);
    let color = if accent { p.accent_bright } else { p.muted };
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_string(), theme::sans(11.), color);
    let size = Vec2::new(galley.size().x + 12., 18.);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let painter = ui.painter();
    if response.hovered() {
        painter.rect_filled(rect, 9., p.hover);
        ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
    }
    painter.rect_stroke(
        rect,
        9.,
        Stroke::new(1., if accent { p.accent } else { p.border_raised }),
        egui::StrokeKind::Inside,
    );
    painter.galley(
        Pos2::new(rect.left() + 6., rect.center().y - galley.size().y / 2.),
        galley,
        color,
    );
    response
}

/// A small, quiet status label that is not a button ("Alpha (pre-release)" next to the
/// version): 18 px, accent outline, no hover.
pub fn status_badge(ui: &mut egui::Ui, text: &str) -> egui::Response {
    let p = theme::of(ui);
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_string(), theme::sans(10.5), p.accent_bright);
    let size = Vec2::new(galley.size().x + 12., 18.);
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 9., p.accent_surface);
    painter.rect_stroke(
        rect,
        9.,
        Stroke::new(1., p.accent.gamma_multiply(0.6)),
        egui::StrokeKind::Inside,
    );
    painter.galley(
        Pos2::new(rect.left() + 6., rect.center().y - galley.size().y / 2.),
        galley,
        p.accent_bright,
    );
    response
}

/// A `.chip`: 32 px, radius 7, optional glyph, `accent` draws the highlighted variant.
pub fn chip(
    ui: &mut egui::Ui,
    glyph_: Option<Glyph>,
    text: &str,
    accent: bool,
    enabled: bool,
) -> egui::Response {
    let p = theme::of(ui);
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_string(), theme::sans(13.), p.text);
    let icon_w = if glyph_.is_some() { 15. + 8. } else { 0. };
    let size = Vec2::new(galley.size().x + icon_w + 24., 32.);
    let (rect, response) = ui.allocate_exact_size(
        size,
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    let hovered = enabled && response.hovered();
    let color = if !enabled {
        p.muted
    } else if accent {
        p.accent_hi
    } else {
        p.text
    };
    let painter = ui.painter();
    painter.rect_filled(rect, 7., if hovered { p.hover } else { p.raised });
    painter.rect_stroke(
        rect,
        7.,
        Stroke::new(1., if accent { p.accent } else { p.border_raised }),
        egui::StrokeKind::Inside,
    );
    let mut x = rect.left() + 12.;
    if let Some(g) = glyph_ {
        glyph(
            painter,
            Rect::from_center_size(Pos2::new(x + 7.5, rect.center().y), Vec2::splat(15.)),
            g,
            color,
        );
        x += 15. + 8.;
    }
    painter.galley(
        Pos2::new(x, rect.center().y - galley.size().y / 2.),
        galley,
        color,
    );
    if hovered {
        ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
    }
    response
}

/// Accent-coloured 12 px link text.
pub fn link(ui: &mut egui::Ui, text: &str) -> egui::Response {
    let p = theme::of(ui);
    let r = ui.add(
        egui::Label::new(
            egui::RichText::new(text)
                .font(theme::sans(12.))
                .color(p.accent_bright),
        )
        .sense(Sense::click())
        .selectable(false),
    );
    if r.hovered() {
        ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
    }
    r
}

/// Text with the search match highlighted like the mockup's `<mark>`.
pub fn highlighted(
    text: &str,
    query: &str,
    font: FontId,
    color: Color32,
    mark_bg: Color32,
    mark_fg: Color32,
) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    let plain = egui::TextFormat {
        font_id: font.clone(),
        color,
        ..Default::default()
    };
    let mark = egui::TextFormat {
        font_id: font,
        color: mark_fg,
        background: mark_bg,
        ..Default::default()
    };
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        job.append(text, 0., plain);
        return job;
    }
    let lower = text.to_lowercase();
    let mut rest = 0;
    while let Some(i) = lower[rest..].find(&q) {
        let start = rest + i;
        let end = start + q.len();
        if start > rest {
            job.append(&text[rest..start], 0., plain.clone());
        }
        job.append(&text[start..end], 0., mark.clone());
        rest = end;
    }
    if rest < text.len() {
        job.append(&text[rest..], 0., plain);
    }
    job
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn formatting_and_precision() {
        assert_eq!(format_number(0.2, 2), "0.20");
        assert_eq!(format_number(15., 0), "15");
        assert_eq!(format_number(-0.0001, 0), "0");
        assert_eq!(format_number(-0.004, 2), "0.00");
        assert_eq!(format_number(-0.5, 1), "-0.5");
        assert_eq!(decimals_for("mm", 0.2), 2);
        assert_eq!(decimals_for("mm", 0.125), 3);
        assert_eq!(decimals_for("%", 15.), 0);
        assert_eq!(decimals_for("%", 12.5), 1);
        assert_eq!(decimals_for("", 3.), 0);
        assert_eq!(decimals_for("", 0.95), 2);
        let row = Rect::from_min_size(Pos2::ZERO, Vec2::new(360., ROW_H));
        let c = control_rect(row);
        assert_eq!(c.right(), 360. - MARGIN);
        assert_eq!(c.width(), CONTROL_W);
        assert_eq!(c.height(), CONTROL_H);
        assert_eq!(c.center().y, ROW_H / 2.);
        assert_eq!((CONTROL_W, CONTROL_H, ROW_H), (132., 26., 30.));
    }
    #[test]
    fn search_highlight_splits_matches() {
        let job = highlighted(
            "Staggered inner seams",
            "seam",
            theme::sans(13.),
            Color32::WHITE,
            Color32::BLACK,
            Color32::GRAY,
        );
        assert_eq!(job.sections.len(), 3);
        let r = &job.sections[1].byte_range;
        assert_eq!(&job.text[usize::from(r.start)..usize::from(r.end)], "seam");
        let none = highlighted(
            "Walls",
            "",
            theme::sans(13.),
            Color32::WHITE,
            Color32::BLACK,
            Color32::GRAY,
        );
        assert_eq!(none.sections.len(), 1);
        assert_eq!(
            spaced_upper("Printer"),
            "P\u{200a}R\u{200a}I\u{200a}N\u{200a}T\u{200a}E\u{200a}R"
        );
    }

    /// A headless egui context showing one number field per frame; `run` feeds events and
    /// returns what the field reported. The value passed to the field is the caller's, re-read
    /// every frame as the tool card does, so a stale value must never clobber the buffer.
    struct Field {
        ctx: egui::Context,
        id: Id,
        rect: Rect,
        value: f64,
    }
    impl Field {
        fn new(value: f64) -> Self {
            let ctx = egui::Context::default();
            theme::apply(&ctx, theme::Mode::Dark);
            Self {
                ctx,
                id: Id::unique("test-field"),
                rect: Rect::from_min_size(Pos2::new(100., 100.), Vec2::new(60., 26.)),
                value,
            }
        }
        fn run(&mut self, events: Vec<egui::Event>) -> Option<f64> {
            let spec = NumberSpec {
                unit: "",
                decimals: 1,
                min: 0.1,
                max: 1e6,
                speed: 0.5,
            };
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(400., 300.))),
                events,
                ..Default::default()
            };
            let mut out = None;
            let (id, rect, value) = (self.id, self.rect, self.value);
            let mut output = self.ctx.run_ui(input, |ui| {
                out = number_with(ui, id, rect, value, &spec, true, false);
            });
            // Nothing renders here; the font atlas deltas are dropped on purpose.
            output.textures_delta.clear();
            if let Some(v) = out {
                self.value = v;
            }
            out
        }
        fn buffer(&self) -> Option<String> {
            self.ctx.data(|d| d.get_temp(self.id.with("editing")))
        }
        fn button(&self, at: Pos2, pressed: bool) -> egui::Event {
            egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            }
        }
        fn key(key: egui::Key) -> egui::Event {
            egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }
        }
        /// Click the field open and let the editor take focus with the value selected.
        fn open(&mut self) {
            let c = self.rect.center();
            assert_eq!(self.run(vec![egui::Event::PointerMoved(c)]), None);
            assert_eq!(self.run(vec![self.button(c, true)]), None);
            assert_eq!(self.run(vec![self.button(c, false)]), None);
            assert!(field_editing(&self.ctx));
            assert_eq!(self.run(vec![]), None);
        }
    }

    #[test]
    fn typing_replaces_the_value_and_enter_commits_it() {
        let mut f = Field::new(100.);
        f.open();
        assert_eq!(f.buffer().as_deref(), Some("100.0"));
        // Typing goes over the selected value; the buffer, not the caller's 100, is shown.
        assert_eq!(f.run(vec![egui::Event::Text("200".into())]), None);
        assert_eq!(f.buffer().as_deref(), Some("200"));
        assert_eq!(f.run(vec![]), None);
        assert_eq!(f.buffer().as_deref(), Some("200"));
        assert_eq!(f.run(vec![Field::key(egui::Key::Enter)]), Some(200.));
        assert!(!field_editing(&f.ctx));
        assert_eq!(f.buffer(), None);
        // Closed: the field shows the committed value and reports nothing more.
        assert_eq!(f.run(vec![]), None);
        assert_eq!(f.value, 200.);
    }

    #[test]
    fn escape_reverts_and_focus_loss_commits() {
        let mut f = Field::new(60.);
        f.open();
        f.run(vec![egui::Event::Text("120".into())]);
        assert_eq!(f.run(vec![Field::key(egui::Key::Escape)]), None);
        assert!(!field_editing(&f.ctx));
        assert_eq!(f.value, 60.);
        // Typing with a decimal comma, then clicking elsewhere commits.
        f.open();
        f.run(vec![egui::Event::Text("31,5".into())]);
        let away = Pos2::new(20., 20.);
        f.run(vec![egui::Event::PointerMoved(away)]);
        let committed = f
            .run(vec![f.button(away, true)])
            .or_else(|| f.run(vec![f.button(away, false)]))
            .or_else(|| f.run(vec![]));
        assert_eq!(committed, Some(31.5));
        assert_eq!(f.value, 31.5);
        // Nonsense typed in is dropped, the value stays.
        f.open();
        f.run(vec![egui::Event::Text("abc".into())]);
        assert_eq!(f.run(vec![Field::key(egui::Key::Enter)]), None);
        assert_eq!(f.value, 31.5);
        // Values below the minimum are clamped to it.
        f.open();
        f.run(vec![egui::Event::Text("0".into())]);
        assert_eq!(f.run(vec![Field::key(egui::Key::Enter)]), Some(0.1));
    }

    #[test]
    fn dragging_applies_live_in_shown_precision() {
        let mut f = Field::new(100.);
        let c = f.rect.center();
        f.run(vec![egui::Event::PointerMoved(c)]);
        f.run(vec![f.button(c, true)]);
        // The first move passes egui's drag threshold, every move after it changes the value.
        f.run(vec![egui::Event::PointerMoved(c + Vec2::new(8., 0.))]);
        let mut applied = 0;
        for step in 1..=5 {
            if f.run(vec![egui::Event::PointerMoved(
                c + Vec2::new(8. + 4. * step as f32, 0.),
            )])
            .is_some()
            {
                applied += 1;
            }
        }
        assert!(applied >= 4, "{applied} of 5 moves applied");
        assert!(f.value > 100., "{}", f.value);
        assert_eq!(f.value, (f.value * 100.).round() / 100.);
        // Releasing does not open the editor after a drag.
        f.run(vec![f.button(c + Vec2::new(28., 0.), false)]);
        assert!(!field_editing(&f.ctx));
        assert_eq!(f.buffer(), None);
    }

    #[test]
    fn cancelled_edit_leaves_nothing_behind() {
        let mut f = Field::new(10.);
        f.open();
        f.run(vec![egui::Event::Text("99".into())]);
        cancel_field_edit(&f.ctx);
        assert!(!field_editing(&f.ctx));
        assert_eq!(f.buffer(), None);
        assert_eq!(f.run(vec![]), None);
        assert_eq!(f.value, 10.);
    }

    /// Typed `nan` passed `clamp` unchanged (NaN compares false) and gave objects a NaN
    /// position or scale through the tool card; `inf` and `1e400` became ±f64::MAX.
    #[test]
    fn non_finite_typed_numbers_are_ignored() {
        let spec = NumberSpec {
            unit: "mm",
            decimals: 1,
            min: -1e6,
            max: 1e6,
            speed: 1.,
        };
        for text in ["nan", "NaN", "inf", "-inf", "1e400", "infinity"] {
            assert_eq!(parse_number(text, "mm", &spec), None, "{text}");
        }
        assert_eq!(parse_number("2e7", "mm", &spec), Some(1e6));
        assert_eq!(parse_number("12,5 mm", "mm", &spec), Some(12.5));
    }
}
