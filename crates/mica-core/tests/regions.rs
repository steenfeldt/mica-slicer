// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

use mica_core::{
    config::Config,
    geometry::{SCALE, polyline_length, signed_area},
    mesh::TriangleMesh,
    print::{LayerSource, prepare_object},
    print_job::{PreparedPrint, PrintObject, ToolAssignments},
    regions::{HeightRange, RegionModel, Volume},
    settings::PrintSettings,
    threemf::PartKind,
    toolpath::{LayerToolpaths, Role},
};

fn config() -> Config {
    let mut c = Config::defaults();
    for (k, v) in [
        ("layer_height", "0.2"),
        ("first_layer_height", "0.2"),
        ("perimeter_generator", "classic"),
        ("elefant_foot_compensation", "0"),
        ("skirts", "0"),
        ("brim_width", "0"),
        ("wipe_tower", "0"),
        ("fill_pattern", "rectilinear"),
        ("fill_density", "15%"),
        ("solid_infill_below_area", "0"),
    ] {
        c.set_raw(k, v).unwrap();
    }
    c
}
fn patch(values: &[(&str, &str)]) -> Config {
    let mut c = Config::new();
    for (k, v) in values {
        c.set_raw(k, v).unwrap();
    }
    c
}
fn cube(a: [f32; 3], b: [f32; 3]) -> TriangleMesh {
    let [x, y, z] = a;
    let [u, v, w] = b;
    TriangleMesh {
        vertices: vec![
            [x, y, z],
            [u, y, z],
            [u, v, z],
            [x, v, z],
            [x, y, w],
            [u, y, w],
            [u, v, w],
            [x, v, w],
        ],
        faces: vec![
            [0, 2, 1],
            [0, 3, 2],
            [4, 5, 6],
            [4, 6, 7],
            [0, 1, 5],
            [0, 5, 4],
            [1, 2, 6],
            [1, 6, 5],
            [2, 3, 7],
            [2, 7, 6],
            [3, 0, 4],
            [3, 4, 7],
        ],
    }
}
fn model() -> RegionModel {
    RegionModel {
        full_spectrum: Default::default(),
        volumes: vec![Volume {
            painting: Default::default(),
            mesh: cube([0.; 3], [20.; 3]),
            kind: PartKind::Model,
            config: Config::new(),
        }],
        height_ranges: vec![],
        layer_height_profile: vec![],
        brim_ears: vec![],
    }
}
fn prepare(m: &RegionModel, c: &Config) -> PrintObject {
    let mut mesh = TriangleMesh::default();
    for v in m.volumes.iter().filter(|v| v.kind == PartKind::Model) {
        let base = mesh.vertices.len() as u32;
        mesh.vertices.extend_from_slice(&v.mesh.vertices);
        mesh.faces
            .extend(v.mesh.faces.iter().map(|f| f.map(|i| i + base)));
    }
    let tools = ToolAssignments::resolve(
        c,
        &Default::default(),
        &Default::default(),
        0,
        c.vector_len("nozzle_diameter").max(1),
    )
    .unwrap();
    PrintObject::prepare_regions("fixture".into(), 0, 0, mesh, m, tools, c).unwrap()
}
fn amount(l: &LayerToolpaths, role: Role) -> f64 {
    l.paths
        .iter()
        .filter(|p| p.role == role)
        .map(|p| polyline_length(&p.points) / SCALE * p.flow.mm3_per_mm())
        .sum()
}
fn area(p: &[mica_core::geometry::ExPolygon]) -> f64 {
    p.iter()
        .map(|p| {
            signed_area(&p.contour).abs()
                - p.holes.iter().map(|h| signed_area(h).abs()).sum::<f64>()
        })
        .sum::<f64>()
        / (SCALE * SCALE)
}

#[test]
fn speed_modifier_on_first_roof_layer_keeps_internal_bridge_support() {
    let mut c = config();
    c.set_raw("top_solid_layers", "5").unwrap();
    let plain = prepare(&model(), &c);
    let mut modified = model();
    modified.volumes.push(Volume {
        mesh: cube([-1., -1., 19.], [21., 21., 19.2]),
        kind: PartKind::Modifier,
        painting: Default::default(),
        config: patch(&[
            ("perimeter_speed", "50"),
            ("external_perimeter_speed", "50"),
            ("infill_speed", "50"),
            ("solid_infill_speed", "50"),
            ("top_solid_infill_speed", "50"),
            ("gap_fill_speed", "50"),
        ]),
    });
    let modified = prepare(&modified, &c);
    assert_eq!(modified.prepared.region_count(), 2);
    let a = plain.prepared.toolpaths(95..96);
    let b = modified.prepared.toolpaths(95..96);
    let reference = amount(&a[0], Role::InternalBridgeInfill);
    assert!(reference > 10., "fixture must contain an internal bridge");
    assert!(
        (amount(&b[0], Role::InternalBridgeInfill) / reference - 1.).abs() < 0.01,
        "a speed-only modifier must not remove the sparse infill anchors below it"
    );
}

