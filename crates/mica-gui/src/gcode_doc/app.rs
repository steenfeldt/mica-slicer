// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The app's side of an opened G-code file: opening (on a thread, with progress), showing it
//! in Preview over the untouched scene, the validator, Save copy…, Print… and closing.

use super::{Check, Doc, Origin, Printer, Restore};
use crate::app::App;
use eframe::egui;
use mica_core::config::Config;
use mica_profiles::project::{split_printer_name, strip_copy};
use std::path::{Path, PathBuf};
use std::sync::mpsc;

impl App {
    /// Whether an opened G-code file is showing (the read-only document mode).
    pub(crate) fn viewing_gcode(&self) -> bool {
        self.gcode.is_some()
    }

    /// Opens a G-code file on a thread; it shows when ready (`poll_gcode`). The scene stays.
    pub(crate) fn open_gcode(&mut self, path: PathBuf, ctx: &egui::Context) {
        let config = self
            .effective_config()
            .cloned()
            .unwrap_or_else(mica_core::config::Config::defaults);
        let c = ctx.clone();
        let name = file_name(&path);
        // A newer request replaces one still loading (dropping it cancels its thread).
        self.gcode_loading = Some(super::start(path, config, move || c.request_repaint()));
        self.library.shown = false;
        self.machine = false;
        self.status = format!("Opening {name}…");
    }

    /// Opens a G-code file and waits for it (screenshots and tests).
    pub(crate) fn open_gcode_now(&mut self, path: &Path) -> Result<(), String> {
        let config = self
            .effective_config()
            .cloned()
            .unwrap_or_else(mica_core::config::Config::defaults);
        let started = std::time::Instant::now();
        let loaded = super::load(
            path,
            &config,
            &std::sync::atomic::AtomicBool::new(false),
            &|_, _| {},
        )?;
        self.show_gcode(loaded, started.elapsed(), None);
        Ok(())
    }

