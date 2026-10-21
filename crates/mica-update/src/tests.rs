// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! End to end on loopback: throwaway minisign keys made in memory (never real ones), small
//! HTTP servers standing in for the website (the manifest) and GitHub Releases (packages
//! behind a redirect to another host, as GitHub sends downloads to its object storage), and
//! the client checking, downloading, verifying and refusing tampered packages and signatures.

use super::*;
use base64::Engine;
use std::{
    io::{BufRead, BufReader, Write},
    net::{TcpListener, TcpStream},
    sync::{Arc, Mutex},
    thread,
};

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

/// A throwaway key pair: (public key as the updater embeds it, secret key).
pub(crate) fn keypair() -> (String, minisign::SecretKey) {
    let kp = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
    let pk = kp.pk.to_box().unwrap().to_string();
    (B64.encode(pk), kp.sk)
}

/// A signature the way `cargo packager signer sign` writes it.
pub(crate) fn sign(sk: &minisign::SecretKey, data: &[u8]) -> String {
    let sig = minisign::sign(
        None,
        sk,
        std::io::Cursor::new(data),
        Some("timestamp:0\tfile:test"),
        Some("signature from cargo-packager secret key"),
    )
    .unwrap();
    B64.encode(sig.to_string())
}

type Route = (u16, Vec<(String, String)>, Vec<u8>);

/// Serves `routes` (path → status, headers, body) on 127.0.0.1 until dropped; records the
/// paths asked for.
struct Server {
    port: u16,
    routes: Arc<Mutex<HashMap<String, Route>>>,
    log: Arc<Mutex<Vec<String>>>,
}
impl Server {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let routes: Arc<Mutex<HashMap<String, Route>>> = Arc::default();
        let log: Arc<Mutex<Vec<String>>> = Arc::default();
        let (r, l) = (routes.clone(), log.clone());
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { break };
                let (r, l) = (r.clone(), l.clone());
                thread::spawn(move || serve(stream, &r, &l));
            }
        });
        Self { port, routes, log }
    }
    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }
    fn route(&self, path: &str, status: u16, headers: &[(&str, &str)], body: &[u8]) {
        self.routes.lock().unwrap().insert(
            path.into(),
            (
                status,
                headers
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
                body.to_vec(),
            ),
        );
    }
}

fn serve(stream: TcpStream, routes: &Mutex<HashMap<String, Route>>, log: &Mutex<Vec<String>>) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() {
        return;
    }
    let path = line.split_whitespace().nth(1).unwrap_or("/").to_string();
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h).is_err() || h.trim().is_empty() {
            break;
        }
    }
    log.lock().unwrap().push(path.clone());
    let (status, headers, body) =
        routes
            .lock()
            .unwrap()
            .get(&path)
            .cloned()
            .unwrap_or((404, vec![], b"not found".to_vec()));
    let mut out = stream;
    let mut head = format!("HTTP/1.1 {status} X\r\nConnection: close\r\n");
    // A route may declare its own (false) length.
    if !headers
        .iter()
        .any(|(k, _)| k.eq_ignore_ascii_case("content-length"))
    {
        head.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    for (k, v) in headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    let _ = out.write_all(head.as_bytes());
    let _ = out.write_all(&body);
}

fn manifest(version: &str, url: &str, signature: &str, format: &str) -> String {
    let platform = platform().unwrap();
    serde_json::json!({
        "version": version,
        "notes": "Faster previews.\nFixed the seam on cylinders.",
        "pub_date": "2026-10-19T12:00:00Z",
        "platforms": {
            platform: { "url": url, "signature": signature, "format": format },
            "windows-x86_64": { "url": "https://example.invalid/x.exe", "signature": "x", "format": "nsis" }
        }
    })
    .to_string()
}

fn this_format() -> &'static str {
    if cfg!(target_os = "macos") {
        "app"
    } else if cfg!(windows) {
        "nsis"
    } else {
        "appimage"
    }
}

