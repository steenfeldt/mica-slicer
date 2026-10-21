// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The Filament card against the printer (color tolerance, names instead of hex codes,
//! ignore until the spool changes) and the per-printer G-code file header learned from
//! Moonraker's metadata, end to end against the mock printer.

use crate::app::{App, Options};
use crate::printhost::config::{FileHeader, Kind, Printer};
use crate::printhost::{Test, TestReport};
use eframe::egui;
use mica_printhost::{Capabilities, LoadedFilament, PrinterState, U1Slot};
use mica_profiles::user::Store;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

static NEXT: AtomicU64 = AtomicU64::new(0);

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mica-printer-filament-{tag}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn app(ctx: &egui::Context) -> App {
    App::create(
        ctx,
        Options {
            prefs_path: None,
            store: Some(Store::open(temp_dir("store"))),
            files: vec![],
            headless: true,
        },
    )
}

/// The app on the Snapmaker U1 (four tools), when its profiles are installed.
/// An app on the Snapmaker U1; `None` (a reported skip) without the pinned Orca profiles.
fn u1_app(ctx: &egui::Context) -> Option<App> {
    if !mica_testkit::require_orca_profiles() {
        return None;
    }
    let mut app = app(ctx);
    let (vi, _, name) = app
        .profiles
        .all_printers()
        .into_iter()
        .find(|(_, _, n)| n == "Snapmaker U1 (0.4 nozzle)")
        .expect("the Snapmaker U1 among the loaded printers");
    app.profiles.select_printer(vi, &name);
    app.settings_changed();
    Some(app)
}

fn hex(s: &str) -> [u8; 3] {
    mica_printhost::u1_mapping::parse_hex(s).unwrap()
}

/// A real U1 (2026-10-20): what is loaded, and what its project says.
const LOADED: [&str; 4] = ["F8F81C", "FFFFFF", "6F4C2F", "000000"];
const PROJECT: [&str; 4] = ["F5D020", "F5F5F5", "7B4A2A", "1A1A1A"];

/// A U1 linked to the current profile that reports `loaded` (material, color) per slot.
fn link_u1(app: &mut App, loaded: &[(&str, &str)]) -> u64 {
    let mut p = Printer::new(Kind::SnapmakerU1);
    p.name = "U1".into();
    p.address = "127.0.0.1:1".into();
    p.profile = Some(format!(
        "{}/{}",
        app.profiles.vendor_name(),
        app.profiles.printer
    ));
    let i = app.printers.put(None, p, None).unwrap();
    report(app, loaded);
    assert_eq!(app.linked_printer(), Some(i));
    app.printers.list[i].id
}

/// The printer's answer (as a finished test connection).
fn report(app: &mut App, loaded: &[(&str, &str)]) {
    app.printers.tests.insert(
        "U1".into(),
        Test::Done(TestReport {
            model: Some("Snapmaker U1".into()),
            firmware: None,
            state: PrinterState::Idle,
            tools: Some(vec![0, 1, 2, 3]),
            capabilities: Capabilities::default(),
            filaments: loaded
                .iter()
                .enumerate()
                .map(|(t, (m, c))| LoadedFilament {
                    tool: t as u32,
                    material: Some((*m).into()),
                    colour: Some(format!("#{c}FF")),
                })
                .collect(),
            camera: false,
            cameras: vec![],
        }),
    );
}

fn set_project_colours(app: &mut App, colours: &[&str]) {
    for (t, c) in colours.iter().enumerate() {
        app.overlays.set_colour(t, Some(format!("#{c}")));
    }
}

/// `#RRGGBB` or one of the raw input codes anywhere in `text`.
fn has_hex(text: &str) -> bool {
    let b = text.as_bytes();
    let hash = (0..b.len()).any(|i| {
        b[i] == b'#' && b.len() >= i + 7 && b[i + 1..i + 7].iter().all(u8::is_ascii_hexdigit)
    });
    hash || LOADED
        .iter()
        .chain(PROJECT.iter())
        .any(|h| text.to_ascii_uppercase().contains(h))
}

