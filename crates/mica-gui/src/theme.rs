// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! One palette for the whole application: egui widgets and the 3D viewport share it. The
//! colours, type sizes and radii follow the design mockups (dark surfaces `#121417` /
//! `#1a1d21` / `#22262b`, violet accent `#8b7cf6`, IBM Plex Sans for text and IBM Plex Mono
//! for numbers). Dark is the default; light is an explicit choice that is persisted.

use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Stroke, TextStyle,
    Vec2,
};
use std::sync::Arc;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Mode {
    #[default]
    Dark,
    Light,
}
impl Mode {
    /// The persisted name; anything that is not an explicit `light` resolves to dark, so a
    /// first run and the old `system` value both start dark.
    pub fn from_name(name: &str) -> Self {
        if name.trim().eq_ignore_ascii_case("light") {
            Self::Light
        } else {
            Self::Dark
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Dark => "dark",
            Self::Light => "light",
        }
    }
}

/// What the user chose in Preferences: a fixed mode, or whatever the operating system uses.
/// Stored as `dark`, `light` or `follow-system`; the older `system` value (from before dark
/// became the default) and anything unknown stay dark.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Choice {
    #[default]
    Dark,
    Light,
    System,
}
impl Choice {
    pub const ALL: [Self; 3] = [Self::Dark, Self::Light, Self::System];
    pub fn from_name(name: &str) -> Self {
        if name.trim().eq_ignore_ascii_case("follow-system") {
            return Self::System;
        }
        match Mode::from_name(name) {
            Mode::Dark => Self::Dark,
            Mode::Light => Self::Light,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Dark => Mode::Dark.name(),
            Self::Light => Mode::Light.name(),
            Self::System => "follow-system",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Dark => "Dark",
            Self::Light => "Light",
            Self::System => "Follow the system",
        }
    }
    /// The mode to draw with; `system` is what the OS reports (dark when it says nothing).
    pub fn resolve(self, system: Option<egui::Theme>) -> Mode {
        match self {
            Self::Dark => Mode::Dark,
            Self::Light => Mode::Light,
            Self::System => match system {
                Some(egui::Theme::Light) => Mode::Light,
                _ => Mode::Dark,
            },
        }
    }
}

/// Violet accent family (mockup values) and the two status colours.
pub const ACCENT: Color32 = Color32::from_rgb(0x8b, 0x7c, 0xf6);
pub const ACCENT_BRIGHT: Color32 = Color32::from_rgb(0xa5, 0x97, 0xf8);
pub const ACCENT_HI: Color32 = Color32::from_rgb(0xc4, 0xba, 0xfb);
pub const ACCENT_DIM: Color32 = Color32::from_rgb(0x6a, 0x55, 0xe8);
pub const WARNING: Color32 = Color32::from_rgb(0xf2, 0xa6, 0x5a);
pub const DANGER: Color32 = Color32::from_rgb(0xe5, 0x53, 0x4b);
/// Gizmo axis colours from the rotation mockup: X red, Y green, Z blue (+ its light tint).
pub const AXIS_X: Color32 = Color32::from_rgb(0xe5, 0x53, 0x4b);
pub const AXIS_Y: Color32 = Color32::from_rgb(0x56, 0xb4, 0x5b);
pub const AXIS_Z: Color32 = Color32::from_rgb(0x4d, 0x7c, 0xf0);
pub const AXIS_Z_LIGHT: Color32 = Color32::from_rgb(0x9d, 0xb5, 0xf7);
pub const OUTLINE: Color32 = Color32::from_rgb(0x0d, 0x0f, 0x11);
/// Hot (hovered or dragged) rotation handle, as the yellow grabber of PrusaSlicer / Orca.
pub const HANDLE_HOT: Color32 = Color32::from_rgb(0xf2, 0xd2, 0x5a);

