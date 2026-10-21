# Infill and surface finishing

This implements the remaining Prusa patterns and the crosshatch pattern used by U1 profiles,
plus ironing and unpainted fuzzy skin. Sequential printing is separate. Exact sources are
registered in `PROVENANCE.md`.

## API and CLI

`mica_core::fill::Pattern` now accepts `concentric`, `honeycomb`, `3dhoneycomb`, `hilbertcurve`,
`archimedeanchords`, `octagramspiral`, `adaptivecubic`, `supportcubic`, `lightning`, and `crosshatch`.
`fill::toolpaths` preserves variable widths for solid concentric fill when Arachne is selected.
The existing `fill::fill` polygon API handles the spatial patterns; object-dependent patterns
must use `fill::toolpaths` with a `PreparedFill` context.

`prepare_object` builds one adaptive/support octree or lightning generator for the object.
The octree uses an indexed arena; lightning retains compact point/parent records per layer and
only one mutable forest during preparation. Neither cache retains extrusion paths. Immutable
caches are shared across layer workers and do not depend on the requested batch ranges.
Two release CLI checks on the 225k-triangle Benchy (adaptive and lightning, 20%) had a combined
child-process peak RSS of 78.8 MiB on macOS, measured with `getrusage`; this includes the whole
slicer, mesh and output buffers, rather than just the cache.

Top and bottom surfaces accept rectilinear, aligned rectilinear, monotonic, monotonic lines,
concentric, Hilbert, Archimedean chords and octagram spiral. Prusa's legacy `solid_fill_pattern`
and `external_fill_pattern` aliases set **both** top and bottom patterns. They do not change
internal solids. `PrintSettings::internal_solid_fill_pattern` defaults to the existing Ensuring
implementation; Orca's retained `internal_solid_infill_pattern` selects an alternative.

`PrintSettings::ironing` contains enablement, `Top`/`Topmost`/`Solid` selection, flow fraction,
spacing, speed, pattern, angle and inset. `Role::Ironing` reaches G-code role comments, speed,
acceleration, time estimation and cyan preview paths. Orca's retained `ironing_pattern`,
`ironing_angle`, `ironing_angle_fixed` and `ironing_inset` are resolved. A zero inset means half
the nozzle diameter, as upstream.

With Orca slicing rules, both the plain-object and indexed-region pipelines print ironing
inside each island: all regions' walls and fills finish first, then each region's ironing,
before the next island's walls. Each ironing ExPolygon remains a no-sort collection, chained
from the preceding extrusion endpoint through `extrusion_order::chain_orca_fills`.
PrusaSlicer rules keep the existing layer-end ironing pass. Mica's CLI enables top ironing
with `--set ironing=1 --set ironing_type=top`; native Orca's `ironing_type=top` enables it
(`no ironing` disables it).

`PrintSettings::fuzzy_skin` contains `None`/`External`/`All`, thickness, point distance and a seed.
`All` includes hole boundaries, rather than all inner wall depths. The first layer stays smooth.
Classic loops are displaced before seam selection; Arachne junctions are displaced before
variable-width/overhang splitting. `FuzzySkinPainting::settings_at` is accepted by the public
`fuzzy_polygon` and `fuzzy_extrusion_line` functions. The print pipeline currently supplies
`NoPainting`; a 3MF painting decoder is not implemented.

```sh
export PATH="/opt/homebrew/opt/rustup/bin:$PATH"
cargo build --offline --release -p mica-cli
./target/release/mica slice tests/models/3DBenchy.stl -o /tmp/benchy.gcode \
  --set fill_pattern=adaptivecubic --set fill_density=20% \
  --preview /tmp/benchy-preview --layers 2,50,95
./target/release/mica slice tests/models/cube20.stl -o /tmp/finish.gcode \
  --set top_fill_pattern=concentric --set perimeter_generator=arachne \
  --set ironing=1 --set ironing_type=top \
  --set fuzzy_skin=external --set fuzzy_skin_thickness=0.3 --set fuzzy_skin_point_dist=0.8
```

## Oracle, 2026-10-16

`scripts/compare-infill.py` runs CORE One HF0.4 SPEED / Prusament PLA against PrusaSlicer 2.9.6,
creates cube30 by scaling the existing cube20 fixture, parses extrusion and XY lengths (including
arcs), and fails if the requested tolerance is exceeded. `--features` includes ironing and both
fuzzy wall generators. Outputs and previews stay in a system temporary directory. For example:

```sh
python3 scripts/compare-infill.py --features --preview
```

The measured delta is `(Mica / reference - 1) * 100`. Sparse patterns use 20% density.
The 24-case report comes from `scripts/compare-infill.py`; all cases passed. Values below
are internal-infill filament differences; their path-length differences agree to 0.01 percentage
points. Tolerances are ±3%, or ±8% for adaptive/support cubic and lightning.

