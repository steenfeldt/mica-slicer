// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Importing the user's own presets from other slicers (PrusaSlicer, OrcaSlicer, Snapmaker
//! Orca) into Mica's profile store, on request: finding them, telling which are new, already
//! imported or changed since, importing a selection idempotently and spotting duplicates.
//!
//! Everything here only reads the other slicers' folders; removing an imported profile moves
//! Mica's copy to Mica's trash and never touches the original file.
//!
//! An imported profile's identity is its source and file (`ImportInfo::source`, `path`), so
//! importing the same file again updates Mica's copy instead of adding a second one.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::orca;
use crate::prusa::{Bundle, Kind};
use crate::user::{ImportInfo, ProfileError, UserProfile, import_prusa_user_preset, kind_name};

/// A slicer whose user presets Mica can import.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Source {
    OrcaSlicer,
    SnapmakerOrca,
    PrusaSlicer,
}

impl Source {
    pub const ALL: [Source; 3] = [
        Source::OrcaSlicer,
        Source::SnapmakerOrca,
        Source::PrusaSlicer,
    ];

    /// The name kept in profile files and `mica.toml`.
    pub fn key(self) -> &'static str {
        match self {
            Source::OrcaSlicer => "orcaslicer",
            Source::SnapmakerOrca => "snapmaker-orca",
            Source::PrusaSlicer => "prusaslicer",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.key() == key.trim())
    }

    /// The application's name, as its users know it.
    pub fn app_name(self) -> &'static str {
        match self {
            Source::OrcaSlicer => "OrcaSlicer",
            Source::SnapmakerOrca => "Snapmaker Orca",
            Source::PrusaSlicer => "PrusaSlicer",
        }
    }

    /// The short badge on imported profiles in pickers and lists.
    pub fn badge(self) -> &'static str {
        match self {
            Source::OrcaSlicer => "Orca",
            Source::SnapmakerOrca => "Snapmaker Orca",
            Source::PrusaSlicer => "PrusaSlicer",
        }
    }

    pub fn is_orca(self) -> bool {
        self != Source::PrusaSlicer
    }
}

/// A folder holding one slicer's user presets in `printer|machine`, `print|process` and
/// `filament` subfolders.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceDir {
    pub source: Source,
    pub root: PathBuf,
}

/// The folders a slicer may keep its settings in for the user whose home folder is `home`
/// (`appdata`: Windows' `APPDATA`): macOS, Linux, Flatpak, Windows, in that order. They may not
/// exist.
pub fn app_bases(source: Source, home: &Path, appdata: Option<&Path>) -> Vec<PathBuf> {
    let (app, flatpak) = match source {
        Source::PrusaSlicer => (
            "PrusaSlicer",
            ".var/app/com.prusa3d.PrusaSlicer/config/PrusaSlicer",
        ),
        Source::OrcaSlicer => (
            "OrcaSlicer",
            ".var/app/com.orcaslicer.OrcaSlicer/config/OrcaSlicer",
        ),
        Source::SnapmakerOrca => (
            "Snapmaker_Orca",
            ".var/app/com.snapmaker.Snapmaker_Orca/config/Snapmaker_Orca",
        ),
    };
    let mut bases = vec![
        home.join("Library/Application Support").join(app),
        home.join(".config").join(app),
        home.join(flatpak),
    ];
    if let Some(a) = appdata {
        bases.push(a.join(app));
    }
    bases
}

/// The user preset folders of the slicers installed for the user whose home folder is `home`
/// (`appdata`: Windows' `APPDATA`). PrusaSlicer: its data folder; OrcaSlicer and Snapmaker
/// Orca: every `user/<account>` folder (`default` when not logged in). Only folders that exist.
pub fn source_dirs(home: &Path, appdata: Option<&Path>) -> Vec<SourceDir> {
    let mut out = Vec::new();
    if let Some(root) = app_bases(Source::PrusaSlicer, home, appdata)
        .into_iter()
        .find(|d| {
            d.join("PrusaSlicer.ini").is_file()
                || ["printer", "print", "filament"]
                    .iter()
                    .any(|k| d.join(k).is_dir())
        })
    {
        out.push(SourceDir {
            source: Source::PrusaSlicer,
            root,
        });
    }
    for source in [Source::OrcaSlicer, Source::SnapmakerOrca] {
        for base in app_bases(source, home, appdata) {
            let Ok(entries) = std::fs::read_dir(base.join("user")) else {
                continue;
            };
            let mut users: Vec<PathBuf> = entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.is_dir())
                .collect();
            users.sort();
            out.extend(users.into_iter().map(|root| SourceDir { source, root }));
        }
    }
    out
}