#[test]
fn loaded_spools_only_flag_the_real_color_difference() {
    let ctx = egui::Context::default();
    let Some(mut app) = u1_app(&ctx) else {
        return;
    };
    let loaded: Vec<(&str, &str)> = LOADED.iter().map(|c| ("PLA", *c)).collect();
    link_u1(&mut app, &loaded);
    set_project_colours(&mut app, &PROJECT);
    let rows: Vec<_> = (0..4).map(|t| app.printer_filament(t).unwrap()).collect();
    // White/off-white, brown/brown and black/near-black are the same spool colors.
    assert_eq!(
        rows.iter().map(|r| r.differs()).collect::<Vec<_>>(),
        [true, false, false, false]
    );
    assert_eq!(
        rows[0].lines(),
        ["Printer has yellow loaded in slot 1 (project: gold)"]
    );
    assert_eq!(
        crate::panels::sync::summary_lines(&rows),
        [("1 color differs from the printer".to_string(), false)]
    );
    // "Use printer's color" takes the printer's yellow for that tool only.
    app.use_printer_colour(0);
    assert_eq!(app.tool_colours()[0], hex("F8F81C"));
    assert_eq!(app.tool_colours()[1], hex("F5F5F5"));
    assert!(!app.printer_filament(0).unwrap().differs());
}

#[test]
fn type_mismatches_warn_and_ignore_lasts_until_the_spool_changes() {
    let ctx = egui::Context::default();
    let Some(mut app) = u1_app(&ctx) else {
        return;
    };
    // Slot 2 holds PETG, slot 3 a red spool where the project has blue.
    link_u1(
        &mut app,
        &[
            ("PLA", "F8F81C"),
            ("PETG", "FFFFFF"),
            ("PLA", "D81E1E"),
            ("PLA", "000000"),
        ],
    );
    set_project_colours(&mut app, &["F8F81C", "FFFFFF", "1E5AD0", "000000"]);
    let rows: Vec<_> = (0..4).map(|t| app.printer_filament(t).unwrap()).collect();
    assert!(rows[1].type_differs && !rows[1].colour_differs);
    assert_eq!(
        rows[1].lines(),
        ["Printer has PETG loaded in slot 2 (project: PLA)"]
    );
    assert_eq!(
        rows[2].lines(),
        ["Printer has red loaded in slot 3 (project: blue)"]
    );
    assert_eq!(
        crate::panels::sync::summary_lines(&rows),
        [
            ("1 filament type differs from the printer".to_string(), true),
            ("1 color differs from the printer".to_string(), false)
        ]
    );
    // Ignored: gone until the printer reports another spool in that slot.
    app.ignore_filament_difference(2);
    assert!(!app.printer_filament(2).unwrap().differs());
    report(
        &mut app,
        &[
            ("PLA", "F8F81C"),
            ("PETG", "FFFFFF"),
            ("PLA", "FF8000"),
            ("PLA", "000000"),
        ],
    );
    let f = app.printer_filament(2).unwrap();
    assert!(f.differs());
    assert_eq!(
        f.lines(),
        ["Printer has orange loaded in slot 3 (project: blue)"]
    );
}

