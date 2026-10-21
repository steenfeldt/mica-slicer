// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

use super::*;
use crate::texture::{Curve, Source};
use std::f64::consts::FRAC_1_SQRT_2;

fn settings(pairs: &[(&str, &str)]) -> TextureSettings {
    TextureSettings::from_lookup(|k| {
        pairs
            .iter()
            .find(|(key, _)| *key == k)
            .map(|(_, v)| v.to_string())
    })
    .unwrap()
}

/// A closed box mesh from `min` to `max`, outward facing.
fn cube(min: [f32; 3], max: [f32; 3]) -> TriangleMesh {
    let v = |i: usize| {
        [
            if i & 1 == 0 { min[0] } else { max[0] },
            if i & 2 == 0 { min[1] } else { max[1] },
            if i & 4 == 0 { min[2] } else { max[2] },
        ]
    };
    let faces = vec![
        [0, 2, 1],
        [1, 2, 3],
        [4, 5, 6],
        [5, 7, 6],
        [0, 1, 4],
        [1, 5, 4],
        [2, 6, 3],
        [3, 6, 7],
        [0, 4, 2],
        [2, 4, 6],
        [1, 3, 5],
        [3, 7, 5],
    ];
    TriangleMesh {
        vertices: (0..8).map(v).collect(),
        faces,
    }
}

/// Counter-clockwise square wall loop `inset` mm inside a `size` mm square at the origin.
fn square(size: f64, inset: f64) -> Polygon {
    let (a, b) = (inset, size - inset);
    [(a, a), (b, a), (b, b), (a, b)]
        .iter()
        .map(|&(x, y)| Point::from_mm(x, y))
        .collect()
}

fn self_intersects(poly: &[Point]) -> bool {
    let n = poly.len();
    for i in 0..n {
        for j in i + 2..n {
            if i == 0 && j == n - 1 {
                continue;
            }
            if crossing(poly[i], poly[(i + 1) % n], poly[j], poly[(j + 1) % n]).is_some() {
                return true;
            }
        }
    }
    false
}

#[test]
fn defaults_leave_walls_alone() {
    let c = crate::config::Config::defaults();
    let s = TextureSettings::from_config(&c).unwrap();
    assert!(!s.enabled());
    let mesh = cube([0.; 3], [20.; 3]);
    assert!(
        TextureSkin::prepare(&mesh, &[&s], None, &[0.1, 0.3])
            .unwrap()
            .is_none()
    );
}

#[test]
fn square_wall_gets_textured_closed_and_untangled() {
    let mesh = cube([0.; 3], [20.; 3]);
    let zs: Vec<f64> = (0..50).map(|i| 0.1 + 0.2 * i as f64).collect();
    let flow = Flow::new(0.45, 0.2);
    for direction in ["outward", "inward", "both"] {
        let s = settings(&[
            ("mica_texture_skin", "external"),
            ("mica_texture_skin_texture", "procedural:knurl_diamond"),
            ("mica_texture_skin_scale", "2"),
            ("mica_texture_skin_amplitude", "0.6"),
            ("mica_texture_skin_direction", direction),
        ]);
        let skin = TextureSkin::prepare(&mesh, &[&s], None, &zs)
            .unwrap()
            .unwrap();
        let surface = Surface {
            skin: &skin,
            offset: Point::default(),
        };
        let mut poly = square(20., 0.2);
        let before = poly.clone();
        surface.displace_polygon(&mut poly, 10, 0, true, &s, &flow);
        assert!(poly.len() > 100, "{direction}: resampled");
        assert!(!self_intersects(&poly), "{direction}: no loops");
        let area = signed_area(&poly);
        let area0 = signed_area(&before);
        match direction {
            "outward" => assert!(area > area0),
            "inward" => assert!(area < area0),
            _ => {}
        }
        // Classic walls cap outward at half the line width and inward at half the spacing.
        let reach = poly
            .iter()
            .map(|p| {
                let (x, y) = (p.x as f64 / SCALE, p.y as f64 / SCALE);
                (0.2 - x).max(0.2 - y).max(x - 19.8).max(y - 19.8)
            })
            .fold(f64::NEG_INFINITY, f64::max);
        assert!(
            reach <= 0.5 * 0.45 * 2f64.sqrt() + 1e-3,
            "{direction}: {reach}"
        );
        // Layer 0 stays plain.
        let mut first = square(20., 0.2);
        surface.displace_polygon(&mut first, 0, 0, true, &s, &flow);
        assert_eq!(first, square(20., 0.2));
        // Inner walls stay plain.
        let mut inner = square(20., 0.6);
        surface.displace_polygon(&mut inner, 10, 1, true, &s, &flow);
        assert_eq!(inner, square(20., 0.6));
    }
}

