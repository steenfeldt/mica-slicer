# GUI: the Calibrate menu

Tuning a new filament from the desktop GUI, on top of the headless generators in
[calibration.md](calibration.md). Three steps, no modal dialogs, and the user's profiles are
not touched until a result is saved. Code: `crates/mica-gui/src/calibration.rs` (the model:
tests, defaults, the estimate, reading a result, the per-firmware setting),
`panels/calibration.rs` (the cards and the painted pictograms), `worker.rs` (`Job::Calibration`),
`plates.rs` (`Plate::calibration`).

## 1. Calibrate › a test

The **Calibrate** menu lists Flow rate (pass 1, pass 2), Pressure advance (line, pattern,
tower), Temperature tower, Retraction test and Max volumetric speed, with the order to run them
for a new filament in its footer. Each entry opens a small non-modal card:

- One plain paragraph: what the test prints, what to look for and when to run it.
- A painted pictogram of the print with the defect at each end and the "best" sample marked.
- The parameters, with defaults from the selected filament and printer: the PA ranges are
  centred on the filament's current pressure advance (the `M572 S…` of the Prusa filament
  start G-code for the nozzle in use, or Orca's `pressure_advance`); the temperature tower runs
  from +15 °C to −20 °C around the nozzle temperature, within the filament's limits; retraction
  up to twice the current length; volumetric speed from half to 1.6 × the current limit. Tower
  tests show the step *per millimetre of height*. PA tests have the engine's suggested fast speed
  or a typed one. Flow has the pass. On multi-tool printers a *Filament to calibrate* dropdown
  chooses the tool whose filament profile is tested and saved. The test prints on that tool
  (`mica calibrate --tool N` does the same): only it is picked up and heated, the other tools
  stay parked and cold, and the card says so.
- A live summary from the generated toolpaths: `Prints 51 bands · about 8 min · 6.2 g · 51 mm
  tall`, or the engine's reason when the range does not fit the bed or the printer's height.

## 2. Create plate and slice

*Create plate and slice* puts the engine's objects on a **new plate named after the test and the
filament** (`PA tower · Extrudr GreenTEC Pro`), locked (its layout is the test's), and slices
it. The plate carries a `PlateJob` (test, parameters, tool, filament, samples, the engine's
overrides); when it is sliced the worker runs `Job::Calibration`: `calib::generate` +
`Generated::prepare` on the plate's own configuration for the chosen tool
(`Parameters::tool`), exported through the engine's `CalibrationPrint` source, so the
per-height PA / temperature / retraction / flow controls and the `; calibration sample=`
comments are in the G-code exactly as from the CLI (`worker::calibration_plate_exports_like_the_cli`
asserts the text is identical). The 32-odd engine overrides apply to that job only; the sidebar
and the other plates keep the user's settings. Export / Print… work as for any plate.

## 3. Enter result → Save to filament profile

The plate's card (top right, where the tool card sits) shows the print's totals, the pictogram
and what to look for, then **Enter result**: the best patch / row / group number, the height in
mm, or the embossed temperature. The computed value appears at once with its formula
(`0 + 21 mm × 0.002 per mm = 0.042`), and the card explains which setting changes:

| Test | Reading | Setting written (user filament profile) |
|---|---|---|
| Flow pass 1 / 2 | patch 1–9 / 1–10 | `extrusion_multiplier` = old × (100 + %) / 100 |
| PA line / pattern / tower | row / group / height | Prusa Buddy: the `M572 S…` value for the nozzle in `start_filament_gcode` (other nozzle branches kept; a profile without one gets a later `M572` line); Klipper: `pressure_advance` + `enable_pressure_advance = 1` |
| Temperature | block °C | `temperature`, and `first_layer_temperature` keeping its offset (switch) |
| Retraction | lowest clean height | `filament_retract_length` (the filament override) |
| Max volumetric speed | height where it turns rough | `filament_max_volumetric_speed`, 10 % below the flow at that height |

**Save to filament profile** writes a Mica user filament profile (TOML in the profile store,
`inherits` = the system preset, only the changed keys; an existing user profile is updated in
place, a system preset gets `<name> tuned`), selects it for the tool, drops any unsaved sidebar
edit of the same key, and shows the confirmation with the next test to run. The system preset
is never written. Orca-only keys such as `pressure_advance` are now typed when a user profile
is applied (`mica_profiles::orca::set_option_text`) and composed per tool, so the U1's exporter
sees them.

## Screenshots (`out/work/gui12-*.png`)

```sh
B=./target/release/mica-gui; U1="Snapmaker U1 (0.4 nozzle)"
$B --screenshot out/work/gui12-menu.png tests/models/cube20.stl --click 233,24 --pointer 240,180
$B --screenshot out/work/gui12-pa-tower-card.png tests/models/cube20.stl --calibrate pa-tower
$B --screenshot out/work/gui12-flow-card.png --calibrate flow1
$B --screenshot out/work/gui12-temperature-card.png --calibrate temperature
$B --screenshot out/work/gui12-pa-tower-plate.png --calibrate pa-tower --calibrate-create
$B --screenshot out/work/gui12-pa-tower-preview.png --calibrate pa-tower --calibrate-create --preview
$B --screenshot out/work/gui12-u1-plate.png --printer "$U1" --calibrate pa-tower:2 --calibrate-create
$B --screenshot out/work/gui12-result.png --calibrate pa-tower --calibrate-create --calibrate-result 21.7
$B --screenshot out/work/gui12-saved.png --calibrate pa-tower --calibrate-create --calibrate-result 21.7 --calibrate-save
```

New flags: `--calibrate TEST[:TOOL]` (`flow1`, `flow2`, `pa-line`, `pa-pattern`, `pa-tower`,
`temperature`, `retraction`, `volumetric`), `--calibrate-create` (the plate, sliced
synchronously), `--calibrate-result V`, `--calibrate-save` (into the throwaway store).

## Tests

- `mica-gui::calibration`: `M572` reading and editing (the nozzle branch only, a plain command,
  no branch, no command, a commented one), defaults from the filament (every test generates),
  the estimate's plausibility, every result → setting per firmware (flow %, PA tower formula and
  clamping, PA line index, Klipper vs Buddy changes, temperature snapping and the first-layer
  offset, retraction schedule, volumetric margin), a second tool tested with its own nozzle,
  filament and current values; the saved job reads back exactly.
