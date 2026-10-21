// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The object tree at the top of the Prepare sidebar: `PLATE 1 · 2 OBJECTS` with "+ Add",
//! one row per object (filament swatch, name, chips with what its overrides change or a muted
//! "profile settings", badges, "+" / "−" instances), its instances, and under an expanded
//! object its parts in fixed kind groups (modifiers, negative volumes, support enforcers and
//! blockers, height ranges), each with a painted icon and a count, folding. User groups
//! ("Hull details") sit inside a kind: made from a part's menu (Group), renamed inline,
//! dissolved (Ungroup). Parts are dragged to reorder them inside their kind and into or out
//! of a group (`tree.rs` has the model). Every part row says what it changes.
//!
//! The list part scrolls inside a height the user drags (the handle under it), or folds to
//! its header, so the settings below keep their room. "+ Add" on a selected object opens the
//! intents (`intents.rs`); without a selection it offers Add model and Text. Both offer "Add
//! shape", the built-in models' gallery (`shapes.rs`).

use super::widgets::{self, Glyph};
use crate::app::App;
use crate::commands::{Command, MenuInfo};
use crate::intents::Intent;
use crate::scene::Sub;
use crate::theme;
use crate::tree::{self as model, Drop, Kind, Node};
use eframe::egui::{self, Align2, Color32, Pos2, Rect, Sense, Stroke, Vec2};
use std::collections::{HashMap, HashSet};

const HEADER_H: f32 = 36.;
const OBJECT_H: f32 = 32.;
const CHILD_H: f32 = 26.;
const INDENT: f32 = 18.;

/// What the tree remembers between frames (not saved).
#[derive(Default)]
pub struct TreeState {
    /// Objects (by id) the user expanded or folded; others follow the selection.
    pub open: HashMap<u64, bool>,
    /// Folded kind groups and user groups: `object id/kind` and `object id/kind/group`.
    pub folded: HashSet<String>,
    /// A user group being renamed: object index, kind, old name, the text typed.
    pub renaming: Option<(usize, Kind, String, String)>,
    /// The selection the tree last scrolled to.
    seen: Option<(Option<usize>, Option<Sub>)>,
    /// The list folded to its header.
    pub collapsed: bool,
    /// What the last frame drew (tests and screenshots).
    pub probe: Vec<ProbeRow>,
    /// The header's plate title (right-click: the plate menu), as last drawn.
    #[cfg_attr(not(test), allow(dead_code))]
    pub plate_title: Option<Rect>,
}

/// A drawn row: what it is and where.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(not(test), allow(dead_code))]
pub struct ProbeRow {
    /// `object`, `instance`, `kind`, `group`, `part`, `model part`, `tower`.
    pub what: &'static str,
    pub label: String,
    /// The muted or accent text on the right (a part's summary, a group's count).
    pub detail: String,
    pub rect: Rect,
    pub selected: bool,
}

/// What "+ Add" picked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AddPick {
    Intent(Intent),
    Recipe(usize),
    DeleteRecipe(usize),
    Model,
    /// The Text tool: a text on the selected object (else a text object on the bed).
    Text,
    /// An SVG file's shape on the selected object (a file dialog first).
    Svg,
    /// A built-in model from the "Add shape" gallery.
    Shape(
        mica_core::builtin_models::BuiltinModel,
        crate::builtin::AddAs,
    ),
}

/// A dragged part.
#[derive(Clone, Copy, Debug, PartialEq)]
struct PartDrag {
    object: usize,
    sub: Sub,
}

#[derive(Clone, Copy)]
enum Row {
    Object(usize),
    Instance(usize),
    Kind {
        i: usize,
        kind: Kind,
        count: usize,
        open: bool,
    },
    Group {
        i: usize,
        kind: Kind,
        /// Index into `names`.
        name: usize,
        count: usize,
        open: bool,
    },
    Part {
        i: usize,
        sub: Sub,
        depth: u8,
    },
    ModelPart {
        i: usize,
        part: usize,
    },
}

/// What a frame asks for, applied after drawing.
enum Act {
    Select(usize, bool),
    SelectSub(usize, Sub),
    SetTool(usize, usize),
    Command(usize, Command),
    RenameDone(usize, Option<String>),
    ToggleOpen(u64, bool),
    Fold(String),
    Group(usize, Sub),
    Ungroup(usize, Kind, String),
    StartGroupRename(usize, Kind, String),
    GroupRenameDone(Option<String>),
    Move(usize, Sub, Drop),
    Delete(usize, Sub),
    SaveRecipe(usize, Sub),
    Tower,
    TowerCommand(crate::tower::TowerCommand),
}

/// A kind's painted icon (14 px line art).
pub fn kind_icon(painter: &egui::Painter, rect: Rect, kind: Kind, color: Color32) {
    let s = rect.width().min(rect.height());
    let r = Rect::from_center_size(rect.center(), Vec2::splat(s * 0.72));
    let stroke = Stroke::new(1.5, color);
    let line = |a: Pos2, b: Pos2| painter.line_segment([a, b], stroke);
    match kind {
        Kind::Parts => {
            painter.rect_stroke(r, 2., stroke, egui::StrokeKind::Middle);
            painter.rect_filled(r.shrink(3.), 1., color);
        }
        Kind::Modifiers | Kind::Other => {
            super::sidebar::dashed_rect(painter, r, 2., stroke);
        }
        Kind::Negatives => {
            painter.rect_stroke(r, 3., stroke, egui::StrokeKind::Middle);
            line(
                Pos2::new(r.left() + 3., r.center().y),
                Pos2::new(r.right() - 3., r.center().y),
            );
        }
        Kind::Enforcers | Kind::Blockers => {
            for (k, h) in [0.55f32, 0.95, 0.4].iter().enumerate() {
                let x = r.left() + r.width() * (0.15 + 0.35 * k as f32);
                line(
                    Pos2::new(x, r.bottom()),
                    Pos2::new(x, r.bottom() - r.height() * h),
                );
            }
            if kind == Kind::Blockers {
                line(r.left_top(), r.right_bottom());
            }
        }
        Kind::Ranges => {
            line(
                Pos2::new(r.left(), r.top() + r.height() * 0.3),
                Pos2::new(r.right(), r.top() + r.height() * 0.3),
            );
            line(
                Pos2::new(r.left(), r.top() + r.height() * 0.7),
                Pos2::new(r.right(), r.top() + r.height() * 0.7),
            );
        }
    }
}

/// A folder (user groups).
fn folder_icon(painter: &egui::Painter, rect: Rect, color: Color32) {
    let r = Rect::from_center_size(
        rect.center(),
        Vec2::new(rect.width() * 0.8, rect.height() * 0.62),
    );
    let tab = r.left() + r.width() * 0.4;
    let pts = vec![
        Pos2::new(r.left(), r.top()),
        Pos2::new(tab, r.top()),
        Pos2::new(tab + 2., r.top() + 2.),
        Pos2::new(r.right(), r.top() + 2.),
        Pos2::new(r.right(), r.bottom()),
        Pos2::new(r.left(), r.bottom()),
        Pos2::new(r.left(), r.top()),
    ];
    painter.add(egui::Shape::line(pts, Stroke::new(1.5, color)));
}

