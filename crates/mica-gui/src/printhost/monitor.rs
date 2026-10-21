// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Live status of one printer, polled on its own thread. The UI sets the wanted interval
//! (`None` pauses polling entirely), queues pause / resume / cancel, and drains the updates
//! each frame into `Live`. The thread owns the connection; nothing here blocks the UI.
//! Cameras are discovered here (with the slowly changing info); playing them is the camera
//! players' job (`printhost::cameras`).

use super::poll;
use mica_printhost::{
    CameraInfo, Capabilities, Error, ErrorKind, LoadedFilament, PrinterHost, PrinterInfo,
    PrinterState, PrinterStatus,
};
use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

/// How often the slowly changing bits are re-read while polling.
const INFO_EVERY: Duration = Duration::from_secs(60);
const FILAMENTS_EVERY: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Pause,
    Resume,
    Cancel,
}
impl Command {
    pub fn label(self) -> &'static str {
        match self {
            Command::Pause => "Pause",
            Command::Resume => "Resume",
            Command::Cancel => "Cancel",
        }
    }
}

#[derive(Debug)]
pub enum Update {
    /// The connection could not even be made (missing secret, bad address).
    Unavailable(String),
    Info(PrinterInfo, Capabilities),
    Status(PrinterStatus),
    Filaments(Vec<LoadedFilament>),
    /// The cameras the printer reports (empty: none), re-read with the info.
    Cameras(Vec<CameraInfo>),
    /// The preview picture of a job (its name as the status reports it), read once per job;
    /// `None`: the printer has none for it.
    Thumbnail(String, Option<Arc<[u8]>>),
    Failed(Error),
    Command(Command, Result<(), Error>),
}

/// Largest job thumbnail read from a printer.
const THUMBNAIL_LIMIT: usize = 4 << 20;

/// What the UI knows about the printer right now.
#[derive(Debug, Default)]
pub struct Live {
    pub info: Option<PrinterInfo>,
    pub capabilities: Capabilities,
    pub status: Option<PrinterStatus>,
    pub filaments: Vec<LoadedFilament>,
    pub filaments_known: bool,
    /// `None` until asked; `Some(vec![])` means the printer reports no camera.
    pub cameras: Option<Vec<CameraInfo>>,
    pub error: Option<Error>,
    pub unavailable: Option<String>,
    pub updated: Option<Instant>,
    /// A command sent and not yet answered.
    pub pending: Option<Command>,
    pub last_command: Option<(Command, Result<(), Error>)>,
    /// Number of status polls received (tests, "live" indicator).
    pub polls: u64,
    /// Job transitions seen since the UI last took them (notifications).
    pub notices: Vec<Notice>,
    /// The latest job's preview picture: the job name and the encoded image, if any.
    pub thumbnail: Option<(String, Option<Arc<[u8]>>)>,
}
impl Live {
    /// The current job's preview picture, when the printer served one for it.
    pub fn job_thumbnail(&self) -> Option<(&str, &Arc<[u8]>)> {
        let job = self.status.as_ref()?.job_name.as_deref()?;
        match &self.thumbnail {
            Some((name, Some(bytes))) if name == job => Some((name, bytes)),
            _ => None,
        }
    }
}

/// A job transition worth telling the user about, seen between two polls.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Notice {
    /// A job that was printing (or paused) is done.
    Finished(Option<String>),
    /// The printer went into an error state (from any other state).
    Failed(Option<String>),
}
impl Notice {
    /// What changed between two states (`None`: nothing worth a notification). The first
    /// reading of a printer is never a transition: it was already finished when Mica looked.
    pub fn between(
        before: Option<mica_printhost::PrinterState>,
        after: &PrinterStatus,
        job: Option<String>,
    ) -> Option<Self> {
        use mica_printhost::PrinterState as S;
        let before = before?;
        match (before, after.state) {
            (S::Printing | S::Paused, S::Finished) => Some(Self::Finished(job)),
            (S::Error, S::Error) => None,
            (_, S::Error) => Some(Self::Failed(job)),
            _ => None,
        }
    }
}
impl Live {
    pub fn failed(&self) -> bool {
        self.error.is_some() || self.unavailable.is_some()
    }
    fn apply(&mut self, update: Update) {
        match update {
            Update::Unavailable(e) => {
                self.unavailable = Some(e);
                self.updated = Some(Instant::now());
            }
            Update::Info(info, capabilities) => {
                self.info = Some(info);
                self.capabilities = capabilities;
                self.unavailable = None;
            }
            Update::Status(s) => {
                let before = self.status.as_ref().map(|p| p.state);
                // The new reading may have lost the name (Moonraker clears it on error).
                let job = s
                    .job_name
                    .clone()
                    .or_else(|| self.status.as_ref().and_then(|p| p.job_name.clone()));
                if let Some(n) = Notice::between(before, &s, job) {
                    self.notices.push(n);
                }
                self.status = Some(s);
                self.error = None;
                self.unavailable = None;
                self.updated = Some(Instant::now());
                self.polls += 1;
            }
            Update::Filaments(f) => {
                self.filaments = f;
                self.filaments_known = true;
            }
            Update::Cameras(c) => self.cameras = Some(c),
            Update::Thumbnail(job, bytes) => self.thumbnail = Some((job, bytes)),
            Update::Failed(e) => {
                self.error = Some(e);
                self.updated = Some(Instant::now());
            }
            Update::Command(c, r) => {
                if self.pending == Some(c) {
                    self.pending = None;
                }
                self.last_command = Some((c, r));
            }
        }
    }
}

