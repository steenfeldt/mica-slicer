// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Override by editing: with an object, a part or a height range selected, the process card
//! shows the same topic rail and sections as the profile, under a scope bar
//! `[Profile | 3DBenchy · 3 | Hull infill · 2]` (a segment per level the selection has, with
//! its override count). At an object or part scope the rows show the inherited value muted
//! behind a dashed outline; editing one makes an override at that scope, shown with the
//! changed marker and `3DBenchy · profile: 2` (a part: `was 2`) under its label, and ↺
//! removes it. Only the settings that level can change are listed (`settings::scope_allows`)
//! plus any override already there. A selected part also gets a header: its kind and size,
//! "Fit to selection", "Change shape" and "Save recipe".

use super::widgets;
use crate::app::App;
use crate::commands::Shape;
use crate::scene::Sub;
use crate::settings::Level;
use crate::theme;
use eframe::egui::{self, Pos2, Rect, Sense, Stroke, Vec2};
use mica_core::config::Config;
use mica_core::threemf::PartKind;
use std::collections::BTreeMap;

/// The object or part scope the settings list edits.
pub struct ScopeEnv {
    /// What the scope may change; `None`: nothing (negative and support volumes).
    pub level: Option<Level>,
    /// `3DBenchy`, `Hull infill`.
    pub name: String,
    /// A part or a height range (inherits from the object).
    pub part: bool,
    /// `object`, `part` or `range`, for "2 for this part".
    pub what: &'static str,
    pub overrides: BTreeMap<String, String>,
    /// The configuration without the scope's own overrides.
    pub inherited: Config,
    /// The configuration with them.
    pub config: Config,
}

impl ScopeEnv {
    /// Whether the list shows `key` at this scope: a setting the level can change, or one it
    /// already overrides.
    pub fn allows(&self, key: &str) -> bool {
        let real = crate::settings::synthetic_keys(key);
        let has = if real.is_empty() {
            self.overrides.contains_key(key)
        } else {
            real.iter().any(|k| self.overrides.contains_key(*k))
        };
        has || self
            .level
            .is_some_and(|l| crate::settings::scope_allows(l, key))
    }
}

/// The scope bar's levels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Segment {
    Profile,
    Object,
    Part,
}

impl App {
    /// The part the sidebar edits: the selected part, unless the scope bar went up to the
    /// object.
    pub(crate) fn scope_sub(&self) -> Option<Sub> {
        if self.editor.object_over_part {
            None
        } else {
            self.sub
        }
    }

    /// The scope bar's current segment.
    pub(crate) fn scope_segment(&self) -> Segment {
        match (
            self.editor.object_mode && self.selected.is_some(),
            self.scope_sub(),
        ) {
            (false, _) => Segment::Profile,
            (true, None) => Segment::Object,
            (true, Some(_)) => Segment::Part,
        }
    }

    /// Switches the scope bar (the tree's selection stays).
    pub(crate) fn set_scope(&mut self, s: Segment) {
        match s {
            Segment::Profile => self.editor.object_mode = false,
            Segment::Object => {
                self.editor.object_mode = true;
                self.editor.object_over_part = self.sub.is_some();
            }
            Segment::Part => {
                self.editor.object_mode = true;
                self.editor.object_over_part = false;
            }
        }
    }

    /// A new selection puts the sidebar at its deepest level (the part, else the object).
    pub(crate) fn follow_selection_scope(&mut self) {
        let now = self
            .selected
            .and_then(|i| self.plates.current().objects.get(i))
            .map(|o| (o.id, self.sub));
        if now != self.editor.scope_seen {
            if now.is_some() {
                self.editor.object_mode = true;
                self.editor.plate_mode = false;
            }
            self.editor.object_over_part = false;
            self.editor.scope_seen = now;
        }
    }

    /// The object or part scope being edited, `None` for the profile.
    pub(crate) fn scope_env(&self) -> Option<ScopeEnv> {
        if self.editor.tower_mode || !self.editor.object_mode {
            return None;
        }
        let i = self.selected?;
        let o = self.plates.current().objects.get(i)?;
        let plate = self.plate_config(self.plates.current).ok()?;
        let object = crate::worker::object_config(&plate, o).ok()?;
        let apply = |base: &Config, overrides: &BTreeMap<String, String>| {
            let mut c = base.clone();
            for (k, v) in overrides {
                let _ = c.set_raw(k, v);
            }
            c
        };
        Some(match self.scope_sub() {
            None => ScopeEnv {
                level: Some(Level::Object),
                name: super::tree::short_name(&o.name),
                part: false,
                what: "object",
                overrides: o.overrides.clone(),
                inherited: plate,
                config: object,
            },
            Some(Sub::Volume(id)) => {
                let v = o.volume(id)?;
                ScopeEnv {
                    level: match v.kind {
                        PartKind::NegativeVolume
                        | PartKind::SupportEnforcer
                        | PartKind::SupportBlocker => None,
                        _ => Some(Level::Modifier),
                    },
                    name: v.name.clone(),
                    part: true,
                    what: "part",
                    config: apply(&object, &v.overrides),
                    overrides: v.overrides.clone(),
                    inherited: object,
                }
            }
            Some(Sub::Range(id)) => {
                let r = o.range(id)?;
                ScopeEnv {
                    level: Some(Level::Range),
                    name: format!("Range {}", r.label()),
                    part: true,
                    what: "range",
                    config: apply(&object, &r.overrides),
                    overrides: r.overrides.clone(),
                    inherited: object,
                }
            }
        })
    }