/// An intent's icon in the "+ Add" menu.
pub fn intent_icon(painter: &egui::Painter, rect: Rect, intent: Intent, color: Color32) {
    let r = Rect::from_center_size(rect.center(), Vec2::splat(rect.width() * 0.75));
    let stroke = Stroke::new(1.6, color);
    let line = |a: Pos2, b: Pos2| painter.line_segment([a, b], stroke);
    match intent {
        Intent::Stronger => {
            painter.rect_stroke(r, 2., stroke, egui::StrokeKind::Middle);
            line(
                Pos2::new(r.left(), r.center().y),
                Pos2::new(r.right(), r.center().y),
            );
            line(
                Pos2::new(r.center().x, r.top()),
                Pos2::new(r.center().x, r.bottom()),
            );
        }
        Intent::Finer => {
            for k in 0..3 {
                let y = r.top() + r.height() * (0.2 + 0.3 * k as f32);
                line(Pos2::new(r.left(), y), Pos2::new(r.right(), y));
            }
        }
        Intent::Supports => kind_icon(painter, rect, Kind::Enforcers, color),
        Intent::NoSupports => kind_icon(painter, rect, Kind::Blockers, color),
        Intent::Smooth => {
            let pts: Vec<Pos2> = (0..=12)
                .map(|k| {
                    let t = k as f32 / 12.;
                    Pos2::new(
                        r.left() + r.width() * t,
                        r.top() + r.height() * (0.75 - 0.35 * (1. - (2. * t - 1.).powi(2))),
                    )
                })
                .collect();
            painter.add(egui::Shape::line(pts, stroke));
            line(
                Pos2::new(r.left(), r.top() + r.height() * 0.3),
                Pos2::new(r.right(), r.top() + r.height() * 0.3),
            );
        }
        Intent::Hole => kind_icon(painter, rect, Kind::Negatives, color),
        Intent::EmptyModifier => kind_icon(painter, rect, Kind::Modifiers, color),
    }
}

/// The "+ Add" menu's body. `object`: the selected object's name (intents), else the
/// add-a-model choices.
pub fn add_menu(
    ui: &mut egui::Ui,
    object: Option<&str>,
    recipes: &[crate::intents::Recipe],
) -> Option<AddPick> {
    let p = theme::of(ui);
    let mut out = None;
    let Some(name) = object else {
        ui.set_min_width(200.);
        if ui.button("Add model…").clicked() {
            out = Some(AddPick::Model);
            ui.close();
        }
        if ui.button("Text or SVG…").clicked() {
            out = Some(AddPick::Text);
            ui.close();
        }
        if let Some(pick) = shape_menu(ui, None) {
            out = Some(pick);
        }
        return out;
    };
    ui.set_min_width(270.);
    ui.set_max_width(270.);
    ui.label(
        egui::RichText::new(widgets::spaced_upper(&format!("Add to {name}")))
            .font(theme::semibold(11.))
            .color(p.muted),
    );
    ui.add_space(4.);
    for intent in Intent::MENU {
        let (r, resp) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 38.), Sense::click());
        if resp.hovered() {
            ui.painter().rect_filled(r, 6., p.accent_surface);
        }
        let color = if resp.hovered() {
            p.mark_fg
        } else {
            p.muted_alt
        };
        intent_icon(
            ui.painter(),
            Rect::from_center_size(Pos2::new(r.left() + 16., r.center().y), Vec2::splat(18.)),
            intent,
            color,
        );
        ui.painter().text(
            Pos2::new(r.left() + 34., r.top() + 11.),
            Align2::LEFT_CENTER,
            intent.label(),
            theme::sans(13.),
            p.text,
        );
        ui.painter().text(
            Pos2::new(r.left() + 34., r.top() + 27.),
            Align2::LEFT_CENTER,
            intent.hint(),
            theme::sans(11.),
            p.muted,
        );
        if resp.clicked() {
            out = Some(AddPick::Intent(intent));
            ui.close();
        }
    }
    ui.separator();
    if ui
        .button("Text")
        .on_hover_text("Text raised on the model; click the model to move it (T)")
        .clicked()
    {
        out = Some(AddPick::Text);
        ui.close();
    }
    if ui
        .button("SVG…")
        .on_hover_text("An SVG file's shapes raised on the model")
        .clicked()
    {
        out = Some(AddPick::Svg);
        ui.close();
    }
    ui.separator();
    if ui.button(Intent::EmptyModifier.label()).clicked() {
        out = Some(AddPick::Intent(Intent::EmptyModifier));
        ui.close();
    }
    ui.menu_button("Recipes", |ui| {
        ui.set_min_width(220.);
        if recipes.is_empty() {
            ui.weak("No recipes yet: save one from a part's settings");
        }
        for (k, r) in recipes.iter().enumerate() {
            ui.horizontal(|ui| {
                if ui
                    .button(&r.name)
                    .on_hover_text(format!(
                        "{} · {}",
                        r.kind,
                        model::summary(&r.overrides).join(" · ")
                    ))
                    .clicked()
                {
                    out = Some(AddPick::Recipe(k));
                    ui.close();
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if widgets::icon_button_sized(ui, Glyph::Close, "Delete this recipe", true, 20.)
                        .clicked()
                    {
                        out = Some(AddPick::DeleteRecipe(k));
                    }
                });
            });
        }
    });
    if let Some(pick) = shape_menu(ui, Some(name)) {
        out = Some(pick);
    }
    out
}

/// "Add shape ›": the built-in models' gallery (`panels::shapes`).
fn shape_menu(ui: &mut egui::Ui, object: Option<&str>) -> Option<AddPick> {
    let mut out = None;
    ui.menu_button("Add shape", |ui| {
        if let Some((m, add_as)) = super::shapes::gallery(ui, object, false) {
            out = Some(AddPick::Shape(m, add_as));
        }
    })
    .response
    .on_hover_text("Benchy, cube, cylinder, sphere and more, ready to print");
    out
}

fn fold_key(object: u64, kind: Kind, group: Option<&str>) -> String {
    match group {
        Some(g) => format!("{object}/{}/{g}", kind.id()),
        None => format!("{object}/{}", kind.id()),
    }
}

/// A short object name (the file stem).
pub fn short_name(name: &str) -> String {
    std::path::Path::new(name)
        .file_stem()
        .map_or(name.to_string(), |s| s.to_string_lossy().into_owned())
}

/// A rounded chip with `text` at `right` (its right edge); returns its left edge.
fn chip(painter: &egui::Painter, right: f32, y: f32, text: &str, p: &theme::Palette) -> f32 {
    let g = painter.layout_no_wrap(text.to_string(), theme::sans(11.), p.mark_fg);
    let r = Rect::from_min_max(
        Pos2::new(right - g.size().x - 14., y - 9.),
        Pos2::new(right, y + 9.),
    );
    painter.rect_filled(r, 9., p.accent_surface);
    painter.galley(Pos2::new(r.left() + 7., y - g.size().y / 2.), g, p.mark_fg);
    r.left()
}

