<!--
SPDX-License-Identifier: AGPL-3.0-only
Provenance: adapted
Source: prusaslicer@b028299c77 src/libslic3r/Support/TreeSupport.cpp :: generate_initial_areas, create_layer_pathing, create_nodes_from_area
Source: orca@8500fcdcca src/libslic3r/Support/TreeSupport3D.cpp :: RichInterfacePlacer, increase_single_area, merge_support_element_states
Changes: Function inventory, rule audit, representation substitutions and analytical validation derivations.
Copyright (c) Prusa Research 2022 - 2023 Vojtěch Bubník, Lukáš Matěna, Tomáš Mészáros
Tree supports by Thomas Rahm, loosely based on CuraEngine tree supports.
Original source: https://github.com/ThomasRahm/CuraEngine
Copyright (c) 2021 Ultimaker B.V. CuraEngine is released under AGPLv3 or higher.
-->

# Organic tree port: pathing

The drawing stage consumes this API; see [Organic tree drawing](tree3d-drawing.md) and
[tree-supports.md](tree-supports.md) for how it is wired in. The statements below record the
pathing baseline as first delivered, before it was wired.

`mica_core::support::tree3d::create_tree_pathing` consumes the foundation settings and
model volumes, overhang polygons on the combined raft/object grid, resolved
`TreeSupportInfill` parameters and optional `TreeRaftContact`. It returns
`TreePathing { move_bounds, roofs }`. `move_bounds` is a vector of per-layer
`SupportElements`, carrying parent indices, full reachable influence polygons,
state and placed `result_on_layer` points after `create_nodes_from_area`.
`roofs` retains contact, interface and base-interface polygons for the drawing stage.

The existing support generator, slicer and G-code paths do not call this module.
No drawing, extrusion generation, dependencies or CLI changes are included.
The [foundation audit](tree3d-foundations.md) remains applicable.

## Function inventory

| File | Upstream functions / classes |
|---|---|
| `tips.rs` | `generate_initial_areas`, `sample_overhang_area`, `RichInterfacePlacer` and its `add_points_along_lines` / `add_point_as_influence_area`, `convert_lines_to_internal`, `evaluate_point_for_next_layer_function`, `split_lines`, `polyline_sample_next_point_at_distance`, `ensure_maximum_distance_polyline`, `generate_support_infill_lines`. The `InterfacePlacer` roof storage/classification and active `generate_raft_contact` / `finalize_raft_contact` behavior are represented by `TreeRoofs` and explicit raft inputs. |
| `geometry.rs` | `safe_union`, `safe_offset_inc`, the active zero-distance/unlimited-distance use of `move_inside` / `move_inside_if_outside`, `make_circle` / `make_circle_num_segments`. |
| `pathing.rs` | `increase_single_area`, `increase_areas_one_layer`, `SupportElementInfluenceAreas`, `SupportElementMerging`, `merge_support_element_states`, `merge_influence_areas_two_elements`, `merge_influence_areas_leaves`, `merge_influence_areas_two_sets`, `merge_influence_areas`, `create_layer_pathing`, `set_points_on_areas`, `set_to_model_contact_simple`, `set_to_model_contact_to_model_gracious`, `remove_deleted_elements`, `create_nodes_from_area`. Spatial ordering ports `AABBTreeIndirect::build_recursive` / `partition_input`. |

`area_increase_order` factors out the original ordered attempt construction;
`create_tree_pathing` composes the three upstream stages. Neither pinned source
has an `extend_infill_lines` function. Disabled `#if 0` tip-insertion lag, alternative
Cura infill, roof-generation checks and raft closing are not enabled by the port.
Overhang detection is an input stage, outside this step's function inventory.

The spreading attempt order retains the previous successful settings shortcut,
slow/fast-safe/fast hole escape, grow/freeze variants, normal/minimum XY pairs,
emergency overspeed and locked-tip priority. Radius catch-up follows radius
buckets, fractional elephant-foot growth and destination revalidation. Successful
movement clears `dont_move_until`; stationary tips retain their preset positions.

Merges retain the gracious/XY compatibility checks, real-radius engulfing offset,
model-radius-increase cap, minimum model-branch depth, safe-radius compatibility,
tiny-area check and -0.025 mm overlap erosion. Parent concatenation and source
compaction follow the original loops. The full collision-free reachable area is
carried down independently of bed/model avoidance subsets. Model placement walks
single-parent chains to the highest placeable element, removes the lower chain,
and remaps parent indices after swap removal.

## Orca difference audit

The complete tips-through-nodes ranges were diffed between the two pins.

