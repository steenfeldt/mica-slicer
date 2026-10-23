# Changelog

What changed in each Mica Slicer release, newest first. Each version's section is written once
and used everywhere: the app shows it after an update (Help › What's new…), and
`scripts/release.sh` publishes it as the GitHub release notes and in the update manifest.

A section starts with `## <version>`. Inside it: `###` headings, `-` bullets, `**bold**` and
`[links](https://…)`.

## 0.0.2

Mica now comes for Windows and Linux too. Both are experimental: they have been tried on one
Windows 11 PC and one Linux laptop so far.

### Windows and Linux

- **Windows:** a 64-bit installer. It installs for your user account only, without an
  administrator prompt. The installer is not code-signed yet, so Windows SmartScreen warns
  about it: choose More info, then Run anyway.
- **Linux:** an AppImage for 64-bit PCs. Make it executable and run it; add it to the app menu
  (for example with Gear Lever) to get Mica's icon in the dock. On GNOME, Mica's window has a
  GNOME-style title bar.

### Slicing

- Spiral vase mode prints with the rules of PrusaSlicer and OrcaSlicer: one continuous wall
  that rises as it goes around. Supports, several objects and several filaments are not
  possible in vase mode; Mica tells you when a project has them.
- Start and end G-code can use the names of the print, filament and printer profiles
  (print_preset, filament_preset and printer_preset), like in PrusaSlicer and OrcaSlicer.

### Fixes

- A fresh install no longer picks a printer on its own. Add your printer first; until then
  Slice and Export say why they are off.
- Projects and profiles from Snapmaker Orca open without errors such as "invalid boolean":
  speeds, accelerations and overhang settings that Snapmaker Orca stores per filament flow
  type now read as one value. When the values differ by flow type, Mica uses the standard one
  and says so.

### Not yet

- Absolute extrusion (M82) is not supported yet.

Found a problem? [Report it on GitHub](https://github.com/steenfeldt/mica-slicer/issues).

## 0.0.1

The first tester build, for macOS on Apple Silicon. Mica is alpha (pre-release) software:
check the preview before printing and stay near your printer for the first layers.

### Slicing

- Prusa printers slice with the rules of **PrusaSlicer 2.9.6**, the Snapmaker U1 and other
  Klipper printers with the rules of **OrcaSlicer**. Tested on real prints: Prusa CORE One, XL,
  MK3.5 and MINI, Snapmaker U1 and RatRig V-Core 3.2.
- Opens PrusaSlicer and OrcaSlicer 3MF projects with their plates, modifiers, painting and
  settings.
- Several plates per project. Slice all plates at once (Cmd+Shift+R), then export or send them together.
- Painting for colors, supports, seams and fuzzy skin; surface textures; mixed colors on
  multi-tool printers; text and SVG on models; cut, boolean, simplify and repair.

### Profiles

- Printer, print and filament profiles for the Snapmaker U1, RatRig, Bambu Lab, Creality, Voron,
  Elegoo, Anycubic, QIDI and Sovol come with the app (from OrcaSlicer).
- Prusa's printer profiles are downloaded from Prusa the first time you need them.
- Bring your own profiles over from PrusaSlicer and OrcaSlicer.

### Library

- Point Mica at your model folders. It indexes STL, 3MF, OBJ, STEP and G-code files and groups
  the files of one download into a project.
- The details panel shows a 3D preview in the project's colors, plate by plate, and which of
  your printers each plate fits on. Drag a model onto the plate.

### Printers

- Send prints to PrusaLink, Prusa Connect and Moonraker printers, watch their cameras, and
  match the loaded filaments to your project.

### Getting started

- A welcome window on first launch, and a short tour of the main window (Help › Show quick
  tour).
- New versions arrive through the app; Check for Updates… looks for them.

### Not yet

- Absolute extrusion (M82) and spiral vase are not supported yet.
- Windows and Linux builds pass the automated tests but have not been tried on real machines.

Found a problem? [Report it on GitHub](https://github.com/steenfeldt/mica-slicer/issues).
