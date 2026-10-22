// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Headless screenshots of the **whole window**: egui panels and the 3D view rendered into a
//! wgpu texture and saved as PNG, so layouts can be checked without a display.
//!
//! ```sh
//! mica-gui --screenshot out.png model.stl [more.stl] [--preview] [--printer "name"] [--xl]
//!          [--select N] [--tab Quality] [--advanced] [--search text] [--changed]
//!          [--collapse Section] [--object-mode]
//!          [--printer-editor] [--filament-editor N] [--all-printers] [--light]
//!          [--sidebar left|right|hidden] [--state slicing|sliced|outdated] [--preview-topic T]
//!          [--drag-ring DEG[,RATIO[,AXIS]]] [--click X,Y] [--type TEXT] [--pointer X,Y]
//!          [--paint support|seam|colour|fuzzy|texture] [--brush circle|sphere|fill|height]
//!          [--brush-size R] [--fill-spread MM] [--paint-tool N] [--block] [--overhangs]
//!          [--paint-overhangs]
//!          [--drag X1,Y1,X2,Y2] [--filament T=NAME]
//!          [--crop W] [--crop-height H] [--size 1440x900] [--scale 2] [--real-store]
//!          [--plate N] [--show-paint support,seam,fuzzy] [--escape] [--deselect]
//!          [--object-tool layers|ears] [--adaptive Q] [--smooth-layers R] [--band W]
//!          [--brim-ear X,Y,D] [--auto-ears]
//!          [--colour T=#RRGGBB] [--lay-on-face]
//!          [--save out.3mf] [--flavour orca|prusa] [--save-as] [--unsaved-prompt]
//!          [--calibrate TEST[:TOOL]] [--calibrate-create] [--calibrate-result V] [--calibrate-save]
//!          [--preferences] [--preferences-tab T] [--auto-slice] [--toolbar compact|normal|large]
//!          [--theme dark|light|follow-system] [--new-project] [--welcome] [--whats-new]
//!          [--library] [--library-list] [--library-search T] [--library-select N]
//!          [--library-filter F] [--library-sort S] [--library-offline] [--library-scanning]
//!          [--library-group G] [--library-project NAME] [--library-folder DIR]
//!          [--library-detail-width W] [--library-zoom F] [--library-plate N]
//!          [--library-pending] [--library-drag N[,M]] [--library-drop-part N] [--library-printers]
//!          [--shortcuts SEARCH] [--emboss KIND[+wrap]:FACE:TEXT|svg=PATH]… [--emboss-close]
//!          [--color-by MODE] [--measure-on] [--measure X,Y[,shift]]... [--section NX,NY,NZ[,POS]]
//!          [--zoom F] [--orbit YAW,PITCH] [--place X,Y] [--mcp CALLS]
//! ```
//! `--mcp CALLS` runs Mica MCP tool calls (a JSON array of `{"tool", "args"}`, inline or a
//! file) in the window as live mode does, after the files load; the badge then says connected.
//! `--layer-gcode 'color:5;pause:10:Insert magnets;custom:12:M117 hi\nG4 S1;tool:3:2;template:8'`
//! puts per-layer custom G-code on the plate before `--preview` slices it; `--layer-menu`
//! opens the layer slider's "+" menu, `--layer-dialog pause|custom[:TEXT]` its editor (at the
//! upper handle's layer, `--layer N`).
//! `--shortcuts` opens the Keyboard shortcuts window with SEARCH in its search field (`""`
//! for the full list).
//! `--emboss` adds a text (or an SVG file's shape) to the selected object with the Text tool:
//! KIND is `raised`, `engraved`, `modifier` or `object`, FACE `top`, `front` (−Y), `right`
//! (+X) or `camera`; `\n` in TEXT breaks lines. The last one's card stays open unless
//! `--emboss-close`.
//! `--place X,Y` puts the selected (else the first) object's centre at that bed position before
//! the camera frames it; `--command move-inside` then runs Move inside on it. A `--state sliced`
//! slice that fails shows the window's failed-slice state.
//! `--zoom` and `--orbit` adjust the camera after the framing flags (closer by `F`; angles in
//! degrees).
//! `--color-by` picks the preview colour scheme (`feature`, `height`, `width`, `speed`,
//! `actual-speed`, `fan`, `temperature`, `flow`, `acceleration`, `layertime`, `layertime-log`,
//! `tool`, `color-print`). `--measure-on` opens the measure tool; each `--measure X,Y` clicks the
//! feature at that point (`,shift` for a point on it). `--section` turns the section view on
//! with that plane normal and position (0–1 across the scene; Prepare, or Preview with
//! `--preview`).
//! Printers (mock servers): `--printer-host NAME=prusalink|moonraker|u1@ADDRESS[#KEY]`,
//! `--printer-camera NAME=URL` (its manual camera URL), `--machine` (the Printers dashboard)
//! or `--machine-printer NAME`; such runs only talk to loopback addresses.
//! `--add-printer found|gallery|setup` opens the Add printer window (see `Args::add_printer`
//! for its `--mdns` / `--add-printer-*` companions; loopback only as well).
//! `--texture-gallery` opens the texture gallery (as the Texture image row does); with
//! `--paint texture`, `--paint-tool N` is the texture slot.
//! `--about about|credits` opens the About window on that tab; `--about-search T` types T into
//! the Credits search (or the Licenses search), `--about-licenses` opens the Open-source licenses
//! view (every crate), `--about-license ID` shows that
//! license's text; `--brick-layers` turns the hidden Brick layers feature on.
//! `--tour N` shows step N (one-based) of the quick tour, settled, on the sample plate (the
//! Benchy when nothing was loaded; step 6 slices it first, as the tour does).
//! `--welcome` opens the Welcome window (with `--import-home`, its Import card shows when that
//! home has presets); `--whats-new` opens What's new in this version.
//! `--import-home DIR` looks for other slicers' presets under DIR (a fake home folder) and
//! shows the startup notice; `--import-all` first imports what the review would check,
//! `--import-review` opens the review window; `--process NAME` selects a print profile and
//! `--other-printers` lists the presets made for other printers too; `--visible-filaments
//! SEARCH` opens T1's filament picker on its visible-filament list (`""`: no search).
//! `--preferences` opens the Settings window (`--preferences-tab general|slicing|profiles|files|features`
//! on that tab); `--toolbar` sets Settings › General › Toolbar size; `--auto-slice` turns
//! automatic slicing on
//! (so its delay shows); `--theme` sets the theme choice; `--new-project` runs File › New
//! project after loading, which asks first when the scene has unsaved changes.
//! `--calibrate` opens a Calibrate card (`flow1`, `flow2`, `pa-line`, `pa-pattern`, `pa-tower`,
//! `temperature`, `retraction`, `volumetric`; `:TOOL` is one-based); `--calibrate-create` makes
//! and slices its plate, `--calibrate-result` types the reading into the plate's card and
//! `--calibrate-save` writes it to the filament profile (of the throwaway store).
//! `--save` writes the scene as a project after the passes (so scripted strokes are in it);
//! `--save-as` opens the "Save project as" dialog and `--unsaved-prompt` the "Save changes?"
//! prompt for a screenshot.
//! `--object-tool` opens the variable layer height (`layers`) or brim ears (`ears`) tool on
//! the selected object; `--adaptive Q` and `--smooth-layers R` press Adaptive (quality Q)
//! and Smooth (radius R) in its card, `--band W` sets the band width; `--brim-ear X,Y,D`
//! places an ear of diameter D at bed X,Y (repeatable) and `--auto-ears` presses Auto-place.
//! `--show-paint` turns the View menu's "Always show … paint" toggles on; `--escape` leaves
//! the painting tool (Esc) after the scripted drag, for a Prepare shot without the tool;
//! `--deselect` clears the selection once the tool is open.
//! Files load after the printer and filaments are selected, so a project's tools resolve
//! against the right printer; `--plate N` (one-based) then shows that plate of a project.
//! `--drag-ring` scripts a rotate-gizmo drag: the pointer presses the ring handle and moves to
//! the angle `DEG` at `RATIO` × ring radius (1 = on the ring, 0.5 = the snap zone) without
//! releasing. `AXIS` is `z` (default: the bed ring, `DEG` absolute on the bed), `x` or `y` (the
//! tilt rings, `DEG` from the handle's rest position). `--click` presses and releases at a point before the final passes
//! (opens dropdowns); `--type` then types into the field the click opened and presses
//! Enter; `--pointer` then hovers. `--state` slices before rendering: `slicing`
//! leaves the worker running, `sliced` waits for it, `outdated` also changes a setting after.
//! `--screenshot-3d` keeps the older 3D-only renderer (`render::screenshot`).

use crate::{
    app::{App, Options},
    render,
    settings::{self, Tab},
};
use eframe::egui;
use egui_wgpu::wgpu;
use mica_profiles::user::Store;
use std::{
    future::Future,
    path::PathBuf,
    pin::pin,
    sync::{Arc, atomic::AtomicBool},
    task::{Context, Poll, Wake, Waker},
};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

struct ThreadWake(std::thread::Thread);
impl Wake for ThreadWake {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.0.unpark();
    }
}
fn block_on<F: Future>(future: F) -> F::Output {
    let waker = Waker::from(Arc::new(ThreadWake(std::thread::current())));
    let mut cx = Context::from_waker(&waker);
    let mut f = pin!(future);
    loop {
        match f.as_mut().poll(&mut cx) {
            Poll::Ready(v) => return v,
            Poll::Pending => std::thread::park(),
        }
    }
}

