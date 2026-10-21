// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The Print dialog's state machine: pick a printer → check it (preflight on a thread) →
//! upload with progress → done or failed. Events from the threads carry the generation
//! they belong to, so switching printers mid-check discards the stale answer.
//!
//! A Snapmaker U1 also reports its slots and start flags at the check; the dialog then holds
//! a `u1map::Plan` (tool → slot, start options) and starts the job with it after uploading.

use super::{
    config::Kind,
    explain::{Explanation, JobTool, Preflight, explain, present},
    monitor::Connector,
    u1map::{Plan, Remembered},
};
use mica_printhost::{
    Error, FileFormat, Issue, IssueKind, JobRequirements, LoadedFilament, PrinterInfo,
    PrinterStatus, ToolRequirement, U1Started, U1TaskConfig, UploadAction, UploadOutcome,
};
use std::{
    path::PathBuf,
    sync::{Arc, mpsc},
    thread,
};

/// Keeps a slice's temporary folder (and so its file) alive.
type Hold = Arc<crate::worker::TempJob>;

/// The sliced file to send and what the job needs of the printer.
#[derive(Clone)]
pub struct JobFile {
    pub path: PathBuf,
    pub format: FileFormat,
    pub tools: Vec<JobTool>,
    pub bed_type: Option<String>,
    /// Keeps the temporary slice directory alive while the dialog is open.
    pub hold: Option<Arc<crate::worker::TempJob>>,
}
impl JobFile {
    pub fn requirements(&self) -> JobRequirements {
        JobRequirements {
            tools: self
                .tools
                .iter()
                .map(|t| ToolRequirement {
                    tool: t.tool,
                    material: t.material.clone(),
                    colour: t.colour.map(crate::project::hex),
                })
                .collect(),
            format: self.format,
            bed_type: self.bed_type.clone(),
            needs_chamber_sensor: false,
        }
    }
}

#[derive(Debug)]
pub struct Checked {
    pub info: PrinterInfo,
    pub status: PrinterStatus,
    pub filaments: Vec<LoadedFilament>,
    pub issues: Vec<Issue>,
    /// A U1's slots and start flags, and its firmware version.
    pub u1: Option<(U1TaskConfig, Option<String>)>,
}

#[derive(Debug)]
pub enum Event {
    Checked(Result<Box<Checked>, Error>),
    /// The connection could not be made at all (missing secret, bad address).
    Unavailable(String),
    Progress(u64, u64),
    Uploaded(UploadOutcome),
    /// A U1 job uploaded and started with its slot map.
    StartedU1(U1Started),
    UploadFailed(Error),
    /// Sending all plates: the thread is done (every file sent, or it stopped).
    BatchEnd,
}

/// One plate's file when the Print dialog sends all plates.
#[derive(Clone)]
pub struct BatchItem {
    /// The plate's name.
    pub plate: String,
    pub file: JobFile,
    /// The file's name on the printer (as Export all plates names it).
    pub remote: String,
    pub state: ItemState,
}

impl BatchItem {
    pub fn new(plate: String, file: JobFile, remote: String) -> Self {
        Self {
            plate,
            file,
            remote,
            state: ItemState::Waiting,
        }
    }
}

/// Where one file of a send of all plates stands.
#[derive(Clone, Debug, PartialEq)]
pub enum ItemState {
    Waiting,
    Uploading {
        sent: u64,
        total: u64,
    },
    Done {
        /// The printer started it.
        started: bool,
        /// Prusa Connect put it in the queue.
        queued: bool,
        message: Option<String>,
        warning: bool,
    },
    Failed(Explanation),
    /// Left out after a failure ("Skip failed").
    Skipped,
}

/// The Print dialog's plate choice when the project has several plates: this plate, or all
/// of them (each plate's file uploaded in turn).
#[derive(Clone, Default)]
pub struct Plates {
    /// How many plates have something to print.
    pub count: usize,
    pub all: bool,
    /// All plates waits for the plates' slices.
    pub preparing: bool,
    pub items: Vec<BatchItem>,
    /// Plates without a file and why ("Plate 2: brim doesn't fit on the bed").
    pub skipped: Vec<String>,
    /// Why All plates could not be prepared (slicing canceled).
    pub problem: Option<String>,
    /// The upload thread runs.
    pub sending: bool,
    /// The current plate's file and name, back when This plate is chosen again.
    single: Option<(JobFile, String)>,
}

