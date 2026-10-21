// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: orca@8500fcdcca src/slic3r/GUI/KBShortcutsDialog.cpp :: KBShortcutsDialog::fill_shortcuts
// Source: orca@8500fcdcca src/slic3r/GUI/GLCanvas3D.cpp :: GLCanvas3D::on_char
// Source: orca@8500fcdcca src/slic3r/GUI/GLCanvas3D.cpp :: GLCanvas3D::on_key
// Source: orca@8500fcdcca src/slic3r/GUI/Gizmos/GLGizmosManager.cpp :: GLGizmosManager::on_char
// Source: orca@8500fcdcca src/slic3r/GUI/Gizmos/GLGizmosManager.cpp :: GLGizmosManager::on_key
// Source: orca@8500fcdcca src/slic3r/GUI/Gizmos/GLGizmosManager.cpp :: GLGizmosManager::handle_shortcut
// Source: orca@8500fcdcca src/slic3r/GUI/MainFrame.cpp :: MainFrame::MainFrame
// Source: orca@8500fcdcca src/slic3r/GUI/GUI_Preview.cpp :: Preview::update_layers_slider_from_canvas
// Source: orca@8500fcdcca src/slic3r/GUI/IMSlider.cpp :: IMSlider::switch_one_layer_mode
// Source: prusaslicer@b028299c77 src/slic3r/GUI/KBShortcutsDialog.cpp :: KBShortcutsDialog::fill_shortcuts
// Changes: Only the key assignments are taken (Orca first, PrusaSlicer where Orca has none: Z,
// K, Cmd+Delete); one table drives the key handling, the menus' shortcut labels and the
// shortcuts window. Single-letter keys never fire while a text field has focus; digits pick a
// filament only with a selection on a multi-tool printer and otherwise the camera views (as in
// PrusaSlicer); one-layer mode and the move slider follow Orca's rules on Mica's `View`.
// Written from scratch for egui.

//! Every keyboard shortcut in one place. [`BINDINGS`] maps a key chord in a scope (anywhere,
//! Prepare, a painting tool, Preview) to an [`Act`]; [`collect`] turns this frame's key presses
//! into acts, [`App::run_key`] carries out the app-level ones and hands the 3D-view ones
//! (camera, tools, nudges, Esc) to the viewport, and [`preview_keys`] moves the Preview
//! sliders. The same table labels the menus ([`shortcut_text`]) and fills the "Keyboard
//! shortcuts" window (`?`). See docs/shortcuts.md for the comparison with Orca and PrusaSlicer.

use crate::{app::App, paint::BrushKind, preview, viewport::gizmo};
use eframe::egui::{self, Key};
use mica_core::painting::PaintKind;

/// A tool opened with a single letter, as in Orca's gizmo bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Move,
    Rotate,
    Scale,
    LayOnFace,
    Cut,
    /// Mesh boolean card (Orca's GLGizmoMeshBoolean, key B).
    MeshBoolean,
    /// The Text / SVG emboss tool (Orca's Emboss gizmo, T); opens without a selection too.
    Emboss,
    /// The measure tool (Orca's and PrusaSlicer's Measure gizmo, U); no selection needed.
    Measure,
    Paint(PaintKind),
    /// The variable layer height tool (no key upstream; G here).
    LayerHeights,
    /// The brim ears tool (Orca's brim ears gizmo, E).
    BrimEars,
}

/// What a shortcut does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    NewProject,
    Open,
    Import,
    Save,
    SaveAs,
    Slice,
    /// Slice every plate that needs it (⇧⌘R).
    SliceAll,
    ExportGcode,
    Print,
    Preferences,
    SearchSettings,
    ShowShortcuts,
    TogglePreview,
    CollapseSidebar,
    Fullscreen,
    Minimize,
    Undo,
    Redo,
    CutToClipboard,
    Copy,
    Paste,
    SelectAll,
    Delete,
    DeleteAll,
    Clone,
    AddInstance,
    RemoveInstance,
    ArrangeAll,
    ArrangePlate,
    Orient,
    OrientPlate,
    Rename,
    /// F2: rename the current plate (in the plate bar).
    RenamePlate,
    /// A plain digit: a filament for the selection, else a camera view (resolved by the app).
    Digit(u8),
    /// Camera view 0–6 (default, top, bottom, front, rear, left, right).
    Camera(u8),
    /// Zoom to the selection (or the plate); `all`: every plate.
    ZoomFit {
        all: bool,
    },
    ZoomIn,
    ZoomOut,
    TogglePerspective,
    Tool(Tool),
    Brush(BrushKind),
    Escape,
    /// Arrow keys: move the selection by `dx`, `dy` steps.
    Nudge {
        dx: i8,
        dy: i8,
        fine: bool,
        camera: bool,
    },
    /// Page Up / Page Down: turn the selection by 45° (positive = counter-clockwise).
    Turn(i8),
    Layer {
        up: bool,
        fast: bool,
    },
    Moves {
        forward: bool,
        fast: bool,
    },
    MovesStart,
    MovesEnd,
    OneLayer,
    /// Add (`+`) or remove (`−`) the custom G-code item at the upper handle's layer, as
    /// Orca's layer slider does (a colour change, or what the slider's "+" adds).
    LayerGCode {
        add: bool,
    },
    /// Tab / Shift+Tab with an object that has parts selected: the next (or previous) part
    /// in the object tree, back to the object itself past the end.
    CyclePart {
        back: bool,
    },
}

/// Where a binding is live.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// Everywhere, also in the Machine view.
    Global,
    /// Prepare and Preview (the 3D view is showing).
    View,
    /// Prepare only.
    Prepare,
    /// In a painting tool (bit per `PaintKind` index); shadows Prepare bindings.
    Paint(u8),
    /// Preview only; shadows View bindings.
    Preview,
    /// Prepare with a selected object that has parts (modifiers, volumes, height ranges);
    /// shadows the Tab bindings of the other scopes.
    Parts,
}

/// Which platforms a binding exists on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Any,
    Mac,
    NotMac,
}

