// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/SeamPainting.cpp :: Painting, is_enforced, is_blocked
// Changes: Public bed-coordinate query API; optional borrowed triangle sets, independent BVH; 3MF facet decoding stays with the importer.

use super::visibility::MeshTree;
use crate::mesh::TriangleMesh;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Paint {
    Enforcer,
    #[default]
    Common,
    Blocker,
}

/// Painting in the same millimetre coordinate system as the placed mesh. Blockers win overlaps.
/// `intersects` tests a sphere, allowing the planner to subdivide only painted edges.
pub trait SeamPainting: Sync {
    fn classify(&self, position: [f64; 3], radius: f64) -> Paint;
    fn intersects(&self, position: [f64; 3], radius: f64) -> bool {
        self.classify(position, radius) != Paint::Common
    }
}
#[derive(Default)]
pub struct NoPainting;
impl SeamPainting for NoPainting {
    fn classify(&self, _: [f64; 3], _: f64) -> Paint {
        Paint::Common
    }
}

pub struct MeshPainting<'a> {
    enforcers: MeshTree<'a>,
    blockers: MeshTree<'a>,
}
impl<'a> MeshPainting<'a> {
    pub fn new(enforcers: &'a TriangleMesh, blockers: &'a TriangleMesh) -> Self {
        Self {
            enforcers: MeshTree::new(enforcers),
            blockers: MeshTree::new(blockers),
        }
    }
}
impl SeamPainting for MeshPainting<'_> {
    fn classify(&self, p: [f64; 3], radius: f64) -> Paint {
        if self.blockers.in_radius(p, radius) {
            Paint::Blocker
        } else if self.enforcers.in_radius(p, radius) {
            Paint::Enforcer
        } else {
            Paint::Common
        }
    }
}
