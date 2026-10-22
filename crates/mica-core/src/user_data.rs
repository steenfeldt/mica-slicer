// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Where Mica finds the user's own data — its settings folder (preferences, profiles,
//! printers, library), crash logs, and other apps' data it reads (PrusaSlicer's and
//! OrcaSlicer's settings folders, vendor bundles, user presets) — and the guard that keeps
//! tests and test tools away from all of it.
//!
//! An **isolated** process never sees the real data: [`home`] is `None` (so nothing under the
//! home folder is found: PrusaSlicer's vendor bundle falls back to the pinned upstream copy,
//! no PrusaSlicer or OrcaSlicer user presets are imported, no last-used filament is
//! detected), [`config_dir`] is `MICA_CONFIG_DIR` or else a fresh folder of this process in
//! the temporary directory, and crash logs go to `MICA_CRASH_LOG_DIR` or the temporary
//! directory. A process is isolated when it is a Cargo test executable (every unit and
//! integration test, whatever `HOME` is), when `MICA_ISOLATED` is set (test harnesses pass
//! it to the Mica binaries they start), or after [`isolate`] (the screenshot tools and the
//! monkey test). Pointing `MICA_CONFIG_DIR` of an isolated process at the real settings
//! folder panics.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

static ISOLATED: AtomicBool = AtomicBool::new(false);

/// The environment variable that isolates a Mica process (any non-empty value).
pub const ISOLATED_VAR: &str = "MICA_ISOLATED";

/// From now on this process reads and writes none of the user's real data (see the module
/// documentation). Children it starts should get [`ISOLATED_VAR`] too.
pub fn isolate() {
    ISOLATED.store(true, Ordering::SeqCst);
}

/// Whether this process is kept away from the user's real data.
pub fn isolated() -> bool {
    static TEST_EXE: OnceLock<bool> = OnceLock::new();
    ISOLATED.load(Ordering::SeqCst)
        || std::env::var_os(ISOLATED_VAR).is_some_and(|v| !v.is_empty())
        || *TEST_EXE.get_or_init(|| std::env::current_exe().is_ok_and(|p| is_test_exe(&p)))
}

/// Cargo puts test (and bench) executables in `target/<profile>/deps`, the binaries one level
/// up: `target/debug/deps/mica_gui-0123abcd` is a test, `target/debug/mica-gui` is not.
fn is_test_exe(path: &Path) -> bool {
    path.parent()
        .and_then(Path::file_name)
        .is_some_and(|d| d == "deps")
}

/// The user's home folder; `None` in an isolated process.
pub fn home() -> Option<PathBuf> {
    if isolated() {
        return None;
    }
    real_home()
}

/// The home folder from the environment, isolated or not: `HOME`, and on Windows (where
/// `HOME` is normally not set) `USERPROFILE` first, as `std::env::home_dir` does there.
fn real_home() -> Option<PathBuf> {
    home_from(cfg!(windows), |name| std::env::var_os(name))
}

fn home_from(windows: bool, var: impl Fn(&str) -> Option<std::ffi::OsString>) -> Option<PathBuf> {
    let names: &[&str] = if windows {
        &["USERPROFILE", "HOME"]
    } else {
        &["HOME"]
    };
    names
        .iter()
        .find_map(|n| var(n).filter(|h| !h.is_empty()))
        .map(PathBuf::from)
}

/// A folder an environment variable names (`APPDATA`, `XDG_DATA_HOME`, …); `None` in an
/// isolated process.
pub fn var_dir(name: &str) -> Option<PathBuf> {
    if isolated() {
        return None;
    }
    std::env::var_os(name)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// The platform's folder for Mica's settings under `home`.
fn default_config_dir(home: &Path) -> PathBuf {
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support/Mica Slicer")
    } else if cfg!(windows) {
        std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.to_path_buf())
            .join("Mica Slicer")
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"))
            .join("mica-slicer")
    }
}

/// Mica's settings folder: `MICA_CONFIG_DIR` if set, else the platform's application-support
/// folder (`~/Library/Application Support/Mica Slicer` on macOS); in an isolated process,
/// else a folder of this process in the temporary directory.
///
/// # Panics
/// In an isolated process whose `MICA_CONFIG_DIR` is the real settings folder.
pub fn config_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("MICA_CONFIG_DIR").filter(|d| !d.is_empty()) {
        let dir = PathBuf::from(dir);
        if isolated() {
            guard(&dir);
        }
        return dir;
    }
    if isolated() {
        static TEMP: OnceLock<PathBuf> = OnceLock::new();
        return TEMP
            .get_or_init(|| {
                let dir = std::env::temp_dir()
                    .join(format!("mica-isolated-config-{}", std::process::id()));
                let _ = std::fs::remove_dir_all(&dir);
                dir
            })
            .clone();
    }
    default_config_dir(&home().unwrap_or_default())
}

