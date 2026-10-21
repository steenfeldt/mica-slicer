<!--
SPDX-License-Identifier: AGPL-3.0-only
Provenance: adapted
Source: prusaslicer@b028299c77 src/libslic3r/Support/TreeSupportCommon.hpp :: TreeSupportSettings
Source: prusaslicer@b028299c77 src/libslic3r/Support/TreeModelVolumes.cpp :: TreeModelVolumes
Source: orca@8500fcdcca src/libslic3r/Support/TreeSupportCommon.hpp :: TreeSupportSettings
Source: orca@8500fcdcca src/libslic3r/Support/TreeSupport3D.hpp :: SupportElementState
Source: orca@8500fcdcca src/libslic3r/Support/TreeModelVolumes.cpp :: calculateAvoidance
Changes: Foundation port inventory, rule differences, implementation limits and validation record.
Copyright (c) Prusa Research 2022 - 2023 Vojtěch Bubník, Pavel Mikuš, Oleksandra Iushchenko
Tree supports by Thomas Rahm, loosely based on CuraEngine tree supports.
Original source: https://github.com/ThomasRahm/CuraEngine
Copyright (c) 2021 Ultimaker B.V. CuraEngine is released under AGPLv3 or higher.
-->

# Organic tree port: foundations

Pathing is documented in [Organic tree pathing](tree3d-pathing.md) and drawing in
[Organic tree drawing](tree3d-drawing.md).

The `mica_core::support::tree3d` module holds the settings, volumes and element state the
pathing and drawing build on. The legacy `tree.rs`, `tree_common.rs`, `tree_volumes.rs` and
`organic.rs` are unchanged by it. Source pins were checked against the pinned checkouts.

## Port inventory

| Upstream source | Types and functions ported |
|---|---|
| Prusa `TreeSupportCommon.hpp/.cpp` | Every `TreeSupportMeshGroupSettings` field/default and its config constructor; every `TreeSupportSettings` field, constructor and grouping equality; `InterfacePreference`; `getRadius`, `recommendedMinRadius`; raft construction, `layer_z`, `layer_idx_ceil`, `layer_idx_floor`; circle resolution, strict-simple and tiny-area constants. |
| Prusa `TreeSupport.hpp` | `AreaIncreaseSettings`, all `SupportElementStateBits` and `SupportElementState` fields, `propagate_down`, result-position reset/query, `locked`, `getEffectiveDTT`, both state and element `support_element_radius` / `support_element_collision_radius` overloads, `SupportElement`, `SupportElements`. Rust also exposes `get_radius_state` and `get_collision_radius` convenience methods. The pins have no function named `getRadiusHeight`; its effective-height logic is `getEffectiveDTT`. |
| Prusa `TreeSupport.cpp` | `LineStatus`, `LineInformation`, `LineInformations`; per-generation zero-gap input corresponding to `group_meshes`. |
| Prusa `TreeModelVolumes.hpp/.cpp` | Constructor, `ceilRadius` (both variants), `getRadiusNextCeil`, `getCollision`, `get_collision_lower_bound_area`, `getCollisionHolefree`, `getAvoidance`, `getPlaceableAreas`, `getWallRestriction`, `calculateCollision`, `calculateCollisionHolefree`, `calculateAvoidance`, `calculatePlaceables`, `calculateWallRestrictions`, `precalculate`, `clear`, `clear_all_but_object_collision`; deterministic cache lookup/insert/extension and predecessor query. All six speed/destination avoidance variants and both wall clearances are retained. |
| Prusa/Orca `Flow.cpp`, `Slicing.cpp`; Orca `SupportParameters.hpp` | Support/interface width selection, float-to-scaled truncation and nozzle choice; contact-gap semantics feeding tree settings; inherited bottom interface counts; drawing double-wall area threshold. |

