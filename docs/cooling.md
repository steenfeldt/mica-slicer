# Cooling slowdown

The cooling solver and both exporter paths now support Prusa's `uniform_cooling`,
`consistent_surface`, and legacy `proportional` modes. **Full oracle acceptance is not
met yet.** The ordinary CORE One cube layers now retain 250 mm/s
inner walls, 200 mm/s external walls, and approximately 21 mm/s sparse infill, but
some solid/mixed-fill layers and Benchy role averages remain outside tolerance.

## API and CLI

`mica_gcode::cooling::CoolingConfig::from_config` resolves a single tool's settings.
`slow_down_multi(&mut [Move], fixed_time, &[CoolingConfig])` accepts source-ordered
extrusion blocks and physical tool indices, updates their feedrates and protected
tail lengths, and returns the whole layer's nominal duration. `slow_down` remains
the single-tool adapter. `CoolingLine` and `PerExtruderAdjustments` expose the
upstream duration/feedrate model; parser-only records are represented by Mica's
writer and layer plan.

The CLI uses imported profiles without new switches or dependencies. Existing
`--set` overrides work, for example:

```sh
export PATH="/opt/homebrew/opt/rustup/bin:$PATH"
cargo build --offline --release -p mica-cli
./target/release/mica slice tests/models/cube20.stl -o /tmp/cube.gcode \
  --set cooling_slowdown_logic=consistent_surface \
  --set cooling_perimeter_transition_distance=10
```

`ExtrusionPath::perimeter_index` carries external=0, first inner=1, deeper inner
indices from classic/Arachne generation through width, seam and overhang splits.
Existing callers constructing paths must initialize this new optional field.

## Port and integration

The numerical solver comes from pinned Prusa `CoolingBuffer.cpp`:

- Separate adjustable/non-adjustable lengths and durations, finite/infinite
  minimum-speed limits, elapsed/max-time sums and minimum-feedrate saturation.
- Adjustable-first, descending-feedrate sorting; fastest-first speed equalization;
  minimum-speed ordering across tools; increasing per-tool layer-time thresholds.
- Consistent surface first slows infill/deeper walls, then external/first-inner
  walls only if required. It preserves the configured length from the **end of
  each tool's eligible extrusion**, skipping bridges and already-minimum-speed
  blocks. This is a return to original feedrate, not a gradual speed ramp.
- Linear and IJ/R arc splits preserve extrusion; scarf splitting interpolates
  XYZ and E while retaining the scarf multiplier. Splits within 0.01 mm of a
  vertex snap to it. Adjusted feeds round to integer mm/min, as upstream does.
- Proportional slowdown retains the upstream five-iteration / 95%-target stopping
  rule. The overall target retains the 1.001 margin.
- Per-tool fan ramps, cooling disable, dynamic requests and bridge restoration.

Orca uses equalization, does not protect first-inner walls, and permanently
excludes external walls when `dont_slow_down_outer_wall` is true. It ignores
motion before the first extrusion marker, continues fan interpolation when
slowdown is disabled, and honors the pinned Orca first-layer fan ramp semantics.
Its typed source options identify the dialect; Klipper alone does not select it.

Two input corrections were necessary to compare the CORE One profile: Prusa's
filament infill/crossing speed caps now apply before cooling, and downstream
first-layer boundaries retain the uncompensated `lslices` boundary, as Prusa does.
The latter prevents elephant-foot compensation from causing false second-layer
overhang slowdown. Geometry used to generate the first layer remains compensated.

Deliberate adaptations and remaining limitations:

- Uses f64 and structured paths rather than an arbitrary G-code parser. Normal
  travel/retraction/lift, tower motion and configured toolchange durations are
  included. Arbitrary moves/waits emitted by custom templates and firmware macro
  internals are not parsed back into this cooling calculation. The final time
  estimator still processes emitted G-code independently.
- Tail selection uses each tool's own transition setting; upstream reads the
  current tool's setting for every adjustment. Empty adjustable ranges are safe,
  and all tools are sorted before mixed-mode processing.
- Proportional processing recomputes elapsed time after saturating inner paths;
  upstream's stale value can produce a negative factor in that branch.
- Scarf extrusion is preserved, but planning still uses the fitted XY length;
  its simultaneous Z motion/speed cap is not represented as separate cooling
  blocks. Fan-specific Orca support-interface/ironing/internal-bridge overrides
  remain in the existing fan integration scope, not a full Orca marker-parser port.