| Pattern | Cube30 | 3DBenchy |
|---|---:|---:|
| Concentric | +0.00% | −0.74% |
| Honeycomb | +0.04% | −0.00% |
| 3D honeycomb | −0.00% | +0.13% |
| Hilbert | −0.07% | −0.50% |
| Archimedean chords | −0.01% | −0.34% |
| Octagram spiral | −0.01% | +0.19% |
| Adaptive cubic | −0.00% | +0.21% |
| Support cubic | −4.11% | +0.19% |
| Lightning | +7.31% | −6.02% |

| Finishing feature | Model | Filament | Path length | Extruding move count |
|---|---|---:|---:|---:|
| Ironing, top | cube20 | +0.02% | +0.00% | +0.00% |
| Ironing, top | 3DBenchy | −0.10% | −0.17% | −1.15% |
| Fuzzy external, classic | cube20 | +0.06% | +0.06% | +0.82% |
| Fuzzy external, Arachne | cube20 | −0.15% | −0.15% | −1.01% |
| Fuzzy external, classic | 3DBenchy | +0.76% | +0.76% | +0.65% |
| Fuzzy external, Arachne | 3DBenchy | −0.06% | −0.07% | −1.62% |

Fuzzy counts are deposited G-code move endpoints, not travel/retraction moves. The reference
randomizes displacement; Mica uses a reproducible per-loop/layer PRNG, so this is a statistical
comparison, not coordinate equality. Ironing passes ±3% filament/length and fuzzy passes ±5%
length/count. An additional cube20 Arachne concentric top/bottom run gives +0.006% top-solid
filament and +0.004% solid-infill filament against Prusa. Previews were inspected for honeycomb, concentric, lightning, ironing and U1
crosshatch at selected layers; the driver emits layers 2, 50 and 95 for every case. Cube lightning layers 130/140/145,
the final concentric top, and fuzzy Benchy layer 50 were also inspected.

U1 profile survey at the pinned Orca source found crosshatch in `fdm_process_U1`, gyroid in high
quality presets, and aligned rectilinear in the Benchy preset. OrcaSlicer 2.4.1 with the installed
U1 Standard / PLA / High Temp Plate profiles and **four explicit filament slots** completed
successfully. Cube20 crosshatch sparse filament is −0.24% and length −0.23%. A separate U1 ironing run (rectilinear, fixed 23° angle, 0.6mm inset,
0.13mm spacing, 15% flow) gives −0.31% filament and −0.33% length; long extrusion segments
measure 23° in both outputs. These are feature checks, not whole-profile parity: the base crosshatch run has
+24.1% internal-solid and −8.9% top-surface filament differences, outside this sparse oracle.

Tests adapt the square, hole and missing-segment cases from upstream `test_fill.cpp`. Additional
checks cover clipping every new spatial pattern against holes, adaptive/lightning batch invariance,
compact octree allocation, all visible-surface patterns, variable concentric widths, ironing face
selection/flow, fuzzy bounds/statistics, first-layer behavior and the painting hook. No standalone
lightning/adaptive test files were present in the pinned Prusa test tree.

Validation completed with `cargo fmt --all`,
`cargo clippy --offline --workspace --all-targets -- -D warnings`,
`cargo test --offline --workspace` and `cargo --offline xtask provenance`.
The workspace run passed with one existing manual thumbnail oracle ignored. Provenance checked
148 files, including 110 ported/adapted files.

## Differences to review

- Adaptive cubic uses the existing boundary connection graph. Upstream's internal T-joint hook
  generation and triangulated internal-bridge refinement of the octree are not yet ported. Mesh
  surface refinement is implemented. Sparse-oracle agreement does not establish junction strength.
- Lightning uses deterministic branch traversal and an explicit stable merge for upstream's
  non-transitive distance/hash ordering. Its boundary realignment uses Clipper2 intersections
  instead of retaining upstream's previous-grounding metadata. Root reconnection, pruning,
  straightening and short collinear-node removal are implemented. The specialized 3mm lightning
  bridge-candidate expansion is not yet ported. Review changing cross-sections and unsupported roofs.
- Solid concentric reuses Mica's existing Arachne implementation, with its existing documented
  width limitations. Fixed-width concentric uses offset bands without upstream's polygon-tree
  traversal for loop order.
- Classic fuzzy skin simplifies inputs on affected layers, matching the upstream pre-sampling
  step; ordinary classic slicing retains its previous simplification behavior. Review narrow
  classic features and hole seams. Noise is reproducible, not Prusa's process-global RNG sequence.
- Orca crosshatch is single-line only. Multiline replication, quarter cubic, line, zig-zag,
  cross zag, locked zag, lateral honeycomb/lattice, TPMS-D and TPMS-FK remain unimplemented.
  U1 shipped presets do not select those patterns.
- Orca fuzzy noise functions, extrusion/combined modes, all-wall and first-layer extensions,
  painted fuzzy areas, per-filament ironing overrides and multi-region ironing are not implemented.
  The supported finishing APIs operate on Mica's existing single-region object pipeline.
