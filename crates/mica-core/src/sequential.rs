// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/ArrangeHelper.cpp :: get_printer_geometry, get_objects_to_print, check_seq_conflict
// Source: prusaslicer@b028299c77 src/libseqarrange/src/seq_interface.cpp :: check_ScheduledObjectsForSequentialConflict
// Source: prusaslicer@b028299c77 src/libseqarrange/src/seq_preprocess.cpp :: decimate_PolygonForSequentialSolver, extend_PolygonConvexUnreachableZone, extend_PolygonBoxUnreachableZone, prepare_ExtruderPolygons
// Source: prusaslicer@b028299c77 src/libseqarrange/src/seq_sequential.cpp :: check_PointsOutsidePolygons, check_PolygonLineIntersections, lines_intersect_open
// Source: prusaslicer@b028299c77 resources/data/printer_gantries/geometries.json
// Source: prusaslicer@b028299c77 src/libslic3r/TriangleMesh.cpp :: its_convex_hull_2d_above
// Source: prusaslicer@b028299c77 src/libslic3r/PrintConfig.cpp :: min_object_distance
// Source: prusaslicer@b028299c77 src/slic3r/GUI/GLCanvas3D.cpp :: GLCanvas3D::_set_warning_notification
// Source: orca@8500fcdcca src/libslic3r/Print.cpp :: Print::sequential_print_clearance_valid, Print::validate, Print::object_skirt_offset
// Source: orca@8500fcdcca src/libslic3r/PrintConfig.cpp :: min_object_distance
// Changes: One module for both upstreams' "print one object after another" checks, in mm and f64. Prusa: the
// Changes: gantry slices are a table instead of a JSON file read at run time; the solver's 0.1 mm truncation
// Changes: and redundant-zone simplification are not reproduced; every conflicting pair is found (the message
// Changes: names the first, as upstream). Orca: offset-hull intersection is an exact convex distance test.
// Changes: Engine choices read mica_slicing_rules (mica_core::rules), not gcode_flavor or the presence of Orca keys (docs/slicing-rules.md).

//! Sequential printing ("complete individual objects", Orca's "print sequence: by object"):
//! whether a plate can be printed one object after another without the print head hitting a
//! finished object, and how far apart the arrange has to keep objects.
//!
//! PrusaSlicer 2.9 models the print head and gantry of its printers as horizontal slices
//! (`resources/data/printer_gantries/geometries.json`, matched against `printer_notes`); a
//! printer without an entry gets a cylinder of `extruder_clearance_radius` plus a gantry bar at
//! `extruder_clearance_height`. A later object may not lie where the head would be while it
//! prints, given the parts of earlier objects standing above each slice. PrusaSlicer reports a
//! conflict after slicing ("Extruder will crash into … while printing …") and still exports, so
//! Mica treats it as a warning.
//!
//! OrcaSlicer refuses to slice instead: each object's convex hull, grown by half the
//! `extruder_clearance_radius`, may not touch another's or a bed exclusion zone, and an object
//! may not be taller than `extruder_clearance_height_to_lid` (the last one may use the full
//! printable height), or `extruder_clearance_height_to_rod` when a later object shares its Y
//! range (the X rod would sweep over it).

use crate::config::Config;
use crate::mesh::TriangleMesh;

type P = [f64; 2];

/// Sequential printing is selected: PrusaSlicer's `complete_objects`, Orca's
/// `print_sequence = by object` (imported as `complete_objects`).
pub fn enabled(c: &Config) -> bool {
    c.bool("complete_objects")
        .unwrap_or_else(|| c.raw("print_sequence").as_deref().map(str::trim) == Some("by object"))
}

/// Whether `c` slices with OrcaSlicer's rules (`mica_slicing_rules`).
pub fn orca_rules(c: &Config) -> bool {
    crate::rules::Rules::of(c).is_orca()
}

fn number(c: &Config, key: &str) -> Option<f64> {
    c.float(key)
        .or_else(|| c.raw(key).and_then(|v| v.trim().parse().ok()))
}

/// The smallest distance the arrange keeps between objects (`min_object_distance`): the
/// extruder clearance radius when objects print one after another and it is larger than the
/// usual spacing (PrusaSlicer's `duplicate_distance`, Orca's fixed 6 mm), else that spacing.
pub fn min_object_distance(c: &Config) -> f64 {
    let spacing = if orca_rules(c) {
        6.
    } else {
        c.float("duplicate_distance").unwrap_or(6.)
    };
    match number(c, "extruder_clearance_radius") {
        Some(r) if enabled(c) && r > spacing => r,
        _ => spacing,
    }
}

/// One object instance as placed on the bed (Z = 0 is the bed), with all its model parts,
/// in print order.
#[derive(Clone, Copy)]
pub struct Instance<'a> {
    pub name: &'a str,
    pub parts: &'a [&'a TriangleMesh],
}

/// What the check found. `errors` stop slicing (Orca); `warnings` are reported but the
/// plate still slices (PrusaSlicer). `colliding` marks, per instance, whether it is part of
/// any conflict, for drawing.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Report {
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
    pub colliding: Vec<bool>,
}

