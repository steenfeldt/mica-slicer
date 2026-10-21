# GUI: sidebar, settings, plates

> Later GUI work restyled the sidebar (cards, combos, curated ten-row Essentials, Cooling &
> temperatures, guard card, object-scope card) and replaced the status bar and plate tabs with the
> header read-out and the plate bar. Where this file and the app disagree, the app is current.

The sidebar, on top of [gui-foundation.md](gui-foundation.md) and the viewport work in
[gui-viewport.md](gui-viewport.md). Orca-familiar structure (left sidebar, plates,
Prepare/Preview tabs), tuned to be more obvious than Orca or PrusaSlicer. All code is original;
the two data tables it reads are generated from upstream (see PROVENANCE.md, "M6 GUI, part 2").

## What you see

```
┌ Mica  File  Edit  Undo Redo ──────── Prepare | Preview ──────── Slice plate  More  Export G-code ┐
│ ┌ Printer ─────────────────── ✎ ┐ │ Plate 1 (2)  Plate 2  +                                    │
│ │ [Prusa CORE One HF0.4 nozzle ▾]│ │        [+ ⊞ ⤓ | ✥ ↻ ⤢ ◭ | ✂ ≡ | ◮ ⊡ ◑]  (viewport toolbar) │
│ │ 0.40 mm nozzle · 250×220×270  │ │ ┌ Objects · Plate 1 ┐              ┌ 3DBenchy.stl ──────┐ │
│ ├ Filament ─────────────────────┤ │ │ 3DBenchy.stl  T1▾ │              │ Position  X Y Z     │ │
│ │ ■ [Extrudr GreenTEC … ▾]    ✎ │ │ │ cube20.stl    T2▾ │              │ Rotation  X Y Z     │ │
│ ├ Process ──────────────── 💾 ⋯ ┤ │ └───────────────────┘              │ Size / Scale / Mirror│ │
│ │ [0.20mm SPEED @COREONE ▾]     │ │                 3D view with gizmo │ Uniform  Drop to bed │ │
│ │ Settings for [All objects  ▾] │ │                                    └─────────────────────┘ │
│ │ [🔍 Search all settings      ] │ │                                                            │
│ │ ⚠ Layer height 0.35 mm … [Set 0.30 mm]                                                        │
│ │ ▾ Essentials                  │ │                                                            │
│ │   LAYERS                      │ │                                                            │
│ │   Layer height     [  0.20 mm]│ │                                                            │
│ │   Wall loops       [       2 ]│ │                                                            │
│ │   Supports         [Enforcers only ▾]                                                         │
│ │ ▸ Quality  0.20 mm layers · 0.45 mm lines · aligned seam                                      │
│ │ ▸ Strength 2 walls · 5/3 top/bottom · 15 % grid                                               │
│ │   Show all settings           [○ ]│   ◬ triad   hint line        ‹ Plate 1 / 2 ›  + │ bed  │
├ Ready to slice · 38m 44s · 3.66 m · 12 g · Last change: +4 min, +3 g          auto-slice on  61 MB ┤
```

**Cards.** Printer (combo of *My printers* + *All printers…*, a one-line summary, the Snapmaker
plate type, ✎ opens the printer editor), Filament (one row per tool: colour swatch, combo, ✎
opens the filament editor; models in the 3D view use the swatch colour), Process (combo, 💾
saves into a user profile, ⋯ holds *Save as*, *Discard changes* and *Show all settings*).

**Scope line.** "Settings for: All objects ▾". Picking an object (or selecting one and using the
link) switches to "*Benchy* (overrides)" with "← back to all objects"; that scope shows only the
object's overrides plus *+ Add override*, and search offers any setting to add.

**Settings list (topic rail, design D2, 2026-10-18).** For all objects the Process card shows a
topic rail on the left (Essentials, Quality, Strength, Speed, Support, Cooling & temperatures,
Multimaterial on multi-tool printers, Advanced: an icon above a short label; icons only below a
340 px card; a dot marks topics with changed settings; the buttons stay in view while the list
scrolls) and one topic at a time on the right. The topic header has its name, "N changed" and a
search field over every topic plus a *Changed* chip. Inside a topic each section (Orca's option
group: Layer height, Seam, Infill, …) has a line icon, a bold title, a rule and a chevron; a click
folds it, and folded sections are remembered in `mica.toml` (`collapsed_sections`, like the picked
`settings_topic`). A folded section with changed settings shows "N changed" on its header. The
header of the section being scrolled sticks to the top. A topic lists Orca's Simple settings and
ends with "Show N more settings" / "Show fewer"; *Show all settings* in the fixed footer (also in
⋯) shows everything. Searching lists the matching rows of every topic, grouped under small topic
headings in rail order, folded sections included, matches coloured; *Changed* does the same for
settings that differ from the preset (both combine). Essentials only repeats rows no other topic
lists. Picking a topic ends search and filter. Code: `panels/topics.rs`, icons in
`panels/settings_icons.rs` (original, drawn with egui strokes).

