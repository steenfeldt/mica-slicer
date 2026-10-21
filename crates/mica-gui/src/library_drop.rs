// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Dragging models from the Library onto the plate. A card in the grid (or the details
//! panel's preview) carries the model, or every picked model, as an egui drag payload. Held
//! over the Prepare tab it switches to Prepare; released over the 3D view it adds the models
//! where they were dropped, released over an object it adds them as parts of that object
//! (with a small choice of part kind afterwards), released on the Prepare tab it adds them
//! next to what is on the plate. Each drop is one undo step. G-code is not added to a plate:
//! it opens in Preview instead.

use crate::app::{App, LoadResult};
use crate::scene::{self, Object, Sub, Volume};
use crate::theme;
use eframe::egui::{self, Align2, Pos2, Rect, Stroke, Vec2};
use glam::{Mat3, Mat4, Vec3};
use mica_core::threemf::PartKind;
use mica_library::Kind;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::mpsc;

/// One model a drag carries.
#[derive(Clone, Debug, PartialEq)]
pub struct DragItem {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
    pub kind: Kind,
    pub online: bool,
}

/// The drag payload: the models in display order.
#[derive(Clone, Debug, PartialEq)]
pub struct LibraryDrag(pub Vec<DragItem>);

/// Where a drop lands.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    /// On a plate's bed at a plate-local point; `None`: next to what is on it.
    Plate { plate: usize, at: Option<Vec3> },
    /// Onto an object (by id) at a plate-local point on its surface; `screen` is where the
    /// part-kind choice opens.
    Part {
        plate: usize,
        object: u64,
        at: Vec3,
        screen: Pos2,
    },
}

/// Models dropped and still loading.
pub struct PendingDrop {
    rx: mpsc::Receiver<LoadResult>,
    target: Target,
}

/// The part-kind choice after a drop onto an object (changes the new volumes in place, so the
/// drop stays one undo step).
#[derive(Clone, Debug, PartialEq)]
pub struct PartPopover {
    pub plate: usize,
    pub object: u64,
    pub volumes: Vec<u64>,
    pub at: Pos2,
    pub kind: PartKind,
}

/// Headless screenshots: a drag in progress, or a drop onto the plate's first object.
#[derive(Clone, Debug, PartialEq)]
pub enum Shot {
    Hover(Vec<DragItem>),
    /// Dropped once the 3D view has been laid out (after a few frames).
    DropOnObject(Vec<DragItem>, u32),
}

/// The choices, in the order the popover shows them.
pub fn part_kinds() -> [(PartKind, &'static str, &'static str); 5] {
    [
        (PartKind::Model, "Part", "Printed as part of the object"),
        (
            PartKind::Modifier,
            "Modifier",
            "Changes the settings inside it (starts with 100% infill)",
        ),
        (
            PartKind::NegativeVolume,
            "Negative",
            "Cut out of the object",
        ),
        (
            PartKind::SupportEnforcer,
            "Enforcer",
            "Supports are added inside it",
        ),
        (PartKind::SupportBlocker, "Blocker", "No supports inside it"),
    ]
}

fn stem(name: &str) -> String {
    std::path::Path::new(name)
        .file_stem()
        .map_or_else(|| name.to_string(), |s| s.to_string_lossy().into_owned())
}

impl App {
    /// A card or the details preview: starts a drag carrying the model (or the picked ones).
    /// `location`: the copy a card inside a project stands for (its path is what is loaded).
    pub(crate) fn library_drag_source(
        &mut self,
        resp: &egui::Response,
        m: &mica_library::Model,
        location: Option<usize>,
    ) {
        if resp.drag_started() {
            let mut items = self.library.drag_items(m);
            if let (Some(l), [one]) = (location.and_then(|l| m.locations.get(l)), &mut items[..]) {
                one.path = l.path.clone();
                one.online = l.online;
            }
            egui::DragAndDrop::set_payload(&resp.ctx, LibraryDrag(items));
        }
    }

    /// A project card: carries all its parts (what "Add all parts to plate" adds), or, with
    /// no parts, its main 3MF (whose first plate lands on the plate).
    pub(crate) fn library_drag_project(
        &mut self,
        resp: &egui::Response,
        pr: &mica_library::Project,
    ) {
        if resp.drag_started() {
            let items = self.library.project_drag_items(pr);
            if !items.is_empty() {
                egui::DragAndDrop::set_payload(&resp.ctx, LibraryDrag(items));
            }
        }
    }