/// Every surface and text colour the panels use, resolved for one mode.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    /// Window and viewport background.
    pub bg: Color32,
    /// Sidebar and floating cards.
    pub panel: Color32,
    pub header: Color32,
    /// Combos and buttons.
    pub raised: Color32,
    /// Number / text fields in the control column.
    pub field: Color32,
    pub field_border: Color32,
    /// Card separators.
    pub border: Color32,
    /// Floating card borders.
    pub border_soft: Color32,
    /// Button and combo borders.
    pub border_raised: Color32,
    pub text: Color32,
    pub label: Color32,
    pub muted: Color32,
    pub muted_alt: Color32,
    pub accent: Color32,
    pub accent_bright: Color32,
    pub accent_hi: Color32,
    /// Fill of the active segment / active toolbar button.
    pub accent_surface: Color32,
    pub changed_row: Color32,
    pub selected_row: Color32,
    pub scope_border: Color32,
    pub scope_text: Color32,
    /// Search match highlight (`<mark>`).
    pub mark_bg: Color32,
    pub mark_fg: Color32,
    pub search_bg: Color32,
    pub warn_surface: Color32,
    pub warn_border: Color32,
    pub warn_text: Color32,
    pub warn_button: Color32,
    pub warn_button_border: Color32,
    pub warn_dim: Color32,
    pub toggle_off: Color32,
    pub toggle_knob: Color32,
    pub toggle_knob_on: Color32,
    /// Dark read-out boxes (size label, tooltips, gizmo angle).
    pub dark_box: Color32,
    pub dark_box_text: Color32,
    /// Icon button hover fill.
    pub hover: Color32,
    /// Text on an accent-filled button.
    pub on_accent: Color32,
    /// Good news: a printer ready, a connection that works.
    pub success: Color32,
}

const fn rgb(v: u32) -> Color32 {
    Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

pub const DARK: Palette = Palette {
    bg: rgb(0x121417),
    panel: rgb(0x1a1d21),
    header: rgb(0x16191c),
    raised: rgb(0x22262b),
    field: rgb(0x262a30),
    field_border: rgb(0x2f343b),
    border: rgb(0x262a30),
    border_soft: rgb(0x2a2f35),
    border_raised: rgb(0x31363d),
    text: rgb(0xe6e8eb),
    label: rgb(0xc9cdd2),
    muted: rgb(0x8b929b),
    muted_alt: rgb(0x9aa1aa),
    accent: ACCENT,
    accent_bright: ACCENT_BRIGHT,
    accent_hi: ACCENT_HI,
    accent_surface: rgb(0x2e2a45),
    changed_row: rgb(0x221f30),
    selected_row: rgb(0x27233a),
    scope_border: rgb(0x3d3760),
    scope_text: rgb(0xb0a8d8),
    mark_bg: rgb(0x3a3366),
    mark_fg: rgb(0xe0dbff),
    search_bg: rgb(0x15181b),
    warn_surface: rgb(0x2a2418),
    warn_border: rgb(0x4a3b1e),
    warn_text: rgb(0xf2d29b),
    warn_button: rgb(0x3a3020),
    warn_button_border: rgb(0x5a4826),
    warn_dim: rgb(0xbfa77a),
    toggle_off: rgb(0x3a3f46),
    toggle_knob: rgb(0xc9cdd2),
    toggle_knob_on: rgb(0x110d1f),
    dark_box: rgb(0x0d0f11),
    dark_box_text: rgb(0xc9cdd2),
    hover: rgb(0x2a2f35),
    on_accent: rgb(0x110d1f),
    success: rgb(0x6fd39b),
};

pub const LIGHT: Palette = Palette {
    bg: rgb(0xeef0f3),
    panel: rgb(0xffffff),
    header: rgb(0xf7f8fa),
    raised: rgb(0xffffff),
    field: rgb(0xf6f7f9),
    field_border: rgb(0xd5d9df),
    border: rgb(0xe3e6ea),
    border_soft: rgb(0xdde1e6),
    border_raised: rgb(0xcfd4da),
    text: rgb(0x1d2126),
    label: rgb(0x3a3f46),
    muted: rgb(0x6b727b),
    muted_alt: rgb(0x7a818a),
    accent: ACCENT_DIM,
    accent_bright: rgb(0x5b46d6),
    accent_hi: rgb(0x4634b0),
    accent_surface: rgb(0xebe7fd),
    changed_row: rgb(0xf3f1fe),
    selected_row: rgb(0xeeebfd),
    scope_border: rgb(0xcfc7f5),
    scope_text: rgb(0x5443a8),
    mark_bg: rgb(0xe2dcfc),
    mark_fg: rgb(0x4634b0),
    search_bg: rgb(0xffffff),
    warn_surface: rgb(0xfff4dc),
    warn_border: rgb(0xead3a3),
    warn_text: rgb(0x6b4e11),
    warn_button: rgb(0xf6e6bf),
    warn_button_border: rgb(0xdcc48c),
    warn_dim: rgb(0x8a6e2e),
    toggle_off: rgb(0xc9ced5),
    toggle_knob: rgb(0xffffff),
    toggle_knob_on: rgb(0xffffff),
    dark_box: rgb(0x1d2126),
    dark_box_text: rgb(0xf3f4f6),
    hover: rgb(0xe8ebef),
    on_accent: rgb(0xffffff),
    success: rgb(0x1f7a46),
};

pub fn palette(dark: bool) -> &'static Palette {
    if dark { &DARK } else { &LIGHT }
}
/// The palette for the theme `ui` is drawing with.
pub fn of(ui: &egui::Ui) -> &'static Palette {
    palette(ui.visuals().dark_mode)
}
/// The palette of the context's theme (for windows drawn without a parent `Ui`).
pub fn of_ctx(ctx: &egui::Context) -> &'static Palette {
    palette(ctx.theme() == egui::Theme::Dark)
}

