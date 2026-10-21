// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! What the library knows about files and models. A *file* is one path on disk; a *model* is
//! one content hash, so identical copies in several folders are one model with several
//! locations.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The file types the library indexes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Kind {
    Stl,
    ThreeMf,
    Obj,
    Step,
    Gcode,
    Bgcode,
}

impl Kind {
    pub const ALL: [Kind; 6] = [
        Kind::Stl,
        Kind::ThreeMf,
        Kind::Obj,
        Kind::Step,
        Kind::Gcode,
        Kind::Bgcode,
    ];

    /// The kind of a path by its extension (case-insensitive), or `None` for other files.
    pub fn of(path: &Path) -> Option<Kind> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        Some(match ext.as_str() {
            "stl" => Kind::Stl,
            "3mf" => Kind::ThreeMf,
            "obj" => Kind::Obj,
            "step" | "stp" => Kind::Step,
            "gcode" | "gco" => Kind::Gcode,
            "bgcode" => Kind::Bgcode,
            _ => return None,
        })
    }

    /// Short label for chips and the list view.
    pub fn label(self) -> &'static str {
        match self {
            Kind::Stl => "STL",
            Kind::ThreeMf => "3MF",
            Kind::Obj => "OBJ",
            Kind::Step => "STEP",
            Kind::Gcode => "G-code",
            Kind::Bgcode => "Binary G-code",
        }
    }

    /// Whether Mica can open this kind as a model or project.
    pub fn openable(self) -> bool {
        matches!(self, Kind::Stl | Kind::ThreeMf)
    }

    /// G-code (text or binary): already sliced.
    pub fn is_gcode(self) -> bool {
        matches!(self, Kind::Gcode | Kind::Bgcode)
    }
}

/// A file next to the models that is not one: a picture or a document. Indexed by path only
/// (pictures get a small cached thumbnail when they can be a project's cover).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ExtraKind {
    /// PNG or JPEG.
    Image,
    /// A readme, license, attribution or instructions file, or any PDF.
    Document,
}

impl ExtraKind {
    /// The kind of a path by its name, or `None` for files the library ignores.
    pub fn of(path: &Path) -> Option<ExtraKind> {
        let name = path.file_name()?.to_str()?.to_ascii_lowercase();
        let (stem, ext) = match name.rsplit_once('.') {
            Some((s, e)) => (s, e),
            None => (name.as_str(), ""),
        };
        match ext {
            "png" | "jpg" | "jpeg" => return Some(ExtraKind::Image),
            "pdf" => return Some(ExtraKind::Document),
            "" | "txt" | "md" | "html" | "htm" | "rtf" => {}
            _ => return None,
        }
        const DOCS: [&str; 7] = [
            "readme",
            "license",
            "licence",
            "attribution",
            "credits",
            "instructions",
            "assembly",
        ];
        DOCS.iter()
            .any(|d| stem.starts_with(d))
            .then_some(ExtraKind::Document)
    }

    pub fn label(self) -> &'static str {
        match self {
            ExtraKind::Image => "Picture",
            ExtraKind::Document => "Document",
        }
    }
}

/// One indexed picture or document.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExtraRecord {
    pub root: PathBuf,
    pub size: u64,
    pub mtime_ns: i128,
    pub kind: ExtraKind,
    /// The cached thumbnail's file name in the thumbnail folder.
    #[serde(default)]
    pub thumbnail: Option<String>,
}

/// A picture or document as the library view shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct Extra {
    pub path: PathBuf,
    pub root: PathBuf,
    pub kind: ExtraKind,
    pub size: u64,
    /// Seconds since the Unix epoch.
    pub modified: i64,
    pub thumbnail: Option<PathBuf>,
    pub online: bool,
}

impl Extra {
    pub fn name(&self) -> String {
        self.path
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
    }
}

/// How the user wants a folder grouped, overriding the automatic project detection. Kept by
/// folder path, so it survives rescans.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Grouping {
    /// Everything under the folder is one project.
    Project,
    /// The folder is never a project: its files are shown one by one.
    Split,
}

/// Everything read from a file's content. Keyed by content hash, so it is extracted once per
/// unique content, however many copies exist.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Info {
    /// Mica understood the content (STEP files are indexed by name only).
    pub readable: bool,
    /// Why reading failed, in plain words.
    pub error: Option<String>,
    /// Bounding box size in mm (X, Y, Z).
    pub size_mm: Option<[f32; 3]>,
    pub triangles: u64,
    pub objects: u32,
    pub plates: u32,
    pub printer: Option<String>,
    pub print_profile: Option<String>,
    pub filament_profiles: Vec<String>,
    /// Filament colours as `#RRGGBB`, one per tool used.
    pub colours: Vec<String>,
    /// The file carries sliced G-code (a G-code file, or a 3MF with embedded plate G-code).
    pub sliced: bool,
    pub print_time_s: Option<f64>,
    pub filament_g: Option<f64>,
    pub filament_m: Option<f64>,
    /// The cached thumbnail's file extension (`png` or `jpg`), when there is one.
    pub thumbnail: Option<String>,
    /// Where the thumbnail came from: embedded in the file or rendered by Mica.
    pub thumbnail_rendered: bool,
    /// The version of Mica's thumbnail renderer that last rendered (or tried to render) this
    /// content (`crate::render::RENDER_VERSION`); 0: never.
    pub render_version: u32,
    /// Why no thumbnail could be rendered (too detailed, unreadable mesh), in plain words.
    pub no_render: Option<String>,
}