Distances are integer scaled coordinates (1 unit = 1 nm); branch-angle settings are
radians and tip density remains percent. Heights supplied by `TreeSlicingParameters`
are millimetres. The caller supplies the resolved slicing/raft grid and the
per-generation “any object has a zero top gap” flag. No mutable global flag is used.
Config mapping reuses `PrintSettings::from_config` and the canonical options used by
existing support parameters. Under Orca rules, preserved raw Orca options take
precedence; this includes organic options even if classic-tree values were mapped
to canonical names by the old importer.

## Orca foundation differences

This inventory comes from diffs of both pinned `TreeSupportCommon.hpp`,
`TreeModelVolumes.hpp/.cpp`, and Prusa `TreeSupport.hpp` against Orca
`TreeSupport3D.hpp`, plus the settings input in `TreeSupport3D.cpp` /
`SupportParameters.hpp`. Behavioral selections use `Rules::of(config)` and the
stored rules. The implementation comments identify each selection.

| Location | Orca difference and handling |
|---|---|
| Mesh settings constructor | Reads `resolution` instead of `gcode_resolution`; `support_threshold_angle`, `support_on_build_plate_only`, `support_object_xy_distance`, `support_interface_top_layers`, `support_interface_bottom_layers`, `support_interface_spacing`, `support_base_pattern_spacing`, `support_interface_pattern`, `support_base_pattern` replace the corresponding Prusa support options. XY is an absolute Orca distance rather than Prusa's float-or-percent of maximum external perimeter flow width. |
| Pattern types | Orca extends the base enum with Default, Lightning and Hollow, and the interface enum with RectilinearInterlaced and Grid. Local tree enums retain every value; Prusa ignores the Orca-only option names. |
| Organic branch options | Reads `tree_support_branch_diameter_organic`, `tree_support_branch_angle_organic`, `tree_support_branch_distance_organic`, `tree_support_angle_slow`, `tree_support_branch_diameter_angle`, `tree_support_tip_diameter`, `tree_support_top_rate`. Prusa reads `support_tree_*`. Classic Orca branch diameter/angle/distance do not override the organic names. Tip diameter is clamped to the branch diameter in both rules. |
| Flow inputs | Orca uses `support_line_width` / `line_width`, `support_filament`, `support_interface_filament`; tool zero selects the first nozzle. Prusa uses support material/extrusion width and support material/interface extruders; tool zero selects the smallest nozzle. Both preserve `Flow`'s float width before integer scaling. |
| Gap inputs | Orca's `support_top_z_distance` and `support_bottom_z_distance` follow its asymmetric zero-interface rules; with `independent_support_layer_height=0`, gaps are rounded to object layers with the EPSILON bias from `Slicing.cpp`. Prusa zero bottom distance inherits top distance, and zero top distance makes both zero. |
| Bottom interfaces | Orca resolves negative bottom interface count to top count before determining floor enable/layers. Prusa does that only for `support_bottom_height`; its floor stays disabled for negative bottom count. Both gates for bottom enable still require top interfaces. |
| First-layer clearance | Orca adds `support_xy_distance_1st_layer`, sourced from `support_object_first_layer_gap`. Retained as Orca metadata; upstream volumes do not use it. |
| Walls | Orca sets mesh `support_wall_count=max(1,tree_support_wall_count)`, Prusa hardcodes 1. Its separate `SupportParameters` drawing threshold is count 0: circle of diameter 5 mm, count 1: maximum double, count >1: 0.1 scaled area units. Prusa uses `support_tree_branch_diameter_double_wall`, with zero disabling the threshold. Stored alongside mesh settings for the drawing stage. |
| `group_meshes` | Orca renames `soluble` to `zero_top_z_gap` and resets it each generation. Both mean any object's top gap is below EPSILON. Explicit per-pass state replaces both statics; minimum XY is raised to 0.1 mm when the flag is set. This also prevents Prusa's cross-job static leakage. |
| Raft constructor | Orca computes `ceil((remaining-EPSILON)/max_support_layer_height)`; Prusa omits the bias. |
| Layer-index helpers | Orca binary searches stored raft/contact/intermediate Zs below the first object layer, with EPSILON tolerance. Prusa clamps those heights to the first object layer. |
| Interface classification | Orca decrements the top-interface-only threshold when it is positive and top base-interface layers exist. `roof_is_interface` carries this switch for pathing's `InterfacePlacer`; polygon placement happens there. |
| Element metadata | Orca adds `type`, millimetre `radius`, float `print_z`, and initializes `missing_roof_layers=0`. Its `roof_recovery_dtt`, `set_pending_roof_recovery`, `has_pending_roof_recovery` and extra propagation behavior are retained. Propagation decrements pending recovery count and advances depth only under Orca rules. Rust gives all new states defined defaults under both rules. |
| Model volumes constructor | Orca retains `m_bed_area` for later drawing. `bed_area` is stored only under Orca rules. Neither upstream enables machine-border collision. |
| Avoidance stepping | Orca returns early for `move_step<EPSILON`; Prusa lacks the check. The Orca rule has an explicit guard. Returning without a cache entry would make upstream's lazy getter recurse forever, so this invalid request panics with a clear message instead. Zero movement with a positive step uses one zero-distance union under both rules instead of Prusa's debug assertion/release undefined behavior. |
| Non-geometric source changes | Orca moves common constructors and the error reporter inline into the header, changes the namespace and include/TBB organization, moves circle resolution and `LineStatus`, makes `tiny_area_threshold` a function and radius constants macros, removes the enabled/style constructor asserts, lowers cache-miss logging from error to debug, permits an empty cancel callback, and simplifies equivalent optional-return / `else` spelling. These do not change polygons and need no duplicate Rust algorithms. Rust cancellation is an explicit fallible closure; diagnostic/UI error reporting and allocator wrappers are not ported. |

