// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Embossed shapes on a cube: an engraved negative volume and a raised part slice as expected,
//! and the editable source survives a 3MF round trip in both dialects.

use mica_core::{
    config::Config,
    emboss::{self, EmbossConfig, EmbossRange, EmbossSource, SvgSource, TextSource, config},
    geometry::{ExPolygon, Point, Polygon, SCALE, signed_area},
    mesh::TriangleMesh,
    mesh_cut::is_watertight,
    print::LayerSource,
    print_job::{PrintObject, ToolAssignments},
    regions::{RegionModel, Volume},
    threemf::{self, Flavour, Instance, Object, Part, PartKind, Project, ResourceId, Transform},
};
use std::sync::Arc;

fn cube() -> TriangleMesh {
    let mut m = mica_core::mesh_edit::cuboid([20., 20., 20.]);
    let (min, _) = m.bounds();
    m.translate([-min[0], -min[1], -min[2]]);
    m
}

/// An "O": a 10 × 6 mm frame with a 1.5 mm wall, centred on the origin.
fn letter_o() -> Vec<ExPolygon> {
    let rect = |x0: f64, y0: f64, x1: f64, y1: f64| -> Polygon {
        [[x0, y0], [x1, y0], [x1, y1], [x0, y1]]
            .iter()
            .map(|p| Point::from_mm(p[0], p[1]))
            .collect()
    };
    let mut hole = rect(-3.5, -1.5, 3.5, 1.5);
    hole.reverse();
    emboss::heal(&[rect(-5., -3., 5., 3.), hole], true)
}

fn placed(mesh: &TriangleMesh, t: Transform) -> TriangleMesh {
    TriangleMesh {
        vertices: mesh.vertices.iter().map(|v| t.apply(*v)).collect(),
        faces: mesh.faces.clone(),
    }
}

fn config() -> Config {
    let mut c = Config::defaults();
    for (k, v) in [
        ("layer_height", "0.2"),
        ("first_layer_height", "0.2"),
        ("perimeter_generator", "classic"),
        ("elefant_foot_compensation", "0"),
        ("skirts", "0"),
        ("brim_width", "0"),
        ("wipe_tower", "0"),
    ] {
        c.set_raw(k, v).unwrap();
    }
    c
}

fn prepare(m: &RegionModel) -> PrintObject {
    let c = config();
    let mut mesh = TriangleMesh::default();
    for v in m.volumes.iter().filter(|v| v.kind == PartKind::Model) {
        let base = mesh.vertices.len() as u32;
        mesh.vertices.extend_from_slice(&v.mesh.vertices);
        mesh.faces
            .extend(v.mesh.faces.iter().map(|f| f.map(|i| i + base)));
    }
    let tools =
        ToolAssignments::resolve(&c, &Default::default(), &Default::default(), 0, 1).unwrap();
    PrintObject::prepare_regions("emboss".into(), 0, 0, mesh, m, tools, &c).unwrap()
}

fn area(p: &[ExPolygon]) -> f64 {
    p.iter()
        .map(|p| {
            signed_area(&p.contour).abs()
                - p.holes.iter().map(|h| signed_area(h).abs()).sum::<f64>()
        })
        .sum::<f64>()
        / (SCALE * SCALE)
}

fn volume(mesh: TriangleMesh, kind: PartKind) -> Volume {
    Volume {
        painting: Default::default(),
        mesh,
        kind,
        config: Config::new(),
    }
}

#[test]
fn engraved_negative_volume_cuts_the_top_and_raised_part_adds_layers() {
    let shape = letter_o();
    let o_area = 60. - 21.;
    // Engraved 1 mm into the top face (z = 20).
    let top = emboss::transform_onto_surface([10., 10., 20.], [0., 0., 1.], emboss::UP_LIMIT);
    let engraved = emboss::emboss_mesh(&shape, EmbossRange::new(false, 1.), None, 0.5);
    assert!(is_watertight(&engraved));
    let mut m = RegionModel {
        volumes: vec![
            volume(cube(), PartKind::Model),
            volume(placed(&engraved, top), PartKind::NegativeVolume),
        ],
        height_ranges: vec![],
        ..Default::default()
    };
    let o = prepare(&m);
    assert_eq!(o.prepared.layer_count(), 100);
    // Layer 97 (z 19.4–19.6) lies inside the engraving, layer 90 below it.
    let cut = area(o.prepared.layer_outlines(97).unwrap());
    assert!(
        (cut - (400. - o_area)).abs() < 0.05,
        "engraved layer area {cut}"
    );
    let below = area(o.prepared.layer_outlines(90).unwrap());
    assert!((below - 400.).abs() < 0.05, "{below}");

    // The same letter raised 1 mm on the side wall facing -Y, and on the top.
    let side = emboss::transform_onto_surface([10., 0., 10.], [0., -1., 0.], emboss::UP_LIMIT);
    let raised = emboss::emboss_mesh(&shape, EmbossRange::new(true, 1.), None, 0.5);
    assert!(is_watertight(&raised));
    m.volumes[1] = volume(placed(&raised, side), PartKind::Model);
    m.volumes.push(volume(
        placed(
            &raised,
            emboss::transform_onto_surface([10., 10., 20.], [0., 0., 1.], 0.9),
        ),
        PartKind::Model,
    ));
    let o = prepare(&m);
    // 20 mm cube + 0.985 mm of letter on top: five more layers.
    assert_eq!(o.prepared.layer_count(), 105);
    let top_layer = area(o.prepared.layer_outlines(102).unwrap());
    assert!(
        (top_layer - o_area).abs() < 0.05,
        "raised top layer {top_layer}"
    );
    // Mid-height: the side letter's 1.5 mm vertical strokes stick out of the -Y wall.
    let mid = area(o.prepared.layer_outlines(50).unwrap());
    assert!(
        (mid - (400. + 2. * 1.5 * 0.985)).abs() < 0.1,
        "side letter layer {mid}"
    );
}