struct Shared {
    /// Wanted interval in milliseconds; 0 pauses polling.
    interval_ms: AtomicU64,
    stop: AtomicBool,
    /// Poll now, whatever the schedule says.
    poke: AtomicBool,
    commands: Mutex<VecDeque<Command>>,
}

pub type Connector = Arc<dyn Fn() -> Result<Box<dyn PrinterHost>, String> + Send + Sync>;

pub struct Watcher {
    shared: Arc<Shared>,
    rx: mpsc::Receiver<Update>,
    pub live: Live,
    handle: Option<thread::JoinHandle<()>>,
}
impl Watcher {
    /// Starts the thread; it sits idle until `set_interval` gives it a rate. `wake` is called
    /// after every update so the UI repaints (`ctx.request_repaint`).
    pub fn start(connect: Connector, wake: impl Fn() + Send + 'static) -> Self {
        let shared = Arc::new(Shared {
            interval_ms: AtomicU64::new(0),
            stop: AtomicBool::new(false),
            poke: AtomicBool::new(false),
            commands: Mutex::new(VecDeque::new()),
        });
        let (tx, rx) = mpsc::channel();
        let worker = shared.clone();
        let handle = thread::Builder::new()
            .name("mica-printer-monitor".into())
            .spawn(move || run(&worker, &tx, &connect, &wake))
            .ok();
        Self {
            shared,
            rx,
            live: Live::default(),
            handle,
        }
    }
    pub fn set_interval(&self, interval: Option<Duration>) {
        let ms = interval.map_or(0, |d| d.as_millis().max(1) as u64);
        self.shared.interval_ms.store(ms, Ordering::Relaxed);
    }
    /// The polling interval set last (`None`: not polling).
    #[cfg(test)]
    pub fn interval(&self) -> Option<Duration> {
        match self.shared.interval_ms.load(Ordering::Relaxed) {
            0 => None,
            ms => Some(Duration::from_millis(ms)),
        }
    }
    /// Poll at the next opportunity (after a command, or when the view opens).
    pub fn poke(&self) {
        self.shared.poke.store(true, Ordering::Relaxed);
    }
    pub fn command(&mut self, c: Command) {
        self.live.pending = Some(c);
        self.live.last_command = None;
        self.shared.commands.lock().unwrap().push_back(c);
        self.poke();
    }
    /// Applies pending updates; true when anything changed.
    pub fn drain(&mut self) -> bool {
        let mut any = false;
        while let Ok(u) = self.rx.try_recv() {
            self.live.apply(u);
            any = true;
        }
        any
    }
}
impl Drop for Watcher {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        self.shared.interval_ms.store(0, Ordering::Relaxed);
        // The thread notices within one sleep step; never block the UI on it.
        drop(self.handle.take());
    }
}

