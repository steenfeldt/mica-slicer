# Roadmap

This page says what works today, what comes next and what is not supported yet. It promises
no dates.

## Status: Alpha (pre-release)

Mica is alpha (pre-release) software. It is tested against PrusaSlicer and OrcaSlicer and on
real printers, but it can still produce G-code that fails a print or damages a printer:

- Check the preview before printing, and watch the first layers.
- Never leave a printer unattended.
- Mica is provided as is, without warranty of any kind. See the [license](LICENSE)
  (GNU AGPL-3.0, sections 15 and 16: disclaimer of warranty and limitation of liability).

## What works today

**Printers.** Output is checked against the reference slicers for these printers:

| Printer | Firmware | Sliced like |
|---|---|---|
| Prusa CORE One, XL (5 tools), MK3.5, MINI | Prusa (Marlin 2), binary G-code where the printer uses it | PrusaSlicer 2.9.6 |
| Snapmaker U1 (4 tools) | Klipper | OrcaSlicer (Snapmaker's fork) |
| RatRig V-Core 3.2 | Klipper | OrcaSlicer |

Other printers from the PrusaSlicer and OrcaSlicer profile libraries load and slice, but have
not been checked print by print.

**Slicing rules.** Each printer follows one slicer's rules: PrusaSlicer's for Prusa printers,
OrcaSlicer's for the Orca-profiled ones. One setting picks them
([`docs/slicing-rules.md`](docs/slicing-rules.md)); the firmware dialect is a separate setting.

**Main features.**

- STL and 3MF input, including PrusaSlicer and OrcaSlicer projects with their plates, settings,
  modifiers, height ranges and painting; 3MF projects are saved in either slicer's layout.
- Classic and Arachne walls, seams (including scarf seams), all PrusaSlicer infill patterns plus
  OrcaSlicer's crosshatch, ironing, fuzzy skin, bridges, overhang control, arc fitting.
- Normal, tree and organic supports, enforcers and blockers, support painting, supports in
  another filament.
- Multi-tool printing: tool per object or part, color painting, tool changes with preheat,
  wipe and prime towers, mixed colors (PrusaSlicer's ColorMix).
- Skirt, brim and brim ears, variable layer height, per-layer color changes, pauses and custom
  G-code, object labels for canceling single objects, sequential printing (one object after
  the other, with clearance checks), spiral vase (one wall rising continuously, also per plate
  on Orca printers).
- Thumbnails, time and filament estimates, Prusa binary G-code, a G-code validator.
- Calibration tests (flow, pressure advance, temperature tower, retraction, maximum volumetric
  speed).
- The desktop app: plates, arrange, auto-orient, cut, measure, section view, painting tools,
  G-code preview, surface textures, a model library, sending prints to PrusaLink, Prusa Connect
  and Moonraker printers, filament and color sync from the loaded spools, printer cameras, an
  in-app updater.
- Mica MCP: a local server that lets AI assistants paint and slice models
  ([`docs/mcp.md`](docs/mcp.md)).

The command line (`mica slice`) and the app produce the same G-code byte for byte.

## What comes next

- Closing the remaining differences to the reference slicers, feature by feature.
- Marking printers as "tested" or "profile only" in the app, so it is clear which ones have been
  checked.
- PrusaSlicer- and OrcaSlicer-compatible post-processing scripts.
- Paint-on ironing, a named color library, and opening Snapmaker's mixed colors.
- Syncing settings between computers (printer secrets never sync).

## Not supported yet

- **Absolute extrusion (M82).** Turn on *Use relative E distances*; Mica refuses the slice
  otherwise.
- **MMU** (single-extruder multi-material, e.g. the Prusa MMU3). Printers with one nozzle per
  tool (XL, U1) are supported.
- **Post-processing scripts.** They are kept in the profile but not run; the slice says so.
- **G-code flavors** other than Marlin 2 and Klipper.
- **Sending prints to Bambu Lab printers.** Their profiles load and slice, untested.

Settings Mica does not apply yet are hidden in the app's settings editor, and a profile that
uses one gets a notice instead of a silent difference.
