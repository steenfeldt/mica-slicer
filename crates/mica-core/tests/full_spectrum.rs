// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

use mica_core::{
    config::Config,
    full_spectrum::{Component, FullSpectrum, VirtualExtruder},
    input::{self, PrusaMapper},
    painting::FacetPainting,
    print::LayerSource,
    print_job::ToolAssignments,
    threemf::{self, Flavour, Instance, Object, Part, PartKind, Project, ResourceId, Transform},
};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};

fn recipe(id: usize, ratios: &[f64]) -> VirtualExtruder {
    VirtualExtruder {
        id,
        color: None,
        components: ratios
            .iter()
            .enumerate()
            .map(|(i, ratio)| Component {
                extruder: i + 1,
                ratio: *ratio,
            })
            .collect(),
        ..Default::default()
    }
}
fn config() -> Config {
    let mut c = Config::defaults();
    for (key, value) in [
        ("nozzle_diameter", "0.4,0.4,0.4,0.4,0.4"),
        ("layer_height", "0.2"),
        ("first_layer_height", "0.2"),
        ("skirts", "0"),
        ("wipe_tower", "0"),
        ("brim_width", "0"),
        ("elefant_foot_compensation", "0"),
        ("mmu_segmented_region_max_width", "0"),
    ] {
        c.set_raw(key, value).unwrap();
    }
    c
}
fn json(id: usize) -> String {
    format!(
        "{{\n  \"version\":1, \"future\": {{\"preserve\":true}}, \"physical_extruders\":[],\n  \"virtual_extruders\":[{{\"id\":{id},\"components\":[{{\"extruder\":1,\"ratio\":0.75}},{{\"extruder\":2,\"ratio\":0.25}}]}}]}}\n"
    )
}
fn project(id: usize, painted: bool, object_assignment: bool) -> Project {
    let source = ResourceId {
        path: "3D/3dmodel.model".into(),
        id: 1,
    };
    let settings = BTreeMap::from([("extruder".into(), id.to_string())]);
    let mut painting = FacetPainting::default();
    if painted {
        let code = if id == 17 { "00EC" } else { "3C" };
        for i in 0..12 {
            painting.material.push(i, code).unwrap();
        }
        assert_eq!(painting.material.states(), vec![id as u8]);
    }
    Project {
        full_spectrum_json: Some(json(id)),
        objects: vec![Object {
            name: "generated mix cube".into(),
            source: source.clone(),
            parts: vec![Part {
                name: "part".into(),
                kind: PartKind::Model,
                source,
                mesh: {
                    let mut mesh = mica_core::mesh_edit::cuboid([10., 10., 4.]);
                    for p in &mut mesh.vertices {
                        p[2] += 2.;
                    }
                    mesh
                },
                source_mesh: None,
                painting,
                transform: Transform::IDENTITY,
                settings: if !painted && !object_assignment {
                    settings.clone()
                } else {
                    BTreeMap::new()
                },
                metadata: BTreeMap::new(),
            }],
            instances: vec![Instance {
                transform: Transform::IDENTITY,
                printable: true,
                plate: None,
            }],
            settings: if !painted && object_assignment {
                settings
            } else {
                BTreeMap::new()
            },
            metadata: BTreeMap::new(),
            height_ranges: vec![],
            layer_height_profile: vec![],
            brim_points: vec![],
        }],
        ..Project::default()
    }
}
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        Self(std::env::temp_dir().join(format!(
            "mica-colormix-test-{}-{}.3mf",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )))
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[test]
fn upstream_cycles_and_object_local_phase() {
    for (ratios, expected) in [
        (vec![0.5, 0.5], vec![1, 2]),
        (vec![0.75, 0.25], vec![1, 1, 2, 1]),
        (vec![0.66, 0.33], vec![1, 2, 1]),
        (vec![1. / 3.; 3], vec![1, 2, 3]),
        (vec![0., 1.], vec![2]),
        (vec![0.3, 0.7], vec![2, 1, 2, 2, 2, 1, 2]),
    ] {
        let fs = FullSpectrum::new(vec![recipe(6, &ratios)], 5).unwrap();
        assert_eq!(fs.extruders()[0].build_sequence(), expected);
        for i in 0..100 {
            assert_eq!(fs.resolve(6, i), expected[i % expected.len()]);
        }
    }
}

