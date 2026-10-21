// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: orca@8500fcdcca src/libslic3r/Brim.cpp :: configBrimWidthByVolumeGroups, make_brim_ears_auto, make_brim_ears, outer_inner_brim_area, tryExPolygonOffset
// Source: orca@8500fcdcca src/libslic3r/MaterialType.cpp :: MaterialType::all
// Changes: Single material per plan; volume groups and nonoverlapping brim ownership by object ID; polygon moments in mm; painted ears come in bed coordinates (no EFC-outline snapping).
// Ear detection inherited from SuperSlicer (Remi Durand), as credited in Orca Brim.cpp.

use super::geometry::{closed, loops_to_paths, offset, solid};
use super::*;
use crate::geometry::clip::{Join, diff_ex, intersection_ex, to_polygons, union_ex};
use crate::geometry::simplify::douglas_peucker;
use crate::geometry::{SCALE, polygon_length};

fn simplify_brim(area: &[ExPolygon], resolution: f64) -> Vec<ExPolygon> {
    let ring = |points: &Polygon| {
        let mut points = douglas_peucker(&closed(points.clone()), resolution);
        points.pop();
        points
    };
    area.iter()
        .filter_map(|ex| {
            let contour = ring(&ex.contour);
            (contour.len() >= 3).then(|| ExPolygon {
                contour,
                holes: ex.holes.iter().map(ring).filter(|h| h.len() >= 3).collect(),
            })
        })
        .collect()
}

/// Orca's auto width, before quantization to an even number of flow spacings.
/// Moments are about the footprint centroid, retaining holes and separate islands.
pub fn auto_width(polygons: &[ExPolygon], height: f64, max_speed: f64, material: &str) -> f64 {
    let Some(bb) = BoundingBox::from_points(polygons.iter().flat_map(|p| p.contour.iter())) else {
        return 0.0;
    };
    // Translate first to avoid loss of precision far from the origin.
    let mut area2 = 0.0;
    let mut mx = 0.0;
    let mut my = 0.0;
    let mut ix = 0.0;
    let mut iy = 0.0;
    for poly in polygons.iter().flat_map(ExPolygon::polygons) {
        for (a, b) in poly
            .iter()
            .zip(poly.iter().cycle().skip(1))
            .take(poly.len())
        {
            let (x, y) = (
                (a.x - bb.min.x) as f64 / SCALE,
                (a.y - bb.min.y) as f64 / SCALE,
            );
            let (u, v) = (
                (b.x - bb.min.x) as f64 / SCALE,
                (b.y - bb.min.y) as f64 / SCALE,
            );
            let cross = x * v - u * y;
            area2 += cross;
            mx += (x + u) * cross;
            my += (y + v) * cross;
            ix += (y * y + y * v + v * v) * cross;
            iy += (x * x + x * u + u * u) * cross;
        }
    }
    if area2 <= 0.0 {
        return 0.0;
    }
    let cx = mx / (3.0 * area2);
    let cy = my / (3.0 * area2);
    let ix = ix / 12.0 - area2 * 0.5 * cy * cy;
    let iy = iy / 12.0 - area2 * 0.5 * cx * cx;
    if ix <= 0.0 || iy <= 0.0 {
        return 0.0;
    }
    let dx = (bb.max.x - bb.min.x) as f64 / SCALE;
    let dy = (bb.max.y - bb.min.y) as f64 / SCALE;
    let thermal = dx.hypot(dy);
    let (adhesion, reference) = material_parameters(material);
    let height_to_area = (height / ix * dy).max(height / iy * dx) * height / 1920.0;
    let width = adhesion
        * (height_to_area * max_speed)
            .max(thermal * 8.0 / reference * height.min(30.0) / 30.0)
            .min(18.0)
            .min(1.5 * thermal);
    if width < 5.0 && width < 1.5 * thermal {
        0.0
    } else {
        width.min(18.0)
    }
}

fn material_parameters(material: &str) -> (f64, f64) {
    match material {
        "PET" | "PET-CF" | "PET-GF" | "PETG" => (2.0, 100.0),
        "PCTG" => (2.0, 200.0),
        "PETG-CF" | "PETG-GF" | "ABS" | "ABS-CF" | "ABS-GF" | "ASA" | "ASA-CF" | "ASA-GF"
        | "ASA-AERO" | "PA" | "PA-CF" | "PA-GF" | "PA6" | "PA6-CF" | "PA6-GF" | "PA11"
        | "PA11-CF" | "PA11-GF" | "PA12" | "PA12-CF" | "PA12-GF" => (1.0, 100.0),
        "PC" | "PC-PBT" => (1.0, 40.0),
        "PC-ABS" | "PC-CF" => (1.0, 80.0),
        "TPU" | "FLEX" => (0.5, 1000.0),
        _ => (1.0, 200.0),
    }
}

