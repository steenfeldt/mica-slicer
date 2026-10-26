// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The Machine view (third header tab). With no printer chosen it is the Printers
//! dashboard (`dashboard.rs`: every printer at a glance, cameras included); with one chosen,
//! the printers on the left and that one's live state on the right: a Job card (the file's
//! preview picture, progress, times, layer and Z, Pause / Resume / Cancel) and a grid of
//! heater tiles (each tool with its loaded filament, bed, chamber) in a column, the camera
//! beside it as tall as the column (`side_layout`); narrow windows put the camera on top.
//! Polling runs on the monitor threads (see `printhost::poll` for when); camera streams run
//! only while their picture is on screen. Also the printer notifications ("finished" and
//! "error", see `notifications`) and the per-frame printer bookkeeping (`poll_printers`).

use super::{duration, widgets};
use crate::app::App;
use crate::printhost::{
    cameras::{CameraViews, Role},
    monitor::{Command, Live, Notice},
};
use crate::theme;
use eframe::egui::{self, Align2, Color32, Pos2, Rect, Sense, Stroke, Vec2};
use mica_printhost::{PrinterState, Temperature};
use std::time::{Duration, Instant};

const LIST_W: f32 = 280.;

#[derive(Default)]
pub struct MachineView {
    /// The camera players on screen (thumbnails and the focused picture).
    pub cameras: CameraViews,
    /// "Cancel the print?" is open.
    pub confirm_cancel: bool,
    /// The decoded preview picture of the shown job: printer id, job name, texture (`None`:
    /// the printer's picture could not be decoded).
    thumbnail: Option<(u64, String, Option<egui::TextureHandle>)>,
    /// The info column's height last frame (0: not shown yet), which sizes the camera card
    /// and so how much width the info column can take.
    info_h: f32,
}

/// Colour of a state dot.
pub fn state_colour(p: &theme::Palette, state: Option<PrinterState>) -> Color32 {
    match state {
        Some(PrinterState::Printing) => p.accent_bright,
        Some(PrinterState::Paused) => theme::WARNING,
        Some(PrinterState::Idle | PrinterState::Finished) => p.muted_alt,
        Some(PrinterState::Error | PrinterState::Offline) => theme::DANGER,
        None => p.muted.gamma_multiply(0.6),
    }
}

/// One word for a state.
pub fn state_word(state: Option<PrinterState>) -> &'static str {
    match state {
        Some(PrinterState::Printing) => "Printing",
        Some(PrinterState::Paused) => "Paused",
        Some(PrinterState::Idle) => "Idle",
        Some(PrinterState::Finished) => "Finished",
        Some(PrinterState::Error) => "Error",
        Some(PrinterState::Offline) => "Offline",
        None => "Not connected",
    }
}

/// `14 s ago`, `just now`.
fn ago(at: Option<Instant>) -> String {
    match at {
        None => "no answer yet".into(),
        Some(t) => {
            let s = t.elapsed().as_secs();
            if s < 2 {
                "just now".into()
            } else if s < 90 {
                format!("{s} s ago")
            } else {
                format!("{} min ago", s / 60)
            }
        }
    }
}

/// A thin progress bar in `rect`.
pub fn bar(ui: &egui::Ui, rect: Rect, fraction: f32, colour: Color32) {
    let p = theme::of(ui);
    let r = rect.height() / 2.;
    ui.painter().rect_filled(rect, r, p.field_border);
    let w = rect.width() * fraction.clamp(0., 1.);
    if w > 0.5 {
        ui.painter().rect_filled(
            Rect::from_min_size(rect.min, Vec2::new(w.max(rect.height()), rect.height())),
            r,
            colour,
        );
    }
}

/// A card of the Machine view: panel surface, soft border, 10 px radius.
fn card<R>(ui: &mut egui::Ui, title: &str, body: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let p = theme::of(ui);
    egui::Frame::new()
        .fill(p.panel)
        .stroke(Stroke::new(1., p.border_soft))
        .corner_radius(10.)
        .inner_margin(egui::Margin::symmetric(14, 12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 6.;
            ui.label(
                egui::RichText::new(widgets::spaced_upper(title))
                    .font(theme::semibold(11.))
                    .color(p.muted),
            );
            ui.add_space(2.);
            body(ui)
        })
        .inner
}

