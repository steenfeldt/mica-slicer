// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! "Add a printer": three screens in one window.
//!
//! 1. **Found on your network** — printers that answered an mDNS / DNS-SD search
//!    (`printhost::discover`, started when the window opens and by Refresh), each matched to a
//!    profile (`printhost::catalog`). "Add this printer" adds the matching profile and the
//!    connection at once; a printer that wants its API key goes to Setup to ask for it.
//!    "Enter an address" looks up one address the same way.
//! 2. **Pick a model** — one card per printer model (search, brand chips).
//! 3. **Set it up** — tools, nozzle (only combinations that exist), input shaper where the
//!    model comes with and without, the name, and an optional connection with Test connection
//!    and the camera.
//!
//! Adding writes what the old flow wrote: the profile goes into `my_printers` (mica.toml) and
//! becomes the current printer; a connection becomes a `printers.toml` entry linked to that
//! profile, its key in the Keychain.

use super::widgets::{self, Glyph};
use crate::app::App;
use crate::printhost::{
    Test,
    cameras::{self, CameraViews, Role},
    catalog::{self, Catalog, Choice, Model},
    config::{Kind, Printer},
    discover::{Discovery, Search},
    explain::Explanation,
};
use crate::theme;
use eframe::egui::{self, Align2, Color32, Pos2, Rect, Sense, Stroke, Vec2};
use mica_printhost::{PrinterState, discovery::Discovered};
use std::{collections::HashMap, sync::mpsc};

pub const SIZE: Vec2 = Vec2::new(960., 700.);
const PAD: f32 = 28.;
const GAP: f32 = 16.;
/// Setup: a field's label above it, the segmented buttons, the gap between rows.
const LABEL: f32 = 20.;
const SEG_H: f32 = 36.;
const ROW_GAP: f32 = 18.;
/// Setup's connection box.
const CONNECT_H: f32 = 244.;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Screen {
    #[default]
    Found,
    Gallery,
    Setup,
}

/// The connection part of Setup.
#[derive(Default)]
pub struct Connect {
    pub kind: Option<Kind>,
    pub address: String,
    pub secret: String,
    /// What the printer reported when it was found, for profile matching later.
    pub model: Option<String>,
    /// The printer asked for its key when it was found.
    pub needs_key: bool,
    pub test: Option<Test>,
    /// A camera URL typed by hand (Prusa printers), and whether its snapshot is shown.
    pub camera: String,
    pub camera_shown: bool,
}

pub struct Setup {
    pub model: usize,
    pub choice: Choice,
    pub name: String,
    pub name_touched: bool,
    pub connect: Connect,
    /// Where Back goes.
    pub back: Screen,
}

/// One address typed under "Enter an address", being looked up.
pub struct Lookup {
    pub address: String,
    pub running: Option<mpsc::Receiver<Option<Discovered>>>,
    pub error: Option<String>,
}

/// The decoded product pictures, by file path (`None`: missing or unreadable).
#[derive(Default)]
pub struct Images {
    textures: HashMap<String, Option<egui::TextureHandle>>,
    /// Pictures decoded this frame (decoding is spread over frames).
    budget: usize,
}
impl Images {
    pub fn begin_frame(&mut self) {
        self.budget = 3;
    }
    /// The picture of a variant's image file, loaded on first use.
    pub fn get(
        &mut self,
        ctx: &egui::Context,
        asset_dir: &str,
        file: Option<&str>,
    ) -> Option<egui::TextureHandle> {
        let file = file?;
        let key = format!("{asset_dir}/{file}");
        if let Some(t) = self.textures.get(&key) {
            return t.clone();
        }
        if self.budget == 0 {
            ctx.request_repaint();
            return None;
        }
        self.budget -= 1;
        let texture = catalog::image_path(asset_dir, file)
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|bytes| image::load_from_memory(&bytes).ok())
            .map(|img| {
                let rgba = img.to_rgba8();
                let size = [rgba.width() as usize, rgba.height() as usize];
                ctx.load_texture(
                    format!("printer-image-{key}"),
                    egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()),
                    egui::TextureOptions::LINEAR,
                )
            });
        self.textures.insert(key, texture.clone());
        texture
    }
}

/// A vendor's full profile bundle being loaded for Setup (off the UI thread).
pub struct VendorLoad {
    pub key: String,
    pub rx: mpsc::Receiver<Result<mica_profiles::orca::Bundle, String>>,
}

pub struct AddPrinter {
    pub screen: Screen,
    pub discovery: Discovery,
    pub catalog: Option<Catalog>,
    /// Every brand's printers, read on a thread when the window first opens.
    pub index: catalog::IndexLoader,
    /// Where the index is cached (`None`: memory only, as in headless runs).
    pub index_cache: Option<std::path::PathBuf>,
    pub vendor_load: Option<VendorLoad>,
    /// The "+N more" brand list is open.
    pub brands_open: bool,
    pub search: String,
    pub brand: Option<String>,
    pub setup: Option<Setup>,
    /// A found printer with no matching model: the connection waits for the model pick.
    pub pending: Option<Connect>,
    pub lookup: Option<Lookup>,
    /// Printers found by "Enter an address", shown with the network's.
    pub looked_up: Vec<Discovered>,
    pub images: Images,
    pub cameras: CameraViews,
    pub error: Option<String>,
    /// The window was open last frame (a fresh open searches again).
    pub(crate) was_open: bool,
}

impl AddPrinter {
    pub fn new(target: Option<std::net::SocketAddr>) -> Self {
        Self {
            screen: Screen::Found,
            discovery: Discovery::new(target),
            catalog: None,
            index: Default::default(),
            index_cache: None,
            vendor_load: None,
            brands_open: false,
            search: String::new(),
            brand: None,
            setup: None,
            pending: None,
            lookup: None,
            looked_up: vec![],
            images: Images::default(),
            cameras: CameraViews::default(),
            error: None,
            was_open: false,
        }
    }
    pub fn with_index_cache(mut self, path: Option<std::path::PathBuf>) -> Self {
        self.index_cache = path;
        self
    }
}

/// How a button looks.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Style {
    Primary,
    /// Accent-tinted surface (the secondary call to action).
    Soft,
    Secondary,
}

/// A fixed-size button painted in `rect`.
pub fn button_in(
    ui: &mut egui::Ui,
    rect: Rect,
    id: egui::Id,
    text: &str,
    style: Style,
    enabled: bool,
) -> egui::Response {
    let p = theme::of(ui);
    let r = ui.interact(
        rect,
        id,
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    let hot = enabled && r.hovered();
    let (fill, stroke, colour, font) = match style {
        Style::Primary => (
            if hot { p.accent_bright } else { p.accent },
            None,
            p.on_accent,
            theme::semibold(13.),
        ),
        Style::Soft => (
            if hot {
                p.selected_row
            } else {
                p.accent_surface
            },
            Some(p.scope_border),
            p.mark_fg,
            theme::medium(13.),
        ),
        Style::Secondary => (
            if hot { p.hover } else { p.raised },
            Some(p.border_raised),
            p.text,
            theme::medium(13.),
        ),
    };
    let alpha = if enabled { 1. } else { 0.45 };
    ui.painter()
        .rect_filled(rect, 8., fill.gamma_multiply(alpha));
    if let Some(s) = stroke {
        ui.painter().rect_stroke(
            rect,
            8.,
            Stroke::new(1., s.gamma_multiply(alpha)),
            egui::StrokeKind::Inside,
        );
    }
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        text,
        font,
        colour.gamma_multiply(alpha),
    );
    if hot {
        ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
    }
    r
}

/// A rounded pill with text (`dot`: a status dot first). Returns its rect.
pub fn pill(
    ui: &egui::Ui,
    at: Pos2,
    text: &str,
    fg: Color32,
    bg: Color32,
    dot: bool,
    mono: bool,
) -> Rect {
    let font = if mono {
        theme::mono(11.)
    } else {
        theme::medium(11.)
    };
    let g = ui.painter().layout_no_wrap(text.to_string(), font, fg);
    let left_pad = if dot { 20. } else { 8. };
    let rect = Rect::from_min_size(at, Vec2::new(g.size().x + left_pad + 8., 20.));
    ui.painter().rect_filled(rect, 10., bg);
    if dot {
        ui.painter()
            .circle_filled(Pos2::new(rect.left() + 11., rect.center().y), 3.5, fg);
    }
    ui.painter().galley(
        Pos2::new(rect.left() + left_pad, rect.center().y - g.size().y / 2.),
        g,
        fg,
    );
    rect
}

/// The colours of a state pill: (text / dot, background).
pub fn state_colours(
    p: &theme::Palette,
    state: Option<PrinterState>,
    failed: bool,
) -> (Color32, Color32) {
    // Text in the theme's readable tone, the tint from the state's own colour.
    let (fg, tint) = match state {
        _ if failed => (p.muted_alt, None),
        Some(PrinterState::Idle | PrinterState::Finished) => (p.success, Some(p.success)),
        Some(PrinterState::Printing | PrinterState::Paused) => (p.warn_text, Some(theme::WARNING)),
        Some(PrinterState::Error) => (theme::DANGER, Some(theme::DANGER)),
        Some(PrinterState::Offline) | None => (p.muted_alt, None),
    };
    (fg, tint.map_or(p.raised, |t| t.gamma_multiply(0.18)))
}

/// "Ready", "Printing 43 %", "Paused", "Offline".
pub fn state_text(state: Option<PrinterState>, progress: Option<f64>, ready: &str) -> String {
    match state {
        Some(PrinterState::Idle) => ready.to_string(),
        Some(PrinterState::Finished) => "Finished".into(),
        Some(PrinterState::Printing) => match progress {
            Some(p) => format!("Printing {p:.0} %"),
            None => "Printing".into(),
        },
        Some(PrinterState::Paused) => "Paused".into(),
        Some(PrinterState::Error) => "Error".into(),
        Some(PrinterState::Offline) | None => "Offline".into(),
    }
}

/// The kind word on a found printer's card.
pub fn kind_word(kind: mica_printhost::PrinterKind) -> &'static str {
    match kind {
        mica_printhost::PrinterKind::PrusaLink => "PrusaLink",
        mica_printhost::PrinterKind::PrusaConnect => "Prusa Connect (API key)",
        _ => "Moonraker",
    }
}

/// A product picture fitted into `rect` on the card's image well, or a neutral silhouette.
pub fn picture(ui: &egui::Ui, rect: Rect, radius: f32, texture: Option<&egui::TextureHandle>) {
    let p = theme::of(ui);
    ui.painter().rect_filled(rect, radius, p.raised);
    if let Some(t) = texture {
        let size = t.size_vec2();
        let inner = rect.shrink(10.);
        let scale = (inner.width() / size.x).min(inner.height() / size.y);
        let shown = Rect::from_center_size(rect.center(), size * scale);
        ui.painter().image(
            t.id(),
            shown,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1., 1.)),
            Color32::WHITE,
        );
        return;
    }
    // A neutral printer: frame, gantry, print head and bed.
    let h = (rect.height() * 0.62).min(rect.width() * 0.5);
    let w = h * 0.92;
    let frame = Rect::from_center_size(rect.center(), Vec2::new(w, h));
    let c = p.muted.gamma_multiply(0.55);
    let s = Stroke::new(1.5, c);
    ui.painter()
        .rect_stroke(frame, 4., s, egui::StrokeKind::Middle);
    let gantry_y = frame.top() + h * 0.32;
    ui.painter().line_segment(
        [
            Pos2::new(frame.left() + 4., gantry_y),
            Pos2::new(frame.right() - 4., gantry_y),
        ],
        s,
    );
    let head = Rect::from_center_size(
        Pos2::new(frame.center().x - w * 0.08, gantry_y + 6.),
        Vec2::new(w * 0.16, h * 0.12),
    );
    ui.painter().rect_filled(head, 2., c);
    let bed_y = frame.bottom() - h * 0.18;
    ui.painter().line_segment(
        [
            Pos2::new(frame.left() + w * 0.12, bed_y),
            Pos2::new(frame.right() - w * 0.12, bed_y),
        ],
        Stroke::new(2.5, c),
    );
}

