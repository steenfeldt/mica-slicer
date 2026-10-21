// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The library's worker thread: owns the index, scans folders, follows file events and
//! publishes snapshots. Every write it makes goes to the app's data folder.

use crate::extract::{Extracted, extract};
use crate::model::{
    Extra, ExtraKind, ExtraRecord, FileRecord, FolderState, FolderStatus, Info, Kind, Location,
    Model, Snapshot,
};
use crate::store::{Changes, Index, Store};
use crate::walk::{Found, FoundExtra, mtime_ns, skipped_path, walk};
use crate::{Cmd, Options, Shared};
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

/// Files up to this size are read once into memory (hashed and parsed from it); larger ones
/// are hashed as a stream and only G-code is read further (its start and end).
const IN_MEMORY: u64 = 384 << 20;
/// Files processed between commits, progress updates and command checks.
const BATCH: usize = 24;
/// Snapshots are published at most this often during a scan.
const PUBLISH_EVERY: Duration = Duration::from_millis(400);
/// Pictures per folder that get a cached thumbnail (the likeliest covers first).
const PICTURES_PER_FOLDER: usize = 4;
/// Pictures larger than this are not read.
const MAX_PICTURE: u64 = 40 << 20;
/// Cached picture thumbnails fit in this many pixels square.
const PICTURE_PX: u32 = 256;

struct Worker {
    options: Options,
    shared: Arc<Mutex<Shared>>,
    paused: Arc<AtomicBool>,
    tx: mpsc::Sender<Cmd>,
    rx: mpsc::Receiver<Cmd>,
    store: Store,
    index: Index,
    thumbs: PathBuf,
    data_dir: PathBuf,
    pool: rayon::ThreadPool,
    status: HashMap<PathBuf, FolderStatus>,
    /// How many indexed files hold each content hash.
    refs: HashMap<String, usize>,
    /// Folders waiting for a full scan, in order.
    queue: VecDeque<PathBuf>,
    /// Changed paths from file events and when they arrived last.
    events: BTreeSet<PathBuf>,
    last_event: Instant,
    watchers: HashMap<PathBuf, notify::RecommendedWatcher>,
    /// Each folder's canonical path (file events report those).
    canonical: HashMap<PathBuf, PathBuf>,
    last_publish: Instant,
    last_offline_check: Instant,
    dirty: bool,
    shutdown: bool,
    /// Set when the folder being scanned was removed meanwhile.
    cancel: Option<PathBuf>,
    /// Content hashes waiting for a rendered thumbnail, in order (after scanning).
    renders: VecDeque<String>,
    /// Pictures that could not be read this session (not tried again until they change).
    bad_pictures: HashSet<PathBuf>,
}

pub(crate) fn run(
    data_dir: PathBuf,
    options: Options,
    shared: Arc<Mutex<Shared>>,
    paused: Arc<AtomicBool>,
    tx: mpsc::Sender<Cmd>,
    rx: mpsc::Receiver<Cmd>,
) {
    let lock = |s: &Arc<Mutex<Shared>>| {
        let mut g = s.lock().unwrap_or_else(|e| e.into_inner());
        g.loaded = true;
        g.handled = g.sent;
        drop(g);
    };
    let (store, index, warning) = match Store::open(&data_dir.join("index.redb")) {
        Ok(v) => v,
        Err(e) => {
            let mut g = shared.lock().unwrap_or_else(|e| e.into_inner());
            g.error = Some(e);
            drop(g);
            lock(&shared);
            return;
        }
    };
    let pool = match rayon::ThreadPoolBuilder::new()
        .num_threads(options.threads.max(1))
        .thread_name(|i| format!("mica-library-{i}"))
        .build()
    {
        Ok(p) => p,
        Err(e) => {
            shared.lock().unwrap_or_else(|e| e.into_inner()).error = Some(e.to_string());
            lock(&shared);
            return;
        }
    };
    {
        let mut g = shared.lock().unwrap_or_else(|e| e.into_inner());
        g.user = index.user.clone();
        g.grouping = index.grouping.clone();
        g.warning = warning;
    }
    let mut w = Worker {
        options,
        shared,
        paused,
        tx,
        rx,
        store,
        thumbs: data_dir.join("thumbs"),
        data_dir,
        pool,
        status: HashMap::new(),
        refs: HashMap::new(),
        queue: VecDeque::new(),
        events: BTreeSet::new(),
        last_event: Instant::now(),
        watchers: HashMap::new(),
        canonical: HashMap::new(),
        last_publish: Instant::now(),
        last_offline_check: Instant::now(),
        dirty: true,
        shutdown: false,
        cancel: None,
        renders: VecDeque::new(),
        bad_pictures: HashSet::new(),
        index,
    };
    for r in w.index.files.values() {
        *w.refs.entry(r.hash.clone()).or_default() += 1;
    }
    for f in w.index.folders.clone() {
        w.status.insert(f.clone(), status(&f, FolderState::Waiting));
        w.queue.push_back(f);
    }
    // Thumbnails an earlier session did not get to (resumed; nothing is rendered twice).
    w.queue_renders();
    w.publish(true);
    {
        let busy = !w.queue.is_empty();
        let mut g = w.lock();
        g.busy = busy;
        g.loaded = true;
    }
    w.main_loop();
}

