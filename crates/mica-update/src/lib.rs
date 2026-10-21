// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Mica Slicer's updater, with no UI: ask the website for the channel's manifest
//! (`https://micaslicer.com/updates/alpha/latest.json`), download the package for this
//! platform from where the manifest points (GitHub Releases, which redirects to its download
//! host), verify its minisign (Ed25519) signature against any public key compiled into the
//! app, and put it in place. The GUI runs these calls on a worker thread and shows every
//! outcome. A missing manifest (HTTP 404) or one older than this build means "no update".
//!
//! The manifest and signatures use cargo-packager-updater's format (`latest.json` with a
//! `platforms` map keyed `macos-aarch64`, `windows-x86_64`, …; each signature is the base64
//! of a minisign `.sig` file, as `cargo packager signer sign` writes it), so the
//! cargo-packager signer and its documentation apply unchanged. The download and install
//! steps are Mica's own (blocking `ureq`, already in the tree) so they never exit the
//! process, never panic, report progress, and refuse layouts they do not recognize.
//!
//! Package formats per platform:
//! - macOS `app`: a `.tar.gz` holding `Mica Slicer.app`; the running bundle is swapped for it
//!   (with an administrator prompt when its folder is not writable).
//! - Linux `appimage`: the `.AppImage` itself, written over `$APPIMAGE`.
//! - Windows `nsis`: the NSIS `-setup.exe`, run passively once Mica has quit.

use base64::Engine;
pub use semver::Version;
use serde::Deserialize;
use std::{
    collections::HashMap,
    io::Read,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

mod install;
pub use install::{Staged, Target, install, relaunch_after_exit, target_for};

/// Where update manifests live: the one place to change the server. Each channel has its
/// manifest at `<base>/<channel>/latest.json`, a static file of the website
/// (`site/updates/<channel>/latest.json` in the website repository). The manifest's package
/// URLs point to GitHub Releases assets (redirects are followed, within the same size limits and time budget).
pub const UPDATE_BASE_URL: &str = "https://micaslicer.com/updates";
/// The channel this build follows.
pub const CHANNEL: &str = "alpha";
/// Overrides the manifest URL (tests, a staging server). The signature check still uses the
/// keys compiled into the app, so this cannot make Mica install anything unsigned.
pub const URL_ENV: &str = "MICA_UPDATE_URL";
/// Enables the updater in a development build (one run from cargo's `target/` folder).
pub const FORCE_ENV: &str = "MICA_UPDATE_FORCE";

/// The embedded public key list (see `update-key.pub`).
const KEY_FILE: &str = include_str!("../update-key.pub");
const NO_KEYS: &str = "this build has no update signing key, so it cannot verify updates";

/// Upper bounds, so a broken server cannot exhaust memory.
const MAX_MANIFEST: u64 = 1 << 20;
const MAX_PACKAGE: u64 = 1 << 30;

/// Network time limits. They hold across redirects: connect and response limits apply to
/// every hop, the manifest's total budget to the whole chain.
#[derive(Clone, Copy, Debug)]
struct Timeouts {
    connect: Duration,
    response: Duration,
    /// The whole manifest request; a package may take a while on a slow line.
    manifest_total: Duration,
}

const TIMEOUTS: Timeouts = Timeouts {
    connect: Duration::from_secs(20),
    response: Duration::from_secs(30),
    manifest_total: Duration::from_secs(60),
};

#[cfg(test)]
thread_local! {
    /// Shorter limits for the tests on this thread.
    static TEST_TIMEOUTS: std::cell::Cell<Option<Timeouts>> = const { std::cell::Cell::new(None) };
}

fn timeouts() -> Timeouts {
    #[cfg(test)]
    if let Some(t) = TEST_TIMEOUTS.with(std::cell::Cell::get) {
        return t;
    }
    TIMEOUTS
}

/// The manifest URL of the channel: [`URL_ENV`] when set, else the official one.
pub fn manifest_url() -> String {
    match std::env::var(URL_ENV) {
        Ok(u) if !u.trim().is_empty() => u.trim().to_string(),
        _ => format!("{UPDATE_BASE_URL}/{CHANNEL}/latest.json"),
    }
}

/// Trusted public keys: a nonblank `MICA_UPDATE_PUBKEY` at build time overrides
/// `update-key.pub`. The file has one key per line; the override also accepts commas.
/// Blank lines and lines starting with `#` are ignored. The first key is the main key,
/// followed by backups; any one may sign an update. An empty list disables updates.
pub fn public_keys() -> Vec<&'static str> {
    configured_keys(KEY_FILE, option_env!("MICA_UPDATE_PUBKEY"))
}

fn configured_keys<'a>(file: &'a str, build_env: Option<&'a str>) -> Vec<&'a str> {
    match build_env.filter(|s| !s.trim().is_empty()) {
        Some(text) => parse_keys(text, true),
        None => parse_keys(file, false),
    }
}