#[test]
fn check_download_verify_and_refuse_tampering() {
    let (pk, sk) = keypair();
    let (backup_pk, backup_sk) = keypair();
    let server = Server::start();
    let package = b"the new Mica Slicer".repeat(5000);
    let sig = sign(&sk, &package);
    // The manifest and the package each sit behind a redirect.
    server.route(
        "/updates/alpha/latest.json",
        302,
        &[("Location", "/moved/alpha/latest.json")],
        b"",
    );
    server.route(
        "/moved/alpha/latest.json",
        200,
        &[("Content-Type", "application/json")],
        manifest("v0.2.0", &server.url("/dl/pkg"), &sig, this_format()).as_bytes(),
    );
    server.route("/dl/pkg", 301, &[("Location", "/objects/pkg")], b"");
    server.route("/objects/pkg", 200, &[], &package);
    let client = Client {
        manifest_url: server.url("/updates/alpha/latest.json"),
        public_keys: vec![pk, backup_pk],
        platform: platform().unwrap(),
    };
    // Newer than 0.1.0: offered, with its notes.
    let release = client
        .check(&Version::new(0, 1, 0))
        .unwrap()
        .expect("an update");
    assert_eq!(release.version, Version::new(0, 2, 0));
    assert!(release.notes.starts_with("Faster previews."));
    assert_eq!(release.date.as_deref(), Some("2026-10-19T12:00:00Z"));
    // Same or newer installed: nothing.
    assert_eq!(client.check(&Version::new(0, 2, 0)).unwrap(), None);
    assert_eq!(client.check(&Version::new(1, 0, 0)).unwrap(), None);
    // A pre-release of 0.2.0 is older than 0.2.0.
    assert!(
        client
            .check(&Version::parse("0.2.0-beta.1").unwrap())
            .unwrap()
            .is_some()
    );
    // Download with progress; the signature checks out.
    let mut seen = vec![];
    let cancel = AtomicBool::new(false);
    let data = client
        .download(&release, &mut |got, total| seen.push((got, total)), &cancel)
        .unwrap();
    assert_eq!(data, package);
    assert_eq!(
        seen.last(),
        Some(&(package.len() as u64, Some(package.len() as u64)))
    );
    assert!(seen.len() > 1);
    // The same build accepts an emergency release signed by its backup key.
    let mut backup_release = release.clone();
    backup_release.signature = sign(&backup_sk, &package);
    assert_eq!(
        client
            .download(&backup_release, &mut |_, _| {}, &cancel)
            .unwrap(),
        package
    );
    // A tampered package is refused.
    let mut bad = package.clone();
    bad[100] ^= 1;
    server.route("/objects/pkg", 200, &[], &bad);
    let e = client
        .download(&release, &mut |_, _| {}, &cancel)
        .unwrap_err();
    assert!(e.contains("does not match"), "{e}");
    // So is a package signed with another key (a valid signature, wrong signer).
    let (_, other) = keypair();
    server.route("/objects/pkg", 200, &[], &package);
    let mut forged = release.clone();
    forged.signature = sign(&other, &package);
    let e = client
        .download(&forged, &mut |_, _| {}, &cancel)
        .unwrap_err();
    assert!(e.contains("different key"), "{e}");
    // A garbled signature too.
    forged.signature = B64.encode("untrusted comment: x\nnot a signature");
    let e = client
        .download(&forged, &mut |_, _| {}, &cancel)
        .unwrap_err();
    assert!(e.contains("malformed"), "{e}");
    // And a manifest whose signature was swapped for another file's.
    let mut swapped = release.clone();
    swapped.signature = sign(&sk, b"some other file");
    let e = client
        .download(&swapped, &mut |_, _| {}, &cancel)
        .unwrap_err();
    assert!(e.contains("does not match"), "{e}");
    // Cancel stops it.
    cancel.store(true, Ordering::Relaxed);
    let e = client
        .download(&release, &mut |_, _| {}, &cancel)
        .unwrap_err();
    assert!(e.contains("canceled"), "{e}");
    let log = server.log.lock().unwrap().clone();
    assert!(
        log.contains(&"/moved/alpha/latest.json".to_string()),
        "{log:?}"
    );
    assert!(log.contains(&"/objects/pkg".to_string()), "{log:?}");
}

