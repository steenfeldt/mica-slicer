// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

use super::Object;
use glam::Vec3;

pub fn triangle(origin: Vec3, ray: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Option<f32> {
    let e1 = b - a;
    let e2 = c - a;
    let p = ray.cross(e2);
    let det = e1.dot(p);
    if det.abs() < 1e-7 {
        return None;
    }
    let t = origin - a;
    let u = t.dot(p) / det;
    if !(0. ..=1.).contains(&u) {
        return None;
    }
    let q = t.cross(e1);
    let v = ray.dot(q) / det;
    if v < 0. || u + v > 1. {
        return None;
    }
    let distance = e2.dot(q) / det;
    (distance >= 0.).then_some(distance)
}

/// Nearest hit along a world ray.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    pub object: usize,
    pub face: usize,
    /// Distance along the (unit) world ray.
    pub distance: f32,
}

/// Nearest face of one mesh placed with `transform` along a world ray: (face, distance).
pub fn pick_mesh(
    mesh: &mica_core::mesh::TriangleMesh,
    local_bounds: ([f32; 3], [f32; 3]),
    transform: glam::Mat4,
    origin: Vec3,
    ray: Vec3,
) -> Option<(usize, f32)> {
    let inv = transform.inverse();
    let a = inv.transform_point3(origin);
    let d = inv.transform_vector3(ray);
    // Non-uniform scale changes the ray length; convert local distances back to world.
    let scale = d.length();
    if scale < 1e-9 {
        return None;
    }
    let d = d / scale;
    let (min, max) = local_bounds;
    let lo = (Vec3::from(min) - a) / d;
    let hi = (Vec3::from(max) - a) / d;
    if lo.min(hi).max_element() > lo.max(hi).min_element() {
        return None;
    }
    let mut hit: Option<(usize, f32)> = None;
    for (f, face) in mesh.faces.iter().enumerate() {
        let [p, q, r] = face.map(|j| Vec3::from(mesh.vertices[j as usize]));
        if let Some(t) = triangle(a, d, p, q, r) {
            let world = t / scale;
            if hit.is_none_or(|(_, best)| world < best) {
                hit = Some((f, world));
            }
        }
    }
    hit
}

pub fn pick_face(objects: &[Object], origin: Vec3, ray: Vec3) -> Option<Hit> {
    let mut hit: Option<Hit> = None;
    for (i, o) in objects.iter().enumerate() {
        if let Some((face, distance)) =
            pick_mesh(&o.mesh, o.local_bounds, o.transform(), origin, ray)
            && hit.is_none_or(|h| distance < h.distance)
        {
            hit = Some(Hit {
                object: i,
                face,
                distance,
            });
        }
    }
    hit
}

/// The nearest of an object's volumes under the ray: (volume id, distance).
pub fn pick_volume(o: &Object, origin: Vec3, ray: Vec3) -> Option<(u64, f32)> {
    nearest_volume(o, origin, ray, |_| true)
}
/// The nearest embossed (text / SVG) volume of `o` under the ray, and its distance.
pub fn pick_text_volume(o: &Object, origin: Vec3, ray: Vec3) -> Option<(u64, f32)> {
    nearest_volume(o, origin, ray, |v| v.emboss.is_some())
}
fn nearest_volume(
    o: &Object,
    origin: Vec3,
    ray: Vec3,
    keep: impl Fn(&crate::scene::Volume) -> bool,
) -> Option<(u64, f32)> {
    let mut best: Option<(u64, f32)> = None;
    for v in o.volumes.iter().filter(|v| keep(v)) {
        if let Some((_, d)) = pick_mesh(&v.mesh, v.local_bounds, o.volume_transform(v), origin, ray)
            && best.is_none_or(|(_, b)| d < b)
        {
            best = Some((v.id, d));
        }
    }
    best
}
/// What a click (or the hover) under the ray picks: an object, or a part of one.
///
/// Drill-down: with object `selected`, any of its volumes under the ray is picked (x-ray:
/// a modifier hidden inside the body counts), unless another object stands in front. With
/// `direct` (Option / Alt held) the part under the pointer is picked whatever is selected:
/// a volume of the object hit, else the nearest volume of any object. Otherwise the object
/// whose surface is hit.
pub fn click_target(
    objects: &[Object],
    selected: Option<usize>,
    origin: Vec3,
    ray: Vec3,
    direct: bool,
) -> Option<(usize, Option<u64>)> {
    let hit = pick_face(objects, origin, ray);
    if direct {
        if let Some(h) = hit
            && let Some((id, _)) = objects
                .get(h.object)
                .and_then(|o| pick_volume(o, origin, ray))
        {
            return Some((h.object, Some(id)));
        }
        let nearest = objects
            .iter()
            .enumerate()
            .filter_map(|(i, o)| pick_volume(o, origin, ray).map(|(id, d)| (i, id, d)))
            .min_by(|a, b| a.2.total_cmp(&b.2));
        if let Some((i, id, _)) = nearest {
            return Some((i, Some(id)));
        }
    } else if let Some(s) = selected
        && let Some(o) = objects.get(s)
        && let Some((id, d)) = pick_volume(o, origin, ray)
        && hit.is_none_or(|h| h.object == s || d <= h.distance + 1e-3)
    {
        return Some((s, Some(id)));
    }
    hit.map(|h| (h.object, None))
}
pub fn pick(objects: &[Object], origin: Vec3, ray: Vec3) -> Option<usize> {
    pick_face(objects, origin, ray).map(|h| h.object)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn triangle_hits_misses_and_backface() {
        let a = Vec3::ZERO;
        let b = Vec3::X;
        let c = Vec3::Y;
        assert_eq!(
            triangle(Vec3::new(0.2, 0.2, 3.), -Vec3::Z, a, b, c),
            Some(3.)
        );
        assert_eq!(
            triangle(Vec3::new(0.2, 0.2, 3.), -Vec3::Z, c, b, a),
            Some(3.)
        );
        assert_eq!(triangle(Vec3::new(2., 2., 3.), -Vec3::Z, a, b, c), None);
        assert_eq!(triangle(Vec3::new(0.2, 0.2, 3.), Vec3::Z, a, b, c), None);
    }
}

#[cfg(test)]
mod scene_tests {
    use super::*;
    use mica_core::{input::InputObject, mesh::TriangleMesh, print_job::ToolAssignments};
    #[test]
    fn nearest_transformed_object_wins() {
        let mesh = TriangleMesh {
            vertices: vec![[0., 0., 0.], [2., 0., 0.], [0., 2., 0.]],
            faces: vec![[0, 1, 2]],
        };
        let mut object = Object::new(InputObject::plain(
            "triangle",
            0,
            0,
            mesh,
            ToolAssignments([Some(0); 5]),
            mica_core::config::Config::defaults(),
        ));
        object.position = Vec3::new(5., 4., 1.);
        object.angle = std::f32::consts::FRAC_PI_2;
        let target = object.transform().transform_point3(Vec3::new(0.4, 0.4, 0.));
        let mut near = object.clone();
        near.position.z += 2.;
        near.scale = Vec3::new(2., 2., 1.);
        let hit = pick_face(&[object, near], target + Vec3::Z * 10., -Vec3::Z).unwrap();
        assert_eq!((hit.object, hit.face), (1, 0));
        // Ray starts 10 above the lower triangle (z = 1); the near one sits at z = 3.
        assert!((hit.distance - 8.).abs() < 1e-4);
    }
}
