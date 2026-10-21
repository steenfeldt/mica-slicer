# Calibration generators

`mica calibrate` generates a test without an input STL. It uses the selected printer's normal
startup/shutdown templates, bed temperatures, filament diameter, flow multiplier and firmware
flavour. `--tool N` (one-based, default 1) chooses the tool that prints the test. Calibration
never edits or saves the selected profile. Save the measured value in a duplicate
filament/profile afterwards.

## Multi-tool printers (XL, U1)

On a printer with more than one tool, the test prints as a plate whose objects are all assigned
to tool N, the way OrcaSlicer and PrusaSlicer print a single-object plate set to extruder N
(`GCode::_do_export` / `GCodeGenerator::_do_export`: the first extruder of the tool ordering
becomes `initial_extruder`, and `is_extruder_used` marks it alone). The engine builds a single-tool
print plan (`PreparedPrint::single_tool`) around its own layers. The export then goes through the
ordinary multi-tool path, so the start templates do the rest:

- **XL:** every other tool is set to `M104 Tn S0`, and only tool N is preheated, picked up
  (`T{N-1} S1 L0 D0`), purged and primed. The others stay parked and cold.
- **U1:** `PRINT_START` runs Snapmaker's own sequence (its automatic feed check visits every
  head, then switches them all off), then heats and selects only `T{N-1}`.

