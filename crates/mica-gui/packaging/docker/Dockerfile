# SPDX-License-Identifier: AGPL-3.0-only
# Provenance: original
#
# Mica Slicer in a container, streamed to the browser by linuxserver.io's Selkies base image
# (docs/docker.md). Built from this checkout for the machine's own architecture (x86-64 or
# arm64):
#
#   docker build -f crates/mica-gui/packaging/docker/Dockerfile -t mica-slicer .
#
# The build stage fetches the pinned OrcaSlicer profiles (scripts/fetch-upstream.sh) and
# bundles them as a release does; nothing from upstream is compiled in.

FROM ubuntu:24.04 AS build
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update -qq && apt-get install -y -qq --no-install-recommends ca-certificates curl \
      git build-essential pkg-config libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev \
    && rm -rf /var/lib/apt/lists/*
ENV CARGO_HOME=/opt/cargo RUSTUP_HOME=/opt/rustup PATH=/opt/cargo/bin:$PATH
RUN curl -fsSL https://sh.rustup.rs | sh -s -- -y -q --profile minimal --default-toolchain stable
WORKDIR /src
COPY . .
RUN scripts/fetch-upstream.sh --profiles orca
# Packaged: never fall back to the checkout's upstream/ at runtime.
ENV MICA_PACKAGED=1
RUN --mount=type=cache,target=/opt/cargo/registry --mount=type=cache,target=/src/target \
    cargo build --locked --release -p mica-gui --bin mica-gui -p mica-web --bin mica-web \
    && mkdir -p /out/usr/bin /out/usr/share/mica-slicer/profiles \
    && cp target/release/mica-gui target/release/mica-web /out/usr/bin/ \
    && strip /out/usr/bin/mica-gui /out/usr/bin/mica-web \
    && cargo xtask bundle-profiles /out/usr/share/mica-slicer/profiles

FROM lscr.io/linuxserver/baseimage-selkies:ubuntunoble
ENV TITLE="Mica Slicer"
# zenity: file dialogs without a desktop portal. mesa-vulkan-drivers: the GPU's Vulkan driver
# when /dev/dri is passed in, else software Vulkan (lavapipe). GStreamer: H.264 cameras.
RUN apt-get update -qq && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq \
      --no-install-recommends zenity mesa-vulkan-drivers libvulkan1 libxkbcommon-x11-0 \
      libgstreamer1.0-0 libgstreamer-plugins-base1.0-0 \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /out /opt/mica
COPY crates/mica-gui/assets/icon/mica-512.png /usr/share/selkies/www/icon.png
COPY crates/mica-gui/assets/icon/mica-512.png /usr/share/selkies/web/icon.png
# Mica fills the streamed screen, without a title bar, and starts with the session.
RUN sed -i 's|<applications>|<applications><application class="mica-slicer"><decor>no</decor><maximized>true</maximized></application>|' \
      /etc/xdg/openbox/rc.xml \
    && printf '%s\n' '/opt/mica/usr/bin/mica-gui' > /defaults/autostart
# The web dashboard (docs/web-dashboard.md) on port 3002: a second s6 service next to the
# streamed Mica, as the same user (abc) with the same home (/config). Read-only unless
# MICA_WEB_ALLOW_PRINTING=1.
COPY crates/mica-gui/packaging/docker/s6-rc.d/ /etc/s6-overlay/s6-rc.d/
# The image is updated as a whole: Mica only announces new versions.
ENV MICA_UPDATES=notify
# Stream at the browser's size in points, at scale 1: a quarter of the pixels of a Retina
# screen to render and encode, and Mica's interface at its normal size whichever of Mica and
# the browser comes first (Mica reads the scale once, at start).
ENV SELKIES_USE_CSS_SCALING="true|locked" SELKIES_SCALING_DPI=96
EXPOSE 3000 3001 3002
