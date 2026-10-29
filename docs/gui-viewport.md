# GUI viewport

> Later GUI work restyled this: dark-only default theme with IBM Plex, no shadows, the
> printer's real bed sheet and texture, bed-level rotation ring, preview legend/slider cards.
> Where this file and the app disagree, the app is current.

The 3D view of `mica-gui`: theme, bed and plates, model rendering, gizmos, camera and the
G-code preview. Everything here is original Mica code (no upstream GUI ports, no bundled
vendor textures or models); it builds on the foundation in [gui-foundation.md](gui-foundation.md).

## Modules

| Module | Owns |
|---|---|
| `theme.rs` | One palette (teal accent, neutral ramp) for egui and the wgpu passes; dark by default, follows the system, `theme::apply(ctx, Mode)` is the toggle hook; `overlay_frame` for floating panels |
| `scene/bed.rs` | `Bed::from_config`: `bed_shape` polygon (rectangular or custom, normalised CCW), `max_print_height`, printer name, `bed_exclude_area` zones (typed points or Orca extras text); `placement()` classifies object bounds as Inside / Outside / Excluded; `grid()` (10 mm minor, 50 mm major), `volume_edges()`, ear-clip `triangulate`, `outline_strip` |
| `scene/plates.rs` | `PlateLayout` trait (count, current, origin) the editor's plate model implements; `Plates` default grid layout; `snapshot`, `plate_bounds`, `all_bounds`, `plate_at` |
| `scene/mod.rs` | `Object` transform = translate · Rz(`angle`) · `tilt` · scale about the bounds centre; `Delta` (Translate / Rotate / Scale) applied with `Object::apply`; `lay_on_face`, `drop_to_bed`, `face_normal`; `tool_colors(config)` from `filament_colour` / `extruder_colour` |
| `scene/camera.rs` | Orbit camera with perspective/orthographic, `orbit_around(pivot)`, pan, `zoom_at` (cursor-anchored, animated via `animate`), `fit(bounds)`, presets 0–6, `project` |
| `scene/picking.rs` | Ray/triangle picking returning object, face and distance (handles non-uniform scale) |
| `render/` | 4× MSAA colour + depth, single-sample selection mask, composite blit with outline; background gradient, bed (procedural minor grid + axes, hatched exclusions, outline strips), major grid and volume outline as lines, planar shadow, lit meshes, instanced preview tubes |
| `viewport/` | `Viewport::show(ui, Scene) -> Vec<Action>`: input, hover, drags, gizmos (`gizmo.rs`), axis triad (`axes.rs`), vector icons (`icons.rs`), floating toolbar, plate bar and per-plate hover icons, labels, hints, preview overlays |
| `preview/` | 32-byte records on disk (unchanged format), `Stats` accumulated while writing, `View` (layer/move range, feature + tool masks, colour mode, gradient range), legend, vertical double-handle layer slider and horizontal move slider, `nearest_segment` for hover tooltips |

### Viewport ↔ editor protocol

The viewport never mutates scene data. `app.rs` builds a `viewport::Scene` each frame
(objects, selection, bed, plate snapshot, tool colours, optional preview + `View`) and
handles the returned `Action`s: `Open`, `Arrange`, `Orient`, `Select`, `Transform {index,
delta}`, `LayOnFace {index, face}`, `Edited`, `AddPlate`, `SelectPlate`, `PlateArrange/
Orient/Lock/Settings`, `Theme`, `ViewChanged`, `Cut`, `LayerHeight`. Transform deltas are
incremental (snapped increments when Shift is held), so the editor can record them for undo.

## Rendering

- **Lighting:** key light that follows the camera (upper left of the viewer), fill from the
  opposite side, hemisphere ambient (sky/ground from the theme), small specular, rim darkening.
  Normals come from screen-space derivatives and are flipped to face the viewer, so STL winding
  does not matter.
- **Selection / hover:** selected (1.0) and hovered (0.5) objects are drawn into the red
  channel of an RG8 mask without depth; the blit pass samples 16 neighbours and paints an
  accent (selection) or bright (hover) halo. Hovered objects are also brightened in the mesh
  shader.