impl Info {
    /// More than one distinct filament colour.
    pub fn multi_colour(&self) -> bool {
        let mut c: Vec<String> = self
            .colours
            .iter()
            .map(|c| c.to_ascii_lowercase())
            .collect();
        c.sort();
        c.dedup();
        c.len() > 1
    }
}

/// One indexed path.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FileRecord {
    /// The library folder the file was found under.
    pub root: PathBuf,
    pub size: u64,
    /// Modification time, nanoseconds since the Unix epoch.
    pub mtime_ns: i128,
    /// xxh3-128 of the content, as 32 hex digits.
    pub hash: String,
    pub kind: Kind,
}

/// Tags and favourites the user gave a model (by content, so every copy shares them).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UserData {
    pub tags: Vec<String>,
    pub favourite: bool,
}

impl UserData {
    pub fn is_empty(&self) -> bool {
        self.tags.is_empty() && !self.favourite
    }
}

/// A configured library folder.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Folder {
    pub path: PathBuf,
}

/// What the scanner is doing with one folder.
#[derive(Clone, Debug, PartialEq)]
pub enum FolderState {
    /// Not scanned yet in this session.
    Waiting,
    Scanning {
        done: usize,
        total: usize,
    },
    /// Up to date (and watched for changes).
    Ready,
    /// The folder (or the network share holding it) is not available. Its models are kept.
    Offline,
    Error(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct FolderStatus {
    pub path: PathBuf,
    pub state: FolderState,
    /// Indexed files under this folder.
    pub files: usize,
    /// Live updates are on.
    pub watching: bool,
    /// Seconds the last full scan took.
    pub last_scan_s: Option<f64>,
}

/// One place a model's content is stored.
#[derive(Clone, Debug, PartialEq)]
pub struct Location {
    pub path: PathBuf,
    pub root: PathBuf,
    pub size: u64,
    /// Seconds since the Unix epoch.
    pub modified: i64,
    /// The location's folder is reachable right now.
    pub online: bool,
}

impl Location {
    /// The containing folder, relative to the library folder (`""` at its top).
    pub fn folder(&self) -> String {
        let parent = self.path.parent().unwrap_or(&self.path);
        parent
            .strip_prefix(&self.root)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| parent.to_string_lossy().into_owned())
    }
}

/// One unique content, as the library view shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct Model {
    /// The content hash.
    pub id: String,
    /// File name of the primary location.
    pub name: String,
    pub kind: Kind,
    /// The primary location first (online before offline, then by path).
    pub locations: Vec<Location>,
    pub info: Info,
    pub thumbnail: Option<PathBuf>,
    pub user: UserData,
}

impl Model {
    pub fn primary(&self) -> &Location {
        &self.locations[0]
    }
    pub fn online(&self) -> bool {
        self.locations.iter().any(|l| l.online)
    }
    /// The newest modification time of any copy.
    pub fn modified(&self) -> i64 {
        self.locations.iter().map(|l| l.modified).max().unwrap_or(0)
    }
    pub fn size(&self) -> u64 {
        self.primary().size
    }
    /// A thumbnail is still to be rendered for this model (the grid shows a placeholder).
    pub fn thumbnail_pending(&self) -> bool {
        self.thumbnail.is_none() && crate::render::needs_render(self.kind, &self.info)
    }
    /// The model has a 3D shape Mica can show (STL, OBJ, a readable 3MF).
    pub fn has_mesh(&self) -> bool {
        matches!(self.kind, Kind::Stl | Kind::Obj | Kind::ThreeMf) && self.info.readable
    }
}

/// The whole library as last published by the scanner.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub models: Vec<Model>,
    /// Pictures and documents next to the models, sorted by path.
    pub extras: Vec<Extra>,
    /// The user's grouping choices by folder.
    pub grouping: std::collections::BTreeMap<PathBuf, Grouping>,
    pub folders: Vec<FolderStatus>,
    /// Files indexed (counting every copy).
    pub files: usize,
    /// Thumbnails waiting to be rendered.
    pub pending_thumbnails: usize,
}

impl Snapshot {
    pub fn scanning(&self) -> bool {
        self.folders
            .iter()
            .any(|f| matches!(f.state, FolderState::Scanning { .. }))
    }
    /// Overall progress over every folder being scanned: (done, total).
    pub fn progress(&self) -> (usize, usize) {
        self.folders
            .iter()
            .filter_map(|f| match f.state {
                FolderState::Scanning { done, total } => Some((done, total)),
                _ => None,
            })
            .fold((0, 0), |(a, b), (d, t)| (a + d, b + t))
    }
}
