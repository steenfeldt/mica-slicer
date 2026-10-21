// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The Printers dashboard: what the Machine tab shows when no single printer is chosen. A
//! grid of tiles, one per printer in My printers, each with a live camera thumbnail, the
//! connection and job state, progress and time left, temperatures and the loaded
//! filaments. Clicking a tile opens that printer's Machine view. Read-only: the tiles have
//! no controls that change a printer.

use super::{duration, machine, widgets};
use crate::app::App;
use crate::printhost::{
    cameras::{self, NoCamera, Role},
    monitor::Live,
    poll,
};
use crate::theme;
use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, Vec2};
use mica_printhost::{PrinterState, Temperature};

const TILE_MIN_W: f32 = 360.;
const GAP: f32 = 16.;

/// Which camera players a picture uses (each set stops what it did not draw this frame).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Players {
    Machine,
    MyPrinters,
}

/// Counts for the header line ("3 printers · 2 printing · 1 needs attention").
fn summary(lives: &[Option<&Live>]) -> String {
    let n = lives.len();
    let mut printing = 0;
    let mut attention = 0;
    for l in lives.iter().flatten() {
        match l.status.as_ref().map(|s| s.state) {
            _ if l.failed() => attention += 1,
            Some(PrinterState::Printing | PrinterState::Paused) => printing += 1,
            Some(PrinterState::Error) => attention += 1,
            _ => {}
        }
    }
    let mut parts = vec![format!("{n} printer{}", if n == 1 { "" } else { "s" })];
    if printing > 0 {
        parts.push(format!("{printing} printing"));
    }
    if attention > 0 {
        parts.push(format!(
            "{attention} need{} attention",
            if attention == 1 { "s" } else { "" }
        ));
    }
    parts.join(" · ")
}

/// The message with its first letter in upper case.
fn capitalised(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

/// `215°` or `215 → 240°` while heating or cooling towards a target.
fn degrees(t: &Temperature) -> String {
    match (t.current, t.target) {
        (Some(c), Some(target)) if target > 0. && (c - target).abs() > 3. => {
            format!("{c:.0} → {target:.0}°")
        }
        (Some(c), _) => format!("{c:.0}°"),
        (None, Some(target)) if target > 0. => format!("→ {target:.0}°"),
        _ => "—".into(),
    }
}

/// The words and colour of a printer's state pill.
pub fn state_pill(p: &theme::Palette, live: Option<&Live>) -> (&'static str, Color32) {
    let failed = live.is_some_and(Live::failed);
    let state = live.and_then(|l| l.status.as_ref()).map(|s| s.state);
    if failed {
        ("No answer", theme::DANGER)
    } else if live.is_none() || state.is_none() {
        ("Connecting…", p.muted)
    } else {
        (machine::state_word(state), machine::state_colour(p, state))
    }
}

/// A pill: dot and word on a tinted rounded rectangle, right-aligned at `right`.
fn pill(ui: &egui::Ui, right: Pos2, word: &str, colour: Color32) -> Rect {
    let g = ui
        .painter()
        .layout_no_wrap(word.to_string(), theme::medium(11.), colour);
    let rect = Rect::from_min_size(
        Pos2::new(right.x - g.size().x - 26., right.y),
        Vec2::new(g.size().x + 26., 20.),
    );
    ui.painter()
        .rect_filled(rect, 10., colour.gamma_multiply(0.16));
    ui.painter()
        .circle_filled(Pos2::new(rect.left() + 10., rect.center().y), 3.5, colour);
    ui.painter().galley(
        Pos2::new(rect.left() + 18., rect.center().y - g.size().y / 2.),
        g,
        colour,
    );
    rect
}

/// Single-line text truncated to `width`.
fn text_line(ui: &egui::Ui, at: Pos2, text: &str, font: egui::FontId, colour: Color32, width: f32) {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_string(), font, colour);
    job.wrap = egui::text::TextWrapping::truncate_at_width(width.max(10.));
    let g = ui.painter().layout_job(job);
    ui.painter().galley(at, g, colour);
}

impl App {
    /// The camera picture of the printer at `index` in `rect`: the newest frame (fitted,
    /// letterboxed on black), or what stands in its way. Starts the player on first use.
    pub(crate) fn camera_picture(
        &mut self,
        ui: &mut egui::Ui,
        rect: Rect,
        index: usize,
        role: Role,
    ) {
        self.camera_picture_in(ui, rect, index, role, Players::Machine);
    }