/// Where automatic ears go on one contour (Orca's `make_brim_ears_auto` point detection, also
/// the brim-ear gizmo's "Auto-generate"): after Douglas–Peucker decimation by
/// `detection_length` mm, the convex corners sharper than `max_angle` (concave ones for a
/// hole, `outer` false).
pub fn ear_points(
    contour: &Polygon,
    detection_length: f64,
    max_angle: f64,
    outer: bool,
) -> Vec<Point> {
    let threshold = (180.0 - max_angle).to_radians();
    let decimated = douglas_peucker(&closed(contour.clone()), detection_length * SCALE);
    let points = if decimated.len() > 4 {
        &decimated[..decimated.len() - 1]
    } else {
        contour
    };
    let mut out = Vec::new();
    for i in 0..points.len() {
        let a = points[(i + points.len() - 1) % points.len()];
        let c = points[(i + 1) % points.len()];
        let v = points[i];
        let (dx, dy) = ((v.x - a.x) as f64, (v.y - a.y) as f64);
        let (ex, ey) = ((c.x - v.x) as f64, (c.y - v.y) as f64);
        let turn = (dx * ey - dy * ex).atan2(dx * ex + dy * ey);
        if (if outer { turn } else { -turn }) > threshold {
            out.push(v);
        }
    }
    out
}

fn ears(area: &[ExPolygon], radius: f64, b: &BrimSettings, outer: bool) -> Vec<ExPolygon> {
    if radius <= 0.0 {
        return Vec::new();
    }
    let mut result = Vec::new();
    for p in area {
        for v in ear_points(&p.contour, b.ears_detection_length, b.ears_max_angle, outer) {
            result.push(ExPolygon {
                contour: (0..24)
                    .map(|i| {
                        let angle = std::f64::consts::TAU * i as f64 / 24.0;
                        Point::new(
                            v.x + (radius * SCALE * angle.cos()) as i64,
                            v.y + (radius * SCALE * angle.sin()) as i64,
                        )
                    })
                    .collect(),
                holes: Vec::new(),
            });
        }
    }
    union_ex(&result)
}

/// Orca's `make_brim_ears`: one 24-sided disc per hand-placed ear on the bed. The disc radius
/// is the ear radius rounded down to an even number of brim spacings (twice for the outer
/// brim, as upstream), less the object gap and one spacing; upstream's `float` steps are kept.
fn painted_ears(
    ears: &[BrimEar],
    input: &AdhesionInput<'_>,
    gap: f64,
    outer: bool,
) -> Vec<ExPolygon> {
    let scaled_spacing = input.flow.scaled_spacing();
    let flow_width = scaled_spacing as f64 / SCALE;
    let brim_offset = (gap * SCALE) as f32;
    ears.iter()
        .filter(|e| e.z <= 0.)
        .map(|e| {
            let mut brim_width =
                ((e.radius * SCALE / flow_width / 2.).floor() * flow_width * 2.) as f32;
            if outer {
                let scaled = flow_width * SCALE;
                brim_width = ((f64::from(brim_width) / scaled / 2.).floor() * scaled * 2.) as f32;
            }
            let size = (brim_width - brim_offset - scaled_spacing as f32) as i64 as f64;
            let (cx, cy) = ((e.x * SCALE) as i32 as i64, (e.y * SCALE) as i32 as i64);
            ExPolygon {
                contour: (0..24)
                    .map(|i| {
                        let angle = 2. * std::f64::consts::PI * i as f64 / 24.;
                        Point::new(
                            (size * angle.cos()).round() as i64 + cx,
                            (size * angle.sin()).round() as i64 + cy,
                        )
                    })
                    .collect(),
                holes: Vec::new(),
            }
        })
        .collect()
}

