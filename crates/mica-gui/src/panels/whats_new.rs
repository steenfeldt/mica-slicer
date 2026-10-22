// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! "What's new in Mica <version>": this version's release notes, shown once after an update
//! (`welcome::launch_windows`) and from Help › What's new….
//!
//! The notes are the `## <version>` section of the repository's `CHANGELOG.md`, compiled in.
//! `scripts/update-manifest.py --changelog` reads the same section with the same rules for the
//! GitHub release and the update manifest, so release notes are written once.

use super::{markdown, widgets};
use crate::app::App;
use crate::theme;
use eframe::egui::{self, Align, Key, Layout, RichText};

/// The changelog this build was made from.
pub const CHANGELOG: &str = include_str!("../../../../CHANGELOG.md");
/// The running version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Every release, newest first (pre-releases included).
pub const RELEASES: &str = concat!(env!("CARGO_PKG_REPOSITORY"), "/releases");

const WINDOW_W: f32 = 520.;

/// The body of `changelog`'s `## <version>` section (a leading `v` and text after the version
/// on the heading line are allowed), trimmed; `None` when missing or empty. Same rules as
/// `changelog_notes` in `scripts/update-manifest.py`.
pub fn notes_for(changelog: &str, version: &str) -> Option<String> {
    let mut body: Option<Vec<&str>> = None;
    for line in changelog.lines() {
        if let Some(head) = line.strip_prefix("## ") {
            if body.is_some() {
                break;
            }
            let word = head.split_whitespace().next().unwrap_or("");
            if word.strip_prefix('v').unwrap_or(word) == version {
                body = Some(vec![]);
            }
            continue;
        }
        if let Some(b) = body.as_mut() {
            b.push(line);
        }
    }
    let text = body?.join("\n").trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// This build's release notes.
pub fn notes() -> Option<String> {
    notes_for(CHANGELOG, VERSION)
}

impl App {
    pub(crate) fn open_whats_new(&mut self) {
        self.dialogs.whats_new = true;
    }

    pub(crate) fn whats_new_window(&mut self, ctx: &egui::Context) {
        if !self.dialogs.whats_new {
            return;
        }
        let mut open = true;
        let mut done = ctx.input(|i| i.key_pressed(Key::Escape))
            && !widgets::field_editing(ctx)
            && !ctx.egui_wants_keyboard_input();
        let height = (ctx.content_rect().height() - 200.).clamp(220., 460.);
        egui::Window::new(format!("What's new in Mica {VERSION}"))
            .id(egui::Id::unique("mica-whats-new"))
            .open(&mut open)
            .default_pos(ctx.content_rect().center_top() + egui::vec2(-WINDOW_W / 2., 90.))
            .default_width(WINDOW_W)
            .resizable(false)
            .collapsible(false)
            .show(ctx, |ui| {
                let p = theme::of(ui);
                ui.set_width(WINDOW_W);
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!("Version {VERSION}"))
                            .font(theme::mono(12.))
                            .color(p.muted),
                    );
                    widgets::status_badge(ui, "Alpha (pre-release)");
                });
                ui.add_space(8.);
                egui::ScrollArea::vertical()
                    .max_height(height)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.set_width(WINDOW_W - 12.);
                        match notes() {
                            Some(text) => markdown::show(ui, &text, 13.),
                            None => {
                                ui.label(
                                    RichText::new(
                                        "No release notes were written for this version.",
                                    )
                                    .font(theme::sans(13.))
                                    .color(p.muted),
                                );
                            }
                        }
                    });
                ui.add_space(10.);
                ui.horizontal(|ui| {
                    if widgets::link(ui, "All releases")
                        .on_hover_text(RELEASES)
                        .clicked()
                    {
                        ui.ctx().open_url(egui::OpenUrl::new_tab(RELEASES));
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if widgets::primary_button(ui, "Done", true).clicked() {
                            done = true;
                        }
                    });
                });
            });
        if !open || done {
            self.dialogs.whats_new = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sections_by_version() {
        let log = "# Changelog\n\nIntro.\n\n## 0.2.0 (2026-11-01)\n\n- New\n\n### Fixes\n- \
                   Fixed\n\n## v0.1.0\n\nFirst.\n\n## 0.0.9\n\n";
        assert_eq!(
            notes_for(log, "0.2.0").as_deref(),
            Some("- New\n\n### Fixes\n- Fixed")
        );
        assert_eq!(notes_for(log, "0.1.0").as_deref(), Some("First."));
        // An empty section, a missing one, a prefix of a version.
        assert_eq!(notes_for(log, "0.0.9"), None);
        assert_eq!(notes_for(log, "0.3.0"), None);
        assert_eq!(notes_for(log, "0.2"), None);
    }

    #[test]
    fn this_version_has_notes() {
        // A release build without its notes would show none: CHANGELOG.md must have them.
        assert!(
            notes().is_some(),
            "CHANGELOG.md has no '## {VERSION}' section"
        );
    }

    #[test]
    fn python_reads_the_same_section() {
        // On Windows `python3` may be the Store placeholder, which starts but fails.
        if !std::process::Command::new("python3")
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
        {
            eprintln!("skipped: no python3");
            return;
        }
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let out = std::process::Command::new("python3")
            .arg("-I")
            .arg(root.join("scripts/update-manifest.py"))
            .args(["--notes-only", "--changelog"])
            .arg(root.join("CHANGELOG.md"))
            .args(["--version", VERSION])
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        assert_eq!(
            // The script writes UTF-8 bytes with `\n` on every OS; `\r\n` is tolerated anyway.
            String::from_utf8(out.stdout.clone())
                .expect("the notes are UTF-8")
                .replace("\r\n", "\n")
                .trim_end(),
            notes().unwrap()
        );
    }
}