/// Modifier set: ⌘ (Ctrl off the Mac), Shift, Alt and, on the Mac, Control.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mods(u8);
impl Mods {
    pub const NONE: Self = Self(0);
    pub const CMD: Self = Self(1);
    pub const SHIFT: Self = Self(2);
    pub const ALT: Self = Self(4);
    pub const CTRL: Self = Self(8);
    pub const CMD_SHIFT: Self = Self(3);
    pub const CMD_CTRL: Self = Self(9);
    pub const CMD_ALT: Self = Self(5);
    pub fn of(m: egui::Modifiers) -> Self {
        // On the Mac `command` is ⌘ and `ctrl` the Control key; elsewhere both are Ctrl.
        let ctrl = m.ctrl && (!m.command || m.mac_cmd);
        Self(m.command as u8 | (m.shift as u8) << 1 | (m.alt as u8) << 2 | (ctrl as u8) << 3)
    }
    pub fn has(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
    fn without_shift(self) -> Self {
        Self(self.0 & !2)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    File,
    Edit,
    Objects,
    Camera,
    Tools,
    Painting,
    Preview,
}
impl Group {
    pub fn title(self) -> &'static str {
        match self {
            Group::File => "File and window",
            Group::Edit => "Edit",
            Group::Objects => "Objects and plates",
            Group::Camera => "Camera",
            Group::Tools => "Tools",
            Group::Painting => "While painting",
            Group::Preview => "Preview",
        }
    }
}

/// One key chord and what it does.
#[derive(Clone, Copy, Debug)]
pub struct Binding {
    pub key: Key,
    pub mods: Mods,
    pub scope: Scope,
    pub act: Act,
    pub platform: Platform,
    /// Key repeat fires it again (arrows, +/−, zoom).
    pub repeat: bool,
    pub group: Group,
    /// What it does, in plain words, for the shortcuts window.
    pub help: &'static str,
    /// Key text for the shortcuts window instead of the chord ("1–9", "Arrows").
    pub keys: &'static str,
}

const fn b(
    key: Key,
    mods: Mods,
    scope: Scope,
    act: Act,
    group: Group,
    help: &'static str,
) -> Binding {
    Binding {
        key,
        mods,
        scope,
        act,
        platform: Platform::Any,
        repeat: false,
        group,
        help,
        keys: "",
    }
}
const fn rep(mut x: Binding) -> Binding {
    x.repeat = true;
    x
}
const fn on(platform: Platform, mut x: Binding) -> Binding {
    x.platform = platform;
    x
}
const fn shown(keys: &'static str, mut x: Binding) -> Binding {
    x.keys = keys;
    x
}

const SUPPORTS: u8 = 1 << 0;
const SEAM: u8 = 1 << 1;
const MATERIAL: u8 = 1 << 2;
const FUZZY: u8 = 1 << 3;
const TEXTURE: u8 = 1 << 4;
fn paint_bit(kind: PaintKind) -> u8 {
    match kind {
        PaintKind::Supports => SUPPORTS,
        PaintKind::Seam => SEAM,
        PaintKind::Material => MATERIAL,
        PaintKind::Fuzzy => FUZZY,
        PaintKind::Texture => TEXTURE,
    }
}

use Act as A;
use Group as G;
use Mods as M;
use Scope as S;

/// Every shortcut, in the order the shortcuts window lists them. The first binding of an act
/// is the one menus show.
#[rustfmt::skip]
pub const BINDINGS: &[Binding] = &[
    // File and window.
    b(Key::N, M::CMD, S::Global, A::NewProject, G::File, "New project"),
    b(Key::O, M::CMD, S::Global, A::Open, G::File, "Open a project or model"),
    b(Key::I, M::CMD, S::Global, A::Import, G::File, "Add a model to the plate"),
    b(Key::S, M::CMD, S::Global, A::Save, G::File, "Save the project"),
    b(Key::S, M::CMD_SHIFT, S::Global, A::SaveAs, G::File, "Save the project under a new name or format"),
    b(Key::R, M::CMD, S::Global, A::Slice, G::File, "Slice the current plate"),
    b(Key::R, M::CMD_SHIFT, S::Global, A::SliceAll, G::File, "Slice all plates (the ones not sliced yet or changed)"),
    b(Key::G, M::CMD, S::Global, A::ExportGcode, G::File, "Export the sliced plate's G-code"),
    b(Key::G, M::CMD_SHIFT, S::Global, A::Print, G::File, "Send the sliced plate to a printer"),
    b(Key::Comma, M::CMD, S::Global, A::Preferences, G::File, "Settings"),
    on(Platform::NotMac, b(Key::P, M::CMD, S::Global, A::Preferences, G::File, "Settings")),
    b(Key::F, M::CMD, S::Global, A::SearchSettings, G::File, "Search the settings"),
    b(Key::Tab, M::NONE, S::Parts, A::CyclePart { back: false }, G::Objects, "With an object that has parts selected: select its next part (after the last, the object again)"),
    b(Key::Tab, M::SHIFT, S::Parts, A::CyclePart { back: true }, G::Objects, "With an object that has parts selected: select its previous part"),
    b(Key::Tab, M::NONE, S::Global, A::TogglePreview, G::File, "Switch between Prepare and Preview"),
    b(Key::Tab, M::SHIFT, S::View, A::CollapseSidebar, G::File, "Hide or show the sidebar"),
    on(Platform::Mac, b(Key::F, M::CMD_CTRL, S::Global, A::Fullscreen, G::File, "Full screen on or off")),
    on(Platform::Mac, b(Key::M, M::CMD, S::Global, A::Minimize, G::File, "Minimize the window")),
    b(Key::Questionmark, M::NONE, S::Global, A::ShowShortcuts, G::File, "Show this list of keyboard shortcuts"),
    b(Key::Slash, M::SHIFT, S::Global, A::ShowShortcuts, G::File, "Show this list of keyboard shortcuts"),
    // Edit.
    b(Key::Z, M::CMD, S::Global, A::Undo, G::Edit, "Undo"),
    b(Key::Y, M::CMD, S::Global, A::Redo, G::Edit, "Redo"),
    b(Key::Z, M::CMD_SHIFT, S::Global, A::Redo, G::Edit, "Redo"),
    b(Key::X, M::CMD, S::Prepare, A::CutToClipboard, G::Edit, "Cut the selection to the clipboard"),
    b(Key::C, M::CMD, S::Prepare, A::Copy, G::Edit, "Copy the selection"),
    b(Key::V, M::CMD, S::Prepare, A::Paste, G::Edit, "Paste (at the pointer when it is over the bed)"),
    b(Key::K, M::CMD, S::Prepare, A::Clone, G::Edit, "Duplicate the selection"),
    b(Key::A, M::CMD, S::Prepare, A::SelectAll, G::Edit, "Select every object on the plate"),
    b(Key::A, M::CMD_SHIFT, S::Prepare, A::SelectAll, G::Edit, "Select every object on the plate"),
    b(Key::Delete, M::NONE, S::Prepare, A::Delete, G::Edit, "Delete the selection (in the cut tool's connector mode: the selected connector)"),
    b(Key::Backspace, M::NONE, S::Prepare, A::Delete, G::Edit, "Delete the selection (in the cut tool's connector mode: the selected connector)"),
    b(Key::D, M::CMD, S::Prepare, A::DeleteAll, G::Edit, "Delete every object on the plate"),
    b(Key::Delete, M::CMD, S::Prepare, A::DeleteAll, G::Edit, "Delete every object on the plate"),
    b(Key::Backspace, M::CMD, S::Prepare, A::DeleteAll, G::Edit, "Delete every object on the plate"),
    b(Key::Escape, M::NONE, S::Prepare, A::Escape, G::Edit, "Leave the active tool, then clear the selection"),
    // Objects and plates.
    b(Key::A, M::NONE, S::Prepare, A::ArrangeAll, G::Objects, "Arrange the objects of every plate"),
    b(Key::A, M::SHIFT, S::Prepare, A::ArrangePlate, G::Objects, "Arrange the current plate"),
    b(Key::Q, M::NONE, S::Prepare, A::Orient, G::Objects, "Auto-orient the selection (nothing selected: the whole plate)"),
    b(Key::Q, M::SHIFT, S::Prepare, A::OrientPlate, G::Objects, "Auto-orient every object on the plate"),
    rep(b(Key::Plus, M::NONE, S::Prepare, A::AddInstance, G::Objects, "Add an instance of the selection")),
    rep(b(Key::Minus, M::NONE, S::Prepare, A::RemoveInstance, G::Objects, "Remove an instance of the selection")),
    b(Key::Space, M::NONE, S::Prepare, A::Rename, G::Objects, "Rename the selected object"),
    b(Key::F2, M::NONE, S::Prepare, A::RenamePlate, G::Objects, "Rename the current plate"),
    shown("1–9", b(Key::Num1, M::NONE, S::View, A::Digit(1), G::Objects, "With a selection on a multi-tool printer: print it with that filament (otherwise 0–6 pick camera views)")),
    b(Key::Num2, M::NONE, S::View, A::Digit(2), G::Objects, "With a selection on a multi-tool printer: print it with that filament (otherwise 0–6 pick camera views)"),
    b(Key::Num3, M::NONE, S::View, A::Digit(3), G::Objects, "With a selection on a multi-tool printer: print it with that filament (otherwise 0–6 pick camera views)"),
    b(Key::Num4, M::NONE, S::View, A::Digit(4), G::Objects, "With a selection on a multi-tool printer: print it with that filament (otherwise 0–6 pick camera views)"),
    b(Key::Num5, M::NONE, S::View, A::Digit(5), G::Objects, "With a selection on a multi-tool printer: print it with that filament (otherwise 0–6 pick camera views)"),
    b(Key::Num6, M::NONE, S::View, A::Digit(6), G::Objects, "With a selection on a multi-tool printer: print it with that filament (otherwise 0–6 pick camera views)"),
    b(Key::Num7, M::NONE, S::View, A::Digit(7), G::Objects, "With a selection on a multi-tool printer: print it with that filament (otherwise 0–6 pick camera views)"),
    b(Key::Num8, M::NONE, S::View, A::Digit(8), G::Objects, "With a selection on a multi-tool printer: print it with that filament (otherwise 0–6 pick camera views)"),
    b(Key::Num9, M::NONE, S::View, A::Digit(9), G::Objects, "With a selection on a multi-tool printer: print it with that filament (otherwise 0–6 pick camera views)"),
    shown("Arrows", rep(b(Key::ArrowUp, M::NONE, S::Prepare, A::Nudge { dx: 0, dy: 1, fine: false, camera: false, }, G::Objects, "Move the selection 10 mm (in the cut tool ↑ ↓ move the cut plane)"))),
    rep(b(Key::ArrowDown, M::NONE, S::Prepare, A::Nudge { dx: 0, dy: -1, fine: false, camera: false, }, G::Objects, "Move the selection 10 mm (in the cut tool ↑ ↓ move the cut plane)")),
    rep(b(Key::ArrowLeft, M::NONE, S::Prepare, A::Nudge { dx: -1, dy: 0, fine: false, camera: false, }, G::Objects, "Move the selection 10 mm (in the cut tool ↑ ↓ move the cut plane)")),
    rep(b(Key::ArrowRight, M::NONE, S::Prepare, A::Nudge { dx: 1, dy: 0, fine: false, camera: false, }, G::Objects, "Move the selection 10 mm (in the cut tool ↑ ↓ move the cut plane)")),
    shown("Shift+Arrows", rep(b(Key::ArrowUp, M::SHIFT, S::Prepare, A::Nudge { dx: 0, dy: 1, fine: true, camera: false, }, G::Objects, "Move the selection 1 mm"))),
    rep(b(Key::ArrowDown, M::SHIFT, S::Prepare, A::Nudge { dx: 0, dy: -1, fine: true, camera: false, }, G::Objects, "Move the selection 1 mm")),
    rep(b(Key::ArrowLeft, M::SHIFT, S::Prepare, A::Nudge { dx: -1, dy: 0, fine: true, camera: false, }, G::Objects, "Move the selection 1 mm")),
    rep(b(Key::ArrowRight, M::SHIFT, S::Prepare, A::Nudge { dx: 1, dy: 0, fine: true, camera: false, }, G::Objects, "Move the selection 1 mm")),
    shown(CMD_ARROWS, rep(b(Key::ArrowUp, M::CMD, S::Prepare, A::Nudge { dx: 0, dy: 1, fine: false, camera: true, }, G::Objects, "Move the selection 10 mm relative to the view"))),
    rep(b(Key::ArrowDown, M::CMD, S::Prepare, A::Nudge { dx: 0, dy: -1, fine: false, camera: true, }, G::Objects, "Move the selection 10 mm relative to the view")),
    rep(b(Key::ArrowLeft, M::CMD, S::Prepare, A::Nudge { dx: -1, dy: 0, fine: false, camera: true, }, G::Objects, "Move the selection 10 mm relative to the view")),
    rep(b(Key::ArrowRight, M::CMD, S::Prepare, A::Nudge { dx: 1, dy: 0, fine: false, camera: true, }, G::Objects, "Move the selection 10 mm relative to the view")),
    rep(b(Key::PageUp, M::NONE, S::Prepare, A::Turn(1), G::Objects, "Turn the selection 45° counter-clockwise")),
    rep(b(Key::PageDown, M::NONE, S::Prepare, A::Turn(-1), G::Objects, "Turn the selection 45° clockwise")),
    // Tools.
    b(Key::M, M::NONE, S::Prepare, A::Tool(Tool::Move), G::Tools, "Move tool (press again to close it)"),
    b(Key::R, M::NONE, S::Prepare, A::Tool(Tool::Rotate), G::Tools, "Rotate tool (press again to close it)"),
    b(Key::S, M::NONE, S::Prepare, A::Tool(Tool::Scale), G::Tools, "Scale tool (press again to close it)"),
    b(Key::F, M::NONE, S::Prepare, A::Tool(Tool::LayOnFace), G::Tools, "Lay on face: click a face to put it on the bed"),
    b(Key::C, M::NONE, S::Prepare, A::Tool(Tool::Cut), G::Tools, "Cut tool"),
    b(Key::B, M::NONE, S::Prepare, A::Tool(Tool::MeshBoolean), G::Tools, "Mesh boolean: join, subtract or intersect with another shape"),
    b(Key::T, M::NONE, S::Prepare, A::Tool(Tool::Emboss), G::Tools, "Text and SVG: add text to the selection (or a text object), or edit the selected text"),
    b(Key::U, M::NONE, S::Prepare, A::Tool(Tool::Measure), G::Tools, "Measure: distances, angles and diameters between points, edges, circles and faces (Delete restarts, Esc unselects)"),
    b(Key::L, M::NONE, S::Prepare, A::Tool(Tool::Paint(PaintKind::Supports)), G::Tools, "Paint supports on or off"),
    b(Key::P, M::NONE, S::Prepare, A::Tool(Tool::Paint(PaintKind::Seam)), G::Tools, "Paint the seam on or off"),
    b(Key::N, M::NONE, S::Prepare, A::Tool(Tool::Paint(PaintKind::Material)), G::Tools, "Paint colors on or off (multi-tool printers)"),
    b(Key::H, M::NONE, S::Prepare, A::Tool(Tool::Paint(PaintKind::Fuzzy)), G::Tools, "Paint fuzzy skin on or off"),
    b(Key::X, M::NONE, S::Prepare, A::Tool(Tool::Paint(PaintKind::Texture)), G::Tools, "Paint textures on or off"),
    b(Key::G, M::NONE, S::Prepare, A::Tool(Tool::LayerHeights), G::Tools, "Layer heights tool on or off"),
    b(Key::E, M::NONE, S::Prepare, A::Tool(Tool::BrimEars), G::Tools, "Brim ears tool on or off"),
    // While painting (Orca's per-tool brush keys).
    b(Key::C, M::NONE, S::Paint(SUPPORTS | SEAM | MATERIAL), A::Brush(BrushKind::Circle), G::Painting, "Circle brush"),
    b(Key::S, M::NONE, S::Paint(SUPPORTS | SEAM | MATERIAL), A::Brush(BrushKind::Sphere), G::Painting, "Sphere brush"),
    b(Key::F, M::NONE, S::Paint(SUPPORTS | MATERIAL), A::Brush(BrushKind::Fill), G::Painting, "Smart fill (support and color painting)"),
    b(Key::H, M::NONE, S::Paint(MATERIAL), A::Brush(BrushKind::Height), G::Painting, "Height range (color painting)"),
    // Camera.
    b(Key::Num0, M::CMD, S::View, A::Camera(0), G::Camera, "Default view"),
    b(Key::Num1, M::CMD, S::View, A::Camera(1), G::Camera, "Top view"),
    b(Key::Num2, M::CMD, S::View, A::Camera(2), G::Camera, "Bottom view"),
    b(Key::Num3, M::CMD, S::View, A::Camera(3), G::Camera, "Front view"),
    b(Key::Num4, M::CMD, S::View, A::Camera(4), G::Camera, "Rear view"),
    b(Key::Num5, M::CMD, S::View, A::Camera(5), G::Camera, "Left view"),
    b(Key::Num6, M::CMD, S::View, A::Camera(6), G::Camera, "Right view"),
    b(Key::Num0, M::NONE, S::View, A::Digit(0), G::Camera, "Default view (also 1–6 for the views below when nothing is selected)"),
    b(Key::Z, M::NONE, S::View, A::ZoomFit { all: false }, G::Camera, "Zoom to the selection (nothing selected: the plate)"),
    b(Key::Z, M::SHIFT, S::View, A::ZoomFit { all: true }, G::Camera, "Zoom to all plates"),
    rep(b(Key::I, M::NONE, S::View, A::ZoomIn, G::Camera, "Zoom in")),
    rep(b(Key::O, M::NONE, S::View, A::ZoomOut, G::Camera, "Zoom out")),
    b(Key::K, M::NONE, S::View, A::TogglePerspective, G::Camera, "Perspective or orthographic camera"),
    // Preview.
    rep(b(Key::ArrowUp, M::NONE, S::Preview, A::Layer { up: true, fast: false, }, G::Preview, "Layer slider up one layer")),
    rep(b(Key::ArrowDown, M::NONE, S::Preview, A::Layer { up: false, fast: false, }, G::Preview, "Layer slider down one layer")),
    rep(b(Key::ArrowLeft, M::NONE, S::Preview, A::Moves { forward: false, fast: false, }, G::Preview, "Move slider back one move (at the start: the layer below)")),
    rep(b(Key::ArrowRight, M::NONE, S::Preview, A::Moves { forward: true, fast: false, }, G::Preview, "Move slider on one move (at the end: the layer above)")),
    shown(if cfg!(target_os = "macos") { "Shift or Cmd + Arrows" } else { "Shift or Ctrl + Arrows" }, rep(b(Key::ArrowUp, M::SHIFT, S::Preview, A::Layer { up: true, fast: true, }, G::Preview, "Sliders five steps at a time"))),
    rep(b(Key::ArrowDown, M::SHIFT, S::Preview, A::Layer { up: false, fast: true, }, G::Preview, "Sliders five steps at a time")),
    rep(b(Key::ArrowLeft, M::SHIFT, S::Preview, A::Moves { forward: false, fast: true, }, G::Preview, "Sliders five steps at a time")),
    rep(b(Key::ArrowRight, M::SHIFT, S::Preview, A::Moves { forward: true, fast: true, }, G::Preview, "Sliders five steps at a time")),
    rep(b(Key::ArrowUp, M::CMD, S::Preview, A::Layer { up: true, fast: true, }, G::Preview, "Sliders five steps at a time")),
    rep(b(Key::ArrowDown, M::CMD, S::Preview, A::Layer { up: false, fast: true, }, G::Preview, "Sliders five steps at a time")),
    rep(b(Key::ArrowLeft, M::CMD, S::Preview, A::Moves { forward: false, fast: true, }, G::Preview, "Sliders five steps at a time")),
    rep(b(Key::ArrowRight, M::CMD, S::Preview, A::Moves { forward: true, fast: true, }, G::Preview, "Sliders five steps at a time")),
    b(Key::Home, M::NONE, S::Preview, A::MovesStart, G::Preview, "Move slider to the first move of the layer"),
    b(Key::End, M::NONE, S::Preview, A::MovesEnd, G::Preview, "Move slider to the last move of the layer"),
    b(Key::L, M::NONE, S::Preview, A::OneLayer, G::Preview, "One-layer mode on or off"),
    b(Key::Plus, M::NONE, S::Preview, A::LayerGCode { add: true }, G::Preview, "Add a color change at the layer"),
    b(Key::Minus, M::NONE, S::Preview, A::LayerGCode { add: false }, G::Preview, "Remove the color change (or other layer G-code) at the layer"),
];

const CMD_ARROWS: &str = if cfg!(target_os = "macos") {
    "Cmd+Arrows"
} else {
    "Ctrl+Arrows"
};

/// Mouse and trackpad gestures, for the shortcuts window only ("Cmd" is Ctrl off the Mac,
/// see [`localise`]).
pub const MOUSE: &[(&str, &str)] = &[
    ("Left drag", "Rotate the view (on an object: move it)"),
    ("Right or middle drag", "Pan the view"),
    ("Scroll", "Zoom"),
    ("Shift or Cmd click", "Add to or remove from the selection"),
    ("Shift drag", "Select objects with a rectangle"),
    ("Right click", "Menu for the object or the bed"),
    ("Shift drag", "While painting: erase paint"),
    ("Alt or Cmd scroll", "While painting: brush size"),
];

/// "Cmd" reads "Ctrl" off the Mac.
pub fn localise(text: &str) -> String {
    if cfg!(target_os = "macos") {
        text.into()
    } else {
        text.replace("Cmd", "Ctrl")
    }
}

impl Binding {
    fn available(&self) -> bool {
        match self.platform {
            Platform::Any => true,
            Platform::Mac => cfg!(target_os = "macos"),
            Platform::NotMac => !cfg!(target_os = "macos"),
        }
    }
    /// The chord as menus print it: `Cmd+Shift+S` on the Mac, `Ctrl+Shift+S` elsewhere.
    pub fn chord(&self) -> String {
        chord_text(self.key, self.mods)
    }
}

fn key_name(key: Key) -> String {
    match key {
        Key::Delete => "Del".into(),
        Key::Backspace => "Backspace".into(),
        Key::Escape => "Esc".into(),
        Key::ArrowUp => "↑".into(),
        Key::ArrowDown => "↓".into(),
        Key::ArrowLeft => "←".into(),
        Key::ArrowRight => "→".into(),
        Key::PageUp => "Page Up".into(),
        Key::PageDown => "Page Down".into(),
        Key::Questionmark => "?".into(),
        Key::Plus => "+".into(),
        Key::Minus => "−".into(),
        Key::Comma => ",".into(),
        Key::Slash => "/".into(),
        k => k.symbol_or_name().into(),
    }
}

/// Spelled out: the bundled fonts have no ⌘ glyph.
pub fn chord_text(key: Key, mods: Mods) -> String {
    let mut s = String::new();
    if cfg!(target_os = "macos") {
        for (m, name) in [
            (M::CTRL, "Ctrl+"),
            (M::CMD, "Cmd+"),
            (M::ALT, "Option+"),
            (M::SHIFT, "Shift+"),
        ] {
            if mods.has(m) {
                s.push_str(name);
            }
        }
    } else {
        for (m, name) in [(M::CMD, "Ctrl+"), (M::ALT, "Alt+"), (M::SHIFT, "Shift+")] {
            if mods.has(m) {
                s.push_str(name);
            }
        }
    }
    s + &key_name(key)
}

/// The binding a menu item shows for `act`: its first one on this platform.
pub fn first_binding(act: Act) -> Option<&'static Binding> {
    BINDINGS.iter().find(|x| x.act == act && x.available())
}

