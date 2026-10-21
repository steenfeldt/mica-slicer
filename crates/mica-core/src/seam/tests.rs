// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 tests/fff_print/test_seam_aligned.cpp :: Snap to angle, Nearest point, Pick seam option picks enforcer
// Source: prusaslicer@b028299c77 tests/fff_print/test_seam_random.cpp :: Random is uniform, Random respects point type
// Source: prusaslicer@b028299c77 tests/fff_print/test_seam_geometry.cpp :: Lists mapping, Vertex angle calculation counterclockwise, Vertex angle calculation clockwise
// Source: prusaslicer@b028299c77 tests/fff_print/test_seam_scarf.cpp :: Ensure max distance, Lineary increase extrusion height, Lineary reduce extrusion amount
// Source: prusaslicer@b028299c77 tests/fff_print/test_seam_perimeters.cpp :: Oversample painted
// Changes: Mica data structures; additional integration regressions for visibility, clipping, painting, holes and batching.

use super::*;
use crate::geometry::{Polygon, polyline_length};
use crate::mesh::tests::box_mesh;
use crate::print::{LayerSource, prepare_object};
use crate::settings::FloatOrPercent;
use painting::{NoPainting, Paint};
fn square() -> Polygon {
    [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)]
        .map(|(x, y)| Point::from_mm(x, y))
        .to_vec()
}
fn boundary(points: &[(f64, f64)]) -> Boundary {
    Boundary::new(
        &points.iter().map(|&(x, y)| Point::from_mm(x, y)).collect(),
        &[],
        1.0,
        0.2,
        &NoPainting,
    )
    .unwrap()
}
fn fixture() -> Boundary {
    boundary(&[(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0), (0.0, 0.5)])
}