/// Checks a plate for sequential printing; `instances` are in print order. Does nothing
/// (an empty report) unless sequential printing is enabled and there are two or more
/// instances.
pub fn check(instances: &[Instance<'_>], c: &Config) -> Report {
    let mut report = Report {
        colliding: vec![false; instances.len()],
        ..Report::default()
    };
    if !enabled(c) || instances.len() < 2 {
        return report;
    }
    if orca_rules(c) {
        orca_check(instances, c, &mut report);
    } else {
        prusa_check(instances, c, &mut report);
    }
    report
}

/// The clearance outline drawn around an instance on the bed: its convex hull grown by half
/// the extruder clearance radius (Orca's sequential-print contour; two outlines that touch
/// mean the objects are closer than the radius).
pub fn clearance_outline(instance: &Instance<'_>, c: &Config) -> Vec<P> {
    let hull = hull_above(instance.parts, f64::NEG_INFINITY);
    let r = 0.5 * number(c, "extruder_clearance_radius").unwrap_or(0.).max(0.);
    rounded_offset(&hull, r)
}

// ---------------------------------------------------------------------------------------
// PrusaSlicer 2.9: gantry slices (libseqarrange's conflict check).

#[derive(Clone, Copy, PartialEq)]
enum Shape {
    Convex,
    Box,
}

/// `(height, shape, polygons)` per slice, in the JSON's order.
type GantrySlice = (f64, Shape, &'static [&'static [P]]);

const C: Shape = Shape::Convex;
const B: Shape = Shape::Box;

