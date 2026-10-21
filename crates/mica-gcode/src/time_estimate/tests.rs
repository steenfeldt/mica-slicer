// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

use super::*;
use std::io::{Cursor, sink};

fn config() -> Config {
    let mut c = Config::defaults();
    for (k, v) in [
        ("gcode_flavor", "marlin2"),
        ("machine_limits_usage", "emit_to_gcode"),
        ("machine_max_acceleration_extruding", "100,50"),
        ("machine_max_acceleration_retracting", "100,50"),
        ("machine_max_acceleration_travel", "100,50"),
        ("machine_max_junction_deviation", "0,0"),
        ("silent_mode", "0"),
    ] {
        c.set_raw(k, v).unwrap();
    }
    for a in ['x', 'y', 'z', 'e'] {
        c.set_raw(&format!("machine_max_acceleration_{a}"), "100,50")
            .unwrap();
        c.set_raw(&format!("machine_max_feedrate_{a}"), "100,100")
            .unwrap();
        c.set_raw(&format!("machine_max_jerk_{a}"), "0,0").unwrap();
    }
    c
}
fn time(g: &str) -> f64 {
    estimate(&mut Cursor::new(g), &config()).unwrap().normal
}
fn close(a: f64, b: f64) {
    assert!((a - b).abs() < 1e-5, "{a} != {b}");
}

#[test]
fn analytic_trapezoid_triangle_and_junction_lookahead() {
    close(time("G1 X10 F600\n"), 1.1);
    close(time("G1 X.1 F600\n"), 2.0 * (0.1f64 / 100.0).sqrt());
    // Splitting a collinear path must not introduce an extra acceleration/deceleration pair.
    let mut c = config();
    for a in ['x', 'y', 'z', 'e'] {
        c.set_raw(&format!("machine_max_jerk_{a}"), "1,1").unwrap();
    }
    let split = estimate(&mut Cursor::new("G1 X10 F600\nG1 X20\n"), &c)
        .unwrap()
        .normal;
    let straight = estimate(&mut Cursor::new("G1 X20 F600\n"), &c)
        .unwrap()
        .normal;
    close(split, straight);
    assert!(time("G1 X10 F600\nG1 X0\n") > straight);
}

#[test]
fn coordinates_flow_feed_override_and_units() {
    let absolute = "M82\nG1 X10 E1 F600\nG92 X100 E0\nG1 X110 E1\n";
    let relative = "G91\nM82\nG1 X10 E1 F600\nG1 X10 E1\n";
    close(time(absolute), time(relative));
    close(time("G20\nG1 X1 F600\n"), time("G21\nG1 X25.4 F600\n"));
    close(
        time("M220 B S50\nG1 X10 F1200\nM220 R\nG1 X20 F600\n"),
        time("G1 X10 F600\nG1 X20 F600\n"),
    );
    close(time("G1 X10 F600\nG28 X\n"), time("G1 X10 F600\nG1 X0\n"));
    assert!(time("M203 E2\nM221 S200\nG1 E10 F600\n") > time("M203 E2\nG1 E10 F600\n"));
    close(time("M221 S200 T1\nG1 E10 F600\n"), time("G1 E10 F600\n"));
}

#[test]
fn waits_and_pressure_advance_match_upstream_scope() {
    close(
        time("G1 X10 F600\nM104 S200\nM109 S200\nM140 S60\nM190 R60\nG4 S120\nG10\nG11\nG1 X20\n"),
        time("G1 X10 F600\nG1 X20\n"),
    );
    let mut c = config();
    c.set_raw("gcode_flavor", "klipper").unwrap();
    let mut e = Estimator::new(&c);
    e.process_line(
        "SET_PRESSURE_ADVANCE EXTRUDER=extruder ADVANCE = 0.035 ; ADVANCE=9",
        &mut sink(),
    )
    .unwrap();
    close(f64::from(e.pressure_advance), 0.035);
    e.process_line(
        "SET_VELOCITY_LIMIT ACCEL_TO_DECEL=1 ACCEL=42 VELOCITY = 8 SQUARE_CORNER_VELOCITY=2",
        &mut sink(),
    )
    .unwrap();
    assert_eq!(e.machines[0].acceleration, [42.0, 100.0, 42.0]);
    assert_eq!(&e.machines[0].axis_speed[..2], &[8.0, 8.0]);
    assert_eq!(&e.machines[0].jerk[..2], &[2.0, 2.0]);
}