#[test]
fn undefined_ids_and_bad_recipes_are_errors() {
    let c = config();
    let settings = BTreeMap::from([("extruder".into(), "6".into())]);
    assert!(
        ToolAssignments::resolve(&c, &settings, &BTreeMap::new(), 0, 5)
            .unwrap_err()
            .contains("tool 6")
    );
    let fs = FullSpectrum::from_json(&json(6), 5).unwrap();
    assert_eq!(
        ToolAssignments::resolve_with_virtuals(&c, &settings, &BTreeMap::new(), 0, 5, &fs)
            .unwrap()
            .0[..3],
        [Some(5); 3]
    );
    let gap = BTreeMap::from([("extruder".into(), "7".into())]);
    assert!(ToolAssignments::resolve_with_virtuals(&c, &gap, &BTreeMap::new(), 0, 5, &fs).is_err());
    for ve in [
        recipe(5, &[0.5, 0.5]),
        recipe(256, &[0.5, 0.5]),
        recipe(6, &[1.]),
        recipe(6, &[0.25; 4]),
    ] {
        assert!(FullSpectrum::new(vec![ve], 5).is_err());
    }
    assert!(FullSpectrum::new(vec![recipe(6, &[0.5, 0.5]), recipe(6, &[0.5, 0.5])], 5).is_err());
    assert!(
        FullSpectrum::from_json(&json(6).replace("\"extruder\":2", "\"extruder\":7"), 5).is_err()
    );
    assert!(
        FullSpectrum::from_json(&json(6).replace("\"version\":1", "\"version\":2"), 5).is_err()
    );
}

#[test]
fn metadata_and_assignments_survive_both_dialects_byte_for_byte() {
    for painted in [false, true] {
        let p = project(17, painted, false);
        for flavour in [Flavour::Prusa, Flavour::Orca] {
            let tmp = Temp::new();
            threemf::write(&tmp.0, &p, flavour).unwrap();
            let loaded = threemf::load_3mf(&tmp.0).unwrap();
            assert_eq!(loaded.full_spectrum_json, p.full_spectrum_json);
            let mut zip = zip::ZipArchive::new(std::fs::File::open(&tmp.0).unwrap()).unwrap();
            let mut text = String::new();
            std::io::Read::read_to_string(
                &mut zip.by_name(mica_core::full_spectrum::ARCHIVE_PATH).unwrap(),
                &mut text,
            )
            .unwrap();
            assert_eq!(Some(text), p.full_spectrum_json);
            assert_eq!(
                loaded.objects[0].parts[0].settings,
                p.objects[0].parts[0].settings
            );
            assert_eq!(
                loaded.objects[0].parts[0].painting,
                p.objects[0].parts[0].painting
            );
        }
    }
}

#[test]
fn volumes_objects_and_extended_paint_only_generate_physical_regions() {
    let c = config();
    for (id, painted, object_assignment) in [
        (6, false, false),
        (6, false, true),
        (6, true, false),
        (17, true, false),
    ] {
        let p = project(id, painted, object_assignment);
        let tmp = Temp::new();
        threemf::write(&tmp.0, &p, Flavour::Prusa).unwrap();
        let loaded = input::load(
            std::slice::from_ref(&tmp.0),
            (0., 0.),
            false,
            true,
            0,
            &BTreeMap::new(),
            &c,
            &PrusaMapper,
        )
        .unwrap();
        let object = loaded
            .objects
            .into_iter()
            .next()
            .unwrap()
            .prepare()
            .unwrap();
        assert_eq!(object.prepared.layer_count(), 20);
        for i in 0..20 {
            let expected = [0, 0, 1, 0][i % 4];
            let regions = object.prepared.layer_regions(i).unwrap();
            assert!(!regions.is_empty());
            for layer_region in regions {
                let region = object.prepared.region(layer_region.region).unwrap();
                assert_eq!(
                    region.tools.0[..3],
                    [Some(expected); 3],
                    "layer {i}, painted={painted}"
                );
            }
        }
        // Validate the region table as a whole, not only the currently active slices.
        for i in 0..object.prepared.region_count() {
            let r = object.prepared.region(i).unwrap();
            assert!(r.tools.0.iter().flatten().all(|t| *t < 5));
            assert!(r.config.int("perimeter_extruder").unwrap() <= 5);
        }
    }
}

#[test]
fn missing_metadata_rejects_virtual_volume_and_paint() {
    let c = config();
    for painted in [false, true] {
        let mut p = project(6, painted, false);
        p.full_spectrum_json = None;
        let tmp = Temp::new();
        threemf::write(&tmp.0, &p, Flavour::Prusa).unwrap();
        let loaded = input::load(
            std::slice::from_ref(&tmp.0),
            (0., 0.),
            false,
            true,
            0,
            &BTreeMap::new(),
            &c,
            &PrusaMapper,
        );
        let error = match loaded {
            Err(e) => e,
            Ok(loaded) => loaded
                .objects
                .into_iter()
                .next()
                .unwrap()
                .prepare()
                .err()
                .expect("undefined painted tool must fail"),
        };
        assert!(error.contains("tool 6"), "{error}");
    }
}
