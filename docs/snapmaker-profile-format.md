# Snapmaker U1 profile formats

Mica reads two pinned Snapmaker profile sets with the same importer
(`crates/mica-profiles/src/orca.rs`). Both use the Orca vendor layout
(`Snapmaker.json` index → `machine/`, `process/`, `filament/` JSON, single-name `inherits`
chains, `instantiation`, `compatible_printers` / `compatible_prints` lists and
`*_condition` expressions). They differ in how a preset holds per-variant values.

## OrcaSlicer 2.4.2 (`upstream/orca`, the default)

- U1 filaments are named `… @U1` (Snapmaker) or `… @Snapmaker U1` (Polymaker/Panchroma/Fiberon,
  in `filament/Polymaker/`), each inheriting a `… base` preset and, through it, `fdm_filament_*`.
- Every vector holds one value; `filament_extruder_variant` (one file) holds one variant.
- The library `OrcaFilamentLibrary.json` (`… @System`) has no `compatible_printers`, so its
  filaments fit every printer, the U1 included.
- 65 vendor filaments fit the 0.4 mm U1 (one, `Snapmaker ABS Benchy @U1`, only with
  `0.25 Benchy @Snapmaker U1 (0.4 nozzle)`); TPU also fits 0.6/0.8 mm, nothing fits 0.2 mm.

## Snapmaker Orca 2.4.0 (`upstream/snapmaker-orca`, also its installed app)

Snapmaker's fork splits presets by nozzle and by **flow variant**:

- Filaments per nozzle: `Snapmaker ABS @U1 0.4 nozzle`, `… 0.2 nozzle`, … each listing only its
  own printer in `compatible_printers`; plain names (`Generic PLA`, `Snapmaker PLA Matte @U1`)
  serve 0.4 mm. Its `OrcaFilamentLibrary.json` is empty: only Snapmaker's filaments appear.
- `filament_flow_support = ["standard", "high_flow"]` declares the variants. The 19
  flow-variant options (`filament_flow_variant_options`: temperatures, flow ratio, PA,
  volumetric speed, fans, retraction, wipe distance, z-hop type, ramming, tower purge) then hold
  one value per variant, e.g. `nozzle_temperature = ["215", "220"]`. Other filament vectors
  keep one value.
- Processes do the same with `process_flow_support` and per-variant speed/acceleration/jerk
  arrays (`inner_wall_speed = ["300", "600"]`); the machine declares `printer_flow_support`.
- The per-filament flow type is project state (`filament_volume_type`, default `standard`).
  A tool reads the variant of its flow type in its filament's `filament_flow_support`, else the
  first one (`get_config_idx`, Filament domain).
- Fork-only options, e.g. `filament_is_high_temperature` and `chamber_cooling_mode` (a start
  G-code variable: strong chamber cooling for a softening temperature ≤ 50 °C, weak ≤ 70 °C,
  else keep warm; high-temperature filaments keep warm).

## What Mica does

- `discovery::load_installed` picks the dialect from the files: a set whose presets use any
  `*_flow_support` key loads as `Dialect::Snapmaker240`, otherwise `Orca242`. So
  `--profiles-dir "/Applications/Snapmaker Orca.app/Contents/Resources/profiles"` (or the pinned
  fork) works with `mica profiles`, `slice` and `validate`; the GUI loads through the same path.
- Composition reduces each flow-variant option to the tool's selected segment and records the
  selected variant in `filament_flow_support` (step size 1), so later readers see one value per
  tool. Process variants follow the machine's `nozzle_volume_type` (see `orca/README.md`).
- Lists (`mica profiles filament --printer … --print …`, the GUI) filter by
  `compatible_printers`/`compatible_prints` and the conditions, as Orca does.
- Integer options keep the leading digits of a fractional value, like Orca's `istream >> int`:
  Polymaker's `temperature_vitrification = "59.8"` is 59.
- G-code: `chamber_cooling_mode` is set for the fork's start G-code, and filaments with
  `activate_air_filtration` turn the exhaust fan on (`M106 P3`) after the start and end G-code
  (OrcaSlicer additionally gates this on `support_air_filtration` and the per-phase switches).
- Options this Orca version does not define never fail a preset: Orca drops them on load, Mica
  keeps them as extras (one value per tool) that no G-code reads.

## Not implemented (affect G-code)

- Mixed flow types within one job are only as good as the project state supplied;
  the CLI always uses `standard`.
- `filament_tower_ironing_area` (fork prime-tower ironing), `filament_long_retractions_when_cut`
  / `filament_retraction_distances_when_cut` (cutter retraction overrides; no U1 template reads
  the cut-retraction variables), non-nil `filament_wipe_distance`, and the filament scarf-seam
  overrides (`filament_scarf_*`, all `none` in the U1 presets) are kept as extras only. No pinned
  U1 filament sets a value for these that would change a U1 cube today.
- Snapmaker Orca 2.4.0's CLI exits with a segmentation fault (139) at startup on this Mac, so the
  fork's results are checked against its source, not a live oracle.
