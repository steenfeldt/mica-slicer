// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/slic3r/GUI/Gizmos/GLGizmoEmboss.cpp :: GLGizmoEmboss::draw_window
// Source: prusaslicer@b028299c77 src/slic3r/GUI/Gizmos/GLGizmoEmboss.cpp :: GLGizmoEmboss::draw_font_list
// Source: prusaslicer@b028299c77 src/slic3r/GUI/Gizmos/GLGizmoEmboss.cpp :: GLGizmoEmboss::draw_model_type
// Source: prusaslicer@b028299c77 src/slic3r/GUI/Gizmos/GLGizmoEmboss.cpp :: GLGizmoEmboss::draw_advanced
// Source: prusaslicer@b028299c77 src/slic3r/GUI/Gizmos/GLGizmoSVG.cpp :: GLGizmoSVG::draw_window
// Source: prusaslicer@b028299c77 src/slic3r/GUI/Gizmos/GLGizmoSVG.cpp :: GLGizmoSVG::draw_mirroring
// Source: prusaslicer@b028299c77 src/slic3r/GUI/SurfaceDrag.cpp :: on_mouse_surface_drag
// Changes: One Mica tool card for text and SVG with plain-language labels (font, size, depth, letter / line spacing in mm, bold / italic only where the family has them, alignment, raised / engraved / modifier / object, wrap onto the surface, SVG size, mirror and curve tolerance); clicking the model places the shape on the clicked surface along its normal instead of PrusaSlicer's drag; edits rebuild the mesh at once (wrapped text in the background) and are undoable; Done keeps the edits, Cancel / Esc puts the text back as it was when the card opened; the card reopens from the object list, a double-click and "Edit text".
// Upstream attribution (GLGizmoEmboss.cpp, GLGizmoSVG.cpp, SurfaceDrag.cpp):
// Copyright (c) Prusa Research 2021 - 2023 Filip Sykala @Jony01, Oleksandra Iushchenko @YuSanka, Lukáš Matěna @lukasmatena, Vojtěch Bubník @bubnikv, Enrico Turri @enricoturri1966
//
// PrusaSlicer is released under the terms of the AGPLv3 or higher

//! The tool card and the editor operations behind it.

use super::{Emboss, EmbossTool, Kind, Original, fonts};
use crate::app::App;
use crate::panels::widgets::{self, NumberSpec};
use crate::scene::{self, Object, Sub, Volume};
use crate::theme;
use eframe::egui::{self, Align2, Pos2, Rect, Sense, Vec2};
use glam::{Mat4, Vec3};
use mica_core::emboss::{EmbossConfig, EmbossSource, HAlign, SvgSource, TextSource, VAlign};
use mica_core::mesh::TriangleMesh;
use std::sync::Arc;

/// An object's mesh with its transform into a text frame.
type Surface = (Arc<TriangleMesh>, mica_core::threemf::Transform);
/// The edited text's object index and volume, source, kind and surface.
type Inputs = (usize, Option<u64>, Arc<Emboss>, Kind, Option<Surface>);

const CARD_W: f32 = 320.;
const LABEL_W: f32 = 92.;
const FIELD_H: f32 = 26.;

/// What a card interaction asks for; applied after the card is drawn.
enum Edit {
    Config(Box<dyn FnOnce(&mut EmbossConfig)>, bool),
    /// A keystroke in the text: one undo step for the whole typing (until the field is left).
    Typing(Box<dyn FnOnce(&mut EmbossConfig)>),
    Kind(Kind),
    Svg,
    Delete,
    /// Done: keep the edits.
    Close,
    /// Cancel: put the text back as it was when the card opened.
    Cancel,
}

impl App {
    /// The edited object's index on the current plate (`None` when it is gone).
    pub(crate) fn emboss_target(&self) -> Option<(usize, Option<u64>)> {
        let tool = self.emboss.as_ref()?;
        let i = self
            .plates
            .current()
            .objects
            .iter()
            .position(|o| o.id == tool.object_id)?;
        let o = &self.plates.current().objects[i];
        match tool.volume {
            Some(id) => o
                .volume(id)
                .filter(|v| v.emboss.is_some())
                .map(|_| (i, Some(id))),
            None => o.emboss.is_some().then_some((i, None)),
        }
    }

    fn target_emboss(&self) -> Option<(Arc<Emboss>, Kind)> {
        let (i, vol) = self.emboss_target()?;
        let o = &self.plates.current().objects[i];
        match vol {
            Some(id) => {
                let v = o.volume(id)?;
                Some((v.emboss.clone()?, Kind::of(&v.kind)))
            }
            None => Some((o.emboss.clone()?, Kind::Object)),
        }
    }

    /// T / the toolbar button: closes the card, or opens it on the selected text (volume or
    /// object), or adds a new text to the selected object (on the surface facing the camera),
    /// or a text object on the bed when nothing is selected.
    pub(crate) fn toggle_emboss(&mut self) {
        // A card whose text is gone (undone, deleted) counts as closed.
        let open = self.emboss_target().is_some();
        if self.emboss.is_some() {
            if open {
                self.close_emboss();
                return;
            }
            self.emboss = None;
        }
        if self.loading.is_some() || self.plate_locked() {
            return;
        }
        // With nothing selected the plate's only model is the one (as for the other tools);
        // with none or several, the text is an object of its own.
        if self.selected.is_none() && self.plates.current().objects.len() == 1 {
            self.select(Some(0), false);
        }
        let Some(i) = self.selected else {
            fonts::start_scan();
            self.cut = None;
            self.add_text_object();
            return;
        };
        if self.edit_text(i, self.sub) {
            return;
        }
        fonts::start_scan();
        self.cut = None;
        self.add_text_volume(i, Kind::Raised);
    }

