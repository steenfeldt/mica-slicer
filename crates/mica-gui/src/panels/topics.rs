// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The process card's settings for all objects: a topic rail on the left (Essentials, Quality,
//! Strength, Speed, Support, Cooling & temperatures, Multimaterial, Advanced: one button each,
//! icon above a short label) and one topic at a time on the right. The topic's header carries
//! its name, how many of its settings differ from the preset, a search field over every topic
//! and a "Changed" filter. Inside a topic, each section (Layer height, Seam, Infill, …) has a
//! line icon, a bold title and a rule; a click folds it (remembered across restarts), and the
//! header of the section being scrolled sticks to the top. Searching or filtering lists the
//! matching rows of every topic, grouped by topic, folded sections included. In Preview the
//! same rail has one more item on top, "Preview", whose page is the legend, totals and layer
//! (`preview_side.rs`); Preview keeps its own open topic, Prepare its own.

use super::{
    editor::{self, Edit, FieldContext},
    scope::ScopeEnv,
    settings_icons,
    sidebar::{GroupDef, RowDef, TopicDef, section_summary, tool_of},
    widgets::{self, LEFT, MARGIN, SEARCH_H},
};
use crate::app::App;
use crate::overlays::Target;
use crate::settings;
use crate::theme;
use eframe::egui::{self, Align2, Pos2, Rect, Sense, Stroke, Vec2};
use mica_core::config::Config;
use std::collections::{BTreeMap, HashSet};

/// Width of the topic rail with labels.
pub const RAIL_W: f32 = 64.;
/// Width of the icon-only rail of a narrow sidebar.
pub const RAIL_W_COMPACT: f32 = 40.;
/// Below this card width the rail drops its labels.
pub const COMPACT_BELOW: f32 = 340.;
const RAIL_BUTTON_H: f32 = 50.;
const RAIL_BUTTON_H_COMPACT: f32 = 34.;
const RAIL_GAP: f32 = 4.;
/// Height of a section header (icon, title, rule).
pub const SECTION_H: f32 = 32.;
/// Height of a topic heading in search / filter results.
const TOPIC_H: f32 = 30.;
/// The Preview's own rail item, above the settings topics (Preview only).
pub const PREVIEW_TOPIC: &str = "Preview";

/// What the settings list drew in the last frame, for tests and screenshots.
#[derive(Clone, Debug, Default)]
#[cfg_attr(not(test), allow(dead_code))] // Read by the sidebar tests.
pub struct Probe {
    /// The topic picked in the rail.
    pub topic: String,
    /// Search or the Changed filter was on: rows of every topic.
    pub filtering: bool,
    /// The rail showed icons only.
    pub compact: bool,
    pub rows: Vec<ProbeRow>,
    pub headers: Vec<ProbeHeader>,
    /// Rail buttons: topic name and screen rect.
    pub rail: Vec<(String, Rect)>,
    pub changed_chip: Option<Rect>,
    /// The sticky header: section id and screen rect.
    pub sticky: Option<(String, Rect)>,
    /// Top of the scroll area's visible part (screen coordinates).
    pub visible_top: f32,
    /// Settings of the picked topic that differ from the preset.
    pub topic_changed: usize,
    /// The Changed chip's count: changed settings of every topic.
    pub all_changed: usize,
    /// The line shown in place of rows when the picked topic lists none.
    pub note: Option<String>,
}

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Clone, Debug)]
pub struct ProbeRow {
    pub topic: &'static str,
    /// The section's id (`Seam`, `Cooling · tool 2`).
    pub section: String,
    pub key: &'static str,
}

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Clone, Debug)]
pub struct ProbeHeader {
    /// `Topic/Section`.
    pub id: String,
    pub rect: Rect,
    pub collapsed: bool,
    /// The "N changed" hint on a folded header.
    pub changed: usize,
}

/// Which rows a plan keeps.
#[derive(Clone, Copy)]
enum Mode {
    /// One topic; `show_more` reveals Essentials' rows behind "Show N more settings".
    Topic { show_more: bool },
    /// Every topic: rows matching the search, only changed ones with `changed`.
    Filter { changed: bool },
}

/// One row to draw and the dependents drawn with it.
struct PlanRow<'a> {
    def: &'a RowDef,
    force: bool,
    /// The row itself matches (a dependent can match while its parent does not).
    own: bool,
    deps: Vec<&'static str>,
}

struct PlanSection<'a> {
    topic: &'static str,
    group: &'a GroupDef,
    rows: Vec<PlanRow<'a>>,
    /// Rows (and dependents) of the plan that differ from the preset.
    changed: usize,
}

impl PlanSection<'_> {
    fn id(&self) -> String {
        format!("{}/{}", self.topic, self.group.id)
    }
}

/// Everything a row needs besides the app's overlays, for one frame.
struct Env<'a> {
    app: &'a App,
    config: &'a Config,
    preset_overrides: Option<&'a BTreeMap<String, String>>,
    search: &'a str,
    /// `global/Topic` and `global/Topic/dep` field ids per topic.
    ids: BTreeMap<&'static str, (String, String)>,
    /// The object or part scope being edited (`scope.rs`); `None`: the profile.
    scope: Option<&'a ScopeEnv>,
}