#[derive(Default)]
struct Args {
    output: Option<PathBuf>,
    files: Vec<PathBuf>,
    preview: bool,
    printer: Option<String>,
    xl: bool,
    select: Option<usize>,
    tab: Option<String>,
    advanced: bool,
    search: Option<String>,
    object_mode: bool,
    printer_editor: bool,
    /// Page of the printer editor to show (`--printer-page N`, zero-based).
    printer_page: Option<usize>,
    filament_editor: Option<usize>,
    all_printers: bool,
    light: bool,
    crop: Option<u32>,
    /// Camera preset name (`iso`, `top`, `bottom`, `below`, `front`, …).
    view: Option<String>,
    /// Frame the selected object instead of the plate.
    focus: bool,
    /// Injected pointer position (hover highlights, preview tooltips).
    pointer: Option<[f32; 2]>,
    /// Folds a section of the settings topic (`Seam` or `Quality/Seam`).
    collapse: Option<String>,
    /// Turns the settings list's Changed filter on.
    changed: bool,
    gizmo: Option<String>,
    plates: usize,
    /// One-based plate of a loaded project to show (`--plate N`).
    plate: Option<usize>,
    /// Paint kinds to always show (`--show-paint support,seam,fuzzy`).
    show_paint: Vec<String>,
    /// Press Esc after the drag (leaves the painting tool).
    escape: bool,
    /// Clear the selection after the tools are set up (a painting tool with nothing selected).
    deselect: bool,
    size: [u32; 2],
    scale: f32,
    real_store: bool,
    overrides: Vec<(String, String)>,
    /// Per-object overrides for the selected object (`--object-set key=value`).
    object_overrides: Vec<(String, String)>,
    /// The current plate's own settings (`--plate-set key=value`, e.g.
    /// `curr_bed_type=Cool Plate`), and its sidebar scope shown (`--plate-scope`).
    plate_overrides: Vec<(String, String)>,
    plate_scope: bool,
    /// `left` or `right` (the default preference).
    sidebar: Option<String>,
    /// `compact`, `normal` or `large` (`--toolbar`).
    toolbar: Option<crate::state::ToolbarSize>,
    /// `slicing`, `sliced` or `outdated`.
    state: Option<String>,
    /// Scripted ring drag: absolute angle in degrees and the radius ratio.
    drag_ring: Option<(f32, f32)>,
    /// The ring of the scripted drag: 0 = X, 1 = Y, 2 = Z (default).
    ring_axis: Option<usize>,
    /// A click (press + release) at this point before the final passes.
    click: Option<[f32; 2]>,
    /// Text typed after the click, committed with Enter (`--type 200`).
    typed: Option<String>,
    /// A left drag from one point to another with intermediate moves, released before the
    /// final passes (`--drag X1,Y1,X2,Y2`): paints a stroke in a painting tool.
    drag: Option<[f32; 4]>,
    /// `--drag-steps N`: the drag's pointer moves (1, 2, 4 or 8; default 8).
    drag_steps: Option<usize>,
    /// Painting tool to activate on the selected object: `support|seam|colour|fuzzy`.
    paint: Option<String>,
    /// Brush settings: `--brush circle|sphere|fill|height`, `--brush-size R`,
    /// `--fill-spread MM` (the Fill brush's max spread), `--paint-tool N` (one-based),
    /// `--block`, `--overhangs` (show), `--paint-overhangs`.
    brush: Option<String>,
    brush_size: Option<f32>,
    fill_spread: Option<f32>,
    paint_tool: Option<usize>,
    /// `--mix 1:50,2:50[/outer|/gradient:20,80][@Name]`: adds a mixed color (repeatable).
    mixes: Vec<String>,
    /// `--mix-editor N|new`: opens the mixed color editor on mix N (one-based) or a new one.
    mix_editor: Option<String>,
    /// `--brush-height H`: the Height brush's band, mm.
    brush_height: Option<f32>,
    block: bool,
    overhangs: bool,
    paint_overhangs: bool,
    /// `--object-tool layers|ears` and the tools' scripted buttons.
    object_tool: Option<String>,
    adaptive: Option<f32>,
    smooth_layers: Option<u32>,
    band: Option<f32>,
    brim_ears: Vec<[f32; 3]>,
    auto_ears: bool,
    /// `--filament T=NAME`: the filament preset (prefix) for one-based tool T.
    filaments: Vec<(usize, String)>,
    /// `--colour T=#RRGGBB`: pick a filament colour for one-based tool T after slicing (as
    /// the sidebar's colour popup does; the slice is kept). Repeatable.
    colours: Vec<(usize, [u8; 3])>,
    /// Turn on Lay on face (`--lay-on-face`); planes are found before the passes.
    lay_on_face: bool,
    /// Keep only the top part, in points.
    crop_height: Option<u32>,
    /// Further selected objects (`--select-also N`, repeatable): a multi-selection.
    select_also: Vec<usize>,
    /// Object commands run after loading, in order (`--command duplicate`, `instance`,
    /// `modifier:box`, `negative:cylinder`, `enforcer:sphere`, `blocker:slab`, `range`,
    /// `arrange`, `arrange-all`, `orient`, `select-all`, `copy`, `paste`, `cut`,
    /// `cut-apply`, `split-objects`, `split-parts`, `centre`, `drop`, `mirror-x|y|z`,
    /// `rotate90`, `delete`, `instance-remove`, `settings`; mesh tools: `boolean[:union|
    /// difference|intersection]`, `boolean-consume` (do not keep the other shape),
    /// `overlap:DX,DY,DZ` (object 2's box corner moved to object 1's corner + offset),
    /// `boolean-apply`, `simplify[:PERCENT]`, `simplify-apply`,
    /// `repair`).
    commands: Vec<String>,
    /// Settings for the selected volume / range after `modifier:` / `range`
    /// (`--sub-set key=value`).
    sub_overrides: Vec<(String, String)>,
    /// Cut plane height and tilt (degrees) before `cut-apply`.
    cut_z: Option<f32>,
    cut_tilt: Option<[f32; 2]>,
    /// Texts / SVGs added with the Text tool (`--emboss`), and whether its card closes after.
    emboss: Vec<String>,
    emboss_close: bool,
    /// Cut connectors on the plane (`--connector plug@-5,0`, repeatable, plane mm), their
    /// shape (`--connector-shape square`) and size (`--connector-size 5,6`: diameter, depth);
    /// `--connector-edit` leaves the card in "Add connectors" mode.
    connectors: Vec<(mica_core::cut_connectors::ConnectorType, [f32; 2])>,
    connector_shape: Option<String>,
    connector_size: Option<[f32; 2]>,
    connector_edit: bool,
    /// `--connector-auto dowel`: that type, then "Place automatically" in the card.
    connector_auto: Option<mica_core::cut_connectors::ConnectorType>,
    /// A right-click (press + release of the secondary button) that opens a context menu.
    right_click: Option<[f32; 2]>,
    /// A Shift-held left drag: rectangle selection.
    shift_drag: Option<[f32; 4]>,
    /// Loose models: `rows` uses the old row placement (the "before" of an arrange shot),
    /// `none` leaves them where they loaded; default packs them.
    arrange: Option<String>,
    /// Tilt the selected object about X and Y (degrees) after loading.
    tilt: Option<[f32; 2]>,
    /// Put the selected object's (else the first object's) centre at this bed X, Y (mm).
    place: Option<[f32; 2]>,
    /// Top layer shown in the preview (one-based).
    layer: Option<usize>,
    /// The settings topic open in the Preview sidebar (`--preview-topic Speed`); without it
    /// the Preview page shows.
    preview_topic: Option<String>,
    /// Per-layer custom G-code to put on the plate (`--layer-gcode`).
    layer_gcode: Option<String>,
    /// Open the layer slider's menu (`--layer-menu`) or editor (`--layer-dialog`).
    layer_menu: bool,
    layer_dialog: Option<String>,
    /// Save the scene as a project after everything else (`--save out.3mf`), in `--flavour
    /// orca|prusa` (default: the project's own, else by printer).
    save: Option<PathBuf>,
    flavour: Option<String>,
    /// Open the "Save project as" dialog, or the unsaved-changes prompt (as if closing).
    save_as: bool,
    unsaved_prompt: bool,
    /// Settings window, automatic slicing on, a theme choice, File › New project.
    preferences: bool,
    /// The Settings tab to show (`--preferences-tab general|slicing|files|features`).
    preferences_tab: Option<String>,
    /// Other slicers' presets under this fake home (`--import-home`), imported first
    /// (`--import-all`), the review window (`--import-review`).
    import_home: Option<PathBuf>,
    import_all: bool,
    import_review: bool,
    /// A print profile to select, and "Show profiles for other printers".
    process: Option<String>,
    other_printers: bool,
    /// `--visible-filaments SEARCH`: T1's filament picker open on its visible-filament list.
    visible_filaments: Option<String>,
    /// The texture gallery window.
    texture_gallery: bool,
    /// The Keyboard shortcuts window, with this text in its search field.
    shortcuts: Option<String>,
    /// The About window on this tab (`about|credits`), its search, an expanded crate list,
    /// a license's text; the hidden Brick layers feature on.
    about: Option<String>,
    about_search: Option<String>,
    about_licenses: bool,
    about_license: Option<String>,
    brick_layers: bool,
    /// The Welcome window, What's new.
    welcome: bool,
    /// One-based quick tour step to show (`--tour N`).
    tour: Option<usize>,
    whats_new: bool,
    auto_slice: bool,
    theme: Option<String>,
    new_project: bool,
    /// Select the current plate's prime tower (`--select-tower`).
    select_tower: bool,
    /// Put the current plate's prime tower at X,Y (and rotation) in bed millimetres
    /// (`--tower X,Y[,R]`).
    tower: Option<[f64; 3]>,
    /// Open a Calibrate card (`--calibrate pa-tower[:TOOL]`, one-based tool).
    calibrate: Option<String>,
    /// Create the card's plate and slice it synchronously (`--calibrate-create`).
    calibrate_create: bool,
    /// Type a result into the calibration plate's card (`--calibrate-result 21`).
    calibrate_result: Option<String>,
    /// Save the result to the filament profile (`--calibrate-save`).
    calibrate_save: bool,
    /// `--printer-host NAME=prusalink|moonraker|u1@ADDRESS[#KEY]`: a physical printer, kept
    /// in memory (nothing is written to printers.toml or the Keychain). Repeatable.
    printer_hosts: Vec<String>,
    /// `--printer-camera NAME=URL`: the manual camera URL of a `--printer-host` printer.
    printer_cameras: Vec<String>,
    /// Open the My printers window; `--test-connection NAME` runs and waits for a test.
    my_printers: bool,
    /// `--add-printer found|gallery|setup`: the Add printer window on that screen.
    /// `--mdns 127.0.0.1:PORT` points its network search at a loopback fake responder (the
    /// mock printers' `--mdns`); `--add-printer-search TEXT` / `--add-printer-brand NAME`
    /// fill the gallery; `--add-printer-model NAME` (with `--add-printer-tools N`,
    /// `--add-printer-nozzle RAW`) opens Setup for that model, `--add-printer-address A`
    /// and `--add-printer-key K` fill its connection, `--add-printer-camera URL` its camera
    /// field and `--add-printer-test` runs Test connection and waits for it.
    add_printer: Option<String>,
    mdns: Option<String>,
    add_printer_search: Option<String>,
    add_printer_brand: Option<String>,
    add_printer_model: Option<String>,
    add_printer_tools: Option<usize>,
    add_printer_nozzle: Option<String>,
    add_printer_address: Option<String>,
    add_printer_key: Option<String>,
    add_printer_camera: Option<String>,
    add_printer_test: bool,
    test_connection: Option<String>,
    /// Open the Print dialog on the sliced plate (with `--state sliced`); `--upload` starts
    /// the upload once the check is done, so the passes catch the progress bar.
    print_dialog: bool,
    upload: bool,
    /// `--print-all`: the Print dialog's All plates (its files prepared first).
    print_all: bool,
    /// Show the Machine view: the Printers dashboard, or the named printer
    /// (`--machine-printer NAME`).
    machine: bool,
    machine_printer: Option<String>,
    /// Sync the filaments from the linked printer (after asking it) before rendering.
    sync_filaments: bool,
    /// Milliseconds to spread over the passes so background printer work lands
    /// (`--settle MS`; the printer flags default to 1500).
    settle_ms: Option<u64>,
    /// Preview colour scheme (`--color-by actual-speed`).
    color: Option<crate::preview::ColorMode>,
    /// Measure tool open, and scripted feature clicks (screen points, Shift).
    measure_on: bool,
    measure: Vec<([f32; 2], bool)>,
    /// Section plane: normal and position across the scene.
    section: Option<([f32; 3], f32)>,
    /// Zoom factor applied after framing (`--zoom 1.6` = closer).
    zoom: Option<f32>,
    /// Camera yaw / pitch in degrees after framing (`--orbit 30,25`).
    orbit: Option<[f32; 2]>,
    /// `--library` and its `--library-*` options (see `panels::library::screenshot_setup`).
    library: Vec<String>,
    /// `--mcp CALLS`: Mica MCP tool calls the window runs as in live mode (see [`run_mcp`]).
    mcp: Option<String>,
}

