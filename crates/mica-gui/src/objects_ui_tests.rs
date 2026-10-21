// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The object tree, the scope bar and viewport selection driven through the whole app:
//! drill-down picking and Esc, Tab through an object's parts, switching the scope bar with
//! a click, overrides made and removed by editing at the object and part scopes, and what
//! the tree lists.

use crate::app::{App, Options};
use crate::intents::Intent;
use crate::panels::editor::Edit;
use crate::panels::scope::Segment;
use crate::scene::{Sub, Volume};
use eframe::egui::{self, Event, Key, Modifiers, PointerButton, RawInput};
use mica_core::threemf::PartKind;
use mica_profiles::user::Store;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT: AtomicUsize = AtomicUsize::new(0);

fn app(models: &[&str]) -> (egui::Context, App) {
    let ctx = egui::Context::default();
    let dir = std::env::temp_dir().join(format!(
        "mica-objects-ui-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let app = App::create(
        &ctx,
        Options {
            prefs_path: None,
            store: Some(Store::open(dir)),
            files: models
                .iter()
                .map(|m| {
                    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/models/"))
                        .join(m)
                })
                .collect(),
            headless: true,
        },
    );
    (ctx, app)
}

fn frame(ctx: &egui::Context, app: &mut App, events: Vec<Event>) {
    let input = RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1440., 900.),
        )),
        events,
        ..Default::default()
    };
    let mut output = ctx.run_ui(input, |ui| app.show(ui));
    output.textures_delta.clear();
}

