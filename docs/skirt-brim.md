# Skirt and brim

`mica-core::brim::generate(AdhesionInput)` prepares one `AdhesionPlan` for a plate. Borrowed
`FirstLayerIsland`s carry object IDs, per-object brim settings and actual mesh height. Pass placed
coordinates, support/raft contact regions, and `SkirtHull`s containing object/support points through
the skirt height. Equal object IDs are grouped before per-object skirt collision merging.
`PrintSettings::adhesion` resolves profile settings once.

`prepare_object` uses `brim/prepare.rs`; the plan is public on `PreparedObject` and `SlicedObject`.
The layer hook prefixes skirt, brim, support, then model paths. Skirts count object/raft layers,
skipping interlaced support-only Zs. `paths_for_tool` accepts the actual layer tool order and height
since the previous skirt. `skirt_ranges` divides Prusa loops evenly among those tools; Orca assigns
its skirt to the first printing tool. `PreparedPrint` now prepares **one plan for all objects**,
replaces the child prefixes, and inserts adhesion once in actual tool order. Orca brims retain
their owners' tools. See [plate integration and oracle results](plate-level.md).

The small G-code hook opens loops and clips their seam gap, then uses the existing travel,
extrusion, cooling and timing pipeline. Roles are separate `Skirt`/`Brim` internally,
`Skirt/Brim` in Prusa G-code, and `Skirt`/`Brim` in Orca G-code. Preview supports both. Prusa skirt
uses support speed; Orca honors positive `skirt_speed`. Percent first-layer speed is supported.

## CLI

No dependencies or new command are needed. Options use the existing `--set key=value`:

```sh
export PATH="/opt/homebrew/opt/rustup/bin:$PATH"
cargo build --offline --release -p mica-cli
./target/release/mica slice tests/models/hole_block.stl -o /tmp/brim.gcode \
  --set skirts=1 --set skirt_distance=3 \
  --set brim_type=outer_and_inner --set brim_width=3 --set brim_separation=0.1

./target/release/mica slice tests/models/cube20.stl -o /tmp/ears.gcode \
  --vendor Snapmaker --printer 'Snapmaker U1 (0.4 nozzle)' \
  --print '0.20 Standard @Snapmaker U1 (0.4 nozzle)' \
  --filament 'Snapmaker PLA @U1' --bed 'High Temp Plate' \
  --set brim_type=brim_ears --set brim_ears_max_angle=125
```

Prusa: `brim_type` (`no_brim`, `outer_only`, `inner_only`, `outer_and_inner`), `brim_width`,
`brim_separation`, `skirts`, `skirt_distance`, `skirt_height`, `min_skirt_length`, `draft_shield`
(`disabled`, `limited`, `enabled`). Orca adds `auto_brim`, `brim_ears`, `brim_ears_max_angle`,
`brim_ears_detection_length`, `skirt_type` (`combined`, `perobject`), `skirt_speed`,
`skirt_start_angle`, and `brim_use_efc_outline`. CLI aliases `skirt_loops` and `brim_object_gap`
map to `skirts` and `brim_separation`, as profile imports do. `painted` prints the object's hand-placed ears (see [layer heights and brim ears](layer-heights-and-brim-ears.md)).

## Geometry and scope

Prusa brims use iterated square expansion, half-spacing centerlines, permitted-area clipping and
inner-hole offsets. Holes containing islands use annular areas with object/gap exclusions.
Skirts use a convex hull across their height, round offsets and minimum **filament** length.
Wide brims enlarge ordinary skirts. Draft shields retain object distance and trim crossing brims.
Per-object skirt envelopes merge on collision. Short open brim connectors check boundaries and
centerlines. Prusa allows brim over supports; Orca masks support contact areas. The adapter
suppresses object brim with a raft, matching a separate Prusa oracle check.

Orca auto brim uses footprint moments, mesh height, maximum speed, material adhesion/thermal
parameters, small-contact-area rules and island-size caps. Widths round down to an even number
of spacings. Ears use decimated corner angles and 24-sided circles; concentric infill uses
Orca's −1.3/+0.3-spacing offsets.

Both dialects' `first_layer_print_*` placeholders now use `LayerSource::first_layer_hull` and
`first_layer_bbox`, including support, skirt and brim envelopes. Nominal brim areas follow upstream
bookkeeping, including the fraction left after rounding the loop count. Orca 2.4.1's zero-loop
auto-brim footprint retains skirt-distance padding even when the resulting brim width is zero.