/// The Job card: the file's preview picture, name, progress, times, layer and height, and
/// Pause / Resume / Cancel (Cancel only asks: `cancel` opens the confirmation).
fn job_card(
    ui: &mut egui::Ui,
    live: &Live,
    thumbnail: Option<&egui::TextureHandle>,
    command: &mut Option<Command>,
    cancel: &mut bool,
) {
    let p = theme::of(ui);
    card(ui, "Job", |ui| {
        let Some(s) = &live.status else {
            ui.label(
                egui::RichText::new("Waiting for the printer…")
                    .font(theme::sans(13.))
                    .color(p.muted),
            );
            return;
        };
        let active = matches!(
            s.state,
            PrinterState::Printing | PrinterState::Paused | PrinterState::Finished
        );
        if !active {
            ui.label(
                egui::RichText::new("No job running")
                    .font(theme::sans(13.))
                    .color(p.label),
            );
            return;
        }
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 14.;
            if let Some(t) = thumbnail {
                let side = 92.;
                let (rect, _) = ui.allocate_exact_size(Vec2::splat(side), Sense::hover());
                ui.painter().rect_filled(rect, 8., p.raised);
                let size = t.size_vec2();
                let scale = (side - 8.) / size.x.max(size.y).max(1.);
                ui.painter().image(
                    t.id(),
                    Rect::from_center_size(rect.center(), size * scale),
                    Rect::from_min_max(Pos2::ZERO, Pos2::new(1., 1.)),
                    Color32::WHITE,
                );
                ui.painter().rect_stroke(
                    rect,
                    8.,
                    Stroke::new(1., p.border_raised),
                    egui::StrokeKind::Inside,
                );
            }
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 8.;
                // Name, and the percentage large at the right.
                let percent = format!("{:.0}%", s.progress.unwrap_or(0.));
                let pg = ui
                    .painter()
                    .layout_no_wrap(percent, theme::mono_medium(22.), p.text);
                let (row, _) =
                    ui.allocate_exact_size(Vec2::new(ui.available_width(), 28.), Sense::hover());
                ui.painter().galley(
                    Pos2::new(row.right() - pg.size().x, row.center().y - pg.size().y / 2.),
                    pg.clone(),
                    p.text,
                );
                let name = s.job_name.clone().unwrap_or_else(|| "Unnamed job".into());
                let mut job =
                    egui::text::LayoutJob::simple_singleline(name, theme::medium(14.), p.text);
                job.wrap =
                    egui::text::TextWrapping::truncate_at_width(row.width() - pg.size().x - 12.);
                let g = ui.painter().layout_job(job);
                ui.painter().galley(
                    Pos2::new(row.left(), row.center().y - g.size().y / 2.),
                    g,
                    p.text,
                );
                let fraction = s.progress.unwrap_or(0.) as f32 / 100.;
                let (bar_rect, _) =
                    ui.allocate_exact_size(Vec2::new(ui.available_width(), 8.), Sense::hover());
                bar(
                    ui,
                    bar_rect,
                    fraction,
                    if s.state == PrinterState::Paused {
                        theme::WARNING
                    } else {
                        p.accent
                    },
                );
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(22., 6.);
                    if let Some(e) = s.elapsed {
                        stat(ui, "Elapsed", &duration(e));
                    }
                    if let Some(r) = s.remaining {
                        stat(ui, "Left", &duration(r));
                    }
                    if let Some(l) = s.current_layer {
                        stat(ui, "Layer", &l.to_string());
                    }
                    if let Some(z) = s.z {
                        stat(ui, "Z", &format!("{z:.2} mm"));
                    }
                });
            });
        });
        let caps = live.capabilities;
        let pending = live.pending;
        let busy = pending.is_some();
        if matches!(s.state, PrinterState::Printing | PrinterState::Paused) {
            ui.add_space(2.);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.;
                if s.state == PrinterState::Printing {
                    let label = if pending == Some(Command::Pause) {
                        "Pausing…"
                    } else {
                        "Pause"
                    };
                    if widgets::button(ui, label, caps.pause_resume && !busy).clicked() {
                        *command = Some(Command::Pause);
                    }
                } else {
                    let label = if pending == Some(Command::Resume) {
                        "Resuming…"
                    } else {
                        "Resume"
                    };
                    if widgets::primary_button(ui, label, caps.pause_resume && !busy).clicked() {
                        *command = Some(Command::Resume);
                    }
                }
                let label = if pending == Some(Command::Cancel) {
                    "Canceling…"
                } else {
                    "Cancel"
                };
                if widgets::button(ui, label, caps.cancel && !busy).clicked() {
                    *cancel = true;
                }
            });
        }
    });
}

/// The Temperatures card: a tile per tool (with its loaded filament), the bed and the
/// chamber, in a grid that wraps; "Use these in the project" sets `sync`.
/// `cols` fixes the number of tile columns (the wide layout keeps it while the column
/// stretches).
fn temperatures_card(
    ui: &mut egui::Ui,
    live: &Live,
    tiles: &[Tile],
    cols: Option<usize>,
    sync: &mut bool,
) {
    let p = theme::of(ui);
    card(ui, "Temperatures", |ui| {
        if live.status.is_none() {
            ui.label(egui::RichText::new("—").color(p.muted));
            return;
        }
        if tiles.is_empty() {
            ui.label(
                egui::RichText::new("No temperatures reported")
                    .font(theme::sans(12.))
                    .color(p.muted),
            );
        }
        let (cols, rows, tile_w) = match cols {
            Some(c) if c > 0 && !tiles.is_empty() => (
                c,
                tiles.len().div_ceil(c),
                ((ui.available_width() - TILE_GAP * (c - 1) as f32) / c as f32).min(TILE_MAX),
            ),
            _ => tile_grid(tiles.len(), ui.available_width()),
        };
        if rows > 0 {
            let h = rows as f32 * TILE_H + (rows - 1) as f32 * TILE_GAP;
            let (grid, _) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), h), Sense::hover());
            for (n, tile) in tiles.iter().enumerate() {
                let (row, col) = (n / cols, n % cols);
                let min = grid.min
                    + Vec2::new(
                        col as f32 * (tile_w + TILE_GAP),
                        row as f32 * (TILE_H + TILE_GAP),
                    );
                heater_tile(
                    ui,
                    Rect::from_min_size(min, Vec2::new(tile_w, TILE_H)),
                    tile,
                );
            }
        }
        // The swatches are the printer's spools; one click puts them in the project.
        let note = match (live.filaments_known, live.filaments.is_empty()) {
            (true, false) => None,
            (true, true) => Some("This printer does not report its spools."),
            (false, _) => Some("Loaded filament not reported yet."),
        };
        ui.add_space(2.);
        match note {
            Some(n) => {
                ui.label(egui::RichText::new(n).font(theme::sans(12.)).color(p.muted));
            }
            None => {
                ui.horizontal_wrapped(|ui| {
                    if widgets::button(ui, "Use these in the project", true)
                        .on_hover_text(
                            "Set each tool's color (and material preset where one matches) \
                             from the filament loaded in the printer",
                        )
                        .clicked()
                    {
                        *sync = true;
                    }
                    ui.label(
                        egui::RichText::new("Swatches show the loaded filament.")
                            .font(theme::sans(12.))
                            .color(p.muted),
                    );
                });
            }
        }
    });
}

impl MachineView {
    /// The texture of a job's preview picture, decoded when the job (or printer) changes.
    fn thumbnail(
        &mut self,
        ctx: &egui::Context,
        printer: u64,
        picture: Option<(&str, &std::sync::Arc<[u8]>)>,
    ) -> Option<egui::TextureHandle> {
        let (job, bytes) = picture?;
        let fresh =
            !matches!(&self.thumbnail, Some((id, name, _)) if *id == printer && name == job);
        if fresh {
            let texture = image::load_from_memory(bytes).ok().map(|img| {
                let rgba = img.to_rgba8();
                let size = [rgba.width() as usize, rgba.height() as usize];
                ctx.load_texture(
                    "machine-job-thumbnail",
                    egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()),
                    egui::TextureOptions::LINEAR,
                )
            });
            self.thumbnail = Some((printer, job.to_string(), texture));
        }
        self.thumbnail.as_ref().and_then(|(_, _, t)| t.clone())
    }
}

