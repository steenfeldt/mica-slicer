// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Small, deterministic dense painting guard; no private project or vendor profile required.
//! Run with `cargo test --offline --release -p mica-core --test performance -- --ignored --nocapture`.
use mica_core::{
    config::Config,
    mesh::TriangleMesh,
    painting::FacetPainting,
    print::LayerSource,
    print_job::{PrintObject, ToolAssignments},
    regions::{RegionModel, Volume},
    threemf::PartKind,
};
use std::time::{Duration, Instant};

#[test]
#[ignore = "release performance guard; run with --release -- --ignored"]
fn dense_painted_slice_finishes_within_budget() {
    let _timings = mica_core::timing::Report::default();
    // A closed corrugated sphere, with four material sectors and curved painted caps.
    // Many small facets exercise projection, cap unions, segmentation and regional walls.
    const COLUMNS: usize = 512;
    const RINGS: usize = 64;
    let mut mesh = TriangleMesh::default();
    mesh.vertices.push([0., 0., 20.]);
    for i in 1..=RINGS {
        let phi = std::f64::consts::PI * i as f64 / (RINGS + 1) as f64;
        for j in 0..COLUMNS {
            let theta = std::f64::consts::TAU * j as f64 / COLUMNS as f64;
            let r = (10. + 0.3 * (theta * 12.).cos()) * phi.sin();
            mesh.vertices.push([
                (r * theta.cos()) as f32,
                (r * theta.sin()) as f32,
                (10. + 10. * phi.cos()) as f32,
            ]);
        }
    }
    let bottom = mesh.vertices.len() as u32;
    mesh.vertices.push([0., 0., 0.]);
    let mut painting = FacetPainting::default();
    let mut face = |a, b, c, column: usize| {
        let wire = ["4", "8", "0C", "1C"][column * 4 / COLUMNS];
        painting
            .material
            .push(mesh.faces.len() as u32, wire)
            .unwrap();
        mesh.faces.push([a, b, c]);
    };
    let v = |ring: usize, col: usize| (1 + ring * COLUMNS + col % COLUMNS) as u32;
    for j in 0..COLUMNS {
        face(0, v(0, j), v(0, j + 1), j);
        for i in 0..RINGS - 1 {
            face(v(i, j), v(i + 1, j), v(i + 1, j + 1), j);
            face(v(i, j), v(i + 1, j + 1), v(i, j + 1), j);
        }
        face(v(RINGS - 1, j), bottom, v(RINGS - 1, j + 1), j);
    }
    let model = RegionModel {
        full_spectrum: Default::default(),
        volumes: vec![Volume {
            mesh: mesh.clone(),
            kind: PartKind::Model,
            config: Config::new(),
            painting,
        }],
        height_ranges: vec![],
        layer_height_profile: vec![],
        brim_ears: vec![],
    };
    let mut config = Config::defaults();
    for (k, v) in [
        ("nozzle_diameter", "0.4,0.4,0.4,0.4"),
        ("perimeter_generator", "arachne"),
        ("seam_position", "rear"),
        ("skirts", "0"),
        ("wipe_tower", "0"),
    ] {
        config.set_raw(k, v).unwrap();
    }
    let tools =
        ToolAssignments::resolve(&config, &Default::default(), &Default::default(), 0, 4).unwrap();
    let start = Instant::now();
    let object =
        PrintObject::prepare_regions("dense painted".into(), 0, 0, mesh, &model, tools, &config)
            .unwrap();
    let mut lengths = [0.; 4];
    for begin in (0..object.prepared.layer_count()).step_by(16) {
        for (layer, regions) in object
            .prepared
            .regional_toolpaths(begin..(begin + 16).min(object.prepared.layer_count()))
        {
            for (path, region) in layer.paths.iter().zip(regions) {
                let tool = region
                    .and_then(|r| object.prepared.region(r))
                    .map_or(&object.tools, |r| &r.tools)
                    .tool(path.role)
                    .unwrap_or(0);
                lengths[tool] += mica_core::geometry::polyline_length(&path.points);
            }
        }
    }
    let elapsed = start.elapsed();
    eprintln!(
        "65,536 painted triangles, {} layers: {elapsed:.3?}",
        object.prepared.layer_count()
    );
    assert!(
        lengths.iter().all(|v| v.is_finite() && *v > 0.),
        "missing material: {lengths:?}"
    );
    assert!(
        elapsed < Duration::from_secs(20),
        "painted slice took {elapsed:?}; run this guard with --release"
    );
}
