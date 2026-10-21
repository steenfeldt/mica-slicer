// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Filament colours: the swatch button, the colour popup (common filament colours, recent
//! picks, a custom picker and a hex field) and the edit behind them.
//!
//! A colour is a visual aid only (PrusaSlicer and Orca both say so), and it is project state,
//! not a change of the filament profile (`Overlays::colours`, like a project's or the
//! printer's colours): picking one repaints the models and the preview at once and the slice
//! is kept. The G-code mentions the colours (settings block, thumbnails), so exporting or
//! printing a slice made before the change slices once more first, unless a slice with
//! these colours is cached (`App::reslice_for_colours`).

use super::widgets;
use crate::app::App;
use crate::theme;
use eframe::egui::{self, Color32, Id, Rect, Sense, Stroke, Vec2};

/// Common filament colours, in the order of a spool rack.
pub const PALETTE: [(&str, [u8; 3]); 16] = [
    ("White", [0xFF, 0xFF, 0xFF]),
    ("Light gray", [0xC8, 0xC8, 0xC8]),
    ("Gray", [0x80, 0x80, 0x80]),
    ("Black", [0x16, 0x16, 0x16]),
    ("Red", [0xD0, 0x2E, 0x2E]),
    ("Orange", [0xF2, 0x79, 0x1F]),
    ("Yellow", [0xF5, 0xD0, 0x2A]),
    ("Natural", [0xE8, 0xDC, 0xB8]),
    ("Green", [0x2E, 0xA0, 0x4F]),
    ("Teal", [0x1E, 0x9C, 0xA0]),
    ("Blue", [0x2B, 0x6C, 0xD8]),
    ("Navy", [0x1F, 0x2E, 0x6B]),
    ("Purple", [0x7B, 0x4A, 0xC2]),
    ("Pink", [0xF0, 0x7C, 0xB4]),
    ("Brown", [0x7A, 0x4E, 0x2D]),
    ("Silver", [0xA8, 0xB0, 0xB8]),
];

/// How many recent picks the popup keeps.
pub const RECENT: usize = 8;

const CELL: f32 = 22.;
const GAP: f32 = 6.;
const COLUMNS: usize = 8;

/// A colour chosen in the popup. `commit` is false while the custom picker is being dragged
/// (the change is shown live; the pick is remembered as recent on release).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pick {
    pub colour: [u8; 3],
    pub commit: bool,
}

fn color32(c: [u8; 3]) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

/// A clickable colour square: hover ring and pointer cursor, accent ring while `open`.
pub fn swatch_button(ui: &mut egui::Ui, colour: [u8; 3], size: f32, open: bool) -> egui::Response {
    let p = theme::of(ui);
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    paint_swatch(ui, rect, colour, response.hovered() || open);
    if response.hovered() {
        ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
    }
    if open {
        ui.painter().rect_stroke(
            rect.expand(2.),
            5.,
            Stroke::new(1.5, p.accent),
            egui::StrokeKind::Inside,
        );
    }
    response
}

fn paint_swatch(ui: &egui::Ui, rect: Rect, colour: [u8; 3], hot: bool) {
    let p = theme::of(ui);
    ui.painter().rect_filled(rect, 3., color32(colour));
    ui.painter().rect_stroke(
        rect,
        3.,
        Stroke::new(1., if hot { p.text } else { p.border_raised }),
        egui::StrokeKind::Inside,
    );
}

/// The colour popup under `anchor` (opened and closed by clicking it). Returns the pick of
/// this frame, if any.
pub fn popup(
    anchor: &egui::Response,
    id: Id,
    title: &str,
    current: [u8; 3],
    recent: &[[u8; 3]],
) -> Option<Pick> {
    egui::Popup::menu(anchor)
        .id(id)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .width(COLUMNS as f32 * (CELL + GAP) - GAP)
        .show(|ui| picker(ui, id, title, current, recent))
        .and_then(|r| r.inner)
}