fn status(path: &Path, state: FolderState) -> FolderStatus {
    FolderStatus {
        path: path.to_path_buf(),
        state,
        files: 0,
        watching: false,
        last_scan_s: None,
    }
}

/// A folder that should exist but does not, or a share mount point that has gone.
fn available(root: &Path) -> bool {
    root.is_dir()
}

/// Folders on another volume (macOS `/Volumes/…`): an empty listing there more likely means
/// a share that is not mounted than a folder that was emptied.
fn on_removable_volume(root: &Path) -> bool {
    root.starts_with("/Volumes") || root.starts_with("/media") || root.starts_with("/mnt")
}

/// Writes a small PNG thumbnail of the picture at `path` to `dest`. `false` when the picture
/// cannot be read.
fn picture_thumbnail(path: &Path, dest: &Path) -> bool {
    let Ok(data) = std::fs::read(path) else {
        return false;
    };
    let Ok(img) = image::load_from_memory(&data) else {
        return false;
    };
    let t = img.thumbnail(PICTURE_PX, PICTURE_PX).to_rgba8();
    let Ok(png) = mica_gcode::thumbnails::encode_png(t.width(), t.height(), t.as_raw()) else {
        return false;
    };
    let tmp = dest.with_extension("png.tmp");
    std::fs::write(&tmp, png).is_ok() && std::fs::rename(&tmp, dest).is_ok()
}

/// The cached thumbnail's file name for a picture (changes when the picture does).
fn picture_thumb_name(path: &Path, size: u64, mtime_ns: i128) -> String {
    let key = format!("{}|{size}|{mtime_ns}", path.display());
    format!("x-{:032x}.png", xxhash_rust::xxh3::xxh3_128(key.as_bytes()))
}

/// Hash (and, for new content, read) one file.
fn read_one(
    f: &Found,
    models: &HashMap<String, Info>,
) -> std::io::Result<(Found, String, Option<Extracted>)> {
    let meta = std::fs::metadata(&f.path)?;
    let found = Found {
        size: meta.len(),
        mtime_ns: mtime_ns(&meta),
        ..f.clone()
    };
    let (hash, data) = if meta.len() <= IN_MEMORY {
        let data = std::fs::read(&f.path)?;
        (xxhash_rust::xxh3::xxh3_128(&data), Some(data))
    } else {
        use std::io::Read;
        let mut h = xxhash_rust::xxh3::Xxh3::new();
        let mut file = std::fs::File::open(&f.path)?;
        let mut buf = vec![0; 4 << 20];
        loop {
            let n = file.read(&mut buf)?;
            if n == 0 {
                break;
            }
            h.update(&buf[..n]);
        }
        (h.digest128(), None)
    };
    let hash = format!("{hash:032x}");
    let extracted =
        (!models.contains_key(&hash)).then(|| extract(f.kind, &f.path, data.as_deref()));
    Ok((found, hash, extracted))
}

