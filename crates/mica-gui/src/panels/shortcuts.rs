// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: orca@8500fcdcca src/slic3r/GUI/KBShortcutsDialog.cpp :: KBShortcutsDialog::KBShortcutsDialog
// Source: orca@8500fcdcca src/slic3r/GUI/KBShortcutsDialog.cpp :: KBShortcutsDialog::create_page
// Changes: Same idea (the shortcuts in titled groups, key on the left, what it does on the
// right, opened with ?) as one searchable window in two columns instead of tabbed pages; the
// rows come from Mica's own key table (`keymap.rs`), so the list is always what Mica does;
// written from scratch for egui.

//! The "Keyboard shortcuts" window (`?`, or Help › Keyboard shortcuts): every shortcut of
//! [`crate::keymap::BINDINGS`] grouped as Orca groups them, plus the mouse gestures, with a
//! search field that filters by key or description.

use super::widgets;
use crate::app::App;
use crate::keymap::{self, Group, Row};
use crate::theme;
use eframe::egui::{self, Align2, Pos2, Rect, Sense, Stroke, Vec2};

const WINDOW_W: f32 = 940.;
/// Width of the key column.
const KEYS_W: f32 = 150.;

impl App {
    pub(crate) fn shortcuts_window(&mut self, ctx: &egui::Context) {
        let mut open = self.keys.dialog;
        let height = (ctx.content_rect().height() - 110.).clamp(320., 860.);
        let all = keymap::rows();
        let mouse: Vec<(String, &str)> = keymap::MOUSE
            .iter()
            .map(|(k, h)| (keymap::localise(k), *h))
            .collect();
        let search_id = egui::Id::unique("mica-shortcuts-search");
        let mut close = false;
        egui::Window::new("Keyboard shortcuts")
            .id(egui::Id::unique("mica-shortcuts"))
            .open(&mut open)
            .default_pos(ctx.content_rect().center_top() + egui::vec2(-WINDOW_W / 2., 56.))
            .fixed_size([WINDOW_W, height])
            .collapsible(false)
            .show(ctx, |ui| {
                let p = theme::of(ui);
                ui.label(
                    egui::RichText::new(
                        "The same keys as OrcaSlicer. Single-letter keys work in the 3D view, not while you type in a field.",
                    )
                    .font(theme::sans(12.))
                    .color(p.muted),
                );
                ui.add_space(8.);
                if std::mem::take(&mut self.keys.focus_dialog_search) {
                    ui.memory_mut(|m| m.request_focus(search_id.with("search")));
                }
                let rows = keymap::search(&all, &self.keys.search);
                let q = self.keys.search.trim().to_lowercase();
                let mouse: Vec<&(String, &str)> = mouse
                    .iter()
                    .filter(|(k, h)| {
                        q.is_empty()
                            || k.to_lowercase().contains(&q)
                            || h.to_lowercase().contains(&q)
                            || (q.len() >= 3 && "mouse".starts_with(&q))
                    })
                    .collect();
                let n = rows.len() + mouse.len();
                let annotation = if q.is_empty() {
                    String::new()
                } else {
                    format!("{n} shortcut{}", if n == 1 { "" } else { "s" })
                };
                widgets::search_field(
                    ui,
                    search_id,
                    &mut self.keys.search,
                    "Search shortcuts: a key or what it does",
                    &annotation,
                );
                ui.add_space(4.);
                egui::ScrollArea::vertical()
                    .max_height(height - 90.)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if n == 0 {
                            ui.add_space(24.);
                            ui.vertical_centered(|ui| {
                                ui.label(
                                    egui::RichText::new("No shortcut matches")
                                        .font(theme::sans(13.))
                                        .color(p.muted),
                                );
                            });
                            return;
                        }
                        // Sections in Orca's order, split into two columns of about equal
                        // height without breaking a section.
                        let mut sections: Vec<(&str, Vec<(&str, &str)>)> = [
                            Group::File,
                            Group::Edit,
                            Group::Objects,
                            Group::Tools,
                            Group::Painting,
                            Group::Camera,
                            Group::Preview,
                        ]
                        .into_iter()
                        .map(|g| {
                            let in_group: Vec<(&str, &str)> = rows
                                .iter()
                                .filter(|r: &&Row| r.group == g)
                                .map(|r| (r.keys.as_str(), r.help))
                                .collect();
                            (g.title(), in_group)
                        })
                        .collect();
                        sections.push((
                            "Mouse",
                            mouse.iter().map(|(k, h)| (k.as_str(), *h)).collect(),
                        ));
                        sections.retain(|(_, r)| !r.is_empty());
                        let weight = |s: &(&str, Vec<(&str, &str)>)| s.1.len() + 2;
                        let total: usize = sections.iter().map(weight).sum();
                        let mut split = sections.len();
                        let mut acc = 0;
                        for (i, s) in sections.iter().enumerate() {
                            if acc > 0 && acc + weight(s) / 2 > total / 2 {
                                split = i;
                                break;
                            }
                            acc += weight(s);
                        }
                        let (left, right) = sections.split_at(split);
                        ui.columns(2, |cols| {
                            for (col, part) in cols.iter_mut().zip([left, right]) {
                                col.set_width(col.available_width() - 10.);
                                for (title, entries) in part {
                                    widgets::group_title(col, title);
                                    col.add_space(2.);
                                    for (k, h) in entries {
                                        row(col, k, h, &q);
                                    }
                                }
                            }
                        });
                    });
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    close = true;
                }
            });
        self.keys.dialog = open && !close;
    }
}