/// Colours the wgpu passes need, resolved for one mode.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewportColors {
    pub background: [f32; 3],
    pub bed: [f32; 3],
    pub bed_inactive: [f32; 3],
    pub grid_minor: [f32; 3],
    pub grid_major: [f32; 3],
    pub bed_outline: [f32; 3],
    pub bed_outline_current: [f32; 3],
    pub volume_outline: [f32; 4],
    pub exclusion: [f32; 4],
    pub selection: [f32; 3],
    pub hover: [f32; 3],
    pub warning: [f32; 3],
    /// Printer bed model (the sheet / frame STL).
    pub bed_model: [f32; 3],
    /// Background behind a bed texture: radial gradient from `texture_back_light` at the
    /// centre to `texture_back_dark` at the edges (as PrusaSlicer's printbed shader).
    pub texture_back_dark: [f32; 3],
    pub texture_back_light: [f32; 3],
    pub sky: [f32; 3],
    pub ground: [f32; 3],
}
/// A colour as RGB floats (0–1) for the renderer.
pub fn c3_of(c: Color32) -> [f32; 3] {
    c3(c)
}
fn c3(c: Color32) -> [f32; 3] {
    [
        c.r() as f32 / 255.,
        c.g() as f32 / 255.,
        c.b() as f32 / 255.,
    ]
}
pub fn viewport_colors(dark: bool) -> ViewportColors {
    if dark {
        ViewportColors {
            background: c3(DARK.bg),
            bed: c3(rgb(0x25292e)),
            bed_inactive: c3(rgb(0x1e2226)),
            grid_minor: c3(rgb(0x2c3137)),
            grid_major: c3(rgb(0x343a41)),
            bed_outline: c3(rgb(0x3a4048)),
            bed_outline_current: c3(ACCENT),
            volume_outline: [0.5, 0.52, 0.56, 0.18],
            exclusion: [0.75, 0.3, 0.25, 0.35],
            selection: c3(ACCENT),
            hover: c3(ACCENT_BRIGHT),
            warning: c3(WARNING),
            bed_model: [0.2, 0.21, 0.225],
            texture_back_dark: [0.165, 0.18, 0.2],
            texture_back_light: [0.235, 0.25, 0.27],
            sky: [0.66, 0.7, 0.76],
            ground: [0.38, 0.37, 0.36],
        }
    } else {
        ViewportColors {
            background: c3(LIGHT.bg),
            bed: c3(rgb(0xd9dce1)),
            bed_inactive: c3(rgb(0xe2e5e9)),
            grid_minor: c3(rgb(0xcfd3d9)),
            grid_major: c3(rgb(0xc0c5cc)),
            bed_outline: c3(rgb(0xa9afb7)),
            bed_outline_current: c3(ACCENT_DIM),
            volume_outline: [0.3, 0.32, 0.36, 0.3],
            exclusion: [0.85, 0.35, 0.3, 0.3],
            selection: c3(ACCENT_DIM),
            hover: c3(ACCENT),
            warning: c3(WARNING),
            bed_model: [0.62, 0.64, 0.67],
            texture_back_dark: [0.6, 0.62, 0.65],
            texture_back_light: [0.74, 0.76, 0.79],
            sky: [0.95, 0.96, 1.0],
            ground: [0.5, 0.49, 0.47],
        }
    }
}

