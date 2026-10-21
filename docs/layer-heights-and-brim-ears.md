# Variable layer height and brim ears

## Variable layer height

`mica_core::layer_profile` is a port of PrusaSlicer's `Slicing.cpp` / `SlicingAdaptive.cpp`
(see `PROVENANCE.md`): the slicing parameters an object's tools give (`min_layer_height`,
`max_layer_height`, 0.75 × nozzle; Orca reads these vectors one tool lower, which is kept),
the profile from height ranges, the **Adaptive** cusp-height profile, Gaussian **Smooth**,
the interactive band edit (`adjust_layer_height_profile`) and `generate_object_layers`.

A profile is `(z, height)` pairs from the object's bottom to its top. `RegionModel::layer_height_profile`
carries it to the slicer; `regions::layer_tops` uses it when its last `z` is within 0.001 mm
of the object's height (upstream's check), otherwise the profile is ignored, as upstream does
after scaling. Jobs without a profile keep the existing schedule (reference hashes unchanged).
Orca's `precise_z_height` (off by default) is not ported.

3MF: PrusaSlicer `Metadata/Slic3r_PE_layer_heights_profile.txt`, Orca
`Metadata/layer_heights_profile.txt`, both `object_id=N|z;h;z;h…` with `%f` values, `N` the
one-based model object (Prusa's resource order, Orca's build order, like the height ranges).
Upstream rejects lists of four values or fewer on read; so does Mica.

**GUI** (toolbar icon after the painting tools): the card shows the profile graph (Z up, layer
height right, a colour strip in upstream's palette: green thin, white normal, red thick), band
width, *Adaptive* with *Detail ↔ speed*, *Smooth* with radius and *Keep thin layers thin*,
*Reset*. On the model: drag up for thicker layers, down for thinner; Shift goes back to the base
height, Alt smooths; Alt-scroll changes the band. Dragging sideways in the graph does the same
at the pressed height. The model shows the layer-height gradient while the tool is open. Every
drag and button is one undo step; the profile is saved with the project.

### PrusaSlicer comparison (2026-10-17)

PrusaSlicer 2.9.6 (`--datadir out/ref/datadir --no-binary-gcode`), CORE One HF0.4 / 0.20mm SPEED,
each sliced from the same 3MF as Mica. The `;{layer_z}` line of the before-layer-change template
is compared as text (full precision) with `out/scripts/zlist.py`:

| Profile | Layer changes | Result |
|---|---:|---|
| Benchy, Adaptive 0.5 (`mica slice … --export-3mf --flavour prusa --adaptive-layers 0.5`) | 363 | identical |
| Benchy, Adaptive 0.2 + Smooth 5 | 508 | identical |
| cube20, one GUI drag (`--object-tool layers --drag …` then `--save`) | 104 | identical |

OrcaSlicer 2.4.1 on the U1 (Benchy, Adaptive 0.5, Orca 3MF): 350 layer changes, identical.

## Brim ears

Automatic ears (`brim_type = brim_ears`, `brim_ears_max_angle`, `brim_ears_detection_length`)
were already ported. New: Orca's **painted** ears (`brim_type = painted`): hand-placed discs from
`make_brim_ears` (radius rounded to even brim spacings, minus the gap and one spacing, upstream's
`float` steps), on the outside and inside holes, and Orca's rule that a brim island must touch
the object, its own brim or another object's brim (a loose ear is dropped).

3MF (Orca only; PrusaSlicer has no ears): `Metadata/brim_ear_points.txt`,
`brim_points_format_version=0` then `object_id=N|x y z r …`. OrcaSlicer recentres a loaded
object on its bounding box without moving the points, so the points are written and read
relative to the centre of the object's bounding box (`threemf::Object::brim_frame_origin`, about
zero for Orca's own centred meshes). CLI: `--export-3mf … --brim-ear X,Y,R`.

**GUI** (toolbar icon after the layer heights): click the model to place an ear (just below the
bed, as Orca), click an ear to remove it; Alt-scroll or the card sets the size (Orca's 16
first-layer line widths by default). *Auto-place* puts an ear on every sharp corner of the first
layer (angle and detection radius in the card); *Remove all*. Loose ears are drawn orange and
counted. If the object's brim is not "Ears where I place them" (Orca's *painted*), the card says
so and offers the object override. In the brim settings an Orca printer offers *Auto*, *Mouse
ears (automatic)* and *Ears where I place them*; the ear angle and detection radius appear under
the brim type while it is the automatic ears.

### OrcaSlicer comparison (2026-10-17)

OrcaSlicer 2.4.1, U1 0.20 Standard, Snapmaker PLA, High Temp Plate, `--slice 1` on the same
3MF; `scripts/compare-adhesion.py`:

| Case | Brim filament Orca / Mica | Loops | Max loop-bound error |
|---|---:|---:|---:|
| cube20, 4 painted ears (5, 5, 5, 3 mm) | 10.80275 / 10.80378 mm | 14 / 14 | 0.009 mm |
| hole_block, 2 outer + 1 hole ear | 11.52459 / 11.52495 mm | 14 / 14 | 0.015 mm |
| hole_block, automatic ears | 3.81310 / 3.81356 mm | 8 / 8 | 0 |
| GUI save: cube20, 2 ears + 1 loose ear (dropped by both) | 4.45990 / 4.46100 mm | 6 / 6 | 0.001 mm |

```sh
B=./target/release/mica-gui
$B --screenshot out/work/gui-vlh-adaptive.png tests/models/3DBenchy.stl --select 0 --focus --object-tool layers --adaptive 0.5
$B --screenshot out/work/gui-vlh-drag.png tests/models/cube20.stl --select 0 --focus --object-tool layers --band 6 --drag 470,480,470,700
$B --screenshot out/work/gui-brimears-auto.png tests/models/cube20.stl --printer 'Snapmaker U1 (0.4 nozzle)' --select 0 --focus --object-tool ears --auto-ears
$B --screenshot out/work/gui-brimears-placed.png tests/models/cube20.stl --printer 'Snapmaker U1 (0.4 nozzle)' --select 0 --focus --object-set brim_type=painted --object-tool ears --brim-ear 125,125,10 --brim-ear 145,145,6 --brim-ear 170,135,8
```
