# Mica Slicer app icon

Original artwork made for Mica, licensed with the project (AGPL-3.0-only).
Three rounded layer bars in Mica's violet on near-black, each layer shifted a little to the
right like a model building up layer by layer. Deliberately simple: no letters (an M was
dropped because it read like the Gmail logo) and no 3D rendering.

- `mica.svg`: the master (used from 64 px up).
- `mica-small.svg`: the same mark with thicker bars and tighter gaps, for 16–32 px.
- `mica.icns` (macOS), `mica.ico` (Windows), `mica-*.png` (Linux / window icon).
- `mica.ico` holds every size Windows asks for (16, 20, 24, 32, 40, 48, 64, 96, 128, 256 px):
  32-bit bitmaps with alpha below 256 px, PNG at 256, rendered with resvg (`mica-small.svg` up
  to 32 px). `build.rs` embeds it in `mica-gui.exe`; the NSIS installer and uninstaller use it
  too, with `packaging/windows/installer-sidebar.bmp` on their welcome and finish pages.

The other rasters are rendered with AppKit onto a transparent canvas (Quick Look flattens onto white,
which gave white corners once). The generator scripts are not part of the repository.
