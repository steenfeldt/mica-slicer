// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! `cargo xtask monkey [--quick] [mica-gui --monkey options]`: builds the release GUI offline
//! and runs its monkey test (crates/mica-gui/src/monkey.rs). `--quick` is three seeds of 200
//! steps (well under a minute); the full run is 20 seeds of 2,000 steps. Crash logs of
//! contained panics go to the system temporary folder, not the user's log folder.

use std::{
    path::Path,
    process::{Command, ExitCode},
};

pub fn run(root: &Path) -> ExitCode {
    let profile = crate::profile();
    let build = Command::new("cargo")
        // The loopback mock printers the monkey's printer actions talk to.
        .args([
            "build",
            "--offline",
            "--profile",
            &profile,
            "-p",
            "mica-gui",
            "-p",
            "mica-printhost",
            // A target filter applies to every package: without `--bin`, only the example
            // was built and the monkey ran whatever GUI binary was left in target/.
            "--bin",
            "mica-gui",
            "--example",
            "mock_printer",
        ])
        .current_dir(root)
        .status();
    if !build.is_ok_and(|s| s.success()) {
        eprintln!("could not build the GUI for the monkey test");
        return ExitCode::FAILURE;
    }
    let exe = root
        .join("target")
        .join(crate::profile_dir(&profile))
        .join(format!("mica-gui{}", std::env::consts::EXE_SUFFIX));
    let logs = std::env::temp_dir().join("mica-monkey-crash-logs");
    match Command::new(&exe)
        .arg("--monkey")
        .args(std::env::args().skip(2))
        .env("MICA_CRASH_LOG_DIR", &logs)
        .env("MICA_ISOLATED", "1")
        .current_dir(root)
        .status()
    {
        Ok(s) if s.success() => ExitCode::SUCCESS,
        Ok(_) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("could not run {}: {e}", exe.display());
            ExitCode::FAILURE
        }
    }
}
