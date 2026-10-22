// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: orca@8500fcdcca src/slic3r/GUI/GLCanvas3D.cpp :: GLCanvas3D::_init_main_toolbar
// Source: orca@8500fcdcca src/slic3r/GUI/Gizmos/GLGizmosManager.cpp :: GLGizmosManager::render_overlay
// Changes: Only the arrangement is taken (a top-centre bar of icon buttons in groups split by
// separators, the less-used tools collapsed when the window is narrow); the groups, the
// fly-out menus, the fitting between Mica's floating cards and all egui code are original.

//! The Prepare toolbar (top centre of the 3D view): add and arrange, the transform tools, the
//! object tools, painting, and the view tools, in groups with separators. Less-used tools sit
//! in fly-outs (the mesh tools, the four painting tools) and, when the view is narrow, in a
//! "more" menu; [`LAYOUTS`] runs from the widest to the narrowest arrangement and the widest
//! that fits between the floating cards (object list, tool card) is drawn. A fly-out's button
//! shows the active tool's icon, highlighted, while one of its tools is open. Every tool keeps
//! its keyboard shortcut (`keymap.rs`), which its tooltip and menu row show.

use super::icons::{self, Icon, Metrics};
use super::{Action, ObjectTool, Scene, Viewport, gizmo};
use crate::commands::Command;
use crate::theme;
use eframe::egui::{self, Align2, Pos2, Rect, Vec2};
use mica_core::painting::PaintKind;

/// One place in the toolbar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Slot {
    Tool(Icon),
    /// A fly-out: the icon it shows while none of its tools is open, and its tools in
    /// sections (separated in the menu).
    Group(Icon, &'static [&'static [Icon]]),
    Sep,
}
use Slot::{Group, Sep, Tool as T};

const MESH: &[&[Icon]] = &[&[Icon::Boolean, Icon::Simplify, Icon::Repair]];
const PAINT: &[&[Icon]] = &[&[
    Icon::PaintSupport,
    Icon::PaintSeam,
    Icon::PaintColour,
    Icon::PaintFuzzy,
    Icon::PaintTexture,
]];

/// The toolbar from the widest to the narrowest arrangement.
const LAYOUTS: [&[Slot]; 5] = [
    // Everything in sight but the mesh and painting fly-outs.
    &[
        T(Icon::Add),
        T(Icon::Arrange),
        Sep,
        T(Icon::Move),
        T(Icon::Rotate),
        T(Icon::Scale),
        T(Icon::LayOnFace),
        Sep,
        T(Icon::Cut),
        Group(Icon::Mesh, MESH),
        T(Icon::Text),
        Sep,
        Group(Icon::Paint, PAINT),
        T(Icon::LayerHeights),
        T(Icon::BrimEars),
        Sep,
        T(Icon::Measure),
        T(Icon::Section),
    ],
    // The per-object tools and the view tools fold into "more".
    &[
        T(Icon::Add),
        T(Icon::Arrange),
        Sep,
        T(Icon::Move),
        T(Icon::Rotate),
        T(Icon::Scale),
        T(Icon::LayOnFace),
        Sep,
        T(Icon::Cut),
        Group(Icon::Mesh, MESH),
        T(Icon::Text),
        Group(Icon::Paint, PAINT),
        Sep,
        Group(
            Icon::More,
            &[
                &[Icon::LayerHeights, Icon::BrimEars],
                &[Icon::Measure, Icon::Section],
            ],
        ),
    ],
    // Lay on face and Text join them.
    &[
        T(Icon::Add),
        T(Icon::Arrange),
        Sep,
        T(Icon::Move),
        T(Icon::Rotate),
        T(Icon::Scale),
        Sep,
        T(Icon::Cut),
        Group(Icon::Mesh, MESH),
        Group(Icon::Paint, PAINT),
        Sep,
        Group(
            Icon::More,
            &[
                &[Icon::LayOnFace, Icon::Text],
                &[Icon::LayerHeights, Icon::BrimEars],
                &[Icon::Measure, Icon::Section],
            ],
        ),
    ],
    // Add and the gizmos; everything else in "more".
    &[
        T(Icon::Add),
        Sep,
        T(Icon::Move),
        T(Icon::Rotate),
        T(Icon::Scale),
        Sep,
        Group(Icon::More, MORE_ALL),
    ],
    // One button.
    &[Group(
        Icon::More,
        &[
            &[Icon::Add, Icon::Arrange],
            &[Icon::Move, Icon::Rotate, Icon::Scale],
            &[Icon::LayOnFace, Icon::Cut, Icon::Text],
            &[Icon::Boolean, Icon::Simplify, Icon::Repair],
            &[
                Icon::PaintSupport,
                Icon::PaintSeam,
                Icon::PaintColour,
                Icon::PaintFuzzy,
                Icon::PaintTexture,
            ],
            &[Icon::LayerHeights, Icon::BrimEars],
            &[Icon::Measure, Icon::Section],
        ],
    )],
];
const MORE_ALL: &[&[Icon]] = &[
    &[Icon::Arrange],
    &[Icon::LayOnFace, Icon::Cut, Icon::Text],
    &[Icon::Boolean, Icon::Simplify, Icon::Repair],
    &[
        Icon::PaintSupport,
        Icon::PaintSeam,
        Icon::PaintColour,
        Icon::PaintFuzzy,
        Icon::PaintTexture,
    ],
    &[Icon::LayerHeights, Icon::BrimEars],
    &[Icon::Measure, Icon::Section],
];

