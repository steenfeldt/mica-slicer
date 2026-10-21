// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 tests/fff_print/test_cooling.cpp :: Cooling unit tests, Cooling integration tests
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/CoolingBuffer.cpp :: CoolingBuffer::calculate_layer_slowdown
// Source: orca@8500fcdcca src/libslic3r/GCode/CoolingBuffer.cpp :: CoolingBuffer::parse_layer_gcode
// Changes: Structured equivalents of upstream G-code blocks; additional mode, saturation, protected-tail and multi-tool regressions. Bridge fan integration is in tests/bridges.rs.
use super::*;

#[test]
fn equal_orca_fan_limits_stay_constant_across_layer_times() {
    let cfg = CoolingConfig {
        orca: true,
        fan_always_on: true,
        min_fan_speed: 100.,
        max_fan_speed: 100.,
        slowdown_below_layer_time: 5.,
        fan_below_layer_time: 100.,
        ..CoolingConfig::default()
    };
    for tick in 0..11000 {
        assert_eq!(fan_speed(2, tick as f64 / 100., &cfg), 100.);
    }
    assert_eq!(fan_speed(0, 10., &cfg), 0.);
}

/// Prusament PLA @MINIIS / @MK3.5: min = max = 100 %, fan below 100 s, slow down below 12 s.
/// PrusaSlicer's float t keeps every layer time at 100 % (no M106 S252.45).
#[test]
fn equal_prusa_fan_limits_stay_constant_across_layer_times() {
    let cfg = CoolingConfig {
        fan_always_on: true,
        min_fan_speed: 100.,
        max_fan_speed: 100.,
        slowdown_below_layer_time: 12.,
        fan_below_layer_time: 100.,
        disable_fan_first_layers: 1,
        full_fan_speed_layer: 4,
        ..CoolingConfig::default()
    };
    for tick in 0..11000 {
        assert_eq!(fan_speed(5, tick as f64 / 100. + 0.0037, &cfg), 100.);
    }
}