/// A user preset found in another slicer's folder.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    pub source: Source,
    pub kind: Kind,
    pub path: PathBuf,
    /// The preset's name in that slicer.
    pub name: String,
    /// Its parent (a system preset), empty for a preset of its own.
    pub inherits: String,
    /// [`hash`] of the file's bytes.
    pub hash: String,
}

fn kind_folders(source: Source) -> [(Kind, &'static str); 3] {
    if source.is_orca() {
        [
            (Kind::Printer, "machine"),
            (Kind::Print, "process"),
            (Kind::Filament, "filament"),
        ]
    } else {
        [
            (Kind::Printer, "printer"),
            (Kind::Print, "print"),
            (Kind::Filament, "filament"),
        ]
    }
}

/// Every user preset in `dirs`, sorted by source, kind and file name; a file that cannot be
/// read is reported in the second list and skipped.
pub fn scan(dirs: &[SourceDir]) -> (Vec<Found>, Vec<String>) {
    let mut found = Vec::new();
    let mut errors = Vec::new();
    for dir in dirs {
        for (kind, folder) in kind_folders(dir.source) {
            let mut files = Vec::new();
            // Orca keeps presets made with "Create filament" in a `base` subfolder.
            for sub in [dir.root.join(folder), dir.root.join(folder).join("base")] {
                if let Ok(entries) = std::fs::read_dir(&sub) {
                    files.extend(entries.flatten().map(|e| e.path()).filter(|p| {
                        p.is_file()
                            && p.extension().is_some_and(|e| {
                                e == if dir.source.is_orca() { "json" } else { "ini" }
                            })
                    }));
                }
            }
            files.sort();
            for path in files {
                match read_found(dir.source, kind, &path) {
                    Ok(f) => found.push(f),
                    Err(e) => errors.push(format!("{}: {e}", path.display())),
                }
            }
        }
    }
    (found, errors)
}

fn read_found(source: Source, kind: Kind, path: &Path) -> Result<Found, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (name, inherits) = if source.is_orca() {
        let json: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|e| format!("not a preset file: {e}"))?;
        let s = |k: &str| {
            json.get(k)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        };
        let name = Some(s("name")).filter(|n| !n.is_empty()).unwrap_or(stem);
        (name, s("inherits"))
    } else {
        let text = String::from_utf8_lossy(&bytes);
        let inherits = crate::ini::parse(&text)
            .into_iter()
            .flat_map(|s| s.entries)
            .find(|(k, _)| k == "inherits")
            .map(|(_, v)| v.trim().trim_matches('"').to_string())
            .unwrap_or_default();
        (stem, inherits)
    };
    Ok(Found {
        source,
        kind,
        path: path.to_path_buf(),
        name,
        inherits,
        hash: hash(&bytes),
    })
}

/// A short content fingerprint (64-bit FNV-1a, hex) to notice a source file changed.
pub fn hash(bytes: &[u8]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// Seconds since 1970.
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// How a found preset relates to Mica's profiles.
#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    /// Not imported yet.
    New,
    /// Imported as the named Mica profile, unchanged since.
    Imported(String),
    /// Imported as the named Mica profile; the file has changed since.
    Changed(String),
}

/// The Mica profile imported from `found`'s file, if any.
pub fn imported_copy<'a>(found: &Found, profiles: &'a [UserProfile]) -> Option<&'a UserProfile> {
    profiles.iter().find(|p| {
        p.kind == found.kind
            && p.source() == Some(found.source)
            && p.source_path().as_deref() == Some(found.path.as_path())
    })
}

pub fn status(found: &Found, profiles: &[UserProfile]) -> Status {
    match imported_copy(found, profiles) {
        None => Status::New,
        Some(p) => match &p.import {
            Some(i) if i.hash != found.hash => Status::Changed(p.name.clone()),
            // Imported before Mica kept a fingerprint: can't tell, count it as current.
            _ => Status::Imported(p.name.clone()),
        },
    }
}

/// A free name for an imported profile: its own name, else "<name> (<badge>)", else with a
/// number. `taken` says whether a name is used by another profile of the same kind.
pub fn unique_name(name: &str, source: Source, taken: impl Fn(&str) -> bool) -> String {
    if !taken(name) {
        return name.to_string();
    }
    let tagged = format!("{name} ({})", source.badge());
    if !taken(&tagged) {
        return tagged;
    }
    (2..)
        .map(|n| format!("{name} ({} {n})", source.badge()))
        .find(|n| !taken(n))
        .expect("an unbounded range always has a free name")
}

