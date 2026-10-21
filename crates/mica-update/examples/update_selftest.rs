// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Runs the updater's real steps against real release artifacts, for testing a release
//! locally (docs/release.md): check `manifest` as version `current`, download, verify against
//! `public_keys` (base64, comma- or newline-separated), and install over `target` (an `.app` bundle on
//! macOS, an `.AppImage` on Linux). Use a throwaway key and a copy of the app, never an
//! installed one.
//!
//!   cargo run -p mica-update --example update_selftest -- <manifest url> <public keys> <current version> <target>

use std::{path::PathBuf, sync::atomic::AtomicBool};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [url, key, current, target] = args.as_slice() else {
        eprintln!("usage: update_selftest <manifest url> <public keys> <current version> <target>");
        std::process::exit(2);
    };
    let client = mica_update::Client {
        manifest_url: url.clone(),
        public_keys: key
            .split([',', '\n'])
            .map(str::trim)
            .filter(|k| !k.is_empty() && !k.starts_with('#'))
            .map(str::to_string)
            .collect(),
        platform: mica_update::platform().expect("a supported platform"),
    };
    let current = mica_update::Version::parse(current).expect("a semver version");
    let release = match client.check(&current) {
        Ok(Some(r)) => r,
        Ok(None) => {
            println!("check: up to date");
            return;
        }
        Err(e) => fail("check", &e),
    };
    println!(
        "check: {} available ({:?}, {})",
        release.version, release.format, release.url
    );
    let mut last = 0;
    let data = client
        .download(
            &release,
            &mut |got, total| {
                let pct = mica_update::fraction(got, total).map_or(0, |f| (f * 10.) as u32);
                if pct != last {
                    last = pct;
                    println!("download: {got} / {total:?}");
                }
            },
            &AtomicBool::new(false),
        )
        .unwrap_or_else(|e| fail("download", &e));
    println!("download: {} bytes, signature verified", data.len());
    let path = PathBuf::from(target);
    let target = if path.extension().is_some_and(|e| e == "app") {
        mica_update::Target::MacApp(path)
    } else {
        mica_update::Target::AppImage(path)
    };
    match mica_update::install(&target, release.format, &data) {
        Ok(s) => println!("install: {s:?}"),
        Err(e) => fail("install", &e),
    }
}

fn fail(step: &str, e: &str) -> ! {
    println!("{step}: FAILED: {e}");
    std::process::exit(1);
}
