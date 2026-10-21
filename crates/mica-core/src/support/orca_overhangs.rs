// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: orca@8500fcdcca src/libslic3r/Support/SupportMaterial.cpp :: detect_overhangs, PrintObjectSupportMaterial::top_contact_layers, OverhangCluster, add_overhang
// Changes: Operates on Mica's per-layer automatic overhangs (after bridge/blocker masks) for normal
// Changes: supports only; object outlines stand in for lslices/raw_slices; one region per layer;
// Changes: polygon-intersection overlap tests; point-to-edge distances replace Polygon::distance_to.
// Copyright (c) Prusa Research 2016 - 2023 Vojtěch Bubník @bubnikv and others; Bambu Lab and
// OrcaSlicer contributors (sharp tails, cantilevers and small overhang clusters).
// OrcaSlicer is released under the terms of the AGPLv3 or higher.

//! Orca's normal-support overhang post-processing: sharp tails (small islands starting in the air
//! and the narrow layers growing out of them) gain full support, cantilevers are marked, and
//! clusters of small automatic overhangs are removed when `support_remove_small_overhang` is set.

use super::{SupportInput, SupportParameters};
use crate::geometry::clip::{Join, diff_ex, intersection_ex, offset_ex, union_ex};
use crate::geometry::{BoundingBox, ExPolygon, Point, SCALE};

/// Islands smaller than this (both bbox sides) or this area are sharp-tail candidates.
const LENGTH_WELL_SUPPORTED: f64 = 6.0;
const AREA_WELL_SUPPORTED: f64 = 36.0;
/// Sharp tails stop gaining support above this accumulated height, mm.
const SHARP_TAIL_MAX_HEIGHT: f64 = 16.0;
/// An overhang whose farthest point is this far from its supported boundary is a cantilever, mm.
const CANTILEVER_DISTANCE: f64 = 3.0;

fn bbox(area: &[ExPolygon]) -> Option<BoundingBox> {
    BoundingBox::from_points(area.iter().flat_map(|e| e.contour.iter()))
}

fn size(area: &[ExPolygon]) -> (f64, f64) {
    bbox(area).map_or((0.0, 0.0), |b| {
        (
            (b.max.x - b.min.x) as f64 / SCALE,
            (b.max.y - b.min.y) as f64 / SCALE,
        )
    })
}

fn overlaps(a: &[ExPolygon], b: &[ExPolygon]) -> bool {
    let (Some(x), Some(y)) = (bbox(a), bbox(b)) else {
        return false;
    };
    if x.max.x < y.min.x || y.max.x < x.min.x || x.max.y < y.min.y || y.max.y < x.min.y {
        return false;
    }
    !intersection_ex(a, b).is_empty()
}