#[test]
fn server_failures_are_explained() {
    let (pk, _) = keypair();
    let server = Server::start();
    let client = |path: &str| Client {
        manifest_url: server.url(path),
        public_keys: vec![pk.clone()],
        platform: platform().unwrap(),
    };
    let v = Version::new(0, 1, 0);
    // 204, 404 (no manifest yet, also behind a redirect): nothing new, no error.
    server.route("/none", 204, &[], b"");
    assert_eq!(client("/none").check(&v).unwrap(), None);
    assert_eq!(client("/missing").check(&v).unwrap(), None);
    server.route("/gone", 302, &[("Location", "/missing")], b"");
    assert_eq!(client("/gone").check(&v).unwrap(), None);
    // Other failures are errors: a server error, broken JSON, a manifest without this
    // platform, a bad version.
    server.route("/down", 503, &[], b"maintenance");
    let e = client("/down").check(&v).unwrap_err();
    assert!(e.contains("HTTP 503"), "{e}");
    server.route("/broken", 200, &[], b"{nope");
    let e = client("/broken").check(&v).unwrap_err();
    assert!(e.contains("unreadable manifest"), "{e}");
    server.route(
        "/other",
        200,
        &[],
        br#"{"version":"9.0.0","platforms":{"plan9-mips":{"url":"http://x","signature":"x","format":"app"}}}"#,
    );
    let e = client("/other").check(&v).unwrap_err();
    assert!(e.contains("no package for this computer"), "{e}");
    // Not newer than this build: no update, whether or not it has a package for this computer.
    assert_eq!(
        client("/other").check(&Version::new(9, 0, 0)).unwrap(),
        None
    );
    server.route("/badver", 200, &[], br#"{"version":"soon","platforms":{}}"#);
    let e = client("/badver").check(&v).unwrap_err();
    assert!(e.contains("invalid version"), "{e}");
    // A redirect loop.
    server.route("/loop", 302, &[("Location", "/loop")], b"");
    let e = client("/loop").check(&v).unwrap_err();
    assert!(e.contains("redirected too many times"), "{e}");
    // Nobody listening.
    let dead = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = dead.local_addr().unwrap().port();
    drop(dead);
    let c = Client {
        manifest_url: format!("http://127.0.0.1:{port}/updates/alpha/latest.json"),
        public_keys: vec![pk.clone()],
        platform: platform().unwrap(),
    };
    let e = c.check(&v).unwrap_err();
    assert!(e.contains("could not be reached"), "{e}");
}

/// Accepts connections and never answers (until the test process ends).
fn silent_server() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        let mut held = vec![];
        for stream in listener.incoming() {
            let Ok(stream) = stream else { break };
            held.push(stream);
        }
    });
    format!("http://127.0.0.1:{port}")
}

