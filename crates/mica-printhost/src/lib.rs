// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Blocking, secret-free printer connections. Callers own threading and credential storage.
//! Tool indices are zero based; times are seconds and progress is percent (0..=100).

mod camera;
pub mod colour;
mod connect;
mod digest;
pub mod discovery;
mod http;
mod moonraker;
mod preflight;
mod prusa;
mod snapmaker;
#[cfg(test)]
mod tests;
mod u1;
pub mod u1_mapping;
#[cfg(test)]
mod u1_tests;
mod websocket;

pub use camera::{CameraInfo, CameraService, is_rtsp, web_origin};
pub use connect::{CONNECT_URL, PrusaConnect};
pub use http::fetch_snapshot;
pub use moonraker::Moonraker;
pub use preflight::{Issue, IssueKind, JobRequirements, ToolRequirement, preflight};
pub use prusa::PrusaLink;
pub use snapmaker::SnapmakerU1;
pub use u1::{
    U1Route, U1Slot, U1StartOptions, U1Started, U1TaskConfig, at_least, parse_firmware,
    parse_task_config,
};
pub use websocket::{accept_key, read_client_text, server_text_frame};

use std::{fmt, path::Path, time::Duration};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Capabilities {
    pub upload: bool,
    pub start: bool,
    pub pause_resume: bool,
    pub cancel: bool,
    pub status: bool,
    pub files: bool,
    pub camera: bool,
    pub filaments: bool,
    pub queue: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrinterKind {
    PrusaConnect,
    PrusaLink,
    Moonraker,
    SnapmakerU1,
}

/// Intentionally redacted in Debug output. Never serialized or persisted by this crate.
#[derive(Clone, Default)]
pub enum Credentials {
    #[default]
    None,
    ApiKey(String),
    Digest {
        username: String,
        password: String,
    },
}
impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::None => "None",
            Self::ApiKey(_) => "ApiKey([redacted])",
            Self::Digest { .. } => "Digest([redacted])",
        })
    }
}

#[derive(Clone, Debug)]
pub struct PrinterConfig {
    pub name: String,
    pub kind: PrinterKind,
    pub address: String,
    pub credentials: Credentials,
}

