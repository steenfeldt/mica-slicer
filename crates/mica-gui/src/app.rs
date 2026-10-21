// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c770b8380ee81c921a2867d522f288123 src/slic3r-arrange-wrapper/src/SceneBuilder.cpp :: get_skirt_inset, brim_offset
// Source: prusaslicer@b028299c770b8380ee81c921a2867d522f288123 src/slic3r-arrange-wrapper/include/arrange-wrapper/Scene.hpp :: SceneBuilderBase::build_scene
// Changes: Original Mica application; arrange2 spacing/margin adapter, fixed per-plate towers and instance identities preserved during overflow.
// Source: orca@8500fcdcca src/slic3r/GUI/3DScene.cpp :: adjust_color_for_rendering
// Changes: Near-black display colours lifted so the brightest channel reaches 0.2, keeping the hue (Orca greys them).
// Changes: Engine choices read mica_slicing_rules (mica_core::rules), not gcode_flavor or the presence of Orca keys (docs/slicing-rules.md).

//! Application state and lifecycle: profiles and preferences, plates and objects, undo,
//! background slicing (manual and automatic), and the top-level window layout. The panels
//! themselves live in `panels/`.

use crate::{
    overlays::{Overlays, Target},
    panels::profiles::{PresetKind, Profiles},
    plates::Plates,
    preview::View,
    render,
    scene::{self, Bed, Delta, Object, camera::Camera, plates},
    state::{Prefs, undo::Undo},
    theme,
    viewport::{Action, Viewport},
    worker::{self, Event, Worker},
};
use eframe::egui;
use glam::Vec3;
use mica_core::config::Config;
use mica_profiles::user::Store;
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

pub type LoadResult = Result<LoadedScene, String>;

/// What opening files produces: objects with the plate each belongs to (`None`: the current
/// plate), a project's plates, and its own settings when it has them.
pub struct LoadedScene {
    pub plates: Vec<crate::plates::Plate>,
    pub objects: Vec<(Option<usize>, Object)>,
    pub warnings: Vec<String>,
    pub tower: Option<[f64; 3]>,
    pub paths: Vec<PathBuf>,
    pub settings: Option<mica_profiles::project::ProjectSettings>,
    /// The archive's own settings text, kept for a verbatim re-save.
    pub config: Option<mica_core::threemf::RawConfig>,
    /// A project (3MF) replaces the scene and becomes the document; models are imported.
    pub project: bool,
    /// Per-layer custom G-code of a project without plates (PrusaSlicer's bed 0).
    pub custom_gcode: mica_core::custom_gcode::Info,
    /// The project's mixed colors and their file text.
    pub mixes: Option<(mica_core::full_spectrum::FullSpectrum, String)>,
}

/// Editor state that is not persisted.
#[derive(Default)]
pub struct Editor {
    /// Global settings or the selected object's overrides.
    pub object_mode: bool,
    /// The sidebar shows the current plate's own settings ("Settings for This plate").
    pub plate_mode: bool,
    /// The sidebar shows the prime tower's settings.
    pub tower_mode: bool,
    pub search: String,
    /// The process card shows only settings that differ from the preset, across all topics.
    pub changed_only: bool,
    /// A topic was picked in the rail: the list scrolls back to the topic's header once.
    pub topic_picked: bool,
    /// What the settings list drew last frame (tests and screenshots read it).
    pub sidebar_probe: crate::panels::topics::Probe,
    pub printer_page: usize,
    pub printer_extruder: usize,
    pub filament_page: usize,
    pub filament_search: String,
    pub printer_search: String,
    /// The object tree's fold states and inline group rename (`panels/tree.rs`).
    pub tree: crate::panels::tree::TreeState,
    /// "Save recipe": the name being typed for the selected part's recipe.
    pub recipe_name: Option<String>,
    /// The sidebar edits the selected object although a part of it is selected (the scope
    /// bar's object segment); cleared when the part selection changes.
    pub object_over_part: bool,
    /// Open the tree's "+ Add" menu on the next frame (screenshots).
    pub open_add_menu: bool,
    /// The selection (object id, part) the scope bar last followed.
    pub scope_seen: Option<(u64, Option<crate::scene::Sub>)>,
    /// The scope bar's segments as drawn last frame (tests).
    pub scope_probe: Vec<(crate::panels::scope::Segment, egui::Rect)>,
}

pub struct SaveAs {
    pub kind: PresetKind,
    pub tool: usize,
    pub name: String,
    pub error: Option<String>,
}

#[derive(Default)]
pub struct Dialogs {
    pub printer_editor: bool,
    pub filament_editor: Option<usize>,
    pub all_printers: bool,
    pub all_printers_filter: String,
    pub save_as: Option<SaveAs>,
    /// "Print…": the send-to-printer dialog (its state is `App::print_job`).
    pub print: bool,
    /// "My printers": the physical printers Mica can send to.
    pub my_printers: bool,
    /// "Add a printer": found on the network, pick a model, set it up.
    pub add_printer: bool,
    /// "Save project as": name and format, then the file picker.
    pub save_project: Option<crate::save::SaveProject>,
    /// "Save changes to X?" and what waits for the answer.
    pub unsaved: Option<crate::save::Pending>,
    /// The open Calibrate card (one test at a time).
    pub calibration: Option<crate::calibration::Card>,
    /// The Preferences window (⌘,).
    pub preferences: bool,
    /// The About window (with the Credits tab).
    pub about: Option<crate::panels::about::About>,
    /// The Welcome window (first launch, Help › Welcome to Mica…).
    pub welcome: Option<crate::panels::welcome::Welcome>,
    /// What's new in this version (once after an update, Help › What's new…).
    pub whats_new: bool,
}

#[derive(Clone, PartialEq)]
pub(crate) struct PlateState {
    pub name: String,
    pub objects: Vec<Object>,
    pub overrides: BTreeMap<String, String>,
    pub locked: bool,
    pub metadata: BTreeMap<String, String>,
    pub tower: Option<[f64; 3]>,
    pub calibration: Option<crate::calibration::PlateJob>,
    pub custom_gcode: mica_core::custom_gcode::Info,
}

/// Everything undo restores.
#[derive(Clone, PartialEq)]
pub struct Snapshot {
    plates: Vec<PlateState>,
    current: usize,
    selected: Option<usize>,
    multi: std::collections::BTreeSet<usize>,
    sub: Option<scene::Sub>,
    tower_selected: bool,
    overlays: Overlays,
    /// The project's mixed colors.
    mixes: Vec<mica_core::full_spectrum::VirtualExtruder>,
}

impl crate::state::undo::Retained for Snapshot {
    fn retained(&self, seen: &mut std::collections::HashSet<usize>) -> usize {
        self.plates
            .iter()
            .flat_map(|p| &p.objects)
            .map(|o| o.retained(seen))
            .sum()
    }
}

impl Snapshot {
    /// What differs between two snapshots, in words (the monkey test's undo report).
    pub(crate) fn diff(&self, other: &Self) -> Vec<String> {
        let mut out = vec![];
        if self.plates.len() != other.plates.len() {
            out.push(format!(
                "{} plates → {}",
                self.plates.len(),
                other.plates.len()
            ));
        }
        for (i, (a, b)) in self.plates.iter().zip(&other.plates).enumerate() {
            if a.objects.len() != b.objects.len() {
                out.push(format!(
                    "plate {i}: {} objects → {}",
                    a.objects.len(),
                    b.objects.len()
                ));
            } else {
                for (k, (x, y)) in a.objects.iter().zip(&b.objects).enumerate() {
                    if x != y {
                        out.push(format!("plate {i} object {k}: {x:?} → {y:?}"));
                    }
                }
            }
            if a.name != b.name {
                out.push(format!("plate {i} name {:?} → {:?}", a.name, b.name));
            }
            if a.tower != b.tower {
                out.push(format!("plate {i} tower {:?} → {:?}", a.tower, b.tower));
            }
            if a.locked != b.locked || a.overrides != b.overrides || a.metadata != b.metadata {
                out.push(format!("plate {i} lock/overrides/metadata"));
            }
            if a.custom_gcode != b.custom_gcode {
                out.push(format!(
                    "plate {i} layer G-code: {} → {} items",
                    a.custom_gcode.items.len(),
                    b.custom_gcode.items.len()
                ));
            }
        }
        if self.current != other.current {
            out.push(format!(
                "current plate {} → {}",
                self.current, other.current
            ));
        }
        if self.selected != other.selected || self.multi != other.multi {
            out.push(format!(
                "selection {:?}+{:?} → {:?}+{:?}",
                self.selected, self.multi, other.selected, other.multi
            ));
        }
        if self.sub != other.sub {
            out.push(format!("sub {:?} → {:?}", self.sub, other.sub));
        }
        if self.tower_selected != other.tower_selected {
            out.push("tower selection".into());
        }
        if self.overlays != other.overlays {
            out.push(format!(
                "settings overlays {:?} → {:?}",
                self.overlays, other.overlays
            ));
        }
        if self.mixes != other.mixes {
            out.push(format!(
                "mixed colors {} → {}",
                self.mixes.len(),
                other.mixes.len()
            ));
        }
        out
    }
}

/// How to start the app: the real window or a headless screenshot.
pub struct Options {
    /// Where preferences are read and written; `None` never touches the disk.
    pub prefs_path: Option<PathBuf>,
    /// Profile store; `None` uses the default location.
    pub store: Option<Store>,
    pub files: Vec<PathBuf>,
    pub headless: bool,
}

pub struct App {
    pub(crate) profiles: Profiles,
    pub(crate) prefs: Prefs,
    pub(crate) prefs_path: Option<PathBuf>,
    pub(crate) prefs_dirty: bool,
    pub(crate) overlays: Overlays,
    pub(crate) config: Result<Config, String>,
    pub(crate) bed: Bed,
    pub(crate) plates: Plates,
    /// The primary selected object (the tool card and the sidebar scope follow it).
    pub(crate) selected: Option<usize>,
    /// Further selected objects (Shift / Cmd-click, box select); never holds `selected`.
    pub(crate) multi: std::collections::BTreeSet<usize>,
    /// The selected volume or height range of the primary object, if any.
    pub(crate) sub: Option<scene::Sub>,
    /// Saved part recipes (`intents.rs`), kept next to the preferences.
    pub(crate) recipes: crate::intents::Recipes,
    /// The current plate's prime tower is selected (instead of objects).
    pub(crate) tower_selected: bool,
    /// Each plate's prime / wipe tower as drawn, when the plate prints one (see `tower.rs`).
    pub(crate) towers: Vec<Option<crate::tower::TowerView>>,
    /// Cached tower estimates per plate (a drag only translates them).
    pub(crate) tower_cache: std::collections::HashMap<
        usize,
        (crate::tower::TowerKey, mica_core::wipe_tower::TowerShape),
    >,
    /// Bumped whenever the effective configuration is rebuilt (cache keys).
    pub(crate) config_generation: u64,
    /// The global configuration on another bed type, for plates with their own (built on
    /// first use, cleared with the configuration).
    bed_configs: std::cell::RefCell<std::collections::HashMap<String, Result<Config, String>>>,
    /// Completed slices by what determines their G-code (see `slice_cache`).
    pub(crate) slice_cache: crate::slice_cache::SliceCache,
    /// The inputs of the running job: `(request, inputs)`.
    job_inputs: Option<(u64, Arc<crate::slice_cache::Inputs>)>,
    /// Plates (ids) whose slice came back from the cache, for the monkey test's check that
    /// it equals a fresh slice.
    pub(crate) restored: Vec<u64>,
    /// Slice the current plate on the next frame (export / print after a colour change).
    pub(crate) reslice_requested: bool,
    /// Cmd+C: objects with their settings, painting, volumes and ranges.
    pub(crate) clipboard: Vec<Object>,
    /// An object being renamed inline in the list: (index, text).
    pub(crate) renaming: Option<(usize, String)>,
    /// The cut tool's plane while it is open.
    pub(crate) cut: Option<crate::cut::CutState>,
    /// The open mesh tool card (boolean or simplify), see `mesh_tools`.
    pub(crate) mesh_tool: Option<crate::mesh_tools::MeshTool>,
    /// The Text / SVG tool's card while it is open (`emboss::panel`).
    pub(crate) emboss: Option<crate::emboss::EmbossTool>,
    pub(crate) viewport: Viewport,
    /// The running slice job and the id of its plate (`Plate::id`, never an index: plates
    /// can be removed or reordered while it runs). At most one job runs at a time.
    pub(crate) worker: Option<(u64, Worker)>,
    /// A cancelled job that has not confirmed it stopped yet. Nothing starts until it has.
    pub(crate) stopping: Option<Worker>,
    /// The latest slice request made while a job was running or stopping: `(plate id,
    /// manual, request)`. Requests coalesce, so pressing Slice five times runs one more job.
    pub(crate) pending_slice: Option<(u64, bool, u64)>,
    /// Slice requests are numbered so a finished job can be matched to the request it answers.
    pub(crate) slice_requests: u64,
    pub(crate) finished_request: Option<u64>,
    /// Plates (ids) Slice all still has to do, last first.
    pub(crate) slice_queue: Vec<u64>,
    /// Slice all plates (or the slicing before Export all plates / sending all plates) while
    /// it runs (`plate_batch`).
    pub(crate) slice_batch: Option<crate::plate_batch::SliceBatch>,
    /// Export all plates while its window shows.
    pub(crate) export_all: Option<crate::plate_batch::ExportAll>,
    pub(crate) manual_slice: bool,
    pub(crate) loading: Option<mpsc::Receiver<LoadResult>>,
    pub(crate) preview: bool,
    pub(crate) view: View,
    pub(crate) status: String,
    /// Overall progress of the running job, 0 ..= 1.
    pub(crate) progress: f32,
    /// The stage the job is in (`Preparing · seam planning`) and its own fraction.
    pub(crate) progress_stage: String,
    pub(crate) progress_fraction: f32,
    pub(crate) memory: Arc<AtomicU64>,
    pub(crate) undo: Undo<Snapshot>,
    pub(crate) editor: Editor,
    pub(crate) dialogs: Dialogs,
    /// Importing profiles from other slicers: the review window and Settings › Profiles.
    pub(crate) import_ui: crate::panels::import::ImportUi,
    pub(crate) dirty_since: Option<Instant>,
    /// The preview view to keep when a layer G-code edit re-slices (same layers, same range).
    pub(crate) keep_view: Option<View>,
    pub(crate) warnings: Vec<String>,
    /// Sequential printing: each current-plate object's clearance outline (bed mm) and
    /// whether it collides, indexed like the plate's objects.
    pub(crate) clearance: Vec<(Vec<[f32; 2]>, bool)>,
    pub(crate) guards: Vec<crate::checks::Guard>,
    pub(crate) notices: Vec<String>,
    pub(crate) headless: bool,
    /// Where the 3D view was drawn last frame (floating panels are placed relative to it).
    pub(crate) viewport_rect: egui::Rect,
    prefs_saved_at: Option<Instant>,
    /// Guard-rail messages the user dismissed (until the plate changes).
    pub(crate) dismissed_guards: Vec<String>,
    /// What the latest settings edit was (`infill → 20 %`), shown under the slice delta.
    pub(crate) last_change: Option<String>,
    /// Paint overlays by object id, rebuilt only when an object's painting changes.
    overlay_cache: std::collections::HashMap<u64, scene::paint::PaintOverlay>,
    /// Brush settings of the painting tools.
    pub(crate) paint: crate::paint::Settings,
    /// The shortcuts window, Shift+Tab's hidden sidebar and focus requests (`keymap.rs`).
    pub(crate) keys: crate::keymap::KeyState,
    /// The selector over the object being painted, while a painting tool is active.
    pub(crate) paint_session: Option<crate::paint::Session>,
    /// The overhang highlight of the painted object (key, overlay), while shown.
    overhang_overlay: Option<scene::paint::PaintOverlay>,
    /// What a Fill click at the pointer would paint, while the Fill brush hovers the object.
    fill_preview: Option<crate::paint::FillPreview>,
    /// The variable layer height tool's card settings and its gradient overlay (by key).
    pub(crate) layer_edit: crate::layer_edit::Settings,
    pub(crate) layer_overlay: Option<scene::paint::PaintOverlay>,
    /// The height the card's graph is being dragged at (mm above the object's bottom).
    pub(crate) layer_graph_z: Option<f64>,
    /// The brim ears tool's card settings.
    pub(crate) brim_ear: crate::brim_ears::Settings,
    /// Loose brim ears of the selected object, by a key of its ears and placement.
    pub(crate) ears_loose: Option<(u64, Vec<usize>)>,
    /// When `status` last changed, so transient messages fade from the hint line.
    pub(crate) status_at: Instant,
    last_status: String,
    /// The opened project's own settings and the state of its card, if the 3MF had any.
    pub(crate) project: Option<crate::project::Prompt>,
    /// The project's own values that differ from the current presets (recomputed with the
    /// configuration; the card lists them).
    pub(crate) project_differences: Vec<mica_profiles::project::Difference>,
    /// View menu: paint kinds shown on every object even outside their tool (non-zero =
    /// on). Colour painting is always shown; see `paint_show`.
    pub(crate) show_paint: scene::paint::Show,
    /// The project file, its flavour and the state at the last open or save.
    pub(crate) document: crate::save::Document,
    /// Set once the unsaved-changes prompt let the window close.
    pub(crate) closing: bool,
    /// The window title last sent to the OS.
    last_title: String,
    /// Physical printers, their secrets' store, test results and live monitors.
    pub(crate) printers: crate::printhost::Printers,
    /// The Machine view (third header tab) is showing.
    pub(crate) machine: bool,
    /// The printer the Machine view shows, by `Printer::id` (never an index: printers can be
    /// removed while the view remembers one).
    pub(crate) machine_printer: Option<u64>,
    pub(crate) machine_view: crate::panels::machine::MachineView,
    /// The open Print dialog's state machine.
    pub(crate) print_job: Option<crate::printhost::job::Dialog>,
    pub(crate) printers_editor: crate::panels::printers::Editor,
    /// The Add printer window's state (network search, model catalog, setup).
    pub(crate) add_printer: crate::panels::add_printer::AddPrinter,
    /// The Preview sidebar (legend, totals, layer, folded print settings).
    pub(crate) preview_side: crate::panels::preview_side::PreviewSide,
    /// Errors, warnings and confirmations, stacked in the 3D view's corner (`notifications`).
    pub(crate) notifications: crate::notifications::Notifications,
    /// The current plate's objects whose skirt or brim would not fit on the bed (by object
    /// index), from the pre-slice check (`worker::adhesion_overrun`).
    pub(crate) adhesion: Vec<(usize, mica_core::adhesion_check::Overrun)>,
    /// Recent failed slices `(plate id, plate epoch, message)`, for the monkey test's check
    /// that a failed slice always shows its error.
    pub(crate) failed_slices: Vec<(u64, u64, String)>,
    /// Each plate's marker for its label (`Plate::id` → worst level): a failed slice, or
    /// warnings about its objects.
    pub(crate) plate_alerts: std::collections::HashMap<u64, crate::notifications::Level>,
    /// The pre-slice skirt and brim check by object state (see `adhesion_key`), so edits
    /// elsewhere do not section every object again.
    adhesion_cache: std::cell::RefCell<
        std::collections::HashMap<u64, Option<mica_core::adhesion_check::Overrun>>,
    >,
    /// A filament sync waiting for the printer's answer (test connection in flight), by
    /// `Printer::id`.
    pub(crate) sync_pending: Option<u64>,
    /// Printer-vs-project filament differences the user chose to ignore, by
    /// (`Printer::id`, tool), until what that printer reports as loaded changes (the value is
    /// what was loaded when ignored).
    pub(crate) filament_ignored: std::collections::HashMap<(u64, usize), String>,
    /// The linked printer's file header the configuration was composed with; a printer
    /// added, edited or removed elsewhere recomposes it (`App::check_file_identity`).
    pub(crate) applied_identity: Option<&'static str>,
    /// What Moonraker printers said about files just uploaded (the slicer their metadata
    /// names), from the upload threads; see `App::check_file_identity`.
    pub(crate) identity_reports: (
        std::sync::mpsc::Sender<crate::printhost::job::SlicerReport>,
        std::sync::mpsc::Receiver<crate::printhost::job::SlicerReport>,
    ),
    /// The Library view (fourth header tab) and the model library behind it.
    pub(crate) library: crate::library::LibraryView,
    /// The macOS system menu bar (`menu::native`); `None` draws the menus in the header.
    pub(crate) native_menu: Option<crate::menu::native::NativeMenu>,
    /// Check for Updates and the daily check (`updater.rs`); off in headless runs.
    pub(crate) updater: crate::updater::Updater,
    /// An opened G-code file shown read-only in Preview over the scene (`gcode_doc`).
    pub(crate) gcode: Option<crate::gcode_doc::Doc>,
    /// A G-code file being opened (on its thread).
    pub(crate) gcode_loading: Option<crate::gcode_doc::Loading>,
    /// The project's mixed colors and their editor (`mixes`).
    pub(crate) mixes: crate::mixes::MixState,
    /// Mica MCP live mode: the local socket AI assistants reach this window on (`mcp_live`);
    /// off in headless runs.
    pub(crate) mcp: crate::mcp_live::Host,
}

