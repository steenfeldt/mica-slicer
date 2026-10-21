// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/Support/OrganicSupport.cpp :: organic_draw_branches, extrude_branch
// Source: prusaslicer@b028299c77 src/libslic3r/MutablePolygon.cpp :: smooth_outward
// Source: orca@8500fcdcca src/libslic3r/Support/TreeSupport3D.cpp :: organic_draw_branches, extrude_branch
// Changes: Analytical cap, smoothing, deterministic end-to-end layer and cancellation fixtures for both rules.
// Copyright (c) Prusa Research 2023 Vojtěch Bubník

use super::{branch_mesh::extrude_branch, geometry::area, smoothing::smooth_outward, *};
use crate::{
    config::Config,
    geometry::{ExPolygon, Point, clip},
    rules::Rules,
    settings::PrintSettings,
    support::{self, SupportConfig, SupportInput, SupportLayerKind as Kind, SupportObjectLayer},
};
fn rect(x: f64, y: f64, w: f64, h: f64) -> Areas {
    vec![ExPolygon {
        contour: [(x, y), (x + w, y), (x + w, y + h), (x, y + h)]
            .map(|(x, y)| Point::from_mm(x, y))
            .to_vec(),
        holes: vec![],
    }]
}
fn config(orca: bool) -> Config {
    let mut cfg = Config::defaults();
    for (k, v) in [
        ("support_material", "1"),
        ("support_material_style", "organic"),
        ("support_material_contact_distance", "0.2"),
        ("support_material_interface_layers", "3"),
        ("support_material_bottom_interface_layers", "2"),
        ("layer_height", "0.2"),
        ("first_layer_height", "0.2"),
        ("dont_support_bridges", "0"),
    ] {
        cfg.set_raw(k, v).unwrap();
    }
    if orca {
        cfg.set_raw("mica_slicing_rules", "orcaslicer").unwrap();
    }
    cfg
}
#[test]
fn organic_settings_always_resolve_tree3d() {
    let cfg = Config::defaults();
    assert!(PrintSettings::from_config(&cfg).unwrap().tree3d.is_some());
}
#[test]
fn synthetic_overhang_draws_base_interfaces_and_bed_deterministically() {
    for orca in [false, true] {
        let cfg = config(orca);
        let settings = PrintSettings::from_config(&cfg).unwrap();
        let config = SupportConfig::from(&settings);
        let outlines: Vec<_> = (0..40)
            .map(|l| {
                if l < 30 {
                    rect(10., 10., 4., 6.)
                } else {
                    rect(3., 10., 18., 6.)
                }
            })
            .collect();
        let layers: Vec<_> = outlines
            .iter()
            .enumerate()
            .map(|(i, a)| SupportObjectLayer {
                print_z: 0.2 * (i + 1) as f64,
                height: 0.2,
                outlines: a,
                overhangs: None,
                bridges: None,
            })
            .collect();
        let input = SupportInput {
            layers: &layers,
            config: &config,
            masks: None,
            origin: Point::default(),
        };
        let a = support::generate(&input).unwrap();
        let b = support::generate(&input).unwrap();
        assert!(!a.layers.is_empty());
        assert!((a.layers[0].print_z - 0.2).abs() < 1e-6);
        assert!(
            a.layers[0]
                .regions
                .iter()
                .any(|r| r.kind == Kind::Base && !r.polygons.is_empty())
        );
        assert!(a.layers.iter().any(|l| {
            l.regions
                .iter()
                .any(|r| r.kind == Kind::TopContact && !r.polygons.is_empty())
        }));
        assert!(a.layers.iter().any(|l| {
            l.regions
                .iter()
                .any(|r| r.kind == Kind::TopInterface && !r.polygons.is_empty())
        }));
        assert_eq!(a.layers.len(), b.layers.len());
        let mut paths = 0;
        for (a, b) in a.layers.iter().zip(&b.layers) {
            assert_eq!(a.print_z, b.print_z);
            assert_eq!(a.height, b.height);
            assert_eq!(a.regions.len(), b.regions.len());
            for (a, b) in a.regions.iter().zip(&b.regions) {
                assert_eq!(a.kind, b.kind);
                assert_eq!(a.polygons, b.polygons);
            }
        }
        for i in 0..a.layers.len() {
            paths += a.toolpaths(i).paths.len();
        }
        assert!(paths > 0);
    }
}
#[test]
fn mesh_is_closed_and_orca_bed_foot_is_flat() {
    for rules in [Rules::PrusaSlicer, Rules::OrcaSlicer] {
        let m = TreeSupportMeshGroupSettings {
            rules,
            layer_height: scaled(0.2),
            ..Default::default()
        };
        let p = TreeSlicingParameters::new(0.2, 0.2);
        let c = TreeSupportSettings::new(&m, &p, false);
        let bounds: Vec<SupportElements> = (0..4)
            .map(|l| {
                [SupportElement {
                    state: SupportElementState {
                        layer_idx: l,
                        result_on_layer: Some(Point::from_mm(0., 0.)),
                        effective_radius_height: 10,
                        distance_to_top: 10,
                        ..Default::default()
                    },
                    ..Default::default()
                }]
                .into_iter()
                .collect()
            })
            .collect();
        let path: Vec<_> = (0..4).map(|l| (l, 0)).collect();
        let (mesh, (lo, hi)) = extrude_branch(&bounds, &path, true, &c, &p);
        assert!(mesh.volume() > 0.);
        assert!(hi > 0.8);
        if rules.is_orca() {
            assert_eq!(lo, 0.);
            assert!(mesh.vertices.iter().all(|p| p[2] >= 0.));
        } else {
            assert!(lo < 0.);
        }
        let mut edges = std::collections::BTreeMap::new();
        for face in &mesh.faces {
            for i in 0..3 {
                let (a, b) = (face[i], face[(i + 1) % 3]);
                *edges.entry((a.min(b), a.max(b))).or_insert(0) += 1;
            }
        }
        assert!(edges.values().all(|&count| count == 2));
    }
}
#[test]
fn outward_smoothing_fills_narrow_concavity_and_preserves_convex_outline() {
    let square = rect(0., 0., 4., 4.);
    assert_eq!(smooth_outward(square.clone(), scaled(0.4)), square);
    let notch = vec![ExPolygon {
        contour: [
            (0., 0.),
            (4., 0.),
            (4., 4.),
            (2.1, 4.),
            (2.1, 2.),
            (1.9, 2.),
            (1.9, 4.),
            (0., 4.),
        ]
        .map(|(x, y)| Point::from_mm(x, y))
        .to_vec(),
        holes: vec![],
    }];
    let smooth = smooth_outward(notch.clone(), scaled(0.4));
    assert!(
        area(&smooth) > area(&notch),
        "before={} after={} result={smooth:?}",
        area(&notch),
        area(&smooth)
    );
    assert!(area(&clip::diff_ex(&notch, &smooth)) < 1.);
    assert_eq!(smooth_outward(notch, scaled(0.4)), smooth);
}
#[test]
fn drawing_cancellation_is_fallible() {
    let m = TreeSupportMeshGroupSettings::default();
    let p = TreeSlicingParameters::new(0.15, 0.2);
    let c = TreeSupportSettings::new(&m, &p, false);
    let mut v = TreeModelVolumes::new(c.clone(), vec![vec![]; 2], vec![], vec![]);
    let pathing = TreePathing {
        move_bounds: vec![SupportElements::new(); 2],
        roofs: TreeRoofs::new(2),
    };
    assert_eq!(
        organic_draw_branches(
            &mut v,
            &c,
            &p,
            &TreeSupportInfill::new(&c),
            pathing,
            || Err("cancelled")
        ),
        Err("cancelled")
    );
}