fn run(shared: &Shared, tx: &mpsc::Sender<Update>, connect: &Connector, wake: &dyn Fn()) {
    let step = Duration::from_millis(100);
    let mut host: Option<Box<dyn PrinterHost>> = None;
    let mut info_at: Option<Instant> = None;
    let mut filaments_at: Option<Instant> = None;
    let mut cameras_at: Option<Instant> = None;
    let mut thumbnail_for: Option<String> = None;
    // The schedule: due at once when polling (re)starts, then every interval; a failure
    // backs off, a poke or a command polls at the next step.
    let mut ticker = poll::Ticker::default();
    let mut failures = 0u32;
    let send = |u: Update| {
        let ok = tx.send(u).is_ok();
        wake();
        ok
    };
    loop {
        if shared.stop.load(Ordering::Relaxed) {
            return;
        }
        // Commands run even while polling is paused (the view may have just closed).
        let command = shared.commands.lock().unwrap().pop_front();
        if let Some(c) = command {
            if host.is_none() {
                host = connect().ok();
            }
            let result = match &host {
                Some(h) => match c {
                    Command::Pause => h.pause(),
                    Command::Resume => h.resume(),
                    Command::Cancel => h.cancel(),
                },
                None => Err(Error {
                    kind: ErrorKind::Unreachable,
                    message: "not connected".into(),
                    http_status: None,
                }),
            };
            if !send(Update::Command(c, result)) {
                return;
            }
            ticker.next = None;
            continue;
        }
        let ms = shared.interval_ms.load(Ordering::Relaxed);
        let wanted = (ms != 0).then(|| Duration::from_millis(ms));
        if shared.poke.swap(false, Ordering::Relaxed) {
            ticker.next = None;
        }
        if !ticker.due(Instant::now(), wanted) {
            thread::sleep(step);
            continue;
        }
        if host.is_none() {
            match connect() {
                Ok(h) => host = Some(h),
                Err(e) => {
                    if !send(Update::Unavailable(e)) {
                        return;
                    }
                    failures += 1;
                    ticker.next = Some(Instant::now() + poll::backoff(failures));
                    continue;
                }
            }
        }
        let h = host.as_ref().expect("connected");
        if info_at.is_none_or(|t| t.elapsed() >= INFO_EVERY) {
            match h.info() {
                Ok(i) => {
                    info_at = Some(Instant::now());
                    if !send(Update::Info(i, h.capabilities())) {
                        return;
                    }
                }
                Err(e) => {
                    if !send(Update::Failed(e)) {
                        return;
                    }
                    failures += 1;
                    ticker.next = Some(Instant::now() + poll::backoff(failures));
                    continue;
                }
            }
        }
        match h.status() {
            Ok(s) => {
                failures = 0;
                // A new job's preview picture is read once (a failure is "none", not retried).
                let new_job = s
                    .job_name
                    .clone()
                    .filter(|j| thumbnail_for.as_ref() != Some(j))
                    .filter(|_| {
                        matches!(
                            s.state,
                            PrinterState::Printing | PrinterState::Paused | PrinterState::Finished
                        )
                    });
                if !send(Update::Status(s)) {
                    return;
                }
                if let Some(job) = new_job {
                    let bytes = h
                        .job_thumbnail(&job, THUMBNAIL_LIMIT)
                        .ok()
                        .flatten()
                        .map(Arc::from);
                    thumbnail_for = Some(job.clone());
                    if !send(Update::Thumbnail(job, bytes)) {
                        return;
                    }
                }
            }
            Err(e) => {
                if !send(Update::Failed(e)) {
                    return;
                }
                failures += 1;
                ticker.next = Some(Instant::now() + poll::backoff(failures));
                continue;
            }
        }
        if filaments_at.is_none_or(|t| t.elapsed() >= FILAMENTS_EVERY) {
            filaments_at = Some(Instant::now());
            if let Ok(f) = h.loaded_filaments()
                && !send(Update::Filaments(f))
            {
                return;
            }
        }
        if cameras_at.is_none_or(|t| t.elapsed() >= INFO_EVERY) {
            cameras_at = Some(Instant::now());
            // A camera list that cannot be read is "no camera", never a printer failure.
            let cameras = h.cameras().unwrap_or_default();
            if !send(Update::Cameras(cameras)) {
                return;
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use mica_printhost::{
        FileFormat, Job, PrinterState, ProgressCallback, Result, Temperature, ToolTemperature,
    };
    use std::path::Path;

    /// A printer that lives in memory: counts calls, reports a fixed state, accepts uploads.
    pub struct Fake {
        pub state: Mutex<PrinterState>,
        pub tools: Vec<u32>,
        pub filaments: Vec<LoadedFilament>,
        pub polls: AtomicU64,
        pub commands: Mutex<Vec<&'static str>>,
        pub fail_status: AtomicBool,
        pub upload_error: Option<Error>,
    }
    impl Fake {
        pub fn new(state: PrinterState, tools: u32) -> Self {
            Self {
                state: Mutex::new(state),
                tools: (0..tools).collect(),
                filaments: vec![],
                polls: AtomicU64::new(0),
                commands: Mutex::new(vec![]),
                fail_status: AtomicBool::new(false),
                upload_error: None,
            }
        }
    }
    /// The shared handle a test connects through (the orphan rule forbids `Arc<Fake>`).
    pub struct Handle(pub Arc<Fake>);
    impl std::ops::Deref for Handle {
        type Target = Fake;
        fn deref(&self) -> &Fake {
            &self.0
        }
    }
    impl PrinterHost for Handle {
        fn capabilities(&self) -> Capabilities {
            Capabilities {
                upload: true,
                start: true,
                pause_resume: true,
                cancel: true,
                status: true,
                filaments: !self.filaments.is_empty(),
                ..Capabilities::default()
            }
        }
        fn info(&self) -> Result<PrinterInfo> {
            Ok(PrinterInfo {
                model: Some("Fake XL".into()),
                firmware: Some("1.0".into()),
                state: *self.state.lock().unwrap(),
                tools: Some(self.tools.clone()),
                formats: vec![FileFormat::Bgcode, FileFormat::Gcode],
                bed_type: None,
            })
        }
        fn upload_with_progress(
            &self,
            path: &Path,
            remote_name: &str,
            start_after: bool,
            progress: &mut ProgressCallback<'_>,
        ) -> Result<Job> {
            if let Some(e) = &self.upload_error {
                return Err(e.clone());
            }
            let total = std::fs::metadata(path).map(|m| m.len()).unwrap_or(10);
            for sent in [0, total / 2, total] {
                progress(mica_printhost::UploadProgress { sent, total });
            }
            if start_after {
                *self.state.lock().unwrap() = PrinterState::Printing;
            }
            Ok(Job {
                name: remote_name.into(),
                id: None,
                started: start_after,
            })
        }
        fn start(&self, _: &str) -> Result<()> {
            Ok(())
        }
        fn pause(&self) -> Result<()> {
            self.commands.lock().unwrap().push("pause");
            *self.state.lock().unwrap() = PrinterState::Paused;
            Ok(())
        }
        fn resume(&self) -> Result<()> {
            self.commands.lock().unwrap().push("resume");
            *self.state.lock().unwrap() = PrinterState::Printing;
            Ok(())
        }
        fn cancel(&self) -> Result<()> {
            self.commands.lock().unwrap().push("cancel");
            *self.state.lock().unwrap() = PrinterState::Idle;
            Ok(())
        }
        fn status(&self) -> Result<PrinterStatus> {
            self.polls.fetch_add(1, Ordering::Relaxed);
            if self.fail_status.load(Ordering::Relaxed) {
                return Err(Error {
                    kind: ErrorKind::Unreachable,
                    message: "printer unreachable or request timed out".into(),
                    http_status: None,
                });
            }
            let state = *self.state.lock().unwrap();
            Ok(PrinterStatus {
                state,
                job_name: Some("cube.bgcode".into()),
                progress: Some(42.),
                elapsed: Some(600.),
                remaining: Some(900.),
                current_layer: Some(50),
                z: Some(10.),
                hotends: self
                    .tools
                    .iter()
                    .map(|&t| ToolTemperature {
                        tool: t,
                        temperature: Temperature {
                            current: Some(210.),
                            target: Some(215.),
                        },
                    })
                    .collect(),
                bed: Some(Temperature {
                    current: Some(60.),
                    target: Some(60.),
                }),
                chamber: None,
            })
        }
        fn loaded_filaments(&self) -> Result<Vec<LoadedFilament>> {
            Ok(self.filaments.clone())
        }
    }

    fn wait(watcher: &mut Watcher, until: impl Fn(&Live) -> bool, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            watcher.drain();
            if until(&watcher.live) {
                return true;
            }
            thread::sleep(Duration::from_millis(10));
        }
        false
    }

    #[test]
    fn watcher_polls_only_while_an_interval_is_set_and_runs_commands() {
        let fake = Arc::new(Fake::new(PrinterState::Printing, 5));
        let f = fake.clone();
        let mut w = Watcher::start(
            Arc::new(move || Ok(Box::new(Handle(f.clone())) as Box<dyn PrinterHost>)),
            || {},
        );
        // Paused: nothing happens.
        thread::sleep(Duration::from_millis(250));
        w.drain();
        assert_eq!(fake.polls.load(Ordering::Relaxed), 0);
        assert!(w.live.status.is_none());
        // Shown: the first poll is immediate, then at the interval.
        w.set_interval(Some(Duration::from_millis(200)));
        assert!(wait(&mut w, |l| l.polls >= 1, Duration::from_secs(5)));
        assert_eq!(
            w.live.info.as_ref().unwrap().tools.as_ref().unwrap().len(),
            5
        );
        assert_eq!(
            w.live.status.as_ref().unwrap().state,
            PrinterState::Printing
        );
        assert_eq!(w.live.cameras, Some(vec![]));
        assert!(wait(&mut w, |l| l.polls >= 3, Duration::from_secs(5)));
        // Hidden again: the count stops growing.
        w.set_interval(None);
        thread::sleep(Duration::from_millis(300));
        w.drain();
        let frozen = fake.polls.load(Ordering::Relaxed);
        thread::sleep(Duration::from_millis(450));
        assert_eq!(fake.polls.load(Ordering::Relaxed), frozen);
        // A command still runs while hidden, and answers.
        w.command(Command::Pause);
        assert_eq!(w.live.pending, Some(Command::Pause));
        assert!(wait(
            &mut w,
            |l| l
                .last_command
                .as_ref()
                .is_some_and(|(c, r)| *c == Command::Pause && r.is_ok()),
            Duration::from_secs(5)
        ));
        assert_eq!(w.live.pending, None);
        assert_eq!(*fake.commands.lock().unwrap(), ["pause"]);
        // Failures are reported, then cleared by the next good poll.
        fake.fail_status.store(true, Ordering::Relaxed);
        w.set_interval(Some(Duration::from_millis(100)));
        assert!(wait(&mut w, |l| l.error.is_some(), Duration::from_secs(5)));
        assert!(w.live.failed());
        fake.fail_status.store(false, Ordering::Relaxed);
        w.poke();
        assert!(wait(
            &mut w,
            |l| l.error.is_none() && l.polls >= 4,
            Duration::from_secs(20)
        ));
    }

    #[test]
    fn job_transitions_become_notices_once() {
        use mica_printhost::PrinterState as S;
        let status = |state| PrinterStatus {
            state,
            job_name: Some("cube.bgcode".into()),
            progress: None,
            elapsed: None,
            remaining: None,
            current_layer: None,
            z: None,
            hotends: vec![],
            bed: None,
            chamber: None,
        };
        let job = || Some("cube.bgcode".to_string());
        assert_eq!(
            Notice::between(Some(S::Printing), &status(S::Finished), job()),
            Some(Notice::Finished(job()))
        );
        assert_eq!(
            Notice::between(Some(S::Paused), &status(S::Finished), job()),
            Some(Notice::Finished(job()))
        );
        assert_eq!(
            Notice::between(Some(S::Printing), &status(S::Error), job()),
            Some(Notice::Failed(job()))
        );
        // Already finished or failed when first seen, or still so: nothing new.
        assert_eq!(Notice::between(None, &status(S::Finished), job()), None);
        assert_eq!(Notice::between(None, &status(S::Error), job()), None);
        assert_eq!(
            Notice::between(Some(S::Finished), &status(S::Finished), job()),
            None
        );
        assert_eq!(
            Notice::between(Some(S::Error), &status(S::Error), job()),
            None
        );
        assert_eq!(
            Notice::between(Some(S::Printing), &status(S::Idle), job()),
            None
        );
        // Through `Live`: the watcher collects them for the UI.
        let mut live = Live::default();
        live.apply(Update::Status(status(S::Printing)));
        live.apply(Update::Status(status(S::Printing)));
        live.apply(Update::Status(status(S::Finished)));
        live.apply(Update::Status(status(S::Finished)));
        assert_eq!(live.notices, vec![Notice::Finished(job())]);
    }

    #[test]
    fn a_connection_that_cannot_be_made_is_reported_once_per_backoff() {
        let mut w = Watcher::start(
            Arc::new(|| Err("The API key for XL is missing from the Keychain".to_string())),
            || {},
        );
        w.set_interval(Some(Duration::from_millis(50)));
        assert!(wait(
            &mut w,
            |l| l.unavailable.is_some(),
            Duration::from_secs(5)
        ));
        assert!(w.live.failed());
        assert!(w.live.unavailable.as_deref().unwrap().contains("Keychain"));
    }
}
