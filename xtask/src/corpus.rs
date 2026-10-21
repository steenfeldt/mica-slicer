// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! `cargo xtask corpus [run.py options]`: the release CLI over the regression corpus
//! (tests/corpus/run.py). The full run (no options) also checks the CLI/GUI byte-identity
//! invariant (`cargo xtask parity`).

use std::{
    path::Path,
    process::{Command, ExitCode},
};
pub fn run(root: &Path) -> ExitCode {
    let profile = crate::profile();
    let build = Command::new("cargo")
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
    if !build.is_ok_and(|s| s.success()) {
        eprintln!("could not build corpus CLI");
        return ExitCode::FAILURE;
    }
    // The CLI reads no user profiles from the real settings folder (the PrusaSlicer vendor
    // bundle it finds is still the installed one, as for the reference hashes).
    let config = std::env::var_os("MICA_CONFIG_DIR").map_or_else(
        || std::env::temp_dir().join(format!("mica-corpus-config-{}", std::process::id())),
        std::path::PathBuf::from,
    );
    let corpus = match Command::new("python3")
        .env("MICA_CONFIG_DIR", &config)
        .env("SOURCE_DATE_EPOCH", "0")
        .arg(root.join("tests/corpus/run.py"))
        .args(std::env::args().skip(2))
        .args(
            (!std::env::args().any(|a| a == "--binary"))
                .then(|| {
                    let bin = root
                        .join("target")
                        .join(crate::profile_dir(&profile))
                        .join(format!("mica{}", std::env::consts::EXE_SUFFIX));
                    ["--binary".to_string(), bin.display().to_string()]
                })
                .into_iter()
                .flatten(),
        )
        .current_dir(root)
        .status()
    {
        Ok(s) if s.success() => ExitCode::SUCCESS,
        Ok(_) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("corpus requires Python 3 (standard library only): {e}");
            ExitCode::FAILURE
        }
    };
    if std::env::args().nth(2).is_some() {
        return corpus;
    }
    let parity = crate::parity::run(root, None);
    if corpus == ExitCode::SUCCESS {
        parity
    } else {
        corpus
    }
}
