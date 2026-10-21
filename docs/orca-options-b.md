# Orca wall, overhang, bridge and scarf options (batch B)

The new controls are opt-in on Prusa profiles. Orca imports retain and resolve their source names;
the U1 consumes its inherited profile values. Source logic was
verified against `orca@8500fcdcca`; executable references use installed OrcaSlicer **2.4.1**.

## API and CLI

`PrintSettings::from_config` resolves these typed process options. `Config::defaults` continues to
contain Prusa defaults only; `config/orca_options.tsv` adds optional option definitions. The GUI
uses Orca labels, tooltips, enum choices and page placement regenerated with:

```sh
export PATH="/opt/homebrew/opt/rustup/bin:$PATH"
CARGO_NET_OFFLINE=true cargo xtask gen-option-text \
  upstream/orca \
  upstream/prusaslicer
```

Use the existing CLI `--set key=value` interface; no separate subcommand is needed:

```sh
mica slice model.stl -o walls.gcode --set perimeters=3 \
  --set 'wall_sequence=inner-outer-inner wall'
mica slice model.stl -o reverse.gcode --set overhang_reverse=1 \
  --set overhang_reverse_internal_only=1 --set overhang_reverse_threshold=50%
mica slice model.stl -o bridges.gcode --set enable_extra_bridge_layer=apply_to_all \
  --set internal_bridge_angle=30 --set bridge_density=80% --set internal_bridge_density=85%
mica slice model.stl -o scarf.gcode --set seam_slope_type=external \
  --set seam_slope_conditional=1 --set scarf_angle_threshold=155 \
  --set scarf_overhang_threshold=40% --set seam_slope_min_length=10 \
  --set scarf_joint_speed=50% --set scarf_joint_flow_ratio=0.9
```

| Options | Resolved API and behavior |
|---|---|
| `wall_sequence` | `PrintSettings::orca_walls.inner_outer_inner`; all three source values preserved. Absent: existing `external_perimeters_first` controls ordering. Present: source enum controls ordering. Sandwich ordering starts after layer zero. Within a wall family, inset 2+ prints inside-out, then inset 0, then inset 1. Fewer than three walls fall back to outside-in after the first layer, as Orca does. |
| `overhang_reverse`, `overhang_reverse_internal_only`, `overhang_reverse_threshold` | `WallOptions`; zero-based odd layers only. Test unsupported lines against lower outlines offset by threshold minus half the bead width. A zero threshold reverses every eligible alternate layer. Detection propagates separately to contours and holes within the island. Internal-only excludes loops containing external-wall extrusion. Seam placement and winding-dependent metadata remain attached to the chosen direction. |
| `enable_extra_bridge_layer` | `ExtraBridgeLayer::{Disabled, External, Internal, All}` maps `disabled`, `external_bridge_only`, `internal_bridge_only`, `apply_to_all`. External layers are classified before full-density conversion and shell propagation; exclude exposed tops and wall-band-sized trivial areas. Internal extras intersect sparse/solid internal surfaces, open away trivial areas and leftovers, and use the preceding layer's last bridge angle plus 90°. Snapshot source bridges prevent cascading. |
| `internal_bridge_angle` | Degrees, zero selects automatic direction; positive values override the detected internal angle before anchor construction. |
| `bridge_density`, `internal_bridge_density` | Existing fixed-flow Orca bridge generation, now canonical optional typed options with GUI metadata. Fractions resolved from percent values; do not introduce Orca bridge policy into an unmodified Prusa config. |
| `seam_slope_type`, `seam_slope_conditional`, `scarf_angle_threshold`, `scarf_overhang_threshold` | Extend `ScarfSettings`. Source placement `none/external/all` maps to nowhere/contours/everywhere. Conditional scarves test the whole loop with nozzle-length arms, then signed unsupported width at the selected seam. The existing seam planner still chooses the seam. |
| `seam_slope_min_length`, `seam_slope_steps`, `seam_slope_entire_loop`, `seam_slope_start_height`, `seam_slope_inner_walls` | Length clamped to the loop; optional entire-loop ramps; maximum segment length is ramp length / steps. Start height accepts mm or percent of actual layer height and is capped at 99%. Inner-wall application is independent. |
| `scarf_joint_speed`, `scarf_joint_flow_ratio` | Ramp sections only. Speed accepts mm/s or percent of the wall's nominal role speed and remains capped by ordinary speed limits. Flow multiplies ramp extrusion. The flat middle retains ordinary speed/flow. Speed boundaries also feed cooling and preview planning. Speed, flow and the conditional angle/overhang controls can also extend native Prusa scarf settings without selecting source scarf placement. |