fn parse_keys(text: &str, commas: bool) -> Vec<&str> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .flat_map(|l| l.split(move |c| commas && c == ','))
        .map(str::trim)
        .filter(|k| !k.is_empty() && !k.starts_with('#'))
        .collect()
}

/// This build's version (the workspace version).
pub fn current_version() -> Version {
    Version::parse(env!("CARGO_PKG_VERSION")).unwrap_or_else(|_| Version::new(0, 0, 0))
}

/// The manifest key of this platform: `macos-aarch64`, `linux-x86_64`, `windows-x86_64`.
pub fn platform() -> Option<String> {
    let os = if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else {
        return None;
    };
    let arch = if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        return None;
    };
    Some(format!("{os}-{arch}"))
}

/// Whether `exe` runs from a cargo build folder (an ancestor named `target` with cargo's
/// `CACHEDIR.TAG`): a development build, which must never replace itself.
pub fn is_dev_build(exe: &Path) -> bool {
    exe.ancestors()
        .any(|a| a.file_name().is_some_and(|n| n == "target") && a.join("CACHEDIR.TAG").exists())
}

/// Whether [`FORCE_ENV`] asks for the updater in a development build.
pub fn forced() -> bool {
    std::env::var(FORCE_ENV).is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
}

/// What a package is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    App,
    AppImage,
    Nsis,
}

/// A newer release for this platform.
#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub version: Version,
    /// Release notes ("What's new"), plain text; may be empty.
    pub notes: String,
    /// RFC 3339 publication date, if the manifest gives one.
    pub date: Option<String>,
    pub url: String,
    /// Base64 of the minisign signature file.
    pub signature: String,
    pub format: Format,
}

#[derive(Deserialize)]
struct Manifest {
    version: String,
    #[serde(default)]
    notes: Option<String>,
    #[serde(default)]
    pub_date: Option<String>,
    #[serde(default)]
    platforms: HashMap<String, Entry>,
    // The single-platform shape.
    #[serde(flatten)]
    single: Option<Entry>,
}

#[derive(Clone, Deserialize)]
struct Entry {
    url: String,
    signature: String,
    format: Format,
}

/// Reads a manifest for `platform`.
pub fn parse_manifest(text: &str, platform: &str) -> Result<Release, String> {
    let (m, version) = read_manifest(text)?;
    release_for(m, version, platform)
}

fn read_manifest(text: &str) -> Result<(Manifest, Version), String> {
    let m: Manifest = serde_json::from_str(text)
        .map_err(|e| format!("the update server sent an unreadable manifest ({e})"))?;
    let version = Version::parse(m.version.trim().trim_start_matches('v')).map_err(|_| {
        format!(
            "the update server named an invalid version \"{}\"",
            m.version
        )
    })?;
    Ok((m, version))
}

fn release_for(m: Manifest, version: Version, platform: &str) -> Result<Release, String> {
    let entry = match m.platforms.get(platform) {
        Some(e) => e.clone(),
        None if m.platforms.is_empty() => m
            .single
            .ok_or("the update manifest names no package for this computer")?,
        None => {
            return Err(format!(
                "Mica Slicer {version} has no package for this computer ({platform})"
            ));
        }
    };
    Ok(Release {
        version,
        notes: m.notes.unwrap_or_default().trim().to_string(),
        date: m.pub_date,
        url: entry.url,
        signature: entry.signature,
        format: entry.format,
    })
}

/// Checks a minisign signature (base64 of the `.sig` file) of `data` against any trusted
/// public key (base64 of each `.pub` file, as `cargo packager signer generate` writes it).
pub fn verify(data: &[u8], signature: &str, public_keys: &[impl AsRef<str>]) -> Result<(), String> {
    if public_keys.is_empty() {
        return Err(NO_KEYS.into());
    }
    let b64 = |s: &str, what: &str| -> Result<String, String> {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(s.trim())
            .map_err(|_| format!("the update's {what} is not valid base64"))?;
        String::from_utf8(bytes).map_err(|_| format!("the update's {what} is not text"))
    };
    let sig = minisign_verify::Signature::decode(&b64(signature, "signature")?)
        .map_err(|_| "the update's signature is malformed".to_string())?;
    let mut valid_key = false;
    let mut bad_signature = false;
    for public_key in public_keys {
        let key = b64(public_key.as_ref(), "public key")
            .ok()
            .and_then(|text| minisign_verify::PublicKey::decode(&text).ok());
        let Some(key) = key else { continue };
        valid_key = true;
        // Legacy (non-prehashed) signatures as cargo-packager-updater accepts them.
        match key.verify(data, &sig, true) {
            Ok(()) => return Ok(()),
            Err(minisign_verify::Error::UnexpectedKeyId) => {}
            Err(_) => bad_signature = true,
        }
    }
    Err(if bad_signature {
        "the update's signature does not match the download (it may be damaged or tampered with)"
    } else if valid_key {
        "the update was signed with a different key than this build trusts"
    } else {
        "this build's update keys are invalid"
    }
    .into())
}