/// Distance from the view's edges and between the toolbar and a card.
const EDGE: f32 = 14.;
const CLEAR: f32 = 10.;
/// What the object list and the tool card take from the top edge (their usual widths), kept
/// clear whenever the plate has objects so the toolbar does not change as the selection does.
const LIST_RESERVE: f32 = 240.;
const CARD_RESERVE: f32 = 300.;

/// The toolbar's outer width for `slots` at the size `m`.
fn width(slots: &[Slot], m: &Metrics) -> f32 {
    let items: f32 = slots
        .iter()
        .map(|s| if *s == Sep { m.sep } else { m.button })
        .sum();
    items + m.gap * (slots.len().saturating_sub(1)) as f32 + 2. * m.margin + 2.
}

/// The widest layout that fits in one of the `free` spans at the size `m`, and its centre
/// x: as close to `centre` as the span allows. `None`: not even one button fits.
fn fit(free: &[(f32, f32)], centre: f32, m: &Metrics) -> Option<(usize, f32)> {
    for (level, slots) in LAYOUTS.iter().enumerate() {
        let w = width(slots, m);
        let best = free
            .iter()
            .filter(|(lo, hi)| hi - lo >= w)
            .map(|&(lo, hi)| centre.clamp(lo + w / 2., hi - w / 2.))
            .min_by(|a, b| (a - centre).abs().total_cmp(&(b - centre).abs()));
        if let Some(x) = best {
            return Some((level, x));
        }
    }
    None
}

/// `[lo, hi]` with every `blocked` span cut out.
fn free_spans(lo: f32, hi: f32, blocked: &[(f32, f32)]) -> Vec<(f32, f32)> {
    let mut spans = vec![(lo, hi)];
    for &(bl, bh) in blocked {
        spans = spans
            .into_iter()
            .flat_map(|(l, h)| {
                if bh <= l || bl >= h {
                    vec![(l, h)]
                } else {
                    [(l, bl), (bh, h)]
                        .into_iter()
                        .filter(|(a, b)| b > a)
                        .collect()
                }
            })
            .collect();
    }
    spans
}

