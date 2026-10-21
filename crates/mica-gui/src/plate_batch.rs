// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Several plates at once. **Slice all plates** (⇧⌘R) queues the plates that need a slice
//! (plates already up to date and empty plates are skipped; locked plates are sliced, as the
//! lock only keeps their layout) and shows "Slicing plate 2 of 4"; one Cancel stops the queue.
//! Each plate has a [`PlateStatus`] (the plate bar and the plate list show it with a mark and
//! words, never color alone) and [`Totals`] add up the sliced plates. **Export all plates…**
//! slices what is missing, then writes one file per plate into a folder, named as Export names
//! one plate's file (`mica_core::export_name`, as `mica slice --each-plate` does). The Print
//! dialog's "All plates" (`panels::print`) slices the same way and then sends the files.

use crate::app::App;
use crate::slice_cache::Fit;
use eframe::egui;
use std::path::PathBuf;
use std::sync::mpsc;

/// Where a plate's slice stands.
#[derive(Clone, Debug, PartialEq)]
pub enum PlateStatus {
    /// No objects: nothing to slice.
    Empty,
    NotSliced,
    /// Waiting in Slice all's queue.
    Queued,
    /// Being sliced; `progress` 0 ..= 1.
    Slicing {
        progress: f32,
    },
    Sliced {
        time_s: f64,
        grams: f64,
        filament_mm: f64,
    },
    /// Sliced before, changed since.
    Outdated,
    Failed {
        short: String,
        message: String,
    },
}

impl PlateStatus {
    /// A few words for the plate list and the plate bar.
    pub fn text(&self) -> String {
        match self {
            PlateStatus::Empty => "empty".into(),
            PlateStatus::NotSliced => "not sliced".into(),
            PlateStatus::Queued => "waiting to slice".into(),
            PlateStatus::Slicing { progress } => {
                format!("slicing {:.0} %", progress.clamp(0., 1.) * 100.)
            }
            PlateStatus::Sliced { time_s, grams, .. } => format!(
                "{} · {}",
                crate::panels::topbar::short_duration(*time_s),
                weight(*grams)
            ),
            PlateStatus::Outdated => "changed, slice again".into(),
            PlateStatus::Failed { short, .. } => format!("failed: {short}"),
        }
    }

    /// The hover text: what the state means, a failure's whole reason.
    pub fn tooltip(&self) -> String {
        match self {
            PlateStatus::Empty => "This plate has no objects".into(),
            PlateStatus::NotSliced => "Not sliced yet".into(),
            PlateStatus::Queued => "Slice all plates slices this plate next".into(),
            PlateStatus::Slicing { .. } => "Slicing this plate now".into(),
            PlateStatus::Sliced {
                time_s,
                grams,
                filament_mm,
            } => format!(
                "Sliced and up to date: {} · {} · {:.2} m of filament",
                crate::panels::duration(*time_s),
                weight(*grams),
                filament_mm / 1000.
            ),
            PlateStatus::Outdated => {
                "The plate changed since its last slice; slice it again for current totals".into()
            }
            PlateStatus::Failed { message, .. } => format!("The slice failed: {message}"),
        }
    }
}

fn weight(grams: f64) -> String {
    format!("{grams:.1} g")
}

