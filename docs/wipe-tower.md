# Wipe and prime towers

`PreparedPrint::new` now plans a tower when `wipe_tower` is enabled and the job actually uses
multiple independent tools. `PreparedPrint::wipe_tower` exposes a compact `TowerPlan`;
`TowerPlan::visits(layer)` generates structured before/after-toolchange moves for one layer.
Coordinates are millimetres, extrusion volumes are mm³, and Rust/G-code tools are zero based.
Existing `export_gcode`, `export_gcode_with_mesh` and `export_file` accept the prepared job,
including Prusa binary output.

## CLI and settings

The XL and U1 process profiles already enable their tower. For example:

```sh
export PATH="/opt/homebrew/opt/rustup/bin:$PATH"
cargo build --offline --release -p mica-cli
./target/release/mica slice tests/models/cube20.stl tests/models/cube20.stl \
  --object-extruder 1=1 --object-extruder 2=2 \
  --printer 'Original Prusa XL - 5T Input Shaper 0.4 nozzle' \
  --print '0.20mm SPEED @XLIS 0.4' --filament 'Prusament PLA @XLIS' \
  -o /tmp/xl-tower.gcode
./target/release/mica slice project.3mf \
  --vendor Snapmaker --printer 'Snapmaker U1 (0.4 nozzle)' \
  --print '0.20 Standard @Snapmaker U1 (0.4 nozzle)' \
  --filament 'Snapmaker PLA @U1' --bed 'High Temp Plate' \
  -o /tmp/u1-tower.gcode
```

CLI tool numbers are one based. `--set wipe_tower=0` disables the tower. Canonical overrides
include `wipe_tower_x`, `wipe_tower_y`, `wipe_tower_rotation_angle`, `wipe_tower_width`,
`wipe_tower_brim_width`, `wipe_tower_cone_angle`, `wipe_tower_extra_spacing`,
`wipe_tower_extra_flow`, `wipe_tower_bridging`, `wipe_tower_no_sparse_layers`,
`wipe_tower_extruder`, and `single_extruder_multi_material_priming`. Percentage settings accept
values such as `--set wipe_tower_extra_spacing=150%`.

Prusa 2.9 stores tower placement in the model, not the printer preset. The 3MF reader imports
`Metadata/Prusa_Slicer_wipe_tower_information.xml`, with legacy INI placement as a fallback.
The CLI uses the selected plate's placement; explicit `--set` placement wins. Conflicting
placements in multiple input projects are errors. Obsolete Prusa preset coordinates/rotation
are discarded as in Prusa's config loader. Default Prusa placement is X180 Y140, rotation 0°.
Orca's per-plate `wipe_tower_x/y` and shared rotation are imported through
`SettingsMapper::tower_placements`, without applying embedded printer/process/filament presets.
These coordinates are already bed-local. Short placement vectors fall back to their first
entry, as in Orca; malformed/nonfinite placement values are errors. CLI edits synchronize
the imported typed and raw values, including `prime_volume` and tower coordinates.

The U1 profile uses non-BBL `WipeTower2` (the `wipe_tower_type` default is `type2`),
`enable_prime_tower=1`, `prime_tower_width=35`, `prime_volume=30`,
`prime_tower_brim_width=5`, rib walls, and `purge_in_prime_tower=0`.
The existing profile mapping converts enable/width/brim to canonical `wipe_tower*` settings.
The independent-head path uses `prime_volume`; the shared-nozzle flush matrix does not set
its purge volume. Prusa's independent-head path likewise uses the incoming tool's
`filament_minimal_purge_on_wipe_tower`, regardless of `wiping_volumes_matrix`.

The tested XL profile uses a 60 mm tower, 3 mm brim, 25° cone, 150% spacing,
100% flow, 8 mm bridging, 15 mm³ minimum purge, and 10 mm³ outgoing ramming at 40 mm³/s.
An explicit `wipe_tower_extruder` adds that structural tool to relevant layers and startup
usage, even when no model part uses it. A job using only one model tool retains the original
single-tool output and does not create a tower merely because this option is set.

## Planning, integration and statistics

The port retains upstream depth reservation, backward depth propagation, five iterations of
finish-layer purge saving, alternating layer orientation, brim, sparse grid, cone and U1 rib
geometry. Orca keeps its NORMAL layout while rotating alternate layers internally; Prusa also
alternates the reversed layout. Mica's existing Clipper2 offsets and monotonic filler supply
brim/cone geometry. No whole-print tower move list is retained.

The print schedule fills raft/empty Z gaps up to the last toolchange using the tools'
`max_layer_height` limits. `wipe_tower_no_sparse_layers` compresses tower Z; travel goes outside
the models before descending, then restores the object Z. Structural fill joins the first
non-soluble toolchange. The XL's physical-tool ramming precedes the existing toolchange template;
the incoming tool purges afterwards. The final XL ram is emitted at the final layer or one
layer above the tower, as applicable. U1 defaults disable ramming.

Optional front-edge priming visits used tools with the first printing tool last, and supports
the Prusa remove-priming prompt. The XL profile defaults this off and primes used tools in its
startup template instead. Unused physical tools are not added to either priming sequence.

The exporter ends the current object's cancellation label before a tower visit. Tower moves
have `;TYPE:Wipe tower` (Prusa) or `;TYPE:Prime tower` (Orca), with no `M486` or `EXCLUDE_OBJECT`
identity. Per-tool offsets, template position/retraction arrays, retract/lift/travel, volume
limits, temperatures and preheat continue through the existing writer. U1's zero
`travel_speed_z` means use travel speed, so descending moves never receive F0.
Prusa and U1 save/restore M220 around visits. XL ramming brackets stuck detection and resets
linear advance; U1's optional ramming pressure-advance override is restored afterwards.

Tower time contributes to the cooling pass as fixed time. Final time/M73 and preheat placement
use the existing motion analysis over all emitted tower and model commands. Tower-job filament
totals in mm/cm³/g are per physical tool and include explicit startup/custom G-code extrusion,
using upstream retraction debt accounting. Arc moves and per-tool diameters/densities are
included. Role totals describe generated paths only. Opaque firmware macros cannot contribute
unknown extrusion or motion; this matters for U1's `SM_PRINT_*` macros. Single-tool statistics
retain their original format and accounting.

`first_layer_print_min/max` includes the tower brim/cone/ribs. Orca's footprint hull and adaptive
mesh bounds include the tower bounding box, while `first_layer_center_no_wipe_tower` remains
model-only. This is a conservative hull rather than a union of every tower polygon.
The plate scheduler now feeds the tower envelope through the combined adhesion plan before
startup template evaluation; see [plate-level integration and combined oracle results](plate-level.md).

## Oracle results (2026-10-16)

