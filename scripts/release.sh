#!/usr/bin/env bash
# Builds and signs a Mica Slicer tester release and prepares its publication: the packages go
# to a GitHub release, the update manifest to the website repository (a separate repository,
# $MICA_WEBSITE_DIR, default ../mica-website): its site/updates/<channel>/latest.json.
# One-time setup and every variable are described in docs/release.md.
#
#   scripts/release.sh [--dry-run] [--prebuilt] [--publish] [--version X.Y.Z]
#                      [--platforms macos,windows,linux] [--profile release]
#
#   (default)    build, sign and package; write dist/<version>/ (packages, their .sig update
#                signatures, latest.json, release-notes.md) and PRINT the publishing commands
#                without running any of them. Nothing leaves this computer.
#   --publish    also create the GitHub pre-release (`gh release create --prerelease`: Mica is
#                alpha, pre-release software) with the packages and
#                signatures, then copy the manifest to the website repository's
#                site/updates/<channel>/latest.json and print how to commit and deploy it from
#                there (the deploy stays a manual step).
#   --prebuilt   skip building: use the packages and .sig files already in dist/<version>/
#                (e.g. `--prebuilt --publish` after checking a default run's packages)
#   --dry-run    everything except code signing, notarization and update signatures: unsigned
#                (ad-hoc signed on macOS) local test packages, never publishable. With
#                CARGO_PACKAGER_SIGN_PRIVATE_KEY set, it still writes update signatures (a
#                throwaway test key, for updater tests).
#   --version    sets the workspace version first (otherwise the current one is released)
#   --platforms  any of macos, windows, linux (default: all three)
#   --profile    cargo profile (default release; `fast` for quicker test packages)
#
# Release notes: the `## <version>` section of CHANGELOG.md, written once per release. The
# app shows the same section after the update (Help › What's new…), the GitHub release and the
# update manifest take it from there too (scripts/update-manifest.py --changelog). A release
# without its section stops before building; a dry run uses a one-line placeholder instead.
#
# Publishing order: the GitHub release first, then the website deploy, so the manifest never
# names a file that is not there yet.
#
# Secrets are read at run time from environment variables or the macOS Keychain, never
# from this repository:
#   MICA_SIGN_IDENTITY       "Developer ID Application: Name (TEAMID)" (macOS signing)
#   MICA_NOTARY_PROFILE      notarytool keychain profile name (default: mica-notary)
#   CARGO_PACKAGER_SIGN_PRIVATE_KEY           path of the minisign private key file
#   CARGO_PACKAGER_SIGN_PRIVATE_KEY_PASSWORD  its password (else Keychain item
#                                             "mica-update-key", account $USER)
#   MICA_WINDOWS_SIGN_COMMAND  optional: a command run with the installer's path to sign it
#   MICA_UPDATE_PUBKEY         build-time trusted public keys (newlines or commas), else
#                              crates/mica-update/update-key.pub (one key per line)
#   gh (GitHub CLI) uses its own login (`gh auth login`); only --publish calls it.
# Overrides (tests): MICA_RELEASE_REPO (GitHub owner/name; default: the workspace
# `repository`), MICA_RELEASE_DIST (default dist/), MICA_WEBSITE_DIR (the website repository,
# default ../mica-website next to this one), MICA_RELEASE_SITE (default $MICA_WEBSITE_DIR/site),
# MICA_RELEASE_ASSET_BASE (package URL prefix instead of the GitHub release, for a local
# updater test; refused with --publish), MICA_RELEASE_CHANGELOG (default CHANGELOG.md).
#
# Packaged builds never enable the `openh264` feature; the script checks every binary.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

dry_run=0
publish=0
prebuilt=0
new_version=""
platforms="macos,windows,linux"
profile="release"
while [ $# -gt 0 ]; do
  case "$1" in
    --dry-run) dry_run=1 ;;
    --publish) publish=1 ;;
    --prebuilt) prebuilt=1 ;;
    --version) new_version="$2"; shift ;;
    --notes) echo "--notes is gone: write the release notes in CHANGELOG.md (## X.Y.Z)" >&2; exit 2 ;;
    --platforms) platforms="$2"; shift ;;
    --profile) profile="$2"; shift ;;
    -h|--help) sed -n '2,52p' "$0"; exit 0 ;;
    *) echo "unknown option: $1" >&2; exit 2 ;;
  esac
  shift
done

