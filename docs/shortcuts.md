# Keyboard shortcuts

Mica uses OrcaSlicer's keys so the muscle memory carries over; PrusaSlicer's keys fill the gaps
where Orca has none. Everything is defined in one table, `crates/mica-gui/src/keymap.rs`
(`BINDINGS`). That table drives the key handling, the shortcut labels in the menus and the
**Keyboard shortcuts** window (`?`, or Help › Keyboard shortcuts).

**Sources** (adapted: only the key assignments are taken, no code):

- orca@8500fcdcca: `src/slic3r/GUI/KBShortcutsDialog.cpp` (`fill_shortcuts`),
  `GLCanvas3D.cpp` (`on_char`, `on_key`), `Gizmos/GLGizmosManager.cpp` (`on_char`, `on_key`,
  `handle_shortcut`) and each gizmo's `m_shortcut_key`, `MainFrame.cpp` (menus and the
  `wxEVT_CHAR_HOOK` keys), `GUI_Preview.cpp` (`update_layers_slider_from_canvas`), `IMSlider.cpp`
  (`switch_one_layer_mode`), `Plater.cpp` (event bindings), `Jobs/ArrangeJob.cpp` and
  `Jobs/OrientJob.cpp` (which arrange/orient scope each key uses).
- prusaslicer@b028299c77: `src/slic3r/GUI/KBShortcutsDialog.cpp` (`fill_shortcuts`).

Where Orca's dialog and its code disagree, the code wins. Examples: the dialog says
`L` = "SLA support points", but `GLGizmoFdmSupports` binds `L` to FDM support painting. The
dialog leaves out `N` (colour painting, `GLGizmoMmuSegmentation`). It lists the camera views as
Ctrl+0–6, and plain digits set the filament.

## Rules

- **⌘ on the Mac is Ctrl elsewhere.** Mac-only keys: Ctrl+⌘F (full screen) and ⌘M (minimise).
  Settings is ⌘, on the Mac; elsewhere it is Ctrl+P (as in Orca) and Ctrl+, also works. In
  Settings, Ctrl+Tab and Ctrl+Shift+Tab (or ⌘] and ⌘[) switch tabs, and Esc closes it.
- **Text fields own the keyboard.** While a text field, number field or search box has focus
  (egui's `wants_keyboard_input`, plus Mica's inline number editors), no shortcut fires. That
  includes Delete, Backspace, Esc, `+`/`−` and the letter keys. Esc in a field only closes the
  field: egui drops a field's focus on Esc before anything else runs, so the keymap also stays
  out of the way for one frame after a field loses focus.
- **Tool keys toggle.** M, R and S open the move, rotate and scale tools, and the same key
  closes them. L, P, N and H turn painting on and off. C opens and closes the cut tool.
  As in Orca, tool keys need a selection, except the painting keys: painting selects the
  object you paint on.
