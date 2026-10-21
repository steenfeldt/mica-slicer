# GUI: everyday object operations

Copy / paste / duplicate, instances, multi-selection, the right-click menus, modifiers,
negative volumes, support volumes and height ranges in the 3D view, a packing arrange, a real
auto-orient and the cut tool, on top of [gui-sidebar.md](gui-sidebar.md) and
[gui-painting.md](gui-painting.md). Every action is one undo entry; nothing is modal.

Previously a multi-plate project also flagged objects on the other plates as "Outside
the printable area": the viewport and the renderer guessed an object's plate from its
bounding-box centre (falling back to the first plate) and clamped the warning label of an
off-screen object into the view, next to whatever sat at the edge. `Scene::plate_of` /
`Frame::plate_of` now carry each object's plate, `scene::plates::placement_on` checks against
that plate's bed, and warning labels are only drawn while their anchor is in view
(`gui9-plates-fixed*.png`; the two objects still flagged are the ones Orca left outside every
plate).

## Objects, parts and their settings (tree, scope bar, intents)

The floating object list is gone; everything about objects and their parts lives in one tree
at the top of the Prepare sidebar, settings are overridden by editing them, and parts are
added by what they are for (`gui-objects-*.png`).

- **One tree** (`panels/tree.rs`, model in `tree.rs`): `PLATE 1 · 2 OBJECTS` with **+ Add**
  and a fold button; the list scrolls inside a height dragged with the handle under it (kept
  in `mica.toml` as `window.tree_height`), so the settings below keep their room. An object
  row has its filament swatch, name, chips with what its own overrides change (`infill 25 %`,
  `+2`) or a muted "profile settings", the repair / skirt / painting badges, the tool chip and
  "+" / "−" instances. Under an expanded object (the selected one by default, the chevron
  folds it) its parts sit in fixed kind groups, shown when not empty, each with a painted icon
  and count: PARTS (model parts), MODIFIERS, NEGATIVE VOLUMES, SUPPORT ENFORCERS, SUPPORT
  BLOCKERS, HEIGHT RANGES; they fold. Every part row says what it changes (`infill 40 % ·
  gyroid`, `0.12 mm layers`) or its kind.
- **User groups** ("Hull details") live inside a kind: a part's menu has *Group* (a new group
  with it, named inline), *Move to group*, *Remove from group*; a group's menu *Rename group*
  and *Ungroup*, a double-click renames. Dragging a part reorders it inside its kind (the
  volume order the engine uses), onto a group joins it, onto the kind header leaves the group.
  Moving a part onto another object is not offered: the scene keeps a volume in its object's
  frame and has no operation to re-home it yet. Groups are a name on the part
  (`Volume::group`, `HeightRange::group`) and are saved in Mica's own sidecar
  (`Metadata/mica.json`: part metadata `mica_group`, object metadata `mica_range_groups`), never
  in the native model settings: PrusaSlicer, OrcaSlicer and older Mica versions ignore them,
  and the G-code does not change (`save::tests::tree_groups_round_trip_in_mica_metadata_only`).
- **Breadcrumb**: with a selection, a chip at the top left of the 3D view reads `3DBenchy ›
  Hull infill`; a click on the object's name goes up to it.
- **Override by editing** (`panels/scope.rs`): with an object, part or height range selected
  the process card shows a scope bar `[Profile | 3DBenchy · 3 | Hull infill · 2]` (a segment
  per level of the selection, with its override count) over the same topic rail and sections
  as the profile. A new selection starts at its deepest level; a click switches. At an object
  or part scope a row that is not overridden shows the inherited value muted behind a dashed
  outline; editing it makes an override at that scope (an edit back to the inherited value
  is none), shown with the changed marker and `3DBenchy · profile: 2 loops` (for a part: `was
  2 loops`) under its label; ↺ removes it. Rail dots, "N for this object" and the Changed chip
  count the scope's overrides; search searches the scope's settings. Only what the level can
  change is listed (`settings::scope_allows`): an object every print setting but the tool
  selectors (as the old "+ Add override" search offered), a modifier the per-region ones
  (not layer heights, supports, raft, brim, skirt, XY compensation, which the engine takes from
  the object), a height range those plus its layer height; an override already there always
  shows. Negative and support volumes have no settings. The "+ Add override" picker is gone.
