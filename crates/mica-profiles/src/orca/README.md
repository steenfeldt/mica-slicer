# Orca profile importer

The public entry points are `Bundle::load_vendor(index, dialect)`,
`Bundle::add_vendor(index)`, `Bundle::load_user_presets(directory)`,
`Bundle::preset`, `Bundle::preset_names`, `Bundle::flatten`,
`Bundle::full_config`, `Bundle::full_config_with_project`,
`Bundle::single_tool_config`, the compatibility query methods, and
`map_config(&raw, dialect)`. `Dialect` selects pinned Orca 2.4.2 or Snapmaker
Orca 2.4.0 defaults. `Kind` is `Machine`, `Process`, or `Filament`.
`RawConfig` is a deterministic map of option names to JSON strings/arrays.
Resolved scalar string options contain decoded text; string vectors retain
their individual elements. `option_text` exposes Orca's text serialization.

```rust
use mica_profiles::orca::{Bundle, Dialect, map_config};

# fn example() -> Result<(), Box<dyn std::error::Error>> {
let root = std::path::Path::new("/path/to/resources/profiles");
let mut bundle = Bundle::load_vendor(
    root.join("OrcaFilamentLibrary.json"), Dialect::Orca242)?;
bundle.add_vendor(root.join("Snapmaker.json"))?;
// Optional: bundle.load_user_presets("/path/to/app/user/default")?;
let raw = bundle.full_config(
    "Snapmaker U1 (0.4 nozzle)",
    "0.20 Standard @Snapmaker U1 (0.4 nozzle)",
    &["Generic PLA @System"; 4],
)?;
let imported = map_config(&raw, Dialect::Orca242);
assert_eq!(imported.config.string("gcode_flavor"), Some("klipper"));
// Inspect every report entry before using unsupported features for a print.
# Ok(())
# }
```

For Snapmaker 2.4.0 use its `Snapmaker.json`, `Dialect::Snapmaker240`,
`0.20mm Standard @Snapmaker U1 (0.4 nozzle)`, and `Generic PLA`.
The application/project bed selection is a separate state layer; pass
`curr_bed_type = "Textured PEI Plate"` through `full_config_with_project`
when selecting that plate. Plain composition uses Orca's Cool Plate default.

## Source behavior

These files are adapted from AGPL-3.0 upstream sources, not runtime dependencies:

- `orca@8500fcdcca src/libslic3r/PresetBundle.cpp`:
  `load_vendor_configs_from_json` and `full_fff_config`.
- `orca@8500fcdcca src/libslic3r/Preset.cpp`: user loading, normalization,
  `s_Preset_*_options`; `Config.hpp`: vector resize uses the first element.
- `orca@8500fcdcca src/libslic3r/Config.cpp`: `ConfigBase::load_from_json`.
- `orca@8500fcdcca src/libslic3r/PrintConfig.cpp`: option definitions,
  enum vocabularies, `handle_legacy` and `handle_legacy_composite`.
- `snapmaker-orca@b1831e5dcb` at the same paths: older defaults, vector process
  options and filament `setting_id` fallback. Standard/high-flow selection is
  read from `nozzle_volume_type` and `process_flow_support`.
- `prusaslicer@b028299c77 src/libslic3r/PrintConfig.cpp`: canonical definitions.

`orca_schema.json` and `snapmaker-orca_schema.json` are **AGPL-3.0 data**
extracted from `PrintConfig.cpp`, `PrintConfig.hpp`, and `Preset.cpp` at those
pins. Regenerate with the standard-library-only Python script:

```sh
python3 crates/mica-profiles/tools/generate_orca_schema.py /path/to/upstream
```

The extractor handles literal/default constructors, enum maps, nullable
filament overrides, axis defaults and preset scopes. Unparsed expressions fail
the generation. It does not derive defaults from the reference project.

Vendor indexes determine which files load. Inheritance is a single exact
preset name, resolved lazily with cycle/missing-parent errors, independent of
index ordering. Abstract presets (`instantiation = "false"`) are available as
parents but excluded from selection. Abstract names are scoped to their vendor,
so Snapmaker and OrcaFilamentLibrary can both define `fdm_filament_common`.
The most recently added vendor is preferred for unqualified public lookup;
user presets take precedence. Exact duplicate index entries are harmless;
conflicting duplicates within a vendor fail.

`from`, version, instantiation, name, description, setting id and inheritance
remain accessible through `Preset` metadata. Directory origin determines
system/user status, not an untrusted `from` value. System ancestry is flattened;
user ancestry is retained in `inherits_group`. Filament ids inherit separately.
User loading is transactional, imports only JSON files, and never rewrites or
deletes the source directory. It supports user-to-user inheritance too.

Composition checks explicit compatibility lists and printer/process expressions before merging.
Lists win over conditions; a parse error remains permissive as in upstream. User machines
also match their immediate parent. `compatible_presets` exposes the same checks to the CLI.

Composition follows upstream precedence: full defaults → process → default
filament → machine → project state → selected filaments. Machine extruder
vectors resize by repeating the first value. Filament scalar options come from
the first selection; filament vectors take one entry per tool, as normalized
single-filament presets do upstream. Compatibility expressions and user
inheritance are gathered into group vectors. Support tool selectors are clamped.
Known keys are parsed with the upstream JSON conventions: semicolon-separated,
quoted string arrays; comma-separated numeric/point arrays; `#` point groups;
C-style scalar strings; nullable vector entries. Integers keep the leading digits of a
fractional value as Orca's `istream >> int` does ("59.8" is 59). Invalid options are preserved
for the mapping report rather than silently replaced with defaults. Filament options the
dialect's schema does not define compose to one element per tool (empty when a filament lacks
them) instead of failing; Orca drops them, so they never reach its G-code.

