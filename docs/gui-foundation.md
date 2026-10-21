# GUI foundation

Part 2 (theme, bed and plates, lighting, gizmos, camera, preview overlays) is described in
[gui-viewport.md](gui-viewport.md); the screenshot command there supersedes the one below.

The native `mica-gui` binary uses eframe 0.37 with the wgpu backend (no glow).
All new GUI code is original. It reuses Mica's existing geometry, profile, slicing,
tower, G-code, PNG and binary G-code implementations.
Dependency license expressions were checked in the fetched crate manifests and recorded
in `THIRD_PARTY.md` (rfd is MIT; bytemuck also offers Zlib). The transitive
egui default fonts have separate Ubuntu/Bitstream/MIT terms; their notices are
retained in `docs/licenses/egui-fonts/`.

## Run

```sh
export PATH="/opt/homebrew/opt/rustup/bin:$PATH"
cargo run --offline -p mica-gui
```

Profiles come from a development checkout's pinned `upstream/` first, then (packaged builds) the
OrcaSlicer subset shipped in the app and the Prusa bundle downloaded from Prusa once
(`mica_profiles::vendor_profiles`, `prusa_download.rs`), then installed PrusaSlicer / OrcaSlicer.
Override locations with `MICA_PRUSA_BUNDLE=/path/PrusaResearch.ini` or
`MICA_ORCA_PROFILES=/path/to/profiles`. Prusa's Mica user profiles are registered from
`mica_profiles::user::Store::default_root()`; Orca native user presets are loaded from
its macOS/Linux `user/default` directories. Printer compatibility filters print and
filament choices. Each nozzle gets its own filament selection. Snapmaker exposes
plate selection; Prusa uses the plate dimensions/settings from its printer profile.

Open or drop STL / 3MF files, select a model with a click, or press and drag any model to
select and move it on the bed in one go. Background left drag orbits, right/middle drag pans, and the wheel zooms.
The object list exposes tool assignment and Z rotation. Position fields are offsets
from imported placement. Arrange uses rows with 10 mm gaps and centers the group.
System theme changes are followed automatically.

Shortcuts: Cmd/Ctrl+O opens, Cmd/Ctrl+G exports, Tab changes Prepare/Preview,
Delete/Backspace deletes, A arranges. Camera views: 0 isometric, 1 top, 2 bottom,
3 front, 4 rear, 5 left, 6 right. Shortcuts do not intercept text/number editing.

Slicing runs on a worker thread. Editing is disabled until it completes or cancels;
camera interaction remains available. Cancel is cooperative between preparation
stages and exported layers; a single mesh preparation, the tool scheduling prepass,
or time-analysis postpass must finish before cancellation takes effect. Export uses
a native save dialog and copies the selected completed `.gcode` / `.bgcode` on a
background thread. Klipper exports text only. Temporary jobs are removed when the
last preview/export reference is released; a force-killed process can leave its
`mica-gui-*` temporary directory behind.

## APIs and rendering

- `mica_core::input::{load, Loaded, InputObject, append}` is the former CLI import
  and placement logic without eager slicing. It returns warnings instead of printing
  them. The CLI wrapper prepares the objects as before and retains its diagnostics.
- `LayerSource::is_cancelled()` defaults to false. Export checks it between layers;
  existing CLI/library callers have unchanged behavior.
- `mica_gcode::multi::objects_info` is now public for binary export metadata.
- `scene/` contains camera math, CPU triangle picking, immutable shared indexed
  meshes and object transforms. Bounds are cached; edits do not copy large meshes.
- `render/` owns indexed mesh uploads, three WGSL pipelines, colour/depth viewport
  targets and `egui_wgpu::CallbackTrait`. Mesh normals are derived per triangle in
  the fragment shader, avoiding an expanded position/normal mesh.
- The callback renders into its own depth-backed texture and composites it inside
  egui. Unchanged scenes reuse the previous frame, including during status updates.
- `preview/` captures the engine's scheduled paths as export requests batches of
  32 layers. Original toolpaths are released after each batch. Lazy tower and priming
  paths are included. A slice's preview never parses G-code; an opened G-code file is
  read into the same records (see "Opening G-code files" below).

