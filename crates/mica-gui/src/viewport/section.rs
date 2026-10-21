// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/slic3r/GUI/MeshUtils.cpp :: MeshClipper::recalculate_triangles
// Source: prusaslicer@b028299c77 src/slic3r/GUI/Gizmos/GLGizmoPainterBase.cpp :: GLGizmoPainterBase::render_cursor
// Changes: The clipping plane of PrusaSlicer's painting and cut gizmos made a view tool of its own: one plane per mode (Prepare cuts the objects and fills the cut faces, Preview cuts the toolpaths), positioned as a fraction across the scene along its normal; "Face camera" is upstream's "Reset direction". The cut is a fragment discard in Mica's shaders and the caps come from mica_core::mesh_cut::section_cap per object (cached by object, transform and plane) instead of MeshClipper's GLU-tessellated slices; drag handles (arrow to move, ball to tilt), the card and its axis presets are original.

//! Section view: a clipping plane through the scene to look inside. In Prepare it cuts the
//! objects and fills the cut faces; in Preview it cuts the toolpaths (the layer slider already
//! cuts horizontally, this plane goes in any direction).

use super::gizmo;
use crate::{render::CapDraw, scene::Object, theme};
use eframe::egui::{self, Align2, Pos2, Rect, Stroke, Vec2 as EVec2};
use glam::Vec3;
use mica_core::{
    mesh::TriangleMesh,
    mesh_cut::{CutPlane, section_cap},
};
use std::{collections::HashMap, sync::Arc};

/// A section plane: everything on the side `normal` points to is cut away.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plane {
    /// Unit normal.
    pub normal: Vec3,
    /// Where the plane sits across the scene along the normal: 0 cuts everything away, 1 nothing.
    pub position: f32,
}
impl Default for Plane {
    fn default() -> Self {
        Self {
            normal: Vec3::new(0., -1., 0.),
            position: 0.5,
        }
    }
}
impl Plane {
    /// The plane facing the camera (cutting away the half nearer to the viewer).
    pub fn facing(direction: Vec3) -> Self {
        Self {
            normal: (-direction).normalize_or(Vec3::Y),
            position: 0.5,
        }
    }
    /// Signed range of `normal · p` over the corners of the box.
    pub fn span(&self, (min, max): (Vec3, Vec3)) -> (f32, f32) {
        let mut lo = f32::INFINITY;
        let mut hi = f32::NEG_INFINITY;
        for x in [min.x, max.x] {
            for y in [min.y, max.y] {
                for z in [min.z, max.z] {
                    let d = self.normal.dot(Vec3::new(x, y, z));
                    lo = lo.min(d);
                    hi = hi.max(d);
                }
            }
        }
        (lo, hi)
    }
    /// The plane equation `[nx, ny, nz, d]` (cut where `n · p > d`) for scene bounds.
    pub fn equation(&self, bounds: (Vec3, Vec3)) -> [f32; 4] {
        let (lo, hi) = self.span(bounds);
        let d = lo + (hi - lo) * self.position.clamp(0., 1.);
        [self.normal.x, self.normal.y, self.normal.z, d]
    }
    /// Sets `position` so the plane passes through `d` (`n · p = d`).
    fn set_offset(&mut self, d: f32, bounds: (Vec3, Vec3)) {
        let (lo, hi) = self.span(bounds);
        self.position = if hi - lo > 1e-6 {
            ((d - lo) / (hi - lo)).clamp(0., 1.)
        } else {
            0.5
        };
    }
}

/// Which handle of the plane the pointer is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handle {
    /// The arrow along the normal: moves the plane.
    Arrow,
    /// The ball at the arrow's tip: tilts the plane about its centre.
    Ball,
}

#[derive(Clone, Copy, Debug)]
struct Grab {
    handle: Handle,
    /// Plane centre and offset at the start of the drag.
    center: Vec3,
    offset: f32,
    /// Parameter along the normal where the pointer grabbed the arrow.
    along: f32,
    radius: f32,
}

