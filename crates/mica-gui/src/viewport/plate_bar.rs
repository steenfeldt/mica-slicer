// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The plate bar under the 3D view: ‹ the plate's name, its slice status, "2 of 3", › then the
//! plate menu (⋯) and Add plate. The name renames in place (click it, or F2); "2 of 3" opens
//! the plate list, where a click shows a plate and dragging a row reorders the plates; each
//! row shows the plate's slice status and the list ends with the totals over the sliced plates
//! and Slice all plates. ⋯ (or a right-click anywhere on the bar) opens the plate menu with
//! every plate action (`plate_menu`).

use super::{Action, Scene, Viewport, alert_dot, icons, icons::Icon};
use crate::plate_batch::{PlateStatus, Totals, status_mark};
use crate::plate_menu::{PlateCommand, PlateMenuInfo, plate_menu};
use crate::plates::default_name;
use crate::theme;
use eframe::egui::{self, Align2, Pos2, Rect, Sense, Stroke, Vec2 as EVec2};

/// A plate name being edited in the bar.
#[derive(Clone, Debug, PartialEq)]
pub struct PlateRename {
    pub index: usize,
    pub text: String,
    /// Focus the field and select its text on the next frame.
    pub focus: bool,
}

/// Where the bar's parts were drawn last frame (UI tests click them).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlateProbe {
    pub name: Option<Rect>,
    pub list: Option<Rect>,
    pub menu: Option<Rect>,
    /// The plate's slice status chip.
    pub status: Option<Rect>,
    /// The plate list's rows, while it is open.
    pub rows: Vec<Rect>,
    /// The plate list's Slice all plates button and its totals line, while it is open.
    pub slice_all: Option<Rect>,
    pub totals: Option<String>,
}

/// The row a plate dragged in the plate list carries.
#[derive(Clone, Copy, Debug)]
struct PlateDrag(usize);

const NAME_ID: &str = "plate bar name";

impl Viewport {
    /// Starts renaming plate `index` (the current one) in the plate bar.
    pub fn start_plate_rename(&mut self, index: usize, name: &str) {
        self.plate_rename = Some(PlateRename {
            index,
            text: name.to_string(),
            focus: true,
        });
    }

