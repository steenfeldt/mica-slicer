// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Memory-test fixture generator: paints every model part of a 3MF project with many small
//! random material brush strokes (as the GUI brushes do, splitting triangles along each brush
//! edge) and saves the result.
//!
//! cargo run --release -p mica-core --example paint_dense -- IN.3mf OUT.3mf [STROKES] [TOOLS] [SEED]
use mica_core::{
    painting::selector::{Brush, Cursor, TriangleSelector},
    threemf::{Flavour, PartKind, load_3mf, write},
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [input, output] = [&args[0], &args[1]];
    let strokes: usize = args.get(2).map_or(Ok(20_000), |s| s.parse())?;
    let tools: u8 = args.get(3).map_or(Ok(5), |s| s.parse())?;
    let mut seed: u64 = args.get(4).map_or(Ok(1), |s| s.parse())?;
    let mut next = move || {
        // SplitMix64.
        seed = seed.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = seed;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        (z ^ (z >> 31)) >> 11
    };
    let mut project = load_3mf(input)?;
    const IDENTITY: [[f32; 4]; 4] = [
        [1., 0., 0., 0.],
        [0., 1., 0., 0.],
        [0., 0., 1., 0.],
        [0., 0., 0., 1.],
    ];
    for part in project
        .objects
        .iter_mut()
        .flat_map(|o| o.parts.iter_mut())
        .filter(|p| p.kind == PartKind::Model)
    {
        let mesh = &part.mesh;
        let mut selector = TriangleSelector::new(mesh, &IDENTITY, false);
        for _ in 0..strokes {
            let facet = next() as usize % mesh.faces.len();
            let [a, b, c] = mesh.faces[facet].map(|v| mesh.vertices[v as usize]);
            let centre: [f32; 3] = std::array::from_fn(|i| (a[i] + b[i] + c[i]) / 3.);
            let n = selector.normal(facet);
            let camera: [f32; 3] = std::array::from_fn(|i| centre[i] + 100. * n[i]);
            // GUI brush radii: 0.4 to 25 mm; small brushes refine the most.
            let radius = 0.4 + (next() % 1000) as f32 / 1000. * 2.6;
            let brush = if next() % 4 == 0 {
                Brush::Sphere
            } else {
                Brush::Circle
            };
            let state = 1 + (next() % tools as u64) as u8;
            let cursor = Cursor::new(brush, centre, camera, radius);
            selector.paint(facet, &cursor, state, true, None);
        }
        part.painting.material = selector.serialize();
        let mut leaves = 0usize;
        selector.leaves(true, |_, _| leaves += 1);
        eprintln!(
            "{}: {} facets → {leaves} leaves after {strokes} strokes",
            part.name,
            mesh.faces.len()
        );
    }
    write(output, &project, Flavour::Orca)?;
    Ok(())
}