#[test]
fn noop_regions_preserve_material_and_settings_are_interned() {
    let c = config();
    let mut m = model();
    m.volumes.push(Volume {
        painting: Default::default(),
        mesh: cube([5.; 3], [15.; 3]),
        kind: PartKind::Modifier,
        config: Config::new(),
    });
    let region = prepare(&m, &c);
    assert_eq!(region.prepared.region_count(), 1);
    let legacy =
        prepare_object(&m.volumes[0].mesh, &PrintSettings::from_config(&c).unwrap()).unwrap();
    let a = legacy.toolpaths(0..legacy.layer_count());
    let b = region.prepared.toolpaths(0..region.prepared.layer_count());
    for (a, b) in a.iter().zip(&b) {
        for role in [
            Role::Perimeter,
            Role::ExternalPerimeter,
            Role::InternalInfill,
            Role::SolidInfill,
            Role::TopSolidInfill,
        ] {
            assert!(
                (amount(a, role) - amount(b, role)).abs() < 0.01,
                "layer {} {role:?}: {} != {}",
                a.index,
                amount(a, role),
                amount(b, role)
            );
        }
    }
}
#[test]
fn infill_modifier_shares_walls_and_changes_only_intersected_layers() {
    let c = config();
    let base = prepare(&model(), &c);
    let mut m = model();
    m.volumes.push(Volume {
        painting: Default::default(),
        mesh: cube([5.; 3], [15.; 3]),
        kind: PartKind::Modifier,
        config: patch(&[("fill_density", "100%")]),
    });
    let modified = prepare(&m, &c);
    assert_eq!(modified.prepared.region_count(), 2);
    for i in [10, 30, 80] {
        let a = base.prepared.toolpaths(i..i + 1).remove(0);
        let b = modified.prepared.toolpaths(i..i + 1).remove(0);
        assert!(
            (amount(&a, Role::ExternalPerimeter) - amount(&b, Role::ExternalPerimeter)).abs()
                < 1e-6
        );
        assert!((amount(&a, Role::Perimeter) - amount(&b, Role::Perimeter)).abs() < 1e-6);
        if i == 30 {
            assert!(amount(&b, Role::InternalInfill) > amount(&a, Role::InternalInfill) * 1.5);
        } else {
            assert!(
                (amount(&a, Role::InternalInfill) - amount(&b, Role::InternalInfill)).abs() < 0.01
            );
        }
        assert!(amount(&b, Role::TopSolidInfill) < 1e-6);
    }
}
#[test]
fn sparse_settings_do_not_split_compatible_solid_surfaces() {
    let c = config();
    let base = prepare(&model(), &c);
    let mut m = model();
    m.volumes.push(Volume {
        painting: Default::default(),
        mesh: cube([5., 5., -1.], [15., 15., 21.]),
        kind: PartKind::Modifier,
        config: patch(&[("fill_density", "40%"), ("fill_pattern", "gyroid")]),
    });
    let modified = prepare(&m, &c);
    for i in [0, 1, 98, 99] {
        let a = base.prepared.toolpaths(i..i + 1).remove(0);
        let b = modified.prepared.toolpaths(i..i + 1).remove(0);
        for role in [Role::SolidInfill, Role::TopSolidInfill] {
            assert!(
                (amount(&a, role) - amount(&b, role)).abs() < 0.01,
                "layer {i} {role:?}: {} != {}",
                amount(&a, role),
                amount(&b, role)
            );
        }
    }
}
#[test]
fn height_range_uses_slice_plane_without_artificial_caps() {
    let c = config();
    let mut m = model();
    m.height_ranges.push(HeightRange {
        min_z: 5.,
        max_z: 10.,
        config: patch(&[("perimeters", "4"), ("fill_pattern", "gyroid")]),
    });
    let o = prepare(&m, &c);
    for (i, walls) in [(24, 3), (25, 4), (49, 4), (50, 3)] {
        let l = o.prepared.toolpaths(i..i + 1).remove(0);
        assert_eq!(
            l.paths.iter().filter(|p| p.role == Role::Perimeter).count(),
            walls - 1
        );
        assert_eq!(amount(&l, Role::TopSolidInfill), 0.);
        // Extra walls may require a narrow vertical-shell ring, but never a whole solid cap.
        assert!(amount(&l, Role::SolidInfill) < 10., "layer {i}");
    }
}
#[test]
fn negative_and_later_solid_obey_volume_order() {
    let c = config();
    let mut m = model();
    m.volumes.push(Volume {
        painting: Default::default(),
        mesh: cube([5., 5., 0.], [15., 15., 20.]),
        kind: PartKind::NegativeVolume,
        config: Config::new(),
    });
    let o = prepare(&m, &c);
    assert!((area(o.prepared.layer_outlines(10).unwrap()) - 300.).abs() < 1e-5);
    m.volumes.push(Volume {
        painting: Default::default(),
        mesh: cube([8., 8., 0.], [12., 12., 20.]),
        kind: PartKind::Model,
        config: Config::new(),
    });
    let o = prepare(&m, &c);
    assert!((area(o.prepared.layer_outlines(10).unwrap()) - 316.).abs() < 1e-5);
}
#[test]
fn overlapping_modifiers_inherit_and_later_values_win() {
    let c = config();
    let mut m = model();
    for values in [
        &[("perimeters", "4")][..],
        &[("fill_density", "100%"), ("perimeters", "5")][..],
    ] {
        m.volumes.push(Volume {
            painting: Default::default(),
            mesh: cube([5.; 3], [15.; 3]),
            kind: PartKind::Modifier,
            config: patch(values),
        });
    }
    let o = prepare(&m, &c);
    let found = (0..o.prepared.region_count())
        .filter_map(|i| o.prepared.region(i))
        .any(|r| r.settings.perimeters == 5 && r.settings.fill_density == 1.);
    assert!(found);
}
#[test]
fn region_tools_survive_scheduling_and_batch_boundaries() {
    let mut c = config();
    c.set_raw("nozzle_diameter", "0.4,0.4").unwrap();
    let mut m = model();
    m.volumes.push(Volume {
        painting: Default::default(),
        mesh: cube([10., 0., 0.], [20.; 3]),
        kind: PartKind::Modifier,
        config: patch(&[("extruder", "2")]),
    });
    let job = PreparedPrint::new(vec![prepare(&m, &c)], &c).unwrap();
    let all = job.scheduled(0..job.layer_count());
    assert!(
        all.iter().all(|l| l.assignments.iter().any(|a| a.tool == 0)
            && l.assignments.iter().any(|a| a.tool == 1))
    );
    let split: Vec<_> = (0..job.layer_count())
        .step_by(13)
        .flat_map(|i| job.scheduled(i..(i + 13).min(job.layer_count())))
        .collect();
    for (a, b) in all.iter().zip(split) {
        assert_eq!(a.layer.paths, b.layer.paths);
        assert_eq!(a.assignments, b.assignments);
    }
}
#[test]
fn object_layer_height_and_combined_infill() {
    let mut c = config();
    c.set_raw("layer_height", "0.1").unwrap();
    c.set_raw("infill_every_layers", "3").unwrap();
    let o = prepare(&model(), &c);
    assert_eq!(o.prepared.layer_count(), 199);
    let layers = o.prepared.toolpaths(40..46);
    assert!(layers.iter().any(|l| {
        l.paths
            .iter()
            .any(|p| p.role == Role::InternalInfill && (p.flow.height - 0.3).abs() < 1e-6)
    }));
    assert!(layers.iter().any(|l| amount(l, Role::InternalInfill) == 0.));
}
#[test]
fn blocker_and_manual_enforcer_control_normal_and_tree_contacts() {
    for style in ["snug", "organic"] {
        let mut c = config();
        for (k, v) in [
            ("support_material", "1"),
            ("support_material_style", style),
            ("dont_support_bridges", "0"),
        ] {
            c.set_raw(k, v).unwrap();
        }
        let mut m = RegionModel {
            full_spectrum: Default::default(),
            volumes: vec![
                Volume {
                    painting: Default::default(),
                    mesh: cube([8., 8., 0.], [12., 12., 8.]),
                    kind: PartKind::Model,
                    config: Config::new(),
                },
                Volume {
                    painting: Default::default(),
                    mesh: cube([0., 0., 8.], [20., 20., 10.]),
                    kind: PartKind::Model,
                    config: Config::new(),
                },
            ],
            height_ranges: vec![],
            layer_height_profile: vec![],
            brim_ears: vec![],
        };
        let support_amount = |o: PrintObject| {
            o.prepared
                .toolpaths(0..o.prepared.layer_count())
                .iter()
                .map(|l| {
                    amount(l, Role::SupportMaterial) + amount(l, Role::SupportMaterialInterface)
                })
                .sum::<f64>()
        };
        assert!(support_amount(prepare(&m, &c)) > 0.);
        m.volumes.push(Volume {
            painting: Default::default(),
            mesh: cube([-1., -1., 0.], [21., 21., 11.]),
            kind: PartKind::SupportBlocker,
            config: Config::new(),
        });
        assert_eq!(support_amount(prepare(&m, &c)), 0.);
        c.set_raw("support_material_auto", "0").unwrap();
        m.volumes.last_mut().unwrap().kind = PartKind::SupportEnforcer;
        assert!(support_amount(prepare(&m, &c)) > 0.);
    }
}