#[test]
fn silent_envelope_limits_dynamic_commands_and_ignore() {
    let mut c = config();
    c.set_raw("silent_mode", "1").unwrap();
    let mut e = Estimator::new(&c);
    for line in ["M201 X99", "M203 X90", "M204 P80 T80", "M205 X9"] {
        e.process_line(line, &mut sink()).unwrap();
    }
    assert_eq!(e.machines[0].axis_accel[0], 99.0);
    assert_eq!(e.machines[1].axis_accel[0], 50.0);
    assert_eq!(e.machines[1].axis_speed[0], 100.0);
    e.process_line("G1 X1 F600", &mut sink()).unwrap();
    for line in [
        "M201 X25",
        "M203 X30",
        "M204 S70 T40",
        "M205 X4 Y3 S1 T2",
        "M566 E120",
    ] {
        e.process_line(line, &mut sink()).unwrap();
    }
    assert_eq!(e.machines[1].axis_accel[0], 25.0);
    assert_eq!(e.machines[1].acceleration, [50.0, 40.0, 50.0]);
    assert_eq!(e.machines[1].jerk, [4.0, 3.0, 0.0, 2.0]);
    assert_eq!(e.machines[1].min_rate, [1.0, 2.0]);
    c.set_raw("machine_limits_usage", "ignore").unwrap();
    let ignored = Estimator::new(&c);
    assert_eq!(ignored.machines[0].axis_accel[0], 9000.0);
}

#[test]
fn arcs_radius_relative_helix_and_explicit_feedrate() {
    let mut c = config();
    c.set_raw("gcode_flavor", "klipper").unwrap();
    let t = |g: &str| estimate(&mut Cursor::new(g), &c).unwrap().normal;
    let ij = t("G1 X10 F600\nG3 X0 Y10 I-10 J0 Z1 E1\n");
    close(ij, t("G1 X10 F600\nG3 X0 Y10 R10 Z1 E1\n"));
    close(ij, t("G1 X10 F600\nG91\nG3 X-10 Y10 I-10 J0 Z1 E1 F600\n"));
    assert!(t("G1 X10 F600\nG3 I-10 J0\n").is_finite());
    assert!(time("G1 X10 F600\nG3 X0 Y10 I-10 F600\n") > time("G1 X10 F600\nG3 X0 Y10 I-10\n"));
}

#[test]
fn first_layer_toolchange_and_bounded_queue() {
    let mut c = config();
    c.set_raw("filament_diameter", "1.75,1.75").unwrap();
    c.set_raw("printer_notes", "PRINTER_MODEL_XL").unwrap();
    let mut e = Estimator::new(&c);
    for line in [
        "G1 X10 F600",
        ";LAYER_CHANGE",
        "G1 X20",
        "T1",
        ";LAYER_CHANGE",
        "G1 X30",
    ] {
        e.process_line(line, &mut sink()).unwrap();
    }
    let t = e.finish(&mut sink()).unwrap();
    assert!(t.first_layer_normal > 6.0 && t.first_layer_normal < t.normal);
    let mut e = Estimator::new(&c);
    for i in 0..20_000 {
        e.process_line(&format!("G1 X{} F600", i % 2), &mut sink())
            .unwrap();
        assert!(e.machines[0].len() <= REFRESH);
    }
    assert!(e.finish(&mut sink()).unwrap().normal.is_finite());
}