/// Draws the status mark of `status` centered at `at` (about 12 px): an empty ring (not
/// sliced), three dots (waiting), a ring filling up (slicing), a check in a filled disc
/// (sliced), a half-filled ring (outdated), a cross in a red disc (failed). Shapes differ, so
/// the state reads without color.
pub fn status_mark(painter: &egui::Painter, at: egui::Pos2, status: &PlateStatus) {
    use egui::{Pos2, Stroke, vec2};
    let p = crate::theme::of_ctx(painter.ctx());
    let r = 5.5;
    match status {
        PlateStatus::Empty => {}
        PlateStatus::NotSliced => {
            painter.circle_stroke(at, r, Stroke::new(1.4, p.muted));
        }
        PlateStatus::Queued => {
            for dx in [-4., 0., 4.] {
                painter.circle_filled(at + vec2(dx, 0.), 1.4, p.muted_alt);
            }
        }
        PlateStatus::Slicing { progress } => {
            painter.circle_stroke(at, r, Stroke::new(1.6, p.field_border));
            let end = progress.clamp(0.02, 1.) * std::f32::consts::TAU;
            let points: Vec<Pos2> = (0..=24)
                .map(|k| {
                    let a = -std::f32::consts::FRAC_PI_2 + end * k as f32 / 24.;
                    at + vec2(a.cos(), a.sin()) * r
                })
                .collect();
            painter.add(egui::Shape::line(points, Stroke::new(2., p.accent_bright)));
        }
        PlateStatus::Sliced { .. } => {
            painter.circle_filled(at, r + 0.5, p.accent);
            painter.add(egui::Shape::line(
                vec![
                    at + vec2(-2.6, 0.2),
                    at + vec2(-0.6, 2.2),
                    at + vec2(2.8, -2.0),
                ],
                Stroke::new(1.6, p.on_accent),
            ));
        }
        PlateStatus::Outdated => {
            painter.circle_stroke(at, r, Stroke::new(1.4, crate::theme::WARNING));
            // The left half filled: "half there".
            let points: Vec<Pos2> = (0..=12)
                .map(|k| {
                    let a = std::f32::consts::FRAC_PI_2 + std::f32::consts::PI * k as f32 / 12.;
                    at + vec2(a.cos(), a.sin()) * r
                })
                .collect();
            painter.add(egui::Shape::convex_polygon(
                points,
                crate::theme::WARNING,
                Stroke::NONE,
            ));
        }
        PlateStatus::Failed { .. } => {
            painter.circle_filled(at, r + 0.5, crate::theme::DANGER);
            let s = 2.4;
            let stroke = Stroke::new(1.6, egui::Color32::WHITE);
            painter.line_segment([at + vec2(-s, -s), at + vec2(s, s)], stroke);
            painter.line_segment([at + vec2(-s, s), at + vec2(s, -s)], stroke);
        }
    }
}

/// Time and filament over the sliced plates, and how many of the plates that have objects
/// are sliced.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Totals {
    pub sliced: usize,
    /// Plates with something to slice.
    pub plates: usize,
    pub time_s: f64,
    pub grams: f64,
    pub filament_mm: f64,
}

impl Totals {
    pub fn of(statuses: &[PlateStatus]) -> Self {
        let mut t = Totals::default();
        for s in statuses {
            if *s != PlateStatus::Empty {
                t.plates += 1;
            }
            if let PlateStatus::Sliced {
                time_s,
                grams,
                filament_mm,
            } = s
            {
                t.sliced += 1;
                t.time_s += time_s;
                t.grams += grams;
                t.filament_mm += filament_mm;
            }
        }
        t
    }

    /// "3 of 4 sliced" (or "all 4 sliced", "none sliced yet").
    pub fn note(&self) -> String {
        match (self.sliced, self.plates) {
            (_, 0) => "no plate has objects".into(),
            (0, _) => "none sliced yet".into(),
            (s, n) if s == n && n > 1 => format!("all {n} sliced"),
            (s, n) => format!("{s} of {n} sliced"),
        }
    }

    /// `2 h 05 min · 46.3 g · 15.42 m` over the sliced plates.
    pub fn text(&self) -> String {
        format!(
            "{} · {} · {:.2} m",
            crate::panels::topbar::short_duration(self.time_s),
            weight(self.grams),
            self.filament_mm / 1000.
        )
    }
}

/// What happens once a batch of slices is done.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Then {
    /// Slice all plates.
    Nothing,
    /// Export all plates: write the files (`ExportAll`).
    Export,
    /// The Print dialog's All plates: hand the files to the dialog.
    Send,
}

/// Plates being sliced one after the other.
#[derive(Clone, Debug)]
pub struct SliceBatch {
    /// The plates (ids) the batch slices, in order; `App::slice_queue` holds those not started.
    pub ids: Vec<u64>,
    /// Plates whose slice failed, with the reason.
    pub failed: Vec<(u64, String)>,
    pub then: Then,
    /// Switch to the Preview when the current plate is done (Slice all; not for exports).
    pub manual: bool,
}

