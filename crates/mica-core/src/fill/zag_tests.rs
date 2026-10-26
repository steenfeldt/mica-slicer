// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Tests of the zig-zag family against Orca's algorithm: consistent line layout, the Cross Zag
//! shift per pair of layers, the Y-axis mirror and Locked Zag's skin and skeleton.

use super::super::{FillParams, Filled};
use super::*;
use crate::geometry::clip::{diff_ex, union_ex_positive};
use crate::geometry::{BoundingBox, Point, Polygon};

fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Polygon {
    vec![
        Point::from_mm(x0, y0),
        Point::from_mm(x1, y0),
        Point::from_mm(x1, y1),
        Point::from_mm(x0, y1),
    ]
}

/// A 30 mm square centred on the origin with an off-centre hole.
fn holed() -> Vec<ExPolygon> {
    diff_ex(
        &union_ex_positive(&[rect(-15., -15., 15., 15.)]),
        &union_ex_positive(&[rect(-6., -2., 3., 9.)]),
    )
}

fn object() -> BoundingBox {
    BoundingBox {
        min: Point::from_mm(-15., -15.),
        max: Point::from_mm(15., 15.),
    }
}

/// Fill angle 0: the lines run along X (`_infill_direction` adds 90°) on even layers.
fn params(layer_id: usize) -> FillParams {
    FillParams {
        spacing: 0.4,
        density: 0.2,
        angle: 0.0,
        layer_id,
        z: 0.2 * (layer_id + 1) as f64,
        anchor_length: 1000.0,
        anchor_length_max: 1000.0,
        dont_adjust: false,
        link_max_length: 0.0,
        origin: Point::default(),
    }
}

fn fill_zag(
    pattern: Pattern,
    area: &[ExPolygon],
    layer_id: usize,
    zag: &ZagSettings,
) -> Vec<Polyline> {
    super::super::fill_with_bounds(pattern, area, &params(layer_id), Some(object()), zag)
        .into_iter()
        .flat_map(|f| f.polylines)
        .collect()
}