The radius formulas, geometric collision/avoidance/placeable/wall recurrences and
radius buckets are otherwise identical at these pins.

## Deliberate representation changes and upstream quirks

- Polygon operations use Mica's `clip::{union_ex,diff_ex,intersection_ex,
  offset_ex_with,simplify_ex}`. Offsets preserve the upstream join, 1.2 miter limit,
  and round-join arc tolerance equal to resolution. The closest simplifier is
  Clipper2 `SimplifyPath`, rather than Prusa's Douglas-Peucker / Clipper6 cleanup;
  exact polygon vertex parity with upstream is not claimed. Analytical test shapes
  disable simplification and account for miter-limit bevels and integer vertices.
- `BTreeMap`/`BTreeSet` replace mutex-protected per-layer maps and unordered job
  schedules; owned polygons and stable per-layer parent indices replace the C++
  allocators. Precalculation and lazy evaluation are serial in this foundation
  step. This does not omit any cache variant. Future parallel scheduling can
  partition radius jobs without changing keys or construction order.
- Both active upstream constructors process one object. Their scene aggregation,
  additional excluded areas, and machine-border generation are disabled by
  `#if 0` / the early return upstream; this module does not invent those behaviors.
- Both `calculateCollision` versions test `outline_idx == layer_outline_indices.size()`
  for `processing_last_mesh`, which cannot hold for an in-range index. The port
  preserves its consequence: blockers are not merged into collision and the final
  collision simplification does not run. Blockers *are* subtracted from model
  landing surfaces, as upstream does independently of that condition.
- Both mesh constructors read `min_feature_size.value` rather than resolving its
  percentage. The port preserves that raw numeric value (25% yields scaled 25).
- A result position uses `Option<Point>` instead of the maximum-coordinate sentinel.
  New element state is fully initialized. Progress/UI logging is not imported.
  Cancellation is checked between precalculation jobs; individual lazy jobs run to
  completion. An additional `precalculate_with_cancel` API returns cancellation.

## Validation

