// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The printers the dashboard shows: `printers.toml` as the window keeps it (re-read when it
//! changes), one polling thread per printer that runs only while a page is looking, and
//! the uploads sent to them. The page gets JSON; secrets never leave this process.

use crate::cameras::Hub;
use mica_camera::Auth;
use mica_printers::{
    Connector,
    config::{self, Printer},
    poll,
    secrets::SecretStore,
};
use mica_printhost::{
    CameraInfo, Capabilities, ErrorKind, FileFormat, LoadedFilament, PrinterHost, PrinterInfo,
    PrinterState, PrinterStatus, Timeouts, UploadAction,
};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant, SystemTime},
};

/// A printer is polled while a page asked for it within this long.
const WATCH: Duration = Duration::from_secs(30);
/// How often the slowly changing bits are re-read while polling.
const INFO_EVERY: Duration = Duration::from_secs(60);
const FILAMENTS_EVERY: Duration = Duration::from_secs(30);
/// `printers.toml` is checked for changes at most this often.
const RELOAD_EVERY: Duration = Duration::from_secs(2);
/// A finished upload stays on its card this long.
const TRANSFER_SHOWN: Duration = Duration::from_secs(60);

pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// What is known about a printer right now.
#[derive(Default)]
pub struct Live {
    pub info: Option<PrinterInfo>,
    pub capabilities: Capabilities,
    pub status: Option<PrinterStatus>,
    pub filaments: Vec<LoadedFilament>,
    /// `None` until asked; `Some(vec![])`: the printer reports no camera.
    pub cameras: Option<Vec<CameraInfo>>,
    /// Why the last poll failed (plain words).
    pub error: Option<String>,
    pub updated: Option<SystemTime>,
    failures: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Sending,
    Done,
    Failed,
}

/// An upload from the dashboard to the printer.
#[derive(Clone, Debug)]
pub struct Transfer {
    pub name: String,
    pub phase: Phase,
    pub sent: u64,
    pub total: u64,
    pub start: bool,
    pub message: Option<String>,
    finished: Option<Instant>,
}

/// One printer of the list.
pub struct Entry {
    /// The printer's name made URL-safe ("Core One" → `core-one`), unique in the list.
    pub id: String,
    pub printer: Printer,
    pub connector: Result<Connector, String>,
    pub camera_auth: Result<Option<Auth>, String>,
    pub live: Mutex<Live>,
    pub transfer: Mutex<Option<Transfer>>,
    pub camera: Arc<Hub>,
    watched: Mutex<Option<Instant>>,
    poke: AtomicBool,
    stop: AtomicBool,
}

impl Entry {
    /// A page looks at this printer: keep (or start) polling it.
    pub fn watch(&self) {
        let mut w = lock(&self.watched);
        if w.is_none_or(|t| t.elapsed() >= WATCH) {
            self.poke.store(true, Ordering::SeqCst);
        }
        *w = Some(Instant::now());
    }
    fn watched(&self) -> bool {
        lock(&self.watched).is_some_and(|t| t.elapsed() < WATCH)
    }
    pub fn state(&self) -> Option<PrinterState> {
        lock(&self.live).status.as_ref().map(|s| s.state)
    }
}

/// The printer list.
pub struct Fleet {
    path: PathBuf,
    secrets: Arc<dyn SecretStore>,
    loopback_only: bool,
    state: Mutex<FleetState>,
}

#[derive(Default)]
struct FleetState {
    loaded: bool,
    stamp: Option<(u64, SystemTime)>,
    checked: Option<Instant>,
    entries: Vec<Arc<Entry>>,
    /// Why `printers.toml` could not be used.
    error: Option<String>,
}

fn stamp(path: &Path) -> Option<(u64, SystemTime)> {
    let m = std::fs::metadata(path).ok()?;
    Some((m.len(), m.modified().ok()?))
}

