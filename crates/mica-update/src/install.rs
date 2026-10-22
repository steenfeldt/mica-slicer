// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Putting a verified package in place, and starting the new version once this one has quit.
//! Only layouts Mica recognizes are touched: the `.app` bundle the running binary sits in
//! (macOS), the AppImage named by `$APPIMAGE` (Linux), an NSIS installation (Windows). Any
//! other layout (a bare binary, a disk image, a translocated app) is refused with a message,
//! never "updated" by guessing.

use crate::{Format, sibling};
use std::{
    path::{Path, PathBuf},
    process::Command,
};

/// What an update replaces.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// The `.app` bundle (macOS).
    MacApp(PathBuf),
    /// The `.AppImage` file (Linux).
    AppImage(PathBuf),
    /// The installed `.exe` (Windows); the NSIS installer replaces it.
    Windows(PathBuf),
}

impl Target {
    pub fn format(&self) -> Format {
        match self {
            Target::MacApp(_) => Format::App,
            Target::AppImage(_) => Format::AppImage,
            Target::Windows(_) => Format::Nsis,
        }
    }
    pub fn path(&self) -> &Path {
        match self {
            Target::MacApp(p) | Target::AppImage(p) | Target::Windows(p) => p,
        }
    }
}

/// The result of [`install`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Staged {
    /// The new version is in place at this path; it starts the next time Mica starts.
    Replaced(PathBuf),
    /// The installer is saved here; it runs once Mica has quit ([`relaunch_after_exit`]).
    Installer(PathBuf),
}

/// What the running executable `exe` belongs to. `appimage`: the `$APPIMAGE` variable.
pub fn target_for(exe: &Path, appimage: Option<&Path>) -> Result<Target, String> {
    if cfg!(target_os = "macos") {
        // …/Mica Slicer.app/Contents/MacOS/mica-gui
        let app = exe
            .parent()
            .filter(|d| d.file_name().is_some_and(|n| n == "MacOS"))
            .and_then(Path::parent)
            .filter(|d| d.file_name().is_some_and(|n| n == "Contents"))
            .and_then(Path::parent)
            .filter(|d| d.extension().is_some_and(|e| e == "app"))
            .ok_or("Mica updates itself only when it runs as an app (Mica Slicer.app)")?;
        let s = app.to_string_lossy();
        if s.contains("/AppTranslocation/") {
            return Err(
                "Mica is running from a temporary, read-only location. Move Mica Slicer to \
                 the Applications folder, open it from there and check again."
                    .into(),
            );
        }
        if s.starts_with("/Volumes/") && read_only(app) {
            return Err(
                "Mica is running from the disk image. Drag Mica Slicer to the Applications \
                 folder, open it from there and check again."
                    .into(),
            );
        }
        Ok(Target::MacApp(app.to_path_buf()))
    } else if cfg!(target_os = "windows") {
        Ok(Target::Windows(exe.to_path_buf()))
    } else {
        let image = appimage
            .filter(|p| p.is_file())
            .ok_or("Mica updates itself only when it runs as an AppImage")?;
        Ok(Target::AppImage(image.to_path_buf()))
    }
}

fn read_only(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.permissions().readonly())
        || std::fs::metadata(path.parent().unwrap_or(path))
            .is_ok_and(|m| m.permissions().readonly())
}

/// Puts the verified package `bytes` of `format` in place for `target`.
pub fn install(target: &Target, format: Format, bytes: &[u8]) -> Result<Staged, String> {
    if format != target.format() {
        return Err(format!(
            "the update package is a {format:?} package, which does not fit this installation"
        ));
    }
    match target {
        // The bundle is swapped while Mica runs: never under another running Mica, and only
        // one process at a time.
        Target::MacApp(app) => {
            let exe = std::fs::read_dir(app.join("Contents").join("MacOS"))
                .ok()
                .and_then(|mut d| d.find_map(|e| e.ok().map(|e| e.path())));
            let _only = match exe {
                Some(exe) => Some(crate::instance::exclusive(&exe)?),
                None => None,
            };
            install_app(app, bytes)
        }
        Target::AppImage(image) => install_appimage(image, bytes),
        Target::Windows(_) => save_installer(bytes),
    }
}