    /// Opens the card on the text of object `i` that `sub` names (a text part), or on the
    /// object itself when it is a text object (double-click, "Edit text", the object list).
    /// Returns whether there was a text to open; a card open on another text is done first.
    pub(crate) fn edit_text(&mut self, i: usize, sub: Option<Sub>) -> bool {
        let Some(o) = self.plates.current().objects.get(i) else {
            return false;
        };
        let volume = match sub {
            Some(Sub::Volume(id)) if o.volume(id).is_some_and(|v| v.emboss.is_some()) => Some(id),
            Some(Sub::Volume(_) | Sub::Range(_)) => return false,
            None if o.emboss.is_some() => None,
            None => return false,
        };
        let object_id = o.id;
        if self.loading.is_some() {
            return false;
        }
        if self
            .emboss
            .as_ref()
            .is_some_and(|t| t.object_id == object_id && t.volume == volume)
            && self.emboss_target().is_some()
        {
            return true;
        }
        if self.emboss.is_some() {
            self.close_emboss();
        }
        fonts::start_scan();
        self.cut = None;
        self.open_emboss(object_id, volume, false);
        true
    }

    /// Opens the card on a text; `added`: the card just made it (Cancel removes it again).
    fn open_emboss(&mut self, object_id: u64, volume: Option<u64>, added: bool) {
        // One tool card at a time: the mesh tools close.
        self.mesh_tool = None;
        let original = (!added)
            .then(|| {
                let o = self
                    .plates
                    .current()
                    .objects
                    .iter()
                    .find(|o| o.id == object_id)?;
                Some(match volume {
                    Some(id) => Original::Volume(Box::new(o.volume(id)?.clone())),
                    None => Original::Object(Box::new(o.clone())),
                })
            })
            .flatten();
        self.emboss = Some(EmbossTool::new(object_id, volume, original));
        if let Some(i) = self
            .plates
            .current()
            .objects
            .iter()
            .position(|o| o.id == object_id)
        {
            if self.selected != Some(i) {
                self.select(Some(i), false);
            }
            self.sub = volume.map(Sub::Volume);
            self.editor.object_mode = volume.is_some();
        }
        self.status = if volume.is_some() {
            "Text: type in the card · click the model to move the text there · Done keeps it · Esc cancels"
        } else {
            "Text: type in the card · Done keeps it · Esc cancels"
        }
        .into();
    }

    /// Done: the edits stay (a rebuild still running is waited for) and the card closes.
    pub(crate) fn close_emboss(&mut self) {
        self.finish_emboss_rebuild();
        self.emboss = None;
    }

    /// Cancel (Esc): puts the text back as it was when the card opened (one undo step), or
    /// takes away the text the card added, and closes the card.
    pub(crate) fn cancel_emboss(&mut self) {
        self.end_gesture();
        let Some(tool) = self.emboss.clone() else {
            return;
        };
        let Some((i, vol)) = self.emboss_target() else {
            self.emboss = None;
            return;
        };
        if self.plate_locked() {
            self.emboss = None;
            return;
        }
        match tool.original {
            None => {
                self.delete_emboss();
                self.status = "Text canceled".into();
                return;
            }
            Some(Original::Volume(v)) => {
                let o = &self.plates.current().objects[i];
                if vol == Some(v.id) && o.volume(v.id) != Some(&*v) {
                    self.record("Cancel text edits");
                    if let Some(slot) = self.plates.current_mut().objects[i].volume_mut(v.id) {
                        *slot = *v;
                    }
                    self.sync_instances(i);
                    self.changed();
                }
            }
            Some(Original::Object(o)) => {
                if vol.is_none() && self.plates.current().objects[i] != *o {
                    self.record("Cancel text edits");
                    self.plates.current_mut().objects[i] = *o;
                    self.sync_instances(i);
                    self.changed();
                }
            }
        }
        self.emboss = None;
        self.status = "Text edits canceled".into();
    }

    /// Where the view looks at object `i`: the surface point under the view's center, else
    /// the one toward the object's center, with its outward normal (world); else the top of
    /// its box.
    fn facing_point(&self, i: usize) -> (Vec3, Vec3) {
        let o = &self.plates.current().objects[i];
        let eye = self.viewport.camera.eye();
        let pick = |dir: Vec3| {
            scene::picking::pick_mesh(&o.mesh, o.local_bounds, o.transform(), eye, dir)
                .map(|hit| (dir, hit))
        };
        let center = (self.viewport.camera.target - eye).normalize_or_zero();
        let toward = (o.pivot_world() - eye).normalize_or_zero();
        if let Some((dir, (face, d))) = pick(center).or_else(|| pick(toward)) {
            let t = o.transform();
            let [a, b, c] = o.mesh.faces[face]
                .map(|k| t.transform_point3(Vec3::from(o.mesh.vertices[k as usize])));
            let mut n = (b - a).cross(c - a).normalize_or_zero();
            if t.determinant() < 0. {
                n = -n;
            }
            return (eye + dir * d, n);
        }
        let (min, max) = o.bounds();
        (
            Vec3::new((min.x + max.x) / 2., (min.y + max.y) / 2., max.z),
            Vec3::Z,
        )
    }

    /// A new text volume of `kind` on object `i`, placed on the surface facing the camera.
    pub(crate) fn add_text_volume(&mut self, i: usize, kind: Kind) {
        let n = self.plates.current().objects[i]
            .volumes
            .iter()
            .filter(|v| v.emboss.is_some())
            .count()
            + 1;
        let config = self.last_text_style();
        self.add_emboss_volume(i, kind, config, format!("Text {n}"));
    }

    /// "Add text": a raised text on the selected object, else a text object on the bed. A
    /// card open on another text is done first.
    pub(crate) fn add_text(&mut self) {
        if self.loading.is_some() || self.plate_locked() {
            return;
        }
        if self.emboss.is_some() {
            self.close_emboss();
        }
        fonts::start_scan();
        self.cut = None;
        match self
            .selected
            .filter(|&i| i < self.plates.current().objects.len())
        {
            Some(i) => self.add_text_volume(i, Kind::Raised),
            None => self.add_text_object(),
        }
    }

    /// "Add SVG…": a file dialog, then the file's shapes raised on the selected object.
    pub(crate) fn pick_svg_for_selection(&mut self) {
        let Some(i) = self
            .selected
            .filter(|&i| i < self.plates.current().objects.len())
        else {
            self.status = "Select a model first, then add the SVG to it".into();
            return;
        };
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("SVG", &["svg"])
            .set_title("Choose an SVG file")
            .pick_file()
        {
            self.add_svg_volume(i, &path);
        }
    }

