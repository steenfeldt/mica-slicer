// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Native file dialogs that never block a frame. A blocking dialog runs the system's modal
//! loop inside the frame; on macOS, resizing the panel then delivers window events to winit
//! while it is still handling the frame, and winit aborts the process. The async dialog
//! opens as a sheet on the window instead, and each frame polls it until it is answered.

use crate::app::App;
use eframe::egui;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};

type Answer = Pin<Box<dyn Future<Output = Vec<PathBuf>> + Send>>;
type Then = Box<dyn FnOnce(&mut App, Vec<PathBuf>, &egui::Context)>;

/// A dialog on screen and what runs with the chosen paths.
pub struct Pending {
    answer: Answer,
    then: Then,
    waker: Waker,
}

/// What the dialog asks for.
pub enum Ask {
    File,
    Files,
    Folder,
    Save,
}

/// Wakes the UI when the dialog is answered.
struct Repaint(egui::Context);

impl Wake for Repaint {
    fn wake(self: Arc<Self>) {
        self.0.request_repaint();
    }
}

fn paths(handles: Option<Vec<rfd::FileHandle>>) -> Vec<PathBuf> {
    handles
        .unwrap_or_default()
        .into_iter()
        .map(|h| h.path().to_path_buf())
        .collect()
}

impl App {
    /// Shows `dialog`; `then` runs with the chosen paths once it is answered (not when it is
    /// cancelled). One dialog at a time: a second request while one is open is ignored.
    pub(crate) fn ask_paths(
        &mut self,
        ctx: &egui::Context,
        dialog: rfd::AsyncFileDialog,
        ask: Ask,
        then: impl FnOnce(&mut App, Vec<PathBuf>, &egui::Context) + 'static,
    ) {
        // Headless runs (tests, screenshots, the monkey test) have no window for a sheet, and
        // rfd would fall back to a blocking dialog.
        if self.headless {
            self.status = "This needs the file dialog; pass files in headless mode".into();
            return;
        }
        if self.file_dialog.is_some() {
            return;
        }
        let answer: Answer = match ask {
            Ask::File => Box::pin(async move { paths(dialog.pick_file().await.map(|h| vec![h])) }),
            Ask::Files => Box::pin(async move { paths(dialog.pick_files().await) }),
            Ask::Folder => {
                Box::pin(async move { paths(dialog.pick_folder().await.map(|h| vec![h])) })
            }
            Ask::Save => Box::pin(async move { paths(dialog.save_file().await.map(|h| vec![h])) }),
        };
        self.file_dialog = Some(Pending {
            answer,
            then: Box::new(then),
            waker: Waker::from(Arc::new(Repaint(ctx.clone()))),
        });
        self.poll_file_dialog(ctx);
    }

    /// The open dialog's answer, once there is one.
    pub(crate) fn poll_file_dialog(&mut self, ctx: &egui::Context) {
        let Some(pending) = &mut self.file_dialog else {
            return;
        };
        let waker = pending.waker.clone();
        let Poll::Ready(chosen) = pending
            .answer
            .as_mut()
            .poll(&mut Context::from_waker(&waker))
        else {
            return;
        };
        let Some(pending) = self.file_dialog.take() else {
            return;
        };
        if !chosen.is_empty() {
            (pending.then)(self, chosen, ctx);
        }
    }
}
