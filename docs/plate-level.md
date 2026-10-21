# Plate adhesion, startup footprint and travel integration

Status on 2026-10-16: plate adhesion and startup hull integration are implemented. **The travel
acceptance target is not met on three of four models. The two-tool live Orca oracle
is incomplete.** The results below distinguish those gaps from the passing XL comparisons.

## API and CLI

`PreparedPrint::adhesion` is one public `AdhesionPlan` for the placed plate. Construction replaces
child adhesion plans before the tool-usage prepass, then gathers all model/support footprints and
the scheduled tower envelope. `LayerSource::first_layer_hull` and `first_layer_bbox` now return
this plate footprint. `object_first_layer_bbox` retains raw model bounds for Orca's
`first_layer_center_no_wipe_tower`.

The scheduler emits adhesion once at each eligible object/raft Z, using height since the previous
skirt rather than an intervening support-only layer. Prusa skirt loops are divided among that
layer's actual ordered tools; its brim uses the first tool. Orca skirts use the first printing
tool, while brims retain their owning instance's perimeter tool. Parts share an instance's brim
identity. Per-object Orca skirts group all copies of the same object and merge colliding envelopes.
A tower contributes occupied space but does not emit a separate per-object skirt. Its obstacle
envelope is expanded only after joining an emitting skirt group. Object brims exclude other
objects; Orca brim areas additionally reserve previously allocated areas to avoid double deposition.
Plate adhesion and tower paths remain outside object-cancellation labels.

`TowerPlan::first_layer_hull()` supplies the nominal Prusa tower/brim/cone envelope and Orca's
tower-local first-layer bounds plus cone, transformed into plate coordinates once. This is separate
from the existing extrusion-centreline `first_layer_bbox()`.

The existing CLI accepts these changes without new flags or dependencies:

```sh
export PATH="/opt/homebrew/opt/rustup/bin:$PATH"
cargo build --offline --release -p mica-cli
python3 scripts/prepare-wipe-tower-oracles.py --plate --output /tmp/mica-plate-inputs
./target/release/mica slice /tmp/mica-plate-inputs/xl-cubes.3mf \
  --printer 'Original Prusa XL - 5T Input Shaper 0.4 nozzle' \
  --print '0.20mm SPEED @XLIS 0.4' --filament 'Prusament PLA @XLIS' \
  --set skirts=3 --set skirt_distance=3 --set brim_width=3 -o /tmp/xl-plate.gcode
./target/release/mica slice /tmp/mica-plate-inputs/u1-cubes.3mf \
  --vendor Snapmaker --printer 'Snapmaker U1 (0.4 nozzle)' \
  --print '0.20 Standard @Snapmaker U1 (0.4 nozzle)' \
  --filament 'Snapmaker PLA @U1' --bed 'High Temp Plate' \
  --set wipe_tower_x=15 --set wipe_tower_y=220 -o /tmp/u1-plate.gcode
```

The generated projects contain three XL cubes on tools 1/3/5, two U1 cubes on 1/3, and two
CORE cubes on tool 1. They preserve stored placement. The generator never launches a slicer.
Its Orca profiles now explicitly expand missing singleton defaults from the already registered
Orca option schema, including `filament_is_support`, as well as supplying four distinct filament
IDs, colours, mappings, flush vectors/matrix and `physical_extruder_map`.

## Travel investigation and remaining mismatch

`AvoidCrossingPerimeters` now constructs external boundaries from every placed object's holes.
Moves between islands can route around those holes instead of always falling back to a straight
line. The active pinned `get_boundary_external` implementation permits crossing outer contours
during such moves; the different contour-ring implementation later in the source is inside
`#if 0`. The external route uses the existing visibility simplifier and mm/percent detour limit.

The reported hole-block deficit was largely an infill path-topology difference. Upstream adds
multi-sweep grid angles as `float` before double-precision rotation. Mica's exact double sums
changed rounded endpoints and the ordering of nearly equal boundary-connection arcs, joining
lines that the reference leaves disconnected. Matching that float accumulation recovers much of
the missing travel. Disconnected fill surfaces are also now chained from the current position;
monotonic/concentric scan order inside a surface is preserved.

Fresh references use PrusaSlicer 2.9.6, CORE One HF0.4 / 0.20mm SPEED / Prusament PLA, its datadir
vendor bundle, default arc fitting, and avoidance enabled. Single models are centred at 125,110;
the two-object 3MF retains placement. Both sides have the same layer counts. Totals are XY travel
from `compare-overhang-travel.py`, including retraction/wipe movements under its existing rules.

| Model | Reference mm | Before mm | Current mm | Current difference |
| --- | ---: | ---: | ---: | ---: |
| cube20 | 3789.469 | 2114.491 | 4242.210 | **+11.95%** |
| hole_block | 2172.611 | 1471.896 | 1837.264 | **−15.44%** |
| 3DBenchy | 23525.512 | 28079.996 | 27760.946 | **+18.00%** |
| Two cubes | 11081.825 | 8918.002 | 11931.662 | +7.67% |

Only the two-object job meets ±10%. The earlier +0.09% Benchy result in `overhang-travel.md`
predates the merged baseline, which started at +19.36%. On a representative hole-block
middle layer, reference travel is 38.78 mm versus 23.60 mm before and 26.66 mm after the precision
fix. Reference infill has two paths there; Mica still has one. Nearly equal connection-arc ordering
and clipping/rounding need further work. Benchy's discrepancy also exists with avoidance disabled
(reference 21361.32 mm, Mica 26308.22 mm before the surface-order fix), so it cannot be corrected
solely in the travel router. Solid/bridge/sparse ordering and geometry remain relevant.