fn part(name: &str, kind: PartKind, mesh: TriangleMesh, metadata: threemf::Settings) -> Part {
    Part {
        name: name.into(),
        kind,
        source: ResourceId {
            path: "3D/3dmodel.model".into(),
            id: 1,
        },
        mesh,
        source_mesh: None,
        painting: Default::default(),
        transform: Transform::IDENTITY,
        settings: Default::default(),
        metadata,
    }
}

#[test]
fn text_and_svg_sources_round_trip_through_3mf() {
    let shape = letter_o();
    let top = emboss::transform_onto_surface([10., 10., 20.], [0., 0., 1.], emboss::UP_LIMIT);
    let mesh = placed(
        &emboss::emboss_mesh(&shape, EmbossRange::new(false, 1.), None, 0.5),
        top,
    );
    let text = EmbossConfig {
        source: EmbossSource::Text(TextSource {
            text: "O\n<o> & \"o\"".into(),
            family: "IBM Plex Sans".into(),
            face: "Regular".into(),
            file_name: "IBMPlexSans-Regular.ttf".into(),
            ..Default::default()
        }),
        depth: 1.,
        use_surface: false,
    };
    let svg = EmbossConfig {
        source: EmbossSource::Svg(SvgSource {
            path: "logo.svg".into(),
            path_in_3mf: "3D/logo.svg".into(),
            data: Arc::from(
                "<svg xmlns=\"http://www.w3.org/2000/svg\"><rect width=\"4\" height=\"4\"/></svg>",
            ),
            mm_per_unit: 0.5,
            mirror: [false, true],
            tolerance_mm: 0.1,
        }),
        depth: 2.,
        use_surface: true,
    };
    let project = Project {
        objects: vec![Object {
            height_ranges: vec![],
            layer_height_profile: vec![],
            brim_points: vec![],
            name: "cube".into(),
            source: ResourceId {
                path: "3D/3dmodel.model".into(),
                id: 1,
            },
            parts: vec![
                part("cube", PartKind::Model, cube(), Default::default()),
                part(
                    "text",
                    PartKind::NegativeVolume,
                    mesh.clone(),
                    text.to_metadata(top),
                ),
                part("logo", PartKind::Model, mesh, svg.to_metadata(top)),
            ],
            instances: vec![Instance {
                transform: Transform::IDENTITY,
                printable: true,
                plate: None,
            }],
            settings: Default::default(),
            metadata: Default::default(),
        }],
        ..Default::default()
    };
    for flavour in [Flavour::Prusa, Flavour::Orca] {
        let path = std::env::temp_dir().join(format!(
            "mica-emboss-{}-{}.3mf",
            std::process::id(),
            flavour.name()
        ));
        threemf::write(&path, &project, flavour).unwrap();
        // The archive carries the SVG file and PrusaSlicer's elements.
        let mut zip = zip::ZipArchive::new(std::fs::File::open(&path).unwrap()).unwrap();
        assert!(zip.by_name("3D/logo.svg").is_ok());
        let config_name = match flavour {
            Flavour::Orca => "Metadata/model_settings.config",
            _ => "Metadata/Slic3r_PE_model.config",
        };
        let mut xml = String::new();
        std::io::Read::read_to_string(&mut zip.by_name(config_name).unwrap(), &mut xml).unwrap();
        assert!(xml.contains("<slic3rpe:text "), "{xml}");
        assert!(xml.contains("<slic3rpe:shape "), "{xml}");
        let back = threemf::load_3mf(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        let parts = &back.objects[0].parts;
        assert_eq!(parts.len(), 3);
        assert!(!back.ignored_features.contains("editable text/shape source"));
        let (t, frame) = EmbossConfig::from_metadata(&parts[1].metadata).unwrap();
        assert_eq!(t, text, "{flavour:?}");
        let frame = frame.unwrap();
        for r in 0..3 {
            for c in 0..4 {
                assert!((frame.0[r][c] - top.0[r][c]).abs() < 1e-9);
            }
        }
        let (s, _) = EmbossConfig::from_metadata(&parts[2].metadata).unwrap();
        assert_eq!(s, svg, "{flavour:?}");
        // Emboss keys are metadata, never slicer settings.
        assert!(parts[1].settings.keys().all(|k| !config::is_emboss_key(k)));
        assert!(is_watertight(&parts[1].mesh));
    }
}