    /// Every frame: the dragged models under the pointer, loads that finished, the part-kind
    /// choice, and the preview's memory once the Library is closed.
    pub(crate) fn library_frame(&mut self, ctx: &egui::Context) {
        if !self.library.shown {
            self.library.preview = None;
        }
        match self.library.shot.take() {
            Some(Shot::Hover(items)) => egui::DragAndDrop::set_payload(ctx, LibraryDrag(items)),
            Some(Shot::DropOnObject(items, n)) if n < 3 => {
                self.library.shot = Some(Shot::DropOnObject(items, n + 1));
            }
            Some(Shot::DropOnObject(items, _)) => {
                if let Some(o) = self.plates.current().objects.first() {
                    let (min, max) = o.bounds();
                    let top = ((min + max) * 0.5).with_z(max.z);
                    let rect = self.viewport_rect;
                    let world = top + self.plates.offset(self.plates.current);
                    let screen = self
                        .viewport
                        .camera
                        .project(world, rect.aspect_ratio())
                        .map_or(rect.center(), |uv| {
                            Pos2::new(
                                rect.left() + uv.x * rect.width(),
                                rect.top() + uv.y * rect.height(),
                            )
                        });
                    let target = Target::Part {
                        plate: self.plates.current,
                        object: o.id,
                        at: top,
                        screen,
                    };
                    self.library_drop(items, target, ctx);
                }
            }
            None => {}
        }
        if let Some(pending) = &self.library.drop {
            match pending.rx.try_recv() {
                Ok(result) => {
                    let target = self.library.drop.take().map(|p| p.target);
                    if let Some(target) = target {
                        self.finish_library_drop(result, target);
                    }
                }
                Err(mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(50))
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.library.drop = None;
                    self.notify_error(
                        "Adding from the library failed",
                        "The model could not be read",
                    );
                }
            }
        }
        if let Some(drag) = egui::DragAndDrop::payload::<LibraryDrag>(ctx) {
            self.library_ghost(ctx, &drag.0);
        }
        if !self.library.shown {
            self.part_popover(ctx);
        }
    }

    /// The dragged card under the pointer: thumbnail, name, how many, and what a drop does.
    fn library_ghost(&mut self, ctx: &egui::Context, items: &[DragItem]) {
        let Some(pos) = ctx.pointer_latest_pos() else {
            return;
        };
        let Some(first) = items.first() else { return };
        let hint = self.library.drag_hint.take().unwrap_or_else(|| {
            if self.library.shown {
                "Drag onto Prepare to add to the plate".into()
            } else {
                "Drop on the plate".into()
            }
        });
        let p = theme::of_ctx(ctx);
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Tooltip,
            egui::Id::unique("lib-drag-ghost"),
        ));
        let rect = Rect::from_min_size(pos + Vec2::new(16., 12.), Vec2::new(236., 60.));
        painter.rect_filled(rect, 9., p.panel.gamma_multiply(0.96));
        painter.rect_stroke(
            rect,
            9.,
            Stroke::new(1., p.accent),
            egui::StrokeKind::Inside,
        );
        let thumb = Rect::from_min_size(rect.min + Vec2::splat(6.), Vec2::splat(48.));
        painter.rect_filled(thumb, 6., p.search_bg);
        let model = self.library.model(&first.id).cloned();
        if let Some(m) = model
            && let Some(t) = self.library.texture(ctx, &m)
        {
            let s = t.size_vec2();
            let k = (thumb.width() / s.x).min(thumb.height() / s.y) * 0.9;
            painter.image(
                t.id(),
                Rect::from_center_size(thumb.center(), s * k),
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1., 1.)),
                egui::Color32::WHITE,
            );
        }
        let text_x = thumb.right() + 8.;
        let name = if items.len() > 1 {
            format!("{} + {} more", first.name, items.len() - 1)
        } else {
            first.name.clone()
        };
        let galley = painter.layout(name, theme::medium(12.), p.text, rect.right() - text_x - 8.);
        let mut job = (*galley.job).clone();
        job.wrap.max_rows = 1;
        job.wrap.break_anywhere = true;
        let galley = painter.fonts_mut(|f| f.layout_job(job));
        painter.galley(Pos2::new(text_x, rect.top() + 11.), galley, p.text);
        painter.text(
            Pos2::new(text_x, rect.top() + 40.),
            Align2::LEFT_CENTER,
            hint,
            theme::sans(11.),
            p.muted,
        );
        if items.len() > 1 {
            let c = Pos2::new(thumb.right() - 2., thumb.top() + 2.);
            painter.circle_filled(c, 9., p.accent);
            painter.text(
                c,
                Align2::CENTER_CENTER,
                items.len().to_string(),
                theme::semibold(10.),
                p.on_accent,
            );
        }
        ctx.request_repaint();
    }

    /// The Prepare tab during a drag: hovering switches to Prepare, a release there adds the
    /// models next to what is on the plate. `true` when it acted.
    pub(crate) fn library_drag_tab(&mut self, ui: &egui::Ui, r: Rect) -> bool {
        let ctx = ui.ctx().clone();
        if egui::DragAndDrop::payload::<LibraryDrag>(&ctx).is_none() {
            return false;
        }
        let over = ctx.pointer_latest_pos().is_some_and(|p| r.contains(p));
        if !over {
            return false;
        }
        ui.painter().rect_stroke(
            r,
            6.,
            Stroke::new(1.5, theme::of(ui).accent),
            egui::StrokeKind::Inside,
        );
        self.library.drag_hint = Some("Release to add next to the plate's models".into());
        if ctx.input(|i| i.pointer.any_released()) {
            if let Some(drag) = egui::DragAndDrop::take_payload::<LibraryDrag>(&ctx) {
                let plate = self.plates.current;
                self.library_drop(drag.0.clone(), Target::Plate { plate, at: None }, &ctx);
            }
            return true;
        }
        if self.library.shown || self.machine || self.preview {
            self.library.shown = false;
            self.machine = false;
            self.preview = false;
            self.library.open_prompt = None;
        }
        true
    }

    /// Where a drop at `pos` in the 3D view lands: an object under the pointer, else the bed.
    fn drop_target(&self, objects: &[Object], map: &[(usize, usize)], pos: Pos2) -> Option<Target> {
        let rect = self.viewport_rect;
        if !rect.contains(pos) {
            return None;
        }
        let aspect = rect.width() / rect.height().max(1.);
        let uv = glam::Vec2::new(
            (pos.x - rect.left()) / rect.width(),
            (pos.y - rect.top()) / rect.height(),
        );
        let (origin, dir) = self.viewport.camera.ray(uv, aspect);
        if let Some(hit) = scene::picking::pick_face(objects, origin, dir)
            && let Some(&(plate, k)) = map.get(hit.object)
            && let Some(o) = self.plates.plate(plate).objects.get(k)
        {
            let world = origin + dir * hit.distance;
            return Some(Target::Part {
                plate,
                object: o.id,
                at: world - self.plates.offset(plate),
                screen: pos,
            });
        }
        let world = self.viewport.camera.bed_point(uv, aspect)?;
        let (min, max) = (self.bed.min, self.bed.max);
        let plate = (0..self.plates.len())
            .find(|&i| {
                let l = world - self.plates.offset(i);
                l.x >= min.x && l.x <= max.x && l.y >= min.y && l.y <= max.y
            })
            .unwrap_or(self.plates.current);
        let mut at = world - self.plates.offset(plate);
        at.z = 0.;
        Some(Target::Plate {
            plate,
            at: Some(at),
        })
    }

    /// The 3D view during a drag: marks where the models would land and drops them on
    /// release. `objects`/`map` are the view's objects (every plate, world positions).
    pub(crate) fn library_drop_in_viewport(
        &mut self,
        objects: &[Object],
        map: &[(usize, usize)],
        ctx: &egui::Context,
    ) {
        let Some(drag) = egui::DragAndDrop::payload::<LibraryDrag>(ctx) else {
            return;
        };
        let Some(pos) = ctx.pointer_latest_pos() else {
            return;
        };
        if !self.viewport_rect.contains(pos) {
            return;
        }
        let gcode_only = drag.0.iter().all(|i| i.kind.is_gcode());
        let target = if gcode_only {
            None
        } else {
            self.drop_target(objects, map, pos)
        };
        let p = theme::of_ctx(ctx);
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::unique("lib-drop-target"),
        ));
        painter.rect_stroke(
            self.viewport_rect.shrink(3.),
            8.,
            Stroke::new(2., p.accent.gamma_multiply(0.6)),
            egui::StrokeKind::Inside,
        );
        self.library.drag_hint = Some(match &target {
            None if gcode_only => "Opens in Preview (G-code is not added to a plate)".into(),
            None => "Drop on the plate".into(),
            Some(Target::Part { plate, object, .. }) => {
                let name = self
                    .plates
                    .plate(*plate)
                    .objects
                    .iter()
                    .find(|o| o.id == *object)
                    .map_or_else(String::new, |o| stem(&o.name));
                painter.circle_stroke(pos, 10., Stroke::new(2., p.accent));
                painter.circle_filled(pos, 3., p.accent);
                format!("Add as a part of {name}")
            }
            Some(Target::Plate { .. }) => {
                painter.circle_stroke(pos, 14., Stroke::new(1.5, p.accent));
                if drag.0.len() > 1 {
                    format!("Add {} models here", drag.0.len())
                } else {
                    "Add to the plate here".into()
                }
            }
        });
        if ctx.input(|i| i.pointer.any_released())
            && let Some(drag) = egui::DragAndDrop::take_payload::<LibraryDrag>(ctx)
        {
            let target = target.unwrap_or(Target::Plate {
                plate: self.plates.current,
                at: None,
            });
            self.library_drop(drag.0.clone(), target, ctx);
        }
    }

    /// Adds dropped models: loads them (in the background in the app) and places them.
    pub(crate) fn library_drop(
        &mut self,
        items: Vec<DragItem>,
        target: Target,
        ctx: &egui::Context,
    ) {
        let (gcode, rest): (Vec<DragItem>, Vec<DragItem>) =
            items.into_iter().partition(|i| i.kind.is_gcode());
        let (models, other): (Vec<DragItem>, Vec<DragItem>) =
            rest.into_iter().partition(|i| i.kind.openable());
        let (models, offline): (Vec<DragItem>, Vec<DragItem>) =
            models.into_iter().partition(|i| i.online);
        if models.is_empty() {
            if let Some(g) = gcode.first().filter(|g| g.online) {
                self.library_act(crate::panels::library::Act::OpenGcode(g.path.clone()), ctx);
            } else if let Some(o) = offline.first().or(gcode.first()) {
                self.notify_warning(format!("{} is on a folder that is offline", o.name));
            } else if let Some(o) = other.first() {
                self.notify_warning(format!(
                    "Mica can't add {} files to the plate yet",
                    o.kind.label()
                ));
            }
            return;
        }
        let left_out = gcode.len() + other.len() + offline.len();
        if left_out > 0 {
            self.notify_warning(format!(
                "{left_out} of the dragged files {} not added: only STL and 3MF models available right now go onto the plate",
                if left_out == 1 { "was" } else { "were" }
            ));
        }
        if self.loading.is_some() || self.library.drop.is_some() {
            self.notify_warning("Mica is still opening files; drop again in a moment");
            return;
        }
        let plate = match &target {
            Target::Plate { plate, .. } | Target::Part { plate, .. } => *plate,
        };
        self.close_gcode();
        self.library.shown = false;
        self.machine = false;
        self.preview = false;
        self.library.open_prompt = None;
        if plate < self.plates.len() {
            self.select_plate(plate);
        }
        if self.plate_locked() {
            return;
        }
        let paths: Vec<PathBuf> = models.iter().map(|m| m.path.clone()).collect();
        if self.headless {
            let result = crate::app::load_files(paths, &self.config, self.bed.center());
            self.finish_library_drop(result, target);
            return;
        }
        let config = self.config.clone();
        let center = self.bed.center();
        let (tx, rx) = mpsc::channel();
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(crate::app::load_files(paths, &config, center));
            repaint.request_repaint();
        });
        self.status = "Loading geometry…".into();
        self.library.drop = Some(PendingDrop { rx, target });
    }

    /// Places what a drop loaded (the objects of a project's first plate; its settings and
    /// other plates are not taken over).
    pub(crate) fn finish_library_drop(&mut self, result: LoadResult, target: Target) {
        let scene = match result {
            Ok(s) => s,
            Err(e) => {
                self.notify_error("Adding from the library failed", e);
                return;
            }
        };
        for w in &scene.warnings {
            self.notify_warning(w.clone());
        }
        let objects: Vec<Object> = scene
            .objects
            .into_iter()
            .filter(|(p, _)| p.is_none_or(|p| p == 0))
            .map(|(_, o)| o)
            .collect();
        if objects.is_empty() {
            self.notify_warning("The dropped file holds no models for the plate");
            return;
        }
        for p in scene.paths {
            self.prefs.push_recent(p);
        }
        self.prefs_dirty = true;
        match target {
            Target::Plate { plate, at } => self.drop_objects(plate, objects, at),
            Target::Part {
                plate,
                object,
                at,
                screen,
            } => {
                if self
                    .plates
                    .plate(plate.min(self.plates.len() - 1))
                    .objects
                    .iter()
                    .any(|o| o.id == object)
                {
                    self.drop_parts(plate, object, objects, at, screen);
                } else {
                    // The object went away while loading: on the bed at the same spot.
                    self.drop_objects(plate, objects, Some(at.with_z(0.)));
                }
            }
        }
    }

    /// New objects with their group centred on `at` (plate-local), or right of the plate's
    /// models; selected; one undo step.
    fn drop_objects(&mut self, plate: usize, objects: Vec<Object>, at: Option<Vec3>) {
        let plate = plate.min(self.plates.len() - 1);
        let bounds = |list: &[Object]| {
            list.iter().fold(
                (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)),
                |(lo, hi), o| {
                    let (a, b) = o.bounds();
                    (lo.min(a), hi.max(b))
                },
            )
        };
        let (lo, hi) = bounds(&objects);
        let centre = (lo + hi) * 0.5;
        let at = at.unwrap_or_else(|| {
            let existing = &self.plates.plate(plate).objects;
            let bed = self.bed.center();
            if existing.is_empty() {
                return bed;
            }
            // Right of what is there, when it fits on the bed; else the middle.
            let (_, ehi) = bounds(existing);
            let x = ehi.x + 10. + (hi.x - lo.x) * 0.5;
            if x + (hi.x - lo.x) * 0.5 <= self.bed.max.x {
                Vec3::new(x, bed.y, 0.)
            } else {
                bed
            }
        });
        let shift = Vec3::new(at.x - centre.x, at.y - centre.y, 0.);
        let names: Vec<String> = objects.iter().map(|o| stem(&o.name)).collect();
        self.record("Add from library");
        let p = &mut self.plates.plates[plate];
        let first = p.objects.len();
        let base = p
            .objects
            .iter()
            .map(|o| o.object_id)
            .max()
            .map_or(0, |n| n + 1);
        let mut ids: BTreeMap<usize, usize> = BTreeMap::new();
        for mut o in objects {
            o.position += shift;
            let next = base + ids.len();
            o.object_id = *ids.entry(o.object_id).or_insert(next);
            p.objects.push(o);
        }
        let n = p.objects.len();
        p.invalidate();
        self.changed();
        self.select_many(&(first..n).collect::<Vec<_>>(), false);
        self.status = match names.as_slice() {
            [one] => format!("Added {one} from the library"),
            many => format!("Added {} models from the library", many.len()),
        };
    }

    /// Each loaded file becomes one model part of the object, centred on `at` (plate-local, on
    /// its surface); the part-kind choice opens at `screen`. One undo step.
    fn drop_parts(
        &mut self,
        plate: usize,
        object: u64,
        objects: Vec<Object>,
        at: Vec3,
        screen: Pos2,
    ) {
        if plate != self.plates.current {
            self.select_plate(plate);
        }
        let Some(k) = self
            .plates
            .current()
            .objects
            .iter()
            .position(|o| o.id == object)
        else {
            return;
        };
        if self.plate_locked() {
            return;
        }
        self.record("Add part from library");
        let o = &self.plates.current().objects[k];
        let inv = o.transform().inverse();
        let local_at = inv.transform_point3(at);
        let linear = Mat4::from_mat3(Mat3::from_mat4(inv));
        // Several files dropped together keep their layout around `at`.
        let (lo, hi) = objects.iter().fold(
            (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)),
            |(lo, hi), o| {
                let (a, b) = o.bounds();
                (lo.min(a), hi.max(b))
            },
        );
        let centre = (lo + hi) * 0.5;
        let mut volumes = vec![];
        for d in &objects {
            let placed = d.print_mesh();
            let (a, b) = placed.bounds();
            let own = (Vec3::from(a) + Vec3::from(b)) * 0.5;
            // Into the object's frame, centred on the part's own middle.
            let mesh = scene::transform_mesh(&placed, linear * Mat4::from_translation(-own));
            let mut v = Volume::new(stem(&d.name), PartKind::Model, mesh);
            v.position = local_at + linear.transform_vector3(own - centre);
            volumes.push(v);
        }
        let ids: Vec<u64> = volumes.iter().map(|v| v.id).collect();
        self.plates.current_mut().objects[k].volumes.extend(volumes);
        self.sync_instances(k);
        self.select(Some(k), false);
        self.sub = ids.first().map(|&id| Sub::Volume(id));
        self.editor.object_mode = true;
        self.changed();
        self.status = format!(
            "Added {} as a part of {}",
            objects
                .iter()
                .map(|o| stem(&o.name))
                .collect::<Vec<_>>()
                .join(", "),
            stem(&self.plates.current().objects[k].name)
        );
        self.library.popover = Some(PartPopover {
            plate: self.plates.current,
            object,
            volumes: ids,
            at: screen,
            kind: PartKind::Model,
        });
    }

    /// Changes the kind of the parts the last drop added (no new undo step: the drop and its
    /// kind are one).
    pub(crate) fn set_dropped_part_kind(&mut self, kind: PartKind) {
        let Some(pop) = self.library.popover.clone() else {
            return;
        };
        let Some(k) = self
            .plates
            .plate(pop.plate.min(self.plates.len() - 1))
            .objects
            .iter()
            .position(|o| o.id == pop.object)
        else {
            self.library.popover = None;
            return;
        };
        if pop.plate != self.plates.current {
            self.library.popover = None;
            return;
        }
        let o = &mut self.plates.current_mut().objects[k];
        for v in o.volumes.iter_mut().filter(|v| pop.volumes.contains(&v.id)) {
            v.kind = kind.clone();
            v.overrides.clear();
            if kind == PartKind::Modifier {
                // A modifier without settings does nothing; start with dense infill.
                v.overrides.insert("fill_density".into(), "100%".into());
            }
        }
        self.sync_instances(k);
        self.changed();
        if let Some(p) = &mut self.library.popover {
            p.kind = kind;
        }
    }

    /// The small choice of part kind next to where the parts were dropped.
    fn part_popover(&mut self, ctx: &egui::Context) {
        let Some(pop) = self.library.popover.clone() else {
            return;
        };
        // Gone with an undo, another plate or another selection.
        let alive = pop.plate == self.plates.current
            && self.plates.current().objects.iter().any(|o| {
                o.id == pop.object && o.volumes.iter().any(|v| pop.volumes.contains(&v.id))
            });
        if !alive || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.library.popover = None;
            return;
        }
        let mut chosen = None;
        let mut done = false;
        let area = egui::Area::new(egui::Id::unique("lib-part-kind"))
            .order(egui::Order::Foreground)
            .fixed_pos(pop.at + Vec2::new(14., 10.))
            .constrain(true)
            .show(ctx, |ui| {
                let p = theme::of(ui);
                egui::Frame::new()
                    .fill(p.panel)
                    .stroke(Stroke::new(1., p.border_soft))
                    .corner_radius(10.)
                    .inner_margin(egui::Margin::same(10))
                    .shadow(egui::Shadow {
                        offset: [0, 4],
                        blur: 16,
                        spread: 0,
                        color: egui::Color32::from_black_alpha(90),
                    })
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing = Vec2::new(4., 6.);
                        ui.label(
                            egui::RichText::new("Add as")
                                .font(theme::medium(12.))
                                .color(p.muted),
                        );
                        ui.horizontal(|ui| {
                            for (kind, label, tip) in part_kinds() {
                                let on = kind == pop.kind;
                                let r =
                                    crate::panels::widgets::badge(ui, label, on).on_hover_text(tip);
                                if r.clicked() && !on {
                                    chosen = Some(kind);
                                }
                            }
                        });
                        if crate::panels::widgets::link(ui, "Done").clicked() {
                            done = true;
                        }
                    });
            });
        if let Some(kind) = chosen {
            self.set_dropped_part_kind(kind);
        }
        if done || area.response.clicked_elsewhere() {
            self.library.popover = None;
        }
    }
}
