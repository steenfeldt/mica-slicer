// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: orca@8500fcdcca src/libslic3r/GCode.cpp :: GCode::extrude_loop
// Changes: Project scarf edges onto existing Arachne width runs, preserving flow and bridge roles; keep the existing seam and ramp geometry.

use crate::{
    geometry::{Point, SCALE},
    toolpath::ExtrusionPath,
};

fn segment_distance(p: Point, a: Point, b: Point) -> f64 {
    let dx = (b.x - a.x) as f64 / SCALE;
    let dy = (b.y - a.y) as f64 / SCALE;
    let px = (p.x - a.x) as f64 / SCALE;
    let py = (p.y - a.y) as f64 / SCALE;
    let t = if dx * dx + dy * dy > 0. {
        ((px * dx + py * dy) / (dx * dx + dy * dy)).clamp(0., 1.)
    } else {
        0.
    };
    (px - t * dx).hypot(py - t * dy)
}

/// Scarf points follow the same closed centreline twice. Flow ownership is geometric,
/// so it remains valid after seam projection, direction reversal and ramp subdivision.
pub(crate) fn split(opened: &ExtrusionPath, runs: &[ExtrusionPath]) -> Vec<ExtrusionPath> {
    let Some(meta) = opened.seam.as_ref() else {
        return vec![];
    };
    let mut owners = Vec::new();
    for edge in opened.points.windows(2) {
        let mid = Point::new(
            edge[0].x + (edge[1].x - edge[0].x) / 2,
            edge[0].y + (edge[1].y - edge[0].y) / 2,
        );
        let owner = runs
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| {
                let distance = |p: &ExtrusionPath| {
                    p.points
                        .windows(2)
                        .map(|w| segment_distance(mid, w[0], w[1]))
                        .fold(f64::INFINITY, f64::min)
                };
                distance(a).total_cmp(&distance(b))
            })
            .map(|(i, _)| i)
            .unwrap_or(0);
        owners.push(owner);
    }
    let mut result = Vec::new();
    let mut start = 0;
    while start < owners.len() {
        let end = (start + 1..owners.len())
            .find(|&i| owners[i] != owners[start])
            .unwrap_or(owners.len());
        let mut path = runs[owners[start]].clone();
        path.points = opened.points[start..=end].to_vec();
        let mut seam = meta.clone();
        seam.samples = meta.samples[start..=end].to_vec();
        seam.scarf_ranges = meta
            .scarf_ranges
            .map(|bounds| bounds.map(|i| i.saturating_sub(start).min(end - start)));
        if end < owners.len() {
            seam.wipe = None;
        }
        path.seam = Some(seam);
        path.overhang = None;
        result.push(path);
        start = end;
    }
    result
}