impl Viewport {
    /// Whether the toolbar button (or its shortcut) for `icon` can be used, else why not, for
    /// its tooltip (empty while slicing or in Preview, where the whole bar is off). A tool
    /// for the selected object needs one: with no model on the plate it is off ("Add a model
    /// first"); with several and none selected it asks for a selection; with exactly one it
    /// picks that one when pressed (`press`).
    pub(super) fn tool_state(icon: Icon, scene: &Scene<'_>) -> Result<(), &'static str> {
        availability(
            icon,
            &Plate {
                editing: scene.preview.is_none() && scene.editable,
                any_object: !scene.objects.is_empty(),
                plate_objects: scene.plate_objects,
                selected: scene.selected.is_some(),
                only_object: scene.only_object.is_some(),
                tools: scene.tool_colors.len(),
                textures: scene.textures,
            },
        )
    }

    /// Whether `icon`'s tool is open (its button highlighted).
    fn tool_active(&self, icon: Icon, scene: &Scene<'_>) -> bool {
        let editing = scene.preview.is_none() && scene.editable;
        let gizmo = |mode: gizmo::Mode| {
            editing
                && scene.selected.is_some()
                && !self.lay_on_face
                && self.paint.is_none()
                && self.object_tool.is_none()
                && self.mode == mode
        };
        match icon {
            Icon::Move => gizmo(gizmo::Mode::Move),
            Icon::Rotate => gizmo(gizmo::Mode::Rotate),
            Icon::Scale => gizmo(gizmo::Mode::Scale),
            Icon::LayOnFace => editing && self.lay_on_face,
            Icon::Cut => scene.cut.is_some(),
            Icon::Boolean | Icon::Simplify => scene.mesh_tool == Some(icon),
            Icon::Text => self.emboss,
            Icon::Measure => editing && self.measure_on,
            Icon::Section => self.section.active(false),
            Icon::LayerHeights => self.object_tool == Some(ObjectTool::LayerHeights),
            Icon::BrimEars => self.object_tool == Some(ObjectTool::BrimEars),
            _ => paint_kind(icon).is_some_and(|k| self.paint == Some(k)),
        }
    }

    /// What pressing `icon`'s button (or picking it from a fly-out) does.
    fn press(&mut self, icon: Icon, scene: &Scene<'_>, actions: &mut Vec<Action>) {
        // A tool for the selected object with none selected works on the plate's only one.
        if needs_object(icon)
            && scene.selected.is_none()
            && let Some(only) = scene.only_object
        {
            actions.push(Action::Select {
                index: Some(only),
                toggle: false,
            });
        }
        let mode = match icon {
            Icon::Move => Some(gizmo::Mode::Move),
            Icon::Rotate => Some(gizmo::Mode::Rotate),
            Icon::Scale => Some(gizmo::Mode::Scale),
            _ => None,
        };
        if let Some(mode) = mode {
            self.mode = mode;
            self.lay_on_face = false;
            self.measure_on = false;
            if self.paint.take().is_some() {
                actions.push(Action::PaintTool(None));
            }
            if self.object_tool.take().is_some() {
                actions.push(Action::ObjectTool(None));
            }
            return;
        }
        if let Some(kind) = paint_kind(icon) {
            self.paint = if self.paint == Some(kind) {
                None
            } else {
                Some(kind)
            };
            self.lay_on_face = false;
            self.measure_on = false;
            if self.object_tool.take().is_some() {
                actions.push(Action::ObjectTool(None));
            }
            actions.push(Action::PaintTool(self.paint));
            return;
        }
        match icon {
            Icon::Add => actions.push(Action::Open),
            Icon::Arrange => actions.push(Action::Arrange),
            Icon::LayOnFace => {
                self.lay_on_face = !self.lay_on_face;
                self.measure_on = false;
                if self.paint.take().is_some() {
                    actions.push(Action::PaintTool(None));
                }
                if self.object_tool.take().is_some() {
                    actions.push(Action::ObjectTool(None));
                }
            }
            Icon::Cut => actions.push(Action::Cut),
            Icon::Measure => actions.extend(self.toggle_measure()),
            Icon::Section => self.section.toggle(false, self.camera.direction()),
            Icon::Boolean | Icon::Simplify | Icon::Repair => {
                let command = match icon {
                    Icon::Boolean => Command::MeshBoolean,
                    Icon::Simplify => Command::Simplify,
                    _ => Command::Repair,
                };
                actions.push(Action::Command {
                    index: scene.selected.or(scene.only_object),
                    command,
                });
            }
            Icon::Text => {
                self.lay_on_face = false;
                if self.paint.take().is_some() {
                    actions.push(Action::PaintTool(None));
                }
                actions.push(Action::Emboss);
            }
            Icon::LayerHeights | Icon::BrimEars => {
                let tool = if icon == Icon::LayerHeights {
                    ObjectTool::LayerHeights
                } else {
                    ObjectTool::BrimEars
                };
                self.object_tool = if self.object_tool == Some(tool) {
                    None
                } else {
                    Some(tool)
                };
                self.lay_on_face = false;
                self.measure_on = false;
                if self.paint.take().is_some() {
                    actions.push(Action::PaintTool(None));
                }
                actions.push(Action::ObjectTool(self.object_tool));
            }
            _ => {}
        }
    }

    /// Which layout to draw and where: the widest that fits between the floating cards at
    /// the top of the view (last frame's, plus room for the list and tool card whenever the
    /// plate has objects), as close to the centre as it can be.
    fn toolbar_layout(
        &self,
        ui: &egui::Ui,
        rect: Rect,
        scene: &Scene<'_>,
    ) -> (usize, Pos2, Align2) {
        let own = ui.scope_id().with("viewport toolbar");
        let top = rect.top() + EDGE;
        let mut blocked: Vec<(f32, f32)> = ui.ctx().memory(|m| {
            m.areas()
                .visible_layer_ids()
                .into_iter()
                .filter(|l| l.order == egui::Order::Middle && l.id != own)
                .filter_map(|l| m.area_rect(l.id))
                // The cards hang from the top edge, as the toolbar does.
                .filter(|r| (r.top() - top).abs() < 1. && r.width() > 0.)
                .map(|r| (r.left() - CLEAR, r.right() + CLEAR))
                .collect()
        });
        if !scene.objects.is_empty() {
            blocked.push((f32::MIN, rect.left() + EDGE + LIST_RESERVE + CLEAR));
            blocked.push((rect.right() - EDGE - CARD_RESERVE - CLEAR, f32::MAX));
        }
        let free = free_spans(rect.left() + EDGE, rect.right() - EDGE, &blocked);
        match fit(&free, rect.center().x, &icons::metrics(ui.ctx())) {
            Some((level, x)) => (level, Pos2::new(x, top), Align2::CENTER_TOP),
            // Too narrow even for one button (the cards meet): the single button in the
            // bottom-left corner, above the axis triad, where nothing floats.
            None => (
                LAYOUTS.len() - 1,
                Pos2::new(rect.left() + EDGE, rect.bottom() - 100.),
                Align2::LEFT_BOTTOM,
            ),
        }
    }

    pub(super) fn toolbar(
        &mut self,
        ui: &mut egui::Ui,
        rect: Rect,
        scene: &Scene<'_>,
        actions: &mut Vec<Action>,
    ) {
        let (level, pos, pivot) = self.toolbar_layout(ui, rect, scene);
        let slots = LAYOUTS[level];
        let mut pressed = None;
        let bar = egui::Area::new(ui.scope_id().with("viewport toolbar"))
            .order(egui::Order::Middle)
            .fixed_pos(pos)
            .pivot(pivot)
            .show(ui.ctx(), |ui| {
                let p = theme::of(ui);
                let m = icons::metrics(ui.ctx());
                theme::overlay_frame(ui)
                    .inner_margin(egui::Margin::same(m.margin as i8))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = m.gap;
                            for slot in slots {
                                match *slot {
                                    Sep => {
                                        let (sep, _) = ui.allocate_exact_size(
                                            Vec2::new(m.sep, m.button),
                                            egui::Sense::hover(),
                                        );
                                        let inset = m.button * 0.18;
                                        ui.painter().line_segment(
                                            [
                                                Pos2::new(sep.center().x, sep.top() + inset),
                                                Pos2::new(sep.center().x, sep.bottom() - inset),
                                            ],
                                            egui::Stroke::new(1., p.border_soft),
                                        );
                                    }
                                    T(Icon::Add) => {
                                        if let Some(icon) = self.add_menu(ui, scene, actions) {
                                            pressed = Some(icon);
                                        }
                                    }
                                    T(icon) => {
                                        let state = Self::tool_state(icon, scene);
                                        if icons::button_tip(
                                            ui,
                                            icon,
                                            &tip(icon, state),
                                            self.tool_active(icon, scene),
                                            state.is_ok(),
                                        ) {
                                            pressed = Some(icon);
                                        }
                                    }
                                    Group(icon, sections) => {
                                        if let Some(icon) = self.group(ui, icon, sections, scene) {
                                            pressed = Some(icon);
                                        }
                                    }
                                }
                            }
                        });
                    });
            });
        crate::tour::anchor(ui, crate::tour::Anchor::Toolbar, bar.response.rect);
        if let Some(icon) = pressed {
            self.press(icon, scene, actions);
        }
    }

    /// The Add button: a menu with a model file, a text and the built-in models' gallery
    /// ("Add shape", `panels::shapes`). Returns the tool picked (Add model, Text).
    fn add_menu(
        &mut self,
        ui: &mut egui::Ui,
        scene: &Scene<'_>,
        actions: &mut Vec<Action>,
    ) -> Option<Icon> {
        let enabled = Self::tool_state(Icon::Add, scene).is_ok();
        let tip = format!(
            "Add: a model file ({}), a text, or a ready-made shape",
            Icon::Add.shortcut()
        );
        let response = icons::group_button(ui, Icon::Add, &tip, false, enabled);
        if std::mem::take(&mut self.open_add_menu) {
            egui::Popup::open_id(ui.ctx(), egui::Popup::default_response_id(&response));
        }
        let object = scene
            .selected
            .and_then(|i| scene.objects.get(i))
            .map(|o| crate::panels::tree::short_name(&o.name));
        let mut picked = None;
        egui::Popup::menu(&response).gap(8.).show(|ui| {
            ui.set_width(crate::panels::shapes::width().max(icons::MENU_W));
            ui.spacing_mut().item_spacing.y = 0.;
            for tool in [Icon::Add, Icon::Text] {
                let state = Self::tool_state(tool, scene);
                if icons::menu_row(ui, tool, false, state.is_ok(), state.err()) {
                    picked = Some(tool);
                    ui.close();
                }
            }
            ui.add_space(3.);
            ui.separator();
            ui.add_space(3.);
            ui.spacing_mut().item_spacing.y = 4.;
            if let Some((m, add_as)) = crate::panels::shapes::gallery(ui, object.as_deref(), true) {
                actions.push(Action::AddShape(m, add_as));
            }
        });
        picked
    }

    /// A fly-out button: the open tool's icon (highlighted) or the group's own; a click
    /// opens the menu of its tools. Returns the tool picked.
    fn group(
        &self,
        ui: &mut egui::Ui,
        icon: Icon,
        sections: &[&[Icon]],
        scene: &Scene<'_>,
    ) -> Option<Icon> {
        let rows: Vec<Vec<(Icon, bool, ToolState)>> = sections
            .iter()
            .map(|s| {
                s.iter()
                    // Texture painting is gone with Textures switched off (Preferences).
                    .filter(|&&i| i != Icon::PaintTexture || scene.textures)
                    .map(|&i| {
                        let state = Self::tool_state(i, scene);
                        (i, self.tool_active(i, scene), state)
                    })
                    .collect()
            })
            .collect();
        let open = rows.iter().flatten().find(|r| r.1).map(|r| r.0);
        let enabled = rows.iter().flatten().any(|r| r.2.is_ok());
        let why_off = rows
            .iter()
            .flatten()
            .find_map(|r| r.2.err())
            .filter(|w| !enabled && !w.is_empty());
        let tip = match (open, why_off) {
            (Some(tool), _) => tool.tooltip(),
            (None, Some(why)) => format!("{}: {why}", icon.text().0),
            (None, None) => {
                let names: Vec<String> = rows
                    .iter()
                    .flatten()
                    .map(|&(i, ..)| {
                        let key = i.shortcut();
                        if key.is_empty() {
                            i.text().0.to_string()
                        } else {
                            format!("{} ({key})", i.text().0)
                        }
                    })
                    .collect();
                format!("{}: {}", icon.text().0, names.join(", "))
            }
        };
        let response = icons::group_button(ui, open.unwrap_or(icon), &tip, open.is_some(), enabled);
        let mut picked = None;
        egui::Popup::menu(&response).gap(8.).show(|ui| {
            ui.set_width(icons::MENU_W);
            ui.spacing_mut().item_spacing.y = 0.;
            for (n, section) in rows.iter().enumerate() {
                if n > 0 {
                    ui.add_space(3.);
                    ui.separator();
                    ui.add_space(3.);
                }
                for &(tool, active, state) in section {
                    let why = state.err().filter(|w| !w.is_empty());
                    if icons::menu_row(ui, tool, active, state.is_ok(), why) {
                        picked = Some(tool);
                        ui.close();
                    }
                }
            }
        });
        picked
    }
}

