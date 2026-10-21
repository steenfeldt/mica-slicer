// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Lay on face in the 3D view: the planes `mica_core::flatten` finds, cached per mesh and
//! scale (rotating an object never changes which planes it has, only where they point), and
//! picked by casting the pointer ray against the plane outlines.
//!
//! Planes are computed on the rayon pool the first time an object is shown with the tool on
//! (the 3DBenchy takes about 40 ms in a release build) and the view repaints when they are
//! ready; `blocking` computes them in place for headless screenshots and tests.

use crate::scene::Object;
use eframe::egui;
use glam::Vec3;
use mica_core::flatten::{self, Plane};
use mica_core::mesh::TriangleMesh;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

/// The identity of a mesh (its shared allocation) and the scale it is shown at.
type Key = (usize, [u32; 3]);

struct Entry {
    /// Keeps the allocation alive so its address cannot be reused by another mesh.
    _mesh: Arc<TriangleMesh>,
    planes: Arc<OnceLock<Arc<Vec<Plane>>>>,
}

#[derive(Default)]
pub struct PlaneCache {
    entries: HashMap<Key, Entry>,
    /// Compute on the calling thread instead of the pool (headless).
    pub blocking: bool,
}

/// The plane under the pointer.
#[derive(Clone, Debug, PartialEq)]
pub struct PlaneHit {
    pub object: usize,
    pub plane: usize,
    pub distance: f32,
    /// The plane's world normal now: laying on it turns this to −Z.
    pub normal: Vec3,
}

fn key(o: &Object) -> Key {
    (
        Arc::as_ptr(&o.mesh) as usize,
        (o.scale * o.mirror).to_array().map(f32::to_bits),
    )
}

impl PlaneCache {
    /// Drops the planes of meshes (or scales) no object in the scene has any more.
    pub fn retain(&mut self, objects: &[Object]) {
        if self.entries.is_empty() {
            return;
        }
        let live: std::collections::HashSet<Key> = objects.iter().map(key).collect();
        self.entries.retain(|k, _| live.contains(k));
    }

    /// The object's planes, or `None` while they are being computed (the view repaints when
    /// they are ready).
    pub fn planes(&mut self, o: &Object, ctx: &egui::Context) -> Option<Arc<Vec<Plane>>> {
        let blocking = self.blocking;
        let entry = self.entries.entry(key(o)).or_insert_with(|| {
            let cell: Arc<OnceLock<Arc<Vec<Plane>>>> = Arc::new(OnceLock::new());
            let mesh = o.mesh.clone();
            let scale = (o.scale * o.mirror).to_array().map(f64::from);
            if blocking {
                let _ = cell.set(Arc::new(flatten::planes(&mesh.vertices, scale)));
            } else {
                let (cell, mesh, ctx) = (cell.clone(), mesh.clone(), ctx.clone());
                rayon::spawn(move || {
                    let _ = cell.set(Arc::new(flatten::planes(&mesh.vertices, scale)));
                    ctx.request_repaint();
                });
            }
            Entry {
                _mesh: mesh,
                planes: cell,
            }
        });
        entry.planes.get().cloned()
    }

    /// The nearest plane outline the ray meets among `candidates`, as long as no other
    /// object's surface is in front of it (`face` is the nearest mesh hit, if any).
    pub fn pick(
        &mut self,
        objects: &[Object],
        candidates: &[usize],
        origin: Vec3,
        dir: Vec3,
        face: Option<(usize, f32)>,
        ctx: &egui::Context,
    ) -> Option<PlaneHit> {
        let mut best: Option<PlaneHit> = None;
        for &i in candidates {
            let Some(o) = objects.get(i) else { continue };
            let Some(planes) = self.planes(o, ctx) else {
                continue;
            };
            let (r, offset) = frame_of(o);
            let inv = r.inverse();
            let local_origin = (inv * (origin - offset)).as_dvec3().to_array();
            let local_dir = (inv * dir).as_dvec3().to_array();
            for (pi, p) in planes.iter().enumerate() {
                let Some(t) = flatten::ray_hits(&p.outline, p.normal, local_origin, local_dir)
                else {
                    continue;
                };
                let t = t as f32;
                let hidden = face.is_some_and(|(fo, d)| fo != i && d + 1e-3 < t);
                if !hidden && best.as_ref().is_none_or(|b| t < b.distance) {
                    best = Some(PlaneHit {
                        object: i,
                        plane: pi,
                        distance: t,
                        normal: (r * Vec3::from(p.normal.map(|c| c as f32))).normalize_or_zero(),
                    });
                }
            }
        }
        best
    }
}

