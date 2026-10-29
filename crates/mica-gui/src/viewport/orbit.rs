// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: orca@8500fcdcca src/slic3r/GUI/GLCanvas3D.cpp :: GLCanvas3D::on_mouse, GLCanvas3D::m_rotation_center
// Source: prusaslicer@b028299c77 src/slic3r/GUI/GLCanvas3D.cpp :: GLCanvas3D::on_mouse
// Changes: The orbit pivot as a small state machine the viewport's egui drag handling asks each frame. Cmd/Ctrl-drag: the scene point under the screen center (object surface by ray picking instead of the depth buffer, else the bed plane, else the camera target) dropped to z = 0, taken the first frame Cmd is held during a rotation and kept until the button goes up (Orca). The "orbit around the screen center" preference turns around the camera target like PrusaSlicer's rotate_on_sphere, ignoring the selection; off keeps Mica's Orca-style selection / plate-center pivot.

//! What a left-drag orbit turns around.
//!
//! - Cmd (Ctrl off the Mac) held: the point under the middle of the view, on the bed plane,
//!   fixed for the rest of that drag (OrcaSlicer).
//! - Preference "Orbit around the screen center": the camera target, which panning and
//!   zooming at the pointer move, so the view always turns around what is in the middle of the
//!   screen (PrusaSlicer); the selection does not matter.
//! - Otherwise the selected object (or prime tower) in Prepare, else the current plate's
//!   center on the bed (OrcaSlicer).

use crate::scene::{Object, camera::Camera, picking};
use glam::{Vec2, Vec3};

/// The pivot of one orbit drag; [`Orbit::end`] on button release.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Orbit {
    /// The Cmd-drag pivot once taken (Orca's `m_rotation_center`).
    centre: Option<Vec3>,
}

impl Orbit {
    /// This frame's pivot. `command`: Cmd/Ctrl is held now; `screen_centre`: the preference;
    /// `under_centre` finds the Cmd pivot (called once per drag); `usual` is the selection or
    /// plate pivot.
    pub fn pivot(
        &mut self,
        command: bool,
        screen_centre: bool,
        camera: &Camera,
        under_centre: impl FnOnce() -> Vec3,
        usual: impl FnOnce() -> Vec3,
    ) -> Vec3 {
        if command {
            *self.centre.get_or_insert_with(under_centre)
        } else if screen_centre {
            camera.target
        } else {
            usual()
        }
    }

    /// The button went up: the next Cmd-drag takes a new pivot.
    pub fn end(&mut self) {
        self.centre = None;
    }

    #[cfg(test)]
    pub fn centre(&self) -> Option<Vec3> {
        self.centre
    }
}

/// The point under the middle of the view (`aspect` = width / height) dropped onto the bed
/// plane: an object's surface there, else where the center ray meets z = 0, else the camera
/// target, each with z = 0.
pub fn under_screen_centre(camera: &Camera, objects: &[Object], aspect: f32) -> Vec3 {
    let (origin, dir) = camera.ray(Vec2::splat(0.5), aspect);
    let distance = picking::pick_face(objects, origin, dir)
        .map(|h| h.distance)
        .or_else(|| {
            (dir.z.abs() > 1e-5)
                .then(|| -origin.z / dir.z)
                .filter(|t| *t > 0.)
        });
    distance
        .map_or(camera.target, |t| origin + dir * t)
        .with_z(0.)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn camera() -> Camera {
        Camera::looking_at(Vec3::new(125., 110., 0.), 300.)
    }

    #[test]
    fn pivot_per_mode() {
        let mut c = camera();
        c.target = Vec3::new(10., 20., 30.);
        let usual = Vec3::new(1., 2., 3.);
        let centre = Vec3::new(7., 8., 0.);
        let mut o = Orbit::default();
        // Off: the selection / plate pivot (Orca).
        assert_eq!(o.pivot(false, false, &c, || centre, || usual), usual);
        // Preference on: the camera target, whatever is selected (PrusaSlicer).
        assert_eq!(o.pivot(false, true, &c, || centre, || usual), c.target);
        // Cmd wins in both.
        assert_eq!(o.pivot(true, false, &c, || centre, || usual), centre);
        o.end();
        assert_eq!(o.pivot(true, true, &c, || centre, || usual), centre);
    }

    #[test]
    fn command_pivot_is_taken_once_per_drag_and_reset_on_release() {
        let c = camera();
        let mut o = Orbit::default();
        let first = Vec3::new(50., 60., 0.);
        assert_eq!(o.pivot(true, false, &c, || first, || Vec3::ZERO), first);
        // Later frames of the same drag keep it, even after Cmd was let go and pressed again.
        let again = o.pivot(
            true,
            false,
            &c,
            || panic!("taken once per drag"),
            || Vec3::ZERO,
        );
        assert_eq!(again, first);
        assert_eq!(o.pivot(false, false, &c, || first, || Vec3::ONE), Vec3::ONE);
        assert_eq!(o.pivot(true, false, &c, || Vec3::X, || Vec3::ZERO), first);
        o.end();
        assert_eq!(o.centre(), None);
        let next = Vec3::new(-5., 5., 0.);
        assert_eq!(o.pivot(true, false, &c, || next, || Vec3::ZERO), next);
    }

    #[test]
    fn screen_centre_lands_on_the_bed() {
        for perspective in [false, true] {
            let mut c = camera();
            c.perspective = perspective;
            c.yaw = 0.4;
            c.pitch = 0.6;
            // A target above the bed (fit frames at 35 % of the height): the pivot is where
            // the center ray meets the bed, on screen at the center.
            c.target = Vec3::new(100., 90., 25.);
            let p = under_screen_centre(&c, &[], 1.5);
            assert_eq!(p.z, 0.);
            let uv = c.project(p, 1.5).unwrap();
            assert!(
                uv.abs_diff_eq(Vec2::splat(0.5), 1e-3),
                "{perspective}: {uv}"
            );
            // Orbiting around it keeps it there.
            c.orbit_around(Vec2::new(40., -15.), p);
            let uv = c.project(p, 1.5).unwrap();
            assert!(
                uv.abs_diff_eq(Vec2::splat(0.5), 1e-3),
                "{perspective}: {uv}"
            );
        }
        // Above the bed but looking upwards: the center ray never meets the bed in front of
        // the camera, so the target, dropped onto the bed, is the pivot.
        let mut c = camera();
        c.pitch = -0.5;
        c.target = Vec3::new(10., 10., 400.);
        assert!(c.eye().z > 0.);
        assert_eq!(under_screen_centre(&c, &[], 1.), Vec3::new(10., 10., 0.));
    }
}