#[test]
fn website_manifest_and_github_download_on_another_host() {
    let (pk, sk) = keypair();
    // The website serves the manifest itself; GitHub answers a release download with a
    // redirect to another host (its object storage), with a signed query string.
    let site = Server::start();
    let github = Server::start();
    let objects = Server::start();
    let package = b"Mica Slicer from GitHub".repeat(3000);
    let asset = "/owner/mica-slicer/releases/download/v0.3.0/MicaSlicer_0.3.0.pkg";
    let stored = "/github-production-release-asset/1234?sig=abc&response-content-type=application%2Foctet-stream";
    site.route(
        "/updates/alpha/latest.json",
        200,
        &[("Content-Type", "application/json")],
        manifest(
            "0.3.0",
            &github.url(asset),
            &sign(&sk, &package),
            this_format(),
        )
        .as_bytes(),
    );
    let location = objects.url(stored);
    github.route(asset, 302, &[("Location", &location)], b"");
    objects.route(stored, 200, &[], &package);
    let client = Client {
        manifest_url: site.url("/updates/alpha/latest.json"),
        public_keys: vec![pk],
        platform: platform().unwrap(),
    };
    let release = client.check(&Version::new(0, 2, 0)).unwrap().unwrap();
    assert_eq!(release.url, github.url(asset));
    let cancel = AtomicBool::new(false);
    let data = client.download(&release, &mut |_, _| {}, &cancel).unwrap();
    assert_eq!(data, package);
    assert_eq!(github.log.lock().unwrap().clone(), vec![asset.to_string()]);
    assert_eq!(
        objects.log.lock().unwrap().clone(),
        vec![stored.to_string()]
    );

    // The size limits still hold behind the redirect: a package that says it is too large,
    // and an oversized manifest.
    objects.route(stored, 200, &[("Content-Length", "2147483648")], b"x");
    let e = client
        .download(&release, &mut |_, _| {}, &cancel)
        .unwrap_err();
    assert!(e.contains("unreasonably large"), "{e}");
    let big = vec![b' '; (MAX_MANIFEST + 1) as usize];
    objects.route("/big.json", 200, &[], &big);
    let big_url = objects.url("/big.json");
    site.route("/big", 302, &[("Location", &big_url)], b"");
    let mut c = client.clone();
    c.manifest_url = site.url("/big");
    let e = c.check(&Version::new(0, 2, 0)).unwrap_err();
    assert!(e.contains("more data than expected"), "{e}");
    // A download that is missing on GitHub is an error (unlike a missing manifest).
    github.route(asset, 404, &[], b"Not Found");
    let e = client
        .download(&release, &mut |_, _| {}, &cancel)
        .unwrap_err();
    assert!(e.contains("HTTP 404"), "{e}");
}

#[test]
fn time_limits_hold_across_redirects() {
    let short = Timeouts {
        connect: Duration::from_millis(500),
        response: Duration::from_millis(300),
        manifest_total: Duration::from_millis(800),
    };
    TEST_TIMEOUTS.with(|t| t.set(Some(short)));
    let (pk, _) = keypair();
    let server = Server::start();
    let silent = silent_server();
    let to_silent = format!("{silent}/hang");
    server.route(
        "/updates/alpha/latest.json",
        302,
        &[("Location", &to_silent)],
        b"",
    );
    server.route("/dl", 302, &[("Location", &to_silent)], b"");
    let client = Client {
        manifest_url: server.url("/updates/alpha/latest.json"),
        public_keys: vec![pk],
        platform: platform().unwrap(),
    };
    let started = std::time::Instant::now();
    let e = client.check(&Version::new(0, 1, 0)).unwrap_err();
    assert!(e.contains("did not answer in time"), "{e}");
    let release = Release {
        version: Version::new(9, 0, 0),
        notes: String::new(),
        date: None,
        url: server.url("/dl"),
        signature: String::new(),
        format: Format::App,
    };
    let e = client
        .download(&release, &mut |_, _| {}, &AtomicBool::new(false))
        .unwrap_err();
    assert!(e.contains("did not answer in time"), "{e}");
    assert!(started.elapsed() < Duration::from_secs(10));
    TEST_TIMEOUTS.with(|t| t.set(None));
}

