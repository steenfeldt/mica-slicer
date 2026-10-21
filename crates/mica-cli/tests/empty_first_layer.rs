// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! An object that prints nothing on its first layer is not printed: PrusaSlicer and
//! OrcaSlicer stop the slice in `collect_layers_to_print` ("There is an object with no
//! extrusions in the first layer.", "One object has an empty first layer and can't be
//! printed."). GUI monkey test, seed 1009: cubes resting on a corner slice to nothing at
//! 0.125 mm, so the first layer held only the prime tower and the bed was heated to 0 °C;
//! seed 3019: a sub-millimeter object printed its first lines on layer 2, at a temperature
//! the hotend was never waited for.

use std::path::{Path, PathBuf};

const U1: &[&str] = &[
    "--vendor",
    "Snapmaker",
    "--printer",
    "Snapmaker U1 (0.4 nozzle)",
    "--print",
    "0.20 Standard @Snapmaker U1 (0.4 nozzle)",
    "--filament",
    "Snapmaker PLA @U1",
    "--bed",
    "High Temp Plate",
];

/// A 20 mm cube balanced on one corner (its long diagonal vertical), corner at z = 0.
fn corner_cube(path: &Path) {
    let rx =
        |[x, y, z]: [f64; 3], a: f64| [x, y * a.cos() - z * a.sin(), y * a.sin() + z * a.cos()];
    let ry =
        |[x, y, z]: [f64; 3], a: f64| [x * a.cos() + z * a.sin(), y, -x * a.sin() + z * a.cos()];
    let mut points = vec![];
    for x in [0., 20.] {
        for y in [0., 20.] {
            for z in [0., 20.] {
                let p = rx([x - 10., y - 10., z - 10.], std::f64::consts::FRAC_PI_4);
                points.push(ry(p, (1. / 2f64.sqrt()).atan()));
            }
        }
    }
    let bottom = points.iter().map(|p| p[2]).fold(f64::MAX, f64::min);
    let quads = [
        [0, 1, 3, 2],
        [4, 6, 7, 5],
        [0, 4, 5, 1],
        [2, 3, 7, 6],
        [0, 2, 6, 4],
        [1, 5, 7, 3],
    ];
    let mut stl = vec![0u8; 80];
    stl.extend(12u32.to_le_bytes());
    for q in quads {
        for tri in [[q[0], q[1], q[2]], [q[0], q[2], q[3]]] {
            stl.extend([0f32; 3].iter().flat_map(|v| v.to_le_bytes()));
            for i in tri {
                let [x, y, z] = points[i];
                for v in [x + 100., y + 100., z - bottom] {
                    stl.extend((v as f32).to_le_bytes());
                }
            }
            stl.extend([0, 0]);
        }
    }
    std::fs::write(path, stl).unwrap();
}

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

