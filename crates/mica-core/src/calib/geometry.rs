// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Procedural calibration solids and an original seven-segment numeral construction.
use crate::{
    geometry::{Point, unscale},
    mesh::TriangleMesh,
};

pub(super) fn append(to: &mut TriangleMesh, from: &TriangleMesh) {
    let offset = to.vertices.len() as u32;
    to.vertices.extend(&from.vertices);
    to.faces
        .extend(from.faces.iter().map(|f| f.map(|i| i + offset)));
}
pub(super) fn cuboid(mesh: &mut TriangleMesh, a: [f64; 3], b: [f64; 3]) {
    let offset = mesh.vertices.len() as u32;
    mesh.vertices.extend(
        [
            [a[0], a[1], a[2]],
            [b[0], a[1], a[2]],
            [b[0], b[1], a[2]],
            [a[0], b[1], a[2]],
            [a[0], a[1], b[2]],
            [b[0], a[1], b[2]],
            [b[0], b[1], b[2]],
            [a[0], b[1], b[2]],
        ]
        .map(|v| v.map(|x| x as f32)),
    );
    mesh.faces.extend(
        [
            [0, 2, 1],
            [0, 3, 2],
            [4, 5, 6],
            [4, 6, 7],
            [0, 1, 5],
            [0, 5, 4],
            [1, 2, 6],
            [1, 6, 5],
            [2, 3, 7],
            [2, 7, 6],
            [3, 0, 4],
            [3, 4, 7],
        ]
        .map(|f| f.map(|i| i + offset)),
    );
}
pub(super) fn ribbon(mesh: &mut TriangleMesh, points: &[Point], width: f64, z0: f64, z1: f64) {
    for edge in points.windows(2) {
        let x = unscale(edge[0].x);
        let y = unscale(edge[0].y);
        let dx = unscale(edge[1].x - edge[0].x);
        let dy = unscale(edge[1].y - edge[0].y);
        let length = dx.hypot(dy);
        if length == 0. {
            continue;
        }
        let mut part = TriangleMesh::default();
        cuboid(&mut part, [0., -width / 2., z0], [length, width / 2., z1]);
        for v in &mut part.vertices {
            let a = f64::from(v[0]);
            let b = f64::from(v[1]);
            v[0] = (x + (a * dx - b * dy) / length) as f32;
            v[1] = (y + (a * dy + b * dx) / length) as f32;
        }
        append(mesh, &part);
    }
}
/// Emboss digits in the XZ plane, facing -Y. Each stroke overlaps the supporting face.
pub(super) fn label(mesh: &mut TriangleMesh, text: &str, origin: [f64; 3], scale: f64) {
    let strokes = [
        (0., 4., 2., 4.6),
        (1.4, 2.3, 2., 4.6),
        (1.4, 0., 2., 2.3),
        (0., 0., 2., 0.6),
        (0., 0., 0.6, 2.3),
        (0., 2.3, 0.6, 4.6),
        (0., 2., 2., 2.6),
    ];
    for (i, c) in text.chars().enumerate() {
        let mask = match c {
            '0' => 0b0111111,
            '1' => 0b0000110,
            '2' => 0b1011011,
            '3' => 0b1001111,
            '4' => 0b1100110,
            '5' => 0b1101101,
            '6' => 0b1111101,
            '7' => 0b0000111,
            '8' => 0b1111111,
            '9' => 0b1101111,
            _ => 0,
        };
        for (j, &(x0, z0, x1, z1)) in strokes.iter().enumerate() {
            if mask & (1 << j) != 0 {
                let x = origin[0] + i as f64 * 2.7 * scale;
                cuboid(
                    mesh,
                    [
                        x + x0 * scale,
                        origin[1] - 0.4 * scale,
                        origin[2] + z0 * scale,
                    ],
                    [
                        x + x1 * scale,
                        origin[1] + 0.2 * scale,
                        origin[2] + z1 * scale,
                    ],
                );
            }
        }
    }
}