## Verified upstream functions

All paths below are relative to `orca@8500fcdcca`.

| Feature | Source |
|---|---|
| Classic and Arachne sandwich ordering | `src/libslic3r/PerimeterGenerator.cpp::PerimeterGenerator::process_classic`, `process_arachne`, `reorderPerimetersByProximity` |
| Alternating reversal | `src/libslic3r/PerimeterGenerator.cpp::detect_steep_overhang`, `traverse_loops`, `traverse_extrusions`, `reorient_perimeters` |
| External extra bridge classification and shell seed | `src/libslic3r/PrintObject.cpp::PrintObject::detect_surfaces_type` |
| Internal extra bridges and angle override | `src/libslic3r/PrintObject.cpp::PrintObject::bridge_over_infill` |
| Bridge density | `src/libslic3r/Fill/Fill.cpp::Layer::make_fills` |
| Conditional scarf, speed and flow | `src/libslic3r/GCode.cpp::GCode::extrude_loop`, `GCode::_extrude` |
| Whole-loop smoothness and ramps | `src/libslic3r/ExtrusionEntity.cpp::ExtrusionLoop::is_smooth`, `ExtrusionLoopSloped::ExtrusionLoopSloped` |
| Option definitions and text | `src/libslic3r/PrintConfig.cpp::PrintConfigDef::init_fff_params` |

## Adaptation boundaries to review

The existing Prusa seam-placement and ramp generator remain in place. Orca's geometry tests,
start-height interpretation, speed and flow controls extend them. Segment subdivision uses Mica's
equal subdivisions rather than Orca's recursive bisection, and seam-gap clipping retains Mica's
ramp interpolation. Cylinder ramp segmentation and precise vertex Z positions can therefore differ.
Mixed-width Arachne scarves project ramp edges onto existing flow runs; width/bridge roles survive
reversal and subdivision. This geometric ownership uses linear searches only while scarves are on.

Sandwich ordering reuses Mica's classic containment families and Arachne adjacency/ownership groups.
It does **not** port Orca's additional proximity re-clustering of disconnected Arachne fragments.
Full Benchy path order consequently needs close review; a passing three-wall cube is insufficient
proof of equivalence on every thin or merging wall family. Relative/model-aligned internal bridge
angle controls and unrelated wall-direction settings are outside this batch.

The existing classic-wall path still lacks some Orca one-wall-on-top, gap-fill and unsupported-wall
behavior. The oracle suite leaves the actual U1 profile settings in place and records these gaps;
it does not disable unrelated profile options to make role totals pass.

## Reproducing the oracles

```sh
export PATH="/opt/homebrew/opt/rustup/bin:$PATH"
cargo build --offline --release -p mica-cli
python3 scripts/orca-options-b-oracle.py --output /tmp/mica-orcaopts-b-review
```

The script creates the staircase, bridge fixtures and a closed 128-sided cylinder in the supplied
temporary directory. Wall cases use three walls on cube20 and Benchy, with both classic and Arachne;
reversal cases use the staircase; bridge cases use pillars, a cavity and an internal roof; scarf
cases use the cylinder with conditional placement both off and on. Every case writes flattened
installed U1 machine/process/material inputs, `commands.sh`, reference/Mica G-code, validator JSON,
per-layer structural diagnostics and a numeric summary. Four explicit filament slots are always
passed to Orca. The script stops all further Orca launches on the first failure. It exits nonzero
when a role, structural or validation gate fails; `--reuse-references` reruns Mica against saved successful
references. Reference outputs are never stored in the repository.

The exact reference command for each case has this form (absolute paths and working directory are
recorded in its generated `commands.sh`):

```sh
cd "$ref"
/Applications/OrcaSlicer.app/Contents/MacOS/OrcaSlicer --datadir "$ref/data" \
  --load-settings "$ref/machine.json;$ref/process.json" \
  --load-filaments "$ref/filament.json;$ref/filament.json;$ref/filament.json;$ref/filament.json" \
  --slice 0 --arrange 1 --allow-rotations=0 --outputdir "$ref" \
  --export-settings "$ref/resolved.json" "$model"
```

Without `--allow-rotations=0` Orca's arrange may rotate the model; pass `--allow-rotations` to the
oracle script to reproduce that behaviour.

Outputs stay in the system temporary directory; the pinned checkouts are read-only.

## Results

**Superseded 2026-10-17:** all twelve cases now meet ±3 % for every role ≥ 1 mm. The oracle now passes
`--allow-rotations=0` to Orca and slices Mica at Orca's object centre; the original run below
compared Orca's rotated arrangement with unrotated Mica slices. The original results follow.

