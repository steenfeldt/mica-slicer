// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! End-to-end tests of the library over temporary folders only.

use crate::testkit::{TempDir, repo_root, synthetic_library};
use crate::{FolderState, Kind, Library, Options};
use std::path::Path;
use std::time::Duration;

const T: Duration = Duration::from_secs(60);

fn quiet() -> Options {
    Options {
        watch: false,
        offline_check: Duration::from_millis(100),
        ..Default::default()
    }
}

fn copy(from: &str, to: &Path) {
    std::fs::create_dir_all(to.parent().unwrap()).unwrap();
    std::fs::copy(repo_root().join(from), to).unwrap();
}

/// Every file under `dir` with its size and modification time.
fn listing(dir: &Path) -> Vec<(String, u64, std::time::SystemTime)> {
    let mut out = vec![];
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            let m = e.metadata().unwrap();
            if m.is_dir() {
                stack.push(e.path());
            } else {
                out.push((
                    e.path().display().to_string(),
                    m.len(),
                    m.modified().unwrap(),
                ));
            }
        }
    }
    out.sort();
    out
}

#[test]
fn scans_reads_metadata_and_never_writes_in_the_library() {
    let lib = TempDir::new("lib");
    let data = TempDir::new("data");
    let stats = synthetic_library(lib.path(), 60).unwrap();
    let before = listing(lib.path());
    let l = Library::open(data.path(), quiet()).unwrap();
    l.add_folder(lib.path()).unwrap();
    assert!(l.wait_idle(T));
    let s = l.snapshot();
    assert_eq!(s.files, stats.files, "hidden/.git/._ files are skipped");
    assert_eq!(s.folders.len(), 1);
    assert_eq!(s.folders[0].state, FolderState::Ready);
    assert_eq!(s.folders[0].files, stats.files);
    // Every kind turned up, with what it should carry.
    for k in Kind::ALL {
        assert!(s.models.iter().any(|m| m.kind == k), "no {k:?}");
    }
    let stl = s.models.iter().find(|m| m.kind == Kind::Stl).unwrap();
    assert!(stl.info.readable && stl.info.size_mm.is_some());
    assert!(stl.thumbnail.as_ref().unwrap().exists());
    assert!(stl.info.thumbnail_rendered || stl.info.thumbnail.is_some());
    let g = s.models.iter().find(|m| m.kind == Kind::Gcode).unwrap();
    assert!(g.info.sliced && g.info.print_time_s.is_some() && g.info.printer.is_some());
    let step = s.models.iter().find(|m| m.kind == Kind::Step).unwrap();
    assert!(!step.info.readable);
    let orca = s
        .models
        .iter()
        .find(|m| m.name.ends_with("plates-orca.3mf"))
        .unwrap();
    assert!(orca.info.plates >= 2);
    // Nothing in the library folder changed; thumbnails are in the data folder.
    assert_eq!(listing(lib.path()), before);
    assert!(stl.thumbnail.as_ref().unwrap().starts_with(data.path()));
}

#[test]
fn duplicates_are_one_model_with_several_locations() {
    let lib = TempDir::new("dup");
    let data = TempDir::new("dup-data");
    copy("tests/models/cube20.stl", &lib.path().join("a/cube.stl"));
    copy(
        "tests/models/cube20.stl",
        &lib.path().join("b/c/cube-copy.stl"),
    );
    copy("tests/models/taper.stl", &lib.path().join("taper.stl"));
    let l = Library::open(data.path(), quiet()).unwrap();
    l.add_folder(lib.path()).unwrap();
    assert!(l.wait_idle(T));
    let s = l.snapshot();
    assert_eq!(s.files, 3);
    assert_eq!(s.models.len(), 2);
    let cube = s.models.iter().find(|m| m.locations.len() == 2).unwrap();
    assert_eq!(cube.name, "cube.stl");
    // The folder is shown with the platform's separator.
    assert_eq!(
        cube.locations[1].folder(),
        Path::new("b").join("c").to_string_lossy()
    );
    // Tags follow the content: both copies share them.
    l.set_tags(
        &cube.id,
        &["Calibration".into(), " calibration ".into(), "".into()],
    );
    l.set_favourite(&cube.id, true);
    let id = cube.id.clone();
    assert!(l.wait_idle(T));
    let m = l
        .snapshot()
        .models
        .iter()
        .find(|m| m.id == id)
        .cloned()
        .unwrap();
    assert_eq!(m.user.tags, ["Calibration"]);
    assert!(m.user.favourite);
    drop(l);
    // ... and persist.
    let l = Library::open(data.path(), quiet()).unwrap();
    assert!(l.wait_idle(T));
    let m = l
        .snapshot()
        .models
        .iter()
        .find(|m| m.id == id)
        .cloned()
        .unwrap();
    assert!(m.user.favourite && m.user.tags == ["Calibration"]);
    assert_eq!(l.all_tags(), ["Calibration"]);
}