    pub(super) fn plate_bar(
        &mut self,
        ui: &mut egui::Ui,
        rect: Rect,
        scene: &Scene<'_>,
        actions: &mut Vec<Action>,
    ) {
        let current = scene.plates.iter().position(|p| p.current).unwrap_or(0);
        let count = scene.plates.len().max(1);
        let plate = scene.plates.get(current);
        let name = plate
            .and_then(|p| p.name.clone())
            .unwrap_or_else(|| default_name(current));
        let locked = plate.is_some_and(|p| p.locked);
        let info = PlateMenuInfo {
            index: current,
            count,
            name: name.clone(),
            locked,
            objects: plate.map_or(0, |p| p.objects),
            editable: scene.editable,
        };
        // A rename of another plate (the plate changed meanwhile) is dropped.
        if self
            .plate_rename
            .as_ref()
            .is_some_and(|r| r.index != current)
        {
            self.plate_rename = None;
        }
        let mut probe = PlateProbe::default();
        egui::Area::new(ui.scope_id().with("plate bar"))
            .order(egui::Order::Middle)
            .fixed_pos(Pos2::new(rect.center().x, rect.bottom() - 14.))
            .pivot(Align2::CENTER_BOTTOM)
            .show(ui.ctx(), |ui| {
                let p = theme::of(ui);
                let m = icons::metrics(ui.ctx());
                theme::overlay_frame(ui)
                    .inner_margin(egui::Margin::same(m.margin as i8))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = m.gap + 2.;
                            if icons::button(ui, Icon::Left, false, current > 0) {
                                actions.push(Action::SelectPlate(current - 1));
                            }
                            let alert = plate.map_or(0, |p| p.alert);
                            if alert > 0 {
                                alert_dot(ui, alert);
                            }
                            // The name: click (or F2) to rename in place.
                            let name_rect = if let Some(r) = self.plate_rename.as_mut() {
                                match name_field(ui, r, m.button, current) {
                                    Some(Some(text)) => {
                                        actions.push(Action::RenamePlate(current, text));
                                        self.plate_rename = None;
                                    }
                                    Some(None) => self.plate_rename = None,
                                    None => {}
                                }
                                ui.min_rect()
                            } else {
                                let r = name_label(ui, &name, m.button, scene.editable);
                                if r.clicked() && scene.editable {
                                    self.start_plate_rename(current, &name);
                                }
                                menu_on(&r, &info, current, actions, self);
                                r.rect
                            };
                            probe.name = Some(name_rect);
                            if locked
                                && icons::button_tip(
                                    ui,
                                    Icon::Lock,
                                    "This plate is locked: its objects stay where they are. \
                                     Click to unlock it.",
                                    true,
                                    scene.editable,
                                )
                            {
                                actions.push(Action::Plate(current, PlateCommand::Lock));
                            }
                            if let Some(plate) = plate
                                && plate.status != PlateStatus::Empty
                            {
                                probe.status = Some(status_chip(ui, &plate.status, m.button).rect);
                            }
                            // "2 of 3": the plate list.
                            let list = count_button(
                                ui,
                                &format!("{} of {}", current + 1, count),
                                m.button,
                            )
                            .on_hover_text("All plates: pick one, or drag them into a new order");
                            probe.list = Some(list.rect);
                            menu_on(&list, &info, current, actions, self);
                            if std::mem::take(&mut self.open_plate_list) {
                                egui::Popup::open_id(
                                    ui.ctx(),
                                    egui::Popup::default_response_id(&list),
                                );
                            }
                            egui::Popup::menu(&list)
                                .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                                .gap(10.)
                                .show(|ui| {
                                    let (rows, slice_all, totals) = plate_list(ui, scene, actions);
                                    probe.rows = rows;
                                    probe.slice_all = slice_all;
                                    probe.totals = Some(totals);
                                });
                            if icons::button(ui, Icon::Right, false, current + 1 < count) {
                                actions.push(Action::SelectPlate(current + 1));
                            }
                            // The plate menu.
                            let more = icons::button_response(
                                ui,
                                Icon::More,
                                "Plate menu: rename, settings, duplicate, move, delete…",
                                true,
                            );
                            probe.menu = Some(more.rect);
                            if std::mem::take(&mut self.open_plate_menu) {
                                egui::Popup::open_id(
                                    ui.ctx(),
                                    egui::Popup::default_response_id(&more),
                                );
                            }
                            egui::Popup::menu(&more).gap(10.).show(|ui| {
                                if let Some(c) = plate_menu(ui, &info) {
                                    self.pick(current, c, &name, actions);
                                }
                            });
                            let (sep, _) = ui.allocate_exact_size(
                                EVec2::new(m.sep - 2., m.button),
                                egui::Sense::hover(),
                            );
                            let inset = m.button * 0.2;
                            ui.painter().line_segment(
                                [
                                    Pos2::new(sep.center().x, sep.top() + inset),
                                    Pos2::new(sep.center().x, sep.bottom() - inset),
                                ],
                                egui::Stroke::new(1., p.border_soft),
                            );
                            if icons::button(ui, Icon::AddPlate, false, scene.editable) {
                                actions.push(Action::AddPlate);
                            }
                        });
                    });
            });
        self.plate_probe = probe;
    }

    /// A plate menu pick: Rename and Reorder are the bar's own (the field, the plate list);
    /// everything else goes to the editor.
    pub(super) fn pick(
        &mut self,
        index: usize,
        c: PlateCommand,
        name: &str,
        actions: &mut Vec<Action>,
    ) {
        match c {
            PlateCommand::Rename => {
                actions.push(Action::SelectPlate(index));
                self.start_plate_rename(index, name);
            }
            PlateCommand::Reorder => self.open_plate_list = true,
            c => actions.push(Action::Plate(index, c)),
        }
    }
}

/// The plate menu on a right-click of `r`.
fn menu_on(
    r: &egui::Response,
    info: &PlateMenuInfo,
    index: usize,
    actions: &mut Vec<Action>,
    vp: &mut Viewport,
) {
    r.context_menu(|ui| {
        if let Some(c) = plate_menu(ui, info) {
            vp.pick(index, c, &info.name, actions);
        }
    });
}

