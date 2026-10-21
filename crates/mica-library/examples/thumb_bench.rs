// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Thumbnail render times per model: the repository's test models and a generated very
//! detailed sphere (simplified before rendering), read, rendered and PNG-encoded:
//!
//! ```sh
//! cargo run --profile fast --offline -p mica-library --example thumb_bench
//! ```

use mica_library::Kind;
use mica_library::render::{load_mesh, thumbnail_png};
use mica_library::testkit::repo_root;
use std::time::Instant;

fn main() {
    let models = repo_root().join("tests/models");
    let mut cases: Vec<(String, Kind, Vec<u8>)> = [
        ("cube20.stl", Kind::Stl),
        ("shells.stl", Kind::Stl),
        ("thin_features.stl", Kind::Stl),
        ("3DBenchy.stl", Kind::Stl),
        ("painted-cube.3mf", Kind::ThreeMf),
        ("plates-orca.3mf", Kind::ThreeMf),
    ]
    .into_iter()
    .filter_map(|(f, k)| Some((f.to_string(), k, std::fs::read(models.join(f)).ok()?)))
    .collect();
    // About two million triangles, to show the simplification path.
    let sphere = mica_core::mesh_edit::sphere(40., 1000, 1000);
    let mut stl = vec![0u8; 80];
    stl.extend((sphere.faces.len() as u32).to_le_bytes());
    for f in &sphere.faces {
        stl.extend([0u8; 12]);
        for &i in f {
            for c in sphere.vertices[i as usize] {
                stl.extend(c.to_le_bytes());
            }
        }
        stl.extend([0u8; 2]);
    }
    cases.push(("sphere-2M.stl (generated)".into(), Kind::Stl, stl));
    println!(
        "{:<28} {:>10} {:>9} {:>9} {:>9}",
        "model", "triangles", "load ms", "render ms", "PNG KB"
    );
    for (name, kind, data) in cases {
        let t = Instant::now();
        let mesh = load_mesh(kind, &data).expect("mesh");
        let load = t.elapsed().as_secs_f64() * 1e3;
        let t = Instant::now();
        let png = thumbnail_png(&mesh).expect("png");
        let render = t.elapsed().as_secs_f64() * 1e3;
        println!(
            "{name:<28} {:>10} {load:>9.1} {render:>9.1} {:>9.1}",
            mesh.faces.len(),
            png.len() as f64 / 1024.
        );
    }
}