impl App {
    /// The real window. `launch`: the Welcome / What's new windows asked for on the command line.
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        files: Vec<PathBuf>,
        launch: crate::panels::welcome::Launch,
    ) -> Self {
        let prefs_existed = Prefs::default_path().exists();
        let state = cc.wgpu_render_state.as_ref().expect("wgpu renderer");
        state
            .renderer
            .write()
            .callback_resources
            .insert(render::Renderer::new(&state.device, state.target_format));
        let mut app = Self::create(
            &cc.egui_ctx,
            Options {
                prefs_path: Some(Prefs::default_path()),
                store: None,
                files,
                headless: false,
            },
        );
        let weak = Arc::downgrade(&app.memory);
        let ctx = cc.egui_ctx.clone();
        std::thread::spawn(move || {
            while let Some(m) = weak.upgrade() {
                if let Some(mb) = worker::memory_mb() {
                    m.store((mb * 10.) as u64, Ordering::Relaxed);
                }
                drop(m);
                ctx.request_repaint();
                std::thread::sleep(Duration::from_secs(2));
            }
        });
        app.status = "Open an STL or 3MF to begin".into();
        // Mica MCP live mode, unless Settings › Features switched Mica MCP off (read at
        // start, like `mica mcp`'s own check).
        if mica_profiles::features::mcp_enabled(&app.prefs.features) {
            match crate::mcp_live::Host::start(&mica_cli::mcp::live::socket_path(), &cc.egui_ctx) {
                Ok(host) => app.mcp = host,
                Err(e) => app.notices.push(format!("Mica MCP live mode is off: {e}")),
            }
        }
        app.init_native_menu(&cc.egui_ctx);
        app.startup_update_check(&cc.egui_ctx);
        let dev = mica_update::is_dev_build(&std::env::current_exe().unwrap_or_default());
        app.launch_windows(prefs_existed, dev, launch.welcome, launch.whats_new);
        app
    }

    /// Builds the app without touching a window. Shared by the real window and screenshots.
    pub fn create(ctx: &egui::Context, options: Options) -> Self {
        let (mut prefs, prefs_warning) = match &options.prefs_path {
            Some(p) => Prefs::load(p),
            None => (Prefs::default(), None),
        };
        let store = options
            .store
            .unwrap_or_else(|| Store::open(Store::default_root()));
        let mut profiles = Profiles::discover_with_store(store);
        let mut notices = profiles.warnings.clone();
        if let Some(w) = prefs_warning {
            notices.push(w);
        }
        // Physical printers live next to the preferences; headless runs keep them (and their
        // secrets) in memory only.
        let printers = match &options.prefs_path {
            Some(_) => crate::printhost::Printers::load(
                Some(crate::printhost::config::default_path()),
                Arc::from(crate::printhost::keychain::system()),
            ),
            None => crate::printhost::Printers::load(
                None,
                Arc::new(crate::printhost::keychain::MemoryStore::default()),
            ),
        };
        if let Some(w) = &printers.warning {
            notices.push(w.clone());
        }
        let (recipes, recipes_warning) = crate::intents::Recipes::load(
            options
                .prefs_path
                .as_deref()
                .map(crate::intents::Recipes::path_for),
        );
        notices.extend(recipes_warning);
        // Orca vendors beyond Snapmaker load lazily; the ones remembered printers name load
        // now so their selection restores.
        let index_cache = options
            .prefs_path
            .as_deref()
            .and_then(|p| Some(p.parent()?.join("cache/printer-gallery.json")));
        let warned = profiles.warnings.len();
        {
            let linked: Vec<String> = printers
                .list
                .iter()
                .filter_map(|p| p.profile.as_deref()?.split_once('/'))
                .map(|(v, _)| v.to_string())
                .collect();
            let keys: Vec<&str> = prefs
                .printers()
                .map(|(v, _)| v)
                .chain(std::iter::once(prefs.selection.vendor.as_str()))
                .chain(prefs.defaults.printer.split_once('/').map(|(v, _)| v))
                .chain(linked.iter().map(String::as_str))
                .collect();
            profiles.load_referenced(keys);
        }
        notices.extend(profiles.warnings[warned..].iter().cloned());
        // Mica used to load OrcaSlicer's and Snapmaker Orca's presets directly: the ones the
        // preferences still name become Mica copies, so the selection keeps working.
        let kept = if prefs.detected {
            profiles.import_in_use(&mut prefs)
        } else {
            Default::default()
        };
        notices.extend(kept.errors.iter().cloned());
        if prefs.detected {
            profiles.restore(&prefs);
        } else {
            profiles.detect(&mut prefs);
        }
        // Settings › Slicing › Printer: a launch starts on that printer (with the
        // default print and filament profiles) instead of the last one used.
        if let Some((vendor, name)) = prefs.defaults.printer.split_once('/')
            && let Some((vi, _, name)) = profiles
                .all_printers()
                .into_iter()
                .find(|(_, v, n)| v == vendor && n == name)
            && (profiles.vendor_name() != vendor || profiles.printer != name)
        {
            profiles.select_printer(vi, &name);
            profiles.reset_to_defaults(&prefs.defaults.print, &prefs.defaults.filament);
        }
        let config = profiles.config();
        // Only the real window opens the library in the app's data folder; headless runs
        // (screenshots, the monkey test) open a temporary one when they need it.
        let library = crate::library::LibraryView::new(
            (!options.headless && options.prefs_path.is_some())
                .then(mica_library::default_data_dir),
        );
        let bed = {
            let c = config
                .as_ref()
                .cloned()
                .unwrap_or_else(|_| Config::defaults());
            let mut bed = Bed::from_config(&c);
            bed.assets = profiles.bed_assets(&c);
            bed
        };
        // Dark unless the user explicitly chose light or to follow the system (a first run
        // and the old `system` value both resolve to dark).
        let mode = theme::Choice::from_name(&prefs.theme).resolve(ctx.system_theme());
        theme::apply(ctx, mode);
        let plates = Plates::new(bed.max - bed.min);
        let mut viewport = Viewport::new(Camera::looking_at(
            bed.center(),
            (bed.max - bed.min).max_element() * 1.25,
        ));
        viewport.theme_mode = mode;
        viewport.camera.perspective = prefs.perspective;
        viewport.zoom_to_pointer = prefs.zoom_to_pointer;
        viewport.reverse_zoom = prefs.reverse_zoom;
        viewport.toolbar_size = prefs.toolbar_size;
        viewport.fit_plate(&bed, &plates::snapshot(&plates), false, 1.5);
        let mut app = Self {
            overlays: Overlays::new(profiles.nozzle_count()),
            profiles,
            prefs_dirty: options.prefs_path.is_some(),
            prefs_path: options.prefs_path,
            prefs,
            config,
            bed,
            plates,
            selected: None,
            multi: Default::default(),
            sub: None,
            recipes,
            tower_selected: false,
            towers: vec![],
            tower_cache: Default::default(),
            config_generation: 0,
            bed_configs: Default::default(),
            slice_cache: Default::default(),
            job_inputs: None,
            restored: vec![],
            reslice_requested: false,
            clipboard: vec![],
            renaming: None,
            cut: None,
            mesh_tool: None,
            emboss: None,
            viewport,
            worker: None,
            stopping: None,
            pending_slice: None,
            slice_requests: 0,
            finished_request: None,
            slice_queue: vec![],
            slice_batch: None,
            export_all: None,
            manual_slice: false,
            loading: None,
            preview: false,
            view: View::default(),
            status: String::new(),
            progress: 0.,
            progress_stage: String::new(),
            progress_fraction: 0.,
            memory: Arc::new(AtomicU64::new(0)),
            undo: Undo::default(),
            editor: Editor::default(),
            dialogs: Dialogs::default(),
            import_ui: Default::default(),
            dirty_since: None,
            keep_view: None,
            warnings: vec![],
            clearance: vec![],
            guards: vec![],
            notices,
            headless: options.headless,
            viewport_rect: egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000., 700.)),
            prefs_saved_at: None,
            dismissed_guards: vec![],
            last_change: None,
            overlay_cache: std::collections::HashMap::new(),
            paint: crate::paint::Settings::default(),
            keys: crate::keymap::KeyState::default(),
            paint_session: None,
            overhang_overlay: None,
            fill_preview: None,
            layer_edit: Default::default(),
            layer_overlay: None,
            layer_graph_z: None,
            brim_ear: Default::default(),
            ears_loose: None,
            status_at: Instant::now(),
            last_status: String::new(),
            project: None,
            project_differences: Vec::new(),
            show_paint: scene::paint::Show::COLOUR_ONLY,
            document: Default::default(),
            closing: false,
            last_title: String::new(),
            native_menu: None,
            printers,
            machine: false,
            machine_printer: None,
            machine_view: Default::default(),
            print_job: None,
            printers_editor: Default::default(),
            // The real window searches the network; headless runs (tests, screenshots) only
            // a loopback fake responder they name themselves.
            add_printer: crate::panels::add_printer::AddPrinter::new(
                (!options.headless).then_some(mica_printhost::discovery::MDNS_GROUP),
            )
            .with_index_cache(index_cache),
            preview_side: Default::default(),
            notifications: Default::default(),
            adhesion: Vec::new(),
            plate_alerts: Default::default(),
            failed_slices: Vec::new(),
            adhesion_cache: Default::default(),
            sync_pending: None,
            filament_ignored: Default::default(),
            applied_identity: None,
            identity_reports: std::sync::mpsc::channel(),
            library,
            updater: if options.headless {
                crate::updater::Updater::default()
            } else {
                crate::updater::Updater::for_this_build()
            },
            gcode: None,
            gcode_loading: None,
            mixes: Default::default(),
            mcp: Default::default(),
        };
        app.refresh_warnings();
        if !kept.names.is_empty() {
            let names: Vec<&str> = kept.names.iter().map(|(_, n)| n.as_str()).collect();
            app.notify_info(format!(
                "Kept {} profile{} you were using from other slicers as Mica copies: {}. Settings \
                 › Profiles manages them.",
                names.len(),
                if names.len() == 1 { "" } else { "s" },
                names.join(", ")
            ));
        }
        app.offer_imports();
        // A fresh window has nothing to save; models opened from the command line are an
        // import (unsaved), a project becomes the document.
        app.mark_clean();
        if !options.files.is_empty() {
            if app.headless {
                let (gcode, models): (Vec<PathBuf>, Vec<PathBuf>) = options
                    .files
                    .into_iter()
                    .partition(|p| crate::gcode_doc::is_gcode_path(p));
                if !models.is_empty() {
                    let result = load_files(models, &app.config, app.bed.center());
                    app.finish_load(result);
                }
                if let Some(path) = gcode.first()
                    && let Err(e) = app.open_gcode_now(path)
                {
                    app.notify_error("The G-code file could not be opened", e);
                }
            } else {
                app.load(options.files, ctx);
            }
        }
        app
    }

    pub(crate) fn busy(&self) -> bool {
        self.worker.is_some()
            || self.stopping.is_some()
            || self.loading.is_some()
            || self.gcode_loading.is_some()
    }

    /// Cancels the running job and keeps it until it confirms it stopped, so no second job
    /// starts while it winds down (the engine checks the flag per layer, so this is quick).
    pub(crate) fn stop_worker(&mut self) {
        if let Some((_, w)) = self.worker.take() {
            w.cancel.store(true, Ordering::Relaxed);
            self.stopping = Some(w);
        }
        self.progress = 0.;
        self.progress_fraction = 0.;
    }

    pub(crate) fn effective_config(&self) -> Option<&Config> {
        self.config.as_ref().ok()
    }

    pub(crate) fn tool_count(&self) -> usize {
        self.profiles.nozzle_count()
    }

    /// Distinct tools plate `i`'s objects print walls and infill with, and the lowest of them
    /// (upstream `Print::object_extruders`, which decides the custom G-code mode).
    pub(crate) fn plate_object_tools(&self, i: usize) -> (usize, usize) {
        let last = self.tool_count().max(1) - 1;
        let tools: std::collections::BTreeSet<usize> = self
            .plates
            .plate(i)
            .objects
            .iter()
            .flat_map(|o| o.tools.0[..3].iter().flatten())
            .flat_map(|t| match self.mixes.list.iter().find(|m| m.id == t + 1) {
                // A mixed color prints with its components (with ColorMix off: its main one).
                Some(m) if !self.prefs.features.color_mix => vec![m.main_extruder() - 1],
                Some(m) => m.components.iter().map(|c| c.extruder - 1).collect(),
                None => vec![*t],
            })
            .map(|t| t.min(last))
            .collect();
        (tools.len().max(1), tools.first().copied().unwrap_or(0))
    }

    /// Applies a layer slider edit to the current plate's custom G-code as one undo step. The
    /// slice is outdated: it re-slices at once and stays in the preview, on the same layer.
    pub(crate) fn edit_layer_gcode(
        &mut self,
        edit: crate::preview::layer_gcode::Edit,
        ctx: &egui::Context,
    ) {
        use crate::preview::layer_gcode::Edit;
        if self.plates.current().locked {
            self.status = "This plate is locked; unlock it from its plate label".into();
            return;
        }
        let label = match &edit {
            Edit::Put { label, .. } => *label,
            Edit::Remove { .. } => "Remove layer G-code",
        };
        let mut info = self.plates.current().custom_gcode.clone();
        match edit {
            Edit::Put { item, mode, .. } => {
                info.mode = mode;
                info.put(item);
            }
            Edit::Remove { print_z } => {
                if let Some(i) = info.at(print_z) {
                    info.items.remove(i);
                }
            }
        }
        if info == self.plates.current().custom_gcode {
            return;
        }
        self.record(label);
        self.plates.current_mut().custom_gcode = info;
        self.layer_gcode_changed(ctx);
        self.status = format!("{label} · slicing again");
    }

    /// The current plate's custom G-code changed: its G-code is outdated. Unlike other edits
    /// the toolpaths stay, so a preview re-slices right away and keeps its layer range.
    pub(crate) fn layer_gcode_changed(&mut self, ctx: &egui::Context) {
        let current = self.plates.current().id;
        self.dirty_since = None;
        if self.preview && self.finished().is_some() {
            self.keep_view = Some(self.view.clone());
            let i = self.plates.current;
            self.slice_plate(i, true, ctx);
        } else {
            self.plates.current_mut().invalidate();
            if self.worker.as_ref().is_some_and(|(id, _)| *id == current) {
                self.stop_worker();
                self.slice_queue.clear();
            }
            self.dirty_since = Some(Instant::now());
        }
    }

    // ----- undo -----------------------------------------------------------------------------

    /// The plates as undo and the unsaved-changes check record them.
    pub(crate) fn plate_states(&self) -> Vec<PlateState> {
        self.plates
            .plates
            .iter()
            .map(|p| PlateState {
                name: p.name.clone(),
                objects: p.objects.clone(),
                overrides: p.overrides.clone(),
                locked: p.locked,
                metadata: p.metadata.clone(),
                tower: p.tower_placement,
                calibration: p.calibration.clone(),
                custom_gcode: p.custom_gcode.clone(),
            })
            .collect()
    }

    pub(crate) fn snapshot(&self) -> Snapshot {
        Snapshot {
            plates: self.plate_states(),
            current: self.plates.current,
            selected: self.selected,
            multi: self.multi.clone(),
            sub: self.sub,
            tower_selected: self.tower_selected,
            overlays: self.overlays.clone(),
            mixes: self.mixes.list.clone(),
        }
    }

    fn restore(&mut self, s: Snapshot) {
        let overlays_changed = s.overlays != self.overlays;
        // Colours alone do not change toolpaths: slices and a running job stay. Mixed colors do.
        let settings_changed =
            !s.overlays.same_but_colours(&self.overlays) || s.mixes != self.mixes.list;
        if s.mixes != self.mixes.list {
            self.mixes.list = s.mixes.clone();
            self.mixes.draft = None;
        }
        let states = self.plate_states();
        let current_id = self.plates.current().id;
        let mut old: Vec<Option<crate::plates::Plate>> = std::mem::take(&mut self.plates.plates)
            .into_iter()
            .map(Some)
            .collect();
        self.plates.plates = s
            .plates
            .into_iter()
            .enumerate()
            .map(|(i, p)| {
                // A plate the step did not touch stays as it is, with its slice and job, also
                // when the step moved it (reordered plates).
                let same = |j: usize| {
                    states.get(j).is_some_and(|st| *st == p)
                        && old.get(j).is_some_and(Option::is_some)
                };
                let found = if same(i) {
                    Some(i)
                } else {
                    (0..states.len()).find(|&j| same(j))
                };
                if let Some(mut plate) = found.and_then(|j| old.get_mut(j).and_then(Option::take)) {
                    if settings_changed {
                        plate.invalidate();
                    }
                    return plate;
                }
                let mut plate = crate::plates::Plate::new(p.name);
                plate.objects = p.objects;
                plate.overrides = p.overrides;
                plate.locked = p.locked;
                plate.metadata = p.metadata;
                plate.tower_placement = p.tower;
                plate.calibration = p.calibration;
                plate.custom_gcode = p.custom_gcode;
                plate
            })
            .collect();
        self.plates.current = s.current.min(self.plates.len().saturating_sub(1));
        let len = self.plates.current().objects.len();
        self.selected = s.selected.filter(|&i| i < len);
        self.multi = s.multi.into_iter().filter(|&i| i < len).collect();
        self.sub = s.sub.filter(|sub| {
            self.selected
                .and_then(|i| self.plates.current().objects.get(i))
                .is_some_and(|o| match sub {
                    scene::Sub::Volume(id) => o.volume(*id).is_some(),
                    scene::Sub::Range(id) => o.range(*id).is_some(),
                })
        });
        self.overlays = s.overlays;
        self.tower_selected = s.tower_selected;
        if overlays_changed {
            // A restored printer override may change the bed, which drops every plate's tower
            // placement (they belong to a bed); the snapshot's placements are the ones to keep
            // (GUI monkey test, seed 1002: undo all + redo all lost both plates' towers).
            let towers: Vec<_> = self
                .plates
                .plates
                .iter()
                .map(|p| p.tower_placement)
                .collect();
            self.recompute_config();
            for (p, t) in self.plates.plates.iter_mut().zip(towers) {
                p.tower_placement = t;
            }
        } else {
            // Plate overrides may name another bed type.
            self.bed_configs.borrow_mut().clear();
        }
        if settings_changed {
            // A running job slices with the settings being undone.
            self.stop_worker();
            self.slice_queue.clear();
        }
        // Replaced plates are new (`Plate::new`): a job for one of them is cancelled.
        self.drop_orphan_jobs();
        // A current plate the step did not touch keeps its slice (and its job).
        let kept = self.plates.current().id == current_id;
        self.after_change(!kept);
    }

    /// Records the state before a discrete change, as one undo step.
    pub(crate) fn record(&mut self, label: &str) {
        let s = self.snapshot();
        self.undo.push(label, s);
        self.trim_undo();
    }

    /// Bounds what the undo history keeps alive (`undo::MEMORY_LIMIT`, as PrusaSlicer does
    /// after every snapshot): the oldest entries go when meshes or paintings that only they
    /// still share exceed it.
    fn trim_undo(&mut self) {
        let current = self.snapshot();
        self.undo
            .trim_memory(&current, crate::state::undo::MEMORY_LIMIT);
    }

    /// Starts a continuous change (a drag or a typed edit): the state is captured now and
    /// becomes one undo step when the gesture ends, if anything changed. Calling it again
    /// during the same gesture is a no-op, so every frame of a drag may call it.
    pub(crate) fn begin_gesture(&mut self, label: &str) {
        if !self.undo.in_gesture() {
            let s = self.snapshot();
            self.undo.begin(label, s);
        }
    }

    /// Ends the gesture in progress (one entry, or none when nothing changed).
    pub(crate) fn end_gesture(&mut self) {
        if self.undo.in_gesture() {
            let s = self.snapshot();
            if self.undo.end(&s) {
                self.trim_undo();
            }
        }
    }

    /// Abandons the gesture in progress and puts the scene back as it was when it began.
    pub(crate) fn cancel_gesture(&mut self) {
        if let Some(s) = self.undo.cancel() {
            self.restore(s);
            self.status = "Canceled".into();
        }
    }

    pub(crate) fn undo(&mut self) {
        let current = self.snapshot();
        if let Some(s) = self.undo.undo(current) {
            self.restore(s);
            self.status = "Undone".into();
        }
    }

    pub(crate) fn redo(&mut self) {
        let current = self.snapshot();
        if let Some(s) = self.undo.redo(current) {
            self.restore(s);
            self.status = "Redone".into();
        }
    }

    // ----- change tracking ------------------------------------------------------------------

    /// The current plate's geometry or settings changed: drop its slice, schedule a re-slice.
    ///
    /// A plate whose inputs are back to a state it was sliced in (an undo, a redo, an edit
    /// put back) gets that slice from the cache at once (`reconcile_slices`).
    pub(crate) fn changed(&mut self) {
        self.after_change(true);
    }

    /// `changed`, optionally leaving the current plate's slice and job alone (an undo step
    /// that did not touch that plate).
    fn after_change(&mut self, invalidate_current: bool) {
        let current = self.plates.current().id;
        self.dismissed_guards.clear();
        let shown = self.finished().map(|f| Arc::as_ptr(&f.preview));
        if invalidate_current {
            self.plates.current_mut().invalidate();
            if self.worker.as_ref().is_some_and(|(id, _)| *id == current) {
                self.stop_worker();
                self.slice_queue.clear();
            }
        }
        self.reconcile_slices();
        self.dirty_since = Some(Instant::now());
        self.validate_selection();
        self.refresh_warnings();
        match self.finished().map(|f| Arc::as_ptr(&f.preview)) {
            Some(now) => {
                if Some(now) != shown {
                    // Another slice than the one shown: its own layer range.
                    if let Some(f) = self.finished() {
                        self.view = Self::view_for(f);
                    }
                    self.status = "Slice restored: this state was sliced before".into();
                }
            }
            None => {
                self.preview = false;
                self.status = if self.plates.current().objects.is_empty() {
                    "Open an STL or 3MF to begin".into()
                } else {
                    "Ready to slice".into()
                };
            }
        }
    }

    /// Gives every plate without a slice the cached slice of its current inputs, if there
    /// is one (exact, or differing only in colours), then trims the cache.
    pub(crate) fn reconcile_slices(&mut self) {
        for i in 0..self.plates.len() {
            let plate = self.plates.plate(i);
            if plate.finished.is_some()
                || plate.slice_error.is_some()
                || plate.objects.is_empty() && plate.calibration.is_none()
                || self.worker.as_ref().is_some_and(|(id, _)| *id == plate.id)
            {
                continue;
            }
            let job = self.slice_job(i);
            if !self.slice_cache.has_job(&job) {
                continue;
            }
            let Ok(config) = self.job_config(i) else {
                continue;
            };
            let inputs = crate::slice_cache::Inputs::new(job, config);
            if let Some((key, finished, _)) = self.slice_cache.find(&inputs) {
                let plate = &mut self.plates.plates[i];
                plate.finished = Some(finished);
                plate.sliced = Some(key);
                self.restored.push(plate.id);
                if self.restored.len() > 64 {
                    self.restored.remove(0);
                }
            }
        }
        self.slice_cache.trim();
    }

    /// What plate `i`'s slice job prints.
    pub(crate) fn slice_job(&self, i: usize) -> crate::slice_cache::Job {
        let plate = self.plates.plate(i);
        match &plate.calibration {
            // The test prints through the chosen tool.
            Some(job) => crate::slice_cache::Job::Calibration(mica_core::calib::Parameters {
                tool: job.tool,
                ..job.params.clone()
            }),
            // Features turned off in Settings › Features: the objects slice without their
            // texture settings and paint, and every mixed color in its main color.
            None => {
                let mut objects = self.with_mixes(plate.objects.clone());
                if !self.prefs.features.color_mix {
                    objects = crate::mixes::main_colors_only(objects);
                }
                if !self.prefs.features.textures {
                    objects = crate::textures::without_textures(objects);
                }
                if !self.prefs.features.brick_layers_enabled() {
                    objects = crate::brick_layers::without_brick_layers(objects);
                }
                crate::slice_cache::Job::Objects(objects, plate.custom_gcode.clone())
            }
        }
    }

    /// Everything that determines plate `i`'s G-code now.
    pub(crate) fn slice_inputs(&self, i: usize) -> Result<crate::slice_cache::Inputs, String> {
        Ok(crate::slice_cache::Inputs::new(
            self.slice_job(i),
            self.job_config(i)?,
        ))
    }

    /// How plate `i`'s slice fits its current inputs: exactly, or only the colours lag;
    /// `None` without a (fitting) slice.
    pub(crate) fn slice_fit(&self, i: usize) -> Option<crate::slice_cache::Fit> {
        let plate = self.plates.plate(i);
        plate.finished.as_ref()?;
        plate.sliced.as_ref()?.fit(&self.slice_inputs(i).ok()?)
    }

    /// Only colours changed (a picked colour, a colour edit): the configuration follows, the
    /// slices stay (the preview takes its tool colours from the app every frame); their
    /// G-code carries the old colours until Export or Print slices again
    /// (`reslice_for_colours`), unless a slice with these colours is cached.
    pub(crate) fn colours_changed(&mut self) {
        self.recompute_config();
        self.prefs_dirty = true;
        self.reconcile_slices();
    }

    /// Before exporting or printing: a slice whose colours lag gets the cached slice with the
    /// current colours, else it is sliced again (the next frame) so the file carries the
    /// colours shown. Returns true when the export or print has to wait for that.
    pub(crate) fn reslice_for_colours(&mut self) -> bool {
        let i = self.plates.current;
        if self.slice_fit(i) != Some(crate::slice_cache::Fit::Colours) {
            return false;
        }
        if let Ok(inputs) = self.slice_inputs(i)
            && let Some((key, finished, crate::slice_cache::Fit::Exact)) =
                self.slice_cache.find(&inputs)
        {
            let plate = self.plates.current_mut();
            plate.finished = Some(finished);
            plate.sliced = Some(key);
            return false;
        }
        self.plates.current_mut().invalidate();
        self.reslice_requested = true;
        self.status = "Colors changed since the slice: slicing again so the G-code has them".into();
        true
    }

    /// Makes `name` of vendor `vendor` the current printer. The printer edits and the
    /// project's values go with the old printer (`Overlays::printer_switched`): a bed shape
    /// edited on the XL never reaches the CORE One. Every way of choosing a printer goes
    /// through here.
    pub(crate) fn switch_printer(&mut self, vendor: usize, name: &str) {
        self.profiles.select_printer(vendor, name);
        self.overlays.printer_switched();
        self.fit_mixes_to_printer();
        self.settings_changed();
    }

    /// A global setting or preset changed: every plate's slice is stale.
    pub(crate) fn settings_changed(&mut self) {
        self.recompute_config();
        self.plates.invalidate_all();
        self.stop_worker();
        self.slice_queue.clear();
        self.prefs_dirty = true;
        self.changed();
    }

    pub(crate) fn recompute_config(&mut self) {
        self.config_generation += 1;
        self.bed_configs.borrow_mut().clear();
        self.overlays.normalize();
        self.overlays.resize(self.profiles.nozzle_count());
        let identity = self.printer_gcode_identity();
        self.applied_identity = identity;
        let differences = &mut self.project_differences;
        let project = &self.overlays.project;
        let mut invalid = Vec::new();
        let prusa_profile = !self.profiles.is_orca();
        self.config = self.profiles.config().map(|mut c| {
            // What the project's values differ in from the current presets (the card's list).
            *differences = project.differences(&c);
            invalid = compose(&mut c, &self.overlays, prusa_profile, identity);
            c
        });
        if !invalid.is_empty() {
            self.notify_warning(format!("Invalid value for {}", invalid.join(", ")));
        }
        if let Ok(c) = &self.config {
            let mut new_bed = Bed::from_config(c);
            new_bed.assets = self.profiles.bed_assets(c);
            if new_bed != self.bed {
                self.bed = new_bed;
                self.plates.set_bed(&self.bed);
                self.viewport
                    .fit_plate(&self.bed, &plates::snapshot(&self.plates), false, 1.5);
                for p in &mut self.plates.plates {
                    p.tower_placement = None;
                }
            }
        }
        // Objects keep their tools even when the selected printer has fewer (a project for
        // another printer, whose settings may be used next); the worker clamps when slicing.
        self.profiles.store_selection(&mut self.prefs);
    }

    pub(crate) fn refresh_warnings(&mut self) {
        self.refresh_towers();
        self.warnings = match &self.config {
            Ok(c) => {
                let plate = self.plates.current();
                if plate.objects.is_empty() {
                    vec![]
                } else {
                    worker::plate_warnings(&plate.objects, &self.bed, c)
                }
            }
            Err(e) => vec![e.clone()],
        };
        self.warnings.extend(self.tower_warnings());
        // Skirts and brims that would not fit on the bed (the print builder refuses them).
        self.adhesion = self.adhesion_overruns(self.plates.current);
        for (i, o) in &self.adhesion {
            let name = &self.plates.current().objects[*i].name;
            self.warnings
                .push(mica_core::adhesion_check::explain(name, o, true));
        }
        self.plate_alerts = (0..self.plates.len())
            .filter(|&i| self.plate_has_object_warnings(i))
            .map(|i| {
                (
                    self.plates.plate(i).id,
                    crate::notifications::Level::Warning,
                )
            })
            .collect();
        // One object after another: the clearance check and the outlines on the bed.
        // The plate's own print order ("This plate") applies.
        let check = match self.plate_config(self.plates.current) {
            Ok(c) => worker::sequential_check(&self.plates.current().objects, &c),
            Err(_) => worker::SequentialCheck {
                report: Default::default(),
                outlines: Vec::new(),
            },
        };
        self.warnings.extend(check.report.errors);
        self.warnings.extend(check.report.warnings);
        self.clearance = check.outlines;
        self.guards = match &self.config {
            Ok(c) => crate::checks::guard_rails(c, &self.plates.current().objects),
            Err(_) => vec![],
        };
    }

    /// The objects of plate `i` whose skirt or brim would not fit on the bed, with the plate's
    /// configuration (cached by object state).
    pub(crate) fn adhesion_overruns(
        &self,
        i: usize,
    ) -> Vec<(usize, mica_core::adhesion_check::Overrun)> {
        let Ok(config) = self.plate_config(i) else {
            return Vec::new();
        };
        let plate = self.plates.plate(i);
        let mut cache = self.adhesion_cache.borrow_mut();
        if cache.len() > 1024 {
            cache.clear();
        }
        plate
            .objects
            .iter()
            .enumerate()
            .filter_map(|(k, o)| {
                let key = self.adhesion_key(i, o);
                cache
                    .entry(key)
                    .or_insert_with(|| worker::adhesion_overrun(o, &config))
                    .clone()
                    .map(|overrun| (k, overrun))
            })
            .collect()
    }

    /// What the pre-slice adhesion check of object `o` on plate `i` depends on.
    fn adhesion_key(&self, i: usize, o: &Object) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.config_generation.hash(&mut h);
        self.plates.plate(i).overrides.hash(&mut h);
        (Arc::as_ptr(&o.mesh) as usize).hash(&mut h);
        o.transform().to_cols_array().map(f32::to_bits).hash(&mut h);
        o.overrides.hash(&mut h);
        o.tools.0.hash(&mut h);
        h.finish()
    }

    /// Whether plate `i` has warnings about its objects (outside the bed, overlapping, a
    /// skirt or brim past the edge): its label's marker.
    fn plate_has_object_warnings(&self, i: usize) -> bool {
        let plate = self.plates.plate(i);
        if plate.objects.is_empty() {
            return false;
        }
        if i == self.plates.current {
            if !self.adhesion.is_empty() {
                return true;
            }
        } else if !self.adhesion_overruns(i).is_empty() {
            return true;
        }
        let Ok(c) = &self.config else {
            return false;
        };
        let general = mica_core::safety::config_warnings(c).len();
        worker::plate_warnings(&plate.objects, &self.bed, c).len() > general
    }

    /// The marker of plate `i`'s label: a failed slice, else warnings about its objects.
    pub(crate) fn plate_alert(&self, i: usize) -> Option<crate::notifications::Level> {
        let plate = self.plates.plate(i);
        if plate.slice_error.is_some() {
            return Some(crate::notifications::Level::Error);
        }
        self.plate_alerts.get(&plate.id).copied()
    }

    /// Colour of each tool's filament (the project colour, else the preset colour, else the
    /// palette), for models and swatches. When every tool has the same colour the palette
    /// tells them apart instead.
    pub(crate) fn tool_colours(&self) -> Vec<[u8; 3]> {
        let mine: Vec<Option<[u8; 3]>> = (0..self.tool_count())
            .map(|t| {
                self.overlays
                    .colour(t)
                    .and_then(mica_profiles::project::parse_colour)
                    .or_else(|| {
                        self.profiles
                            .filaments
                            .get(t)
                            .and_then(|f| self.profiles.filament_colour(f))
                    })
            })
            .collect();
        let distinct = mine
            .iter()
            .flatten()
            .collect::<std::collections::BTreeSet<_>>()
            .len();
        // A colour someone chose (picked, from a project or the printer) is shown as it is,
        // even when every tool has the same one.
        let chosen = self
            .overlays
            .colours
            .iter()
            .take(self.tool_count())
            .any(Option::is_some);
        mine.into_iter()
            .enumerate()
            .map(|(t, c)| match c {
                Some(c) if chosen || distinct > 1 || self.tool_count() == 1 => c,
                _ => crate::preview::legend::TOOLS[t % 8],
            })
            .collect()
    }
    /// Tool colours for drawing models and toolpaths. Near-black filaments are lifted so their
    /// shading stays readable; swatches, saved projects and G-code keep the real colour.
    pub(crate) fn tool_colors_f32(&self) -> Vec<[f32; 3]> {
        if let Some(doc) = &self.gcode {
            return doc
                .colours
                .iter()
                .map(|c| display_colour(c.map(|v| v as f32 / 255.)))
                .collect();
        }
        self.tool_colours()
            .into_iter()
            .map(|c| display_colour(c.map(|v| v as f32 / 255.)))
            .collect()
    }
    /// Which paint kinds an object shows, and how strongly. Colour painting is always drawn
    /// (it is the model's look). The active tool's kind is drawn at full strength on the
    /// object being painted. Otherwise support and fuzzy paint show on the selected object,
    /// and any kind shows everywhere when its View toggle is on, both at half strength so
    /// the colour underneath stays readable.
    pub(crate) fn paint_show(
        &self,
        object: u64,
        selected: bool,
        active: Option<(u64, mica_core::painting::PaintKind)>,
    ) -> scene::paint::Show {
        use mica_core::painting::PaintKind;
        let mut show = scene::paint::Show::COLOUR_ONLY;
        for kind in [PaintKind::Supports, PaintKind::Seam, PaintKind::Fuzzy] {
            let toggled = self.show_paint.strength(kind) > 0.;
            let with_selection = selected && kind != PaintKind::Seam;
            if toggled || with_selection {
                show.set(kind, scene::paint::Show::SUBTLE);
            }
        }
        if let Some((id, kind)) = active
            && id == object
        {
            show.set(kind, 1.);
        }
        show
    }

    /// The paint overlays of every painted object in the scene, from the cache when the
    /// painting and the visibility rules did not change (`paint_show`).
    fn paint_overlays(
        &mut self,
        objects: &[Object],
        tool_colors: &[[f32; 3]],
    ) -> Vec<scene::paint::PaintOverlay> {
        self.overlay_cache
            .retain(|id, _| objects.iter().any(|o| o.id == *id));
        let active = self
            .paint_session
            .as_ref()
            .filter(|_| self.viewport.paint.is_some())
            .map(|s| (s.object_id, s.kind));
        let selected = self
            .selected
            .and_then(|i| self.plates.current().objects.get(i))
            .map(|o| o.id);
        let mut out = Vec::new();
        // Painted colors follow the tool colors (a mixed color's prediction changes with them).
        let colours = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            for c in tool_colors {
                c.map(f32::to_bits).hash(&mut h);
            }
            h.finish()
        };
        for o in objects {
            if o.painting.is_empty() {
                self.overlay_cache.remove(&o.id);
                continue;
            }
            let show = self.paint_show(o.id, selected == Some(o.id), active);
            let key = scene::paint::overlay_key(o.id, o.painting.fingerprint(), show, colours);
            let overlay =
                self.overlay_cache
                    .entry(o.id)
                    .or_insert_with(|| scene::paint::PaintOverlay {
                        object: o.id,
                        key: 0,
                        vertices: Arc::new(vec![]),
                    });
            if overlay.key != key {
                *overlay = scene::paint::PaintOverlay {
                    object: o.id,
                    key,
                    vertices: Arc::new(scene::paint::overlay_vertices(
                        &o.mesh,
                        &o.painting,
                        show,
                        tool_colors,
                    )),
                };
            }
            out.push(overlay.clone());
        }
        out
    }

    /// The overhang highlight for the support painting tool ("Show overhangs"): the painted
    /// object's facets steeper than the threshold, in the warning tint.
    fn overhang_overlay(&mut self, objects: &[Object]) -> Option<scene::paint::PaintOverlay> {
        let session = self.paint_session.as_ref()?;
        if self.viewport.paint != Some(mica_core::painting::PaintKind::Supports)
            || !self.paint.show_overhangs
        {
            self.overhang_overlay = None;
            return None;
        }
        let o = objects.iter().find(|o| o.id == session.object_id)?;
        let key = scene::paint::overlay_key(
            o.id,
            self.paint.overhang_angle.to_bits().into(),
            scene::paint::Show::ALL,
            o.transform()
                .to_cols_array()
                .iter()
                .fold(0u64, |h, v| h.rotate_left(5) ^ u64::from(v.to_bits())),
        );
        if self.overhang_overlay.as_ref().is_none_or(|v| v.key != key) {
            let facets = session.overhang_facets(&self.paint);
            self.overhang_overlay = Some(scene::paint::PaintOverlay {
                object: o.id,
                key,
                vertices: Arc::new(scene::paint::facet_vertices(
                    &o.mesh,
                    &facets,
                    scene::paint::OVERHANG,
                )),
            });
        }
        self.overhang_overlay.clone()
    }

    /// The Fill brush's hover preview: what a click at the pointer would paint (the fill
    /// within its angle and max spread), hatched in the brush's colour over the object. Without
    /// a spread limit one preview serves every facet of its region, so it is rebuilt only when
    /// the pointer enters another region; with one, when the pointer moves.
    fn fill_preview_overlay(
        &mut self,
        objects: &[Object],
        tool_colors: &[[f32; 3]],
    ) -> Option<scene::paint::PaintOverlay> {
        use std::hash::{Hash, Hasher};
        let found = self
            .viewport
            .paint
            .filter(|_| self.paint.brush == crate::paint::BrushKind::Fill)
            .zip(self.viewport.paint_hover_hit())
            .and_then(|(kind, hit)| {
                let s = self.paint_session.as_ref().filter(|s| s.kind == kind)?;
                let o = objects.iter().find(|o| o.id == s.object_id)?;
                Some((kind, hit, o))
            });
        let Some((kind, hit, o)) = found else {
            self.fill_preview = None;
            return None;
        };
        let settings = &self.paint;
        let limited = settings.fill_spread > 0.;
        let mut h = std::collections::hash_map::DefaultHasher::new();
        o.id.hash(&mut h);
        for v in o.transform().to_cols_array() {
            v.to_bits().hash(&mut h);
        }
        (kind as u8).hash(&mut h);
        settings.fill_angle.to_bits().hash(&mut h);
        settings.fill_spread.to_bits().hash(&mut h);
        settings
            .stroke_overhang(kind)
            .map(f32::to_bits)
            .hash(&mut h);
        settings.state_for(kind, false).hash(&mut h);
        for c in tool_colors {
            c.map(f32::to_bits).hash(&mut h);
        }
        if limited {
            hit.face.hash(&mut h);
            (hit.point * 20.)
                .round()
                .to_array()
                .map(|v| v as i64)
                .hash(&mut h);
        }
        let key = h.finish();
        if let Some(p) = &self.fill_preview
            && p.key == key
            && (limited || p.region.binary_search(&hit.face).is_ok())
        {
            return Some(p.overlay.clone());
        }
        let session = self.paint_session.as_ref()?;
        let preview = crate::paint::FillPreview::new(session, settings, hit, key, tool_colors);
        let overlay = preview.overlay.clone();
        self.fill_preview = Some(preview);
        Some(overlay)
    }

    /// The painting session for the selected object and the active tool, (re)built when it
    /// does not mirror the object any more (another object, an undo, a move).
    pub(crate) fn paint_session_for(&mut self, kind: mica_core::painting::PaintKind) -> bool {
        let Some(i) = self.selected else {
            self.paint_session = None;
            return false;
        };
        let Some(o) = self.plates.current().objects.get(i) else {
            self.paint_session = None;
            return false;
        };
        if self
            .paint_session
            .as_ref()
            .is_none_or(|s| !s.matches(o, kind))
        {
            self.paint_session = Some(crate::paint::Session::new(o, kind));
        }
        true
    }

    /// One stroke frame of the active painting tool on the selected object.
    fn paint_stroke(&mut self, path: &[crate::paint::Hit], erase: bool) {
        let Some(kind) = self.viewport.paint else {
            return;
        };
        if !self.paint_session_for(kind) {
            return;
        }
        let Some(i) = self.selected else { return };
        let settings = self.paint.clone();
        let mut session = self.paint_session.take().unwrap();
        if session.stroke_path(&settings, path, erase) {
            self.begin_gesture(crate::paint::tool_name(kind));
            session.store(&mut self.plates.current_mut().objects[i]);
            self.changed();
        }
        self.paint_session = Some(session);
    }

    /// "Paint overhangs as enforcers" on the selected object: one undo step.
    pub(crate) fn paint_overhangs(&mut self) {
        if !self.paint_session_for(mica_core::painting::PaintKind::Supports) {
            return;
        }
        let Some(i) = self.selected else { return };
        let settings = self.paint.clone();
        let mut session = self.paint_session.take().unwrap();
        let count = session.paint_overhangs(&settings);
        if count > 0 {
            self.record("Paint overhangs");
            session.store(&mut self.plates.current_mut().objects[i]);
            self.changed();
        }
        self.status = format!(
            "{count} overhang facet{} painted as support enforcers",
            if count == 1 { "" } else { "s" }
        );
        self.paint_session = Some(session);
    }

    /// "Clear all" of the active tool's painting on the selected object: one undo step.
    pub(crate) fn clear_painting(&mut self) {
        let Some(kind) = self.viewport.paint else {
            return;
        };
        if !self.paint_session_for(kind) {
            return;
        }
        let Some(i) = self.selected else { return };
        let mut session = self.paint_session.take().unwrap();
        if session.selector.has_painting() {
            self.record("Clear painting");
            session.clear();
            session.store(&mut self.plates.current_mut().objects[i]);
            self.changed();
        }
        self.paint_session = Some(session);
    }

    /// The preview view for a fresh slice: coloured by tool when more than one tool printed
    /// (multi-tool or colour-painted jobs), by feature otherwise.
    pub(crate) fn view_for(f: &worker::Finished) -> View {
        View {
            high: f.preview.layers.len().saturating_sub(1),
            color: if f
                .preview
                .color_print
                .as_ref()
                .is_some_and(|c| c.segments.len() > 1)
            {
                crate::preview::ColorMode::ColorPrint
            } else if f.summary.tools.len() > 1 {
                crate::preview::ColorMode::Tool
            } else {
                crate::preview::ColorMode::Feature
            },
            ..Default::default()
        }
    }

    /// Objects of every plate in world coordinates, current plate first so that scene indices
    /// below the current plate's count are its own indices. Returns the index map too.
    pub(crate) fn scene_objects(&self) -> (Vec<Object>, Vec<(usize, usize)>) {
        let mut out = Vec::new();
        let mut map = Vec::new();
        let current = self.plates.current;
        let order =
            std::iter::once(current).chain((0..self.plates.len()).filter(|&i| i != current));
        for pi in order {
            let origin = self.plates.offset(pi);
            for (oi, o) in self.plates.plate(pi).objects.iter().enumerate() {
                let mut o = o.clone();
                if let Some(preview) = self.simplify_preview(pi, &o) {
                    o.mesh = preview;
                }
                o.position += origin;
                out.push(o);
                map.push((pi, oi));
            }
        }
        (out, map)
    }
    pub(crate) fn fit_current_plate(&mut self, all: bool) {
        let aspect = self.viewport_rect.aspect_ratio().max(0.5);
        if let Some(doc) = &self.gcode {
            let bed = doc.bed.as_ref().unwrap_or(&self.bed);
            self.viewport
                .fit_plate(bed, &crate::gcode_doc::single_plate(), false, aspect);
            return;
        }
        self.viewport
            .fit_plate(&self.bed, &plates::snapshot(&self.plates), all, aspect);
    }
    pub(crate) fn set_theme(&mut self, ctx: &egui::Context, mode: theme::Mode) {
        self.set_theme_choice(
            ctx,
            match mode {
                theme::Mode::Dark => theme::Choice::Dark,
                theme::Mode::Light => theme::Choice::Light,
            },
        );
    }

    /// Dark, light or following the operating system (Settings › General).
    pub(crate) fn set_theme_choice(&mut self, ctx: &egui::Context, choice: theme::Choice) {
        self.prefs.theme = choice.name().into();
        self.prefs_dirty = true;
        let mode = choice.resolve(ctx.system_theme());
        theme::apply(ctx, mode);
        self.viewport.theme_mode = mode;
    }

    /// Per frame: follow the system's dark / light switch when the user chose that.
    fn follow_system_theme(&mut self, ctx: &egui::Context) {
        let choice = theme::Choice::from_name(&self.prefs.theme);
        if choice == theme::Choice::System {
            let mode = choice.resolve(ctx.system_theme());
            if mode != self.viewport.theme_mode {
                theme::apply(ctx, mode);
                self.viewport.theme_mode = mode;
            }
        }
    }

    /// Keeps the 3D view's options and the preferences in step: Preferences sets them, and
    /// the P key and View › Perspective camera change the projection too.
    fn sync_view_prefs(&mut self) {
        if self.viewport.camera.perspective != self.prefs.perspective {
            self.prefs.perspective = self.viewport.camera.perspective;
            self.prefs_dirty = true;
        }
        self.viewport.zoom_to_pointer = self.prefs.zoom_to_pointer;
        self.viewport.reverse_zoom = self.prefs.reverse_zoom;
        self.viewport.toolbar_size = self.prefs.toolbar_size;
    }

    /// Applies one settings edit to a target (global overlay or the selected object).
    pub(crate) fn apply_edit(&mut self, target: Option<Target>, edit: crate::panels::editor::Edit) {
        use crate::panels::editor::Edit;
        // A tool selector never names a tool the printer does not have (the option table
        // leaves them unbounded; a tool past the end would make every model fail to open).
        let edit = match edit {
            Edit::Set { key, raw } if crate::settings::is_tool_selector(&key) => {
                // An object's or part's model roles may also pick a mixed color.
                let mixes = if target.is_none()
                    && self.prefs.features.color_mix
                    && matches!(
                        key.as_str(),
                        "extruder"
                            | "perimeter_extruder"
                            | "infill_extruder"
                            | "solid_infill_extruder"
                    ) {
                    self.mixes.list.len()
                } else {
                    0
                };
                let tools = (self.tool_count() + mixes) as i64;
                let raw = match raw.trim().parse::<i64>() {
                    Ok(n) => n.clamp(0, tools).to_string(),
                    Err(_) => raw,
                };
                Edit::Set { key, raw }
            }
            edit => edit,
        };
        let label = match &edit {
            Edit::Set { key, .. } => format!("Change {}", crate::settings::label(key)),
            Edit::Reset(key) => format!("Reset {}", crate::settings::label(key)),
        };
        self.last_change = Some(match &edit {
            Edit::Set { key, raw } => change_text(key, raw),
            Edit::Reset(key) => format!("{} reset", plain_label(key)),
        });
        let edit_key = match &edit {
            Edit::Set { key, .. } | Edit::Reset(key) => key.clone(),
        };
        match target {
            Some(target) => {
                // One slider drag or one typed value is one undo step (see `begin_gesture`).
                self.begin_gesture(&label);
                let tool = match target {
                    Target::Filament(i) => i,
                    _ => self.editor.printer_extruder,
                };
                // The keys a set writes, checked against the presets afterwards.
                let mut written = Vec::new();
                // A merged setting (wall order, scarf seams) writes its PrusaSlicer
                // counterpart in the same step, so PrusaSlicer files and rules agree.
                let edit = match edit {
                    Edit::Set { key, raw } if mica_core::merged::by_orca(&key).is_some() => {
                        let edits = self
                            .effective_config()
                            .map(|c| mica_core::merged::edits_with_counterparts(c, &key, &raw));
                        for (k, v) in edits.unwrap_or_default() {
                            if k != key {
                                written.push(k.clone());
                                self.overlays.get_mut(target).insert(k, v);
                            }
                        }
                        Edit::Set { key, raw }
                    }
                    Edit::Reset(key) => {
                        if let Some(p) = mica_core::merged::by_orca(&key) {
                            self.overlays.get_mut(target).remove(p.prusa);
                        }
                        Edit::Reset(key)
                    }
                    edit => edit,
                };
                match edit {
                    Edit::Set { key, raw } => {
                        if key == crate::overlays::BED_SHAPE_KEY
                            && let Some(problem) = crate::overlays::bed_shape_problem(&raw)
                        {
                            self.notify_warning(format!(
                                "The bed shape was not changed: {problem}."
                            ));
                            return;
                        }
                        // Printer vectors are stored whole; filament overlays per tool. Only
                        // a per-extruder option takes the selected extruder's element.
                        let raw = if matches!(target, Target::Printer)
                            && mica_core::config::option_defs()
                                .get(key.as_str())
                                .is_some_and(|d| d.ty.is_vector())
                            && let Some(element) = crate::overlays::printer_element(&key, tool)
                            && let Some(c) = self.effective_config()
                        {
                            let mut c = c.clone();
                            match crate::overlays::set_tool_value(&mut c, &key, &raw, element) {
                                Ok(()) => c.raw(&key).unwrap_or(raw),
                                Err(e) => {
                                    self.notify_warning(format!("{key}: {e}"));
                                    return;
                                }
                            }
                        } else {
                            raw
                        };
                        written.push(key.clone());
                        self.overlays.get_mut(target).insert(key, raw);
                        self.drop_unchanged_edits(target, &written);
                    }
                    Edit::Reset(key) => {
                        if key == crate::overlays::COLOUR_KEY
                            && let Target::Filament(t) = target
                        {
                            self.overlays.set_colour(t, None);
                        }
                        self.overlays.get_mut(target).remove(&key);
                    }
                }
                // A filament colour is project state (`Overlays::colours`), not an edit of
                // the filament profile.
                self.overlays.normalize();
                if crate::slice_cache::COLOUR_KEYS.contains(&edit_key.as_str()) {
                    // Only how the models look: the slices stay.
                    self.colours_changed();
                    return;
                }
                self.recompute_config();
                self.plates.invalidate_all();
                self.stop_worker();
                self.changed();
            }
            None => {
                // The object's overrides, or the selected volume's / height range's.
                let Some(i) = self.selected else { return };
                self.begin_gesture(&label);
                let counterparts = match &edit {
                    Edit::Set { key, raw } if mica_core::merged::by_orca(key).is_some() => self
                        .effective_config()
                        .map(|c| mica_core::merged::edits_with_counterparts(c, key, raw))
                        .unwrap_or_default(),
                    _ => Vec::new(),
                };
                // The part the sidebar edits (the scope bar may be on the object).
                let sub = self.scope_sub();
                // What the overrides lie on: the plate's configuration for the object's own,
                // the object's for a volume's or a range's.
                let parent = matches!(edit, Edit::Set { .. })
                    .then(|| self.plate_config(self.plates.current).ok())
                    .flatten()
                    .and_then(|c| match (sub, self.plates.current().objects.get(i)) {
                        (None, _) => Some(c),
                        (Some(_), Some(o)) => crate::worker::object_config(&c, o).ok(),
                        (Some(_), None) => None,
                    });
                let plate = self.plates.current_mut();
                let Some(o) = plate.objects.get_mut(i) else {
                    return;
                };
                let Some(target) = (match sub {
                    None => Some(&mut o.overrides),
                    Some(scene::Sub::Volume(id)) => o.volume_mut(id).map(|v| &mut v.overrides),
                    Some(scene::Sub::Range(id)) => o.range_mut(id).map(|r| &mut r.overrides),
                }) else {
                    return;
                };
                match edit {
                    Edit::Set { key, raw } => {
                        // The object's merged setting writes its counterpart too.
                        let mut written = vec![key.clone()];
                        for (k, v) in counterparts {
                            written.push(k.clone());
                            target.insert(k, v);
                        }
                        target.insert(key, raw);
                        // An override back at the inherited value is no override.
                        if parent
                            .as_ref()
                            .is_some_and(|p| overrides_unchanged(p, target, &written))
                        {
                            for k in &written {
                                target.remove(k);
                            }
                        }
                    }
                    Edit::Reset(key) => {
                        if let Some(p) = mica_core::merged::by_orca(&key) {
                            target.remove(p.prusa);
                        }
                        target.remove(&key);
                    }
                }
                self.sync_instances(i);
                self.changed();
            }
        }
    }

    /// Drops the edits of `keys` (what one set wrote: a key, or a merged setting with its
    /// counterparts) from `target`'s overlay when they no longer change anything: every one
    /// is what the presets (with the project's values and the other edits) give without
    /// them, as after a reset. A value set back to the profile's is no change.
    fn drop_unchanged_edits(&mut self, target: Target, keys: &[String]) {
        // An explicit tower position edit wins over the plates' own placements
        // (`plate_config`), so it stays while any plate has one.
        let placed = self
            .plates
            .plates
            .iter()
            .any(|p| p.tower_placement.is_some());
        if keys.iter().any(|k| {
            k == crate::overlays::COLOUR_KEY
                || (placed && k.starts_with("wipe_tower_"))
                || !self.overlays.get(target).contains_key(k)
        }) {
            return;
        }
        let Ok(base) = self.profiles.config() else {
            return;
        };
        let prusa_profile = !self.profiles.is_orca();
        let identity = self.printer_gcode_identity();
        let composed = |overlays: &Overlays| {
            let mut c = base.clone();
            let invalid = compose(&mut c, overlays, prusa_profile, identity);
            (c, invalid)
        };
        let (with, invalid) = composed(&self.overlays);
        // An edit that does not parse stays, to be warned about and fixed.
        if keys.iter().any(|k| invalid.contains(k)) {
            return;
        }
        let mut without = self.overlays.clone();
        for k in keys {
            without.get_mut(target).remove(k);
        }
        if unchanged(&with, &composed(&without).0, keys) {
            self.overlays = without;
        }
    }

    /// Selects filament preset `name` for `tool`: that filament's edits go (they belong to
    /// the old preset); the colour follows `Overlays::filament_switched`.
    pub(crate) fn switch_filament(&mut self, tool: usize, name: String) {
        let has_colour = self.profiles.filament_colour(&name).is_some();
        if let Some(f) = self.profiles.filaments.get_mut(tool) {
            *f = name;
        }
        let slot = tool.min(self.overlays.filaments.len() - 1);
        self.overlays.filament_switched(slot, has_colour);
        self.profiles.refresh();
        self.settings_changed();
    }

    /// Saves the current overlay of `target` as a user profile and selects it.
    pub(crate) fn save_profile(
        &mut self,
        kind: PresetKind,
        tool: usize,
        name: &str,
    ) -> Result<(), String> {
        let (target, parent) = match kind {
            PresetKind::Print => (Target::Print, self.profiles.process.clone()),
            PresetKind::Printer => (Target::Printer, self.profiles.printer.clone()),
            PresetKind::Filament => (
                Target::Filament(tool),
                self.profiles.filaments[tool.min(self.profiles.filaments.len() - 1)].clone(),
            ),
        };
        let settings = self.overlays.get(target).clone();
        let saved = self
            .profiles
            .save_user_profile(kind, name, &parent, &settings)?;
        self.overlays.get_mut(target).clear();
        match kind {
            PresetKind::Print => self.profiles.process = saved.name,
            PresetKind::Printer => {
                let vendor = self.profiles.vendor;
                self.prefs
                    .add_printer(self.profiles.vendor_name(), &saved.name);
                self.profiles.select_printer(vendor, &saved.name);
            }
            PresetKind::Filament => {
                if let Some(f) = self.profiles.filaments.get_mut(tool) {
                    *f = saved.name;
                }
            }
        }
        self.profiles.refresh();
        self.settings_changed();
        self.status = format!("Saved profile “{name}”");
        Ok(())
    }

    // ----- files ----------------------------------------------------------------------------

    pub(crate) fn open_dialog(&mut self, ctx: &egui::Context) {
        // Headless runs (screenshots, the monkey test) have no window to parent a dialog to,
        // and a modal picker would block them forever.
        if self.headless {
            self.status = "Opening needs the file dialog; pass files in headless mode".into();
            return;
        }
        if let Some(paths) = rfd::FileDialog::new()
            .add_filter(
                "Models, projects and G-code",
                &["stl", "3mf", "gcode", "bgcode"],
            )
            .add_filter("Models", &["stl", "3mf"])
            .add_filter("G-code", &["gcode", "bgcode"])
            .pick_files()
        {
            self.load(paths, ctx);
        }
    }

    /// Opens files: a project (3MF) replaces the scene, after asking about unsaved changes;
    /// models (STL) join the current plate.
    pub(crate) fn load(&mut self, paths: Vec<PathBuf>, ctx: &egui::Context) {
        if self.loading.is_some() {
            return;
        }
        // A G-code file opens read-only over the scene (one at a time); anything else closes
        // an open one first.
        let (gcode, paths): (Vec<PathBuf>, Vec<PathBuf>) = paths
            .into_iter()
            .partition(|p| crate::gcode_doc::is_gcode_path(p));
        if let Some(first) = gcode.first() {
            if gcode.len() > 1 || !paths.is_empty() {
                self.notify_warning(format!(
                    "Opened {}; Mica shows one G-code file at a time, and opens models separately",
                    first.file_name().unwrap_or_default().to_string_lossy()
                ));
            }
            self.open_gcode(first.clone(), ctx);
            return;
        }
        if paths.is_empty() {
            return;
        }
        self.close_gcode();
        if crate::save::is_project(&paths) && self.is_modified() && self.dialogs.unsaved.is_none() {
            self.dialogs.unsaved = Some(crate::save::Pending::Open(paths));
            return;
        }
        self.load_now(paths, ctx);
    }

    /// Loads without asking (the prompt's answer, or nothing to ask).
    pub(crate) fn load_now(&mut self, paths: Vec<PathBuf>, ctx: &egui::Context) {
        if self.loading.is_some() {
            return;
        }
        let config = match &self.config {
            Ok(c) => Ok(c.clone()),
            Err(e) => Err(e.clone()),
        };
        let center = self.bed.center();
        let ctx = ctx.clone();
        let (tx, rx) = mpsc::channel();
        self.loading = Some(rx);
        self.status = "Loading geometry…".into();
        std::thread::spawn(move || {
            let _ = tx.send(load_files(paths, &config, center));
            ctx.request_repaint();
        });
    }

    pub(crate) fn finish_load(&mut self, result: LoadResult) {
        match result {
            Ok(scene) => {
                let LoadedScene {
                    plates,
                    objects,
                    warnings,
                    tower,
                    paths,
                    settings,
                    config,
                    project,
                    custom_gcode,
                    mixes,
                } = scene;
                let had_plates = !plates.is_empty();
                if !objects.is_empty() || had_plates {
                    self.record("Open");
                    if project {
                        // A project replaces the scene (as Open project does in OrcaSlicer and
                        // PrusaSlicer) and becomes the document.
                        self.reset_scene();
                        self.document.path = (paths.len() == 1).then(|| paths[0].clone());
                        self.document.flavour = config.as_ref().map(|c| {
                            if c.path.ends_with("project_settings.config") {
                                mica_core::threemf::Flavour::Orca
                            } else {
                                mica_core::threemf::Flavour::Prusa
                            }
                        });
                        self.document.original = config;
                        self.load_mixes(mixes);
                    } else if mixes.is_some() && self.mixes.list.is_empty() {
                        self.load_mixes(mixes);
                    } else if mixes.is_some() {
                        self.notify_warning(
                            "The imported model's mixed colors were not added: this project already has its own",
                        );
                    }
                    let mut plates = plates;
                    // Orca keeps the prime tower position per plate in the project settings.
                    if let Some(s) = &settings {
                        for (plate, tower) in plates.iter_mut().zip(&s.towers) {
                            if plate.tower_placement.is_none()
                                && let Some([x, y]) = tower
                            {
                                plate.tower_placement = Some([*x, *y, 0.]);
                            }
                        }
                    }
                    let base = self.plates.import(plates);
                    // Object ids stay unique per plate (a plate is one print job).
                    let mut offsets: BTreeMap<usize, usize> = BTreeMap::new();
                    let mut first: Option<(usize, usize)> = None;
                    for (plate, mut o) in objects {
                        let target = plate.map_or(self.plates.current, |p| base + p);
                        let target = target.min(self.plates.len() - 1);
                        let plate = &mut self.plates.plates[target];
                        let offset = *offsets.entry(target).or_insert_with(|| {
                            plate
                                .objects
                                .iter()
                                .map(|o| o.object_id)
                                .max()
                                .map_or(0, |n| n + 1)
                        });
                        o.object_id += offset;
                        first.get_or_insert((target, plate.objects.len()));
                        plate.objects.push(o);
                    }
                    if had_plates {
                        self.plates.current = base;
                        self.selected = None;
                    } else {
                        if tower.is_some() {
                            self.plates.current_mut().tower_placement = tower;
                        }
                        if project && !custom_gcode.is_empty() {
                            self.plates.current_mut().custom_gcode = custom_gcode;
                        }
                        self.selected = first.map(|(_, i)| i);
                    }
                    for p in paths {
                        self.prefs.push_recent(p);
                    }
                    self.prefs_dirty = true;
                    self.changed();
                    if had_plates {
                        self.fit_current_plate(false);
                    }
                }
                if let Some(settings) = settings {
                    self.offer_project_settings(settings);
                }
                if project {
                    self.mark_clean();
                }
                if !warnings.is_empty() {
                    // An export reports its file here; anything else is an import warning.
                    for w in &warnings {
                        if w.starts_with("Exported ") {
                            self.notify_info(w.clone());
                        } else {
                            self.notify_warning(w.clone());
                        }
                    }
                    self.notices
                        .extend(warnings.into_iter().filter(|w| !w.starts_with("Exported ")));
                }
            }
            Err(e) if e.starts_with("Export failed") => self.notify_error("Export failed", e),
            Err(e) => self.notify_error("Opening failed", e),
        }
    }

    /// File › New project (⌘N): asks about unsaved changes first (Save / Don't save /
    /// Cancel), then starts over.
    pub(crate) fn new_project(&mut self) {
        if self.loading.is_some() {
            return;
        }
        if self.is_modified() && self.dialogs.unsaved.is_none() {
            self.dialogs.unsaved = Some(crate::save::Pending::New);
            return;
        }
        self.new_project_now();
    }

    /// Starts over without asking: one empty plate, no objects, an empty undo history, and
    /// the current printer with its default print and filament profiles (or the ones
    /// Settings › Slicing names, when the printer offers them) and no edits.
    pub(crate) fn new_project_now(&mut self) {
        self.close_gcode();
        self.reset_scene();
        self.undo = Undo::default();
        self.tower_selected = false;
        self.towers.clear();
        self.tower_cache.clear();
        self.overlay_cache.clear();
        self.slice_cache.clear();
        self.dismissed_guards.clear();
        self.last_change = None;
        self.editor.object_mode = false;
        self.editor.plate_mode = false;
        self.editor.tower_mode = false;
        self.dialogs.calibration = None;
        self.profiles
            .reset_to_defaults(&self.prefs.defaults.print, &self.prefs.defaults.filament);
        self.overlays = Overlays::new(self.profiles.nozzle_count());
        self.settings_changed();
        // Nothing to slice yet; an automatic slice has nothing to wait for.
        self.dirty_since = None;
        self.fit_current_plate(false);
        self.mark_clean();
        self.status = "New project".into();
    }

    /// Empties the scene for a project that replaces it: plates, selection, tools, the
    /// previous project's card and document. Undo keeps the previous scene (see `record`).
    fn reset_scene(&mut self) {
        self.stop_worker();
        self.cancel_batch();
        self.pending_slice = None;
        self.plates = Plates::new(self.bed.max - self.bed.min);
        self.selected = None;
        self.multi.clear();
        self.sub = None;
        self.cut = None;
        self.renaming = None;
        self.paint_session = None;
        self.overhang_overlay = None;
        self.preview = false;
        self.project = None;
        self.document = Default::default();
        self.mixes = Default::default();
    }

    // ----- a project's own settings ----------------------------------------------------------

    /// A 3MF with its own settings opened: its filament colours apply right away; the rest is
    /// used at once for a project made for the selected printer, else offered on the card.
    fn offer_project_settings(&mut self, settings: mica_profiles::project::ProjectSettings) {
        let prompt = crate::project::Prompt::new(settings, &self.profiles.all_printers());
        let decision = crate::project::decide(
            prompt.printer.as_ref(),
            self.profiles.vendor,
            &self.profiles.printer,
        );
        self.project = Some(prompt);
        match decision {
            crate::project::Decision::Use => self.use_project_settings(),
            crate::project::Decision::Ask => {
                self.apply_project_colours();
                self.settings_changed();
            }
        }
    }

    /// The project's filament colours into the tool overlays (also with "Keep mine").
    fn apply_project_colours(&mut self) {
        if let Some(p) = &self.project {
            self.overlays.resize(self.profiles.nozzle_count());
            crate::project::colour_overlay(&mut self.overlays, &p.settings.colours);
        }
    }

    fn preset_selection(&self) -> crate::project::Selection {
        crate::project::Selection {
            vendor: self.profiles.vendor,
            printer: self.profiles.printer.clone(),
            process: self.profiles.process.clone(),
            filaments: self.profiles.filaments.clone(),
            bed: self.profiles.bed.clone(),
            overlays: self.overlays.clone(),
        }
    }

    fn restore_selection(&mut self, s: crate::project::Selection) {
        if s.vendor != self.profiles.vendor || s.printer != self.profiles.printer {
            self.profiles.select_printer(s.vendor, &s.printer);
        }
        self.profiles.process = s.process;
        self.profiles.refresh();
        for (i, f) in s.filaments.into_iter().enumerate() {
            if i < self.profiles.filaments.len() && self.profiles.materials.contains(&f) {
                self.profiles.filaments[i] = f;
            }
        }
        self.profiles.bed = s.bed;
        self.profiles.refresh();
        self.overlays = s.overlays;
    }

    /// "Use project settings": the project's printer, process and filaments (matched to
    /// Mica's presets by name, else by model / material type) and its plate type, with the
    /// project's own values on top (its embedded configuration where it differs from them:
    /// the project slices as it was saved, as in OrcaSlicer and PrusaSlicer), and its colours.
    /// The project's values are not edits; edits made afterwards go on top of them.
    pub(crate) fn use_project_settings(&mut self) {
        let Some(prompt) = self.project.clone() else {
            return;
        };
        let before = self.preset_selection();
        self.record("Use project settings");
        let mut notes = Vec::new();
        if let Some((vi, name)) = &prompt.printer {
            if *vi != self.profiles.vendor || *name != self.profiles.printer {
                self.profiles.select_printer(*vi, name);
                let vendor = self.profiles.vendor_name().to_string();
                self.prefs.add_printer(&vendor, name);
            }
        } else if let Some(p) = &prompt.settings.printer {
            notes.push(format!(
                "printer {p} not found, kept {}",
                self.profiles.printer
            ));
        }
        if let Some(p) = &prompt.settings.process {
            match crate::project::match_name(&self.profiles.processes, p) {
                Some(found) => self.profiles.process = found,
                None => notes.push(format!(
                    "process {} not found, kept {}",
                    crate::preset_names::display_name(p),
                    crate::preset_names::display_name(&self.profiles.process)
                )),
            }
            self.profiles.refresh();
        }
        let tools = self.profiles.nozzle_count();
        for t in 0..tools {
            let Some(wanted) = prompt.settings.filaments.get(t) else {
                break;
            };
            let ty = prompt.settings.filament_types.get(t).map(String::as_str);
            match crate::project::match_filament(&self.profiles.materials, wanted, ty) {
                Some(found) => self.profiles.filaments[t] = found,
                None => notes.push(format!(
                    "filament {} not found, kept T{}",
                    crate::preset_names::display_name(wanted),
                    t + 1
                )),
            }
        }
        // The plate type the project was set up for (bed temperatures follow it).
        if let Some(bed) = &prompt.settings.bed
            && self.profiles.orca_dialect().is_some()
            && crate::panels::profiles::SNAPMAKER_BEDS.contains(&bed.as_str())
        {
            self.profiles.bed = bed.clone();
        }
        self.profiles.refresh();
        let base = self
            .profiles
            .config()
            .unwrap_or_else(|_| Config::defaults());
        let (layer, rejected) = prompt.settings.layer(
            &base,
            self.profiles.orca_dialect(),
            mica_profiles::project::Mode::Embedded,
        );
        let mut overlays = Overlays::new(self.profiles.nozzle_count());
        crate::project::colour_overlay(&mut overlays, &prompt.settings.colours);
        overlays.project = std::sync::Arc::new(layer);
        self.overlays = overlays;
        // The archive's own settings text describes exactly this selection and these values:
        // a save without further edits writes it back verbatim.
        self.document.applied = Some((self.presets(), self.overlays.clone()));
        self.settings_changed();
        // A printer change drops the plates' tower placements (they belong to a bed); the
        // project's own positions are valid again now that its printer is selected.
        for (plate, tower) in self.plates.plates.iter_mut().zip(&prompt.settings.towers) {
            if plate.tower_placement.is_none()
                && let Some([x, y]) = tower
            {
                plate.tower_placement = Some([*x, *y, 0.]);
            }
        }
        if let Some(p) = &mut self.project {
            p.changes = self.project_differences.len();
            p.updated = false;
            p.state = crate::project::State::Applied {
                before: Box::new(before),
            };
        }
        self.notices.extend(rejected.iter().cloned());
        notes.extend(rejected);
        self.status = if notes.is_empty() {
            "Using the project's settings".into()
        } else {
            format!("Using the project's settings; {}", notes.join("; "))
        };
    }

    /// "Update to current profile": today's presets instead of the values the project was
    /// saved with, keeping only what the project marks as deliberately changed (Orca's
    /// `different_settings_to_system`; a PrusaSlicer project lists none). Edits stay. An undo
    /// step; "Revert to my settings" still goes back to the selection before the project.
    pub(crate) fn update_project_to_current(&mut self) {
        let Some(prompt) = self.project.clone() else {
            return;
        };
        if !matches!(prompt.state, crate::project::State::Applied { .. }) {
            return;
        }
        self.record("Update to current profile");
        let base = self
            .profiles
            .config()
            .unwrap_or_else(|_| Config::defaults());
        let (mut layer, _) = prompt.settings.layer(
            &base,
            self.profiles.orca_dialect(),
            mica_profiles::project::Mode::Listed,
        );
        // Scopes whose preset was switched since stay without the project's values.
        if self.overlays.project.process == Config::new() {
            layer.drop_process();
        }
        for (tool, f) in self.overlays.project.filaments.iter().enumerate() {
            if *f == Config::new() {
                layer.drop_tool(tool);
            }
        }
        self.overlays.project = std::sync::Arc::new(layer);
        self.settings_changed();
        let kept = self.project_differences.len();
        if let Some(p) = &mut self.project {
            p.updated = true;
        }
        self.status = format!(
            "Updated to the current profiles; {kept} value{} the project changed itself kept",
            if kept == 1 { "" } else { "s" }
        );
    }

    /// "Keep mine": the card goes; the project's colours stay.
    pub(crate) fn keep_my_settings(&mut self) {
        if let Some(p) = &mut self.project {
            p.state = crate::project::State::Dismissed;
        }
    }

    /// "Revert to my settings" after an automatic or chosen apply; the colours stay.
    pub(crate) fn revert_project_settings(&mut self) {
        let Some(crate::project::State::Applied { before }) =
            self.project.as_ref().map(|p| p.state.clone())
        else {
            return;
        };
        self.record("Revert project settings");
        self.restore_selection(*before);
        self.apply_project_colours();
        self.document.applied = None;
        self.settings_changed();
        if let Some(p) = &mut self.project {
            p.state = crate::project::State::Dismissed;
        }
        self.status = "Back to your settings (the project's colors stay)".into();
    }

    /// Edits on a locked plate are refused with a hint.
    pub(crate) fn plate_locked(&mut self) -> bool {
        if self.plates.current().locked {
            self.status = "This plate is locked; unlock it from its plate label".into();
            true
        } else {
            false
        }
    }

    /// Deletes the selection; with a volume or range selected, only that.
    pub(crate) fn delete_selected(&mut self) {
        if self.plate_locked() {
            return;
        }
        if self.sub.is_some() {
            self.delete_sub();
            return;
        }
        let sel = self.selection();
        if sel.is_empty() {
            return;
        }
        self.record("Delete");
        let plate = self.plates.current_mut();
        for i in sel.iter().rev() {
            plate.objects.remove(*i);
        }
        self.select(None, false);
        self.changed();
    }

    /// Minimum footprint distance, including sequential-print clearance.
    pub(crate) fn arrange_spacing(&self) -> f64 {
        self.effective_config().map_or(6.0, |c| {
            mica_core::arrange::Settings::from_config(c).spacing
        })
    }

    fn arrange_settings(&self, plate: usize, rotations: bool) -> mica_core::arrange::Settings {
        let mut settings = self
            .plate_config(plate)
            .ok()
            .map_or_else(mica_core::arrange::Settings::default, |c| {
                mica_core::arrange::Settings::from_config(&c)
            });
        // Prusa reserves the maximum outer brim of all PrintObjects. Resolve
        // Mica object overrides too, without changing the slicing config.
        if let Ok(c) = self.plate_config(plate) {
            for o in &self.plates.plate(plate).objects {
                let mut object_config = c.clone();
                for (key, value) in &o.overrides {
                    let _ = object_config.set_raw(key, value);
                }
                settings.bed_margin = settings
                    .bed_margin
                    .max(mica_core::arrange::Settings::from_config(&object_config).bed_margin);
            }
        }
        settings.rotations = rotations;
        settings
    }

    /// The bed as the arrange sees it: outline, exclusion zones and the wipe tower's real
    /// footprint (brim, cone and ribs included) when the plate prints with one.
    fn arrange_bed(&self, plate: usize) -> mica_core::arrange::Bed {
        let mut forbidden: Vec<Vec<[f64; 2]>> = self
            .bed
            .exclusions
            .iter()
            .map(|z| z.iter().map(|p| [p[0] as f64, p[1] as f64]).collect())
            .collect();
        if let Some(t) = self.towers.get(plate).and_then(|t| t.as_ref()) {
            forbidden.push(t.shape.hull.clone());
        }
        mica_core::arrange::Bed {
            polygon: self
                .bed
                .polygon
                .iter()
                .map(|p| [p[0] as f64, p[1] as f64])
                .collect(),
            forbidden,
        }
    }

    /// Packs plate `plate`'s objects (locked plates refuse). Objects that do not fit move to
    /// a new plate, which is arranged in turn; objects larger than the bed stay put. Returns
    /// how many spilled and how many are too large for any plate.
    pub(crate) fn arrange_plate(&mut self, plate: usize, rotations: bool) -> (usize, usize) {
        let mut spilled = 0;
        let mut oversized = 0;
        let mut target = plate;
        loop {
            self.refresh_towers();
            let bed = self.arrange_bed(target);
            let settings = self.arrange_settings(target, rotations);
            let objects = &self.plates.plate(target).objects;
            if objects.is_empty() {
                break;
            }
            let items: Vec<mica_core::arrange::Item> = objects
                .iter()
                .map(|o| mica_core::arrange::Item {
                    footprint: o.footprint_hull(),
                    pivot: {
                        let p = o.pivot_world();
                        [p.x as f64, p.y as f64]
                    },
                    fixed: false,
                })
                .collect();
            let result = mica_core::arrange::arrange(&items, &bed, &settings);
            let objects = &mut self.plates.plates[target].objects;
            for (i, placement) in &result.placed {
                let o = &mut objects[*i];
                if placement.rotation.abs() > 1e-6 {
                    o.apply(Delta::Rotate {
                        axis: Vec3::Z,
                        angle: placement.rotation as f32,
                    });
                }
                o.position += Vec3::new(
                    placement.translation[0] as f32,
                    placement.translation[1] as f32,
                    0.,
                );
            }
            self.plates.plates[target].invalidate();
            // The tower is a fixed obstacle; its existing position survives Arrange.
            self.refresh_towers();
            // An object that does not fit even an empty bed (scaled past it) stays where it
            // is: spilling it would make a new plate for it forever.
            let mut unplaced: Vec<usize> = result
                .unplaced
                .iter()
                .copied()
                .filter(|&i| {
                    let alone = mica_core::arrange::arrange(
                        std::slice::from_ref(&items[i]),
                        &bed,
                        &settings,
                    );
                    let fits = !alone.placed.is_empty();
                    if !fits {
                        oversized += 1;
                    }
                    fits
                })
                .collect();
            if unplaced.is_empty() {
                break;
            }
            // Spill: the unplaced objects go to a fresh plate after this one.
            unplaced.sort_unstable();
            let mut moved = vec![];
            for i in unplaced.iter().rev() {
                moved.push(self.plates.plates[target].objects.remove(*i));
            }
            spilled += moved.len();
            let current = self.plates.current;
            let new = self.plates.add();
            self.plates.current = current;
            let overrides = self.plates.plates[target].overrides.clone();
            let tower_placement = self.plates.plates[target].tower_placement;
            let plate = &mut self.plates.plates[new];
            plate.overrides = overrides;
            plate.tower_placement = tower_placement;
            // A fresh plate has no id collisions. Preserve the original id
            // for every instance instead of assigning one id per copy.
            plate.objects.extend(moved.into_iter().rev());
            target = new;
        }
        (spilled, oversized)
    }

    /// The status after an arrange: what spilled and what is too large for the bed.
    fn arrange_status(&mut self, spilled: usize, oversized: usize) {
        let plural = |n: usize| if n == 1 { "" } else { "s" };
        let mut parts = vec![];
        if spilled > 0 {
            parts.push(format!(
                "{spilled} object{} did not fit and moved to a new plate",
                plural(spilled)
            ));
        }
        if oversized > 0 {
            parts.push(format!(
                "{oversized} object{} larger than the bed stayed where {}",
                plural(oversized),
                if oversized == 1 {
                    "it was"
                } else {
                    "they were"
                }
            ));
        }
        if oversized > 0 {
            self.notify_warning(parts.join("; "));
        } else if !parts.is_empty() {
            self.status = parts.join("; ");
        }
    }

    pub(crate) fn arrange(&mut self) {
        if self.plate_locked() {
            return;
        }
        self.record("Arrange");
        let current = self.plates.current;
        self.select(None, false);
        let (spilled, oversized) = self.arrange_plate(current, false);
        self.changed();
        self.arrange_status(spilled, oversized);
    }

    /// Arranges every unlocked plate: all their objects are gathered onto the first unlocked
    /// plate and packed, spilling onto further plates as needed; empty plates are dropped.
    pub(crate) fn arrange_all(&mut self) {
        let unlocked: Vec<usize> = (0..self.plates.len())
            .filter(|&i| !self.plates.plate(i).locked)
            .collect();
        let Some(&first) = unlocked.first() else {
            self.status = "Every plate is locked".into();
            return;
        };
        self.record("Arrange all plates");
        self.select(None, false);
        // Ids are per plate: renumber per (plate, id), so instances stay instances of each
        // other and objects from different plates do not merge.
        let mut gathered: Vec<Object> = vec![];
        let mut ids: BTreeMap<(usize, usize), usize> = BTreeMap::new();
        for &i in &unlocked {
            for mut o in std::mem::take(&mut self.plates.plates[i].objects) {
                let next = ids.len();
                o.object_id = *ids.entry((i, o.object_id)).or_insert(next);
                gathered.push(o);
            }
        }
        self.plates.plates[first].objects = gathered;
        // Empty unlocked plates (other than the first) go, so the spill creates clean ones.
        for &i in unlocked.iter().skip(1).rev() {
            if self.plates.plates[i].objects.is_empty() && self.plates.len() > 1 {
                self.plates.remove(i);
            }
        }
        self.drop_orphan_jobs();
        let (spilled, oversized) = self.arrange_plate(first, false);
        self.plates.current = first.min(self.plates.len() - 1);
        self.plates.invalidate_all();
        self.changed();
        self.fit_current_plate(self.plates.len() > 1);
        self.status = if spilled > 0 {
            format!("Arranged over {} plates", self.plates.len())
        } else {
            "Arranged all plates".into()
        };
        if oversized > 0 {
            self.arrange_status(0, oversized);
        }
    }

    /// Auto-orient: the orientation search of `mica_core::orient` on each selected object
    /// (minimises overhangs and support need, prefers a large flat contact face). One undo
    /// entry.
    pub(crate) fn auto_orient(&mut self) {
        let sel = self.selection();
        if sel.is_empty() || self.plate_locked() {
            return;
        }
        self.record("Auto-orient");
        let mut note = None;
        for &i in &sel {
            let Some(o) = self.plates.current().objects.get(i) else {
                continue;
            };
            let placed = o.placed_mesh();
            let result = mica_core::orient::best_orientation(&placed.vertices, &placed.faces);
            let r = result.rotation;
            if r.angle.abs() > 1e-4 {
                let o = &mut self.plates.current_mut().objects[i];
                o.apply(Delta::Rotate {
                    axis: Vec3::new(r.axis[0] as f32, r.axis[1] as f32, r.axis[2] as f32),
                    angle: r.angle as f32,
                });
            }
            if sel.len() == 1 {
                note = Some(format!(
                    "Auto-orient: overhang {:.0} mm², contact {:.0} mm², height {:.1} mm",
                    result.best.overhang_area, result.best.bottom_area, result.best.height
                ));
            }
        }
        self.changed();
        if let Some(n) = note {
            self.status = n;
        }
    }

    pub(crate) fn clear_plate(&mut self) {
        if self.plate_locked() {
            return;
        }
        self.record("Clear plate");
        self.plates.current_mut().objects.clear();
        self.selected = None;
        self.changed();
    }

    // ----- slicing --------------------------------------------------------------------------

    /// The configuration plate `plate` slices with: the global one plus the plate's
    /// overrides and its imported tower placement (an explicit edit of `wipe_tower_x/y` in
    /// the settings wins over the placement the project came with).
    pub(crate) fn plate_config(&self, plate: usize) -> Result<Config, String> {
        let p = self.plates.plate(plate);
        let mut c = match crate::plate_settings::bed_type(&p.overrides) {
            Some(bed) if self.profiles.is_orca() && bed != self.profiles.bed => {
                self.bed_config(bed)?
            }
            _ => self.config.clone()?,
        };
        for (k, v) in &p.overrides {
            if k == crate::plate_settings::BED_TYPE {
                continue;
            }
            c.set_raw(k, v).map_err(|e| e.to_string())?;
        }
        if let Some(t) = p.tower_placement {
            for (key, value) in ["wipe_tower_x", "wipe_tower_y", "wipe_tower_rotation_angle"]
                .into_iter()
                .zip(t)
            {
                if self.overlays.print.contains_key(key) {
                    continue;
                }
                c.set_raw(key, &value.to_string())
                    .map_err(|e| e.to_string())?;
            }
        }
        Ok(c)
    }

    /// The global configuration (presets and edits) on bed type `bed`, as a plate with its own
    /// bed type slices with it (bed temperatures follow the bed in Orca bundles).
    fn bed_config(&self, bed: &str) -> Result<Config, String> {
        if let Some(c) = self.bed_configs.borrow().get(bed) {
            return c.clone();
        }
        let identity = self.printer_gcode_identity();
        let c = self.profiles.config_for_bed(bed).map(|mut c| {
            compose(&mut c, &self.overlays, false, identity);
            c
        });
        self.bed_configs
            .borrow_mut()
            .insert(bed.to_string(), c.clone());
        c
    }

    /// Starts slicing plate `i`; `manual` switches to the preview when it finishes.
    pub(crate) fn slice_plate(&mut self, i: usize, manual: bool, ctx: &egui::Context) {
        if self.loading.is_some() || self.plates.plate(i).objects.is_empty() {
            return;
        }
        if self.viewing_gcode() {
            self.status = "Close the G-code file to slice the plate".into();
            return;
        }
        // A tower over an object (or off the bed) would print into it: refuse, say why.
        if let Some(why) = self.tower_blocks_slicing(i) {
            // Slice all goes on with the next plate (`advance_batch`).
            self.slice_failed(i, why);
            return;
        }
        self.slice_requests += 1;
        let request = self.slice_requests;
        if self.worker.is_some() || self.stopping.is_some() {
            // Never two jobs at once: the running one is cancelled and this request waits
            // for it to stop. Only the latest request is kept.
            self.stop_worker();
            self.pending_slice = Some((self.plates.plate(i).id, manual, request));
            self.status = "Canceling the running slice, then slicing again…".into();
            return;
        }
        self.start_job(i, manual, request, ctx);
    }

    /// The configuration plate `i`'s slice job runs with: the plate's configuration (a
    /// calibration plate prints on its tool with that tool's entries). The G-code of the job
    /// has to be judged against this one.
    pub(crate) fn job_config(&self, i: usize) -> Result<Config, String> {
        let mut c = self.plate_config(i)?;
        if !self.prefs.features.textures {
            crate::textures::config_without_textures(&mut c);
        }
        if !self.prefs.features.brick_layers_enabled() {
            mica_core::features::config_without_brick_layers(&mut c);
        }
        Ok(c)
    }

    /// Starts the job answering `request` on plate `i`; nothing else may be running.
    fn start_job(&mut self, i: usize, manual: bool, request: u64, ctx: &egui::Context) {
        debug_assert!(self.worker.is_none() && self.stopping.is_none());
        match self.job_config(i) {
            Ok(c) => {
                self.plates.plates[i].invalidate();
                self.progress = 0.;
                self.progress_fraction = 0.;
                self.progress_stage = "Preparing".into();
                self.manual_slice = manual;
                self.dirty_since = None;
                // A calibration plate regenerates its test from the parameters; an ordinary
                // plate slices its objects.
                let inputs = Arc::new(crate::slice_cache::Inputs::new(self.slice_job(i), c));
                let job = inputs.job.to_worker();
                let id = self.plates.plate(i).id;
                self.worker = Some((
                    id,
                    Worker::start(job, inputs.config.clone(), ctx.clone(), request),
                ));
                self.job_inputs = Some((request, inputs));
                self.status = format!("Slicing {}…", self.plates.plate(i).name);
            }
            Err(e) => self.slice_failed(i, e),
        }
    }

    pub(crate) fn slice_current(&mut self, ctx: &egui::Context) {
        self.slice_queue.clear();
        let i = self.plates.current;
        self.slice_plate(i, true, ctx);
    }

    /// Forgets jobs whose plate is gone (removed, or replaced by an undo or a project): the
    /// running one is cancelled, waiting and queued ones dropped.
    pub(crate) fn drop_orphan_jobs(&mut self) {
        if let Some((id, _)) = &self.worker
            && self.plates.index_of(*id).is_none()
        {
            self.stop_worker();
        }
        if let Some((id, _, _)) = self.pending_slice
            && self.plates.index_of(id).is_none()
        {
            self.pending_slice = None;
        }
        let plates = &self.plates;
        self.slice_queue.retain(|id| plates.index_of(*id).is_some());
    }

    pub(crate) fn cancel_slice(&mut self) {
        let had_job =
            self.worker.is_some() || self.pending_slice.is_some() || self.slice_batch.is_some();
        self.stop_worker();
        self.pending_slice = None;
        if had_job {
            self.cancel_batch();
            self.status = "Canceled".into();
        }
    }

    /// The slice Preview shows: an opened G-code file's, else the current plate's.
    pub(crate) fn finished(&self) -> Option<&worker::Finished> {
        if let Some(doc) = &self.gcode {
            return Some(&doc.finished);
        }
        self.plates.current().finished.as_ref()
    }

    pub(crate) fn export(&mut self, ctx: &egui::Context) {
        if self.viewing_gcode() {
            self.save_gcode_copy(ctx);
            return;
        }
        if self.headless {
            self.status = "Exporting needs the file dialog".into();
            return;
        }
        if self.reslice_for_colours() {
            return;
        }
        let Some(f) = self.finished() else { return };
        let stem = self.export_stem();
        let mut dialog = rfd::FileDialog::new().set_file_name(if f.binary {
            format!("{stem}.bgcode")
        } else {
            format!("{stem}.gcode")
        });
        if f.binary {
            dialog = dialog.add_filter("Prusa binary G-code", &["bgcode"]);
        }
        dialog = dialog.add_filter("Text G-code", &["gcode"]);
        if let Some(mut path) = dialog.save_file() {
            if path.extension().is_none() {
                path.set_extension(if f.binary { "bgcode" } else { "gcode" });
            }
            let extension = path
                .extension()
                .unwrap_or_default()
                .to_string_lossy()
                .to_lowercase();
            if !matches!(extension.as_str(), "gcode" | "bgcode")
                || extension == "bgcode" && !f.binary
            {
                self.notify_warning("Choose .gcode, or .bgcode for a Prusa printer");
                return;
            }
            let job = f.preview.job.clone();
            let source = job.path.join(format!("print.{extension}"));
            // Copy large exports off the UI thread. Retain the temp job until the copy finishes.
            let (tx, rx) = mpsc::channel();
            self.loading = Some(rx);
            self.status = "Exporting…".into();
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                let result = std::fs::copy(source, &path)
                    .map(|_| LoadedScene {
                        plates: vec![],
                        objects: vec![],
                        warnings: vec![format!("Exported {}", path.display())],
                        tower: None,
                        paths: vec![],
                        settings: None,
                        config: None,
                        project: false,
                        custom_gcode: Default::default(),
                        mixes: None,
                    })
                    .map_err(|e| format!("Export failed: {}: {e}", path.display()));
                drop(job);
                let _ = tx.send(result);
                ctx.request_repaint();
            });
        }
    }

    pub(crate) fn poll(&mut self, ctx: &egui::Context) {
        self.drop_orphan_jobs();
        self.poll_gcode(ctx);
        // A cancelled job: wait for its thread to confirm, then run the request that
        // replaced it (if any).
        if let Some(w) = &self.stopping {
            let mut stopped = false;
            loop {
                match w.receiver.try_recv() {
                    Ok(Event::Done(_)) | Err(mpsc::TryRecvError::Disconnected) => {
                        stopped = true;
                        break;
                    }
                    Ok(Event::Progress(_)) => {}
                    Err(mpsc::TryRecvError::Empty) => break,
                }
            }
            if stopped {
                self.stopping = None;
                if let Some((id, manual, request)) = self.pending_slice.take()
                    && let Some(i) = self.plates.index_of(id)
                {
                    self.start_job(i, manual, request, ctx);
                }
            } else {
                ctx.request_repaint_after(Duration::from_millis(50));
            }
        }
        if let Some(rx) = &self.loading {
            match rx.try_recv() {
                Ok(result) => {
                    self.loading = None;
                    self.finish_load(result);
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.loading = None;
                    self.notify_error(
                        "Background operation stopped unexpectedly",
                        "Background operation stopped unexpectedly",
                    );
                }
                Err(_) => {}
            }
        }
        let mut done = None;
        if let Some((_, w)) = &self.worker {
            while let Ok(event) = w.receiver.try_recv() {
                match event {
                    Event::Progress(p) => {
                        self.progress = p.overall;
                        self.progress_fraction = p.fraction;
                        self.progress_stage = p.stage;
                        self.status = p.detail;
                    }
                    Event::Done(result) => done = Some(*result),
                }
            }
        }
        if let Some(result) = done
            && let Some((id, w)) = self.worker.take()
        {
            self.finished_request = Some(w.request);
            let failed_plate = self.plates.index_of(id);
            // The plate is looked up by id: it may have moved since the job started.
            let result = match failed_plate {
                Some(plate) => result.map(|f| (plate, f)),
                None => Err("The sliced plate is gone; its slice was discarded".to_string()),
            };
            let inputs = self
                .job_inputs
                .take()
                .filter(|(r, _)| *r == w.request)
                .map(|(_, inputs)| inputs);
            match result {
                Ok((plate, f)) => {
                    // An opened G-code file keeps the view; the scene's comes back on close.
                    if plate == self.plates.current && self.gcode.is_some() {
                        if let Some(doc) = &mut self.gcode {
                            doc.restore.view = Self::view_for(&f);
                        }
                    } else if plate == self.plates.current {
                        let fresh = Self::view_for(&f);
                        self.view = match self.keep_view.take() {
                            Some(old) if old.high < f.preview.layers.len() => View {
                                color: if fresh.color == crate::preview::ColorMode::ColorPrint
                                    && matches!(
                                        old.color,
                                        crate::preview::ColorMode::Feature
                                            | crate::preview::ColorMode::Tool
                                    ) {
                                    fresh.color
                                } else {
                                    old.color
                                },
                                ..old
                            },
                            _ => fresh,
                        };
                        if self.manual_slice {
                            self.preview = true;
                        }
                    }
                    self.status = format!(
                        "{} sliced · {} · {:.2} m filament",
                        self.plates.plate(plate).name,
                        crate::panels::duration(f.summary.time_s),
                        f.summary.filament_mm / 1000.
                    );
                    self.store_slice(plate, inputs, f);
                    if let Some(text) = self.plates.plate(plate).delta().and_then(|d| d.text()) {
                        self.status
                            .push_str(&format!(" · since your last change: {text}"));
                    }
                    // The sliced job knows the tower's exact size.
                    self.refresh_towers();
                }
                Err(e) => {
                    // Slice all notes the failure and goes on with the next plate.
                    match failed_plate {
                        Some(plate) => self.slice_failed(plate, e),
                        // Its plate was removed meanwhile: nothing shows a slice of it.
                        None => self.status = e,
                    }
                    self.progress = 0.;
                }
            }
            if let Some((id, manual, request)) = self.pending_slice.take()
                && let Some(i) = self.plates.index_of(id)
            {
                self.start_job(i, manual, request, ctx);
            }
        }
        // Slice all's next plate, or what waited for it (Export all plates, sending).
        self.advance_batch(ctx);
        self.poll_export_all();
        if std::mem::take(&mut self.reslice_requested) && !self.busy() {
            self.slice_current(ctx);
        }
        // Automatic re-slice shortly after the last change (Settings › Slicing).
        let delay = self.prefs.auto_slice_delay();
        if self.prefs.auto_slice
            && !self.headless
            && self.gcode.is_none()
            && let Some(since) = self.dirty_since
        {
            if since.elapsed() >= delay {
                self.dirty_since = None;
                if !self.busy()
                    && self.config.is_ok()
                    && !self.plates.current().objects.is_empty()
                    && self.plates.current().finished.is_none()
                {
                    let i = self.plates.current;
                    self.slice_plate(i, false, ctx);
                }
            } else {
                ctx.request_repaint_after(delay.saturating_sub(since.elapsed()));
            }
        }
        self.poll_printers(ctx);
    }

    /// A completed slice of plate `plate`, made from `inputs`: the plate shows it and the
    /// cache keeps it for when the plate returns to this state.
    pub(crate) fn store_slice(
        &mut self,
        plate: usize,
        inputs: Option<Arc<crate::slice_cache::Inputs>>,
        f: worker::Finished,
    ) {
        if let Some(inputs) = &inputs {
            self.slice_cache.insert(inputs.clone(), &f);
        }
        let p = &mut self.plates.plates[plate];
        p.sliced = inputs;
        p.finished = Some(f);
        p.slice_error = None;
        self.slice_cache.trim();
    }

    /// A slice of plate `i` failed with `message`: the plate forgets its slice and shows the
    /// error (header, notification, plate label) until the next slice or edit.
    pub(crate) fn slice_failed(&mut self, i: usize, message: String) {
        let id = self.plates.plate(i).id;
        self.batch_failure(id, &message);
        let p = &mut self.plates.plates[i];
        p.invalidate();
        p.slice_error = Some(crate::plates::SliceError::new(message.clone()));
        let record = (p.id, p.epoch, message);
        self.failed_slices.push(record);
        if self.failed_slices.len() > 16 {
            self.failed_slices.remove(0);
        }
        if i == self.plates.current {
            self.preview = false;
        }
        self.status.clear();
    }

    pub(crate) fn save_prefs(&mut self) {
        if let Some(path) = &self.prefs_path
            && self.prefs_dirty
        {
            self.profiles.store_selection(&mut self.prefs);
            if let Err(e) = self.prefs.save(path) {
                self.notify_error("Settings were not saved", e);
            }
            self.prefs_dirty = false;
        }
        if self.printers.dirty
            && let Err(e) = self.printers.save()
        {
            self.notify_error("The printer list was not saved", e);
        }
    }

    // ----- input ----------------------------------------------------------------------------

    // Keyboard shortcuts: `keymap.rs` (`App::shortcuts`, `App::run_key`).

    // ----- viewport -------------------------------------------------------------------------

    /// Whether the status text is a routine state that needs no announcement.
    fn routine_status(status: &str) -> bool {
        status.is_empty()
            || matches!(
                status,
                "Ready to slice"
                    | "Open an STL or 3MF to begin"
                    | "Slice complete"
                    | "Undone"
                    | "Redone"
            )
            || [
                "Preparing",
                "Scheduling",
                "Slicing",
                "Packing",
                "Ready to export",
            ]
            .iter()
            .any(|s| status.starts_with(s))
            || status.contains(" sliced · ")
    }

    fn viewport(&mut self, ui: &mut egui::Ui) {
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ui, |ui| {
                self.viewport_rect = ui.available_rect_before_wrap();
                // Clicks place the text while the tool edits a text volume.
                let text = self.emboss_target();
                self.viewport.emboss = matches!(text, Some((_, Some(_))));
                self.viewport.emboss_open = text.is_some();
                // An opened G-code file shows on the current plate's bed with nothing else.
                let doc = self.gcode.is_some();
                let (objects, map) = if doc {
                    (vec![], vec![])
                } else {
                    self.scene_objects()
                };
                let plate_of: Vec<usize> = map.iter().map(|&(plate, _)| plate).collect();
                let mut snapshot = plates::snapshot(&self.plates);
                if doc {
                    snapshot = crate::gcode_doc::single_plate();
                }
                for p in &mut snapshot {
                    p.alert = match self.plate_alert(p.index) {
                        Some(crate::notifications::Level::Error) => 2,
                        Some(_) => 1,
                        None => 0,
                    };
                    if !doc {
                        p.status = self.plate_status(p.index);
                    }
                }
                let tool_colors = self.scene_colours_f32();
                let tool_labels: Vec<String> =
                    self.tool_choices().into_iter().map(|c| c.label).collect();
                // A Preview waiting for its new slice shows the models, not for editing.
                let editable = self.loading.is_none() && !self.previewing();
                let current_len = self.plates.current().objects.len();
                let selected = self.selected.filter(|&i| i < current_len);
                let selection = self.selection();
                let movable: Vec<usize> = (0..current_len)
                    .filter(|&k| self.move_inside_shift(self.plates.current, k).is_some())
                    .filter_map(|k| map.iter().position(|&m| m == (self.plates.current, k)))
                    .collect();
                let cut = self.cut.clone().filter(|_| self.cut_target().is_some());
                if self.cut.is_some() && cut.is_none() {
                    self.cut = None;
                }
                let status = (self.status_at.elapsed() < Duration::from_secs(6)
                    && !Self::routine_status(&self.status))
                .then(|| self.status.clone());
                let totals = self.finished().map(|f| crate::viewport::Totals {
                    time_s: f.summary.time_s,
                    grams: f.summary.grams,
                    filament_mm: f.summary.filament_mm,
                    delta: self.plates.current().delta().and_then(|d| d.text()),
                    cause: self.last_change.clone(),
                });
                let mut overlays = self.paint_overlays(&objects, &tool_colors);
                // Surface texture previews for the 3D view (none with Textures switched off).
                let surfaces = match self.effective_config() {
                    Some(c) if self.prefs.features.textures => {
                        crate::textures::previews(&objects, c)
                    }
                    _ => vec![],
                };
                crate::textures::publish(ui.ctx(), surfaces);
                if let Some(o) = self.overhang_overlay(&objects) {
                    // Under the paint so painted overhangs show their paint.
                    overlays.insert(0, o);
                }
                if let Some(o) = self.fill_preview_overlay(&objects, &tool_colors) {
                    overlays.push(o);
                }
                // The object tools need a selected object on this plate.
                if self.viewport.object_tool.is_some() && selected.is_none() {
                    self.viewport.object_tool = None;
                }
                let object_tool = self.object_tool_view(&objects, selected, &mut overlays);
                let paint_view = self.viewport.paint.map(|kind| crate::viewport::PaintView {
                    kind,
                    brush: self.paint.brush,
                    radius: self.paint.radius,
                    erase: self.paint.erase,
                });
                let progress = self
                    .worker
                    .as_ref()
                    .filter(|(id, _)| *id == self.plates.current().id)
                    .map(|_| self.progress);
                let towers: Vec<crate::tower::TowerView> = if doc {
                    vec![]
                } else {
                    self.towers.iter().flatten().cloned().collect()
                };
                let tower_selected = self.tower_selected && self.tower().is_some();
                let (object_tools, base_tool) = self.plate_object_tools(self.plates.current);
                let (klipper, has_template) = self.config.as_ref().map_or((false, false), |c| {
                    (
                        c.string("gcode_flavor") == Some("klipper"),
                        !c.string("template_custom_gcode")
                            .unwrap_or("")
                            .trim()
                            .is_empty(),
                    )
                });
                let tools = self.tool_count();
                let preview = if let Some(d) = &self.gcode {
                    Some((&d.finished.preview, &mut self.view))
                } else if self.preview {
                    self.plates.plates[self.plates.current]
                        .finished
                        .as_ref()
                        .map(|f| (&f.preview, &mut self.view))
                } else {
                    None
                };
                let calibration = self.plates.plates[self.plates.current]
                    .calibration
                    .is_some();
                let layer_gcode =
                    (!calibration && !doc).then(|| crate::preview::layer_gcode::LayerGCode {
                        info: &self.plates.plates[self.plates.current].custom_gcode,
                        tools,
                        object_tools,
                        base_tool,
                        klipper,
                        has_template,
                        tool_colors: &tool_colors,
                    });
                let offset = self.plates.offset(self.plates.current);
                // The skirt or brim outlines that do not fit (drawn in red).
                let adhesion_outlines: Vec<Vec<[f32; 2]>> = self
                    .adhesion
                    .iter()
                    .map(|(_, o)| {
                        o.outline
                            .iter()
                            .map(|p| [p[0] as f32 + offset.x, p[1] as f32 + offset.y])
                            .collect()
                    })
                    .collect();
                let clearance: Vec<(Vec<[f32; 2]>, bool)> = self
                    .clearance
                    .iter()
                    .map(|(outline, colliding)| {
                        (
                            outline
                                .iter()
                                .map(|p| [p[0] + offset.x, p[1] + offset.y])
                                .collect(),
                            *colliding,
                        )
                    })
                    .collect();
                let current_plate = self.plates.current;
                let warned: Vec<(usize, String)> = self
                    .adhesion
                    .iter()
                    .filter_map(|(k, o)| {
                        let scene = map.iter().position(|&m| m == (current_plate, *k))?;
                        let side = o.sides.first().map_or("", |(s, _)| s.name());
                        Some((
                            scene,
                            format!(
                                "{} {}",
                                match o.kind {
                                    mica_core::adhesion_check::Kind::Brim => "Brim",
                                    mica_core::adhesion_check::Kind::Skirt => "Skirt",
                                },
                                if side.is_empty() {
                                    "past the bed edge".to_string()
                                } else {
                                    format!("past the {side} edge")
                                }
                            ),
                        ))
                    })
                    .collect();
                let failed = self
                    .plates
                    .current()
                    .slice_error
                    .as_ref()
                    .filter(|_| !doc)
                    .map(|e| e.short.clone());
                let (clearance, warned, movable, adhesion_outlines) = if doc {
                    (vec![], vec![], vec![], vec![])
                } else {
                    (clearance, warned, movable, adhesion_outlines)
                };
                let actions = self.viewport.show(
                    ui,
                    crate::viewport::Scene {
                        textures: self.prefs.features.textures,
                        objects: &objects,
                        plate_of: &plate_of,
                        selected,
                        selection: &selection,
                        plate_objects: current_len,
                        only_object: (current_len == 1)
                            .then(|| map.iter().position(|&m| m == (current_plate, 0)))
                            .flatten(),
                        sub: self.sub,
                        towers: &towers,
                        tower_selected,
                        cut: cut.as_ref(),
                        mesh_tool: match &self.mesh_tool {
                            Some(crate::mesh_tools::MeshTool::Boolean(_)) => {
                                Some(crate::viewport::icons::Icon::Boolean)
                            }
                            Some(crate::mesh_tools::MeshTool::Simplify(_)) => {
                                Some(crate::viewport::icons::Icon::Simplify)
                            }
                            None => None,
                        },
                        can_paste: !self.clipboard.is_empty(),
                        bed: self
                            .gcode
                            .as_ref()
                            .and_then(|d| d.bed.as_ref())
                            .unwrap_or(&self.bed),
                        plates: &snapshot,
                        tool_colors: &tool_colors,
                        tool_labels: &tool_labels,
                        overlays: &overlays,
                        paint: paint_view,
                        object_tool,
                        preview,
                        editable: editable && !doc,
                        can_slice_all: !doc && self.config.is_ok() && self.loading.is_none(),
                        status,
                        totals,
                        progress,
                        layer_gcode,
                        clearance: &clearance,
                        warned: &warned,
                        failed,
                        movable: &movable,
                        adhesion_outlines: &adhesion_outlines,
                    },
                );
                let ctx = ui.ctx().clone();
                for action in actions {
                    // Nothing edits the hidden scene while a G-code file shows.
                    if doc
                        && !matches!(
                            action,
                            crate::viewport::Action::Open | crate::viewport::Action::ViewChanged
                        )
                    {
                        continue;
                    }
                    self.handle(action, &map, &ctx);
                }
                // Models dragged in from the Library.
                if !doc {
                    self.library_drop_in_viewport(&objects, &map, &ctx);
                }
            });
    }

    /// Editor side of the viewport protocol. `map` translates scene indices to
    /// `(plate, object)`.
    pub(crate) fn handle(&mut self, action: Action, map: &[(usize, usize)], ctx: &egui::Context) {
        let busy = self.loading.is_some();
        // Objects on another plate: switch to it and select there.
        let resolve = |app: &mut App, index: usize| -> Option<usize> {
            let &(plate, object) = map.get(index)?;
            if plate != app.plates.current {
                app.plates.current = plate;
                app.selected = None;
                app.preview = false;
                app.refresh_warnings();
            }
            Some(object)
        };
        match action {
            Action::Open if !busy => self.open_dialog(ctx),
            Action::AddShape(m, add_as) if !busy => self.add_builtin(m, add_as),
            Action::Arrange if !busy => self.arrange(),
            Action::PlateArrange(i) if !busy => {
                if i < self.plates.len() && self.plates.plate(i).locked {
                    self.status = "This plate is locked; unlock it from its plate label".into();
                } else if i < self.plates.len() {
                    self.record("Arrange");
                    if i != self.plates.current {
                        self.select(None, false);
                    }
                    let (spilled, oversized) = self.arrange_plate(i, false);
                    if i == self.plates.current {
                        self.changed();
                    }
                    self.arrange_status(spilled, oversized);
                }
            }
            Action::PlateOrient(i) if !busy => {
                if i < self.plates.len() && !self.plates.plate(i).locked {
                    self.select_plate(i);
                    let all: Vec<usize> = (0..self.plates.current().objects.len()).collect();
                    self.select_many(&all, false);
                    self.auto_orient();
                    self.select(None, false);
                }
            }
            Action::Select { index, toggle } => {
                let resolved = index.and_then(|i| resolve(self, i));
                self.select(resolved, toggle && resolved.is_some());
            }
            Action::BoxSelect { indices, toggle } => {
                // Only objects of the current plate join a box selection.
                let own: Vec<usize> = indices
                    .iter()
                    .filter_map(|&i| map.get(i))
                    .filter(|(p, _)| *p == self.plates.current)
                    .map(|(_, o)| *o)
                    .collect();
                if !own.is_empty() || !toggle {
                    self.select_many(&own, toggle);
                }
            }
            Action::SelectSub { index, sub } => {
                if let Some(oi) = resolve(self, index) {
                    if self.selected != Some(oi) {
                        self.select(Some(oi), false);
                    }
                    self.sub = sub;
                    if sub.is_some() {
                        self.editor.object_mode = true;
                    }
                }
            }
            Action::Command { index, command } if !busy => {
                if let Some(i) = index
                    && let Some(oi) = resolve(self, i)
                    && !self.selection().contains(&oi)
                {
                    self.select(Some(oi), false);
                }
                self.run_command(command, ctx);
            }
            Action::CutPlane(delta) if !busy => {
                if let Some(c) = &mut self.cut {
                    c.apply(delta);
                }
            }
            Action::CutConnector(edit) if !busy => {
                let target = self.cut_target();
                if let Some(c) = &mut self.cut {
                    // A click off the cut face places nothing (it may still pick one).
                    if let crate::cut::ConnectorEdit::Click(p) = edit
                        && c.connector_at(p).is_none()
                        && !target
                            .and_then(|i| self.plates.current().objects.get(i))
                            .is_some_and(|o| crate::cut::on_cut_face(o, c, p))
                    {
                        self.status = "Click on the cut face to place a connector".into();
                    } else {
                        c.edit_connectors(edit);
                    }
                }
            }
            Action::SelectTower if !busy => self.select_tower(),
            Action::TowerTransform(delta) if !busy => {
                if self.tower().is_some() && !self.plates.current().locked {
                    // Drags open a gesture first; a bare delta (a nudge) is one step.
                    if !self.undo.in_gesture() {
                        self.record(match delta {
                            Delta::Rotate { .. } => "Turn prime tower",
                            _ => "Move prime tower",
                        });
                    }
                    if !self.tower_selected {
                        self.select_tower();
                    }
                    self.move_tower(delta);
                }
            }
            Action::TowerCommand(cmd) if !busy => self.run_tower_command(cmd),
            Action::GestureBegin(label) if !busy => self.begin_gesture(label),
            Action::GestureCancel => self.cancel_gesture(),
            Action::Paint {
                index,
                hit,
                mut trail,
                erase,
            } if !busy => {
                if let Some(oi) = resolve(self, index) {
                    self.selected = Some(oi);
                    trail.push(hit);
                    self.paint_stroke(&trail, erase);
                }
            }
            Action::BrushSize(delta) if self.viewport.object_tool.is_some() => {
                self.object_tool_size(delta);
            }
            Action::BrushSize(delta) => {
                let r = self.paint.radius;
                self.paint.set_radius(r + delta * r.max(1.));
            }
            Action::ObjectTool(tool) => self.object_tool_changed(tool),
            Action::LayerEdit {
                index,
                z,
                amount,
                shift,
                alt,
            } if !busy => {
                if let Some(oi) = resolve(self, index) {
                    self.selected = Some(oi);
                    self.layer_edit_step(z, amount, shift, alt);
                }
            }
            Action::AddBrimEar { index, at } if !busy => {
                if let Some(oi) = resolve(self, index) {
                    self.selected = Some(oi);
                    self.add_brim_ear(at);
                }
            }
            Action::RemoveBrimEar { index, ear } if !busy => {
                if let Some(oi) = resolve(self, index) {
                    self.selected = Some(oi);
                    self.remove_brim_ear(ear);
                }
            }
            Action::PaintTool(kind) => {
                self.paint_session = None;
                self.overhang_overlay = None;
                if let Some(kind) = kind {
                    self.paint_session_for(kind);
                    self.status = format!(
                        "{}: left-drag to paint · Shift-drag to erase · Alt-scroll to change brush size",
                        crate::paint::tool_name(kind)
                    );
                }
            }
            Action::Transform { index, delta } if !busy => {
                if let Some(oi) = resolve(self, index)
                    && !self.plate_locked()
                {
                    // Drags open a gesture first; a bare delta (quick buttons) is one step.
                    if !self.undo.in_gesture() {
                        self.record(match delta {
                            Delta::Translate(_) => "Move",
                            Delta::Rotate { .. } => "Rotate",
                            Delta::Scale(_) => "Scale",
                        });
                    }
                    if !self.selection().contains(&oi) {
                        self.select(Some(oi), false);
                    }
                    match (self.sub, self.selected) {
                        // A selected volume moves alone, relative to its object.
                        (Some(scene::Sub::Volume(id)), Some(p)) if p == oi => {
                            if let Some(o) = self.plates.current_mut().objects.get_mut(oi) {
                                o.apply_to_volume(id, delta);
                            }
                            self.sync_instances(oi);
                        }
                        _ => self.transform_selection(delta),
                    }
                    self.changed();
                }
            }
            Action::SetRotation { index, euler } if !busy => {
                if let Some(oi) = resolve(self, index) {
                    self.begin_gesture("Rotate");
                    if let Some(o) = self.plates.current_mut().objects.get_mut(oi) {
                        o.set_euler(euler);
                        o.drop_to_bed();
                    }
                    self.changed();
                }
            }
            Action::LayOnFace { index, face } if !busy => {
                if let Some(oi) = resolve(self, index)
                    && !self.plate_locked()
                {
                    self.record("Lay on face");
                    scene::lay_on_face(&mut self.plates.current_mut().objects, oi, face);
                    self.selected = Some(oi);
                    self.changed();
                }
            }
            Action::LayOnPlane { index, normal } if !busy => {
                if let Some(oi) = resolve(self, index)
                    && !self.plate_locked()
                {
                    self.record("Lay on face");
                    if let Some(o) = self.plates.current_mut().objects.get_mut(oi) {
                        o.lay_on_face(normal);
                    }
                    self.selected = Some(oi);
                    self.changed();
                }
            }
            Action::Edited => self.end_gesture(),
            Action::AddPlate if !busy => {
                self.record("Add plate");
                self.plates.add();
                self.selected = None;
                self.changed();
                self.fit_current_plate(true);
            }
            Action::SelectPlate(i) => self.select_plate(i),
            Action::SliceAll => self.slice_all(ctx),
            Action::RemovePlate(i) if !busy => {
                if i < self.plates.len() {
                    self.record("Remove plate");
                    self.plates.remove(i);
                    self.drop_orphan_jobs();
                    self.selected = None;
                    self.changed();
                }
            }
            Action::ViewChanged => {}
            Action::Hint(text) => self.status = text,
            Action::Plate(i, c) => self.plate_command(i, c),
            Action::RenamePlate(i, name) if !busy => self.rename_plate(i, &name),
            Action::MovePlate { from, to } if !busy => self.move_plate(from, to),
            Action::PlateLock(i) if !busy => {
                if i < self.plates.len() {
                    self.record("Lock plate");
                    let p = &mut self.plates.plates[i];
                    p.locked = !p.locked;
                    self.status = format!(
                        "{} {}",
                        self.plates.plate(i).name,
                        if self.plates.plate(i).locked {
                            "locked: its layout stays as it is"
                        } else {
                            "unlocked"
                        }
                    );
                }
            }
            Action::Cut => self.toggle_cut(),
            Action::LayerGCode(edit) if !busy => self.edit_layer_gcode(edit, ctx),
            Action::Emboss if !busy => self.toggle_emboss(),
            Action::EmbossCancel => self.cancel_emboss(),
            Action::EditText { index, volume } if !busy => {
                if let Some(oi) = resolve(self, index) {
                    if self.selected != Some(oi) {
                        self.select(Some(oi), false);
                    }
                    self.sub = volume.map(crate::scene::Sub::Volume);
                    if !self.edit_text(oi, self.sub) {
                        self.status = "That is not a text".into();
                    }
                }
            }
            Action::EmbossPlace {
                index,
                point,
                normal,
            } if !busy => {
                if let Some(oi) = resolve(self, index) {
                    self.emboss_place(oi, point, normal);
                }
            }
            _ => {}
        }
        // Several actions move the primary selection directly (painting, laying on a face,
        // switching plates); what hangs off the old one must not outlive it.
        self.validate_selection();
    }

    /// Keeps the selection consistent with the current plate: indices in range, the selected
    /// volume or height range on the primary object, the cut tool on it.
    pub(crate) fn validate_selection(&mut self) {
        let objects = &self.plates.current().objects;
        let len = objects.len();
        self.selected = self.selected.filter(|&i| i < len);
        let primary = self.selected;
        self.multi.retain(|&i| i < len && Some(i) != primary);
        if let Some(sub) = self.sub {
            let exists = primary
                .and_then(|i| objects.get(i))
                .is_some_and(|o| match sub {
                    scene::Sub::Volume(id) => o.volume(id).is_some(),
                    scene::Sub::Range(id) => o.range(id).is_some(),
                });
            if !exists {
                self.sub = None;
            }
        }
        self.validate_cut();
        self.validate_mesh_tool();
    }

    /// Switches to plate `i` and brings it into view.
    /// A pick from the plate menu, for plate `i`.
    pub(crate) fn plate_command(&mut self, i: usize, c: crate::plate_menu::PlateCommand) {
        use crate::plate_menu::PlateCommand as C;
        if i >= self.plates.len() {
            return;
        }
        let busy = self.loading.is_some();
        match c {
            C::Rename => {
                self.select_plate(i);
                let name = self.plates.plate(i).name.clone();
                self.viewport.start_plate_rename(i, &name);
            }
            C::Settings => {
                self.select_plate(i);
                self.editor.plate_mode = true;
                self.editor.object_mode = false;
            }
            C::ArrangeObjects if !busy => {
                self.select_plate(i);
                if self.plates.plate(i).locked {
                    self.status = "This plate is locked; unlock it from the plate menu".into();
                } else {
                    self.arrange();
                }
            }
            C::Orient if !busy => {
                self.select_plate(i);
                if !self.plates.plate(i).locked {
                    let all: Vec<usize> = (0..self.plates.current().objects.len()).collect();
                    self.select_many(&all, false);
                    self.auto_orient();
                    self.select(None, false);
                }
            }
            C::SelectAll => {
                self.select_plate(i);
                let all: Vec<usize> = (0..self.plates.current().objects.len()).collect();
                self.select_many(&all, false);
            }
            C::Lock if !busy => {
                self.record("Lock plate");
                let p = &mut self.plates.plates[i];
                p.locked = !p.locked;
                self.status = format!(
                    "{} {}",
                    self.plates.plate(i).name,
                    if self.plates.plate(i).locked {
                        "locked: its layout stays as it is"
                    } else {
                        "unlocked"
                    }
                );
            }
            C::Duplicate if !busy => {
                self.record("Duplicate plate");
                let next = self.next_object_id();
                let n = self.plates.duplicate(i, next);
                self.tower_cache.clear();
                self.selected = None;
                self.multi.clear();
                self.tower_selected = false;
                self.changed();
                self.fit_current_plate(false);
                self.status = format!("Duplicated as {}", self.plates.plate(n).name);
            }
            C::MoveLeft if !busy && i > 0 => self.move_plate(i, i - 1),
            C::MoveRight if !busy => self.move_plate(i, i + 1),
            C::Reorder => self.viewport.open_plate_list = true,
            C::Delete if !busy => {
                self.record("Delete plate");
                self.plates.remove(i);
                self.drop_orphan_jobs();
                self.tower_cache.clear();
                self.selected = None;
                self.changed();
            }
            _ => {}
        }
    }

    /// Names plate `i` (empty: `Plate N` again), as one undo step.
    pub(crate) fn rename_plate(&mut self, i: usize, name: &str) {
        if i >= self.plates.len() {
            return;
        }
        let before = self.snapshot();
        if self.plates.rename(i, name) {
            self.undo.push("Rename plate", before);
            self.trim_undo();
            self.status = format!("Renamed plate {} to {}", i + 1, self.plates.plate(i).name);
        }
    }

    /// Moves plate `from` to position `to` with its objects, settings and slice, as one undo
    /// step. Nothing is re-sliced: a plate's G-code does not depend on its position.
    pub(crate) fn move_plate(&mut self, from: usize, to: usize) {
        let before = self.snapshot();
        if self.plates.move_plate(from, to) {
            self.undo.push("Move plate", before);
            self.trim_undo();
            // Cached towers are kept by position.
            self.tower_cache.clear();
            self.refresh_warnings();
            self.fit_current_plate(false);
            self.status = format!(
                "Moved {} to position {}",
                self.plates.plate(to).name,
                to + 1
            );
        }
    }

    pub(crate) fn select_plate(&mut self, i: usize) {
        if i < self.plates.len() && i != self.plates.current {
            self.plates.current = i;
            self.selected = None;
            self.tower_selected = false;
            self.editor.object_mode = false;
            self.editor.tower_mode = false;
            self.preview = self.preview && self.finished().is_some();
            if let Some(f) = self.finished() {
                self.view = Self::view_for(f);
            }
            self.refresh_warnings();
            self.fit_current_plate(false);
        }
    }

    /// The whole window. Shared by the eframe app and the headless screenshot.
    pub fn show(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        // Opaque window background, whatever the theme (panels only cover their own areas).
        ui.painter()
            .rect_filled(ui.max_rect(), 0., theme::of(ui).bg);
        self.follow_system_theme(&ctx);
        self.sync_view_prefs();
        self.poll(&ctx);
        self.poll_mcp(&ctx);
        self.poll_updater(&ctx);
        self.library_frame(&ctx);
        self.shortcuts(&ctx);
        let dropped = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .collect::<Vec<_>>()
        });
        if !dropped.is_empty() {
            self.load(dropped, &ctx);
        }
        if self.status != self.last_status {
            self.last_status = self.status.clone();
            self.status_at = Instant::now();
        }
        self.sync_notifications();
        self.topbar(ui);
        // The library scans in the background, but waits while Mica slices.
        self.library.set_paused(self.worker.is_some());
        // No camera streams run outside the Machine view. The Library covers it too (it can
        // open over the Machine tab), and nothing would stop its players while it shows.
        if (self.library.shown || !self.machine) && !self.machine_view.cameras.is_empty() {
            self.machine_view.cameras.clear();
        }
        if self.library.shown {
            self.library_view(ui);
            self.dialogs(&ctx);
            self.window_notifications(&ctx);
            self.save_prefs_throttled();
            return;
        }
        if self.machine {
            // The Machine view replaces the sidebar and the 3D view.
            self.machine_view(ui);
            self.dialogs(&ctx);
            self.window_notifications(&ctx);
            self.save_prefs_throttled();
            return;
        }
        // The sidebar, on the right by default (View › Sidebar on the left / right): Prepare's
        // cards, or in Preview the topic rail with the Preview page on top
        // (`preview_side.rs`). Both share the panel, so its width and collapse carry over.
        self.sync_preview_side();
        let previewing = self.previewing();
        if !self.keys.sidebar_hidden {
            let width = self.prefs.window.sidebar_width;
            let p = theme::of(ui);
            let panel = if self.prefs.sidebar_right {
                egui::Panel::right("sidebar")
            } else {
                egui::Panel::left("sidebar")
            };
            let r = panel
                .default_size(width)
                .size_range(320.0..=560.0)
                .frame(
                    egui::Frame::NONE
                        .fill(p.panel)
                        .stroke(egui::Stroke::new(1., p.border)),
                )
                .show(ui, |ui| {
                    if previewing {
                        self.preview_sidebar(ui);
                    } else {
                        self.sidebar(ui);
                    }
                });
            let new_width = r.response.rect.width();
            if (new_width - width).abs() > 1. {
                self.prefs.window.sidebar_width = new_width;
                self.prefs_dirty = true;
            }
        }
        self.viewport(ui);
        self.mcp_indicator(&ctx);
        self.preview_side.probe.chip = None;
        if previewing && self.finished().is_some() && self.keys.sidebar_hidden {
            self.preview_legend_chip(&ctx);
        }
        if !self.preview {
            self.breadcrumb(&ctx);
            if self.selected.is_some() {
                self.object_window(&ctx);
            } else if let Some(kind) = self.viewport.paint {
                // A painting tool left open with nothing selected says what it needs.
                self.paint_pick_card(&ctx, kind);
            } else if self.tower_selected && self.tower().is_some() {
                self.tower_window(&ctx);
            } else if self.plates.current().calibration.is_some() {
                self.calibration_window(&ctx);
            }
        } else if self.plates.current().calibration.is_some() && self.gcode.is_none() {
            self.calibration_window(&ctx);
        }
        if self.gcode.is_some() && self.previewing() {
            self.gcode_chip(&ctx);
        }
        self.dialogs(&ctx);
        // The 3D view's corner, above the totals card (Prepare) or the legend's foot.
        let r = self.viewport_rect;
        let corner = egui::pos2(
            r.right() - 14.,
            r.bottom() - if self.preview { 40. } else { 74. },
        );
        self.notifications_ui(&ctx, corner, true);
        self.sync_recent_colours(&ctx);
        // A gesture driven by a field (slider drag, typed value) ends when the pointer is up
        // and nothing in the 3D view is being dragged; gizmo drags end through `Edited`.
        if self.undo.in_gesture()
            && !self.viewport.dragging()
            && !ctx.input(|i| i.pointer.any_down())
        {
            self.end_gesture();
        }
        self.save_prefs_throttled();
    }

    /// The notifications outside the plate views (Machine, Library): global ones only, in the
    /// window's bottom-right corner.
    fn window_notifications(&mut self, ctx: &egui::Context) {
        let r = ctx.content_rect();
        self.notifications_ui(ctx, r.right_bottom() - egui::vec2(16., 16.), false);
    }

    /// Preferences (and the printer list) are written at most once a second, and on exit.
    fn save_prefs_throttled(&mut self) {
        if (self.prefs_dirty || self.printers.dirty)
            && self
                .prefs_saved_at
                .is_none_or(|t| t.elapsed() > Duration::from_secs(1))
        {
            self.save_prefs();
            self.prefs_saved_at = Some(Instant::now());
        }
    }

    /// The file stem exports and uploads are named after: the first object (and the plate,
    /// with several plates).
    pub(crate) fn export_stem(&self) -> String {
        self.export_stem_of(self.plates.current)
    }

    /// [`Self::export_stem`] of plate `i` (`mica slice --each-plate` names its files the same
    /// way: `mica_core::export_name`).
    pub(crate) fn export_stem_of(&self, i: usize) -> String {
        let plate = self.plates.plate(i);
        mica_core::export_name::stem(
            plate.objects.first().map(|o| o.name.as_str()),
            &plate.name,
            self.plates.len(),
        )
    }
}

