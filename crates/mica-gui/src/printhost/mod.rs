// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The GUI's side of printer connections: the list of physical printers (`printers.toml`),
//! their secrets (Keychain), test connections, the live monitors behind the Machine view and
//! the Print dialog's state machine. Everything that talks to a printer runs on a thread;
//! the UI only drains channels.

pub mod cameras;
pub mod catalog;
pub mod config;
pub mod discover;
pub mod explain;
pub mod file_header;
pub mod job;
pub mod keychain;
pub mod monitor;
pub mod poll;
pub mod u1map;

use config::{Fit, Printer};
use explain::Explanation;
use keychain::SecretStore;
use mica_printhost::{
    CameraInfo, Capabilities, LoadedFilament, PrinterHost, PrinterState, Timeouts,
};
use monitor::{Connector, Watcher};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, mpsc},
    thread,
};

/// What "Test connection" found out.
#[derive(Clone, Debug, PartialEq)]
pub struct TestReport {
    pub model: Option<String>,
    pub firmware: Option<String>,
    pub state: PrinterState,
    pub tools: Option<Vec<u32>>,
    pub capabilities: Capabilities,
    pub filaments: Vec<LoadedFilament>,
    pub camera: bool,
    /// The cameras the printer reports (Moonraker's webcam list; empty: none).
    pub cameras: Vec<CameraInfo>,
}
impl TestReport {
    /// The capability words the dialog lists.
    pub fn capability_words(&self) -> Vec<&'static str> {
        let c = self.capabilities;
        let mut words = vec![];
        if c.upload {
            words.push("upload");
        }
        if c.queue {
            words.push("print / queue on upload");
        }
        if c.start {
            words.push("start");
        }
        if c.pause_resume {
            words.push("pause / resume");
        }
        if c.cancel {
            words.push("cancel");
        }
        if c.status {
            words.push("status");
        }
        if c.filaments {
            words.push("loaded filament");
        }
        if self.camera {
            words.push("camera");
        }
        words
    }
}

pub enum Test {
    Running(mpsc::Receiver<Result<TestReport, Explanation>>),
    Done(TestReport),
    Failed(Explanation),
}

/// Set by test harnesses (the monkey test): connections go to loopback addresses only, so
/// random input typed into an address field can never reach a real printer or the network.
static LOOPBACK_ONLY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether this process only talks to printers on this computer (see `restrict_to_loopback`).
pub fn loopback_only() -> bool {
    LOOPBACK_ONLY.load(std::sync::atomic::Ordering::Relaxed)
}

/// Restricts every printer connection of this process to loopback addresses.
pub fn restrict_to_loopback() {
    LOOPBACK_ONLY.store(true, std::sync::atomic::Ordering::Relaxed);
    // Camera URLs (manual or reported by a printer) too.
    mica_camera::restrict_to_loopback();
}

