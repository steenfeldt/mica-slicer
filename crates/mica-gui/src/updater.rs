// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! "Check for Updates…" and the quiet daily check at start, on top of `mica-update`. All
//! network and disk work runs on a worker thread; the UI is a notification in the 3D view's
//! corner ("Mica Slicer 0.x is available — What's new / Install / Later"), a "What's new"
//! window, and progress while the package downloads. Every failure is shown. Installing puts
//! the new version in place without stopping anything; "Restart now" is offered only while
//! no slice, upload or watched print is running, and goes through the normal close path
//! (unsaved changes are still asked about).
//!
//! A development build (run from cargo's `target/` folder) never updates itself unless
//! `MICA_UPDATE_FORCE=1`; headless runs (tests, screenshots, the monkey) never go online.

use crate::app::App;
use crate::notifications::{Action, Level, Notification};
use crate::panels::preferences::PrefsTab;
use eframe::egui;
use mica_update::{Client, Release, Staged};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The quiet check runs at most this often.
pub const CHECK_EVERY: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Clone, Debug, PartialEq)]
pub enum Phase {
    Idle,
    Checking {
        manual: bool,
    },
    Available(Release),
    Downloading {
        release: Release,
        received: u64,
        total: Option<u64>,
    },
    Installing(Release),
    /// In place (or, on Windows, the installer is saved); waiting for a restart.
    Ready {
        release: Release,
        staged: Staged,
    },
    /// `manual`: the user asked (an error, not a warning). `retry`: what "Try again" redoes.
    Failed {
        message: String,
        manual: bool,
        retry: Retry,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Retry {
    Check,
    Install(Release),
}

enum Msg {
    Checked(Result<Option<Release>, String>),
    Progress(u64, Option<u64>),
    /// Downloaded and the signature checks out; installing now.
    Verified,
    Installed(Result<Staged, String>),
}

pub struct Updater {
    /// `Err`: why this build does not update itself (shown when the user asks).
    client: Result<Client, String>,
    pub phase: Phase,
    rx: Option<mpsc::Receiver<Msg>>,
    cancel: Arc<AtomicBool>,
    /// "Later" on the offer: hidden until the next check finds something.
    pub later: bool,
    /// The "What's new" window.
    pub notes_open: bool,
    /// A manual check found nothing new: say so once.
    pub up_to_date_note: bool,
}

impl Default for Updater {
    fn default() -> Self {
        Self::disabled("updates are off in this run")
    }
}

impl Updater {
    pub fn disabled(why: &str) -> Self {
        Self {
            client: Err(why.into()),
            phase: Phase::Idle,
            rx: None,
            cancel: Arc::default(),
            later: false,
            notes_open: false,
            up_to_date_note: false,
        }
    }

    /// The real app's updater: disabled for development builds unless forced.
    pub fn for_this_build() -> Self {
        let exe = std::env::current_exe().unwrap_or_default();
        if mica_update::is_dev_build(&exe) && !mica_update::forced() {
            return Self::disabled(
                "this is a development build (it runs from cargo's target folder), which never \
                 replaces itself; set MICA_UPDATE_FORCE=1 to try the updater anyway",
            );
        }
        let mut u = Self::disabled("");
        u.client = Client::official();
        u
    }

    pub fn enabled(&self) -> bool {
        self.client.is_ok()
    }

    pub fn working(&self) -> bool {
        matches!(
            self.phase,
            Phase::Checking { .. } | Phase::Downloading { .. } | Phase::Installing(_)
        )
    }

    /// The release on offer or being installed, if any.
    pub fn release(&self) -> Option<&Release> {
        match &self.phase {
            Phase::Available(r)
            | Phase::Downloading { release: r, .. }
            | Phase::Installing(r)
            | Phase::Ready { release: r, .. } => Some(r),
            _ => None,
        }
    }

    fn spawn(
        &mut self,
        ctx: &egui::Context,
        job: impl FnOnce(&mpsc::Sender<Msg>) + Send + 'static,
    ) {
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        let ctx = ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("mica-update".into())
            .spawn(move || {
                job(&tx);
                ctx.request_repaint();
            });
        if let Err(e) = spawned {
            self.rx = None;
            self.phase = Phase::Failed {
                message: format!("cannot start the update thread: {e}"),
                manual: true,
                retry: Retry::Check,
            };
        }
    }

    /// Asks the server. `manual`: from the menu (an up-to-date answer is shown too).
    pub fn check(&mut self, manual: bool, ctx: &egui::Context) {
        // Already installed and waiting for a restart: its notification says so.
        if self.working() || matches!(self.phase, Phase::Ready { .. }) {
            return;
        }
        let client = match &self.client {
            Ok(c) => c.clone(),
            Err(why) => {
                self.phase = Phase::Failed {
                    message: format!("Mica can't update itself: {why}."),
                    manual,
                    retry: Retry::Check,
                };
                return;
            }
        };
        self.later = false;
        self.phase = Phase::Checking { manual };
        let current = mica_update::current_version();
        self.spawn(ctx, move |tx| {
            let _ = tx.send(Msg::Checked(client.check(&current)));
        });
    }

    /// Downloads, verifies and installs `release`.
    pub fn install(&mut self, release: Release, ctx: &egui::Context) {
        if self.working() {
            return;
        }
        let Ok(client) = self.client.clone() else {
            return;
        };
        let exe = std::env::current_exe().unwrap_or_default();
        let appimage = std::env::var_os("APPIMAGE").map(std::path::PathBuf::from);
        let target = match mica_update::target_for(&exe, appimage.as_deref()) {
            Ok(t) => t,
            Err(e) => {
                self.phase = Phase::Failed {
                    message: e,
                    manual: true,
                    retry: Retry::Install(release),
                };
                return;
            }
        };
        self.later = false;
        self.cancel = Arc::default();
        let cancel = self.cancel.clone();
        self.phase = Phase::Downloading {
            release: release.clone(),
            received: 0,
            total: None,
        };
        let repaint = ctx.clone();
        self.spawn(ctx, move |tx| {
            let mut last = std::time::Instant::now();
            let progress_tx = tx.clone();
            let bytes = client.download(
                &release,
                &mut |got, total| {
                    // A few updates a second are plenty.
                    if last.elapsed() > Duration::from_millis(100) || Some(got) == total {
                        last = std::time::Instant::now();
                        let _ = progress_tx.send(Msg::Progress(got, total));
                        repaint.request_repaint();
                    }
                },
                &cancel,
            );
            let result = bytes.and_then(|b| {
                let _ = tx.send(Msg::Verified);
                mica_update::install(&target, release.format, &b)
            });
            let _ = tx.send(Msg::Installed(result));
        });
    }

    /// Takes what the worker reported.
    pub fn poll(&mut self) {
        let Some(rx) = &self.rx else { return };
        loop {
            match rx.try_recv() {
                Ok(Msg::Checked(r)) => {
                    let manual = matches!(self.phase, Phase::Checking { manual: true });
                    self.phase = match r {
                        Ok(Some(release)) => Phase::Available(release),
                        Ok(None) => {
                            self.up_to_date_note = manual;
                            Phase::Idle
                        }
                        Err(message) => Phase::Failed {
                            message,
                            manual,
                            retry: Retry::Check,
                        },
                    };
                }
                Ok(Msg::Progress(got, n)) => {
                    if let Phase::Downloading {
                        received, total, ..
                    } = &mut self.phase
                    {
                        *received = got;
                        *total = n;
                    }
                }
                Ok(Msg::Verified) => {
                    if let Some(r) = self.release().cloned() {
                        self.phase = Phase::Installing(r);
                    }
                }
                Ok(Msg::Installed(r)) => {
                    let Some(release) = self.release().cloned() else {
                        continue;
                    };
                    self.phase = match r {
                        Ok(staged) => Phase::Ready { release, staged },
                        Err(message) => Phase::Failed {
                            message,
                            manual: true,
                            retry: Retry::Install(release),
                        },
                    };
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.rx = None;
                    if self.working() {
                        self.phase = Phase::Failed {
                            message: "the update thread stopped unexpectedly".into(),
                            manual: true,
                            retry: Retry::Check,
                        };
                    }
                    break;
                }
            }
        }
    }
}

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Whether the quiet check is due (`last`: seconds since 1970 of the last one).
pub fn due(last: u64, now: u64) -> bool {
    last > now || now - last >= CHECK_EVERY.as_secs()
}

const KEY: &str = "update";

impl App {
    /// The quiet check at start: on (Preferences), not in the last day, a build that updates.
    pub(crate) fn startup_update_check(&mut self, ctx: &egui::Context) {
        let now = now_secs();
        if self.headless
            || !self.prefs.check_updates
            || !self.updater.enabled()
            || !due(self.prefs.last_update_check, now)
        {
            return;
        }
        self.prefs.last_update_check = now;
        self.prefs_dirty = true;
        self.updater.check(false, ctx);
    }

    /// "Check for Updates…".
    pub(crate) fn check_for_updates(&mut self, ctx: &egui::Context) {
        self.prefs.last_update_check = now_secs();
        self.prefs_dirty = true;
        self.updater.check(true, ctx);
    }

    /// What keeps "Restart now" back (a slice, an upload, a print being watched), as the
    /// end of "Restart becomes available once …".
    pub(crate) fn update_blocker(&self) -> Option<&'static str> {
        if self.busy() {
            return Some("the slice has finished");
        }
        if self.print_job.as_ref().is_some_and(|d| d.busy()) {
            return Some("the upload has finished");
        }
        let watching = (0..self.printers.list.len()).any(|i| {
            self.printers.live(i).is_some_and(|l| {
                matches!(
                    l.status.as_ref().map(|s| s.state),
                    Some(
                        mica_printhost::PrinterState::Printing
                            | mica_printhost::PrinterState::Paused
                    )
                )
            })
        });
        watching.then_some("the print Mica is watching is done")
    }

    /// Takes the worker's news; called every frame.
    pub(crate) fn poll_updater(&mut self, ctx: &egui::Context) {
        self.updater.poll();
        if std::mem::take(&mut self.updater.up_to_date_note) {
            self.notify_info(format!(
                "Mica Slicer {} is up to date.",
                mica_update::current_version()
            ));
        }
        if self.updater.working() {
            ctx.request_repaint_after(Duration::from_millis(200));
        }
    }

    /// The update's notification, rebuilt every frame with the others.
    pub(crate) fn update_notification(&self) -> Option<Notification> {
        let u = &self.updater;
        let n = match &u.phase {
            Phase::Idle | Phase::Checking { manual: false } => return None,
            Phase::Checking { manual: true } => Notification::new(
                Level::Info,
                "Checking for updates…",
                "Checking for updates…",
            )
            .keyed(format!("{KEY}:checking")),
            Phase::Available(r) => {
                if u.later {
                    return None;
                }
                let title = format!("Mica Slicer {} is available", r.version);
                let text = format!(
                    "{title}. You have {}. Installing downloads it in the background; nothing \
                     you are doing stops.",
                    mica_update::current_version()
                );
                Notification::new(Level::Info, title, text)
                    .keyed(format!("{KEY}:available:{}", r.version))
                    .action("What's new", Action::UpdateNotes)
                    .action("Install", Action::UpdateInstall)
                    .action("Later", Action::UpdateLater)
            }
            Phase::Downloading {
                release,
                received,
                total,
            } => {
                let amount = match mica_update::fraction(*received, *total) {
                    Some(f) => format!("{:.0} %", f * 100.),
                    None => format!("{:.1} MB", *received as f64 / 1e6),
                };
                let title = format!("Downloading Mica Slicer {} · {amount}", release.version);
                Notification::new(Level::Info, title.clone(), title)
                    .keyed(format!("{KEY}:downloading"))
                    .action("Cancel", Action::UpdateCancel)
            }
            Phase::Installing(r) => {
                let t = format!("Installing Mica Slicer {}…", r.version);
                Notification::new(Level::Info, t.clone(), t).keyed(format!("{KEY}:installing"))
            }
            Phase::Ready { .. } if u.later => return None,
            Phase::Ready { release, staged } => {
                let windows = matches!(staged, Staged::Installer(_));
                let title = if windows {
                    format!("Mica Slicer {} is ready to install", release.version)
                } else {
                    format!("Mica Slicer {} is installed", release.version)
                };
                let mut text = if windows {
                    format!(
                        "{title}. The installer runs when Mica restarts (or quits, after \
                         \"Later\")."
                    )
                } else {
                    format!("{title}. It starts the next time you open Mica.")
                };
                let mut n = Notification::new(Level::Info, title, String::new())
                    .keyed(format!("{KEY}:ready:{}", release.version));
                match self.update_blocker() {
                    None => n = n.action("Restart now", Action::UpdateRestart),
                    Some(why) => text.push_str(&format!(" Restart becomes available once {why}.")),
                }
                n.text = text;
                n.action("Later", Action::UpdateLater)
            }
            Phase::Failed {
                message,
                manual,
                retry,
            } => {
                let (level, title) = match retry {
                    Retry::Check => (
                        if *manual {
                            Level::Error
                        } else {
                            Level::Warning
                        },
                        "Couldn't check for updates",
                    ),
                    Retry::Install(_) => (Level::Error, "The update couldn't be installed"),
                };
                let mut text = format!("{title}: {message}");
                if !*manual {
                    text.push_str(
                        " Mica checks once a day at start; Settings › Files turns this off.",
                    );
                }
                let mut n =
                    Notification::new(level, title, text).keyed(format!("{KEY}:failed:{message}"));
                if self.updater.enabled() {
                    n = n.action("Try again", Action::UpdateRetry);
                }
                if !*manual {
                    n = n.action("Settings", Action::OpenPreferences(PrefsTab::Files));
                }
                n
            }
        };
        let mut n = n;
        // Offers and progress stay until acted on; nothing about updates fades unseen.
        n.life = None;
        Some(n)
    }

    pub(crate) fn run_update_action(&mut self, a: &Action, ctx: &egui::Context) {
        match a {
            Action::UpdateNotes => self.updater.notes_open = true,
            Action::UpdateInstall => {
                if let Some(r) = self.updater.release().cloned() {
                    self.updater.notes_open = false;
                    self.updater.install(r, ctx);
                }
            }
            Action::UpdateLater => match &self.updater.phase {
                Phase::Ready { staged, .. } => {
                    // Windows: the installer runs once Mica quits.
                    if matches!(staged, Staged::Installer(_))
                        && let Err(e) = mica_update::relaunch_after_exit(staged, false)
                    {
                        self.notify_error("The update couldn't be scheduled", e);
                    }
                    // Stays installed (a later check does not offer it again); just hidden.
                    self.updater.later = true;
                }
                _ => {
                    self.updater.later = true;
                    self.updater.notes_open = false;
                }
            },
            Action::UpdateCancel => self.updater.cancel.store(true, Ordering::Relaxed),
            Action::UpdateRestart => {
                if let Some(why) = self.update_blocker() {
                    self.notify_warning(format!("Restart becomes available once {why}."));
                    return;
                }
                if let Phase::Ready { staged, .. } = &self.updater.phase {
                    match mica_update::relaunch_after_exit(staged, true) {
                        Ok(()) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                        Err(e) => self.notify_error("Mica couldn't restart", e),
                    }
                }
            }
            Action::UpdateRetry => match &self.updater.phase {
                Phase::Failed {
                    retry: Retry::Install(r),
                    ..
                } => {
                    let r = r.clone();
                    self.updater.phase = Phase::Idle;
                    self.updater.install(r, ctx);
                }
                Phase::Failed { manual, .. } => {
                    let manual = *manual;
                    self.updater.phase = Phase::Idle;
                    self.updater.check(manual, ctx);
                }
                _ => {}
            },
            _ => {}
        }
    }

    /// "What's new in Mica Slicer 0.x".
    pub(crate) fn update_notes_window(&mut self, ctx: &egui::Context) {
        if !self.updater.notes_open {
            return;
        }
        let Some(release) = self.updater.release().cloned() else {
            self.updater.notes_open = false;
            return;
        };
        let mut open = true;
        let mut action = None;
        egui::Window::new(format!("What's new in Mica Slicer {}", release.version))
            .id(egui::Id::unique("mica-update-notes"))
            .open(&mut open)
            .default_width(460.)
            .collapsible(false)
            .show(ctx, |ui| {
                if let Some(d) = &release.date {
                    ui.weak(format!(
                        "Released {} · you have {}",
                        d.get(..10).unwrap_or(d),
                        mica_update::current_version()
                    ));
                }
                egui::ScrollArea::vertical()
                    .max_height(360.)
                    .show(ui, |ui| {
                        if release.notes.is_empty() {
                            ui.label("No release notes were published for this version.");
                        } else {
                            crate::panels::markdown::show(ui, &release.notes, 13.);
                        }
                    });
                ui.add_space(8.);
                ui.horizontal(|ui| {
                    let available = matches!(self.updater.phase, Phase::Available(_));
                    if crate::panels::widgets::primary_button(ui, "Install", available).clicked() {
                        action = Some(Action::UpdateInstall);
                    }
                    if crate::panels::widgets::button(ui, "Later", true).clicked() {
                        action = Some(Action::UpdateLater);
                    }
                });
            });
        if !open {
            self.updater.notes_open = false;
        }
        if let Some(a) = action {
            self.run_update_action(&a, ctx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daily_check_is_due_once_a_day() {
        let day = CHECK_EVERY.as_secs();
        assert!(due(0, 1_800_000_000));
        assert!(!due(1_800_000_000, 1_800_000_000 + day - 1));
        assert!(due(1_800_000_000, 1_800_000_000 + day));
        // A clock set back: check rather than wait for years.
        assert!(due(1_800_000_000, 1_700_000_000));
    }

    #[test]
    fn a_disabled_updater_says_why_when_asked() {
        let ctx = egui::Context::default();
        let mut u = Updater::disabled("this is a development build");
        assert!(!u.enabled());
        u.check(true, &ctx);
        match &u.phase {
            Phase::Failed {
                message, manual, ..
            } => {
                assert!(message.contains("development build"), "{message}");
                assert!(*manual);
            }
            p => panic!("{p:?}"),
        }
    }

    #[test]
    fn notifications_for_each_phase() {
        let ctx = egui::Context::default();
        let mut app = crate::app::App::create(
            &ctx,
            crate::app::Options {
                prefs_path: None,
                store: Some(mica_profiles::user::Store::open(
                    std::env::temp_dir().join(format!("mica-updater-test-{}", std::process::id())),
                )),
                files: vec![],
                headless: true,
            },
        );
        assert!(app.update_notification().is_none());
        let release = Release {
            version: mica_update::Version::new(9, 1, 0),
            notes: "New things.".into(),
            date: Some("2026-10-19T00:00:00Z".into()),
            url: "http://127.0.0.1:1/x".into(),
            signature: String::new(),
            format: mica_update::Format::App,
        };
        app.updater.phase = Phase::Available(release.clone());
        let n = app.update_notification().unwrap();
        assert_eq!(n.title, "Mica Slicer 9.1.0 is available");
        let labels: Vec<&str> = n.actions.iter().map(|a| a.0.as_str()).collect();
        assert_eq!(labels, ["What's new", "Install", "Later"]);
        assert!(n.life.is_none());
        app.run_update_action(&Action::UpdateNotes, &ctx);
        assert!(app.updater.notes_open);
        app.run_update_action(&Action::UpdateLater, &ctx);
        assert!(app.update_notification().is_none());
        app.updater.later = false;
        app.updater.phase = Phase::Downloading {
            release: release.clone(),
            received: 50,
            total: Some(200),
        };
        assert!(app.update_notification().unwrap().title.ends_with("25 %"));
        app.updater.phase = Phase::Ready {
            release: release.clone(),
            staged: Staged::Replaced("/Applications/Mica Slicer.app".into()),
        };
        let n = app.update_notification().unwrap();
        assert!(n.actions.iter().any(|a| a.1 == Action::UpdateRestart));
        // Failures are visible: a quiet one as a warning with a way to turn checks off.
        app.updater.phase = Phase::Failed {
            message: "the update server could not be reached".into(),
            manual: false,
            retry: Retry::Check,
        };
        let n = app.update_notification().unwrap();
        assert_eq!(n.level, Level::Warning);
        assert!(n.text.contains("could not be reached"));
        assert!(
            n.actions
                .iter()
                .any(|a| a.1 == Action::OpenPreferences(PrefsTab::Files))
        );
        // The headless app never checks by itself.
        app.updater.phase = Phase::Idle;
        app.prefs.check_updates = true;
        app.startup_update_check(&ctx);
        assert_eq!(app.updater.phase, Phase::Idle);
    }
}
