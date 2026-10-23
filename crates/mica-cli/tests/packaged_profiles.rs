// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! A packaged Mica on a computer without PrusaSlicer or OrcaSlicer: no checkout `upstream/`
//! (`MICA_UPSTREAM_DIR` names an empty folder), no home folder (`MICA_ISOLATED`), an empty
//! settings folder, and only the Orca subset `cargo xtask bundle-profiles` writes
//! (`MICA_BUNDLED_PROFILES`). Orca printers come from the shipped subset; Prusa printers say
//! they need `mica profiles fetch`, and once the pinned bundle is in the download folder they
//! slice to the reference hash.

use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

struct Packaged {
    root: PathBuf,
}

impl Packaged {
    fn new(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!("mica-packaged-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for d in ["upstream", "config", "app/profiles"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        mica_profiles::vendor_profiles::write_orca_subset(
            &mica_testkit::orca_profiles(),
            &root.join("app/profiles/orca"),
            &mica_profiles::vendor_profiles::SHIPPED_ORCA_VENDORS,
        )
        .unwrap();
        Self { root }
    }

    fn mica(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_mica"))
            .env("MICA_UPSTREAM_DIR", self.root.join("upstream"))
            .env("MICA_BUNDLED_PROFILES", self.root.join("app/profiles"))
            .env("MICA_CONFIG_DIR", self.root.join("config"))
            .env("MICA_ISOLATED", "1")
            .env_remove("MICA_PRUSA_BUNDLE")
            .env_remove("MICA_ORCA_PROFILES")
            .env("SOURCE_DATE_EPOCH", "0")
            .args(args)
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }

    fn slice(&self, model: &str, extra: &[&str]) -> String {
        let out = self.root.join(format!("{model}.gcode"));
        let model = mica_testkit::model(model);
        let mut args = vec!["slice", model.to_str().unwrap()];
        args.extend_from_slice(extra);
        args.extend(["-o", out.to_str().unwrap()]);
        let r = self.mica(&args);
        assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
        let hash = Sha256::digest(mica_testkit::reference_gcode(&std::fs::read(&out).unwrap()));
        hash.iter().take(4).map(|b| format!("{b:02x}")).collect()
    }
}

impl Drop for Packaged {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

const U1: [&str; 10] = [
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

#[test]
fn a_packaged_build_uses_the_shipped_orca_profiles_and_asks_to_fetch_prusas() {
    if !mica_testkit::require_orca_profiles() {
        return;
    }
    let app = Packaged::new("orca");
    let shipped = app.root.join("app/profiles/orca");

    // Orca: the U1 lists and slices from the shipped subset.
    let r = app.mica(&["profiles", "printer", "--vendor", "Snapmaker"]);
    let out = text(&r);
    assert!(r.status.success(), "{out}");
    assert!(out.contains("Snapmaker U1 (0.4 nozzle)"), "{out}");
    assert!(
        slashes(&out).contains(&slashes(shipped.to_str().unwrap())),
        "{out}"
    );
    assert_eq!(app.slice("cube20.stl", &U1), "d86e39a8");

    // A vendor that is not shipped comes from an installed OrcaSlicer, if any, or says so.
    let r = app.mica(&["profiles", "printer", "--vendor", "Flashforge"]);
    if r.status.success() {
        assert!(
            !slashes(&text(&r)).contains(&slashes(shipped.to_str().unwrap())),
            "{}",
            text(&r)
        );
    } else {
        assert!(
            text(&r).contains("Mica ships only some vendors"),
            "{}",
            text(&r)
        );
    }

    // Prusa: nothing to read until it is downloaded, and the error names the command.
    let cube = mica_testkit::model("cube20.stl");
    for args in [
        vec!["profiles", "printer"],
        vec!["slice", cube.to_str().unwrap()],
    ] {
        let r = app.mica(&args);
        assert!(!r.status.success(), "{args:?}");
        assert!(text(&r).contains("mica profiles fetch"), "{}", text(&r));
    }
}

#[test]
fn the_downloaded_prusa_bundle_slices_the_reference() {
    if !mica_testkit::require_orca_profiles() || !mica_testkit::require_prusa_profiles() {
        return;
    }
    let app = Packaged::new("prusa");
    let cache = app.root.join("config/vendor-profiles/PrusaResearch");
    std::fs::create_dir_all(&cache).unwrap();
    // A file that is not the pinned bundle is never used.
    std::fs::write(
        cache.join("2.5.12.ini"),
        "[vendor]\nname = Prusa Research\n",
    )
    .unwrap();
    let r = app.mica(&["profiles", "printer"]);
    assert!(text(&r).contains("mica profiles fetch"), "{}", text(&r));
    // The pinned bundle, as `mica profiles fetch` stores it.
    std::fs::copy(mica_testkit::prusa_bundle(), cache.join("2.5.12.ini")).unwrap();
    let r = app.mica(&["profiles", "printer"]);
    assert!(r.status.success(), "{}", text(&r));
    assert!(
        slashes(&text(&r)).contains(&slashes(&cache.to_string_lossy())),
        "{}",
        text(&r)
    );
    assert_eq!(app.slice("cube20.stl", &[]), "d833d655");
}

/// Paths as text with one separator, so a Windows path printed with `\` matches one built with `/`.
fn slashes(s: &str) -> String {
    s.replace('\\', "/")
}