/// Whether a printer address (`host[:port]`, optionally with a scheme and path) names this
/// computer by a literal loopback address or `localhost`.
pub fn is_loopback(address: &str) -> bool {
    let a = address.trim();
    let a = a.split_once("://").map_or(a, |(_, rest)| rest);
    let host = a.split(['/', '?', '#']).next().unwrap_or("");
    let host = host.rsplit_once('@').map_or(host, |(_, h)| h);
    let host = if let Some(rest) = host.strip_prefix('[') {
        rest.split(']').next().unwrap_or("")
    } else {
        host.split(':').next().unwrap_or("")
    };
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

pub struct Printers {
    pub list: Vec<Printer>,
    /// Where the list is saved; `None` never touches the disk (screenshots, tests).
    pub path: Option<PathBuf>,
    pub secrets: Arc<dyn SecretStore>,
    pub dirty: bool,
    /// Why the file could not be read, if it could not.
    pub warning: Option<String>,
    /// Live monitors by printer name, created when first looked at.
    pub watchers: HashMap<String, Watcher>,
    pub tests: HashMap<String, Test>,
    /// The next `Printer::id` to hand out.
    next_id: u64,
}

impl Printers {
    pub fn load(path: Option<PathBuf>, secrets: Arc<dyn SecretStore>) -> Self {
        let (mut list, warning) = match &path {
            Some(p) => config::load(p),
            None => (vec![], None),
        };
        for (i, p) in list.iter_mut().enumerate() {
            p.id = i as u64 + 1;
        }
        Self {
            next_id: list.len() as u64 + 1,
            list,
            path,
            secrets,
            dirty: false,
            warning,
            watchers: HashMap::new(),
            tests: HashMap::new(),
        }
    }

    /// Where the printer with this id is in the list now, if it still exists.
    pub fn position(&self, id: u64) -> Option<usize> {
        self.list.iter().position(|p| p.id == id)
    }

    /// The id of the printer at `index`.
    pub fn id_at(&self, index: usize) -> Option<u64> {
        self.list.get(index).map(|p| p.id)
    }

    pub fn save(&mut self) -> Result<(), String> {
        if let Some(path) = &self.path
            && self.dirty
        {
            config::save(path, &self.list)?;
        }
        self.dirty = false;
        Ok(())
    }

    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.list.iter().position(|p| p.name == name)
    }

    /// Adds or replaces an entry (by position) and stores its secret, if one was typed.
    /// Returns the index of the entry.
    pub fn put(
        &mut self,
        index: Option<usize>,
        mut printer: Printer,
        secret: Option<&str>,
    ) -> Result<usize, String> {
        printer.validate()?;
        if self
            .list
            .iter()
            .enumerate()
            .any(|(i, p)| Some(i) != index && p.name.trim() == printer.name.trim())
        {
            return Err(format!(
                "There is already a printer named {}",
                printer.name.trim()
            ));
        }
        printer.name = printer.name.trim().to_string();
        printer.address = printer.address.trim().to_string();
        if let Some(u) = &printer.username
            && u.trim().is_empty()
        {
            printer.username = None;
        }
        match secret {
            Some(s) if !s.is_empty() => {
                if !self.secrets.available() {
                    return Err(format!(
                        "Mica cannot save the {} here: secret storage is {}",
                        if printer.username.is_some() {
                            "password"
                        } else {
                            "API key"
                        },
                        self.secrets.label()
                    ));
                }
                let reference = printer
                    .keychain
                    .clone()
                    .unwrap_or_else(keychain::new_reference);
                self.secrets.set(&reference, s)?;
                printer.keychain = Some(reference);
                // A typed secret replaces a CLI-style environment reference.
                printer.api_key_env = None;
                printer.password_env = None;
            }
            // An empty field clears the secret ("no key").
            Some(_) => {
                if let Some(r) = printer.keychain.take() {
                    let _ = self.secrets.delete(&r);
                }
            }
            None => {}
        }
        let old_name = index.and_then(|i| self.list.get(i)).map(|p| p.name.clone());
        let i = match index {
            Some(i) if i < self.list.len() => {
                // An edit keeps the entry's identity.
                if self.list[i].kind != printer.kind
                    && self.list[i].keychain != printer.keychain
                    && let Some(reference) = &self.list[i].keychain
                {
                    let _ = self.secrets.delete(reference);
                }
                printer.id = self.list[i].id;
                self.list[i] = printer;
                i
            }
            _ => {
                printer.id = self.next_id;
                self.next_id += 1;
                self.list.push(printer);
                self.list.len() - 1
            }
        };
        // The monitor and test of an edited printer are stale.
        if let Some(n) = old_name {
            self.watchers.remove(&n);
            self.tests.remove(&n);
        }
        self.dirty = true;
        Ok(i)
    }

    pub fn remove(&mut self, index: usize) {
        if index >= self.list.len() {
            return;
        }
        let p = self.list.remove(index);
        for r in [&p.keychain, &p.camera_keychain].into_iter().flatten() {
            let _ = self.secrets.delete(r);
        }
        self.watchers.remove(&p.name);
        self.tests.remove(&p.name);
        self.dirty = true;
    }

    /// Stores (or, with an empty string, forgets) the camera password of the printer at
    /// `index` in the secret store; the list keeps only the reference.
    pub fn put_camera_secret(&mut self, index: usize, secret: &str) -> Result<(), String> {
        let available = self.secrets.available();
        let label = self.secrets.label();
        let p = self.list.get_mut(index).ok_or("no such printer")?;
        if secret.is_empty() {
            if let Some(r) = p.camera_keychain.take() {
                let _ = self.secrets.delete(&r);
                self.dirty = true;
            }
            return Ok(());
        }
        if !available {
            return Err(format!(
                "Mica cannot save the camera password here: secret storage is {label}"
            ));
        }
        let reference = p
            .camera_keychain
            .clone()
            .unwrap_or_else(keychain::new_reference);
        self.secrets.set(&reference, secret)?;
        p.camera_keychain = Some(reference);
        self.dirty = true;
        Ok(())
    }

    /// The camera login of the printer at `index` (secret read on this thread).
    pub fn camera_auth(&self, index: usize) -> Result<Option<mica_camera::Auth>, String> {
        self.list
            .get(index)
            .ok_or("no such printer")?
            .camera_auth(self.secrets.as_ref())
    }

    /// A connection factory for the printer at `index`: the secret is read now, on the
    /// calling thread, so the Keychain is never touched from a worker.
    pub fn connector(&self, index: usize) -> Result<Connector, String> {
        let p = self.list.get(index).ok_or("no such printer")?;
        if LOOPBACK_ONLY.load(std::sync::atomic::Ordering::Relaxed) && !is_loopback(&p.address) {
            return Err(format!(
                "{}: this run only talks to printers on this computer (127.0.0.1)",
                p.address
            ));
        }
        let config = p.config(self.secrets.as_ref())?;
        Ok(Arc::new(move || {
            mica_printhost::connect(config.clone(), Timeouts::default()).map_err(|e| e.to_string())
        }))
    }

    pub fn target(&self, index: usize) -> Option<job::Target> {
        self.list.get(index).map(|p| job::Target {
            id: p.id,
            name: p.name.clone(),
            kind: p.kind,
            address: p.address.clone(),
        })
    }

    /// "Test connection": info, status, loaded filaments and the camera, on a thread.
    pub fn test(&mut self, index: usize, wake: impl Fn() + Send + 'static) {
        let Some(p) = self.list.get(index).cloned() else {
            return;
        };
        let connector = self.connector(index);
        let rx = spawn_test(p.clone(), connector, wake);
        self.tests.insert(p.name.clone(), Test::Running(rx));
    }

    /// "Test connection" for a printer not saved yet (the Add printer flow): `secret` is the
    /// typed key or password, used for this test only and never stored here.
    pub fn test_unsaved(
        &self,
        printer: &Printer,
        secret: Option<&str>,
        wake: impl Fn() + Send + 'static,
    ) -> mpsc::Receiver<Result<TestReport, Explanation>> {
        let connector = unsaved_connector(printer, secret);
        spawn_test(printer.clone(), connector, wake)
    }
}

