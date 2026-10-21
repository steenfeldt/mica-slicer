// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The reference hashes: `mica slice` of cube20 and 3DBenchy on the CORE One (the default
//! printer), XL 5T, Snapmaker U1, MINI IS and MK3.5, with the merge gate's exact presets and a
//! fixed clock (`SOURCE_DATE_EPOCH=0`), must produce G-code whose sha256 starts with the
//! recorded prefix. Any change to the G-code of these ten slices fails here; a change that is
//! meant (matching upstream better) updates the prefix in the same commit, with the reason.
//!
//! The binary runs isolated, so it slices with the pinned vendor profiles of `upstream/`
//! (PrusaSlicer profiles 2.5.12, OrcaSlicer 2.4.2). The hashes are recorded on macOS arm64;
//! other platforms' math libraries may round differently, so the tests are ignored there. The
//! Benchy slices are ignored in debug builds (minutes each); `cargo test --profile fast` (CI)
//! and `cargo test --release` run them.

use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

/// The default printer (Prusa CORE One HF0.4, SPEED, Prusament PLA): no profile arguments.
const CORE_ONE: &[&str] = &[];
const XL: &[&str] = &[
    "--printer",
    "Original Prusa XL - 5T Input Shaper 0.4 nozzle",
    "--print",
    "0.20mm SPEED @XLIS 0.4",
    "--filament",
    "Prusament PLA @XLIS",
];
const U1: &[&str] = &[
    "--vendor",
    "Snapmaker",
    "--printer",
    "Snapmaker U1 (0.4 nozzle)",
    "--print",
    "0.20 Standard @Snapmaker U1 (0.4 nozzle)",
    "--filament",
    "Snapmaker PLA @U1",
    "--bed",
    "High Temp Plate",
];
const MINI: &[&str] = &[
    "--printer",
    "Original Prusa MINI & MINI+ Input Shaper",
    "--print",
    "0.20mm SPEED @MINIIS 0.4",
    "--filament",
    "Prusament PLA @MINIIS",
];
const MK35: &[&str] = &[
    "--printer",
    "Original Prusa MK3.5 & MK3.5S 0.4 nozzle",
    "--print",
    "0.20mm SPEED @MK3.5 0.4",
    "--filament",
    "Prusament PLA @MK3.5",
];

/// Removes the case's folder (config and G-code) when done.
struct Temp(PathBuf);
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// `mica slice tests/models/<model>.stl <profile> -o …` from the repository root (as the gate
/// runs it) and the sha256 of the G-code, as lowercase hex.
fn slice_hash(model: &str, profile: &[&str]) -> Option<String> {
    if !mica_testkit::require_profiles_for(profile) {
        return None;
    }
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let dir = Temp(std::env::temp_dir().join(format!(
        "mica-reference-hash-{}-{model}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )));
    let _ = std::fs::remove_dir_all(&dir.0);
    std::fs::create_dir_all(&dir.0).unwrap();
    let out = dir.0.join("out.gcode");
    let result = Command::new(env!("CARGO_BIN_EXE_mica"))
        .current_dir(mica_testkit::root())
        .env("SOURCE_DATE_EPOCH", "0")
        .env("MICA_ISOLATED", "1")
        .env("MICA_CONFIG_DIR", dir.0.join("config"))
        .env_remove(mica_core::user_data::UPSTREAM_VAR)
        .arg("slice")
        .arg(format!("tests/models/{model}.stl"))
        .args(profile)
        .arg("-o")
        .arg(&out)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "mica slice {model} {profile:?}: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let digest = Sha256::digest(std::fs::read(&out).unwrap());
    Some(digest.iter().map(|b| format!("{b:02x}")).collect())
}

#[track_caller]
fn check(model: &str, profile: &[&str], prefix: &str) {
    let Some(hash) = slice_hash(model, profile) else {
        return;
    };
    assert!(
        hash.starts_with(prefix),
        "{model} {profile:?}: sha256 {hash}, the reference is {prefix}…; if the G-code change \
         is intended, update the prefix here and in docs/quality-safety.md with the reason"
    );
}

macro_rules! reference {
    ($cube:ident, $benchy:ident, $profile:expr, $cube_hash:literal, $benchy_hash:literal) => {
        #[test]
        #[cfg_attr(
            not(all(target_os = "macos", target_arch = "aarch64")),
            ignore = "reference hashes are recorded on macOS arm64"
        )]
        fn $cube() {
            check("cube20", $profile, $cube_hash);
        }

        #[test]
        #[cfg_attr(
            not(all(target_os = "macos", target_arch = "aarch64")),
            ignore = "reference hashes are recorded on macOS arm64"
        )]
        #[cfg_attr(
            debug_assertions,
            ignore = "slow in debug: cargo test --profile fast (CI) or --release runs it"
        )]
        fn $benchy() {
            check("3DBenchy", $profile, $benchy_hash);
        }
    };
}

reference!(
    core_one_cube20,
    core_one_benchy,
    CORE_ONE,
    "d833d655",
    "82b5234b"
);
reference!(xl_5t_cube20, xl_5t_benchy, XL, "525f4840", "50db0e69");
reference!(u1_cube20, u1_benchy, U1, "d86e39a8", "1a3357ae");
reference!(mini_is_cube20, mini_is_benchy, MINI, "5093345f", "1c06b7ab");
reference!(mk35_cube20, mk35_benchy, MK35, "c923aa75", "01f46892");