fn settle(ctx: &egui::Context, app: &mut App) {
    for _ in 0..30 {
        frame(ctx, app, vec![]);
        if app.loading.is_none() && !app.plates.current().objects.is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    frame(ctx, app, vec![]);
}

fn press(ctx: &egui::Context, app: &mut App, key: Key, modifiers: Modifiers) {
    let ev = |pressed| Event::Key {
        key,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers,
    };
    frame(ctx, app, vec![Event::ModifiersChanged(modifiers), ev(true)]);
    frame(
        ctx,
        app,
        vec![ev(false), Event::ModifiersChanged(Modifiers::NONE)],
    );
}

fn click(ctx: &egui::Context, app: &mut App, at: egui::Pos2) {
    frame(ctx, app, vec![Event::PointerMoved(at)]);
    let button = |pressed| Event::PointerButton {
        pos: at,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    };
    frame(ctx, app, vec![button(true)]);
    frame(ctx, app, vec![button(false)]);
    frame(ctx, app, vec![]);
}

/// A cube with a modifier inside it (fully hidden by the body).
fn cube_with_inner_modifier() -> crate::scene::Object {
    let mesh = mica_core::mesh_edit::cuboid([20., 20., 20.]);
    let mut o = crate::scene::Object::new(mica_core::input::InputObject::plain(
        "cube",
        0,
        0,
        mesh,
        mica_core::print_job::ToolAssignments([Some(0); 5]),
        mica_core::config::Config::defaults(),
    ));
    let v = Volume::new(
        "inner",
        PartKind::Modifier,
        mica_core::mesh_edit::cuboid([6., 6., 6.]),
    );
    o.volumes.push(v);
    o
}

#[test]
fn a_click_drills_down_from_the_object_to_its_hidden_part() {
    use crate::scene::picking::click_target;
    use glam::Vec3;
    let o = cube_with_inner_modifier();
    let id = o.volumes[0].id;
    let objects = [o];
    let down = -Vec3::Z;
    let centre = Vec3::new(0., 0., 50.);
    // Nothing selected: the click selects the object, not the modifier inside it.
    assert_eq!(
        click_target(&objects, None, centre, down, false),
        Some((0, None))
    );
    // The object selected: the same click picks its modifier (x-ray).
    assert_eq!(
        click_target(&objects, Some(0), centre, down, false),
        Some((0, Some(id)))
    );
    // Beside the modifier: the object again.
    let side = Vec3::new(8., 8., 50.);
    assert_eq!(
        click_target(&objects, Some(0), side, down, false),
        Some((0, None))
    );
    // Option-click: the part directly, whatever is selected.
    assert_eq!(
        click_target(&objects, None, centre, down, true),
        Some((0, Some(id)))
    );
    // Missing everything.
    let off = Vec3::new(50., 50., 50.);
    assert_eq!(click_target(&objects, Some(0), off, down, true), None);
}

#[test]
fn esc_goes_up_one_level_and_tab_cycles_the_parts() {
    let (ctx, mut a) = app(&["cube20.stl"]);
    settle(&ctx, &mut a);
    a.select(Some(0), false);
    a.add_intent(Intent::Stronger);
    a.add_intent(Intent::Hole);
    a.add_intent(Intent::Finer);
    frame(&ctx, &mut a, vec![]);
    let order = crate::tree::tree_order(&a.plates.current().objects[0]);
    assert_eq!(order.len(), 3);
    // Esc: part → object → nothing.
    a.sub = Some(order[1]);
    press(&ctx, &mut a, Key::Escape, Modifiers::NONE);
    assert_eq!((a.selected, a.sub), (Some(0), None));
    press(&ctx, &mut a, Key::Escape, Modifiers::NONE);
    assert_eq!(a.selected, None);
    // Tab walks the parts in tree order and comes back to the object; Shift+Tab goes back.
    a.select(Some(0), false);
    frame(&ctx, &mut a, vec![]);
    let preview = a.preview;
    for want in order.iter().map(|s| Some(*s)).chain([None, Some(order[0])]) {
        press(&ctx, &mut a, Key::Tab, Modifiers::NONE);
        assert_eq!(a.sub, want);
    }
    press(&ctx, &mut a, Key::Tab, Modifiers::SHIFT);
    assert_eq!(a.sub, None);
    press(&ctx, &mut a, Key::Tab, Modifiers::SHIFT);
    assert_eq!(a.sub, Some(order[2]));
    // Tab never switched to Preview, and Shift+Tab never hid the sidebar here.
    assert_eq!(a.preview, preview);
    assert!(!a.keys.sidebar_hidden);
    // The tree follows: the selected part's row is the selected one.
    frame(&ctx, &mut a, vec![]);
    let selected: Vec<&str> = a
        .editor
        .tree
        .probe
        .iter()
        .filter(|r| r.selected)
        .map(|r| r.label.as_str())
        .collect();
    assert_eq!(
        selected,
        [crate::tree::part_name(&a.plates.current().objects[0], order[2]).as_str()]
    );
    // Without parts Tab is Prepare / Preview again (the keymap's own binding).
    let (ctx, mut b) = app(&["cube20.stl"]);
    settle(&ctx, &mut b);
    b.select(Some(0), false);
    press(&ctx, &mut b, Key::Tab, Modifiers::SHIFT);
    assert!(b.keys.sidebar_hidden);
}

#[test]
fn the_tree_lists_kinds_groups_and_what_parts_change() {
    let (ctx, mut a) = app(&["cube20.stl", "hole_block.stl"]);
    settle(&ctx, &mut a);
    a.select(Some(0), false);
    a.add_intent(Intent::Stronger);
    a.add_intent(Intent::Smooth);
    let s = a.sub.unwrap();
    a.tree_edit(0, "Group", |o| crate::tree::group(o, &[s], "Top"));
    a.add_intent(Intent::Supports);
    frame(&ctx, &mut a, vec![]);
    let rows: Vec<(&str, &str, &str)> = a
        .editor
        .tree
        .probe
        .iter()
        .map(|r| (r.what, r.label.as_str(), r.detail.as_str()))
        .collect();
    assert_eq!(rows[0].0, "object");
    assert!(rows.contains(&("kind", "Modifiers · 2", "")), "{rows:?}");
    assert!(rows.contains(&("group", "Top", "group · 1")), "{rows:?}");
    assert!(
        rows.contains(&("part", "Smooth top here", "ironing")),
        "{rows:?}"
    );
    assert!(
        rows.contains(&("part", "Stronger here", "infill 40 % · gyroid")),
        "{rows:?}"
    );
    assert!(
        rows.contains(&("kind", "Support enforcers · 1", "")),
        "{rows:?}"
    );
    assert!(
        rows.contains(&("part", "Supports here", "Support enforcer")),
        "{rows:?}"
    );
    // The other object is folded and prints with the profile.
    assert!(
        rows.contains(&("object", "hole_block", "profile settings")),
        "{rows:?}"
    );
    // The group folds.
    let key = format!("{}/modifiers/Top", a.plates.current().objects[0].id);
    a.editor.tree.folded.insert(key);
    frame(&ctx, &mut a, vec![]);
    assert!(
        !a.editor
            .tree
            .probe
            .iter()
            .any(|r| r.label == "Smooth top here")
    );
}

#[test]
fn the_scope_bar_switches_with_a_click_and_edits_land_at_its_scope() {
    let (ctx, mut a) = app(&["cube20.stl"]);
    settle(&ctx, &mut a);
    a.select(Some(0), false);
    frame(&ctx, &mut a, vec![]);
    // Selecting an object puts the sidebar on it.
    assert_eq!(a.scope_segment(), Segment::Object);
    a.add_intent(Intent::Stronger);
    frame(&ctx, &mut a, vec![]);
    assert_eq!(a.scope_segment(), Segment::Part);
    let segments = a.editor.scope_probe.clone();
    assert_eq!(
        segments.iter().map(|(s, _)| *s).collect::<Vec<_>>(),
        [Segment::Profile, Segment::Object, Segment::Part]
    );
    // Part scope: only what a modifier can change (no layer height), and the rows show the
    // pre-filled walls as an override.
    let keys: Vec<&str> = a.editor.sidebar_probe.rows.iter().map(|r| r.key).collect();
    assert!(!keys.contains(&"layer_height"), "{keys:?}");
    // Click the object segment.
    let at = |s: Segment| segments.iter().find(|(k, _)| *k == s).unwrap().1.center();
    click(&ctx, &mut a, at(Segment::Object));
    assert_eq!(a.scope_segment(), Segment::Object);
    assert!(a.sub.is_some(), "the part stays selected");
    let keys: Vec<&str> = a.editor.sidebar_probe.rows.iter().map(|r| r.key).collect();
    assert!(keys.contains(&"layer_height"), "{keys:?}");
    // An edit at the object scope is the object's override, though a part is selected.
    a.apply_edit(
        None,
        Edit::Set {
            key: "fill_density".into(),
            raw: "25%".into(),
        },
    );
    a.end_gesture();
    let o = &a.plates.current().objects[0];
    assert_eq!(
        o.overrides.get("fill_density").map(String::as_str),
        Some("25%")
    );
    assert_eq!(
        o.volumes[0]
            .overrides
            .get("fill_density")
            .map(String::as_str),
        Some("40%")
    );
    // ↺ removes it again.
    a.apply_edit(None, Edit::Reset("fill_density".into()));
    a.end_gesture();
    assert!(a.plates.current().objects[0].overrides.is_empty());
    // An edit equal to what the scope inherits is no override.
    let profile = a
        .plate_config(a.plates.current)
        .unwrap()
        .raw("fill_density")
        .unwrap();
    a.apply_edit(
        None,
        Edit::Set {
            key: "fill_density".into(),
            raw: profile,
        },
    );
    a.end_gesture();
    assert!(a.plates.current().objects[0].overrides.is_empty());
    // Back to the part: its edit lands on the part.
    let segments = a.editor.scope_probe.clone();
    let at = |s: Segment| segments.iter().find(|(k, _)| *k == s).unwrap().1.center();
    click(&ctx, &mut a, at(Segment::Part));
    assert_eq!(a.scope_segment(), Segment::Part);
    a.apply_edit(
        None,
        Edit::Set {
            key: "ironing".into(),
            raw: "1".into(),
        },
    );
    a.end_gesture();
    let o = &a.plates.current().objects[0];
    assert_eq!(
        o.volumes[0].overrides.get("ironing").map(String::as_str),
        Some("1")
    );
    assert!(!o.overrides.contains_key("ironing"));
    // The rail's count is the part's overrides (walls, density, pattern, ironing).
    frame(&ctx, &mut a, vec![]);
    // Profile: the bar stays, the list edits the profile and the object is untouched.
    let segments = a.editor.scope_probe.clone();
    let at = |s: Segment| segments.iter().find(|(k, _)| *k == s).unwrap().1.center();
    click(&ctx, &mut a, at(Segment::Profile));
    assert_eq!(a.scope_segment(), Segment::Profile);
    assert!(a.scope_env().is_none());
    assert!(a.selected.is_some());
}

#[test]
fn a_hovered_part_of_the_selected_object_is_outlined() {
    let (ctx, mut a) = app(&["cube20.stl"]);
    settle(&ctx, &mut a);
    a.select(Some(0), false);
    a.add_intent(Intent::Stronger);
    a.sub = None;
    a.fit_current_plate(true);
    frame(&ctx, &mut a, vec![]);
    // Point at the object's centre on screen: its modifier (inside) is hovered.
    let r = a.viewport_rect;
    let o = &a.plates.current().objects[0];
    let c = o.pivot_world();
    let uv = a
        .viewport
        .camera
        .project(c, r.width() / r.height())
        .expect("on screen");
    let at = egui::pos2(r.left() + uv.x * r.width(), r.top() + uv.y * r.height());
    frame(&ctx, &mut a, vec![Event::PointerMoved(at)]);
    frame(&ctx, &mut a, vec![Event::PointerMoved(at)]);
    let id = a.plates.current().objects[0].volumes[0].id;
    assert_eq!(a.viewport.hovered_volume, Some(id));
    // And a click selects it.
    click(&ctx, &mut a, at);
    assert_eq!(a.sub, Some(Sub::Volume(id)));
}