fn movement(length_mm: f64, speed: f64) -> Move {
    Move {
        length_mm,
        speed,
        adjustable: true,
        ..Move::default()
    }
}
fn config() -> CoolingConfig {
    CoolingConfig {
        disable_fan_first_layers: 0,
        ..CoolingConfig::default()
    }
}
fn wall(length: f64, speed: f64, index: usize) -> Move {
    Move {
        external: index == 0,
        perimeter_index: Some(index),
        ..movement(length, speed)
    }
}
#[test]
fn upstream_blocks_threshold_travel_and_extruder_only_time() {
    let mut c = config();
    c.slowdown_below_layer_time = 1.;
    let mut moves = [movement(100., 50.)];
    assert_eq!(slow_down(&mut moves, 0., &c), 2.);
    assert_eq!(moves[0].speed, 50.);
    c.slowdown_below_layer_time = 3.;
    // G1 X50 F2500; 50 mm extrusion at F3000; E-only 4 mm at F400.
    let fixed = 50. / (2500. / 60.) + 4. / (400. / 60.);
    moves[0] = movement(50., 50.);
    assert!((slow_down(&mut moves, fixed, &c) - 3.003).abs() < 1e-8);
    assert!(moves[0].speed < 50.);
    assert!((50. / moves[0].speed + fixed - 3.003).abs() < 1e-8);
}
#[test]
fn upstream_fans_and_multiple_objects_at_same_or_different_z() {
    let mut c = config();
    c.slowdown_below_layer_time = 1.;
    c.fan_below_layer_time = 1.;
    assert_eq!(fan_speed(0, 2., &c), 0.);
    c.slowdown_below_layer_time = 3.;
    c.fan_below_layer_time = 2.;
    let mut both = [movement(100., 50.), movement(100., 50.)];
    assert_eq!(slow_down(&mut both, 0., &c), 4.);
    assert_eq!(both[0].speed, 50.);
    // Separate flushes never accumulate the previous layer's time.
    for layer in 0..2 {
        assert_eq!(fan_speed(layer, 4., &c), 0.);
    }
    c.slowdown_below_layer_time = 5.;
    c.fan_below_layer_time = 5.;
    for layer in 0..2 {
        assert_eq!(fan_speed(layer, 4., &c), 100.);
    }
    // Upstream two-tool fan case: same layer time, cooling disabled on T1.
    c.fan_below_layer_time = 5.;
    c.slowdown_below_layer_time = 6.;
    let off = CoolingConfig {
        enabled: false,
        ..c
    };
    assert_eq!(fan_speed(0, 4., &c), 100.);
    assert_eq!(fan_speed(0, 4., &off), 0.);
}
#[test]
fn uniform_equalizes_fast_moves_but_never_accelerates_slower_moves() {
    let c = CoolingConfig {
        slowdown_below_layer_time: 5. / 1.001,
        ..config()
    };
    let mut m = [
        movement(100., 100.),
        movement(100., 50.),
        movement(20., 10.),
    ];
    let t = slow_down(&mut m, 0., &c);
    assert_eq!(t, 5.); // Already at the target.
    assert_eq!(m[0].speed, 100.);
    let c = CoolingConfig {
        slowdown_below_layer_time: 6. / 1.001,
        ..c
    };
    assert!((slow_down(&mut m, 0., &c) - 6.).abs() < 1e-8);
    assert!((m[0].speed - 50.).abs() < 1e-8);
    assert_eq!(m[1].speed, 50.);
    assert_eq!(m[2].speed, 10.);
}
#[test]
fn consistent_surface_protects_first_inner_but_not_deeper_walls() {
    let c = CoolingConfig {
        logic: SlowdownLogic::ConsistentSurface,
        slowdown_below_layer_time: 8.,
        min_print_speed: 20.,
        ..config()
    };
    let mut m = [
        wall(80., 200., 0),
        wall(75., 250., 1),
        wall(150., 250., 2),
        movement(60., 300.),
    ];
    assert!((slow_down(&mut m, 0., &c) - 8.008).abs() < 1e-8);
    assert_eq!(m[0].speed, 200.);
    assert_eq!(m[1].speed, 250.);
    assert!((m[2].speed - m[3].speed).abs() < 1e-8);
    assert!(m[2].speed > 20. && m[2].speed < 40.);
    let mut short = [wall(20., 200., 0), wall(20., 250., 1), movement(20., 300.)];
    assert_eq!(slow_down(&mut short, 0., &c), 3.);
    assert!(short.iter().all(|m| (m.speed - 20.).abs() < 1e-8));
}
#[test]
fn transition_walks_backwards_across_blocks_skips_bridges_and_minimum_speed() {
    let c = CoolingConfig {
        logic: SlowdownLogic::ConsistentSurface,
        perimeter_transition_distance: 10.,
        slowdown_below_layer_time: 8.,
        min_print_speed: 20.,
        ..config()
    };
    let mut m = [
        movement(100., 100.),
        movement(7., 80.),
        wall(50., 200., 0),
        Move {
            adjustable: false,
            ..movement(30., 50.)
        },
        movement(4., 20.),
    ];
    slow_down(&mut m, 0., &c);
    assert_eq!(m[0].non_adjustable_length, 3.);
    assert_eq!(m[1].speed, 80.);
    assert_eq!(m[3].speed, 50.);
    assert_eq!(m[4].speed, 20.);
    assert_eq!(m[0].speed, 20.);
    let expected = 8.008;
    let mut same = [
        movement(100., 100.),
        movement(7., 80.),
        wall(50., 200., 0),
        Move {
            adjustable: false,
            ..movement(30., 50.)
        },
        movement(4., 20.),
    ];
    assert!((slow_down(&mut same, 0., &c) - expected).abs() < 1e-8);
}
#[test]
fn zero_minimum_empty_fixed_only_and_transition_larger_than_layer() {
    for logic in [
        SlowdownLogic::Proportional,
        SlowdownLogic::UniformCooling,
        SlowdownLogic::ConsistentSurface,
    ] {
        let c = CoolingConfig {
            min_print_speed: 0.,
            slowdown_below_layer_time: 10.,
            logic,
            ..config()
        };
        let mut m = [movement(1., 50.)];
        assert!((slow_down(&mut m, 0., &c) - 10.01).abs() < 1e-8);
        assert!(m[0].speed > 0. && m[0].speed.is_finite());
        assert_eq!(slow_down(&mut [], 1., &c), 1.);
        let mut fixed = [
            Move {
                adjustable: false,
                ..movement(1., 50.)
            },
            movement(0., 50.),
        ];
        assert_eq!(slow_down(&mut fixed, 1., &c), 1.02);
    }
    let c = CoolingConfig {
        logic: SlowdownLogic::ConsistentSurface,
        perimeter_transition_distance: 1000.,
        ..config()
    };
    let mut m = [movement(50., 100.)];
    assert_eq!(slow_down(&mut m, 0., &c), 0.5);
    assert_eq!(m[0].speed, 100.);
}
#[test]
fn per_tool_threshold_order_disabled_tool_and_nonconsecutive_tools() {
    let base = CoolingConfig {
        min_print_speed: 10.,
        slowdown_below_layer_time: 4. / 1.001,
        ..config()
    };
    let configs = [
        base,
        CoolingConfig {
            enabled: false,
            ..base
        },
        CoolingConfig {
            slowdown_below_layer_time: 8. / 1.001,
            ..base
        },
    ];
    let mut m = [
        movement(100., 100.),
        Move {
            tool: 2,
            ..movement(100., 100.)
        },
        Move {
            tool: 1,
            ..movement(100., 100.)
        },
    ];
    assert!((slow_down_multi(&mut m, 0., &configs) - 8.).abs() < 1e-8);
    // T0 and T2 first share 4s minus the 1s fixed tool. T0 is then finalized at 1.5s.
    assert!((m[0].speed - 100. / 1.5).abs() < 1e-8);
    assert!((m[1].speed - 100. / 5.5).abs() < 1e-8);
    assert_eq!(m[2].speed, 100.);
    // Physical source order does not affect equalization, nor does revisiting a tool.
    let mut reversed = [
        m[2],
        movement(50., 100.),
        Move {
            tool: 2,
            ..movement(100., 100.)
        },
        movement(50., 100.),
    ];
    assert!((slow_down_multi(&mut reversed, 0., &configs) - 8.).abs() < 1e-8);
    assert!((reversed[1].speed - m[0].speed).abs() < 1e-8);
}
#[test]
fn different_material_minimums_saturate_independently() {
    let c = CoolingConfig {
        slowdown_below_layer_time: 8. / 1.001,
        min_print_speed: 40.,
        ..config()
    };
    let configs = [
        c,
        CoolingConfig {
            min_print_speed: 10.,
            ..c
        },
    ];
    let mut m = [
        movement(100., 100.),
        Move {
            tool: 1,
            ..movement(100., 80.)
        },
    ];
    assert!((slow_down_multi(&mut m, 0., &configs) - 8.).abs() < 1e-8);
    assert_eq!(m[0].speed, 40.);
    assert!((m[1].speed - 100. / 5.5).abs() < 1e-8);
}
#[test]
fn orca_outer_wall_exclusion_is_absolute_and_inner_wall_not_protected() {
    let c = CoolingConfig {
        orca: true,
        dont_slow_down_outer_wall: true,
        slowdown_below_layer_time: 10.,
        min_print_speed: 20.,
        ..config()
    };
    let mut m = [wall(20., 200., 0), wall(20., 250., 1), movement(20., 300.)];
    assert!((slow_down(&mut m, 0., &c) - 2.1).abs() < 1e-8);
    assert_eq!(m[0].speed, 200.);
    assert_eq!(m[1].speed, 20.);
    assert_eq!(m[2].speed, 20.);
    let c = CoolingConfig {
        dont_slow_down_outer_wall: false,
        ..c
    };
    m = [wall(20., 200., 0), wall(20., 250., 1), movement(20., 300.)];
    assert_eq!(slow_down(&mut m, 0., &c), 3.);
    assert_eq!(m[0].speed, 20.);
    // Orca still interpolates fans with slowdown disabled, and permits a first-layer ramp.
    let c = CoolingConfig {
        enabled: false,
        disable_fan_first_layers: 0,
        full_fan_speed_layer: 4,
        ..c
    };
    assert_eq!(fan_speed(0, 1., &c), 25.);
}