Inputs come from `scripts/prepare-wipe-tower-oracles.py`. References use PrusaSlicer 2.9.6
with XL 5T Input Shaper 0.4 / 0.20mm SPEED @XLIS 0.4 / Prusament PLA @XLIS, and OrcaSlicer
2.4.1 with U1 0.4 / 0.20 Standard / Snapmaker PLA @U1 / High Temp Plate. Arrangement is disabled.
The four models are synthetic per-object-tool 3MFs built from a 20 mm cube and the repository's
CC0 Benchy. Both references and Mica print the same stored placements.

All four cases have identical complete T-command lists and identical per-layer extruding-tool
sequences. Every tower layer, including final ramming at its actual Z, is within ±3% filament.

| Case (one-based tools) | Worst tower layer difference | Reference time | Mica time |
| --- | ---: | ---: | ---: |
| XL, two cubes, 1/2 | 0.157% | 33m55s | 36m21s |
| XL, three Benchys, 1/3/5 | 0.191% | 3h09m45s | 3h12m22s |
| U1, four cubes, 1/2/3/4 | 1.554% | 1h27m10s | 1h26m11s |
| U1, four Benchys, 1/2/3/4 | 1.554% | 4h23m54s | 4h43m28s |

Tower extrusion extents match the references to the displayed 0.001 mm (centreline bounds,
including brim/cone/ribs):

| Case | Nominal X/Y, width × maximum depth | Extrusion X range | Extrusion Y range |
| --- | --- | --- | --- |
| XL cubes | 180/140, 60 × 4.25 mm | 176.800–243.200 | 134.535–149.715 |
| XL Benchys | 180/140, 60 × 8.00 mm | 176.800–243.200 | 130.202–157.798 |
| U1 cubes and Benchys | 15/220, 35 × 13.50 mm | 9.055–55.945 | 212.525–240.975 |

Total explicit filament, including startup extrusion and excluding retraction debt and opaque
macros, in mm per used tool in the order shown above:

| Case | Reference | Mica |
| --- | --- | --- |
| XL cubes | 2092.98, 2133.82 | 2092.99, 2133.73 |
| XL Benchys | 7820.67, 5304.35, 5229.89 | 7824.22, 5305.55, 5231.66 |
| U1 cubes | 2285.54, 2462.47, 1820.28, 1808.62 | 2222.87, 2401.64, 1757.82, 1746.17 |
| U1 Benchys | 6372.60, 6704.59, 5246.69, 5235.04 | 6495.80, 6826.35, 5364.22, 5355.45 |

The U1 differences in whole-job filament/time include existing model wall/infill/bridge and
motion/cooling differences; this port does not claim full Orca geometry or timing parity.
The largest tower difference is U1's first layer, from brim/rib offset and fillet geometry;
subsequent layers are much closer. Optional XL front-edge priming also matched tool order and
filament within 0.0001 mm per tool (13.2457 and 6.8363 mm reference).

Single-tool cube20 and 3DBenchy on CORE One, XL and U1 were compared against the saved main
binary at `f83e76b`: all six complete text outputs are byte-identical. Automated regressions
cover tower-on/off single-tool equivalence, nonconsecutive tools, labels, emitted filament,
offsets, forced structural tools, front priming, rotation, invalid settings, sparse Z return,
raft gaps and U1 Z speed. Tests adapt the applicable scenarios in upstream `test_multi.cpp`;
there is no dedicated `test_wipe_tower` file at the pinned Prusa revision.

## Reproducing the references

The standard-library setup script creates inputs and flattened profiles in a temporary directory;
it never launches slicers. Keep all generated files outside the repository:

```sh
oracle_dir=$(python3 scripts/prepare-wipe-tower-oracles.py)
'out/ref/PrusaSlicer-2.9.6.app/Contents/MacOS/PrusaSlicer' \
  --datadir out/ref/datadir \
  --printer-profile 'Original Prusa XL - 5T Input Shaper 0.4 nozzle' \
  --print-profile '0.20mm SPEED @XLIS 0.4' --material-profile 'Prusament PLA @XLIS' \
  --load "$oracle_dir/xl-matrix.ini" --dont-arrange --no-binary-gcode \
  --export-gcode -o "$oracle_dir/xl-cubes-ref.gcode" "$oracle_dir/xl-cubes.3mf"
mkdir -p "$oracle_dir/u1-cubes"
(cd "$oracle_dir" && /Applications/OrcaSlicer.app/Contents/MacOS/OrcaSlicer \
  --datadir "$oracle_dir/data" \
  --load-settings "$oracle_dir/machine.json;$oracle_dir/process.json" \
  --load-filaments "$oracle_dir/filament1.json;$oracle_dir/filament2.json;$oracle_dir/filament3.json;$oracle_dir/filament4.json" \
  --slice 0 --arrange 0 --outputdir "$oracle_dir/u1-cubes" "$oracle_dir/u1-cubes.3mf")
```

Repeat with `xl-benchys.3mf` and `u1-benchys.3mf` in separate output directories. Slice these
same files with Mica using the profile arguments above, then run
`python3 scripts/compare-wipe-tower.py reference.gcode mica.gcode`.
The JSON includes tower per-Z/per-tool extrusion, whole-stream explicit filament, bounds,
T commands, layer tool sequences and time. Positive XY path totals are reported separately.

The explicit Prusa 5×5 matrix avoids a malformed single-entry CLI default during priming setup.
Orca receives explicit four-slot defaults, colours, mapping, flush matrix/vector and distinct
filament preset IDs. The generator also expands missing singleton schema defaults, including
`filament_is_support`; four filament files alone can still fail validation. The successful U1
runs did not segfault. See the later two-tool attempt's [validation failure and retry status](plate-level.md#u1-oracle-status).

The generator now resolves `inherits` within the Snapmaker vendor (then OrcaFilamentLibrary),
as Orca does. Before, a global name index picked an arbitrary vendor's `fdm_filament_pla` and
`fdm_filament_common`, so "Snapmaker PLA @U1" came out with 40 mm/s retraction instead of 60,
15 mm³ minimal purge instead of 0 and 12 or 20 mm³/s maximum flow instead of 14. References
made with the old script carry those values. The singleton schema defaults skip the
extruder-variant vectors (`extruder_variant_list`, `filament_extruder_variant`,
`filament_self_index`, `default_nozzle_volume_type`): four-entry copies abort Orca 2.4.1 with
`ConfigOptionVector::set_with_restore_2(): invalid size found`. For a project 3MF given with
`--load-settings`, drop its `Application` metadata: otherwise the CLI treats it as an Orca
project and crashes on the missing `printer_settings_id`.

## U1 tower placement investigation (2026-10-16)

