# GUI painting tools

Support, seam, colour and fuzzy-skin painting in the 3D view, on top of the engine's facet
painting from [painting.md](painting.md). The painted state is the same `FacetPainting` the 3MF
importer produces, so what you paint is what the slicer sees, and a loaded project's own painting
shows up and can be edited.

## What you see

- **Toolbar**: after Move / Rotate / Scale / Lay on face / Cut, four more icons: *Support
  painting*, *Seam painting*, *Colour painting* (enabled on multi-tool printers only: XL, U1),
  *Fuzzy skin painting*. One is active at a time; the gizmos are off while it is. `M`, `R`, `S`,
  `F` and Esc leave the tool.
- **Tool card** (top right, instead of the transform card): the brush shape (*Circle* = what the
  pointer covers, front faces only; *Sphere* = around the hit point, through to the back; *Fill*
  = the connected surface within an angle; *Height* = a horizontal band around the hit), the size
  field with a slider (or the fill angle / band height), the mode (*Enforce* / *Block* for
  supports, *Seam here* / *No seam* for seams, one swatch chip per tool for colours), *Erase*
  (or hold Shift), *Clear all*, and for supports *Show overhangs steeper than N°* with *Paint on
  overhangs only* and *Paint overhangs as enforcers*. The card's last line and the hint line
  read "Left-drag to paint · Shift-drag to erase · Alt-scroll to change brush size".
- **On the model**: a ring of the brush radius in the plane of the facet under the pointer (a
  sphere adds a faint second ring), in the tool's colour, red while erasing. Painted facets are an
  overlay. The model's base colour is always its filament or colour-painting colour (solid, in
  the tool's colour, always shown). Every other kind is a **pattern over** that colour, never a
  replacement, so a fuzzy-skin stroke on a colour-painted pocket no longer turns it into a yellow
  filament: support enforcers a teal diagonal hatch, blockers a red hatch; seam enforcers violet
  stripes, blockers pink stripes; fuzzy skin a fine light stipple. The patterns are screen-space
  (crisp at any zoom) and blended at a strength: full for the active tool on the object being
  painted, half otherwise. Outside its tool a kind shows only when its *View › Always show …
  paint* toggle is on (support and fuzzy skin also on the selected object; seam only with the
  toggle or its tool). With *Show overhangs* the facets steeper than the threshold are tinted
  magenta (faces on the bed excluded).
- **Object list**: a small brush after the name of a painted object (hover lists the kinds).
- **Preview**: a job that printed with more than one tool (multi-tool or colour-painted) opens
  coloured by *Filament*; the legend's tool rows carry the grams per tool.

## How it works

`mica_core::painting::selector::TriangleSelector` (a port of PrusaSlicer's `TriangleSelector`,
see `PROVENANCE.md`) holds the original triangles plus the subdivision painting made: a facet the
brush edge crosses is split along its long sides (one, two or three, down to a fifth of the brush
radius; 0.1 mm for height ranges) and the children are tested again, so patches follow the brush.
Children that end up with one state merge back into their parent. The selector keeps a bed-space
copy of the vertices next to the mesh-local one, so every cursor test runs in millimetres on the
bed regardless of the object's scale or mirror. `serialize()` writes the tree as the 3MF
`TriangleSelector` nibble codes (`FacetStates::push_codes`, the same stream `FacetStates::push`
decodes from the attribute text), and `load()` reads them back; `FacetStates::wire_string` gives
the attribute text for an export.

`crates/mica-gui/src/paint.rs` owns the brush `Settings` and the `Session` (one selector for the
selected object and the active kind, rebuilt when the object, kind, placement or painting
changed, for example after undo). A stroke frame from the viewport (`Action::Paint { index, hit,
trail, erase }`, the hits from `picking::pick_face` on the selected object) runs
`Session::stroke_path` and,
when something changed, writes the serialised kind back into `Object::painting` (an
`Arc<FacetPainting>`, mesh-local). The viewport opens an undo gesture when a stroke starts and
ends it on release, so one stroke is one undo entry; *Clear all* and *Paint overhangs* are one
entry each.