- `mica-gui::panels::calibration`: the CORE One round trip (create → locked named plate with the
  job and untouched global settings → reading → a one-key user profile inheriting the system
  preset, selected, printing with 0.042 while the preset keeps 0.036 → saving again updates the
  same profile → undo removes the plate); every card opens with an estimate; flow and
  temperature save their keys into one profile.
- `mica-gui::worker`: the calibration job's G-code equals the engine's own export of the same
  parameters, with 51 sample comments and the `M572` sweep; the binary file and preview exist.
- `screenshot`: the new flags parse.
- `mica-gui::save`: a calibration plate for XL tool 3 survives Save/Open in both flavours (test,
  tool, parameters, samples, overrides and the typed reading); the reopened project is
  unmodified and reads the same result. `mica-core/tests/threemf_write.rs` checks that the
  `mica_` plate key is only in `Metadata/mica.json`.
- `mica-gui::regressions`: a U1 temperature tower for tool 2's PETG prints on T1 alone and
  validates against the plate's configuration.

## Saving calibration plates in a project

A project keeps which plates are calibration plates. Each one's `PlateJob` (test, tool,
parameters, samples, overrides, filament, profile name, the typed reading and the saved
message) is stored as JSON under the plate metadata key `mica_calibration`. The native plate
metadata (Orca's `model_settings.config`) skips `mica_` keys, so the value only goes into Mica's own
`Metadata/mica.json`. OrcaSlicer and PrusaSlicer ignore that entry. Opening the project restores
the job and strips the key from the plate's metadata. A file that another slicer saved again
loses the sidecar (its manifest no longer matches), and its plates open as ordinary locked
plates.

Checked on 2026-10-17: OrcaSlicer 2.4.1 (`--slice 2`, clean `--datadir`) slices the saved U1
calibration plate on T2, as Mica does, and PrusaSlicer 3.0.0-alpha11 exports the XL Prusa-flavour
project. OrcaSlicer's CLI refuses the XL projects with or without a calibration plate, with the
same error codes (-5 Orca flavour, -51 Prusa flavour), so the calibration plate is not the cause.

## Left for later

- The estimate ignores acceleration; the header shows the exporter's numbers once sliced.
- Orca's YOLO flow test, VFA and input-shaping tests are not offered (not in the engine).