/// `geometries.json`: the first entry whose `printer_notes_regex` matches wins.
const GANTRIES: &[(&str, &[GantrySlice])] = &[
    (
        "SEQ_ARRANGE_MODEL_COREONE_INDX",
        &[
            (0., C, &[&[[-5., -5.], [5., -5.], [5., 5.], [-5., 5.]]]),
            (
                2.,
                C,
                &[&[[-25., -75.], [27., -75.], [27., 18.], [-25., 18.]]],
            ),
            (
                23.,
                C,
                &[&[[-27., -52.], [25., -52.], [25., 21.], [-27., 21.]]],
            ),
            (
                23.,
                C,
                &[&[[-27., -90.], [28., -90.], [28., -47.], [-27., -47.]]],
            ),
            (
                29.,
                B,
                &[&[[-300., -58.], [300., -58.], [300., -36.], [-300., -36.]]],
            ),
            (
                120.,
                B,
                &[&[[-300., -300.], [300., -300.], [300., 10.], [-300., 10.]]],
            ),
            (
                170.,
                B,
                &[&[[-300., -300.], [300., -300.], [300., 300.], [-300., 300.]]],
            ),
        ],
    ),
    (
        "PRINTER_MODEL_MK4(?!S)",
        &[
            (0., C, &[&[[-5., -5.], [5., -5.], [5., 5.], [-5., 5.]]]),
            (
                3.,
                C,
                &[
                    &[[-9., -17.], [40., -17.], [40., 44.], [-9., 44.]],
                    &[[-36., -44.], [40., -44.], [40., -13.], [-36., -13.]],
                ],
            ),
            (
                22.,
                C,
                &[
                    &[[-41., -45.], [16., -45.], [16., 22.], [-41., 22.]],
                    &[[11., -45.], [39., -45.], [39., 45.], [11., 45.]],
                ],
            ),
            (
                11.,
                B,
                &[&[[-300., -23.], [300., -23.], [300., -35.], [-300., -35.]]],
            ),
            (
                13.,
                B,
                &[
                    &[[-13., -84.], [11., -84.], [11., -38.], [-13., -38.]],
                    &[[11., -300.], [300., -300.], [300., -84.], [11., -84.]],
                ],
            ),
        ],
    ),
    (
        "PRINTER_MODEL_MK4S",
        &[
            (
                0.,
                C,
                &[&[[-29., -19.], [28., -19.], [28., 37.], [-29., 37.]]],
            ),
            (
                5.,
                C,
                &[&[[-37., -40.], [34., -40.], [34., 43.], [-37., 43.]]],
            ),
            (
                7.,
                C,
                &[
                    &[[-48., -15.], [24., -15.], [24., 66.], [-48., 66.]],
                    &[[-37., -47.], [40., -47.], [40., 21.], [-37., 21.]],
                ],
            ),
            (
                32.,
                C,
                &[
                    &[[-49., 7.], [27., 7.], [27., 83.], [-49., 83.]],
                    &[[-42., -46.], [40., -46.], [40., 14.], [-42., 14.]],
                ],
            ),
            (
                11.,
                B,
                &[&[[-300., -38.], [300., -38.], [300., -22.], [-300., -22.]]],
            ),
            (
                12.,
                B,
                &[
                    &[[-14., -85.], [11., -85.], [11., -39.], [-14., -39.]],
                    &[[-14., -300.], [300., -300.], [300., -39.], [-14., -39.]],
                ],
            ),
        ],
    ),
    (
        "PRINTER_MODEL_MK3",
        &[
            (
                0.,
                C,
                &[
                    &[[-5., -5.], [5., -5.], [5., 5.], [-5., 5.]],
                    &[[-30., -12.], [-14., -12.], [-14., 2.], [-30., 2.]],
                ],
            ),
            (
                2.,
                C,
                &[&[[-20., -38.], [44., -38.], [44., 18.], [-20., 18.]]],
            ),
            (
                6.,
                C,
                &[
                    &[[-34., -43.], [37., -43.], [37., 16.], [-34., 16.]],
                    &[[-45., 9.], [37., 9.], [37., 69.], [-45., 69.]],
                ],
            ),
            (
                11.,
                B,
                &[
                    &[[-8., -82.], [8., -82.], [8., -36.], [-8., -36.]],
                    &[[-8., -82.], [250., -82.], [250., -300.], [-8., -300.]],
                ],
            ),
            (
                17.,
                B,
                &[&[[-300., -35.], [300., -35.], [300., -21.], [-300., -21.]]],
            ),
        ],
    ),
    (
        "PRINTER_MODEL_MINI",
        &[
            (
                0.,
                C,
                &[
                    &[[-5., -5.], [5., -5.], [5., 5.], [-5., 5.]],
                    &[[24., -3.], [35., -3.], [35., 10.], [24., 10.]],
                    &[[-5., 4.], [5., 4.], [5., 18.], [-5., 18.]],
                ],
            ),
            (
                3.,
                C,
                &[&[[-16., -44.], [37., -44.], [37., 31.], [-16., 31.]]],
            ),
            (
                10.,
                C,
                &[
                    &[[-10., -88.], [10., -88.], [10., -38.], [-10., -38.]],
                    &[[-17., -44.], [43., -44.], [43., 33.], [-17., 33.]],
                ],
            ),
            (
                22.,
                B,
                &[&[[-200., -28.], [200., -28.], [200., -14.], [-200., -14.]]],
            ),
            (
                100.,
                B,
                &[&[[-200., -200.], [10., -200.], [10., 10.], [-200., 10.]]],
            ),
        ],
    ),
    (
        "PRINTER_MODEL_XL",
        &[
            (0., C, &[&[[-5., -5.], [5., -5.], [5., 5.], [-5., 5.]]]),
            (
                2.,
                C,
                &[
                    &[[-10., -47.], [34., -47.], [34., 16.], [-10., 16.]],
                    &[[-43., 11.], [28., 11.], [28., 66.], [-43., 66.]],
                ],
            ),
            (
                23.,
                C,
                &[
                    &[[-43., 11.], [28., 11.], [28., 66.], [-43., 66.]],
                    &[[-33., -37.], [43., -37.], [43., 18.], [-33., 18.]],
                    &[[-13., -68.], [47., -68.], [47., -30.], [-13., -30.]],
                ],
            ),
            (
                19.,
                B,
                &[&[[-400., 24.], [400., 24.], [400., 59.], [-400., 59.]]],
            ),
            (
                220.,
                B,
                &[&[[-400., -400.], [400., -400.], [400., 10.], [-400., 10.]]],
            ),
            (
                280.,
                B,
                &[&[[-400., -400.], [400., -400.], [400., 400.], [-400., 400.]]],
            ),
        ],
    ),
    (
        "SEQ_ARRANGE_MODEL_COREONEL",
        &[
            (
                0.,
                C,
                &[&[[-33., -44.], [27., -44.], [27., 19.], [-33., 19.]]],
            ),
            (
                2.,
                C,
                &[&[[-40., -73.], [27., -73.], [27., 20.], [-40., 20.]]],
            ),
            (
                23.,
                C,
                &[&[[-44., -52.], [35., -52.], [35., 23.], [-44., 23.]]],
            ),
            (
                23.,
                C,
                &[&[[-38., -96.], [25., -96.], [25., -47.], [-38., -47.]]],
            ),
            (
                33.,
                B,
                &[&[[-400., -49.], [400., -49.], [400., -24.], [-400., -24.]]],
            ),
            (
                120.,
                B,
                &[&[[-400., -400.], [400., -400.], [400., 10.], [-400., 10.]]],
            ),
            (
                185.,
                B,
                &[&[[-400., -400.], [400., -400.], [400., 400.], [-400., 400.]]],
            ),
        ],
    ),
    (
        "PRINTER_MODEL_COREONE",
        &[
            (
                0.,
                C,
                &[&[[-33., -37.], [27., -37.], [27., 19.], [-33., 19.]]],
            ),
            (
                2.,
                C,
                &[&[[-40., -69.], [27., -69.], [27., 20.], [-40., 20.]]],
            ),
            (
                23.,
                C,
                &[&[[-44., -52.], [35., -52.], [35., 23.], [-44., 23.]]],
            ),
            (
                23.,
                C,
                &[&[[-37., -98.], [26., -98.], [26., -47.], [-37., -47.]]],
            ),
            (
                33.,
                B,
                &[&[[-300., -49.], [300., -49.], [300., -19.], [-300., -19.]]],
            ),
            (
                120.,
                B,
                &[&[[-300., -300.], [300., -300.], [300., 10.], [-300., 10.]]],
            ),
            (
                170.,
                B,
                &[&[[-300., -300.], [300., -300.], [300., 300.], [-300., 300.]]],
            ),
        ],
    ),
    (
        "PRINTER_MODEL_HT90",
        &[
            (0., C, &[&[[-5., -5.], [5., -5.], [5., 5.], [-5., 5.]]]),
            (
                1.,
                C,
                &[
                    &[[-33., -2.], [33., -2.], [33., 48.], [-33., 48.]],
                    &[[-46., -37.], [46., -37.], [46., 2.], [-46., 2.]],
                ],
            ),
            (
                20.,
                C,
                &[&[[-55., -55.], [55., -55.], [55., 55.], [-55., 55.]]],
            ),
            (
                28.,
                C,
                &[&[[-76., -68.], [76., -68.], [76., 65.], [-76., 65.]]],
            ),
            (
                40.,
                C,
                &[&[[-120., -118.], [120., -118.], [120., 94.], [-120., 94.]]],
            ),
            (
                60.,
                B,
                &[&[[-170., -164.], [170., -164.], [170., 130.], [-170., 130.]]],
            ),
            (
                80.,
                B,
                &[&[[-400., -400.], [400., -400.], [400., 400.], [-400., 400.]]],
            ),
        ],
    ),
];