/// Why a tool for the selected object is off: the plate is empty, or nothing is selected.
pub const ADD_FIRST: &str = "Add a model first";
pub const SELECT_FIRST: &str = "Select a model first: click one in the 3D view or the object list";

/// Tools that work on the selected object (or the plate's only one).
pub(super) fn needs_object(icon: Icon) -> bool {
    matches!(
        icon,
        Icon::Move
            | Icon::Rotate
            | Icon::Scale
            | Icon::LayOnFace
            | Icon::Cut
            | Icon::Boolean
            | Icon::Simplify
            | Icon::Repair
            | Icon::LayerHeights
            | Icon::BrimEars
            | Icon::PaintSupport
            | Icon::PaintSeam
            | Icon::PaintColour
            | Icon::PaintFuzzy
            | Icon::PaintTexture
    )
}

/// The toolbar button a tool's shortcut stands for.
pub(super) fn icon_for(tool: crate::keymap::Tool) -> Option<Icon> {
    LAYOUTS[LAYOUTS.len() - 1]
        .iter()
        .flat_map(|slot| match slot {
            Group(_, sections) => sections.iter().flat_map(|s| s.iter()).copied().collect(),
            T(icon) => vec![*icon],
            Sep => vec![],
        })
        .find(|i| i.act() == Some(crate::keymap::Act::Tool(tool)))
}