#[test]
fn fan_ramp_and_proportional_mode_match_upstream() {
    let c = CoolingConfig {
        logic: SlowdownLogic::Proportional,
        slowdown_below_layer_time: 8.,
        min_print_speed: 10.,
        min_fan_speed: 85.,
        max_fan_speed: 100.,
        fan_below_layer_time: 40.,
        fan_always_on: true,
        disable_fan_first_layers: 1,
        full_fan_speed_layer: 3,
        ..config()
    };
    assert_eq!(fan_speed(0, 8., &c), 0.);
    assert_eq!(fan_speed(1, 8., &c), 50.);
    assert_eq!(fan_speed(2, 8., &c), 100.);
    assert_eq!(fan_speed(10, 100., &c), 85.);
    assert_eq!(fan_speed(10, 24., &c), 92.);
    let mut m = [
        movement(100., 100.),
        movement(100., 200.),
        wall(80., 200., 0),
    ];
    assert!((slow_down(&mut m, 0., &c) - 8.008).abs() < 1e-8);
    assert!((m[0].speed * 2. - m[1].speed).abs() < 1e-8);
    assert_eq!(m[2].speed, 200.);
}

#[test]
fn unused_extruders_do_not_select_the_layers_slowdown_logic() {
    let unused = CoolingConfig {
        logic: SlowdownLogic::UniformCooling,
        slowdown_below_layer_time: 7.,
        ..config()
    };
    let used = CoolingConfig {
        logic: SlowdownLogic::ConsistentSurface,
        slowdown_below_layer_time: 8.,
        ..config()
    };
    let mut m = [
        Move {
            tool: 2,
            ..wall(80., 200., 0)
        },
        Move {
            tool: 2,
            ..movement(160., 250.)
        },
    ];
    slow_down_multi(&mut m, 0., &[unused, unused, used]);
    assert_eq!(m[0].speed, 200.);
}