/// macOS: unpacks the `.tar.gz` next to the running bundle and swaps the two, keeping the old
/// one until the new one is in place. A folder Mica may not write (an `/Applications` owned
/// by an administrator) is done through the system's administrator prompt.
fn install_app(app: &Path, bytes: &[u8]) -> Result<Staged, String> {
    // The new bundle must carry the executable the running one has.
    let exe_name = std::fs::read_dir(app.join("Contents").join("MacOS"))
        .ok()
        .and_then(|mut d| d.find_map(|e| e.ok().map(|e| e.file_name())));
    let near = sibling(app, "update");
    let staging = match std::fs::create_dir_all(&near) {
        Ok(()) => near,
        // Not writable here: unpack in the temporary folder, move with the admin prompt.
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            let t = std::env::temp_dir().join(format!("mica-update-{}", std::process::id()));
            std::fs::create_dir_all(&t)
                .map_err(|e| format!("cannot unpack the update in {}: {e}", t.display()))?;
            t
        }
        Err(e) => {
            return Err(format!(
                "cannot unpack the update next to {}: {e}",
                app.display()
            ));
        }
    };
    let result = unpack_and_swap(app, &staging, bytes, exe_name.as_deref());
    let _ = std::fs::remove_dir_all(&staging);
    result.map(|()| Staged::Replaced(app.to_path_buf()))
}

fn unpack_and_swap(
    app: &Path,
    staging: &Path,
    bytes: &[u8],
    exe_name: Option<&std::ffi::OsStr>,
) -> Result<(), String> {
    let archive = staging.join("update.tar.gz");
    std::fs::write(&archive, bytes).map_err(|e| format!("cannot save the update: {e}"))?;
    let out = staging.join("unpacked");
    std::fs::create_dir_all(&out).map_err(|e| format!("cannot unpack the update: {e}"))?;
    let run = Command::new("/usr/bin/tar")
        .arg("-xzf")
        .arg(&archive)
        .arg("-C")
        .arg(&out)
        .output()
        .map_err(|e| format!("cannot run tar to unpack the update: {e}"))?;
    if !run.status.success() {
        let why = String::from_utf8_lossy(&run.stderr);
        return Err(format!(
            "the update package could not be unpacked ({})",
            why.lines().next().unwrap_or("tar failed").trim()
        ));
    }
    let new = new_bundle(&out, exe_name)?;
    let old = sibling(app, "old");
    let _ = std::fs::remove_dir_all(&old);
    match std::fs::rename(app, &old) {
        Ok(()) => {
            if let Err(e) = std::fs::rename(&new, app) {
                // Put the running version back.
                let back = std::fs::rename(&old, app);
                return Err(match back {
                    Ok(()) => format!("cannot put the new version in place: {e}"),
                    Err(b) => format!(
                        "cannot put the new version in place ({e}), and the old one could not be \
                         restored ({b}); it is at {}",
                        old.display()
                    ),
                });
            }
            let _ = std::fs::remove_dir_all(&old);
            touch(app);
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            swap_as_admin(app, &new, &old)?;
            touch(app);
            Ok(())
        }
        Err(e) => Err(format!("cannot replace {}: {e}", app.display())),
    }
}

/// The one `.app` in the unpacked archive, with the same executable as the running one.
fn new_bundle(dir: &Path, exe_name: Option<&std::ffi::OsStr>) -> Result<PathBuf, String> {
    let apps: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| format!("cannot read the unpacked update: {e}"))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "app") && p.is_dir())
        .collect();
    let [app] = apps.as_slice() else {
        return Err("the update package does not hold exactly one app".into());
    };
    let macos = app.join("Contents").join("MacOS");
    let ok = match exe_name {
        Some(n) => macos.join(n).is_file(),
        None => macos.is_dir(),
    };
    if !ok || !app.join("Contents").join("Info.plist").is_file() {
        return Err("the update package does not hold a complete Mica Slicer app".into());
    }
    Ok(app.clone())
}

/// A single-quoted shell word.
fn sh_quote(p: &Path) -> String {
    format!("'{}'", p.to_string_lossy().replace('\'', r"'\''"))
}

/// Moves `new` over `app` with the administrator prompt, restoring `app` if that fails.
fn swap_as_admin(app: &Path, new: &Path, old: &Path) -> Result<(), String> {
    let (a, n, o) = (sh_quote(app), sh_quote(new), sh_quote(old));
    let shell = format!(
        "mv -f {a} {o} && if mv -f {n} {a}; then rm -rf {o}; else mv -f {o} {a}; exit 1; fi"
    );
    let script = format!(
        "do shell script \"{}\" with administrator privileges",
        shell.replace('\\', r"\\").replace('"', "\\\"")
    );
    let status = Command::new("/usr/bin/osascript")
        .arg("-e")
        .arg(script)
        .status()
        .map_err(|e| format!("cannot ask for administrator rights: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "Mica Slicer could not be replaced in {} (the administrator prompt was canceled or \
             refused)",
            app.parent().unwrap_or(app).display()
        ))
    }
}