    /// `camera_picture` with the players of `set`: the Machine view's, or My printers' (its
    /// cards refresh a snapshot every few seconds, and only while the window is open).
    pub(crate) fn camera_picture_in(
        &mut self,
        ui: &mut egui::Ui,
        rect: Rect,
        index: usize,
        role: Role,
        set: Players,
    ) {
        let p = theme::of(ui);
        let ctx = ui.ctx().clone();
        let radius = if role == Role::Thumbnail { 0. } else { 8. };
        ui.painter()
            .rect_filled(rect, radius, Color32::from_rgb(10, 10, 12));
        let Some(printer) = self.printers.list.get(index).cloned() else {
            return;
        };
        let reported = self
            .printers
            .live(index)
            .and_then(|l| l.cameras.as_deref())
            .map(<[_]>::to_vec);
        let centre_text = |ui: &egui::Ui, text: &str, colour: Color32| {
            let mut job = egui::text::LayoutJob::simple(
                text.to_string(),
                theme::sans(12.),
                colour,
                rect.width() - 32.,
            );
            job.halign = egui::Align::Center;
            let g = ui.painter().layout_job(job);
            ui.painter().galley(
                Pos2::new(rect.center().x, rect.center().y - g.size().y / 2.),
                g,
                colour,
            );
        };
        let choice = match cameras::choose(&printer, reported.as_deref(), role) {
            Ok(c) => c,
            Err(NoCamera::Unknown) => {
                centre_text(ui, "Looking for a camera…", p.muted);
                return;
            }
            Err(NoCamera::None) => {
                centre_text(
                    ui,
                    if role == Role::Thumbnail {
                        "No camera"
                    } else {
                        "This printer reports no camera. Enter a camera URL in My printers \
                         (rtsp://…, an MJPEG stream or a snapshot JPEG)."
                    },
                    p.muted,
                );
                return;
            }
            Err(NoCamera::Unsupported(e) | NoCamera::BadUrl(e)) => {
                centre_text(ui, &e, p.muted);
                return;
            }
        };
        // A picture nobody can see plays nothing: not drawn means its player is dropped at
        // the end of the frame (scrolled out of view, or the window minimised).
        let minimised = !self.headless && ctx.input(|i| i.viewport().minimized == Some(true));
        let visible = ui.clip_rect().intersect(rect);
        if minimised || visible.width() < 1. || visible.height() < 1. {
            return;
        }
        let window_focused = self.headless || ctx.input(|i| i.focused);
        let rate = match (role, window_focused) {
            _ if set == Players::MyPrinters => mica_camera::Rate {
                fps: poll::CARD_FPS,
                max_width: 480,
            },
            // The Machine view's camera stays live while the window is merely in the
            // background (watched on a second screen); snapshots then refresh as fast as
            // the camera answers, up to `mica_camera::SNAPSHOT_MAX_FPS`.
            (Role::Focused, _) => mica_camera::Rate {
                fps: poll::FOCUSED_FPS,
                max_width: 0,
            },
            (Role::Thumbnail, false) => mica_camera::Rate {
                fps: poll::BACKGROUND_FPS,
                max_width: 640,
            },
            (Role::Thumbnail, true) => mica_camera::Rate {
                fps: poll::THUMBNAIL_FPS,
                max_width: 480,
            },
        };
        let printers = &self.printers;
        let views = match set {
            Players::Machine => &mut self.machine_view.cameras,
            Players::MyPrinters => &mut self.printers_editor.cameras,
        };
        let shown = views.show(
            &ctx,
            printer.id,
            role,
            &choice,
            || printers.connector(index),
            || printers.camera_auth(index),
            rate,
        );
        let view = match shown {
            Ok(v) => v,
            Err(e) => {
                centre_text(ui, &format!("Camera: {e}"), p.warn_text);
                return;
            }
        };
        let status = view.status();
        if let Some(t) = view.texture() {
            let size = t.size_vec2();
            if role == Role::Thumbnail {
                // Thumbnails fill the tile (cropping the long side); the focused view shows
                // the whole picture.
                let (sx, sy) = (rect.width() / size.x, rect.height() / size.y);
                let uv = if sx > sy {
                    let h = sy / sx;
                    Rect::from_min_max(Pos2::new(0., (1. - h) / 2.), Pos2::new(1., (1. + h) / 2.))
                } else {
                    let w = sx / sy;
                    Rect::from_min_max(Pos2::new((1. - w) / 2., 0.), Pos2::new((1. + w) / 2., 1.))
                };
                ui.painter().image(t.id(), rect, uv, Color32::WHITE);
            } else {
                let scale = (rect.width() / size.x).min(rect.height() / size.y);
                let shown = Rect::from_center_size(rect.center(), size * scale);
                ui.painter().image(
                    t.id(),
                    shown,
                    Rect::from_min_max(Pos2::ZERO, Pos2::new(1., 1.)),
                    Color32::WHITE,
                );
            }
        }
        // The source and its health, bottom left.
        let stale = status.last_frame.is_none_or(|t| t.elapsed().as_secs() > 15);
        let (badge, colour) = match (&status.error, view.texture().is_some(), stale) {
            (Some(_), _, true) if !view.starting() => ("NO SIGNAL", theme::DANGER),
            (_, true, false) => ("LIVE", p.accent_bright),
            _ => ("CONNECTING", p.muted),
        };
        let words = format!("{badge} · {}", view.label());
        let g = ui
            .painter()
            .layout_no_wrap(words, theme::medium(10.), Color32::WHITE);
        let tag = Rect::from_min_size(
            Pos2::new(rect.left() + 8., rect.bottom() - 8. - 18.),
            Vec2::new((g.size().x + 22.).min(rect.width() - 16.), 18.),
        );
        ui.painter()
            .rect_filled(tag, 9., Color32::from_black_alpha(170));
        ui.painter()
            .circle_filled(Pos2::new(tag.left() + 9., tag.center().y), 3., colour);
        ui.painter().with_clip_rect(tag).galley(
            Pos2::new(tag.left() + 16., tag.center().y - g.size().y / 2.),
            g,
            Color32::WHITE,
        );
        if view.texture().is_none() || stale {
            let text = match &status.error {
                Some(e) if !view.starting() => format!("{}\nRetrying…", capitalised(e)),
                _ => "Connecting to the camera…".into(),
            };
            centre_text(ui, &text, Color32::from_gray(190));
        }
        if role == Role::Focused
            && let Some(note) = &view.note()
        {
            text_line(
                ui,
                Pos2::new(rect.left() + 10., rect.top() + 8.),
                note,
                theme::sans(11.),
                Color32::from_gray(200),
                rect.width() - 20.,
            );
        }
    }

