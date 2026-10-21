# Snapmaker U1: Orca profiles and Klipper output

Mica supports the U1's four independent tools, separate objects/instances, object/part assignments,
profile toolchange templates and timed preheating. See [the multi-tool API, CLI and oracles](multitool.md).
The U1 prime tower is implemented; see [tower scope and oracles](wipe-tower.md).
Vendor presets are read from the user's installation, never bundled.
The single-tool startup and profile-import behavior below remains the baseline.

## CLI

The explicit `--vendor` selects the profile format: `Prusa` (the existing default) uses the INI
bundle; any other name selects that Orca vendor's JSON index. For example:

```sh
mica profiles printer --vendor Snapmaker
mica profiles print --vendor Snapmaker --printer 'Snapmaker U1 (0.4 nozzle)'
mica profiles filament --vendor Snapmaker --printer 'Snapmaker U1 (0.4 nozzle)' \
  --print '0.20 Standard @Snapmaker U1 (0.4 nozzle)'
mica slice tests/models/cube20.stl --vendor Snapmaker \
  --printer 'Snapmaker U1 (0.4 nozzle)' \
  --print '0.20 Standard @Snapmaker U1 (0.4 nozzle)' \
  --filament 'Snapmaker PLA @U1' --bed 'High Temp Plate' -o /tmp/u1.gcode
```

`--vendor Snapmaker` defaults to U1 0.4, 0.20 Standard, and **Generic PLA @System**. That library
filament has a nonzero temperature for the machine's default Textured PEI Plate. The oracle uses
**Snapmaker PLA @U1**, which has 55°C in `hot_plate_temp` but **0°C in its textured-plate values**.
Use `--bed 'High Temp Plate'` with that filament, matching the plate actually installed. The
serialized enum is `High Temp Plate`; the filament's separate `bed_type` metadata says `Hot Plate`.
`--bed` defaults to the machine's `default_bed_type`. Mica does not invent temperatures for a plate.

One selected filament fills all physical tool slots, so indexed machine and filament templates have
four entries; `is_extruder_used` is `[true,false,false,false]`. `full_config[_with_project]` checks
printer and process compatibility lists/conditions, including user-machine parent matches, before
composition. Explicit lists win over expressions. Expression parse errors count as compatible, as
in Orca and Prusa; missing presets and explicit mismatches are errors.

`--profiles-dir /path/to/resources/profiles` overrides discovery. Otherwise discovery checks:

1. `/Applications/OrcaSlicer.app/Contents/Resources/profiles`.
2. Linux FHS `/usr/local/share/OrcaSlicer/profiles`, `/usr/share/OrcaSlicer/profiles`;
   Flatpak `/app/share/OrcaSlicer/profiles` or `/app/resources/profiles`; `/opt/OrcaSlicer/resources/profiles`;
   AppImage `$APPDIR/resources/profiles`; Windows `%ProgramFiles%`, `%ProgramFiles(x86)%`,
   `%LOCALAPPDATA%/Programs/OrcaSlicer/resources/profiles` and `%LOCALAPPDATA%/OrcaSlicer/resources/profiles`.
   Executables on PATH also locate portable resources beside the Windows binary or one level above
   a Linux `bin` directory. Host Flatpak installs are checked under `/var/lib/flatpak` and
   `~/.local/share/flatpak`, for `com.orcaslicer.OrcaSlicer` and legacy `io.github.softfever.OrcaSlicer`,
   x86_64/aarch64 stable active deployments, in `files/share/OrcaSlicer` and `files/resources`.
3. This checkout's `upstream/orca/resources/profiles`.

These layouts come from pinned Orca `CLI::setup`, its CMake install rules and Flatpak manifest.
Custom install prefixes, running AppImages outside their mount, and non-stable Flatpak deployments
can use `--profiles-dir`. The shared `OrcaFilamentLibrary` loads before the selected vendor.
`--bundle` remains Prusa-only. Tests use the pinned checkout at
`upstream/orca/resources/profiles`.

