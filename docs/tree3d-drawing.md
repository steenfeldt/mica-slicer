<!--
SPDX-License-Identifier: AGPL-3.0-only
Provenance: adapted
Source: prusaslicer@b028299c77 src/libslic3r/Support/OrganicSupport.cpp :: organic_draw_branches, extrude_branch, organic_smooth_branches_avoid_collisions
Source: prusaslicer@b028299c77 src/libslic3r/Support/SupportCommon.cpp :: generate_interface_layers
Source: prusaslicer@b028299c77 src/libslic3r/MutablePolygon.cpp :: smooth_outward
Source: orca@8500fcdcca src/libslic3r/Support/TreeSupport3D.cpp :: organic_draw_branches, generate_support_areas
Changes: Function inventory, output adapter, rule audit, validation results and retained validation gate.
Copyright (c) Prusa Research 2021 - 2023 Vojtěch Bubník, Lukáš Hejl, Lukáš Matěna
Tree supports by Thomas Rahm, loosely based on CuraEngine tree supports.
Copyright (c) 2021 Ultimaker B.V.
-->

# Organic tree drawing and wiring

> **Update 2026-10-18:** tree3d passed the PrusaSlicer 2.9.6 / OrcaSlicer 2.4.1 comparison
> and is now the only organic generator; `mica_tree_support_engine` and the legacy organic
> drawing were removed. See [tree-supports.md](tree-supports.md#organic-trees-tree3d-is-the-generator-2026-10-18)
> for the fixes, measurements and timings. The text below records the drawing stage as first delivered.

This completes the drawing stage after [foundations](tree3d-foundations.md) and
[pathing](tree3d-pathing.md). `mica_tree_support_engine=legacy` is the default.
`tree3d` selects the new generator for `support_material_style=organic`; the other
support styles keep their existing generators. No old modules have been deleted.

The option is in the shared typed option table, has print scope, and accepts only
`legacy` or `tree3d` at settings resolution. CLI overrides, GUI profile/project
loading, and GUI slicing all resolve the same `PrintSettings::tree3d` input.
The GUI retains the option but offers no editor page or object/modifier override
for it. The default is excluded from binary G-code metadata under both rules.

Example (CORE One defaults):

```sh
python3 tests/models/support_organic.py /tmp
cargo build --offline --profile fast -p mica-cli
target/fast/mica slice /tmp/support_figurine.stl -o /tmp/tree3d.gcode \
  --set support_material=1 --set support_material_auto=1 \
  --set support_material_style=organic --set mica_tree_support_engine=tree3d
```

The CORE One process has automatic support detection disabled: setting only
`support_material=1` enables enforcers-only supports. The explicit auto setting
above is required for the smoke tests to exercise automatic branches.

## Port inventory and output

| Rust file | Active upstream functions / behavior |
|---|---|
| `branch_mesh.rs` | `extrude_branch`, `discretize_circle`, `triangulate_fan`, `triangulate_strip`; rounded tube caps and averaged segment normals; Orca flat buildplate feet. |
| `nudge.rs` | `organic_smooth_branches_avoid_collisions`: collision cache boundaries, sphere layer spans, root/tip locks, deepest collision, 100 Jacobi iterations, collision nudge and Laplacian smoothing, Orca linked-layer limits. |
| `smoothing.rs` | `MutablePolygon::range`, duplicate removal, `smooth_outward`, `clip_narrow_corner`; outward shortcuts, narrow-crack traversal and interpolated final cuts, including clockwise holes. |
| `drawing.rs` | `organic_draw_branches`: tree/branch traversal, capped branch meshes sliced at layer mid-heights in Positive mode, per-branch collision subtraction, bottom landing/drop propagation, per-tree union, forest assembly, `smooth_outward(union_(base), support_line_width)`, `polygons_simplify`, top/bottom contact subtraction. |
| `pipeline.rs` | `generate_support_areas` adapter, threshold/blocker/enforcer overhang input, resolved `SupportParameters` counts and sampling inputs; active organic equivalent of `finalize_interface_and_support_areas`, followed by `SupportCommon::generate_interface_layers` compact intermediate-layer slabs and regularization. |
| `slice.rs` | Branch-only Positive mesh slicing with float-scaled XY and rule-specific crossing rounding, using the existing facet and edge-connectivity slicer. Ordinary object slicing retains its previous mode. |

At the pinned Prusa source, the active organic code is in `OrganicSupport.cpp`.
It does **not** call classic-tree `draw_areas` or that path's standalone
`finalize_interface_and_support_areas`. Their circle smoothing / floor inference
is not added to the organic path. The equivalent active organic tail is ported
from the end of `organic_draw_branches`; the adapter's finalizer additionally
performs the shared interface projection stage. Disabled alternative forest
smoothing, OpenVDB collision nudging and missing-roof slice recovery remain off.

`TreeDrawing` retains layer-indexed base, bottom-contact and `TreeRoofs` areas.
The adapter produces existing `SupportLayer` / `SupportRegion` objects with
absolute print Z, previous-layer bottom Z and physical height. It keeps top
contacts, bottom contacts, top/bottom interfaces, sparse base and base interfaces.
As upstream does, base interfaces have `Base` type. Precomputed organic roofs
are unioned with projected interfaces without clipping them to branch base: a
roof can span several tips. Intermediate slabs count allocated nonempty base
layers, rather than assuming every object layer contains a branch.

The existing common raft/flange generator, hollow tree toolpaths, material/tool
routing, schedule, path ordering and G-code extrusion export consume these areas.
Tip sampling retains the constructor's sheath/pattern selection before upstream's
late `with_sheath=true` override. The resolved double-wall area threshold is converted to the diameter accepted
by the existing hollow-path generator; its code is unchanged.

## Orca drawing switches

| Difference from Prusa | Rule-selected behavior |
|---|---|
| Bed root cap | A root at layer zero has a vertical flat foot starting at Z=0. |
| Smoothing drift | Each candidate remains reachable from upper/lower previous-iteration anchors, accounting for radius differences and slow movement. |
| Slice cleanup | Collision/bed booleans use safety offsets and discard tiny individual contours/holes. |
| Leading empty slices | Remove them before choosing the bottom-contact reference and advance the layer origin. Prusa skips landing generation when the untrimmed front is empty. |
| Gracious bottom contacts | Nonzero bottom gaps keep the footprint without clipping to placeables; retain fallback contact areas when clipping removes everything. |
| Nongracious drop | The first lower slice uses collision at the original landing layer; lowest extra footprint/contact fallback replaces Prusa's multiple placeable intersections. |
| Interface/base counts | Orca includes contact in configured interface counts; raw bottom counts, soluble/different-filament zero-gap base counts and ironing-induced top sampling density are resolved explicitly. |
| Final bed mask | Clip every region, including assembled interfaces and raft, to the bed after common layer assembly. |

## Representation differences and remaining gate

- Execution is deterministic and serial across tree branches. Collision queries
  scan per-layer boundary segments with bounding-box rejection instead of the
  upstream AABB index. This changes acceleration, not the closest-segment query; equal-distance tie
  order can differ from upstream AABB traversal.
  Lazy volume jobs replace eager precalculation; all cache variants remain available.
- Booleans/offsets and `polygons_simplify` use the established tree3d Clipper2
  substitutions. Branch slice Positive union uses Mica's existing Clipper 6.4
  wrapper. Exact vertex parity is not asserted across these geometry boundaries.
- Mica's existing facet/edge-connectivity slicer handles branch meshes. It retains
  its existing open-chain recovery in place of upstream's multi-stage repair.
  Branch meshes have float vertices; normals and vector arithmetic use Rust doubles
  between float storage points. Zero-length shifts are stationary, and tiny caps
  have at least two half-cap steps / three ring vertices instead of undefined
  upstream tessellation or uninitialized strips.
- The adapter uses the foundation's uniform `TreeSlicingParameters` grid, the
  existing single-region support flows/toolpath backend and common raft geometry.
  The zero-top-gap generation flag is resolved per object, because `SupportInput`
  does not carry the other objects in a plate.
  It consumes already-sliced masks and classified bridge exclusions. Orca's
  separate overhang morphology / sharp-tail detector is outside this drawing port;
  threshold detection here follows Prusa's organic input calculation.
- No source pins, dependencies, reference applications or defaults were changed.
  This is an opt-in implementation gate, not a parity or print-quality approval.
  The separate PrusaSlicer 2.9.6 / OrcaSlicer 2.4.1 comparison pass must decide
  whether to promote it. Keep `legacy` until that decision.

## Validation

The final CLI was built with `cargo build --offline --profile fast -p mica-cli`.
All six default slices were compared byte-for-byte with the pre-change binary,
matched the expected SHA-256 prefixes, and validated with 0 errors.

| Profile/model | Unchanged SHA-256 prefix |
|---|---|
| core / cube20 | `65f08d88` |
| core / 3DBenchy | `98eba153` |
| xl / cube20 | `cc06b5d7` |
| xl / 3DBenchy | `90fc179d` |
| u1 / cube20 | `02c2be3a` |
| u1 / 3DBenchy | `e13da618` |

The CORE One Benchy baseline is `98eba153`; the older `728711a7` recorded in
the pathing document is stale for this checkout.

All smoke slices completed and `mica validate --json` reported **0 errors**.
The U1 figurine is the corpus `organic_figure/u1` case. Both engines used the
manifest profiles, plus `support_material=1`, `support_material_auto=1`,
`support_material_style=organic` and their respective `mica_tree_support_engine`.
Support filament below is base + interface, excluding brim.

| Profile/model | Legacy support mm | Tree3d support mm | Legacy wall s | Tree3d wall s |
|---|---:|---:|---:|---:|
| core / support_figurine | 786.83 | 718.88 | 2.67 | 3.33 |
| core / 3DBenchy | 1667.12 | 1668.42 | 17.72 | 14.57 |
| u1 / support_figurine | 846.80 | 786.45 | 1.53 | 1.89 |

These are single end-to-end CLI wall measurements taken while workspace/corpus
checks were running, with uncontrolled cache and CPU load. They are smoke
measurements, not a performance baseline; no settings were tuned from them.

Two separate CLI slices of `support_arms.stl` with tree3d were byte-identical
(`b39f9941c7b11346433cf1174f910c7d22784a574767f6e155083e93f1e25548`);
both validated with 0 errors. The synthetic unit fixture separately checks
nonempty bed-reaching base, top contacts and interfaces under both rules,
including complete polygon equality across repeated runs.

Final `cargo fmt --all` / `--check`, `cargo clippy --offline --workspace
--all-targets -- -D warnings` and `MICA_PROFILE=fast cargo xtask provenance`
passed (539 source files checked, 328 ported/adapted).
`MICA_PROFILE=fast cargo xtask corpus` passed **33 applicable cases**, all with
0 validation errors; its complete **26-test CLI/GUI parity pass** also passed.
The private dense-painted fixture is skipped when absent; some other
corpus cases have no upstream numeric reference and are marked unmeasured.

The focused final tree3d suite passed **38 tests**. The complete workspace command
`cargo test --offline --workspace --profile fast --no-fail-fast` finished with
exactly two failed crate targets: `mica-camera --lib` and `mica-library --lib`.
Every other unit, integration and documentation target passed; core units passed
**458** (one existing ignored), GUI units passed **373** (four existing ignored).
The default debug workspace command was also run; its redundant passes were
stopped after the complete fast-profile result was available.

Those two targets hold the socket tests of `mica-camera` and the file-event test of
`mica-library`, which cannot pass in a restricted build environment (no loopback sockets, no
file-system events). The explicit
GUI hidden-option / shared settings test and default binary-metadata regression
both passed in the complete workspace run.

## Ported-From trailers

```text
Ported-From: prusaslicer@b028299c77 src/libslic3r/Support/OrganicSupport.cpp
Ported-From: prusaslicer@b028299c77 src/libslic3r/Support/TreeSupport.cpp
Ported-From: prusaslicer@b028299c77 src/libslic3r/Support/TreeSupportCommon.hpp
Ported-From: prusaslicer@b028299c77 src/libslic3r/Support/SupportCommon.cpp
Ported-From: prusaslicer@b028299c77 src/libslic3r/Support/SupportParameters.cpp
Ported-From: prusaslicer@b028299c77 src/libslic3r/MutablePolygon.cpp
Ported-From: prusaslicer@b028299c77 src/libslic3r/MutablePolygon.hpp
Ported-From: prusaslicer@b028299c77 src/libslic3r/TriangleMeshSlicer.cpp
Ported-From: orca@8500fcdcca src/libslic3r/Support/TreeSupport3D.cpp
Ported-From: orca@8500fcdcca src/libslic3r/Support/SupportParameters.hpp
Ported-From: orca@8500fcdcca src/libslic3r/TriangleMeshSlicer.cpp
```
