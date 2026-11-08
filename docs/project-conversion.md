# Converting a project to another printer (design, proposed)

Status: design for review, nothing built. Related: [opening a 3MF project](gui-project-import.md),
[setting names](setting-names.md), [multitool](multitool.md), [prime tower](gui-prime-tower.md).

## The problem

A 3MF made for one printer (a Bambu X1C, an H2D) opened for another (a U1, an XL, a CORE One)
has two answers today, and neither is right:

- **Use project settings** keeps the selected printer when the project's is another one, then
  slices with the project's *whole* embedded configuration (`Mode::Embedded`): the other
  machine's speeds, accelerations, temperatures, flow, retraction, tower and flush volumes.
  An H2D project on a U1 differs in 153 values; the H2D's `extruder_printable_area` cannot
  even be read, and the slice stops ("wipe tower supports independent tools, not MMU") or
  places the tower off the bed.
- **Keep mine** slices cleanly with the selected presets, but drops what the designer chose
  on purpose: supports, infill, brim, walls, seam. Only geometry, plates, paint, colours and
  per-object settings survive.

OrcaSlicer, Bambu Studio and PrusaSlicer offer the same two choices ("load settings" or
"geometry only"). Converting is new.

How common it is, from one user's library of 145 downloaded and own projects (137 Orca/Bambu,
8 PrusaSlicer):

- 76 % were saved for a printer other than the one they print on (X1C 26, P1S 20, H2D 17, A1 17,
  P2S 10, A1 mini 9, …).
- What the projects list as deliberately changed (`different_settings_to_system`) is almost all
  portable: `enable_support` 84, `support_type` 60, `sparse_infill_density` 32,
  `sparse_infill_pattern` 32, `brim_type` 16, `wall_loops` 14, `seam_position` 13. Machine
  tuning is rare: `outer_wall_speed` 7, other speeds 2–5.
- Filament changes are machine tuning: `enable_pressure_advance` 11,
  `filament_max_volumetric_speed` 11, temperatures 3–4.
- 65 of 137 have more than 4 filament slots (5: 34, 6: 17, up to 16): more than a U1 has tools.

## Goal

Converting keeps the design (what is printed, in which colours, with which deliberate print
choices) and takes everything about the machine from the target printer's own presets.

Non-goals: converting machine tuning (speeds scaled by acceleration ratios, temperatures
between hotends), and slicing like the source printer would. A project for the selected
printer is used as today, unchanged.

## When it happens

`decide` gets a third answer:

| Project made for | Decision | Card |
|---|---|---|
| the selected printer (same preset, a copy, same model and nozzle) | `Use` (today) | "Using this project's settings … [Revert] [OK]" |
| another printer, or one Mica does not know | `Convert` (new; was `Ask`) | "Made for Bambu Lab H2D … [Convert to Snapmaker U1] [Keep mine]" |

The same model with another nozzle diameter is another printer: line widths and layer heights
depend on the nozzle.

## What carries over

### The intent: what the designer changed

The carried settings are the ones the designer changed from their system presets, never the
whole embedded configuration:

1. Orca/Bambu projects list them: `different_settings_to_system` (process; one entry per
   filament; printer).
2. A project without the list (PrusaSlicer, older Orca) is compared with the system preset it
   names (`print_settings_id`), when Mica has it. Mica ships the Orca bundles, Bambu's
   included, and the Prusa bundle; so this covers nearly every file.
3. Neither: nothing but the layer height carries (see below), and the report says so.

### Which changed settings are portable

