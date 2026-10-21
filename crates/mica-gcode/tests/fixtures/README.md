# Binary G-code regression fixtures

## Exact layer-move oracle (2026-10-16)

`coreone_cube20_layer_moves.gcode` is PrusaSlicer 2.9.6 output normalized by
`scripts/gcode-exact-diff.py`: 100 layers, 6,256 commands, 151,320 bytes. Its SHA-256 is
`88ce18d7b9fb39002161e34a29f80ee18818e16999620dfb495b7bbf231295d4`.
It uses cube20, Arachne, Prusa CORE One HF0.4 nozzle, 0.20mm SPEED @COREONE HF0.4,
Prusament PLA @COREONE HF0.4, and placement at 125,110. It is generated output,
registered in `THIRD_PARTY.md`, not upstream source.

The comparator retains command boundaries, parameter presence, feedrate-only moves,
fan/temperature/acceleration commands and the final retract/fan-off. It drops comments,
M73, M486, M117 and object-exclusion labels, and converts E to relative extrusion.
Comparison starts at the first `;LAYER_CHANGE` and ends before shutdown `;TYPE:Custom`.
Tolerances: XYZ/IJK/R 0.002 mm, E 0.0001 mm, F 1 mm/min; other words must agree exactly.
Counts are differing command **positions per layer**, without resynchronization: an
inserted command can produce many subsequent differences. Field categories overlap.

```sh
python3 scripts/gcode-exact-diff.py reference.gcode mica.gcode -n 5
python3 scripts/gcode-exact-diff.py reference.gcode mica.gcode --layer 50 -n 10
python3 scripts/gcode-exact-diff.py reference.gcode --normalize normalized.gcode
```

`--layer` limits detail output only; repeatable `--only-layer` limits comparison.
Exit status is 0 for equality, 1 for differences, 2 for invalid input.

Regenerate the fixture with the installed reference application, storing its full export
in the system temporary directory:

```sh
oracle_dir="$(mktemp -d "${TMPDIR:-/tmp}/mica-exact.XXXXXX")"
out/ref/PrusaSlicer-2.9.6.app/Contents/MacOS/PrusaSlicer \
  --datadir out/ref/datadir \
  --printer-profile "Prusa CORE One HF0.4 nozzle" \
  --print-profile "0.20mm SPEED @COREONE HF0.4" \
  --material-profile "Prusament PLA @COREONE HF0.4" \
  --center 125,110 --perimeter-generator arachne --no-binary-gcode \
  --export-gcode -o "$oracle_dir/cube20.gcode" tests/models/cube20.stl
python3 scripts/gcode-exact-diff.py "$oracle_dir/cube20.gcode" \
  --normalize crates/mica-gcode/tests/fixtures/coreone_cube20_layer_moves.gcode
```

**Move-for-move acceptance is not achieved.** The active regression compares all commands
on Arachne layers 0, 1, 2, 96, 97, 98 and 99, and checks fixture round-trip preservation.
The full 100-layer assertion is an explicitly ignored, currently failing acceptance test:

```sh
cargo test --offline -p mica-gcode --test exact cube_all_layers -- --ignored
```

Current live comparisons, with the same profiles and placement in both slicers:

| Model | Generator | Differing command positions | Differing layers | Reference / Mica commands |
|---|---|---:|---:|---:|
| cube20 | Arachne | 429 | 93 / 100 | 6,256 / 6,254 |
| cube20 | Classic | 598 | 94 / 100 | 6,253 / 6,262 |
| hole_block | Arachne | 8,093 | 50 / 50 | 8,948 / 8,801 |
| hole_block | Classic | 4,219 | 50 / 50 | 5,767 / 5,706 |
| 3DBenchy | Arachne | 124,161 | 240 / 240 | 125,483 / 120,170 |
| 3DBenchy | Classic | 116,338 | 240 / 240 | 117,335 / 119,827 |

