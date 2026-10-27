// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Several plates at once, end to end in the headless editor: Slice all plates skips plates
//! already sliced (or cached) and empty ones, goes on after a failed plate and cancels as one;
//! each plate's status and the totals; Export all plates writes one file per plate with the
//! single export's names and bytes (and asks before replacing); the Print dialog's All plates
//! sends every file (Prusa Connect: the queue; a Moonraker mock printer: uploads, the first
//! started), with retry and skip after a failure.

use crate::app::{App, Options};
use crate::plate_batch::{ExportStage, PlateStatus, Totals};
use crate::plate_menu::PlateCommand;
use crate::printhost::config::Kind;
use crate::printhost::job::{BatchItem, Dialog, ItemState, JobFile, Plates, Stage, Target};
use crate::printhost::monitor::Connector;
use eframe::egui;
use mica_printhost::{
    Capabilities, Error, FileFormat, Job, PrinterHost, PrinterInfo, PrinterState, PrinterStatus,
    ProgressCallback, UploadAction, UploadOutcome,
};
use mica_profiles::user::Store;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

static NEXT: AtomicU64 = AtomicU64::new(0);

fn model(name: &str) -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/models/")).join(name)
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mica-plate-batch-{tag}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The default printer (CORE One) with cube20 on plate 1 and `copies` more plates holding a
/// copy of it, plate 1 current.
fn editor(ctx: &egui::Context, copies: usize) -> App {
    let mut app = App::create(
        ctx,
        Options {
            prefs_path: None,
            store: Some(Store::open(temp_dir("store"))),
            files: vec![],
            headless: true,
        },
    );
    let result = crate::app::load_files(vec![model("cube20.stl")], &app.config, app.bed.center());
    app.finish_load(result);
    for _ in 0..copies {
        app.plate_command(0, PlateCommand::Duplicate);
    }
    app.select_plate(0);
    app
}

fn add_empty_plate(app: &mut App, ctx: &egui::Context) {
    app.handle(crate::viewport::Action::AddPlate, &[], ctx);
    app.select_plate(0);
}