- **Outside the printable volume:** only the part of an object outside the volume is marked,
  per fragment: the uniform block carries the plate origin, the maximum print height, the bed
  outline and up to six exclusion zones (`render::volume_shape`), and `outside_volume` in
  `common.wgsl` tests each fragment's world position against them (past the outline, inside a
  zone, above the height or below the bed, with `Bed::placement`'s 0.01 mm tolerance). Those
  fragments get fine red screen-space stripes; the rest keeps its colour. A second, depth-tested
  draw of every object writes the visible outside fragments into the mask's green channel, and
  the blit draws the selection-style outline around them in red (also along the line where the
  outside part meets the rest of the object). An object whose skirt or brim does not fit keeps
  the whole-object warning stripes. The plate warning (`scene::outside`) says where and how far:
  "Bracket sticks out 12.4 mm past the back edge and 3.1 mm above the maximum height", with
  Move inside (when it fits on the bed), Scale to fit (when it is larger than the volume:
  PrusaSlicer's `scale_to_fit_print_volume`, one undo step) and Arrange.
- **Shadow:** the mesh is flattened along the key light onto z = 0.03 with `Less` depth so
  overlapping triangles do not stack alpha; one extra draw per object.
- **Bed:** vertices at z = −0.02 with the minor grid and the 40 mm X (red) / Y (green) origin
  axes in the fragment shader; major lines are real geometry (crisp under MSAA); the plate
  outline is a flat strip (0.7 mm, 1.6 mm accent for the current plate); exclusion zones are
  hatched at z = 0; the printable volume is a faint line box.
- **Preview:** six-sided tubes with elliptical cross-section and smooth normals, extended by
  0.3 × min(width, height) at both ends to close joints. Colour modes: feature, filament/tool
  (profile colours), speed, fan, width, height, layer time (per-layer storage buffer looked up
  by binary search on record index). Gradient ranges come from the real statistics. Speeds and
  fans go through the same layer-time slowdown (`mica_gcode::cooling`) as the exporter, so
  they reflect emitted values apart from travel routing. Feature and tool visibility are
  shader masks; the GPU batch buffer stays 2 MiB.

## Interaction

- Press and drag on a model selects it and moves it in one gesture, as in PrusaSlicer and Orca
  (`viewport/drag.rs`): a model of the selection moves the whole selection, any other model is
  selected alone and moved. The press becomes a move only after the pointer has left it by more
  than 5 px along either axis (`MoveThresholdPx`), so a click only selects; each drag is one
  undo step. Locked plates do not move (the drag turns the camera), and while a tool gives the
  drag its own meaning (painting, layer heights, brim ears, measure, cut, lay on face; with the
  Text tool only an already selected model moves) it keeps that meaning.
- Left drag on empty space orbits around the selection (or the current plate centre), like
  OrcaSlicer; with Settings › General › "Orbit around the screen center" it orbits around the
  camera target instead (PrusaSlicer: the middle of the view, which panning and zooming at the
  pointer move; the selection does not matter). Cmd/Ctrl-drag orbits around the point under
  the middle of the view on the bed, taken when the rotation starts and kept until the button
  goes up (Orca's `m_rotation_center`, `viewport/orbit.rs`); while a gizmo (the default Move
  one included) or a tool is on, a Cmd-drag that starts on a model orbits too instead of moving
  or painting it; with none on it still moves the model, as in Orca. Right/middle drag pans,
  wheel zooms towards the cursor with a short animation; `0`–`6` views, `Z` fits the selection
  or plate (`Shift+Z` all plates), `P` toggles perspective, double-click focuses an object.
- `M`/`R`/`S` pick the move/rotate/scale gizmo, `F` toggles lay-on-face, `Esc` deselects.
  Shift snaps to 1 mm / 5° / 5 %. The gizmo shows live numbers next to the pointer.
- The axis triad in the corner is clickable (positive and negative tips snap to the six views).
- A hint line above the plate bar explains the current mode.
- The plate bar (`viewport/plate_bar.rs`): `‹ name  2 of 3 › ⋯ | +`. Clicking the name (or F2,
  or "Rename plate…") edits it in place: Enter or clicking elsewhere keeps it, Esc cancels,
  an empty name is `Plate N` again (unnamed plates follow their position and are saved with
  an empty `plater_name`). "2 of 3" opens the plate list: click a row to show that plate,
  drag rows to reorder. `⋯` opens the plate menu (`plate_menu.rs`): rename, plate settings
  (the sidebar's "This plate" scope), arrange / auto-orient / select the plate's objects,
  lock, duplicate, move plate left / right, reorder, delete. The same menu opens on a
  right-click of the bar, of a plate's bed or its corner label, and of the object list's
  plate header; the hover strip by a plate's label keeps its shortcuts plus `⋯`. Moving a
  plate is one undo step, keeps its objects (plate-local positions), settings, lock, tower
  and slice, and never re-slices: a plate's G-code does not depend on its position.
- The Prepare toolbar (`viewport/toolbar.rs`) groups its tools as Orca's top bar does:
  add / arrange | move / rotate / scale / lay on face | cut / mesh tools / text | painting /
  layer heights / brim ears | measure / section. The mesh tools (boolean, simplify, repair)
  and the four painting tools sit in fly-outs; while one of their tools is open the fly-out's
  button shows that tool's icon, highlighted. The toolbar never runs under the object list or
  the tool card: of five layouts, from everything in sight to a single menu button, it draws
  the widest that fits between them (at 1280 px wide with the default sidebar: add, arrange,
  the gizmos, cut, mesh, paint and a "more" menu; from about 1440 px everything). Tooltips
  and menu rows show each tool's shortcut, taken from `keymap.rs`.
- Preview: legend top-left (colour scheme, rows with time and share, click to hide), layer
  slider on the right (Z, layer number and layer time at the top handle, tool-change ticks,
  arrow keys), move slider at the bottom with the tool-head marker, hover tooltip for the line
  under the pointer (feature, layer, speed, fan, width × height, filament). The hover search
  only scans the last 40 000 visible records and only when the pointer moves.

## Rotate gizmo vs PrusaSlicer

Compared with `src/slic3r/GUI/Gizmos/GLGizmoRotate.cpp/.hpp` at prusaslicer@b028299c77 (Orca's
copy at orca@8500fcdcca is the same algorithm: it only colours the grabber with the axis
colour and hover colour, draws the centre-to-grabber line grey when idle, prints the tooltip
angle with 2 instead of 4 digits, switches the coordinates to World when the gizmo opens, and
does not offer the rotate tool for the wipe tower at all).

### Upstream

- **Three full rings, one code path.** `GLGizmoRotate3D` holds three `GLGizmoRotate`s that
  differ only by axis. Each ring is a whole circle in its own local frame, `m_orient_matrix ·
  local_transform`: Z is the identity (angle 0 at +X, +90° at +Y), X is `Ry(90°)·Rz(−90°)`
  (angle 0 at −Y, i.e. the front, +90° at −Z) and Y is `Rz(−90°)·Ry(−90°)` (angle 0 at +Z,
  the top, +90° at +X). In every frame u × v is the rotation axis, so a positive angle is a
  right-handed turn about it. Both halves are drawn (depth-tested against each other; the
  depth buffer is cleared first, so the gizmo is always over the model).
- **Size and centre.** All three rings share the selection's bounding sphere: centre = sphere
  centre, radius = sphere radius + `Offset` (5 mm). The sphere is rotation-invariant, so rings
  do not grow while turning; they are world-sized (they zoom with the scene). They are
  re-initialised every frame except while the grabber is hovered or dragged, i.e. frozen
  during a drag.
- **World vs local.** `m_orient_matrix` is the rotation of the bounding box "in the current
  reference system": World (identity, the default; Orca forces it) or Object / Part (the
  object's rotation, the rings then lie in the object's planes). The applied transformation
  follows the same choice (`World/Instance/Local_Relative_Joint`); Alt rotates each instance
  about its own centre.
- **Drag mapping (`mouse_position_in_local_plane` + `on_dragging`).** The mouse ray is moved
  into the ring's local frame and intersected with its plane z = 0 (as a line, so a plane
  behind the camera gives a mirrored point). θ = the angle of that point from local +X
  (acos + cross sign, 0..2π). The gizmo's angle **is** θ: `data_changed` resets every ring's
  angle to 0 when a drag starts, so the rotation applied is the absolute pointer angle in the
  frame, i.e. relative to the orientation at the drag's start. Edge-on: only when the local
  ray is parallel to the plane within `EPSILON` (1e-4, on the unnormalised ray) does it fall
  back: angle 0 if the ray runs along the grabber direction, else the near (or far) end of the
  mouse ray projected into the plane. Nearly edge-on rays use the plain intersection, which can
  be very far away or behind the camera.