// ----- fonts -------------------------------------------------------------------------------

const SANS: &str = "IBM Plex Sans";
const SANS_MEDIUM: &str = "IBM Plex Sans Medium";
const SANS_SEMIBOLD: &str = "IBM Plex Sans SemiBold";
const MONO: &str = "IBM Plex Mono";
const MONO_MEDIUM: &str = "IBM Plex Mono Medium";
const FAMILY_MEDIUM: &str = "mica-medium";
const FAMILY_SEMIBOLD: &str = "mica-semibold";
const FAMILY_MONO_MEDIUM: &str = "mica-mono-medium";

/// Body text (IBM Plex Sans Regular).
pub fn sans(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}
/// Weight 500: button labels, the scope line's object name.
pub fn medium(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(Arc::from(FAMILY_MEDIUM)))
}
/// Weight 600: card headers, section titles, the wordmark.
pub fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(Arc::from(FAMILY_SEMIBOLD)))
}
/// Numbers and read-outs (IBM Plex Mono Regular).
pub fn mono(size: f32) -> FontId {
    FontId::new(size, FontFamily::Monospace)
}
/// Large read-outs (IBM Plex Mono Medium): tabular digits that stand out.
pub fn mono_medium(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(Arc::from(FAMILY_MONO_MEDIUM)))
}