/// Talks to one update server with a list of trusted keys.
#[derive(Clone, Debug)]
pub struct Client {
    pub manifest_url: String,
    pub public_keys: Vec<String>,
    pub platform: String,
}

impl Client {
    /// The official server (or [`URL_ENV`]) with the keys compiled into this build.
    pub fn official() -> Result<Self, String> {
        let public_keys = public_keys();
        if public_keys.is_empty() {
            return Err(NO_KEYS.into());
        }
        let platform = platform().ok_or("updates are not offered for this kind of computer")?;
        Ok(Self {
            manifest_url: manifest_url(),
            public_keys: public_keys.into_iter().map(str::to_string).collect(),
            platform,
        })
    }

    fn agent(download: bool) -> ureq::Agent {
        let t = timeouts();
        let config = ureq::Agent::config_builder()
            .timeout_connect(Some(t.connect))
            .timeout_recv_response(Some(t.response))
            // A manifest is small; a package may take a while on a slow line.
            .timeout_global((!download).then_some(t.manifest_total))
            .max_redirects(10)
            .http_status_as_error(false)
            .user_agent(format!("Mica Slicer/{}", env!("CARGO_PKG_VERSION")))
            .build();
        ureq::Agent::new_with_config(config)
    }

    /// Asks for the manifest: `Ok(None)` when this build is up to date, when the manifest is
    /// older than this build (the placeholder before the first release), or when the server
    /// has none (404 Not Found, e.g. a channel without a release yet; 204 No Content).
    pub fn check(&self, current: &Version) -> Result<Option<Release>, String> {
        if self.public_keys.is_empty() {
            return Err(NO_KEYS.into());
        }
        let mut response = Self::agent(false)
            .get(&self.manifest_url)
            .header("Accept", "application/json")
            .call()
            .map_err(|e| network_error(&e, "the update server"))?;
        match response.status().as_u16() {
            204 | 404 => return Ok(None),
            200..=299 => {}
            s => return Err(format!("the update server answered HTTP {s}")),
        }
        let text = response
            .body_mut()
            .with_config()
            .limit(MAX_MANIFEST)
            .read_to_string()
            .map_err(|e| network_error(&e, "the update server"))?;
        let (manifest, version) = read_manifest(&text)?;
        if version <= *current {
            return Ok(None);
        }
        release_for(manifest, version, &self.platform).map(Some)
    }

    /// Downloads `release` and verifies its signature; `progress(received, total)` is called as
    /// bytes arrive. Setting `cancel` stops the download with an error.
    pub fn download(
        &self,
        release: &Release,
        progress: &mut dyn FnMut(u64, Option<u64>),
        cancel: &AtomicBool,
    ) -> Result<Vec<u8>, String> {
        if self.public_keys.is_empty() {
            return Err(NO_KEYS.into());
        }
        let mut response = Self::agent(true)
            .get(&release.url)
            .header("Accept", "application/octet-stream")
            .call()
            .map_err(|e| network_error(&e, "the download server"))?;
        let status = response.status().as_u16();
        if !(200..=299).contains(&status) {
            return Err(format!("the download server answered HTTP {status}"));
        }
        let total = response
            .headers()
            .get("content-length")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok());
        if total.is_some_and(|t| t > MAX_PACKAGE) {
            return Err("the update package is unreasonably large".into());
        }
        let mut reader = response
            .body_mut()
            .with_config()
            .limit(MAX_PACKAGE)
            .reader();
        let mut data = Vec::with_capacity(total.unwrap_or(0) as usize);
        let mut buf = vec![0u8; 64 * 1024];
        progress(0, total);
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
        verify(&data, &release.signature, &self.public_keys)?;
        Ok(data)
    }
}

fn network_error(e: &ureq::Error, who: &str) -> String {
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
        ureq::Error::TooManyRedirects => format!("{who} redirected too many times"),
        ureq::Error::BodyExceedsLimit(_) => format!("{who} sent more data than expected"),
        other => format!("{who}: {other}"),
    }
}

/// Download progress as a fraction, when the size is known.
pub fn fraction(received: u64, total: Option<u64>) -> Option<f32> {
    total
        .filter(|t| *t > 0)
        .map(|t| (received as f64 / t as f64).clamp(0., 1.) as f32)
}

/// Where a staged package or the old app goes while swapping: next to `path`, hidden, named
/// after this process so two runs never collide.
fn sibling(path: &Path, tag: &str) -> PathBuf {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    path.with_file_name(format!(".{name}.{tag}-{}", std::process::id()))
}

#[cfg(all(test, unix))]
mod release_tests;
#[cfg(test)]
mod tests;