/// The tool's state: one plane for Prepare and one for Preview, each `None` while off.
#[derive(Default)]
pub struct Section {
    pub prepare: Option<Plane>,
    pub preview: Option<Plane>,
    pub hover: Option<Handle>,
    grab: Option<Grab>,
    /// Filled cut faces per object id: (content key, world-space cap, the object mesh it was
    /// cut from, held so the key's mesh address is not reused while it is cached).
    caps: HashMap<u64, (u64, Arc<TriangleMesh>, Arc<TriangleMesh>)>,
}

/// Union of the objects' bounds (the bed volume when there are none).
pub fn scene_bounds<'a>(
    objects: impl IntoIterator<Item = &'a Object>,
    fallback: (Vec3, Vec3),
) -> (Vec3, Vec3) {
    let mut out: Option<(Vec3, Vec3)> = None;
    for o in objects {
        let (a, b) = o.bounds();
        out = Some(match out {
            Some((lo, hi)) => (lo.min(a), hi.max(b)),
            None => (a, b),
        });
    }
    out.unwrap_or(fallback)
}

fn key_of(o: &Object, eq: [f32; 4]) -> u64 {
    use std::hash::{Hash, Hasher};
    let (id, transform) = (o.id, o.transform());
    let mut h = std::collections::hash_map::DefaultHasher::new();
    id.hash(&mut h);
    // The mesh too: a text object's mesh is rebuilt under the same id.
    (Arc::as_ptr(&o.mesh) as usize).hash(&mut h);
    transform.to_cols_array().map(f32::to_bits).hash(&mut h);
    eq.map(f32::to_bits).hash(&mut h);
    h.finish()
}

/// The world-space cap of one object cut by the plane `eq` (`n · p = d`).
pub fn object_cap(o: &Object, eq: [f32; 4]) -> TriangleMesh {
    let t = o.transform();
    let n = Vec3::new(eq[0], eq[1], eq[2]);
    // A point of the plane, then the plane in the object's own coordinates (normals go by the
    // inverse transpose: n_local ∝ Tᵀ n).
    let p = n * eq[3];
    let inv = t.inverse();
    let local_point = inv.transform_point3(p);
    let local_normal = t.transpose().transform_vector3(n);
    let mut cap = section_cap(
        &o.mesh,
        &CutPlane {
            point: local_point.to_array(),
            normal: local_normal.to_array(),
        },
    );
    for v in &mut cap.vertices {
        *v = t.transform_point3(Vec3::from(*v)).to_array();
    }
    // A mirroring transform flips the triangles; keep them facing the plane normal.
    if t.determinant() < 0. {
        for f in &mut cap.faces {
            f.swap(1, 2);
        }
    }
    cap
}