    /// Progress of the opening file and the validator's answer.
    pub(crate) fn poll_gcode(&mut self, ctx: &egui::Context) {
        let mut done = None;
        if let Some(l) = &mut self.gcode_loading {
            loop {
                match l.rx.try_recv() {
                    Ok(super::Event::Progress(f, stage)) => {
                        l.progress = f;
                        l.stage = stage;
                    }
                    Ok(super::Event::Done(result)) => {
                        done = Some(*result);
                        break;
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        done = Some(Err("Opening stopped unexpectedly".into()));
                        break;
                    }
                }
            }
        }
        if let Some(result) = done
            && let Some(l) = self.gcode_loading.take()
        {
            let name = file_name(&l.path);
            match result {
                Ok(loaded) => self.show_gcode(loaded, l.started.elapsed(), Some(ctx.clone())),
                Err(e) if e == "Canceled" => self.status = "Canceled".into(),
                Err(e) => {
                    self.status.clear();
                    self.notify_error(format!("{name} could not be opened"), e);
                }
            }
        }
        let mut finished_check = None;
        if let Some(doc) = &mut self.gcode
            && let Some(rx) = &doc.check_rx
        {
            match rx.try_recv() {
                Ok(check) => finished_check = Some(check),
                Err(mpsc::TryRecvError::Disconnected) => {
                    finished_check =
                        Some(Check::Failed("The file check stopped unexpectedly".into()))
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if let Some(check) = finished_check
            && let Some(doc) = &mut self.gcode
        {
            doc.check_rx = None;
            doc.check = check;
            let name = doc.name.clone();
            match &doc.check {
                Check::Done { errors, .. } if *errors > 0 => {
                    let errors = *errors;
                    self.notify(
                        crate::notifications::Notification::new(
                            crate::notifications::Level::Error,
                            format!("{name}: {errors} G-code {}", plural(errors, "error")),
                            "Mica's check found problems in this file. The sidebar lists them; Print asks before sending it.",
                        )
                        .keyed("gcode-check"),
                    );
                }
                Check::Failed(e) => {
                    let e = e.clone();
                    self.notify_warning(format!("{name}: {e}"));
                }
                _ => {}
            }
        }
    }

    /// A loaded file becomes the document: Preview shows it, the validator starts.
    fn show_gcode(
        &mut self,
        loaded: super::Loaded,
        took: std::time::Duration,
        ctx: Option<egui::Context>,
    ) {
        let restore = match self.gcode.take() {
            Some(old) => old.restore,
            None => Restore {
                preview: self.preview,
                view: self.view.clone(),
            },
        };
        let mut doc = loaded.into_doc(restore);
        doc.printer = self.resolve_gcode_printer(&doc.origin);
        self.view = Self::view_for(&doc.finished);
        self.preview = true;
        self.machine = false;
        self.library.shown = false;
        self.preview_side.hold = false;
        let (config, against, other) = self.gcode_printer_config(&doc);
        // Another printer's file shows on that printer's bed.
        if other && let Ok(c) = &config {
            let mut bed = crate::scene::bed::Bed::from_config(c);
            let vendor = doc
                .printer
                .preset
                .as_ref()
                .map_or(self.profiles.vendor, |(vi, _)| *vi);
            bed.assets = self.profiles.bed_assets_of(vendor, c);
            doc.bed = Some(bed);
        }
        doc.check_rx = Some(super::start_check(
            doc.path.clone(),
            config.and_then(|c| mica_gcode::validate::Profile::from_config(&c)),
            against,
            move || {
                if let Some(c) = &ctx {
                    c.request_repaint();
                }
            },
        ));
        self.status = format!(
            "{} · {} · {} layers · read-only (opened in {:.1} s)",
            doc.name,
            crate::panels::duration(doc.finished.summary.time_s),
            doc.finished.preview.layers.len(),
            took.as_secs_f64()
        );
        self.gcode = Some(doc);
        self.fit_current_plate(false);
    }

    /// Closes the G-code file: the scene and its Preview state come back as they were.
    pub(crate) fn close_gcode(&mut self) {
        self.gcode_loading = None;
        let Some(doc) = self.gcode.take() else {
            return;
        };
        if self.print_job.is_some() && !self.print_job.as_ref().is_some_and(|d| d.busy()) {
            self.print_job = None;
            self.dialogs.print = false;
        }
        self.preview = doc.restore.preview && self.finished().is_some();
        self.view = match self.finished() {
            Some(f)
                if doc.restore.view.high < f.preview.layers.len()
                    || doc.restore.view.high == usize::MAX =>
            {
                doc.restore.view
            }
            Some(f) => Self::view_for(f),
            None => doc.restore.view,
        };
        self.status = format!("Closed {}", doc.name);
    }

    /// The installed printer preset and model name the file's printer resolves to.
    pub(crate) fn resolve_gcode_printer(&self, origin: &Origin) -> Printer {
        let all = self.profiles.all_printers();
        let found = |name: &str| {
            all.iter()
                .find(|(_, _, n)| n == name || *n == strip_copy(name))
                .map(|(vi, _, n)| (*vi, n.clone()))
        };
        if let Some(p) = &origin.preset
            && let Some(hit) = found(p)
        {
            return Printer {
                model_name: Some(split_printer_name(&hit.1).0),
                preset: Some(hit),
            };
        }
        // A file that does not name its nozzle: the selected printer when it is that model,
        // else the common 0.4 mm one.
        let nozzle = origin.nozzle.as_deref().map(|n| n.replace(' ', ""));
        let current = (self.profiles.vendor, self.profiles.printer.clone());
        let variant_fits = |variant: &str| {
            variant
                .replace(' ', "")
                .eq_ignore_ascii_case(nozzle.as_deref().unwrap_or("0.4"))
        };
        let best = |candidates: Vec<(usize, String, String)>| {
            let selected = nozzle
                .is_none()
                .then(|| {
                    candidates
                        .iter()
                        .find(|(vi, n, _)| (*vi, n) == (current.0, &current.1))
                })
                .flatten();
            selected
                .or_else(|| candidates.iter().find(|(_, _, v)| variant_fits(v)))
                .or(candidates.first())
                .map(|(vi, n, _)| (*vi, n.clone()))
        };
        // A model code (`COREONE`): the presets that say they are that model.
        if let Some(code) = &origin.model
            && !code.contains(' ')
            && let Some((vi, name)) = best(self.profiles.presets_of_model(code))
        {
            return Printer {
                model_name: Some(split_printer_name(&name).0),
                preset: Some((vi, name)),
            };
        }
        // A model name (`Snapmaker U1`) or a preset Mica does not have: the same model.
        let model = origin
            .preset
            .as_deref()
            .map(|p| split_printer_name(p).0)
            .or_else(|| origin.model.clone().filter(|m| m.contains(' ')));
        if let Some(model) = model {
            let candidates = all
                .iter()
                .filter_map(|(vi, _, n)| {
                    let (m, variant) = split_printer_name(n);
                    m.eq_ignore_ascii_case(&model)
                        .then(|| (*vi, n.clone(), variant))
                })
                .collect();
            return Printer {
                model_name: Some(model),
                preset: best(candidates),
            };
        }
        Printer::default()
    }

    /// The configuration the file is checked against and its name: the selected printer's
    /// settings when the file is for it (or does not say), else the file's printer with
    /// Mica's default profiles (`true`: another printer than the selected one).
    fn gcode_printer_config(&self, doc: &Doc) -> (Result<Config, String>, String, bool) {
        let current = self.profiles.printer.clone();
        match &doc.printer.preset {
            Some((vi, preset))
                if !(*vi == self.profiles.vendor
                    && mica_profiles::project::same_machine(preset, &current)) =>
            {
                (
                    self.profiles.printer_defaults_config(*vi, preset),
                    preset.clone(),
                    true,
                )
            }
            _ => (
                self.effective_config()
                    .cloned()
                    .ok_or_else(|| "the selected printer has no valid profile".to_string()),
                current,
                false,
            ),
        }
    }

    /// Save copy…: the opened file's bytes, unchanged, under another name.
    pub(crate) fn save_gcode_copy(&mut self, ctx: &egui::Context) {
        let Some(doc) = &self.gcode else { return };
        if self.headless {
            self.status = "Saving a copy needs the file dialog".into();
            return;
        }
        let ext = doc
            .path
            .extension()
            .map(|e| e.to_string_lossy().to_string())
            .unwrap_or_else(|| "gcode".into());
        let source = doc.path.clone();
        let Some(mut target) = rfd::FileDialog::new()
            .set_file_name(&doc.name)
            .add_filter("G-code", &[ext.as_str()])
            .save_file()
        else {
            return;
        };
        if target.extension().is_none() {
            target.set_extension(&ext);
        }
        if let Err(e) = self.copy_gcode(&source, &target, ctx) {
            self.notify_error("The copy was not saved", e);
        }
    }

    /// Copies the opened file to `target` on a thread (never onto itself).
    pub(crate) fn copy_gcode(
        &mut self,
        source: &Path,
        target: &Path,
        ctx: &egui::Context,
    ) -> Result<(), String> {
        let same = match (source.canonicalize(), target.canonicalize()) {
            (Ok(a), Ok(b)) => a == b,
            _ => source == target,
        };
        if same {
            return Err("That is the opened file itself; choose another name or folder.".into());
        }
        if self.loading.is_some() {
            return Err("Mica is busy; try again in a moment.".into());
        }
        let (tx, rx) = mpsc::channel();
        self.loading = Some(rx);
        self.status = "Saving a copy…".into();
        let ctx = ctx.clone();
        let (source, target) = (source.to_path_buf(), target.to_path_buf());
        std::thread::spawn(move || {
            let result = std::fs::copy(&source, &target)
                .map(|_| crate::app::LoadedScene {
                    plates: vec![],
                    objects: vec![],
                    warnings: vec![format!("Saved a copy as {}", target.display())],
                    tower: None,
                    paths: vec![],
                    settings: None,
                    config: None,
                    project: false,
                    custom_gcode: Default::default(),
                    mixes: None,
                })
                .map_err(|e| format!("The copy was not saved: {}: {e}", target.display()));
            let _ = tx.send(result);
            ctx.request_repaint();
        });
        Ok(())
    }

    /// Print… for the opened file: its own bytes, the tools it prints with, its colours and
    /// materials, and the printer that fits it chosen first.
    pub(crate) fn open_gcode_print_dialog(&mut self) {
        use crate::printhost::{
            explain::JobTool,
            job::{Dialog, JobFile},
        };
        let Some(doc) = &self.gcode else { return };
        let format = if doc.finished.binary {
            mica_printhost::FileFormat::Bgcode
        } else {
            mica_printhost::FileFormat::Gcode
        };
        let tools: Vec<JobTool> = doc
            .used_tools()
            .into_iter()
            .map(|t| JobTool {
                tool: t as u32,
                preset: doc
                    .filament_names
                    .get(t)
                    .map(|n| crate::preset_names::display_name(n).to_string())
                    .unwrap_or_default(),
                material: doc.materials.get(t).cloned().filter(|m| !m.is_empty()),
                colour: doc.colours.get(t).copied(),
            })
            .collect();
        let stem = doc
            .path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "print".into());
        let file = JobFile {
            path: doc.path.clone(),
            format,
            tools,
            bed_type: None,
            hold: None,
        };
        let mut dialog = Dialog::new(file, stem);
        if let Some(i) = self.gcode_printer_choice() {
            dialog.choose(self.printers.target(i));
            dialog.u1_remembered = self.remembered_u1(dialog.target.as_ref());
        }
        self.print_job = Some(dialog);
        self.dialogs.print = true;
    }

    /// The configured printer that fits the opened file best: linked to its preset, then the
    /// same model, then the same family; else the profile's default printer.
    pub(crate) fn gcode_printer_choice(&self) -> Option<usize> {
        use crate::printhost::config::Fit;
        let doc = self.gcode.as_ref()?;
        let mut ranked: Vec<(usize, Fit)> = self
            .printers
            .list
            .iter()
            .enumerate()
            .filter_map(|(i, p)| Some((i, super::fit(p, &doc.origin, &doc.printer)?)))
            .collect();
        ranked.sort_by_key(|(i, f)| (*f, *i));
        ranked
            .into_iter()
            .find(|(_, f)| *f != Fit::Other)
            .map(|(i, _)| i)
            .or_else(|| {
                (!doc.origin.known())
                    .then(|| self.linked_printer())
                    .flatten()
            })
    }

    /// The warning for the Print dialog's chosen printer, when the file was made for another
    /// model.
    /// `reported`: the model the printer gave at the dialog's check (newer than the saved one).
    pub(crate) fn gcode_mismatch(&self, printer_id: u64, reported: Option<&str>) -> Option<String> {
        let doc = self.gcode.as_ref()?;
        let mut p = self
            .printers
            .list
            .iter()
            .find(|p| p.id == printer_id)?
            .clone();
        if let Some(m) = reported.filter(|m| !m.trim().is_empty()) {
            p.model = Some(m.to_string());
        }
        super::mismatch(&p, &doc.origin, &doc.printer)
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

pub(crate) fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        word.to_string()
    } else {
        format!("{word}s")
    }
}