say() { printf '\n==> %s\n' "$*"; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }
skip() { printf 'SKIPPED: %s\n' "$*"; skipped="$skipped\n  - $*"; }
want() { case ",$platforms," in *",$1,"*) return 0 ;; *) return 1 ;; esac; }
need() { command -v "$1" >/dev/null 2>&1 || die "$1 is not installed ($2)"; }
skipped=""

export PATH="/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH"
# Packaged binaries read profiles only from installed slicers, never from this checkout's
# upstream/ (mica_core::user_data::upstream_dir).
export MICA_PACKAGED=1
[ $dry_run = 1 ] && [ $publish = 1 ] && die "--dry-run packages are never published; drop one of --dry-run and --publish"
[ $prebuilt = 1 ] && [ -n "$new_version" ] && die "--prebuilt publishes the current version; set the version when building"
need cargo "rustup"
[ $prebuilt = 1 ] || need cargo-packager "cargo install --locked cargo-packager@0.11.8"
need python3 "Xcode command line tools"

# The one place the update server is configured: the manifest is a static file of the website.
site_url="https://micaslicer.com"
base_url="$(sed -n 's/^pub const UPDATE_BASE_URL: &str = "\(.*\)";/\1/p' crates/mica-update/src/lib.rs)"
channel="$(sed -n 's/^pub const CHANNEL: &str = "\(.*\)";/\1/p' crates/mica-update/src/lib.rs)"
[ -n "$base_url" ] && [ -n "$channel" ] || die "cannot read UPDATE_BASE_URL / CHANNEL"
case "$base_url" in
  "$site_url"/*) manifest_path="${base_url#"$site_url"/}/$channel/latest.json" ;;
  *) die "UPDATE_BASE_URL ($base_url) is not on $site_url, which serves the website's site/" ;;
esac
# Where the packages are published: GitHub releases of the public repository.
repo="${MICA_RELEASE_REPO:-$(sed -n 's|^repository = "https://github.com/\([^"]*\)"$|\1|p' Cargo.toml | head -1)}"
[ -n "$repo" ] || die "cannot read the GitHub repository from Cargo.toml (or set MICA_RELEASE_REPO)"
dist_root="${MICA_RELEASE_DIST:-$root/dist}"
# The website (micaslicer.com) is its own repository; the manifest goes into its site/ folder
# and is deployed from there.
website_dir="${MICA_WEBSITE_DIR:-$root/../mica-website}"
site_dir="${MICA_RELEASE_SITE:-$website_dir/site}"
website_root="$(dirname "$site_dir")"

if [ $dry_run = 0 ] && [ $prebuilt = 0 ]; then
  [ -z "$(git status --porcelain --untracked-files=no)" ] || die "the working tree has changes; commit first"
fi

# The release notes: CHANGELOG.md's section for a version (scripts/update-manifest.py reads it).
changelog="${MICA_RELEASE_CHANGELOG:-$root/CHANGELOG.md}"
changelog_notes() {
  python3 -I "$root/scripts/update-manifest.py" --notes-only --changelog "$changelog" --version "$1"
}
# Checked before the version changes or anything is built.
if [ -n "$new_version" ] && [ $dry_run = 0 ]; then
  changelog_notes "$new_version" >/dev/null || die "write the release notes for $new_version in $changelog first"
fi

if [ -n "$new_version" ]; then
  say "Setting the workspace version to $new_version"
  python3 -I - "$root/Cargo.toml" "$new_version" <<'PY'
import re, sys
path, version = sys.argv[1], sys.argv[2]
text = open(path).read()
head, sep, rest = text.partition("[workspace.package]")
rest = re.sub(r'(?m)^version = "[^"]*"', f'version = "{version}"', rest, count=1)
open(path, "w").write(head + sep + rest)
PY
  cargo update --workspace --offline -q
fi
version="$(cargo metadata --no-deps --format-version 1 --offline \
  | python3 -I -c 'import json,sys; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"]=="mica-gui"))')"
mode="$( [ $prebuilt = 1 ] && echo 'prebuilt packages' || echo "profile $profile")"
say "Mica Slicer $version, channel $channel, $mode$( [ $dry_run = 1 ] && echo ', DRY RUN')$( [ $publish = 1 ] && echo ', PUBLISH')"
if notes="$(changelog_notes "$version" 2>/dev/null)"; then
  have_notes=1
elif [ $dry_run = 1 ]; then
  have_notes=0
  notes="Mica Slicer $version ($channel)."
  echo "warning: $changelog has no '## $version' section; the dry run uses a placeholder"
else
  changelog_notes "$version" >/dev/null || true
  die "write the release notes for $version in $changelog (## $version) and commit them first"
fi

# Keep these parsing rules in sync with mica_update::public_keys. With a .sig argument,
# report the trusted public key identified by the signature's minisign key ID. This is a
# diagnostic, not a replacement for signature verification; it never reads the private key.
update_keys() {
  python3 -I - "$@" <<'PY'
import base64, hashlib, os, sys
from pathlib import Path

override = os.environ.get("MICA_UPDATE_PUBKEY", "")
text = override if override.strip() else Path("crates/mica-update/update-key.pub").read_text()
keys = []
for line in text.splitlines():
    line = line.strip()
    if not line or line.startswith("#"):
        continue
    for key in line.split(",") if override.strip() else [line]:
        key = key.strip()
        if key and not key.startswith("#"):
            keys.append(key)

if len(sys.argv) == 1:
    print(len(keys))
    sys.exit(0)

def raw_box(encoded):
    lines = base64.b64decode(encoded.strip(), validate=True).decode().splitlines()
    return base64.b64decode(lines[1], validate=True)

try:
    signature = raw_box(Path(sys.argv[1]).read_text())
    if len(signature) != 74:
        raise ValueError("invalid minisign signature length")
    signer_id = signature[2:10]
    for index, key in enumerate(keys, 1):
        try:
            public = raw_box(key)
        except (ValueError, IndexError):
            continue
        if len(public) == 42 and public[2:10] == signer_id:
            fingerprint = hashlib.sha256(public[10:]).hexdigest()[:12]
            role = "main" if index == 1 else "backup"
            print(f"Update signer: trusted {role} key #{index}, SHA-256 {fingerprint} "
                  f"(minisign ID {int.from_bytes(signer_id, 'little'):016X})")
            break
    else:
        print(f"Update signer: no trusted key matches minisign ID "
              f"{int.from_bytes(signer_id, 'little'):016X}")
except (ValueError, IndexError, OSError):
    print("Update signer: could not read the signature's key ID")
PY
}

key_count="$(update_keys)"
say "Trusted update keys: $key_count"
if [ $dry_run = 0 ] && [ $prebuilt = 0 ]; then
  [ "$key_count" -gt 0 ] || die "no trusted update public keys in MICA_UPDATE_PUBKEY or crates/mica-update/update-key.pub (docs/release.md, step 3)"
fi

dist="$dist_root/$version"
# Each run replaces the packages of the platforms it builds (and the manifest).
mkdir -p "$dist"
rm -f "$dist/latest.json"
artifacts=()   # release assets: packages, their .sig files, the .dmg
manifest_entries=()  # "platform=file:format" (update-manifest.py)
# A dry run marks its packages so --prebuilt --publish never ships them.
dry_marker="$dist/DRY-RUN-PACKAGES"
if [ $prebuilt = 0 ]; then
  if [ $dry_run = 1 ]; then : >"$dry_marker"; else rm -f "$dry_marker"; fi
elif [ $publish = 1 ] && [ -e "$dry_marker" ]; then
  die "$dist holds --dry-run packages; build the release without --dry-run first"
fi

profile_dir() { if [ "$1" = dev ]; then echo debug; else echo "$1"; fi; }
pdir="$(profile_dir "$profile")"

# No OpenH264 in a shipped binary (the H.264 patent licence covers only Cisco's binaries).
check_no_openh264() {
  if grep -a -q -i -E 'openh264|WelsCreateDecoder|WelsDecoder' "$1"; then
    die "$1 contains OpenH264 code; packaged builds must not enable the openh264 feature"
  fi
  echo "ok: no OpenH264 in $(basename "$1")"
}

# ---------------------------------------------------------------- update signatures
sign_update() {  # $1 file → writes $1.sig (base64 minisign signature); none in a dry run without a key
  local file="$1"
  rm -f "$file.sig"
  if [ -z "${CARGO_PACKAGER_SIGN_PRIVATE_KEY:-}" ]; then
    [ $dry_run = 1 ] || die "CARGO_PACKAGER_SIGN_PRIVATE_KEY is not set (docs/release.md, step 3)"
    return
  fi
  if [ -z "${CARGO_PACKAGER_SIGN_PRIVATE_KEY_PASSWORD+x}" ]; then
    CARGO_PACKAGER_SIGN_PRIVATE_KEY_PASSWORD="$(security find-generic-password -a "$USER" -s mica-update-key -w 2>/dev/null)" \
      || die "no key password: set CARGO_PACKAGER_SIGN_PRIVATE_KEY_PASSWORD or the Keychain item mica-update-key"
    export CARGO_PACKAGER_SIGN_PRIVATE_KEY_PASSWORD
  fi
  cargo packager signer sign "$file" >/dev/null 2>&1 \
    || die "signing $file failed (wrong key or password?)"
  update_keys "$file.sig"
}

add_package() {  # platform file format: an update package and its signature, if any
  manifest_entries+=("$1=$(basename "$2"):$3")
  artifacts+=("$2")
  [ -f "$2.sig" ] && artifacts+=("$2.sig")
  return 0
}

# ---------------------------------------------------------------- macOS
build_macos() {
  say "macOS: Apple Silicon binary (arm64)"
  export MACOSX_DEPLOYMENT_TARGET=14.0
  rustup target add aarch64-apple-darwin >/dev/null
  cargo build --profile "$profile" --target aarch64-apple-darwin -p mica-gui --bin mica-gui
  check_no_openh264 "$root/target/aarch64-apple-darwin/$pdir/mica-gui"

  say "macOS: app bundle"
  local out="$dist/macos"
  rm -rf "$out" "$dist"/MicaSlicer_*_arm64.*
  # cargo-packager finds the binary in target/<--target>/<--profile>/ (its --binaries-dir
  # option is ignored for configs in Cargo.toml).
  (cd crates/mica-gui && env -u CARGO_PACKAGER_SIGN_PRIVATE_KEY cargo packager --formats app \
    --profile "$profile" --target aarch64-apple-darwin --out-dir "$out")
  local app="$out/Mica Slicer.app"
  [ -d "$app" ] || die "cargo-packager made no app bundle"

  if [ $dry_run = 1 ]; then
    codesign --force --deep --sign - "$app"
    skip "macOS: Developer ID signing, notarization and stapling (dry run; ad-hoc signed)"
  else
    [ -n "${MICA_SIGN_IDENTITY:-}" ] || die "MICA_SIGN_IDENTITY is not set (docs/release.md, step 1)"
    say "macOS: signing with $MICA_SIGN_IDENTITY (hardened runtime)"
    codesign --force --timestamp --options runtime \
      --entitlements crates/mica-gui/packaging/macos/entitlements.plist \
      --sign "$MICA_SIGN_IDENTITY" "$app"
    codesign --verify --strict --verbose=2 "$app"
    say "macOS: notarizing the app"
    local zip="$out/notarize.zip"
    ditto -c -k --keepParent "$app" "$zip"
    xcrun notarytool submit "$zip" --keychain-profile "${MICA_NOTARY_PROFILE:-mica-notary}" --wait
    rm -f "$zip"
    xcrun stapler staple "$app"
    spctl --assess --type execute --verbose "$app"
  fi

  # The updater's package: the (signed, stapled) bundle as .tar.gz.
  local tgz="$dist/MicaSlicer_${version}_arm64.app.tar.gz"
  COPYFILE_DISABLE=1 tar -czf "$tgz" -C "$out" "Mica Slicer.app"

  say "macOS: disk image"
  local stage="$out/dmg"
  rm -rf "$stage"
  mkdir -p "$stage"
  cp -R "$app" "$stage/"
  ln -s /Applications "$stage/Applications"
  local dmg="$dist/MicaSlicer_${version}_arm64.dmg"
  # An explicit size: hdiutil's own estimate for -srcfolder is sometimes too small ("No
  # space left on device").
  local mb
  mb="$(du -sm "$stage" | cut -f1)"
  hdiutil create -quiet -volname "Mica Slicer" -srcfolder "$stage" -size "$((mb * 13 / 10 + 20))m" \
    -ov -format UDZO "$dmg"
  rm -rf "$stage"
  if [ $dry_run = 0 ]; then
    codesign --force --timestamp --sign "$MICA_SIGN_IDENTITY" "$dmg"
    xcrun notarytool submit "$dmg" --keychain-profile "${MICA_NOTARY_PROFILE:-mica-notary}" --wait
    xcrun stapler staple "$dmg"
  fi
  sign_update "$tgz"
  add_package macos-aarch64 "$tgz" app
  artifacts+=("$dmg")
}

# ---------------------------------------------------------------- Windows
build_windows() {
  say "Windows: x86_64 binary (cargo-xwin)"
  if ! command -v cargo-xwin >/dev/null 2>&1; then skip "Windows: cargo-xwin is not installed"; return; fi
  if ! command -v makensis >/dev/null 2>&1; then skip "Windows: makensis is not installed (brew install makensis)"; return; fi
  # cargo-xwin needs clang-cl, llvm-lib and lld-link: Homebrew's LLVM (keg-only).
  local d
  for d in /opt/homebrew/opt/llvm/bin /opt/homebrew/opt/llvm@*/bin /opt/homebrew/opt/lld/bin /opt/homebrew/opt/lld@*/bin; do
    [ -d "$d" ] && PATH="$PATH:$d"
  done
  if ! command -v clang-cl >/dev/null 2>&1 || ! command -v lld-link >/dev/null 2>&1; then
    skip "Windows: clang-cl / lld-link not found (brew install llvm lld)"; return
  fi
  rustup target add x86_64-pc-windows-msvc >/dev/null
  cargo xwin build --profile "$profile" --target x86_64-pc-windows-msvc -p mica-gui --bin mica-gui
  local bin="$root/target/x86_64-pc-windows-msvc/$pdir"
  check_no_openh264 "$bin/mica-gui.exe"
  say "Windows: NSIS installer"
  local out="$dist/windows"
  rm -rf "$out" "$dist"/MicaSlicer_*-setup.exe
  (cd crates/mica-gui && env -u CARGO_PACKAGER_SIGN_PRIVATE_KEY cargo packager --formats nsis --target x86_64-pc-windows-msvc \
    --profile "$profile" --out-dir "$out")
  local made
  made="$(ls "$out"/*-setup.exe 2>/dev/null | head -1)"
  [ -n "$made" ] || die "cargo-packager made no installer"
  local exe="$dist/MicaSlicer_${version}_x64-setup.exe"
  cp "$made" "$exe"
  if [ $dry_run = 0 ] && [ -n "${MICA_WINDOWS_SIGN_COMMAND:-}" ]; then
    say "Windows: signing ($MICA_WINDOWS_SIGN_COMMAND)"
    $MICA_WINDOWS_SIGN_COMMAND "$exe"
  else
    skip "Windows: Authenticode signing (no MICA_WINDOWS_SIGN_COMMAND, or dry run)"
  fi
  sign_update "$exe"
  add_package windows-x86_64 "$exe" nsis
}

