# 3MF geometry import

`mica_core::threemf::load_3mf(path)` and `read_3mf(Read + Seek)` return a `Project`.
The importer streams ZIP entries through quick-xml without extracting files or holding XML documents
in memory. It imports core meshes, nested components, build instances, all six core units, package
relationships and Production `p:path` references. Resource IDs are scoped by model-file path.

`Project.objects` contains build-referenced objects, in first build occurrence order. Each object has
named, typed parts, raw setting overrides, structural metadata, and build instances. Repeated build
references share an object; Prusa's extra single-component instance aliases are collapsed too. Orca
objects with separate metadata stay distinct even when they reference the same mesh resource.
Unreferenced resource objects do not become printable objects.

Part meshes are in **object coordinates, in millimetres**. `Part.transform` records the accumulated
component transform already baked into the mesh: **do not apply it twice**. `Instance.transform`
places that mesh in the build coordinate system. Mirroring preserves outward triangle winding.
Slicer metadata `matrix` values remain in `Part.metadata`; they describe source/reload transforms
and must not be applied again. `settings` contains raw overrides; `metadata` contains names, type,
matrix and source fields. The importer itself does not apply settings. The print-job adapter applies object/part settings, tools, modifiers, negatives and support controls; see [regions](regions.md) and [multi-tool printing](multitool.md).

Prusa's inclusive `volume firstid/lastid` ranges split a resource into compact part meshes.
`Part.painting` retains sparse support, seam, material and fuzzy-skin TriangleSelector streams.
Split trees and source winding survive volume extraction, component mirrors and instance transforms;
[painting.md](painting.md) describes their slicing APIs and validation. Orca's
`part id/subtype` associates metadata with component references; its older range representation is
also supported. Both type vocabularies cover model parts, negatives, modifiers and support
enforcers/blockers. Unknown types survive as `PartKind::Unknown` and are excluded from slicing.

`Object::placed_model_mesh()` merges model parts at all printable instances of that object.
`Project::placed_model_mesh()` merges all such objects. Neither helper re-centres or drops objects
to Z=0. `printable="false"` instances remain in the project but are excluded by these helpers.
Orca plate IDs are retained on each instance, with index-based membership in `Project.plates`.
An empty plate list means the archive has no explicit plate metadata. A `None` instance plate means
unassigned, not necessarily plate 1. Project config is retained as its original INI/JSON text.

The parser checks XML completeness, finite coordinates, triangle indices, duplicate resource IDs,
missing references, cycles, nonsingular transforms and complete/nonoverlapping triangle ranges.
Relative, absolute and percent-escaped package paths (including Orca literal `#` filenames) resolve within the ZIP; external model URLs
are rejected. Relationship targets and explicit component paths are both followed (a missing `.rels`
file is tolerated when `p:path` is sufficient). It does not repair topology; it normalizes signed
volume/winding and compacts referenced vertices. Expansion is limited to 128 component levels and
100,000 parts per object.

## CLI

```sh
mica slice project.3mf -o /tmp/plate1.gcode
mica slice project.3mf --plate 2 -o /tmp/plate2.gcode
mica slice project.3mf --all-plates -o /tmp/global-layout.gcode
mica slice project.3mf --each-plate -o /tmp/plates/
mica slice project.3mf --first-object -o /tmp/first.gcode
```

For a multi-plate 3MF, the default selects **plate 1**. `--plate N` selects the one-based
position in the archive's plate list, preserving all printable instances assigned to that plate and
excluding unassigned instances. With no explicit plate list, plate 1 means all printable instances;
single-plate inputs retain the previous default behavior. `--plate`, `--all-plates` and
`--each-plate` are mutually exclusive; zero and out-of-range plate numbers are errors.

`--each-plate -o DIR` slices every plate on its own (as `--plate N` would) and writes one file per
plate into `DIR`, in the printer's format (`.bgcode` for Prusa binary G-code), named as the GUI's
**Export all plates…** names them: the first object and the plate's name, sanitized
(`cube20_plate_2.bgcode`, `benchy_gift___tom.gcode`), with `-2`, `-3`… for repeated names. The
files are byte-identical to the GUI's (`cli_parity_tests::each_plate_*`).

All selected printable objects and instances are sliced independently, retaining relative placement
and object/part tool metadata. `--all-objects` remains an explicit alias; `--first-object` selects the
first printable build object on the selected plate. Object/tool override indices remain the original build-object indices. Parts of each instance share a region pipeline, including ordered Boolean clipping,
modifier intersections and height ranges. Equal settings share a region-table entry. The CLI reports
unknown settings/types and unsupported features. Facet painting is applied; see [painting.md](painting.md). Printer/process/filament presets still come from CLI selection. Explicit
`--center x,y` translates the whole selected layout to that centre and Z=0. See [multitool.md](multitool.md)
for multiple STL inputs and one-based CLI tool overrides.

