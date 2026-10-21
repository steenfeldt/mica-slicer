// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 tests/fff_print/test_support_material.cpp :: SupportMaterial: Three raft layers created, SupportMaterial: support_layers_z and contact_distance
// Changes: Outline fixtures, plus normal/tree/organic regressions for gaps, merging, model landings,
// Changes: buildplate-only routing, narrow tips, double walls, bridge masks and lazy Z merging.

use super::*;
use crate::geometry::clip::{intersection_ex, offset_polylines};
use crate::geometry::{ExPolygon, Point};
use crate::settings::PrintSettings;
use crate::toolpath::Role;

fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> ExPolygon {
    ExPolygon {
        contour: vec![
            Point::from_mm(x0, y0),
            Point::from_mm(x1, y0),
            Point::from_mm(x1, y1),
            Point::from_mm(x0, y1),
        ],
        holes: Vec::new(),
    }
}
fn config() -> SupportConfig {
    let mut c = SupportConfig::from(&PrintSettings::core_one_hf04_speed_pla());
    // Organic trees take their settings from the resolved tree3d config.
    c.tree3d = PrintSettings::from_config(&crate::config::Config::defaults())
        .unwrap()
        .tree3d;
    c.enabled = true;
    c.automatic = true;
    c.style = SupportStyle::Snug;
    c.first_layer_expansion = 0.0;
    c
}
fn run(c: &SupportConfig, areas: &[Vec<ExPolygon>]) -> SupportPlan {
    let p = SupportParameters::new(c).unwrap();
    let layers: Vec<_> = areas
        .iter()
        .enumerate()
        .map(|(i, area)| SupportObjectLayer {
            print_z: p.object_z_offset + c.first_layer_height + i as f64 * c.layer_height,
            height: if i == 0 {
                c.first_layer_height
            } else {
                c.layer_height
            },
            outlines: area,
            overhangs: None,
            bridges: None,
        })
        .collect();
    generate(&SupportInput {
        masks: None,
        layers: &layers,
        config: c,
        origin: Point::default(),
    })
    .unwrap()
}
fn tee() -> Vec<Vec<ExPolygon>> {
    (0..60)
        .map(|i| {
            vec![if i < 50 {
                rect(10.0, 0.0, 20.0, 8.0)
            } else {
                rect(0.0, 0.0, 30.0, 8.0)
            }]
        })
        .collect()
}
#[test]
fn upright_cube_needs_no_supports() {
    assert!(
        run(&config(), &vec![vec![rect(0.0, 0.0, 20.0, 20.0)]; 100])
            .layers
            .is_empty()
    );
}
#[test]
fn disabled_and_manual_only_are_empty() {
    let mut c = config();
    c.enabled = false;
    assert!(run(&c, &tee()).layers.is_empty());
    c.enabled = true;
    c.automatic = false;
    assert!(run(&c, &tee()).layers.is_empty());
}
#[test]
fn support_heights_and_exact_contact_gap() {
    for first in [0.2, 0.3, 0.4] {
        for gap in [0.0, 0.15, 0.2, 0.27] {
            let mut c = config();
            c.first_layer_height = first;
            c.top_gap = gap;
            let plan = run(&c, &tee());
            assert!((plan.layers[0].print_z - first).abs() < EPS);
            let contact = first + 49.0 * c.layer_height - gap;
            assert!((plan.layers.last().unwrap().print_z - contact).abs() < EPS);
            for l in &plan.layers[1..] {
                assert!(
                    l.height >= c.min_layer_height - EPS && l.height <= c.max_layer_height + EPS,
                    "{l:?}"
                );
            }
            assert!(plan.layers.windows(2).all(|w| w[0].print_z < w[1].print_z));
        }
    }
}
#[test]
fn top_interface_count_and_roles() {
    for style in [SupportStyle::Snug, SupportStyle::Grid, SupportStyle::Tree] {
        for count in [0, 1, 3] {
            let mut c = config();
            c.style = style;
            c.top_interface_layers = count;
            let plan = run(&c, &tee());
            let dense = (0..plan.layers.len())
                .filter(|&i| {
                    plan.toolpaths(i)
                        .paths
                        .iter()
                        .any(|p| p.role == Role::SupportMaterialInterface)
                })
                .count();
            assert_eq!(dense, count, "{style:?} {count}");
            // Orca's interface count excludes the contact layer. Normal supports get one more
            // dense layer; under a single tree roof the tip layer is converted as well (Orca
            // 2.4.1 prints 3 dense layers under this T with normal(auto) and 2 interface layers,
            // and 2 with tree(auto) and 1 interface layer).
            let mut orca = c.clone();
            orca.orca_semantics = true;
            let orca_plan = run(&orca, &tee());
            let orca_dense = (0..orca_plan.layers.len())
                .filter(|&i| {
                    orca_plan
                        .toolpaths(i)
                        .paths
                        .iter()
                        .any(|p| p.role == Role::SupportMaterialInterface)
                })
                .count();
            let expected = match (style.is_tree(), count) {
                (_, 0) => 0,
                (false, n) => n + 1,
                (true, 1) => 2,
                (true, n) => n,
            };
            assert_eq!(orca_dense, expected, "orca {style:?} {count}");
            assert!(
                plan.toolpaths(0)
                    .paths
                    .iter()
                    .any(|p| p.role == Role::SupportMaterial)
            );
        }
    }
}
/// `generate_interface_layers`: with base interface layers (a soluble interface over another
/// filament), the dense layers furthest from the contact print as base (the base tool) in the
/// interface pattern; the contact stays interface.
#[test]
fn base_interface_layers_take_the_lowest_dense_layers() {
    for style in [SupportStyle::Snug, SupportStyle::Grid] {
        let mut c = config();
        c.style = style;
        c.top_gap = 0.0;
        c.top_interface_layers = 3;
        let dense = |c: &SupportConfig| {
            let plan = run(c, &tee());
            let count = |role| {
                (0..plan.layers.len())
                    .filter(|&i| {
                        let paths = plan.toolpaths(i).paths;
                        paths.iter().any(|p| p.role == role)
                            && (role == Role::SupportMaterialInterface
                                || plan.layers[i]
                                    .regions
                                    .iter()
                                    .any(|r| r.kind == SupportLayerKind::BaseInterface))
                    })
                    .count()
            };
            (
                count(Role::SupportMaterialInterface),
                count(Role::SupportMaterial),
            )
        };
        assert_eq!(dense(&c), (3, 0), "{style:?}");
        c.top_base_interface_layers = 1;
        assert_eq!(dense(&c), (2, 1), "{style:?}");
        // Orca counts one more interface layer; one base interface layer below it.
        c.orca_semantics = true;
        assert_eq!(dense(&c), (3, 1), "{style:?}");
    }
}

