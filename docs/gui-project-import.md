# GUI: opening a 3MF project (plates, project settings, colours)

Opening an OrcaSlicer / Snapmaker Orca or PrusaSlicer project now behaves like opening it in
its own slicer: every plate comes in as a plate, the models show the project's filament
colours and colour painting, and the project's own printer/process/filament settings are
offered on a card (or used right away when the project was made for the selected printer).
Previously the GUI loaded the archive's **global** layout onto one plate, ignored the
embedded settings and colours, and a four-tool project could not even open on a one-tool
printer ("extruder selects tool 2, printer has 1 tools"), which is why a headless
screenshot of a five-plate U1 project showed an empty plate.

## What you see

- **Plates.** "Plate 1 of 5", each plate with the objects Orca had on it, in that plate's bed
  coordinates (Orca's grid: square, 20 % bed-size gap, rows along −Y, bed size from the
  archive's `printable_area`). Plate names (`plater_name`) show in the plate bar and the
  corner labels ("4 · Simple Paint Job"), locked plates (`locked`) say so and refuse
  arrange / move / lay-on-face / delete / clear until unlocked (plate label icon, or the plate
  bar's menu). Switching plates moves the camera to the plate. Objects Orca had outside every
  plate join the nearest plate and stay where they were (so they show as outside the bed).
  Orca's per-plate prime tower position (`wipe_tower_x[i]`, `wipe_tower_y[i]`) becomes the
  plate's tower placement; an explicit edit of `wipe_tower_x/y` in the settings wins over it.
  The raw per-plate metadata (`filament_maps`, …) is kept on the plate for a settings page.
- **Colours.** The project's `filament_colour` list goes into the per-tool filament overlays
  as soon as the project opens, with "Use" and with "Keep mine" alike: the swatches, the
  models (object / part `extruder`) and the painted facets use them. Changing a tool's
  filament preset clears that tool's overlay, colour included (as editing does today).
- **The card** at the top of the sidebar, non-modal: "This project has its own settings:
  printer Snapmaker U1 0.4, process 0.20 Standard (+2 changes), 4 filaments (1 changed), 1
  printer change — [Use project settings] [Keep mine]". For a project made for the selected
  printer (same preset, a `- Copy` of it, or the same model and nozzle) the settings are used
  without asking and the card reads "Using this project's settings … [Revert to my settings]
  [OK]". Revert goes back to the previous selection and edits; the colours stay.
- **Use project settings** selects the project's printer (by preset name, without a copy
  suffix, else by `printer_model` + nozzle variant; a user's Orca copy preset is matched as
  itself), the process (by name, else by the name before ` @`), and each filament (by name,
  else a `Generic <type>` preset, else the current one stays) and the project's plate type,
  then slices with **the project's own values**: its whole embedded configuration (see
  "Project values" below), as OrcaSlicer and PrusaSlicer do. These are the project's, not
  edits: no accent bars, no "N changes" on the process combo, nothing for "Discard
  changes". Edits made afterwards go on top of them.
- **In use**, the card reads "Using this project's settings", with "N values differ from the
  current <process> profile" (expand it for the list: option, the project's value, the
  profile's), and offers **Update to current profile** (today's presets, keeping only what
  the project marks as deliberately changed in Orca's `different_settings_to_system`; a
  PrusaSlicer project lists none), **Revert to my settings** and **OK**. Use, Update and
  Revert are undo steps.

## Project values (2026-10-17)

A 3MF project slices with the settings it was saved with. Before, Mica matched the project
to the nearest current preset and applied only Orca's `different_settings_to_system`, so a
vendor profile changed after the project was saved silently replaced the project's values
(an older private Orca 2.3.3 project, plate 3: prime tower 35% smaller and 19% less filament than OrcaSlicer).