**Rows** (`panels/editor.rs`, `panels/widgets.rs`). Label left, one control in a fixed 132 × 24
column right-aligned to the card edge; 28 px rows. Numbers are right-aligned with the unit in a
muted colour and sensible precision ("0.20 mm", "15 %", "5 layers"); drag to change, click to
type. Booleans are toggle switches. Dropdowns carry pictograms (seam position, infill and
support patterns, brim type, support style) in the closed control and in every item. Float-or-
percent settings use the same number field with the unit it currently has. Changed values show
an accent bar at the row's left and a ↺ reset on hover; values a user profile changes relative
to its system preset show an orange bar. Settings with no effect are greyed out with the reason
on hover (brim width with no brim, support style/angle with supports off, raft/skirt/ironing/
fuzzy-skin/prime-tower dependents). Hovering a label gives the explanation, unit, range and key.

**Supports.** PrusaSlicer presets encode "supports for enforcers only" as `support_material = 1`
with `support_material_auto = 0`, so a bare "Enable support" toggle looked wrong (the CORE One
SPEED profile *does* have supports on, for enforcers). Essentials shows one *Supports* dropdown
instead: Off / Enforcers only / Everywhere / Build plate only, standing for the three keys.
The *Support* section still has the raw toggle, with an "enforcers only" hint.

**Floating panels** over the 3D view use the theme's `overlay_frame`: the Objects list (top
left: tree with 3MF parts and per-object settings as children, tool picker per row, plate
warnings) and the selected object's panel (top right: position, rotation as Euler angles,
size in mm and scale in % with a uniform lock, mirror, Drop to bed, tool, link to its
overrides). The viewport toolbar owns add / arrange / move / rotate / scale / lay on face /
cut / mesh tools / text / painting / layer heights / brim ears / measure / section (grouped,
see gui-viewport.md), so those are not repeated in menus.

**Preview** replaces the sidebar with the slice summary (print time, filament length / weight /
cost per tool with swatches, layers, output format, warnings); legend and sliders live in the
3D view.

### Intuitiveness choices (where they beat Orca parity)

- *Progressive disclosure.* Essentials holds the ~15 settings most prints need (layer heights,
  walls, top/bottom layers, seam, infill density/pattern, supports mode/style/angle, brim/skirt,
  three speeds, nozzle and bed temperatures per tool). Everything else sits behind per-section
  "Show more", *Show all settings* and search. Anything the user changed stays visible.
- *Plain language.* Labels are Orca's (Wall loops, Sparse infill density, Outer wall) for every
  printer. Where the value conversion between Orca and PrusaSlicer is not one-to-one (line
  widths, accelerations, bed temperatures, shrinkage), the label stays Orca's but the
  explanation, unit and range are PrusaSlicer's, because the stored values are PrusaSlicer's.
- *Consequences.* Auto-slice runs 0.5 s after the last change; the status bar and summary show
  the effect on time and weight ("Last change: +4 min, +3 g").
- *Guard rails* (`checks.rs`): layer height outside 10–75 % of the nozzle, first layer taller
  than the nozzle, zero walls, open top with sparse infill, tall thin part without a brim,
  0 °C nozzle. Inline above the list with a *Set …* fix button, never a modal.
- *Fewer clicks.* Tool picker on every object row and in the object panel; one primary button
  (*Slice plate*, with *Slice all plates* under *More*); Save-as is a small window; shortcuts in
  tooltips (Cmd+O, Cmd+R slice, Cmd+G export, Cmd+Z/Shift+Cmd+Z, A arrange, Delete, Tab,
  0–6 views, Z fit, P perspective, M/R/S gizmos, F lay on face).
- *Vocabulary.* "Tool N", "Filament", "Print time" mean the same in the sidebar, object list,
  status bar, legend and summary.