#[test]
fn support_extrusions_do_not_cross_object() {
    let mut c = config();
    for style in [
        SupportStyle::Snug,
        SupportStyle::Grid,
        SupportStyle::Tree,
        SupportStyle::Organic,
    ] {
        c.style = style;
        let plan = run(&c, &tee());
        for i in 0..plan.layers.len() {
            for path in plan.toolpaths(i).paths {
                let swept = offset_polylines(
                    &[path.points],
                    path.flow.width * crate::geometry::SCALE / 2.0,
                );
                assert!(intersection_ex(&swept, &[rect(10.0, 0.0, 20.0, 8.0)]).is_empty());
            }
        }
    }
}
#[test]
fn three_raft_layers_created_with_supports_disabled() {
    let mut c = config();
    c.enabled = false;
    c.raft_layers = 3;
    let plan = run(&c, &vec![vec![rect(0.0, 0.0, 20.0, 20.0)]; 100]);
    assert_eq!(plan.layers.len(), 3);
    assert!((plan.parameters.object_z_offset - 0.95).abs() < EPS);
    for i in 0..3 {
        assert!(!plan.toolpaths(i).paths.is_empty());
    }
}
#[test]
fn buildplate_only_excludes_columns_on_a_shelf() {
    let areas: Vec<_> = (0..60)
        .map(|i| {
            vec![if !(10..50).contains(&i) {
                rect(0.0, 0.0, 30.0, 8.0)
            } else {
                rect(10.0, 0.0, 20.0, 8.0)
            }]
        })
        .collect();
    let mut c = config();
    assert!(!run(&c, &areas).layers.is_empty());
    c.buildplate_only = true;
    assert!(run(&c, &areas).layers.is_empty());
}
#[test]
fn bridge_mask_is_optional_and_honoured() {
    let areas = tee();
    let mut c = config();
    c.dont_support_bridges = true;
    let mask = vec![rect(0.0, 0.0, 30.0, 8.0)];
    let layers: Vec<_> = areas
        .iter()
        .enumerate()
        .map(|(i, a)| SupportObjectLayer {
            print_z: (i + 1) as f64 * 0.2,
            height: 0.2,
            outlines: a,
            overhangs: None,
            bridges: Some(super::Bridges::Ready(&mask)),
        })
        .collect();
    assert!(
        generate(&SupportInput {
            masks: None,
            layers: &layers,
            config: &c,
            origin: Point::default()
        })
        .unwrap()
        .layers
        .is_empty()
    );
    assert!(!run(&c, &areas).layers.is_empty());
}
#[test]
fn independent_z_merge_and_support_first() {
    let plan = run(&config(), &tee());
    let tops: Vec<_> = (1..=60).map(|i| i as f64 * 0.2).collect();
    let merged = schedule::merge(&tops, &[0.2; 60], &plan);
    assert!(merged.len() > tops.len());
    assert!(merged.windows(2).all(|w| w[0].print_z < w[1].print_z));
    assert_eq!(merged.iter().filter(|l| l.object.is_some()).count(), 60);
    assert_eq!(
        merged.iter().filter(|l| l.support.is_some()).count(),
        plan.layers.len()
    );
    let fake = crate::toolpath::ExtrusionPath {
        overhang: None,
        perimeter_index: None,
        seam: None,
        role: Role::Perimeter,
        points: vec![Point::default(), Point::from_mm(1.0, 0.0)],
        flow: crate::flow::Flow::new(0.4, 0.2),
        brick: None,
    };
    let layer = schedule::toolpaths(0, merged[0], &plan, |_| crate::toolpath::LayerToolpaths {
        paths: vec![fake],
        ..Default::default()
    });
    assert_eq!(layer.paths.first().unwrap().role, Role::SupportMaterial);
    assert_eq!(layer.paths.last().unwrap().role, Role::Perimeter);
}
#[test]
fn unsupported_modes_and_bad_heights_are_errors() {
    let mut c = config();
    c.style = SupportStyle::Snug;
    c.pattern = SupportPattern::Honeycomb;
    assert!(SupportParameters::new(&c).is_err());
    c.pattern = SupportPattern::Rectilinear;
    c.max_layer_height = 0.01;
    assert!(SupportParameters::new(&c).is_err());
}

