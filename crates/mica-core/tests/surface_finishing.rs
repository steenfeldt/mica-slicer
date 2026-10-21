// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original
use mica_core::{
    config::Config,
    fill::Pattern,
    fuzzy_skin::{FuzzySkinSettings, Mode, NoPainting, fuzzy_polygon},
    geometry::{Point, polygon_length},
    ironing::IroningType,
    mesh::TriangleMesh,
    print::{LayerSource, prepare_object},
    settings::{PerimeterGenerator, PrintSettings},
    toolpath::Role,
};
fn cube() -> TriangleMesh {
    let mut mesh = TriangleMesh::load_stl(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/models/cube20.stl"
    ))
    .unwrap();
    mesh.place_on_bed(125., 110.);
    mesh
}
#[test]
fn fuzzy_skin_statistics_and_boundary_selection() {
    let base = vec![
        Point::from_mm(0., 0.),
        Point::from_mm(100., 0.),
        Point::from_mm(100., 100.),
        Point::from_mm(0., 100.),
    ];
    let s = FuzzySkinSettings {
        mode: Mode::External,
        thickness: 0.3,
        point_distance: 0.8,
        seed: 42,
    };
    let mut total_count = 0;
    let mut total_length = 0.;
    for layer in 1..101 {
        let mut poly = base.clone();
        fuzzy_polygon(&mut poly, &s, layer, 0, true, &NoPainting);
        total_count += poly.len();
        total_length += polygon_length(&poly) / 1e6;
        assert!(
            poly.iter()
                .all(|p| p.x >= -300000 && p.x <= 100300000 && p.y >= -300000 && p.y <= 100300000)
        );
    }
    assert!((49500..50500).contains(&total_count));
    assert!((41000. ..44000.).contains(&total_length));
    for (layer, depth, contour) in [(0, 0, true), (1, 1, true), (1, 0, false)] {
        let mut poly = base.clone();
        fuzzy_polygon(&mut poly, &s, layer, depth, contour, &NoPainting);
        assert_eq!(poly, base);
    }
}
#[test]
fn ironing_roles_selection_and_flow() {
    let mesh = cube();
    let mut s = PrintSettings::core_one_hf04_speed_pla();
    s.ironing.enabled = true;
    for kind in [IroningType::Top, IroningType::Topmost, IroningType::Solid] {
        s.ironing.kind = kind;
        let obj = prepare_object(&mesh, &s).unwrap();
        let paths = obj.toolpaths(0..100);
        let iron = |i: usize| {
            paths[i]
                .paths
                .iter()
                .filter(|p| p.role == Role::Ironing)
                .count()
        };
        assert!(iron(99) > 0);
        assert_eq!(iron(50), 0);
        if kind == IroningType::Solid {
            assert!(iron(1) > 0)
        } else {
            assert_eq!(iron(1), 0)
        }
        let volume: f64 = paths[99]
            .paths
            .iter()
            .filter(|p| p.role == Role::Ironing)
            .map(|p| mica_core::geometry::polyline_length(&p.points) / 1e6 * p.flow.mm3_per_mm())
            .sum();
        // Analytic 19.6mm-square face at 0.2mm * 15% nominal ironing height.
        assert!((volume - 19.6 * 19.6 * 0.2 * 0.15).abs() < 0.4);
        assert_eq!(paths[99].paths.last().unwrap().role, Role::Ironing);
    }
}
#[test]
fn fuzzy_both_wall_generators_are_reproducible_and_skip_first_layer() {
    for generator in [PerimeterGenerator::Classic, PerimeterGenerator::Arachne] {
        let mut s = PrintSettings::core_one_hf04_speed_pla();
        s.perimeter_generator = generator;
        let plain = prepare_object(&cube(), &s).unwrap().toolpaths(0..3);
        s.fuzzy_skin.mode = Mode::External;
        let obj = prepare_object(&cube(), &s).unwrap();
        let fuzzy = obj.toolpaths(0..3);
        assert_eq!(fuzzy[0].paths, plain[0].paths);
        assert_eq!(fuzzy[1].paths, obj.toolpaths(1..2)[0].paths);
        let count = |layer: &mica_core::toolpath::LayerToolpaths| {
            layer
                .paths
                .iter()
                .filter(|p| p.role == Role::ExternalPerimeter)
                .map(|p| p.points.len())
                .sum::<usize>()
        };
        assert!(count(&fuzzy[1]) > 3 * count(&plain[1]));
    }
}
#[test]
fn settings_reject_invalid_finishing_and_resolve_orca_extras() {
    let mut c = Config::defaults();
    c.set_raw("ironing", "1").unwrap();
    c.set_raw("ironing_spacing", "0").unwrap();
    assert!(PrintSettings::from_config(&c).is_err());
    c.set_raw("ironing_spacing", "0.1").unwrap();
    c.set_raw("ironing_pattern", "concentric").unwrap();
    c.set_raw("ironing_angle", "23").unwrap();
    c.set_raw("ironing_inset", "0.6").unwrap();
    c.set_raw("solid_fill_pattern", "hilbertcurve").unwrap();
    let s = PrintSettings::from_config(&c).unwrap();
    assert_eq!(s.ironing.pattern, Pattern::Concentric);
    assert_eq!(s.ironing.angle, Some(23.));
    assert_eq!(s.ironing.inset, 0.6);
    assert_eq!(s.top_fill_pattern, Pattern::HilbertCurve);
    assert_eq!(s.bottom_fill_pattern, Pattern::HilbertCurve);
    assert_eq!(s.internal_solid_fill_pattern, Pattern::Ensuring);
    c.set_raw("fuzzy_skin", "external").unwrap();
    c.set_raw("fuzzy_skin_point_dist", "0").unwrap();
    assert!(PrintSettings::from_config(&c).is_err());
}