    /// An SVG file's shapes as a raised part on object `i` (one undo step), with the card
    /// open on it.
    pub(crate) fn add_svg_volume(&mut self, i: usize, path: &std::path::Path) {
        if self.loading.is_some() || self.plate_locked() {
            return;
        }
        let data = match std::fs::read_to_string(path) {
            Ok(d) => d,
            Err(e) => {
                self.notify_error(
                    "The SVG file could not be read",
                    format!("Cannot read {}: {e}", path.display()),
                );
                return;
            }
        };
        let path = path.to_string_lossy();
        let (src, stem) = match svg_source(&path, data, 0) {
            Ok(s) => (s, svg_stem(&path)),
            Err(e) => {
                self.notify_error("The SVG file has no usable shapes", e);
                return;
            }
        };
        if self.emboss.is_some() {
            self.close_emboss();
        }
        self.cut = None;
        let config = EmbossConfig {
            source: EmbossSource::Svg(src),
            depth: 1.,
            use_surface: false,
        };
        self.add_emboss_volume(i, Kind::Raised, config, format!("{stem}.svg"));
    }

    /// A new embossed volume of `kind` from `config` on object `i`, placed on the surface
    /// facing the view (one undo step), with the card open on it.
    fn add_emboss_volume(&mut self, i: usize, kind: Kind, mut config: EmbossConfig, name: String) {
        let (point, normal) = self.facing_point(i);
        self.record(if config.svg().is_some() {
            "Add SVG"
        } else {
            "Add text"
        });
        let mut v = Volume::new(name, kind.part_kind(), TriangleMesh::default());
        // An SVG's copy in the project is named after its volume.
        if let EmbossSource::Svg(s) = &mut config.source {
            s.path_in_3mf = format!("3D/{}_{}.svg", svg_stem(&s.path), v.id);
        }
        v.emboss = Some(Arc::new(Emboss {
            config,
            frame: Mat4::IDENTITY,
        }));
        if kind == Kind::Modifier {
            v.overrides.insert("fill_density".into(), "100%".into());
        }
        let id = v.id;
        let object_id = {
            let o = &mut self.plates.current_mut().objects[i];
            o.volumes.push(v);
            o.id
        };
        self.place_volume(i, id, point, normal);
        self.open_emboss(object_id, Some(id), true);
        self.rebuild_emboss();
        self.sync_instances(i);
        self.changed();
    }

    /// A text object lying on the bed (nothing was selected).
    pub(crate) fn add_text_object(&mut self) {
        self.record("Add text object");
        let config = self.last_text_style();
        let mesh = match super::build(&config, Kind::Object, &[]) {
            Ok((m, _)) => m,
            Err(e) => {
                self.notify_error("The text was not created", e);
                return;
            }
        };
        let mut o = self.plain_object(self.unique_name("Text"), mesh);
        o.emboss = Some(Arc::new(Emboss {
            config,
            frame: Mat4::IDENTITY,
        }));
        let (a, b) = o.bounds();
        let size = (b - a).truncate();
        let spot = scene::free_spot(
            &self.plates.current().objects,
            size,
            self.bed.center().truncate() - size * 0.5,
            &self.bed,
            6.,
        );
        o.position += (spot - a.truncate()).extend(0.);
        o.drop_to_bed();
        let object_id = o.id;
        self.plates.current_mut().objects.push(o);
        let i = self.plates.current().objects.len() - 1;
        self.select(Some(i), false);
        self.open_emboss(object_id, None, true);
        self.changed();
    }

    /// The style of the text edited last (so a second text matches the first), else the
    /// default.
    fn last_text_style(&self) -> EmbossConfig {
        let mut base = super::default_text();
        let last = self
            .plates
            .current()
            .objects
            .iter()
            .flat_map(|o| {
                o.volumes
                    .iter()
                    .filter_map(|v| v.emboss.clone())
                    .chain(o.emboss.clone())
            })
            .rfind(|e| e.config.text().is_some());
        if let Some(e) = last {
            base.depth = e.config.depth;
            base.use_surface = e.config.use_surface;
            if let (EmbossSource::Text(t), Some(prev)) = (&mut base.source, e.config.text()) {
                let text = std::mem::take(&mut t.text);
                *t = prev.clone();
                t.text = text;
            }
        }
        base
    }

    /// Moves volume `id` of object `i` so its text frame sits on the world `point` with the
    /// outward `normal` (the mesh is rebuilt by the caller).
    fn place_volume(&mut self, i: usize, id: u64, point: Vec3, normal: Vec3) {
        let o = &mut self.plates.current_mut().objects[i];
        let t = o.transform();
        let inv = t.inverse();
        let p = inv.transform_point3(point);
        // Normals transform with the inverse transpose.
        let n = t.transpose().transform_vector3(normal).normalize_or_zero();
        let n = if n == Vec3::ZERO { Vec3::Z } else { n };
        let frame = super::frame_on_surface(p, n);
        let (_, rotation, position) = frame.to_scale_rotation_translation();
        if let Some(v) = o.volume_mut(id) {
            v.position = position;
            v.rotation = rotation;
            v.scale = Vec3::ONE;
            if let Some(e) = &v.emboss {
                v.emboss = Some(Arc::new(Emboss {
                    config: e.config.clone(),
                    frame: Mat4::IDENTITY,
                }));
            }
        }
    }

    /// A click on the model while the card is open: the text moves there.
    pub(crate) fn emboss_place(&mut self, index: usize, point: Vec3, normal: Vec3) {
        let Some((i, vol)) = self.emboss_target() else {
            return;
        };
        let Some(id) = vol else {
            self.status = "A text object stands on its own; move it with the gizmo".into();
            return;
        };
        if index != i {
            self.status = "Click the text's own object to move it there".into();
            return;
        }
        if self.plate_locked() {
            return;
        }
        self.finish_emboss_rebuild();
        self.record("Place text");
        self.place_volume(i, id, point, normal);
        self.rebuild_emboss();
        self.sync_instances(i);
        self.changed();
    }