#[derive(Clone, Copy, Debug)]
pub struct Timeouts {
    pub connect: Duration,
    pub request: Duration,
    pub upload: Duration,
}
impl Default for Timeouts {
    fn default() -> Self {
        Self {
            connect: Duration::from_secs(5),
            request: Duration::from_secs(10),
            upload: Duration::from_secs(60),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    Unauthorised,
    Unreachable,
    Busy,
    Protocol,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    pub kind: ErrorKind,
    /// Sanitized diagnostic; never contains credentials or response bodies.
    pub message: String,
    pub http_status: Option<u16>,
}
impl Error {
    pub(crate) fn protocol(message: impl Into<String>) -> Self {
        Self {
            kind: ErrorKind::Protocol,
            message: message.into(),
            http_status: None,
        }
    }
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.message)
    }
}
impl std::error::Error for Error {}
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrinterState {
    Idle,
    Printing,
    Paused,
    Finished,
    Error,
    Offline,
}
impl PrinterState {
    pub(crate) fn parse(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "idle" | "ready" | "standby" | "operational" | "cancelled" | "canceled" => Self::Idle,
            "printing" | "busy" | "heating" => Self::Printing,
            "paused" | "pausing" => Self::Paused,
            "finished" | "complete" | "completed" => Self::Finished,
            "offline" | "disconnected" | "startup" => Self::Offline,
            _ => Self::Error, // Unknown firmware states must never look idle to preflight.
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileFormat {
    Gcode,
    Bgcode,
}
impl FileFormat {
    pub fn from_path(path: &Path) -> Option<Self> {
        match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
            "gcode" | "gco" | "g" => Some(Self::Gcode),
            "bgcode" => Some(Self::Bgcode),
            _ => None,
        }
    }
}
#[derive(Clone, Debug)]
pub struct PrinterInfo {
    pub model: Option<String>,
    pub firmware: Option<String>,
    pub state: PrinterState,
    /// None means unknown, not zero tools.
    pub tools: Option<Vec<u32>>,
    pub formats: Vec<FileFormat>,
    pub bed_type: Option<String>,
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Temperature {
    pub current: Option<f64>,
    pub target: Option<f64>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct ToolTemperature {
    pub tool: u32,
    pub temperature: Temperature,
}
#[derive(Clone, Debug, PartialEq)]
pub struct LoadedFilament {
    pub tool: u32,
    pub material: Option<String>,
    pub colour: Option<String>,
}
#[derive(Clone, Debug)]
pub struct PrinterStatus {
    pub state: PrinterState,
    pub job_name: Option<String>,
    pub progress: Option<f64>,
    pub elapsed: Option<f64>,
    pub remaining: Option<f64>,
    pub current_layer: Option<u64>,
    pub z: Option<f64>,
    pub hotends: Vec<ToolTemperature>,
    pub bed: Option<Temperature>,
    pub chamber: Option<Temperature>,
}
impl PrinterStatus {
    pub(crate) fn empty(state: PrinterState) -> Self {
        Self {
            state,
            job_name: None,
            progress: None,
            elapsed: None,
            remaining: None,
            current_layer: None,
            z: None,
            hotends: vec![],
            bed: None,
            chamber: None,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Job {
    pub name: String,
    pub id: Option<String>,
    pub started: bool,
}
/// Requested action after an upload. Cloud acceptance never confirms physical printing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UploadAction {
    UploadOnly,
    Print,
    Queue,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UploadOutcome {
    pub job: Job,
    /// Bounded, plain server acceptance message (credentials redacted).
    pub message: Option<String>,
    pub warning: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct UploadProgress {
    pub sent: u64,
    pub total: u64,
}
/// Reports file bytes read for the network, not printer acknowledgement. Must not panic.
pub type ProgressCallback<'a> = dyn FnMut(UploadProgress) + 'a;

pub trait PrinterHost: Send + Sync {
    fn capabilities(&self) -> Capabilities;
    fn info(&self) -> Result<PrinterInfo>;
    fn upload(&self, path: &Path, remote_name: &str, start_after: bool) -> Result<Job> {
        self.upload_with_progress(path, remote_name, start_after, &mut |_| {})
    }
    fn upload_with_progress(
        &self,
        path: &Path,
        remote_name: &str,
        start_after: bool,
        progress: &mut ProgressCallback<'_>,
    ) -> Result<Job>;
    /// Extended upload options; existing LAN hosts keep their upload/start behavior.
    fn upload_action_with_progress(
        &self,
        path: &Path,
        remote_name: &str,
        action: UploadAction,
        progress: &mut ProgressCallback<'_>,
    ) -> Result<UploadOutcome> {
        if action == UploadAction::Queue {
            return Err(Error::protocol("this host does not support a print queue"));
        }
        self.upload_with_progress(path, remote_name, action == UploadAction::Print, progress)
            .map(|job| UploadOutcome {
                job,
                message: None,
                warning: false,
            })
    }
    fn start(&self, name: &str) -> Result<()>;
    fn pause(&self) -> Result<()>;
    fn resume(&self) -> Result<()>;
    fn cancel(&self) -> Result<()>;
    fn status(&self) -> Result<PrinterStatus>;
    /// Empty means unsupported or no known loaded slots; use capabilities after querying.
    fn loaded_filaments(&self) -> Result<Vec<LoadedFilament>> {
        Ok(vec![])
    }
    /// The cameras this printer reports (empty: none, or the API has no camera list).
    fn cameras(&self) -> Result<Vec<CameraInfo>> {
        Ok(vec![])
    }
    /// A snapshot served by the printer's own API (`CameraInfo::printer_snapshot`), sent
    /// with the printer's credentials. `Ok(None)`: no picture yet. Read-only (a GET).
    fn camera_snapshot(&self, _path: &str, _limit: usize) -> Result<Option<Vec<u8>>> {
        Err(Error::protocol("this printer serves no camera snapshots"))
    }
    /// The preview picture embedded in the current job's file (`job` as `status` names it),
    /// as the printer serves it (PNG or JPEG bytes, at most `limit`). `Ok(None)`: the
    /// printer has none for this file, or its API cannot serve one. Read-only (GETs).
    fn job_thumbnail(&self, _job: &str, _limit: usize) -> Result<Option<Vec<u8>>> {
        Ok(None)
    }
    /// The slicer the printer's file index names for an uploaded file (Moonraker's
    /// `/server/files/metadata` `slicer`, e.g. `OrcaSlicer` or `Unknown`). `Ok(None)`: no
    /// metadata (yet), or this printer has no such index. Read-only (a GET).
    fn file_slicer(&self, _name: &str) -> Result<Option<String>> {
        Ok(None)
    }
    /// The first camera's credential-free snapshot URL, if it has one.
    fn camera_snapshot_url(&self) -> Result<Option<String>> {
        Ok(self.cameras()?.into_iter().find_map(|c| c.snapshot_url))
    }
    fn preflight(&self, job: &JobRequirements) -> Vec<Issue> {
        preflight(self, job)
    }
    /// The Snapmaker U1 extras (slot map at start), when this is one.
    fn as_u1(&self) -> Option<&SnapmakerU1> {
        None
    }
}

pub fn connect(config: PrinterConfig, timeouts: Timeouts) -> Result<Box<dyn PrinterHost>> {
    match config.kind {
        PrinterKind::PrusaConnect => Ok(Box::new(PrusaConnect::new(config, timeouts)?)),
        PrinterKind::PrusaLink => Ok(Box::new(PrusaLink::new(config, timeouts)?)),
        PrinterKind::Moonraker => Ok(Box::new(Moonraker::new(config, timeouts)?)),
        PrinterKind::SnapmakerU1 => Ok(Box::new(SnapmakerU1::new(config, timeouts)?)),
    }
}

pub(crate) fn string(v: &serde_json::Value) -> Option<String> {
    v.as_str().filter(|s| !s.is_empty()).map(str::to_owned)
}
pub(crate) fn number(v: &serde_json::Value) -> Option<f64> {
    v.as_f64().filter(|n| n.is_finite())
}
pub(crate) fn temperature(v: &serde_json::Value) -> Option<Temperature> {
    let t = Temperature {
        current: number(&v["temperature"]),
        target: number(&v["target"]),
    };
    (t.current.is_some() || t.target.is_some()).then_some(t)
}