fn parse(args: &[String]) -> Result<Args, String> {
    let mut a = Args {
        size: [1440, 900],
        scale: 1.,
        plates: 1,
        ..Default::default()
    };
    let mut iter = args.iter();
    let value = |iter: &mut std::slice::Iter<'_, String>, flag: &str| {
        iter.next()
            .cloned()
            .ok_or_else(|| format!("{flag} needs a value"))
    };
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--screenshot" => a.output = Some(PathBuf::from(value(&mut iter, "--screenshot")?)),
            "--preview" => a.preview = true,
            "--printer" => a.printer = Some(value(&mut iter, "--printer")?),
            "--xl" => a.xl = true,
            "--select" => {
                a.select = Some(
                    value(&mut iter, "--select")?
                        .parse()
                        .map_err(|_| "--select needs a number")?,
                )
            }
            "--tab" => a.tab = Some(value(&mut iter, "--tab")?),
            "--preview-topic" => a.preview_topic = Some(value(&mut iter, "--preview-topic")?),
            "--advanced" => a.advanced = true,
            "--changed" => a.changed = true,
            "--search" => a.search = Some(value(&mut iter, "--search")?),
            "--object-mode" => a.object_mode = true,
            "--printer-editor" => a.printer_editor = true,
            "--printer-page" => {
                a.printer_page = Some(
                    value(&mut iter, "--printer-page")?
                        .parse()
                        .map_err(|_| "--printer-page N")?,
                )
            }
            "--filament-editor" => {
                a.filament_editor = Some(
                    value(&mut iter, "--filament-editor")?
                        .parse()
                        .map_err(|_| "--filament-editor needs a tool number")?,
                )
            }
            "--all-printers" => a.all_printers = true,
            "--light" => a.light = true,
            "--view" => a.view = Some(value(&mut iter, "--view")?),
            "--focus" => a.focus = true,
            "--pointer" => {
                let v = value(&mut iter, "--pointer")?;
                let (x, y) = v.split_once(',').ok_or("--pointer needs X,Y")?;
                a.pointer = Some([
                    x.trim().parse().map_err(|_| "bad pointer x")?,
                    y.trim().parse().map_err(|_| "bad pointer y")?,
                ]);
            }
            "--collapse" => a.collapse = Some(value(&mut iter, "--collapse")?),
            "--crop" => {
                a.crop = Some(
                    value(&mut iter, "--crop")?
                        .parse()
                        .map_err(|_| "--crop needs a width")?,
                )
            }
            "--gizmo" => a.gizmo = Some(value(&mut iter, "--gizmo")?),
            "--plates" => {
                a.plates = value(&mut iter, "--plates")?
                    .parse()
                    .map_err(|_| "--plates needs a number")?
            }
            "--show-paint" => {
                a.show_paint = value(&mut iter, "--show-paint")?
                    .split(',')
                    .map(|k| k.trim().to_string())
                    .collect();
            }
            "--escape" => a.escape = true,
            "--deselect" => a.deselect = true,
            "--lay-on-face" => a.lay_on_face = true,
            "--colour" | "--color" => {
                let v = value(&mut iter, "--colour")?;
                let usage = "--colour T=#RRGGBB (one-based tool)";
                let (t, c) = v.split_once('=').ok_or(usage)?;
                let t: usize = t.trim().parse().map_err(|_| usage)?;
                let c = mica_profiles::project::parse_colour(c.trim()).ok_or(usage)?;
                a.colours.push((t, c));
            }
            "--plate" => {
                a.plate = Some(
                    value(&mut iter, "--plate")?
                        .parse()
                        .map_err(|_| "--plate needs a number")?,
                )
            }
            "--real-store" => a.real_store = true,
            "--set" => {
                let v = value(&mut iter, "--set")?;
                let (k, v) = v.split_once('=').ok_or("--set needs key=value")?;
                a.overrides.push((k.to_string(), v.to_string()));
            }
            "--plate-set" => {
                let v = value(&mut iter, "--plate-set")?;
                let (k, v) = v.split_once('=').ok_or("--plate-set needs key=value")?;
                a.plate_overrides.push((k.to_string(), v.to_string()));
            }
            "--plate-scope" => a.plate_scope = true,
            "--object-set" => {
                let v = value(&mut iter, "--object-set")?;
                let (k, v) = v.split_once('=').ok_or("--object-set needs key=value")?;
                a.object_overrides.push((k.to_string(), v.to_string()));
            }
            "--size" => {
                let v = value(&mut iter, "--size")?;
                let (w, h) = v.split_once('x').ok_or("--size needs WxH")?;
                a.size = [
                    w.parse().map_err(|_| "bad width")?,
                    h.parse().map_err(|_| "bad height")?,
                ];
            }
            "--scale" => {
                a.scale = value(&mut iter, "--scale")?
                    .parse()
                    .map_err(|_| "bad scale")?
            }
            "--sidebar" => a.sidebar = Some(value(&mut iter, "--sidebar")?),
            "--toolbar" => {
                let v = value(&mut iter, "--toolbar")?;
                a.toolbar = Some(
                    crate::state::ToolbarSize::ALL
                        .into_iter()
                        .find(|t| t.key() == v)
                        .ok_or("--toolbar compact|normal|large")?,
                );
            }
            "--state" => a.state = Some(value(&mut iter, "--state")?),
            "--drag-ring" => {
                let v = value(&mut iter, "--drag-ring")?;
                let mut parts = v.split(',');
                let deg = parts.next().unwrap_or_default();
                let ratio = parts.next().unwrap_or("1");
                a.ring_axis = match parts.next().map(str::trim) {
                    None | Some("z") => Some(2),
                    Some("x") => Some(0),
                    Some("y") => Some(1),
                    Some(_) => return Err("--drag-ring axis is x, y or z".into()),
                };
                a.drag_ring = Some((
                    deg.trim().parse().map_err(|_| "bad drag angle")?,
                    ratio.trim().parse().map_err(|_| "bad drag ratio")?,
                ));
            }
            "--click" => {
                let v = value(&mut iter, "--click")?;
                let (x, y) = v.split_once(',').ok_or("--click needs X,Y")?;
                a.click = Some([
                    x.trim().parse().map_err(|_| "bad click x")?,
                    y.trim().parse().map_err(|_| "bad click y")?,
                ]);
            }
            "--type" => a.typed = Some(value(&mut iter, "--type")?),
            "--drag" => {
                let v = value(&mut iter, "--drag")?;
                let parts: Vec<f32> = v
                    .split(',')
                    .map(|x| x.trim().parse().map_err(|_| "bad drag coordinate"))
                    .collect::<Result<_, _>>()?;
                if parts.len() != 4 {
                    return Err("--drag needs X1,Y1,X2,Y2".into());
                }
                a.drag = Some([parts[0], parts[1], parts[2], parts[3]]);
            }
            "--drag-steps" => {
                let n: usize = value(&mut iter, "--drag-steps")?
                    .parse()
                    .map_err(|_| "--drag-steps needs 1, 2, 4 or 8")?;
                if ![1, 2, 4, 8].contains(&n) {
                    return Err("--drag-steps needs 1, 2, 4 or 8".into());
                }
                a.drag_steps = Some(n);
            }
            "--paint" => a.paint = Some(value(&mut iter, "--paint")?),
            "--brush" => a.brush = Some(value(&mut iter, "--brush")?),
            "--fill-spread" => {
                a.fill_spread = Some(
                    value(&mut iter, "--fill-spread")?
                        .parse()
                        .map_err(|_| "--fill-spread needs a distance in mm")?,
                )
            }
            "--brush-size" => {
                a.brush_size = Some(
                    value(&mut iter, "--brush-size")?
                        .parse()
                        .map_err(|_| "--brush-size needs a radius")?,
                )
            }
            "--paint-tool" => {
                a.paint_tool = Some(
                    value(&mut iter, "--paint-tool")?
                        .parse()
                        .map_err(|_| "--paint-tool needs a tool number")?,
                )
            }
            "--block" => a.block = true,
            "--mix" => a.mixes.push(value(&mut iter, "--mix")?),
            "--mix-editor" => a.mix_editor = Some(value(&mut iter, "--mix-editor")?),
            "--brush-height" => {
                a.brush_height = Some(
                    value(&mut iter, "--brush-height")?
                        .parse()
                        .map_err(|_| "--brush-height needs millimetres")?,
                )
            }
            "--overhangs" => a.overhangs = true,
            "--paint-overhangs" => a.paint_overhangs = true,
            "--object-tool" => a.object_tool = Some(value(&mut iter, "--object-tool")?),
            "--adaptive" => {
                a.adaptive = Some(
                    value(&mut iter, "--adaptive")?
                        .parse()
                        .map_err(|_| "--adaptive needs a quality 0..1")?,
                )
            }
            "--smooth-layers" => {
                a.smooth_layers = Some(
                    value(&mut iter, "--smooth-layers")?
                        .parse()
                        .map_err(|_| "--smooth-layers needs a radius")?,
                )
            }
            "--band" => {
                a.band = Some(
                    value(&mut iter, "--band")?
                        .parse()
                        .map_err(|_| "--band needs a width")?,
                )
            }
            "--brim-ear" => {
                let v = value(&mut iter, "--brim-ear")?;
                let n: Vec<f32> = v
                    .split(',')
                    .map(|x| x.trim().parse().map_err(|_| "--brim-ear needs X,Y,D"))
                    .collect::<Result<_, _>>()?;
                if n.len() != 3 {
                    return Err("--brim-ear needs X,Y,D".into());
                }
                a.brim_ears.push([n[0], n[1], n[2]]);
            }
            "--auto-ears" => a.auto_ears = true,
            "--filament" => {
                let v = value(&mut iter, "--filament")?;
                let (t, name) = v.split_once('=').ok_or("--filament needs T=NAME")?;
                a.filaments.push((
                    t.trim().parse().map_err(|_| "bad filament tool")?,
                    name.trim().to_string(),
                ));
            }
            "--crop-height" => {
                a.crop_height = Some(
                    value(&mut iter, "--crop-height")?
                        .parse()
                        .map_err(|_| "--crop-height needs a height")?,
                )
            }
            "--select-also" => a.select_also.push(
                value(&mut iter, "--select-also")?
                    .parse()
                    .map_err(|_| "--select-also needs a number")?,
            ),
            "--command" => a.commands.push(value(&mut iter, "--command")?),
            "--emboss" => a.emboss.push(value(&mut iter, "--emboss")?),
            "--emboss-close" => a.emboss_close = true,
            "--sub-set" => {
                let v = value(&mut iter, "--sub-set")?;
                let (k, v) = v.split_once('=').ok_or("--sub-set needs key=value")?;
                a.sub_overrides.push((k.to_string(), v.to_string()));
            }
            "--cut-z" => {
                a.cut_z = Some(
                    value(&mut iter, "--cut-z")?
                        .parse()
                        .map_err(|_| "--cut-z needs a height")?,
                )
            }
            "--cut-tilt" => {
                let v = value(&mut iter, "--cut-tilt")?;
                let (x, y) = v.split_once(',').ok_or("--cut-tilt needs X,Y degrees")?;
                a.cut_tilt = Some([
                    x.trim().parse().map_err(|_| "bad tilt x")?,
                    y.trim().parse().map_err(|_| "bad tilt y")?,
                ]);
            }
            "--connector" => {
                let v = value(&mut iter, "--connector")?;
                let (kind, at) = v
                    .split_once('@')
                    .ok_or("--connector needs KIND@X,Y (plug|dowel|snap, plane mm)")?;
                let (x, y) = at.split_once(',').ok_or("--connector needs KIND@X,Y")?;
                let kind = match kind {
                    "plug" => mica_core::cut_connectors::ConnectorType::Plug,
                    "dowel" => mica_core::cut_connectors::ConnectorType::Dowel,
                    "snap" => mica_core::cut_connectors::ConnectorType::Snap,
                    _ => return Err(format!("--connector: unknown type {kind}")),
                };
                a.connectors.push((
                    kind,
                    [
                        x.trim().parse().map_err(|_| "bad connector x")?,
                        y.trim().parse().map_err(|_| "bad connector y")?,
                    ],
                ));
            }
            "--connector-auto" => {
                let v = value(&mut iter, "--connector-auto")?;
                a.connector_auto = Some(
                    *mica_core::cut_connectors::ConnectorType::ALL
                        .iter()
                        .find(|t| t.label().eq_ignore_ascii_case(&v))
                        .ok_or("--connector-auto needs plug|dowel|snap")?,
                );
            }
            "--connector-shape" => a.connector_shape = Some(value(&mut iter, "--connector-shape")?),
            "--connector-size" => {
                let v = value(&mut iter, "--connector-size")?;
                let (d, h) = v
                    .split_once(',')
                    .ok_or("--connector-size needs SIZE,DEPTH")?;
                a.connector_size = Some([
                    d.trim().parse().map_err(|_| "bad connector size")?,
                    h.trim().parse().map_err(|_| "bad connector depth")?,
                ]);
            }
            "--connector-edit" => a.connector_edit = true,
            "--right-click" => {
                let v = value(&mut iter, "--right-click")?;
                let (x, y) = v.split_once(',').ok_or("--right-click needs X,Y")?;
                a.right_click = Some([
                    x.trim().parse().map_err(|_| "bad right-click x")?,
                    y.trim().parse().map_err(|_| "bad right-click y")?,
                ]);
            }
            "--arrange" => a.arrange = Some(value(&mut iter, "--arrange")?),
            "--place" => {
                let v = value(&mut iter, "--place")?;
                let (x, y) = v.split_once(',').ok_or("--place needs X,Y")?;
                a.place = Some([
                    x.trim().parse().map_err(|_| "bad place x")?,
                    y.trim().parse().map_err(|_| "bad place y")?,
                ]);
            }
            "--tilt" => {
                let v = value(&mut iter, "--tilt")?;
                let (x, y) = v.split_once(',').ok_or("--tilt needs X,Y degrees")?;
                a.tilt = Some([
                    x.trim().parse().map_err(|_| "bad tilt x")?,
                    y.trim().parse().map_err(|_| "bad tilt y")?,
                ]);
            }
            "--layer-gcode" => a.layer_gcode = Some(value(&mut iter, "--layer-gcode")?),
            "--layer-menu" => a.layer_menu = true,
            "--layer-dialog" => a.layer_dialog = Some(value(&mut iter, "--layer-dialog")?),
            "--layer" => {
                a.layer = Some(
                    value(&mut iter, "--layer")?
                        .parse()
                        .map_err(|_| "--layer needs a number")?,
                )
            }
            "--save" => a.save = Some(PathBuf::from(value(&mut iter, "--save")?)),
            "--flavour" => a.flavour = Some(value(&mut iter, "--flavour")?),
            "--save-as" => a.save_as = true,
            "--unsaved-prompt" => a.unsaved_prompt = true,
            "--preferences" => a.preferences = true,
            "--preferences-tab" => {
                a.preferences = true;
                a.preferences_tab = Some(value(&mut iter, "--preferences-tab")?);
            }
            "--import-home" => {
                a.import_home = Some(PathBuf::from(value(&mut iter, "--import-home")?));
            }
            "--import-all" => a.import_all = true,
            "--import-review" => a.import_review = true,
            "--process" => a.process = Some(value(&mut iter, "--process")?),
            "--other-printers" => a.other_printers = true,
            "--visible-filaments" => {
                a.visible_filaments = Some(value(&mut iter, "--visible-filaments")?)
            }
            "--texture-gallery" => a.texture_gallery = true,
            "--shortcuts" => a.shortcuts = Some(value(&mut iter, "--shortcuts")?),
            "--about" => a.about = Some(value(&mut iter, "--about")?),
            "--about-search" => a.about_search = Some(value(&mut iter, "--about-search")?),
            "--about-licenses" => a.about_licenses = true,
            "--about-license" => a.about_license = Some(value(&mut iter, "--about-license")?),
            "--brick-layers" => a.brick_layers = true,
            "--welcome" => a.welcome = true,
            "--tour" => {
                let n: usize = value(&mut iter, "--tour")?
                    .parse()
                    .map_err(|_| "--tour N (1-7)")?;
                if !(1..=crate::tour::TOUR_STEPS.len()).contains(&n) {
                    return Err(format!(
                        "--tour: steps are 1-{}",
                        crate::tour::TOUR_STEPS.len()
                    ));
                }
                // The Preview step shows a sliced plate.
                if crate::tour::TOUR_STEPS[n - 1].view == crate::tour::View::Preview {
                    a.preview = true;
                }
                a.tour = Some(n);
            }
            "--whats-new" => a.whats_new = true,
            "--auto-slice" => a.auto_slice = true,
            "--theme" => a.theme = Some(value(&mut iter, "--theme")?),
            "--new-project" => a.new_project = true,
            "--shift-drag" => {
                let v = value(&mut iter, "--shift-drag")?;
                let parts: Vec<f32> = v
                    .split(',')
                    .map(|x| x.trim().parse().map_err(|_| "bad drag coordinate"))
                    .collect::<Result<_, _>>()?;
                if parts.len() != 4 {
                    return Err("--shift-drag needs X1,Y1,X2,Y2".into());
                }
                a.shift_drag = Some([parts[0], parts[1], parts[2], parts[3]]);
            }
            "--select-tower" => a.select_tower = true,
            "--printer-host" => a.printer_hosts.push(value(&mut iter, "--printer-host")?),
            "--my-printers" => a.my_printers = true,
            "--add-printer" => a.add_printer = Some(value(&mut iter, "--add-printer")?),
            "--mdns" => a.mdns = Some(value(&mut iter, "--mdns")?),
            "--add-printer-search" => {
                a.add_printer_search = Some(value(&mut iter, "--add-printer-search")?)
            }
            "--add-printer-brand" => {
                a.add_printer_brand = Some(value(&mut iter, "--add-printer-brand")?)
            }
            "--add-printer-model" => {
                a.add_printer_model = Some(value(&mut iter, "--add-printer-model")?)
            }
            "--add-printer-tools" => {
                a.add_printer_tools = Some(
                    value(&mut iter, "--add-printer-tools")?
                        .parse()
                        .map_err(|_| "--add-printer-tools N")?,
                )
            }
            "--add-printer-nozzle" => {
                a.add_printer_nozzle = Some(value(&mut iter, "--add-printer-nozzle")?)
            }
            "--add-printer-address" => {
                a.add_printer_address = Some(value(&mut iter, "--add-printer-address")?)
            }
            "--add-printer-key" => a.add_printer_key = Some(value(&mut iter, "--add-printer-key")?),
            "--add-printer-camera" => {
                a.add_printer_camera = Some(value(&mut iter, "--add-printer-camera")?)
            }
            "--add-printer-test" => a.add_printer_test = true,
            "--test-connection" => a.test_connection = Some(value(&mut iter, "--test-connection")?),
            "--printer-camera" => a
                .printer_cameras
                .push(value(&mut iter, "--printer-camera")?),
            "--print-dialog" => a.print_dialog = true,
            "--upload" => a.upload = true,
            "--print-all" => a.print_all = true,
            "--machine" => a.machine = true,
            "--machine-printer" => {
                a.machine = true;
                a.machine_printer = Some(value(&mut iter, "--machine-printer")?);
            }
            "--sync-filaments" => a.sync_filaments = true,
            "--settle" => {
                a.settle_ms = Some(
                    value(&mut iter, "--settle")?
                        .parse()
                        .map_err(|_| "--settle needs milliseconds")?,
                )
            }
            "--tower" => {
                let v = value(&mut iter, "--tower")?;
                let parts: Vec<f64> = v
                    .split(',')
                    .map(|s| s.trim().parse::<f64>())
                    .collect::<Result<_, _>>()
                    .map_err(|_| "--tower needs X,Y[,R]")?;
                match parts.as_slice() {
                    [x, y] => a.tower = Some([*x, *y, 0.]),
                    [x, y, r] => a.tower = Some([*x, *y, *r]),
                    _ => return Err("--tower needs X,Y[,R]".into()),
                }
            }
            "--calibrate" => a.calibrate = Some(value(&mut iter, "--calibrate")?),
            "--calibrate-create" => a.calibrate_create = true,
            "--calibrate-result" => {
                a.calibrate_result = Some(value(&mut iter, "--calibrate-result")?)
            }
            "--calibrate-save" => a.calibrate_save = true,
            "--color-by" | "--colour-by" => {
                let v = value(&mut iter, "--color-by")?;
                a.color = Some(
                    crate::preview::ColorMode::parse(&v)
                        .ok_or_else(|| format!("Unknown color scheme {v}"))?,
                );
            }
            "--measure-on" => a.measure_on = true,
            "--zoom" => {
                a.zoom = Some(
                    value(&mut iter, "--zoom")?
                        .parse()
                        .map_err(|_| "--zoom needs a factor")?,
                )
            }
            "--orbit" => {
                let v = value(&mut iter, "--orbit")?;
                let (y, p) = v.split_once(',').ok_or("--orbit needs YAW,PITCH degrees")?;
                a.orbit = Some([
                    y.trim().parse().map_err(|_| "bad yaw")?,
                    p.trim().parse().map_err(|_| "bad pitch")?,
                ]);
            }
            "--measure" => {
                let v = value(&mut iter, "--measure")?;
                let parts: Vec<&str> = v.split(',').map(str::trim).collect();
                let num = |s: &str| s.parse::<f32>().map_err(|_| "--measure needs X,Y[,shift]");
                match parts.as_slice() {
                    [x, y] => a.measure.push(([num(x)?, num(y)?], false)),
                    [x, y, "shift"] => a.measure.push(([num(x)?, num(y)?], true)),
                    _ => return Err("--measure needs X,Y[,shift]".into()),
                }
                a.measure_on = true;
            }
            "--section" => {
                let v = value(&mut iter, "--section")?;
                let parts: Vec<f32> = v
                    .split(',')
                    .map(|s| s.trim().parse::<f32>())
                    .collect::<Result<_, _>>()
                    .map_err(|_| "--section needs NX,NY,NZ[,POS]")?;
                a.section = match parts.as_slice() {
                    [x, y, z] => Some(([*x, *y, *z], 0.5)),
                    [x, y, z, p] => Some(([*x, *y, *z], *p)),
                    _ => return Err("--section needs NX,NY,NZ[,POS]".into()),
                };
            }
            "--mcp" => a.mcp = Some(value(&mut iter, "--mcp")?),
            x if x.starts_with("--library") => {
                a.library.push(x.to_string());
                if crate::panels::library::SHOT_VALUE_FLAGS.contains(&x) {
                    a.library.push(value(&mut iter, x)?);
                }
            }
            x if x.starts_with('-') => return Err(format!("Unknown option {x}")),
            _ => a.files.push(PathBuf::from(arg)),
        }
    }
    if a.output.is_none() {
        return Err("Missing --screenshot path".into());
    }
    Ok(a)
}