#[test]
fn incremental_rescan_reads_only_what_changed() {
    let lib = TempDir::new("inc");
    let data = TempDir::new("inc-data");
    synthetic_library(lib.path(), 40).unwrap();
    let l = Library::open(data.path(), quiet()).unwrap();
    l.add_folder(lib.path()).unwrap();
    assert!(l.wait_idle(T));
    let first = l.snapshot();
    let stl = first
        .models
        .iter()
        .find(|m| m.kind == Kind::Stl && m.locations.len() == 1)
        .unwrap();
    let target = stl.primary().path.clone();
    let old_id = stl.id.clone();
    l.set_tags(&old_id, &["keep me".into()]);
    // Edit one file, add one, delete one.
    std::thread::sleep(Duration::from_millis(20));
    copy("tests/models/hole_block.stl", &target);
    copy(
        "tests/models/thin_features.stl",
        &lib.path().join("New/thin.stl"),
    );
    let victim = first
        .models
        .iter()
        .find(|m| m.kind == Kind::Obj)
        .unwrap()
        .primary()
        .path
        .clone();
    std::fs::remove_file(&victim).unwrap();
    l.rescan(None);
    assert!(l.wait_idle(T));
    let s = l.snapshot();
    assert_eq!(s.files, first.files);
    let edited = s
        .models
        .iter()
        .find(|m| m.locations.iter().any(|x| x.path == target))
        .unwrap();
    assert_ne!(edited.id, old_id, "content hash follows the edit");
    assert_eq!(edited.user.tags, ["keep me"], "tags follow an edited file");
    assert!(s.models.iter().any(|m| m.name == "thin.stl"));
    assert!(
        !s.models
            .iter()
            .any(|m| m.locations.iter().any(|x| x.path == victim))
    );
    // Unchanged files are skipped by size and time: a second rescan reads nothing.
    let t = std::time::Instant::now();
    l.rescan(None);
    assert!(l.wait_idle(T));
    assert!(t.elapsed() < Duration::from_secs(5));
    assert_eq!(l.snapshot().files, s.files);
}

#[test]
fn offline_folder_keeps_its_models_and_comes_back() {
    let base = TempDir::new("vol");
    let data = TempDir::new("vol-data");
    let share = base.path().join("share");
    copy("tests/models/cube20.stl", &share.join("cube.stl"));
    copy("tests/models/taper.stl", &share.join("sub/taper.stl"));
    let l = Library::open(data.path(), quiet()).unwrap();
    l.add_folder(&share).unwrap();
    assert!(l.wait_idle(T));
    assert_eq!(l.snapshot().models.len(), 2);
    // "Unmount" (the mount point disappears), rescan: the models stay, marked offline.
    let parked = base.path().join("parked");
    std::fs::rename(&share, &parked).unwrap();
    l.rescan(None);
    assert!(l.wait_idle(T));
    let s = l.snapshot();
    assert_eq!(s.folders[0].state, FolderState::Offline);
    assert_eq!(s.models.len(), 2);
    assert!(s.models.iter().all(|m| !m.online()));
    // A restart while it is away keeps them too.
    drop(l);
    let l = Library::open(data.path(), quiet()).unwrap();
    assert!(l.wait_idle(T));
    assert_eq!(l.snapshot().models.len(), 2);
    assert_eq!(l.snapshot().folders[0].state, FolderState::Offline);
    // Back again: noticed by the availability check, scanned, online.
    std::fs::rename(&parked, &share).unwrap();
    assert!(l.wait_for(T, |s| s.folders[0].state == FolderState::Ready
        && s.models.iter().all(|m| m.online())));
    assert_eq!(l.snapshot().models.len(), 2);
}