/// Export all plates, from the folder to the result (its window: `panels::export_all`).
pub struct ExportAll {
    pub dir: PathBuf,
    /// The file each plate gets, as planned when the folder was chosen (the window lists it).
    pub plan: Vec<(usize, String)>,
    pub stage: ExportStage,
}

pub enum ExportStage {
    /// Some files are already in the folder: replace them?
    Confirm {
        existing: Vec<String>,
    },
    /// Slicing the plates that need it.
    Slicing,
    /// Copying the files (off the UI thread); the plates left out, and why.
    Writing {
        rx: mpsc::Receiver<Result<Vec<PathBuf>, String>>,
        skipped: Vec<String>,
    },
    Done {
        files: Vec<PathBuf>,
        /// Plates without a file and why ("Plate 2: brim doesn't fit on the bed").
        skipped: Vec<String>,
    },
    Failed(String),
    Canceled,
}

impl ExportStage {
    pub fn busy(&self) -> bool {
        matches!(self, ExportStage::Slicing | ExportStage::Writing { .. })
    }
}

impl App {
    /// Whether plate `i` has anything to slice (objects, or a calibration test).
    pub(crate) fn sliceable(&self, i: usize) -> bool {
        let p = self.plates.plate(i);
        !p.objects.is_empty() || p.calibration.is_some()
    }

    /// Whether plate `i` has a slice of what it is now (colors included).
    pub(crate) fn up_to_date(&self, i: usize) -> bool {
        self.plates.plate(i).finished.is_some() && self.slice_fit(i) != Some(Fit::Colours)
    }

    /// Plate `i`'s slice status.
    pub(crate) fn plate_status(&self, i: usize) -> PlateStatus {
        let plate = self.plates.plate(i);
        if self.worker.as_ref().is_some_and(|(id, _)| *id == plate.id) {
            return PlateStatus::Slicing {
                progress: self.progress,
            };
        }
        if self.pending_slice.is_some_and(|(id, _, _)| id == plate.id)
            || self.slice_queue.contains(&plate.id)
        {
            return PlateStatus::Queued;
        }
        if let Some(e) = &plate.slice_error {
            return PlateStatus::Failed {
                short: e.short.clone(),
                message: e.message.clone(),
            };
        }
        if !self.sliceable(i) {
            return PlateStatus::Empty;
        }
        if let Some(f) = &plate.finished {
            return PlateStatus::Sliced {
                time_s: f.summary.time_s,
                grams: f.summary.grams,
                filament_mm: f.summary.filament_mm,
            };
        }
        if plate.previous.is_some() {
            PlateStatus::Outdated
        } else {
            PlateStatus::NotSliced
        }
    }

    pub(crate) fn plate_statuses(&self) -> Vec<PlateStatus> {
        (0..self.plates.len())
            .map(|i| self.plate_status(i))
            .collect()
    }

    /// The plates (indices) a batch slices: those with objects whose slice is missing, out of
    /// date or carries old colors. A slice with the current colors in the cache is taken
    /// instead of slicing again.
    fn plates_to_slice(&mut self) -> Vec<usize> {
        self.reconcile_slices();
        let mut out = vec![];
        for i in 0..self.plates.len() {
            if !self.sliceable(i) {
                continue;
            }
            if self.slice_fit(i) == Some(Fit::Colours) {
                if let Ok(inputs) = self.slice_inputs(i)
                    && let Some((key, finished, Fit::Exact)) = self.slice_cache.find(&inputs)
                {
                    let plate = &mut self.plates.plates[i];
                    plate.finished = Some(finished);
                    plate.sliced = Some(key);
                    continue;
                }
                self.plates.plates[i].invalidate();
            }
            if !self.up_to_date(i) {
                out.push(i);
            }
        }
        out
    }

    /// Slice all plates (⇧⌘R): every plate that needs it, one after the other.
    pub(crate) fn slice_all(&mut self, ctx: &egui::Context) {
        self.start_batch(Then::Nothing, ctx);
    }