impl Plates {
    pub fn new(count: usize) -> Self {
        Self {
            count,
            ..Self::default()
        }
    }
    /// The files All plates sends now (none while it is not chosen or still slicing).
    pub fn sending_all(&self) -> bool {
        self.all && !self.preparing && !self.items.is_empty()
    }
    pub fn failed(&self) -> usize {
        self.items
            .iter()
            .filter(|i| matches!(i.state, ItemState::Failed(_)))
            .count()
    }
    pub fn done(&self) -> usize {
        self.items
            .iter()
            .filter(|i| matches!(i.state, ItemState::Done { .. }))
            .count()
    }
    /// Files still to send (not sent yet, or failed).
    pub fn to_send(&self) -> usize {
        self.items
            .iter()
            .filter(|i| matches!(i.state, ItemState::Waiting | ItemState::Failed(_)))
            .count()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Stage {
    /// No printer chosen yet, or the chosen one not checked.
    Pick,
    Checking,
    Checked(Box<Preflight>),
    CheckFailed(Explanation),
    /// Cloud readiness cannot be checked with the API key. No request is made here.
    CloudReady,
    Uploading {
        sent: u64,
        total: u64,
    },
    Done {
        job: String,
        started: bool,
        message: Option<String>,
        warning: bool,
    },
    Failed(Explanation),
}

/// What a Moonraker printer's file index said about a file Mica just uploaded: the slicer
/// its metadata names (`Unknown` when its firmware does not recognise the header).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlicerReport {
    /// `Printer::id`.
    pub printer: u64,
    pub file: String,
    /// The identity the file was written with (`mica` / `orca_compatible`).
    pub identity: &'static str,
    pub slicer: String,
}

/// The Mica identity a local text G-code file carries, from its first 16 KiB.
pub fn file_identity(path: &std::path::Path) -> Option<&'static str> {
    use std::io::Read;
    let mut head = vec![0; 16 * 1024];
    let mut f = std::fs::File::open(path).ok()?;
    let n = f.read(&mut head).ok()?;
    mica_gcode::identity::of_text(&String::from_utf8_lossy(&head[..n]))
}

/// After an upload: asks the printer which slicer its metadata names for the file (it may
/// take Moonraker a moment to index it). Read-only; `None` when it never says.
fn read_slicer(h: &dyn mica_printhost::PrinterHost, remote: &str) -> Option<String> {
    for attempt in 0..5 {
        if attempt > 0 {
            thread::sleep(std::time::Duration::from_millis(400));
        }
        match h.file_slicer(remote) {
            Ok(Some(s)) => return Some(s),
            Ok(None) => {}
            Err(_) => return None,
        }
    }
    None
}

/// Who the dialog talks to, for messages and the connection.
#[derive(Clone, Debug, PartialEq)]
pub struct Target {
    /// `Printer::id`: the list position changes when printers are removed.
    pub id: u64,
    pub name: String,
    pub kind: Kind,
    pub address: String,
}

pub struct Dialog {
    pub file: JobFile,
    pub target: Option<Target>,
    pub start_after: bool,
    pub queue: bool,
    pub file_name: String,
    pub stage: Stage,
    /// A U1's slot map and start options, once its check has answered.
    pub u1: Option<Plan>,
    /// The start options last sent to the chosen U1 (set by the app when it is chosen).
    pub u1_remembered: Option<Remembered>,
    /// How the last start went, for the result line (U1: which route).
    pub note: Option<String>,
    /// An opened G-code file with validation errors: the dialog is asking whether to send it
    /// anyway, and whether the answer was yes.
    pub asking_errors: bool,
    pub errors_accepted: bool,
    /// The model the chosen printer reported at the check (`Original Prusa XL`).
    pub reported_model: Option<String>,
    /// Where the upload thread reports what a Moonraker printer's metadata says about the
    /// uploaded file (set by the app; the report outlives the dialog).
    pub identity_reports: Option<mpsc::Sender<SlicerReport>>,
    /// This plate / All plates, when the project has several plates with objects.
    pub plates: Option<Plates>,
    rx: Option<mpsc::Receiver<(u64, Event)>>,
    /// Sending all plates: events tagged with the file's index.
    batch_rx: Option<mpsc::Receiver<(u64, usize, Event)>>,
    generation: u64,
}
impl Dialog {
    pub fn new(file: JobFile, file_name: String) -> Self {
        Self {
            file,
            target: None,
            start_after: true,
            queue: false,
            file_name,
            stage: Stage::Pick,
            u1: None,
            u1_remembered: None,
            note: None,
            asking_errors: false,
            errors_accepted: false,
            reported_model: None,
            identity_reports: None,
            plates: None,
            rx: None,
            batch_rx: None,
            generation: 0,
        }
    }

    /// This plate (`false`) or All plates (`true`). All plates waits for the plates' files
    /// (`set_plates`); the app slices what is missing meanwhile.
    pub fn choose_all(&mut self, all: bool) {
        let Some(p) = &mut self.plates else { return };
        if p.all == all || p.sending {
            return;
        }
        p.all = all;
        p.problem = None;
        p.skipped.clear();
        if all {
            p.preparing = true;
            p.items.clear();
        } else {
            p.preparing = false;
            p.items.clear();
            if let Some((file, name)) = p.single.take() {
                self.file = file;
                self.file_name = name;
                self.recheck();
            }
        }
    }

    /// All plates' files are ready: the first plate's file is the one the check and a U1's
    /// slot map are about (it is the one that may start).
    pub fn set_plates(&mut self, items: Vec<BatchItem>, skipped: Vec<String>) {
        let Some(p) = &mut self.plates else { return };
        if !p.all {
            return;
        }
        p.preparing = false;
        p.skipped = skipped;
        p.problem = None;
        if let Some(first) = items.first() {
            if p.single.is_none() {
                p.single = Some((self.file.clone(), self.file_name.clone()));
            }
            self.file = first.file.clone();
            self.file_name = first.remote.clone();
        } else {
            p.problem = Some("No plate could be sliced".into());
        }
        p.items = items;
        self.recheck();
    }