- Existing travel, fill-group ordering, small-perimeter speed selection and bridge
  generation are retained. Their output differences affect cooling budgets and
  which role receives the protected tail. This work does not assert geometric or
  nominal-speed parity for those stages.

## Verification and oracle results (2026-10-16)

`cargo fmt --all`, workspace Clippy with `-D warnings`, and provenance passed.
The workspace test run passed **401 tests**, with one pre-existing ignored test
and the live Orca launch test excluded. After the final cooling changes, the complete
`mica-gcode` suite passed 100 tests, with its one pre-existing ignored test.

The workspace command:

```sh
cargo test --offline --workspace -- --skip u1_live_orcaslicer_start_end_and_layer_structure
```

The upstream cooling test scenarios were translated to structured-block tests:
thresholds, fixed travel/E time, same/different-Z objects, disabled-tool fan,
bridge fan integration (in `tests/bridges.rs`), and the cube's zero-minimum-speed
10-second layer target. Additional tests cover the three modes, saturation,
nonconsecutive/revisited tools, distinct minima and transition distances, empty
ranges, Orca exclusion/prefix timing, and linear/IJ/R tail extrusion conservation.
Four two-tool cases also compare against numeric results from compiling the
pinned C++ adjustment functions in a temporary harness, within 0.001 mm/s.

Fresh PrusaSlicer 2.9.6 outputs used CORE One HF0.4 / 0.20mm SPEED / Prusament PLA,
center 125,110 and text G-code. Small fixtures were a 10 mm cube scaled from
`cube20.stl` and a 10 mm diameter × 10 mm high cylinder with 128 sides.

| Model | Reference / Mica estimated seconds | Time error | Role-speed result |
|---|---:|---:|---|
| CORE One cube20 | 923 / 914 | −0.98% | Interior sparse layers meet 250/200/21; full every-layer criterion fails |
| CORE One Benchy | 2308 / 2288 | −0.87% | External +0.08%, inner −8.16%, sparse +10.57%, solid +6.70%, top +1.86%, bridge 0%, overhang +0.58% |
| CORE One cube10 | 317 / 314 | −0.95% | External −0.18%, inner −0.18%, sparse +1.71%, solid −7.45%, top +0.003% |
| CORE One cylinder | 248 / 245 | −1.21% | External/inner about −5.25%, sparse +0.004%, solid +1.92%, top 0% |
| U1 cube20 | 908 / 924 | +1.76% | External +0.001%, inner +1.83%, sparse +0.16%, solid −6.71%, top −3.25%, bridge −33.33% |

Role percentages are extrusion-length-weighted mean feedrates, not planner
velocity or arithmetic means over G-code lines. `scripts/compare-cooling.py`
reports per-layer means, feedrate sets, overall role means and estimated time;
it handles relative/absolute E, linear/IJ/R arcs and Orca role aliases:

```sh
python3 scripts/compare-cooling.py reference.gcode mica.gcode --json /tmp/report.json
```

For cube20, inner-wall per-layer means differ by at most 0.35 mm/s and external
means by 0.10 mm/s. However, the second-layer external **feedrate set** still has
a discrepancy over 1 mm/s. Solid-fill layers 2, 95, 97 and 98 exceed 1 mm/s.
Layer 95 prints solid before sparse in Mica and sparse before solid in Prusa,
putting the 10 mm fast tail on different roles. Its role-mean differences are
−30.45 mm/s solid and +12.33 mm/s sparse. These failures are reported, not hidden
by the aggregate cube averages.

A cooling-disabled Benchy control also fails nominal role-speed parity: inner
walls −10.20% and solid fill +5.88%. Thus the cooling solver alone cannot satisfy
the Benchy role-average limits. This control used the same fresh
Prusa oracle and Mica with `cooling=0`.

The attempted fresh Orca launch supplied four explicit filament files but Orca
rejected vector validation (`filament_is_support` count 1 versus
`filament_colour` count 4). It did not slice or crash. The U1 comparison instead
uses the existing complete Orca 2.4.1 cube output from the infill oracle run
(2026-10-16), with the U1 / Standard / Snapmaker PLA / High Temp Plate settings.
It is a reused oracle, not a fresh run.

Review the shared exporter changes, first-layer outline accessor, heterogeneous
material behavior, and the stated oracle failures closely before treating this
as full slicer parity.