    /// The edited text's source, kind and surface (the object's mesh in the text frame).
    fn emboss_inputs(&self) -> Option<Inputs> {
        let (i, vol) = self.emboss_target()?;
        let o = &self.plates.current().objects[i];
        Some(match vol {
            Some(id) => {
                let v = o.volume(id)?;
                let e = v.emboss.clone()?;
                let to_text = (v.local_transform() * e.frame).inverse();
                let surface = (o.mesh.clone(), scene::to_threemf(to_text));
                (i, vol, e, Kind::of(&v.kind), Some(surface))
            }
            None => (i, vol, o.emboss.clone()?, Kind::Object, None),
        })
    }

    /// Rebuilds the edited mesh from its config (wrapped on the object's surface when asked).
    /// Flat text takes well under a millisecond and is rebuilt at once; wrapping onto a
    /// detailed surface can take a good part of a second, so it runs in the background (one
    /// rebuild at a time; `poll_emboss` applies it and starts the next for newer edits).
    pub(crate) fn rebuild_emboss(&mut self) {
        let Some((i, vol, source, kind, surface)) = self.emboss_inputs() else {
            return;
        };
        if source.config.use_surface && surface.is_some() {
            if let Some(t) = &mut self.emboss
                && t.rebuild.is_none()
            {
                t.rebuild = Some(super::Rebuild::spawn(source, kind, surface));
            }
            return;
        }
        let result = super::build_in_frame(&source, kind, surface.as_ref());
        self.apply_built(i, vol, result);
    }

    /// Applies a finished background rebuild when it matches the text as it is now, else
    /// starts one for the newer text. Returns whether a rebuild is still running.
    pub(crate) fn poll_emboss(&mut self) -> bool {
        let Some(job) = self.emboss.as_ref().and_then(|t| t.rebuild.clone()) else {
            return false;
        };
        let Some(result) = job.take() else {
            return true;
        };
        if let Some(t) = &mut self.emboss {
            t.rebuild = None;
        }
        let Some((i, vol, source, ..)) = self.emboss_inputs() else {
            return false;
        };
        if Arc::ptr_eq(&source, &job.source) || *source == *job.source {
            self.apply_built(i, vol, result);
            self.sync_instances(i);
            self.changed();
        } else {
            self.rebuild_emboss();
        }
        self.emboss.as_ref().is_some_and(|t| t.rebuild.is_some())
    }