/// The name `found` is imported under: the name of Mica's earlier copy of the same file (so
/// importing again updates it), else a free name ([`unique_name`]) among `existing` and the
/// names `taken` reports (the vendor bundle's own presets).
pub fn target_name(
    found: &Found,
    existing: &[UserProfile],
    taken: impl Fn(&str) -> bool,
) -> String {
    if let Some(p) = imported_copy(found, existing) {
        return p.name.clone();
    }
    unique_name(&found.name, found.source, |n| {
        taken(n) || existing.iter().any(|p| p.kind == found.kind && p.name == n)
    })
}

/// Saves an imported profile, replacing Mica's earlier copy of the same file, and updates
/// `existing` to match.
pub fn save(
    store: &crate::user::Store,
    profile: UserProfile,
    existing: &mut Vec<UserProfile>,
) -> Result<PathBuf, ProfileError> {
    let path = store.save(&profile)?;
    existing.retain(|p| !(p.kind == profile.kind && p.name == profile.name));
    existing.push(profile);
    Ok(path)
}

fn import_info(found: &Found, date: u64) -> ImportInfo {
    ImportInfo {
        source: found.source,
        path: found.path.clone(),
        original_name: found.name.clone(),
        date,
        hash: found.hash.clone(),
        settings: None,
    }
}

/// A PrusaSlicer user preset as a Mica profile named `name` (only its real changes against the
/// parent, see `user::import_prusa_user_preset`).
pub fn prusa_profile(
    found: &Found,
    bundle: &Bundle,
    name: &str,
    date: u64,
) -> Result<UserProfile, ProfileError> {
    let mut p = import_prusa_user_preset(&found.path, found.kind, bundle)?;
    p.name = name.to_string();
    p.import = Some(ImportInfo {
        settings: Some(p.settings.clone()),
        ..import_info(found, date)
    });
    Ok(p)
}

/// An OrcaSlicer / Snapmaker Orca user preset as a Mica profile named `name`, built on the Orca
/// vendor `vendor` (Mica's vendor key, e.g. `Snapmaker`) at `version`. The preset's own values
/// are kept as they are (`UserProfile::orca`).
pub fn orca_profile(
    found: &Found,
    vendor: &str,
    version: &str,
    name: &str,
    date: u64,
) -> Result<UserProfile, String> {
    let text = std::fs::read_to_string(&found.path)
        .map_err(|e| format!("{}: {e}", found.path.display()))?;
    let json: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", found.path.display()))?;
    let obj = json
        .as_object()
        .ok_or_else(|| format!("{}: not a preset file", found.path.display()))?;
    let values: orca::RawConfig = obj
        .iter()
        .filter(|(k, _)| !matches!(k.as_str(), "name" | "inherits"))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    Ok(UserProfile {
        kind: found.kind,
        name: name.to_string(),
        inherits: found.inherits.clone(),
        inherits_source: vendor.to_string(),
        inherits_version: version.to_string(),
        imported_from: Some(format!(
            "{}: {}",
            found.source.app_name(),
            found.path.display()
        )),
        settings: Default::default(),
        import: Some(import_info(found, date)),
        orca: Some(values),
    })
}

/// The Orca index whose filaments every Orca vendor shares (`Generic PLA @System`, …). A
/// profile built directly on one of them records it as its `inherits_source` and is listed for
/// every printer sliced with OrcaSlicer rules, as OrcaSlicer itself lists them.
pub const ORCA_LIBRARY: &str = "OrcaFilamentLibrary";

/// Mica's own changes to an imported profile: setting → Mica's value (`None`: Mica removed it).
/// For an Orca import every `settings` entry is Mica's; for a PrusaSlicer import, what differs
/// from the settings as imported (none known for imports made before Mica kept them).
pub fn mica_edits(p: &UserProfile) -> BTreeMap<String, Option<String>> {
    if p.orca.is_some() {
        return p
            .settings
            .iter()
            .map(|(k, v)| (k.clone(), Some(v.clone())))
            .collect();
    }
    let Some(base) = p.import.as_ref().and_then(|i| i.settings.as_ref()) else {
        return BTreeMap::new();
    };
    let mut edits: BTreeMap<String, Option<String>> = p
        .settings
        .iter()
        .filter(|(k, v)| base.get(*k) != Some(*v))
        .map(|(k, v)| (k.clone(), Some(v.clone())))
        .collect();
    for k in base.keys() {
        if !p.settings.contains_key(k) {
            edits.insert(k.clone(), None);
        }
    }
    edits
}

