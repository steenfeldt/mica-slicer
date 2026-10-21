# Normal supports

The module is in `crates/mica-core/src/support/`. It adapts the pinned PrusaSlicer normal-support
pipeline in the survey's order: layer/parameter types, shared layers/interfaces/raft/toolpaths,
then snug and grid contact detection, downward projection and the build-plate coverage mask.
The original Python model generator writes T and bridge STL fixtures.

## API and integration

`support::generate(&SupportInput) -> Result<SupportPlan, SupportError>` takes borrowed
`SupportObjectLayer` records containing absolute `print_z`, extrusion `height`, outlines and optional
classified overhang/bridge polygons, plus a `SupportConfig` and pattern origin. `None` overhangs invokes
outline-based detection; `Some(&[])` explicitly says there are none. A bridge mask is subtracted only
when `dont_support_bridges` is enabled. No mesh or `PreparedObject` dependency is required.

`SupportPlan` retains polygon regions, not toolpaths. Its `toolpaths(index)` lazily returns a
`LayerToolpaths` with `SupportMaterial` and `SupportMaterialInterface` roles. Support Z levels are
independent of object levels. The plan includes resolved flows, gap/density parameters and raft lift.
When supplying raft inputs directly, lift the object layers by `SupportParameters::object_z_offset`;
the generator validates that the object is above its raft.

`support::schedule` is the thin pipeline adapter. `print::try_prepare_object` slices the object,
generates the support polygon plan, and merges the physical Z schedules. Existing `prepare_object`
remains a compatibility wrapper which panics on an invalid/unsupported support configuration. Object
indices still drive infill and shell calculations. Export indices count support-only layers as well.
At equal Z, support paths precede object paths. Paths remain batched, and start-code/probing bounds
include the first support layer or raft. The shell discovery and wall/infill algorithms are unchanged.

`PrintSettings` / `resolve.rs` include the normal-support, raft and speed settings, plus
closing radius, sheath, enforced layer count, synchronization, bridge exclusion, first-layer density
and expansion, and height limits. Automatic support width resolves to nozzle diameter; the support
first-layer width has its own fallback. Bottom gap zero inherits the top gap, and bottom interface
count -1 inherits the top count. Interface speed percentages resolve against support speed.

The actual pinned CORE One profile in this checkout resolves to support enabled, automatic detection
**disabled**, and **snug**. These values are preserved, and the existing complete-settings equality
test passes. Force both automatic detection and the normal style when comparing or using supports.
Honeycomb support and contact-loop generation report errors when requested. Tree and organic support
are implemented in [tree-supports.md](tree-supports.md).

## CLI

The CLI adds repeatable `--set key=value` overrides using the existing typed config parser:

```sh
export PATH="/opt/homebrew/opt/rustup/bin:$PATH"
cargo build --release --offline -p mica-cli
python3 tests/models/support_overhangs.py /tmp/mica-support-models
./target/release/mica slice /tmp/mica-support-models/support_t.stl \
  -o /tmp/mica-t-snug.gcode \
  --set support_material=1 --set support_material_auto=1 \
  --set support_material_style=snug \
  --preview /tmp/mica-support-preview --layers 1,3,61,65,67
```

Use `grid` for grid-shaped columns. `support_material_pattern=rectilinear-grid` independently
alternates the sparse fill direction; this is different from the column style. Add
`--set support_material_buildplate_only=1` to exclude support above object surfaces, or
`--set raft_layers=3` for a raft (also works with support disabled).

Preview colours: support base green, interface cyan. T layers 1, 3, 61, 65 and 67, Benchy support
layers and raft layers were rendered and inspected: the flanges, sparse columns, dense interfaces,
clearance from the pillar and lifted object are visible. The previews and reference G-code stay in a
system temporary directory; no reference outputs or vendor profile data are in the repository.

## Oracle measurements

PrusaSlicer 2.9.6, CORE One HF0.4, 0.20mm SPEED, Prusament PLA, centred at 125,110. The supplied
`--help-fff` flags were checked. Run with `--support-material --support-material-auto
--support-material-style snug` (or `grid`) and `--no-binary-gcode`. Other settings are the profile's
values: 0.2 mm top gap, 0.4 mm support width, 2 mm sparse gap, 0.2 mm interface gap, 3 top interfaces,
0 bottom interfaces, 35° threshold, 80% XY gap, no sheath, thin bridges.

```sh
ref=out/ref/PrusaSlicer-2.9.6.app/Contents/MacOS/PrusaSlicer
"$ref" --datadir out/ref/datadir \
  --printer-profile "Prusa CORE One HF0.4 nozzle" \
  --print-profile "0.20mm SPEED @COREONE HF0.4" \
  --material-profile "Prusament PLA @COREONE HF0.4" \
  --center 125,110 --no-binary-gcode --support-material --support-material-auto \
  --support-material-style snug --export-gcode \
  -o /tmp/prusa-t-snug.gcode /tmp/mica-support-models/support_t.stl
python3 scripts/compare-supports.py /tmp/prusa-t-snug.gcode /tmp/mica-t-snug.gcode /tmp/t-snug.csv
```

The comparison script totals positive extrusion by support role and physical print Z. It handles
absolute/relative E, resets and **G2/G3 arcs**, and excludes stationary retraction/deretraction. It
matches Z rounded to 0.001 mm (Mica writes three decimal places). CSVs retain separate base/interface
filament totals per Z. Missing layers are reported separately, never treated as passing comparisons.
Numbers below are filament millimetres from the 2026-10-15 run; Prusa's path optimization can vary
slightly across runs.