    /// Starts slicing the plates that need it, then does `then`. A batch replaces a running one.
    pub(crate) fn start_batch(&mut self, then: Then, ctx: &egui::Context) {
        if self.viewing_gcode() {
            self.status = "Close the G-code file to slice the plates".into();
            return;
        }
        if let Err(e) = &self.config {
            self.status = format!("Cannot slice: {e}");
            if then != Then::Nothing {
                self.batch_aborted(then, format!("The settings have a problem: {e}"));
            }
            return;
        }
        if let Some(old) = self.slice_batch.take()
            && old.then != then
        {
            self.batch_aborted(old.then, "Another slice of all plates started".into());
        }
        let plates = self.plates_to_slice();
        let ids: Vec<u64> = plates.iter().map(|&i| self.plates.plate(i).id).collect();
        if ids.is_empty() && then == Then::Nothing {
            self.status = if (0..self.plates.len()).any(|i| self.sliceable(i)) {
                "Every plate is already sliced".into()
            } else {
                "Nothing to slice".into()
            };
            return;
        }
        // The plate a running job slices stays with that job.
        self.slice_queue = ids.iter().rev().copied().collect();
        self.slice_batch = Some(SliceBatch {
            ids,
            failed: vec![],
            then,
            manual: then == Then::Nothing,
        });
        self.advance_batch(ctx);
    }

    /// "plate 2 of 4" while a batch slices: the plate being sliced (or next) and how many.
    pub(crate) fn batch_position(&self) -> Option<(usize, usize)> {
        let b = self.slice_batch.as_ref()?;
        let n = b.ids.len();
        Some(((n - self.slice_queue.len()).clamp(1, n.max(1)), n))
    }

    /// Starts the batch's next plate once nothing else runs; finishes the batch when none is
    /// left. Called every frame.
    pub(crate) fn advance_batch(&mut self, ctx: &egui::Context) {
        if self.slice_batch.is_none() || self.busy() || self.pending_slice.is_some() {
            return;
        }
        let manual = self.slice_batch.as_ref().is_some_and(|b| b.manual);
        while let Some(id) = self.slice_queue.pop() {
            let Some(i) = self.plates.index_of(id) else {
                continue;
            };
            // Sliced meanwhile (a job that was running, a cached slice).
            if self.up_to_date(i) || !self.sliceable(i) {
                continue;
            }
            self.slice_plate(i, manual, ctx);
            if self.worker.is_some() || self.stopping.is_some() || self.pending_slice.is_some() {
                return;
            }
            // Refused (a tower over an object, the settings): its failure is on the plate.
        }
        if let Some(b) = self.slice_batch.take() {
            self.finish_batch(b, ctx);
        }
    }

    /// A slice of plate `id` failed while a batch runs: the batch notes it and goes on.
    pub(crate) fn batch_failure(&mut self, id: u64, message: &str) {
        if let Some(b) = &mut self.slice_batch
            && b.ids.contains(&id)
            && !b.failed.iter().any(|(f, _)| *f == id)
        {
            b.failed.push((id, message.to_string()));
        }
    }

    /// Cancel while a batch runs: the queue goes, and what waited for it is told.
    pub(crate) fn cancel_batch(&mut self) {
        self.slice_queue.clear();
        if let Some(b) = self.slice_batch.take() {
            self.batch_aborted(b.then, "Canceled".into());
        }
    }

    fn batch_aborted(&mut self, then: Then, why: String) {
        match then {
            Then::Nothing => {}
            Then::Export => {
                if let Some(e) = &mut self.export_all
                    && matches!(e.stage, ExportStage::Slicing)
                {
                    e.stage = if why == "Canceled" {
                        ExportStage::Canceled
                    } else {
                        ExportStage::Failed(why)
                    };
                }
            }
            Then::Send => {
                if let Some(d) = &mut self.print_job {
                    d.plates_failed(why);
                }
            }
        }
    }

    fn finish_batch(&mut self, b: SliceBatch, ctx: &egui::Context) {
        let names: Vec<String> = b
            .failed
            .iter()
            .filter_map(|(id, _)| self.plates.index_of(*id))
            .map(|i| self.plates.plate(i).name.clone())
            .collect();
        match b.then {
            Then::Nothing => {
                let done = b
                    .ids
                    .iter()
                    .filter_map(|id| self.plates.index_of(*id))
                    .filter(|&i| self.up_to_date(i))
                    .count();
                self.status = match (done, names.is_empty()) {
                    (n, true) if n == b.ids.len() => {
                        format!("Sliced {n} {}", if n == 1 { "plate" } else { "plates" })
                    }
                    (n, true) => format!("Sliced {n} of {} plates", b.ids.len()),
                    (n, false) => format!(
                        "Sliced {n} of {} plates · failed: {}",
                        b.ids.len(),
                        names.join(", ")
                    ),
                };
            }
            Then::Export => self.write_export_all(ctx),
            Then::Send => self.fill_send_batch(),
        }
    }

