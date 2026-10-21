// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/Support/SupportMaterial.cpp :: detect_overhangs, detect_bottom_contacts, SupportGridPattern, PrintObjectSupportMaterial::buildplate_covered, PrintObjectSupportMaterial::top_contact_layers, PrintObjectSupportMaterial::bottom_contact_layers_and_layer_support_areas, PrintObjectSupportMaterial::generate_base_layers, PrintObjectSupportMaterial::trim_support_layers_by_object
// Changes: Single-region normal supports; scanline cell clipping instead of AGG, geometric column
// Changes: projection on a common support schedule; bridge/enforcer/blocker masks supplied by caller.
// Changes: Bottom contacts intersect exposed top surfaces, excluding vertical walls.
// Source: prusaslicer@b028299c77 src/libslic3r/Support/TreeSupport.cpp :: generate_overhangs
// Changes: Tree styles widen enforced overhangs by the tip diameter (upstream enforcer_overhang_offset).
// Changes: Two phases so Orca's overhang clusters (orca_overhangs.rs) can edit automatic overhangs.
// Copyright (c) Prusa Research 2016 - 2023 Vojtěch Bubník @bubnikv, Pavel Mikuš @Godrak, Lukáš Hejl @hejllukas, Roman Beránek @zavorka, Lukáš Matěna @lukasmatena, Vojtěch Král @vojtechkral
// Copyright (c) SuperSlicer 2023 Remi Durand @supermerill
// Copyright (c) 2016 Sakari Kapanen @Flannelhead
//
// ported from lib/Slic3r/Print/SupportMaterial.pm:
// Copyright (c) Prusa Research 2016 - 2017 Vojtěch Bubník @bubnikv
// Copyright (c) 2016 Joseph Lenox @lordofhyphens
// Copyright (c) Slic3r 2013 - 2015 Alessandro Ranellucci @alranel
// Copyright (c) 2013 Mark Hindess
//
// PrusaSlicer is released under the terms of the AGPLv3 or higher
//

use super::{
    EPS, SupportInput, SupportLayer, SupportLayerKind, SupportParameters, SupportRegion,
    SupportStyle,
};
use crate::geometry::clip::{Join, diff_ex, intersection_ex, offset_ex, offset2_ex, union_ex};
use crate::geometry::{BoundingBox, ExPolygon, Point, SCALE};
use rayon::prelude::*;

#[derive(Clone, Debug)]
pub(super) struct Contact {
    pub print_z: f64,
    pub polygons: Vec<ExPolygon>,
    pub projection: Vec<ExPolygon>,
    /// Explicit contacts may rest on the model even in build-plate-only mode.
    pub enforced: Vec<ExPolygon>,
}

fn snug(area: &[ExPolygon], p: &SupportParameters) -> Vec<ExPolygon> {
    // Upstream uses square joins. Simplify at 10 µm in place of MutablePolygon outward smoothing
    // so repeated projection does not accumulate sub-micron polygon vertices.
    let r = p.config.closing_radius * SCALE;
    let mut closed = offset2_ex(area, r, -r, Join::Square);
    for ex in &mut closed {
        for polygon in std::iter::once(&mut ex.contour).chain(ex.holes.iter_mut()) {
            let path: Vec<_> = polygon
                .iter()
                .map(|p| clipper2_rust::Point64::new(p.x, p.y))
                .collect();
            *polygon = clipper2_rust::simplify_path(&path, 0.01 * SCALE, true)
                .into_iter()
                .map(|p| Point::new(p.x, p.y))
                .collect();
        }
    }
    union_ex(&closed)
}

/// Prefix union of all lower object slices, with the safety expansion applied once per slice.
fn buildplate_covered(input: &SupportInput<'_>) -> Vec<Vec<ExPolygon>> {
    let mut covered = Vec::with_capacity(input.layers.len());
    let mut acc = Vec::new();
    for layer in input.layers {
        covered.push(acc.clone());
        acc.extend(offset_ex(layer.outlines, 0.01 * SCALE, Join::Square));
        acc = union_ex(&acc);
    }
    covered
}

/// Slope allowance: how far a layer may extend past the one below without support.
pub(super) fn allowance(c: &super::SupportConfig, i: usize, lower_height: f64) -> f64 {
    if i < c.enforce_layers {
        0.0
    } else if c.threshold > 0.0 {
        lower_height / ((c.threshold + 1.0).min(90.0).to_radians().tan())
    } else {
        0.5 * c.external_width
    }
}

