// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Where vendor profiles come from in a build that is not a development checkout.
//!
//! - **OrcaSlicer profiles are shipped.** OrcaSlicer's `resources/profiles` are AGPL-3.0 like
//!   Mica. A packaged build carries a curated subset of the pinned `upstream/orca` copy:
//!   the vendors in [`SHIPPED_ORCA_VENDORS`] with every file their indexes list, the shared
//!   `OrcaFilamentLibrary` they inherit from, and the bed models, textures and gallery covers
//!   of their printers. `cargo xtask bundle-profiles` writes it ([`write_orca_subset`]) and
//!   `scripts/release.sh` puts it into each package, where [`bundled_orca`] finds it.
//! - **Prusa profiles are downloaded.** The PrusaSlicer-settings repository states no license
//!   (PROVENANCE.md), so Mica never redistributes it. On first need Mica downloads exactly the
//!   pinned `PrusaResearch` bundle ([`prusa::VERSION`]) from Prusa's public profile
//!   repository, the one PrusaSlicer updates from, checks its SHA-256 and keeps it in Mica's
//!   settings folder ([`prusa::fetch`], [`prusa::cached_bundle`]).
//!
//! Lookup order everywhere: a development checkout's pinned `upstream/` (never in a packaged
//! build), then the shipped Orca subset / the downloaded Prusa bundle, then an installed
//! PrusaSlicer / OrcaSlicer.

use std::path::{Path, PathBuf};

/// The OrcaSlicer vendors a packaged Mica ships (Orca's vendor index names, `<name>.json` in
/// `resources/profiles`). The one place to change the list: the xtask, the release script and
/// the tests all read it. Everything they inherit from is added by [`write_orca_subset`].
pub const SHIPPED_ORCA_VENDORS: [&str; 10] = [
    // Tested with Mica.
    "Snapmaker",
    "Ratrig",
    // Orca's own Prusa profiles: a Prusa printer switched to Orca rules fills its Orca-only
    // settings from them (`rules_fill`), as in a development build.
    "Prusa",
    // Popular printers (untested with Mica; the gallery lists tested printers first).
    "BBL",
    "Creality",
    "Voron",
    "Elegoo",
    "Anycubic",
    "Qidi",
    "Sovol",
];

/// Orca's shared filament library: every vendor's filaments may inherit from it.
pub const ORCA_LIBRARY: &str = "OrcaFilamentLibrary";

/// Names a folder holding the shipped profiles (`orca/…`) instead of the app's own resources
/// (tests, and running an unpacked build against the xtask's output).
pub const BUNDLED_VAR: &str = "MICA_BUNDLED_PROFILES";

/// The `profiles` folder shipped with a packaged app: [`BUNDLED_VAR`] when set, else next to
/// the executable as the packages lay it out (macOS `Contents/Resources/profiles`, Windows
/// `<install dir>/profiles`, the AppImage's `usr/share/mica-slicer/profiles`).
pub fn bundled_root() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os(BUNDLED_VAR).filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    bundled_root_beside(&std::env::current_exe().ok()?)
}

/// The shipped `profiles` folder of the executable `exe` (see [`bundled_root`]).
fn bundled_root_beside(exe: &Path) -> Option<PathBuf> {
    let bin = exe.parent()?;
    [
        bin.join("../Resources/profiles"),
        bin.join("profiles"),
        bin.join("../share/mica-slicer/profiles"),
    ]
    .into_iter()
    .find(|p| p.is_dir())
}

/// The shipped OrcaSlicer `profiles` folder, when this build has one.
pub fn bundled_orca() -> Option<PathBuf> {
    let dir = bundled_root()?.join("orca");
    dir.join(format!("{ORCA_LIBRARY}.json"))
        .is_file()
        .then_some(dir)
}

/// What [`write_orca_subset`] wrote.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Copied {
    pub vendors: Vec<String>,
    pub files: usize,
    pub bytes: u64,
}