| Difference | Rule selection |
|---|---|
| Infill density | Roof sampling reads `interface_density` for Prusa and `top_interface_density` for Orca. The resolved input can carry distinct values, including Orca's ironing-induced solid top. |
| Forced tip interfaces | Prusa uses `soluble_interface`; Orca uses `zero_gap_interface_top`. Both still test tip circle area against minimum roof area. |
| Released roof-tip metadata | Orca records the current recovery depth (`dtt_roof_tip + supports_roof_layers`) when an unplaceable roof tip is released. Prusa retains its original missing-roof count. Step-1 propagation decrements pending count/advances depth only for Orca. |
| Merge recovery | Prusa takes the minimum missing-roof count. Orca preserves the maximum pending count and recovery depth, using `set_pending_roof_recovery`. |
| Zero movement step | Both overspeed recovery and ordinary spreading use Orca's 0.1 mm fallback when the safe movement step is zero. Prusa's zero divisor is rejected explicitly. |
| Interface classification | Uses the foundation's `roof_is_interface`, including Orca's reservation when base-interface layers exist. Resolved `SupportParameters` counts differ: Prusa excludes top contact; Orca includes it. |
| Status helper | Orca extracts `get_avoidance_status`; the port shares that identical classification with Prusa's inline loop. |

Other differences in these ranges are constants moved to runtime storage,
`tiny_area_threshold()` spelling, logging severity, Eigen explicit types, comments,
formatting and includes. They do not need separate algorithms. Source debug/UI
logging and timing/progress instrumentation are omitted. The API is synchronous;
fallible `*_with_cancel` variants check cancellation between overhang parts,
spreading elements, merge buckets and node-placement layers. Individual Clipper2
operations and lazy volume jobs complete before the next cancellation check.

## Representation and geometry substitutions

- Distances and tolerances remain scaled integers (1 unit = 1 nm). The literal
  `5000`, `1000`, `3000` merge-frequency numerators are preserved as written at
  both pins; they are **not** rescaled to match the upstream comment's millimetres.
  Both pinned merge helpers compact pointer ranges without resizing their source
  vector, so the caller's size-based `reduced_by_merging` stays false. Owned Rust
  vectors compact away empty entries, while preserving that interval decision.
- Owned `ExPolygon` vectors replace C++ polygon allocators. Clipper2 booleans,
  round offsets with 0.01 mm arc tolerance, miter offsets with limit 1.2 and the
  foundation simplifier substitution remain in use. Recovery buffers use an explicit
  Clipper2 miter/butt adapter. Bounding-box collision trimming is omitted as a
  performance optimization; the complete collision region is subtracted.
- `SUPPORT_TREE_CIRCLE_RESOLUTION=25` is the circle's **scaled error tolerance**,
  not its vertex count. The 0.01 mm tip seed has
  `ceil(pi/acos(1-25/10000)) = 45` vertices. Roof-tip circles scale those discretized
  seed vertices as upstream does.
- Mica's coordinate storage is 64-bit. In particular, the closed-contour
  farthest-vertex sampler retains its squared-distance maximum in `i64` rather
  than upstream's 32-bit `coord_t`. Ordinary millimetre-scale squared distances
  overflow the latter's floating-to-integer conversion; that undefined native
  conversion is not emulated. The port follows the stated farthest-pair formula
  with a defined wider accumulator.
- Sampling uses Mica's existing `fill::fill` port and `FillParams` with the original
  spacing/density/angle/anchor settings. Exact upstream polygon vertex or fill
  traversal parity is not claimed across the Clipper6/Clipper2 boundary. The
  upstream roof angle expression's conditional precedence is preserved: it tests
  `interface_angle + layer_parity` as a boolean before selecting +/-45 degrees.
- `TreeSupportInfill` is the resolved `SupportParameters` subset, not the old
  support scheduler's parameters. Its convenience constructor supplies the common
  single-material, zero-angle, no-sheath defaults. Callers with other angles,
  sheath, multiple materials, ironing or special interface/base counts must supply
  the resolved fields explicitly. It computes float flow spacing from scaled
  widths; a caller can supply original resolved flow spacing without that scaling
  round trip. Contact-pattern configuration does not replace interface fill
  pattern here: upstream's tip sampler specifically uses `interface_fill_pattern`.
- Execution is serial and uses ordered duplicate-point sets and the foundation caches.
  Spatial ordering uses the upstream median-of-three partition, including ties.
  Merge buckets use the upstream **single-worker schedule**, independent of CPU
  count (four entries where possible, otherwise two). This fixes upstream's
  hardware-dependent choice while retaining leaf/set merge order. Parent indices
  are deterministic and refer to the layer immediately above.
- Only zero-distance inside projection is required by active callers; unused
  signed-boundary-distance and maximum-distance overloads are not exposed. Invalid
  zero offset divisors and sub-coordinate step requests have defined Rust checks
  or a one-unit minimum instead of undefined integer division. Negative initial
  expansion preserves the pinned function's no-growth behavior.
- Raft inputs carry the already resolved contact index, first tree layer, first
  object outline and expansion; allocation into classic support-layer objects
  happens in the drawing stage. Expanded-raft tip removal uses polygon containment instead
  of the upstream signed-distance grid (same interior test, possible boundary
  rounding differences).