## Flavor behavior

`export_gcode` still has one layer/path pipeline. `Writer` selects `Flavor::PrusaBuddy` or
`Flavor::Klipper`. The existing Prusa fixtures are unchanged. `export_gcode_with_mesh` adds the
positioned mesh needed for the true XY projected convex hull; exclusion without a mesh errors
rather than substituting a rectangle. Object names follow Orca's sanitizer and `_id_0_copy_0`
suffix, with control characters also sanitized. Polygon coordinates include the closing vertex.

- Klipper uses deduplicated `SET_VELOCITY_LIMIT ACCEL=… ACCEL_TO_DECEL=…`, adding square corner
  velocity when jerk is enabled. Acceleration is capped by the extruding and X/Y machine limits;
  square corner velocity by X/Y jerk limits. Zero defaults leave firmware state alone. The U1
  sets `default_jerk=0`, so it emits **no SCV commands**. `ACCEL_TO_DECEL` is conditional on the
  profile flag and percentage factor. The pinned writer does not emit `MINIMUM_CRUISE_RATIO`.
- `enable_pressure_advance[0]` emits `SET_PRESSURE_ADVANCE ADVANCE=0.02` for the oracle filament,
  after the active filament start script and before T0 selection. Disabled PA emits nothing.
- `exclude_object` controls DEFINE and each layer's START/END. Prusa's `gcode_label_objects`
  default cannot accidentally enable Marlin labels on Klipper. No Marlin envelope is emitted.
- Orca preamble order is G90, G21, M83; Klipper does not auto-insert temperature commands around
  the machine start script. The profile itself contains **M204 S10000**; that custom macro call
  is preserved. All generated acceleration commands are Klipper commands.
- Part cooling uses M106 S0/S255; auxiliary cooling uses M106 P2, including Orca's truncation
  (70% becomes S178). Both are off at startup, enabled after the configured first layers, and
  shut down before the end script. Cooling still uses its existing approximation; final time/progress use the bounded G-code planner.
- Orca's `travel_speed_z=0` means use travel speed, not F0. Its fixed-height final layer is kept
  when the midpoint is below the mesh: this cube ends at **Z20.05**, not a shortened Z20.00 layer.
  This required one settings flag and one resolver assignment; Prusa scheduling stays unchanged.

For U1 the header is:

```gcode
; HEADER_BLOCK_START
; generated by Mica Slicer 0.0.1 (OrcaSlicer 2.4.2 on 2026-10-20 at 12:00:00)
```

The U1 profile defaults `mica_gcode_identity` to `orca_compatible` for its older Moonraker
parser. Other printers default to Mica's ordinary timestamped identity; the CLI can override
it with `--gcode-identity mica|orca-compatible`. See [configuration and identity](gcode-config-identity.md)
for timestamp injection, full settings footers, coverage and hashes.
Orca header/executable block markers, layer count, filament density/diameter, used tool and maximum
Z are emitted. Thumbnails (48×48 and 300×300 PNG) are rendered from the placed mesh and inserted after
`HEADER_BLOCK_END`, using Orca's thumbnail block wrappers; see [thumbnails.md](thumbnails.md). The installed Orca CLI also produced no thumbnail blocks for this STL.

## Template inventory

The call sites were checked in `orca@8500fcdcca src/libslic3r/GCode.cpp`: `_do_export`,
`process_layer`, `set_extruder`, and `PlaceholderParserIntegration`. The unchanged `mica-template`
language handles the U1 templates, including local variables, conditionals and string arithmetic;
no language workaround was needed. The source option schema now supplies numeric, boolean,
nullable and string-vector types for Orca extras. Unknown options remain strings. Typed values
win over the raw extras retained for round trips.

