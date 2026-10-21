# G-code quality and stability safety net

`mica validate FILE --printer PRESET` validates text or Prusa binary G-code. It exits nonzero for
errors and accepts the same `--vendor`, `--bundle`, `--profiles-dir`, `--print`, `--filament`, `--bed`
and `--set` profile options as slicing. `--json` emits counts, diagnostics, tool totals and per-role
filament. XL and U1 printer names select compatible default process/filament families; pass the
actual filament and bed for temperature checks. A valid file is not a hardware print certification.

The library API is `mica_gcode::validate::{Profile::from_config, validate_text, validate_file}`.
`Report` includes errors/warnings, line numbers, layer/toolchange counts, net deposited filament
per tool, and per-role lengths. `Profile` permits explicit exclusion polygons and a known tower
footprint. Binary line numbers refer to concatenated decoded G-code. Container failures include
block byte offsets where available. EOF findings use the last decoded line.

## Rules and interpretation

| Area | Checks | Interpretation |
|---|---|---|
| Motion | Absolute/relative XYZ and E, G92, units, G0/G1 and I/J G2/G3, finite coordinates, bed polygon and exclusion crossings, maximum Z | Printable moves are bounded; profile startup/shutdown and custom scripts are outside the printable-motion phase. Homing invalidates unknown axes. Linear polygon crossings are tested at all intersection intervals; arcs are sampled at ≤0.02 radians. R arcs are explicitly warned as unsupported. |
| Layer Z | Layer markers, nondecreasing Z, pending/missing Z, positive heights, profile height range, downward motion below the reached layer, extrusion above the marked Z | Travel before arriving at the next layer is legal. Scarf ramps, support layers, thick bridges and sparse towers can require exceptions; below-layer/path-height/position findings are warnings. Sequential-object Z resets are currently errors. |
| Extrusion | Per-tool retraction debt, G92 and absolute-E resets, excessive retracts, stationary excess priming, explicit travel-role extrusion, single-move E above 100 mm | Retracts may accompany wipe motion. Final parked tools may retain a bounded retract. Firmware G10/G11 is approximated, since the file need not specify its configured length. |
| Flow | Rounded-rectangle area from reported width/height, filament diameter and multiplier; deviations beyond ±25%, with a 0.001 mm rounding allowance | Warnings, excluding very short segments, wipes, tower purge and ironing. Thin bridges use their actual height; equal width/height gives circular thick-bridge area. Variable-width/scarf segments may still need inspection. |
| Heat | Hotend target, initial M109 wait, bed target/M190 wait (to the first printing tool's first-layer bed temperature), print temperatures within filament limits, preheat before tool changes, M106 PWM in 0–255 (0–100%) | Bounds use filament low/high fields where available, otherwise selected print targets ±30°C. These are commanded temperatures, not sensor measurements. Standby that remains below the printing range is detected when extrusion resumes. |
| Tools | Tool index, preheat, XL S1 selection, U1 preextrusion macro (including the initial-tool exception), per-tool totals | Checks required observable commands, not full arbitrary template equivalence. Firmware macro internals and dock motion remain opaque. |
| Towers | Balanced Prusa/Orca tower entry/exit markers, explicit-footprint collisions, observed-footprint clearance warnings | An explicit footprint gives a definite collision error. Inferred rectangular bounds are conservative, especially for rotated or sparse towers. Entry/exit travel and firmware-controlled docking cannot be certified from extrusion bounds alone. |
| Structure | M486 and EXCLUDE_OBJECT nesting/names, M73 P/Q monotonicity and range, P100 completion, generator/header metadata | Startup label resets are warnings. Missing progress or metadata is warned. M486 object-name text is not interpreted as numeric G-code. |
| Totals | Aggregate/vector filament lengths and Mica per-tool length comments compared with simulated extrusion | A difference above max(2 mm, 2%) warns. Firmware-only purge/loading is unobservable. Role totals exclude retraction repayment and custom startup extrusion. |
| Binary | File magic/version, block types/order/lengths, CRC32, zlib/heatshrink/MeatPack decoding and required metadata blocks | Bounded streaming visitor reuses the existing codec. Checksum-free files are identified by a warning. No entire G-code file is retained. |
| Thumbnails | Base64, dimensions, PNG chunk CRCs and zlib scanline/filter validity, QOI op lengths/pixel counts/end marker | PNG and QOI supported. JPEG and interlaced PNG are explicitly rejected, not claimed as decoded. |
| U1 format | Orca header/executable markers, Klipper object labels, tool preextrusion | Unknown firmware macros are preserved/ignored; their execution requires hardware/firmware validation. |

Diagnostic storage retains the first 20 occurrences per rule while keeping full counts. Text lines
are limited to 1 MiB, binary blocks and encoded thumbnails to 16 MiB, PNG output to 64 MiB, and
role metadata to 64 bounded names. These limits prevent corrupt files from growing the simulator
without bound; they can reject unusual but otherwise valid files.

## Tests, fixtures and CI

**Fixtures.** Most slicing tests need the pinned vendor profiles in `upstream/` (see
`upstream/UPSTREAM.lock`): `scripts/fetch-upstream.sh --profiles prusa-profiles orca snapmaker-orca`
fetches exactly what the tests read (the Prusa profiles repository and sparse checkouts of
`resources/profiles` of OrcaSlicer 2.4.2 and Snapmaker Orca 2.4.0, about 330 MB, ~15 s). A full
`scripts/fetch-upstream.sh` (the source trees for porting) serves the tests too. Test processes
are isolated (`mica_core::user_data`), so they slice with these pinned profiles, never with an
installed PrusaSlicer or OrcaSlicer, and all paths are repo-relative (no machine's home folder).

**The skip rule** (`crates/mica-testkit`, a dev-dependency of every crate with such tests): a test
checks its fixture up front with `mica_testkit::require_fixture(path, why)` (or
`require_prusa_profiles`, `require_orca_profiles`, `require_snapmaker_orca_profiles`,
`require_profiles_for(args)`). When it is missing the test prints `SKIP <test>: … (why)` and
returns; with `MICA_REQUIRE_FIXTURES=1` (CI) it **fails** instead. No test decides to skip by
matching an error message, so a real failure always fails. The same rule covers the
`mock_printer` example the GUI's send tests start (`cargo test --workspace` builds it).

**Ignored tests** need what neither the repository nor the fetch script provides, and fail
when run (`cargo test -- --ignored`) without it:

| Test | Needs |
|---|---|
| `mica-core` `threemf_oracles::user_projects` | private projects under `MICA_PRIVATE_MODELS` (default: `~/models/private`) and the PrusaSlicer 2.9.6 reference app |
| `mica-core` `threemf_oracles::upstream_benchy_matches_prusaslicer_info`, `prusa_export` | the PrusaSlicer 2.9.6 reference app (`out/ref`, or `MICA_PRUSASLICER` + `MICA_PRUSA_DATADIR`); `prusa_export` also `MICA_PRUSA_3MF` |
| `mica-gcode` `time_estimate::live_prusaslicer_cube_and_benchy` | the PrusaSlicer 2.9.6 reference app |
| `mica-profiles` `u1_gcode::u1_live_orcaslicer_start_end_and_layer_structure` | OrcaSlicer 2.4.1 in `/Applications` (or `MICA_U1_ORCA_REFERENCE`) |
| `mica-profiles` `u1_gcode::saved_benchy_motion_estimator` | `MICA_U1_BENCHY_REFERENCE` (a saved Orca U1 Benchy) |
| `mica-cli` `project_settings_export::private_cat_no_change_export_is_verbatim` | a private project (`MICA_CAT_PROJECT` or `MICA_PRIVATE_MODELS`) |
| `mica-gui` `cli_parity_tests::binary_outputs_match_with_the_same_timestamp`, the 3DBenchy parity cases | `SOURCE_DATE_EPOCH=0` and release speed: `cargo xtask parity` runs them |
| `mica-core` `shortest_path::tests::reference_cases_from_env` | the C++ harness output (`MICA_SHORTEST_PATH_CASES`, `MICA_SHORTEST_PATH_EXPECTED`) |
| `mica-videodec` `a_missing_system_decoder_is_reported` | a system without an H.264 decoder (`MICA_VIDEODEC_EXPECT_UNAVAILABLE=1`) |
| `mica-profiles` `orca_vendors` reports | (reports, not checks) |

Tests that need the environment rather than data (a GPU for `render::gpu_tests`, a loopback
socket for the `mica-printhost` mocks) still return early when it is missing.

**Reference hashes as a test.** `crates/mica-cli/tests/reference_hashes.rs` slices cube20 and
3DBenchy on the CORE One, XL 5T, U1, MINI IS and MK3.5 with the gate's presets and
`SOURCE_DATE_EPOCH=0`, and asserts the sha256 prefixes (`d833d655 82b5234b`, `525f4840 50db0e69`,
`d86e39a8 1a3357ae`, `5093345f 1c06b7ab`, `c923aa75 01f46892`). The cube cases run in every build;
the Benchy cases need `--profile fast` or `--release` (about 40 s each in debug). The hashes are
recorded on macOS arm64; elsewhere the tests are ignored (other math libraries may round
differently). An intended G-code change updates the prefixes in the same commit, with the reason.

**CI** (`.github/workflows/ci.yml`), on macOS, Ubuntu and Windows: fetch (cached by the hash of
`UPSTREAM.lock` and the fetch script) the pinned profiles as above, `cargo fmt --check`, `cargo
clippy`, `cargo test --workspace --profile fast` with `MICA_REQUIRE_FIXTURES=1` (the reference
hashes, Benchy included, on the macOS arm64 runner), then `cargo xtask corpus --quick` (macOS and
Ubuntu; `MICA_CORPUS_QUICK_BUDGET=300` widens the 60 s local budget for shared runners). Expected
time per runner: ~15 s fetch (0 when cached), 10–20 min cold build (~3–6 min with the Rust
cache), ~3–5 min tests, ~1–3 min corpus. The full corpus and `cargo xtask parity` stay in the
local merge gate.

**Packaged builds** never read the build machine's checkout. A development binary falls back to
the pinned profiles of the checkout it was built in (`mica_core::user_data::upstream_dir`) only
when no PrusaSlicer/OrcaSlicer is installed and only while it runs from inside that checkout;
`scripts/release.sh` compiles with `MICA_PACKAGED=1`, which removes the fallback entirely.
`MICA_UPSTREAM_DIR` names the folder explicitly (the corpus runs a copy of the CLI outside the
checkout and sets it).

## Corpus

Run offline:

```sh
export PATH="/opt/homebrew/opt/rustup/bin:$PATH"
cargo xtask corpus --quick
cargo xtask corpus
cargo xtask corpus --case plates_orca --printer u1
```

`xtask` builds the release CLI offline, then runs the standard-library-only Python harness. Each
run snapshots the binary into its own system temporary directory so a concurrent rebuild cannot
change half of the matrix. The quick time budget excludes compilation.
All applicable cases require zero validation errors. Roles using at least 1 mm have a default
±25% oracle gate, configurable through `--role-tolerance`; smaller changes remain visible in the
report. This is a gross-regression guard, not a claim that ±25% geometry parity is desirable.
The report explicitly marks measurements without an oracle.

`tests/corpus/manifest.json` owns the matrix. The generated staircase, bridge pillars, 80 mm thin
brim tower and organic figure are deterministic, generated into temporary storage. Public models
retain their existing licenses and attribution. The painted cube uses two XL tools. Plate 1 of
`plates-orca.3mf` normally uses only three tools: the corpus overrides its second object to tool 4,
then requires four nonzero tool totals and a prime-tower role. Binary cube cases cover CORE One,
XL, MINI and MK3.5 (the Prusa printers whose profiles default to binary G-code). The MINI IS and
MK3.5 references are PrusaSlicer 2.9.6 slices with the pinned 2.5.12 profiles (a temporary
`--datadir` with `model:MINIIS` and `model:MK3.5` enabled, `--center` at the bed center,
`--no-binary-gcode`); `sequential_cubes` is too wide for the MINI's 180 mm bed, so the MINI
prints `sequential_cubes_small` (three cubes 60 mm apart, right to left).
Printer-specific fixtures are marked N/A on other printers, not reported as passes.

Private projects are read in place. `MICA_CORPUS_CAT` and `MICA_CORPUS_DENSE_PAINTED` override the
optional paths; absent files are reported as skipped. The `cat_plate1` case is multi-tool and therefore
runs on XL/U1. No private geometry or G-code is copied into the repository.

The harness writes `out/corpus-report.md`; complete JSON diagnostics and generated G-code stay
in its reported temporary directory. Peak RSS comes from `wait4` for each slice child (macOS bytes,
Linux KiB), not a cumulative maximum over earlier cases. The harness currently targets Unix;
Windows peak-memory collection remains unimplemented.

To calibrate a deliberately chosen matching reference without storing its G-code:

```sh
cargo xtask corpus --record-reference cube20/core=/path/to/reference.gcode
```

The full run (no options) then checks the **CLI/GUI invariant** (`cargo xtask parity`, also
runnable alone, optionally with a test name): for the same files, presets and placement the
GUI's G-code is byte-identical to `mica slice`'s. `crates/mica-gui/src/cli_parity_tests.rs`
drives the editor headless (open, gizmo edits, arrange, the slice worker, Save) and the CLI
through `mica_cli::run`, the code the binary runs: STLs on the CORE One, XL and U1 (3DBenchy
only in this release run), cube20 on the MINI and MK3.5, two-STL plates, `painted-cube.3mf` and both plates of
`plates-orca.3mf`, a moved, turned and scaled STL and edited projects saved by the GUI and
sliced by the CLI, and an arranged plate that reopens to bit-identical model frames. Both sides
build the print with `mica_core::input::prepare_plate` from the same objects: a plain object (an
STL, a one-volume 3MF object) as its model frame (the mesh as loaded plus the instance
transform, the editor's transform composed on), which a saved project stores as is.

Only validated references are accepted. `references.json` stores numbers, source/model hashes,
source filenames and profile arguments. Model/profile changes require explicit recalibration.
Current coverage includes fresh Prusa 2.9.6 references for all eight ordinary models on CORE/XL
and saved Orca 2.4.1 cube/Benchy references. Separate saved XL/U1 multi-tool and Prusa binary files
were also validated. Native painted-project oracles were not relaunched: prior documented Prusa
and Orca crashes remain unresolved (see [3MF export](3mf-export.md)).

## Stability changes and audit

`print::prepare_object` and `slice_object` now return `Result`; callers must handle failure. The
existing `try_prepare_object*` APIs remain. Preparation checks mesh indices, finite/range-limited
coordinates, zero thickness, all-degenerate triangles, layer/nozzle/flow compatibility, bounded
wall/raft/skin counts and a 100,000-layer allocation limit before geometry work. Region meshes
receive the same validation. Configuration numeric parsing rejects NaN/infinity. CLI unknown
setting names error while known Mica tower coordinates remain supported.

CLI preflight reports open/non-manifold edges, potentially intersecting shell bounds, sub-mm
models, partially below-bed models and zero-walls/zero-infill settings. It rejects out-of-volume
and entirely below-bed models. Shell bounds are a conservative diagnostic, not a full triangle
self-intersection/repair algorithm; intersecting closed shells can be intentionally unioned.

The unwrap/index audit focused on import and job boundaries: malformed height-range XML now
uses errors, facet-offset arithmetic checks overflow, public preparation no longer calls `expect`,
and invalid mesh indices are rejected before downstream indexing. Optional Voronoi capture I/O
and coordinate conversion no longer panic; its cache mutex recovers after an unwound job.
Assertions tied to internal geometry invariants remain, including the Arachne graph assertion.
This is not a proof that every low-level geometry function is panic-free on arbitrary manually
constructed public structs.

`mica_core::job::Boundary` wraps CLI slicing and the GUI worker with `catch_unwind`, reports the
active stage and a repair/reporting hint, and writes a crash log to `~/Library/Logs/Mica Slicer/`
on macOS (platform state folders elsewhere). If logging fails, the error says so explicitly.
Logs include the payload and a **post-unwind boundary backtrace**, not the original panic stack.
The GUI keeps its scene and can start another job. Rust aborts, allocation failure and stack
overflow cannot be recovered with `catch_unwind`; input limits reduce, but cannot eliminate,
resource exhaustion.

Deterministic tests cover open/non-manifold and intersecting meshes, invalid face indices,
degenerate/flat/empty geometry, nonfinite/enormous/tiny/below-bed models, empty and malformed
3MF XML, invalid painting strings, facet index overflow, zero walls/infill, excessive layer height,
unknown CLI keys, STL byte mutations, panic reporting and a subsequent successful job. Validator
tests inject motion, heat, flow, label, progress, tower, thumbnail and binary corruption faults.

## GUI monkey test

The desktop GUI must never crash or hang, whatever the user does. `crates/mica-gui/src/monkey.rs`
checks that with a monkey test: a headless `App` with the real profile bundles (and a throwaway
profile store) performs thousands of random but valid user actions from the editor's own
vocabulary, and checks invariants after every step.

```sh
export PATH="/opt/homebrew/opt/rustup/bin:$PATH"
cargo xtask monkey --quick                          # 3 seeds × 200 steps, well under a minute
cargo xtask monkey                                  # 20 seeds × 2,000 steps (about 15 min, 12 cores)
cargo xtask monkey --seed 7 --steps 900 --verbose   # one seed in-process, printing every action
cargo test -p mica-gui monkey                       # 40-step smoke run in the unit tests
```

`xtask` builds the release GUI offline and runs `mica-gui --monkey`. Options: `--seeds N`,
`--first S`, `--steps N`, `--jobs J` (seeds side by side; each child gets its share of the
cores as `RAYON_NUM_THREADS`), `--seed S` (one seed in this process), `--cancel-after MS`
(every slice wait cancels after MS, to probe cancel latency), `--rss-every N`,
`--rss-growth-mb N` (one seed: another memory-growth limit, to follow a growth further),
`--save-at STEP` (one seed: save the scene as an Orca project before that
step, to reproduce a slice with `mica slice`), `--no-new-features` (the action sequence of runs
before the October 2026 feature actions, to reproduce an older seed) and `--verbose`.
Every seed runs in its own child process, so aborts, stack overflows and hangs that
`catch_unwind` cannot contain are still reported per seed. Crash logs of contained panics go to
`MICA_CRASH_LOG_DIR` (the job boundary honours this variable; xtask points it at the system
temporary folder). An unoptimised build (`target/debug/mica-gui --monkey …`) also checks
integer overflow and debug assertions; its timeouts are ten times longer.

**Actions** (weighted, chosen by a seeded SplitMix64 generator): open models and projects (cube20,
Benchy, hole_block, the painted cube, `plates-orca.3mf`, copied into the run's folder so a ⌘S
never touches the fixtures) and use or keep a project's settings; select, toggle, box-select,
select all and select volumes or height ranges; gizmo deltas as nudges and as drag gestures
(sometimes cancelled with Esc), Euler rotations and lay-on-face; tool-card position, rotation,
size, scale, mirror and reset up to the fields' limits (±10⁶ mm, 0.01 mm, 0.1 %–10⁶ %); every
object command (copy, paste, duplicate, instances ±, delete, centre, drop, mirror, rotate,
change tool, move to plate, modifiers, negative and support volumes of each shape, height
ranges, split, arrange, arrange all, auto-orient, export STL); undo and redo bursts; settings of
the print, filament and printer tabs and object/volume/range overrides with values from the
option table (enum choices, toggles, numbers inside the table's range and at both ends, far
outside it where the table sets no bound, broken custom G-code templates, degenerate bed
shapes); printer switches between CORE One, XL 5T and U1 and filament changes per tool; plates
(add, remove, select, arrange, orient, lock); the cut tool (plane moves, tilts, cut); all four
paint kinds with every brush, radius, erase and overhang option, paint-overhangs and clear;
the prime tower (placement, drags, typed values, its commands); slicing the plate or all
plates, waiting for it, cancelling at a random point or outright; preview toggles and slider,
colour-mode and mask scrubbing; save as Orca or PrusaSlicer project and reopen; the printer
workflow against the loopback mock printers (`mock_printer` example, built by xtask on free
ports: XL, CORE One, U1): My printers, connection tests, the Machine view (also on a stale
printer index), the Print dialog's check and upload, filament sync, adding printers at random
addresses and removing them; the calibration menu (every test, its plate, typed results and
saving them); the October 2026 features (`monkey/features.rs`): built-in shapes from the Add
shape gallery as objects, parts, modifiers and support volumes at tiny, huge and degenerate
sizes, textures on all surfaces (print or object settings, built-in, procedural and missing
custom images, depths up to 2 mm, point distances down to 0.05 mm, per-slot paint overrides)
with slices and "Bake texture into mesh", ColorMix mixes (valid and broken specs, delete, move,
objects on mixes), Brick layers unlocked by hand with odd start layers, margins and layer
heights, profile import from a fake home with OrcaSlicer, Snapmaker Orca and PrusaSlicer
presets (extreme values, @-tagged and non-ASCII names, self-inheriting, orphaned and broken
files: review, import, use, remove, merge duplicates, re-import after a change), and Mica MCP
live calls through the window's request queue (every tool with valid and broken arguments,
`create_mix`, `add_shape`, `undo`, `slice_summary`, `save_project`), mostly not waited for, so
the window keeps being edited while they run; and random
pointer frames through the whole window (clicks, drags, right-click menus, typing `nan`,
`1e400`, `-0`, `12,5` … into whatever field took focus, shortcuts with ⌘/⇧).

**Invariants** after every step:

- no panic on any thread (a panic hook records message, location and the workspace frames;
  every step runs under `catch_unwind`), and no job reports a contained crash;
- no hang: a watchdog aborts with the trace when no action completes for 180 s, the parent
  kills a seed past its wall-clock budget, and a cancelled job must have stopped within 15 s
  (measured while waiting, too; the run reports the slowest stop);
- undo everything then redo everything returns exactly the same scene (every 100 steps);
- the scene stays valid: finite transforms, scale > 0, finite bounds, height ranges ordered,
  selection, multi-selection, selected volume/range and cut tool on existing objects, and every
  running, waiting or queued slice job naming a plate that exists;
- a saved project opens again with every object (those on a bed on the same plate);
- process memory grows by less than 3,000 MB within any 1,000 steps. On macOS this is the
  physical footprint (`/usr/bin/footprint`, what Activity Monitor shows), elsewhere the resident
  size; `ps`'s resident size also counts pages the allocator already released. The heaviest
  slices the monkey builds (a painted Benchy scaled 3× at 0.07 mm layers, about 1,900 layers)
  legitimately peak near 2.7 GB (the allocator keeps part of it after the job);
- printer connections go to loopback addresses only (`printhost::restrict_to_loopback`): an
  address typed at random is refused before any connection or name lookup;
- every completed slice passes `mica validate` with zero errors, using the configuration the
  job started with. Validator errors are only waived (and counted, with a `MONKEY-NOTE` line)
  when that configuration holds an implausible value: outside the option table's range, or,
  where the table sets no bound, negative for a non-negative option, above 500 °C, or more than
  20× the default (a 10 m retraction is accepted by the editor and must not crash anything,
  but its G-code cannot be valid);
- a failed slice always shows: until its plate changes, the plate keeps the error and shows no
  slice, its label carries the error marker, and its notification is shown (or was dismissed),
  so the header reads "Slice failed: …" instead of the previous totals;
- no slice folder outlives the app;
- every Mica MCP live call is answered within 120 s, whatever the window did meanwhile.

A failure prints its kind, the evidence (for validator errors the kept G-code and the job's
temperatures and layer heights) and the last 60 actions, and the command that reproduces it.

### What it found

| Issue | Root cause | Fix | Regression test |
|---|---|---|---|
| Slice result written to the wrong plate, or an out-of-bounds panic, after removing a plate, Arrange all or undo while a job ran; removing an earlier plate also changed the current plate | Jobs, waiting requests and the Slice-all queue named plates by index; `Plates::remove` did not shift `current` | `Plate::id`; jobs, requests and the queue use ids; jobs of vanished plates are cancelled (`drop_orphan_jobs`); `remove` keeps the current plate | `a_slice_finds_its_plate_after_an_earlier_plate_is_removed`, `undo_cancels_a_job_whose_plate_was_replaced` |
| Arrange hung the window (endless new plates) | An object larger than the bed was spilled to a new plate, where it did not fit either | Objects that do not fit an empty bed stay put and are reported | `arrange_leaves_an_object_larger_than_the_bed_alone` |
| UI froze for minutes after scaling a model 10,000× | The prime-tower estimate planned up to 100,000 layers on the UI thread at every change | The estimate stops at `max_print_height` | `estimate_never_plans_past_the_printer_height` (mica-core) |
| A cancelled slice ran on for minutes | The GUI sliced objects outside the build volume (the CLI refuses them); a 200 m model spent minutes in single uncancellable geometry operations | The worker runs the CLI's `safety::placement` preflight | `slicing_refuses_an_object_outside_the_build_volume_at_once` |
| 3 GB for one paint stroke | The circle brush along a huge facet seen edge-on subdivided it into tens of millions of triangles | `TriangleSelector` subdivision budget (2,000,000 added triangles) | `a_stroke_along_a_huge_facet_stays_within_the_subdivision_budget` (mica-core) |
| Models could not be opened at all; saved projects could not be reopened ("perimeter_extruder selects tool 2, printer has 1 tools") | Tool selectors were unbounded in the editor and kept after a printer switch, and loading resolved them strictly | The editor clamps tool selectors to the printer's tools; GUI loading keeps assignments up to 64 tools (`SettingsMapper::lenient_tools`), the worker clamps when slicing; the CLI stays strict | `tool_selectors_stay_within_the_printer_and_models_still_open` |
| Orca-flavour projects with an object's `top_solid_infill_speed = 50%` could not be reopened | Orca has no percentages for that option; the importer rejected the whole project for one value | Save resolves such percentages to absolute values; the GUI import reports and skips a rejected object value instead of failing (CLI strict) | `orca_projects_with_percent_speeds_open_again`, `the_editor_skips_a_rejected_object_value` (mica-profiles) |
| The cut tool cut an object that was not the selected one | `CutState` named its object by index; Select all, deletes and reorders left it open | The cut also holds the object's id; `cut_target` and `validate_cut` close a stale tool | `the_cut_tool_never_cuts_another_object` |
| Undo/redo restored a different selection | Painting, lay-on-face and plate switches moved the primary selection directly and left the old object's volume selected | `validate_selection` after every viewport action and change | `a_new_primary_object_drops_the_old_ones_selected_part` |
| `mica validate` errors on U1 multi-tool G-code (cold extrusion) | Orca-only filament options (`nozzle_temperature_range_low/high`, …) were taken from the first filament for every tool | Per-tool composition of Orca filament options | `every_u1_tool_keeps_its_own_filament_options` |
| Validator false error on XL plates whose first tool's bed is cooler than tool 1's, and on filaments whose later layers run hotter | The bed-wait rule used tool 1's hottest bed temperature | It uses the first printing tool's first-layer bed temperature | `bed_wait_follows_the_tool_that_prints_first` (mica-gcode) |
| The first-layer bed temperature field clamped every value to 0 °C | PrusaSlicer's `def->max = 0; def->max = 300;` typo read as the first assignment; a stray maximum of 20 on `min_bead_width` | `gen-option-text` takes the last assignment and ends a definition at the next `def =` | `last_assignment_wins_and_blocks_end_at_the_next_def` (xtask), `the_first_layer_bed_temperature_field_goes_up_to_300` |
| G-code with an empty layer at `;Z:0` | A 0.0001 mm first layer passed preparation; G-code Z has three decimals | `safety::settings` refuses layers under 0.01 mm | `contradictory_and_nonfinite_settings_are_diagnosed` (robustness) |
| Arrange all turned instances into separate objects | It renumbered every gathered object | Renumbers per (plate, id) | `arrange_all_keeps_instances_and_plate_identities` |
| 9,358 `mica validate` errors ("hotend exceeds filament maximum") on a U1 temperature tower for tool 4's PETG (seed 0) | **Validator false positive from the harness.** The tower prints through T0 with the tested filament moved into slot 0 (`calibration::tool_first`, as `mica calibrate` and the card's "load it in tool 1" note); the monkey validated it against the plate's own configuration, i.e. tool 1's PLA limits. The G-code was right for the filament it heats. `tool_first` also left the Orca-only filament option text (`nozzle_temperature_range_*` in the extras) on tool 1's values | `App::job_config` is the configuration a job slices with (the start of a job and the monkey both use it); `tool_first` swaps the numeric Orca filament extras too | `a_calibration_plate_is_validated_against_the_filament_it_tests`, `tool_first_swaps_orca_filament_text` |
| Stack overflow painting a seam with a 0.4 mm brush on a model scaled about 10,000× (seed 5) | **Unbounded recursion.** Far from the origin neighbouring f32 coordinates are further apart than the brush's 0.08 mm edge limit, so a side's midpoint was one of its ends: every split copied the triangle and `select_recursive` nested 43,306 levels deep (macOS crash report) until the main thread's 8 MB stack ran out | Splitting stops at `MAX_SPLIT_DEPTH` (64, the depth the 3MF facet format accepts, so every recursive walk of the tree is bounded), and a side whose midpoint equals an end is treated like one under the edge limit (not split) | `a_fine_brush_far_from_the_origin_stays_shallow` (mica-core; aborts with a stack overflow without either fix) |
| 11 `huge-extrusion` errors, single moves extruding 266 mm of filament (seed 15) | **Real G-code bug.** A first-layer width of 17.24 mm on a 0.4 mm nozzle (the editor accepts PrusaSlicer's 50 mm literal maximum) printed a 41-layer raft base with 17 mm lines. OrcaSlicer's `validate()` refuses `initial_layer_line_width` and every other line width above 5× the nozzle (`MAX_LINE_WIDTH_MULTIPLIER`) and bridge widths above the nozzle; Mica checked only the lower bound | `safety::settings` refuses widths above 5× the nozzle (first-layer and support widths included) and bridge widths above the nozzle, before any geometry | `contradictory_and_nonfinite_settings_are_diagnosed` (robustness) |
| A cancelled slice still in "supports" after 15 s (seed 16) | **Quadratic loop and an unbounded setting.** A 1 km `xy_contour_compensation` grew every layer kilometres wide; Clipper2's `SimplifyPath` finds the next and prior surviving vertex by scanning the removal flags, so removing a long run of collinear vertices costs the square of its length (400,000 vertices: 24 s; one top surface held the job for minutes) | `simplify_path` keeps the surviving vertices in a linked ring (O(1) neighbours, the same vertices removed in the same order, checked against the flag-scanning original); `safety::placement` refuses an XY compensation that grows the model past the bed, which would print walls outside it | `test_simplify_path_matches_the_flag_scanning_original`, `test_simplify_path_with_a_long_collinear_run_is_fast` (clipper2), `enormous_tiny_and_below_bed_models_are_diagnosed` (robustness) |
| A cancelled "slice all" still preparing after 15 s (seed 14) | **Missing cancel checks.** A 100 mm bridge line width (also unprintable, see seed 15) on large scaled Benchies; the support column projection and bridge-over-infill ran 10–20 s each without polling the cancel flag | The bridge width is refused; `support::material::project_columns` polls per layer, bridge-over-infill per layer (candidates and clusters) and per region group. A cancel during the support stage of that plate now stops in 0.04 s (3.7 s before) | `support_projection_stops_at_a_cancel` (mica-core) |
| Stability campaign 2026-10-21 (seeds 107, 112): a finished slice looked stale at once and the slice cache never found it | **Fresh identity per job.** With Brick layers (or Textures) off, every slice job stripped an object's regions model with `Arc::make_mut`, a new copy each time; jobs compare regions by identity | `scene::derived_region` keeps one stripped copy per source region | `a_slice_fits_its_plate_with_a_feature_off_that_its_parts_use` |
| Library model indexed outside the test folder (seeds 108, 115) | A folder added while it was away (moved, unmounted) was stored as typed (`/var/…`, a symbolic link) instead of resolved | `mica_library::canonical` resolves the deepest existing ancestor | `a_folder_added_while_away_is_stored_resolved` (mica-library) |
| No frame for minutes with the variable layer height tool open on a model scaled to 1 km (seed 112, first run) | The overlay split every wall into 0.4 mm slabs: millions of slabs, gigabytes of vertices on the UI thread | The bands grow past 1,000,000 slabs per object | `the_overlay_of_a_huge_object_stays_bounded` |
| A saved project could not be opened again: "invalid or duplicate virtual extruder ID 5" (seed 302) | A ColorMix mix made on the 4-tool U1 kept number 5 on the 5-tool XL; the file reused the opened text verbatim | Mixes are renumbered after the printer's tools on a printer change (objects and paint follow); the opened text is only reused while it fits; a mix using a missing tool is reported | `mixes_follow_a_printer_change_and_the_project_still_opens` |
| Validator error "nonfinite command parameter" on CORE One L G-code (seed 311) | **Validator false positive.** `M106 P5 R A125 B10` (the profile's bed fans; `R` is a flag) | `R` without a number is accepted on M106 | `temperature_waits_and_fan_are_checked` (mica-gcode) |
| 5 GB in 50 steps (seed 605), `mica slice` 55 s and 6 GB before refusing | A 1 km `wipe_tower_brim_width` built the whole brim before the bed check; the GUI's tower estimate built it on the UI thread | `TowerPlan::plan` refuses a tower or brim wider than the bed before any geometry | `a_brim_wider_than_the_bed_is_refused_before_any_geometry` (mica-core) |
| CLI fuzz: "Parsing error … Not a variable name" in the start G-code | An XY compensation that removes the whole object left no first-layer box for `first_layer_print_min` | Such an object stops with upstream's empty-first-layer error | `an_object_compensated_away_is_not_printed` (mica-cli) |
| CLI fuzz: 64 damaged bytes in a texture cache entry sliced a wrong mesh (100,000 layers) | The disk cache checked lengths only | Entries carry a SHA-256 (`MICATEX3`); writers use unique temporary names | `the_disk_format_round_trips_and_rejects_damage` (mica-core) |
| Texture paint lost when an Orca/U1 project is opened | The Orca import dropped every Mica object option as "no canonical equivalent" | Mica's own options map to themselves | `texture_paint_slots_round_trip` |

Found by the review of user and file data paths: typed `nan`, `inf` and `1e400` reached object
transforms (`parse_number` ignores non-finite input; `non_finite_typed_numbers_are_ignored`); a
3MF model part without triangles underflowed the painting split
(`a_model_part_without_triangles_slices_its_painting_safely`); the binary export indexed the
last layer of an empty job; slice-job mutexes now survive a contained panic; the wgpu callbacks
no longer unwrap their renderer. The new temperature guard rail tells the user before slicing
when a nozzle temperature is outside the filament's declared range. File dialogs return
immediately in headless mode (screenshots, the monkey).

### Last full run

`cargo xtask monkey` on a 12-core Mac (6 seeds side by side), release build, 2026-10-17, after the
seed 0/5/14/15/16 fixes above (before them, on the memory-fix merge, 5 of 20 seeds failed): 20 seeds ×
2,000 steps in 891 s, **0 failures**. 40,000 steps; 1,765 slice jobs started, 430 completed and
validated with 0 validator errors (28,324 warnings), 866 jobs ended with an ordinary error (now
including the refused widths and XY compensation), 95 cancelled while watched (slowest stop
5.4 s); 420 undo-all/redo-all checks; 367 save/load round trips; 66,792 whole-window frames; peak
RSS 1,877 MB, largest growth within 1,000 steps 1,840 MB. Eleven completed slices with
implausible settings (hand-edited start G-code, a 10 m toolchange restart extrusion) had their
validator errors waived.

The run before it, 2026-10-17, ahead of the calibration and printer GUIs: 20 seeds × 2,000 steps
in 893 s, **0 failures**. 40,000 steps; 1,576 slice jobs started, 375 completed and
validated with 0 validator errors (34,109 warnings), 733 jobs ended with an ordinary error
(settings or placement the engine refuses, a tower over an object, …), 110 cancelled while
watched (slowest stop 3.0 s); 420 undo-all/redo-all checks; 377 save/load round trips; 59,490
whole-window frames; peak RSS 1,628 MB, largest growth within 1,000 steps 1,331 MB. Six completed
slices with implausible settings (a 10 m toolchange restart extrusion, a 1,000 °C standby delta)
had their validator errors waived. After the merge of the calibration and printer GUIs (6 seeds × 1,000 steps, with the
printer actions): 0 failures in 143 s, 275 slices started, 75 validated with 0 errors, peak RSS
1,260 MB; hand-edited start G-code (Klipper gets no automatic temperature commands) is waived
like an implausible value. `cargo xtask monkey --quick`: 3 × 200 steps in 18 s. An
unoptimised build (overflow checks, debug assertions), 6 seeds × 400 steps: 0 failures.

**Limits.** The 3D view's GPU paint callbacks do not run headless (the CPU side of every panel
and the viewport does); native file dialogs, the printer connection (not yet wired into the
GUI) and the automatic re-slice timer (off headless) are outside it. Slices are bounded at 25 s
before they are cancelled, so very slow jobs are exercised for cancel, not completion. The
seeded action sequence depends on the monkey's code: a seed reproduces with the same binary.

## G-code corrections

1. Single-tool and tower-free multi-tool headers omitted explicit startup purge extrusion.
   All exports now use the existing timing pass's per-tool retraction-debt accounting for total
   length/volume and emit per-tool totals for every multi-tool job. Macros remain unmeasurable.
   This intentionally changes Mica's old path-only totals; it does not change E moves.
2. Flow height was written only at layer changes/tower boundaries. Combined infill and thick
   bridges can use a different height. The exporter now emits `;HEIGHT:` when path flow changes,
   restoring accurate viewer/validator metadata. Motion and extrusion amounts remain unchanged.

New upstream-derived code is the streaming libbgcode visitor and bounded QOI format validation;
register entries identify both. Other validator/harness/job-safety code is original and calls
existing Mica flow/codec APIs. No new libraries or third-party assets were introduced.

`cargo xtask corpus` writes the complete printer table, reference results and remaining warnings
to `out/corpus-report.md`.