    // ----- Export all plates ----------------------------------------------------------------

    /// File › Export all plates…: choose a folder, then [`Self::export_all_to`].
    pub(crate) fn export_all_dialog(&mut self, ctx: &egui::Context) {
        if self.headless {
            self.status = "Exporting needs the file dialog".into();
            return;
        }
        if let Some(dir) = rfd::FileDialog::new()
            .set_title("Export all plates into a folder")
            .pick_folder()
        {
            self.export_all_to(dir, ctx);
        }
    }

    /// Whether Export all plates can run now.
    pub(crate) fn can_export_all(&self) -> bool {
        !self.viewing_gcode()
            && self.config.is_ok()
            && self.loading.is_none()
            && !self.export_all.as_ref().is_some_and(|e| e.stage.busy())
            && (0..self.plates.len()).any(|i| self.sliceable(i))
    }

    /// The file each plate with objects is written to: `(plate index, file name)`, named as
    /// Export names a plate's file, made unique.
    pub(crate) fn export_all_plan(&self) -> Vec<(usize, String)> {
        let mut taken = vec![];
        (0..self.plates.len())
            .filter(|&i| self.sliceable(i))
            .map(|i| {
                let stem = mica_core::export_name::unique(&self.export_stem_of(i), &mut taken);
                (i, format!("{stem}.{}", self.export_extension(i)))
            })
            .collect()
    }

