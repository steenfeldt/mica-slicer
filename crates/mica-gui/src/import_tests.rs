// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Importing profiles from other slicers, end to end in a headless app: fake OrcaSlicer,
//! Snapmaker Orca and PrusaSlicer folders under a temporary home (never the user's), the
//! startup notice and "Not now", the review list's default selection, idempotent import,
//! badges and groups in the picker data, "Show all", duplicates, changed sources, removal
//! and "Don't ask again".

use crate::app::{App, Options};
use crate::notifications::Action;
use crate::panels::profiles::{Origin, PresetKind};
use crate::state::Prefs;
use eframe::egui;
use mica_profiles::import::{Source, Status};
use mica_profiles::prusa::Kind;
use mica_profiles::user::Store;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mica-import-ui-{name}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

const U1: &str = "Snapmaker U1 (0.4 nozzle)";
const U1_PROCESS: &str = "0.20 Standard @Snapmaker U1 (0.4 nozzle)";

/// A home folder with an OrcaSlicer user printer, process and filament for the U1, the same
/// process again in Snapmaker Orca, a process for a printer Mica doesn't have, and two
/// PrusaSlicer filaments (one for the CORE One, one on an unknown parent).
fn fake_home() -> PathBuf {
    let home = temp("home");
    let orca = home.join("Library/Application Support/OrcaSlicer/user/default");
    write(
        &orca.join("machine/My U1.json"),
        &format!(
            r#"{{"from":"User","inherits":"{U1}","name":"My U1","printer_extruder_id":["1","2","3","4"]}}"#
        ),
    );
    write(&orca.join("machine/My U1.info"), "sync_info =\n");
    write(
        &orca.join("process/Fine.json"),
        &format!(
            r#"{{"from":"User","inherits":"{U1_PROCESS}","name":"Fine","sparse_infill_density":"10%","wall_loops":"3"}}"#
        ),
    );
    write(
        &orca.join("filament/My PLA.json"),
        r#"{"from":"User","inherits":"Snapmaker PLA @U1","name":"My PLA","nozzle_temperature":["215"]}"#,
    );
    write(
        &orca.join("process/Voron fast.json"),
        r#"{"from":"User","inherits":"0.20mm Standard @Nowhere 0.4","name":"Voron fast"}"#,
    );
    let snap = home.join("Library/Application Support/Snapmaker_Orca/user/default");
    write(
        &snap.join("process/Fine.json"),
        &format!(
            r#"{{"from":"User","inherits":"{U1_PROCESS}","name":"Fine","sparse_infill_density":"10%","wall_loops":"3","version":"2.4.0.7"}}"#
        ),
    );
    let prusa = home.join("Library/Application Support/PrusaSlicer");
    write(&prusa.join("PrusaSlicer.ini"), "[presets]\n");
    write(
        &prusa.join("filament/PLA Tweaked.ini"),
        "inherits = Prusament PLA @COREONE HF0.4\ntemperature = 212\n",
    );
    write(
        &prusa.join("filament/Odd.ini"),
        "inherits = No Such Filament\ntemperature = 212\n",
    );
    home
}

/// A headless app on its own store and preferences, looking at `home` for other slicers;
/// `None` (a reported skip) without the pinned Prusa and Orca profiles.
fn app(ctx: &egui::Context, store: &Path, home: &Path, prefs: Option<Prefs>) -> Option<App> {
    if !(mica_testkit::require_prusa_profiles() && mica_testkit::require_orca_profiles()) {
        return None;
    }
    let mut app = App::create(
        ctx,
        Options {
            prefs_path: None,
            store: Some(Store::open(store)),
            files: vec![],
            headless: true,
        },
    );
    assert!(
        app.profiles.vendor_index("Snapmaker").is_some(),
        "no Snapmaker profiles"
    );
    if let Some(p) = prefs {
        app.prefs = p;
    }
    app.prefs.add_printer("Snapmaker", U1);
    app.profiles.look_in(home, None);
    Some(app)
}

