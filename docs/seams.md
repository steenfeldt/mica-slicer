# Seam placement

The engine adapts the PrusaSlicer 2.8+ seam system at `prusaslicer@b028299c77`.
Every derived source and test is registered in `PROVENANCE.md`.

## Engine and CLI

`PrintSettings::seam` contains `SeamSettings`, `SeamPosition` and `ScarfSettings`.
`prepare_object` builds an immutable `PreparedObject::seams` before releasing the mesh input.
`SeamPlan::prepare(PlanInput)` is also available independently. Alignment sees the whole object,
then discards its candidate chains, ray index and visibility samples. Aligned, rear and random
retain a small record per boundary per layer. Inner walls project the same boundary seam.
Only nearest retains classified boundary candidates, because it must choose using the current
position while generating toolpaths. Reordered or differently sized toolpath batches produce
identical results.

The existing `mica slice --set key=value` interface now consumes:

- `seam_position=aligned|nearest|rear|random`
- `staggered_inner_seams`
- `seam_gap_distance`, including negative overlap and percentages of nozzle diameter;
  a non-nil `filament_seam_gap_distance` overrides it for the first extruder
- `scarf_seam_placement=nowhere|contours|everywhere`, `scarf_seam_only_on_smooth`,
  `scarf_seam_start_height` (percent), `scarf_seam_entire_loop`, `scarf_seam_length` (mm),
  `scarf_seam_max_segment_length` (mm), and `scarf_seam_on_inner_perimeters`
- `wipe` and its non-nil `filament_wipe` override select Prusa's inward seam-hiding condition:
  inward travel after ordinary external loops when perimeter retraction wiping is disabled
  and there is more than one wall. This change does not implement retraction wiping itself.

Orca's `seam_gap` now maps to `seam_gap_distance`. Orca-specific scarf settings retain their
existing extras-only reports; their semantics are not silently equated to Prusa's options.

`try_prepare_object_with_seam_painting` takes a `SeamPainting` query in placed mesh coordinates.
`MeshPainting` accepts borrowed enforcer/blocker triangle meshes; blockers win overlaps.
Painted edges are sampled at 0.2 mm, classified within 0.05 mm, and respected by placement and
scarf stop distances. Decoding 3MF seam facets and exposing painting in a GUI remain future work.

Opened wall paths have `ExtrusionPath::seam` metadata. Ordinary seams use an empty sample array;
scarf paths have one height/extrusion sample per point, and optional inward wipes are travels.
The G-code hook preserves these samples instead of applying ordinary XY simplification. XYZ
extrusion also observes the configured Z travel speed. `--preview` marks loop starts in cyan;
it remains a 2D preview, not a visualization of scarf height.

## Upstream behavior and adaptations

- `SeamGeometry`, `SeamShells`: signed angles/normals, bounding-box shell correspondence,
  strongest predecessor at merges, new shells at splits, edge projection and loop opening.
- `SeamPerimeters`, `SeamChoice`, `SeamPainting`: two angle arm lengths and thresholds,
  enforcer/common/blocker priority, previous-layer overhang classification and painting queries.
- `SeamAligned`: complete candidate chains, mesh visibility plus corner modifiers, 1 mm detour,
  0.6 visibility jump threshold, continuity modifier 2, and upstream candidate-order ties.
- `SeamRear`, `SeamRandom`: rear-centre edge projection with 1 mm tolerance/20 mm offset,
  and seeded, length-uniform random selection including isolated painted points.