#[test]
fn protected_tail_emission_preserves_linear_and_arc_extrusion() {
    use crate::{overhang_plan::LayerPlan, smooth_path::ArcFitting, writer::Writer};
    use mica_core::{
        flow::Flow,
        geometry::{Point, SCALE, arc_welder as arc},
        settings::PrintSettings,
        toolpath::{ExtrusionPath, LayerToolpaths, Role},
    };
    for mode in [
        ArcFitting::Disabled,
        ArcFitting::EmitCenter,
        ArcFitting::EmitRadius,
    ] {
        let points = if mode == ArcFitting::Disabled {
            vec![
                Point::from_mm(0., 0.),
                Point::from_mm(20., 0.),
                Point::from_mm(40., 0.),
            ]
        } else {
            arc::arc_discretize(
                Point::from_mm(20., 0.),
                Point::from_mm(0., 20.),
                20. * SCALE,
                true,
                0.001 * SCALE,
            )
        };
        let mut config = Config::defaults();
        for (k, v) in [
            ("infill_speed", "100"),
            ("filament_max_volumetric_speed", "0"),
            ("max_volumetric_speed", "0"),
        ] {
            config.set_raw(k, v).unwrap();
        }
        let settings = PrintSettings::from_config(&config).unwrap();
        let layer = LayerToolpaths {
            support_groups: Vec::new(),
            index: 2,
            print_z: 0.6,
            height: 0.2,
            paths: vec![ExtrusionPath {
                role: Role::InternalInfill,
                perimeter_index: None,
                points,
                flow: Flow::new(0.45, 0.2),
                seam: None,
                overhang: None,
                brick: None,
            }],
        };
        let mut plan = LayerPlan::new(
            &layer,
            |_| (&config, &settings, 0),
            crate::overhang_plan::LayerCurves::new(
                &layer,
                |_| (&config, &settings, 0),
                0.005 * SCALE,
                mode,
            ),
            &mut None,
        );
        let cooling = CoolingConfig {
            logic: SlowdownLogic::ConsistentSurface,
            perimeter_transition_distance: 10.,
            min_print_speed: 2.,
            slowdown_below_layer_time: 5.,
            ..self::config()
        };
        let nominal = plan.moves[0].speed;
        let length = plan.moves[0].length_mm;
        let time = slow_down(&mut plan.moves, 0., &cooling);
        assert!((time - 5.005).abs() < 1e-8);
        assert_eq!(plan.moves[0].non_adjustable_length, 10.);
        let mut w = Writer::new();
        w.travel_to(
            layer.paths[0].points[0].x as f64 / SCALE,
            layer.paths[0].points[0].y as f64 / SCALE,
            100.,
        );
        w.move_z(0.6, 10.);
        plan.emit(
            &mut w,
            0,
            &layer.paths[0],
            &layer,
            &cooling,
            time,
            100.,
            0.1,
            10.,
            mode,
            false,
        );
        assert!(
            (w.stats.filament_mm - length * 0.1).abs() < 0.002,
            "{:?}",
            w.stats
        );
        assert!(w.as_str().contains(&format!("F{}", nominal * 60.)));
        if mode != ArcFitting::Disabled {
            assert!(w.as_str().matches("G3 ").count() >= 2, "{}", w.as_str());
        }
    }
}

