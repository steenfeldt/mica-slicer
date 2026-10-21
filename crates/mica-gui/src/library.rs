// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The Library view's state: the open library, the search and filters, the selection and the
//! thumbnail textures. The drawing is in `panels/library.rs`.

use eframe::egui;
use mica_library::group::{self, FileRef, GroupBy, Item, Project, Projects};
use mica_library::{Filter, Grouping, Kind, Library, Model, Options, Snapshot, Sort, query};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Thumbnails decoded per frame (the rest follow on the next frames).
const TEXTURES_PER_FRAME: usize = 10;

/// Size presets for the "Size" filter: label and the largest side's bounds in mm.
pub const SIZES: [(&str, Option<f32>, Option<f32>); 5] = [
    ("Any size", None, None),
    ("Up to 50 mm", None, Some(50.)),
    ("50 – 150 mm", Some(50.), Some(150.)),
    ("150 – 250 mm", Some(150.), Some(250.)),
    ("Over 250 mm", Some(250.), None),
];
/// Date presets: label and days.
pub const DATES: [(&str, Option<u32>); 4] = [
    ("Any time", None),
    ("Last 7 days", Some(7)),
    ("Last 30 days", Some(30)),
    ("Last year", Some(365)),
];

/// One cell of the grid or row of the list, in display order.
#[derive(Clone, Debug, PartialEq)]
pub enum Entry {
    /// A section header (Group by Folder, Printer, Type, Tag; the parts of a project).
    Header {
        title: String,
        count: usize,
        /// The folder a folder section shows.
        folder: Option<PathBuf>,
    },
    /// A project card (index into [`Projects::list`]).
    Project(usize),
    /// A model card (index into the snapshot's models).
    Model(usize),
    /// A file of the open project.
    File(FileRef),
    /// A picture or document of the open project (index into the snapshot's extras).
    Extra(usize),
}

/// What a double-click on a model asks.
#[derive(Clone, Debug, PartialEq)]
pub struct OpenPrompt {
    pub path: PathBuf,
    pub name: String,
}

type ViewKey = (u64, Filter, Sort, bool, GroupBy, Option<PathBuf>);

pub struct LibraryView {
    /// The Library tab is showing.
    pub shown: bool,
    pub lib: Option<Library>,
    /// Why the library could not be opened.
    pub error: Option<String>,
    pub filter: Filter,
    pub group_by: GroupBy,
    /// The project shown in the grid instead of the whole library (its folder).
    pub open_project: Option<PathBuf>,
    pub sort: Sort,
    pub descending: bool,
    /// List view instead of the thumbnail grid.
    pub list: bool,
    /// The selected model's id (its content hash).
    pub selected: Option<String>,
    pub tag_input: String,
    pub folder_input: String,
    /// A transient message under the folder list (an add that was refused, …).
    pub notice: Option<String>,
    pub open_prompt: Option<OpenPrompt>,
    open_at: Option<(PathBuf, Options)>,
    opening: bool,
    snapshot: Arc<Snapshot>,
    generation: u64,
    hay: Vec<String>,
    projects: Arc<Projects>,
    view: group::View,
    entries: Vec<Entry>,
    visible: Vec<usize>,
    visible_for: Option<ViewKey>,
    /// Milliseconds the last project detection took.
    pub grouping_ms: f64,
    pub printers: Vec<String>,
    pub tags: Vec<String>,
    textures: HashMap<String, Option<egui::TextureHandle>>,
    /// Further models picked with ⌘ or Shift-click (dragged together with the selected one).
    pub picked: std::collections::BTreeSet<String>,
    /// The details panel's 3D preview of the selected model.
    pub preview: Option<crate::library_preview::Preview>,
    /// Models dropped on the plate, loading.
    pub drop: Option<crate::library_drop::PendingDrop>,
    /// The part-kind choice after a drop onto an object.
    pub popover: Option<crate::library_drop::PartPopover>,
    /// Build volumes (X, Y, Z mm) of printer profiles, by `Vendor/Preset`.
    pub volumes: HashMap<String, Option<[f32; 3]>>,
    /// What a drop would do where the pointer is (set by the drop targets each frame).
    pub drag_hint: Option<String>,
    /// Headless screenshots: a drag or a drop to stage.
    pub shot: Option<crate::library_drop::Shot>,
    loads_this_frame: usize,
}

