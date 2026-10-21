// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/slic3r/GUI/Gizmos/GLGizmoRotate.cpp :: GLGizmoRotate::on_dragging
// Source: prusaslicer@b028299c77 src/slic3r/GUI/Gizmos/GLGizmoRotate.cpp :: GLGizmoRotate::mouse_position_in_local_plane
// Source: prusaslicer@b028299c77 src/slic3r/GUI/Gizmos/GLGizmoRotate.cpp :: GLGizmoRotate::local_transform
// Source: prusaslicer@b028299c77 src/slic3r/GUI/Gizmos/GLGizmoRotate.cpp :: GLGizmoRotate::init_data_from_selection
// Source: prusaslicer@b028299c77 src/slic3r/GUI/Gizmos/GLGizmoRotate.cpp :: GLGizmoRotate::on_render
// Source: prusaslicer@b028299c77 src/slic3r/GUI/Gizmos/GLGizmoRotate.cpp :: GLGizmoRotate::render_scale
// Source: prusaslicer@b028299c77 src/slic3r/GUI/Gizmos/GLGizmoRotate.cpp :: GLGizmoRotate::render_snap_radii
// Source: prusaslicer@b028299c77 src/slic3r/GUI/Gizmos/GLGizmoRotate.cpp :: GLGizmoRotate::render_reference_radius
// Source: prusaslicer@b028299c77 src/slic3r/GUI/Gizmos/GLGizmoRotate.cpp :: GLGizmoRotate::render_angle_arc
// Source: prusaslicer@b028299c77 src/slic3r/GUI/Gizmos/GLGizmoRotate.cpp :: GLGizmoRotate3D::on_render
// Changes: Rust / egui re-implementation of the rotate gizmo's interaction. All three rings use
//   one code path (`Ring`): each is a full circle in its own frame (PrusaSlicer's local_transform:
//   X ring from −Y towards −Z, Y ring from +Z towards +X, Z ring from +X towards +Y; world axes,
//   as PrusaSlicer's default World coordinates, with an optional frame rotation for local ones);
//   the pointer ray is intersected with the ring's plane in that frame and atan2 gives the angle
//   (mouse_position_in_local_plane). The X / Y rings sit around the pivot with the radius of the
//   pivot-centred bounding sphere plus a margin (PrusaSlicer: bounding sphere + 5 mm); the Z ring
//   stays on the bed outside the footprint. Radii are frozen while dragging. The angle turned is
//   measured from where the ring was pressed (PrusaSlicer: from the grabber, which is the only
//   pick target there) and snapped on the absolute angle in the ring's frame: the X / Y handles
//   rest at 0 (PrusaSlicer resets the gizmo angle per drag), the Z handle at the object's yaw.
//   Snap zones by pointer distance as upstream (R/3..2R/3 → 45° with eight spokes; the tick band →
//   5°; elsewhere free) but with Mica's band 0.88R..1.13R so the handle on the ring snaps to 5°,
//   plus Shift = 1°. Exactly edge-on, or with the plane behind the camera, the ray point nearest
//   the ring centre is used (PrusaSlicer: the near / far end of the mouse ray). While dragging,
//   the other rings are hidden (GLGizmoRotate3D::on_render), and the dragged one shows ticks,
//   spokes, the reference radius, the radius to the handle and the swept arc. Hit testing,
//   handle glyphs, labels, the back-half dimming and all drawing are Mica's own.

//! Move / rotate / scale gizmos. The math (hit testing against world rays, drag deltas,
//! snapping) is independent of egui; drawing projects the 3D handles with the camera.
//!
//! Rotation follows PrusaSlicer's `GLGizmoRotate`: three full rings, one per axis, each with a
//! grab handle, a tick scale (5° / 15° / 45°) and a live angle label. Turning on the bed (Z) is a
//! ring flat on the bed outside the model's footprint; tilting about X and Y uses rings around
//! the pivot, sized by the sphere that holds the model. The pointer is mapped to an angle by
//! intersecting its ray with the ring's plane (in the ring's own frame) and taking atan2, so the
//! handle follows the pointer around the ring from any view. Dragging with the pointer on the
//! tick band snaps to 5°, moving it inwards (between a third and two thirds of the radius, where
//! spokes appear) snaps to 45°, moving it elsewhere turns freely, and Shift always gives 1°
//! steps. Ring radii do not change while the object turns and are frozen during a drag. Every
//! handle is a stroke over a dark outline so it reads on any model colour, and the back half of
//! each ring is dimmed for depth.

use crate::{scene::Delta, scene::camera::Camera, theme};
use eframe::egui::{self, Align2, Color32, Pos2, Rect, Stroke, Vec2 as EVec2};
use glam::{Quat, Vec2, Vec3};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Mode {
    #[default]
    None,
    Move,
    Rotate,
    Scale,
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Handle {
    /// Translate along axis 0 = X, 1 = Y, 2 = Z.
    Axis(usize),
    /// Translate in the plane whose normal is the axis.
    Plane(usize),
    /// Rotate about the axis: 2 is the bed ring, 0 / 1 the tilt rings.
    Ring(usize),
    /// Scale along the axis.
    ScaleAxis(usize),
    Uniform,
}
pub const AXES: [Vec3; 3] = [Vec3::X, Vec3::Y, Vec3::Z];
const AXIS_COLORS: [Color32; 3] = [theme::AXIS_X, theme::AXIS_Y, theme::AXIS_Z];
/// Ticks around the ring: 72 of them (every 5°), longer every 15°, longest every 45°.
pub const TICK_DEG: f32 = 5.;
pub const TICK_MEDIUM_DEG: f32 = 15.;
pub const TICK_LONG_DEG: f32 = 45.;
/// Snap steps: on the tick band, in the inner zone (the spokes), with Shift.
pub const FINE_STEP_DEG: f32 = 5.;
pub const COARSE_STEP_DEG: f32 = 45.;
pub const SHIFT_STEP_DEG: f32 = 1.;
/// The inner snap zone spans these fractions of the ring radius.
pub const COARSE_IN: f32 = 1. / 3.;
pub const COARSE_OUT: f32 = 2. / 3.;
/// The tick band (fine snapping) spans these fractions of the ring radius.
pub const FINE_IN: f32 = 0.88;
pub const FINE_OUT: f32 = 1.13;
/// Below this |cos| between the pointer ray and the ring's normal the ring counts as seen
/// exactly edge-on (PrusaSlicer's `EPSILON` on the ray direction).
const EDGE_ON: f32 = 1e-4;
/// Points sampled around a ring for hit testing.
const RING_SAMPLES: usize = 256;

/// How a rotation drag snaps, from where the pointer is relative to the ring.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SnapZone {
    /// Outside the ticks (or well inside the spokes): no snapping.
    Free,
    /// On the tick band: 5° steps.
    #[default]
    Fine,
    /// Between a third and two thirds of the radius: 45° steps.
    Coarse,
    /// Shift held: 1° steps wherever the pointer is.
    Shift,
}
impl SnapZone {
    /// The zone for a pointer at `ratio` × ring radius from the centre.
    pub fn at(ratio: f32, shift: bool) -> Self {
        if shift {
            SnapZone::Shift
        } else if (COARSE_IN..=COARSE_OUT).contains(&ratio) {
            SnapZone::Coarse
        } else if (FINE_IN..=FINE_OUT).contains(&ratio) {
            SnapZone::Fine
        } else {
            SnapZone::Free
        }
    }
    /// The step in degrees, `None` for free rotation.
    pub fn step_deg(self) -> Option<f32> {
        match self {
            SnapZone::Free => None,
            SnapZone::Fine => Some(FINE_STEP_DEG),
            SnapZone::Coarse => Some(COARSE_STEP_DEG),
            SnapZone::Shift => Some(SHIFT_STEP_DEG),
        }
    }
    /// Text for the angle label.
    pub fn label(self) -> &'static str {
        match self {
            SnapZone::Free => "free",
            SnapZone::Fine => "snap 5°",
            SnapZone::Coarse => "snap 45°",
            SnapZone::Shift => "snap 1°",
        }
    }
}
/// Snaps the absolute angle `start + total` to the zone's step and returns the resulting
/// offset from `start` (radians), so snapped positions coincide with the spokes and ticks.
pub fn snap_offset(start: f32, total: f32, zone: SnapZone) -> f32 {
    match zone.step_deg() {
        None => total,
        Some(deg) => {
            let step = deg.to_radians();
            ((start + total) / step).round() * step - start
        }
    }
}