#[test]
fn negative_top_trims_schedule_and_range_heights_change_it() {
    let c = config();
    let mut m = model();
    m.volumes.push(Volume {
        painting: Default::default(),
        mesh: cube([-1., -1., 10.], [21.; 3]),
        kind: PartKind::NegativeVolume,
        config: Config::new(),
    });
    assert_eq!(prepare(&m, &c).prepared.layer_count(), 50);
    let mut m = model();
    m.height_ranges.push(HeightRange {
        min_z: 5.,
        max_z: 10.,
        config: patch(&[("layer_height", "0.1"), ("perimeters", "4")]),
    });
    let o = prepare(&m, &c);
    assert_eq!(o.prepared.layer_count(), 125);
    assert!((o.prepared.layer_z(25).1 - 0.1).abs() < 1e-6);
    assert!((o.prepared.layer_z(75).1 - 0.2).abs() < 1e-6);
}

#[test]
fn layer_height_profile_drives_the_layers() {
    use mica_core::layer_profile::{SlicingParams, tops};
    let c = config();
    let mut m = model();
    // 0.2 mm up to 5 mm, then a ramp to 0.1 mm at the top (20 mm).
    m.layer_height_profile = vec![0., 0.2, 0.2, 0.2, 5., 0.2, 20., 0.1];
    let o = prepare(&m, &c);
    let p = SlicingParams::from_config(&c, &[0], 20.);
    let expected = tops(&p, &m.layer_height_profile);
    assert_eq!(o.prepared.layer_count(), expected.len());
    for (i, z) in expected.iter().enumerate() {
        assert!((o.prepared.layer_z(i).0 - z).abs() < 1e-9, "layer {i}");
    }
    assert!(o.prepared.layer_count() > 100);
    let last = o.prepared.layer_count() - 1;
    assert!(o.prepared.layer_z(last).1 < 0.15);
    // A profile for another object height is ignored (upstream drops it).
    m.layer_height_profile = vec![0., 0.2, 0.2, 0.2, 10., 0.1];
    assert_eq!(prepare(&m, &c).prepared.layer_count(), 100);
}