- `mica_profiles::project::ProjectSettings::layer(base, dialect, mode)` maps the embedded
  configuration (Orca `project_settings.config` through the Orca option table without
  Mica's defaults, `orca::mapping::map_sparse`; Prusa `Slic3r_PE.config` as is) and keeps
  what differs from `base`, the composed presets the project names, as a `Layer`: process
  and printer values whole (vectors in the preset's shape: per-extruder lengths, the
  top-left block of a larger flush matrix), filament values per tool. Object, part and
  height-range settings come on top as before. Options the project does not set (newer than
  the project) come from the presets. Bookkeeping (`*_settings_id`, `inherits`, the change
  lists), the colours (project state), `curr_bed_type` (the presets are composed for the
  project's plate type), per-plate tower positions and print-host keys are not part of it;
  nor are options of another fork that neither Mica nor the bundle's schema knows.
- Values the mapping rejects (`ironing_angle = -1`, `wall_direction = auto` in the cow)
  keep the preset's value with a note; readable but absurd values reach the safety checks,
  which refuse the slice with a message.
- `Mode::Listed` (GUI "Update to current profile", CLI `--project-settings update`) keeps
  only the listed changes over today's presets, the former behaviour. An Orca project on a
  PrusaSlicer printer goes through the lossy dialect translation and uses its listed changes
  only.
- GUI: the layer is `Overlays::project` (an `Arc`, so undo steps share it), applied before
  the edits; choosing another process, filament or printer drops the project's values of
  that scope. `App::project_differences` is recomputed with the configuration for the card.
  Save writes the archive's settings text verbatim while the presets and the project's values
  are unchanged (plus new edits); otherwise the selected presets with the project's values
  and the edits listed as changes. The slice cache keys on the composed configuration.

Verification against the unmodified OrcaSlicer 2.4 app (`--datadir <tmp> --slice N`), per-tool
filament from the G-code (`scripts/compare-wipe-tower.py`):

| Project, plate | Orca total mm | Mica before | Mica now | Tower (Orca → now) |
|---|---:|---:|---:|---|
| Cow plate 1 | 17386.4 | 15444.3 (−11.2%) | 17266.9 (−0.7%) | 4216 → 4292 mm (+1.8%), 47.7×24.3 → 47.8×25.2 mm |
| Cow plate 3 | 14725.6 | 11958.8 (−18.8%) | 14594.2 (−0.9%) | 7282 → 7279 mm (−0.0%), same bounds 49.4×49.3 mm |
| Cat plate 1 | 15928.7 | 15953.2 (+0.2%) | 15953.2 (+0.2%) | unchanged |
| Cat plate 2 | 55676.9 | 55956.3 (+0.5%) | 55956.3 (+0.5%) | unchanged |

Cow per tool now within −3.7…+1.2% (plate 1) and −3.2…+0.2% (plate 3). The cow (Orca
2.3.3) differs from today's U1 presets in 86 values (arc fitting off, prime volume 45,
tower width 30, extra spacing 120%, rib length 8, multitool ramming, 1.5 mm retraction at
30 mm/s, classic walls, grid infill, older start/tool-change G-code, …); the cat is recent
(19 values: its silk filament's 7.5 mm³/s and 0.5 mm retraction, 8% infill, organic
supports, flush volumes) and slices as before except the print time. A PrusaSlicer 2.9.6
project (cube20 with 4 perimeters, 35% gyroid, 7 top layers, 228 °C, 1.1 mm retraction;
config block of its own G-code as `Slic3r_PE.config`) slices with those values: the same
G-code as before this change, filament within 0.2% of PrusaSlicer per role.

## How it works

- `mica_core::input::PlateSelection::Each` (new): keeps every instance, moves each plate to
  its own bed coordinates and gives unassigned instances the nearest plate.
  `InputObject::plate` indexes `Loaded::plates` (`LoadedPlate { name, locked, tower,
  metadata }`); `Loaded::config` carries the archive's raw project settings. The CLI's `Auto`
  / `Plate(n)` / `All` are unchanged; the CLI prints its own "embedded settings are not
  applied" note now that the loader no longer reports them as ignored.
- `SettingsMapper::tool_count` (new, default `None`): `mica_profiles::project::Mapper {
  project_tools: true }` (the GUI) resolves object tools against the project's filament
  count when it exceeds the printer's, with a warning, instead of failing the import. The
  CLI's `Mapper::default()` keeps the strict check. The worker clamps tools to the printer's
  count when it slices; the recompute no longer rewrites the objects' tools, so "Use project
  settings" after "Keep mine" still finds them.
- `mica_profiles::project::ProjectSettings::parse(&RawConfig)` reads Orca's
  `project_settings.config` (JSON) and Prusa's `Slic3r_PE.config` (INI): preset names,
  model / variant, filament names, types, colours, per-plate tower positions and `Changes`
  (`changes_against(&base)`). `crates/mica-gui/src/project.rs` holds the matching
  (`match_printer`, `match_name`, `match_filament`, `same_machine`), the `decide` rule, the
  card's `Prompt` / `State` and the overlay builders (`overlays_from`, `colour_overlay`).
  `App::use_project_settings`, `keep_my_settings`, `revert_project_settings` drive it; both
  apply and revert are undo steps.
- `App::tool_colours` reads a tool's `filament_colour` overlay first, then the preset colour,
  then the palette.
- `plates::Plates::import` replaces the plates of a blank scene, else appends;
  `Plate::imported` names unnamed plates `Plate N`. `scene::plates::PlateLayout` gained
  `name(i)` / `locked(i)` for the viewport.
- Headless: `mica-gui --screenshot` loads files **after** the printer and filaments are
  chosen (so the project's tools resolve against the right printer), `--plate N` shows that
  plate, a project keeps its layout (loose STLs are still arranged), and `--preview` slices
  with the plate's configuration (tower placement included).
- The tower planner's `number()` reads an explicit scalar text value of `wipe_tower_x/y`
  before the typed value the Orca mapper retains (the same precedence `orca_motion` uses), so
  an edit or `--set` of these Orca-only keys takes effect.

## Screenshots (`out/work/gui8-*.png`)

```sh
B=./target/release/mica-gui; F="$MICA_PRIVATE_MODELS/cat.3mf"   # the optional private multi-tool test project
$B --screenshot out/work/gui8-cat-plate1.png "$F" --printer "Snapmaker U1 (0.4 nozzle)"
$B --screenshot out/work/gui8-cat-plate1-focus.png "$F" --printer "Snapmaker U1 (0.4 nozzle)" --select 0 --focus
$B --screenshot out/work/gui8-cat-plate2.png "$F" --printer "Snapmaker U1 (0.4 nozzle)" --plate 2
$B --screenshot out/work/gui8-cat-plates.png "$F" --printer "Snapmaker U1 (0.4 nozzle)" --view top --plates 5
$B --screenshot out/work/gui8-cat-banner.png "$F"      # default (Prusa) printer: the card asks
$B --screenshot out/work/gui8-cat-preview.png "$F" --printer "Snapmaker U1 (0.4 nozzle)" --preview --set wipe_tower_x=200 --set wipe_tower_y=10
```

`gui8-cat-plate1.png`: plate 1 of 5 with its two objects in the project's colours (orange
silk body, black / white / red painted details), the card "Using this project's settings",
the four filament rows with the project colours, the process with its two changes.
`gui8-cat-banner.png`: the same project on the CORE One: the card offers the settings, the
colour of tool 1 already applies. `gui8-cat-plates.png`: all five plates from above with the
plate names. `gui8-cat-preview.png`: plate 1 sliced, coloured by filament.

The project's tower position (165, 233) does not fit Mica's tower for four filaments and
~240 layers (86 mm deep, X 157..208, Y 227..313 on the 270 mm bed), so the preview shot sets
`wipe_tower_x/y` explicitly; in the GUI the same edit in the settings does it. Making Mica's
U1 tower as compact as Orca's is engine work outside this change.

## Tests

- `mica_core::input::plates`: `Each` moves every plate to its bed, adopts unassigned
  instances onto the nearest plate, grid origins and nearest-plate lookups.
- `mica_profiles::project`: Orca project settings (names, colours, listed changes mapped to
  canonical names per scope and tool, per-plate towers, tool count through the mapper) and
  Prusa settings (names, colours, option-by-option diff against a base).
- `mica-gui`: `project.rs` (printer matching by name / copy / model, the decision rule,
  name and filament fallbacks, summary text, overlays from changes and colours),
  `plates.rs` (import into a blank or used scene, names, locks), and the headless app on
  the synthetic fixture: plates with their objects, names, locks, plate-local positions,
  tools, tower hook, locked-plate refusal and undo; colours on open, painted facets in
  project colours, offered → kept → used → reverted; paint visibility rules.
- The fixture `tests/models/plates-orca.3mf` is written by `scripts/make-plates-fixture.py`
  (two plates, four filaments with colours, per-object tools, colour painting with an
  extended state, project settings with one listed change, a loose object). The private
  multi-tool project (`MICA_PRIVATE_MODELS`) is read in place and never copied.

## Left for later

- No plate settings page yet (`Plate.metadata` / `Plate.overrides` are the hooks); Orca's
  `filament_maps` are not applied.
- A project's filament that matches no preset falls back to a generic preset of its type
  (with its listed changes on top); there is no "Project filament N" pseudo-preset.
- The card's choice is not remembered per file. (Saving is in [3mf-export.md](3mf-export.md),
  "Saving from the GUI": a project opened here and saved without further edits keeps its
  settings text verbatim.)
