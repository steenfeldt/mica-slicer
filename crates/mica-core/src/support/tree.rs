// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/Support/TreeSupport.cpp :: generate_initial_areas, sample_overhang_area, ensure_maximum_distance_polyline, polyline_sample_next_point_at_distance, create_layer_pathing, merge_influence_areas, create_nodes_from_area, generate_branch_areas, draw_areas
// Changes: Stable arena, parallel branch candidates/layer drawing, deterministic sequential merging, shared contact detection and physical-Z
// Changes: schedule; two movement attempts and radius freeze replace upstream's multi-state search.
// Copyright (c) Prusa Research 2022 - 2023 Vojtěch Bubník @bubnikv, Lukáš Matěna @lukasmatena, Tomáš Mészáros @tamasmeszaros
// PrusaSlicer is released under the terms of the AGPLv3 or higher.
// Tree supports by Thomas Rahm, losely based on Tree Supports by CuraEngine.
// Original source of Thomas Rahm's tree supports:
// https://github.com/ThomasRahm/CuraEngine
//
// Original CuraEngine copyright:
// Copyright (c) 2021 Ultimaker B.V.
// CuraEngine is released under the terms of the AGPLv3 or higher.

use rayon::prelude::*;

use super::material::Contact;
use super::tree_common::{
    Element, Forest, TreeSettings, TreeStatistics, circle, contains, move_inside,
};
use super::tree_common::{offset_ex, simplify};
use super::tree_volumes::{ModelVolumes, safe_offset};
use super::{
    EPS, SupportInput, SupportLayer, SupportLayerKind, SupportParameters, SupportRegion, common,
    material,
};
use crate::fill::{self, FillParams, Pattern};
use crate::geometry::clip::{Join, diff_ex, intersection_ex, union_ex};
use crate::geometry::{BoundingBox, ExPolygon, Point, SCALE};

