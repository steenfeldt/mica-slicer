# ColorMix / Full Spectrum (mixed colors)

Phase 1 (engine, 3MF, CLI) comes first; phase 2 (patterns, the GUI, the color model) is in
"Phase 2: mixed colors in the app" below.

## Phase 1

Mica slices PrusaSlicer 2.9.6 virtual-extruder projects from the CLI without additional flags:
`MICA_CONFIG_DIR=$(mktemp -d) ./target/release/mica slice project.3mf -o print.gcode`.
A project with embedded printer/process/filament settings uses those settings normally. A geometry-only
fixture needs the usual XL profile selection. No physical filament slots are added.

## API and implementation

- `mica_core::full_spectrum::{Component, VirtualExtruder, FullSpectrum}`: one-based IDs, optional
  stored display colour, up to three physical components, normalized ratios and cached cycles.
  `FullSpectrum::new` / `from_json` validate recipes; `VirtualExtruder::build_sequence` ports the
  upstream count quantisation, GCD reduction and strict `>` deficit tie-breaking. `resolve(id, layer)`
  selects the physical ID; `expand` discovers a recipe's physical tools.
- `Project::full_spectrum_json: Option<String>` retains the original UTF-8 JSON, including whitespace,
  physical colours and unknown fields. `threemf::{load_3mf, read_3mf, write}` read/write
  `Metadata/Prusa_Slicer_full_spectrum.json` in either export dialect. Import matches its name
  case-insensitively. Saving does not parse or regenerate it, including unsupported future metadata.
- `ToolAssignments::resolve_with_virtuals` accepts defined model-role virtual IDs. The existing
  `resolve` retains its physical-only contract. Support assignments and the CLI's default physical
  tool keep their existing physical-tool constraints. Undefined IDs and gaps between defined IDs
  remain errors. Painting uses `segmentation::segment_with_virtuals` with the same membership check.
- `RegionModel::full_spectrum` carries definitions from `input::load_with_plate`. Painting is
  segmented into physical plus defined virtual channels, then virtual masks are appended to physical
  channels using the **object-local layer index**, before part clipping and modifiers.
- Whole-volume logical configurations remain distinct until all volume/modifier Boolean operations
  finish. An intermediate region's settings use a physical component, while its configuration retains
  the virtual source. A final pass interns each layer's physical targets and discards the entire
  intermediate table. Thus ordering, supports, wipe towers, flow resolution and G-code see physical
  region assignments only. The upstream source is selected by `perimeter_extruder`; its physical
  target receives all three model roles, including when another role had an explicit physical setting.
- Tool discovery and variable-height profile limits expand recipes into their physical components.
  Instance sharing also compares definitions, preventing different recipes from sharing paths.

Engine work is in `crates/mica-core/src/full_spectrum.rs`, with focused hooks in `input.rs`,
`print_job.rs`, `regions/{mod,schedule}.rs`, `painting/segmentation.rs`, `instances.rs` and
`threemf/{mod,json,parse,write}.rs`. The core module is registered in `PROVENANCE.md`. Existing
explicit `RegionModel` literals gained an empty default; the GUI scene adapter carries that field
through its existing transformation. There are no GUI features, exporter changes, new crates,
third-party data or changes to pinned upstreams.

## Port boundary

Ported from `prusaslicer@b028299c77`, primarily
`src/libslic3r/Feature/FullSpectrum/VirtualExtruder.{cpp,hpp}`: blend definitions and normalization,
count quantisation (candidate 2..64, 0.03 maximum component error), GCD reduction, deficit interleave,
object-local phase, physical expansion, painted-channel and whole-region reassignment.
50/50 is `AB`; 75/25 is `AABA`; 66/33 is `ABA`; equal triples are `ABC`.

Mica creates physical targets lazily after logical clipping instead of upstream's pre-created empty
painted regions. It uses its existing polygon union and region/wall/fill grouping for coincident
physical targets, rather than copying upstream's region allocation and `closing_ex(10*EPSILON)`
merge machinery. The six oracles cover ordinary adjoining regions; micron-scale gaps are not an
oracle claim. Valid physical settings are resolved separately for each resulting tool.

