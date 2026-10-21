// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/SeamAligned.cpp :: snap_to_angle, get_seam_options, pick_seam_option
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/SeamPlacer.cpp :: NearestCorner
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/SeamRear.cpp :: RearestPointCalculator
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/SeamRandom.cpp :: get_segments, pick_random_segment, pick_random_point
// Changes: Linear searches and fixed two-word MT19937 real conversion for portable length-uniform random choices.
// Changes: Prusa profiles keep searching past a farther corner of the same kind (snap_to_angle's visitor) and compare squared segment distances like line_alg::distance_to_squared.

use super::geometry::{Choice, V, distance_to_segment_squared, on_segment};
use super::perimeters::{Angle, Boundary};

pub(super) fn snap(b: &Boundary, p: V, start: usize, max_detour: f64) -> Option<usize> {
    let n = b.points.len();
    for kind in [Angle::Convex, Angle::Concave] {
        let mut best = None;
        let mut distance = f64::INFINITY;
        for forward in [false, true] {
            for k in 0..n {
                let i = if forward {
                    (start + k) % n
                } else {
                    (start + n - 1 - k) % n
                };
                let d = (b.points[i] - p).norm();
                if d > max_detour {
                    break;
                }
                if b.kinds[i] == kind && d < distance {
                    best = Some(i);
                    distance = d;
                    break;
                }
                // Upstream keeps searching past a farther point of the same kind.
                if b.kinds[i] == kind && !b.parity {
                    break;
                }
            }
        }
        if best.is_some() {
            return best;
        }
    }
    None
}
pub(super) fn nearest(b: &Boundary, p: V, max_detour: f64, corners_only: bool) -> Choice {
    // The index holds exactly the eligible points and returns the first-minimal `norm()` candidate,
    // as a linear scan over `(0..n).filter(eligible).min_by(norm)` would.
    let index = b.index();
    let eligible = index.best;
    if corners_only && let Some(i) = index.nearest(&b.points, p, |i| b.kinds[i] != Angle::Smooth) {
        return Choice::vertex(&b.points, i);
    }
    let i = index.nearest(&b.points, p, |_| true).unwrap_or(0);
    let n = b.points.len();
    let before = (i + n - 1) % n;
    let after = (i + 1) % n;
    let (a, c, forward) = if b.parity {
        let (a, da) = distance_to_segment_squared(b.points[before], b.points[i], p);
        let (c, dc) = distance_to_segment_squared(b.points[i], b.points[after], p);
        (a, c, dc < da)
    } else {
        let a = on_segment(p, b.points[before], b.points[i]);
        let c = on_segment(p, b.points[i], b.points[after]);
        (a, c, (c - p).norm() < (a - p).norm())
    };
    let adjacent = if forward { after } else { before };
    let on_edge = if forward { c } else { a };
    if let Some(s) = snap(b, on_edge, i, max_detour)
        && (b.paint[s], b.classes[s]) <= eligible
    {
        return Choice::vertex(&b.points, s);
    }
    if (b.paint[adjacent], b.classes[adjacent]) > eligible {
        return Choice::vertex(&b.points, i);
    }
    Choice {
        point: on_edge,
        previous: if forward { i } else { adjacent },
        next: if forward { adjacent } else { i },
    }
}
pub(super) fn rear(b: &Boundary) -> Choice {
    let best = b.best();
    let edges: Vec<usize> = (0..b.points.len())
        .filter(|&i| {
            let j = (i + 1) % b.points.len();
            (b.paint[i], b.classes[i]) == best && (b.paint[j], b.classes[j]) == best
        })
        .collect();
    if edges.is_empty() {
        return nearest(b, V::from(b.bbox.max), 0.0, false);
    }
    let max = edges
        .iter()
        .flat_map(|&i| [i, (i + 1) % b.points.len()])
        .max_by(|&a, &c| b.points[a].y.total_cmp(&b.points[c].y))
        .unwrap();
    let max_point = Choice::vertex(&b.points, max);
    if b.kinds[max] == Angle::Concave
        && !edges
            .iter()
            .flat_map(|&i| [i, (i + 1) % b.points.len()])
            .any(|j| {
                j != max
                    && b.kinds[j] == Angle::Concave
                    && b.points[max].y < b.points[j].y + 0.01
                    && (b.points[max] - b.points[j]).norm() > 0.01
            })
    {
        return max_point;
    }
    let project = |p: V| {
        edges
            .iter()
            .map(|&i| Choice {
                point: on_segment(p, b.points[i], b.points[(i + 1) % b.points.len()]),
                previous: i,
                next: (i + 1) % b.points.len(),
            })
            .min_by(|a, c| (a.point - p).norm().total_cmp(&(c.point - p).norm()))
            .unwrap()
    };
    let lo = V::from(b.bbox.min);
    let hi = V::from(b.bbox.max);
    let center = V::new((lo.x + hi.x) * 0.5, hi.y);
    let far = project(center + V::new(0.0, 20.0));
    let near = project(center);
    let dy = far.point.y - near.point.y;
    let result = if dy < 0.0 {
        near
    } else if dy <= 1.0 {
        project(far.point * dy + near.point * (1.0 - dy))
    } else {
        far
    };
    if hi.y - result.point.y > 1.0 {
        max_point
    } else {
        result
    }
}

pub(super) struct Random(super::random::Mt19937_32);
impl Random {
    pub fn new(seed: u32) -> Self {
        Self(super::random::Mt19937_32::new(seed))
    }
    pub fn choose(&mut self, b: &Boundary) -> Choice {
        // Consecutive eligible vertices define an eligible segment. Keep isolated points
        // as zero-length segments so a tiny painted enforcer is still selectable.
        let best = b.best();
        let n = b.points.len();
        let mut segments = Vec::new();
        let mut start = None;
        let mut distance = 0.0;
        let mut begin_distance = 0.0;
        let mut previous = b.points[0];
        for i in 0..=n {
            let j = i % n;
            let previous_distance = distance;
            distance += (b.points[j] - previous).norm();
            previous = b.points[j];
            if (b.paint[j], b.classes[j]) == best {
                if start.is_none() {
                    start = Some(j);
                    begin_distance = distance;
                }
            } else if let Some(begin) = start.take() {
                segments.push((begin, previous_distance - begin_distance));
            }
        }
        if let Some(begin) = start {
            segments.push((begin, distance - begin_distance));
        }
        let total: f64 = segments.iter().map(|s| s.1).sum();
        let mut at = self.0.unit() * total;
        let mut selected = segments[0];
        for segment in segments {
            if at <= segment.1 {
                selected = segment;
                break;
            }
            at -= segment.1;
        }
        let (begin, length) = selected;
        if length < f64::EPSILON {
            return Choice::vertex(&b.points, begin);
        }
        let mut remaining = self.0.unit() * length;
        for k in 0..n {
            let i = (begin + k) % n;
            let j = (i + 1) % n;
            let edge = b.points[j] - b.points[i];
            let len = edge.norm();
            if len > 0.0 && remaining <= len {
                return Choice {
                    point: b.points[i] + edge * (remaining / len),
                    previous: i,
                    next: j,
                };
            }
            remaining -= len;
        }
        Choice::vertex(&b.points, begin)
    }
}