/// `Core One #2` → `core-one-2`.
fn slug(name: &str) -> String {
    let mut s = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            s.push(c.to_ascii_lowercase());
        } else if !s.ends_with('-') {
            s.push('-');
        }
    }
    let s = s.trim_matches('-').to_string();
    if s.is_empty() { "printer".into() } else { s }
}

/// What a failed request means, in plain words.
pub fn describe(e: &mica_printhost::Error) -> String {
    match e.kind {
        ErrorKind::Unreachable => {
            "Mica can't reach this printer. Check that it is on and connected to the network."
                .into()
        }
        ErrorKind::Unauthorised => {
            "The printer refused the API key or password Mica has for it.".into()
        }
        ErrorKind::Busy => "The printer is busy; try again in a moment.".into(),
        ErrorKind::Protocol => format!("The printer gave an unexpected answer ({}).", e.message),
    }
}

/// Why a printer's secret could not be read, in words that say what to do on this server.
fn secret_problem(printer: &Printer, secrets: &dyn SecretStore, e: String) -> String {
    if printer.keychain.is_some() && !secrets.available() {
        return format!(
            "Mica can't read this printer's API key or password: saving printer secrets is {}. \
             Add api_key_env (or password_env) to this printer's entry in \
             printers.toml and set that environment variable for mica-web.",
            secrets.label()
        );
    }
    format!("Mica can't read this printer's API key or password: {e}")
}

/// Whether `address` (a host name, IP or URL) is on this computer.
fn local(address: &str) -> bool {
    let a = address.trim();
    let url = if a.contains("://") {
        a.to_string()
    } else {
        format!("http://{a}")
    };
    mica_camera::is_loopback(&url)
}