- `ModelVisibility` (Prusa profiles, `seam/model_visibility.rs`): an exact port. The STL mesh
  as loaded (`InputObject::seam_source`, PrusaSlicer's model frame) is re-indexed like admesh's
  import (`seam/shared_vertices.rs`), centred and un-centred like `ModelVolume`, decimated to
  16,000 faces by `its_short_edge_collpase` (`seam/decimate.rs`) and moved to the object frame
  of `PrintObject::trafo_centered`. Then 30,000 area samples, 5 x 5 hemisphere rays against the
  upstream balanced AABB tree, and KD-tree interpolation, all with upstream float arithmetic
  (Eigen 3.3 reduction order, Apple clang's contracted multiply-adds). A C++ harness built from
  the upstream sources gives bit-identical decimated meshes, samples, sample visibilities and
  point queries for cube20, hole_block and 3DBenchy at both bed placements. Without a loaded
  mesh (GUI, 3MF) the placed mesh is used as the model frame. Negative volumes are not
  ray-cast; the placed mesh must already represent the desired solid.
- Orca profiles (`seam_slope_type` present, `SeamSettings::prusa_parity` false) keep Mica's
  earlier model: full-mesh f64 visibility (`seam/visibility.rs`), simplify-first boundaries,
  same-hole shell matching and nearest-vertex Arachne splits. U1 output is unchanged.
- `SeamScarf`: boundary-projected endpoints, subdivided rising/falling ramps, inner-wall offsets,
  smooth/contour/first-layer gates, whole-loop scarf, and clipping. Height and E fractions are
  emitted as simultaneous XYZ extrusion. The inward wipe follows the upstream wedge, with
  guards for zero gaps and clockwise holes.

Review these remaining differences closely:

1. Perimeter planning uses single-region slice boundaries. It does not reconstruct Prusa's
   external extrusion collections (one perimeter per external Arachne extrusion, so thin walls
   can give upstream duplicate or missing boundaries), embedded internal-region points, explicit
   bridge-role intervals, or its special two/three-perimeter thin-ring rule. Overhangs are
   classified from the previous outlines (for Prusa profiles with upstream's order, angle test,
   per-run simplification, and the uncompensated first layer below layer 1) and scarves
   conservatively avoid them.
2. Visibility is exact for Prusa profiles (above); what remains are the inputs: Mica's slice
   polygons and Arachne walls are not vertex-identical to PrusaSlicer's, and the first-layer
   expansion uses Clipper2 instead of Clipper1 (no shortest-edge filter). Near-ties between
   mirrored corners (Benchy's symmetric holes) can therefore still pick the other twin.
3. Ordinary staggering follows the final wall's distance; combining staggering with inner
   scarves also uses wall-distance offsets. Unstaggered scarves project boundary endpoints.
4. Nearest uses Mica's per-layer toolpath position/order; its first loop uses Mica's existing
   layer-start position, not the previous layer's final firmware position. This preserves
   parallel, order-independent batches.
5. Random uses MT19937 with an explicit two-word real conversion. It is deterministic in Mica;
   C++ standard-library distribution implementations need not produce identical coordinates.
6. There is no arc fitting in the seam implementation. Scarf paths preserve their linear
   segments, including the flat section. No physical test print was performed.

## Visibility parity, 2026-10-17

Same reference runs as the move-level oracle (Arachne, CORE One centred at 125,110 and XL at
180,180). Counts are reference loop starts with a Mica start within 0.5 mm.

| Model, printer | Before (external / internal) | After (external / internal) |
|---|---:|---:|
| 3DBenchy, CORE One | 526/760, 544/720 | 714/760, 695/720 |
| 3DBenchy, XL | 526/760, 534/720 | 714/760, 695/720 |
| cube20, hole_block (both) | 100% | 100% (G-code unchanged) |

Of the 46 remaining external Benchy misses (CORE One), 41 are on open Arachne paths (thin walls) whose start
is decided by path ordering, not seam placement; the rest are layer 0 (first-layer expansion)
and two mirrored near-ties on the small stern hole (layers 23 and 32).

## Oracle, 2026-10-15

Reference: PrusaSlicer 2.9.6, CORE One HF0.4, `0.20mm SPEED @COREONE HF0.4`,
`Prusament PLA @COREONE HF0.4`, `--perimeter-generator classic`, centred at 125,110.
Other settings come from the same installed profiles. Results and previews stay in a system
temporary directory; no generated G-code or model is in the repository.

The extraction script records the position before the first external extrusion following travel,
tracks absolute/relative E, and measures G2/G3 arc length for selecting the largest loop. It writes
per-layer CSVs and JSON metrics. Distances below are **bidirectional nearest external starts on the
same layer**, without assuming print order. Counts match on every layer for all four models.

| Model | External starts (each slicer) | Median separation | 95th percentile | Maximum |
|---|---:|---:|---:|---:|
| cube20 | 100 | 0 mm | 0 mm | 0 mm |
| hole_block | 100 | 0.0005 mm | 0.003 mm | 0.003 mm |
| cylinder, radius 10 mm / height 20 mm / 128 facets | 100 | 0 mm | 0.003 mm | 0.009 mm |
| 3DBenchy | 745 | 0.0842 mm | 4.4413 mm | 46.2945 mm |

Cube layer 2 is **(134.775, 119.775)** in both generated files, the rear-right corner.
The often-quoted (134.55, 119.55) coordinate does not describe this reference run's 0.45 mm external
line centre. All 100 cube layers agree, including elephant-foot compensation on layer 1.

For the cylinder, excluding layer 1, RMS spread of seam positions is **0.00127 mm Mica /
0.00109 mm Prusa**. The largest adjacent-layer movement is 0.009 / 0.003 mm. Layer 2 is
(125.474, 100.237) in both files.

For Benchy's largest external loop per layer, median adjacent-layer movement is
**0.0338 mm Mica / 0.0295 mm Prusa**. Whole-object RMS position spread is 15.578 / 16.854 mm,
which also includes genuine geometry/topology changes and is not a pure continuity measure.
The corresponding largest-loop median distance between slicers is 0.1026 mm. Lower hull
choices and some shell transitions still differ markedly (layer 2 is at the stern in Mica and
near the bow in Prusa); this is the main remaining oracle discrepancy.

An additional cylinder scarf run used `scarf_seam_placement=everywhere`, start height 20%,
and inner scarves enabled. All 100 external start positions match, including layer 2 at
(115.927, 113.630). The first layer remains an ordinary seam. Tests separately check ramp
resolution, exact un-clipped material conservation, clipping metadata and simultaneous XYZ/E.

Visually inspected `--preview` layers: cube 2, hole block 25, Benchy 160 and scarf cylinder 2.
Cyan markers confirm the intended corner, independent hole seam, cabin corners and displaced
scarf start. All reference and preview artifacts are temporary.

## Reproduce

```sh
export PATH="/opt/homebrew/opt/rustup/bin:$PATH"
cargo build --offline --release -p mica-cli
mkdir -p /tmp/mica-seams-oracle
python3 - <<'PY'
import runpy
runpy.run_path('scripts/seam-oracle.py')['generate_cylinder']('/tmp/mica-seams-oracle/cylinder.stl')
PY

out/ref/PrusaSlicer-2.9.6.app/Contents/MacOS/PrusaSlicer \
  --datadir out/ref/datadir \
  --printer-profile 'Prusa CORE One HF0.4 nozzle' \
  --print-profile '0.20mm SPEED @COREONE HF0.4' \
  --material-profile 'Prusament PLA @COREONE HF0.4' \
  --center 125,110 --perimeter-generator classic --no-binary-gcode --export-gcode \
  -o /tmp/mica-seams-oracle/prusa-cube20.gcode tests/models/cube20.stl

./target/release/mica slice tests/models/cube20.stl \
  -o /tmp/mica-seams-oracle/mica-cube20.gcode \
  --preview /tmp/mica-seams-oracle/preview-cube --layers 2,50,99
python3 scripts/seam-oracle.py \
  /tmp/mica-seams-oracle/prusa-cube20.gcode /tmp/mica-seams-oracle/mica-cube20.gcode \
  --out /tmp/mica-seams-oracle/cube20-comparison
```

Repeat with `hole_block.stl`, `3DBenchy.stl`, and the temporary cylinder. The compact plan and
scarf/wipe modules have upstream-derived geometry, snapping, random-uniformity, mapping,
painted-edge and ramp tests, plus Mica integration regressions. Required verification:
`cargo fmt --all`, `cargo clippy --offline --workspace --all-targets -- -D warnings`,
`cargo test --offline --workspace`, and `cargo xtask provenance`. Profile tests need the pinned
profiles from `scripts/fetch-upstream.sh --profiles`.

Final verification passed: release CLI build, formatting, clippy with warnings denied, provenance
(99 files checked, 67 ported/adapted), and the full workspace suite (258 passed, 1 existing ignored).
This includes 23 core seam tests and 2 G-code seam tests.