/// Whether a tool can be used, else why not.
type ToolState = Result<(), &'static str>;

/// What decides whether a toolbar tool can be used.
#[derive(Clone, Copy, Debug)]
pub(super) struct Plate {
    /// Prepare, not slicing: the scene can be edited.
    pub editing: bool,
    /// Any plate has an object.
    pub any_object: bool,
    /// Objects on the current plate.
    pub plate_objects: usize,
    pub selected: bool,
    /// The current plate has exactly one object (picked when a tool needs one).
    pub only_object: bool,
    /// Tools (filaments and mixed colors) to paint with.
    pub tools: usize,
    pub textures: bool,
}

/// Whether `icon`'s tool can be used, else why not ("" while slicing or in Preview, where
/// the whole bar is off).
pub(super) fn availability(icon: Icon, s: &Plate) -> Result<(), &'static str> {
    if icon == Icon::Section {
        return if s.any_object { Ok(()) } else { Err(ADD_FIRST) };
    }
    if !s.editing {
        return Err("");
    }
    let picked = if s.plate_objects == 0 {
        Err(ADD_FIRST)
    } else if s.selected || s.only_object {
        Ok(())
    } else {
        Err(SELECT_FIRST)
    };
    match icon {
        Icon::Arrange | Icon::Measure if !s.any_object => Err(ADD_FIRST),
        // Color painting needs more than one tool.
        Icon::PaintColour if s.tools < 2 => Err("needs two or more filaments loaded"),
        Icon::PaintTexture if !s.textures => Err("Textures are off in Settings › Features"),
        _ if needs_object(icon) => picked,
        _ => Ok(()),
    }
}