impl Default for LibraryView {
    fn default() -> Self {
        Self {
            shown: false,
            lib: None,
            error: None,
            filter: Filter::default(),
            group_by: GroupBy::Project,
            open_project: None,
            sort: Sort::Name,
            descending: false,
            list: false,
            selected: None,
            tag_input: String::new(),
            folder_input: String::new(),
            notice: None,
            open_prompt: None,
            open_at: None,
            opening: false,
            snapshot: Arc::new(Snapshot::default()),
            generation: u64::MAX,
            hay: vec![],
            projects: Arc::new(Projects::default()),
            view: group::View::default(),
            entries: vec![],
            visible: vec![],
            visible_for: None,
            grouping_ms: 0.,
            printers: vec![],
            tags: vec![],
            textures: HashMap::new(),
            picked: Default::default(),
            preview: None,
            drop: None,
            popover: None,
            volumes: HashMap::new(),
            drag_hint: None,
            shot: None,
            loads_this_frame: 0,
        }
    }
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

impl LibraryView {
    /// The real app: the library in the app's data folder (nothing is scanned until the
    /// user adds a folder). Headless runs pass `None` and open one explicitly.
    pub fn new(data_dir: Option<PathBuf>) -> Self {
        let mut v = Self::default();
        if let Some(d) = data_dir {
            v.open(&d, Options::default());
        }
        v
    }

    pub fn open(&mut self, data_dir: &Path, options: Options) {
        // Release our previous handle before opening the same file again.
        self.lib = None;
        self.open_at = Some((data_dir.to_path_buf(), options.clone()));
        self.snapshot = Arc::new(Snapshot::default());
        self.hay.clear();
        self.projects = Arc::new(Projects::default());
        self.view = group::View::default();
        self.entries.clear();
        self.open_project = None;
        self.visible.clear();
        self.printers.clear();
        self.tags.clear();
        self.notice = None;
        self.selected = None;
        self.picked.clear();
        self.preview = None;
        self.open_prompt = None;
        self.opening = false;
        match Library::open(data_dir, options) {
            Ok(l) => {
                self.lib = Some(l);
                self.error = None;
                self.opening = true;
            }
            Err(e) => self.error = Some(e),
        }
        self.generation = u64::MAX;
        self.visible_for = None;
        self.textures.clear();
    }

    pub fn retry(&mut self) {
        if let Some((dir, options)) = self.open_at.clone() {
            self.open(&dir, options);
        }
    }

    pub fn loading(&self) -> bool {
        self.opening
    }

    /// Picks up a new snapshot and re-runs the search when anything changed. Cheap when
    /// nothing did.
    pub fn refresh(&mut self) {
        self.loads_this_frame = 0;
        let Some(lib) = &self.lib else { return };
        // Read completion before the error: once loaded, the worker has published either
        // the index or its error. Never show an empty library between those two reads.
        self.opening = !lib.loaded();
        self.error = lib.error();
        let generation = lib.generation();
        if generation != self.generation {
            self.generation = generation;
            let snap = lib.snapshot();
            // Thumbnails of models that changed content (or were rendered again) are dropped.
            if !Arc::ptr_eq(&snap, &self.snapshot) {
                self.hay = query::haystacks(&snap.models);
                let t = std::time::Instant::now();
                self.projects = Arc::new(group::projects(&snap));
                self.grouping_ms = t.elapsed().as_secs_f64() * 1e3;
                self.printers = query::printers(&snap.models);
                self.tags = lib.all_tags();
                let keys: std::collections::HashSet<String> =
                    snap.models.iter().map(texture_key).collect();
                let pics: std::collections::HashSet<String> = snap
                    .extras
                    .iter()
                    .filter_map(|x| x.thumbnail.as_ref())
                    .map(|t| format!("pic:{}", t.display()))
                    .collect();
                self.textures
                    .retain(|k, _| keys.contains(k) || pics.contains(k));
                self.picked
                    .retain(|id| snap.models.iter().any(|m| &m.id == id));
            }
            self.snapshot = snap;
        }
        // A project that is gone (split, deleted, its folder removed) closes.
        if let Some(f) = &self.open_project
            && self.projects.by_folder(f).is_none()
        {
            self.open_project = None;
        }
        let key = (
            self.generation,
            self.filter.clone(),
            self.sort,
            self.descending,
            self.group_by,
            self.open_project.clone(),
        );
        if self.visible_for.as_ref() != Some(&key) {
            self.view = group::view(
                &self.snapshot.models,
                &self.hay,
                &self.projects,
                &self.filter,
                self.group_by,
                self.sort,
                self.descending,
                now(),
            );
            self.visible = self.view.model_order(&self.projects);
            self.entries = match self.open_project_index() {
                Some(p) => self.project_entries(p),
                None => self.top_entries(),
            };
            self.visible_for = Some(key);
        }
    }