/// Positions (y on even layers, x on odd ones) of the long infill lines, sorted, in µm.
fn line_positions(lines: &[Polyline], layer_id: usize) -> Vec<i64> {
    let mut out: Vec<i64> = lines
        .iter()
        .flat_map(|l| l.windows(2))
        .filter_map(|w| {
            let (a, b) = (w[0], w[1]);
            if layer_id.is_multiple_of(2) {
                ((a.y - b.y).abs() < 10 && (a.x - b.x).abs() > 5_000_000).then_some(a.y / 1000)
            } else {
                ((a.x - b.x).abs() < 10 && (a.y - b.y).abs() > 5_000_000).then_some(a.x / 1000)
            }
        })
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

#[test]
fn zigzag_never_links_back_to_a_previous_line() {
    // Orca's traverse_graph_generate_polylines with a consistent pattern: every polyline walks the
    // vertical lines (here horizontal ones, at increasing y) in one direction only.
    let zag = ZagSettings::default();
    for layer_id in [0, 2, 4] {
        let lines = fill_zag(Pattern::ZigZag, &holed(), layer_id, &zag);
        assert!(!lines.is_empty());
        for line in &lines {
            for w in line.windows(2) {
                assert!(
                    w[1].y >= w[0].y - 10,
                    "zig-zag stepped back from y {} to {}",
                    w[0].y,
                    w[1].y
                );
            }
        }
    }
}

#[test]
fn zigzag_lines_follow_the_object_grid_not_the_area() {
    // `extended_object_bounding_box`: lines of a small area of the object sit on the same
    // positions as the lines of the whole object's area.
    let zag = ZagSettings::default();
    let whole = line_positions(&fill_zag(Pattern::ZigZag, &holed(), 0, &zag), 0);
    let part = union_ex_positive(&[rect(4.3, -11.7, 12.9, 13.1)]);
    let part = line_positions(&fill_zag(Pattern::ZigZag, &part, 0, &zag), 0);
    assert!(part.len() > 3);
    for y in part {
        assert!(
            whole.contains(&y),
            "line at {y} µm is off the object's grid"
        );
    }
}

#[test]
fn zigzag_repeats_every_second_layer() {
    let zag = ZagSettings::default();
    assert_eq!(
        fill_zag(Pattern::ZigZag, &holed(), 2, &zag),
        fill_zag(Pattern::ZigZag, &holed(), 6, &zag)
    );
    assert_eq!(
        fill_zag(Pattern::ZigZag, &holed(), 1, &zag),
        fill_zag(Pattern::ZigZag, &holed(), 3, &zag)
    );
}

#[test]
fn crosszag_shifts_by_the_step_every_two_layers() {
    // Make::fills: horiz_move = ∓ infill_shift_step × (layer_id / 2), even layers negative; the
    // first line moves by horiz_move modulo two line spacings (2 × 0.4 mm / 20 % = 4 mm).
    let zag = ZagSettings {
        infill_shift_step: 0.4,
        orca: true,
        ..ZagSettings::default()
    };
    let at = |layer| line_positions(&fill_zag(Pattern::CrossZag, &holed(), layer, &zag), layer);
    let base_even = at(0);
    let base_odd = at(1);
    assert!(base_even.len() > 5 && base_odd.len() > 5);
    // Layer 2: −0.4 mm, layer 4: −0.8 mm; layers 3 and 5 move +0.4 and +0.8 mm along their
    // rotated frame, which is turned by 180° (fill angle 0 + 90° alternation + 90°): −x in the world.
    for (layer, base, shift) in [
        (2, &base_even, -400),
        (4, &base_even, -800),
        (3, &base_odd, -400),
        (5, &base_odd, -800),
    ] {
        let moved = at(layer);
        let common = moved
            .iter()
            .filter(|p| base.iter().any(|b| (b + shift - **p).abs() <= 1))
            .count();
        assert!(
            common + 2 >= moved.len().min(base.len()),
            "layer {layer}: {moved:?} is not {base:?} moved by {shift} µm"
        );
    }
    // Zig Zag does not move.
    assert_eq!(
        line_positions(&fill_zag(Pattern::ZigZag, &holed(), 4, &zag), 4),
        line_positions(&fill_zag(Pattern::ZigZag, &holed(), 0, &zag), 0)
    );
}

#[test]
fn crosszag_odd_line_shift_keeps_the_zigzag_phase() {
    // gap_line odd: Orca moves by horiz_move minus an even number of line spacings and adds a line.
    let pattern = Pattern::CrossZag;
    let zag = ZagSettings {
        infill_shift_step: 3.0,
        orca: true,
        ..ZagSettings::default()
    };
    // Layer 2: horiz_move −3 mm, line spacing 2 mm: gap_line −1, so the lines move by +1 mm
    // (−3 mm + 2 × 2 mm).
    let base = line_positions(&fill_zag(pattern, &holed(), 0, &zag), 0);
    let moved = line_positions(&fill_zag(pattern, &holed(), 2, &zag), 2);
    let common = moved
        .iter()
        .filter(|p| base.iter().any(|b| (b + 1000 - **p).abs() <= 1))
        .count();
    assert!(
        common + 2 >= moved.len().min(base.len()),
        "{moved:?} vs {base:?}"
    );
    assert_eq!(horiz_move(pattern, 2, &zag), Some(-3_000_000.0));
    assert_eq!(horiz_move(pattern, 3, &zag), Some(3_000_000.0));
    assert_eq!(horiz_move(pattern, 1, &zag), Some(0.0));
}

#[test]
fn symmetric_y_axis_mirrors_the_fill() {
    // symmetric_infill_y_axis: the area is mirrored about the object's centre line, filled, and
    // the lines are mirrored back.
    let plain = ZagSettings::default();
    let symmetric = ZagSettings {
        symmetric_infill_y_axis: true,
        ..plain
    };
    let mirror_ex = |ex: &ExPolygon| {
        let mut ex = ex.clone();
        for poly in std::iter::once(&mut ex.contour).chain(ex.holes.iter_mut()) {
            for p in poly.iter_mut() {
                p.x = -p.x;
            }
            poly.reverse();
        }
        ex
    };
    let area = holed();
    let mirrored: Vec<_> = area.iter().map(mirror_ex).collect();
    for layer in [0, 1] {
        let mut expected = fill_zag(Pattern::ZigZag, &mirrored, layer, &plain);
        for l in &mut expected {
            for p in l.iter_mut() {
                p.x = -p.x;
            }
        }
        assert_eq!(
            fill_zag(Pattern::ZigZag, &area, layer, &symmetric),
            expected
        );
    }
}

#[test]
fn extended_box_is_square_and_rotation_proof() {
    let b = extended_object_bounding_box(BoundingBox {
        min: Point::from_mm(-10., -4.),
        max: Point::from_mm(10., 4.),
    });
    assert_eq!(b.min, Point::new(-14_142_135, -14_142_135));
    assert_eq!(b.max, Point::new(14_142_135, 14_142_135));
}

fn square() -> Vec<ExPolygon> {
    union_ex_positive(&[rect(-15., -15., 15., 15.)])
}

fn locked_zag(layer_id: usize, zag: &ZagSettings) -> Filled {
    let mut filled = super::super::fill_with_bounds(
        Pattern::LockedZag,
        &square(),
        &params(layer_id),
        Some(object()),
        zag,
    );
    assert_eq!(filled.len(), 1);
    filled.remove(0)
}

/// Lines of a fill split into its skeleton and skin parts.
fn split(f: &Filled) -> (&[Polyline], &[Polyline]) {
    f.polylines
        .split_at(f.skin_from.expect("locked zag marks its skin"))
}

/// Distance of a point from the square's outline, mm.
fn depth(p: &Point) -> f64 {
    (15. - (p.x.abs().max(p.y.abs()) as f64 / 1e6)).max(0.)
}

#[test]
fn locked_zag_splits_skin_and_skeleton_at_the_skin_depth() {
    // fill_surface_locked_zag: skin = area − offset(area, −skin depth); skeleton =
    // offset(core, +lock depth) ∩ area.
    let zag = ZagSettings {
        skin_infill_depth: 3.0,
        infill_lock_depth: 1.0,
        orca: true,
        ..ZagSettings::default()
    };
    let filled = locked_zag(0, &zag);
    let (skeleton, skin) = split(&filled);
    assert!(!skeleton.is_empty() && !skin.is_empty());
    // The skeleton reaches 1 mm into the 3 mm skin band, no further (half a line of tolerance).
    let deepest_reach = skeleton
        .iter()
        .flatten()
        .map(depth)
        .fold(f64::MAX, f64::min);
    assert!(
        (1.7..=2.1).contains(&deepest_reach),
        "skeleton reaches {deepest_reach}"
    );
    // The skin stays in its band.
    let innermost = skin.iter().flatten().map(depth).fold(0., f64::max);
    assert!(innermost <= 3.05, "skin reaches {innermost} mm deep");
}

#[test]
fn locked_zag_skeleton_is_turned_and_has_its_own_density() {
    let zag = ZagSettings {
        skin_infill_density: 0.4,
        skeleton_infill_density: 0.1,
        orca: true,
        ..ZagSettings::default()
    };
    let filled = locked_zag(0, &zag);
    let (skeleton, skin) = split(&filled);
    // Layer 0, fill angle 0: skin lines run along X (as Cross Zag), skeleton lines along Y.
    let skel = line_positions_odd(skeleton);
    // Skin lines in the left band: horizontal pieces across the 2 mm skin, at its own spacing.
    let mut skin_y: Vec<i64> = skin
        .iter()
        .flat_map(|l| l.windows(2))
        .filter(|w| {
            let len = (w[1].x - w[0].x).abs();
            (w[0].y - w[1].y).abs() < 10
                && w[0].x < -10_000_000
                && (1_000_000..2_600_000).contains(&len)
        })
        .map(|w| w[0].y / 1000)
        .collect();
    skin_y.sort_unstable();
    skin_y.dedup();
    assert!(skin_y.len() > 10, "{skin_y:?}");
    assert!(skel.len() >= 5, "{skel:?}");
    // Line spacing: 0.4 mm / density.
    let step = |v: &[i64]| v.windows(2).map(|w| w[1] - w[0]).min().unwrap();
    // (Positions are truncated to µm.)
    assert!((step(&skel) - 4000).abs() <= 1, "{skel:?}");
    assert!((step(&skin_y) - 1000).abs() <= 1, "{skin_y:?}");
}

/// Positions of the long lines along Y (x values), µm.
fn line_positions_odd(lines: &[Polyline]) -> Vec<i64> {
    line_positions(lines, 1)
}

#[test]
fn locked_zag_skin_shifts_but_the_skeleton_does_not() {
    let zag = ZagSettings {
        infill_shift_step: 0.4,
        orca: true,
        ..ZagSettings::default()
    };
    let (a, b) = (locked_zag(0, &zag), locked_zag(2, &zag));
    let (skel_a, skin_a) = split(&a);
    let (skel_b, skin_b) = split(&b);
    assert_eq!(line_positions_odd(skel_a), line_positions_odd(skel_b));
    assert_ne!(line_positions(skin_a, 0), line_positions(skin_b, 0));
}

#[test]
fn locked_zag_paths_take_the_skin_and_skeleton_widths() {
    let mut s =
        crate::settings::PrintSettings::from_config(&crate::config::Config::defaults()).unwrap();
    s.zag.skin_infill_line_width = crate::settings::FloatOrPercent::Mm(0.5);
    s.zag.skeleton_infill_line_width = crate::settings::FloatOrPercent::Percent(110.);
    let flow = crate::flow::Flow::new(0.45, 0.2);
    let collections = super::super::collections(
        Pattern::LockedZag,
        &square(),
        &params(0),
        flow,
        crate::toolpath::Role::InternalInfill,
        &s,
        None,
    );
    assert_eq!(collections.len(), 1);
    let widths: std::collections::BTreeSet<_> = collections[0]
        .entities
        .iter()
        .flatten()
        .map(|p| (p.flow.width * 1000.).round() as i64)
        .collect();
    let skeleton = (f64::from((s.nozzle_diameter * 1.1) as f32) * 1000.).round() as i64;
    assert_eq!(widths, [500, skeleton].into_iter().collect());
    assert!(
        collections[0]
            .entities
            .iter()
            .flatten()
            .all(|p| p.flow.height == 0.2)
    );
    // Orca's automatic width for 0: 112.5 % of the nozzle.
    assert_eq!(
        line_width(crate::settings::FloatOrPercent::Mm(0.), 0.4),
        f64::from(1.125f32 * 0.4f32)
    );
}
