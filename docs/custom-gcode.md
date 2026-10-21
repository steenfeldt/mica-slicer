# Per-layer custom G-code

Colour changes, pauses, custom G-code and template G-code at a layer, plus per-layer tool
switches on multi-tool printers. This is PrusaSlicer's `CustomGCode::Info` (Orca: the plate's
custom G-code per layer). The printer profile supplies the templates (`color_change_gcode`,
`pause_print_gcode`, Orca's `machine_pause_gcode`, `template_custom_gcode`). A plate stores the
items that place them on layers.

## Items

| Kind | 3MF `type` | What is emitted (PrusaSlicer flavour) | Orca / Klipper flavour |
|---|---|---|---|
| Colour change | 0 | `;COLOR_CHANGE,T<e>,<colour>` and `color_change_gcode`, just before the layer's first extrusion | `;CUSTOM_GCODE` and an empty line (Orca has no M600 colour change) |
| Pause | 1 | `;PAUSE_PRINT`, `M117 <message>` when there is a message, `pause_print_gcode` | `;PAUSE_PRINT`, `machine_pause_gcode`, an empty line; no message |
| Tool change | 2 | no block: from its layer up, every object prints with that tool (single-tool plates on multi-tool printers) | same |
| Template | 3 | `;CUSTOM_GCODE` and `template_custom_gcode` | same |
| Custom | 4 | `;CUSTOM_GCODE` and the item's text | same |

Which layer an item lands on follows upstream `ToolOrdering::assign_custom_gcodes`:

- **PrusaSlicer:** the lowest layer whose print_z reaches the item's Z (with half of libslic3r's
  EPSILON of tolerance). An item between two layers moves up to the next one.
- **Orca:** the nearest layer, split at the midpoints between layers.
- **Both:** at most one item per layer. Colour changes entered for a multi-material plate are
  ignored on a single-material plate and the other way round (the `mode`). A colour change
  for a tool that prints nothing from there on is dropped.

When objects print one after another (`complete_objects`, Orca's "by object"), neither slicer
applies per-layer custom G-code: `assign_custom_gcodes` runs only for layer-by-layer prints. Mica
drops the items and says so in a warning.

On a tool changer (XL), a colour change for another tool (mode `MultiExtruder`) makes that tool
print first on its layer (`extruder_needed_for_color_changer`). The change then runs right after
the tool is picked, followed by PrusaSlicer's "move to print after unknown position" (retract,
lift, travel, Z).

`M600` primes the nozzle. As in PrusaSlicer, the writer counts the pending retraction as repaid
(the upstream `writer().unretract()` whose text is discarded). The cooling slowdown adds the
template's E-only moves and drops that repaid unretract, as PrusaSlicer's `CoolingBuffer` sees
the layer's text.

The time estimate follows `GCodeProcessor`:

- A colour change for the active extruder, and every pause, synchronises the planner
  (`simulate_st_synchronize`) and records a printer stop.
- The rewrite pass emits the remaining time to the next stop: `M73 C` (normal mode) and
  `M73 D` (silent mode). This is whole minutes, then the last minute as a fraction, and an
  integer at the last stop.

`mica validate` treats `M600` as a refill: nothing is left retracted, and the prime after it is
expected.

## Projects

- **PrusaSlicer:** `Metadata/Prusa_Slicer_custom_gcode_per_print_z.xml` holds one
  `custom_gcodes_per_print_z` element per bed (`bed_idx`), with `code` items and a `mode`. Bed 0
  is always written.
- **Orca:** `Metadata/custom_gcode_per_layer.xml` holds one `plate` per plate (`plate_info id`,
  one based), with `layer` items (`top_z`) and a `mode`.
- **Both:**
  - Mica reads both files, including pre-2.2 items without a `type`.
  - Opening a project slices with the selected plate's items.
  - Saving writes the file of the chosen flavour, including the legacy `gcode` attribute from
    the printer's templates.

## Command line

```
--color-change Z[:N]    colour change (M600) at height Z mm, for tool N (default 1)
--tool-change Z:N       switch the plate to tool N from height Z (multi-tool printers)
--pause Z[:message]     pause at height Z, showing the message on the printer
--custom-gcode Z:text   insert G-code at height Z (\n separates lines)
--template-gcode Z      insert the printer's template custom G-code at height Z
```

The options add to the project's items and are saved by `--export-3mf`. `mica slice` prints
each item with its height.

## Verification (2026-10-17)

The method is the same for every printer:

1. Mica exports a 3MF with the items (`--export-3mf … --flavour prusa|orca`).
2. The reference slicer slices that same file natively. Mica slices it as well.
3. The comparison is between what the items change in each output: `diff(no items, items)` per
   slicer, compared line by line, so baseline differences between the slicers drop out.

Reference slicers: PrusaSlicer 2.9.6 (`out/ref`, `--datadir out/ref/datadir --dont-arrange
--no-binary-gcode`) and OrcaSlicer (`--slice 1`).

- **CORE One, cube20, colour change at Z=5, pause at Z=10 ("Insert magnets"):**
  - The blocks are identical:
    - `;COLOR_CHANGE,T0,#E67E22`, `M600`, `G1 E0.3 F1500 ; prime after color change` after
      `G1 Z5 F720`.
    - The layer's own unretract is gone in both.
    - `;PAUSE_PRINT`, `M117 Insert magnets`, `M601` at Z=10.
  - The countdown is identical: `M73 C3/C2/C1/C0.01`, `C3…C0`, and the same `M73 D` lines.
  - The slowdown change on the colour-change layer is the same in both: PrusaSlicer 1260→1257,
    Mica 1264→1261. The 4 mm/min offset is a baseline difference that is there without items.
  - The filament total drops by the same 0.40 mm.
- **XL 5T, two cubes on T1/T2, colour change for T2 at Z=5, pause at Z=10 (no tower):**
  - Both slicers pick T2 first on layer 5 (`; Change Tool0 -> Tool1`, `T1 S1 L0 D0`).
  - Then `;COLOR_CHANGE,T1,#E67E22`, `M600`, `G1 E-.8`, a lift, `G1 X204.368 Y189.368 F24000`,
    `G1 Z5 F720`, `G1 E.8 F1500`.
  - The countdown `M73 C0, C15 … C0` is identical, including upstream's `C0` on the first line:
    the stop comes right after a tool change, which left the planner empty.
  - Remaining differences also appear in the baseline without items:
    - The lift height (Z7.7 against Z7.6): the Z each slicer assumes after the XL tool-change
      template.
    - PrusaSlicer's `M109 … ; set temperature and wait` after a tool change.
    - The placement of `M106`, and of `M486` around the tool changes (with one extruder the
      `M486` lines now match PrusaSlicer's; see "Object labels" below).
- **XL 5T, one cube, `--tool-change 5:2`, pause at Z=10:**
  - The switch to T1 lands on the same layer.
  - The countdown `M73 C9 … C0` is identical.
  - The filament split matches: PrusaSlicer 356.99 + 967.00 mm, Mica 1323.99 mm in total.
  - Remaining differences are baseline ones: the XL multi-tool start sequence (PrusaSlicer
    `G1 E-20` from `filament_retract_length_toolchange`, Mica `E-1.2`) and the tool-change
    cooling.
- **U1, cube20, tool change to T2 at Z=5, pause at Z=10, custom G-code at Z=12:**
  - OrcaSlicer opens Mica's 3MF natively and slices with the items.
  - `;PAUSE_PRINT`, `M600` (the U1's `machine_pause_gcode`) and an empty line come after
    `;_SET_FAN_SPEED_CHANGING_LAYER` and before `EXCLUDE_OBJECT_START` in both.
  - The `;CUSTOM_GCODE` block is identical.
  - `T1` comes on the same layer.
  - OrcaSlicer re-saved the project (`--export-3mf`). Mica reads Orca's file and slices it to
    the byte-identical program.

Without items every output is byte-identical to before. The checked SHA-256 prefixes are:

| Printer | cube20 | Benchy |
|---|---|---|
| CORE One | `d4fc843c` | `0e782b80` |
| XL | `139f8059` | `4532ea45` |

These hashes are from 2026-10-17; later changes to the Prusa output (below) changed them.

## Object labels (2026-10-21)

Mica follows PrusaSlicer 2.9.6's `LabelObjects` under PrusaSlicer rules:

- With one extruder the label of an object opens in the travel to its first extrusion: after
  the first layer's lift, otherwise before the travel's second-to-last point. It stays open
  across layers. It changes only when another object prints, closes before a layer's skirt or
  brim, and closes after the last layer. A single object therefore has `M486 S0` once (cube20:
  5 `M486` lines, as PrusaSlicer, instead of 203).
- With several extruders (any tool other than the first prints) labels follow the extrusions
  as before: at the layer change and after the unretract.
- OctoPrint labels are `; printing object <name> id:<object> copy <instance>` and the matching
  stop comment, also for a single object. Orca rules keep closing the label every layer.