#[test]
fn the_notice_asks_first_and_not_now_is_remembered() {
    let ctx = egui::Context::default();
    let home = fake_home();
    let Some(mut app) = app(&ctx, &temp("store"), &home, None) else {
        return;
    };
    let before = app.profiles.store.list_all().unwrap().len();
    app.offer_imports();
    let n = app
        .notifications
        .list
        .iter()
        .find(|n| n.actions.iter().any(|(_, a)| *a == Action::ReviewImports))
        .expect("a notice")
        .clone();
    assert!(
        n.text.starts_with(
            "Found 1 printer, 1 filament and 2 print profiles in OrcaSlicer, 1 print profile \
             in Snapmaker Orca and 2 filament profiles in PrusaSlicer"
        ),
        "{}",
        n.text
    );
    assert!(
        n.actions
            .iter()
            .any(|(l, a)| l == "Review and import…" && *a == Action::ReviewImports)
    );
    // Nothing was imported by finding them.
    assert_eq!(app.profiles.store.list_all().unwrap().len(), before);
    app.run_notification_action(n.id, Action::DismissImports, &ctx);
    assert!(app.notifications.list.iter().all(|x| x.id != n.id));
    // Asked once: the same presets are not offered again (the preferences remember it).
    let prefs = Prefs::from_toml(&app.prefs.to_toml()).unwrap();
    app.prefs = prefs;
    app.offer_imports();
    assert!(
        !app.notifications
            .list
            .iter()
            .any(|n| n.actions.iter().any(|(_, a)| *a == Action::ReviewImports))
    );
    // A new preset is offered on its own.
    write(
        &home.join("Library/Application Support/OrcaSlicer/user/default/filament/New PLA.json"),
        r#"{"inherits":"Snapmaker PLA @U1","name":"New PLA"}"#,
    );
    app.offer_imports();
    let n = app.notifications.list.last().unwrap();
    assert!(
        n.text.starts_with("Found 1 filament profile in OrcaSlicer"),
        "{}",
        n.text
    );
}

#[test]
fn dont_ask_again_is_per_slicer_and_persists() {
    let ctx = egui::Context::default();
    let home = fake_home();
    let Some(mut app) = app(&ctx, &temp("store"), &home, None) else {
        return;
    };
    app.prefs.import.set_asks(Source::OrcaSlicer, false);
    app.prefs.import.set_asks(Source::SnapmakerOrca, false);
    let prefs = Prefs::from_toml(&app.prefs.to_toml()).unwrap();
    assert!(!prefs.import.asks(Source::OrcaSlicer));
    assert!(prefs.import.asks(Source::PrusaSlicer));
    app.prefs = prefs;
    app.offer_imports();
    let n = app.notifications.list.last().unwrap();
    assert_eq!(n.title, "Found 2 filament profiles in PrusaSlicer");
    // The review window still lists every slicer (Settings › Profiles imports at any time).
    app.open_import_review();
    let review = app.import_ui.review.as_ref().unwrap();
    assert!(
        review
            .rows
            .iter()
            .any(|r| r.found.source == Source::OrcaSlicer)
    );
    assert!(!review.ask[&Source::OrcaSlicer]);
}

