# Bridges

The single-region pipeline now prints bottom bridges and internal bridges over sparse infill.
`mica_core::bridge` exposes `BridgeSurface`, `BridgeFlow`, and `detector::BridgeDetector`.
`FillSurfaces` retains expanded bottom geometry plus oriented external/internal bridge surfaces.
Shell classification runs the internal-bridge pass after horizontal shells. Candidate polygons are
kept per layer; actual sparse toolpaths are generated only for the lower layer being examined.
The mesh-coordinate infill origin is shared with final toolpath generation.

The existing CLI accepts all controls via profiles or `--set`: `bridge_angle` (degrees, 0 automatic,
180 forces X), `bridge_flow_ratio`, `thick_bridges`, `bridge_speed`, `bridge_fan_speed`,
`bridge_acceleration`, `dont_support_bridges`, and `solid_infill_every_layers`. Internal bridges
always use thick flow, even with thin external bridges. Density 0 still permits roof bridges;
density 100 does not. Soluble support contact (zero gap) suppresses external bridge classification.
Both bridge kinds are written as `;TYPE:Bridge infill` for Prusa printers, as PrusaSlicer has no separate internal-bridge role; Klipper output uses Orca's `Bridge` / `Internal Bridge`. `scripts/compare-gcode.py --split-bridges` separates them by height.
Bridge speeds participate in volumetric limiting but are fixed during layer-time slowdown. Bridge fan
boost obeys the first-layer ramp, never lowers normal cooling, and restores the layer fan on the next
ordinary path. It also works when automatic cooling is disabled. Zero bridge acceleration inherits
the default acceleration.

## Port and deliberate differences

All sources are pinned to `prusaslicer@b028299c77`; per-file attribution is in `PROVENANCE.md`.

- `BridgeDetector.{cpp,hpp}`: five-degree search plus contour/anchor candidates, coverage scoring,
  shortest-span tie breaking, trapezoid coverage, unsupported edges, and the active floating-edge
  direction rule. The active surface path in 2.9.6 uses floating edges, not the legacy search.
  Fully surrounded bridges use polygon area moments to find their short axis.
- `LayerRegion.cpp`: reuse Mica's existing wave expansion and bridge anchor grouping; retain each
  merged surface's direction. An explicit angle override is respected.
- `PrintObject::bridge_over_infill`: candidate filtering, depth checks, sparse path anchors, weighted
  direction selection, scanline extension/reconstruction, neighbouring-angle reuse, trimming and
  solid anchor rings on the preceding layer. Linear segment queries replace the upstream AABB tree;
  candidate layers run sequentially, so adjacent thick bridges cannot overlap in Z. No lightning or
  adaptive-infill integration, multi-region merging, or `combine_infill`.
- `Fill.cpp` / `Flow.cpp`: monotonic bridge fill when the top pattern is monotonic, fixed direction,
  thin/thick cross-sections, bridge flow ratio, spacing adjustment and matching flow adjustment.
  A circular cross-section is represented by `Flow` with equal width/height; bridge pattern spacing
  is stored separately. No global change to ordinary `Flow` semantics.
- `FillEnsuring.cpp`: narrow internal solids are filled with the existing Arachne engine. Broad
  regions retain Mica's rectilinear filler. The upstream vibrating-line graph filter and gap-path
  reconnection are omitted. This restores the narrow solids around internal bridges that a fixed-width
  rectilinear fill otherwise loses. `separate_infill_above_bridges` remains unported.
- Support exclusion uses sampled spans whose **both endpoints** are anchored, respecting the angle
  override. The legacy trapezoid coverage is retained for its upstream tests but is not safe for
  support exclusion (it can mistake two longitudinal anchored edges for a bridged rectangle).
  Coverage is sampled at nozzle-width intervals; tiny or complex partial bridges are approximate. Perimeter-specific support removal from upstream is not ported.

Review the support coverage approximation, internal bridge reconstruction and narrow-solid fill most
closely. Direction ties are deterministic in Rust; upstream uses unordered iteration for some ties.
On Benchy's embossed underside and symmetric sparse grids, equally scored orientations can differ
from Prusa. The pillar direction requirements below are met. These are G-code oracles, not print tests.

## Reproducing the oracle

Run from the repository root with the pinned profiles available. All generated meshes, G-code and previews
belong in a system temporary directory:

```sh
export PATH="/opt/homebrew/opt/rustup/bin:$PATH"
cargo build --offline --release -p mica-cli
python3 -B tests/models/bridges.py /tmp/mica-bridge-oracle/models
```

Reference command (repeat with `classic` and `arachne`, each model and distinct output names):

```sh
out/ref/PrusaSlicer-2.9.6.app/Contents/MacOS/PrusaSlicer \
  --datadir out/ref/datadir \
  --printer-profile 'Prusa CORE One HF0.4 nozzle' \
  --print-profile '0.20mm SPEED @COREONE HF0.4' \
  --material-profile 'Prusament PLA @COREONE HF0.4' \
  --center 125,110 --perimeter-generator arachne --no-binary-gcode \
  --export-gcode -o /tmp/mica-bridge-oracle/reference.gcode tests/models/3DBenchy.stl
./target/release/mica slice tests/models/3DBenchy.stl \
  --bundle upstream/prusa-profiles/PrusaResearch/2.5.12.ini \
  --perimeter-generator arachne -o /tmp/mica-bridge-oracle/mica.gcode \
  --preview /tmp/mica-bridge-oracle/preview --layers 2,39,184,196
python3 scripts/compare-gcode.py /tmp/mica-bridge-oracle/reference.gcode \
  /tmp/mica-bridge-oracle/mica.gcode --split-bridges
```

Prusa 2.9.6 labels **both** bridge kinds `Bridge infill`. The comparison script's optional
`--split-bridges` classifies its thicker paths as internal bridges; it requires thin external bridges
and unit flow ratio, as in this oracle. Mica's explicit internal role is preserved. Without the flag,
the script still reports literal role names.

Measured 2026-10-16, filament mm, Prusa → Mica (percent difference):

| Model / walls | External bridge | Internal bridge | Solid infill |
|---|---:|---:|---:|
| Benchy / Classic | 51.07 → 52.36 (+2.5%) | 168.90 → 171.21 (+1.4%) | 661.36 → 665.25 (+0.6%) |
| Benchy / Arachne | 51.06 → 52.65 (+3.1%) | 168.77 → 169.75 (+0.6%) | 667.70 → 665.37 (−0.3%) |
| Pillars 0° / both | 18.88 → 18.88 | 34.57 → 34.57 | within 0.2% |
| Pillars 30° / both | 18.88 → 18.88 | 35.39 → 35.39 | within 1.0% |
| Pillars 90° / both | 18.88 → 18.88 | 34.57 → 34.57 | within 0.2% |
| Sparse-core box / both | none | 217.35 → 217.35 | within 0.1% |
| Air cavity / Classic | 76.40 → 76.40 | 249.62 → 249.62 | within 0.1% |
| Air cavity / Arachne | 76.49 → 76.36 (−0.2%) | 249.62 → 249.62 | within 0.1% |
| Holed overhang / Classic | 28.99 → 28.99 | 108.95 → 108.95 | within 0.1% |
| Holed overhang / Arachne | 28.97 → 28.97 | 109.07 → 109.07 | within 0.1% |

The Classic run retains the pre-existing overhang-perimeter role deficit and gap-fill differences;
the bridge and solid-infill targets above pass independently.

Pillar extrusion directions: 0.0°, 30.0°, 90.0° for both slicers and both wall generators (measured to
0.1°, requirement ±2°). Previews inspected: pillars at layers 51/56, Benchy at 2/39/184/196, holed
overhang at 51. The hole remains open and bridge ends extend onto the anchors.

The Snapmaker U1 profile also slices the pillar model successfully and emits both Klipper bridge roles.
A CLI support check produces no support roles for the straight two-pillar bridge with
`dont_support_bridges=1`, and retains support when forced to `bridge_angle=90`.

Tests port the upstream O-shaped, two-sided, C-shaped and partial L-shaped bridge fixtures and thick
bridge flow ratios. Added regressions cover rotations, detached/cantilever coverage, angle overrides,
zero-contact supports, wrong-direction support retention, sparse/empty/full density, depth filtering,
periodic solid infill, surface partitioning, batched paths, setting resolution, and emitted bridge
speed/acceleration/fan restoration.

Validation: `cargo fmt`, `cargo clippy --workspace --all-targets -- -D warnings`,
`cargo test --workspace` (329 tests, one ignored) and `cargo xtask provenance` pass. The Prusa
profile tests need the pinned profiles from `scripts/fetch-upstream.sh --profiles`.
