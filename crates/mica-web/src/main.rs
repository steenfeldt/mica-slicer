// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! `mica-web`: Mica's web dashboard (see `docs/web-dashboard.md`).

use mica_printers::secrets::{self, MemoryStore, SecretStore};
use mica_web::{Dashboard, Options};
use std::{net::SocketAddr, path::PathBuf, process::ExitCode, sync::Arc};

const DEFAULT_LISTEN: &str = "127.0.0.1:7380";

const HELP: &str = "\
mica-web: Mica Slicer's web dashboard (printers, cameras, uploads, library)

USAGE: mica-web [OPTIONS]

OPTIONS:
  --listen ADDR:PORT    Where to listen (default 127.0.0.1:7380, this computer only).
                        Any other address makes the dashboard reachable by everyone who
                        can reach that address: it has no login. Use a private network.
  --allow-host NAME     Also answer requests addressed to NAME, a full DNS name such as
                        mica.tailnet.ts.net (or .tailnet.ts.net for all names under it).
                        IP addresses, localhost, short names (mica-box) and .local names
                        are always answered; other names are refused (DNS rebinding).
                        Repeatable; also MICA_WEB_ALLOW_HOSTS (comma-separated).
  --open-url URL        Where \"Open in Mica\" goes (the streamed Mica window); {host}
                        stands for the name the page was opened with, e.g.
                        https://{host}:3001/. Also MICA_WEB_OPEN_URL.
  --allow-printing      Take G-code uploads and send / start prints from the page. Off by
                        default: the dashboard is read-only (status, cameras, library).
                        Also MICA_WEB_ALLOW_PRINTING=1.
  --printers FILE       The printer list (default: printers.toml in Mica's settings
                        folder, MICA_CONFIG_DIR when set).
  --library DIR         The library's data folder (default: Mica's).
  -h, --help            This help.
  -V, --version         The version.
";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("mica-web: {e}");
            ExitCode::FAILURE
        }
    }
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

fn run() -> Result<(), String> {
    let mut listen: SocketAddr = DEFAULT_LISTEN.parse().expect("valid default");
    let mut open_url = env("MICA_WEB_OPEN_URL");
    let mut printers = None;
    let mut library = None;
    let mut allow_printing = env("MICA_WEB_ALLOW_PRINTING")
        .is_some_and(|v| matches!(v.trim(), "1" | "true" | "yes" | "on"));
    let mut allowed_hosts: Vec<String> = env("MICA_WEB_ALLOW_HOSTS")
        .map(|v| {
            v.split([',', ' '])
                .filter(|h| !h.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut value = |what: &str| {
            args.next()
                .ok_or_else(|| format!("{a} needs {what} (see --help)"))
        };
        match a.as_str() {
            "--listen" => {
                let v = value("an address and port")?;
                listen = v
                    .parse()
                    .map_err(|_| format!("--listen {v}: expected ADDR:PORT, e.g. 0.0.0.0:7380"))?;
            }
            "--allow-host" => allowed_hosts.push(value("a host name")?),
            "--open-url" => open_url = Some(value("a URL")?),
            "--allow-printing" => allow_printing = true,
            "--printers" => printers = Some(PathBuf::from(value("a file")?)),
            "--library" => library = Some(PathBuf::from(value("a folder")?)),
            "-h" | "--help" => {
                print!("{HELP}");
                return Ok(());
            }
            "-V" | "--version" => {
                println!("mica-web {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            other => return Err(format!("unknown option {other} (see --help)")),
        }
    }
    if let Some(u) = &open_url {
        let ok = (u.starts_with("http://") || u.starts_with("https://"))
            && !u.contains(|c: char| c.is_whitespace() || c == '"' || c == '<' || c == '>');
        if !ok {
            return Err(format!(
                "--open-url {u}: expected an http:// or https:// URL"
            ));
        }
    }
    // Printer and camera secrets come from the window's store: the macOS Keychain, on Linux
    // `secrets.toml` in Mica's settings folder (only its owner may read it, so the dashboard
    // runs as the same user as Mica). `api_key_env` / `password_env` work everywhere.
    let secrets: Arc<dyn SecretStore> = if mica_core::user_data::isolated() {
        // Started by a test harness: never the real secret store.
        Arc::new(MemoryStore::default())
    } else {
        Arc::from(secrets::system())
    };
    let options = Options {
        listen,
        printers: printers.unwrap_or_else(mica_printers::config::default_path),
        library: library.unwrap_or_else(mica_library::default_data_dir),
        secrets,
        open_url,
        loopback_only: false,
        allowed_hosts,
        allow_printing,
    };
    eprintln!("mica-web: printers from {}", options.printers.display());
    eprintln!("mica-web: library from {}", options.library.display());
    if options.secrets.available() {
        eprintln!("mica-web: secrets from the {}", options.secrets.label());
    } else {
        eprintln!(
            "mica-web: printer secrets: {}; printers need api_key_env / password_env in printers.toml",
            options.secrets.label()
        );
    }
    eprintln!(
        "mica-web: {}",
        if allow_printing {
            "uploads and printing are ON (--allow-printing)"
        } else {
            "read-only: no uploads or prints (--allow-printing turns them on)"
        }
    );
    if !listen.ip().is_loopback() {
        eprintln!(
            "mica-web: listening on {listen}: the dashboard has no login; anyone who can reach this address can see the printers, their cameras and the library{}",
            if allow_printing {
                ", and send them files"
            } else {
                ""
            }
        );
    }
    let dashboard =
        Dashboard::start(options).map_err(|e| format!("cannot listen on {listen}: {e}"))?;
    eprintln!("mica-web: open http://{}", dashboard.addr());
    dashboard.join();
    Ok(())
}
