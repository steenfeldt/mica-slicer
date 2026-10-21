# Dynamic overhang control and perimeter avoidance

Ported against PrusaSlicer 2.9.6 at `b028299c77`.
**Full oracle parity is not met.** The overhang-role speed checks and default Benchy time
pass; hole-block travel length and some other role/fan comparisons remain outside target.
The later [plate integration sweep](plate-level.md) supersedes the travel totals below for the
merged branch; it adds external multi-object routing and records the remaining failures.

## API and CLI

- `mica_core::toolpath::ExtrusionPath::overhang` is an optional boxed array of 16-byte
  `OverhangSection` records. Each record ends at a point index and carries start/end support
  distance and curl proximity. Ordinary constructors initialize it to `None`.
- `mica_gcode::overhang::OverhangProcessor::process` annotates paths in output order, retaining
  only the preceding object's boundary and external extrusion/curl lines. Support-only layers
  do not advance that history. Supplied annotations are preserved. Export invokes it across
  its existing 32-layer batches. Missing outlines leave paths unannotated.
- `calculate_overhang_speed` resolves the four support knots, percentages against capped
  external-perimeter speed, zero-as-reference speed, curl slowdown and 3% fan-decrease
  hysteresis. The emitter applies volumetric limits again after interpolation.
- `AvoidCrossingPerimeters::new` / `route` builds a layer-local island router. It follows the
  shorter inset boundary walk around holes/concavities, removes visible intermediate points,
  validates quantized legs, and enforces the maximum **extra** distance in mm or percent of
  direct distance. Retraction thresholds, lift/ramp motion and cooling use the resulting route.
  One recent route is cached for repeated queries. Different islands use plate-wide external hole
  boundaries. Missing outlines, failed routes and exceeded detour limits fall back to straight travel.

No new CLI syntax or dependencies. Existing `--set` options are now effective:

```sh
export PATH="/opt/homebrew/opt/rustup/bin:$PATH"
cargo build --offline --release -p mica-cli
./target/release/mica slice tests/models/3DBenchy.stl -o /tmp/benchy.gcode \
  --set enable_dynamic_overhang_speeds=1 \
  --set enable_dynamic_fan_speeds=1 \
  --set avoid_crossing_perimeters=1 \
  --set avoid_crossing_perimeters_max_detour=50%
```

`overhang_speed_0..3` and `overhang_fan_speed_0..3` retain their upstream meanings: 0%, 25%,
50%, 75% support, followed by the normal speed / zero requested fan at 100% support.
CORE One SPEED enables both dynamic switches. CORE One and XL inherit perimeter avoidance
**off** from the Prusa bundle; U1 overrides Orca's `reduce_crossing_wall` to **off**. Those
profile defaults are unchanged. The U1 CLI also successfully exports with avoidance enabled.

Cooling consumes the upstream SET/RESET and bridge-fan marker semantics directly; temporary
Prusa cooling markers are not written into final G-code. Dynamic requests respect disabled fan
layers, ramp limits, the normal cooling floor and bridge restoration. Repeated fan commands
are suppressed; custom Prusa M106/M107 commands invalidate the cached fan. Bridges contribute
time but are not slowed by layer cooling. The upstream 1.001 slowdown margin and floor-before-
cast fan rounding are retained.

## Oracle, 2026-10-16

Reference: the pinned PrusaSlicer app, its `out/ref/datadir`, CORE One HF0.4 / 0.20mm SPEED /
Prusament PLA, centered at 125,110. Mica explicitly uses that datadir's vendor INI. Normal
profile arc fitting remains enabled. Generated models, G-code, CSVs, logs and JSON reports
stay in a system temporary directory, not in the repository.

| Measurement | Prusa | Mica | Result |
|---|---:|---:|---|
| Benchy shared overhang-role layers | 31 | 31 | Maximum median-speed error 0.008%; all within 5% |
| Generated 20°–80° staircase shared overhang-role layers | 40 | 40 | Maximum median-speed error 1.22%; all within 5% |
| Benchy default estimated time | 38m28s | 37m26s | −2.69%; within 3% |
| Staircase estimated time | 21m53s | 21m40s | −0.99% |
| Benchy travel, avoidance enabled | 23,525.51 mm | 23,547.50 mm | +0.09%; within 10% |
| Hole-block travel, avoidance enabled | 2,172.61 mm | 1,416.82 mm | **−34.79%; fails 10% target** |
| Benchy crossing travel runs, avoidance enabled | 883 | 671 | Compare with Mica's 948 with avoidance off |
| Hole-block crossing travel runs, avoidance enabled | 50 | 0 | No measured exterior crossings in Mica |
| Benchy estimated time, avoidance enabled | 39m14s | 37m49s | **−3.61%; outside 3%** |

