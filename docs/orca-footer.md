# Orca-rule footer statistics

The U1 footer now supplies the per-tool `; filament used [g]` vector consumed by
Moonraker's `filament_used_g` / Klipper slot-use detection. This changes only Orca-rule
statistics comments. There are no new API entry points, CLI flags or dependencies.
Prusa-rule output is unchanged, including when the firmware flavor is Klipper.

## Upstream contract and implementation

Checked the complete statistics blocks in the read-only pins:

- `snapmaker-orca@b1831e5dcb src/libslic3r/GCode.cpp`,
  `DoExport::update_print_stats_and_format_filament_stats` (1992–2059), and
  `GCode::_do_export` (3080–3098).
- `orca@8500fcdcca src/libslic3r/GCode.cpp`, the same functions (2313–2378,
  3524–3555).
- Both pins' `src/libslic3r/GCode/GCodeProcessor.cpp`,
  `GCodeProcessor::run_post_process` (Snapmaker: 4044–4059, 4481–4507;
  Orca: 993–1009, 1149–1175).

The final postprocessor matters: the generator fills missing earlier tool indices
with bare `0`, but **the exported file** has every configured slot, including trailing
unused slots, as `0.00`. Mica matches that final format: comma plus space separators,
two decimal places, physical slot order. Weight is volume in cm³ × density; cost is
weight in grams × configured cost per kg × 0.001. Totals sum unrounded values.

The standard block now has this order:

| Line | Change / condition |
|---|---|
| `filament used [mm]` | Aggregate becomes per-slot vector |
| `filament used [cm3]` | Aggregate becomes per-slot vector |
| `filament used [g]` | Added when any tool has positive weight |
| `filament cost` | Added when a tool has positive weight and positive cost |
| `total filament used [g]` | Added, always present |
| `total filament cost` | Added, always present |
| `total filament change` | Added only when tool changes > 0 |
| `total layers count` | Added, always present |
| `estimated printing time (normal mode)` | Existing, unchanged |
| `estimated first layer printing time (normal mode)` | Existing, unchanged |

Silent-mode time lines continue through the existing timing pass when enabled.
There are no separate wipe-tower weight/cost comment lines in either upstream
Orca block; their internal tower statistics are not additional footer lines.
The newer stock Orca also falls back to tool ordering for no-tower change counts;
Mica uses its actual generated change count with or without a tower.

`export.rs` emits the Orca block after Mica's existing per-role/per-tool comments.
Those extension comments and the explicit-statistics marker are retained. The small
new `time_estimate/filament/orca.rs` formatter fills the block from the existing
bounded explicit-G-code measurement pass. This includes startup purge and tower
extrusions once and excludes retraction/deretraction debt. It avoids introducing
upstream's separate tower-volume estimate (which assumes 1.75 mm filament).
MMU accounting remains outside Mica's independent-tool support.

## Before/after outputs

A release build before editing reproduced all six supplied hashes. Afterward,
`cmp` passed for CORE One and XL, cube20 and 3DBenchy. Both U1 unified diffs contain
only statistics comments: the old two aggregate lines move below the retained Mica
extension block and become vectors; five new lines are added. The single-tool jobs
have no `total filament change` line. Everything else, including all executable
commands, thumbnails, headers, role statistics and time estimates, is identical.

For cube20 the replacement standard block is:

```gcode
; filament used [mm] = 1214.36, 0.00, 0.00, 0.00
; filament used [cm3] = 2.92, 0.00, 0.00, 0.00
; filament used [g] = 3.62, 0.00, 0.00, 0.00
; filament cost = 0.22, 0.00, 0.00, 0.00
; total filament used [g] = 3.62
; total filament cost = 0.22
; total layers count = 100
; estimated printing time (normal mode) = 15m 10s
; estimated first layer printing time (normal mode) = 39s
```