/// A single-line text field drawn like the mockup's (12 px inset, 8 px radius).
#[allow(clippy::too_many_arguments)]
pub fn field(
    ui: &mut egui::Ui,
    rect: Rect,
    id: egui::Id,
    text: &mut String,
    hint: &str,
    mono: bool,
    password: bool,
    highlight: bool,
) -> egui::Response {
    let p = theme::of(ui);
    let te_id = id.with("text");
    let focused = ui.memory(|m| m.has_focus(te_id));
    ui.painter().rect_filled(rect, 8., p.search_bg);
    ui.painter().rect_stroke(
        rect,
        8.,
        Stroke::new(
            1.,
            if focused || highlight {
                p.accent
            } else {
                p.field_border
            },
        ),
        egui::StrokeKind::Inside,
    );
    let font = if mono {
        theme::mono(13.)
    } else {
        theme::sans(13.)
    };
    let row = ui.fonts_mut(|f| f.row_height(&font));
    let inner = Rect::from_min_max(
        Pos2::new(rect.left() + 8., rect.center().y - row / 2.),
        Pos2::new(rect.right() - 8., rect.center().y + row / 2.),
    );
    ui.put(
        inner,
        egui::TextEdit::singleline(text)
            .id(te_id)
            .font(font.clone())
            .frame(egui::Frame::NONE)
            .password(password)
            .hint_text(egui::RichText::new(hint).color(p.muted).font(font))
            .margin(egui::Margin::symmetric(4, 0))
            .desired_width(inner.width()),
    )
}

/// Single-line text truncated to `width`, top-left at `at`.
pub fn line(
    ui: &egui::Ui,
    at: Pos2,
    text: &str,
    font: egui::FontId,
    colour: Color32,
    width: f32,
) -> f32 {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_string(), font, colour);
    job.wrap = egui::text::TextWrapping::truncate_at_width(width.max(10.));
    let g = ui.painter().layout_job(job);
    let h = g.size().y;
    ui.painter().galley(at, g, colour);
    h
}

/// Clickable text ("← Back", "All printer profiles…").
fn text_link(
    ui: &mut egui::Ui,
    at: Pos2,
    align: Align2,
    id: egui::Id,
    text: &str,
    colour: Color32,
    hot: Color32,
) -> bool {
    let g = ui
        .painter()
        .layout_no_wrap(text.to_string(), theme::sans(13.), colour);
    let rect = align.anchor_size(at, g.size());
    let r = ui.interact(rect.expand(4.), id, Sense::click());
    let c = if r.hovered() { hot } else { colour };
    ui.painter()
        .text(rect.min, Align2::LEFT_TOP, text, theme::sans(13.), c);
    if r.hovered() {
        ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
    }
    r.clicked()
}

/// The brand a found printer's backend belongs to: PrusaLink is Prusa, the U1 Snapmaker;
/// Moonraker runs on many brands' Klipper printers (RatRig, Voron, Creality…), so any.
fn vendor_of(kind: mica_printhost::PrinterKind) -> Option<&'static str> {
    match kind {
        mica_printhost::PrinterKind::PrusaLink | mica_printhost::PrinterKind::PrusaConnect => {
            Some("Prusa")
        }
        mica_printhost::PrinterKind::SnapmakerU1 => Some("Snapmaker"),
        mica_printhost::PrinterKind::Moonraker => None,
    }
}

/// `vendor_of` for a saved connection's kind.
pub fn family(kind: Kind) -> Option<&'static str> {
    match kind {
        Kind::PrusaLink | Kind::PrusaConnect => Some("Prusa"),
        Kind::SnapmakerU1 => Some("Snapmaker"),
        Kind::Moonraker => None,
    }
}

/// "HF 0.4 nozzle", "5 tools · 0.4 nozzle".
pub fn choice_summary(model: &Model, c: &Choice) -> String {
    let nozzle = model
        .variants
        .iter()
        .find(|v| v.nozzle.raw == c.nozzle)
        .map(|v| v.nozzle.short())
        .unwrap_or_else(|| c.nozzle.clone());
    let tools = match (&c.tools.mmu, c.tools.count) {
        (Some(m), _) => format!("{m} · "),
        (None, 1) => String::new(),
        (None, n) => format!("{n} tools · "),
    };
    format!("{tools}{nozzle} nozzle")
}

/// A default printer name for a model and choice: "XL 5T", "CORE One", "U1".
pub fn default_name(model: &Model, c: &Choice) -> String {
    let short = model
        .name
        .strip_prefix(&format!("{} ", model.brand))
        .unwrap_or(&model.name)
        .to_string();
    let several = model.tool_options().len() > 1;
    let mut name = match (&c.tools.mmu, c.tools.count) {
        (Some(m), _) => format!("{short} {m}"),
        (None, n) if n > 1 && several => format!("{short} {n}T"),
        _ => short,
    };
    // "V-Core 4 400", "V-Core 4 IDEX 300 Copy".
    if model.size_options().len() > 1 && !c.size.is_empty() {
        name = format!("{name} {}", c.size);
    }
    if !c.mode.is_empty() {
        name = format!("{name} {}", c.mode);
    }
    name
}

impl App {
    /// Opens the Add printer window on its first screen (and searches the network).
    pub(crate) fn open_add_printer(&mut self) {
        self.dialogs.add_printer = true;
        self.add_printer.screen = Screen::Found;
        self.add_printer.error = None;
    }

    /// The models of the loaded bundles and, once read, of every other vendor.
    pub(crate) fn catalog(&mut self) -> &Catalog {
        if self.add_printer.catalog.is_none() {
            let index = self.add_printer.index.index.clone();
            self.add_printer.catalog = Some(Catalog::build_with(&self.profiles, index.as_deref()));
        }
        self.add_printer.catalog.as_ref().expect("built above")
    }

    /// Starts reading every brand's printers (once per run; cached on disk).
    pub(crate) fn start_gallery_index(&mut self, repaint: impl FnOnce() + Send + 'static) {
        let root = self.profiles.orca_root.clone();
        let cache = self.add_printer.index_cache.clone();
        self.add_printer.index.start(root, cache, repaint);
    }

    /// Reads every brand's printers now (headless screenshots, tests, the monkey test).
    pub(crate) fn load_gallery_index_now(&mut self) {
        self.start_gallery_index(|| {});
        self.add_printer.index.wait();
        self.rebuild_catalog();
    }

    /// Waits for the vendor bundle Setup is loading (headless screenshots and tests).
    pub(crate) fn finish_vendor_load(&mut self) {
        self.poll_vendor_load(true);
    }

    /// Takes the brands' printers in when they arrive: the catalog is rebuilt and an open
    /// Setup keeps its model.
    pub(crate) fn poll_gallery_index(&mut self) {
        if !self.add_printer.index.poll() {
            return;
        }
        self.rebuild_catalog();
    }

    fn rebuild_catalog(&mut self) {
        let old = self.add_printer.catalog.take();
        let picked = self.add_printer.setup.as_ref().and_then(|s| {
            let m = old.as_ref()?.models.get(s.model)?;
            Some((m.vendor.clone(), m.name.clone()))
        });
        let catalog = self.catalog();
        let found = picked.and_then(|(vendor, name)| {
            catalog
                .models
                .iter()
                .position(|m| m.vendor == vendor && m.name == name)
        });
        if let (Some(mi), Some(s)) = (found, &mut self.add_printer.setup) {
            s.model = mi;
        }
    }

    /// Setup's model comes from a vendor that is not loaded yet: load it on a thread.
    fn prefetch_vendor(&mut self, key: &str, repaint: impl FnOnce() + Send + 'static) {
        if self.profiles.vendor_index(key).is_some()
            || self
                .add_printer
                .vendor_load
                .as_ref()
                .is_some_and(|l| l.key == key)
        {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let root = self.profiles.orca_root.clone();
        let k = key.to_string();
        let spawned = std::thread::Builder::new()
            .name("mica-vendor-profiles".into())
            .spawn(move || {
                let _ = tx.send(crate::panels::profiles::Profiles::load_orca_vendor(
                    root.as_deref(),
                    &k,
                ));
                repaint();
            });
        if spawned.is_ok() {
            self.add_printer.vendor_load = Some(VendorLoad {
                key: key.to_string(),
                rx,
            });
        }
    }

    /// Installs a vendor bundle loaded by `prefetch_vendor` (`block`: wait for it).
    fn poll_vendor_load(&mut self, block: bool) {
        let Some(l) = &self.add_printer.vendor_load else {
            return;
        };
        let got = if block {
            l.rx.recv().map_err(|_| mpsc::TryRecvError::Disconnected)
        } else {
            l.rx.try_recv()
        };
        let key = l.key.clone();
        match got {
            Ok(Ok(bundle)) => {
                self.add_printer.vendor_load = None;
                self.profiles.install_vendor(&key, bundle);
            }
            Ok(Err(e)) => {
                self.add_printer.vendor_load = None;
                self.add_printer.error = Some(e);
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                self.add_printer.vendor_load = None;
                self.add_printer.error = Some(format!("Loading the {key} profiles stopped."));
            }
        }
    }

    /// Whether Setup is waiting for its vendor's profiles.
    fn vendor_loading(&self) -> bool {
        self.add_printer.vendor_load.is_some()
    }

    /// The model card of the found printer `d`: (model index, choice), if one fits.
    fn match_found(&mut self, d: &Discovered) -> Option<(usize, Choice)> {
        let text = d
            .model
            .clone()
            .or_else(|| mica_printhost::discovery::guess_model(&d.name).map(str::to_string))
            .or_else(|| {
                d.hostname
                    .as_deref()
                    .and_then(mica_printhost::discovery::guess_model)
                    .map(str::to_string)
            })
            .unwrap_or_else(|| d.name.clone());
        let tools = d.tools.map(|t| t as usize);
        let vendor = vendor_of(d.kind);
        // Moonraker printers: the U1 first (as before), then any brand.
        let c = self.catalog();
        vendor
            .or(Some("Snapmaker"))
            .and_then(|v| c.match_printer(Some(v), &text, tools, d.nozzle))
            .or_else(|| {
                vendor
                    .is_none()
                    .then(|| c.match_printer(None, &text, tools, d.nozzle))
                    .flatten()
            })
    }

    pub(crate) fn add_printer_window(&mut self, ctx: &egui::Context) {
        let open = self.dialogs.add_printer;
        if !open {
            if self.add_printer.was_open {
                self.add_printer.was_open = false;
                self.add_printer.cameras.clear();
            }
            return;
        }
        if !self.add_printer.was_open {
            self.add_printer.was_open = true;
            let c = ctx.clone();
            self.add_printer
                .discovery
                .start(move || c.request_repaint());
        }
        if !self.add_printer.index.loading() && self.add_printer.index.index.is_none() {
            let c = ctx.clone();
            self.start_gallery_index(move || c.request_repaint());
        }
        self.poll_gallery_index();
        self.poll_vendor_load(false);
        self.add_printer.discovery.poll();
        self.poll_lookup();
        self.poll_setup_test();
        self.add_printer.images.begin_frame();
        self.add_printer.cameras.begin_frame();
        let p = theme::of_ctx(ctx);
        let mut keep = true;
        egui::Window::new("Add a printer")
            .id(egui::Id::unique("add-printer"))
            .title_bar(false)
            .resizable(false)
            .collapsible(false)
            .fixed_size(SIZE)
            .default_pos(ctx.content_rect().center() - SIZE / 2.)
            .frame(
                egui::Frame::window(&ctx.global_style())
                    .fill(p.bg)
                    .stroke(Stroke::new(1., p.border_soft))
                    .corner_radius(12)
                    .inner_margin(0),
            )
            .show(ctx, |ui| {
                let (rect, _) = ui.allocate_exact_size(SIZE, Sense::hover());
                match self.add_printer.screen {
                    Screen::Found => self.found_screen(ui, rect, &mut keep),
                    Screen::Gallery => self.gallery_screen(ui, rect),
                    Screen::Setup => self.setup_screen(ui, rect),
                }
            });
        self.add_printer.cameras.end_frame();
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) && !widgets::field_editing(ctx) {
            keep = false;
        }
        if !keep {
            self.dialogs.add_printer = false;
        }
    }