#[test]
fn bottom_contacts_preserve_the_air_gap_and_interface_count() {
    let areas: Vec<_> = (0..60)
        .map(|i| {
            vec![if !(10..50).contains(&i) {
                rect(0.0, 0.0, 30.0, 8.0)
            } else {
                rect(10.0, 0.0, 20.0, 8.0)
            }]
        })
        .collect();
    for gap in [0.15, 0.2, 0.35] {
        let mut c = config();
        c.bottom_gap = gap;
        c.bottom_interface_layers = 2;
        let plan = run(&c, &areas);
        let first = &plan.layers[0];
        assert!((first.print_z - (2.0 + gap + 0.2)).abs() < EPS, "{first:?}");
        assert!((first.height - 0.2).abs() < EPS);
        assert!(
            first
                .regions
                .iter()
                .any(|r| r.kind == SupportLayerKind::BottomContact)
        );
        assert!(
            plan.layers[1]
                .regions
                .iter()
                .any(|r| r.kind == SupportLayerKind::BottomInterface)
        );
    }
}

#[test]
fn raft_pipeline_lifts_object_and_remains_batch_invariant() {
    use crate::print::{LayerSource, try_prepare_object};
    let mesh = crate::mesh::TriangleMesh::load_stl(std::path::Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/models/cube20.stl"
    )))
    .unwrap();
    let mut s = PrintSettings::core_one_hf04_speed_pla();
    s.raft_layers = 3;
    let prepared = try_prepare_object(&mesh, &s).unwrap();
    assert_eq!(prepared.layer_count(), 103);
    let layers = prepared.toolpaths(0..5);
    assert!((layers[3].print_z - 1.15).abs() < EPS);
    assert!(layers[..3].iter().all(|l| l.paths.iter().all(|p| matches!(
        p.role,
        Role::SupportMaterial | Role::SupportMaterialInterface
    ))));
    assert!(
        layers[3]
            .paths
            .iter()
            .any(|p| p.role == Role::ExternalPerimeter)
    );
    for (i, layer) in layers.iter().enumerate() {
        assert_eq!(layer.paths, prepared.toolpaths(i..i + 1)[0].paths);
    }
}