#[test]
fn every_surface_pattern_and_arachne_concentric_widths() {
    use mica_core::{fill, flow::Flow, geometry::ExPolygon};
    let mesh = cube();
    let mut s = PrintSettings::core_one_hf04_speed_pla();
    for name in [
        "rectilinear",
        "alignedrectilinear",
        "monotonic",
        "monotoniclines",
        "concentric",
        "hilbertcurve",
        "archimedeanchords",
        "octagramspiral",
    ] {
        let pattern = Pattern::from_config_name(name).unwrap();
        s.top_fill_pattern = pattern;
        s.bottom_fill_pattern = pattern;
        let obj = prepare_object(&mesh, &s).unwrap();
        assert!(
            obj.toolpaths(0..1)[0]
                .paths
                .iter()
                .any(|p| p.role == Role::SolidInfill),
            "bottom {name}"
        );
        assert!(
            obj.toolpaths(99..100)[0]
                .paths
                .iter()
                .any(|p| p.role == Role::TopSolidInfill),
            "top {name}"
        );
    }
    let area = vec![ExPolygon {
        contour: [(0., 0.), (15., 0.), (15., 3.1), (0., 1.3)]
            .into_iter()
            .map(|(x, y)| Point::from_mm(x, y))
            .collect(),
        holes: vec![],
    }];
    let flow = Flow::new(0.45, 0.2);
    let params = fill::FillParams {
        spacing: flow.spacing(),
        density: 1.,
        angle: 0.,
        layer_id: 1,
        z: 0.4,
        origin: Point::default(),
        anchor_length: 0.,
        anchor_length_max: 0.,
        dont_adjust: false,
        link_max_length: 0.,
    };
    s.perimeter_generator = PerimeterGenerator::Arachne;
    let paths = fill::toolpaths(
        Pattern::Concentric,
        &area,
        &params,
        flow,
        Role::TopSolidInfill,
        &s,
        None,
        Point::default(),
    );
    assert!(!paths.is_empty());
    let min = paths
        .iter()
        .map(|p| p.flow.width)
        .fold(f64::INFINITY, f64::min);
    let max = paths.iter().map(|p| p.flow.width).fold(0., f64::max);
    assert!(
        min > 0. && max.is_finite() && max - min > 0.03,
        "concentric must retain variable widths: {min}..{max}"
    );
    for path in paths {
        assert_eq!(path.role, Role::TopSolidInfill);
        assert!(mica_core::geometry::clip::diff_pl(&[path.points], &area).is_empty());
    }
}

#[test]
fn painting_hook_can_enable_fuzzy_on_an_unpainted_object() {
    use mica_core::fuzzy_skin::FuzzySkinPainting;
    struct Paint;
    impl FuzzySkinPainting for Paint {
        fn settings_at(&self, _: Point, _: usize) -> Option<FuzzySkinSettings> {
            Some(FuzzySkinSettings {
                mode: Mode::External,
                ..Default::default()
            })
        }
    }
    let mut poly = vec![
        Point::from_mm(0., 0.),
        Point::from_mm(20., 0.),
        Point::from_mm(20., 20.),
        Point::from_mm(0., 20.),
    ];
    fuzzy_polygon(&mut poly, &FuzzySkinSettings::default(), 1, 0, true, &Paint);
    assert!(poly.len() > 80);
}