/// The shortcut a menu item shows for `act` (empty when it has none here).
pub fn shortcut_text(act: Act) -> String {
    first_binding(act).map(Binding::chord).unwrap_or_default()
}

/// What decides which bindings are live this frame.
#[derive(Clone, Copy, Debug, Default)]
pub struct Context {
    pub preview: bool,
    pub machine: bool,
    pub painting: Option<PaintKind>,
    /// The shortcuts window is open: only `?` (to close it) works.
    pub dialog: bool,
    /// The macOS menu bar is up: the chords it shows are its own (`menu::native_owns`), so
    /// they never fire twice.
    pub native_menu: bool,
    /// Prepare has a selected object with parts: Tab cycles through them.
    pub parts: bool,
}
impl Context {
    fn live(&self, scope: Scope) -> bool {
        match scope {
            S::Global => true,
            S::View => !self.machine,
            S::Prepare => !self.machine && !self.preview,
            S::Paint(mask) => {
                !self.machine
                    && !self.preview
                    && self.painting.is_some_and(|k| mask & paint_bit(k) != 0)
            }
            S::Preview => !self.machine && self.preview,
            S::Parts => !self.machine && !self.preview && self.painting.is_none() && self.parts,
        }
    }
}
fn priority(scope: Scope) -> u8 {
    match scope {
        S::Paint(_) => 0,
        S::Preview => 1,
        S::Parts => 2,
        S::Prepare => 3,
        S::View => 4,
        S::Global => 5,
    }
}

