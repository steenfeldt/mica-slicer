# Saving 3MF projects

`mica_core::threemf::write(path, &Project, Flavour)` saves the editable project without
GUI code or new dependencies. It uses the existing `zip` 2.4.2 writer and a 64 KiB buffer.
Vertices, triangles and their painting attributes go directly into compressed ZIP entries;
there is no combined mesh allocation or full XML string. A temporary sibling file is finished
and synced before replacing the destination. Validation and write failures preserve the old file.

```rust,no_run
use mica_core::threemf::{self, Flavour};
let mut project = threemf::load_3mf("input.3mf")?;
project.objects[0].name = "Edited object".into();
threemf::write("orca.3mf", &project, Flavour::Orca)?;
threemf::write("prusa.3mf", &project, Flavour::Prusa)?;
# Ok::<(), threemf::ThreeMfError>(())
```

Use the existing `threemf::Project`, rather than reconstructing it from the slicing adapter's
`input::Loaded`: the latter has already filtered non-printable instances and flattened volumes.
`Project` retains object/part names, kinds, source identity, overrides, metadata, instance transforms,
painting, height ranges, plates, tower placement and the raw embedded configuration.
`Project::update_settings(&Settings)` overlays canonical option-text settings on the original
configuration. Edit `Project::wipe_towers` for model-owned tower positions; its keys are zero-based
plate indices and its values are `[x, y, rotation_degrees]`.

## CLI

```sh
mica slice project.3mf --project-settings use --export-3mf saved.3mf
mica slice project.3mf --project-settings use --export-3mf saved-prusa.3mf --flavour prusa
mica slice cube20.stl --export-3mf cube.3mf --set perimeters=4
mica slice project.3mf --project-settings use --export-3mf saved.3mf -o sliced.gcode
```

The default flavour is `orca`. `--export-3mf` alone saves without slicing; supplying `-o` also
slices using the selected profiles, embedded settings as described below, and plate selection. Export keeps the whole source
project, including unselected plates, unassigned objects and non-printable instances. `--plate`,
`--all-plates` and `--first-object` select what to **slice**. `--plate` also selects which saved
plate receives an explicit tower-position override. `--center` translates the entire saved layout
to the requested centre and Z=0; on multi-plate projects this is a global-layout operation.

When project settings are in use, export in the source's native flavour keeps their text
verbatim, plus only explicit `--set` / `--perimeter-generator` edits. `--object-extruder` changes
object metadata. Profile arguments do not replace native project settings that are in use.
When project settings are ignored, or for STL inputs, export composes the selected profiles
through the same writer as GUI Save. Export-only commands resolve profiles too and require
an installed bundle. Several inputs may be combined; conflicting embedded configurations
require `--project-settings ignore` or separate exports. Object numbers remain one-based.

`mica slice project.3mf --project-settings use|update|ignore` controls embedded printer/process/
filament settings during slicing and export. By default, settings apply when the project's printer matches
selected model and nozzle (including copied or renamed presets); otherwise the CLI keeps the
selected profiles and prints a note. `use` explicitly applies them against the selected printer.
The CLI shares preset matching and the project layer with the GUI: project process and
filament names resolve against compatible installed presets, with notes for missing names,
the presets are composed for the project's plate type, then the project's whole embedded
configuration (where it differs from them) and its colours apply, so the project slices as
saved (see [gui-project-import.md](gui-project-import.md), "Project values"); a note says
how many values differ from the current presets. `update` uses today's presets with only
the changes the project lists (Orca's `different_settings_to_system`; none for PrusaSlicer
projects), the former behaviour. An explicit `--print`, `--filament` or `--bed` that names
another preset than the project's wins for its scope: that scope's project values (the
filament values of each tool whose project filament differs; the bed temperatures) are not
applied. `--set` and `--perimeter-generator` apply last. Multiple projects with different embedded configurations
must be sliced separately or with `--project-settings ignore`. Save-only exports use the same
use/ignore decision as slicing.

Support write-back translates `support_material_auto` and `support_material_style` together to
Orca's `support_type` and `support_style`. Grid/snug become normal supports; organic becomes
Orca's default tree style; Mica tree becomes `tree_slim`. Orca's slim/strong/hybrid subtypes
collapse to Mica's single tree style on import; an untouched native configuration is preserved.
Editing only automatic/manual mode or only style retains the other setting from the project.

## Native layouts and Mica metadata

