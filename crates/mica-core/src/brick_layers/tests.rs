// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

use super::*;
use crate::mesh::TriangleMesh;
use crate::print::LayerSource;
use crate::settings::{PerimeterGenerator, PrintSettings};

fn square(x0: f64, y0: f64, x1: f64, y1: f64) -> ExPolygon {
    ExPolygon {
        contour: vec![
            Point::from_mm(x0, y0),
            Point::from_mm(x1, y0),
            Point::from_mm(x1, y1),
            Point::from_mm(x0, y1),
        ],
        holes: vec![],
    }
}

/// Side walls of a vertical prism over `rings` (counter-clockwise outer rings, clockwise holes):
/// enough to slice (the caps lie on the slice-free planes `z0` and `z1`).
fn prism(rings: &[Vec<(f32, f32)>], z0: f32, z1: f32) -> Vec<[[f32; 3]; 3]> {
    let mut tris = Vec::new();
    for ring in rings {
        for k in 0..ring.len() {
            let (a, b) = (ring[k], ring[(k + 1) % ring.len()]);
            let (p, q) = ([a.0, a.1, z0], [b.0, b.1, z0]);
            let (r, t) = ([b.0, b.1, z1], [a.0, a.1, z1]);
            tris.push([p, q, r]);
            tris.push([p, r, t]);
        }
    }
    tris
}

fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> Vec<(f32, f32)> {
    vec![(x0, y0), (x1, y0), (x1, y1), (x0, y1)]
}

fn hole(x0: f32, y0: f32, x1: f32, y1: f32) -> Vec<(f32, f32)> {
    let mut r = rect(x0, y0, x1, y1);
    r.reverse();
    r
}

fn settings(generator: PerimeterGenerator, perimeters: usize) -> PrintSettings {
    let mut s = PrintSettings::core_one_hf04_speed_pla();
    s.perimeter_generator = generator;
    s.perimeters = perimeters;
    s.brick.enabled = true;
    s
}

fn layers(tris: &[[[f32; 3]; 3]], s: &PrintSettings) -> Vec<LayerToolpaths> {
    let mut mesh = TriangleMesh::from_triangles(tris);
    mesh.place_on_bed(125.0, 105.0);
    let obj = crate::print::prepare_object(&mesh, s).unwrap();
    obj.toolpaths(0..obj.layer_count())
}

use crate::toolpath::LayerToolpaths;

fn raised(p: &ExtrusionPath) -> bool {
    p.z_raise() > 0.0
}

#[test]
fn classify_follows_the_table() {
    // A 30 mm line along y = 0; raised here on x 10..20, raised below on x 5..15, skipped
    // below on x 22..26.
    let line = vec![Point::from_mm(0.0, 0.0), Point::from_mm(30.0, 0.0)];
    let here = [square(10.0, -1.0, 20.0, 1.0)];
    let below = [square(5.0, -1.0, 15.0, 1.0)];
    let held = [square(22.0, -1.0, 26.0, 1.0)];
    let areas = [Area::new(&here), Area::new(&below), Area::new(&held)];
    let classes: Vec<(i64, i64, Class)> = classify(&line, &areas)
        .into_iter()
        .map(|(a, b, c)| ((a / 1e6).round() as i64, (b / 1e6).round() as i64, c))
        .collect();
    assert_eq!(
        classes,
        [
            (0, 5, Class::Keep),
            (5, 10, Class::Skip),
            (10, 15, Class::Raised(Below::Raised)),
            (15, 20, Class::Raised(Below::Normal)),
            (20, 22, Class::Keep),
            (22, 26, Class::Tall),
            (26, 30, Class::Keep),
        ]
    );
}

#[test]
fn cut_keeps_direction_and_the_wipe_on_the_last_stretch() {
    let path = ExtrusionPath {
        role: Role::Perimeter,
        perimeter_index: Some(1),
        points: vec![
            Point::from_mm(0.0, 0.0),
            Point::from_mm(10.0, 0.0),
            Point::from_mm(10.0, 10.0),
        ],
        flow: Flow::new(0.45, 0.2),
        seam: Some(Box::new(crate::seam::PathSeam {
            wipe: Some(Point::from_mm(9.0, 9.0)),
            ..Default::default()
        })),
        overhang: None,
        brick: None,
    };
    let a = cut(&path, 5e6, 15e6, false).unwrap();
    assert_eq!(
        a.points,
        [
            Point::from_mm(5.0, 0.0),
            Point::from_mm(10.0, 0.0),
            Point::from_mm(10.0, 5.0)
        ]
    );
    assert!(a.seam.as_ref().unwrap().wipe.is_none());
    let b = cut(&path, 15e6, 20e6, true).unwrap();
    assert_eq!(b.points[0], Point::from_mm(10.0, 5.0));
    assert!(b.seam.as_ref().unwrap().wipe.is_some());
}

