// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: prusaslicer@b028299c77 src/libslic3r/KDTreeIndirect.hpp :: KDTreeIndirect, find_closest_point
// Source: prusaslicer@b028299c77 src/libslic3r/Utils.hpp :: next_highest_power_of_2
// Changes: Two-dimensional f64 tree over a borrowed point slice; the visitor of find_closest_points<1>
// Changes: is inlined. Build (median-of-three quickselect) and traversal order are kept, so equal
// Changes: distances resolve to the same index (the last visited wins); the squared distance keeps the
// Changes: fused multiply-add the reference clang build forms for `distance_sq += d * d`.
// Upstream attribution (KDTreeIndirect.hpp):
// Copyright (c) Prusa Research 2019 - 2022 Pavel Mikuš @Godrak, Tomáš Mészáros @tamasmeszaros, Vojtěch Bubník @bubnikv, Lukáš Matěna @lukasmatena
//
// PrusaSlicer is released under the terms of the AGPLv3 or higher

//! Closest-point KD tree over indices into a point set.

const NPOS: usize = usize::MAX;
/// PrusaSlicer's `EPSILON`.
const EPSILON: f64 = 1e-4;

pub(crate) struct KdTree {
    nodes: Vec<usize>,
}

fn next_highest_power_of_2(mut v: u64) -> u64 {
    v = v.saturating_sub(1);
    v |= v >> 1;
    v |= v >> 2;
    v |= v >> 4;
    v |= v >> 8;
    v |= v >> 16;
    v |= v >> 32;
    v.wrapping_add(1)
}

impl KdTree {
    /// Builds a balanced tree over the indices `0..points.len()`.
    pub(crate) fn build(points: &[[f64; 2]]) -> Self {
        let mut tree = Self { nodes: Vec::new() };
        if points.is_empty() {
            return tree;
        }
        let mut input: Vec<usize> = (0..points.len()).collect();
        tree.nodes = vec![NPOS; next_highest_power_of_2(points.len() as u64 + 1) as usize];
        tree.build_recursive(points, &mut input, 0, 0, 0, points.len() - 1);
        tree
    }

    fn build_recursive(
        &mut self,
        pts: &[[f64; 2]],
        input: &mut [usize],
        node: usize,
        dimension: usize,
        left: usize,
        right: usize,
    ) {
        if left > right {
            return;
        }
        if left == right {
            self.nodes[node] = input[left];
            return;
        }
        let center = (left + right) / 2;
        partition_input(pts, input, dimension, left, right, center);
        self.nodes[node] = input[center];
        let next_dimension = if dimension + 1 == 2 { 0 } else { dimension + 1 };
        if center > left {
            self.build_recursive(pts, input, node * 2 + 1, next_dimension, left, center - 1);
        }
        self.build_recursive(pts, input, node * 2 + 2, next_dimension, center + 1, right);
    }

    /// `find_closest_point`: the closest point accepted by `filter`, `usize::MAX` if none.
    pub(crate) fn find_closest(
        &self,
        pts: &[[f64; 2]],
        point: [f64; 2],
        mut filter: impl FnMut(usize) -> bool,
    ) -> usize {
        let mut best = (NPOS, f64::MAX);
        if !self.nodes.is_empty() {
            self.visit(pts, 0, 0, point, &mut filter, &mut best);
        }
        best.0
    }

    fn visit(
        &self,
        pts: &[[f64; 2]],
        node: usize,
        dimension: usize,
        point: [f64; 2],
        filter: &mut impl FnMut(usize) -> bool,
        best: &mut (usize, f64),
    ) {
        if node >= self.nodes.len() || self.nodes[node] == NPOS {
            return;
        }
        let idx = self.nodes[node];
        if filter(idx) {
            let d0 = point[0] - pts[idx][0];
            let d1 = point[1] - pts[idx][1];
            // `distance_sq += double(d) * d` over both dimensions, contracted to an FMA.
            let distance_sq = d1.mul_add(d1, d0 * d0);
            // std::lower_bound on a one-element array: a tie replaces the earlier result.
            if best.1 >= distance_sq {
                *best = (idx, distance_sq);
            }
        }
        // descent_mask
        let dist = point[dimension] - pts[idx][dimension];
        let (go_left, go_right) = if dist * dist < best.1 + EPSILON {
            (true, true)
        } else if dist > 0. {
            (false, true)
        } else {
            (true, false)
        };
        let next_dimension = if dimension + 1 == 2 { 0 } else { dimension + 1 };
        let left = node * 2 + 1;
        if go_left {
            self.visit(pts, left, next_dimension, point, filter, best);
        }
        if go_right {
            self.visit(pts, left + 1, next_dimension, point, filter, best);
        }
    }
}

/// Quickselect partition of `input[left..=right]` around the `k`th element in `dimension`.
fn partition_input(
    pts: &[[f64; 2]],
    input: &mut [usize],
    dimension: usize,
    mut left: usize,
    mut right: usize,
    k: usize,
) {
    let coord = |i: usize| pts[i][dimension];
    while left < right {
        let center = (left + right) / 2;
        let pivot;
        {
            let mut left_value = coord(input[left]);
            let mut center_value = coord(input[center]);
            let mut right_value = coord(input[right]);
            if left_value > center_value {
                input.swap(left, center);
                std::mem::swap(&mut left_value, &mut center_value);
            }
            if left_value > right_value {
                input.swap(left, right);
                right_value = left_value;
            }
            if center_value > right_value {
                input.swap(center, right);
                center_value = right_value;
            }
            pivot = center_value;
        }
        if right <= left + 2 {
            break;
        }
        let mut i = left;
        let mut j = right - 1;
        input.swap(center, j);
        loop {
            loop {
                i += 1;
                if coord(input[i]) >= pivot {
                    break;
                }
            }
            loop {
                j -= 1;
                if !(coord(input[j]) > pivot && i < j) {
                    break;
                }
            }
            if i >= j {
                break;
            }
            input.swap(i, j);
        }
        input.swap(i, right - 1);
        if k < i {
            right = i - 1;
        } else if k == i {
            break;
        } else {
            left = i + 1;
        }
    }
}