/// The in-plane axes (`u` at angle 0, `v` at +90°) of the ring about axis `i`, as PrusaSlicer's
/// `GLGizmoRotate::local_transform` lays them out: X from −Y towards −Z (the handle starts at
/// the front), Y from +Z towards +X (the handle starts on top), Z from +X towards +Y. `u × v` is
/// the rotation axis, so a positive angle turns right-handed about it. `frame` turns the whole
/// set (identity = world axes; an object's rotation = its local axes).
pub fn ring_axes(i: usize, frame: Quat) -> (Vec3, Vec3) {
    let (u, v) = match i {
        0 => (-Vec3::Y, -Vec3::Z),
        1 => (Vec3::Z, Vec3::X),
        _ => (Vec3::X, Vec3::Y),
    };
    (frame * u, frame * v)
}

/// One rotation ring: a circle of `radius` around `center` in the plane spanned by `u` and `v`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ring {
    pub axis: usize,
    pub center: Vec3,
    pub radius: f32,
    pub u: Vec3,
    pub v: Vec3,
    /// Where the grab handle rests (radians from `u`): the yaw for the bed ring, 0 for tilts.
    pub rest: f32,
}
impl Ring {
    /// The rotation axis (the plane's normal).
    pub fn normal(&self) -> Vec3 {
        self.u.cross(self.v)
    }
    /// A point in the ring's plane at `angle` and `ratio` × radius from the centre.
    pub fn point(&self, angle: f32, ratio: f32) -> Vec3 {
        self.center + (self.u * angle.cos() + self.v * angle.sin()) * (self.radius * ratio)
    }
    /// The pointer ray in the ring's local frame: (u, v, normal) coordinates of the origin
    /// relative to the centre, and of the direction.
    fn local_ray(&self, origin: Vec3, dir: Vec3) -> (Vec3, Vec3) {
        let n = self.normal();
        let a = origin - self.center;
        (
            Vec3::new(a.dot(self.u), a.dot(self.v), a.dot(n)),
            Vec3::new(dir.dot(self.u), dir.dot(self.v), dir.dot(n)),
        )
    }
    /// Where the pointer ray meets the ring's plane, in the ring's (u, v) coordinates
    /// (PrusaSlicer's `mouse_position_in_local_plane`). Seen exactly edge-on, or when the plane
    /// lies behind the camera, the ray point nearest the centre stands in for it: it lies on the
    /// line the ring appears as, under the pointer, so the angle still follows the pointer.
    pub fn local_point(&self, origin: Vec3, dir: Vec3) -> Vec2 {
        let (a, d) = self.local_ray(origin, dir);
        if d.z.abs() >= EDGE_ON {
            let t = -a.z / d.z;
            if t >= 0. {
                return (a + d * t).truncate();
            }
        }
        // Parallel to the plane and along the handle's tangent: angle 0, as upstream.
        let dd = d.length_squared();
        if dd < 1e-12 {
            return Vec2::X;
        }
        let t = (-a.dot(d) / dd).max(0.);
        let p = (a + d * t).truncate();
        if p.length_squared() < 1e-12 {
            Vec2::X
        } else {
            p
        }
    }
    /// Closest approach of the ray to the ring line (millimetres).
    pub fn ray_distance(&self, origin: Vec3, dir: Vec3) -> f32 {
        (0..RING_SAMPLES)
            .map(|k| {
                let a = k as f32 / RING_SAMPLES as f32 * std::f32::consts::TAU;
                ray_point_distance(origin, dir, self.point(a, 1.))
            })
            .fold(f32::INFINITY, f32::min)
    }
}

/// Geometry of a gizmo at `origin` (the object's pivot), with handle length `size` in
/// millimetres. The rotate mode also carries the bed ring and the tilt rings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layout {
    pub origin: Vec3,
    pub size: f32,
    pub mode: Mode,
    /// Centre of the Z ring: the pivot projected onto the bed plane.
    pub ring_center: Vec3,
    /// Radius of the Z ring, outside the footprint.
    pub ring_radius: f32,
    /// The object's current Z rotation (radians), where the Z grab handle rests.
    pub angle: f32,
    /// Radius of the X / Y rings around the pivot, outside the bounding sphere.
    pub tilt_radius: f32,
    /// Orientation of the rings' frame: identity for world axes (PrusaSlicer's default World
    /// coordinates, and Mica's only one), an object's rotation for its local axes.
    pub frame: Quat,
    /// Only the bed ring (PrusaSlicer disables the X / Y grabbers for the wipe tower).
    pub z_only: bool,
}
impl Layout {
    /// Plain layout for move / scale (and rotation about the pivot with default rings).
    pub fn new(origin: Vec3, size: f32, mode: Mode) -> Self {
        Self {
            origin,
            size,
            mode,
            ring_center: origin.with_z(0.),
            ring_radius: size,
            angle: 0.,
            tilt_radius: size,
            frame: Quat::IDENTITY,
            z_only: false,
        }
    }
    /// Rotate layout for an object whose vertices lie within `footprint_radius` of the
    /// vertical axis through `origin` (the pivot) and within `sphere_radius` of the pivot,
    /// turned by `angle` about Z. The bed ring is centred under the pivot and the tilt rings
    /// around it; each clears the object by `margin` mm and is never smaller than `size`. The
    /// inputs are invariant under rotation about the pivot, so the rings keep their size while
    /// the object turns (PrusaSlicer: bounding sphere + 5 mm).
    pub fn rotate(
        origin: Vec3,
        footprint_radius: f32,
        sphere_radius: f32,
        size: f32,
        angle: f32,
        margin: f32,
    ) -> Self {
        Self {
            origin,
            size,
            mode: Mode::Rotate,
            ring_center: origin.with_z(0.),
            ring_radius: (footprint_radius + margin).max(size),
            angle,
            tilt_radius: (sphere_radius + margin).max(size),
            frame: Quat::IDENTITY,
            z_only: false,
        }
    }
    /// Only the bed ring (the prime tower).
    pub fn z_only(mut self) -> Self {
        self.z_only = true;
        self
    }
    /// The ring about axis `i`.
    pub fn ring(&self, i: usize) -> Ring {
        let (u, v) = ring_axes(i, self.frame);
        if i == 2 {
            Ring {
                axis: 2,
                center: self.ring_center,
                radius: self.ring_radius,
                u,
                v,
                rest: self.angle,
            }
        } else {
            Ring {
                axis: i,
                center: self.origin,
                radius: self.tilt_radius,
                u,
                v,
                rest: 0.,
            }
        }
    }
    /// The rings that can be dragged.
    pub fn rings(&self) -> Vec<Ring> {
        let first = if self.z_only { 2 } else { 0 };
        (first..3).map(|i| self.ring(i)).collect()
    }
    /// `pick_radius` is the hit tolerance in millimetres (a few pixels at the gizmo's depth).
    pub fn hit(&self, origin: Vec3, dir: Vec3, pick_radius: f32) -> Option<Handle> {
        let mut best: Option<(f32, Handle)> = None;
        let mut consider = |d: f32, h: Handle| {
            if d <= pick_radius && best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, h));
            }
        };
        match self.mode {
            Mode::None => return None,
            Mode::Move | Mode::Scale => {
                if self.mode == Mode::Scale {
                    // Checked first so a ray down the Z handle still prefers the centre cube.
                    consider(
                        ray_point_distance(origin, dir, self.origin),
                        Handle::Uniform,
                    );
                }
                for (i, axis) in AXES.iter().enumerate() {
                    let a = self.origin + *axis * self.size * 0.2;
                    let b = self.origin + *axis * self.size;
                    let d = ray_segment_distance(origin, dir, a, b);
                    consider(
                        d,
                        if self.mode == Mode::Move {
                            Handle::Axis(i)
                        } else {
                            Handle::ScaleAxis(i)
                        },
                    );
                }
                if self.mode == Mode::Move {
                    for (i, axis) in AXES.iter().enumerate() {
                        let (j, k) = ((i + 1) % 3, (i + 2) % 3);
                        if let Some(p) = ray_plane(origin, dir, self.origin, *axis) {
                            let l = p - self.origin;
                            let (u, v) = (l.dot(AXES[j]) / self.size, l.dot(AXES[k]) / self.size);
                            if (0.3..=0.6).contains(&u) && (0.3..=0.6).contains(&v) {
                                consider(0., Handle::Plane(i));
                            }
                        }
                    }
                }
            }
            Mode::Rotate => {
                // The grab handles are the easiest targets, then the rings themselves (the
                // closest approach of the ray, so an edge-on ring is as easy to hit as its line).
                for ring in self.rings() {
                    let h = Handle::Ring(ring.axis);
                    consider(
                        ray_point_distance(origin, dir, ring.point(ring.rest, 1.)) * 0.5,
                        h,
                    );
                    consider(ring.ray_distance(origin, dir), h);
                }
            }
        }
        best.map(|(_, h)| h)
    }
}

