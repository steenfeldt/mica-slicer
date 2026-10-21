// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: orca@8500fcdcca src/libslic3r/ExtrusionEntity.cpp :: ExtrusionLoop::is_smooth, ExtrusionLoopSloped::ExtrusionLoopSloped
// Changes: Optional Orca batch B wall, bridge or scarf controls; existing Prusa defaults retained.
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/SeamScarf.cpp :: add_scarf_seam, ensure_max_distance, lineary_increase_extrusion_height, lineary_readuce_extrusion_amount
// Source: prusaslicer@b028299c77 src/libslic3r/GCode.cpp :: SmoothPathGenerator::operator()
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/Wipe.cpp :: wipe_hide_seam
// Changes: Single constant-width polyline, no arc smoothing; optional per-point Z/extrusion fractions; robust zero-gap and tiny-loop handling.

use super::geometry::{self, V};
use super::{PathSeam, ScarfSample, ScarfSettings};
use crate::geometry::{Point, Polygon, SCALE, polyline_length};

fn subdivide(points: &[Point], max: f64) -> Polygon {
    let mut out = Vec::new();
    if let Some(&p) = points.first() {
        out.push(p);
    }
    for w in points.windows(2) {
        let n = (w[0].distance_to(w[1]) / SCALE / max).ceil().max(1.0) as usize;
        for i in 1..=n {
            let p =
                (V::from(w[0]) + (V::from(w[1]) - V::from(w[0])) * (i as f64 / n as f64)).point();
            if out.last() != Some(&p) {
                out.push(p);
            }
        }
    }
    out
}
/// Open at the scarf's end, walk backwards by length, rotate, then repeat the ramp at the end.
pub(super) fn add(poly: &[Point], end: V, length: f64, s: &ScarfSettings) -> (Polygon, PathSeam) {
    let closed = geometry::open(poly, end);
    let total = polyline_length(&closed) / SCALE;
    let length = length.min(total);
    let (i, start) = geometry::along(&closed, (total - length).max(0.0));
    let mut ramp = vec![start];
    ramp.extend_from_slice(&closed[i + 1..]);
    ramp.dedup();
    let ramp = subdivide(&ramp, s.max_segment_length.max(1e-4));
    let ramp_length = polyline_length(&ramp) / SCALE;
    let mut points = ramp.clone();
    points.extend_from_slice(&closed[1..=i]);
    if points.last() != Some(&start) {
        points.push(start);
    }
    let ramp_count = ramp.len();
    let middle_end = points.len();
    points.extend_from_slice(&ramp[1..]);
    let mut samples = vec![ScarfSample::default(); points.len()];
    let mut d = 0.0;
    for k in 0..ramp_count {
        if k > 0 {
            d += ramp[k - 1].distance_to(ramp[k]) / SCALE;
        }
        let t = if k + 1 == ramp_count {
            1.0
        } else if ramp_length > 0.0 {
            (d / ramp_length).min(1.0)
        } else {
            1.0
        };
        samples[k] = ScarfSample {
            height: s.start_height + t * (1.0 - s.start_height),
            extrusion: if s.orca {
                s.start_height + t * (1. - s.start_height)
            } else {
                t
            },
        };
        if k > 0 {
            samples[middle_end + k - 1] = ScarfSample {
                height: 1.0,
                extrusion: (1.0 - t) * if s.orca { 1. - s.start_height } else { 1. },
            };
        }
    }
    if s.flow_ratio != 1. {
        for sample in samples.iter_mut().take(ramp_count).skip(1) {
            sample.extrusion *= s.flow_ratio;
        }
        for sample in &mut samples[middle_end..] {
            sample.extrusion *= s.flow_ratio;
        }
    }
    (
        points,
        PathSeam {
            samples,
            wipe: None,
            original_clockwise: false,
            loop_length_mm: None,
            scarf_speed: None,
            scarf_ranges: Some([ramp_count - 1, middle_end - 1]),
        },
    )
}