/// Panics when `dir` is the real settings folder (tests must never open it).
fn guard(dir: &Path) {
    let Some(real_home) = real_home() else {
        return;
    };
    let real = default_config_dir(&real_home);
    let same = |a: &Path, b: &Path| match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    };
    if same(dir, &real) {
        panic!(
            "a test (or test tool) opened the real Mica settings folder {}: set MICA_CONFIG_DIR to a temporary folder",
            real.display()
        );
    }
}

/// Where crash logs go: `MICA_CRASH_LOG_DIR`, else the platform's log folder (the temporary
/// directory in an isolated process).
pub fn crash_log_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("MICA_CRASH_LOG_DIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(dir);
    }
    let Some(home) = home() else {
        return std::env::temp_dir().join("mica-crash-logs");
    };
    if cfg!(target_os = "macos") {
        home.join("Library/Logs/Mica Slicer")
    } else if cfg!(target_os = "windows") {
        std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or(home)
            .join("Mica Slicer/Logs")
    } else {
        home.join(".local/state/mica-slicer/logs")
    }
}

/// The environment variable naming a folder of pinned upstream data (a checkout's `upstream/`)
/// for a binary that does not run from its checkout (the corpus runs a copy of the CLI).
pub const UPSTREAM_VAR: &str = "MICA_UPSTREAM_DIR";

/// The pinned upstream data (`upstream/`, see `upstream/UPSTREAM.lock`) that a development
/// build falls back to when no PrusaSlicer or OrcaSlicer is installed: `MICA_UPSTREAM_DIR` if
/// set, else the `upstream/` folder of the checkout this binary was built in, but only while
/// the binary runs from inside that checkout (`target/…`: tests, `cargo run`, the xtasks). A
/// packaged build (compiled with `MICA_PACKAGED` set, as `scripts/release.sh` does) and any
/// copy installed elsewhere never read the build machine's checkout.
pub fn upstream_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os(UPSTREAM_VAR).filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    static CHECKOUT: OnceLock<Option<PathBuf>> = OnceLock::new();
    CHECKOUT
        .get_or_init(|| {
            if option_env!("MICA_PACKAGED").is_some_and(|v| !v.is_empty()) {
                return None;
            }
            let root = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .canonicalize()
                .ok()?;
            let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
            exe.starts_with(&root).then(|| root.join("upstream"))
        })
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_test_binary_finds_the_checkouts_upstream_folder() {
        // Test executables live in this checkout's target/ (MICA_UPSTREAM_DIR is never set by
        // the test harness).
        if std::env::var_os(UPSTREAM_VAR).is_none() {
            let dir = upstream_dir().expect("tests run from inside the checkout");
            assert!(dir.join("UPSTREAM.lock").is_file(), "{}", dir.display());
        }
    }

    #[test]
    fn tests_never_see_the_users_data() {
        assert!(isolated());
        assert_eq!(home(), None);
        assert_eq!(var_dir("HOME"), None);
        if std::env::var_os("MICA_CONFIG_DIR").is_none() {
            assert!(config_dir().starts_with(std::env::temp_dir()));
        }
        if std::env::var_os("MICA_CRASH_LOG_DIR").is_none() {
            assert!(crash_log_dir().starts_with(std::env::temp_dir()));
        }
        assert!(is_test_exe(Path::new(
            "/w/target/debug/deps/mica_core-0123abcd"
        )));
        assert!(!is_test_exe(Path::new("/w/target/debug/mica-gui")));
    }

    #[test]
    fn windows_finds_the_home_folder_without_home() {
        use std::ffi::OsString;
        fn env<'a>(vars: &'a [(&str, &str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
            move |n| {
                vars.iter()
                    .find(|(k, _)| *k == n)
                    .map(|(_, v)| OsString::from(v))
            }
        }
        let profile = [("USERPROFILE", r"C:\Users\Zoë Ørsted")];
        assert_eq!(
            home_from(true, env(&profile)),
            Some(PathBuf::from(r"C:\Users\Zoë Ørsted"))
        );
        // USERPROFILE wins over an MSYS-style HOME; HOME only when it is all there is.
        let both = [("HOME", "/c/Users/zoe"), ("USERPROFILE", r"C:\Users\zoe")];
        assert_eq!(
            home_from(true, env(&both)),
            Some(PathBuf::from(r"C:\Users\zoe"))
        );
        assert_eq!(
            home_from(true, env(&[("HOME", r"D:\h")])),
            Some(PathBuf::from(r"D:\h"))
        );
        assert_eq!(home_from(true, env(&[("USERPROFILE", "")])), None);
        // Elsewhere only HOME counts.
        assert_eq!(home_from(false, env(&profile)), None);
        assert_eq!(
            home_from(false, env(&[("HOME", "/home/zoë ø")])),
            Some(PathBuf::from("/home/zoë ø"))
        );
    }

    #[test]
    fn the_real_settings_folder_panics_in_tests() {
        let Some(home) = real_home() else {
            return;
        };
        let real = default_config_dir(&home);
        assert!(std::panic::catch_unwind(|| guard(&real)).is_err());
        guard(&std::env::temp_dir().join("mica-other"));
    }
}
