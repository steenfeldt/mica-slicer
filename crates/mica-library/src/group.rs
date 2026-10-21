// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Projects and grouping: pure functions over a [`Snapshot`], so the view can recompute them
//! cheaply whenever the snapshot or the user's choices change.
//!
//! A *project* is a folder whose files belong together, typically one download from a model
//! site: the parts (STL, OBJ, STEP), an assembled 3MF, sliced G-code, pictures, a readme and a
//! license. Its files are shown as one card. The detection:
//!
//! 1. Every file's folder is mapped to a *candidate*: helper sub-folders of a download
//!    (`files/`, `stl/`, `3mf/`, `images/`, …) fold into the folder above them (up to three
//!    levels), so a download is grouped at its own top folder. Files directly in a library
//!    folder are never grouped.
//! 2. A candidate becomes a project when its files look like they belong together. Signals
//!    worth two points: helper sub-folders were folded in; the folder is named like a
//!    download (`…-model_files`, a model-site id such as `Name - 4823117`); a 3MF named like
//!    the folder or like the other files; one name word shared by at least 60 % of the files.
//!    Signals worth one point: a readme / license / PDF, pictures, a G-code named like a
//!    model, the folder's name inside a file name. Up to 40 files need 3 points, up to 120
//!    need 4, up to 250 need 5; bigger folders, folders with fewer than two files and folders
//!    holding two or more other candidates (collections of downloads) are never projects.
//! 3. The user's choices win: "Treat this folder as a project" ([`Grouping::Project`]) makes
//!    everything below a folder one project (the deepest such folder wins); "Split this
//!    project" ([`Grouping::Split`]) keeps a folder (and the helper folders folded into it)
//!    from being one.
//!
//! Identical content is one model with several locations. A model with a copy in a project is
//! shown inside that project only, not again as a loose card.

use crate::model::{ExtraKind, Grouping, Kind, Model, Snapshot};
use crate::query::{self, Filter, Sort};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Sub-folder names that hold part of a download rather than a model of their own.
const CONTAINERS: &[&str] = &[
    "files",
    "file",
    "stl",
    "stls",
    "stl files",
    "stl_files",
    "3mf",
    "3mfs",
    "obj",
    "step",
    "stp",
    "cad",
    "source",
    "sources",
    "models",
    "model",
    "parts",
    "print files",
    "print_files",
    "printfiles",
    "gcode",
    "g-code",
    "gcodes",
    "sliced",
    "images",
    "image",
    "img",
    "imgs",
    "pictures",
    "pics",
    "photos",
    "renders",
    "render",
    "docs",
    "documentation",
    "instructions",
    "supported",
    "unsupported",
    "presupported",
    "pre-supported",
];

/// Name words that say nothing about what a file is.
const NOISE: &[&str] = &[
    "stl",
    "3mf",
    "obj",
    "step",
    "stp",
    "gcode",
    "bgcode",
    "part",
    "parts",
    "model",
    "models",
    "file",
    "files",
    "print",
    "prints",
    "the",
    "and",
    "for",
    "with",
    "pla",
    "petg",
    "abs",
    "asa",
    "tpu",
    "nozzle",
    "plate",
    "final",
    "fixed",
    "copy",
    "new",
    "old",
    "version",
    "coreone",
    "core",
    "one",
    "mk3",
    "mk3s",
    "mk4",
    "mk4s",
    "mini",
    "prusa",
    "bambu",
    "orca",
    "x1c",
    "p1s",
    "p1p",
    "layer",
    "infill",
    "support",
    "supports",
    "supported",
    "unsupported",
    "presupported",
    "left",
    "right",
    "top",
    "bottom",
    "base",
    "body",
    "small",
    "large",
];

/// Picture names that are a download's cover.
const COVER_WORDS: &[&str] = &[
    "cover",
    "preview",
    "photo",
    "render",
    "thumbnail",
    "thumb",
    "main",
    "title",
    "hero",
    "featured",
];

pub fn is_container(name: &str) -> bool {
    let n = name.to_lowercase();
    CONTAINERS.contains(&n.as_str())
}

/// A picture named like a cover (`cover.png`, `preview_1.jpg`, `photo.jpeg`, …).
pub fn cover_name(path: &Path) -> bool {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    COVER_WORDS.iter().any(|w| stem.contains(w))
}

/// The meaningful words of a name (lower case, no numbers, no file-format or printer noise).
pub fn tokens(name: &str) -> Vec<String> {
    let mut out: Vec<String> = name
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| {
            let digits = t.chars().filter(char::is_ascii_digit).count();
            let letters = t.chars().count() - digits;
            t.chars().count() >= 3
                && !NOISE.contains(t)
                && !(t.starts_with('v') && t.len() > 1 && digits == t.len() - 1)
                && !(t.starts_with(|c: char| c.is_ascii_digit())
                    && (digits >= letters || t.ends_with("mm")))
        })
        .map(str::to_string)
        .collect();
    out.sort();
    out.dedup();
    out
}

fn stem(p: &Path) -> String {
    p.file_stem()
        .map_or_else(String::new, |s| s.to_string_lossy().into_owned())
}

fn file_name(p: &Path) -> String {
    p.file_name().map_or_else(
        || p.display().to_string(),
        |s| s.to_string_lossy().into_owned(),
    )
}

/// A folder named like a model site's download: `…-model_files`, or a site id
/// (`Cable Organizer - 5123456`, `4823117-dragon`).
pub fn download_name(name: &str) -> bool {
    let l = name.to_lowercase();
    if [
        "model_files",
        "model-files",
        "model files",
        "_files",
        "-files",
    ]
    .iter()
    .any(|s| l.ends_with(s))
    {
        return true;
    }
    let words: Vec<&str> = l.split(|c: char| !c.is_alphanumeric()).collect();
    words
        .iter()
        .any(|w| w.len() >= 5 && w.chars().all(|c| c.is_ascii_digit()))
        && words
            .iter()
            .any(|w| w.len() >= 3 && w.chars().all(char::is_alphabetic))
}