/// The overlays applied to the presets' configuration `c`, as the app slices with it;
/// returns the keys that failed to parse. `identity` is the linked Moonraker printer's G-code
/// file header (`mica_gcode_identity`), applied last like the CLI's `--gcode-identity`.
fn compose(
    c: &mut Config,
    overlays: &Overlays,
    prusa_profile: bool,
    identity: Option<&str>,
) -> Vec<String> {
    let mut invalid = overlays.apply(c);
    if let Some(id) = identity
        && c.set_raw("mica_gcode_identity", id).is_err()
    {
        invalid.push("mica_gcode_identity".into());
    }
    // A PrusaSlicer profile sliced like OrcaSlicer: the Orca-only settings from Orca's
    // matching profile or Orca's defaults, as the CLI does (`mica_cli` slice_job).
    if prusa_profile && mica_core::rules::Rules::of(c).is_orca() {
        mica_profiles::rules_fill::fill_orca_rules(c, None);
    }
    invalid
}

/// Whether every one of `keys` has the same value in `a` and `b`: a whole vector for per-tool
/// keys, a merged setting as either slicer reads it (the Orca key falls back to its
/// translated counterpart, which a PrusaSlicer profile alone has). A key neither knows
/// counts as different, so it is never dropped.
fn unchanged(a: &Config, b: &Config, keys: &[String]) -> bool {
    use mica_core::merged;
    fn both<T: PartialEq>(x: Option<T>, y: Option<T>) -> bool {
        x.is_some() && x == y
    }
    keys.iter().all(|k| {
        if merged::by_orca(k).is_some() {
            both(merged::orca_value(a, k), merged::orca_value(b, k))
        } else if merged::by_prusa(k).is_some() {
            both(merged::prusa_value(a, k), merged::prusa_value(b, k))
        } else {
            match (a.get(k), b.get(k)) {
                (Some(x), Some(y)) => x == y,
                (None, None) => both(a.raw(k), b.raw(k)),
                _ => false,
            }
        }
    })
}

