// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Replay MICA_VORONOI_DUMP inputs without validation, copying or cache hits.
use boostvoronoi::prelude::{Builder, Line, Point};
use std::{hint::black_box, time::Instant};

fn main() {
    let mut args = std::env::args().skip(1);
    let bytes =
        std::fs::read(args.next().expect("usage: voronoi_bench CAPTURE [REPEATS]")).unwrap();
    let repeats: usize = args.next().map(|s| s.parse().unwrap()).unwrap_or(1);
    let mut inputs = Vec::new();
    let mut cursor = bytes.as_slice();
    while !cursor.is_empty() {
        let count = u32::from_le_bytes(cursor[..4].try_into().unwrap()) as usize;
        cursor = &cursor[4..];
        let lines: Vec<_> = cursor[..count * 16]
            .as_chunks::<16>()
            .0
            .iter()
            .map(|record| {
                let v: Vec<_> = record
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|b| i32::from_le_bytes(*b))
                    .collect();
                Line::new(Point { x: v[0], y: v[1] }, Point { x: v[2], y: v[3] })
            })
            .collect();
        inputs.push(lines);
        cursor = &cursor[count * 16..];
    }
    let start = Instant::now();
    let mut vertices = 0;
    for _ in 0..repeats {
        for lines in &inputs {
            let diagram = Builder::<i32>::default()
                .with_segments(lines.iter())
                .unwrap()
                .build()
                .unwrap();
            vertices += black_box(&diagram).vertices().len();
        }
    }
    println!(
        "diagrams={} segments={} repeats={} vertices={} sweep_s={:.6}",
        inputs.len(),
        inputs.iter().map(Vec::len).sum::<usize>(),
        repeats,
        vertices,
        start.elapsed().as_secs_f64()
    );
}
