// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

use super::*;
use crate::config::Config;
use crate::settings::PrintSettings;

/// A closed box from `min` to `max`, outward facing, as 12 triangles.
fn cube(min: [f32; 3], max: [f32; 3]) -> TriangleMesh {
    let v = |i: usize| {
        [
            if i & 1 == 0 { min[0] } else { max[0] },
            if i & 2 == 0 { min[1] } else { max[1] },
            if i & 4 == 0 { min[2] } else { max[2] },
        ]
    };
    TriangleMesh {
        vertices: (0..8).map(v).collect(),
        faces: vec![
            [0, 2, 1],
            [1, 2, 3],
            [4, 5, 6],
            [5, 7, 6],
            [0, 1, 4],
            [1, 5, 4],
            [2, 6, 3],
            [3, 6, 7],
            [0, 4, 2],
            [2, 4, 6],
            [1, 3, 5],
            [3, 7, 5],
        ],
    }
}

fn config(pairs: &[(&str, &str)]) -> Config {
    let mut c = Config::defaults();
    for (k, v) in pairs {
        c.set_raw(k, v).unwrap();
    }
    c
}

fn settings(pairs: &[(&str, &str)]) -> PrintSettings {
    PrintSettings::from_config(&config(pairs)).unwrap()
}

const ALL: &[(&str, &str)] = &[
    ("mica_texture_skin", "external"),
    ("mica_texture_skin_surfaces", "walls_and_tops"),
    ("mica_texture_skin_texture", "procedural:knurl_diamond"),
    ("mica_texture_skin_scale", "4"),
    ("mica_texture_skin_amplitude", "0.5"),
    ("mica_texture_skin_point_dist", "0.5"),
];

fn plain(mesh: &TriangleMesh, s: &PrintSettings) -> Option<Arc<Displaced>> {
    for_object(mesh, s, None, &crate::progress::Silent).unwrap()
}

/// Every undirected edge is shared by exactly two facets, once in each direction.
fn assert_watertight(m: &TriangleMesh) {
    let mut directed: HashMap<(u32, u32), u32> = HashMap::new();
    for f in &m.faces {
        for k in 0..3 {
            *directed.entry((f[k], f[(k + 1) % 3])).or_default() += 1;
        }
    }
    for (&(a, b), &n) in &directed {
        assert_eq!(n, 1, "edge {a}-{b} used {n} times in one direction");
        assert_eq!(
            directed.get(&(b, a)).copied(),
            Some(1),
            "edge {a}-{b} has no twin: a crack"
        );
    }
}

#[test]
fn walls_only_and_textures_off_change_nothing() {
    let mesh = cube([0.; 3], [20.; 3]);
    assert!(plain(&mesh, &settings(&[])).is_none());
    assert!(plain(&mesh, &settings(&[("mica_texture_skin", "external")])).is_none());
    // "All surfaces" without a texture turned on (and no paint) is nothing to do either.
    assert!(
        plain(
            &mesh,
            &settings(&[("mica_texture_skin_surfaces", "walls_and_tops")])
        )
        .is_none()
    );
}

#[test]
fn a_textured_cube_is_watertight_with_a_textured_top() {
    let mesh = cube([0.; 3], [20.; 3]);
    let d = plain(&mesh, &settings(ALL)).unwrap();
    // The top and the walls near it are refined (the rest of the walls is left to the wall
    // texture, see below).
    assert!(d.mesh.faces.len() > 5_000, "{}", d.mesh.faces.len());
    assert_watertight(&d.mesh);
    // The top carries the texture: vertices above 20 mm, by at most the depth.
    let top = d
        .mesh
        .vertices
        .iter()
        .filter(|v| v[2] > 19.9)
        .map(|v| v[2])
        .fold(f32::MIN, f32::max);
    assert!(top > 20.2 && top <= 20.5 + 1e-4, "{top}");
    // Nothing goes below the bed, and the first layer stays as it was.
    let (lo, _) = d.mesh.bounds();
    assert!(lo[2] >= 0.);
    for v in &d.mesh.vertices {
        if v[2] < 0.1 {
            assert!(
                (-1e-4..=20.0001).contains(&v[0]) && (-1e-4..=20.0001).contains(&v[1]),
                "{v:?}"
            );
        }
    }
    // The sides moved outward too (at the rims; the wall texture does the rest).
    let (_, hi) = d.mesh.bounds();
    assert!(hi[0] > 20.2);
    assert_eq!(d.volumes.len(), 1);
    assert!(same_mesh(d.volumes[0].as_ref().unwrap(), &d.mesh));
}