    /// The close button at the window's top right.
    fn close_button(&mut self, ui: &mut egui::Ui, rect: Rect, keep: &mut bool) {
        let p = theme::of(ui);
        let r = Rect::from_min_size(
            Pos2::new(rect.right() - PAD - 32., rect.top() + 20.),
            Vec2::splat(32.),
        );
        let resp = ui.interact(r, egui::Id::unique("add-printer-close"), Sense::click());
        ui.painter()
            .rect_filled(r, 8., if resp.hovered() { p.hover } else { p.raised });
        widgets::glyph(ui.painter(), r.shrink(9.), Glyph::Close, p.muted_alt);
        if resp.on_hover_text("Close").clicked() {
            *keep = false;
        }
    }

    // ---- Screen 1: found on your network -------------------------------------------------

    fn found_screen(&mut self, ui: &mut egui::Ui, rect: Rect, keep: &mut bool) {
        let p = theme::of(ui);
        let left = rect.left() + PAD;
        let inner_w = rect.width() - 2. * PAD;
        ui.painter().text(
            Pos2::new(left, rect.top() + 20.),
            Align2::LEFT_TOP,
            "Add a printer",
            theme::semibold(22.),
            p.text,
        );
        ui.painter().text(
            Pos2::new(left, rect.top() + 52.),
            Align2::LEFT_TOP,
            "Mica looked on your network. Pick a printer and it's ready in one click.",
            theme::sans(13.),
            p.muted_alt,
        );
        self.close_button(ui, rect, keep);
        // Refresh, left of the close button.
        let running = self.add_printer.discovery.running();
        let refresh = Rect::from_min_size(
            Pos2::new(rect.right() - PAD - 32. - 8. - 96., rect.top() + 20.),
            Vec2::new(96., 32.),
        );
        if button_in(
            ui,
            refresh,
            egui::Id::unique("add-printer-refresh"),
            if running { "Searching…" } else { "Refresh" },
            Style::Secondary,
            !running,
        )
        .clicked()
        {
            let c = ui.ctx().clone();
            self.add_printer
                .discovery
                .start(move || c.request_repaint());
        }
        // The list: the network's answers and looked-up addresses.
        let mut found: Vec<Discovered> = self.add_printer.discovery.found().to_vec();
        for d in &self.add_printer.looked_up {
            if !found.iter().any(|f| f.address == d.address) {
                found.push(d.clone());
            }
        }
        let y = rect.top() + 92.;
        let (dot, words) = match &self.add_printer.discovery.state {
            Search::Running(_) => (p.muted, "SEARCHING YOUR NETWORK…".to_string()),
            _ if !found.is_empty() => (
                p.success,
                format!("FOUND ON YOUR NETWORK · {}", found.len()),
            ),
            _ => (p.muted, "FOUND ON YOUR NETWORK".to_string()),
        };
        ui.painter()
            .circle_filled(Pos2::new(left + 4., y + 7.), 4., dot);
        ui.painter().text(
            Pos2::new(left + 16., y),
            Align2::LEFT_TOP,
            widgets::spaced_upper(&words),
            theme::semibold(11.),
            p.muted,
        );
        let bottom_h = 64.;
        let list = Rect::from_min_max(
            Pos2::new(left, y + 26.),
            Pos2::new(rect.right() - PAD, rect.bottom() - 24. - bottom_h - 16.),
        );
        if found.is_empty() {
            let text = match &self.add_printer.discovery.state {
                Search::Running(_) | Search::Idle => "Asking the printers on your network…".to_string(),
                Search::Off(why) => format!("{why} Pick the model below, or enter the printer's address."),
                Search::Failed(e) => e.clone(),
                Search::Done(_) => "No printer answered. Check that it is switched on and on the same network as this computer, or enter its address below.".into(),
            };
            let mut job =
                egui::text::LayoutJob::simple(text, theme::sans(13.), p.label, inner_w * 0.7);
            job.halign = egui::Align::Center;
            let g = ui.painter().layout_job(job);
            ui.painter()
                .galley(Pos2::new(list.center().x, list.top() + 60.), g, p.label);
        } else {
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(list));
            egui::ScrollArea::vertical()
                .id_salt("add-printer-found")
                .auto_shrink([false, false])
                .show(&mut child, |ui| {
                    let cols = 3usize;
                    let w = ((list.width() - GAP * (cols as f32 - 1.)) / cols as f32).floor();
                    let h = 296.;
                    let rows = found.len().div_ceil(cols);
                    let (area, _) = ui.allocate_exact_size(
                        Vec2::new(list.width(), rows as f32 * (h + GAP) - GAP),
                        Sense::hover(),
                    );
                    for (i, d) in found.iter().enumerate() {
                        let (row, col) = (i / cols, i % cols);
                        let card = Rect::from_min_size(
                            Pos2::new(
                                area.left() + col as f32 * (w + GAP),
                                area.top() + row as f32 * (h + GAP),
                            ),
                            Vec2::new(w, h),
                        );
                        if ui.is_rect_visible(card) {
                            self.found_card(ui, card, i, d);
                        }
                    }
                });
        }
        // Bottom: not listed?
        let bottom = Rect::from_min_size(
            Pos2::new(left, rect.bottom() - 24. - bottom_h),
            Vec2::new(inner_w, bottom_h),
        );
        ui.painter().rect_filled(bottom, 10., p.panel);
        if let Some(lookup) = &mut self.add_printer.lookup {
            let busy = lookup.running.is_some();
            let field_r = Rect::from_min_size(
                Pos2::new(bottom.left() + 16., bottom.center().y - 17.),
                Vec2::new(360., 34.),
            );
            let r = field(
                ui,
                field_r,
                egui::Id::unique("add-printer-lookup"),
                &mut lookup.address,
                "192.0.2.50, prusa-xl.local or u1.local:7125",
                true,
                false,
                false,
            );
            let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let find = Rect::from_min_size(
                Pos2::new(field_r.right() + 8., field_r.top()),
                Vec2::new(120., 34.),
            );
            let go = button_in(
                ui,
                find,
                egui::Id::unique("add-printer-lookup-go"),
                if busy { "Looking…" } else { "Find printer" },
                Style::Soft,
                !busy && !lookup.address.trim().is_empty(),
            )
            .clicked()
                || enter && !busy;
            let cancel = Rect::from_min_size(
                Pos2::new(find.right() + 8., field_r.top()),
                Vec2::new(84., 34.),
            );
            let close = button_in(
                ui,
                cancel,
                egui::Id::unique("add-printer-lookup-cancel"),
                "Cancel",
                Style::Secondary,
                true,
            )
            .clicked();
            if let Some(e) = &lookup.error {
                line(
                    ui,
                    Pos2::new(cancel.right() + 12., bottom.center().y - 8.),
                    e,
                    theme::sans(12.),
                    p.warn_text,
                    bottom.right() - cancel.right() - 28.,
                );
            }
            if go {
                self.start_lookup(ui.ctx());
            }
            if close {
                self.add_printer.lookup = None;
            }
        } else {
            ui.painter().text(
                Pos2::new(bottom.left() + 16., bottom.top() + 14.),
                Align2::LEFT_TOP,
                "Printer not listed, or not on the network?",
                theme::medium(13.),
                p.text,
            );
            ui.painter().text(
                Pos2::new(bottom.left() + 16., bottom.top() + 33.),
                Align2::LEFT_TOP,
                "Pick the model yourself. You can connect it later.",
                theme::sans(13.),
                p.muted_alt,
            );
            let addr = Rect::from_min_size(
                Pos2::new(bottom.right() - 16. - 136., bottom.center().y - 17.),
                Vec2::new(136., 34.),
            );
            let pick = Rect::from_min_size(
                Pos2::new(addr.left() - 8. - 112., addr.top()),
                Vec2::new(112., 34.),
            );
            if button_in(
                ui,
                pick,
                egui::Id::unique("add-printer-pick"),
                "Pick a model",
                Style::Soft,
                true,
            )
            .clicked()
            {
                self.add_printer.pending = None;
                self.add_printer.screen = Screen::Gallery;
            }
            if button_in(
                ui,
                addr,
                egui::Id::unique("add-printer-address"),
                "Enter an address",
                Style::Secondary,
                true,
            )
            .clicked()
            {
                self.add_printer.lookup = Some(Lookup {
                    address: String::new(),
                    running: None,
                    error: None,
                });
                ui.memory_mut(|m| {
                    m.request_focus(egui::Id::unique("add-printer-lookup").with("text"))
                });
            }
        }
    }

    fn found_card(&mut self, ui: &mut egui::Ui, card: Rect, i: usize, d: &Discovered) {
        let p = theme::of(ui);
        let matched = self.match_found(d);
        let response = ui.interact(card, egui::Id::unique(("found-card", i)), Sense::hover());
        let hot = response.hovered() || ui.rect_contains_pointer(card);
        ui.painter()
            .rect_filled(card, 12., if hot { p.changed_row } else { p.panel });
        ui.painter().rect_stroke(
            card,
            12.,
            Stroke::new(1., if hot { p.accent } else { p.border_raised }),
            egui::StrokeKind::Inside,
        );
        let pad = 16.;
        let inner = card.shrink(pad);
        let image = Rect::from_min_size(inner.min, Vec2::new(inner.width(), 132.));
        let texture = matched.as_ref().and_then(|(mi, choice)| {
            let m = self.add_printer.catalog.as_ref()?.models.get(*mi)?.clone();
            let v = m.resolve(choice).or(Some(m.default_variant()))?.clone();
            self.add_printer
                .images
                .get(ui.ctx(), &m.asset_dir, v.image.as_deref())
        });
        picture(ui, image, 8., texture.as_ref());
        let mut y = image.bottom() + 10.;
        line(
            ui,
            Pos2::new(inner.left(), y),
            &d.name,
            theme::semibold(15.),
            p.text,
            inner.width(),
        );
        y += 21.;
        let second = match &matched {
            Some((mi, c)) => {
                let m = &self.add_printer.catalog.as_ref().expect("built").models[*mi];
                format!("{} · {}", m.name, choice_summary(m, c))
            }
            None => format!(
                "{} · pick the model",
                d.model.clone().unwrap_or_else(|| "Unknown model".into())
            ),
        };
        line(
            ui,
            Pos2::new(inner.left(), y),
            &second,
            theme::sans(13.),
            p.muted_alt,
            inner.width(),
        );
        y += 26.;
        let (fg, bg) = if d.needs_key {
            (p.warn_text, p.warn_surface)
        } else {
            state_colours(p, d.state, false)
        };
        let word = if d.needs_key {
            "Needs API key".to_string()
        } else {
            state_text(d.state, d.progress, "Idle")
        };
        let a = pill(ui, Pos2::new(inner.left(), y), &word, fg, bg, false, false);
        let b = pill(
            ui,
            Pos2::new(a.right() + 6., y),
            kind_word(d.kind),
            p.label,
            p.raised,
            false,
            false,
        );
        line(
            ui,
            Pos2::new(b.right() + 8., y + 3.),
            &d.address,
            theme::mono(11.),
            p.muted,
            inner.right() - b.right() - 8.,
        );
        let added = self
            .printers
            .list
            .iter()
            .any(|x| same_address(&x.address, &d.address));
        let btn = Rect::from_min_size(
            Pos2::new(inner.left(), inner.bottom() - 36.),
            Vec2::new(inner.width(), 36.),
        );
        let label = if added {
            "Already in My printers"
        } else if matched.is_some() {
            "Add this printer"
        } else {
            "Choose the model…"
        };
        if button_in(
            ui,
            btn,
            egui::Id::unique(("found-add", i)),
            label,
            if hot && !added {
                Style::Primary
            } else {
                Style::Secondary
            },
            !added,
        )
        .clicked()
        {
            self.add_found(d.clone(), matched);
        }
    }

    /// "Add this printer": straight in when nothing is missing, else Setup (the key) or the
    /// model gallery (no model fits).
    fn add_found(&mut self, d: Discovered, matched: Option<(usize, Choice)>) {
        let connect = Connect {
            kind: Some(match d.kind {
                mica_printhost::PrinterKind::PrusaLink => Kind::PrusaLink,
                mica_printhost::PrinterKind::PrusaConnect => Kind::PrusaConnect,
                mica_printhost::PrinterKind::Moonraker => Kind::Moonraker,
                mica_printhost::PrinterKind::SnapmakerU1 => Kind::SnapmakerU1,
            }),
            address: d.address.clone(),
            model: d.model.clone(),
            needs_key: d.needs_key,
            ..Connect::default()
        };
        let Some((mi, choice)) = matched else {
            self.add_printer.pending = Some(connect);
            self.add_printer.screen = Screen::Gallery;
            return;
        };
        let needs_key = d.needs_key;
        self.begin_setup(mi, Some(choice), Some(connect), Screen::Found);
        if let Some(s) = &mut self.add_printer.setup {
            s.name = unique_name(&self.printers.list, &d.name);
            s.name_touched = true;
        }
        if !needs_key {
            self.finish_setup();
        }
    }

    fn start_lookup(&mut self, ctx: &egui::Context) {
        let Some(l) = &mut self.add_printer.lookup else {
            return;
        };
        let address = l
            .address
            .trim()
            .trim_start_matches("http://")
            .trim_end_matches('/')
            .to_string();
        l.error = None;
        if crate::printhost::loopback_only() && !crate::printhost::is_loopback(&address) {
            l.error = Some("This run only talks to printers on this computer (127.0.0.1).".into());
            return;
        }
        let (tx, rx) = mpsc::channel();
        l.running = Some(rx);
        let c = ctx.clone();
        std::thread::Builder::new()
            .name("mica-printer-lookup".into())
            .spawn(move || {
                // A bare host gets the usual ports tried: PrusaLink on 80, Moonraker on 7125.
                let tries: Vec<String> = if address.contains(':') && !address.starts_with('[')
                    || address.contains("]:")
                {
                    vec![address.clone()]
                } else {
                    vec![address.clone(), format!("{address}:7125")]
                };
                let mut found = None;
                for a in tries {
                    let candidate = mica_printhost::discovery::Candidate {
                        name: address.clone(),
                        address: a,
                        hint: None,
                        txt: Default::default(),
                        host: Some(address.split(['.', ':']).next().unwrap_or("").to_string()),
                    };
                    found = mica_printhost::discovery::identify(
                        &candidate,
                        mica_printhost::discovery::probe_timeouts(),
                    );
                    if found.is_some() {
                        break;
                    }
                }
                let _ = tx.send(found);
                c.request_repaint();
            })
            .ok();
    }

    fn poll_lookup(&mut self) {
        let Some(l) = &mut self.add_printer.lookup else {
            return;
        };
        let Some(rx) = &l.running else { return };
        match rx.try_recv() {
            Ok(Some(d)) => {
                l.running = None;
                self.add_printer
                    .looked_up
                    .retain(|x| x.address != d.address);
                self.add_printer.looked_up.push(d);
                self.add_printer.lookup = None;
            }
            Ok(None) => {
                l.running = None;
                l.error = Some(format!(
                    "No PrusaLink or Moonraker answered at {}.",
                    l.address.trim()
                ));
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                l.running = None;
                l.error = Some("The lookup stopped unexpectedly.".into());
            }
        }
    }

    // ---- Screen 2: pick a model ----------------------------------------------------------

    fn gallery_screen(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let p = theme::of(ui);
        self.catalog();
        let left = rect.left() + PAD;
        let top = rect.top() + 20.;
        if text_link(
            ui,
            Pos2::new(left, top + 18.),
            Align2::LEFT_CENTER,
            egui::Id::unique("gallery-back"),
            "← Back",
            p.muted_alt,
            p.text,
        ) {
            self.add_printer.screen = Screen::Found;
        }
        ui.painter().text(
            Pos2::new(left + 64., top + 18.),
            Align2::LEFT_CENTER,
            "Pick your printer",
            theme::semibold(22.),
            p.text,
        );
        let brand = self.add_printer.brand.clone();
        let shown = self
            .add_printer
            .catalog
            .as_ref()
            .expect("built")
            .search(&self.add_printer.search, brand.as_deref());
        let search = Rect::from_min_size(
            Pos2::new(rect.right() - PAD - 300., top),
            Vec2::new(300., 36.),
        );
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(search));
        widgets::search_field(
            &mut child,
            egui::Id::unique("gallery-search"),
            &mut self.add_printer.search,
            "Search printers",
            &format!(
                "{} model{}",
                shown.len(),
                if shown.len() == 1 { "" } else { "s" }
            ),
        );
        // Brand chips: All brands, then the popular brands first, as many as fit in one row;
        // the rest under "+N more" (a selected one from there takes the last place).
        let chips_y = top + 52.;
        let brands = self.add_printer.catalog.as_ref().expect("built").brands();
        let mut pick: Option<Option<String>> = None;
        let chip_w = |ui: &egui::Ui, label: &str| {
            ui.painter()
                .layout_no_wrap(label.to_string(), theme::medium(13.), p.text)
                .size()
                .x
                + 24.
        };
        let row_right = rect.right() - PAD;
        let all_w = chip_w(ui, "All brands") + 8.;
        let more_w = chip_w(ui, "+99 more");
        let mut shown_brands: Vec<String> = vec![];
        let mut used = left + all_w;
        for (i, b) in brands.iter().enumerate() {
            let w = chip_w(ui, b) + 8.;
            let rest = brands.len() - i - 1;
            let limit = if rest > 0 {
                row_right - more_w
            } else {
                row_right
            };
            if used + w > limit {
                break;
            }
            used += w;
            shown_brands.push(b.clone());
        }
        if let Some(sel) = &self.add_printer.brand
            && !shown_brands.contains(sel)
            && brands.contains(sel)
        {
            // Make room for the selected brand at the end of the row.
            while !shown_brands.is_empty() && used + chip_w(ui, sel) + 8. > row_right - more_w {
                let last = shown_brands.pop().expect("not empty");
                used -= chip_w(ui, &last) + 8.;
            }

            shown_brands.push(sel.clone());
        }
        let hidden: Vec<String> = brands
            .iter()
            .filter(|b| !shown_brands.contains(b))
            .cloned()
            .collect();
        let mut x = left;
        let mut more_rect = None;
        let chips: Vec<(Option<String>, String)> =
            std::iter::once((None, "All brands".to_string()))
                .chain(shown_brands.iter().map(|b| (Some(b.clone()), b.clone())))
                .chain((!hidden.is_empty()).then(|| (None, format!("+{} more", hidden.len()))))
                .collect();
        let n_chips = chips.len();
        for (i, (b, label)) in chips.into_iter().enumerate() {
            let is_more = !hidden.is_empty() && i == n_chips - 1;
            let on = if is_more {
                self.add_printer.brands_open
            } else {
                self.add_printer.brand == b
            };
            let r = Rect::from_min_size(Pos2::new(x, chips_y), Vec2::new(chip_w(ui, &label), 28.));
            let resp = ui.interact(r, egui::Id::unique(("brand-chip", i)), Sense::click());
            if on {
                ui.painter().rect_filled(r, 14., p.accent);
            } else {
                ui.painter()
                    .rect_filled(r, 14., if resp.hovered() { p.hover } else { p.raised });
                ui.painter().rect_stroke(
                    r,
                    14.,
                    Stroke::new(1., p.border_raised),
                    egui::StrokeKind::Inside,
                );
            }
            ui.painter().text(
                r.center(),
                Align2::CENTER_CENTER,
                &label,
                theme::medium(13.),
                if on { p.on_accent } else { p.text },
            );
            if resp.clicked() {
                if is_more {
                    self.add_printer.brands_open = !self.add_printer.brands_open;
                } else {
                    pick = Some(b);
                }
            }
            if resp.hovered() {
                ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
            }
            if is_more {
                more_rect = Some(r);
            }
            x = r.right() + 8.;
        }
        if let Some(b) = pick {
            self.add_printer.brand = b;
            self.add_printer.brands_open = false;
        }
        let rule_y = chips_y + 28. + 14.;
        ui.painter().line_segment(
            [
                Pos2::new(rect.left(), rule_y),
                Pos2::new(rect.right(), rule_y),
            ],
            Stroke::new(1., p.border_soft),
        );
        let footer_h = 48.;
        let list = Rect::from_min_max(
            Pos2::new(left, rule_y + 18.),
            Pos2::new(rect.right() - PAD, rect.bottom() - footer_h - 12.),
        );
        let mut chosen = None;
        if shown.is_empty() {
            ui.painter().text(
                Pos2::new(list.center().x, list.top() + 60.),
                Align2::CENTER_TOP,
                "No printer model matches. Try fewer words, or All brands.",
                theme::sans(13.),
                p.label,
            );
        } else {
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(list));
            egui::ScrollArea::vertical()
                .id_salt("add-printer-gallery")
                .auto_shrink([false, false])
                .show(&mut child, |ui| {
                    let cols = 3usize;
                    let w = ((list.width() - GAP * (cols as f32 - 1.)) / cols as f32).floor();
                    let h = 236.;
                    let rows = shown.len().div_ceil(cols);
                    let (area, _) = ui.allocate_exact_size(
                        Vec2::new(list.width(), rows as f32 * (h + GAP) - GAP),
                        Sense::hover(),
                    );
                    for (n, &mi) in shown.iter().enumerate() {
                        let (row, col) = (n / cols, n % cols);
                        let card = Rect::from_min_size(
                            Pos2::new(
                                area.left() + col as f32 * (w + GAP),
                                area.top() + row as f32 * (h + GAP),
                            ),
                            Vec2::new(w, h),
                        );
                        if ui.is_rect_visible(card) && self.model_card(ui, card, mi) {
                            chosen = Some(mi);
                        }
                    }
                });
        }
        if self.add_printer.brands_open
            && let Some(more) = more_rect
        {
            self.more_brands(ui, rect, more, &hidden);
        }
        // Footer.
        let fy = rect.bottom() - footer_h;
        ui.painter().line_segment(
            [Pos2::new(rect.left(), fy), Pos2::new(rect.right(), fy)],
            Stroke::new(1., p.border_soft),
        );
        let note = if self.add_printer.index.loading() {
            "Reading every brand's printers…".to_string()
        } else {
            "One card per printer model. Nozzles, tool count and variants are chosen in the next step."
                .to_string()
        };
        ui.painter().text(
            Pos2::new(left, fy + footer_h / 2.),
            Align2::LEFT_CENTER,
            note,
            theme::sans(13.),
            p.muted_alt,
        );
        if text_link(
            ui,
            Pos2::new(rect.right() - PAD, fy + footer_h / 2.),
            Align2::RIGHT_CENTER,
            egui::Id::unique("gallery-all"),
            "All printer profiles…",
            p.accent_bright,
            p.accent_hi,
        ) {
            // The full list, with the user's own and custom printer profiles.
            self.dialogs.all_printers = true;
            self.dialogs.add_printer = false;
        }
        if let Some(mi) = chosen {
            let connect = self.add_printer.pending.take();
            self.begin_setup(mi, None, connect, Screen::Gallery);
        }
    }

    /// The brands that did not fit in the chip row, as a grid under "+N more".
    fn more_brands(&mut self, ui: &mut egui::Ui, window: Rect, more: Rect, hidden: &[String]) {
        let p = theme::of(ui);
        let cols = 4usize;
        let cell = Vec2::new(150., 30.);
        let rows = hidden.len().div_ceil(cols);
        let height = (rows as f32 * cell.y + 16.).min(window.height() - 160.);
        let width = cols as f32 * cell.x + 16.;
        let left = (more.right() - width).max(window.left() + PAD);
        let area = Rect::from_min_size(
            Pos2::new(left, more.bottom() + 6.),
            Vec2::new(width, height),
        );
        let mut pick = None;
        egui::Area::new(egui::Id::unique("more-brands"))
            .order(egui::Order::Foreground)
            .fixed_pos(area.min)
            .show(ui.ctx(), |ui| {
                ui.painter().rect_filled(area, 10., p.panel);
                ui.painter().rect_stroke(
                    area,
                    10.,
                    Stroke::new(1., p.border_raised),
                    egui::StrokeKind::Inside,
                );
                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(area.shrink(8.)));
                egui::ScrollArea::vertical()
                    .id_salt("more-brands-scroll")
                    .auto_shrink([false, false])
                    .show(&mut child, |ui| {
                        let (grid, _) = ui.allocate_exact_size(
                            Vec2::new(cols as f32 * cell.x, rows as f32 * cell.y),
                            Sense::hover(),
                        );
                        for (i, b) in hidden.iter().enumerate() {
                            let r = Rect::from_min_size(
                                grid.min
                                    + Vec2::new(
                                        (i % cols) as f32 * cell.x,
                                        (i / cols) as f32 * cell.y,
                                    ),
                                cell - Vec2::new(6., 4.),
                            );
                            let resp =
                                ui.interact(r, egui::Id::unique(("more-brand", i)), Sense::click());
                            if resp.hovered() {
                                ui.painter().rect_filled(r, 6., p.hover);
                                ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
                            }
                            line(
                                ui,
                                Pos2::new(r.left() + 8., r.center().y - 8.),
                                b,
                                theme::sans(13.),
                                p.text,
                                r.width() - 12.,
                            );
                            if resp.clicked() {
                                pick = Some(b.clone());
                            }
                        }
                    });
            });
        if let Some(b) = pick {
            self.add_printer.brand = Some(b);
            self.add_printer.brands_open = false;
        }
    }

    /// One model card; true when clicked.
    fn model_card(&mut self, ui: &mut egui::Ui, card: Rect, mi: usize) -> bool {
        let p = theme::of(ui);
        let m = self.add_printer.catalog.as_ref().expect("built").models[mi].clone();
        let r = ui.interact(card, egui::Id::unique(("model-card", mi)), Sense::click());
        let hot = r.hovered();
        ui.painter()
            .rect_filled(card, 12., if hot { p.changed_row } else { p.panel });
        ui.painter().rect_stroke(
            card,
            12.,
            Stroke::new(1., if hot { p.accent } else { p.border_raised }),
            egui::StrokeKind::Inside,
        );
        let inner = card.shrink(14.);
        let image = Rect::from_min_size(inner.min, Vec2::new(inner.width(), 150.));
        let v = m.default_variant();
        let texture = self
            .add_printer
            .images
            .get(ui.ctx(), &m.asset_dir, v.image.as_deref());
        picture(ui, image, 8., texture.as_ref());
        let y = image.bottom() + 12.;
        let size = m.volume_text().replace(" mm", "");
        let sg = ui
            .painter()
            .layout_no_wrap(size.clone(), theme::sans(12.), p.muted_alt);
        let size_w = sg.size().x;
        ui.painter()
            .galley(Pos2::new(inner.right() - size_w, y + 3.), sg, p.muted_alt);
        line(
            ui,
            Pos2::new(inner.left(), y),
            &m.name,
            theme::semibold(15.),
            p.text,
            inner.width() - size_w - 8.,
        );
        let mut x = inner.left();
        let cy = y + 28.;
        for (text, accent) in m.chips() {
            let (fg, bg) = if accent {
                (p.mark_fg, p.accent_surface)
            } else {
                (p.label, p.raised)
            };
            let g = ui
                .painter()
                .layout_no_wrap(text.clone(), theme::sans(11.), fg);
            if x + g.size().x + 16. > inner.right() {
                break;
            }
            let r = pill(ui, Pos2::new(x, cy), &text, fg, bg, false, false);
            x = r.right() + 6.;
        }
        if hot {
            ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
        }
        r.clicked()
    }

    // ---- Screen 3: set it up -------------------------------------------------------------

    /// Opens Setup for model `mi` (`choice`: preselected, else the model's default).
    pub(crate) fn begin_setup(
        &mut self,
        mi: usize,
        choice: Option<Choice>,
        connect: Option<Connect>,
        back: Screen,
    ) {
        self.catalog();
        let m = &self.add_printer.catalog.as_ref().expect("built").models[mi];
        let mut choice = choice.unwrap_or_else(|| m.default_choice());
        m.fix(&mut choice);
        let name = unique_name(&self.printers.list, &default_name(m, &choice));
        let mut connect = connect.unwrap_or_default();
        if connect.kind.is_none() {
            connect.kind = Some(kind_for(m));
        }
        self.add_printer.setup = Some(Setup {
            model: mi,
            choice,
            name,
            name_touched: false,
            connect,
            back,
        });
        self.add_printer.error = None;
        self.add_printer.screen = Screen::Setup;
        let key = self.add_printer.catalog.as_ref().expect("built").models[mi]
            .vendor
            .clone();
        self.prefetch_vendor(&key, || {});
    }

    fn poll_setup_test(&mut self) {
        let Some(s) = &mut self.add_printer.setup else {
            return;
        };
        let Some(Test::Running(rx)) = &s.connect.test else {
            return;
        };
        match rx.try_recv() {
            Ok(Ok(r)) => {
                // A Moonraker camera found by the test is shown right away.
                if !r.cameras.is_empty() {
                    s.connect.camera_shown = true;
                }
                if let Some(m) = &r.model {
                    s.connect.model = Some(m.clone());
                }
                s.connect.test = Some(Test::Done(r));
            }
            Ok(Err(e)) => s.connect.test = Some(Test::Failed(e)),
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                s.connect.test = Some(Test::Failed(Explanation {
                    headline: "The test stopped unexpectedly".into(),
                    advice: String::new(),
                }));
            }
        }
    }

    /// The entry Setup would save (without its secret).
    fn setup_printer(&self) -> Option<(Printer, String)> {
        let s = self.add_printer.setup.as_ref()?;
        let m = self.add_printer.catalog.as_ref()?.models.get(s.model)?;
        let v = m.resolve(&s.choice)?;
        let vendor = v.vendor.clone();
        let mut printer = Printer::new(s.connect.kind.unwrap_or_else(|| kind_for(m)));
        printer.name = s.name.trim().to_string();
        printer.address = s.connect.address.trim().to_string();
        printer.profile = Some(format!("{vendor}/{}", v.preset));
        printer.model = s.connect.model.clone();
        printer.camera = Some(s.connect.camera.trim().to_string()).filter(|c| !c.is_empty());
        Some((printer, vendor))
    }

    fn setup_screen(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let p = theme::of(ui);
        let Some(si) = self.add_printer.setup.as_ref().map(|s| s.model) else {
            self.add_printer.screen = Screen::Gallery;
            return;
        };
        let m = self.add_printer.catalog.as_ref().expect("built").models[si].clone();
        // Left column: picture, model, steps.
        let col = Rect::from_min_size(rect.min, Vec2::new(300., rect.height()));
        ui.painter().rect_filled(
            col,
            egui::CornerRadius {
                nw: 12,
                sw: 12,
                ne: 0,
                se: 0,
            },
            p.panel,
        );
        ui.painter().line_segment(
            [col.right_top(), col.right_bottom()],
            Stroke::new(1., p.border_soft),
        );
        let inner = col.shrink(24.);
        let choice = self
            .add_printer
            .setup
            .as_ref()
            .expect("setup")
            .choice
            .clone();
        let variant = m
            .resolve(&choice)
            .cloned()
            .unwrap_or_else(|| m.default_variant().clone());
        let texture = self
            .add_printer
            .images
            .get(ui.ctx(), &m.asset_dir, variant.image.as_deref());
        let image = Rect::from_min_size(inner.min, Vec2::new(inner.width(), 200.));
        picture(ui, image, 10., texture.as_ref());
        let mut y = image.bottom() + 14.;
        line(
            ui,
            Pos2::new(inner.left(), y),
            &m.name,
            theme::semibold(17.),
            p.text,
            inner.width(),
        );
        y += 24.;
        let mut facts = vec![];
        if let Some([a, b, c]) = variant.volume {
            facts.push(format!("{a:.0} × {b:.0} × {c:.0} mm"));
        }
        if variant.input_shaper {
            facts.push("Input shaper".into());
        }
        line(
            ui,
            Pos2::new(inner.left(), y),
            &facts.join(" · "),
            theme::sans(13.),
            p.muted_alt,
            inner.width(),
        );
        y += 34.;
        let connected = matches!(
            self.add_printer
                .setup
                .as_ref()
                .and_then(|s| s.connect.test.as_ref()),
            Some(Test::Done(_))
        );
        for (n, (label, state)) in [
            ("Model", 2u8),
            ("Your setup", 1),
            ("Connect (optional)", if connected { 2 } else { 0 }),
        ]
        .into_iter()
        .enumerate()
        {
            let c = Pos2::new(inner.left() + 11., y + 11.);
            match state {
                2 => {
                    ui.painter()
                        .circle_filled(c, 11., p.success.gamma_multiply(0.18));
                    check_mark(ui.painter(), c, 9., p.success);
                }
                1 => {
                    ui.painter().circle_filled(c, 11., p.accent);
                    ui.painter().text(
                        c,
                        Align2::CENTER_CENTER,
                        (n + 1).to_string(),
                        theme::semibold(12.),
                        p.on_accent,
                    );
                }
                _ => {
                    ui.painter()
                        .circle_stroke(c, 10.5, Stroke::new(1., p.border_raised));
                    ui.painter().text(
                        c,
                        Align2::CENTER_CENTER,
                        (n + 1).to_string(),
                        theme::sans(12.),
                        p.muted,
                    );
                }
            }
            ui.painter().text(
                Pos2::new(inner.left() + 32., c.y),
                Align2::LEFT_CENTER,
                label,
                if state == 1 {
                    theme::semibold(13.)
                } else {
                    theme::sans(13.)
                },
                match state {
                    2 => p.success,
                    1 => p.text,
                    _ => p.muted,
                },
            );
            y += 32.;
        }
        // Right side.
        let right = Rect::from_min_max(
            Pos2::new(col.right() + 28., rect.top() + 24.),
            Pos2::new(rect.right() - 28., rect.bottom() - 24.),
        );
        let short = m
            .name
            .strip_prefix(&format!("{} ", m.brand))
            .unwrap_or(&m.name)
            .to_string();
        let mut y = right.top();
        ui.painter().text(
            Pos2::new(right.left(), y),
            Align2::LEFT_TOP,
            format!("How is your {short} set up?"),
            theme::semibold(20.),
            p.text,
        );
        y += 44.;
        let mut changed = false;
        let mut new_choice = choice.clone();
        // Tools, and input shaper beside them when the model comes both ways.
        let tools = m.tool_options();
        let show_tools = tools.len() > 1 || tools.first().is_some_and(|t| t.count > 1);
        let mut x = right.left();
        if show_tools {
            ui.painter().text(
                Pos2::new(x, y),
                Align2::LEFT_TOP,
                "Tools",
                theme::sans(13.),
                p.muted_alt,
            );
            for (i, t) in tools.iter().enumerate() {
                let label = t.label();
                let w = (ui
                    .painter()
                    .layout_no_wrap(label.clone(), theme::semibold(13.), p.text)
                    .size()
                    .x
                    + 28.)
                    .max(52.);
                let r = Rect::from_min_size(Pos2::new(x, y + LABEL), Vec2::new(w, SEG_H));
                if segment(
                    ui,
                    r,
                    egui::Id::unique(("setup-tools", i)),
                    &label,
                    *t == choice.tools,
                    true,
                ) {
                    new_choice.tools = t.clone();
                    changed = true;
                }
                x = r.right() + 8.;
            }
            x += 24.;
        }
        if m.has_shaper_choice() {
            ui.painter().text(
                Pos2::new(x, y),
                Align2::LEFT_TOP,
                "Input shaper",
                theme::sans(13.),
                p.muted_alt,
            );
            for (i, (label, on)) in [("On", true), ("Off (old firmware)", false)]
                .into_iter()
                .enumerate()
            {
                let w = (ui
                    .painter()
                    .layout_no_wrap(label.to_string(), theme::semibold(13.), p.text)
                    .size()
                    .x
                    + 28.)
                    .max(52.);
                let r = Rect::from_min_size(Pos2::new(x, y + LABEL), Vec2::new(w, SEG_H));
                if segment(
                    ui,
                    r,
                    egui::Id::unique(("setup-shaper", i)),
                    label,
                    choice.input_shaper == on,
                    true,
                ) {
                    new_choice.input_shaper = on;
                    changed = true;
                }
                x = r.right() + 8.;
            }
        }
        if show_tools || m.has_shaper_choice() {
            y += LABEL + SEG_H + ROW_GAP;
        }
        // Build size (RatRig V-Core 300 / 400 / 500, Voron 250 / 300 / 350) and mode (an
        // IDEX printer's copy / mirror profiles), beside each other.
        let sizes = m.size_options();
        let modes = m.mode_options();
        if !sizes.is_empty() || !modes.is_empty() {
            let mut x = right.left();
            for (row, title, options) in [(0, "Size", &sizes), (1, "Mode", &modes)] {
                if options.is_empty() {
                    continue;
                }
                ui.painter().text(
                    Pos2::new(x, y),
                    Align2::LEFT_TOP,
                    title,
                    theme::sans(13.),
                    p.muted_alt,
                );
                for (i, o) in options.iter().enumerate() {
                    let label = match (row, o.as_str()) {
                        (0, s) => format!("{s} mm"),
                        (_, "") => "Normal".to_string(),
                        (_, s) => s.to_string(),
                    };
                    let w = (ui
                        .painter()
                        .layout_no_wrap(label.clone(), theme::semibold(13.), p.text)
                        .size()
                        .x
                        + 28.)
                        .max(52.);
                    let r = Rect::from_min_size(Pos2::new(x, y + LABEL), Vec2::new(w, SEG_H));
                    let on = if row == 0 {
                        *o == choice.size
                    } else {
                        *o == choice.mode
                    };
                    let mut probe = choice.clone();
                    if row == 0 {
                        probe.size = o.clone();
                    } else {
                        probe.mode = o.clone();
                    }
                    let valid = m
                        .variants
                        .iter()
                        .any(|v| v.size == probe.size && v.mode == probe.mode);
                    if segment(
                        ui,
                        r,
                        egui::Id::unique(("setup-variant", row, i)),
                        &label,
                        on,
                        valid || row == 0,
                    ) {
                        new_choice.size = probe.size;
                        new_choice.mode = probe.mode;
                        changed = true;
                    }
                    x = r.right() + 8.;
                }
                x += 24.;
            }
            y += LABEL + SEG_H + ROW_GAP;
        }
        ui.painter().text(
            Pos2::new(right.left(), y),
            Align2::LEFT_TOP,
            "Nozzle",
            theme::sans(13.),
            p.muted_alt,
        );
        let mut x = right.left();
        let mut row_y = y + LABEL;
        for (i, n) in m.nozzle_options().iter().enumerate() {
            let label = n.label();
            let valid = m.nozzle_valid(&choice, &n.raw);
            let w = ui
                .painter()
                .layout_no_wrap(label.clone(), theme::semibold(13.), p.text)
                .size()
                .x
                + 28.;
            if x + w > right.right() {
                x = right.left();
                row_y += SEG_H + 8.;
            }
            let r = Rect::from_min_size(Pos2::new(x, row_y), Vec2::new(w, SEG_H));
            if segment(
                ui,
                r,
                egui::Id::unique(("setup-nozzle", i)),
                &label,
                n.raw == choice.nozzle,
                valid,
            ) {
                new_choice.nozzle = n.raw.clone();
                changed = true;
            }
            x = r.right() + 8.;
        }
        y = row_y + SEG_H + 6.;
        if choice.tools.count > 1 && choice.tools.mmu.is_none() {
            ui.painter().text(
                Pos2::new(right.left(), y),
                Align2::LEFT_TOP,
                "Different nozzles per tool? You can change each tool later.",
                theme::sans(12.),
                p.muted,
            );
            y += 18.;
        }
        y += ROW_GAP - 6.;
        if changed {
            let s = self.add_printer.setup.as_mut().expect("setup");
            m.fix(&mut new_choice);
            s.choice = new_choice;
            if !s.name_touched {
                s.name = unique_name(&self.printers.list, &default_name(&m, &s.choice));
            }
        }
        // Name.
        ui.painter().text(
            Pos2::new(right.left(), y),
            Align2::LEFT_TOP,
            "Name",
            theme::sans(13.),
            p.muted_alt,
        );
        let name_r = Rect::from_min_size(
            Pos2::new(right.left(), y + LABEL),
            Vec2::new(right.width(), 38.),
        );
        {
            let s = self.add_printer.setup.as_mut().expect("setup");
            if field(
                ui,
                name_r,
                egui::Id::unique("setup-name"),
                &mut s.name,
                "e.g. XL in the workshop",
                false,
                false,
                false,
            )
            .changed()
            {
                s.name_touched = true;
            }
        }
        y = name_r.bottom() + ROW_GAP;
        // The connection, as tall as its content.
        let boxr = Rect::from_min_size(
            Pos2::new(right.left(), y),
            Vec2::new(right.width(), CONNECT_H),
        );
        self.connect_box(ui, boxr, &m);
        // Back / Add.
        let back = self
            .add_printer
            .setup
            .as_ref()
            .map_or(Screen::Gallery, |s| s.back);
        if text_link(
            ui,
            Pos2::new(right.left(), right.bottom() - 20.),
            Align2::LEFT_CENTER,
            egui::Id::unique("setup-back"),
            "← Back",
            p.muted_alt,
            p.text,
        ) {
            self.add_printer.screen = back;
        }
        let name = self
            .add_printer
            .setup
            .as_ref()
            .map(|s| s.name.trim().to_string())
            .unwrap_or_default();
        let loading = self.vendor_loading();
        let label = if loading {
            format!("Loading {} profiles…", m.brand)
        } else if name.is_empty() {
            "Add printer".to_string()
        } else {
            format!("Add {name}")
        };
        let w = (ui
            .painter()
            .layout_no_wrap(label.clone(), theme::semibold(13.), p.text)
            .size()
            .x
            + 40.)
            .min(360.);
        let add = Rect::from_min_size(
            Pos2::new(right.right() - w, right.bottom() - 40.),
            Vec2::new(w, 40.),
        );
        let valid = m.resolve(&choice).is_some() && !name.is_empty() && !loading;
        if loading {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(50));
        }
        if button_in(
            ui,
            add,
            egui::Id::unique("setup-add"),
            &label,
            Style::Primary,
            valid,
        )
        .clicked()
        {
            self.finish_setup();
        }
        if let Some(e) = &self.add_printer.error {
            line(
                ui,
                Pos2::new(right.left() + 70., right.bottom() - 28.),
                e,
                theme::sans(12.),
                p.warn_text,
                add.left() - right.left() - 84.,
            );
        }
    }

    fn connect_box(&mut self, ui: &mut egui::Ui, boxr: Rect, m: &Model) {
        let p = theme::of(ui);
        ui.painter().rect_filled(boxr, 10., p.panel);
        ui.painter().rect_stroke(
            boxr,
            10.,
            Stroke::new(1., p.border_soft),
            egui::StrokeKind::Inside,
        );
        let inner = boxr.shrink2(Vec2::new(16., 14.));
        if !connectable(m) {
            ui.painter().text(
                inner.min,
                Align2::LEFT_TOP,
                "Connect to send prints",
                theme::semibold(13.),
                p.text,
            );
            let mut job = egui::text::LayoutJob::simple(
                format!(
                    "Mica cannot send prints to {} printers yet. Add it to slice and save the G-code; \
                     copy the file to the printer yourself.",
                    m.brand
                ),
                theme::sans(13.),
                p.muted_alt,
                inner.width(),
            );
            job.wrap.max_rows = 3;
            let g = ui.painter().layout_job(job);
            ui.painter()
                .galley(Pos2::new(inner.left(), inner.top() + 28.), g, p.muted_alt);
            return;
        }
        let mut y = inner.top();
        ui.painter().text(
            Pos2::new(inner.left(), y),
            Align2::LEFT_TOP,
            "Connect to send prints",
            theme::semibold(13.),
            p.text,
        );
        ui.painter().text(
            Pos2::new(inner.right(), y + 1.),
            Align2::RIGHT_TOP,
            "optional · can be done later",
            theme::sans(12.),
            p.muted,
        );
        y += 28.;
        let half = ((inner.width() - 10.) / 2.).floor();
        let kind = self
            .add_printer
            .setup
            .as_ref()
            .and_then(|s| s.connect.kind)
            .unwrap_or_else(|| kind_for(m));
        let store_ok = self.printers.secrets.available();
        let (key_hint, addr_hint) = match kind {
            Kind::PrusaLink => ("PrusaLink API key", "192.0.2.42 or prusa-xl.local"),
            Kind::SnapmakerU1 => ("API key (optional)", "192.0.2.57:7125 or u1.local:7125"),
            _ => (
                "API key (optional)",
                "192.0.2.57:7125 or printer.local:7125",
            ),
        };
        let needs_key;
        let mut test_now = false;
        {
            let s = self.add_printer.setup.as_mut().expect("setup");
            needs_key = s.connect.needs_key;
            let a = Rect::from_min_size(Pos2::new(inner.left(), y), Vec2::new(half, 36.));
            if field(
                ui,
                a,
                egui::Id::unique("setup-address"),
                &mut s.connect.address,
                addr_hint,
                true,
                false,
                false,
            )
            .changed()
            {
                s.connect.test = None;
            }
            let k = Rect::from_min_size(Pos2::new(a.right() + 10., y), Vec2::new(half, 36.));
            let hint = if store_ok {
                key_hint.to_string()
            } else {
                format!("{key_hint} (cannot be saved here)")
            };
            let highlight = needs_key && s.connect.secret.is_empty();
            if field(
                ui,
                k,
                egui::Id::unique("setup-key"),
                &mut s.connect.secret,
                &hint,
                true,
                true,
                highlight,
            )
            .changed()
            {
                s.connect.test = None;
            }
        }
        y += 46.;
        let has_address = self
            .add_printer
            .setup
            .as_ref()
            .is_some_and(|s| !s.connect.address.trim().is_empty());
        let testing = matches!(
            self.add_printer
                .setup
                .as_ref()
                .and_then(|s| s.connect.test.as_ref()),
            Some(Test::Running(_))
        );
        let t = Rect::from_min_size(Pos2::new(inner.left(), y), Vec2::new(132., 32.));
        if button_in(
            ui,
            t,
            egui::Id::unique("setup-test"),
            if testing {
                "Testing…"
            } else {
                "Test connection"
            },
            Style::Secondary,
            has_address && !testing,
        )
        .clicked()
        {
            test_now = true;
        }
        let tx = t.right() + 12.;
        let tw = inner.right() - tx;
        let result = self
            .add_printer
            .setup
            .as_ref()
            .and_then(|s| match &s.connect.test {
                None if needs_key => Some((
                    format!(
                        "This printer asks for its {}. Paste it, then test.",
                        if kind == Kind::PrusaLink {
                            "API key (Settings → Network → PrusaLink)"
                        } else {
                            "API key"
                        }
                    ),
                    p.warn_text,
                )),
                None => None,
                Some(Test::Running(_)) => Some(("Asking the printer…".into(), p.muted)),
                Some(Test::Failed(e)) => {
                    Some((format!("{} — {}", e.headline, e.advice), theme::DANGER))
                }
                Some(Test::Done(r)) => Some((connected_summary(r), p.success)),
            });
        let done = matches!(
            self.add_printer
                .setup
                .as_ref()
                .and_then(|s| s.connect.test.as_ref()),
            Some(Test::Done(_))
        );
        if let Some((text, colour)) = result {
            let (tx, tw) = if done {
                check_mark(ui.painter(), Pos2::new(tx + 6., t.center().y), 10., colour);
                (tx + 18., tw - 18.)
            } else {
                (tx, tw)
            };
            let mut job = egui::text::LayoutJob::simple(text, theme::sans(13.), colour, tw);
            job.wrap.max_rows = 2;
            let g = ui.painter().layout_job(job);
            let gy = t.center().y - g.size().y / 2.;
            ui.painter().galley(Pos2::new(tx, gy), g, colour);
        }
        y += 44.;
        // Camera.
        ui.painter().line_segment(
            [
                Pos2::new(inner.left(), y - 4.),
                Pos2::new(inner.right(), y - 4.),
            ],
            Stroke::new(1., p.border_soft),
        );
        y += 8.;
        ui.painter().text(
            Pos2::new(inner.left(), y),
            Align2::LEFT_TOP,
            "Camera",
            theme::semibold(13.),
            p.text,
        );
        let preview = Rect::from_min_size(Pos2::new(inner.right() - 160., y), Vec2::new(160., 90.));
        let text_w = preview.left() - inner.left() - 16.;
        let show_preview = if kind == Kind::PrusaLink {
            line(
                ui,
                Pos2::new(inner.left(), y + 22.),
                "Camera via Prusa Connect (not supported yet).",
                theme::sans(12.),
                p.muted,
                text_w,
            );
            let cr = Rect::from_min_size(
                Pos2::new(inner.left(), y + 44.),
                Vec2::new(text_w - 72. - 8., 32.),
            );
            let ct = Rect::from_min_size(Pos2::new(cr.right() + 8., cr.top()), Vec2::new(72., 32.));
            let s = self.add_printer.setup.as_mut().expect("setup");
            if field(
                ui,
                cr,
                egui::Id::unique("setup-camera"),
                &mut s.connect.camera,
                "Camera address (optional): rtsp://… or http://…/snapshot.jpg",
                true,
                false,
                false,
            )
            .changed()
            {
                s.connect.camera_shown = false;
            }
            let has = !s.connect.camera.trim().is_empty();
            if button_in(
                ui,
                ct,
                egui::Id::unique("setup-camera-test"),
                "Test",
                Style::Secondary,
                has,
            )
            .clicked()
            {
                s.connect.camera_shown = true;
                self.add_printer.cameras.forget(u64::MAX);
            }
            self.add_printer
                .setup
                .as_ref()
                .is_some_and(|s| s.connect.camera_shown && !s.connect.camera.trim().is_empty())
        } else {
            let reported = self
                .add_printer
                .setup
                .as_ref()
                .and_then(|s| match &s.connect.test {
                    Some(Test::Done(r)) => Some(r.cameras.clone()),
                    _ => None,
                });
            let text = match &reported {
                None => "Mica looks for the printer's camera when you test the connection.".to_string(),
                Some(c) if c.is_empty() => "The printer reports no camera. You can enter a camera URL later in My printers.".into(),
                Some(c) => {
                    // HLS and WebRTC other than camera-streamer's are not played (yet): Mica
                    // shows the camera's snapshots.
                    let played = matches!(
                        c[0].service,
                        mica_printhost::CameraService::Mjpeg
                            | mica_printhost::CameraService::Rtsp
                            | mica_printhost::CameraService::Snapshot
                    ) || c[0]
                        .service_name
                        .eq_ignore_ascii_case("webrtc-camerastreamer");
                    format!(
                        "{} · {}{} — found and selected.",
                        c[0].name,
                        c[0].service.label(),
                        if played { "" } else { ", shown as snapshots" }
                    )
                }
            };
            let mut job = egui::text::LayoutJob::simple(
                text,
                theme::sans(12.),
                if reported.as_ref().is_some_and(|c| !c.is_empty()) {
                    p.success
                } else {
                    p.muted
                },
                text_w,
            );
            job.wrap.max_rows = 3;
            let g = ui.painter().layout_job(job);
            ui.painter()
                .galley(Pos2::new(inner.left(), y + 22.), g, p.muted);
            reported.is_some_and(|c| !c.is_empty())
        };
        if show_preview {
            self.setup_camera_preview(ui, preview);
        }
        if test_now {
            self.test_setup(ui.ctx());
        }
    }

    /// The small live snapshot of the camera Setup would save.
    fn setup_camera_preview(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let p = theme::of(ui);
        let Some((mut printer, _)) = self.setup_printer() else {
            return;
        };
        if printer.name.is_empty() {
            printer.name = "printer".into();
        }
        let reported = self
            .add_printer
            .setup
            .as_ref()
            .and_then(|s| match &s.connect.test {
                Some(Test::Done(r)) => Some(r.cameras.clone()),
                _ => None,
            });
        ui.painter()
            .rect_filled(rect, 8., Color32::from_rgb(10, 10, 12));
        let choice = match cameras::choose(&printer, reported.as_deref(), Role::Thumbnail) {
            Ok(c) => c,
            Err(cameras::NoCamera::BadUrl(e) | cameras::NoCamera::Unsupported(e)) => {
                centred(ui, rect, &e, p.warn_text);
                return;
            }
            Err(_) => return,
        };
        let secret = self
            .add_printer
            .setup
            .as_ref()
            .map(|s| s.connect.secret.clone());
        let ctx = ui.ctx().clone();
        let shown = self.add_printer.cameras.show(
            &ctx,
            u64::MAX,
            Role::Thumbnail,
            &choice,
            || crate::printhost::unsaved_connector(&printer, secret.as_deref()),
            || Ok(None),
            mica_camera::Rate {
                fps: 0.5,
                max_width: 320,
            },
        );
        match shown {
            Ok(view) => {
                if let Some(t) = view.texture() {
                    let size = t.size_vec2();
                    let scale = (rect.width() / size.x).min(rect.height() / size.y);
                    let r = Rect::from_center_size(rect.center(), size * scale);
                    ui.painter().image(
                        t.id(),
                        r,
                        Rect::from_min_max(Pos2::ZERO, Pos2::new(1., 1.)),
                        Color32::WHITE,
                    );
                } else {
                    let status = view.status();
                    let text = match &status.error {
                        Some(e) if !view.starting() => e.clone(),
                        _ => "Connecting…".into(),
                    };
                    centred(ui, rect, &text, Color32::from_gray(190));
                }
            }
            Err(e) => centred(ui, rect, &e, p.warn_text),
        }
    }

    fn test_setup(&mut self, ctx: &egui::Context) {
        let Some((printer, _)) = self.setup_printer() else {
            return;
        };
        let secret = self
            .add_printer
            .setup
            .as_ref()
            .map(|s| s.connect.secret.clone());
        let c = ctx.clone();
        let rx = self
            .printers
            .test_unsaved(&printer, secret.as_deref(), move || c.request_repaint());
        if let Some(s) = &mut self.add_printer.setup {
            s.connect.test = Some(Test::Running(rx));
        }
        self.add_printer.cameras.forget(u64::MAX);
    }

    /// "Add <name>": the profile into My printers (and selected), the connection into
    /// printers.toml when an address was given.
    pub(crate) fn finish_setup(&mut self) {
        let Some((printer, vendor)) = self.setup_printer() else {
            self.add_printer.error =
                Some("Choose a tool count and nozzle that this model comes with.".into());
            return;
        };
        let preset = printer
            .profile
            .as_deref()
            .and_then(|p| p.split_once('/'))
            .map(|(_, n)| n.to_string())
            .unwrap_or_default();
        // The vendor's full bundle: loaded by Setup on a thread, or now (a one-click add).
        if self
            .add_printer
            .vendor_load
            .as_ref()
            .is_some_and(|l| l.key == vendor)
        {
            self.poll_vendor_load(true);
        }
        let vi = match self.profiles.ensure_vendor(&vendor) {
            Ok(vi) => vi,
            Err(e) => {
                self.add_printer.error = Some(e);
                self.add_printer.screen = Screen::Setup;
                return;
            }
        };
        if !self.profiles.printer_exists(vi, &preset) {
            self.add_printer.error = Some(format!("{preset} is not in the {vendor} profiles."));
            self.add_printer.screen = Screen::Setup;
            return;
        }
        let name = printer.name.clone();
        let connected = !printer.address.is_empty();
        if connected {
            let secret = self
                .add_printer
                .setup
                .as_ref()
                .map(|s| s.connect.secret.clone())
                .filter(|s| !s.is_empty());
            if let Err(e) = self.printers.put(None, printer, secret.as_deref()) {
                self.add_printer.error = Some(e);
                self.add_printer.screen = Screen::Setup;
                return;
            }
        }
        self.prefs.add_printer(&vendor, &preset);
        self.prefs_dirty = true;
        self.switch_printer(vi, &preset);
        self.add_printer.setup = None;
        self.add_printer.error = None;
        self.dialogs.add_printer = false;
        self.dialogs.my_printers = true;
        self.printers_editor.view = super::printers::View::Cards;
        use crate::notifications::{Level, Notification};
        let text = if connected {
            format!("{name} is added and connected. It is the current printer now.")
        } else {
            format!("{preset} is added. Connect it any time in My printers.")
        };
        self.notify(Notification::new(
            Level::Info,
            format!("Added {}", if connected { name } else { preset }),
            text,
        ));
    }
}