/// A button's tooltip: what it does, or why it is off right now.
pub(super) fn tip(icon: Icon, state: Result<(), &str>) -> String {
    match state {
        Err(why) if !why.is_empty() => format!("{}: {why}", icon.text().0),
        _ => icon.tooltip(),
    }
}

fn paint_kind(icon: Icon) -> Option<PaintKind> {
    Some(match icon {
        Icon::PaintSupport => PaintKind::Supports,
        Icon::PaintSeam => PaintKind::Seam,
        Icon::PaintColour => PaintKind::Material,
        Icon::PaintFuzzy => PaintKind::Fuzzy,
        Icon::PaintTexture => PaintKind::Texture,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every tool, on an empty plate, with models but none selected, with the only model
    /// unselected and with a selection: usable, or off with a reason in its tooltip. Never
    /// usable with nothing to work on.
    #[test]
    fn every_tool_is_usable_or_says_why() {
        let base = Plate {
            editing: true,
            any_object: true,
            plate_objects: 2,
            selected: false,
            only_object: false,
            tools: 4,
            textures: true,
        };
        let cases = [
            (
                "empty plate",
                Plate {
                    any_object: false,
                    plate_objects: 0,
                    ..base
                },
            ),
            ("several, none selected", base),
            (
                "the only one, unselected",
                Plate {
                    plate_objects: 1,
                    only_object: true,
                    ..base
                },
            ),
            (
                "selected",
                Plate {
                    selected: true,
                    ..base
                },
            ),
        ];
        for icon in tools(LAYOUTS[0]) {
            for (case, plate) in &cases {
                let state = availability(icon, plate);
                if let Err(why) = state {
                    assert!(!why.is_empty(), "{icon:?}, {case}: off without a reason");
                    assert!(
                        tip(icon, state).contains(why),
                        "{icon:?}, {case}: the tooltip says why"
                    );
                }
                if needs_object(icon) {
                    let expect = match *case {
                        "empty plate" => Err(ADD_FIRST),
                        "several, none selected" => Err(SELECT_FIRST),
                        _ => Ok(()),
                    };
                    assert_eq!(state, expect, "{icon:?}, {case}");
                }
            }
            // Text and SVG works without a selection, even on an empty plate.
            if icon == Icon::Text {
                assert_eq!(availability(icon, &cases[0].1), Ok(()));
            }
        }
        // Every per-object tool is in the list, and every shortcut maps back to its button.
        for icon in tools(LAYOUTS[0]) {
            if let Some(crate::keymap::Act::Tool(t)) = icon.act() {
                assert_eq!(icon_for(t), Some(icon), "{icon:?}");
            }
        }
        assert_eq!(
            availability(
                Icon::PaintColour,
                &Plate {
                    tools: 1,
                    selected: true,
                    ..base
                }
            ),
            Err("needs two or more filaments loaded")
        );
    }

    fn tools(slots: &[Slot]) -> Vec<Icon> {
        let mut out: Vec<Icon> = slots
            .iter()
            .flat_map(|s| match *s {
                T(i) => vec![i],
                Group(_, sections) => sections.iter().flat_map(|s| s.iter().copied()).collect(),
                Sep => vec![],
            })
            .collect();
        out.sort_by_key(|i| format!("{i:?}"));
        out
    }

    #[test]
    fn every_layout_has_every_tool_once() {
        let all = tools(LAYOUTS[0]);
        assert_eq!(all.len(), 20);
        let mut dedup = all.clone();
        dedup.dedup();
        assert_eq!(dedup, all, "a tool appears twice");
        for (n, layout) in LAYOUTS.iter().enumerate() {
            assert_eq!(tools(layout), all, "layout {n}");
        }
    }

    const SIZES: [icons::ToolbarSize; 3] = icons::ToolbarSize::ALL;

    #[test]
    fn layouts_get_narrower() {
        for size in SIZES {
            let m = Metrics::of(size);
            let widths: Vec<f32> = LAYOUTS.iter().map(|l| width(l, &m)).collect();
            assert!(
                widths.windows(2).all(|w| w[0] > w[1]),
                "{size:?} {widths:?}"
            );
        }
    }

    /// Each size is larger than the one before it, every glyph fits its button with room to
    /// spare, and Normal and Large meet the sizes asked for (glyph ink about 3/4 of its box).
    #[test]
    fn sizes_grow_and_fit() {
        let all: Vec<Metrics> = SIZES.iter().map(|&s| Metrics::of(s)).collect();
        for w in all.windows(2) {
            assert!(w[1].button > w[0].button && w[1].glyph > w[0].glyph);
            assert!(w[1].sep >= w[0].sep && w[1].gap >= w[0].gap);
        }
        for m in &all {
            assert!(m.glyph + 8. <= m.button, "{m:?}");
        }
        let normal = Metrics::of(icons::ToolbarSize::Normal);
        let large = Metrics::of(icons::ToolbarSize::Large);
        assert_eq!(Metrics::of(icons::ToolbarSize::Compact).button, 28.);
        assert_eq!(normal.button, 36.);
        assert!((18.0..=20.).contains(&(normal.glyph * 0.75)));
        assert_eq!(large.button, 44.);
        assert!((23.0..=25.).contains(&(large.glyph * 0.75)));
    }

    #[test]
    fn every_tool_with_a_key_shows_it() {
        for icon in tools(LAYOUTS[0]) {
            if !matches!(icon, Icon::Simplify | Icon::Repair | Icon::Section) {
                assert!(!icon.shortcut().is_empty(), "{icon:?} has no shortcut");
                assert!(icon.tooltip().contains(&icon.shortcut()), "{icon:?}");
            }
        }
    }

    /// The toolbar level for a `window` px wide window (default 370 px sidebar) with the
    /// object list and the tool card: the layout fits between them.
    fn level_at(window: f32, m: &Metrics) -> usize {
        let view = window - 370.;
        let blocked = [
            (f32::MIN, EDGE + LIST_RESERVE + CLEAR),
            (view - EDGE - CARD_RESERVE - CLEAR, f32::MAX),
        ];
        let free = free_spans(EDGE, view - EDGE, &blocked);
        let (level, x) = fit(&free, view / 2., m).expect("fits");
        let w = width(LAYOUTS[level], m);
        assert!(x - w / 2. >= EDGE + LIST_RESERVE + CLEAR, "{window}");
        assert!(x + w / 2. <= view - EDGE - CARD_RESERVE - CLEAR, "{window}");
        level
    }

    /// Wider windows show more of the toolbar at every size, a larger toolbar never shows
    /// more than a smaller one, and 1920 px always has the whole toolbar.
    #[test]
    fn fits_between_the_cards() {
        let windows = [1100., 1280., 1440., 1710., 1920., 2560.];
        let mut previous: Option<Vec<usize>> = None;
        for size in SIZES {
            let m = Metrics::of(size);
            let levels: Vec<usize> = windows.iter().map(|&w| level_at(w, &m)).collect();
            assert!(
                levels.windows(2).all(|l| l[1] <= l[0]),
                "{size:?} {levels:?}"
            );
            assert_eq!(levels[4], 0, "{size:?}: 1920 shows everything");
            if let Some(prev) = &previous {
                assert!(levels.iter().zip(prev).all(|(a, b)| a >= b), "{size:?}");
            }
            previous = Some(levels);
            // No room at all: nothing fits.
            assert!(fit(&free_spans(0., 20., &[]), 10., &m).is_none());
        }
    }

    /// The levels each size gets at the common window widths.
    #[test]
    fn levels_per_size() {
        let at = |size, window| level_at(window, &Metrics::of(size));
        use icons::ToolbarSize::{Compact, Large, Normal};
        // Compact: as before the sizes existed.
        assert_eq!(at(Compact, 1280.), 2);
        assert_eq!(at(Compact, 1440.), 0);
        assert_eq!(at(Compact, 1920.), 0);
        // Normal: the gizmos and "more" at 1280, everything but the per-object and view tools
        // at 1440, everything at 1710 (a 15-inch laptop at full width).
        assert_eq!(at(Normal, 1280.), 3);
        assert_eq!(at(Normal, 1440.), 1);
        assert_eq!(at(Normal, 1710.), 0);
        assert_eq!(at(Normal, 1920.), 0);
        // Large.
        assert_eq!(at(Large, 1280.), 3);
        assert_eq!(at(Large, 1440.), 3);
        assert_eq!(at(Large, 1710.), 1);
        assert_eq!(at(Large, 1920.), 0);
    }

    #[test]
    fn free_spans_cut_out_the_cards() {
        let free = free_spans(0., 100., &[(10., 20.), (50., 60.), (90., 200.)]);
        assert_eq!(free, vec![(0., 10.), (20., 50.), (60., 90.)]);
    }
}