#[test]
fn printer_stops_synchronise_and_count_down() {
    let mut c = config();
    c.set_raw("remaining_times", "1").unwrap();
    // Three minutes of motion, a colour change for the active extruder, then two more.
    let mut g = String::from(";_GP_FIRST_LINE_M73_PLACEHOLDER\nG1 F600\n");
    for i in 0..18 {
        g += &format!("G1 X{}\n", if i % 2 == 0 { 100 } else { 0 });
    }
    g += ";COLOR_CHANGE,T0,#FF0000\nM600\n";
    for i in 0..12 {
        g += &format!("G1 X{}\n", if i % 2 == 0 { 100 } else { 0 });
    }
    // A colour change for another extruder is no stop for this one.
    g += ";COLOR_CHANGE,T1,#00FF00\n;_GP_LAST_LINE_M73_PLACEHOLDER\n";
    let mut a = Analysis::new(&mut Cursor::new(g.as_bytes()), &c).unwrap();
    assert_eq!(a.stops.len(), 1);
    let mut out = Vec::new();
    a.write(
        &mut Cursor::new(g.as_bytes()),
        &mut out,
        MoveStats::default(),
    )
    .unwrap();
    let out = String::from_utf8(out).unwrap();
    let stops: Vec<_> = out.lines().filter(|l| l.starts_with("M73 C")).collect();
    // The first line names the whole wait, then whole minutes, then the last minute (the only
    // stop, so an integer at the end).
    assert_eq!(stops.first(), Some(&"M73 C3"), "{stops:?}");
    assert!(stops.contains(&"M73 C1"), "{stops:?}");
    assert_eq!(stops.last(), Some(&"M73 C0"), "{stops:?}");
    let at = out.find(";COLOR_CHANGE,T0").unwrap();
    assert!(
        !out[at..].contains("M73 C"),
        "no countdown after the last stop"
    );
}

#[test]
fn klipper_uses_orca_centripetal_corner_limit_and_vector_jerk() {
    let mut prusa = config();
    for a in ['x', 'y', 'z', 'e'] {
        prusa
            .set_raw(&format!("machine_max_jerk_{a}"), "1000,1000")
            .unwrap();
    }
    let mut orca = prusa.clone();
    orca.set_raw("gcode_flavor", "klipper").unwrap();
    let t = |c: &Config, g: &str| estimate(&mut Cursor::new(g), c).unwrap().normal;
    // A polyline of 1 mm segments turning 10° at each vertex: BBS limits every junction to
    // sqrt(a·r), r = 0.5 mm / sin(5°), while a classic-jerk planner keeps the cruise speed.
    let mut g = String::from("G1 X0 Y0 F6000\n");
    let (mut x, mut y, mut angle) = (0f64, 0f64, 0f64);
    for _ in 0..20 {
        angle += 10f64.to_radians();
        x += angle.cos();
        y += angle.sin();
        g += &format!("G1 X{x:.4} Y{y:.4}\n");
    }
    assert!(t(&orca, &g) > 1.2 * t(&prusa, &g));
    // A straight line is unaffected.
    let straight = "G1 X10 F600\nG1 X20\n";
    close(t(&orca, straight), t(&prusa, straight));
    // Orca compares one XYZ jerk vector: a diagonal reversal is limited on both axes at once.
    let mut jerky = config();
    for a in ['x', 'y', 'z', 'e'] {
        jerky
            .set_raw(&format!("machine_max_jerk_{a}"), "3,3")
            .unwrap();
    }
    let mut orca_jerky = jerky.clone();
    orca_jerky.set_raw("gcode_flavor", "klipper").unwrap();
    let corner = "G1 X10 F600\nG1 X10 Y10\nG1 X20 Y10\n";
    assert!(t(&orca_jerky, corner).is_finite() && t(&jerky, corner).is_finite());
}
