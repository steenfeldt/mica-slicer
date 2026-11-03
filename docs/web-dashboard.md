<!--
SPDX-License-Identifier: AGPL-3.0-only
Provenance: original
-->

# Web dashboard (`mica-web`)

A small companion page for running Mica on a home server: the printers Mica knows with their
live status, their cameras and the model library, in any browser (a phone too), and, when you
allow it, G-code uploads and prints. The full Mica window is streamed separately (the Docker
image, `docs/docker.md`, runs both); the dashboard's **Open in Mica** button links there.

It is one binary, `mica-web`, serving one static page. The page (`crates/mica-web/src/page/`:
HTML, CSS and a little JavaScript, no framework, no build step) is embedded in the binary and
only draws the JSON the server sends; the logic is in Mica's crates.

## Running it

```sh
cargo build --release -p mica-web
MICA_CONFIG_DIR=/srv/mica/config ./target/release/mica-web --open-url https://mica-box.example.ts.net/
# mica-web: open http://127.0.0.1:7380
```

| Flag | Environment | Meaning |
|---|---|---|
| `--listen ADDR:PORT` | | Where to listen. Default `127.0.0.1:7380`: this computer only. |
| `--allow-printing` | `MICA_WEB_ALLOW_PRINTING=1` | Take G-code uploads and send / start prints. **Off by default**: the dashboard is read-only. |
| `--allow-host NAME` | `MICA_WEB_ALLOW_HOSTS` (comma-separated) | Also answer requests addressed to the DNS name `NAME` (`mica-box.tailnet.ts.net`; `.tailnet.ts.net` allows every name under it). Repeatable. See *Security*. |
| `--open-url URL` | `MICA_WEB_OPEN_URL` | Where **Open in Mica** goes. `{host}` stands for the name the dashboard was opened with (`https://{host}:3001/`). Without it the button is hidden. |
| `--printers FILE` | | The printer list. Default: `printers.toml` in Mica's settings folder. |
| `--library DIR` | | The library's data folder. Default: Mica's (`$MICA_CONFIG_DIR/library` when set). |

`MICA_CONFIG_DIR` (or `HOME`) works as for the window: point the dashboard and Mica at the
same settings folder and both see the same printers, secrets and library.

## Read-only unless you allow printing

By default the dashboard **changes nothing**: it shows printer status, cameras and the library.
The page then has no upload, send or start controls, and the server refuses every request that
would send a file or start a print (`403`, saying how to turn it on), however it is made.
`/api/info` tells the page which mode it is in (`"printing": false`).

Start it with `--allow-printing` (or `MICA_WEB_ALLOW_PRINTING=1`) to add:

- **Uploads** of a dropped or picked `.gcode` / `.bgcode` file, through the server to the
  printer with progress, optionally starting the print. Starting is refused while the printer
  is printing.
- **Send** of a G-code file from the library to a printer (from where it is stored),
  optionally starting it.

There is no pause, cancel or other printer control. The U1's tool → slot mapping and Mica's
preflight checks are the window's; a print started here uses the printer's own defaults.

## What it reads

- **Printers** come from `printers.toml`, exactly as the window's *My printers* keeps it (the
  shared code is `crates/mica-printers`). The file is re-read when it changes. Printers are
  polled only while a page is open (at the window's rates: every 2 s while printing, 10 s
  otherwise, backing off when a printer is unreachable).
- **Cameras** are played on the server (MJPEG, snapshots, RTSP, camera-streamer WebRTC, as in
  the window), re-encoded as JPEG and sent to the browser as MJPEG. Camera addresses and
  logins never reach the browser. A camera runs only while someone watches it.
- **The library** is read, never written: the dashboard reads the index Mica saved (a private
  copy of `library/index.redb`) and its thumbnails. Adding folders and rescanning happen in the
  Mica window, which keeps owning the index.

## Secrets

`printers.toml` never holds secrets, only references, and the dashboard reads them from the
same store as the window (`mica-printers/src/secrets.rs`, see `docs/gui-printers.md`): the
macOS Keychain, on Linux `secrets.toml` in Mica's settings folder. That covers printer API keys
and passwords and camera passwords (`camera_keychain`). `api_key_env` / `password_env` entries
read an environment variable of the `mica-web` process instead.

On Linux the secrets file must belong to the user running `mica-web` and be readable by it
alone (mode 0600), as for the window: run the dashboard as the same user as Mica (the Docker
image does). If a secret cannot be read, the printer's card says why (a missing entry, a file
others can read, and so on) and what to do.

## Security

The dashboard has **no login**. Anyone who can reach its address can see the printers, their
cameras and the library, and, with `--allow-printing`, send files to the printers. So:

- By default it listens on 127.0.0.1 only. Any other address must be given with `--listen`,
  and `mica-web` warns about it when it starts.
- Reach it over your own network or Tailscale; never forward the port to the internet.
- It only answers requests addressed to names a web page elsewhere cannot point at it (DNS
  rebinding): IP addresses, `localhost`, single-label names (`mica-box`, LAN or Tailscale
  MagicDNS short names) and `.local` names. Any other DNS name, such as a full Tailscale name
  (`mica-box.tailnet.ts.net`) or the name a reverse proxy like `tailscale serve` passes on,
  must be listed with `--allow-host` / `MICA_WEB_ALLOW_HOSTS`; others get `421`.
- Every request that changes something carries a header a plain cross-site form cannot send,
  and the server never allows cross-site scripts to send it.
- The page is served with a strict content security policy (its own scripts and styles only).

## In a container

The Docker image (`docs/docker.md`) runs `mica-web` next to the streamed Mica, on port 3002,
as Mica's user with Mica's settings folder, read-only unless `MICA_WEB_ALLOW_PRINTING=1`, with
**Open in Mica** pointing to port 3000 of the same host.

`mica-web` needs no display or GPU. On Linux, `mica-camera` decodes H.264 (RTSP and WebRTC
cameras) with GStreamer: building needs `libgstreamer1.0-dev` and
`libgstreamer-plugins-base1.0-dev`; at run time the GStreamer libraries and an H.264 decoder
plugin. MJPEG and snapshot cameras need neither.

## JSON API

The page uses these; they are not a stable public API.

| Request | Answer |
|---|---|
| `GET /api/info` | Version, the Open in Mica URL, whether printing is allowed, where printers and secrets come from |
| `GET /api/printers` | Every printer: state, job, temperatures, loaded filaments (colour as CSS value + name), camera availability, the last upload |
| `GET /api/printers/ID/camera` | The camera as `multipart/x-mixed-replace` JPEG |
| `POST /api/printers/ID/upload?name=FILE&start=0\|1` | With `--allow-printing`: the file as the body; `202` once received, progress in `/api/printers` |
| `POST /api/printers/ID/send` | With `--allow-printing`: `{"model": ID, "start": bool}`, a library G-code file to the printer |
| `GET /api/library?q=&gcode=1&offset=&limit=` | Library search, newest first |
| `GET /api/library/ID/thumbnail` | The model's cached picture |

`POST` requests are refused (`403`) without `--allow-printing`, and need the header
`X-Mica-Dashboard: 1`.

## Why a separate binary

`mica-web` is its own crate and binary rather than a `mica serve` subcommand: the CLI stays
free of the camera stack (RTSP, WebRTC, video decoding) and the library index, and the
server never links the window's GPU and windowing crates (eframe, wgpu). It shares the
printer list, secrets, camera choice and polling with the window through `mica-printers`, the
printer connections through `mica-printhost`, the cameras through `mica-camera` and the
library through `mica-library`.