    /// Waits for a background rebuild (and those it starts) so the mesh matches the text:
    /// before an undo step is recorded, and when the card closes.
    pub(crate) fn finish_emboss_rebuild(&mut self) {
        while self.poll_emboss() {
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }

    /// Puts a built mesh on the edited volume (or text object and its instances).
    fn apply_built(&mut self, i: usize, vol: Option<u64>, result: super::Built) {
        match result {
            Ok((mesh, warnings)) => {
                if let Some(t) = &mut self.emboss {
                    t.error = None;
                    t.warnings = warnings;
                }
                let mesh = Arc::new(mesh);
                let bounds = mesh.bounds();
                match vol {
                    Some(id) => {
                        if let Some(v) = self.plates.current_mut().objects[i].volume_mut(id) {
                            v.mesh = mesh;
                            v.local_bounds = bounds;
                        }
                    }
                    None => {
                        let source = self.plates.current().objects[i].emboss.clone();
                        for k in self.instances_of(i).into_iter().chain([i]) {
                            let o = &mut self.plates.current_mut().objects[k];
                            o.emboss = source.clone();
                            let anchor = |o: &Object| {
                                let f = o.emboss.as_ref().map_or(Mat4::IDENTITY, |e| e.frame);
                                o.transform().transform_point3(f.w_axis.truncate())
                            };
                            let before = anchor(o);
                            o.mesh = mesh.clone();
                            o.local_bounds = bounds;
                            o.painting = Default::default();
                            let after = anchor(o);
                            o.position += before - after;
                            o.drop_to_bed();
                        }
                    }
                }
            }
            Err(e) => {
                if let Some(t) = &mut self.emboss {
                    t.error = Some(e);
                }
            }
        }
    }

    /// Applies an edit of the config (undoable) and rebuilds.
    pub(crate) fn edit_emboss(&mut self, f: impl FnOnce(&mut EmbossConfig), gesture: bool) {
        let Some((i, vol)) = self.emboss_target() else {
            return;
        };
        if self.plate_locked() {
            return;
        }
        // The undo step holds the mesh that matches the text before this edit.
        if !self.undo.in_gesture() {
            self.finish_emboss_rebuild();
        }
        if gesture {
            self.begin_gesture("Edit text");
        } else {
            self.record("Edit text");
        }
        self.apply_emboss(i, vol, f);
    }

    /// Applies an edit of the config without an undo step and rebuilds.
    fn apply_emboss(&mut self, i: usize, vol: Option<u64>, f: impl FnOnce(&mut EmbossConfig)) {
        let o = &mut self.plates.current_mut().objects[i];
        let slot = match vol {
            Some(id) => o.volume_mut(id).and_then(|v| v.emboss.as_mut()),
            None => o.emboss.as_mut(),
        };
        if let Some(e) = slot {
            let mut next = (**e).clone();
            f(&mut next.config);
            // Gaps are kept in font units: follow the font's units per em.
            if let (Some(u), EmbossSource::Text(t)) =
                (super::units_per_em(&next.config), &mut next.config.source)
            {
                t.units_per_em = u;
            }
            *e = Arc::new(next);
        }
        self.rebuild_emboss();
        self.sync_instances(i);
        self.changed();
    }

    pub(crate) fn set_emboss_kind(&mut self, kind: Kind) {
        let Some((i, Some(id))) = self.emboss_target() else {
            return;
        };
        if self.plate_locked() {
            return;
        }
        self.finish_emboss_rebuild();
        self.record("Change text type");
        if let Some(v) = self.plates.current_mut().objects[i].volume_mut(id) {
            v.kind = kind.part_kind();
            if kind == Kind::Modifier && v.overrides.is_empty() {
                v.overrides.insert("fill_density".into(), "100%".into());
            }
            if kind != Kind::Modifier {
                v.overrides.clear();
            }
        }
        self.rebuild_emboss();
        self.sync_instances(i);
        self.changed();
    }

    /// Replaces the edited source with an SVG file's fills.
    pub(crate) fn emboss_svg_file(&mut self, path: &std::path::Path) {
        match std::fs::read_to_string(path) {
            Ok(data) => self.emboss_svg_data(&path.to_string_lossy(), data),
            Err(e) => self.notify_error(
                "The SVG file could not be read",
                format!("Cannot read {}: {e}", path.display()),
            ),
        }
    }

    /// Replaces the edited source with SVG text (`path` names it).
    pub(crate) fn emboss_svg_data(&mut self, path: &str, data: String) {
        let Some((_, vol)) = self.emboss_target() else {
            return;
        };
        let stem = svg_stem(path);
        let src = match svg_source(path, data, vol.unwrap_or(0)) {
            Ok(s) => s,
            Err(e) => {
                self.notify_error("The SVG file has no usable shapes", e);
                return;
            }
        };
        self.edit_emboss(move |c| c.source = EmbossSource::Svg(src), false);
        // A default "Text n" name becomes the file's name.
        if let Some((i, vol)) = self.emboss_target() {
            let o = &mut self.plates.current_mut().objects[i];
            let name = match vol {
                Some(id) => o.volume_mut(id).map(|v| &mut v.name),
                None => Some(&mut o.name),
            };
            if let Some(n) = name
                && n.starts_with("Text")
            {
                *n = format!("{stem}.svg");
            }
            self.sync_instances(i);
        }
    }

    /// The card at the top right of the 3D view.
    pub(crate) fn emboss_window(&mut self, ctx: &egui::Context) {
        // A card whose text is gone (undone, deleted) closes.
        if self.target_emboss().is_none() {
            self.emboss = None;
            return;
        }
        // Selecting something else leaves the tool (the edits stay); selecting another text
        // of the object (the object list, a click) moves the card to it.
        let Some((i, vol)) = self.emboss_target() else {
            return;
        };
        if Some(i) != self.selected {
            self.close_emboss();
            return;
        }
        if self.sub != vol.map(Sub::Volume) {
            let sub = self.sub;
            if !(sub.is_some() && self.edit_text(i, sub)) {
                self.close_emboss();
            }
            return;
        }
        // A background rebuild: its mesh lands when done.
        if self.poll_emboss() {
            ctx.request_repaint_after(std::time::Duration::from_millis(30));
        }
        let Some(tool) = self.emboss.clone() else {
            return;
        };
        let Some((emboss, kind)) = self.target_emboss() else {
            return;
        };
        let lib = fonts::library();
        let mut edits: Vec<Edit> = Vec::new();
        let mut filter = tool.font_filter.clone();
        let viewport = self.viewport_rect;
        let title = match &emboss.config.source {
            EmbossSource::Text(_) => "Text",
            EmbossSource::Svg(_) => "SVG",
        };
        let mut typing_done = false;
        egui::Area::new(egui::Id::unique("emboss-card"))
            .order(egui::Order::Middle)
            .fixed_pos(viewport.right_top() + egui::vec2(-14. - CARD_W, 14.))
            .show(ctx, |ui| {
                let p = theme::of(ui);
                theme::overlay_frame(ui)
                    .inner_margin(egui::Margin::symmetric(12, 10))
                    .show(ui, |ui| {
                        ui.set_width(CARD_W - 24.);
                        ui.spacing_mut().item_spacing = Vec2::new(4., 4.);
                        ui.label(
                            egui::RichText::new(widgets::spaced_upper(&format!(
                                "{title} · {}",
                                kind.label()
                            )))
                            .font(theme::semibold(11.))
                            .color(p.muted),
                        );
                        ui.add_space(4.);
                        // Source: text or an SVG file.
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 6.;
                            let is_text = emboss.config.text().is_some();
                            if widgets::chip(ui, None, "Text", is_text, true).clicked() && !is_text
                            {
                                edits.push(Edit::Config(
                                    Box::new(|c| {
                                        if let EmbossSource::Text(t) = super::default_text().source
                                        {
                                            c.source = EmbossSource::Text(t);
                                        }
                                    }),
                                    false,
                                ));
                            }
                            if widgets::chip(
                                ui,
                                None,
                                if is_text {
                                    "SVG file…"
                                } else {
                                    "Other SVG…"
                                },
                                !is_text,
                                true,
                            )
                            .on_hover_text("Import an SVG file's filled shapes")
                            .clicked()
                            {
                                edits.push(Edit::Svg);
                            }
                        });
                        ui.add_space(2.);
                        match &emboss.config.source {
                            EmbossSource::Text(t) => {
                                text_rows(ui, t, &lib, &mut filter, &mut edits, &mut typing_done)
                            }
                            EmbossSource::Svg(s) => svg_rows(ui, s, &mut edits),
                        }
                        // Depth and placement.
                        let depth_label = if kind.outside() { "Thickness" } else { "Depth" };
                        if let Some(v) = number_row(
                            ui,
                            depth_label,
                            "emb-depth",
                            emboss.config.depth,
                            "mm",
                            2,
                            0.05,
                            100.,
                            0.02,
                        ) {
                            edits.push(Edit::Config(Box::new(move |c| c.depth = v), true));
                        }
                        ui.add_space(4.);
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 6.;
                            label(ui, "Type");
                            if kind == Kind::Object {
                                widgets::chip(ui, None, "Separate object", true, false);
                            } else {
                                for k in [Kind::Raised, Kind::Engraved, Kind::Modifier] {
                                    let tip = match k {
                                        Kind::Raised => "Added on the surface",
                                        Kind::Engraved => "Cut into the surface (negative volume)",
                                        _ => "A modifier: its own settings inside the shape",
                                    };
                                    if widgets::chip(ui, None, k.label(), kind == k, true)
                                        .on_hover_text(tip)
                                        .clicked()
                                        && kind != k
                                    {
                                        edits.push(Edit::Kind(k));
                                    }
                                }
                            }
                        });
                        if kind != Kind::Object {
                            ui.add_space(2.);
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 8.;
                                let (r, _) =
                                    ui.allocate_exact_size(Vec2::new(34., 20.), Sense::hover());
                                if let Some(v) = widgets::toggle_at(
                                    ui,
                                    ui.make_persistent_id("emb-surface"),
                                    r,
                                    emboss.config.use_surface,
                                    true,
                                ) {
                                    edits.push(Edit::Config(
                                        Box::new(move |c| c.use_surface = v),
                                        false,
                                    ));
                                }
                                ui.label(
                                    egui::RichText::new("Wrap onto the surface")
                                        .font(theme::sans(12.))
                                        .color(p.label),
                                )
                                .on_hover_text("Follow a curved surface instead of staying flat");
                            });
                        }
                        if let Some(e) = &tool.error {
                            ui.add_space(2.);
                            ui.label(
                                egui::RichText::new(e)
                                    .font(theme::sans(12.))
                                    .color(p.warn_text),
                            );
                        }
                        for w in &tool.warnings {
                            ui.label(
                                egui::RichText::new(w)
                                    .font(theme::sans(11.5))
                                    .color(p.warn_text),
                            );
                        }
                        if tool.rebuild.is_some() {
                            ui.label(
                                egui::RichText::new("Wrapping the text onto the surface…")
                                    .font(theme::sans(11.5))
                                    .color(p.muted),
                            );
                        }
                        ui.add_space(6.);
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 8.;
                            if widgets::primary_button(ui, "Done", true)
                                .on_hover_text("Keep the text as it is now (T)")
                                .clicked()
                            {
                                edits.push(Edit::Close);
                            }
                            let cancel = if tool.original.is_some() {
                                "Put the text back as it was when the card opened (Esc)"
                            } else {
                                "Take the new text away again (Esc)"
                            };
                            if widgets::styled_button(ui, "Cancel", true, false, 28.)
                                .on_hover_text(cancel)
                                .clicked()
                            {
                                edits.push(Edit::Cancel);
                            }
                            if widgets::styled_button(ui, "Delete", true, false, 28.).clicked() {
                                edits.push(Edit::Delete);
                            }
                        });
                        ui.label(
                            egui::RichText::new(if kind == Kind::Object {
                                "Changes show as you type · Esc cancels"
                            } else {
                                "Click the model to move the text there · Esc cancels"
                            })
                            .font(theme::sans(11.))
                            .color(p.muted),
                        );
                    });
            });
        if let Some(t) = &mut self.emboss {
            t.font_filter = filter;
        }
        if typing_done && let Some(t) = &mut self.emboss {
            t.typing = false;
        }
        for e in edits {
            match e {
                Edit::Config(f, gesture) => self.edit_emboss(f, gesture),
                Edit::Typing(f) => {
                    if let Some((i, vol)) = self.emboss_target()
                        && !self.plate_locked()
                    {
                        if !self.emboss.as_ref().is_some_and(|t| t.typing) {
                            self.finish_emboss_rebuild();
                            self.record("Edit text");
                        }
                        if let Some(t) = &mut self.emboss {
                            t.typing = true;
                        }
                        self.apply_emboss(i, vol, f);
                    }
                }
                Edit::Kind(k) => self.set_emboss_kind(k),
                Edit::Svg => {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("SVG", &["svg"])
                        .set_title("Choose an SVG file")
                        .pick_file()
                    {
                        self.emboss_svg_file(&path);
                    }
                }
                Edit::Delete => self.delete_emboss(),
                Edit::Close => self.close_emboss(),
                Edit::Cancel => self.cancel_emboss(),
            }
        }
    }

    /// `--emboss KIND[+wrap]:FACE:TEXT` for screenshots: a text (or `svg=PATH`) on the selected
    /// object (`object`: on the bed), placed on a face of its box.
    pub(crate) fn emboss_script(&mut self, spec: &str) -> Result<(), String> {
        let mut it = spec.splitn(3, ':');
        let (kind, face, text) = match (it.next(), it.next(), it.next()) {
            (Some(k), Some(f), Some(t)) => (k, f, t.replace("\\n", "\n")),
            _ => return Err(format!("--emboss {spec}: expected KIND:FACE:TEXT")),
        };
        let wrap = kind.ends_with("+wrap");
        let kind = match kind.trim_end_matches("+wrap") {
            "raised" => Kind::Raised,
            "engraved" => Kind::Engraved,
            "modifier" => Kind::Modifier,
            "object" => Kind::Object,
            k => return Err(format!("--emboss: unknown kind {k}")),
        };
        self.close_emboss();
        if kind == Kind::Object {
            self.select(None, false);
            self.add_text_object();
        } else {
            let i = self
                .selected
                .ok_or("--emboss needs a selected object (--select N)")?;
            self.add_text_volume(i, kind);
            let (min, max) = self.plates.current().objects[i].bounds();
            let c = (min + max) * 0.5;
            let at = match face {
                "top" => Some((Vec3::new(c.x, c.y, max.z), Vec3::Z)),
                "front" => Some((Vec3::new(c.x, min.y, c.z), -Vec3::Y)),
                "right" => Some((Vec3::new(max.x, c.y, c.z), Vec3::X)),
                "camera" => None,
                f => return Err(format!("--emboss: unknown face {f}")),
            };
            if let Some((p, n)) = at {
                self.emboss_place(i, p, n);
            }
        }
        if let Some(path) = text.strip_prefix("svg=") {
            self.emboss_svg_file(std::path::Path::new(path));
        } else {
            self.edit_emboss(
                move |c| {
                    if let EmbossSource::Text(t) = &mut c.source {
                        t.text = text;
                    }
                },
                false,
            );
        }
        if wrap {
            self.edit_emboss(|c| c.use_surface = true, false);
        }
        self.finish_emboss_rebuild();
        if let Some(e) = self.emboss.as_ref().and_then(|t| t.error.clone()) {
            return Err(format!("--emboss {spec}: {e}"));
        }
        Ok(())
    }

    /// Deletes the edited text (its volume, or the text object) and closes the card.
    fn delete_emboss(&mut self) {
        let Some((i, vol)) = self.emboss_target() else {
            return;
        };
        self.emboss = None;
        match vol {
            Some(id) => {
                self.sub = Some(Sub::Volume(id));
                self.delete_sub();
            }
            None => {
                self.select(Some(i), false);
                self.delete_selected();
            }
        }
    }
}

