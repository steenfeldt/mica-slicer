# Layer custom G-code in the GUI

Colour changes, pauses, custom and template G-code and tool switches at a layer
(PrusaSlicer's `custom_gcode_per_print_z`, Orca's per-plate `custom_gcode_per_layer`).
The engine side is `mica_core::custom_gcode`; this page covers the editor.

## Where it lives

- Each plate stores a `mica_core::custom_gcode::Info` (`plates::Plate::custom_gcode`). It is part
  of the undo snapshot and the unsaved-changes check (`app::PlateState`), opens from 3MF projects
  per plate (`LoadedPlate::custom_gcode`, or `Loaded::custom_gcode` for projects without plates)
  and saves into `Project::custom_gcodes` with the printer's colour change / pause / template
  G-code for the legacy `gcode` attribute.
- Slicing passes it to `PreparedPrint::with_custom_gcode` (`worker::Job::Objects(objects, info)`,
  `worker::slice_with_custom_gcode`); the exporter emits it.

## The layer slider

- A violet **+** sits left of the upper handle. Click: add what the printer can do at that
  layer — a colour change (M600) on a single-tool printer, a switch to the next tool when every
  object prints with one tool on a tool changer (the U1 too, as Orca), else a colour change of
  the objects' first tool (a "Change filament" pause on Klipper, which has no M600 change).
- Right-click: a menu with *Add colour change* (not on Klipper), *Add pause…* (message editor),
  *Add custom G-code…* (multi-line editor), *Add template G-code* (when the printer has one) and,
  on tool changers, *Change to tool N* (enabled when every object prints with one tool).
- On a layer that already has an item the button shows **×**: click removes it; right-click
  offers *Edit…* (pause message, custom G-code) and *Remove*, or replacing it.
- Markers right of the track: a colour swatch for colour and tool changes, two bars for a pause,
  "G" for custom / template G-code. Hover for what it does, its height and layer; click to move
  the upper handle there (then × removes it).
- An edit is one undo step and outdates the plate's G-code: the plate re-slices at once and the
  preview keeps its layer range (`App::keep_view`).

## Colour print view

With colour or tool changes the preview opens in **Colour by › Colour print**: every layer takes
the colour of the change that started its segment (per tool when objects use several tools). The
legend lists the segments with their height range, time and share, and the pauses and custom
G-code below. The shader reads one packed value per layer (`preview::color_print`).

## Headless checks

`mica-gui --screenshot out.png model.stl --preview --layer-gcode 'color:5;pause:10:Magnets'`
preloads items; `--layer-menu` and `--layer-dialog pause|custom[:TEXT]` open the menu or editor at
`--layer N`. The monkey test adds, removes and edits items and checks that projects round-trip
them.