Legacy aliases, wall/infill order, hybrid support, thumbnail format, and the
legacy wipe matrix composite migration are implemented in `legacy.rs`. The
original per-file values remain in `Preset::values`, and renamed option keys
also remain in the raw config as extras. Metadata and compatibility keys are
intentionally separated during full composition, as described above.

## Mapping and diagnostics

See [MAPPING.md](MAPPING.md) and the executable [mapping.tsv](mapping.tsv).
There are 279 reviewed source-key rules: 211 identical-meaning rules and 68
conversion/selection rules. The remainder of the 674-key Orca schema is
explicitly listed as extras-only. Each input key gets exactly one `MappingEntry`
with `Mapped`, `Converted`, `ExtrasOnly`, or `Rejected`. Counts depend on actual
values, vector shape and active bed. Invalid numbers, booleans, enum names,
array shapes and non-finite results never silently become engine values.

The recorded pre-batch-A U1 Orca composition had 669 keys: 192 mapped, 43 converted, 434 extras-only,
0 rejected. the Snapmaker baseline had 596: 174 mapped, 62 converted, 360 extras-only,
0 rejected. The raw JSON map is always returned unchanged as `MappingResult::orca`, and
every key is also in `MappingResult::extras` in Orca option-text form.

Widths expressed in percent resolve against the nozzle diameter, since Prusa
uses layer height for percent widths. Small-perimeter speed resolves against
outer-wall speed; bridge/infill acceleration resolves against Orca's base.
Shrinkage converts retained-size percent to shrink percent. Active plate
selection chooses the appropriate bed-temperature vectors. Explicit enum
translations include `back → rear`, `tri-hexagon → stars`,
`monotonicline → monotoniclines`, and print sequence → `complete_objects`.
G-code flavors are checked explicitly, even where both pins use the same name.
Unsupported patterns/modes remain extras-only.

Mica's schema does not currently contain the nullable `filament_retract_*`
options, so non-nil filament overrides are applied to their corresponding
per-tool canonical printer vectors, after the printer mapping. Nil preserves
the printer value. Source filament vectors remain available in the raw map.

## Deliberate boundaries and review points

- Known Orca-only options now retain both their raw extras and validated typed values in `Config`.
  `templates::config_context` gives typed values precedence; arithmetic and indexed temperature,
  boolean, percent, nullable and string vectors therefore work without guessing types from text.
  Scalar percentages with a declared ratio resolve against Orca's source keys. Unknown options
  remain strings. Canonical-name collisions use the mapped canonical value and retain original
  text in `MappingResult::orca` and `MappingResult::extras`; runtime aliases override where needed.
  Unresolvable percentage dependencies and point-group options are not a complete Orca parser port.
- This importer preserves unknown/misplaced/obsolete values instead of Orca's
  removal/default-substitution behavior. Review `ExtrasOnly`/`Rejected` before
  executing templates or slicing; a retained option does not imply engine
  support for the corresponding feature.
- Snapmaker multi-flow filaments compose to each tool's flow variant (project
  `filament_volume_type`, default `standard`; see
  [the Snapmaker profile format](../../../../docs/snapmaker-profile-format.md)).
  `discovery::load_installed` selects `Snapmaker240` for profile sets that use flow variants.
  Multiple Bambu `filament_extruder_variant` entries are rejected. Snapmaker process vectors can map using a uniform declared nozzle flow;
  mixed nozzle-flow selections cannot reduce to Mica scalar process values.
- Read with OrcaSlicer's schema (`Orca242`), a Snapmaker Orca project or preset stores its
  46 flow-variant process options (`process_flow_variant_options`: speeds, accelerations,
  jerks, `enable_overhang_speed`, `slow_down_layers`, …) as lists where OrcaSlicer has single
  values. `flow_variant::resolved` reads each list as the element Snapmaker Orca's
  `get_config_idx` gives the filaments (their `filament_volume_type` in
  `process_flow_support`, else the first): a uniform list is its value; filaments of
  different flow types would print with different values per tool, which Mica cannot hold,
  so the standard (first) value is used with a note. OrcaSlicer itself reads the first value
  of such a list (`ConfigOptionBool::deserialize`, `istream` for numbers), and so does the
  value parser for any other list given to a single-value option.
  Bambu extruder/AMS variant remapping and Snapmaker mixed-filament grouping
  are not implemented. Original data remains in the loaded presets.
- Project UI state (colors, plate positions, purge-volume calculation, AMS
  assignments) is caller-supplied. Dirty-setting lists, physical/cloud printer
  persistence, machine-model installation
  filtering, renamed-preset lookup and every historical legacy migration are
  not replicated. Explicit missing parents fail instead of selecting another
  preset. SLA is outside this FFF importer.
- Generated defaults and enum mappings deserve close review. Shared option
  units do not imply that Orca and Prusa path-planning algorithms are identical.
  Auto brim, aligned-back seams, extra infill patterns, differing support
  algorithms and Orca-only cooling/pressure modes are not approximated.

See the [fixture README](../../tests/fixtures/u1/README.md) for the exhaustive
stock-preset 2.4.0 project comparison and the report of 2.4.2 versus 2.4.0 differences.

## CLI and Klipper output

See [U1 output and oracle reproduction](../../../../docs/u1-klipper.md). `--vendor Snapmaker`
selects Orca format; `--profiles-dir` overrides installed-resource discovery. No profiles are copied
into Mica. `single_tool_config` repeats the selected filament across physical slots and selects
`curr_bed_type` from the explicit plate or machine default before mapping.

[Batch A](../../../../docs/orca-options-a.md) activates six additional source keys and separates contour compensation from Prusa XY size compensation; the composition counts above describe the earlier baseline.