/// Polls until no slice runs and no batch waits.
fn wait(app: &mut App, ctx: &egui::Context) {
    let deadline = Instant::now() + Duration::from_secs(300);
    while (app.busy() || app.slice_batch.is_some()) && Instant::now() < deadline {
        app.poll(ctx);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(!app.busy() && app.slice_batch.is_none(), "{}", app.status);
}

fn sliced(s: &PlateStatus) -> bool {
    matches!(s, PlateStatus::Sliced { .. })
}

#[test]
fn slice_all_skips_sliced_cached_and_empty_plates() {
    let ctx = egui::Context::default();
    let mut app = editor(&ctx, 1);
    add_empty_plate(&mut app, &ctx);
    assert_eq!(app.plates.len(), 3);
    assert!(
        app.plate_statuses()
            .iter()
            .zip([
                PlateStatus::NotSliced,
                PlateStatus::NotSliced,
                PlateStatus::Empty
            ])
            .all(|(a, b)| *a == b),
        "{:?}",
        app.plate_statuses()
    );
    // Plate 1 is sliced on its own first.
    app.slice_current(&ctx);
    wait(&mut app, &ctx);
    assert!(sliced(&app.plate_status(0)));
    // Locked plates are sliced too: the lock keeps the layout only.
    app.plate_command(1, PlateCommand::Lock);
    assert!(app.plates.plate(1).locked);
    app.select_plate(0);
    app.slice_all(&ctx);
    let batch = app.slice_batch.as_ref().expect("a batch");
    assert_eq!(
        batch.ids,
        [app.plates.plate(1).id],
        "only plate 2 needs a slice"
    );
    assert_eq!(app.batch_position(), Some((1, 1)));
    assert!(matches!(
        app.plate_status(1),
        PlateStatus::Slicing { .. } | PlateStatus::Queued
    ));
    wait(&mut app, &ctx);
    let statuses = app.plate_statuses();
    assert!(sliced(&statuses[0]) && sliced(&statuses[1]), "{statuses:?}");
    assert_eq!(statuses[2], PlateStatus::Empty);
    assert_eq!(app.status, "Sliced 1 plate");
    let totals = Totals::of(&statuses);
    assert_eq!((totals.sliced, totals.plates), (2, 2));
    assert_eq!(totals.note(), "all 2 sliced");
    let sum: f64 = (0..2)
        .map(|i| {
            app.plates
                .plate(i)
                .finished
                .as_ref()
                .unwrap()
                .summary
                .time_s
        })
        .sum();
    assert!((totals.time_s - sum).abs() < 1e-9);
    // Nothing left: no batch.
    app.slice_all(&ctx);
    assert!(app.slice_batch.is_none());
    assert_eq!(app.status, "Every plate is already sliced");
    // A setting changed and changed back: the cached slices come back, nothing is sliced.
    app.overlays
        .print
        .insert("fill_density".into(), "35%".into());
    app.settings_changed();
    assert_eq!(app.plate_status(0), PlateStatus::Outdated);
    app.overlays.print.remove("fill_density");
    app.settings_changed();
    app.slice_all(&ctx);
    assert!(
        app.slice_batch.is_none(),
        "cached plates are not sliced again"
    );
    assert!(app.plate_statuses()[..2].iter().all(sliced));
}

#[test]
fn slice_all_goes_on_after_a_failed_plate_and_cancels_as_one() {
    let ctx = egui::Context::default();
    let mut app = editor(&ctx, 2);
    // Plate 2's cube far off its bed: that plate fails, the others slice.
    app.plates.plates[1].objects[0].position.x += 2000.;
    app.changed();
    app.slice_all(&ctx);
    assert_eq!(app.slice_batch.as_ref().unwrap().ids.len(), 3);
    wait(&mut app, &ctx);
    let statuses = app.plate_statuses();
    assert!(sliced(&statuses[0]) && sliced(&statuses[2]), "{statuses:?}");
    match &statuses[1] {
        PlateStatus::Failed { message, .. } => assert!(!message.is_empty()),
        s => panic!("plate 2: {s:?}"),
    }
    assert!(statuses[1].text().starts_with("failed: "));
    assert!(app.status.contains("failed: Plate 2"), "{}", app.status);
    let totals = Totals::of(&statuses);
    assert_eq!(totals.note(), "2 of 3 sliced");
    // Cancel stops the running plate and the rest of the queue (a new setting: every plate
    // needs a new slice).
    app.plates.plates[1].objects[0].position.x -= 2000.;
    app.changed();
    app.overlays
        .print
        .insert("fill_density".into(), "35%".into());
    app.settings_changed();
    app.slice_all(&ctx);
    assert_eq!(app.slice_batch.as_ref().unwrap().ids.len(), 3);
    assert!(app.worker.is_some());
    app.cancel_slice();
    assert!(app.slice_batch.is_none() && app.slice_queue.is_empty());
    assert_eq!(app.status, "Canceled");
    for _ in 0..200 {
        app.poll(&ctx);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(app.worker.is_none(), "nothing started after Cancel");
    assert!(
        app.plate_statuses().iter().all(|s| !sliced(s)),
        "{:?}",
        app.plate_statuses()
    );
}

#[test]
fn export_all_writes_one_file_per_plate_named_and_equal_to_single_exports() {
    let ctx = egui::Context::default();
    let mut app = editor(&ctx, 2);
    add_empty_plate(&mut app, &ctx);
    app.rename_plate(1, "Gift / Tom");
    app.rename_plate(2, "Gift / Tom");
    // Plate 1 sliced before, the others are sliced by the export.
    app.slice_current(&ctx);
    wait(&mut app, &ctx);
    let dir = temp_dir("export");
    app.export_all_to(dir.clone(), &ctx);
    assert!(matches!(
        app.export_all.as_ref().unwrap().stage,
        ExportStage::Slicing
    ));
    let deadline = Instant::now() + Duration::from_secs(300);
    while app.export_all.as_ref().is_some_and(|e| e.stage.busy()) && Instant::now() < deadline {
        app.poll(&ctx);
        std::thread::sleep(Duration::from_millis(5));
    }
    let names = [
        "cube20_plate_1.bgcode",
        "cube20_gift___tom.bgcode",
        "cube20_gift___tom-2.bgcode",
    ];
    match &app.export_all.as_ref().unwrap().stage {
        ExportStage::Done { files, skipped } => {
            assert!(skipped.is_empty(), "{skipped:?}");
            let got: Vec<String> = files
                .iter()
                .map(|f| f.file_name().unwrap().to_string_lossy().into_owned())
                .collect();
            assert_eq!(got, names);
        }
        _ => panic!("not done: {}", app.status),
    }
    let mut on_disk: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    on_disk.sort();
    let mut want = names.map(String::from).to_vec();
    want.sort();
    assert_eq!(on_disk, want, "one file per plate with objects");
    // The bytes single Export writes for each plate (its slice's file).
    for (i, name) in names.iter().enumerate() {
        app.select_plate(i);
        assert!(app.export_stem() == name.trim_end_matches(".bgcode") || i == 2);
        let single = app
            .finished()
            .unwrap()
            .preview
            .job
            .path
            .join("print.bgcode");
        assert_eq!(
            std::fs::read(dir.join(name)).unwrap(),
            std::fs::read(single).unwrap(),
            "{name}"
        );
    }
    assert!(
        app.notifications
            .list
            .iter()
            .any(|n| n.title.starts_with("Exported 3 files to ")
                && n.actions
                    .iter()
                    .any(|(l, _)| l == crate::plate_batch::reveal_label())),
        "{:?}",
        app.notifications
            .list
            .iter()
            .map(|n| &n.title)
            .collect::<Vec<_>>()
    );
    // Again into the same folder: it asks first, nothing is written until Replace.
    app.export_all = None;
    std::fs::write(dir.join(names[0]), b"old").unwrap();
    app.export_all_to(dir.clone(), &ctx);
    match &app.export_all.as_ref().unwrap().stage {
        ExportStage::Confirm { existing } => assert_eq!(existing.len(), 3),
        _ => panic!("no question"),
    }
    assert_eq!(std::fs::read(dir.join(names[0])).unwrap(), b"old");
    app.export_all_start(&ctx);
    let deadline = Instant::now() + Duration::from_secs(60);
    while app
        .export_all
        .as_ref()
        .is_some_and(|e| !matches!(e.stage, ExportStage::Done { .. }))
        && Instant::now() < deadline
    {
        app.poll(&ctx);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_ne!(std::fs::read(dir.join(names[0])).unwrap(), b"old");
}

#[test]
fn export_all_cancel_writes_nothing() {
    let ctx = egui::Context::default();
    let mut app = editor(&ctx, 1);
    let dir = temp_dir("export-cancel");
    app.export_all_to(dir.clone(), &ctx);
    app.cancel_slice();
    assert!(matches!(
        app.export_all.as_ref().unwrap().stage,
        ExportStage::Canceled
    ));
    for _ in 0..100 {
        app.poll(&ctx);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
}

// ----- Send all plates -----------------------------------------------------------------------

/// A printer that records what it is asked to do and supports Prusa Connect's queue; the
/// upload of a file whose name contains `fail` fails while `failing` is set.
#[derive(Default)]
struct Recorder {
    calls: Mutex<Vec<(String, UploadAction)>>,
    failing: Mutex<Option<String>>,
}
struct RecorderHandle(Arc<Recorder>);
impl PrinterHost for RecorderHandle {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            upload: true,
            start: true,
            ..Capabilities::default()
        }
    }
    fn info(&self) -> mica_printhost::Result<PrinterInfo> {
        Ok(PrinterInfo {
            model: Some("Recorder".into()),
            firmware: None,
            state: PrinterState::Idle,
            tools: Some(vec![0]),
            formats: vec![FileFormat::Bgcode, FileFormat::Gcode],
            bed_type: None,
        })
    }
    fn upload_with_progress(
        &self,
        _: &Path,
        _: &str,
        _: bool,
        _: &mut ProgressCallback<'_>,
    ) -> mica_printhost::Result<Job> {
        unreachable!("the dialog uses upload_action_with_progress")
    }
    fn upload_action_with_progress(
        &self,
        path: &Path,
        remote: &str,
        action: UploadAction,
        progress: &mut ProgressCallback<'_>,
    ) -> mica_printhost::Result<UploadOutcome> {
        if let Some(f) = self.0.failing.lock().unwrap().as_ref()
            && remote.contains(f.as_str())
        {
            return Err(Error {
                kind: mica_printhost::ErrorKind::Protocol,
                message: "printer returned HTTP 500".into(),
                http_status: Some(500),
            });
        }
        let total = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        progress(mica_printhost::UploadProgress { sent: total, total });
        self.0
            .calls
            .lock()
            .unwrap()
            .push((remote.to_string(), action));
        Ok(UploadOutcome {
            job: Job {
                name: remote.into(),
                id: None,
                started: action == UploadAction::Print,
            },
            message: None,
            warning: false,
        })
    }
    fn start(&self, _: &str) -> mica_printhost::Result<()> {
        Ok(())
    }
    fn pause(&self) -> mica_printhost::Result<()> {
        Ok(())
    }
    fn resume(&self) -> mica_printhost::Result<()> {
        Ok(())
    }
    fn cancel(&self) -> mica_printhost::Result<()> {
        Ok(())
    }
    fn status(&self) -> mica_printhost::Result<PrinterStatus> {
        Ok(PrinterStatus {
            state: PrinterState::Idle,
            job_name: None,
            progress: None,
            elapsed: None,
            remaining: None,
            current_layer: None,
            z: None,
            hotends: vec![],
            bed: None,
            chamber: None,
        })
    }
}

fn job_file(dir: &Path, name: &str) -> JobFile {
    let path = dir.join(name);
    std::fs::write(&path, vec![7u8; 2048]).unwrap();
    JobFile {
        path,
        format: FileFormat::Bgcode,
        tools: vec![],
        bed_type: None,
        hold: None,
    }
}

/// A dialog with All plates chosen and three plates' files, on `kind`.
fn all_plates_dialog(kind: Kind) -> Dialog {
    let dir = temp_dir("send");
    let mut d = Dialog::new(job_file(&dir, "single.bgcode"), "single".into());
    d.plates = Some(Plates::new(3));
    d.choose(Some(Target {
        id: 1,
        name: "XL".into(),
        kind,
        address: "xl.local".into(),
    }));
    d.choose_all(true);
    assert!(d.plates.as_ref().unwrap().preparing);
    let items = ["cube_plate_1", "cube_plate_2_fail", "cube_plate_3"]
        .iter()
        .enumerate()
        .map(|(i, stem)| {
            BatchItem::new(
                format!("Plate {}", i + 1),
                job_file(&dir, &format!("{stem}.bgcode")),
                format!("{stem}.bgcode"),
            )
        })
        .collect();
    d.set_plates(items, vec![]);
    assert_eq!(
        d.file_name, "cube_plate_1.bgcode",
        "the first plate is checked"
    );
    d
}

fn settle(d: &mut Dialog) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while d.busy() && Instant::now() < deadline {
        d.poll();
        std::thread::sleep(Duration::from_millis(5));
    }
    d.poll();
}

fn recorder() -> (Arc<Recorder>, Connector) {
    let r = Arc::new(Recorder::default());
    let h = r.clone();
    (
        r,
        Arc::new(move || Ok(Box::new(RecorderHandle(h.clone())) as Box<dyn PrinterHost>)),
    )
}

#[test]
fn connect_queues_every_plate_and_optionally_prints_the_first() {
    let (rec, connect) = recorder();
    let mut d = all_plates_dialog(Kind::PrusaConnect);
    d.check(Ok(connect.clone()), || {});
    assert_eq!(d.stage, Stage::CloudReady);
    // Add all to the queue, in plate order.
    d.start_after = false;
    assert!(d.can_upload_all());
    d.upload_all(Ok(connect.clone()), || {});
    settle(&mut d);
    assert_eq!(
        *rec.calls.lock().unwrap(),
        [
            ("cube_plate_1.bgcode".to_string(), UploadAction::Queue),
            ("cube_plate_2_fail.bgcode".to_string(), UploadAction::Queue),
            ("cube_plate_3.bgcode".to_string(), UploadAction::Queue),
        ]
    );
    assert!(d.batch_complete());
    assert!(d.plates.as_ref().unwrap().items.iter().all(|i| matches!(
        i.state,
        ItemState::Done {
            queued: true,
            started: false,
            ..
        }
    )));
    // Print the first, queue the rest: the others go first, the printing one last.
    let mut d = all_plates_dialog(Kind::PrusaConnect);
    rec.calls.lock().unwrap().clear();
    d.check(Ok(connect.clone()), || {});
    d.start_after = true;
    d.upload_all(Ok(connect), || {});
    settle(&mut d);
    assert_eq!(
        *rec.calls.lock().unwrap(),
        [
            ("cube_plate_2_fail.bgcode".to_string(), UploadAction::Queue),
            ("cube_plate_3.bgcode".to_string(), UploadAction::Queue),
            ("cube_plate_1.bgcode".to_string(), UploadAction::Print),
        ]
    );
}

#[test]
fn a_failed_file_can_be_retried_or_skipped() {
    let (rec, connect) = recorder();
    *rec.failing.lock().unwrap() = Some("fail".into());
    let mut d = all_plates_dialog(Kind::PrusaLink);
    d.stage = Stage::Failed(crate::printhost::explain::Explanation {
        headline: String::new(),
        advice: String::new(),
    });
    d.start_after = true;
    d.upload_all(Ok(connect.clone()), || {});
    settle(&mut d);
    let p = d.plates.as_ref().unwrap();
    assert_eq!((p.done(), p.failed()), (2, 1));
    match &p.items[1].state {
        ItemState::Failed(x) => assert!(!x.headline.is_empty()),
        s => panic!("{s:?}"),
    }
    assert!(!d.batch_complete());
    // Retry sends only the failed one.
    *rec.failing.lock().unwrap() = None;
    rec.calls.lock().unwrap().clear();
    assert!(d.can_upload_all());
    d.upload_all(Ok(connect.clone()), || {});
    settle(&mut d);
    assert_eq!(
        *rec.calls.lock().unwrap(),
        [(
            "cube_plate_2_fail.bgcode".to_string(),
            UploadAction::UploadOnly
        )]
    );
    assert!(d.batch_complete());
    // Skip: a failure left out finishes the send.
    *rec.failing.lock().unwrap() = Some("fail".into());
    let mut d = all_plates_dialog(Kind::PrusaLink);
    d.stage = Stage::Failed(crate::printhost::explain::Explanation {
        headline: String::new(),
        advice: String::new(),
    });
    d.upload_all(Ok(connect), || {});
    settle(&mut d);
    assert!(!d.batch_complete());
    d.skip_failed();
    assert!(d.batch_complete());
    assert_eq!(
        d.plates.as_ref().unwrap().items[1].state,
        ItemState::Skipped
    );
}

/// The loopback mock printers (`mock_printer` example), killed on drop.
struct Mock {
    child: std::process::Child,
    port: u16,
}
impl Drop for Mock {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
/// The mock printers on a free loopback port: `printer` is `--coreone` (PrusaLink) or `--u1`
/// (Moonraker).
fn mock(printer: &str) -> Option<Mock> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let name = format!("mock_printer{}", std::env::consts::EXE_SUFFIX);
    let bin = [
        dir.join("examples").join(&name),
        dir.join("../examples").join(&name),
    ]
    .into_iter()
    .find(|p| p.exists())?;
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .ok()?
        .local_addr()
        .ok()?
        .port();
    let mut cmd = std::process::Command::new(bin);
    for flag in ["--xl", "--coreone", "--u1"] {
        if flag == printer {
            cmd.args([flag, &port.to_string()]);
        } else {
            cmd.args([flag, "off"]);
        }
    }
    let child = cmd
        .args([
            "--rtsp",
            "off",
            "--ipcam",
            "off",
            "--camera-streamer",
            "off",
        ])
        .args(["--u1-idle", "--xl-idle"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let m = Mock { child, port };
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return Some(m);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    None
}

/// Three plates with a cube on `printer` (a preset name; `None`: the default CORE One) sent
/// with All plates to a mock printer of `kind`, the first started: every file arrives, the
/// dialog closes with one notification.
fn send_all_case(printer: Option<&str>, flag: &str, kind: Kind, ext: &str) {
    crate::printhost::restrict_to_loopback();
    let Some(m) = mock(flag) else {
        mica_testkit::require(
            false,
            "the mock_printer example is not built (or loopback is unavailable)",
            "cargo test --workspace builds it",
        );
        return;
    };
    let ctx = egui::Context::default();
    if printer.is_some() && !mica_testkit::require_orca_profiles() {
        return;
    }
    let mut app = editor(&ctx, 0);
    if let Some(name) = printer {
        let (vendor, _, name) = app
            .profiles
            .all_printers()
            .into_iter()
            .find(|(_, _, n)| n == name)
            .unwrap_or_else(|| panic!("{name} is not among the loaded printers"));
        app.switch_printer(vendor, &name);
    }
    app.plate_command(0, PlateCommand::Duplicate);
    app.plate_command(0, PlateCommand::Duplicate);
    app.select_plate(0);
    let mut p = crate::printhost::config::Printer::new(kind);
    p.name = "Mock".into();
    p.address = format!("127.0.0.1:{}", m.port);
    app.printers.put(None, p, None).unwrap();
    // Only plate 1 is sliced when the dialog opens; All plates slices the others.
    app.slice_current(&ctx);
    wait(&mut app, &ctx);
    app.open_print_dialog();
    let d = app.print_job.as_mut().unwrap();
    assert_eq!(d.plates.as_ref().unwrap().count, 3);
    let single = d.file_name.clone();
    d.choose(app.printers.target(0));
    d.choose_all(true);
    app.prepare_send_all(&ctx);
    assert!(app.slice_batch.is_some());
    wait(&mut app, &ctx);
    let d = app.print_job.as_ref().unwrap();
    let pl = d.plates.as_ref().unwrap();
    assert!(!pl.preparing && pl.items.len() == 3, "{:?}", pl.problem);
    let remotes: Vec<String> = pl.items.iter().map(|i| i.remote.clone()).collect();
    assert_eq!(
        remotes,
        (1..=3)
            .map(|n| format!("cube20_plate_{n}.{ext}"))
            .collect::<Vec<_>>()
    );
    // The check (of the first plate's file), then all three, the first started.
    let deadline = Instant::now() + Duration::from_secs(30);
    app.start_check(&ctx);
    while app.print_job.as_ref().unwrap().busy() && Instant::now() < deadline {
        app.print_job.as_mut().unwrap().poll();
        std::thread::sleep(Duration::from_millis(5));
    }
    let d = app.print_job.as_mut().unwrap();
    assert!(matches!(d.stage, Stage::Checked(_)), "{:?}", d.stage);
    d.start_after = true;
    assert!(d.can_upload_all());
    app.start_upload_all(&ctx);
    let d = app.print_job.as_mut().unwrap();
    settle(d);
    let items = &d.plates.as_ref().unwrap().items;
    assert!(
        matches!(items[0].state, ItemState::Done { started: true, .. }),
        "{:?}",
        items[0].state
    );
    for i in &items[1..] {
        assert!(
            matches!(i.state, ItemState::Done { started: false, .. }),
            "{:?}",
            i.state
        );
    }
    assert!(d.batch_complete());
    if kind == Kind::Moonraker {
        // Moonraker indexes the uploads: the three files are on the printer.
        let h = (app.printers.connector(0).unwrap())().unwrap();
        for name in &remotes {
            let found = (0..20).any(|_| {
                let there = h.file_slicer(name).ok().flatten().is_some();
                if !there {
                    std::thread::sleep(Duration::from_millis(100));
                }
                there
            });
            assert!(found, "{name} is not on the mock printer");
        }
    }
    // The dialog closes with one notification.
    app.finish_upload_all();
    assert!(app.print_job.is_none() && !app.dialogs.print);
    assert!(
        app.notifications.list.iter().any(|n| n
            .title
            .starts_with("Uploaded 3 plates to Mock · Plate 1 printing")),
        "{:?}",
        app.notifications
            .list
            .iter()
            .map(|n| &n.title)
            .collect::<Vec<_>>()
    );
    // This plate / All plates switch back.
    app.open_print_dialog();
    let d = app.print_job.as_mut().unwrap();
    d.choose_all(true);
    d.choose_all(false);
    assert_eq!(d.file_name, single);
    app.print_job = None;
    drop(m);
}

#[test]
fn all_plates_go_to_a_prusalink_mock_printer_and_the_first_starts() {
    send_all_case(None, "--coreone", Kind::PrusaLink, "bgcode");
}

#[test]
fn all_plates_go_to_a_moonraker_mock_printer_and_the_first_starts() {
    send_all_case(
        Some("Snapmaker U1 (0.4 nozzle)"),
        "--u1",
        Kind::Moonraker,
        "gcode",
    );
}

// ----- Through the whole window --------------------------------------------------------------

fn frame(ctx: &egui::Context, app: &mut App, events: Vec<egui::Event>) {
    let input = egui::RawInput {
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

fn click(ctx: &egui::Context, app: &mut App, at: egui::Pos2) {
    frame(ctx, app, vec![egui::Event::PointerMoved(at)]);
    let ev = |pressed| egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    frame(ctx, app, vec![ev(true)]);
    frame(ctx, app, vec![ev(false)]);
    frame(ctx, app, vec![]);
}

#[test]
fn the_slice_button_s_menu_slices_all_plates() {
    let ctx = egui::Context::default();
    let mut app = editor(&ctx, 1);
    frame(&ctx, &mut app, vec![]);
    frame(&ctx, &mut app, vec![]);
    let chevron =
        crate::panels::topbar::probed(&ctx, "slice menu").expect("the Slice split button");
    click(&ctx, &mut app, chevron.center());
    let row = crate::panels::topbar::probed(&ctx, "Slice all plates").expect("its menu");
    assert!(crate::panels::topbar::probed(&ctx, "Slice plate").is_some());
    click(&ctx, &mut app, row.center());
    assert!(app.slice_batch.is_some(), "{}", app.status);
    assert_eq!(app.slice_batch.as_ref().unwrap().ids.len(), 2);
    wait(&mut app, &ctx);
    assert!(app.plate_statuses().iter().all(sliced));
}

#[test]
fn shift_cmd_r_slices_all_plates_and_the_plate_list_shows_it() {
    let ctx = egui::Context::default();
    let mut app = editor(&ctx, 1);
    add_empty_plate(&mut app, &ctx);
    frame(&ctx, &mut app, vec![]);
    let m = egui::Modifiers::COMMAND | egui::Modifiers::SHIFT;
    let ev = |pressed| egui::Event::Key {
        key: egui::Key::R,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers: m,
    };
    frame(
        &ctx,
        &mut app,
        vec![egui::Event::ModifiersChanged(m), ev(true)],
    );
    frame(
        &ctx,
        &mut app,
        vec![
            ev(false),
            egui::Event::ModifiersChanged(egui::Modifiers::NONE),
        ],
    );
    assert!(app.slice_batch.is_some(), "⇧⌘R starts Slice all plates");
    let key = crate::keymap::shortcut_text(crate::keymap::Act::SliceAll);
    assert!(key.contains("Shift") && key.ends_with('R'), "{key}");
    // While it runs: the bar's status chip, the list's rows.
    app.viewport.open_plate_list = true;
    frame(&ctx, &mut app, vec![]);
    frame(&ctx, &mut app, vec![]);
    let probe = app.viewport.plate_probe.clone();
    assert_eq!(probe.rows.len(), 3);
    assert!(probe.status.is_some(), "the plate bar shows the status");
    wait(&mut app, &ctx);
    // Slice all shows the Preview of the current plate when it is done; back to Prepare.
    assert!(app.preview);
    app.preview = false;
    app.viewport.open_plate_list = true;
    frame(&ctx, &mut app, vec![]);
    frame(&ctx, &mut app, vec![]);
    let totals = app
        .viewport
        .plate_probe
        .totals
        .clone()
        .expect("the list's totals");
    assert!(totals.ends_with(" · all 2 sliced"), "{totals}");
    let t = Totals::of(&app.plate_statuses());
    assert!(totals.starts_with(&t.text()), "{totals}");
    // The footer's Slice all is off: nothing to slice.
    let slice_all = app
        .viewport
        .plate_probe
        .slice_all
        .expect("Slice all plates");
    click(&ctx, &mut app, slice_all.center());
    assert!(app.slice_batch.is_none());
}

#[test]
fn statuses_read_without_color() {
    let failed = PlateStatus::Failed {
        short: "brim doesn't fit on the bed".into(),
        message: "The brim of cube20.stl would go 3 mm past the left edge".into(),
    };
    assert_eq!(failed.text(), "failed: brim doesn't fit on the bed");
    assert!(failed.tooltip().contains("3 mm past the left edge"));
    assert_eq!(PlateStatus::NotSliced.text(), "not sliced");
    assert_eq!(PlateStatus::Outdated.text(), "changed, slice again");
    assert_eq!(
        PlateStatus::Slicing { progress: 0.42 }.text(),
        "slicing 42 %"
    );
    let s = PlateStatus::Sliced {
        time_s: 38. * 60.,
        grams: 11.62,
        filament_mm: 3900.,
        summary: Default::default(),
    };
    assert_eq!(s.text(), "38 min · 11.6 g");
    let t = Totals::of(&[
        s.clone(),
        s,
        PlateStatus::NotSliced,
        PlateStatus::Outdated,
        PlateStatus::Empty,
    ]);
    assert_eq!(t.note(), "2 of 4 sliced");
    assert_eq!(t.text(), "1 h 16 min · 23.2 g · 7.80 m");
    assert_eq!(
        Totals::of(&[PlateStatus::NotSliced]).note(),
        "none sliced yet"
    );
}