impl Fleet {
    pub fn new(path: PathBuf, secrets: Arc<dyn SecretStore>, loopback_only: bool) -> Self {
        Self {
            path,
            secrets,
            loopback_only,
            state: Mutex::new(FleetState::default()),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The printers, re-read when `printers.toml` changed, and why it could not be read.
    pub fn entries(&self) -> (Vec<Arc<Entry>>, Option<String>) {
        let mut st = lock(&self.state);
        let due = st.checked.is_none_or(|t| t.elapsed() >= RELOAD_EVERY);
        if due {
            st.checked = Some(Instant::now());
            let now = stamp(&self.path);
            if !st.loaded || now != st.stamp {
                st.loaded = true;
                st.stamp = now;
                for e in &st.entries {
                    e.stop.store(true, Ordering::SeqCst);
                }
                let (printers, error) = config::load(&self.path);
                st.error = error;
                st.entries = self.make(printers);
            }
        }
        (st.entries.clone(), st.error.clone())
    }

    pub fn find(&self, id: &str) -> Option<Arc<Entry>> {
        self.entries().0.into_iter().find(|e| e.id == id)
    }

    fn make(&self, printers: Vec<Printer>) -> Vec<Arc<Entry>> {
        let mut ids: Vec<String> = vec![];
        let mut out = vec![];
        for printer in printers {
            let mut id = slug(&printer.name);
            let base = id.clone();
            let mut n = 2;
            while ids.contains(&id) {
                id = format!("{base}-{n}");
                n += 1;
            }
            ids.push(id.clone());
            // Secrets are read here, once, on this thread; the threads get connectors.
            let connector = if self.loopback_only && !local(&printer.address) {
                Err(format!(
                    "{}: this server only talks to printers on this computer",
                    printer.address
                ))
            } else {
                printer
                    .config(self.secrets.as_ref())
                    .map(|config| {
                        let c: Connector = Arc::new(move || {
                            mica_printhost::connect(config.clone(), Timeouts::default())
                                .map_err(|e| describe(&e))
                        });
                        c
                    })
                    .map_err(|e| secret_problem(&printer, self.secrets.as_ref(), e))
            };
            let camera_auth = printer.camera_auth(self.secrets.as_ref());
            let entry = Arc::new(Entry {
                id,
                printer,
                connector,
                camera_auth,
                live: Mutex::default(),
                transfer: Mutex::default(),
                camera: Arc::default(),
                watched: Mutex::default(),
                poke: AtomicBool::new(false),
                stop: AtomicBool::new(false),
            });
            let e = entry.clone();
            let _ = thread::Builder::new()
                .name("mica-web-poll".into())
                .spawn(move || run(&e));
            out.push(entry);
        }
        out
    }

    /// Stops every polling thread (the server is shutting down).
    pub fn stop(&self) {
        for e in &lock(&self.state).entries {
            e.stop.store(true, Ordering::SeqCst);
        }
    }

    /// The printer list as the page shows it; marks every printer as watched.
    pub fn json(&self) -> Value {
        let (entries, error) = self.entries();
        let printers: Vec<Value> = entries
            .iter()
            .map(|e| {
                e.watch();
                entry_json(e)
            })
            .collect();
        json!({ "printers": printers, "error": error })
    }
}

/// Polls `entry` while it is watched, at the window's rates (`mica_printers::poll`).
fn run(entry: &Entry) {
    let mut host: Option<Box<dyn PrinterHost>> = None;
    let mut next = Instant::now();
    let mut last_info: Option<Instant> = None;
    let mut last_filaments: Option<Instant> = None;
    while !entry.stop.load(Ordering::SeqCst) {
        let poke = entry.poke.swap(false, Ordering::SeqCst);
        if !entry.watched() || (!poke && Instant::now() < next) {
            thread::sleep(Duration::from_millis(100));
            continue;
        }
        let connector = match &entry.connector {
            Ok(c) => c,
            Err(e) => {
                let mut live = lock(&entry.live);
                live.error = Some(e.clone());
                // Nothing will be learned about its cameras either.
                live.cameras.get_or_insert_with(Vec::new);
                next = Instant::now() + poll::FAILED_MAX;
                continue;
            }
        };
        if host.is_none() {
            match connector() {
                Ok(h) => host = Some(h),
                Err(e) => {
                    let mut live = lock(&entry.live);
                    live.failures += 1;
                    live.error = Some(e);
                    live.status = None;
                    next = Instant::now() + poll::backoff(live.failures);
                    continue;
                }
            }
        }
        let h = host.as_ref().expect("connected above");
        if last_info.is_none_or(|t| t.elapsed() >= INFO_EVERY) {
            let info = h.info().ok();
            let cameras = h.cameras().unwrap_or_default();
            let mut live = lock(&entry.live);
            if let Some(info) = info {
                live.info = Some(info);
                live.capabilities = h.capabilities();
                last_info = Some(Instant::now());
            }
            live.cameras = Some(cameras);
        }
        let status = h.status();
        let ok = status.is_ok();
        {
            let mut live = lock(&entry.live);
            match status {
                Ok(s) => {
                    live.status = Some(s);
                    live.error = None;
                    live.failures = 0;
                    live.updated = Some(SystemTime::now());
                }
                Err(e) => {
                    live.failures += 1;
                    live.error = Some(describe(&e));
                    live.status = None;
                }
            }
        }
        if ok && last_filaments.is_none_or(|t| t.elapsed() >= FILAMENTS_EVERY) {
            if let Ok(f) = h.loaded_filaments() {
                lock(&entry.live).filaments = f;
            }
            last_filaments = Some(Instant::now());
        }
        let live = lock(&entry.live);
        next = Instant::now()
            + if live.failures > 0 {
                poll::backoff(live.failures)
            } else {
                poll::interval(live.status.as_ref().map(|s| s.state), false)
            };
        if !ok {
            // Reconnect next time (a restarted printer, a new address).
            host = None;
            last_info = None;
        }
    }
}

fn state_word(s: PrinterState) -> (&'static str, &'static str) {
    match s {
        PrinterState::Idle => ("idle", "Ready"),
        PrinterState::Printing => ("printing", "Printing"),
        PrinterState::Paused => ("paused", "Paused"),
        PrinterState::Finished => ("finished", "Finished"),
        PrinterState::Error => ("error", "Needs attention"),
        PrinterState::Offline => ("offline", "Offline"),
    }
}

fn temp(t: &mica_printhost::Temperature) -> Value {
    json!({ "current": t.current, "target": t.target })
}

/// `#RRGGBB`, `RRGGBB` or `RRGGBBAA` → RGB.
fn rgb(colour: &str) -> Option<[u8; 3]> {
    let h = colour.trim().trim_start_matches('#');
    if h.len() < 6 || !h.is_char_boundary(6) {
        return None;
    }
    let v = u32::from_str_radix(&h[..6], 16).ok()?;
    Some([(v >> 16) as u8, (v >> 8) as u8, v as u8])
}

/// A filament colour for the page: the swatch as CSS data and its name as the words.
pub fn colour_json(colour: Option<&str>) -> Value {
    match colour.and_then(rgb) {
        Some(c) => json!({
            "css": format!("rgb({}, {}, {})", c[0], c[1], c[2]),
            "name": mica_printhost::colour::colour_name(c),
        }),
        None => Value::Null,
    }
}

fn entry_json(e: &Entry) -> Value {
    let live = lock(&e.live);
    let p = &e.printer;
    let (state, state_label) = match (&live.status, &live.error) {
        (Some(s), _) => state_word(s.state),
        (None, Some(_)) => ("unreachable", "Not connected"),
        (None, None) => ("connecting", "Connecting…"),
    };
    let job = live.status.as_ref().and_then(|s| {
        let active = matches!(
            s.state,
            PrinterState::Printing | PrinterState::Paused | PrinterState::Finished
        );
        (active || s.job_name.is_some()).then(|| {
            json!({
                "name": s.job_name,
                "progress": s.progress,
                "elapsed": s.elapsed,
                "remaining": s.remaining,
                "layer": s.current_layer,
                "z": s.z,
            })
        })
    });
    let temps = live.status.as_ref().map(|s| {
        json!({
            "hotends": s.hotends.iter().map(|h| json!({
                "tool": h.tool + 1,
                "current": h.temperature.current,
                "target": h.temperature.target,
            })).collect::<Vec<_>>(),
            "bed": s.bed.as_ref().map(temp),
            "chamber": s.chamber.as_ref().map(temp),
        })
    });
    let filaments: Vec<Value> = live
        .filaments
        .iter()
        .map(|f| {
            json!({
                "tool": f.tool + 1,
                "material": f.material,
                "colour": colour_json(f.colour.as_deref()),
            })
        })
        .collect();
    let camera = match mica_printers::camera::choose(
        p,
        live.cameras.as_deref(),
        mica_printers::camera::Role::Focused,
    ) {
        Ok(c) => json!({
            "available": true,
            "label": c.label,
            "note": c.note,
            "error": e.camera_auth.as_ref().err().cloned().or_else(|| crate::cameras::status_error(&e.camera)),
        }),
        Err(mica_printers::camera::NoCamera::Unknown) => {
            json!({ "available": false, "unknown": true })
        }
        Err(mica_printers::camera::NoCamera::None) => json!({ "available": false }),
        Err(
            mica_printers::camera::NoCamera::Unsupported(why)
            | mica_printers::camera::NoCamera::BadUrl(why),
        ) => json!({ "available": false, "note": why }),
    };
    let transfer = {
        let mut t = lock(&e.transfer);
        if t.as_ref()
            .is_some_and(|t| t.finished.is_some_and(|f| f.elapsed() >= TRANSFER_SHOWN))
        {
            *t = None;
        }
        t.as_ref().map(|t| {
            json!({
                "name": t.name,
                "phase": match t.phase {
                    Phase::Sending => "sending",
                    Phase::Done => "done",
                    Phase::Failed => "failed",
                },
                "sent": t.sent,
                "total": t.total,
                "start": t.start,
                "message": t.message,
            })
        })
    };
    let formats: Vec<&str> = live
        .info
        .as_ref()
        .map(|i| {
            i.formats
                .iter()
                .map(|f| match f {
                    FileFormat::Gcode => "gcode",
                    FileFormat::Bgcode => "bgcode",
                })
                .collect()
        })
        .unwrap_or_default();
    json!({
        "id": e.id,
        "name": p.name,
        "kind": p.kind.label(),
        "model": live.info.as_ref().and_then(|i| i.model.clone()).or_else(|| p.model.clone()),
        "state": state,
        "state_label": state_label,
        "error": live.error,
        "job": job,
        "temperatures": temps,
        "filaments": filaments,
        "camera": camera,
        "transfer": transfer,
        "formats": formats,
        "updated": live.updated.and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok()).map(|d| d.as_secs()),
    })
}

/// A file name the printer may get: the last path component of what the browser sent, no
/// control characters, ending in `.gcode` or `.bgcode`.
pub fn remote_name(name: &str) -> Result<(String, FileFormat), String> {
    let base = name.rsplit(['/', '\\']).next().unwrap_or("").trim();
    let clean: String = base
        .chars()
        .filter(|c| !c.is_control() && !matches!(c, '"' | '<' | '>' | '|' | ':' | '*' | '?'))
        .collect();
    let clean = clean.trim_start_matches('.').trim().to_string();
    let format = FileFormat::from_path(Path::new(&clean))
        .ok_or("Only G-code files (.gcode or .bgcode) can be sent to a printer.")?;
    if clean.len() > 200 {
        return Err("The file name is too long for the printer.".into());
    }
    Ok((clean, format))
}

const BUSY_UPLOADING: &str = "Another upload to this printer is still running.";

/// Whether `name` may be sent to the printer of `entry` now (and started, with `start`):
/// the name the printer gets and the connection to send it with. Checked before a file is
/// received from the browser, so a refused upload is not transferred first.
pub fn check(entry: &Entry, name: &str, start: bool) -> Result<(String, Connector), String> {
    let (remote, format) = remote_name(name)?;
    {
        let live = lock(&entry.live);
        if let Some(info) = &live.info
            && !info.formats.is_empty()
            && !info.formats.contains(&format)
        {
            return Err(match format {
                FileFormat::Bgcode => "This printer does not take binary G-code (.bgcode).".into(),
                FileFormat::Gcode => "This printer takes binary G-code (.bgcode) only.".into(),
            });
        }
        if start
            && live
                .status
                .as_ref()
                .is_some_and(|s| matches!(s.state, PrinterState::Printing | PrinterState::Paused))
        {
            return Err(
                "The printer is busy with another print. Upload without starting, or wait until it finishes."
                    .into(),
            );
        }
    }
    if lock(&entry.transfer)
        .as_ref()
        .is_some_and(|t| t.phase == Phase::Sending)
    {
        return Err(BUSY_UPLOADING.into());
    }
    let connector = entry.connector.clone()?;
    Ok((remote, connector))
}

/// Sends the file at `path` to the printer of `entry` as `name`, on a thread; `start`
/// starts printing it. `remove` deletes `path` afterwards (an upload received from the
/// browser). Refused while another upload to this printer runs, and when starting is asked
/// while the printer is busy.
pub fn send(
    entry: &Arc<Entry>,
    path: PathBuf,
    name: &str,
    start: bool,
    remove: bool,
) -> Result<(), String> {
    let (remote, connector) = match check(entry, name, start) {
        Ok(r) => r,
        Err(why) => {
            if remove {
                let _ = std::fs::remove_file(&path);
            }
            return Err(why);
        }
    };
    let total = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    {
        let mut t = lock(&entry.transfer);
        if t.as_ref().is_some_and(|t| t.phase == Phase::Sending) {
            drop(t);
            if remove {
                let _ = std::fs::remove_file(&path);
            }
            return Err(BUSY_UPLOADING.into());
        }
        *t = Some(Transfer {
            name: remote.clone(),
            phase: Phase::Sending,
            sent: 0,
            total,
            start,
            message: None,
            finished: None,
        });
    }
    let e = entry.clone();
    let spawned = thread::Builder::new()
        .name("mica-web-upload".into())
        .spawn(move || {
            let result = connector().and_then(|h| {
                let action = if start {
                    UploadAction::Print
                } else {
                    UploadAction::UploadOnly
                };
                let mut progress = |p: mica_printhost::UploadProgress| {
                    if let Some(t) = lock(&e.transfer).as_mut() {
                        t.sent = p.sent;
                        t.total = p.total;
                    }
                };
                h.upload_action_with_progress(&path, &remote, action, &mut progress)
                    .map_err(|err| describe(&err))
            });
            if remove {
                let _ = std::fs::remove_file(&path);
            }
            if let Some(t) = lock(&e.transfer).as_mut() {
                t.finished = Some(Instant::now());
                match result {
                    Ok(outcome) => {
                        t.phase = Phase::Done;
                        t.sent = t.total;
                        t.message = Some(outcome.message.unwrap_or_else(|| {
                            if start {
                                "Uploaded and started.".into()
                            } else {
                                "Uploaded.".into()
                            }
                        }));
                    }
                    Err(why) => {
                        t.phase = Phase::Failed;
                        t.message = Some(why);
                    }
                }
            }
            e.poke.store(true, Ordering::SeqCst);
        });
    if spawned.is_err() {
        *lock(&entry.transfer) = None;
        return Err("Mica could not start the upload.".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_and_names_are_safe() {
        assert_eq!(slug("Core One #2"), "core-one-2");
        assert_eq!(slug("  ÜÜ "), "printer");
        assert_eq!(
            remote_name("C:\\fakepath\\benchy.gcode").unwrap().0,
            "benchy.gcode"
        );
        assert_eq!(
            remote_name("../../etc/x.BGCODE").unwrap().1,
            FileFormat::Bgcode
        );
        assert_eq!(remote_name(".hidden.gcode").unwrap().0, "hidden.gcode");
        assert!(remote_name("model.stl").is_err());
        assert!(remote_name("a\u{0}b.gcode").unwrap().0 == "ab.gcode");
        assert!(local("127.0.0.1:8080") && local("http://localhost") && !local("xl.local"));
    }

    #[test]
    fn unreadable_secrets_say_what_to_do() {
        struct NoStore;
        impl SecretStore for NoStore {
            fn label(&self) -> &'static str {
                "not supported yet on this platform"
            }
            fn available(&self) -> bool {
                false
            }
            fn get(&self, _: &str) -> Result<Option<String>, String> {
                Err("not supported".into())
            }
            fn set(&self, _: &str, _: &str) -> Result<(), String> {
                Err("not supported".into())
            }
            fn delete(&self, _: &str) -> Result<(), String> {
                Ok(())
            }
        }
        let mut p = Printer::new(config::Kind::PrusaLink);
        p.name = "XL".into();
        p.address = "127.0.0.1:1".into();
        p.keychain = Some("mica-printer-1".into());
        let fleet = Fleet::new(PathBuf::new(), Arc::new(NoStore), true);
        let e = &fleet.make(vec![p.clone()])[0];
        let why = e.connector.as_ref().err().unwrap();
        assert!(
            why.contains("api_key_env") && why.contains("not supported yet"),
            "{why}"
        );
        e.stop.store(true, Ordering::SeqCst);
        // A store that works but lacks the item: its own message, prefixed.
        let store = mica_printers::secrets::MemoryStore::default();
        let why = secret_problem(&p, &store, p.config(&store).unwrap_err());
        assert!(
            why.starts_with("Mica can't read") && why.contains("missing"),
            "{why}"
        );
    }

    #[test]
    fn colours_are_named_not_hex() {
        let c = colour_json(Some("#FF0000FF"));
        assert_eq!(c["css"], "rgb(255, 0, 0)");
        assert!(!c["name"].as_str().unwrap().contains('#'));
        assert!(colour_json(Some("nope")).is_null());
        assert!(colour_json(None).is_null());
    }
}