The selected plate is translated from Orca's global grid to bed-local coordinates (square grid,
20% bed-size gap, rows along negative Y). The profile mapper reads the original archive's
`printable_area` for grid spacing, falling back to the selected printer's bed dimensions when absent.
`--center` then centres the selected layout, including modifiers and support volumes. Tower placement
is selected by the same plate index when the archive provides it.

`--all-plates` explicitly retains the old behavior: all printable instances, including unassigned
ones, are combined in **one file at their global placement**. It does not concatenate separate print
jobs, move every plate onto the same bed or bypass printable-volume validation. The cat project's
global layout therefore still fails that validation. To produce printable files for every plate,
run `--plate 1` through `--plate N` with separate output names.

The API's existing `input::load` keeps its all-plate behavior. `input::load_with_plate` accepts
`PlateSelection::{Auto, Plate(usize), All, Each}`; `input::prepare_objects` prepares independent instances
in one Rayon pool and returns them in input order. `Each` is the GUI's selection: every instance is kept,
each plate is moved to its own bed-local coordinates, unassigned instances join the nearest plate, and
`InputObject::plate` indexes `Loaded::plates` (`LoadedPlate`: name, `locked`, Prusa tower, raw plate
metadata); `Loaded::config` carries the archive's raw project settings for the GUI's project card. The
loader no longer lists the embedded printer/process/filament settings as ignored; the CLI prints its own
note that it does not apply them. `SettingsMapper::tool_count` lets a caller resolve object tools against
the project's own filament count (the GUI, with a warning) instead of failing on a printer with fewer
tools; the CLI keeps the strict check. See [gui-project-import.md](gui-project-import.md).

Negative volumes subtract material; modifiers split regional settings; support blockers
and enforcers feed normal/tree contact detection. Supports must still be enabled for enforcer volumes.

## Geometry-import validation, 2026-10-15

Subsequent setting/modifier slicing validation is recorded in [regions.md](regions.md).

The private projects (`MICA_PRIVATE_MODELS`) are read in place; none are copied or extracted into the
repository. PrusaSlicer 2.9.6 `--info` was
run on every oracle. Volume units below are mm³. Mica's totals include printable model instances at
build placement. `--info` reports Orca modifier meshes separately; its `number_of_parts` counts mesh
components, not slicer metadata volumes.

| File | Mica objects | Parts (volumes) | Explicit plates | Model triangles | Mica model volume | Prusa model volume |
|---|---:|---:|---:|---:|---:|---:|
| `dense-painted.3mf` | 1 | 1 | 1 | 1,998,378 | 38,542.494836 | 38,541.828125 |
| `cat.3mf` | 20 | 20 | 5 | 1,462,512 | 801,200.409245 | 801,204.857909 |
| `holder.3mf` | 1 | 2 | 1 | 19,840 | 358,178.756816 | 358,177.343750 |
| `five-plates.3mf` | 5 | 5 | 5 | 87,440 | 59,255.838096 | 59,255.811523 |
| Prusa-exported `hole_block.3mf` | 1 | 1 | 0 | 272 | 3,498.152228 | 3,498.155029 |
| Snapmaker upstream `3DBenchy.3mf` | 1 | 1 | 0 | 225,154 | 15,550.532575 | 15,550.378906 |

Every imported part's triangle count matches Prusa's count. Per-part volume comparisons pass at a
0.003% relative tolerance (the largest observed model difference is under 0.002%). Mica sums in
f64, Prusa reports float volumes, and mesh transforms are rounded to f32. Dense-painted is not closed:
Prusa reports 81 open edges and 44 connected mesh components, so its volume is also sensitive to
coordinate origin. Mica deliberately leaves its topology unchanged.

The holder's second part is `Generic-Cube`, a **modifier**: 12 triangles, 11,968.909995 mm³ in object
coordinates; Prusa reports it separately at 11,968.921875 mm³. It is excluded from Mica's model totals
and the model mesh. Its volume and overrides now feed the region pipeline; the original geometry-import
measurements below remain unchanged. See the [region slicing results](regions.md#oracle-results-2026-10-16).

The cat project's per-object volumes below are before build placement (component transforms are
already applied); each object has one model part and one instance:

| Object IDs | Name | Plate assignments, in ID order | Volume per object |
|---|---|---|---:|
| 2, 4, 11, 17, 22, 23, 28, 29 | `CAT.stl_4` | 1, 2, 2, 2, 4, 2, 5, unassigned | 42,081.516911 |
| 6, 8, 14, 20, 25, 26, 31, 32 | `CAT.stl_1` | 1, 2, 2, 2, 4, 2, 5, unassigned | 56,472.683232 |
| 10 | `CAT.stl_2` | 3 | 3,107.986150 |
| 13 | unnamed (`Object 13`) | 3 | 3,212.199469 |
| 16 | unnamed (`Object 16`) | 3 | 3,226.861749 |
| 19 | unnamed (`Object 19`) | 3 | 3,219.773976 |

Five-plates has one part/instance per object: `part-a.stl` (ID 2, plate 3, 11,780.598958 mm³),
`part-b.stl` (4, plate 4, 12,167.715751), `part-c.stl` (6, plate 2, 12,411.229945),
`part-d.stl` (8, plate 5, 11,505.888806), and `part-e.stl` (10, plate 1, 11,390.401509).
Dense-painted contains one part (ID 2, plate 1). Its object-coordinate volume is
945.164115 mm³; build scaling gives the placed volume in the table.

**Benchy asset discrepancy:** the pinned 3MF and repository STL are not identical tessellations.
The STL has 225,102 triangles and Mica volume 15,550.735132 mm³; the 3MF has 52 more triangles and
0.00130% less volume. Prusa independently confirms both triangle counts and slightly different
bounding dimensions. The regression test records both counts and compares volume within 0.01%;
it does not discard triangles to force equality.

Prusa export used:

```sh
PrusaSlicer --datadir out/ref/datadir \
  --export-3mf -o /tmp/mica-threemf-oracle/hole_block.3mf tests/models/hole_block.stl
PrusaSlicer --datadir out/ref/datadir \
  --info /tmp/mica-threemf-oracle/hole_block.3mf
```

Prusa's CLI exposes no modifier-add option; generated XML/ZIP fixtures exercise all modifier types,
settings and Prusa volume splitting instead. Hand-made tests additionally cover every unit, nested
noncommuting transforms, mirroring, printable flags, repeated instances, Production paths and local
IDs, alternate root relationships, raw config retention and malformed input. Optional local oracle
tests skip absent files; `MICA_PRUSA_3MF` enables a generated Prusa export oracle.

`cargo fmt`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`
(192 tests) and `cargo xtask provenance` passed. Profile tests need the pinned profiles from
`scripts/fetch-upstream.sh --profiles`.

The CLI smoke fixture has model cubes at (40,60) and (100,120), plus a displaced modifier. First-object
extrusion stays at X=40.225–43.775, Y=60.225–63.775. Merged extrusion also includes
X=100.225–103.775, Y=120.225–123.775; no modifier extrusion appears. Outputs are ordinary text G-code.

Reproduce inspection without slicing:

```sh
cargo run --offline --release -p mica-core --example inspect_3mf -- /path/to/project.3mf
MICA_PRUSA_3MF=/tmp/mica-threemf-oracle/hole_block.3mf \
  cargo test --offline -p mica-core --test threemf_oracles -- --nocapture
```

## Deliberate subset and review points

The parsing/assembly behavior is adapted from the pinned Prusa `_3MF_Importer` and Orca
`_BBS_3MF_Importer`, with volume type names from their `ModelVolume::type_from_string`. The Rust
project API, XML adapter, CLI integration and generated fixtures are original. `PROVENANCE.md`
registers all three adapted modules; `THIRD_PARTY.md` records ZIP/XML dependencies.

A part's editable emboss source (`<slic3rpe:text>` / `<slic3rpe:shape>`, PrusaSlicer's and Orca's
text and SVG tool) is kept as `slic3rpe:text:*` / `slic3rpe:shape:*` part metadata, the SVG file
named by `filepath3mf` included, so the GUI's Text tool re-opens it.
Thumbnails and plate pictures are not read (Mica draws its own when it saves, so they are not
reported). Cut relationships are not kept: the cut parts load as separate objects, and the import
says so in an informational note rather than a project warning. Adaptive layer-height profiles,
core color/material assignments and custom G-code are not interpreted. Explicit height-range settings
and their piecewise constant layer heights are applied. Detected
features are reported. Embedded project config remains raw, including custom G-code contained there.
Project writing is documented in [3mf-export.md](3mf-export.md). No topology repair, source reload, automatic arrangement or complete configuration migration is implemented.
The region pipeline performs 2D slice booleans; it does not repair or rewrite the source meshes. Part/object transforms and source matrices, Prusa alias
normalization, instance plate indexing, and the explicit Benchy tolerance deserve close review.
