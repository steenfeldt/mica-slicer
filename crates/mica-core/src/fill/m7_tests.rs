// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 tests/fff_print/test_fill.cpp :: Fill: Pattern Path Length, Infill does not exceed perimeters
// Changes: Rust API fixtures; test all new patterns, object-cache batch invariance and deterministic generation.
use super::*;
use crate::geometry::clip::{diff_pl, offset_ex};
use crate::geometry::{Polygon, polyline_length};
fn rectangle(a: f64, b: f64, c: f64, d: f64) -> Polygon {
    [(a, b), (c, b), (c, d), (a, d)]
        .into_iter()
        .map(|(x, y)| Point::from_mm(x, y))
        .collect()
}
fn params() -> FillParams {
    FillParams {
        spacing: 0.40708,
        density: 0.2,
        angle: 45.,
        layer_id: 12,
        z: 2.6,
        anchor_length: 2.,
        anchor_length_max: 12.,
        dont_adjust: false,
        link_max_length: 0.,
        origin: Point::default(),
    }
}
#[test]
fn patterns_never_cross_holes_or_outer_boundary() {
    let mut hole = rectangle(7., 4., 17., 14.);
    hole.reverse();
    let area = vec![ExPolygon {
        contour: rectangle(0., 0., 30., 20.),
        holes: vec![hole],
    }];
    let limit = offset_ex(&area, 1000., Join::Miter);
    for pattern in [
        Pattern::Concentric,
        Pattern::Honeycomb,
        Pattern::Honeycomb3D,
        Pattern::HilbertCurve,
        Pattern::ArchimedeanChords,
        Pattern::OctagramSpiral,
        Pattern::CrossHatch,
    ] {
        for density in [0.2, 1.] {
            let p = FillParams {
                density,
                ..params()
            };
            let result = fill(pattern, &area, &p);
            assert_eq!(result, fill(pattern, &area, &p));
            let lines: Vec<_> = result.into_iter().flat_map(|f| f.polylines).collect();
            assert!(!lines.is_empty(), "{pattern:?}");
            assert!(
                diff_pl(&lines, &limit)
                    .iter()
                    .map(|p| polyline_length(p))
                    .sum::<f64>()
                    < 1.,
                "{pattern:?}"
            );
        }
    }
}
#[test]
fn upstream_square_hole_and_missing_segments() {
    let mut p = FillParams {
        spacing: 5.,
        density: 0.1,
        angle: -90.,
        anchor_length: 1000.,
        anchor_length_max: 1000.,
        dont_adjust: true,
        ..params()
    };
    let square = rectangle(0., 0., 100., 100.);
    for start in 0..4 {
        let mut contour = square.clone();
        contour.rotate_left(start);
        let filled = fill(
            Pattern::Rectilinear,
            &[ExPolygon {
                contour,
                holes: vec![],
            }],
            &p,
        );
        assert_eq!(filled[0].polylines.len(), 1);
    }
    let mut hole = rectangle(25., 25., 75., 75.);
    hole.reverse();
    let area = vec![ExPolygon {
        contour: square,
        holes: vec![hole],
    }];
    for angle in [-90., -45., -180., 90., 180.] {
        for spacing in [25., 5., 7.5, 8.5] {
            p.angle = angle;
            p.density = 5. / spacing;
            let lines: Vec<_> = fill(Pattern::Rectilinear, &area, &p)
                .into_iter()
                .flat_map(|f| f.polylines)
                .collect();
            assert!((1..=3).contains(&lines.len()));
            assert!(diff_pl(&lines, &offset_ex(&area, 1000., Join::Miter)).is_empty());
        }
    }
    let contour = [
        (25771516, 14142125),
        (14142138, 25771515),
        (2512749, 14142131),
        (14142125, 2512749),
    ]
    .into_iter()
    .map(|(x, y)| Point::new(x, y))
    .collect();
    p.spacing = 0.654498;
    p.density = 1.;
    p.angle = 45.;
    p.dont_adjust = false;
    p.layer_id = 66;
    p.z = 20.15;
    let result = fill(
        Pattern::Rectilinear,
        &[ExPolygon {
            contour,
            holes: vec![],
        }],
        &p,
    );
    assert_eq!(result[0].polylines.len(), 1);
}
#[test]
fn object_patterns_are_compact_and_batch_invariant() {
    use crate::print::{LayerSource, prepare_object};
    let mut mesh = crate::mesh::TriangleMesh::load_stl(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/models/cube20.stl"
    ))
    .unwrap();
    mesh.place_on_bed(125., 110.);
    for pattern in [
        Pattern::AdaptiveCubic,
        Pattern::SupportCubic,
        Pattern::Lightning,
    ] {
        let mut s = crate::settings::PrintSettings::core_one_hf04_speed_pla();
        s.fill_pattern = pattern;
        s.fill_density = 0.2;
        let obj = prepare_object(&mesh, &s).unwrap();
        let all = obj.toolpaths(0..obj.layer_count());
        let parts: Vec<_> = (0..obj.layer_count())
            .step_by(13)
            .flat_map(|i| obj.toolpaths(i..(i + 13).min(obj.layer_count())))
            .collect();
        assert!(
            all.iter().any(|l| l
                .paths
                .iter()
                .any(|p| p.role == crate::toolpath::Role::InternalInfill)),
            "{pattern:?}"
        );
        for (a, b) in all.iter().zip(parts) {
            assert_eq!(a.paths, b.paths, "{pattern:?} layer {}", a.index);
        }
    }
    let tree = adaptive::Octree::build(&mesh, Point::from_mm(125., 110.), 6.75, false);
    assert!(tree.node_count() > 1 && tree.node_count() < 1000);
}