/// A file name's stem made safe for a path inside a 3MF.
fn svg_stem(path: &str) -> String {
    std::path::Path::new(path)
        .file_stem()
        .map_or("shape".to_string(), |s| s.to_string_lossy().into_owned())
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// An SVG source from a file's text (`path` names it, `suffix` makes its copy in the project
/// unique), at the file's own size kept between 5 and 150 mm on its longer side.
fn svg_source(path: &str, data: String, suffix: u64) -> Result<SvgSource, String> {
    let mut src = SvgSource {
        path: path.to_string(),
        path_in_3mf: format!("3D/{}_{suffix}.svg", svg_stem(path)),
        data: Arc::from(data),
        mm_per_unit: super::svg::MM_PER_PX,
        mirror: [false, false],
        tolerance_mm: 0.1,
    };
    let size = super::svg::outline(&src)?.size_units;
    let longest = size[0].max(size[1]).max(1e-9) * src.mm_per_unit;
    if !(5. ..=150.).contains(&longest) {
        src.mm_per_unit *= longest.clamp(5., 150.) / longest;
    }
    Ok(src)
}

fn label(ui: &mut egui::Ui, text: &str) {
    let p = theme::of(ui);
    let (r, _) = ui.allocate_exact_size(Vec2::new(LABEL_W - 6., FIELD_H), Sense::hover());
    ui.painter().text(
        Pos2::new(r.left(), r.center().y),
        Align2::LEFT_CENTER,
        text,
        theme::sans(12.),
        p.muted_alt,
    );
}

/// One labelled number field; returns the new value.
#[allow(clippy::too_many_arguments)]
fn number_row(
    ui: &mut egui::Ui,
    text: &str,
    id: &str,
    value: f64,
    unit: &'static str,
    decimals: usize,
    min: f64,
    max: f64,
    speed: f64,
) -> Option<f64> {
    let p = theme::of(ui);
    let (row, _) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), FIELD_H + 2.),
        Sense::hover(),
    );
    ui.painter().text(
        Pos2::new(row.left(), row.center().y),
        Align2::LEFT_CENTER,
        text,
        theme::sans(12.),
        p.muted_alt,
    );
    let rect = Rect::from_min_size(
        Pos2::new(row.left() + LABEL_W, row.center().y - FIELD_H / 2.),
        Vec2::new(96., FIELD_H),
    );
    let spec = NumberSpec {
        unit,
        decimals,
        min,
        max,
        speed,
    };
    widgets::number_with(
        ui,
        ui.make_persistent_id(("emb-field", id)),
        rect,
        value,
        &spec,
        true,
        false,
    )
}