/// GUI monkey test, seed 108: a folder added while it was away was stored under the path as
/// typed (`/var/…` on macOS, a symbolic link), not the resolved one the library uses
/// everywhere else, so it could be indexed twice.
/// Unix only: creating a symbolic link on Windows needs a privilege test runners lack.
#[cfg(unix)]
#[test]
fn a_folder_added_while_away_is_stored_resolved() {
    let base = TempDir::new("away");
    let data = TempDir::new("away-data");
    let real = base.path().join("real");
    std::fs::create_dir_all(&real).unwrap();
    let link = base.path().join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let share = link.join("share");
    let l = Library::open(data.path(), quiet()).unwrap();
    l.add_folder(&share).unwrap();
    assert!(l.wait_idle(T));
    assert_eq!(l.snapshot().folders[0].path, real.join("share"));
    copy("tests/models/cube20.stl", &real.join("share/cube.stl"));
    assert!(l.add_folder(&real.join("share")).is_err(), "already there");
    l.rescan(None);
    assert!(l.wait_for(T, |s| s.files == 1));
    let s = l.snapshot();
    assert!(
        s.models
            .iter()
            .all(|m| m.locations.iter().all(|x| x.path.starts_with(&real))),
        "{:?}",
        s.models
    );
}

#[test]
fn removing_a_folder_forgets_its_files_only() {
    let a = TempDir::new("rm-a");
    let b = TempDir::new("rm-b");
    let data = TempDir::new("rm-data");
    copy("tests/models/cube20.stl", &a.path().join("cube.stl"));
    copy("tests/models/cube20.stl", &b.path().join("cube.stl"));
    copy("tests/models/taper.stl", &b.path().join("taper.stl"));
    let l = Library::open(data.path(), quiet()).unwrap();
    l.add_folder(a.path()).unwrap();
    l.add_folder(b.path()).unwrap();
    assert!(l.wait_idle(T));
    assert!(
        l.add_folder(&a.path().join("sub")).is_err(),
        "inside a library folder"
    );
    assert!(l.add_folder(a.path()).is_err(), "already there");
    assert!(
        l.add_folder(&data.path().join("x")).is_err(),
        "Mica's own data"
    );
    assert_eq!(l.snapshot().files, 3);
    l.remove_folder(b.path());
    assert!(l.wait_idle(T));
    let s = l.snapshot();
    assert_eq!(s.files, 1);
    assert_eq!(s.models.len(), 1);
    assert_eq!(s.folders.len(), 1);
    assert!(
        b.path().join("taper.stl").exists(),
        "the folder itself is untouched"
    );
}

#[test]
fn live_updates_follow_file_events() {
    let lib = TempDir::new("live");
    let data = TempDir::new("live-data");
    copy("tests/models/cube20.stl", &lib.path().join("cube.stl"));
    let l = Library::open(
        data.path(),
        Options {
            debounce: Duration::from_millis(100),
            ..Default::default()
        },
    )
    .unwrap();
    l.add_folder(lib.path()).unwrap();
    assert!(l.wait_idle(T));
    assert!(l.snapshot().folders[0].watching);
    // FSEvents may take a moment to start delivering; keep nudging with new files.
    let mut found = false;
    for i in 0..20 {
        copy(
            "tests/models/taper.stl",
            &lib.path().join(format!("drop/taper{i}.stl")),
        );
        if l.wait_for(Duration::from_secs(3), |s| s.files >= 2) {
            found = true;
            break;
        }
    }
    assert!(found, "a new file shows up without a rescan");
    let n = l.snapshot().files;
    std::fs::remove_dir_all(lib.path().join("drop")).unwrap();
    assert!(
        l.wait_for(Duration::from_secs(20), |s| s.files == 1),
        "a deleted folder disappears: {} → {}",
        n,
        l.snapshot().files
    );
}

