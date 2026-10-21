// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/Emboss.cpp :: Emboss::get_font_list_by_folder
// Source: prusaslicer@b028299c77 src/libslic3r/Emboss.cpp :: Emboss::create_font_file
// Changes: Scans the platform font folders (macOS, Linux, Windows) once in a background thread and reads family / face names, weight, italic and a variable weight axis with skrifa instead of the registry, wxFontEnumerator or fontconfig; IBM Plex (SIL OFL 1.1, already bundled for the UI) is always offered first so text works without system fonts; font files are loaded on demand and cached.
// Changes: The home folder comes from mica_core::user_data (none in tests and test tools).
// Upstream attribution (Emboss.cpp):
// Copyright (c) Prusa Research 2021 - 2023 Filip Sykala @Jony01, Lukáš Matěna @lukasmatena, Vojtěch Bubník @bubnikv
//
// PrusaSlicer is released under the terms of the AGPLv3 or higher

//! The font picker's list: bundled IBM Plex plus the fonts installed on this computer. Mica
//! ships no font it is not licensed to; system fonts are only read where they are installed.

use skrifa::{MetadataProvider, Tag, raw::FileRef, string::StringId};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

/// Where a face's bytes come from.
#[derive(Clone, Debug, PartialEq)]
pub enum FontData {
    Bundled(&'static [u8]),
    File(PathBuf),
}

/// One face of a font family.
#[derive(Clone, Debug, PartialEq)]
pub struct FontFace {
    pub family: String,
    pub face: String,
    pub data: FontData,
    /// Index inside a collection (`.ttc`).
    pub index: u32,
    /// OS/2 weight class (400 regular, 700 bold).
    pub weight: f32,
    pub italic: bool,
    /// A variable font with a weight (`wght`) axis: bold is a position on it.
    pub weight_axis: Option<(f32, f32)>,
}
impl FontFace {
    /// The file name PrusaSlicer stores as the font descriptor.
    pub fn file_name(&self) -> String {
        match &self.data {
            FontData::Bundled(_) => format!(
                "{}-{}.ttf",
                self.family.replace(' ', ""),
                self.face.replace(' ', "")
            ),
            FontData::File(p) => p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
        }
    }
    pub fn is_bundled(&self) -> bool {
        matches!(self.data, FontData::Bundled(_))
    }
}

const PLEX: [&[u8]; 5] = [
    include_bytes!("../../assets/fonts/IBMPlexSans-Regular.ttf"),
    include_bytes!("../../assets/fonts/IBMPlexSans-Medium.ttf"),
    include_bytes!("../../assets/fonts/IBMPlexSans-SemiBold.ttf"),
    include_bytes!("../../assets/fonts/IBMPlexMono-Regular.ttf"),
    include_bytes!("../../assets/fonts/IBMPlexMono-Medium.ttf"),
];

/// The faces of one font file (all faces of a collection).
fn read_faces(bytes: &[u8], data: &FontData) -> Vec<FontFace> {
    let Ok(file) = FileRef::new(bytes) else {
        return vec![];
    };
    let mut out = vec![];
    for (index, font) in file.fonts().enumerate() {
        let Ok(font) = font else { continue };
        let name = |ids: &[StringId]| {
            ids.iter().find_map(|&id| {
                font.localized_strings(id)
                    .english_or_first()
                    .map(|s| s.to_string())
                    .filter(|s| !s.trim().is_empty())
            })
        };
        let Some(family) = name(&[StringId::TYPOGRAPHIC_FAMILY_NAME, StringId::FAMILY_NAME]) else {
            continue;
        };
        // Hidden system faces (".SF NS …") are not for users.
        if family.starts_with('.') || font.outline_glyphs().iter().next().is_none() {
            continue;
        }
        let face = name(&[
            StringId::TYPOGRAPHIC_SUBFAMILY_NAME,
            StringId::SUBFAMILY_NAME,
        ])
        .unwrap_or_else(|| "Regular".into());
        let attrs = font.attributes();
        let weight_axis = font
            .axes()
            .get_by_tag(Tag::new(b"wght"))
            .map(|a| (a.min_value(), a.max_value()));
        out.push(FontFace {
            family,
            face,
            data: data.clone(),
            index: index as u32,
            weight: attrs.weight.value(),
            italic: !matches!(attrs.style, skrifa::attribute::Style::Normal),
            weight_axis,
        });
    }
    out
}

fn bundled() -> Vec<FontFace> {
    PLEX.iter()
        .flat_map(|b| read_faces(b, &FontData::Bundled(b)))
        .collect()
}

fn font_dirs() -> Vec<PathBuf> {
    let home = mica_core::user_data::home();
    let mut dirs: Vec<PathBuf> = Vec::new();
    if cfg!(target_os = "macos") {
        dirs.extend(["/System/Library/Fonts", "/Library/Fonts"].map(PathBuf::from));
        dirs.extend(home.iter().map(|h| h.join("Library/Fonts")));
    } else if cfg!(target_os = "windows") {
        let windir =
            std::env::var_os("WINDIR").map_or_else(|| PathBuf::from("C:\\Windows"), PathBuf::from);
        dirs.push(windir.join("Fonts"));
        if let Some(local) = mica_core::user_data::var_dir("LOCALAPPDATA") {
            dirs.push(local.join("Microsoft\\Windows\\Fonts"));
        }
    } else {
        dirs.extend(["/usr/share/fonts", "/usr/local/share/fonts"].map(PathBuf::from));
        dirs.extend(
            home.iter()
                .flat_map(|h| [h.join(".local/share/fonts"), h.join(".fonts")]),
        );
    }
    dirs
}

fn scan_dir(dir: &Path, depth: usize, out: &mut Vec<FontFace>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    paths.sort();
    for p in paths {
        if p.file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with('.'))
        {
            continue;
        }
        if p.is_dir() {
            if depth < 4 {
                scan_dir(&p, depth + 1, out);
            }
            continue;
        }
        let ext = p
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        if !matches!(ext.as_str(), "ttf" | "otf" | "ttc" | "otc") {
            continue;
        }
        // Huge collections (CJK) are listed but read lazily like every other file.
        if let Ok(bytes) = std::fs::read(&p) {
            out.extend(read_faces(&bytes, &FontData::File(p.clone())));
        }
    }
}