/// The plate's name, clickable to rename: a soft outline on hover says it is editable.
fn name_label(ui: &mut egui::Ui, name: &str, height: f32, editable: bool) -> egui::Response {
    let p = theme::of(ui);
    let galley = ui
        .painter()
        .layout_no_wrap(name.to_string(), theme::semibold(13.), p.text);
    let size = EVec2::new(galley.size().x + 12., height);
    let (rect, r) = ui.allocate_exact_size(size, Sense::click());
    if editable && r.hovered() {
        ui.painter().rect_stroke(
            rect.shrink2(EVec2::new(0., 3.)),
            5.,
            Stroke::new(1., p.border),
            egui::StrokeKind::Inside,
        );
        ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::Text);
    }
    ui.painter().galley(
        Pos2::new(rect.left() + 6., rect.center().y - galley.size().y / 2.),
        galley,
        p.text,
    );
    if editable {
        r.on_hover_text("Click to rename this plate (F2)")
    } else {
        r
    }
}

/// The name field while renaming: `Some(Some(text))` when confirmed (Enter, or clicking
/// elsewhere), `Some(None)` when canceled (Esc), `None` while editing.
fn name_field(
    ui: &mut egui::Ui,
    r: &mut PlateRename,
    height: f32,
    index: usize,
) -> Option<Option<String>> {
    let id = ui.make_persistent_id(NAME_ID);
    if std::mem::take(&mut r.focus) {
        let mut state = egui::text_edit::TextEditState::load(ui.ctx(), id).unwrap_or_default();
        state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::select_all_str(&r.text)));
        state.store(ui.ctx(), id);
        ui.memory_mut(|m| m.request_focus(id));
    }
    let width = ui
        .painter()
        .layout_no_wrap(r.text.clone(), theme::semibold(13.), egui::Color32::WHITE)
        .size()
        .x
        .clamp(120., 260.)
        + 16.;
    let response = ui.add_sized(
        [width, height - 4.],
        egui::TextEdit::singleline(&mut r.text)
            .id(id)
            .font(theme::semibold(13.))
            .hint_text(default_name(index))
            .align(Align2::LEFT_CENTER)
            .margin(egui::Margin::symmetric(6, 4)),
    );
    let escape = ui.input(|i| i.key_pressed(egui::Key::Escape));
    if escape {
        return Some(None);
    }
    let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
    if response.lost_focus() || (enter && response.has_focus()) {
        return Some(Some(r.text.trim().to_string()));
    }
    // Focus requested this frame arrives next frame; until then the field is not "lost".
    None
}

/// "2 of 3" with a small chevron: opens the plate list.
fn count_button(ui: &mut egui::Ui, text: &str, height: f32) -> egui::Response {
    let p = theme::of(ui);
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_string(), theme::sans(12.5), p.muted_alt);
    let size = EVec2::new(galley.size().x + 26., height);
    let (rect, r) = ui.allocate_exact_size(size, Sense::click());
    let open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&r));
    if r.hovered() || open {
        ui.painter()
            .rect_filled(rect.shrink2(EVec2::new(0., 3.)), 5., p.raised);
        ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
    }
    let text_pos = Pos2::new(rect.left() + 7., rect.center().y - galley.size().y / 2.);
    ui.painter().galley(text_pos, galley, p.muted_alt);
    // The chevron: points up, as the list opens above the bar.
    let c = Pos2::new(rect.right() - 10., rect.center().y);
    ui.painter().add(egui::Shape::line(
        vec![
            c + EVec2::new(-3.5, 1.75),
            c + EVec2::new(0., -1.75),
            c + EVec2::new(3.5, 1.75),
        ],
        Stroke::new(1.4, p.muted_alt),
    ));
    r
}

