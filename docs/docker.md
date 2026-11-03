# Mica in a container

Mica Slicer can run on a home server and be used from a browser. The image is
linuxserver.io's [Selkies](https://github.com/linuxserver/docker-baseimage-selkies) base with
Mica added: Selkies runs an X display, starts Mica in it, and streams the screen to the browser
(WebRTC or WebSockets), with keyboard, mouse, clipboard and file transfer. This is the whole
desktop app, not a separate web version.

Files: `crates/mica-gui/packaging/docker/Dockerfile` and `compose.yaml` next to it.

## Build and run

From the repository root:

```sh
docker compose -f crates/mica-gui/packaging/docker/compose.yaml up -d --build
```

or by hand:

```sh
docker build -f crates/mica-gui/packaging/docker/Dockerfile -t mica-slicer .
docker run -d --name mica-slicer -p 3000:3000 -p 3001:3001 --shm-size=1g \
  -e PUID=$(id -u) -e PGID=$(id -g) -v "$PWD/mica-config:/config" mica-slicer
```

Then open `http://<host>:3000`. The image builds for the machine it is built on (x86-64 or
arm64). The build compiles Mica from this checkout and fetches the pinned OrcaSlicer profiles
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

Whoever reaches the page drives Mica, and Mica can start prints on your printers. Keep the
ports on your own network or behind Tailscale; do not forward them to the internet. Selkies
can ask for a login (`CUSTOM_USER`, `PASSWORD`) and serves HTTPS on port 3001 with a
self-signed certificate.

Mica is licensed AGPL-3.0: if you offer a modified Mica to other people over a network, offer
them its source as well.
