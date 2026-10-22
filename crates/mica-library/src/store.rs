// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The on-disk index: one `redb` file in the app's data folder. Values are small JSON
//! records, so the format stays readable and can grow fields without a migration.
//!
//! The scanner keeps the whole index in memory ([`Index`]) and writes changes in batches;
//! the database is only read once, when the library opens.

use crate::model::{ExtraRecord, FileRecord, Grouping, Info, UserData};
use redb::{
    Database, DatabaseError, ReadableDatabase, ReadableTable, StorageError, TableDefinition,
};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

type Table = TableDefinition<'static, &'static [u8], &'static [u8]>;
const META: Table = TableDefinition::new("meta");
const FOLDERS: Table = TableDefinition::new("folders");
const FILES: Table = TableDefinition::new("files");
const MODELS: Table = TableDefinition::new("models");
const USER: Table = TableDefinition::new("user");
/// Pictures and documents next to models (path → [`ExtraRecord`]).
const EXTRAS: Table = TableDefinition::new("extras");
/// The user's grouping choices (folder path → [`Grouping`]).
const GROUPING: Table = TableDefinition::new("grouping");

/// Bumped when what the extractor reads changes: files and models are then re-read (tags,
/// favourites and folders are kept).
pub const SCHEMA: &[u8] = b"1";

/// Everything the index holds, in memory.
#[derive(Clone, Debug, Default)]
pub struct Index {
    pub folders: Vec<PathBuf>,
    pub files: BTreeMap<PathBuf, FileRecord>,
    pub models: HashMap<String, Info>,
    pub user: HashMap<String, UserData>,
    pub extras: BTreeMap<PathBuf, ExtraRecord>,
    pub grouping: BTreeMap<PathBuf, Grouping>,
}

/// A batch of writes.
#[derive(Debug, Default)]
pub struct Changes {
    pub folders: Option<Vec<PathBuf>>,
    pub files: Vec<(PathBuf, Option<FileRecord>)>,
    pub models: Vec<(String, Option<Info>)>,
    pub user: Vec<(String, Option<UserData>)>,
    pub extras: Vec<(PathBuf, Option<ExtraRecord>)>,
    pub grouping: Vec<(PathBuf, Option<Grouping>)>,
}

impl Changes {
    pub fn is_empty(&self) -> bool {
        self.folders.is_none()
            && self.files.is_empty()
            && self.models.is_empty()
            && self.user.is_empty()
            && self.extras.is_empty()
            && self.grouping.is_empty()
    }
}

pub struct Store {
    db: Database,
}

/// What another Mica process holding the index means to the user: shown as an error only if
/// even a read-only view is impossible.
pub const IN_USE: &str = "The library is open in another Mica window";

/// Why [`Store::open`] failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpenError {
    /// Another process (or handle) has the index open: it is in use, not damaged, and is left
    /// exactly as it is.
    InUse,
    /// Anything else (permissions, I/O). The file is left alone too.
    Failed(String),
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OpenError::InUse => f.write_str(IN_USE),
            OpenError::Failed(e) => f.write_str(e),
        }
    }
}

impl From<String> for OpenError {
    fn from(e: String) -> Self {
        OpenError::Failed(e)
    }
}

/// Size and modification time of the index file (a cheap "did it change" check).
pub fn stamp(path: &Path) -> Option<(u64, std::time::SystemTime)> {
    let m = std::fs::metadata(path).ok()?;
    Some((m.len(), m.modified().ok()?))
}

fn err(e: impl std::fmt::Display) -> String {
    format!("library index: {e}")
}

fn damaged(e: &DatabaseError) -> bool {
    match e {
        DatabaseError::Storage(StorageError::Corrupted(_)) | DatabaseError::UpgradeRequired(_) => {
            true
        }
        // redb 4.4 uses InvalidData for a missing/invalid file magic number, after
        // acquiring the file lock. Other I/O errors are operational, not corruption.
        DatabaseError::Storage(StorageError::Io(e)) => e.kind() == std::io::ErrorKind::InvalidData,
        _ => false,
    }
}

#[cfg(unix)]
fn path_key(p: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    p.as_os_str().as_bytes().to_vec()
}
#[cfg(not(unix))]
fn path_key(p: &Path) -> Vec<u8> {
    p.to_string_lossy().as_bytes().to_vec()
}
#[cfg(unix)]
fn key_path(k: &[u8]) -> PathBuf {
    use std::os::unix::ffi::OsStrExt;
    PathBuf::from(std::ffi::OsStr::from_bytes(k))
}
#[cfg(not(unix))]
fn key_path(k: &[u8]) -> PathBuf {
    PathBuf::from(String::from_utf8_lossy(k).into_owned())
}