impl Section {
    pub fn active(&self, preview: bool) -> bool {
        if preview {
            self.preview.is_some()
        } else {
            self.prepare.is_some()
        }
    }
    pub fn plane(&self, preview: bool) -> Option<Plane> {
        if preview { self.preview } else { self.prepare }
    }
    fn plane_mut(&mut self, preview: bool) -> &mut Option<Plane> {
        if preview {
            &mut self.preview
        } else {
            &mut self.prepare
        }
    }
    /// Turns the tool on (facing the camera) or off for a mode.
    pub fn toggle(&mut self, preview: bool, camera_direction: Vec3) {
        let p = self.plane_mut(preview);
        *p = match p {
            Some(_) => None,
            None => Some(Plane::facing(camera_direction)),
        };
        self.grab = None;
        self.hover = None;
    }
    /// Caps of the objects the plane crosses, recomputed only when an object or the plane moved.
    pub fn caps(
        &mut self,
        objects: &[Object],
        eq: [f32; 4],
        color_of: impl Fn(&Object) -> [f32; 3],
    ) -> Vec<CapDraw> {
        let n = Vec3::new(eq[0], eq[1], eq[2]);
        let mut out = vec![];
        let mut keep = vec![];
        for o in objects {
            let (a, b) = o.bounds();
            let plane = Plane {
                normal: n,
                position: 0.,
            };
            let (lo, hi) = plane.span((a, b));
            if eq[3] <= lo || eq[3] >= hi {
                continue;
            }
            let key = key_of(o, eq);
            let cap = match self.caps.get(&o.id) {
                Some((k, cap, _)) if *k == key => cap.clone(),
                _ => {
                    let cap = Arc::new(object_cap(o, eq));
                    self.caps.insert(o.id, (key, cap.clone(), o.mesh.clone()));
                    cap
                }
            };
            keep.push(o.id);
            out.push(CapDraw {
                key,
                mesh: cap,
                color: color_of(o),
            });
        }
        self.caps.retain(|id, _| keep.contains(id));
        out
    }
    /// Centre of the plane (the bounds centre moved onto it) and the handle length.
    fn handle_geometry(eq: [f32; 4], bounds: (Vec3, Vec3), mm_per_px: f32) -> (Vec3, Vec3, f32) {
        let n = Vec3::new(eq[0], eq[1], eq[2]);
        let c = (bounds.0 + bounds.1) * 0.5;
        let center = c + n * (eq[3] - n.dot(c));
        (center, n, mm_per_px * 70.)
    }
    /// The handle under the pointer, if any.
    pub fn hit(
        &self,
        gp: &gizmo::Painter<'_>,
        eq: [f32; 4],
        bounds: (Vec3, Vec3),
        pointer: Pos2,
    ) -> Option<Handle> {
        let mm = gp.camera.mm_per_pixel(gp.rect.height());
        let (center, n, len) = Self::handle_geometry(eq, bounds, mm);
        let base = gp.project(center)?;
        let tip = gp.project(center + n * len)?;
        if tip.distance(pointer) < 11. {
            return Some(Handle::Ball);
        }
        let ab = tip - base;
        let t = ((pointer - base).dot(ab) / ab.length_sq().max(1e-6)).clamp(0., 1.);
        ((base + ab * t).distance(pointer) < 8.).then_some(Handle::Arrow)
    }
    /// Starts dragging a handle of the current mode's plane.
    pub fn begin(
        &mut self,
        preview: bool,
        handle: Handle,
        bounds: (Vec3, Vec3),
        ray: (Vec3, Vec3),
        mm_per_px: f32,
    ) {
        let Some(plane) = self.plane(preview) else {
            return;
        };
        let eq = plane.equation(bounds);
        let (center, n, len) = Self::handle_geometry(eq, bounds, mm_per_px);
        self.grab = Some(Grab {
            handle,
            center,
            offset: eq[3],
            along: closest_on_line(center, n, ray.0, ray.1),
            radius: len,
        });
    }
    /// Follows the pointer during a drag; returns whether the plane changed. Shift snaps a tilt
    /// to the nearest axis within 10°.
    pub fn update(
        &mut self,
        preview: bool,
        bounds: (Vec3, Vec3),
        ray: (Vec3, Vec3),
        shift: bool,
    ) -> bool {
        let Some(g) = self.grab else {
            return false;
        };
        let Some(mut plane) = self.plane(preview) else {
            return false;
        };
        let before = plane;
        match g.handle {
            Handle::Arrow => {
                let along = closest_on_line(g.center, plane.normal, ray.0, ray.1);
                plane.set_offset(g.offset + (along - g.along), bounds);
            }
            Handle::Ball => {
                // The point on the sphere about the plane's centre under the pointer (or the
                // closest one along the ray when it misses) gives the new normal.
                let (o, d) = ray;
                let t = (g.center - o).dot(d);
                let closest = o + d * t;
                let off = (closest - g.center).length();
                let point = if off < g.radius {
                    let back = (g.radius * g.radius - off * off).sqrt();
                    // The near side of the sphere.
                    o + d * (t - back)
                } else {
                    closest
                };
                let mut normal = (point - g.center).normalize_or(plane.normal);
                if shift {
                    for axis in [Vec3::X, Vec3::Y, Vec3::Z, -Vec3::X, -Vec3::Y, -Vec3::Z] {
                        if normal.dot(axis) > 10f32.to_radians().cos() {
                            normal = axis;
                        }
                    }
                }
                plane.normal = normal;
                // Keep the plane through its centre while it turns.
                plane.set_offset(normal.dot(g.center), bounds);
            }
        }
        *self.plane_mut(preview) = Some(plane);
        plane != before
    }
    /// Moves the current mode's plane by `delta` of the scene span (Alt-scroll); true if it moved.
    pub fn nudge(&mut self, preview: bool, delta: f32) -> bool {
        let Some(p) = self.plane_mut(preview).as_mut() else {
            return false;
        };
        let before = p.position;
        p.position = (p.position + delta).clamp(0., 1.);
        p.position != before
    }
    pub fn end(&mut self) {
        self.grab = None;
    }
    /// Draws the plane's outline across the scene bounds and its handles.
    pub fn draw(&self, gp: &gizmo::Painter<'_>, preview: bool, bounds: (Vec3, Vec3)) {
        let Some(plane) = self.plane(preview) else {
            return;
        };
        let eq = plane.equation(bounds);
        let mm = gp.camera.mm_per_pixel(gp.rect.height());
        let outline = box_section(bounds, eq, mm * 6.);
        let pts: Vec<Pos2> = outline.iter().filter_map(|p| gp.project(*p)).collect();
        if pts.len() >= 3 {
            gp.painter.add(egui::Shape::convex_polygon(
                pts.clone(),
                theme::ACCENT.gamma_multiply(0.10),
                Stroke::NONE,
            ));
            gp.painter.add(egui::Shape::closed_line(
                pts,
                Stroke::new(1.5, theme::ACCENT_BRIGHT.gamma_multiply(0.85)),
            ));
        }
        let (center, n, len) = Self::handle_geometry(eq, bounds, mm);
        let active = self.grab.map(|g| g.handle);
        let hot = |h: Handle| active == Some(h) || (active.is_none() && self.hover == Some(h));
        if let (Some(a), Some(b)) = (gp.project(center), gp.project(center + n * len)) {
            let color = if hot(Handle::Arrow) {
                theme::HANDLE_HOT
            } else {
                theme::ACCENT_BRIGHT
            };
            gp.painter
                .line_segment([a, b], Stroke::new(6., theme::OUTLINE));
            gp.painter.line_segment([a, b], Stroke::new(2.5, color));
            gp.painter.circle_filled(a, 4., color);
            let ball = if hot(Handle::Ball) {
                theme::HANDLE_HOT
            } else {
                theme::ACCENT_HI
            };
            gp.painter
                .circle(b, 7., ball, Stroke::new(2., theme::OUTLINE));
        }
    }
    /// The floating card: position slider, direction presets, flip, reset and close. Returns
    /// whether the plane changed.
    pub fn card(
        &mut self,
        ui: &mut egui::Ui,
        rect: Rect,
        preview: bool,
        camera_direction: Vec3,
    ) -> bool {
        let Some(mut plane) = self.plane(preview) else {
            return false;
        };
        let before = plane;
        let mut close = false;
        // Under the toolbar (Prepare) or the preview's section button, 14 px down; their
        // height follows the toolbar size.
        let top = rect.top() + 14. + super::icons::metrics(ui.ctx()).bar_height() + 12.;
        egui::Area::new(ui.scope_id().with(("section card", preview)))
            .order(egui::Order::Middle)
            .fixed_pos(Pos2::new(rect.center().x, top))
            .pivot(Align2::CENTER_TOP)
            .show(ui.ctx(), |ui| {
                let p = theme::of(ui);
                theme::overlay_frame(ui)
                    .inner_margin(egui::Margin::symmetric(12, 8))
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing = EVec2::new(6., 6.);
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(crate::panels::widgets::spaced_upper(
                                    "Section view",
                                ))
                                .font(theme::semibold(11.))
                                .color(p.muted),
                            );
                            let mut pct = plane.position * 100.;
                            let slider = egui::Slider::new(&mut pct, 0.0..=100.)
                                .show_value(false)
                                .trailing_fill(true);
                            if ui
                                .add_sized([180., 18.], slider)
                                .on_hover_text("Where the plane cuts, across the scene")
                                .changed()
                            {
                                plane.position = pct / 100.;
                            }
                            ui.label(
                                egui::RichText::new(format!("{:.0} %", plane.position * 100.))
                                    .font(theme::mono(12.))
                                    .color(p.label),
                            );
                        });
                        ui.horizontal(|ui| {
                            let small = |ui: &mut egui::Ui, text: &str, tip: &str| {
                                crate::panels::widgets::styled_button(ui, text, true, false, 24.)
                                    .on_hover_text(tip)
                                    .clicked()
                            };
                            if small(ui, "Face camera", "Turn the plane towards the view") {
                                plane = Plane {
                                    position: plane.position,
                                    ..Plane::facing(camera_direction)
                                };
                            }
                            for (label, axis) in [("X", Vec3::X), ("Y", Vec3::Y), ("Z", Vec3::Z)] {
                                if small(ui, label, &format!("Cut across {label}")) {
                                    plane.normal = axis;
                                }
                            }
                            if small(ui, "Flip", "Keep the other side") {
                                plane.normal = -plane.normal;
                                plane.position = 1. - plane.position;
                            }
                            if small(ui, "Reset", "Facing the camera, through the middle") {
                                plane = Plane::facing(camera_direction);
                            }
                            if small(ui, "Close", "Leave the section view") {
                                close = true;
                            }
                        });
                        ui.label(
                            egui::RichText::new(if preview {
                                "Drag the arrow to move the cut, the ball to tilt it (Shift snaps to an axis)"
                            } else {
                                "Cut faces are filled · drag the arrow to move, the ball to tilt (Shift snaps)"
                            })
                            .font(theme::sans(11.))
                            .color(p.muted),
                        );
                    });
            });
        if close {
            *self.plane_mut(preview) = None;
            self.grab = None;
            return true;
        }
        *self.plane_mut(preview) = Some(plane);
        plane != before
    }
}

