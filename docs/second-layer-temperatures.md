<!-- SPDX-License-Identifier: AGPL-3.0-only -->
<!-- Provenance: adapted -->
<!-- Source: prusaslicer@b028299c77 src/libslic3r/GCode.cpp :: GCodeGenerator::process_layer, GCodeGenerator::_print_first_layer_bed_temperature -->
<!-- Source: prusaslicer@b028299c77 src/libslic3r/GCode/GCodeWriter.cpp :: GCodeWriter::set_bed_temperature -->
<!-- Source: orca@8500fcdcca src/libslic3r/GCode.cpp :: GCode::process_layer, GCode::get_highest_bed_temperature, GCode::_print_first_layer_bed_temperature -->
<!-- Source: orca@8500fcdcca src/libslic3r/GCodeWriter.cpp :: GCodeWriter::set_bed_temperature, GCodeWriter::enable_power_loss_recovery -->
<!-- Changes: Document the pinned second-layer contract, Mica fixes, offline reference comparison and regression checks. -->

# Second-layer temperature transition

## Exact upstream behavior

PrusaSlicer `b028299c770b8380ee81c921a2867d522f288123`,
`src/libslic3r/GCode.cpp:2702–2727`, `GCodeGenerator::process_layer`:

```cpp
if (! first_layer && ! m_second_layer_things_done) {
    for (const Extruder &extruder : m_writer.extruders()) {
        if (print.config().single_extruder_multi_material.value || m_ooze_prevention.enable) {
            if (extruder.id() != m_writer.extruder()->id())
                continue;
        }
        // Emit the normal temperature only if positive and different from first_layer_temperature.
    }
}
```

Orca `8500fcdccaa10b5099ac20d252af3a7c560046f1`,
`src/libslic3r/GCode.cpp:4777–4823`, `GCode::process_layer`, uses the same guard:

```cpp
if ((print.config().single_extruder_multi_material.value || m_ooze_prevention.enable) &&
    extruder.id() != m_writer.filament()->id())
    continue;
```

Its temperature keys are `nozzle_temperature` and `nozzle_temperature_initial_layer`,
mapped to Mica's canonical `temperature` and `first_layer_temperature`. Both upstream
initializers enable ooze prevention with `ooze_prevention && !single_extruder_multi_material`
(Prusa line 776; Orca line 2282). The combined skip condition is therefore equivalent
to checking the two configuration booleans. Mica still rejects unsupported multi-tool MMU jobs.

The active tool is the tool left selected by layer 1, before layer 2's first toolchange.
The transition must skip parked tools entirely, even when their configured normal and
first-layer temperatures differ. Mica previously emitted an extra standby-temperature
`M104` for each such parked tool. The active tool receives its positive **normal** target;
equal first-layer/normal targets and zero normal nozzle targets emit nothing.

## Other behavior reviewed

| Behavior | Upstream contract and Mica action |
|---|---|
| Prusa bed selection | Use layer 2's first printing tool (`layer_tools.extruders.front()`), or the one-based `bed_temperature_extruder` if in range. Fix Mica's use of the previous layer's active-tool settings. |
| Orca bed selection | `by_first_filament` uses the first printing tool; `by_highest_temp` (Orca's default) uses all slice-used filaments, excluding unused configured tools. Canonical bed vectors already reflect the selected plate type. Implement both modes. |
| Bed zero | Both call `set_bed_temperature` unconditionally, including zero. Fix Mica's positive-only guard. |
| Bed deduplication | Both writers suppress a target equal to their remembered bed target. Prusa's `_print_first_layer_bed_temperature` records a parsed start-G-code target when autoemit is enabled. Orca's equivalent override is disabled by `#if 0`, so its remembered target is the planned first-layer target. Preserve this distinction. Use the same bed-selection helper for startup and layer 2. |
| Chamber | Neither second-layer block sets chamber temperature. Preserve Mica's absence of a second-layer chamber command. Startup/shutdown chamber control is outside this transition. |
| Orca acceleration/jerk | Upstream resets first-layer motion settings here. Mica already selects normal acceleration and Orca jerk from each non-first-layer path; no new thermal fix is needed. |
| Orca power-loss recovery/inspection | The recovery writer emits nothing for Klipper, and inspection is Bambu-only. Neither adds commands to the supported U1/Klipper path. |

## Offline reference evidence

The local PrusaSlicer 2.9.6 executable successfully sliced an XL two-object job using T0/T1,
ooze prevention, first-layer temperatures `220,230,240,240,240`, and normal temperatures
`200,210,215,215,215`. Parked temperatures used a standby delta of -30 with nullable idle targets.
Mica exported the Prusa-flavour project, preserving the two object/tool assignments.

```sh
./target/release/mica slice tests/models/cube20.stl tests/models/cube20.stl \
  --printer 'Original Prusa XL - 5T Input Shaper 0.4 nozzle' \
  --print '0.20mm SPEED @XLIS 0.4' --filament 'Prusament PLA @XLIS' \
  --object-extruder 2=2 --set ooze_prevention=1 \
  --set first_layer_temperature=220,230,240,240,240 \
  --set temperature=200,210,215,215,215 --set wipe_tower=0 \
  --set thumbnails= --set binary_gcode=0 --set standby_temperature_delta=-30 \
  --set idle_temperature=nil,nil,nil,nil,nil \
  --export-3mf "$ref/xl-two-tools.3mf" --flavour prusa \
  -o "$ref/mica-two-tools.gcode"

# The reference app and datadir live under out/ref (see quality-safety.md):
out/ref/PrusaSlicer-2.9.6.app/Contents/MacOS/PrusaSlicer \
  --datadir out/ref/datadir --export-gcode "$ref/xl-two-tools.3mf" \
  --ooze-prevention --no-binary-gcode -o "$ref/prusa-two-tools.gcode"
```

On layer 2, the reference emits exactly one plain nozzle `M104`:

```gcode
M104 S210 T1 ; set temperature
```

Before the fix Mica also emitted `M104 S170 T0 ; set temperature`. XL's separate
`M104.1` preheat scheduling and subsequent `M109` toolchange commands are outside the
second-layer transition and are excluded from this comparison.

After rebuilding, Mica's layer-2 plain `M104` lines exactly matched the reference above.
A second reference run with `--bed-temperature 0,0,0,0,0` emitted
`M140 S0 ; set bed temperature` on layer 2.

`crates/mica-gcode/tests/second_layer.rs` covers Prusa/Marlin and Orca/Klipper output
with two tools on an XL-sized bed, both idle-temperature modes, ooze disabled, equal/zero
nozzle targets, layer-1 active versus layer-2 first-tool selection, explicit Prusa bed-tool
override (including unused and invalid tools), Orca bed formulas, zero bed target, start
temperature deduplication, and unchanged chamber behavior. The nozzle and bed regressions
failed on the original exporter; the ooze-disabled control already passed.

The four default G-code SHA-256 prefixes are `d4fc843c` (CORE cube), `0e782b80`
(CORE Benchy), `139f8059` (XL cube), and `4532ea45` (XL Benchy). Fresh release outputs
matched these prefixes and compared byte-for-byte equal to the pre-change baselines.
