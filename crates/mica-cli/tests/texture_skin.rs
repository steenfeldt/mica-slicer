// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Texture skin end to end: textured cylinders and Benchies on the CORE One (PrusaSlicer rules)
//! and the U1 (OrcaSlicer rules) slice and validate without errors, every direction and texture
//! source changes the outer walls as it should, and a custom image travels inside the project
//! 3MF.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

const CORE_ONE: &[&str] = &[
    "--printer",
    "Prusa CORE One HF0.4 nozzle",
    "--print",
    "0.20mm SPEED @COREONE HF0.4",
    "--filament",
    "Prusament PLA @COREONE HF0.4",
];
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

fn temp(ext: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "mica-texture-{}-{}.{ext}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}

/// A 20 mm cylinder (Ø20, 96 facets) standing at (100, 100).
fn cylinder(path: &Path) {
    let n = 96;
    let (r, h, c) = (10f32, 20f32, 100f32);
    let ring = |i: usize, z: f32| {
        let a = std::f32::consts::TAU * i as f32 / n as f32;
        [c + r * a.cos(), c + r * a.sin(), z]
    };
    let mut tris = Vec::new();
    for i in 0..n {
        let j = (i + 1) % n;
        let (a0, a1, b0, b1) = (ring(i, 0.), ring(j, 0.), ring(i, h), ring(j, h));
        tris.push([a0, a1, b1]);
        tris.push([a0, b1, b0]);
        tris.push([[c, c, 0.], a1, a0]);
        tris.push([[c, c, h], b0, b1]);
    }
    let mut stl = vec![0u8; 80];
    stl.extend((tris.len() as u32).to_le_bytes());
    for t in tris {
        stl.extend([0f32; 3].iter().flat_map(|v| v.to_le_bytes()));
        for p in t {
            for v in p {
                stl.extend(v.to_le_bytes());
            }
        }
        stl.extend([0u8; 2]);
    }
    std::fs::write(path, stl).unwrap();
}

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

/// Slices and validates (0 errors); the G-code, or `None` without installed profiles.
fn slice(model: &Path, profile: &[&str], extra: &[&str]) -> Option<String> {
    let path = temp("gcode");
    let mut all = args(&["slice", &model.to_string_lossy()]);
    all.extend(args(profile));
    all.extend(args(extra));
    all.extend(["-o".into(), path.to_string_lossy().into_owned()]);
    if !mica_testkit::require_profiles_for(&all) {
        return None;
    }
    match mica_cli::run(&all) {
        Ok(()) => {}
        Err(e) => panic!("mica {all:?}: {e}"),
    }
    let mut validate = args(&["validate", &path.to_string_lossy()]);
    if !model.extension().is_some_and(|e| e == "3mf") {
        validate.extend(args(profile));
    }
    mica_cli::run(&validate).unwrap_or_else(|e| panic!("validate {extra:?}: {e}"));
    let text = std::fs::read_to_string(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    Some(text)
}

/// Total filament of the outer walls (mm of E) and their number of moves.
fn outer_walls(gcode: &str) -> (f64, usize) {
    let mut outer = false;
    let (mut e, mut moves) = (0., 0);
    for line in gcode.lines() {
        if let Some(t) = line.strip_prefix(";TYPE:") {
            outer = t == "External perimeter" || t == "Outer wall";
            continue;
        }
        if outer && line.starts_with("G1 ") && line.contains(" X") {
            moves += 1;
            if let Some(v) = line
                .split_whitespace()
                .find_map(|w| w.strip_prefix('E'))
                .and_then(|v| v.parse::<f64>().ok())
            {
                e += v;
            }
        }
    }
    (e, moves)
}

#[test]
fn textured_cylinders_validate_on_both_slicing_rules() {
    let model = temp("stl");
    cylinder(&model);
    for profile in [CORE_ONE, U1] {
        let Some(plain) = slice(&model, profile, &[]) else {
            continue;
        };
        let (plain_e, plain_moves) = outer_walls(&plain);
        for (texture, direction) in [
            ("procedural:knurl_diamond", "outward"),
            ("builtin:roof_tiles", "inward"),
            ("procedural:hexagons", "both"),
            ("procedural:wood_grain", "outward"),
        ] {
            let tex = format!("mica_texture_skin_texture={texture}");
            let dir = format!("mica_texture_skin_direction={direction}");
            let g = slice(
                &model,
                profile,
                &[
                    "--set",
                    "mica_texture_skin=external",
                    "--set",
                    &tex,
                    "--set",
                    &dir,
                ],
            )
            .unwrap();
            let (e, moves) = outer_walls(&g);
            assert!(
                moves > plain_moves * 2,
                "{texture}: {moves} vs {plain_moves} moves"
            );
            match direction {
                // The bumpy wall is longer: more filament on the outer wall.
                "outward" => assert!(e > plain_e * 1.01, "{texture}: {e} vs {plain_e}"),
                // Inward keeps the part inside its outline; the wall is still longer.
                _ => assert!(e > plain_e * 0.99, "{texture}: {e} vs {plain_e}"),
            }
        }
    }
    let _ = std::fs::remove_file(&model);
}

#[test]
fn textured_benchy_validates_on_both_slicing_rules() {
    let benchy = PathBuf::from(format!(
        "{}/../../tests/models/3DBenchy.stl",
        env!("CARGO_MANIFEST_DIR")
    ));
    for profile in [CORE_ONE, U1] {
        let Some(g) = slice(
            &benchy,
            profile,
            &[
                "--set",
                "mica_texture_skin=external",
                "--set",
                "mica_texture_skin_texture=builtin:shingles",
                "--set",
                "mica_texture_skin_scale=6",
            ],
        ) else {
            continue;
        };
        assert!(outer_walls(&g).1 > 0);
    }
}

#[test]
fn custom_texture_image_travels_in_the_project() {
    let model = temp("stl");
    cylinder(&model);
    // A stripe image: what matters is that the project brings it back.
    let image = temp("png");
    let map = mica_core::texture::heightmap::HeightMap::new(
        8,
        8,
        (0..64).map(|i| if i % 8 < 4 { 1. } else { 0. }).collect(),
    )
    .unwrap();
    std::fs::write(&image, map.to_png()).unwrap();
    let project = temp("3mf");
    let Some(direct) = slice(
        &model,
        CORE_ONE,
        &[
            "--texture-image",
            &image.to_string_lossy(),
            "--set",
            "mica_texture_skin=external",
            "--export-3mf",
            &project.to_string_lossy(),
        ],
    ) else {
        return;
    };
    // ZIP entry names are stored as plain text.
    let bytes = std::fs::read(&project).unwrap();
    let name = b"Metadata/Mica/textures/";
    assert!(
        bytes.windows(name.len()).any(|w| w == name),
        "the image is in the project"
    );
    let from_project = slice(&project, CORE_ONE, &[]).unwrap();
    let plain = slice(&model, CORE_ONE, &[]).unwrap();
    let moves = |g: &str| outer_walls(g).1;
    assert!(moves(&from_project) > moves(&plain) * 2);
    assert!(moves(&direct) > moves(&plain) * 2);
    for p in [&model, &image, &project] {
        let _ = std::fs::remove_file(p);
    }
}