impl App {
    /// The tree at the top of the sidebar: header, list (scrolling, resizable) and a rule.
    pub(crate) fn object_tree(&mut self, ui: &mut egui::Ui) {
        let p = theme::of(ui);
        let count = self.plates.current().objects.len();
        let collapsed = self.editor.tree.collapsed;
        let (h, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), HEADER_H), Sense::hover());
        let title = format!(
            "{} · {} object{}",
            self.plates.current().name,
            count,
            if count == 1 { "" } else { "s" }
        );
        ui.painter().text(
            Pos2::new(h.left() + 16., h.center().y + 1.),
            Align2::LEFT_CENTER,
            widgets::spaced_upper(&title),
            theme::semibold(11.),
            p.muted,
        );
        // Fold, then "+ Add", from the right.
        let fold = Rect::from_center_size(
            Pos2::new(h.right() - 16. - 11., h.center().y),
            Vec2::splat(22.),
        );
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(fold));
        if widgets::icon_button_sized(
            &mut child,
            if collapsed { Glyph::Down } else { Glyph::Up },
            if collapsed {
                "Show the object list"
            } else {
                "Fold the object list"
            },
            true,
            22.,
        )
        .clicked()
        {
            self.editor.tree.collapsed = !collapsed;
        }
        let add = Rect::from_min_max(
            Pos2::new(fold.left() - 8. - 56., h.center().y - 12.),
            Pos2::new(fold.left() - 8., h.center().y + 12.),
        );
        // The title names the plate: right-click for the plate menu, double-click to rename.
        let title_rect = Rect::from_min_max(h.min, Pos2::new(add.left() - 8., h.max.y));
        let title_resp = ui
            .interact(
                title_rect,
                ui.scope_id().with("plate title"),
                Sense::click(),
            )
            .on_hover_text("Right-click for the plate menu, double-click to rename the plate");
        let plate_info = crate::plate_menu::PlateMenuInfo {
            index: self.plates.current,
            count: self.plates.len(),
            name: self.plates.current().name.clone(),
            locked: self.plates.current().locked,
            objects: count,
            editable: self.loading.is_none() && !self.previewing(),
        };
        self.editor.tree.plate_title = Some(title_rect);
        let mut plate_pick = None;
        if title_resp.double_clicked() && plate_info.editable {
            plate_pick = Some(crate::plate_menu::PlateCommand::Rename);
        }
        title_resp.context_menu(|ui| {
            if let Some(c) = crate::plate_menu::plate_menu(ui, &plate_info) {
                plate_pick = Some(c);
            }
        });
        if let Some(c) = plate_pick {
            self.plate_command(self.plates.current, c);
        }
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(add)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        let add_resp = widgets::styled_button(
            &mut child,
            "+ Add",
            !self.plates.current().locked,
            false,
            24.,
        )
        .on_hover_text(if self.selected.is_some() {
            "Add to the selected object: stronger, finer, supports, a hole…"
        } else {
            "Add a model or a text"
        });
        let object_name = self
            .selected
            .and_then(|i| self.plates.current().objects.get(i))
            .map(|o| short_name(&o.name));
        let mut pick = None;
        let menu_id = egui::Id::unique("tree-add-menu");
        if std::mem::take(&mut self.editor.open_add_menu) {
            egui::Popup::open_id(ui.ctx(), menu_id);
        }
        egui::Popup::menu(&add_resp).id(menu_id).show(|ui| {
            pick = add_menu(ui, object_name.as_deref(), &self.recipes.list);
        });
        if let Some(pick) = pick {
            self.run_add_pick(pick, ui.ctx());
        }
        if !collapsed && (count > 0 || self.tower().is_some()) {
            let max_h = self.prefs.window.tree_height.clamp(80., 900.);
            egui::ScrollArea::vertical()
                .id_salt("object-tree-scroll")
                .max_height(max_h)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing = Vec2::ZERO;
                    self.tree_rows(ui);
                });
            // The handle: drag to give the list more or less room.
            let (handle, resp) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), 8.), Sense::drag());
            let hot = resp.hovered() || resp.dragged();
            ui.painter().line_segment(
                [
                    Pos2::new(handle.center().x - 16., handle.center().y),
                    Pos2::new(handle.center().x + 16., handle.center().y),
                ],
                Stroke::new(2., if hot { p.accent } else { p.border_raised }),
            );
            if hot {
                ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::ResizeVertical);
            }
            if resp.dragged() {
                let used = max_h.min(
                    self.editor
                        .tree
                        .probe
                        .last()
                        .map_or(max_h, |r| r.rect.bottom() - h.bottom()),
                );
                self.prefs.window.tree_height = (used + resp.drag_delta().y).clamp(80., 900.);
                self.prefs_dirty = true;
            }
        } else {
            ui.add_space(4.);
        }
        let (rule, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.), Sense::hover());
        ui.painter().line_segment(
            [rule.left_top(), rule.right_top()],
            Stroke::new(1., p.border),
        );
    }

    /// Carries out a "+ Add" choice.
    pub(crate) fn run_add_pick(&mut self, pick: AddPick, ctx: &egui::Context) {
        match pick {
            AddPick::Intent(i) => self.add_intent(i),
            AddPick::Recipe(k) => self.add_recipe(k),
            AddPick::DeleteRecipe(k) => self.delete_recipe(k),
            AddPick::Model => self.run_command(Command::AddModel, ctx),
            AddPick::Text => self.add_text(),
            AddPick::Svg => self.pick_svg_for_selection(),
            AddPick::Shape(m, add_as) => self.add_builtin(m, add_as),
        }
    }

    /// Whether object `id` is expanded (the user's choice, else while it is selected).
    fn tree_open(&self, i: usize, id: u64) -> bool {
        self.editor
            .tree
            .open
            .get(&id)
            .copied()
            .unwrap_or(self.selected == Some(i))
    }

    fn tree_rows(&mut self, ui: &mut egui::Ui) {
        let p = theme::of(ui);
        let width = ui.available_width();
        let tools = self.tool_count();
        // Mixed colors follow the physical tools (`mixes`).
        let colours = self.model_colours();
        let choices = self.tool_choices();
        let selected = self.selected;
        let selection = self.selection();
        let sub = self.sub;
        let toggle_held = ui.input(|i| i.modifiers.shift || i.modifiers.command);
        let plate_names: Vec<String> = self
            .plates
            .plates
            .iter()
            .enumerate()
            .map(|(k, pl)| pl.custom_name(k).unwrap_or_default())
            .collect();
        let current_plate = self.plates.current;
        let locked = self.plates.current().locked;
        let can_paste = !self.clipboard.is_empty();
        let mut renaming = self.renaming.take();
        let mut group_renaming = self.editor.tree.renaming.take();
        let scroll_to = {
            let now = (selected, sub);
            let changed = self.editor.tree.seen != Some(now);
            self.editor.tree.seen = Some(now);
            changed
        };
        let objects = &self.plates.current().objects;
        let move_inside: Vec<bool> = (0..objects.len())
            .map(|k| self.move_inside_shift(self.plates.current, k).is_some())
            .collect();
        let adhesion: Vec<(usize, String)> = self
            .adhesion
            .iter()
            .filter_map(|(k, o)| {
                let object = objects.get(*k)?;
                Some((
                    object.object_id,
                    mica_core::adhesion_check::explain(&object.name, o, true),
                ))
            })
            .collect();
        // The rows: objects (instances grouped under the first), then an expanded object's
        // kind groups, user groups and parts.
        let mut rows: Vec<Row> = vec![];
        let mut names: Vec<String> = vec![];
        let mut seen_ids: Vec<usize> = vec![];
        for (i, o) in objects.iter().enumerate() {
            if seen_ids.contains(&o.object_id) {
                continue;
            }
            seen_ids.push(o.object_id);
            rows.push(Row::Object(i));
            for (k, q) in objects.iter().enumerate() {
                if k != i && q.object_id == o.object_id {
                    rows.push(Row::Instance(k));
                }
            }
            // The selected instance's parts are the ones shown (they are shared anyway).
            let shown = selected
                .filter(|s| objects[*s].object_id == o.object_id)
                .unwrap_or(i);
            if !self.tree_open(i, o.id) {
                continue;
            }
            let so = &objects[shown];
            for g in model::groups(so) {
                let key = fold_key(o.id, g.kind, None);
                let open = !self.editor.tree.folded.contains(&key);
                let model_parts = if g.kind == Kind::Parts && so.parts.len() > 1 {
                    so.parts.len()
                } else {
                    0
                };
                rows.push(Row::Kind {
                    i: shown,
                    kind: g.kind,
                    count: g.count + model_parts,
                    open,
                });
                if !open {
                    continue;
                }
                for part in 0..model_parts {
                    rows.push(Row::ModelPart { i: shown, part });
                }
                for n in g.nodes {
                    match n {
                        Node::Part(s) => rows.push(Row::Part {
                            i: shown,
                            sub: s,
                            depth: 2,
                        }),
                        Node::Group { name, members } => {
                            let open = !self.editor.tree.folded.contains(&fold_key(
                                o.id,
                                g.kind,
                                Some(&name),
                            ));
                            names.push(name);
                            rows.push(Row::Group {
                                i: shown,
                                kind: g.kind,
                                name: names.len() - 1,
                                count: members.len(),
                                open,
                            });
                            if open {
                                rows.extend(members.into_iter().map(|s| Row::Part {
                                    i: shown,
                                    sub: s,
                                    depth: 3,
                                }));
                            }
                        }
                    }
                }
            }
            // A 3MF object's model parts without other volumes still show.
            if so.parts.len() > 1
                && !so
                    .volumes
                    .iter()
                    .any(|v| v.kind == mica_core::threemf::PartKind::Model)
            {
                let key = fold_key(o.id, Kind::Parts, None);
                let open = !self.editor.tree.folded.contains(&key);
                let at = rows
                    .iter()
                    .rposition(|r| matches!(r, Row::Object(k) if *k == i))
                    .unwrap_or(0);
                let instances = rows[at + 1..]
                    .iter()
                    .take_while(|r| matches!(r, Row::Instance(_)))
                    .count();
                let mut extra = vec![Row::Kind {
                    i: shown,
                    kind: Kind::Parts,
                    count: so.parts.len(),
                    open,
                }];
                if open {
                    extra.extend((0..so.parts.len()).map(|part| Row::ModelPart { i: shown, part }));
                }
                let pos = at + 1 + instances;
                rows.splice(pos..pos, extra);
            }
        }
        let mut acts: Vec<Act> = vec![];
        let mut probe: Vec<ProbeRow> = vec![];
        let to_colour = |c: [f32; 4]| {
            Color32::from_rgb(
                (c[0] * 255.) as u8,
                (c[1] * 255.) as u8,
                (c[2] * 255.) as u8,
            )
        };
        for row in &rows {
            let (i, height, depth) = match *row {
                Row::Object(i) => (i, OBJECT_H, 0u8),
                Row::Instance(i) => (i, CHILD_H, 1),
                Row::Kind { i, .. } => (i, CHILD_H, 1),
                Row::Group { i, .. } => (i, CHILD_H, 2),
                Row::Part { i, depth, .. } => (i, CHILD_H, depth),
                Row::ModelPart { i, .. } => (i, CHILD_H, 2),
            };
            let o = &objects[i];
            let sense = if matches!(row, Row::Part { .. }) {
                Sense::click_and_drag()
            } else {
                Sense::click()
            };
            let (rect, response) = ui.allocate_exact_size(Vec2::new(width, height), sense);
            let inner = Rect::from_min_max(
                Pos2::new(rect.left() + 8., rect.top() + 1.),
                Pos2::new(rect.right() - 8., rect.bottom() - 1.),
            );
            let is_selected = match *row {
                Row::Object(i) | Row::Instance(i) => {
                    selection.contains(&i) && (sub.is_none() || selected != Some(i))
                }
                Row::Part { i, sub: s, .. } => selected == Some(i) && sub == Some(s),
                _ => false,
            };
            if is_selected {
                ui.painter().rect_filled(inner, 6., p.selected_row);
                ui.painter().rect_filled(
                    Rect::from_min_max(
                        inner.left_top(),
                        Pos2::new(inner.left() + 3., inner.bottom()),
                    ),
                    2.,
                    p.accent,
                );
                if scroll_to {
                    ui.scroll_to_rect(rect, None);
                }
            } else if response.hovered() {
                ui.painter().rect_filled(inner, 6., p.hover);
            }
            let x0 = inner.left() + 8. + depth as f32 * INDENT;
            let cy = inner.center().y;
            let mut right = inner.right() - 8.;
            let label: String;
            let mut detail = String::new();
            let what: &'static str;
            match *row {
                Row::Object(_) | Row::Instance(_) => {
                    let object_row = matches!(row, Row::Object(_));
                    what = if object_row { "object" } else { "instance" };
                    let open = object_row && self.tree_open(i, o.id);
                    let has_parts =
                        !o.volumes.is_empty() || !o.height_ranges.is_empty() || o.parts.len() > 1;
                    if object_row && has_parts {
                        let chev = Rect::from_center_size(Pos2::new(x0 + 4., cy), Vec2::splat(16.));
                        let r = ui.interact(
                            chev,
                            ui.make_persistent_id(("tree-open", o.id)),
                            Sense::click(),
                        );
                        widgets::chevron(
                            ui.painter(),
                            chev.center(),
                            open,
                            if r.hovered() { p.text } else { p.muted },
                        );
                        if r.clicked() {
                            acts.push(Act::ToggleOpen(o.id, !open));
                        }
                    }
                    let tool = o.tools.0[0].unwrap_or(0);
                    let c = colours[tool.min(colours.len().saturating_sub(1))];
                    let sw = Rect::from_center_size(
                        Pos2::new(x0 + 20., cy),
                        Vec2::splat(if object_row { 10. } else { 8. }),
                    );
                    ui.painter()
                        .rect_filled(sw, 3., Color32::from_rgb(c[0], c[1], c[2]));
                    label = if object_row {
                        short_name(&o.name)
                    } else {
                        format!("Instance {}", o.instance_id + 1)
                    };
                    // Right side: tool chip, badges, instance buttons, override chips.
                    if tools > 1 {
                        let chip_r = Rect::from_min_size(
                            Pos2::new(right - 40., cy - 9.),
                            Vec2::new(40., 18.),
                        );
                        let r = ui.interact(
                            chip_r,
                            ui.make_persistent_id(("tree-tool", o.id)),
                            Sense::click(),
                        );
                        ui.painter().rect_filled(chip_r, 4., p.field);
                        ui.painter().rect_stroke(
                            chip_r,
                            4.,
                            Stroke::new(
                                1.,
                                if r.hovered() {
                                    p.accent
                                } else {
                                    p.field_border
                                },
                            ),
                            egui::StrokeKind::Inside,
                        );
                        ui.painter().text(
                            Pos2::new(chip_r.left() + 6., cy),
                            Align2::LEFT_CENTER,
                            choices
                                .get(tool)
                                .map_or(format!("T{}", tool + 1), |c| c.short.clone()),
                            theme::mono(11.),
                            p.text,
                        );
                        widgets::chevron(
                            ui.painter(),
                            Pos2::new(chip_r.right() - 8., cy),
                            true,
                            p.muted,
                        );
                        egui::Popup::menu(&r).show(|ui| {
                            for c in &choices {
                                let row = ui.horizontal(|ui| {
                                    super::swatch(ui, c.colour, 12.);
                                    ui.selectable_label(c.tool == tool, &c.label)
                                });
                                if row.inner.clicked() {
                                    acts.push(Act::SetTool(i, c.tool));
                                    ui.close();
                                }
                            }
                        });
                        right = chip_r.left() - 6.;
                    }
                    if object_row {
                        if let Some(report) = o.repair.as_ref() {
                            let r =
                                Rect::from_center_size(Pos2::new(right - 7., cy), Vec2::splat(14.));
                            crate::panels::mesh_tools::warning_glyph(ui.painter(), r, p.warn_text);
                            let lines = report.lines();
                            ui.interact(
                                r,
                                ui.make_persistent_id(("repair-badge", o.id)),
                                Sense::hover(),
                            )
                            .on_hover_text(format!(
                                "The mesh needed repair:\n{}",
                                if lines.is_empty() {
                                    "nothing left to fix".to_string()
                                } else {
                                    lines.join("\n")
                                }
                            ));
                            right -= 20.;
                        }
                        let warnings: Vec<&str> = adhesion
                            .iter()
                            .filter(|(id, _)| *id == o.object_id)
                            .map(|(_, t)| t.as_str())
                            .collect();
                        if !warnings.is_empty() {
                            let r =
                                Rect::from_center_size(Pos2::new(right - 7., cy), Vec2::splat(14.));
                            crate::panels::mesh_tools::warning_glyph(
                                ui.painter(),
                                r,
                                theme::WARNING,
                            );
                            ui.interact(
                                r,
                                ui.make_persistent_id(("adhesion-badge", o.id)),
                                Sense::hover(),
                            )
                            .on_hover_text(format!(
                                "{}\n\nRight-click the object for Move inside.",
                                warnings.join("\n")
                            ));
                            right -= 20.;
                        }
                        if !o.painting.is_empty() {
                            let r =
                                Rect::from_center_size(Pos2::new(right - 7., cy), Vec2::splat(14.));
                            widgets::glyph(ui.painter(), r, Glyph::Brush, p.accent_bright);
                            ui.interact(
                                r,
                                ui.make_persistent_id(("paint-badge", o.id)),
                                Sense::hover(),
                            )
                            .on_hover_text(format!(
                                "Painted: {}",
                                o.painting
                                    .kinds()
                                    .map(|k| k.label())
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            ));
                            right -= 20.;
                        }
                        if is_selected || response.hovered() || selected == Some(i) {
                            let instances = objects
                                .iter()
                                .filter(|q| q.object_id == o.object_id)
                                .count();
                            for (glyph, tip, cmd, enabled) in [
                                (
                                    Glyph::Plus,
                                    "Add an instance (+): shares the mesh and settings",
                                    Command::AddInstance,
                                    true,
                                ),
                                (
                                    Glyph::Minus,
                                    "Remove an instance (−)",
                                    Command::RemoveInstance,
                                    instances > 1,
                                ),
                            ] {
                                let b = Rect::from_center_size(
                                    Pos2::new(right - 10., cy),
                                    Vec2::splat(20.),
                                );
                                let mut child_ui = ui.new_child(egui::UiBuilder::new().max_rect(b));
                                if widgets::icon_button_sized(
                                    &mut child_ui,
                                    glyph,
                                    tip,
                                    enabled && !locked,
                                    20.,
                                )
                                .clicked()
                                {
                                    acts.push(Act::Command(i, cmd));
                                }
                                right -= 22.;
                            }
                        }
                        // What the object's own overrides change.
                        let s = model::summary(&o.overrides);
                        if s.is_empty() {
                            let g = ui.painter().layout_no_wrap(
                                "profile settings".into(),
                                theme::sans(11.),
                                p.muted,
                            );
                            let w = g.size().x;
                            if right - w > x0 + 90. {
                                ui.painter().galley(
                                    Pos2::new(right - w, cy - g.size().y / 2.),
                                    g,
                                    p.muted,
                                );
                                right -= w + 8.;
                            }
                            detail = "profile settings".into();
                        } else {
                            let more = s.len() - 1;
                            let mut left = right;
                            if more > 0 {
                                left = chip(ui.painter(), left, cy, &format!("+{more}"), p) - 4.;
                            }
                            let g = ui.painter().layout_no_wrap(
                                s[0].clone(),
                                theme::sans(11.),
                                p.mark_fg,
                            );
                            if left - g.size().x - 14. > x0 + 80. {
                                left = chip(ui.painter(), left, cy, &s[0], p) - 4.;
                            }
                            right = left - 4.;
                            detail = s.join(" · ");
                        }
                    }
                    if !is_selected && selected == Some(i) && sub.is_some() {
                        // The object of the selected part: a quieter mark.
                        ui.painter().rect_filled(
                            Rect::from_min_max(
                                inner.left_top(),
                                Pos2::new(inner.left() + 3., inner.bottom()),
                            ),
                            2.,
                            p.border_raised,
                        );
                    }
                    let name_left = x0 + 30.;
                    let renaming_this =
                        object_row && renaming.as_ref().is_some_and(|(ri, _)| *ri == i);
                    if renaming_this {
                        let text = &mut renaming.as_mut().expect("renaming").1;
                        let field = Rect::from_min_max(
                            Pos2::new(name_left - 4., inner.top() + 3.),
                            Pos2::new(right, inner.bottom() - 3.),
                        );
                        let id = ui.make_persistent_id(("rename", o.id));
                        let resp = ui.put(
                            field,
                            egui::TextEdit::singleline(text)
                                .id(id)
                                .font(theme::sans(13.))
                                .margin(egui::Margin::symmetric(4, 2)),
                        );
                        resp.request_focus();
                        let (enter, escape) = ui.input(|i| {
                            (
                                i.key_pressed(egui::Key::Enter),
                                i.key_pressed(egui::Key::Escape),
                            )
                        });
                        if escape {
                            acts.push(Act::RenameDone(i, None));
                        } else if enter || resp.lost_focus() {
                            acts.push(Act::RenameDone(i, Some(text.clone())));
                        }
                    } else {
                        text_clipped(
                            ui,
                            &label,
                            name_left,
                            right,
                            inner,
                            if object_row {
                                theme::medium(13.)
                            } else {
                                theme::sans(12.)
                            },
                            p.text,
                        );
                    }
                    if response.clicked() {
                        acts.push(Act::Select(i, toggle_held));
                    }
                    if response.double_clicked() && object_row {
                        acts.push(Act::Command(i, Command::Rename));
                    }
                    let info = MenuInfo {
                        move_inside: move_inside.get(i).copied().unwrap_or(false),
                        tools: choices.len(),
                        tool_labels: choices.iter().map(|c| c.label.clone()).collect(),
                        tool: o.tools.0[0].unwrap_or(0),
                        plates: plate_names.clone(),
                        current_plate,
                        instances: objects
                            .iter()
                            .filter(|q| q.object_id == o.object_id)
                            .count(),
                        can_paste,
                        selection: if selection.contains(&i) {
                            selection.len()
                        } else {
                            1
                        },
                        locked,
                        sub: None,
                        textures: self.prefs.features.textures,
                        text: o.emboss.is_some(),
                    };
                    response.context_menu(|ui| {
                        if let Some(c) = crate::commands::object_menu(ui, &info) {
                            acts.push(Act::Command(i, c));
                        }
                    });
                    if response.hovered() {
                        ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
                    }
                }
                Row::Kind {
                    kind, count, open, ..
                } => {
                    what = "kind";
                    widgets::chevron(ui.painter(), Pos2::new(x0 + 4., cy), open, p.muted);
                    kind_icon(
                        ui.painter(),
                        Rect::from_center_size(Pos2::new(x0 + 20., cy), Vec2::splat(14.)),
                        kind,
                        p.muted_alt,
                    );
                    label = format!("{} · {count}", kind.title());
                    if kind == Kind::Ranges {
                        let lo = o
                            .height_ranges
                            .iter()
                            .map(|r| r.min_z)
                            .fold(f32::INFINITY, f32::min);
                        let hi = o.height_ranges.iter().map(|r| r.max_z).fold(0f32, f32::max);
                        detail = format!("{lo:.0}–{hi:.0} mm");
                        let g =
                            ui.painter()
                                .layout_no_wrap(detail.clone(), theme::sans(11.), p.muted);
                        right -= g.size().x;
                        ui.painter()
                            .galley(Pos2::new(right, cy - g.size().y / 2.), g, p.muted);
                    }
                    text_clipped(
                        ui,
                        &widgets::spaced_upper(&label),
                        x0 + 32.,
                        right - 8.,
                        inner,
                        theme::semibold(10.5),
                        p.muted_alt,
                    );
                    if response.clicked() {
                        acts.push(Act::Fold(fold_key(o.id, kind, None)));
                    }
                    // Dropping a part on its kind takes it out of its group.
                    if let Some(d) = response.dnd_hover_payload::<PartDrag>()
                        && d.object == i
                        && model::kind_of(o, d.sub) == Some(kind)
                    {
                        ui.painter().rect_stroke(
                            inner,
                            6.,
                            Stroke::new(1., p.accent),
                            egui::StrokeKind::Inside,
                        );
                    }
                    if let Some(d) = response.dnd_release_payload::<PartDrag>()
                        && d.object == i
                    {
                        acts.push(Act::Move(i, d.sub, Drop::OutOfGroups));
                    }
                }
                Row::Group {
                    kind,
                    name,
                    count,
                    open,
                    ..
                } => {
                    what = "group";
                    let name = names[name].clone();
                    widgets::chevron(ui.painter(), Pos2::new(x0 + 4., cy), open, p.muted);
                    folder_icon(
                        ui.painter(),
                        Rect::from_center_size(Pos2::new(x0 + 20., cy), Vec2::splat(14.)),
                        p.muted_alt,
                    );
                    label = name.clone();
                    detail = format!("group · {count}");
                    let g = ui
                        .painter()
                        .layout_no_wrap(detail.clone(), theme::sans(11.), p.muted);
                    right -= g.size().x;
                    ui.painter()
                        .galley(Pos2::new(right, cy - g.size().y / 2.), g, p.muted);
                    let editing = group_renaming
                        .as_ref()
                        .is_some_and(|(gi, gk, old, _)| *gi == i && *gk == kind && *old == name);
                    if editing {
                        let text = &mut group_renaming.as_mut().expect("group rename").3;
                        let field = Rect::from_min_max(
                            Pos2::new(x0 + 28., inner.top() + 2.),
                            Pos2::new(right - 8., inner.bottom() - 2.),
                        );
                        let resp = ui.put(
                            field,
                            egui::TextEdit::singleline(text)
                                .id(ui.make_persistent_id(("group-rename", o.id, kind.id())))
                                .font(theme::sans(12.5))
                                .margin(egui::Margin::symmetric(4, 1)),
                        );
                        resp.request_focus();
                        let (enter, escape) = ui.input(|i| {
                            (
                                i.key_pressed(egui::Key::Enter),
                                i.key_pressed(egui::Key::Escape),
                            )
                        });
                        if escape {
                            acts.push(Act::GroupRenameDone(None));
                        } else if enter || resp.lost_focus() {
                            acts.push(Act::GroupRenameDone(Some(text.clone())));
                        }
                    } else {
                        text_clipped(
                            ui,
                            &label,
                            x0 + 32.,
                            right - 8.,
                            inner,
                            theme::sans(12.5),
                            p.label,
                        );
                    }
                    if response.clicked() {
                        acts.push(Act::Fold(fold_key(o.id, kind, Some(&name))));
                    }
                    if response.double_clicked() {
                        acts.push(Act::StartGroupRename(i, kind, name.clone()));
                    }
                    response.context_menu(|ui| {
                        if ui.button("Rename group").clicked() {
                            acts.push(Act::StartGroupRename(i, kind, name.clone()));
                            ui.close();
                        }
                        if ui.button("Ungroup").clicked() {
                            acts.push(Act::Ungroup(i, kind, name.clone()));
                            ui.close();
                        }
                    });
                    if let Some(d) = response.dnd_hover_payload::<PartDrag>()
                        && d.object == i
                        && model::kind_of(o, d.sub) == Some(kind)
                    {
                        ui.painter().rect_stroke(
                            inner,
                            6.,
                            Stroke::new(1., p.accent),
                            egui::StrokeKind::Inside,
                        );
                    }
                    if let Some(d) = response.dnd_release_payload::<PartDrag>()
                        && d.object == i
                    {
                        acts.push(Act::Move(i, d.sub, Drop::IntoGroup(name.clone())));
                    }
                }
                Row::Part { sub: s, .. } => {
                    what = "part";
                    label = model::part_name(o, s);
                    let (text, changes) = model::part_text(o, s);
                    detail = text.clone();
                    // The marker: the volume's colour (striped for negatives), a band for ranges.
                    let sw = Rect::from_center_size(Pos2::new(x0 + 4., cy), Vec2::splat(8.));
                    match s {
                        Sub::Volume(id) => {
                            if let Some(v) = o.volume(id) {
                                let (col, _) = crate::render::volume_color(&v.kind, false);
                                ui.painter().rect_filled(sw, 2., to_colour(col));
                                if v.kind == mica_core::threemf::PartKind::NegativeVolume {
                                    ui.painter().line_segment(
                                        [sw.left_bottom(), sw.right_top()],
                                        Stroke::new(1.5, p.panel),
                                    );
                                }
                            }
                        }
                        Sub::Range(_) => {
                            ui.painter().rect_filled(
                                Rect::from_center_size(sw.center(), Vec2::new(8., 4.)),
                                1.,
                                to_colour(crate::render::RANGE_COLOR),
                            );
                        }
                    }
                    let color = if changes { p.mark_fg } else { p.muted };
                    let g = ui.painter().layout_no_wrap(text, theme::sans(11.), color);
                    let w = g.size().x.min((right - x0 - 70.).max(0.));
                    ui.painter()
                        .with_clip_rect(Rect::from_min_max(
                            Pos2::new(right - w, inner.top()),
                            Pos2::new(right, inner.bottom()),
                        ))
                        .galley(Pos2::new(right - w, cy - g.size().y / 2.), g, color);
                    right -= w + 8.;
                    text_clipped(
                        ui,
                        &label,
                        x0 + 14.,
                        right,
                        inner,
                        theme::sans(12.5),
                        if is_selected { p.text } else { p.label },
                    );
                    if response.clicked() {
                        acts.push(Act::SelectSub(i, s));
                    }
                    if response.drag_started() || response.dragged() {
                        response.dnd_set_drag_payload(PartDrag { object: i, sub: s });
                    }
                    if let Some(d) = response.dnd_hover_payload::<PartDrag>()
                        && d.object == i
                        && d.sub != s
                        && model::kind_of(o, d.sub) == model::kind_of(o, s)
                        && let Some(pos) = ui.ctx().pointer_hover_pos()
                    {
                        let y = if pos.y > cy {
                            inner.bottom()
                        } else {
                            inner.top()
                        };
                        ui.painter().line_segment(
                            [Pos2::new(x0, y), Pos2::new(inner.right(), y)],
                            Stroke::new(2., p.accent),
                        );
                    }
                    if let Some(d) = response.dnd_release_payload::<PartDrag>()
                        && d.object == i
                        && d.sub != s
                    {
                        let after = ui
                            .ctx()
                            .pointer_interact_pos()
                            .is_some_and(|pos| pos.y > cy);
                        acts.push(Act::Move(i, d.sub, Drop::Beside { target: s, after }));
                    }
                    let kind = model::kind_of(o, s);
                    let groups: Vec<String> = kind
                        .map(|k| {
                            model::groups(o)
                                .into_iter()
                                .filter(|g| g.kind == k)
                                .flat_map(|g| g.nodes)
                                .filter_map(|n| match n {
                                    Node::Group { name, .. } => Some(name),
                                    _ => None,
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    let own_group = model::group_of(o, s).unwrap_or("").to_string();
                    let is_text = match s {
                        Sub::Volume(id) => o.volume(id).is_some_and(|v| v.emboss.is_some()),
                        Sub::Range(_) => false,
                    };
                    response.context_menu(|ui| {
                        ui.set_min_width(200.);
                        if is_text && ui.button("Edit text").clicked() {
                            acts.push(Act::SelectSub(i, s));
                            ui.close();
                        }
                        if ui.button("Settings").clicked() {
                            acts.push(Act::SelectSub(i, s));
                            ui.close();
                        }
                        ui.separator();
                        if ui
                            .button("Group")
                            .on_hover_text("Put this part into a new group of its own kind")
                            .clicked()
                        {
                            acts.push(Act::Group(i, s));
                            ui.close();
                        }
                        let others: Vec<&String> =
                            groups.iter().filter(|g| **g != own_group).collect();
                        ui.add_enabled_ui(!others.is_empty(), |ui| {
                            ui.menu_button("Move to group", |ui| {
                                for g in others {
                                    if ui.button(g).clicked() {
                                        acts.push(Act::Move(i, s, Drop::IntoGroup(g.clone())));
                                        ui.close();
                                    }
                                }
                            });
                        });
                        if !own_group.is_empty() && ui.button("Remove from group").clicked() {
                            acts.push(Act::Move(i, s, Drop::OutOfGroups));
                            ui.close();
                        }
                        ui.separator();
                        if ui
                            .button("Save recipe…")
                            .on_hover_text(
                                "Keep its kind, shape and settings for “+ Add › Recipes”",
                            )
                            .clicked()
                        {
                            acts.push(Act::SaveRecipe(i, s));
                            ui.close();
                        }
                        if ui
                            .add_enabled(!locked, egui::Button::new("Delete"))
                            .clicked()
                        {
                            acts.push(Act::Delete(i, s));
                            ui.close();
                        }
                    });
                    if response.hovered() {
                        ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
                    }
                }
                Row::ModelPart { part, .. } => {
                    what = "model part";
                    let (name, _) = &o.parts[part];
                    label = name.clone();
                    detail = "Part".into();
                    let sw = Rect::from_center_size(Pos2::new(x0 + 4., cy), Vec2::splat(8.));
                    ui.painter().rect_filled(sw, 2., p.muted_alt);
                    let g = ui
                        .painter()
                        .layout_no_wrap("Part".into(), theme::sans(11.), p.muted);
                    right -= g.size().x;
                    ui.painter()
                        .galley(Pos2::new(right, cy - g.size().y / 2.), g, p.muted);
                    text_clipped(
                        ui,
                        &label,
                        x0 + 14.,
                        right - 8.,
                        inner,
                        theme::sans(12.5),
                        p.label,
                    );
                    if response.clicked() {
                        acts.push(Act::Select(i, false));
                    }
                }
            }
            probe.push(ProbeRow {
                what,
                label,
                detail,
                rect,
                selected: is_selected,
            });
        }
        self.renaming = renaming;
        self.editor.tree.renaming = group_renaming;
        // The plate's prime tower: not an object, selected like one.
        if let Some(t) = self.tower() {
            let is_selected = self.tower_selected && selected.is_none();
            let (rect, response) =
                ui.allocate_exact_size(Vec2::new(width, OBJECT_H), Sense::click());
            let inner = Rect::from_min_max(
                Pos2::new(rect.left() + 8., rect.top() + 1.),
                Pos2::new(rect.right() - 8., rect.bottom() - 1.),
            );
            if is_selected {
                ui.painter().rect_filled(inner, 6., p.selected_row);
            } else if response.hovered() {
                ui.painter().rect_filled(inner, 6., p.hover);
            }
            let icon = Rect::from_center_size(
                Pos2::new(inner.left() + 28., inner.center().y),
                Vec2::splat(14.),
            );
            let tint = if t.problems.is_empty() {
                p.label
            } else {
                theme::WARNING
            };
            widgets::glyph(ui.painter(), icon, Glyph::Tower, tint);
            let s = &t.shape;
            let right_text = format!(
                "{:.0} × {:.0} mm{}",
                s.width,
                s.depth,
                if s.exact { "" } else { " · est." }
            );
            let g = ui
                .painter()
                .layout_no_wrap(right_text, theme::sans(11.), p.muted);
            ui.painter().galley(
                Pos2::new(
                    inner.right() - 8. - g.size().x,
                    inner.center().y - g.size().y / 2.,
                ),
                g,
                p.muted,
            );
            let name = t.label();
            ui.painter().text(
                Pos2::new(icon.right() + 8., inner.center().y),
                Align2::LEFT_CENTER,
                name,
                theme::sans(13.),
                p.text,
            );
            response.clone().on_hover_text(if t.problems.is_empty() {
                format!("{name}: drag it on the bed or edit X / Y in its card. Kept per plate.")
            } else {
                format!("{name}: {}", t.problems[0].text().to_lowercase())
            });
            if response.clicked() {
                acts.push(Act::Tower);
            }
            response.context_menu(|ui| {
                if let Some(c) = crate::viewport::tower_menu_ui(ui, name, !locked) {
                    acts.push(Act::TowerCommand(c));
                }
            });
            probe.push(ProbeRow {
                what: "tower",
                label: name.to_string(),
                detail: String::new(),
                rect,
                selected: is_selected,
            });
        }
        self.editor.tree.probe = probe;
        self.run_tree_acts(acts, ui.ctx());
    }

    fn run_tree_acts(&mut self, acts: Vec<Act>, ctx: &egui::Context) {
        for act in acts {
            match act {
                Act::Select(i, toggle) => self.select(Some(i), toggle),
                Act::SelectSub(i, s) => {
                    if self.selected != Some(i) {
                        self.select(Some(i), false);
                    }
                    self.sub = Some(s);
                    self.editor.object_mode = true;
                    // A text part opens the Text tool on it (its settings stay in the
                    // sidebar's scope bar).
                    self.edit_text(i, Some(s));
                }
                Act::SetTool(i, t) => {
                    self.record("Change tool");
                    self.plates.current_mut().objects[i].set_tool(t);
                    self.sync_instances(i);
                    self.changed();
                }
                Act::Command(i, cmd) => {
                    if !self.selection().contains(&i) {
                        self.select(Some(i), false);
                    }
                    self.run_command(cmd, ctx);
                }
                Act::RenameDone(i, result) => {
                    self.renaming = None;
                    if let Some(name) = result
                        && let Some(o) = self.plates.current().objects.get(i)
                        && !name.trim().is_empty()
                        && name.trim() != o.name
                    {
                        self.record("Rename");
                        let name = name.trim().to_string();
                        for k in self.instances_of(i) {
                            self.plates.current_mut().objects[k].name = name.clone();
                        }
                        self.changed();
                    }
                }
                Act::ToggleOpen(id, open) => {
                    self.editor.tree.open.insert(id, open);
                }
                Act::Fold(key) => {
                    if !self.editor.tree.folded.remove(&key) {
                        self.editor.tree.folded.insert(key);
                    }
                }
                Act::Group(i, s) => {
                    let Some(o) = self.plates.current().objects.get(i) else {
                        continue;
                    };
                    let Some(kind) = model::kind_of(o, s) else {
                        continue;
                    };
                    let name = model::new_group_name(o, kind);
                    self.tree_edit(i, "Group", |o| model::group(o, &[s], &name));
                    self.editor.tree.renaming = Some((i, kind, name.clone(), name));
                }
                Act::Ungroup(i, kind, name) => {
                    self.tree_edit(i, "Ungroup", |o| model::ungroup(o, kind, &name));
                }
                Act::StartGroupRename(i, kind, name) => {
                    self.editor.tree.renaming = Some((i, kind, name.clone(), name));
                }
                Act::GroupRenameDone(new) => {
                    if let Some((i, kind, old, _)) = self.editor.tree.renaming.take()
                        && let Some(new) = new
                        && !model::clean_name(&new).is_empty()
                    {
                        self.tree_edit(i, "Rename group", |o| {
                            model::rename_group(o, kind, &old, &new)
                        });
                    }
                }
                Act::Move(i, s, drop) => {
                    self.tree_edit(i, "Move part", |o| model::move_part(o, s, &drop));
                }
                Act::Delete(i, s) => {
                    if self.selected != Some(i) {
                        self.select(Some(i), false);
                    }
                    self.sub = Some(s);
                    self.delete_sub();
                }
                Act::SaveRecipe(i, s) => {
                    if self.selected != Some(i) {
                        self.select(Some(i), false);
                    }
                    self.sub = Some(s);
                    self.editor.object_mode = true;
                    let name = self
                        .plates
                        .current()
                        .objects
                        .get(i)
                        .map(|o| model::part_name(o, s))
                        .unwrap_or_default();
                    self.editor.recipe_name = Some(name);
                }
                Act::Tower => self.select_tower(),
                Act::TowerCommand(c) => {
                    self.select_tower();
                    self.run_tower_command(c);
                }
            }
        }
    }

    /// One tree edit of object `i` as one undo step (nothing recorded when nothing changes);
    /// instances follow.
    pub(crate) fn tree_edit(
        &mut self,
        i: usize,
        label: &str,
        edit: impl FnOnce(&mut crate::scene::Object) -> bool,
    ) {
        if self.plate_locked() {
            return;
        }
        let Some(mut o) = self.plates.current().objects.get(i).cloned() else {
            return;
        };
        if !edit(&mut o) {
            return;
        }
        self.record(label);
        self.plates.current_mut().objects[i] = o;
        self.sync_instances(i);
        self.changed();
    }

    /// The breadcrumb chip over the 3D view while something is selected:
    /// `3DBenchy › Hull infill`; a click on the object's name goes up to it.
    pub(crate) fn breadcrumb(&mut self, ctx: &egui::Context) {
        let Some(o) = self
            .selected
            .and_then(|i| self.plates.current().objects.get(i))
        else {
            return;
        };
        let object = short_name(&o.name);
        let part = self.sub.map(|s| model::part_name(o, s));
        let many = self.selection().len();
        let mut up = false;
        egui::Area::new(egui::Id::unique("selection-breadcrumb"))
            .order(egui::Order::Middle)
            .fixed_pos(self.viewport_rect.left_top() + egui::vec2(14., 14.))
            .show(ctx, |ui| {
                let p = theme::of(ui);
                egui::Frame::new()
                    .fill(p.raised)
                    .stroke(Stroke::new(1., p.border))
                    .corner_radius(999.)
                    .inner_margin(egui::Margin::symmetric(12, 6))
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.x = 6.;
                        ui.horizontal(|ui| {
                            let color = if part.is_some() { p.label } else { p.text };
                            let r = ui.add(
                                egui::Label::new(
                                    egui::RichText::new(&object)
                                        .font(theme::sans(12.))
                                        .color(color),
                                )
                                .sense(Sense::click()),
                            );
                            if part.is_some() && r.on_hover_text("Select the object").clicked() {
                                up = true;
                            }
                            if let Some(part) = &part {
                                ui.label(
                                    egui::RichText::new("›")
                                        .font(theme::sans(12.))
                                        .color(p.muted),
                                );
                                ui.label(
                                    egui::RichText::new(part)
                                        .font(theme::sans(12.))
                                        .color(p.text),
                                );
                            }
                            if many > 1 {
                                ui.label(
                                    egui::RichText::new(format!("+{} more", many - 1))
                                        .font(theme::sans(12.))
                                        .color(p.muted),
                                );
                            }
                        });
                    });
            });
        if up {
            self.sub = None;
        }
    }
}

/// `text` from `left`, clipped at `right`, centred in `row`.
fn text_clipped(
    ui: &egui::Ui,
    text: &str,
    left: f32,
    right: f32,
    row: Rect,
    font: egui::FontId,
    color: Color32,
) {
    if right <= left {
        return;
    }
    let g = ui.painter().layout_no_wrap(text.to_string(), font, color);
    ui.painter()
        .with_clip_rect(Rect::from_min_max(
            Pos2::new(left, row.top()),
            Pos2::new(right, row.bottom()),
        ))
        .galley(Pos2::new(left, row.center().y - g.size().y / 2.), g, color);
}
