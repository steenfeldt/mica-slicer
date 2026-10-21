// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Organic tree support on the figurine fixture (tests/models/support_organic.py) stays close to
//! PrusaSlicer 2.9.6. Only the reference number is stored: support + support interface filament
//! of PrusaSlicer 2.9.6's own CLI slice (CORE One defaults, `--support-material
//! --support-material-auto --support-material-style organic`), 697.3 mm. The legacy simplified
//! tree generator used 786.8 mm (+12.8 %).

use std::path::Path;

const PRUSA_SUPPORT_MM: f64 = 697.3;

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

fn sphere(c: [f64; 3], r: f64) -> Vec<Tri> {
    let (rings, segments) = (24, 48);
    let point = |i: usize, j: usize| {
        let (t, p) = (
            std::f64::consts::PI * i as f64 / rings as f64,
            std::f64::consts::TAU * j as f64 / segments as f64,
        );
        [
            (c[0] + r * t.sin() * p.cos()) as f32,
            (c[1] + r * t.sin() * p.sin()) as f32,
            (c[2] + r * t.cos()) as f32,
        ]
    };
    let mut out = Vec::new();
    for i in 0..rings {
        for j in 0..segments {
            let (a, b, c, d) = (
                point(i, j),
                point(i + 1, j),
                point(i + 1, (j + 1) % segments),
                point(i, (j + 1) % segments),
            );
            if i < rings - 1 {
                out.push([a, b, c]);
            }
            if i > 0 {
                out.push([a, c, d]);
            }
        }
    }
    out
}

fn write_figurine(path: &Path) {
    let mut tris = boxes(&[
        [-4., -4., 0., 4., 4., 40.],
        [-28., -4., 34., 28., 4., 40.],
        [21., -1., 26., 23., 1., 35.],
    ]);
    tris.extend(sphere([22., 0., 22.], 6.));
    tris.extend(sphere([0., 0., 45.], 6.));
    let mut data = vec![b' '; 80];
    data.extend((tris.len() as u32).to_le_bytes());
    for t in &tris {
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

/// Filament extruded by Support / Support interface moves, mm (relative or absolute E).
fn support_filament(gcode: &str) -> f64 {
    let (mut relative, mut e, mut support, mut total) = (false, 0., false, 0.);
    for line in gcode.lines() {
        if let Some(role) = line.strip_prefix(";TYPE:") {
            support = role.starts_with("Support");
        }
        let code = line.split(';').next().unwrap_or("").trim();
        let mut words = code.split_whitespace();
        match words.next() {
            Some("M83") => relative = true,
            Some("M82") => relative = false,
            Some("G92") => {
                if let Some(v) = words.find_map(|w| w.strip_prefix('E')) {
                    e = v.parse().unwrap_or(0.);
                }
            }
            Some("G1" | "G2" | "G3") => {
                let words: Vec<&str> = words.collect();
                // Deretractions (E without XY) are not extrusion.
                let moves = words
                    .iter()
                    .any(|w| w.starts_with('X') || w.starts_with('Y'));
                if let Some(v) = words.iter().find_map(|w| w.strip_prefix('E')) {
                    let v: f64 = v.parse().unwrap_or(0.);
                    let delta = if relative { v } else { v - e };
                    if !relative {
                        e = v;
                    }
                    if support && moves && delta > 0. {
                        total += delta;
                    }
                }
            }
            _ => {}
        }
    }
    total
}

#[test]
fn core_one_figurine_organic_support_matches_prusaslicer() {
    let dir = std::env::temp_dir().join(format!("mica-organic-figurine-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let model = dir.join("support_figurine.stl");
    let out = dir.join("figurine.gcode");
    write_figurine(&model);
    let args: Vec<String> = [
        "slice",
        model.to_str().unwrap(),
        "--set",
        "support_material=1",
        "--set",
        "support_material_auto=1",
        "--set",
        "support_material_style=organic",
        "-o",
        out.to_str().unwrap(),
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    if !mica_testkit::require_profiles_for(&args) {
        let _ = std::fs::remove_dir_all(&dir);
        return;
    }
    match mica_cli::run(&args) {
        Ok(()) => {}
        Err(e) => panic!("mica {args:?}: {e}"),
    }
    let gcode = std::fs::read_to_string(&out).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    let mica = support_filament(&gcode);
    let error = mica / PRUSA_SUPPORT_MM - 1.;
    eprintln!("support filament {mica:.1} mm ({:+.2} %)", error * 100.);
    assert!(
        error.abs() < 0.06,
        "support filament {mica:.1} mm vs PrusaSlicer {PRUSA_SUPPORT_MM} mm ({:+.2} %)",
        error * 100.
    );
}