#[test]
fn vertex_angles_both_windings() {
    let mut p: Vec<V> = square().into_iter().map(V::from).collect();
    for a in geometry::angles(&p, 0.1) {
        assert!((a + std::f64::consts::FRAC_PI_2).abs() < 1e-12);
    }
    p.reverse();
    for a in geometry::angles(&p, 0.1) {
        assert!((a - std::f64::consts::FRAC_PI_2).abs() < 1e-12);
    }
}
#[test]
fn upstream_lists_mapping() {
    let lists = [
        vec![],
        vec![7i32, 2, 3],
        vec![9, 6, 3, 6, 7],
        vec![1, 1, 3],
        vec![1],
        vec![3],
        vec![1],
        vec![],
        vec![3],
    ];
    let sizes: Vec<usize> = lists.iter().map(Vec::len).collect();
    let (m, n) = geometry::mapping(&sizes, |l, i| {
        let mut best = None;
        let mut weight = 0;
        for (j, &v) in lists[l + 1].iter().enumerate() {
            let d = (v - lists[l][i]).abs();
            if d > weight {
                weight = d;
                best = Some((j, d as f64));
            }
        }
        best
    });
    assert_eq!(
        m,
        vec![
            vec![],
            vec![0, 1, 2],
            vec![1, 3, 0, 4, 5],
            vec![1, 6, 7],
            vec![7],
            vec![7],
            vec![7],
            vec![],
            vec![8]
        ]
    );
    assert_eq!(n, 9);
    assert!(geometry::mapping(&[], |_, _| None).0.is_empty());
}
#[test]
fn snap_to_angle() {
    let b = fixture();
    assert_eq!(choice::snap(&b, V::new(0.0, 0.4), 4, 0.5), Some(0));
    assert_eq!(choice::snap(&b, V::new(0.0, 0.4), 4, 0.3), None);
}
#[test]
fn nearest_point_on_edge() {
    let b = fixture();
    let c = choice::nearest(&b, V::new(0.4, -0.1), 0.2, false);
    assert_eq!((c.previous, c.next), (0, 1));
    assert!((c.point - V::new(0.4, 0.0)).norm() < 1e-9);
}
#[test]
fn nearest_corners_and_enforcer() {
    let mut b = fixture();
    let c = choice::nearest(&b, V::new(0.1, 0.5), 1.0, true);
    assert!(!b.smooth(c));
    b.set_paint(4, Paint::Enforcer);
    let c = choice::nearest(&b, V::new(0.1, 0.5), 1.0, true);
    assert_eq!(c.point, b.points[4]);
}
#[test]
fn rear_splits_flat_edge() {
    let b = fixture();
    let c = choice::rear(&b);
    assert!((c.point - V::new(0.5, 1.0)).norm() < 1e-9);
}
#[test]
fn random_is_uniform_and_respects_blockers() {
    let mut b = boundary(&[(0.0, 0.0), (0.5, 0.0), (1.0, 0.0)]);
    let mut r = choice::Random::new(42);
    let mut bins = [0usize; 11];
    for _ in 0..1001 {
        let c = r.choose(&b);
        bins[(c.point.x * 11.0) as usize] += 1;
    }
    let expected = 1001.0 / 11.0;
    let chi: f64 = bins
        .iter()
        .map(|&n| (n as f64 - expected).powi(2) / expected)
        .sum();
    assert!(chi < 29.588, "{chi}");
    b.set_paint(0, Paint::Blocker);
    for _ in 0..100 {
        assert_ne!(r.choose(&b).previous, 0);
    }
}
#[test]
fn clipping_and_negative_overlap() {
    for gap in [0.06, 0.0, -0.2, 9.0, 39.99, 40.0, 50.0] {
        let mut p = geometry::open(&square(), V::new(10.0, 10.0));
        scarf::gap(&mut p, &mut None, gap);
        let expected = (40.0 - gap).max(0.0);
        assert!(
            (polyline_length(&p) / SCALE - expected).abs() < 1e-6,
            "{gap}"
        );
    }
}
#[test]
fn scarf_ramps_resolution_and_volume() {
    for length in [4.3, 40.0] {
        let s = ScarfSettings {
            max_segment_length: 0.7,
            start_height: 0.2,
            ..ScarfSettings::default()
        };
        let (points, meta) = scarf::add(&square(), V::new(10.0, 10.0), length, &s);
        assert_eq!(points.len(), meta.samples.len());
        assert!((meta.samples[0].height - 0.2).abs() < 1e-9);
        assert_eq!(meta.samples[0].extrusion, 0.0);
        assert_eq!(meta.samples.last().unwrap().extrusion, 0.0);
        let mut volume = 0.0;
        for (i, p) in points.windows(2).enumerate() {
            let l = p[0].distance_to(p[1]) / SCALE;
            volume += l * meta.samples[i + 1].extrusion;
            if meta.samples[i + 1].extrusion < 1.0 || meta.samples[i].height < 1.0 {
                assert!(l <= 0.700_002);
            }
        }
        assert!((volume - 40.0).abs() < 1e-5, "{volume}");
        let mut meta = Some(Box::new(meta));
        let mut points = points;
        scarf::gap(&mut points, &mut meta, 0.06);
        assert_eq!(points.len(), meta.unwrap().samples.len());
    }
}
#[test]
fn visibility_detects_occlusion() {
    let cube = box_mesh([0.0; 3], [10.0; 3]);
    let tree = visibility::MeshTree::new(&cube);
    assert!(tree.occluded([-1.0, 5.0, 5.0], [1.0, 0.0, 0.0]));
    assert!(!tree.occluded([-1.0, 5.0, 5.0], [-1.0, 0.0, 0.0]));
    let vis = visibility::Visibility::new(&cube);
    assert!(vis.at([0.0, 5.0, 5.0]) > 0.95);
    let mut cavity = box_mesh([0.0; 3], [10.0; 3]);
    for f in &mut cavity.faces {
        f.swap(1, 2);
    }
    let vis = visibility::Visibility::new(&cavity);
    assert!(vis.at([0.0, 5.0, 5.0]) < 0.1);
}
#[test]
fn painting_triangle_queries_and_edge_sampling() {
    let facets =
        TriangleMesh::from_triangles(&[[[4.0, 0.0, 0.0], [6.0, 0.0, 0.0], [5.0, 0.0, 2.0]]]);
    let empty = TriangleMesh::default();
    let paint = painting::MeshPainting::new(&facets, &empty);
    assert_eq!(paint.classify([5.0, 0.0, 1.0], 0.05), Paint::Enforcer);
    let b = Boundary::new(&square(), &[], 1.0, 0.2, &paint).unwrap();
    let c = choice::nearest(&b, V::new(0.0, 0.0), 1.0, true);
    assert!(c.point.x >= 4.5 && c.point.x <= 5.5);
    assert!(c.point.y.abs() < 1e-9);
    let paint = painting::MeshPainting::new(&facets, &facets);
    assert_eq!(paint.classify([5.0, 0.0, 1.0], 0.05), Paint::Blocker);
}
#[test]
fn config_overrides_and_validation() {
    let mut c = crate::config::Config::defaults();
    c.set_raw("seam_gap_distance", "25%").unwrap();
    c.set_raw("filament_seam_gap_distance", "0.3").unwrap();
    c.set_raw("filament_wipe", "1").unwrap();
    let s = SeamSettings::from_config(&c).unwrap();
    assert_eq!(s.gap, FloatOrPercent::Mm(0.3));
    assert!(s.wipe);
    c.set_raw("filament_seam_gap_distance", "nil").unwrap();
    assert_eq!(
        SeamSettings::from_config(&c).unwrap().gap,
        FloatOrPercent::Percent(25.0)
    );
    c.set_raw("scarf_seam_max_segment_length", "0").unwrap();
    assert!(SeamSettings::from_config(&c).is_err());
}
#[test]
fn aligned_cube_and_batch_independence() {
    let m = box_mesh([0.0; 3], [10.0, 10.0, 1.0]);
    // Classic loops carry the full seam metadata (wipe, scarf); Arachne runs are tested below.
    let mut s = PrintSettings::core_one_hf04_speed_pla();
    s.perimeter_generator = crate::settings::PerimeterGenerator::Classic;
    let obj = prepare_object(&m, &s).unwrap();
    assert_eq!(obj.seams.seam_count(), 5);
    let all = obj.toolpaths(0..5);
    let mut split = obj.toolpaths(3..5);
    split.extend(obj.toolpaths(0..3));
    for l in &all {
        let other = split.iter().find(|b| b.index == l.index).unwrap();
        assert_eq!(l.paths, other.paths);
        let outer = l
            .paths
            .iter()
            .find(|p| p.role == Role::ExternalPerimeter)
            .unwrap();
        assert!(outer.points[0].x > 9_000_000 && outer.points[0].y > 9_000_000);
        assert!(outer.seam.as_ref().unwrap().wipe.is_some());
    }
}
#[test]
fn random_plan_reproducible() {
    let m = box_mesh([0.0; 3], [10.0, 10.0, 1.0]);
    let mut s = PrintSettings::core_one_hf04_speed_pla();
    s.seam.position = SeamPosition::Random;
    let a = prepare_object(&m, &s).unwrap();
    let b = prepare_object(&m, &s).unwrap();
    let pa: Vec<_> = (0..5).flat_map(|i| a.seams.points(i)).collect();
    let pb: Vec<_> = (0..5).flat_map(|i| b.seams.points(i)).collect();
    assert_eq!(pa, pb);
    assert!(pa.windows(2).any(|p| p[0] != p[1]));
}
#[test]
fn scarf_first_layer_smooth_gate_and_inner_stagger() {
    let m = box_mesh([0.0; 3], [10.0, 10.0, 0.6]);
    let mut s = PrintSettings::core_one_hf04_speed_pla();
    s.perimeter_generator = crate::settings::PerimeterGenerator::Classic;
    s.seam.scarf.placement = ScarfPlacement::Everywhere;
    s.seam.scarf.length = 4.0;
    let obj = prepare_object(&m, &s).unwrap();
    assert!(
        obj.toolpaths(0..3)
            .iter()
            .flat_map(|l| &l.paths)
            .all(|p| p.seam.as_ref().is_none_or(|m| m.samples.is_empty()))
    );
    s.seam.scarf.only_on_smooth = false;
    s.seam.scarf.on_inner_perimeters = true;
    let obj = prepare_object(&m, &s).unwrap();
    let layers = obj.toolpaths(0..3);
    assert!(
        layers[0]
            .paths
            .iter()
            .all(|p| p.seam.as_ref().is_none_or(|m| m.samples.is_empty()))
    );
    assert_eq!(
        layers[1]
            .paths
            .iter()
            .filter(|p| p.seam.as_ref().is_some_and(|m| !m.samples.is_empty()))
            .count(),
        2
    );
    s.seam.scarf.placement = ScarfPlacement::Nowhere;
    s.seam.staggered_inner_seams = true;
    let a = prepare_object(&m, &s).unwrap().toolpaths(1..2);
    s.seam.staggered_inner_seams = false;
    let b = prepare_object(&m, &s).unwrap().toolpaths(1..2);
    for role in [Role::Perimeter, Role::ExternalPerimeter] {
        let pa = &a[0].paths.iter().find(|p| p.role == role).unwrap().points;
        let pb = &b[0].paths.iter().find(|p| p.role == role).unwrap().points;
        assert_eq!(pa[0] == pb[0], role == Role::ExternalPerimeter);
    }
}
#[test]
fn inward_wipe_on_contours_holes_and_zero_gap() {
    for hole in [false, true] {
        let mut poly = square();
        if hole {
            poly.reverse();
        }
        for gap in [0.0, 0.06] {
            let mut points = geometry::open(&poly, V::new(10.0, 10.0));
            scarf::gap(&mut points, &mut None, gap);
            let p = scarf::wipe(&points, &poly, 0.4).unwrap();
            if gap == 0.0 {
                assert_ne!(geometry::contains(&poly, V::from(p)), hole);
            } else {
                // Upstream rotates by the full wedge angle, including for holes.
                let end = V::from(*points.last().unwrap());
                let expected = end + (V::from(geometry::along(&points, 0.4).1) - end).unit() * 0.4;
                assert!((V::from(p) - expected).norm() < 1e-6);
            }
        }
    }
}