    fn top_entries(&self) -> Vec<Entry> {
        let mut out = vec![];
        for s in &self.view.sections {
            if !s.title.is_empty() {
                out.push(Entry::Header {
                    title: s.title.clone(),
                    count: s.items.len(),
                    folder: s.folder.clone(),
                });
            }
            out.extend(s.items.iter().map(|it| match *it {
                Item::Project(p) => Entry::Project(p),
                Item::Model(m) => Entry::Model(m),
            }));
        }
        out
    }

    /// The open project's files: its 3MF projects, the parts, G-code, then pictures and
    /// documents, each sorted like the library.
    fn project_entries(&self, pi: usize) -> Vec<Entry> {
        let p = &self.projects.list[pi];
        let models = &self.snapshot.models;
        let mut out = vec![];
        type Pick = fn(Kind) -> bool;
        let kinds: [(&str, Pick); 3] = [
            ("3MF projects", |k| k == Kind::ThreeMf),
            ("Parts", |k| matches!(k, Kind::Stl | Kind::Obj | Kind::Step)),
            ("G-code", Kind::is_gcode),
        ];
        for (title, pred) in kinds {
            let mut files: Vec<FileRef> = p
                .files
                .iter()
                .copied()
                .filter(|f| pred(models[f.model].kind))
                .collect();
            if files.is_empty() {
                continue;
            }
            group::sort_files(&mut files, models, self.sort, self.descending);
            out.push(Entry::Header {
                title: title.into(),
                count: files.len(),
                folder: None,
            });
            out.extend(files.into_iter().map(Entry::File));
        }
        if !p.extras.is_empty() {
            out.push(Entry::Header {
                title: "Pictures and documents".into(),
                count: p.extras.len(),
                folder: None,
            });
            out.extend(p.extras.iter().map(|&x| Entry::Extra(x)));
        }
        out
    }

    /// The cells of the grid (or rows of the list), in order.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn projects(&self) -> &Projects {
        &self.projects
    }

    /// Projects and models shown at the top level.
    pub fn shown_counts(&self) -> (usize, usize) {
        (self.view.projects, self.view.models)
    }

    fn open_project_index(&self) -> Option<usize> {
        self.open_project
            .as_ref()
            .and_then(|f| self.projects.by_folder(f))
    }

    pub fn current_project(&self) -> Option<&Project> {
        self.open_project_index().map(|i| &self.projects.list[i])
    }

    /// The search or a filter is narrowing what is shown (matching files are then marked).
    pub fn searching(&self) -> bool {
        !self.filter.text.trim().is_empty() || self.filter.narrowed()
    }

    /// A model matches the search and filters.
    pub fn matched(&self, model: usize) -> bool {
        self.view.matched.get(model).copied().unwrap_or(false)
    }

    /// Overrides the project detection for a folder.
    pub fn set_grouping(&mut self, folder: &Path, grouping: Option<Grouping>) {
        if let Some(l) = &self.lib {
            l.set_grouping(folder, grouping);
        }
    }

    /// The user's choice for `folder`.
    pub fn grouping_of(&self, folder: &Path) -> Option<Grouping> {
        self.snapshot.grouping.get(folder).copied()
    }

    pub fn snapshot(&self) -> &Arc<Snapshot> {
        &self.snapshot
    }

