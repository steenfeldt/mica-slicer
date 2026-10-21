# Slicing rules: PrusaSlicer or OrcaSlicer

Mica has two engines in one codebase: PrusaSlicer 2.9.6's rules (the CORE One and the XL) and
OrcaSlicer's (the Snapmaker U1). One printer option, `mica_slicing_rules`, picks the engine. The GUI
calls it **Slicing rules**. Its values are `prusaslicer` and `orcaslicer`.

The G-code flavor (`gcode_flavor`) is a separate setting. It only decides the **firmware dialect**:
Klipper's EXCLUDE_OBJECT, SET_VELOCITY_LIMIT, SET_PRESSURE_ADVANCE and its file layout, versus Prusa
firmware's M486, M572, M201-M205 and its metadata.

Before this change Mica chose the engine for each feature in one of two ways:

- about 22 `gcode_flavor == "klipper"` checks;
- about 11 "is this Orca-only key in the config" checks.

So a Klipper printer with a Prusa-style profile got an untested mix. On the CORE One and the XL,
merely setting an Orca option flipped seams, supports or bridges to Orca's algorithm (settings-menu
audit, findings 3 and 4). Every engine decision now asks one helper, `mica_core::rules::Rules::of(&config)`.

## The option

| | |
|---|---|
| Key | `mica_slicing_rules` (`crates/mica-core/src/config/mica_options.tsv`, printer scope) |
| Values | `prusaslicer` (the option default), `orcaslicer` |
| Helper | `mica_core::rules::Rules::of`, `crates/mica-core/src/rules.rs` |
| Companion | `mica_orca_values` = `orca_profile` (default) \| `orca_defaults`: where Orca-only values come from (see below) |

**Default by profile origin:**