/// The popup's content (also usable inline).
pub fn picker(
    ui: &mut egui::Ui,
    id: Id,
    title: &str,
    current: [u8; 3],
    recent: &[[u8; 3]],
) -> Option<Pick> {
    let p = theme::of(ui);
    let mut pick = None;
    let width = COLUMNS as f32 * (CELL + GAP) - GAP;
    ui.set_width(width);
    ui.spacing_mut().item_spacing.y = 6.;
    ui.label(egui::RichText::new(title).font(theme::semibold(13.)));
    let grid = |ui: &mut egui::Ui, colours: &mut dyn Iterator<Item = (String, [u8; 3])>| {
        let mut chosen = None;
        let items: Vec<(String, [u8; 3])> = colours.collect();
        for row in items.chunks(COLUMNS) {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = GAP;
                for (name, c) in row {
                    let (rect, r) = ui.allocate_exact_size(Vec2::splat(CELL), Sense::click());
                    paint_swatch(ui, rect, *c, r.hovered());
                    if *c == current {
                        ui.painter().rect_stroke(
                            rect.expand(2.),
                            5.,
                            Stroke::new(1.5, p.accent),
                            egui::StrokeKind::Inside,
                        );
                    }
                    if r.hovered() {
                        ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
                    }
                    if r.on_hover_text(name.as_str()).clicked() {
                        chosen = Some(*c);
                    }
                }
            });
        }
        chosen
    };
    if let Some(c) = grid(ui, &mut PALETTE.iter().map(|(n, c)| (n.to_string(), *c))) {
        pick = Some(Pick {
            colour: c,
            commit: true,
        });
    }
    if !recent.is_empty() {
        ui.add_space(2.);
        ui.label(
            egui::RichText::new("Recent")
                .font(theme::sans(12.))
                .color(p.muted),
        );
        if let Some(c) = grid(
            ui,
            &mut recent
                .iter()
                .take(RECENT)
                .map(|c| (crate::project::hex(*c), *c)),
        ) {
            pick = Some(Pick {
                colour: c,
                commit: true,
            });
        }
    }
    ui.add_space(2.);
    ui.label(
        egui::RichText::new("Custom")
            .font(theme::sans(12.))
            .color(p.muted),
    );
    let (changed, colour) = sv_hue(ui, id.with("custom"), current, width);
    let down = ui.input(|i| i.pointer.any_down());
    let dragging = id.with("dragging");
    if changed {
        if down {
            ui.data_mut(|d| d.insert_temp(dragging, true));
        }
        pick = Some(Pick {
            colour,
            commit: !down,
        });
    } else if !down && ui.data(|d| d.get_temp::<bool>(dragging)).unwrap_or(false) {
        ui.data_mut(|d| d.remove::<bool>(dragging));
        pick = Some(Pick {
            colour: current,
            commit: true,
        });
    }
    // The hex text, applied on Enter or when the field loses focus.
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.;
        ui.label(
            egui::RichText::new("Hex")
                .font(theme::sans(12.))
                .color(p.muted),
        );
        let (rect, _) = ui.allocate_exact_size(
            Vec2::new(ui.available_width(), widgets::COMBO_H),
            Sense::hover(),
        );
        if let Some(text) = widgets::text_field(
            ui,
            id.with("hex"),
            rect,
            &crate::project::hex(current),
            true,
        ) {
            match mica_profiles::project::parse_colour(text.trim())
                .or_else(|| mica_profiles::project::parse_colour(&format!("#{}", text.trim())))
            {
                Some(colour) => {
                    pick = Some(Pick {
                        colour,
                        commit: true,
                    })
                }
                None => {
                    ui.data_mut(|d| d.remove::<String>(id.with("hex").with("buffer")));
                }
            }
        }
    });
    pick
}

/// Where the app leaves the recent colours for the settings editor, and where the editor
/// leaves a committed pick for the app to remember (egui temp data, read every frame).
fn recent_id() -> Id {
    Id::unique("mica-recent-colours")
}
fn committed_id() -> Id {
    Id::unique("mica-committed-colour")
}