/// Whether the overrides of `keys` (what one set wrote) change nothing: `parent` (the
/// configuration the overrides lie on) with all `overrides` is the same as without them.
fn overrides_unchanged(
    parent: &Config,
    overrides: &BTreeMap<String, String>,
    keys: &[String],
) -> bool {
    let apply = |skip: bool| {
        let mut c = parent.clone();
        for (k, v) in overrides {
            let ours = keys.contains(k);
            if !(skip && ours) && c.set_raw(k, v).is_err() && ours {
                return None;
            }
        }
        Some(c)
    };
    match (apply(false), apply(true)) {
        (Some(with), Some(without)) => unchanged(&with, &without, keys),
        _ => false,
    }
}

/// The Essentials label of a key in lower case (`infill density`), else Orca's label.
fn plain_label(key: &str) -> String {
    crate::settings::ESSENTIALS_CURATED
        .iter()
        .find(|(k, _)| *k == key)
        .map_or_else(|| crate::settings::label(key), |(_, l)| l.to_string())
        .to_lowercase()
}

/// `infill density 20 %`, `layer height 0.2 mm`, `ironing on`: the text after "after …".
pub fn change_text(key: &str, raw: &str) -> String {
    let unit = crate::settings::text()
        .get(key)
        .map(|t| crate::panels::editor::unit_for(key, &t.unit))
        .unwrap_or("");
    let value = match raw.trim() {
        "1" if unit.is_empty() => "on".to_string(),
        "0" if unit.is_empty() => "off".to_string(),
        v if v.ends_with('%') => format!("{} %", v.trim_end_matches('%').trim()),
        v if v.parse::<f64>().is_ok() && !unit.is_empty() => format!("{v} {unit}"),
        v => crate::settings::text()
            .get(key)
            .and_then(|t| t.enums.iter().find(|(val, _)| val == v))
            .map_or(v.to_string(), |(_, l)| l.to_lowercase()),
    };
    format!("{} → {value}", plain_label(key))
}