| Feature | Orca/Bambu | PrusaSlicer |
|---|---|---|
| Geometry | `3D/3dmodel.model` references `3D/Objects/object_N.model`; globally unique resource IDs | One merged mesh resource per object, plus component aliases for extra instances |
| Part metadata | `Metadata/model_settings.config`, `part id/subtype` | `Metadata/Slic3r_PE_model.config`, inclusive volume triangle ranges |
| Settings | `Metadata/project_settings.config` JSON | `Metadata/Slic3r_PE.config` INI |
| Height ranges | `Metadata/layer_config_ranges.xml` | `Metadata/Prusa_Slicer_layer_config_ranges.xml` |
| Painting | `paint_supports`, `paint_seam`, `paint_color`, `paint_fuzzy_skin` | Corresponding `slic3rpe:` attributes, including version metadata |
| Plates | Membership, names, locked flags and plate metadata | First plate printable on the native single bed; other instances retained as non-printable |
| Towers | Per-plate X/Y vectors; global rotation | First bed's `Prusa_Slicer_wipe_tower_information.xml` |

All five part kinds are written: model, modifier, negative, support enforcer and support blocker.
The engine's `Part.mesh` stays in f32 object coordinates for slicing. Alongside it,
`Part.source_mesh` retains the original f64 source vertices and component transform. Orca
exports write those source vertices and the native f64 component/build matrices. Source data
is used only while it still matches the editable mesh; mesh edits invalidate it automatically.
GUI saves retain this data through settings/painting edits, moves, and added modifiers, keeping
the original build matrix separate from the GUI's additional transform. Prusa exports retain
the existing merged object-coordinate layout. `Part.transform` is inspection/source metadata;
it is never applied a second time to `Part.mesh`. Painting trees are reoriented to outward-facing wire triangles, so native mesh repair cannot change
split-side meaning. Two-edge splits become equivalent one-edge cascades under reflection; all four
channels may have independent winding flags. A reoriented tree exceeding the importer's 64-level
limit is rejected. Standard mesh import can compact
vertex numbering; ordered triangle coordinates and painting remain unchanged.

`Metadata/mica.json` schema 1 records the Mica version, flavour and metadata that needs lossless
restoration: original settings/dialect, source IDs and transforms, structural metadata, all plates,
original printable flags and per-plate rotations. It contains **no mesh or painting**. When facet winding needs conversion,
`Metadata/mica_painting.bin` streams the exact original sparse codes for only the affected channels;
this preserves the original tree representation and fingerprint on Mica reload. A single backed-up
facet is bounded to 16 MiB; the whole painting is never buffered for writing.
A manifest of native entry CRCs and uncompressed sizes prevents applying it after another slicer
changes the native archive. Stale markers are ignored with an import note. Unknown marker schemas
and malformed markers are errors. The marker is bounded to 64 MiB on read.

The `Application=BambuStudio-2.4.1` and `OrcaSlicer=2.4.1` compatibility tags select the native
project importer in Orca and the older Snapmaker fork. This follows Orca's own legacy application
tag convention; `Generator` and `mica.json` identify Mica as the actual writer. Numeric resource IDs are unique across the whole package because
Orca's shared-mesh bookkeeping relies on this, beyond the Production extension's path-scoped IDs.

Native same-flavour settings retain unknown options and template text. Cross-flavour conversion
uses the existing Orca/Prusa option-name table, vector/string serialization and common reversible
enum mappings. It is **not a complete preset migration**: vendor-only enums, relative-width/speed
semantics, filament override composition, bed-dependent temperature choices and custom G-code
languages may have no identical native equivalent. Their original values survive in the marker;
review a cross-flavour project's settings before printing in the other application. Prusa's native
single-bed view cannot reproduce Orca's plate grid; Mica restores that grid and every instance.

Thumbnails and `plate_N.json` previews are optional and are omitted. Embossed text / SVG parts
write their source back as PrusaSlicer's `<slic3rpe:text …/>` / `<slic3rpe:shape …/>` elements in
the part's config entry (both dialects; Orca uses the same elements) and the SVG file at its
`filepath3mf`. Cached G-code, editable CAD
sources, adaptive layer-height profiles and other features the importer does not model are not
reconstructed. There is no source mesh reload, topology repair, arrangement or GUI integration.

## Validation, 2026-10-16

