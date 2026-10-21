// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original
use super::*;
fn rectangle(w: f64, h: f64) -> Vec<ExPolygon> {
    vec![ExPolygon {
        contour: vec![
            Point::from_mm(0., 0.),
            Point::from_mm(w, 0.),
            Point::from_mm(w, h),
            Point::from_mm(0., h),
        ],
        holes: vec![],
    }]
}
#[test]
fn square_has_closed_walls_and_infill() {
    let out = WallToolPaths::generate(
        &rectangle(20., 20.),
        407079.,
        407079.,
        2,
        0.2,
        0.4,
        &ArachneSettings::default(),
    );
    assert_eq!(out.paths.len(), 2, "{:#?}", out.paths);
    assert!(out.paths.iter().all(|p| p.is_closed));
    assert_eq!(out.inner_contour.len(), 1);
}
#[test]
fn thin_strip_is_one_variable_bead() {
    let out = WallToolPaths::generate(
        &rectangle(20., 0.55),
        407079.,
        407079.,
        2,
        0.2,
        0.4,
        &ArachneSettings::default(),
    );
    assert_eq!(out.paths.len(), 1, "{:#?}", out.paths);
    assert!(out.paths[0].is_odd);
    assert!(out.inner_contour.is_empty());
    assert!(out.paths[0].length() > 19. * SCALE);
}

fn taper() -> Vec<ExPolygon> {
    vec![ExPolygon {
        contour: vec![
            Point::from_mm(0., 0.),
            Point::from_mm(40., 0.),
            Point::from_mm(40., 2.4),
            Point::from_mm(0., 0.15),
        ],
        holes: vec![],
    }]
}

#[test]
fn taper_transitions_without_gap_fill_or_width_jumps() {
    let out = WallToolPaths::generate(
        &taper(),
        407079.,
        407079.,
        2,
        0.2,
        0.4,
        &ArachneSettings::default(),
    );
    assert!(out.paths.iter().any(|p| p.is_odd));
    assert!(out.paths.iter().any(|p| p.is_closed));
    assert!(out.paths.iter().any(|p| p.inset == 1));
    let mut widths = std::collections::BTreeSet::new();
    for line in out.paths {
        for j in &line.junctions {
            assert!(j.width.is_finite() && j.width > 0.);
            widths.insert((j.width / 10_000.) as i64);
        }
        for pair in line.junctions.windows(2) {
            // Bead transitions distribute the width change along the path, rather than jumping
            // at a shared coordinate. Allow the minimum-feature widening at the narrow tip.
            assert!(
                (pair[1].width - pair[0].width).abs() <= pair[1].p.distance_to(pair[0].p) + 100.
            );
        }
    }
    assert!(
        widths.len() > 5,
        "taper needs several distinct extrusion widths"
    );
}

#[test]
fn clip_interpolates_widths_and_conserves_deposited_area() {
    let line = ExtrusionLine {
        inset: 1,
        is_odd: true,
        is_closed: false,
        junctions: vec![
            Junction {
                p: Point::from_mm(-5., 5.),
                width: 200_000.,
                perimeter: 1,
            },
            Junction {
                p: Point::from_mm(15., 5.),
                width: 600_000.,
                perimeter: 1,
            },
        ],
    };
    let inside = clip::clip_extrusion(&line, &rectangle(10., 10.), true);
    assert_eq!(inside.len(), 1);
    let mut points = inside[0].junctions.clone();
    points.sort_by_key(|j| j.p.x);
    assert_eq!(points[0].p, Point::from_mm(0., 5.));
    assert_eq!(points[1].p, Point::from_mm(10., 5.));
    assert!((points[0].width - 300_000.).abs() <= 1.);
    assert!((points[1].width - 500_000.).abs() <= 1.);
    let mut all = inside;
    all.extend(clip::clip_extrusion(&line, &rectangle(10., 10.), false));
    let area: f64 = all
        .iter()
        .flat_map(|l| l.junctions.windows(2))
        .map(|w| w[0].p.distance_to(w[1].p) * (w[0].width + w[1].width) / 2.)
        .sum();
    assert!((area / SCALE.powi(2) - 8.).abs() < 0.0001);
    assert!(all.iter().all(|p| p.inset == 1 && p.is_odd));
}

