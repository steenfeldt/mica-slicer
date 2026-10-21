// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The read-only document mode in the app: opening over a plate, what stays locked, Save
//! copy…, closing back to the untouched scene, and sending to the mock printers.

use crate::app::{App, Options};
use crate::keymap::Act;
use crate::menu::Action;
use eframe::egui;
use mica_profiles::user::Store;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

static NEXT: AtomicU64 = AtomicU64::new(0);

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mica-gcode-app-{tag}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn app(ctx: &egui::Context, files: Vec<PathBuf>) -> App {
    App::create(
        ctx,
        Options {
            prefs_path: None,
            store: Some(Store::open(temp_dir("store"))),
            files,
            headless: true,
        },
    )
}

fn model(name: &str) -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/models/")).join(name)
}

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../mica-gcode/tests/fixtures/"
    ))
    .join(name)
}

/// Waits for the validator's answer.
fn settle(app: &mut App, ctx: &egui::Context) {
    let deadline = Instant::now() + Duration::from_secs(120);
    while (app.gcode_loading.is_some() || app.gcode.as_ref().is_some_and(|d| d.check_rx.is_some()))
        && Instant::now() < deadline
    {
        app.poll(ctx);
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn a_gcode_file_opens_read_only_over_the_plate_and_closes_back_to_it() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx, vec![model("cube20.stl")]);
    assert_eq!(app.plates.current().objects.len(), 1);
    let objects = app.plates.current().objects.clone();
    let modified = app.is_modified();
    let title = app.window_title();
    let path = fixture("cube20_ps.gcode");
    let bytes = std::fs::read(&path).unwrap();
    // Opening goes through the same path as File › Open and drag and drop (on a thread).
    app.load(vec![path.clone()], &ctx);
    assert!(app.gcode_loading.is_some());
    assert!(app.busy());
    settle(&mut app, &ctx);
    assert!(app.viewing_gcode(), "{}", app.status);
    assert!(app.previewing());
    assert!(!app.machine && !app.library.shown);
    let doc = app.gcode.as_ref().unwrap();
    assert!(
        matches!(doc.check, crate::gcode_doc::Check::Done { .. }),
        "{:?}",
        doc.check
    );
    // Preview shows the file, the title names it, the scene is untouched underneath.
    let f = app.finished().unwrap();
    assert!(std::ptr::eq(f, &app.gcode.as_ref().unwrap().finished));
    assert_eq!(f.preview.layers.len(), 100);
    assert_eq!(app.view.high, 99);
    assert_eq!(
        app.window_title(),
        "cube20_ps.gcode (read-only) — Mica Slicer"
    );
    assert_eq!(app.plates.current().objects, objects);
    // Nothing slices or edits: Slice, Delete, Undo and the editing menu items are refused.
    app.slice_current(&ctx);
    assert!(app.worker.is_none());
    assert!(
        app.status.contains("Close the G-code file"),
        "{}",
        app.status
    );
    app.selected = Some(0);
    assert_eq!(app.run_key(Act::Delete, &ctx), None);
    assert_eq!(app.run_key(Act::Undo, &ctx), None);
    assert_eq!(app.plates.current().objects, objects);
    assert!(app.status.contains("read-only"), "{}", app.status);
    // View keys still work.
    assert_eq!(app.run_key(Act::ZoomIn, &ctx), Some(Act::ZoomIn));
    assert!(!app.menu_state(Action::Act(Act::Slice), false).enabled);
    assert!(!app.menu_state(Action::Act(Act::Delete), false).enabled);
    let export = app.menu_state(Action::Act(Act::ExportGcode), false);
    assert!(export.enabled);
    assert_eq!(export.label.as_deref(), Some("Save copy…"));
    assert!(app.menu_state(Action::Act(Act::Print), false).enabled);
    // The legend and viewport use the file's colours.
    assert_eq!(
        app.tool_colors_f32()[0],
        crate::app::display_colour([1., 128. / 255., 0.])
    );
    // Save copy…: the same bytes elsewhere; never onto the file itself.
    let copy = temp_dir("copy").join("copy.gcode");
    app.copy_gcode(&path, &copy, &ctx).unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    while app.loading.is_some() && Instant::now() < deadline {
        app.poll(&ctx);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(std::fs::read(&copy).unwrap(), bytes);
    let refused = app.copy_gcode(&path, &path, &ctx).unwrap_err();
    assert!(refused.contains("opened file itself"), "{refused}");
    // Close: back to the plate exactly as it was.
    app.close_gcode();
    assert!(!app.viewing_gcode());
    assert!(!app.previewing());
    assert_eq!(app.plates.current().objects, objects);
    assert_eq!(app.is_modified(), modified);
    assert_eq!(app.window_title(), title);
    assert!(app.finished().is_none());
    // The file was only read.
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
}

#[test]
fn opening_a_model_closes_the_gcode_file_and_garbage_shows_an_error() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx, vec![fixture("cube20_ps.bgcode")]);
    assert!(app.viewing_gcode(), "{}", app.status);
    assert!(app.gcode.as_ref().unwrap().finished.binary);
    // A model opens on the plate again.
    app.load(vec![model("cube20.stl")], &ctx);
    assert!(!app.viewing_gcode());
    let deadline = Instant::now() + Duration::from_secs(60);
    while app.loading.is_some() && Instant::now() < deadline {
        app.poll(&ctx);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(app.plates.current().objects.len(), 1);
    // Not G-code: an error notification, nothing opened, nothing changed.
    let bad = temp_dir("bad").join("letter.gcode");
    std::fs::write(&bad, "Dear printer,\nplease print this.\n".repeat(100)).unwrap();
    app.load(vec![bad.clone()], &ctx);
    settle(&mut app, &ctx);
    assert!(!app.viewing_gcode());
    let shown: Vec<String> = app
        .notifications
        .visible(None)
        .iter()
        .map(|n| format!("{} {}", n.title, n.text))
        .collect();
    assert!(
        shown
            .iter()
            .any(|t| t.contains("letter.gcode") && t.contains("not a G-code file")),
        "{shown:?}"
    );
    assert_eq!(app.plates.current().objects.len(), 1);
}