#[test]
fn paused_scanner_waits() {
    let lib = TempDir::new("pause");
    let data = TempDir::new("pause-data");
    synthetic_library(lib.path(), 80).unwrap();
    let l = Library::open(data.path(), quiet()).unwrap();
    l.set_paused(true);
    l.add_folder(lib.path()).unwrap();
    std::thread::sleep(Duration::from_millis(400));
    assert!(!l.idle());
    assert!(l.snapshot().files < 80);
    l.set_paused(false);
    assert!(l.wait_idle(T));
    assert!(l.snapshot().files >= 80);
}

#[test]
fn downloads_become_projects_and_choices_survive_rescans() {
    use crate::group::{Cover, projects};
    use crate::{ExtraKind, Grouping};
    let lib = TempDir::new("projects");
    let data = TempDir::new("projects-data");
    crate::testkit::synthetic_downloads(lib.path(), 30).unwrap();
    synthetic_library(&lib.path().join("Mixed"), 12).unwrap();
    let before = listing(lib.path());
    let l = Library::open(data.path(), quiet()).unwrap();
    l.add_folder(lib.path()).unwrap();
    assert!(l.wait_idle(T));
    let s = l.snapshot();
    let p = projects(&s);
    let names: Vec<&str> = p.list.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(
        names,
        ["Articulated Dragon", "Cable Organizer", "Desk Hook"]
    );
    let dragon = &p.list[0];
    assert_eq!((dragon.projects, dragon.parts, dragon.gcodes), (1, 10, 1));
    // The 3MF's own picture is the cover; the pictures got small cached thumbnails.
    assert!(matches!(dragon.cover, Some(Cover::Model(m)) if s.models[m].kind == Kind::ThreeMf));
    let cover = s
        .extras
        .iter()
        .find(|x| x.path.ends_with("images/cover.png"))
        .unwrap();
    assert!(cover.thumbnail.as_ref().unwrap().starts_with(data.path()));
    let photo = s
        .extras
        .iter()
        .find(|x| x.path.ends_with("photo.jpg"))
        .unwrap();
    assert!(photo.thumbnail.as_ref().unwrap().exists());
    assert_eq!(
        s.extras
            .iter()
            .filter(|x| x.kind == ExtraKind::Document)
            .count(),
        3,
        "README, LICENSE and the synthetic library's readme"
    );
    // The loose copy of the dragon's head is a location of a project model.
    let head = s
        .models
        .iter()
        .position(|m| {
            m.locations
                .iter()
                .any(|l| l.path.ends_with("Toys/dragon head.stl"))
        })
        .unwrap();
    assert_eq!(s.models[head].locations.len(), 2);
    assert_eq!(p.of_model[head], [0]);
    // The user's choices, kept across a rescan and a restart.
    let dragon_dir = dragon.folder.clone();
    let mixed = lib.path().join("Mixed/Toys");
    l.set_grouping(&dragon_dir, Some(Grouping::Split));
    l.set_grouping(&mixed, Some(Grouping::Project));
    l.rescan(None);
    assert!(l.wait_idle(T));
    let names = |s: &crate::Snapshot| -> Vec<String> {
        projects(s).list.iter().map(|p| p.name.clone()).collect()
    };
    assert_eq!(
        names(&l.snapshot()),
        ["Cable Organizer", "Desk Hook", "Toys"]
    );
    drop(l);
    let l = Library::open(data.path(), quiet()).unwrap();
    assert!(l.wait_idle(T));
    assert_eq!(
        names(&l.snapshot()),
        ["Cable Organizer", "Desk Hook", "Toys"]
    );
    l.set_grouping(&dragon_dir, None);
    assert_eq!(
        names(&l.snapshot()),
        ["Articulated Dragon", "Cable Organizer", "Desk Hook", "Toys"]
    );
    // A picture removed is forgotten with its cached thumbnail.
    let thumb = cover.thumbnail.clone().unwrap();
    std::fs::remove_file(&cover.path).unwrap();
    l.rescan(None);
    assert!(l.wait_idle(T));
    assert!(!l.snapshot().extras.iter().any(|x| x.path == cover.path));
    assert!(!thumb.exists());
    drop(l);
    // Only the picture this test removed changed in the library folder.
    let after = listing(lib.path());
    assert_eq!(after.len(), before.len() - 1);
}