/// Slices `count` corner-standing cubes with `printer` and returns the error.
fn corner_cubes_fail(name: &str, count: usize, printer: &[&str], extra: &[&str]) -> Option<String> {
    if !mica_testkit::require_profiles_for(printer) {
        return None;
    }
    let dir: PathBuf =
        std::env::temp_dir().join(format!("mica-empty-first-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (a, b, out) = (dir.join("a.stl"), dir.join("b.stl"), dir.join("out.gcode"));
    corner_cube(&a);
    corner_cube(&b);
    let path = |p: &Path| p.to_string_lossy().into_owned();
    let mut all = args(&["slice", &path(&a)]);
    if count > 1 {
        all.push(path(&b));
    }
    all.extend(args(printer));
    all.extend(args(extra));
    all.extend(["-o".into(), path(&out)]);
    let result = mica_cli::run(&all);
    let _ = std::fs::remove_dir_all(&dir);
    match result {
        Ok(()) => panic!("an object with an empty first layer was printed: {all:?}"),
        Err(e) => Some(e),
    }
}

#[test]
fn an_object_with_an_empty_first_layer_is_not_printed() {
    // A multi-tool U1 print with a prime tower (seed 1009).
    if let Some(e) = corner_cubes_fail(
        "u1",
        2,
        U1,
        &["--object-extruder", "2=2", "--set", "brim_type=no_brim"],
    ) {
        assert!(e.contains("empty first layer"), "{e}");
    }
    // The CORE One: a plate of two, and one object alone (its own export path).
    for count in [2, 1] {
        if let Some(e) = corner_cubes_fail(&format!("core{count}"), count, &[], &[]) {
            assert!(e.contains("no extrusions in the first layer"), "{e}");
        }
    }
}

/// Axis-aligned boxes as one binary STL (outward facing triangles).
fn boxes(path: &Path, list: &[[f32; 6]]) {
    let mut stl = vec![0u8; 80];
    stl.extend((list.len() as u32 * 12).to_le_bytes());
    for &[x0, y0, z0, x1, y1, z1] in list {
        let v = |i: usize| {
            [
                if i & 4 == 0 { x0 } else { x1 },
                if i & 2 == 0 { y0 } else { y1 },
                if i & 1 == 0 { z0 } else { z1 },
            ]
        };
        for q in [
            [0, 1, 3, 2],
            [4, 6, 7, 5],
            [0, 4, 5, 1],
            [2, 3, 7, 6],
            [0, 2, 6, 4],
            [1, 5, 7, 3],
        ] {
            for tri in [[q[0], q[2], q[1]], [q[0], q[3], q[2]]] {
                stl.extend([0f32; 3].iter().flat_map(|v| v.to_le_bytes()));
                for i in tri {
                    stl.extend(v(i).iter().flat_map(|c| c.to_le_bytes()));
                }
                stl.extend([0, 0]);
            }
        }
    }
    std::fs::write(path, stl).unwrap();
}

/// GUI monkey test (seeds 1012, 1016, 1028; a simplified mesh with open edges near the bed):
/// two objects (so one print job) whose second layer prints nothing panicked the G-code
/// export, which took the second layer's first tool for its bed temperature.
#[test]
fn empty_second_layer_of_a_print_job_exports() {
    let dir: PathBuf =
        std::env::temp_dir().join(format!("mica-empty-second-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (a, b, out) = (dir.join("a.stl"), dir.join("b.stl"), dir.join("out.gcode"));
    // A 0.2 mm base and, after a 0.6 mm gap, a block (in one object).
    let gap = [[0., 0., 0., 20., 20., 0.2], [0., 0., 0.8, 20., 20., 10.]];
    boxes(&a, &gap);
    boxes(&b, &gap);
    let path = |p: &Path| p.to_string_lossy().into_owned();
    let all = args(&["slice", &path(&a), &path(&b), "-o", &path(&out)]);
    if !mica_testkit::require_profiles_for(&all) {
        return;
    }
    match mica_cli::run(&all) {
        Ok(()) => {}
        Err(e) => panic!("mica {all:?}: {e}"),
    }
    let gcode = std::fs::read_to_string(&out).unwrap();
    assert!(gcode.contains(";Z:.4\n"), "the empty layer is kept");
    let _ = std::fs::remove_dir_all(&dir);
}

/// CLI fuzz, 2026-10-21: an XY compensation that eats the whole object left no layer with a
/// slice; the slice then failed in the start G-code ("Parsing error … Not a variable name" at
/// `first_layer_print_min`) instead of with upstream's empty-first-layer error.
#[test]
fn an_object_compensated_away_is_not_printed() {
    let dir: PathBuf =
        std::env::temp_dir().join(format!("mica-compensated-away-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (a, b, out) = (dir.join("a.stl"), dir.join("b.stl"), dir.join("out.gcode"));
    boxes(&a, &[[100., 100., 0., 120., 120., 10.]]);
    boxes(&b, &[[130., 100., 0., 150., 120., 10.]]);
    let path = |p: &Path| p.to_string_lossy().into_owned();
    for (count, printer, words) in [
        (1, &[][..], "no extrusions in the first layer"),
        (2, &[][..], "no extrusions in the first layer"),
        (1, U1, "empty first layer"),
        (2, U1, "empty first layer"),
    ] {
        if !mica_testkit::require_profiles_for(printer) {
            continue;
        }
        let mut all = args(&["slice", &path(&a)]);
        if count > 1 {
            all.push(path(&b));
        }
        all.extend(args(printer));
        all.extend(args(&[
            "--set",
            "xy_size_compensation=-15",
            "-o",
            &path(&out),
        ]));
        match mica_cli::run(&all) {
            Ok(()) => panic!("an object compensated away was printed: {all:?}"),
            Err(e) => assert!(e.contains(words), "{all:?}: {e}"),
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}