/// IBM Plex (SIL OFL 1.1, see THIRD_PARTY.md) in front of egui's defaults, which stay as
/// fallbacks for symbols and emoji.
pub fn font_definitions() -> FontDefinitions {
    let mut fonts = FontDefinitions::default();
    let faces: [(&str, &'static [u8]); 5] = [
        (
            SANS,
            include_bytes!("../assets/fonts/IBMPlexSans-Regular.ttf"),
        ),
        (
            SANS_MEDIUM,
            include_bytes!("../assets/fonts/IBMPlexSans-Medium.ttf"),
        ),
        (
            SANS_SEMIBOLD,
            include_bytes!("../assets/fonts/IBMPlexSans-SemiBold.ttf"),
        ),
        (
            MONO,
            include_bytes!("../assets/fonts/IBMPlexMono-Regular.ttf"),
        ),
        (
            MONO_MEDIUM,
            include_bytes!("../assets/fonts/IBMPlexMono-Medium.ttf"),
        ),
    ];
    for (name, bytes) in faces {
        fonts
            .font_data
            .insert(name.to_owned(), Arc::new(FontData::from_static(bytes)));
    }
    let defaults_prop = fonts
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    let defaults_mono = fonts
        .families
        .get(&FontFamily::Monospace)
        .cloned()
        .unwrap_or_default();
    let with = |first: &[&str], rest: &[String]| -> Vec<String> {
        first
            .iter()
            .map(|s| s.to_string())
            .chain(rest.iter().cloned())
            .collect()
    };
    fonts
        .families
        .insert(FontFamily::Proportional, with(&[SANS], &defaults_prop));
    fonts
        .families
        .insert(FontFamily::Monospace, with(&[MONO, SANS], &defaults_mono));
    fonts.families.insert(
        FontFamily::Name(Arc::from(FAMILY_MEDIUM)),
        with(&[SANS_MEDIUM, SANS], &defaults_prop),
    );
    fonts.families.insert(
        FontFamily::Name(Arc::from(FAMILY_SEMIBOLD)),
        with(&[SANS_SEMIBOLD, SANS], &defaults_prop),
    );
    fonts.families.insert(
        FontFamily::Name(Arc::from(FAMILY_MONO_MEDIUM)),
        with(&[MONO_MEDIUM, MONO, SANS], &defaults_mono),
    );
    fonts
}

/// Installs the fonts and the palette for both egui themes and selects `mode`. Call once at
/// start-up and again whenever the mode changes.
pub fn apply(ctx: &egui::Context, mode: Mode) {
    let fonts_id = egui::Id::unique("mica-fonts-installed");
    if !ctx.data(|d| d.get_temp::<bool>(fonts_id).unwrap_or(false)) {
        ctx.set_fonts(font_definitions());
        ctx.data_mut(|d| d.insert_temp(fonts_id, true));
    }
    ctx.options_mut(|o| o.fallback_theme = egui::Theme::Dark);
    ctx.set_theme(match mode {
        Mode::Dark => egui::ThemePreference::Dark,
        Mode::Light => egui::ThemePreference::Light,
    });
    for theme in [egui::Theme::Dark, egui::Theme::Light] {
        ctx.style_mut_of(theme, |style| {
            style.visuals = visuals(theme == egui::Theme::Dark);
            style.spacing.item_spacing = Vec2::new(8., 6.);
            style.spacing.button_padding = Vec2::new(12., 6.);
            style.spacing.interact_size = Vec2::new(40., 26.);
            style.spacing.slider_width = 140.;
            style.spacing.combo_width = 120.;
            style.spacing.indent = 14.;
            style.spacing.menu_margin = egui::Margin::same(6);
            style.spacing.window_margin = egui::Margin::same(10);
            // No rubber-band overscroll: pulling past the end of a list opened a large empty
            // gap in the sidebar before it sprang back.
            style.spacing.scroll.kinetic.rubber_band = false;
            style.text_styles = [
                (TextStyle::Small, sans(11.)),
                (TextStyle::Body, sans(13.)),
                (TextStyle::Button, sans(13.)),
                (TextStyle::Heading, semibold(15.)),
                (TextStyle::Monospace, mono(12.5)),
            ]
            .into();
            style.interaction.tooltip_delay = 0.4;
        });
    }
}

fn visuals(dark: bool) -> egui::Visuals {
    let p = palette(dark);
    let mut v = if dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    let radius = CornerRadius::same(6);
    v.panel_fill = p.panel;
    v.window_fill = p.panel;
    v.faint_bg_color = p.raised;
    v.extreme_bg_color = p.field;
    v.code_bg_color = p.dark_box;
    v.window_corner_radius = CornerRadius::same(10);
    v.menu_corner_radius = CornerRadius::same(8);
    v.window_stroke = Stroke::new(1., p.border_soft);
    v.override_text_color = Some(p.text);
    v.weak_text_color = Some(p.muted);
    v.hyperlink_color = p.accent_bright;
    v.warn_fg_color = WARNING;
    v.error_fg_color = DANGER;
    v.selection.bg_fill = p.accent_surface;
    v.selection.stroke = Stroke::new(1., p.accent);
    v.widgets.noninteractive.bg_fill = p.panel;
    v.widgets.noninteractive.weak_bg_fill = p.panel;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1., p.border);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1., p.muted);
    v.widgets.noninteractive.corner_radius = radius;
    v.widgets.inactive.bg_fill = p.raised;
    v.widgets.inactive.weak_bg_fill = p.raised;
    v.widgets.inactive.bg_stroke = Stroke::new(1., p.border_raised);
    v.widgets.inactive.fg_stroke = Stroke::new(1., p.text);
    v.widgets.inactive.corner_radius = radius;
    v.widgets.hovered.bg_fill = p.hover;
    v.widgets.hovered.weak_bg_fill = p.hover;
    v.widgets.hovered.bg_stroke = Stroke::new(1., p.border_raised);
    v.widgets.hovered.fg_stroke = Stroke::new(1.5, p.text);
    v.widgets.hovered.corner_radius = radius;
    v.widgets.active.bg_fill = p.hover;
    v.widgets.active.weak_bg_fill = p.hover;
    v.widgets.active.bg_stroke = Stroke::new(1., p.accent);
    v.widgets.active.fg_stroke = Stroke::new(1.5, p.text);
    v.widgets.active.corner_radius = radius;
    v.widgets.open.bg_fill = p.raised;
    v.widgets.open.weak_bg_fill = p.raised;
    v.widgets.open.bg_stroke = Stroke::new(1., p.accent);
    v.widgets.open.fg_stroke = Stroke::new(1., p.text);
    v.widgets.open.corner_radius = radius;
    // Flat design: no drop shadows anywhere.
    v.window_shadow = egui::epaint::Shadow::NONE;
    v.popup_shadow = egui::epaint::Shadow::NONE;
    v.slider_trailing_fill = true;
    v
}