#[test]
fn hole_loops_follow_the_mode() {
    let mesh = cube([0.; 3], [20.; 3]);
    let zs = [0.1, 0.3, 0.5];
    let flow = Flow::new(0.45, 0.2);
    let ext = settings(&[("mica_texture_skin", "external")]);
    let skin = TextureSkin::prepare(&mesh, &[&ext], None, &zs)
        .unwrap()
        .unwrap();
    let surface = Surface {
        skin: &skin,
        offset: Point::default(),
    };
    let mut hole: Polygon = square(20., 0.2).into_iter().rev().collect();
    let before = hole.clone();
    surface.displace_polygon(&mut hole, 1, 0, false, &ext, &flow);
    assert_eq!(hole, before);
}

#[test]
fn untangle_cuts_a_swallowtail() {
    // A loop with a small knot: 0 → 3 crosses 1 → 2 backwards.
    let mm = |x: f64, y: f64| (Point::from_mm(x, y), 0.);
    let mut pts = vec![
        mm(0., 0.),
        mm(10., 0.),
        mm(10.5, 1.),
        mm(9.5, 1.),
        mm(10.2, -0.5),
        mm(12., 0.),
        mm(12., 10.),
        mm(0., 10.),
    ];
    untangle(&mut pts, 8);
    let poly: Vec<Point> = pts.iter().map(|p| p.0).collect();
    assert!(!self_intersects(&poly));
    assert!(poly.len() < 8);
}

#[test]
fn mitre_moves_both_edges_by_one() {
    let m = mitre([0., -1.], [1., 0.]);
    assert!((m[0] - 1.).abs() < 1e-12 && (m[1] + 1.).abs() < 1e-12);
    // Straight run: the normal itself.
    assert_eq!(mitre([0., 1.], [0., 1.]), [0., 1.]);
    // Very sharp: capped at twice the normal.
    let m = mitre([0.999, 0.0447], [-0.999, 0.0447]);
    assert!(m[0].hypot(m[1]) <= 2. + 1e-9);
}

#[test]
fn falloff_curves_match_bumpmesh() {
    assert_eq!(Curve::Linear.apply(0.25), 0.25);
    assert!((Curve::SCurve.apply(0.25) - 0.15625).abs() < 1e-12);
    assert_eq!(Curve::Ease.apply(0.5), 0.25);
    assert_eq!(Curve::Ease.apply(2.), 1.);
}

#[test]
fn normals_come_from_the_nearest_facet() {
    // A wedge: the +X face leans back 45°.
    let mesh = TriangleMesh {
        vertices: vec![
            [0., 0., 0.],
            [10., 0., 0.],
            [10., 10., 0.],
            [0., 10., 0.],
            [0., 0., 10.],
            [0., 10., 10.],
        ],
        faces: vec![
            [0, 2, 1],
            [0, 3, 2],
            [1, 2, 5],
            [1, 5, 4],
            [0, 1, 4],
            [3, 5, 2],
            [0, 4, 5],
            [0, 5, 3],
        ],
    };
    let zs = [2.5, 5.];
    let n = SurfaceNormals::new(&mesh, &zs);
    let layer = n.layer(1).unwrap();
    // Wall point just inside the sloped face at z = 5 (the face is at x = 5 there).
    let normal = layer.normal([4.8, 5.], [1., 0.], 1.).unwrap();
    assert!(
        (normal[0] - FRAC_1_SQRT_2).abs() < 0.01 && (normal[2] - FRAC_1_SQRT_2).abs() < 0.01,
        "{normal:?}"
    );
    // From the left side the vertical face is found instead.
    let left = layer.normal([0.2, 5.], [-1., 0.], 1.).unwrap();
    assert!((left[0] + 1.).abs() < 1e-3, "{left:?}");
}