/// The settings (Mica's names) the other slicer changed between Mica's earlier copy `old` and a
/// fresh import `new` of the same file.
pub fn source_changes(old: &UserProfile, new: &UserProfile) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    match (&old.orca, &new.orca) {
        (Some(a), Some(b)) => {
            let table = orca::mapping::mapping_table();
            for k in a.keys().chain(b.keys()) {
                if a.get(k) != b.get(k) {
                    out.insert(k.clone());
                    if let Some((canonical, _)) = table.get(k.as_str()) {
                        out.insert((*canonical).to_string());
                    }
                }
            }
        }
        _ => {
            let empty = BTreeMap::new();
            let before = old
                .import
                .as_ref()
                .and_then(|i| i.settings.as_ref())
                .unwrap_or(&empty);
            let after = new
                .import
                .as_ref()
                .and_then(|i| i.settings.as_ref())
                .unwrap_or(&new.settings);
            for k in before.keys().chain(after.keys()) {
                if before.get(k) != after.get(k) {
                    out.insert(k.clone());
                }
            }
        }
    }
    out
}

/// The settings Mica changed that the other slicer changed too since `old` was imported: what
/// a re-import asks about ("Changed in PrusaSlicer and in Mica").
pub fn conflicts(old: &UserProfile, new: &UserProfile) -> Vec<String> {
    let changed = source_changes(old, new);
    mica_edits(old)
        .into_keys()
        .filter(|k| changed.contains(k))
        .collect()
}

/// Lays Mica's own changes to `old` on top of `new`, a fresh import of the same file: a
/// re-import keeps them. With `take_theirs`, the settings both sides changed take the other
/// slicer's value instead.
pub fn carry_edits(old: &UserProfile, new: &mut UserProfile, take_theirs: bool) {
    let changed = if take_theirs {
        source_changes(old, new)
    } else {
        BTreeSet::new()
    };
    for (k, v) in mica_edits(old) {
        if changed.contains(&k) {
            continue;
        }
        match v {
            Some(v) => {
                new.settings.insert(k, v);
            }
            None => {
                new.settings.remove(&k);
            }
        }
    }
}

/// Groups of profiles (indexes into `profiles`) with identical content, each group holding at
/// least one imported profile: what "Merge duplicates" offers.
pub fn duplicates(profiles: &[UserProfile]) -> Vec<Vec<usize>> {
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for (i, p) in profiles.iter().enumerate() {
        match groups.iter_mut().find(|g| profiles[g[0]].same_content(p)) {
            Some(g) => g.push(i),
            None => groups.push(vec![i]),
        }
    }
    groups.retain(|g| g.len() > 1 && g.iter().any(|&i| profiles[i].source().is_some()));
    groups
}

