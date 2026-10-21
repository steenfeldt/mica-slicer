// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/Emboss.cpp :: Emboss::create_transformation_onto_surface
// Source: prusaslicer@b028299c77 src/libslic3r/Emboss.cpp :: Emboss::suggest_up
// Source: prusaslicer@b028299c77 src/slic3r/GUI/SurfaceDrag.hpp :: UP_LIMIT
// Changes: Rust port of the placement frame (text X right, Y up, Z out of the surface) built from orthonormal axes instead of two Eigen angle-axis rotations; f64 row-major `threemf::Transform`.
// Upstream attribution (Emboss.cpp):
// Copyright (c) Prusa Research 2021 - 2023 Filip Sykala @Jony01, Lukáš Matěna @lukasmatena, Vojtěch Bubník @bubnikv
//
// PrusaSlicer is released under the terms of the AGPLv3 or higher

//! Embossed text and SVG shapes: the 2D outline (in the text frame, millimetres) becomes a
//! closed prism along the frame's Z, either flat or following the surface it sits on, and its
//! editable source (`config`) travels with the volume and in 3MF projects the PrusaSlicer way.
//!
//! The text frame: X runs along the text, Y up the letters, Z out of the surface the text was
//! placed on. A raised part spans `0..depth` above the surface, engraved volumes and modifiers
//! `-depth..0` below it (each with PrusaSlicer's 0.015 mm overlap into the other side).

pub mod config;
pub mod mesh;
pub mod surface;

pub use config::{EmbossConfig, EmbossSource, HAlign, SvgSource, TextSource, VAlign};
pub use mesh::{EmbossRange, emboss_mesh, heal};
pub use surface::SurfaceProbe;

use crate::threemf::Transform;

/// Above this |normal.z| the text's "up" follows world Y instead of world Z (PrusaSlicer).
pub const UP_LIMIT: f64 = 0.9;

fn normalize(v: [f64; 3]) -> Option<[f64; 3]> {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    (l > 1e-12 && l.is_finite()).then(|| v.map(|c| c / l))
}
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// The direction on a surface with `normal` that text should read "up" along: the projection of
/// world Z onto the surface, or of world Y on nearly horizontal surfaces.
pub fn suggest_up(normal: [f64; 3], up_limit: f64) -> [f64; 3] {
    let wanted = if normal[2].abs() > up_limit {
        [0., 1., 0.]
    } else {
        [0., 0., 1.]
    };
    normalize(cross(cross(normal, wanted), normal)).unwrap_or([0., 1., 0.])
}

/// The text frame at `position` on a surface with outward `normal`: Z along the normal, Y the
/// suggested up direction, X = Y × Z. Falls back to a frame looking up when `normal` is zero.
pub fn transform_onto_surface(position: [f64; 3], normal: [f64; 3], up_limit: f64) -> Transform {
    let z = normalize(normal).unwrap_or([0., 0., 1.]);
    let y = suggest_up(z, up_limit);
    let x = normalize(cross(y, z)).unwrap_or([1., 0., 0.]);
    let y = cross(z, x);
    Transform([
        [x[0], y[0], z[0], position[0]],
        [x[1], y[1], z[1], position[1]],
        [x[2], y[2], z[2], position[2]],
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_on_a_side_wall_reads_upright() {
        // Wall facing -Y: text Z = -Y, up = +Z, so X (reading direction) = Y × Z = +X... seen
        // from outside (looking along +Y) the text reads left to right.
        let t = transform_onto_surface([1., 2., 3.], [0., -1., 0.], UP_LIMIT);
        let col = |c: usize| [t.0[0][c], t.0[1][c], t.0[2][c]];
        let close = |a: [f64; 3], b: [f64; 3]| a.iter().zip(b).all(|(p, q)| (p - q).abs() < 1e-9);
        assert!(close(col(2), [0., -1., 0.]));
        assert!(close(col(1), [0., 0., 1.]));
        assert!(close(col(0), [1., 0., 0.]));
        assert!(close(col(3), [1., 2., 3.]));
        assert!((t.determinant() - 1.).abs() < 1e-9);
    }

    #[test]
    fn frame_on_top_face_uses_world_y_as_up() {
        let t = transform_onto_surface([0., 0., 10.], [0., 0., 1.], UP_LIMIT);
        assert!((t.0[1][1] - 1.).abs() < 1e-9, "{t:?}");
        assert!((t.0[0][0] - 1.).abs() < 1e-9, "{t:?}");
    }
}