// ----- layout ------------------------------------------------------------------------------

/// Space between the cards.
const GAP: f32 = 16.;
/// The info column (job and temperatures) beside the camera: at least this wide…
const INFO_MIN: f32 = 440.;
/// …at most this wide, and this share of the detail width in between.
const INFO_MAX: f32 = 720.;
const INFO_SHARE: f32 = 0.4;
/// Narrowest camera card beside the info column; narrower, the camera goes on top.
const CAMERA_SIDE_MIN: f32 = 380.;
/// The camera card never gets wider than a 16:9 picture of its height (wider screens leave
/// the rest of the row empty rather than letterboxing the picture).
const CAMERA_ASPECT: f32 = 16. / 9.;
/// Lowest camera card beside the info column.
const CAMERA_MIN_H: f32 = 260.;
/// Heater tiles: width range, height and gap.
const TILE_MIN: f32 = 156.;
const TILE_MAX: f32 = 250.;
const TILE_H: f32 = 84.;
const TILE_GAP: f32 = 10.;
/// A card's frame: inner margins and the title line above the body.
const CARD_PAD: Vec2 = Vec2::new(14., 12.);
const CARD_TITLE: f32 = 22.;

/// The info column's width when the camera fits beside it, `None` when the camera goes on top
/// (narrow windows). `width` is the detail area's.
pub(crate) fn info_width(width: f32) -> Option<f32> {
    let info = (width * INFO_SHARE).clamp(INFO_MIN, INFO_MAX);
    (width - info - GAP >= CAMERA_SIDE_MIN).then_some(info)
}

/// The heater tile grid for `n` tiles in `width`: `(columns, rows, tile width)`. Rows are
/// balanced (six tiles in two rows of three, not five and one) and tiles never grow past
/// `TILE_MAX`.
pub(crate) fn tile_grid(n: usize, width: f32) -> (usize, usize, f32) {
    if n == 0 {
        return (1, 0, TILE_MIN);
    }
    let fit = (((width + TILE_GAP) / (TILE_MIN + TILE_GAP)).floor() as usize).clamp(1, n);
    let rows = n.div_ceil(fit);
    let cols = n.div_ceil(rows);
    let w = ((width - TILE_GAP * (cols - 1) as f32) / cols as f32).min(TILE_MAX);
    (cols, rows, w)
}

/// The wide layout's info column width and tile columns. `base` is `info_width`'s width;
/// `info_h` the column's height last frame (0: unknown). The camera card is as tall as the
/// column and as wide as its picture (`aspect`); width the camera does not need goes to the
/// info column as long as the tiles keep their rows (so its height, and the camera's size,
/// stay put), at most `TILE_MAX` per tile.
pub(crate) fn side_layout(
    width: f32,
    base: f32,
    info_h: f32,
    aspect: f32,
    tiles: usize,
) -> (f32, usize) {
    let (cols, _, _) = tile_grid(tiles, base - 2. * CARD_PAD.x);
    if info_h <= 0. {
        return (base, cols);
    }
    let picture_h = info_h.max(CAMERA_MIN_H) - 2. * CARD_PAD.y - CARD_TITLE;
    let camera_w = (picture_h * aspect + 2. * CARD_PAD.x).max(CAMERA_SIDE_MIN);
    let widest = cols as f32 * TILE_MAX + (cols.max(1) - 1) as f32 * TILE_GAP + 2. * CARD_PAD.x;
    let info = (width - GAP - camera_w).min(widest.max(INFO_MAX)).max(base);
    // Fewer rows fit the wider column: take them (the column gets lower, the camera
    // narrower and the column wider still, so this settles within a frame or two).
    let (wide_cols, wide_rows, _) = tile_grid(tiles, info - 2. * CARD_PAD.x);
    if wide_rows < tiles.div_ceil(cols.max(1)) {
        return (info, wide_cols);
    }
    (info, cols)
}

/// What a tool's tile says about its filament.
#[derive(Clone, Debug, PartialEq)]
enum Spool {
    /// The printer does not report spools (or not yet).
    Unknown,
    /// Reported, nothing loaded.
    Empty,
    Loaded {
        colour: Option<[u8; 3]>,
        material: Option<String>,
    },
}

/// One heater tile: a tool (with its filament), the bed or the chamber.
struct Tile {
    label: String,
    temperature: Temperature,
    spool: Option<Spool>,
}

/// The tiles for a status: the tools in order, then bed and chamber.
fn heater_tiles(live: &Live) -> Vec<Tile> {
    let Some(s) = &live.status else {
        return vec![];
    };
    let mut tiles: Vec<Tile> = s
        .hotends
        .iter()
        .map(|h| {
            let spool = if !live.filaments_known || live.filaments.is_empty() {
                Spool::Unknown
            } else {
                match live.filaments.iter().find(|f| f.tool == h.tool) {
                    Some(f) => Spool::Loaded {
                        colour: f.colour.as_deref().and_then(mica_core::color::parse_hex),
                        material: f.material.clone(),
                    },
                    None => Spool::Empty,
                }
            };
            Tile {
                label: format!("T{}", h.tool + 1),
                temperature: h.temperature.clone(),
                spool: Some(spool),
            }
        })
        .collect();
    if let Some(b) = &s.bed {
        tiles.push(Tile {
            label: "Bed".into(),
            temperature: b.clone(),
            spool: None,
        });
    }
    if let Some(c) = &s.chamber {
        tiles.push(Tile {
            label: "Chamber".into(),
            temperature: c.clone(),
            spool: None,
        });
    }
    tiles
}

/// A tool tile's filament: `PLA · yellow` (a color name, never a hex code).
pub(crate) fn tile_spool_text(material: Option<&str>, colour: Option<[u8; 3]>) -> String {
    let m = material
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .unwrap_or("Unknown");
    match colour {
        Some(c) => format!("{m} · {}", mica_printhost::colour::colour_name(c)),
        None => m.to_string(),
    }
}