/// "12 printer, 30 filament and 8 print profiles": counts per kind in reading order.
pub fn count_phrase(printers: usize, filaments: usize, prints: usize) -> String {
    let parts: Vec<String> = [
        (printers, kind_name(Kind::Printer)),
        (filaments, kind_name(Kind::Filament)),
        (prints, kind_name(Kind::Print)),
    ]
    .into_iter()
    .filter(|(n, _)| *n > 0)
    .map(|(n, k)| format!("{n} {k}"))
    .collect();
    let total = printers + filaments + prints;
    let noun = if total == 1 { "profile" } else { "profiles" };
    match parts.as_slice() {
        [] => format!("no {noun}"),
        [one] => format!("{one} {noun}"),
        [rest @ .., last] => format!("{} and {last} {noun}", rest.join(", ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mica-import-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn finds_presets_of_every_slicer_in_a_fake_home() {
        let h = home("discover");
        let orca = h.join("Library/Application Support/OrcaSlicer/user/default");
        write(
            &orca.join("process/Fine.json"),
            r#"{"name":"Fine","inherits":"0.20 Standard @Snapmaker U1 (0.4 nozzle)","from":"User","layer_height":"0.1"}"#,
        );
        write(&orca.join("process/Fine.info"), "sync_info =\n");
        write(
            &orca.join("filament/base/My PLA.json"),
            r#"{"name":"My PLA","inherits":"Generic PLA @System"}"#,
        );
        write(&orca.join("machine/broken.json"), "{ not json");
        let snap = h.join(".config/Snapmaker_Orca/user/12345");
        write(
            &snap.join("machine/U1 mine.json"),
            r#"{"name":"U1 mine","inherits":"Snapmaker U1 (0.4 nozzle)"}"#,
        );
        let prusa = h.join(".config/PrusaSlicer");
        write(&prusa.join("PrusaSlicer.ini"), "[presets]\n");
        write(
            &prusa.join("filament/Tweaked.ini"),
            "inherits = Base PLA\ntemperature = 210\n",
        );
        let dirs = source_dirs(&h, None);
        assert_eq!(
            dirs.iter().map(|d| d.source).collect::<Vec<_>>(),
            [
                Source::PrusaSlicer,
                Source::OrcaSlicer,
                Source::SnapmakerOrca
            ]
        );
        let (found, errors) = scan(&dirs);
        assert_eq!(errors.len(), 1, "{errors:?}");
        let names: Vec<(Source, Kind, &str, &str)> = found
            .iter()
            .map(|f| (f.source, f.kind, f.name.as_str(), f.inherits.as_str()))
            .collect();
        assert_eq!(
            names,
            [
                (Source::PrusaSlicer, Kind::Filament, "Tweaked", "Base PLA"),
                (
                    Source::OrcaSlicer,
                    Kind::Print,
                    "Fine",
                    "0.20 Standard @Snapmaker U1 (0.4 nozzle)"
                ),
                (
                    Source::OrcaSlicer,
                    Kind::Filament,
                    "My PLA",
                    "Generic PLA @System"
                ),
                (
                    Source::SnapmakerOrca,
                    Kind::Printer,
                    "U1 mine",
                    "Snapmaker U1 (0.4 nozzle)"
                ),
            ]
        );
        // An empty home has nothing.
        assert!(source_dirs(&home("empty"), None).is_empty());
    }

    #[test]
    fn status_names_and_counts() {
        let h = home("status");
        let file = h.join("process/Fine.json");
        write(
            &file,
            r#"{"name":"Fine","inherits":"P","layer_height":"0.1"}"#,
        );
        let dirs = [SourceDir {
            source: Source::OrcaSlicer,
            root: h.clone(),
        }];
        let (found, _) = scan(&dirs);
        let f = &found[0];
        let p = orca_profile(f, "Snapmaker", "2.0", "Fine", 7).unwrap();
        assert_eq!(p.orca.as_ref().unwrap()["layer_height"], "0.1");
        assert_eq!(p.source(), Some(Source::OrcaSlicer));
        let profiles = vec![p];
        assert_eq!(status(f, &profiles), Status::Imported("Fine".into()));
        write(
            &file,
            r#"{"name":"Fine","inherits":"P","layer_height":"0.12"}"#,
        );
        let (found, _) = scan(&dirs);
        assert_eq!(status(&found[0], &profiles), Status::Changed("Fine".into()));
        assert_eq!(status(&found[0], &[]), Status::New);
        let taken = |n: &str| n == "Fine" || n == "Fine (Orca)";
        assert_eq!(
            unique_name("Fine", Source::OrcaSlicer, taken),
            "Fine (Orca 2)"
        );
        assert_eq!(unique_name("Other", Source::OrcaSlicer, taken), "Other");
        assert_eq!(
            count_phrase(12, 30, 8),
            "12 printer, 30 filament and 8 print profiles"
        );
        assert_eq!(count_phrase(0, 1, 0), "1 filament profile");
        assert_eq!(count_phrase(1, 0, 1), "1 printer and 1 print profiles");
    }

    #[test]
    fn duplicates_ignore_names_and_bookkeeping() {
        let h = home("dupes");
        let a = h.join("a/process/Fine.json");
        let b = h.join("b/process/Fine copy.json");
        write(
            &a,
            r#"{"name":"Fine","inherits":"P","layer_height":"0.1","version":"2.3"}"#,
        );
        write(
            &b,
            r#"{"name":"Fine copy","inherits":"P","layer_height":"0.1","version":"2.4","print_settings_id":"Fine copy"}"#,
        );
        let (found, _) = scan(&[
            SourceDir {
                source: Source::OrcaSlicer,
                root: h.join("a"),
            },
            SourceDir {
                source: Source::SnapmakerOrca,
                root: h.join("b"),
            },
        ]);
        let profiles: Vec<UserProfile> = found
            .iter()
            .map(|f| orca_profile(f, "Snapmaker", "1", &f.name, 1).unwrap())
            .collect();
        assert_eq!(duplicates(&profiles), vec![vec![0, 1]]);
        let mut changed = profiles.clone();
        changed[1]
            .settings
            .insert("layer_height".into(), "0.2".into());
        assert!(duplicates(&changed).is_empty());
    }
}