/// Apply the same seam gap to ordinary and scarf paths. Modulation follows the clipped endpoint.
pub(super) fn gap(points: &mut Polygon, meta: &mut Option<Box<PathSeam>>, gap: f64) {
    let total = polyline_length(points) / SCALE;
    if gap >= total {
        points.clear();
        *meta = None;
        return;
    }
    if gap > 0.0 {
        let (i, p) = geometry::along(points, total - gap);
        let old_a = points[i];
        let old_b = points[i + 1];
        if let Some(m) = meta
            && !m.samples.is_empty()
        {
            let len = old_a.distance_to(old_b);
            let t = if len > 0.0 {
                old_a.distance_to(p) / len
            } else {
                0.0
            };
            let a = m.samples[i];
            let b = m.samples[i + 1];
            m.samples.truncate(i + 2);
            m.samples[i + 1] = ScarfSample {
                height: a.height + (b.height - a.height) * t,
                extrusion: a.extrusion + (b.extrusion - a.extrusion) * t,
            };
        }
        points.truncate(i + 2);
        points[i + 1] = p;
    } else if gap < 0.0 && total > 0.0 {
        let (i, p) = geometry::along(points, (-gap).min(total));
        let mut extra = points[1..=i].to_vec();
        extra.push(p);
        if let Some(m) = meta
            && !m.samples.is_empty()
        {
            let mut samples = m.samples[1..=i].to_vec();
            samples.push(m.samples[i + 1]);
            m.samples.extend(samples);
        }
        points.extend(extra);
    }
}

pub(super) fn wipe(points: &[Point], _poly: &[Point], length: f64) -> Option<Point> {
    if polyline_length(points) / SCALE <= 2.5 * length || points.len() < 2 {
        return None;
    }
    let current = V::from(*points.last()?);
    let next = V::from(points[0]);
    let prev = V::from(geometry::along(points, length).1);
    let a = next - current;
    let b = prev - current;
    // Equivalent to rotating the end-to-start vector by the signed wedge angle upstream.
    let direction = if a.norm() > 1e-8 {
        b.unit()
    } else {
        let incoming = current - V::from(points[points.len() - 2]);
        let outgoing = V::from(points[1]) - current;
        (incoming.normal() + outgoing.normal()).unit() * -1.0
    };
    if direction.norm() < 1e-8 {
        return None;
    }
    // wipe_hide_seam rotates by the full wedge angle. Its winding adjustment adds
    // a whole turn, so it does not clip the resulting move against material or holes.
    Some((current + direction * length).point())
}

/// Orca tests the whole loop with nozzle-length arms, then the actual seam's unsupported width.
pub(super) fn conditional(
    poly: &[Point],
    seam: V,
    lower: Option<&[crate::geometry::ExPolygon]>,
    width: f64,
    nozzle: f64,
    s: &ScarfSettings,
) -> bool {
    if poly.len() < 3 {
        return false;
    }
    let perimeter = crate::geometry::polygon_length(poly) / SCALE;
    if perimeter < 2. * nozzle {
        return false;
    }
    for i in 0..poly.len() {
        let p = V::from(poly[i]);
        let arm = |forward: bool| {
            let mut distance = 0.;
            let mut j = i;
            for _ in 0..poly.len() {
                let next = if forward {
                    (j + 1) % poly.len()
                } else {
                    (j + poly.len() - 1) % poly.len()
                };
                distance += poly[j].distance_to(poly[next]) / SCALE;
                j = next;
                if distance >= nozzle {
                    break;
                }
            }
            (V::from(poly[j]) - p).unit()
        };
        let angle = arm(false).dot(arm(true)).clamp(-1., 1.).acos().to_degrees();
        if angle < s.angle_threshold {
            return false;
        }
    }
    if s.overhang_threshold > 0.
        && let Some(lower) = lower
    {
        let inside = lower.iter().any(|e| {
            geometry::contains(&e.contour, seam)
                && !e.holes.iter().any(|h| geometry::contains(h, seam))
        });
        let distance = lower
            .iter()
            .flat_map(|e| e.polygons())
            .map(|p| {
                let p: Vec<_> = p.iter().copied().map(V::from).collect();
                (geometry::closest(&p, seam).point - seam).norm()
            })
            .fold(f64::INFINITY, f64::min);
        // Orca's estimator reports a signed distance at the boundary (not the bead centre).
        let unsupported = if inside {
            width * 0.5 - distance
        } else {
            width * 0.5 + distance
        };
        if unsupported >= s.overhang_threshold * width {
            return false;
        }
    }
    true
}