The reported CLI job (plate 1 of the private five-plate U1 project) already planned a **35 × 13.5 mm body**,
including with organic supports enabled. The reported 86 mm depth was not reproduced.
The confirmed defects were that the CLI ignored Orca project placement and that a typed
`--set wipe_tower_x/y` edit left stale raw coordinates which the tower planner read first.
Both are fixed. No depth cap or reduction of required purge volume was introduced, and
Prusa/XL planning and path generation are unchanged.

For `cat.3mf`, `--plate 1` now uses **X165 Y233.362**.
With the documented U1 Standard / Snapmaker PLA / High Temp Plate profiles, the tower's
extrusion centreline bounds are **X159.055–205.945, Y225.887–254.337 mm**, including ribs and
brim. The body is 35 × 13.5 mm; its complete footprint is 46.890 × 28.450 mm and fits the
256 mm bed. The job has 244 model layers and 181 tower layers, using 2724.64266 mm of tower
filament. This is a Mica measurement, not an Orca filament comparison for this project.

The source comparison covered Orca `WipeTower2` construction, `plan_toolchange`, `plan_tower`,
`save_on_last_wipe`, `generate`, rib generation and `Print::_make_wipe_tower`:

| Setting/step | U1 behavior |
| --- | --- |
| Width and line width | `prime_tower_width=35`; 1.25 × 0.4 = 0.5 mm tower line width; 33.5 mm planning width |
| Per-change volume | `prime_volume=30` mm³; independent heads ignore the shared-nozzle flush matrix/vector and `flush_multiplier` |
| Ramming | `enable_filament_ramming=0`, per-filament multitool ramming off; zero reserved ram depth |
| Depth reservation | Rounded row count at the actual tower layer height; first-layer spacing uses extra flow, later spacing uses extra flow × extra spacing (both 100% here) |
| Layer planning | Sum actual changes, propagate required depth downward, repeat structural purge savings five times; incoming filament minimum is 15 mm³ |
| Four-tool sections | Three changes per layer: 2/4/4 mm on the 0.25 mm first layer; 3/5/5 mm on the 0.20 mm layers after structural savings; 13 mm propagated depth plus 0.5 mm outer boundary |
| Brim | 5 mm requested; 0.25 mm first-layer height gives 0.44635 mm extrusion spacing, rounded to 11 brim loops (4.90985 mm) |
| Rib/cone | Rib wall, 8 mm configured rib width clamped against the body depth, zero extra rib length, fillets enabled; the configured 15° cone is inactive for rib walls |
| Other prime/flush options | Type2 selected; type1 framework/infill-gap/skip-point options do not size this tower; ironing/interface features remain outside the port |

Snapmaker's pinned fork changes physical-tool ramming reservation and adds local-Z scheduling,
but the U1 default ramming switches are off. Its M220 backup/restore and pressure-advance handling
remain in Mica's existing exporter. Those fork changes do not justify enlarging this Orca-profile
tower. The saved `out/ref/u1-benchy/plate_1.gcode` confirms the width, prime volume, spacing and
rib settings, but is single-tool and has no tower; its filament minimum is also 0 rather than
this job's 15 mm³. It cannot establish this job's tower filament parity.

The saved four-tool Orca 2.4.1 references from the oracle run above do exercise the tower.
Reference towers were at
X15 Y220; subtracting the explicit translation (+150, +13.362) from the new tower gives:

| Case | Orca tower filament | Mica tower filament | Total difference | Bounds difference |
| --- | ---: | ---: | ---: | ---: |
| Four cubes, 100 layers | 3455.72810 mm | 3457.11723 mm | +0.04020% | 0.000 mm |
| Four Benchys, 240 tower layers | 8194.92410 mm | 8195.98283 mm | +0.01292% | 0.000 mm |

Both cases have identical T-command lists and layer tool sequences; the worst individual
layer difference is 1.554%. The regression suite checks the cube's size and total filament,
tall rib towers' independence from cone angle, flush-volume isolation, per-plate placement,
rotation, short vectors and CLI override precedence. XL two-cube and three-Benchy complete
text outputs are byte-identical to the binary built before these edits.

### Fresh reference attempt

Only one Orca launch was attempted. It aborted with exit 134 before slicing:
`ConfigOptionVector::set_with_restore_2(): invalid size found`. There was no retry. The
exact preparation and command are:

```sh
ref=$(mktemp -d)
python3 scripts/prepare-wipe-tower-oracles.py --output "$ref/oracle"
python3 - "$ref" <<'PYSETTINGS'
import json, sys
from pathlib import Path
p = Path(sys.argv[1]) / 'oracle/process.json'
d = json.loads(p.read_text())
d['wipe_tower_x'] = ['165'] * 5
d['wipe_tower_y'] = ['233.362'] * 5
p.write_text(json.dumps(d))
PYSETTINGS
cd "$ref/oracle"
/Applications/OrcaSlicer.app/Contents/MacOS/OrcaSlicer \
  --datadir data \
  --load-settings 'machine.json;process.json' \
  --load-filaments 'filament1.json;filament2.json;filament3.json;filament4.json' \
  --slice 1 --arrange 0 --outputdir . \
  --export-settings resolved.json \
  "$MICA_PRIVATE_MODELS/cat.3mf" \
  > orca.log 2>&1
```

Review the unresolved 86 mm report against its original settings and the missing fresh project
oracle closely. The successful saved-reference comparisons establish the standard four-tool
tower geometry and filament tolerance, not complete equivalence with Orca on this project. No
hardware print was performed.

Validation passed: offline release CLI build, `cargo fmt --all`, workspace
Clippy with `-D warnings`, `cargo test --offline --workspace`, and `cargo xtask provenance`
with `CARGO_NET_OFFLINE=true`. The live U1 test reused its saved Orca reference
(`MICA_U1_ORCA_REFERENCE`), so it did not relaunch Orca.

## Support layers (2026-10-16)

With supports on, the five-plate project's U1 tower grew to 86 mm in Y and left the bed. Cause: the
tower planned **every** job layer as a tower layer, including the support-only layers that
independent support layer heights interleave with the object layers (295 job layers for 244
object layers with normal supports). A tool change on an object layer just above a support
layer then got a 0.05 mm tower layer height, and the purge volume at that height needs about
four times the rows; the depth propagated down to the whole tower. Orca and Prusa plan the tower
only on `LayerTools::has_wipe_tower` layers (`ToolOrdering::fill_wipe_tower_partitions`) and give
each the distance to the previous tower layer (`wipe_tower_layer_height`).

`ToolOrdering::tower_layers` ports that rule: object layers below the last tool change, all
layers below the first object layer (raft), any layer that changes tools, and the extra layers
that keep the tower from rising more than `calc_max_layer_height` (now
`tool_ordering::max_layer_height`, shared with the gap-filler layers). `LayerTools::has_object`
comes from the job's object/support layer sources; gap-filler layers count as object layers.
`TowerPlan` now holds only tower layers and maps job layers to them (`job_layers`); `visits`
returns nothing for a job layer without tower. Support material on the current tool never adds
a tool change. Jobs without support-only layers plan exactly as before (byte-identical outputs).