/// `.*TOKEN.*` against `printer_notes`; `(?!S)` is the MK4 entry's "not followed by S".
fn notes_match(pattern: &str, notes: &str) -> bool {
    match pattern.strip_suffix("(?!S)") {
        Some(token) => notes
            .match_indices(token)
            .any(|(i, _)| !notes[i + token.len()..].starts_with('S')),
        None => notes.contains(pattern),
    }
}

/// Slices by height, as `get_printer_geometry` builds its map: a repeated height keeps
/// the first entry's polygons.
fn printer_slices(c: &Config) -> Vec<(f64, Shape, Vec<Vec<P>>)> {
    let notes = c.string("printer_notes").unwrap_or("");
    let mut out: Vec<(f64, Shape, Vec<Vec<P>>)> = Vec::new();
    if !notes.is_empty()
        && let Some((_, slices)) = GANTRIES.iter().find(|(p, _)| notes_match(p, notes))
    {
        for (h, shape, polygons) in slices.iter() {
            if !out.iter().any(|s| s.0 == *h) {
                out.push((*h, *shape, polygons.iter().map(|p| p.to_vec()).collect()));
            }
        }
    }
    if out.is_empty() {
        let r = number(c, "extruder_clearance_radius")
            .unwrap_or(0.)
            .max(0.1);
        let h = number(c, "extruder_clearance_height")
            .unwrap_or(0.)
            .max(0.1);
        let bed = c.points("bed_shape").unwrap_or_default();
        let (lo, hi) = bed.iter().fold((f64::MAX, f64::MIN), |(lo, hi), p| {
            (lo.min(p.0), hi.max(p.0))
        });
        let x = if hi > lo { hi - lo } else { 0. };
        let square = |s: f64| vec![[-s, -s], [s, -s], [s, s], [-s, s]];
        out.push((0., Shape::Convex, vec![square(5.)]));
        if !out.iter().any(|s| s.0 == 1.) {
            out.push((1., Shape::Box, vec![square(r)]));
        }
        if !out.iter().any(|s| s.0 == h) {
            out.push((
                h,
                Shape::Box,
                vec![vec![[-x, -r], [x, -r], [x, r], [-x, r]]],
            ));
        }
    }
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    out
}

fn prusa_check(instances: &[Instance<'_>], c: &Config, report: &mut Report) {
    let slices = printer_slices(c);
    // Footprint (lowest convex level) and unreachable zones of every instance.
    let mut footprints = Vec::with_capacity(instances.len());
    let mut zones: Vec<Vec<Vec<P>>> = Vec::with_capacity(instances.len());
    for instance in instances {
        let mut footprint = None;
        let mut own = Vec::new();
        for (height, shape, polygons) in &slices {
            let hull = hull_above(instance.parts, *height);
            if hull.is_empty() {
                continue;
            }
            let decimated = decimate(&hull);
            for extruder in polygons {
                own.push(match shape {
                    Shape::Convex => minkowski(&decimated, extruder),
                    Shape::Box => box_sum(&decimated, extruder),
                });
            }
            if *shape == Shape::Convex && footprint.is_none() {
                footprint = Some(decimated);
            }
        }
        footprints.push(footprint.unwrap_or_default());
        zones.push(own);
    }
    // Earlier objects stand while later ones print: a later footprint inside, or crossing,
    // an earlier object's zones means the head hits that earlier object.
    let mut first = None;
    for (i, earlier) in zones.iter().enumerate() {
        for (j, later) in footprints.iter().enumerate().skip(i + 1) {
            let points = later
                .iter()
                .any(|p| earlier.iter().any(|z| strictly_inside(*p, z)));
            let lines = || {
                earlier.iter().any(|z| {
                    edges(z).any(|(a, b)| edges(later).any(|(p, q)| open_cross(a, b, p, q)))
                })
            };
            if points || lines() {
                report.colliding[i] = true;
                report.colliding[j] = true;
                // Upstream checks every pair for points before any pair for lines.
                let rank = (u8::from(!points), i, j);
                if first.is_none_or(|f: (u8, usize, usize)| rank < f) {
                    first = Some(rank);
                }
            }
        }
    }
    if let Some((_, i, j)) = first {
        report.warnings.push(format!(
            "Extruder will crash into {} while printing {}.",
            instances[i].name, instances[j].name
        ));
    }
}

/// `decimate_PolygonForSequentialSolver` (low precision, no extra safety): Douglas–Peucker
/// at 0.65 mm, then grown about its centre in 0.5 % steps until it contains the original;
/// fewer than four points become the bounding rectangle.
fn decimate(hull: &[P]) -> Vec<P> {
    let mut closed = hull.to_vec();
    closed.push(hull[0]);
    let mut out = douglas_peucker(&closed, 0.65);
    out.pop();
    let (min, max) = bbox(hull);
    if out.len() < 4 {
        return vec![
            [min[0], min[1]],
            [max[0], min[1]],
            [max[0], max[1]],
            [min[0], max[1]],
        ];
    }
    let center = [(min[0] + max[0]) / 2., (min[1] + max[1]) / 2.];
    for _ in 0..2000 {
        for p in &mut out {
            *p = [p[0] * 1.005, p[1] * 1.005];
        }
        let (a, b) = bbox(&out);
        let shift = [
            (a[0] + b[0]) / 2. - center[0],
            (a[1] + b[1]) / 2. - center[1],
        ];
        for p in &mut out {
            *p = [p[0] - shift[0], p[1] - shift[1]];
        }
        if hull.iter().all(|p| contains(&out, *p)) {
            break;
        }
    }
    out
}

fn douglas_peucker(points: &[P], tolerance: f64) -> Vec<P> {
    if points.len() < 3 {
        return points.to_vec();
    }
    let (a, b) = (points[0], points[points.len() - 1]);
    let (index, distance) = points[1..points.len() - 1]
        .iter()
        .enumerate()
        .map(|(i, p)| (i + 1, segment_distance(*p, a, b)))
        .fold((0, -1.), |best, d| if d.1 > best.1 { d } else { best });
    if distance > tolerance {
        let mut left = douglas_peucker(&points[..=index], tolerance);
        left.pop();
        left.extend(douglas_peucker(&points[index..], tolerance));
        left
    } else {
        vec![a, b]
    }
}

fn minkowski(a: &[P], b: &[P]) -> Vec<P> {
    let sums: Vec<P> = a
        .iter()
        .flat_map(|p| b.iter().map(move |q| [p[0] + q[0], p[1] + q[1]]))
        .collect();
    convex_hull(sums)
}

fn box_sum(a: &[P], b: &[P]) -> Vec<P> {
    let (amin, amax) = bbox(a);
    let (bmin, bmax) = bbox(b);
    let (x0, y0, x1, y1) = (
        amin[0] + bmin[0],
        amin[1] + bmin[1],
        amax[0] + bmax[0],
        amax[1] + bmax[1],
    );
    vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]]
}

