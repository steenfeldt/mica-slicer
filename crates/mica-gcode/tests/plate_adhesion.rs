// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

use mica_core::{
    config::{Config, ConfigValue, Value},
    geometry::{BoundingBox, Point, clip::intersection_pl},
    mesh::TriangleMesh,
    print::LayerSource,
    print_job::{PreparedPrint, PrintObject, ToolAssignments},
    settings::PrintSettings,
    toolpath::Role,
};
use mica_gcode::export::export_gcode;

fn config(orca: bool) -> Config {
    let mut c = Config::defaults();
    for (k, v) in [
        ("nozzle_diameter", "0.4,0.4,0.4"),
        ("layer_height", "0.2"),
        ("first_layer_height", "0.2"),
        ("first_layer_speed", "30"),
        ("perimeter_generator", "classic"),
        ("bed_shape", "0x0,250x0,250x250,0x250"),
        ("use_relative_e_distances", "1"),
        ("wipe_tower", "0"),
        ("gcode_flavor", "marlin2"),
        ("skirts", "3"),
        ("skirt_height", "3"),
        ("skirt_distance", "3"),
        ("min_skirt_length", "0"),
        ("brim_width", "3"),
        ("gcode_label_objects", "firmware"),
        (
            "start_gcode",
            "; footprint {first_layer_print_min[0]},{first_layer_print_max[0]} hull {first_layer_print_convex_hull[0]}",
        ),
    ] {
        c.set_raw(k, v).unwrap();
    }
    if orca {
        c.set(
            "initial_layer_print_height",
            ConfigValue::Scalar(Value::Float(0.2)),
        );
        c.set_raw("gcode_flavor", "klipper").unwrap();
        c.set_raw("mica_slicing_rules", "orcaslicer").unwrap();
        c.set_raw("exclude_object", "1").unwrap();
    }
    c
}

fn object(c: &Config, id: usize, instance: usize, x: f32, tool: usize) -> PrintObject {
    let mut mesh = TriangleMesh::load_stl(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/models/cube20.stl"
    ))
    .unwrap();
    for v in &mut mesh.vertices {
        v[2] *= 0.04;
    }
    mesh.place_on_bed(x, 80.);
    PrintObject::prepare(
        format!("cube {id}"),
        id,
        instance,
        mesh,
        ToolAssignments([Some(tool), Some(tool), Some(tool), None, None]),
        c,
    )
    .unwrap()
}

#[test]
fn combined_plate_skirt_allocation_brim_and_batch_identity() {
    let c = config(false);
    let job =
        PreparedPrint::new(vec![object(&c, 0, 0, 40., 0), object(&c, 1, 0, 80., 2)], &c).unwrap();
    assert_eq!(job.adhesion.skirt.len(), 3);
    for skirt in &job.adhesion.skirt {
        let bb = BoundingBox::from_points(&skirt.points).unwrap();
        assert!(bb.min.x < Point::from_mm(30., 0.).x && bb.max.x > Point::from_mm(90., 0.).x);
    }
    let all = job.scheduled_toolpaths(0..job.layer_count());
    for (i, l) in all.iter().enumerate() {
        let one = job.scheduled_toolpaths(i..i + 1).remove(0);
        assert_eq!(l.layer.paths, one.layer.paths);
        assert_eq!(l.assignments, one.assignments);
        let skirt_tools: Vec<_> = l
            .layer
            .paths
            .iter()
            .zip(&l.assignments)
            .filter(|(p, _)| p.role == Role::Skirt)
            .map(|(_, a)| a.tool)
            .collect();
        assert_eq!(skirt_tools.len(), if i < 3 { 3 } else { 0 });
        if i < 3 {
            let tools = &job.ordering.layers[i].extruders;
            assert_eq!(skirt_tools, [tools[0], tools[0], tools[1]]);
            assert!(
                l.layer
                    .paths
                    .iter()
                    .filter(|p| p.role == Role::Skirt)
                    .all(|p| (p.flow.height - 0.2).abs() < 1e-6)
            );
        }
        for (p, a) in l.layer.paths.iter().zip(&l.assignments) {
            if p.role == Role::Brim {
                assert_eq!((i, a.tool), (0, job.ordering.first));
            }
        }
    }
    assert!(
        job.objects
            .iter()
            .all(|o| o.prepared.adhesion.skirt.is_empty())
    );
    let g = export_gcode(&job, &c, &PrintSettings::from_config(&c).unwrap(), "plate")
        .unwrap()
        .gcode;
    let mut label = false;
    for line in g.lines() {
        if line == "M486 S-1" {
            label = false;
        } else if line.starts_with("M486 S") {
            label = true;
        }
        if line == ";TYPE:Skirt/Brim" {
            assert!(!label);
        }
    }
    assert!(g.contains("; footprint "));
}