fn key_of(mesh: &TriangleMesh, s: &PrintSettings) -> String {
    let mut input = plan(mesh, s, None).unwrap().unwrap();
    input.walls = walls_left_to_wall_texture(&input).unwrap();
    cache_key(&input)
}

#[test]
fn displacement_is_deterministic_and_cached() {
    let mesh = cube([0.; 3], [12.; 3]);
    let s = settings(ALL);
    let a = plain(&mesh, &s).unwrap();
    let b = plain(&mesh, &s).unwrap();
    assert!(Arc::ptr_eq(&a, &b), "the second slice reuses the cache");
    forget(&key_of(&mesh, &s));
    let c = plain(&mesh, &s).unwrap();
    assert!(!Arc::ptr_eq(&a, &c));
    assert_eq!(*a, *c, "recomputing gives the same mesh");
}

#[test]
fn the_cache_follows_settings_mesh_and_paint() {
    let mesh = cube([0.; 3], [10.; 3]);
    let s = settings(ALL);
    let a = plain(&mesh, &s).unwrap();
    let mut deeper = ALL.to_vec();
    deeper.push(("mica_texture_skin_amplitude", "0.3"));
    let b = plain(&mesh, &settings(&deeper)).unwrap();
    assert_ne!(*a, *b, "a new depth recomputes");
    let moved = cube([0.; 3], [10., 10., 11.]);
    let c = plain(&moved, &s).unwrap();
    assert_ne!(*a, *c, "a new mesh recomputes");
    // Paint is part of the key.
    let base = config(&[("mica_texture_skin_surfaces", "walls_and_tops")]);
    let model = |paint: &str| {
        let mut painting = crate::painting::FacetPainting::default();
        painting.texture.push(4, paint).unwrap();
        RegionModel {
            volumes: vec![crate::regions::Volume {
                mesh: mesh.clone(),
                painting,
                kind: PartKind::Model,
                config: Config::new(),
            }],
            ..Default::default()
        }
    };
    let ps = PrintSettings::from_config(&base).unwrap();
    let (m1, m2) = (model("4"), model("8"));
    let p1 = for_object(&mesh, &ps, Some((&m1, &base)), &crate::progress::Silent)
        .unwrap()
        .unwrap();
    let p2 = for_object(&mesh, &ps, Some((&m2, &base)), &crate::progress::Silent)
        .unwrap()
        .unwrap();
    assert!(!Arc::ptr_eq(&p1, &p2));
    let mut c1 = vec![];
    let mut c2 = vec![];
    let i1 = Input {
        parts: vec![(0, &mesh, Some(&m1.volumes[0].config))],
        volume_count: 1,
        base: Base::Config(&base),
        ranges: vec![],
        modifiers: vec![],
        model: Some(&m1),
        bounds: Bounds {
            min: [0.; 3],
            max: [10.; 3],
        },
        floor: 0.2,
        bed: 0.,
        settings: &ps,
        walls: false,
    };
    c1.push(cache_key(&i1));
    let i2 = Input {
        model: Some(&m2),
        ..i1
    };
    c2.push(cache_key(&i2));
    assert_ne!(c1, c2);
}

#[test]
fn painted_slots_texture_only_their_area_with_their_own_depth() {
    let mesh = cube([0.; 3], [20.; 3]);
    let mut painting = crate::painting::FacetPainting::default();
    // The two top triangles (indices 2 and 3) with slot 1.
    painting.texture.push(2, "4").unwrap();
    painting.texture.push(3, "4").unwrap();
    let model = RegionModel {
        volumes: vec![crate::regions::Volume {
            mesh: mesh.clone(),
            painting,
            kind: PartKind::Model,
            config: Config::new(),
        }],
        ..Default::default()
    };
    let depth = |slot_depth: Option<&str>| {
        let slots = match slot_depth {
            Some(d) => format!("1:surfaces=walls_and_tops,amplitude={d}"),
            None => "1:surfaces=walls_and_tops".into(),
        };
        let c = config(&[
            ("mica_texture_skin_amplitude", "0.8"),
            ("mica_texture_skin_falloff", "0"),
            ("mica_texture_paint_slots", &slots),
        ]);
        let s = PrintSettings::from_config(&c).unwrap();
        let d = for_object(&mesh, &s, Some((&model, &c)), &crate::progress::Silent)
            .unwrap()
            .unwrap();
        assert_watertight(&d.mesh);
        let (_, hi) = d.mesh.bounds();
        // Only the top moved (with its rim): the sides below it stay where they were.
        for v in d.mesh.vertices.iter().filter(|v| v[2] < 19.9) {
            assert!(v.iter().all(|c| (-1e-4..=20.0001).contains(c)), "{v:?}");
        }
        f64::from(hi[2]) - 20.
    };
    let own = depth(Some("0.15"));
    assert!(own > 0.05 && own <= 0.15 + 1e-4, "{own}");
    // Without its own depth the slot uses the object's.
    let inherited = depth(None);
    assert!(inherited > 0.3 && inherited <= 0.8 + 1e-4, "{inherited}");
}

