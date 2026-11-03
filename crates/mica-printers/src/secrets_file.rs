// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Printer secrets in a file of Mica's settings folder, for Linux and other Unix systems
//! without the macOS Keychain (the AppImage, the Docker image whose settings folder is the
//! mounted `/config`). `secrets.toml` sits next to `printers.toml` and maps each reference
//! `printers.toml` stores to its secret, so a `printers.toml` copied to another machine works
//! the same as with the Keychain: it just lacks the secrets.
//!
//! The file is only as private as its permissions, so Mica keeps them strict and checks them:
//! - it is created with mode 0600 (a temporary file in the same folder, made 0600 before the
//!   secret is written, then renamed over the old file, so a crash never leaves half a file);
//! - a settings folder Mica has to create is created 0700 (an existing folder is left as is:
//!   it also holds `mica.toml` and the user's profiles);
//! - a file that other users can read or write, or that belongs to another user, is refused
//!   with the `chmod` / `chown` that fixes it. Mica never repairs it silently: a secrets file
//!   that was readable by others may have been read, and its owner should know.

use crate::secrets::SecretStore;
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

pub const FILE_NAME: &str = "secrets.toml";

const HEADER: &str = "# Mica Slicer printer secrets (API keys and passwords), keyed by the references in\n\
# printers.toml. Only its owner may read this file (chmod 600); Mica refuses it otherwise.\n";

#[derive(serde::Serialize, serde::Deserialize, Default)]
struct File {
    #[serde(default)]
    secrets: BTreeMap<String, String>,
}

/// `secrets.toml` in a settings folder.
pub struct FileStore {
    path: PathBuf,
    /// One read-modify-write at a time within this process.
    lock: Mutex<()>,
}

impl FileStore {
    /// The store of the settings folder `dir` (the file is created on the first save).
    pub fn new(dir: &Path) -> Self {
        Self {
            path: dir.join(FILE_NAME),
            lock: Mutex::new(()),
        }
    }

    /// All secrets; a missing file is an empty map.
    fn read(&self) -> Result<BTreeMap<String, String>, String> {
        let file = match fs::File::open(&self.path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
            Err(e) => return Err(format!("{}: {e}", self.path.display())),
        };
        // The checks look at the opened file itself, not at the path again.
        let meta = file
            .metadata()
            .map_err(|e| format!("{}: {e}", self.path.display()))?;
        check(&self.path, &meta, rustix::process::geteuid().as_raw())?;
        let text =
            std::io::read_to_string(file).map_err(|e| format!("{}: {e}", self.path.display()))?;
        // The parser's message is not echoed: it can quote a secret.
        toml::from_str::<File>(&text)
            .map(|f| f.secrets)
            .map_err(|_| {
                format!(
                    "{} is not valid (expected a [secrets] table of reference = \"secret\" lines)",
                    self.path.display()
                )
            })
    }