pub(super) fn generate(
    input: &SupportInput<'_>,
    p: &SupportParameters,
    observer: &dyn crate::progress::Observer,
) -> Result<(Vec<SupportLayer>, TreeStatistics), super::SupportError> {
    use crate::progress::Stage;
    let check = || crate::progress::check(observer).map_err(super::SupportError);
    let timer = crate::timing::Span::new(crate::timing::Stage::SupportContacts);
    let mut contacts = material::top_contact_layers(input, p);
    check()?;
    observer.progress(Stage::Supports, 0.2);
    // Trees share the object layer grid, including the first layer and exact contact gaps.
    let mut synchronized = p.clone();
    synchronized.config.synchronize_layers = true;
    let mut layers = common::generate_support_layers(input, &contacts, &[], &synchronized);
    if layers.is_empty() {
        common::generate_raft_base(input, &mut layers, p);
        return Ok((layers, TreeStatistics::default()));
    }
    drop(timer);
    let timer = crate::timing::Span::new(crate::timing::Stage::SupportVolumes);
    let settings = TreeSettings::new(&p.config);
    let mut volumes = ModelVolumes::new(input, &layers, p, &settings);
    drop(timer);
    let mut forest = Forest {
        layers: vec![Vec::new(); layers.len()],
        ..Default::default()
    };
    let mut roofs = vec![Vec::new(); layers.len()];
    check()?;
    observer.progress(Stage::Supports, 0.3);
    let timer = crate::timing::Span::new(crate::timing::Stage::SupportTips);
    generate_initial_areas(
        input,
        p,
        &settings,
        &mut contacts,
        &mut volumes,
        &layers,
        &mut roofs,
        &mut forest,
    );
    check()?;
    drop(timer);
    let timer = crate::timing::Span::new(crate::timing::Stage::SupportPathing);
    // Pathing walks the layers top-down and is the long part: one tick per layer.
    let ticker = crate::progress::Ticker::span(observer, Stage::Supports, layers.len(), 0.4, 0.8);
    create_layer_pathing(&mut forest, &layers, &mut volumes, &settings, p, &ticker);
    check()?;
    create_nodes_from_area(&mut forest);
    drop(timer);
    // The collision radius may freeze in a narrow passage. The printable tip must still
    // widen to the branch diameter; clipping in draw_areas handles the excess radius.
    for node in &mut forest.elements {
        node.radius = node
            .radius
            .max(settings.radius(node.distance_to_top.min(settings.tip_height)));
    }
    let stats = TreeStatistics {
        tips: forest
            .elements
            .iter()
            .filter(|n| n.parents.is_empty())
            .count(),
        supported_tips: forest
            .elements
            .iter()
            .filter(|n| n.parents.is_empty() && n.valid)
            .count(),
        bed_trunks: forest.layers[0]
            .iter()
            .filter(|&&id| forest.elements[id].valid)
            .count(),
        model_trunks: forest
            .elements
            .iter()
            .filter(|n| n.layer > 0 && n.child.is_none() && n.valid)
            .count(),
    };
    observer.progress(Stage::Supports, 0.8);
    let timer = crate::timing::Span::new(crate::timing::Stage::SupportDraw);
    // Classic (non-organic) tree style only; organic trees use tree3d.
    let mut areas = draw_areas(&forest, &volumes);
    check()?;
    drop(timer);
    let _timer = crate::timing::Span::new(crate::timing::Stage::SupportFinalize);
    observer.progress(Stage::Supports, 0.9);
    let mut bottoms = Vec::new();
    for element in &forest.elements {
        if element.valid && element.child.is_none() && element.layer > 0 {
            let polygons = circle(element.position, element.radius);
            bottoms.push(Contact {
                print_z: layers[element.layer].print_z,
                projection: polygons.clone(),
                enforced: Vec::new(),
                polygons,
            });
        }
    }
    bottoms.sort_by(|a, b| a.print_z.total_cmp(&b.print_z));
    // Branch areas without roofs: upstream's intermediate layers for interface projection.
    let intermediate: Vec<bool> = areas.iter().map(|a| !a.is_empty()).collect();
    layers
        .par_iter_mut()
        .zip(areas.par_iter_mut())
        .enumerate()
        .for_each(|(i, (layer, area))| {
            area.extend_from_slice(&roofs[i]);
            let all = union_ex(area);
            let tip_mask: Vec<_> = forest.layers[i]
                .iter()
                .flat_map(|&id| {
                    let n = &forest.elements[id];
                    if n.valid && n.distance_to_top <= settings.tip_height {
                        circle(n.position, n.radius)
                    } else {
                        Vec::new()
                    }
                })
                .chain(roofs[i].iter().cloned())
                .collect();
            let polygons = union_ex(
                &[
                    diff_ex(&all, &volumes.collision[i]),
                    diff_ex(
                        &intersection_ex(&all, &tip_mask),
                        &volumes.minimum_collision[i],
                    ),
                ]
                .concat(),
            );
            layer.regions.push(SupportRegion {
                kind: SupportLayerKind::Base,
                polygons,
            });
        });
    common::generate_interface_layers(&mut layers, &contacts, &bottoms, p, Some(&intermediate));
    common::generate_raft_base(input, &mut layers, p);
    Ok((layers, stats))
}

