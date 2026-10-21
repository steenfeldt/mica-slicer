// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: orca@8500fcdcca src/libslic3r/Fill/Fill.cpp :: split_solid_surface, filter_vibrating_extrusions
// Source: orca@8500fcdcca src/libslic3r/Fill/FillConcentricInternal.cpp :: FillConcentricInternal::fill_surface_extrusion
// Changes: Reuse the shared Prusa vibration graph/reconstruction and Mica Arachne; clipped scanlines replace AABB queries.

use super::{FillParams, Pattern, base::rotate_points};
use crate::{
    arachne::{ArachneSettings, WallToolPaths},
    flow::Flow,
    geometry::clip::{
        Join, diff_ex, intersection_ex, intersection_pl, offset_ex, offset2_ex, union_ex,
        union_ex_positive, union_safety_offset_ex,
    },
    geometry::{BoundingBox, ExPolygon, Point, SCALE, medial_axis::ThickPolyline},
    settings::{FloatOrPercent, PrintSettings},
    toolpath::{ExtrusionPath, Role},
};

pub(crate) fn split(
    area: &[ExPolygon],
    p: &FillParams,
    pattern: Pattern,
) -> (Vec<ExPolygon>, Vec<ExPolygon>) {
    let spacing = (p.spacing * SCALE) as i64;
    if area.is_empty() || spacing <= 0 {
        return (area.to_vec(), vec![]);
    }
    let line_based = matches!(
        pattern,
        Pattern::Rectilinear
            | Pattern::Monotonic
            | Pattern::MonotonicLines
            | Pattern::AlignedRectilinear
    );
    if !line_based {
        let normal = intersection_ex(
            area,
            &offset2_ex(area, -(spacing as f64), spacing as f64, Join::Miter),
        );
        return (normal.clone(), diff_ex(area, &normal));
    }
    let angle = std::f64::consts::PI
        - (p.angle.to_radians() + std::f64::consts::FRAC_PI_2 + pattern.layer_angle(p.layer_id));
    let mut coverage = Vec::new();
    for polygon in area {
        let mut ex = polygon.clone();
        for ring in std::iter::once(&mut ex.contour).chain(&mut ex.holes) {
            super::translate(ring, -p.origin.x, -p.origin.y);
            rotate_points(ring, angle);
        }
        let Some(bb) = BoundingBox::from_points(ex.contour.iter()) else {
            continue;
        };
        let inner = offset_ex(
            &intersection_ex(
                std::slice::from_ref(&ex),
                &offset2_ex(
                    std::slice::from_ref(&ex),
                    -2. * spacing as f64,
                    3. * spacing as f64,
                    Join::Miter,
                ),
            ),
            -0.5 * spacing as f64,
            Join::Miter,
        );
        let count = (bb.max.x - bb.min.x + spacing - 1) / spacing;
        let mut sections = Vec::new();
        for i in 0..count {
            let x = bb.min.x + i * spacing;
            let mut lines = intersection_pl(
                &[vec![Point::new(x, bb.min.y), Point::new(x, bb.max.y)]],
                &inner,
            );
            for line in &mut lines {
                line.sort_by_key(|p| p.y);
            }
            lines.retain(|l| l.last().unwrap().y - l[0].y > spacing);
            lines.sort_by_key(|l| l[0].y);
            sections.push(
                lines
                    .into_iter()
                    .map(|l| [l[0], *l.last().unwrap()])
                    .collect(),
            );
        }
        super::ensuring_filter::filter(&mut sections);
        for mut ring in super::ensuring::reconstruct(&sections, spacing) {
            rotate_points(&mut ring, -angle);
            super::translate(&mut ring, p.origin.x, p.origin.y);
            coverage.push(ring);
        }
    }
    let mut normal = union_safety_offset_ex(&union_ex_positive(&coverage));
    let mut narrow = Vec::new();
    for ex in diff_ex(area, &normal) {
        if offset_ex(
            std::slice::from_ref(&ex),
            -0.5 * spacing as f64,
            Join::Miter,
        )
        .is_empty()
            && !intersection_ex(
                &normal,
                &offset_ex(std::slice::from_ref(&ex), 0.3 * spacing as f64, Join::Miter),
            )
            .is_empty()
        {
            normal.push(ex);
        } else {
            narrow.push(ex);
        }
    }
    if narrow.is_empty() {
        return (area.to_vec(), vec![]);
    }
    (
        intersection_ex(
            &offset_ex(&union_ex(&normal), 0.5 * spacing as f64, Join::Miter),
            area,
        ),
        narrow,
    )
}