Strokes are continuous at any drag speed (PrusaSlicer's `get_projected_mouse_positions`): each
frame the viewport samples the pointer from its previous position to the current one, about half
a brush radius apart (at most `paint::MAX_STROKE_SAMPLES` per frame), raycasts every sample,
simplifies the hits (Douglas-Peucker at a tenth of the radius) and sends them as one stroke
frame; the circle and sphere brushes then sweep a capsule over each pair of consecutive hits
(`Cursor::capsule`: upstream's `Capsule2D` on the view plane for the circle, `Capsule3D` for the
sphere), fill and height range apply at every hit. A sample off the model ends the run, the next
hit starts a new one. The split depth (`MAX_SPLIT_DEPTH`) and the subdivision budget bound
capsules as they bound dabs.

`scene::paint::overlay_vertices` turns the painting into a per-vertex mesh of colour, pattern
and strength (`Show`: a strength per kind, from `App::paint_show`; cached per object by the
painting's fingerprint and the visibility) that `render/paint.wgsl` draws over the object, lifted
a little towards the eye: solid for colour painting, otherwise a pixel-space hatch / stripes /
stipple that discards between its marks and alpha-blends on them, so the base colour shows through.

Slicing: `Object::placed_region` hands the painting to the engine. A 3MF object's painting is
split back onto its model volumes by face ranges (the merged mesh is their concatenation); a plain
STL gets a one-volume regions model, so a painted object always goes through
`PrintObject::prepare_regions`. Imported painting with mixed windings across volumes stays on the
volumes and is not editable (rare). The worker keeps `Volume.painting` through transforms and
mirrors (winding flipped with the faces).

## Screenshots

```sh
B=./target/release/mica-gui; M=tests/models/3DBenchy.stl
$B --screenshot out/work/gui6-3mf-colour.png tests/models/painted-cube.3mf --xl --select 0 --focus --filament "2=Prusament PC Blend"
$B --screenshot out/work/gui6-3mf-colour-preview.png tests/models/painted-cube.3mf --xl --preview --filament "2=Prusament PC Blend"
$B --screenshot out/work/gui6-paint-support.png $M --select 0 --focus --paint support --brush-size 4 --drag 460,500,600,522
$B --screenshot out/work/gui6-paint-overhangs.png $M --select 0 --view below --focus --paint support --paint-overhangs --pointer 560,470
$B --screenshot out/work/gui6-paint-seam.png $M --select 0 --focus --paint seam --brush-size 1.5 --drag 520,445,523,500
$B --screenshot out/work/gui6-paint-colour.png tests/models/cube20.stl --xl --select 0 --focus --paint colour --paint-tool 2 --filament "2=Prusament PC Blend" --brush sphere --brush-size 6 --drag 450,350,600,380
```

```sh
# Colour-painted cube with a fuzzy-skin stroke: the fuzzy tool active, and Prepare without a tool.
$B --screenshot out/work/gui8-paint-fuzzy-tool.png tests/models/painted-cube.3mf --xl --select 0 --focus --filament "2=Prusament PC Blend" --paint fuzzy --brush sphere --brush-size 5 --drag 500,420,640,470
$B --screenshot out/work/gui8-paint-fuzzy-prepare.png tests/models/painted-cube.3mf --xl --select 0 --focus --filament "2=Prusament PC Blend" --paint fuzzy --brush sphere --brush-size 5 --drag 500,420,640,470 --escape
```

`--escape` presses Esc after the drag (leaves the tool); `--show-paint support,seam,fuzzy` turns
the View toggles on.

New flags: `--paint support|seam|colour|fuzzy` (activates the tool on the selected object),
`--brush circle|sphere|fill|height`, `--brush-size R`, `--paint-tool N` (one-based), `--block`,
`--overhangs` (show), `--paint-overhangs` (press the button), `--drag X1,Y1,X2,Y2` (a released
left drag with eight intermediate moves: a stroke), `--filament T=NAME` (the filament preset for a
tool, by prefix). `tests/models/painted-cube.3mf` is the colour fixture from
`scripts/make-painting-oracle.py` (two adjacent side faces painted tool 2).

## Tests

- `mica_core::painting::selector`: neighbours and outward normals (also mirrored), brush hit tests
  (circle vs sphere depth, pointer in triangle, edge inside, visibility), split-on-paint (disc
  area, subdivision count, idempotent repaint, erase merges children back), sphere paints through
  and the overhang filter, "paint overhangs" on a lifted cube, height range and smart fill, the
  round trip selector → nibble codes → wire strings → engine decoder → leaf geometry → selector,
  reversed streams, every state nibble form.
- `mica-gui`: overlay colours, patterns and strengths (a fuzzy stroke over colour painting keeps
  the colour solid underneath), the visibility rules (`App::paint_show`), `paint::Settings` states, a stroke stored on an object
  and read back (stale after a move, erase, fill, height range, overhangs); the colour-painted 3MF
  sliced through the worker with per-tool filament equal to the CLI's path and tool 2 printing;
  a painted support enforcer under a floating slab producing support where "Enforcers only"
  printed none; a painted seam stripe pulling every layer's seam into the stripe; the `--drag`
  event script and the new flags.

## Left for later

- Strokes are painted where each frame's pointer hits; a very fast drag can leave gaps between
  dabs (PrusaSlicer bridges them with capsule cursors).
- No bucket fill, no clipping plane, no "paint on the whole object" helpers beyond *Paint
  overhangs*. (The painting is saved with the project since File › Save exists; see
  [3mf-export.md](3mf-export.md).)
- The overhang threshold on the card is independent of the Supports *Threshold angle* setting.