/// Raised walls are the odd inner walls, around the outline and around a hole alike, also
/// where the hole is so close to the outline that their walls meet (Classic and Arachne).
#[test]
fn hole_next_to_contour_raises_odd_walls_only() {
    // A 20 × 20 mm block, 8 mm tall, with a 6 × 8 mm hole 1.5 mm from its left side.
    let tris = prism(
        &[rect(0.0, 0.0, 20.0, 20.0), hole(1.5, 6.0, 7.5, 14.0)],
        0.0,
        8.0,
    );
    for generator in [PerimeterGenerator::Classic, PerimeterGenerator::Arachne] {
        let s = settings(generator, 4);
        let layers = layers(&tris, &s);
        let mid = &layers[layers.len() / 2];
        let walls: Vec<&ExtrusionPath> = mid
            .paths
            .iter()
            .filter(|p| p.role == Role::Perimeter)
            .collect();
        assert!(walls.iter().any(|p| raised(p)), "{generator:?}");
        for p in &walls {
            let k = p.perimeter_index.unwrap();
            assert_eq!(raised(p), k % 2 == 1, "{generator:?}: wall {k}");
        }
        // Around the hole too: a raised wall inside the hole's surroundings (x < 9.5 mm from
        // the block's left side, y between the hole's ends).
        let left = Point::from_mm(125.0 - 10.0 + 9.5, 0.0).x;
        assert!(
            walls
                .iter()
                .filter(|p| raised(p))
                .any(|p| p.points.iter().all(|q| q.x < left)),
            "{generator:?}: a raised wall around the hole"
        );
        // Raised stretches come last on the layer.
        let first_raised = mid.paths.iter().position(raised).unwrap();
        assert!(mid.paths[first_raised..].iter().all(raised));
    }
}

/// A 20 mm block with a 10 mm tower on top: the block's walls stop being raised before the
/// step's top shell — one layer skipped, the next 1.5 layers tall — and the top and bottom
/// shells print as without brick layers.
#[test]
fn mid_part_top_rejoins_the_grid_below_the_top_shell() {
    let mut tris = prism(&[rect(0.0, 0.0, 20.0, 20.0)], 0.0, 6.0);
    tris.extend(prism(&[rect(5.0, 5.0, 15.0, 15.0)], 6.0, 12.0));
    // The block's top ring (z = 6) as a cap, so the mesh is closed where it matters.
    let s = settings(PerimeterGenerator::Classic, 3);
    let on = layers(&tris, &s);
    let mut plain = s.clone();
    plain.brick.enabled = false;
    let off = layers(&tris, &plain);
    assert_eq!(on.len(), off.len());
    let h = s.layer_height;
    // Object layers whose top is at or below 6 mm belong to the block.
    let block_top = on.iter().rposition(|l| l.print_z <= 6.0 + 1e-6).unwrap();
    // Walls of the block (outside the tower's footprint).
    let outer = |p: &ExtrusionPath| {
        p.points.iter().any(|q| {
            let (x, y) = (q.x as f64 / 1e6 - 115.0, q.y as f64 / 1e6 - 95.0);
            !(4.0..=16.0).contains(&x) || !(4.0..=16.0).contains(&y)
        })
    };
    let raised_layers: Vec<usize> = (0..=block_top)
        .filter(|&i| on[i].paths.iter().any(|p| raised(p) && outer(p)))
        .collect();
    let last = *raised_layers.last().expect("the block raises walls");
    let top_shell = s
        .top_solid_layers
        .max((s.top_solid_min_thickness / h).ceil() as usize);
    // The skip, the 1.5-layer bead and the margin come before the top shell.
    assert!(
        last + 2 + s.brick.end_margin_layers < block_top + 1 - top_shell,
        "last raised layer {last}, block top {block_top}"
    );
    // The layer after the last raised one has no odd inner wall on the block (skipped), the
    // one after that prints it at layer height, 1.5 layers tall.
    let odd = |p: &&ExtrusionPath| p.role == Role::Perimeter && p.perimeter_index == Some(1);
    assert!(!on[last + 1].paths.iter().filter(odd).any(&outer));
    let tall: Vec<&ExtrusionPath> = on[last + 2]
        .paths
        .iter()
        .filter(odd)
        .filter(|p| outer(p))
        .collect();
    assert!(!tall.is_empty());
    for p in tall {
        assert_eq!(p.z_raise(), 0.0);
        assert!((p.flow.height - 1.5 * h).abs() < 1e-6, "{}", p.flow.height);
    }
    // Shells: the block's top shell, the bottom and the tower's top print as without bricks.
    for i in (0..s.bottom_solid_layers + 1)
        .chain(block_top + 1 - top_shell..=block_top)
        .chain(on.len() - top_shell..on.len())
    {
        assert_eq!(on[i].paths, off[i].paths, "layer {i}");
    }
    // Every bead is at least a layer tall, and the first raised bead 1.5 layers.
    for l in &on {
        for p in l.paths.iter().filter(|p| p.brick.is_some()) {
            assert!(p.flow.height >= l.height - 1e-9);
            let ratio = p.brick.unwrap().flow_ratio;
            assert!(ratio >= 1.0);
        }
    }
    let first = raised_layers[0];
    assert!(
        on[first]
            .paths
            .iter()
            .filter(|p| raised(p))
            .all(|p| (p.flow.height - 1.5 * h).abs() < 1e-6)
    );
}

