# Snapmaker U1 comparison fixtures

`project_settings.config` is `Metadata/project_settings.config` from a fresh project with
only stock presets, saved by the Snapmaker Orca **2.4.0** macOS CLI
(`/Applications/Snapmaker Orca.app`, CFBundleShortVersionString 2.4.0, CLI banner
`Snapmaker_Orca-01.10.01.50`), copied without modification. Selection: printer
`Snapmaker U1 (0.4 nozzle)`, process `0.20mm Standard @Snapmaker U1 (0.4 nozzle)`,
`Generic PLA` in all four slots, and the printer's `default_bed_type`
(Textured PEI Plate) as a new project uses. The model is `tests/models/cube20.stl`.

The CLI needs flattened preset JSON, so each installed preset's `inherits` chain
(Snapmaker plus OrcaFilamentLibrary manifests in the app's resources, as in
`scripts/prepare-u1-oracle.py`) was merged into a temporary `machine.json`, `process.json`,
and `filament.json`, with `curr_bed_type` set from `default_bed_type`.
`wipe_tower_filament` was dropped from the process input because loading it makes the
2.4.0 CLI segfault; the saved value equals both the preset and the built-in default (0).
The twelve selected chain files (listed below) are byte-identical in the installed
app and the `snapmaker-orca@b1831e5dcb` pin. The run, from an empty system temporary directory with an empty
`--datadir`:

```sh
"/Applications/Snapmaker Orca.app/Contents/MacOS/Snapmaker_Orca" --datadir "$tmp/data" \
  --load-settings "$tmp/machine.json;$tmp/process.json" \
  --load-filaments "$tmp/filament.json;$tmp/filament.json;$tmp/filament.json;$tmp/filament.json" \
  --export-3mf "$tmp/cube.3mf" /absolute/path/to/tests/models/cube20.stl
```

It has 584 keys. `snapmaker240_differences.json` records **every** nonmatching key across
the union of the project and composed config, its exact resolved value (null = absent),
and an individual reason. The test checks 570 matching project keys, 14 differing
project keys, and 23 additional retained keys. It fails on new differences,
changed resolved values, or stale exemptions. No blanket skip list is used.

The project fixture, difference values, and comparison report contain **AGPL-3.0 profile/default
data**, derived from these pinned repositories (not copied implementation files):

- `snapmaker-orca@b1831e5dcb`, `src/libslic3r/PrintConfig.cpp`, `PrintConfig.hpp`,
  `Preset.cpp`, `PresetBundle.cpp` (built-in defaults and serialization).
- `snapmaker-orca@b1831e5dcb`, `resources/profiles/Snapmaker.json` and the selected
  preset inheritance chains under `resources/profiles/Snapmaker/` listed below.
- `orca@8500fcdcca`, the same source paths, `resources/profiles/Snapmaker.json`,
  `resources/profiles/OrcaFilamentLibrary.json`, and their selected chains below.

`orca242_comparison.md` is a captured report only. It compares the two composed vendor
configs and does not read the project fixture. Its differences are printed
again by `report_orca242_versus_snapmaker240`, without assertions on drift.
Tests read the vendor profiles from the provided absolute upstream paths and
skip gracefully when the respective checkout is absent. Mapping the saved
project fixture and synthetic importer tests always run.

The project-only bed selection is intentionally not injected into the vendor
comparison. `full_config_with_project` is available when a caller has plate
state; plain `full_config` uses Orca's built-in Cool Plate default. The saved
project mapping test separately verifies all four Textured PEI bed temperatures.

## Profile data source paths

snapmaker-orca:

- `resources/profiles/Snapmaker/machine/Snapmaker U1 (0.4 nozzle).json`
- `resources/profiles/Snapmaker/machine/fdm_U1.json`
- `resources/profiles/Snapmaker/machine/fdm_toolchanger.json`
- `resources/profiles/Snapmaker/machine/fdm_klipper.json`
- `resources/profiles/Snapmaker/process/0.20mm Standard @Snapmaker U1 (0.4 nozzle).json`
- `resources/profiles/Snapmaker/process/fdm_process_U1_0.20.json`
- `resources/profiles/Snapmaker/process/fdm_process_U1_common.json`
- `resources/profiles/Snapmaker/process/fdm_process_U1.json`
- `resources/profiles/Snapmaker/filament/Generic PLA.json`
- `resources/profiles/Snapmaker/filament/Generic PLA @U1 base.json`
- `resources/profiles/Snapmaker/filament/fdm_filament_pla_category.json`
- `resources/profiles/Snapmaker/filament/fdm_filament_common.json`

orca:

- `resources/profiles/Snapmaker/machine/Snapmaker U1 (0.4 nozzle).json`
- `resources/profiles/Snapmaker/machine/fdm_U1.json`
- `resources/profiles/Snapmaker/machine/fdm_toolchanger.json`
- `resources/profiles/Snapmaker/machine/fdm_klipper.json`
- `resources/profiles/Snapmaker/process/0.20 Standard @Snapmaker U1 (0.4 nozzle).json`
- `resources/profiles/Snapmaker/process/fdm_process_U1_0.20.json`
- `resources/profiles/Snapmaker/process/fdm_process_U1_common.json`
- `resources/profiles/Snapmaker/process/fdm_process_U1.json`
- `resources/profiles/OrcaFilamentLibrary/filament/Generic PLA @System.json`
- `resources/profiles/Snapmaker/filament/fdm_filament_pla.json`
- `resources/profiles/Snapmaker/filament/fdm_filament_common.json`
