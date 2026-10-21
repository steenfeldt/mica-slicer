// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

use mica_core::threemf::{PartKind, Project, read_3mf};
use std::io::{Cursor, Write};
use zip::{ZipWriter, write::SimpleFileOptions};

const NS: &str = "xmlns=\"http://schemas.microsoft.com/3dmanufacturing/core/2015/02\" xmlns:p=\"http://schemas.microsoft.com/3dmanufacturing/production/2015/06\"";

fn archive(entries: &[(&str, &str)]) -> Cursor<Vec<u8>> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    zip.start_file("[Content_Types].xml", SimpleFileOptions::default())
        .unwrap();
    zip.write_all(br#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="model" ContentType="application/vnd.ms-package.3dmanufacturing-3dmodel+xml"/></Types>"#).unwrap();
    for (name, contents) in entries {
        zip.start_file(
            *name,
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated),
        )
        .unwrap();
        zip.write_all(contents.as_bytes()).unwrap();
    }
    zip.finish().unwrap()
}
fn tetrahedra(n: usize) -> String {
    let mut vertices = String::new();
    let mut triangles = String::new();
    for i in 0..n {
        for [x, y, z] in [[0, 0, 0], [1, 0, 0], [0, 1, 0], [0, 0, 1]] {
            vertices += &format!(r#"<vertex x="{}" y="{y}" z="{z}"/>"#, x + i * 3);
        }
        for [a, b, c] in [[0, 2, 1], [0, 1, 3], [0, 3, 2], [1, 2, 3]] {
            triangles += &format!(
                r#"<triangle v1="{}" v2="{}" v3="{}"/>"#,
                a + i * 4,
                b + i * 4,
                c + i * 4
            );
        }
    }
    format!("<mesh><vertices>{vertices}</vertices><triangles>{triangles}</triangles></mesh>")
}
fn model(unit: &str, objects: &str, build: &str) -> String {
    format!(
        r#"<model {NS} unit="{unit}"><resources>{objects}</resources><build>{build}</build></model>"#
    )
}
fn simple(unit: &str) -> String {
    model(
        unit,
        &format!(
            r#"<object id="1" name="A &amp; B">{}</object>"#,
            tetrahedra(1)
        ),
        r#"<item objectid="1"/>"#,
    )
}
fn load(model: &str, config: Option<(&str, &str)>) -> Project {
    let mut entries = vec![("3D/3dmodel.model", model)];
    entries.extend(config);
    read_3mf(archive(&entries)).unwrap()
}
fn near(a: f64, b: f64) {
    assert!((a - b).abs() < b.abs().max(1.0) * 1e-6, "{a} != {b}");
}

#[test]
fn facet_annotations_follow_prusa_ranges_and_orca_component_winding() {
    for prefix in ["slic3rpe:", ""] {
        let keys = if prefix.is_empty() {
            [
                "paint_supports",
                "paint_seam",
                "paint_color",
                "paint_fuzzy_skin",
            ]
        } else {
            [
                "custom_supports",
                "custom_seam",
                "mmu_segmentation",
                "fuzzy_skin",
            ]
        };
        let attrs = keys
            .iter()
            .map(|k| format!(" {prefix}{k}=\"481\""))
            .collect::<String>();
        let mesh = tetrahedra(2).replace("v1=\"4\"", &format!("{attrs} v1=\"4\""));
        let root = model(
            "millimeter",
            &format!(r#"<object id="1">{mesh}</object>"#),
            r#"<item objectid="1"/>"#,
        );
        let p = load(
            &root,
            Some((
                "Metadata/Slic3r_PE_model.config",
                r#"<config><object id="1"><volume firstid="0" lastid="3"/><volume firstid="4" lastid="7"/></object></config>"#,
            )),
        );
        assert_eq!(p.objects[0].parts.len(), 2);
        assert!(p.objects[0].parts[0].painting.material.is_empty());
        let part = &p.objects[0].parts[1];
        for states in [
            &part.painting.supports,
            &part.painting.seam,
            &part.painting.material,
            &part.painting.fuzzy,
        ] {
            let mut leaves = 0;
            states.visit(&part.mesh, false, |state, t| {
                assert!(state == 1 || state == 2);
                assert!(t.iter().all(|p| p[0] >= 3.));
                leaves += 1;
            });
            assert_eq!(leaves, 6);
        }
    }
    let mesh = tetrahedra(1).replace("v1=\"0\"", "paint_seam=\"481\" v1=\"0\"");
    let nested = model(
        "millimeter",
        &format!(
            r#"<object id="1">{mesh}</object><object id="2"><components><component objectid="1" transform="-1 0 0 0 1 0 0 0 1 10 0 0"/></components></object>"#
        ),
        r#"<item objectid="2"/>"#,
    );
    let p = load(&nested, None);
    let part = &p.objects[0].parts[0];
    let mut states = vec![];
    part.painting.seam.visit(&part.mesh, false, |s, t| {
        states.push(s);
        assert!(t.iter().all(|p| p[0] >= 9. && p[0] <= 10.));
    });
    assert_eq!(states, vec![2, 1, 2, 1, 2, 1]);
}

#[test]
fn literal_hash_in_orca_model_path_and_invalid_paint_are_handled() {
    let child = model(
        "millimeter",
        &format!(r#"<object id="1">{}</object>"#, tetrahedra(1)),
        "",
    );
    let root = model(
        "millimeter",
        r#"<object id="2"><components><component objectid="1" p:path="/3D/Objects/#paint.model"/></components></object>"#,
        r#"<item objectid="2"/>"#,
    );
    let p = read_3mf(archive(&[
        ("3D/3dmodel.model", &root),
        ("3D/Objects/#paint.model", &child),
    ]))
    .unwrap();
    assert_eq!(p.objects[0].parts[0].mesh.faces.len(), 4);
    let bad = simple("millimeter").replace("v1=\"0\"", "paint_color=\"12\" v1=\"0\"");
    assert!(read_3mf(archive(&[("3D/3dmodel.model", &bad)])).is_err());
}

#[test]
fn all_core_units_and_default_transform() {
    for (unit, scale) in [
        ("micron", 0.001),
        ("millimeter", 1.0),
        ("centimeter", 10.0),
        ("inch", 25.4),
        ("foot", 304.8),
        ("meter", 1000.0),
    ] {
        let p = load(&simple(unit), None);
        assert_eq!(p.objects.len(), 1);
        assert_eq!(p.objects[0].name, "A & B");
        let m = p.placed_model_mesh().unwrap();
        assert_eq!(m.faces.len(), 4);
        near(m.volume(), scale * scale * scale / 6.0);
        near(f64::from(m.bounds().1[0]), scale);
    }
}

#[test]
fn nested_noncommuting_transforms_mirrors_and_instances() {
    let objects = format!(
        r#"<object id="1">{}</object>
      <object id="2"><components><component objectid="1" transform="1 0 0 0 1 0 0 0 1 2 0 0"/></components></object>
      <object id="3"><components><component objectid="2" transform="0 1 0 -1 0 0 0 0 1 0 0 0"/></components></object>"#,
        tetrahedra(1)
    );
    let p = load(
        &model(
            "inch",
            &objects,
            r#"<item objectid="3" transform="-2 0 0 0 3 0 0 0 4 10 20 1"/><item objectid="3" transform="1 0 0 0 1 0 0 0 1 0 0 2"/><item objectid="3" printable="false"/>"#,
        ),
        None,
    );
    let o = &p.objects[0];
    assert_eq!(o.instances.len(), 3);
    near(f64::from(o.parts[0].mesh.bounds().0[1]), 50.8);
    let m = o.placed_model_mesh().unwrap();
    assert_eq!(m.faces.len(), 8);
    near(m.volume(), 25.4_f64.powi(3) * 25.0 / 6.0);
    near(f64::from(m.bounds().1[0]), 12.0 * 25.4);
    near(f64::from(m.bounds().1[1]), 29.0 * 25.4);
    near(f64::from(m.bounds().0[2]), 25.4);
}

#[test]
fn production_paths_relationships_units_and_scoped_ids() {
    let root = model(
        "millimeter",
        r#"<object id="1"><components><component p:path="Objects/a%20b.model" objectid="1" transform="1 0 0 0 1 0 0 0 1 10 0 0"/><component p:path="/3D/Objects/second.model" objectid="1"/></components></object>"#,
        r#"<item objectid="1" transform="1 0 0 0 1 0 0 0 1 100 50 0"/>"#,
    );
    let first = simple("inch");
    let second = simple("centimeter");
    let rels = r#"<Relationships><Relationship Target="Objects/a%20b.model" Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel"/><Relationship Target="/3D/Objects/second.model" Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel"/></Relationships>"#;
    let p = read_3mf(archive(&[
        ("3D/3dmodel.model", &root),
        ("3D/Objects/a b.model", &first),
        ("3D/Objects/second.model", &second),
        ("3D/_rels/3dmodel.model.rels", rels),
    ]))
    .unwrap();
    assert_eq!(p.objects.len(), 1);
    assert_eq!(p.objects[0].parts.len(), 2);
    near(
        p.placed_model_mesh().unwrap().volume(),
        (25.4_f64.powi(3) + 1000.0) / 6.0,
    );
    near(
        f64::from(p.placed_model_mesh().unwrap().bounds().0[0]),
        100.0,
    );
}

#[test]
fn root_relationship_and_prefixed_core_xml() {
    let root = simple("millimeter")
        .replace(
            "<model ",
            "<c:model xmlns:c=\"http://schemas.microsoft.com/3dmanufacturing/core/2015/02\" ",
        )
        .replace("</model>", "</c:model>");
    let p=read_3mf(archive(&[("Models/main.model",&root),("_rels/.rels",r#"<Relationships><Relationship Target="/Models/main.model" Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel"/></Relationships>"#)])).unwrap();
    assert_eq!(p.objects[0].source.path, "Models/main.model");
}

#[test]
fn prusa_triangle_ranges_types_raw_settings_and_source_matrix() {
    let objects = format!(r#"<object id="1">{}</object>"#, tetrahedra(5));
    let mut config = String::from(
        r#"<config><object id="1"><metadata type="object" key="name" value="Assembly &amp; parts"/><metadata type="object" key="perimeters" value="5"/>"#,
    );
    let kinds = [
        "ModelPart",
        "NegativeVolume",
        "ParameterModifier",
        "SupportEnforcer",
        "SupportBlocker",
    ];
    for (i, kind) in kinds.iter().enumerate() {
        config += &format!(
            r#"<volume firstid="{}" lastid="{}"><metadata type="volume" key="volume_type" value="{kind}"/><metadata type="volume" key="name" value="part {i}"/><metadata type="volume" key="fill_density" value="17%"/><metadata type="volume" key="matrix" value="3 0 0 100 0 3 0 200 0 0 3 300 0 0 0 1"/></volume>"#,
            i * 4,
            i * 4 + 3
        );
    }
    config += "</object></config>";
    let p = load(
        &model("millimeter", &objects, r#"<item objectid="1"/>"#),
        Some(("Metadata/Slic3r_PE_model.config", &config)),
    );
    let o = &p.objects[0];
    assert_eq!(o.name, "Assembly & parts");
    assert_eq!(o.settings["perimeters"], "5");
    assert_eq!(
        o.parts.iter().map(|p| p.kind.clone()).collect::<Vec<_>>(),
        vec![
            PartKind::Model,
            PartKind::NegativeVolume,
            PartKind::Modifier,
            PartKind::SupportEnforcer,
            PartKind::SupportBlocker
        ]
    );
    for p in &o.parts {
        assert_eq!(p.mesh.vertices.len(), 4);
        assert_eq!(p.settings["fill_density"], "17%");
        assert!(p.metadata.contains_key("matrix"));
        near(p.mesh.volume(), 1.0 / 6.0);
    }
    assert_eq!(o.placed_model_mesh().unwrap().faces.len(), 4);
}

#[test]
fn orca_component_parts_and_per_instance_plates() {
    let objects = format!(
        r#"<object id="1">{}</object><object id="2"><components><component objectid="1"/><component objectid="1" transform="1 0 0 0 1 0 0 0 1 2 0 0"/></components></object>"#,
        tetrahedra(1)
    );
    let config = r#"<config><object id="2"><metadata key="name" value="Orca"/><metadata key="wall_loops" value="4"/><part id="1" subtype="normal_part"><metadata key="extruder" value="2"/></part><part id="1" subtype="modifier_part"><metadata key="volume_type" value="negative_part"/><metadata key="sparse_infill_density" value="30%"/></part></object><plate><metadata key="plater_id" value="1"/><metadata key="plater_name" value="First"/><model_instance><metadata key="object_id" value="2"/><metadata key="instance_id" value="0"/></model_instance></plate><plate><metadata key="plater_id" value="7"/><model_instance><metadata key="object_id" value="2"/><metadata key="instance_id" value="1"/></model_instance></plate></config>"#;
    let p = load(
        &model(
            "millimeter",
            &objects,
            r#"<item objectid="2"/><item objectid="2" transform="1 0 0 0 1 0 0 0 1 50 0 0"/>"#,
        ),
        Some(("Metadata/model_settings.config", config)),
    );
    assert_eq!(p.plates.len(), 2);
    let o = &p.objects[0];
    assert_eq!(
        o.instances.iter().map(|i| i.plate).collect::<Vec<_>>(),
        vec![Some(1), Some(7)]
    );
    assert_eq!(o.parts[0].settings["extruder"], "2");
    assert_eq!(o.parts[1].kind, PartKind::NegativeVolume);
    assert_eq!(o.settings["wall_loops"], "4");
    assert_eq!(o.placed_model_mesh().unwrap().faces.len(), 8);
    assert_eq!(p.plates[1].instances[0].instance, 1);
}

#[test]
fn retained_config_and_reported_skipped_features() {
    let root = simple("millimeter").replace("v1=\"0\"", "paint_color=\"8\" v1=\"0\"");
    let text = "; layer_height = 0.1\n; custom_gcode = stuff\n";
    let p = read_3mf(archive(&[
        ("3D/3dmodel.model", &root),
        ("Metadata/Slic3r_PE.config", text),
        ("Metadata/thumbnail.png", ""),
        ("Metadata/Slic3r_PE_layer_heights_profile.txt", ""),
        ("Metadata/custom_gcode_per_layer.xml", ""),
        ("Metadata/cut_information.xml", ""),
    ]))
    .unwrap();
    assert_eq!(p.config.unwrap().text, text);
    assert!(!p.ignored_features.contains("painting"));
    assert_eq!(
        p.objects[0].parts[0].painting.material.painted_triangles(),
        3
    );
    for feature in ["thumbnails/images", "cut information"] {
        assert!(p.ignored_features.contains(feature));
    }
    // Per-layer custom G-code and the layer height profile are read now (this file has neither).
    assert!(!p.ignored_features.contains("custom G-code"));
    assert!(p.custom_gcodes.is_empty());
    assert!(!p.ignored_features.contains("variable layer height"));
    assert!(p.objects[0].layer_height_profile.is_empty());
}

#[test]
fn layer_height_profiles_follow_each_dialects_object_order() {
    let first = simple("millimeter");
    let object = first
        .split("<resources>")
        .nth(1)
        .unwrap()
        .split("</resources>")
        .next()
        .unwrap();
    let model = first
        .replace(
            object,
            &format!("{object}{}", object.replace("id=\"1\"", "id=\"7\"")),
        )
        .replace(
            "<item objectid=\"1\"/>",
            "<item objectid=\"7\"/><item objectid=\"1\"/>",
        );
    let text = "object_id=1|0.000000;0.200000;0.200000;0.200000;1.000000;0.100000\n";
    for (name, id) in [
        ("Metadata/Slic3r_PE_layer_heights_profile.txt", 1),
        ("Metadata/layer_heights_profile.txt", 7),
    ] {
        let project = read_3mf(archive(&[("3D/3dmodel.model", &model), (name, text)])).unwrap();
        let with: Vec<_> = project
            .objects
            .iter()
            .filter(|o| !o.layer_height_profile.is_empty())
            .collect();
        assert_eq!(with.len(), 1);
        assert_eq!(with[0].source.id, id);
        assert_eq!(
            with[0].layer_height_profile,
            vec![0., 0.2, 0.2, 0.2, 1., 0.1]
        );
        assert!(!project.ignored_features.contains("variable layer height"));
    }
    let ears = "brim_points_format_version=0\nobject_id=2|1.000000 2.000000 -0.500100 4.000000\n";
    let project = read_3mf(archive(&[
        ("3D/3dmodel.model", &model),
        ("Metadata/brim_ear_points.txt", ears),
    ]))
    .unwrap();
    assert!(project.objects[0].brim_points.is_empty());
    assert_eq!(project.objects[1].brim_points, vec![[1., 2., -0.5001, 4.]]);
    // Ears are relative to the object's bounding-box centre (the unit tetrahedron's).
    assert_eq!(project.objects[1].brim_frame_origin(), [0.5, 0.5, 0.5]);
}

#[test]
fn invalid_geometry_references_cycles_transforms_and_xml_fail_cleanly() {
    let good = simple("millimeter");
    let cases = [
        good.replace("v1=\"0\"", "v1=\"9\""),
        good.replace("x=\"0\"", "x=\"NaN\""),
        good.replace("<item objectid=\"1\"/>", "<item objectid=\"8\"/>"),
        good.replace(
            "<item objectid=\"1\"/>",
            "<item objectid=\"1\" transform=\"1 2\"/>",
        ),
        good.replace(
            "<item objectid=\"1\"/>",
            "<item objectid=\"1\" transform=\"0 0 0 0 0 0 0 0 0 0 0 0\"/>",
        ),
        good.replace("</model>", ""),
        good.replace("millimeter", "furlong"),
        model(
            "millimeter",
            r#"<object id="1"><components><component objectid="2"/></components></object><object id="2"><components><component objectid="1"/></components></object>"#,
            r#"<item objectid="1"/>"#,
        ),
    ];
    for root in cases {
        assert!(
            read_3mf(archive(&[("3D/3dmodel.model", &root)])).is_err(),
            "accepted {root}"
        );
    }
    assert!(read_3mf(Cursor::new(b"not a ZIP")).is_err());
}

#[test]
fn missing_production_file_and_archive_escape_fail() {
    for path in [
        "/3D/Objects/missing.model",
        "../../outside.model",
        "https://example.com/model",
    ] {
        let root = model(
            "millimeter",
            &format!(
                r#"<object id="1"><components><component objectid="1" p:path="{path}"/></components></object>"#
            ),
            r#"<item objectid="1"/>"#,
        );
        assert!(read_3mf(archive(&[("3D/3dmodel.model", &root)])).is_err());
    }
}

#[test]
fn invalid_ranges_are_rejected() {
    for (first, last) in [(0, 4), (3, 1), (1, 3)] {
        let cfg = format!(
            r#"<config><object id="1"><volume firstid="{first}" lastid="{last}"/></object></config>"#
        );
        assert!(
            read_3mf(archive(&[
                ("3D/3dmodel.model", &simple("millimeter")),
                ("Metadata/Slic3r_PE_model.config", &cfg)
            ]))
            .is_err()
        );
    }
}

#[test]
fn unknown_part_types_are_retained_and_excluded() {
    let cfg = r#"<config><object id="1"><volume firstid="0" lastid="3"><metadata type="volume" key="volume_type" value="future_kind"/></volume></object></config>"#;
    let p = load(
        &simple("millimeter"),
        Some(("Metadata/Slic3r_PE_model.config", cfg)),
    );
    assert_eq!(
        p.objects[0].parts[0].kind,
        PartKind::Unknown("future_kind".into())
    );
    assert!(p.placed_model_mesh().unwrap().faces.is_empty());
}

#[test]
fn prusa_wrapper_resources_become_instances_with_shared_settings() {
    let objects = format!(
        r#"<object id="1">{}</object><object id="2"><components><component objectid="1" transform="1 0 0 0 1 0 0 0 1 3 0 0"/></components></object>"#,
        tetrahedra(1)
    );
    let config = r#"<config><object id="1"><metadata type="object" key="perimeters" value="7"/><volume firstid="0" lastid="3"><metadata type="volume" key="volume_type" value="ModelPart"/></volume></object></config>"#;
    let p = load(
        &model(
            "millimeter",
            &objects,
            r#"<item objectid="1"/><item objectid="2" transform="1 0 0 0 1 0 0 0 1 20 0 0"/>"#,
        ),
        Some(("Metadata/Slic3r_PE_model.config", config)),
    );
    assert_eq!(p.objects.len(), 1);
    assert_eq!(p.objects[0].instances.len(), 2);
    assert_eq!(p.objects[0].settings["perimeters"], "7");
    near(
        f64::from(p.placed_model_mesh().unwrap().bounds().1[0]),
        24.0,
    );
}

#[test]
fn orca_legacy_ranges_and_all_subtypes() {
    for (value, kind) in [
        ("normal_part", PartKind::Model),
        ("negative_part", PartKind::NegativeVolume),
        ("modifier_part", PartKind::Modifier),
        ("support_enforcer", PartKind::SupportEnforcer),
        ("support_blocker", PartKind::SupportBlocker),
    ] {
        let config = format!(
            r#"<config><object id="1"><part firstid="0" lastid="3" subtype="{value}"><metadata key="extruder" value="3"/></part></object></config>"#
        );
        let p = load(
            &simple("millimeter"),
            Some(("Metadata/model_settings.config", &config)),
        );
        assert_eq!(p.objects[0].parts[0].kind, kind);
        assert_eq!(p.objects[0].parts[0].settings["extruder"], "3");
    }
}

#[test]
fn raw_orca_project_config_is_unchanged() {
    let text = "{\n  \"layer_height\": \"0.16\", \"unknown\": [\"a\",\"b\"]\n}\n";
    let p = load(
        &simple("millimeter"),
        Some(("Metadata/project_settings.config", text)),
    );
    assert_eq!(p.config.unwrap().text, text);
    assert!(!p.ignored_features.contains("painting"));
}

#[test]
fn height_range_dialects_and_invalid_bounds() {
    let model = simple("millimeter");
    for name in [
        "Metadata/Prusa_Slicer_layer_config_ranges.xml",
        "Metadata/layer_config_ranges.xml",
    ] {
        let xml = r#"<objects><object id="1"><range min_z="5" max_z="10"><option opt_key="perimeters">4</option><option opt_key="fill_pattern">gyroid</option></range></object></objects>"#;
        let project = read_3mf(archive(&[("3D/3dmodel.model", &model), (name, xml)])).unwrap();
        let r = &project.objects[0].height_ranges[0];
        assert_eq!((r.min_z, r.max_z), (5., 10.));
        assert_eq!(r.settings["perimeters"], "4");
        assert!(!project.ignored_features.contains("height-range settings"));
        for broken in [
            xml.replace("max_z=\"10\"", "max_z=\"4\""),
            xml.replace("min_z=\"5\"", "min_z=\"NaN\""),
            xml.replace("id=\"1\"", "id=\"0\""),
            xml.replace("</objects>", ""),
        ] {
            assert!(read_3mf(archive(&[("3D/3dmodel.model", &model), (name, &broken)])).is_err());
        }
    }
}

#[test]
fn height_range_indices_follow_each_dialects_object_order() {
    let first = simple("millimeter");
    let object = first
        .split("<resources>")
        .nth(1)
        .unwrap()
        .split("</resources>")
        .next()
        .unwrap();
    let model = first
        .replace(
            object,
            &format!("{object}{}", object.replace("id=\"1\"", "id=\"7\"")),
        )
        .replace(
            "<item objectid=\"1\"/>",
            "<item objectid=\"7\"/><item objectid=\"1\"/>",
        );
    let xml = r#"<objects><object id="1"><range min_z="5" max_z="10"><option opt_key="perimeters">4</option></range></object></objects>"#;
    for (name, id) in [
        ("Metadata/Prusa_Slicer_layer_config_ranges.xml", 1),
        ("Metadata/layer_config_ranges.xml", 7),
    ] {
        let project = read_3mf(archive(&[("3D/3dmodel.model", &model), (name, xml)])).unwrap();
        assert_eq!(project.objects[0].source.id, 7);
        let configured = project
            .objects
            .iter()
            .find(|o| !o.height_ranges.is_empty())
            .unwrap();
        assert_eq!(configured.source.id, id);
    }
}