pub(super) fn brim(input: &AdhesionInput<'_>) -> (Vec<ExtrusionPath>, Vec<ExPolygon>, Vec<u64>) {
    let spacing = input.flow.spacing();
    let quantize = |w: f64| (w / spacing / 2.0).floor() * spacing * 2.0;
    let mut areas = std::collections::BTreeMap::<u64, Vec<ExPolygon>>::new();
    let mut forbidden = input.supports.to_vec();
    for island in input.islands {
        let area = areas.entry(island.object_id).or_default();
        let b = island.brim;
        let p = island.polygon;
        let painted = b.kind == BrimType::Painted;
        let is_outer = matches!(
            b.kind,
            BrimType::Outer
                | BrimType::OuterAndInner
                | BrimType::Auto
                | BrimType::Ears
                | BrimType::Painted
        );
        let is_inner = matches!(
            b.kind,
            BrimType::Inner | BrimType::OuterAndInner | BrimType::Ears | BrimType::Painted
        );
        let mut width = if b.kind == BrimType::Auto {
            let group: Vec<_> = input
                .islands
                .iter()
                .filter(|p| p.object_id == island.object_id)
                .map(|p| p.polygon.clone())
                .collect();
            quantize(auto_width(
                &group,
                island.object_height,
                input.max_speed,
                input.filament_type,
            ))
        } else {
            quantize(b.width)
        };
        if b.kind == BrimType::Auto {
            if width < spacing
                && island.object_height > 10.0
                && p.area_mm2() / (polygon_length(&p.contour) / SCALE) < 1.1
            {
                width += quantize(5.0);
            }
            if let Some(bb) = BoundingBox::from_points(p.contour.iter()) {
                width = quantize(
                    width.min((bb.max.x - bb.min.x).max(bb.max.y - bb.min.y) as f64 / SCALE),
                );
            }
        }
        // Painted ears do not depend on the brim width (upstream emits them at any width).
        if is_outer && (width > 0.0 || painted) {
            let base = offset(&[solid(&p.contour)], b.separation, Join::Round);
            let outside = if painted {
                painted_ears(island.ears, input, b.separation, true)
            } else if b.kind == BrimType::Ears {
                ears(&base, width - b.separation - spacing, b, true)
            } else {
                offset(&base, width, Join::Round)
            };
            area.extend(diff_ex(&outside, &base));
        }
        for hole in &p.holes {
            let hole = [solid(hole)];
            let inside = offset(&hole, -width - b.separation, Join::Miter);
            if is_inner && (width > 0.0 || painted) {
                let base = offset(&hole, -b.separation, Join::Miter);
                let outside = if painted {
                    painted_ears(island.ears, input, b.separation, false)
                } else if b.kind == BrimType::Ears {
                    ears(&base, width - b.separation - spacing, b, false)
                } else {
                    base
                };
                area.extend(intersection_ex(&diff_ex(&outside, &inside), &hole));
            } else {
                forbidden.extend(diff_ex(&hole, &offset(&hole, -spacing, Join::Miter)));
            }
        }
        if !is_outer {
            forbidden.extend(offset(std::slice::from_ref(p), spacing, Join::Miter));
        }
        forbidden.extend(offset(std::slice::from_ref(p), b.separation, Join::Round));
    }
    // Earlier object brims reserve their area; no material is deposited twice at collisions.
    let mut reserved = Vec::new();
    let mut per_object: Vec<(u64, Vec<ExPolygon>)> = Vec::new();
    for (object, area) in areas {
        let area = diff_ex(&diff_ex(&union_ex(&area), &forbidden), &reserved);
        reserved.extend(area.iter().cloned());
        per_object.push((object, area));
    }
    // A brim island must come within two spacings of an object island (grown by its gap),
    // another island of the same brim or another object's brim; a loose one goes (Orca's
    // "brim should be contacted to at least one object's island or brim area").
    let object_islands: Vec<ExPolygon> = input
        .islands
        .iter()
        .flat_map(|i| {
            offset(
                std::slice::from_ref(i.polygon),
                i.brim.separation,
                Join::Round,
            )
        })
        .collect();
    for k in 0..per_object.len() {
        let others: Vec<ExPolygon> = per_object
            .iter()
            .enumerate()
            .filter(|(j, _)| *j != k)
            .flat_map(|(_, (_, a))| a.iter().cloned())
            .collect();
        let area = &per_object[k].1;
        let kept: Vec<ExPolygon> = area
            .iter()
            .enumerate()
            .filter(|(ia, a)| {
                let grown = offset(std::slice::from_ref(*a), 2. * spacing, Join::Round);
                let own: Vec<ExPolygon> = area
                    .iter()
                    .enumerate()
                    .filter(|(io, _)| io != ia)
                    .map(|(_, o)| o.clone())
                    .collect();
                !intersection_ex(&grown, &object_islands).is_empty()
                    || !intersection_ex(&grown, &own).is_empty()
                    || !intersection_ex(&grown, &others).is_empty()
            })
            .map(|(_, a)| a.clone())
            .collect();
        per_object[k].1 = kept;
    }
    let mut all_area = Vec::new();
    let mut paths = Vec::new();
    let mut owners = Vec::new();
    for (object, area) in per_object {
        let mut inset = offset(
            &simplify_brim(&area, 0.0125 * SCALE),
            -0.5 * spacing,
            Join::Round,
        );
        let mut loops = Vec::new();
        while !inset.is_empty() {
            inset = simplify_brim(&inset, 0.0125 * SCALE);
            loops.extend(to_polygons(&inset));
            inset = offset(&inset, -1.3 * spacing, Join::Round);
            inset = offset(
                &simplify_brim(&inset, 0.0125 * SCALE),
                0.3 * spacing,
                Join::Round,
            );
        }
        let object_paths = super::geometry::connect_lines(
            loops_to_paths(loops, input.flow, false),
            &area,
            input.flow,
        );
        owners.extend(std::iter::repeat_n(object, object_paths.len()));
        paths.extend(object_paths);
        all_area.extend(area);
    }
    (paths, all_area, owners)
}