#[allow(clippy::too_many_arguments)]
fn generate_initial_areas(
    input: &SupportInput<'_>,
    p: &SupportParameters,
    t: &TreeSettings,
    contacts: &mut [Contact],
    volumes: &mut ModelVolumes,
    layers: &[SupportLayer],
    roofs: &mut [Vec<ExPolygon>],
    forest: &mut Forest,
) {
    volumes.prepare(&[(t.tip_radius, true)], layers.len() - 1);
    for contact in contacts {
        let at = layers
            .partition_point(|l| l.print_z < contact.print_z - EPS)
            .min(layers.len() - 1);
        let forbidden = if p.config.buildplate_only {
            volumes.avoidance(t.tip_radius, at, false)
        } else {
            volumes.collision(t.tip_radius, at)
        };
        let regular = diff_ex(&contact.polygons, forbidden);
        // A sloping overhang may be narrower than the tip radius. Move its support centre
        // outwards so that part of a tip can carry it, as generate_initial_areas does upstream.
        let remaining = diff_ex(
            &contact.polygons,
            &offset_ex(&regular, 0.5 * p.config.width * SCALE, Join::Round),
        );
        let extra = (t.tip_radius - 0.5 * p.config.width).max(0.0) + 0.5 * p.config.width;
        let expanded = diff_ex(
            &offset_ex(&remaining, extra * SCALE, Join::Round),
            forbidden,
        );
        contact.polygons = union_ex(&[regular, expanded].concat());
        let mut area = contact.polygons.clone();
        let mut count = 0;
        let mut tip_layer = at;
        for depth in 0..p.config.top_interface_layers.min(at) {
            let i = at - depth;
            let next = diff_ex(&area, volumes.collision(t.tip_radius, i - 1));
            if next.iter().map(ExPolygon::area_mm2).sum::<f64>() < 1.0 {
                break;
            }
            // Area alone does not guarantee a printable interface on a thin curved overhang.
            // Use the shared backend's actual path generation, as upstream checks roof infill.
            let mut roof_layer = layers[i].clone();
            roof_layer.regions = vec![SupportRegion {
                kind: SupportLayerKind::TopContact,
                polygons: area.clone(),
            }];
            if common::generate_support_toolpaths(&roof_layer, i, p, input.origin)
                .paths
                .is_empty()
            {
                break;
            }
            roofs[i].extend(area.clone());
            tip_layer = i - 1;
            area = next;
            count += 1;
        }
        // Small/steep overhangs are carried by tips, not unprintable slivers of dense roof.
        if count == 0 && at > 0 {
            contact.polygons.clear();
        }
        if area.is_empty() {
            continue;
        }
        let flow_spacing = p.flow.spacing();
        let line_distance = if count > 0 {
            flow_spacing + p.config.interface_spacing
        } else {
            flow_spacing + p.config.tree.branch_distance
        };
        let params = FillParams {
            spacing: flow_spacing,
            density: flow_spacing / line_distance,
            angle: if count > 0 { -45.0 } else { p.config.angle },
            origin: input.origin,
            layer_id: 0,
            z: layers[tip_layer].print_z,
            anchor_length: 1000.0,
            anchor_length_max: 1000.0,
            dont_adjust: true,
            link_max_length: 0.0,
        };
        let mut points = Vec::new();
        for ex in &area {
            let mut part = Vec::new();
            for filled in fill::fill(Pattern::SupportBase, std::slice::from_ref(ex), &params) {
                for line in filled.polylines {
                    part.extend(sample_line(&line, t.connect_length * SCALE, 1));
                }
            }
            let minimum = ((crate::geometry::polygon_length(&ex.contour)
                / (t.connect_length * SCALE)) as usize)
                .clamp(1, 3);
            if part.len() <= minimum {
                let inset = offset_ex(
                    std::slice::from_ref(ex),
                    -p.config.width / 2.2 * SCALE,
                    Join::Round,
                );
                let uncovered = offset_ex(
                    &diff_ex(std::slice::from_ref(ex), &inset),
                    p.config.width.max(t.connect_length) * SCALE,
                    Join::Round,
                );
                let boundary = if !inset.is_empty()
                    && uncovered.iter().map(ExPolygon::area_mm2).sum::<f64>() < 0.000001
                {
                    &inset
                } else {
                    std::slice::from_ref(ex)
                };
                part.clear();
                for poly in boundary.iter().flat_map(ExPolygon::polygons) {
                    let mut line = poly.clone();
                    line.push(line[0]);
                    part.extend(sample_line(&line, t.connect_length * SCALE, minimum));
                }
            }
            if part.is_empty()
                && let Some(bb) = BoundingBox::from_points(&ex.contour)
            {
                part.push(move_inside(
                    std::slice::from_ref(ex),
                    Point::new((bb.min.x + bb.max.x) / 2, (bb.min.y + bb.max.y) / 2),
                ));
            }
            points.extend(part);
        }
        for point in points {
            if forest.layers[tip_layer]
                .iter()
                .any(|&id| forest.elements[id].position.distance_to(point) < t.tip_radius * SCALE)
            {
                continue;
            }
            let to_buildplate = !contains(volumes.avoidance(t.tip_radius, tip_layer, false), point);
            if !to_buildplate && p.config.buildplate_only {
                continue;
            }
            let influence = diff_ex(
                &circle(point, 0.01),
                volumes.collision(t.tip_radius, tip_layer),
            );
            if influence.is_empty() {
                continue;
            }
            forest.insert(Element {
                layer: tip_layer,
                parents: Vec::new(),
                child: None,
                area: influence,
                position: point,
                radius: t.tip_radius,
                distance_to_top: 0.0,
                to_buildplate,
                valid: false,
            });
        }
    }
}