- **Snapping.** By the pointer's distance `len` from the centre in the plane: R/3 ≤ len ≤ 2R/3
  snaps θ to 45° (`SnapRegionsCount` = 8); R ≤ len ≤ R·1.1 (the tick band, `ScaleLongTooth`)
  snaps to 5° (`ScaleStepsCount` = 72, `ScaleStepRad`); anywhere else θ is free. Snapping is
  on θ, i.e. relative to the drag's start orientation. No modifier for fine steps.
- **Grabber.** One per ring, a box at R·1.15 (`GrabberOffset`) at the current angle with two
  cone extensions along the ring's tangent (±local Y); it is the only pick target (raycaster),
  the ring line itself is not. Its idle colour is the axis colour (Orca: plus a hover colour);
  the ring line turns to the drag colour while its grabber is hovered or dragged.
- **Rendering while hovered or dragged.** Only that ring is rendered (`on_render` hides the
  other two as soon as one has `m_hover_id`). It shows the 72-tick scale (long ticks every 10°:
  0.1R, short 0.05R), the eight snap spokes from R/3 to 2R/3, the reference radius (centre to
  angle 0 at 1.15R), the angle arc from 0 to the current angle at 1.15R in the axis colour,
  and the line from the centre to the grabber. The value is a tooltip "X: 45.0000" while
  hovered or dragging. Idle, only the three circles, the centre lines and the grabbers show.