/// Active drag: converts successive world rays into transform deltas. The layout is the one
/// the drag began with, so ring radii and centres stay put until release.
#[derive(Clone, Debug, PartialEq)]
pub struct Drag {
    pub handle: Handle,
    layout: Layout,
    start: f32,
    start_plane: Vec3,
    /// Raw accumulated value (mm, radians or factor) since the drag began.
    total: f32,
    total_plane: Vec3,
    /// What has been emitted so far, so snapping emits exact increments.
    emitted: f32,
    emitted_plane: Vec3,
    /// Snap zone of the last pointer position (rotation only).
    zone: SnapZone,
}
impl Drag {
    pub fn begin(layout: Layout, handle: Handle, origin: Vec3, dir: Vec3) -> Option<Self> {
        let (start, start_plane) = Self::measure(&layout, handle, origin, dir)?;
        let one = matches!(handle, Handle::ScaleAxis(_) | Handle::Uniform);
        Some(Self {
            handle,
            layout,
            start,
            start_plane,
            total: if one { 1. } else { 0. },
            total_plane: Vec3::ZERO,
            emitted: if one { 1. } else { 0. },
            emitted_plane: Vec3::ZERO,
            zone: SnapZone::Fine,
        })
    }
    /// The geometry frozen at the start of the drag.
    pub fn layout(&self) -> &Layout {
        &self.layout
    }
    /// The snap zone the pointer was last in (rotation drags).
    pub fn zone(&self) -> SnapZone {
        self.zone
    }
    /// Returns the measured value and, for rings, the pointer's (u, v) position in the ring's
    /// plane relative to its centre (its length picks the snap zone).
    fn measure(layout: &Layout, handle: Handle, origin: Vec3, dir: Vec3) -> Option<(f32, Vec3)> {
        match handle {
            Handle::Axis(i) | Handle::ScaleAxis(i) => {
                let t = ray_line_parameter(origin, dir, layout.origin, AXES[i])?;
                Some((t, Vec3::ZERO))
            }
            Handle::Plane(i) => {
                let p = ray_plane(origin, dir, layout.origin, AXES[i])?;
                Some((0., p))
            }
            Handle::Ring(i) => {
                let p = layout.ring(i).local_point(origin, dir);
                Some((p.y.atan2(p.x), p.extend(0.)))
            }
            Handle::Uniform => {
                // Distance from the origin in the view plane through the origin.
                let p = ray_plane(origin, dir, layout.origin, dir)?;
                Some(((p - layout.origin).length().max(1e-3), Vec3::ZERO))
            }
        }
    }
    /// Feeds the current pointer ray; returns the delta to apply (already snapped if asked).
    /// `shift` snaps moves to 1 mm, scales to 5 % and rotations to 1°; without it a rotation
    /// snaps according to the pointer's [`SnapZone`].
    pub fn update(&mut self, origin: Vec3, dir: Vec3, shift: bool) -> Option<Delta> {
        let (now, now_plane) = Self::measure(&self.layout, self.handle, origin, dir)?;
        match self.handle {
            Handle::Axis(i) => {
                self.total = now - self.start;
                let target = if shift {
                    self.total.round()
                } else {
                    self.total
                };
                let step = target - self.emitted;
                self.emitted = target;
                (step.abs() > 1e-6).then(|| Delta::Translate(AXES[i] * step))
            }
            Handle::Plane(_) => {
                self.total_plane = now_plane - self.start_plane;
                let target = if shift {
                    self.total_plane.round()
                } else {
                    self.total_plane
                };
                let step = target - self.emitted_plane;
                self.emitted_plane = target;
                (step.length() > 1e-6).then_some(Delta::Translate(step))
            }
            Handle::Ring(i) => {
                let ring = self.layout.ring(i);
                let mut d = now - self.start;
                // Unwrap relative to the previous total so the angle keeps turning past ±π.
                while d - self.total > std::f32::consts::PI {
                    d -= std::f32::consts::TAU;
                }
                while d - self.total < -std::f32::consts::PI {
                    d += std::f32::consts::TAU;
                }
                self.total = d;
                self.zone = SnapZone::at(now_plane.length() / ring.radius.max(1e-6), shift);
                // Snap the absolute angle in the ring's frame, so the handle lands on the
                // ticks and spokes (upstream snaps the angle measured from the ring's u axis).
                let target = snap_offset(ring.rest, self.total, self.zone);
                let step = target - self.emitted;
                self.emitted = target;
                (step.abs() > 1e-6).then_some(Delta::Rotate {
                    axis: ring.normal(),
                    angle: step,
                })
            }
            Handle::ScaleAxis(i) => {
                let factor = if self.start.abs() > 1e-4 {
                    (now / self.start).max(0.01)
                } else {
                    1.
                };
                self.scale_step(factor, shift).map(|f| {
                    let mut s = Vec3::ONE;
                    s[i] = f;
                    Delta::Scale(s)
                })
            }
            Handle::Uniform => {
                let factor = (now / self.start).max(0.01);
                self.scale_step(factor, shift)
                    .map(|f| Delta::Scale(Vec3::splat(f)))
            }
        }
    }
    fn scale_step(&mut self, factor: f32, snap: bool) -> Option<f32> {
        self.total = factor;
        let target = if snap {
            ((factor / 0.05).round() * 0.05).max(0.05)
        } else {
            factor
        };
        let step = target / self.emitted;
        self.emitted = target;
        ((step - 1.).abs() > 1e-6).then_some(step)
    }
    /// Angle turned so far (radians), for the sweep arc and the label.
    pub fn turned(&self) -> f32 {
        self.emitted
    }
    /// Human-readable value for the on-screen read-out.
    pub fn feedback(&self) -> String {
        match self.handle {
            Handle::Axis(i) => {
                format!("Δ{} {:+.1} mm", "XYZ".chars().nth(i).unwrap(), self.emitted)
            }
            Handle::Plane(_) => {
                let d = self.emitted_plane;
                format!("ΔX {:+.1}  ΔY {:+.1}  ΔZ {:+.1} mm", d.x, d.y, d.z)
            }
            Handle::Ring(i) => format!(
                "{} {:+.0}°",
                "XYZ".chars().nth(i).unwrap(),
                self.emitted.to_degrees()
            ),
            Handle::ScaleAxis(i) => format!(
                "Scale {} ×{:.2}",
                "XYZ".chars().nth(i).unwrap(),
                self.emitted
            ),
            Handle::Uniform => format!("Scale ×{:.2} ({:.0}%)", self.emitted, self.emitted * 100.),
        }
    }
}

/// Wraps an angle in degrees to (−180, 180].
pub fn wrap_degrees(deg: f32) -> f32 {
    let mut d = deg % 360.;
    if d > 180. {
        d -= 360.;
    } else if d <= -180. {
        d += 360.;
    }
    if d == -0. { 0. } else { d }
}

// Geometry helpers -----------------------------------------------------------------------------
pub fn ray_plane(origin: Vec3, dir: Vec3, point: Vec3, normal: Vec3) -> Option<Vec3> {
    let denom = dir.dot(normal);
    if denom.abs() < 1e-6 {
        return None;
    }
    let t = (point - origin).dot(normal) / denom;
    (t >= 0.).then(|| origin + dir * t)
}
/// Parameter along `axis` of the point on the line `point + t·axis` closest to the ray.
pub fn ray_line_parameter(origin: Vec3, dir: Vec3, point: Vec3, axis: Vec3) -> Option<f32> {
    let w = origin - point;
    let b = dir.dot(axis);
    let denom = 1. - b * b;
    if denom.abs() < 1e-6 {
        return None;
    }
    let d = w.dot(dir);
    let e = w.dot(axis);
    // Closest points between two lines with unit directions: t on the line.
    Some((e - b * d) / denom)
}
pub fn ray_point_distance(origin: Vec3, dir: Vec3, p: Vec3) -> f32 {
    let t = (p - origin).dot(dir).max(0.);
    (origin + dir * t).distance(p)
}
pub fn ray_segment_distance(origin: Vec3, dir: Vec3, a: Vec3, b: Vec3) -> f32 {
    let axis = b - a;
    let len = axis.length();
    if len < 1e-6 {
        return ray_point_distance(origin, dir, a);
    }
    let axis = axis / len;
    let t = ray_line_parameter(origin, dir, a, axis)
        .unwrap_or(0.)
        .clamp(0., len);
    ray_point_distance(origin, dir, a + axis * t)
}

