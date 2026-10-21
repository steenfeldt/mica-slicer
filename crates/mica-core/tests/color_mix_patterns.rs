// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Mixed-colour patterns (Layers / Outer wall first / Gradient), renumbering and the project
//! file round trip. Phase 1's PrusaSlicer cycles stay in `full_spectrum.rs`.

use mica_core::{
    config::Config,
    full_spectrum::{Component, FullSpectrum, Pattern, VirtualExtruder},
    input::{self, PrusaMapper},
    painting::FacetPainting,
    print::LayerSource,
    threemf::{self, Flavour, Instance, Object, Part, PartKind, Project, ResourceId, Transform},
};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};

fn mix(id: usize, ratios: &[f64], pattern: Pattern) -> VirtualExtruder {
    VirtualExtruder {
        id,
        components: ratios
            .iter()
            .enumerate()
            .map(|(i, ratio)| Component {
                extruder: i + 1,
                ratio: *ratio,
            })
            .collect(),
        pattern,
        ..Default::default()
    }
}

fn counts(seq: &[usize]) -> BTreeMap<usize, usize> {
    let mut out = BTreeMap::new();
    for t in seq {
        *out.entry(*t).or_default() += 1;
    }
    out
}

#[test]
fn outer_wall_first_keeps_the_exact_share() {
    // 75/25: Prusa's cycle is A A B A; over 8 layers the walls show B once, the inside 3 times.
    let fs = FullSpectrum::new(vec![mix(6, &[0.75, 0.25], Pattern::OuterWallFirst)], 5).unwrap();
    let (walls, inside) = fs.cycle(6).unwrap();
    assert_eq!(walls, [1, 1, 1, 1, 2, 1, 1, 1]);
    assert_eq!(inside, [1, 2, 1, 1, 2, 1, 2, 1]);
    // 50/50 has no main colour: the walls follow the cycle, the inside the other tool.
    let fs = FullSpectrum::new(vec![mix(6, &[0.5, 0.5], Pattern::OuterWallFirst)], 5).unwrap();
    assert_eq!(fs.cycle(6).unwrap(), (&[1, 2][..], &[2, 1][..]));
    // Every recipe: walls + inside together hold exactly twice the Layers cycle's counts, and
    // the main colour shows on the walls at least as often as in the cycle.
    for ratios in [
        vec![0.75, 0.25],
        vec![0.66, 0.34],
        vec![0.3, 0.7],
        vec![0.9, 0.1],
        vec![0.5, 0.25, 0.25],
        vec![0.6, 0.3, 0.1],
        vec![1. / 3.; 3],
        vec![0.4, 0.4, 0.2],
    ] {
        let layers = FullSpectrum::new(vec![mix(6, &ratios, Pattern::Layers)], 5).unwrap();
        let cycle = layers.cycle(6).unwrap().0.to_vec();
        let fs = FullSpectrum::new(vec![mix(6, &ratios, Pattern::OuterWallFirst)], 5).unwrap();
        let (walls, inside) = fs.cycle(6).unwrap();
        assert_eq!(walls.len(), inside.len());
        let scale = walls.len() / cycle.len();
        assert_eq!(walls.len() % cycle.len(), 0);
        let mut both = walls.to_vec();
        both.extend(inside);
        let expected: BTreeMap<_, _> = counts(&cycle)
            .into_iter()
            .map(|(t, n)| (t, 2 * n * scale))
            .collect();
        assert_eq!(counts(&both), expected, "{ratios:?}");
        let main = *counts(&cycle).iter().max_by_key(|(_, n)| **n).unwrap().0;
        assert!(
            counts(walls)[&main] * cycle.len() >= counts(&cycle)[&main] * walls.len(),
            "{ratios:?}"
        );
    }
}

