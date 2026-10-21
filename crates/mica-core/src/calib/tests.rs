// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: orca@8500fcdcca src/libslic3r/GCode.cpp :: GCode::process_layer, GCode::interpolate_value_across_layers
// Source: orca@8500fcdcca src/libslic3r/calib.cpp :: CalibPressureAdvanceLine::print_pa_lines, CalibPressureAdvancePattern::generate_custom_gcodes
// Source: orca@8500fcdcca resources/calib/filament_flow/flowrate-test-pass1.3mf :: object names
// Source: orca@8500fcdcca resources/calib/filament_flow/flowrate-test-pass2.3mf :: object names
// Changes: Source-oracle sequences and geometry invariants tested without GUI/assets.
use super::*;

#[test]
fn flow_passes_match_orca_asset_names() {
    let c = Config::defaults();
    let mut p = Parameters::new(Kind::FlowRate);
    let g = generate(&p, &c).unwrap();
    assert_eq!(
        g.samples,
        vec![-20., -15., -10., -5., 0., 5., 10., 15., 20.]
    );
    assert_eq!(g.objects[0].flow_ratio, 0.8);
    p.pass = 2;
    let g = generate(&p, &c).unwrap();
    assert_eq!(
        g.samples,
        vec![-9., -8., -7., -6., -5., -4., -3., -2., -1., 0.]
    );
    assert_eq!(g.objects.last().unwrap().flow_ratio, 1.);
}
#[test]
fn pa_paths_have_orca_values_and_slow_fast_slow() {
    let c = Config::defaults();
    let mut p = Parameters::new(Kind::PressureAdvanceLine);
    p.end = 0.01;
    p.speed = Some(100.);
    let g = generate(&p, &c).unwrap();
    let print = g.prepare(&c).unwrap();
    let controls = &print.controls[0];
    assert_eq!(
        controls.iter().filter_map(|c| c.sample).collect::<Vec<_>>(),
        vec![0., 0.005, 0.01]
    );
    assert_eq!(controls[1].speed_mm_s, Some(10.));
    assert_eq!(controls[2].speed_mm_s, Some(100.));
    assert_eq!(controls[3].speed_mm_s, Some(10.));
    assert_eq!(
        print.layers[0].paths[1].points[0].distance_to(print.layers[0].paths[1].points[1])
            / crate::geometry::SCALE,
        20.
    );
    p.kind = Kind::PressureAdvancePattern;
    let print = generate(&p, &c).unwrap().prepare(&c).unwrap();
    assert_eq!(print.layer_count(), 4);
    for controls in print.controls {
        assert_eq!(
            controls.iter().filter_map(|c| c.sample).collect::<Vec<_>>(),
            vec![0., 0.005, 0.01]
        );
        assert_eq!(controls.iter().filter(|c| c.sample.is_some()).count(), 3);
    }
}
#[test]
fn tower_source_oracles_at_boundaries() {
    let c = Config::defaults();
    let p = Parameters::new(Kind::PressureAdvanceTower);
    let g = generate(&p, &c).unwrap();
    assert_eq!(g.control(0.8, 3, 100).pressure_advance, Some(0.));
    assert_eq!(g.control(1., 4, 100).pressure_advance, Some(0.002));
    let p = Parameters::new(Kind::Retraction);
    let g = generate(&p, &c).unwrap();
    for (z, value) in [(0.4, 0.), (1.2, 0.), (1.4, 0.), (1.6, 0.1), (2.4, 0.2)] {
        assert!((g.control(z, 0, 100).retract_length.unwrap() - value).abs() < 1e-9);
    }
    let mut p = Parameters::new(Kind::Temperature);
    p.end = 220.;
    let g = generate(&p, &c).unwrap();
    // 150 layers at 0.2 mm: the f32 interpolation/truncation from GCode.cpp.
    for (i, t) in [
        (0, 230.),
        (1, 230.),
        (2, 230.),
        (49, 230.),
        (50, 225.),
        (99, 225.),
        (100, 220.),
        (149, 220.),
    ] {
        assert_eq!(g.control((i + 1) as f64 * 0.2, i, 150).temperature, Some(t));
    }
    let p = Parameters::new(Kind::MaxVolumetricSpeed);
    let g = generate(&p, &c).unwrap();
    let area = Flow::new(0.7, 0.32).mm3_per_mm();
    assert_eq!(
        g.control(10., 0, 100).speed_mm_s,
        Some((10. / area).round())
    );
}
#[test]
fn invalid_ranges_fail_before_allocating_models() {
    let c = Config::defaults();
    for (start, end, step) in [
        (0., 1., 0.),
        (0., 1., -1.),
        (f64::NAN, 1., 0.1),
        (0., f64::INFINITY, 0.1),
        (1., 0., 0.1),
        (0., 1., 1e-12),
    ] {
        let mut p = Parameters::new(Kind::PressureAdvanceLine);
        p.start = start;
        p.end = end;
        p.step = step;
        assert!(generate(&p, &c).is_err());
    }
    let mut p = Parameters::new(Kind::FlowRate);
    p.pass = 3;
    assert!(generate(&p, &c).is_err());
    let mut p = Parameters::new(Kind::Temperature);
    p.step = 10.;
    assert!(generate(&p, &c).is_err());
}
#[test]
fn procedural_meshes_are_outward_and_on_bed() {
    let c = Config::defaults();
    for kind in [
        Kind::FlowRate,
        Kind::PressureAdvanceLine,
        Kind::PressureAdvancePattern,
        Kind::PressureAdvanceTower,
        Kind::Temperature,
        Kind::Retraction,
        Kind::MaxVolumetricSpeed,
    ] {
        let g = generate(&Parameters::new(kind), &c).unwrap();
        for o in g.objects {
            assert!(o.mesh.volume() > 0.);
            assert_eq!(o.mesh.bounds().0[2], 0.);
        }
    }
}

