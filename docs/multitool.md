# Multiple objects and tools

XL and U1 wipe/prime towers now build on this foundation. See [tower API, settings and
oracle results](wipe-tower.md). The tower-off results below document the original foundation.
`PreparedPrint` also owns one plate adhesion plan and exposes its startup hull; see
[plate adhesion, travel and combined oracle results](plate-level.md).

`mica_core::print_job::PreparedPrint` wraps independently prepared objects/parts/instances and
implements `LayerSource`. Existing `export_gcode`, `export_gcode_with_mesh` and `export_file` accept
it, including binary Prusa output. `PrintObject::prepare` takes a positioned mesh, object/instance
identity, `ToolAssignments` and a resolved profile `Config`. Tools in the Rust API and G-code are
**zero based**; imported metadata and CLI tool numbers are **one based**.

## CLI

```sh
# All printable 3MF objects/instances, with object/volume tool metadata.
mica slice project.3mf --vendor Snapmaker --filament 'Snapmaker PLA @U1' \
  --bed 'High Temp Plate' --set wipe_tower=0 -o /tmp/job.gcode

# Independent STL objects. Multiple STLs are placed in a row with 10 mm spacing,
# then the whole row is centred on the bed (or --center).
mica slice tests/models/cube20.stl tests/models/cube20.stl \
  --vendor Snapmaker --filament 'Snapmaker PLA @U1' --bed 'High Temp Plate' \
  --object-extruder 1=2 --object-extruder 2=4 --set wipe_tower=0 -o /tmp/two.gcode

# Tool 5 as the default for an XL object. Use matching XL printer/print/filament presets.
mica slice tests/models/cube20.stl --extruder 5 \
  --printer 'Original Prusa XL - 5T Input Shaper 0.4 nozzle' \
  --print '0.20mm SPEED @XLIS 0.4' --filament 'Prusament PLA @XLIS' \
  --set wipe_tower=0 -o /tmp/xl.gcode
```

`--extruder n` defaults to 1 and replaces the default tool for model roles. Explicit profile role
assignments other than 1 remain explicit. `--object-extruder i=n` is repeatable; object indices
follow input order and then 3MF build-object order, starting at 1. It replaces an object's and its
parts' `extruder` defaults. Explicit role settings in part/object metadata remain more specific.
Invalid object indices and tools outside the printer's physical tool count are errors.

All 3MF objects are selected by default; `--all-objects` is an alias and `--first-object` preserves
the old first-object selection. Stored instance transforms are retained. Explicit `--center`
translates the entire layout. Multi-plate coordinates are retained with a warning; plate selection
and local plate remapping remain outside this foundation. Nonprintable instances are skipped.

Canonical role settings work through `--set`: `perimeter_extruder` (including gap fill),
`infill_extruder`, `solid_infill_extruder` (including top), `support_material_extruder` and
`support_material_interface_extruder`. Support zero means the first/current tool in that layer's
ordered tool list. A part's `extruder` overrides the object's default for model roles. Explicit
part role settings override object role settings. For multi-tool tower jobs, `wipe_tower_extruder`
can add a structural tool. Tower flags passed to templates reflect the actual plan; single-tool
jobs have no tower even when the profile enables it.

One CLI filament preset fills every physical slot. Per-tool temperatures, filament diameters, flow,
retraction, fans and volumetric limits can be supplied as profile vectors or canonical `--set`
vectors. Orca identity aliases are kept synchronized by the CLI, and imported numeric/boolean types
are retained when overriding `preheat_time`, `preheat_steps` or other source-specific options.
Selecting a distinct named filament preset for each tool is not a separate CLI option yet.

## Scheduling and emission

A bounded prepass collects actual role/tool usage before startup templates run; it discards finished
paths and retains compact layer tool lists. Z schedules merge with Prusa’s 0.0001 mm tolerance and
midpoint print Z. Export regenerates requested batches (32 layers), grouping paths by tool and a
stable nearest-neighbour object/instance chain. Tool ordering ports Prusa's sorted/deduplicated lists, don't-care support and
last-tool-to-front rotation. Empty layers do not reset the active tool. `LayerTools::wiping` remains
reserved for wipe-into-object overrides; tower moves use the separate compact `TowerPlan`.
No whole-print path list is retained.

The existing exporter still emits paths, seams, arcs and travel. Its print adapter handles:

- Startup `is_extruder_used`, initial/current tool, total tool changes and combined first-layer bounds.
- Prusa `toolchange_gcode` / mapped Orca `change_filament_gcode`, with previous/next tool and writable
  position/retraction arrays. XL also receives the initial call with `previous_extruder=-1`.
- Per-tool temperatures, idle/standby cooldown, first-to-normal temperature transitions, filament
  start/end scripts, pressure advance, retraction state and toolchange retraction top-up.
- Tool XY offsets, filament diameter/flow, volumetric speed limits and active-tool travel settings.
- One cancellation identity per object instance, shared across its parts: Prusa M486 or Klipper
  EXCLUDE_OBJECT_DEFINE/START/END, controlled by the profile. Definitions use projected convex hulls;
  Prusa binary metadata lists the separate instances too.