    /// The printer's own format for plate `i`'s file: `bgcode` for Prusa binary G-code.
    pub(crate) fn export_extension(&self, i: usize) -> &'static str {
        match self.job_config(i) {
            Ok(c) if mica_cli::binary_output(&c) => "bgcode",
            _ => "gcode",
        }
    }

    /// Export all plates into `dir`: asks first when files would be replaced, else slices
    /// what is missing and writes the files.
    pub(crate) fn export_all_to(&mut self, dir: PathBuf, ctx: &egui::Context) {
        if !self.can_export_all() {
            self.status = "Nothing to export: every plate is empty".into();
            return;
        }
        let plan = self.export_all_plan();
        let existing: Vec<String> = plan
            .iter()
            .map(|(_, name)| name.clone())
            .filter(|name| dir.join(name).exists())
            .collect();
        self.export_all = Some(ExportAll {
            dir,
            plan,
            stage: ExportStage::Confirm {
                existing: existing.clone(),
            },
        });
        if existing.is_empty() {
            self.export_all_start(ctx);
        }
    }

    /// Replace (or nothing to replace): slice what is missing, then write.
    pub(crate) fn export_all_start(&mut self, ctx: &egui::Context) {
        let Some(e) = &mut self.export_all else {
            return;
        };
        e.stage = ExportStage::Slicing;
        self.start_batch(Then::Export, ctx);
    }

    /// The batch is done: copy each plate's sliced file into the folder, off the UI thread.
    fn write_export_all(&mut self, ctx: &egui::Context) {
        let Some(dir) = self.export_all.as_ref().map(|e| e.dir.clone()) else {
            return;
        };
        let mut copies = vec![];
        let mut skipped = vec![];
        for (i, name) in self.export_all_plan() {
            let plate = self.plates.plate(i);
            match &plate.finished {
                Some(f) if self.up_to_date(i) => {
                    let ext = if name.ends_with(".bgcode") {
                        "bgcode"
                    } else {
                        "gcode"
                    };
                    copies.push((
                        f.preview.job.path.join(format!("print.{ext}")),
                        dir.join(&name),
                        f.preview.job.clone(),
                    ));
                }
                _ => skipped.push(match &plate.slice_error {
                    Some(e) => format!("{}: {}", plate.name, e.short),
                    None => format!("{}: changed while slicing; export again", plate.name),
                }),
            }
        }
        if copies.is_empty() {
            if let Some(e) = &mut self.export_all {
                e.stage = ExportStage::Failed(format!(
                    "No plate could be sliced: {}",
                    skipped.join("; ")
                ));
            }
            return;
        }
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let mut written = vec![];
            let mut result = Ok(());
            for (from, to, hold) in copies {
                if let Err(e) = std::fs::copy(&from, &to) {
                    result = Err(format!("{}: {e}", to.display()));
                    break;
                }
                drop(hold);
                written.push(to);
            }
            let _ = tx.send(result.map(|()| written));
            ctx.request_repaint();
        });
        if let Some(e) = &mut self.export_all {
            e.stage = ExportStage::Writing { rx, skipped };
        }
    }

    /// Picks up the copy's result: the window shows it and a notification says it.
    pub(crate) fn poll_export_all(&mut self) {
        let Some(e) = &mut self.export_all else {
            return;
        };
        let ExportStage::Writing { rx, skipped } = &e.stage else {
            return;
        };
        let result = match rx.try_recv() {
            Ok(r) => r,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => {
                Err("the export stopped unexpectedly".to_string())
            }
        };
        let dir = e.dir.clone();
        let skipped = skipped.clone();
        match result {
            Ok(files) => {
                let text = export_message(files.len(), &dir, &skipped);
                if let Some(e) = &mut self.export_all {
                    e.stage = ExportStage::Done {
                        files: files.clone(),
                        skipped: skipped.clone(),
                    };
                }
                use crate::notifications::{Action, Level, Notification};
                let level = if skipped.is_empty() {
                    Level::Info
                } else {
                    Level::Warning
                };
                let mut n = Notification::new(level, text.clone(), text);
                if let Some(first) = files.first() {
                    n = n.action(reveal_label(), Action::Reveal(first.clone()));
                }
                self.notify(n);
            }
            Err(why) => {
                if let Some(e) = &mut self.export_all {
                    e.stage = ExportStage::Failed(why.clone());
                }
                self.notify_error("Export failed", format!("Export failed: {why}"));
            }
        }
    }

    // ----- Send all plates ------------------------------------------------------------------

    /// The Print dialog's All plates: slice what is missing, then hand the files over.
    pub(crate) fn prepare_send_all(&mut self, ctx: &egui::Context) {
        self.start_batch(Then::Send, ctx);
    }

    /// Every sliced plate's file for the Print dialog, in plate order.
    fn fill_send_batch(&mut self) {
        let mut items = vec![];
        let mut skipped = vec![];
        let plan = self.export_all_plan();
        for (i, name) in plan {
            let plate = self.plates.plate(i);
            match &plate.finished {
                Some(f) if self.up_to_date(i) => {
                    // The format Print sends one plate in.
                    let file = self.job_file(f);
                    let stem = name
                        .rsplit_once('.')
                        .map_or(name.as_str(), |(stem, _)| stem);
                    let remote = format!(
                        "{stem}.{}",
                        match file.format {
                            mica_printhost::FileFormat::Bgcode => "bgcode",
                            mica_printhost::FileFormat::Gcode => "gcode",
                        }
                    );
                    items.push(crate::printhost::job::BatchItem::new(
                        plate.name.clone(),
                        file,
                        remote,
                    ));
                }
                _ => skipped.push(match &plate.slice_error {
                    Some(e) => format!("{}: {}", plate.name, e.short),
                    None => format!("{}: not sliced", plate.name),
                }),
            }
        }
        if let Some(d) = &mut self.print_job {
            d.set_plates(items, skipped);
        }
    }
}

/// "Exported 4 files to ~/Desktop/prints" (with the plates left out).
pub fn export_message(files: usize, dir: &std::path::Path, skipped: &[String]) -> String {
    let mut text = format!(
        "Exported {files} {} to {}",
        if files == 1 { "file" } else { "files" },
        dir.display()
    );
    if !skipped.is_empty() {
        text.push_str(&format!(" · not exported: {}", skipped.join("; ")));
    }
    text
}

/// "Show in Finder" on the Mac, "Show in folder" elsewhere.
pub fn reveal_label() -> &'static str {
    if cfg!(target_os = "macos") {
        "Show in Finder"
    } else {
        "Show in folder"
    }
}