/// `check_PointsOutsidePolygons`: inside every edge's half-plane of a CCW convex polygon,
/// the boundary counting as outside.
fn strictly_inside(p: P, polygon: &[P]) -> bool {
    polygon.len() >= 3
        && edges(polygon).all(|(a, b)| {
            let normal = [b[1] - a[1], -(b[0] - a[0])];
            normal[0] * (p[0] - a[0]) + normal[1] * (p[1] - a[1]) <= -1e-9
        })
}

/// `lines_intersect_open`: the open segments cross (touching ends do not count).
fn open_cross(a: P, b: P, p: P, q: P) -> bool {
    let u = [b[0] - a[0], b[1] - a[1]];
    let v = [q[0] - p[0], q[1] - p[1]];
    let den = u[0] * v[1] - u[1] * v[0];
    if den.abs() < 1e-12 {
        return false;
    }
    let w = [p[0] - a[0], p[1] - a[1]];
    let t = (w[0] * v[1] - w[1] * v[0]) / den;
    let s = (w[0] * u[1] - w[1] * u[0]) / den;
    let e = 1e-9;
    t > e && t < 1. - e && s > e && s < 1. - e
}

// ---------------------------------------------------------------------------------------
// OrcaSlicer: offset hulls and clearance heights (sequential_print_clearance_valid).