/// A heater's state in one word and its colour: heating up, at its target, cooling down
/// or off (`None` for a sensor without a target).
fn heater_state(p: &theme::Palette, t: &Temperature) -> Option<(&'static str, Color32)> {
    let current = t.current?;
    match t.target {
        Some(target) if target > 0. => Some(if current < target - 3. {
            ("Heating", theme::WARNING)
        } else if current > target + 5. {
            ("Cooling", p.muted_alt)
        } else {
            ("Ready", p.success)
        }),
        Some(_) if current >= 45. => Some(("Cooling", p.muted_alt)),
        Some(_) => Some(("Off", p.muted)),
        None => None,
    }
}

/// Paints one heater tile in `rect`.
fn heater_tile(ui: &egui::Ui, rect: Rect, tile: &Tile) {
    let p = theme::of(ui);
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 8., p.raised);
    painter.rect_stroke(
        rect,
        8.,
        Stroke::new(1., p.border_raised),
        egui::StrokeKind::Inside,
    );
    let inner = rect.shrink2(Vec2::new(10., 8.));
    let top = inner.top() + 8.;
    // Top line: name, filament swatch and material, state at the right.
    let g = painter.layout_no_wrap(tile.label.clone(), theme::semibold(12.), p.text);
    let mut x = inner.left();
    painter.galley(Pos2::new(x, top - g.size().y / 2.), g.clone(), p.text);
    x += g.size().x + 7.;
    let state = heater_state(p, &tile.temperature);
    let state_w = state.map_or(0., |(word, colour)| {
        let g = painter.layout_no_wrap(word.into(), theme::sans(11.), colour);
        let w = g.size().x;
        painter.galley(
            Pos2::new(inner.right() - w, top - g.size().y / 2.),
            g,
            colour,
        );
        painter.circle_filled(Pos2::new(inner.right() - w - 7., top), 3., colour);
        w + 14.
    });
    let room = inner.right() - state_w - 6.;
    match &tile.spool {
        Some(Spool::Loaded { colour, material }) => {
            let sw = Rect::from_center_size(Pos2::new(x + 6., top), Vec2::splat(12.));
            match colour {
                Some(c) => {
                    painter.rect_filled(sw, 3., Color32::from_rgb(c[0], c[1], c[2]));
                    painter.rect_stroke(
                        sw,
                        3.,
                        Stroke::new(1., p.border),
                        egui::StrokeKind::Inside,
                    );
                }
                None => {
                    painter.rect_stroke(sw, 3., Stroke::new(1., p.muted), egui::StrokeKind::Inside);
                }
            }
            x += 18.;
            let text = tile_spool_text(material.as_deref(), *colour);
            text_clipped(&painter, Pos2::new(x, top), &text, p.label, room - x);
        }
        Some(Spool::Empty) => {
            let sw = Rect::from_center_size(Pos2::new(x + 6., top), Vec2::splat(12.));
            painter.rect_stroke(sw, 3., Stroke::new(1., p.muted), egui::StrokeKind::Inside);
            x += 18.;
            text_clipped(&painter, Pos2::new(x, top), "Empty", p.muted, room - x);
        }
        Some(Spool::Unknown) | None => {}
    }
    // The reading: current large, target small after it.
    let t = &tile.temperature;
    let current = t
        .current
        .map_or_else(|| "—".to_string(), |c| format!("{c:.0}"));
    let g = painter.layout_no_wrap(current, theme::mono_medium(24.), p.text);
    let base = inner.top() + 22.;
    painter.galley(Pos2::new(inner.left(), base), g.clone(), p.text);
    let target = match t.target {
        Some(target) if target > 0. => format!("/ {target:.0} °C"),
        Some(_) => "/ off".into(),
        None => "°C".into(),
    };
    let tg = painter.layout_no_wrap(target, theme::mono(12.), p.muted_alt);
    painter.galley(
        Pos2::new(
            inner.left() + g.size().x + 6.,
            base + g.size().y - tg.size().y - 4.,
        ),
        tg,
        p.muted_alt,
    );
    // A thin bar to the target (a 300 °C scale while off).
    let scale = t.target.filter(|t| *t > 0.).unwrap_or(300.).max(1.);
    let fraction = (t.current.unwrap_or(0.) / scale) as f32;
    let colour = state.map_or(p.muted_alt, |(_, c)| c);
    bar(
        ui,
        Rect::from_min_max(
            Pos2::new(inner.left(), inner.bottom() - 3.),
            Pos2::new(inner.right(), inner.bottom()),
        ),
        fraction,
        colour,
    );
}

/// Single-line text, vertically centred on `pos.y`, cut with "…" to `width`.
fn text_clipped(painter: &egui::Painter, pos: Pos2, text: &str, colour: Color32, width: f32) {
    if width < 12. {
        return;
    }
    let mut job = egui::text::LayoutJob::simple_singleline(text.into(), theme::sans(11.), colour);
    job.wrap = egui::text::TextWrapping::truncate_at_width(width);
    let g = painter.layout_job(job);
    painter.galley(Pos2::new(pos.x, pos.y - g.size().y / 2.), g, colour);
}

/// A card frame painted into a fixed `rect` (the camera's, sized to match the info column):
/// the same look as `card`. Returns the body below the title.
fn card_frame(ui: &egui::Ui, rect: Rect, title: &str) -> Rect {
    let p = theme::of(ui);
    ui.painter().rect_filled(rect, 10., p.panel);
    ui.painter().rect_stroke(
        rect,
        10.,
        Stroke::new(1., p.border_soft),
        egui::StrokeKind::Inside,
    );
    let inner = rect.shrink2(CARD_PAD);
    ui.painter().text(
        inner.left_top(),
        Align2::LEFT_TOP,
        widgets::spaced_upper(title),
        theme::semibold(11.),
        p.muted,
    );
    Rect::from_min_max(Pos2::new(inner.left(), inner.top() + CARD_TITLE), inner.max)
}

/// One stat of the job card: a small label over a value.
fn stat(ui: &mut egui::Ui, label: &str, value: &str) {
    let p = theme::of(ui);
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 1.;
        ui.label(
            egui::RichText::new(widgets::spaced_upper(label))
                .font(theme::semibold(9.5))
                .color(p.muted),
        );
        ui.label(
            egui::RichText::new(value)
                .font(theme::mono(13.))
                .color(p.text),
        );
    });
}