/// Loads model files and places them around `center`: STLs on the bed, a project's plates
/// each in their own bed coordinates. Runs off the UI thread.
pub fn load_files(
    paths: Vec<PathBuf>,
    config: &Result<Config, String>,
    center: Vec3,
) -> LoadResult {
    let config = config.clone()?;
    mica_core::input::load_with_plate(
        &paths,
        (center.x, center.y),
        false,
        true,
        0,
        &Default::default(),
        &config,
        &mica_profiles::project::Mapper {
            project_tools: true,
        },
        mica_core::input::PlateSelection::Each,
    )
    .map(|loaded| {
        let mut warnings = loaded.warnings;
        // Broken meshes are repaired on import; clean ones are left exactly as they are.
        let objects = crate::mesh_tools::import_objects(loaded.objects, &mut warnings);
        LoadedScene {
            plates: loaded
                .plates
                .iter()
                .enumerate()
                .map(|(i, p)| crate::plates::Plate::imported(i, p))
                .collect(),
            objects,
            warnings,
            tower: loaded.tower_placement,
            project: crate::save::is_project(&paths),
            paths,
            settings: loaded
                .config
                .as_ref()
                .and_then(mica_profiles::project::ProjectSettings::parse)
                .map(|mut s| {
                    s.mica = loaded.mica_settings.clone();
                    s
                }),
            config: loaded.config,
            custom_gcode: loaded.custom_gcode,
            mixes: loaded.full_spectrum,
        }
    })
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        if let Some(state) = frame.wgpu_render_state()
            && let Some(renderer) = state
                .renderer
                .read()
                .callback_resources
                .get::<render::Renderer>()
            && let Some(error) = &renderer.error
        {
            self.notify_error(
                "Preview read failed",
                format!("Preview read failed: {error}"),
            );
        }
        let ctx = ui.ctx().clone();
        if let Some(rect) = ctx.input(|i| i.viewport().inner_rect)
            && ((rect.width() - self.prefs.window.width).abs() > 1.
                || (rect.height() - self.prefs.window.height).abs() > 1.)
        {
            self.prefs.window.width = rect.width();
            self.prefs.window.height = rect.height();
            self.prefs_dirty = true;
        }
        // Menu bar clicks first, as if they were this frame's key presses.
        self.native_menu_events(&ctx);
        self.show(ui);
        self.native_menu_refresh(&ctx);
        let title = self.window_title();
        if title != self.last_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.last_title = title;
        }
        if ctx.input(|i| i.viewport().close_requested()) {
            if self.is_modified() && !self.closing {
                // Ask first; `proceed` closes the window once the answer allows it.
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.dialogs.unsaved = Some(crate::save::Pending::Close);
            } else {
                self.save_prefs();
            }
        }
    }

    fn on_exit(&mut self) {
        self.mcp.shutdown();
        self.save_prefs();
    }
}

