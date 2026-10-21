// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Lossless source geometry beside the engine's f32 mesh. Public mesh edits invalidate the
//! source automatically: saving never substitutes stale geometry for an edited mesh.

use super::Transform;
use crate::mesh::TriangleMesh;

#[derive(Clone, Debug)]
pub struct SourceMesh {
    pub vertices: Vec<[f64; 3]>,
    pub faces: Vec<[u32; 3]>,
    pub transform: Transform,
}

impl SourceMesh {
    /// Check against the same two-stage conversion used by the slicing importer. This keeps
    /// existing slicing coordinates stable while the native archive retains all source bits.
    pub fn matches(&self, mesh: &TriangleMesh) -> bool {
        let determinant = self.transform.determinant();
        determinant.is_finite()
            && determinant != 0.
            && self.transform.0.iter().flatten().all(|v| v.is_finite())
            && self.vertices.iter().flatten().all(|v| v.is_finite())
            && self.vertices.len() == mesh.vertices.len()
            && self.faces.len() == mesh.faces.len()
            && self.vertices.iter().zip(&mesh.vertices).all(|(a, b)| {
                self.transform.apply(a.map(|v| v as f32)).map(f32::to_bits) == b.map(f32::to_bits)
            })
            && self.faces.iter().zip(&mesh.faces).all(|(a, b)| {
                let mut a = *a;
                if determinant < 0. {
                    a.swap(1, 2);
                }
                a == *b
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn affine_inverse_handles_shear_nonuniform_scale_and_reflection() {
        let t = Transform([
            [-2.5, 0.3, -0.2, 123.456789012345],
            [0.1, 6.5, 0.4, -20.1234567890123],
            [0.2, -0.1, 1.5, 0.123456789012345],
        ]);
        let inverse = t.inverse().unwrap();
        for product in [t.compose(inverse), inverse.compose(t)] {
            for (actual, expected) in product
                .0
                .iter()
                .flatten()
                .zip(Transform::IDENTITY.0.iter().flatten())
            {
                assert!((actual - expected).abs() < 1e-12);
            }
        }
        assert!(Transform([[0.; 4]; 3]).inverse().is_none());
    }
}
