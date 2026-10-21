// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The release tooling against the updater: `scripts/update-manifest.py` and
//! `scripts/release.sh --prebuilt` run on fake packages signed with a throwaway key in a
//! temporary folder, with `gh` and `npx` replaced by stubs that only log their arguments (no
//! network). The manifests they write must be what this crate reads and verifies.

use super::tests::{keypair, sign};
use super::*;
use std::process::{Command, Output};

const REPO_URL: &str = "https://github.com/steenfeldt/mica-slicer/releases/download";

/// `owner/name` of the repository in [`REPO_URL`].
fn repo() -> &'static str {
    REPO_URL
        .trim_start_matches("https://github.com/")
        .trim_end_matches("/releases/download")
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn have(tool: &str) -> bool {
    Command::new(tool)
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

struct TempDir(PathBuf);
impl TempDir {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("mica-release-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The SHA-256 of `path` from the system tool, if there is one.
fn sha256_of(path: &Path) -> Option<String> {
    [("shasum", &["-a", "256"][..]), ("sha256sum", &[][..])]
        .iter()
        .find_map(|(tool, args)| {
            let out = Command::new(tool).args(*args).arg(path).output().ok()?;
            out.status.success().then(|| {
                String::from_utf8_lossy(&out.stdout)
                    .split_whitespace()
                    .next()
                    .unwrap_or_default()
                    .to_string()
            })
        })
}

/// Fake update packages for `version` in `dir`, signed with `sk` (`.sig` next to each, as
/// `cargo packager signer sign` writes them), plus an unsigned `.dmg`.
fn packages(dir: &Path, version: &str, sk: &minisign::SecretKey) -> Vec<(&'static str, String)> {
    std::fs::create_dir_all(dir).unwrap();
    let files = [
        (
            "macos-aarch64",
            format!("MicaSlicer_{version}_arm64.app.tar.gz"),
        ),
        (
            "windows-x86_64",
            format!("MicaSlicer_{version}_x64-setup.exe"),
        ),
        (
            "linux-x86_64",
            format!("MicaSlicer_{version}_x86_64.AppImage"),
        ),
    ];
    for (platform, name) in &files {
        let data = format!("{platform} package of Mica Slicer {version}").repeat(100);
        std::fs::write(dir.join(name), &data).unwrap();
        std::fs::write(dir.join(format!("{name}.sig")), sign(sk, data.as_bytes())).unwrap();
    }
    std::fs::write(
        dir.join(format!("MicaSlicer_{version}_arm64.dmg")),
        "disk image",
    )
    .unwrap();
    files.to_vec()
}

/// Every platform of `manifest` has the GitHub asset URL, size, SHA-256 and a signature this
/// crate verifies against `pk`.
fn check_manifest(text: &str, dir: &Path, version: &str, pk: &str) {
    let json: serde_json::Value = serde_json::from_str(text).unwrap();
    assert_eq!(json["version"], version);
    let formats = [
        ("macos-aarch64", Format::App),
        ("windows-x86_64", Format::Nsis),
        ("linux-x86_64", Format::AppImage),
    ];
    for (platform, format) in formats {
        let release = parse_manifest(text, platform).unwrap();
        let name = release.url.rsplit('/').next().unwrap().to_string();
        assert_eq!(release.url, format!("{REPO_URL}/v{version}/{name}"));
        assert_eq!(release.format, format);
        assert_eq!(release.version, Version::parse(version).unwrap());
        let data = std::fs::read(dir.join(&name)).unwrap();
        verify(&data, &release.signature, &[pk]).unwrap();
        let entry = &json["platforms"][platform];
        assert_eq!(entry["size"], data.len() as u64);
        let sha = entry["sha256"].as_str().unwrap();
        assert_eq!(sha.len(), 64);
        if let Some(expected) = sha256_of(&dir.join(&name)) {
            assert_eq!(sha, expected);
        }
    }
}

fn generator(args: &[&str]) -> Output {
    Command::new("python3")
        .arg("-I")
        .arg(repo_root().join("scripts/update-manifest.py"))
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn manifest_generator_writes_what_the_updater_verifies() {
    if !have("python3") {
        eprintln!("skipped: no python3");
        return;
    }
    let tmp = TempDir::new("gen");
    let (pk, sk) = keypair();
    let dir = tmp.0.join("dist");
    let version = "0.4.0-alpha.2";
    let files = packages(&dir, version, &sk);
    let notes = tmp.0.join("notes.txt");
    std::fs::write(&notes, "Faster previews.\n").unwrap();
    let out = tmp.0.join("latest.json");
    let entries: Vec<String> = files
        .iter()
        .zip(["app", "nsis", "appimage"])
        .map(|((platform, name), format)| format!("{platform}={name}:{format}"))
        .collect();
    let mut args = vec![
        "--version",
        version,
        "--repo",
        repo(),
        "--dir",
        dir.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--notes-file",
        notes.to_str().unwrap(),
        "--pub-date",
        "2026-10-21T12:00:00Z",
    ];
    args.extend(entries.iter().map(String::as_str));
    let run = generator(&args);
    assert!(run.status.success(), "{run:?}");
    let text = std::fs::read_to_string(&out).unwrap();
    check_manifest(&text, &dir, version, &pk);
    let release = parse_manifest(&text, "linux-x86_64").unwrap();
    assert_eq!(release.notes, "Faster previews.");
    assert_eq!(release.date.as_deref(), Some("2026-10-21T12:00:00Z"));
    // The same inputs give the same file.
    assert!(generator(&args).status.success());
    assert_eq!(std::fs::read_to_string(&out).unwrap(), text);

    // Refusals: a missing signature, a missing package, a name GitHub would rename, a bad
    // version, an unknown format. None of them leaves a manifest behind.
    std::fs::remove_file(&out).unwrap();
    let unsigned = format!("{}.sig", files[2].1);
    std::fs::rename(dir.join(&unsigned), tmp.0.join("aside.sig")).unwrap();
    let run = generator(&args);
    assert!(!run.status.success());
    assert!(String::from_utf8_lossy(&run.stderr).contains("missing signature"));
    assert!(!out.exists());
    // A dry run may go without (and says so in the manifest).
    let mut dry = args.clone();
    dry.push("--allow-unsigned");
    assert!(generator(&dry).status.success());
    let release = parse_manifest(&std::fs::read_to_string(&out).unwrap(), "linux-x86_64").unwrap();
    assert_eq!(release.signature, "UNSIGNED-DRY-RUN");
    std::fs::remove_file(&out).unwrap();
    for (bad, why) in [
        ("linux-x86_64=nothing.AppImage:appimage", "missing package"),
        (
            "linux-x86_64=Mica Slicer.AppImage:appimage",
            "renamed by GitHub",
        ),
        ("macos-aarch64=x.tar.gz:dmg", "unknown package format"),
        ("plan9-mips=x:app", "unknown platform"),
    ] {
        let mut a = args[..12].to_vec();
        a.push(bad);
        let run = generator(&a);
        assert!(!run.status.success(), "{bad}");
        assert!(
            String::from_utf8_lossy(&run.stderr).contains(why),
            "{run:?}"
        );
    }
    let mut a = args.clone();
    a[1] = "v1";
    assert!(!generator(&a).status.success());
    assert!(!out.exists());
    // A local test server instead of GitHub (updater tests with a packaged build).
    let mut local = dry.clone();
    local.extend(["--asset-base", "http://127.0.0.1:8000/"]);
    assert!(generator(&local).status.success());
    let release = parse_manifest(&std::fs::read_to_string(&out).unwrap(), "macos-aarch64").unwrap();
    assert_eq!(
        release.url,
        format!("http://127.0.0.1:8000/MicaSlicer_{version}_arm64.app.tar.gz")
    );
}

/// Runs `scripts/release.sh` with stubbed `gh`/`npx` (logging to `log`, exiting `gh_exit`).
fn release_sh(tmp: &Path, pk: &str, gh_exit: u8, args: &[&str]) -> (Output, String) {
    release_sh_env(tmp, pk, gh_exit, args, &[])
}

/// [`release_sh`] with extra environment variables.
fn release_sh_env(
    tmp: &Path,
    pk: &str,
    gh_exit: u8,
    args: &[&str],
    env: &[(&str, &Path)],
) -> (Output, String) {
    let bin = tmp.join("bin");
    let log = tmp.join("stub.log");
    std::fs::create_dir_all(&bin).unwrap();
    let _ = std::fs::remove_file(&log);
    for tool in ["gh", "npx", "wrangler", "curl"] {
        let path = bin.join(tool);
        std::fs::write(
            &path,
            format!(
                "#!/bin/sh\necho \"{tool} $*\" >> '{}'\nexit {gh_exit}\n",
                log.display()
            ),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let mut path = bin.into_os_string();
    if let Some(cargo_dir) = std::env::var_os("CARGO")
        .map(PathBuf::from)
        .and_then(|c| c.parent().map(Path::to_path_buf))
    {
        path.push(":");
        path.push(cargo_dir);
    }
    path.push(":");
    path.push(std::env::var_os("PATH").unwrap_or_default());
    let out = Command::new(repo_root().join("scripts/release.sh"))
        .args(args)
        .env_remove("MICA_RELEASE_CHANGELOG")
        .envs(env.iter().map(|(k, v)| (*k, *v)))
        .env("PATH", path)
        .env("MICA_RELEASE_DIST", tmp.join("dist"))
        .env("MICA_RELEASE_SITE", tmp.join("site"))
        .env_remove("MICA_WEBSITE_DIR")
        .env("MICA_UPDATE_PUBKEY", pk)
        .env_remove("MICA_RELEASE_REPO")
        .env_remove("MICA_RELEASE_ASSET_BASE")
        .env_remove("CARGO_PACKAGER_SIGN_PRIVATE_KEY")
        .env_remove("CARGO_PACKAGER_SIGN_PRIVATE_KEY_PASSWORD")
        .output()
        .unwrap();
    (out, std::fs::read_to_string(&log).unwrap_or_default())
}

#[test]
fn release_script_prepares_then_publishes_github_first() {
    if !have("python3") || !have("bash") {
        eprintln!("skipped: no python3 or bash");
        return;
    }
    let tmp = TempDir::new("sh");
    let (pk, sk) = keypair();
    let version = env!("CARGO_PKG_VERSION");
    let dist = tmp.0.join("dist").join(version);
    packages(&dist, version, &sk);
    let site_manifest = tmp.0.join("site/updates/alpha/latest.json");

    // Default: writes the manifest into dist/ and only prints the commands.
    let (out, log) = release_sh(&tmp.0, &pk, 0, &["--prebuilt"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(log, "", "nothing may run without --publish");
    assert!(!site_manifest.exists());
    let gh_line = format!("gh release create v{version} --repo github.com/{}", repo());
    let gh_at = stdout.find(&gh_line).expect("the gh command");
    let deploy_at = stdout
        .find("CLOUDFLARE_ACCOUNT_ID=<account id> npx wrangler deploy")
        .expect("the deploy command");
    assert!(gh_at < deploy_at, "GitHub release first:\n{stdout}");
    // Always a pre-release (Mica is alpha), with CHANGELOG.md's section as its text.
    assert!(
        stdout[gh_at..].contains("--prerelease --notes-file"),
        "{stdout}"
    );
    let notes = changelog_section(version);
    assert_eq!(
        std::fs::read_to_string(dist.join("release-notes.md"))
            .unwrap()
            .trim_end(),
        notes
    );
    for name in [
        format!("MicaSlicer_{version}_arm64.app.tar.gz.sig"),
        format!("MicaSlicer_{version}_arm64.dmg"),
        format!("MicaSlicer_{version}_x86_64.AppImage"),
    ] {
        assert!(stdout.contains(&name), "{name} in\n{stdout}");
    }
    assert!(
        stdout.contains("Update signer: trusted main key #1"),
        "{stdout}"
    );
    let text = std::fs::read_to_string(dist.join("latest.json")).unwrap();
    check_manifest(&text, &dist, version, &pk);
    assert_eq!(
        parse_manifest(&text, "linux-x86_64").unwrap().notes,
        notes,
        "the manifest's notes are CHANGELOG.md's section too"
    );

    // The deploy command runs in the website repository (here: the site folder's parent).
    assert!(
        stdout.contains(&format!("cd {} && CLOUDFLARE_ACCOUNT_ID", tmp.0.display())),
        "{stdout}"
    );

    // Without the website checkout, --publish stops before anything is published.
    let (out, log) = release_sh(&tmp.0, &pk, 0, &["--prebuilt", "--publish"]);
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("MICA_WEBSITE_DIR"),
        "{out:?}"
    );
    assert_eq!(log, "", "nothing may run without the website checkout");
    std::fs::create_dir_all(tmp.0.join("site")).unwrap();

    // A failed GitHub release leaves the site alone.
    let (out, log) = release_sh(&tmp.0, &pk, 1, &["--prebuilt", "--publish"]);
    assert!(!out.status.success());
    assert!(log.starts_with(&gh_line), "{log}");
    assert!(!site_manifest.exists());

    // --publish: the GitHub release with every package and signature, then the manifest.
    let (out, log) = release_sh(&tmp.0, &pk, 0, &["--prebuilt", "--publish"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(log.lines().count(), 1, "only gh runs: {log}");
    assert!(log.starts_with(&gh_line), "{log}");
    assert!(
        log.contains(&format!(
            "--title Mica Slicer {version} --prerelease --notes-file"
        )),
        "{log}"
    );
    for f in std::fs::read_dir(&dist).unwrap() {
        let name = f.unwrap().file_name().to_string_lossy().into_owned();
        if name == "release-notes.md" {
            continue; // --notes-file
        }
        let asset = name.starts_with("MicaSlicer_");
        assert_eq!(log.contains(&format!("/{name}")), asset, "{name}: {log}");
    }
    let published = std::fs::read_to_string(&site_manifest).unwrap();
    check_manifest(&published, &dist, version, &pk);
    assert!(stdout.contains("npx wrangler deploy"), "{stdout}");

    // Dry-run packages are never published.
    let (out, log) = release_sh(&tmp.0, &pk, 0, &["--dry-run", "--publish"]);
    assert!(!out.status.success());
    assert_eq!(log, "");
    std::fs::write(dist.join("DRY-RUN-PACKAGES"), "").unwrap();
    let (out, log) = release_sh(&tmp.0, &pk, 0, &["--prebuilt", "--publish"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("--dry-run packages"));
    assert_eq!(log, "");
}

/// The `## <version>` section of the repository's CHANGELOG.md, as the app reads it.
fn changelog_section(version: &str) -> String {
    let text = std::fs::read_to_string(repo_root().join("CHANGELOG.md")).unwrap();
    let start = text
        .lines()
        .position(|l| {
            l.strip_prefix("## ")
                .and_then(|h| h.split_whitespace().next())
                == Some(version)
        })
        .unwrap_or_else(|| panic!("CHANGELOG.md has no ## {version}"));
    let body: Vec<&str> = text
        .lines()
        .skip(start + 1)
        .take_while(|l| !l.starts_with("## "))
        .collect();
    body.join("\n").trim().to_string()
}

#[test]
fn release_notes_come_from_the_changelog() {
    if !have("python3") || !have("bash") {
        eprintln!("skipped: no python3 or bash");
        return;
    }
    let tmp = TempDir::new("notes");
    let (pk, sk) = keypair();
    let version = env!("CARGO_PKG_VERSION");
    let dist = tmp.0.join("dist").join(version);
    packages(&dist, version, &sk);

    // A dry run prints the pre-release command with the changelog's notes.
    let (out, log) = release_sh(&tmp.0, &pk, 0, &["--dry-run", "--prebuilt"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(log, "");
    assert!(
        stdout.contains(&format!(
            "gh release create v{version} --repo github.com/{} --title Mica\\ Slicer\\ {version} \
             --prerelease --notes-file",
            repo()
        )),
        "{stdout}"
    );
    assert_eq!(
        std::fs::read_to_string(dist.join("release-notes.md"))
            .unwrap()
            .trim_end(),
        changelog_section(version)
    );

    // Without a section for the version: a release stops, a dry run uses a placeholder.
    let other = tmp.0.join("CHANGELOG.md");
    std::fs::write(&other, "# Changelog\n\n## 0.0.0-other\n\n- Something.\n").unwrap();
    let env = [("MICA_RELEASE_CHANGELOG", other.as_path())];
    let (out, log) = release_sh_env(&tmp.0, &pk, 0, &["--prebuilt"], &env);
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains(&format!("## {version}")),
        "{out:?}"
    );
    assert_eq!(log, "");
    let (out, _) = release_sh_env(&tmp.0, &pk, 0, &["--dry-run", "--prebuilt"], &env);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(
        std::fs::read_to_string(dist.join("release-notes.md")).unwrap(),
        format!("Mica Slicer {version} (alpha).\n")
    );
    // --notes is gone: the notes are written once, in CHANGELOG.md.
    let (out, _) = release_sh(&tmp.0, &pk, 0, &["--prebuilt", "--notes", "x.txt"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("CHANGELOG.md"));
}