/// Display colour of a filament (Orca's `adjust_color_for_rendering`): a colour whose every
/// channel is below 0.2 is drawn lighter so its shading stays visible. Orca replaces it with flat
/// 0.2 grey; Mica raises all channels by the same amount until the brightest reaches 0.2, so a
/// dark navy stays navy.
pub(crate) fn display_colour(c: [f32; 3]) -> [f32; 3] {
    const FLOOR: f32 = 0.2;
    let max = c[0].max(c[1]).max(c[2]);
    if max >= FLOOR {
        return c;
    }
    let lift = FLOOR - max;
    c.map(|v| v + lift)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headless_app(ctx: &egui::Context, model: &str) -> App {
        let dir = std::env::temp_dir().join(format!(
            "mica-app-test-{}-{}",
            std::process::id(),
            NEXT_TEST_STORE.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        App::create(
            ctx,
            Options {
                prefs_path: None,
                store: Some(Store::open(dir)),
                files: vec![
                    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/models/"))
                        .join(model),
                ],
                headless: true,
            },
        )
    }
    static NEXT_TEST_STORE: AtomicU64 = AtomicU64::new(0);

    /// One paint stroke (gesture begin, frames, release) is one undo entry; undo takes the
    /// paint away and redo brings it back; Clear all is another entry.
    #[test]
    fn paint_strokes_are_single_undo_entries() {
        use crate::viewport::Action;
        use mica_core::painting::PaintKind;
        let ctx = egui::Context::default();
        let mut app = headless_app(&ctx, "cube20.stl");
        app.selected = Some(0);
        app.viewport.paint = Some(PaintKind::Supports);
        app.paint.brush = crate::paint::BrushKind::Sphere;
        app.paint.set_radius(4.);
        let o = app.plates.current().objects[0].clone();
        let (min, max) = o.bounds();
        let camera = Vec3::new(max.x + 100., (min.y + max.y) / 2., 10.);
        let hit = scene::picking::pick_face(std::slice::from_ref(&o), camera, Vec3::NEG_X).unwrap();
        let stroke = |z: f32| Action::Paint {
            index: 0,
            hit: crate::paint::Hit {
                face: hit.face,
                point: Vec3::new(max.x, (min.y + max.y) / 2., z),
                camera,
            },
            trail: vec![],
            erase: false,
        };
        let map = [(0usize, 0usize)];
        let undo_before = app.undo.can_undo();
        app.handle(
            Action::GestureBegin(crate::paint::tool_name(PaintKind::Supports)),
            &map,
            &ctx,
        );
        app.handle(stroke(10.), &map, &ctx);
        app.handle(stroke(11.), &map, &ctx);
        app.handle(stroke(12.), &map, &ctx);
        app.handle(Action::Edited, &map, &ctx);
        let painted = app.plates.current().objects[0].painting.clone();
        assert!(!painted.supports.is_empty());
        assert!(app.plates.current().objects[0].painting_edited);
        assert_eq!(app.undo.undo_label(), Some("Support painting"));
        app.undo();
        assert!(app.plates.current().objects[0].painting.is_empty());
        assert_eq!(app.undo.can_undo(), undo_before);
        app.redo();
        assert_eq!(app.plates.current().objects[0].painting, painted);
        // The session follows the undo/redo (it is rebuilt when stale) and Clear all is one step.
        app.clear_painting();
        assert!(app.plates.current().objects[0].painting.is_empty());
        assert_eq!(app.undo.undo_label(), Some("Clear painting"));
        app.undo();
        assert_eq!(app.plates.current().objects[0].painting, painted);
    }

    /// Four quick Slice presses: the first starts a job, the others cancel it and coalesce
    /// into one follow-up job, which answers the last request.
    #[test]
    fn slice_requests_coalesce_into_one_follow_up_job() {
        let ctx = egui::Context::default();
        let mut app = headless_app(&ctx, "cube20.stl");
        assert_eq!(app.plates.current().objects.len(), 1);
        assert!(app.config.is_ok(), "{:?}", app.config.as_ref().err());
        let before = worker::jobs_started();
        app.slice_current(&ctx);
        assert!(app.worker.is_some());
        for _ in 0..3 {
            app.slice_current(&ctx);
        }
        // The running job was cancelled and is winding down; one request waits.
        assert!(app.worker.is_none());
        assert!(app.stopping.is_some());
        assert_eq!(app.pending_slice, Some((app.plates.plate(0).id, true, 4)));
        assert!(app.busy());
        assert_eq!(worker::jobs_started() - before, 1);
        assert_eq!(app.slice_requests, 4);
        let deadline = Instant::now() + Duration::from_secs(180);
        while (app.worker.is_some() || app.stopping.is_some() || app.pending_slice.is_some())
            && Instant::now() < deadline
        {
            app.poll(&ctx);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(!app.busy(), "the follow-up job did not finish in time");
        assert_eq!(worker::jobs_started() - before, 2, "more than two jobs ran");
        assert!(app.finished().is_some(), "{}", app.status);
        assert_eq!(app.finished_request, Some(4));
        assert_eq!(app.pending_slice, None);
        // A cancel while the follow-up stops leaves no job behind.
        app.slice_current(&ctx);
        app.slice_current(&ctx);
        app.cancel_slice();
        assert!(app.worker.is_none() && app.pending_slice.is_none());
        let deadline = Instant::now() + Duration::from_secs(60);
        while app.stopping.is_some() && Instant::now() < deadline {
            app.poll(&ctx);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(!app.busy());
        assert_eq!(worker::jobs_started() - before, 3);
    }

    /// An app without files, on the CORE One (one tool), so a project for the U1 is offered
    /// rather than used.
    fn core_one_app(ctx: &egui::Context) -> App {
        let dir = std::env::temp_dir().join(format!(
            "mica-app-test-{}-{}",
            std::process::id(),
            NEXT_TEST_STORE.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let mut app = App::create(
            ctx,
            Options {
                prefs_path: None,
                store: Some(Store::open(dir)),
                files: vec![],
                headless: true,
            },
        );
        let (vi, _, name) = app
            .profiles
            .all_printers()
            .into_iter()
            .find(|(_, _, n)| n == "Prusa CORE One HF0.4 nozzle")
            .expect("the Prusa bundle has the CORE One");
        app.profiles.select_printer(vi, &name);
        app.settings_changed();
        app
    }

    fn fixture() -> Vec<PathBuf> {
        vec![
            PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/models/"))
                .join("plates-orca.3mf"),
        ]
    }

    /// The synthetic Orca project (`scripts/make-plates-fixture.py`) opens as its plates:
    /// objects on the plate they belong to, in plate-local coordinates, the plate names and
    /// locks, the per-object tools, and the loose object on the nearest plate.
    #[test]
    fn project_plates_import_with_their_objects_names_and_locks() {
        let ctx = egui::Context::default();
        let mut app = core_one_app(&ctx);
        let result = load_files(fixture(), &app.config, app.bed.center());
        app.finish_load(result);
        assert_eq!(app.plates.len(), 2, "{}", app.status);
        assert_eq!(app.plates.current, 0);
        assert_eq!(app.plates.plate(0).name, "Plate 1");
        assert_eq!(app.plates.plate(1).name, "Lid");
        assert!(!app.plates.plate(0).locked && app.plates.plate(1).locked);
        let names = |i: usize| -> Vec<String> {
            app.plates
                .plate(i)
                .objects
                .iter()
                .map(|o| o.name.clone())
                .collect()
        };
        assert_eq!(names(0), ["painted", "second"]);
        assert_eq!(names(1), ["lid", "loose"]);
        // Plate 2 sits one grid stride (270 mm × 1.2) to the right in the archive; its cube
        // comes back at the bed-local 100 mm. The loose object keeps its off-bed position.
        let lid = &app.plates.plate(1).objects[0];
        let (min, _) = lid.bounds();
        assert!(
            (min.x - 100.).abs() < 1e-3 && (min.y - 100.).abs() < 1e-3,
            "{min:?}"
        );
        let loose = &app.plates.plate(1).objects[1];
        assert!(loose.bounds().0.y > 270.);
        assert_eq!(app.plates.plate(0).objects[0].tools.0[0], Some(0));
        assert_eq!(app.plates.plate(0).objects[1].tools.0[0], Some(1));
        assert_eq!(lid.tools.0[0], Some(2));
        // Orca's per-plate tower position becomes the plate's placement hook.
        assert_eq!(app.plates.plate(0).tower_placement, Some([165., 233., 0.]));
        assert!(app.plates.plate(1).metadata.contains_key("filament_maps"));
        // A locked plate refuses layout edits.
        app.select_plate(1);
        app.arrange();
        assert!(app.status.contains("locked"));
        // Undo takes the whole import back.
        app.undo();
        assert_eq!(app.plates.len(), 1);
        assert!(app.plates.is_blank());
    }

    /// The project's filament colours apply on open even when its settings are only offered
    /// (another printer); the painted facets map to those colours; "Use project settings"
    /// puts its changes in the overlays and "Revert" takes them out, colours staying.
    #[test]
    fn project_colours_apply_and_settings_are_offered_used_and_reverted() {
        use crate::project::State;
        let ctx = egui::Context::default();
        let mut app = core_one_app(&ctx);
        let result = load_files(fixture(), &app.config, app.bed.center());
        app.finish_load(result);
        let prompt = app.project.as_ref().expect("the project has settings");
        // The CORE One is not the project's printer: offered, not used.
        assert_eq!(prompt.state, State::Pending);
        assert!(
            prompt
                .summary()
                .contains("process 0.20 Standard (+1 change)")
        );
        assert!(prompt.summary().contains("4 filaments"));
        // Colours already apply (the printer has one tool, so one colour here).
        let colours = app.tool_colours();
        assert_eq!(colours[0], [0xCB, 0x8F, 0x29]);
        assert!(app.overlays.print.is_empty());
        // Painted facets of the first object are tools 2 and 3: with four project colours
        // they render black and white.
        let painted = app.plates.plate(0).objects[0].clone();
        let project_colours: Vec<[f32; 3]> =
            [[0xCB, 0x8F, 0x29], [0, 0, 0], [255, 255, 255], [0xFB, 2, 7]]
                .iter()
                .map(|c| c.map(|v| v as f32 / 255.))
                .collect();
        let vertices = scene::paint::overlay_vertices(
            &painted.mesh,
            &painted.painting,
            scene::paint::Show::ALL,
            &project_colours,
        );
        assert_eq!(
            vertices.len(),
            12,
            "two faces per tool, three vertices each"
        );
        assert_eq!(
            vertices
                .iter()
                .filter(|v| v.color == [0., 0., 0., 1.])
                .count(),
            6
        );
        assert_eq!(
            vertices
                .iter()
                .filter(|v| v.color == [1., 1., 1., 1.])
                .count(),
            6
        );
        // Keep mine: the card goes, the colour stays.
        app.keep_my_settings();
        assert_eq!(app.project.as_ref().unwrap().state, State::Dismissed);
        assert_eq!(app.tool_colours()[0], [0xCB, 0x8F, 0x29]);
        // Use: the project's values are sliced with (whatever printer matched), as the
        // project's own: no edits, no changed markers.
        let before = app.profiles.printer.clone();
        app.use_project_settings();
        assert!(matches!(
            app.project.as_ref().unwrap().state,
            State::Applied { .. }
        ));
        assert!(app.overlays.print.is_empty());
        assert_eq!(app.config.as_ref().unwrap().float("fill_density"), Some(8.));
        assert!(
            app.project_differences
                .iter()
                .any(|d| d.key == "fill_density" && d.project == "8%")
        );
        assert_eq!(
            app.project.as_ref().unwrap().changes,
            app.project_differences.len()
        );
        // Undo and redo the use; then "Update to current profile" keeps the listed change.
        app.undo();
        assert!(app.overlays.project.is_empty());
        app.redo();
        assert_eq!(app.config.as_ref().unwrap().float("fill_density"), Some(8.));
        app.update_project_to_current();
        assert!(app.project.as_ref().unwrap().updated);
        assert_eq!(app.config.as_ref().unwrap().float("fill_density"), Some(8.));
        assert!(app.overlays.print.is_empty());
        app.undo();
        assert_eq!(app.config.as_ref().unwrap().float("fill_density"), Some(8.));
        assert_eq!(app.tool_colours()[0], [0xCB, 0x8F, 0x29]);
        if app.tool_count() >= 4 {
            assert_eq!(app.tool_colours()[1], [0, 0, 0]);
            assert_eq!(app.tool_colours()[3], [0xFB, 2, 7]);
        }
        // Revert: back to the printer and clean overlays, colours kept.
        app.revert_project_settings();
        assert_eq!(app.project.as_ref().unwrap().state, State::Dismissed);
        assert_eq!(app.profiles.printer, before);
        assert!(app.overlays.print.is_empty());
        assert_eq!(app.tool_colours()[0], [0xCB, 0x8F, 0x29]);
    }

    /// Paint visibility: colour painting always; the active tool full strength on its
    /// object; support and fuzzy on the selected object and any kind with its View toggle,
    /// both subtle; seam only with its tool or toggle.
    #[test]
    fn paint_visibility_rules() {
        use mica_core::painting::PaintKind;
        let ctx = egui::Context::default();
        let app = core_one_app(&ctx);
        let colour_only = app.paint_show(1, false, None);
        assert_eq!(colour_only, scene::paint::Show::COLOUR_ONLY);
        let selected = app.paint_show(1, true, None);
        assert_eq!(
            selected.strength(PaintKind::Supports),
            scene::paint::Show::SUBTLE
        );
        assert_eq!(
            selected.strength(PaintKind::Fuzzy),
            scene::paint::Show::SUBTLE
        );
        assert_eq!(selected.strength(PaintKind::Seam), 0.);
        assert_eq!(selected.strength(PaintKind::Material), 1.);
        let active = app.paint_show(1, true, Some((1, PaintKind::Fuzzy)));
        assert_eq!(active.strength(PaintKind::Fuzzy), 1.);
        let other = app.paint_show(2, false, Some((1, PaintKind::Fuzzy)));
        assert_eq!(other.strength(PaintKind::Fuzzy), 0.);
        let mut toggled = app;
        toggled.show_paint.set(PaintKind::Seam, 1.);
        assert_eq!(
            toggled.paint_show(2, false, None).strength(PaintKind::Seam),
            scene::paint::Show::SUBTLE
        );
    }

    /// File › New project: asks about unsaved changes first; then one empty plate, no undo
    /// history, no edits, and the current printer's default print and filament profiles (or
    /// the ones Settings › Slicing names, when the printer offers them).
    #[test]
    fn new_project_asks_then_starts_over_with_the_printers_defaults() {
        use crate::save::Pending;
        let ctx = egui::Context::default();
        let mut app = core_one_app(&ctx);
        let printer = app.profiles.printer.clone();
        let default = app
            .profiles
            .processes
            .iter()
            .find(|p| p.contains("0.20") && (p.contains("SPEED") || p.contains("Standard")))
            .cloned()
            .expect("a 0.20 mm default");
        let other = app
            .profiles
            .processes
            .iter()
            .find(|p| **p != default)
            .cloned()
            .expect("a second print profile");
        let model = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/models/"))
            .join("cube20.stl");
        let loaded = load_files(vec![model], &app.config, app.bed.center());
        app.finish_load(loaded);
        app.profiles.process = other.clone();
        app.profiles.refresh();
        app.overlays.print.insert("perimeters".into(), "4".into());
        app.settings_changed();
        app.record("Add plate");
        app.plates.add();
        app.changed();
        assert!(app.is_modified());
        assert!(app.undo.can_undo());
        // Unsaved changes: nothing happens until the prompt is answered.
        app.new_project();
        assert_eq!(app.dialogs.unsaved, Some(Pending::New));
        assert_eq!(app.plates.len(), 2);
        // "Don't save".
        app.dialogs.unsaved = None;
        app.proceed(Pending::New, &ctx);
        assert_eq!(app.plates.len(), 1);
        assert!(app.plates.is_blank());
        assert!(!app.undo.can_undo() && !app.undo.can_redo());
        assert_eq!(app.overlays, Overlays::new(app.profiles.nozzle_count()));
        assert_eq!(app.profiles.printer, printer);
        assert_eq!(app.profiles.process, default);
        assert!(app.selected.is_none());
        assert!(app.document.path.is_none());
        assert!(!app.is_modified());
        assert_eq!(app.window_title(), "Untitled — Mica Slicer");
        // Nothing to lose: no prompt. A print profile chosen in Preferences is used...
        app.prefs.defaults.print = other.clone();
        app.new_project();
        assert!(app.dialogs.unsaved.is_none());
        assert_eq!(app.profiles.process, other);
        assert!(!app.is_modified());
        // ...unless the printer does not offer it.
        app.prefs.defaults.print = "No such profile".into();
        app.new_project();
        assert_eq!(app.profiles.process, default);
    }

    /// The Machine view and an open Print dialog remember their printer by id: removing
    /// another printer in My printers (which shifts the list) must not point them at a
    /// different one, and removing theirs must not either.
    #[test]
    fn removing_a_printer_keeps_the_machine_view_and_print_dialog_on_theirs() {
        use crate::printhost::config::{Kind, Printer};
        let printer = |name: &str, port: u16| {
            let mut p = Printer::new(Kind::PrusaLink);
            p.name = name.into();
            p.address = format!("127.0.0.1:{port}");
            p
        };
        let ctx = egui::Context::default();
        let mut app = core_one_app(&ctx);
        for (name, port) in [("A", 1), ("B", 2), ("C", 3)] {
            app.printers.put(None, printer(name, port), None).unwrap();
        }
        let id = |app: &App, name: &str| app.printers.list[app.printers.index_of(name).unwrap()].id;
        let name_of = |app: &App, i: Option<usize>| i.map(|i| app.printers.list[i].name.clone());
        let (b, c) = (id(&app, "B"), id(&app, "C"));
        // The Machine view shows C (the third), the Print dialog targets B (the second).
        app.machine_printer = Some(c);
        let file = crate::printhost::job::JobFile {
            path: PathBuf::from("/nonexistent/cube.bgcode"),
            format: mica_printhost::FileFormat::Bgcode,
            tools: vec![],
            bed_type: None,
            hold: None,
        };
        let mut dialog = crate::printhost::job::Dialog::new(file, "cube".into());
        dialog.choose(app.printers.target(1));
        app.print_job = Some(dialog);
        // Removing A moves B and C up one place; both keep their printer.
        app.printers.remove(0);
        app.poll_printers(&ctx);
        assert_eq!(name_of(&app, app.machine_index()), Some("C".into()));
        let target = app.print_job.as_ref().unwrap().target.clone().unwrap();
        assert_eq!(target.id, b);
        assert_eq!(
            name_of(&app, app.printers.position(target.id)),
            Some("B".into())
        );
        // Editing (renaming) a printer keeps its identity.
        let i = app.printers.index_of("C").unwrap();
        let mut renamed = app.printers.list[i].clone();
        renamed.name = "C2".into();
        app.printers.put(Some(i), renamed, None).unwrap();
        assert_eq!(app.machine_printer, Some(c));
        assert_eq!(name_of(&app, app.machine_index()), Some("C2".into()));
        // Removing the dialog's printer: it asks for a printer again rather than sending to
        // the one that moved into its place.
        app.printers.remove(app.printers.index_of("B").unwrap());
        app.poll_printers(&ctx);
        assert!(app.print_job.as_ref().unwrap().target.is_none());
        assert_eq!(name_of(&app, app.machine_index()), Some("C2".into()));
        // Removing the Machine view's printer leaves it without one (the view then shows the
        // Printers dashboard), never on another printer's slot.
        app.printers.remove(app.printers.index_of("C2").unwrap());
        assert_eq!(app.machine_index(), None);
        app.poll_printers(&ctx);
        assert_eq!(app.machine_printer, None);
        // A new printer never reuses a removed printer's id.
        let i = app.printers.put(None, printer("D", 4), None).unwrap();
        assert!(app.printers.list[i].id > c);
        assert_eq!(app.machine_index(), None);
    }

    #[test]
    fn near_black_filament_is_drawn_lighter_but_keeps_its_hue() {
        assert_eq!(super::display_colour([0., 0., 0.]), [0.2, 0.2, 0.2]);
        let navy = super::display_colour([0.02, 0.03, 0.12]);
        assert!((navy[2] - 0.2).abs() < 1e-6 && navy[0] < navy[2]);
        let orange = [0.9, 0.5, 0.1];
        assert_eq!(super::display_colour(orange), orange);
    }
}