/// Copies `vendors` (index names) and [`ORCA_LIBRARY`] from an Orca `resources/profiles`
/// folder `src` into `dst`: each `<vendor>.json` index, every preset file it lists, and the
/// bed model, bed texture and `<model>_cover.png` of each printer model that exist. Fails when
/// a copied preset inherits from a preset outside the copy (the vendor list would need more).
/// `dst` must not exist yet or be empty, so stale files never ship.
pub fn write_orca_subset(src: &Path, dst: &Path, vendors: &[&str]) -> Result<Copied, String> {
    if dst
        .read_dir()
        .is_ok_and(|mut entries| entries.next().is_some())
    {
        return Err(format!("{} is not empty", dst.display()));
    }
    let mut all: Vec<&str> = vendors.to_vec();
    if !all.contains(&ORCA_LIBRARY) {
        all.push(ORCA_LIBRARY);
    }
    let mut out = Copied::default();
    let mut names = std::collections::BTreeSet::new();
    let mut parents: Vec<(String, String)> = vec![];
    let copy = |rel: &Path, out: &mut Copied| -> Result<(), String> {
        let from = src.join(rel);
        let to = dst.join(rel);
        if let Some(dir) = to.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let n = std::fs::copy(&from, &to).map_err(|e| format!("{}: {e}", from.display()))?;
        out.files += 1;
        out.bytes += n;
        Ok(())
    };
    for vendor in all {
        if !crate::orca::discovery::is_index_name(vendor) {
            return Err(format!("'{vendor}' is not an Orca vendor index name"));
        }
        let index = format!("{vendor}.json");
        let text = std::fs::read_to_string(src.join(&index))
            .map_err(|e| format!("{}: {e}", src.join(&index).display()))?;
        let json: serde_json::Value =
            serde_json::from_str(&text).map_err(|e| format!("{index}: {e}"))?;
        copy(Path::new(&index), &mut out)?;
        for list in [
            "machine_model_list",
            "process_list",
            "filament_list",
            "machine_list",
        ] {
            for entry in json
                .get(list)
                .and_then(|l| l.as_array())
                .into_iter()
                .flatten()
            {
                let (Some(name), Some(sub)) = (
                    entry.get("name").and_then(|n| n.as_str()),
                    entry.get("sub_path").and_then(|n| n.as_str()),
                ) else {
                    continue;
                };
                let rel = Path::new(vendor).join(sub);
                if rel
                    .components()
                    .any(|c| !matches!(c, std::path::Component::Normal(_)))
                {
                    return Err(format!("{index}: unsafe sub_path {sub}"));
                }
                copy(&rel, &mut out)?;
                names.insert(name.to_string());
                let preset: serde_json::Value = serde_json::from_str(
                    &std::fs::read_to_string(src.join(&rel))
                        .map_err(|e| format!("{}: {e}", rel.display()))?,
                )
                .map_err(|e| format!("{}: {e}", rel.display()))?;
                if let Some(parent) = preset.get("inherits").and_then(|p| p.as_str())
                    && !parent.is_empty()
                {
                    parents.push((format!("{vendor}/{sub}"), parent.to_string()));
                }
                if list == "machine_model_list" {
                    let mut assets: Vec<String> = ["bed_model", "bed_texture"]
                        .iter()
                        .filter_map(|k| preset.get(*k).and_then(|v| v.as_str()))
                        .map(str::to_string)
                        .collect();
                    assets.push(format!("{name}_cover.png"));
                    for asset in assets {
                        let rel = Path::new(vendor).join(&asset);
                        // Plain file names only; a model may name an asset Orca does not ship.
                        if Path::new(&asset).components().count() == 1
                            && src.join(&rel).is_file()
                            && !dst.join(&rel).exists()
                        {
                            copy(&rel, &mut out)?;
                        }
                    }
                }
            }
        }
        out.vendors.push(vendor.to_string());
    }
    let missing: Vec<String> = parents
        .into_iter()
        .filter(|(_, parent)| !names.contains(parent))
        .map(|(file, parent)| format!("{file} inherits '{parent}'"))
        .collect();
    if !missing.is_empty() {
        return Err(format!(
            "the shipped vendors inherit from presets outside them (add their vendor): {}",
            missing.join("; ")
        ));
    }
    Ok(out)
}