The router still uses ordinary Clipper2 insets and a smaller-inset fallback. Upstream's variable
inset/resampling, external-wall-centreline early-out, top-surface exclusions, endpoint extensions
and wipe suppression are not fully ported. Extending only the centreline early-out without that
machinery increased Benchy travel; that incomplete change is not included. No artificial travel
or oracle-specific detour multiplier was added.

## XL three-object oracle

PrusaSlicer 2.9.6, XL 5T Input Shaper / 0.20mm SPEED / Prusament PLA, three 20 mm cubes on
tools 1/3/5, three skirt loops at distance 3 mm, 3 mm object brim, default wipe tower. The explicit
5×5 wiping matrix is supplied for safe CLI setup.

- All 21 adhesion loops have identical bounds at the emitted 0.001 mm precision.
- Adhesion filament: 94.64382 mm reference, 94.64603 mm Mica, **+0.00234%**.
- All layer tool sequences and all 207 T commands match. The worst tower-layer filament
  difference is **0.0147%**; no tower layer is outside ±3%.

Per-role filament is in mm; tool numbers in this table are one based. Model-role entries are
identical across the three tools, so each of those rows applies separately to tools 1, 3 and 5.

| Tool | Role | Reference | Mica | Difference |
| --- | --- | ---: | ---: | ---: |
| 1 | Skirt/Brim | 71.630 | 71.632 | +0.003% |
| 3 | Skirt/Brim | 11.561 | 11.562 | +0.009% |
| 5 | Skirt/Brim | 11.452 | 11.453 | +0.009% |
| 1 | Wipe tower | 1621.261 | 1621.244 | −0.001% |
| 3 | Wipe tower | 616.947 | 616.950 | +0.0005% |
| 5 | Wipe tower | 560.485 | 560.485 | 0.000% |
| 1, 3, 5 each | Perimeter | 253.701 | 253.701 | 0.000% |
| 1, 3, 5 each | External perimeter | 264.750 | 264.750 | 0.000% |
| 1, 3, 5 each | Solid infill | 170.345 | 170.349 | +0.002% |
| 1, 3, 5 each | Internal infill | 490.646 | 490.585 | −0.012% |
| 1, 3, 5 each | Bridge infill | 40.368 | 40.368 | 0.000% |
| 1, 3, 5 each | Top solid infill | 28.178 | 28.178 | 0.000% |

Startup templates were instrumented with comment-only copies of `first_layer_print_min/max`
and every hull point. Both print min=127.729,102.729 and max=247.571,156.004, and exactly:

```gcode
M555 X127.729 Y102.729 W119.842 H53.2756
```

The convex-hull vertex lists are not identical: 27 reference vertices versus 28 Mica vertices.
Maximum vertex-to-opposite-polygon distance is 0.0678 mm. Mica retains an extra round-offset
vertex near the tower's right-front corner. Clipper2's rounded offset differs from upstream
Clipper; matching bounds and extrusion loop counts do not imply identical hull polygons.

Reproduce the reference using PrusaSlicer 2.9.6 with a fresh data directory, the XL presets above,
`--load /tmp/mica-plate-inputs/xl-matrix.ini --dont-arrange --no-binary-gcode --export-gcode`.
Compare with `compare-adhesion.py`, `compare-multitool.py` and `compare-wipe-tower.py` in `scripts/`.
The detailed reports are `xl-adhesion.json`, `xl-roles.json`, `xl-comparison.json` and
`xl-placeholder-comparison.json` in the temporary output directory.

## U1 oracle status

The two-object, tools 1/3 reference stopped during Orca 2.4.1 input validation:
`filament_is_support` had one entry while `filament_colour` had four. Exit code was 251;
it did not reach slicing or the ToolOrdering crash. The four explicit filament files alone were
insufficient. Missing vectors have been corrected in the input generator, but the run was not
repeated. This exact job's reference role/tool totals and hull remain unverified.

Mica exports the two-object job successfully with default U1 adhesion and a prime
tower at 15,220. Its mesh bounds are min=7.98174,108, max=192,231.802, bicubic 5×4.

A supplemental comparison against the previously successful **four-tool** saved Orca reference
confirms the integration reaches startup templates. Both emit exactly:

```gcode
BED_MESH_CALIBRATE mesh_min=7.05502,108 mesh_max=262,242.975 ALGORITHM=bicubic PROBE_COUNT=7,4 ADAPTIVE=0 ADAPTIVE_MARGIN=0
```

This includes the tower and Orca's zero-skirt auto-brim distance padding. It is not a replacement
for the two-tool oracle. The ordinary single-tool live U1 workspace test also passes.

## Validation and review

Offline release build, `cargo fmt --all`, strict offline workspace Clippy,
`cargo test --offline --workspace`, and `cargo --offline xtask provenance` pass. The full suite
includes the pinned Prusa profiles and existing live Prusa/Orca tests; the pre-existing manual
thumbnail/performance oracle remains ignored.

New regressions cover combined skirt/tool allocation, Orca brim ownership and copy grouping,
collision exclusion, tower skirt obstacles, raft heights, first-layer hull bounds, cancellation
labels, batch identity, multi-object external-hole travel and disconnected monotonic surfaces.

Adaptations are registered per file in `PROVENANCE.md`: Prusa/Orca plate assembly and tower
footprints, Prusa external travel boundaries and fill collection ordering. Painted adhesion,
per-material auto-brim volume groups, per-tool bed exclusions, sequential printing and automatic
object/tower collision placement remain outside this implementation. Shared adhesion flow still
uses the first tool's width/material settings, so mixed-nozzle/material adhesion needs review.

Review the disconnected-fill precision/order changes, close/interlocking multi-material brims,
rotated/conical tower envelopes and narrow-neck travel fallback closely. The three failing
travel targets and missing two-tool Orca oracle are outstanding acceptance work.
