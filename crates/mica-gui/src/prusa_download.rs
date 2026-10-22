// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The Prusa printer profiles, downloaded from Prusa once (`mica_profiles::vendor_profiles`):
//! Mica does not redistribute them. When no Prusa bundle is found at launch, the real window
//! starts the download on a worker thread; the Welcome window, the Add printer gallery and
//! the printer menu say what is happening ("Prusa printer profiles are downloaded from Prusa
//! once"), show the progress, and offer "Try again" after a failure, as does a notification.
//! Once the bundle is verified the Prusa printers appear, and a remembered Prusa printer is
//! selected again. Headless runs (tests, screenshots, the monkey) and isolated processes never
//! go online by themselves; there the same places offer "Download".

use crate::app::App;
use crate::notifications::{Action, Level, Notification};
use crate::theme;
use eframe::egui::{self, RichText};
use mica_profiles::vendor_profiles::prusa;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::Duration;

#[derive(Clone, Debug, PartialEq)]
pub enum Phase {
    /// Not started (headless runs wait for "Download").
    Idle,
    Downloading(f32),
    Failed(String),
    Done,
}

enum Msg {
    Progress(f32),
    Done(Result<prusa::Fetched, String>),
}

pub struct PrusaDownload {
    pub phase: Phase,
    rx: Option<mpsc::Receiver<Msg>>,
    cancel: Arc<AtomicBool>,
    /// The Prusa printer the settings remembered, selected again once the profiles are here.
    pub restore: Option<String>,
}

impl Default for PrusaDownload {
    fn default() -> Self {
        Self {
            phase: Phase::Idle,
            rx: None,
            cancel: Arc::default(),
            restore: None,
        }
    }
}

impl Drop for PrusaDownload {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl PrusaDownload {
    pub fn working(&self) -> bool {
        matches!(self.phase, Phase::Downloading(_))
    }

    /// Starts the download on its own thread (nothing happens while one runs).
    pub fn start(&mut self, ctx: &egui::Context) {
        if self.working() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.cancel = Arc::default();
        let cancel = self.cancel.clone();
        let repaint = ctx.clone();
        self.phase = Phase::Downloading(0.);
        let spawned = std::thread::Builder::new()
            .name("mica-prusa-profiles".into())
            .spawn(move || {
                let progress_tx = tx.clone();
                let mut last = std::time::Instant::now();
                let result = prusa::fetch(
                    &mut |f| {
                        if last.elapsed() > Duration::from_millis(100) || f >= 1. {
                            last = std::time::Instant::now();
                            let _ = progress_tx.send(Msg::Progress(f));
                            repaint.request_repaint();
                        }
                    },
                    &cancel,
                );
                let _ = tx.send(Msg::Done(result));
                repaint.request_repaint();
            });
        match spawned {
            Ok(_) => self.rx = Some(rx),
            Err(e) => {
                self.phase = Phase::Failed(format!("the download could not start ({e})"));
            }
        }
    }

    /// What the worker reported: the finished download, once.
    fn poll(&mut self) -> Option<prusa::Fetched> {
        let rx = self.rx.as_ref()?;
        loop {
            match rx.try_recv() {
                Ok(Msg::Progress(f)) => {
                    if self.working() {
                        self.phase = Phase::Downloading(f);
                    }
                }
                Ok(Msg::Done(Ok(fetched))) => {
                    self.rx = None;
                    self.phase = Phase::Done;
                    return Some(fetched);
                }
                Ok(Msg::Done(Err(e))) => {
                    self.rx = None;
                    self.phase = Phase::Failed(e);
                    return None;
                }
                Err(mpsc::TryRecvError::Empty) => return None,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.rx = None;
                    self.phase = Phase::Failed("the download stopped unexpectedly".into());
                    return None;
                }
            }
        }
    }
}

const KEY: &str = "prusa-profiles";