Each print option is classified once, by where OrcaSlicer's own Tab puts it
(`crates/mica-gui/src/settings/pages.tsv`, generated from Orca's `Tab.cpp`), plus a short list
of key exceptions:

| Orca page / group | Converted |
|---|---|
| Quality: layer height, seam, ironing, wall generator, walls and surfaces, overhangs, bridges | carried |
| Quality: line width | carried when the nozzle diameter is the same, else dropped |
| Quality: precision | carried except XY hole/contour compensation and elephant foot (calibrated per machine) |
| Strength (walls, shells, infill) | carried |
| Support, raft, tree supports | carried; support/interface filament through the tool map |
| Others: skirt, brim, special mode, fuzzy skin | carried |
| Multimaterial: filament for features | carried through the tool map |
| Multimaterial: prime tower | `enable_prime_tower` carried; size, brim, ribs, position dropped |
| Multimaterial: flush options, ooze prevention | dropped (the target's tower and purge rules) |
| Speed (all, acceleration included) | dropped, listed in the report |
| Others: G-code output, post-processing, notes | dropped |
| Filament tab (all) | dropped: each tool gets the target's preset for the type (below) |
| Printer tab (all) | dropped |

Object, part and height-range overrides go through the same filter; what is dropped is
reported per object. A test fails when an Orca print option has no class, so a new option is
classified on the bundle update that brings it.

### The base presets

- **Process:** the target printer's process with the nearest layer height to the project's
  `layer_height`, among equal distance the same quality word in the name (Standard, Fine,
  Strength, Speed, Draft), then the printer's default. The project's carried changes go on
  top as project values (`Overlays::project`, not edits), as with Use today.
- **Filaments:** per tool, by type: the target's preset of the project filament's vendor and
  type when the target has one, else `Generic <type>` for the target, else the tool's current
  preset (`match_filament`, with the project's type). The colour always carries.
- **Plate type:** the project's when the target has it (`curr_bed_type`), else the target's
  default.

### Tools

Converting keeps every filament: a project has filaments, a printer has tools, and each plate
maps the filaments it uses onto tools (below, "Filaments and tools"). Merging or dropping
colours is only needed on a plate that uses more filaments at once than the printer has tools.

### Plates and the bed

Plates come in as today, then each is re-fitted to the target bed: the plate's objects keep
their layout, centred on the target bed, when it fits; else the plate is arranged; an object
larger than the bed stays and is reported (with Scale to fit, existing action). The project's
multi-nozzle printable areas and per-plate tower positions are dropped; the target places its
tower.

## Filaments and tools (built first)

Today the scene's tool numbers are the printer's tools, and a part on tool 6 of a 4-tool printer
prints with tool 4 without a word (the worker clamps). The model becomes:

- **The project has filaments** F1…Fn (preset, overlay, colour each), as many as it needs,
  never fewer than the printer's tools. Parts, paint, mixes, feature filaments, height ranges,
  overrides and layer tool changes name filaments. A project with as many filaments as tools
  works exactly as before (filament n is tool n).
- **Each plate maps its filaments to tools.** Only the filaments the plate uses need a tool;
  a plate is one print job. Automatic by default (filament n on tool n when free, else the
  first free tool; later: the tool holding the same colour and type on the linked printer),
  with per-plate choices saved in the project (Mica's own plate key; Orca's `filament_maps`
  means filament → nozzle on the H2D, not this).
- **The slicer only sees tools.** At the slice boundary the plate is translated: its
  objects, paint, mixes and layer changes go from filaments to tools (the renumbering behind
  Swap/Move), and its configuration takes each tool's filament settings from the filament
  mapped to it. The engine is unchanged. A plate that uses more filaments than the printer
  has tools does not slice: it says which, instead of clamping.
- Mixed colors keep their numbers after the filaments; at the boundary their components
  become tools.

Phases, each gated and merged on its own:

1. Model and boundary: filament list independent of the printer's tool count, overlays per
   filament, the per-plate map with the automatic rule, the translation at slicing (no more
   clamping), projects with more filaments than tools load and save them all. No visible
   change for projects that fit.
2. The Filaments card: rows per filament with the tool it is on for this plate, add a
   filament, change a plate's map; Filament sync maps to what the printer has loaded; the
   Mica MCP reports filaments and tools.
3. Sending: compare the plate's map with what is loaded, offer "Re-map to loaded filaments"
   (slice again, then send). Then conversion on top, and later planned spool swaps (a pause
   at a layer: "Load F6 into T3") for a plate with more colours than tools over its height.

## The card and the report

Before: "This project was made for Bambu Lab H2D 0.4 nozzle. [Convert to Snapmaker U1]
[Keep mine]" (the printer name from the project; Keep mine as today).

After converting (one undo step "Convert project"):

> Converted for Snapmaker U1: 0.16 High Quality, 4 tools.
> **Kept** supports (tree, on), infill 15 % gyroid, brim off.
> **Dropped** outer wall speed 200 mm/s (H2D tuning), 4 filament settings.
> **Check** plate 2: "Base" is 290 mm wide, the bed 270 mm.
> [Details] [Revert] [OK]

Details lists every carried and dropped value with the project's value. Revert goes back to the
state before, as today.

**Saving.** The document stays the opened file; the first Save after converting asks, defaulting
to Save As "<name> (Snapmaker U1).3mf" beside the original, so a Bambu project is not overwritten
by a U1 one unless chosen.

## CLI and MCP

- CLI: `--project-settings convert`, the default when the project's printer is not the
  selected one (today the default ignores the project's settings then). The report prints as
  notes; `use` and `update` stay.
- Mica MCP: `open` reports the conversion (kept, dropped, tool map) in its result, so an
  assistant can tell the user.

## Building it

- `mica-profiles::project`: `Mode::Converted { nozzle_same }`: the listed or computed changes,
  filtered by a `portability(key) -> Portable | NozzleBound | Machine` table generated with the
  page table (xtask) plus the exceptions; returns the layer and a `ConversionReport`.
- Changed settings for unlisted projects: `ProjectSettings` diff against the named system
  preset composed by Mica.
- `selection.rs`: `Decision::Convert`, process choice by layer height and quality word.
- GUI: the card states; the tool map (reuses `renumber_tools`); plate re-fit (plates, arrange);
  Save As prompt.
- Tests: the classifier covers every print option; conversion of self-made fixture projects
  (an X1C 6-filament project to the U1 and to the XL, a PrusaSlicer CORE One project to the
  U1): carried values, dropped values, the tool map, every plate inside the bed, a slice
  without errors. There is no upstream oracle: no slicer converts.

## Open questions

1. Speed changes: drop them (proposed), or carry slowdowns (a designer's slower outer wall) as a
   ratio to the source preset's value, capped by the target's limits?
2. A plate with more colours than tools: plan spool swaps by height, merge the nearest colours,
   or always ask?
3. Should converting also run without the card, when the selected printer differs (one click
   less), with Revert on the card afterwards?
4. Save As by default after a conversion, or only a note that Save overwrites the original?