- **Undo.** `do_rotate("Gizmo-Rotate")` on release: one snapshot per drag; the object is put
  back on the bed then (not during the drag).

### Mica before this change

Only Z worked like that. The Z ring sat flat on the bed outside the footprint, with ticks,
snap zones, spokes and one handle. X and Y were small half arcs (0.75 × 80 px, constant on
screen) around the pivot, only the camera-facing half drawn and hittable, no ticks, no handle,
no label; the snap zones were measured against that tiny radius, so a pointer a little away
from the arc was "outside" and the drag was mostly free, and the angle came from a ray–plane
intersection without any edge-on handling (an edge-on arc returned nothing and the drag
stopped). That is what felt "weird": nothing to grab, no scale, and tilts that jumped or
stalled depending on the view.

### Mica now (`viewport/gizmo.rs`)

- One `Ring` type for all three axes, frames as upstream's `local_transform` (X handle starts
  at the front, Y on top, Z at the object's yaw), world axes (upstream's default; `Layout::frame`
  takes an object rotation for local axes, not exposed in the UI yet). Full circles, back half
  dimmed. While one ring is dragged the others are hidden; while one is hovered the others fade.
- Pointer → angle: ray into the ring frame, intersect its plane, atan2 (`Ring::local_point`).
  Exactly edge-on (|cos| < 1e-4) or with the plane behind the camera, the ray point nearest the
  centre stands in (upstream: the near / far end of the mouse ray). That point lies on the line
  the edge-on ring appears as, under the pointer, so the handle still follows the pointer and
  never mirrors.
- The sweep is measured from where the ring was pressed (Mica lets you grab the ring anywhere,
  not just the handle, and pressing must not turn anything), and snapping applies to the
  handle's absolute angle in the ring frame: for X / Y the handle rests at 0 each drag, which is
  exactly upstream's relative snapping; for Z it rests at the yaw, so Z keeps snapping to the
  absolute bed angles as before.