impl<'a> Env<'a> {
    fn searching(&self) -> bool {
        !self.search.trim().is_empty()
    }

    fn advanced(&self, topic: &str) -> bool {
        self.app.prefs.advanced
            || self.searching()
            || self.app.prefs.expanded_sections.iter().any(|s| s == topic)
    }

    /// The overrides a row edits: the scope's, else the target's unsaved changes.
    fn overlay(&self, target: Target) -> &crate::overlays::Overlay {
        match self.scope {
            Some(s) => &s.overrides,
            None => self.app.overlays.get(target),
        }
    }

    fn inherit(&self) -> Option<editor::Inherit<'_>> {
        self.scope.map(|s| editor::Inherit {
            config: &s.inherited,
            scope: &s.name,
            part: s.part,
        })
    }

    /// Whether the scope lists a row at all: profile rows always; at an object or part scope
    /// only print settings that level can change (or already overrides).
    fn allowed(&self, target: Target, key: &str) -> bool {
        match self.scope {
            None => true,
            Some(s) => target == Target::Print && s.allows(key),
        }
    }

    fn ctx(&self, topic: &'static str, target: Target, force: bool) -> FieldContext<'_> {
        FieldContext {
            config: self.config,
            overlay: self.overlay(target),
            inherit: self.inherit(),
            preset_overrides: self.preset_overrides,
            tool: tool_of(target),
            advanced: self.advanced(topic),
            search: self.search,
            id: self.ids.get(topic).map_or("global", |(id, _)| id.as_str()),
            force,
        }
    }

    fn dep_ctx(&self, topic: &'static str, target: Target) -> FieldContext<'_> {
        FieldContext {
            config: self.config,
            overlay: self.overlay(target),
            inherit: self.inherit(),
            preset_overrides: self.preset_overrides,
            tool: tool_of(target),
            advanced: true,
            search: self.search,
            id: self
                .ids
                .get(topic)
                .map_or("global/dep", |(_, id)| id.as_str()),
            force: true,
        }
    }

    fn keep(ctx: &FieldContext<'_>, key: &str, mode: Mode) -> bool {
        match mode {
            Mode::Topic { .. } => ctx.visible(key),
            Mode::Filter { changed } => ctx.visible(key) && (!changed || ctx.modified(key)),
        }
    }

    /// The rows of one section under `mode`. `only_changed` keeps changed rows only (Essentials'
    /// hidden "more" rows); `seen` drops rows an earlier topic already listed.
    fn plan_group<'g>(
        &self,
        topic: &'static str,
        group: &'g GroupDef,
        force: bool,
        mode: Mode,
        only_changed: bool,
        mut seen: Option<&mut HashSet<(Target, &'static str)>>,
    ) -> PlanSection<'g> {
        let mut rows = Vec::new();
        let mut changed = 0;
        let kept: Vec<&RowDef> = group
            .rows
            .iter()
            .filter(|r| {
                let force = force || settings::is_topic_key(topic, r.key);
                let c = self.ctx(topic, r.target, force);
                self.allowed(r.target, r.key)
                    && Self::keep(&c, r.key, mode)
                    && (!only_changed || c.modified(r.key))
            })
            .collect();
        let group_keys: Vec<&str> = kept.iter().map(|r| r.key).collect();
        for r in &group.rows {
            let force = force || settings::is_topic_key(topic, r.key);
            let c = self.ctx(topic, r.target, force);
            let mut own = group_keys.contains(&r.key);
            if let Some(seen) = seen.as_deref_mut()
                && own
                && !seen.insert((r.target, r.key))
            {
                own = false;
            }
            // A synthetic row (Essentials' "Supports") stands for real keys: once another
            // topic listed one of them, it is the same change, not one more.
            if let Some(seen) = seen.as_deref()
                && own
                && settings::synthetic_keys(r.key)
                    .iter()
                    .any(|k| seen.contains(&(r.target, *k)))
            {
                own = false;
            }
            let mut deps = Vec::new();
            // Dependents appear right under a switched-on parent, indented, unless they are
            // rows of this section anyway.
            if !only_changed && settings::dependents_active(r.key, self.config) {
                let d = self.dep_ctx(topic, r.target);
                for &key in settings::dependents(r.key) {
                    if group_keys.contains(&key)
                        || !self.allowed(r.target, key)
                        || !Self::keep(&d, key, mode)
                        || !crate::brim_ears::option_shown(key, self.config)
                    {
                        continue;
                    }
                    if let Some(seen) = seen.as_deref_mut()
                        && !seen.insert((r.target, key))
                    {
                        continue;
                    }
                    if d.modified(key) {
                        changed += 1;
                    }
                    deps.push(key);
                }
            }
            if own && c.modified(r.key) {
                changed += 1;
            }
            if own || !deps.is_empty() {
                rows.push(PlanRow {
                    def: r,
                    force,
                    own,
                    deps,
                });
            }
        }
        PlanSection {
            topic,
            group,
            rows,
            changed,
        }
    }

    /// The sections of one topic.
    fn plan_topic<'g>(&self, topic: &'g TopicDef, mode: Mode) -> Vec<PlanSection<'g>> {
        let essentials = topic.name == "Essentials";
        let mut out: Vec<PlanSection<'g>> = topic
            .groups
            .iter()
            .map(|g| self.plan_group(topic.name, g, essentials, mode, false, None))
            .collect();
        if essentials {
            let only_changed = matches!(mode, Mode::Topic { show_more: false });
            out.extend(
                topic
                    .more
                    .iter()
                    .map(|g| self.plan_group(topic.name, g, true, mode, only_changed, None)),
            );
        }
        out
    }

    /// Search / filter results of every topic. A row listed by another topic is not repeated
    /// in Essentials (which only gathers rows from the others).
    fn plan_all<'g>(&self, topics: &'g [TopicDef], changed: bool) -> Vec<Vec<PlanSection<'g>>> {
        let mode = Mode::Filter { changed };
        let mut seen = HashSet::new();
        let mut order: Vec<usize> = (0..topics.len()).collect();
        order.sort_by_key(|&i| topics[i].name == "Essentials");
        let mut out: Vec<Vec<PlanSection<'g>>> = topics.iter().map(|_| Vec::new()).collect();
        for i in order {
            let topic = &topics[i];
            let essentials = topic.name == "Essentials";
            for g in topic.groups.iter().chain(topic.more.iter()) {
                let section =
                    self.plan_group(topic.name, g, essentials, mode, false, Some(&mut seen));
                out[i].push(section);
            }
        }
        out
    }

    /// Simple-mode rows of a topic hidden behind "Show N more settings".
    fn hidden(&self, topic: &TopicDef) -> usize {
        if topic.name == "Essentials" {
            topic
                .more
                .iter()
                .flat_map(|g| g.rows.iter())
                .filter(|r| {
                    self.allowed(r.target, r.key)
                        && !self.ctx(topic.name, r.target, true).modified(r.key)
                })
                .count()
        } else {
            topic
                .groups
                .iter()
                .flat_map(|g| g.rows.iter())
                // Rows "Show N more settings" reveals: shown in full, not in the simple list.
                .filter(|r| {
                    self.allowed(r.target, r.key)
                        && !settings::is_topic_key(topic.name, r.key)
                        && self.ctx(topic.name, r.target, true).shown(r.key)
                        && !self.ctx(topic.name, r.target, false).shown(r.key)
                })
                .count()
        }
    }
}

