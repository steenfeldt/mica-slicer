// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! `cargo xtask bundle-profiles <out-dir>`: the OrcaSlicer vendor profiles a packaged Mica
//! ships, copied from the pinned `upstream/orca` checkout into `<out-dir>/orca/` (the vendors
//! in `mica_profiles::vendor_profiles::SHIPPED_ORCA_VENDORS`, everything they inherit from,
//! and their printers' bed models, textures and gallery covers), with OrcaSlicer's license
//! and a NOTICE naming the source. `scripts/release.sh` runs it and puts the folder into each
//! package. Prusa's profiles are never bundled (PROVENANCE.md): Mica downloads them.

use mica_profiles::vendor_profiles::{SHIPPED_ORCA_VENDORS, write_orca_subset};
use std::path::Path;
use std::process::{Command, ExitCode};

pub fn run(root: &Path, out: Option<&str>) -> ExitCode {
    let Some(out) = out else {
        eprintln!("usage: cargo xtask bundle-profiles <out-dir>");
        return ExitCode::from(2);
    };
    match bundle(root, Path::new(out)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("bundle-profiles: {e}");
            ExitCode::FAILURE
        }
    }
}

/// The pinned `orca` commit of `upstream/UPSTREAM.lock`.
fn pinned_commit(root: &Path) -> Result<String, String> {
    let lock = std::fs::read_to_string(root.join("upstream/UPSTREAM.lock"))
        .map_err(|e| format!("upstream/UPSTREAM.lock: {e}"))?;
    lock.lines()
        .map(str::split_whitespace)
        .find_map(|mut w| (w.next() == Some("orca")).then(|| w.nth(2).map(str::to_string))?)
        .ok_or_else(|| "upstream/UPSTREAM.lock has no orca line".into())
}

fn bundle(root: &Path, out: &Path) -> Result<(), String> {
    let checkout = root.join("upstream/orca");
    let src = checkout.join("resources/profiles");
    if !src.join("OrcaFilamentLibrary.json").is_file() {
        return Err(format!(
            "{} is missing: scripts/fetch-upstream.sh orca",
            src.display()
        ));
    }
    let commit = pinned_commit(root)?;
    // A checkout with its git data must be at the pinned commit (fetch-upstream.sh's sparse
    // copies are checked when they are fetched).
    if checkout.join(".git").exists() {
        let head = Command::new("git")
            .arg("-C")
            .arg(&checkout)
            .args(["rev-parse", "HEAD"])
            .output()
            .map_err(|e| format!("git: {e}"))?;
        let head = String::from_utf8_lossy(&head.stdout).trim().to_string();
        if head != commit {
            return Err(format!(
                "upstream/orca is at {head}, UPSTREAM.lock pins {commit}"
            ));
        }
    }
    let dst = out.join("orca");
    if dst.exists() {
        std::fs::remove_dir_all(&dst).map_err(|e| format!("{}: {e}", dst.display()))?;
    }
    let copied = write_orca_subset(&src, &dst, &SHIPPED_ORCA_VENDORS)?;
    std::fs::copy(checkout.join("LICENSE.txt"), dst.join("LICENSE.txt"))
        .map_err(|e| format!("OrcaSlicer LICENSE.txt: {e}"))?;
    std::fs::write(
        dst.join("NOTICE.txt"),
        format!(
            "These printer, process and filament profiles and their bed models, textures and\n\
             pictures are from OrcaSlicer (https://github.com/OrcaSlicer/OrcaSlicer, commit\n\
             {commit}, resources/profiles), licensed under the GNU Affero General Public\n\
             License v3.0 (LICENSE.txt). Copyright their respective authors and printer\n\
             vendors. Mica Slicer ships them unchanged; vendors: {}.\n",
            copied.vendors.join(", ")
        ),
    )
    .map_err(|e| format!("NOTICE.txt: {e}"))?;
    println!(
        "bundle-profiles: {} files, {:.1} MB from orca@{} into {} ({})",
        copied.files,
        copied.bytes as f64 / 1e6,
        &commit[..10],
        dst.display(),
        copied.vendors.join(", ")
    );
    Ok(())
}