| Model / style | Support layers Prusa / Mica | Base mm Prusa / Mica | Interface mm Prusa / Mica | Total difference | Same-Z layers within ±10% / all Prusa support layers |
|---|---:|---:|---:|---:|---:|
| T / snug | 33 / 33 | 167.005 / 166.156 | 36.476 / 36.637 | -0.34% | 33 / 33 |
| T / grid | 33 / 33 | 203.166 / 205.053 | 53.215 / 53.151 | +0.71% | 33 / 33 |
| Bridge / snug | 33 / 33 | 150.505 / 150.443 | 36.063 / 36.135 | +0.01% | 33 / 33 |
| Bridge / grid | 33 / 33 | 166.442 / 168.810 | 45.260 / 45.454 | +1.21% | 33 / 33 |
| Benchy / snug | 174 / 172 | 1241.179 / 1211.215 | 66.501 / 44.588 | -3.97% | 93 / 174 |
| Benchy / grid | 188 / 184 | 1955.797 / 1947.214 | 211.738 / 222.593 | +0.10% | 131 / 188 |
| Benchy / snug, build plate only | 88 / 89 | 455.917 / 383.616 | 3.206 / 0.426 | **-16.35%** | 25 / 88 |
| Cube / 1-layer raft | 1 / 1 | 0 / 0 | 28.700 / 28.614 | -0.30% | 1 / 1 |
| Cube / 3-layer raft | 3 / 3 | 62.789 / 61.926 | 84.470 / 84.054 | -0.87% | 3 / 3 |

The T and bridge meet the initial per-layer target in both styles. Benchy meets the **total** target
in both unrestricted styles, but does **not** meet full per-layer parity. Snug has 121 common Z levels
(53 missing, 51 extra); grid has 144 (44 missing, 40 extra). Build-plate-only Benchy remains outside the
total target and has 61 common levels (27 missing, 28 extra). Its narrow bow columns and interfaces
need closer review. Totals alone would hide these differences.

## Adaptations and remaining work

- Support contacts share one non-overlapping slab schedule. Prusa keeps independent top/bottom
  contact thicknesses and modulates extrusion where contacts overlap intermediate layers. Mica
  preserves exact simple top/bottom gaps and maximum thickness, but this simpler schedule changes
  complex-model Z levels and interface thickness/count assignments.
- Snug performs square-join morphological closing after each object slice is subtracted. Clipper2
  simplification at 10 µm replaces `MutablePolygon::smooth_outward`. Thin curved regions therefore
  differ. This is particularly visible on the build-plate-only Benchy case.
- Grid uses scanline strips and exact polygon clipping of occupied cells, with connected-piece
  filtering at object masks. This replaces AGG subpixel rasterization and macro-cell flood fill.
  The propagation footprint excludes the half-line printing margin so it does not grow every layer.
- `FillSupportBase` scanline placement is ported and uses Mica's existing boundary graph. The
  dedicated upstream ranked arches, caps and T-joint heuristics are not ported. This affects thin,
  curved support columns. Dense fills reuse the existing rectilinear implementation. Concentric
  interfaces use repeated polygon offsets.
- No enforcers/blockers/painting, separate support tools, mixed soluble/non-soluble materials, trees
  or organic supports in this first-part implementation; see [tree supports](tree-supports.md) for the
  subsequent tree/organic work. Honeycomb and contact loops are explicitly unsupported. Bottom contacts use
  normal, thin extrusion flow; thick-bridge contact splitting is absent.
- The narrow API accepts bridge masks, but the current outline-only pipeline does not supply them.
  With no masks, `dont_support_bridges` conservatively retains support. Object bridge detection,
  bridge flow/speed above supports and above rafts still belong to the object bridge stage.

Review `material.rs` (especially grid masks and narrow snug columns), `common.rs` contact scheduling
and the small `print.rs` adapter closely. `LayerSource` counts physical output layers; object layer
indices remain separate.

## Validation

`cargo fmt --all`, `cargo clippy --offline --workspace --all-targets -- -D warnings`,
`cargo test --offline --workspace` (**199 passed**, including the profile equality tests against the
pinned Prusa profiles) and `cargo xtask provenance` pass.

The support regressions adapt Prusa's three-raft-layer and support-height tests, and add disabled/
manual modes, exact non-integral contact gaps, bottom contact thickness/interface counts, interface
roles/counts, object collision exclusion, build-plate-only shelves, optional bridge masks, independent
Z merging/support-first ordering, raft pipeline lift and batch invariance, and invalid settings.
The old elephant-foot raft-gate test now accounts for actual raft layers before the object.

## Orca normal supports (2026-10-17)

Orca configurations (those that set `support_remove_small_overhang`) select Orca's
`SupportParameters` semantics: the interface count excludes the contact layer, and
`support/orca_overhangs.rs` applies Orca's sharp-tail, cantilever and small-overhang-cluster rules
to the automatic overhangs. `tree_support_wall_count` also selects the sheath (`with_sheath = wall
count > 0`). On a `normal(auto)` copy of the private multi-tool U1 project this brings support from
+29.7 % to −4.0 % of Orca 2.4.1.