fn row_count(sections: &[PlanSection<'_>]) -> usize {
    sections
        .iter()
        .flat_map(|s| s.rows.iter())
        .map(|r| usize::from(r.own) + r.deps.len())
        .sum()
}

fn changed_count(sections: &[PlanSection<'_>]) -> usize {
    sections.iter().map(|s| s.changed).sum()
}

/// The short rail label of a topic.
pub fn short_name(topic: &str) -> &str {
    match topic {
        "Cooling & temperatures" => "Cooling",
        "Multimaterial" => "Colors",
        other => other,
    }
}

/// What a frame of the list asks the app to do afterwards.
#[derive(Default)]
struct Actions {
    edits: Vec<(Target, Edit)>,
    fold: Vec<String>,
    more: Option<&'static str>,
    pick: Option<&'static str>,
}

impl App {
    /// The topic rail and the picked topic (or search / filter results across all topics).
    pub(super) fn settings_list(&mut self, ui: &mut egui::Ui) {
        let p = theme::of(ui);
        // An object or part scope (the scope bar): its own overrides over what it inherits.
        let scope = self.scope_env();
        if let Some(s) = &scope
            && s.level.is_none()
            && s.overrides.is_empty()
        {
            ui.add_space(14.);
            ui.horizontal(|ui| {
                ui.add_space(LEFT);
                ui.label(
                    egui::RichText::new(super::scope::no_settings_text(s))
                        .font(theme::sans(12.5))
                        .color(p.muted),
                );
            });
            self.editor.sidebar_probe = Probe::default();
            return;
        }
        let Some(config) = scope
            .as_ref()
            .map(|s| s.config.clone())
            .or_else(|| self.effective_config().cloned())
        else {
            if let Err(e) = &self.config {
                ui.horizontal(|ui| {
                    ui.add_space(16.);
                    ui.colored_label(ui.visuals().error_fg_color, e);
                });
            }
            return;
        };
        let preset_overrides = self
            .profiles
            .store
            .load(mica_profiles::prusa::Kind::Print, &self.profiles.process)
            .ok()
            .map(|p| p.settings)
            .filter(|_| scope.is_none());
        let mut topics = self.sections();
        if let Some(s) = &scope {
            // Only topics with something this scope can change.
            topics.retain(|t| {
                t.groups
                    .iter()
                    .chain(t.more.iter())
                    .flat_map(|g| g.rows.iter())
                    .any(|r| r.target == Target::Print && s.allows(r.key))
            });
        }
        // A topic with nothing to show even in full (every row is for the other slicing
        // rules or not implemented) leaves the rail rather than opening empty.
        {
            let env = Env {
                app: &*self,
                config: &config,
                preset_overrides: preset_overrides.as_ref(),
                search: "",
                ids: BTreeMap::new(),
                scope: scope.as_ref(),
            };
            topics.retain(|t| {
                t.groups
                    .iter()
                    .chain(t.more.iter())
                    .flat_map(|g| g.rows.iter())
                    .any(|r| {
                        env.allowed(r.target, r.key) && env.ctx(t.name, r.target, true).shown(r.key)
                    })
            });
        }
        // In Preview the rail has the "Preview" page on top (`preview_side.rs`) and its own
        // topic (`None`: the Preview page); Prepare keeps its topic in the preferences.
        let preview_rail = self.preview_side.in_rail;
        if preview_rail {
            if let Some(t) = &self.preview_side.topic
                && !topics.iter().any(|d| d.name == t)
            {
                self.preview_side.topic = None;
            }
        } else if scope.is_none() && !topics.iter().any(|t| t.name == self.prefs.settings_topic) {
            self.prefs.settings_topic = "Essentials".into();
        }
        let on_page = preview_rail && self.preview_side.topic.is_none();
        let current_topic = if preview_rail {
            self.preview_side.topic.clone().unwrap_or_default()
        } else {
            self.prefs.settings_topic.clone()
        };
        let id_prefix = match &scope {
            None => "global",
            Some(s) if s.part => "part",
            Some(_) => "object",
        };
        let focus_search = std::mem::take(&mut self.keys.focus_settings_search);
        let scroll_to_topic = std::mem::take(&mut self.editor.topic_picked);
        let mut search = self.editor.search.clone();
        let mut changed_only = self.editor.changed_only;

        let full = ui.available_rect_before_wrap();
        let compact = full.width() < COMPACT_BELOW;
        let rail_w = if compact { RAIL_W_COMPACT } else { RAIL_W };
        let top = ui.cursor().top();
        let visible_top = ui.clip_rect().top();
        let content_rect = Rect::from_min_max(
            Pos2::new(full.left() + rail_w, top),
            Pos2::new(full.right(), full.bottom().max(top + 1.)),
        );
        let mut probe = Probe {
            topic: if on_page {
                "Preview".to_string()
            } else {
                current_topic.clone()
            },
            compact,
            visible_top,
            ..Default::default()
        };
        let mut actions = Actions::default();
        let mut topic_marks: Vec<(usize, usize)> = Vec::new();
        let content = ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(content_rect)
                .layout(egui::Layout::top_down(egui::Align::Min)),
            |ui| {
                ui.set_width(content_rect.width());
                if on_page {
                    // The Preview page: legend, totals and layer; the rail keeps its marks.
                    self.preview_page(ui);
                    let env = Env {
                        app: &*self,
                        config: &config,
                        preset_overrides: preset_overrides.as_ref(),
                        search: "",
                        ids: BTreeMap::new(),
                        scope: None,
                    };
                    return topics
                        .iter()
                        .map(|t| changed_count(&env.plan_topic(t, Mode::Topic { show_more: true })))
                        .collect();
                }
                // Header: title and count, then search and the Changed chip. Drawn before
                // the plan so this frame's typing shapes the list below it.
                ui.add_space(12.);
                let (title_row, _) =
                    ui.allocate_exact_size(Vec2::new(ui.available_width(), 24.), Sense::hover());
                ui.add_space(8.);
                let (search_row, _) = ui
                    .allocate_exact_size(Vec2::new(ui.available_width(), SEARCH_H), Sense::hover());
                let chip_font = theme::sans(12.5);
                let chip_text_w = ui
                    .painter()
                    .layout_no_wrap("Changed 000".into(), chip_font.clone(), p.text)
                    .size()
                    .x;
                let chip = Rect::from_min_max(
                    Pos2::new(
                        search_row.right() - MARGIN - chip_text_w - 16.,
                        search_row.top(),
                    ),
                    Pos2::new(search_row.right() - MARGIN, search_row.bottom()),
                );
                let field = Rect::from_min_max(
                    Pos2::new(search_row.left() + 12., search_row.top()),
                    Pos2::new(chip.left() - 6., search_row.bottom()),
                );
                let search_id = ui.make_persistent_id("topic-search");
                if focus_search {
                    // ⌘F (keymap.rs): the search field takes the keyboard.
                    ui.memory_mut(|m| m.request_focus(search_id.with("search")));
                }
                ui.scope_builder(egui::UiBuilder::new().max_rect(field), |ui| {
                    widgets::search_field(ui, search_id, &mut search, "Search all topics", "");
                });
                let chip_response =
                    ui.interact(chip, ui.make_persistent_id("changed-chip"), Sense::click());
                if chip_response.clicked() {
                    changed_only = !changed_only;
                }
                let chip_response = chip_response
                    .on_hover_text("Only settings that differ from the preset, in every topic");
                probe.changed_chip = Some(chip);
                ui.add_space(12.);
                let (rule, _) =
                    ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.), Sense::hover());
                ui.painter().line_segment(
                    [rule.left_top(), rule.right_top()],
                    Stroke::new(1., p.border),
                );
                ui.add_space(4.);
                if scroll_to_topic && title_row.top() < visible_top {
                    ui.scroll_to_rect(title_row, Some(egui::Align::Min));
                }

                let ids = topics
                    .iter()
                    .map(|t| {
                        (
                            t.name,
                            (
                                format!("{id_prefix}/{}", t.name),
                                format!("{id_prefix}/{}/dep", t.name),
                            ),
                        )
                    })
                    .collect();
                let env = Env {
                    app: &*self,
                    config: &config,
                    preset_overrides: preset_overrides.as_ref(),
                    search: &search,
                    ids,
                    scope: scope.as_ref(),
                };
                let filtering = changed_only || env.searching();
                probe.filtering = filtering;
                // Changed settings per topic (rail marks, header count) and in all.
                let per_topic: Vec<usize> = topics
                    .iter()
                    .map(|t| {
                        let all = Env {
                            search: "",
                            ids: BTreeMap::new(),
                            ..env
                        };
                        changed_count(&all.plan_topic(t, Mode::Topic { show_more: true }))
                    })
                    .collect();
                let all_changed: usize = {
                    let all = Env {
                        search: "",
                        ids: BTreeMap::new(),
                        ..env
                    };
                    all.plan_all(&topics, true)
                        .iter()
                        .map(|s| row_count(s))
                        .sum()
                };
                let picked = topics
                    .iter()
                    .position(|t| t.name == current_topic)
                    .unwrap_or(0);
                probe.topic_changed = per_topic[picked];
                probe.all_changed = all_changed;

                // The chip: "Changed N", accent while on.
                let painter = ui.painter();
                let hot = chip_response.hovered();
                painter.rect_filled(
                    chip,
                    6.,
                    if changed_only {
                        p.accent_surface
                    } else if hot {
                        p.hover
                    } else {
                        p.search_bg
                    },
                );
                painter.rect_stroke(
                    chip,
                    6.,
                    Stroke::new(
                        1.,
                        if changed_only {
                            p.accent
                        } else {
                            p.border_soft
                        },
                    ),
                    egui::StrokeKind::Inside,
                );
                let chip_label = if all_changed > 0 {
                    format!("Changed {all_changed}")
                } else {
                    "Changed".to_string()
                };
                painter.text(
                    chip.center(),
                    Align2::CENTER_CENTER,
                    chip_label,
                    chip_font,
                    if changed_only { p.mark_fg } else { p.label },
                );
                if hot {
                    ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
                }

                let mut drawn = DrawState {
                    visible_top,
                    sticky: None,
                    probe: &mut probe,
                    actions: &mut actions,
                };
                let (title, count) = if filtering {
                    let plans = env.plan_all(&topics, changed_only);
                    let total: usize = plans.iter().map(|s| row_count(s)).sum();
                    for (i, sections) in plans.iter().enumerate() {
                        topic_marks.push((i, row_count(sections)));
                    }
                    draw_results(ui, &env, &topics, &plans, &mut drawn);
                    if total == 0 {
                        ui.add_space(14.);
                        ui.horizontal(|ui| {
                            ui.add_space(LEFT);
                            ui.label(
                                egui::RichText::new(if env.searching() {
                                    "No setting matches the search."
                                } else {
                                    "Every setting is as the preset has it."
                                })
                                .font(theme::sans(12.5))
                                .color(p.muted),
                            );
                        });
                    }
                    (
                        if env.searching() {
                            "Search results".to_string()
                        } else {
                            "Changed settings".to_string()
                        },
                        format!("{total} result{}", if total == 1 { "" } else { "s" }),
                    )
                } else {
                    let topic = &topics[picked];
                    let show_more = self.prefs.advanced
                        || self.prefs.expanded_sections.iter().any(|s| s == topic.name);
                    let plan = env.plan_topic(topic, Mode::Topic { show_more });
                    draw_topic(ui, &env, &plan, &mut drawn);
                    let hidden = if show_more { 0 } else { env.hidden(topic) };
                    if row_count(&plan) == 0 {
                        // Never an empty topic: say why, with the way to the rest below.
                        let why = if hidden > 0 {
                            format!("{} has advanced settings only.", topic.name)
                        } else {
                            format!("Nothing in {} can be changed here.", topic.name)
                        };
                        drawn.probe.note = Some(why.clone());
                        ui.add_space(10.);
                        ui.horizontal(|ui| {
                            ui.add_space(LEFT);
                            ui.label(
                                egui::RichText::new(why)
                                    .font(theme::sans(12.5))
                                    .color(p.muted),
                            );
                        });
                    }
                    let expanded = self.prefs.expanded_sections.iter().any(|s| s == topic.name);
                    if hidden > 0 || (expanded && !self.prefs.advanced) {
                        let text = if expanded {
                            "Show fewer settings".to_string()
                        } else {
                            format!(
                                "Show {hidden} more setting{}",
                                if hidden == 1 { "" } else { "s" }
                            )
                        };
                        ui.add_space(8.);
                        ui.horizontal(|ui| {
                            ui.add_space(LEFT);
                            if widgets::link(ui, &text).clicked() {
                                drawn.actions.more = Some(topic.name);
                            }
                        });
                    }
                    let n = per_topic[picked];
                    (
                        topic.name.to_string(),
                        match (n, &scope) {
                            (0, _) => String::new(),
                            (n, Some(s)) => format!("{n} for this {}", s.what),
                            (n, None) => format!("{n} changed"),
                        },
                    )
                };
                ui.add_space(12.);
                // The sticky header over the rows that scrolled past the top.
                if let Some((rect, section, icon)) = drawn.sticky.take() {
                    let r = ui.interact(
                        rect,
                        ui.make_persistent_id(("sticky", &section)),
                        Sense::click(),
                    );
                    ui.painter().rect_filled(rect, 0., p.panel);
                    paint_section_header(
                        ui,
                        rect,
                        &icon,
                        if filtering { None } else { Some(false) },
                        0,
                        r.hovered() && !filtering,
                    );
                    ui.painter().line_segment(
                        [rect.left_bottom(), rect.right_bottom()],
                        Stroke::new(1., p.border),
                    );
                    if r.clicked() && !filtering {
                        drawn.actions.fold.push(section.clone());
                    }
                    drawn.probe.sticky = Some((section, rect));
                }
                // Title row, painted now that the counts are known.
                let painter = ui.painter();
                let galley = painter.layout_no_wrap(title.clone(), theme::semibold(18.), p.text);
                let count_galley = painter.layout_no_wrap(count, theme::sans(12.), p.scope_text);
                let count_x = title_row.right() - MARGIN - count_galley.size().x;
                painter.galley(
                    Pos2::new(
                        count_x,
                        title_row.center().y - count_galley.size().y / 2. + 2.,
                    ),
                    count_galley,
                    p.scope_text,
                );
                painter
                    .with_clip_rect(Rect::from_min_max(
                        title_row.min,
                        Pos2::new(count_x - 8., title_row.bottom()),
                    ))
                    .galley(
                        Pos2::new(
                            title_row.left() + 12.,
                            title_row.center().y - galley.size().y / 2.,
                        ),
                        galley,
                        p.text,
                    );
                per_topic
            },
        );
        let per_topic = content.inner;
        // The rail: as tall as the topic (or the buttons), its buttons kept in view.
        let step = if compact {
            RAIL_BUTTON_H_COMPACT
        } else {
            RAIL_BUTTON_H
        } + RAIL_GAP;
        let rail_names: Vec<&'static str> = preview_rail
            .then_some(PREVIEW_TOPIC)
            .into_iter()
            .chain(topics.iter().map(|t| t.name))
            .collect();
        let offset = usize::from(preview_rail);
        let buttons_h = step * rail_names.len() as f32;
        let mut bottom = content.response.rect.bottom();
        if bottom < top + buttons_h + 16. {
            ui.add_space(top + buttons_h + 16. - bottom);
            bottom = top + buttons_h + 16.;
        }
        let rail = Rect::from_min_max(
            Pos2::new(full.left(), top),
            Pos2::new(full.left() + rail_w, bottom),
        );
        ui.painter().line_segment(
            [rail.right_top(), rail.right_bottom()],
            Stroke::new(1., p.border),
        );
        let y0 = (visible_top + 8.)
            .max(rail.top() + 8.)
            .min(rail.bottom() - buttons_h - 4.);
        for (k, &name) in rail_names.iter().enumerate() {
            let is_page = preview_rail && k == 0;
            // The topic's index (meaningless for the Preview page).
            let i = k.wrapping_sub(offset);
            let h = step - RAIL_GAP;
            let r = Rect::from_min_size(
                Pos2::new(rail.left() + 4., y0 + k as f32 * step),
                Vec2::new(rail_w - 8., h),
            );
            let id = ui.make_persistent_id(("rail", name));
            let response = ui.interact(r, id, Sense::click());
            let active = if is_page {
                on_page
            } else {
                !on_page && !probe.filtering && name == probe.topic
            };
            let matches = topic_marks
                .iter()
                .find(|(t, _)| *t == i)
                .map_or(1, |&(_, n)| n);
            let dim = !is_page && probe.filtering && matches == 0;
            let painter = ui.painter();
            if active {
                painter.rect_filled(r, 8., p.accent_surface);
            } else if response.hovered() {
                painter.rect_filled(r, 8., p.hover);
            }
            let color = if active {
                p.mark_fg
            } else if dim {
                p.muted.gamma_multiply(0.6)
            } else if response.hovered() {
                p.text
            } else {
                p.muted_alt
            };
            let icon_center = if compact {
                r.center()
            } else {
                Pos2::new(r.center().x, r.top() + 17.)
            };
            settings_icons::topic(
                painter,
                Rect::from_center_size(icon_center, Vec2::splat(18.)),
                name,
                color,
            );
            if !compact {
                let label =
                    painter.layout_no_wrap(short_name(name).to_string(), theme::sans(10.5), color);
                painter.with_clip_rect(r).galley(
                    Pos2::new(r.center().x - label.size().x / 2., r.top() + 31.),
                    label,
                    color,
                );
            }
            // A dot for topics with changed settings.
            if per_topic.get(i).copied().unwrap_or(0) > 0 {
                painter.circle_filled(Pos2::new(r.right() - 9., r.top() + 9.), 3., p.accent_bright);
            }
            if response.hovered() {
                ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
            }
            let tip = if is_page {
                "Color by, totals and the layer the slider shows".to_string()
            } else {
                section_summary(name, &config)
            };
            let response = response.on_hover_ui(|ui| {
                ui.label(egui::RichText::new(name).font(theme::semibold(13.)));
                ui.weak(tip);
            });
            if response.clicked() {
                actions.pick = Some(name);
            }
            probe.rail.push((name.to_string(), r));
        }

        // Apply what the frame asked for.
        self.editor.search = search;
        self.editor.changed_only = changed_only;
        if let Some(topic) = actions.pick {
            // A topic shows that topic: search and filter end.
            self.editor.search.clear();
            self.editor.changed_only = false;
            if preview_rail {
                self.preview_side.topic = (topic != PREVIEW_TOPIC).then(|| topic.to_string());
            } else if self.prefs.settings_topic != topic {
                self.prefs.settings_topic = topic.to_string();
                self.prefs_dirty = true;
            }
            self.editor.topic_picked = true;
        }
        for id in actions.fold {
            if let Some(i) = self.prefs.collapsed_sections.iter().position(|s| *s == id) {
                self.prefs.collapsed_sections.remove(i);
            } else {
                self.prefs.collapsed_sections.push(id);
            }
            self.prefs_dirty = true;
        }
        if let Some(name) = actions.more {
            if let Some(i) = self.prefs.expanded_sections.iter().position(|s| s == name) {
                self.prefs.expanded_sections.remove(i);
            } else {
                self.prefs.expanded_sections.push(name.to_string());
            }
            self.prefs_dirty = true;
        }
        for (target, e) in actions.edits {
            // At an object or part scope an edit is an override there (`apply_edit(None)`).
            self.apply_edit(scope.is_none().then_some(target), e);
        }
        self.editor.sidebar_probe = probe;
    }
}