- Snap zones as upstream (R/3..2R/3 → 45° with spokes, tick band → 5°, elsewhere free) with
  Mica's wider tick band 0.88R..1.13R so dragging the handle, which sits on the ring, snaps to
  5° (upstream's grabber sits outside the band at 1.15R and drags freely); Shift = 1° anywhere.
  Ticks every 5° in three lengths (5° / 15° / 45°).
- Size: the bed ring is the footprint radius + 22 px; the X / Y rings are the radius of the
  sphere around the pivot (`Object::pivot_radius`, invariant under any rotation about the pivot;
  upstream: minimal bounding sphere) + 22 px; never smaller than 80 px. Frozen while dragging,
  as upstream. Because Mica drops the object to the bed on every step, a tilt can move the
  object's centre during a drag while the ring stays where it started.
- Hit testing: the handles (weighted ×0.5) and the closest approach of the ray to each ring's
  line (sampled, so an edge-on ring is as easy to grab as the line it appears as). The prime
  tower gets only the bed ring (upstream disables X / Y for the wipe tower).
- While dragging: ticks, spokes and the snap-zone circles, the reference radius at the drag's
  start, the radius to the handle and the swept arc; the label next to the handle reads
  `X +30°  snap 5°` (sweep of this drag for X / Y; the absolute yaw for Z, which can still be
  typed after a double-click). Handles are yellow when hot and show a two-way arrow along the
  ring's tangent (upstream's cone extensions).
- Undo: one step per drag, as before.

## Headless screenshots

```sh
cargo build --offline --release -p mica-gui
./target/release/mica-gui --screenshot out.png model.stl [more.stl] [--preview] [--xl]
    [--color feature|tool|speed|fan|width|height|layertime] [--view iso|top|bottom|front|rear|left|right]
    [--select N] [--hover N] [--gizmo move|rotate|scale|none] [--drag-ring DEG[,RATIO[,x|y|z]]]
    [--lay-on-face] [--plates N]
    [--light] [--ortho] [--layer N] [--moves N] [--pointer X,Y] [--offset X,Y] [--size WxH] [--no-ui]
```

egui runs without a window (`Context::run_ui` for three passes so floating areas know their
size and hover state settles) and its shapes are drawn with `egui_wgpu::Renderer` into the
same texture as the viewport callback, so the PNG shows exactly what the app shows.
`--pointer` injects a pointer position (hover highlights, preview tooltips), `--offset` moves
the first object (out-of-bed warnings).

## Memory

Nothing new is retained per segment: the preview file format is unchanged; `Stats` is a few
hundred bytes; the layer-time storage buffer is 8 bytes per layer. The MSAA colour and depth
targets add 4 × 2 × 4 bytes per viewport pixel (≈ 40 MB at 1400 × 900 on a 2× display).
Headless renders of the Benchy preview peak around 110–125 MB RSS including slicing.

## Tests

`cargo test -p mica-gui` covers: hex colour parsing and palettes; bed from `bed_shape`
(size label, grid line counts and placement on multiples of the step, centre-origin beds,
exclusion zones from typed points and extras text, placement classification, triangulation,
outline strips, CW normalisation); plate grid layout and lookup; object deltas, lay-on-face
orientation and resting on the bed, filament colours per tool; camera projection/ray round
trips in both modes, cursor-anchored zoom, orbit about a pivot, fit; picking with
non-uniform scale; gizmo hit tests, axis/plane/ring/scale drags with snapping and angle
unwrapping, the ray → ring-angle mapping for X / Y / Z rings from several camera positions,
near and exactly edge-on, in world and turned frames; legend statistics (time, percentages, ranges, tool changes), time formatting,
gradient; slider mapping and handle ordering; preview cooling slowdown, nearest-segment
search, record/layer lookups; screenshot argument parsing; all WGSL shaders validate with Naga.

## Not done (at the time of this part)

- Plates: `Plates` is a stand-in grid layout owned by `app.rs`; wire the editor's plate model
  to `scene::plates::PlateLayout` and handle `PlateLock` / `PlateSettings` / `PlateOrient`.
- Auto-orient is a placeholder (lays the largest face down). Cut and variable layer height are
  disabled toolbar icons.
- No undo/redo; `Action::Edited` marks the end of a drag for that purpose.
- The hover tooltip and tool-head marker read single records from the preview file each frame
  the pointer moves; fine on SSDs, could be cached if it ever shows up in profiles.