#[test]
fn mersenne_twisters_match_standard_reference_vectors() {
    let mut r = random::Mt19937::new(5489);
    assert_eq!(r.next(), 14_514_284_786_278_117_030);
    assert_eq!(r.next(), 4_620_546_740_167_642_908);
    let mut r = random::Mt19937_32::new(5489);
    assert_eq!(r.next(), 3_499_211_612);
    assert_eq!(r.next(), 581_869_302);
}

#[test]
fn overhangs_are_avoided_without_overriding_painting() {
    let poly = square();
    let lower = ExPolygon {
        contour: vec![
            Point::from_mm(0.0, 0.0),
            Point::from_mm(5.0, 0.0),
            Point::from_mm(5.0, 10.0),
            Point::from_mm(0.0, 10.0),
        ],
        holes: Vec::new(),
    };
    let mut b = Boundary::new(&poly, &[lower], 1.0, 0.2, &NoPainting).unwrap();
    let c = choice::nearest(&b, V::new(10.0, 10.0), 1.0, true);
    assert!(c.point.x < 1e-6);
    b.set_paint(2, Paint::Enforcer);
    let c = choice::nearest(&b, V::new(0.0, 0.0), 1.0, true);
    assert_eq!(c.point, V::new(10.0, 10.0));
}

#[test]
fn rear_holes_stay_on_their_own_boundaries() {
    let model = TriangleMesh::load_stl(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/models/hole_block.stl"
    ))
    .unwrap();
    let mut s = PrintSettings::core_one_hf04_speed_pla();
    s.seam.position = SeamPosition::Rear;
    let prepared = prepare_object(&model, &s).unwrap();
    let layers = prepared.toolpaths(1..2);
    let starts: Vec<_> = layers[0]
        .paths
        .iter()
        .filter(|p| p.role == Role::ExternalPerimeter)
        .map(|p| p.points[0])
        .collect();
    assert_eq!(starts.len(), 2);
    assert!(starts[0].distance_to(starts[1]) / SCALE > 5.0);
}