pub fn run(args: &[String]) -> Result<(), String> {
    let a = parse(args)?;
    // Screenshots never see the user's own data (preferences, PrusaSlicer's last filament,
    // OrcaSlicer user presets, Mica's settings folder) unless `--real-store` asks for it.
    if !a.real_store {
        mica_core::user_data::isolate();
    }
    let output = a.output.clone().unwrap();
    let ctx = egui::Context::default();
    // A throwaway profile store keeps screenshots from importing into the real one.
    let store = if a.real_store {
        None
    } else {
        let dir = std::env::temp_dir().join(format!("mica-shot-store-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        Some(Store::open(dir))
    };
    // Files are loaded below, once the printer and filaments are chosen.
    let mut app = App::create(
        &ctx,
        Options {
            prefs_path: None,
            store,
            files: vec![],
            headless: true,
        },
    );
    app.set_theme(
        &ctx,
        if a.light {
            crate::theme::Mode::Light
        } else {
            crate::theme::Mode::Dark
        },
    );
    match a.sidebar.as_deref() {
        None => {}
        Some("left") => app.prefs.sidebar_right = false,
        Some("right") => app.prefs.sidebar_right = true,
        Some("hidden") => app.keys.sidebar_hidden = true,
        Some(_) => return Err("--sidebar left|right|hidden".into()),
    }
    if let Some(t) = a.toolbar {
        app.prefs.toolbar_size = t;
    }
    if let Some(g) = &a.gizmo {
        app.viewport.mode = match g.as_str() {
            "move" => crate::viewport::gizmo::Mode::Move,
            "rotate" => crate::viewport::gizmo::Mode::Rotate,
            "scale" => crate::viewport::gizmo::Mode::Scale,
            "none" => crate::viewport::gizmo::Mode::None,
            _ => return Err("--gizmo move|rotate|scale|none".into()),
        };
    }
    for n in 1..a.plates {
        app.plates.add();
        app.plates.plates[n].name = format!("Plate {}", n + 1);
    }
    if a.plates > 1 {
        app.plates.current = 0;
        app.changed();
    }
    if a.xl {
        let xl = app
            .profiles
            .printers
            .iter()
            .find(|p| {
                p.contains("XL") && p.contains("5T") && p.contains("0.4") && p.contains("Input")
            })
            .or_else(|| {
                app.profiles
                    .printers
                    .iter()
                    .find(|p| p.contains("XL") && p.contains("5T") && p.contains("0.4"))
            })
            .cloned()
            .ok_or("No XL 5-tool profile available")?;
        let vendor = app.profiles.vendor;
        app.profiles.select_printer(vendor, &xl);
        app.prefs.add_printer("Prusa", &xl);
        app.settings_changed();
        // Spread filaments over the tools and objects over the filaments.
        let materials = app.profiles.materials.clone();
        let picks = [
            "Prusament PLA",
            "Prusament PETG",
            "Prusament ASA",
            "Generic PLA",
            "Prusament PC",
        ];
        for (i, f) in app.profiles.filaments.iter_mut().enumerate() {
            if let Some(m) = materials
                .iter()
                .find(|m| m.starts_with(picks[i % picks.len()]))
            {
                *f = m.clone();
            }
        }
        app.profiles.refresh();
        app.settings_changed();
    }
    for (t, name) in &a.filaments {
        let m = app
            .profiles
            .materials
            .iter()
            .find(|m| m.starts_with(name.as_str()))
            .cloned()
            .ok_or_else(|| format!("No filament starting with {name}"))?;
        let slot = t.checked_sub(1).ok_or("--filament tools are one-based")?;
        if let Some(f) = app.profiles.filaments.get_mut(slot) {
            *f = m;
        }
        app.profiles.refresh();
        app.settings_changed();
    }
    if let Some(name) = &a.printer {
        let all = app.profiles.all_printers();
        let (vi, _, found) = all
            .iter()
            .find(|(_, _, n)| n == name)
            .ok_or_else(|| format!("No printer preset named {name}"))?
            .clone();
        app.profiles.select_printer(vi, &found);
        app.settings_changed();
    }
    if let Some(home) = &a.import_home {
        let vendor = app.profiles.vendor_name().to_string();
        let printer = app.profiles.printer.clone();
        app.prefs.add_printer(&vendor, &printer);
        app.profiles.look_in(home, None);
        if a.import_all {
            let (rows, _) = app.profiles.import_candidates(&app.prefs);
            let chosen: Vec<_> = rows.iter().filter(|r| r.selected).collect();
            let done = app.profiles.import(&chosen, &mut app.prefs);
            if !done.errors.is_empty() {
                return Err(done.errors.join("; "));
            }
            app.settings_changed();
            // The startup notice named what is imported now.
            app.notifications.list.retain(|n| {
                !n.actions
                    .iter()
                    .any(|(_, act)| *act == crate::notifications::Action::ReviewImports)
            });
        }
        app.offer_imports();
        if a.import_review {
            // The review is where the notice leads; it would cover the window's foot.
            app.notifications.list.retain(|n| {
                !n.actions
                    .iter()
                    .any(|(_, act)| *act == crate::notifications::Action::ReviewImports)
            });
            app.open_import_review();
        }
    }
    if let Some(name) = &a.process {
        if !app.profiles.processes.contains(name) {
            return Err(format!("No print profile named {name} for this printer"));
        }
        app.profiles.process = name.clone();
        app.profiles.refresh();
        app.settings_changed();
    }
    if a.other_printers {
        app.show_presets_for_other_printers(true);
    }
    if let Some(search) = &a.visible_filaments {
        ctx.data_mut(|d| {
            d.insert_temp(
                egui::Id::unique(crate::panels::import::OPEN_VISIBLE_FILAMENTS),
                search.clone(),
            )
        });
    }
    let project = a
        .files
        .iter()
        .any(|f| f.extension().is_some_and(|e| e.eq_ignore_ascii_case("3mf")));
    // G-code files open read-only over the plate (`gcode_doc`); models and projects load.
    let (gcode, models): (Vec<PathBuf>, Vec<PathBuf>) = a
        .files
        .iter()
        .cloned()
        .partition(|p| crate::gcode_doc::is_gcode_path(p));
    if !models.is_empty() {
        let result = crate::app::load_files(models, &app.config, app.bed.center());
        app.finish_load(result);
        if !app.status.is_empty() {
            eprintln!("{}", app.status);
        }
    }
    if let Some(path) = gcode.first() {
        let started = std::time::Instant::now();
        app.open_gcode_now(path)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        let opened = started.elapsed();
        // The validator runs on its thread; wait for it so the passes show its result.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
        while app.gcode.as_ref().is_some_and(|d| d.check_rx.is_some())
            && std::time::Instant::now() < deadline
        {
            app.poll_gcode(&ctx);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        eprintln!(
            "{} (preview ready in {:.3} s, check done in {:.3} s)",
            app.status,
            opened.as_secs_f64(),
            started.elapsed().as_secs_f64()
        );
        if let Some(d) = &app.gcode {
            eprintln!(
                "G-code: {} layers, {} records, printer {:?} → {:?}, check {:?}",
                d.finished.preview.layers.len(),
                d.finished.preview.count,
                d.origin,
                d.printer.preset,
                match &d.check {
                    crate::gcode_doc::Check::Done {
                        errors,
                        warnings,
                        against,
                        ..
                    } => format!("{errors} errors, {warnings} warnings against {against}"),
                    other => format!("{other:?}"),
                }
            );
        }
    }
    if let Some(calls) = &a.mcp {
        run_mcp(&mut app, &ctx, calls)?;
    }
    if let Some(n) = a.plate {
        let i = n.checked_sub(1).ok_or("--plate is one-based")?;
        if i >= app.plates.len() {
            return Err(format!(
                "--plate {n}: the project has {} plates",
                app.plates.len()
            ));
        }
        app.select_plate(i);
    }
    for (k, v) in &a.plate_overrides {
        app.apply_plate_edit(crate::panels::editor::Edit::Set {
            key: k.clone(),
            raw: v.clone(),
        });
        app.end_gesture();
    }
    app.editor.plate_mode = a.plate_scope;
    if a.xl {
        // Spread the objects over the filaments.
        let tools = app.tool_count();
        for (i, o) in app.plates.current_mut().objects.iter_mut().enumerate() {
            o.set_tool(i % tools);
        }
    }
    for (k, v) in &a.overrides {
        // Printer options (`mica_slicing_rules`…) are printer edits, the rest print edits.
        let printer = mica_core::config::option_defs()
            .get(k.as_str())
            .is_some_and(|d| d.scope == mica_core::config::Scope::Printer);
        if printer {
            app.overlays.printer.insert(k.clone(), v.clone());
        } else {
            app.overlays.print.insert(k.clone(), v.clone());
        }
    }
    if !a.overrides.is_empty() {
        app.settings_changed();
    }
    // Several loose models are arranged; a project keeps its own layout.
    if app.plates.current().objects.len() > 1 && !project {
        match a.arrange.as_deref() {
            None | Some("pack") => app.arrange(),
            Some("rows") => {
                let bed = app.bed.clone();
                crate::scene::arrange_rows(&mut app.plates.current_mut().objects, &bed);
                app.changed();
            }
            Some("none") => {}
            Some(_) => return Err("--arrange pack|rows|none".into()),
        }
    }
    // `--advanced` with a `--tab` expands that section; alone it shows everything.
    app.prefs.advanced |= a.advanced && a.tab.is_none();
    if let Some(tab) = &a.tab {
        // Picks that topic in the rail (and, with --advanced, shows all of its settings).
        let name = ["Essentials", "Cooling & temperatures", "Advanced"]
            .into_iter()
            .chain(settings::pages(Tab::Print).iter().map(|p| p.name.as_str()))
            .find(|n| n.eq_ignore_ascii_case(tab))
            .ok_or_else(|| format!("No topic named {tab}"))?
            .to_string();
        if a.advanced && !app.prefs.expanded_sections.contains(&name) {
            app.prefs.expanded_sections.push(name.clone());
        }
        app.prefs.settings_topic = name;
    }
    if let Some(fold) = &a.collapse {
        let id = if fold.contains('/') {
            fold.clone()
        } else {
            format!("{}/{fold}", app.prefs.settings_topic)
        };
        if !app.prefs.collapsed_sections.contains(&id) {
            app.prefs.collapsed_sections.push(id);
        }
    }
    app.editor.changed_only = a.changed;
    if let Some(s) = &a.search {
        app.editor.search = s.clone();
        app.editor.printer_search = s.clone();
        app.editor.filament_search = s.clone();
    }
    app.selected = a.select.filter(|&i| i < app.plates.current().objects.len());
    if let Some(v) = &a.view {
        let aspect = (a.size[0] as f32 - 370.) / (a.size[1] as f32 - 48.);
        match v.as_str() {
            "iso" => app.viewport.camera.preset(0),
            "top" => app.viewport.camera.preset(1),
            "bottom" => app.viewport.camera.preset(2),
            "front" => app.viewport.camera.preset(3),
            "rear" => app.viewport.camera.preset(4),
            "left" => app.viewport.camera.preset(5),
            "right" => app.viewport.camera.preset(6),
            // A slightly oblique look from under the bed (see-through plate).
            "below" => {
                app.viewport.camera.yaw = -1.05;
                app.viewport.camera.pitch = -0.42;
            }
            _ => return Err("--view iso|top|bottom|below|front|rear|left|right".into()),
        }
        let snapshot = crate::scene::plates::snapshot(&app.plates);
        app.viewport
            .fit_plate(&app.bed, &snapshot, a.plates > 1, aspect);
    }
    if let Some([x, y]) = a.place {
        let i = app.selected.unwrap_or(0);
        if let Some(o) = app.plates.current_mut().objects.get_mut(i) {
            let (min, max) = o.bounds();
            let centre = (min + max) * 0.5;
            o.position.x += x - centre.x;
            o.position.y += y - centre.y;
            app.changed();
        }
    }
    if a.focus
        && let Some(i) = app.selected
        && let Some(o) = app.plates.current().objects.get(i)
    {
        let aspect = (a.size[0] as f32 - 370.) / (a.size[1] as f32 - 48.);
        let o = o.clone();
        app.viewport.focus(&o, aspect);
    }
    for &k in &a.select_also {
        if k < app.plates.current().objects.len() {
            app.select(Some(k), true);
        }
    }
    if let (Some([x, y]), Some(i)) = (a.tilt, app.selected)
        && let Some(o) = app.plates.current_mut().objects.get_mut(i)
    {
        let mut e = o.euler();
        e.x += x.to_radians();
        e.y += y.to_radians();
        o.set_euler(e);
        o.drop_to_bed();
        app.changed();
    }
    // The prime tower: a placement on the current plate, then its selection.
    if let Some(t) = a.tower {
        let plate = app.plates.current;
        app.set_tower_placement(plate, t);
        app.changed();
    }
    if a.select_tower {
        if app.tower().is_none() {
            return Err("--select-tower: this plate prints no prime tower".into());
        }
        app.select_tower();
    }
    // Scripted object commands, in order, on the selection.
    for cmd in &a.commands {
        run_command(&mut app, cmd, &ctx)?;
    }
    if let Some(c) = &mut app.cut {
        if let Some(z) = a.cut_z {
            c.set_z(z);
        }
        if let Some([x, y]) = a.cut_tilt {
            c.tilt_x = x.to_radians();
            c.tilt_y = y.to_radians();
        }
        let (mut attributes, mut sizes) = c.connector;
        if let Some([d, h]) = a.connector_size {
            sizes.radius = d / 2.;
            sizes.height = h;
        }
        if let Some(shape) = &a.connector_shape {
            use mica_core::cut_connectors::ConnectorShape;
            attributes.shape = *ConnectorShape::ALL
                .iter()
                .find(|s| s.label().eq_ignore_ascii_case(shape))
                .ok_or_else(|| format!("--connector-shape: unknown shape {shape}"))?;
        }
        for &(kind, [x, y]) in &a.connectors {
            // Each one new (the last placed stays selected and would take the settings).
            c.selected_connector = None;
            c.set_connector(
                mica_core::cut_connectors::Attributes { kind, ..attributes },
                sizes,
            );
            let (ax, ay) = c.axes();
            let p = c.center + ax * x + ay * y;
            c.edit_connectors(crate::cut::ConnectorEdit::Click(p));
        }
        c.editing = a.connector_edit;
        c.wants_connectors |= a.connector_edit;
        if let Some(kind) = a.connector_auto {
            c.selected_connector = None;
            c.set_connector(
                mica_core::cut_connectors::Attributes { kind, ..attributes },
                sizes,
            );
        }
    }
    if a.connector_auto.is_some() {
        app.auto_place_connectors(None);
    }
    if a.commands.iter().any(|c| c == "cut-apply") {
        app.apply_cut();
    }
    for spec in &a.emboss {
        app.emboss_script(spec)?;
    }
    if a.emboss_close {
        app.close_emboss();
    }
    if !a.sub_overrides.is_empty() {
        if app.sub.is_none() {
            return Err(
                "--sub-set needs a selected volume or range (`--command modifier:box`)".into(),
            );
        }
        for (k, v) in &a.sub_overrides {
            app.apply_edit(
                None,
                crate::panels::editor::Edit::Set {
                    key: k.clone(),
                    raw: v.clone(),
                },
            );
        }
        app.end_gesture();
    }
    if a.object_mode || app.sub.is_some() {
        app.editor.object_mode = app.selected.is_some();
    }
    if let Some(i) = app.selected
        && !a.object_overrides.is_empty()
    {
        let o = &mut app.plates.current_mut().objects[i];
        for (k, v) in &a.object_overrides {
            o.overrides.insert(k.clone(), v.clone());
        }
        app.sync_instances(i);
        app.changed();
    }
    for spec in &a.mixes {
        app.add_mix_spec(spec)?;
    }
    if let Some(which) = &a.mix_editor {
        app.open_mix_editor(which)?;
    }
    if let Some(h) = a.brush_height {
        app.paint.height = h;
    }
    if let Some(kind) = &a.paint {
        let kind = match kind.as_str() {
            "support" | "supports" => mica_core::painting::PaintKind::Supports,
            "seam" => mica_core::painting::PaintKind::Seam,
            "colour" | "color" => mica_core::painting::PaintKind::Material,
            "fuzzy" => mica_core::painting::PaintKind::Fuzzy,
            "texture" => mica_core::painting::PaintKind::Texture,
            _ => return Err("--paint support|seam|colour|fuzzy|texture".into()),
        };
        if app.selected.is_none() {
            return Err("--paint needs --select".into());
        }
        app.viewport.paint = Some(kind);
        app.paint_session_for(kind);
    }
    if let Some(b) = &a.brush {
        app.paint.brush = match b.as_str() {
            "circle" => crate::paint::BrushKind::Circle,
            "sphere" => crate::paint::BrushKind::Sphere,
            "fill" => crate::paint::BrushKind::Fill,
            "height" => crate::paint::BrushKind::Height,
            _ => return Err("--brush circle|sphere|fill|height".into()),
        };
    }
    if let Some(r) = a.brush_size {
        app.paint.set_radius(r);
    }
    if let Some(d) = a.fill_spread {
        app.paint.set_fill_spread(d);
    }
    if let Some(t) = a.paint_tool {
        app.paint.tool = t.checked_sub(1).ok_or("--paint-tool is one-based")?;
        app.paint.texture_slot = t.min(usize::from(crate::textures::SLOTS)) as u8;
    }
    app.paint.enforce = !a.block;
    app.paint.show_overhangs = a.overhangs || a.paint_overhangs;
    for kind in &a.show_paint {
        let kind = match kind.as_str() {
            "support" | "supports" => mica_core::painting::PaintKind::Supports,
            "seam" => mica_core::painting::PaintKind::Seam,
            "fuzzy" => mica_core::painting::PaintKind::Fuzzy,
            _ => return Err("--show-paint support,seam,fuzzy".into()),
        };
        app.show_paint.set(kind, 1.);
    }
    if a.paint_overhangs {
        app.paint_overhangs();
    }
    if let Some(tool) = &a.object_tool {
        let tool = match tool.as_str() {
            "layers" | "layer-heights" => crate::viewport::ObjectTool::LayerHeights,
            "ears" | "brim-ears" => crate::viewport::ObjectTool::BrimEars,
            _ => return Err("--object-tool layers|ears".into()),
        };
        if app.selected.is_none() {
            return Err("--object-tool needs --select".into());
        }
        app.viewport.object_tool = Some(tool);
        app.object_tool_changed(Some(tool));
    }
    if let Some(w) = a.band {
        app.layer_edit.set_band_width(w);
    }
    if let Some(q) = a.adaptive {
        app.layer_edit.quality = q.clamp(0., 1.);
        app.layer_adaptive();
    }
    if let Some(r) = a.smooth_layers {
        app.layer_edit.smooth_radius = r.clamp(1, 10);
        app.layer_smooth();
    }
    for e in &a.brim_ears {
        app.brim_ear.set_diameter(e[2]);
        app.add_brim_ear(glam::Vec2::new(e[0], e[1]));
    }
    if a.auto_ears {
        app.auto_place_ears();
    }
    app.dialogs.printer_editor = a.printer_editor;
    if let Some(page) = a.printer_page {
        app.editor.printer_page = page;
    }
    app.dialogs.filament_editor = a.filament_editor;
    app.dialogs.all_printers = a.all_printers;
    // Physical printers (mock servers), kept in memory; tests run and finish before the passes.
    // Screenshots only ever talk to this computer (printers and their cameras).
    if !a.printer_hosts.is_empty() {
        crate::printhost::restrict_to_loopback();
    }
    for spec in &a.printer_hosts {
        let usage = "--printer-host NAME=prusalink|moonraker|u1@ADDRESS[#KEY]";
        let (name, rest) = spec.split_once('=').ok_or(usage)?;
        let (kind, rest) = rest.split_once('@').ok_or(usage)?;
        let (address, key) = rest
            .split_once('#')
            .map_or((rest, None), |(a, k)| (a, Some(k)));
        let kind = match kind {
            "prusalink" => crate::printhost::config::Kind::PrusaLink,
            "moonraker" | "klipper" => crate::printhost::config::Kind::Moonraker,
            "u1" | "snapmaker" => crate::printhost::config::Kind::SnapmakerU1,
            _ => return Err(usage.into()),
        };
        let mut printer = crate::printhost::config::Printer::new(kind);
        printer.name = name.to_string();
        printer.address = address.to_string();
        printer.camera = a
            .printer_cameras
            .iter()
            .filter_map(|c| c.split_once('='))
            .find(|(n, _)| *n == name)
            .map(|(_, url)| url.to_string());
        app.printers.put(None, printer, key)?;
    }
    let wait_tests = |app: &mut App| {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while app
            .printers
            .tests
            .values()
            .any(|t| matches!(t, crate::printhost::Test::Running(_)))
            && std::time::Instant::now() < deadline
        {
            app.printers.drain();
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    };
    if let Some(name) = &a.test_connection {
        let i = app
            .printers
            .index_of(name)
            .ok_or_else(|| format!("--test-connection: no printer named {name}"))?;
        app.printers.test(i, || {});
        wait_tests(&mut app);
        app.printers_editor.selected = Some(i);
        if let Some(crate::printhost::Test::Failed(e)) = app.printers.tests.get(name) {
            eprintln!("Test connection {name}: {} — {}", e.headline, e.advice);
        }
    }
    app.dialogs.my_printers = a.my_printers;
    if let Some(screen) = &a.add_printer {
        let shot = crate::panels::add_printer::Shot {
            screen: screen.clone(),
            mdns: a.mdns.clone(),
            search: a.add_printer_search.clone(),
            brand: a.add_printer_brand.clone(),
            model: a.add_printer_model.clone(),
            tools: a.add_printer_tools,
            nozzle: a.add_printer_nozzle.clone(),
            address: a.add_printer_address.clone(),
            key: a.add_printer_key.clone(),
            camera: a.add_printer_camera.clone(),
            test: a.add_printer_test,
        };
        crate::panels::add_printer::screenshot_setup(&mut app, &ctx, &shot)?;
    }
    if a.sync_filaments {
        app.sync_filaments_from_printer(&ctx);
        wait_tests(&mut app);
        app.finish_pending_sync();
        eprintln!("Sync: {}", app.status);
    }
    if let Some(name) = &a.machine_printer {
        app.machine_printer = Some(
            app.printers
                .index_of(name)
                .and_then(|i| app.printers.id_at(i))
                .ok_or_else(|| format!("--machine-printer: no printer named {name}"))?,
        );
    }
    app.machine = a.machine;
    // A synthetic library in a temporary folder (kept until the image is written).
    let _library = if a.library.is_empty() {
        None
    } else {
        Some(crate::panels::library::screenshot_setup(
            &mut app, &a.library,
        )?)
    };
    // The details' 3D preview: read the model now, so the frames below draw it.
    app.library.sync_preview(&ctx);
    if let Some(pv) = &mut app.library.preview {
        let t = std::time::Instant::now();
        while pv.loading() && t.elapsed() < std::time::Duration::from_secs(60) {
            std::thread::sleep(std::time::Duration::from_millis(10));
            pv.poll(&ctx);
        }
        if let Some(i) = a.library.iter().position(|f| f == "--library-zoom") {
            let z = &a.library[i + 1];
            pv.view.zoom = z
                .parse()
                .map_err(|_| format!("--library-zoom needs a number, not {z}"))?;
        }
        // `--library-plate N`: a project's plate N (one-based) in the 3D preview.
        if let Some(i) = a.library.iter().position(|f| f == "--library-plate") {
            let n = &a.library[i + 1];
            let plates = pv.plates().len().max(1);
            match n.parse::<usize>() {
                Ok(n) if (1..=plates).contains(&n) => pv.set_plate(n - 1),
                _ => return Err(format!("--library-plate: plate 1 to {plates}, not {n}")),
            }
        }
    }
    let flavour = match a.flavour.as_deref() {
        None => None,
        Some("orca") => Some(mica_core::threemf::Flavour::Orca),
        Some("prusa") => Some(mica_core::threemf::Flavour::Prusa),
        Some(_) => return Err("--flavour orca|prusa".into()),
    };
    if a.save_as {
        app.save_as(None);
        if let (Some(f), Some(d)) = (flavour, &mut app.dialogs.save_project) {
            d.flavour = f;
        }
    }
    if a.unsaved_prompt {
        app.dialogs.unsaved = Some(crate::save::Pending::Close);
    }
    if let Some(t) = &a.theme {
        let choice = crate::theme::Choice::from_name(t);
        if choice.name() != t.trim() {
            return Err("--theme dark|light|follow-system".into());
        }
        app.set_theme_choice(&ctx, choice);
    }
    app.prefs.auto_slice |= a.auto_slice;
    if let Some(t) = &a.preferences_tab {
        let tab = crate::panels::preferences::PrefsTab::from_key(t);
        if tab.key() != t.trim() {
            return Err("--preferences-tab general|slicing|profiles|files|features".into());
        }
        app.set_preferences_tab(tab);
    }
    app.dialogs.preferences = a.preferences;
    if a.texture_gallery {
        let current = app
            .effective_config()
            .and_then(|c| c.raw(crate::textures::TEXTURE_KEY))
            .unwrap_or_default();
        crate::textures::open_gallery(&ctx, egui::Id::unique("screenshot-texture-row"), &current);
    }
    if let Some(search) = &a.shortcuts {
        app.keys.dialog = true;
        app.keys.search = search.clone();
    }
    app.prefs.features.brick_layers |= a.brick_layers;
    if let Some(t) = &a.about {
        let tab = crate::panels::about::AboutTab::from_key(t).ok_or("--about about|credits")?;
        app.open_about(tab);
        if let Some(about) = app.dialogs.about.as_mut() {
            let search = a.about_search.clone().unwrap_or_default();
            about.licenses = a.about_licenses;
            if a.about_licenses {
                about.licenses_search = search;
            } else {
                about.search = search;
            }
            about.license = a.about_license.clone();
        }
    }
    if a.whats_new {
        app.open_whats_new();
    }
    if a.welcome {
        app.open_welcome();
    }
    if a.new_project {
        app.new_project();
    }
    // Calibration: the card, its plate (sliced synchronously), a typed result, the save.
    if let Some(spec) = &a.calibrate {
        let (name, tool) = spec.split_once(':').unwrap_or((spec.as_str(), "1"));
        let test = crate::calibration::Test::parse(name)
            .ok_or_else(|| format!("--calibrate: unknown test {name}"))?;
        let tool: usize = tool.parse().map_err(|_| "--calibrate TEST[:TOOL]")?;
        app.open_calibration(test);
        if let Some(card) = &mut app.dialogs.calibration
            && let Some(c) = app.config.as_ref().ok().cloned()
        {
            card.set_tool(
                tool.checked_sub(1)
                    .ok_or("--calibrate tools are one-based")?,
                &c,
            );
        }
        if a.calibrate_create {
            let plate = app.create_calibration_plate(&ctx)?;
            // The worker was started; run the same job here and wait for it instead.
            app.cancel_slice();
            let job = app.plates.plate(plate).calibration.clone().unwrap();
            let config = app.job_config(plate)?;
            let finished = crate::worker::slice_calibration(
                &job.params,
                config,
                &AtomicBool::new(false),
                &|p| eprintln!("{}", p.detail),
            )?;
            eprintln!(
                "Calibration plate: {} — {} samples, {}, {:.1} g, {} layers",
                app.plates.plate(plate).name,
                job.samples.len(),
                crate::panels::duration(finished.summary.time_s),
                finished.summary.grams,
                finished.summary.layers
            );
            app.view = App::view_for(&finished);
            app.plates.plates[plate].finished = Some(finished);
            app.status = "Slice complete".into();
            if let Some(r) = &a.calibrate_result
                && let Some(j) = &mut app.plates.plates[plate].calibration
            {
                j.input = r.clone();
            }
            if let Some(Ok(o)) = app.calibration_outcome() {
                eprintln!(
                    "Calibration result: {} ({}) → {:?}",
                    o.headline, o.detail, o.changes
                );
            }
            if a.calibrate_save {
                app.save_calibration_result()?;
                eprintln!("{}", app.status);
                eprintln!("Filaments now: {:?}", app.profiles.filaments);
            }
        }
    }
    eprintln!(
        "Printer: {}\nProcess: {}\nFilaments: {:?}",
        app.profiles.printer, app.profiles.process, app.profiles.filaments
    );
    if let Some(p) = &app.project {
        eprintln!(
            "Project: {:?} -> {:?}; {} changes; {}",
            p.settings.printer,
            p.printer,
            p.changes,
            match &p.state {
                crate::project::State::Pending => "offered",
                crate::project::State::Applied { .. } => "in use",
                crate::project::State::Dismissed => "dismissed",
            }
        );
    }
    // The tower as drawn (estimate), for comparing with sliced G-code.
    let report_tower = |app: &App| {
        if let Some(t) = app.tower() {
            let (min, max) = t.shape.bbox();
            eprintln!(
                "Tower: {} at X {} Y {} rot {}°, body {:.3} × {:.3} mm, {:.3} mm tall, footprint X {:.3}–{:.3} Y {:.3}–{:.3}, {}",
                t.label(),
                t.shape.position[0],
                t.shape.position[1],
                t.shape.rotation,
                t.shape.width,
                t.shape.depth,
                t.shape.height,
                min[0],
                max[0],
                min[1],
                max[1],
                if t.shape.exact { "exact" } else { "estimated" }
            );
        }
    };
    report_tower(&app);
    if let Some(spec) = &a.layer_gcode {
        let tools = app.tool_count();
        let (object_tools, base) = app.plate_object_tools(app.plates.current);
        let colors = app.tool_colors_f32();
        let info = parse_layer_gcode(spec, tools, object_tools, base, &colors)?;
        app.plates.current_mut().custom_gcode = info;
    }
    if a.tour.is_some() {
        // The tour's sample plate, before `--preview` slices it.
        app.tour_sample();
        app.select(None, false);
    }
    if a.preview && app.plates.current().finished.is_some() {
        // Already sliced (a calibration plate): just show it.
        app.preview = true;
    } else if a.preview {
        let config = app.plate_config(app.plates.current)?;
        let objects = app.plates.current().objects.clone();
        if objects.is_empty() {
            return Err("--preview needs a model".into());
        }
        let custom = app.plates.current().custom_gcode.clone();
        let finished = crate::worker::slice_with_custom_gcode(
            objects,
            custom,
            config,
            &AtomicBool::new(false),
            &|p| eprintln!("{}", p.detail),
        )?;
        eprintln!(
            "Sliced: {} layers, {}, {:.2} m, {:.1} g; tools {:?}",
            finished.summary.layers,
            crate::panels::duration(finished.summary.time_s),
            finished.summary.filament_mm / 1000.,
            finished.summary.grams,
            finished
                .summary
                .tools
                .iter()
                .map(|t| (t.tool, t.filament.as_str()))
                .collect::<Vec<_>>()
        );
        app.view = App::view_for(&finished);
        if let Some(mode) = a.color {
            app.view.color = mode;
        }
        if let Some(n) = a.layer {
            app.view.high = n
                .saturating_sub(1)
                .min(finished.preview.layers.len().saturating_sub(1));
        }
        let zs: Vec<f64> = finished.preview.layer_zs();
        app.plates.current_mut().finished = Some(finished);
        app.preview = true;
        app.status = "Slice complete".into();
        app.view.layer_ui.menu = a.layer_menu;
        if let Some(d) = &a.layer_dialog {
            let (kind, text) = d.split_once(':').unwrap_or((d.as_str(), ""));
            app.view.layer_ui.dialog = Some(crate::preview::layer_gcode::Dialog {
                kind: match kind {
                    "pause" => mica_core::custom_gcode::Kind::PausePrint,
                    "custom" => mica_core::custom_gcode::Kind::Custom,
                    _ => return Err("--layer-dialog pause|custom[:TEXT]".into()),
                },
                print_z: zs.get(app.view.high).copied().unwrap_or(0.),
                text: text.replace("\\n", "\n"),
            });
        }
        app.refresh_towers();
        report_tower(&app);
    }
    if let Some(topic) = &a.preview_topic {
        app.preview_side.open_topic(topic);
    }
    match a.state.as_deref() {
        None => {}
        Some("slicing") => {
            // The worker runs in the background while the passes render.
            app.slice_current(&ctx);
        }
        Some(state @ ("sliced" | "outdated")) => {
            let config = app.plate_config(app.plates.current)?;
            let objects = app.plates.current().objects.clone();
            match crate::worker::slice(objects, config, &AtomicBool::new(false), &|p| {
                eprintln!("{}", p.detail)
            }) {
                Ok(finished) => {
                    app.plates.current_mut().finished = Some(finished);
                    app.status = "Slice complete".into();
                }
                // What the window shows when a slice fails.
                Err(e) => {
                    eprintln!("slice failed: {e}");
                    let plate = app.plates.current;
                    app.slice_failed(plate, e);
                }
            }
            if state == "outdated" {
                // A later change drops the slice and keeps its totals for the read-out.
                app.overlays
                    .print
                    .insert("fill_density".into(), "25%".into());
                app.settings_changed();
            }
        }
        Some(_) => return Err("--state slicing|sliced|outdated".into()),
    }
    for &(t, colour) in &a.colours {
        let tool = t.checked_sub(1).ok_or("--colour tools are one-based")?;
        app.set_filament_colour(
            tool,
            crate::panels::colour::Pick {
                colour,
                commit: true,
            },
        );
        app.end_gesture();
        eprintln!(
            "Color T{t} = {} (slice kept: {})",
            crate::project::hex(colour),
            app.finished().is_some()
        );
    }
    if a.lay_on_face {
        app.viewport.lay_on_face = true;
        app.viewport.planes.blocking = true;
        for o in &app.plates.current().objects {
            let planes = app.viewport.planes.planes(o, &ctx).unwrap_or_default();
            eprintln!(
                "Lay on face: {} has {} planes (largest {:.0} mm²)",
                o.name,
                planes.len(),
                planes.first().map_or(0., |p| p.area)
            );
        }
    }
    if a.print_dialog {
        app.open_print_dialog();
        if app.print_job.is_none() {
            return Err("--print-dialog needs a sliced plate (--state sliced)".into());
        }
        if app.print_job.as_ref().is_some_and(|d| d.target.is_none()) {
            return Err(
                "--print-dialog: no printer fits the profile; add one with --printer-host".into(),
            );
        }
        // All plates: their files first (slicing the others).
        if a.print_all {
            if let Some(d) = &mut app.print_job {
                d.choose_all(true);
            }
            app.prepare_send_all(&ctx);
            wait_batch(&mut app, &ctx);
            app.preview = false;
        }
        // The check runs now, so the passes show its result (and `--upload` the progress).
        app.start_check(&ctx);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while app.print_job.as_ref().is_some_and(|d| d.busy())
            && std::time::Instant::now() < deadline
        {
            if let Some(d) = &mut app.print_job {
                d.poll();
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        if let Some(d) = &app.print_job {
            eprintln!("Print dialog: {:?}", d.stage);
        }
        if a.upload && a.print_all {
            app.start_upload_all(&ctx);
        } else if a.upload {
            // As the Upload button: an opened G-code file with errors asks first.
            app.request_upload(&ctx);
        }
    }
    let printer_work =
        !a.printer_hosts.is_empty() || a.machine || a.print_dialog || a.add_printer.is_some();
    let settle = a
        .settle_ms
        .or_else(|| printer_work.then_some(1500))
        .map(std::time::Duration::from_millis);
    if let Some(mode) = a.color {
        app.view.color = mode;
    }
    if let Some([yaw, pitch]) = a.orbit {
        app.viewport.camera.yaw = yaw.to_radians();
        app.viewport.camera.pitch = pitch.to_radians();
    }
    if let Some(z) = a.zoom.filter(|z| *z > 0.) {
        app.viewport.camera.span /= z;
        app.viewport.camera.span_goal = app.viewport.camera.span;
    }
    if a.measure_on && !app.viewport.measure_on {
        app.viewport.toggle_measure();
    }
    if let Some((n, position)) = a.section {
        let plane = crate::viewport::section::Plane {
            normal: glam::Vec3::from(n).normalize_or(glam::Vec3::Z),
            position,
        };
        if app.preview {
            app.viewport.section.preview = Some(plane);
        } else {
            app.viewport.section.prepare = Some(plane);
        }
    }
    if a.deselect {
        app.select(None, false);
    }
    if let Some(n) = a.tour {
        app.start_tour(&ctx);
        app.tour_jump(n - 1, &ctx);
    }
    let crop = Crop {
        width: a.crop,
        height: a.crop_height,
        right: app.prefs.sidebar_right,
    };
    let script = Script {
        pointer: a.pointer,
        click: a.click,
        typed: a.typed.clone(),
        drag_ring: a.drag_ring,
        ring_axis: a.ring_axis.unwrap_or(2),
        drag: a.drag,
        drag_steps: a.drag_steps.unwrap_or(8),
        escape: a.escape,
        right_click: a.right_click,
        shift_drag: a.shift_drag,
        settle,
        measure: a.measure.clone(),
    };
    block_on(render(
        &output, &ctx, &mut app, a.size, a.scale, crop, script,
    ))?;
    eprintln!("Rendered {}", output.display());
    if let Some(path) = &a.save {
        // After the passes, so a scripted paint stroke is in the file too.
        let flavour = flavour.unwrap_or_else(|| app.default_flavour());
        app.save_project_to(path, flavour)?;
        eprintln!("Saved {} ({})", path.display(), flavour.name());
    }
    Ok(())
}

/// `--layer-gcode` items: `KIND:Z[:ARG]` separated by `;` (`color:Z[:#RRGGBB]`,
/// `pause:Z[:message]`, `custom:Z:G-code` with `\n` for line breaks, `tool:Z:N` one based,
/// `template:Z`).
fn parse_layer_gcode(
    spec: &str,
    tools: usize,
    object_tools: usize,
    base: usize,
    colors: &[[f32; 3]],
) -> Result<mica_core::custom_gcode::Info, String> {
    use mica_core::custom_gcode::{Info, Item, Kind, Mode};
    let mut info = Info {
        mode: Mode::of_print(tools, object_tools),
        ..Info::default()
    };
    for part in spec.split(';').filter(|p| !p.trim().is_empty()) {
        let mut f = part.splitn(3, ':');
        let kind = f.next().unwrap_or("").trim();
        let z: f64 = f
            .next()
            .and_then(|z| z.trim().parse().ok())
            .ok_or_else(|| format!("--layer-gcode: {part:?} needs a height"))?;
        let arg = f.next().unwrap_or("").replace("\\n", "\n");
        let item = match kind {
            "color" => {
                let g = crate::preview::layer_gcode::LayerGCode {
                    info: &info,
                    tools,
                    object_tools,
                    base_tool: base,
                    klipper: false,
                    has_template: false,
                    tool_colors: colors,
                };
                let mut item = g.color_change(z, base);
                if !arg.is_empty() {
                    item.color = arg;
                }
                item
            }
            "pause" => Item {
                extra: arg,
                ..Item::new(z, Kind::PausePrint, base + 1)
            },
            "custom" => Item {
                extra: arg,
                ..Item::new(z, Kind::Custom, base + 1)
            },
            "template" => Item::new(z, Kind::Template, base + 1),
            "tool" => {
                let n: usize = arg
                    .trim()
                    .parse()
                    .map_err(|_| format!("--layer-gcode: {part:?} needs a tool number"))?;
                Item {
                    color: crate::preview::color_print::to_hex(
                        colors
                            .get(n.saturating_sub(1))
                            .copied()
                            .unwrap_or_else(|| crate::preview::legend::tool_color(n - 1)),
                    ),
                    ..Item::new(z, Kind::ToolChange, n.max(1))
                }
            }
            _ => return Err(format!("--layer-gcode: unknown kind {kind:?}")),
        };
        info.put(item);
    }
    Ok(info)
}

/// `--mcp CALLS`: runs Mica MCP tool calls in the window exactly as live mode does when an AI
/// assistant is attached (the requests go through the window's own queue, as from the socket),
/// so a shot shows what the AI made and the "Mica MCP connected" badge. CALLS is a JSON array
/// of `{"tool": "...", "args": {...}}`, inline or in a file; a failed call stops the shot.
fn run_mcp(app: &mut App, ctx: &egui::Context, calls: &str) -> Result<(), String> {
    use mica_cli::mcp::live::{PROTOCOL, Request};
    let text = if calls.trim_start().starts_with('[') {
        calls.to_string()
    } else {
        std::fs::read_to_string(calls).map_err(|e| format!("--mcp {calls}: {e}"))?
    };
    let calls: Vec<serde_json::Value> =
        serde_json::from_str(&text).map_err(|e| format!("--mcp: {e}"))?;
    let (host, tx) = crate::mcp_live::Host::in_process();
    app.mcp = host;
    app.mcp.attach_in_process_client();
    for (id, call) in (1u64..).zip(&calls) {
        let tool = call["tool"]
            .as_str()
            .ok_or("--mcp: every call needs a \"tool\"")?;
        let args = call
            .get("args")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));
        let (reply, answer) = std::sync::mpsc::channel();
        tx.send(crate::mcp_live::Incoming {
            request: Request {
                v: PROTOCOL,
                id,
                tool: tool.into(),
                args,
            },
            reply,
        })
        .map_err(|e| format!("--mcp {tool}: {e}"))?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
        let response = loop {
            app.poll(ctx);
            app.poll_mcp(ctx);
            if let Ok(r) = answer.try_recv() {
                break r;
            }
            if std::time::Instant::now() > deadline {
                return Err(format!("--mcp {tool}: no answer after 300 s"));
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        };
        if !response.ok {
            return Err(format!(
                "--mcp {tool}: {}",
                response.error.unwrap_or_default()
            ));
        }
        eprintln!("Mica MCP: {tool} done");
    }
    Ok(())
}

/// One `--command` on the app's selection; `cut-apply` runs later, after the plane flags.
/// Polls until Slice all plates (and whatever waited for it) is done.
fn wait_batch(app: &mut App, ctx: &egui::Context) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(600);
    while (app.busy() || app.slice_batch.is_some()) && std::time::Instant::now() < deadline {
        app.poll(ctx);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

fn run_command(app: &mut App, cmd: &str, ctx: &egui::Context) -> Result<(), String> {
    use crate::commands::{Command, Shape};
    let (name, arg) = cmd.split_once(':').unwrap_or((cmd, ""));
    let shape = || {
        Shape::parse(arg)
            .or_else(|| arg.is_empty().then_some(Shape::Box))
            .ok_or_else(|| format!("--command {name}: unknown shape {arg}"))
    };
    let tower = match name {
        "tower-free" => Some(crate::tower::TowerCommand::FreeSpot),
        "tower-disable" => Some(crate::tower::TowerCommand::Disable),
        "tower-settings" => Some(crate::tower::TowerCommand::Settings),
        _ => None,
    };
    if let Some(c) = tower {
        if app.tower().is_none() {
            return Err(format!(
                "--command {name}: this plate prints no prime tower"
            ));
        }
        app.select_tower();
        app.run_tower_command(c);
        return Ok(());
    }
    if let Some(r) = app.mesh_tool_script(name, arg) {
        return r;
    }
    // The object tree and "+ Add" (gui-objects.md): `intent:stronger|finer|supports|
    // no-supports|smooth|hole|empty`, `part:NAME` (select a part by name), `group:NAME` (the
    // selected part into a group), `scope:profile|object|part`, `add-menu` (opens "+ Add").
    // Add shape: `add-shape:MODEL[@object|part|modifier|negative|enforcer|blocker]` (a
    // built-in model, e.g. `add-shape:torus@modifier`), `shape-gallery` (opens the toolbar's
    // Add menu with the gallery).
    match name {
        "add-shape" => {
            use crate::builtin::AddAs;
            let (model, kind) = arg.split_once('@').unwrap_or((arg, "object"));
            let m = mica_core::builtin_models::BuiltinModel::parse(model)
                .ok_or_else(|| format!("--command add-shape: unknown model {model}"))?;
            let add_as = AddAs::ALL
                .into_iter()
                .find(|a| a.label().eq_ignore_ascii_case(kind))
                .ok_or_else(|| format!("--command add-shape: unknown kind {kind}"))?;
            app.add_builtin(m, add_as);
            return Ok(());
        }
        "shape-gallery" => {
            app.viewport.open_add_menu = true;
            return Ok(());
        }
        "intent" => {
            use crate::intents::Intent;
            let i = match arg {
                "stronger" => Intent::Stronger,
                "finer" => Intent::Finer,
                "supports" => Intent::Supports,
                "no-supports" => Intent::NoSupports,
                "smooth" => Intent::Smooth,
                "hole" => Intent::Hole,
                "empty" => Intent::EmptyModifier,
                other => return Err(format!("--command intent: unknown intent {other}")),
            };
            app.add_intent(i);
            return Ok(());
        }
        "part" => {
            let i = app.selected.ok_or("--command part needs --select")?;
            if arg.is_empty() {
                // `part:` goes up to the object.
                app.sub = None;
                return Ok(());
            }
            let o = &app.plates.current().objects[i];
            let sub = crate::tree::tree_order(o)
                .into_iter()
                .find(|s| crate::tree::part_name(o, *s) == arg)
                .ok_or_else(|| format!("--command part: no part {arg}"))?;
            app.sub = Some(sub);
            app.editor.object_mode = true;
            return Ok(());
        }
        "group" => {
            let (Some(i), Some(sub)) = (app.selected, app.sub) else {
                return Err("--command group needs a selected part".into());
            };
            let name = arg.to_string();
            app.tree_edit(i, "Group", |o| crate::tree::group(o, &[sub], &name));
            return Ok(());
        }
        "scope" => {
            // The scope bar follows the selection first; then the scope is the one asked for.
            app.follow_selection_scope();
            match arg {
                "profile" => app.editor.object_mode = false,
                "object" => {
                    app.editor.object_mode = true;
                    app.editor.object_over_part = true;
                }
                _ => {
                    app.editor.object_mode = true;
                    app.editor.object_over_part = false;
                }
            }
            return Ok(());
        }
        "add-menu" => {
            app.editor.open_add_menu = true;
            return Ok(());
        }
        // T (the Text tool's key and button): text on the selected (or only) model.
        "text-key" => {
            app.toggle_emboss();
            return Ok(());
        }
        // Plates: `plate-menu` (the plate bar's ⋯ menu), `plate-list` (its plate list),
        // `plate-name:NAME` (names the current plate), `rename-plate[:TEXT]` (the bar's name
        // field open, TEXT typed), `move-plate:FROM,TO` (one-based), `duplicate-plate`.
        "plate-menu" => {
            app.viewport.open_plate_menu = true;
            return Ok(());
        }
        "plate-list" => {
            app.viewport.open_plate_list = true;
            return Ok(());
        }
        "plate-name" => {
            app.rename_plate(app.plates.current, arg);
            return Ok(());
        }
        "rename-plate" => {
            app.plate_command(app.plates.current, crate::plate_menu::PlateCommand::Rename);
            if !arg.is_empty()
                && let Some(r) = app.viewport.plate_rename.as_mut()
            {
                r.text = arg.to_string();
            }
            return Ok(());
        }
        "move-plate" => {
            let (from, to) = arg
                .split_once(',')
                .and_then(|(a, b)| {
                    Some((
                        a.trim().parse::<usize>().ok()?,
                        b.trim().parse::<usize>().ok()?,
                    ))
                })
                .filter(|(a, b)| *a >= 1 && *b >= 1)
                .ok_or("--command move-plate:FROM,TO (one-based)")?;
            app.move_plate(from - 1, to - 1);
            return Ok(());
        }
        // Several plates at once: `slice-all` (Slice all plates, waiting for it),
        // `slice-all-start` (started; the passes show its progress), `slice-menu` /
        // `export-menu` (the header's split button menus), `export-all:DIR` (Export all plates
        // into DIR, waiting for it), `plate-off-bed:N` (plate N's first object far off its bed,
        // so its slice fails).
        "slice-all" | "slice-all-start" => {
            app.slice_all(ctx);
            if name == "slice-all" {
                wait_batch(app, ctx);
                app.preview = false;
            }
            eprintln!("Plates: {:?}", app.plate_statuses());
            return Ok(());
        }
        "slice-menu" | "export-menu" => {
            crate::panels::topbar::request_menu(ctx, &name.replace('-', " "));
            return Ok(());
        }
        "export-all" => {
            if arg.is_empty() {
                return Err("--command export-all:DIR".into());
            }
            let dir = PathBuf::from(arg);
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            app.export_all_to(dir, ctx);
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(600);
            while app.export_all.as_ref().is_some_and(|e| e.stage.busy())
                && std::time::Instant::now() < deadline
            {
                app.poll(ctx);
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            app.preview = false;
            return Ok(());
        }
        "plate-off-bed" => {
            let n = arg
                .parse::<usize>()
                .ok()
                .filter(|n| (1..=app.plates.len()).contains(n))
                .ok_or("--command plate-off-bed:N (one-based)")?;
            let o = app.plates.plates[n - 1]
                .objects
                .first_mut()
                .ok_or("that plate has no objects")?;
            o.position.x += 2000.;
            app.plates.plates[n - 1].invalidate();
            app.changed();
            return Ok(());
        }
        "duplicate-plate" => {
            app.plate_command(
                app.plates.current,
                crate::plate_menu::PlateCommand::Duplicate,
            );
            return Ok(());
        }
        _ => {}
    }
    let command = match name {
        "duplicate" => Command::Duplicate,
        "copy" => Command::Copy,
        "paste" => Command::Paste,
        "instance" => Command::AddInstance,
        "instance-remove" => Command::RemoveInstance,
        "modifier" => Command::AddModifier(shape()?),
        "negative" => Command::AddNegative(shape()?),
        "enforcer" => Command::AddSupportEnforcer(shape()?),
        "blocker" => Command::AddSupportBlocker(shape()?),
        "range" => Command::AddHeightRange,
        "arrange" => Command::Arrange,
        "arrange-all" => Command::ArrangeAll,
        "orient" => Command::AutoOrient,
        "select-all" => Command::SelectAll,
        "cut" => Command::Cut,
        "cut-apply" => return Ok(()),
        "split-objects" => Command::SplitToObjects,
        "split-parts" => Command::SplitToParts,
        "centre" | "center" => Command::Centre,
        "drop" => Command::DropToBed,
        "mirror-x" => Command::Mirror(0),
        "mirror-y" => Command::Mirror(1),
        "mirror-z" => Command::Mirror(2),
        "rotate90" => Command::Rotate90,
        "delete" => Command::Delete,
        "settings" => Command::ObjectSettings,
        "move-inside" => Command::MoveInside,
        "add-text" => Command::AddText,
        "edit-text" => Command::EditText,
        other => return Err(format!("--command: unknown command {other}")),
    };
    app.run_command(command, ctx);
    Ok(())
}

/// Which part of the window to keep: `width` keeps the sidebar's side (left or right),
/// `height` the top.
#[derive(Clone, Copy, Default)]
struct Crop {
    width: Option<u32>,
    height: Option<u32>,
    right: bool,
}

/// Pointer and keyboard events injected over the passes.
#[derive(Clone, Default)]
struct Script {
    pointer: Option<[f32; 2]>,
    click: Option<[f32; 2]>,
    /// Typed after the click: the text on one pass, Enter pressed and released on the next two.
    typed: Option<String>,
    drag_ring: Option<(f32, f32)>,
    /// The ring `drag_ring` drags (0 = X, 1 = Y, 2 = Z).
    ring_axis: usize,
    /// A released left drag with intermediate moves (`[x1, y1, x2, y2]`).
    drag: Option<[f32; 4]>,
    /// Pointer moves of the drag: 8 (default), 4, 2 or 1 (a fast drag samples sparsely).
    drag_steps: usize,
    /// Esc pressed and released after the drag (leaves the painting tool).
    escape: bool,
    /// A secondary-button click in the middle passes (opens the context menu there).
    right_click: Option<[f32; 2]>,
    /// A left drag with Shift held (rectangle selection), released before the final passes.
    shift_drag: Option<[f32; 4]>,
    /// Wall-clock time spread over the passes, for background printer work to land.
    settle: Option<std::time::Duration>,
    /// Measure clicks (screen point, Shift) made after the first pass.
    measure: Vec<([f32; 2], bool)>,
}
impl Script {
    /// The events of pass `pass` (of `passes`): a ring drag presses on the handle, then moves
    /// to the target and stays pressed; a click presses and releases; the hover pointer is
    /// sent on every pass that has nothing else to do.
    fn events(
        &self,
        pass: usize,
        passes: usize,
        ring: Option<(egui::Pos2, egui::Pos2)>,
    ) -> Vec<egui::Event> {
        let at = |p: [f32; 2]| egui::Pos2::new(p[0], p[1]);
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        if let Some(c) = self.right_click {
            // The right-click lands in the middle so the menu has passes to open and settle.
            let c = at(c);
            let secondary = |pressed| egui::Event::PointerButton {
                pos: c,
                button: egui::PointerButton::Secondary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            };
            let mid = passes / 2;
            return match pass {
                p if p + 2 == mid => vec![egui::Event::PointerMoved(c)],
                p if p + 1 == mid => vec![secondary(true)],
                p if p == mid => vec![secondary(false)],
                p if p < mid => vec![],
                _ => self
                    .pointer
                    .map(|p| vec![egui::Event::PointerMoved(at(p))])
                    .unwrap_or_default(),
            };
        }
        if let Some([x1, y1, x2, y2]) = self.shift_drag {
            let a = egui::Pos2::new(x1, y1);
            let b = egui::Pos2::new(x2, y2);
            let shifted = |pos, pressed| egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::SHIFT,
            };
            // Shift goes down before the press (egui tracks it from `ModifiersChanged`)
            // and comes up after the release.
            return match pass {
                0 => vec![],
                1 => vec![
                    egui::Event::ModifiersChanged(egui::Modifiers::SHIFT),
                    egui::Event::PointerMoved(a),
                ],
                2 => vec![shifted(a, true)],
                3..=10 => {
                    let t = (pass - 2) as f32 / 8.;
                    vec![egui::Event::PointerMoved(a.lerp(b, t))]
                }
                11 => vec![shifted(b, false)],
                12 => vec![egui::Event::ModifiersChanged(egui::Modifiers::NONE)],
                _ => self
                    .pointer
                    .map(|p| vec![egui::Event::PointerMoved(at(p))])
                    .unwrap_or_else(|| vec![egui::Event::PointerMoved(b)]),
            };
        }
        if let Some([x1, y1, x2, y2]) = self.drag {
            // Press at the start, eight moves towards the end, release, then hover there.
            let a = egui::Pos2::new(x1, y1);
            let b = egui::Pos2::new(x2, y2);
            let esc = |pressed| egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            };
            return match pass {
                0 => vec![],
                1 => vec![egui::Event::PointerMoved(a)],
                2 => vec![button(a, true)],
                // `drag_steps` of the eight passes move the pointer (fewer = a faster drag).
                3..=10
                    if ((pass - 2)
                        * if self.drag_steps == 0 {
                            8
                        } else {
                            self.drag_steps
                        })
                    .is_multiple_of(8) =>
                {
                    let t = (pass - 2) as f32 / 8.;
                    vec![egui::Event::PointerMoved(a.lerp(b, t))]
                }
                3..=10 => vec![],
                11 => vec![button(b, false)],
                12 if self.escape => vec![esc(true)],
                13 if self.escape => vec![esc(false)],
                // Away from the model after Esc, so no hover highlight remains.
                _ if self.escape => vec![egui::Event::PointerMoved(egui::Pos2::new(40., 300.))],
                _ => vec![egui::Event::PointerMoved(b)],
            };
        }
        if let Some((handle, target)) = ring {
            return match pass {
                0 => vec![],
                1 => vec![egui::Event::PointerMoved(handle)],
                2 => vec![button(handle, true)],
                // Two moves, so the drag threshold is passed before the target is read.
                3 => vec![
                    egui::Event::PointerMoved(handle.lerp(target, 0.5)),
                    egui::Event::PointerMoved(target),
                ],
                _ => vec![egui::Event::PointerMoved(target)],
            };
        }
        if let Some(c) = self.click {
            let c = at(c);
            // The click goes in the middle (earlier when text follows); hover afterwards.
            let mid = if self.typed.is_some() {
                passes / 2 - 2
            } else {
                passes / 2
            };
            if pass + 2 == mid {
                return vec![egui::Event::PointerMoved(c)];
            }
            if pass + 1 == mid {
                return vec![button(c, true)];
            }
            if pass == mid {
                return vec![button(c, false)];
            }
            if pass < mid {
                return vec![];
            }
            if let Some(text) = &self.typed {
                let enter = |pressed| egui::Event::Key {
                    key: egui::Key::Enter,
                    physical_key: None,
                    pressed,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                };
                // One pass for the field to open and take focus, then the text, then Enter.
                if pass == mid + 2 {
                    return vec![egui::Event::Text(text.clone())];
                }
                if pass == mid + 3 {
                    return vec![enter(true)];
                }
                if pass == mid + 4 {
                    return vec![enter(false)];
                }
                if pass <= mid + 4 {
                    return vec![];
                }
            }
        }
        self.pointer
            .map(|p| vec![egui::Event::PointerMoved(at(p))])
            .unwrap_or_default()
    }
}

async fn render(
    path: &std::path::Path,
    ctx: &egui::Context,
    app: &mut App,
    size: [u32; 2],
    scale: f32,
    crop: Crop,
    script: Script,
) -> Result<(), String> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions::default())
        .await
        .map_err(|e| e.to_string())?;
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor::default())
        .await
        .map_err(|e| e.to_string())?;
    let mut renderer =
        egui_wgpu::Renderer::new(&device, FORMAT, egui_wgpu::RendererOptions::default());
    renderer
        .callback_resources
        .insert(render::Renderer::new(&device, FORMAT));
    let pixels = [
        (size[0] as f32 * scale) as u32,
        (size[1] as f32 * scale) as u32,
    ];
    let screen = egui_wgpu::ScreenDescriptor {
        size_in_pixels: pixels,
        pixels_per_point: scale,
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Mica window screenshot"),
        size: wgpu::Extent3d {
            width: pixels[0],
            height: pixels[1],
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&Default::default());
    // Several passes let panels, scroll areas and windows settle before the final frame;
    // scripted pointer input needs a few more.
    let passes = if script.drag.is_some() && script.escape {
        17
    } else if script.drag.is_some() || script.shift_drag.is_some() {
        14
    } else if script.typed.is_some() {
        12
    } else if script.click.is_some() || script.drag_ring.is_some() || script.right_click.is_some() {
        7
    } else if script.settle.is_some() {
        12
    } else {
        4
    };
    let mut ring: Option<(egui::Pos2, egui::Pos2)> = None;
    for pass in 0..passes {
        if pass > 0
            && let Some(settle) = script.settle
        {
            std::thread::sleep(settle / (passes as u32 - 1));
        }
        // The last frame shows the details' 3D preview at the size asked for (and faded in).
        if pass + 1 == passes
            && let Some(pv) = &mut app.library.preview
        {
            let t = std::time::Instant::now();
            while t.elapsed() < std::time::Duration::from_secs(30) {
                pv.poll(ctx);
                // A finer mesh for the zoom is asked for at once and waited for.
                if pv.error().is_some() || (pv.settled() && pv.fade() >= 1. && !pv.refine_pending())
                {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
        if pass == 1
            && let Some((deg, ratio)) = script.drag_ring
            && let Some(i) = app.selected
            && let Some(o) = app.plates.current().objects.get(i)
        {
            // The viewport rect is known after the first pass.
            let o = o.clone();
            ring = app.viewport.ring_screen_points(
                &o,
                app.viewport_rect,
                script.ring_axis,
                deg.to_radians(),
                ratio,
            );
        }
        if pass == 1 && !script.measure.is_empty() {
            // The viewport rect is known after the first pass.
            let (objects, _) = app.scene_objects();
            let rect = app.viewport_rect;
            for (at, shift) in &script.measure {
                let picked = app.viewport.measure_pick(
                    &objects,
                    rect,
                    egui::Pos2::new(at[0], at[1]),
                    *shift,
                );
                eprintln!(
                    "Measure click {:?}: {}",
                    at,
                    if picked { "picked" } else { "nothing" }
                );
            }
            for row in app.viewport.measure.rows() {
                eprintln!("  {}: {}", row.label, row.value);
            }
        }
        let mut input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(size[0] as f32, size[1] as f32),
            )),
            time: Some(pass as f64 * 0.1),
            predicted_dt: 0.1,
            max_texture_side: Some(device.limits().max_texture_dimension_2d as usize),
            events: script.events(pass, passes, ring),
            ..Default::default()
        };
        input
            .viewports
            .entry(egui::ViewportId::ROOT)
            .or_default()
            .native_pixels_per_point = Some(scale);
        let output = ctx.run_ui(input, |ui| app.show(ui));
        for (id, deltas) in &output.textures_delta.set {
            for delta in deltas {
                renderer.update_texture(&device, &queue, *id, delta);
            }
        }
        let clipped = ctx.tessellate(output.shapes, output.pixels_per_point);
        let mut encoder = device.create_command_encoder(&Default::default());
        let buffers = renderer.update_buffers(&device, &queue, &mut encoder, &clipped, &screen);
        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Mica egui pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                ..Default::default()
            });
            let mut pass = pass.forget_lifetime();
            renderer.render(&mut pass, &clipped, &screen);
        }
        queue.submit(buffers.into_iter().chain([encoder.finish()]));
        for id in &output.textures_delta.free {
            renderer.free_texture(id);
        }
    }
    if let Some(e) = renderer
        .callback_resources
        .get::<render::Renderer>()
        .and_then(|r| r.error.clone())
    {
        return Err(e);
    }
    let [w, h] = pixels;
    let stride = (w * 4).div_ceil(256) * 256;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Screenshot readback"),
        size: (stride * h) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(stride),
                rows_per_image: Some(h),
            },
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([encoder.finish()]);
    let (tx, rx) = std::sync::mpsc::channel();
    buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .map_err(|e| e.to_string())?;
    rx.recv()
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    let data = buffer
        .slice(..)
        .get_mapped_range()
        .map_err(|e| e.to_string())?;
    // `crop.width` keeps the sidebar's side only (sidebar close-ups), `crop.height` the top.
    let keep = crop.width.map_or(w, |c| ((c as f32 * scale) as u32).min(w));
    let keep_h = crop
        .height
        .map_or(h, |c| ((c as f32 * scale) as u32).min(h));
    let x0 = if crop.right { (w - keep) as usize } else { 0 };
    let mut rgb = Vec::with_capacity((keep * keep_h * 3) as usize);
    for row in data.chunks_exact(stride as usize).take(keep_h as usize) {
        for pixel in row[x0 * 4..(x0 + keep as usize) * 4].as_chunks::<4>().0 {
            rgb.extend_from_slice(&pixel[..3]);
        }
    }
    let png =
        mica_gcode::thumbnails::encode_png_rgb(keep, keep_h, &rgb).map_err(|e| e.to_string())?;
    std::fs::write(path, png).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn arguments() {
        let a = parse(
            &[
                "--screenshot",
                "o.png",
                "a.stl",
                "--xl",
                "--select",
                "1",
                "--tab",
                "Speed",
                "--size",
                "800x600",
                "--scale",
                "2",
            ]
            .map(String::from),
        )
        .unwrap();
        assert_eq!(a.files, [PathBuf::from("a.stl")]);
        assert!(a.xl);
        assert_eq!(a.select, Some(1));
        assert_eq!(a.tab.as_deref(), Some("Speed"));
        assert_eq!(a.size, [800, 600]);
        assert_eq!(a.scale, 2.);
        assert!(parse(&["--bogus".to_string()]).is_err());
        assert!(parse(&["a.stl".to_string()]).is_err());
        let a = parse(
            &[
                "--screenshot",
                "o.png",
                "a.stl",
                "--drag-ring",
                "40,0.5",
                "--sidebar",
                "left",
            ]
            .map(String::from),
        )
        .unwrap();
        assert_eq!(a.drag_ring, Some((40., 0.5)));
        assert_eq!(a.sidebar.as_deref(), Some("left"));
        let a = parse(&["--screenshot", "o.png", "--toolbar", "large"].map(String::from)).unwrap();
        assert_eq!(a.toolbar, Some(crate::state::ToolbarSize::Large));
        assert!(parse(&["--screenshot", "o.png", "--toolbar", "huge"].map(String::from)).is_err());
        let a = parse(&["--screenshot", "o.png", "a.stl", "--drag-ring", "90"].map(String::from))
            .unwrap();
        assert_eq!(a.drag_ring, Some((90., 1.)));
        assert_eq!(a.ring_axis, Some(2));
        let a =
            parse(&["--screenshot", "o.png", "a.stl", "--drag-ring", "30,1,x"].map(String::from))
                .unwrap();
        assert_eq!((a.drag_ring, a.ring_axis), (Some((30., 1.)), Some(0)));
        assert!(
            parse(&["--screenshot", "o.png", "a.stl", "--drag-ring", "30,1,w"].map(String::from))
                .is_err()
        );
        let a = parse(
            &[
                "--screenshot",
                "o.png",
                "a.stl",
                "--tower",
                "100,120",
                "--select-tower",
            ]
            .map(String::from),
        )
        .unwrap();
        assert_eq!(a.tower, Some([100., 120., 0.]));
        assert!(a.select_tower);
        let a = parse(
            &[
                "--screenshot",
                "o.png",
                "a.stl",
                "--color-by",
                "actual-speed",
                "--measure",
                "10,20",
                "--measure",
                "30,40,shift",
                "--section",
                "0,-1,0,0.4",
            ]
            .map(String::from),
        )
        .unwrap();
        assert_eq!(a.color, Some(crate::preview::ColorMode::ActualSpeed));
        assert!(a.measure_on);
        assert_eq!(a.measure, vec![([10., 20.], false), ([30., 40.], true)]);
        assert_eq!(a.section, Some(([0., -1., 0.], 0.4)));
        assert!(
            parse(&["--screenshot", "o.png", "--color-by", "bogus"].map(String::from)).is_err()
        );
        let a = parse(&["--screenshot", "o.png", "a.stl", "--tower", "10,20,90"].map(String::from))
            .unwrap();
        assert_eq!(a.tower, Some([10., 20., 90.]));
        assert!(parse(&["--screenshot", "o.png", "--tower", "10"].map(String::from)).is_err());
    }
    #[test]
    fn scripted_events() {
        let s = Script {
            click: Some([10., 20.]),
            pointer: Some([30., 40.]),
            typed: None,
            drag_ring: None,
            ring_axis: 2,
            drag: None,
            drag_steps: 8,
            escape: false,
            right_click: None,
            shift_drag: None,
            settle: None,
            measure: vec![],
        };
        // 7 passes: move, press, release at pass 1..=3, then the hover pointer.
        assert!(s.events(0, 7, None).is_empty());
        assert!(matches!(
            s.events(1, 7, None)[..],
            [egui::Event::PointerMoved(_)]
        ));
        assert!(matches!(
            s.events(2, 7, None)[..],
            [egui::Event::PointerButton { pressed: true, .. }]
        ));
        assert!(matches!(
            s.events(3, 7, None)[..],
            [egui::Event::PointerButton { pressed: false, .. }]
        ));
        assert!(matches!(s.events(6, 7, None)[..], [egui::Event::PointerMoved(p)] if p.x == 30.));
        let ring = Some((egui::Pos2::new(0., 0.), egui::Pos2::new(100., 0.)));
        let d = Script {
            drag_ring: Some((45., 1.)),
            ..Default::default()
        };
        assert!(matches!(
            d.events(2, 7, ring)[..],
            [egui::Event::PointerButton { pressed: true, .. }]
        ));
        assert_eq!(d.events(3, 7, ring).len(), 2);
        assert!(matches!(d.events(6, 7, ring)[..], [egui::Event::PointerMoved(p)] if p.x == 100.));
        // `--type`: 12 passes, the click ends at pass 4, the text goes in at 6, Enter at 7 / 8.
        let t = Script {
            click: Some([10., 20.]),
            typed: Some("200".into()),
            ..Default::default()
        };
        assert!(matches!(
            t.events(4, 12, None)[..],
            [egui::Event::PointerButton { pressed: false, .. }]
        ));
        assert!(t.events(5, 12, None).is_empty());
        assert!(matches!(&t.events(6, 12, None)[..], [egui::Event::Text(s)] if s == "200"));
        assert!(matches!(
            t.events(7, 12, None)[..],
            [egui::Event::Key {
                key: egui::Key::Enter,
                pressed: true,
                ..
            }]
        ));
        assert!(matches!(
            t.events(8, 12, None)[..],
            [egui::Event::Key {
                key: egui::Key::Enter,
                pressed: false,
                ..
            }]
        ));
        assert!(t.events(11, 12, None).is_empty());
        let a =
            parse(&["--screenshot", "o.png", "a.stl", "--type", "200"].map(String::from)).unwrap();
        assert_eq!(a.typed.as_deref(), Some("200"));
        // `--drag`: 14 passes; press at 2, eight moves, release at 11, hover at the end.
        let d = Script {
            drag: Some([0., 0., 80., 0.]),
            ..Default::default()
        };
        assert!(matches!(
            d.events(2, 14, None)[..],
            [egui::Event::PointerButton { pressed: true, .. }]
        ));
        assert!(
            matches!(d.events(6, 14, None)[..], [egui::Event::PointerMoved(p)] if (p.x - 40.).abs() < 1e-4)
        );
        assert!(matches!(
            d.events(11, 14, None)[..],
            [egui::Event::PointerButton { pressed: false, .. }]
        ));
        assert!(matches!(d.events(13, 14, None)[..], [egui::Event::PointerMoved(p)] if p.x == 80.));
        let a = parse(
            &[
                "--screenshot",
                "o.png",
                "a.stl",
                "--paint",
                "support",
                "--drag",
                "1,2,3,4",
                "--brush",
                "sphere",
                "--brush-size",
                "5",
                "--paint-tool",
                "2",
                "--filament",
                "2=Prusament PETG",
            ]
            .map(String::from),
        )
        .unwrap();
        assert_eq!(a.paint.as_deref(), Some("support"));
        assert_eq!(a.drag, Some([1., 2., 3., 4.]));
        assert_eq!(a.brush.as_deref(), Some("sphere"));
        assert_eq!(a.brush_size, Some(5.));
        assert_eq!(a.paint_tool, Some(2));
        assert_eq!(a.filaments, vec![(2, "Prusament PETG".to_string())]);
    }
}