/// No user-visible text of the Filament card, the popovers, the print dialog's slot
/// dropdowns and filament rows or the Machine view tiles shows a hex code (tooltips may).
#[test]
fn user_visible_filament_text_never_shows_hex_codes() {
    let mut texts: Vec<String> = vec![];
    let ctx = egui::Context::default();
    if let Some(mut app) = u1_app(&ctx) {
        let loaded: Vec<(&str, &str)> = LOADED.iter().map(|c| ("PLA", *c)).collect();
        link_u1(&mut app, &loaded);
        set_project_colours(&mut app, &PROJECT);
        app.overlays.set_colour(2, Some("#1E5AD0".into()));
        let rows: Vec<_> = (0..4).map(|t| app.printer_filament(t).unwrap()).collect();
        for r in &rows {
            texts.extend(r.lines());
            // The tooltip is where the codes live.
            assert!(r.tooltip().contains('#'), "{}", r.tooltip());
        }
        texts.extend(
            crate::panels::sync::summary_lines(&rows)
                .into_iter()
                .map(|(t, _)| t),
        );
    }
    let slots: Vec<U1Slot> = LOADED
        .iter()
        .enumerate()
        .map(|(i, c)| U1Slot {
            index: i as u32,
            exists: Some(true),
            material: Some("PLA".into()),
            subtype: Some("Basic".into()),
            vendor: Some("Snapmaker".into()),
            colour: Some(format!("{c}FF")),
        })
        .collect();
    for s in &slots {
        texts.push(crate::panels::print::slot_text(s));
    }
    assert_eq!(texts.last().unwrap(), "Slot 4 · PLA Basic · black");
    for (c, p) in LOADED.iter().zip(PROJECT) {
        texts.push(crate::printhost::explain::loaded_text(
            Some("PLA"),
            Some(hex(c)),
        ));
        texts.push(crate::panels::machine::tile_spool_text(
            Some("PLA"),
            Some(hex(p)),
        ));
    }
    // The print dialog's preflight against a printer with these spools.
    let info = mica_printhost::PrinterInfo {
        model: Some("Snapmaker U1".into()),
        firmware: None,
        state: PrinterState::Idle,
        tools: Some(vec![0, 1, 2, 3]),
        formats: vec![mica_printhost::FileFormat::Gcode],
        bed_type: None,
    };
    let status = mica_printhost::PrinterStatus {
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
    };
    let loaded: Vec<LoadedFilament> = LOADED
        .iter()
        .enumerate()
        .map(|(t, c)| LoadedFilament {
            tool: t as u32,
            material: Some("PLA".into()),
            colour: Some(format!("#{c}")),
        })
        .collect();
    let job: Vec<crate::printhost::explain::JobTool> = PROJECT
        .iter()
        .enumerate()
        .map(|(t, c)| crate::printhost::explain::JobTool {
            tool: t as u32,
            preset: "Snapmaker PLA".into(),
            material: Some("PLA".into()),
            colour: Some(hex(c)),
        })
        .collect();
    let pre = crate::printhost::explain::present(
        &info,
        &status,
        &loaded,
        &job,
        mica_printhost::FileFormat::Gcode,
        &[],
    );
    // Only yellow against gold differs.
    assert_eq!(
        pre.rows
            .iter()
            .map(|r| r.verdict == crate::printhost::explain::Verdict::Differs)
            .collect::<Vec<_>>(),
        [true, false, false, false]
    );
    texts.extend(pre.warnings.iter().cloned());
    texts.push(pre.tools_text.clone());
    assert!(texts.len() >= 12);
    for t in &texts {
        assert!(!has_hex(t), "hex code in user-visible text: {t:?}");
    }
}