## Integration with the viewport (gui-viewport.md)

- **Transform model**: `scene::Object` keeps the viewport's `angle` (Z) + `tilt: Quat` +
  `scale`, plus `mirror` (±1 per axis), `overrides` (per-object settings) and `parts` (3MF
  parts for the tree). Gizmo `Delta`s, `lay_on_face` and the object panel all drive the same
  fields; the panel edits Euler angles through `Object::euler` / `set_euler`.
- **Plates**: `plates::Plates` (the editor's model: objects per plate in plate-local
  coordinates, per-plate override hook, slice result and before/after totals) implements
  `scene::plates::PlateLayout` (`count`, `current`, `origin(i)`: a near-square grid, 20 mm
  gaps). The stand-in struct is gone. The viewport gets every plate's objects in world
  coordinates (current plate first, so its indices are the plate's own); selecting or moving an
  object on another plate switches to that plate. Plate tabs above the view, the plate bar below
  it and the per-plate hover icons all act on the same model (lock and per-plate settings are
  still placeholders).
- **Theme**: `theme::apply` runs at start-up with the remembered mode (`mica.toml`:
  `theme = system|dark|light`); the toolbar's theme button cycles and persists it. Floating
  panels use `theme::overlay_frame`; the sidebar's controls use the theme's fills and the accent
  for hover/focus.
- **One model loader.** `mica_core::input::load` (STL/3MF, placement, tool assignments, the
  regions model with modifiers, negative/support volumes, height ranges and per-object/part
  settings) serves the CLI and the GUI; `InputObject::prepare` slices with regions when the
  object has them. Dialect-specific settings mapping is injected through
  `input::SettingsMapper` (`mica_profiles::project::Mapper` handles Orca names), so the engine
  stays free of profile and GUI code. The CLI is a thin wrapper that prints the loader's
  warnings and the preparation statistics. In the GUI, a 3MF object's own settings become its
  overrides, and the worker applies overrides and the moved regions model
  (`Object::placed_region`) when it prepares each object.
- **Preview times and speeds come from the export.** `mica_core::print::LayerSource::emitted`
  (default no-op) is called by the exporter once a layer's cooling slowdown has run, with one
  `PathEmission` (path index, point range, final feedrate, fan) per emitted stretch and the
  layer time; `LayerPlan::emissions` produces them. The GUI's capture keeps the current batch's
  scheduled layers and writes each to the preview when its emission arrives
  (`Builder::push_emitted`), so the speed/fan colours, per-feature times and layer times match
  the G-code; the legend's total is the exporter's print time (`Preview::total_time`). The
  estimate path (`Builder::push`) remains for layers the exporter never reports.

## First run and persistence

`state/printers.rs` reads `PrusaSlicer.ini` (installed models/nozzles from
`[vendor:PrusaResearch]`, last presets from `[presets]`), imports PrusaSlicer user presets
(`PrusaSlicer/{print,filament,printer}/*.ini`) into Mica's profile store, and adds Orca user
machines (`OrcaSlicer/user/default/machine`, `Snapmaker_Orca/user/default/machine`). Nothing
in those installations is modified; no credentials are read.

`state/mod.rs` keeps `~/Library/Application Support/Mica Slicer/mica.toml` (`MICA_CONFIG_DIR`
overrides the folder): window and sidebar size, selected printer/process/filaments/plate type,
*My printers*, recent files, auto-slice, *Show all settings*, theme, open and expanded
sections. Reads tolerate missing or broken files (defaults, with a notice); writes are atomic
and debounced to once a second.

## Settings data

`cargo xtask gen-option-text [orca-dir] [prusaslicer-dir]` regenerates
`crates/mica-gui/src/settings/option_text.tsv` (label, full label, category, unit, min, max,
Simple/Advanced, tooltip, enum choices, source per canonical key) and `pages.tsv` (tab, page,
group, line label, key). It needs the pinned `orca` and `prusaslicer` checkouts (or two
directories with `src/libslic3r/PrintConfig.cpp` and `src/slic3r/GUI/Tab.cpp`). Orca keys are
translated through `crates/mica-profiles/src/orca/mapping.tsv`; canonical options Orca does not
show land in a "More settings" group of the page their PrusaSlicer category suggests.

Edits live in `overlays.rs` (print, printer, one per filament tool) as PrusaSlicer text on top
of the selected presets; the effective `Config` is presets + overlays + plate overrides, and each
object is prepared with its own overrides applied (`worker::object_config`). *Save as* writes a
Mica user profile (TOML, only the differences, `inherits` = system preset) and selects it.

## Plate settings, project colours, kept slices (2026-10-17)

**This plate.** "Settings for ▾" offers *This plate* next to *All objects* and the objects. It
lists only the current plate's own settings (`Plate::overrides`), each with the changed marker,
↺ / × back to the global value and "All plates: …" under it; "+ Add plate setting" offers
Orca's plate options first (bed type on printers with bed types, print sequence, spiral vase),
then the object-override groups; searching adds any print setting. They apply to that plate's
slice only (`App::plate_config`; a bed type re-resolves the Orca bundle on that bed, so bed
temperatures follow). Saved as Orca's own plate keys in `model_settings.config` (`bed_type`,
`print_sequence`, `spiral_mode`; OrcaSlicer 2.4.1 `--slice 1` of a Mica save printed
`curr_bed_type = Textured PEI Plate`, `spiral_mode = 1`), other settings as
`mica_override:KEY` (ignored by Orca), and in the Prusa flavour through Mica's marker. A locked
plate keeps its layout, not its settings. Code: `plate_settings.rs`, `panels/plate_scope.rs`.