#[test]
fn gradient_follows_the_ratio_over_the_height() {
    let mut ve = mix(6, &[1., 0.], Pattern::Gradient);
    ve.end_ratios = vec![0., 1.];
    let fs = FullSpectrum::new(vec![ve], 5).unwrap();
    let ve = &fs.extruders()[0];
    let n = 100;
    let seq = ve.gradient_sequence(n);
    assert_eq!(seq.len(), n);
    assert_eq!(seq[0], 1);
    assert_eq!(seq[n - 1], 2);
    // Every prefix is within one layer of the integral of the linear share.
    let mut b = 0.;
    let mut got = 0;
    for (j, t) in seq.iter().enumerate() {
        b += j as f64 / (n - 1) as f64;
        got += usize::from(*t == 2);
        assert!((got as f64 - b).abs() <= 1., "layer {j}: {got} vs {b}");
    }
    // Ten-layer windows show the local share.
    for w in 0..10 {
        let window = &seq[w * 10..w * 10 + 10];
        let share = window.iter().filter(|t| **t == 2).count() as f64 / 10.;
        let mid = (w as f64 * 10. + 4.5) / (n - 1) as f64;
        assert!((share - mid).abs() <= 0.2, "window {w}: {share} vs {mid}");
    }
    // Resolving inside the span uses the gradient; outside it the start-ratio cycle.
    let span = 10..110;
    assert_eq!(fs.roles(6, 10, Some(&span)).walls, 1);
    assert_eq!(fs.roles(6, 109, Some(&span)).walls, 2);
    assert_eq!(fs.resolve(6, 50), 1);
}

#[test]
fn layers_pattern_is_the_prusa_cycle() {
    for (ratios, expected) in [
        (vec![0.5, 0.5], vec![1, 2]),
        (vec![0.75, 0.25], vec![1, 1, 2, 1]),
        (vec![0.66, 0.33], vec![1, 2, 1]),
        (vec![1. / 3.; 3], vec![1, 2, 3]),
    ] {
        let fs = FullSpectrum::new(vec![mix(6, &ratios, Pattern::Layers)], 5).unwrap();
        let (walls, inside) = fs.cycle(6).unwrap();
        assert_eq!(walls, expected);
        assert_eq!(inside, expected);
    }
}

#[test]
fn deleting_a_mix_renumbers_the_later_ones() {
    let fs = FullSpectrum::new(
        vec![
            mix(6, &[0.5, 0.5], Pattern::Layers),
            mix(7, &[0.25, 0.75], Pattern::OuterWallFirst),
            mix(8, &[0.2, 0.3, 0.5], Pattern::Layers),
        ],
        5,
    )
    .unwrap();
    let (rest, map) = fs.without(7, 5).unwrap();
    assert_eq!(
        rest.extruders().iter().map(|v| v.id).collect::<Vec<_>>(),
        [6, 7]
    );
    assert_eq!(rest.extruders()[1].components.len(), 3);
    // The removed mix's parts go to its main colour (tool 2, 75 %); mix 8 becomes 7.
    assert_eq!(map, BTreeMap::from([(7, 2), (8, 7)]));
    // Painted facets follow the renumbering.
    let mut painting = FacetPainting::default();
    let mut codes = vec![];
    mica_core::painting::selector::encode_state(8, &mut codes);
    painting.material.push_codes(0, &codes).unwrap();
    codes.clear();
    mica_core::painting::selector::encode_state(7, &mut codes);
    painting.material.push_codes(3, &codes).unwrap();
    codes.clear();
    mica_core::painting::selector::encode_state(2, &mut codes);
    painting.material.push_codes(5, &codes).unwrap();
    let mapped = painting
        .material
        .map_states(|s| map.get(&(s as usize)).map_or(s, |t| *t as u8));
    assert_eq!(mapped.states(), vec![2, 7]);
    assert_eq!(mapped.painted_triangles(), 3);
    let (swapped, map) = fs.swapped(6, 8, 5).unwrap();
    assert_eq!(map, BTreeMap::from([(6, 8), (8, 6)]));
    assert_eq!(swapped.get(6).unwrap().components.len(), 3);
}