// Drawing -------------------------------------------------------------------------------------
pub struct Painter<'a> {
    pub painter: &'a egui::Painter,
    pub rect: Rect,
    pub camera: &'a Camera,
}
/// What the rotate gizmo shows besides its geometry.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RotateState {
    /// Sweep of the dragged ring from the start of the drag, radians (None when no ring is
    /// being dragged).
    pub sweep: Option<f32>,
    /// The snap zone in effect (the pointer's while dragging, Shift / default otherwise).
    pub zone: SnapZone,
}
/// The light tint of an axis colour, for the tick scale.
fn tick_color(axis: usize) -> Color32 {
    if axis == 2 {
        return theme::AXIS_Z_LIGHT;
    }
    let c = AXIS_COLORS[axis];
    let mix = |a: u8| (a as f32 + (255. - a as f32) * 0.45) as u8;
    Color32::from_rgb(mix(c.r()), mix(c.g()), mix(c.b()))
}
impl Painter<'_> {
    pub fn project(&self, p: Vec3) -> Option<Pos2> {
        let aspect = self.rect.width() / self.rect.height().max(1.);
        self.camera.project(p, aspect).map(|uv| {
            Pos2::new(
                self.rect.left() + uv.x * self.rect.width(),
                self.rect.top() + uv.y * self.rect.height(),
            )
        })
    }
    /// A line with a dark outline underneath.
    fn outlined_line(&self, a: Vec3, b: Vec3, width: f32, color: Color32) {
        if let (Some(pa), Some(pb)) = (self.project(a), self.project(b)) {
            self.painter
                .line_segment([pa, pb], Stroke::new(width + 4., theme::OUTLINE));
            self.painter
                .line_segment([pa, pb], Stroke::new(width, color));
        }
    }
    fn arrow_head(&self, tip: Vec3, back: Vec3, color: Color32) {
        if let (Some(pt), Some(pb)) = (self.project(tip), self.project(back)) {
            let d = (pt - pb).normalized();
            let n = EVec2::new(-d.y, d.x);
            let pts = vec![pt + d * 8., pt - d * 8. + n * 7., pt - d * 8. - n * 7.];
            self.painter.add(egui::Shape::convex_polygon(
                pts.clone(),
                color,
                Stroke::new(2., theme::OUTLINE),
            ));
        }
    }
    fn cube(&self, at: Vec3, size: f32, color: Color32) {
        if let Some(p) = self.project(at) {
            let r = Rect::from_center_size(p, EVec2::splat(size));
            self.painter.rect_filled(r.expand(1.5), 3., theme::OUTLINE);
            self.painter.rect_filled(r, 2., color);
        }
    }
    /// Polyline through world points; `None` points break the line.
    fn polyline(&self, pts: &[Vec3], stroke: Stroke) {
        let mut run: Vec<Pos2> = Vec::with_capacity(pts.len());
        for p in pts {
            match self.project(*p) {
                Some(q) => run.push(q),
                None => {
                    if run.len() > 1 {
                        self.painter
                            .add(egui::Shape::line(std::mem::take(&mut run), stroke));
                    }
                    run.clear();
                }
            }
        }
        if run.len() > 1 {
            self.painter.add(egui::Shape::line(run, stroke));
        }
    }
    pub fn draw(
        &self,
        layout: &Layout,
        hovered: Option<Handle>,
        active: Option<Handle>,
        rotate: RotateState,
    ) {
        let o = layout.origin;
        let s = layout.size;
        let color = |i: usize, h: Handle| {
            if active == Some(h) || (active.is_none() && hovered == Some(h)) {
                theme::ACCENT_BRIGHT
            } else if active.is_some() {
                AXIS_COLORS[i].gamma_multiply(0.35)
            } else {
                AXIS_COLORS[i]
            }
        };
        match layout.mode {
            Mode::None => {}
            Mode::Move => {
                for i in 0..3 {
                    let (j, k) = ((i + 1) % 3, (i + 2) % 3);
                    let h = Handle::Plane(i);
                    let c = color(i, h);
                    let quad = [
                        o + (AXES[j] + AXES[k]) * s * 0.3,
                        o + AXES[j] * s * 0.6 + AXES[k] * s * 0.3,
                        o + (AXES[j] + AXES[k]) * s * 0.6,
                        o + AXES[j] * s * 0.3 + AXES[k] * s * 0.6,
                    ];
                    let pts: Vec<_> = quad.iter().filter_map(|p| self.project(*p)).collect();
                    if pts.len() == 4 {
                        self.painter.add(egui::Shape::convex_polygon(
                            pts,
                            c.gamma_multiply(if hovered == Some(h) { 0.65 } else { 0.35 }),
                            Stroke::new(1.5, c),
                        ));
                    }
                }
                for (i, axis) in AXES.iter().enumerate() {
                    let h = Handle::Axis(i);
                    let c = color(i, h);
                    let tip = o + *axis * s;
                    self.outlined_line(o, o + *axis * s * 0.88, 4., c);
                    self.arrow_head(tip, o + *axis * s * 0.8, c);
                }
            }
            Mode::Rotate => self.draw_rotate(layout, hovered, active, rotate),
            Mode::Scale => {
                for (i, axis) in AXES.iter().enumerate() {
                    let h = Handle::ScaleAxis(i);
                    let c = color(i, h);
                    let tip = o + *axis * s;
                    self.outlined_line(o, tip, 4., c);
                    self.cube(tip, 12., c);
                }
                let h = Handle::Uniform;
                let c = if hovered == Some(h) || active == Some(h) {
                    theme::ACCENT_BRIGHT
                } else {
                    Color32::from_gray(225)
                };
                self.cube(o, 13., c);
            }
        }
        if layout.mode != Mode::Rotate
            && let Some(p) = self.project(o)
        {
            self.painter.circle(
                p,
                3.5,
                Color32::from_gray(235),
                Stroke::new(1.5, theme::OUTLINE),
            );
        }
    }
    /// Where ring `ring`'s handle is drawn: at rest, or moved by the sweep while dragged.
    pub fn handle_angle(ring: &Ring, active: Option<Handle>, rotate: RotateState) -> f32 {
        match rotate.sweep {
            Some(sweep) if active == Some(Handle::Ring(ring.axis)) => ring.rest + sweep,
            _ => ring.rest,
        }
    }
    fn draw_rotate(
        &self,
        layout: &Layout,
        hovered: Option<Handle>,
        active: Option<Handle>,
        rotate: RotateState,
    ) {
        let hot_ring = match active.or(hovered) {
            Some(Handle::Ring(i)) => Some(i),
            _ => None,
        };
        // The hot ring last, on top of the others.
        let mut rings = layout.rings();
        rings.sort_by_key(|r| Some(r.axis) == hot_ring);
        for ring in &rings {
            let h = Handle::Ring(ring.axis);
            // While one ring is dragged the others are hidden, as in PrusaSlicer.
            if active.is_some() && active != Some(h) {
                continue;
            }
            self.draw_ring(ring, active, hovered, hot_ring, rotate);
        }
    }
    fn draw_ring(
        &self,
        ring: &Ring,
        active: Option<Handle>,
        hovered: Option<Handle>,
        hot_ring: Option<usize>,
        rotate: RotateState,
    ) {
        let view = self.camera.direction();
        let h = Handle::Ring(ring.axis);
        let is_active = active == Some(h);
        let is_hot = is_active || (active.is_none() && hovered == Some(h));
        // Another ring under the pointer fades this one.
        let fade = if hot_ring.is_some() && !is_hot {
            0.4
        } else {
            1.
        };
        let axis_color = AXIS_COLORS[ring.axis];
        let line = if is_active {
            theme::ACCENT_BRIGHT
        } else {
            axis_color
        }
        .gamma_multiply(fade);
        // The circle: dark outline, dimmed back half, full front half.
        let n = 144;
        let circle: Vec<Vec3> = (0..=n)
            .map(|k| ring.point(k as f32 / n as f32 * std::f32::consts::TAU, 1.))
            .collect();
        self.polyline(
            &circle,
            Stroke::new(6.5, theme::OUTLINE.gamma_multiply(0.9 * fade)),
        );
        let is_front = |p: &Vec3| (*p - ring.center).dot(view) >= 0.;
        let mut runs: Vec<(bool, Vec<Vec3>)> = Vec::new();
        for p in &circle {
            let f = is_front(p);
            match runs.last_mut() {
                Some((rf, run)) if *rf == f => run.push(*p),
                _ => {
                    // Start a new run sharing the boundary point so there is no gap.
                    let mut run = Vec::new();
                    if let Some((_, prev)) = runs.last()
                        && let Some(last) = prev.last()
                    {
                        run.push(*last);
                    }
                    run.push(*p);
                    runs.push((f, run));
                }
            }
        }
        let width = if is_hot { 3. } else { 2.5 };
        for (front, run) in &runs {
            if !front {
                self.polyline(run, Stroke::new(width, line.gamma_multiply(0.45)));
            }
        }
        for (front, run) in &runs {
            if *front {
                self.polyline(run, Stroke::new(width, line));
            }
        }
        // The tick scale outside the ring: every 5°, longer every 15°, longest every 45°. The
        // bed ring always shows it; a tilt ring when it is hovered or dragged (PrusaSlicer shows
        // the scale of the hovered ring only).
        if ring.axis == 2 || is_hot {
            let tick = tick_color(ring.axis).gamma_multiply(fade);
            for k in 0..(360. / TICK_DEG) as usize {
                let deg = k as f32 * TICK_DEG;
                let (len, width) = if deg % TICK_LONG_DEG == 0. {
                    (FINE_OUT - 1., 2.)
                } else if deg % TICK_MEDIUM_DEG == 0. {
                    (0.085, 1.5)
                } else {
                    (0.045, 1.2)
                };
                let a = deg.to_radians();
                let inner = ring.point(a, 1.);
                let outer = ring.point(a, 1. + len);
                let alpha = if is_front(&inner) { 1. } else { 0.4 };
                self.polyline(
                    &[inner, outer],
                    Stroke::new(width, tick.gamma_multiply(alpha)),
                );
            }
        }
        let handle = Self::handle_angle(ring, active, rotate);
        // While dragging: the spokes of the inner snap zone, the reference radius at the drag's
        // start, the radius to the handle and the swept arc, as PrusaSlicer shows them.
        if is_active && let Some(sweep) = rotate.sweep {
            let coarse = rotate.zone == SnapZone::Coarse;
            let spoke = if coarse {
                Stroke::new(2., Color32::from_gray(240).gamma_multiply(0.95))
            } else {
                Stroke::new(1.2, Color32::from_gray(225).gamma_multiply(0.5))
            };
            for k in 0..(360. / COARSE_STEP_DEG) as usize {
                let a = (k as f32 * COARSE_STEP_DEG).to_radians();
                self.polyline(
                    &[ring.point(a, COARSE_IN), ring.point(a, COARSE_OUT)],
                    spoke,
                );
            }
            let bound = Stroke::new(
                1.,
                Color32::from_gray(225).gamma_multiply(if coarse { 0.35 } else { 0.18 }),
            );
            for ratio in [COARSE_IN, COARSE_OUT] {
                let c: Vec<Vec3> = (0..=96)
                    .map(|k| ring.point(k as f32 / 96. * std::f32::consts::TAU, ratio))
                    .collect();
                self.polyline(&c, bound);
            }
            self.polyline(
                &[ring.center, ring.point(ring.rest, 1.)],
                Stroke::new(1., Color32::from_gray(225).gamma_multiply(0.35)),
            );
            self.polyline(
                &[ring.center, ring.point(handle, 1.)],
                Stroke::new(1.5, theme::HANDLE_HOT.gamma_multiply(0.8)),
            );
            if sweep.abs() > 1e-4 {
                let steps = ((sweep.abs() / 0.03).ceil() as usize).clamp(2, 400);
                let arc: Vec<Vec3> = (0..=steps)
                    .map(|k| ring.point(ring.rest + sweep * k as f32 / steps as f32, 1.))
                    .collect();
                self.polyline(
                    &arc,
                    Stroke::new(7., theme::HANDLE_HOT.gamma_multiply(0.45)),
                );
            }
        }
        // The grab handle with a two-way arrow along the ring; yellow when hot.
        let at = ring.point(handle, 1.);
        if let Some(p) = self.project(at) {
            let r = if is_hot { 12. } else { 11. };
            self.painter.circle(
                p,
                r,
                if is_hot {
                    theme::HANDLE_HOT
                } else {
                    axis_color.gamma_multiply(fade)
                },
                Stroke::new(3., theme::OUTLINE.gamma_multiply(fade)),
            );
            // The ring's direction on screen at the handle (vertical when it is degenerate).
            let t = match (
                self.project(ring.point(handle + 0.05, 1.)),
                self.project(ring.point(handle - 0.05, 1.)),
            ) {
                (Some(a), Some(b)) if (a - b).length() > 0.5 => (a - b).normalized(),
                _ => EVec2::new(0., -1.),
            };
            let n = EVec2::new(-t.y, t.x);
            let w = Stroke::new(
                1.8,
                if is_hot {
                    theme::OUTLINE
                } else {
                    Color32::WHITE.gamma_multiply(fade)
                },
            );
            for s in [1., -1.] {
                let tip = p + t * (7. * s);
                self.painter.add(egui::Shape::line(
                    vec![
                        tip - t * (4. * s) + n * 5.,
                        tip,
                        tip - t * (4. * s) - n * 5.,
                    ],
                    w,
                ));
            }
        }
    }
    /// The live angle label (`Z  +45°   snap 45°`) next to ring `ring`'s handle at
    /// `handle` (radians), kept inside the view. Returns the label's rect for double-click hit
    /// testing.
    pub fn angle_label(
        &self,
        ring: &Ring,
        handle: f32,
        angle_deg: f32,
        zone: SnapZone,
        hot: bool,
    ) -> Option<Rect> {
        let p = self.project(ring.point(handle, 1.))?;
        let value = format!(
            "{}  {:+.0}°",
            "XYZ".chars().nth(ring.axis).unwrap_or('Z'),
            wrap_degrees(angle_deg)
        );
        let snap = zone.label().to_string();
        let font = theme::mono(13.);
        let g1 = self
            .painter
            .layout_no_wrap(value, font.clone(), theme::DARK.text);
        let g2 = self.painter.layout_no_wrap(snap, font, theme::DARK.muted);
        let size = EVec2::new(g1.size().x + 12. + g2.size().x + 16., g1.size().y + 8.);
        let mut r = Rect::from_min_size(p + EVec2::new(18., 14.), size);
        r = r.translate(super::clamp_shift(self.rect, r));
        self.painter.rect_filled(r, 6., theme::DARK.dark_box);
        self.painter.rect_stroke(
            r,
            6.,
            Stroke::new(
                1.,
                if hot {
                    theme::HANDLE_HOT
                } else {
                    AXIS_COLORS[ring.axis]
                },
            ),
            egui::StrokeKind::Inside,
        );
        self.painter
            .galley(r.min + EVec2::new(8., 4.), g1.clone(), theme::DARK.text);
        self.painter.galley(
            Pos2::new(r.left() + 8. + g1.size().x + 12., r.top() + 4.),
            g2,
            theme::DARK.muted,
        );
        Some(r)
    }
    /// Small read-out for move / scale drags, next to the pointer.
    pub fn readout(&self, at: Pos2, text: &str) {
        let g = self
            .painter
            .layout_no_wrap(text.to_string(), theme::mono(12.), theme::DARK.text);
        let mut r = Rect::from_min_size(at, g.size() + EVec2::new(16., 8.));
        r = r.translate(super::clamp_shift(self.rect, r));
        self.painter.rect_filled(r, 6., theme::DARK.dark_box);
        self.painter.rect_stroke(
            r,
            6.,
            Stroke::new(1., theme::DARK.border_raised),
            egui::StrokeKind::Inside,
        );
        self.painter
            .galley(r.min + EVec2::new(8., 4.), g, theme::DARK.text);
        let _ = Align2::LEFT_TOP;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::{FRAC_PI_2, PI};
    fn layout(mode: Mode) -> Layout {
        Layout::new(Vec3::new(10., 20., 5.), 30., mode)
    }
    /// A ray looking straight down at `p`.
    fn down(p: Vec3) -> (Vec3, Vec3) {
        (p + Vec3::Z * 100., -Vec3::Z)
    }
    /// A perspective-style ray from `eye` through `p`.
    fn through(eye: Vec3, p: Vec3) -> (Vec3, Vec3) {
        (eye, (p - eye).normalize())
    }
    /// The pointer's angle in the ring's frame and its distance as a fraction of the radius.
    fn pointer(r: &Ring, origin: Vec3, dir: Vec3) -> (f32, f32) {
        let p = r.local_point(origin, dir);
        (p.y.atan2(p.x), p.length() / r.radius)
    }
    fn deg(x: f32) -> f32 {
        x.to_radians()
    }
    /// Angle difference wrapped to (−π, π].
    fn ang_diff(a: f32, b: f32) -> f32 {
        let mut d = (a - b) % std::f32::consts::TAU;
        if d > PI {
            d -= std::f32::consts::TAU;
        } else if d <= -PI {
            d += std::f32::consts::TAU;
        }
        d
    }
    /// Rings of radius 30 (bed) and 40 (tilts) around the pivot (0, 0, 5), Z handle at `angle`.
    fn ring(angle: f32) -> Layout {
        Layout::rotate(Vec3::new(0., 0., 5.), 22., 32., 30., angle, 8.)
    }
    /// Camera positions around the pivot: 3/4 views from front-left and back-right, a low
    /// front view, a low side view, a near-top view.
    const EYES: [Vec3; 5] = [
        Vec3::new(-220., -260., 240.),
        Vec3::new(260., 240., 180.),
        Vec3::new(30., -380., 40.),
        Vec3::new(380., 50., 60.),
        Vec3::new(20., -40., 400.),
    ];
    #[test]
    fn hits_axes_planes_and_rings() {
        let l = layout(Mode::Move);
        let (o, d) = down(l.origin + Vec3::X * 20. + Vec3::Y * 0.5);
        assert_eq!(l.hit(o, d, 2.), Some(Handle::Axis(0)));
        let (o, d) = down(l.origin + Vec3::new(13., 14., 0.));
        assert_eq!(l.hit(o, d, 2.), Some(Handle::Plane(2)));
        let (o, d) = down(l.origin + Vec3::new(20., 20., 0.));
        assert_eq!(l.hit(o, d, 2.), None);
        // Rotate: the bed ring lies under the pivot, footprint radius + margin wide; the tilt
        // rings around the pivot, sphere radius + margin wide.
        let r = Layout::rotate(Vec3::new(10., 20., 5.), 22., 32., 30., 0., 8.);
        assert_eq!(r.ring_center, Vec3::new(10., 20., 0.));
        assert!((r.ring_radius - 30.).abs() < 1e-5, "{}", r.ring_radius);
        assert!((r.tilt_radius - 40.).abs() < 1e-5, "{}", r.tilt_radius);
        // ... but never smaller than the handle size.
        let small = Layout::rotate(Vec3::ZERO, 2., 3., 30., 0., 3.);
        assert_eq!((small.ring_radius, small.tilt_radius), (30., 30.));
        let (o, d) = down(r.ring_center + Vec3::Y * r.ring_radius);
        assert_eq!(r.hit(o, d, 2.), Some(Handle::Ring(2)));
        // The Z handle sits at the current angle on the bed ring.
        assert!(
            r.ring(2)
                .point(0., 1.)
                .abs_diff_eq(Vec3::new(40., 20., 0.), 1e-5)
        );
        // The X ring is a full circle in the YZ plane through the pivot: hittable all round.
        let eye = r.origin + Vec3::new(300., 0., 0.);
        for a in [0., 1., 2.5, 4.] {
            let p = r.ring(0).point(a, 1.);
            let (o, d) = through(eye, p);
            assert_eq!(r.hit(o, d, 1.), Some(Handle::Ring(0)), "{a}");
        }
        // Seen exactly edge-on (from the front, along Y) the X ring is a line, still hittable.
        let p = r.ring(0).point(deg(60.), 1.);
        let (o, d) = (p - Vec3::Y * 300., Vec3::Y);
        assert_eq!(r.hit(o, d, 1.), Some(Handle::Ring(0)));
        // The prime tower has only the bed ring.
        let t = r.z_only();
        assert_eq!(t.rings().len(), 1);
        let p = r.ring(0).point(deg(30.), 1.);
        let (o, d) = through(eye, p);
        assert_ne!(t.hit(o, d, 1.), Some(Handle::Ring(0)));
        let s = layout(Mode::Scale);
        let (o, d) = down(s.origin);
        assert_eq!(s.hit(o, d, 2.), Some(Handle::Uniform));
        let (o, d) = down(s.origin + Vec3::Y * 25.);
        assert_eq!(s.hit(o, d, 2.), Some(Handle::ScaleAxis(1)));
        assert_eq!(layout(Mode::None).hit(o, d, 100.), None);
    }
    #[test]
    fn ring_frames_follow_prusaslicer() {
        // local_transform: X ring from −Y towards −Z, Y ring from +Z towards +X, Z from +X to +Y;
        // u × v is the axis in each case, so positive angles turn right-handed about it.
        let l = ring(0.);
        for (i, axis) in AXES.iter().enumerate() {
            let r = l.ring(i);
            assert!(r.normal().abs_diff_eq(*axis, 1e-6), "{i}");
            assert!(r.u.dot(r.v).abs() < 1e-6);
        }
        let x = l.ring(0);
        assert!(x.point(0., 1.).abs_diff_eq(l.origin - Vec3::Y * 40., 1e-4));
        assert!(
            x.point(FRAC_PI_2, 1.)
                .abs_diff_eq(l.origin - Vec3::Z * 40., 1e-4)
        );
        let y = l.ring(1);
        assert!(y.point(0., 1.).abs_diff_eq(l.origin + Vec3::Z * 40., 1e-4));
        // Tilt handles rest at 0 (PrusaSlicer resets the angle for every drag); the bed
        // handle at the yaw.
        assert_eq!((x.rest, y.rest), (0., 0.));
        assert_eq!(ring(0.7).ring(2).rest, 0.7);
        // A local frame turns all three rings with it.
        let q = Quat::from_euler(glam::EulerRot::ZYX, 0.6, -0.4, 0.9);
        let mut lf = l;
        lf.frame = q;
        for (i, axis) in AXES.iter().enumerate() {
            assert!(lf.ring(i).normal().abs_diff_eq(q * *axis, 1e-5), "{i}");
        }
    }
    #[test]
    fn pointer_angle_from_ray_plane_intersection_for_every_ring_and_view() {
        // mouse_position_in_local_plane: a ray through the point at angle a and ratio k of a
        // ring gives back a and k, from any camera that does not see the ring edge-on.
        let mut checked = 0;
        for frame in [
            Quat::IDENTITY,
            // The object already turned (a local frame): same mapping in the turned planes.
            Quat::from_euler(glam::EulerRot::ZYX, 0.8, 0.3, -0.5),
        ] {
            let mut l = ring(0.4);
            l.frame = frame;
            for i in 0..3 {
                let r = l.ring(i);
                for eye in EYES {
                    let along = (r.center - eye).normalize().dot(r.normal()).abs();
                    for a in [-170., -90., -20., 0., 15., 45., 100., 179.] {
                        for k in [0.5, 1., 1.4] {
                            let p = r.point(deg(a), k);
                            let (o, d) = through(eye, p);
                            let (got, ratio) = pointer(&r, o, d);
                            // Grazing views amplify float noise; still well under a degree.
                            let tol = if along < 0.2 { 0.2 } else { 0.01 };
                            assert!(
                                ang_diff(got, deg(a)).abs().to_degrees() < tol,
                                "ring {i} eye {eye} a {a} k {k}: {}",
                                got.to_degrees()
                            );
                            assert!((ratio - k).abs() < 1e-2, "{ratio} vs {k}");
                            checked += 1;
                        }
                    }
                }
            }
        }
        assert_eq!(checked, 2 * 3 * EYES.len() * 8 * 3);
    }
    #[test]
    fn near_edge_on_and_edge_on() {
        let l = ring(0.);
        let x = l.ring(0);
        // Nearly edge-on: the eye 0.5° off the X ring's plane. The intersection still lands
        // on the pointed-at point.
        let eye = x.center + Vec3::new(400. * deg(0.5).sin(), -400., 0.);
        for a in [10., 80., 135., -60.] {
            let (o, d) = through(eye, x.point(deg(a), 1.));
            let (got, ratio) = pointer(&x, o, d);
            assert!(
                ang_diff(got, deg(a)).abs() < deg(0.5),
                "{a}: {}",
                got.to_degrees()
            );
            assert!((ratio - 1.).abs() < 0.02, "{ratio}");
        }
        // Exactly edge-on (an orthographic front view along +Y): the ring is a vertical line
        // and the ray point nearest the centre stands in, so the angle follows the pointer's
        // height: h above the centre is c + Z·h, which is (0, −h) in (u, v) = (−Y, −Z).
        for h in [12., 30., -25.] {
            let (o, d) = (x.center + Vec3::new(0., -400., h), Vec3::Y);
            let p = x.local_point(o, d);
            assert!(p.abs_diff_eq(Vec2::new(0., -h), 1e-3), "{p}");
        }
        // Straight at the centre along the handle direction: angle 0, as upstream.
        let (o, d) = (x.center + Vec3::new(0., -400., 0.), Vec3::Y);
        assert_eq!(x.local_point(o, d), Vec2::X);
        // The plane behind the camera: the eye's own projection, not a mirrored intersection.
        let (o, d) = (
            x.center + Vec3::new(50., 0., 20.),
            Vec3::new(1., 0., 0.2).normalize(),
        );
        let p = x.local_point(o, d);
        assert!(p.abs_diff_eq(Vec2::new(0., -20.), 1e-3), "{p}");
        // A drag along the edge-on line: from the top to below the centre is half a turn.
        let top = x.center + Vec3::new(0., -400., 38.);
        let mut drag = Drag::begin(l, Handle::Ring(0), top, Vec3::Y).unwrap();
        assert!(drag.update(top + Vec3::Z * 0.5, Vec3::Y, false).is_none());
        let bottom = x.center + Vec3::new(0., -400., -38.);
        match drag.update(bottom, Vec3::Y, false) {
            Some(Delta::Rotate { axis, angle }) => {
                assert_eq!(axis, Vec3::X);
                assert!((angle.abs() - PI).abs() < 1e-3, "{angle}");
            }
            other => panic!("{other:?}"),
        }
    }
    #[test]
    fn tilt_ring_drags_snap_like_the_bed_ring_from_a_three_quarter_view() {
        let eye = EYES[0];
        for i in 0..2 {
            let l = ring(0.9);
            let r = l.ring(i);
            let (o, d) = through(eye, r.point(r.rest, 1.));
            let mut drag = Drag::begin(l, Handle::Ring(i), o, d).unwrap();
            // 22° along the tick band snaps to 20°.
            let (o, d) = through(eye, r.point(deg(22.), 1.));
            match drag.update(o, d, false) {
                Some(Delta::Rotate { axis, angle }) => {
                    assert!(axis.abs_diff_eq(AXES[i], 1e-6));
                    assert!((angle - deg(20.)).abs() < 1e-4, "{i}: {angle}");
                }
                other => panic!("{other:?}"),
            }
            assert_eq!(drag.zone(), SnapZone::Fine);
            // Inwards, in the spokes' zone: 45°.
            let (o, d) = through(eye, r.point(deg(37.), 0.5));
            match drag.update(o, d, false) {
                Some(Delta::Rotate { angle, .. }) => {
                    assert!((angle - deg(25.)).abs() < 1e-4, "{angle}")
                }
                other => panic!("{other:?}"),
            }
            assert_eq!(drag.zone(), SnapZone::Coarse);
            assert_eq!(drag.feedback(), format!("{} +45°", ["X", "Y"][i]));
            // Outside the ticks: free.
            let (o, d) = through(eye, r.point(deg(52.3), 1.5));
            let _ = drag.update(o, d, false);
            assert_eq!(drag.zone(), SnapZone::Free);
            assert!((drag.turned() - deg(52.3)).abs() < 1e-3);
            // Shift: 1° anywhere.
            let (o, d) = through(eye, r.point(deg(-30.4), 0.8));
            let _ = drag.update(o, d, true);
            assert_eq!(drag.zone(), SnapZone::Shift);
            assert!((drag.turned() + deg(30.)).abs() < 1e-4);
            // The ring geometry stays frozen.
            assert_eq!(drag.layout().tilt_radius, l.tilt_radius);
        }
    }
    #[test]
    fn grabbing_the_ring_away_from_the_handle_does_not_jump() {
        // Mica measures the sweep from where the ring was pressed (any point on it can be
        // grabbed; upstream only the handle), so pressing does not turn anything.
        let l = ring(0.);
        let r = l.ring(1);
        let eye = EYES[1];
        let (o, d) = through(eye, r.point(deg(130.), 1.));
        let mut drag = Drag::begin(l, Handle::Ring(1), o, d).unwrap();
        assert_eq!(drag.update(o, d, false), None);
        // 30° further turns the handle from 0° to 30°, a multiple of 5°.
        let (o, d) = through(eye, r.point(deg(160.), 1.));
        match drag.update(o, d, false) {
            Some(Delta::Rotate { axis, angle }) => {
                assert_eq!(axis, Vec3::Y);
                assert!((angle - deg(30.)).abs() < 1e-4, "{angle}");
            }
            other => panic!("{other:?}"),
        }
    }
    #[test]
    fn local_frame_rotates_about_the_turned_axis() {
        let mut l = ring(0.);
        l.frame = Quat::from_rotation_z(deg(30.)) * Quat::from_rotation_x(deg(20.));
        let r = l.ring(0);
        let eye = EYES[0];
        let (o, d) = through(eye, r.point(0., 1.));
        let mut drag = Drag::begin(l, Handle::Ring(0), o, d).unwrap();
        let (o, d) = through(eye, r.point(deg(90.), 0.5));
        match drag.update(o, d, false) {
            Some(Delta::Rotate { axis, angle }) => {
                assert!(axis.abs_diff_eq(l.frame * Vec3::X, 1e-5), "{axis}");
                assert!((angle - deg(90.)).abs() < 1e-4);
            }
            other => panic!("{other:?}"),
        }
    }
    #[test]
    fn ring_radius_does_not_depend_on_the_angle() {
        // The same footprint radius at any angle gives the same ring; a drag freezes it too.
        let radii: Vec<f32> = [0., 0.4, 1.2, 2.9, -1.0]
            .iter()
            .map(|a| ring(*a).ring_radius)
            .collect();
        assert!(
            radii.iter().all(|r| (r - radii[0]).abs() < 1e-6),
            "{radii:?}"
        );
        let l = ring(0.3);
        let (o, d) = down(l.ring(2).point(0.3, 1.));
        let drag = Drag::begin(l, Handle::Ring(2), o, d).unwrap();
        assert_eq!(drag.layout().ring_radius, l.ring_radius);
        assert_eq!(drag.layout().tilt_radius, l.tilt_radius);
    }
    #[test]
    fn snap_zones_and_angles() {
        assert_eq!(SnapZone::at(1.0, false), SnapZone::Fine);
        assert_eq!(SnapZone::at(0.9, false), SnapZone::Fine);
        assert_eq!(SnapZone::at(1.1, false), SnapZone::Fine);
        assert_eq!(SnapZone::at(0.5, false), SnapZone::Coarse);
        assert_eq!(SnapZone::at(1. / 3., false), SnapZone::Coarse);
        assert_eq!(SnapZone::at(0.75, false), SnapZone::Free);
        assert_eq!(SnapZone::at(0.2, false), SnapZone::Free);
        assert_eq!(SnapZone::at(1.4, false), SnapZone::Free);
        assert_eq!(SnapZone::at(0.5, true), SnapZone::Shift);
        assert_eq!(SnapZone::Fine.step_deg(), Some(5.));
        assert_eq!(SnapZone::Coarse.step_deg(), Some(45.));
        assert_eq!(SnapZone::Shift.step_deg(), Some(1.));
        assert_eq!(SnapZone::Free.step_deg(), None);
        // Absolute snapping: from 7°, a 40° sweep in the coarse zone lands on the 45° spoke.
        let off = snap_offset(deg(7.), deg(40.), SnapZone::Coarse);
        assert!((off - deg(38.)).abs() < 1e-5, "{}", off.to_degrees());
        let off = snap_offset(deg(7.), deg(40.), SnapZone::Fine);
        assert!((off - deg(38.)).abs() < 1e-5);
        let off = snap_offset(deg(7.), deg(40.4), SnapZone::Shift);
        assert!((off - deg(40.)).abs() < 1e-4);
        assert_eq!(snap_offset(deg(7.), deg(40.4), SnapZone::Free), deg(40.4));
        assert_eq!(SnapZone::Coarse.label(), "snap 45°");
    }
    #[test]
    fn axis_drag_moves_along_axis_only_and_snaps() {
        let l = layout(Mode::Move);
        let (o, d) = down(l.origin + Vec3::X * 20.);
        let mut drag = Drag::begin(l, Handle::Axis(0), o, d).unwrap();
        // Pointer moves diagonally; only X changes.
        let (o, d) = down(l.origin + Vec3::new(32.3, 9., 0.));
        match drag.update(o, d, false) {
            Some(Delta::Translate(v)) => assert!(v.abs_diff_eq(Vec3::new(12.3, 0., 0.), 1e-4)),
            other => panic!("{other:?}"),
        }
        assert_eq!(drag.update(o, d, false), None);
        let (o, d) = down(l.origin + Vec3::new(35.6, 9., 0.));
        // With snapping the total becomes 16 → emitted step 16 − 12.3.
        match drag.update(o, d, true) {
            Some(Delta::Translate(v)) => assert!((v.x - 3.7).abs() < 1e-4 && v.y == 0.),
            other => panic!("{other:?}"),
        }
        assert_eq!(drag.feedback(), "ΔX +16.0 mm");
    }
    #[test]
    fn ring_drag_snaps_by_zone_then_1_with_shift_and_unwraps() {
        let r = ring(0.);
        let (o, d) = down(r.ring(2).point(0., 1.));
        let mut drag = Drag::begin(r, Handle::Ring(2), o, d).unwrap();
        // 22° of pointer motion on the ring snaps to the nearest 5°.
        let (o, d) = down(r.ring(2).point(deg(22.), 1.));
        match drag.update(o, d, false) {
            Some(Delta::Rotate { axis, angle }) => {
                assert_eq!(axis, Vec3::Z);
                assert!((angle - deg(20.)).abs() < 1e-4, "{angle}");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(drag.zone(), SnapZone::Fine);
        // Moving inwards to half the radius snaps to 45°: +25° more.
        let (o, d) = down(r.ring(2).point(deg(40.), 0.5));
        match drag.update(o, d, false) {
            Some(Delta::Rotate { angle, .. }) => {
                assert!((angle - deg(25.)).abs() < 1e-4, "{angle}")
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(drag.zone(), SnapZone::Coarse);
        assert_eq!(drag.feedback(), "Z +45°");
        // Outside the ticks the rotation is free.
        let (o, d) = down(r.ring(2).point(deg(47.3), 1.5));
        match drag.update(o, d, false) {
            Some(Delta::Rotate { angle, .. }) => {
                assert!((angle - deg(2.3)).abs() < 1e-4, "{angle}")
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(drag.zone(), SnapZone::Free);
        // 22° with Shift: 1° steps.
        let (o, d) = down(r.ring(2).point(deg(22.4), 1.));
        match drag.update(o, d, true) {
            Some(Delta::Rotate { angle, .. }) => {
                assert!((angle + deg(25.3)).abs() < 1e-4, "{angle}")
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(drag.zone(), SnapZone::Shift);
        assert_eq!(drag.feedback(), "Z +22°");
        assert!((drag.turned() - deg(22.)).abs() < 1e-4);
        // A full turn and more does not wrap back at ±180°: the pointer ends at 382°, which
        // snaps to 380° in 5° steps, and the emitted increments add up to exactly that.
        let mut total = drag.turned();
        for step in 1..=24 {
            let a = deg(22.) + step as f32 * deg(15.);
            let (o, d) = down(r.ring(2).point(a, 1.));
            if let Some(Delta::Rotate { angle, .. }) = drag.update(o, d, false) {
                total += angle;
            }
        }
        assert!((total - deg(380.)).abs() < 1e-3, "{}", total.to_degrees());
        assert!((drag.turned() - deg(380.)).abs() < 1e-3);
        assert_eq!(wrap_degrees(382.), 22.);
        assert_eq!(wrap_degrees(-190.), 170.);
        assert_eq!(wrap_degrees(180.), 180.);
    }
    #[test]
    fn bed_ring_snaps_on_absolute_angles_from_any_view() {
        // Starting at 7°, a drag to 41° in the coarse zone lands on 45°, not on 7° + 45°, seen
        // from above or from 3/4 and low side views.
        let r = ring(deg(7.));
        for eye in [None, Some(EYES[0]), Some(EYES[3])] {
            let ray = |p: Vec3| eye.map_or_else(|| down(p), |e| through(e, p));
            let (o, d) = ray(r.ring(2).point(deg(7.), 1.));
            let mut drag = Drag::begin(r, Handle::Ring(2), o, d).unwrap();
            let (o, d) = ray(r.ring(2).point(deg(41.), 0.5));
            match drag.update(o, d, false) {
                Some(Delta::Rotate { angle, .. }) => {
                    assert!((angle - deg(38.)).abs() < 1e-4, "{angle}")
                }
                other => panic!("{other:?}"),
            }
        }
    }
    #[test]
    fn drawn_handle_moves_with_the_sweep() {
        let l = ring(0.5);
        let state = RotateState {
            sweep: Some(0.3),
            zone: SnapZone::Fine,
        };
        let x = l.ring(0);
        assert_eq!(Painter::handle_angle(&x, Some(Handle::Ring(0)), state), 0.3);
        assert_eq!(Painter::handle_angle(&x, None, state), 0.);
        assert_eq!(
            Painter::handle_angle(&l.ring(2), Some(Handle::Ring(2)), state),
            0.8
        );
    }
    #[test]
    fn scale_drags_are_relative_factors() {
        let s = layout(Mode::Scale);
        let (o, d) = down(s.origin + Vec3::Y * 20.);
        let mut drag = Drag::begin(s, Handle::ScaleAxis(1), o, d).unwrap();
        let (o, d) = down(s.origin + Vec3::Y * 30.);
        assert_eq!(
            drag.update(o, d, false),
            Some(Delta::Scale(Vec3::new(1., 1.5, 1.)))
        );
        let (o, d) = down(s.origin + Vec3::Y * 40.);
        match drag.update(o, d, false) {
            Some(Delta::Scale(v)) => assert!((v.y - 4. / 3.).abs() < 1e-5),
            other => panic!("{other:?}"),
        }
        assert_eq!(drag.feedback(), "Scale Y ×2.00");
        let (o, d) = (s.origin + Vec3::new(0., 20., 100.), -Vec3::Z);
        let mut drag = Drag::begin(s, Handle::Uniform, o, d).unwrap();
        let (o, d) = (s.origin + Vec3::new(0., 24.4, 100.), -Vec3::Z);
        match drag.update(o, d, true) {
            Some(Delta::Scale(v)) => assert!((v.x - 1.2).abs() < 1e-5 && v.x == v.y && v.y == v.z),
            other => panic!("{other:?}"),
        }
    }
    #[test]
    fn geometry_helpers() {
        let (o, d) = (Vec3::new(0., 0., 10.), -Vec3::Z);
        assert_eq!(ray_plane(o, d, Vec3::ZERO, Vec3::Z), Some(Vec3::ZERO));
        assert_eq!(ray_plane(o, Vec3::X, Vec3::ZERO, Vec3::Z), None);
        assert_eq!(
            ray_line_parameter(o, d, Vec3::new(-5., 0., 0.), Vec3::X),
            Some(5.)
        );
        assert!(
            (ray_segment_distance(o, d, Vec3::new(3., 0., 0.), Vec3::new(8., 0., 0.)) - 3.).abs()
                < 1e-5
        );
        assert!((ray_point_distance(o, d, Vec3::new(0., 4., 0.)) - 4.).abs() < 1e-5);
    }
}
