// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/ExtrusionOrder.cpp :: get_support_extrusions
// Source: prusaslicer@b028299c77 src/libslic3r/ShortestPath.cpp :: chain_extrusion_references, chain_extrusion_entities
// Source: prusaslicer@b028299c77 src/libslic3r/GCode.cpp :: GCodeGenerator::process_layer, GCodeGenerator::extrude_support
// Changes: Chain scheduled support entities per object/tool, retaining no-sort organic collections
// Changes: and anchor direction. Reuse mica-core's pinned ShortestPath port. Seed with the preceding
// Changes: extrusion endpoint for organic supports; normal supports use upstream's unseeded outer
// Changes: chain. Tool filtering is done by PreparedPrint::scheduled.

// Upstream attribution (GCode.cpp):
// Copyright (c) Prusa Research 2016 - 2023 Lukáš Matěna @lukasmatena, Vojtěch Bubník @bubnikv, Enrico Turri @enricoturri1966, Pavel Mikuš @Godrak, Oleksandra Iushchenko @YuSanka, Lukáš Hejl @hejllukas, Filip Sykala @Jony01, David Kocík @kocikdav
// Copyright (c) SuperSlicer 2023 Remi Durand @supermerill
// Copyright (c) 2021 Justin Schuh @jschuh
// Copyright (c) 2020 Paul Arden @ardenpm
// Copyright (c) 2020 sckunkle
// Copyright (c) 2020 Kyle Maas @KyleMaas
// Copyright (c) 2019 Thomas Moore
// Copyright (c) 2019 Bryan Smith
// Copyright (c) Slic3r 2015 - 2016 Alessandro Ranellucci @alranel
// Copyright (c) 2016 Chow Loong Jin @hyperair
// Copyright (c) 2015 Maksim Derbasov @ntfshard
// Copyright (c) 2015 Vicious-one @Vicious-one
// Copyright (c) 2015 Luís Andrade
//
// ported from lib/Slic3r/GCode.pm:
// Copyright (c) Slic3r 2011 - 2015 Alessandro Ranellucci @alranel
// Copyright (c) 2013 Robert Giseburt
// Copyright (c) 2012 Mark Hindess
// Copyright (c) 2012 Henrik Brix Andersen @henrikbrixandersen
//
// PrusaSlicer is released under the terms of the AGPLv3 or higher
//

//! Prusa support order, before curve fitting; Orca keeps its own export_batch ordering.

use mica_core::{
    geometry::Point,
    print_job::ScheduledLayer,
    shortest_path::{Entity, chain_entities},
    toolpath::Role,
};

pub(crate) fn chain_support(
    scheduled: &mut ScheduledLayer,
    last: &mut Option<Point>,
    normal: bool,
) {
    let support = |r| matches!(r, Role::SupportMaterial | Role::SupportMaterialInterface);
    let key = |i: usize| scheduled.assignments.get(i).map(|a| (a.object, a.tool));
    let paths = &scheduled.layer.paths;
    if !paths.iter().any(|p| support(p.role)) {
        *last = paths
            .iter()
            .rev()
            .find_map(|p| p.points.last().copied())
            .or(*last);
        return;
    }
    let mut order = Vec::with_capacity(paths.len());
    let mut groups = Vec::new();
    let mut i = 0;
    while i < paths.len() {
        if !support(paths[i].role) {
            order.push((i, false));
            *last = paths[i].points.last().copied().or(*last);
            i += 1;
            continue;
        }
        let start = i;
        let mut ranges = Vec::new();
        let mut entities = Vec::new();
        while i < paths.len() && support(paths[i].role) && key(i) == key(start) {
            let group = scheduled.layer.support_groups.iter().find(|g| g.start == i);
            let end = group.map_or(i + 1, |g| g.end);
            let first = paths[i].points[0];
            let last = *paths[end - 1].points.last().unwrap();
            entities.push(Entity {
                first,
                last,
                is_loop: first == last,
                can_reverse: group.is_none(),
            });
            ranges.push((i..end, group.is_some()));
            i = end;
        }
        // Base and interface are one set when assigned to the same tool. Distinct tools
        // already form separate runs, with their own assignments and speeds.
        // Pinned get_support_extrusions passes no start_near to the outer chain. Keep
        // the previously validated endpoint seed for organic supports only.
        for (index, reverse) in chain_entities(&entities, if normal { None } else { *last }, false)
        {
            let (range, fixed) = &ranges[index];
            if *fixed {
                groups.push(order.len()..order.len() + range.len());
            }
            if reverse {
                order.extend(range.clone().rev().map(|k| (k, true)));
                *last = paths[range.start].points.first().copied().or(*last);
            } else {
                order.extend(range.clone().map(|k| (k, false)));
                *last = paths[range.end - 1].points.last().copied().or(*last);
            }
        }
    }
    let paths = std::mem::take(&mut scheduled.layer.paths);
    let assignments = std::mem::take(&mut scheduled.assignments);
    for (index, reverse) in order {
        let mut path = paths[index].clone();
        if reverse {
            path.points.reverse();
        }
        scheduled.layer.paths.push(path);
        if let Some(assignment) = assignments.get(index) {
            scheduled.assignments.push(*assignment);
        }
    }
    scheduled.layer.support_groups = groups;
}