#[test]
fn normal_enforcer_can_rest_on_model_with_buildplate_only_enabled() {
    let mut c = config();
    for (k, v) in [
        ("support_material", "1"),
        ("support_material_style", "snug"),
        ("support_material_buildplate_only", "1"),
        ("dont_support_bridges", "0"),
    ] {
        c.set_raw(k, v).unwrap();
    }
    let mut m = RegionModel {
        full_spectrum: Default::default(),
        volumes: vec![],
        height_ranges: vec![],
        layer_height_profile: vec![],
        brim_ears: vec![],
    };
    for (a, b) in [
        ([0.; 3], [12., 12., 1.]),
        ([5., 5., 1.], [7., 7., 4.]),
        ([0., 0., 4.], [12., 12., 5.]),
    ] {
        m.volumes.push(Volume {
            painting: Default::default(),
            mesh: cube(a, b),
            kind: PartKind::Model,
            config: Config::new(),
        });
    }
    let count = |o: PrintObject| {
        o.prepared
            .toolpaths(0..o.prepared.layer_count())
            .iter()
            .map(|l| amount(l, Role::SupportMaterial) + amount(l, Role::SupportMaterialInterface))
            .sum::<f64>()
    };
    assert_eq!(count(prepare(&m, &c)), 0.);
    m.volumes.push(Volume {
        painting: Default::default(),
        mesh: cube([-1.; 3], [13.; 3]),
        kind: PartKind::SupportEnforcer,
        config: Config::new(),
    });
    assert!(count(prepare(&m, &c)) > 0.);
}