/// The mock printers (`crates/mica-printhost/examples/mock_printer.rs`, built with the
/// workspace's tests) on free loopback ports; killed on drop.
struct Mock {
    child: std::process::Child,
    ports: [u16; 3],
}
impl Drop for Mock {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
fn mock() -> Option<Mock> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let name = format!("mock_printer{}", std::env::consts::EXE_SUFFIX);
    let bin = [
        dir.join("examples").join(&name),
        dir.join("../examples").join(&name),
    ]
    .into_iter()
    .find(|p| p.exists())?;
    let free = || {
        std::net::TcpListener::bind("127.0.0.1:0")
            .ok()
            .and_then(|l| l.local_addr().ok())
            .map(|a| a.port())
    };
    let ports = [free()?, free()?, free()?];
    let child = std::process::Command::new(bin)
        .args(["--xl", &ports[0].to_string()])
        .args(["--coreone", &ports[1].to_string()])
        .args(["--u1", &ports[2].to_string()])
        .args(["--rtsp", "off", "--ipcam", "off", "--u1-idle", "--xl-idle"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let m = Mock { child, ports };
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if ports
            .iter()
            .all(|p| std::net::TcpStream::connect(("127.0.0.1", *p)).is_ok())
        {
            return Some(m);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    None
}

fn wait_dialog(app: &mut App) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while app.print_job.as_ref().is_some_and(|d| d.busy()) && Instant::now() < deadline {
        if let Some(d) = &mut app.print_job {
            d.poll();
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn send(app: &mut App, ctx: &egui::Context, expect_printer: &str) -> crate::printhost::job::Stage {
    app.open_print_dialog();
    let d = app.print_job.as_ref().expect("dialog");
    assert_eq!(
        d.target.as_ref().map(|t| t.name.as_str()),
        Some(expect_printer)
    );
    // The opened file itself goes out, never a re-export.
    assert_eq!(d.file.path, app.gcode.as_ref().unwrap().path);
    app.start_check(ctx);
    wait_dialog(app);
    app.request_upload(ctx);
    // A file with validation errors asks first (the synthetic U1 part has no heat-up).
    let errors = app.gcode.as_ref().unwrap().check.errors();
    assert_eq!(app.print_job.as_ref().unwrap().asking_errors, errors > 0);
    if errors > 0 {
        app.send_despite_errors(ctx);
    }
    wait_dialog(app);
    app.print_job.as_ref().unwrap().stage.clone()
}

#[test]
fn an_opened_file_is_sent_to_the_mock_printers_unchanged() {
    use crate::printhost::config::{Kind, Printer};
    use crate::printhost::job::Stage;
    crate::printhost::restrict_to_loopback();
    let Some(m) = mock() else {
        // `cargo test --workspace` builds the example; a crate-only run may not have it.
        mica_testkit::require(
            false,
            "the mock_printer example is not built (or loopback is unavailable)",
            "cargo test --workspace builds it",
        );
        return;
    };
    let ctx = egui::Context::default();
    let mut app = app(&ctx, vec![]);
    for (name, kind, port) in [
        ("Mock XL", Kind::PrusaLink, m.ports[0]),
        ("Mock CORE One", Kind::PrusaLink, m.ports[1]),
        ("Mock U1", Kind::SnapmakerU1, m.ports[2]),
    ] {
        let mut p = Printer::new(kind);
        p.name = name.into();
        p.address = format!("127.0.0.1:{port}");
        if name == "Mock CORE One" {
            p.model = Some("Prusa CORE One".into());
        }
        app.printers.put(None, p, None).unwrap();
    }
    // A U1 file (Orca feature names, four tools in the header, one used) goes to the U1 with
    // its slot map for the tool it prints with.
    let u1 = temp_dir("u1").join("u1_part.gcode");
    std::fs::write(&u1, super::tests::ORCA_U1).unwrap();
    let before = std::fs::read(&u1).unwrap();
    app.open_gcode_now(&u1).unwrap();
    settle(&mut app, &ctx);
    let stage = send(&mut app, &ctx, "Mock U1");
    assert!(
        matches!(&stage, Stage::Done { job, started: true, .. } if job == "u1_part.gcode"),
        "{stage:?}"
    );
    let plan = app
        .print_job
        .as_ref()
        .unwrap()
        .u1
        .as_ref()
        .expect("slot map");
    assert_eq!(
        plan.rows.iter().map(|r| r.job.tool).collect::<Vec<_>>(),
        [0, 1],
        "the tools the file uses"
    );
    assert_eq!(std::fs::read(&u1).unwrap(), before);
    app.print_job = None;
    // A PrusaSlicer CORE One binary file goes to the CORE One (its reported model fits).
    app.open_gcode_now(&fixture("cube20_ps.bgcode")).unwrap();
    settle(&mut app, &ctx);
    let stage = send(&mut app, &ctx, "Mock CORE One");
    assert!(
        matches!(&stage, Stage::Done { job, .. } if job == "cube20_ps.bgcode"),
        "{stage:?}"
    );
    assert_eq!(
        app.gcode_mismatch(
            app.print_job.as_ref().unwrap().target.as_ref().unwrap().id,
            None
        ),
        None
    );
    app.print_job = None;
    // Errors ask before sending; "Send anyway" sends. A mismatched printer only warns.
    if let Some(doc) = &mut app.gcode {
        doc.check = super::Check::Done {
            errors: 2,
            warnings: 0,
            details: vec![],
            against: "test".into(),
        };
    }
    app.open_print_dialog();
    let xl = app.printers.list[0].clone();
    app.print_job
        .as_mut()
        .unwrap()
        .choose(app.printers.target(0));
    app.start_check(&ctx);
    wait_dialog(&mut app);
    let reported = app.print_job.as_ref().unwrap().reported_model.clone();
    let warning = app.gcode_mismatch(xl.id, reported.as_deref());
    assert!(
        warning.as_deref().is_some_and(|w| w.contains("Mock XL")),
        "{warning:?} ({reported:?})"
    );
    app.request_upload(&ctx);
    let d = app.print_job.as_ref().unwrap();
    assert!(d.asking_errors);
    assert!(
        matches!(d.stage, Stage::Checked(_)),
        "not sent yet: {:?}",
        d.stage
    );
    app.send_despite_errors(&ctx);
    wait_dialog(&mut app);
    let stage = app.print_job.as_ref().unwrap().stage.clone();
    assert!(matches!(stage, Stage::Done { .. }), "{stage:?}");
    drop(m);
}

#[test]
fn the_files_printer_resolves_to_installed_presets() {
    if !mica_testkit::require_prusa_profiles() {
        return;
    }
    let ctx = egui::Context::default();
    let app = app(&ctx, vec![]);
    assert!(
        !app.profiles.all_printers().is_empty(),
        "no printers loaded"
    );
    // A preset the file names.
    let origin = super::Origin {
        vendor: Some("Prusa"),
        preset: Some("Prusa CORE One HF0.4 nozzle".into()),
        model: Some("COREONE".into()),
        nozzle: Some("HF0.4".into()),
        ..Default::default()
    };
    let r = app.resolve_gcode_printer(&origin);
    assert_eq!(
        r.preset.as_ref().map(|(_, n)| n.as_str()),
        Some("Prusa CORE One HF0.4 nozzle")
    );
    assert_eq!(r.model_name.as_deref(), Some("Prusa CORE One"));
    // Only a model code and the nozzle check (Mica's text output, `M862`): the preset whose
    // printer_model and variant match.
    let origin = super::Origin {
        vendor: Some("Prusa"),
        model: Some("XL5IS".into()),
        nozzle: Some("0.4".into()),
        ..Default::default()
    };
    let r = app.resolve_gcode_printer(&origin);
    let name = r.preset.map(|(_, n)| n).unwrap_or_default();
    assert!(
        name.contains("XL") && name.contains("5T") && name.contains("0.4"),
        "{name}"
    );
    // A Snapmaker U1 without a nozzle: the 0.4 mm one.
    if app.profiles.vendor_index("Snapmaker").is_some() {
        let origin = super::Origin {
            vendor: Some("Snapmaker"),
            model: Some("Snapmaker U1".into()),
            ..Default::default()
        };
        let r = app.resolve_gcode_printer(&origin);
        assert_eq!(
            r.preset.map(|(_, n)| n).as_deref(),
            Some("Snapmaker U1 (0.4 nozzle)")
        );
    }
}