/// A share as "45 %".
fn percent(f: f32) -> String {
    format!("{:.0} %", (f * 100.).clamp(0., 100.))
}

impl App {
    /// At launch (real window only): downloads the Prusa profiles when none were found.
    pub(crate) fn startup_prusa_download(&mut self, ctx: &egui::Context) {
        // The remembered Prusa printer waits in `prusa_download.restore` (`App::create`).
        if !self.profiles.prusa_missing {
            return;
        }
        if !self.headless && !mica_core::user_data::isolated() {
            self.prusa_download.start(ctx);
        }
    }

    /// "Download" / "Try again".
    pub(crate) fn retry_prusa_download(&mut self, ctx: &egui::Context) {
        if self.profiles.prusa_missing {
            self.prusa_download.start(ctx);
        }
    }

    /// Takes the worker's news; called every frame. A finished download adds the Prusa
    /// printers (and selects the remembered one).
    pub(crate) fn poll_prusa_download(&mut self, ctx: &egui::Context) {
        if let Some(fetched) = self.prusa_download.poll() {
            self.prusa_profiles_arrived(fetched);
        }
        if self.prusa_download.working() {
            ctx.request_repaint_after(Duration::from_millis(200));
        }
    }

    /// The downloaded bundle: the Prusa printers appear, and the remembered Prusa printer is
    /// selected again (unless the user chose another meanwhile).
    pub(crate) fn prusa_profiles_arrived(&mut self, fetched: prusa::Fetched) {
        match self.profiles.install_prusa(&fetched.bundle) {
            Ok(()) => {
                self.add_printer.catalog = None;
                if let Some(printer) = self.prusa_download.restore.take()
                    && let Some(vi) = self.profiles.vendor_index("Prusa")
                    && self.profiles.printer_exists(vi, &printer)
                {
                    self.switch_printer(vi, &printer);
                }
                let mut text = "The Prusa printer profiles are ready.".to_string();
                if !fetched.missing_assets.is_empty() {
                    text.push_str(&format!(
                        " {} bed pictures could not be downloaded; those printers show a \
                         plain bed.",
                        fetched.missing_assets.len()
                    ));
                }
                self.notify_info(text);
            }
            Err(e) => self.prusa_download.phase = Phase::Failed(e),
        }
    }

    /// The download's notification, rebuilt every frame with the others.
    pub(crate) fn prusa_download_notification(&self) -> Option<Notification> {
        if !self.profiles.prusa_missing {
            return None;
        }
        let mut n = match &self.prusa_download.phase {
            Phase::Done => return None,
            Phase::Idle => {
                Notification::new(Level::Info, "Prusa printer profiles", prusa::DOWNLOAD_NOTE)
                    .keyed(format!("{KEY}:idle"))
                    .action("Download", Action::PrusaProfilesDownload)
            }
            Phase::Downloading(f) => {
                let title = format!("Downloading Prusa printer profiles · {}", percent(*f));
                Notification::new(Level::Info, title, prusa::DOWNLOAD_NOTE)
                    .keyed(format!("{KEY}:downloading"))
            }
            Phase::Failed(e) => Notification::new(
                Level::Error,
                "Couldn't download the Prusa printer profiles",
                format!("{} The download failed: {e}.", prusa::DOWNLOAD_NOTE),
            )
            .keyed(format!("{KEY}:failed:{e}"))
            .action("Try again", Action::PrusaProfilesDownload),
        };
        // Stays while it holds, like the update's progress.
        n.life = None;
        Some(n)
    }