/// Parameter `s` of the point on the line `o + d·s` closest to the ray `ro + rd·t`.
fn closest_on_line(o: Vec3, d: Vec3, ro: Vec3, rd: Vec3) -> f32 {
    let w = o - ro;
    let (a, b, c) = (d.dot(d), d.dot(rd), rd.dot(rd));
    let (dd, e) = (d.dot(w), rd.dot(w));
    let denom = a * c - b * b;
    if denom.abs() < 1e-9 {
        return 0.;
    }
    (b * e - c * dd) / denom
}

/// The polygon where the plane `eq` crosses the box (grown by `pad`), in order around the normal.
pub fn box_section(bounds: (Vec3, Vec3), eq: [f32; 4], pad: f32) -> Vec<Vec3> {
    let (min, max) = (bounds.0 - Vec3::splat(pad), bounds.1 + Vec3::splat(pad));
    let n = Vec3::new(eq[0], eq[1], eq[2]);
    let corners: Vec<Vec3> = (0..8)
        .map(|i| {
            Vec3::new(
                if i & 1 == 0 { min.x } else { max.x },
                if i & 2 == 0 { min.y } else { max.y },
                if i & 4 == 0 { min.z } else { max.z },
            )
        })
        .collect();
    let mut pts = vec![];
    for i in 0..8 {
        for bit in [1, 2, 4] {
            let j = i | bit;
            if j == i {
                continue;
            }
            let (a, b) = (corners[i], corners[j]);
            let (da, db) = (n.dot(a) - eq[3], n.dot(b) - eq[3]);
            if (da <= 0.) != (db <= 0.) {
                pts.push(a + (b - a) * (da / (da - db)));
            }
        }
    }
    if pts.len() < 3 {
        return pts;
    }
    let c = pts.iter().copied().sum::<Vec3>() / pts.len() as f32;
    let u = (pts[0] - c).normalize_or(n.any_orthonormal_vector());
    let v = n.cross(u);
    pts.sort_by(|a, b| {
        let fa = (*a - c).dot(v).atan2((*a - c).dot(u));
        let fb = (*b - c).dot(v).atan2((*b - c).dot(u));
        fa.partial_cmp(&fb).unwrap_or(std::cmp::Ordering::Equal)
    });
    pts
}