/// A project's title from its folder name: download suffixes and site ids removed,
/// underscores (and dashes, in names without spaces) turned into spaces.
pub fn clean_name(raw: &str) -> String {
    let mut s = raw.trim().to_string();
    for suffix in [
        "-model_files",
        "_model_files",
        " model files",
        "-model-files",
        "_files",
        "-files",
    ] {
        if s.len() > suffix.len() && s.to_lowercase().ends_with(suffix) {
            s.truncate(s.len() - suffix.len());
            break;
        }
    }
    let sep = |c: char| c == ' ' || c == '_' || c == '-';
    // A trailing site id: `Name - 4823117`, `name_4823117`.
    if let Some(i) = s.rfind(sep) {
        let tail = &s[i + 1..];
        if tail.len() >= 5 && tail.chars().all(|c| c.is_ascii_digit()) {
            s = s[..i].trim_end_matches(sep).to_string();
        }
    }
    // A leading one: `4823117-dragon`.
    if let Some(i) = s.find(sep) {
        let head = &s[..i];
        if head.len() >= 5 && head.chars().all(|c| c.is_ascii_digit()) {
            s = s[i..].trim_start_matches(sep).to_string();
        }
    }
    s = if s.contains(' ') {
        s.replace('_', " ")
    } else {
        s.replace(['_', '-'], " ")
    };
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.is_empty() { raw.to_string() } else { s }
}

/// One file of a project: a model and which of its locations is in the project.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FileRef {
    pub model: usize,
    pub location: usize,
}

/// What a project card shows as its picture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cover {
    /// A model's thumbnail (index into the snapshot's models).
    Model(usize),
    /// A picture's thumbnail (index into the snapshot's extras).
    Extra(usize),
}

/// A folder of files that belong together.
#[derive(Clone, Debug, PartialEq)]
pub struct Project {
    pub folder: PathBuf,
    /// The library folder it is in.
    pub root: PathBuf,
    /// The cleaned-up folder name.
    pub name: String,
    /// Model files and G-code, by path.
    pub files: Vec<FileRef>,
    /// Pictures and documents (indices into the snapshot's extras), by path.
    pub extras: Vec<usize>,
    pub cover: Option<Cover>,
    /// The user made this folder a project.
    pub chosen: bool,
    /// 3MF files.
    pub projects: usize,
    /// STL, OBJ and STEP files.
    pub parts: usize,
    /// G-code files (text and binary).
    pub gcodes: usize,
    /// Printers its 3MF and G-code files were set up for.
    pub printers: Vec<String>,
    /// Newest modification time.
    pub modified: i64,
    /// All files' sizes together.
    pub size: u64,
    /// The largest model dimension in mm.
    pub largest: Option<f32>,
    /// The shortest print time of its sliced files.
    pub print_time_s: Option<f64>,
    pub online: bool,
}

impl Project {
    /// The text a project is searched by (lower case).
    pub fn haystack(&self) -> String {
        format!("{}\n{}", self.name, self.folder.display()).to_lowercase()
    }

    /// The location of `f` (a file of this project).
    pub fn path<'a>(&self, models: &'a [Model], f: FileRef) -> &'a Path {
        &models[f.model].locations[f.location].path
    }

    /// The 3MF to open as "the project", if there is one.
    pub fn main_3mf(&self, models: &[Model]) -> Option<FileRef> {
        self.files
            .iter()
            .copied()
            .filter(|f| models[f.model].kind == Kind::ThreeMf)
            .min_by_key(|f| (self.path(models, *f).components().count(), f.model))
    }

    /// The parts Mica can put on a plate (STL files).
    pub fn plate_parts(&self, models: &[Model]) -> Vec<FileRef> {
        self.files
            .iter()
            .copied()
            .filter(|f| models[f.model].kind == Kind::Stl)
            .collect()
    }
}

/// Every project of a snapshot.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Projects {
    /// Sorted by folder.
    pub list: Vec<Project>,
    /// For each model (by index): the projects holding a copy of it.
    pub of_model: Vec<Vec<usize>>,
}

impl Projects {
    pub fn by_folder(&self, folder: &Path) -> Option<usize> {
        self.list
            .binary_search_by(|p| p.folder.as_path().cmp(folder))
            .ok()
    }

    /// The project a model's location belongs to.
    pub fn of_location(&self, model: usize, location: usize) -> Option<usize> {
        self.of_model.get(model)?.iter().copied().find(|&p| {
            self.list[p]
                .files
                .iter()
                .any(|f| f.model == model && f.location == location)
        })
    }

    /// Distinct folders (other than `dir`) holding a copy of `model`.
    pub fn other_folders(models: &[Model], model: usize, dir: Option<&Path>) -> usize {
        let mut d: Vec<&Path> = models[model]
            .locations
            .iter()
            .filter_map(|l| l.path.parent())
            .filter(|p| Some(*p) != dir)
            .collect();
        d.sort();
        d.dedup();
        d.len()
    }
}

/// The folder a file's folder is grouped at: helper folders fold into the folder above (never
/// into the library folder itself).
pub fn grouping_folder<'a>(dir: &'a Path, root: &Path) -> &'a Path {
    let mut d = dir;
    for _ in 0..3 {
        let Some(parent) = d.parent() else { break };
        if d == root || parent == root || !parent.starts_with(root) {
            break;
        }
        let name = d
            .file_name()
            .map(|n| n.to_string_lossy())
            .unwrap_or_default();
        if !is_container(&name) {
            break;
        }
        d = parent;
    }
    d
}

/// Who a folder's files belong to.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum Owner {
    Chosen(PathBuf),
    Candidate(PathBuf),
    Loose,
}

fn owner(dir: &Path, root: &Path, grouping: &BTreeMap<PathBuf, Grouping>) -> Owner {
    if dir == root || !dir.starts_with(root) {
        return Owner::Loose;
    }
    if !grouping.is_empty() {
        for a in dir.ancestors() {
            if a == root || !a.starts_with(root) {
                break;
            }
            match grouping.get(a) {
                Some(Grouping::Split) => break,
                Some(Grouping::Project) => return Owner::Chosen(a.to_path_buf()),
                None => {}
            }
        }
    }
    let c = grouping_folder(dir, root);
    if c == root || grouping.get(c) == Some(&Grouping::Split) {
        return Owner::Loose;
    }
    Owner::Candidate(c.to_path_buf())
}