/// A connection factory for an entry that is not in the list (yet), with a typed secret.
pub fn unsaved_connector(printer: &Printer, secret: Option<&str>) -> Result<Connector, String> {
    if loopback_only() && !is_loopback(&printer.address) {
        return Err(format!(
            "{}: this run only talks to printers on this computer (127.0.0.1)",
            printer.address
        ));
    }
    let mut p = printer.clone();
    if p.name.trim().is_empty() {
        p.name = "the printer".into();
    }
    p.validate()?;
    p.keychain = None;
    p.api_key_env = None;
    p.password_env = None;
    let mut config = p.config(&keychain::MemoryStore::default())?;
    if let Some(s) = secret.filter(|s| !s.is_empty()) {
        config.credentials = match &p.username {
            Some(u) if p.kind == config::Kind::PrusaLink => mica_printhost::Credentials::Digest {
                username: u.clone(),
                password: s.to_string(),
            },
            _ => mica_printhost::Credentials::ApiKey(s.to_string()),
        };
    }
    Ok(Arc::new(move || {
        mica_printhost::connect(config.clone(), Timeouts::default()).map_err(|e| e.to_string())
    }))
}

fn spawn_test(
    p: Printer,
    connector: Result<Connector, String>,
    wake: impl Fn() + Send + 'static,
) -> mpsc::Receiver<Result<TestReport, Explanation>> {
    let (tx, rx) = mpsc::channel();
    thread::Builder::new()
        .name("mica-printer-test".into())
        .spawn(move || {
            let result = match connector {
                Err(e) => Err(Explanation {
                    headline: format!("Mica cannot connect to {}", p.name),
                    advice: e,
                }),
                Ok(connect) => match connect() {
                    Err(e) => Err(Explanation {
                        headline: format!("Mica cannot connect to {}", p.name),
                        advice: e,
                    }),
                    Ok(h) => test_host(h.as_ref())
                        .map_err(|e| explain::explain(&e, p.kind, &p.name, &p.address)),
                },
            };
            let _ = tx.send(result);
            wake();
        })
        .ok();
    rx
}