pub(super) fn top_contact_layers(input: &SupportInput<'_>, p: &SupportParameters) -> Vec<Contact> {
    let c = &p.config;
    if !(c.enabled && (c.automatic || input.masks.is_some()) || c.enforce_layers > 0) {
        return Vec::new();
    }
    let covered = if c.buildplate_only && !c.style.is_tree() {
        buildplate_covered(input)
    } else {
        Vec::new()
    };
    // Phase 1: automatic overhangs per layer (after bridge and blocker masks).
    let mut detected: Vec<Option<Vec<ExPolygon>>> = input
        .layers
        .par_iter()
        .enumerate()
        .map(|(i, layer)| {
            if i == 0 {
                return None;
            }
            let enforcers = input
                .masks
                .and_then(|(e, _)| e.get(i))
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            let blockers = input
                .masks
                .and_then(|(_, b)| b.get(i))
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            if !(c.enabled && (c.automatic || !enforcers.is_empty()) || i < c.enforce_layers) {
                return None;
            }
            let lower = &input.layers[i - 1];
            let allowance = allowance(c, i, lower.height);
            let mut overhangs = if !c.automatic && i >= c.enforce_layers {
                Vec::new()
            } else if let Some(mask) = layer.overhangs {
                let area = intersection_ex(mask, layer.outlines);
                if !covered.is_empty() {
                    diff_ex(&area, &covered[i])
                } else {
                    area
                }
            } else {
                let raw = diff_ex(
                    layer.outlines,
                    &offset_ex(lower.outlines, allowance * SCALE, Join::Square),
                );
                let raw = if !covered.is_empty() {
                    diff_ex(&raw, &covered[i])
                } else {
                    raw
                };
                // Restore the full unsupported footprint after applying the slope threshold.
                diff_ex(
                    &intersection_ex(
                        &offset_ex(&raw, allowance * SCALE, Join::Square),
                        layer.outlines,
                    ),
                    lower.outlines,
                )
            };
            // An empty overhang stays empty: only a non-empty one needs the coverage.
            if c.dont_support_bridges
                && !overhangs.is_empty()
                && let Some(bridges) = layer.bridges
            {
                overhangs = diff_ex(&overhangs, bridges.get());
            }
            // Block automatic contacts, then add explicitly enforced unsupported surfaces (Prusa order).
            overhangs = diff_ex(&overhangs, &offset_ex(blockers, 0.1 * SCALE, Join::Square));
            Some(overhangs)
        })
        .collect();
    if c.orca_semantics && c.automatic && !c.style.is_tree() {
        super::orca_overhangs::apply(input, p, &mut detected);
    }
    // Phase 2: contacts.
    let candidates: Vec<_> = input
        .layers
        .par_iter()
        .zip(detected.into_par_iter())
        .enumerate()
        .skip(1)
        .map(|(i, (layer, overhangs))| {
            let mut overhangs = overhangs?;
            let enforcers = input
                .masks
                .and_then(|(e, _)| e.get(i))
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            let lower = &input.layers[i - 1];
            let allowance = allowance(c, i, lower.height);
            let mut forced = diff_ex(
                &intersection_ex(layer.outlines, enforcers),
                &offset_ex(lower.outlines, 0.01 * SCALE, Join::Square),
            );
            if c.style.is_tree() && !forced.is_empty() {
                // Trees widen enforced overhangs by the tip diameter so that steep enforced
                // walls still receive tips (TreeSupport generate_overhangs' fudge constant).
                forced = diff_ex(
                    &offset_ex(&forced, c.tree.tip_diameter * SCALE, Join::Miter),
                    lower.outlines,
                );
            }
            overhangs = union_ex(&[overhangs, forced.clone()].concat());
            let trimming = offset_ex(
                lower.outlines,
                allowance.min(c.xy_gap) * SCALE,
                Join::Square,
            );
            let trimming = if !covered.is_empty() {
                union_ex(&[trimming, covered[i].clone()].concat())
            } else {
                trimming
            };
            let enforced = if c.buildplate_only {
                forced.clone()
            } else {
                Vec::new()
            };
            let mut polygons = union_ex(&[diff_ex(&overhangs, &trimming), forced].concat());
            polygons = match c.style {
                SupportStyle::Grid => {
                    let regular = grid(&polygons, &trimming, p, input.origin);
                    let explicit = grid(&enforced, lower.outlines, p, input.origin);
                    union_ex(&[regular, explicit].concat())
                }
                SupportStyle::Tree | SupportStyle::Organic => polygons,
                _ => snug(&polygons, p),
            };
            let projection = if c.style == SupportStyle::Grid {
                offset_ex(
                    &polygons,
                    -0.5 * p.flow.spacing() * SCALE - 10.0,
                    Join::Miter,
                )
            } else {
                polygons.clone()
            };
            let projection = union_ex(&[projection, overhangs].concat());
            let print_z = layer.print_z - layer.height - c.top_gap;
            if print_z < c.first_layer_height - EPS || polygons.is_empty() {
                return None;
            }
            let print_z = if print_z < c.first_layer_height + p.min_height {
                c.first_layer_height
            } else {
                print_z
            };
            Some(Contact {
                print_z,
                polygons,
                projection,
                enforced,
            })
        })
        .collect();
    let mut contacts: Vec<Contact> = Vec::new();
    for Contact {
        print_z,
        polygons,
        projection,
        enforced,
    } in candidates.into_iter().flatten()
    {
        if let Some(last) = contacts
            .last_mut()
            .filter(|last| print_z - last.print_z < p.min_height - EPS)
        {
            last.projection.extend(projection);
            last.projection = union_ex(&last.projection);
            last.polygons.extend(polygons);
            last.polygons = union_ex(&last.polygons);
            last.enforced.extend(enforced);
            last.enforced = union_ex(&last.enforced);
        } else {
            contacts.push(Contact {
                print_z,
                polygons,
                projection,
                enforced,
            });
        }
    }
    contacts
}