/// Every face the picker offers: bundled IBM Plex first, then the system's, by family.
#[derive(Debug, Default)]
pub struct FontLibrary {
    pub faces: Vec<FontFace>,
    /// Whether the system folders have been read yet.
    pub complete: bool,
    /// Family names in picker order (bundled first, then alphabetical).
    families: Vec<String>,
}

impl FontLibrary {
    pub fn new(faces: Vec<FontFace>, complete: bool) -> Self {
        let mut seen = std::collections::HashSet::new();
        let families = faces
            .iter()
            .filter(|f| seen.insert(f.family.clone()))
            .map(|f| f.family.clone())
            .collect();
        Self {
            faces,
            complete,
            families,
        }
    }
    /// The family names in picker order (bundled first, then alphabetical).
    pub fn families(&self) -> &[String] {
        &self.families
    }
    pub fn faces_of(&self, family: &str) -> std::vec::IntoIter<&FontFace> {
        self.faces
            .iter()
            .filter(|f| f.family == family)
            .collect::<Vec<_>>()
            .into_iter()
    }
    /// The face to use for `family` with the bold / italic flags: the family's own bold /
    /// italic faces when it has them (closest weight), else its regular face.
    pub fn variant(&self, family: &str, bold: bool, italic: bool) -> Option<&FontFace> {
        let want = if bold { 700. } else { 400. };
        self.faces_of(family).min_by(|a, b| {
            let score = |f: &FontFace| {
                (if f.italic != italic { 10_000. } else { 0. }) + (f.weight - want).abs()
            };
            score(a).total_cmp(&score(b))
        })
    }
    /// Whether the family has a bold face (or a weight axis reaching 600) / an italic face.
    pub fn supports(&self, family: &str) -> (bool, bool) {
        let bold = self
            .faces_of(family)
            .any(|f| f.weight >= 600. || f.weight_axis.is_some_and(|(_, max)| max >= 600.));
        let italic = self.faces_of(family).any(|f| f.italic);
        (bold, italic)
    }
    /// The face a stored text names: exact family + face, then the family's bold / italic
    /// variant, then the same file name, else the default.
    pub fn find(
        &self,
        family: &str,
        face: &str,
        file_name: &str,
        bold: bool,
        italic: bool,
    ) -> &FontFace {
        self.faces
            .iter()
            .find(|f| f.family == family && f.face == face)
            .or_else(|| self.variant(family, bold, italic))
            .or_else(|| {
                (!file_name.is_empty())
                    .then(|| self.faces.iter().find(|f| f.file_name() == file_name))
                    .flatten()
            })
            .unwrap_or_else(|| default_face(self))
    }
}