#[test]
fn small_feature_modifiers_keep_distinct_wall_and_fill_settings() {
    let c = config();
    let mut m = model();
    m.volumes.push(Volume {
        mesh: cube([10., 0., 0.], [21., 21., 21.]),
        kind: PartKind::Modifier,
        painting: Default::default(),
        config: patch(&[
            ("small_area_infill_flow_compensation", "1"),
            ("small_perimeter_threshold", "1"),
            ("small_perimeter_speed", "25%"),
        ]),
    });
    let job = PreparedPrint::new(vec![prepare(&m, &c)], &c).unwrap();
    let layer = job.scheduled_toolpaths(99..100).remove(0);
    let mut fill = [false; 2];
    let mut walls = [false; 2];
    for (path, assignment) in layer.layer.paths.iter().zip(&layer.assignments) {
        let (_, s) = job.objects[assignment.object].path_config(assignment.region, assignment.tool);
        if path.role == Role::TopSolidInfill {
            fill[usize::from(s.small_features.flow_compensation.is_some())] = true;
        }
        if matches!(path.role, Role::Perimeter | Role::ExternalPerimeter) {
            walls[usize::from(s.small_features.small_perimeter_threshold == 1.)] = true;
        }
    }
    assert_eq!(
        fill,
        [true, true],
        "fill grouping lost a modifier's flow model"
    );
    assert_eq!(
        walls,
        [true, true],
        "wall grouping lost a modifier's loop threshold"
    );
}

#[test]
fn prusa_regions_print_island_by_island_like_a_single_region_object() {
    let c = config();
    let mut m = model();
    m.volumes[0].mesh = cube([0.; 3], [10., 10., 6.]);
    m.volumes.push(Volume {
        painting: Default::default(),
        mesh: cube([20., 0., 0.], [30., 10., 6.]),
        kind: PartKind::Model,
        config: Config::new(),
    });
    let region = prepare(&m, &c);
    let mut merged = TriangleMesh::default();
    for v in &m.volumes {
        let base = merged.vertices.len() as u32;
        merged.vertices.extend_from_slice(&v.mesh.vertices);
        merged
            .faces
            .extend(v.mesh.faces.iter().map(|f| f.map(|i| i + base)));
    }
    let legacy = prepare_object(&merged, &PrintSettings::from_config(&c).unwrap()).unwrap();
    let a = legacy.toolpaths(0..legacy.layer_count());
    let b = region.prepared.toolpaths(0..region.prepared.layer_count());
    let xs = || a[0].paths.iter().flat_map(|p| p.points.iter().map(|q| q.x));
    let centre = (xs().min().unwrap() + xs().max().unwrap()) / 2;
    let islands = |l: &LayerToolpaths| {
        let mut seq: Vec<bool> = l.paths.iter().map(|p| p.points[0].x < centre).collect();
        seq.dedup();
        seq
    };
    let roles = |l: &LayerToolpaths| l.paths.iter().map(|p| p.role).collect::<Vec<_>>();
    for (a, b) in a.iter().zip(&b) {
        // GCode/ExtrusionOrder.cpp: an island's perimeters and fills, then the next island's.
        assert_eq!(islands(b).len(), 2, "layer {}", b.index);
        assert_eq!(islands(a), islands(b), "layer {}", b.index);
        assert_eq!(roles(a), roles(b), "layer {}", b.index);
    }
}