`cargo fmt --all`, offline workspace clippy with `-D warnings`, the release CLI build and
`cargo xtask provenance` pass (262 files checked, 160 ported/adapted). The offline workspace
suite passed: 607 tests, zero failures, three ignored. The U1 time oracle reused its saved Orca
reference (`MICA_U1_ORCA_REFERENCE`) instead of relaunching Orca after the crash. Writer and
G-code round-trip tests passed again after the final ZIP content-type declaration change.

Regression tests cover both repository fixtures, repeated saves in both flavours, all part kinds,
multiple/mirrored instances, split painting in all four channels, height ranges, escaped Unicode
metadata, plate/tower restoration, native loading with the marker removed, stale-marker behavior,
resource-ID uniqueness and destination preservation on validation or mid-write errors. Recursive
reorientation tests check all split sides and leaf geometry. A native Prusa re-export check initially exposed reversed split seam states swapping under mesh
repair. After reorienting the wire trees, the native re-export preserves the exact painted leaf
positions and states. The profile integration test
compares complete Mica G-code bytes for `painted-cube.3mf` and both plates of `plates-orca.3mf` after
each flavour's round trip. CLI tests cover full-project retention and explicit overrides.

The nine fixtures generated by `scripts/make-regions-oracle.py` also produced byte-identical Mica
G-code after both flavours' round trips. PrusaSlicer 2.9.6 loaded/re-exported written modifier,
painted and multi-plate projects. Its source/written Prusa region comparisons were identical:

| Fixture | Source and written filament, mm |
|---|---:|
| blocker | 556.13 |
| combine | 1533.18 |
| enforcer | 557.46 |
| height | 1378.90 |
| modifier | 1628.37 |
| negative | 1362.07 |
| object | 1415.37 |
| plain | 1264.99 |
| support | 725.95 |

OrcaSlicer 2.4.1 successfully sliced the initial written multi-plate fixture. Plate 2 matched exactly
(tool 3: 2088.22 mm). Plate 1 exposed repeated volume IDs across split files: tool 2 used 1563.61 mm
instead of 961.33 mm, while tools 1/3 matched at 840.96/356.66 mm. The writer now assigns globally
unique IDs, with a regression test. **That correction has not been rechecked in Orca:** a subsequent
painted-cube run segfaulted (signal 11) despite four explicit filament inputs. Orca was not launched
again. Prusa's XL painted-source slice also segfaulted; native painted multi-tool filament parity is
unverified. Snapmaker Orca has not been run separately. These are outstanding oracle checks, not
passing claims; the ±0.1% acceptance criterion is met for the nine Prusa region fixtures only.

Reference projects, logs and G-code stay in a system temporary directory; no reference outputs are
stored in this repository.
A synthetic 1,000,008-triangle save produced 38,334,494 bytes of uncompressed object XML and a
903-byte marker. Loading plus writing peaked at 52,936,704 bytes RSS (50.5 MiB), measured using
`getrusage(RUSAGE_CHILDREN)` on macOS. This checks serialization memory, not slicing memory.

### Reproduce

Build with `cargo build --offline --release -p mica-cli`. Use a fresh system temporary directory,
generate regions there, and export both flavours with the CLI above. Prusa comparisons used:

```sh
PrusaSlicer --datadir out/ref/datadir \
  --printer-profile 'Prusa CORE One HF0.4 nozzle' \
  --print-profile '0.20mm SPEED @COREONE HF0.4' \
  --material-profile 'Prusament PLA @COREONE HF0.4' \
  --no-binary-gcode --export-gcode -o "$ref/result.gcode" "$ref/project.3mf"
```

For Orca, prepare installed U1 profiles using
`python3 scripts/prepare-u1-oracle.py --output "$ref/u1"`. Run from the temporary directory,
using the actual source and corrected written paths in separate output directories:

```sh
/Applications/OrcaSlicer.app/Contents/MacOS/OrcaSlicer \
  --datadir "$ref/u1/data" \
  --load-settings "$ref/u1/machine.json;$ref/u1/process.json" \
  --load-filaments "$ref/u1/filament.json;$ref/u1/filament.json;$ref/u1/filament.json;$ref/u1/filament.json" \
  --slice 1 --outputdir "$ref/written-plate1" "$ref/plates-orca-orca.3mf"
```

Repeat with `--slice 2` for the second plate. The failed command used the same options with
`--slice 1` and `painted-cube-orca.3mf`. Compare each tool's `; filament used [mm]` entry. For painted
Prusa comparisons use the XL 5T / `0.20mm SPEED @XLIS 0.4` / `Prusament PLA @XLIS` profiles; the source
fixture's native slice failed here before a written/source comparison was possible.