#[test]
fn review_checks_what_suits_my_printers_and_import_is_idempotent() {
    let ctx = egui::Context::default();
    let home = fake_home();
    let store = temp("store");
    let Some(mut app) = app(&ctx, &store, &home, None) else {
        return;
    };
    let (rows, errors) = app.profiles.import_candidates(&app.prefs);
    assert!(errors.is_empty(), "{errors:?}");
    let row = |source: Source, name: &str| {
        rows.iter()
            .find(|r| r.found.source == source && r.found.name == name)
            .unwrap_or_else(|| panic!("{name}"))
    };
    for name in ["My U1", "Fine", "My PLA"] {
        let r = row(Source::OrcaSlicer, name);
        assert_eq!(r.vendor.as_deref(), Some("Snapmaker"), "{name}");
        assert!(r.selected && r.for_my_printers, "{name}");
    }
    let twin = row(Source::SnapmakerOrca, "Fine");
    assert_eq!(twin.duplicate_of.as_deref(), Some("OrcaSlicer's “Fine”"));
    assert!(twin.importable() && !twin.selected);
    assert!(row(Source::OrcaSlicer, "Voron fast").problem.is_some());
    assert!(!row(Source::OrcaSlicer, "Voron fast").selected);
    // The CORE One is in My printers (the first-run default).
    let tweaked = row(Source::PrusaSlicer, "PLA Tweaked");
    assert_eq!(tweaked.vendor.as_deref(), Some("Prusa"));
    assert!(tweaked.selected);
    assert!(row(Source::PrusaSlicer, "Odd").problem.is_some());

    let chosen: Vec<_> = rows.iter().filter(|r| r.selected).collect();
    let done = app.profiles.import(&chosen, &mut app.prefs);
    assert!(done.errors.is_empty(), "{:?}", done.errors);
    // Snapmaker Orca's "Fine" has the same settings as OrcaSlicer's: listed, left unchecked.
    assert_eq!(done.names.len(), 4, "{:?}", done.names);
    let stored = app.profiles.store.list_all().unwrap();
    assert_eq!(stored.len(), 4);
    assert!(app.prefs.has_printer("Snapmaker", "My U1"));
    // Badges: imported presets carry their slicer; system presets none.
    assert_eq!(
        app.profiles.origin(PresetKind::Print, "Fine"),
        Origin::Imported(Source::OrcaSlicer)
    );
    assert_eq!(
        app.profiles.origin(PresetKind::Print, U1_PROCESS),
        Origin::System
    );
    let p = app
        .profiles
        .user_profile(PresetKind::Print, "Fine")
        .unwrap();
    let tip = crate::panels::import::origin_text(p);
    assert!(tip.starts_with("Imported from OrcaSlicer\nFile: "), "{tip}");
    assert!(tip.contains("Imported on "), "{tip}");
    // Picker data: on the U1 the imported process and filament are listed; on its own user
    // printer too (Orca accepts a user printer's parent).
    let (vi, _, _) = app
        .profiles
        .all_printers()
        .into_iter()
        .find(|(_, _, n)| n == "My U1")
        .unwrap();
    app.profiles.select_printer(vi, "My U1");
    assert!(app.profiles.processes.iter().any(|n| n == "Fine"));
    assert!(app.profiles.materials.iter().any(|n| n == "My PLA"));
    app.profiles.process = "Fine".into();
    app.profiles.refresh();
    assert_eq!(
        app.profiles.config().unwrap().float("fill_density"),
        Some(10.)
    );

    // Importing again changes nothing: same names, same files, no duplicates.
    let (again, _) = app.profiles.import_candidates(&app.prefs);
    for r in again
        .iter()
        .filter(|r| r.problem.is_none() && r.duplicate_of.is_none())
    {
        assert!(
            matches!(r.status, Status::Imported(_)),
            "{:?}",
            r.found.name
        );
        assert!(!r.importable());
    }
    let every: Vec<_> = again
        .iter()
        .filter(|r| r.selected || matches!(r.status, Status::Imported(_)))
        .collect();
    let done = app.profiles.import(&every, &mut app.prefs);
    assert!(done.names.is_empty(), "{:?}", done.names);
    assert_eq!(app.profiles.store.list_all().unwrap(), stored);

    // A fresh start finds the copies in Mica's store, with their badges.
    let Some(fresh) = self::app(&ctx, &store, &home, Some(app.prefs.clone())) else {
        return;
    };
    assert_eq!(
        fresh.profiles.origin(PresetKind::Filament, "PLA Tweaked"),
        Origin::Imported(Source::PrusaSlicer)
    );
    assert_eq!(
        fresh.profiles.origin(PresetKind::Printer, "My U1"),
        Origin::Imported(Source::OrcaSlicer)
    );
}

