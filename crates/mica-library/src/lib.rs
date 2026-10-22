// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Mica's model library: a local, read-only index of the folders where the user keeps
//! models, projects and G-code.
//!
//! - **Read-only.** The scanner only lists, reads and hashes files. The index (`index.redb`)
//!   and the thumbnail cache (`thumbs/`) live in the app's data folder, never in a library
//!   folder.
//! - **In the background.** One worker thread owns the index; files are hashed and read on a
//!   small pool (two threads by default) that pauses while Mica slices. The UI only ever
//!   takes a cheap snapshot.
//! - **Incremental.** Unchanged files (same size and modification time) are skipped; live
//!   updates come from the OS's file events (FSEvents on macOS) via `notify`.
//! - **Network shares.** A folder that is missing (an unmounted share) is marked offline and
//!   its models are kept; it is rescanned when it comes back.
//! - **Thumbnails.** Models without an embedded picture (STL, OBJ, many 3MFs) get one rendered
//!   after scanning, on the same pool and also paused while Mica slices, once per content hash
//!   and renderer version (`render`). Meshes above the triangle budget keep a placeholder.
//! - **Duplicates.** Models are keyed by content (xxh3-128), so identical copies are one
//!   model with several locations; tags and favourites follow the content.

pub mod extract;
pub mod group;
pub mod image;
pub mod model;
pub mod query;
pub mod render;
pub mod store;
pub mod testkit;
pub mod walk;
mod worker;

pub use group::{GroupBy, Item, Project, Projects, Section};
pub use model::{
    Extra, ExtraKind, FileRecord, Folder, FolderState, FolderStatus, Grouping, Info, Kind,
    Location, Model, Snapshot, UserData,
};
pub use query::{Colours, Filter, Sliced, Sort};

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

/// How the library runs.
#[derive(Clone, Debug)]
pub struct Options {
    /// Threads hashing and reading files.
    pub threads: usize,
    /// Follow file-system events (off in some tests).
    pub watch: bool,
    /// How often missing folders are checked for coming back.
    pub offline_check: Duration,
    /// Quiet period after the last file event before it is processed.
    pub debounce: Duration,
    /// Render thumbnails for models without one (after scanning, on the same pool).
    pub render: bool,
    /// Meshes with more triangles get no rendered thumbnail (a placeholder shows).
    pub render_budget: u64,
    /// While another Mica has the index (read-only), how often taking it over is tried.
    pub takeover_check: Duration,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            threads: 2,
            watch: true,
            offline_check: Duration::from_secs(15),
            debounce: Duration::from_millis(800),
            render: true,
            render_budget: render::DEFAULT_BUDGET,
            takeover_check: Duration::from_secs(3),
        }
    }
}

pub(crate) enum Cmd {
    AddFolder(PathBuf),
    RemoveFolder(PathBuf),
    Rescan(Option<PathBuf>),
    PersistUser(String),
    PersistGrouping(PathBuf),
    Fs(Vec<PathBuf>),
    FsRescan(PathBuf),
    Shutdown,
}

#[derive(Default)]
pub(crate) struct Shared {
    pub snapshot: Arc<Snapshot>,
    pub generation: u64,
    pub user: HashMap<String, UserData>,
    pub grouping: std::collections::BTreeMap<PathBuf, Grouping>,
    /// Commands sent / fully handled (idle when equal and nothing is queued).
    pub sent: u64,
    pub handled: u64,
    pub busy: bool,
    pub loaded: bool,
    pub error: Option<String>,
    pub warning: Option<String>,
    /// Another Mica has the index: browsing only (see [`READ_ONLY_NOTE`]).
    pub read_only: bool,
    /// Thumbnails rendered (or given up on) since the library was opened.
    pub rendered: u64,
}

/// What the library says while another Mica process has the index open: browsing and search
/// work on what that process last saved; it makes every change.
pub const READ_ONLY_NOTE: &str = "Library is open in another Mica window. Changes are made there.";

/// The library: a handle to the background worker. Dropping it stops the worker.
pub struct Library {
    shared: Arc<Mutex<Shared>>,
    paused: Arc<AtomicBool>,
    tx: mpsc::Sender<Cmd>,
    thread: Option<std::thread::JoinHandle<()>>,
    data_dir: PathBuf,
}