# ---------------------------------------------------------------- Linux
build_linux() {
  # Built in an Ubuntu 22.04 container (scripts/linux-appimage.sh): mica-videodec links the
  # system's GStreamer, so the link needs the target's GStreamer libraries.
  say "Linux: x86_64 binary and AppImage (Ubuntu 22.04 container, glibc 2.35)"
  if ! command -v docker >/dev/null 2>&1 || ! docker info >/dev/null 2>&1; then
    skip "Linux: Docker is not available (needed to build against GStreamer)"; return
  fi
  local out="$dist/linux"
  rm -rf "$out" "$dist"/MicaSlicer_*.AppImage
  mkdir -p "$out"
  local rust
  rust="$(rustc --version | cut -d' ' -f2)"
  # Toolchain, registry and appimagetool are cached in Docker volumes between runs.
  docker run --rm --platform linux/amd64 \
    -v "$root:/src" -v mica-release-cargo:/opt/cargo -v mica-release-rustup:/opt/rustup \
    -v mica-release-tools:/opt/cache \
    -e CARGO_HOME=/opt/cargo -e RUSTUP_HOME=/opt/rustup -e MICA_PACKAGED=1 \
    -e MICA_UPDATE_PUBKEY \
    ubuntu:22.04 sh /src/scripts/linux-appimage.sh "$profile" "$version" \
    "/src/dist/$version/linux" "$rust"
  local image="MicaSlicer_${version}_x86_64.AppImage"
  [ -f "$out/$image" ] || die "no AppImage was made"
  mv "$out/$image" "$dist/$image"
  sign_update "$dist/$image"
  add_package linux-x86_64 "$dist/$image" appimage
}