    /// The models shown, in order.
    pub fn visible(&self) -> impl ExactSizeIterator<Item = &Model> + '_ {
        self.visible.iter().map(|&i| &self.snapshot.models[i])
    }

    pub fn visible_len(&self) -> usize {
        self.visible.len()
    }

    pub fn visible_at(&self, i: usize) -> Option<&Model> {
        self.visible.get(i).map(|&i| &self.snapshot.models[i])
    }

    pub fn model(&self, id: &str) -> Option<&Model> {
        self.snapshot.models.iter().find(|m| m.id == id)
    }

    pub fn selected_model(&self) -> Option<&Model> {
        self.selected.as_deref().and_then(|id| self.model(id))
    }

    /// The thumbnail texture of a model, decoded on first use (a few per frame). A thumbnail
    /// rendered again by a newer renderer is decoded again.
    pub fn texture(&mut self, ctx: &egui::Context, m: &Model) -> Option<egui::TextureHandle> {
        let key = texture_key(m);
        self.texture_at(ctx, &key, m.thumbnail.as_deref())
    }

    /// A cached picture's thumbnail texture.
    pub fn picture_texture(
        &mut self,
        ctx: &egui::Context,
        x: &mica_library::Extra,
    ) -> Option<egui::TextureHandle> {
        let key = format!("pic:{}", x.thumbnail.as_ref()?.display());
        self.texture_at(ctx, &key, x.thumbnail.as_deref())
    }

    /// A project's cover texture.
    pub fn cover_texture(
        &mut self,
        ctx: &egui::Context,
        p: &Project,
    ) -> Option<egui::TextureHandle> {
        let snap = self.snapshot.clone();
        match p.cover? {
            group::Cover::Model(m) => self.texture(ctx, snap.models.get(m)?),
            group::Cover::Extra(x) => self.picture_texture(ctx, snap.extras.get(x)?),
        }
    }

    fn texture_at(
        &mut self,
        ctx: &egui::Context,
        key: &str,
        path: Option<&Path>,
    ) -> Option<egui::TextureHandle> {
        if let Some(t) = self.textures.get(key) {
            return t.clone();
        }
        let path = path?;
        if self.loads_this_frame >= TEXTURES_PER_FRAME {
            ctx.request_repaint();
            return None;
        }
        self.loads_this_frame += 1;
        let tex = std::fs::read(path)
            .ok()
            .and_then(|b| image::load_from_memory(&b).ok())
            .map(|img| {
                let rgba = img.to_rgba8();
                let size = [rgba.width() as usize, rgba.height() as usize];
                let ci = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
                ctx.load_texture(format!("lib-{key}"), ci, egui::TextureOptions::LINEAR)
            });
        self.textures.insert(key.to_string(), tex.clone());
        tex
    }

    /// Keeps the details panel's 3D preview on the selected model: a new selection drops the
    /// old preview (its texture and mesh) and starts reading the new one.
    pub fn sync_preview(&mut self, ctx: &egui::Context) {
        let want = self
            .selected_model()
            .filter(|m| self.shown && m.has_mesh() && m.online())
            .map(|m| (m.id.clone(), m.kind, m.primary().path.clone()));
        match want {
            None => self.preview = None,
            Some((id, kind, path)) => {
                if self.preview.as_ref().is_none_or(|p| p.id != id) {
                    self.preview = None;
                    self.preview = Some(crate::library_preview::Preview::new(&id, kind, path, ctx));
                }
                if let Some(p) = &mut self.preview {
                    p.poll(ctx);
                }
            }
        }
    }

    /// What a dragged project card carries: its STL parts (as "Add all parts to plate"), or,
    /// without any, its main 3MF (whose first plate is added).
    pub fn project_drag_items(&self, pr: &Project) -> Vec<crate::library_drop::DragItem> {
        let models = &self.snapshot.models;
        let mut files = pr.plate_parts(models);
        if files.is_empty() {
            files.extend(pr.main_3mf(models));
        }
        files
            .into_iter()
            .map(|f| {
                let m = &models[f.model];
                let l = &m.locations[f.location];
                crate::library_drop::DragItem {
                    id: m.id.clone(),
                    name: l
                        .path
                        .file_name()
                        .map_or_else(|| m.name.clone(), |n| n.to_string_lossy().into_owned()),
                    path: l.path.clone(),
                    kind: m.kind,
                    online: l.online,
                }
            })
            .collect()
    }

    /// Models a drag from `m` carries: the picked ones (in display order) when `m` is one of
    /// them, else `m` alone.
    pub fn drag_items(&self, m: &Model) -> Vec<crate::library_drop::DragItem> {
        let item = |m: &Model| crate::library_drop::DragItem {
            id: m.id.clone(),
            name: m.name.clone(),
            path: m.primary().path.clone(),
            kind: m.kind,
            online: m.online(),
        };
        let group = self.picked.contains(&m.id) || self.selected.as_deref() == Some(&m.id);
        if !group || self.picked.is_empty() {
            return vec![item(m)];
        }
        // In display order where shown, then any picked inside closed projects.
        let wanted =
            |v: &&Model| self.picked.contains(&v.id) || self.selected.as_deref() == Some(&v.id);
        let mut out: Vec<_> = self.visible().filter(wanted).map(item).collect();
        for v in self.snapshot.models.iter().filter(wanted) {
            if !out.iter().any(|i| i.id == v.id) {
                out.push(item(v));
            }
        }
        out
    }

    /// Adds a typed or picked folder; refusals land in `notice`.
    pub fn add_folder(&mut self, path: &Path) {
        let Some(lib) = &self.lib else { return };
        match lib.add_folder(path) {
            Ok(()) => self.notice = None,
            Err(e) => self.notice = Some(e),
        }
    }

    pub fn set_paused(&self, paused: bool) {
        if let Some(l) = &self.lib {
            l.set_paused(paused);
        }
    }

    pub fn toggle_favourite(&mut self, id: &str) {
        let fav = self.model(id).is_some_and(|m| m.user.favourite);
        if let Some(l) = &self.lib {
            l.set_favourite(id, !fav);
        }
    }

    pub fn add_tag(&mut self, id: &str, tag: &str) {
        let Some(m) = self.model(id) else { return };
        let mut tags = m.user.tags.clone();
        tags.push(tag.to_string());
        if let Some(l) = &self.lib {
            l.set_tags(id, &tags);
        }
    }

    pub fn remove_tag(&mut self, id: &str, tag: &str) {
        let Some(m) = self.model(id) else { return };
        let tags: Vec<String> = m.user.tags.iter().filter(|t| *t != tag).cloned().collect();
        if let Some(l) = &self.lib {
            l.set_tags(id, &tags);
        }
        if self
            .filter
            .tag
            .as_deref()
            .is_some_and(|t| t.eq_ignore_ascii_case(tag))
            && !self.tags.iter().any(|t| t.eq_ignore_ascii_case(tag))
        {
            self.filter.tag = None;
        }
    }

    /// The kind filter chips: G-code covers text and binary.
    pub fn kind_on(&self, k: Kind) -> bool {
        self.filter.kinds.contains(&k)
    }

    pub fn toggle_kind(&mut self, k: Kind) {
        let kinds: &[Kind] = if k.is_gcode() {
            &[Kind::Gcode, Kind::Bgcode]
        } else {
            std::slice::from_ref(&k)
        };
        let on = self.filter.kinds.contains(&k);
        for k in kinds {
            if on {
                self.filter.kinds.remove(k);
            } else {
                self.filter.kinds.insert(*k);
            }
        }
    }
}

