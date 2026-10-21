# Mica Slicer

A 3D-printing slicer written in Rust. It slices with the same rules as PrusaSlicer and
OrcaSlicer, so your existing printer, print and filament profiles give the prints you know.

Mica works on one layer at a time and streams its output, so memory use stays low even for
large prints. It runs on macOS, Linux and Windows.

## Status: Alpha (pre-release)

Mica is alpha (pre-release) software. It is tested against PrusaSlicer and OrcaSlicer and on
real printers, but it can still produce G-code that fails a print or damages a printer:

- Check the preview before printing, and watch the first layers.
- Never leave a printer unattended.
- Mica is provided as is, without warranty of any kind. See the [license](LICENSE)
  (GNU AGPL-3.0, sections 15 and 16: disclaimer of warranty and limitation of liability).

Mica slices STL and 3MF files (models and PrusaSlicer/OrcaSlicer projects) from the command
line (`mica slice`) and from the desktop app. The CLI and the app write the same G-code byte
for byte.

Tested printers, with output checked against the reference slicers:

| Printer | Firmware | Sliced like |
|---|---|---|
| Prusa CORE One | Prusa (Marlin 2), binary G-code | PrusaSlicer 2.9.6 |
| Prusa XL (5 tools) | Prusa (Marlin 2), binary G-code | PrusaSlicer 2.9.6 |
| Prusa MK3.5 | Prusa (Marlin 2) | PrusaSlicer 2.9.6 |
| Prusa MINI | Prusa (Marlin 2) | PrusaSlicer 2.9.6 |
| Snapmaker U1 (4 tools) | Klipper | OrcaSlicer (Snapmaker's fork) |
| RatRig V-Core 3.2 | Klipper | OrcaSlicer |

Other printers from the PrusaSlicer and OrcaSlicer profile libraries load and slice too, but
have not been checked print by print. Which slicer's rules a printer follows is described in
[`docs/slicing-rules.md`](docs/slicing-rules.md).

Not supported yet:

- **Spiral vase.** A profile or project with it on slices as a normal print, with a warning.
- **Absolute extrusion (M82).** Turn on *Use relative E distances* in Printer settings;
  Mica refuses the slice otherwise.
- **MMU** (single-extruder multi-material, e.g. the Prusa MMU3). Printers with one nozzle
  per tool (XL, U1) are supported.
- **Post-processing scripts.** They are kept in the profile but not run; the slice says so.
- G-code flavors other than Marlin 2 and Klipper.

Settings Mica does not apply yet are hidden in the app's settings editor, and a profile that
uses one gets a notice instead of a silent difference.

Tester packages (macOS `.dmg`, Windows installer, Linux AppImage) and the in-app updater:
[`docs/release.md`](docs/release.md).

## Building

```sh
cargo build --workspace
cargo test --workspace
cargo run -p mica-cli -- slice --help
cargo run -p mica-gui --features openh264   # optional: OpenH264 for RTSP cameras (development only)
```

Slicing needs the printer profiles. The app ships OrcaSlicer's profiles for Snapmaker, RatRig,
Bambu Lab, Creality, Voron, Elegoo, Anycubic, QIDI and Sovol, plus its Prusa profiles for Orca rules. **Prusa printer profiles are
downloaded from Prusa on first use** (once, checked by SHA-256; `mica profiles fetch` on the
command line), since Mica may not redistribute them. An installed PrusaSlicer or OrcaSlicer is
used as a fallback; development builds read the pinned profile checkouts
(`scripts/fetch-upstream.sh`, see below).

```sh
mica slice model.stl                                   # Prusa CORE One, 0.20mm SPEED, Prusament PLA
mica slice model.stl --printer 'Original Prusa XL - 5T Input Shaper 0.4 nozzle' \
  --print '0.20mm SPEED @XLIS 0.4' --filament 'Prusament PLA @XLIS'
mica slice project.3mf                                 # slices with the project's own settings
mica slice model.stl --vendor Snapmaker --printer 'Snapmaker U1 (0.4 nozzle)'
```

## Layout

| Path | What |
|---|---|
| `crates/mica-core` | Engine: mesh loading and repair, slicing, regions, walls, infill, supports, seams, toolpaths, 3MF projects |
| `crates/mica-gcode` | Output: text G-code (Prusa/Marlin 2 and Klipper), Prusa binary G-code, thumbnails, time estimates, G-code validation |
| `crates/mica-profiles` | Profiles: PrusaSlicer INI bundles and OrcaSlicer JSON vendors, inheritance, user profiles |
| `crates/mica-template` | The G-code template language of custom G-code (PrusaSlicer's placeholder syntax) |
| `crates/mica-cli` | The `mica` command (slice, calibrate, validate, profiles, print, MCP server) |
| `crates/mica-gui` | The desktop app |
| `crates/mica-printhost` | Printer connections (PrusaLink, Prusa Connect, Moonraker) |
| `crates/mica-camera`, `crates/mica-videodec` | Printer cameras and H.264 decoding with the system's decoder |
| `crates/mica-library` | The model library: a read-only index of your model folders |
| `crates/mica-update` | In-app updater: signed update manifests, download, verify, install |
| `crates/mica-testkit` | Test helpers: fixture paths, and skips that fail when `MICA_REQUIRE_FIXTURES=1` (CI) |
| `crates/clipper2`, `crates/boostvoronoi` | Vendored forks of the polygon clipping and Voronoi libraries |
| `xtask` | Repository tasks: `provenance`, `corpus` (reference prints), `parity` (CLI = app), `monkey` (random GUI testing) |
| `docs/` | Design notes per feature; start with [`docs/slicing-rules.md`](docs/slicing-rules.md) and [`docs/upstream-map.md`](docs/upstream-map.md) |
| `upstream/` | Pinned upstream slicers and profiles used as reference (fetched, not committed) |

The website (micaslicer.com) lives in a separate repository.

What works today and what comes next: [`ROADMAP.md`](ROADMAP.md).

## Where the logic comes from

Mica ports proven algorithms from OrcaSlicer, PrusaSlicer, Snapmaker's Orca fork and libbgcode.
Every ported or adapted file says exactly where it came from, and CI enforces it.
See [`PROVENANCE.md`](PROVENANCE.md).

```sh
scripts/fetch-upstream.sh   # fetch the pinned upstream sources into upstream/
cargo xtask provenance      # check every file's provenance header and the register
```

## License

[AGPL-3.0-only](LICENSE). See [`PROVENANCE.md`](PROVENANCE.md#licensing) for why.