impl Printers {
    /// Drains finished tests; a successful one records the reported model for profile
    /// matching. True when something changed.
    pub fn poll_tests(&mut self) -> bool {
        let mut changed = false;
        let mut models: Vec<(String, String)> = vec![];
        for (name, test) in &mut self.tests {
            if let Test::Running(rx) = test
                && let Ok(result) = rx.try_recv()
            {
                changed = true;
                *test = match result {
                    Ok(report) => {
                        if let Some(m) = &report.model {
                            models.push((name.clone(), m.clone()));
                        }
                        Test::Done(report)
                    }
                    Err(e) => Test::Failed(e),
                };
            }
        }
        for (name, model) in models {
            if let Some(p) = self.list.iter_mut().find(|p| p.name == name)
                && p.model.as_deref() != Some(&model)
            {
                p.model = Some(model);
                self.dirty = true;
            }
        }
        changed
    }

    /// The monitor of the printer at `index`, started on first use (idle until given a rate).
    pub fn watcher(
        &mut self,
        index: usize,
        wake: impl Fn() + Send + 'static,
    ) -> Option<&mut Watcher> {
        let printer = self.list.get(index)?;
        if printer.kind == config::Kind::PrusaConnect {
            return None;
        }
        let name = printer.name.clone();
        if !self.watchers.contains_key(&name) {
            let connector: Connector = match self.connector(index) {
                Ok(c) => c,
                Err(e) => Arc::new(move || Err(e.clone())),
            };
            self.watchers
                .insert(name.clone(), Watcher::start(connector, wake));
        }
        self.watchers.get_mut(&name)
    }

    /// Polling intervals for this frame (see `poll::wanted`): the printers on screen
    /// (`watching`: the dashboard's tiles or the Machine view's printer) poll at the rate
    /// their state asks for; a running job elsewhere keeps a slow watch; nothing polls while
    /// the window is minimised (`window` false).
    pub fn schedule(&mut self, watching: &[usize], window: bool) {
        let watched: Vec<&str> = watching
            .iter()
            .filter_map(|&i| self.list.get(i))
            .map(|p| p.name.as_str())
            .collect();
        for (name, w) in &mut self.watchers {
            let visible = watched.contains(&name.as_str());
            let state = w.live.status.as_ref().map(|s| s.state);
            w.set_interval(poll::wanted(visible, window, state, w.live.failed()));
        }
    }

    /// Drains every monitor; true when any changed.
    pub fn drain(&mut self) -> bool {
        let mut any = self.poll_tests();
        for w in self.watchers.values_mut() {
            any |= w.drain();
        }
        any
    }

    /// Physical printers ordered for a profile (see `config::ranked`).
    pub fn ranked(&self, vendor: &str, preset: &str) -> Vec<(usize, Fit)> {
        let (model, _) = mica_profiles::project::split_printer_name(preset);
        config::ranked(&self.list, vendor, preset, &model)
    }

    /// The printer a profile prints on by default: the first that fits (never one of
    /// another family).
    pub fn default_for(&self, vendor: &str, preset: &str) -> Option<usize> {
        self.ranked(vendor, preset)
            .into_iter()
            .find(|(_, f)| *f != Fit::Other)
            .map(|(i, _)| i)
    }

    /// What the printer at `index` reports as loaded, from its monitor or its last test.
    pub fn loaded_filaments(&self, index: usize) -> Option<&[LoadedFilament]> {
        let p = self.list.get(index)?;
        if let Some(w) = self.watchers.get(&p.name)
            && w.live.filaments_known
        {
            return Some(&w.live.filaments);
        }
        match self.tests.get(&p.name) {
            Some(Test::Done(r)) => Some(&r.filaments),
            _ => None,
        }
    }

    /// The live state of the printer at `index`, if it is being watched.
    pub fn live(&self, index: usize) -> Option<&monitor::Live> {
        let p = self.list.get(index)?;
        self.watchers.get(&p.name).map(|w| &w.live)
    }
}

fn test_host(h: &dyn PrinterHost) -> mica_printhost::Result<TestReport> {
    let info = h.info()?;
    let state = if h.capabilities().status {
        h.status()?.state
    } else {
        mica_printhost::PrinterState::Offline
    };
    let filaments = h.loaded_filaments().unwrap_or_default();
    let cameras = h.cameras().unwrap_or_default();
    let camera = !cameras.is_empty();
    Ok(TestReport {
        model: info.model,
        firmware: info.firmware,
        state,
        tools: info.tools,
        capabilities: h.capabilities(),
        filaments,
        camera,
        cameras,
    })
}

#[cfg(test)]
mod tests {
    use super::config::Kind;
    use super::*;
    use keychain::MemoryStore;

    fn printers() -> Printers {
        Printers::load(None, Arc::new(MemoryStore::default()))
    }