pub(crate) fn concentric(
    area: &[ExPolygon],
    p: &FillParams,
    flow: Flow,
    s: &PrintSettings,
) -> Vec<ExtrusionPath> {
    let spacing = (p.spacing * SCALE).trunc();
    let settings = ArachneSettings {
        wall_transition_length: FloatOrPercent::Mm(0.4),
        ..ArachneSettings::default()
    };
    let mut runs = Vec::new();
    for ex in area {
        let Some(bb) = BoundingBox::from_points(ex.contour.iter()) else {
            continue;
        };
        let count = ((bb.max.x - bb.min.x).max(bb.max.y - bb.min.y) as f64 / spacing) as usize + 1;
        let walls = WallToolPaths::generate(
            std::slice::from_ref(ex),
            spacing,
            spacing,
            count,
            flow.height,
            s.nozzle_diameter,
            &settings,
        );
        for mut line in walls.paths {
            if line.junctions.len() < 2 {
                continue;
            }
            if line.is_closed {
                line.junctions.pop();
                let at = (0..line.junctions.len())
                    .min_by_key(|&i| line.junctions[i].p.distance_sq(p.origin))
                    .unwrap();
                line.junctions.rotate_left(at);
                line.junctions.push(line.junctions[0]);
            }
            let mut thick = ThickPolyline {
                points: line.polygon(),
                width: line
                    .junctions
                    .windows(2)
                    .flat_map(|j| [j[0].width, j[1].width])
                    .collect(),
                endpoints: (!line.is_closed, !line.is_closed),
            };
            super::ensuring::clip_end(
                &mut thick,
                s.seam.gap.resolve(s.nozzle_diameter, 1.).max(0.) * SCALE,
            );
            if thick.points.len() > 1 {
                runs.push(crate::perimeters::thick_polyline_to_multi_path(
                    &thick,
                    Role::SolidInfill,
                    flow,
                    0.05 * SCALE,
                    100.,
                ));
            }
        }
    }
    crate::perimeters::chain_runs(runs, p.origin)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn rectangle(width: f64, height: f64) -> ExPolygon {
        ExPolygon {
            contour: vec![
                Point::from_mm(0., 0.),
                Point::from_mm(width, 0.),
                Point::from_mm(width, height),
                Point::from_mm(0., height),
            ],
            holes: vec![],
        }
    }
    #[test]
    fn narrow_solids_use_continuous_variable_width_fill() {
        let s = PrintSettings::core_one_hf04_speed_pla();
        let flow = Flow::new(0.45, 0.2);
        let p = FillParams {
            spacing: flow.spacing(),
            density: 1.,
            angle: 45.,
            layer_id: 0,
            z: 0.2,
            anchor_length: 1000.,
            anchor_length_max: 1000.,
            dont_adjust: false,
            link_max_length: 0.,
            origin: Point::default(),
        };
        let thin = rectangle(0.7, 20.);
        let (normal, narrow) = split(std::slice::from_ref(&thin), &p, Pattern::Monotonic);
        assert!(normal.is_empty());
        assert!(diff_ex(std::slice::from_ref(&thin), &narrow).is_empty());
        let paths = concentric(&narrow, &p, flow, &s);
        assert!(!paths.is_empty());
        assert!(
            paths
                .iter()
                .all(|p| p.role == Role::SolidInfill && p.flow.width > 0.)
        );
        let length: f64 = paths
            .iter()
            .map(|p| crate::geometry::polyline_length(&p.points) / SCALE)
            .sum();
        assert!(
            length > 19. && length < 45.,
            "one or two continuous passes: {length}"
        );
        let wide = rectangle(20., 20.);
        let (normal, narrow) = split(std::slice::from_ref(&wide), &p, Pattern::Monotonic);
        // Diagonal sweeps leave small corner tips for concentric fill, while the wide core
        // retains normal sweeps. Together they must still cover the original region.
        let area = |polygons: &[ExPolygon]| {
            polygons
                .iter()
                .map(|p| crate::geometry::signed_area(&p.contour).abs())
                .sum::<f64>()
                / SCALE
                / SCALE
        };
        assert!(area(&normal) > 360. && area(&narrow) < 40.);
        let mut covered = normal;
        covered.extend(narrow);
        assert!(area(&diff_ex(&[wide], &union_ex(&covered))) < 1e-4);
    }
}