#[test]
fn trees_are_deterministic_and_merge_towards_the_bed() {
    for style in [SupportStyle::Tree, SupportStyle::Organic] {
        let mut c = config();
        c.style = style;
        let single = rayon::ThreadPoolBuilder::new()
            .num_threads(1)
            .build()
            .unwrap();
        let parallel = rayon::ThreadPoolBuilder::new()
            .num_threads(4)
            .build()
            .unwrap();
        let a = single.install(|| run(&c, &tee()));
        let b = parallel.install(|| run(&c, &tee()));
        assert_eq!(a.layers.len(), b.layers.len());
        for (x, y) in a.layers.iter().zip(&b.layers) {
            assert_eq!(x.print_z, y.print_z);
            assert_eq!(x.regions.len(), y.regions.len());
            for (x, y) in x.regions.iter().zip(&y.regions) {
                assert_eq!(x.polygons, y.polygons);
            }
        }
        let islands = |layer: &SupportLayer| {
            layer
                .regions
                .iter()
                .map(|r| r.polygons.len())
                .sum::<usize>()
        };
        assert!(islands(&a.layers[2]) < islands(&a.layers[a.layers.len() - 5]));
        assert!(
            a.layers
                .iter()
                .all(|l| l.height > 0.0 && l.height <= c.nozzle_diameter + EPS)
        );
        assert!((a.layers.last().unwrap().print_z - 9.8).abs() < EPS);
    }
}

#[test]
fn tree_settings_reject_nonfinite_and_singular_values() {
    let mut c = config();
    c.style = SupportStyle::Organic;
    for angle in [90.0, -1.0, f64::NAN, f64::INFINITY] {
        c.tree.angle = angle;
        assert!(SupportParameters::new(&c).is_err());
    }
    c.tree.angle = 40.0;
    c.tree.top_rate = 0.0;
    assert!(SupportParameters::new(&c).is_err());
    c.tree.top_rate = 0.3;
    c.tree.tip_diameter = -0.1;
    assert!(SupportParameters::new(&c).is_err());
}

#[test]
fn organic_double_wall_threshold_changes_shells() {
    let mut c = config();
    c.style = SupportStyle::Organic;
    c.tree.branch_diameter_double_wall = 0.0;
    let one = run(&c, &tee());
    c.tree.branch_diameter_double_wall = 1.0;
    let two = run(&c, &tee());
    let length = |p: &SupportPlan| {
        p.toolpaths(2)
            .paths
            .iter()
            .map(|p| crate::geometry::polyline_length(&p.points))
            .sum::<f64>()
    };
    assert!(length(&two) > 1.4 * length(&one));
}

#[test]
fn tree_raft_and_disabled_modes() {
    for style in [SupportStyle::Tree, SupportStyle::Organic] {
        let mut c = config();
        c.style = style;
        c.enabled = false;
        assert!(run(&c, &tee()).layers.is_empty());
        c.raft_layers = 3;
        let raft = run(&c, &tee());
        assert_eq!(raft.layers.len(), 3);
        c.enabled = true;
        let supported = run(&c, &tee());
        assert!(supported.layers.len() > 3);
        assert!(
            supported
                .layers
                .windows(2)
                .all(|w| w[0].print_z < w[1].print_z)
        );
    }
}

#[test]
fn trees_land_on_model_and_buildplate_only_rejects_enclosed_roofs() {
    let areas: Vec<_> = (0..60)
        .map(|i| {
            vec![if i < 10 {
                rect(-50.0, -50.0, 50.0, 50.0)
            } else if i < 50 {
                rect(-2.0, -2.0, 2.0, 2.0)
            } else {
                rect(-10.0, -5.0, 10.0, 5.0)
            }]
        })
        .collect();
    // Organic trees (tree3d) read these options from their own resolved settings, not from
    // the SupportConfig fields this test varies; tree3d has its own tests.
    {
        let style = SupportStyle::Tree;
        let mut c = config();
        c.style = style;
        c.bottom_interface_layers = 2;
        let plan = run(&c, &areas);
        let stats = plan.tree_statistics.unwrap();
        assert!(stats.model_trunks > 0);
        assert_eq!(stats.supported_tips, stats.tips);
        assert!((plan.layers.first().unwrap().print_z - 2.4).abs() < EPS);
        assert!(plan.layers.iter().any(|l| {
            l.regions
                .iter()
                .any(|r| r.kind == SupportLayerKind::BottomContact)
        }));
        c.buildplate_only = true;
        assert!(run(&c, &areas).layers.is_empty());
    }
}