/// A compact custom picker: saturation × value square over a hue bar. The hue is kept
/// between frames so greys (which have none) do not snap it back to red.
fn sv_hue(ui: &mut egui::Ui, id: Id, current: [u8; 3], width: f32) -> (bool, [u8; 3]) {
    use egui::ecolor::Hsva;
    let p = theme::of(ui);
    let mut hsv = match ui.data(|d| d.get_temp::<Hsva>(id)) {
        Some(h) if h.to_srgb() == current => h,
        _ => Hsva::from_srgb(current),
    };
    let mut changed = false;
    // Saturation (x) × value (y, bright at the top).
    let (sv, r) = ui.allocate_exact_size(Vec2::new(width, 96.), Sense::click_and_drag());
    let mut mesh = egui::Mesh::default();
    const N: usize = 16;
    for row in 0..=N {
        for col in 0..=N {
            let (s, v) = (col as f32 / N as f32, 1. - row as f32 / N as f32);
            mesh.colored_vertex(
                egui::pos2(
                    sv.left() + s * sv.width(),
                    sv.top() + (1. - v) * sv.height(),
                ),
                Hsva::new(hsv.h, s, v, 1.).into(),
            );
        }
    }
    for row in 0..N as u32 {
        for col in 0..N as u32 {
            let i = row * (N as u32 + 1) + col;
            let j = i + N as u32 + 1;
            mesh.add_triangle(i, i + 1, j + 1);
            mesh.add_triangle(i, j + 1, j);
        }
    }
    ui.painter().add(egui::Shape::mesh(mesh));
    ui.painter().rect_stroke(
        sv,
        3.,
        Stroke::new(1., p.border_raised),
        egui::StrokeKind::Inside,
    );
    if let Some(pos) = r.interact_pointer_pos() {
        hsv.s = ((pos.x - sv.left()) / sv.width()).clamp(0., 1.);
        hsv.v = 1. - ((pos.y - sv.top()) / sv.height()).clamp(0., 1.);
        changed = true;
    }
    let knob = egui::pos2(
        sv.left() + hsv.s * sv.width(),
        sv.top() + (1. - hsv.v) * sv.height(),
    );
    ui.painter()
        .circle_stroke(knob, 5., Stroke::new(2., Color32::WHITE));
    ui.painter()
        .circle_stroke(knob, 6.5, Stroke::new(1., Color32::from_black_alpha(160)));
    // Hue.
    let (bar, r) = ui.allocate_exact_size(Vec2::new(width, 12.), Sense::click_and_drag());
    let mut mesh = egui::Mesh::default();
    const H: usize = 36;
    for k in 0..=H {
        let h = k as f32 / H as f32;
        let x = bar.left() + h * bar.width();
        let c: Color32 = Hsva::new(h, 1., 1., 1.).into();
        mesh.colored_vertex(egui::pos2(x, bar.top()), c);
        mesh.colored_vertex(egui::pos2(x, bar.bottom()), c);
    }
    for k in 0..H as u32 {
        mesh.add_triangle(2 * k, 2 * k + 1, 2 * k + 3);
        mesh.add_triangle(2 * k, 2 * k + 3, 2 * k + 2);
    }
    ui.painter().add(egui::Shape::mesh(mesh));
    if let Some(pos) = r.interact_pointer_pos() {
        hsv.h = ((pos.x - bar.left()) / bar.width()).clamp(0., 1.);
        changed = true;
    }
    let x = bar.left() + hsv.h * bar.width();
    ui.painter().rect_stroke(
        Rect::from_center_size(
            egui::pos2(x, bar.center().y),
            Vec2::new(5., bar.height() + 4.),
        ),
        2.,
        Stroke::new(2., Color32::WHITE),
        egui::StrokeKind::Middle,
    );
    ui.data_mut(|d| d.insert_temp(id, hsv));
    let colour = hsv.to_srgb();
    (changed && colour != current, colour)
}