- **Esc goes outwards.** It first cancels a drag. Then it closes the cut tool, painting, the
  measure tool (after first dropping its selected features), lay on face, or rotate or scale
  (rotate and scale go back to move, Mica's default tool), then the section view. Then it drops
  a selected part, and finally clears the selection. In Preview it closes the section view.
- **Painting shadows some tool keys, as in Orca.** In support and colour painting, F, S and C
  pick the fill, sphere and circle brushes. In seam painting only S and C do; there F is still
  lay on face. In colour painting, H picks the height-range brush. Fuzzy-skin painting has no
  brush keys.
- **Digits.** With objects selected on a printer with more than one tool, 1–9 set the
  filament (Orca). Otherwise 0–6 are the camera views (PrusaSlicer). ⌘0–⌘6 always pick the
  camera views (Orca).
- Key repeat repeats only the arrows, Page Up/Down, `+`/`−` and I/O.
- Tab and the arrow keys would also move egui's keyboard focus. When they trigger a shortcut,
  the focus move is cancelled, so a shortcut never hands the keyboard to a widget.

## Comparison

"—" means unbound. **Bold** in the last column marks a change from Mica's earlier key.
"pending: X" means Orca has the key, but Mica doesn't have the feature X yet. The key is left
free for it.

### File and window

| Key | OrcaSlicer | PrusaSlicer | Mica before | Mica after |
|---|---|---|---|---|
| ⌘N | New project | New project | New project | New project |
| ⌘O | Open project | Open project | Open | Open |
| ⌘I | Import model | Import STL/3MF/STEP/OBJ | — | **Add model** (file dialog) |
| ⌘S | Save project | Save project | Save | Save |
| ⌘⇧S | Save project as | — | Save as | Save as |
| ⌘⌥S | — | Save project as | — | — (Orca uses ⌘⇧S) |
| ⌘R | Slice plate | (Re)slice | Slice plate | Slice plate |
| ⌘⇧R | — | — | — | **Slice all plates** (the ones not sliced yet or changed) |
| ⌘G | Export plate sliced file | Export G-code | Export G-code | Export G-code |
| ⌘⇧G | Print plate | Send G-code | — | **Print…** (send to printer) |
| ⌘, (Mac) / Ctrl+P | Preferences | Preferences | Preferences (⌘,) | Settings (⌘,; Ctrl+P off the Mac) |
| ⌘F | Search settings | Search | — | **Focus the settings search** |
| ⌘J | Print host upload queue | Print host upload queue | — | pending: print-host upload queue |
| ⌘E | Show labels | Export config | — | pending: object labels in the 3D view |
| ⌘L / ⌘⌥L | — | Import / merge config | — | — |
| ⌘U / ⌘T | — | Export to / eject SD card | — | — |
| ⌘⇧I | — | Open new instance | — | — |
| ⌘⇧M | 3Dconnexion settings (Mac) | 3Dconnexion settings (Mac) | — | pending: 3Dconnexion support |
| ⌘M (Mac) | Minimise | Minimise | — | **Minimise** |
| Ctrl+⌘F (Mac) | Full screen | — | — | **Full screen** |
| ⌘Q / ⌘H (Mac) | Quit / hide | Quit / hide (system) | macOS app menu | macOS app menu (unchanged) |
| Ctrl+Tab | Switch tab page | — | — | pending: tab switching by key (⌘Tab is the macOS app switcher) |
| ⌘⇧Enter | Show wireframe | — | — | pending: wireframe view |
| F5 / ⌘⇧R | Reload printer page | Reload from disk | — | pending: reload from disk |
| ? (⇧/) | Keyboard shortcuts | Keyboard shortcuts | — | **Keyboard shortcuts window** |
| Tab | Prepare ↔ Preview | Editor ↔ Preview | Prepare ↔ Preview | Prepare ↔ Preview (slices first if needed) |
| ⇧Tab | Collapse sidebar | Collapse sidebar | — | **Hide / show the sidebar** (also View menu) |
| Tab / ⇧Tab (object with parts selected) | — | — | — | **Next / previous part of the selected object** (modifiers, volumes, height ranges in tree order, then the object again); shadows the two rows above only while such an object is selected in Prepare |

### Edit

| Key | OrcaSlicer | PrusaSlicer | Mica before | Mica after |
|---|---|---|---|---|
| ⌘Z | Undo | Undo | Undo | Undo |
| ⌘Y | Redo | Redo | Redo | Redo |
| ⌘⇧Z | — | — | Redo | Redo (kept: macOS convention) |
| ⌘X | Cut to clipboard | — | — | **Cut** (copy, then delete) |
| ⌘C | Copy | Copy | Copy | Copy |
| ⌘V | Paste | Paste | Paste | Paste (at the pointer over the bed) |
| ⌘A | Select all on the plate | Select all | Select all | Select all on the plate |
| ⌘⇧A | Select all (all plates) | — | — | **Select all on the plate** (Mica's selection is per plate) |
| ⌘D | Delete all | — | Duplicate | **Delete all** on the plate (Orca's key; Duplicate moved to ⌘K) |
| ⌘Del / ⌘⌫ | — | Delete all | — | **Delete all** |
| ⌘K | Clone selected | — | — | **Duplicate** |
| Del / ⌫ | Delete selected (in the cut gizmo: the selected connector; in the measure gizmo: restart) | Delete selected | Delete (connector mode: the connector) | Delete; in the cut tool's connector mode the selected connector; in the measure tool **restart the measurement** (never while typing) |
| Esc | Close gizmo, else deselect all | Unselect gizmo or clear selection | Leave tool, else deselect | **Leave connector mode, then the tool (measure: unselect first; rotate/scale → move), then the section view, then deselect**; in Preview closes the section view |
| Space | Rename (object list) | — | — | **Rename the selected object** |

### Objects and plates (Prepare)

| Key | OrcaSlicer | PrusaSlicer | Mica before | Mica after |
|---|---|---|---|---|
| A | Arrange all objects (every plate) | Arrange | Arrange the current plate | **Arrange every plate** |
| ⇧A | Arrange the current plate | Arrange selection | Arrange all plates | **Arrange the current plate** |
| Q | Auto-orient the selection (else all) | — | — | **Auto-orient the selection** (nothing selected: the plate) |
| ⇧Q | Auto-orient the active plate | — | — | **Auto-orient the plate** |
| + | Add an instance | Add an instance | — | **Add an instance** |
| − | Remove an instance | Remove an instance | — | **Remove an instance** |
| 1–9 | Filament for the selection | Extruder for the selection (object list) | 1–6: camera views | **Filament with a multi-tool selection**, else 1–6 camera views |
| 1 then 0–6 | Filament 10–16 (double tap) | — | — | pending: two-digit filament numbers |
| V | Toggle printable | — | — | pending: printable flag |
| ↑ ↓ ← → | Move 10 mm | Move 10 mm | Move 1 mm | **Move 10 mm** |
| ⇧ + arrows | Move 1 mm | Move 1 mm | Move 10 mm | **Move 1 mm** |
| ⌘ + arrows | Move in camera space | Move in camera space | — | **Move along the view** |
| ↑ ↓ in the cut tool | Move the cut plane 1 mm | — | Moved the object | **Move the cut plane 1 mm** |
| Page Up / Down | Rotate 45° CCW / CW | Rotate 45° CCW / CW | — | **Rotate 45°** |

### Tools (Orca's gizmo keys, Prepare)

| Key | OrcaSlicer | PrusaSlicer | Mica before | Mica after |
|---|---|---|---|---|
| M | Move | Move | Move | Move, **toggles** |
| R | Rotate | Rotate | Rotate | Rotate, **toggles** |
| S | Scale | Scale | Scale | Scale, **toggles** |
| F | Lay on face | Place on face | Lay on face | Lay on face |
| C | Cut (Preview: G-code window) | Cut | Cut | Cut (Preview: pending: G-code window) |
| L | FDM support painting (Preview: one-layer mode) | SLA support points / FDM supports | — | **Support painting** (Preview: one-layer mode) |
| P | Seam painting | Seam painting | Perspective camera | **Seam painting** (perspective moved to K) |
| N | Colour (multi-material) painting | MMU painting | — | **Colour painting** (multi-tool printers) |
| H | Fuzzy-skin painting | SLA hollow / fuzzy-skin painting | — | **Fuzzy-skin painting** |
| B | Mesh boolean | Zoom to bed | — | **Mesh boolean** (Esc closes the card, then deselects) |
| T | Text emboss | Text emboss | — | **Text / SVG tool**: a text on the selected (or only) model, or edits the selected text; Done again, Esc cancels (several models, none selected: a text object) |
| E | Brim ears | Show/hide labels | — | **Brim ears tool** |
| G | — | — (Variable layer height is a toolbar button) | — | **Layer heights tool** (Mica's own key) |
| U | Measure | — (⌘U / measure gizmo button) | — | **Measure tool** (Del restarts, Esc unselects then leaves) |
| Y | Assembly | — | — | pending: assembly view |
| Shift (move gizmo) | Snap by 1 mm | Snap by 1 mm | — | pending: snapping while dragging |
| Alt + scroll (painting) | Move the clipping plane | — | Brush size | Brush size |
| Alt + scroll (section view on) | Move the clipping plane | — | — | **Move the section plane** (Prepare and Preview) |
| ⌘ + scroll (painting) | Brush size | — | — | **Brush size** |

### While painting

| Key | OrcaSlicer | PrusaSlicer | Mica before | Mica after |
|---|---|---|---|---|
| C | Circle brush (supports, seam, colour) | — | (cut tool) | **Circle brush** |
| S | Sphere brush (supports, seam, colour) | — | (scale tool) | **Sphere brush** |
| F | Smart fill (supports, colour) | — | (lay on face) | **Smart fill** |
| H | Height range (colour) | — | — | **Height-range brush** |
| T | Triangle brush (colour) | — | — | pending: triangle brush |
| G | Gap fill (supports, colour) | — | — | pending: gap-fill brush |
| 1–9 | Pick the colour (colour painting) | — | — | pending: colour pick by key |

### Camera (Prepare and Preview)

| Key | OrcaSlicer | PrusaSlicer | Mica before | Mica after |
|---|---|---|---|---|
| ⌘0 | Default view + zoom to plate | — | — | **Default view** |
| ⌘1 … ⌘6 | Top, bottom, front, rear, left, right | Settings tabs / 3D / Preview | — | **Top, bottom, front, rear, left, right** |
| ⌘7 | Plate view | — | — | pending: plate-view camera |
| 0 … 6 | (filament keys) | Camera views | Camera views | Camera views when no multi-tool selection |
| Z | — (disabled in Orca) | Zoom to selection / all | Zoom to selection / plate | Zoom to selection / plate |
| ⇧Z | — | — | Zoom to all plates | Zoom to all plates |
| I / O | Zoom in / out | Zoom in / out | — | **Zoom in / out** (×1.25 per press) |
| K | — (disabled in Orca) | Perspective ↔ orthographic | — | **Perspective ↔ orthographic** |

### Preview

| Key | OrcaSlicer | PrusaSlicer | Mica before | Mica after |
|---|---|---|---|---|
| ↑ / ↓ | Layer slider up / down | Layer slider up / down | Only with the pointer on the slider | **Layer slider, anywhere in Preview** |
| ← / → | Move slider (wraps to the next or previous layer) | Move slider | Only with the pointer on the slider | **Move slider, wrapping between layers** |
| ⇧ or ⌘ + arrows | 5× steps | 5× steps | — | **5× steps** |
| Home / End | First / last move of the layer | — | — | **First / last move** |
| L | One-layer mode | Show / hide legend | — | **One-layer mode** |
| X | — | One-layer mode | — | — (Orca uses L) |
| W S A D | — | Slider thumbs | — | — (Orca's A/S/D mean other things) |
| C | G-code window | G-code window | — | pending: G-code window |
| + / − | Add / remove a colour change at the layer | Add / remove a colour change | — | **Add the slider's "+" item / remove the layer's item** |
| ⇧G / ⌘G | Go to layer | — | — | pending: go-to-layer box (⌘G stays Export) |
| ⇧ or ⌘ + scroll on the slider | 5× steps | 5× steps | — | pending: slider scrolling |

### Mouse

| Gesture | OrcaSlicer | PrusaSlicer | Mica |
|---|---|---|---|
| Left drag | Rotate the view (configurable) | Rotate the view | Rotate the view; on an object, move it |
| Right / middle drag | Pan (configurable) | Pan | Pan |
| Scroll | Zoom | Zoom | Zoom |
| ⇧ drag | Rectangle selection | Rectangle selection | Rectangle selection |
| ⌘ click / ⇧ click | Add to selection | Add to selection | Add to or remove from the selection |
| Alt click | Select a part | — | pending: part pick with Alt (parts are picked in the object list) |
| ⇧ drag (painting) | Erase | Erase | Erase |

## Summary

- **Added:** ⌘I, ⌘⇧G, ⌘F, ⌘X, ⌘K, ⌘⇧A, ⌘Del/⌘⌫, Ctrl+P (off the Mac), ⌘M and
  Ctrl+⌘F (Mac), `?`, ⇧Tab, Space, Q, ⇧Q, `+`, `−`, 1–9 (filament), ⌘ + arrows, Page Up/Down,
  ↑↓ in the cut tool, L, P, N, H (painting tools), T (text / SVG tool), C, S, F, H (brushes), ⌘0–⌘6, I, O, K, and
  in Preview ↑↓←→ anywhere, ⇧/⌘ fast steps, Home, End and L. Also ⌘-scroll for the brush
  size, plus a Help menu and shortcut labels on every menu item and context-menu entry that
  has a key.
- **Changed to Orca's meaning:** A ↔ ⇧A (every plate / current plate), ⌘D (duplicate →
  delete all; duplicate is ⌘K), P (perspective → seam painting; perspective is K), arrows
  (1 mm → 10 mm) and ⇧ + arrows (10 mm → 1 mm), M/R/S now toggle, Esc closes rotate/scale
  before it deselects, Preview arrows no longer need the pointer on the slider.
- **Pending (18):** print-host queue (⌘J), object labels (⌘E), 3Dconnexion (⌘⇧M),
  tab switching (Ctrl+Tab), wireframe (⌘⇧Enter), reload from disk (F5), two-digit filament
  numbers, printable flag (V), assembly (Y), move snapping (Shift), triangle and gap-fill brushes
  (T, G while painting), colour pick by key, plate-view camera (⌘7), G-code window (C in
  Preview), go to layer (⇧G), slider scrolling, part pick with Alt.

## Tests

`crates/mica-gui/src/keymap.rs` tests:

- No chord is bound twice in a scope.
- Orca's keys map to Orca's actions in Prepare, Preview, painting and the Machine view.
- Menus print the bound chord.
- The window merges aliases and searches.
- Real frames through `App::show`:
  - M/R/S toggle, Esc closes and then deselects, L/S/Esc work while painting, C opens and
    closes cut.
  - `+`/`−`, ⌘K, ⌘A, 10 mm/1 mm nudges, ⌘D, ⌘Z, ⌘1, K, ⇧Tab and `?` do what they say.
  - With the settings search focused, R, A, Del, ⌫, `+` and Esc do nothing.
  - The Preview slider rules hold (fast steps, wrap, one-layer mode).
- The GUI monkey presses every key in the table with random modifiers.