/// Bookkeeping while the sections are drawn.
struct DrawState<'p> {
    visible_top: f32,
    /// The first section whose header scrolled away while its rows are still in view: its
    /// sticky rect, id and icon name.
    sticky: Option<(Rect, String, String)>,
    probe: &'p mut Probe,
    actions: &'p mut Actions,
}

/// The sections of the picked topic: folded ones show their header only.
fn draw_topic(ui: &mut egui::Ui, env: &Env<'_>, plan: &[PlanSection<'_>], st: &mut DrawState<'_>) {
    for section in plan {
        if section.rows.is_empty() {
            continue;
        }
        if section.group.name.is_empty() {
            // Essentials' curated rows: no header.
            ui.add_space(4.);
            draw_rows(ui, env, section, st);
            continue;
        }
        let id = section.id();
        let collapsed = env.app.prefs.collapsed_sections.contains(&id);
        let header = section_header(
            ui,
            &section.group.name,
            Some(collapsed),
            if collapsed { section.changed } else { 0 },
        );
        st.probe.headers.push(ProbeHeader {
            id: id.clone(),
            rect: header.rect,
            collapsed,
            changed: if collapsed { section.changed } else { 0 },
        });
        if header.clicked() {
            st.actions.fold.push(id.clone());
        }
        if collapsed {
            continue;
        }
        draw_rows(ui, env, section, st);
        ui.add_space(6.);
        stick(ui, st, header.rect, id, &section.group.name);
    }
}