fn orca_check(instances: &[Instance<'_>], c: &Config, report: &mut Report) {
    if c.raw("timelapse_type").as_deref().map(str::trim) == Some("1") {
        report.errors.push(
            "Smooth mode of timelapse is not supported when \"by object\" sequence is enabled."
                .into(),
        );
        return;
    }
    if c.raw("enable_wrapping_detection")
        .as_deref()
        .is_some_and(|v| matches!(v.trim(), "1" | "true"))
    {
        report.errors.push(
            "Clumping detection is not supported when \"by object\" sequence is enabled.".into(),
        );
        return;
    }
    let radius = number(c, "extruder_clearance_radius").unwrap_or(40.);
    let lid = number(c, "extruder_clearance_height_to_lid").unwrap_or(120.);
    let rod = number(c, "extruder_clearance_height_to_rod").unwrap_or(40.);
    let printable = number(c, "max_print_height")
        .or_else(|| number(c, "printable_height"))
        .unwrap_or(f64::INFINITY);
    let nozzle_height = number(c, "nozzle_height").unwrap_or(2.5);
    let hulls: Vec<Vec<P>> = instances
        .iter()
        .map(|i| hull_above(i.parts, f64::NEG_INFINITY))
        .collect();
    let heights: Vec<f64> = instances.iter().map(|i| height(i.parts)).collect();
    let short = heights.iter().all(|h| *h < nozzle_height);
    let skirt = object_skirt_offset(c, radius, short);
    // Shrunk a tiny bit so objects arranged exactly at the radius do not collide.
    let distance = if short {
        (0.5f64 * 4.).max(skirt) - 0.1
    } else {
        0.5 * radius + skirt - 0.1
    };
    let exclusions: Vec<Vec<P>> = c
        .points("bed_exclude_area")
        .filter(|p| p.len() >= 3)
        .map(|p| p.into_iter().map(|(x, y)| [x, y]).collect())
        .into_iter()
        .collect();
    let mut messages = Vec::new();
    for (k, hull) in hulls.iter().enumerate() {
        if hull.is_empty() {
            continue;
        }
        if exclusions.iter().any(|e| polygons_overlap(hull, e)) {
            messages.push(format!(
                "{} is too close to exclusion area, there may be collisions when printing.",
                instances[k].name
            ));
        }
        // Each instance against the earlier ones; the first overlap is enough.
        if let Some(other) = (0..k)
            .find(|&o| !hulls[o].is_empty() && convex_distance(&hulls[o], hull) < 2. * distance)
        {
            messages.push(format!(
                "{} is too close to others, and collisions may be caused.",
                instances[k].name
            ));
            report.colliding[other] = true;
            report.colliding[k] = true;
        }
    }
    // Heights, in object order: the last object may use the whole printable height, the
    // others stay below the lid, or below the rod when a later object shares their Y range.
    let grown: Vec<(P, P)> = hulls
        .iter()
        .map(|h| {
            let (min, max) = bbox(h);
            (
                [min[0] - distance, min[1] - distance],
                [max[0] + distance, max[1] + distance],
            )
        })
        .collect();
    let inset = 0.5 * radius + skirt;
    for k in 0..instances.len() {
        if hulls[k].is_empty() {
            continue;
        }
        let (iy1, iy2) = (grown[k].0[1] + inset, grown[k].1[1] - inset);
        let mut limit = if k + 1 == instances.len() {
            printable
        } else {
            lid
        };
        if (k + 1..instances.len())
            .any(|i| !hulls[i].is_empty() && iy2.min(grown[i].1[1]) - iy1.max(grown[i].0[1]) > 0.)
        {
            limit = rod;
        }
        if limit < heights[k] - 1e-6 {
            messages.push(format!(
                "{} is too tall, and collisions will be caused.",
                instances[k].name
            ));
            report.colliding[k] = true;
        }
    }
    if !messages.is_empty() {
        report.errors.push(messages.join("\n"));
    }
}

/// `Print::object_skirt_offset`: room a per-object skirt needs beyond half the radius.
fn object_skirt_offset(c: &Config, radius: f64, short: bool) -> f64 {
    let loops = c.int("skirts").unwrap_or(0);
    let per_object = c.raw("skirt_type").as_deref().map(str::trim) == Some("per_object");
    if loops <= 0 || !per_object {
        return 0.;
    }
    let nozzle = (0..c.vector_len("nozzle_diameter").max(1))
        .filter_map(|i| c.float_at("nozzle_diameter", i))
        .fold(0.4f64, f64::max);
    let layer = c.float("first_layer_height").unwrap_or(0.2);
    let width = c
        .abs_value("first_layer_extrusion_width", nozzle)
        .filter(|w| *w > 0.)
        .unwrap_or(nozzle * 1.125);
    let spacing = width - layer * (1. - std::f64::consts::FRAC_PI_4);
    let skirt_width = width + (loops - 1) as f64 * spacing;
    let distance = c.float("skirt_distance").unwrap_or(0.);
    let max_layer = (0..c.vector_len("max_layer_height").max(1))
        .filter_map(|i| c.float_at("max_layer_height", i))
        .fold(0., f64::max);
    let nozzle_height = number(c, "nozzle_height").unwrap_or(2.5);
    let shield = c.string("draft_shield").is_some_and(|v| v == "enabled");
    if short {
        distance + skirt_width
    } else if shield || c.float("skirt_height").unwrap_or(1.) * max_layer > nozzle_height {
        distance + width
    } else if distance + skirt_width > radius / 2. {
        distance + skirt_width - radius / 2.
    } else {
        0.
    }
}

// ---------------------------------------------------------------------------------------
// Geometry in mm.

fn height(parts: &[&TriangleMesh]) -> f64 {
    parts
        .iter()
        .flat_map(|m| &m.vertices)
        .map(|v| f64::from(v[2]))
        .fold(0., f64::max)
}

/// `its_convex_hull_2d_above`: the hull of everything at or above `z`, cutting edges that
/// cross it (counter-clockwise, empty when nothing is that high).
fn hull_above(parts: &[&TriangleMesh], z: f64) -> Vec<P> {
    let mut points = Vec::new();
    for mesh in parts {
        for face in &mesh.faces {
            let v = face.map(|i| mesh.vertices[i as usize].map(f64::from));
            for e in 0..3 {
                let (p1, p2) = (v[(e + 2) % 3], v[e]);
                if (p1[2] < z && p2[2] > z) || (p2[2] < z && p1[2] > z) {
                    let t = (z - p1[2]) / (p2[2] - p1[2]);
                    points.push([p1[0] + (p2[0] - p1[0]) * t, p1[1] + (p2[1] - p1[1]) * t]);
                }
                if p2[2] >= z {
                    points.push([p2[0], p2[1]]);
                }
            }
        }
    }
    convex_hull(points)
}

/// Andrew's monotone chain, counter-clockwise, without collinear points.
fn convex_hull(mut points: Vec<P>) -> Vec<P> {
    points.sort_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
    points.dedup();
    if points.len() < 3 {
        return points;
    }
    let cross = |o: P, a: P, b: P| (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0]);
    let mut hull: Vec<P> = Vec::with_capacity(points.len() * 2);
    for pass in 0..2 {
        let start = hull.len();
        let iter: Box<dyn Iterator<Item = &P>> = if pass == 0 {
            Box::new(points.iter())
        } else {
            Box::new(points.iter().rev())
        };
        for &p in iter {
            while hull.len() >= start + 2
                && cross(hull[hull.len() - 2], hull[hull.len() - 1], p) <= 0.
            {
                hull.pop();
            }
            hull.push(p);
        }
        hull.pop();
    }
    hull
}