/// The plate's slice status in the bar: its mark and a few words ("38 min · 11.6 g", "not
/// sliced", "failed: …"); the hover says more (a failure's whole reason).
fn status_chip(ui: &mut egui::Ui, status: &PlateStatus, height: f32) -> egui::Response {
    let p = theme::of(ui);
    let text = status.text();
    let color = match status {
        PlateStatus::Failed { .. } => theme::DANGER,
        PlateStatus::Sliced { .. } => p.text,
        _ => p.muted_alt,
    };
    let mut job = egui::text::LayoutJob::simple_singleline(text, theme::sans(12.), color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(200.);
    let galley = ui.painter().layout_job(job);
    let size = EVec2::new(galley.size().x + 26., height);
    let (rect, r) = ui.allocate_exact_size(size, Sense::hover());
    status_mark(
        ui.painter(),
        Pos2::new(rect.left() + 8., rect.center().y),
        status,
    );
    ui.painter().galley(
        Pos2::new(rect.left() + 20., rect.center().y - galley.size().y / 2.),
        galley,
        color,
    );
    r.on_hover_text(status.tooltip())
}

/// The plate list: one row per plate (number, name, objects and lock, slice status), the
/// current one marked, then the totals over the sliced plates and Slice all plates. A click
/// shows the plate; dragging a row reorders. Returns the rows' rects, the Slice all button's
/// and the totals text.
fn plate_list(
    ui: &mut egui::Ui,
    scene: &Scene<'_>,
    actions: &mut Vec<Action>,
) -> (Vec<Rect>, Option<Rect>, String) {
    let p = theme::of(ui);
    let width = 380.;
    ui.set_width(width);
    ui.spacing_mut().item_spacing.y = 2.;
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("Plates")
                .font(theme::semibold(13.))
                .color(p.text),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new(if scene.plates.len() > 1 && scene.editable {
                    "Drag to reorder"
                } else {
                    ""
                })
                .font(theme::sans(11.5))
                .color(p.muted),
            );
        });
    });
    ui.add_space(2.);
    let dragging = egui::DragAndDrop::payload::<PlateDrag>(ui.ctx()).map(|d| d.0);
    let mut rows = vec![];
    for plate in scene.plates {
        let i = plate.index;
        let sense = if scene.editable && scene.plates.len() > 1 {
            Sense::click_and_drag()
        } else {
            Sense::click()
        };
        let (rect, r) = ui.allocate_exact_size(EVec2::new(width, 40.), sense);
        rows.push(rect);
        let painter = ui.painter();
        if plate.current {
            painter.rect_filled(rect, 6., p.selected_row);
        } else if r.hovered() || dragging == Some(i) {
            painter.rect_filled(rect, 6., p.raised);
        }
        // Grip: two columns of dots.
        if sense.senses_drag() {
            for k in 0..3 {
                for c in 0..2 {
                    painter.circle_filled(
                        Pos2::new(
                            rect.left() + 10. + c as f32 * 4.,
                            rect.center().y - 4. + k as f32 * 4.,
                        ),
                        1.1,
                        p.muted,
                    );
                }
            }
        }
        let y = rect.center().y;
        let (top, bottom) = (y - 8., y + 8.);
        painter.text(
            Pos2::new(rect.left() + 32., top),
            Align2::RIGHT_CENTER,
            format!("{}", i + 1),
            theme::sans(12.),
            p.muted,
        );
        // Right: the slice status, its mark and words (the whole reason on hover).
        let status_width = 150.;
        let status_left = rect.right() - 10. - status_width;
        if plate.status != PlateStatus::Empty {
            status_mark(painter, Pos2::new(status_left + 6., y), &plate.status);
        }
        let status_color = match plate.status {
            PlateStatus::Failed { .. } => theme::DANGER,
            PlateStatus::Sliced { .. } => p.text,
            _ => p.muted_alt,
        };
        let mut job = egui::text::LayoutJob::simple_singleline(
            plate.status.text(),
            theme::sans(12.),
            status_color,
        );
        job.wrap = egui::text::TextWrapping::truncate_at_width(status_width - 18.);
        let g = painter.layout_job(job);
        painter.galley(
            Pos2::new(status_left + 18., y - g.size().y / 2.),
            g,
            status_color,
        );
        let status_rect = Rect::from_min_max(
            Pos2::new(status_left, rect.top()),
            Pos2::new(rect.right(), rect.bottom()),
        );
        // Under the name: objects, lock, warnings.
        let mut details = vec![match plate.objects {
            0 => "no objects".to_string(),
            1 => "1 object".to_string(),
            n => format!("{n} objects"),
        }];
        if plate.locked {
            details.push("locked".into());
        }
        if plate.alert == 1 {
            details.push("warnings".into());
        }
        let name = plate.name.clone().unwrap_or_else(|| default_name(i));
        let name_rect = Rect::from_min_max(
            Pos2::new(rect.left() + 42., rect.top()),
            Pos2::new(status_left - 8., rect.bottom()),
        );
        let clipped = painter.with_clip_rect(name_rect);
        clipped.text(
            Pos2::new(name_rect.left(), top),
            Align2::LEFT_CENTER,
            name,
            if plate.current {
                theme::semibold(13.)
            } else {
                theme::sans(13.)
            },
            p.text,
        );
        clipped.text(
            Pos2::new(name_rect.left(), bottom),
            Align2::LEFT_CENTER,
            details.join(" · "),
            theme::sans(11.5),
            p.muted,
        );
        if r.clicked() {
            actions.push(Action::SelectPlate(i));
        }
        if r.drag_started() || r.dragged() {
            r.dnd_set_drag_payload(PlateDrag(i));
            ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::Grabbing);
        } else if r.hovered() && sense.senses_drag() {
            ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::Grab);
        }
        // Where a dragged plate would land: above or below this row's middle.
        let target = |pointer: Pos2, from: usize| {
            let slot = if pointer.y < y { i } else { i + 1 };
            if from < slot { slot - 1 } else { slot }
        };
        if let (Some(from), Some(pointer)) = (dragging, ui.ctx().pointer_hover_pos())
            && r.contains_pointer()
            && from != target(pointer, from)
        {
            let line_y = if pointer.y < y {
                rect.top() - 1.
            } else {
                rect.bottom() + 1.
            };
            ui.painter().line_segment(
                [
                    Pos2::new(rect.left() + 4., line_y),
                    Pos2::new(rect.right() - 4., line_y),
                ],
                Stroke::new(2., p.accent),
            );
        }
        if let Some(d) = r.dnd_release_payload::<PlateDrag>()
            && let Some(pointer) = ui.ctx().pointer_latest_pos()
        {
            let to = target(pointer, d.0);
            if to != d.0 {
                actions.push(Action::MovePlate { from: d.0, to });
            }
        }
        let pointer_on_status = ui
            .ctx()
            .pointer_hover_pos()
            .is_some_and(|at| status_rect.contains(at));
        if pointer_on_status && plate.status != PlateStatus::Empty {
            r.on_hover_text(plate.status.tooltip());
        } else {
            r.on_hover_text(if sense.senses_drag() {
                "Click to show this plate, drag to move it in the order"
            } else {
                "Click to show this plate"
            });
        }
    }
    // Totals over the sliced plates.
    let statuses: Vec<PlateStatus> = scene.plates.iter().map(|p| p.status.clone()).collect();
    let totals = Totals::of(&statuses);
    let totals_text = if totals.sliced > 0 {
        format!("{} · {}", totals.text(), totals.note())
    } else {
        totals.note()
    };
    ui.add_space(4.);
    ui.separator();
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("Total")
                .font(theme::semibold(12.5))
                .color(p.text),
        );
        let text = egui::RichText::new(if totals.sliced > 0 {
            totals.text()
        } else {
            "—".into()
        })
        .font(theme::mono(12.))
        .color(p.text);
        ui.label(text).on_hover_text(if totals.sliced > 0 {
            format!(
                "Print time · filament weight · filament length over the {} sliced {}",
                totals.sliced,
                if totals.sliced == 1 {
                    "plate"
                } else {
                    "plates"
                }
            )
        } else {
            "No plate is sliced yet".to_string()
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new(totals.note())
                    .font(theme::sans(11.5))
                    .color(p.muted),
            );
        });
    });
    ui.add_space(2.);
    let mut slice_all = None;
    ui.horizontal(|ui| {
        if ui
            .add_enabled(scene.editable, egui::Button::new("+ Add plate"))
            .clicked()
        {
            actions.push(Action::AddPlate);
        }
        let waiting = statuses.iter().any(|s| {
            matches!(
                s,
                PlateStatus::NotSliced | PlateStatus::Outdated | PlateStatus::Failed { .. }
            )
        });
        let busy = statuses
            .iter()
            .any(|s| matches!(s, PlateStatus::Queued | PlateStatus::Slicing { .. }));
        let r = ui
            .add_enabled(
                waiting && !busy && scene.can_slice_all,
                egui::Button::new("Slice all plates")
                    .shortcut_text(crate::keymap::shortcut_text(crate::keymap::Act::SliceAll)),
            )
            .on_hover_text("Slice every plate that is not sliced yet or changed")
            .on_disabled_hover_text(if busy {
                "Slicing…"
            } else {
                "Every plate with objects is sliced"
            });
        slice_all = Some(r.rect);
        if r.clicked() {
            actions.push(Action::SliceAll);
        }
    });
    (rows, slice_all, totals_text)
}
