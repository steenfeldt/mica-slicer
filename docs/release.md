# Releasing tester builds

`scripts/release.sh` builds Mica Slicer for macOS (Apple Silicon only, arm64, macOS 14 or later; `.app` in a
`.dmg`), Windows (NSIS installer, cross-built from the Mac with `cargo-xwin`) and Linux
(AppImage, built in an Ubuntu 22.04 Docker container), signs everything and writes the update
manifest. Publishing has two parts: the packages go to a **GitHub pre-release** of
`github.com/steenfeldt/mica-slicer` (Mica is alpha, pre-release software), and the manifest is a static file of the **website**
(`site/updates/alpha/latest.json` in the website repository, served at
`https://micaslicer.com/updates/alpha/latest.json`), where the in-app updater finds it.

The website is a separate repository. The release script finds its checkout at
`$MICA_WEBSITE_DIR` (default `../mica-website`, next to this repository) and writes the
manifest into its `site/` folder (`MICA_RELEASE_SITE` overrides that folder directly);
`--publish` stops before publishing anything when the checkout is missing.

```sh
scripts/release.sh --dry-run                       # everything except signing/notarizing; never publishable
scripts/release.sh --version 0.1.0                 # build + sign; prints the publishing commands
scripts/release.sh --prebuilt --publish            # then: GitHub pre-release + manifest into the website repo
scripts/release.sh --dry-run --platforms macos --profile fast   # quick local package
```

Without `--publish` the script never publishes anything: it prints the exact commands instead.

## Release notes: CHANGELOG.md

The release notes are written once, in [`CHANGELOG.md`](../CHANGELOG.md), as a `## X.Y.Z`
section (`###` headings, `-` bullets, `**bold**`, `[links](https://…)`). That one section is:

- the **What's new** window the app shows once after updating to that version (and from
  Help › What's new…): the app bundles `CHANGELOG.md` at build time
  (`crates/mica-gui/src/panels/whats_new.rs`);
- the GitHub release text (`dist/X.Y.Z/release-notes.md`);
- the `notes` of the update manifest (`scripts/update-manifest.py --changelog`), which the
  update notification's *What's new* shows before installing.

`scripts/release.sh` stops before building when `CHANGELOG.md` has no section for the version
(a `--dry-run` uses a one-line placeholder instead). Commit the section with the version bump:
the app compiles the file in, so notes added after the build are not in it.

Output goes to `dist/<version>/` (ignored by git):

| File | What |
|---|---|
| `MicaSlicer_<v>_arm64.dmg` | macOS download for testers |
| `MicaSlicer_<v>_arm64.app.tar.gz` | macOS update package (the updater installs this) |
| `MicaSlicer_<v>_x64-setup.exe` | Windows installer and update package |
| `MicaSlicer_<v>_x86_64.AppImage` | Linux download and update package |
| `*.sig` | the update packages' minisign signatures (base64), also uploaded to the release |
| `latest.json` | the update manifest (`scripts/update-manifest.py`): per platform the GitHub asset URL, signature, format, size and SHA-256 |
| `release-notes.md` | the GitHub release text: the version's `CHANGELOG.md` section |

Every package URL in the manifest is
`https://github.com/steenfeldt/mica-slicer/releases/download/v<version>/<file>`; GitHub
answers it with a redirect to its download host, which the updater follows (same size limits
and time budget).

## Publishing a release

Order matters: **the GitHub release first, then the website deploy**, so the live manifest never
names a file that is not there yet.

1. Write the release notes in `CHANGELOG.md` (`## X.Y.Z`), commit the release state (version
   bump, notes, `update-key.pub`), and push the public tree for
   this version to `github.com/steenfeldt/mica-slicer`: `gh release create` tags the public
   repository's default branch.
2. Build and sign: `scripts/release.sh --version X.Y.Z` (secrets: see the
   one-time setup below). Check the packages in `dist/X.Y.Z/`, e.g. install the `.dmg` on a
   test Mac.
3. Publish the packages: `scripts/release.sh --prebuilt --publish`. It runs
   `gh release create vX.Y.Z --repo github.com/steenfeldt/mica-slicer --title "Mica Slicer X.Y.Z"
   --prerelease --notes-file dist/X.Y.Z/release-notes.md <packages, .sig files, .dmg>` (gh's own login;
   `gh auth login` once), and only when that succeeds copies `dist/X.Y.Z/latest.json` to
   the website repository's `site/updates/alpha/latest.json`. Without `--publish` it prints
   the same commands to run by hand.