#[test]
fn alpha_channel_on_the_website_and_the_placeholder_manifest() {
    assert_eq!(CHANNEL, "alpha");
    assert_eq!(UPDATE_BASE_URL, "https://micaslicer.com/updates");
    if std::env::var(URL_ENV).is_err() {
        assert_eq!(
            manifest_url(),
            "https://micaslicer.com/updates/alpha/latest.json"
        );
    }
    // The placeholder the website ships before the first release (the website repository's
    // site/updates/alpha/latest.json, same shape as the release script's manifest) means
    // "no update", not an error.
    let placeholder = r#"{
  "version": "0.0.0",
  "notes": "No release yet. scripts/release.sh --publish replaces this placeholder with the real manifest.",
  "platforms": {}
}
"#;
    let (pk, _) = keypair();
    let server = Server::start();
    server.route(
        "/updates/alpha/latest.json",
        200,
        &[],
        placeholder.as_bytes(),
    );
    let client = Client {
        manifest_url: server.url("/updates/alpha/latest.json"),
        public_keys: vec![pk],
        platform: platform().unwrap(),
    };
    assert_eq!(client.check(&Version::new(0, 0, 0)).unwrap(), None);
    assert_eq!(client.check(&current_version()).unwrap(), None);
    // That the website serves /updates/* with `Cache-Control: no-cache` is the website
    // repository's concern (its site/_headers; docs/release.md).
}

