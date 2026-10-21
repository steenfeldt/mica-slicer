// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

use mica_core::{painting::PaintKind, threemf::*};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};
fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/models")
        .join(name)
}
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static N: AtomicUsize = AtomicUsize::new(0);
        let p = std::env::temp_dir().join(format!(
            "mica-write-test-{}-{}.3mf",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        Self(p)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
fn triangles(p: &Part) -> Vec<[[f32; 3]; 3]> {
    p.mesh
        .faces
        .iter()
        .map(|f| f.map(|i| p.mesh.vertices[i as usize]))
        .collect()
}
fn same(a: &Project, b: &Project) {
    assert_eq!(format!("{:?}", a.config), format!("{:?}", b.config));
    assert_eq!(format!("{:?}", a.plates), format!("{:?}", b.plates));
    assert_eq!(a.wipe_towers, b.wipe_towers);
    assert_eq!(a.custom_gcodes, b.custom_gcodes);
    assert_eq!(a.ignored_features, b.ignored_features);
    assert_eq!(a.objects.len(), b.objects.len());
    for (a, b) in a.objects.iter().zip(&b.objects) {
        assert_eq!(a.name, b.name);
        assert_eq!(a.source, b.source);
        assert_eq!(a.settings, b.settings);
        assert_eq!(a.metadata, b.metadata);
        assert_eq!(format!("{:?}", a.instances), format!("{:?}", b.instances));
        assert_eq!(
            format!("{:?}", a.height_ranges),
            format!("{:?}", b.height_ranges)
        );
        assert_eq!(a.parts.len(), b.parts.len());
        for (a, b) in a.parts.iter().zip(&b.parts) {
            assert_eq!(a.name, b.name);
            assert_eq!(a.kind, b.kind);
            assert_eq!(a.source, b.source);
            assert_eq!(a.settings, b.settings);
            assert_eq!(a.metadata, b.metadata);
            assert_eq!(a.transform, b.transform);
            assert_eq!(a.painting, b.painting);
            assert_eq!(triangles(a), triangles(b));
        }
    }
}
fn roundtrip(p: &Project) {
    for flavour in [Flavour::Orca, Flavour::Prusa] {
        let temp = Temp::new();
        write(&temp.0, p, flavour).unwrap();
        let again = load_3mf(&temp.0).unwrap();
        assert_eq!(
            format!("{:?}", load_config(&temp.0).unwrap()),
            format!("{:?}", again.config)
        );
        same(p, &again);
        write(&temp.0, &again, flavour).unwrap();
        same(p, &load_3mf(&temp.0).unwrap());
    }
}
#[test]
fn repository_fixtures_roundtrip() {
    for name in ["plates-orca.3mf", "painted-cube.3mf"] {
        roundtrip(&load_3mf(fixture(name)).unwrap());
    }
}

#[test]
fn orca_resource_ids_are_unique_across_model_files() {
    use quick_xml::{Reader, events::Event};
    let p = load_3mf(fixture("plates-orca.3mf")).unwrap();
    let temp = Temp::new();
    write(&temp.0, &p, Flavour::Orca).unwrap();
    let mut z = zip::ZipArchive::new(std::fs::File::open(&temp.0).unwrap()).unwrap();
    let mut ids = std::collections::BTreeSet::new();
    for i in 0..z.len() {
        let f = z.by_index(i).unwrap();
        if !f.name().ends_with(".model") {
            continue;
        }
        let mut r = Reader::from_reader(std::io::BufReader::new(f));
        let mut buf = Vec::new();
        loop {
            match r.read_event_into(&mut buf).unwrap() {
                Event::Start(e) | Event::Empty(e) if e.local_name().as_ref() == b"object" => {
                    let id = e
                        .attributes()
                        .map(Result::unwrap)
                        .find(|a| a.key.as_ref() == b"id")
                        .unwrap()
                        .value
                        .into_owned();
                    assert!(
                        ids.insert(id),
                        "Orca shared meshes require globally unique resource IDs"
                    );
                }
                Event::Eof => break,
                _ => {}
            }
            buf.clear();
        }
    }
    assert_eq!(ids.len(), 8);
}
fn complex() -> Project {
    let mut p = load_3mf(fixture("painted-cube.3mf")).unwrap();
    p.config=Some(RawConfig{path:"Metadata/Slic3r_PE.config".into(),text:"; layer_height = 0.2\n; perimeters = 3\n; filament_colour = #12AB34;#AB1234\n; nozzle_diameter = 0.4,0.4\n; start_gcode = G28\\nM117 \"ready & go\"\n".into()});
    let o = &mut p.objects[0];
    o.name = "Ω <&\"\n😀".into();
    o.settings.insert("perimeters".into(), "4".into());
    o.height_ranges.push(HeightRange {
        min_z: 1.,
        max_z: 2.,
        settings: Settings::from([
            ("fill_density".into(), "75%".into()),
            ("layer_height".into(), "0.1".into()),
        ]),
    });
    for kind in [
        PartKind::Modifier,
        PartKind::NegativeVolume,
        PartKind::SupportEnforcer,
        PartKind::SupportBlocker,
        PartKind::Unknown("future volume".into()),
    ] {
        let mut part = o.parts[0].clone();
        part.kind = kind;
        part.painting = Default::default();
        part.settings = Settings::from([("fill_density".into(), "25%".into())]);
        part.mesh.translate([2., 3., 4.]);
        part.transform.0[0][3] = 2.;
        o.parts.push(part);
    }
    // Recursive split trees, including different split sides, extended states and mirrored winding.
    for k in PaintKind::ALL {
        let s = o.parts[0].painting.get_mut(k);
        *s = Default::default();
        for (i, codes) in ["489", "844A", "48483", "0C", "1C", "8", "4"]
            .into_iter()
            .enumerate()
        {
            s.push(i as u32, codes).unwrap();
        }
    }
    o.parts[0].painting.reverse_winding();
    let mut other = o.instances[0].clone();
    other.transform = Transform([[-1., 0., 0., 50.], [0., 2., 0., 40.], [0., 0., 0.5, 0.]]);
    other.printable = false;
    o.instances.push(other);
    o.instances[0].plate = Some(7);
    o.instances[1].plate = Some(11);
    p.plates = vec![
        Plate {
            id: 7,
            name: "First".into(),
            instances: vec![PlateInstance {
                object: 0,
                instance: 0,
            }],
            metadata: Settings::new(),
        },
        Plate {
            id: 11,
            name: "Locked".into(),
            instances: vec![PlateInstance {
                object: 0,
                instance: 1,
            }],
            metadata: Settings::from([("locked".into(), "true".into())]),
        },
    ];
    p.wipe_towers.insert(0, [10., 20., 30.]);
    p.wipe_towers.insert(1, [20., 30., 40.]);
    p
}
#[test]
fn parts_ranges_instances_and_all_painting_roundtrip() {
    roundtrip(&complex());
    let mut p = complex();
    let seam = &mut p.objects[0].parts[0].painting.seam;
    *seam = std::mem::take(seam).with_reversed(false);
    roundtrip(&p);
    for part in &mut p.objects[0].parts {
        for face in &mut part.mesh.faces {
            face.swap(1, 2);
        }
    }
    roundtrip(&p);
}
#[test]
fn per_layer_custom_gcode_roundtrips_in_both_dialects() {
    use mica_core::custom_gcode::{Info, Item, Kind, Mode};
    let mut p = complex();
    let mut first = Info {
        mode: Mode::SingleExtruder,
        ..Info::default()
    };
    first.put(Item {
        color: "#E67E22".into(),
        ..Item::new(5.0, Kind::ColorChange, 1)
    });
    first.put(Item {
        extra: "Insert <magnets> & \"nuts\"".into(),
        ..Item::new(10.0, Kind::PausePrint, 1)
    });
    first.put(Item {
        extra: "M117 one\nM117 two".into(),
        ..Item::new(12.2, Kind::Custom, 1)
    });
    let mut third = Info {
        mode: Mode::MultiAsSingle,
        ..Info::default()
    };
    third.put(Item {
        color: "#00FF00".into(),
        ..Item::new(0.6, Kind::ToolChange, 2)
    });
    p.custom_gcodes.insert(0, first);
    p.custom_gcodes.insert(2, third);
    roundtrip(&p);
    for (flavour, file, other) in [
        (
            Flavour::Orca,
            "Metadata/custom_gcode_per_layer.xml",
            "Metadata/Prusa_Slicer_custom_gcode_per_print_z.xml",
        ),
        (
            Flavour::Prusa,
            "Metadata/Prusa_Slicer_custom_gcode_per_print_z.xml",
            "Metadata/custom_gcode_per_layer.xml",
        ),
    ] {
        let temp = Temp::new();
        write(&temp.0, &p, flavour).unwrap();
        let mut z = zip::ZipArchive::new(std::fs::File::open(&temp.0).unwrap()).unwrap();
        let mut text = String::new();
        z.by_name(file).unwrap().read_to_string(&mut text).unwrap();
        assert!(text.contains("extra=\"M117 one&#10;M117 two\""), "{text}");
        assert!(z.by_name(other).is_err());
        // Read without Mica's sidecar, as another slicer would leave it.
        rewrite(&temp.0, |name, _| name != "Metadata/mica.json");
        let again = load_3mf(&temp.0).unwrap();
        assert_eq!(again.custom_gcodes, p.custom_gcodes);
        assert!(
            !again
                .ignored_features
                .iter()
                .any(|f| f.contains("custom G-code"))
        );
    }
}
/// Plate metadata keys starting with `mica_` (a calibration plate's job) are Mica's own: they
/// round-trip through Metadata/mica.json and never reach the native files other slicers read.
#[test]
fn mica_plate_metadata_stays_in_the_sidecar() {
    let mut p = complex();
    let job = "{\"test\":\"pa-tower\",\"note\":\"<&>\"}";
    p.plates[1]
        .metadata
        .insert("mica_calibration".into(), job.into());
    for flavour in [Flavour::Orca, Flavour::Prusa] {
        let temp = Temp::new();
        write(&temp.0, &p, flavour).unwrap();
        let mut z = zip::ZipArchive::new(std::fs::File::open(&temp.0).unwrap()).unwrap();
        let mut sidecar = false;
        for i in 0..z.len() {
            let mut f = z.by_index(i).unwrap();
            let mut data = Vec::new();
            f.read_to_end(&mut data).unwrap();
            let text = String::from_utf8_lossy(&data);
            if f.name() == "Metadata/mica.json" {
                sidecar = text.contains("mica_calibration");
            } else {
                assert!(
                    !text.contains("mica_calibration"),
                    "{flavour:?} {}",
                    f.name()
                );
            }
        }
        assert!(sidecar, "{flavour:?}");
        let again = load_3mf(&temp.0).unwrap();
        assert_eq!(
            again.plates[1]
                .metadata
                .get("mica_calibration")
                .map(String::as_str),
            Some(job)
        );
        assert_eq!(
            again.plates[1].metadata.get("locked").map(String::as_str),
            Some("true")
        );
        // Without the sidecar (a file re-saved by another slicer) the key is simply gone.
        rewrite(&temp.0, |name, _| name != "Metadata/mica.json");
        let native = load_3mf(&temp.0).unwrap();
        assert!(
            native
                .plates
                .iter()
                .all(|p| !p.metadata.contains_key("mica_calibration"))
        );
    }
}
fn rewrite(path: &Path, mut change: impl FnMut(&str, &mut Vec<u8>) -> bool) {
    let mut z = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
    let mut out = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for i in 0..z.len() {
        let mut f = z.by_index(i).unwrap();
        let mut data = Vec::new();
        f.read_to_end(&mut data).unwrap();
        if change(f.name(), &mut data) {
            out.start_file(f.name(), zip::write::SimpleFileOptions::default())
                .unwrap();
            out.write_all(&data).unwrap();
        }
    }
    std::fs::write(path, out.finish().unwrap().into_inner()).unwrap();
}
#[test]
fn native_content_works_without_marker_and_stale_marker_does_not_override_edits() {
    for flavour in [Flavour::Orca, Flavour::Prusa] {
        let p = complex();
        let temp = Temp::new();
        write(&temp.0, &p, flavour).unwrap();
        rewrite(&temp.0, |name, _| name != "Metadata/mica.json");
        let native = load_3mf(&temp.0).unwrap();
        assert_eq!(native.objects[0].parts.len(), 6);
        assert_eq!(native.objects[0].instances.len(), 2);
        assert_eq!(native.objects[0].height_ranges.len(), 1);
        for (a, b) in p.objects[0].parts.iter().zip(&native.objects[0].parts) {
            assert_eq!(a.kind, b.kind);
            assert_eq!(triangles(a), triangles(b));
            // Texture paint is Mica's own and lives only in the sidecar this test removed.
            assert!(b.painting.texture.is_empty());
            for k in PaintKind::ALL
                .into_iter()
                .filter(|&k| k != PaintKind::Texture)
            {
                let mut aa = vec![];
                let mut bb = vec![];
                a.painting
                    .get(k)
                    .visit(&a.mesh, false, |s, t| aa.push((s, t)));
                b.painting
                    .get(k)
                    .visit(&b.mesh, false, |s, t| bb.push((s, t)));
                let canonical = |leaves: Vec<(u8, [[f64; 3]; 3])>| {
                    let mut out: Vec<_> = leaves
                        .into_iter()
                        .map(|(state, t)| {
                            let mut t = t.map(|p| p.map(f64::to_bits));
                            let first = (0..3).min_by_key(|&i| t[i]).unwrap();
                            t.rotate_left(first);
                            (state, t)
                        })
                        .collect();
                    out.sort();
                    out
                };
                assert_eq!(canonical(aa), canonical(bb));
            }
        }
        write(&temp.0, &p, flavour).unwrap();
        rewrite(&temp.0, |name, data| {
            if name.ends_with("model_settings.config") || name.ends_with("Slic3r_PE_model.config") {
                *data = String::from_utf8(data.clone())
                    .unwrap()
                    .replace("value=\"4\"", "value=\"5\"")
                    .into_bytes();
            }
            true
        });
        let changed = load_3mf(&temp.0).unwrap();
        assert!(
            changed
                .ignored_features
                .iter()
                .any(|s| s.starts_with("stale Mica"))
        );
        assert_eq!(
            changed.objects[0]
                .settings
                .get(if flavour == Flavour::Orca {
                    "wall_loops"
                } else {
                    "perimeters"
                })
                .unwrap(),
            "5"
        );
    }
}
#[test]
fn invalid_project_preserves_existing_destination() {
    let temp = Temp::new();
    std::fs::write(&temp.0, b"keep me").unwrap();
    let mut p = complex();
    p.objects[0].parts[0].mesh.vertices[0][0] = f32::NAN;
    assert!(write(&temp.0, &p, Flavour::Orca).is_err());
    assert_eq!(std::fs::read(&temp.0).unwrap(), b"keep me");
    let mut p = complex();
    p.plates[0].instances[0].instance = 99;
    assert!(write(&temp.0, &p, Flavour::Prusa).is_err());
    let mut p = complex();
    let range = p.objects[0].height_ranges[0].clone();
    p.objects[0].height_ranges.push(range);
    assert!(write(&temp.0, &p, Flavour::Prusa).is_err());
    let mut p = complex();
    p.config = Some(RawConfig {
        path: "Metadata/project_settings.config".into(),
        text: "{broken".into(),
    });
    // This fails while writing settings, after the mesh entries have been written.
    assert!(write(&temp.0, &p, Flavour::Orca).is_err());
    assert_eq!(std::fs::read(&temp.0).unwrap(), b"keep me");
}

#[test]
fn orca_source_vertices_and_component_build_matrices_keep_all_bits() {
    for (name, mirrored) in [("plates-orca.3mf", false), ("painted-cube.3mf", true)] {
        let mut p = load_3mf(fixture(name)).unwrap();
        // Values deliberately not representable as f32; the cat's 6.5x component scale
        // must remain a matrix rather than being baked into the archive's mesh.
        for object in &mut p.objects {
            for part in &mut object.parts {
                let source = std::sync::Arc::make_mut(part.source_mesh.as_mut().unwrap());
                for vertex in &mut source.vertices {
                    vertex[0] += 0.123456789012345;
                    vertex[1] -= 0.234567890123456;
                }
                source.transform.0[0][0] *= if mirrored { -6.5 } else { 6.5 };
                source.transform.0[1][1] *= 6.5;
                source.transform.0[2][2] *= 6.5;
                part.mesh.faces = source.faces.clone();
                if source.transform.determinant() < 0. {
                    for face in &mut part.mesh.faces {
                        face.swap(1, 2);
                    }
                    part.painting.reverse_winding();
                    // New painting can have independent winding flags even though the
                    // retained source component still reflects its mesh. Native wire
                    // reorientation and exact-painting backup must use the same frame.
                    for (i, kind) in PaintKind::ALL.into_iter().enumerate() {
                        let states = part.painting.get_mut(kind);
                        *states =
                            mica_core::painting::FacetStates::default().with_reversed(i % 2 == 0);
                        states.push(0, "489").unwrap();
                    }
                }
                part.transform = source.transform;
                part.mesh.vertices = source
                    .vertices
                    .iter()
                    .map(|v| source.transform.apply(v.map(|x| x as f32)))
                    .collect();
            }
            object.instances[0].transform.0[0][3] += 0.123456789012345;
        }
        let temp = Temp::new();
        for _ in 0..3 {
            write(&temp.0, &p, Flavour::Orca).unwrap();
            let mut zip = zip::ZipArchive::new(std::fs::File::open(&temp.0).unwrap()).unwrap();
            let mut xml = String::new();
            zip.by_name("3D/3dmodel.model")
                .unwrap()
                .read_to_string(&mut xml)
                .unwrap();
            // Orca's native parser splits on literal spaces without trimming; a leading
            // newline/space makes 13 tokens and silently changes the matrix to identity.
            for matrix in xml.split("transform=\"").skip(1) {
                let matrix = matrix.split('"').next().unwrap();
                assert_eq!(matrix.trim(), matrix);
                assert_eq!(matrix.split(' ').count(), 12);
            }
            // Native data, without Mica's metadata, must also retain the exact matrices.
            let native = Temp::new();
            std::fs::copy(&temp.0, &native.0).unwrap();
            rewrite(&native.0, |name, _| name != "Metadata/mica.json");
            let native = load_3mf(&native.0).unwrap();
            for (a, b) in p.objects.iter().zip(&native.objects) {
                assert_eq!(a.instances[0].transform, b.instances[0].transform);
                for (a, b) in a.parts.iter().zip(&b.parts) {
                    assert_eq!(
                        a.source_mesh.as_ref().unwrap().transform,
                        b.source_mesh.as_ref().unwrap().transform
                    );
                }
            }
            let again = load_3mf(&temp.0).unwrap();
            for (a, b) in p.objects.iter().zip(&again.objects) {
                assert_eq!(a.instances[0].transform, b.instances[0].transform);
                for (a, b) in a.parts.iter().zip(&b.parts) {
                    let (a, b) = (
                        a.source_mesh.as_ref().unwrap(),
                        b.source_mesh.as_ref().unwrap(),
                    );
                    assert_eq!(a.transform, b.transform);
                    assert_eq!(
                        a.vertices
                            .iter()
                            .map(|v| v.map(f64::to_bits))
                            .collect::<Vec<_>>(),
                        b.vertices
                            .iter()
                            .map(|v| v.map(f64::to_bits))
                            .collect::<Vec<_>>()
                    );
                }
            }
            same(&p, &again);
            p = again;
        }
    }
}

#[test]
fn edited_mesh_invalidates_retained_source() {
    let mut p = load_3mf(fixture("painted-cube.3mf")).unwrap();
    p.objects[0].parts[0].mesh.vertices[0][0] += 0.5;
    assert!(
        !p.objects[0].parts[0]
            .source_mesh
            .as_ref()
            .unwrap()
            .matches(&p.objects[0].parts[0].mesh)
    );
    let temp = Temp::new();
    write(&temp.0, &p, Flavour::Orca).unwrap();
    same(&p, &load_3mf(&temp.0).unwrap());
}

/// The layer height profile round-trips in both dialects (in each dialect's file); brim ears
/// only in Orca's (PrusaSlicer has none).
#[test]
fn layer_height_profile_and_brim_ears_roundtrip() {
    let mut p = complex();
    let profile = vec![0., 0.2, 0.2, 0.2, 1.5, 0.1, 3., 0.25];
    p.objects[0].layer_height_profile = profile.clone();
    p.objects[0].brim_points = vec![[1.25, -2.5, -0.0001, 4.], [3., 4., -0.5, 2.5]];
    for (flavour, file, ears) in [
        (Flavour::Orca, "Metadata/layer_heights_profile.txt", true),
        (
            Flavour::Prusa,
            "Metadata/Slic3r_PE_layer_heights_profile.txt",
            false,
        ),
    ] {
        let temp = Temp::new();
        write(&temp.0, &p, flavour).unwrap();
        let mut zip = zip::ZipArchive::new(std::fs::File::open(&temp.0).unwrap()).unwrap();
        let names: Vec<String> = zip.file_names().map(str::to_owned).collect();
        assert!(names.iter().any(|n| n == file), "{names:?}");
        assert_eq!(
            names.iter().any(|n| n == "Metadata/brim_ear_points.txt"),
            ears
        );
        let mut text = String::new();
        zip.by_name(file)
            .unwrap()
            .read_to_string(&mut text)
            .unwrap();
        assert!(text.starts_with("object_id=1|0.000000;0.200000;"), "{text}");
        let again = load_3mf(&temp.0).unwrap();
        assert_eq!(again.objects[0].layer_height_profile, profile);
        if ears {
            assert_eq!(again.objects[0].brim_points, p.objects[0].brim_points);
        } else {
            assert!(again.objects[0].brim_points.is_empty());
        }
    }
}