/// A thumbnail texture's cache key: the content, and the renderer that drew its picture.
fn texture_key(m: &Model) -> String {
    format!("{}:{}", m.id, m.info.render_version)
}

// ----- formatting ------------------------------------------------------------------------

/// `1 h 12 min`, `25 min`, `40 s`.
pub fn duration(s: f64) -> String {
    let s = s.round() as i64;
    if s >= 86_400 {
        format!("{} d {} h", s / 86_400, s % 86_400 / 3600)
    } else if s >= 3600 {
        format!("{} h {} min", s / 3600, s % 3600 / 60)
    } else if s >= 60 {
        format!("{} min", s / 60)
    } else {
        format!("{s} s")
    }
}

pub fn bytes(n: u64) -> String {
    let n = n as f64;
    if n >= 1e9 {
        format!("{:.1} GB", n / 1e9)
    } else if n >= 1e6 {
        format!("{:.1} MB", n / 1e6)
    } else if n >= 1e3 {
        format!("{:.0} KB", n / 1e3)
    } else {
        format!("{n:.0} B")
    }
}

pub fn dims(s: [f32; 3]) -> String {
    let f = |v: f32| {
        if v >= 100. {
            format!("{v:.0}")
        } else {
            format!("{v:.1}")
        }
    };
    format!("{} × {} × {} mm", f(s[0]), f(s[1]), f(s[2]))
}

