// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Mica's web dashboard: a small companion page for running Mica on a home server. It lists
//! the printers Mica knows (`printers.toml`, shared with the window) with their live status,
//! streams their cameras, takes G-code uploads, and browses the model library. All logic is
//! in Mica's crates; the page (`page/`, embedded) only draws the JSON it gets.
//!
//! There is no login: the server listens on 127.0.0.1 unless told otherwise and is meant to
//! sit behind a private network (Tailscale). Secrets and camera addresses stay on the server.
//! See `docs/web-dashboard.md`.

pub mod cameras;
pub mod fleet;
pub mod http;
pub mod library;

use fleet::Fleet;
use http::{Request, Response};
use library::Library;
use mica_printers::secrets::SecretStore;
use serde_json::json;
use std::{
    io::{self, Read, Write},
    net::SocketAddr,
    path::PathBuf,
    sync::Arc,
};

/// Uploads from the browser are refused above this size.
pub const MAX_UPLOAD: u64 = 2 << 30;
/// JSON request bodies are small.
const MAX_JSON: u64 = 64 * 1024;
/// The header every state-changing request carries: a plain HTML form on another site
/// cannot send it, and a script there cannot either without this server's consent (which it
/// never gives).
pub const ACTION_HEADER: &str = "X-Mica-Dashboard";

const INDEX_HTML: &str = include_str!("page/index.html");
const APP_JS: &str = include_str!("page/app.js");
const APP_CSS: &str = include_str!("page/app.css");

pub struct Options {
    pub listen: SocketAddr,
    /// `printers.toml`.
    pub printers: PathBuf,
    /// The library's data folder (its index and thumbnails).
    pub library: PathBuf,
    pub secrets: Arc<dyn SecretStore>,
    /// Where "Open in Mica" goes (the streamed Mica window), if anywhere. `{host}` stands for
    /// the host name the page was opened with (`http://{host}:3000/` in the Docker image).
    pub open_url: Option<String>,
    /// Refuse printers and cameras that are not on this computer (tests).
    pub loopback_only: bool,
    /// Take uploads and send or start prints. Off by default: the dashboard then only shows
    /// status, cameras and the library, and refuses every request that would change a
    /// printer.
    pub allow_printing: bool,
    /// Host names accepted besides the ones that cannot be rebound (see [`trusted_name`]):
    /// a full name such as `mica.tailnet.ts.net`, or `.tailnet.ts.net` for every name under
    /// it.
    pub allowed_hosts: Vec<String>,
}

struct App {
    fleet: Arc<Fleet>,
    library: Library,
    open_url: Option<String>,
    uploads: PathBuf,
    secrets_label: &'static str,
    printers_path: PathBuf,
    allowed_hosts: Vec<String>,
    allow_printing: bool,
}

/// What a read-only dashboard answers to a request that would send or start a print.
pub const READ_ONLY: &str = "This dashboard is read-only: uploads and printing are off. Start mica-web with --allow-printing (or MICA_WEB_ALLOW_PRINTING=1) to allow them.";

/// The running dashboard. Dropping it stops the server and the printer polling.
pub struct Dashboard {
    server: Option<http::Server>,
    fleet: Arc<Fleet>,
    uploads: PathBuf,
}