fn text_rows(
    ui: &mut egui::Ui,
    t: &TextSource,
    lib: &fonts::FontLibrary,
    filter: &mut String,
    edits: &mut Vec<Edit>,
    typing_done: &mut bool,
) {
    // The text itself, several lines allowed; every key rebuilds (one undo step per typing).
    let mut text = t.text.clone();
    let te = egui::TextEdit::multiline(&mut text)
        .id(ui.make_persistent_id("emb-text"))
        .font(theme::sans(14.))
        .desired_rows(2)
        .desired_width(ui.available_width())
        .hint_text("Type the text");
    let r = ui.add(te);
    if r.changed() {
        edits.push(Edit::Typing(Box::new(move |c| {
            if let EmbossSource::Text(t) = &mut c.source {
                t.text = text;
            }
        })));
    }
    if r.lost_focus() {
        *typing_done = true;
    }
    ui.add_space(2.);
    // Font family (filtered) and face.
    let face = lib.find(&t.family, &t.face, &t.file_name, t.bold, t.italic);
    ui.horizontal(|ui| {
        label(ui, "Font");
        let w = ui.available_width();
        let (rect, _) = ui.allocate_exact_size(Vec2::new(w, FIELD_H), Sense::hover());
        let needle = filter.to_lowercase();
        let items: Vec<(String, String)> = lib
            .families()
            .iter()
            .filter(|f| {
                needle.is_empty() || f.to_lowercase().contains(&needle) || **f == face.family
            })
            .map(|f| (f.clone(), f.clone()))
            .collect();
        if let Some(family) = widgets::dropdown_with(
            ui,
            ui.make_persistent_id("emb-family"),
            rect,
            &face.family,
            &face.family,
            None,
            &items,
            true,
            false,
        ) {
            let (bold, italic) = (t.bold, t.italic);
            if let Some(f) = lib.variant(&family, bold, italic) {
                let (fam, fc, file) = (f.family.clone(), f.face.clone(), f.file_name());
                let (sb, si) = lib.supports(&fam);
                edits.push(Edit::Config(
                    Box::new(move |c| {
                        if let EmbossSource::Text(t) = &mut c.source {
                            t.family = fam;
                            t.face = fc;
                            t.file_name = file;
                            t.bold &= sb;
                            t.italic &= si;
                        }
                    }),
                    false,
                ));
            }
        }
    });
    ui.horizontal(|ui| {
        label(ui, "");
        let w = ui.available_width();
        let mut f = filter.clone();
        let r = ui.add_sized(
            [w, 22.],
            egui::TextEdit::singleline(&mut f)
                .id(ui.make_persistent_id("emb-filter"))
                .font(theme::sans(12.))
                .hint_text(if lib.complete {
                    "Filter fonts…"
                } else {
                    "Filter fonts… (reading system fonts)"
                }),
        );
        if r.changed() {
            *filter = f;
        }
    });
    ui.horizontal(|ui| {
        label(ui, "Style");
        let faces: Vec<(String, String)> = lib
            .faces_of(&face.family)
            .map(|f| (f.face.clone(), f.face.clone()))
            .collect();
        let (rect, _) = ui.allocate_exact_size(Vec2::new(118., FIELD_H), Sense::hover());
        if let Some(choice) = widgets::dropdown_with(
            ui,
            ui.make_persistent_id("emb-face"),
            rect,
            &face.face,
            &face.face,
            None,
            &faces,
            faces.len() > 1,
            false,
        ) && let Some(f) = lib.faces_of(&face.family).find(|f| f.face == choice)
        {
            let (fc, file, bold, italic) =
                (f.face.clone(), f.file_name(), f.weight >= 600., f.italic);
            edits.push(Edit::Config(
                Box::new(move |c| {
                    if let EmbossSource::Text(t) = &mut c.source {
                        t.face = fc;
                        t.file_name = file;
                        t.bold = bold;
                        t.italic = italic;
                    }
                }),
                false,
            ));
        }
        let (can_bold, can_italic) = lib.supports(&face.family);
        for (name, on, can, is_bold) in [
            ("B", t.bold, can_bold, true),
            ("I", t.italic, can_italic, false),
        ] {
            let r = widgets::chip(ui, None, name, on, can).on_hover_text(match (is_bold, can) {
                (true, true) => "Bold",
                (true, false) => "This font has no bold face",
                (false, true) => "Italic",
                (false, false) => "This font has no italic face",
            });
            if r.clicked() && can {
                let family = face.family.clone();
                let (b, i) = if is_bold {
                    (!t.bold, t.italic)
                } else {
                    (t.bold, !t.italic)
                };
                if let Some(f) = lib.variant(&family, b, i) {
                    let (fc, file) = (f.face.clone(), f.file_name());
                    edits.push(Edit::Config(
                        Box::new(move |c| {
                            if let EmbossSource::Text(t) = &mut c.source {
                                t.face = fc;
                                t.file_name = file;
                                t.bold = b;
                                t.italic = i;
                            }
                        }),
                        false,
                    ));
                }
            }
        }
    });
    // Sizes in mm; PrusaSlicer keeps the gaps in font units.
    let upem = f64::from(t.units_per_em.max(1));
    let to_mm = |units: i32| f64::from(units) * t.size_mm / upem;
    if let Some(v) = number_row(ui, "Size", "emb-size", t.size_mm, "mm", 1, 0.5, 500., 0.1) {
        edits.push(Edit::Config(
            Box::new(move |c| {
                if let EmbossSource::Text(t) = &mut c.source {
                    t.size_mm = v;
                }
            }),
            true,
        ));
    }
    if let Some(v) = number_row(
        ui,
        "Letter spacing",
        "emb-char",
        to_mm(t.char_gap),
        "mm",
        2,
        -50.,
        100.,
        0.02,
    ) {
        edits.push(Edit::Config(
            Box::new(move |c| {
                if let EmbossSource::Text(t) = &mut c.source {
                    t.char_gap = (v * f64::from(t.units_per_em.max(1)) / t.size_mm).round() as i32;
                }
            }),
            true,
        ));
    }
    if t.text.contains('\n')
        && let Some(v) = number_row(
            ui,
            "Line spacing",
            "emb-line",
            to_mm(t.line_gap),
            "mm",
            2,
            -50.,
            100.,
            0.02,
        )
    {
        edits.push(Edit::Config(
            Box::new(move |c| {
                if let EmbossSource::Text(t) = &mut c.source {
                    t.line_gap = (v * f64::from(t.units_per_em.max(1)) / t.size_mm).round() as i32;
                }
            }),
            true,
        ));
    }
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.;
        label(ui, "Align");
        for (a, name) in [
            (HAlign::Left, "Left"),
            (HAlign::Center, "Center"),
            (HAlign::Right, "Right"),
        ] {
            if widgets::chip(ui, None, name, t.h_align == a, true).clicked() && t.h_align != a {
                edits.push(Edit::Config(
                    Box::new(move |c| {
                        if let EmbossSource::Text(t) = &mut c.source {
                            t.h_align = a;
                        }
                    }),
                    false,
                ));
            }
        }
    });
    if t.text.contains('\n') {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.;
            label(ui, "");
            for (a, name) in [
                (VAlign::Top, "Top"),
                (VAlign::Center, "Middle"),
                (VAlign::Bottom, "Bottom"),
            ] {
                if widgets::chip(ui, None, name, t.v_align == a, true).clicked() && t.v_align != a {
                    edits.push(Edit::Config(
                        Box::new(move |c| {
                            if let EmbossSource::Text(t) = &mut c.source {
                                t.v_align = a;
                            }
                        }),
                        false,
                    ));
                }
            }
        });
    }
}