/// The rendered thumbnails' files, by model id.
fn rendered(s: &crate::Snapshot) -> Vec<(String, Vec<u8>)> {
    let mut out: Vec<(String, Vec<u8>)> = s
        .models
        .iter()
        .filter(|m| m.info.thumbnail_rendered)
        .map(|m| {
            (
                m.id.clone(),
                std::fs::read(m.thumbnail.as_ref().unwrap()).unwrap(),
            )
        })
        .collect();
    out.sort();
    out
}

#[test]
fn thumbnails_render_in_the_background_once_per_content() {
    let lib = TempDir::new("thumbs");
    let data = TempDir::new("thumbs-data");
    synthetic_library(lib.path(), 40).unwrap();
    let before = listing(lib.path());
    let l = Library::open(data.path(), quiet()).unwrap();
    l.add_folder(lib.path()).unwrap();
    assert!(l.wait_idle(T));
    let s = l.snapshot();
    assert_eq!(s.pending_thumbnails, 0);
    for m in &s.models {
        match m.kind {
            // Every mesh without an embedded picture got one, in the data folder.
            Kind::Stl | Kind::Obj | Kind::ThreeMf => {
                let t = m.thumbnail.as_ref().unwrap_or_else(|| panic!("{}", m.name));
                assert!(t.starts_with(data.path()) && t.exists());
                assert!(m.info.thumbnail_rendered && !m.thumbnail_pending());
                assert_eq!(m.info.render_version, crate::render::RENDER_VERSION);
            }
            Kind::Step => assert!(m.thumbnail.is_none() && !m.thumbnail_pending()),
            Kind::Gcode | Kind::Bgcode => assert!(!m.info.thumbnail_rendered),
        }
    }
    // Duplicates are one model: one render each.
    let meshes = s
        .models
        .iter()
        .filter(|m| matches!(m.kind, Kind::Stl | Kind::Obj | Kind::ThreeMf))
        .count() as u64;
    assert_eq!(l.thumbnails_rendered(), meshes);
    // Nothing was written in the library folder.
    assert_eq!(listing(lib.path()), before);
    let first = rendered(&s);
    let stamps: Vec<_> = s
        .models
        .iter()
        .filter_map(|m| m.thumbnail.as_ref())
        .map(|p| std::fs::metadata(p).unwrap().modified().unwrap())
        .collect();
    drop(l);
    // Reopened: the cache is used, nothing is rendered again.
    let l = Library::open(data.path(), quiet()).unwrap();
    assert!(l.wait_idle(T));
    assert_eq!(l.thumbnails_rendered(), 0);
    let s = l.snapshot();
    assert_eq!(rendered(&s), first);
    let again: Vec<_> = s
        .models
        .iter()
        .filter_map(|m| m.thumbnail.as_ref())
        .map(|p| std::fs::metadata(p).unwrap().modified().unwrap())
        .collect();
    assert_eq!(again, stamps);
    drop(l);
    // Another data folder renders the same bytes (deterministic).
    let other = TempDir::new("thumbs-data2");
    let l = Library::open(other.path(), quiet()).unwrap();
    l.add_folder(lib.path()).unwrap();
    assert!(l.wait_idle(T));
    assert_eq!(rendered(&l.snapshot()), first);
    assert_eq!(listing(lib.path()), before);
}

