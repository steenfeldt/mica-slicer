// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The dashboard's JSON API, cameras and uploads against the loopback mock printers
//! (`crates/mica-printhost/examples/mock_printer.rs`). Nothing here leaves 127.0.0.1.

use mica_library::testkit::{TempDir, synthetic_library};
use mica_printers::secrets::{MemoryStore, SecretStore};
use mica_web::{ACTION_HEADER, Dashboard, Options};
use serde_json::Value;
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

const KEY: &str = "mock-api-key-1234";

/// The mock printers on free loopback ports (XL, CORE One, U1, the XL's IP camera), with an
/// API key; killed on drop.
struct Mock {
    child: std::process::Child,
    xl: u16,
    u1: u16,
    ipcam: u16,
}
impl Drop for Mock {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn free() -> Option<u16> {
    Some(
        std::net::TcpListener::bind("127.0.0.1:0")
            .ok()?
            .local_addr()
            .ok()?
            .port(),
    )
}

fn mock() -> Option<Mock> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let name = format!("mock_printer{}", std::env::consts::EXE_SUFFIX);
    let bin = [
        dir.join("examples").join(&name),
        dir.join("../examples").join(&name),
    ]
    .into_iter()
    .find(|p| p.exists())?;
    let (xl, coreone, u1, ipcam) = (free()?, free()?, free()?, free()?);
    let child = std::process::Command::new(bin)
        .args(["--xl", &xl.to_string(), "--coreone", &coreone.to_string()])
        .args(["--u1", &u1.to_string(), "--ipcam", &ipcam.to_string()])
        .args(["--rtsp", "off", "--camera-streamer", "off"])
        .args(["--api-key", KEY, "--job-seconds", "600"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let m = Mock {
        child,
        xl,
        u1,
        ipcam,
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if [xl, u1, ipcam]
            .iter()
            .all(|p| TcpStream::connect(("127.0.0.1", *p)).is_ok())
        {
            return Some(m);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    None
}

/// One HTTP request; the status and the body.
fn request(
    addr: SocketAddr,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> (u16, Vec<u8>) {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    let mut head = format!(
        "{method} {path} HTTP/1.1\r\nContent-Length: {}\r\n",
        body.len()
    );
    if !headers.iter().any(|(n, _)| *n == "Host") {
        head.push_str(&format!("Host: {addr}\r\n"));
    }
    for (n, v) in headers {
        head.push_str(&format!("{n}: {v}\r\n"));
    }
    head.push_str("\r\n");
    s.write_all(head.as_bytes()).unwrap();
    s.write_all(body).unwrap();
    let mut out = vec![];
    s.read_to_end(&mut out).unwrap();
    let end = out.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
    let status = String::from_utf8_lossy(&out[9..12]).parse().unwrap();
    (status, out[end..].to_vec())
}

fn get_json(addr: SocketAddr, path: &str) -> Value {
    let (status, body) = request(addr, "GET", path, &[], b"");
    assert_eq!(status, 200, "{path}: {}", String::from_utf8_lossy(&body));
    serde_json::from_slice(&body).unwrap()
}

fn post(addr: SocketAddr, path: &str, body: &[u8]) -> (u16, Value) {
    let (status, body) = request(addr, "POST", path, &[(ACTION_HEADER, "1")], body);
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}

fn printer<'a>(list: &'a Value, id: &str) -> &'a Value {
    list["printers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == id)
        .unwrap_or_else(|| panic!("no printer {id} in {list}"))
}

/// Polls the printer list until `done` holds for it.
fn wait_for(addr: SocketAddr, what: &str, done: impl Fn(&Value) -> bool) -> Value {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let v = get_json(addr, "/api/printers");
        if done(&v) {
            return v;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {what}: {v}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Opens the camera stream of printer `id` and returns its first JPEG.
fn first_frame(addr: SocketAddr, id: &str) -> Vec<u8> {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    write!(
        s,
        "GET /api/printers/{id}/camera HTTP/1.1\r\nHost: {addr}\r\n\r\n"
    )
    .unwrap();
    let mut buf = vec![];
    let mut chunk = [0u8; 4096];
    loop {
        let n = s.read(&mut chunk).unwrap();
        assert!(n > 0, "the stream ended: {}", String::from_utf8_lossy(&buf));
        buf.extend_from_slice(&chunk[..n]);
        let text = String::from_utf8_lossy(&buf).to_string();
        assert!(
            text.starts_with("HTTP/1.1 200")
                && text.contains("multipart/x-mixed-replace; boundary=")
                || !text.contains("\r\n\r\n"),
            "{text}"
        );
        // The first part: its headers, then Content-Length bytes.
        const TAG: &str = "Content-Type: image/jpeg\r\nContent-Length: ";
        let Some(part) = text.find(TAG) else {
            continue;
        };
        let rest = &text[part + TAG.len()..];
        let Some(eol) = rest.find("\r\n\r\n") else {
            continue;
        };
        let len: usize = rest[..eol].trim().parse().unwrap();
        let start = part + TAG.len() + eol + 4;
        if buf.len() >= start + len {
            return buf[start..start + len].to_vec();
        }
    }
}

fn wait_until(what: &str, f: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !f() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn write_config(dir: &Path, m: &Mock) {
    let printers = format!(
        r#"
[[printers]]
name = "XL"
kind = "PrusaLink"
address = "127.0.0.1:{xl}"
keychain = "xl-key"
camera = "http://127.0.0.1:{ipcam}/stream.mjpg"

[[printers]]
name = "U1"
kind = "SnapmakerU1"
address = "127.0.0.1:{u1}"
keychain = "u1-key"

[[printers]]
name = "Lost key"
kind = "PrusaLink"
address = "127.0.0.1:{xl}"
keychain = "never-stored"
"#,
        xl = m.xl,
        u1 = m.u1,
        ipcam = m.ipcam
    );
    std::fs::write(dir.join("printers.toml"), printers).unwrap();
}

#[test]
fn printers_cameras_uploads_and_the_library_against_the_mocks() {
    let Some(m) = mock() else {
        mica_testkit::require(
            false,
            "the mock_printer example is not built (or loopback is unavailable)",
            "cargo test --workspace builds it",
        );
        return;
    };
    let config = TempDir::new("web-config");
    write_config(config.path(), &m);
    // A library with G-code in it, indexed the way the window does it.
    let models = TempDir::new("web-models");
    synthetic_library(models.path(), 12).unwrap();
    let data = config.path().join("library");
    {
        let lib = mica_library::Library::open(
            &data,
            mica_library::Options {
                watch: false,
                ..Default::default()
            },
        )
        .unwrap();
        lib.add_folder(models.path()).unwrap();
        assert!(lib.wait_idle(Duration::from_secs(60)));
    }
    // The keychain the window would have filled (a test never opens the real one).
    let secrets = Arc::new(MemoryStore::default());
    secrets.set("xl-key", KEY).unwrap();
    secrets.set("u1-key", KEY).unwrap();
    let dash = Dashboard::start(Options {
        listen: "127.0.0.1:0".parse().unwrap(),
        printers: config.path().join("printers.toml"),
        library: data,
        secrets,
        open_url: Some("https://mica.example/vnc".into()),
        loopback_only: true,
        allowed_hosts: vec![],
        allow_printing: true,
    })
    .unwrap();
    let addr = dash.addr();

    // The page and its settings.
    let (status, page) = request(addr, "GET", "/", &[], b"");
    assert_eq!(status, 200);
    assert!(String::from_utf8_lossy(&page).contains("app.js"));
    assert_eq!(request(addr, "GET", "/app.js", &[], b"").0, 200);
    let info = get_json(addr, "/api/info");
    assert_eq!(info["open_url"], "https://mica.example/vnc");
    assert_eq!(info["printing"], true);

    // Status, job, temperatures and loaded filaments, polled on the server.
    let list = wait_for(addr, "the printers' status and filaments", |v| {
        printer(v, "xl")["state"] == "printing"
            && !printer(v, "u1")["filaments"].as_array().unwrap().is_empty()
            && printer(v, "u1")["camera"]["available"] == true
    });
    assert_eq!(list["printers"].as_array().unwrap().len(), 3);
    let xl = printer(&list, "xl");
    assert_eq!(xl["state_label"], "Printing");
    assert!(xl["job"]["progress"].as_f64().is_some());
    assert!(xl["job"]["name"].as_str().is_some());
    assert!(xl["temperatures"]["bed"]["current"].as_f64().is_some());
    assert_eq!(xl["camera"]["label"], "Camera URL · MJPEG");
    let u1 = printer(&list, "u1");
    let f = &u1["filaments"][0];
    assert!(f["material"].as_str().is_some() && f["colour"]["name"].as_str().is_some());
    let lost = printer(&list, "lost-key");
    assert_eq!(lost["state"], "unreachable");
    assert!(lost["error"].as_str().unwrap().contains("missing"));
    // Secrets and camera addresses never reach the browser.
    let text = list.to_string();
    assert!(!text.contains(KEY));
    assert!(!text.contains(&format!(":{}/", m.ipcam)));

    // Cameras: a manual MJPEG URL (XL) and the one Moonraker reports (U1), as JPEG frames;
    // the player stops when the last viewer leaves.
    for id in ["xl", "u1"] {
        let jpeg = first_frame(addr, id);
        assert_eq!(&jpeg[..2], &[0xFF, 0xD8], "{id}: not a JPEG");
        let entry = dash.fleet().find(id).unwrap();
        wait_until("the camera to stop with its last viewer", || {
            !entry.camera.playing()
        });
    }
    // Two viewers share one player.
    {
        let entry = dash.fleet().find("u1").unwrap();
        let a = std::thread::spawn(move || first_frame(addr, "u1"));
        let b = first_frame(addr, "u1");
        assert_eq!(&a.join().unwrap()[..2], &b[..2]);
        wait_until("both viewers to leave", || entry.camera.viewers() == 0);
        assert!(!entry.camera.playing());
    }
    let (status, _) = request(addr, "GET", "/api/printers/lost-key/camera", &[], b"");
    assert_eq!(status, 404);

    // Uploads: refused without the dashboard's header, for other files and other hosts.
    let gcode = b"; generated by Mica Slicer\nG28\nG1 X10 Y10\n";
    let path = "/api/printers/u1/upload?name=part.gcode&start=0";
    assert_eq!(request(addr, "POST", path, &[], gcode).0, 403);
    assert_eq!(
        post(addr, "/api/printers/u1/upload?name=part.stl", gcode).0,
        400
    );
    assert_eq!(
        request(
            addr,
            "GET",
            "/api/printers",
            &[("Host", "rebound.example:7380")],
            b""
        )
        .0,
        421
    );
    assert_eq!(
        post(addr, "/api/printers/nope/upload?name=a.gcode", gcode).0,
        404
    );
    let (status, v) = post(addr, path, gcode);
    assert_eq!(status, 202, "{v}");
    let list = wait_for(addr, "the upload to finish", |v| {
        printer(v, "u1")["transfer"]["phase"] != "sending"
    });
    let t = &printer(&list, "u1")["transfer"];
    assert_eq!(t["phase"], "done", "{t}");
    assert_eq!(t["name"], "part.gcode");
    assert_eq!(t["sent"], gcode.len());
    // Starting while the printer prints is refused before anything is sent.
    let (status, v) = post(addr, "/api/printers/xl/upload?name=b.gcode&start=1", gcode);
    assert_eq!(status, 409);
    assert!(v["error"].as_str().unwrap().contains("busy"));

    // The library: search, thumbnails, and G-code sent from where it is stored.
    let lib = get_json(addr, "/api/library?gcode=1");
    let models = lib["models"].as_array().unwrap();
    assert!(!models.is_empty() && models.iter().all(|m| m["gcode"] == true));
    let all = get_json(addr, "/api/library?gcode=0&limit=500");
    assert!(all["total"].as_u64().unwrap() > lib["total"].as_u64().unwrap());
    let with_thumb = all["models"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["thumbnail"] == true)
        .unwrap();
    let (status, png) = request(
        addr,
        "GET",
        &format!(
            "/api/library/{}/thumbnail",
            with_thumb["id"].as_str().unwrap()
        ),
        &[],
        b"",
    );
    assert_eq!(status, 200);
    assert!(png.len() > 8);
    assert_eq!(
        request(addr, "GET", "/api/library/..%2f..%2fx/thumbnail", &[], b"").0,
        404
    );
    let name = models[0]["name"].as_str().unwrap();
    let words = get_json(addr, &format!("/api/library?q={}", name.replace(' ', "+")));
    assert!(
        words["models"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["name"] == name)
    );
    let not_gcode = all["models"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["gcode"] == false)
        .unwrap();
    let (status, _) = post(
        addr,
        "/api/printers/u1/send",
        format!(r#"{{"model":"{}"}}"#, not_gcode["id"].as_str().unwrap()).as_bytes(),
    );
    assert_eq!(status, 400);
    let (status, v) = post(
        addr,
        "/api/printers/u1/send",
        format!(
            r#"{{"model":"{}","start":false}}"#,
            models[0]["id"].as_str().unwrap()
        )
        .as_bytes(),
    );
    assert_eq!(status, 202, "{v}");
    let list = wait_for(addr, "the library file to arrive", |v| {
        let t = &printer(v, "u1")["transfer"];
        t["name"] == name && t["phase"] != "sending"
    });
    assert_eq!(printer(&list, "u1")["transfer"]["phase"], "done");

    // printers.toml edited in the window: the list follows.
    let text = std::fs::read_to_string(config.path().join("printers.toml")).unwrap();
    let cut = text.find("[[printers]]\nname = \"Lost key\"").unwrap();
    std::fs::write(config.path().join("printers.toml"), &text[..cut]).unwrap();
    wait_for(addr, "the removed printer to go", |v| {
        v["printers"].as_array().unwrap().len() == 2
    });
    drop(dash);
}

#[test]
fn an_empty_settings_folder_shows_no_printers_and_an_empty_library() {
    let config = TempDir::new("web-empty");
    let dash = Dashboard::start(Options {
        listen: "127.0.0.1:0".parse().unwrap(),
        printers: config.path().join("printers.toml"),
        library: config.path().join("library"),
        secrets: Arc::new(MemoryStore::default()),
        open_url: None,
        loopback_only: true,
        allowed_hosts: vec!["box.tailnet.example".into()],
        allow_printing: false,
    })
    .unwrap();
    let addr = dash.addr();
    let v = get_json(addr, "/api/printers");
    assert_eq!(v["printers"], Value::Array(vec![]));
    assert!(v["error"].is_null());
    let lib = get_json(addr, "/api/library");
    assert_eq!(lib["count"], 0);
    let info = get_json(addr, "/api/info");
    assert!(info["open_url"].is_null());
    // Read-only by default: the page is told so, and every request that would send a file
    // or start a print is refused, whatever it carries.
    assert_eq!(info["printing"], false);
    std::fs::write(
        config.path().join("printers.toml"),
        "[[printers]]\nname = \"Desk\"\nkind = \"Moonraker\"\naddress = \"127.0.0.1:9\"\n",
    )
    .unwrap();
    wait_for(addr, "the printer to show", |v| {
        v["printers"].as_array().unwrap().len() == 1
    });
    for (path, body) in [
        (
            "/api/printers/desk/upload?name=a.gcode&start=0",
            &b"G28\n"[..],
        ),
        (
            "/api/printers/desk/upload?name=a.gcode&start=1",
            &b"G28\n"[..],
        ),
        (
            "/api/printers/desk/send",
            &br#"{"model":"00","start":true}"#[..],
        ),
        ("/api/printers/nope/upload?name=a.gcode", &b"G28\n"[..]),
    ] {
        let (status, v) = post(addr, path, body);
        assert_eq!(status, 403, "{path}: {v}");
        assert_eq!(v["error"], mica_web::READ_ONLY, "{path}");
    }
    // A host name allowed for a reverse proxy is answered; others are not.
    let ok = request(
        addr,
        "GET",
        "/api/printers",
        &[("Host", "box.tailnet.example")],
        b"",
    );
    assert_eq!(ok.0, 200);
    let other = |host: &str| request(addr, "GET", "/api/printers", &[("Host", host)], b"").0;
    assert_eq!(other("other.tailnet.example"), 421);
    // Names that cannot be rebound: LAN addresses and short names.
    assert_eq!(other("192.0.2.10:3002"), 200);
    assert_eq!(other("mica-box:3002"), 200);
    // A broken printers.toml is reported, not fatal.
    std::fs::write(
        config.path().join("printers.toml"),
        "[[printers]]\nname = 1\n",
    )
    .unwrap();
    let v = wait_for(addr, "the broken file to be reported", |v| {
        !v["error"].is_null()
    });
    assert!(v["error"].as_str().unwrap().contains("printers.toml"));
}