#[test]
fn painted_slots_texture_unpainted_objects() {
    let mesh = cube([0.; 3], [20.; 3]);
    let mut painting = crate::painting::FacetPainting::default();
    // Paint the two −Y triangles (indices 4 and 5) with slot 2.
    painting.texture.push(4, "8").unwrap();
    painting.texture.push(5, "8").unwrap();
    let model = crate::regions::RegionModel {
        volumes: vec![crate::regions::Volume {
            mesh: mesh.clone(),
            painting,
            kind: crate::threemf::PartKind::Model,
            config: crate::config::Config::new(),
        }],
        ..Default::default()
    };
    let s = settings(&[
        (
            "mica_texture_paint_slots",
            "2:texture=procedural:ribs,amplitude=0.3",
        ),
        ("mica_texture_skin_falloff", "0"),
    ]);
    assert_eq!(s.slot(2).unwrap().source, Source::Procedural("ribs".into()));
    let zs: Vec<f64> = (0..20).map(|i| 0.1 + 0.2 * i as f64).collect();
    let skin = TextureSkin::prepare(&mesh, &[&s], Some(&model), &zs)
        .unwrap()
        .unwrap();
    let surface = Surface {
        skin: &skin,
        offset: Point::default(),
    };
    let flow = Flow::new(0.45, 0.2);
    let mut poly = square(20., 0.2);
    surface.displace_polygon(&mut poly, 5, 0, true, &s, &flow);
    // Only the −Y side (y ≈ 0.2) moved.
    let moved_y = poly.iter().any(|p| (p.y as f64 / SCALE) < 0.19);
    let moved_x = poly
        .iter()
        .any(|p| (p.x as f64 / SCALE) > 19.81 && (p.y as f64 / SCALE) > 2.);
    assert!(moved_y && !moved_x);
}

/// A painted slot's own depth sets how far its walls move; without one it uses the object's.
#[test]
fn painted_slots_use_their_own_depth() {
    let mesh = cube([0.; 3], [20.; 3]);
    let mut painting = crate::painting::FacetPainting::default();
    painting.texture.push(4, "4").unwrap();
    painting.texture.push(5, "4").unwrap();
    let model = crate::regions::RegionModel {
        volumes: vec![crate::regions::Volume {
            mesh: mesh.clone(),
            painting,
            kind: crate::threemf::PartKind::Model,
            config: crate::config::Config::new(),
        }],
        ..Default::default()
    };
    let zs: Vec<f64> = (0..20).map(|i| 0.1 + 0.2 * i as f64).collect();
    let reach = |slots: &str| {
        let s = settings(&[
            ("mica_texture_paint_slots", slots),
            ("mica_texture_skin_amplitude", "0.4"),
            ("mica_texture_skin_direction", "outward"),
            ("mica_texture_skin_falloff", "0"),
        ]);
        let skin = TextureSkin::prepare(&mesh, &[&s], Some(&model), &zs)
            .unwrap()
            .unwrap();
        let surface = Surface {
            skin: &skin,
            offset: Point::default(),
        };
        let mut poly = square(20., 0.2);
        surface.displace_polygon(&mut poly, 5, 0, true, &s, &Flow::new(0.45, 0.2));
        // How far the −Y wall (at y = 0.2) moved outward.
        poly.iter()
            .filter(|p| (p.x as f64 / SCALE) > 2. && (p.x as f64 / SCALE) < 18.)
            .map(|p| 0.2 - p.y as f64 / SCALE)
            .fold(0., f64::max)
    };
    let own = reach("1:texture=procedural:ribs,amplitude=0.1");
    let inherited = reach("1:texture=procedural:ribs");
    assert!(own > 0.05 && own <= 0.1 + 1e-6, "{own}");
    assert!(inherited > 0.15 && inherited <= 0.4 + 1e-6, "{inherited}");
}