    /// Replaces the file with `secrets`, atomically and never readable by others.
    fn write(&self, secrets: BTreeMap<String, String>) -> Result<(), String> {
        let dir = self.path.parent().unwrap_or(Path::new("."));
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
            .map_err(|e| format!("{}: {e}", dir.display()))?;
        let text = format!(
            "{HEADER}{}",
            toml::to_string(&File { secrets }).map_err(|e| e.to_string())?
        );
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let tmp = dir.join(format!(
            ".{FILE_NAME}.{}.{}.tmp",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let result = (|| {
            // Created 0600 (the umask can only take bits away); set again explicitly before
            // anything secret goes in, in case the file system ignores the creation mode.
            let mut f = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&tmp)?;
            f.set_permissions(fs::Permissions::from_mode(0o600))?;
            f.write_all(text.as_bytes())?;
            f.sync_all()?;
            drop(f);
            fs::rename(&tmp, &self.path)?;
            // Make the rename durable; not every file system can sync a folder.
            if let Ok(d) = fs::File::open(dir) {
                let _ = d.sync_all();
            }
            Ok::<(), std::io::Error>(())
        })();
        result.map_err(|e| {
            let _ = fs::remove_file(&tmp);
            format!("{}: {e}", self.path.display())
        })
    }
}

/// Refuses a secrets file that is not a plain file of `uid` with no group or other access.
fn check(path: &Path, meta: &fs::Metadata, uid: u32) -> Result<(), String> {
    let shown = path.display();
    if !meta.is_file() {
        return Err(format!(
            "{shown} is not a file; Mica keeps printer secrets there"
        ));
    }
    if meta.uid() != uid {
        return Err(format!(
            "{shown} belongs to another user, so Mica will not use its printer secrets. \
             If it is yours, run: chown \"$(id -u)\" '{shown}' && chmod 600 '{shown}'"
        ));
    }
    if meta.mode() & 0o077 != 0 {
        return Err(format!(
            "{shown} can be read or changed by other users, so Mica will not use its printer \
             secrets. Fix it with: chmod 600 '{shown}' (and consider changing the keys in it)"
        ));
    }
    Ok(())
}

impl SecretStore for FileStore {
    fn label(&self) -> &'static str {
        "secrets file in Mica's settings folder that only you can read"
    }
    fn short_label(&self) -> &'static str {
        "secrets file"
    }
    fn get(&self, reference: &str) -> Result<Option<String>, String> {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        Ok(self.read()?.remove(reference))
    }
    fn set(&self, reference: &str, secret: &str) -> Result<(), String> {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut all = self.read()?;
        if all.get(reference).map(String::as_str) == Some(secret) {
            return Ok(());
        }
        all.insert(reference.to_string(), secret.to_string());
        self.write(all)
    }
    fn delete(&self, reference: &str) -> Result<(), String> {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut all = self.read()?;
        if all.remove(reference).is_none() {
            return Ok(());
        }
        self.write(all)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("mica-secrets-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn mode(p: &Path) -> u32 {
        fs::metadata(p).unwrap().mode() & 0o777
    }

    #[test]
    fn round_trips_in_an_owner_only_file() {
        let dir = temp_dir("round-trip").join("settings");
        let store = FileStore::new(&dir);
        // Nothing saved yet: no file, no error.
        assert_eq!(store.get("a").unwrap(), None);
        store.delete("a").unwrap();
        assert!(!dir.exists());

        store.set("a", "s3cret").unwrap();
        store.set("b", "with \"quotes\" and = signs").unwrap();
        assert_eq!(mode(&dir), 0o700, "a folder Mica creates is private");
        let file = dir.join(FILE_NAME);
        assert_eq!(mode(&file), 0o600);
        assert_eq!(store.get("a").unwrap().as_deref(), Some("s3cret"));
        assert_eq!(
            FileStore::new(&dir).get("b").unwrap().as_deref(),
            Some("with \"quotes\" and = signs")
        );
        store.set("a", "changed").unwrap();
        assert_eq!(store.get("a").unwrap().as_deref(), Some("changed"));
        store.delete("a").unwrap();
        store.delete("a").unwrap();
        assert_eq!(store.get("a").unwrap(), None);
        assert_eq!(mode(&file), 0o600, "rewrites stay private");
        // No temporary files are left behind.
        let names: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(names, [FILE_NAME]);
        fs::remove_dir_all(dir.parent().unwrap()).unwrap();
    }

    #[test]
    fn an_existing_folder_keeps_its_mode() {
        let dir = temp_dir("existing");
        fs::create_dir_all(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        FileStore::new(&dir).set("a", "x").unwrap();
        assert_eq!(mode(&dir), 0o755);
        assert_eq!(mode(&dir.join(FILE_NAME)), 0o600);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_file_others_can_read_is_refused_with_the_fix() {
        let dir = temp_dir("open");
        let store = FileStore::new(&dir);
        store.set("a", "s3cret").unwrap();
        let file = dir.join(FILE_NAME);
        for bits in [0o644, 0o640, 0o604, 0o620] {
            fs::set_permissions(&file, fs::Permissions::from_mode(bits)).unwrap();
            for e in [
                store.get("a").unwrap_err(),
                store.set("a", "new").unwrap_err(),
                store.delete("a").unwrap_err(),
            ] {
                assert!(e.contains("chmod 600"), "{e}");
                assert!(!e.contains("s3cret"), "{e}");
            }
            assert_eq!(mode(&file), bits, "Mica does not repair it silently");
        }
        fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(store.get("a").unwrap().as_deref(), Some("s3cret"));
        // Owner read-only is fine too.
        fs::set_permissions(&file, fs::Permissions::from_mode(0o400)).unwrap();
        assert_eq!(store.get("a").unwrap().as_deref(), Some("s3cret"));
        fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn another_users_file_is_refused() {
        let dir = temp_dir("owner");
        FileStore::new(&dir).set("a", "x").unwrap();
        let file = dir.join(FILE_NAME);
        let meta = fs::metadata(&file).unwrap();
        check(&file, &meta, meta.uid()).unwrap();
        let e = check(&file, &meta, meta.uid().wrapping_add(1)).unwrap_err();
        assert!(e.contains("another user") && e.contains("chown"), "{e}");
        let e = check(&dir, &fs::metadata(&dir).unwrap(), meta.uid()).unwrap_err();
        assert!(e.contains("not a file"), "{e}");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_broken_file_is_reported_without_its_contents() {
        let dir = temp_dir("broken");
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join(FILE_NAME);
        fs::write(&file, "secrets = \"hunter2\" oops").unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
        let e = FileStore::new(&dir).get("a").unwrap_err();
        assert!(e.contains("not valid") && !e.contains("hunter2"), "{e}");
        fs::remove_dir_all(&dir).unwrap();
    }
}