/// One shortcut: key caps on the left (alternatives separated by "or"), the description,
/// wrapped, on the right; search matches highlighted.
fn row(ui: &mut egui::Ui, keys: &str, help: &str, query: &str) {
    let p = theme::of(ui);
    let width = ui.available_width();
    let desc_w = width - KEYS_W - 8.;
    let job = widgets::highlighted(help, query, theme::sans(12.5), p.text, p.mark_bg, p.mark_fg);
    let mut job = job;
    job.wrap.max_width = desc_w;
    let galley = ui.fonts_mut(|f| f.layout_job(job));
    let caps: Vec<&str> = keys.split(" / ").collect();
    let font = theme::medium(11.5);
    let cap_galleys: Vec<_> = caps
        .iter()
        .map(|c| {
            ui.painter()
                .layout_no_wrap(c.to_string(), font.clone(), p.text)
        })
        .collect();
    // Key caps flow over lines inside the key column.
    let mut lines = 1;
    let mut x = 0.;
    for g in &cap_galleys {
        let w = g.size().x + 12.;
        if x > 0. && x + w > KEYS_W {
            lines += 1;
            x = 0.;
        }
        x += w + 22.;
    }
    let cap_h = 20.;
    let h = (galley.size().y).max(lines as f32 * (cap_h + 4.) - 4.) + 8.;
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, h), Sense::hover());
    if response.hovered() {
        ui.painter().rect_filled(rect, 4., p.hover);
    }
    let painter = ui.painter();
    let mut x = rect.left() + 6.;
    let mut y = rect.top() + 4.;
    for (i, g) in cap_galleys.into_iter().enumerate() {
        let w = g.size().x + 12.;
        if x > rect.left() + 6. && x + w > rect.left() + KEYS_W {
            x = rect.left() + 6.;
            y += cap_h + 4.;
        }
        let cap = Rect::from_min_size(Pos2::new(x, y), Vec2::new(w, cap_h));
        painter.rect_filled(cap, 4., p.raised);
        painter.rect_stroke(
            cap,
            4.,
            Stroke::new(1., p.border_raised),
            egui::StrokeKind::Inside,
        );
        painter.galley(
            Pos2::new(cap.left() + 6., cap.center().y - g.size().y / 2.),
            g,
            p.text,
        );
        x += w;
        if i + 1 < caps.len() {
            painter.text(
                Pos2::new(x + 11., cap.center().y),
                Align2::CENTER_CENTER,
                "or",
                theme::sans(10.5),
                p.muted,
            );
            x += 22.;
        }
    }
    painter.galley(
        Pos2::new(
            rect.left() + KEYS_W + 8.,
            rect.top() + 4. + (cap_h - 16.) / 2.,
        ),
        galley,
        p.text,
    );
}