#[test]
fn orca_ignores_motion_before_the_first_extrude_marker() {
    let c = CoolingConfig {
        orca: true,
        slowdown_below_layer_time: 4. / 1.001,
        min_print_speed: 0.,
        ..config()
    };
    let mut moves = [
        Move {
            adjustable: false,
            ..movement(100., 50.)
        },
        movement(100., 100.),
        Move {
            adjustable: false,
            ..movement(50., 50.)
        },
    ];
    assert!((slow_down(&mut moves, 0., &c) - 4.).abs() < 1e-8);
    assert_eq!(moves[0].speed, 50.);
    assert!((moves[1].speed - 100. / 3.).abs() < 1e-8);
}

#[test]
fn interleaved_tools_keep_independent_transition_distances() {
    let c = CoolingConfig {
        logic: SlowdownLogic::ConsistentSurface,
        min_print_speed: 0.,
        slowdown_below_layer_time: 10.,
        perimeter_transition_distance: 5.,
        ..config()
    };
    let other = CoolingConfig {
        perimeter_transition_distance: 15.,
        ..c
    };
    let mut moves = [
        movement(100., 100.),
        Move {
            tool: 1,
            ..movement(100., 100.)
        },
        movement(10., 100.),
    ];
    assert!((slow_down_multi(&mut moves, 0., &[c, other]) - 10.01).abs() < 1e-8);
    assert_eq!(moves[0].non_adjustable_length, 0.);
    assert_eq!(moves[1].non_adjustable_length, 15.);
    assert_eq!(moves[2].non_adjustable_length, 5.);
}

#[test]
fn numeric_two_tool_oracle_from_pinned_cpp_adjustment_functions() {
    // Recorded from the unmodified pinned PerExtruderAdjustments / extruder_range
    // functions, compiled with clang++ -std=c++17 -DNDEBUG. Each tool has 40/60/80/100
    // mm at 100/150/200/250 mm/s. First two blocks are external / first-inner walls.
    let expected = [
        [
            100., 114.15, 114.15, 114.15, 43.0339, 43.0339, 43.0339, 43.0339,
        ],
        [100., 150., 102.256, 102.256, 100., 150., 30.0009, 30.0009],
        [100., 150., 93.9794, 117.474, 100., 150., 28.0382, 35.0478],
        [100., 150., 61.4203, 76.7754, 100., 150., 200., 250.],
    ];
    for (case, expected) in expected.iter().enumerate() {
        let logic = match case {
            0 => SlowdownLogic::UniformCooling,
            1 => SlowdownLogic::ConsistentSurface,
            _ => SlowdownLogic::Proportional,
        };
        let c = CoolingConfig {
            logic,
            perimeter_transition_distance: 10.,
            min_print_speed: 20.,
            slowdown_below_layer_time: 5.,
            ..config()
        };
        let configs = [
            c,
            CoolingConfig {
                min_print_speed: 10.,
                slowdown_below_layer_time: 9.,
                enabled: case != 3,
                ..c
            },
        ];
        let mut moves: Vec<_> = (0..8)
            .map(|i| {
                let j = i % 4;
                Move {
                    tool: i / 4,
                    external: j == 0,
                    perimeter_index: (j < 2).then_some(j),
                    ..movement(40. + j as f64 * 20., 100. + j as f64 * 50.)
                }
            })
            .collect();
        let time = slow_down_multi(&mut moves, 0., &configs);
        assert!((time - if case == 3 { 5.005 } else { 9.009 }).abs() < 0.0001);
        for (m, &speed) in moves.iter().zip(expected) {
            assert!(
                (m.speed - speed).abs() < 0.001,
                "case {case}: {} vs {speed}",
                m.speed
            );
        }
    }
}