fn bbox(points: &[P]) -> (P, P) {
    points.iter().fold(
        ([f64::MAX, f64::MAX], [f64::MIN, f64::MIN]),
        |(min, max), p| {
            (
                [min[0].min(p[0]), min[1].min(p[1])],
                [max[0].max(p[0]), max[1].max(p[1])],
            )
        },
    )
}

fn edges(polygon: &[P]) -> impl Iterator<Item = (P, P)> + '_ {
    (0..polygon.len()).map(move |i| (polygon[i], polygon[(i + 1) % polygon.len()]))
}

fn segment_distance(p: P, a: P, b: P) -> f64 {
    let d = [b[0] - a[0], b[1] - a[1]];
    let len = d[0] * d[0] + d[1] * d[1];
    let t = if len > 0. {
        (((p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1]) / len).clamp(0., 1.)
    } else {
        0.
    };
    (p[0] - a[0] - t * d[0]).hypot(p[1] - a[1] - t * d[1])
}

/// Even–odd point in polygon (boundary undefined).
fn contains(polygon: &[P], p: P) -> bool {
    let mut inside = false;
    for (a, b) in edges(polygon) {
        if (a[1] > p[1]) != (b[1] > p[1])
            && p[0] < a[0] + (p[1] - a[1]) / (b[1] - a[1]) * (b[0] - a[0])
        {
            inside = !inside;
        }
    }
    inside
}

/// Two simple polygons share area (crossing edges or one inside the other).
fn polygons_overlap(a: &[P], b: &[P]) -> bool {
    edges(a).any(|(p, q)| edges(b).any(|(r, s)| open_cross(p, q, r, s)))
        || a.iter().any(|p| contains(b, *p))
        || b.iter().any(|p| contains(a, *p))
}

/// Distance between two convex polygons, zero when they overlap.
fn convex_distance(a: &[P], b: &[P]) -> f64 {
    if polygons_overlap(a, b) {
        return 0.;
    }
    let one = |a: &[P], b: &[P]| {
        a.iter()
            .flat_map(|p| edges(b).map(move |(q, r)| segment_distance(*p, q, r)))
            .fold(f64::INFINITY, f64::min)
    };
    one(a, b).min(one(b, a))
}