/// Search / filter results: each topic with matches under a small heading, its sections open.
fn draw_results(
    ui: &mut egui::Ui,
    env: &Env<'_>,
    topics: &[TopicDef],
    plans: &[Vec<PlanSection<'_>>],
    st: &mut DrawState<'_>,
) {
    let p = theme::of(ui);
    for (topic, sections) in topics.iter().zip(plans) {
        if row_count(sections) == 0 {
            continue;
        }
        ui.add_space(6.);
        let (rect, response) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), TOPIC_H), Sense::click());
        let hot = response.hovered();
        let color = if hot { p.accent_hi } else { p.scope_text };
        let painter = ui.painter();
        settings_icons::topic(
            painter,
            Rect::from_center_size(
                Pos2::new(rect.left() + 16. + 8., rect.center().y),
                Vec2::splat(16.),
            ),
            topic.name,
            color,
        );
        painter.text(
            Pos2::new(rect.left() + 16. + 16. + 8., rect.center().y),
            Align2::LEFT_CENTER,
            widgets::spaced_upper(topic.name),
            theme::semibold(11.),
            color,
        );
        if hot {
            ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
        }
        if response.on_hover_text("Open this topic").clicked() {
            st.actions.pick = Some(topic.name);
        }
        for section in sections {
            if section.rows.is_empty() {
                continue;
            }
            let id = section.id();
            let name = if section.group.name.is_empty() {
                topic.name
            } else {
                &section.group.name
            };
            let header = section_header(ui, name, None, 0);
            st.probe.headers.push(ProbeHeader {
                id: id.clone(),
                rect: header.rect,
                collapsed: env.app.prefs.collapsed_sections.contains(&id),
                changed: 0,
            });
            draw_rows(ui, env, section, st);
            ui.add_space(6.);
            stick(ui, st, header.rect, id, name);
        }
    }
}