| Profile / model | Before SHA-256 | After SHA-256 |
|---|---|---|
| CORE One / cube20 | `65f08d88a671a5e55b7f4a657f10c39c86aa2bbfb74edcbad2fe91ddb58167f3` | unchanged |
| CORE One / 3DBenchy | `98eba153ebf3563312c37bdf524bce60f2f01ead26d44000e1f35c3eafb54b92` | unchanged |
| XL / cube20 | `cc06b5d71bf0d9989d1733e74a8b1d716c045270f0604c81c28f0ec6289d078b` | unchanged |
| XL / 3DBenchy | `90fc179d2cd7a402991dd588b92700b0ae7e9bb1172f1c8193600d2572654be7` | unchanged |
| U1 / cube20 | `2ca749170a635d17293362b3f33680569e469bc69b95a8ac5e3cd16ba9fe64e9` | `2eda1532ffd63677496e86b2b4ef6c2a5d02950bdf141601c9f2a7b005780e79` |
| U1 / 3DBenchy | `2c1e3b32bb521b0192b3632bca27fae4399e7136c06f323cdbb282ff40ad49b5` | `7cfb06f9362ef110ef9470dbc2d0de6083764c3c9314c956acb6556426528d49` |

No tests pin the old U1 output hashes. Corpus references retain the hashes of the
**real slicer reference files**, which have not changed. Older documents keep
their historical before/after hashes; this is the footer-change hash list.
The directory contains both binaries, all twelve outputs, six validator reports,
the two nonempty `.diff` files, `verification.json`, and
`oracle-footer-comparison.json`. Every manual Mica launch used a fresh temporary
`MICA_CONFIG_DIR`.

## Saved real Orca U1 comparison

Compared every standard-block line with the existing OrcaSlicer U1 output at
`out/ref/u1-benchy/plate_1.gcode:138609`.
All nine tags, ordering, slot counts, separators and numeric precision match.
No fresh Orca process was needed.

| Statistic | Saved Orca 2.4.1 | Mica after change |
|---|---|---|
| mm | `3780.67, 0.00, 0.00, 0.00` | `3780.53, 0.00, 0.00, 0.00` |
| cm³ | `9.09, 0.00, 0.00, 0.00` | identical |
| g | `11.28, 0.00, 0.00, 0.00` | identical |
| cost | `0.68, 0.00, 0.00, 0.00` | identical |
| total g | `11.28` | identical |
| total cost | `0.68` | identical |
| total layers | `240` | identical |
| normal time | `40m 11s` | identical |
| first-layer time | `18s` | `1m 11s` |

Length and first-layer time are existing engine/planner differences, unchanged by
this patch. This compares metadata to a saved slicer oracle, not physical U1 slot
behavior. Review the full-slot vectors and positive-value gates closely; the next
hardware check is whether Moonraker populates the expected per-tool weights and
Klipper selects those slots.

## Verification

- Release CLI build, `cargo fmt --all` / `--check`, workspace clippy with
  `--offline --all-targets -- -D warnings`, and `cargo xtask provenance` pass
  (587 source files checked, 337 ported/adapted).
- `cargo xtask corpus --quick` passes all **20** cases in 4.95 seconds, with
  **0 validation errors**. All six manual default outputs also validate with
  **0 errors**; their existing warning counts remain 1/9 (CORE One), 1/9 (XL),
  1/11 (U1).
- Three new formatter tests cover nonconsecutive slots, heterogeneous diameters,
  density/cost, startup purge, arcs and per-tool retraction debt; zero weight,
  zero cost and positive values that round to zero; rule/flavor independence;
  and idempotent postprocessing. The existing filament accounting test passes.
- The new export integration test exercises T1/T3 jobs with and without a tower,
  verifies all footer tags in order, confirms zero entries at T0/T2 and actual
  tool-change/layer counts, and compares filament lengths with the independent
  validator (0 errors).

In a restricted build environment (no loopback sockets, no file-system events), the socket tests
of `mica-camera` and `mica-update` and the file-event test of `mica-library` cannot pass; all
G-code and profile tests pass.