fn create_layer_pathing(
    forest: &mut Forest,
    layers: &[SupportLayer],
    volumes: &mut ModelVolumes,
    t: &TreeSettings,
    p: &SupportParameters,
    ticker: &crate::progress::Ticker<'_>,
) {
    for layer in (1..layers.len()).rev() {
        if ticker.cancelled() {
            // The caller checks the observer and discards the forest.
            return;
        }
        ticker.tick();
        let planned: Vec<_> = forest.layers[layer]
            .iter()
            .map(|&id| {
                let parent = &forest.elements[id];
                let dz = layers[layer].print_z - layers[layer - 1].print_z;
                let distance = parent.distance_to_top + dz;
                let mut radius = t.next_radius(parent.radius, dz);
                // Grow towards the bed flare only as quickly as a printable wall can expand.
                if parent.to_buildplate {
                    radius = radius.max(
                        t.recommended_radius(layers[layer - 1].print_z)
                            .min(parent.radius + dz * t.foot_growth),
                    );
                }
                (id, dz, distance, radius)
            })
            .collect();
        let requests: Vec<_> = planned
            .iter()
            .flat_map(|&(id, _, _, radius)| {
                let parent = &forest.elements[id];
                // Merges can inherit a bed route, so both speeds are needed for every radius.
                [(radius, true), (parent.radius, true)]
            })
            .collect();
        volumes.prepare(&requests, layer - 1);
        let candidates: Vec<_> = planned
            .par_iter()
            .map(|&(id, dz, distance, radius)| {
                let parent = &forest.elements[id];
                let mut area = Vec::new();
                let mut used_radius = radius;
                // Upstream prefers freezing radius at slow speed over a sudden faster movement.
                for (r, slow) in [
                    (radius, true),
                    (parent.radius, true),
                    (radius, false),
                    (parent.radius, false),
                ] {
                    let collision = volumes.collision(r, layer - 1);
                    let grown = safe_offset(
                        &parent.area,
                        dz * if slow { t.slow } else { t.fast }
                            + (r - parent.radius).max(0.0)
                            + 5.0 / SCALE,
                        collision,
                        2.0 * (p.config.xy_gap + parent.radius),
                    );
                    area = if parent.to_buildplate {
                        diff_ex(&grown, volumes.avoidance(r, layer - 1, slow))
                    } else {
                        grown
                    };
                    if !area.is_empty() {
                        used_radius = r;
                        break;
                    }
                }
                let mut to_buildplate = parent.to_buildplate;
                if area.is_empty() && !p.config.buildplate_only {
                    // Coarse cache radii are deliberately conservative. Before giving up, try the
                    // exact collision radius and allow the branch to seek an object landing.
                    let collision = offset_ex(
                        &volumes.collision[layer - 1],
                        parent.radius * SCALE,
                        Join::Round,
                    );
                    area = safe_offset(
                        &parent.area,
                        dz * t.fast,
                        &collision,
                        2.0 * (p.config.xy_gap + parent.radius),
                    );
                    if !area.is_empty() {
                        used_radius = parent.radius;
                        to_buildplate = false;
                    }
                }
                if area.is_empty() {
                    // Only terminate on a real, sufficiently wide upper object surface.
                    if !p.config.buildplate_only {
                        let placeable = offset_ex(
                            &volumes.placeable[layer],
                            -parent.radius * SCALE,
                            Join::Round,
                        );
                        let mut landing = intersection_ex(&parent.area, &placeable);
                        if landing.is_empty() {
                            // Non-gracious landing: accept a partial footprint only when it actually
                            // rests on the model. Never mark a failed branch as grounded in mid-air.
                            let candidate = intersection_ex(
                                &parent.area,
                                &offset_ex(
                                    &volumes.placeable[layer],
                                    0.5 * parent.radius * SCALE,
                                    Join::Round,
                                ),
                            );
                            if !candidate.is_empty() {
                                let point = move_inside(&candidate, parent.position);
                                let footprint = circle(point, parent.radius);
                                let supported: f64 =
                                    intersection_ex(&footprint, &volumes.placeable[layer])
                                        .iter()
                                        .map(ExPolygon::area_mm2)
                                        .sum();
                                if supported >= 0.2 * std::f64::consts::PI * parent.radius.powi(2) {
                                    landing = candidate;
                                }
                            }
                        }
                        if !landing.is_empty() {
                            return (None, Some((id, landing)));
                        }
                    }
                    return (None, None);
                }
                area = simplify(area);
                (
                    Some(Element {
                        layer: layer - 1,
                        parents: vec![id],
                        child: None,
                        area,
                        position: parent.position,
                        radius: used_radius,
                        distance_to_top: distance,
                        to_buildplate,
                        valid: false,
                    }),
                    None,
                )
            })
            .collect();
        let mut next: Vec<Element> = Vec::new();
        for (candidate, landing) in candidates {
            if let Some((id, area)) = landing {
                forest.elements[id].area = area;
                forest.elements[id].valid = true;
            }
            if let Some(node) = candidate {
                next.push(node);
            }
        }
        // Existing tips on the destination layer participate in the same merge pass through
        // their influence regions on the next descent. Never remove/reindex an arena element.
        for i in 0..next.len() {
            if next[i].parents.is_empty() {
                continue;
            }
            for j in i + 1..next.len() {
                if next[j].parents.is_empty() {
                    continue;
                }
                if next[i].to_buildplate != next[j].to_buildplate {
                    let (bed, model) = if next[i].to_buildplate {
                        (&next[i], &next[j])
                    } else {
                        (&next[j], &next[i])
                    };
                    if bed.radius - model.radius > 0.5 {
                        continue;
                    }
                }
                let (small, big) = if next[i].radius <= next[j].radius {
                    (&next[i], &next[j])
                } else {
                    (&next[j], &next[i])
                };
                let delta = big.radius - small.radius;
                let mut a =
                    BoundingBox::from_points(small.area.iter().flat_map(|e| &e.contour)).unwrap();
                let b = BoundingBox::from_points(big.area.iter().flat_map(|e| &e.contour)).unwrap();
                let expansion = (delta * SCALE).ceil() as i64;
                a.min.x -= expansion;
                a.min.y -= expansion;
                a.max.x += expansion;
                a.max.y += expansion;
                if a.max.x < b.min.x || b.max.x < a.min.x || a.max.y < b.min.y || b.max.y < a.min.y
                {
                    continue;
                }
                // A thicker branch already contains a thinner branch before their centre
                // influence areas overlap. Enlarge the smaller area by the radius difference.
                let enlarged = safe_offset(
                    &small.area,
                    delta,
                    volumes.collision(small.radius, layer - 1),
                    2.0 * (p.config.xy_gap + small.radius),
                );
                let shared = intersection_ex(&enlarged, &big.area);
                if offset_ex(&shared, -0.025 * SCALE, Join::Round).is_empty() {
                    continue;
                }
                // Preserve the larger branch's collision clearance after merging.
                let radius = next[i].radius.max(next[j].radius);
                let mut shared = diff_ex(&shared, volumes.collision(radius, layer - 1));
                let to_buildplate = next[i].to_buildplate || next[j].to_buildplate;
                if to_buildplate {
                    // Without a separate to-model avoidance cache, retain an existing bed route.
                    // Otherwise a model-seeking branch could consume a reachable bed branch and
                    // strand all of its tips later in the descent.
                    shared = diff_ex(&shared, volumes.avoidance(radius, layer - 1, false));
                }
                if shared.is_empty() {
                    continue;
                }
                next[i].position = move_inside(&shared, next[i].position);
                next[i].area = shared;
                next[i].radius = radius;
                next[i].to_buildplate = to_buildplate;
                next[i].distance_to_top = next[i].distance_to_top.max(next[j].distance_to_top);
                let parents = std::mem::take(&mut next[j].parents);
                next[i].parents.extend(parents);
            }
        }
        for node in next.into_iter().filter(|n| !n.parents.is_empty()) {
            forest.insert(node);
        }
    }
}