    /// Preparing All plates stopped (`why`: "Canceled", a settings problem).
    pub fn plates_failed(&mut self, why: String) {
        if let Some(p) = &mut self.plates
            && p.preparing
        {
            p.preparing = false;
            p.problem = Some(why);
        }
    }

    /// The file changed: the chosen printer is checked again (the next frame).
    fn recheck(&mut self) {
        if self.target.is_some() && !self.busy() {
            self.stage = Stage::Pick;
            self.u1 = None;
            self.note = None;
            self.generation += 1;
            self.rx = None;
        }
    }

    /// Whether All plates is chosen with its files ready.
    pub fn sending_all(&self) -> bool {
        self.plates.as_ref().is_some_and(Plates::sending_all)
    }

    /// Whether All plates can send (or retry) now.
    pub fn can_upload_all(&self) -> bool {
        self.sending_all()
            && self
                .plates
                .as_ref()
                .is_some_and(|p| !p.sending && p.to_send() > 0)
            && self.target.is_some()
            && matches!(
                self.stage,
                Stage::Checked(_) | Stage::CloudReady | Stage::Failed(_)
            )
            && (!self.start_after || self.u1.as_ref().is_none_or(|p| p.options().is_some()))
    }

    /// Every file of All plates was sent or skipped.
    pub fn batch_complete(&self) -> bool {
        self.plates.as_ref().is_some_and(|p| {
            p.sending_all()
                && !p.sending
                && p.done() > 0
                && p.items
                    .iter()
                    .all(|i| matches!(i.state, ItemState::Done { .. } | ItemState::Skipped))
        })
    }

    /// "Skip failed": the failed files are left out.
    pub fn skip_failed(&mut self) {
        if let Some(p) = &mut self.plates {
            for i in &mut p.items {
                if matches!(i.state, ItemState::Failed(_)) {
                    i.state = ItemState::Skipped;
                }
            }
        }
    }

    /// What file `index` of All plates asks of the printer: the first plate starts when "start
    /// printing" is chosen; on Prusa Connect the others go to the queue, elsewhere they are
    /// only uploaded.
    pub fn item_action(&self, index: usize) -> UploadAction {
        let connect = self
            .target
            .as_ref()
            .is_some_and(|t| t.kind == Kind::PrusaConnect);
        if index == 0 && self.start_after {
            UploadAction::Print
        } else if connect {
            UploadAction::Queue
        } else {
            UploadAction::UploadOnly
        }
    }

    /// Sends All plates' files that are not sent yet (or failed), one after the other on a
    /// thread. The file that starts goes last, so the printer is idle for the other uploads.
    /// A failure that concerns the connection (unreachable, refused credentials) stops the
    /// rest; one about a file goes on with the next.
    pub fn upload_all(
        &mut self,
        connect: Result<Connector, String>,
        wake: impl Fn() + Send + Sync + 'static,
    ) {
        if !self.can_upload_all() {
            return;
        }
        let connect = match connect {
            Ok(c) => c,
            Err(e) => {
                let name = self
                    .target
                    .as_ref()
                    .map(|t| t.name.clone())
                    .unwrap_or_default();
                if let Some(p) = &mut self.plates {
                    for i in &mut p.items {
                        if matches!(i.state, ItemState::Waiting | ItemState::Failed(_)) {
                            i.state = ItemState::Failed(Explanation {
                                headline: format!("Mica cannot connect to {name}"),
                                advice: e.clone(),
                            });
                        }
                    }
                }
                return;
            }
        };
        let plates = self.plates.as_mut().expect("sending all");
        let mut order: Vec<usize> = (0..plates.items.len())
            .filter(|&i| {
                matches!(
                    plates.items[i].state,
                    ItemState::Waiting | ItemState::Failed(_)
                )
            })
            .collect();
        for &i in &order {
            plates.items[i].state = ItemState::Waiting;
        }
        plates.sending = true;
        if let Some(pos) = order.iter().position(|&i| i == 0)
            && self.start_after
        {
            let first = order.remove(pos);
            order.push(first);
        }
        let jobs: Vec<(usize, PathBuf, String, UploadAction, Option<Hold>)> = order
            .iter()
            .map(|&i| {
                let item = &self.plates.as_ref().expect("plates").items[i];
                (
                    i,
                    item.file.path.clone(),
                    item.remote.clone(),
                    self.item_action(i),
                    item.file.hold.clone(),
                )
            })
            .collect();
        let mapped = self.u1.as_ref().and_then(Plan::options);
        self.generation += 1;
        let generation = self.generation;
        let (tx, rx) = mpsc::channel();
        self.batch_rx = Some(rx);
        thread::Builder::new()
            .name("mica-print-upload-all".into())
            .spawn(move || {
                let send = |i: usize, e: Event| {
                    let _ = tx.send((generation, i, e));
                    wake();
                };
                let h = match connect() {
                    Ok(h) => h,
                    Err(e) => {
                        if let Some(&(i, ..)) = jobs.first() {
                            send(i, Event::Unavailable(e));
                        }
                        send(usize::MAX, Event::BatchEnd);
                        return;
                    }
                };
                for (i, path, remote, action, hold) in jobs {
                    let mut last = std::time::Instant::now();
                    let mut first = true;
                    let mut report = |p: mica_printhost::UploadProgress| {
                        if first || p.sent == p.total || last.elapsed().as_millis() >= 50 {
                            first = false;
                            last = std::time::Instant::now();
                            send(i, Event::Progress(p.sent, p.total));
                        }
                    };
                    let event = match (action, &mapped, h.as_u1()) {
                        (UploadAction::Print, Some(o), Some(u1)) => {
                            match u1.start_with_mapping(&path, &remote, o, &mut report) {
                                Ok(s) => Event::StartedU1(s),
                                Err(e) => Event::UploadFailed(e),
                            }
                        }
                        _ => {
                            match h.upload_action_with_progress(&path, &remote, action, &mut report)
                            {
                                Ok(job) => Event::Uploaded(job),
                                Err(e) => Event::UploadFailed(e),
                            }
                        }
                    };
                    let stop = matches!(
                        &event,
                        Event::UploadFailed(e) if matches!(
                            e.kind,
                            mica_printhost::ErrorKind::Unreachable
                                | mica_printhost::ErrorKind::Unauthorised
                        )
                    );
                    send(i, event);
                    drop(hold);
                    if stop {
                        break;
                    }
                }
                send(usize::MAX, Event::BatchEnd);
            })
            .ok();
    }