Cat plate 1 (`cat.3mf`, U1 0.20 Standard, Generic PLA,
High Temp Plate, project settings `enable_support=1`, `sparse_infill_density=8%`, crosshatch,
`support_type=tree(manual)`), against OrcaSlicer 2.4.1 on the unchanged project:

| Job | Tower extrusion bounds X / Y (mm) | Tower layers | Tower filament T1/T2/T3/T4 (mm) |
| --- | --- | ---: | --- |
| Orca, project settings (tree(manual)) | 159.055–205.945 / 225.887–254.337 | 181 | 1484.3 / 245.2 / 1063.2 / 139.8 |
| Mica before, supports on (then normal/auto) | **157.1–207.9 / 227.0–313.2**, rejected as off-bed | — | — |
| Mica after, project settings | 159.055–205.945 / 225.887–254.337 | 181 | 1505.4 / 256.4 / 1060.2 / 139.8 |
| Orca, `support_type=normal(auto)` copy | 159.055–205.945 / 225.887–254.337 | 181 | 1484.3 / 245.2 / 1063.2 / 139.8 |
| Mica after, `normal(auto)` (295 job layers) | 159.055–205.945 / 225.887–254.337 | 181 | 1505.4 / 256.4 / 1060.2 / 139.8 |

Bounds match to the printed 0.001 mm and equal the supports-off tower. Tower filament per tool
is within ±5% (T2 +4.6%); the remaining per-layer difference (sparse tower layers 11.0 versus
14.5 mm) is the same with supports off and predates this change. `mica validate` on the project
job reports 0 errors (18 layer-height warnings, the same as the supports-off job).

## XL toolchange retraction and tower integration (2026-10-17)

**Filament retraction overrides.** PrusaSlicer layers every non-nil `filament_<key>` over the
printer's per-extruder `<key>` for the 17 retract keys of `PrintConfigDef::init_extruder_option_keys`
(`PrintApply.cpp::print_config_diffs`, `ConfigOptionVector::apply_override`). Both G-code
generation (`Extruder::retract_length_toolchange`, `GCodeWriter::retract_for_toolchange`) and the
placeholder parser see the result. Mica only did this for a few keys of the first tool and never
for templates, so the XL start G-code's `{e_retracted[1] = retract_length_toolchange[1]}` parked
non-initial tools 1.2 mm retracted instead of Prusament PLA @XLIS's 20 mm, and the cooling estimate
used 1.2 mm. `mica_core::filament_overrides` ports the rule. Export, templates and per-tool
configurations now use it; `nil` elements keep the printer value. The written `filament_*` values
are unchanged, so `is_nil(filament_…)` still works. The Orca mapping already applies Orca's own
override list at import (`filament_retraction_length` and so on). Orca has no filament override
for `retract_length_toolchange`, and the U1 toolchange retraction (10 mm at the filament's
60 mm/s) matched a fresh native Orca 2.4.1 slice before and after this change.

**Tower visits.** These now follow `WipeTowerIntegration::append_tcr` and
`WipeTower::toolchange_Change`:

- The travel to the tower runs before the tower G-code and uses the layer's travel planner
  (ramping lift), rather than a lift, travel and descent inside it.