fn create_nodes_from_area(forest: &mut Forest) {
    for (layer, ids) in forest.layers.iter().enumerate() {
        for &id in ids {
            let node = &forest.elements[id];
            let valid =
                layer == 0 || node.valid || node.child.is_some_and(|c| forest.elements[c].valid);
            if !valid {
                continue;
            }
            let suggested = node
                .child
                .map_or(node.position, |c| forest.elements[c].position);
            let position = move_inside(&node.area, suggested);
            forest.elements[id].position = position;
            forest.elements[id].valid = true;
        }
    }
}

fn draw_areas(forest: &Forest, volumes: &ModelVolumes) -> Vec<Vec<ExPolygon>> {
    forest
        .layers
        .par_iter()
        .map(|ids| {
            let mut area = Vec::new();
            for node in ids
                .iter()
                .map(|&id| &forest.elements[id])
                .filter(|e| e.valid)
            {
                let mut polygons = circle(node.position, node.radius);
                for other in node.parents.iter().copied().chain(node.child) {
                    let neighbor = &forest.elements[other];
                    if !neighbor.valid {
                        continue;
                    }
                    let dx = (neighbor.position.x - node.position.x) as f64 / SCALE;
                    let dy = (neighbor.position.y - node.position.y) as f64 / SCALE;
                    let r = node.radius.max(neighbor.radius);
                    let norm = 0.5 / (0.01 + (dx * dx + dy * dy).sqrt() / r);
                    let center = Point::new(
                        (node.position.x + neighbor.position.x) / 2,
                        (node.position.y + neighbor.position.y) / 2,
                    );
                    let mut ellipse = circle(Point::default(), r);
                    for point in &mut ellipse[0].contour {
                        let x = point.x as f64;
                        let y = point.y as f64;
                        point.x = center.x
                            + (x * (1.0 + dx * dx / (r * r) * norm) + y * dx * dy / (r * r) * norm)
                                .round() as i64;
                        point.y = center.y
                            + (y * (1.0 + dy * dy / (r * r) * norm) + x * dx * dy / (r * r) * norm)
                                .round() as i64;
                    }
                    polygons.extend(ellipse);
                }
                let collision = &volumes.minimum_collision[node.layer];
                area.extend(diff_ex(&union_ex(&polygons), collision));
            }
            union_ex(&area)
        })
        .collect()
}