#[test]
fn interface_slabs_keep_precomputed_roofs_outside_branch_base_and_bottom_kind() {
    let cfg = config(false);
    let s = PrintSettings::from_config(&cfg).unwrap();
    let resolved = s.tree3d.unwrap();
    let p = TreeSlicingParameters::new(0.2, 0.2);
    let c = TreeSupportSettings::new(&resolved.mesh, &p, false);
    let infill = TreeSupportInfill::new(&c);
    let mut draw = TreeDrawing {
        base: vec![rect(0., 0., 2., 2.); 6],
        bottom_contacts: vec![vec![]; 6],
        roofs: TreeRoofs::new(6),
    };
    draw.bottom_contacts[0] = rect(0., 0., 2., 2.);
    draw.base[0].clear();
    draw.roofs.top_interfaces[2] = rect(5., 0., 2., 2.);
    draw.roofs.top_contacts[5] = rect(0., 0., 2., 2.);
    draw.base[5].clear();
    let layers =
        super::pipeline::finalize_interface_and_support_areas(draw, &c, &p, &infill, &resolved);
    assert!(
        layers[0]
            .regions
            .iter()
            .any(|r| r.kind == Kind::BottomContact)
    );
    assert!(
        layers[1]
            .regions
            .iter()
            .any(|r| r.kind == Kind::BottomInterface)
    );
    assert!(
        layers[2]
            .regions
            .iter()
            .any(|r| r.kind == Kind::TopInterface
                && !clip::intersection_ex(&r.polygons, &rect(5., 0., 2., 2.)).is_empty())
    );
    assert!(layers[5].regions.iter().any(|r| r.kind == Kind::TopContact));
}

/// TriangleMeshSlicer Positive mode makes every sliced loop CCW before the loops are
/// used (OrganicSupport.cpp then diffs them with NonZero), so an inverted shell, such
/// as a folded-over branch tube, still counts as solid.
#[test]
fn branch_slicing_positive_mode_keeps_inverted_shells_solid() {
    use crate::slice::{SliceRounding, slice_branch_mesh};
    let mut mesh = crate::mesh::tests::box_mesh([0., 0., 0.], [2., 2., 2.]);
    let inverted = crate::mesh::tests::box_mesh([4., 0., 0.], [6., 2., 2.]);
    let offset = mesh.vertices.len() as u32;
    mesh.vertices.extend(inverted.vertices);
    mesh.faces.extend(
        inverted
            .faces
            .into_iter()
            .map(|[a, b, c]| [a + offset, c + offset, b + offset]),
    );
    for rules in [SliceRounding::Prusa, SliceRounding::Nearest] {
        let slices = slice_branch_mesh(&mesh, &[1.], rules);
        assert_eq!(slices[0].islands.len(), 2);
        for island in &slices[0].islands {
            assert!((island.area_mm2() - 4.).abs() < 1e-4);
            assert!(island.holes.is_empty());
        }
    }
}