#[test]
fn duplicates_merge_changed_sources_reimport_and_removal_keeps_the_originals() {
    let ctx = egui::Context::default();
    let home = fake_home();
    let Some(mut app) = app(&ctx, &temp("store"), &home, None) else {
        return;
    };
    let (rows, _) = app.profiles.import_candidates(&app.prefs);
    let all: Vec<_> = rows.iter().filter(|r| r.importable()).collect();
    app.profiles.import(&all, &mut app.prefs);
    // The two "Fine" presets are identical.
    let groups = app.profiles.duplicate_groups();
    assert_eq!(groups.len(), 1, "{groups:?}");
    let mut names: Vec<&str> = groups[0].iter().map(|(_, n)| n.as_str()).collect();
    names.sort_unstable();
    assert_eq!(names, ["Fine", "Fine (Snapmaker Orca)"]);
    // Merging keeps the one in use and points the selection at it.
    let (vi, _, _) = app
        .profiles
        .all_printers()
        .into_iter()
        .find(|(_, _, n)| n == U1)
        .unwrap();
    app.profiles.select_printer(vi, U1);
    app.profiles.process = "Fine (Snapmaker Orca)".into();
    app.profiles.refresh();
    app.profiles.store_selection(&mut app.prefs);
    let merged = app.profiles.merge_duplicates(&groups[0], &mut app.prefs);
    assert_eq!(merged.names, [(Kind::Print, "Fine".to_string())]);
    assert_eq!(app.profiles.process, "Fine (Snapmaker Orca)");
    assert!(app.profiles.duplicate_groups().is_empty());

    // A changed source file is noticed and re-imported in place.
    assert!(app.profiles.stale_imports().is_empty());
    let my_pla =
        home.join("Library/Application Support/OrcaSlicer/user/default/filament/My PLA.json");
    write(
        &my_pla,
        r#"{"from":"User","inherits":"Snapmaker PLA @U1","name":"My PLA","nozzle_temperature":["220"]}"#,
    );
    let stale = app.profiles.stale_imports();
    assert_eq!(stale.len(), 1);
    assert_eq!(stale[0].name, "My PLA");
    assert!(stale[0].exists);
    let done = app
        .profiles
        .reimport(&[(Kind::Filament, "My PLA".into())], false, &mut app.prefs);
    assert_eq!(done.names, [(Kind::Filament, "My PLA".to_string())]);
    assert!(app.profiles.stale_imports().is_empty());
    let p = app
        .profiles
        .user_profile(PresetKind::Filament, "My PLA")
        .unwrap();
    assert_eq!(p.orca.as_ref().unwrap()["nozzle_temperature"][0], "220");

    // Unused imports: not selected, not in My printers, not a default.
    let unused = app.profiles.unused_imported(&app.prefs);
    assert!(unused.contains(&(Kind::Filament, "PLA Tweaked".to_string())));
    assert!(!unused.iter().any(|(_, n)| n == "Fine (Snapmaker Orca)"));
    assert!(!unused.iter().any(|(_, n)| n == "My U1"));
    // Removing all of a slicer's imports: Mica's copies go to its trash, the other slicer's
    // files stay, My printers and the selection let go.
    let orca: Vec<(Kind, String)> = app
        .profiles
        .imported()
        .into_iter()
        .filter(|p| p.source() == Some(Source::OrcaSlicer))
        .map(|p| (p.kind, p.name.clone()))
        .collect();
    let done = app.profiles.remove_profiles(&orca, &mut app.prefs);
    assert!(done.errors.is_empty(), "{:?}", done.errors);
    assert!(my_pla.is_file());
    assert!(!app.prefs.has_printer("Snapmaker", "My U1"));
    assert!(
        app.profiles
            .imported()
            .iter()
            .all(|p| p.source() != Some(Source::OrcaSlicer))
    );
    assert!(app.profiles.store.root.join("trash/filament").is_dir());
    assert!(!app.profiles.materials.iter().any(|m| m == "My PLA"));
}

#[test]
fn pickers_hide_presets_for_other_printers_until_show_all() {
    let ctx = egui::Context::default();
    let Some(mut app) = app(&ctx, &temp("store"), &temp("empty-home"), None) else {
        return;
    };
    let (vi, _, name) = app
        .profiles
        .all_printers()
        .into_iter()
        .find(|(_, v, _)| v == "Prusa")
        .unwrap();
    app.profiles
        .select_printer(vi, "Prusa CORE One HF0.4 nozzle");
    let _ = name;
    let compatible = app.profiles.processes.clone();
    assert!(
        compatible
            .iter()
            .all(|p| !app.profiles.is_incompatible(PresetKind::Print, p))
    );
    assert!(!compatible.iter().any(|p| p.contains("@XLIS")));
    let current = app.profiles.process.clone();
    let hidden = app.profiles.hidden_count(PresetKind::Print);
    assert!(hidden > 0);
    app.show_presets_for_other_printers(true);
    assert_eq!(app.profiles.processes.len(), compatible.len() + hidden);
    assert_eq!(app.profiles.hidden_count(PresetKind::Print), 0);
    assert!(app.profiles.processes.iter().any(|p| p.contains("@XLIS")));
    let other = app
        .profiles
        .processes
        .iter()
        .find(|p| p.contains("@XLIS"))
        .unwrap()
        .clone();
    assert!(app.profiles.is_incompatible(PresetKind::Print, &other));
    assert_eq!(
        app.profiles.incompatible_count(PresetKind::Print),
        app.profiles.processes.len() - compatible.len()
    );
    // The compatible ones come first and the selection stays.
    assert_eq!(app.profiles.processes[..compatible.len()], compatible[..]);
    assert_eq!(app.profiles.process, current);
    app.show_presets_for_other_printers(false);
    assert_eq!(app.profiles.processes, compatible);
    assert_eq!(app.profiles.incompatible_count(PresetKind::Print), 0);
}