/// Sample Euclidean distance along a connected zig-zag, rejecting points too close to earlier
/// samples on the same polyline. Arc-length sampling of each segment overpopulates roof tips.
fn sample_line(line: &[Point], distance: f64, minimum: usize) -> Vec<Point> {
    if line.is_empty() {
        return Vec::new();
    }
    let length = crate::geometry::polyline_length(line);
    if length < 2.0 * distance && minimum <= 1 {
        let mut remaining = 0.5 * length;
        for edge in line.windows(2) {
            let len = edge[0].distance_to(edge[1]);
            if remaining <= len && len > 0.0 {
                let f = remaining / len;
                return vec![Point::new(
                    edge[0].x + ((edge[1].x - edge[0].x) as f64 * f).round() as i64,
                    edge[0].y + ((edge[1].y - edge[0].y) as f64 * f).round() as i64,
                )];
            }
            remaining -= len;
        }
        return vec![line[0]];
    }
    let mut line = line.to_vec();
    let mut end = line.len() - 1;
    if line[0] == line[end] && end > 0 {
        // A closed, thin overhang needs its two farthest vertices supported first.
        let mut start = 0;
        let mut farthest = 0.0;
        for i in 0..line.len() - 1 {
            for j in 0..i {
                let d = line[i].distance_to(line[j]);
                if d > farthest {
                    start = i;
                    end = j;
                    farthest = d;
                }
            }
        }
        let n = line.len() - 1;
        line[..n].rotate_left(start);
        line[n] = line[0];
        end = (n + end - start) % n;
    }
    let mut distance = distance.max(0.1 * SCALE);
    loop {
        let out = sample_line_at_distance(&line, distance, minimum, end);
        if out.len() >= minimum || distance < 0.1 * SCALE {
            return out;
        }
        distance *= 0.9;
    }
}

fn sample_line_at_distance(
    line: &[Point],
    distance: f64,
    minimum: usize,
    end: usize,
) -> Vec<Point> {
    let mut out = vec![line[0]];
    if minimum > 1 || line[0].distance_to(line[end]) > distance {
        out.push(line[end]);
    }
    let mut current = line[0];
    let mut segment = 0;
    let mut step = distance;
    loop {
        let mut next = None;
        for i in segment..line.len() - 1 {
            let a = line[i];
            let b = line[i + 1];
            if b.distance_to(current) < step {
                continue;
            }
            let dx = (b.x - a.x) as f64;
            let dy = (b.y - a.y) as f64;
            let px = (a.x - current.x) as f64;
            let py = (a.y - current.y) as f64;
            let aa = dx * dx + dy * dy;
            if aa < 1.0 {
                continue;
            }
            let bb = px * dx + py * dy;
            let discriminant = bb * bb - aa * (px * px + py * py - step * step);
            if discriminant < 0.0 {
                continue;
            }
            let f = (-bb + discriminant.sqrt()) / aa;
            if (0.0..=1.0).contains(&f) {
                next = Some((
                    i,
                    Point::new(a.x + (f * dx).round() as i64, a.y + (f * dy).round() as i64),
                ));
                break;
            }
        }
        let Some((i, point)) = next else {
            break;
        };
        if point == current {
            step += distance;
            continue;
        }
        let min_dist = out
            .iter()
            .map(|&p| p.distance_to(point))
            .fold(f64::INFINITY, f64::min);
        if min_dist >= distance - 1.0 {
            out.push(point);
            step = distance;
        } else {
            step = (distance - min_dist).max(0.1 * SCALE);
        }
        current = point;
        segment = i;
    }
    out
}