#[test]
fn holes_and_contours_follow_the_requested_print_direction() {
    use crate::settings::PerimeterGenerator;
    let model = TriangleMesh::load_stl(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/models/hole_block.stl"
    ))
    .unwrap();
    for generator in [PerimeterGenerator::Classic, PerimeterGenerator::Arachne] {
        for clockwise in [false, true] {
            let mut s = PrintSettings::core_one_hf04_speed_pla();
            s.perimeter_generator = generator;
            s.seam.prefer_clockwise_movements = clockwise;
            let prepared = prepare_object(&model, &s).unwrap();
            let layers = prepared.toolpaths(1..2);
            let walls: Vec<_> = layers[0]
                .paths
                .iter()
                .filter(|p| p.role == Role::ExternalPerimeter)
                .collect();
            assert_eq!(walls.len(), 2);
            for wall in walls {
                assert_eq!(crate::geometry::signed_area(&wall.points) < 0., clockwise);
            }
        }
    }
}

#[test]
fn upstream_oversample_painted_edge() {
    struct Dot;
    impl SeamPainting for Dot {
        fn classify(&self, p: [f64; 3], radius: f64) -> Paint {
            if ((p[0] - 0.5).powi(2) + p[1].powi(2) + (p[2] - 1.0).powi(2)).sqrt() < radius {
                Paint::Enforcer
            } else {
                Paint::Common
            }
        }
    }
    let poly = vec![
        Point::from_mm(0.0, 0.0),
        Point::from_mm(1.0, 0.0),
        Point::from_mm(1.0, 1.0),
        Point::from_mm(0.0, 1.0),
    ];
    let b = Boundary::new(&poly, &[], 1.0, 0.2, &Dot).unwrap();
    assert_eq!(b.points.len(), 8);
    assert!((b.points[1] - V::new(0.2, 0.0)).norm() < 1e-9);
}