impl Dashboard {
    pub fn start(options: Options) -> io::Result<Dashboard> {
        if options.loopback_only {
            mica_camera::restrict_to_loopback();
        }
        // Received files wait here until they are sent on; one folder per dashboard.
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let uploads = std::env::temp_dir().join(format!(
            "mica-web-uploads-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&uploads)?;
        let fleet = Arc::new(Fleet::new(
            options.printers.clone(),
            options.secrets.clone(),
            options.loopback_only,
        ));
        let app = Arc::new(App {
            fleet: fleet.clone(),
            library: Library::new(options.library),
            open_url: options.open_url,
            uploads: uploads.clone(),
            secrets_label: options.secrets.label(),
            printers_path: options.printers,
            allowed_hosts: options.allowed_hosts,
            allow_printing: options.allow_printing,
        });
        let server = http::Server::start(options.listen, move |req| app.handle(req))?;
        Ok(Dashboard {
            server: Some(server),
            fleet,
            uploads,
        })
    }
    pub fn addr(&self) -> SocketAddr {
        self.server.as_ref().expect("running").addr()
    }
    pub fn fleet(&self) -> &Arc<Fleet> {
        &self.fleet
    }
    /// Serves until the process ends.
    pub fn join(mut self) {
        if let Some(s) = self.server.take() {
            s.join();
        }
    }
}

impl Drop for Dashboard {
    fn drop(&mut self) {
        self.server = None;
        self.fleet.stop();
        let _ = std::fs::remove_dir_all(&self.uploads);
    }
}

/// The name in a `Host` header, without the port.
fn host_name(host: &str) -> &str {
    if let Some(rest) = host.strip_prefix('[') {
        rest.split(']').next().unwrap_or("")
    } else {
        host.rsplit_once(':').map_or(host, |(h, _)| h)
    }
}

/// Whether the name in `host` (a `Host` header) is one a web page elsewhere cannot point at
/// this server (DNS rebinding): `localhost`, an IP address typed as such, a single-label
/// name (`mica-box`, a LAN or Tailscale MagicDNS short name) or an mDNS `.local` name. Public
/// DNS names are only accepted when listed with `--allow-host`.
fn trusted_name(host: &str) -> bool {
    let name = host_name(host);
    let plain = !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b':'));
    plain
        && (name.eq_ignore_ascii_case("localhost")
            || name.parse::<std::net::IpAddr>().is_ok()
            || !name.contains('.')
            || name.to_ascii_lowercase().ends_with(".local"))
}

/// The "Open in Mica" link for a page opened as `host`: `{host}` in `url` becomes its name.
fn open_url(url: &str, host: Option<&str>) -> Option<String> {
    if !url.contains("{host}") {
        return Some(url.to_string());
    }
    let host = host?;
    let name = host_name(host);
    let ok = !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b':'));
    if !ok {
        return None;
    }
    let name = if name.contains(':') {
        format!("[{name}]")
    } else {
        name.to_string()
    };
    Some(url.replace("{host}", &name))
}

impl App {
    fn allowed_host(&self, host: &str) -> bool {
        let name = host_name(host).to_ascii_lowercase();
        self.allowed_hosts.iter().any(|a| {
            let a = a.trim().to_ascii_lowercase();
            if a.starts_with('.') {
                name.ends_with(&a)
            } else {
                !a.is_empty() && a == name
            }
        })
    }