    /// One event of the All plates thread.
    fn apply_item(&mut self, index: usize, event: Event) -> bool {
        let Some(target) = self.target.clone() else {
            return false;
        };
        let explained = |e: &Error| explain(e, target.kind, &target.name, &target.address);
        let action = self.item_action(index);
        let Some(p) = &mut self.plates else {
            return false;
        };
        if let Event::BatchEnd = event {
            p.sending = false;
            self.batch_rx = None;
            return true;
        }
        let Some(item) = p.items.get_mut(index) else {
            return false;
        };
        let mut note = None;
        item.state = match event {
            Event::Progress(sent, total) => ItemState::Uploading { sent, total },
            Event::Uploaded(job) => ItemState::Done {
                started: job.job.started,
                queued: action == UploadAction::Queue,
                message: job.message,
                warning: job.warning,
            },
            Event::StartedU1(s) => {
                note = Some(s.summary());
                ItemState::Done {
                    started: s.job.started,
                    queued: false,
                    message: None,
                    warning: false,
                }
            }
            Event::UploadFailed(e) => ItemState::Failed(explained(&e)),
            Event::Unavailable(e) => ItemState::Failed(Explanation {
                headline: format!("Mica cannot connect to {}", target.name),
                advice: e,
            }),
            Event::Checked(_) | Event::BatchEnd => return false,
        };
        if note.is_some() {
            self.note = note;
        }
        true
    }
    /// Chooses (or clears) the printer; anything in flight for the old one is ignored.
    pub fn choose(&mut self, target: Option<Target>) {
        if self.target == target && self.stage != Stage::Pick {
            return;
        }
        self.target = target;
        self.queue = false;
        self.stage = Stage::Pick;
        self.u1 = None;
        self.u1_remembered = None;
        self.note = None;
        self.reported_model = None;
        self.generation += 1;
        self.rx = None;
        self.batch_rx = None;
        if let Some(p) = &mut self.plates {
            p.sending = false;
            for i in &mut p.items {
                i.state = ItemState::Waiting;
            }
        }
    }
    /// The remote file name with the right extension for the format.
    pub fn remote_name(&self) -> String {
        let ext = match self.file.format {
            FileFormat::Bgcode => "bgcode",
            FileFormat::Gcode => "gcode",
        };
        let name = self.file_name.trim();
        let name = if name.is_empty() { "print" } else { name };
        let stem = name
            .strip_suffix(".bgcode")
            .or_else(|| name.strip_suffix(".gcode"))
            .unwrap_or(name);
        format!("{stem}.{ext}")
    }
    pub fn busy(&self) -> bool {
        matches!(self.stage, Stage::Checking | Stage::Uploading { .. })
            || self.plates.as_ref().is_some_and(|p| p.sending)
    }
    pub fn can_upload(&self) -> bool {
        self.target.is_some()
            && matches!(
                self.stage,
                Stage::Checked(_) | Stage::CloudReady | Stage::Failed(_)
            )
            && (!self.start_after || self.u1.as_ref().is_none_or(|p| p.options().is_some()))
    }
    pub fn preflight(&self) -> Option<&Preflight> {
        match &self.stage {
            Stage::Checked(p) => Some(p),
            _ => None,
        }
    }

    /// A connection that cannot even be attempted (missing secret, bad address).
    fn unavailable(&mut self, e: String) {
        let name = self
            .target
            .as_ref()
            .map(|t| t.name.clone())
            .unwrap_or_default();
        let x = Explanation {
            headline: format!("Mica cannot connect to {name}"),
            advice: e,
        };
        self.stage = if matches!(self.stage, Stage::Uploading { .. }) {
            Stage::Failed(x)
        } else {
            Stage::CheckFailed(x)
        };
        self.rx = None;
    }