#[test]
fn boundary_offsets_preserve_edge_orientation_and_stop_at_blockers() {
    let mut b = fixture();
    let c = Choice::vertex(&b.points, 0);
    let next = b.offset(c, 0.25, true).unwrap();
    assert_eq!((next.previous, next.next), (0, 1));
    assert_eq!(next.point, V::new(0.25, 0.0));
    let prev = b.offset(c, 0.25, false).unwrap();
    assert_eq!((prev.previous, prev.next), (4, 0));
    assert_eq!(prev.point, V::new(0.0, 0.25));
    b.set_paint(1, Paint::Blocker);
    assert!(b.offset(c, 1.5, true).is_none());
}

#[test]
fn floating_start_after_empty_layers_is_overhanging() {
    let model = box_mesh([0.0, 0.0, 1.0], [10.0, 10.0, 1.4]);
    let mut s = PrintSettings::core_one_hf04_speed_pla();
    s.seam.scarf.placement = ScarfPlacement::Everywhere;
    s.seam.scarf.only_on_smooth = false;
    s.seam.scarf.length = 4.0;
    let prepared = prepare_object(&model, &s).unwrap();
    assert!(prepared.seams.layers[..5].iter().all(Vec::is_empty));
    assert!(prepared.seams.layers[5].iter().all(|r| r.selected.overhang));
    assert!(
        prepared.toolpaths(5..6)[0]
            .paths
            .iter()
            .all(|p| p.seam.as_ref().is_none_or(|m| m.samples.is_empty()))
    );
}