/// What the detection looks at for one candidate folder.
#[derive(Clone, Debug, Default)]
pub struct Evidence {
    /// The folder's own name.
    pub name: String,
    /// Model files and G-code: path and kind.
    pub files: Vec<(PathBuf, Kind)>,
    pub images: usize,
    pub documents: usize,
    /// Helper sub-folders were folded into it.
    pub folded: bool,
    /// Other candidate folders below it.
    pub nested: usize,
}

/// The points a candidate scores (see the module documentation).
pub fn score(e: &Evidence) -> u32 {
    let n = e.files.len();
    let folder: HashSet<String> = tokens(&e.name).into_iter().collect();
    let words: Vec<HashSet<String>> = e
        .files
        .iter()
        .map(|(p, _)| tokens(&stem(p)).into_iter().collect())
        .collect();
    let mut points = 0;
    if e.folded {
        points += 2;
    }
    if download_name(&e.name) {
        points += 2;
    }
    // A 3MF named like the folder or like the other files.
    let related_3mf = e.files.iter().enumerate().any(|(i, (_, k))| {
        if *k != Kind::ThreeMf {
            return false;
        }
        if !words[i].is_disjoint(&folder) {
            return true;
        }
        let others = n - 1;
        let shared = (0..n)
            .filter(|&j| j != i && !words[i].is_disjoint(&words[j]))
            .count();
        others > 0 && shared * 2 >= others
    });
    if related_3mf {
        points += 2;
    }
    // One word in most names.
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for w in &words {
        for t in w {
            *counts.entry(t.as_str()).or_default() += 1;
        }
    }
    let common = counts.values().copied().max().unwrap_or(0);
    if n >= 2 && common >= 2 && common * 10 >= n * 6 {
        points += 2;
    }
    if e.documents > 0 {
        points += 1;
    }
    if e.images > 0 {
        points += 1;
    }
    if words.iter().any(|w| !w.is_disjoint(&folder)) {
        points += 1;
    }
    let gcode_matches = e.files.iter().enumerate().any(|(i, (_, k))| {
        k.is_gcode()
            && e.files
                .iter()
                .enumerate()
                .any(|(j, (_, k2))| !k2.is_gcode() && !words[i].is_disjoint(&words[j]))
    });
    if gcode_matches {
        points += 1;
    }
    points
}

/// Whether a candidate folder is a project.
pub fn is_project(e: &Evidence) -> bool {
    let n = e.files.len();
    if n < 2 || e.nested >= 2 {
        return false;
    }
    let need = match n {
        0..=40 => 3,
        41..=120 => 4,
        121..=250 => 5,
        _ => return false,
    };
    score(e) >= need
}

/// Finds the projects of a snapshot.
pub fn projects(snap: &Snapshot) -> Projects {
    let models = &snap.models;
    // Every folder's files and pictures.
    #[derive(Default)]
    struct Dir<'a> {
        root: Option<&'a Path>,
        files: Vec<FileRef>,
        extras: Vec<usize>,
    }
    let mut dirs: HashMap<&Path, Dir> = HashMap::new();
    for (mi, m) in models.iter().enumerate() {
        for (li, l) in m.locations.iter().enumerate() {
            let Some(d) = l.path.parent() else { continue };
            let e = dirs.entry(d).or_default();
            e.root = Some(&l.root);
            e.files.push(FileRef {
                model: mi,
                location: li,
            });
        }
    }
    for (ei, x) in snap.extras.iter().enumerate() {
        let Some(d) = x.path.parent() else { continue };
        let e = dirs.entry(d).or_default();
        e.root = Some(&x.root);
        e.extras.push(ei);
    }
    // Each folder's owner, and what each owner holds.
    struct Group<'a> {
        root: &'a Path,
        files: Vec<FileRef>,
        extras: Vec<usize>,
        folded: bool,
    }
    let mut groups: BTreeMap<Owner, Group> = BTreeMap::new();
    for (d, dir) in &dirs {
        let Some(root) = dir.root else { continue };
        let o = owner(d, root, &snap.grouping);
        if o == Owner::Loose {
            continue;
        }
        let folded = match &o {
            Owner::Candidate(c) => c.as_path() != *d && !dir.files.is_empty(),
            _ => false,
        };
        let g = groups.entry(o).or_insert_with(|| Group {
            root,
            files: vec![],
            extras: vec![],
            folded: false,
        });
        g.files.extend(&dir.files);
        g.extras.extend(&dir.extras);
        g.folded |= folded;
    }
    // Candidates holding other candidates are collections.
    let mut nested: HashMap<PathBuf, usize> = HashMap::new();
    let candidates: HashSet<&Path> = groups
        .iter()
        .filter(|(o, g)| matches!(o, Owner::Candidate(_)) && !g.files.is_empty())
        .filter_map(|(o, _)| match o {
            Owner::Candidate(c) => Some(c.as_path()),
            _ => None,
        })
        .collect();
    for (o, g) in &groups {
        let Owner::Candidate(c) = o else { continue };
        if g.files.is_empty() {
            continue;
        }
        for a in c.ancestors().skip(1) {
            if a == g.root || !a.starts_with(g.root) {
                break;
            }
            if candidates.contains(a) {
                *nested.entry(a.to_path_buf()).or_default() += 1;
            }
        }
    }
    let mut list = vec![];
    for (o, mut g) in groups {
        let (folder, chosen) = match o {
            Owner::Chosen(f) => (f, true),
            Owner::Candidate(f) => (f, false),
            Owner::Loose => continue,
        };
        if g.files.is_empty() {
            continue;
        }
        let path = |f: &FileRef| &models[f.model].locations[f.location].path;
        g.files.sort_by(|a, b| path(a).cmp(path(b)));
        g.extras
            .sort_by(|a, b| snap.extras[*a].path.cmp(&snap.extras[*b].path));
        let raw = file_name(&folder);
        if !chosen {
            let e = Evidence {
                name: raw.clone(),
                files: g
                    .files
                    .iter()
                    .map(|f| (path(f).clone(), models[f.model].kind))
                    .collect(),
                images: g
                    .extras
                    .iter()
                    .filter(|&&x| snap.extras[x].kind == ExtraKind::Image)
                    .count(),
                documents: g
                    .extras
                    .iter()
                    .filter(|&&x| snap.extras[x].kind == ExtraKind::Document)
                    .count(),
                folded: g.folded,
                nested: nested.get(&folder).copied().unwrap_or(0),
            };
            if !is_project(&e) {
                continue;
            }
        }
        list.push(summarise(
            snap, folder, g.root, &raw, g.files, g.extras, chosen,
        ));
    }
    list.sort_by(|a, b| a.folder.cmp(&b.folder));
    let mut of_model = vec![vec![]; models.len()];
    for (pi, p) in list.iter().enumerate() {
        for f in &p.files {
            let v: &mut Vec<usize> = &mut of_model[f.model];
            if v.last() != Some(&pi) {
                v.push(pi);
            }
        }
    }
    Projects { list, of_model }
}