## Saving from the GUI

File › **Save** (⌘S), **Save As…** (⇧⌘S). Save As asks for a name and the format,
*OrcaSlicer / Snapmaker project (.3mf)* or *PrusaSlicer project (.3mf)*, then the native file
picker. The format offered first is the one the project was opened from (or last saved as),
else Orca for Snapmaker printers and Prusa for Prusa printers. Saved files join *Open recent*.
The header shows the project name with a `•` while there are unsaved changes, and the window
title reads `• Name — Mica Slicer`; closing the window with unsaved changes asks *Save changes
to "Name"?* (Save / Don't save / Cancel), and so does opening another project over a modified
scene (a 3MF now **replaces** the scene, like *Open project* in Orca and PrusaSlicer; STLs are
still added to the current plate). A read-only project file is never overwritten:
⌘S turns into Save As.

What is saved (`crates/mica-gui/src/save.rs`, `App::build_project`): every plate (name,
`locked`, Orca's plate metadata, the tower position the plate slices with), each GUI object as a
project object, objects sharing an id as one object with several instances (sorted by
instance), the instance transform as the GUI's `Mat4` in f64 plus Orca's plate-grid origin
(`input::grid_origin` with the selected bed), model parts with their painting split by face
ranges, modifiers / negative / support volumes as parts in the object's frame (volume mesh ×
local transform), height ranges (sorted, clipped at 0), the object's overrides plus the tool keys
(`extruder` when walls and infills share a tool, else per role; support roles only when they
differ from the configuration's own), and the settings below. Objects beside their plate's bed
stay unassigned, as Orca keeps them. Unknown Orca spellings an import kept next to the canonical
value are dropped before translating (`mica_core::threemf::translate`, new), so an edited value
cannot be shadowed by its stale source spelling.

Settings are the **effective** configuration (presets + unsaved edits + the project's settings
when they are in use):

- A project opened from a native archive whose presets are still selected and whose listed
  changes are still applied (`Document::applied`) is saved with its own settings text
  **verbatim** (the cat project's `project_settings.config` comes back byte-identical). New edits
  on top are applied as full per-slot vectors through `Project::update_settings` and their Orca
  keys are merged into `different_settings_to_system`, per scope (process, one entry per
  filament, printer). This list matters: Orca's `load_external_preset` resets every option of a
  system preset's project copy that is *not* listed to the installed preset's value, so an
  unlisted edit would silently vanish in Orca.
- Otherwise, Orca flavour on a Snapmaker printer composes the selected presets the way Orca
  writes a project (`Profiles::orca_project_config`: `Bundle::full_config_with_project` with
  the per-tool filaments, `curr_bed_type`, `filament_colour`, the preset names, user profiles
  named as selected with their parents in `inherits_group`), then applies the overlays and user
  profile settings as above with the list. Prusa flavour writes the whole canonical configuration
  as `; key = value` lines. A Prusa printer saved in Orca flavour goes through the conservative
  dialect translation (the whole configuration, plus the list), as before.
- `filament_colour` is always the colours the GUI shows (`App::tool_colours`).

Round trip: the GUI's `transform_mesh` and `lowest_z` now go through the writer's f64
`Transform` (`scene::to_threemf`), and volumes are placed in two steps (local, then object), so
a saved and reopened project slices to **byte-identical G-code** through the GUI worker — the
tests check this for an Orca-flavour save of `plates-orca.3mf` on the XL (modifier, height
range, mirrored instance, scaled and turned object, support stroke, a process edit) and a
Prusa-flavour save of `painted-cube.3mf` on the XL (negative cylinder, colour stroke, a filament
edit), and a U1 test (skipped without the installed Orca profiles) checks the verbatim settings,
the per-slot vectors and the `different_settings_to_system` list. `ProjectSettings::parse_prusa`
now reads PrusaSlicer's `; key = value` lines (it skipped them as comments before, so a real
PrusaSlicer project's presets were never recognised). "Change tool" now sets the wall and infill
roles only; the support roles keep "current tool", which is what PrusaSlicer's wipe tower
requires for non-soluble supports (an explicit support tool made PrusaSlicer refuse the file).

Headless: `mica-gui --screenshot … --save out.3mf [--flavour orca|prusa]` saves after the
passes (so a scripted paint stroke is in the file), `--save-as` opens the dialog,
`--unsaved-prompt` the prompt.

```sh
B=./target/release/mica-gui; F="$MICA_PRIVATE_MODELS/cat.3mf"   # the optional private multi-tool test project
$B --screenshot out/work/gui10-save-as.png "$F" --printer "Snapmaker U1 (0.4 nozzle)" --save-as
$B --screenshot out/work/gui10-title-modified.png "$F" --printer "Snapmaker U1 (0.4 nozzle)" --select 0 --command modifier:box --focus --crop-height 60
$B --screenshot out/work/gui10-unsaved-prompt.png "$F" --printer "Snapmaker U1 (0.4 nozzle)" --select 0 --command modifier:box --unsaved-prompt
$B --screenshot out/work/gui10-cat-modified.png "$F" --printer "Snapmaker U1 (0.4 nozzle)" --select 0 --command modifier:box --sub-set fill_density=100% --focus --paint colour --paint-tool 2 --brush sphere --brush-size 10 --drag 500,380,640,420 --save out/work/modified.3mf --flavour orca
```

Screenshots (`out/work/gui10-*.png`): `gui10-save-as.png` (the dialog with the format choice
over the project), `gui10-title-modified.png` (the header with `… •` after adding a
modifier; `gui10-modified-window.png` is the whole window), `gui10-unsaved-prompt.png` (the
modal prompt), `gui10-cat-modified.png` (the modifier and the tool-2 stroke that went into the
verification file), `gui10-save-as-prusa.png` (the dialog on the XL, Prusa preselected), and
`gui10-title-window.png`, a `screencapture -l` of the real window (title
`<project> — Mica Slicer`; synthetic System Events input does not
reach the egui window, so the `•` form of the OS title is covered by `App::window_title`'s test
and the header shot, not by a native capture).

### Native verification, 2026-10-16

OrcaSlicer 2.4.1, embedded project settings only (fresh `--datadir`), plate 1 of the cat
project opened with the U1 selected (its settings in use), `--slice 1`:

```sh
/Applications/OrcaSlicer.app/Contents/MacOS/OrcaSlicer --datadir $V/data --slice 1 --outputdir $V/out <file>
```

| File | filament used [mm] T1 / T2 / T3 / T4 | total g | time |
|---|---|---:|---|
| original | 11944.38 / 340.30 / 3286.36 / 357.70 | 47.51 | 3h 7m 38s |
| GUI save, no changes | 11931.15 / 341.99 / 3282.56 / 368.95 | 47.50 | 3h 7m 40s |
| core writer alone (`mica slice … --export-3mf`) | 11940.50 / 339.23 / 3289.94 / 366.86 | | |
| GUI save + modifier (100 % infill) + colour paint (tool 2) | 14894.77 / 1773.64 / 3103.92 / 368.95 | 60.07 | 3h 47m 12s |

The settings text is byte-identical to the original, the painted triangle counts and codes per
object are identical, the 244 layers and the per-layer tool-change sequences are identical.
Tools 1–3 are within 0.12 % / 0.5 %; tool 4 (the smallest colour, 0.36 m) is 3.1 % over in
both the GUI save and the core writer's own copy, so it comes from the baked part meshes
(`Part.mesh` is in object coordinates in f32; Orca kept the unscaled source mesh and the
6.5× volume matrix in double) shifting Orca's MMU segmentation boundaries a little, not from the
GUI layer. Writing source meshes with their component matrices would need `Part` to keep
them; left for later. The modifier (dense infill in the first cat) and the tool-2 stroke add
2.95 m / 1.43 m as expected, and the file opens and slices in Orca without complaint.

PrusaSlicer 2.9.6 (`--datadir out/ref/datadir`), `painted-cube.3mf` on the XL 5T (Mica's
`--xl` selection: 0.10mm FAST DETAIL, Prusament PLA/PETG/ASA, Generic PLA, PC Blend), saved in
Prusa flavour and sliced with the embedded settings, against the fixture sliced with the same
presets named explicitly:

```sh
PrusaSlicer --datadir out/ref/datadir --no-binary-gcode --export-gcode -o saved.gcode cube-prusa.3mf
PrusaSlicer --datadir out/ref/datadir --printer-profile 'Original Prusa XL - 5T Input Shaper 0.4 nozzle' \
  --print-profile '0.10mm FAST DETAIL @XLIS 0.4' --material-profile 'Prusament PLA @XLIS' \
  --material-profile 'Prusament PETG @XL' --material-profile 'Prusament ASA @XL' \
  --material-profile 'Generic PLA @XLIS' --material-profile 'Prusament PC Blend @XL' \
  --no-binary-gcode --export-gcode -o orig.gcode tests/models/painted-cube.3mf
```

The fixture sliced with the five `--material-profile`s **segfaults** in PrusaSlicer 2.9.6
(exit 139 at "Calculating overhanging perimeters", as the earlier oracle note recorded), so
the exact parity check loads the saved file's own `Slic3r_PE.config` as an INI onto the
fixture instead (`--load saved-config.ini`, the `; ` prefix stripped), which slices:

| File | filament used [mm] T1 / T2 (T3–T5 unused) | total g | time |
|---|---|---:|---|
| fixture + saved settings (`--load`) | 2917.42 / 2921.78 | 17.63 | 1h 23m 43s |
| GUI save, Prusa flavour (embedded settings) | 2917.42 / 2921.78 | 17.63 | 1h 23m 43s |
| GUI save + modifier box (100 % infill) | 3209.82 / 2921.78 | 18.50 | 1h 28m 33s |

Geometry, colour painting and settings are exactly equal in PrusaSlicer; the modifier adds
292 mm of tool 1 as expected. Before the "Change tool" fix the saved object carried
`support_material_extruder = 1`, which PrusaSlicer refused ("The Wipe Tower currently supports
the non-soluble supports only if they are printed with the current extruder…").

CLI G-code for cube20 and Benchy (CORE One defaults and the U1) is byte-identical to the
pre-change build (`cmp`). `cargo fmt`, clippy `-D warnings`, `cargo test --workspace` and
`cargo xtask provenance` pass.

Left for later: a loose STL saved as a project slices through the engine's regions path
afterwards (every 3MF object does), which differs from the plain path by ~0.03 % of filament on
the cube (first-layer perimeter corners) — an engine divergence, not a file one; the Orca tool-4
deviation above; Prusa flavour prints the first plate only (the writer's rule), so a project
whose first plate is empty shows nothing printable in PrusaSlicer; no "New project"; the Save As
dialog is followed by the native picker (two steps) rather than embedding one.

## Correctness fixes: source precision and CLI settings

The source-retention API is `Part::source_mesh` (`Arc<SourceMesh>`: f64 vertices, source
faces, f64 accumulated component transform). The engine continues using its existing f32
mesh and placement arithmetic. `SourceMesh::matches` checks geometry before reuse, so edits
cannot accidentally save stale source data. `InputObject::source` carries the native object
and bed-local f64 build placement into the GUI. The GUI matches unchanged parts separately
from settings/painting, unplaces newly added or edited volumes, and composes editor moves
with the original build placement. Cuts use their newly authored meshes. Prusa's merged
native layout and the engine's f32 slicing arithmetic remain as before; nested components
continue to flatten to their accumulated f64 transform.

`threemf::load_config` reads effective settings without loading geometry and honours the same
CRC/size-guarded Mica marker as full import. Printer/preset matching and per-tool assignment
are shared in `mica_profiles::project`; `ProjectSettings::apply` applies scoped changes and
colours, and `merge_filament` composes the resolved per-tool filament vectors. The CLI applies
these before object import, then applies explicit setting overrides. An explicit `use` keeps
the selected printer; matching project process/filament presets and project changes apply to it.
Support write-back extends the already adapted Orca/Prusa option mapping; all other glue is
original Mica code.

Regression coverage includes eight support mode/style combinations, sparse support edits,
matching/mismatching printer decisions and explicit use/ignore, distinct per-tool filament
presets and overrides, repeated f64 source round trips with 6.5x/reflected components,
independent painting winding flags, native matrices without the marker, stale source geometry,
and GUI saves with new modifiers and painting. Painting backups use the same native triangle
frame as the geometry writer, including retained component reflections.

The real cat archive retained bit-identical source vertex coordinates in all 20 parts and
byte-identical embedded settings. Slicing the original and corrected saved cat with embedded
settings produces byte-identical Mica G-code; both validate with zero errors. CORE One and XL
cube20/3DBenchy text outputs compare equal
to the pre-change binaries and validate with zero errors. The quick corpus covers 11
printer/model combinations, all passing with zero validation errors. The existing edited
Orca/Prusa GUI save tests also retain identical G-code bytes.

Native Orca 2.4.1 was invoked with four explicit filament inputs. The original cat's plate 1
used 11803.40 / 338.00 / 3188.93 / 357.70 mm. The first written archive exited 156 (generic
slicing error), without a segmentation fault. Source inspection then exposed leading whitespace
inside component matrix attributes: Orca's literal-space parser counts an extra token and
falls back to identity. The writer now emits exactly 12 tokens with no surrounding whitespace,
checked by a regression and all 40 native matrices in the corrected cat archive. Orca was not
launched again, following the stop-after-one-failure rule. **Native written/source filament
parity is therefore still unverified**; the whitespace diagnosis is based on source inspection.

The workspace U1 oracle reuses its saved Orca reference (`MICA_U1_ORCA_REFERENCE`) rather than
relaunching Orca.

Final checks passed: `cargo fmt --all --check`, offline workspace clippy with `-D warnings`,
`cargo test --offline --workspace` (856 passed, four ignored), release CLI build,
`cargo xtask provenance` (341 files checked, 213 ported/adapted), and
`cargo xtask corpus --quick` (11 passing cases, 5.73 s, no validation errors). Review source-cache matching for
edited geometry and the explicit-use/mismatched-printer policy; native Orca filament parity
still needs an oracle run.

## CLI settings preservation (2026-10-16)

CLI export now uses the same `mica_profiles::project::SaveSettings::write` as GUI Save.
Profile resolution returns the source `RawConfig` only when embedded settings were actually
used (explicit `--project-settings use`, or the matching-printer default). In the source's
native flavour, a no-change save writes that text byte-for-byte, regardless of whether
profile arguments or `-o` are present. `--set` edits and the explicit perimeter-generator
flag patch only their keys; Orca filament values are full slot vectors and the edited keys
are merged into `different_settings_to_system` by process, filament slot and printer.
Unedited native percentages, enum spellings, unknown options and preset identities survive.
Prusa edits likewise patch the original settings instead of adding resolved defaults.

With `--project-settings ignore` (or a printer mismatch without explicit use), export writes
the selected profiles. `project::selected_orca` composes native Orca profiles for both clients;
Prusa settings and cross-dialect fallback use the shared writer. Export-only commands now
resolve profiles and the use/ignore decision too; the former default-config shortcut and
"any profile flag means export the whole resolved config" rule have been removed.

Regression coverage exercises the real CLI with and without slicing, automatic and explicit
use, ignored settings, native Prusa settings on a copy of the geometry-only painted fixture,
explicit alias/scope assignment, and the private multi-tool project when available. The shared writer tests
assert the exact set of changed JSON keys, full filament/printer vectors, retention of existing
change-list entries, and preservation of Prusa CRLF source text on no-change saves.

Local release verification:

- Cat source `Metadata/project_settings.config`: **34,890 bytes**, identical in the exported
  archive. Original and exported cat plate 1 produce identical Mica G-code bytes (244 layers);
  both validate with **0 errors**. Per-tool filament: 12042.736 / 358.556 / 3151.614 / 376.552 mm.
- Cat with `--set perimeters=5 --set temperature=221,222,223,224
  --set retract_length=0.9,1,1.1,1.2`: only `wall_loops`, `nozzle_temperature`,
  `retraction_length` and `different_settings_to_system` change. Temperature/retraction vectors
  have four entries; the native shell enum and bridge percentage remain unchanged. The edited
  archive reloads, slices and validates with **0 errors** too.
- CORE One and XL cube20 / 3DBenchy text G-code match the pre-change release byte-for-byte;
  all four outputs validate with **0 errors**. Quick corpus: **11 passing cases, 0 errors**.

Native OrcaSlicer parity of the exported filament settings is not verified yet: slice the
original and the exported project with OrcaSlicer (a fresh data directory each, the project's
filament settings loaded explicitly) and compare.

The native edit/serialization logic was moved from GUI Save; the schema scope lookup uses the existing Orca schema data.
Cross-flavour saves retain the existing conservative translation limits.

Final checks passed: `cargo fmt --all --check`, offline workspace clippy with `-D warnings`,
`cargo test --offline --workspace`, the final CLI/shared-writer unit tests, release CLI build,
`cargo xtask provenance` (346 files, 214 ported/adapted) and `cargo xtask corpus --quick`.
The existing GUI Orca/Prusa edited-save G-code regressions and U1 native-settings save regression
pass.