The tested tool's vector entries supply nozzle, filament and limits. Overrides of per-tool
options (temperatures, `filament_max_volumetric_speed`, the U1's `nozzle_temperature`) change
only that tool's slot. Calibration temperature commands name the tool (`M104 S230 T2 ;
calibration temperature`); PA commands (`M572` / `SET_PRESSURE_ADVANCE`) apply to the active
tool. The layer height must suit every tool's nozzle, as for any multi-tool print. A single-tool
printer (CORE One) keeps the plain single-object export.

Evidence on 2026-10-17 (`--tool 3`, XL PA tower): `M104 T0 S0`, `M104 T1 S0`, `M104 T3 S0`,
`M104 T4 S0`, `M104 T2 S170`, `T2 S1 L0 D0`, `M109 T2 S170`, …, `T2 S1 L0 D0; pick the tool`,
`M572 S0.036`, `M109 S230 T2`, then the `M572 S… ; calibration pressure advance` sweep. Same
test on the U1: `M104 T2 S140`, `T2`, the feed check, `M104 S0 T0..T3 A0`, `M104 T2 S130`,
`M109 T2 S130`, …, `T2`. OrcaSlicer itself slices the saved U1 calibration plate on `T2`.


```sh
export PATH="/opt/homebrew/opt/rustup/bin:$PATH"
cargo build --offline --release -p mica-cli
./target/release/mica calibrate flow --pass 1 -o /tmp/flow.gcode
./target/release/mica calibrate pa-line --start 0 --end 0.08 --step 0.004 -o /tmp/pa.gcode
./target/release/mica calibrate pa-pattern --start 0.02 --end 0.04 --step 0.002 -o /tmp/pattern.gcode
./target/release/mica calibrate pa-tower --start 0 --end 0.1 --step 0.002 -o /tmp/pa-tower.gcode
./target/release/mica calibrate temperature --start 230 --end 200 --step 5 -o /tmp/temp.gcode
./target/release/mica calibrate retraction --start 0 --end 2 --step 0.1 -o /tmp/retract.gcode
./target/release/mica calibrate max-volumetric-speed --start 5 --end 30 --step 0.5 -o /tmp/volume.gcode
```

Printer selection uses the same options as `slice`. CORE One defaults to the HF0.4 SPEED/Prusament
PLA combination. To use the other printers, append one of these sets of arguments:

```sh
--printer 'Original Prusa XL - 5T Input Shaper 0.4 nozzle' \
--print '0.20mm SPEED @XLIS 0.4' --filament 'Prusament PLA @XLIS'

--vendor Snapmaker --printer 'Snapmaker U1 (0.4 nozzle)' \
--print '0.20 Standard @Snapmaker U1 (0.4 nozzle)' \
--filament 'Snapmaker PLA @U1' --bed 'High Temp Plate'
```

`-o` is required. `.gcode` forces text; `.bgcode` works for Buddy printers. `--bundle` and
`--profiles-dir` select explicit profile installations. `--speed N` sets the fast PA speed (mm/s).
Without it, PA uses Orca's suggested speed: floor of the smaller of
`max(100, external_perimeter_speed)` and filament volumetric limit divided by bead cross-section.
An unset volumetric limit does not cap that speed. Line tests additionally enforce a fast/slow gap
of at least 5 mm/s; slow speed is `max(10, fast/10)`.

Ranges require finite, nonnegative endpoints and a positive step. Temperature descends; other
ranges ascend. PA values are seconds. A range may contain at most 100 samples, must fit a rectangular
bed with a 3 mm margin, and must fit the printer's height (at most 5000 layers). Non-divisible PA
line/pattern ranges round the count **up**, as Orca does, so the last sample can exceed `end`.
Use divisible ranges for an exact final value. Flow uses `--pass`, not range arguments.

## Reading flow patches

The classic Orca two-pass percentage method is implemented, rather than YOLO's additive correction.
Pass 1 has nine patches: **−20, −15, −10, −5, 0, +5, +10, +15, +20%**. Pass 2 has ten:
**−9 through 0%**, in 1% steps. These sets match the object names in Orca's pass 1/pass 2 3MF assets.

Patches are 30 × 24 mm, ten layers high, with one wall, two bottom layers, three intermediate
layers at 35% rectilinear infill, and five top layers. Layer height is half the nozzle diameter;
solid and top bead widths are 1.2 times the nozzle diameter. Skin speed is capped for the largest
multiplier in the pass. The chosen filament's existing flow multiplier remains in effect:
`test multiplier = existing multiplier × (1 + patch_percent/100)`.

**Patch labels are omitted.** Viewed from above with bed +Y away from you, read left to right,
then front to back. Each row has three patches; pass 2 has a tenth patch at the left of the back
row. Values ascend in that order. The first row is closest to bed Y=0. G-code comments also record
`; calibration sample=<percent>` at the first path of every patch on every layer. Keep the bed
orientation marked when removing patches.

Choose the smoothest top surface, with adjacent lines touching but without ridges or open gaps.
Update the filament's **extrusion multiplier / flow ratio** with
`new = old × (100 + selected_percent)/100`. Run pass 2 using a filament profile containing the
pass 1 result, then apply the same formula again. Do temperature first and PA afterwards.

## Reading pressure advance

Buddy commands are `M572 S<seconds>`; Klipper commands are
`SET_PRESSURE_ADVANCE ADVANCE=<seconds>`. Commands are emitted after startup/filament macros, so
those macros cannot overwrite the tested values. PA resets to zero after a PA test, before the
normal end templates. A subsequent print must set its filament PA as usual.

- **Line:** one layer, 3.5 mm between rows; 20 mm slow, 40 mm fast, 20 mm slow. Values ascend in
  bed +Y, starting at the front row. Vertical anchor lines bracket the test. Choose the row with
  the least bulge or thinning where the speed changes, then narrow the range around that value.
- **Pattern:** four layers of three nested 90° chevrons per value, with 30 mm sides and 2 mm gaps
  between groups. Values ascend in bed +X. Layer 1 runs at 30 mm/s with an anchoring frame; layers
  2–4 use the fast speed. Choose the group with crisp corners without a bulge, gap or rounded tip.
- **Tower:** an open rectangular tower, two walls and no infill/top/bottom. PA is
  `start + floor(print_z_mm) × step`. `step` is PA **per millimetre**, not per layer. Height is
  `ceil((end-start)/step)+1` mm, matching Orca's final full band. Choose the height where the
  corners become sharp without thinning. Read Z from the preview or measure from the bed.

There are no printed PA numerals; use position, the formula, and the sample comments. Store the
chosen value in the filament's **pressure advance** setting (and enable pressure advance on
Klipper). Prusa filament start G-code currently owns Buddy PA: update its `M572 S…` command; the
resolved `PrintSettings.pressure_advance` field is not an active Buddy profile override.

## Reading the temperature tower

Two posts support a 20 mm bridge in each 10 mm block for a 0.4 mm nozzle. The geometry scales with
nozzle diameter. Temperatures are embossed on the front in a small, built-in seven-segment font.
The default range is 230–200°C. The generator accepts descending 5°C steps in 150–350°C,
restricted further to the selected filament's temperature limits. If the profile has no explicit
limits, these are the smaller normal/initial temperature minus 30°C and the larger plus 30°C,
matching validation. Choose a range suitable for the filament and printer. It sets both initial and normal nozzle temperature to `start` before startup, including
Orca's typed source keys used by the U1's templates. `M104` changes temperature without waiting
between blocks, as Orca does.

The exact schedule follows pinned Orca `GCode::interpolate_value_across_layers`: for zero-based
layer `i <= 1`, use `start`. Afterwards, using f32 arithmetic,
`temperature = trunc(((start+5) + i/(layer_count-1) × (end-start-5))/5) × 5`.
That makes the boundaries slightly different from exact geometric 10 mm multiples. The embossed
number is the nominal block temperature; the emitted `M104` and sample comment are authoritative
at a boundary. A single-temperature range remains constant.

Compare bridging sag, stringing between posts, surface consistency and layer bonding. Pick a
temperature that balances these for the intended part; a brittle but pretty sample is not enough.
Update the filament's **normal nozzle temperature**, and its **first-layer temperature** as needed.

## Reading retraction

Two hollow posts separated by a 28 mm gap force repeated travel/retraction. A two-layer base joins
them; posts have two walls, three bottom layers, no infill and no top. Firmware retraction and wipe
are disabled for this test: the writer changes actual retract/unretract E distances.

Orca's schedule is `length = start + floor(max(0, print_z_mm - 0.4)) × step`. The initial section
therefore stays at `start` until approximately Z=1.4 mm, then each millimetre adds `step`.
Floating-point rounding at an exact boundary follows the engine's actual unrounded print Z.
A full final layer (particularly with Orca's layer policy) can touch the next band and emit one
extra step above `end`, just as the source height formula allows; read the final sample comment.
Height is `1.4 + (end-start)/step - epsilon`, preserving Orca's final band. Choose the shortest
length that removes strings without gaps after a travel. Save it as the filament's **retraction
length override**, or printer retraction length if that filament has no override. Retraction speed
continues to come from the selected profile.

## Reading maximum volumetric speed

An open rectangular single-wall tower gives long straight runs. Width is up to 160 mm and adapts
to the bed. Bead width is `1.75 × nozzle` and height is `0.8 × nozzle` (including layer 1).
A final partial layer follows the engine's height policy; its speed uses the actual cross-section.
The volumetric schedule is continuous: `Q = start + print_z_mm × step` in mm³/s. Requested speed is
`round(Q / (bead_area × existing_filament_flow_multiplier))` mm/s. Bead area uses Mica/Prusa's rounded
rectangle model. The step is **(mm³/s)/mm**, not a per-layer change. Height is
`(end-start+1)/step`, as in Orca, so the last part intentionally continues about 1 mm³/s beyond `end`.

Cooling slowdown and the normal filament/global volumetric caps are disabled. Physical firmware
speed and acceleration limits still apply: a short wall or high requested speed can fail to reach
the target flow. Look for the first sustained roughness, gaps, loss of gloss or weak extrusion;
choose a lower value with margin and save it as filament **max volumetric speed**. Sample comments
record Q before integer speed rounding. Existing flow multiplier matters to the Q-to-speed conversion.

## Engine API and scope

```rust,ignore
use mica_core::calib::{self, Kind, Parameters};
let params = Parameters::new(Kind::PressureAdvanceLine);
let generated = calib::generate(&params, &resolved_config)?;
// generated.objects: named Rust-built meshes and relative flow multipliers
// generated.overrides: canonical config values (plus required Orca template keys)
// generated.samples: values in spatial order
let print = generated.prepare(&resolved_config)?;
mica_gcode::export::export_file(
    &print, &print.config, &print.settings, "calibration",
    Some(&generated.combined_mesh()), output_path,
)?;
```

`CalibrationPrint` implements `LayerSource`; its path-boundary `Control` carries optional PA,
temperature, retraction, relative flow and requested speed. Ordinary sources return `None` and
use the original export path. Towers/patches pass through normal mesh slicing; line/pattern PA
paths are generated directly. Calibration preparation keeps these bounded, small test toolpaths
in memory; it does not change ordinary slicing's streaming behaviour.

Adapted from **orca@8500fcdcca**:

| Behaviour | Source relative to the Orca checkout |
|---|---|
| Mode/parameters | `src/libslic3r/calib.hpp::CalibMode`, `Calib_Params` |
| PA line width, speeds and trajectory | `src/libslic3r/calib.cpp::CalibPressureAdvanceLine::print_pa_lines`, constructor, `CalibPressureAdvance::find_optimal_PA_speed` |
| PA pattern values, four layers, three chevrons | `src/libslic3r/calib.cpp::CalibPressureAdvancePattern::generate_custom_gcodes` |
| Flow pass geometry/settings and relative multiplier | `src/slic3r/GUI/Plater.cpp::adjust_settings_for_flowrate_calib`, `calib_flowrate`; `resources/calib/filament_flow/flowrate-test-pass{1,2}.3mf` object names |
| Tower heights/config | `src/slic3r/GUI/Plater.cpp::_calib_pa_tower`, `calib_temp`, `calib_retraction`, `calib_max_vol_speed` |
| Per-height values and temperature rounding | `src/libslic3r/GCode.cpp::GCode::process_layer`, `interpolate_value_across_layers` |
| GUI/device mode naming (reference only) | `src/slic3r/Utils/CalibUtils.cpp::get_calib_mode_name` |

Simplifications: procedural rectangular patches/towers replace Orca's detailed bundled models;
the temperature tower tests bridges and stringing but omits Orca's additional overhang sculptures.
PA line anchors print before/after the rows instead of Orca's first-row anchor ordering. Pattern
uses a single frame, omits Orca's first-layer shrink, number tab, trick line and multi-speed/
acceleration matrix. Patch/PA printed labels are omitted; temperature labels are original Rust-built
stroke solids. No bundled STLs, fonts, external data or new dependencies. YOLO/additive flow,
Bambu/device auto calibration, VFA, input shaping, GUI menu and automatic profile saving are outside
this API. (The desktop GUI's Calibrate menu, plates and result-to-profile step are built on it:
see [gui-calibration.md](gui-calibration.md).) Firmware PA is specifically Buddy/Klipper; other Marlin variants are not supported.

## Verification

`cargo test --offline -p mica-core -p mica-gcode calib --lib` checks sample sets, PA trajectories,
boundary schedules, invalid ranges, outward mesh winding, typed Orca template overrides, actual
extrusion/feedrate changes, startup retraction accounting and firmware commands.
`python3 tests/calibration/check.py target/release/mica` generates both flow passes plus all six
other methods on CORE One, then XL and U1 on tool 1 and tool 3: 40 files, each passed to
`mica validate --json`. On the XL and U1 it also checks that the chosen tool is the only one
selected, and that every other tool's temperature command is `S0`. It
compares emitted sample schedules/PA/temperature commands and actual retraction E moves against
independent expectations. All artifacts go to the system temporary directory.

`cargo fmt --all`, workspace Clippy with `-D warnings`, `cargo test --offline --workspace`
(with the pinned Prusa profiles) and `cargo xtask provenance` all pass.

Verification on 2026-10-16: the 24-case sequence matrix, 21 default tests (seven kinds × three
printers), and Buddy binary PA outputs for CORE One/XL all validated with **0 errors** (47 files).
Temperature bridges produce expected layer-height warnings; the 0.32 mm volumetric test height
exceeds the unmodified CORE One/XL profiles' 0.30 mm advertised maximum and also produces warnings.
PA, flow and retraction outputs have no warnings. The quick corpus passes (11 cases, no failures),
and `cmp` confirms byte-identical ordinary cube20 and 3DBenchy output before/after on both CORE One
and XL.

The installed **OrcaSlicer 2.4.1** pass 1/pass 2 assets have identical percentage sample names.
Its `--help` exposes slicing but no calibration
generation/mode/start/end/step switches. Loading a stock calibration mesh alone does not invoke
Plater's calibration setup or GCode's calibration mode. Therefore the oracle here is the pinned
code and the flow assets' sample names, rather than an invented CLI-equivalence claim. The installed
binary is 2.4.1; the pinned source identifies itself as 2.4.2. No multi-tool Orca slicing run
was attempted, avoiding the known ToolOrdering crash.

Review the physical usefulness of the simplified geometry, temperature boundary interpolation and
PA corner behaviour with actual prints. Automated validation establishes command/geometry safety
and schedule correspondence, not measured calibration accuracy.