Update 2026-10-17 (Prusa wipe, ported avoid crossing perimeters, ShortestPath ordering, seam
visibility decimation, CoolingBuffer float splits), Arachne defaults, CORE One / XL:
cube20 421 / 421 positions (XL was 6,544 without wipe); 3DBenchy resynced differences
57,779 / 61,273 (were 76,157 / 83,906), travel +0.96% / +1.18% versus PrusaSlicer. The remaining
cube difference is the first grid-anchor connection: two boundary arcs of 2.6919 mm differ by
6 scaled units, decided by a 3-unit endpoint offset of the 225° sweep's corner line.

Update 2026-10-17 (infill-anchor tie). A harness built from the pinned upstream sources
(`FillRectilinear.cpp`/`FillBase.cpp`, `ClipperUtils.cpp`/`clipper.cpp`, Arachne `WallToolPaths`)
showed Mica's grid fill, boundary and anchors are bit-identical to upstream for the same surface;
the surface itself was off. Two upstream precision quirks, both now ported for Prusa profiles:
`TriangleMeshSlicer.cpp::slice_facet` adds the rounding half twice to every edge crossing
(`a·(1−t)+b·t+(0.5,0.5)` then `v3f_scaled_to_contour_point`'s `floor(v+0.5)`), so the cube
slices to 10000001 / −9999999; and Clipper 6.4's `FRound` rounds the half-unit offsets of
`process_arachne`'s `offset2_ex(…, -min_perimeter_infill_spacing / 2., inset + …/2.)` upwards
where Clipper2 rounds them away from zero. The infill square becomes 9225446 / −9225442
(was ±9225445), which black-box density probes of the unmodified reference also single out.
Prusa STL slicing now also uses the float object frame (`make_trafo_for_slicing`) and float
slice heights; the slicer quirk applies only there (CLI STL with the loaded mesh), while GUI/3MF
slices keep nearest rounding because their vertices are not PrusaSlicer's. CORE One / XL: cube20 62 / 69 positions in 2 / 3 layers (were 421 / 421), travel
+0.01% / +0.14% (were +13.1% / +11.5%); hole_block resynced 1,901 / 2,015 (were 3,490 /
3,846), travel +11.8% / +10.2% (were −12.4% / −19.7%); 3DBenchy resynced 56,772 / 58,869,
travel +0.74% / +1.05% (were +0.96% / +1.18%). U1 output is unchanged.

Update 2026-10-18 (layer extrusion order: `PolyTreeToExPolygons` order, exact Arachne
`PerimeterOrder`, island-by-island regional order, `infill_first`; see `docs/wipe-tower.md`).
CORE One / XL: 3DBenchy resynced 54,387 / 55,805 (were 56,772 / 58,869), travel +0.48% /
+0.74%; cube20 and hole_block unchanged. hole_block's remaining travel is the grid connection
topology on 24 layers, caused by Mica's Arachne inner contour, not by print order.

The cube Arachne baseline was 6,010 differing positions. On layers 3–93 the common
four differences are the first grid-anchor travel, its feed rate, and the first two
extrusions. Layers 6, 13, 27 and 55 also have two repeated Z words on cooling-split
extrusions in the reference which Mica omits. The grid now ends at the reference location, so the following layer-change
ramp starts at the correct point. Layer 94 still orders/starts the ensuring-fill ring
differently; layer 95 inherits its previous endpoint and differs in two ramp commands.
The layer-change source point, short-ramp feed rates and bridge fan restoration now match.

Hole-block also differs in first-layer geometry, solid/grid connections, arc fitting,
overhang speed segmentation and the resulting cooling/travel. Classic perimeter nesting
now emits the hole before its enclosing contour. The hole's ordinary Arachne perimeters
now follow the reference direction and use a projected seam rather than a snapped vertex.

Benchy differences include perimeter geometry and ordering, seam positions, line/arc
segmentation, solid/sparse fill geometry and connections, extrusion amounts, overhang
speeds, cooling, fan/acceleration boundaries, and travel/ramp endpoints. Applying classic
input simplification reduced its command count from 355,731 to 119,827; arc/line splitting
still differs. Counts are not evidence of
geometric equivalence or print-quality equivalence.

Port scope and review points:

- Fill direction and spacing use upstream float precision. Monotonic chaining consumes
  bounded random draws like libc++ and no longer skips the ant search for zero-length
  greedy solutions. Exact ordering remains sensitive to numerical and library behavior.
- Ensuring fill now reconstructs sweep coverage, filters vibrating runs, generates Arachne
  gap paths and orders touching paths. It retains Clipper2 geometry and linear proximity
  searches; gap reconnection is still simpler than upstream's mutable endpoint graph.
- Arachne walls use the object's coordinate frame. Classic keeps its existing frame.
  Classic simplifies closed input rings with Douglas–Peucker before offsetting, and
  nesting uses a tree but nearest-sibling chaining; thin walls remain separate.
  Upstream's complete ShortestPath multifragment/two-exchange ordering is not ported.
- Uniform Arachne loops use projected seams. Mixed-width runs still use vertex seams.
  Curl estimation reconstructs clipped uniform loops and preserves original winding;
  moving the entire overhang annotation before seam placement remains unfinished.
- Review geometry precision, gap-fill changes, direction/wipe handling on holes, and
  cooling/overhang behavior on non-cube models closely. No model-specific corrections,
  new dependencies, or increased comparator tolerances are used.

## Existing binary fixtures

`cube20_ps.bgcode` and `cube20_ps.gcode` were supplied for this port. PrusaSlicer 2.9.6
generated them in separate runs using the same 20 mm cube and CORE One HF0.4 / PLA
profiles. These are exports, not copies of upstream source files. The embedded
Prusa profile settings are used only as regression data.

| File | SHA-256 |
|---|---|
| `cube20_ps.bgcode` | `f05c31e72052afec3153ef36a06f68f06763c6f84b038ca9b702052e977fbd00` |
| `cube20_ps.gcode` | `6624ec31d593e130419f4c18e16f1a75d971030f0dbd7fb50f95bf87c1624a00` |

The binary fixture uses CRC32 for every block:

| Type | Compression | Encoded/uncompressed bytes | Stored payload bytes |
|---|---|---:|---:|
| File metadata | None | 66 | 66 |
| Printer metadata | None | 610 | 610 |
| Print metadata | Deflate (zlib) | 392 | 166 |
| Slicer metadata | Deflate (zlib) | 15237 | 4933 |
| G-code | Heatshrink 12/4 | 39761 | 9054 |
| G-code | Heatshrink 12/4 | 41553 | 7095 |
| G-code | Heatshrink 12/4 | 22780 | 7134 |

The tests compare all 167360 decoded text bytes independently of the MeatPack
implementation. They account explicitly for the generation header, seven ASCII-only
extrusion-width comments, twelve statistics/object metadata lines, the full 373-entry
config, inline comments and blank lines. The only config difference is `binary_gcode`;
generation times are 19:30:58 and 19:55:07 UTC on 2026-10-15. The custom
`; max_layer_z = 20` comment remains in G-code. No other differences are ignored.
Neither CLI fixture contains thumbnails; synthetic byte-preservation tests cover all
three thumbnail format tags separately.

Heatshrink re-encoding is byte-identical for every G-code block. Reconstructing the
original binary source text from the ASCII fixture also reproduces all three packed
and compressed payloads, including the 64 KiB source-cache boundaries.

Re-encoding only the decoded text cannot recover the original packed representation:
upstream MeatPack appends a newline to G lines even if one is present, whereas inline
comment stripping first removes that original newline. Its decoder discards duplicate
newlines. Repacking decoded blocks adds 11 packed bytes to the first block and one to
the third; the second is identical. This is explicitly tested and does not change the
decoded text. Inline comments and removed blank lines also affect source-cache block
boundaries. A decoded-text round trip therefore guarantees identical text and metadata,
not an identical whole file. Deflate output is format-compatible; byte identity with
Prusa's zlib encoder is not required or claimed.

`coreone_cube20_start.gcode` predates this port and is used by the machine G-code tests.

## Time-estimation oracles (2026-10-15)

The existing `cube20_ps.gcode` is also the always-on timing fixture. No new complete G-code
fixtures are checked in. `tests/time_estimate.rs` generates cube20 and 3DBenchy in the system
temporary directory with PrusaSlicer 2.9.6, CORE One HF0.4, 0.20mm SPEED and Prusament PLA.
Set `MICA_PRUSASLICER` and `MICA_PRUSA_DATADIR` to run this oracle on another installation;
it skips if the application or settings directory is absent. Expected numbers are stored in
the test, and estimated-time comments are explicitly excluded from estimator configuration.

| Prusa G-code | Prusa normal / silent | Mica estimator, seconds normal / silent | M73 exact placement and values |
|---|---|---|---|
| cube20 | 15m 23s / 16m 38s | 923.019 / 998.026 | 228 / 228 |
| 3DBenchy | 38m 28s / 44m 26s | 2308.991 / 2666.620 | 282 / 283 |

Both modes are within 0.05% of the displayed reference time. First-layer estimates are
34.701 / 35.335 seconds for cube20 (reference 35 / 35), and 57.477 / 59.328 for Benchy
(reference 57 / 59). One Benchy silent progress boundary differs by a move due to the small
numerical difference; the live test requires >99% exact M73 placement and values.

`mica-profiles/tests/u1_gcode.rs` also estimates the live OrcaSlicer 2.4.1 U1 cube export:
911.694 seconds versus Orca's 908 seconds (0.41%). The Mica estimator uses Prusa's first-layer
accounting, which includes startup motion; Orca reports preparation separately. The U1 run
uses 0.20 Standard, Snapmaker PLA and High Temp Plate as documented in `docs/u1-klipper.md`.

On Mica-generated toolpaths, CORE One cube20 is 15m 19s / 16m 16s and U1 cube20 is 15m 24s.
Mica's CORE One Benchy is 50m 4s / 59m 57s, significantly slower than Prusa's paths. The close
same-G-code oracle does not imply toolpath or cooling parity between the slicers.

The `estimate_time` example accepts external text G-code, an optional Prusa-style config INI,
and an optional output path. It streams the embedded config footer when no INI is supplied.
Orca external configs must first use Mica's canonical keys (`machine_max_feedrate_*`, etc.).
Normal `mica slice` needs no new arguments: text/binary metadata, M73, CLI statistics and Orca
inline time placeholders all use the postprocessed estimate.

The postprocess stage analyzes then rewinds the generated temporary G-code before rewriting
it, so header totals and binary metadata are known before encoding. This is two pipeline
stages but three text traversals including generation. RAM is bounded by one line, 257 blocks
per mode (retaining 64 at each refresh) and fixed I/O buffers. Two temporary binary streams
hold the timeline and arc-to-physical-line mapping; they are removed on drop, including errors.

Port scope: Prusa trapezoids, reverse/forward planning, classic jerk, junction deviation,
axis limits, normal/silent modes, G0/G1/G2/G3, positioning/extrusion modes, G28/G92, machine
limit commands, flow/feed overrides and tool-change load/unload times (XL: 4.5 seconds).
Klipper uses Orca's velocity/acceleration/SCV handlers and corrected explicit-F arc handling.
PA is recorded but does not affect the upstream time calculation; ACCEL_TO_DECEL and
MINIMUM_CRUISE_RATIO are likewise not simulated by those handlers.

The pinned Prusa processor ignores G4 and heater wait durations; it models G28 as a move to
zero rather than a measured homing delay. Firmware macros remain opaque. No preview vertices,
producer-specific preview tags, preheat backtracing, pause/color-change countdowns (M73 C/D),
or firmware-level thermal simulation are included. Unlike upstream's two-block minimum,
Mica finalizes a one-block program and preserves explicit tool-change time with an empty queue.
Cooling still chooses speeds using its existing approximation; this port measures the resulting
G-code. Review numeric arc behavior, the intentional Prusa explicit-F arc conversion, first-layer
semantics and the postprocess rewind/API change when updating upstream.
