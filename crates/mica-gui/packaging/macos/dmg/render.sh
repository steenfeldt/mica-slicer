#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# Provenance: original
#
# Renders the disk image backgrounds (variant-N.svg) to variant-N.png (1x, 640x440: the
# 640x420 window plus a bottom bleed) and variant-N@2x.png (Retina); dmgbuild combines each
# pair into one multi-resolution TIFF.
# Text is IBM Plex Sans from crates/mica-gui/assets/fonts (embedded for the render).
# Needs Google Chrome (headless); only needed after changing an SVG.
#
#   render.sh [--mock DIR]   --mock also writes DIR/variant-N.png: a mock of the mounted disk
#                            window (Finder title bar, real app icon, the Applications folder
#                            icon, Finder-style labels) for choosing a design
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../../../../.." && pwd)"
fonts="$root/crates/mica-gui/assets/fonts"
mock=""
[ "${1:-}" = --mock ] && mock="$2"
chrome="${CHROME:-/Applications/Google Chrome.app/Contents/MacOS/Google Chrome}"
[ -x "$chrome" ] || { echo "Google Chrome not found (set CHROME)" >&2; exit 1; }
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

face() { printf "@font-face{font-family:'IBM Plex Sans';font-weight:%s;src:url(data:font/ttf;base64,%s)}" \
  "$1" "$(base64 -i "$fonts/$2")"; }
fontcss="$(face 400 IBMPlexSans-Regular.ttf)$(face 500 IBMPlexSans-Medium.ttf)$(face 600 IBMPlexSans-SemiBold.ttf)"

shot() { # html out width height scale
  "$chrome" --headless --disable-gpu --hide-scrollbars --no-first-run --no-default-browser-check \
    --user-data-dir="$tmp/profile" --force-device-scale-factor="$5" --window-size="$3,$4" \
    --screenshot="$2" "file://$1" >/dev/null 2>&1
  [ -f "$2" ] || { echo "Chrome wrote no $2" >&2; exit 1; }
}

if [ -n "$mock" ]; then
  mkdir -p "$mock"
  res=/System/Library/CoreServices/CoreTypes.bundle/Contents/Resources
  sips -s format png -Z 256 "$res/ApplicationsFolderIcon.icns" --out "$tmp/apps.png" >/dev/null
  sips -s format png -Z 256 "$res/AliasBadgeIcon.icns" --out "$tmp/alias.png" >/dev/null
  cp "$root/crates/mica-gui/assets/icon/mica-512.png" "$tmp/mica.png"
fi

for svg in "$here"/variant-*.svg; do
  name="$(basename "$svg" .svg)"
  {
    printf '<!doctype html><meta charset="utf-8"><style>%s html,body{margin:0;background:#000}svg{display:block}</style>' "$fontcss"
    cat "$svg"
  } >"$tmp/$name.html"
  shot "$tmp/$name.html" "$here/$name.png" 640 440 1
  shot "$tmp/$name.html" "$here/$name@2x.png" 640 440 2
  echo "rendered $name.png, $name@2x.png"
  if [ -n "$mock" ]; then
    cat >"$tmp/$name-mock.html" <<HTML
<!doctype html><meta charset="utf-8"><style>
html,body{margin:0;background:#00000000}
.win{position:absolute;left:40px;top:40px;width:640px;border-radius:12px;overflow:hidden;
  box-shadow:0 0 0 .5px rgba(0,0,0,.35),0 22px 60px rgba(0,0,0,.45)}
.bar{height:32px;background:#fff;display:flex;align-items:center;font:600 15px -apple-system,sans-serif;color:#3a3a3c}
.dot{width:13px;height:13px;border-radius:50%;margin-left:9px;background:#ececec;box-shadow:inset 0 0 0 .5px #c8c8c8}
.dot:first-child{margin-left:12px}
.disk{width:14px;height:16px;margin:0 6px 0 22px;background:linear-gradient(#f4f4f4,#d8d8d8);border-radius:2px;box-shadow:inset 0 0 0 .5px #aaa}
.content{position:relative;width:640px;height:420px;background:url($here/$name@2x.png) 0 0/640px 440px}
.icon{position:absolute;width:128px;height:128px;transform:translate(-64px,-64px)}
.label{position:absolute;transform:translate(-50%,-50%);font:13px -apple-system,sans-serif;color:#000;white-space:nowrap}
</style>
<div class="win"><div class="bar"><span class="dot"></span><span class="dot"></span><span class="dot"></span><span class="disk"></span>Mica Slicer</div>
<div class="content">
<img class="icon" src="$tmp/mica.png" style="left:160px;top:175px">
<div class="label" style="left:160px;top:258px">Mica Slicer</div>
<img class="icon" src="$tmp/apps.png" style="left:480px;top:175px">
<img class="icon" src="$tmp/alias.png" style="left:480px;top:175px">
<div class="label" style="left:480px;top:258px">Applications</div>
</div></div>
HTML
    shot "$tmp/$name-mock.html" "$mock/$name.png" 720 532 2
    echo "mock $mock/$name.png"
  fi
done