    fn handle(&self, req: &mut Request) -> Option<Response> {
        // A page on another site that renamed itself to this server's address (DNS
        // rebinding) is turned away: only names that cannot be rebound, or listed ones.
        if !req
            .header("Host")
            .is_some_and(|h| trusted_name(h) || self.allowed_host(h))
        {
            return Some(Response::error(
                421,
                "This dashboard does not answer to that name; add it with --allow-host (MICA_WEB_ALLOW_HOSTS).",
            ));
        }
        let parts: Vec<String> = req
            .path
            .trim_matches('/')
            .split('/')
            .map(str::to_string)
            .collect();
        let parts: Vec<&str> = parts.iter().map(String::as_str).collect();
        let method = req.method.clone();
        // Every POST sends a file or starts a print: refused unless the operator opted in.
        if method == "POST" && !self.allow_printing {
            return Some(Response::error(403, READ_ONLY));
        }
        if method == "POST" && req.header(ACTION_HEADER).is_none() {
            return Some(Response::error(
                403,
                "Missing the dashboard's request header.",
            ));
        }
        Some(match (method.as_str(), parts.as_slice()) {
            ("GET", [""]) => page(INDEX_HTML, "text/html; charset=utf-8"),
            ("GET", ["app.js"]) => page(APP_JS, "text/javascript; charset=utf-8"),
            ("GET", ["app.css"]) => page(APP_CSS, "text/css; charset=utf-8"),
            ("GET", ["api", "info"]) => Response::json(&json!({
                "version": env!("CARGO_PKG_VERSION"),
                "open_url": self.open_url.as_deref().and_then(|u| open_url(u, req.header("Host"))),
                "printers_file": self.printers_path.display().to_string(),
                "secrets": self.secrets_label,
                "max_upload": MAX_UPLOAD,
                "printing": self.allow_printing,
            })),
            ("GET", ["api", "printers"]) => Response::json(&self.fleet.json()),
            ("GET", ["api", "printers", id, "camera"]) => match self.fleet.find(id) {
                Some(e) => return cameras::serve(&e, &mut req.stream),
                None => no_printer(),
            },
            ("POST", ["api", "printers", id, "upload"]) => self.upload(req, id),
            ("POST", ["api", "printers", id, "send"]) => self.send_from_library(req, id),
            ("GET", ["api", "library"]) => {
                let num = |k: &str, d: usize| {
                    req.param(k)
                        .and_then(|v| v.parse::<usize>().ok())
                        .unwrap_or(d)
                };
                Response::json(&self.library.search(
                    req.param("q").unwrap_or(""),
                    req.param("gcode") == Some("1"),
                    num("offset", 0),
                    num("limit", 60),
                ))
            }
            ("GET", ["api", "library", id, "thumbnail"]) => match self.library.thumbnail(id) {
                Some((bytes, kind)) => Response::new(200, kind, bytes)
                    .with_header("Cache-Control", "private, max-age=3600"),
                None => Response::error(404, "No picture for this model."),
            },
            (_, ["api", ..]) | ("POST", _) => Response::error(404, "Unknown request."),
            _ => Response::new(404, "text/plain; charset=utf-8", "Not found"),
        })
    }

