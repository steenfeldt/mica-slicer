// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

use glam::{Mat4, Vec2, Vec3};

/// Orbit camera. `span` is the visible height in millimetres at the target; perspective keeps
/// the same framing by deriving its distance from `span` and `fov`.
#[derive(Clone, Debug, PartialEq)]
pub struct Camera {
    pub target: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub span: f32,
    pub perspective: bool,
    pub fov: f32,
    /// Zoom animation goal; `animate` moves `span` towards it.
    pub span_goal: f32,
    anchor: Option<(Vec2, Vec3)>,
}
impl Default for Camera {
    fn default() -> Self {
        Self {
            target: Vec3::new(125., 110., 0.),
            yaw: -1.05,
            pitch: 0.75,
            span: 330.,
            perspective: true,
            fov: 45f32.to_radians(),
            span_goal: 330.,
            anchor: None,
        }
    }
}
impl Camera {
    pub fn looking_at(target: Vec3, span: f32) -> Self {
        Self {
            target,
            span,
            span_goal: span,
            ..Default::default()
        }
    }
    pub fn direction(&self) -> Vec3 {
        Vec3::new(
            self.yaw.cos() * self.pitch.cos(),
            self.yaw.sin() * self.pitch.cos(),
            self.pitch.sin(),
        )
    }
    fn up(&self) -> Vec3 {
        let d = self.direction();
        if d.z.abs() > 0.9999 {
            // Looking straight down/up: keep +Y up on screen.
            if d.z > 0. { Vec3::Y } else { -Vec3::Y }
        } else {
            Vec3::Z
        }
    }
    /// Screen-space basis: (right, up) in world units.
    pub fn basis(&self) -> (Vec3, Vec3) {
        let d = self.direction();
        let right = self.up().cross(d).normalize_or(Vec3::X);
        let up = d.cross(right).normalize_or(Vec3::Z);
        (right, up)
    }
    pub fn distance(&self) -> f32 {
        if self.perspective {
            self.span / (2. * (self.fov / 2.).tan())
        } else {
            10000.
        }
    }
    pub fn eye(&self) -> Vec3 {
        self.target + self.direction() * self.distance()
    }
    pub fn matrix(&self, aspect: f32) -> Mat4 {
        self.matrix_reaching(aspect, 0.)
    }
    /// `matrix` whose far plane (perspective) lies at least `reach` mm from the eye, so the
    /// scene is never cut off when the target is close to the eye (zoomed in towards the
    /// pointer, then orbited or panned).
    pub fn matrix_reaching(&self, aspect: f32, reach: f32) -> Mat4 {
        let view = glam::camera::rh::view::look_at_mat4(self.eye(), self.target, self.up());
        let projection = if self.perspective {
            let d = self.distance();
            let far = (d * 40.).max(reach * 1.05);
            glam::camera::rh::proj::directx::perspective(self.fov, aspect, d * 0.02, far)
        } else {
            glam::camera::rh::proj::directx::orthographic(
                -self.span * aspect / 2.,
                self.span * aspect / 2.,
                -self.span / 2.,
                self.span / 2.,
                0.1,
                20000.,
            )
        };
        projection * view
    }
    /// World ray through viewport coordinates `uv` in 0..1 (origin top-left).
    pub fn ray(&self, uv: Vec2, aspect: f32) -> (Vec3, Vec3) {
        let inv = self.matrix(aspect).inverse();
        let p = Vec3::new(2. * uv.x - 1., 1. - 2. * uv.y, 0.);
        let a = inv.project_point3(p);
        let b = inv.project_point3(p.with_z(1.));
        (a, (b - a).normalize())
    }
    /// Projects a world point to viewport coordinates 0..1; `None` when behind the camera.
    pub fn project(&self, world: Vec3, aspect: f32) -> Option<Vec2> {
        let c = self.matrix(aspect) * world.extend(1.);
        if c.w <= 1e-6 {
            return None;
        }
        let n = c.truncate() / c.w;
        Some(Vec2::new(n.x * 0.5 + 0.5, 0.5 - n.y * 0.5))
    }
    /// Millimetres per viewport pixel at the target depth.
    pub fn mm_per_pixel(&self, height_px: f32) -> f32 {
        self.span / height_px.max(1.)
    }
    pub fn bed_point(&self, uv: Vec2, aspect: f32) -> Option<Vec3> {
        let (a, d) = self.ray(uv, aspect);
        (d.z.abs() > 1e-5).then(|| a - d * (a.z / d.z))
    }
    /// Intersection with the plane through the target facing the camera.
    pub fn plane_point(&self, uv: Vec2, aspect: f32) -> Vec3 {
        let (a, d) = self.ray(uv, aspect);
        let n = self.direction();
        let denom = d.dot(n);
        if denom.abs() < 1e-6 {
            return self.target;
        }
        a + d * ((self.target - a).dot(n) / denom)
    }
    pub fn orbit(&mut self, delta: Vec2) {
        self.yaw -= delta.x * 0.008;
        self.pitch = (self.pitch + delta.y * 0.008).clamp(-1.55, 1.55);
    }
    /// Orbits around `pivot` (selection or bed centre) so it stays fixed on screen.
    pub fn orbit_around(&mut self, delta: Vec2, pivot: Vec3) {
        let (r0, u0) = self.basis();
        let d0 = self.direction();
        let offset = self.target - pivot;
        let local = Vec3::new(offset.dot(r0), offset.dot(u0), offset.dot(d0));
        self.orbit(delta);
        let (r1, u1) = self.basis();
        let d1 = self.direction();
        self.target = pivot + r1 * local.x + u1 * local.y + d1 * local.z;
    }
    pub fn pan(&mut self, delta: Vec2, height: f32) {
        let (right, up) = self.basis();
        let mm = self.mm_per_pixel(height);
        self.target += (-right * delta.x + up * delta.y) * mm;
    }
    /// Immediate zoom (no animation) centred on the target.
    pub fn zoom(&mut self, delta: f32) {
        self.span = (self.span * (-delta * 0.002).exp()).clamp(5., 4000.);
        self.span_goal = self.span;
        self.anchor = None;
    }
    /// Smooth zoom towards the point under the cursor. Call `animate` each frame afterwards.
    pub fn zoom_at(&mut self, delta: f32, uv: Vec2, aspect: f32) {
        self.span_goal = (self.span_goal * (-delta * 0.002).exp()).clamp(5., 4000.);
        if self.anchor.is_none() {
            self.anchor = Some((uv, self.plane_point(uv, aspect)));
        }
    }
    /// Advances zoom animation; returns true while more frames are needed.
    pub fn animate(&mut self, dt: f32, aspect: f32) -> bool {
        if (self.span - self.span_goal).abs() < 1e-3 * self.span {
            self.span = self.span_goal;
            self.anchor = None;
            return false;
        }
        let k = 1. - (-dt * 18.).exp();
        self.span += (self.span_goal - self.span) * k.clamp(0.05, 1.);
        if let Some((uv, world)) = self.anchor {
            let now = self.plane_point(uv, aspect);
            self.target += world - now;
        }
        true
    }
    /// Frames a world bounding box.
    pub fn fit(&mut self, min: Vec3, max: Vec3, aspect: f32) {
        let (right, up) = self.basis();
        let center = (min + max) * 0.5;
        let mut ext_r = 0f32;
        let mut ext_u = 0f32;
        for x in [min.x, max.x] {
            for y in [min.y, max.y] {
                for z in [min.z, max.z] {
                    let p = Vec3::new(x, y, z) - center;
                    ext_r = ext_r.max(p.dot(right).abs());
                    ext_u = ext_u.max(p.dot(up).abs());
                }
            }
        }
        let span = (2. * ext_u).max(2. * ext_r / aspect.max(0.1)) * 1.15;
        self.target = center;
        self.span = span.clamp(5., 4000.);
        self.span_goal = self.span;
        self.anchor = None;
    }
    pub fn preset(&mut self, n: usize) {
        (self.yaw, self.pitch) = match n {
            1 => (-std::f32::consts::FRAC_PI_2, std::f32::consts::FRAC_PI_2),
            2 => (-std::f32::consts::FRAC_PI_2, -std::f32::consts::FRAC_PI_2),
            3 => (-std::f32::consts::FRAC_PI_2, 0.),
            4 => (std::f32::consts::FRAC_PI_2, 0.),
            5 => (std::f32::consts::PI, 0.),
            6 => (0., 0.),
            _ => (-1.05, 0.75),
        };
    }
    pub fn preset_name(n: usize) -> &'static str {
        ["Iso", "Top", "Bottom", "Front", "Rear", "Left", "Right"][n.min(6)]
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn projection_ray_roundtrip_all_views_both_modes() {
        for perspective in [false, true] {
            let mut c = Camera {
                perspective,
                ..Default::default()
            };
            for n in 0..7 {
                c.preset(n);
                let p = c.matrix(1.4).project_point3(c.target);
                assert!(p.x.abs() < 1e-4 && p.y.abs() < 1e-4, "{perspective} {n}");
                let (a, d) = c.ray(Vec2::splat(0.5), 1.4);
                assert!((c.target - a).cross(d).length() < 0.05, "{perspective} {n}");
                assert!(
                    c.project(c.target, 1.4)
                        .unwrap()
                        .abs_diff_eq(Vec2::splat(0.5), 1e-4)
                );
            }
        }
    }
    #[test]
    fn bed_and_zoom() {
        let mut c = Camera {
            perspective: false,
            ..Default::default()
        };
        assert!(
            c.bed_point(Vec2::splat(0.5), 1.)
                .unwrap()
                .distance(c.target)
                < 0.01
        );
        c.zoom(1e6);
        assert_eq!(c.span, 5.);
        c.zoom(-1e6);
        assert_eq!(c.span, 4000.);
    }
    #[test]
    fn zoom_to_cursor_keeps_point_fixed() {
        for perspective in [false, true] {
            let mut c = Camera {
                perspective,
                ..Default::default()
            };
            let uv = Vec2::new(0.8, 0.3);
            let before = c.plane_point(uv, 1.5);
            c.zoom_at(300., uv, 1.5);
            for _ in 0..200 {
                if !c.animate(1. / 60., 1.5) {
                    break;
                }
            }
            assert!(c.span < 330.);
            let after = c.plane_point(uv, 1.5);
            assert!(
                before.distance(after) < 0.5,
                "{perspective}: {before} {after}"
            );
        }
    }
    #[test]
    fn orbit_around_pivot_keeps_it_on_screen() {
        let mut c = Camera {
            perspective: false,
            ..Default::default()
        };
        let pivot = Vec3::new(40., 20., 10.);
        let before = c.project(pivot, 1.3).unwrap();
        c.orbit_around(Vec2::new(60., -25.), pivot);
        let after = c.project(pivot, 1.3).unwrap();
        assert!(before.abs_diff_eq(after, 1e-3));
    }
    #[test]
    fn far_plane_reaches_the_scene_when_zoomed_in() {
        let c = Camera {
            span: 5.,
            ..Default::default()
        };
        let far_point = c.eye() - c.direction() * 900.;
        let clip = c.matrix(1.4).project_point3(far_point);
        assert!(clip.z > 1., "cut off without a reach");
        let clip = c.matrix_reaching(1.4, 900.).project_point3(far_point);
        assert!((0. ..1.).contains(&clip.z));
    }
    #[test]
    fn fit_frames_bounds() {
        let mut c = Camera::default();
        c.preset(1);
        c.fit(Vec3::ZERO, Vec3::new(250., 220., 0.), 1.5);
        assert_eq!(c.target, Vec3::new(125., 110., 0.));
        assert!((c.span - 220. * 1.15).abs() < 1e-3);
        c.fit(Vec3::ZERO, Vec3::new(600., 100., 0.), 1.5);
        assert!((c.span - 600. / 1.5 * 1.15).abs() < 1e-3);
    }
}
