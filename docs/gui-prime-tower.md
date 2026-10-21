# GUI: the prime tower on the bed

In OrcaSlicer the prime tower is an object you can move around on the bed. In Mica it is one too. On a multi-tool printer with the tower enabled, every
plate that uses more than one tool shows its prime tower (Snapmaker / Orca naming) or wipe tower
(Prusa naming) as a translucent box of its real footprint and height, with the brim, cone and
U1 ribs as a flat slab under it. The box is selected, dragged, nudged and turned like an object,
its position is kept **per plate**, saved into projects and used by the slice. Code:
`crates/mica-core/src/wipe_tower/estimate.rs` (size), `crates/mica-gui/src/tower.rs`
(placement, clamping, overlap, free spot, picking, the `App` side), `panels/tower.rs` (card and
sidebar scope), and the viewport / object list / arrange hooks.

## What you see and can do

- **The box.** Before a slice it is the planner's *estimate*: `wipe_tower::estimate` plans a
  synthetic job, every tool the plate uses (object and part tools, plus painted colours) on
  every layer up to the tallest object, with the real `TowerPlan` and the plate's configuration,
  so the depth, brim, cone and rib footprint come from the same code that prints them. After
  the slice the box is the sliced job's **exact** plan (`Finished.tower`,
  `TowerPlan::shape(true)`); a change drops the slice and the box goes back to the estimate.
  The label under the box says `Wipe tower · 60.0 × 13.2 mm · 48.0 mm tall · estimated`.
  Orca's own GUI uses a volume / area formula (`PartPlate::estimate_wipe_tower_size`); Mica's
  estimate is within 15 % of the exact tower on the XL and U1 cases and equal in width.
- **Select and move.** Click the box or its brim to select it (accent outline, tool card, the
  tower row in the object list). Drag it on the bed (Shift snaps to whole millimetres), use
  the Move gizmo's arrows and plane, the arrow keys (1 mm, Shift 10 mm; objects now nudge the
  same way), or type X / Y in the card. The Rotate ring turns it about its centre
  (`wipe_tower_rotation_angle`); double-click the handle to type an angle. Prusa towers and
  Orca's `WipeTower2` (every non-Bambu printer, the U1 included) turn; a Bambu type1 tower would
  not, but Mica does not plan those anyway. Scale does nothing to a tower.
- **Stays on the bed.** Every move is clamped so the complete footprint (brim included) stays
  inside the printable area and is pushed out of exclusion zones by the shortest axis move
  (`tower::clamp_shift`). A tower that is wider than the bed in an axis is left alone and
  flagged.
- **Overlaps warn and block.** `tower::problems` checks the footprint hull against every
  object's footprint hull (`arrange::polygons_overlap`): the box turns amber, a floating
  `! Overlaps CAT` label appears, the object list lists `Prime tower overlaps cat`, the card
  says slicing is blocked, and Slice refuses with `Prime tower overlaps cat: move it before
  slicing`. OrcaSlicer refuses to slice too; Mica says why and offers the fix.