/// A convex polygon grown by `r` with round corners (8° steps), counter-clockwise.
fn rounded_offset(hull: &[P], r: f64) -> Vec<P> {
    if hull.len() < 3 || r <= 0. {
        return hull.to_vec();
    }
    let mut out = Vec::new();
    let n = hull.len();
    for i in 0..n {
        let (prev, p, next) = (hull[(i + n - 1) % n], hull[i], hull[(i + 1) % n]);
        let normal = |a: P, b: P| {
            let d = [b[0] - a[0], b[1] - a[1]];
            let l = d[0].hypot(d[1]).max(1e-12);
            [d[1] / l, -d[0] / l]
        };
        let (n1, n2) = (normal(prev, p), normal(p, next));
        let a1 = n1[1].atan2(n1[0]);
        let mut a2 = n2[1].atan2(n2[0]);
        while a2 < a1 {
            a2 += std::f64::consts::TAU;
        }
        let steps = ((a2 - a1) / 8f64.to_radians()).ceil().max(1.) as usize;
        for s in 0..=steps {
            let a = a1 + (a2 - a1) * s as f64 / steps as f64;
            out.push([p[0] + r * a.cos(), p[1] + r * a.sin()]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh_edit::cuboid;

    fn cube(x: f32, y: f32, h: f32) -> TriangleMesh {
        let mut m = cuboid([20., 20., h]);
        let (min, max) = m.bounds();
        m.translate([
            x - (min[0] + max[0]) / 2.,
            y - (min[1] + max[1]) / 2.,
            -min[2],
        ]);
        m
    }

    fn prusa(notes: &str) -> Config {
        let mut c = Config::defaults();
        c.set_raw("complete_objects", "1").unwrap();
        c.set_raw("printer_notes", notes).unwrap();
        c.set_raw("extruder_clearance_radius", "75").unwrap();
        c.set_raw("extruder_clearance_height", "33").unwrap();
        c.set_raw("bed_shape", "0x-4,250x-4,250x220,0x220").unwrap();
        c
    }

    fn run(c: &Config, cubes: &[TriangleMesh]) -> Report {
        let parts: Vec<Vec<&TriangleMesh>> = cubes.iter().map(|m| vec![m]).collect();
        let names = ["A", "B", "C", "D"];
        let instances: Vec<Instance<'_>> = parts
            .iter()
            .enumerate()
            .map(|(i, p)| Instance {
                name: names[i],
                parts: p,
            })
            .collect();
        check(&instances, c)
    }

    #[test]
    fn layer_by_layer_and_single_objects_are_not_checked() {
        let mut c = prusa("PRINTER_MODEL_COREONE");
        let cubes = [cube(50., 110., 20.), cube(60., 110., 20.)];
        assert!(run(&c, &cubes[..1]).warnings.is_empty());
        c.set_raw("complete_objects", "0").unwrap();
        assert!(run(&c, &cubes).warnings.is_empty());
    }

    #[test]
    fn core_one_gantry_spreads_and_crashes() {
        let c = prusa("Don't remove\nPRINTER_MODEL_COREONE\nPG");
        let spread = [
            cube(50., 110., 20.),
            cube(125., 110., 20.),
            cube(200., 110., 20.),
        ];
        assert_eq!(
            run(&c, &spread),
            Report {
                colliding: vec![false; 3],
                ..Report::default()
            }
        );
        // 30 mm apart: the head (x -40..27 at 2 mm) reaches the first cube.
        let close = [cube(50., 110., 20.), cube(80., 110., 20.)];
        let r = run(&c, &close);
        assert_eq!(r.warnings, ["Extruder will crash into A while printing B."]);
        assert_eq!(r.colliding, [true, true]);
        assert!(r.errors.is_empty());
        // The gantry bar (33 mm up, 19–49 mm in front of the nozzle, the whole bed wide)
        // sweeps over a later object in front of a taller earlier one, even far away in X.
        let tall = [cube(60., 160., 40.), cube(180., 110., 20.)];
        assert_eq!(run(&c, &tall).warnings.len(), 1);
        let low = [cube(60., 160., 20.), cube(180., 110., 20.)];
        assert!(run(&c, &low).warnings.is_empty());
    }

    #[test]
    fn gantry_entries_match_printer_notes() {
        assert!(notes_match(
            "PRINTER_MODEL_MK4(?!S)",
            "x\nPRINTER_MODEL_MK4\nPG"
        ));
        assert!(!notes_match("PRINTER_MODEL_MK4(?!S)", "PRINTER_MODEL_MK4S"));
        assert!(notes_match(
            "PRINTER_MODEL_XL",
            "PRINTER_VENDOR_PRUSA3D\nPRINTER_MODEL_XLIS"
        ));
        let core = printer_slices(&prusa("PRINTER_MODEL_COREONE"));
        // The second 23 mm entry is dropped, as std::map::insert does.
        assert_eq!(core.iter().filter(|s| s.0 == 23.).count(), 1);
        assert_eq!(core[2].2[0][1], [35., -52.]);
        let generic = printer_slices(&prusa("UNKNOWN"));
        assert_eq!(generic.len(), 3);
        assert_eq!(generic[1].2[0][2], [75., 75.]);
        assert_eq!(generic[2].0, 33.);
    }

    fn orca() -> Config {
        let mut c = Config::defaults();
        c.set_raw("complete_objects", "1").unwrap();
        c.set_raw("initial_layer_print_height", "0.25").unwrap();
        c.set_raw("mica_slicing_rules", "orcaslicer").unwrap();
        c.set_raw("extruder_clearance_radius", "72.5").unwrap();
        c.set_raw("max_print_height", "270").unwrap();
        c
    }

    #[test]
    fn orca_radius_lid_and_rod() {
        let mut c = orca();
        c.set_raw("extruder_clearance_height_to_rod", "27.5")
            .unwrap();
        c.set_raw("extruder_clearance_height_to_lid", "140")
            .unwrap();
        // 95 mm apart: a 75 mm gap between the cubes, over the 72.5 mm radius.
        let ok = [
            cube(40., 135., 20.),
            cube(135., 135., 20.),
            cube(230., 135., 20.),
        ];
        assert!(run(&c, &ok).errors.is_empty());
        let close = [cube(40., 135., 20.), cube(100., 135., 20.)];
        let r = run(&c, &close);
        assert_eq!(
            r.errors,
            ["B is too close to others, and collisions may be caused."]
        );
        // Taller than the rod with a later object in the same Y band.
        let tall = [cube(40., 135., 30.), cube(135., 135., 20.)];
        assert_eq!(
            run(&c, &tall).errors,
            ["A is too tall, and collisions will be caused."]
        );
        // The last object may be taller; one in another Y band only has to clear the lid.
        let last = [cube(40., 135., 20.), cube(135., 135., 30.)];
        assert!(run(&c, &last).errors.is_empty());
        let apart = [cube(40., 40., 30.), cube(135., 200., 20.)];
        assert!(run(&c, &apart).errors.is_empty());
        assert_eq!(min_object_distance(&c), 72.5);
    }

    #[test]
    fn outline_grows_the_hull_by_half_the_radius() {
        let c = prusa("PRINTER_MODEL_COREONE");
        let m = cube(100., 100., 20.);
        let parts = [&m];
        let o = clearance_outline(
            &Instance {
                name: "A",
                parts: &parts,
            },
            &c,
        );
        let (min, max) = bbox(&o);
        assert!((min[0] - (90. - 37.5)).abs() < 1e-6 && (max[1] - (110. + 37.5)).abs() < 1e-6);
        assert_eq!(min_object_distance(&c), 75.);
    }
}