| Call | Variables added or refreshed |
|---|---|
| All six, shared print context | `initial_tool`, `initial_extruder`, `current_extruder`, `first_tools`, `first_filaments`, `first_non_support_tools`, `first_non_support_filaments`, `initial_no_support_tool`, `initial_no_support_extruder`, `initial_no_support_hotend`, `current_hotend`, `num_extruders`, `is_extruder_used`, `total_layer_count`, `current_object_idx`, `has_wipe_tower`, `has_single_extruder_multi_material_priming`, `total_toolchanges` |
| Shared geometry | `print_bed_min/max/size`, `first_layer_print_min/max/size`, `first_layer_print_convex_hull`, `first_layer_center_no_wipe_tower`, `adaptive_bed_mesh_min/max`, `bed_mesh_probe_count`, `bed_mesh_algo`, `max_print_z`, `max_print_height`, `in_head_wrap_detect_zone`, `hold_chamber_temp_for_flat_print` |
| Shared temperatures/material | `temperature`, `first_layer_temperature`, `first_layer_bed_temperature`, `bed_temperature_initial_layer`, `bed_temperature`, `bed_temperature_initial_layer_single`, `bed_temperature_initial_layer_vector`, `bbl_bed_temperature_gcode`, `overall_chamber_temperature`, `min_vitrification_temperature`, `enable_high_low_temp_mix`, `max_additional_fan`, `flush_volumetric_speeds`, `flush_temperatures`, `during_print_exhaust_fan_speed_num`, `outer_wall_volumetric_speed`, `is_all_bbl_filament`, `has_tpu_in_first_layer` |
| Shared metadata | `model_name`, `plate_number` (one plate), `plate_name` (empty), `first_layer_height`, `print_time_sec`, `used_filament_length` |
| Each template evaluation | `position`, `e_retracted`, `e_restart_extra`, `zhop`, `extruded_volume/weight`, `extruded_volume_total/weight_total`; writable outputs validated and applied to the T0 writer |
| `machine_start_gcode` | Shared context; source `nozzle_temperature[_initial_layer]`, `curr_bed_type`, chamber/filament options are typed config values, not guessed runtime strings |
| `filament_start_gcode` (T0) | `filament_extruder_id=0`, `layer_num=-1`, `layer_z=0`, `max_layer_z=0`, scalar `retraction_distance_when_cut/ec`, `long_retraction_when_cut/ec` aliases |
| `before_layer_change_gcode` | `layer_num` zero-based, `layer_z`, `max_layer_z`; runs on the first layer too |
| `layer_change_gcode` | `layer_num` zero-based after change, `layer_z`, `most_used_physical_extruder_id=0`; current maximum Z also available |
| `filament_end_gcode` | Each physical slot in order, `filament_extruder_id`, correct implicit vector index, last zero-based `layer_num`, `layer_z`, `max_layer_z` |
| `machine_end_gcode` | Same final-layer values; implicit current tool resets to T0 |

`chamber_cooling_mode` belongs to Snapmaker's fork, not this Orca U1 profile: none of the selected
six templates references it, so it is not synthesized. `chamber_temperature`,
`chamber_minimal_temperature`, `filament_cooling_before_tower`, retraction vectors,
`first_x_layer_fan_speed`, and the additional-fan layer options are already typed source options.
Absolute E output is explicitly rejected for Klipper, so its conditional `e_position` output is
not needed. Calibration-only `scan_first_layer` and Bambu printer paths are outside this milestone.

The adaptive footprint uses the prepared support, brim and skirt hull, retaining Orca 2.4.1's
skirt-distance padding in the zero-loop auto-brim case. The default cube probes min=123.5,124 and
max=147.5,148 with bicubic 4×4. Ear and tall-thin auto-brim bounds match the live oracle too;
see [skirt/brim scope and comparisons](skirt-brim.md). Tower jobs add the tower's brim/rib footprint.
Multi-object jobs now use the shared plate adhesion hull; [plate-level.md](plate-level.md) records
the passing saved four-tool mesh comparison and the incomplete two-tool live oracle.
Inline time placeholders use the postprocessed acceleration-aware estimate; filament placeholders use
the writer's extrusion total. Firmware macro internals and preheating durations remain opaque. M73
progress uses the final estimate and respects `disable_m73`.