/// Notes the section as the sticky one when its header scrolled past the visible top while
/// its rows are still in view (the scroll area's clip top, in the rows' screen coordinates).
fn stick(ui: &egui::Ui, st: &mut DrawState<'_>, header: Rect, id: String, icon: &str) {
    let end_y = ui.cursor().top();
    if st.sticky.is_none() && header.top() < st.visible_top && end_y > st.visible_top + SECTION_H {
        st.sticky = Some((
            Rect::from_min_size(
                Pos2::new(header.left(), st.visible_top),
                Vec2::new(header.width(), SECTION_H),
            ),
            id,
            icon.to_string(),
        ));
    }
}

fn draw_rows(ui: &mut egui::Ui, env: &Env<'_>, section: &PlanSection<'_>, st: &mut DrawState<'_>) {
    for row in &section.rows {
        let r = row.def;
        let mut e = Vec::new();
        if row.own {
            let ctx = env.ctx(section.topic, r.target, row.force);
            if settings::is_long_text(r.key) {
                editor::long_text(ui, r.key, &ctx, &mut e);
            } else {
                editor::row(ui, r.key, Some(&r.label), &ctx, &mut e);
            }
            st.probe.rows.push(ProbeRow {
                topic: section.topic,
                section: section.group.id.clone(),
                key: r.key,
            });
        }
        if !row.deps.is_empty() {
            let dctx = env.dep_ctx(section.topic, r.target);
            for &key in &row.deps {
                if row.own {
                    editor::row_indented(ui, key, None, &dctx, &mut e, editor::DEPENDENT_INDENT);
                } else {
                    editor::row(ui, key, None, &dctx, &mut e);
                }
                st.probe.rows.push(ProbeRow {
                    topic: section.topic,
                    section: section.group.id.clone(),
                    key,
                });
            }
        }
        st.actions
            .edits
            .extend(e.into_iter().map(|e| (r.target, e)));
    }
}