#[test]
fn thumbnails_over_the_budget_keep_a_placeholder() {
    let lib = TempDir::new("budget");
    let data = TempDir::new("budget-data");
    copy("tests/models/3DBenchy.stl", &lib.path().join("benchy.stl"));
    copy("tests/models/cube20.stl", &lib.path().join("cube.stl"));
    let l = Library::open(
        data.path(),
        Options {
            render_budget: 1000,
            ..quiet()
        },
    )
    .unwrap();
    l.add_folder(lib.path()).unwrap();
    assert!(l.wait_idle(T));
    let s = l.snapshot();
    let benchy = s.models.iter().find(|m| m.name == "benchy.stl").unwrap();
    assert!(benchy.thumbnail.is_none() && !benchy.thumbnail_pending());
    assert!(
        benchy
            .info
            .no_render
            .as_deref()
            .is_some_and(|w| w.contains("too detailed"))
    );
    let cube = s.models.iter().find(|m| m.name == "cube.stl").unwrap();
    assert!(cube.thumbnail.is_some() && cube.info.no_render.is_none());
    drop(l);
    // Not tried again on the next start.
    let l = Library::open(data.path(), quiet()).unwrap();
    assert!(l.wait_idle(T));
    assert_eq!(l.thumbnails_rendered(), 0);
}

#[test]
fn thumbnails_wait_while_slicing_and_resume_after_a_restart() {
    let lib = TempDir::new("resume");
    let data = TempDir::new("resume-data");
    synthetic_library(lib.path(), 30).unwrap();
    let opts = Options {
        threads: 1,
        ..quiet()
    };
    let l = Library::open(data.path(), opts.clone()).unwrap();
    l.add_folder(lib.path()).unwrap();
    // Stop as soon as the scan is done and a first thumbnail is in.
    assert!(l.wait_for(T, |s| !s.scanning()
        && s.files > 0
        && s.models.iter().any(|m| m.info.thumbnail_rendered)));
    l.set_paused(true);
    // A render already running when the pause was set may still finish; wait for that.
    assert!(l.wait_paused(T));
    let paused_at = l.thumbnails_rendered();
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(
        l.thumbnails_rendered(),
        paused_at,
        "nothing renders while Mica slices"
    );
    assert!(!l.idle());
    let pending = l.snapshot().pending_thumbnails;
    drop(l);
    // Reopened: the rest is rendered, the done ones are not.
    let l = Library::open(data.path(), opts).unwrap();
    assert!(l.wait_idle(T));
    let s = l.snapshot();
    assert_eq!(s.pending_thumbnails, 0);
    assert!(l.thumbnails_rendered() as usize <= pending + 1);
    assert!(
        s.models
            .iter()
            .filter(|m| matches!(m.kind, Kind::Stl | Kind::Obj | Kind::ThreeMf))
            .all(|m| m.thumbnail.is_some())
    );
}

/// Run as a separate process by the test below: a first Mica that owns the index of
/// `MICA_TEST_LIBRARY_DATA`, has scanned `MICA_TEST_LIBRARY_DIR` and tagged a model, and
/// keeps the index until its input closes.
#[test]
fn first_process_holding_the_index() {
    let (Some(data), Some(dir)) = (
        std::env::var_os("MICA_TEST_LIBRARY_DATA"),
        std::env::var_os("MICA_TEST_LIBRARY_DIR"),
    ) else {
        return;
    };
    let l = Library::open(Path::new(&data), quiet()).unwrap();
    assert!(!l.read_only());
    l.add_folder(Path::new(&dir)).unwrap();
    assert!(l.wait_idle(T));
    let id = l.snapshot().models[0].id.clone();
    l.set_tags(&id, &["Kept".into()]);
    assert!(l.wait_idle(T));
    println!("ready {id}");
    let mut line = String::new();
    let _ = std::io::stdin().read_line(&mut line);
}