#[test]
fn trees_route_around_a_shelf_instead_of_using_vertical_coverage() {
    let areas: Vec<_> = (0..60)
        .map(|i| {
            let mut a = vec![rect(15.0, 0.0, 18.0, 6.0)];
            if i < 10 {
                a.push(rect(0.0, 0.0, 10.0, 6.0));
            }
            if i >= 50 {
                a = vec![rect(8.0, 0.0, 18.0, 6.0)];
            }
            a
        })
        .collect();
    let mut c = config();
    c.style = SupportStyle::Organic;
    c.buildplate_only = true;
    let plan = run(&c, &areas);
    let stats = plan.tree_statistics.unwrap();
    assert_eq!(stats.supported_tips, stats.tips);
    assert!(stats.bed_trunks > 0);
    assert_eq!(stats.model_trunks, 0);
    let contact = plan
        .layers
        .last()
        .unwrap()
        .regions
        .iter()
        .flat_map(|r| r.polygons.clone())
        .collect::<Vec<_>>();
    assert!(super::tree_common::contains(
        &contact,
        Point::from_mm(9.0, 3.0)
    ));
}

#[test]
fn tree_nonintegral_gap_and_first_layer_height() {
    // Organic trees (tree3d) read these options from their own resolved settings, not from
    // the SupportConfig fields this test varies; tree3d has its own tests.
    {
        let style = SupportStyle::Tree;
        let mut c = config();
        c.style = style;
        c.first_layer_height = 0.3;
        c.top_gap = 0.15;
        let plan = run(&c, &tee());
        assert!((plan.layers[0].height - 0.3).abs() < EPS);
        assert!((plan.layers.last().unwrap().print_z - 9.95).abs() < EPS);
        assert!(
            plan.layers
                .iter()
                .all(|l| l.height > 0.0 && l.height <= c.nozzle_diameter + EPS)
        );
        assert!(plan.tree_statistics.unwrap().supported_tips > 0);
    }
}

#[test]
fn narrow_overhang_uses_printable_tips_instead_of_empty_roofs() {
    // Cover both a small island and a long sliver whose area exceeds the roof threshold.
    for overhang in [rect(3.0, 0.0, 4.0, 0.8), rect(3.0, 0.0, 10.0, 0.25)] {
        let areas: Vec<_> = (0..60)
            .map(|i| {
                let mut a = vec![rect(0.0, 0.0, 2.0, 2.0)];
                if i >= 50 {
                    a.push(overhang.clone());
                }
                a
            })
            .collect();
        for style in [SupportStyle::Tree, SupportStyle::Organic] {
            let mut c = config();
            c.style = style;
            let plan = run(&c, &areas);
            let i = plan.layers.len() - 1;
            assert!((plan.layers[i].print_z - 9.8).abs() < EPS);
            assert!(!plan.toolpaths(i).paths.is_empty());
            let stats = plan.tree_statistics.unwrap();
            assert_eq!(stats.tips, stats.supported_tips);
        }
    }
}

/// A sloped overhang above a column: every layer adds a 1.2 mm overhang band.
fn ramp() -> Vec<Vec<ExPolygon>> {
    (0..50)
        .map(|i| {
            let reach = if i < 30 {
                10.0
            } else {
                10.0 + 1.2 * (i - 29) as f64
            };
            vec![rect(0.0, 0.0, reach, 12.0)]
        })
        .collect()
}

fn region_area(plan: &SupportPlan, kinds: &[SupportLayerKind]) -> f64 {
    plan.layers
        .iter()
        .flat_map(|l| &l.regions)
        .filter(|r| kinds.contains(&r.kind))
        .flat_map(|r| &r.polygons)
        .map(ExPolygon::area_mm2)
        .sum()
}

