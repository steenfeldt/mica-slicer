// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Supports read the first layer without the elephant foot compensation, as PrusaSlicer's and
//! Orca's support generators read the uncompensated `lslices` of the first layer.

use mica_core::print::LayerSource;
use mica_core::{
    config::Config,
    mesh::TriangleMesh,
    painting::FacetPainting,
    regions::{RegionModel, Volume},
    threemf::PartKind,
    toolpath::Role,
};

fn box_mesh(lo: [f32; 3], hi: [f32; 3]) -> TriangleMesh {
    let v = |x: usize, y: usize, z: usize| {
        [
            if x == 0 { lo[0] } else { hi[0] },
            if y == 0 { lo[1] } else { hi[1] },
            if z == 0 { lo[2] } else { hi[2] },
        ]
    };
    TriangleMesh {
        vertices: vec![
            v(0, 0, 0),
            v(1, 0, 0),
            v(1, 1, 0),
            v(0, 1, 0),
            v(0, 0, 1),
            v(1, 0, 1),
            v(1, 1, 1),
            v(0, 1, 1),
        ],
        faces: vec![
            [0, 2, 1],
            [0, 3, 2],
            [4, 5, 6],
            [4, 6, 7],
            [0, 1, 5],
            [0, 5, 4],
            [1, 2, 6],
            [1, 6, 5],
            [2, 3, 7],
            [2, 7, 6],
            [3, 0, 4],
            [3, 4, 7],
        ],
    }
}

/// Support extrusion (layer, role, points) of a stem carrying a wide roof.
fn support_paths(elephant_foot: &str, style: &str) -> Vec<(usize, Vec<(i64, i64)>)> {
    let mut c = Config::defaults();
    for (k, v) in [
        ("skirts", "0"),
        ("support_material", "1"),
        ("support_material_auto", "1"),
        ("support_material_style", style),
        ("raft_layers", "0"),
        ("elefant_foot_compensation", elephant_foot),
    ] {
        c.set_raw(k, v).unwrap();
    }
    let volume = |mesh| Volume {
        mesh,
        kind: PartKind::Model,
        config: Config::new(),
        painting: FacetPainting::default(),
    };
    let model = RegionModel {
        full_spectrum: Default::default(),
        volumes: vec![
            volume(box_mesh([6., 6., 0.], [14., 14., 6.])),
            volume(box_mesh([0., 0., 6.], [20., 20., 7.])),
        ],
        height_ranges: vec![],
        layer_height_profile: vec![],
        brim_ears: vec![],
    };
    let mut mesh = TriangleMesh::default();
    for v in &model.volumes {
        let base = mesh.vertices.len() as u32;
        mesh.vertices.extend_from_slice(&v.mesh.vertices);
        mesh.faces
            .extend(v.mesh.faces.iter().map(|f| f.map(|i| i + base)));
    }
    let tools = mica_core::print_job::ToolAssignments::resolve(
        &c,
        &Default::default(),
        &Default::default(),
        0,
        c.vector_len("nozzle_diameter"),
    )
    .unwrap();
    let o = mica_core::print_job::PrintObject::prepare_regions(
        "stem".into(),
        0,
        0,
        mesh,
        &model,
        tools,
        &c,
    )
    .unwrap();
    let layers = o.prepared.toolpaths(0..o.prepared.layer_count());
    layers
        .iter()
        .enumerate()
        .flat_map(|(i, l)| {
            l.paths
                .iter()
                .filter(|p| {
                    matches!(
                        p.role,
                        Role::SupportMaterial | Role::SupportMaterialInterface
                    )
                })
                .map(move |p| (i, p.points.iter().map(|q| (q.x, q.y)).collect()))
        })
        .collect()
}

#[test]
fn supports_ignore_the_elephant_foot_compensation_of_the_first_layer() {
    for style in ["organic", "grid"] {
        let plain = support_paths("0", style);
        assert!(!plain.is_empty(), "{style}: no support generated");
        assert_eq!(
            support_paths("0.3", style),
            plain,
            "{style}: the elephant foot compensation changed the supports"
        );
    }
}