- **Free spot.** The card's *Free spot* (also in the right-click menu and `--command
  tower-free`) puts the tower in a bed corner, else along an edge, else on a 10 mm grid, 2 mm
  from the edge and the arrange spacing from every object, preferring the spot nearest to
  where it stood. **Arrange** reserves the tower's real footprint (it used to be a square of
  its width) and, when the tower still collides after packing (it stood off the bed, or
  objects were fixed around it), moves it to a free spot itself.
- **Per plate.** `Plate.tower_placement` holds `[x, y, rotation]`; a plate without one shows
  the configuration's position (Orca `wipe_tower_x/y[i]`, Prusa `wipe_tower_x/y`, defaults
  15/220 and 180/140). An explicit `wipe_tower_x/y/rotation_angle` edit in the process
  settings used to win over the plate; the first move of a tower writes that edit into every
  plate's placement and drops it, so the plates govern from then on
  (`App::set_tower_placement`). `File › Save` writes the placement per plate in both flavours
  (Orca `wipe_tower_x/y` vectors, Prusa `Prusa_Slicer_wipe_tower_information.xml`); reopening
  in Mica, Orca or PrusaSlicer keeps it.
- **Undo.** One drag, one nudge, one typed value or one turn is one undo entry (`Move prime
  tower`, `Turn prime tower`); *Free spot* and *Disable* are entries too.
- **Settings.** The card's `Prime tower settings…` (also the list row's menu, or
  `--command tower-settings`) switches the sidebar to *Settings for Prime tower*: the tower's
  rows of the process settings (enable, width, brim width, stabilisation cone, purge line
  spacing and extra flow, bridging, no sparse layers, tower filament), the purge volumes per
  tool (`filament_minimal_purge_on_wipe_tower`, and Prusa's printer-level
  `multimaterial_purging`), and on Orca printers the prime volume, wall type (rectangle, cone,
  rib), rib width, extra rib length and rib fillets, which the option tables do not type and
  are edited as the text extras they are. These are process settings and apply to every plate.
- **Not an object.** The tower row sits under the plate's objects with a tower icon and its
  size (`35 × 14 mm · est.`); it has no tool, no instances and no Delete: the menu offers
  *Disable prime tower* (`wipe_tower = 0` in the process settings, undoable), *Move to a free
  spot* and *Prime tower settings…*.

## Engine side

`TowerPlan::new` now splits into `plan` (the plan) and the bed check, so the estimate can size a
tower that still has to be moved onto the bed. `TowerPlan::shape(exact)` and
`TowerPlan::brim_width()` (the quantised brim `first_layer_hull` already used) are new;
`estimate(config, tools, height)` plans whole layers only (a thin last layer would need more
purge rows and, as the depth propagates downwards, deepen the whole tower, the same effect the
support-layer fix removed). `FacetStates::states()` lists the painted states without walking
the mesh, so painted colours count as used tools. The CLI's G-code is byte-identical to the
binary built before this change for cube20 and 3DBenchy on the CORE One, the XL two-cube tower
job and plate 1 of the private multi-tool U1 project.

## Verification: the slice prints where the box stands

The headless run prints the drawn footprint (`Tower:` line); the CLI slices the same job and
the tower's extrusion centreline bounds (`;TYPE:Prime tower` / `;TYPE:Wipe tower` XY moves with
positive E, brim and ribs included) are compared. Required: ±0.1 mm.

| Job | Box as drawn (footprint X / Y, mm) | G-code tower extrusion bounds (mm) | Difference |
| --- | --- | --- | ---: |
| U1 cat plate 1, project placement X 165 Y 233.362 (`gui11-cat-tower-selected`) | 159.055–205.945 / 225.887–254.337, body 35 × 13.5 | 159.055–205.945 / 225.887–254.337, 181 tower layers, Z 0.25–36.25 | 0.000 |
| U1 cat plate 1 moved to X 120 Y 140 (`--tower 120,140`; CLI `--set wipe_tower_x=120 --set wipe_tower_y=140`) | 114.055–160.945 / 132.525–160.975 | 114.055–160.945 / 132.525–160.975 | 0.000 |
| XL two 20 mm cubes on tools 1/2, X 180 Y 140 (estimate test) | 176.8–243.2 / 134.5–149.7, body 60 × 4.25 | 176.800–243.200 / 134.535–149.715, 101 layers, Z 0.2–20.2 | < 0.1 |

The cat's estimate before slicing says 48.85 mm tall (the tallest cat); the exact box after the
slice is 36.25 mm (the last tool change), the same 35 × 13.5 mm body and the same footprint.
The U1 profile's `0.20 Standard` / `Snapmaker PLA @U1` / High Temp Plate CLI job and the
GUI's project-settings job agree on the footprint; the GUI uses the project's own settings.
`mica-gui::tower::sliced_tower_stands_where_the_box_was_drawn` checks the same thing in the
test suite: the sliced job's `Finished.tower` has the plate's placement, the estimate's width
and footprint X extents, and replaces the estimate on the plate.

## Screenshots (`out/work/gui11-*.png`)

```sh
B=./target/release/mica-gui; F="$MICA_PRIVATE_MODELS/cat.3mf"   # the optional private multi-tool test project
U1="Snapmaker U1 (0.4 nozzle)"
$B --screenshot out/work/gui11-cat-tower.png "$F" --printer "$U1"
$B --screenshot out/work/gui11-cat-tower-selected.png "$F" --printer "$U1" --select-tower --gizmo move
$B --screenshot out/work/gui11-cat-tower-overlap.png "$F" --printer "$U1" --tower 120,140 --select-tower --gizmo move
$B --screenshot out/work/gui11-cat-tower-settings.png "$F" --printer "$U1" --command tower-settings
$B --screenshot out/work/gui11-cat-tower-sliced.png "$F" --printer "$U1" --preview
$B --screenshot out/work/gui11-xl-tower.png tests/models/cube20.stl tests/models/3DBenchy.stl --xl --select-tower --gizmo rotate
```

New flags: `--tower X,Y[,R]` (the current plate's placement), `--select-tower`, and the
commands `tower-free`, `tower-disable`, `tower-settings`. The headless run prints a `Tower:`
line with the drawn placement, body, height and footprint (estimated, and exact after
`--preview`).

## Tests

- `mica_core::wipe_tower::estimate`: estimate within 15 % of the exact plan (XL two and three
  tools, U1 four tools), the documented XL footprint (X 176.8–243.2, Y 134.5–149.7, 60 × 4.25
  body), the U1 four-tool body with ribs and brim (35 × 13.5 body, 46.89 × 28.45 footprint),
  no tower for one tool or a disabled tower, an off-bed estimate that can be translated, whole
  layers and a turned footprint.
- `mica-gui::tower`: clamping to the bed and out of exclusion zones, overlap detection naming
  the object and a free spot that clears it, picking the body and the brim (also turned),
  rotation support by tower type, and the app round: two cubes on two XL tools show a wipe
  tower; a drag is one undo entry (undo / redo), an off-bed drag clamps, typed values and
  nudges are entries, rotation keeps the centre, a second plate has its own position, an
  overlap warns and blocks slicing until *Free spot*, arrange keeps the objects clear of the
  footprint, *Disable* writes `wipe_tower = 0` and is undoable; a settings edit of the
  position becomes the plates' placement; the sliced job's exact tower has the placement of
  the box and the plate draws it; painted colours give the painted cube a tower.
- `screenshot`: the new flags parse.

## Left for later

- The estimate assumes every used tool prints on every layer; a print that changes tools only
  low down gets a taller box than its tower (the exact box replaces it after the slice).
- Picking uses the body box and the footprint outline; a rotated tower's outline halo is the
  box's.
- Circular or concave beds are clamped by their bounding box (the engine's own bed containment
  is rectangular too); the exclusion-zone push is axis-aligned on zone bounding boxes.
- Orca's `wipe_tower_x/y` per-plate vectors are written for every plate; PrusaSlicer stores one
  position (bed 0).