fn summarise(
    snap: &Snapshot,
    folder: PathBuf,
    root: &Path,
    raw: &str,
    files: Vec<FileRef>,
    extras: Vec<usize>,
    chosen: bool,
) -> Project {
    let models = &snap.models;
    let m = |f: &FileRef| &models[f.model];
    let count = |pred: &dyn Fn(Kind) -> bool| files.iter().filter(|f| pred(m(f).kind)).count();
    let mut printers: Vec<String> = files
        .iter()
        .filter(|f| m(f).kind == Kind::ThreeMf || m(f).kind.is_gcode())
        .filter_map(|f| m(f).info.printer.clone())
        .collect();
    printers.sort();
    printers.dedup();
    let largest = files
        .iter()
        .filter(|f| !m(f).kind.is_gcode())
        .filter_map(|f| m(f).info.size_mm.map(|s| s[0].max(s[1]).max(s[2])))
        .reduce(f32::max);
    let print_time_s = files
        .iter()
        .filter_map(|f| m(f).info.print_time_s)
        .reduce(f64::min);
    // The cover: an embedded 3MF picture, a G-code picture, a picture named like a cover,
    // any picture, a rendered 3MF, then the first part with a picture.
    let with_thumb = |pred: &dyn Fn(&Model) -> bool| {
        files
            .iter()
            .find(|f| m(f).thumbnail.is_some() && pred(m(f)))
            .map(|f| Cover::Model(f.model))
    };
    let picture = |named: bool| {
        extras
            .iter()
            .copied()
            .filter(|&x| {
                let e = &snap.extras[x];
                e.kind == ExtraKind::Image
                    && e.thumbnail.is_some()
                    && (!named || cover_name(&e.path))
            })
            .min_by_key(|&x| snap.extras[x].path.components().count())
            .map(Cover::Extra)
    };
    let cover = with_thumb(&|x| x.kind == Kind::ThreeMf && !x.info.thumbnail_rendered)
        .or_else(|| with_thumb(&|x| x.kind.is_gcode()))
        .or_else(|| picture(true))
        .or_else(|| picture(false))
        .or_else(|| with_thumb(&|x| x.kind == Kind::ThreeMf))
        .or_else(|| with_thumb(&|_| true));
    Project {
        name: clean_name(raw),
        root: root.to_path_buf(),
        projects: count(&|k| k == Kind::ThreeMf),
        parts: count(&|k| matches!(k, Kind::Stl | Kind::Obj | Kind::Step)),
        gcodes: count(&|k| k.is_gcode()),
        printers,
        modified: files
            .iter()
            .map(|f| m(f).locations[f.location].modified)
            .max()
            .unwrap_or(0),
        size: files.iter().map(|f| m(f).locations[f.location].size).sum(),
        largest,
        print_time_s,
        online: files.iter().any(|f| m(f).locations[f.location].online),
        cover,
        files,
        extras,
        chosen,
        folder,
    }
}

/// How the library view groups what it shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum GroupBy {
    /// Projects as one card each, other models one by one.
    #[default]
    Project,
    /// A section per folder.
    Folder,
    /// Every model, ungrouped.
    None,
    /// A section per printer the files were sliced or set up for.
    Printer,
    /// A section per file type.
    Type,
    /// A section per tag.
    Tag,
}

impl GroupBy {
    pub const ALL: [GroupBy; 6] = [
        GroupBy::Project,
        GroupBy::Folder,
        GroupBy::None,
        GroupBy::Printer,
        GroupBy::Type,
        GroupBy::Tag,
    ];
    pub fn label(self) -> &'static str {
        match self {
            GroupBy::Project => "Projects",
            GroupBy::Folder => "Folder",
            GroupBy::None => "None",
            GroupBy::Printer => "Printer",
            GroupBy::Type => "Type",
            GroupBy::Tag => "Tag",
        }
    }
}

/// One card in the view.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Item {
    /// Index into [`Projects::list`].
    Project(usize),
    /// Index into the snapshot's models.
    Model(usize),
}

/// A run of cards under a header (the header is empty when the view is not grouped).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Section {
    pub title: String,
    /// The folder a folder section shows.
    pub folder: Option<PathBuf>,
    pub items: Vec<Item>,
}

/// What the view shows for a search, filters, grouping and order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct View {
    pub sections: Vec<Section>,
    /// For each model: it matches the search and filters.
    pub matched: Vec<bool>,
    /// Models shown at least once.
    pub models: usize,
    /// Projects shown.
    pub projects: usize,
}

impl View {
    /// Every model shown, in display order, once each.
    pub fn model_order(&self, projects: &Projects) -> Vec<usize> {
        let mut seen = HashSet::new();
        let mut out = vec![];
        for s in &self.sections {
            for it in &s.items {
                match *it {
                    Item::Model(m) => {
                        if seen.insert(m) {
                            out.push(m);
                        }
                    }
                    Item::Project(p) => {
                        for f in &projects.list[p].files {
                            if self.matched[f.model] && seen.insert(f.model) {
                                out.push(f.model);
                            }
                        }
                    }
                }
            }
        }
        out
    }
}

/// What items are sorted by.
struct Key {
    name: String,
    modified: i64,
    size: u64,
    largest: f32,
    time: f64,
    kind: Kind,
}