impl Worker {
    fn lock(&self) -> std::sync::MutexGuard<'_, Shared> {
        self.shared.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn main_loop(&mut self) {
        while !self.shutdown {
            let paused = self.paused.load(Ordering::Relaxed);
            let busy = !self.queue.is_empty()
                || !self.events.is_empty()
                || (!self.renders.is_empty() && !paused);
            let wait = if !self.events.is_empty() {
                Duration::from_millis(50)
            } else if busy {
                Duration::ZERO
            } else {
                Duration::from_millis(500)
            };
            match self.rx.recv_timeout(wait) {
                Ok(cmd) => {
                    self.handle(cmd);
                    // Handle everything already waiting before starting work.
                    while let Ok(cmd) = self.rx.try_recv() {
                        self.handle(cmd);
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
            if self.shutdown {
                break;
            }
            if let Some(root) = self.queue.pop_front() {
                self.scan_root(&root);
                self.queue_renders();
                if self.queue.is_empty() {
                    self.collect_garbage();
                }
            } else if !self.events.is_empty() && self.last_event.elapsed() >= self.options.debounce
            {
                self.process_events();
                self.queue_renders();
            } else if self.events.is_empty()
                && !self.renders.is_empty()
                && !self.paused.load(Ordering::Relaxed)
            {
                // Scanning comes first; thumbnails follow, a few at a time, and wait while
                // Mica slices.
                self.render_batch();
            }
            if self.last_offline_check.elapsed() >= self.options.offline_check {
                self.check_availability();
            }
            self.flush(&mut Changes::default());
            if self.dirty {
                self.publish(true);
            }
            let idle = self.queue.is_empty() && self.events.is_empty() && self.renders.is_empty();
            let mut g = self.lock();
            g.busy = !idle;
        }
        self.flush(&mut Changes::default());
    }

    /// Every model still needing a rendered thumbnail, queued once, in name order.
    fn queue_renders(&mut self) {
        if !self.options.render {
            return;
        }
        let queued: HashSet<String> = self.renders.iter().cloned().collect();
        let mut kinds: HashMap<&str, (Kind, &Path)> = HashMap::new();
        for (p, r) in &self.index.files {
            kinds
                .entry(r.hash.as_str())
                .or_insert((r.kind, p.as_path()));
        }
        let mut new: Vec<(String, String)> = self
            .index
            .models
            .iter()
            .filter(|(h, info)| {
                !queued.contains(*h)
                    && kinds
                        .get(h.as_str())
                        .is_some_and(|(k, _)| crate::render::needs_render(*k, info))
            })
            .map(|(h, _)| {
                let name = kinds[h.as_str()]
                    .1
                    .file_name()
                    .map(|n| n.to_string_lossy().to_lowercase())
                    .unwrap_or_default();
                (name, h.clone())
            })
            .collect();
        if new.is_empty() {
            return;
        }
        // The grid's default order, so the top of the library fills in first.
        new.sort();
        self.renders.extend(new.into_iter().map(|(_, h)| h));
        self.dirty = true;
    }

    /// Renders the next few thumbnails on the pool and stores them in the app's data folder.
    fn render_batch(&mut self) {
        let n = self.options.threads.max(1);
        let mut want: Vec<String> = vec![];
        while want.len() < n
            && let Some(h) = self.renders.pop_front()
        {
            want.push(h);
        }
        // A readable copy of each (online folders first).
        let offline: HashSet<&Path> = self
            .status
            .iter()
            .filter(|(_, s)| s.state == FolderState::Offline)
            .map(|(p, _)| p.as_path())
            .collect();
        let wanted: HashSet<&str> = want.iter().map(String::as_str).collect();
        let mut at: HashMap<&str, (Kind, PathBuf)> = HashMap::new();
        for (p, r) in &self.index.files {
            if wanted.contains(r.hash.as_str()) && !offline.contains(r.root.as_path()) {
                at.entry(r.hash.as_str())
                    .or_insert_with(|| (r.kind, p.clone()));
            }
        }
        let budget = self.options.render_budget;
        let jobs: Vec<(String, Kind, PathBuf, u64)> = want
            .iter()
            .filter_map(|h| {
                let info = self.index.models.get(h)?;
                let (kind, path) = at.get(h.as_str())?.clone();
                crate::render::needs_render(kind, info)
                    .then(|| (h.clone(), kind, path, info.triangles))
            })
            .collect();
        if jobs.is_empty() {
            return;
        }
        let results: Vec<(String, crate::render::Rendered)> = self.pool.install(|| {
            use rayon::prelude::*;
            jobs.par_iter()
                .map(|(h, kind, path, tris)| {
                    (
                        h.clone(),
                        crate::render::render_file(*kind, path, *tris, budget),
                    )
                })
                .collect()
        });
        let mut c = Changes::default();
        let mut done = 0;
        for (hash, r) in results {
            let Some(mut info) = self.index.models.get(&hash).cloned() else {
                continue;
            };
            match r {
                crate::render::Rendered::Png(png) => {
                    let dest = self.thumbs.join(format!("{hash}.png"));
                    let tmp = self.thumbs.join(format!(".{hash}.png.tmp"));
                    if std::fs::write(&tmp, &png).is_err() || std::fs::rename(&tmp, &dest).is_err()
                    {
                        let _ = std::fs::remove_file(&tmp);
                        self.lock().warning = Some(format!(
                            "Could not store a thumbnail in {}",
                            self.thumbs.display()
                        ));
                        continue;
                    }
                    if let Some(old) = info.thumbnail.as_deref()
                        && old != "png"
                    {
                        let _ = std::fs::remove_file(self.thumbs.join(format!("{hash}.{old}")));
                    }
                    info.thumbnail = Some("png".into());
                    info.thumbnail_rendered = true;
                    info.no_render = None;
                }
                crate::render::Rendered::Skipped(why) => info.no_render = Some(why),
                // Gone or offline meanwhile: a later scan queues it again.
                crate::render::Rendered::Unavailable => continue,
            }
            info.render_version = crate::render::RENDER_VERSION;
            c.models.push((hash.clone(), Some(info.clone())));
            self.index.models.insert(hash, info);
            done += 1;
        }
        self.flush(&mut c);
        self.lock().rendered += done;
        self.dirty = true;
        self.publish(self.renders.is_empty());
    }

    /// Handles one command. During a scan this runs between batches.
    fn handle(&mut self, cmd: Cmd) {
        // File events come from the watcher, not the `Library` handle: they are not counted.
        let counted = !matches!(cmd, Cmd::Fs(_) | Cmd::FsRescan(_));
        match cmd {
            Cmd::AddFolder(path) => {
                if !self.index.folders.contains(&path) {
                    self.index.folders.push(path.clone());
                    self.index.folders.sort();
                    let mut c = Changes {
                        folders: Some(self.index.folders.clone()),
                        ..Default::default()
                    };
                    self.flush(&mut c);
                    self.status
                        .insert(path.clone(), status(&path, FolderState::Waiting));
                    self.queue.push_back(path);
                    self.dirty = true;
                }
            }
            Cmd::RemoveFolder(path) => {
                self.index.folders.retain(|f| f != &path);
                self.queue.retain(|f| f != &path);
                self.status.remove(&path);
                self.watchers.remove(&path);
                self.canonical.remove(&path);
                let mut c = Changes {
                    folders: Some(self.index.folders.clone()),
                    ..Default::default()
                };
                let gone: Vec<PathBuf> = self
                    .index
                    .files
                    .iter()
                    .filter(|(_, r)| r.root == path)
                    .map(|(p, _)| p.clone())
                    .collect();
                for p in gone {
                    self.remove_file(&p, &mut c);
                }
                let gone: Vec<PathBuf> = self
                    .index
                    .extras
                    .iter()
                    .filter(|(_, r)| r.root == path)
                    .map(|(p, _)| p.clone())
                    .collect();
                for p in gone {
                    self.remove_extra(&p, &mut c);
                }
                self.flush(&mut c);
                self.cancel = Some(path);
                self.dirty = true;
            }
            Cmd::Rescan(folder) => {
                let all = self.index.folders.clone();
                for f in all
                    .into_iter()
                    .filter(|f| folder.as_ref().is_none_or(|x| x == f))
                {
                    if !self.queue.contains(&f) {
                        self.queue.push_back(f);
                    }
                }
            }
            Cmd::PersistGrouping(path) => {
                let g = self.lock().grouping.get(&path).copied();
                match g {
                    Some(g) => self.index.grouping.insert(path.clone(), g),
                    None => self.index.grouping.remove(&path),
                };
                let mut c = Changes {
                    grouping: vec![(path, g)],
                    ..Default::default()
                };
                self.flush(&mut c);
            }
            Cmd::PersistUser(id) => {
                let user = self.lock().user.get(&id).cloned();
                let mut c = Changes {
                    user: vec![(id, user)],
                    ..Default::default()
                };
                self.flush(&mut c);
            }
            Cmd::Fs(paths) => {
                self.events.extend(paths);
                self.last_event = Instant::now();
            }
            Cmd::FsRescan(root) => {
                if !self.queue.contains(&root) {
                    self.queue.push_back(root);
                }
            }
            Cmd::Shutdown => self.shutdown = true,
        }
        let mut g = self.lock();
        if counted {
            g.handled += 1;
        }
        g.busy = true;
    }

    /// Between batches: commands, the pause while Mica slices. `true` to stop the scan of
    /// `root`.
    fn checkpoint(&mut self, root: &Path) -> bool {
        loop {
            while let Ok(cmd) = self.rx.try_recv() {
                self.handle(cmd);
            }
            if self.shutdown || self.cancel.as_deref() == Some(root) {
                return true;
            }
            if !self.paused.load(Ordering::Relaxed) {
                return false;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    fn flush(&mut self, c: &mut Changes) {
        if c.is_empty() {
            return;
        }
        if let Err(e) = self.store.write(c) {
            self.lock().warning = Some(e);
        }
        *c = Changes::default();
    }

    fn set_state(&mut self, root: &Path, state: FolderState) {
        if let Some(s) = self.status.get_mut(root) {
            s.state = state;
        }
        self.dirty = true;
    }

    fn remove_file(&mut self, path: &Path, c: &mut Changes) {
        if let Some(old) = self.index.files.remove(path) {
            c.files.push((path.to_path_buf(), None));
            self.unref(&old.hash);
            self.release_hash(&old.hash, c);
        }
    }

    fn remove_extra(&mut self, path: &Path, c: &mut Changes) {
        if let Some(old) = self.index.extras.remove(path) {
            c.extras.push((path.to_path_buf(), None));
            if let Some(t) = old.thumbnail {
                let _ = std::fs::remove_file(self.thumbs.join(t));
            }
        }
    }

    /// Records the pictures and documents found under `root`. After a full walk
    /// (`unreadable` is then the walk's unreadable folders), the ones not found are dropped.
    fn sync_extras(&mut self, root: &Path, found: Vec<FoundExtra>, unreadable: Option<&[PathBuf]>) {
        let mut c = Changes::default();
        if let Some(unreadable) = unreadable {
            let seen: HashSet<&Path> = found.iter().map(|f| f.path.as_path()).collect();
            let gone: Vec<PathBuf> = self
                .index
                .extras
                .iter()
                .filter(|(p, r)| {
                    r.root == root
                        && !seen.contains(p.as_path())
                        && !unreadable.iter().any(|u| p.starts_with(u))
                })
                .map(|(p, _)| p.clone())
                .collect();
            for p in gone {
                self.remove_extra(&p, &mut c);
            }
        }
        for f in found {
            if let Some(old) = self.index.extras.get(&f.path) {
                if old.size == f.size
                    && old.mtime_ns == f.mtime_ns
                    && old.root == root
                    && old.kind == f.kind
                {
                    continue;
                }
                self.remove_extra(&f.path, &mut c);
            }
            self.bad_pictures.remove(&f.path);
            let rec = ExtraRecord {
                root: root.to_path_buf(),
                size: f.size,
                mtime_ns: f.mtime_ns,
                kind: f.kind,
                thumbnail: None,
            };
            self.index.extras.insert(f.path.clone(), rec.clone());
            c.extras.push((f.path, Some(rec)));
            self.dirty = true;
        }
        self.flush(&mut c);
        self.thumbnail_pictures(root);
    }

    /// Caches thumbnails of the pictures that can be a project's cover: up to a few per
    /// folder, in folders holding models or next to them (`images/` beside the parts, a
    /// `cover.jpg` above `files/`).
    fn thumbnail_pictures(&mut self, root: &Path) {
        let mut model_dirs: HashSet<&Path> = HashSet::new();
        let mut near: HashSet<&Path> = HashSet::new();
        for (p, r) in self.index.files.range(root.to_path_buf()..) {
            if !p.starts_with(root) {
                break;
            }
            if r.root != root {
                continue;
            }
            let Some(d) = p.parent() else { continue };
            if model_dirs.insert(d) {
                for a in d.ancestors().take(3) {
                    if !a.starts_with(root) {
                        break;
                    }
                    near.insert(a);
                }
            }
        }
        let mut by_dir: HashMap<&Path, Vec<(&PathBuf, &ExtraRecord)>> = HashMap::new();
        for (p, r) in self.index.extras.range(root.to_path_buf()..) {
            if !p.starts_with(root) {
                break;
            }
            if r.root == root
                && r.kind == ExtraKind::Image
                && let Some(d) = p.parent()
                && (near.contains(d) || d.parent().is_some_and(|d| model_dirs.contains(d)))
            {
                by_dir.entry(d).or_default().push((p, r));
            }
        }
        let mut todo: Vec<(PathBuf, String)> = vec![];
        for (_, mut pics) in by_dir {
            pics.sort_by_key(|(p, _)| (!crate::group::cover_name(p), (*p).clone()));
            for (p, r) in pics.into_iter().take(PICTURES_PER_FOLDER) {
                if r.thumbnail.is_none() && r.size <= MAX_PICTURE && !self.bad_pictures.contains(p)
                {
                    todo.push((p.clone(), picture_thumb_name(p, r.size, r.mtime_ns)));
                }
            }
        }
        todo.sort();
        for batch in todo.chunks(BATCH) {
            if self.checkpoint(root) {
                return;
            }
            let thumbs = &self.thumbs;
            let done: Vec<bool> = self.pool.install(|| {
                use rayon::prelude::*;
                batch
                    .par_iter()
                    .map(|(p, name)| picture_thumbnail(p, &thumbs.join(name)))
                    .collect()
            });
            let mut c = Changes::default();
            for ((p, name), ok) in batch.iter().zip(done) {
                if !ok {
                    self.bad_pictures.insert(p.clone());
                    continue;
                }
                if let Some(r) = self.index.extras.get_mut(p) {
                    r.thumbnail = Some(name.clone());
                    c.extras.push((p.clone(), Some(r.clone())));
                } else {
                    let _ = std::fs::remove_file(self.thumbs.join(name));
                }
            }
            self.flush(&mut c);
            self.dirty = true;
            self.publish(false);
        }
    }

    fn unref(&mut self, hash: &str) {
        if let Some(n) = self.refs.get_mut(hash) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                self.refs.remove(hash);
            }
        }
    }

    /// Drops a model's facts once no file holds its content (tags stay).
    fn release_hash(&mut self, hash: &str, c: &mut Changes) {
        if !self.refs.contains_key(hash)
            && let Some(info) = self.index.models.remove(hash)
        {
            c.models.push((hash.to_string(), None));
            if let Some(ext) = info.thumbnail {
                let _ = std::fs::remove_file(self.thumbs.join(format!("{hash}.{ext}")));
            }
        }
    }

    fn watch(&mut self, root: &Path) {
        if !self.options.watch || self.watchers.contains_key(root) {
            return;
        }
        use notify::Watcher;
        let tx = self.tx.clone();
        let r = root.to_path_buf();
        let handler = move |res: notify::Result<notify::Event>| match res {
            Ok(ev) if ev.need_rescan() => {
                let _ = tx.send(Cmd::FsRescan(r.clone()));
            }
            Ok(ev) => {
                if !ev.kind.is_access() && !ev.paths.is_empty() {
                    let _ = tx.send(Cmd::Fs(ev.paths));
                }
            }
            Err(_) => {}
        };
        if let Ok(mut w) = notify::recommended_watcher(handler)
            && w.watch(root, notify::RecursiveMode::Recursive).is_ok()
        {
            self.watchers.insert(root.to_path_buf(), w);
            if let Some(s) = self.status.get_mut(root) {
                s.watching = true;
            }
        }
    }

    fn unwatch(&mut self, root: &Path) {
        self.watchers.remove(root);
        if let Some(s) = self.status.get_mut(root) {
            s.watching = false;
        }
    }

    /// Full scan of one library folder.
    fn scan_root(&mut self, root: &Path) {
        if !self.index.folders.iter().any(|f| f == root) {
            return;
        }
        self.cancel = None;
        let started = Instant::now();
        if !available(root) {
            self.unwatch(root);
            self.set_state(root, FolderState::Offline);
            return;
        }
        self.set_state(root, FolderState::Scanning { done: 0, total: 0 });
        self.publish(true);
        let data_dir = self.data_dir.clone();
        let stop = || false;
        let walked = match walk(root, Some(&data_dir), &stop) {
            Ok(w) => w,
            Err(_) => {
                self.unwatch(root);
                self.set_state(root, FolderState::Offline);
                return;
            }
        };
        let known = self.index.files.values().filter(|r| r.root == root).count();
        if walked.files.is_empty() && known > 0 && on_removable_volume(root) {
            // An empty share is far more likely unmounted than emptied: keep everything.
            self.set_state(root, FolderState::Offline);
            return;
        }
        if !available(root) {
            self.set_state(root, FolderState::Offline);
            return;
        }
        self.canonical.insert(
            root.to_path_buf(),
            std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf()),
        );
        self.watch(root);
        // Files that are gone (but not those in sub-folders that could not be read).
        let seen: HashSet<&Path> = walked.files.iter().map(|f| f.path.as_path()).collect();
        let gone: Vec<PathBuf> = self
            .index
            .files
            .iter()
            .filter(|(p, r)| {
                r.root == root
                    && !seen.contains(p.as_path())
                    && !walked.unreadable.iter().any(|u| p.starts_with(u))
            })
            .map(|(p, _)| p.clone())
            .collect();
        let mut c = Changes::default();
        for p in gone {
            self.remove_file(&p, &mut c);
        }
        self.flush(&mut c);
        let total = walked.files.len();
        let stopped = self.process(root, walked.files, total);
        if !stopped {
            self.sync_extras(root, walked.extras, Some(&walked.unreadable));
        }
        if stopped || self.cancel.as_deref() == Some(root) {
            if let Some(s) = self.status.get_mut(root) {
                s.state = FolderState::Waiting;
            }
            self.dirty = true;
            return;
        }
        let state = if available(root) {
            FolderState::Ready
        } else {
            self.unwatch(root);
            FolderState::Offline
        };
        if let Some(s) = self.status.get_mut(root) {
            s.state = state;
            s.last_scan_s = Some(started.elapsed().as_secs_f64());
        }
        self.dirty = true;
    }

    /// Indexes `found` (files under `root`); `total` is the progress denominator. `true` when
    /// the scan was stopped.
    fn process(&mut self, root: &Path, found: Vec<Found>, total: usize) -> bool {
        let mut done = 0;
        let mut changed = vec![];
        let mut c = Changes::default();
        for f in found {
            let same = self.index.files.get(&f.path).is_some_and(|r| {
                r.size == f.size
                    && r.mtime_ns == f.mtime_ns
                    && r.root == root
                    && self.index.models.contains_key(&r.hash)
            });
            if same {
                done += 1;
            } else {
                changed.push(f);
            }
        }
        self.set_state(root, FolderState::Scanning { done, total });
        for batch in changed.chunks(BATCH) {
            if self.checkpoint(root) {
                self.flush(&mut c);
                return true;
            }
            let models = &self.index.models;
            let results: Vec<_> = self.pool.install(|| {
                use rayon::prelude::*;
                batch
                    .par_iter()
                    .map(|f| (f.path.clone(), read_one(f, models)))
                    .collect()
            });
            for (path, r) in results {
                match r {
                    Ok((found, hash, extracted)) => {
                        self.apply(root, found, hash, extracted, &mut c)
                    }
                    // Vanished meanwhile, or unreadable: forget it (a later event or scan
                    // brings it back).
                    Err(_) => self.remove_file(&path, &mut c),
                }
            }
            done += batch.len();
            self.flush(&mut c);
            self.set_state(root, FolderState::Scanning { done, total });
            if !available(root) {
                return false;
            }
            self.publish(false);
        }
        self.flush(&mut c);
        false
    }

    fn apply(
        &mut self,
        root: &Path,
        found: Found,
        hash: String,
        extracted: Option<Extracted>,
        c: &mut Changes,
    ) {
        // Known content when it was read, but its last other copy went in this batch.
        let extracted = extracted.or_else(|| {
            (!self.index.models.contains_key(&hash)).then(|| {
                let data = (found.size <= IN_MEMORY)
                    .then(|| std::fs::read(&found.path).ok())
                    .flatten();
                extract(found.kind, &found.path, data.as_deref())
            })
        });
        if let Some(mut e) = extracted
            && !self.index.models.contains_key(&hash)
        {
            if let Some((bytes, ext)) = e.thumbnail.take() {
                let dest = self.thumbs.join(format!("{hash}.{ext}"));
                let tmp = self.thumbs.join(format!(".{hash}.{ext}.tmp"));
                if std::fs::write(&tmp, &bytes).is_ok() && std::fs::rename(&tmp, &dest).is_ok() {
                    e.info.thumbnail = Some(ext.to_string());
                }
            }
            c.models.push((hash.clone(), Some(e.info.clone())));
            self.index.models.insert(hash.clone(), e.info);
        }
        let rec = FileRecord {
            root: root.to_path_buf(),
            size: found.size,
            mtime_ns: found.mtime_ns,
            hash: hash.clone(),
            kind: found.kind,
        };
        let old = self.index.files.insert(found.path.clone(), rec.clone());
        c.files.push((found.path, Some(rec)));
        *self.refs.entry(hash.clone()).or_default() += 1;
        if let Some(old) = &old {
            self.unref(&old.hash);
        }
        if let Some(old) = old
            && old.hash != hash
        {
            // Edited in place: tags follow the file when no other copy keeps the old content.
            if !self.refs.contains_key(&old.hash) {
                let mut g = self.lock();
                if let Some(u) = g.user.remove(&old.hash)
                    && !g.user.contains_key(&hash)
                {
                    g.user.insert(hash.clone(), u.clone());
                    drop(g);
                    c.user.push((old.hash.clone(), None));
                    c.user.push((hash.clone(), Some(u)));
                }
            }
            self.release_hash(&old.hash, c);
        }
        self.dirty = true;
    }

    /// Maps an event path (canonical) back to a library folder and the path below it.
    fn map_event(&self, p: &Path) -> Option<(PathBuf, PathBuf)> {
        for root in &self.index.folders {
            if p.starts_with(root) {
                return Some((root.clone(), p.to_path_buf()));
            }
            if let Some(c) = self.canonical.get(root)
                && let Ok(rel) = p.strip_prefix(c)
            {
                return Some((root.clone(), root.join(rel)));
            }
        }
        None
    }

    fn process_events(&mut self) {
        let events = std::mem::take(&mut self.events);
        let mut by_root: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
        for p in events {
            if let Some((root, p)) = self.map_event(&p)
                && !skipped_path(&root, &p)
                && !p.starts_with(&self.data_dir)
            {
                by_root.entry(root).or_default().push(p);
            }
        }
        for (root, paths) in by_root {
            if self.queue.contains(&root) || !available(&root) {
                continue;
            }
            let mut c = Changes::default();
            let mut files: Vec<Found> = vec![];
            let mut extras: Vec<FoundExtra> = vec![];
            for p in paths {
                match std::fs::symlink_metadata(&p) {
                    Ok(m) if m.is_file() => {
                        if let Some(kind) = Kind::of(&p) {
                            files.push(Found {
                                size: m.len(),
                                mtime_ns: mtime_ns(&m),
                                path: p,
                                kind,
                            });
                        } else if let Some(kind) = ExtraKind::of(&p) {
                            extras.push(FoundExtra {
                                size: m.len(),
                                mtime_ns: mtime_ns(&m),
                                path: p,
                                kind,
                            });
                        }
                    }
                    Ok(m) if m.is_dir() => {
                        // A folder appeared or changed: walk it, and drop what left it.
                        if let Ok(w) = walk(&p, Some(&self.data_dir.clone()), &|| false) {
                            let seen: HashSet<PathBuf> =
                                w.files.iter().map(|f| f.path.clone()).collect();
                            let gone: Vec<PathBuf> = self
                                .index
                                .files
                                .range(p.clone()..)
                                .take_while(|(k, _)| k.starts_with(&p))
                                .filter(|(k, _)| !seen.contains(*k))
                                .map(|(k, _)| k.clone())
                                .collect();
                            for g in gone {
                                self.remove_file(&g, &mut c);
                            }
                            let seen: HashSet<PathBuf> =
                                w.extras.iter().map(|f| f.path.clone()).collect();
                            let gone: Vec<PathBuf> = self
                                .index
                                .extras
                                .range(p.clone()..)
                                .take_while(|(k, _)| k.starts_with(&p))
                                .filter(|(k, _)| !seen.contains(*k))
                                .map(|(k, _)| k.clone())
                                .collect();
                            for g in gone {
                                self.remove_extra(&g, &mut c);
                            }
                            files.extend(w.files);
                            extras.extend(w.extras);
                        }
                    }
                    Ok(_) => {}
                    Err(_) => {
                        // Deleted (a file, or a whole folder).
                        let gone: Vec<PathBuf> = self
                            .index
                            .files
                            .range(p.clone()..)
                            .take_while(|(k, _)| k.starts_with(&p))
                            .map(|(k, _)| k.clone())
                            .collect();
                        for g in gone {
                            self.remove_file(&g, &mut c);
                        }
                        let gone: Vec<PathBuf> = self
                            .index
                            .extras
                            .range(p.clone()..)
                            .take_while(|(k, _)| k.starts_with(&p))
                            .map(|(k, _)| k.clone())
                            .collect();
                        for g in gone {
                            self.remove_extra(&g, &mut c);
                        }
                    }
                }
            }
            self.flush(&mut c);
            files.sort_by(|a, b| a.path.cmp(&b.path));
            files.dedup_by(|a, b| a.path == b.path);
            let n = files.len();
            let before = self.status.get(&root).map(|s| s.state.clone());
            self.process(&root, files, n);
            extras.sort_by(|a, b| a.path.cmp(&b.path));
            extras.dedup_by(|a, b| a.path == b.path);
            self.sync_extras(&root, extras, None);
            if let Some(s) = self.status.get_mut(&root) {
                s.state = match before {
                    Some(FolderState::Scanning { .. }) | None => FolderState::Ready,
                    Some(other) => other,
                };
            }
            self.dirty = true;
        }
    }

    /// Folders that went missing are marked offline; offline ones that came back are scanned.
    fn check_availability(&mut self) {
        self.last_offline_check = Instant::now();
        for root in self.index.folders.clone() {
            let offline = matches!(
                self.status.get(&root).map(|s| &s.state),
                Some(FolderState::Offline)
            );
            let up = available(&root);
            if offline && up {
                if !self.queue.contains(&root) {
                    self.queue.push_back(root);
                }
            } else if !offline && !up {
                self.unwatch(&root);
                self.set_state(&root, FolderState::Offline);
            }
        }
    }

    /// Removes cached thumbnails no model refers to (only in the app's own folder).
    fn collect_garbage(&mut self) {
        let Ok(dir) = std::fs::read_dir(&self.thumbs) else {
            return;
        };
        let pictures: HashSet<&str> = self
            .index
            .extras
            .values()
            .filter_map(|r| r.thumbnail.as_deref())
            .collect();
        for e in dir.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let stem = name.split('.').next().unwrap_or("");
            let keep = !name.starts_with('.')
                && (pictures.contains(name.as_str())
                    || self.index.models.get(stem).is_some_and(|i| {
                        i.thumbnail.as_deref().is_some_and(|x| name.ends_with(x))
                    }));
            if !keep {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }

    fn build(&self) -> Snapshot {
        let mut by_hash: HashMap<&str, Vec<(&PathBuf, &FileRecord)>> = HashMap::new();
        for (p, r) in &self.index.files {
            by_hash.entry(r.hash.as_str()).or_default().push((p, r));
        }
        let offline: HashSet<&Path> = self
            .status
            .iter()
            .filter(|(_, s)| s.state == FolderState::Offline)
            .map(|(p, _)| p.as_path())
            .collect();
        let mut models: Vec<Model> = by_hash
            .into_iter()
            .filter_map(|(hash, files)| {
                let info = self.index.models.get(hash)?.clone();
                let mut locations: Vec<Location> = files
                    .iter()
                    .map(|(p, r)| Location {
                        path: (*p).clone(),
                        root: r.root.clone(),
                        size: r.size,
                        modified: (r.mtime_ns / 1_000_000_000) as i64,
                        online: !offline.contains(r.root.as_path()),
                    })
                    .collect();
                locations.sort_by(|a, b| b.online.cmp(&a.online).then(a.path.cmp(&b.path)));
                let first = &locations[0];
                Some(Model {
                    id: hash.to_string(),
                    name: first
                        .path
                        .file_name()
                        .map_or_else(String::new, |n| n.to_string_lossy().into_owned()),
                    kind: files[0].1.kind,
                    thumbnail: info
                        .thumbnail
                        .as_ref()
                        .map(|ext| self.thumbs.join(format!("{hash}.{ext}"))),
                    info,
                    locations,
                    user: Default::default(),
                })
            })
            .collect();
        models.sort_by(|a, b| {
            a.name
                .to_lowercase()
                .cmp(&b.name.to_lowercase())
                .then(a.id.cmp(&b.id))
        });
        let mut folders: Vec<FolderStatus> = self
            .index
            .folders
            .iter()
            .map(|f| {
                let mut s = self
                    .status
                    .get(f)
                    .cloned()
                    .unwrap_or_else(|| status(f, FolderState::Waiting));
                s.files = self.index.files.values().filter(|r| &r.root == f).count();
                s
            })
            .collect();
        folders.sort_by(|a, b| a.path.cmp(&b.path));
        let extras = self
            .index
            .extras
            .iter()
            .map(|(p, r)| Extra {
                path: p.clone(),
                root: r.root.clone(),
                kind: r.kind,
                size: r.size,
                modified: (r.mtime_ns / 1_000_000_000) as i64,
                thumbnail: r.thumbnail.as_ref().map(|t| self.thumbs.join(t)),
                online: !offline.contains(r.root.as_path()),
            })
            .collect();
        Snapshot {
            files: self.index.files.len(),
            pending_thumbnails: self.renders.len(),
            models,
            extras,
            grouping: Default::default(),
            folders,
        }
    }

    /// Publishes a new snapshot (throttled unless `now`).
    fn publish(&mut self, now: bool) {
        if !now && self.last_publish.elapsed() < PUBLISH_EVERY {
            return;
        }
        let mut snap = self.build();
        let mut g = self.lock();
        for m in &mut snap.models {
            if let Some(u) = g.user.get(&m.id) {
                m.user = u.clone();
            }
        }
        snap.grouping = g.grouping.clone();
        g.snapshot = Arc::new(snap);
        g.generation += 1;
        drop(g);
        self.last_publish = Instant::now();
        self.dirty = false;
    }
}