- The unload ends with a planner flush.
- After the toolchange template, the tower travels back without skipping, forces the layer Z
  (the XL template's `position[2] += 2` hop no longer adds a stray Z move) and deretracts, then
  flushes.
- The new tool's part fan is re-emitted right after the toolchange (`;_TOOLCHANGE_END`), not
  after the tower.

`finish_layer` now starts in the corner `m_left_to_right` gives after the first non-soluble
toolchange, including the flip at the end of a non-last wipe. This state carries over layers
without changes, and the rule is the same in Orca's `WipeTower2`. Previously every finish started
in one fixed corner, which added a tower-width travel on alternate layers. The cooling estimate
now includes the travels to and from the tower, with their retractions.

PrusaSlicer 2.9.6 against Mica (XL 5T IS / 0.20mm SPEED @XLIS / Prusament PLA @XLIS; synthetic
cube projects from `scripts/prepare-wipe-tower-oracles.py`, explicit 140 mm³ matrix):

| Job | Time ref | Mica before | Mica after | Z moves ref / before / after | Tower filament ref / after |
| --- | ---: | ---: | ---: | ---: | ---: |
| 2 cubes, T1/T2 | 33m55s | 36m53s | 33m58s | 614 / 814 / 614 | 1654.83 / 1654.73 mm |
| 3 cubes, T1/T2/T3 | 56m48s | 57m40s | 56m16s | 1017 / 1417 / 1017 | 2798.69 / 2798.68 mm |
| 3 Benchys, T1/T3/T5 | 3h09m45s | 3h11m34s | 3h07m58s | 6161 / 7439 / 6479 | 6693.37 / 6693.14 mm |

All toolchange retractions are now 20 mm, the same count as the reference (101/202/482); before,
each non-initial tool had a 1.2 mm one. T-command lists, per-layer tool sequences and tower bounds
are identical, and the worst tower layer is within 0.16%. Single-tool CORE One/XL output is
byte-identical.

Remaining differences are outside the tower:

- PrusaSlicer wipes on retraction (`wipe = 1`, `retract_before_wipe = 80%`: 0.64 mm plus a 0.16 mm
  wipe). Mica's Prusa path has no retraction wipe, for single-tool output too. This is most of
  the travel-count gap and about 200 s on the Benchys.
- Long ramped travels are one segment instead of Prusa's blended five
  (`Travels.cpp` smoothing).
- `OozePrevention::post_toolchange` emits an extra `M109` wait (fixed below).
- On the U1, Orca wipes and spiral-lifts during the toolchange retraction (9.316 mm plus a
  0.684 mm wipe); Mica retracted 10 mm without a wipe. Ported below.

## U1 toolchange retraction, wipe and lift (2026-10-17)

The U1 (Orca configuration, `WipeTower2`) toolchange now follows Orca's
`WipeTowerIntegration::append_tcr2` and `GCode::set_extruder` (`orca_toolchange.rs`); Prusa
configurations keep their code path. The causes of the gap, upstream file::function:

- `GCode.cpp::GCode::set_extruder` → `GCode::retract(toolchange=true)`: with `wipe` and a stored
  wipe path, `Wipe::calculateWipeRetractionLengths` splits `retract_length_toolchange` (10 mm)
  into `retract_for_toolchange(before_wipe)` and the wipe's own retraction
  (`retraction_speed × wipe length / wipe speed`), then `Wipe::wipe` follows the last path back.
  Mica retracted 10 mm in place. The path is reset afterwards ("Always reset the extrusion path").
- `GCode.cpp::GCode::set_extruder` prepends `retract(false, false, SpiralLift, true)` to
  `change_filament_gcode`: already retracted, it only runs `GCodeWriter.cpp::eager_lift`, a
  full helix in place (centre one radius in +X, radius `z_hop / (2π·atan(travel_slope))`,
  `G17` + `G3 … P1`, or 4–16 linear segments without arc fitting). Mica had no lift.
- `OozePrevention::pre_toolchange/post_toolchange` (U1: `ooze_prevention=1`,
  `standby_temperature_delta=-150`): `M104 S70 T<old> ; set nozzle temperature ;cooldown` and an
  `M109 S<temp> T<new>` wait after the filament start G-code, even though the U1 template
  waits too. CoolingBuffer's `;_FORCE_RESUME_FAN_SPEED` re-emits the auxiliary and part fans after
  the template. The new tool's pressure advance is set again after the tower result.
- `WipeTower2.cpp::toolchange_Unload/toolchange_Change` and `GCode.cpp::post_process_wipe_tower_moves`:
  without ramming the independent tool changes in place (no travel to the tower first);
  `G4 S0` after the unload; after the template the post-processor writes the feedrate line as a
  travel to `tool_change_start_pos`, then the never-skipped move to the wipe start, the forced
  layer Z, the deretraction and `G4 S0`. Mica lifted, travelled and descended instead.
- `append_tcr2` "Prepare a future wipe": `m_wipe.path = tcr.wipe_path` (toolchange_Wipe's three
  points, or finish_layer's last loop edge; `merge_tcr` keeps the second's), so leaving the tower
  wipes. Mica left with a plain retraction. The path is the same `TowerVisit::wipe_path` the XL
  port uses (Prusa's and Orca's tower writers add the same wipe points); the U1 hands it to the
  Orca wipe, the XL to the Prusa one. Orca-only block data (`TowerBlock::joined`, the merge
  point, and `entry`, `tool_change_start_pos`) stays separate. A visit that needs a travel to the tower (finish only,
  finish first, lower tower layer, ramming, `tool_change_on_wipe_tower`) uses `GCode::retract`'s
  default NormalLift: `G1 Z+hop`, the XYZ travel, `G1 Z`, deretract.
- Normal travels already had Orca's wipe, Slope/Spiral/Auto lift and overhang test
  (`orca_motion.rs`, from `GCode::travel_to`, `needs_retraction`, `GCodeWriter::travel_to_xyz`).
  Three details were fixed: the wipe-rate bound is the (filament-overridden) retraction speed
  (Snapmaker PLA: 60 mm/s, as Orca's print config; Mica used the machine's 40), a remainder below
  `EPSILON` writes no `G1 E-0` (216 such lines in a U1 Benchy), and the first travel after a lift
  carries the lifted Z as `travel_to_xyz` writes it.

Against fresh native OrcaSlicer 2.4.1 (fresh datadir), Mica before → after. Retraction is the
toolchange retraction before the wipe (most common value):

| Job | Orca | Mica before | Mica after |
| --- | --- | --- | --- |
| plates-orca plate 1 (4 tools, 271 changes): toolchange retraction | 9.468 mm + wipe | 10 mm, no wipe | 9.457 mm + wipe |
| … spiral lifts / wipes / `M109` / `G4` | 370 / 1092 / 547 / 815 | 97 / 646 / 277 / 273 | 370 / 1190 / 550 / 819 |
| … tower departures with a wipe | 197 of 272 | 0 of 273 | 273 of 273 |
| … time | 1h9m57s | 1h9m55s | 1h10m29s |
| painted cube (2 tools, 98 changes): toolchange retraction | 9.316 mm + 0.684 mm wipe | 10 mm | 9.302 + 0.698 mm |
| … spiral lifts / wipes / `M109` / pressure advance | 197 / 581 / 201 / 198 | 99 / 294 / 102 / 99 | 197 / 491 / 200 / 198 |
| … time | 28m44s | 28m30s | 28m40s |
| cat plate 1 (4 tools, 221 changes): wipes / `M109` / `G4` | 6632 / 447 / 665 | 7078 / 228 / 236 | 7535 / 452 / 672 |
| … time | 3h7m38s | 3h10m17s | 3h10m45s |
| U1 Benchy (single tool): `G1 E-0` lines / time | 0 / 40m18s | 216 / 41m6s | 0 / 41m2s |

Per-tool filament and tower filament are unchanged by this port (wipes only move retraction),
for example plates 1835.4/1056.3/1332.0/883.6 mm against Orca's 1806.6/1056.3/1327.7/894.0 mm
and painted 1453.1/1124.4 mm against 1436.9/1143.1 mm. Tool-change blocks now match Orca line
for line apart from formatting (the tower writer's inline `F` and `G1  X…` spacing, Mica's
`; CP WIPE TOWER` markers, object labels that end before the tower), Snapmaker's `M220 B/R`
(kept from the fork) and the wipe moves themselves, which follow each slicer's last model path.
Validation reports 0 errors for all jobs; single-tool CORE One/XL and XL multi-tool outputs are
byte-identical.

Remaining differences, outside the toolchange code:

- Orca's rib wall closes with near-duplicate vertices, so its finish wipe is usually empty
  (CoolingBuffer drops zero-length wipe lines); Mica's rib polygon wipes back up to the last
  edge. Orca's rib loop also has fewer, differently placed vertices (45 against 256 segments in
  the cat), and its tower writer inherits the previous feed rate where Mica travels at full speed.
- Orca keeps the tower inside the previous object's `EXCLUDE_OBJECT` block; Mica does not.
- Orca wipes along the arc-fitted polyline (chords), Mica along the dense path, so Mica's wipes
  have more segments (cat: 75637 against 21553 wipe moves) and fewer pre-wipe retractions.
- Auto Lift picks a spiral more often in Mica than Orca's `is_through_overhang` (cat about 755 against
  390 travel spirals, Benchy 68 against 33 beyond the layer changes), and Orca skips the retraction
  of travels inside support islands (`needs_retraction`), which Mica does not port.
- Orca emits the part fan at many more places (Benchy 727 against 12 `M106`); this is cooling.
- Time: the helix, wipe and extra travels cost Mica's estimator about 0.1 s per toolchange; the
  single-tool Benchy is 44 s over Orca's estimate without any toolchange.

### Cow plate 3 (head): less filament than Orca