| Profile | Rules | Where |
|---|---|---|
| Prusa bundle printer | PrusaSlicer | the option default (`Config::defaults`) |
| Orca vendor profile (Snapmaker, RatRig, Orca's own Prusa profiles, …) | OrcaSlicer | `mica_profiles::orca::map_config` |
| Custom printer (user printer preset with no system parent) | OrcaSlicer | `mica_profiles::prusa::Bundle::full_config` |
| Orca project / Prusa project | the project's dialect, unless it records its own choice | `ProjectSettings::default_rules` (`crates/mica-profiles/src/project.rs`) |

A config without the key, such as a hand-built test config, follows PrusaSlicer.

**Where the choice is saved:**

- **User printer presets:** the key is an ordinary printer option, so it is stored when it differs
  from the parent.
- **Prusa INI and 3MF:** a plain `mica_slicing_rules = …` line. PrusaSlicer ignores unknown keys.
- **Orca 3MF:** Mica's sidecar (`Metadata/mica.json`, `mica_settings`). Orca's
  `project_settings.config` only ever holds Orca options
  (`Project::update_settings`, `SaveSettings::write`).
- **Default `.bgcode` files:** the slicer-metadata block leaves the `mica_*` keys out under
  PrusaSlicer rules, so it stays byte-identical to before. Under Orca rules they are included.

**CLI:**

- `--slicing-rules prusaslicer|orcaslicer`, or `--set mica_slicing_rules=…`.
- `--vendor Orca:Prusa` loads OrcaSlicer's own Prusa profiles. Plain `Prusa` is PrusaSlicer's bundle.

## Filling in the other engine's settings

`crates/mica-profiles/src/rules_fill.rs`:

- **Orca rules on a PrusaSlicer profile.** Options PrusaSlicer has no equivalent for are filled in.
  These are Orca's own keys (Z-hop type, Orca's scarf extras, Orca's bridge density, …; not the merged
  wall order and scarf seam, which keep the Prusa profile's value) and the optional Orca
  table. They come from OrcaSlicer's matching profile when Orca ships one; otherwise from OrcaSlicer's
  defaults.
  - **Matching.** The printer is matched by name tokens, so "Prusa CORE One HF0.4 nozzle" finds
    "Prusa CORE One HF 0.4 nozzle", and "Original Prusa XL - 5T Input Shaper 0.4 nozzle" finds
    "Prusa XL 5T 0.4 nozzle". Process and filament are matched among the presets compatible with that
    printer. If one of them has no counterpart (a user's own filament, say), the Orca printer's
    default one is used.
  - **What is never touched.** Anything the Prusa profile sets, and Orca keys that map onto a
    PrusaSlicer option (`outer_wall_speed`, `nozzle_temperature`, …), keep the Prusa value.
  - **Filament overrides.** Orca's filament overrides of a filled option (`filament_z_hop_types`)
    are left unset (`nil`), so the printer value applies.
  - **Where it runs.** The CLI (`mica_cli` `apply_slicing_rules`) and the GUI (`App::recompute_config`)
    run the same fill, so their G-code stays identical.
  - **Reporting.** The CLI prints a note naming the source, e.g. "367 Orca-only settings from
    OrcaSlicer's profiles Prusa CORE One HF 0.4 nozzle / 0.20mm SPEED @CORE One HF 0.4 /
    Prusament PLA @CORE One HF 0.4". The GUI note names the Orca printer. The "Take the Orca-only
    settings from OrcaSlicer's own … profile" box sets `mica_orca_values`; off means Orca's defaults.
- **PrusaSlicer rules on an Orca profile.** No lookup is needed. Orca profiles are mapped onto
  `Config::defaults`, so every PrusaSlicer-only option already holds PrusaSlicer's default.
  `rules_fill::prusa_only_options` lists them: ramping travel lift, cooling slowdown logic,
  `autoemit_temperature_commands`, `bed_temperature_extruder`, …

Two limits:

- Multi-tool fill uses tool 1's filament counterpart for every tool.
- The Orca-only values are not written back into the Prusa preset. They are re-derived on each
  resolution and appear in projects only through the resolved configuration.

## Site classification

The line numbers are from this commit.

**Engine** means the decision now reads `Rules::of`. **Firmware** means it stays on `gcode_flavor`.
**Templates** means Orca-format template variables (`orca_templates = klipper || orca`): Klipper
printers' profiles come from Orca, and Orca rules mean Orca's variable set.

### Former "Orca key present" checks: all engine

| # | Site | Old test | Now |
|---|---|---|---|
| 1 | `crates/mica-core/src/layer_profile.rs:70` | `initial_layer_print_height` present | Orca's tool-index rule for layer heights |
| 2 | `crates/mica-core/src/resolve.rs:329` | `initial_layer_print_height` present | complete layer heights |
| 3 | `crates/mica-core/src/resolve.rs:390` | `support_remove_small_overhang` present | Orca support semantics (audit finding 3) |
| 4 | `crates/mica-core/src/resolve.rs:445` | `min_width_top_surface` present | Orca's top-surface wall extension |
| 5 | `crates/mica-core/src/resolve.rs:466` | `bridge_density` present | Orca bridge style (audit finding 3) |
| 6 | `crates/mica-core/src/resolve.rs:516` | `top_bottom_infill_wall_overlap` present | Orca's top/bottom infill-wall overlap (25% if unset) |
| 7 | `crates/mica-core/src/sequential.rs:50` (`orca_rules`, used at `:62`, `:102`, `print_job.rs:572`) | `initial_layer_print_height` present | Orca's sequential clearance and order |
| 8 | `crates/mica-core/src/brim/settings.rs:124` | `initial_layer_print_height` present | Orca skirt/brim (`adhesion.orca`; also gates `infill_first` at `print.rs:1166`) |
| 9 | `crates/mica-core/src/seam/settings.rs:189` | `seam_slope_type` present | Orca seam placer and scarf (audit finding 3); the scarf values themselves are one merged setting (`mica_core::merged`) |
| 10 | `crates/mica-core/src/wall_options.rs:54` | `bridge_line_width` present | Orca's overhang bridge width; ignored under Prusa rules |
| 11 | `crates/mica-gcode/src/cooling.rs:79` | `slow_down_for_layer_cooling` present | Orca CoolingBuffer (uniform slowdown, fan rules) |
| 12 | `crates/mica-gcode/src/overhang_plan.rs:276` | same | Orca cooling slows bridges |
| 13 | `crates/mica-gcode/src/multi.rs:470` | same (absent) | Prusa's load/unload time in the cooling estimate |
| 14 | `crates/mica-gcode/src/orca_motion.rs:63` (`enabled`; used by `travels.rs`'s `is_orca`, `export.rs:813,1206,1571,1829`, `multi.rs:291`, `prusa_wipe.rs:103`, `export_batch.rs:103`, `wipe_tower.rs:333`) | `z_hop_types` present | Orca travel/lift/wipe/toolchange motion |
| 15 | `crates/mica-gcode/src/extrusion_speed.rs:43` | `outer_wall_speed` present | Orca small-perimeter speed |
| 16 | `crates/mica-gcode/src/extrusion_speed.rs:70` | `initial_layer_infill_speed` present | Orca first-layer infill speed |
| 17 | `crates/mica-gui/src/brim_ears.rs:84` | `initial_layer_print_height` present | ear brim types offered |
| 18 | custom G-code `Dialect`: `crates/mica-gcode/src/custom_gcode.rs:36` (`Plan::new(…, orca)`), `crates/mica-gui/src/preview/color_print.rs:21` | `klipper` | Orca counts filaments / Orca layer rule |

### Former `gcode_flavor == "klipper"` checks

| Site | Class | Decision |
|---|---|---|
| `crates/mica-core/src/wipe_tower/mod.rs:98` | **engine** | Orca WipeTower2 vs Prusa tower |
| `crates/mica-gcode/src/travels.rs:58` | **engine** | Prusa ramping lift off under Orca rules (Z-hop type decides) |
| `crates/mica-gcode/src/writer.rs:162` | **engine** | Prusa CoolingBuffer feed-rate rules: a retraction's repeated F and a zero-length layer arrival at the current F are dropped, each layer starts without a current F; travels use `XYZ_EPSILON` and truncated scaled Z |
| `crates/mica-gcode/src/travels.rs:85` | **engine** | Prusa elevated travels on scaled points with Marlin 2's parabola blend (`get_smoothing_params` keeps the firmware check: only `marlin2` blends) |
| `crates/mica-gcode/src/multi.rs:240` | **engine** | Prusa runs the toolchange template at startup |
| `crates/mica-gcode/src/multi.rs:393` | **engine** | Prusa CoolingBuffer re-emits the fan after a toolchange |
| `crates/mica-gcode/src/multi.rs:433` | **engine** | Prusa OozePrevention waits after a toolchange |
| `crates/mica-gcode/src/export.rs:361` | **engine** | Orca `bed_temperature_formula` vs Prusa `bed_temperature_extruder` |
| `crates/mica-gcode/src/export.rs:580` | **engine** | Prusa autoemit of temperatures around the start G-code |
| `crates/mica-gcode/src/export.rs:662` | **engine** | Prusa ooze-prevention wait after start |
| `crates/mica-gcode/src/export.rs:677` | **engine** | Orca: explicit `T<n>`, fan reset and aux fan after start |
| `crates/mica-gcode/src/export.rs:784` | **engine** | Orca `initial_layer_travel_speed` |
| `crates/mica-gcode/src/export.rs:825,851` (`sequential.rs`) | **engine** | Orca/Prusa move to the next object, first-layer temperatures |
| `crates/mica-gcode/src/export.rs:1109` | **engine** | Prusa CoolingBuffer times custom G-code |
| `crates/mica-gcode/src/export.rs:1159,1314` | **engine** | fan before (Prusa) / after (Orca, `;_SET_FAN_SPEED_CHANGING_LAYER`) the layer change |
| `crates/mica-gcode/src/export.rs:1172-1177,1229-1234` | **engine** | Orca runs the layer templates on layer 1, with `max_layer_z` incl. the current layer |
| `crates/mica-gcode/src/export.rs:1195` | **engine** | Prusa starts the layer's first label before the layer-change retraction |
| `crates/mica-gcode/src/export.rs:1298,1326-1345` | **engine** | Prusa vs Orca placement of per-layer custom G-code; Orca aux fan and first-layer accel |
| `crates/mica-gcode/src/export.rs:1381` | **engine** | Prusa "move after unknown position" after a toolchange |
| `crates/mica-gcode/src/export.rs:1446` | **engine** | Orca opens the label at once |
| `crates/mica-gcode/src/export.rs:1589,1683` | **engine** | Orca travel/role acceleration + jerk vs Prusa `M204` (the writer emits per flavor) |
| `crates/mica-gcode/src/export.rs:1609` | **engine** | Prusa short-travel acceleration onto external perimeters |
| `crates/mica-gcode/src/export.rs:1624` | **engine** | Orca descends in the move to a sequential object |
| `crates/mica-gcode/src/export.rs:1743,1774` | **engine** | Prusa resets the default acceleration after paths/layers |
| `crates/mica-gcode/src/export.rs:1803-1804` | **engine** | label kept across layers (Prusa) / closed per layer (Orca) |
| `crates/mica-gcode/src/export.rs:1840` | **engine** | end: Orca fan off + aux fan vs Prusa `M107` |
| `crates/mica-gui/src/tower.rs:75,495`, `crates/mica-gui/src/panels/tower.rs:192` | **engine** | tower kind, name and options in the GUI |
| `crates/mica-gcode/src/export.rs:456,473,488,643,1873,1883` | templates | Orca variables, `orca::run`, layer var −1 at start, current extruder per filament end |
| `crates/mica-gcode/src/export.rs:413` | firmware | Klipper needs relative E |
| `crates/mica-gcode/src/export.rs:434,1224,1347,1837` | firmware | EXCLUDE_OBJECT define/start/end |
| `crates/mica-gcode/src/export.rs:502,559,629,1888` | firmware | Klipper file layout (HEADER/EXECUTABLE blocks, `G90/G21`) |
| `crates/mica-gcode/src/export.rs:540`, `thumbnails/mod.rs:145` | firmware | thumbnail block format and camera |
| `crates/mica-gcode/src/export.rs:670` | firmware | `SET_PRESSURE_ADVANCE` at start |
| `crates/mica-gcode/src/export.rs:1165-1166,1696` | firmware | Klipper hosts' comment formats (`;Z:` decimals, Orca `;TYPE:` names) |
| `crates/mica-gcode/src/export.rs:1777,1893` | firmware | M486 labels, `objects_info` (Prusa firmware) |
| `crates/mica-gcode/src/multi.rs:174,188,199,222` | firmware | EXCLUDE_OBJECT vs M486 labels |
| `crates/mica-gcode/src/multi.rs:448`, `orca_toolchange.rs:181`, `calib.rs:124,287,308`, `writer.rs:275` | firmware | `SET_PRESSURE_ADVANCE` vs `M572` |
| `crates/mica-gcode/src/writer.rs:397,415` | firmware | `SET_VELOCITY_LIMIT` vs `M204/M205` |
| `crates/mica-gcode/src/smooth_path.rs:40` | firmware | Klipper's G2/G3 parser has no `R` |
| `crates/mica-gcode/src/export.rs:199` (`machine_envelope`) | firmware | `M201`… only for Marlin 2 |
| `crates/mica-gcode/src/time_estimate/mod.rs:206,778`, `planner.rs` | firmware | the estimator models the firmware (junction deviation, silent mode, M73 format) |
| `crates/mica-gcode/src/validate.rs:139` | firmware | validator dialect |
| `crates/mica-gui/src/preview/layer_gcode.rs:125` | firmware | Klipper has no M600: "Change filament" pause |

## GUI

- **Printer settings → Basic information.** A new first group, "Slicer", holds **Slicing rules**
  (PrusaSlicer / OrcaSlicer). The page opens with one line: "Slicing rules: which slicer's behaviour
  Mica follows for walls, seams, travel, cooling and tool changes. Firmware commands are set by
  G-code flavor."
  - The row lives in `crates/mica-gui/src/settings/mica_text.tsv`, its help text in `help_text.tsv`, the placement in `mica_pages.tsv`.
    Both are hand-written next to the generated tables.
- **Printer card badge.** "PrusaSlicer rules" or "Orca rules". It is accented when the choice
  differs from the profile's origin, and clicking it opens Basic information.
- **Note when the choice differs from the profile's origin.** "This profile is tuned for
  PrusaSlicer. G-code will follow OrcaSlicer's rules." It adds the choice to take the Orca-only
  settings from OrcaSlicer's own matching printer profile (or Orca's defaults). In the other
  direction it says that PrusaSlicer-only settings use PrusaSlicer's defaults.
- **Merged settings show once, on every printer** (`mica_core::merged`, the full list in
  [setting-names.md](setting-names.md)). Wall order (Orca's `wall_sequence`), the scarf seam
  (`seam_slope_*`) and Label objects (`gcode_label_objects`) are one row each under both rules.
  - The Orca key is Mica's. A PrusaSlicer profile has only the counterpart
    (`external_perimeters_first`, `scarf_seam_*`), so the row shows its value translated; an edit
    writes both keys in one undo step. The counterparts never show.
  - Engines: PrusaSlicer rules run PrusaSlicer's scarf code with the Orca values it has
    (placement, smooth-only, entire loop, inner walls, length, start height, segment length from
    the steps); Orca's joint speed, flow ratio and angle/overhang thresholds stay hidden and
    neutral there. Orca rules run Orca's scarf with the PrusaSlicer values translated. All three
    wall orders work under both rules.
  - Object labels: the firmware decides the form. Firmware-specific is M486 on Marlin and
    EXCLUDE_OBJECT on Klipper (as PrusaSlicer's LabelObjects); OctoPrint comments are the same
    everywhere. Orca's Label objects + Exclude objects map onto it.
  - Files: PrusaSlicer INI/3MF get the counterparts written back (the Orca keys stay next to them;
    PrusaSlicer ignores them). On import a counterpart that disagrees with its Orca key was edited
    in PrusaSlicer, so it wins. Orca 3MFs get Orca's keys. `mica profile export prusaslicer`
    writes a Mica profile as a PrusaSlicer user preset.
  - Orca rules on a PrusaSlicer profile no longer fill the merged keys from OrcaSlicer's profile:
    the profile's own value applies.
- **Same group name, rule-specific contents.** *Travel lift* (printer Extruder page and filament
  Advanced page): ramping lift, max lift and lift before obstacle under PrusaSlicer rules, Z-hop
  type under Orca rules. *Bridges* (Quality): bridge line width under Orca rules; flow, densities,
  thick bridges and the extra bridge layer under both. *Cooling slowdown* (filament Cooling):
  slowdown logic and perimeter transition distance under PrusaSlicer rules. The groups come from
  `RULE_GROUPS` in `xtask/src/gen_option_text.rs`.
- **Settings that don't apply are hidden** (`settings::applies`), unless they carry an unsaved change.
  - Hidden under Orca rules:
    - cooling slowdown logic, perimeter transition distance;
    - infill first;
    - ramping lift, lift before obstacle and max lift, both the printer and the filament versions;
    - autoemit temperatures;
    - bed temperature by extruder;
    - short-travel acceleration.
  - Hidden under PrusaSlicer rules:
    - Orca's scarf extras (`scarf_angle_threshold`, `scarf_overhang_threshold`,
      `scarf_joint_speed`, `scarf_joint_flow_ratio`);
    - Ignore small overhangs;
    - bridge line width;
    - Z-hop type.
  - Orca features that work under both rules stay visible: overhang reversal, extra bridge layer,
    bridge densities, internal bridge angle.
- **Undo, the unsaved marker and the slice cache.** The switch is an ordinary printer edit, so it is
  covered by undo and the unsaved marker. The slice cache keys on the resolved configuration, so it
  is covered there too. `cli_parity_tests::rules_case` checks all three.

## Tests

- **`crates/mica-cli/tests/slicing_rules.rs`.** Three cases:
  - **CORE One like OrcaSlicer.** Orca cooling markers appear; no Klipper commands; Auto Lift and
    Normal Lift differ; Orca scarf takes effect; the result validates with 0 errors.
  - **U1 like PrusaSlicer.** The Prusa path, with EXCLUDE_OBJECT and SET_VELOCITY_LIMIT kept;
    0 errors.
  - **Orca keys on the CORE One under PrusaSlicer rules.** The G-code is byte-identical to the
    untouched preset.
- **`crates/mica-gui/src/cli_parity_tests.rs`.** GUI = CLI for Orca rules on the CORE One and
  PrusaSlicer rules on the U1, both direct and through a saved Prusa or Orca project (sidecar).
- **Unit tests.** `rules.rs`, `rules_fill.rs` (name matching, fill scope, idempotence) and
  `project.rs` (project dialect and explicit choice).

## A/B: Orca rules + Orca's CORE One profile vs native OrcaSlicer

Setup:

- **Model and profiles:** Benchy on Orca's own "Prusa CORE One HF 0.4 nozzle", "0.20mm SPEED @CORE One
  HF 0.4" and "Prusament PLA @CORE One HF 0.4", on the Textured PEI Plate.
- **Mica:** `mica slice 3DBenchy.stl --vendor Orca:Prusa --bed "Textured PEI Plate"
  --export-3mf benchy.3mf --flavour orca`.
- **Native Orca:** OrcaSlicer 2.4.1 slicing the same 3MF with
  `OrcaSlicer --datadir <tmp>/data --slice 1 --outputdir <tmp> benchy.3mf`.

Per-role filament is E mm; time is path length / F, without acceleration (the same method for both):

| Role | Mica mm | Orca mm | Δ % | Mica s | Orca s | Δ % |
|---|---:|---:|---:|---:|---:|---:|
| Outer wall | 1097.7 | 1097.7 | 0.0 | 582 | 571 | +2.0 |
| Inner wall | 1146.3 | 1145.1 | +0.1 | 507 | 488 | +3.8 |
| Sparse infill | 600.0 | 608.7 | −1.4 | 194 | 189 | +2.8 |
| Internal solid infill | 632.1 | 630.4 | +0.3 | 165 | 157 | +5.4 |
| Top surface | 114.8 | 114.8 | 0.0 | 44 | 44 | +0.5 |
| Bridge | 208.0 | 206.4 | +0.8 | 75 | 74 | +1.4 |
| Overhang wall | 5.2 | 5.2 | +0.5 | 4 | 4 | +3.2 |
| Total extrusion | 3821.4 | 3825.6 | −0.1 | 1577 | 1532 | +2.9 |
| Travel | 25295 mm, 6010 moves, 161 s | 23146 mm, 5257 moves, 155 s | +9.3 (length) | | | +4.3 |

- **Retractions:** 1783 for Mica, 1773 for Orca.
- **Own time estimates:** Mica 42 min 11 s, Orca 43 min 18 s.
- **Validation:** Mica's file validates with 0 errors.
- **Fixed while measuring.** Two Orca-path differences were found and fixed. Orca writes slope and
  XYZ travels at the travel speed: `GCodeWriter::travel_to_xyz` does not limit them by
  `travel_speed_z`. Orca's writer also counts a retraction's F as the current speed. (PrusaSlicer
  does the same in its CoolingBuffer, so this is no longer a rules switch: the writer always does it.)
  - The U1 never showed either difference, because its `travel_speed_z` is 0 and it uses Auto Lift.
- **What is left, not yet investigated:**
  - about 9% more travel length and 14% more travel moves;
  - +3–5% extrusion time in the walls and solid infill.

  Likely suspects are the avoid-crossing routes and the cooling slowdown speeds.