Clipper2 replaces Clipper 6.4.2. Containment booleans and stable area ordering replace Z-tagged
polygon trees; segment scans replace EdgeGrid. Auto-brim volume groups are whole object IDs with
one material/max-speed input per plan. Split material/volume groups, per-tool bed
exclusion polygons and sequential-print adhesion emission are left out. Plate jobs now include
wipe-tower footprints in combined skirts and startup hulls.
Prusa loop opening uses the nearest vertex; Orca's skirt start angle projects onto an edge.
Negative seam-gap extensions and clockwise preferences are not added by the adhesion hook.

## Oracle results (2026-10-16)

PrusaSlicer 2.9.6, CORE One HF0.4 / 0.20mm SPEED / Prusament PLA, center 125,110.
Separate overrides: skirt (`skirts=1`, `skirt_distance=3`); outer (`brim_width=5`); both
(`brim_type=outer_and_inner`, `brim_width=3`); separated (`brim_width=5`, `brim_separation=0.1`).
The profile already uses 0.1 mm separation, so outer and separated cases agree.

| Model | Skirt filament difference | Outer 5 mm | Both 3 mm | Separated 5 mm | Maximum loop-bound error |
|---|---:|---:|---:|---:|---:|
| cube20 | +0.1% | +0.0% | +0.0% | +0.0% | <0.001 mm |
| hole_block | +0.1% | +0.0% | +0.0% | +0.0% | 0.006 mm |
| 3DBenchy | +0.0% | +0.0% | +0.1% | +0.0% | 0.026 mm |

All twelve cases meet ±2% filament and have identical loop counts. `scripts/compare-gcode.py`
checks per-role filament; `scripts/compare-adhesion.py` emits JSON with matched per-loop bounds
(including arcs), filament totals and a failing exit outside ±2% / 0.06 mm. Raw exports and loop
JSON stay in the system temp directory.

OrcaSlicer 2.4.1, U1 / 0.20 Standard / Snapmaker PLA / High Temp Plate, four explicit filament files
as in `u1-klipper.md`; same placement at 135.5,136. The tower is cube20 scaled to 2×8×100 mm in a
temporary STL. No Orca run crashed.

| Case | Reference / Mica brim filament | Loops | Maximum loop-bound error | Identical startup mesh bounds |
|---|---:|---:|---:|---|
| Default cube | 0 / 0 mm | 0 | 0 | 123.5,124 → 147.5,148 |
| Cube, brim_ears | 13.64198 / 13.64245 mm | 16 | 0.013 mm | 119.498,119.998 → 151.502,152.002 |
| Tall thin, auto_brim | 31.22806 / 31.23111 mm | 16 | 0.013 mm | 125.258,122.758 → 145.742,149.242 |

An additional U1 run with two skirts on three layers, 37 mm/s and start angle 90° matches all
six loop bounds exactly, its bed-mesh command exactly, and filament to 0.0003%. Both emit F2220.

Tests adapt active upstream scenarios and add nested holes, gap exclusion, supports, draft shields,
per-object grouping, tool allocation, profile mapping, footprint macros and batch equivalence.
Upstream's disabled multi-tool/ear cases become explicit allocation/angle assertions; these do not
claim live multi-tool export coverage in the original single-object implementation. The subsequent
plate integration adds scheduled multi-tool tests and an XL three-object live oracle.

Review plate scheduler integration closely: dialect-correct compensated brim contours,
uncompensated skirt-height hulls, adhesion inserted once, and actual layer tool order. Review
nested-hole topology and Clipper2 rounding against the oracle matrix when geometry changes.

## U1 Benchy brim (2026-10-17)

The +24.9 % U1 Benchy brim reported earlier is Orca's CLI arrange rotating the model 90°
(`allow_rotations`). In the same orientation the brim is −2.39 % (unrotated) / +2.04 % (rotated).

## Skirt and brim past the bed (2026-10-18)

As in PrusaSlicer and Orca, an object outside the print area is a warning before slicing and a
toolpath outside it an error after. `mica_core::adhesion_check` does both for skirts and brims:

- **Before slicing** (`preflight`, run by `input::prepare_plate` and the GUI's plate warnings):
  the object's section at half the first layer height, grown by the outer brim (PrusaSlicer's whole
  loops from the elephant-foot-compensated outline, Orca's brim width) and the skirt around it,
  against the bed rectangle. The GUI tints the object, draws the outline in red and offers
  **Move inside** (one undo step, never automatic).
- **After preparing** (`plan_overrun`, in `PreparedPlate::finish`): the generated skirt and brim
  paths by the exporter's own bounds rule. A print the exporter would refuse fails here instead,
  in the same words for the CLI and the GUI, for example
  `The brim of cube20.stl goes 7.5 mm past the left edge of the bed; move it 7.5 mm to the right,
  or make the brim narrower`.