fn segment_distance(p: Point, a: Point, b: Point) -> f64 {
    let (px, py) = (p.x as f64, p.y as f64);
    let (ax, ay) = (a.x as f64, a.y as f64);
    let (dx, dy) = (b.x as f64 - ax, b.y as f64 - ay);
    let len2 = dx * dx + dy * dy;
    let t = if len2 > 0.0 {
        (((px - ax) * dx + (py - ay) * dy) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    ((px - ax - t * dx).powi(2) + (py - ay - t * dy).powi(2)).sqrt()
}

fn distance_to_boundary(p: Point, area: &[ExPolygon]) -> f64 {
    area.iter()
        .flat_map(ExPolygon::polygons)
        .flat_map(|poly| {
            (0..poly.len()).map(move |k| segment_distance(p, poly[k], poly[(k + 1) % poly.len()]))
        })
        .fold(f64::INFINITY, f64::min)
}

struct Cluster {
    /// (layer, index into that layer's overhang list)
    members: Vec<(usize, usize)>,
    dilated: Vec<ExPolygon>,
    min_layer: usize,
    max_layer: usize,
    cantilever: bool,
}

/// Apply Orca's sharp-tail, cantilever and small-cluster rules to `overhangs` (indexed by object
/// layer; `None` where no automatic detection ran).
pub(super) fn apply(
    input: &SupportInput<'_>,
    p: &SupportParameters,
    overhangs: &mut [Option<Vec<ExPolygon>>],
) {
    let c = &p.config;
    let fw = c.external_width * SCALE;
    let ew = c.width * SCALE;
    let n = input.layers.len();
    let mut tails: Vec<Vec<(ExPolygon, f64)>> = vec![Vec::new(); n];
    let mut cantilevers: Vec<Vec<ExPolygon>> = vec![Vec::new(); n];
    for i in 1..n {
        let Some(found) = overhangs[i].as_mut() else {
            continue;
        };
        let layer = &input.layers[i];
        let lower = &input.layers[i - 1];
        // detect_overhangs: islands with nothing below that are small enough are sharp tails.
        let lower_kept: Vec<ExPolygon> = lower
            .outlines
            .iter()
            .filter(|e| !offset_ex(std::slice::from_ref(e), -fw / 2.0, Join::Miter).is_empty())
            .cloned()
            .collect();
        for ex in layer.outlines {
            let one = std::slice::from_ref(ex);
            if offset_ex(one, -0.5 * fw, Join::Miter).is_empty() {
                continue;
            }
            if !overlaps(&offset_ex(one, 0.5 * fw, Join::Miter), &lower_kept)
                && ex.area_mm2() < AREA_WELL_SUPPORTED
                && !offset_ex(one, -0.1 * fw, Join::Miter).is_empty()
            {
                tails[i].push((ex.clone(), layer.height));
                found.extend(offset_ex(
                    &diff_ex(one, &lower_kept),
                    0.05 * fw,
                    Join::Miter,
                ));
            }
        }
        *found = union_ex(found);
        // Cantilevers: the farthest overhang point is far from the supported boundary.
        let allowance = super::material::allowance(c, i, lower.height) * SCALE;
        let near = offset_ex(lower.outlines, fw.max(allowance) + 0.1 * SCALE, Join::Miter);
        for poly in found.iter() {
            let boundary = intersection_ex(std::slice::from_ref(poly), &near);
            if boundary.is_empty() {
                continue;
            }
            let far = poly
                .contour
                .iter()
                .map(|&q| distance_to_boundary(q, &boundary))
                .fold(0.0, f64::max);
            if far > CANTILEVER_DISTANCE * SCALE {
                cantilevers[i].push(poly.clone());
            }
        }
    }
    // top_contact_layers: extend sharp tails upwards while they stay narrow and low.
    for i in 1..n {
        if overhangs[i].is_none() || tails[i - 1].is_empty() {
            continue;
        }
        let lower_tails: Vec<ExPolygon> = tails[i - 1].iter().map(|(e, _)| e.clone()).collect();
        let layer = &input.layers[i];
        for ex in layer.outlines {
            if tails[i].iter().any(|(t, _)| t == ex) {
                continue;
            }
            let one = std::slice::from_ref(ex);
            let supported = intersection_ex(one, &lower_tails);
            if supported.is_empty() {
                continue;
            }
            let (sx, sy) = size(&supported);
            if sx > LENGTH_WELL_SUPPORTED && sy > LENGTH_WELL_SUPPORTED {
                continue;
            }
            let mut height = layer.height;
            if let Some((_, h)) = tails[i - 1]
                .iter()
                .find(|(t, _)| overlaps(std::slice::from_ref(t), one))
            {
                height += h;
            }
            if height > SHARP_TAIL_MAX_HEIGHT {
                continue;
            }
            let grown = diff_ex(one, &lower_tails);
            let (gx, gy) = size(&grown);
            let (lx, ly) = size(&lower_tails);
            if (gx - lx > 5.0 && gy - ly > 5.0)
                || !offset_ex(&grown, -5.0 * ew, Join::Miter).is_empty()
            {
                continue;
            }
            tails[i].push((ex.clone(), height));
            let mut overhang = diff_ex(one, input.layers[i - 1].outlines);
            if let Some((_, blockers)) = input.masks
                && let Some(b) = blockers.get(i)
            {
                overhang = diff_ex(&overhang, b);
            }
            if let Some(found) = overhangs[i].as_mut() {
                found.extend(overhang);
            }
        }
    }
    if !c.remove_small_overhang {
        return;
    }
    // Cluster vertically adjacent overhangs and remove the small ones.
    let mut clusters: Vec<Cluster> = Vec::new();
    for (i, found) in overhangs.iter().enumerate().skip(1) {
        let Some(found) = found else {
            continue;
        };
        for (k, poly) in found.iter().enumerate() {
            let dilated = offset_ex(std::slice::from_ref(poly), ew, Join::Miter);
            let at = clusters.iter().position(|cl| {
                i + 1 >= cl.min_layer && i <= cl.max_layer + 1 && overlaps(&dilated, &cl.dilated)
            });
            let cl = match at {
                Some(at) => &mut clusters[at],
                None => {
                    clusters.push(Cluster {
                        members: Vec::new(),
                        dilated: Vec::new(),
                        min_layer: i,
                        max_layer: i,
                        cantilever: false,
                    });
                    clusters.last_mut().unwrap()
                }
            };
            cl.members.push((i, k));
            cl.dilated = union_ex(&[std::mem::take(&mut cl.dilated), dilated].concat());
            cl.min_layer = cl.min_layer.min(i);
            cl.max_layer = cl.max_layer.max(i);
            if overlaps(std::slice::from_ref(poly), &cantilevers[i]) {
                cl.cantilever = true;
            }
        }
    }
    let mut removed: Vec<Vec<usize>> = vec![Vec::new(); n];
    for cl in &clusters {
        let tail = (cl.min_layer..=cl.max_layer).any(|i| {
            let t: Vec<ExPolygon> = tails[i].iter().map(|(e, _)| e.clone()).collect();
            overlaps(&t, &cl.dilated)
        });
        if tail || cl.cantilever {
            continue;
        }
        let (x, y) = size(&offset_ex(&cl.dilated, -ew, Join::Miter));
        if x * SCALE < 2.0 * ew || y * SCALE < 2.0 * ew {
            for &(i, k) in &cl.members {
                removed[i].push(k);
            }
        }
    }
    for (i, gone) in removed.into_iter().enumerate() {
        if gone.is_empty() {
            continue;
        }
        if let Some(found) = overhangs[i].as_mut() {
            let kept: Vec<ExPolygon> = found
                .iter()
                .enumerate()
                .filter(|(k, _)| !gone.contains(k))
                .map(|(_, e)| e.clone())
                .collect();
            *found = kept;
        }
    }
}