## Analytical tests

`pathing_tests.rs` contains formula derivations alongside the assertions:

- Thin T arm: width 0.4 mm / rate 15% gives connection distance 2.666666 mm.
  The 0.2 mm arm cannot fit the 0.357 mm flow spacing; collision splits it into
  two short contours. Each contour's length is below twice the connection
  distance, producing one arc-length midpoint: **two tips**. Tiny round hanging
  blob and ledge sections each produce **one tip**, which reaches the bed.
- Equal-radius tips 1 mm apart: 0.01 mm seeds expand by 0.100005 mm per layer
  (0.1 mm slow speed plus upstream's 5-coordinate bias). After five layers overlap
  width is 0.02005 mm, removed by the -0.025 mm merge guard. After six it is
  0.22006 mm: **first merge on the sixth descending layer**.
- Radius: through a four-layer tip, `r(d)=0.2+0.4*d/4` mm, then
  `r(d)=0.6+(d-4)*0.02` mm. Collision-bucket catch-up stops at effective depth 3
  for DTT 4 due to the strict `effective+1 < DTT` condition. Merging 0.77 mm and
  0.68 mm radii with a 0.10 mm foot increment retains **0.9 fractional foot steps**.
- A broad platform ending at outline 3 has its first zero-gap landing surface at
  layer 4 (`outline[3] - outline[4]`, eroded by radius+XY). The branch lands there;
  buildplate-only classification rejects the same central tip. Failure to spread
  into the platform clears the gracious flag exactly as upstream does. A separate
  stepped-platform chain verifies highest gracious contact and lower-chain removal.
- Additional tests cover locked-tip release, normal/minimum XY attempt order,
  grow/freeze attempt variants, fallible cancellation, continuous tips, disabled ovalisation, pending
  roof recovery insertion/merge, roof layering, bed/model merge compatibility,
  full reachable area versus the smaller avoidance subset, ring-wall tunneling,
  projection around holes, swapped parent-index remapping, and nine-tip spatial
  buckets under Orca zero clearance. Repeated runs and reversed cache preparation
  produce identical complete element graphs.

## Validation

The final focused run passed **31 tests** (16 pathing tests plus the 15 foundation
fixtures), with zero failures. `cargo fmt --all`, `cargo fmt --all --check`,
`cargo clippy --offline --workspace --all-targets -- -D warnings` and
`MICA_PROFILE=fast cargo xtask provenance` passed (530 source files checked,
320 ported/adapted).

`MICA_PROFILE=fast cargo xtask corpus --quick` passed all 14 applicable cases,
with zero validation errors. The final CLI was built with
`cargo build --offline --profile fast -p mica-cli` through the corpus task.
The four final slices were compared byte-for-byte with pre-change slices and
validated with their matching printer/process/filament/bed settings. All were
identical and had zero validation errors; existing layer/bridge warnings remain.
The CORE One Benchy base at the time was `728711a7`.

| Profile/model | Unchanged SHA-256 |
|---|---|
| CORE One cube20 | `65f08d88a671a5e55b7f4a657f10c39c86aa2bbfb74edcbad2fe91ddb58167f3` |
| CORE One 3DBenchy | `728711a7e069fe22700d07f5892c52f3f5efe8adbff6f2d6d8164978fac92a41` |
| U1 cube20 | `02c2be3ad7de748cdfed4adb6ff943ca6207089d97546d56a520948c1c4d6522` |
| U1 3DBenchy | `e13da618c421a557609e62eb9f1b87f25989fe8d45f7a0e32939693457602efd` |

`cargo test --offline --workspace --no-fail-fast`: every unit, integration, GUI and
documentation target passed (core units 450, GUI units 372) except the socket tests of
`mica-camera` and the file-event test of `mica-library`, which cannot pass in a restricted
build environment (no loopback sockets, no file-system events).

## Ported-From trailers

```text
Ported-From: prusaslicer@b028299c77 src/libslic3r/Support/TreeSupport.cpp
Ported-From: prusaslicer@b028299c77 src/libslic3r/Support/TreeSupportCommon.hpp
Ported-From: prusaslicer@b028299c77 src/libslic3r/Support/SupportParameters.cpp
Ported-From: prusaslicer@b028299c77 src/libslic3r/Flow.cpp
Ported-From: prusaslicer@b028299c77 src/libslic3r/Polygon.cpp
Ported-From: prusaslicer@b028299c77 src/libslic3r/AABBTreeIndirect.hpp
Ported-From: orca@8500fcdcca src/libslic3r/Support/TreeSupport3D.cpp
Ported-From: orca@8500fcdcca src/libslic3r/Support/TreeSupportCommon.hpp
Ported-From: orca@8500fcdcca src/libslic3r/Support/SupportParameters.hpp
```