- Generated-path filament totals by tool and role in comments. Tower jobs additionally report
  per-tool totals from explicit G-code, including custom priming. Opaque macros remain excluded.

The time estimator includes the U1 profile's `machine_tool_change_time` (5 s). Its disk-backed
motion timeline drives progress and a bounded preheat backtrace: XL uses 120 s / 10 steps and
M104.1 with normal/silent P/Q; Orca uses profile `preheat_time` / `preheat_steps` and M104. Homing and
previous use of the same tool stop backtracing; superseded cooldown commands are removed. The cache
is capped at 8 MiB (very large zero-time custom scripts can shorten the available lookback).
Snapmaker's explicit `delta_temperature` is supported; the current Orca U1 profile doesn't request it.
The first-layer temperature selection follows the corrected pinned Orca implementation.

## Oracle results (2026-10-16)

Reference artifacts, synthetic 3MFs, flattened profiles, logs and comparison JSON stay in a system
temporary directory, not the repository.
`python3 scripts/compare-multitool.py reference.gcode mica.gcode` compares actual extruding-tool
sequences, T commands, temperatures and model filament by role. It accepts relative-E text files.

| Case | Oracle | Result |
| --- | --- | --- |
| Three 20 mm cubes, tools 1/2/5 | PrusaSlicer 2.9.6, XL 5T Input Shaper 0.4, 0.20mm SPEED @XLIS 0.4, Prusament PLA @XLIS; tower off | All 100 layer tool sequences identical; 207 T commands in both, including the same seven startup selections; only tools 1/2/5 primed |
| Four 20 mm cubes, tools 1–4 | OrcaSlicer 2.4.1, U1 0.4, 0.20 Standard, Snapmaker PLA @U1, High Temp Plate; tower off | All 100 layer tool sequences identical; 303 T commands in both; 220°C tool waits and 70°C standby |
| Reprocess the XL oracle through Mica's timing/preheat pass | Same G-code geometry, full oracle config footer | All 465 M104.1 commands have identical tool, motion insertion boundary, P and Q. Fourteen first-layer preheats use 230°C instead of the oracle's stale 225°C; later temperatures match |
| CORE One cube20 and 3DBenchy | The build before this change | Both complete text outputs byte-identical |

For each XL tool, Mica versus oracle model filament (mm): perimeter 253.701 / 253.701,
external perimeter 264.749 / 264.750, sparse infill 491.173 / 490.646, top 28.178 / 28.178.
Solid + bridge infill is 196.196 / 210.713; the difference belongs to the existing bridge pipeline.
Each U1 tool likewise receives equal geometry; its model filament is about 1150.69 / 1226.55 mm,
reflecting the existing Mica/Orca wall, solid, bridge and infill differences.

Generated-job timing is approximately XL 45m14s / oracle 44m27s and U1 52m42s / oracle 53m56s.
Preheat positions on Mica-generated geometry therefore differ from the oracle. Reprocessing the
U1 oracle estimates 3228.73 s; its own footer says 3236 s. U1 preheat placement is not byte-identical
(182 versus 186 commands on that reference stream); short lookback intervals are sensitive to the
remaining Orca/Prusa motion-planner differences. This is not a claim of exact Orca timing parity.

The Orca CLI required consistent four-slot filament defaults/colour/mapping vectors in its temporary
settings; otherwise its flush-ordering code crashed. The four-tool native Orca metadata fixture was
also sliced with arrangement disabled. Prusa's boolean CLI form is `--no-wipe-tower`; `--wipe-tower 0`
was interpreted by this binary as an input named `0`. The private multi-tool project
(`MICA_PRIVATE_MODELS`) was not part of this oracle; see [regions.md](regions.md) for
the per-object subset test and the failed Orca reference load.

## Scope and review

This implements independent physical tools, layer-by-layer printing and model-part assignments.
Facet material painting now assigns indexed regions to physical tools; see [painting.md](painting.md).
Wipe-into-infill/object, MMU switching and sequential printing are not implemented.
The region pipeline prepares all parts of an instance together, clips overlapping solids/negatives, applies
object/part/range overrides and shares compatible walls/fills; see [regions.md](regions.md).
Path assignments carry a region index, and export resolves its speeds with the selected physical
tool's settings. Mixed nozzle diameters and soluble support interfaces still lack flow parity;
the target oracles use identical 0.4 mm nozzles. Embedded project printer/process/filament presets
remain ignored in favour of CLI profile selection.
Object order uses a deterministic greedy nearest-neighbour chain seeded by the first input;
upstream's full chain optimizer remains unported.

Review template state and retraction across switches, the source-Z merge, support don't-care routing,
and preheat barriers closely before hardware validation. Custom moves must update the template's
writable position/retraction variables; opaque firmware macros are not reverse-engineered. Existing
cooling remains the single-profile slowdown approximation, with per-tool fans and path speed limits.
The new scheduling, offsets, role assignment, retraction and preheat regressions run without vendor
profiles; profile tests and both installed reference applications were exercised separately.

Validation passed: `cargo fmt --all --check`, `cargo clippy --offline --workspace --all-targets --
-D warnings`, `cargo test --offline --workspace`, `cargo xtask provenance`, and the offline release
CLI build, including the Prusa profile tests against the pinned profiles.