fn svg_rows(ui: &mut egui::Ui, s: &SvgSource, edits: &mut Vec<Edit>) {
    let p = theme::of(ui);
    let name = std::path::Path::new(&s.path).file_name().map_or_else(
        || s.path_in_3mf.clone(),
        |n| n.to_string_lossy().into_owned(),
    );
    ui.label(
        egui::RichText::new(name)
            .font(theme::sans(12.))
            .color(p.label),
    );
    let size = super::svg::size_units(s);
    let (w, h) = (size[0] * s.mm_per_unit, size[1] * s.mm_per_unit);
    if let Some(v) = number_row(ui, "Width", "emb-svg-w", w, "mm", 1, 0.5, 1000., 0.1) {
        let k = v / size[0].max(1e-9);
        edits.push(Edit::Config(
            Box::new(move |c| set_svg(c, |s| s.mm_per_unit = k)),
            true,
        ));
    }
    if let Some(v) = number_row(ui, "Height", "emb-svg-h", h, "mm", 1, 0.5, 1000., 0.1) {
        let k = v / size[1].max(1e-9);
        edits.push(Edit::Config(
            Box::new(move |c| set_svg(c, |s| s.mm_per_unit = k)),
            true,
        ));
    }
    if let Some(v) = number_row(
        ui,
        "Curve detail",
        "emb-svg-tol",
        s.tolerance_mm,
        "mm",
        3,
        0.005,
        2.,
        0.002,
    ) {
        edits.push(Edit::Config(
            Box::new(move |c| set_svg(c, |s| s.tolerance_mm = v)),
            true,
        ));
    }
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.;
        label(ui, "Mirror");
        for (axis, name, tip) in [
            (0, "↔ Flip", "Mirror left to right"),
            (1, "↕ Flip", "Mirror top to bottom"),
        ] {
            if widgets::chip(ui, None, name, s.mirror[axis], true)
                .on_hover_text(tip)
                .clicked()
            {
                edits.push(Edit::Config(
                    Box::new(move |c| set_svg(c, |s| s.mirror[axis] = !s.mirror[axis])),
                    false,
                ));
            }
        }
    });
}

fn set_svg(c: &mut EmbossConfig, f: impl FnOnce(&mut SvgSource)) {
    if let EmbossSource::Svg(s) = &mut c.source {
        f(s);
    }
}