Every preview segment is **32 bytes**: two XYZ endpoints, packed micrometre
width/height, and packed role/tool/fan/speed. The file is indexed by per-layer record
offset/count/Z. Only the requested layer/move range is read, in batches of at most
65,536 segments. The reusable GPU instance buffer is **2 MiB**. Upload submissions
are drained every eight batches to bound pending staging memory. No full preview
copy is retained in CPU or GPU memory; cached scene meshes are kept for switching
back to Prepare. The renderer draws instanced boxes with feature/tool palettes or
speed/fan/width gradients. Feature visibility and travel visibility are shader masks. Attributes for the later colour schemes (actual speed, flow, temperature, acceleration) sit in a
parallel `attrs.bin`, 8 bytes per record, read with the same batches; see
[gui-view-tools.md](gui-view-tools.md).

## Verification on 2026-10-16

The executable and screenshot path compile. Camera projection/rays, transformed
nearest-hit picking, concave bed triangulation, all three WGSL shaders (Naga parse
and validation without an adapter), packed segment fields/counts, disk seeking and
batch limits, layer/move ranges, per-tool filament composition, cancellation, temp
cleanup and text/binary motion equivalence have automated tests.
`cargo fmt --all`, workspace clippy with `-D warnings`, the workspace tests and
`cargo xtask provenance` passed. The profile tests need the pinned profiles from
`scripts/fetch-upstream.sh --profiles`.

Release-mode pipeline measurements used Python's `resource.getrusage(RUSAGE_CHILDREN)`
around the real screenshot executable. Geometry remained in the scene while slicing.
Runs used the installed profiles; the dense fixture is generated by
`scripts/gui-dense-fixture.py`, with no downloaded data.

| Job | Triangles | Layers | Preview records | Disk bytes | Peak process RSS |
|---|---:|---:|---:|---:|---:|
| CORE One Benchy | 225,102 | 240 | 93,976 | 3,007,232 | 96.8 MiB |
| CORE One dense sphere | 3,800,000 | 300 | 180,615 | 5,779,680 | 483.5 MiB |
| XL 5T, two cubes on tools 1/2, tower enabled | 24 | 100 | 14,799 | 473,568 | 44.2 MiB |

**These are CPU/pipeline peaks, not a whole-window/GPU memory measurement.** The dense mesh's
indexed GPU geometry is approximately 65 MiB; that is a size calculation, not an observed GPU
allocation.

The reference PrusaSlicer 2.9.6 sliced Benchy with the CORE One HF0.4,
0.20mm SPEED and Prusament PLA profiles. Mica CLI still produces 240 layers and the
same totals as the GUI worker (rounded console display):

| CORE One Benchy | Filament | Normal time |
|---|---:|---:|
| PrusaSlicer 2.9.6 | 3,865.84 mm | 38m 28s |
| Mica CLI / GUI pipeline | 3,851.75 mm | 38m 29s |

Filament differs by −0.36%; time differs by +1 second. Reference and Mica outputs
were written under `/tmp`, not checked in. No Orca multi-tool oracle was launched.
This GUI change does not alter slicing algorithms.

Screenshots of the same cases:

```sh
export PATH="/opt/homebrew/opt/rustup/bin:$PATH"
cargo build --offline --release -p mica-gui
./target/release/mica-gui --screenshot /tmp/mica-prepare.png tests/models/3DBenchy.stl
./target/release/mica-gui --screenshot /tmp/mica-preview.png tests/models/3DBenchy.stl --preview
./target/release/mica-gui --screenshot /tmp/mica-xl.png tests/models/cube20.stl tests/models/cube20.stl --preview --xl --color tool
python3 scripts/gui-dense-fixture.py /tmp/mica-dense.stl
./target/release/mica-gui --screenshot /tmp/mica-dense.png /tmp/mica-dense.stl --preview
```

## Scope and close review

This is the GUI foundation, not the whole GUI. Arrangement uses bounding-box
rows, with no packing, collision avoidance or concave-bed containment. Import has the
same limitations as the CLI: project settings, modifiers/painting and multi-plate
semantics are reported but not applied. There is no profile editor, project save,
undo/redo, cutting, texture/model bed asset, printer upload or exact emitted-move
simulation yet.

Preview shows extrusion centerlines from the engine, plus direct connections between
paths as optional travels. Travel routing, Z-hop, wipe/retract moves, scarf modulation,
arc fitting and template-generated paths are not simulated. Tower geometry is appended
to each layer's preview; the move slider is preview-record order, not the precise
interleaved machine execution order. Speed is nominal role/volumetric-limited speed;
fan is the profile/bridge value before cooling and dynamic fan processing. Both are
labelled in the legend. Width is the toolpath flow width. The preview appears after
slicing; progressive display during slicing is deferred. Very large visible ranges
are streamed and can take longer to redraw, although retained memory stays bounded.