fn model_key(m: &Model) -> Key {
    Key {
        name: m.name.to_lowercase(),
        modified: m.modified(),
        size: m.size(),
        largest: m.info.size_mm.map_or(-1., |s| s[0].max(s[1]).max(s[2])),
        time: m.info.print_time_s.unwrap_or(-1.),
        kind: m.kind,
    }
}

fn project_key(p: &Project) -> Key {
    Key {
        name: p.name.to_lowercase(),
        modified: p.modified,
        size: p.size,
        largest: p.largest.unwrap_or(-1.),
        time: p.print_time_s.unwrap_or(-1.),
        kind: if p.projects > 0 {
            Kind::ThreeMf
        } else {
            Kind::Stl
        },
    }
}

fn compare(a: &Key, b: &Key, sort: Sort, descending: bool) -> std::cmp::Ordering {
    let o = match sort {
        Sort::Name => a.name.cmp(&b.name),
        Sort::Modified => a.modified.cmp(&b.modified),
        Sort::FileSize => a.size.cmp(&b.size),
        Sort::Dimensions => a.largest.total_cmp(&b.largest),
        Sort::PrintTime => a.time.total_cmp(&b.time),
        Sort::Kind => a.kind.cmp(&b.kind),
    };
    let o = if descending { o.reverse() } else { o };
    o.then_with(|| a.name.cmp(&b.name))
}

fn sort_items(
    items: &mut [Item],
    models: &[Model],
    projects: &Projects,
    sort: Sort,
    descending: bool,
) {
    let key = |it: &Item| match *it {
        Item::Project(p) => project_key(&projects.list[p]),
        Item::Model(m) => model_key(&models[m]),
    };
    let mut keyed: Vec<(Key, Item)> = items.iter().map(|it| (key(it), *it)).collect();
    keyed.sort_by(|(ka, a), (kb, b)| {
        compare(ka, kb, sort, descending).then_with(|| {
            let rank = |it: &Item| match *it {
                Item::Project(p) => (0, p),
                Item::Model(m) => (1, m),
            };
            rank(a).cmp(&rank(b))
        })
    });
    for (slot, (_, it)) in items.iter_mut().zip(keyed) {
        *slot = it;
    }
}

/// Sorts the files of a project (by their own file name for "Name").
pub fn sort_files(files: &mut [FileRef], models: &[Model], sort: Sort, descending: bool) {
    files.sort_by(|a, b| {
        let key = |f: &FileRef| {
            let mut k = model_key(&models[f.model]);
            let l = &models[f.model].locations[f.location];
            k.name = file_name(&l.path).to_lowercase();
            k.modified = l.modified;
            k.size = l.size;
            k
        };
        compare(&key(a), &key(b), sort, descending).then(a.cmp(b))
    });
}

/// "Models › Downloads › Dragon" for a folder below a library folder.
pub fn folder_title(dir: &Path, root: &Path) -> String {
    let mut parts = vec![file_name(root)];
    if let Ok(rel) = dir.strip_prefix(root) {
        parts.extend(
            rel.components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned()),
        );
    } else {
        parts = vec![dir.display().to_string()];
    }
    parts.join(" › ")
}