impl App {
    /// Per frame: keep the monitors scheduled (poll only what is being looked at, only while
    /// the window is focused), drain their updates, finish pending syncs and uploads.
    pub(crate) fn poll_printers(&mut self, ctx: &egui::Context) {
        let wake = {
            let c = ctx.clone();
            move || c.request_repaint()
        };
        // Polling continues while the window is in the background (a dashboard on a second
        // screen); only a minimised window stops it.
        let window = self.headless || ctx.input(|i| i.viewport().minimized != Some(true));
        // The Machine view remembers a printer id; a removed printer goes back to the
        // dashboard rather than to whichever printer moved into its place.
        if self.machine_printer.is_some() && self.machine_index().is_none() {
            self.machine_printer = None;
            self.machine_view.confirm_cancel = false;
        }
        // Likewise the Print dialog: its printer was removed in My printers, so it asks for
        // a printer again rather than sending to whichever one took its place in the list.
        if let Some(d) = &mut self.print_job
            && let Some(t) = &d.target
            && self.printers.position(t.id).is_none()
        {
            d.choose(None);
        }
        // On screen: the chosen printer, or every printer on the dashboard.
        let mut watching: Vec<usize> = match (self.machine, self.machine_index()) {
            (false, _) => vec![],
            (true, Some(i)) => vec![i],
            (true, None) => (0..self.printers.list.len()).collect(),
        };
        // My printers' cards show every printer's status while the window is open.
        if self.dialogs.my_printers
            && self.printers_editor.view == crate::panels::printers::View::Cards
        {
            watching = (0..self.printers.list.len()).collect();
        }
        for &i in &watching {
            self.printers.watcher(i, wake.clone());
        }
        self.printers.schedule(&watching, window);
        if self.printers.drain() {
            ctx.request_repaint();
        }
        self.notify_job_changes();
        if self.sync_pending.is_some() {
            self.finish_pending_sync();
        }
        self.check_file_identity();
        if let Some(d) = &mut self.print_job
            && d.poll()
            && matches!(d.stage, crate::printhost::job::Stage::Done { .. })
        {
            self.finish_upload();
        }
        // All plates: every file sent (or skipped).
        if self.print_job.as_ref().is_some_and(|d| d.batch_complete()) {
            self.finish_upload_all();
        }
        if (self.machine || self.dialogs.my_printers) && !watching.is_empty() {
            // The "x s ago" labels and the camera frame age.
            ctx.request_repaint_after(Duration::from_millis(500));
        }
    }

    /// Turns the monitors' job transitions into notifications: "finished" and "error".
    fn notify_job_changes(&mut self) {
        let mut notices = vec![];
        for p in &self.printers.list {
            if let Some(w) = self.printers.watchers.get_mut(&p.name) {
                for n in w.live.notices.drain(..) {
                    notices.push((p.id, p.name.clone(), n));
                }
            }
        }
        for (id, name, n) in notices {
            let (text, error) = match n {
                Notice::Finished(job) => (
                    match job {
                        Some(j) => format!("{name} finished {j}"),
                        None => format!("{name} finished its print"),
                    },
                    false,
                ),
                Notice::Failed(job) => (
                    match job {
                        Some(j) => format!("{name} reports an error while printing {j}"),
                        None => format!("{name} reports an error"),
                    },
                    true,
                ),
            };
            use crate::notifications::{Action, Level, Notification, PRINTER_LIFE};
            let n = if error {
                Notification::new(Level::Error, text.clone(), text)
            } else {
                Notification::new(Level::Info, text.clone(), text).life(PRINTER_LIFE)
            };
            self.notify(n.action("Open", Action::OpenMachine(id)));
        }
    }

    /// Where the Machine view's printer is in the list now (`None`: none chosen, or removed).
    pub(crate) fn machine_index(&self) -> Option<usize> {
        self.machine_printer
            .and_then(|id| self.printers.position(id))
    }