/// Frame for panels floating over the 3D view (toolbar, legend, object list, plate bar):
/// opaque panel surface, 1 px soft border, 10 px radius, no shadow.
pub fn overlay_frame(ui: &egui::Ui) -> egui::Frame {
    let p = of(ui);
    egui::Frame::new()
        .fill(p.panel)
        .stroke(Stroke::new(1., p.border_soft))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(egui::Margin::same(8))
}

pub fn to_color32(c: [f32; 3]) -> Color32 {
    Color32::from_rgb(
        (c[0] * 255.).round() as u8,
        (c[1] * 255.).round() as u8,
        (c[2] * 255.).round() as u8,
    )
}

/// Parses `#RRGGBB` / `#RRGGBBAA` / `RRGGBB` as used by `filament_colour` and `extruder_colour`.
pub fn parse_hex(text: &str) -> Option<[f32; 3]> {
    let hex = text.trim().trim_start_matches('#');
    if hex.len() != 6 && hex.len() != 8 {
        return None;
    }
    let channel = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    Some([
        channel(0)? as f32 / 255.,
        channel(2)? as f32 / 255.,
        channel(4)? as f32 / 255.,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn theme_choices_round_trip_and_old_system_stays_dark() {
        for c in Choice::ALL {
            assert_eq!(Choice::from_name(c.name()), c);
        }
        assert_eq!(Choice::from_name("system"), Choice::Dark);
        assert_eq!(Choice::from_name(""), Choice::Dark);
        assert_eq!(
            Choice::System.resolve(Some(egui::Theme::Light)),
            Mode::Light
        );
        assert_eq!(Choice::System.resolve(None), Mode::Dark);
        assert_eq!(Choice::Light.resolve(Some(egui::Theme::Dark)), Mode::Light);
        // `Mode` alone never reads the new value as light.
        assert_eq!(Mode::from_name("follow-system"), Mode::Dark);
    }
    #[test]
    fn hex_colours() {
        assert_eq!(parse_hex("#FF8000"), Some([1., 128. / 255., 0.]));
        assert_eq!(parse_hex("29B2B2"), parse_hex("#29b2b2"));
        assert_eq!(parse_hex("#29B2B2FF"), parse_hex("#29B2B2"));
        assert_eq!(parse_hex(""), None);
        assert_eq!(parse_hex("#GGGGGG"), None);
    }
    #[test]
    fn dark_is_the_default_and_system_resolves_to_dark() {
        assert_eq!(Mode::default(), Mode::Dark);
        assert_eq!(Mode::from_name("system"), Mode::Dark);
        assert_eq!(Mode::from_name(""), Mode::Dark);
        assert_eq!(Mode::from_name("dark"), Mode::Dark);
        assert_eq!(Mode::from_name("light"), Mode::Light);
        assert_eq!(Mode::from_name("Light "), Mode::Light);
        assert_eq!(Mode::from_name(Mode::Light.name()), Mode::Light);
    }
    #[test]
    fn palettes_match_the_mockups() {
        assert_eq!(DARK.bg, Color32::from_rgb(0x12, 0x14, 0x17));
        assert_eq!(DARK.panel, Color32::from_rgb(0x1a, 0x1d, 0x21));
        assert_eq!(DARK.field, Color32::from_rgb(0x26, 0x2a, 0x30));
        assert_eq!(DARK.accent, Color32::from_rgb(0x8b, 0x7c, 0xf6));
        assert_ne!(viewport_colors(true), viewport_colors(false));
        assert!(viewport_colors(true).background[0] < 0.2);
        assert!(viewport_colors(false).background[0] > 0.8);
    }
    #[test]
    fn plex_fonts_lead_every_family() {
        let f = font_definitions();
        for name in [SANS, SANS_MEDIUM, SANS_SEMIBOLD, MONO, MONO_MEDIUM] {
            assert!(f.font_data.contains_key(name), "{name}");
        }
        assert_eq!(f.families[&FontFamily::Proportional][0], SANS);
        assert_eq!(f.families[&FontFamily::Monospace][0], MONO);
        assert_eq!(
            f.families[&FontFamily::Name(Arc::from(FAMILY_SEMIBOLD))][0],
            SANS_SEMIBOLD
        );
        // egui's defaults remain as fallbacks for symbols.
        assert!(f.families[&FontFamily::Proportional].len() > 2);
    }
}
