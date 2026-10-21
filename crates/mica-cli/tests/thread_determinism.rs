// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The G-code does not depend on the number of worker threads: the same job sliced with one
//! worker and with several gives byte-identical files. The jobs cover organic tree supports on
//! their own worker pool, the tree drawing's parallel unions, an object without regions (its
//! surfaces beside the seams), a painted multi-material object with supports enabled (layer
//! toolpaths generated ahead of the prepass while supports run) and the time analysis that
//! reads the text while it is written.

use std::path::Path;
use std::process::Command;

const U1: [&str; 10] = [
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
const TREE: [&str; 6] = [
    "--set",
    "enable_support=1",
    "--set",
    "support_type=tree(auto)",
    "--set",
    "support_style=organic",
];

/// An L-shaped prism (30 x 10 x 15 mm) whose arm overhangs 20 mm, as binary STL.
fn write_l_prism(path: &Path) {
    // Cross-section in XZ, extruded along Y from 0 to 10 mm.
    let outline = [
        (0., 0.),
        (10., 0.),
        (10., 10.),
        (30., 10.),
        (30., 15.),
        (10., 15.),
        (0., 15.),
    ];
    let caps: [&[(f32, f32)]; 2] = [
        &[(0., 0.), (10., 0.), (10., 10.), (10., 15.), (0., 15.)],
        &[(10., 10.), (30., 10.), (30., 15.), (10., 15.)],
    ];
    let mut tris: Vec<[[f32; 3]; 3]> = Vec::new();
    for cap in caps {
        for i in 1..cap.len() - 1 {
            let (a, b, c) = (cap[0], cap[i], cap[i + 1]);
            tris.push([[a.0, 0., a.1], [b.0, 0., b.1], [c.0, 0., c.1]]);
            tris.push([[a.0, 10., a.1], [c.0, 10., c.1], [b.0, 10., b.1]]);
        }
    }
    for i in 0..outline.len() {
        let (p, q): ((f32, f32), (f32, f32)) = (outline[i], outline[(i + 1) % outline.len()]);
        let (a, b, c, d) = (
            [p.0, 0., p.1],
            [q.0, 0., q.1],
            [q.0, 10., q.1],
            [p.0, 10., p.1],
        );
        tris.push([a, c, b]);
        tris.push([a, d, c]);
    }
    let mut out = vec![0u8; 80];
    out.extend((tris.len() as u32).to_le_bytes());
    for t in &tris {
        out.extend([0u8; 12]);
        for v in t {
            for c in v {
                out.extend(c.to_le_bytes());
            }
        }
        out.extend([0u8; 2]);
    }
    std::fs::write(path, out).unwrap();
}

/// Slices with `threads` workers; `None` (a reported skip) when the profiles are not fetched.
fn slice(dir: &Path, threads: usize, args: &[&str], name: &str) -> Option<Vec<u8>> {
    if !mica_testkit::require_profiles_for(args) {
        return None;
    }
    let output = dir.join(format!("{name}-{threads}.gcode"));
    let result = Command::new(env!("CARGO_BIN_EXE_mica"))
        .env("MICA_CONFIG_DIR", dir.join("config"))
        .env("MICA_ISOLATED", "1")
        .env("SOURCE_DATE_EPOCH", "0")
        .env("RAYON_NUM_THREADS", threads.to_string())
        .arg("slice")
        .args(args)
        .arg("-o")
        .arg(&output)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&result.stderr);

    assert!(result.status.success(), "mica: {stderr}");
    Some(std::fs::read(output).unwrap())
}

#[test]
fn gcode_is_the_same_with_one_and_several_workers() {
    let dir = std::env::temp_dir().join(format!("mica-thread-determinism-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let model = dir.join("l_prism.stl");
    write_l_prism(&model);
    let models = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/models");
    let painted = models.join("painted-cube.3mf");
    let l_args: Vec<&str> = [model.to_str().unwrap()]
        .into_iter()
        .chain(U1)
        .chain(TREE)
        .collect();
    let painted_args: Vec<&str> = [
        painted.to_str().unwrap(),
        "--vendor",
        "Snapmaker",
        "--printer",
        "Snapmaker U1 (0.4 nozzle)",
        "--bed",
        "High Temp Plate",
    ]
    .into_iter()
    .chain(TREE)
    .collect();
    for (name, args) in [("l_prism", &l_args), ("painted", &painted_args)] {
        let Some(single) = slice(&dir, 1, args, name) else {
            std::fs::remove_dir_all(&dir).unwrap();
            return;
        };
        let several = slice(&dir, 6, args, name).unwrap();
        assert!(!single.is_empty(), "{name}: empty G-code");
        assert!(
            single == several,
            "{name}: G-code differs between 1 and 6 workers"
        );
    }
    std::fs::remove_dir_all(&dir).unwrap();
}