/// The binding a key press triggers in `cx`, if any.
pub fn lookup(
    key: Key,
    mods: egui::Modifiers,
    repeat: bool,
    cx: Context,
) -> Option<&'static Binding> {
    let mods = Mods::of(mods);
    BINDINGS
        .iter()
        .filter(|x| x.key == key && x.available() && cx.live(x.scope))
        .filter(|x| {
            if matches!(key, Key::Questionmark | Key::Plus) {
                mods.without_shift() == x.mods
            } else {
                mods == x.mods
            }
        })
        .filter(|x| !repeat || x.repeat)
        .filter(|x| !cx.dialog || x.act == A::ShowShortcuts)
        .filter(|x| !cx.native_menu || !crate::menu::native_owns(x.key, x.mods))
        .min_by_key(|x| priority(x.scope))
}

/// Whether the keyboard belongs to a widget: a text or number field is being typed into, so
/// shortcuts stay out of the way (also on the frame egui drops the focus on Esc).
pub fn typing(ctx: &egui::Context) -> bool {
    ctx.egui_wants_keyboard_input() || crate::panels::widgets::field_editing(ctx)
}

/// This frame's shortcuts, in the order the keys were pressed.
pub fn collect(ctx: &egui::Context, cx: Context) -> Vec<Act> {
    // egui drops a text field's focus on Esc before anything runs, so a field that had the
    // keyboard on the previous frame still owns this frame's keys (Esc must not deselect).
    let id = egui::Id::unique("mica-keymap-typing");
    let was = ctx.data(|d| d.get_temp::<bool>(id)).unwrap_or(false);
    let now = typing(ctx);
    ctx.data_mut(|d| d.insert_temp(id, now));
    if now || was {
        return vec![];
    }
    let acts: Vec<Act> = ctx.input(|i| {
        i.events
            .iter()
            .filter_map(|e| match e {
                egui::Event::Key {
                    key,
                    pressed: true,
                    repeat,
                    modifiers,
                    ..
                } => lookup(*key, *modifiers, *repeat, cx).map(|x| x.act),
                _ => None,
            })
            .collect()
    });
    if !acts.is_empty() {
        // Tab and the arrows also move egui's keyboard focus; a shortcut must not hand the
        // keyboard to some widget (which would then swallow the next shortcut).
        ctx.memory_mut(|m| m.move_focus(egui::FocusDirection::None));
    }
    acts
}

/// Every key the table uses (for the monkey's random key presses).
pub fn keys() -> Vec<Key> {
    let mut out: Vec<Key> = BINDINGS.iter().map(|x| x.key).collect();
    out.sort_by_key(|k| k.name());
    out.dedup();
    out
}

/// One row of the shortcuts window: the keys and what they do.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub group: Group,
    pub keys: String,
    pub help: &'static str,
}

/// The rows of the shortcuts window: bindings with the same text in a group share a row
/// (`Del / ⌫`), a binding's own key text (`1–9`) stands for the bindings after it.
pub fn rows() -> Vec<Row> {
    let mut out: Vec<Row> = vec![];
    let mut fixed: Vec<(Group, &str)> = vec![];
    for x in BINDINGS.iter().filter(|x| x.available()) {
        if let Some(r) = out
            .iter_mut()
            .find(|r| r.group == x.group && r.help == x.help)
        {
            if !fixed.contains(&(x.group, x.help)) {
                let chord = x.chord();
                if !r.keys.split(" / ").any(|k| k == chord) {
                    r.keys = format!("{} / {chord}", r.keys);
                }
            }
            continue;
        }
        if !x.keys.is_empty() {
            fixed.push((x.group, x.help));
        }
        out.push(Row {
            group: x.group,
            keys: if x.keys.is_empty() {
                x.chord()
            } else {
                x.keys.into()
            },
            help: x.help,
        });
    }
    out
}