/// The swatch-and-hex control of a colour option in the settings editor (`filament_colour`,
/// `extruder_colour`). Returns the new `#RRGGBB` text.
pub fn option_control(
    ui: &mut egui::Ui,
    id: Id,
    rect: Rect,
    title: &str,
    value: &str,
    enabled: bool,
) -> Option<String> {
    let p = theme::of(ui);
    let recent: Vec<[u8; 3]> = ui
        .data(|d| d.get_temp::<Vec<[u8; 3]>>(recent_id()))
        .unwrap_or_default();
    let current = mica_profiles::project::parse_colour(value.trim());
    let popup_id = id.with("colour");
    let open = egui::Popup::is_id_open(ui.ctx(), popup_id);
    let response = ui.interact(
        rect,
        id.with("button"),
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    widgets::frame(ui, rect, response.hovered() || open, enabled);
    let square = Rect::from_center_size(
        egui::pos2(rect.left() + 8. + 8., rect.center().y),
        Vec2::splat(16.),
    );
    match current {
        Some(c) => paint_swatch(ui, square, c, false),
        None => {
            ui.painter().rect_stroke(
                square,
                3.,
                Stroke::new(1., p.border_raised),
                egui::StrokeKind::Inside,
            );
        }
    }
    let text = match current {
        Some(c) => crate::project::hex(c),
        None if value.trim().is_empty() => "None".into(),
        None => value.trim().to_string(),
    };
    let g = ui.painter().layout_no_wrap(
        text,
        theme::mono(12.),
        if enabled { p.text } else { p.muted },
    );
    ui.painter().galley(
        egui::pos2(square.right() + 8., rect.center().y - g.size().y / 2.),
        g,
        p.text,
    );
    if response.hovered() && enabled {
        ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
    }
    if !enabled {
        return None;
    }
    let pick = popup(
        &response,
        popup_id,
        title,
        current.unwrap_or([0x80, 0x80, 0x80]),
        &recent,
    )?;
    if pick.commit {
        ui.data_mut(|d| d.insert_temp(committed_id(), pick.colour));
    }
    Some(crate::project::hex(pick.colour))
}

impl App {
    /// The recent colour picks, newest first.
    pub(crate) fn recent_colours(&self) -> Vec<[u8; 3]> {
        self.prefs
            .recent_colours
            .iter()
            .filter_map(|c| mica_profiles::project::parse_colour(c))
            .collect()
    }

    /// Once a frame: hands the recent colours to the settings editor and remembers the
    /// colour it committed, if any.
    pub(crate) fn sync_recent_colours(&mut self, ctx: &egui::Context) {
        if let Some(c) = ctx.data_mut(|d| d.remove_temp::<[u8; 3]>(committed_id())) {
            self.remember_colour(c);
        }
        let recent = self.recent_colours();
        ctx.data_mut(|d| d.insert_temp(recent_id(), recent));
    }

    /// Remembers a pick as the newest recent colour.
    pub(crate) fn remember_colour(&mut self, c: [u8; 3]) {
        let hex = crate::project::hex(c);
        let recent = &mut self.prefs.recent_colours;
        recent.retain(|r| !r.eq_ignore_ascii_case(&hex));
        recent.insert(0, hex);
        recent.truncate(RECENT);
        self.prefs_dirty = true;
    }

    /// Sets tool `tool`'s filament colour: a project colour (none when it is the preset's
    /// own), shown at once, the slice kept (`App::colours_changed`).
    pub(crate) fn set_filament_colour(&mut self, tool: usize, pick: Pick) {
        if tool >= self.tool_count() {
            return;
        }
        self.begin_gesture("Change filament color");
        self.overlays.resize(self.tool_count());
        let preset = self
            .profiles
            .filaments
            .get(tool)
            .and_then(|f| self.profiles.filament_colour(f));
        // Project state (`Overlays::colours`), not an edit of the filament profile; the
        // preset's own colour needs no entry.
        let colour = (preset != Some(pick.colour)).then(|| crate::project::hex(pick.colour));
        self.overlays.set_colour(tool, colour);
        self.colours_changed();
        if pick.commit {
            self.remember_colour(pick.colour);
            self.last_change = Some(format!(
                "T{} color → {}",
                tool + 1,
                crate::mixes::colour_name(pick.colour).to_lowercase()
            ));
        }
    }
}

/// Whether `key` only changes how things look (no re-slice needed).
pub fn is_colour_key(key: &str) -> bool {
    matches!(key, "filament_colour" | "extruder_colour")
}
