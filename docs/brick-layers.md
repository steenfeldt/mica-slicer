# Brick layers

Every other inner wall prints half a layer higher, so the walls' layer lines are staggered like
bricks and no layer line runs straight through a wall. Inspired by GeekDetour's Brick Layers;
strength tests by CNC Kitchen.

Brick layers is a **hidden** feature: Settings › Features does not list it and the print
settings do not show its group until it is turned on by hand in Mica's settings folder
(`mica.toml`, see `mica_core::user_data::config_dir`):

```toml
[features]
brick_layers = true
```

The GUI and `mica slice` read the same table (`mica_profiles::features`), so the same settings
folder gives the same G-code from both. Without it, a project that sets `mica_brick_layers`
slices without brick layers, keeps the settings when saved, and says so.

## Settings (print, Strength › Brick layers)

| Key | Default | Meaning |
|---|---|---|
| `mica_brick_layers` | off | Raise every other inner wall. |
| `mica_brick_layers_flow_ratio` | 105 % | Extra flow on raised walls and the inner walls between them. Never below 100 %. |
| `mica_brick_layers_start_layer` | 2 | First layer (from 0) whose walls may be raised. |
| `mica_brick_layers_end_margin_layers` | 1 | Unraised layers kept between raised walls and every top or bottom shell, beyond the shell. |
| `mica_brick_layers_travel_lift` | 0 | Travel height above raised walls, mm; 0 = the retraction lift, at least 0.2 mm. |

They are Mica-only options (`mica_options.tsv`), saved in 3MF projects like the other `mica_*`
settings, and work under PrusaSlicer rules (CORE One, XL) and Orca rules (U1).

## Product rules

These hold for every print; `mica_gcode::brick_check` verifies them on the G-code (see
Validation).

1. **Every bead is at least as tall as its layer.** No extrusion is thinner than the nominal
   layer height, and no flow is ever reduced (or speed raised) to make a bead thinner. The only
   flow change is the extra-flow setting, which is never below 100 %.
2. **Starts are taller.** The first raised bead of a wall reaches from the layer below to half a
   layer above its own layer: 1.5 layers tall.
3. **Endings rejoin the layer grid upward.** A raised wall stops being raised below every top
   surface (the part's top, a step, a hole's ceiling): it skips one layer, and the next layer's
   bead fills the 1.5-layer gap at the normal layer height. Raised walls also start only above
   every bottom shell. The top and bottom shell layers, plus the margin, print exactly as
   without brick layers.
4. **Walls stay walls.** Only stretches of inner wall loops move up. No extra beads, caps or end
   pieces are added.

With variable layer heights the raise is half the object's thinnest layer, the same on every
layer, so a raised bead on a raised bead is exactly as tall as its own layer.

## How it works (`mica_core::brick_layers`)

- Walls are numbered from the outside (outer wall 0). Inner walls with an odd number are the
  raised walls (Classic and Arachne alike; around holes the numbering counts from the hole).
- Once per object, `Plan::build` decides per layer the area where raised walls print raised:
  the outlines (and with modifiers the region slices) of the layers around must stay the same,
  within a quarter wall width per layer. The window covers the bottom shell and margin below,
  and the skip, the 1.5-layer bead, the margin and the top shell above. Changes rule out
  everything within the wall band around them.
- Per layer, `Plan::apply` cuts each raised wall at that area's borders and prints each stretch
  raised, skipped, or at layer height with a taller bead (see the table in the module). Each
  stretch carries its true bead height as its flow height and its raise in
  `ExtrusionPath::brick`. Raised stretches move to the end of the object's layer.
- The G-code writer prints a raised path at its own Z (as scarf seams do) and keeps travels at
  least the travel lift above raised walls until the next layer starts above them; no extra
  `;LAYER_CHANGE`, so the preview, progress, binary G-code and U1 timelapse count layers as
  before.

## v1 limits

- **One tool only.** A plate that prints with several tools (tool per object or role, paint,
  per-layer tool switches) slices without brick layers and says so: another tool's walls or a
  wipe tower travel could meet raised walls.
- Supports on their own layer heights turn brick layers off for that object (they print between
  the object's layers).
- Vase mode: off. Fewer than 2 walls: nothing to raise.
- Sloped walls that move more than a quarter wall width per layer are not raised, nor walls near
  a top or bottom surface: brick layers work on vertical and near-vertical walls.
- Brick-layers settings on modifiers or height ranges are ignored; the object's setting
  counts.
- Raised stretches lose their scarf seam and their overhang speed sections.
- Print time grows (Benchy: about +10 %): extra travels, Z lifts and the taller start beads.

## Validation

- Unit tests: `mica-core` `brick_layers::tests` (table, cutting, hole next to the outline with
  Classic and Arachne, a step's top shell, variable layer heights, settings).
- G-code checks: `mica-cli` `tests/brick_layers.rs` slices CORE One, U1 and XL models with and
  without brick layers and runs `mica_gcode::brick_check`:
  - every extrusion's measured bead (nozzle Z minus the highest material below its centreline)
    and declared `;HEIGHT:` are at least the layer height;
  - every wall's filament per mm³ of bead is at least the print's without brick layers;
  - nothing prints at layer height after a raised wall on a one-object layer;
  - the first 4 and last 6 layers print the same extrusions as without brick layers.
- GUI and CLI print the same (`cli_parity_tests::brick_layers_*`).
- Any pair of files: `MICA_BRICK_ON=on.gcode MICA_BRICK_OFF=off.gcode cargo test -p mica-cli
  --test brick_layers -- --ignored check_files`.
