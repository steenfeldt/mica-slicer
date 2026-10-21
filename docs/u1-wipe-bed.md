# U1 wipe retraction and second-layer bed temperature, 2026-10-19

These changes apply under OrcaSlicer rules. No CLI or profile options were added.
The CORE One and XL Prusa-rules outputs remain byte-identical.

`orca_motion.rs` retains original boundary-segment endpoints in its bounded wipe cache,
then replaces the first vertex with the actual position before clipping. Previously the
cache itself clipped the segment; a seam-clipped loop then wiped along a different slope.
The retraction split uses the configured total and percentage, the filament retraction
rate, the pre-cooling wipe speed and the unrounded wipe-distance denominator. A negligible
wipe capacity leaves the remainder for the final retraction, as upstream does. Integer
clipping truncates coordinates like `Polyline::clip_end`.

`extrusion_speed.rs` uses float32 path volume and includes the filament flow ratio in Orca's volumetric speed limit,
following `GCode::_extrude`. This is needed for role-based wipe speed as well as extrusion:
with the U1 PLA ratio of 0.98, the old nominal speed was 2% too low and shifted retraction
from before the wipe into the wipe. Other role flow-ratio options were not added here.

For example, a native cube20 wall departure retracts 0.16651 mm before its wipe and
0.63349 mm during it, ending at X143.290 Y145.754. A regression supplies that exact
path and speed and compares every emitted line to Orca 2.4.1. The old output at this
departure retracted 0.15358 + 0.64642 mm and ended at Y145.790.

`export.rs` now reflects Orca's Klipper startup behavior: `_do_export` skips
`_print_first_layer_bed_temperature`, so the writer's remembered target stays at zero
regardless of custom startup M140/M190 commands. Layer 2 selects the other-layers target
and emits `M140 S55 ; set bed temperature` for this U1 job. The command follows the
layer macro, `;_SET_FAN_SPEED_CHANGING_LAYER` and default-acceleration reset (and any
nozzle-temperature transition), before object start and travel. No M190 is added; the
profile's startup M190 is preserved. A zero other-layers target is deduplicated against
the initial zero for Klipper. Orca on other firmware retains its planned startup target;
Prusa keeps its existing custom-start target tracking.

## Native comparison

Fresh native OrcaSlicer **2.4.1** CLI runs succeeded for both STLs. Inputs were made by
`scripts/prepare-u1-oracle.py` from installed profiles, with four explicit copies of the
filament JSON, `--slice 0 --arrange 1`, fresh data directories and no extra setting edits.
Mica uses the U1 arguments in [u1-klipper.md](u1-klipper.md), with a temporary
`MICA_CONFIG_DIR`.

The counts below strip semicolon comments, trim whitespace and discard blank lines.
They count **deleted plus added lines** using Git's Myers diff with the indent heuristic
disabled; a replaced line counts twice. Retraction/wipe includes negative-E G1/G2/G3
commands and all commands inside WIPE_START/END (including feed-rate commands).
No M73, object-label, coordinate, orientation or speed normalization is applied.

| Model | All non-comment lines before → after | Retraction/wipe before → after | Negative-E moves only before → after |
|---|---:|---:|---:|
| cube20 | 14,070 → 13,497 | 1,401 → 649 | 1,007 → 427 |
| 3DBenchy | 199,668 → 198,209 | 14,616 → 14,100 | 12,023 → 11,787 |

These are improvements, **not whole-job move-for-move parity**. Native Orca arranges
Benchy with a 90-degree rotation; Mica's CLI command retains the source
orientation. Existing geometry,
ordering, segment speeds and wipe paths still differ, even on the cube. The exact
native-path regression verifies the split and endpoint behavior independently of those
differences. The layer-2 bed command's position and lack of an added wait match both
native outputs.

SHA-256 prefixes of text G-code:

| Printer | cube20 before → after | 3DBenchy before → after |
|---|---|---|
| CORE One | 65f08d88 → 65f08d88 | 98eba153 → 98eba153 |
| XL | cc06b5d7 → cc06b5d7 | 90fc179d → 90fc179d |
| U1 | 02c2be3a → 2ca74917 | e13da618 → 2c1e3b32 |

The four Prusa files also pass byte comparisons. All six final files validate with
zero errors. Reference inputs, commands, outputs, raw line extracts, `move-diffs.json`,
validation reports and slicing logs stay in a system temporary directory. The line counts
above come from ad hoc diff scripts (Git's Myers diff over the stripped line lists, as described
above) that are not in the repository.

## Verification and attribution

Unit coverage includes full/partial and rate-limited retractions, short paths, a
negligible-capacity wipe followed by the remaining E-only retraction, the native cube
sequence, volumetric flow-ratio caps, unchanged/changed/zero bed targets, firmware/rules
selection, exact second-layer command ordering and no additional M190 or later M140.

Final checks:

- `cargo fmt --all --check`: passed.
- `cargo clippy --offline --workspace --all-targets -- -D warnings`: passed.
- `cargo xtask provenance`: passed (549 files, 328 ported/adapted).
- Final G-code library tests: 130 passed, one ignored; all six second-layer tests passed.
- `MICA_PROFILE=fast cargo xtask corpus`: passed, 33 applicable slicing cases with zero
  errors, plus all 26 CLI/GUI parity tests (including Benchy).
- `cargo test --offline --profile fast --workspace --no-fail-fast`: everything passes except
  the socket tests of `mica-camera` and the file-event test of `mica-library`, which cannot
  pass in a restricted build environment (no loopback sockets, no file-system events).

The volumetric-cap calculation also changes Orca extrusion feedrates.

Adapted from `orca@8500fcdcca` `src/libslic3r/GCode.cpp`
(`Wipe::calculateWipeRetractionLengths`, `Wipe::wipe`, `GCode::_extrude`, `_do_export`,
`process_layer`), `GCodeWriter.cpp` (`retract`, `set_bed_temperature`) and
`Polyline.cpp` (`clip_end`) and `Flow.cpp` (`mm3_per_mm`). No dependencies, assets or source pins changed.

Commit trailers:

```text
Ported-From: orca@8500fcdcca src/libslic3r/GCode.cpp
Ported-From: orca@8500fcdcca src/libslic3r/GCodeWriter.cpp
Ported-From: orca@8500fcdcca src/libslic3r/Polyline.cpp
Ported-From: orca@8500fcdcca src/libslic3r/Flow.cpp
```