Malformed/duplicate/out-of-range definitions produce clear errors instead of Prusa's logging and
silent filtering. Negative ratios are dropped and nonunit positive sums normalized as upstream does;
all-zero weights use its first-component fallback. JSON preservation is independent of slice support.

Left out: gradient execution (explicit error when slicing; its JSON still round-trips), swatch colour
prediction/prusa-fdm-mixer, presets, colour matching, GUI editing/painting palette/save integration,
and changing printers with colliding virtual IDs (`remap_full_spectrum_on_import`). Virtual IDs
colliding with selected physical slots are rejected. Single-role virtual infill assignments without a
virtual perimeter source are outside the whole-region path; use a volume/object `extruder` assignment.
No Snapmaker-specific mixing, XY dithering, Local-Z or per-wall pattern behavior is included.

## Oracle and corpus

`python3 tests/oracles/colormix.py` generates projects with `tests/models/colormix.py`, obtains
settings from the reference PrusaSlicer CLI, slices both engines and validates Mica's outputs.
It uses XL 5T Input Shaper 0.4, `0.20mm SPEED @XLIS 0.4`, and `Prusament PLA @XLIS`, with the wipe
tower enabled. Mica gets **only the 3MF and output path**. Reference artifacts and extracted
vendor settings remain in the system temporary directory.

Prusa's CLI saves `wiping_volumes_matrix = 0` for this five-tool preset. Its `WipeTower::prime`
indexes the square matrix even with custom volumes disabled; an initial run crashed there.
The harness supplies the complete 5×5 matrix (diagonal 0, other entries 140), with custom volumes
still disabled. These are generated test settings, not a Mica default change.

The painted cube has one watertight mesh with a horizontal 4.2..12.2 mm band assigned to virtual 6;
its start is intentionally not a multiple of the two-layer cycle. The adjoining case has two
20 mm parts in one object, one on virtual 6 and the other on physical 1. The mixed/plain case has
two separate objects on virtual 6 and physical 4. Startup priming is excluded from the per-layer
sequence, while tower extrusion is included. Filament comparison also checks model-only extrusion.

| Generated case | Layers compared | Sequence mismatches | Maximum model filament difference per tool |
|---|---:|---:|---:|
| 50/50 cube | 100 | 0 | 0.31% |
| 75/25 cube | 100 | 0 | 0.62% |
| Equal triple cube | 100 | 0 | 0.49% |
| Painted virtual band | 100 | 0 | 0.00% |
| Mixed object + plain object | 100 | 0 | 0.31% |
| Mix adjoining its own physical tool | 100 | 0 | 0.10% |

`tests/corpus/manifest.json` adds all six as quick XL cases. The generator is deterministic and
contains no vendor profiles. `references.json` stores validated Prusa numeric summaries and per-layer
tool-sequence hashes. The corpus checks those hashes, required physical tool count, tower presence,
exact tool-change count and per-tool total filament within 3%, in addition to existing role gates.

`crates/mica-core/tests/full_spectrum.rs` exercises known cycles and tie order, object and volume
assignments, paint states 6 and 17, entirely physical prepared region tables, both export dialects,
verbatim JSON/paint/assignment preservation, invalid IDs, missing definitions and bad recipes.

## Upstream caller inventory

The complete search was `rg -n 'VirtualExtruder|full_spectrum|virtual extruder' upstream/prusaslicer/src`.
The matching files, excluding the definition/implementation itself, are:

| Area | Files under upstream `src/` and responsibilities |
|---|---|
| Engine model/validation | `libslic3r/Model.hpp` (storage/serialization), `Print.hpp` (definitions, physical/total counts), `Print.cpp` (`Print::object_extruders` expansion), `PrintObject.cpp` (`clamp_exturder_to_default`, `region_config_from_model_volume`), `PrintRegion.cpp` (`collect_object_printing_extruders`, virtual-aware validation) |
| Apply/slice | `libslic3r/PrintApply.cpp` (`Print::apply` filtering/invalidation, source-aware region identity, physical target creation, painted-tool expansion); `PrintObjectSlice.cpp` (`apply_mm_segmentation`, `PrintObject::slice` remap hooks); `MultiMaterialSegmentation.cpp` (channel count including virtual IDs) |
| Project I/O | `libslic3r/Format/3mf.{cpp,hpp}` (extract/serialize full-spectrum metadata), `libslic3r/FileReader.hpp` (import-result configuration) |
| Build | `libslic3r/CMakeLists.txt`, `slic3r/CMakeLists.txt` |
| Dialog and palette | `slic3r/GUI/FullSpectrumDialog.{cpp,hpp}`, `Widgets/VirtualExtruderListBox.{cpp,hpp}`, `wxExtensions.{cpp,hpp}` |
| Sidebar and visibility | `slic3r/GUI/Sidebar.{cpp,hpp}`, `FrequentlyChangedParameters.{cpp,hpp}` |
| Import and display | `slic3r/GUI/Plater.cpp`, `3DScene.cpp`, `GLCanvas3D.cpp`, `ObjectDataViewModel.cpp`, `GUI_ObjectList.cpp` |
| Painting UI | `slic3r/GUI/Gizmos/GLGizmoMmuSegmentation.cpp` |

The downstream ToolOrdering, wipe-tower and G-code implementations do not need a virtual-tool API:
the two slice hooks resolve all content before reaching them.

## Review and verification

Review logical source identity through modifiers, physical flow/settings selection, painted-mask merge
precedence, phase at a band's first occupied layer, instance sharing, and the intentionally stricter
malformed-definition behavior. Heterogeneous nozzles, gradients, printer migrations and GUI workflows
are outside these homogeneous-XL oracles. Hardware print quality is untested.

Reference/Mica artifacts: `$TMPDIR/mica-colormix-tv1afj__` (`results.json`, six reference outputs,
six Mica outputs, validation JSON, and before/after default outputs). All six ColorMix outputs
and all six defaults validate with **0 errors**. ColorMix totals including the tower differ by at
most 0.28% per tool; model-only differences are shown above.

Default output SHA-256 prefixes, confirmed by `cmp` before/after:

| Profile | cube20 | 3DBenchy |
|---|---|---|
| CORE One | `65f08d88` | `98eba153` |
| XL | `cc06b5d7` | `90fc179d` |
| U1 | `2ca74917` | `2c1e3b32` |

`cargo fmt --all --check`, workspace clippy with `-D warnings`, and `cargo xtask provenance` pass.
`cargo xtask corpus --quick` passes all 20 cases, including all six new ColorMix cases;
every output has 0 validation errors. Its report is `out/corpus-report.md`.
In a restricted build environment (no loopback sockets, no file-system events), the socket tests
of `mica-camera` and `mica-update` and the file-event test of `mica-library` cannot pass; the
rest of the workspace passes.

All engine, CLI, G-code, profiles, GUI and other workspace targets pass, including 476 core unit
tests, the new five ColorMix integration tests and 421 GUI tests. These unrelated tests were not
modified or disabled. The new ColorMix tests and workspace clippy were run again after the final
ordinary-project fast-path adjustment.

## Phase 2: mixed colors in the app

The design: one engine and one resolver for every printer, Prusa's exact cycle
as the base, and one **Pattern** per mixed color (Layers / Outer wall first / Gradient) on all
printers, without new global settings. Snapmaker's quirks (rounded ratios, cadence restarting
per island) are not copied. Left out: Local-Z sub-layers, 4-color dithering, bias/indentation.

### Patterns (engine: `mica_core::full_spectrum`, shared by the CLI and the GUI)

`FullSpectrum::roles(id, layer, span)` returns the layer's physical tools for the **walls**
(`perimeter_extruder`) and the **inside** (`infill_extruder`, `solid_infill_extruder`).
`RegionModel::slice`'s final pass writes them into each mixed region's settings; tool ordering,
the tower and G-code only ever see physical tools, as in phase 1.