/// The PrusaResearch vendor bundle: downloaded from Prusa once, never shipped.
pub mod prusa {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    /// The vendor's folder name (PrusaSlicer's `vendor/PrusaResearch`).
    pub const VENDOR: &str = "PrusaResearch";
    /// The pinned profile version (`upstream/prusa-profiles` at `c712aaf177`).
    pub const VERSION: &str = "2.5.12";
    /// Prusa's public profile repository for its own FFF printers, as PrusaSlicer 2.9 reads
    /// it (`resources/profiles/ArchiveRepositoryManifest.json`, `prusa-fff` → `url`; files
    /// are `<url><vendor>/<version>.ini` and `<url><vendor>/<asset>`,
    /// `PresetUpdater::priv::sync_config` and `get_or_copy_missing_resource`).
    pub const REPOSITORY: &str = "https://preset-repo-api.prusa3d.com/v1/repos/prusa-fff/";
    /// SHA-256 of the pinned bundle: as Prusa's repository serves it (it adds `thumbnail`
    /// lines to 16 older printer models; nothing that slices differently), and as the
    /// pinned git commit has it (`upstream/prusa-profiles/PrusaResearch/2.5.12.ini`).
    pub const BUNDLE_SHA256: [&str; 2] = [
        "60769af8affc66ec7d74a597318a1f9815e91984ccce28d44f13a1c2c88a4e59",
        "49ba89a77065ad2fae067a7676984159acb2e21f575482daa0b917c0a6d3df4f",
    ];
    /// The served bundle's size, for progress when the server sends no length.
    const BUNDLE_BYTES: u64 = 2_128_423;
    /// Nothing Mica downloads here is anywhere near this big.
    const MAX_FILE: u64 = 32 * 1024 * 1024;
    /// The bed models, textures and thumbnails the bundle names, with their SHA-256.
    const ASSETS_TSV: &str = include_str!("vendor_profiles/prusa_assets.tsv");

    /// What the GUI says wherever the Prusa printers would be.
    pub const DOWNLOAD_NOTE: &str = "Prusa printer profiles are downloaded from Prusa once.";

    /// The pinned files: the bundle's accepted hashes and the assets' names and hashes.
    #[derive(Clone, Debug)]
    pub struct Pins {
        pub bundle_sha256: Vec<String>,
        pub assets: Vec<(String, String)>,
    }

    impl Pins {
        /// The real pins of this build.
        pub fn pinned() -> Self {
            Self {
                bundle_sha256: BUNDLE_SHA256.iter().map(|s| s.to_string()).collect(),
                assets: ASSETS_TSV
                    .lines()
                    .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
                    .filter_map(|l| l.split_once('\t'))
                    .map(|(f, h)| (f.trim().to_string(), h.trim().to_string()))
                    .collect(),
            }
        }
    }

    /// Where the download is kept: `vendor-profiles/PrusaResearch` in Mica's settings folder
    /// (`MICA_CONFIG_DIR` when set).
    pub fn cache_dir() -> PathBuf {
        mica_core::user_data::config_dir()
            .join("vendor-profiles")
            .join(VENDOR)
    }

    fn bundle_file(dir: &Path) -> PathBuf {
        dir.join(format!("{VERSION}.ini"))
    }

