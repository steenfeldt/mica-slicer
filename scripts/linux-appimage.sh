#!/bin/sh
# Runs INSIDE an Ubuntu 22.04 (glibc 2.35) x86_64 container, started by
# scripts/release.sh: builds mica-gui for Linux and assembles the AppImage.
#
#   linux-appimage.sh <cargo profile> <version> <out dir> <rust version>   (repo at /src)
#
# Why a container and not cargo-zigbuild: mica-videodec links the system's GStreamer
# (core + gst-plugins-base app/video) dynamically, so linking needs the target's GStreamer
# libraries, which Ubuntu provides here. Ubuntu 22.04 (supported until 2027) sets the glibc
# baseline at 2.35: Ubuntu 22.04+, Debian 12+, Fedora 36+ (Debian 11 is end of life).
#
# GStreamer in the AppImage: AppRun uses the system's GStreamer when it is installed (its
# plugins, including the distribution's H.264 decoder, must match its own core), and only
# otherwise falls back to the core + base libraries bundled in usr/lib/fallback, so Mica
# starts everywhere; without the system's GStreamer an H.264 camera then says it needs
# GStreamer with an H.264 decoder. No codec plugin is bundled.
set -eu
profile="$1"
version="$2"
out="$3"
rust_version="$4"
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq >/dev/null
apt-get install -y -qq --no-install-recommends ca-certificates curl build-essential pkg-config \
  file desktop-file-utils libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev >/dev/null
if [ ! -x "$CARGO_HOME/bin/cargo" ]; then
  curl -fsSL https://sh.rustup.rs | sh -s -- -y -q --profile minimal --default-toolchain "$rust_version" >/dev/null
fi
export PATH="$CARGO_HOME/bin:$PATH"
cd /src
# Keep the host's target/ untouched (a different OS and glibc).
export CARGO_TARGET_DIR=/src/target/linux-container
# Packaged: never fall back to the checkout's upstream/ at runtime.
export MICA_PACKAGED=1
cargo build --locked --profile "$profile" -p mica-gui --bin mica-gui
dir="$profile"
[ "$profile" = dev ] && dir=debug
bin="$CARGO_TARGET_DIR/$dir/mica-gui"
strip "$bin"
if grep -a -q -i -E 'openh264|WelsCreateDecoder' "$bin"; then
  echo "error: the Linux binary contains OpenH264 code" >&2
  exit 1
fi
echo "ok: no OpenH264 in mica-gui (linux)"

app="$out/AppDir"
rm -rf "$app"
mkdir -p "$app/usr/bin" "$app/usr/lib/fallback" "$app/usr/share/applications" \
  "$app/usr/share/icons/hicolor/512x512/apps"
cp "$bin" "$app/usr/bin/mica-gui"
# The fallback GStreamer: every library the binary links except glibc itself.
ldd "$bin" | awk '/=> \// {print $3}' | while read -r lib; do
  case "$(basename "$lib")" in
    libc.so*|libm.so*|libdl.so*|libpthread.so*|librt.so*|libresolv.so*|ld-linux*|libgcc_s.so*) ;;
    *) cp -L "$lib" "$app/usr/lib/fallback/" ;;
  esac
done
ls "$app/usr/lib/fallback"
cp crates/mica-gui/assets/icon/mica-512.png "$app/mica-slicer.png"
cp crates/mica-gui/assets/icon/mica-512.png "$app/usr/share/icons/hicolor/512x512/apps/mica-slicer.png"
cp crates/mica-gui/packaging/linux/mica-slicer.desktop "$app/mica-slicer.desktop"
cp crates/mica-gui/packaging/linux/mica-slicer.desktop "$app/usr/share/applications/"
cp crates/mica-gui/packaging/linux/AppRun "$app/AppRun"
chmod +x "$app/AppRun"
desktop-file-validate "$app/mica-slicer.desktop"

tool=/opt/cache/appimagetool-1.9.0
if [ ! -x "$tool" ]; then
  mkdir -p /opt/cache
  curl -fsSL -o "$tool" https://github.com/AppImage/appimagetool/releases/download/1.9.0/appimagetool-x86_64.AppImage
  chmod +x "$tool"
fi
cd "$out"
ARCH=x86_64 APPIMAGE_EXTRACT_AND_RUN=1 "$tool" --no-appstream AppDir "MicaSlicer_${version}_x86_64.AppImage"
rm -rf "$app"
