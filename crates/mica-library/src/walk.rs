// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Walking a library folder: only metadata is read here (names, sizes, times). Hidden
//! folders, version-control folders, NAS housekeeping folders and symlinked folders are
//! skipped.

use crate::model::{ExtraKind, Kind};
use std::path::{Path, PathBuf};

/// Folder names never entered (besides every name starting with a dot).
const SKIPPED_DIRS: &[&str] = &[
    "node_modules",
    "target",
    "__MACOSX",
    "@eaDir",
    "#recycle",
    "#snapshot",
    "$RECYCLE.BIN",
    "System Volume Information",
    "lost+found",
];

/// Whether a file or folder name is skipped: hidden (`.git`, `.Trash`, `._foo.stl` resource
/// forks from network shares) or a housekeeping folder.
pub fn skipped_name(name: &str) -> bool {
    name.starts_with('.') || SKIPPED_DIRS.contains(&name)
}

/// Whether any component of `path` below `root` is skipped.
pub fn skipped_path(root: &Path, path: &Path) -> bool {
    path.strip_prefix(root).is_ok_and(|rel| {
        rel.components()
            .any(|c| skipped_name(&c.as_os_str().to_string_lossy()))
    })
}

/// A candidate file found by the walk.
#[derive(Clone, Debug)]
pub struct Found {
    pub path: PathBuf,
    pub size: u64,
    pub mtime_ns: i128,
    pub kind: Kind,
}

pub fn mtime_ns(meta: &std::fs::Metadata) -> i128 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos() as i128)
}

/// A picture or document found by the walk.
#[derive(Clone, Debug)]
pub struct FoundExtra {
    pub path: PathBuf,
    pub size: u64,
    pub mtime_ns: i128,
    pub kind: ExtraKind,
}

/// The walk's result.
#[derive(Debug, Default)]
pub struct Walk {
    pub files: Vec<Found>,
    /// Pictures and documents (sorted by path).
    pub extras: Vec<FoundExtra>,
    /// Sub-folders that could not be read (permissions, a share dropping out mid-walk).
    pub unreadable: Vec<PathBuf>,
}

/// Walks `start` (a folder below or equal to `root`). `skip` is a folder never entered (the
/// app's own data folder). `stop` is polled between folders.
pub fn walk(start: &Path, skip: Option<&Path>, stop: &dyn Fn() -> bool) -> std::io::Result<Walk> {
    let mut out = Walk::default();
    // The start folder itself must be readable, or the whole folder counts as unavailable.
    let first = std::fs::read_dir(start)?;
    let mut stack = vec![(start.to_path_buf(), Some(first))];
    while let Some((dir, entries)) = stack.pop() {
        if stop() {
            break;
        }
        let entries = match entries {
            Some(e) => e,
            None => match std::fs::read_dir(&dir) {
                Ok(e) => e,
                Err(_) => {
                    out.unreadable.push(dir);
                    continue;
                }
            },
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if skipped_name(&name) {
                continue;
            }
            // `file_type` does not follow symlinks: symlinked folders are not entered (no
            // cycles, no walking out of the library), symlinked files are skipped too.
            let Ok(ft) = entry.file_type() else { continue };
            let path = entry.path();
            if ft.is_dir() {
                if skip.is_some_and(|s| path == s) {
                    continue;
                }
                stack.push((path, None));
            } else if ft.is_file()
                && let Some(kind) = Kind::of(&path)
                && let Ok(meta) = entry.metadata()
            {
                out.files.push(Found {
                    size: meta.len(),
                    mtime_ns: mtime_ns(&meta),
                    path,
                    kind,
                });
            } else if ft.is_file()
                && let Some(kind) = ExtraKind::of(&path)
                && let Ok(meta) = entry.metadata()
            {
                out.extras.push(FoundExtra {
                    size: meta.len(),
                    mtime_ns: mtime_ns(&meta),
                    path,
                    kind,
                });
            }
        }
    }
    out.files.sort_by(|a, b| a.path.cmp(&b.path));
    out.extras.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skips_hidden_and_housekeeping() {
        let dir = crate::testkit::TempDir::new("walk");
        let r = dir.path();
        for p in [
            "a.stl",
            "B.STL",
            "notes.txt",
            ".hidden.stl",
            "._a.stl",
            ".git/x.stl",
            "@eaDir/a.stl/thumb.stl",
            "sub/deep/c.3mf",
            "sub/d.gcode",
            "sub/e.step",
            "node_modules/f.stl",
            "sub/cover.PNG",
            "sub/README",
            "sub/data.csv",
        ] {
            let p = r.join(p);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, b"x").unwrap();
        }
        let w = walk(r, None, &|| false).unwrap();
        let names: Vec<String> = w
            .files
            .iter()
            .map(|f| {
                // Joined with '/' on every platform.
                let rel = f.path.strip_prefix(r).unwrap();
                let parts: Vec<_> = rel
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy())
                    .collect();
                parts.join("/")
            })
            .collect();
        assert_eq!(
            names,
            [
                "B.STL",
                "a.stl",
                "sub/d.gcode",
                "sub/deep/c.3mf",
                "sub/e.step"
            ]
        );
        let extras: Vec<(String, ExtraKind)> = w
            .extras
            .iter()
            .map(|f| {
                (
                    f.path.file_name().unwrap().to_string_lossy().into_owned(),
                    f.kind,
                )
            })
            .collect();
        assert_eq!(
            extras,
            [
                ("README".to_string(), ExtraKind::Document),
                ("cover.PNG".to_string(), ExtraKind::Image),
            ]
        );
        assert!(skipped_path(r, &r.join(".git/x.stl")));
        assert!(!skipped_path(r, &r.join("sub/x.stl")));
        assert!(walk(&r.join("missing"), None, &|| false).is_err());
    }
}