#[test]
fn arena_links_survive_transition_insertion() {
    let strategy = Strategy {
        outer: 407079.,
        inner: 407079.,
        min_feature: 100_000.,
        min_width: 340_000.,
        split: 0.51,
        add: 0.76,
        max_beads: 4,
        distribution: 1,
        transition_length: 400_000.,
        angle: 10_f64.to_radians(),
        outer_inset: 0.,
    };
    let mut skeleton = Skeleton {
        graph: Graph::construct(&to_polygons(&taper()), strategy.angle).unwrap(),
        strategy,
        filter_deviation: 100_000.,
    };
    skeleton.mark();
    skeleton.transitions();
    let graph = skeleton.graph;
    for (id, edge) in graph.edges.iter().enumerate().filter(|(_, e)| e.alive) {
        let twin = &graph.edges[edge.twin];
        assert!(twin.alive && graph.nodes[edge.from].alive && graph.nodes[edge.to].alive);
        assert_eq!(twin.twin, id);
        assert_eq!((twin.from, twin.to), (edge.to, edge.from));
        if edge.next != graph::NONE {
            assert_eq!(graph.edges[edge.next].prev, id);
            assert_eq!(graph.edges[edge.next].from, edge.to);
        }
        if edge.prev != graph::NONE {
            assert_eq!(graph.edges[edge.prev].next, id);
        }
    }
}

fn params() -> WallParams {
    let flow = crate::flow::Flow::new(0.45, 0.2);
    WallParams {
        perimeters: 2,
        external: flow,
        internal: flow,
        solid_infill: flow,
        infill_overlap: FloatOrPercent::Percent(15.),
        nozzle_diameter: 0.4,
        thin_walls: false,
        gap_fill: true,
        filter_out_gap_fill: 0.,
        fill_density: 0.15,
    }
}

#[test]
fn top_and_first_layer_wall_counts_keep_fill_area() {
    use crate::settings::PrintSettings;
    let square = rectangle(20., 20.);
    let p = params();
    let mut s = PrintSettings::core_one_hf04_speed_pla();
    let regular = make_walls(&square[0], &p, &s, None, None, 2);
    assert_eq!(regular.variable_walls.len(), 2);
    s.top_one_perimeter = TopOnePerimeter::TopmostOnly;
    let top = make_walls(&square[0], &p, &s, None, None, 2);
    assert_eq!(top.variable_walls.len(), 1);
    assert!(!top.infill_area.is_empty());
    let covered = make_walls(&square[0], &p, &s, None, Some(&square), 2);
    assert_eq!(covered.variable_walls.len(), 2);
    s.only_one_perimeter_first_layer = true;
    assert_eq!(
        make_walls(&square[0], &p, &s, None, Some(&square), 0)
            .variable_walls
            .len(),
        1
    );
    s.top_one_perimeter = TopOnePerimeter::TopSurfaces;
    let partial = make_walls(
        &square[0],
        &p,
        &s,
        Some(&square),
        Some(&rectangle(10., 20.)),
        2,
    );
    let inner: Vec<_> = partial
        .variable_walls
        .iter()
        .flatten()
        .filter(|p| p.role == Role::Perimeter)
        .collect();
    assert!(!inner.is_empty());
    assert!(
        inner
            .iter()
            .flat_map(|p| &p.points)
            .all(|p| p.x < 10 * SCALE as i64)
    );
    let zero = make_walls(
        &square[0],
        &WallParams { perimeters: 0, ..p },
        &s,
        None,
        None,
        0,
    );
    assert!(zero.variable_walls.is_empty());
    assert!(!zero.infill_area.is_empty());
}

#[test]
fn unsupported_walls_use_configured_bridge_cross_section() {
    let square = rectangle(20., 20.);
    let p = params();
    let mut s = crate::settings::PrintSettings::core_one_hf04_speed_pla();
    let thin = make_walls(&square[0], &p, &s, Some(&[]), None, 2);
    assert!(
        thin.variable_walls
            .iter()
            .flatten()
            .all(|p| p.role == Role::OverhangPerimeter && p.flow.height == 0.2)
    );
    s.thick_bridges = true;
    s.bridge_flow_ratio = 0.81;
    let thick = make_walls(&square[0], &p, &s, Some(&[]), None, 2);
    assert!(
        thick
            .variable_walls
            .iter()
            .flatten()
            .all(|p| (p.flow.height - 0.36).abs() < 1e-9 && p.flow.width == p.flow.height)
    );
    s.supports_touching = true;
    let supported = make_walls(&square[0], &p, &s, Some(&[]), None, 2);
    assert!(
        supported
            .variable_walls
            .iter()
            .flatten()
            .all(|p| p.role != Role::OverhangPerimeter)
    );
}

#[test]
fn first_layer_bead_minimum_does_not_change_later_layers() {
    let p = params();
    let s = crate::settings::PrintSettings::core_one_hf04_speed_pla();
    let island = &taper()[0];
    let normal = make_walls(island, &p, &s, None, None, 0);
    let mut changed = s.clone();
    changed.small_features.initial_layer_min_bead_width = Some(FloatOrPercent::Percent(125.));
    let first = make_walls(island, &p, &changed, None, None, 0);
    let later = make_walls(island, &p, &changed, None, None, 1);
    assert_ne!(first.variable_walls, normal.variable_walls);
    assert_eq!(later.variable_walls, normal.variable_walls);
}