/// Lets Finder and Launch Services notice the new bundle.
fn touch(app: &Path) {
    let _ = Command::new("/usr/bin/touch").arg(app).status();
}

/// Linux: writes the new AppImage next to the old one, then renames it over (atomic on one
/// file system; the running image keeps its old file until it exits).
fn install_appimage(image: &Path, bytes: &[u8]) -> Result<Staged, String> {
    let tmp = sibling(image, "update");
    std::fs::write(&tmp, bytes).map_err(|e| {
        format!(
            "cannot write the update next to {} ({e}); is that folder writable?",
            image.display()
        )
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(image)
            .map(|m| m.permissions().mode())
            .unwrap_or(0o755)
            | 0o111;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode));
    }
    std::fs::rename(&tmp, image).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("cannot replace {}: {e}", image.display())
    })?;
    Ok(Staged::Replaced(image.to_path_buf()))
}

/// Windows: saves the NSIS installer; it runs after Mica has quit.
fn save_installer(bytes: &[u8]) -> Result<Staged, String> {
    let path = std::env::temp_dir().join(format!(
        "Mica Slicer update {}-setup.exe",
        std::process::id()
    ));
    std::fs::write(&path, bytes).map_err(|e| format!("cannot save the installer: {e}"))?;
    Ok(Staged::Installer(path))
}

/// Starts a small helper that waits (up to ten minutes) for this process to exit and then
/// opens the new version (`restart`) or, on Windows, runs the installer (which starts Mica
/// again only with `restart`). The caller then closes Mica the normal way, so unsaved work is
/// still asked about; if Mica is still open after ten minutes, the helper gives up and nothing
/// happens.
pub fn relaunch_after_exit(staged: &Staged, restart: bool) -> Result<(), String> {
    let pid = std::process::id().to_string();
    let spawned = match staged {
        Staged::Replaced(path) => {
            if !restart {
                return Ok(());
            }
            let open = if cfg!(target_os = "macos") {
                r#"exec /usr/bin/open "$0""#
            } else {
                r#"exec "$0""#
            };
            let script = format!(
                r#"i=0; while kill -0 "$1" 2>/dev/null && [ $i -lt 3000 ]; do sleep 0.2; i=$((i+1)); done; kill -0 "$1" 2>/dev/null || {open}"#
            );
            Command::new("/bin/sh")
                .arg("-c")
                .arg(script)
                .arg(path)
                .arg(&pid)
                .spawn()
        }
        Staged::Installer(exe) => windows_installer(exe, &pid, restart),
    };
    spawned
        .map(|_| ())
        .map_err(|e| format!("cannot start the helper that restarts Mica: {e}"))
}

#[cfg(windows)]
fn windows_installer(exe: &Path, pid: &str, restart: bool) -> std::io::Result<std::process::Child> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let quoted = exe.to_string_lossy().replace('\'', "''");
    // /P: passive (progress only); /R: start Mica when done (cargo-packager's NSIS template).
    let args = if restart { "'/P','/R'" } else { "'/P'" };
    let script = format!(
        "$p = Get-Process -Id {pid} -ErrorAction SilentlyContinue; \
         if ($p) {{ if (-not $p.WaitForExit(600000)) {{ exit 1 }} }}; \
         Start-Process -FilePath '{quoted}' -ArgumentList {args}"
    );
    let shell = std::env::var("SYSTEMROOT").map_or_else(
        |_| "powershell.exe".to_string(),
        |r| format!("{r}\\System32\\WindowsPowerShell\\v1.0\\powershell.exe"),
    );
    Command::new(shell)
        .creation_flags(CREATE_NO_WINDOW)
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-WindowStyle",
            "Hidden",
            "-Command",
        ])
        .arg(script)
        .spawn()
}

#[cfg(not(windows))]
fn windows_installer(
    _exe: &Path,
    _pid: &str,
    _restart: bool,
) -> std::io::Result<std::process::Child> {
    Err(std::io::Error::other("installers run only on Windows"))
}