/// Rows matching a search (keys or description, case-insensitive).
pub fn search(rows: &[Row], query: &str) -> Vec<Row> {
    let q = query.trim().to_lowercase();
    rows.iter()
        .filter(|r| {
            q.is_empty()
                || r.help.to_lowercase().contains(&q)
                || r.keys.to_lowercase().contains(&q)
                || r.group.title().to_lowercase().contains(&q)
        })
        .cloned()
        .collect()
}

/// State the shortcuts own across frames.
#[derive(Default)]
pub struct KeyState {
    /// The "Keyboard shortcuts" window is open.
    pub dialog: bool,
    pub search: String,
    /// Focus the window's search field on its next frame.
    pub focus_dialog_search: bool,
    /// Focus the settings search on the sidebar's next frame (⌘F).
    pub focus_settings_search: bool,
    /// Shift+Tab hid the sidebar.
    pub sidebar_hidden: bool,
}

/// Camera step of I / O, as Orca's `_update_camera_zoom(±1)` (×1.25 per press).
pub const ZOOM_STEP: f32 = 111.57;

/// Applies the Preview acts to the sliders. Returns true when the view changed.
pub fn preview_keys(
    acts: &[Act],
    layers: &[preview::Layer],
    view: &mut preview::View,
    one_layer: &mut bool,
) -> bool {
    if layers.is_empty() {
        return false;
    }
    let max = layers.len() - 1;
    let before = (view.low, view.high, view.moves);
    view.high = view.high.min(max);
    view.low = view.low.min(view.high);
    let count = |high: usize| layers[high].count;
    for &act in acts {
        match act {
            A::Layer { up, fast } => {
                let step = if fast { 5 } else { 1 };
                view.high = if up {
                    (view.high + step).min(max)
                } else {
                    view.high
                        .saturating_sub(step)
                        .max(if *one_layer { 0 } else { view.low })
                };
                view.moves = u64::MAX;
            }
            A::Moves { forward, fast } => {
                let step = if fast { 5 } else { 1 };
                let n = count(view.high);
                let cur = view.moves.min(n);
                if forward {
                    if cur >= n && view.high < max {
                        view.high += 1;
                        view.moves = 0;
                    } else {
                        view.moves = (cur + step).min(n);
                    }
                } else if cur == 0 && view.high > view.low {
                    view.high -= 1;
                    view.moves = u64::MAX;
                } else {
                    view.moves = cur.saturating_sub(step);
                }
            }
            A::MovesStart => view.moves = 0,
            A::MovesEnd => view.moves = u64::MAX,
            A::LayerGCode { add } => view.layer_ui.key = Some(add),
            A::OneLayer => {
                *one_layer = !*one_layer;
                if *one_layer {
                    if view.high >= max {
                        view.high = max / 2;
                    }
                } else {
                    view.low = 0;
                    view.high = max;
                }
                view.moves = u64::MAX;
            }
            _ => {}
        }
    }
    if *one_layer {
        view.low = view.high;
    }
    (view.low, view.high, view.moves) != before
}

impl App {
    /// Reads this frame's shortcuts, runs the app-level ones and queues the 3D-view ones.
    pub(crate) fn shortcuts(&mut self, ctx: &egui::Context) {
        let cx = Context {
            preview: self.preview && self.finished().is_some(),
            machine: self.machine,
            painting: self.viewport.paint,
            dialog: self.keys.dialog,
            native_menu: self.has_native_menu(),
            parts: self.cut.is_none()
                && self
                    .selected
                    .and_then(|i| self.plates.current().objects.get(i))
                    .is_some_and(|o| !o.volumes.is_empty() || !o.height_ranges.is_empty()),
        };
        let acts = collect(ctx, cx);
        let mut view = vec![];
        for act in acts {
            if let Some(a) = self.run_key(act, ctx) {
                view.push(a);
            }
        }
        self.viewport.keys = view;
    }

    /// Carries out one shortcut; returns it when the 3D view handles it.
    pub(crate) fn run_key(&mut self, act: Act, ctx: &egui::Context) -> Option<Act> {
        use crate::commands::Command;
        let has_objects = !self.plates.current().objects.is_empty();
        // An opened G-code file is read-only: only file, window and view keys work; editing
        // keys would change the hidden plate.
        if self.viewing_gcode() {
            match act {
                A::NewProject
                | A::Open
                | A::Import
                | A::Save
                | A::SaveAs
                | A::ExportGcode
                | A::Print
                | A::Preferences
                | A::ShowShortcuts
                | A::CollapseSidebar
                | A::Fullscreen
                | A::Minimize
                | A::Camera(_)
                | A::ZoomFit { .. }
                | A::ZoomIn
                | A::ZoomOut
                | A::TogglePerspective
                | A::Layer { .. }
                | A::Moves { .. }
                | A::MovesStart
                | A::MovesEnd
                | A::OneLayer => {}
                A::Digit(n) if n <= 6 => return Some(A::Camera(n)),
                A::Escape => return Some(act),
                _ => {
                    self.status =
                        "This G-code file is read-only; close it to edit the plate".into();
                    return None;
                }
            }
        }
        match act {
            A::NewProject => self.new_project(),
            A::Open | A::Import => self.open_dialog(ctx),
            A::Save => self.save(),
            A::SaveAs => self.save_as(None),
            A::Slice => self.slice_current(ctx),
            A::SliceAll => self.slice_all(ctx),
            A::ExportGcode if self.finished().is_some() => self.export(ctx),
            A::Print if self.finished().is_some() => self.open_print_dialog(),
            A::Preferences => self.open_preferences(None),
            A::SearchSettings => {
                self.machine = false;
                self.preview = false;
                self.keys.sidebar_hidden = false;
                self.keys.focus_settings_search = true;
            }
            A::ShowShortcuts => {
                self.keys.dialog = !self.keys.dialog;
                self.keys.focus_dialog_search = self.keys.dialog;
            }
            A::TogglePreview => {
                self.machine = false;
                if self.finished().is_some() || self.previewing() {
                    // A Preview waiting for its new slice goes back to Prepare too.
                    self.preview = !self.preview;
                } else if !self.preview {
                    // Preview of an unsliced plate slices it first.
                    self.slice_current(ctx);
                }
            }
            A::CollapseSidebar => self.keys.sidebar_hidden = !self.keys.sidebar_hidden,
            A::Fullscreen => {
                let full = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!full));
            }
            A::Minimize => ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true)),
            A::Undo => self.undo(),
            A::Redo => self.redo(),
            A::CutToClipboard if self.selected.is_some() => {
                self.copy_selection();
                self.delete_selected();
            }
            A::Copy => self.copy_selection(),
            A::Paste => {
                let at = self.viewport.bed_hover.map(|p| p.truncate());
                self.paste(at);
            }
            A::Clone => self.duplicate_selection(),
            A::SelectAll => self.run_command(Command::SelectAll, ctx),
            // In the cut tool's "Add connectors" mode Delete removes the selected connector,
            // never the object being cut (Orca's GLGizmosManager::on_char).
            // In the measure tool Delete restarts the measurement (Orca's measure gizmo).
            A::Delete if self.viewport.measure_on => {
                self.viewport.measure.unselect();
            }
            A::Delete if self.cut.as_ref().is_some_and(|c| c.editing) => {
                if let Some(c) = self.cut.as_mut() {
                    c.edit_connectors(crate::cut::ConnectorEdit::DeleteSelected);
                }
            }
            A::Delete if self.selected.is_some() => self.delete_selected(),
            A::DeleteAll if has_objects => self.clear_plate(),
            A::AddInstance if self.selected.is_some() => self.add_instances(),
            A::RemoveInstance if self.selected.is_some() => self.remove_instances(),
            A::ArrangeAll if has_objects || self.plates.len() > 1 => self.arrange_all(),
            A::ArrangePlate if has_objects => self.arrange(),
            A::Orient if self.selected.is_some() => self.auto_orient(),
            A::Orient | A::OrientPlate if has_objects => {
                let selection = self.selection();
                let all: Vec<usize> = (0..self.plates.current().objects.len()).collect();
                self.select_many(&all, false);
                self.auto_orient();
                self.select_many(&selection, false);
                if selection.is_empty() {
                    self.select(None, false);
                }
            }
            A::Rename if self.selected.is_some() => self.run_command(Command::Rename, ctx),
            A::RenamePlate => {
                self.plate_command(self.plates.current, crate::plate_menu::PlateCommand::Rename)
            }
            A::Digit(n) => {
                let tools = self.tool_count();
                let editing = !(self.preview && self.finished().is_some());
                if editing && self.selected.is_some() && tools > 1 && n >= 1 {
                    if (n as usize) <= tools {
                        self.run_command(Command::ChangeTool(n as usize - 1), ctx);
                    }
                } else if n <= 6 {
                    return Some(A::Camera(n));
                }
            }
            A::Brush(kind) => self.paint.brush = kind,
            A::CyclePart { back } => self.cycle_part(back),
            // Orca's GLGizmosManager::handle_shortcut opens Emboss even with nothing selected.
            A::Tool(Tool::Emboss) => {
                if self.viewport.paint.take().is_some() {
                    self.handle(crate::viewport::Action::PaintTool(None), &[], ctx);
                }
                self.viewport.lay_on_face = false;
                self.toggle_emboss();
            }
            // With nothing selected the plate's only object is the one (as the button does).
            A::Tool(Tool::Cut)
                if self.selected.is_some() || self.plates.current().objects.len() == 1 =>
            {
                if self.selected.is_none() {
                    self.select(Some(0), false);
                }
                if self.viewport.paint.take().is_some() {
                    self.handle(crate::viewport::Action::PaintTool(None), &[], ctx);
                }
                self.viewport.lay_on_face = false;
                self.toggle_cut();
            }
            // With nothing selected the plate's only object is the one (as the button does).
            A::Tool(Tool::MeshBoolean)
                if self.selected.is_some() || self.plates.current().objects.len() == 1 =>
            {
                if self.selected.is_none() {
                    self.select(Some(0), false);
                }
                if self.viewport.paint.take().is_some() {
                    self.handle(crate::viewport::Action::PaintTool(None), &[], ctx);
                }
                self.viewport.lay_on_face = false;
                self.toggle_boolean();
            }
            A::Camera(_)
            | A::ZoomFit { .. }
            | A::ZoomIn
            | A::ZoomOut
            | A::TogglePerspective
            | A::Tool(_)
            | A::Escape
            | A::Nudge { .. }
            | A::Turn(_)
            | A::Layer { .. }
            | A::Moves { .. }
            | A::MovesStart
            | A::MovesEnd
            | A::OneLayer
            | A::LayerGCode { .. } => return Some(act),
            _ => {}
        }
        None
    }
}