#[test]
fn boundary_offset_includes_the_final_partial_edge() {
    let b = fixture();
    let c = Choice {
        point: V::new(0.4, 0.0),
        previous: 0,
        next: 1,
    };
    let forward = b.offset(c, 3.8, true).unwrap();
    assert!((forward.point - V::new(0.2, 0.0)).norm() < 1e-9);
    let backward = b.offset(c, 3.8, false).unwrap();
    assert!((backward.point - V::new(0.6, 0.0)).norm() < 1e-9);
    assert!(b.offset(c, 4.1, true).is_none());
}

#[test]
fn arachne_walls_start_at_the_aligned_seam() {
    // The CORE One default (Arachne): closed variable-width runs are opened at the planned seam corner.
    let m = box_mesh([0.0; 3], [10.0, 10.0, 1.0]);
    let s = PrintSettings::core_one_hf04_speed_pla();
    assert_eq!(
        s.perimeter_generator,
        crate::settings::PerimeterGenerator::Arachne
    );
    let obj = prepare_object(&m, &s).unwrap();
    for l in obj.toolpaths(0..5) {
        let outer = l
            .paths
            .iter()
            .find(|p| p.role == Role::ExternalPerimeter)
            .unwrap();
        assert!(
            outer.points[0].x > 9_000_000 && outer.points[0].y > 9_000_000,
            "layer {}: starts at {:?}",
            l.index,
            outer.points[0]
        );
    }
}

#[test]
fn orca_conditional_scarf_angle_and_signed_overhang_thresholds() {
    let s = ScarfSettings {
        orca: true,
        conditional: true,
        ..ScarfSettings::default()
    };
    let circle: Vec<_> = (0..128)
        .map(|i| {
            let a = i as f64 * std::f64::consts::TAU / 128.;
            Point::from_mm(10. * a.cos(), 10. * a.sin())
        })
        .collect();
    let lower = ExPolygon {
        contour: circle.clone(),
        holes: vec![],
    };
    let at = V::new(9.8, 0.);
    assert!(scarf::conditional(
        &circle,
        at,
        Some(std::slice::from_ref(&lower)),
        0.4,
        0.4,
        &s
    ));
    assert!(!scarf::conditional(&square(), at, None, 0.4, 0.4, &s));
    assert!(!scarf::conditional(
        &circle,
        V::new(10., 0.),
        Some(&[lower]),
        0.4,
        0.4,
        &s
    ));
    let loose = ScarfSettings {
        overhang_threshold: 0.,
        angle_threshold: 0.,
        ..s
    };
    assert!(scarf::conditional(
        &square(),
        at,
        Some(&[]),
        0.4,
        0.4,
        &loose
    ));
}

#[test]
fn scarf_keeps_variable_width_flow_and_reversed_direction() {
    let mut poly = square();
    poly.reverse();
    let s = ScarfSettings {
        orca: true,
        length: 5.,
        max_segment_length: 0.5,
        ..ScarfSettings::default()
    };
    let (points, mut meta) = scarf::add(&poly, V::new(10., 0.), 5., &s);
    meta.scarf_speed = Some(30.);
    let mut runs = Vec::new();
    for i in 0..poly.len() {
        runs.push(ExtrusionPath {
            points: vec![poly[i], poly[(i + 1) % poly.len()]],
            flow: Flow::new(0.4 + i as f64 * 0.1, 0.2),
            role: Role::ExternalPerimeter,
            perimeter_index: Some(0),
            overhang: None,
            seam: None,
            brick: None,
        });
    }
    let path = ExtrusionPath {
        points,
        seam: Some(Box::new(meta)),
        ..runs[0].clone()
    };
    let split = split_variable(&path, &runs);
    assert!(split.len() > 4);
    let rebuilt: Vec<_> = split
        .iter()
        .flat_map(|p| p.points[..p.points.len() - 1].iter().copied())
        .chain(split.last().unwrap().points.last().copied())
        .collect();
    assert_eq!(rebuilt, path.points);
    assert!(
        split
            .iter()
            .all(|p| p.seam.as_ref().unwrap().samples.len() == p.points.len())
    );
    assert!(split.iter().any(|p| (p.flow.width - 0.7).abs() < 1e-9));
    assert!(
        split
            .iter()
            .all(|p| p.seam.as_ref().unwrap().scarf_speed == Some(30.))
    );
}

