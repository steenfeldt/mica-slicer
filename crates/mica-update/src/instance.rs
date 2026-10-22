// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Which Mica processes run from an installation, so an update never swaps an app another
//! Mica process is running from.
//!
//! Every Mica window process holds a shared advisory lock on its own executable for as long
//! as it runs ([`hold`]). Installing an update needs the exclusive lock ([`exclusive`]): it is
//! only granted when no other process holds the shared one, and while it is held no second
//! process can install. The executable is only opened for reading; nothing is written to it.

use std::fs::File;
use std::path::Path;
use std::sync::Mutex;

/// What installing says while another Mica runs from the same installation.
pub const OTHER_RUNNING: &str =
    "Another Mica window is open. Close it, then install the update from this one.";

/// This process's shared lock on its executable (see [`hold`]).
static HELD: Mutex<Option<File>> = Mutex::new(None);

fn held() -> std::sync::MutexGuard<'static, Option<File>> {
    HELD.lock().unwrap_or_else(|e| e.into_inner())
}

/// Marks this process as running from `exe` until it exits. Called once at start by the
/// window process; without it (tests, the command-line tool) this process does not count.
pub fn hold(exe: &Path) {
    let Ok(file) = File::open(exe) else { return };
    if file.lock_shared().is_ok() {
        *held() = Some(file);
    }
}

/// While held, no other Mica process runs from the installation and none can install.
/// Dropping it returns to the shared lock [`hold`] took.
pub struct Exclusive {
    file: Option<File>,
    /// The file is this process's own [`hold`] lock (given back on drop).
    own: bool,
}

impl Drop for Exclusive {
    fn drop(&mut self) {
        if let Some(file) = self.file.take() {
            if self.own {
                let _ = file.lock_shared();
                *held() = Some(file);
            } else {
                let _ = file.unlock();
            }
        }
    }
}

/// The exclusive lock on `exe` (the executable of the installation being replaced), or
/// [`OTHER_RUNNING`] when another process runs from it or is installing. Never waits.
pub fn exclusive(exe: &Path) -> Result<Exclusive, String> {
    let own = held().take();
    let (file, is_own) = match own {
        Some(f) => (f, true),
        None => match File::open(exe) {
            Ok(f) => (f, false),
            // No executable to lock (an unusual layout): nothing to protect.
            Err(_) => {
                return Ok(Exclusive {
                    file: None,
                    own: false,
                });
            }
        },
    };
    match file.try_lock() {
        Ok(()) => Ok(Exclusive {
            file: Some(file),
            own: is_own,
        }),
        Err(std::fs::TryLockError::WouldBlock) => {
            if is_own {
                // Changing the lock may have let go of the shared one: take it again.
                let _ = file.lock_shared();
                *held() = Some(file);
            }
            Err(OTHER_RUNNING.into())
        }
        // A file system without locks: nothing can be told apart, go ahead as before.
        Err(std::fs::TryLockError::Error(_)) => {
            if is_own {
                *held() = Some(file);
            }
            Ok(Exclusive {
                file: None,
                own: false,
            })
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};

    /// Run as a child process by the test below: holds `MICA_TEST_HOLD` like a running Mica
    /// until its input closes.
    #[test]
    fn holder_process() {
        let Some(path) = std::env::var_os("MICA_TEST_HOLD") else {
            return;
        };
        hold(Path::new(&path));
        assert!(held().is_some());
        println!("held");
        let mut line = String::new();
        let _ = std::io::stdin().read_line(&mut line);
    }

    #[test]
    fn a_second_running_process_blocks_installing_until_it_quits() {
        let dir = std::env::temp_dir().join(format!("mica-instance-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("mica-gui");
        std::fs::write(&exe, b"not really a program").unwrap();
        // Alone: installing may go ahead, and a second installer meanwhile may not.
        let first = exclusive(&exe).unwrap();
        let other = File::open(&exe).unwrap();
        assert!(matches!(
            other.try_lock_shared(),
            Err(std::fs::TryLockError::WouldBlock)
        ));
        assert_eq!(exclusive(&exe).err().as_deref(), Some(OTHER_RUNNING));
        drop(first);
        drop(other);
        // Another process running from the same executable blocks installing.
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "instance::tests::holder_process", "--nocapture"])
            .env("MICA_TEST_HOLD", &exe)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut out = BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        while !line.contains("held") {
            line.clear();
            assert!(out.read_line(&mut line).unwrap() > 0, "holder ended early");
        }
        assert_eq!(exclusive(&exe).err().as_deref(), Some(OTHER_RUNNING));
        assert_eq!(exclusive(&exe).err().as_deref(), Some(OTHER_RUNNING));
        // It quits: installing may go ahead.
        drop(child.stdin.take());
        assert!(child.wait().unwrap().success());
        assert!(exclusive(&exe).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