/// "Connected · firmware 6.2.3 · 5 tools · PLA, PETG, ASA, PLA loaded".
fn connected_summary(r: &crate::printhost::TestReport) -> String {
    let mut parts = vec!["Connected".to_string()];
    if let Some(f) = &r.firmware {
        parts.push(format!("firmware {f}"));
    }
    if let Some(t) = &r.tools
        && t.len() > 1
    {
        parts.push(format!("{} tools", t.len()));
    }
    let loaded: Vec<String> = r
        .filaments
        .iter()
        .filter_map(|f| f.material.clone())
        .collect();
    if !loaded.is_empty() {
        parts.push(format!("{} loaded", loaded.join(", ")));
    }
    parts.join(" · ")
}

/// A check mark of width `size` centred at `c`.
pub fn check_mark(painter: &egui::Painter, c: Pos2, size: f32, colour: Color32) {
    let s = size / 2.;
    painter.line(
        vec![
            Pos2::new(c.x - s, c.y + s * 0.05),
            Pos2::new(c.x - s * 0.3, c.y + s * 0.7),
            Pos2::new(c.x + s, c.y - s * 0.65),
        ],
        Stroke::new(1.8, colour),
    );
}

fn centred(ui: &egui::Ui, rect: Rect, text: &str, colour: Color32) {
    let mut job = egui::text::LayoutJob::simple(
        text.to_string(),
        theme::sans(11.),
        colour,
        rect.width() - 16.,
    );
    job.halign = egui::Align::Center;
    job.wrap.max_rows = 3;
    let g = ui.painter().layout_job(job);
    ui.painter().galley(
        Pos2::new(rect.center().x, rect.center().y - g.size().y / 2.),
        g,
        colour,
    );
}