fn fnv1a(bytes: impl IntoIterator<Item = u8>) -> u64 {
    bytes.into_iter().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}
fn model(name: &str) -> TriangleMesh {
    TriangleMesh::load_stl(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/models")
            .join(name),
    )
    .unwrap()
}
/// Reference values from a C++ harness compiling PrusaSlicer 2.9.6's ShortEdgeCollapse.cpp,
/// NormalUtils.cpp and admesh with Eigen 3.3.7 (Apple clang, arm64): the decimated 3DBenchy.
#[test]
fn decimation_is_bit_identical_to_prusaslicer() {
    let mut its = shared_vertices::admesh_shared_vertices(&model("3DBenchy.stl"));
    assert_eq!((its.vertices.len(), its.faces.len()), (112_543, 225_102));
    decimate::short_edge_collapse(&mut its, 16_000);
    assert_eq!((its.vertices.len(), its.faces.len()), (7_980, 15_976));
    let vertices = fnv1a(its.vertices.iter().flatten().flat_map(|x| x.to_le_bytes()));
    let faces = fnv1a(
        its.faces
            .iter()
            .flatten()
            .flat_map(|&i| (i as i32).to_le_bytes()),
    );
    assert_eq!(vertices, 0x4cce_8d52_d73e_3018);
    assert_eq!(faces, 0x17f8_b0b0_8581_10fd);
}
/// The same harness's 30,000 samples, normals and ray-cast visibilities of hole_block, which
/// must not depend on where the object is placed.
#[test]
fn model_visibility_is_bit_identical_to_prusaslicer() {
    let loaded = model("hole_block.stl");
    for (x, y) in [(125.0, 110.0), (180.0, 180.0)] {
        let mut placed = loaded.clone();
        placed.place_on_bed(x, y);
        let vis = model_visibility::ModelVisibility::new(
            &placed,
            Some(&loaded),
            &crate::progress::Silent,
        );
        let (positions, normals, visibility, radius) = vis.samples();
        let bytes = |v: &[[f32; 3]]| fnv1a(v.iter().flatten().flat_map(|x| x.to_le_bytes()));
        assert_eq!(bytes(positions), 0xc3c4_548f_71da_6581);
        assert_eq!(bytes(normals), 0xb81f_8580_6c32_d42f);
        assert_eq!(
            fnv1a(visibility.iter().flat_map(|x| x.to_le_bytes())),
            0x4893_7f75_0b87_b2e3
        );
        assert_eq!(radius.to_bits(), 0x3f56_fdc2);
        // The hole's wall is far less visible than the top face.
        let (x, y) = (f64::from(x), f64::from(y));
        let hole = vis.at([x + 4.0, y, 5.0]);
        let top = vis.at([x, y + 8.0, 10.0]);
        assert!(hole < top, "{hole} {top}");
    }
    // Without the loaded mesh the placed one is the model frame; it still works.
    let mut placed = loaded.clone();
    placed.place_on_bed(125.0, 110.0);
    let vis = model_visibility::ModelVisibility::new(&placed, None, &crate::progress::Silent);
    assert_eq!(vis.samples().0.len(), 30_000);
}
