// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! `cargo xtask parity [test filter]`: the CLI/GUI invariant in release. For the same models,
//! presets and placement the GUI's exported G-code is byte-identical to `mica slice`'s
//! (crates/mica-gui/src/cli_parity_tests.rs): STLs on the CORE One, XL and U1 (3DBenchy
//! included, which the default debug test run skips) and on the MINI and MK3.5, an edited
//! (moved, turned, scaled) STL and projects saved by the GUI and sliced by the CLI, 3MF
//! projects, multi-object and arranged plates. The full `cargo xtask corpus` runs it too.

use std::{
    path::Path,
    process::{Command, ExitCode},
};

pub fn run(root: &Path, filter: Option<&str>) -> ExitCode {
    let profile = crate::profile();
    let status = Command::new("cargo")
        .args([
            "test",
            "--offline",
            "--profile",
            &profile,
            "-p",
            "mica-gui",
            "--bin",
            "mica-gui",
            &format!("cli_parity_tests::{}", filter.unwrap_or("")),
            "--",
            "--include-ignored",
        ])
        .env("SOURCE_DATE_EPOCH", "0")
        .current_dir(root)
        .status();
    match status {
        Ok(s) if s.success() => ExitCode::SUCCESS,
        Ok(_) => {
            eprintln!("GUI and CLI G-code differ; see the test output for the files to diff");
            ExitCode::FAILURE
        }
        Err(e) => {
            eprintln!("could not run cargo test: {e}");
            ExitCode::FAILURE
        }
    }
}