#[test]
fn a_modifier_textures_only_inside_it_without_cracks() {
    let mesh = cube([0.; 3], [20.; 3]);
    let modifier = cube([10., -1., -1.], [21., 21., 21.]);
    let mut on = Config::new();
    for (k, v) in ALL {
        on.set_raw(k, v).unwrap();
    }
    let model = RegionModel {
        volumes: vec![
            crate::regions::Volume {
                mesh: mesh.clone(),
                painting: Default::default(),
                kind: PartKind::Model,
                config: Config::new(),
            },
            crate::regions::Volume {
                mesh: modifier,
                painting: Default::default(),
                kind: PartKind::Modifier,
                config: on,
            },
        ],
        ..Default::default()
    };
    let c = Config::defaults();
    let s = PrintSettings::from_config(&c).unwrap();
    let d = for_object(&mesh, &s, Some((&model, &c)), &crate::progress::Silent)
        .unwrap()
        .unwrap();
    assert_watertight(&d.mesh);
    assert!(d.volumes[1].is_none(), "modifiers are not displaced");
    let (lo, hi) = d.mesh.bounds();
    assert!(hi[0] > 20.2, "inside the modifier the +X side moved");
    assert!(lo[0] >= -1e-4, "outside it the -X side did not");
    // The top moved only on the modifier's half.
    for v in &d.mesh.vertices {
        if v[2] > 20.0001 {
            assert!(v[0] > 9.5, "{v:?}");
        }
    }
}

#[test]
fn thin_walls_keep_their_inside() {
    // A 0.8 mm plate textured 1 mm inward on both sides would cut through itself.
    let mesh = cube([0.; 3], [20., 0.8, 10.]);
    let mut pairs = ALL.to_vec();
    pairs.push(("mica_texture_skin_direction", "inward"));
    pairs.push(("mica_texture_skin_amplitude", "1"));
    let d = plain(&mesh, &settings(&pairs)).unwrap();
    assert!(
        d.warnings.iter().any(|w| w.contains("shallower")),
        "{:?}",
        d.warnings
    );
    // On the big faces no vertex reached the middle of the plate.
    for v in &d.mesh.vertices {
        assert!(v[1] > -1e-3 && v[1] < 0.801, "{v:?}");
        if v[0] > 1. && v[0] < 19. && v[2] > 1. && v[2] < 9. {
            assert!(!(0.365..=0.435).contains(&v[1]), "{v:?}");
        }
    }
}

#[test]
fn a_mitre_moves_every_facet_by_one() {
    let x = [1., 0., 0.];
    let y = [0., 1., 0.];
    let z = [0., 0., 1.];
    for groups in [vec![x], vec![x, y], vec![x, y, z]] {
        let m = mitre(&groups);
        for g in &groups {
            assert!((dot(m, *g) - 1.).abs() < 1e-9, "{m:?}");
        }
    }
    // Nearly opposite facets are capped at twice the depth.
    let m = mitre(&[[1., 0., 0.], unit([-0.9, 0.1, 0.])]);
    assert!(length(m) <= 2. + 1e-9);
}

#[test]
fn the_disk_format_round_trips_and_rejects_damage() {
    let mesh = cube([0.; 3], [5.; 3]);
    let d = Displaced {
        mesh: mesh.clone(),
        volumes: vec![None, Some(mesh)],
        warnings: vec!["Texture: a note".into()],
        edge: 0.25,
        walls: Some(Arc::new(Rims::new(
            vec![[[0., 0., 5.], [5., 0., 5.]]],
            0.3,
            1.2,
        ))),
    };
    let bytes = encode(&d);
    assert_eq!(decode(&bytes), Some(d));
    assert_eq!(decode(&bytes[..bytes.len() - 1]), None);
    let mut bad = bytes.clone();
    bad[8] ^= 1;
    assert_eq!(decode(&bad), None, "another version");
    // Texture CLI fuzz, 2026-10-21: scrambled vertex bytes keep every length valid; without
    // the checksum they decoded into a wrong mesh (a 100,000-layer model).
    for at in [40, bytes.len() / 2, bytes.len() - 40] {
        let mut bad = bytes.clone();
        bad[at] ^= 0x40;
        assert_eq!(decode(&bad), None, "damage at byte {at}");
    }
}