/// The mock printers on free loopback ports, with `extra` flags; killed on drop.
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
fn mock_u1(extra: &[&str]) -> Option<Mock> {
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
    let child = std::process::Command::new(bin)
        .args(["--xl", "off", "--coreone", "off", "--u1", &port.to_string()])
        .args([
            "--rtsp",
            "off",
            "--ipcam",
            "off",
            "--camera-streamer",
            "off",
        ])
        .arg("--u1-idle")
        .args(extra)
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

/// Uploads a file with `header` as its identity line to the printer `printer` (upload only,
/// no start) and returns what the printer's metadata said about it.
fn upload(app: &mut App, printer: usize, header: &str) -> crate::printhost::job::SlicerReport {
    use crate::printhost::job::{Dialog, JobFile, Stage};
    let path = temp_dir("upload").join("part.gcode");
    std::fs::write(&path, format!("{header}\nG28\nG1 X10 Y10\n")).unwrap();
    let mut d = Dialog::new(
        JobFile {
            path,
            format: mica_printhost::FileFormat::Gcode,
            tools: vec![],
            bed_type: None,
            hold: None,
        },
        "part".into(),
    );
    d.choose(app.printers.target(printer));
    d.start_after = false;
    d.identity_reports = Some(app.identity_reports.0.clone());
    // Straight to the upload (the dialog's check is covered elsewhere).
    d.stage = Stage::Failed(crate::printhost::explain::Explanation {
        headline: String::new(),
        advice: String::new(),
    });
    d.upload(app.printers.connector(printer), || {});
    let deadline = Instant::now() + Duration::from_secs(20);
    while d.busy() && Instant::now() < deadline {
        d.poll();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(matches!(d.stage, Stage::Done { .. }), "{:?}", d.stage);
    app.identity_reports
        .1
        .recv_timeout(Duration::from_secs(10))
        .expect("the printer's metadata about the upload")
}

const MICA: &str = "; generated by Mica Slicer 0.0.1 on 1970-01-01 at 00:00:00";
const ORCA: &str = "; generated by Mica Slicer 0.0.1 (OrcaSlicer 2.4.2 on 1970-01-01 at 00:00:00)";

fn moonraker_app(ctx: &egui::Context, port: u16) -> (App, usize) {
    crate::printhost::restrict_to_loopback();
    let mut app = u1_app(ctx).unwrap_or_else(|| app(ctx));
    let mut p = Printer::new(Kind::SnapmakerU1);
    p.name = "Mock U1".into();
    p.address = format!("127.0.0.1:{port}");
    p.profile = Some(format!(
        "{}/{}",
        app.profiles.vendor_name(),
        app.profiles.printer
    ));
    let i = app.printers.put(None, p, None).unwrap();
    (app, i)
}

#[test]
fn legacy_moonraker_switches_the_printer_to_the_orca_compatible_header() {
    let Some(m) = mock_u1(&["--moonraker-legacy-slicers"]) else {
        mica_testkit::require(
            false,
            "the mock_printer example is not built (or loopback is unavailable)",
            "cargo test --workspace builds it",
        );
        return;
    };
    let ctx = egui::Context::default();
    let (mut app, i) = moonraker_app(&ctx, m.port);
    let u1_profile = app.profiles.printer == "Snapmaker U1 (0.4 nozzle)";
    // The U1 profile's own default is the Orca-compatible header; say Mica explicitly in
    // the slice configuration, as a profile with the `mica` default would.
    app.overlays
        .printer
        .insert("mica_gcode_identity".into(), "mica".into());
    app.settings_changed();
    let r = upload(&mut app, i, MICA);
    assert_eq!((r.identity, r.slicer.as_str()), ("mica", "Unknown"));
    assert!(app.apply_slicer_report(&r));
    assert_eq!(
        app.printers.list[i].file_header_detected,
        Some(FileHeader::OrcaCompatible)
    );
    assert!(
        app.notifications
            .list
            .iter()
            .any(|n| n.text.contains(crate::printhost::file_header::SWITCHED)),
        "one notification"
    );
    if u1_profile {
        // Future exports: the slice configuration carries the printer's header now.
        assert_eq!(
            app.effective_config()
                .and_then(|c| c.string("mica_gcode_identity")),
            Some("orca_compatible")
        );
    }
    // Once is enough: the next report changes nothing, and the old firmware does recognise
    // the Orca-compatible header.
    assert!(!app.apply_slicer_report(&r));
    let r = upload(&mut app, i, ORCA);
    assert_eq!(r.slicer, "OrcaSlicer");
    assert!(!app.apply_slicer_report(&r));
    // A header chosen by hand is never overridden.
    app.printers.list[i].file_header = Some(FileHeader::Mica);
    app.printers.list[i].file_header_detected = None;
    let r = upload(&mut app, i, MICA);
    assert!(!app.apply_slicer_report(&r));
    assert_eq!(app.printers.list[i].file_header_detected, None);
    drop(m);
}

#[test]
fn modern_moonraker_keeps_mica_s_own_header() {
    let Some(m) = mock_u1(&["--moonraker-modern"]) else {
        mica_testkit::require(
            false,
            "the mock_printer example is not built (or loopback is unavailable)",
            "cargo test --workspace builds it",
        );
        return;
    };
    let ctx = egui::Context::default();
    let (mut app, i) = moonraker_app(&ctx, m.port);
    let r = upload(&mut app, i, MICA);
    assert_eq!((r.identity, r.slicer.as_str()), ("mica", "Mica Slicer"));
    assert!(!app.apply_slicer_report(&r));
    assert_eq!(app.printers.list[i].file_header_detected, None);
    assert_eq!(app.printers.list[i].gcode_identity(), None);
    drop(m);
}
