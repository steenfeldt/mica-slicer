#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Provenance: original
"""Writes the in-app updater's manifest (latest.json) for packages published as GitHub
release assets. Called by scripts/release.sh; see docs/release.md.

  update-manifest.py --version 0.1.0 --repo owner/name --dir dist/0.1.0 --out FILE
                     [--changelog CHANGELOG.md | --notes-file FILE]
                     [--pub-date 2026-10-21T12:00:00Z] [--allow-unsigned] [--asset-base URL]
                     PLATFORM=FILE:FORMAT ...
  update-manifest.py --notes-only --changelog CHANGELOG.md --version 0.1.0

The release notes come from CHANGELOG.md, the one place they are written: --changelog takes
the `## <version>` section of that file (the app shows the same section as "What's new";
crates/mica-gui/src/panels/whats_new.rs reads it with the same rules). --notes-only prints that
section and exits (scripts/release.sh writes it as the GitHub release text). A missing or
empty section is an error. --notes-file FILE takes the notes from FILE as they are instead.

Each PLATFORM=FILE:FORMAT names an update package in --dir (e.g.
macos-aarch64=MicaSlicer_0.1.0_arm64.app.tar.gz:app). Its signature is read from FILE.sig next
to it (the base64 minisign signature `cargo packager signer sign` writes). The package URL is
https://github.com/<repo>/releases/download/v<version>/<FILE>, so FILE must be the asset's
exact name on the release. The manifest also records each package's size and SHA-256.
--allow-unsigned (dry runs only) writes UNSIGNED-DRY-RUN for a missing signature.
--asset-base URL replaces the GitHub download prefix (local updater tests only).
Standard library only; never contacts the network.
"""

import argparse
import datetime
import hashlib
import json
import os
import re
import sys

FORMATS = {"app", "appimage", "nsis"}
PLATFORM = re.compile(r"^(macos|windows|linux)-(aarch64|x86_64)$")
# Names GitHub keeps unchanged as asset names (it rewrites spaces and most punctuation).
SAFE_NAME = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]*$")
VERSION = re.compile(r"^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$")
REPO = re.compile(r"^[A-Za-z0-9-]+/[A-Za-z0-9._-]+$")
UNSIGNED = "UNSIGNED-DRY-RUN"


def fail(message):
    print(f"update-manifest: {message}", file=sys.stderr)
    sys.exit(1)


def digest(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()


def changelog_notes(path, version):
    """The body of the `## <version>` section of a changelog (a leading `v` and text after
    the version on the heading line are allowed), trimmed; None when missing or empty."""
    with open(path, encoding="utf-8") as f:
        lines = f.read().splitlines()
    body = None
    for line in lines:
        if line.startswith("## "):
            if body is not None:
                break
            words = line[3:].split()
            if words and words[0].removeprefix("v") == version:
                body = []
            continue
        if body is not None:
            body.append(line)
    text = "\n".join(body or []).strip()
    return text or None


def main():
    p = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    p.add_argument("--version", required=True)
    p.add_argument("--repo")
    p.add_argument("--dir")
    p.add_argument("--out")
    p.add_argument("--notes-file")
    p.add_argument("--changelog")
    p.add_argument("--notes-only", action="store_true")
    p.add_argument("--pub-date")
    p.add_argument("--allow-unsigned", action="store_true")
    p.add_argument("--asset-base")
    p.add_argument("entries", nargs="*")
    a = p.parse_args()

    if not VERSION.match(a.version):
        fail(f"not a release version: {a.version!r}")
    if a.changelog and a.notes_file:
        fail("--changelog and --notes-file both name the notes; give one")
    changelog = None
    if a.changelog:
        changelog = changelog_notes(a.changelog, a.version)
        if changelog is None:
            fail(
                f"{a.changelog} has no release notes for {a.version}: add a "
                f"'## {a.version}' section with what changed"
            )
    if a.notes_only:
        if changelog is None:
            fail("--notes-only needs --changelog")
        # UTF-8 bytes on every OS: a Windows console or pipe would otherwise encode the
        # notes in the locale code page (cp1252), turning e.g. "…" into a byte the GUI's copy
        # (whats_new.rs) does not have.
        sys.stdout.buffer.write((changelog + "\n").encode("utf-8"))
        return
    for name in ("repo", "dir", "out"):
        if not getattr(a, name):
            fail(f"--{name} is required")
    if not REPO.match(a.repo):
        fail(f"not a GitHub owner/name: {a.repo!r}")
    if not a.entries:
        fail("no update packages given")
    base = f"https://github.com/{a.repo}/releases/download/v{a.version}"
    if a.asset_base:
        if not re.match(r"^https?://[^\s\"]+$", a.asset_base):
            fail(f"not an http(s) URL: {a.asset_base!r}")
        base = a.asset_base.rstrip("/")
    platforms = {}
    for entry in a.entries:
        platform, sep, rest = entry.partition("=")
        name, sep2, fmt = rest.rpartition(":")
        if not sep or not sep2:
            fail(f"expected PLATFORM=FILE:FORMAT, got {entry!r}")
        if not PLATFORM.match(platform):
            fail(f"unknown platform {platform!r}")
        if fmt not in FORMATS:
            fail(f"unknown package format {fmt!r}")
        if platform in platforms:
            fail(f"{platform} is listed twice")
        if not SAFE_NAME.match(name):
            fail(f"{name!r} would be renamed by GitHub; use letters, digits, '.', '_', '-'")
        path = os.path.join(a.dir, name)
        if not os.path.isfile(path):
            fail(f"missing package {path}")
        sig_path = path + ".sig"
        if os.path.isfile(sig_path):
            with open(sig_path, encoding="utf-8") as f:
                signature = f.read().strip()
            if not signature:
                fail(f"empty signature {sig_path}")
        elif a.allow_unsigned:
            signature = UNSIGNED
        else:
            fail(f"missing signature {sig_path} (sign the package first)")
        platforms[platform] = {
            "url": f"{base}/{name}",
            "signature": signature,
            "format": fmt,
            "size": os.path.getsize(path),
            "sha256": digest(path),
        }

    notes = changelog or ""
    if a.notes_file:
        with open(a.notes_file, encoding="utf-8") as f:
            notes = f.read().strip()
    pub_date = a.pub_date or datetime.datetime.now(datetime.timezone.utc).strftime(
        "%Y-%m-%dT%H:%M:%SZ"
    )
    manifest = {
        "version": a.version,
        "notes": notes,
        "pub_date": pub_date,
        "platforms": dict(sorted(platforms.items())),
    }
    text = json.dumps(manifest, indent=2) + "\n"
    tmp = a.out + ".tmp"
    with open(tmp, "w", encoding="utf-8") as f:
        f.write(text)
    os.replace(tmp, a.out)
    sys.stdout.write(text)


if __name__ == "__main__":
    main()
