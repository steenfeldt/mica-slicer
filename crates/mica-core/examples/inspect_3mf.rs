// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Read-only oracle/inspection utility: cargo run --release -p mica-core --example inspect_3mf -- file.3mf …
use mica_core::threemf::{PartKind, load_3mf};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    for path in std::env::args().skip(1) {
        let project = load_3mf(&path)?;
        println!(
            "FILE {path}\nobjects={} parts={} plates={}",
            project.objects.len(),
            project.objects.iter().map(|o| o.parts.len()).sum::<usize>(),
            project.plates.len()
        );
        let mut total = 0.0;
        for object in &project.objects {
            let model_volume: f64 = object
                .parts
                .iter()
                .filter(|p| p.kind == PartKind::Model)
                .map(|p| p.mesh.volume())
                .sum();
            let all_volume: f64 = object.parts.iter().map(|p| p.mesh.volume()).sum();
            let placed = object.placed_model_mesh()?;
            total += placed.volume();
            println!(
                "  object={} name={:?} parts={} instances={} plates={:?} triangles={} model_mm3={model_volume:.6} all_parts_mm3={all_volume:.6} placed_model_mm3={:.6}",
                object.source.id,
                object.name,
                object.parts.len(),
                object.instances.len(),
                object.instances.iter().map(|i| i.plate).collect::<Vec<_>>(),
                placed.faces.len(),
                placed.volume()
            );
            for p in &object.parts {
                println!(
                    "    part={:?} kind={:?} triangles={} mm3={:.6}",
                    p.name,
                    p.kind,
                    p.mesh.faces.len(),
                    p.mesh.volume()
                );
            }
        }
        println!(
            "total_placed_model_mm3={total:.6}\nignored={:?}",
            project.ignored_features
        );
    }
    Ok(())
}
