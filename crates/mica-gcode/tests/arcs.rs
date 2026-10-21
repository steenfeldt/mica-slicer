// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original
use mica_core::{
    config::Config,
    flow::Flow,
    geometry::{
        Point, SCALE,
        arc_welder::{self as arc, Segment},
    },
    print::SlicedObject,
    settings::PrintSettings,
    toolpath::{ExtrusionPath, LayerToolpaths, Role},
};
use mica_gcode::{
    bgcode,
    export::{export_file, export_gcode},
    smooth_path::{self, ArcFitting, SmoothPathCache},
    time_estimate::{self, Analysis},
    writer::Writer,
};
use std::io::Cursor;

fn config() -> Config {
    let mut c = Config::defaults();
    for line in
        include_str!("../../mica-profiles/tests/fixtures/coreone_hf04_speed_pla.ini").lines()
    {
        if let Some((k, v)) = line.split_once(" = ") {
            c.set_raw(k, v).unwrap();
        }
    }
    c
}
fn circular_path() -> ExtrusionPath {
    ExtrusionPath {
        role: Role::ExternalPerimeter,
        flow: Flow::new(0.45, 0.2),
        overhang: None,
        perimeter_index: None,
        seam: None,
        points: arc::arc_discretize(
            Point::from_mm(135., 110.),
            Point::from_mm(125., 120.),
            10. * SCALE,
            true,
            0.001 * SCALE,
        ),
        brick: None,
    }
}
#[test]
fn settings_guards_and_layer_cache_seam() {
    let mut c = config();
    assert_eq!(ArcFitting::from_config(&c), ArcFitting::EmitCenter);
    for key in [
        "spiral_vase",
        "max_volumetric_extrusion_rate_slope_positive",
        "max_volumetric_extrusion_rate_slope_negative",
    ] {
        c.set_raw(key, "1").unwrap();
        assert_eq!(ArcFitting::from_config(&c), ArcFitting::Disabled);
        c.set_raw(key, "0").unwrap();
    }
    c.set_raw("arc_fitting", "emit_radius").unwrap();
    assert_eq!(ArcFitting::from_config(&c), ArcFitting::EmitRadius);
    c.set_raw("gcode_flavor", "klipper").unwrap();
    assert_eq!(ArcFitting::from_config(&c), ArcFitting::EmitCenter);
    let mut path = circular_path();
    path.points.extend(
        arc::arc_discretize(
            *path.points.last().unwrap(),
            path.points[0],
            -10. * SCALE,
            true,
            0.001 * SCALE,
        )
        .into_iter()
        .skip(1),
    );
    let cache = SmoothPathCache::new(&[path.clone()], 0.01 * SCALE, ArcFitting::EmitCenter);
    let split = cache.resolve_or_fit_split_with_seam(
        0,
        &path.points,
        false,
        0.01 * SCALE,
        (Point::from_mm(125., 100.), 1.),
    );
    assert_eq!(split[0].point, split.last().unwrap().point);
    assert!((arc::path_length(&split) - arc::path_length(cache.get(0))).abs() < 10.);
}
#[test]
fn emitted_arcs_use_analytic_extrusion_and_estimate_like_radius_form() {
    for radius in [10. * SCALE, -10. * SCALE] {
        for ccw in [true, false] {
            let path = vec![
                Segment::line(Point::from_mm(10., 0.)),
                Segment {
                    point: Point::from_mm(0., 10.),
                    radius,
                    ccw,
                },
            ];
            let mut times = Vec::new();
            for mode in [ArcFitting::EmitCenter, ArcFitting::EmitRadius] {
                let mut w = Writer::new();
                w.raw("G90\nM83\nG92 X10 Y0 E0\n;LAYER_CHANGE\n;TYPE:External perimeter");
                w.travel_to(10., 0., 100.);
                smooth_path::emit(&mut w, &path, 0.04, 50., mode);
                let length = arc::path_length(&path) / SCALE;
                assert!((w.stats.filament_mm - length * 0.04).abs() < 1e-9);
                let g = w.as_str();
                assert!(g.contains(if ccw { "G3 X" } else { "G2 X" }));
                let mut src = Cursor::new(g.as_bytes());
                let mut analysis = Analysis::new(&mut src, &config()).unwrap();
                let report = analysis.breakdown(&mut src, &config()).unwrap();
                assert_eq!(report.total.arcs, 1);
                assert!((report.total.seconds - analysis.estimate.normal).abs() < 1e-8);
                assert!((analysis.estimate.normal - length / 50.).abs() < 0.15);
                times.push(analysis.estimate.normal);
            }
            assert!((times[0] - times[1]).abs() < 0.002, "{times:?}");
        }
    }
    // Rounded endpoints can be farther apart than twice the original radius.
    let path = vec![
        Segment::line(Point::from_mm(0., 0.)),
        Segment {
            point: Point::from_mm(2.0004, 0.0004),
            radius: 1.00021 * SCALE,
            ccw: true,
        },
    ];
    let mut w = Writer::new();
    w.travel_to(0., 0., 50.);
    smooth_path::emit(&mut w, &path, 0.04, 50., ArcFitting::EmitRadius);
    assert!(w.as_str().contains("G3"));
}
#[test]
fn arc_fitted_binary_export_survives_meatpack() {
    let c = config();
    let s = PrintSettings::from_config(&c).unwrap();
    let obj = SlicedObject {
        adhesion: Default::default(),
        outlines: Vec::new(),
        layers: vec![LayerToolpaths {
            support_groups: Vec::new(),
            index: 0,
            print_z: 0.2,
            height: 0.2,
            paths: vec![circular_path()],
        }],
        first_layer_bbox: Some(mica_core::geometry::BoundingBox {
            min: Point::from_mm(115., 100.),
            max: Point::from_mm(135., 120.),
        }),
        report: Default::default(),
    };
    let text = export_gcode(&obj, &c, &s, "arc-test").unwrap();
    let arc_lines: Vec<_> = text
        .gcode
        .lines()
        .filter(|l| l.starts_with("G2 ") || l.starts_with("G3 "))
        .collect();
    assert!(!arc_lines.is_empty());
    let path =
        std::env::temp_dir().join(format!("mica-arc-roundtrip-{}.bgcode", std::process::id()));
    export_file(&obj, &c, &s, "arc-test", None, &path).unwrap();
    let decoded = bgcode::decode(&std::fs::read(&path).unwrap()).unwrap();
    std::fs::remove_file(path).unwrap();
    let actual: Vec<_> = decoded
        .gcode
        .lines()
        .filter(|l| l.starts_with("G2 ") || l.starts_with("G3 "))
        .collect();
    assert_eq!(actual, arc_lines);
    assert_eq!(
        time_estimate::estimate(&mut Cursor::new(decoded.gcode.as_bytes()), &c)
            .unwrap()
            .normal,
        text.stats.time_s
    );
}
