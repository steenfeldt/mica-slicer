// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The selected object's tool card over the 3D view in Prepare (top right of the viewport,
//! so left of the sidebar when that is on the right; the object list is the sidebar's tree,
//! `tree.rs`). The tool card's content follows the active tool: Move shows the
//! position and Drop to bed; Rotate the 90° turns, Lay on face, Auto-orient, the X / Y / Z
//! angles and Reset; Scale the size, scale, mirror, Uniform and Reset; without a tool the
//! whole transform is shown. Its fields read the object every frame, so they follow a gizmo
//! drag live; a field being typed into shows its own buffer until Enter or a focus loss
//! commits it (Esc reverts), and a slider drag applies live. One typed value and one drag are
//! one undo entry each. [`Transform`] holds the field logic apart from the widgets.

use super::widgets::{self, Glyph, NumberSpec};
use crate::app::App;
use crate::scene::Object;
use crate::theme;
use eframe::egui::{self, Align2, Pos2, Rect, Sense, Stroke, Vec2};
use glam::Vec3;

const CARD_W: f32 = 272.;
const FIELD_W: f32 = 56.;
const FIELD_H: f32 = 26.;
const LABEL_W: f32 = 74.;
/// Smallest scale factor a field accepts (0.1 %).
const MIN_SCALE: f32 = 0.001;

/// The editable transform behind the tool card's fields, so what one typed or dragged value
/// does to the object can be tested without widgets. Size and scale are two views of the
/// same factors over the mesh's untransformed size (`base`), so they can never disagree.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Transform {
    pub position: Vec3,
    /// Euler angles (X, Y, Z) in degrees.
    pub rotation: Vec3,
    pub scale: Vec3,
    /// 1 or −1 per axis.
    pub mirror: Vec3,
    /// Size of the untransformed mesh.
    pub base: Vec3,
}

impl Transform {
    pub fn of(o: &Object) -> Self {
        let (bmin, bmax) = o.local_bounds;
        let e = o.euler();
        Self {
            position: o.position,
            rotation: Vec3::new(e.x.to_degrees(), e.y.to_degrees(), e.z.to_degrees()),
            scale: o.scale,
            mirror: o.mirror,
            base: Vec3::from(bmax) - Vec3::from(bmin),
        }
    }

    /// Writes the transform back. The rotation is only re-derived when an angle changed, so
    /// an unrelated edit never drifts the tilt through a Euler round trip.
    pub fn apply_to(&self, o: &mut Object) {
        o.position = self.position;
        if self.rotation != Self::of(o).rotation {
            o.set_euler(Vec3::new(
                self.rotation.x.to_radians(),
                self.rotation.y.to_radians(),
                self.rotation.z.to_radians(),
            ));
        }
        o.scale = self.scale;
        o.mirror = self.mirror;
    }

    /// Size in mm along each axis.
    pub fn size(&self) -> Vec3 {
        self.base * self.scale
    }

    pub fn scale_percent(&self) -> Vec3 {
        self.scale * 100.
    }

    pub fn set_position(&mut self, axis: usize, mm: f32) {
        self.position[axis] = mm;
    }

    pub fn set_rotation_degrees(&mut self, axis: usize, degrees: f32) {
        self.rotation[axis] = degrees;
    }

    /// Sets one size; with `uniform` the other axes scale by the same factor. Does nothing
    /// for a flat mesh (zero base along that axis).
    pub fn set_size(&mut self, axis: usize, mm: f32, uniform: bool) {
        if self.base[axis] > 0. {
            self.set_factor(axis, mm / self.base[axis], uniform);
        }
    }

    pub fn set_scale_percent(&mut self, axis: usize, percent: f32, uniform: bool) {
        self.set_factor(axis, percent / 100., uniform);
    }

    fn set_factor(&mut self, axis: usize, factor: f32, uniform: bool) {
        let factor = factor.max(MIN_SCALE);
        if uniform {
            self.scale = Vec3::splat(factor);
        } else {
            self.scale[axis] = factor;
        }
    }

    pub fn toggle_mirror(&mut self, axis: usize) {
        self.mirror[axis] = -self.mirror[axis];
    }