Mica additionally classifies an overhang path on Benchy layer 2; the reference has none in that
role there. Shared-role medians exclude that unmatched layer and the CSV records it explicitly.
The comparison script also reports ordinary perimeter roles: on default Benchy, 82/240 external
and 172/240 internal perimeter layer medians are outside 5%. These are **not** a claim of full
per-role speed parity. CORE One selects upstream `consistent_surface` cooling; Mica retains its
existing proportional, external-perimeters-last slowdown. Existing bridge-infill/ordering
and short variable-width path differences also affect the comparisons.

The minimum/maximum fan during extrusion agrees within one percentage point on 236/240 Benchy
layers and 149/150 staircase layers. Remaining Benchy layers: 2, 3, 39, 184; staircase: 146.
Explicit fan command counts are 586/327 (Prusa/Mica) on Benchy and 328/164 on the staircase.
These counts are not expected to be identical: repeated requests are suppressed, and path
segmentation/cooling and missing bridge-infill roles differ. Fan range agreement does not imply
identical command placement along a path.

Hole-block's travel mismatch is largely present in its infill ordering: on a representative
middle layer, reference travel is 38.78 mm versus Mica's 23.60 mm, with different infill entry
points and an additional reference travel. The router does not add purposeless detours to make
total length agree. Benchy travel counts still include unavoidable inter-island moves and
straight fallbacks.

`compare-overhang-travel.py` parses modal F and E, linear and circular extrusion, and records
unweighted per-command medians plus length-weighted medians. It groups consecutive non-extruding
XY moves into travel runs. Crossings are intersections with that layer's **emitted external
perimeter strokes**, excluding endpoint contacts, and counted regardless of Z lift. This is an
observable G-code metric; it omits seam gaps and overhang-role strokes whose exterior/interior
identity is not encoded in the role. It is not a proof of zero physical nozzle collisions.

Reproduce the generated model and a comparison:

```sh
python3 tests/models/overhang_staircase.py /tmp/overhang_staircase.stl
# Slice with PrusaSlicer (CORE One HF0.4, 0.20mm SPEED, Prusament PLA) and Mica, using the same model and bundle.
python3 scripts/compare-overhang-travel.py /tmp/reference.gcode /tmp/mica.gcode \
  --csv /tmp/overhang-layers.csv --json /tmp/overhang-summary.json
```

The generator makes a closed 12 mm wide solid with seven successive 4 mm rises, inclined
20°, 30°, …, 80° **from vertical**, above a 2 mm pedestal. It has 40 triangles and requires
only Python's standard library.

## Port scope and review

Adapted upstream support-distance/corner sampling, 3/9/16 mm curvature windows, geometric curl
height propagation/decay, 0.1 mm curl threshold, long-line curl coverage guard, proximity
slowdown, speed/fan interpolation and boundary-following travel. Copyright notices and register
entries are in `PROVENANCE.md`; no new third-party libraries or data were imported. The
pre-existing conflict markers in that register were removed while retaining both sets of rows.

Deliberate simplifications / omissions:

- Curl estimation uses Mica's ordered external wall paths and a layer-local line index. The
  existing overhang role lacks an exterior/interior bit, so its paths also contribute to curl
  history. Support-material curl and the full stability/support-spot model are not implemented.
- `avoid_crossing_curled_overhangs` remains unimplemented. Curl estimation here controls speed.
- The travel router covers the current island and plate-wide external hole boundaries, with ordinary
  Clipper2 inset fallback. Upstream's variable inset/resampling heuristics, top-surface exclusion,
  endpoint-extension machinery and wipe suppression are not ported.
- Mica's existing `consistent_surface` cooling omission, bridge-infill classification and
  infill/perimeter ordering differences remain. Classic walls receive dynamic attributes but
  retain their existing unsupported-wall classification limitations.

Review the inset fallback and boundary visibility checks closely on narrow necks and concave
holes. Also review curl behavior on short variable-width Arachne paths, scarf Z/E preservation,
and the output-index mapping of outlines when support-only layers are inserted. The new
integration tests exercise those schedule mappings and history across two export batch boundaries.
The upstream `test_avoid_crossing_perimeters.cpp` two-cube smoke test is adapted alongside numeric
speed/fan/curl tests, detour tests, lifted-route checks, fan-cache restoration, and scarf-volume
conservation checks.

Validation: `cargo fmt --all`, strict offline workspace Clippy, `cargo test --offline --workspace`,
and `cargo --offline xtask provenance` pass. The workspace run included the pinned Prusa vendor
profiles, XL profile tests and the installed Orca U1 oracle. One pre-existing manual
thumbnail/performance oracle remains ignored.
