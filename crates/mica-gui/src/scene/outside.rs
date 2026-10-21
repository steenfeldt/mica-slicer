// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/slic3r/GUI/Selection.cpp :: Selection::scale_to_fit_print_volume (fit_rectangle)
// Changes: One object instead of the selection; the factor is computed here and the app applies it (uniform scale about the object's centre, then centred on the bed and dropped onto it, one undo step); only offered when the object is larger than the volume; rectangular volume only (a non-rectangular bed uses its bounding box, as upstream's fit_rectangle does for BuildVolume::Type::Convex/Custom).

//! Which part of an object lies outside the printable volume, past which edge and how far:
//! what the plate warning says ("Bracket sticks out 12.4 mm past the back edge and 3.1 mm
//! above the maximum height") and what "Scale to fit" needs. The 3D view marks the same part
//! per fragment (`render/common.wgsl`, `outside_volume`), against the same outline,
//! exclusion zones and height.

use super::{Bed, Object, Placement};
use glam::{Vec2, Vec3};

/// How far (mm) past the volume still counts as inside: `Bed::placement`'s tolerance.
const TOLERANCE: f32 = 0.01;

/// Where an object reaches outside the printable volume.
#[derive(Clone, Debug, PartialEq)]
pub struct Outside {
    /// The bed edges it passes ("left", "right", "front", "back"; on a non-rectangular bed
    /// also "back-right" and the like) and how far (mm), farthest first.
    pub sides: Vec<(&'static str, f32)>,
    /// How far (mm) it rises above the maximum print height (0 when it does not).
    pub above: f32,
    /// It reaches into an exclusion zone of the bed.
    pub excluded: bool,
    /// Wider or deeper than the bed: moving cannot bring it inside ("Scale to fit" can).
    pub too_wide: bool,
    /// Taller than the maximum print height.
    pub too_tall: bool,
}

/// Where object `o` (bed coordinates, as on its plate) reaches outside `bed`'s printable
/// volume; `None` when it is inside (an object that only sinks into the bed has its own
/// warning).
pub fn outside(o: &Object, bed: &Bed) -> Option<Outside> {
    let (min, max) = o.bounds();
    if bed.placement(min, max) == Placement::Inside {
        return None;
    }
    let size = max - min;
    let bed_size = bed.size();
    let too_wide = size.x > bed_size.x + TOLERANCE || size.y > bed_size.y + TOLERANCE;
    let too_tall = size.z > bed.height + TOLERANCE;
    let above = (max.z - bed.height).max(0.);
    let above = if above > TOLERANCE { above } else { 0. };
    let t = o.transform();
    let placed = || {
        o.mesh
            .vertices
            .iter()
            .map(move |v| t.transform_point3(Vec3::from(*v)).truncate())
    };
    let mut sides: Vec<(&str, f32)> = if is_rectangle(bed) {
        [
            ("left", bed.min.x - min.x),
            ("right", max.x - bed.max.x),
            ("front", bed.min.y - min.y),
            ("back", max.y - bed.max.y),
        ]
        .into_iter()
        .filter(|(_, d)| *d > TOLERANCE)
        .collect()
    } else {
        polygon_side(bed, placed()).into_iter().collect()
    };
    sides.sort_by(|a, b| b.1.total_cmp(&a.1));
    let excluded = bed.exclusions.iter().any(|zone| {
        placed().any(|p| inside_by(zone, p, TOLERANCE)) || {
            let hull: Vec<[f32; 2]> = o
                .footprint_hull()
                .into_iter()
                .map(|[x, y]| [x as f32, y as f32])
                .collect();
            hull.len() >= 3
                && zone
                    .iter()
                    .any(|z| inside_by(&hull, Vec2::from(*z), TOLERANCE))
        }
    });
    let out = Outside {
        sides,
        above,
        excluded,
        too_wide,
        too_tall,
    };
    (!out.sides.is_empty() || out.above > 0. || out.excluded).then_some(out)
}

/// "Bracket sticks out 12.4 mm past the back edge and 3.1 mm above the maximum height",
/// with "; it is larger than the bed" when moving cannot fit it.
pub fn describe(name: &str, o: &Outside) -> String {
    let mut parts: Vec<String> = o
        .sides
        .iter()
        .map(|(side, d)| format!("{d:.1} mm past the {side} edge"))
        .collect();
    if o.above > 0. {
        parts.push(format!("{:.1} mm above the maximum height", o.above));
    }
    let mut text = if parts.is_empty() {
        format!("{name} reaches into an excluded area of the bed")
    } else {
        let mut t = format!("{name} sticks out {}", join(&parts));
        if o.excluded {
            t.push_str(" and reaches into an excluded area of the bed");
        }
        t
    };
    if o.too_wide {
        text.push_str("; it is larger than the bed");
    } else if o.too_tall {
        text.push_str("; it is taller than the printer can print");
    }
    text
}

/// "a", "a and b", "a, b and c".
fn join(parts: &[String]) -> String {
    match parts {
        [] => String::new(),
        [a] => a.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// The uniform factor that fits object `o` into the printable volume (PrusaSlicer's
/// `fit_rectangle`: the smallest of bed size / object size per axis, with 1/100 mm of slack
/// on both XY sides against rounding); `None` when it fits already.
pub fn fit_scale(o: &Object, bed: &Bed) -> Option<f32> {
    let (min, max) = o.bounds();
    let size = max - min;
    let bed_size = bed.size();
    let s = (bed_size.x / (size.x + 0.02))
        .min(bed_size.y / (size.y + 0.02))
        .min(bed.height / size.z.max(1e-6));
    (s.is_finite() && s > 0. && s < 1.).then_some(s)
}

/// Whether the bed outline is its own bounding rectangle (the edge distances are then the
/// bounding-box overruns).
fn is_rectangle(bed: &Bed) -> bool {
    bed.polygon.len() == 4
        && bed.polygon.iter().all(|p| {
            ((p[0] - bed.min.x).abs() < 1e-3 || (p[0] - bed.max.x).abs() < 1e-3)
                && ((p[1] - bed.min.y).abs() < 1e-3 || (p[1] - bed.max.y).abs() < 1e-3)
        })
}

/// A non-rectangular bed: the point farthest past the outline, by its distance to it, and
/// the edge it faces seen from the bed's centre (in the bed's proportions, eight directions).
fn polygon_side(bed: &Bed, points: impl Iterator<Item = Vec2>) -> Option<(&'static str, f32)> {
    let centre = ((bed.min + bed.max) * 0.5).truncate();
    let half = (bed.size() * 0.5).max(Vec2::splat(1e-3));
    let (p, d) = points
        .map(|p| (p, signed_distance(&bed.polygon, p)))
        .max_by(|a, b| a.1.total_cmp(&b.1))?;
    if d <= TOLERANCE {
        return None;
    }
    let r = (p - centre) / half;
    let sector = ((r.y.atan2(r.x) / std::f32::consts::FRAC_PI_4).round() as i32).rem_euclid(8);
    let name = [
        "right",
        "back-right",
        "back",
        "back-left",
        "left",
        "front-left",
        "front",
        "front-right",
    ][sector as usize];
    Some((name, d))
}

/// Whether `p` lies inside `polygon` by more than `margin`.
fn inside_by(polygon: &[[f32; 2]], p: Vec2, margin: f32) -> bool {
    polygon.len() >= 3 && signed_distance(polygon, p) < -margin
}

/// Distance from `p` to the polygon's outline, negative inside (the crossing rule, as the
/// shader's `polygon_distance`).
fn signed_distance(polygon: &[[f32; 2]], p: Vec2) -> f32 {
    let mut inside = false;
    let mut d = f32::INFINITY;
    let n = polygon.len();
    for k in 0..n {
        let a = Vec2::from(polygon[k]);
        let b = Vec2::from(polygon[(k + 1) % n]);
        let ab = b - a;
        let t = ((p - a).dot(ab) / ab.length_squared().max(1e-12)).clamp(0., 1.);
        d = d.min(p.distance(a + ab * t));
        if (a.y > p.y) != (b.y > p.y) && p.x < a.x + (p.y - a.y) / (b.y - a.y) * ab.x {
            inside = !inside;
        }
    }
    if inside { -d } else { d }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mica_core::config::Config;

    fn bed(shape: &str, height: f64, exclude: Option<&str>) -> Bed {
        let mut c = Config::defaults();
        c.set_raw("bed_shape", shape).unwrap();
        c.set_raw("max_print_height", &height.to_string()).unwrap();
        if let Some(e) = exclude {
            c.set_raw("bed_exclude_area", e).unwrap();
        }
        Bed::from_config(&c)
    }
    /// A box `size` mm with its minimum corner at `at` (bed mm).
    fn block(size: [f32; 3], at: [f32; 3]) -> Object {
        let mesh = mica_core::mesh_edit::cuboid(size);
        let mut o = Object::new(mica_core::input::InputObject::plain(
            "Bracket",
            0,
            0,
            mesh,
            mica_core::print_job::ToolAssignments([Some(0); 5]),
            Config::defaults(),
        ));
        let (min, _) = o.bounds();
        o.position += Vec3::from(at) - min;
        o
    }

    #[test]
    fn sides_and_height_of_the_outside_part() {
        let bed = bed("0x0,250x0,250x210,0x210", 220., None);
        // 12.4 mm past the back edge and 3.1 mm above the maximum height.
        let o = block([40., 40., 223.1], [100., 210. + 12.4 - 40., 0.]);
        let out = outside(&o, &bed).unwrap();
        assert_eq!(out.sides.len(), 1);
        assert_eq!(out.sides[0].0, "back");
        assert!((out.sides[0].1 - 12.4).abs() < 1e-3, "{out:?}");
        assert!((out.above - 3.1).abs() < 1e-3, "{out:?}");
        assert!(out.too_tall && !out.too_wide, "taller than the volume");
        assert_eq!(
            describe("Bracket", &out),
            "Bracket sticks out 12.4 mm past the back edge and 3.1 mm above the maximum \
             height; it is taller than the printer can print"
        );
        // Past the left and the front edge, farthest first; it fits when moved.
        let o = block([30., 30., 10.], [-5., -8., 0.]);
        let out = outside(&o, &bed).unwrap();
        assert_eq!(
            out.sides.iter().map(|(s, _)| *s).collect::<Vec<_>>(),
            ["front", "left"]
        );
        assert!(!out.too_wide && !out.too_tall);
        assert_eq!(
            describe("Bracket", &out),
            "Bracket sticks out 8.0 mm past the front edge and 5.0 mm past the left edge"
        );
        // Wider than the bed.
        let o = block([260., 30., 10.], [-5., 10., 0.]);
        let out = outside(&o, &bed).unwrap();
        assert!(out.too_wide);
        assert!(describe("Bracket", &out).ends_with("; it is larger than the bed"));
        // Inside: nothing to say.
        assert_eq!(outside(&block([30., 30., 10.], [10., 10., 0.]), &bed), None);
    }

    #[test]
    fn round_beds_and_exclusion_zones() {
        // An octagon around (100, 100) with radius 100.
        let pts: Vec<String> = (0..8)
            .map(|k| {
                let a = std::f64::consts::TAU * k as f64 / 8. + std::f64::consts::PI / 8.;
                format!("{:.4}x{:.4}", 100. + 100. * a.cos(), 100. + 100. * a.sin())
            })
            .collect();
        let round = bed(&pts.join(","), 200., None);
        // Inside the bounding box but past the slanted edge at the back right.
        let o = block([10., 10., 10.], [175., 175., 0.]);
        let out = outside(&o, &round).unwrap();
        assert_eq!(out.sides.len(), 1, "{out:?}");
        assert_eq!(out.sides[0].0, "back-right");
        assert!((out.sides[0].1 - 27.82).abs() < 0.05, "{out:?}");
        assert!(
            describe("Bracket", &out)
                .starts_with("Bracket sticks out 27.8 mm past the back-right edge")
        );
        // An exclusion zone in a corner of a rectangular bed.
        let zoned = bed("0x0,250x0,250x210,0x210", 220., Some("0x0,30x0,30x30,0x30"));
        let o = block([20., 20., 10.], [20., 20., 0.]);
        let out = outside(&o, &zoned).unwrap();
        assert!(out.excluded && out.sides.is_empty());
        assert_eq!(
            describe("Bracket", &out),
            "Bracket reaches into an excluded area of the bed"
        );
        assert_eq!(
            outside(&block([20., 20., 10.], [40., 40., 0.]), &zoned),
            None
        );
    }

    #[test]
    fn fit_scale_is_the_tightest_axis() {
        let bed = bed("0x0,250x0,250x210,0x210", 220., None);
        let o = block([300., 100., 50.], [0., 0., 0.]);
        let s = fit_scale(&o, &bed).unwrap();
        assert!((s - 250. / 300.02).abs() < 1e-5);
        let tall = block([50., 50., 440.], [0., 0., 0.]);
        assert!((fit_scale(&tall, &bed).unwrap() - 0.5).abs() < 1e-5);
        assert_eq!(fit_scale(&block([50., 50., 50.], [0., 0., 0.]), &bed), None);
    }
}