- **+ Add by intent** (`intents.rs`): on a selected object the menu offers *Stronger here*
  (modifier: walls +2, infill 40 %, gyroid), *Finer layers here* (height range around the
  selected part's heights, else the middle third: layer height 60 % of the current one, at
  least 0.08 mm and the printer's minimum), *Supports here* / *No supports here* (enforcer /
  blocker), *Smooth top here* (modifier with ironing, top third), *Cut a hole* (negative
  volume through the object), then *Empty modifier…* and *Recipes*. The part is a box 40 % of
  the object's footprint across at its centre, named after the intent, selected so the gizmo
  moves it; each is one undo step. The part header in the sidebar has *Fit to selection* (the
  object's bounds), *Change shape…* (box, cylinder, sphere, slab of the same size) and *Save
  recipe* ({kind, shape, overrides} under a name in `recipes.toml` next to `mica.toml`;
  deleted from the Recipes submenu; failures go to the notifications). With a selection "+ Add"
  also offers *Text* and *SVG…* (a raised part on that object); without one it offers Add
  model and Text or SVG.
- **Text and SVG** (`emboss/`): T, the toolbar's Text button, the Add menus and the object's
  right-click *Add text…* / *Add SVG…* put a raised part on the selected model (with nothing
  selected, on the plate's only model; with several, T makes a text object on the bed). The
  new text ("Text") is placed at once on the surface under the view's center and the card
  opens; a click on the model moves the text there. Every change rebuilds the mesh as you type
  (wrapping onto a surface runs in the background); the 3D view uploads a volume's mesh again
  whenever it is replaced. Done (or T) keeps the text; Cancel (or Esc) puts it back as it was
  when the card opened (one undo step), or takes a just-added text away. A text opens again
  from the object list (a click on its row), T with the part selected, a double-click on it,
  the right-click *Edit text*, or *Edit text* in the part header; a click on another text
  while the card is open moves the card to it. Texts read from Mica, PrusaSlicer and Orca
  3MFs (`slic3rpe:text` / `slic3rpe:shape`) edit the same way.
- **Add shape** (`builtin.rs`, gallery `panels/shapes.rs`, models
  `mica_core::builtin_models`): the last entry of "+ Add" (with or without a selection) and
  part of the 3D view toolbar's Add menu (Add model, Text and SVG, then the gallery). A grid of
  small rendered thumbnails (made once on a background thread, kept for the session) with
  names: Benchy (the official #3DBenchy STL, CC0, credited in the tooltip and the Credits tab),
  then the generated shapes: cube, cylinder, cone, sphere, torus, pyramid, wedge (45° ramp),
  tube and disc (Ø 40 × 2 mm), mostly 20 mm. A click puts it on the current plate (the middle
  of an empty plate, else the nearest free spot), selected, one undo step ("Add cube"). With an
  object selected, pills above the grid pick *Object*, *Part* (stands beside the object, joins
  its print), *Modifier* (centred, 100 % infill to start), *Negative*, *Enforcer* or *Blocker*
  (centred in the object); the part keeps its true size whatever the object's scale ("Add
  cube modifier"). Size afterwards with the normal scale tools; there is no parameter card
  (radius, segments) yet. Saved projects store them as plain mesh objects named "3DBenchy",
  "Cube", …, so other slicers open them. Mica MCP's `add_shape` does the same (docs/mcp.md).
  New built-in models (a Mica mascot) go into `BuiltinModel`.
- **Viewport selection**: a click selects the object; with it selected, a click on one of its
  parts selects the part (x-ray: a modifier hidden inside the body counts), the body again
  goes back to the object; ⌥-click picks the part under the pointer directly; Esc goes up a
  level (part → object → nothing). Hovering a part of the selected object (or any part with ⌥)
  draws its outline, lighter than the selection's. Tab / Shift+Tab walk the selected object's
  parts in tree order and back to the object; they only take over Tab (Prepare / Preview)
  and Shift+Tab (hide the sidebar) while an object with parts is selected in Prepare
  (`keymap::Scope::Parts`).

Screenshots (`target/fast/mica-gui`, B = the binary):

```sh
M=tests/models/3DBenchy.stl; C=tests/models/cube20.stl; O=out/work
$B --screenshot $O/gui-objects-tree.png $M $C --select 0 --object-set fill_density=25% --object-set perimeters=4 --object-set seam_position=rear --command intent:stronger --command "group:Hull details" --command intent:smooth --command "group:Hull details" --command intent:empty --command intent:hole --command intent:supports --command intent:finer --command part:
$B --screenshot $O/gui-objects-object-scope.png $M $C --size 1440x1300 --tab Strength --select 0 --object-set fill_density=25% --object-set perimeters=4 --object-set seam_position=rear --command intent:stronger --command part:
$B --screenshot $O/gui-objects-part-scope.png $M --size 1440x1300 --tab Strength --select 0 --command intent:stronger
$B --screenshot $O/gui-objects-add-menu.png $M --select 0 --command add-menu
$B --screenshot $O/gui-objects-hover.png $M --select 0 --command intent:stronger --command part: --focus --pointer 520,470
```

New `--command`s: `intent:stronger|finer|supports|no-supports|smooth|hole|empty`,
`part:NAME` (select a part by name; `part:` the object), `group:NAME` (the selected part into
a group), `scope:profile|object|part`, `add-menu`.

Tests: `tree` (kind groups and order, user groups, moves, summaries), `save` (groups round
trip in both flavours with unchanged G-code), `intents` (each intent's kind, values and one
undo step; finer layer floor; fit and change shape; recipes save / load / add / delete and a
failed save), `settings` (scope allow-lists), `objects_ui_tests` (drill-down picking, Esc,
Tab, the tree's rows and folding, scope bar clicks and edits at each scope, hover outline).

## What you can do

- **Select several objects**: Shift- or Cmd-click adds to or removes from the selection,
  Shift-drag over empty bed draws a rectangle (objects whose box meets it join), Cmd+A selects
  the plate. The first-selected object is the *primary* (tool card, sidebar scope, gizmo);
  every selected object gets the halo. Group moves translate all; a Z turn or a scale acts
  about the selection's centre so the group keeps its layout; other commands act per object.
- **Copy / paste / duplicate**: Cmd+C puts the selection (settings, painting, volumes, height
  ranges) in the in-app clipboard; Cmd+V pastes on the current plate next to the originals or,
  when the pointer is over the bed, around that point (the group keeps its relative layout);
  Cmd+D duplicates next to each object. Copies get new object ids and unique names
  (`name (2)`), share the mesh `Arc`, and never land on another object (`scene::free_spot`
  searches rings around the spot, then the bed).
- **Instances**: "+" / "−" on an object row (and *Add / Remove instance* in the menu) add an
  instance next to the object, sharing its mesh, settings, painting, volumes and ranges
  (`object_id` shared, `instance_id` consecutive); the list shows them as *Instance N*
  children, and a settings, tool, painting or volume edit on one instance reaches all
  (`App::sync_instances`). "−" removes the highest instance, never the last. Slicing prepares
  each instance as a `PrintObject` with its `instance_id`; the engine has no shared
  preparation across placements yet, so each is prepared on its own (in parallel).
- **Right-click menu** (`commands.rs`), in the viewport and the object list: Copy, Paste,
  Duplicate, Delete, Add / Remove instance, Centre on plate, Drop to bed, Rotate 90°,
  Auto-orient, Mirror X/Y/Z, Change tool (multi-tool printers), Move to plate (existing or
  new), Add modifier / negative volume / support enforcer / blocker (box, cylinder, sphere,
  slab), Add height range, Split to objects / to parts, Cut…, Export as STL…, Rename (inline,
  also on double-click), Object settings…. On empty bed: Paste, Add model…, Arrange plate,
  Arrange all plates, Select all. The same commands sit behind the Edit menu and shortcuts.
- **Modifiers, negative volumes, support volumes** (`scene::Volume`): drawn translucent
  with an x-ray depth test so a volume inside its object stays visible: amber modifiers, grey
  striped negatives ("material taken away"), the support-paint colours for enforcers and
  blockers. Clicking one on the selected object selects it (`Sub::Volume`): the gizmos move,
  turn and scale it *relative to the object* (`Object::apply_to_volume` takes world deltas
  into the object's frame), the tool card edits its position, rotation and size in the
  object's millimetres, and the sidebar's scope bar moves to the part (see above).
  New modifiers start with *Infill density 100 %* so they do something; Esc or "← Object" goes
  back. A 3MF's modifiers, negatives and support volumes import as editable volumes; its
  model parts stay in the regions model.
- **Height ranges** (`scene::HeightRange`): *From / To* millimetres above the object's lowest
  point, listed under the object, drawn as translucent blue slabs while the object is
  selected (brighter for the selected range), each with its own overrides (new ranges start
  with *Layer height 0.1 mm*). Ranges stay sorted and non-overlapping
  (`Object::normalize_ranges`).
- **Engine**: `Object::placed_region` assembles the regions model the worker already used
  for 3MF objects: the model parts (or the mesh), then the volumes in order with
  `scene::config_from(overrides)` as their sparse config, then the ranges in bed Z. The 100 %
  modifier box in a 20 mm cube raises the filament from 1.18 m to 1.53 m; the negative
  cylinder lowers it to 1.35 m and leaves a hole in the preview (`gui9-*-preview.png`).
- **Arrange** (`mica_core::arrange`, original): convex-hull footprints inflated by half the
  spacing, packed largest first bottom-left-fill style with centre-preferring cost, 0/90
  (and 45/135 with rotations on) tried per item, exclusion zones and the wipe tower's
  footprint forbidden, non-rectangular beds respected; items that do not fit spill onto a
  new plate, which is arranged in turn. Spacing comes from Orca's `bed_arrange_distance`,
  else `min_object_distance` when objects print one at a time, else 6 mm. *A* arranges the
  plate, *Shift+A* / *Arrange all plates* gathers every unlocked plate's objects and packs
  them over as many plates as needed. 50 mixed objects pack in ~12 ms (release).
- **Auto-orient** (`mica_core::orient`, adapted from OrcaSlicer's `Orient.cpp`
  `AutoOrienter`, itself derived from Tweaker-3): candidates from area-weighted face-normal
  clusters, hull normals and the axis directions; cost = Orca's weighted overhang area,
  bottom contact and hull contact, contour length, low-angle faces and a penalty for a tiny
  footprint, with Orca's default (`min_area`) weights; the best candidate's normal is turned
  onto −Z about the object's pivot and the object dropped to the bed. Hint line: "Auto-orient:
  overhang 454 mm², contact 454 mm², height 48.0 mm".
- **Cut** (`cut.rs`, `mica_core::mesh_cut` adapted from PrusaSlicer's `cut_mesh`): *C* or the
  scissors open a translucent plane through the middle of the selected object; the Move
  gizmo drags it along Z (and X/Y for tilted planes), *R* tilts it with the rings, the card
  has Height, Tilt X / Y, Keep upper / lower / both, Place on cut face, Flip, Reset,
  Connectors and *Cut*. The halves are watertight (split
  facets share their split points, the caps are ear-clipped with holes), named
  `name (upper)` / `name (lower)`, placed on their cut faces when asked, set beside each
  other and selected. Painting is not carried over (face indices change).
- **Cut connectors** (`cut.rs`, `panels/subcards.rs`, `mica_core::cut_connectors` ported from
  PrusaSlicer's `GLGizmoCut3D` / `CutUtils` / `its_make_*`): *Add connectors* switches the
  view to placing: the gizmo hides, a click on the cut face places a connector (a click on
  one selects it), *Delete* removes the selected one, *Clear all* removes all, *Esc* or
  *Done* leaves the mode. The card shows Type (Plug / Dowel / Snap) with its dependent
  settings indented under it (Style Prism / Frustum for plugs, Shape Triangle / Square /
  Hexagon / Circle for plugs and dowels, snaps are round), Size with its Tolerance under it
  (diameters), Depth with its Tolerance under it, and Rotation; edits apply to the selected
  connector and to the next one placed (defaults as upstream: 2.5 mm, 3 mm deep, 0.1 mm
  depth tolerance). Connectors sit in the plane's own X / Y, so they follow the plane when it
  moves. A connector off the cut face, out of the object's box or overlapping another turns
  red and disables *Cut* with the reason. The cut keeps both halves; a **plug** or **snap**
  becomes a model part of the lower half (a two-part object like *Split to parts*) and a
  negative volume in the upper half, widened by the size tolerance, deepened by the depth
  tolerance and opening 0.05 mm through the cut face; a **dowel** (twice the depth long)
  becomes a negative volume in each half (its widened hole cut by the plane) and an object of
  its own, `name (dowel N)`, upright on the bed. With *Place on cut face* the upper half turns
  over with its holes; the half carrying plugs or snaps stays on its base. The parts and
  negative volumes save as ordinary 3MF volumes (`normal_part` / `negative_part` for Orca,
  `ModelPart` / `NegativeVolume` for PrusaSlicer), so projects reopen as the same scene and
  slice to the same G-code.
- **Split**: *to objects* makes one object per connected component
  (`mica_core::mesh_edit::connected_components`); *to parts* keeps one object whose
  components become model parts of its regions model.
- **Export as STL…** writes the placed, merged selection as binary STL
  (`mesh_edit::write_stl`).

## Headless screenshots (`out/work/gui9-*.png`)

```sh
B=./target/release/mica-gui; M=tests/models/3DBenchy.stl; C=tests/models/cube20.stl; H=tests/models/hole_block.stl
F="$MICA_PRIVATE_MODELS/cat.3mf"   # the optional private multi-tool test project
$B --screenshot out/work/gui9-plates-fixed.png "$F" --printer "Snapmaker U1 (0.4 nozzle)" --plate 2
$B --screenshot out/work/gui9-plates-fixed-top.png "$F" --printer "Snapmaker U1 (0.4 nozzle)" --view top --plates 5
$B --screenshot out/work/gui9-context-menu.png $M --select 0 --right-click 505,520
$B --screenshot out/work/gui9-multiselect.png $C $H $C $H --select 0 --select-also 1 --select-also 3
$B --screenshot out/work/gui9-box-select.png $C $H $C $H --shift-drag 380,420,700,640
$B --screenshot out/work/gui9-paste.png $M --select 0 --command copy --command paste --command paste
$B --screenshot out/work/gui9-instances.png $H --select 0 --command instance --command instance
$B --screenshot out/work/gui9-modifier.png $M --select 0 --command modifier:box --focus
$B --screenshot out/work/gui9-plain-preview.png $C --select 0 --preview --layer 60 --focus
$B --screenshot out/work/gui9-modifier-preview.png $C --select 0 --command modifier:box --preview --layer 60 --focus
$B --screenshot out/work/gui9-negative.png $C --select 0 --command negative:cylinder --focus
$B --screenshot out/work/gui9-negative-preview.png $C --select 0 --command negative:cylinder --preview --layer 50 --focus
$B --screenshot out/work/gui9-height-range.png $M --select 0 --command range --command range --focus
$B --screenshot out/work/gui9-arrange-before.png $C $H $M $C $H $M $C $H $C $H $M $C --arrange rows --view top
$B --screenshot out/work/gui9-arrange-after.png  $C $H $M $C $H $M $C $H $C $H $M $C --view top
$B --screenshot out/work/gui9-orient-before.png $M --select 0 --tilt 35,20 --focus --gizmo rotate
$B --screenshot out/work/gui9-orient-after.png  $M --select 0 --tilt 35,20 --command orient --focus --gizmo rotate
$B --screenshot out/work/gui9-cut.png $M --select 0 --command cut --cut-z 18 --cut-tilt 0,12 --focus
$B --screenshot out/work/gui9-cut-result.png $M --select 0 --command cut --cut-z 18 --command cut-apply --focus
C20=tests/models/cube20.stl
$B --screenshot out/work/gui-cut-connectors.png $C20 --select 0 --command cut --connector-size 5,6 --connector plug@-5,-4 --connector dowel@4,-4 --connector snap@0,5 --connector-edit --focus
$B --screenshot out/work/gui-cut-connectors-square.png $C20 --select 0 --command cut --connector-shape square --connector-size 6,5 --connector plug@-4,0 --connector plug@4,0 --connector plug@9,8 --connector-edit --focus
$B --screenshot out/work/gui-cut-connectors-result.png $C20 --select 0 --command cut --connector-size 5,6 --connector plug@-5,-4 --connector dowel@4,-4 --connector snap@0,5 --command cut-apply --view iso
```

Connector flags: `--connector KIND@X,Y` (repeatable; `plug`, `dowel`, `snap` at plane mm from
the plane's centre), `--connector-shape triangle|square|hexagon|circle`, `--connector-size
DIAMETER,DEPTH`, `--connector-edit` (leave the card in *Add connectors* mode).

New flags: `--select-also N` (multi-selection), `--command NAME[:ARG]` (repeatable, in order:
`duplicate`, `copy`, `paste`, `instance`, `instance-remove`, `modifier:box|cylinder|sphere|slab`,
`negative:…`, `enforcer:…`, `blocker:…`, `range`, `arrange`, `arrange-all`, `orient`,
`select-all`, `cut`, `cut-apply`, `split-objects`, `split-parts`, `centre`, `drop`,
`mirror-x|y|z`, `rotate90`, `delete`, `settings`), `--sub-set key=value` (an override on the
selected volume / range), `--cut-z Z`, `--cut-tilt X,Y`, `--right-click X,Y` (opens the
context menu there), `--shift-drag X1,Y1,X2,Y2` (rectangle selection), `--arrange pack|rows|
none` (how loose models are laid out), `--tilt X,Y` (tilt the selected object), `--layer N`
(top layer shown in the preview).

## Tests

- `mica_core::mesh_cut`: box and tilted cuts with watertight halves and exact volumes, a
  through-hole frame keeps its hole in the caps, misses, `cap = false`, determinism, dense
  sphere / cylinder / `hole_block.stl` stress cuts. `mesh_edit`: components, STL round trip,
  watertight primitives, mirrored transforms.
- `mica_core::arrange`: 12 and 50 items packed without overlap and inside the bed (with the
  spacing), spill, exclusion zone and tower avoided, a bar that only fits rotated, an
  octagonal bed, centring, fixed items, determinism, timing.
- `mica_core::orient`: a flat box stays, a tilted box comes back flat, wedges pick their large
  face, a pyramid its base, a T prism the overhang-free side, `rotation_to_down`,
  determinism, a 100k-triangle mesh in well under a second.
- `mica-gui`: `scene` (placed regions with volumes and ranges, imported modifiers become
  editable, volume deltas in the object's frame, range normalisation and instance sync, free
  spots), `commands` (selection toggling, copy / paste / duplicate, instances sharing and
  syncing, group commands incl. move to plate, volumes and ranges feeding the regions model,
  split, arrange packing and spilling, auto-orient on a tilted cube, the cut command, shapes),
  `cut` (plane state, halves' volumes, keep one side, a miss), `plates`
  (`placement_on` against the object's own plate), the screenshot flags.

## Left for later

- The painting of cut halves; cutting an object with modifiers drops them. Connectors:
  no auto mode, no snap bulge / space fields (upstream's defaults are used), no
  multi-selection, and connector edits are not undo steps.
- Instances are separate `PrintObject`s in the engine; a shared preparation per mesh would
  save time on plates full of one part.
- Arrange uses convex hulls (no NFP sliding between concave parts) and bbox-flush candidate
  positions. (The wipe tower was reserved as a square of its width; since
  [gui-prime-tower.md](gui-prime-tower.md) it is its real estimated or sliced footprint.)
- Volumes, ranges and painting are written back by File › Save (see
  [3mf-export.md](3mf-export.md), "Saving from the GUI"); modifier settings use the
  object-override groups, so printer-only keys are not offered.