#[test]
fn manifest_shapes() {
    // The single-platform shape cargo-packager-updater also accepts.
    let r = parse_manifest(
        r#"{"version":"1.2.3","url":"https://x/y.tar.gz","signature":"s","format":"app"}"#,
        "macos-aarch64",
    )
    .unwrap();
    assert_eq!(r.version, Version::new(1, 2, 3));
    assert_eq!(r.format, Format::App);
    assert_eq!(r.notes, "");
    let r = parse_manifest(
        r#"{"version":"0.3.0","platforms":{"linux-x86_64":{"url":"u","signature":"s","format":"appimage"}}}"#,
        "linux-x86_64",
    )
    .unwrap();
    assert_eq!(r.format, Format::AppImage);
    assert!(parse_manifest(r#"{"version":"1.0.0"}"#, "linux-x86_64").is_err());
}

#[test]
fn key_file_and_build_env_lists() {
    // The committed list: the main key, then the offline backup; both decode.
    let committed = parse_keys(KEY_FILE, false);
    assert_eq!(committed.len(), 2);
    for key in &committed {
        let raw = base64::engine::general_purpose::STANDARD
            .decode(key)
            .unwrap();
        let text = String::from_utf8(raw).unwrap();
        assert!(minisign_verify::PublicKey::decode(&text).is_ok());
    }
    let (main, _) = keypair();
    let (backup, _) = keypair();
    let file = format!("# Main, then backups\n\n  {main} \r\n  # Backup\r\n\t{backup}\t\n");
    let expected = vec![main.as_str(), backup.as_str()];
    assert_eq!(configured_keys(&file, None), expected);
    assert_eq!(configured_keys(&file, Some(" \r\n\t")), expected);
    for env in [
        format!(" {main}\r\n\n {backup} \n"),
        format!(" , {main},, {backup}, "),
        format!(" # Comment, ignored\n{main},\n  # Backup\n{backup}\n"),
    ] {
        // The build override replaces the file, rather than appending to it.
        assert_eq!(configured_keys("unused", Some(&env)), expected);
    }
    assert_eq!(configured_keys(&file, Some(&main)), vec![main.as_str()]);
    assert!(configured_keys(&file, Some("# No keys\n , , ")).is_empty());
    assert!(configured_keys("# No keys\n\n", None).is_empty());
}

#[test]
fn any_trusted_key_verifies_and_empty_list_refuses() {
    let (main, main_sk) = keypair();
    let (backup, backup_sk) = keypair();
    let (_, unrelated_sk) = keypair();
    let keys = vec![main.as_str(), backup.as_str()];
    let data = b"a release with a new embedded main key";
    let main_sig = sign(&main_sk, data);
    let backup_sig = sign(&backup_sk, data);
    verify(data, &main_sig, &keys).unwrap();
    verify(data, &backup_sig, &keys).unwrap();
    verify(data, &main_sig, &[main.as_str()]).unwrap();
    assert!(
        verify(data, &sign(&unrelated_sk, data), &keys)
            .unwrap_err()
            .contains("different key")
    );
    // A malformed entry does not prevent trying a valid backup (or allow a forgery).
    let malformed = ["invalid key", backup.as_str()];
    verify(data, &backup_sig, &malformed).unwrap();
    assert!(verify(data, &main_sig, &malformed).is_err());
    assert!(
        verify(data, &backup_sig, &["invalid key"])
            .unwrap_err()
            .contains("keys are invalid")
    );
    // Preserve the tampering diagnostic even when later keys have a different ID.
    for signature in [&main_sig, &backup_sig] {
        assert!(
            verify(b"tampered", signature, &keys)
                .unwrap_err()
                .contains("does not match")
        );
    }
    assert!(
        verify(data, "not base64!", &keys)
            .unwrap_err()
            .contains("base64")
    );
    assert!(
        verify(data, &B64.encode("not a signature"), &keys)
            .unwrap_err()
            .contains("malformed")
    );
    let empty: [&str; 0] = [];
    assert_eq!(verify(data, &backup_sig, &empty).unwrap_err(), NO_KEYS);
    let client = Client {
        manifest_url: "http://127.0.0.1:1/unused".into(),
        public_keys: vec![],
        platform: platform().unwrap(),
    };
    assert_eq!(client.check(&Version::new(0, 0, 0)).unwrap_err(), NO_KEYS);
    let release = parse_manifest(
        &manifest("1.0.0", &client.manifest_url, &backup_sig, this_format()),
        &client.platform,
    )
    .unwrap();
    assert_eq!(
        client
            .download(
                &release,
                &mut |_, _| panic!("must not download"),
                &AtomicBool::new(false)
            )
            .unwrap_err(),
        NO_KEYS
    );
    // Rotation is just a new trust list: the old main loses access, the backup remains.
    let (new_main, new_main_sk) = keypair();
    let rotated = [new_main.as_str(), backup.as_str()];
    verify(data, &sign(&new_main_sk, data), &rotated).unwrap();
    verify(data, &backup_sig, &rotated).unwrap();
    assert!(verify(data, &main_sig, &rotated).is_err());
}

#[test]
fn key_file_and_build_detection() {
    // The official client refuses to run without a key (a build made without one).
    if public_keys().is_empty() {
        assert!(
            Client::official()
                .unwrap_err()
                .contains("no update signing key")
        );
    }
    assert!(manifest_url().ends_with("/latest.json"));
    let dir = std::env::temp_dir().join(format!("mica-update-dev-{}", std::process::id()));
    let target = dir.join("target");
    std::fs::create_dir_all(target.join("release")).unwrap();
    let exe = target.join("release").join("mica-gui");
    assert!(!is_dev_build(&exe), "no CACHEDIR.TAG yet");
    std::fs::write(
        target.join("CACHEDIR.TAG"),
        "Signature: 8a477f597d28d172789f06886806bc55",
    )
    .unwrap();
    assert!(is_dev_build(&exe));
    assert!(!is_dev_build(Path::new(
        "/Applications/Mica Slicer.app/Contents/MacOS/mica-gui"
    )));
    // The test binary itself runs from target/.
    assert!(is_dev_build(&std::env::current_exe().unwrap()));
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(fraction(5, Some(10)), Some(0.5));
    assert_eq!(fraction(5, None), None);
}

/// A fake `Mica Slicer.app` with `marker` in its executable.
#[cfg(target_os = "macos")]
fn fake_app(dir: &Path, marker: &str) -> PathBuf {
    let app = dir.join("Mica Slicer.app");
    let macos = app.join("Contents").join("MacOS");
    std::fs::create_dir_all(&macos).unwrap();
    std::fs::write(app.join("Contents").join("Info.plist"), "<plist/>").unwrap();
    std::fs::write(macos.join("mica-gui"), marker).unwrap();
    app
}

/// A `.tar.gz` of `app` the way the release script makes it.
#[cfg(target_os = "macos")]
fn tarball(app: &Path) -> Vec<u8> {
    let out = app.with_extension("tar.gz");
    let ok = std::process::Command::new("/usr/bin/tar")
        .arg("-czf")
        .arg(&out)
        .arg("-C")
        .arg(app.parent().unwrap())
        .arg(app.file_name().unwrap())
        .status()
        .unwrap()
        .success();
    assert!(ok);
    std::fs::read(out).unwrap()
}

#[cfg(target_os = "macos")]
#[test]
fn mac_app_is_swapped_and_bad_packages_leave_it_alone() {
    let root = std::env::temp_dir().join(format!("mica-update-app-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let installed = root.join("Applications");
    let built = root.join("build");
    std::fs::create_dir_all(&installed).unwrap();
    std::fs::create_dir_all(&built).unwrap();
    let app = fake_app(&installed, "old");
    let exe = app.join("Contents/MacOS/mica-gui");
    let target = target_for(&exe, None).unwrap();
    assert_eq!(target, Target::MacApp(app.clone()));
    // Not an app bundle, a translocated app: refused.
    assert!(target_for(Path::new("/usr/local/bin/mica-gui"), None).is_err());
    let e = target_for(
        Path::new(
            "/private/var/folders/x/AppTranslocation/1/d/Mica Slicer.app/Contents/MacOS/mica-gui",
        ),
        None,
    )
    .unwrap_err();
    assert!(e.contains("Applications folder"), "{e}");
    // A good package replaces the bundle.
    let new = fake_app(&built, "new");
    std::fs::write(new.join("Contents/Resources.txt"), "extra").unwrap();
    let pkg = tarball(&new);
    assert_eq!(
        install(&target, Format::App, &pkg).unwrap(),
        Staged::Replaced(app.clone())
    );
    assert_eq!(std::fs::read_to_string(&exe).unwrap(), "new");
    assert!(app.join("Contents/Resources.txt").is_file());
    // No staging or old copies left behind.
    let left: Vec<_> = std::fs::read_dir(&installed)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(left, vec![std::ffi::OsString::from("Mica Slicer.app")]);
    // A package with another executable, garbage, the wrong format: refused, app untouched.
    let other = built.join("other");
    std::fs::create_dir_all(other.join("Mica Slicer.app/Contents/MacOS")).unwrap();
    std::fs::write(other.join("Mica Slicer.app/Contents/Info.plist"), "x").unwrap();
    std::fs::write(other.join("Mica Slicer.app/Contents/MacOS/evil"), "x").unwrap();
    let e = install(
        &target,
        Format::App,
        &tarball(&other.join("Mica Slicer.app")),
    )
    .unwrap_err();
    assert!(e.contains("complete Mica Slicer app"), "{e}");
    let e = install(&target, Format::App, b"not a tarball").unwrap_err();
    assert!(e.contains("could not be unpacked"), "{e}");
    let e = install(&target, Format::Nsis, &pkg).unwrap_err();
    assert!(e.contains("does not fit"), "{e}");
    assert_eq!(std::fs::read_to_string(&exe).unwrap(), "new");
    std::fs::remove_dir_all(&root).unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn appimage_is_replaced() {
    let root = std::env::temp_dir().join(format!("mica-update-ai-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let image = root.join("Mica_Slicer.AppImage");
    std::fs::write(&image, "old").unwrap();
    assert!(target_for(Path::new("/usr/bin/mica-gui"), None).is_err());
    let target = target_for(Path::new("/tmp/.mount/AppRun"), Some(&image)).unwrap();
    install(&target, Format::AppImage, b"new").unwrap();
    assert_eq!(std::fs::read_to_string(&image).unwrap(), "new");
    std::fs::remove_dir_all(&root).unwrap();
}