`cow.3mf` plate 3 (a private Orca 2.3.3 project): Orca 14725.6 mm, Mica 11958.8 mm (Mica −18.8 %,
Orca +23.1 %). The tower is −35.4 % (4706.8 against 7281.9 mm, 28.4 against 49.3 mm deep with
brim); walls, sparse infill and supports are within 9 %. Orca also reports 79 mm gap infill and
62 mm bottom surface, which Mica's job has none of (its internal solid infill is 128 mm higher). The cause is not in the tower or toolchange code. The project embeds a complete,
older Snapmaker U1 configuration, which Orca's CLI uses as is. Its `different_settings_to_system`
lists only `enable_support;support_type`, and Mica's project import
(`mica-profiles project::ProjectSettings::apply`, `mica-cli project_settings.rs::compose`) rebuilds
the configuration from today's installed presets plus those listed edits. The embedded values differ
from the current presets: `prime_volume` 45 (now 30), `prime_tower_width` 30 (35),
`wipe_tower_extra_spacing` 120 % (100 %), `wipe_tower_extra_rib_length` 8 (0),
`wipe_tower_bridging` 10, `filament_multitool_ramming` on with 0.5 mm³ at 20 mm³/s (off),
retraction 1.5 mm at 30 mm/s (0.8 mm at 40 mm/s), arc fitting off and an older
`change_filament_gcode`. With only the tower keys passed as `--set`, Mica's tower is 7296.7 mm
(+0.2 %) and the job 14548.6 mm (−1.2 %). Whether project import should prefer the embedded values
over current presets is a project-import decision; it is documented here, not changed.

## XL toolchange wipes and ooze prevention (2026-10-17)

The retraction wipe has since been ported (`prusa_wipe.rs`). Three toolchange gaps remained:

- **Leaving the tower.** `WipeTowerIntegration::append_tcr` sets `m_wipe` to the tower's
  `ToolChangeResult::wipe_path` when the tower printed at the object Z, so the next retraction
  wipes on the tower. `toolchange_Wipe` adds three points (down one line, then towards the side the
  last line started from) and `finish_layer` two (back along the outer contour); `merge_tcr`
  keeps the second's. Mica dropped the path. `TowerVisit::wipe_path` now carries it in bed
  coordinates, and `emit_tower` follows append_tcr: wipe before the travel, reset for ramming,
  the tower path afterwards (reset after a sparse-layer descent).
- **Toolchange without a tower.** `GCodeGenerator::set_extruder` calls `retract_and_wipe(true)`:
  `retract_for_toolchange(true)` retracts `retract_before_wipe` of the toolchange length
  (16 of 20 mm on the XL), the rest is wiped along the last extrusion, then topped up. `Wipe::init`
  makes the path long enough for the toolchange length. The path is then reset, and
  `last_position` is forgotten: the next path is reached "after unknown position" (XY travel,
  forced layer Z, no lift or avoid crossing). Mica retracted 20 mm in place and then ramped.
- **`M109` after the toolchange.** `OozePrevention::post_toolchange` waits
  (`M109 S… T… ; set temperature and wait for it to be reached`, first-layer temperature on layer 0)
  whenever `ooze_prevention` is on and `standby_temperature_delta` is not zero, after the
  start filament G-code, with or without a tower. The XL profile has `ooze_prevention = 1`
  and `standby_temperature_delta = -110`, so each XL toolchange waits twice (template and
  ooze prevention). Mica had only the template's wait.

Instance labels in multi-tool Prusa jobs now also follow `_extrude` / `change_layer`: with several
extruders the label changes at the layer change, starts after the unretract when none is active,
and stays open across layers.

PrusaSlicer 2.9.6 against Mica (same profiles and inputs as above; one-based tools; ooze
prevention as in the profile unless noted). Times are the header's normal-mode estimate:

| Job | PrusaSlicer | Mica before | Mica after |
| --- | ---: | ---: | ---: |
| 2 cubes, T1/T2 | 33m55s | 34m12s | 34m15s |
| 3 cubes, T1/T2/T3 | 56m48s | 56m37s | 56m44s |
| 3 Benchys, T1/T3/T5 | 3h09m45s | 3h10m51s | 3h11m08s |
| 3 cubes, no tower | 44m28s | 43m49s | 44m34s |
| 3 Benchys, no tower | 2h37m34s | 2h38m29s | 2h39m20s |

Ooze prevention off gives the same times. The `M109` waits after `T` (one per toolchange) went
from 0 to the reference count, and to zero with `ooze_prevention = 0`, in all nine jobs. On the
cubes, retraction and wipe counts now equal the reference (2 cubes 609/506, 3 cubes 1014/809,
no tower 618/609; before 406, 609 and 409 wipes). Per layer, the motion sequence with object labels
(ramped travel collapsed, feed rates ignored) is identical on 97 of 100 layers for all three cube
jobs (0 before), and 198 of 200 no-tower toolchange windows are identical. The remaining layers
are the top layers, where the last extrusion before a change is a different path, and the final
purge travel. Per-role totals are unchanged.

The Benchy differences are not in the toolchange: Mica ends a tool's block with sparse infill
more often (234 against 142 of 480 changes), so it wipes along different paths. This, Benchy
retraction counts (7242 against 6830) and travel segmentation are model ordering and travel
planning differences. Mica's own estimator on the PrusaSlicer files gives +10 s on the
two-cube job and +14 s on the no-tower job, so part of the time gap is the estimator itself.

Not covered: the XL's no-tower first travel lifts from the start template's Z (2.6 mm in
PrusaSlicer, 0.8 mm in Mica); the move to the final purge; priming wipe paths (the XL primes in
its start G-code); Mica's own fallback `M109` when a toolchange template sets no temperature, which
upstream has no equivalent for. The no-tower cooling estimate still counts the full toolchange
retraction rather than the wipe. U1 output and single-tool CORE One and XL output are
byte-identical.

## Layer extrusion order (2026-10-18)

The Benchy toolchange gap above was the order in which a layer's islands, regions and
extrusions are printed. Four divergences from `GCode/ExtrusionOrder.cpp` and its inputs:

- **Regional (3MF/GUI) pipeline.** `get_slices_extrusions` / `extract_island_extrusions` /
  `extract_infill_ranges` print a layer island by island: the lslices in
  `lslice_indices_sorted_by_print_order`, each lslice's islands chained from the previous
  position (`chain_and_reorder_layer_islands`), per island the perimeters and then its fills,
  where all fills of one region (solid, sparse, top, bridges and the gap fill) are one chained set.
  Mica's region pipeline printed every wall group of the layer first and then the fills kind by
  kind across all islands, so a tool's block nearly always ended with whatever kind came last.
  Prusa profiles now follow upstream (`PreparedRegions::layer_prusa`); islands are matched to
  their lslice by containment (`Layer::sort_perimeters_into_islands`). A single-region 3MF object
  now orders its paths like the same STL. Orca profiles keep the previous order.
- **ExPolygon order.** `ClipperUtils.cpp::PolyTreeToExPolygons` lists an outer polygon before the
  islands nested in its holes; Mica's tree conversion listed it after them. `chain_expolygons` has
  no start point, so with two islands it keeps the input order, and Mica printed the Benchy cabin
  before the hull on layers 43–52.
