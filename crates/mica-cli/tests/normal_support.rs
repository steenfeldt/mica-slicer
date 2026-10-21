// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Normal-support motion oracle, measured with the unmodified PrusaSlicer 2.9.6 CLI.
//! CORE One HF0.4 / 0.20mm SPEED / Prusament PLA, automatic grid support, arm-out T
//! from tests/models/support_organic.py, centered at 125,110. No reference slicer needed
//! at test time. Metrics count same-layer support-to-support travels/retractions (wipes
//! included), positive support E, and all non-extruding XY travel.

use std::{path::Path, process::Command};

type Tri = [[f32; 3]; 3];

/// Union of axis-aligned boxes as one closed orthogonal shell (support_overhangs.write_model).
fn boxes(boxes: &[[f32; 6]]) -> Vec<Tri> {
    let axis = |a: usize| {
        let mut v: Vec<f32> = boxes.iter().flat_map(|b| [b[a], b[a + 3]]).collect();
        v.sort_by(f32::total_cmp);
        v.dedup();
        v
    };
    let (xs, ys, zs) = (axis(0), axis(1), axis(2));
    let inside = |i: isize, j: isize, k: isize| {
        if i < 0 || j < 0 || k < 0 {
            return false;
        }
        let (i, j, k) = (i as usize, j as usize, k as usize);
        if i + 1 >= xs.len() || j + 1 >= ys.len() || k + 1 >= zs.len() {
            return false;
        }
        let m = [
            (xs[i] + xs[i + 1]) / 2.,
            (ys[j] + ys[j + 1]) / 2.,
            (zs[k] + zs[k + 1]) / 2.,
        ];
        boxes
            .iter()
            .any(|b| (0..3).all(|a| b[a] < m[a] && m[a] < b[a + 3]))
    };
    let mut out = Vec::new();
    let mut quad = |a, b, c, d| {
        out.push([a, b, c]);
        out.push([a, c, d]);
    };
    for i in 0..xs.len() - 1 {
        for j in 0..ys.len() - 1 {
            for k in 0..zs.len() - 1 {
                let (ii, jj, kk) = (i as isize, j as isize, k as isize);
                if !inside(ii, jj, kk) {
                    continue;
                }
                let (x0, x1, y0, y1, z0, z1) =
                    (xs[i], xs[i + 1], ys[j], ys[j + 1], zs[k], zs[k + 1]);
                if !inside(ii, jj, kk - 1) {
                    quad([x0, y0, z0], [x0, y1, z0], [x1, y1, z0], [x1, y0, z0]);
                }
                if !inside(ii, jj, kk + 1) {
                    quad([x0, y0, z1], [x1, y0, z1], [x1, y1, z1], [x0, y1, z1]);
                }
                if !inside(ii - 1, jj, kk) {
                    quad([x0, y0, z0], [x0, y0, z1], [x0, y1, z1], [x0, y1, z0]);
                }
                if !inside(ii + 1, jj, kk) {
                    quad([x1, y0, z0], [x1, y1, z0], [x1, y1, z1], [x1, y0, z1]);
                }
                if !inside(ii, jj - 1, kk) {
                    quad([x0, y0, z0], [x1, y0, z0], [x1, y0, z1], [x0, y0, z1]);
                }
                if !inside(ii, jj + 1, kk) {
                    quad([x0, y1, z0], [x0, y1, z1], [x1, y1, z1], [x1, y1, z0]);
                }
            }
        }
    }
    out
}

fn write_arms(path: &Path) {
    let triangles = boxes(&[[-4., -4., 0., 4., 4., 40.], [-28., -4., 34., 28., 4., 40.]]);
    let mut data = vec![b' '; 80];
    data.extend((triangles.len() as u32).to_le_bytes());
    for t in &triangles {
        let u: Vec<f32> = (0..3).map(|i| t[1][i] - t[0][i]).collect();
        let v: Vec<f32> = (0..3).map(|i| t[2][i] - t[0][i]).collect();
        let n = [
            u[1] * v[2] - u[2] * v[1],
            u[2] * v[0] - u[0] * v[2],
            u[0] * v[1] - u[1] * v[0],
        ];
        let l = n
            .iter()
            .map(|x| x * x)
            .sum::<f32>()
            .sqrt()
            .max(f32::MIN_POSITIVE);
        for x in n.iter().map(|x| x / l).chain(t.iter().flatten().copied()) {
            data.extend(x.to_le_bytes());
        }
        data.extend([0, 0]);
    }
    std::fs::write(path, data).unwrap();
}

