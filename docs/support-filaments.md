# Supports in another filament

Supports and their interface can print with other tools than the model:
`support_material_extruder` / `support_material_interface_extruder` (Orca `support_filament` /
`support_interface_filament`, also per object), 0 = "Default" (the layer's tool). First measured end
to end on 2026-10-19 against PrusaSlicer 2.9.6 (XL 5T) and OrcaSlicer 2.4.1 (U1), with the corpus
figurine (`tests/models/support_organic.py`, `support_figurine.stl`).

## What Mica now does like the reference slicers

| Behaviour | Upstream | Before |
|---|---|---|
| A soluble interface over a non-soluble (or Default) base with a zero top gap gets base interface layers: the dense layers furthest from the contact print in the base material, interface pattern. Orca does the same for any other interface filament (one layer). | `SupportParameters::SupportParameters`, `SupportCommon.cpp :: generate_interface_layers`, `generate_support_toolpaths` (both) | Organic: the per-tool configuration saw only its own `filament_soluble`, so the interface never read as soluble. Normal supports had no base interface layers at all. |
| Base and interface regions merge only when one tool prints both, or one is Default and the object prints with exactly the other (upstream compares the zero-based object extruder with the one-based tool; kept). | `SupportParameters::can_merge_support_regions`, `PrintObject::object_extruders` | Any Default tool merged. |
| With a wipe tower, PrusaSlicer's first layer starts with a soluble tool. | `ToolOrdering::reorder_extruders` | Sorted order. |
| Orca, Default base under an explicit interface: the layer's first non-soluble tool other than the interface's. Otherwise the layer's first tool, moving on to a non-soluble, then a non-support filament (`filament_is_support`). | `GCode::process_layer` | The layer's first tool (the interface filament on its own layers). |
| Orca, support-only layers (no object extrusions) with a Default base and an explicit interface: the base prints with the used filament cheapest to flush to from the interface's (`flush_volumes_matrix`), not soluble; **Avoid interface filament for base** (`support_interface_not_for_body`, default on, in the Support topic under Orca rules) excludes the interface filament itself. | `ToolOrdering::collect_extruders` | The interface filament. |
| Orca: a prime tower that stays on (several used filaments) turns `independent_support_layer_height` off, so the supports print on the object layers; a zero top gap alone no longer synchronizes Orca's layers. | `DynamicPrintConfig::normalize_fdm_2`, `Print::extruders`, `raft_and_intermediate_support_layers` | 94 support-only layers, each with its own tower layer (3.2 m of tower instead of 1.1 m). |
| PrusaSlicer refuses a wipe tower with normal (not organic) supports when soluble supports (zero gap) are not synchronized, or when non-soluble supports need a tool change. Mica now stops with the same reasons. | `Print::validate` | Sliced. |

## Measurements

XL: `0.20mm SPEED @XLIS 0.4`, Prusament PLA @XLIS on every tool, T2 with PETG temperatures
(240/240 °C) and `filament_soluble = 0,1,0,0,0`, interface on T2, contact distance 0, wipe tower,
`--center 180,180` (both slicers; PrusaSlicer also `--load` a 5×5 `wiping_volumes_matrix`). Cases:
A organic, base Default; B organic, base T2; C organic, base T1 (same as A); E snug, base Default,
synchronized; F grid, base T2, synchronized; G grid, base T1, synchronized.

U1: `0.20 Standard`, slot 1 Snapmaker PLA @U1, slot 2 Snapmaker Breakaway Support For PLA @U1 (its
flattened filament preset for Orca, the differing values as per-slot `--set` for Mica), interface on
slot 2, base Default, prime tower, explicit 4×4 flush matrix, figurine at 135,135. UA organic, zero
top gap; UB organic, base slot 2; UC normal(auto), zero top gap; UD normal(auto), default gaps;
UT / UTn like UD without the prime tower, `support_interface_not_for_body` 1 / 0.

"Seq" compares the extruding tools of every layer, "iface Z" the Z of every interface layer.
Base/tower columns are tool 1 / tool 2 (mm), M109 counts tool heat-up waits.

| Case | Run | Seq / iface Z = ref | T cmds | Base mm | Interface mm | Tower mm | Interface layers | Support-only layers | M109 | Time |
|---|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| A | PrusaSlicer |  | 15 | 768/0 | 47.3 | 1916/55 | 9 | 0 | 20 | 56m 58s |
| A | Mica before | no / no | 17 | 723/0 | 70.4 | 1927/66 | 11 | 0 | 24 | 57m 54s |
| A | Mica after | yes / yes | 15 | 747/0 | 47.2 | 1916/55 | 9 | 0 | 20 | 57m 40s |
| B | PrusaSlicer |  | 176 | 0/745 | 69.9 | 1868/953 | 11 | 0 | 342 | 1h 10m 16s |
| B | Mica before | no / yes | 175 | 0/723 | 70.4 | 1853/951 | 11 | 0 | 340 | 1h 9m 35s |
| B | Mica after | yes / yes | 176 | 0/723 | 70.4 | 1867/953 | 11 | 0 | 342 | 1h 9m 39s |
| E | PrusaSlicer |  | 15 | 1272/0 | 46.8 | 1916/55 | 10 | 0 | 20 | 54m 7s |
| E | Mica before | no / no | 21 | 1245/0 | 73.3 | 1916/91 | 13 | 0 | 32 | 55m 49s |
| E | Mica after | no / no | 16 | 1268/0 | 49.8 | 1909/59 | 11 | 0 | 22 | 55m 18s |
| F | PrusaSlicer |  | 206 | 0/1389 | 103.8 | 2200/1121 | 15 | 0 | 402 | 1h 9m 27s |
| F | Mica before | no / no | 207 | 0/1384 | 110.2 | 2207/1130 | 16 | 0 | 404 | 1h 9m 58s |
| F | Mica after | yes / no | 206 | 0/1384 | 110.2 | 2200/1121 | 16 | 0 | 402 | 1h 9m 41s |
| G | PrusaSlicer |  | 17 | 1421/0 | 74.3 | 2273/66 | 12 | 0 | 24 | 55m 53s |
| G | Mica before | no / no | 25 | 1384/0 | 110.2 | 2272/113 | 16 | 0 | 40 | 58m 14s |
| G | Mica after | no / no | 18 | 1416/0 | 78.1 | 2266/70 | 13 | 0 | 26 | 57m 28s |
| UA | OrcaSlicer |  | 11 | 748/0 | 18.4 | 1151/43 | 6 | 0 | 16 | 51m 7s |
| UA | Mica before | yes / yes | 11 | 729/8 | 17.6 | 1151/43 | 6 | 0 | 16 | 45m 50s |
| UA | Mica after | yes / yes | 11 | 740/0 | 17.6 | 1151/43 | 6 | 0 | 16 | 45m 53s |
| UB | OrcaSlicer |  | 173 | 0/733 | 34.6 | 922/976 | 8 | 0 | 340 | 1h 9m 34s |
| UB | Mica before/after | yes / yes | 173 | 0/725 | 33.7 | 922/975 | 8 | 0 | 340 | 1h 10m 37s |
| UC | OrcaSlicer |  | 23 | 1488/0 | 74.2 | 1132/108 | 20 | 0 | 40 | 50m 24s |
| UC | Mica before | no / no | 23 | 1458/70 | 92.5 | 1139/108 | 19 | 0 | 40 | 46m 8s |
| UC | Mica after | no / no | 15 | 1563/0 | 57.8 | 1140/65 | 10 | 0 | 24 | 44m 42s |
| UD | OrcaSlicer |  | 33 | 1450/0 | 105.7 | 1122/163 | 27 | 0 | 60 | 51m 42s |
| UD | Mica before | no / no | 29 | 1310/104 | 127.1 | 3215/215 | 19 | 94 | 52 | 1h 3m 36s |
| UD | Mica after | no / no | 23 | 1517/0 | 92.5 | 1132/108 | 19 | 0 | 40 | 46m 12s |
| UT | OrcaSlicer |  | 39 | 1345/11 | 139.9 | – | 27 | 88 | 72 | 40m 19s |
| UT | Mica before | no / no | 29 | 1310/104 | 127.1 | – | 19 | 94 | 52 | 51m 30s |
| UT | Mica after | no / no | 29 | 1414/0 | 127.1 | – | 19 | 94 | 52 | 51m 32s |
| UTn | OrcaSlicer |  | 39 | 1295/61 | 139.9 | – | 27 | 88 | 72 | 40m 28s |
| UTn | Mica after | no / no | 29 | 1363/52 | 127.1 | – | 19 | 94 | 52 | 51m 32s |

Temperatures at the tool changes (M104/M104.1/M109 per tool and value) match the references in A
and C. B and F differ only in two to five first-layer preheats (Mica 230 °C, the first-layer
temperature; the known XL preheat difference in [multitool.md](multitool.md)); E and G in the extra
first-layer tool change below. Fan commands follow the layer-time cooling differences (U1: see
below); UA/UB/UC/UD M109 counts follow the tool changes.
Mica's estimator gives 51m 08s for Orca's own UA file (Orca: 51m 07s) and 40m 20s for UT, so the U1
time differences are in the G-code, not the estimate (see open items).

## Remaining differences (not support-tool logic)

- **PrusaSlicer normal supports with a zero top gap** (E, G and single-tool G0): Mica prints a
  ~10 mm patch of support interface on the first layer around the column; PrusaSlicer does not.
  With a Default base and a soluble interface this adds one tool change and one interface layer.
- **Orca normal supports** (UD0/UC0i with one filament show the same): fewer interface layers on
  the hanging sphere's bottom contacts (19 vs 27), hence fewer tool changes. With a zero top gap and
  one filament Orca keeps independent support layer heights; Mica now does too (single-tool
  zero-gap U1 normal supports changed accordingly), but its intermediate layers still differ.
- **Orca zero top gap**: Orca prints the first layer over the supports as Bridge / Overhang wall,
  Mica as solid infill and walls (also with one filament).
- **U1 layer cooling with several filaments**: with the prime tower Orca slows the model more
  (wall feeds 2200–2500 vs Mica's 3000–3600 mm/min, 51 vs 46 min), without it less (40 vs 52 min).
  The same with PLA in both slots, so it is the multi-tool cooling time accounting, not the
  support filament.
- **Orca dont-care-only first layers** (raft base with a Default base and another interface
  filament): Orca's flush-volume reordering picks the body filament, Mica (like PrusaSlicer) the
  first tool that prints. Mica does not port `reorder_extruders_for_minimum_flush_volume`.
- Orca's support-only layer rule reads the top-level support roles; nested collections hide the
  base there, so Orca occasionally prints one such layer's base with the interface filament (UT:
  11 mm). Mica does not copy that.

## Corpus

`tests/corpus/manifest.json`: `support_soluble_interface` (A), `support_soluble_all` (B) and
`support_soluble_grid` (F) on the XL, `support_orca_interface` (UA) and `support_orca_normal` (UD)
on the U1. Their references (`--record-reference`) also store `tool_roles` (filament per tool and
role, from `mica validate --json`) and `toolchanges`; `run.py` gates both. Documented exceptions per
case: `known_role_gaps` (UA: Bridge, Overhang wall), `role_tolerance` (UD: 35 %, the tower on T2
follows the tool changes) and `toolchange_tolerance` (UD: 12).

## GUI guard rails

`crates/mica-gui/src/checks.rs` (plate settings and objects with their own support settings):
a support tool the printer does not have (fix: Default) or without a filament; a soluble interface
with a gap to the model (fix: 0 mm, plus synchronized layers for PrusaSlicer's wipe tower); a
soluble support tool that also prints the model; PrusaSlicer's two wipe tower rules (fixes:
Synchronize, Use Default). An "+ Add → Soluble supports here" intent was not added: support tools
are object settings upstream, so an enforcer part cannot carry them, and the intent would have to
pick a soluble tool and edit the object.

## Reproducing

Reference slicers run as installed (`out/ref/PrusaSlicer-2.9.6.app`, `/Applications/OrcaSlicer.app`).
XL: `PrusaSlicer --datadir out/ref/datadir --printer-profile 'Original Prusa XL - 5T Input Shaper 0.4
nozzle' --print-profile '0.20mm SPEED @XLIS 0.4' --material-profile 'Prusament PLA @XLIS' --load
matrix.ini --load case.ini --center 180,180 --no-binary-gcode --export-gcode -o ref.gcode
support_figurine.stl`, the case INI holding the settings above; Mica gets the same keys as `--set`.
U1: flatten the four filament presets like `scripts/prepare-wipe-tower-oracles.py` (vendor-local
inheritance, four-slot schema defaults), then `OrcaSlicer --datadir … --load-settings
"machine.json;process.json" --load-filaments "f1.json;f2.json;f3.json;f4.json" --slice 0 --arrange 0`
on a one-object 3MF at 135,135. Compare with `python3 scripts/compare-support-tools.py ref.gcode
mica.gcode`.