Review the custom callback/pipeline integration on real Metal hardware, interaction
and layer-slider layout, per-tool profile vectors, cancellation stage latency, and
the binary metadata path most closely. There are no upstream GUI ports or newly
bundled profile assets.

## Opening G-code files

File › Open, drag and drop and the Library's Open accept `.gcode` and `.bgcode` (also `.gco`, `.g`).
The file opens read-only over the scene (`crates/mica-gui/src/gcode_doc/`): Preview shows it with a
"Viewing G-code file" chip and Close, the title and header name it, Prepare, slicing and every
editing command are off, and Close returns to the plate and its Preview state untouched (nothing
is discarded, so no unsaved-changes prompt is needed; quitting still asks about the plate).

Reading happens on a thread with a progress bar in the header (Cancel stops it), in two streaming
passes that never hold the file: a metadata scan (header and configuration comments, binary
metadata blocks, `M862.3` / `M862.1` checks, the `;TYPE:` names in use, the statistics footer, `M73`)
and the move pass, which writes the same 32-byte `preview.bin` records and 8-byte `attrs.bin`
attributes a slice writes, one layer buffered at a time (at most 262,144 records). A `.bgcode` is
first unpacked block by block into the job folder. Feature roles come from PrusaSlicer's and Orca's
`;TYPE:` names; `Custom` and unknown or missing types share one role slot the file does not
otherwise use, labelled `Custom` or `Unknown`. Layers follow `;LAYER_CHANGE` / `;Z:` (the start
G-code's purge belongs to the first layer), or, without tags, a change of extrusion height. Widths
and heights come from `;WIDTH:` / `;HEIGHT:`, else from the extruded volume and the layer step. Arcs
(`G2`/`G3` with `I`/`J`) are cut into chords as Marlin does. Tools follow `T<n>`, fan `M106`/`M107`
(part fan only), temperatures `M104`/`M109`, accelerations `M204` / `SET_VELOCITY_LIMIT`; actual
speeds are planned per layer as for slices. The print time is the file's own (footer or `M73`), else
Mica's time estimator over the file; per-tool filament and weight come from the footer, else the
moves. Tool colours are `extruder_colour`, else `filament_colour`, else Mica's defaults.

The printer the file was made for comes from `printer_settings_id` / `printer_model` (configuration
block or binary printer metadata), `M862.3`, or Mica's own Snapmaker header; it resolves to an
installed preset (by name, by model code and nozzle variant, or by model name), whose bed the file is
shown on. Print… pre-selects the configured printer that fits (linked, same model, same family) and
warns, without blocking, when the chosen printer is set up as or reports another model. The file's
own bytes are uploaded (PrusaLink, Moonraker, the U1 with its slot map for the tools the file
uses); Export becomes Save copy…, a plain byte copy that refuses the opened file itself. Mica's
validator runs on the file after it opens, against the selected printer's settings when the file is
for it, else the file's printer with Mica's default print and filament profiles; the sidebar card
shows the errors and warnings with the first findings. Warnings never block sending; errors ask
before sending. A file that is not G-code (binary data, prose, no moves, a damaged `.bgcode`) is a
notification with the reason; the file is never written.

Measured on 2026-10-20 (`--profile fast`, headless screenshot, Apple silicon): the 18.6 MB
Snapmaker U1 file `out/mica_cat_plate1_u1.gcode` (748,191 lines, 244 layers, 627,492 records)
shows its preview 0.2–0.6 s after opening (the reading thread; building the default configuration
for another printer's check adds about 0.1 s on the UI thread), the validator finishes about 0.5 s
later, and the whole screenshot process peaks at 150 MB resident against 125 MB for the same run
without a file.

```sh
MICA_CONFIG_DIR=$(mktemp -d) ./target/fast/mica-gui --screenshot /tmp/gcode.png out/plate1_u1.gcode
MICA_CONFIG_DIR=$(mktemp -d) ./target/fast/mica-gui --screenshot /tmp/send.png benchy.bgcode \
    --printer-host 'Mock CORE One=prusalink@127.0.0.1:18083' --print-dialog
```