    /// The whole Machine view, in place of the sidebar and the 3D view.
    pub(crate) fn machine_view(&mut self, ui: &mut egui::Ui) {
        let p = theme::of(ui);
        self.machine_view.cameras.begin_frame();
        let dashboard = self.machine_index().is_none() && !self.printers.list.is_empty();
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(p.bg))
            .show(ui, |ui| {
                let rect = ui.max_rect();
                if dashboard {
                    let mut board = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink(22.)));
                    self.printers_dashboard(&mut board);
                    return;
                }
                let list =
                    Rect::from_min_max(rect.min, Pos2::new(rect.left() + LIST_W, rect.bottom()));
                ui.painter().rect_filled(list, 0., p.panel);
                ui.painter().line_segment(
                    [list.right_top(), list.right_bottom()],
                    Stroke::new(1., p.border),
                );
                let mut list_ui = ui.new_child(egui::UiBuilder::new().max_rect(list.shrink(14.)));
                self.machine_list(&mut list_ui);
                let detail = Rect::from_min_max(Pos2::new(list.right() + 1., rect.top()), rect.max);
                let mut detail_ui =
                    ui.new_child(egui::UiBuilder::new().max_rect(detail.shrink(22.)));
                self.machine_detail(&mut detail_ui);
            });
        // Pictures not drawn this frame stop their streams.
        self.machine_view.cameras.end_frame();
        if self.machine_view.confirm_cancel {
            self.confirm_cancel_modal(ui.ctx());
        }
    }

    fn machine_list(&mut self, ui: &mut egui::Ui) {
        let p = theme::of(ui);
        ui.spacing_mut().item_spacing.y = 8.;
        widgets::card_header(ui, "Printers", |ui| {
            if widgets::icon_button(ui, widgets::Glyph::Plus, "Add or edit printers", true)
                .clicked()
            {
                self.dialogs.my_printers = true;
            }
        });
        if self.printers.list.is_empty() {
            ui.add_space(6.);
            ui.label(
                egui::RichText::new(
                    "No printers yet. Add your PrusaLink, Klipper or Snapmaker U1 printer to \
                     watch it here and send plates to it.",
                )
                .font(theme::sans(12.))
                .color(p.muted),
            );
            ui.add_space(6.);
            if widgets::primary_button(ui, "Add a printer…", true).clicked() {
                self.dialogs.my_printers = true;
            }
            return;
        }
        let mut pick = None;
        let mut all = false;
        {
            let (row, r) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), 34.), Sense::click());
            let fill = if r.hovered() { p.hover } else { p.raised };
            ui.painter().rect_filled(row, 8., fill);
            ui.painter().rect_stroke(
                row,
                8.,
                Stroke::new(1., p.border_raised),
                egui::StrokeKind::Inside,
            );
            ui.painter().text(
                Pos2::new(row.left() + 12., row.center().y),
                Align2::LEFT_CENTER,
                "← All printers",
                theme::medium(13.),
                p.label,
            );
            if r.clicked() {
                all = true;
            }
            if r.hovered() {
                ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
            }
        }
        for printer in &self.printers.list {
            let live = self.printers.watchers.get(&printer.name).map(|w| &w.live);
            let state = live.and_then(|l| l.status.as_ref()).map(|s| s.state);
            let selected = self.machine_printer == Some(printer.id);
            let (row, r) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), 58.), Sense::click());
            let fill = if selected {
                p.accent_surface
            } else if r.hovered() {
                p.hover
            } else {
                p.raised
            };
            ui.painter().rect_filled(row, 8., fill);
            ui.painter().rect_stroke(
                row,
                8.,
                Stroke::new(1., if selected { p.accent } else { p.border_raised }),
                egui::StrokeKind::Inside,
            );
            ui.painter().text(
                Pos2::new(row.left() + 12., row.top() + 18.),
                Align2::LEFT_CENTER,
                &printer.name,
                theme::medium(13.),
                if selected { p.accent_hi } else { p.text },
            );
            let model = live
                .and_then(|l| l.info.as_ref())
                .and_then(|i| i.model.clone())
                .or_else(|| printer.model.clone());
            let second = match model {
                Some(m) => format!("{m} · {}", printer.kind.label()),
                None => format!("{} · {}", printer.kind.label(), printer.address),
            };
            let mut job =
                egui::text::LayoutJob::simple_singleline(second, theme::sans(11.), p.muted);
            job.wrap = egui::text::TextWrapping::truncate_at_width(row.width() - 24.);
            let g = ui.painter().layout_job(job);
            ui.painter()
                .galley(Pos2::new(row.left() + 12., row.top() + 32.), g, p.muted);
            // State dot and word, right.
            let failed = live.is_some_and(Live::failed);
            let colour = if failed {
                theme::DANGER
            } else {
                state_colour(p, state)
            };
            let word = if failed {
                "No answer"
            } else {
                state_word(state)
            };
            let g = ui
                .painter()
                .layout_no_wrap(word.into(), theme::sans(11.), p.muted_alt);
            ui.painter().galley(
                Pos2::new(
                    row.right() - 12. - g.size().x,
                    row.top() + 18. - g.size().y / 2.,
                ),
                g.clone(),
                p.muted_alt,
            );
            ui.painter().circle_filled(
                Pos2::new(row.right() - 12. - g.size().x - 10., row.top() + 18.),
                4.,
                colour,
            );
            if r.clicked() {
                pick = Some(printer.id);
            }
            if r.hovered() {
                ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
            }
        }
        if let Some(i) = pick {
            self.machine_printer = Some(i);
            self.machine_view.confirm_cancel = false;
        }
        if all {
            self.machine_printer = None;
            self.machine_view.confirm_cancel = false;
        }
        ui.add_space(4.);
        if widgets::button(ui, "My printers…", true).clicked() {
            self.dialogs.my_printers = true;
        }
    }

    fn machine_detail(&mut self, ui: &mut egui::Ui) {
        let p = theme::of(ui);
        let Some(i) = self.machine_index() else {
            if !self.printers.list.is_empty() {
                ui.label(
                    egui::RichText::new("Choose a printer on the left.")
                        .font(theme::sans(13.))
                        .color(p.muted),
                );
            }
            return;
        };
        let Some(printer) = self.printers.list.get(i).cloned() else {
            return;
        };
        if printer.kind == crate::printhost::config::Kind::PrusaConnect {
            ui.heading(&printer.name);
            ui.label("Send via Prusa Connect using this printer's API key. Status, cameras and queue management are available in your browser.");
            if let Some(url) = printer.connect_page() {
                if widgets::button(ui, "Open in Prusa Connect", true).clicked() {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(url));
                }
            } else {
                ui.weak("Set the printer's Connect ID in My printers to link its page.");
            }
            if widgets::button(ui, "My printers…", true).clicked() {
                self.dialogs.my_printers = true;
            }
            return;
        }
        let Some(watcher) = self.printers.watchers.get(&printer.name) else {
            return;
        };
        let live = &watcher.live;
        let state = live.status.as_ref().map(|s| s.state);
        let mut command: Option<Command> = None;
        let mut sync = false;
        ui.spacing_mut().item_spacing.y = 12.;
        // Header: name, model · firmware · address, state chip, last update.
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(&printer.name)
                    .font(theme::semibold(20.))
                    .color(p.text),
            );
            ui.add_space(6.);
            let colour = if live.failed() {
                theme::DANGER
            } else {
                state_colour(p, state)
            };
            let word = if live.failed() {
                "No answer"
            } else {
                state_word(state)
            };
            let g = ui
                .painter()
                .layout_no_wrap(word.into(), theme::medium(12.), colour);
            let (chip, _) =
                ui.allocate_exact_size(Vec2::new(g.size().x + 30., 24.), Sense::hover());
            ui.painter()
                .rect_filled(chip, 12., colour.gamma_multiply(0.15));
            ui.painter()
                .circle_filled(Pos2::new(chip.left() + 11., chip.center().y), 4., colour);
            ui.painter().galley(
                Pos2::new(chip.left() + 20., chip.center().y - g.size().y / 2.),
                g,
                colour,
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(format!("updated {}", ago(live.updated)))
                        .font(theme::sans(12.))
                        .color(p.muted),
                );
            });
        });
        let mut line = vec![];
        if let Some(info) = &live.info {
            if let Some(m) = &info.model {
                line.push(m.clone());
            }
            if let Some(f) = &info.firmware {
                line.push(format!("firmware {f}"));
            }
        } else if let Some(m) = &printer.model {
            line.push(m.clone());
        }
        line.push(printer.kind.label().to_string());
        line.push(printer.address.clone());
        ui.label(
            egui::RichText::new(line.join(" · "))
                .font(theme::sans(12.))
                .color(p.muted_alt),
        );
        if let Some(e) = &live.unavailable {
            warn_card(ui, &format!("Mica cannot connect to {}", printer.name), e);
        } else if let Some(e) = &live.error {
            let x = crate::printhost::explain::explain(
                e,
                printer.kind,
                &printer.name,
                &printer.address,
            );
            warn_card(ui, &x.headline, &x.advice);
        }
        if let Some((c, Err(e))) = &live.last_command {
            let x = crate::printhost::explain::explain(
                e,
                printer.kind,
                &printer.name,
                &printer.address,
            );
            warn_card(
                ui,
                &format!("{} failed: {}", c.label(), x.headline),
                &x.advice,
            );
        }
        // The job's preview picture, decoded once per job.
        let thumbnail = self
            .machine_view
            .thumbnail(ui.ctx(), printer.id, live.job_thumbnail());
        let tiles = heater_tiles(live);
        // The camera card only reserves its place; the picture is drawn last (it needs
        // `&mut self`), clipped to the scroll area.
        let mut camera_slot: Option<(Rect, Rect)> = None;
        let mut cancel = false;
        let avail_h = ui.available_height();
        // The picture's own shape once a frame arrived (16:9 until then), and the info
        // column's height last frame: the camera card is sized from both.
        let aspect = self
            .machine_view
            .cameras
            .aspect(printer.id, Role::Focused)
            .unwrap_or(CAMERA_ASPECT)
            .clamp(1., 2.4);
        let info_h_before = self.machine_view.info_h;
        let mut info_h = None;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 12.;
                let width = ui.available_width();
                match info_width(width) {
                    // Wide: job and temperatures on the left, the camera beside them as
                    // tall as they are.
                    Some(base) => {
                        let (info_w, cols) =
                            side_layout(width, base, info_h_before, aspect, tiles.len());
                        let top = ui.cursor().min;
                        let mut info = ui.new_child(
                            egui::UiBuilder::new()
                                .max_rect(Rect::from_min_size(top, Vec2::new(info_w, 4000.)))
                                .layout(egui::Layout::top_down(egui::Align::Min)),
                        );
                        info.spacing_mut().item_spacing.y = 12.;
                        job_card(
                            &mut info,
                            live,
                            thumbnail.as_ref(),
                            &mut command,
                            &mut cancel,
                        );
                        temperatures_card(&mut info, live, &tiles, Some(cols), &mut sync);
                        info_h = Some(info.min_rect().height());
                        let card_h = info.min_rect().height().max(CAMERA_MIN_H);
                        let picture_h = card_h - 2. * CARD_PAD.y - CARD_TITLE;
                        let camera_w =
                            (width - info_w - GAP).min(picture_h * aspect + 2. * CARD_PAD.x);
                        let rect = Rect::from_min_size(
                            Pos2::new(top.x + info_w + GAP, top.y),
                            Vec2::new(camera_w, card_h),
                        );
                        camera_slot = Some((card_frame(ui, rect, "Camera"), ui.clip_rect()));
                        ui.allocate_rect(
                            Rect::from_min_size(top, Vec2::new(width, card_h)),
                            Sense::hover(),
                        );
                    }
                    // Narrow: the camera on top, the cards below it.
                    None => {
                        let picture_h =
                            ((width - 2. * CARD_PAD.x) / aspect).min((avail_h * 0.38).max(200.));
                        let (rect, _) = ui.allocate_exact_size(
                            Vec2::new(width, picture_h + 2. * CARD_PAD.y + CARD_TITLE),
                            Sense::hover(),
                        );
                        camera_slot = Some((card_frame(ui, rect, "Camera"), ui.clip_rect()));
                        job_card(ui, live, thumbnail.as_ref(), &mut command, &mut cancel);
                        temperatures_card(ui, live, &tiles, None, &mut sync);
                    }
                }
            });
        if cancel {
            self.machine_view.confirm_cancel = true;
        }
        if let Some(h) = info_h {
            if (h - info_h_before).abs() > 0.5 {
                ui.ctx().request_repaint();
            }
            self.machine_view.info_h = h;
        }
        let loaded = sync.then(|| live.filaments.clone());
        if let Some(c) = command
            && let Some(w) = self.printers.watchers.get_mut(&printer.name)
        {
            w.command(c);
        }
        if let Some((r, clip)) = camera_slot {
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r));
            child.set_clip_rect(r.intersect(clip));
            self.camera_picture(&mut child, r, i, Role::Focused);
        }
        if let Some(loaded) = loaded {
            self.apply_loaded_filaments(i, &loaded);
        }
    }

    /// "Cancel the print?" — the one destructive control asks first.
    fn confirm_cancel_modal(&mut self, ctx: &egui::Context) {
        let Some(i) = self.machine_index() else {
            self.machine_view.confirm_cancel = false;
            return;
        };
        let name = self
            .printers
            .list
            .get(i)
            .map(|p| p.name.clone())
            .unwrap_or_default();
        let job = self
            .printers
            .live(i)
            .and_then(|l| l.status.as_ref())
            .and_then(|s| s.job_name.clone())
            .unwrap_or_else(|| "the current job".into());
        let mut decision: Option<bool> = None;
        let response = egui::Modal::new(egui::Id::unique("confirm-cancel")).show(ctx, |ui| {
            let p = theme::of(ui);
            ui.set_width(380.);
            ui.spacing_mut().item_spacing.y = 8.;
            ui.label(
                egui::RichText::new(format!("Cancel {job} on {name}?"))
                    .font(theme::semibold(15.))
                    .color(p.text),
            );
            ui.label(
                egui::RichText::new(
                    "The printer stops where it is and the job cannot be resumed. The file stays \
                     on the printer.",
                )
                .font(theme::sans(13.))
                .color(p.label),
            );
            ui.add_space(4.);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 8.;
                    if danger_button(ui, "Cancel the print").clicked() {
                        decision = Some(true);
                    }
                    if widgets::button(ui, "Keep printing", true).clicked() {
                        decision = Some(false);
                    }
                });
            });
        });
        if response.should_close() {
            decision = Some(false);
        }
        if let Some(yes) = decision {
            self.machine_view.confirm_cancel = false;
            if yes
                && let Some(p) = self.printers.list.get(i).map(|p| p.name.clone())
                && let Some(w) = self.printers.watchers.get_mut(&p)
            {
                w.command(Command::Cancel);
            }
        }
    }

    /// Closes the Print dialog on success and announces it with a link to the Machine view.
    pub(crate) fn finish_upload(&mut self) {
        let Some(d) = &self.print_job else { return };
        let (
            crate::printhost::job::Stage::Done {
                job,
                started,
                message,
                warning,
            },
            Some(target),
        ) = (&d.stage, &d.target)
        else {
            return;
        };
        let text = if target.kind == crate::printhost::config::Kind::PrusaConnect {
            format!(
                "Prusa Connect accepted {job} for {}{}{}. Printing is not confirmed.{}",
                target.name,
                if d.queue {
                    " · queue requested"
                } else if d.start_after {
                    " · print requested"
                } else {
                    ""
                },
                if *warning { " · warning" } else { "" },
                message
                    .as_ref()
                    .map(|m| format!(" {m}"))
                    .unwrap_or_default()
            )
        } else if *started {
            format!("Uploaded {job} to {} · printing started", target.name)
        } else if d.start_after {
            format!(
                "Uploaded {job} to {} · the printer did not confirm a start; check its screen",
                target.name
            )
        } else {
            format!("Uploaded {job} to {}", target.name)
        };
        let id = target.id;
        // A U1 start says which route it took (start_local_print or the slot macros).
        let detail = match &d.note {
            Some(n) => format!("{text}. {n}."),
            None => text.clone(),
        };
        self.notify(
            crate::notifications::Notification::new(
                if *warning {
                    crate::notifications::Level::Warning
                } else {
                    crate::notifications::Level::Info
                },
                text,
                detail,
            )
            .action(
                "Open Machine view",
                crate::notifications::Action::OpenMachine(id),
            ),
        );
        self.dialogs.print = false;
        self.print_job = None;
    }
}