# Packages an earlier run left in dist/<version>/.
use_prebuilt() {  # platform file format [extra asset]
  local file="$dist/$2"
  if [ ! -f "$file" ]; then skip "$1: no $2 in $dist"; return; fi
  if [ -f "$file.sig" ]; then update_keys "$file.sig"; fi
  add_package "$1" "$file" "$3"
  if [ -n "${4:-}" ]; then
    [ -f "$dist/$4" ] && artifacts+=("$dist/$4") || skip "$1: no $4 in $dist"
  fi
}

if [ $prebuilt = 1 ]; then
  say "Using the packages in $dist"
  want macos && use_prebuilt macos-aarch64 "MicaSlicer_${version}_arm64.app.tar.gz" app "MicaSlicer_${version}_arm64.dmg"
  want windows && use_prebuilt windows-x86_64 "MicaSlicer_${version}_x64-setup.exe" nsis
  want linux && use_prebuilt linux-x86_64 "MicaSlicer_${version}_x86_64.AppImage" appimage
else
  want macos && { if [ "$(uname)" = Darwin ]; then build_macos; else skip "macOS: needs a Mac"; fi; }
  want windows && build_windows
  want linux && build_linux
fi
[ ${#manifest_entries[@]} -gt 0 ] || die "no packages to release"

# ---------------------------------------------------------------- latest.json
say "Update manifest (packages on GitHub release v$version of $repo)"
# The GitHub release text: the CHANGELOG.md section, as the manifest and the app have it.
notes_out="$dist/release-notes.md"
printf '%s\n' "$notes" >"$notes_out"
extra=()
if [ $have_notes = 1 ]; then
  extra+=(--changelog "$changelog")
else
  extra+=(--notes-file "$notes_out")
fi
[ $dry_run = 1 ] && extra+=(--allow-unsigned)
if [ -n "${MICA_RELEASE_ASSET_BASE:-}" ]; then
  [ $publish = 0 ] || die "MICA_RELEASE_ASSET_BASE is for local tests; a published manifest points at GitHub"
  extra+=(--asset-base "$MICA_RELEASE_ASSET_BASE")
fi
python3 -I "$root/scripts/update-manifest.py" --version "$version" --repo "$repo" --dir "$dist" \
  --out "$dist/latest.json" ${extra[@]+"${extra[@]}"} \
  "${manifest_entries[@]}" || die "could not write the update manifest"

# ---------------------------------------------------------------- publishing
q() { printf '%q ' "$@"; }
# A pre-release: Mica is alpha software (the website's download link points at /releases).
gh_cmd=(gh release create "v$version" --repo "github.com/$repo" --title "Mica Slicer $version" \
  --prerelease --notes-file "$notes_out" "${artifacts[@]}")
site_manifest="$site_dir/$manifest_path"
install_cmd="mkdir -p $(q "$(dirname "$site_manifest")")&& cp $(q "$dist/latest.json" "$site_manifest")"
deploy_cmd="cd $(q "$website_root")&& CLOUDFLARE_ACCOUNT_ID=<account id> npx wrangler deploy"

say "Done: $dist"
ls -lh "$dist" | sed -n '2,$p'
if [ -n "$skipped" ]; then printf '\nSkipped:%b\n' "$skipped"; fi

if [ $dry_run = 1 ]; then
  say "Dry run: these packages are not publishable. A real release would run, in this order:"
  printf '  1. %s\n  2. %s\n  3. %s\n' "$(q "${gh_cmd[@]}")" "$install_cmd" "$deploy_cmd"
  exit 0
fi

if [ $publish = 0 ]; then
  say "Nothing was published. To publish, in this order:"
  printf '  1. GitHub release (or: scripts/release.sh --prebuilt --publish, which runs 1 and 2):\n     %s\n' "$(q "${gh_cmd[@]}")"
  printf '  2. Manifest into the website repository:\n     %s\n' "$install_cmd"
  printf '  3. Commit it in the website repository and deploy from there, only after 1 succeeded:\n     %s\n' "$deploy_cmd"
  exit 0
fi

need gh "brew install gh, then gh auth login"
# Checked before anything is published, so a missing checkout cannot strand a release.
[ -d "$site_dir" ] || die "the website repository's site/ folder is not at $site_dir; set MICA_WEBSITE_DIR to your mica-website checkout (or MICA_RELEASE_SITE to its site/ folder)"
say "Creating GitHub pre-release v$version on $repo"
"${gh_cmd[@]}" || die "gh release create failed; the site manifest was not changed"
mkdir -p "$(dirname "$site_manifest")"
cp "$dist/latest.json" "$site_manifest"
say "Manifest written to $site_manifest"
printf 'Next, in this order:\n'
printf '  1. Check the release page: https://github.com/%s/releases/tag/v%s\n' "$repo" "$version"
printf '  2. Commit it in the website repo and deploy from there:\n     %s\n' "$deploy_cmd"
printf '  3. Check: curl -fsS %s/%s\n' "$site_url" "$manifest_path"