/// The rows a grouped picker draws, in order (headless frame).
fn picker_rows(
    ctx: &egui::Context,
    profiles: &crate::panels::profiles::Profiles,
    kind: PresetKind,
    names: &[String],
) -> Vec<String> {
    let mut rows = Vec::new();
    let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
        let id = ui.make_persistent_id("test-picker");
        crate::panels::import::grouped_presets(
            ui,
            id,
            profiles,
            kind,
            names,
            "",
            None,
            |_, n, _| {
                rows.push(n.to_string());
                false
            },
        );
    });
    // No renderer here: the font atlas upload is dropped on purpose.
    output.textures_delta.clear();
    rows
}

/// Pickers list the user's own profiles first, then imported ones, then the system presets
/// (print, filament and printer pickers alike).
#[test]
fn pickers_list_mine_then_imported_then_system() {
    let ctx = egui::Context::default();
    let home = fake_home();
    let Some(mut app) = app(&ctx, &temp("store"), &home, None) else {
        return;
    };
    let (rows, _) = app.profiles.import_candidates(&app.prefs);
    let chosen: Vec<_> = rows.iter().filter(|r| r.selected).collect();
    app.profiles.import(&chosen, &mut app.prefs);
    let (vi, _, _) = app
        .profiles
        .all_printers()
        .into_iter()
        .find(|(_, _, n)| n == U1)
        .unwrap();
    app.profiles.select_printer(vi, U1);
    let own = app
        .profiles
        .save_user_profile(
            PresetKind::Print,
            "Zz mine",
            U1_PROCESS,
            &[("fill_density".to_string(), "15%".to_string())]
                .into_iter()
                .collect(),
        )
        .unwrap();
    assert_eq!(own.source(), None);
    app.profiles.refresh();
    let listed = picker_rows(
        &ctx,
        &app.profiles,
        PresetKind::Print,
        &app.profiles.processes.clone(),
    );
    let at = |n: &str| {
        listed
            .iter()
            .position(|x| x == n)
            .unwrap_or_else(|| panic!("{n}"))
    };
    assert_eq!(at("Zz mine"), 0, "{listed:?}");
    assert_eq!(at("Fine"), 1, "{listed:?}");
    assert!(at(U1_PROCESS) > 1);
    // Filaments: the imported one ahead of every system filament.
    let filaments = picker_rows(
        &ctx,
        &app.profiles,
        PresetKind::Filament,
        &app.profiles.materials.clone(),
    );
    assert_eq!(filaments[0], "My PLA", "{filaments:?}");
    // My printers: the imported "My U1" ahead of the system printers added before it.
    let printers: Vec<String> = app
        .profiles
        .my_printers_in_picker_order(&app.prefs)
        .into_iter()
        .map(|(_, n)| n)
        .collect();
    assert_eq!(printers[0], "My U1", "{printers:?}");
    assert!(printers.iter().any(|n| n == U1));
}