#[derive(Debug, Default)]
struct Metrics {
    support_travel: f64,
    support_retractions: usize,
    support_extrusion: f64,
    total_travel: f64,
}

fn metrics(gcode: &str) -> Metrics {
    let mut out = Metrics::default();
    let (mut x, mut y, mut e) = (0., 0., 0.);
    let (mut support, mut last_support, mut retracted, mut relative) = (false, false, false, false);
    let mut pending_travel = 0.;
    for line in gcode.lines() {
        if line.starts_with(";LAYER_CHANGE") {
            last_support = false;
            retracted = false;
            pending_travel = 0.;
        }
        if let Some(role) = line.strip_prefix(";TYPE:") {
            support = role.starts_with("Support");
        }
        let mut words = line.split(';').next().unwrap_or("").split_whitespace();
        let Some(cmd) = words.next() else { continue };
        if cmd == "M83" {
            relative = true;
        }
        if cmd == "M82" {
            relative = false;
        }
        if cmd.starts_with('T') {
            last_support = false;
            retracted = false;
            pending_travel = 0.;
        }
        let mut next = (x, y);
        let mut next_e = None;
        for w in words {
            let Some(value) = w.get(1..).and_then(|s| s.parse::<f64>().ok()) else {
                continue;
            };
            match w.as_bytes()[0] {
                b'X' => next.0 = value,
                b'Y' => next.1 = value,
                b'E' => next_e = Some(value),
                _ => {}
            }
        }
        if cmd == "G92" {
            if let Some(v) = next_e {
                e = v;
            }
            continue;
        }
        if !matches!(cmd, "G0" | "G1" | "G2" | "G3") {
            continue;
        }
        let delta = next_e.map_or(0., |v| if relative { v } else { v - e });
        if !relative && let Some(v) = next_e {
            e = v;
        }
        let d = (next.0 - x).hypot(next.1 - y);
        if delta < 0. {
            retracted = true;
        }
        if delta > 0. && d > 0. {
            if support {
                out.support_extrusion += delta;
                if last_support {
                    out.support_travel += pending_travel;
                    out.support_retractions += usize::from(retracted);
                }
            }
            last_support = support;
            retracted = false;
            pending_travel = 0.;
        } else if d > 0. {
            pending_travel += d;
            out.total_travel += d;
        }
        (x, y) = next;
    }
    out
}

#[test]
fn core_one_grid_support_motion_matches_prusaslicer() {
    if !mica_testkit::require_prusa_profiles() {
        return;
    }
    let dir = std::env::temp_dir().join(format!("mica-normal-support-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let model = dir.join("support_arms.stl");
    let output = dir.join("arms.gcode");
    write_arms(&model);
    let result = Command::new(env!("CARGO_BIN_EXE_mica"))
        .env("MICA_CONFIG_DIR", dir.join("config"))
        .env("MICA_ISOLATED", "1")
        .args([
            "slice",
            model.to_str().unwrap(),
            "--set",
            "support_material=1",
            "--set",
            "support_material_auto=1",
            "--set",
            "support_material_style=grid",
            "-o",
            output.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&result.stderr);

    assert!(result.status.success(), "mica: {stderr}");
    let measured = metrics(&std::fs::read_to_string(output).unwrap());
    std::fs::remove_dir_all(dir).unwrap();
    eprintln!("{measured:?}");
    // PrusaSlicer 2.9.6 CLI, b028299c77; the reference G-code is not checked in.
    let reference = Metrics {
        support_travel: 1140.5,
        support_retractions: 120,
        support_extrusion: 1408.745,
        total_travel: 11071.4,
    };
    assert!(
        (measured.support_travel / reference.support_travel - 1.).abs() < 0.05,
        "{measured:?}"
    );
    assert!(
        measured
            .support_retractions
            .abs_diff(reference.support_retractions)
            <= 3,
        "{measured:?}"
    );
    assert!(
        (measured.support_extrusion / reference.support_extrusion - 1.).abs() < 0.01,
        "{measured:?}"
    );
    // Total includes the still-different object and layer-arrival tours.
    assert!(
        (measured.total_travel / reference.total_travel - 1.).abs() < 0.20,
        "{measured:?}"
    );
}