/// The rotation and translation taking the scaled mesh frame (`vertex × scale × mirror`) to
/// the world: `world = r · q + offset`.
pub fn frame_of(o: &Object) -> (glam::Quat, Vec3) {
    (o.rotation(), o.transform().transform_point3(Vec3::ZERO))
}

/// World points of a plane's outline.
pub fn world_outline(o: &Object, points: &[[f64; 3]]) -> Vec<Vec3> {
    let (r, offset) = frame_of(o);
    points
        .iter()
        .map(|p| r * Vec3::from(p.map(|c| c as f32)) + offset)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cube() -> Object {
        let mesh = TriangleMesh::load_stl(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/models/cube20.stl"
        ))
        .unwrap();
        let mut o = Object::new(mica_core::input::InputObject::plain(
            "cube",
            0,
            0,
            mesh,
            mica_core::print_job::ToolAssignments([Some(0); 5]),
            mica_core::config::Config::defaults(),
        ));
        o.position = Vec3::new(100., 100., 0.);
        o
    }

    #[test]
    fn planes_are_cached_per_mesh_and_scale_and_picked_in_the_world() {
        let ctx = egui::Context::default();
        let mut cache = PlaneCache {
            blocking: true,
            ..Default::default()
        };
        let mut o = cube();
        o.drop_to_bed();
        let planes = cache.planes(&o, &ctx).unwrap();
        assert_eq!(planes.len(), 6);
        // A rotation reuses the entry; a scale makes a new one.
        o.angle = 0.7;
        assert!(Arc::ptr_eq(&planes, &cache.planes(&o, &ctx).unwrap()));
        let mut scaled = o.clone();
        scaled.scale = Vec3::splat(2.);
        assert!(!Arc::ptr_eq(&planes, &cache.planes(&scaled, &ctx).unwrap()));
        assert_eq!(cache.entries.len(), 2);
        // Looking straight down at the cube hits its top.
        let (min, max) = o.bounds();
        let centre = (min + max) / 2.;
        let objects = [o.clone()];
        let hit = cache
            .pick(
                &objects,
                &[0],
                Vec3::new(centre.x, centre.y, 100.),
                -Vec3::Z,
                None,
                &ctx,
            )
            .unwrap();
        assert!(hit.normal.z > 0.99, "{hit:?}");
        assert!((hit.distance - (100. - max.z)).abs() < 1e-3);
        // From the side, after the turn, the hit normal is the turned side's.
        let hit = cache
            .pick(
                &objects,
                &[0],
                centre + Vec3::new(-100. * 0.7f32.cos(), -100. * 0.7f32.sin(), 0.),
                Vec3::new(0.7f32.cos(), 0.7f32.sin(), 0.),
                None,
                &ctx,
            )
            .unwrap();
        assert!(
            hit.normal
                .dot(Vec3::new(-(0.7f32.cos()), -(0.7f32.sin()), 0.))
                > 0.99
        );
        // Laying on it puts that side down and keeps the cube on the bed.
        o.lay_on_face(hit.normal);
        let (min, max) = o.bounds();
        assert!(min.z.abs() < 1e-3 && (max.z - 20.).abs() < 1e-3);
    }
}