/// Object-synchronized downward projection. Morphological closing is applied after every object
/// slice is subtracted, as upstream does; closing just the initial contacts loses narrow columns
/// around concave walls (for example the Benchy bow).
pub(super) fn project_columns(
    input: &SupportInput<'_>,
    tops: &[Contact],
    p: &SupportParameters,
    observer: &dyn crate::progress::Observer,
) -> Result<(Vec<Contact>, Vec<Vec<ExPolygon>>), super::SupportError> {
    let mut active = Vec::new();
    let mut enforced = Vec::new();
    let mut next = tops.len();
    let mut bottoms = Vec::new();
    let mut areas = vec![Vec::new(); input.layers.len()];
    let covered = if p.config.buildplate_only {
        buildplate_covered(input)
    } else {
        Vec::new()
    };
    for (i, layer) in input.layers.iter().enumerate().rev() {
        // Per layer: projecting a large model's columns down can take tens of seconds.
        crate::progress::check(observer).map_err(super::SupportError)?;
        while next > 0 && tops[next - 1].print_z >= layer.print_z - EPS {
            next -= 1;
            active.extend(tops[next].projection.clone());
            enforced.extend(tops[next].enforced.clone());
        }
        if active.is_empty() && enforced.is_empty() {
            continue;
        }
        active = union_ex(&active);
        enforced = union_ex(&enforced);
        if !p.config.buildplate_only || !enforced.is_empty() {
            let top = input.layers.get(i + 1).map_or_else(
                || layer.outlines.to_vec(),
                |upper| diff_ex(layer.outlines, upper.outlines),
            );
            let touching = intersection_ex(
                if p.config.buildplate_only {
                    &enforced
                } else {
                    &active
                },
                &top,
            );
            if !touching.is_empty() {
                let polygons = offset_ex(&touching, p.config.width * SCALE, Join::Square);
                bottoms.push(Contact {
                    print_z: layer.print_z + p.bottom_gap + p.config.layer_height,
                    projection: polygons.clone(),
                    enforced: Vec::new(),
                    polygons,
                });
            }
        }
        let trim = if p.config.buildplate_only {
            covered[i].clone()
        } else {
            layer.outlines.to_vec()
        };
        let project = |area: &[ExPolygon], trim: &[ExPolygon]| {
            let area = diff_ex(area, trim);
            if p.config.style == SupportStyle::Grid {
                let filled = grid(&area, trim, p, input.origin);
                // Retract the grid boundary margin before continuing the projection.
                let next = offset_ex(&filled, -0.5 * p.flow.spacing() * SCALE - 10.0, Join::Miter);
                (next, filled)
            } else {
                let filled = snug(&area, p);
                (filled.clone(), filled)
            }
        };
        let (next_active, mut area) = project(&active, &trim);
        active = next_active;
        if !enforced.is_empty() {
            let (next_enforced, explicit) = project(&enforced, layer.outlines);
            enforced = next_enforced;
            area = union_ex(&[area, explicit].concat());
        }
        areas[i] = area;
    }
    bottoms.reverse();
    Ok((bottoms, areas))
}

