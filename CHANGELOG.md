# Changelog

What changed in each Mica Slicer release, newest first. Each version's section is written once
and used everywhere: the app shows it after an update (Help › What's new…), and
`scripts/release.sh` publishes it as the GitHub release notes and in the update manifest.

A section starts with `## <version>`. Inside it: `###` headings, `-` bullets, `**bold**` and
`[links](https://…)`.

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