    /// The same status inline (Welcome window, Add printer gallery, printer menu): nothing
    /// once the Prusa profiles are there.
    pub(crate) fn prusa_download_ui(&mut self, ui: &mut egui::Ui) {
        if !self.profiles.prusa_missing {
            return;
        }
        let p = theme::of(ui);
        let mut start = false;
        ui.vertical(|ui| {
            ui.label(
                RichText::new(prusa::DOWNLOAD_NOTE)
                    .font(theme::sans(12.5))
                    .color(p.muted),
            );
            match &self.prusa_download.phase {
                Phase::Idle => start = ui.button("Download").clicked(),
                Phase::Downloading(f) => {
                    ui.add(
                        egui::ProgressBar::new(*f)
                            .desired_width(ui.available_width().min(260.))
                            .text(format!("Downloading… {}", percent(*f))),
                    );
                }
                Phase::Failed(e) => {
                    ui.label(
                        RichText::new(format!("The download failed: {e}."))
                            .font(theme::sans(12.5))
                            .color(theme::DANGER),
                    );
                    start = ui.button("Try again").clicked();
                }
                Phase::Done => {}
            }
        });
        if start {
            let ctx = ui.ctx().clone();
            self.retry_prusa_download(&ctx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(ctx: &egui::Context, tag: &str) -> App {
        App::create(
            ctx,
            crate::app::Options {
                prefs_path: None,
                store: Some(mica_profiles::user::Store::open(
                    std::env::temp_dir()
                        .join(format!("mica-prusa-download-{tag}-{}", std::process::id())),
                )),
                files: vec![],
                headless: true,
            },
        )
    }

    #[test]
    fn headless_runs_never_download_by_themselves() {
        let ctx = egui::Context::default();
        let mut app = app(&ctx, "headless");
        app.profiles.prusa_missing = true;
        app.startup_prusa_download(&ctx);
        assert_eq!(app.prusa_download.phase, Phase::Idle);
        let n = app.prusa_download_notification().unwrap();
        assert_eq!(n.text, prusa::DOWNLOAD_NOTE);
        assert_eq!(n.actions[0].0, "Download");
        assert!(n.life.is_none());
        // A failure says why and offers to try again.
        app.prusa_download.phase =
            Phase::Failed("Prusa's profile server could not be reached".into());
        let n = app.prusa_download_notification().unwrap();
        assert_eq!(n.level, Level::Error);
        assert!(n.text.starts_with(prusa::DOWNLOAD_NOTE), "{}", n.text);
        assert!(n.text.contains("could not be reached"), "{}", n.text);
        assert_eq!(n.actions[0].0, "Try again");
        // Nothing once the profiles are there.
        app.profiles.prusa_missing = false;
        assert!(app.prusa_download_notification().is_none());
    }

    #[test]
    fn a_finished_download_adds_the_prusa_printers_and_restores_the_selection() {
        if !mica_testkit::require_prusa_profiles() || !mica_testkit::require_orca_profiles() {
            return;
        }
        let ctx = egui::Context::default();
        let mut app = app(&ctx, "done");
        // As launched without the Prusa bundle: on the U1, remembering the CORE One.
        app.profiles.vendors.retain(|(k, _)| k != "Prusa");
        app.profiles.prusa_missing = true;
        let snap = app.profiles.vendor_index("Snapmaker").unwrap();
        app.switch_printer(snap, "Snapmaker U1 (0.4 nozzle)");
        app.prusa_download.restore = Some("Prusa CORE One HF0.4 nozzle".into());
        let (tx, rx) = mpsc::channel();
        app.prusa_download.rx = Some(rx);
        app.prusa_download.phase = Phase::Downloading(0.5);
        tx.send(Msg::Done(Ok(prusa::Fetched {
            bundle: mica_testkit::prusa_bundle(),
            assets: 0,
            missing_assets: vec![],
        })))
        .unwrap();
        app.poll_prusa_download(&ctx);
        assert!(!app.profiles.prusa_missing);
        assert_eq!(app.prusa_download.phase, Phase::Done);
        assert_eq!(app.profiles.vendor_name(), "Prusa");
        assert_eq!(app.profiles.printer, "Prusa CORE One HF0.4 nozzle");
        assert!(app.prusa_download_notification().is_none());
    }
}