#[test]
fn project_json_round_trips_and_stays_readable_by_prusaslicer() {
    let mut gradient = mix(8, &[0.8, 0.2], Pattern::Gradient);
    gradient.end_ratios = vec![0.2, 0.8];
    gradient.name = Some("Sunset \"fade\"".into());
    let mut owf = mix(7, &[0.25, 0.75], Pattern::OuterWallFirst);
    owf.color = Some("#a05020".into());
    let fs = FullSpectrum::new(
        vec![mix(6, &[0.5, 0.25, 0.25], Pattern::Layers), owf, gradient],
        5,
    )
    .unwrap();
    let physical: Vec<String> = (0..5).map(|i| format!("#00000{i}")).collect();
    let text = fs.to_json(&physical);
    let back = FullSpectrum::from_json(&text, 5).unwrap();
    assert_eq!(back, fs);
    assert_eq!(back.to_json(&physical), text);
    // PrusaSlicer's reader: version 1, kind fullspectrum, no `position` keys (which would
    // make an entry a Prusa gradient), and the start ratios; it ignores the `mica` object.
    assert!(text.contains("\"version\":1"));
    assert!(!text.contains("position"));
    assert!(!text.contains("\"gradient\",\"minz\""));
    assert_eq!(text.matches("\"kind\":\"fullspectrum\"").count(), 3);
    assert!(text.contains("\"physical_extruders\":[{\"color\":\"#000000\",\"id\":1}"));
    // Without the Mica fields every mix is a plain layer mix with the same start ratios.
    let mut prusa = String::new();
    let mut depth = 0;
    let mut skip = false;
    for (i, c) in text.char_indices() {
        if !skip && text[i..].starts_with(",\"mica\":{") {
            skip = true;
            depth = 0;
        }
        if skip {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        skip = false;
                    }
                }
                _ => {}
            }
            continue;
        }
        prusa.push(c);
    }
    let plain = FullSpectrum::from_json(&prusa, 5).unwrap();
    for (a, b) in plain.extruders().iter().zip(fs.extruders()) {
        assert_eq!(a.pattern, Pattern::Layers);
        assert_eq!(a.components, b.components);
        assert_eq!(a.name, None);
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

fn project(fs: &FullSpectrum, id: usize, painted: bool) -> Project {
    let source = ResourceId {
        path: "3D/3dmodel.model".into(),
        id: 1,
    };
    let mut painting = FacetPainting::default();
    let mut settings = BTreeMap::new();
    if painted {
        let mut codes = vec![];
        mica_core::painting::selector::encode_state(id as u8, &mut codes);
        for i in 0..12 {
            painting.material.push_codes(i, &codes).unwrap();
        }
    } else {
        settings.insert("extruder".to_string(), id.to_string());
    }
    Project {
        full_spectrum_json: Some(fs.to_json(&[])),
        objects: vec![Object {
            name: "pattern cube".into(),
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
                settings,
                metadata: BTreeMap::new(),
            }],
            instances: vec![Instance {
                transform: Transform::IDENTITY,
                printable: true,
                plate: None,
            }],
            settings: BTreeMap::new(),
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
            "mica-colormix-pattern-{}-{}.3mf",
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

/// `(walls, inside)` physical tools (one-based) of each layer of the sliced cube.
fn layer_tools(p: &Project) -> Vec<(usize, usize)> {
    let tmp = Temp::new();
    threemf::write(&tmp.0, p, Flavour::Prusa).unwrap();
    let loaded = input::load(
        std::slice::from_ref(&tmp.0),
        (0., 0.),
        false,
        true,
        0,
        &BTreeMap::new(),
        &config(),
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
    (0..object.prepared.layer_count())
        .map(|i| {
            let regions = object.prepared.layer_regions(i).unwrap();
            assert_eq!(regions.len(), 1, "layer {i}");
            let tools = &object.prepared.region(regions[0].region).unwrap().tools;
            (tools.0[0].unwrap() + 1, tools.0[1].unwrap() + 1)
        })
        .collect()
}

#[test]
fn sliced_regions_follow_each_pattern() {
    for painted in [false, true] {
        let fs =
            FullSpectrum::new(vec![mix(6, &[0.75, 0.25], Pattern::OuterWallFirst)], 5).unwrap();
        let (walls, inside) = fs.cycle(6).unwrap();
        let tools = layer_tools(&project(&fs, 6, painted));
        assert_eq!(tools.len(), 20);
        for (i, t) in tools.iter().enumerate() {
            assert_eq!(
                *t,
                (walls[i % 8], inside[i % 8]),
                "layer {i} painted={painted}"
            );
        }

        let mut ve = mix(6, &[0.9, 0.1], Pattern::Gradient);
        ve.end_ratios = vec![0.1, 0.9];
        let fs = FullSpectrum::new(vec![ve], 5).unwrap();
        let expected = fs.extruders()[0].gradient_sequence(20);
        let tools = layer_tools(&project(&fs, 6, painted));
        assert_eq!(
            tools.iter().map(|t| t.0).collect::<Vec<_>>(),
            expected,
            "painted={painted}"
        );
        assert!(tools.iter().all(|t| t.0 == t.1));
        assert_eq!(tools[0].0, 1);
        assert_eq!(tools[19].0, 2);

        let fs = FullSpectrum::new(vec![mix(6, &[0.75, 0.25], Pattern::Layers)], 5).unwrap();
        let tools = layer_tools(&project(&fs, 6, painted));
        for (i, t) in tools.iter().enumerate() {
            let e = [1, 1, 2, 1][i % 4];
            assert_eq!(*t, (e, e), "layer {i}");
        }
    }
}