/// A section header (32 px): line icon, bold title, a rule to the right. `collapsed` is
/// `None` where the section cannot fold (search results); a folded section with changed
/// settings says how many.
fn section_header(
    ui: &mut egui::Ui,
    name: &str,
    collapsed: Option<bool>,
    changed: usize,
) -> egui::Response {
    let sense = if collapsed.is_some() {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), SECTION_H), sense);
    let hovered = collapsed.is_some() && response.hovered();
    paint_section_header(ui, rect, name, collapsed, changed, hovered);
    if hovered {
        ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
    }
    response
}

pub(super) fn paint_section_header(
    ui: &egui::Ui,
    rect: Rect,
    name: &str,
    collapsed: Option<bool>,
    changed: usize,
    hovered: bool,
) {
    let p = theme::of(ui);
    let painter = ui.painter();
    let y = rect.center().y + 3.;
    let icon = Rect::from_center_size(Pos2::new(rect.left() + 16. + 8., y), Vec2::splat(16.));
    let title_color = if hovered { p.accent_hi } else { p.text };
    settings_icons::section(
        painter,
        icon,
        name,
        if hovered { p.accent_hi } else { p.muted_alt },
    );
    let title = painter.layout_no_wrap(name.to_string(), theme::semibold(13.), title_color);
    let title_x = icon.right() + 8.;
    let mut right = rect.right() - MARGIN;
    if let Some(folded) = collapsed {
        widgets::chevron(
            painter,
            Pos2::new(right - 5., y),
            !folded,
            if hovered { p.accent_hi } else { p.muted },
        );
        right -= 18.;
    }
    if changed > 0 {
        let hint =
            painter.layout_no_wrap(format!("{changed} changed"), theme::sans(11.), p.scope_text);
        right -= hint.size().x;
        painter.galley(Pos2::new(right, y - hint.size().y / 2.), hint, p.scope_text);
        right -= 8.;
    }
    let title_w = title.size().x.min((right - title_x - 8.).max(0.));
    painter
        .with_clip_rect(Rect::from_min_max(
            Pos2::new(title_x, rect.top()),
            Pos2::new(right, rect.bottom()),
        ))
        .galley(
            Pos2::new(title_x, y - title.size().y / 2.),
            title,
            title_color,
        );
    let rule_left = title_x + title_w + 10.;
    if rule_left + 8. < right {
        painter.line_segment(
            [Pos2::new(rule_left, y), Pos2::new(right, y)],
            Stroke::new(1., p.border_raised),
        );
    }
}

#[cfg(test)]
#[path = "topics_tests.rs"]
mod tests;