/// A segmented choice button; true when clicked.
fn segment(ui: &mut egui::Ui, r: Rect, id: egui::Id, label: &str, on: bool, enabled: bool) -> bool {
    let p = theme::of(ui);
    let resp = ui.interact(
        r,
        id,
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    let hot = enabled && resp.hovered();
    let fill = if on {
        p.accent_surface
    } else if hot {
        p.hover
    } else {
        p.raised
    };
    ui.painter().rect_filled(r, 8., fill);
    ui.painter().rect_stroke(
        r,
        8.,
        Stroke::new(1., if on { p.accent } else { p.border_raised }),
        egui::StrokeKind::Inside,
    );
    let colour = if !enabled {
        p.muted.gamma_multiply(0.6)
    } else {
        p.text
    };
    ui.painter().text(
        r.center(),
        Align2::CENTER_CENTER,
        label,
        if on {
            theme::semibold(13.)
        } else {
            theme::sans(13.)
        },
        colour,
    );
    if hot {
        ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
    }
    if !enabled {
        resp.on_hover_text("This model has no profile for that combination");
        return false;
    }
    resp.clicked()
}

/// Whether Mica can talk to this brand's printers: Bambu Lab's own protocol is not supported
/// (their printers are listed for slicing only).
pub fn connectable(m: &Model) -> bool {
    m.vendor != "BBL"
}

/// The backend a model's printers usually speak.
pub fn kind_for(m: &Model) -> Kind {
    if m.vendor == "Prusa" {
        Kind::PrusaLink
    } else if m.name.contains("U1") {
        Kind::SnapmakerU1
    } else {
        Kind::Moonraker
    }
}

/// `name`, or `name 2`, `name 3`… when a printer of that name exists.
pub fn unique_name(list: &[Printer], name: &str) -> String {
    let taken = |n: &str| {
        list.iter()
            .any(|p| p.name.trim().eq_ignore_ascii_case(n.trim()))
    };
    if !taken(name) {
        return name.to_string();
    }
    (2..)
        .map(|i| format!("{name} {i}"))
        .find(|n| !taken(n))
        .expect("a free name")
}

/// Whether two printer addresses name the same host and port (scheme and slash ignored).
pub fn same_address(a: &str, b: &str) -> bool {
    let norm = |s: &str| {
        let s = s.trim().trim_end_matches('/');
        let s = s.split_once("://").map_or(s, |(_, r)| r);
        let s = s.strip_suffix(":80").unwrap_or(s);
        s.to_ascii_lowercase()
    };
    norm(a) == norm(b)
}

/// What a headless screenshot of the Add printer window shows (`--add-printer …`).
pub struct Shot {
    pub screen: String,
    pub mdns: Option<String>,
    pub search: Option<String>,
    pub brand: Option<String>,
    pub model: Option<String>,
    pub tools: Option<usize>,
    pub nozzle: Option<String>,
    pub address: Option<String>,
    pub key: Option<String>,
    pub camera: Option<String>,
    pub test: bool,
}

/// Puts the window into the state a screenshot asks for, waiting for the network search and
/// the connection test. Only loopback addresses are ever contacted.
pub fn screenshot_setup(app: &mut App, ctx: &egui::Context, shot: &Shot) -> Result<(), String> {
    crate::printhost::restrict_to_loopback();
    if let Some(m) = &shot.mdns {
        let target: std::net::SocketAddr = m.parse().map_err(|_| "--mdns 127.0.0.1:PORT")?;
        if !target.ip().is_loopback() {
            return Err("--mdns only takes a loopback address".into());
        }
        app.add_printer.discovery.target = Some(target);
    }
    app.open_add_printer();
    app.load_gallery_index_now();
    let wait = |done: &mut dyn FnMut() -> bool| {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while !done() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    };
    match shot.screen.as_str() {
        "found" => {
            app.add_printer.discovery.start(|| {});
            wait(&mut || {
                app.add_printer.discovery.poll();
                !app.add_printer.discovery.running()
            });
            app.add_printer.was_open = true;
        }
        "gallery" => {
            app.add_printer.was_open = true;
            app.add_printer.screen = Screen::Gallery;
            app.add_printer.search = shot.search.clone().unwrap_or_default();
            app.add_printer.brand = shot.brand.clone();
        }
        "setup" => {
            app.add_printer.was_open = true;
            let name = shot
                .model
                .clone()
                .ok_or("--add-printer setup needs --add-printer-model NAME")?;
            let mi = app
                .catalog()
                .models
                .iter()
                .position(|m| m.name == name)
                .ok_or_else(|| format!("--add-printer-model: no model named {name}"))?;
            let m = app.catalog().models[mi].clone();
            let mut choice = m.default_choice();
            if let Some(t) = shot.tools
                && let Some(o) = m.tool_options().into_iter().find(|o| o.count == t)
            {
                choice.tools = o;
            }
            if let Some(n) = &shot.nozzle {
                choice.nozzle = n.clone();
            }
            let connect = Connect {
                address: shot.address.clone().unwrap_or_default(),
                secret: shot.key.clone().unwrap_or_default(),
                camera: shot.camera.clone().unwrap_or_default(),
                camera_shown: shot.camera.is_some(),
                ..Connect::default()
            };
            app.begin_setup(mi, Some(choice), Some(connect), Screen::Gallery);
            app.finish_vendor_load();
            if shot.test {
                app.test_setup(ctx);
                wait(&mut || {
                    app.poll_setup_test();
                    !matches!(
                        app.add_printer
                            .setup
                            .as_ref()
                            .and_then(|s| s.connect.test.as_ref()),
                        Some(Test::Running(_))
                    )
                });
            }
        }
        other => return Err(format!("--add-printer found|gallery|setup, not {other}")),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_stay_unique_and_addresses_compare_loosely() {
        let mut xl = Printer::new(Kind::PrusaLink);
        xl.name = "XL 5T".into();
        let list = vec![xl];
        assert_eq!(unique_name(&list, "XL 5T"), "XL 5T 2");
        assert_eq!(unique_name(&list, "CORE One"), "CORE One");
        assert!(same_address("http://192.0.2.42/", "192.0.2.42"));
        assert!(same_address("192.0.2.42:80", "192.0.2.42"));
        assert!(!same_address("192.0.2.42:7125", "192.0.2.42"));
        assert_eq!(
            state_text(Some(PrinterState::Printing), Some(43.2), "Idle"),
            "Printing 43 %"
        );
        assert_eq!(state_text(Some(PrinterState::Idle), None, "Ready"), "Ready");
    }
}