    /// The hex SHA-256 of `bytes`.
    pub fn sha256_hex(bytes: &[u8]) -> String {
        Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    /// The downloaded bundle in `dir` when it is there and matches a pinned hash; a file that
    /// does not match is never used.
    pub fn cached_bundle_in(dir: &Path, pins: &Pins) -> Option<PathBuf> {
        let path = bundle_file(dir);
        let bytes = std::fs::read(&path).ok()?;
        let hash = sha256_hex(&bytes);
        pins.bundle_sha256.contains(&hash).then_some(path)
    }

    /// The downloaded bundle in Mica's settings folder, when it is there and intact.
    pub fn cached_bundle() -> Option<PathBuf> {
        cached_bundle_in(&cache_dir(), &Pins::pinned())
    }

    /// An installed PrusaSlicer's own (updated) bundle: none in an isolated process.
    pub fn installed_bundles() -> Vec<PathBuf> {
        let mut out = vec![];
        if let Some(home) = mica_core::user_data::home() {
            for p in [
                "Library/Application Support/PrusaSlicer/vendor/PrusaResearch.ini",
                ".config/PrusaSlicer/vendor/PrusaResearch.ini",
                ".var/app/com.prusa3d.PrusaSlicer/config/PrusaSlicer/vendor/PrusaResearch.ini",
            ] {
                out.push(home.join(p));
            }
        }
        if let Some(appdata) = mica_core::user_data::var_dir("APPDATA") {
            out.push(appdata.join("PrusaSlicer/vendor/PrusaResearch.ini"));
        }
        out
    }

    /// The Prusa bundle to read: `explicit` (`--bundle`), else `MICA_PRUSA_BUNDLE`, else a
    /// development checkout's pinned copy, else the downloaded one, else an installed
    /// PrusaSlicer's. `None`: it needs downloading ([`fetch`]).
    pub fn find_bundle(explicit: Option<&Path>) -> Option<PathBuf> {
        if let Some(p) = explicit {
            return Some(p.to_path_buf());
        }
        if let Some(p) = std::env::var_os("MICA_PRUSA_BUNDLE").filter(|p| !p.is_empty()) {
            return Some(PathBuf::from(p));
        }
        if let Some(upstream) = mica_core::user_data::upstream_dir() {
            let pinned = upstream.join("prusa-profiles/PrusaResearch/2.5.12.ini");
            if pinned.is_file() {
                return Some(pinned);
            }
        }
        if let Some(p) = cached_bundle() {
            return Some(p);
        }
        installed_bundles().into_iter().find(|p| p.is_file())
    }

    /// Download progress: the share done, 0 to 1.
    pub type Progress<'a> = &'a mut dyn FnMut(f32);

    /// What [`fetch`] got.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct Fetched {
        pub bundle: PathBuf,
        /// Bed models, textures and thumbnails in place (downloaded now or before).
        pub assets: usize,
        /// Assets that could not be downloaded or did not match; those printers get Mica's
        /// plain bed drawing.
        pub missing_assets: Vec<String>,
    }

    /// Downloads the pinned bundle (and its bed assets) from Prusa's repository into
    /// [`cache_dir`]. Blocking: call it off the UI thread.
    pub fn fetch(progress: Progress, cancel: &AtomicBool) -> Result<Fetched, String> {
        fetch_from(REPOSITORY, &cache_dir(), &Pins::pinned(), progress, cancel)
    }

