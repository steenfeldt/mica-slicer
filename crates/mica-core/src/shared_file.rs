// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Files in Mica's settings folder that more than one Mica process (or window) may write.
//!
//! - [`write`] puts a file in place atomically: a uniquely named temporary file next to it
//!   (process id and a per-process counter, so neither two processes nor two threads share
//!   one), flushed to disk, then renamed over. Readers see the old file or the new one, never
//!   half of one.
//! - [`locked`] runs a read-modify-write under an exclusive advisory lock on `<file>.lock`,
//!   so two writers that each re-read the file before writing it cannot lose each other's
//!   changes. The lock file is never removed (removing it would let two writers lock two
//!   different files).

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// A temporary name next to `path`, unique to this process and call.
pub fn temp_path(path: &Path) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    path.with_file_name(format!(
        ".{name}.tmp-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}

/// Writes `bytes` to `path` atomically (creating the folder). On failure the temporary file is
/// removed and `path` is unchanged.
pub fn write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = temp_path(path);
    let result = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        io::Write::write_all(&mut f, bytes)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// [`write`] with the error as text naming the file (for messages).
pub fn write_text(path: &Path, text: &str) -> Result<(), String> {
    write(path, text.as_bytes()).map_err(|e| format!("{}: {e}", path.display()))
}

/// The lock file guarding `path`.
pub fn lock_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    path.with_file_name(format!("{name}.lock"))
}

/// Runs `f` while holding the exclusive lock of `path` (waiting for other holders). A lock that
/// cannot be taken (a file system without locks) still runs `f`: the write stays atomic, only
/// the protection against a simultaneous writer is lost.
pub fn locked<T>(path: &Path, f: impl FnOnce() -> T) -> T {
    let lock = lock_path(path);
    if let Some(dir) = lock.parent().filter(|d| !d.as_os_str().is_empty()) {
        let _ = std::fs::create_dir_all(dir);
    }
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock)
        .ok()
        .filter(|file| file.lock().is_ok());
    let out = f();
    if let Some(file) = file {
        let _ = file.unlock();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("mica-shared-file-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn concurrent_writers_never_leave_a_mixed_or_temporary_file() {
        let dir = temp("write");
        let path = dir.join("cache.bin");
        let payloads: Vec<Vec<u8>> = (0..8u8).map(|i| vec![i; 256 * 1024]).collect();
        std::thread::scope(|s| {
            for p in &payloads {
                let path = &path;
                s.spawn(move || {
                    for _ in 0..10 {
                        write(path, p).unwrap();
                        let got = std::fs::read(path).unwrap();
                        assert!(got.len() == p.len() && got.iter().all(|b| *b == got[0]));
                    }
                });
            }
        });
        let names: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["cache.bin".to_string()]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn locked_read_modify_write_loses_no_update() {
        let dir = temp("lock");
        let path = dir.join("count.txt");
        std::thread::scope(|s| {
            for _ in 0..8 {
                let path = &path;
                s.spawn(move || {
                    for _ in 0..25 {
                        locked(path, || {
                            let n: u64 = std::fs::read_to_string(path)
                                .ok()
                                .and_then(|t| t.trim().parse().ok())
                                .unwrap_or(0);
                            write(path, format!("{}", n + 1).as_bytes()).unwrap();
                        });
                    }
                });
            }
        });
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "200");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