    /// A G-code file from the browser (the body), sent on to the printer.
    fn upload(&self, req: &mut Request, id: &str) -> Response {
        let Some(entry) = self.fleet.find(id) else {
            return no_printer();
        };
        let name = req.param("name").unwrap_or("").to_string();
        let start = req.param("start") == Some("1");
        if let Err(e) = fleet::remote_name(&name) {
            return Response::error(400, &e);
        }
        // Refused before the file is transferred.
        if let Err(e) = fleet::check(&entry, &name, start) {
            return Response::error(409, &e);
        }
        if req.body_len() == 0 {
            return Response::error(400, "The file is empty.");
        }
        if req.body_len() > MAX_UPLOAD {
            return Response::error(413, "The file is larger than the dashboard accepts (2 GB).");
        }
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let path = self.uploads.join(format!(
            "{}.upload",
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let expected = req.body_len();
        let received = (|| -> io::Result<u64> {
            let mut file = std::fs::File::create(&path)?;
            let n = io::copy(&mut req.body().take(expected), &mut file)?;
            file.flush()?;
            Ok(n)
        })();
        match received {
            Ok(n) if n == expected => {}
            _ => {
                let _ = std::fs::remove_file(&path);
                return Response::error(400, "The upload was cut short.");
            }
        }
        match fleet::send(&entry, path, &name, start, true) {
            Ok(()) => Response::new(
                202,
                "application/json",
                json!({ "accepted": true }).to_string(),
            ),
            Err(e) => Response::error(409, &e),
        }
    }

    /// A G-code file from the library (`{"model": id, "start": bool}`), sent to the printer
    /// from where it is stored.
    fn send_from_library(&self, req: &mut Request, id: &str) -> Response {
        let Some(entry) = self.fleet.find(id) else {
            return no_printer();
        };
        let body = match req.body_bytes(MAX_JSON) {
            Ok(b) => b,
            Err(r) => return r,
        };
        let Ok(v) = serde_json::from_slice::<serde_json::Value>(&body) else {
            return Response::error(400, "Expected JSON.");
        };
        let start = v["start"].as_bool().unwrap_or(false);
        let Some(model) = v["model"].as_str().and_then(|m| self.library.model(m)) else {
            return Response::error(404, "That file is no longer in the library.");
        };
        if !model.kind.is_gcode() {
            return Response::error(
                400,
                "Only G-code can be sent to a printer; slice the model in Mica first.",
            );
        }
        let Some(location) = model
            .locations
            .iter()
            .find(|l| l.online && l.path.is_file())
        else {
            return Response::error(
                404,
                "The file's folder is not available right now (a disconnected drive or share?).",
            );
        };
        match fleet::send(&entry, location.path.clone(), &model.name, start, false) {
            Ok(()) => Response::new(
                202,
                "application/json",
                json!({ "accepted": true }).to_string(),
            ),
            Err(e) => Response::error(409, &e),
        }
    }
}

fn page(body: &'static str, kind: &'static str) -> Response {
    Response::new(200, kind, body).with_header(
        "Content-Security-Policy",
        "default-src 'self'; img-src 'self' data:; style-src 'self'; script-src 'self'; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'",
    )
}

fn no_printer() -> Response {
    Response::error(404, "No printer by that name (was it renamed or removed?).")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_names_that_cannot_be_rebound() {
        for h in [
            "localhost:7380",
            "127.0.0.1:7380",
            "[::1]:7380",
            "LOCALHOST",
            "10.0.0.5:3002",
            "[fe80::1]:80",
            "mica-box:3002",
            "printers.local",
        ] {
            assert!(trusted_name(h), "{h}");
        }
        for h in [
            "evil.example:7380",
            "mica.tailnet.ts.net",
            "",
            "a b",
            "x.local.evil.com",
        ] {
            assert!(!trusted_name(h), "{h}");
        }
    }

    #[test]
    fn open_in_mica_follows_the_page_host() {
        let u = "http://{host}:3000/";
        assert_eq!(
            open_url(u, Some("mica-box:3002")).unwrap(),
            "http://mica-box:3000/"
        );
        assert_eq!(
            open_url(u, Some("[fd00::5]:3002")).unwrap(),
            "http://[fd00::5]:3000/"
        );
        assert_eq!(
            open_url(u, Some("10.0.0.5")).unwrap(),
            "http://10.0.0.5:3000/"
        );
        assert_eq!(open_url(u, Some("x\"><script>")), None);
        assert_eq!(open_url(u, None), None);
        assert_eq!(
            open_url("https://m.example/", None).unwrap(),
            "https://m.example/"
        );
    }

    /// `#` followed by 3 to 8 hex digits and then no further word character.
    fn has_hex_colour(text: &str) -> bool {
        let b = text.as_bytes();
        (0..b.len()).any(|i| {
            if b[i] != b'#' {
                return false;
            }
            let n = b[i + 1..]
                .iter()
                .take_while(|c| c.is_ascii_hexdigit())
                .count();
            let next = b.get(i + 1 + n).copied().unwrap_or(b' ');
            (3..=8).contains(&n) && !(next.is_ascii_alphanumeric() || next == b'_')
        })
    }

    #[test]
    fn the_page_shows_colour_names_not_hex_codes() {
        // Colours are CSS data in the stylesheet; the page's words and the colours it gets
        // from the server (`css` + `name`) carry no hex codes.
        assert!(has_hex_colour("color: #8b7cf6;") && !has_hex_colour("#printers"));
        for text in [INDEX_HTML, APP_JS] {
            assert!(!has_hex_colour(text));
        }
        let c = fleet::colour_json(Some("#F8F81C"));
        assert!(!has_hex_colour(&c.to_string()), "{c}");
    }
}