/// A filament built on Orca's shared library (`Generic PLA @System`) is listed for every
/// printer sliced with Orca rules, as in OrcaSlicer, not only for the vendor it came with.
#[test]
fn library_filaments_show_for_every_orca_printer() {
    let ctx = egui::Context::default();
    let home = temp("lib-home");
    write(
        &home.join("Library/Application Support/OrcaSlicer/user/default/filament/Lib PLA.json"),
        r#"{"from":"User","inherits":"Generic PLA @System","name":"Lib PLA","nozzle_temperature":["212"]}"#,
    );
    let store = temp("store");
    let Some(mut app) = app(&ctx, &store, &home, None) else {
        return;
    };
    let (rows, errors) = app.profiles.import_candidates(&app.prefs);
    assert!(errors.is_empty(), "{errors:?}");
    let row = rows.iter().find(|r| r.found.name == "Lib PLA").unwrap();
    assert_eq!(
        row.vendor.as_deref(),
        Some(mica_profiles::import::ORCA_LIBRARY)
    );
    assert!(row.selected && row.for_my_printers);
    let done = app.profiles.import(&[row], &mut app.prefs);
    assert!(done.errors.is_empty(), "{:?}", done.errors);
    let stored = app
        .profiles
        .user_profile(PresetKind::Filament, "Lib PLA")
        .unwrap()
        .clone();
    assert_eq!(stored.inherits_source, mica_profiles::import::ORCA_LIBRARY);
    let pick = |app: &mut App, vendor: &str| {
        let (vi, _, name) = app
            .profiles
            .all_printers()
            .into_iter()
            .find(|(_, v, _)| v == vendor)
            .unwrap();
        app.profiles.select_printer(vi, &name);
        name
    };
    pick(&mut app, "Snapmaker");
    assert!(app.profiles.materials.iter().any(|m| m == "Lib PLA"));
    // Another Orca vendor loaded after the import lists it too, and it composes there.
    app.profiles
        .ensure_vendor("Anker")
        .expect("Anker in the pinned Orca profiles");
    let anker = pick(&mut app, "Anker");
    assert!(
        app.profiles.materials.iter().any(|m| m == "Lib PLA"),
        "{:?}",
        app.profiles.materials
    );
    app.profiles.filaments.fill("Lib PLA".into());
    app.profiles.refresh();
    assert_eq!(
        app.profiles.config().unwrap().float_at("temperature", 0),
        Some(212.)
    );
    // Selected on the Anker printer, it is in use; on a fresh start with that printer, the
    // vendor loads with it.
    app.profiles.store_selection(&mut app.prefs);
    assert!(app.profiles.in_use(&stored, &app.prefs));
    app.prefs.add_printer("Anker", &anker);
    let Some(mut fresh) = self::app(&ctx, &store, &home, Some(app.prefs.clone())) else {
        return;
    };
    fresh.profiles.load_referenced(["Anker"]);
    fresh.profiles.restore(&fresh.prefs);
    assert_eq!(fresh.profiles.vendor_name(), "Anker");
    assert_eq!(fresh.profiles.filaments[0], "Lib PLA");
    // Removing it takes it out of every vendor.
    let done = fresh
        .profiles
        .remove_profiles(&[(Kind::Filament, "Lib PLA".into())], &mut fresh.prefs);
    assert!(done.errors.is_empty(), "{:?}", done.errors);
    assert!(!fresh.profiles.materials.iter().any(|m| m == "Lib PLA"));
    pick(&mut fresh, "Snapmaker");
    assert!(!fresh.profiles.materials.iter().any(|m| m == "Lib PLA"));
}

/// Re-importing a changed PrusaSlicer preset keeps the changes made in Mica on top; a
/// setting changed on both sides is named, and "Take theirs" uses PrusaSlicer's value.
#[test]
fn prusaslicer_reimport_keeps_mica_edits_and_asks_about_conflicts() {
    let ctx = egui::Context::default();
    let home = fake_home();
    let Some(mut app) = app(&ctx, &temp("store"), &home, None) else {
        return;
    };
    let (rows, _) = app.profiles.import_candidates(&app.prefs);
    let row = rows
        .iter()
        .find(|r| r.found.source == Source::PrusaSlicer && r.found.name == "PLA Tweaked")
        .unwrap();
    app.profiles.import(&[row], &mut app.prefs);
    let (vi, _, _) = app
        .profiles
        .all_printers()
        .into_iter()
        .find(|(_, _, n)| n == "Prusa CORE One HF0.4 nozzle")
        .unwrap();
    app.profiles
        .select_printer(vi, "Prusa CORE One HF0.4 nozzle");
    // Edited in Mica: one setting PrusaSlicer will change too, one it won't.
    app.profiles
        .save_user_profile(
            PresetKind::Filament,
            "PLA Tweaked",
            "PLA Tweaked",
            &[
                ("temperature".to_string(), "216".to_string()),
                ("first_layer_temperature".to_string(), "231".to_string()),
            ]
            .into_iter()
            .collect(),
        )
        .unwrap();
    let file = home.join("Library/Application Support/PrusaSlicer/filament/PLA Tweaked.ini");
    write(
        &file,
        "inherits = Prusament PLA @COREONE HF0.4\ntemperature = 200\nbed_temperature = 63\n",
    );
    let stale = app.profiles.stale_imports();
    assert_eq!(stale.len(), 1);
    assert_eq!(stale[0].conflicts, ["temperature"]);
    let (rows, _) = app.profiles.import_candidates(&app.prefs);
    let row = rows.iter().find(|r| r.found.name == "PLA Tweaked").unwrap();
    assert!(matches!(row.status, Status::Changed(_)));
    assert_eq!(row.conflicts, ["temperature"]);
    assert!(!row.take_theirs);
    // Keep mine: Mica's values stay, PrusaSlicer's other change comes in.
    let done = app.profiles.reimport(
        &[(Kind::Filament, "PLA Tweaked".into())],
        false,
        &mut app.prefs,
    );
    assert!(done.errors.is_empty(), "{:?}", done.errors);
    let settings = |app: &App| {
        app.profiles
            .user_profile(PresetKind::Filament, "PLA Tweaked")
            .unwrap()
            .settings
            .clone()
    };
    let s = settings(&app);
    assert_eq!(s["temperature"], "216");
    assert_eq!(s["first_layer_temperature"], "231");
    assert_eq!(s["bed_temperature"], "63");
    assert!(app.profiles.stale_imports().is_empty());
    // It survives the profile file and resolves for slicing.
    let stored = app
        .profiles
        .store
        .load(Kind::Filament, "PLA Tweaked")
        .unwrap();
    assert_eq!(stored.settings, s);
    // Changed on both sides again: Take theirs.
    write(
        &file,
        "inherits = Prusament PLA @COREONE HF0.4\ntemperature = 205\nbed_temperature = 63\n",
    );
    assert_eq!(app.profiles.stale_imports()[0].conflicts, ["temperature"]);
    app.profiles.reimport(
        &[(Kind::Filament, "PLA Tweaked".into())],
        true,
        &mut app.prefs,
    );
    let s = settings(&app);
    assert_eq!(s["temperature"], "205");
    assert_eq!(s["first_layer_temperature"], "231");
    app.profiles.filaments.fill("PLA Tweaked".into());
    app.profiles.refresh();
    assert_eq!(
        app.profiles.config().unwrap().float_at("temperature", 0),
        Some(205.)
    );
    // Changed only in PrusaSlicer: no question, Mica's change stays.
    write(
        &file,
        "inherits = Prusament PLA @COREONE HF0.4\ntemperature = 205\nbed_temperature = 64\n",
    );
    assert!(app.profiles.stale_imports()[0].conflicts.is_empty());
}