/// `~/Library/Application Support/Mica Slicer/library` (or `$MICA_CONFIG_DIR/library`).
pub fn default_data_dir() -> PathBuf {
    use mica_core::user_data;
    if std::env::var_os("MICA_CONFIG_DIR").is_some()
        || user_data::isolated()
        || cfg!(target_os = "macos")
    {
        return user_data::config_dir().join("library");
    }
    let home = user_data::home().unwrap_or_else(|| PathBuf::from("."));
    user_data::var_dir("XDG_DATA_HOME")
        .unwrap_or_else(|| home.join(".local/share"))
        .join("mica-slicer/library")
}

/// Expands a leading `~/` to the home folder.
pub fn expand_home(p: &Path) -> PathBuf {
    if let Ok(rest) = p.strip_prefix("~")
        && let Some(home) = mica_core::user_data::home()
    {
        return home.join(rest);
    }
    p.to_path_buf()
}

/// `p` with symbolic links resolved, also when it does not exist (yet): the deepest existing
/// ancestor is resolved and the rest appended. A folder added while it is offline (a share not
/// mounted, a folder moved away) is then stored as the same path it has once it is back, so
/// it is not indexed twice under two spellings (`/var/…` and `/private/var/…` on macOS).
pub fn canonical(p: &Path) -> PathBuf {
    if let Ok(c) = std::fs::canonicalize(p) {
        return c;
    }
    let mut rest = Vec::new();
    let mut base = p;
    while let Some(parent) = base.parent() {
        if let Some(name) = base.file_name() {
            rest.push(name.to_os_string());
        }
        base = parent;
        if let Ok(mut c) = std::fs::canonicalize(base) {
            c.extend(rest.iter().rev());
            return c;
        }
    }
    p.to_path_buf()
}