impl Store {
    /// Opens (or creates) the index file. Only corruption/format errors set the file aside
    /// (renamed, never deleted). Lock, permission and other operational errors preserve it.
    pub fn open(path: &Path) -> Result<(Store, Index, Option<String>), OpenError> {
        let mut warning = None;
        let db = match Database::create(path) {
            Ok(db) => db,
            Err(DatabaseError::DatabaseAlreadyOpen) => return Err(OpenError::InUse),
            // All other (including future) errors must leave the file alone.
            Err(e) if damaged(&e) => {
                let aside = path.with_extension(format!(
                    "redb.damaged-{}",
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_or(0, |d| d.as_secs())
                ));
                std::fs::rename(path, &aside).map_err(err)?;
                warning = Some(format!(
                    "The library index could not be read ({e}); started a new one and kept the old file as {}",
                    aside.display()
                ));
                Database::create(path).map_err(err)?
            }
            Err(e) => return Err(OpenError::Failed(err(e))),
        };
        let store = Store { db };
        let index = store.load()?;
        Ok((store, index, warning))
    }

    /// The last committed state of an index another process has open (and locked), read
    /// from a private copy: the file itself is only read, never locked, repaired, moved or
    /// changed. A missing file is an empty index. On macOS the copy is a clone of the file
    /// (instant, one consistent moment); a commit the other process was writing during the
    /// copy is dropped when the copy opens, as after a crash, leaving the commit before it.
    pub fn read_snapshot(path: &Path) -> Result<Index, String> {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        if !path.exists() {
            return Ok(Index::default());
        }
        let copy = std::env::temp_dir().join(format!(
            "mica-library-view-{}-{}.redb",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let result = (|| {
            std::fs::copy(path, &copy).map_err(err)?;
            let store = Store {
                db: Database::create(&copy).map_err(err)?,
            };
            store.load()
        })();
        let _ = std::fs::remove_file(&copy);
        result
    }

    fn load(&self) -> Result<Index, String> {
        // Create every table (and check the schema) in one write.
        let fresh_schema;
        {
            let tx = self.db.begin_write().map_err(err)?;
            {
                let mut meta = tx.open_table(META).map_err(err)?;
                let schema = meta
                    .get(b"schema".as_slice())
                    .map_err(err)?
                    .map(|v| v.value().to_vec());
                fresh_schema = schema.as_deref() != Some(SCHEMA);
                if fresh_schema {
                    meta.insert(b"schema".as_slice(), SCHEMA).map_err(err)?;
                }
            }
            if fresh_schema {
                tx.delete_table(FILES).map_err(err)?;
                tx.delete_table(MODELS).map_err(err)?;
                tx.delete_table(EXTRAS).map_err(err)?;
            }
            for t in [FOLDERS, FILES, MODELS, USER, EXTRAS, GROUPING] {
                tx.open_table(t).map_err(err)?;
            }
            tx.commit().map_err(err)?;
        }
        let tx = self.db.begin_read().map_err(err)?;
        let mut index = Index::default();
        for row in tx.open_table(FOLDERS).map_err(err)?.iter().map_err(err)? {
            let (k, _) = row.map_err(err)?;
            index.folders.push(key_path(k.value()));
        }
        for row in tx.open_table(FILES).map_err(err)?.iter().map_err(err)? {
            let (k, v) = row.map_err(err)?;
            if let Ok(rec) = serde_json::from_slice::<FileRecord>(v.value()) {
                index.files.insert(key_path(k.value()), rec);
            }
        }
        for row in tx.open_table(MODELS).map_err(err)?.iter().map_err(err)? {
            let (k, v) = row.map_err(err)?;
            if let Ok(info) = serde_json::from_slice::<Info>(v.value()) {
                index
                    .models
                    .insert(String::from_utf8_lossy(k.value()).into_owned(), info);
            }
        }
        for row in tx.open_table(USER).map_err(err)?.iter().map_err(err)? {
            let (k, v) = row.map_err(err)?;
            if let Ok(user) = serde_json::from_slice::<UserData>(v.value()) {
                index
                    .user
                    .insert(String::from_utf8_lossy(k.value()).into_owned(), user);
            }
        }
        for row in tx.open_table(EXTRAS).map_err(err)?.iter().map_err(err)? {
            let (k, v) = row.map_err(err)?;
            if let Ok(rec) = serde_json::from_slice::<ExtraRecord>(v.value()) {
                index.extras.insert(key_path(k.value()), rec);
            }
        }
        for row in tx.open_table(GROUPING).map_err(err)?.iter().map_err(err)? {
            let (k, v) = row.map_err(err)?;
            if let Ok(g) = serde_json::from_slice::<Grouping>(v.value()) {
                index.grouping.insert(key_path(k.value()), g);
            }
        }
        Ok(index)
    }

    /// Writes one batch in one transaction.
    pub fn write(&self, changes: &Changes) -> Result<(), String> {
        if changes.is_empty() {
            return Ok(());
        }
        let tx = self.db.begin_write().map_err(err)?;
        {
            if let Some(folders) = &changes.folders {
                let mut t = tx.open_table(FOLDERS).map_err(err)?;
                t.retain(|_, _| false).map_err(err)?;
                for f in folders {
                    t.insert(path_key(f).as_slice(), b"".as_slice())
                        .map_err(err)?;
                }
            }
            let mut files = tx.open_table(FILES).map_err(err)?;
            for (p, rec) in &changes.files {
                let k = path_key(p);
                match rec {
                    Some(r) => {
                        let v = serde_json::to_vec(r).map_err(err)?;
                        files.insert(k.as_slice(), v.as_slice()).map_err(err)?;
                    }
                    None => {
                        files.remove(k.as_slice()).map_err(err)?;
                    }
                }
            }
            let mut models = tx.open_table(MODELS).map_err(err)?;
            for (h, info) in &changes.models {
                match info {
                    Some(i) => {
                        let v = serde_json::to_vec(i).map_err(err)?;
                        models.insert(h.as_bytes(), v.as_slice()).map_err(err)?;
                    }
                    None => {
                        models.remove(h.as_bytes()).map_err(err)?;
                    }
                }
            }
            let mut user = tx.open_table(USER).map_err(err)?;
            for (h, u) in &changes.user {
                match u {
                    Some(u) if !u.is_empty() => {
                        let v = serde_json::to_vec(u).map_err(err)?;
                        user.insert(h.as_bytes(), v.as_slice()).map_err(err)?;
                    }
                    _ => {
                        user.remove(h.as_bytes()).map_err(err)?;
                    }
                }
            }
            let mut extras = tx.open_table(EXTRAS).map_err(err)?;
            for (p, rec) in &changes.extras {
                let k = path_key(p);
                match rec {
                    Some(r) => {
                        let v = serde_json::to_vec(r).map_err(err)?;
                        extras.insert(k.as_slice(), v.as_slice()).map_err(err)?;
                    }
                    None => {
                        extras.remove(k.as_slice()).map_err(err)?;
                    }
                }
            }
            let mut grouping = tx.open_table(GROUPING).map_err(err)?;
            for (p, g) in &changes.grouping {
                let k = path_key(p);
                match g {
                    Some(g) => {
                        let v = serde_json::to_vec(g).map_err(err)?;
                        grouping.insert(k.as_slice(), v.as_slice()).map_err(err)?;
                    }
                    None => {
                        grouping.remove(k.as_slice()).map_err(err)?;
                    }
                }
            }
        }
        tx.commit().map_err(err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Kind;

    #[test]
    fn only_corruption_and_format_errors_are_damage() {
        use std::io::{Error, ErrorKind};
        assert!(damaged(&DatabaseError::UpgradeRequired(1)));
        assert!(damaged(&DatabaseError::Storage(StorageError::Corrupted(
            "bad checksum".into()
        ))));
        assert!(damaged(&DatabaseError::Storage(StorageError::Io(
            Error::from(ErrorKind::InvalidData)
        ))));
        for kind in [
            ErrorKind::PermissionDenied,
            ErrorKind::WouldBlock,
            ErrorKind::ResourceBusy,
            ErrorKind::InvalidInput,
            ErrorKind::UnexpectedEof,
            ErrorKind::Other,
        ] {
            assert!(!damaged(&DatabaseError::Storage(StorageError::Io(
                Error::from(kind)
            ))));
        }
        for error in [
            DatabaseError::DatabaseAlreadyOpen,
            DatabaseError::RepairAborted,
            DatabaseError::TransactionInProgress,
            DatabaseError::Storage(StorageError::PreviousIo),
            DatabaseError::Storage(StorageError::Unsupported),
        ] {
            assert!(!damaged(&error));
        }
    }

    #[test]
    fn round_trip_and_schema() {
        let dir = crate::testkit::TempDir::new("store");
        let path = dir.path().join("index.redb");
        {
            let (store, index, warning) = Store::open(&path).unwrap();
            assert!(index.files.is_empty() && warning.is_none());
            let rec = FileRecord {
                root: "/r".into(),
                size: 3,
                mtime_ns: 7,
                hash: "ab".into(),
                kind: Kind::Stl,
            };
            store
                .write(&Changes {
                    folders: Some(vec!["/r".into()]),
                    files: vec![("/r/a.stl".into(), Some(rec))],
                    models: vec![("ab".into(), Some(Info::default()))],
                    user: vec![(
                        "ab".into(),
                        Some(UserData {
                            tags: vec!["x".into()],
                            favourite: true,
                        }),
                    )],
                    extras: vec![(
                        "/r/cover.png".into(),
                        Some(ExtraRecord {
                            root: "/r".into(),
                            size: 9,
                            mtime_ns: 1,
                            kind: crate::model::ExtraKind::Image,
                            thumbnail: Some("x-1.png".into()),
                        }),
                    )],
                    grouping: vec![("/r/dragon".into(), Some(Grouping::Split))],
                })
                .unwrap();
        }
        let (_, index, _) = Store::open(&path).unwrap();
        assert_eq!(index.grouping[Path::new("/r/dragon")], Grouping::Split);
        assert_eq!(
            index.extras[Path::new("/r/cover.png")].thumbnail.as_deref(),
            Some("x-1.png")
        );
        assert_eq!(index.folders, vec![PathBuf::from("/r")]);
        assert_eq!(index.files.len(), 1);
        assert!(index.models.contains_key("ab"));
        assert!(index.user["ab"].favourite);
    }

    #[test]
    fn open_index_is_never_set_aside() {
        let dir = crate::testkit::TempDir::new("store-in-use");
        let path = dir.path().join("index.redb");
        let (store, _, _) = Store::open(&path).unwrap();
        store
            .write(&Changes {
                folders: Some(vec!["/r".into()]),
                files: vec![(
                    "/r/a.stl".into(),
                    Some(FileRecord {
                        root: "/r".into(),
                        size: 3,
                        mtime_ns: 7,
                        hash: "ab".into(),
                        kind: Kind::Stl,
                    }),
                )],
                models: vec![("ab".into(), Some(Info::default()))],
                user: vec![(
                    "ab".into(),
                    Some(UserData {
                        tags: vec!["Keep me".into()],
                        favourite: true,
                    }),
                )],
                ..Default::default()
            })
            .unwrap();
        // Verify redb's same-process locking behaves like a second Mica process.
        assert!(matches!(
            Database::create(&path),
            Err(DatabaseError::DatabaseAlreadyOpen)
        ));
        let before = std::fs::read(&path).unwrap();
        for _ in 0..2 {
            let error = Store::open(&path).err().expect("second open must fail");
            assert_eq!(error, OpenError::InUse);
            assert_eq!(
                error.to_string(),
                "The library is open in another Mica window"
            );
            assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
            assert_eq!(std::fs::read(&path).unwrap(), before);
        }
        let check = |index: Index| {
            assert_eq!(index.folders, vec![PathBuf::from("/r")]);
            assert_eq!(index.files[Path::new("/r/a.stl")].hash, "ab");
            assert!(index.models.contains_key("ab"));
            assert_eq!(index.user["ab"].tags, vec!["Keep me"]);
            assert!(index.user["ab"].favourite);
        };
        check(store.load().unwrap());
        // The first handle can still write, and a retry after it closes loads all data.
        store
            .write(&Changes {
                folders: Some(vec!["/r".into()]),
                ..Default::default()
            })
            .unwrap();
        drop(store);
        let (_, index, warning) = Store::open(&path).unwrap();
        assert!(warning.is_none());
        check(index);
    }

    #[cfg(unix)]
    #[test]
    fn permission_error_preserves_index() {
        let dir = crate::testkit::TempDir::new("store-permission");
        let path = dir.path().join("index.redb");
        drop(Store::open(&path).unwrap());
        let before = std::fs::read(&path).unwrap();
        let permissions = std::fs::metadata(&path).unwrap().permissions();
        let mut readonly = permissions.clone();
        readonly.set_readonly(true);
        std::fs::set_permissions(&path, readonly).unwrap();
        let result = Store::open(&path);
        std::fs::set_permissions(&path, permissions).unwrap();
        assert!(
            result.is_err(),
            "read-only index must fail to open for writing"
        );
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    #[test]
    fn damaged_index_is_set_aside() {
        let dir = crate::testkit::TempDir::new("store-damaged");
        let path = dir.path().join("index.redb");
        let garbage = b"this is not a database at all, not even close";
        std::fs::write(&path, garbage).unwrap();
        let (store, index, warning) = Store::open(&path).unwrap();
        assert!(index.files.is_empty());
        assert!(warning.unwrap().contains("started a new one"));
        let kept = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .find(|e| e.file_name().to_string_lossy().contains("damaged"))
            .expect("damaged file retained");
        assert_eq!(std::fs::read(kept.path()).unwrap(), garbage);
        drop(store);
        assert!(Store::open(&path).unwrap().2.is_none());
    }
}