**Filament colours are project state** (`Overlays::colours`), as `filament_colour` is in Orca's
project config and not a filament preset option: no changed marker, no filament "change", never
saved into a profile; a reopened project reads as unchanged. Choosing another filament preset
keeps the colour unless that preset has its own (Orca's `update_ams_color`). Edits of
`filament_colour` arriving through a filament overlay (editor, colour picker) are moved there by
`Overlays::normalize`.

**Kept slices** (`slice_cache.rs`). Completed slices are kept by everything that determines the
G-code (the job's objects or calibration parameters and the job's resolved configuration). An
undo, redo or edit that returns a plate to a sliced state gets that slice (preview and export
file) back at once; an undo step keeps every plate it did not touch as it is. Slices the plates
show are always kept, plus the 4 newest others within 2 GiB of disk. A colour-only change keeps
the slice; Export and Print slice again first unless a slice with those colours is cached. The
monkey test checks that a shown slice fits its plate's inputs and slices a restored state again,
comparing the G-code byte for byte.

## Headless screenshots

`mica-gui --screenshot out.png model.stl …` renders the **whole window** (egui + 3D view) into
a wgpu texture: `[--preview] [--xl] [--printer name] [--select N] [--gizmo move|rotate|scale]
[--plates N] [--tab Essentials|Quality|…] [--collapse Section] [--advanced] [--search text] [--changed]
[--object-mode] [--object-set key=value] [--set key=value] [--printer-editor]
[--filament-editor N] [--all-printers] [--light] [--size 1440x900] [--scale 2] [--crop 390]`.
`--crop` keeps the left part (sidebar close-ups). It uses a throwaway profile store
(`--real-store` to use the real one) and never writes preferences. `--screenshot-3d` is the
viewport-only path from gui-viewport.md. The review set lives in `out/work/gui3-*.png`.

## Tests

`cargo test -p mica-gui`: printer detection from a sample `PrusaSlicer.ini`, preferences
round trip and broken-file fallback, compatibility filtering and user-profile save/restore,
overlay application per tool, undo stack (limit, redo, coalesced drags), per-tool cost math and
G-code footer parsing, plate warnings, per-object overrides changing one object's slice, guard
rails, dependent-setting reasons and hints, Delta text, section summaries, number formatting
and the control column, settings tables (Orca labels, one placement per key), search,
Euler/mirror round trips, plate layout, screenshot argument parsing. The table generator has
its own unit tests in `xtask`.

## Not done

- Per-object `layer_height` overrides slice (layers are merged by Z) but have not been
  print-tested. Height ranges follow the object's Z position and scale; rotating a 3MF object
  with height ranges does not re-slice the ranges along the new axis.
- Filament rows equal the printer's tool count; there is no AMS-style variable slot count.
- Plate lock and per-plate settings are hooks only (status message).
- No 3MF project save (Cmd+S left out), no OBJ import (engine has STL/3MF only).
- Multi-line texts (custom G-code, notes) are edited in a full-width box under their label
  in the printer/filament editors; in the sidebar they appear as single-line fields.
