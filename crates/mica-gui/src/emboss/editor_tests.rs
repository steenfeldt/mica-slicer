// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The tool in a headless editor: text on a cube (raised, engraved), a text object and an SVG
//! survive Save / Open in both dialects and re-open editable; the scene slices.

use super::Kind;
use crate::app::{App, Options};
use eframe::egui;
use glam::Vec3;
use mica_core::emboss::EmbossSource;
use mica_core::mesh_cut::is_watertight;
use mica_core::threemf::{Flavour, PartKind};
use mica_profiles::user::Store;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mica-emboss-test-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

fn app(ctx: &egui::Context, files: Vec<PathBuf>) -> App {
    App::create(
        ctx,
        Options {
            prefs_path: None,
            store: Some(Store::open(scratch("store"))),
            files,
            headless: true,
        },
    )
}

fn cube() -> PathBuf {
    PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/models/cube20.stl"
    ))
}

fn reopen(ctx: &egui::Context, path: &Path) -> App {
    let mut b = app(ctx, vec![]);
    let result = crate::app::load_files(vec![path.to_path_buf()], &b.config, b.bed.center());
    b.finish_load(result);
    assert!(!b.plates.is_blank(), "{}", b.status);
    b
}

fn layers(app: &App) -> usize {
    let config = app.plate_config(app.plates.current).unwrap();
    let objects = app.plates.current().objects.clone();
    let finished = crate::worker::slice(
        objects,
        config,
        &std::sync::atomic::AtomicBool::new(false),
        &|_| {},
    )
    .unwrap();
    let gcode = std::fs::read_to_string(finished.preview.job.path.join("print.gcode")).unwrap();
    gcode
        .lines()
        .filter(|l| l.starts_with(";LAYER_CHANGE"))
        .count()
}

fn set_text(a: &mut App, text: &str) {
    let text = text.to_string();
    a.edit_emboss(
        move |c| {
            if let EmbossSource::Text(t) = &mut c.source {
                t.text = text;
            }
        },
        false,
    );
}

/// Top centre and −Y face centre of the (only) cube, world.
fn faces(a: &App) -> ((Vec3, Vec3), (Vec3, Vec3)) {
    let (min, max) = a.plates.current().objects[0].bounds();
    let c = (min + max) * 0.5;
    (
        (Vec3::new(c.x, c.y, max.z), Vec3::Z),
        (Vec3::new(c.x, min.y, c.z), -Vec3::Y),
    )
}

#[test]
fn raised_and_engraved_text_on_a_cube_reopen_editable() {
    let ctx = egui::Context::default();
    let mut a = app(&ctx, vec![cube()]);
    let plain_layers = layers(&a);
    a.select(Some(0), false);
    // T on a selected object adds a raised text and opens the card on it.
    a.toggle_emboss();
    let (i, Some(raised)) = a.emboss_target().unwrap() else {
        panic!("a text volume")
    };
    assert_eq!(i, 0);
    let ((top, up), (side, out)) = faces(&a);
    a.emboss_place(0, top, up);
    set_text(&mut a, "Top");
    a.emboss = None;
    // A second text, engraved into the -Y wall.
    a.add_text_volume(0, Kind::Engraved);
    let (_, Some(engraved)) = a.emboss_target().unwrap() else {
        panic!()
    };
    a.emboss_place(0, side, out);
    set_text(&mut a, "Side");
    let o = &a.plates.current().objects[0];
    assert_eq!(o.volumes.len(), 2);
    for v in &o.volumes {
        assert!(is_watertight(&v.mesh), "{}", v.name);
    }
    assert_eq!(o.volume(raised).unwrap().kind, PartKind::Model);
    assert_eq!(o.volume(engraved).unwrap().kind, PartKind::NegativeVolume);
    // The raised text adds layers above the cube's 20 mm.
    let with_text = layers(&a);
    assert!(with_text > plain_layers, "{plain_layers} → {with_text}");
    for flavour in [Flavour::Prusa, Flavour::Orca] {
        let path = scratch(&format!("text-{}.3mf", flavour.name()));
        a.save_project_to(&path, flavour).unwrap();
        let mut b = reopen(&ctx, &path);
        let ob = &b.plates.current().objects[0];
        assert_eq!(ob.volumes.len(), 2, "{flavour:?}");
        let texts: Vec<String> = ob
            .volumes
            .iter()
            .map(|v| {
                v.emboss
                    .as_ref()
                    .and_then(|e| e.config.text())
                    .unwrap()
                    .text
                    .clone()
            })
            .collect();
        assert!(texts.contains(&"Top".to_string()) && texts.contains(&"Side".to_string()));
        let side_vol = ob
            .volumes
            .iter()
            .find(|v| v.emboss.as_ref().unwrap().config.text().unwrap().text == "Side")
            .unwrap();
        assert_eq!(side_vol.kind, PartKind::NegativeVolume, "{flavour:?}");
        let side_id = side_vol.id;
        let before = ob.volume_bounds(side_vol);
        // Re-opening the tool on it and editing rebuilds it in place on the same wall.
        b.select(Some(0), false);
        b.sub = Some(crate::scene::Sub::Volume(side_id));
        b.toggle_emboss();
        assert_eq!(b.emboss_target(), Some((0, Some(side_id))), "{flavour:?}");
        set_text(&mut b, "Side");
        let ob = &b.plates.current().objects[0];
        let after = ob.volume_bounds(ob.volume(side_id).unwrap());
        assert!(
            (before.0 - after.0).abs().max_element() < 0.02
                && (before.1 - after.1).abs().max_element() < 0.02,
            "{flavour:?}: {before:?} → {after:?}"
        );
        assert_eq!(layers(&b), with_text, "{flavour:?}");
    }
}