#[test]
fn source_options_are_overridden_in_both_typed_and_raw_forms() {
    use crate::config::{OptType, parse_value};
    let mut base = Config::defaults();
    base.set(
        "nozzle_temperature_initial_layer",
        parse_value(OptType::Ints, "210,210,210,210").unwrap(),
    );
    base.set(
        "nozzle_temperature",
        parse_value(OptType::Ints, "210,210,210,210").unwrap(),
    );
    base.set(
        "thick_internal_bridges",
        parse_value(OptType::Bool, "1").unwrap(),
    );
    let g = generate(&Parameters::new(Kind::Temperature), &base).unwrap();
    let c = g.config(&base).unwrap();
    assert_eq!(c.float("first_layer_temperature"), Some(230.));
    assert_eq!(c.float("nozzle_temperature_initial_layer"), Some(230.));
    assert_eq!(c.float("nozzle_temperature"), Some(230.));
    let g = generate(&Parameters::new(Kind::FlowRate), &base).unwrap();
    assert!(
        !PrintSettings::from_config(&g.config(&base).unwrap())
            .unwrap()
            .thick_internal_bridges
    );
}

#[test]
fn temperature_respects_selected_filament_limits() {
    let mut c = Config::defaults();
    c.set_raw("temperature", "230").unwrap();
    c.set_raw("first_layer_temperature", "230").unwrap();
    let mut p = Parameters::new(Kind::Temperature);
    assert!(generate(&p, &c).is_ok());
    p.end = 190.;
    assert!(generate(&p, &c).unwrap_err().contains("filament limits"));
}

/// On a multi-tool printer the test is a plate whose objects are all assigned to the tested
/// tool: that tool is the first and only used one, per-tool overrides change its slot only,
/// and every path is scheduled for it. A single-tool printer keeps the plain source.
#[test]
fn multi_tool_tests_print_on_the_chosen_tool_only() {
    let mut c = Config::defaults();
    for (k, v) in [
        ("nozzle_diameter", "0.6,0.6,0.4"),
        ("temperature", "215,240,250"),
        ("first_layer_temperature", "215,240,250"),
        ("filament_max_volumetric_speed", "11,12,13"),
    ] {
        c.set_raw(k, v).unwrap();
    }
    let mut p = Parameters::new(Kind::Temperature);
    p.tool = 2;
    p.start = 255.;
    p.end = 245.;
    let g = generate(&p, &c).unwrap();
    // The tested tool's 0.4 mm nozzle sizes the layers (every tool must suit them).
    let height = &g
        .overrides
        .iter()
        .find(|(k, _)| k == "layer_height")
        .unwrap()
        .1;
    assert_eq!(height, "0.2");
    let config = g.config(&c).unwrap();
    assert_eq!(config.raw("temperature").as_deref(), Some("215,240,255"));
    assert_eq!(
        config.raw("first_layer_temperature").as_deref(),
        Some("215,240,255")
    );
    for kind in [
        Kind::Temperature,
        Kind::FlowRate,
        Kind::PressureAdvanceLine,
        Kind::MaxVolumetricSpeed,
    ] {
        let mut p = if kind == Kind::Temperature {
            p.clone()
        } else {
            Parameters::new(kind)
        };
        p.tool = 2;
        let print = generate(&p, &c).unwrap().prepare(&c).unwrap();
        assert_eq!(print.tool(), 2);
        assert_eq!(print.settings.nozzle_diameter, 0.4);
        let job = print.print_job().expect("multi-tool plan");
        assert_eq!(job.ordering.used, vec![2], "{kind:?}");
        assert_eq!(job.ordering.first, 2);
        assert_eq!(job.ordering.changes, 0);
        assert!(job.object_first_layer_bbox().is_some());
        let n = print.layer_count();
        for layer in print.scheduled_toolpaths(0..n) {
            assert_eq!(layer.assignments.len(), layer.layer.paths.len());
            assert!(
                layer
                    .assignments
                    .iter()
                    .all(|a| a.tool == 2 && a.object < job.objects.len())
            );
        }
    }
    let mut p = Parameters::new(Kind::MaxVolumetricSpeed);
    p.tool = 2;
    let config = generate(&p, &c).unwrap().config(&c).unwrap();
    assert_eq!(
        config.raw("filament_max_volumetric_speed").as_deref(),
        Some("11,12,0")
    );
    // Tools beyond the printer's are refused; one tool keeps the single-object source.
    p.tool = 3;
    assert!(generate(&p, &c).unwrap_err().contains("tool 4"));
    let single = Config::defaults();
    let print = generate(&Parameters::new(Kind::PressureAdvanceTower), &single)
        .unwrap()
        .prepare(&single)
        .unwrap();
    assert!(print.print_job().is_none());
    assert!(print.scheduled_toolpaths(0..1)[0].assignments.is_empty());
}