/// A warning card: headline and advice on the warning surface.
pub fn warn_card(ui: &mut egui::Ui, headline: &str, advice: &str) {
    let p = theme::of(ui);
    egui::Frame::new()
        .fill(p.warn_surface)
        .stroke(Stroke::new(1., p.warn_border))
        .corner_radius(8.)
        .inner_margin(egui::Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 4.;
            ui.label(
                egui::RichText::new(headline)
                    .font(theme::medium(13.))
                    .color(p.warn_text),
            );
            if !advice.is_empty() {
                ui.label(
                    egui::RichText::new(advice)
                        .font(theme::sans(12.))
                        .color(p.warn_dim),
                );
            }
        });
}

/// A red-outlined button for the one destructive action.
pub fn danger_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_string(), theme::semibold(13.), theme::DANGER);
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(galley.size().x + 24., 30.), Sense::click());
    let hovered = response.hovered();
    let painter = ui.painter();
    painter.rect_filled(
        rect,
        6.,
        theme::DANGER.gamma_multiply(if hovered { 0.3 } else { 0.15 }),
    );
    painter.rect_stroke(
        rect,
        6.,
        Stroke::new(1., theme::DANGER),
        egui::StrokeKind::Inside,
    );
    painter.galley(
        Pos2::new(
            rect.center().x - galley.size().x / 2.,
            rect.center().y - galley.size().y / 2.,
        ),
        galley,
        theme::DANGER,
    );
    if hovered {
        ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The detail area's width for a window `w` wide (printer list, margins).
    fn detail(w: f32) -> f32 {
        w - LIST_W - 1. - 44.
    }

    #[test]
    fn heater_tiles_wrap_in_balanced_rows_and_stay_compact() {
        // U1 (4 tools, bed, chamber), XL (5 tools, bed), CORE One (tool, bed, chamber).
        for n in [6, 3, 2, 1] {
            for width in [412., 600., 692., 900., 1400.] {
                let (cols, rows, w) = tile_grid(n, width);
                assert!(cols * rows >= n && (rows - 1) * cols < n, "{n} in {width}");
                assert!(w <= TILE_MAX, "{n} tiles in {width}: {w} px each");
                assert!(w >= TILE_MIN, "{n} tiles in {width}: {w} px each");
                // Balanced: the last row is never short by a whole row's worth.
                assert!(rows * cols - n < cols, "{n} in {width}: {cols}×{rows}");
            }
        }
        assert_eq!(tile_grid(6, 612.), (3, 2, (612. - 20.) / 3.));
        assert_eq!(tile_grid(6, 1100.).0, 6);
    }

    #[test]
    fn camera_beside_from_about_1200_px_and_never_overflowing() {
        assert_eq!(info_width(detail(1100.)), None);
        for window in (1200..=2560).step_by(40) {
            let width = detail(window as f32);
            let base = info_width(width).unwrap_or_else(|| panic!("{window} px: stacked"));
            assert!((INFO_MIN..=INFO_MAX).contains(&base));
            for (tiles, info_h) in [(6, 0.), (6, 460.), (6, 560.), (3, 360.), (2, 300.)] {
                for aspect in [4. / 3., 16. / 9.] {
                    let (info, cols) = side_layout(width, base, info_h, aspect, tiles);
                    assert!(info >= base && cols >= 1);
                    // The camera keeps at least its narrowest card beside the column.
                    assert!(
                        width - info - GAP >= CAMERA_SIDE_MIN - 0.5,
                        "{window}: {info}"
                    );
                    let w = (info - 2. * CARD_PAD.x - TILE_GAP * (cols - 1) as f32) / cols as f32;
                    assert!(w >= TILE_MIN - 0.5, "{window}: {tiles} tiles {w} px");
                }
            }
        }
    }

    #[test]
    fn heater_states() {
        let p = &theme::DARK;
        let t = |c: f64, t: Option<f64>| Temperature {
            current: Some(c),
            target: t,
        };
        let word = |x: Temperature| heater_state(p, &x).map(|(w, _)| w);
        assert_eq!(word(t(120., Some(215.))), Some("Heating"));
        assert_eq!(word(t(214., Some(215.))), Some("Ready"));
        assert_eq!(word(t(230., Some(215.))), Some("Cooling"));
        assert_eq!(word(t(120., Some(0.))), Some("Cooling"));
        assert_eq!(word(t(24., Some(0.))), Some("Off"));
        assert_eq!(word(t(34., None)), None);
    }
}