    /// Runs the preflight on a thread; `connect` makes the connection there.
    pub fn check(&mut self, connect: Result<Connector, String>, wake: impl Fn() + Send + 'static) {
        if self.target.is_none() {
            return;
        }
        if self
            .target
            .as_ref()
            .is_some_and(|t| t.kind == Kind::PrusaConnect)
        {
            self.stage = Stage::CloudReady;
            return;
        }
        self.generation += 1;
        self.stage = Stage::Checking;
        let connect = match connect {
            Ok(c) => c,
            Err(e) => return self.unavailable(e),
        };
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        let generation = self.generation;
        let requirements = self.file.requirements();
        thread::Builder::new()
            .name("mica-print-check".into())
            .spawn(move || {
                let event = match connect() {
                    Err(e) => Event::Unavailable(e),
                    Ok(h) => Event::Checked(check(h.as_ref(), &requirements).map(Box::new)),
                };
                let _ = tx.send((generation, event));
                wake();
            })
            .ok();
    }

    /// Uploads (and starts, when asked) on a thread, reporting progress.
    pub fn upload(
        &mut self,
        connect: Result<Connector, String>,
        wake: impl Fn() + Send + Sync + 'static,
    ) {
        if !self.can_upload() {
            return;
        }
        self.generation += 1;
        self.stage = Stage::Uploading { sent: 0, total: 0 };
        let connect = match connect {
            Ok(c) => c,
            Err(e) => return self.unavailable(e),
        };
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        let generation = self.generation;
        let path = self.file.path.clone();
        let remote = self.remote_name();
        let action = if self.queue
            && self
                .target
                .as_ref()
                .is_some_and(|t| t.kind == Kind::PrusaConnect)
        {
            UploadAction::Queue
        } else if self.start_after {
            UploadAction::Print
        } else {
            UploadAction::UploadOnly
        };
        let mapped = self
            .u1
            .as_ref()
            .filter(|_| action == UploadAction::Print)
            .and_then(Plan::options);
        self.note = None;
        let hold = self.file.hold.clone();
        // A Moonraker printer is asked afterwards whether it recognised the file's header.
        let identity = self
            .target
            .as_ref()
            .filter(|t| matches!(t.kind, Kind::Moonraker | Kind::SnapmakerU1))
            .and_then(|t| Some((t.id, self.identity_reports.clone()?)))
            .and_then(|(id, tx)| Some((id, tx, file_identity(&path)?)));
        thread::Builder::new()
            .name("mica-print-upload".into())
            .spawn(move || {
                let event = match connect() {
                    Err(e) => Event::Unavailable(e),
                    Ok(h) => {
                        let mut last = std::time::Instant::now();
                        let mut first = true;
                        let mut report = |p: mica_printhost::UploadProgress| {
                            // At most ~20 progress events a second, plus the first and last.
                            if first || p.sent == p.total || last.elapsed().as_millis() >= 50 {
                                first = false;
                                last = std::time::Instant::now();
                                let _ = tx.send((generation, Event::Progress(p.sent, p.total)));
                                wake();
                            }
                        };
                        match (&mapped, h.as_u1()) {
                            (Some(o), Some(u1)) => {
                                match u1.start_with_mapping(&path, &remote, o, &mut report) {
                                    Ok(s) => Event::StartedU1(s),
                                    Err(e) => Event::UploadFailed(e),
                                }
                            }
                            (Some(_), None) => Event::UploadFailed(Error {
                                kind: mica_printhost::ErrorKind::Protocol,
                                message: "this connection cannot start a U1 slot map".into(),
                                http_status: None,
                            }),
                            (None, _) => {
                                match h.upload_action_with_progress(
                                    &path,
                                    &remote,
                                    action,
                                    &mut report,
                                ) {
                                    Ok(job) => Event::Uploaded(job),
                                    Err(e) => Event::UploadFailed(e),
                                }
                            }
                        }
                    }
                };
                let uploaded = matches!(event, Event::Uploaded(_) | Event::StartedU1(_));
                let _ = tx.send((generation, event));
                wake();
                if uploaded
                    && let Some((printer, reports, identity)) = identity
                    && let Ok(h) = connect()
                    && let Some(slicer) = read_slicer(h.as_ref(), &remote)
                {
                    let _ = reports.send(SlicerReport {
                        printer,
                        file: remote.clone(),
                        identity,
                        slicer,
                    });
                    wake();
                }
                drop(hold);
            })
            .ok();
    }