## Oracle and regression tests

The [Orca footer statistics contract](orca-footer.md) records per-slot filament weight/cost,
the Moonraker slot-use fix, saved-Orca footer comparison and updated default U1 hashes.

The [wipe/bed follow-up](u1-wipe-bed.md) records the retraction split, layer-2 M140,
fresh native 2.4.1 comparisons and unchanged Prusa hashes.

Later U1 role-parity work added the vendor-scoped input preparer
(`scripts/prepare-u1-oracle.py`) and a saved-reference test mode.

The required run used `/Applications/OrcaSlicer.app/Contents/MacOS/OrcaSlicer --help` (2.4.1),
installed Snapmaker JSON inherited recursively into three temporary files, four copies of the same
PLA filament, `curr_bed_type=High Temp Plate`, and:

```sh
OrcaSlicer --datadir "$ref/data" \
  --load-settings "$ref/machine.json;$ref/process.json" \
  --load-filaments "$ref/filament.json;$ref/filament.json;$ref/filament.json;$ref/filament.json" \
  --slice 0 --arrange 1 --outputdir "$ref" --export-settings "$ref/resolved.json" \
  /absolute/path/to/tests/models/cube20.stl
```

Use an absolute model path and run in `$ref` (a system temporary directory): Orca can also create
`00000.log` in its working directory. The initial reference remains in the system temp directory;
only its custom startup/preamble and custom ending are stored in
`crates/mica-profiles/tests/fixtures/u1_cube20_orcaslicer.gcode`.

`u1_custom_gcode_matches_orcaslicer` compares that fixture against the absolute pinned profiles,
skipping when unavailable. `u1_live_orcaslicer_start_end_and_layer_structure` reproduces the CLI run
from installed profiles, skipping when the app/profiles or expected 2.4.1 version is absent. It
compares **everything before the first layer**, excluding blank lines, M73 progress and the deliberate
generator attribution difference and thumbnail blocks (Orca's STL CLI has no renderer), plus the end block and every layer macro/object-label sequence.
Layer heights are compared numerically because Orca float32 comments contain values such as
0.199999; its extra thick-infill height comments describe geometry rather than layer boundaries.
Other tests cover label/PA disabling, typed template arithmetic, compatibility, nonrectangular
object polygons and Klipper limits/SCV commands.

The selected machine, machine base, process and PLA/base preset JSONs are identical between the
installed 2.4.1 resources and the 2.4.2 pin. No start/end template difference was observed. The pin's
X/Y acceleration cap is implemented; the chosen 25,000 mm/s² limits do not constrain this reference.
The surprising behaviors were the plate's zero temperatures, first height 0.25 despite the 0.20
preset name, Z20.05 final layer, first-layer timelapse/detection macros, adaptive mesh padding even
with zero skirts, and all four heads' start calibration macros in a T0-only job.

Open work: real printer validation of the header/macros; exact
Orca walls/infill, support footprints, wipe/spiral Z-hop and role cooling. Multi-tool/preheat scope is documented in [multitool.md](multitool.md), and prime towers in [wipe-tower.md](wipe-tower.md). The exporter retains Mica's toolpath planner; final time and M73 use the
Prusa timing planner with Orca Klipper limit handling. The timing oracle on Orca's own U1 cube G-code
is 911.694 seconds versus Orca 2.4.1's 908 seconds. After the role-parity changes,
Mica's own cube is 15m 18s versus Orca's 15m 8s.
See `crates/mica-gcode/tests/fixtures/README.md` for the timing API, scope and other oracle results. Raw custom
G-code moves are measured by the postprocessor but are not parsed back into the generator's motion state; templates can explicitly update writable state.
Point-group options and all source-specific percentage fallback semantics are not a complete Orca
PlaceholderParser port. No required U1 template uses those unsupported forms.