| Pattern | Rule | Example (A = 75 %, B = 25 %) |
|---|---|---|
| Layers | PrusaSlicer's exact cycle, whole region per layer (phase 1, unchanged) | `A A B A` |
| Outer wall first | With a main color (one component with the most layers in the cycle of length L), both sequences span 2L layers and every other color moves half of its wall layers inside; per component, wall + inside layers = 2 × its cycle count, so the share stays exact. Without a main color (50/50, 33/33/33) the walls follow the cycle and the inside the cycle shifted to differ on as many layers as possible | walls `A A A A B A A A`, inside `A B A A B A B A` (12 of 16 slots A = 75 %); 50/50: walls `A B`, inside `B A` |
| Gradient | Shares move linearly from the start ratios (bottom) to the end ratios (top) over the layers the mix covers in each object (first to last layer of its regions, painted or assigned). Each layer takes the component furthest behind its accumulated target (ties to the first), so every prefix of the span is within one layer of the ideal | 80/20 → 20/80 over 10 layers: `A A B A B A B A B B` |

The idea of Outer wall first is Snapmaker Orca's per-perimeter pattern groups
(`MixedFilamentManager::resolve_perimeter`: outer wall = group 0, infill follows the last
group); Mica derives both groups from the exact cycle instead of a hand-written pattern. Its
granularity is PrusaSlicer's role tools: all walls (outer and inner) form the "walls" slot, infill
and solid infill (tops and bottoms included) the "inside" slot; the share is exact in layer
slots, not in grams (walls and infill differ in volume). Painted Layers mixes are still resolved
before the region booleans exactly as upstream; painted Outer wall first and Gradient mixes stay
virtual regions until the final pass.

### Project file

`FullSpectrum::to_json` writes PrusaSlicer's `Metadata/Prusa_Slicer_full_spectrum.json`
(`version` 1, `physical_extruders` with the tool colors, `kind` `fullspectrum`, the start
ratios, the predicted `color`). Mica's own fields go in a `mica` object per entry: `name`,
`pattern` (`outer_wall_first` / `gradient`; absent = Layers) and `end_ratios`. PrusaSlicer's
reader (`deserialize_virtual_extruders_from_json`, nlohmann `value`/`contains`) ignores unknown
keys, and no entry carries `position` or `kind: gradient`, so PrusaSlicer opens every mix as
plain layer mixing at its start ratio. Mica → Mica is identical: the GUI keeps the opened text
and writes it back verbatim while the list (with fresh predicted colors) is unchanged; otherwise
`to_json` regenerates it, and `from_json(to_json(x)) == x`.

### GUI (`crates/mica-gui/src/mixes.rs`)

- **Filaments card, "Mixed colors" section** under the physical tools (printers with two or
  more tools): one row per mix (predicted swatch, a gradient's two ends as a vertical blend,
  `M1`, the name, recipe and pattern), edit, move up/down, delete; `+` adds one.
- **Editor**: 2–3 loaded tools (A/B/C), share sliders that keep 100 % and snap on release to
  the shares the exact cycle prints (70/30 becomes 71/29, cycle `A B A A A B A`); the cycle
  text; pattern chips; for a gradient a second slider row ("at the top"); for Outer wall first
  the walls and inside sequences; the predicted color and its nearest color name (the hex only
  in the tooltip); a name field (empty: the nearest color name, or `Mix N` when an earlier mix
  has that name).
- **Notes in the editor**: the tool changes per layer of the mix alone (last-tool-first rule)
  and roughly the added time per 10 mm of height (printer tool change time, else the filaments'
  load + unload time, plus a short prime; 15 s without either); a hint when layers are thicker
  than 0.15 mm; an info note naming objects that use the mix and have flat tops or bottoms
  (they print one tool per layer and look striped).