/// IBM Plex Sans Regular (always present).
pub fn default_face(lib: &FontLibrary) -> &FontFace {
    lib.faces
        .iter()
        .find(|f| f.is_bundled() && f.family == "IBM Plex Sans" && !f.italic && f.weight <= 400.)
        .or_else(|| lib.faces.first())
        .expect("IBM Plex is bundled")
}

static LIBRARY: OnceLock<Mutex<Arc<FontLibrary>>> = OnceLock::new();
static SCAN: OnceLock<()> = OnceLock::new();

fn slot() -> &'static Mutex<Arc<FontLibrary>> {
    LIBRARY.get_or_init(|| Mutex::new(Arc::new(FontLibrary::new(bundled(), false))))
}

/// The current list (bundled only until the background scan finishes).
pub fn library() -> Arc<FontLibrary> {
    slot().lock().map(|l| l.clone()).unwrap_or_default()
}

/// Starts reading the system font folders once, in the background.
pub fn start_scan() {
    SCAN.get_or_init(|| {
        let _ = std::thread::Builder::new()
            .name("font scan".into())
            .spawn(|| {
                let mut faces = bundled();
                let mut system = Vec::new();
                for dir in font_dirs() {
                    scan_dir(&dir, 0, &mut system);
                }
                system.sort_by(|a, b| {
                    a.family
                        .to_lowercase()
                        .cmp(&b.family.to_lowercase())
                        .then(a.italic.cmp(&b.italic))
                        .then(a.weight.total_cmp(&b.weight))
                        .then(a.face.cmp(&b.face))
                });
                system.dedup_by(|a, b| a.family == b.family && a.face == b.face);
                faces.extend(system);
                if let Ok(mut l) = slot().lock() {
                    *l = Arc::new(FontLibrary::new(faces, true));
                }
            });
    });
}

/// The bytes of a face's file (cached; a few files at a time).
pub fn load(face: &FontFace) -> Option<Arc<[u8]>> {
    type Cache = Mutex<Vec<(PathBuf, Arc<[u8]>)>>;
    static CACHE: OnceLock<Cache> = OnceLock::new();
    match &face.data {
        FontData::Bundled(b) => Some(Arc::from(*b)),
        FontData::File(p) => {
            let cache = CACHE.get_or_init(Default::default);
            if let Ok(c) = cache.lock()
                && let Some((_, d)) = c.iter().find(|(q, _)| q == p)
            {
                return Some(d.clone());
            }
            let data: Arc<[u8]> = Arc::from(std::fs::read(p).ok()?);
            if let Ok(mut c) = cache.lock() {
                c.insert(0, (p.clone(), data.clone()));
                c.truncate(6);
            }
            Some(data)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_plex_is_listed_with_its_weights() {
        let lib = FontLibrary::new(bundled(), false);
        let families = lib.families();
        assert_eq!(families[0], "IBM Plex Sans", "{families:?}");
        assert!(families.iter().any(|f| f == "IBM Plex Mono"));
        let regular = default_face(&lib);
        assert_eq!(regular.face, "Regular");
        // SemiBold (600) is the boldest bundled Sans face.
        assert_eq!(lib.supports("IBM Plex Sans"), (true, false));
        assert_eq!(
            lib.variant("IBM Plex Sans", true, false).unwrap().face,
            "SemiBold"
        );
        let found = lib.find("IBM Plex Mono", "Medium", "", false, false);
        assert_eq!(
            (found.family.as_str(), found.face.as_str()),
            ("IBM Plex Mono", "Medium")
        );
        let fallback = lib.find("No Such Font", "Regular", "nothing.ttf", false, false);
        assert_eq!(fallback.family, "IBM Plex Sans");
    }
}