The tests include CORE One and U1 organic option fixtures; derived movement,
radius growth, tip height, bed widening, spacing and interfaces; DTT and fractional
foot growth; frozen collision radius; raw/normal/minimum radius buckets; raft Z
rounding/inversion; floor and roof recovery switches; tool flow and double-wall
thresholds; a slab/pillar with both Z gaps; all avoidance modes and model landing;
a narrow enclosed hole; wall restrictions; cancellation and cache clearing; and
lazy/precalculated/request-order equivalence. Each analytical polygon expectation
states its upstream formula in a comment. The closed-ring fixture catches the
wall tunneling that a single large negative offset would permit.

Final targeted foundation run: **15 passed, 0 failed**. `cargo fmt --all` and
`cargo fmt --all --check` passed. `cargo clippy --offline --workspace --all-targets
-- -D warnings` passed. `MICA_PROFILE=fast cargo xtask provenance` passed (526
source files checked, 316 ported/adapted). `MICA_PROFILE=fast cargo xtask corpus
--quick` passed all 14 applicable cases, with zero validation errors.

The final CLI was built with `cargo build --offline --profile fast -p mica-cli`.
Each final slice was compared byte-for-byte with its pre-change slice, checked
against the expected SHA-256 prefix, and validated with `mica validate --json`
using its matching printer/process/filament options. All four were identical and
had zero validation errors (the pre-existing layer/bridge warnings remain).

| Profile/model | Unchanged SHA-256 |
|---|---|
| CORE One cube20 | `65f08d88a671a5e55b7f4a657f10c39c86aa2bbfb74edcbad2fe91ddb58167f3` |
| CORE One 3DBenchy | `728711a7e069fe22700d07f5892c52f3f5efe8adbff6f2d6d8164978fac92a41` |
| U1 cube20 | `02c2be3ad7de748cdfed4adb6ff943ca6207089d97546d56a520948c1c4d6522` |
| U1 3DBenchy | `e13da618c421a557609e62eb9f1b87f25989fe8d45f7a0e32939693457602efd` |

In a restricted build environment (no loopback sockets, no file-system events), the socket tests
of `mica-camera` and the file-event test of `mica-library` cannot pass; every other unit,
integration and documentation target passed.

## Ported-From trailers

```text
Ported-From: prusaslicer@b028299c77 src/libslic3r/Support/TreeSupportCommon.hpp
Ported-From: prusaslicer@b028299c77 src/libslic3r/Support/TreeSupportCommon.cpp
Ported-From: prusaslicer@b028299c77 src/libslic3r/Support/TreeSupport.hpp
Ported-From: prusaslicer@b028299c77 src/libslic3r/Support/TreeSupport.cpp
Ported-From: prusaslicer@b028299c77 src/libslic3r/Support/TreeModelVolumes.hpp
Ported-From: prusaslicer@b028299c77 src/libslic3r/Support/TreeModelVolumes.cpp
Ported-From: prusaslicer@b028299c77 src/libslic3r/Flow.cpp
Ported-From: prusaslicer@b028299c77 src/libslic3r/Slicing.cpp
Ported-From: orca@8500fcdcca src/libslic3r/Support/TreeSupportCommon.hpp
Ported-From: orca@8500fcdcca src/libslic3r/Support/TreeSupport3D.hpp
Ported-From: orca@8500fcdcca src/libslic3r/Support/TreeSupport3D.cpp
Ported-From: orca@8500fcdcca src/libslic3r/Support/TreeModelVolumes.hpp
Ported-From: orca@8500fcdcca src/libslic3r/Support/TreeModelVolumes.cpp
Ported-From: orca@8500fcdcca src/libslic3r/Support/SupportParameters.hpp
Ported-From: orca@8500fcdcca src/libslic3r/PrintConfig.hpp
Ported-From: orca@8500fcdcca src/libslic3r/Flow.cpp
Ported-From: orca@8500fcdcca src/libslic3r/Slicing.cpp
```
