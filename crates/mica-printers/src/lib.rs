// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The physical printers Mica knows, shared by the window and the web dashboard: the
//! `printers.toml` list ([`config`]), where their secrets live ([`secrets`]), which camera of
//! a printer to play ([`camera`]) and how often a printer is asked for its status
//! ([`poll`]). Talking to a printer is `mica-printhost`'s job, playing a camera
//! `mica-camera`'s; nothing here draws.

pub mod camera;
pub mod config;
pub mod poll;
pub mod secrets;
// Used on Linux and other Unix systems; built (and tested) on macOS too.
#[cfg(unix)]
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub mod secrets_file;

use mica_printhost::PrinterHost;
use std::sync::Arc;

/// A connection factory for one printer, its secret already read on the calling thread (so
/// a keychain is never touched from a worker).
pub type Connector = Arc<dyn Fn() -> Result<Box<dyn PrinterHost>, String> + Send + Sync>;