- **Numbers**: mixes are tools N+1, N+2… after the N physical tools, exactly as PrusaSlicer
  numbers virtual extruders. Deleting one moves the later ones down and remaps every object and
  part assignment, layer range and painted facet (`FacetStates::map_states`); the deleted mix's
  own parts and painting go to its main color (a notice says so). Moving swaps two numbers the
  same way. Each is one undo step (`Snapshot::mixes`); edits re-slice every plate.
- **Everywhere a tool is chosen** (`App::tool_choices`): the object list's tool chip (`M1`
  and the swatch), its menu and the context menu's Change tool, the object card's tool combo,
  the paint card's chips (painting with a mix), and number fields of object/part tool settings.
- **Colors**: the 3D view draws objects and painted facets in the predicted mix color (a
  gradient's middle); the G-code preview by Filament shows the real per-layer tools.
- **Slicing**: `App::slice_job` puts the mixes on the job's copies of the objects
  (`Object::mixes`, part of the slice cache key); with mixes every object goes through the
  regions model, as the CLI does for a project with mixes. Opening a project loads its mixes;
  Save writes them in both flavours.

### Color model (`mica_core::fdm_mixer`)

A Rust port of prusa-fdm-mixer (MIT, github.com/prusa3d/prusa-fdm-mixer `0124582`, the same code
PrusaSlicer bundles): Yule–Nielsen in linear RGB plus fitted lightness, chroma and cyan-hue
corrections. Upstream's 33 C++ checks (`cpp/test_prusa_fdm_mixer.cpp`: hex and LAB round trips,
ΔE2000, hex parsing, endpoints, seven reference predictions, three-color mixes) are its unit
tests. It is only a prediction; it never affects G-code.

### Tests

- `crates/mica-core/tests/color_mix_patterns.rs`: Outer wall first cycles and the exact share
  over eight recipes; gradient prefix and window shares over the height; Layers = Prusa cycles;
  renumbering on delete and swap, including painted states; JSON round trip (identical text) and
  the PrusaSlicer view (the `mica` object stripped: plain Layers with the same ratios); sliced
  regions of a whole and a painted cube for every pattern.
- `crates/mica-gui/src/mixes.rs`: slider snapping and sums, drafts, red + yellow predicts
  orange, tool changes per layer, flat-surface detection, and an XL editor session: delete
  remaps assignments and painting, undo restores both, swap, the slice job carries the mixes.
- `cli_parity_tests::mixed_colors_saved_for_the_cli_on_the_{xl,u1}`: an Outer wall first and a
  gradient mix on two objects, GUI export = CLI slice of the saved project byte for byte, both
  physical tools print, the reopened project has the same mixes and assignments.
- Phase 1's PrusaSlicer oracle cases (corpus) and the reference hashes are unchanged: without
  mixes nothing changes, and the Layers pattern is the phase 1 resolver.

Screenshots (temp `MICA_CONFIG_DIR`; `--mix SPEC` adds a mix, `--mix-editor N|new` opens the
editor, `--brush-height H` sets the Height brush):

```sh
B=./target/release/mica-gui; U1=(--printer 'Snapmaker U1 (0.4 nozzle)' --colour 1=#C81E23 --colour 2=#F5D728)
$B --screenshot out/work/mix-editor.png tests/models/3DBenchy.stl $U1 --mix 1:50,2:50 --mix 1:75,2:25/outer --mix-editor 2 --select 0 --focus
$B --screenshot out/work/mix-paint.png tests/models/3DBenchy.stl $U1 --mix 1:50,2:50 --select 0 --focus --paint colour --paint-tool 5 --brush height --brush-height 10 --drag 540,518,600,520 --save /tmp/benchy-mix.3mf
$B --screenshot out/work/mix-preview.png /tmp/benchy-mix.3mf $U1 --select 0 --focus --preview
```

Open: Snapmaker project import (their `mixed_filament_definitions` → exact-ratio mixes), color
match (target color → recipe), mixes in the G-code preview legend, a part-row tool chip in the
object list (parts take a mix through their tool setting), and remapping mixes when the printer's
tool count changes (they then show a "does not fit this printer" note and print with the last
tool).