- **Arachne perimeter order.** `Arachne/PerimeterOrder.cpp::ordered_perimeter_extrusions` picks the
  nearest start (a closer closed line always wins, an open one only while no closed line is
  nearest) and sorts with libc++ `std::sort`; Mica preferred every open line over every closed
  one and used a stable sort, so the hull's small hawse-hole loop came first. Ported exactly for
  Prusa profiles, with Clipper's border-inclusive `PointInPolygon`.
- **`infill_first`** was ignored. Prusa profiles now print an island's fills before its
  perimeters (both region paths). The first island of a layer still chains from the object
  origin instead of the previous layer's end (Mica orders toolpaths before export), which costs
  more travel with this option than without (Benchy +2.3%, hole_block +30% against PrusaSlicer
  with `--infill-first`).

PrusaSlicer 2.9.6 against Mica before and after, XL 3 Benchys on T1/T3/T5 (`docs` inputs above;
"last" is the extrusion role printed last before each of the 480 toolchanges):

| | PrusaSlicer | Mica before | Mica after |
| --- | ---: | ---: | ---: |
| Last: external perimeter / sparse / solid / top / bridge | 190 / 142 / 130 / 12 / 6 | 116 / 234 / 76 / 52 / 2 | 168 / 127 / 172 / 5 / 8 |
| Retractions, tower | 6830 | 7242 | 6997 |
| Retractions, no tower | 6190 | 6536 | 6339 |
| Travel XY mm, tower | 197,480 | 214,008 | 200,131 |
| Travel XY mm, no tower | 97,018 | 114,607 | 99,599 |
| Time, tower | 3h09m45s | 3h11m08s | 3h09m00s |
| Time, no tower | 2h37m34s | 2h39m20s | 2h37m10s |

Single objects (`--center 125,110` CORE One, `180,180` XL; resynced command differences and
travel against PrusaSlicer): 3DBenchy CORE One 56,772 → 54,387, travel +0.74% → +0.48%; XL
58,869 → 55,805, +1.05% → +0.74%. cube20 and hole_block are unchanged (62 / 69 and 1,901 / 2,015;
travel +0.01% / +0.14% and +11.8% / +10.2%). The cube and two-cube jobs keep their metrics.

What remains is not order:

- **hole_block.** Role sequences already matched on 46 of 50 layers. The extra travel is the grid
  infill's connection topology: on 24 layers `FillBase.cpp::connect_infill` connects the mirrored
  pair of lines (about 7 mm more travel each). A harness from the pinned sources (upstream
  `make_expolygons`, `process_arachne`'s offsets and `WallToolPaths`, `FillRectilinear`) gives
  the reference topology on every middle layer, from Mica's raw slice loops and from Mica's own
  slices alike, and Mica's grid is bit-identical to upstream for Mica's surface. The difference is
  Mica's Arachne inner contour (the zero-width bead boundary): on layer 19 its outer ring has 88
  vertices against 76 upstream, so the simplified infill boundary keeps a different collinear
  vertex and the arc lengths that `connect_infill` sorts change.
- **Identical objects at different bed positions** are not sliced alike: in the 3-Benchy job
  objects 1 and 2 differ from object 0 on 53 and 75 of 240 layers (mostly seams), where
  PrusaSlicer prints all three identically. 3MF objects are sliced in bed coordinates.
- Benchy fill geometry (solid fill pieces, monotonic chaining) and Clipper 2 versus Clipper 6.4
  vertex sets (start points, collinear vertices) still differ; the Benchy single-object role
  sequences match on 140 of 240 layers.

U1: single-object output and the four-tool cube job are byte-identical. The four-tool Benchy 3MF
changes island order only (per-role filament and path length identical): Orca's
`Layer::make_slices` chains the same pre-order `PolyTreeToExPolygons` output with `chain_points`,
which keeps two islands in input order, so the hull now comes before the cabin as there.
Checked against native OrcaSlicer 2.4.1 on the same file (fresh datadir and the flattened
profiles from `scripts/prepare-wipe-tower-oracles.py`, `--slice 0 --arrange 0`): of the 156
layer/object pairs that have both the hull and the cabin, Orca prints the hull first on 112;
Mica agreed with Orca's island order on 44 before and on 88 after (hull first 72 → 116). Travel
362,414 mm against 363,058 before (Orca 331,075) and time 4h25m40s against 4h25m43s (Orca
4h25m16s) also move towards Orca; retraction counts are unchanged.

## XL tower exit sequence (2026-10-18)

A report said Mica retracted or wiped about twice as much as PrusaSlicer 2.9.6 when leaving the
XL tower. That did not reproduce on main. On the five XL cases below, every tower exit retracts
0.64 mm (80% `retract_before_wipe` of 0.8 mm) and wipes 0.16 mm along the tower wipe path, with
one lift, travel, Z restore and 0.8 mm deretraction, the same as PrusaSlicer. Only the
toolchange's own 20 mm retraction is inside the tower, in both. The M-codes around the exit
differed, and now match `WipeTower::tool_change`, `merge_tcr` and `append_tcr`:

- `tool_change` ends with `M220 R`, `G1 F<travel speed>`, `G4 S0`, `G92 E0`. `merge_tcr` then
  appends `finish_layer`. Mica wrote that trailer after the joined finish, without the feed
  rate.
- A `finish_layer` result on its own (for example the first layer of a painted object with one
  tool) has no `M220 B/S100/R`, `G4 S0` or `G92 E0`. Mica wrote all of them, plus `M591 R`.
- The final unload (`tool_change(-1)`) does not call `toolchange_Change`, so it writes no
  `G4 S0` / `M591 R`. Mica wrote one.
- `wipe_tower_acceleration = 0` sets no acceleration. Mica wrote `default_acceleration`.
  `GCodeWriter::toolchange` also keeps the last acceleration, so `append_tcr`'s
  `set_print_acceleration` after the tower writes nothing. Mica's tool selection forgot the
  acceleration, which added an `M204 P2500` before every exit retraction.

PrusaSlicer 2.9.6 against Mica before and after (inputs from `scripts/prepare-wipe-tower-oracles.py`
plus the corpus painted cube, and two projects with boxes of different heights: T1 10 mm / T2 30 mm,
and T1 30 / T2 10 / T4 20 mm; explicit 140 mm³ matrix; exit retractions are the pure-E retractions
between the last tower extrusion and the next model extrusion):

| Job | Exit retractions (mm) + wipe, all three | M204 P ref / before / after | M591 R | G4 S0 | Time ref / Mica |
| --- | --- | ---: | ---: | ---: | ---: |
| 2 cubes T1/T2 | 100 × 0.64 (64.0) + 16.0 | 1085 / 1189 / 1089 | 101 / 102 / 101 | 503 / 504 / 503 | 33m55s / 34m13s |
| 3 Benchys T1/T3/T5 | 481 (307.2) + 76.8 | 16237 / 16553 / 16073 | 481 / 482 / 481 | 2403 / 2404 / 2403 | 3h09m45s / 3h08m45s |
| Painted cube | 99 (63.36) + 15.84 | 1304 / 1572 / 1474 | 99 / 101 / 99 | 493 / 496 / 493 | 32m04s / 32m44s |
| Boxes T1/T2 | 51 (32.64) + 8.16 | 1085 / 1140 / 1089 | 52 / 53 / 52 | 258 / 259 / 258 | 36m44s / 36m59s |
| Boxes T1/T2/T4 | 152 (96.64) + 24.16 | 1627 / 1784 / 1633 | 152 / 153 / 152 | 758 / 759 / 758 | 55m12s / 55m22s |

`M220 B/S/R` and `G92 E0` counts now equal the reference in all five jobs (painted cube: 100 → 99).
Time estimates are unchanged, because Mica's estimator gives these commands no time. Whole-print
retraction counts are unchanged too (614/612, 6903/6834, 873/783, 516/514, 923/920 Mica/reference).
The remaining surplus is in model infill (painted cube: 275 against 183 infill-to-infill
retractions), not in the tower. The remaining `M204` gap is model role order (Benchys: Mica ends a
tool's block with solid infill more often). Single-tool CORE One/XL and U1 output is byte-identical.

## Purge into infill and objects

`wipe_into_infill` (per region: print settings, objects and modifiers) and `wipe_into_objects`
(per object only) are PrusaSlicer's options; OrcaSlicer's `flush_into_infill` /
`flush_into_objects` map onto them in profiles and 3MF files (`mapping.tsv`). Off by default.
Ported from `ToolOrdering.cpp` (`is_overriddable`, `collect_extruders`,
`WipingExtrusions::ensure_perimeters_infills_order`) and `ExtrusionOrder.cpp`
(`get_extruder_id`, overridden extrusions first) into `mica_core::tool_ordering` and
`PreparedPrint`.

What upstream actually does on a toolchanger (XL, CORE One with tools, U1):

- An overridable entity (sparse infill with `wipe_into_infill`; every wall and fill with
  `wipe_into_objects`; never soluble filament, supports, skirt or brim) does **not** ask for its
  own tool in `collect_extruders`. A region whose layer also has non-overridable fill (solid
  infill) still asks for both its solid and its sparse infill tool.
- `mark_wiping_extrusions` (the part that would let an extrusion absorb purge volume) marks
  nothing: PrusaSlicer calls it with the purge left after `filament_minimal_purge_on_wipe_tower`,
  which is zero without single-extruder multi-material (`extract_wipe_volumes`); Orca's
  WipeTower2 calls it only for SEMM `purge_in_prime_tower`. Mica refuses SEMM towers, so it is
  not ported and the tower volume per tool change is unchanged.
- On the layers the tower plans (up to the last tool change), `ensure_perimeters_infills_order`
  forces each overridable entity to the layer's last (walls: first) non-soluble tool, unless its
  walls print last and its own tool is on the layer; such forced paths print first with their
  tool. Elsewhere (no tower, layers above the last change) a path whose tool left the layer
  prints with the layer's last tool. Orca's version passes `last || !has_extruder(sparse)` as a
  tool index to `is_extruder_order` and skips support filaments; Mica keeps both.
- Objects printed one after another get no wiping (upstream's per-object ordering has no print
  config). `flush_into_support` (Orca, default on) has no effect without SEMM and stays unmapped.

So with walls on tool 1 and infill on tool 2, purging into infill saves the tool changes of every
layer without solid infill by printing that sparse infill with tool 1. PrusaSlicer 2.9.6 against
Mica, XL 5T 0.4 (`0.20mm SPEED @XL 0.4`, Prusament PLA, infill on T2, centered at 180,180):

| Job | PrusaSlicer T1 / T2 mm | Mica T1 / T2 mm | Tool changes (both) | Time PS / Mica |
| --- | ---: | ---: | ---: | ---: |
| cube20 | 1659.95 / 1263.30 | 1659.94 / 1263.23 | 92 | 30m51s / 31m30s |
| cube20, into infill | 2384.68 / 59.61 | 2384.68 / 59.55 | 2 | 25m03s / 26m25s |
| 3DBenchy | 5420.50 / 1529.99 | 5420.34 / 1530.36 | 116 | 1h25m43s / 1h29m06s |
| 3DBenchy, into infill | unchanged (each sparse layer also has solid infill) | unchanged | 116 | unchanged |
| 3DBenchy, into object | 4081.13 / 0 | 4081.40 / 0 | 0 | 58m24s / 59m50s |

Layer tool sequences are equal in all five (`scripts/compare-multitool.py`). Mica's extra
initial toolchange template call with a single used tool on the XL ("Change Tool-1 -> Tool0")
predates this change and also shows in a plain one-tool XL slice.

## Scope and review

The sources are pinned Prusa `WipeTower`, `WipeTowerIntegration`, wiping-layer scheduling and
`Print::_make_wipe_tower`, Orca `WipeTower2`, and Snapmaker-Orca's U1 speed/pressure handling.
See the per-file register in [PROVENANCE.md](../PROVENANCE.md).

Purging into infill and objects follows upstream (see [below](#purge-into-infill-and-objects)):
for independent tools it changes which tool prints a path, never the tower volume. MMU loading/cooling,
Bambu/type1 towers, tower ironing/interface specialization, sequential printing and mixed-nozzle
towers are outside this port. MMU, type1 and unequal nozzles are rejected. Front-edge priming and
bed containment use a rectangular bed extent; circular/concave beds need further work.
Soluble/forced-tool structural selection is present, but dissimilar-material interfaces have no
physical validation. The engine itself does no object/tower collision avoidance or automatic
placement; the GUI shows the tower on the bed, keeps it inside the printable area, warns about
and refuses to slice an overlap, and can move it to a free corner (see
[gui-prime-tower.md](gui-prime-tower.md); `wipe_tower::estimate` sizes the tower before a
slice). On the CLI, keep the chosen tower clear of model geometry, including brim and
stabilizers.

Review toolchange travel/retraction/template state, sparse-layer descent and final ram carefully
before hardware use. Cone/rib offsets reuse Mica geometry and rounded corners use linear segments;
these explain the small reference differences. Existing preheat and single-profile cooling
approximations remain. No hardware print has been run for this change.

Validation: offline release build, `cargo fmt --all`,
`cargo clippy --offline --workspace --all-targets -- -D warnings`,
`cargo test --offline --workspace` and `cargo xtask provenance`.