4. Check the release page (every asset present), then commit
   `site/updates/alpha/latest.json` in the website repository.
5. Deploy the website from the website repository (the maintainer's step, never automatic):
   ```sh
   cd ../mica-website && CLOUDFLARE_ACCOUNT_ID=<account id> npx wrangler deploy
   ```
6. Check: `curl -fsS https://micaslicer.com/updates/alpha/latest.json` shows the new version;
   a tester build offers it with Help › Check for Updates….

Every release is marked as a **pre-release** on GitHub (`--prerelease`): Mica is alpha
(pre-release) software. GitHub's `/releases/latest` never points at a pre-release, so the
website's download section links to `github.com/steenfeldt/mica-slicer/releases`, where the
newest pre-release is listed first. The in-app updater does not depend on this: it reads the
manifest on the website, which names the release's assets directly.

Until the first release, the website repository's `site/updates/alpha/latest.json` is a
placeholder (version `0.0.0`, no packages), which the updater treats as "no update". A missing manifest (HTTP 404) is also
"no update", never an error notification.

Later, optionally: run the build and `gh release create` from a GitHub Actions workflow in a
protected `release` environment (required reviewers, the signing key as an environment
secret), so no release key lives on a laptop. Not set up yet.

## Before every push

Check that nothing personal is committed: home-folder paths, local network addresses,
secrets, private test models (including names inside 3MF metadata) and stray files.

## The update server: one setting

`crates/mica-update/src/lib.rs`: `UPDATE_BASE_URL = "https://micaslicer.com/updates"` and
`CHANNEL = "alpha"`. The app reads `<base>/<channel>/latest.json`; the release script reads the
same two constants (and writes the manifest to the matching path under the website
repository's `site/`), so they are
the only place to change. The GitHub repository the packages go to is the workspace
`repository` in `Cargo.toml`. The updater follows redirects.
`MICA_UPDATE_URL=<manifest url>` overrides the manifest URL at run time (a staging server,
tests); signatures are still checked against the trusted keys compiled into the app.

## Identity (decide before the first tester build)

- **Bundle identifier: `com.micaslicer.app`** (`[package.metadata.packager]` in
  `crates/mica-gui/Cargo.toml`), on the micaslicer.com domain. Never change it after testers
  installed a build: macOS would see a different app, and updates, Keychain items (printer
  passwords are stored per app) and file associations would not carry over.
- **Update signing keys** (step 3): every build trusts a small list of public keys: the main
  key first, followed by offline backups. Any one can sign an update. Losing all trusted
  private keys means testers must reinstall by hand.

## Vendor profiles in the packages

`scripts/release.sh` first runs `cargo xtask bundle-profiles target/bundled-profiles`: the
OrcaSlicer vendors in `SHIPPED_ORCA_VENDORS` (`crates/mica-profiles/src/vendor_profiles.rs`, the
one place to change the list), everything they inherit from (`OrcaFilamentLibrary`), their bed
models, textures and covers, OrcaSlicer's LICENSE.txt and a NOTICE, copied from the pinned
`upstream/orca` (needs `scripts/fetch-upstream.sh orca`; refused when the checkout is not at the
pinned commit). cargo-packager puts it into the app (`resources` in `crates/mica-gui/Cargo.toml`:
macOS `Contents/Resources/profiles/orca`, Windows `<install dir>\profiles\orca`), the AppImage
script into `usr/share/mica-slicer/profiles/orca`; the macOS build stops if it is missing. About
24 MB (7,800 files, ~8.7 MB compressed).

Prusa profiles are never packaged (PROVENANCE.md): the app downloads the pinned 2.5.12 bundle from
Prusa's profile repository on first use and checks its SHA-256 (`mica profiles fetch` on the
command line). Moving the Prusa pin means updating `prusa::VERSION`, `BUNDLE_SHA256` and
`vendor_profiles/prusa_assets.tsv` together. Check a package without network: `MICA_CONFIG_DIR=$(mktemp -d)
"…/Mica Slicer.app/Contents/MacOS/mica-gui" --list-printers` lists the shipped printers and says
the Prusa profiles still need downloading.

## H.264 and OpenH264

Packaged builds contain **no OpenH264 code** (H.264 patent licensing covers only Cisco's own
prebuilt binaries, not builds from source). H.264 camera video is decoded by the operating
system (`crates/mica-videodec`: VideoToolbox on macOS, Media Foundation on Windows, the
distribution's GStreamer on Linux). OpenH264 is only a development alternative behind the
cargo feature `openh264` (`mica-videodec` / `mica-camera`, forwarded by `mica-gui`), **off by
default**; `cargo run -p mica-gui --features openh264` turns it on. The release script never
passes `--features` and fails if a built binary contains OpenH264 symbols.

## Linux: GStreamer and the AppImage

`mica-videodec` links GStreamer core and gst-plugins-base (app, video) dynamically. So:

- The Linux binary is built in an **Ubuntu 22.04 container** (`scripts/linux-appimage.sh`,
  started by the release script) against Ubuntu's GStreamer 1.20 rather than cross-built
  with cargo-zigbuild, which has no target GStreamer to link. glibc baseline 2.35: Ubuntu
  22.04+, Debian 12+, Fedora 36+. (Debian 11 is end of life; its packages are gone from the
  mirrors.)
- The AppImage **bundles GStreamer core + base and their dependencies** (glib, gobject, orc,
  …; no codec plugin) in `usr/lib/fallback`, but `AppRun` uses them **only when the system
  has no GStreamer**. With the system's GStreamer installed, Mica uses it, so the
  distribution's plugins (its H.264 decoder: libav, VA-API, …) load against the core they
  were built for; bundling the core unconditionally would break those plugins on newer
  distributions. Without GStreamer, Mica still starts and an H.264 camera says it needs
  GStreamer with an H.264 decoder; MJPEG and snapshot cameras work either way.
- Cleaner long-term alternative: load GStreamer at run time (dlopen) in `mica-videodec`;
  then the AppImage would need no fallback libraries.

## One-time setup (maintainer)

None of these secrets ever goes into the repository; the script reads them at run time.

### 1. Developer ID certificate (macOS signing)

1. Apple Developer account → Certificates → **+** → *Developer ID Application* (G2 Sub-CA).
   Create the CSR with Keychain Access → Certificate Assistant → *Request a Certificate From
   a Certificate Authority* (saved to disk), upload, download the `.cer`, double-click it so
   it lands in the **login** keychain next to its private key.
2. Check: `security find-identity -v -p codesigning` lists
   `Developer ID Application: <Your Name> (TEAMID)`.
3. Before releasing: `export MICA_SIGN_IDENTITY="Developer ID Application: <Your Name> (TEAMID)"`.

The script signs with the hardened runtime (`--options runtime`), a secure timestamp and
`crates/mica-gui/packaging/macos/entitlements.plist` (no sandbox; see the comments there).

### 2. Notarization credentials

1. appleid.apple.com → Sign-In and Security → App-Specific Passwords → generate one
   ("mica-notary").
2. Store it in the Keychain as a notarytool profile (the script never sees the password):
   ```sh
   xcrun notarytool store-credentials mica-notary \
     --apple-id <your Apple ID> --team-id <TEAMID> --password <app-specific password>
   ```
3. The script uses the profile `mica-notary` (override with `MICA_NOTARY_PROFILE`). It
   notarizes and staples both the `.app` (before packing the update `.tar.gz`) and the `.dmg`.

### 3. Update signing keys (minisign / Ed25519)

1. Generate **two** key pairs with cargo-packager's signer
   (`cargo install --locked cargo-packager@0.11.8`): a main key for routine releases and a
   separate backup for emergencies. Create them in a temporary directory:
   ```sh
   t=$(mktemp -d)
   chmod 700 "$t"
   cargo packager signer generate --path "$t/mica-update-main.key"
   cargo packager signer generate --path "$t/mica-update-backup.key"
   ```
   Choose strong passwords. Each command writes a private `.key` file and a public
   `.key.pub` file (both base64 single lines).
2. Put **both public lines** below the comments in `crates/mica-update/update-key.pub`:
   `mica-update-main.key.pub` first, then `mica-update-backup.key.pub`. Commit the public
   list. Every build trusts any key on its embedded list. Never put private keys in this file
   or anywhere in the repository.
3. Store the backup private key and its password in a separate **password-manager vault
   item** (a document or encrypted note), reserved for offline recovery; do not leave a copy
   on disk after creation. Store a recovery copy of the main key in the password manager too. Move only
   the main private key to the release machine's protected key directory, then delete the
   temporary directory after confirming the vault items and public lines:
   ```sh
   mkdir -p ~/.mica-release && chmod 700 ~/.mica-release
   mv "$t/mica-update-main.key" ~/.mica-release/mica-update.key
   chmod 600 ~/.mica-release/mica-update.key
   rm -r "$t"
   ```
   Store the main key's password in the Keychain so the script can read it at run time:
   ```sh
   security add-generic-password -a "$USER" -s mica-update-key -w
   ```
4. Before releasing: `export CARGO_PACKAGER_SIGN_PRIVATE_KEY=~/.mica-release/mica-update.key`
   (the password comes from the Keychain item, or `CARGO_PACKAGER_SIGN_PRIVATE_KEY_PASSWORD`).
   The script signs with whichever private key this variable names. After signing it reports
   the matching trusted public key's short SHA-256 fingerprint, using the minisign key ID
   from the signature as a diagnostic.

For a test build, `MICA_UPDATE_PUBKEY` at **build time** overrides the entire public list;
separate keys with newlines or commas. An unset or whitespace-only override uses the file.
Blank lines and `#` comment lines are ignored. The Linux container receives this override
too. An empty list cannot verify updates, and a real release refuses to build with it.

**Lost or leaked main key:** create a new main key pair in a temporary directory and store
it as above. Replace the old main public line with the new main public line, keeping the
backup line. Temporarily restore the backup private key from its password-manager item into a
protected temporary directory, point `CARGO_PACKAGER_SIGN_PRIVATE_KEY` at it, and provide
its password with `CARGO_PACKAGER_SIGN_PRIVATE_KEY_PASSWORD`. Sign and publish one release
that embeds the new main plus the backup public key. Existing builds accept that release
because they already trust the backup. Remove the temporary backup copy afterward and
switch routine signing to the new main key. No special rotation protocol is needed.

Revoking a key means removing its public line from the list in that release; builds that
install it stop trusting the old key. Older builds still trust their embedded list until
they update. If neither trusted private key remains available, recovery requires a manual
reinstall.

### 4. GitHub CLI and the website

1. GitHub CLI: `brew install gh`, then `gh auth login` with an account that may create
   releases on `github.com/steenfeldt/mica-slicer`. Only `--publish` calls it.
2. The website is a Cloudflare Worker in its own repository (see the website repository's
   README and docs); check it out next to this one as `../mica-website`, or set
   `MICA_WEBSITE_DIR`. Deploying it needs Node's `npx` and a Cloudflare login
   (`npx wrangler login`) or an API token that may deploy Workers. The release script never
   deploys; it prints the command, to run in the website repository.
3. The website must serve `/updates/*` with `Cache-Control: no-cache` (its `site/_headers`),
   so a new manifest is visible at once after a deploy. That rule lives and is checked in the
   website repository; this repository no longer tests it.

### 5. Build tools (once per Mac)

```sh
cargo install --locked cargo-packager@0.11.8 cargo-xwin
brew install makensis llvm lld
rustup target add aarch64-apple-darwin x86_64-pc-windows-msvc
```
Docker (OrbStack) is needed for Linux (binary and AppImage are built in a container; the
toolchain, crates and appimagetool are cached in Docker volumes `mica-release-*`); without
it the script skips Linux with a message. `cargo-xwin` needs Homebrew's `clang-cl`,
`llvm-lib` and `lld-link` (keg-only `llvm`/`lld`; the script finds them) and downloads the
Microsoft CRT and Windows SDK headers on first use (accepting Microsoft's license for them).

### 6. Windows code signing (optional hook)

Unsigned installers trigger SmartScreen warnings. When a certificate exists (e.g. Azure
Trusted Signing or an OV/EV certificate), set `MICA_WINDOWS_SIGN_COMMAND` to a command that
signs the file passed as its last argument (for example
`MICA_WINDOWS_SIGN_COMMAND="jsign --storetype TRUSTEDSIGNING ... "`). It runs before the
update signature is made.

## The in-app updater

- **Check for Updates…** sits in the Mica application menu on macOS and in **Help** on
  Windows/Linux (one menu description, `crates/mica-gui/src/menu/mod.rs`).
- A quiet check runs at start at most once a day (Settings › Files › *Check for
  Mica updates*, default on).
- A newer version shows a notification "Mica Slicer 0.x is available" with *What's new*
  (the version's `CHANGELOG.md` section), *Install* and *Later*. Install downloads with progress, verifies the
  minisign signature against any embedded trusted public key, then installs: macOS swaps the `.app`
  bundle (administrator prompt only if its folder is not writable), Linux replaces the
  AppImage, Windows saves the installer and runs it passively once Mica quits. *Restart now*
  appears once no slice, upload or watched print is running, and goes through the normal
  close path (unsaved changes are asked about).
- Every failure is a notification (a quiet check that fails is a warning with a link to
  Settings › Files); nothing is silent.
- Version comparison is semver: `0.2.0-beta.1` < `0.2.0` < `0.2.1`.
- A development build (any binary under cargo's `target/` folder) never replaces itself; set
  `MICA_UPDATE_FORCE=1` to try the updater from one. Headless runs (tests, screenshots, the
  monkey) never go online. A build without a key in `update-key.pub` says it cannot verify
  updates when asked.
- Not updatable in place: an app run from the mounted `.dmg` or a translocated location (the
  message asks to move it to Applications), a bare Linux binary (not an AppImage).

The manifest and signature format is cargo-packager-updater's (`latest.json` with a
`platforms` map, base64 minisign signatures), so cargo-packager's signer and docs apply. The
client itself is Mica's (`crates/mica-update`, on `ureq` and `minisign-verify`) instead of
the `cargo-packager-updater` crate, because that crate pulls in a second HTTP stack (reqwest,
hyper, tokio, quinn, ~60 crates), ends the process with `std::process::exit` during a Windows
install (no chance to save), panics if the installer cannot start, and on macOS would
replace the executable's parent folder when not run from an `.app` bundle.

## Testing the updater locally

`cargo test -p mica-update` runs the end-to-end test on loopback: throwaway keys made in
memory, a manifest and package behind redirects (including GitHub's redirect to another host,
with the size limits and time limits still applied), check, download, verify, and refusal of a
tampered package, a package signed with an unrelated third key, a garbled or swapped signature,
acceptance of both main and backup signatures, empty-list refusal, key rotation, "no update"
for a 404 and for the committed placeholder manifest, plus a real `.app` swap in a temporary
folder. It also runs `scripts/update-manifest.py` and `scripts/release.sh --prebuilt` (with and
without `--publish`) on fake packages signed with a throwaway key in a temporary folder, with
`gh` and `npx` replaced by logging stubs (no network), and verifies the manifests they write.

A full app-level test with a packaged build (never the real release key):

```sh
t=$(mktemp -d)
cargo packager signer generate --ci --password "" --path "$t/test.key"
export MICA_UPDATE_PUBKEY="$(cat "$t/test.key.pub")"     # compiled into this test build only
CARGO_PACKAGER_SIGN_PRIVATE_KEY="$t/test.key" CARGO_PACKAGER_SIGN_PRIVATE_KEY_PASSWORD="" \
MICA_RELEASE_ASSET_BASE=http://127.0.0.1:8000 \
  scripts/release.sh --dry-run --platforms macos --profile fast
# python3 -m http.server -d dist/<v> -b 127.0.0.1 8000, set
# MICA_UPDATE_URL=http://127.0.0.1:8000/latest.json, open the app copied out of the .dmg with
# a temporary MICA_CONFIG_DIR, and Check for Updates.
```

## Known gaps

- macOS file associations (`.3mf`, `.stl`) are registered, but a double-click in Finder only
  opens Mica: the file arrives as an Apple Event, which winit does not pass on yet. Windows
  and Linux pass the file on the command line, which Mica opens.
- The Windows `.exe` has no embedded icon resource yet: the window and taskbar icon are set
  at run time, but Explorer and the Start menu shortcut show a generic icon (needs a
  resource step, e.g. `winresource` with `llvm-rc`, in the cross build).
- Windows: cargo-packager's NSIS template closes a running Mica before installing (its
  `/P` passive mode); the updater runs the installer only after Mica has quit.