#[test]
fn the_disk_cache_serves_a_new_process_and_stays_small() {
    let dir = std::env::temp_dir().join(format!("mica-texcache-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    set_disk_cache(Some(dir.clone()));
    let mesh = cube([0.; 3], [7.; 3]);
    let s = settings(ALL);
    let key = key_of(&mesh, &s);
    forget(&key);
    let a = plain(&mesh, &s).unwrap();
    // As after a restart: not in memory, read back from the folder.
    forget(&key);
    let b = plain(&mesh, &s).unwrap();
    set_disk_cache(None);
    assert!(!Arc::ptr_eq(&a, &b));
    assert_eq!(*a, *b);
    assert!(dir.join(format!("{key}.bin")).exists());
    trim_disk(&dir, 0);
    assert!(!dir.join(format!("{key}.bin")).exists());
    let _ = std::fs::remove_dir_all(&dir);
}

/// The vertices of `m` within the heights `z` that moved off the side planes of a box from the
/// origin to `max` (by more than a micrometre).
fn off_the_box(m: &TriangleMesh, max: f32, z: std::ops::Range<f32>) -> usize {
    m.vertices
        .iter()
        .filter(|v| z.contains(&v[2]))
        .filter(|v| {
            let on = |c: f32| c.abs() < 1e-3 || (c - max).abs() < 1e-3;
            !(on(v[0]) || on(v[1]))
        })
        .count()
}

#[test]
fn steep_walls_are_left_to_the_wall_texture_away_from_the_rims() {
    let mesh = cube([0.; 3], [20.; 3]);
    let s = settings(ALL);
    let d = plain(&mesh, &s).unwrap();
    assert_watertight(&d.mesh);
    let rims = d.walls.as_ref().expect("walls left to the wall texture");
    // The four top edges (the bottom has no texture).
    assert_eq!(rims.segs.len(), 4);
    for seg in &rims.segs {
        assert!(seg.iter().all(|p| (p[2] - 20.).abs() < 1e-9), "{seg:?}");
    }
    // Mid-height walls stay on the box; near the rims they carry the texture (the mitred
    // corner).
    assert_eq!(off_the_box(&d.mesh, 20., 2.0..15.0), 0);
    assert!(off_the_box(&d.mesh, 20., 19.5..20.01) > 100);
    let (_, hi) = d.mesh.bounds();
    assert!(hi[0] > 20.2 && hi[2] > 20.2, "{hi:?}");
    // Far fewer triangles than texturing every wall in the mesh, which baking still does.
    let baked = bake(&mesh, &s, None).unwrap().unwrap();
    assert!(baked.walls.is_none());
    assert_watertight(&baked.mesh);
    assert!(off_the_box(&baked.mesh, 20., 2.0..15.0) > 1000);
    assert!(
        d.mesh.faces.len() * 2 < baked.mesh.faces.len(),
        "{} vs {}",
        d.mesh.faces.len(),
        baked.mesh.faces.len()
    );
    // Classic walls cap the wall texture's outward depth: then the mesh keeps the walls.
    let classic = settings(&[ALL, &[("perimeter_generator", "classic")]].concat());
    assert!(plain(&mesh, &classic).unwrap().walls.is_none());
}

#[test]
fn the_shares_of_mesh_and_wall_texture_add_up() {
    let rims = Rims::new(vec![[[0., 0., 10.], [10., 0., 10.]]], 0.3, 1.2);
    // On the rim and within `near` the mesh carries everything, past `near + blend` nothing.
    assert_eq!(rims.wall_share([5., 0., 10.]), 0.);
    assert_eq!(rims.wall_share([5., 0., 9.75]), 0.);
    assert_eq!(rims.wall_share([5., 0., 8.]), 1.);
    assert_eq!(rims.wall_share([50., 0., 10.]), 1.);
    let mid = rims.wall_share([5., 0., 9.1]);
    assert!(mid > 0.3 && mid < 0.7, "{mid}");
    // A rim vertex moves only along the top's normal (the wall stays in its plane); a wall
    // vertex by its share.
    let x = [1., 0., 0.];
    let z = [0., 0., 1.];
    let m = mitre_to(&[x, z], &[0., 1.]);
    assert!(length(sub(m, z)) < 1e-12, "{m:?}");
    let m = mitre_to(&[x], &[0.25]);
    assert!(length(sub(m, scale(x, 0.25))) < 1e-12, "{m:?}");
}