#[test]
fn text_object_and_svg_round_trip() {
    let ctx = egui::Context::default();
    let mut a = app(&ctx, vec![cube()]);
    // Add › Text with nothing selected (T would pick the plate's only model).
    a.select(None, false);
    a.add_text();
    let (i, None) = a.emboss_target().unwrap() else {
        panic!("a text object")
    };
    assert_eq!(a.plates.current().objects.len(), 2);
    let text_object = &a.plates.current().objects[i];
    assert!(is_watertight(&text_object.mesh));
    let (min, _) = text_object.bounds();
    assert!(min.z.abs() < 1e-3, "on the bed: {min}");
    // An SVG on the cube.
    a.emboss = None;
    a.select(Some(0), false);
    a.add_text_volume(0, Kind::Raised);
    a.emboss_svg_data(
        "star.svg",
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="20mm" height="20mm" viewBox="0 0 100 100"><path d="M50 5 L61 39 L97 39 L68 61 L79 95 L50 74 L21 95 L32 61 L3 39 L39 39 Z"/></svg>"#.into(),
    );
    a.edit_emboss(|c| c.depth = 2., false);
    let svg_cfg = a.plates.current().objects[0].volumes[0]
        .emboss
        .clone()
        .unwrap()
        .config
        .clone();
    assert!(svg_cfg.svg().is_some());
    for flavour in [Flavour::Prusa, Flavour::Orca] {
        let path = scratch(&format!("objects-{}.3mf", flavour.name()));
        a.save_project_to(&path, flavour).unwrap();
        let b = reopen(&ctx, &path);
        let objects = &b.plates.current().objects;
        assert_eq!(objects.len(), 2, "{flavour:?}");
        let text = objects
            .iter()
            .find(|o| o.emboss.is_some())
            .expect("text object");
        assert_eq!(
            text.emboss.as_ref().unwrap().config.text().unwrap().text,
            "Text"
        );
        let cube = objects.iter().find(|o| o.emboss.is_none()).unwrap();
        let svg = cube.volumes[0].emboss.as_ref().expect("svg volume");
        assert_eq!(svg.config, svg_cfg, "{flavour:?}");
    }
}

/// Copies of an object with the same text share one prepared object (shared instances
/// compare the print mesh and the volumes, text included); a different text does not.
#[test]
fn text_copies_share_and_different_texts_do_not() {
    let ctx = egui::Context::default();
    let mut a = app(&ctx, vec![cube()]);
    a.select(Some(0), false);
    a.toggle_emboss();
    let ((top, up), _) = faces(&a);
    a.emboss_place(0, top, up);
    a.emboss = None;
    let mut copy = a.plates.current().objects[0].fresh();
    copy.position.x += 40.;
    // An independent copy (an instance would take the edit too, see below).
    copy.object_id = a.next_object_id();
    a.plates.current_mut().objects.push(copy);
    let groups = |a: &App| {
        let config = a.plate_config(a.plates.current).unwrap();
        let objects = &a.plates.current().objects;
        let inputs: Vec<_> = objects
            .iter()
            .map(|o| {
                (
                    o.print_mesh(),
                    o.placed_region(),
                    crate::worker::object_config(&config, o).unwrap(),
                )
            })
            .collect();
        let candidates: Vec<_> = inputs
            .iter()
            .zip(objects)
            .map(
                |((mesh, region, config), o)| mica_core::instances::Candidate {
                    mesh,
                    region: region.as_ref(),
                    tools: &o.tools,
                    config,
                },
            )
            .collect();
        mica_core::instances::group(&candidates)
    };
    let g = groups(&a);
    assert_eq!((g[0].0, g[1].0), (0, 0), "same text: shared");
    // The copy's text changes: its own prepared object.
    a.select(Some(1), false);
    let id = a.plates.current().objects[1].volumes[0].id;
    a.sub = Some(crate::scene::Sub::Volume(id));
    a.toggle_emboss();
    set_text(&mut a, "Other");
    let g = groups(&a);
    assert_eq!((g[0].0, g[1].0), (0, 1), "different text: not shared");
    // Instances (same object id) follow the edit and keep sharing.
    a.plates.current_mut().objects[1].object_id = a.plates.current().objects[0].object_id;
    a.sync_instances(1);
    let g = groups(&a);
    assert_eq!((g[0].0, g[1].0), (0, 0), "instances share the edited text");
}

/// The slice cache's key covers the text: editing it is a new key, undo returns the old one.
#[test]
fn slice_cache_key_follows_the_text() {
    let ctx = egui::Context::default();
    let mut a = app(&ctx, vec![cube()]);
    a.select(Some(0), false);
    a.toggle_emboss();
    let before = a.slice_inputs(a.plates.current).unwrap();
    set_text(&mut a, "Changed");
    let after = a.slice_inputs(a.plates.current).unwrap();
    assert!(before != after);
    a.undo();
    assert!(a.slice_inputs(a.plates.current).unwrap() == before);
    // Depth alone (same outline) is also part of the key.
    a.edit_emboss(|c| c.depth = 2.5, false);
    assert!(a.slice_inputs(a.plates.current).unwrap() != before);
}