/// Settings on all surfaces leave the walls to the displaced mesh: no wall texture context.
#[test]
fn all_surfaces_leave_the_walls_to_the_mesh() {
    let mesh = cube([0.; 3], [20.; 3]);
    let s = settings(&[
        ("mica_texture_skin", "external"),
        ("mica_texture_skin_surfaces", "walls_and_tops"),
    ]);
    assert!(
        TextureSkin::prepare(&mesh, &[&s], None, &[0.1, 0.3])
            .unwrap()
            .is_none()
    );
    assert!(warnings(&s, &Flow::new(0.45, 0.2), false, 1).is_empty());
}

/// The steep walls the displacement leaves to the wall texture get it by their share: all of
/// it away from the rims, none at a rim (the mesh's), on holes too.
#[test]
fn steep_walls_left_by_the_mesh_get_the_texture_by_their_share() {
    let mesh = cube([0.; 3], [20.; 3]);
    let zs: Vec<f64> = (0..100).map(|i| 0.1 + 0.2 * i as f64).collect();
    let flow = Flow::new(0.45, 0.2);
    let s = settings(&[
        ("mica_texture_skin", "external"),
        ("mica_texture_skin_surfaces", "walls_and_tops"),
        ("mica_texture_skin_texture", "procedural:knurl_diamond"),
        ("mica_texture_skin_scale", "2"),
        ("mica_texture_skin_amplitude", "0.2"),
    ]);
    let top = |a: [f64; 2], b: [f64; 2]| [[a[0], a[1], 20.], [b[0], b[1], 20.]];
    let rims = Arc::new(crate::texture::displace::Rims::new(
        vec![
            top([0., 0.], [20., 0.]),
            top([20., 0.], [20., 20.]),
            top([20., 20.], [0., 20.]),
            top([0., 20.], [0., 0.]),
        ],
        0.3,
        1.2,
    ));
    let skin = TextureSkin::prepare_with_walls(&mesh, &[&s], None, &zs, Some(rims))
        .unwrap()
        .unwrap();
    let surface = Surface {
        skin: &skin,
        offset: Point::default(),
    };
    // Mid-height: the whole texture.
    let mut mid = square(20., 0.2);
    surface.displace_polygon(&mut mid, 50, 0, true, &s, &flow);
    assert!(mid.len() > 100);
    assert!(signed_area(&mid) > signed_area(&square(20., 0.2)));
    // At the rim the mesh carries all of it.
    let mut rim = square(20., 0.2);
    surface.displace_polygon(&mut rim, 99, 0, true, &s, &flow);
    assert_eq!(rim, square(20., 0.2));
    // A hole's wall is textured too ("Outer walls" covers holes on all surfaces, as in the
    // mesh).
    let mut holed = cube([0.; 3], [20.; 3]);
    let inner = cube([5.; 3], [15., 15., 20.]);
    let base = holed.vertices.len() as u32;
    holed.vertices.extend(inner.vertices);
    // Facing into the hole.
    holed.faces.extend(
        inner
            .faces
            .iter()
            .map(|f| [f[0] + base, f[2] + base, f[1] + base]),
    );
    let no_rims = Arc::new(crate::texture::displace::Rims::new(vec![], 0.3, 1.2));
    let skin = TextureSkin::prepare_with_walls(&holed, &[&s], None, &zs, Some(no_rims))
        .unwrap()
        .unwrap();
    let surface = Surface {
        skin: &skin,
        offset: Point::default(),
    };
    let mut hole: Polygon = [(5.2, 5.2), (5.2, 14.8), (14.8, 14.8), (14.8, 5.2)]
        .iter()
        .map(|&(x, y)| Point::from_mm(x, y))
        .collect();
    let before = hole.clone();
    surface.displace_polygon(&mut hole, 50, 0, false, &s, &flow);
    assert!(hole.len() > 50);
    // Outward from the material is into the hole: the hole gets smaller.
    assert!(signed_area(&hole).abs() < signed_area(&before).abs());
}