    /// The scope bar and, for a selected part, its header.
    pub(crate) fn scope_bar(&mut self, ui: &mut egui::Ui) {
        let p = theme::of(ui);
        let Some(i) = self.selected else { return };
        let Some(o) = self.plates.current().objects.get(i) else {
            return;
        };
        let object = super::tree::short_name(&o.name);
        let own = o.overrides.len();
        let part = self.sub.map(|s| {
            let n = match s {
                Sub::Volume(id) => o.volume(id).map_or(0, |v| v.overrides.len()),
                Sub::Range(id) => o.range(id).map_or(0, |r| r.overrides.len()),
            };
            (crate::tree::part_name(o, s), n)
        });
        let mut segments = vec![
            (Segment::Profile, "Profile".to_string(), 1.),
            (
                Segment::Object,
                format!("{object} · {}", o.overrides.len()),
                1.3,
            ),
        ];
        if let Some((name, n)) = &part {
            segments.push((Segment::Part, format!("{name} · {n}"), 1.3));
        }
        let current = self.scope_segment();
        let (bar, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 30.), Sense::hover());
        ui.painter().rect_filled(bar, 7., p.search_bg);
        let total: f32 = segments.iter().map(|s| s.2).sum();
        let mut x = bar.left();
        let mut pick = None;
        let mut rects = vec![];
        for (k, (seg, text, w)) in segments.iter().enumerate() {
            let width = bar.width() * w / total;
            let r = Rect::from_min_size(Pos2::new(x, bar.top()), Vec2::new(width, bar.height()));
            x += width;
            let resp = ui.interact(r, ui.make_persistent_id(("scope-seg", k)), Sense::click());
            let active = *seg == current;
            if active {
                ui.painter().rect_filled(r.shrink(2.), 6., p.accent_surface);
            } else if resp.hovered() {
                ui.painter().rect_filled(r.shrink(2.), 6., p.hover);
            }
            if k > 0 && !active && segments[k - 1].0 != current {
                ui.painter().line_segment(
                    [
                        Pos2::new(r.left(), r.top() + 7.),
                        Pos2::new(r.left(), r.bottom() - 7.),
                    ],
                    Stroke::new(1., p.border_soft),
                );
            }
            let color = if active { p.mark_fg } else { p.label };
            let font = if active {
                theme::medium(12.)
            } else {
                theme::sans(12.)
            };
            let g = ui.painter().layout_no_wrap(text.clone(), font, color);
            let gx = (r.center().x - g.size().x / 2.).max(r.left() + 6.);
            ui.painter()
                .with_clip_rect(r.shrink2(Vec2::new(6., 0.)))
                .galley(Pos2::new(gx, r.center().y - g.size().y / 2.), g, color);
            let tip = match seg {
                Segment::Profile => "Edit the profile: every object prints with it".to_string(),
                Segment::Object => format!("Edit {object}'s own settings"),
                Segment::Part => "Edit the selected part's own settings".to_string(),
            };
            if resp.on_hover_text(tip).clicked() {
                pick = Some(*seg);
            }
            rects.push((*seg, r));
        }
        ui.painter().rect_stroke(
            bar,
            7.,
            Stroke::new(1., p.border_soft),
            egui::StrokeKind::Inside,
        );
        self.editor.scope_probe = rects;
        if let Some(s) = pick {
            self.set_scope(s);
        }
        ui.add_space(6.);
        let note = match self.scope_segment() {
            Segment::Profile => format!(
                "Settings for every object. {object} changes {}.",
                plural(own, "setting")
            ),
            Segment::Object => format!(
                "Gray values come from the profile; change one to override it for {object}."
            ),
            Segment::Part => match &part {
                Some((name, _)) => {
                    format!("Gray values come from {object}; a change applies inside {name}.")
                }
                None => String::new(),
            },
        };
        ui.label(
            egui::RichText::new(note)
                .font(theme::sans(11.5))
                .color(p.scope_text),
        );
        if self.scope_segment() == Segment::Part {
            self.part_header(ui);
        }
    }

    /// The selected part's header: kind and size, Fit to selection, Change shape, Save recipe.
    fn part_header(&mut self, ui: &mut egui::Ui) {
        let p = theme::of(ui);
        let (Some(i), Some(sub)) = (self.selected, self.sub) else {
            return;
        };
        let Some(o) = self.plates.current().objects.get(i) else {
            return;
        };
        let object = super::tree::short_name(&o.name);
        let part_name = crate::tree::part_name(o, sub);
        let mut text = false;
        let (kind, size) = match sub {
            Sub::Volume(id) => {
                let Some(v) = o.volume(id) else { return };
                let (a, b) = o.volume_bounds(v);
                let s = b - a;
                // A text or SVG part is not a primitive: no shape to guess or change.
                let shape = match v.emboss.as_ref().map(|e| e.config.text().is_some()) {
                    Some(is_text) => {
                        text = true;
                        if is_text { "Text" } else { "SVG" }
                    }
                    None => crate::intents::primitive_shape(&v.mesh).map_or("Mesh", |s| s.label()),
                };
                (
                    v.kind_label().to_lowercase(),
                    Some(format!("{shape} · {:.0} × {:.0} × {:.0} mm", s.x, s.y, s.z)),
                )
            }
            Sub::Range(_) => ("height range".to_string(), None),
        };
        let mut edit_text = false;
        let mut fit = false;
        let mut shape = None;
        let mut save = false;
        ui.add_space(8.);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.;
            ui.label(
                egui::RichText::new(&part_name)
                    .font(theme::semibold(15.))
                    .color(p.text),
            );
            ui.label(
                egui::RichText::new(format!("{kind} in {object}"))
                    .font(theme::sans(12.))
                    .color(p.muted),
            );
        });
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 6.;
            if let Some(size) = &size {
                ui.label(
                    egui::RichText::new(size)
                        .font(theme::sans(12.))
                        .color(p.muted_alt),
                );
                ui.label(egui::RichText::new("·").color(p.muted));
                if text {
                    if widgets::link(ui, "Edit text")
                        .on_hover_text("Open the Text tool on it (T, or double-click it)")
                        .clicked()
                    {
                        edit_text = true;
                    }
                } else {
                    if widgets::link(ui, "Fit to selection")
                        .on_hover_text("Fit the shape to the object's bounds")
                        .clicked()
                    {
                        fit = true;
                    }
                    ui.label(egui::RichText::new("·").color(p.muted));
                    let r = widgets::link(ui, "Change shape…");
                    egui::Popup::menu(&r).show(|ui| {
                        for s in Shape::ALL {
                            if ui.button(s.label()).clicked() {
                                shape = Some(s);
                                ui.close();
                            }
                        }
                    });
                }
                ui.label(egui::RichText::new("·").color(p.muted));
            }
            if widgets::link(ui, "Save recipe")
                .on_hover_text("Keep this part's kind, shape and settings for “+ Add › Recipes”")
                .clicked()
            {
                save = true;
            }
        });
        if let Some(name) = &mut self.editor.recipe_name {
            let mut done: Option<bool> = None;
            ui.add_space(4.);
            ui.horizontal(|ui| {
                let id = ui.make_persistent_id("recipe-name");
                let r = ui.add(
                    egui::TextEdit::singleline(name)
                        .id(id)
                        .hint_text("Recipe name")
                        .desired_width(ui.available_width() - 130.),
                );
                if !r.has_focus() && !r.lost_focus() && ui.memory(|m| m.focused().is_none()) {
                    r.request_focus();
                }
                if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    done = Some(true);
                }
                if widgets::styled_button(ui, "Save", !name.trim().is_empty(), true, 24.).clicked()
                {
                    done = Some(true);
                }
                if widgets::styled_button(ui, "Cancel", true, false, 24.).clicked() {
                    done = Some(false);
                }
            });
            match done {
                Some(true) => {
                    let name = self.editor.recipe_name.take().unwrap_or_default();
                    self.save_recipe(&name);
                }
                Some(false) => self.editor.recipe_name = None,
                None => {}
            }
        }
        if save && self.editor.recipe_name.is_none() {
            self.editor.recipe_name = Some(part_name);
        }
        if fit {
            self.fit_part_to_object();
        }
        if edit_text {
            self.edit_text(i, Some(sub));
        }
        if let Some(s) = shape {
            self.change_part_shape(s);
        }
    }
}

/// `1 setting`, `3 settings`, `no settings`.
fn plural(n: usize, word: &str) -> String {
    match n {
        0 => format!("no {word}s"),
        1 => format!("1 {word}"),
        n => format!("{n} {word}s"),
    }
}

/// The text a scope's empty list shows (negative and support volumes have no settings).
pub fn no_settings_text(env: &ScopeEnv) -> String {
    format!(
        "{} has no settings of its own: the shape itself is what it does.",
        env.name
    )
}