The strict Orca acceptance gate was **not fully met**. Six of twelve cases met ±3% for every
material role of at least 1 mm (after combining Orca bottom surface with internal solid infill).
All 24 reference/Mica files validate with **0 errors**. Full per-role filament totals, percentages
and structural measurements are in [the numeric results](orca-options-b-results.json). The script
returns exit 1 for the remaining role failures.

| Orca 2.4.1 case | Per-role ±3% | Structural observation |
|---|---|---|
| Three-wall cube, classic | Fail: top −16.80% | Correct sandwich on all 99 shared three-wall layers; final-layer wall count differs due to existing classic single-wall-top gap |
| Three-wall cube, Arachne | Pass | Wall-role sequence matches all 100 layers; 99 three-wall layers |
| Three-wall Benchy, classic | Fail | Exact role-run sequence matches 81/240 layers; classic top/gap/overhang differences prevent full equivalence |
| Three-wall Benchy, Arachne | Fail: internal bridges −9.34%; small brim +24.90% | Exact role-run sequence matches 93/240 layers; disconnected/thin wall grouping is not fully equivalent |
| Staircase, classic | Fail | Same 40 reversed layers: zero-based 71,73,…,149 |
| Staircase, Arachne | Fail: external bridges +6.00%, top −3.52% | Same 40 reversed layers: zero-based 71,73,…,149 |
| Extra external bridge, pillars | Fail: external bridges −3.53% | Bridge layers 50/51 and internal bridge 55 match |
| Extra external bridge, cavity | Pass | External bridge layers 60/61 match; internal bridges 5 and 65 match |
| Extra internal bridge, internal roof | Pass | Internal bridge layers 55/56 match; explicit 30°/120° checked in engine tests |
| Extra bridge, apply-to-all, internal roof | Pass | Internal bridge layers 55/56 match |
| Cylinder scarf, unconditional | Pass | 14 rising segments at 100 mm/s in both; measured rising XY length 9.920 mm Orca / 9.997 mm Mica for configured 10 mm |
| Cylinder scarf, conditional | Pass | Same ramp count, speed and lengths as unconditional cylinder |

Classic Benchy's failing roles are gap infill −34.65%, inner wall +8.35%, internal bridges
−14.85%, missing overhang-wall role, and a small brim +24.90%. Classic staircase additionally has
external bridges +7.90%, inner walls +4.11%, outer walls +4.76%, top −11.68%, and a missing
separate overhang-wall role. These comparisons retain the source U1 defaults. The saved pre-change
binary reproduces the classic top/gap/wall differences and the pillar's first-bridge shortfall.
Benchy ordering still requires review against Orca's proximity grouping; the cube result does not
establish full Benchy equivalence.

Safety and default-regression checks:

- `cargo fmt --all`, workspace Clippy with `-D warnings`, and provenance checks pass.
- `cargo xtask corpus --quick` passes for CORE One, XL and U1, with zero validation errors.
- `cmp` passes for cube20 **and** 3DBenchy on both CORE One and XL before/after; all four outputs
  validate with zero errors.
- `cargo test --offline --workspace` passes with the pinned Prusa profiles and a default two-wall
  Orca cube reference supplied through `MICA_U1_ORCA_REFERENCE`, which avoids an Orca launch.


The clean baseline was commit `ec7954f93c406c454dfc28b50f23c829ba73d697`. The matching
CORE One and XL text-G-code checksums are:

| Printer | Model | SHA-256 (before = after) |
|---|---|---|
| core | cube20.stl | `d4fc843cd8b4d25833823934507d3ae656d50ac0c0a5ce4bb1f90bc39512289e` |
| core | 3DBenchy.stl | `0e782b80f4054924906bea23d365924ac3782a7d16eefa9094bd99f3984f5093` |
| xl | cube20.stl | `139f80591386929d926c138b43ef30477b2c95ffebda093ca4ead38ae70d2ebc` |
| xl | 3DBenchy.stl | `4532ea45fd6fc2ce5fb89a4ca372aca5bc85573001ebd4ce60ee4e38c8ad8a85` |

The workspace startup/time oracle uses the separately generated **default two-wall** cube
(15m 8s), not a batch B three-wall reference.
Its presets come from `scripts/prepare-u1-oracle.py` without process overrides; the CLI command is
the same four-filament form above.

Structural diagnostics include G1/G2/G3 winding, close the seam gap geometrically for the
orientation test, and join contiguous supported/overhang wall fragments. Benchy role-sequence
counts are diagnostics of emitted wall runs, including thin/open runs, rather than inset labels.

Final verification completed with the default U1 reference; all workspace tests passed.
