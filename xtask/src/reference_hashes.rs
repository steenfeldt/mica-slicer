// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Fixed-clock reference hash entry point for the review/merge gate.
use std::{
    path::Path,
    process::{Command, ExitCode},
};

pub fn run(root: &Path) -> ExitCode {
    let profile = crate::profile();
    let built = Command::new("cargo")
        .args([
            "build",
            "--offline",
            "--profile",
            &profile,
            "-p",
            "mica-cli",
        ])
        .current_dir(root)
        .status();
    if !built.is_ok_and(|s| s.success()) {
        return ExitCode::FAILURE;
    }
    let status = Command::new("python3")
        .arg(root.join("scripts/gcode-reference-hashes.py"))
        .args(std::env::args().skip(2))
        .arg("--binary")
        .arg(
            root.join("target")
                .join(crate::profile_dir(&profile))
                .join(format!("mica{}", std::env::consts::EXE_SUFFIX)),
        )
        .env("SOURCE_DATE_EPOCH", "0")
        .current_dir(root)
        .status();
    if status.is_ok_and(|s| s.success()) {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
