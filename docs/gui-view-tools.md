# GUI view tools: measure, section view, preview colour schemes

Three viewer features on top of [gui-viewport.md](gui-viewport.md). Provenance is in
[`PROVENANCE.md`](../PROVENANCE.md) ("M6 GUI, view tools").

## Measure (U)

The ruler in the Prepare toolbar, or **U** (Orca's and PrusaSlicer's key, `WXK_CONTROL_U`).
Hovering a model highlights the feature under the pointer, as PrusaSlicer's measure gizmo finds
it (`measure/features.rs`, ported from `Measure.cpp`):

- **Flat face**: facets flood-filled by normal (within 0.001 per component).
- **Edge**: a border edge of the face, within 0.5 mm of the pointer; near either end (10 % of
  its length, 0.025–0.5 mm) the **point** at that end.
- **Circle**: a whole border that fits a circle (more than 8 points, RANSAC error < 0.05 mm), or a
  circular stretch of a border subtending at least ~90°. Holes and round bosses show up as
  circles; a regular 5–8-gon gives its edges plus a centre marker.

Click picks the first feature (teal), a second click the second (magenta); clicking a selected
feature drops it (the second moves up), a third feature replaces the second. **Shift** picks a
point on the feature instead, clicking the small centre dot of a selected circle or polygon
measures from its centre (and back). **Delete** restarts, **Esc** unselects, then leaves.

The card (bottom left) lists the selections (edge length, circle diameter) and the results:
angle, distance (perpendicular / direct when they differ), distance XYZ for two points, diameter
and radius of a single circle. Each row has **Copy**, which puts `label: value` on the clipboard
(as upstream). Dimension lines with arrow heads and labels, and the angle arc with its extension
lines, are drawn in the view. Features are computed per object in world coordinates and cached
until the object moves; two features may come from different objects.

Not ported: "edit to scale" (typing a distance scales the object), the debug dialog, inches.

## Section view

The plane-cut box in the Prepare toolbar, and the same button in the small toolbar at the top of
Preview. Each mode has its own plane. It starts facing the camera through the middle of the
current plate's objects and cuts away the half on the viewer's side.

- **Prepare** cuts the objects in the shaders (fragments beyond the plane are discarded, volumes,
  paint and the selection halo too) and **fills the cut faces** with a hatched cap in the
  object's colour (`mica_core::mesh_cut::section_cap`, cached per object, transform and plane).
- **Preview** cuts the toolpath tubes the same way (the layer slider still cuts horizontally);
  lines wholly on the cut side are not hovered.

Drag the arrow to move the plane along its normal, the ball at its tip to tilt it (Shift snaps to
the nearest axis within 10°). The card has a position slider (0–100 % across the scene), Face
camera (PrusaSlicer's "reset direction"), X / Y / Z presets, Flip, Reset and Close.

## Preview colour schemes

"Colour by" now offers PrusaSlicer's / Orca's view types (`libvgcode::EViewType` order):

| Scheme | Value | Source |
|---|---|---|
| Feature | role colours, table of time / share / filament per feature, click a row to hide | records |
| Layer height, Line width | path cross-section | records |
| Speed | commanded speed after layer-time slowdown | records (exporter's emitted speeds) |
| Actual speed | mean speed of each line after acceleration and junction limits | `attrs.bin` |
| Fan | fan % after cooling | records |
| Temperature | nozzle temperature (first layer, filament, calibration bands) | `attrs.bin` |
| Volumetric flow | cross-section × commanded speed, mm³/s | `attrs.bin` |
| Acceleration | the `M204` / `SET_VELOCITY_LIMIT` value per feature; "not emitted" when the profile sends none | `attrs.bin` |
| Layer time, Layer time (log) | per layer after slowdown; the log view maps `ln t` | per-layer buffer |
| Filament | tool colours | records |
| Colour print | colour segments of the plate's colour and tool changes | the plate's custom G-code items |

Gradient legends show the real range with five ticks (log-spaced in the log view) and a plain
note about what the value means; Acceleration says so when the printer gets no acceleration
commands.

**Actual speed** runs a firmware-style planner over each layer's records in order (travel
included): classic per-axis jerk junctions (or junction deviation when
`machine_max_junction_deviation` is set, or Klipper's square corner velocity), forward and
backward passes, trapezoid time per line; the commanded acceleration capped by
`machine_max_acceleration_extruding` / `_travel`. It is an estimate: E-axis jerk, minimum segment
time, arc fitting and travel routing are not simulated.

**Colour print** comes from the plate's colour changes (and tool changes): the per-layer custom
G-code items added with the layer slider's "+" (see [custom-gcode.md](custom-gcode.md)).
`preview::color_print::ColorPrint` packs a palette index per tool into each layer's value and
the legend lists the colour segments with their times; without changes the scheme shows the
filament colours. In the shader it is mode 12 of the `ColorMode` numbering.

### Data and memory

Nothing is re-sliced or re-parsed when the scheme changes. The 32-byte record format is
unchanged; a parallel `attrs.bin` holds 8 bytes per record (`preview::Attr`: actual speed
0.1 mm/s, flow 0.01 mm³/s, temperature 0.1 °C, acceleration mm/s², all `u16`), written by the
same builder while the exporter streams layers (one layer is buffered for the planner). The
renderer reads both files batch by batch into a second 512 KiB instance buffer next to the 2 MiB
record buffer; colour print reuses the per-layer value buffer (packed palette indices). Benchy: 93,976 records → 752 KB extra
on disk, nothing extra kept in memory.

## Headless screenshots

```sh
mica-gui --screenshot out.png model.stl --preview --color-by actual-speed
mica-gui --screenshot out.png hole_block.stl --measure 462,413 --measure 388,372,shift --pointer 600,330
mica-gui --screenshot out.png model.stl --section 0,-1,0.3,0.5 [--preview]
    [--zoom F] [--orbit YAW,PITCH] [--measure-on]
```

`--measure` clicks a feature at a screen point after the first pass (`,shift` for a point on
it) and prints the result rows; `--zoom` / `--orbit` adjust the camera after framing.

## Tests

`cargo test -p mica-gui`: plane, edge, point, circle and regular-polygon detection on a cube, a
cylinder, a hexagonal prism and `tests/models/hole_block.stl`; RANSAC; every measurement pair
(points, edges, perpendicular vs direct, edge / plane / circle angles, parallel and crossing
circles); the selection rules, rows and clipboard text; the section plane equation, box outline
and caps (area and cache); the planner (trapezoid / triangle times, straight vs zig-zag, junction
deviation), attribute packing, emitted accelerations, volumetric flow; legend ranges for every new scheme, tick values (linear / log) and labels. `mica-core`:
`section_cap` area, orientation and misses. The monkey adds measure clicks (finite results),
section planes in both modes (caps on their plane), the U key and colour schemes.