    #[test]
    fn put_stores_the_secret_in_the_store_and_only_a_reference_in_the_list() {
        let mut p = printers();
        let mut xl = Printer::new(Kind::PrusaLink);
        xl.name = " XL ".into();
        xl.address = "xl.local ".into();
        let i = p.put(None, xl.clone(), Some("KEY-1")).unwrap();
        assert_eq!(p.list[i].name, "XL");
        assert_eq!(p.list[i].address, "xl.local");
        let reference = p.list[i].keychain.clone().expect("a reference");
        assert_eq!(p.secrets.get(&reference).unwrap().as_deref(), Some("KEY-1"));
        assert!(!config::to_toml(&p.list).contains("KEY-1"));
        assert!(p.dirty);
        // Editing keeps the reference; a new secret overwrites in place.
        let mut edited = p.list[i].clone();
        edited.address = "192.0.2.50".into();
        p.put(Some(i), edited, Some("KEY-2")).unwrap();
        assert_eq!(p.list[i].keychain.as_deref(), Some(reference.as_str()));
        assert_eq!(p.secrets.get(&reference).unwrap().as_deref(), Some("KEY-2"));
        // No change to the secret field leaves it alone; an empty one clears it.
        let edited = p.list[i].clone();
        p.put(Some(i), edited.clone(), None).unwrap();
        assert_eq!(p.secrets.get(&reference).unwrap().as_deref(), Some("KEY-2"));
        p.put(Some(i), edited, Some("")).unwrap();
        assert_eq!(p.list[i].keychain, None);
        assert_eq!(p.secrets.get(&reference).unwrap(), None);
        // Duplicate names are refused; removing deletes the secret.
        let mut again = Printer::new(Kind::PrusaLink);
        again.name = "XL".into();
        again.address = "x".into();
        assert!(p.put(None, again, None).is_err());
        p.put(Some(i), p.list[i].clone(), Some("KEY-3")).unwrap();
        let reference = p.list[i].keychain.clone().unwrap();
        p.remove(i);
        assert!(p.list.is_empty());
        assert_eq!(p.secrets.get(&reference).unwrap(), None);
        // Missing-secret connectors fail readably, without touching the network.
        let mut u1 = Printer::new(Kind::SnapmakerU1);
        u1.name = "U1".into();
        // A loopback address: the monkey test in the same process may have switched on the
        // loopback-only guard, which would answer first for any other address.
        u1.address = "127.0.0.1:7125".into();
        u1.keychain = Some("gone".into());
        let i = p.put(None, u1, None).unwrap();
        match p.connector(i) {
            Err(e) => assert!(e.contains("missing")),
            Ok(_) => panic!("a missing secret must not connect"),
        }
        assert_eq!(
            p.default_for("Snapmaker", "Snapmaker U1 (0.4 nozzle)"),
            Some(i)
        );
        assert_eq!(p.default_for("Prusa", "Prusa CORE One HF0.4 nozzle"), None);
    }
    #[test]
    fn connect_credentials_use_the_existing_store_and_never_start_a_monitor() {
        let store = Arc::new(MemoryStore::default());
        let mut printers = Printers::load(None, store.clone());
        let mut p = Printer::new(Kind::PrusaConnect);
        p.name = "Cloud XL".into();
        p.connect_printer_id = Some("1234-abcd".into());
        let index = printers.put(None, p, Some("fake-connect-key")).unwrap();
        let p = &printers.list[index];
        let reference = p.keychain.as_ref().unwrap().clone();
        assert_eq!(
            store.get(&reference).unwrap().as_deref(),
            Some("fake-connect-key")
        );
        let text = config::to_toml(&printers.list);
        assert!(!text.contains("fake-connect-key"));
        assert!(text.contains("PrusaConnect"));
        let p = config::parse(&text).unwrap().remove(0);
        assert_eq!(
            p.connect_page().as_deref(),
            Some("https://connect.prusa3d.com/printer/1234-abcd/dashboard")
        );
        assert!(matches!(
            p.credentials(store.as_ref()).unwrap(),
            mica_printhost::Credentials::ApiKey(_)
        ));
        assert!(
            printers
                .watcher(index, || panic!("no polling wake"))
                .is_none()
        );
        printers.schedule(&[index], true);
        assert!(printers.watchers.is_empty());
        let mut bad = p;
        bad.connect_printer_id = Some("id?secret=bad".into());
        assert!(bad.validate().is_err());
        assert!(bad.connect_page().is_none());
        printers.remove(index);
        assert!(store.get(&reference).unwrap().is_none());
    }
}