    /// Rotation 0°, scale 100 %, no mirror; the position stays.
    pub fn reset(&mut self) {
        self.rotation = Vec3::ZERO;
        self.scale = Vec3::ONE;
        self.mirror = Vec3::ONE;
    }
}

impl App {
    /// A tool-card edit of object `i` from `before` to `t`. One drag of a field, or one typed
    /// value, is one undo step; the gesture ends when the pointer is released (see
    /// `App::show`). With several objects selected the change in position moves them all;
    /// rotation and scale are per object. `keep_on_bed` drops the object after a rotation or
    /// scale, as the gizmos do.
    pub(crate) fn apply_card_transform(
        &mut self,
        i: usize,
        before: Transform,
        t: Transform,
        keep_on_bed: bool,
    ) {
        if self.plates.current().objects.get(i).is_none() {
            return;
        }
        self.begin_gesture("Transform");
        let shift = t.position - before.position;
        let o = &mut self.plates.current_mut().objects[i];
        t.apply_to(o);
        if keep_on_bed {
            o.drop_to_bed();
        }
        if shift != Vec3::ZERO {
            for k in self.selection() {
                if k != i {
                    self.plates.current_mut().objects[k].position += shift;
                }
            }
        }
        self.changed();
    }

    /// The floating tool card for the selected object, at the top right of the 3D view.
    pub(crate) fn object_window(&mut self, ctx: &egui::Context) {
        use crate::viewport::gizmo::Mode;
        let Some(i) = self.selected else {
            // No card: an editor left open in it is dropped rather than reopened later.
            widgets::cancel_field_edit(ctx);
            return;
        };
        let Some(o) = self.plates.current().objects.get(i) else {
            self.selected = None;
            widgets::cancel_field_edit(ctx);
            return;
        };
        if let Some(kind) = self.viewport.paint {
            self.paint_window(ctx, kind);
            return;
        }
        match self.viewport.object_tool {
            Some(crate::viewport::ObjectTool::LayerHeights) => return self.layer_window(ctx),
            Some(crate::viewport::ObjectTool::BrimEars) => return self.ears_window(ctx),
            None => {}
        }
        if self.cut.is_some() {
            self.mesh_tool = None;
            self.cut_window(ctx);
            return;
        }
        if self.mesh_tool.is_some() {
            self.mesh_tool_window(ctx);
            return;
        }
        if self.emboss.is_some() {
            self.emboss_window(ctx);
            return;
        }
        match self.sub {
            Some(crate::scene::Sub::Volume(id)) => {
                self.volume_window(ctx, i, id);
                return;
            }
            Some(crate::scene::Sub::Range(id)) => {
                self.range_window(ctx, i, id);
                return;
            }
            None => {}
        }
        let name = std::path::Path::new(&o.name)
            .file_stem()
            .map_or(o.name.clone(), |s| s.to_string_lossy().into_owned());
        let mode = self.viewport.mode;
        let lay_on_face = self.viewport.lay_on_face;
        let (title, show_pos, show_rot, show_scale) = match mode {
            Mode::Move => (format!("{name} · Move"), true, false, false),
            Mode::Rotate => (format!("{name} · Rotate"), false, true, false),
            Mode::Scale => (format!("{name} · Scale"), false, false, true),
            Mode::None => (name.clone(), true, true, true),
        };
        let card_w = if show_rot { 404. } else { CARD_W };
        let before = Transform::of(o);
        let mut t = before;
        let tool = o.tools.0[0].unwrap_or(0);
        let override_count = o.overrides.len();
        let tools = self.tool_count();
        let choices = self.tool_choices();
        let uniform_id = egui::Id::unique("uniform-scale");
        let mut uniform = ctx.data_mut(|d| *d.get_temp_mut_or(uniform_id, true));
        // Whether an edit changes the footprint (rotation, scale, mirror): those keep the
        // model on the bed, as the gizmos do; a typed position is taken as is.
        let mut keep_on_bed = false;
        let mut drop = false;
        let mut reset = false;
        let mut turn: Option<f32> = None;
        let mut toggle_lay = false;
        let mut orient = false;
        let mut new_tool = None;
        let mut edit_settings = false;
        let viewport = self.viewport_rect;
        egui::Area::new(egui::Id::unique("object-card"))
            .order(egui::Order::Middle)
            .fixed_pos(viewport.right_top() + egui::vec2(-14. - card_w, 14.))
            .show(ctx, |ui| {
                let p = theme::of(ui);
                theme::overlay_frame(ui)
                    .inner_margin(egui::Margin::symmetric(12, 10))
                    .show(ui, |ui| {
                        ui.set_width(card_w - 24.);
                        ui.spacing_mut().item_spacing = Vec2::new(4., 4.);
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(widgets::spaced_upper(&title))
                                    .font(theme::semibold(11.))
                                    .color(p.muted),
                            );
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if tools > 1 {
                                    egui::ComboBox::from_id_salt("object-card-tool")
                                        .width(78.)
                                        .selected_text(
                                            choices
                                                .get(tool)
                                                .map_or(format!("Tool {}", tool + 1), |c| c.label.clone()),
                                        )
                                        .show_ui(ui, |ui| {
                                            for c in &choices {
                                                let row = ui.horizontal(|ui| {
                                                    super::swatch(ui, c.rgba(), 12.);
                                                    ui.selectable_label(c.tool == tool, &c.label)
                                                });
                                                if row.inner.clicked() {
                                                    new_tool = Some(c.tool);
                                                }
                                            }
                                        });
                                }
                            });
                        });
                        ui.add_space(4.);
                        if show_rot {
                            // Quick turns and orientation tools, as in the rotation mockup.
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 8.;
                                if widgets::chip(ui, Some(Glyph::RotateCcw), "90°", false, true)
                                    .on_hover_text("Turn 90° counter-clockwise on the bed")
                                    .clicked()
                                {
                                    turn = Some(std::f32::consts::FRAC_PI_2);
                                }
                                if widgets::chip(ui, Some(Glyph::RotateCw), "90°", false, true)
                                    .on_hover_text("Turn 90° clockwise on the bed")
                                    .clicked()
                                {
                                    turn = Some(-std::f32::consts::FRAC_PI_2);
                                }
                                if widgets::chip(
                                    ui,
                                    Some(Glyph::LayOnFace),
                                    "Lay on face",
                                    lay_on_face,
                                    true,
                                )
                                .on_hover_text("Then click the face that should rest on the bed (F)")
                                .clicked()
                                {
                                    toggle_lay = true;
                                }
                                if widgets::chip(ui, Some(Glyph::Star), "Auto-orient", true, true)
                                    .on_hover_text("Find the orientation with the least overhang and support, resting on a large flat face")
                                    .clicked()
                                {
                                    orient = true;
                                }
                            });
                            ui.add_space(2.);
                        }
                        let axes = ["X", "Y", "Z"];
                        // A row of three compact fields after a muted label. Returns the axis
                        // and value of the field that was edited this frame (typed and
                        // committed, or dragged), and the row rect. The fields show `values`,
                        // which the caller reads from the object every frame, except while
                        // one is being typed into (its buffer is shown then).
                        let fields = |ui: &mut egui::Ui,
                                          label: &str,
                                          id: &str,
                                          values: Vec3,
                                          decimals: usize,
                                          min: f64,
                                          speed: f64,
                                          accent_z: bool|
                         -> (Option<(usize, f32)>, Rect) {
                            let mut edited = None;
                            let (row, _) = ui.allocate_exact_size(
                                Vec2::new(ui.available_width(), FIELD_H + 2.),
                                Sense::hover(),
                            );
                            ui.painter().text(
                                Pos2::new(row.left(), row.center().y),
                                Align2::LEFT_CENTER,
                                label,
                                theme::sans(12.),
                                p.muted_alt,
                            );
                            for (k, axis) in axes.iter().enumerate() {
                                let x = row.left() + LABEL_W + k as f32 * (FIELD_W + 6.);
                                let rect = Rect::from_min_size(
                                    Pos2::new(x, row.center().y - FIELD_H / 2.),
                                    Vec2::new(FIELD_W, FIELD_H),
                                );
                                ui.painter().text(
                                    Pos2::new(rect.left() + 6., rect.center().y),
                                    Align2::LEFT_CENTER,
                                    *axis,
                                    theme::sans(10.5),
                                    p.muted,
                                );
                                let spec = NumberSpec {
                                    unit: "",
                                    decimals,
                                    min,
                                    max: 1e6,
                                    speed,
                                };
                                let fid = ui.make_persistent_id(("object-field", id, k));
                                if let Some(v) = widgets::number_with(
                                    ui,
                                    fid,
                                    rect,
                                    values[k] as f64,
                                    &spec,
                                    true,
                                    accent_z && k == 2,
                                ) {
                                    edited = Some((k, v as f32));
                                }
                            }
                            (edited, row)
                        };
                        if show_pos
                            && let (Some((k, v)), _) =
                                fields(ui, "Position mm", "pos", t.position, 1, -1e6, 0.5, false)
                        {
                            t.set_position(k, v);
                        }
                        if show_rot {
                            let (edited, row) =
                                fields(ui, "Rotation °", "rot", t.rotation, 0, -1e6, 1., mode == Mode::Rotate);
                            if let Some((k, v)) = edited {
                                t.set_rotation_degrees(k, v);
                                keep_on_bed = true;
                            }
                            if mode == Mode::Rotate {
                                // Reset right of the angle fields (the card is wide enough).
                                let x = row.left() + LABEL_W + 3. * (FIELD_W + 6.) + 4.;
                                let r = Rect::from_min_size(
                                    Pos2::new(x, row.center().y - 13.),
                                    Vec2::new(row.right() - x, 26.),
                                );
                                let mut child = ui.new_child(
                                    egui::UiBuilder::new()
                                        .max_rect(r)
                                        .layout(egui::Layout::left_to_right(egui::Align::Center)),
                                );
                                if widgets::styled_button(&mut child, "Reset", true, false, 26.)
                                    .on_hover_text("Back to the imported orientation")
                                    .clicked()
                                {
                                    t.rotation = Vec3::ZERO;
                                    keep_on_bed = true;
                                }
                            }
                        }
                        if show_scale {
                            if let (Some((k, v)), _) =
                                fields(ui, "Size mm", "size", t.size(), 1, 0.01, 0.5, false)
                            {
                                t.set_size(k, v, uniform);
                                keep_on_bed = true;
                            }
                            if let (Some((k, v)), _) =
                                fields(ui, "Scale %", "scale", t.scale_percent(), 1, 0.1, 0.5, false)
                            {
                                t.set_scale_percent(k, v, uniform);
                                keep_on_bed = true;
                            }
                            // Mirror: three toggle buttons in the field positions.
                            let (row, _) = ui.allocate_exact_size(
                                Vec2::new(ui.available_width(), FIELD_H + 2.),
                                Sense::hover(),
                            );
                            ui.painter().text(
                                Pos2::new(row.left(), row.center().y),
                                Align2::LEFT_CENTER,
                                "Mirror",
                                theme::sans(12.),
                                p.muted_alt,
                            );
                            for (k, axis) in axes.iter().enumerate() {
                                let x = row.left() + LABEL_W + k as f32 * (FIELD_W + 6.);
                                let rect = Rect::from_min_size(
                                    Pos2::new(x, row.center().y - FIELD_H / 2.),
                                    Vec2::new(FIELD_W, FIELD_H),
                                );
                                let on = t.mirror[k] < 0.;
                                let r = ui.interact(
                                    rect,
                                    ui.make_persistent_id(("mirror", k)),
                                    Sense::click(),
                                );
                                ui.painter().rect_filled(
                                    rect,
                                    5.,
                                    if on { p.accent_surface } else { p.field },
                                );
                                ui.painter().rect_stroke(
                                    rect,
                                    5.,
                                    Stroke::new(1., if on || r.hovered() { p.accent } else { p.field_border }),
                                    egui::StrokeKind::Inside,
                                );
                                ui.painter().text(
                                    rect.center(),
                                    Align2::CENTER_CENTER,
                                    *axis,
                                    theme::medium(12.),
                                    if on { p.accent_hi } else { p.label },
                                );
                                if r.on_hover_text(format!("Mirror along {axis}")).clicked() {
                                    t.toggle_mirror(k);
                                    keep_on_bed = true;
                                }
                            }
                        }
                        ui.add_space(6.);
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 8.;
                            if show_scale {
                                let (r, _) = ui.allocate_exact_size(Vec2::new(34., 20.), Sense::hover());
                                if let Some(v) = widgets::toggle_at(
                                    ui,
                                    ui.make_persistent_id("uniform-toggle"),
                                    r,
                                    uniform,
                                    true,
                                ) {
                                    uniform = v;
                                    ctx.data_mut(|d| d.insert_temp(uniform_id, v));
                                }
                                ui.label(
                                    egui::RichText::new("Uniform")
                                        .font(theme::sans(12.))
                                        .color(p.label),
                                )
                                .on_hover_text("Scale all axes together");
                            }
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                ui.spacing_mut().item_spacing.x = 6.;
                                if (show_scale || mode == Mode::None)
                                    && widgets::styled_button(ui, "Reset", true, false, 26.)
                                        .on_hover_text("Rotation 0°, scale 100 %, no mirror")
                                        .clicked()
                                {
                                    reset = true;
                                }
                                if (show_pos || mode == Mode::None)
                                    && widgets::styled_button(ui, "Drop to bed", true, false, 26.)
                                        .on_hover_text("Rest the lowest point on the bed · F lays a chosen face down")
                                        .clicked()
                                {
                                    drop = true;
                                }
                            });
                        });
                        ui.add_space(4.);
                        let text = if override_count == 0 {
                            "Object settings…".to_string()
                        } else {
                            format!(
                                "Object settings · {override_count} override{}",
                                if override_count == 1 { "" } else { "s" }
                            )
                        };
                        if widgets::link(ui, &text).clicked() {
                            edit_settings = true;
                        }
                    });
            });
        if t != before {
            self.apply_card_transform(i, before, t, keep_on_bed);
        }
        if let Some(angle) = turn {
            self.record("Rotate");
            self.transform_selection(crate::scene::Delta::Rotate {
                axis: Vec3::Z,
                angle,
            });
            self.changed();
        }
        if toggle_lay {
            self.viewport.lay_on_face = !self.viewport.lay_on_face;
        }
        if orient {
            self.auto_orient();
        }
        if drop {
            self.record("Drop to bed");
            for k in self.selection() {
                self.plates.current_mut().objects[k].drop_to_bed();
            }
            self.changed();
        }
        if reset {
            self.record("Reset transform");
            let o = &mut self.plates.current_mut().objects[i];
            let mut t = Transform::of(o);
            t.reset();
            t.apply_to(o);
            o.drop_to_bed();
            self.changed();
        }
        if let Some(t) = new_tool
            && t != tool
        {
            self.run_command(crate::commands::Command::ChangeTool(t), ctx);
        }
        if edit_settings {
            self.editor.object_mode = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mica_core::{
        config::Config, input::InputObject, mesh::TriangleMesh, print_job::ToolAssignments,
    };

    /// A 60 × 31 × 48 box (the Benchy's size), its bottom 5 mm below the bed.
    fn benchy_box() -> Object {
        let (x, y, z) = (60., 31., 48.);
        let mesh = TriangleMesh {
            vertices: vec![
                [0., 0., -5.],
                [x, 0., -5.],
                [x, y, -5.],
                [0., y, -5.],
                [0., 0., z - 5.],
                [x, 0., z - 5.],
                [x, y, z - 5.],
                [0., y, z - 5.],
            ],
            faces: vec![
                [0, 2, 1],
                [0, 3, 2],
                [4, 5, 6],
                [4, 6, 7],
                [0, 1, 5],
                [0, 5, 4],
                [1, 2, 6],
                [1, 6, 5],
                [2, 3, 7],
                [2, 7, 6],
                [3, 0, 4],
                [3, 4, 7],
            ],
        };
        Object::new(InputObject::plain(
            "box",
            0,
            0,
            mesh,
            ToolAssignments([Some(0); 5]),
            Config::defaults(),
        ))
    }

    fn close(a: Vec3, b: Vec3) -> bool {
        (a - b).abs().max_element() < 1e-3
    }

    #[test]
    fn typed_size_scales_uniformly_and_keeps_scale_consistent() {
        let o = benchy_box();
        let mut t = Transform::of(&o);
        assert!(close(t.size(), Vec3::new(60., 31., 48.)));
        assert!(close(t.scale_percent(), Vec3::splat(100.)));
        // Size X typed as 120 with Uniform on: everything doubles.
        t.set_size(0, 120., true);
        assert!(close(t.scale, Vec3::splat(2.)));
        assert!(close(t.size(), Vec3::new(120., 62., 96.)));
        assert!(close(t.scale_percent(), Vec3::splat(200.)));
        // Scale % Z typed as 50 with Uniform on: everything halves.
        t.set_scale_percent(2, 50., true);
        assert!(close(t.size(), Vec3::new(30., 15.5, 24.)));
        // Uniform off: only the edited axis changes, the others keep their factor.
        t.set_size(1, 62., false);
        assert!(close(t.scale, Vec3::new(0.5, 2., 0.5)));
        assert!(close(t.size(), Vec3::new(30., 62., 24.)));
        t.set_scale_percent(0, 100., false);
        assert!(close(t.size(), Vec3::new(60., 62., 24.)));
        // Zero and negative sizes clamp to the smallest factor instead of collapsing.
        t.set_size(2, 0., false);
        assert_eq!(t.scale.z, MIN_SCALE);
        t.set_scale_percent(2, -40., true);
        assert_eq!(t.scale, Vec3::splat(MIN_SCALE));
    }

    #[test]
    fn edits_apply_to_the_object_and_read_back() {
        let mut o = benchy_box();
        let mut t = Transform::of(&o);
        t.set_position(0, 12.5);
        t.set_position(2, 3.);
        t.set_rotation_degrees(2, 90.);
        t.set_size(0, 120., true);
        t.toggle_mirror(1);
        t.apply_to(&mut o);
        assert_eq!(o.position, Vec3::new(12.5, 0., 3.));
        assert!((o.angle - std::f32::consts::FRAC_PI_2).abs() < 1e-5);
        assert!(close(o.scale, Vec3::splat(2.)));
        assert_eq!(o.mirror, Vec3::new(1., -1., 1.));
        // What the fields show next frame is what was typed.
        let again = Transform::of(&o);
        assert!(close(again.position, t.position));
        assert!(close(again.rotation, Vec3::new(0., 0., 90.)));
        assert!(close(again.size(), Vec3::new(120., 62., 96.)));
        assert_eq!(again.mirror, t.mirror);
        // The mirrored, scaled and turned box still rests on the bed after a drop.
        o.drop_to_bed();
        assert!(o.lowest_z().abs() < 1e-4);
        // X / Y tilts survive the round trip too, and an unrelated edit does not touch them.
        t = Transform::of(&o);
        t.set_rotation_degrees(0, 30.);
        t.set_rotation_degrees(1, -20.);
        t.apply_to(&mut o);
        let tilt = o.tilt;
        let mut u = Transform::of(&o);
        assert!(close(u.rotation, Vec3::new(30., -20., 90.)));
        u.set_position(1, 40.);
        u.apply_to(&mut o);
        assert_eq!(o.tilt, tilt);
        assert_eq!(o.position.y, 40.);
    }

    #[test]
    fn reset_restores_orientation_scale_and_mirror_but_not_position() {
        let mut o = benchy_box();
        let mut t = Transform::of(&o);
        t.set_position(0, 50.);
        t.set_rotation_degrees(2, 45.);
        t.set_rotation_degrees(0, 10.);
        t.set_scale_percent(1, 300., false);
        t.toggle_mirror(0);
        t.apply_to(&mut o);
        let mut r = Transform::of(&o);
        r.reset();
        r.apply_to(&mut o);
        assert_eq!(o.position.x, 50.);
        assert!(o.angle.abs() < 1e-6);
        assert!(close(Transform::of(&o).rotation, Vec3::ZERO));
        assert_eq!(o.scale, Vec3::ONE);
        assert_eq!(o.mirror, Vec3::ONE);
        // A reset of an untouched object is a no-op for the undo comparison.
        let fresh = benchy_box();
        let mut t = Transform::of(&fresh);
        let before = t;
        t.reset();
        assert_eq!(t, before);
    }
}
