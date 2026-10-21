// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: prusaslicer@b028299c77 src/libslic3r/Measure.hpp :: SurfaceFeature, MeasurementResult, are_parallel, are_perpendicular
// Source: prusaslicer@b028299c77 src/libslic3r/MeasureUtils.hpp :: get_orthogonal
// Changes: Rust enum instead of a tagged class; planes carry their own border edges (so measuring against a plane needs no Measuring object and features of different objects mix); glam f64 vectors; EPSILON is libslic3r's 1e-4.
// Upstream attribution (Measure.hpp):
// Copyright (c) Prusa Research 2022 - 2023 Lukáš Matěna @lukasmatena, Enrico Turri @enricoturri1966, Vojtěch Bubník @bubnikv
//
// PrusaSlicer is released under the terms of the AGPLv3 or higher
//

//! Measuring on meshes: surface features under the pointer (point, edge, circle, plane) and the
//! distance / angle between two of them, as PrusaSlicer's measure gizmo computes them.

pub mod features;
pub mod math;
pub use features::Measuring;
pub use math::{AngleAndEdges, MeasurementResult, measure};

use glam::DVec3;
use std::sync::Arc;

/// libslic3r's `EPSILON`.
pub const EPSILON: f64 = 1e-4;

/// A feature of a mesh surface, in world coordinates (mm).
#[derive(Clone, Debug)]
pub enum Feature {
    Point(DVec3),
    /// An edge; `center` is the centre of the regular polygon it belongs to, when it does.
    Edge {
        a: DVec3,
        b: DVec3,
        center: Option<DVec3>,
    },
    Circle {
        center: DVec3,
        radius: f64,
        normal: DVec3,
    },
    /// A flat region: its index in its object's `Measuring`, unit normal, a point on it (the
    /// centre of its border) and its border edges (for distances to it).
    Plane {
        index: usize,
        normal: DVec3,
        origin: DVec3,
        edges: Arc<Vec<(DVec3, DVec3)>>,
    },
}

fn approx(a: DVec3, b: DVec3) -> bool {
    // Eigen's isApprox: relative to the smaller norm, with an absolute floor.
    (a - b).length() <= 1e-9_f64.max(1e-12 * a.length().min(b.length()))
}

impl PartialEq for Feature {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Point(a), Self::Point(b)) => approx(*a, *b),
            (Self::Edge { a, b, .. }, Self::Edge { a: c, b: d, .. }) => {
                (approx(*a, *c) && approx(*b, *d)) || (approx(*a, *d) && approx(*b, *c))
            }
            (
                Self::Circle {
                    center: c1,
                    radius: r1,
                    normal: n1,
                },
                Self::Circle {
                    center: c2,
                    radius: r2,
                    normal: n2,
                },
            ) => approx(*c1, *c2) && approx(*n1, *n2) && (r1 - r2).abs() < EPSILON,
            (
                Self::Plane {
                    index: i1,
                    normal: n1,
                    origin: o1,
                    ..
                },
                Self::Plane {
                    index: i2,
                    normal: n2,
                    origin: o2,
                    ..
                },
            ) => i1 == i2 && approx(*n1, *n2) && approx(*o1, *o2),
            _ => false,
        }
    }
}

impl Feature {
    /// Upstream's type order (point < edge < circle < plane), used to order pairs.
    pub fn rank(&self) -> u8 {
        match self {
            Self::Point(_) => 0,
            Self::Edge { .. } => 1,
            Self::Circle { .. } => 2,
            Self::Plane { .. } => 3,
        }
    }
    pub fn name(&self) -> &'static str {
        match self {
            Self::Point(_) => "Point",
            Self::Edge { .. } => "Edge",
            Self::Circle { .. } => "Circle",
            Self::Plane { .. } => "Plane",
        }
    }
    /// The centre a circle or a regular polygon's edge offers as a point to measure from.
    pub fn center(&self) -> Option<DVec3> {
        match self {
            Self::Circle { center, .. } => Some(*center),
            Self::Edge { center, .. } => *center,
            _ => None,
        }
    }
}

pub fn edge_direction(a: DVec3, b: DVec3) -> DVec3 {
    (b - a).normalize_or_zero()
}
pub fn are_parallel(v1: DVec3, v2: DVec3) -> bool {
    (v1.dot(v2).abs() - 1.).abs() < EPSILON
}
pub fn are_perpendicular(v1: DVec3, v2: DVec3) -> bool {
    v1.dot(v2).abs() < EPSILON
}

/// A unit vector orthogonal to `v` (`MeasureUtils.hpp :: get_orthogonal`).
pub fn get_orthogonal(v: DVec3) -> DVec3 {
    let a = [v.x, v.y, v.z];
    let mut imax = 0;
    for i in 1..3 {
        if a[i].abs() > a[imax].abs() {
            imax = i;
        }
    }
    let inext = (imax + 1) % 3;
    let mut r = [0.; 3];
    r[imax] = a[inext];
    r[inext] = -a[imax];
    DVec3::from(r).normalize_or_zero()
}

/// An orthonormal basis whose third vector is `v` (`Measure.cpp :: orthonormal_basis`).
pub fn orthonormal_basis(v: DVec3) -> [DVec3; 3] {
    let z = v.normalize_or_zero();
    let abs = z.abs();
    let x = if abs.x >= abs.y && abs.x >= abs.z {
        DVec3::new(z.y, -z.x, 0.)
    } else if abs.y >= abs.z {
        DVec3::new(0., z.z, -z.y)
    } else {
        DVec3::new(-z.z, 0., z.x)
    }
    .normalize_or_zero();
    let y = z.cross(x).normalize_or_zero();
    [x, y, z]
}