/// The section caps' colour: the object's filament colour, darkened like a cut surface.
pub fn cap_color(base: [f32; 3]) -> [f32; 3] {
    base.map(|c| c * 0.82 + 0.04)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn plane_equation_spans_the_bounds() {
        let b = (Vec3::new(0., 0., 0.), Vec3::new(10., 20., 30.));
        let p = Plane {
            normal: Vec3::Z,
            position: 0.25,
        };
        assert_eq!(p.equation(b), [0., 0., 1., 7.5]);
        let mut q = Plane {
            normal: -Vec3::X,
            position: 0.,
        };
        // Facing −X: n · p runs from −10 to 0.
        assert_eq!(q.equation(b)[3], -10.);
        q.set_offset(-4., b);
        assert!((q.position - 0.6).abs() < 1e-6);
        assert_eq!(
            Plane::facing(Vec3::new(0., 1., 0.)).normal,
            Vec3::new(0., -1., 0.)
        );
    }
    #[test]
    fn section_of_a_box_is_a_closed_polygon() {
        let b = (Vec3::ZERO, Vec3::splat(10.));
        let quad = box_section(b, [0., 0., 1., 5.], 0.);
        assert_eq!(quad.len(), 4);
        assert!(quad.iter().all(|p| (p.z - 5.).abs() < 1e-6));
        // A diagonal plane through the centre cuts a hexagon.
        let n = Vec3::ONE.normalize();
        let hex = box_section(b, [n.x, n.y, n.z, n.dot(Vec3::splat(5.))], 0.);
        assert_eq!(hex.len(), 6);
        assert!(box_section(b, [0., 0., 1., 50.], 0.).is_empty());
    }
    #[test]
    fn caps_of_a_cube_object() {
        let mut o = Object::new(mica_core::input::InputObject::plain(
            "cube",
            0,
            0,
            mica_core::mesh_edit::cuboid([20., 20., 20.]),
            mica_core::print_job::ToolAssignments([Some(0); 5]),
            mica_core::config::Config::defaults(),
        ));
        // The cube spans −10..10; put it on the bed at (50, 50).
        o.position = Vec3::new(50., 50., 10.) - o.pivot_world() + o.position;
        let cap = object_cap(&o, [0., 0., 1., 10.]);
        let area: f32 = cap
            .faces
            .iter()
            .map(|f| {
                let [a, b, c] = f.map(|i| Vec3::from(cap.vertices[i as usize]));
                (b - a).cross(c - a).z * 0.5
            })
            .sum();
        assert!((area - 400.).abs() < 1e-2, "{area}");
        assert!(cap.vertices.iter().all(|v| (v[2] - 10.).abs() < 1e-4));
        let mut s = Section::default();
        let caps = s.caps(std::slice::from_ref(&o), [0., 0., 1., 10.], |_| {
            [1., 1., 1.]
        });
        assert_eq!(caps.len(), 1);
        // Unchanged plane and object: the cached mesh comes back.
        let again = s.caps(std::slice::from_ref(&o), [0., 0., 1., 10.], |_| {
            [1., 1., 1.]
        });
        assert!(Arc::ptr_eq(&caps[0].mesh, &again[0].mesh));
        // A plane above the object: no cap.
        assert!(s.caps(&[o], [0., 0., 1., 40.], |_| [1., 1., 1.]).is_empty());
    }
}