    fn agent() -> ureq::Agent {
        let config = ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(15)))
            .timeout_recv_response(Some(Duration::from_secs(30)))
            .timeout_global(Some(Duration::from_secs(300)))
            .max_redirects(5)
            .http_status_as_error(false)
            .user_agent(format!("MicaSlicer/{}", env!("CARGO_PKG_VERSION")))
            .build();
        ureq::Agent::new_with_config(config)
    }

    fn network_error(e: &ureq::Error) -> String {
        let who = "Prusa's profile server";
        match e {
            ureq::Error::Timeout(_) => format!("{who} did not answer in time"),
            ureq::Error::HostNotFound => {
                format!("{who} could not be found (is this computer online?)")
            }
            ureq::Error::ConnectionFailed | ureq::Error::Io(_) => {
                format!("{who} could not be reached (is this computer online?)")
            }
            ureq::Error::Tls(_) | ureq::Error::Rustls(_) => {
                format!("{who}'s secure connection could not be verified")
            }
            other => format!("{who}: {other}"),
        }
    }

    /// One file into memory; `progress(received, total)` as it arrives.
    fn get(
        agent: &ureq::Agent,
        url: &str,
        progress: &mut dyn FnMut(u64, Option<u64>),
        cancel: &AtomicBool,
    ) -> Result<Vec<u8>, String> {
        let mut response = agent.get(url).call().map_err(|e| network_error(&e))?;
        let status = response.status().as_u16();
        if !(200..=299).contains(&status) {
            return Err(format!(
                "Prusa's profile server answered HTTP {status} for {url}"
            ));
        }
        let total = response
            .headers()
            .get("content-length")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok());
        let mut reader = response.body_mut().with_config().limit(MAX_FILE).reader();
        let mut data = Vec::with_capacity(total.unwrap_or(0).min(MAX_FILE) as usize);
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err("the download was canceled".into());
            }
            let n = reader
                .read(&mut buf)
                .map_err(|e| format!("the download broke off ({e})"))?;
            if n == 0 {
                break;
            }
            data.extend_from_slice(&buf[..n]);
            progress(data.len() as u64, total);
        }
        if total.is_some_and(|t| t != data.len() as u64) {
            return Err("the download ended early".into());
        }
        Ok(data)
    }

    /// Writes `bytes` to `path` through a temporary file next to it, so `path` is either
    /// complete or absent.
    fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
        let dir = path.parent().ok_or("no folder")?;
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let tmp = dir.join(format!(".{name}.part-{}", std::process::id()));
        let written = std::fs::write(&tmp, bytes)
            .and_then(|()| std::fs::File::open(&tmp)?.sync_all())
            .and_then(|()| std::fs::rename(&tmp, path));
        if let Err(e) = written {
            let _ = std::fs::remove_file(&tmp);
            return Err(format!("{}: {e}", path.display()));
        }
        Ok(())
    }

    fn file_matches(path: &Path, sha: &str) -> bool {
        std::fs::read(path).is_ok_and(|b| sha256_hex(&b) == sha)
    }

    /// [`fetch`] from repository `base` (ending in `/`) into `dir`, checked against `pins`.
    /// The bundle must match a pinned hash or nothing is written; assets that fail are
    /// listed in [`Fetched::missing_assets`].
    pub fn fetch_from(
        base: &str,
        dir: &Path,
        pins: &Pins,
        progress: Progress,
        cancel: &AtomicBool,
    ) -> Result<Fetched, String> {
        // The bundle is most of the bytes: 0–70 %, the assets the rest.
        const BUNDLE_SHARE: f32 = 0.7;
        let agent = agent();
        let bundle = bundle_file(dir);
        if cached_bundle_in(dir, pins).is_none() {
            let url = format!("{base}{VENDOR}/{VERSION}.ini");
            progress(0.);
            let bytes = get(
                &agent,
                &url,
                &mut |got, total| {
                    let total = total.unwrap_or(BUNDLE_BYTES).max(1);
                    progress(BUNDLE_SHARE * (got as f32 / total as f32).min(1.));
                },
                cancel,
            )?;
            let hash = sha256_hex(&bytes);
            if !pins.bundle_sha256.contains(&hash) {
                return Err(format!(
                    "the downloaded Prusa profiles are not version {VERSION} as Mica expects \
                     (SHA-256 {hash}); nothing was saved"
                ));
            }
            write_atomic(&bundle, &bytes)?;
        }
        let mut fetched = Fetched {
            bundle,
            assets: 0,
            missing_assets: vec![],
        };
        let n = pins.assets.len().max(1) as f32;
        for (i, (name, sha)) in pins.assets.iter().enumerate() {
            progress(BUNDLE_SHARE + (1. - BUNDLE_SHARE) * i as f32 / n);
            if cancel.load(Ordering::Relaxed) {
                return Err("the download was canceled".into());
            }
            let path = dir.join(name);
            if Path::new(name).components().count() != 1 {
                fetched.missing_assets.push(name.clone());
                continue;
            }
            if file_matches(&path, sha) {
                fetched.assets += 1;
                continue;
            }
            let got = get(
                &agent,
                &format!("{base}{VENDOR}/{name}"),
                &mut |_, _| {},
                cancel,
            )
            .and_then(|bytes| {
                if sha256_hex(&bytes) == *sha {
                    write_atomic(&path, &bytes)
                } else {
                    Err("checksum differs".into())
                }
            });
            match got {
                Ok(()) => fetched.assets += 1,
                Err(_) => fetched.missing_assets.push(name.clone()),
            }
        }
        progress(1.);
        Ok(fetched)
    }
}

#[cfg(test)]
#[path = "vendor_profiles_tests.rs"]
mod tests;