#[test]
fn smooth_supports_regularize_sloped_interfaces() {
    use SupportLayerKind as K;
    let top = [K::TopContact, K::TopInterface];
    let share = |style, orca| {
        let mut c = config();
        c.style = style;
        c.orca_semantics = orca;
        c.top_interface_layers = 2;
        c.interface_spacing = 0.5;
        let plan = run(&c, &ramp());
        let interface = region_area(&plan, &top);
        let base = region_area(&plan, &[K::Base]);
        interface / (interface + base)
    };
    // Organic trees (tree3d) read the interface options from their own resolved settings.
    for style in [SupportStyle::Tree, SupportStyle::Grid] {
        let (prusa, orca) = (share(style, false), share(style, true));
        // Grid supports are unchanged under Prusa semantics (union only, no closing). Orca's
        // count adds a dense layer below every band of the slope (trees through the slab test
        // over continuing branches), raising the interface share by about a third.
        if style == SupportStyle::Grid {
            assert!((prusa - 0.091).abs() < 0.005, "{prusa}");
        }
        assert!(prusa > 0.06, "{style:?} {prusa}");
        assert!(orca > prusa + 0.02, "{style:?} {prusa} {orca}");
    }
}

#[test]
fn orca_small_overhang_clusters_are_removed_and_sharp_tails_kept() {
    // A column with a thin fin (0.4 mm wide) and, separately, a wide shelf from layer 30.
    let model = |fin: bool, shelf: bool, tail: bool| -> Vec<Vec<ExPolygon>> {
        (0..50)
            .map(|i| {
                let mut a = vec![rect(0.0, 0.0, 10.0, 10.0)];
                if fin && i >= 30 {
                    a[0] = rect(0.0, 0.0, 10.0, 10.0);
                    a.push(rect(10.0, 4.0, 13.0, 4.4));
                }
                if shelf && i >= 30 {
                    a[0] = rect(0.0, 0.0, 14.0, 10.0);
                }
                // A small island starting in the air (a sharp tail, e.g. a dangling tip).
                if tail && i >= 30 {
                    a.push(rect(20.0, 0.0, 20.0 + 0.4 + 0.1 * (i - 30) as f64, 0.5));
                }
                a
            })
            .collect()
    };
    let support = |c: &SupportConfig, areas: &[Vec<ExPolygon>]| {
        let plan = run(c, areas);
        (0..plan.layers.len())
            .flat_map(|i| plan.toolpaths(i).paths)
            .filter(|p| {
                matches!(
                    p.role,
                    Role::SupportMaterial | Role::SupportMaterialInterface
                )
            })
            .count()
    };
    let mut c = config();
    c.style = SupportStyle::Grid;
    c.orca_semantics = true;
    c.remove_small_overhang = false;
    assert!(support(&c, &model(true, false, false)) > 0);
    c.remove_small_overhang = true;
    assert_eq!(support(&c, &model(true, false, false)), 0);
    assert!(support(&c, &model(false, true, false)) > 0);
    // Sharp tails are never removed as small clusters, even though their footprint is tiny.
    assert!(support(&c, &model(false, false, true)) > 0);
    // Prusa semantics ignore the Orca option.
    c.orca_semantics = false;
    assert!(support(&c, &model(true, false, false)) > 0);
}

/// GUI monkey test (seed 14): projecting a large model's support columns down ran for tens of
/// seconds without a cancel check. The projection now polls the observer per layer: a cancel
/// arriving during it ends the support stage there.
#[test]
fn support_projection_stops_at_a_cancel() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct CancelAfter(AtomicUsize, usize);
    impl crate::progress::Observer for CancelAfter {
        fn progress(&self, _: crate::progress::Stage, _: f32) {}
        fn cancelled(&self) -> bool {
            self.0.fetch_add(1, Ordering::Relaxed) >= self.1
        }
    }
    let c = config();
    let p = SupportParameters::new(&c).unwrap();
    let areas = tee();
    let layers: Vec<_> = areas
        .iter()
        .enumerate()
        .map(|(i, area)| SupportObjectLayer {
            print_z: p.object_z_offset + c.first_layer_height + i as f64 * c.layer_height,
            height: if i == 0 {
                c.first_layer_height
            } else {
                c.layer_height
            },
            outlines: area,
            overhangs: None,
            bridges: None,
        })
        .collect();
    let input = SupportInput {
        masks: None,
        layers: &layers,
        config: &c,
        origin: Point::default(),
    };
    // One poll after the contacts, then one per projected layer: cancelled five layers in.
    let observer = CancelAfter(AtomicUsize::new(0), 6);
    let Err(error) = generate_observed(&input, &observer) else {
        panic!("not cancelled");
    };
    assert_eq!(error.0, crate::progress::CANCELLED);
    assert_eq!(
        observer.0.load(Ordering::Relaxed),
        7,
        "stopped at the first cancelled poll"
    );
}
