# Mica in a container

Mica Slicer can run on a home server and be used from a browser. The image is
linuxserver.io's [Selkies](https://github.com/linuxserver/docker-baseimage-selkies) base with
Mica added: Selkies runs an X display, starts Mica in it, and streams the screen to the browser
(WebRTC or WebSockets), with keyboard, mouse, clipboard and file transfer. This is the whole
desktop app, not a separate web version. Next to it runs Mica's **web dashboard**
(`docs/web-dashboard.md`) on port 3002: a light page for a phone with the printers' status,
their cameras and the library.

Files: `crates/mica-gui/packaging/docker/Dockerfile` and `compose.yaml` next to it.

## Build and run

From the repository root:

```sh
docker compose -f crates/mica-gui/packaging/docker/compose.yaml up -d --build
```

or by hand:

```sh
docker build -f crates/mica-gui/packaging/docker/Dockerfile -t mica-slicer .
docker run -d --name mica-slicer -p 3000:3000 -p 3001:3001 -p 3002:3002 --shm-size=1g \
  -e PUID=$(id -u) -e PGID=$(id -g) -v "$PWD/mica-config:/config" mica-slicer
```

Then open `http://<host>:3000` for Mica, `http://<host>:3002` for the dashboard. The image
builds for the machine it is built on (x86-64 or arm64). The build compiles Mica from this checkout and fetches the pinned OrcaSlicer profiles
(`scripts/fetch-upstream.sh --profiles orca`), so it needs network access.

## Where things are kept

Everything Mica writes lives under `/config`, the container's home folder. Mount it from the
host, or it is lost with the container:

| Path | Contents |
|---|---|
| `/config/.config/mica-slicer` | Preferences, profiles, `printers.toml`, the printer secrets file, downloaded vendor profiles |
| `/config/.local/share/mica-slicer` | The library's index and thumbnails |
| `/config/Desktop` | Files uploaded through the Selkies side panel |

Mount your models and projects too (a NAS share mounted on the host, for example
`-v /mnt/3dprint:/prints`); Mica's file dialogs show the container's folders.

`PUID` and `PGID` make the files in the mounted folders belong to that host user.

## The web dashboard (port 3002)

The image runs `mica-web` as a second service (an s6-rc service, `svc-mica-web`, beside
Selkies' own), as the same user as Mica (`abc`) and with the same home (`/config`). So it
reads the same `printers.toml`, the same printer secrets file (which only that user may read)
and the same library index; it never writes any of them. Inside the container it listens on
all interfaces; the published port decides who can reach it.

It is **read-only by default**: status, cameras and library only. To let it take G-code
uploads and send or start prints, set

```yaml
      - MICA_WEB_ALLOW_PRINTING=1
```

It answers requests addressed by IP address, a short host name (`mica-box`) or a `.local`
name. Opened by a full DNS name, such as a Tailscale name, that name must be listed:
`MICA_WEB_ALLOW_HOSTS=mica-box.tailnet-name.ts.net` (comma-separated; `.tailnet-name.ts.net`
for every name under it). Its **Open in Mica** button goes to port 3000 of the host the
dashboard was opened with; `MICA_WEB_OPEN_URL` changes that (`{host}` stands for that host,
e.g. `https://{host}:3001/`). The lines are in `compose.yaml`, commented out.

The dashboard has no login of its own: `CUSTOM_USER` / `PASSWORD` protect only ports 3000 and
3001. Do not publish 3002 where you would not publish Mica itself, or leave it out of `ports`.

## Updates

The image is updated as a whole: rebuild or pull it and recreate the container; `/config`
keeps everything. The image sets `MICA_UPDATES=notify`, so Mica checks for new versions but
only announces them (no *Install*). Mica also recognizes a container by itself
(`/.dockerenv`, `/run/.containerenv`); see `docs/release.md`.

## Graphics and speed

Mica draws with Vulkan. Without a GPU in the container it uses Mesa's software Vulkan
(lavapipe): the plate, menus and settings are fine, big previews are slow, and Selkies encodes
the video on the CPU too. Pass an Intel or AMD GPU in with `--device /dev/dri:/dev/dri` (the
`devices` lines in `compose.yaml`): Mica then renders on it and Selkies can encode on it.

The image streams at the browser's size in points at scale 1
(`SELKIES_USE_CSS_SCALING="true|locked"`, `SELKIES_SCALING_DPI=96`). On a Retina screen
that is a quarter of the pixels to render and encode, slightly soft; and Mica's interface
comes out at its normal size whether Mica or the browser starts first (Mica reads the
display scale once, at start).

## Security

Whoever reaches the page drives Mica, and Mica can start prints on your printers; whoever
reaches port 3002 sees the printers, their cameras and the library (and can send prints when
`MICA_WEB_ALLOW_PRINTING=1`). Keep the ports on your own network or behind Tailscale; do not forward them to the internet. Selkies
can ask for a login (`CUSTOM_USER`, `PASSWORD`) and serves HTTPS on port 3001 with a
self-signed certificate.

Mica is licensed AGPL-3.0: if you offer a modified Mica to other people over a network, offer
them its source as well.