/// Applies the search and filters, groups and sorts. `hay` are the models' haystacks
/// ([`query::haystacks`]); `now` is for the date filter.
#[allow(clippy::too_many_arguments)]
pub fn view(
    models: &[Model],
    hay: &[String],
    projects: &Projects,
    filter: &Filter,
    by: GroupBy,
    sort: Sort,
    descending: bool,
    now: i64,
) -> View {
    let matched: Vec<bool> = models
        .iter()
        .enumerate()
        .map(|(i, m)| query::matches(m, hay.get(i).map_or("", String::as_str), filter, now))
        .collect();
    let all: Vec<usize> = (0..models.len()).filter(|&i| matched[i]).collect();
    let mut sections: Vec<Section> = vec![];
    let one = |items: Vec<usize>| Section {
        items: items.into_iter().map(Item::Model).collect(),
        ..Default::default()
    };
    match by {
        GroupBy::None => sections.push(one(all)),
        GroupBy::Project => {
            let words: Vec<String> = filter
                .text
                .to_lowercase()
                .split_whitespace()
                .map(str::to_string)
                .collect();
            let unfiltered = Filter {
                text: String::new(),
                ..filter.clone()
            };
            let mut items = vec![];
            for (pi, p) in projects.list.iter().enumerate() {
                let by_file = p.files.iter().any(|f| matched[f.model]);
                let by_name = !by_file && !words.is_empty() && {
                    let h = p.haystack();
                    words.iter().all(|w| h.contains(w.as_str()))
                        && (!filter.narrowed()
                            || p.files
                                .iter()
                                .any(|f| query::matches(&models[f.model], "", &unfiltered, now)))
                };
                if by_file || by_name {
                    items.push(Item::Project(pi));
                }
            }
            items.extend(
                all.into_iter()
                    .filter(|&m| projects.of_model.get(m).is_none_or(Vec::is_empty))
                    .map(Item::Model),
            );
            sections.push(Section {
                items,
                ..Default::default()
            });
        }
        GroupBy::Folder => {
            let mut by_dir: BTreeMap<(&Path, &Path), Vec<usize>> = BTreeMap::new();
            for &m in &all {
                for l in &models[m].locations {
                    let d = l.path.parent().unwrap_or(&l.path);
                    let v = by_dir.entry((l.root.as_path(), d)).or_default();
                    if v.last() != Some(&m) {
                        v.push(m);
                    }
                }
            }
            for ((root, dir), items) in by_dir {
                let mut s = one(items);
                s.title = folder_title(dir, root);
                s.folder = Some(dir.to_path_buf());
                sections.push(s);
            }
        }
        GroupBy::Printer => {
            let mut by: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
            let mut none = vec![];
            for &m in &all {
                match models[m].info.printer.as_deref() {
                    Some(p) => by.entry(p).or_default().push(m),
                    None => none.push(m),
                }
            }
            for (p, items) in by {
                let mut s = one(items);
                s.title = p.to_string();
                sections.push(s);
            }
            if !none.is_empty() {
                let mut s = one(none);
                s.title = "No printer".into();
                sections.push(s);
            }
        }
        GroupBy::Type => {
            for k in Kind::ALL {
                let items: Vec<usize> = all
                    .iter()
                    .copied()
                    .filter(|&m| models[m].kind == k)
                    .collect();
                if !items.is_empty() {
                    let mut s = one(items);
                    s.title = k.label().to_string();
                    sections.push(s);
                }
            }
        }
        GroupBy::Tag => {
            let mut by: BTreeMap<String, (String, Vec<usize>)> = BTreeMap::new();
            let mut none = vec![];
            for &m in &all {
                let tags = &models[m].user.tags;
                if tags.is_empty() {
                    none.push(m);
                }
                for t in tags {
                    let e = by
                        .entry(t.to_lowercase())
                        .or_insert_with(|| (t.clone(), vec![]));
                    if e.1.last() != Some(&m) {
                        e.1.push(m);
                    }
                }
            }
            for (_, (t, items)) in by {
                let mut s = one(items);
                s.title = t;
                sections.push(s);
            }
            if !none.is_empty() {
                let mut s = one(none);
                s.title = "No tags".into();
                sections.push(s);
            }
        }
    }
    let mut shown_models = HashSet::new();
    let mut shown_projects = 0;
    for s in &mut sections {
        sort_items(&mut s.items, models, projects, sort, descending);
        for it in &s.items {
            match *it {
                Item::Model(m) => {
                    shown_models.insert(m);
                }
                Item::Project(p) => {
                    shown_projects += 1;
                    for f in &projects.list[p].files {
                        if matched[f.model] {
                            shown_models.insert(f.model);
                        }
                    }
                }
            }
        }
    }
    sections.retain(|s| !s.items.is_empty());
    View {
        sections,
        matched,
        models: shown_models.len(),
        projects: shown_projects,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Extra, Info, Location, UserData};

    const ROOT: &str = "/lib";

    /// A model per path (`content` groups identical copies: same content, one model).
    fn snapshot(files: &[(&str, &str)], extras: &[&str]) -> Snapshot {
        let mut by: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for (path, content) in files {
            by.entry(content).or_default().push(path);
        }
        let models = by
            .into_iter()
            .map(|(content, paths)| {
                let kind = Kind::of(Path::new(paths[0])).unwrap();
                Model {
                    id: content.into(),
                    name: file_name(Path::new(paths[0])),
                    kind,
                    locations: paths
                        .iter()
                        .map(|p| Location {
                            path: PathBuf::from(format!("{ROOT}/{p}")),
                            root: ROOT.into(),
                            size: 100,
                            modified: 1000,
                            online: true,
                        })
                        .collect(),
                    info: Info {
                        readable: true,
                        printer: kind.is_gcode().then(|| "XL".to_string()),
                        size_mm: Some([10., 10., 10.]),
                        ..Default::default()
                    },
                    thumbnail: Some(PathBuf::from(format!("/thumbs/{content}.png"))),
                    user: UserData::default(),
                }
            })
            .collect();
        let extras = extras
            .iter()
            .map(|p| {
                let path = PathBuf::from(format!("{ROOT}/{p}"));
                Extra {
                    kind: ExtraKind::of(&path).unwrap(),
                    thumbnail: (ExtraKind::of(&path) == Some(ExtraKind::Image))
                        .then(|| PathBuf::from("/thumbs/x.png")),
                    path,
                    root: ROOT.into(),
                    size: 10,
                    modified: 1000,
                    online: true,
                }
            })
            .collect();
        Snapshot {
            models,
            extras,
            ..Default::default()
        }
    }

    fn unique(paths: &[&'static str]) -> Vec<(&'static str, &'static str)> {
        paths.iter().map(|p| (*p, *p)).collect()
    }

    fn folders(p: &Projects) -> Vec<String> {
        p.list
            .iter()
            .map(|p| p.folder.strip_prefix(ROOT).unwrap().display().to_string())
            .collect()
    }

    fn download() -> Vec<(&'static str, &'static str)> {
        let mut f = unique(&[
            "Downloads/Articulated_Dragon-model_files/files/head.stl",
            "Downloads/Articulated_Dragon-model_files/files/jaw.stl",
            "Downloads/Articulated_Dragon-model_files/files/body_1.stl",
            "Downloads/Articulated_Dragon-model_files/files/body_2.stl",
            "Downloads/Articulated_Dragon-model_files/files/body_3.stl",
            "Downloads/Articulated_Dragon-model_files/files/tail.stl",
            "Downloads/Articulated_Dragon-model_files/files/wing_left.stl",
            "Downloads/Articulated_Dragon-model_files/files/wing_right.stl",
            "Downloads/Articulated_Dragon-model_files/Articulated_Dragon.3mf",
            "Downloads/Articulated_Dragon-model_files/Articulated_Dragon_0.4n_PLA_COREONE_3h12m.gcode",
        ]);
        // Two identical legs: one model, two files of the project.
        f.push((
            "Downloads/Articulated_Dragon-model_files/files/leg_left.stl",
            "leg",
        ));
        f.push((
            "Downloads/Articulated_Dragon-model_files/files/leg_right.stl",
            "leg",
        ));
        f
    }

    const DOWNLOAD_EXTRAS: &[&str] = &[
        "Downloads/Articulated_Dragon-model_files/images/cover.png",
        "Downloads/Articulated_Dragon-model_files/images/print-1.jpg",
        "Downloads/Articulated_Dragon-model_files/README.txt",
        "Downloads/Articulated_Dragon-model_files/LICENSE.txt",
    ];

    #[test]
    fn names_and_words() {
        assert_eq!(
            clean_name("Articulated_Dragon-model_files"),
            "Articulated Dragon"
        );
        assert_eq!(clean_name("Cable Organizer - 5123456"), "Cable Organizer");
        assert_eq!(clean_name("4823117-flexi-rex"), "flexi rex");
        assert_eq!(clean_name("Bracket_v2"), "Bracket v2");
        assert_eq!(clean_name("T-Rex skull"), "T-Rex skull");
        assert_eq!(clean_name("12345"), "12345");
        assert!(download_name("Cable Organizer - 5123456"));
        assert!(download_name("dragon-model_files"));
        assert!(!download_name("Archive 2024"));
        assert!(!download_name("Printables downloads"));
        assert_eq!(
            tokens("Articulated_Dragon_0.4n_0.2mm_PLA_COREONE_3h12m"),
            ["articulated", "dragon"]
        );
        assert_eq!(tokens("3DBenchy v2"), ["3dbenchy"]);
        assert!(cover_name(Path::new("/a/Cover_Photo.JPG")));
        assert!(!cover_name(Path::new("/a/step-3.png")));
        assert!(is_container("STL") && is_container("files") && !is_container("Dragon"));
    }

    #[test]
    fn a_download_with_parts_project_gcode_and_pictures_is_one_project() {
        let s = snapshot(&download(), DOWNLOAD_EXTRAS);
        let p = projects(&s);
        assert_eq!(folders(&p), ["Downloads/Articulated_Dragon-model_files"]);
        let d = &p.list[0];
        assert_eq!(d.name, "Articulated Dragon");
        assert_eq!((d.projects, d.parts, d.gcodes), (1, 10, 1));
        assert_eq!(
            d.files.len(),
            12,
            "both identical legs are files of the project"
        );
        assert_eq!(d.extras.len(), 4);
        assert_eq!(d.printers, ["XL"]);
        assert!(!d.chosen);
        // The G-code's picture beats a rendered 3MF; an embedded 3MF picture beats both.
        let cover_kind = |p: &Projects, s: &Snapshot| match p.list[0].cover {
            Some(Cover::Model(m)) => s.models[m].kind.label().to_string(),
            Some(Cover::Extra(x)) => file_name(&s.extras[x].path),
            None => "none".into(),
        };
        assert_eq!(cover_kind(&p, &s), "3MF");
        let mut s2 = s.clone();
        for m in &mut s2.models {
            m.info.thumbnail_rendered = true;
        }
        let p2 = projects(&s2);
        assert_eq!(cover_kind(&p2, &s2), "G-code");
        for m in &mut s2.models {
            if m.kind.is_gcode() {
                m.thumbnail = None;
            }
        }
        let p2 = projects(&s2);
        assert_eq!(cover_kind(&p2, &s2), "cover.png");
        // Every part maps back to the project.
        for f in &d.files {
            assert_eq!(p.of_model[f.model], [0]);
        }
        let main = d.main_3mf(&s.models).unwrap();
        assert!(d.path(&s.models, main).ends_with("Articulated_Dragon.3mf"));
        assert_eq!(d.plate_parts(&s.models).len(), 10);
    }

    #[test]
    fn nested_helper_folders_group_at_the_download() {
        let s = snapshot(
            &unique(&[
                "Robot arm/STL/Supported/base.stl",
                "Robot arm/STL/Supported/arm.stl",
                "Robot arm/STL/gripper.stl",
                "Robot arm/3MF/robot arm.3mf",
                "Robot arm/gcode/robot arm.bgcode",
            ]),
            &["Robot arm/Images/photo.jpg"],
        );
        let p = projects(&s);
        assert_eq!(folders(&p), ["Robot arm"]);
        assert_eq!(p.list[0].files.len(), 5);
        assert_eq!(p.list[0].extras.len(), 1);
        // A helper folder right in the library folder is not folded into it.
        let s = snapshot(&unique(&["stl/a.stl", "stl/b.stl"]), &[]);
        assert!(projects(&s).list.is_empty());
    }

    #[test]
    fn collections_and_loose_files_stay_single() {
        // 300 unrelated models: never one project.
        let words = ["gear", "vase", "hook", "clip", "knob", "lid", "box", "rail"];
        let names: Vec<String> = (0..300)
            .map(|i| format!("STL Collection/{}-{i:04}.stl", words[i % words.len()]))
            .collect();
        let refs: Vec<(&str, &str)> = names.iter().map(|n| (n.as_str(), n.as_str())).collect();
        assert!(projects(&snapshot(&refs, &[])).list.is_empty());
        // 60 unrelated models: too few signals.
        assert!(projects(&snapshot(&refs[..60], &[])).list.is_empty());
        // A category folder with a few unrelated models, a G-code and a readme.
        let s = snapshot(
            &unique(&[
                "Toys/knob-0003.stl",
                "Toys/gear-0011.gcode",
                "Toys/0007-cube20.stl",
                "Toys/vase-0019.stl",
            ]),
            &["Toys/readme.txt"],
        );
        assert!(projects(&s).list.is_empty());
        // Files right in the library folder.
        let s = snapshot(
            &unique(&["dragon.stl", "dragon.3mf", "dragon.gcode"]),
            &["cover.png"],
        );
        assert!(projects(&s).list.is_empty());
        // A folder holding several downloads is a collection, its downloads are projects.
        let mut f = download();
        f.extend(unique(&[
            "Downloads/Desk Hook/desk-hook.stl",
            "Downloads/Desk Hook/desk-hook.bgcode",
            "Downloads/loose-a.stl",
            "Downloads/loose-b.stl",
            "Downloads/loose-c.3mf",
        ]));
        let p = projects(&snapshot(&f, DOWNLOAD_EXTRAS));
        assert_eq!(
            folders(&p),
            [
                "Downloads/Articulated_Dragon-model_files",
                "Downloads/Desk Hook"
            ]
        );
    }

    #[test]
    fn the_users_choices_win() {
        let mut s = snapshot(&download(), DOWNLOAD_EXTRAS);
        let dragon = PathBuf::from(format!("{ROOT}/Downloads/Articulated_Dragon-model_files"));
        s.grouping.insert(dragon.clone(), Grouping::Split);
        assert!(projects(&s).list.is_empty());
        // A folder the detection skips, made a project (with everything below it).
        let mut s = snapshot(
            &unique(&[
                "Planter/base.stl",
                "Planter/pot.stl",
                "Planter/v2/saucer.stl",
            ]),
            &[],
        );
        assert!(projects(&s).list.is_empty());
        s.grouping
            .insert(format!("{ROOT}/Planter").into(), Grouping::Project);
        let p = projects(&s);
        assert_eq!(folders(&p), ["Planter"]);
        assert!(p.list[0].chosen);
        assert_eq!(p.list[0].files.len(), 3);
        // A split inside a chosen project keeps that folder out of it.
        s.grouping
            .insert(format!("{ROOT}/Planter/v2").into(), Grouping::Split);
        assert_eq!(projects(&s).list[0].files.len(), 2);
    }

    #[test]
    fn group_by_modes_sort_and_search_inside_projects() {
        let mut f = download();
        // The dragon's head, copied loose into a toys folder.
        f.push((
            "Toys/dragon head.stl",
            "Downloads/Articulated_Dragon-model_files/files/head.stl",
        ));
        f.extend(unique(&["Toys/yo-yo.stl", "Toys/spinner.3mf", "zebra.stl"]));
        let mut s = snapshot(&f, DOWNLOAD_EXTRAS);
        for m in &mut s.models {
            if m.name == "yo-yo.stl" {
                m.user.tags = vec!["Kids".into()];
                m.locations[0].modified = 5000;
            }
        }
        let p = projects(&s);
        let hay = query::haystacks(&s.models);
        let names = |v: &View| -> Vec<Vec<String>> {
            v.sections
                .iter()
                .map(|sec| {
                    let mut out = vec![sec.title.clone()];
                    out.extend(sec.items.iter().map(|it| match *it {
                        Item::Project(i) => format!("[{}]", p.list[i].name),
                        Item::Model(m) => s.models[m].name.clone(),
                    }));
                    out
                })
                .collect()
        };
        let all = Filter::default();
        let v = view(
            &s.models,
            &hay,
            &p,
            &all,
            GroupBy::Project,
            Sort::Name,
            false,
            0,
        );
        // The loose copy of the head is not shown again outside the project.
        assert_eq!(
            names(&v),
            [[
                "",
                "[Articulated Dragon]",
                "spinner.3mf",
                "yo-yo.stl",
                "zebra.stl"
            ]]
        );
        assert_eq!((v.projects, v.models), (1, 14));
        // Sort applies within groups.
        let v = view(
            &s.models,
            &hay,
            &p,
            &all,
            GroupBy::Project,
            Sort::Modified,
            true,
            0,
        );
        assert_eq!(names(&v)[0][1], "yo-yo.stl");
        // No grouping: every model, as before.
        let v = view(
            &s.models,
            &hay,
            &p,
            &all,
            GroupBy::None,
            Sort::Name,
            false,
            0,
        );
        assert_eq!(v.sections.len(), 1);
        assert_eq!(v.sections[0].items.len(), s.models.len());
        // Folders: a section per folder, with the copy in both.
        let v = view(
            &s.models,
            &hay,
            &p,
            &all,
            GroupBy::Folder,
            Sort::Name,
            false,
            0,
        );
        let titles: Vec<String> = v.sections.iter().map(|x| x.title.clone()).collect();
        assert_eq!(
            titles,
            [
                "lib",
                "lib › Downloads › Articulated_Dragon-model_files",
                "lib › Downloads › Articulated_Dragon-model_files › files",
                "lib › Toys"
            ]
        );
        assert_eq!(v.sections[3].items.len(), 3);
        // Type, printer, tag.
        let v = view(
            &s.models,
            &hay,
            &p,
            &all,
            GroupBy::Type,
            Sort::Name,
            false,
            0,
        );
        let titles: Vec<String> = v.sections.iter().map(|x| x.title.clone()).collect();
        assert_eq!(titles, ["STL", "3MF", "G-code"]);
        let v = view(
            &s.models,
            &hay,
            &p,
            &all,
            GroupBy::Printer,
            Sort::Name,
            false,
            0,
        );
        assert_eq!(
            names(&v)[0][..2],
            ["XL", "Articulated_Dragon_0.4n_PLA_COREONE_3h12m.gcode"]
        );
        assert_eq!(v.sections[1].title, "No printer");
        let v = view(
            &s.models,
            &hay,
            &p,
            &all,
            GroupBy::Tag,
            Sort::Name,
            false,
            0,
        );
        assert_eq!(names(&v)[0], ["Kids", "yo-yo.stl"]);
        assert_eq!(v.sections[1].title, "No tags");
        // A search matching a file inside a project shows the project, the file marked.
        let wing = Filter {
            text: "wing_left".into(),
            ..Default::default()
        };
        let v = view(
            &s.models,
            &hay,
            &p,
            &wing,
            GroupBy::Project,
            Sort::Name,
            false,
            0,
        );
        assert_eq!(names(&v), [["", "[Articulated Dragon]"]]);
        let marked: Vec<String> = p.list[0]
            .files
            .iter()
            .filter(|f| v.matched[f.model])
            .map(|f| file_name(p.list[0].path(&s.models, *f)))
            .collect();
        assert_eq!(marked, ["wing_left.stl"]);
        // The project's name matches too (here the folder name is in every path anyway).
        let v = view(
            &s.models,
            &hay,
            &p,
            &Filter {
                text: "articulated dragon".into(),
                ..Default::default()
            },
            GroupBy::Project,
            Sort::Name,
            false,
            0,
        );
        assert_eq!(names(&v), [["", "[Articulated Dragon]"]]);
        // A filter narrowing to G-code: the project (it has one) and nothing else.
        let mut g = Filter::default();
        g.kinds.insert(Kind::Gcode);
        let v = view(
            &s.models,
            &hay,
            &p,
            &g,
            GroupBy::Project,
            Sort::Name,
            false,
            0,
        );
        assert_eq!(names(&v), [["", "[Articulated Dragon]"]]);
        // Copies elsewhere.
        let head = s
            .models
            .iter()
            .position(|m| m.locations.len() == 2)
            .unwrap();
        let dir = s.models[head].locations[0]
            .path
            .parent()
            .unwrap()
            .to_path_buf();
        assert_eq!(Projects::other_folders(&s.models, head, Some(&dir)), 1);
        // Files inside a project, sorted by their own names.
        let mut files = p.list[0].files.clone();
        sort_files(&mut files, &s.models, Sort::Name, true);
        assert!(
            p.list[0]
                .path(&s.models, files[0])
                .ends_with("wing_right.stl")
        );
    }
}