#[test]
fn ironing_is_per_island_only_under_orca_rules() {
    use mica_core::rules::Rules;

    for rules in Rules::ALL {
        let mut c = config();
        rules.set(&mut c);
        c.set_raw("ironing", "1").unwrap();
        c.set_raw("ironing_type", "top").unwrap();
        let mut m = model();
        m.volumes[0].mesh = cube([0.; 3], [10., 10., 1.]);
        m.volumes.push(Volume {
            painting: Default::default(),
            mesh: cube([20., 0., 0.], [30., 10., 1.]),
            kind: PartKind::Model,
            config: Config::new(),
        });
        let mut merged = m.volumes[0].mesh.clone();
        let base = merged.vertices.len() as u32;
        merged
            .vertices
            .extend_from_slice(&m.volumes[1].mesh.vertices);
        merged
            .faces
            .extend(m.volumes[1].mesh.faces.iter().map(|f| f.map(|i| i + base)));

        // Exercise the plain-object path, the indexed path with one region, and two regions
        // in each island. All regions' normal fills must finish before the ironing pass.
        for pipeline in 0..3 {
            if pipeline == 2 {
                m.volumes.push(Volume {
                    painting: Default::default(),
                    mesh: cube([-1., -1., -1.], [31., 5., 2.]),
                    kind: PartKind::Modifier,
                    config: patch(&[("perimeter_speed", "40"), ("ironing_spacing", "0.2")]),
                });
            }
            let top = |config: &Config| {
                if pipeline == 0 {
                    let p = prepare_object(&merged, &PrintSettings::from_config(config).unwrap())
                        .unwrap();
                    p.toolpaths(p.layer_count() - 1..p.layer_count())
                        .pop()
                        .unwrap()
                } else {
                    let p = prepare(&m, config);
                    let n = p.prepared.layer_count();
                    if pipeline == 2 {
                        assert_eq!(p.prepared.layer_regions(n - 1).unwrap().len(), 2);
                    }
                    p.prepared.toolpaths(n - 1..n).pop().unwrap()
                }
            };
            let layer = top(&c);
            let mut seq: Vec<_> = layer
                .paths
                .iter()
                .map(|p| {
                    let island = usize::from(p.points[0].x > mica_core::geometry::scale(15.));
                    let stage = match p.role {
                        Role::Ironing => 2,
                        Role::TopSolidInfill => 1,
                        Role::Perimeter | Role::ExternalPerimeter | Role::GapFill => 0,
                        role => panic!("unexpected top-layer role {role:?}"),
                    };
                    (island, stage)
                })
                .collect();
            seq.dedup();
            if rules.is_orca() {
                let first = seq[0].0;
                assert_eq!(
                    seq,
                    vec![
                        (first, 0),
                        (first, 1),
                        (first, 2),
                        (1 - first, 0),
                        (1 - first, 1),
                        (1 - first, 2)
                    ],
                    "pipeline {pipeline}"
                );
            } else {
                let first_ironing = layer
                    .paths
                    .iter()
                    .position(|p| p.role == Role::Ironing)
                    .unwrap();
                assert!(
                    layer.paths[first_ironing..]
                        .iter()
                        .all(|p| p.role == Role::Ironing)
                );
                let mut disabled = c.clone();
                disabled.set_raw("ironing", "0").unwrap();
                assert_eq!(
                    layer.paths[..first_ironing],
                    top(&disabled).paths,
                    "Prusa normal paths changed in pipeline {pipeline}"
                );
                let ironed: std::collections::HashSet<_> = seq
                    .iter()
                    .filter(|(_, stage)| *stage == 2)
                    .map(|(island, _)| *island)
                    .collect();
                assert_eq!(ironed.len(), 2);
            }
        }
    }
}

#[test]
fn texture_modifier_textures_only_the_walls_it_covers() {
    let c = config();
    let base = prepare(&model(), &c);
    let mut m = model();
    m.volumes.push(Volume {
        painting: Default::default(),
        mesh: cube([-1., -1., 9.], [21., 21., 15.]),
        kind: PartKind::Modifier,
        config: patch(&[
            ("mica_texture_skin", "external"),
            ("mica_texture_skin_texture", "procedural:ribs"),
        ]),
    });
    let textured = prepare(&m, &c);
    assert_eq!(textured.prepared.region_count(), 2);
    let points = |o: &PrintObject, i: usize| -> usize {
        o.prepared.toolpaths(i..i + 1)[0]
            .paths
            .iter()
            .filter(|p| p.role == Role::ExternalPerimeter)
            .map(|p| p.points.len())
            .sum()
    };
    for (layer, inside) in [(20, false), (60, true), (90, false)] {
        let (a, b) = (points(&base, layer), points(&textured, layer));
        if inside {
            assert!(b > a * 10, "layer {layer}: {b} vs {a} points");
        } else {
            assert_eq!(a, b, "layer {layer}");
        }
    }
}