    /// Applies the threads' events; true when the stage changed.
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        if let Some(rx) = &self.batch_rx {
            let events: Vec<(u64, usize, Event)> =
                std::iter::from_fn(|| rx.try_recv().ok()).collect();
            for (generation, index, event) in events {
                if generation == self.generation {
                    changed |= self.apply_item(index, event);
                }
            }
        }
        let Some(rx) = &self.rx else {
            return changed;
        };
        let events: Vec<(u64, Event)> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        for (generation, event) in events {
            if generation != self.generation {
                continue;
            }
            changed |= self.apply(event);
        }
        changed
    }

    /// One transition. Events that do not fit the current stage are ignored.
    pub fn apply(&mut self, event: Event) -> bool {
        let Some(target) = self.target.clone() else {
            return false;
        };
        let explained = |e: &Error| explain(e, target.kind, &target.name, &target.address);
        match (&self.stage, event) {
            (Stage::Checking, Event::Checked(Ok(c))) => {
                self.reported_model = c.info.model.clone();
                self.u1 = match (&c.u1, target.kind) {
                    (Some((config, firmware)), Kind::SnapmakerU1) => Some(Plan::new(
                        config,
                        firmware.clone(),
                        &self.file.tools,
                        self.u1_remembered,
                    )),
                    _ => None,
                };
                // With a slot map, tool i is no longer slot i: those comparisons go.
                let issues: Vec<Issue> = c
                    .issues
                    .iter()
                    .filter(|i| {
                        self.u1.is_none()
                            || !matches!(
                                i.kind,
                                IssueKind::Material
                                    | IssueKind::Colour
                                    | IssueKind::UnknownFilament
                                    | IssueKind::MissingTool
                            )
                    })
                    .cloned()
                    .collect();
                let mut p = present(
                    &c.info,
                    &c.status,
                    if self.u1.is_some() { &[] } else { &c.filaments },
                    &self.file.tools,
                    self.file.format,
                    &issues,
                );
                if self.u1.is_some() {
                    p.rows.clear();
                }
                self.stage = Stage::Checked(Box::new(p));
            }
            (Stage::Checking, Event::Checked(Err(e))) => {
                self.stage = Stage::CheckFailed(explained(&e));
            }
            (Stage::Checking, Event::Unavailable(e)) => {
                self.stage = Stage::CheckFailed(Explanation {
                    headline: format!("Mica cannot connect to {}", target.name),
                    advice: e,
                });
            }
            (Stage::Uploading { .. }, Event::Progress(sent, total)) => {
                self.stage = Stage::Uploading { sent, total };
            }
            (Stage::Uploading { .. }, Event::Uploaded(job)) => {
                self.stage = Stage::Done {
                    job: job.job.name,
                    started: job.job.started,
                    message: job.message,
                    warning: job.warning,
                };
            }
            (Stage::Uploading { .. }, Event::StartedU1(s)) => {
                self.note = Some(s.summary());
                self.stage = Stage::Done {
                    job: s.job.name,
                    started: s.job.started,
                    message: None,
                    warning: false,
                };
            }
            (Stage::Uploading { .. }, Event::UploadFailed(e)) => {
                self.stage = Stage::Failed(explained(&e));
            }
            (Stage::Uploading { .. }, Event::Unavailable(e)) => {
                self.stage = Stage::Failed(Explanation {
                    headline: format!("Mica cannot connect to {}", target.name),
                    advice: e,
                });
            }
            _ => return false,
        }
        true
    }
}