/// The gizmo mode a tool key leaves the 3D view in: the same key again closes the tool.
pub fn toggled(current: gizmo::Mode, pressed: gizmo::Mode, other_tool_open: bool) -> gizmo::Mode {
    if current == pressed && !other_tool_open {
        gizmo::Mode::None
    } else {
        pressed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{App, Options};
    use eframe::egui::{Event, Modifiers, RawInput};
    use mica_profiles::user::Store;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    fn app(models: &[&str]) -> (egui::Context, App) {
        let ctx = egui::Context::default();
        let dir = std::env::temp_dir().join(format!(
            "mica-keymap-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let app = App::create(
            &ctx,
            Options {
                prefs_path: None,
                store: Some(Store::open(dir)),
                files: models
                    .iter()
                    .map(|m| {
                        PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/models/"))
                            .join(m)
                    })
                    .collect(),
                headless: true,
            },
        );
        (ctx, app)
    }

    fn frame(ctx: &egui::Context, app: &mut App, events: Vec<Event>) {
        let input = RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1440., 900.),
            )),
            events,
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| app.show(ui));
        output.textures_delta.clear();
    }

    fn press(ctx: &egui::Context, app: &mut App, key: Key, modifiers: Modifiers) {
        let ev = |pressed| Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers,
        };
        frame(ctx, app, vec![Event::ModifiersChanged(modifiers), ev(true)]);
        frame(
            ctx,
            app,
            vec![ev(false), Event::ModifiersChanged(Modifiers::NONE)],
        );
    }

    /// Puts the keyboard into the settings search box, as a click (or ⌘F) would.
    fn focus_text_field(ctx: &egui::Context, app: &mut App) {
        app.keys.focus_settings_search = true;
        frame(ctx, app, vec![]);
        frame(ctx, app, vec![]);
        assert!(typing(ctx), "the search field has the keyboard");
    }

    #[test]
    fn every_chord_is_unique_in_its_scope() {
        for (i, x) in BINDINGS.iter().enumerate() {
            for y in &BINDINGS[i + 1..] {
                let same_scope = x.scope == y.scope
                    || matches!((x.scope, y.scope), (S::Paint(a), S::Paint(c)) if a & c != 0);
                let overlap = x.platform == y.platform
                    || x.platform == Platform::Any
                    || y.platform == Platform::Any;
                assert!(
                    !(x.key == y.key && x.mods == y.mods && same_scope && overlap),
                    "{:?} {:?} bound twice: {:?} and {:?}",
                    x.mods,
                    x.key,
                    x.act,
                    y.act
                );
            }
        }
    }

    #[test]
    fn orca_keys_map_to_orca_actions() {
        let prepare = Context::default();
        let preview = Context {
            preview: true,
            ..Context::default()
        };
        let none = Modifiers::NONE;
        let cmd = Modifiers::COMMAND;
        let shift = Modifiers::SHIFT;
        let act = |k, m, cx| lookup(k, m, false, cx).map(|x| x.act);
        assert_eq!(act(Key::M, none, prepare), Some(A::Tool(Tool::Move)));
        assert_eq!(act(Key::F, none, prepare), Some(A::Tool(Tool::LayOnFace)));
        assert_eq!(act(Key::C, none, prepare), Some(A::Tool(Tool::Cut)));
        assert_eq!(act(Key::B, none, prepare), Some(A::Tool(Tool::MeshBoolean)));
        assert_eq!(act(Key::T, none, prepare), Some(A::Tool(Tool::Emboss)));
        assert_eq!(act(Key::U, none, prepare), Some(A::Tool(Tool::Measure)));
        assert_eq!(
            act(Key::G, none, prepare),
            Some(A::Tool(Tool::LayerHeights))
        );
        assert_eq!(act(Key::E, none, prepare), Some(A::Tool(Tool::BrimEars)));
        assert_eq!(
            act(Key::N, none, prepare),
            Some(A::Tool(Tool::Paint(PaintKind::Material)))
        );
        assert_eq!(
            act(Key::P, none, prepare),
            Some(A::Tool(Tool::Paint(PaintKind::Seam)))
        );
        assert_eq!(act(Key::A, none, prepare), Some(A::ArrangeAll));
        assert_eq!(act(Key::A, shift, prepare), Some(A::ArrangePlate));
        assert_eq!(act(Key::D, cmd, prepare), Some(A::DeleteAll));
        assert_eq!(act(Key::K, cmd, prepare), Some(A::Clone));
        assert_eq!(act(Key::Num3, cmd, prepare), Some(A::Camera(3)));
        assert_eq!(act(Key::Tab, shift, prepare), Some(A::CollapseSidebar));
        assert_eq!(
            act(Key::Questionmark, shift, prepare),
            Some(A::ShowShortcuts)
        );
        assert_eq!(act(Key::Plus, shift, prepare), Some(A::AddInstance));
        // Preview: arrows drive the sliders, Shift is five times faster, L is one-layer mode.
        assert_eq!(
            act(Key::ArrowUp, none, preview),
            Some(A::Layer {
                up: true,
                fast: false
            })
        );
        assert_eq!(
            act(Key::ArrowRight, shift, preview),
            Some(A::Moves {
                forward: true,
                fast: true
            })
        );
        assert_eq!(act(Key::L, none, preview), Some(A::OneLayer));
        // `+` / `−` edit the layer's custom G-code in Preview, instances in Prepare.
        assert_eq!(
            act(Key::Plus, none, preview),
            Some(A::LayerGCode { add: true })
        );
        assert_eq!(
            act(Key::Minus, none, preview),
            Some(A::LayerGCode { add: false })
        );
        assert_eq!(act(Key::Plus, none, prepare), Some(A::AddInstance));
        // Prepare-only keys are dead in Preview, tools too.
        assert_eq!(act(Key::M, none, preview), None);
        // While painting supports, F / S / C pick brushes instead of tools.
        let painting = Context {
            painting: Some(PaintKind::Supports),
            ..Context::default()
        };
        assert_eq!(act(Key::F, none, painting), Some(A::Brush(BrushKind::Fill)));
        assert_eq!(
            act(Key::S, none, painting),
            Some(A::Brush(BrushKind::Sphere))
        );
        assert_eq!(act(Key::M, none, painting), Some(A::Tool(Tool::Move)));
        // Seam painting has no fill brush: F is lay-on-face there, as in Orca.
        let seam = Context {
            painting: Some(PaintKind::Seam),
            ..Context::default()
        };
        assert_eq!(act(Key::F, none, seam), Some(A::Tool(Tool::LayOnFace)));
        // Key repeat only repeats the repeatable ones.
        assert!(lookup(Key::M, none, true, prepare).is_none());
        assert!(lookup(Key::Plus, none, true, prepare).is_some());
        // Machine view: only global keys.
        let machine = Context {
            machine: true,
            ..Context::default()
        };
        assert_eq!(act(Key::M, none, machine), None);
        assert_eq!(act(Key::S, cmd, machine), Some(A::Save));
    }

    #[test]
    fn menus_show_the_bound_chord() {
        let mac = cfg!(target_os = "macos");
        assert_eq!(
            shortcut_text(A::SaveAs),
            if mac { "Cmd+Shift+S" } else { "Ctrl+Shift+S" }
        );
        assert_eq!(shortcut_text(A::Tool(Tool::Cut)), "C");
        assert_eq!(shortcut_text(A::Tool(Tool::Emboss)), "T");
        assert_eq!(
            shortcut_text(A::Camera(1)),
            if mac { "Cmd+1" } else { "Ctrl+1" }
        );
        assert_eq!(shortcut_text(A::ShowShortcuts), "?");
    }

    #[test]
    fn rows_merge_aliases_and_search() {
        let rows = rows();
        let delete = rows
            .iter()
            .find(|r| r.help.starts_with("Delete the selection"))
            .unwrap();
        assert_eq!(delete.keys, "Del / Backspace");
        let digits = rows.iter().find(|r| r.keys == "1–9").unwrap();
        assert_eq!(digits.group, Group::Objects);
        assert_eq!(rows.iter().filter(|r| r.keys == "1–9").count(), 1);
        let found = search(&rows, "paint");
        assert!(found.len() >= 4);
        assert!(found.iter().all(|r| r.help.to_lowercase().contains("paint")
            || r.group.title().to_lowercase().contains("paint")));
        assert!(search(&rows, "zzzz-nothing").is_empty());
    }

    #[test]
    fn tool_keys_toggle_and_escape_closes_then_deselects() {
        let (ctx, mut app) = app(&["cube20.stl"]);
        frame(&ctx, &mut app, vec![]);
        app.select(Some(0), false);
        press(&ctx, &mut app, Key::R, Modifiers::NONE);
        assert_eq!(app.viewport.mode, gizmo::Mode::Rotate);
        press(&ctx, &mut app, Key::R, Modifiers::NONE);
        assert_eq!(app.viewport.mode, gizmo::Mode::None, "R again closes it");
        press(&ctx, &mut app, Key::S, Modifiers::NONE);
        assert_eq!(app.viewport.mode, gizmo::Mode::Scale);
        press(&ctx, &mut app, Key::Escape, Modifiers::NONE);
        assert_eq!(app.viewport.mode, gizmo::Mode::Move, "Esc leaves the tool");
        assert_eq!(app.selected, Some(0), "… but keeps the selection");
        press(&ctx, &mut app, Key::Escape, Modifiers::NONE);
        assert_eq!(app.selected, None, "the second Esc deselects");
        // Painting: L opens support painting, L again closes it.
        app.select(Some(0), false);
        press(&ctx, &mut app, Key::L, Modifiers::NONE);
        assert_eq!(app.viewport.paint, Some(PaintKind::Supports));
        press(&ctx, &mut app, Key::S, Modifiers::NONE);
        assert_eq!(
            app.paint.brush,
            BrushKind::Sphere,
            "S picks the sphere brush"
        );
        assert_eq!(app.viewport.paint, Some(PaintKind::Supports));
        press(&ctx, &mut app, Key::Escape, Modifiers::NONE);
        assert_eq!(app.viewport.paint, None, "Esc leaves the painting tool");
        assert_eq!(app.selected, Some(0));
        // C toggles the cut tool.
        press(&ctx, &mut app, Key::C, Modifiers::NONE);
        assert!(app.cut.is_some());
        press(&ctx, &mut app, Key::Escape, Modifiers::NONE);
        assert!(app.cut.is_none());
    }

    /// A tool shortcut never opens a tool with nothing to work on: on an empty plate it says
    /// "Add a model first", with several models and none selected "Select a model first",
    /// and with one model it picks that one and opens its card.
    #[test]
    fn tool_keys_need_a_model_and_pick_the_only_one() {
        // Move, Rotate, Scale, Lay on face, Cut, Boolean, the painting tools, layer heights
        // and brim ears (color painting needs a multi-tool printer).
        let keys = [
            (Key::M, "Move"),
            (Key::R, "Rotate"),
            (Key::S, "Scale"),
            (Key::F, "Lay on face"),
            (Key::C, "Cut"),
            (Key::B, "Mesh boolean"),
            (Key::L, "Support painting"),
            (Key::P, "Seam painting"),
            (Key::H, "Fuzzy skin painting"),
            (Key::X, "Texture painting"),
            (Key::G, "Layer heights"),
            (Key::E, "Brim ears"),
        ];
        let nothing_open = |app: &App| {
            app.viewport.paint.is_none()
                && !app.viewport.lay_on_face
                && app.viewport.object_tool.is_none()
                && app.cut.is_none()
                && app.mesh_tool.is_none()
        };
        // An empty plate.
        let (ctx, mut app) = self::app(&[]);
        frame(&ctx, &mut app, vec![]);
        for (key, name) in keys {
            app.status.clear();
            press(&ctx, &mut app, key, Modifiers::NONE);
            assert!(nothing_open(&app), "{name} opened on an empty plate");
            assert_eq!(app.status, format!("{name}: Add a model first"));
        }
        // Two models, none selected.
        let (ctx, mut app) = self::app(&["cube20.stl", "cube20.stl"]);
        frame(&ctx, &mut app, vec![]);
        assert_eq!(app.plates.current().objects.len(), 2);
        for (key, name) in keys {
            app.select(None, false);
            app.status.clear();
            press(&ctx, &mut app, key, Modifiers::NONE);
            assert!(nothing_open(&app), "{name} opened with nothing selected");
            assert!(
                app.status
                    .starts_with(&format!("{name}: Select a model first")),
                "{name}: {}",
                app.status
            );
        }
        // One model, not selected: the tool picks it.
        let (ctx, mut app) = self::app(&["cube20.stl"]);
        frame(&ctx, &mut app, vec![]);
        for (key, name) in keys {
            app.select(None, false);
            app.viewport.mode = gizmo::Mode::None;
            frame(&ctx, &mut app, vec![]);
            press(&ctx, &mut app, key, Modifiers::NONE);
            frame(&ctx, &mut app, vec![]);
            assert_eq!(app.selected, Some(0), "{name} picks the only model");
            let open = match key {
                Key::M => app.viewport.mode == gizmo::Mode::Move,
                Key::R => app.viewport.mode == gizmo::Mode::Rotate,
                Key::S => app.viewport.mode == gizmo::Mode::Scale,
                _ => !nothing_open(&app),
            };
            assert!(open, "{name} opened on the only model");
            press(&ctx, &mut app, Key::Escape, Modifiers::NONE);
            app.cut = None;
            app.mesh_tool = None;
            app.viewport.mode = gizmo::Mode::None;
        }
    }

    /// A painting tool whose object was deselected shows a card that says what to do.
    #[test]
    fn a_painting_tool_without_a_selection_shows_what_to_do() {
        let (ctx, mut app) = app(&["cube20.stl", "cube20.stl"]);
        frame(&ctx, &mut app, vec![]);
        app.select(Some(0), false);
        press(&ctx, &mut app, Key::X, Modifiers::NONE);
        assert_eq!(app.viewport.paint, Some(PaintKind::Texture));
        app.select(None, false);
        frame(&ctx, &mut app, vec![]);
        frame(&ctx, &mut app, vec![]);
        let card = egui::LayerId::new(egui::Order::Middle, egui::Id::unique("paint-pick-card"));
        assert!(
            ctx.memory(|m| m.areas().visible_layer_ids().contains(&card)),
            "the select-a-model card shows"
        );
    }

    #[test]
    fn edit_keys_follow_orca() {
        let (ctx, mut app) = app(&["cube20.stl"]);
        frame(&ctx, &mut app, vec![]);
        app.select(Some(0), false);
        press(&ctx, &mut app, Key::Plus, Modifiers::NONE);
        assert_eq!(app.plates.current().objects.len(), 2, "+ adds an instance");
        press(&ctx, &mut app, Key::Minus, Modifiers::NONE);
        assert_eq!(app.plates.current().objects.len(), 1, "− removes it");
        app.select(Some(0), false);
        press(&ctx, &mut app, Key::K, Modifiers::COMMAND);
        assert_eq!(app.plates.current().objects.len(), 2, "⌘K duplicates");
        press(&ctx, &mut app, Key::A, Modifiers::COMMAND);
        assert_eq!(app.selection().len(), 2, "⌘A selects all");
        // Arrows move 10 mm, Shift 1 mm (Orca).
        app.select(Some(0), false);
        let x0 = app.plates.current().objects[0].position.x;
        press(&ctx, &mut app, Key::ArrowRight, Modifiers::NONE);
        let x1 = app.plates.current().objects[0].position.x;
        assert!((x1 - x0 - 10.).abs() < 1e-3, "→ moves 10 mm: {x0} → {x1}");
        press(&ctx, &mut app, Key::ArrowLeft, Modifiers::SHIFT);
        let x2 = app.plates.current().objects[0].position.x;
        assert!((x1 - x2 - 1.).abs() < 1e-3, "⇧← moves 1 mm: {x1} → {x2}");
        press(&ctx, &mut app, Key::D, Modifiers::COMMAND);
        assert!(app.plates.current().objects.is_empty(), "⌘D deletes all");
        press(&ctx, &mut app, Key::Z, Modifiers::COMMAND);
        assert_eq!(app.plates.current().objects.len(), 2, "⌘Z brings them back");
        // Camera: ⌘1 is the top view, K toggles perspective.
        press(&ctx, &mut app, Key::Num1, Modifiers::COMMAND);
        assert!((app.viewport.camera.pitch - std::f32::consts::FRAC_PI_2).abs() < 1e-4);
        let persp = app.viewport.camera.perspective;
        press(&ctx, &mut app, Key::K, Modifiers::NONE);
        assert_ne!(app.viewport.camera.perspective, persp);
        // Shift+Tab hides the sidebar, ? opens the shortcuts window.
        press(&ctx, &mut app, Key::Tab, Modifiers::SHIFT);
        assert!(app.keys.sidebar_hidden);
        press(&ctx, &mut app, Key::Tab, Modifiers::SHIFT);
        assert!(!app.keys.sidebar_hidden);
        press(&ctx, &mut app, Key::Questionmark, Modifiers::SHIFT);
        assert!(app.keys.dialog);
    }

    #[test]
    fn delete_in_connector_mode_removes_the_connector_not_the_object() {
        let (ctx, mut app) = app(&["cube20.stl"]);
        frame(&ctx, &mut app, vec![]);
        app.select(Some(0), false);
        press(&ctx, &mut app, Key::C, Modifiers::NONE);
        let o = app.plates.current().objects[0].clone();
        let (min, max) = o.bounds();
        let c = app.cut.as_mut().expect("C opens the cut tool");
        c.editing = true;
        let mid = (min + max) * 0.5;
        c.edit_connectors(crate::cut::ConnectorEdit::Click(mid.with_z(c.z())));
        assert_eq!(c.connectors.len(), 1);
        press(&ctx, &mut app, Key::Backspace, Modifiers::NONE);
        let c = app.cut.as_ref().unwrap();
        assert!(c.connectors.is_empty(), "⌫ deletes the selected connector");
        assert_eq!(app.plates.current().objects.len(), 1, "… not the object");
        press(&ctx, &mut app, Key::Escape, Modifiers::NONE);
        assert!(
            app.cut.as_ref().is_some_and(|c| !c.editing),
            "Esc leaves connector mode"
        );
        press(&ctx, &mut app, Key::Escape, Modifiers::NONE);
        assert!(app.cut.is_none(), "the next Esc closes the cut tool");
    }

    #[test]
    fn b_toggles_the_boolean_card_and_esc_closes_mesh_tools() {
        use crate::mesh_tools::MeshTool;
        let (ctx, mut app) = app(&["cube20.stl", "cube20.stl"]);
        frame(&ctx, &mut app, vec![]);
        app.select(Some(0), false);
        press(&ctx, &mut app, Key::B, Modifiers::NONE);
        assert!(
            matches!(app.mesh_tool, Some(MeshTool::Boolean(_))),
            "B opens it"
        );
        press(&ctx, &mut app, Key::B, Modifiers::NONE);
        assert!(app.mesh_tool.is_none(), "B again closes it");
        press(&ctx, &mut app, Key::B, Modifiers::NONE);
        press(&ctx, &mut app, Key::Escape, Modifiers::NONE);
        assert!(app.mesh_tool.is_none(), "Esc closes the card");
        assert_eq!(app.selected, Some(0), "… before it deselects");
        app.toggle_simplify();
        press(&ctx, &mut app, Key::Escape, Modifiers::NONE);
        assert!(app.mesh_tool.is_none(), "Esc closes the simplify card");
        assert_eq!(app.selected, Some(0));
    }

    #[test]
    fn no_single_key_or_delete_while_typing() {
        let (ctx, mut app) = app(&["cube20.stl"]);
        frame(&ctx, &mut app, vec![]);
        app.select(Some(0), false);
        focus_text_field(&ctx, &mut app);
        let mode = app.viewport.mode;
        for key in [
            Key::R,
            Key::A,
            Key::Delete,
            Key::Backspace,
            Key::Plus,
            Key::Escape,
        ] {
            let ev = |pressed| Event::Key {
                key,
                physical_key: None,
                pressed,
                repeat: false,
                modifiers: Modifiers::NONE,
            };
            frame(&ctx, &mut app, vec![ev(true)]);
            frame(&ctx, &mut app, vec![ev(false)]);
        }
        assert_eq!(
            app.viewport.mode, mode,
            "R does not switch tools while typing"
        );
        assert_eq!(
            app.plates.current().objects.len(),
            1,
            "Del / ⌫ / + leave the objects alone"
        );
        assert_eq!(app.selected, Some(0), "Esc in a field does not deselect");
        // Once the field lets go, the same keys work again.
        ctx.memory_mut(|m| m.stop_text_input());
        frame(&ctx, &mut app, vec![]);
        assert!(!typing(&ctx));
        press(&ctx, &mut app, Key::R, Modifiers::NONE);
        assert_eq!(app.viewport.mode, gizmo::Mode::Rotate);
    }

    #[test]
    fn plain_digits_pick_a_camera_without_a_multi_tool_selection() {
        let (ctx, mut app) = app(&["cube20.stl"]);
        frame(&ctx, &mut app, vec![]);
        app.select(None, false);
        press(&ctx, &mut app, Key::Num3, Modifiers::NONE);
        assert!(
            app.viewport.camera.pitch.abs() < 1e-4,
            "3 is the front view"
        );
    }

    fn layers(counts: &[u64]) -> Vec<preview::Layer> {
        let mut start = 0;
        counts
            .iter()
            .map(|&count| {
                let l = preview::Layer {
                    start,
                    count,
                    z: 0.,
                    time_s: 0.,
                };
                start += count;
                l
            })
            .collect()
    }

    #[test]
    fn preview_sliders_follow_orca() {
        let p = layers(&[10, 10, 10, 10, 10, 10, 10, 10]);
        let mut view = preview::View::default();
        let mut one = false;
        let up = |fast| A::Layer { up: true, fast };
        let down = |fast| A::Layer { up: false, fast };
        assert!(preview_keys(&[down(false)], &p, &mut view, &mut one));
        assert_eq!(view.high, 6);
        preview_keys(&[down(true)], &p, &mut view, &mut one);
        assert_eq!(view.high, 1, "Shift moves five layers");
        preview_keys(&[up(false)], &p, &mut view, &mut one);
        assert_eq!(view.high, 2);
        // Move slider: back from the full layer, wrap to the layer below at the start.
        preview_keys(&[A::MovesStart], &p, &mut view, &mut one);
        assert_eq!(view.moves, 0);
        preview_keys(
            &[A::Moves {
                forward: false,
                fast: false,
            }],
            &p,
            &mut view,
            &mut one,
        );
        assert_eq!((view.high, view.moves), (1, u64::MAX));
        preview_keys(
            &[A::Moves {
                forward: true,
                fast: false,
            }],
            &p,
            &mut view,
            &mut one,
        );
        assert_eq!(
            (view.high, view.moves),
            (2, 0),
            "past the end: the layer above"
        );
        preview_keys(
            &[A::Moves {
                forward: true,
                fast: true,
            }],
            &p,
            &mut view,
            &mut one,
        );
        assert_eq!(view.moves, 5);
        // One-layer mode keeps low == high while the slider moves.
        preview_keys(&[A::OneLayer], &p, &mut view, &mut one);
        assert!(one);
        assert_eq!((view.low, view.high), (2, 2));
        preview_keys(&[up(false)], &p, &mut view, &mut one);
        assert_eq!((view.low, view.high), (3, 3));
        preview_keys(&[A::OneLayer], &p, &mut view, &mut one);
        assert!(!one);
        assert_eq!((view.low, view.high), (0, 7), "off again: the whole print");
    }
}