impl Library {
    /// Opens the library stored in `data_dir` and starts its worker (which reads the index
    /// and then scans every configured folder). Never blocks on the index or the folders.
    pub fn open(data_dir: &Path, options: Options) -> Result<Library, String> {
        std::fs::create_dir_all(data_dir.join("thumbs"))
            .map_err(|e| format!("cannot create {}: {e}", data_dir.display()))?;
        let data_dir = canonical(data_dir);
        let shared = Arc::new(Mutex::new(Shared::default()));
        let paused = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        let thread = {
            let shared = shared.clone();
            let paused = paused.clone();
            let tx = tx.clone();
            let data_dir = data_dir.clone();
            std::thread::Builder::new()
                .name("mica-library".into())
                .spawn(move || worker::run(data_dir, options, shared, paused, tx, rx))
                .map_err(|e| e.to_string())?
        };
        Ok(Library {
            shared,
            paused,
            tx,
            thread: Some(thread),
            data_dir,
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Shared> {
        self.shared.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn send(&self, cmd: Cmd) {
        self.lock().sent += 1;
        if self.tx.send(cmd).is_err() {
            // The worker is gone (it failed to open the index); nothing will handle it.
            self.lock().handled += 1;
        }
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// The latest published state (cheap: shared, never copied).
    pub fn snapshot(&self) -> Arc<Snapshot> {
        self.lock().snapshot.clone()
    }

    /// Bumped on every change of the snapshot.
    pub fn generation(&self) -> u64 {
        self.lock().generation
    }

    /// Another Mica process has the index open: this library shows what that process last
    /// saved and changes nothing (adding folders, rescans, tags, favorites and grouping are
    /// refused or ignored). It takes the index over by itself once the other process quits.
    pub fn read_only(&self) -> bool {
        self.lock().read_only
    }

    /// The index could not be opened (permissions, I/O). Another Mica having it open is not an
    /// error: see [`Library::read_only`].
    pub fn error(&self) -> Option<String> {
        self.lock().error.clone()
    }

    /// The initial index open has finished (successfully or with an error).
    pub fn loaded(&self) -> bool {
        self.lock().loaded
    }

    pub fn warning(&self) -> Option<String> {
        self.lock().warning.clone()
    }

    /// Thumbnails this session rendered (or found it could not render).
    pub fn thumbnails_rendered(&self) -> u64 {
        self.lock().rendered
    }

    /// Adds a library folder and scans it. Refuses folders inside the app's data folder and
    /// folders already covered by another library folder.
    pub fn add_folder(&self, path: &Path) -> Result<(), String> {
        if self.read_only() {
            return Err(READ_ONLY_NOTE.into());
        }
        let path = expand_home(path);
        if !path.is_absolute() {
            return Err("Choose a full folder path".into());
        }
        let path = canonical(&path);
        if path.starts_with(&self.data_dir) {
            return Err("That folder is Mica's own data folder".into());
        }
        let snap = self.snapshot();
        for f in &snap.folders {
            if path == f.path {
                return Err("That folder is already in the library".into());
            }
            if path.starts_with(&f.path) {
                return Err(format!("Already included through {}", f.path.display()));
            }
        }
        self.send(Cmd::AddFolder(path));
        Ok(())
    }

    /// Removes a library folder from the index (the folder itself is not touched). Tags and
    /// favourites are kept in case the models come back.
    pub fn remove_folder(&self, path: &Path) {
        if self.read_only() {
            return;
        }
        self.send(Cmd::RemoveFolder(path.to_path_buf()));
    }

    /// Rescans one folder, or all of them.
    pub fn rescan(&self, folder: Option<&Path>) {
        if self.read_only() {
            return;
        }
        self.send(Cmd::Rescan(folder.map(Path::to_path_buf)));
    }

    /// While paused (Mica is slicing), the scanner waits between files.
    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::Relaxed);
    }

    fn update_user(&self, id: &str, f: impl FnOnce(&mut UserData)) {
        {
            let mut s = self.lock();
            if s.read_only {
                return;
            }
            let mut user = s.user.get(id).cloned().unwrap_or_default();
            f(&mut user);
            s.user.insert(id.to_string(), user.clone());
            // Show it at once: patch the published snapshot.
            let snap = Arc::make_mut(&mut s.snapshot);
            for m in snap.models.iter_mut().filter(|m| m.id == id) {
                m.user = user.clone();
            }
            s.generation += 1;
        }
        self.send(Cmd::PersistUser(id.to_string()));
    }

    /// Sets a model's tags (trimmed, de-duplicated, case kept).
    pub fn set_tags(&self, id: &str, tags: &[String]) {
        let mut clean: Vec<String> = vec![];
        for t in tags {
            let t = t.trim();
            if !t.is_empty() && !clean.iter().any(|c| c.eq_ignore_ascii_case(t)) {
                clean.push(t.to_string());
            }
        }
        self.update_user(id, |u| u.tags = clean);
    }

    /// Overrides the automatic project detection for a folder (`None` goes back to it).
    pub fn set_grouping(&self, folder: &Path, grouping: Option<Grouping>) {
        {
            let mut s = self.lock();
            if s.read_only {
                return;
            }
            match grouping {
                Some(g) => s.grouping.insert(folder.to_path_buf(), g),
                None => s.grouping.remove(folder),
            };
            let all = s.grouping.clone();
            Arc::make_mut(&mut s.snapshot).grouping = all;
            s.generation += 1;
        }
        self.send(Cmd::PersistGrouping(folder.to_path_buf()));
    }

    pub fn set_favourite(&self, id: &str, favourite: bool) {
        self.update_user(id, |u| u.favourite = favourite);
    }

    /// Every tag in use, sorted.
    pub fn all_tags(&self) -> Vec<String> {
        let s = self.lock();
        let mut t: Vec<String> = s.user.values().flat_map(|u| u.tags.clone()).collect();
        t.sort_by_key(|a| a.to_lowercase());
        t.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
        t
    }

    /// The worker has handled every command and has nothing queued.
    pub fn idle(&self) -> bool {
        let s = self.lock();
        s.loaded && s.handled >= s.sent && !s.busy
    }

    /// Waits until [`Library::idle`] (tests, screenshots, the benchmark). `false` on timeout.
    pub fn wait_idle(&self, timeout: Duration) -> bool {
        let start = Instant::now();
        while start.elapsed() < timeout {
            if self.idle() || self.error().is_some() {
                return self.error().is_none();
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        false
    }

    /// Waits until `f` holds for the snapshot. `false` on timeout.
    pub fn wait_for(&self, timeout: Duration, f: impl Fn(&Snapshot) -> bool) -> bool {
        let start = Instant::now();
        while start.elapsed() < timeout {
            if f(&self.snapshot()) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    }

    /// The worker thread is still running (a panic there ends it).
    pub fn alive(&self) -> bool {
        self.thread.as_ref().is_some_and(|t| !t.is_finished())
    }
}

impl Drop for Library {
    fn drop(&mut self) {
        let _ = self.tx.send(Cmd::Shutdown);
        // The worker stops between files; a slow read on a network share is not waited for
        // longer than a few seconds (the thread then ends on its own).
        if let Some(t) = self.thread.take() {
            let start = Instant::now();
            while !t.is_finished() && start.elapsed() < Duration::from_secs(5) {
                std::thread::sleep(Duration::from_millis(2));
            }
            if t.is_finished() {
                let _ = t.join();
            }
        }
    }
}

#[cfg(test)]
mod tests;