#[cfg(test)]
mod tests {
    use super::*;
    use mica_core::{
        flow::Flow,
        geometry::SCALE,
        print_job::PathAssignment,
        toolpath::{ExtrusionPath, LayerToolpaths},
    };

    fn line(role: Role, a: f64, b: f64) -> ExtrusionPath {
        ExtrusionPath {
            role,
            points: vec![Point::from_mm(a, 0.), Point::from_mm(b, 0.)],
            flow: Flow::new(0.42, 0.2),
            perimeter_index: None,
            overhang: None,
            seam: None,
            brick: None,
        }
    }

    fn sample() -> ScheduledLayer {
        ScheduledLayer {
            layer: LayerToolpaths {
                paths: vec![
                    line(Role::Perimeter, -2., -1.),
                    line(Role::SupportMaterial, 50., 51.),
                    line(Role::SupportMaterial, 0., 1.),
                    line(Role::SupportMaterialInterface, 3., 2.),
                ],
                ..Default::default()
            },
            assignments: Vec::new(),
        }
    }

    #[test]
    fn nearest_support_from_last_position_and_stored_travel() {
        let mut scheduled = sample();
        let mut last = None;
        chain_support(&mut scheduled, &mut last, false);
        let starts: Vec<_> = scheduled.layer.paths.iter().map(|p| p.points[0]).collect();
        assert_eq!(starts, [-2., 0., 2., 50.].map(|x| Point::from_mm(x, 0.)));
        let travel: f64 = scheduled
            .layer
            .paths
            .windows(2)
            .map(|p| p[0].points.last().unwrap().distance_to(p[1].points[0]) / SCALE)
            .sum();
        assert!((travel - 49.0).abs() < 1e-6, "support travel {travel} mm");
        assert_eq!(last, Some(Point::from_mm(51., 0.)));
        // A new batch/layer starts from the preceding batch's endpoint.
        let mut next = ScheduledLayer {
            layer: LayerToolpaths {
                paths: vec![
                    line(Role::SupportMaterial, 0., 1.),
                    line(Role::SupportMaterial, 49., 50.),
                ],
                ..Default::default()
            },
            assignments: Vec::new(),
        };
        chain_support(&mut next, &mut last, false);
        assert_eq!(next.layer.paths[0].points[0], Point::from_mm(50., 0.));
    }

    #[test]
    fn normal_support_uses_the_unseeded_upstream_tour() {
        let mut a = sample();
        let mut b = sample();
        // Different preceding object endpoints must not seed normal support ordering.
        a.layer.paths[0].points = vec![Point::from_mm(52., 0.), Point::from_mm(51., 0.)];
        let mut last_a = Some(Point::from_mm(51., 0.));
        let mut last_b = Some(Point::from_mm(-1., 0.));
        chain_support(&mut a, &mut last_a, true);
        chain_support(&mut b, &mut last_b, true);
        assert_eq!(a.layer.paths[1..], b.layer.paths[1..]);
        let mut organic = sample();
        chain_support(&mut organic, &mut last_a, false);
        assert_ne!(a.layer.paths[1..], organic.layer.paths[1..]);
    }

    #[test]
    fn organic_no_sort_collection_and_anchors_keep_their_direction() {
        let mut scheduled = sample();
        // The two base paths model an inner/outer pair: it cannot be split or reversed.
        scheduled.layer.support_groups = std::iter::once(1..3).collect();
        let pair = scheduled.layer.paths[1..3].to_vec();
        let mut last = None;
        chain_support(&mut scheduled, &mut last, false);
        let group = &scheduled.layer.support_groups[0];
        assert_eq!(scheduled.layer.paths[group.clone()], pair);
        assert_eq!(group.len(), 2);
    }

    #[test]
    fn separate_interface_tool_and_objects_are_not_interleaved() {
        let mut scheduled = sample();
        scheduled.assignments = [0, 0, 0, 1]
            .map(|tool| PathAssignment {
                object: 0,
                tool,
                region: None,
            })
            .to_vec();
        let mut last = None;
        chain_support(&mut scheduled, &mut last, false);
        assert_eq!(
            scheduled.layer.paths.last().unwrap().role,
            Role::SupportMaterialInterface
        );
        assert_eq!(scheduled.assignments.last().unwrap().tool, 1);
        assert!(scheduled.assignments[..3].iter().all(|a| a.tool == 0));
        let mut scheduled = sample();
        scheduled.assignments = [0, 0, 1, 1]
            .map(|object| PathAssignment {
                object,
                tool: 0,
                region: None,
            })
            .to_vec();
        chain_support(&mut scheduled, &mut last, false);
        assert_eq!(scheduled.layer.paths[1].points[0], Point::from_mm(50., 0.));
        assert_eq!(
            scheduled
                .assignments
                .iter()
                .map(|a| a.object)
                .collect::<Vec<_>>(),
            [0, 0, 1, 1]
        );
    }
}
