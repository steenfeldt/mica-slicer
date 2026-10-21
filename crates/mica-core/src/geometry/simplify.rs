// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: prusaslicer@b028299c77 src/libslic3r/MultiPoint.hpp :: douglas_peucker
// Changes: Rust port of the iterative (explicit stack) variant with segment distances, for scaled points.

//! Polyline simplification.

use super::Point;

/// Douglas–Peucker: drops points closer than `tolerance` (scaled units) to the simplified path.
/// Endpoints are always kept.
pub fn douglas_peucker(points: &[Point], tolerance: f64) -> Vec<Point> {
    if points.len() <= 2 || tolerance <= 0.0 {
        return points.to_vec();
    }
    let tol_sq = (tolerance * tolerance) as i128;
    let mut out = Vec::with_capacity(points.len());
    out.push(points[0]);
    let mut anchor = 0usize;
    let mut stack = vec![points.len() - 1];
    while let Some(&floater) = stack.last() {
        let take_floater = if anchor + 1 == floater {
            true
        } else {
            let (a, f) = (points[anchor], points[floater]);
            let v = ((f.x - a.x) as i128, (f.y - a.y) as i128);
            let l2 = v.0 * v.0 + v.1 * v.1;
            let mut max_dist_sq = -1i128;
            let mut furthest = anchor;
            for (k, &p) in points.iter().enumerate().take(floater).skip(anchor + 1) {
                let va = ((p.x - a.x) as i128, (p.y - a.y) as i128);
                let dist_sq = if l2 == 0 {
                    va.0 * va.0 + va.1 * va.1
                } else {
                    let t = va.0 * v.0 + va.1 * v.1;
                    if t <= 0 {
                        va.0 * va.0 + va.1 * va.1
                    } else if t >= l2 {
                        let pf = ((p.x - f.x) as i128, (p.y - f.y) as i128);
                        pf.0 * pf.0 + pf.1 * pf.1
                    } else {
                        let dt = t as f64 / l2 as f64;
                        let w = ((dt * v.0 as f64) as i128, (dt * v.1 as f64) as i128);
                        let d = (w.0 - va.0, w.1 - va.1);
                        d.0 * d.0 + d.1 * d.1
                    }
                };
                if dist_sq > max_dist_sq {
                    max_dist_sq = dist_sq;
                    furthest = k;
                }
            }
            if max_dist_sq <= tol_sq {
                true
            } else {
                stack.push(furthest);
                false
            }
        };
        if take_floater {
            out.push(points[floater]);
            anchor = floater;
            stack.pop();
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::scale;

    #[test]
    fn removes_points_within_tolerance_and_keeps_corners() {
        let p = |x: f64, y: f64| Point::from_mm(x, y);
        // A straight line with jitter of 0.003 mm, then a corner.
        let line = vec![
            p(0.0, 0.0),
            p(1.0, 0.003),
            p(2.0, -0.002),
            p(3.0, 0.0),
            p(3.0, 5.0),
        ];
        let s = douglas_peucker(&line, scale(0.008) as f64);
        assert_eq!(s, vec![p(0.0, 0.0), p(3.0, 0.0), p(3.0, 5.0)]);
        // Larger than the tolerance: kept.
        let bump = vec![p(0.0, 0.0), p(1.0, 0.02), p(2.0, 0.0)];
        assert_eq!(douglas_peucker(&bump, scale(0.008) as f64).len(), 3);
    }

    #[test]
    fn short_and_zero_tolerance_inputs_are_unchanged() {
        let p = |x: f64, y: f64| Point::from_mm(x, y);
        let two = vec![p(0.0, 0.0), p(1.0, 0.0)];
        assert_eq!(douglas_peucker(&two, 1.0), two);
        let three = vec![p(0.0, 0.0), p(1.0, 0.0), p(2.0, 0.0)];
        assert_eq!(douglas_peucker(&three, 0.0), three);
    }
}