/// The read-only checks, on the worker thread.
fn check(h: &dyn mica_printhost::PrinterHost, job: &JobRequirements) -> Result<Checked, Error> {
    let status = h.status()?;
    let info = h.info()?;
    let filaments = h.loaded_filaments().unwrap_or_default();
    let issues = h.preflight(job);
    // A U1 that cannot report its slots keeps the plain upload (slot i = Ti).
    let u1 = h.as_u1().and_then(|u| {
        let config = u.task_config().ok().flatten()?;
        Some((config, u.firmware_version().ok().flatten()))
    });
    Ok(Checked {
        info,
        status,
        filaments,
        issues,
        u1,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::printhost::monitor::tests::{Fake, Handle};
    use mica_printhost::{ErrorKind, Job, PrinterHost, PrinterState};
    use std::time::{Duration, Instant};

    fn file() -> JobFile {
        let dir = std::env::temp_dir().join(format!("mica-job-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("print.bgcode");
        std::fs::write(&path, vec![0u8; 4096]).unwrap();
        JobFile {
            path,
            format: FileFormat::Bgcode,
            tools: vec![
                JobTool {
                    tool: 0,
                    preset: "Prusament PLA".into(),
                    material: Some("PLA".into()),
                    colour: Some([255, 125, 56]),
                },
                JobTool {
                    tool: 1,
                    preset: "Prusament PETG".into(),
                    material: Some("PETG".into()),
                    colour: Some([65, 180, 240]),
                },
            ],
            bed_type: None,
            hold: None,
        }
    }
    fn target() -> Target {
        Target {
            id: 1,
            name: "XL".into(),
            kind: Kind::PrusaLink,
            address: "xl.local".into(),
        }
    }
    fn settle(d: &mut Dialog) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while d.busy() && Instant::now() < deadline {
            d.poll();
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn stages_follow_pick_check_upload_done() {
        let mut d = Dialog::new(file(), "3DBenchy".into());
        assert_eq!(d.stage, Stage::Pick);
        assert!(d.start_after);
        assert_eq!(d.remote_name(), "3DBenchy.bgcode");
        d.file_name = "benchy.gcode".into();
        assert_eq!(d.remote_name(), "benchy.bgcode");
        assert!(!d.can_upload());
        // Nothing happens without a printer.
        let fake = Arc::new(Fake::new(PrinterState::Idle, 5));
        let f = fake.clone();
        let connect: Connector =
            Arc::new(move || Ok(Box::new(Handle(f.clone())) as Box<dyn PrinterHost>));
        d.check(Ok(connect.clone()), || {});
        assert_eq!(d.stage, Stage::Pick);
        d.choose(Some(target()));
        d.check(Ok(connect.clone()), || {});
        assert_eq!(d.stage, Stage::Checking);
        assert!(d.busy());
        settle(&mut d);
        let p = d.preflight().expect("checked");
        assert!(p.ready);
        assert_eq!(p.tools_text, "5 tools (T1–T5)");
        assert_eq!(p.rows.len(), 2);
        assert!(d.can_upload());
        d.upload(Ok(connect.clone()), || {});
        assert!(matches!(d.stage, Stage::Uploading { .. }));
        settle(&mut d);
        assert_eq!(
            d.stage,
            Stage::Done {
                job: "benchy.bgcode".into(),
                started: true,
                message: None,
                warning: false,
            }
        );
        assert_eq!(*fake.state.lock().unwrap(), PrinterState::Printing);
    }

    #[test]
    fn a_u1_check_builds_the_slot_map_and_drops_slot_i_equals_ti() {
        let mut f = file();
        f.format = FileFormat::Gcode;
        let mut d = Dialog::new(f, "cube".into());
        d.choose(Some(Target {
            name: "U1".into(),
            kind: Kind::SnapmakerU1,
            ..target()
        }));
        d.u1_remembered = Some(Remembered {
            bed_leveling: true,
            flow_calibration: false,
            timelapse: true,
        });
        d.stage = Stage::Checking;
        let config = mica_printhost::parse_task_config(&serde_json::json!({
            "filament_exist": [true, true, true, false],
            "filament_type": ["PLA", "PETG", "PLA", ""],
            "filament_color_rgba": ["FFFFFFFF", "41B4F0FF", "FF7D38FF", ""],
            "auto_bed_leveling": false, "flow_calibrate": true, "time_lapse_camera": false
        }));
        let fake = Handle(Arc::new(Fake::new(PrinterState::Idle, 4)));
        let checked = Checked {
            info: fake.info().unwrap(),
            status: fake.status().unwrap(),
            filaments: vec![],
            issues: vec![Issue {
                kind: IssueKind::Material,
                tool: Some(1),
                message: "T1 requires PETG; loaded PLA".into(),
            }],
            u1: Some((config, Some("1.5.2.13".into()))),
        };
        assert!(d.apply(Event::Checked(Ok(Box::new(checked)))));
        let p = d.preflight().unwrap();
        assert!(p.rows.is_empty(), "no tool-i-is-slot-i table");
        assert!(p.warnings.is_empty(), "{:?}", p.warnings);
        let plan = d.u1.as_ref().unwrap();
        // Orange PLA → slot 3, blue PETG → slot 2; the remembered flags win.
        assert_eq!(
            plan.rows.iter().map(|r| r.slot).collect::<Vec<_>>(),
            [Some(2), Some(1)]
        );
        assert!(plan.bed_leveling && plan.timelapse && !plan.flow_calibration);
        assert!(d.can_upload());
        // Choosing the empty slot 4 blocks the start, not a plain upload.
        d.u1.as_mut().unwrap().choose(0, 3);
        assert!(!d.can_upload());
        d.upload(
            Ok(Arc::new(|| panic!("invalid slot map must not connect"))),
            || {},
        );
        assert!(matches!(d.stage, Stage::Checked(_)));
        d.start_after = false;
        assert!(d.can_upload());
        let f = fake.0.clone();
        let connect: Connector = Arc::new(move || Ok(Box::new(Handle(f.clone()))));
        d.upload(Ok(connect.clone()), || {});
        settle(&mut d);
        assert!(matches!(d.stage, Stage::Done { started: false, .. }));
        assert_eq!(*fake.state.lock().unwrap(), PrinterState::Idle);
        // A mapped start must use the U1 API, never silently take the plain upload path.
        // This fake has no U1 API, so it must fail before starting anything.
        d.stage = Stage::Failed(Explanation {
            headline: String::new(),
            advice: String::new(),
        });
        d.start_after = true;
        d.u1.as_mut().unwrap().choose(0, 0);
        d.upload(Ok(connect), || {});
        settle(&mut d);
        match &d.stage {
            Stage::Failed(x) => assert!(x.advice.contains("cannot start a U1 slot map")),
            s => panic!("{s:?}"),
        }
        assert_eq!(*fake.state.lock().unwrap(), PrinterState::Idle);
        // Done with the route in the note.
        d.stage = Stage::Uploading { sent: 0, total: 0 };
        assert!(d.apply(Event::StartedU1(mica_printhost::U1Started {
            job: Job {
                name: "cube.gcode".into(),
                id: None,
                started: true
            },
            firmware: Some("1.5.2.13".into()),
            route: mica_printhost::U1Route::StartLocalPrint,
        })));
        assert_eq!(
            d.stage,
            Stage::Done {
                job: "cube.gcode".into(),
                started: true,
                message: None,
                warning: false,
            }
        );
        assert_eq!(
            d.note.as_deref(),
            Some("Started with start_local_print (firmware 1.5.2.13)")
        );
    }

    #[test]
    fn connect_queue_is_not_a_print_and_switching_printers_clears_it() {
        let mut d = Dialog::new(file(), "queued".into());
        d.choose(Some(Target {
            kind: Kind::PrusaConnect,
            ..target()
        }));
        let fake = Arc::new(Fake::new(PrinterState::Idle, 5));
        let f = fake.clone();
        let connect: Connector = Arc::new(move || Ok(Box::new(Handle(f.clone()))));
        d.check(Ok(connect.clone()), || {});
        d.queue = true;
        // Even with start_after set, queue wins. The LAN fake refuses queues; if the
        // dialog accidentally sent Print or UploadOnly instead, this would succeed.
        d.upload(Ok(connect.clone()), || {});
        settle(&mut d);
        match &d.stage {
            Stage::Failed(x) => assert!(x.headline.contains("does not support a print queue")),
            s => panic!("{s:?}"),
        }
        assert_eq!(*fake.state.lock().unwrap(), PrinterState::Idle);
        d.choose(Some(target()));
        assert!(!d.queue);
        d.check(Ok(connect.clone()), || {});
        settle(&mut d);
        d.upload(Ok(connect), || {});
        settle(&mut d);
        assert!(matches!(d.stage, Stage::Done { started: true, .. }));
    }

    #[test]
    fn failures_and_stale_events_are_handled() {
        let mut d = Dialog::new(file(), "x".into());
        d.choose(Some(target()));
        // A connection that cannot be made.
        d.check(
            Ok(Arc::new(|| Err("The API key for XL is missing".into()))),
            || {},
        );
        settle(&mut d);
        match &d.stage {
            Stage::CheckFailed(x) => assert!(x.advice.contains("API key")),
            s => panic!("{s:?}"),
        }
        assert!(!d.can_upload());
        // A 401 during the check becomes the plain-language explanation.
        d.stage = Stage::Checking;
        assert!(d.apply(Event::Checked(Err(Error {
            kind: ErrorKind::Unauthorised,
            message: "printer returned HTTP 401".into(),
            http_status: Some(401),
        }))));
        match &d.stage {
            Stage::CheckFailed(x) => {
                assert_eq!(x.headline, "401: XL refused the credentials");
                assert!(x.advice.contains("API key"));
            }
            s => panic!("{s:?}"),
        }
        // Progress only counts while uploading; a busy printer fails the upload readably.
        assert!(!d.apply(Event::Progress(1, 2)));
        d.stage = Stage::Uploading { sent: 0, total: 0 };
        assert!(d.apply(Event::Progress(2048, 4096)));
        assert_eq!(
            d.stage,
            Stage::Uploading {
                sent: 2048,
                total: 4096
            }
        );
        assert!(d.apply(Event::UploadFailed(Error {
            kind: ErrorKind::Busy,
            message: "printer returned HTTP 409".into(),
            http_status: Some(409),
        })));
        match &d.stage {
            Stage::Failed(x) => assert!(x.headline.contains("busy")),
            s => panic!("{s:?}"),
        }
        // Failed uploads can be retried; switching printers resets everything.
        assert!(d.can_upload());
        d.choose(Some(Target {
            name: "CORE One".into(),
            ..target()
        }));
        assert_eq!(d.stage, Stage::Pick);
        // A late answer for the previous printer is ignored.
        let fake = Arc::new(Fake::new(PrinterState::Idle, 1));
        let f = fake.clone();
        d.check(
            Ok(Arc::new(move || {
                Ok(Box::new(Handle(f.clone())) as Box<dyn PrinterHost>)
            })),
            || {},
        );
        d.choose(Some(Target {
            name: "U1".into(),
            kind: Kind::SnapmakerU1,
            ..target()
        }));
        thread::sleep(Duration::from_millis(100));
        assert!(!d.poll());
        assert_eq!(d.stage, Stage::Pick);
    }
    #[test]
    fn connect_check_is_local_and_upload_warnings_survive() {
        let mut d = Dialog::new(file(), "cloud".into());
        d.choose(Some(Target {
            kind: Kind::PrusaConnect,
            ..target()
        }));
        d.check(
            Ok(Arc::new(|| panic!("cloud preflight must not connect"))),
            || {},
        );
        assert_eq!(d.stage, Stage::CloudReady);
        assert!(d.can_upload());
        assert!(d.preflight().is_none());
        d.stage = Stage::Uploading { sent: 0, total: 0 };
        assert!(d.apply(Event::Uploaded(UploadOutcome {
            job: mica_printhost::Job {
                name: "cloud.bgcode".into(),
                id: None,
                started: false
            },
            message: Some("Printer is busy; file accepted".into()),
            warning: true,
        })));
        assert_eq!(
            d.stage,
            Stage::Done {
                job: "cloud.bgcode".into(),
                started: false,
                message: Some("Printer is busy; file accepted".into()),
                warning: true
            }
        );
    }
}