/// Trim the whole extrusion slab, including both vertical clearances, against the object.
pub(super) fn generate_base_layers(
    input: &SupportInput<'_>,
    tops: &[Contact],
    areas: &[Vec<ExPolygon>],
    layers: &mut [SupportLayer],
    p: &SupportParameters,
) {
    for layer in layers {
        let i = input
            .layers
            .partition_point(|o| o.print_z <= layer.print_z + EPS)
            .saturating_sub(1);
        let mut active = areas[i].clone();
        for contact in tops {
            if contact.print_z >= layer.print_z - EPS
                && contact.print_z < input.layers[i].print_z - EPS
            {
                active.extend(contact.projection.clone());
            }
        }
        if active.is_empty() {
            continue;
        }
        let bottom = layer.print_z - layer.height;
        let mut solid = Vec::new();
        let start = input
            .layers
            .partition_point(|o| o.print_z <= bottom - p.bottom_gap + EPS);
        for object in &input.layers[start..] {
            if object.print_z - object.height >= layer.print_z + p.config.top_gap - EPS {
                break;
            }
            solid.extend_from_slice(object.outlines);
        }
        let trim = offset_ex(&union_ex(&solid), p.config.xy_gap * SCALE, Join::Square);
        let polygons = diff_ex(&union_ex(&active), &trim);
        if !polygons.is_empty() {
            layer.regions.push(SupportRegion {
                kind: SupportLayerKind::Base,
                polygons,
            });
        }
    }
}

fn rectangle(x0: i64, y0: i64, x1: i64, y1: i64) -> ExPolygon {
    ExPolygon {
        contour: vec![
            Point::new(x0, y0),
            Point::new(x1, y0),
            Point::new(x1, y1),
            Point::new(x0, y1),
        ],
        holes: Vec::new(),
    }
}
fn rotated(area: &[ExPolygon], angle: f64, origin: Point, back: bool) -> Vec<ExPolygon> {
    let (sin, cos) = angle.sin_cos();
    let mut out = area.to_vec();
    for ex in &mut out {
        for poly in std::iter::once(&mut ex.contour).chain(ex.holes.iter_mut()) {
            for pt in poly {
                let (x, y) = if back {
                    (pt.x as f64, pt.y as f64)
                } else {
                    ((pt.x - origin.x) as f64, (pt.y - origin.y) as f64)
                };
                *pt = Point::new(
                    (x * cos - y * sin).round() as i64,
                    (x * sin + y * cos).round() as i64,
                );
                if back {
                    pt.x += origin.x;
                    pt.y += origin.y;
                }
            }
        }
    }
    out
}

/// Scanline rasterization onto the sparse support grid. Every occupied cell is split by the
/// object mask; only pieces connected to an overhang seed survive. This prevents leakage across
/// thin walls without an AGG dependency. Exact polygon clipping replaces subpixel flood fill.
fn grid(
    area: &[ExPolygon],
    trimming: &[ExPolygon],
    p: &SupportParameters,
    origin: Point,
) -> Vec<ExPolygon> {
    let angle = p.config.angle.to_radians();
    let area = rotated(area, -angle, origin, false);
    let trim = rotated(trimming, -angle, origin, false);
    let Some(bb) = BoundingBox::from_points(area.iter().flat_map(|e| e.contour.iter())) else {
        return Vec::new();
    };
    let cell = ((p.config.spacing + p.flow.spacing()) * SCALE)
        .round()
        .max(1.0) as i64;
    let mut cells = Vec::new();
    let mut y = bb.min.y.div_euclid(cell) * cell;
    while y < bb.max.y {
        let strip = rectangle(bb.min.x - 1, y, bb.max.x + 1, y + cell);
        let pieces = intersection_ex(&area, &[strip]);
        let mut spans: Vec<(i64, i64)> = pieces
            .iter()
            .filter_map(|ex| {
                BoundingBox::from_points(ex.contour.iter())
                    .map(|b| (b.min.x.div_euclid(cell), (b.max.x - 1).div_euclid(cell)))
            })
            .collect();
        spans.sort_unstable();
        let mut last = None;
        for (from, to) in spans {
            for x in from..=to {
                if last.is_some_and(|l| x <= l) {
                    continue;
                }
                last = Some(x);
                let block = rectangle(x * cell, y, (x + 1) * cell, y + cell);
                for piece in diff_ex(&[block], &trim) {
                    if !intersection_ex(std::slice::from_ref(&piece), &area).is_empty() {
                        cells.push(piece);
                    }
                }
            }
        }
        y += cell;
    }
    let grid = offset_ex(
        &union_ex(&cells),
        0.5 * p.flow.spacing() * SCALE,
        Join::Miter,
    );
    rotated(&diff_ex(&grid, &trim), angle, origin, true)
}
