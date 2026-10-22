// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The shipped Orca subset (written from the pinned checkout, as `cargo xtask bundle-profiles`
//! does) and the Prusa download against a loopback HTTP server (never the real one).

use super::prusa::{self, Pins};
use super::*;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::AtomicBool;

fn temp(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mica-vendor-profiles-{tag}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[test]
fn the_shipped_orca_subset_loads_every_vendor_on_its_own() {
    if !mica_testkit::require_orca_profiles() {
        return;
    }
    let dst = temp("subset");
    let copied =
        write_orca_subset(&mica_testkit::orca_profiles(), &dst, &SHIPPED_ORCA_VENDORS).unwrap();
    assert_eq!(copied.vendors.len(), SHIPPED_ORCA_VENDORS.len() + 1);
    assert!(dst.join("OrcaFilamentLibrary.json").is_file());
    // Only the listed vendors, nothing else from Orca's 70.
    let mut indexes = crate::orca::discovery::vendor_indexes(&dst);
    indexes.sort();
    let mut want: Vec<String> = SHIPPED_ORCA_VENDORS.iter().map(|s| s.to_string()).collect();
    want.sort();
    assert_eq!(indexes, want);
    for vendor in SHIPPED_ORCA_VENDORS {
        let (bundle, root) = crate::orca::discovery::load_installed(Some(&dst), vendor)
            .unwrap_or_else(|e| panic!("{vendor}: {e}"));
        assert_eq!(root, dst);
        use crate::orca::Kind;
        assert!(
            !bundle.preset_names(Kind::Machine).is_empty(),
            "{vendor} has no printers"
        );
        // Every preset's whole inheritance chain resolves inside the copy.
        for kind in [Kind::Machine, Kind::Process, Kind::Filament] {
            for name in bundle.preset_names(kind) {
                bundle
                    .flatten(kind, name)
                    .unwrap_or_else(|e| panic!("{vendor} {name}: {e}"));
            }
        }
    }
    // The U1's bed model, texture and gallery cover came along.
    for f in [
        "Snapmaker U1_bed.stl",
        "Snapmaker U1_texture.svg",
        "Snapmaker U1_cover.png",
    ] {
        assert!(dst.join("Snapmaker").join(f).is_file(), "{f}");
    }
    // A second run into the same folder refuses (no stale files ship).
    assert!(write_orca_subset(&mica_testkit::orca_profiles(), &dst, &["Voron"]).is_err());
    let _ = std::fs::remove_dir_all(&dst);
}

#[test]
fn a_vendor_inheriting_from_outside_the_subset_is_refused() {
    let src = temp("broken-src");
    let dst = temp("broken-dst");
    std::fs::create_dir_all(src.join("Acme/machine")).unwrap();
    std::fs::create_dir_all(src.join(ORCA_LIBRARY)).unwrap();
    std::fs::write(
        src.join("Acme.json"),
        r#"{"name":"Acme","machine_list":[{"name":"Acme One","sub_path":"machine/Acme One.json"}]}"#,
    )
    .unwrap();
    std::fs::write(
        src.join("Acme/machine/Acme One.json"),
        r#"{"name":"Acme One","inherits":"fdm_other_vendor_common"}"#,
    )
    .unwrap();
    std::fs::write(
        src.join(format!("{ORCA_LIBRARY}.json")),
        r#"{"name":"lib"}"#,
    )
    .unwrap();
    let e = write_orca_subset(&src, &dst, &["Acme"]).unwrap_err();
    assert!(e.contains("fdm_other_vendor_common"), "{e}");
    let _ = std::fs::remove_dir_all(&src);
    let _ = std::fs::remove_dir_all(&dst);
}

/// A reply of the loopback server: status, body, and the Content-Length it claims.
#[derive(Clone)]
struct Reply {
    status: u16,
    body: Vec<u8>,
    length: usize,
}
impl Reply {
    fn ok(body: &[u8]) -> Self {
        Self {
            status: 200,
            body: body.to_vec(),
            length: body.len(),
        }
    }
    /// Claims more bytes than it sends, then hangs up.
    fn cut(body: &[u8]) -> Self {
        Self {
            status: 200,
            body: body[..body.len() / 2].to_vec(),
            length: body.len(),
        }
    }
    fn missing() -> Self {
        Self {
            status: 404,
            body: b"not found".to_vec(),
            length: 9,
        }
    }
}

/// Serves `routes` on 127.0.0.1 until the test ends; `None` when this sandbox cannot listen.
fn serve(routes: HashMap<String, Reply>) -> Option<String> {
    let listener = match TcpListener::bind("127.0.0.1:0") {
        Ok(l) => l,
        Err(e) => {
            eprintln!("skipped: cannot listen on loopback here ({e})");
            return None;
        }
    };
    let base = format!("http://{}/repo/", listener.local_addr().ok()?);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut head = Vec::new();
            let mut byte = [0u8; 1];
            while !head.ends_with(b"\r\n\r\n") && stream.read(&mut byte).is_ok_and(|n| n == 1) {
                head.push(byte[0]);
            }
            let text = String::from_utf8_lossy(&head);
            let path = text.split_whitespace().nth(1).unwrap_or("").to_string();
            let reply = routes
                .get(path.trim_start_matches("/repo/"))
                .cloned()
                .unwrap_or_else(Reply::missing);
            let _ = write!(
                stream,
                "HTTP/1.1 {} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                reply.status, reply.length
            );
            let _ = stream.write_all(&reply.body);
        }
    });
    Some(base)
}