    /// The dashboard: a header and a grid of printer tiles.
    pub(crate) fn printers_dashboard(&mut self, ui: &mut egui::Ui) {
        let p = theme::of(ui);
        let lives: Vec<Option<&Live>> = (0..self.printers.list.len())
            .map(|i| self.printers.live(i))
            .collect();
        let line = summary(&lives);
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("Printers")
                    .font(theme::semibold(20.))
                    .color(p.text),
            );
            ui.add_space(8.);
            ui.label(
                egui::RichText::new(line)
                    .font(theme::sans(13.))
                    .color(p.muted),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if widgets::button(ui, "My printers…", true).clicked() {
                    self.dialogs.my_printers = true;
                }
            });
        });
        ui.add_space(4.);
        let mut open = None;
        egui::ScrollArea::vertical()
            .id_salt("printers-dashboard")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let width = ui.available_width();
                let columns = ((width + GAP) / (TILE_MIN_W + GAP)).floor().max(1.) as usize;
                let tile_w = ((width - GAP * (columns as f32 - 1.)) / columns as f32).floor();
                let camera_h = (tile_w * 9. / 16.).round();
                let tile_h = camera_h + 168.;
                let n = self.printers.list.len();
                let rows = n.div_ceil(columns);
                let (area, _) = ui.allocate_exact_size(
                    Vec2::new(width, rows as f32 * (tile_h + GAP)),
                    Sense::hover(),
                );
                for i in 0..n {
                    let (row, col) = (i / columns, i % columns);
                    let min = Pos2::new(
                        area.left() + col as f32 * (tile_w + GAP),
                        area.top() + row as f32 * (tile_h + GAP),
                    );
                    let rect = Rect::from_min_size(min, Vec2::new(tile_w, tile_h));
                    if !ui.is_rect_visible(rect) {
                        continue;
                    }
                    if self.printer_tile(ui, rect, camera_h, i) {
                        open = self.printers.id_at(i);
                    }
                }
            });
        if let Some(id) = open {
            self.machine_printer = Some(id);
            self.machine_view.confirm_cancel = false;
        }
    }

    /// One tile; true when clicked.
    fn printer_tile(&mut self, ui: &mut egui::Ui, rect: Rect, camera_h: f32, index: usize) -> bool {
        let p = theme::of(ui);
        let response = ui.interact(
            rect,
            egui::Id::unique(("printer-tile", index)),
            Sense::click(),
        );
        let hovered = response.hovered();
        ui.painter().rect_filled(rect, 12., p.panel);
        // The camera, clipped to the tile's rounded top.
        let camera = Rect::from_min_size(rect.min, Vec2::new(rect.width(), camera_h));
        {
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(camera));
            child.set_clip_rect(camera.intersect(ui.clip_rect()));
            self.camera_picture(&mut child, camera, index, Role::Thumbnail);
        }
        ui.painter().rect_stroke(
            rect,
            12.,
            Stroke::new(
                if hovered { 1.5 } else { 1. },
                if hovered { p.accent } else { p.border_soft },
            ),
            egui::StrokeKind::Inside,
        );
        let Some(printer) = self.printers.list.get(index).cloned() else {
            return false;
        };
        let live = self.printers.live(index);
        let colours = self.tool_colours();
        let pad = 14.;
        let left = rect.left() + pad;
        let inner_w = rect.width() - 2. * pad;
        let mut y = camera.bottom() + 12.;
        // Name and state.
        let cloud = printer.kind == crate::printhost::config::Kind::PrusaConnect;
        let (word, colour) = if cloud {
            ("Send via Connect", p.muted)
        } else {
            state_pill(p, live)
        };
        let pill_rect = pill(ui, Pos2::new(rect.right() - pad, y), word, colour);
        text_line(
            ui,
            Pos2::new(left, y),
            &printer.name,
            theme::semibold(15.),
            p.text,
            pill_rect.left() - left - 8.,
        );
        y += 22.;
        let model = live
            .and_then(|l| l.info.as_ref())
            .and_then(|i| i.model.clone())
            .or_else(|| printer.model.clone());
        let second = match model {
            Some(m) if m == printer.kind.label() => m,
            Some(m) => format!("{m} · {}", printer.kind.label()),
            None => format!("{} · {}", printer.kind.label(), printer.address),
        };
        text_line(
            ui,
            Pos2::new(left, y),
            &second,
            theme::sans(11.),
            p.muted,
            inner_w,
        );
        y += 24.;
        // The job.
        let status = live.and_then(|l| l.status.as_ref());
        match (live, status) {
            (Some(l), _) if l.failed() => {
                let why = l
                    .unavailable
                    .clone()
                    .or_else(|| {
                        l.error.as_ref().map(|e| {
                            crate::printhost::explain::explain(
                                e,
                                printer.kind,
                                &printer.name,
                                &printer.address,
                            )
                            .headline
                        })
                    })
                    .unwrap_or_default();
                text_line(
                    ui,
                    Pos2::new(left, y),
                    &why,
                    theme::sans(12.),
                    p.warn_text,
                    inner_w,
                );
                y += 18.;
                text_line(
                    ui,
                    Pos2::new(left, y),
                    "Mica keeps trying, less often while it gets no answer.",
                    theme::sans(11.),
                    p.muted,
                    inner_w,
                );
                y += 22.;
            }
            (_, Some(s))
                if matches!(
                    s.state,
                    PrinterState::Printing
                        | PrinterState::Paused
                        | PrinterState::Finished
                        | PrinterState::Error
                ) && (s.job_name.is_some() || s.progress.is_some()) =>
            {
                let name = s.job_name.clone().unwrap_or_else(|| "Unnamed job".into());
                text_line(
                    ui,
                    Pos2::new(left, y),
                    &name,
                    theme::medium(12.),
                    p.text,
                    inner_w,
                );
                y += 20.;
                let fraction = if s.state == PrinterState::Finished {
                    1.
                } else {
                    s.progress.unwrap_or(0.) as f32 / 100.
                };
                let bar_colour = match s.state {
                    PrinterState::Paused => theme::WARNING,
                    PrinterState::Error => theme::DANGER,
                    PrinterState::Finished => p.muted_alt,
                    _ => p.accent,
                };
                machine::bar(
                    ui,
                    Rect::from_min_size(Pos2::new(left, y), Vec2::new(inner_w, 6.)),
                    fraction,
                    bar_colour,
                );
                y += 12.;
                let left_text = match s.state {
                    PrinterState::Finished => "Done".to_string(),
                    _ => format!("{:.0} %", s.progress.unwrap_or(0.)),
                };
                text_line(
                    ui,
                    Pos2::new(left, y),
                    &left_text,
                    theme::mono(12.),
                    p.text,
                    80.,
                );
                if let Some(r) = s
                    .remaining
                    .filter(|_| matches!(s.state, PrinterState::Printing | PrinterState::Paused))
                {
                    let t = format!("{} left", duration(r));
                    let g = ui
                        .painter()
                        .layout_no_wrap(t, theme::mono(12.), p.muted_alt);
                    ui.painter().galley(
                        Pos2::new(rect.right() - pad - g.size().x, y),
                        g,
                        p.muted_alt,
                    );
                }
                y += 22.;
            }
            (_, Some(_)) => {
                text_line(
                    ui,
                    Pos2::new(left, y),
                    "Ready · no job running",
                    theme::sans(12.),
                    p.label,
                    inner_w,
                );
                y += 40.;
            }
            _ => {
                text_line(
                    ui,
                    Pos2::new(left, y),
                    if cloud {
                        "Status and queue available in Prusa Connect"
                    } else {
                        "Waiting for the printer…"
                    },
                    theme::sans(12.),
                    p.muted,
                    inner_w,
                );
                y += 40.;
            }
        }
        // Temperatures.
        if let Some(s) = status {
            // Bed and chamber first: with many tools the row runs out of room at the end.
            let mut parts: Vec<(String, String)> = vec![];
            if s.hotends.len() == 1 {
                parts.push(("Nozzle".into(), degrees(&s.hotends[0].temperature)));
            }
            if let Some(b) = &s.bed {
                parts.push(("Bed".into(), degrees(b)));
            }
            if let Some(c) = &s.chamber {
                parts.push(("Chamber".into(), degrees(c)));
            }
            if s.hotends.len() > 1 {
                for h in &s.hotends {
                    parts.push((format!("T{}", h.tool + 1), degrees(&h.temperature)));
                }
            }
            let mut x = left;
            for (label, value) in parts {
                let gl = ui
                    .painter()
                    .layout_no_wrap(label, theme::sans(11.), p.muted);
                let gv = ui.painter().layout_no_wrap(value, theme::mono(11.), p.text);
                let w = gl.size().x + 4. + gv.size().x;
                if x + w > rect.right() - pad {
                    break;
                }
                let lw = gl.size().x;
                ui.painter().galley(Pos2::new(x, y), gl, p.muted);
                ui.painter().galley(Pos2::new(x + lw + 4., y), gv, p.text);
                x += w + 12.;
            }
        }
        y += 22.;
        // Loaded filaments: a swatch per slot, the material under the colour.
        if let Some(l) = live
            && l.filaments_known
            && !l.filaments.is_empty()
        {
            let mut x = left;
            for f in &l.filaments {
                let c = f
                    .colour
                    .as_deref()
                    .and_then(crate::printhost::explain::parse_colour)
                    .or_else(|| colours.get(f.tool as usize).copied());
                let label = f.material.clone().unwrap_or_else(|| "?".into());
                let g = ui
                    .painter()
                    .layout_no_wrap(label, theme::sans(11.), p.label);
                let w = 14. + 4. + g.size().x;
                if x + w > rect.right() - pad {
                    break;
                }
                let sw = Rect::from_min_size(Pos2::new(x, y + 1.), Vec2::splat(12.));
                match c {
                    Some(c) => {
                        ui.painter().circle_filled(
                            sw.center(),
                            6.,
                            Color32::from_rgb(c[0], c[1], c[2]),
                        );
                    }
                    None => {
                        ui.painter()
                            .circle_stroke(sw.center(), 5.5, Stroke::new(1., p.muted));
                    }
                }
                ui.painter()
                    .circle_stroke(sw.center(), 6., Stroke::new(1., p.border_raised));
                ui.painter().galley(Pos2::new(x + 18., y), g, p.label);
                x += w + 12.;
            }
        } else {
            text_line(
                ui,
                Pos2::new(left, y),
                if live.is_some_and(|l| l.filaments_known) {
                    "Spools not reported"
                } else {
                    ""
                },
                theme::sans(11.),
                p.muted,
                inner_w,
            );
        }
        if hovered {
            ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
        }
        response
            .on_hover_text("Open this printer's Machine view")
            .clicked()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temperatures_read_plainly() {
        let t = |c, target| Temperature {
            current: Some(c),
            target: Some(target),
        };
        assert_eq!(degrees(&t(214.6, 215.)), "215°");
        assert_eq!(degrees(&t(120., 215.)), "120 → 215°");
        assert_eq!(degrees(&t(24., 0.)), "24°");
        assert_eq!(degrees(&Temperature::default()), "—");
        assert_eq!(summary(&[None, None]), "2 printers");
    }
}