#[test]
fn orca_per_object_skirts_keep_brim_tools_and_group_copies() {
    let mut c = config(true);
    c.set_raw("skirt_type", "perobject").unwrap();
    let job =
        PreparedPrint::new(vec![object(&c, 0, 0, 40., 0), object(&c, 1, 0, 90., 2)], &c).unwrap();
    assert_eq!(job.adhesion.skirt.len(), 6);
    let first = job.scheduled_toolpaths(0..1).remove(0);
    let mut brim_tools = std::collections::BTreeSet::new();
    for (p, a) in first.layer.paths.iter().zip(&first.assignments) {
        if p.role == Role::Skirt {
            assert_eq!(a.tool, job.ordering.first);
        }
        if p.role == Role::Brim {
            brim_tools.insert(a.tool);
            assert_eq!(a.tool, job.objects[a.object].tools.0[0].unwrap());
        }
    }
    assert_eq!(brim_tools, [0, 2].into());
    let copies =
        PreparedPrint::new(vec![object(&c, 0, 0, 40., 0), object(&c, 0, 1, 90., 0)], &c).unwrap();
    assert_eq!(copies.adhesion.skirt.len(), 3);
    assert!(copies.adhesion.skirt.iter().all(|p| {
        let bb = BoundingBox::from_points(&p.points).unwrap();
        bb.min.x < Point::from_mm(30., 0.).x && bb.max.x > Point::from_mm(100., 0.).x
    }));
}

#[test]
fn neighboring_objects_exclude_brims_and_merge_colliding_skirts() {
    for orca in [false, true] {
        let mut c = config(orca);
        c.set_raw("skirt_type", "perobject").unwrap();
        let job = PreparedPrint::new(vec![object(&c, 0, 0, 40., 0), object(&c, 1, 0, 63., 2)], &c)
            .unwrap();
        assert_eq!(job.adhesion.skirt.len(), 3);
        let obstacles: Vec<_> = job
            .objects
            .iter()
            .flat_map(|o| o.prepared.layer_outlines(0).unwrap().iter().cloned())
            .collect();
        for p in &job.adhesion.brim {
            assert!(intersection_pl(std::slice::from_ref(&p.points), &obstacles).is_empty());
        }
        let bb = job.first_layer_bbox().unwrap();
        assert_eq!(
            Some(bb),
            BoundingBox::from_points(job.first_layer_hull().unwrap())
        );
    }
}

#[test]
fn tower_is_inside_combined_skirt_and_first_layer_hull() {
    let mut c = config(false);
    for (k, v) in [
        ("wipe_tower", "1"),
        ("wipe_tower_x", "150"),
        ("wipe_tower_y", "140"),
        ("wipe_tower_width", "40"),
    ] {
        c.set_raw(k, v).unwrap();
    }
    let job =
        PreparedPrint::new(vec![object(&c, 0, 0, 40., 0), object(&c, 1, 0, 80., 2)], &c).unwrap();
    let tower = job.wipe_tower.as_ref().unwrap().first_layer_bbox().unwrap();
    let hull = job.first_layer_bbox().unwrap();
    assert!(hull.min.x < tower.min.x && hull.max.x > tower.max.x && hull.max.y > tower.max.y);
    assert!(job.object_first_layer_bbox().unwrap().max.x < tower.min.x);
    for path in &job.adhesion.skirt {
        assert!(BoundingBox::from_points(&path.points).unwrap().max.y > tower.max.y);
    }
}

#[test]
fn raft_footprint_and_skirts_use_printed_layer_heights() {
    let mut c = config(false);
    c.set_raw("raft_layers", "3").unwrap();
    c.set_raw("support_material", "1").unwrap();
    let job =
        PreparedPrint::new(vec![object(&c, 0, 0, 40., 0), object(&c, 1, 0, 80., 2)], &c).unwrap();
    assert!(job.adhesion.brim.is_empty());
    let mut last_z = 0.;
    let mut skirt_layers = 0;
    for l in job.toolpaths(0..job.layer_count()) {
        if let Some(p) = l.paths.iter().find(|p| p.role == Role::Skirt) {
            assert!((p.flow.height - (l.print_z - last_z)).abs() < 1e-6);
            last_z = l.print_z;
            skirt_layers += 1;
        }
    }
    assert_eq!(skirt_layers, 3);
}