/// Visible filaments start from OrcaSlicer's selected filaments (Snapmaker Orca's for the
/// U1) and PrusaSlicer's installed ones, per printer, when profiles are imported. The picker
/// hides the others until starred or "Show all"; the selection and the slice don't change.
#[test]
fn visible_filaments_are_seeded_and_only_change_the_picker() {
    let ctx = egui::Context::default();
    let home = fake_home();
    write(
        &home.join("Library/Application Support/OrcaSlicer/OrcaSlicer.conf"),
        "{\n\"filaments\": [\"Generic PETG @System\"]\n}\n# MD5 checksum 00\n",
    );
    write(
        &home.join("Library/Application Support/Snapmaker_Orca/Snapmaker_Orca.conf"),
        "{\"filaments\": [\"Snapmaker PLA @U1\", \"Generic PLA @System\", \"Not a filament\"]}",
    );
    write(
        &home.join("Library/Application Support/PrusaSlicer/PrusaSlicer.ini"),
        "[filaments]\nPrusament PLA @COREONE HF0.4 = 1\nPrusament PETG @COREONE HF0.4 = \n",
    );
    let Some(mut app) = app(&ctx, &temp("store"), &home, None) else {
        return;
    };
    app.prefs
        .add_printer("Prusa", "Prusa CORE One HF0.4 nozzle");
    let (rows, _) = app.profiles.import_candidates(&app.prefs);
    let chosen: Vec<_> = rows.iter().filter(|r| r.selected).collect();
    app.profiles.import(&chosen, &mut app.prefs);
    let u1 = crate::state::printer_id("Snapmaker", U1);
    assert_eq!(
        app.prefs.visible_filaments[&u1],
        ["Generic PLA @System", "Snapmaker PLA @U1"]
    );
    assert_eq!(
        app.prefs.visible_filaments
            [&crate::state::printer_id("Prusa", "Prusa CORE One HF0.4 nozzle")],
        ["Prusament PLA @COREONE HF0.4"]
    );
    // An imported printer gets its own list too.
    assert!(
        app.prefs
            .visible_filaments
            .contains_key(&crate::state::printer_id("Snapmaker", "My U1"))
    );
    // The list survives mica.toml.
    let back = crate::state::Prefs::from_toml(&app.prefs.to_toml()).unwrap();
    assert_eq!(back.visible_filaments, app.prefs.visible_filaments);

    let (vi, _, _) = app
        .profiles
        .all_printers()
        .into_iter()
        .find(|(_, _, n)| n == U1)
        .unwrap();
    app.profiles.select_printer(vi, U1);
    app.profiles.filaments.fill("Snapmaker PETG @U1".into());
    app.profiles.refresh();
    let materials = app.profiles.materials.clone();
    let config = app.profiles.config().unwrap();
    let hidden = |app: &App, n: &str| app.profiles.filament_hidden(&app.prefs, n);
    // Listed: the seeded ones, the selected one, the imported one.
    assert!(!hidden(&app, "Snapmaker PLA @U1"));
    assert!(!hidden(&app, "Snapmaker PETG @U1"));
    assert!(!hidden(&app, "My PLA"));
    let other = materials
        .iter()
        .find(|m| {
            !m.contains("Snapmaker PLA @U1")
                && !m.contains("Snapmaker PETG @U1")
                && !app.profiles.is_user_preset(PresetKind::Filament, m)
                && *m != "Generic PLA @System"
        })
        .unwrap()
        .clone();
    assert!(hidden(&app, &other));
    // A star lists it; another hides one; "Show all" goes back to every filament.
    app.profiles
        .set_filament_visible(&mut app.prefs, &other, true);
    assert!(!hidden(&app, &other));
    app.profiles
        .set_filament_visible(&mut app.prefs, "Snapmaker PLA @U1", false);
    assert!(hidden(&app, "Snapmaker PLA @U1"));
    app.profiles.show_all_filaments(&mut app.prefs);
    assert!(!app.prefs.visible_filaments.contains_key(&u1));
    assert!(!hidden(&app, &other));
    // None of it touched what is listed for slicing, the selection or the config.
    assert_eq!(app.profiles.materials, materials);
    assert_eq!(app.profiles.filaments[0], "Snapmaker PETG @U1");
    assert_eq!(
        app.profiles.config().unwrap().values().collect::<Vec<_>>(),
        config.values().collect::<Vec<_>>()
    );
    // A printer without a list, or a slicer without one, lists everything (Mica's default).
    let Some(mut bare) = self::app(&ctx, &temp("store"), &temp("empty-home"), None) else {
        return;
    };
    let (rows, _) = bare.profiles.import_candidates(&bare.prefs);
    assert!(rows.is_empty());
    bare.profiles.import(&[], &mut bare.prefs);
    assert!(bare.prefs.visible_filaments.is_empty());
}