#[test]
fn a_second_process_browses_read_only_and_takes_the_index_over() {
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};
    let lib = TempDir::new("two-lib");
    let data = TempDir::new("two-data");
    synthetic_library(lib.path(), 6).unwrap();
    let mut first = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "tests::first_process_holding_the_index",
            "--nocapture",
        ])
        .env("MICA_TEST_LIBRARY_DATA", data.path())
        .env("MICA_TEST_LIBRARY_DIR", lib.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut out = BufReader::new(first.stdout.take().unwrap());
    let mut line = String::new();
    let id = loop {
        line.clear();
        assert!(out.read_line(&mut line).unwrap() > 0, "first process ended");
        if let Some(id) = line.trim().strip_prefix("ready ") {
            break id.to_string();
        }
    };
    let before = listing(data.path());
    let second = Library::open(
        data.path(),
        Options {
            takeover_check: Duration::from_millis(100),
            ..quiet()
        },
    )
    .unwrap();
    assert!(second.wait_idle(T));
    assert!(second.read_only());
    assert!(second.error().is_none() && second.warning().is_none());
    // Browsing and search work from what the first process committed.
    let s = second.snapshot();
    let models = s.models.len();
    assert!(models >= 6, "{models}");
    assert_eq!(s.folders.len(), 1);
    assert_eq!(s.folders[0].state, FolderState::Ready);
    let tagged = s.models.iter().find(|m| m.id == id).unwrap();
    assert_eq!(tagged.user.tags, vec!["Kept".to_string()]);
    assert_eq!(second.all_tags(), vec!["Kept".to_string()]);
    // Changes are refused or ignored; nothing in the data folder changes, nothing is set
    // aside, and no scan starts.
    assert_eq!(
        second.add_folder(lib.path()).unwrap_err(),
        crate::READ_ONLY_NOTE
    );
    second.set_favourite(&id, true);
    second.set_tags(&id, &["Lost".into()]);
    second.rescan(None);
    second.remove_folder(lib.path());
    assert!(second.wait_idle(T));
    std::thread::sleep(Duration::from_millis(600));
    assert!(second.read_only());
    assert!(!second.snapshot().models.iter().any(|m| m.user.favourite));
    assert_eq!(second.snapshot().folders.len(), 1);
    assert_eq!(listing(data.path()), before);
    // The first process quits: the second takes the index over by itself, keeps the tag and
    // can change things again.
    drop(first.stdin.take());
    assert!(first.wait().unwrap().success());
    let start = std::time::Instant::now();
    while second.read_only() && start.elapsed() < T {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!second.read_only());
    assert!(second.wait_idle(T));
    let s = second.snapshot();
    assert_eq!(s.models.len(), models);
    assert_eq!(
        s.models.iter().find(|m| m.id == id).unwrap().user.tags,
        vec!["Kept".to_string()]
    );
    second.set_favourite(&id, true);
    assert!(second.wait_idle(T));
    drop(second);
    let (_, index, warning) = crate::store::Store::open(&data.path().join("index.redb")).unwrap();
    assert!(warning.is_none());
    assert!(index.user[&id].favourite);
    assert!(
        !std::fs::read_dir(data.path()).unwrap().any(|e| e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains("damaged"))
    );
}

#[test]
fn a_second_handle_in_one_process_is_read_only_too() {
    let lib = TempDir::new("two-handles-lib");
    let data = TempDir::new("two-handles-data");
    synthetic_library(lib.path(), 3).unwrap();
    let first = Library::open(data.path(), quiet()).unwrap();
    first.add_folder(lib.path()).unwrap();
    assert!(first.wait_idle(T));
    let second = Library::open(
        data.path(),
        Options {
            takeover_check: Duration::from_millis(50),
            ..quiet()
        },
    )
    .unwrap();
    assert!(second.wait_idle(T));
    assert!(second.read_only());
    assert_eq!(
        second.snapshot().models.len(),
        first.snapshot().models.len()
    );
    drop(first);
    assert!(second.wait_for(T, |_| true));
    let start = std::time::Instant::now();
    while second.read_only() && start.elapsed() < T {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!second.read_only());
    assert!(second.wait_idle(T));
    assert_eq!(second.snapshot().models.len(), 3);
}