fn pins(bundle: &[u8], assets: &[(&str, &[u8])]) -> Pins {
    Pins {
        bundle_sha256: vec![prusa::sha256_hex(bundle)],
        assets: assets
            .iter()
            .map(|(n, b)| (n.to_string(), prusa::sha256_hex(b)))
            .collect(),
    }
}

const BUNDLE: &[u8] = b"[vendor]\nname = Prusa Research\nconfig_version = 2.5.12\n";
const BED: &[u8] = b"solid bed\nendsolid bed\n";

fn files(dir: &Path) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(dir)
        .map(|d| {
            d.filter_map(|e| Some(e.ok()?.file_name().to_string_lossy().into_owned()))
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    out
}

#[test]
fn a_good_download_is_kept_and_found() {
    let routes = HashMap::from([
        ("PrusaResearch/2.5.12.ini".to_string(), Reply::ok(BUNDLE)),
        ("PrusaResearch/bed.stl".to_string(), Reply::ok(BED)),
        // A texture whose bytes differ from the pin, and one the server lacks.
        ("PrusaResearch/bed.svg".to_string(), Reply::ok(b"<svg/>")),
    ]);
    let Some(base) = serve(routes) else { return };
    let dir = temp("good");
    let pins = pins(
        BUNDLE,
        &[
            ("bed.stl", BED),
            ("bed.svg", b"<svg></svg>"),
            ("gone.png", b"x"),
        ],
    );
    let mut last = 0.;
    let got = prusa::fetch_from(
        &base,
        &dir,
        &pins,
        &mut |f| last = f,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(last, 1.);
    assert_eq!(got.bundle, dir.join("2.5.12.ini"));
    assert_eq!(std::fs::read(&got.bundle).unwrap(), BUNDLE);
    assert_eq!(got.assets, 1);
    assert_eq!(got.missing_assets, vec!["bed.svg", "gone.png"]);
    // Only verified files, no leftovers.
    assert_eq!(files(&dir), vec!["2.5.12.ini", "bed.stl"]);
    assert_eq!(
        prusa::cached_bundle_in(&dir, &pins),
        Some(dir.join("2.5.12.ini"))
    );
    // A file changed on disk afterwards is not used.
    std::fs::write(dir.join("2.5.12.ini"), b"[vendor]\nname = Tampered\n").unwrap();
    assert_eq!(prusa::cached_bundle_in(&dir, &pins), None);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_bundle_with_the_wrong_hash_is_not_stored() {
    let routes = HashMap::from([(
        "PrusaResearch/2.5.12.ini".to_string(),
        Reply::ok(b"[vendor]\nname = Something else\n"),
    )]);
    let Some(base) = serve(routes) else { return };
    let dir = temp("wrong");
    let e = prusa::fetch_from(
        &base,
        &dir,
        &pins(BUNDLE, &[]),
        &mut |_| {},
        &AtomicBool::new(false),
    )
    .unwrap_err();
    assert!(e.contains("not version 2.5.12"), "{e}");
    assert!(files(&dir).is_empty(), "{:?}", files(&dir));
    assert_eq!(prusa::cached_bundle_in(&dir, &pins(BUNDLE, &[])), None);
}

#[test]
fn a_broken_or_failed_download_leaves_no_file() {
    let routes = HashMap::from([("PrusaResearch/2.5.12.ini".to_string(), Reply::cut(BUNDLE))]);
    let Some(base) = serve(routes) else { return };
    let dir = temp("cut");
    let pins = pins(BUNDLE, &[]);
    assert!(prusa::fetch_from(&base, &dir, &pins, &mut |_| {}, &AtomicBool::new(false)).is_err());
    assert!(files(&dir).is_empty(), "{:?}", files(&dir));
    // A server without the file (HTTP 404), and a canceled download.
    let e = prusa::fetch_from(
        &format!("{base}elsewhere/"),
        &dir,
        &pins,
        &mut |_| {},
        &AtomicBool::new(false),
    )
    .unwrap_err();
    assert!(e.contains("HTTP 404"), "{e}");
    assert!(prusa::fetch_from(&base, &dir, &pins, &mut |_| {}, &AtomicBool::new(true)).is_err());
    assert!(files(&dir).is_empty(), "{:?}", files(&dir));
}

#[test]
fn the_pinned_assets_and_hashes_are_well_formed() {
    let pins = Pins::pinned();
    assert_eq!(pins.bundle_sha256.len(), 2);
    assert!(pins.assets.len() >= 50, "{}", pins.assets.len());
    for (name, sha) in &pins.assets {
        assert_eq!(Path::new(name).components().count(), 1, "{name}");
        assert_eq!(sha.len(), 64, "{name}");
    }
    // The pinned git copy of the bundle is one of the accepted files, and its assets match.
    if mica_testkit::require_prusa_profiles() {
        let bytes = std::fs::read(mica_testkit::prusa_bundle()).unwrap();
        assert!(pins.bundle_sha256.contains(&prusa::sha256_hex(&bytes)));
        let dir = mica_testkit::prusa_bundle().parent().unwrap().to_path_buf();
        for (name, sha) in &pins.assets {
            let b = std::fs::read(dir.join(name)).unwrap();
            assert_eq!(&prusa::sha256_hex(&b), sha, "{name}");
        }
    }
}

#[test]
fn two_downloads_at_once_both_end_with_the_verified_files() {
    // Two Mica processes (here: two downloads in one, which share even the process id)
    // fetching into the same folder at the same time.
    let mut big = BUNDLE.to_vec();
    big.extend(std::iter::repeat_n(b'#', 4 << 20));
    let routes = HashMap::from([
        ("PrusaResearch/2.5.12.ini".to_string(), Reply::ok(&big)),
        ("PrusaResearch/bed.stl".to_string(), Reply::ok(BED)),
    ]);
    let Some(base) = serve(routes) else { return };
    let dir = temp("twice");
    let pins = pins(&big, &[("bed.stl", BED)]);
    std::thread::scope(|s| {
        for _ in 0..4 {
            let (base, dir, pins, big) = (&base, &dir, &pins, &big);
            s.spawn(move || {
                let got = prusa::fetch_from(base, dir, pins, &mut |_| {}, &AtomicBool::new(false))
                    .unwrap();
                assert_eq!(got.assets, 1);
                // Whatever the other download is doing, the file here is complete.
                assert_eq!(&std::fs::read(&got.bundle).unwrap(), big);
            });
        }
    });
    assert_eq!(files(&dir), vec!["2.5.12.ini", "bed.stl"]);
    assert_eq!(
        prusa::cached_bundle_in(&dir, &pins),
        Some(dir.join("2.5.12.ini"))
    );
    let _ = std::fs::remove_dir_all(&dir);
}