/// The labels a grouped picker draws for its rows, in order (headless frame).
fn picker_labels(
    ctx: &egui::Context,
    profiles: &crate::panels::profiles::Profiles,
    kind: PresetKind,
    names: &[String],
) -> Vec<String> {
    let mut rows = Vec::new();
    let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
        let id = ui.make_persistent_id("test-picker-labels");
        crate::panels::import::grouped_presets(
            ui,
            id,
            profiles,
            kind,
            names,
            "",
            None,
            |ui, n, l| {
                rows.push(l.text());
                crate::panels::import::preset_row(ui, profiles, kind, n, l, false)
            },
        );
    });
    output.textures_delta.clear();
    rows
}

/// The CORE One, XL and U1 pickers show presets without their ` @…` tags, every row still
/// told apart from the others (with and without the presets for other printers), while the
/// selection keeps the full names.
#[test]
fn picker_labels_drop_compatibility_tags() {
    let ctx = egui::Context::default();
    let Some(mut app) = app(&ctx, &temp("store"), &temp("empty-home"), None) else {
        return;
    };
    let mut checked = 0;
    for printer in [
        "Prusa CORE One HF0.4 nozzle",
        "Original Prusa XL - 5T Input Shaper 0.4 nozzle",
        U1,
    ] {
        let (vi, _, _) = app
            .profiles
            .all_printers()
            .into_iter()
            .find(|(_, _, n)| n == printer)
            .unwrap_or_else(|| panic!("{printer} is not among the loaded printers"));
        app.profiles.select_printer(vi, printer);
        for all in [false, true] {
            app.show_presets_for_other_printers(all);
            for (kind, names) in [
                (PresetKind::Print, app.profiles.processes.clone()),
                (PresetKind::Filament, app.profiles.materials.clone()),
            ] {
                assert!(
                    names.iter().any(|n| n.contains(" @")),
                    "{printer}: {names:?}"
                );
                let labels = picker_labels(&ctx, &app.profiles, kind, &names);
                assert_eq!(labels.len(), names.len());
                for l in &labels {
                    assert!(!l.contains('@'), "{printer}: {l}");
                }
                let unique: std::collections::BTreeSet<&String> = labels.iter().collect();
                assert_eq!(unique.len(), labels.len(), "{printer} all={all}");
                checked += 1;
            }
        }
        app.show_presets_for_other_printers(false);
        assert!(
            app.profiles.process.contains(" @"),
            "{}",
            app.profiles.process
        );
        assert!(app.profiles.filaments[0].contains(" @"));
    }
    assert!(checked > 0);
}