/// With variable layer heights a raised wall stands half the thinnest layer above its layer:
/// a raised bead on a raised bead is exactly its layer's height, a start taller.
#[test]
fn variable_layer_heights_keep_every_bead_a_layer_tall() {
    let heights = [0.2, 0.2, 0.1, 0.15, 0.25, 0.3, 0.1, 0.2, 0.2, 0.2, 0.2, 0.2];
    let mut tops = Vec::new();
    let mut z = 0.0;
    for h in heights {
        z += h;
        tops.push(z);
    }
    let n = heights.len();
    let outlines = vec![vec![square(0.0, 0.0, 20.0, 20.0)]; n];
    let mut s = settings(PerimeterGenerator::Classic, 3);
    s.top_solid_layers = 1;
    s.top_solid_min_thickness = 0.0;
    s.bottom_solid_layers = 1;
    s.bottom_solid_min_thickness = 0.0;
    s.brick.start_layer = 1;
    s.brick.end_margin_layers = 0;
    let plan = Plan::build(PlanInput {
        outlines: &outlines,
        first_layer: None,
        regions: None,
        tops: &tops,
        heights: &heights,
        settings: vec![&s],
        independent_support_layers: false,
    })
    .unwrap();
    let raised: Vec<usize> = (0..n)
        .filter(|&i| !plan.raised_area(i).is_empty())
        .collect();
    // Bottom shell 1 layer: from layer 1; top: one shell, the skip and the 1.5-layer bead.
    assert_eq!(raised, (1..n - 3).collect::<Vec<_>>());
    let wall = || ExtrusionPath {
        role: Role::Perimeter,
        perimeter_index: Some(1),
        points: vec![Point::from_mm(5.0, 5.0), Point::from_mm(15.0, 5.0)],
        flow: Flow::new(0.45, 0.2),
        seam: None,
        overhang: None,
        brick: None,
    };
    let half = 0.1 / 2.0;
    for layer in 1..n {
        let mut paths = vec![wall()];
        paths[0].flow = Flow::new(0.45, heights[layer]);
        plan.apply::<()>(layer, &mut paths, None);
        let h = heights[layer];
        match (raised.contains(&layer), raised.contains(&(layer - 1))) {
            (true, false) => {
                assert!((paths[0].flow.height - (h + half)).abs() < 1e-9);
                assert!((paths[0].z_raise() - half).abs() < 1e-9);
            }
            (true, true) => assert!((paths[0].flow.height - h).abs() < 1e-9, "{layer}"),
            (false, true) => assert!(paths.is_empty(), "skipped on layer {layer}"),
            (false, false) => {
                if raised.contains(&(layer - 2)) {
                    let expect = h + heights[layer - 1] - half;
                    assert!((paths[0].flow.height - expect).abs() < 1e-9, "{layer}");
                    assert_eq!(paths[0].z_raise(), 0.0);
                } else {
                    assert_eq!(paths[0], wall_at(h));
                }
            }
        }
        for p in &paths {
            assert!(p.flow.height >= h - 1e-9);
        }
    }
    fn wall_at(h: f64) -> ExtrusionPath {
        ExtrusionPath {
            role: Role::Perimeter,
            perimeter_index: Some(1),
            points: vec![Point::from_mm(5.0, 5.0), Point::from_mm(15.0, 5.0)],
            flow: Flow::new(0.45, h),
            seam: None,
            overhang: None,
            brick: None,
        }
    }
}

#[test]
fn settings_never_reduce_flow() {
    let mut c = Config::defaults();
    c.set_raw("mica_brick_layers", "1").unwrap();
    c.set_raw("mica_brick_layers_flow_ratio", "80%").unwrap();
    let s = BrickSettings::from_config(&c).unwrap();
    assert!(s.enabled);
    assert_eq!(s.flow_ratio, 1.0);
    c.set_raw("mica_brick_layers_flow_ratio", "110%").unwrap();
    assert!((BrickSettings::from_config(&c).unwrap().flow_ratio - 1.1).abs() < 1e-12);
    c.set_raw("spiral_vase", "1").unwrap();
    assert!(!BrickSettings::from_config(&c).unwrap().enabled);
    assert_eq!(
        BrickSettings::from_config(&Config::defaults()).unwrap(),
        BrickSettings::default()
    );
}