/// The civil date of a Unix time (UTC), as `2026-10-17`.
pub fn date(t: i64) -> String {
    // Howard Hinnant's days-to-civil algorithm.
    let z = t.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

/// `today`, `yesterday`, `5 days ago`, else the date.
pub fn when(t: i64, now: i64) -> String {
    let days = (now - t).div_euclid(86_400);
    match days {
        i64::MIN..=0 => "today".into(),
        1 => "yesterday".into(),
        2..=13 => format!("{days} days ago"),
        _ => date(t),
    }
}

/// One line under a project card's name: `3MF + 10 parts · 1 G-code`.
pub fn project_summary(p: &Project) -> String {
    let mut parts = vec![];
    let plural = |n: usize, one: &str, many: &str| {
        if n == 1 {
            format!("1 {one}")
        } else {
            format!("{n} {many}")
        }
    };
    match (p.projects, p.parts) {
        (0, 0) => {}
        (0, n) => parts.push(plural(n, "part", "parts")),
        (1, 0) => parts.push("3MF".into()),
        (k, 0) => parts.push(format!("{k} 3MF")),
        (1, n) => parts.push(format!("3MF + {}", plural(n, "part", "parts"))),
        (k, n) => parts.push(format!("{k} 3MF + {}", plural(n, "part", "parts"))),
    }
    if p.gcodes > 0 {
        parts.push(plural(p.gcodes, "G-code", "G-codes"));
    }
    parts.join(" · ")
}

/// A printer's short name for a badge: `Prusa CORE One HF0.4 nozzle` → `CORE One`,
/// `Original Prusa XL - 5T Input Shaper 0.4 nozzle` → `XL`.
pub fn short_printer(name: &str) -> String {
    let mut s = name.split(" - ").next().unwrap_or(name).trim();
    for prefix in ["Original Prusa ", "Prusa ", "Snapmaker ", "Bambu Lab "] {
        if let Some(rest) = s.strip_prefix(prefix) {
            s = rest;
            break;
        }
    }
    let s = s
        .split_whitespace()
        .take_while(|w| {
            let l = w.to_ascii_lowercase();
            !(l.starts_with('(')
                || l.starts_with("hf0")
                || l.starts_with("0.")
                || l == "nozzle"
                || l == "input")
        })
        .collect::<Vec<_>>()
        .join(" ");
    if s.is_empty() { name.to_string() } else { s }
}

/// One line under a card's name: type and the most useful facts.
pub fn summary(m: &Model) -> String {
    let i = &m.info;
    let mut parts = vec![m.kind.label().to_string()];
    if m.kind == Kind::ThreeMf && i.plates > 1 {
        parts.push(format!("{} plates", i.plates));
    }
    if let Some(t) = i.print_time_s {
        parts.push(duration(t));
    } else if let Some(s) = i.size_mm {
        let l = s[0].max(s[1]).max(s[2]);
        parts.push(format!("{l:.0} mm"));
    }
    if !i.readable && m.kind == Kind::Step {
        parts.push("name only".into());
    }
    parts.join(" · ")
}

pub fn parse_colour(hex: &str) -> Option<egui::Color32> {
    let h = hex.strip_prefix('#')?;
    let v = u32::from_str_radix(h.get(..6)?, 16).ok()?;
    Some(egui::Color32::from_rgb(
        (v >> 16) as u8,
        (v >> 8) as u8,
        v as u8,
    ))
}

/// Shows a file in Finder (or the file manager), selected.
pub fn reveal(path: &Path) -> Result<(), String> {
    let mut cmd = if cfg!(target_os = "macos") {
        let mut c = std::process::Command::new("open");
        c.arg("-R").arg(path);
        c
    } else if cfg!(target_os = "windows") {
        let mut c = std::process::Command::new("explorer");
        c.arg(format!("/select,{}", path.display()));
        c
    } else {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(path.parent().unwrap_or(path));
        c
    };
    cmd.spawn().map(|_| ()).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formatting() {
        assert_eq!(date(0), "1970-01-01");
        assert_eq!(date(1_792_195_200), "2026-10-17");
        assert_eq!(when(1000, 1000 + 86_400 * 3), "3 days ago");
        assert_eq!(duration(4320.), "1 h 12 min");
        assert_eq!(dims([20., 120.5, 3.26]), "20.0 × 120 × 3.3 mm");
        assert_eq!(bytes(1_500_000), "1.5 MB");
        assert_eq!(short_printer("Prusa CORE One HF0.4 nozzle"), "CORE One");
        assert_eq!(
            short_printer("Original Prusa XL - 5T Input Shaper 0.4 nozzle"),
            "XL"
        );
        assert_eq!(short_printer("Snapmaker U1 (0.4 nozzle)"), "U1");
        assert_eq!(
            parse_colour("#8B7CF6"),
            Some(egui::Color32::from_rgb(0x8b, 0x7c, 0xf6))
        );
    }
}
