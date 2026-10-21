// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

use mica_core::print::LayerSource;
use mica_core::{
    config::Config,
    geometry::{ExPolygon, Point},
    mesh::TriangleMesh,
    painting::{FacetPainting, FacetStates, projection, query::PaintedQuery, segmentation},
    regions::{RegionModel, Volume},
    seam::painting::{Paint, SeamPainting},
    threemf::PartKind,
};

fn cube() -> TriangleMesh {
    TriangleMesh {
        vertices: vec![
            [0., 0., 0.],
            [20., 0., 0.],
            [20., 20., 0.],
            [0., 20., 0.],
            [0., 0., 20.],
            [20., 0., 20.],
            [20., 20., 20.],
            [0., 20., 20.],
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
fn config() -> Config {
    let mut c = Config::defaults();
    for (k, v) in [
        ("nozzle_diameter", "0.4,0.4"),
        ("top_solid_layers", "0"),
        ("bottom_solid_layers", "0"),
        ("mmu_segmented_region_max_width", "0"),
        ("skirts", "0"),
        ("wipe_tower", "0"),
        ("seam_position", "rear"),
    ] {
        c.set_raw(k, v).unwrap();
    }
    c
}
fn area(polys: &[ExPolygon]) -> f64 {
    polys.iter().map(ExPolygon::area_mm2).sum()
}
fn outline() -> Vec<ExPolygon> {
    vec![ExPolygon {
        contour: vec![
            Point::from_mm(0., 0.),
            Point::from_mm(20., 0.),
            Point::from_mm(20., 20.),
            Point::from_mm(0., 20.),
        ],
        holes: vec![],
    }]
}

#[test]
fn selector_all_splits_and_extended_states() {
    let mesh = TriangleMesh {
        vertices: vec![[0., 0., 0.], [4., 0., 0.], [0., 4., 0.]],
        faces: vec![[0, 1, 2]],
    };
    // The rightmost nibble is the root. Children are consumed highest index first.
    for (wire, expected) in [
        ("4", vec![1]),
        ("8", vec![2]),
        ("0C", vec![3]),
        ("DC", vec![16]),
        ("00EC", vec![17]),
        ("EEEC", vec![255]),
        ("481", vec![2, 1]),
        ("0482", vec![2, 1, 0]),
        ("04883", vec![2, 2, 1, 0]),
    ] {
        let mut s = FacetStates::default();
        s.push(0, wire).unwrap();
        let mut values = vec![];
        let mut total = 0.;
        s.visit(&mesh, true, |state, t| {
            values.push(state);
            total += projection::normal_z(t) * 0.5;
        });
        assert_eq!(values, expected, "{wire}");
        assert!((total - 8.).abs() < 1e-10, "{wire}");
    }
    for side in [0, 1, 2] {
        for splits in [1, 2] {
            let root = char::from_digit(side * 4 + splits, 16).unwrap();
            let wire = format!("{}{root}", "4".repeat(splits as usize + 1));
            let mut s = FacetStates::default();
            s.push(0, &wire).unwrap();
            let mut total = 0.;
            s.visit(&mesh, false, |_, t| total += projection::normal_z(t) * 0.5);
            assert!((total - 8.).abs() < 1e-10);
        }
    }
}
#[test]
fn selector_rejects_malformed_or_unbounded_data() {
    for wire in ["G", "1", "C", "EC", "FFEC", "F", "40", "4437", "444D"] {
        assert!(FacetStates::default().push(0, wire).is_err(), "{wire}");
    }
    let deep = format!("{}{}", "4".repeat(66), "1".repeat(66));
    assert!(FacetStates::default().push(0, &deep).is_err());
}
#[test]
fn split_geometry_survives_winding_repair() {
    let mut mesh = TriangleMesh {
        vertices: vec![[0., 0., 0.], [4., 0., 0.], [0., 4., 0.]],
        faces: vec![[0, 1, 2]],
    };
    let mut paint = FacetPainting::default();
    paint.seam.push(0, "481").unwrap();
    let mut a = vec![];
    paint.seam.visit(&mesh, false, |s, t| a.push((s, t)));
    mesh.faces[0].swap(1, 2);
    paint.reverse_winding();
    let mut b = vec![];
    paint.seam.visit(&mesh, false, |s, mut t| {
        t.swap(1, 2);
        b.push((s, t));
    });
    assert_eq!(a, b);
}
#[test]
fn support_projection_keeps_downward_facets_and_split_states() {
    let mut mesh = cube();
    for p in &mut mesh.vertices {
        p[2] += 8.;
    }
    let mut s = FacetStates::default();
    s.push(0, "4").unwrap();
    s.push(1, "8").unwrap();
    s.push(2, "4").unwrap();
    let zs = vec![7.9, 8.1, 8.3, 28.1];
    let mut en = vec![vec![]; 4];
    let mut bl = en.clone();
    projection::support_masks(&mesh, &s, &zs, &mut en, &mut bl);
    assert!((area(&en[1]) - 200.).abs() < 1e-6);
    assert!((area(&bl[1]) - 200.).abs() < 1e-6);
    assert!(en[0].is_empty() && en[2].is_empty() && en[3].is_empty());
}
#[test]
fn seam_query_blockers_win_and_partial_facets_are_local() {
    let mut paint = FacetPainting::default();
    paint.seam.push(6, "4").unwrap();
    paint.seam.push(7, "8").unwrap();
    let model = RegionModel {
        full_spectrum: Default::default(),
        volumes: vec![Volume {
            mesh: cube(),
            kind: PartKind::Model,
            config: Config::new(),
            painting: paint,
        }],
        height_ranges: vec![],
        layer_height_profile: vec![],
        brim_ears: vec![],
    };
    let q = PaintedQuery::new(&model, false);
    assert_eq!(q.classify([20., 15., 5.], 0.1), Paint::Enforcer);
    assert_eq!(q.classify([20., 5., 15.], 0.1), Paint::Blocker);
    assert_eq!(q.classify([20., 10., 10.], 0.1), Paint::Blocker);
    assert_eq!(q.classify([0., 10., 10.], 0.1), Paint::Common);
}
#[test]
fn adjacent_painted_sides_partition_cube_and_width_depth() {
    let mesh = cube();
    let mut states = FacetStates::default();
    for i in 6..10 {
        states.push(i, "8").unwrap();
    }
    let mut c = config();
    let outlines = vec![outline(); 2];
    let result = segmentation::segment(&mesh, &states, &[9.9, 10.1], &outlines, &c).unwrap();
    for layer in &result {
        assert!(
            (area(&layer[2]) - 200.).abs() < 0.01,
            "{:?}",
            layer.iter().map(|a| area(a)).collect::<Vec<_>>()
        );
        assert!((layer.iter().map(|a| area(a)).sum::<f64>() - 400.).abs() < 1e-6);
    }
    c.set_raw("mmu_segmented_region_max_width", "2").unwrap();
    c.set_raw("mmu_segmented_region_interlocking_depth", "1")
        .unwrap();
    let cut = segmentation::segment(&mesh, &states, &[9.9, 10.1], &outlines, &c).unwrap();
    assert!(
        (area(&cut[0][2]) - 38.).abs() < 0.01,
        "{}",
        area(&cut[0][2])
    );
    assert!(
        (area(&cut[1][2]) - 72.).abs() < 0.01,
        "{}",
        area(&cut[1][2])
    );
    c.set_raw("nozzle_diameter", "0.4").unwrap();
    assert!(segmentation::segment(&mesh, &states, &[10.], &[outline()], &c).is_err());
}

#[test]
fn painted_top_propagates_into_solid_layers_without_repainting_exterior() {
    let mesh = cube();
    let mut states = FacetStates::default();
    states.push(2, "8").unwrap();
    states.push(3, "8").unwrap();
    let mut c = config();
    c.set_raw("top_solid_layers", "3").unwrap();
    let zs = vec![18.9, 19.1, 19.3, 19.5, 19.7, 19.9];
    let layers =
        segmentation::segment(&mesh, &states, &zs, &vec![outline(); zs.len()], &c).unwrap();
    assert!(layers[0][2].is_empty());
    assert!(layers[1][2].is_empty());
    assert!((area(&layers[5][2]) - 400.).abs() < 0.01);
    assert!(area(&layers[4][2]) > 300. && area(&layers[4][2]) < 400.);
}

#[test]
fn interlocking_beams_alternate_and_preserve_total_material() {
    let mesh = cube();
    let mut states = FacetStates::default();
    for i in 6..10 {
        states.push(i, "8").unwrap();
    }
    let mut c = config();
    for (k, v) in [
        ("interlocking_beam", "1"),
        ("interlocking_boundary_avoidance", "1"),
        ("interlocking_orientation", "0"),
    ] {
        c.set_raw(k, v).unwrap();
    }
    let zs: Vec<_> = (0..100).map(|i| i as f64 * 0.2 + 0.1).collect();
    let result = segmentation::segment(&mesh, &states, &zs, &vec![outline(); 100], &c).unwrap();
    assert_ne!(result[50][2], result[52][2]);
    for layer in &result {
        assert!((layer.iter().map(|a| area(a)).sum::<f64>() - 400.).abs() < 0.01);
        assert!(mica_core::geometry::clip::intersection_ex(&layer[1], &layer[2]).is_empty());
    }
    c.set_raw("interlocking_beam_layer_count", "0").unwrap();
    assert!(segmentation::segment(&mesh, &states, &zs, &vec![outline(); 100], &c).is_err());
}

fn prepare(model: &RegionModel, c: &Config) -> mica_core::print_job::PrintObject {
    let mut mesh = TriangleMesh::default();
    for v in model.volumes.iter().filter(|v| v.kind == PartKind::Model) {
        let base = mesh.vertices.len() as u32;
        mesh.vertices.extend_from_slice(&v.mesh.vertices);
        mesh.faces
            .extend(v.mesh.faces.iter().map(|f| f.map(|i| i + base)));
    }
    let tools = mica_core::print_job::ToolAssignments::resolve(
        c,
        &Default::default(),
        &Default::default(),
        0,
        c.vector_len("nozzle_diameter"),
    )
    .unwrap();
    mica_core::print_job::PrintObject::prepare_regions(
        "painted".into(),
        0,
        0,
        mesh,
        model,
        tools,
        c,
    )
    .unwrap()
}

#[test]
fn split_material_preserves_model_silhouette_and_region_coverage() {
    let mut mesh = cube();
    for p in &mut mesh.vertices {
        p[0] = p[0] * 1.15 + 168.5;
        p[1] = p[1] * 0.95 + 170.5;
    }
    let mut model = RegionModel {
        full_spectrum: Default::default(),
        volumes: vec![Volume {
            mesh,
            kind: PartKind::Model,
            config: Config::new(),
            painting: FacetPainting::default(),
        }],
        height_ranges: vec![],
        layer_height_profile: vec![],
        brim_ears: vec![],
    };
    let c = config();
    let plain = prepare(&model, &c);
    for (triangle, wire) in [(6, "081"), (7, "0802"), (8, "08083")] {
        model.volumes[0]
            .painting
            .material
            .push(triangle, wire)
            .unwrap();
    }
    let painted = prepare(&model, &c);
    for i in 0..painted.prepared.layer_count() {
        let a = plain.prepared.layer_outlines(i).unwrap();
        let b = painted.prepared.layer_outlines(i).unwrap();
        assert_eq!(
            a, b,
            "colour boundaries changed model silhouette on layer {i}"
        );
        let regions = painted.prepared.layer_regions(i).unwrap();
        let regions_area: f64 = regions.iter().map(|r| area(&r.slices)).sum();
        // The first layer uses elephant-foot compensated region geometry.
        if i > 0 {
            assert!(
                (regions_area - area(b)).abs() < 1e-5,
                "layer {i}: {regions_area}"
            );
        }
    }
}

#[test]
fn fuzzy_painting_reaches_classic_and_arachne_walls_in_placed_coordinates() {
    for generator in ["classic", "arachne"] {
        let mut c = config();
        for (k, v) in [
            ("perimeter_generator", generator),
            ("fuzzy_skin", "none"),
            ("fuzzy_skin_thickness", "0.3"),
            ("fuzzy_skin_point_dist", "0.8"),
        ] {
            c.set_raw(k, v).unwrap();
        }
        let mut mesh = cube();
        for p in &mut mesh.vertices {
            p[0] += 40.;
            p[1] += 60.;
        }
        let mut paint = FacetPainting::default();
        for i in [6, 7] {
            paint.fuzzy.push(i, "4").unwrap();
        }
        let m = RegionModel {
            full_spectrum: Default::default(),
            volumes: vec![Volume {
                mesh,
                kind: PartKind::Model,
                config: Config::new(),
                painting: paint,
            }],
            height_ranges: vec![],
            layer_height_profile: vec![],
            brim_ears: vec![],
        };
        let o = prepare(&m, &c);
        let layer = o.prepared.toolpaths(10..11).pop().unwrap();
        let points: Vec<_> = layer
            .paths
            .iter()
            .filter(|p| p.role == mica_core::toolpath::Role::ExternalPerimeter)
            .flat_map(|p| &p.points)
            .collect();
        assert!(
            points
                .iter()
                .any(|p| p.x as f64 / mica_core::geometry::SCALE > 59.85),
            "{generator}"
        );
        let left: Vec<_> = points
            .iter()
            .filter(|p| {
                p.x as f64 / mica_core::geometry::SCALE < 40.5
                    && p.y > 65_000_000
                    && p.y < 75_000_000
            })
            .collect();
        assert!(!left.is_empty());
        let min = left.iter().map(|p| p.x).min().unwrap();
        let max = left.iter().map(|p| p.x).max().unwrap();
        assert!(
            max - min < 10,
            "unpainted wall changed for {generator}: {min}..{max}"
        );
    }
}

#[test]
fn material_painting_wins_modifier_tool_and_batches_keep_region_assignments() {
    let mut c = config();
    c.set_raw("perimeter_generator", "classic").unwrap();
    let mut paint = FacetPainting::default();
    for i in 6..10 {
        paint.material.push(i, "8").unwrap();
    }
    let mut modifier = Config::new();
    modifier.set_raw("extruder", "1").unwrap();
    modifier.set_raw("perimeter_speed", "17").unwrap();
    let m = RegionModel {
        full_spectrum: Default::default(),
        volumes: vec![
            Volume {
                mesh: cube(),
                kind: PartKind::Model,
                config: Config::new(),
                painting: paint,
            },
            Volume {
                mesh: cube(),
                kind: PartKind::Modifier,
                config: modifier,
                painting: FacetPainting::default(),
            },
        ],
        height_ranges: vec![],
        layer_height_profile: vec![],
        brim_ears: vec![],
    };
    let o = prepare(&m, &c);
    let job = mica_core::print_job::PreparedPrint::new(vec![o], &c).unwrap();
    let all = job.scheduled(10..14);
    assert!(all.iter().flat_map(|l| &l.assignments).any(|a| a.tool == 1));
    let split = job
        .scheduled(10..12)
        .into_iter()
        .chain(job.scheduled(12..14));
    for (a, b) in all.iter().zip(split) {
        assert_eq!(a.assignments, b.assignments);
        assert_eq!(a.layer.paths, b.layer.paths);
    }
}

#[test]
fn facet_blockers_and_manual_enforcers_feed_normal_and_tree_contacts() {
    for style in ["snug", "organic"] {
        let mut c = config();
        for (k, v) in [
            ("support_material", "1"),
            ("support_material_style", style),
            ("dont_support_bridges", "0"),
            ("support_material_auto", "0"),
        ] {
            c.set_raw(k, v).unwrap();
        }
        let mut stem = cube();
        for p in &mut stem.vertices {
            p[0] = p[0] * 0.1 + 3.;
            p[1] = p[1] * 0.1 + 3.;
            p[2] *= 0.2;
        }
        let mut roof = cube();
        for p in &mut roof.vertices {
            p[0] *= 0.4;
            p[1] *= 0.4;
            p[2] = p[2] * 0.05 + 4.;
        }
        let volume = |mesh| Volume {
            mesh,
            kind: PartKind::Model,
            config: Config::new(),
            painting: FacetPainting::default(),
        };
        let mut m = RegionModel {
            full_spectrum: Default::default(),
            volumes: vec![volume(stem), volume(roof)],
            height_ranges: vec![],
            layer_height_profile: vec![],
            brim_ears: vec![],
        };
        for i in [0, 1] {
            m.volumes[1].painting.supports.push(i, "4").unwrap();
        }
        let supports = |o: mica_core::print_job::PrintObject| {
            o.prepared
                .toolpaths(0..o.prepared.layer_count())
                .iter()
                .any(|l| {
                    l.paths.iter().any(|p| {
                        matches!(
                            p.role,
                            mica_core::toolpath::Role::SupportMaterial
                                | mica_core::toolpath::Role::SupportMaterialInterface
                        )
                    })
                })
        };
        assert!(supports(prepare(&m, &c)), "{style}");
        c.set_raw("support_material_auto", "1").unwrap();
        m.volumes[1].painting.supports = FacetStates::default();
        for i in [0, 1] {
            m.volumes[1].painting.supports.push(i, "8").unwrap();
        }
        assert!(!supports(prepare(&m, &c)), "{style}");
    }
}

/// A box spanning `lo..hi` (mm), built from the 20 mm cube so facets 0 and 1 are its bottom.
fn box_mesh(lo: [f32; 3], hi: [f32; 3]) -> TriangleMesh {
    let mut m = cube();
    for p in &mut m.vertices {
        for a in 0..3 {
            p[a] = lo[a] + (hi[a] - lo[a]) * p[a] / 20.;
        }
    }
    m
}

#[test]
fn enforcers_only_supports_follow_paint_and_enforcer_volumes() {
    // A 2 mm stem carries a 20 x 20 mm roof made of a left and a right half.
    let volume = |mesh, kind| Volume {
        mesh,
        kind,
        config: Config::new(),
        painting: FacetPainting::default(),
    };
    let model = || RegionModel {
        full_spectrum: Default::default(),
        volumes: vec![
            volume(box_mesh([9., 9., 0.], [11., 11., 6.]), PartKind::Model),
            volume(box_mesh([0., 0., 6.], [10., 20., 7.]), PartKind::Model),
            volume(box_mesh([10., 0., 6.], [20., 20., 7.]), PartKind::Model),
        ],
        height_ranges: vec![],
        layer_height_profile: vec![],
        brim_ears: vec![],
    };
    // X range (mm) of the support under the roof (above the trunks' spread and the first-layer
    // flange), or None without any support extrusion.
    let support_x = |m: &RegionModel, c: &Config| {
        let o = prepare(m, c);
        let layers = o.prepared.toolpaths(0..o.prepared.layer_count());
        let support = |p: &&mica_core::toolpath::ExtrusionPath| {
            matches!(
                p.role,
                mica_core::toolpath::Role::SupportMaterial
                    | mica_core::toolpath::Role::SupportMaterialInterface
            )
        };
        if !layers.iter().flat_map(|l| &l.paths).any(|p| support(&p)) {
            return None;
        }
        let xs: Vec<f64> = layers
            .iter()
            .filter(|l| l.print_z > 5.)
            .flat_map(|l| &l.paths)
            .filter(support)
            .flat_map(|p| p.points.iter().map(|q| q.x as f64 / 1e6))
            .collect();
        Some((
            xs.iter().copied().fold(f64::INFINITY, f64::min),
            xs.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        ))
    };
    for style in ["tree", "organic", "snug"] {
        let mut c = config();
        for (k, v) in [
            ("support_material", "1"),
            ("support_material_style", style),
            ("dont_support_bridges", "0"),
            ("support_material_auto", "1"),
        ] {
            c.set_raw(k, v).unwrap();
        }
        // Automatic supports cover both halves.
        let (lo, hi) = support_x(&model(), &c).expect(style);
        assert!(lo < 2. && hi > 18., "{style}: automatic {lo}..{hi}");
        // Enforcers only, nothing painted: no support at all.
        c.set_raw("support_material_auto", "0").unwrap();
        assert_eq!(support_x(&model(), &c), None, "{style}: no enforcers");
        // A painted enforcer on the left half's underside: support only there.
        let mut painted = model();
        for i in [0, 1] {
            painted.volumes[1].painting.supports.push(i, "4").unwrap();
        }
        let (lo, hi) = support_x(&painted, &c).expect(style);
        assert!(lo < 2. && hi < 12., "{style}: painted left half {lo}..{hi}");
        // An enforcer volume under the right half: support only there.
        let mut enforced = model();
        enforced.volumes.push(volume(
            box_mesh([12., 0., 0.], [20., 20., 7.]),
            PartKind::SupportEnforcer,
        ));
        let (lo, hi) = support_x(&enforced, &c).expect(style);
        assert!(lo > 10. && hi > 18., "{style}: enforcer volume {lo}..{hi}");
    }
}

#[test]
fn orca_xy_compensation_is_disabled_for_painted_multi_material_only() {
    let mut c = config();
    c.set_raw("filament_diameter", "1.75,1.75").unwrap();
    c.set_raw("elefant_foot_compensation", "0").unwrap();
    let mut model = RegionModel {
        full_spectrum: Default::default(),
        volumes: vec![Volume {
            mesh: cube(),
            kind: PartKind::Model,
            config: Config::new(),
            painting: FacetPainting::default(),
        }],
        height_ranges: vec![],
        layer_height_profile: vec![],
        brim_ears: vec![],
    };
    let plain = prepare(&model, &c);
    c.set_raw("xy_contour_compensation", "0.2").unwrap();
    let grown = prepare(&model, &c);
    assert!(area(grown.prepared.layer_outlines(2).unwrap()) > 415.);
    for i in 6..10 {
        model.volumes[0].painting.material.push(i, "8").unwrap();
    }
    let painted = prepare(&model, &c);
    assert_eq!(
        painted.prepared.layer_outlines(2),
        plain.prepared.layer_outlines(2)
    );
    // Prusa's separate compensation retains its existing semantics.
    c.set_raw("xy_size_compensation", "0.1").unwrap();
    let prusa = prepare(&model, &c);
    assert!(area(prusa.prepared.layer_outlines(2).unwrap()) > 407.);
}
