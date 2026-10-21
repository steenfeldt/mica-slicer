// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/SeamAligned.cpp :: VisibilityCalculator, get_seam_candidate, get_shell_seam, get_object_seams
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/SeamShells.cpp :: create_shells
// Changes: Parallel candidate chains with ordered score/start reduction; retain only worker-local winners and preserve polygon-order tie-breaking.
// Changes: No KD tree; mesh sample visibility is interpolated at projected choices. Orca profiles keep Mica's earlier same-hole shell matching.

use super::choice;
use super::geometry::{self, Choice};
use super::perimeters::Boundary;
use super::visibility::PointVisibility;
use rayon::prelude::*;

pub(super) fn angle_modifier(angle: f64) -> f64 {
    let weight = if angle > 0.0 { 1.1 } else { 0.9 };
    let a = angle.abs();
    let right = std::f64::consts::FRAC_PI_2;
    if a > right {
        -weight
    } else {
        let linear = a / right;
        -(linear * weight + (right - a) / right * linear)
    }
}
fn score(b: &Boundary, c: Choice, visibility: &dyn PointVisibility) -> f64 {
    visibility.at([c.point.x, c.point.y, b.z]) + angle_modifier(b.angle(c))
}

pub(super) fn plan(
    layers: &[Vec<Boundary>],
    visibility: &dyn PointVisibility,
    observer: &dyn crate::progress::Observer,
) -> Vec<Vec<Choice>> {
    let sizes: Vec<usize> = layers.iter().map(Vec::len).collect();
    let (mapping, count) = geometry::mapping(&sizes, |l, i| {
        let b = &layers[l][i];
        layers[l + 1]
            .iter()
            .enumerate()
            .filter(|(_, c)| b.parity || c.hole == b.hole)
            .map(|(j, c)| (j, geometry::bb_distance(b.bbox, c.bbox)))
            .min_by(|a, c| a.1.total_cmp(&c.1))
            .filter(|&(_, d)| d <= 5.0)
            .map(|(j, d)| (j, 1.0 / d.max(1e-12)))
    });
    let mut shells = vec![Vec::new(); count];
    for (l, ids) in mapping.iter().enumerate() {
        for (i, &id) in ids.iter().enumerate() {
            shells[id].push((l, i));
        }
    }
    let mut result: Vec<Vec<Choice>> = layers
        .iter()
        .map(|l| vec![Choice::default(); l.len()])
        .collect();
    // One tick per shell (0.7 .. 1 of the seam stage); a cancel leaves the rest unplanned.
    let ticker = crate::progress::Ticker::span(
        observer,
        crate::progress::Stage::Seams,
        shells.len(),
        0.7,
        1.,
    );
    for shell in shells {
        if ticker.cancelled() {
            break;
        }
        ticker.tick();
        let Some(&(first_layer, first_index)) = shell.first() else {
            continue;
        };
        let first = &layers[first_layer][first_index];
        let visibilities: Vec<Vec<f64>> = shell
            .par_iter()
            .map(|&(l, i)| {
                let b = &layers[l][i];
                (0..b.points.len())
                    .map(|p| score(b, Choice::vertex(&b.points, p), visibility))
                    .collect()
            })
            .collect();
        let least: Vec<Choice> = shell
            .iter()
            .zip(&visibilities)
            .map(|(&(l, i), v)| {
                let b = &layers[l][i];
                let best = b.best();
                let idx = (0..b.points.len())
                    .filter(|&p| (b.paint[p], b.classes[p]) == best)
                    .min_by(|&a, &c| v[a].total_cmp(&v[c]))
                    .unwrap();
                Choice::vertex(&b.points, idx)
            })
            .collect();
        let paint = *first.paint.iter().min().unwrap();
        let starts: Vec<usize> = (0..first.points.len())
            .filter(|&i| first.paint[i] == paint)
            .collect();
        // Chains depend on their own previous choice and completed shells only. Reduce in
        // score/start order so equal scores retain the original first candidate.
        let best = starts
            .into_par_iter()
            .filter_map(|start| {
                let mut previous = first.points[start];
                let mut chain = Vec::with_capacity(shell.len());
                let mut total = 0.0;
                for (k, &(l, i)) in shell.iter().enumerate() {
                    let b = &layers[l][i];
                    let mut c = choice::nearest(b, previous, 1.0, false);
                    let candidate_score = if c.previous == c.next {
                        visibilities[k][c.previous]
                    } else {
                        score(b, c, visibility)
                    };
                    let low = least[k];
                    let low_score = visibilities[k][low.previous];
                    if (c.point - previous).norm() > 1.0
                        || (!b.smooth(low) && candidate_score > low_score + 0.6)
                    {
                        c = low;
                    }
                    // Upstream scores the proposed candidate even when it has to jump to a hidden corner.
                    total += candidate_score;
                    if k == 0 && first_layer > 0 {
                        let distance = result[first_layer - 1]
                            .iter()
                            .map(|p| (p.point - c.point).norm())
                            .fold(f64::INFINITY, f64::min);
                        if distance < 1.0 {
                            total -= 2.0 * (1.0 - distance);
                        }
                    }
                    previous = c.point;
                    chain.push(c);
                }
                (total < f64::INFINITY).then_some((total, start, chain))
            })
            .min_by(|a, b| a.0.partial_cmp(&b.0).unwrap().then(a.1.cmp(&b.1)))
            .map(|(_, _, chain)| chain)
            .unwrap_or_default();
        for (&(l, i), c) in shell.iter().zip(best) {
            result[l][i] = c;
        }
    }
    result
}
