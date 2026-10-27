// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Vector icons for the floating toolbars (the mockup's 16 px line icons), drawn with egui
//! shapes so they follow the theme and need no image assets.

use crate::keymap::{self, Act, Tool};
use crate::theme;
use eframe::egui::{self, Color32, Pos2, Rect, Response, Sense, Shape, Stroke, Vec2};
use mica_core::painting::PaintKind;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Icon {
    Add,
    Arrange,
    Orient,
    Move,
    Rotate,
    Scale,
    LayOnFace,
    Cut,
    /// A ruler (measure tool) and a box cut by a plane (section view).
    Measure,
    Section,
    /// Mesh boolean: two overlapping squares.
    Boolean,
    /// Simplify: a dense triangle fan thinning out.
    Simplify,
    /// Repair mesh: a sticking plaster.
    Repair,
    /// The mesh tools' group (boolean, simplify, repair): a triangulated square.
    Mesh,
    /// The Text / SVG emboss tool: a serif "T".
    Text,
    /// Painting tools: a brush with a support pillar, a seam line, a drop (colour), a wavy
    /// edge (fuzzy skin); `Paint` is the bare brush of their group.
    PaintSupport,
    PaintSeam,
    PaintColour,
    PaintFuzzy,
    PaintTexture,
    Paint,
    /// Variable layer height: stacked layers of different thickness.
    LayerHeights,
    /// Brim ears: a corner with a disc on it.
    BrimEars,
    /// The toolbar's overflow menu: three dots.
    More,
    Lock,
    Unlock,
    Settings,
    Left,
    Right,
    AddPlate,
    /// Rename: a pencil.
    Rename,
}
impl Icon {
    /// The tool's name (menus) and what it does (tooltips; may be empty).
    pub fn text(self) -> (&'static str, &'static str) {
        match self {
            Self::Add => ("Add model", "put a model file on the plate"),
            Self::Arrange => ("Arrange", "spread the objects out on their plates"),
            Self::Orient => ("Auto-orient", ""),
            Self::Move => ("Move", ""),
            Self::Rotate => ("Rotate", ""),
            Self::Scale => ("Scale", ""),
            Self::LayOnFace => ("Lay on face", "click the face that should rest on the bed"),
            Self::Cut => ("Cut", "split the object with a plane"),
            Self::Boolean => (
                "Mesh boolean",
                "join, subtract or intersect with another shape",
            ),
            Self::Simplify => ("Simplify", "fewer triangles, same shape"),
            Self::Repair => ("Repair mesh", "close holes and fix flipped faces"),
            Self::Mesh => ("Mesh tools", "boolean, simplify, repair"),
            Self::Measure => (
                "Measure",
                "distances, angles and diameters between points, edges, circles and faces",
            ),
            Self::Section => ("Section view", "cut the view with a plane to look inside"),
            Self::Text => (
                "Text and SVG",
                "raise, engrave or add text or an SVG on the model",
            ),
            Self::PaintSupport => (
                "Support painting",
                "enforce or block supports on the surface",
            ),
            Self::PaintSeam => ("Seam painting", "pull the seam to a stripe or keep it away"),
            Self::PaintColour => ("Color painting", "paint faces with another tool's filament"),
            Self::PaintFuzzy => ("Fuzzy skin painting", "rough up only the painted faces"),
            Self::PaintTexture => (
                "Texture painting",
                "give the painted faces a surface texture",
            ),
            Self::Paint => ("Painting", "supports, seam, color, fuzzy skin"),
            Self::LayerHeights => (
                "Layer heights",
                "thinner layers where detail matters, thicker where it does not",
            ),
            Self::BrimEars => ("Brim ears", "small brim disks only where you place them"),
            Self::More => ("More tools", ""),
            Self::Lock => ("Plate locked", ""),
            Self::Unlock => ("Lock plate", ""),
            Self::Settings => ("Plate settings", ""),
            Self::Left => ("Previous plate", ""),
            Self::Right => ("Next plate", ""),
            Self::AddPlate => ("Add plate", ""),
            Self::Rename => ("Rename plate", ""),
        }
    }
    /// The keyboard shortcut that does what the button does (see `keymap::BINDINGS`).
    pub fn act(self) -> Option<Act> {
        Some(match self {
            Self::Add => Act::Import,
            Self::Arrange => Act::ArrangeAll,
            Self::Orient => Act::Orient,
            Self::Move => Act::Tool(Tool::Move),
            Self::Rotate => Act::Tool(Tool::Rotate),
            Self::Scale => Act::Tool(Tool::Scale),
            Self::LayOnFace => Act::Tool(Tool::LayOnFace),
            Self::Cut => Act::Tool(Tool::Cut),
            Self::Boolean => Act::Tool(Tool::MeshBoolean),
            Self::Text => Act::Tool(Tool::Emboss),
            Self::Measure => Act::Tool(Tool::Measure),
            Self::PaintSupport => Act::Tool(Tool::Paint(PaintKind::Supports)),
            Self::PaintSeam => Act::Tool(Tool::Paint(PaintKind::Seam)),
            Self::PaintColour => Act::Tool(Tool::Paint(PaintKind::Material)),
            Self::PaintFuzzy => Act::Tool(Tool::Paint(PaintKind::Fuzzy)),
            Self::PaintTexture => Act::Tool(Tool::Paint(PaintKind::Texture)),
            Self::LayerHeights => Act::Tool(Tool::LayerHeights),
            Self::BrimEars => Act::Tool(Tool::BrimEars),
            _ => return None,
        })
    }
    /// The shortcut as menus print it (`M`, `Cmd+I`), empty when there is none.
    pub fn shortcut(self) -> String {
        self.act().map(keymap::shortcut_text).unwrap_or_default()
    }
    /// Tooltip: `Name (key): what it does`.
    pub fn tooltip(self) -> String {
        let (name, detail) = self.text();
        let key = self.shortcut();
        let mut s = name.to_string();
        if !key.is_empty() {
            s = format!("{s} ({key})");
        }
        if !detail.is_empty() {
            s = format!("{s}: {detail}");
        }
        s
    }
}

/// Width of a fly-out row.
pub const MENU_W: f32 = 236.;

pub use crate::state::ToolbarSize;

/// The measurements of the floating toolbars (tool bar, plate bar, preview bar) for one
/// [`ToolbarSize`]. Glyphs are vector shapes scaled to `glyph`, so every size stays sharp.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Metrics {
    /// Side of a button.
    pub button: f32,
    /// Side of the box a glyph is drawn in (its 24-unit grid); the strokes fill about 3/4.
    pub glyph: f32,
    /// Space between buttons.
    pub gap: f32,
    /// Width of a group separator (the line sits in its middle).
    pub sep: f32,
    /// Padding between the bar's border and its buttons.
    pub margin: f32,
    /// Main and thin stroke widths.
    pub stroke: f32,
    pub thin: f32,
    /// Corner radius of a button's fill.
    pub radius: f32,
    /// Leg of the fly-out corner triangle.
    pub mark: f32,
    /// Height of a fly-out menu row and its glyph.
    pub row: f32,
    pub row_glyph: f32,
}

impl Metrics {
    pub const fn of(size: ToolbarSize) -> Self {
        match size {
            // About the size the toolbar had before there was a choice.
            ToolbarSize::Compact => Self {
                button: 28.,
                glyph: 16.,
                gap: 2.,
                sep: 8.,
                margin: 4.,
                stroke: 1.8,
                thin: 1.4,
                radius: 6.,
                mark: 5.,
                row: 30.,
                row_glyph: 16.,
            },
            ToolbarSize::Normal => Self {
                button: 36.,
                glyph: 25.,
                gap: 3.,
                sep: 11.,
                margin: 5.,
                stroke: 2.1,
                thin: 1.6,
                radius: 8.,
                mark: 7.,
                row: 32.,
                row_glyph: 18.,
            },
            ToolbarSize::Large => Self {
                button: 44.,
                glyph: 32.,
                gap: 4.,
                sep: 15.,
                margin: 6.,
                stroke: 2.5,
                thin: 1.9,
                radius: 9.,
                mark: 8.,
                row: 36.,
                row_glyph: 20.,
            },
        }
    }

    /// Outer height of a bar of buttons in an [`theme::overlay_frame`] (1 px border).
    pub fn bar_height(&self) -> f32 {
        self.button + 2. * self.margin + 2.
    }
}

/// The toolbar size the 3D view draws with this frame (set by [`set_size`]).
pub fn metrics(ctx: &egui::Context) -> Metrics {
    Metrics::of(ctx.data(|d| d.get_temp::<ToolbarSize>(size_id()).unwrap_or_default()))
}

/// Sets the toolbar size for the frame (from Settings › General).
pub fn set_size(ctx: &egui::Context, size: ToolbarSize) {
    ctx.data_mut(|d| d.insert_temp(size_id(), size));
}

fn size_id() -> egui::Id {
    egui::Id::unique("mica toolbar size")
}

/// The colors of a toolbar button in each state, for the theme in use.
struct Look {
    fill: Option<Color32>,
    border: Option<Color32>,
    glyph: Color32,
}

fn look(p: &theme::Palette, dark: bool, active: bool, hovered: bool, enabled: bool) -> Look {
    if !enabled {
        // Clearly faded, but the shape still reads.
        let glyph = if dark {
            Color32::from_rgb(0x55, 0x5b, 0x63)
        } else {
            Color32::from_rgb(0xb4, 0xb9, 0xc0)
        };
        return Look {
            fill: None,
            border: None,
            glyph,
        };
    }
    if active {
        return Look {
            fill: Some(p.accent_surface),
            border: Some(p.accent.gamma_multiply(if dark { 0.75 } else { 0.6 })),
            glyph: p.accent_hi,
        };
    }
    if hovered {
        return Look {
            fill: Some(if dark {
                Color32::from_rgb(0x2e, 0x33, 0x3a)
            } else {
                Color32::from_rgb(0xe6, 0xe9, 0xee)
            }),
            border: Some(p.border_raised),
            glyph: p.text,
        };
    }
    Look {
        fill: None,
        border: None,
        // Bright on the dark panel, near black on the light one.
        glyph: if dark {
            Color32::from_rgb(0xd6, 0xd9, 0xde)
        } else {
            Color32::from_rgb(0x2e, 0x33, 0x3a)
        },
    }
}

/// Toolbar icon button (size from [`metrics`]): a bright glyph, a filled and outlined
/// surface on hover, the accent surface when `active`, a faded glyph when disabled.
pub fn button(ui: &mut egui::Ui, icon: Icon, active: bool, enabled: bool) -> bool {
    button_tip(ui, icon, &icon.tooltip(), active, enabled)
}

/// [`button`] with its own tooltip.
pub fn button_tip(ui: &mut egui::Ui, icon: Icon, tip: &str, active: bool, enabled: bool) -> bool {
    let response = paint_button(ui, icon, active, enabled, false).on_hover_text(tip);
    enabled && response.clicked()
}

/// [`button_tip`] that hands back its response (to anchor a menu to it); the tooltip hides
/// while that menu is open.
pub fn button_response(ui: &mut egui::Ui, icon: Icon, tip: &str, enabled: bool) -> Response {
    let response = paint_button(ui, icon, false, enabled, false);
    if egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&response)) {
        response
    } else {
        response.on_hover_text(tip)
    }
}

/// A toolbar button that opens a fly-out (a corner mark says so); the caller shows
/// the menu from the response. The tooltip hides while the fly-out is open.
pub fn group_button(
    ui: &mut egui::Ui,
    icon: Icon,
    tip: &str,
    active: bool,
    enabled: bool,
) -> Response {
    let response = paint_button(ui, icon, active, enabled, true);
    if egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&response)) {
        response
    } else {
        response.on_hover_text(tip)
    }
}

fn paint_button(
    ui: &mut egui::Ui,
    icon: Icon,
    active: bool,
    enabled: bool,
    flyout: bool,
) -> Response {
    let p = theme::of(ui);
    let dark = ui.visuals().dark_mode;
    let m = metrics(ui.ctx());
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(m.button), sense);
    let painter = ui.painter();
    let open =
        flyout && egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&response));
    let hovered = enabled && (response.hovered() || open);
    let l = look(p, dark, active, hovered, enabled);
    if let Some(fill) = l.fill {
        painter.rect_filled(rect, m.radius, fill);
    }
    if let Some(border) = l.border {
        painter.rect_stroke(
            rect,
            m.radius,
            Stroke::new(1., border),
            egui::StrokeKind::Inside,
        );
    }
    glyph_sized(
        painter,
        Rect::from_center_size(rect.center(), Vec2::splat(m.glyph)),
        icon,
        l.glyph,
        m.stroke,
        m.thin,
    );
    if flyout {
        // A filled triangle tucked into the bottom-right corner: this button opens a menu.
        let inset = (m.radius * 0.45).max(2.5);
        let c = rect.right_bottom() + Vec2::new(-inset, -inset);
        painter.add(Shape::convex_polygon(
            vec![c, c + Vec2::new(0., -m.mark), c + Vec2::new(-m.mark, 0.)],
            l.glyph,
            Stroke::NONE,
        ));
    }
    if enabled && response.hovered() {
        ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
    }
    response
}

/// One row of a toolbar fly-out: icon, name and shortcut; the accent surface when `active`.
/// `why_off`: why the tool is off right now, the row's tooltip instead of what it does.
pub fn menu_row(
    ui: &mut egui::Ui,
    icon: Icon,
    active: bool,
    enabled: bool,
    why_off: Option<&str>,
) -> bool {
    let p = theme::of(ui);
    let m = metrics(ui.ctx());
    let width = MENU_W;
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, m.row), sense);
    let painter = ui.painter();
    let hovered = enabled && response.hovered();
    if active {
        painter.rect_filled(rect, 6., p.accent_surface);
    } else if hovered {
        painter.rect_filled(rect, 6., p.hover);
    }
    let color = if !enabled {
        p.muted.gamma_multiply(0.45)
    } else if active {
        p.accent_hi
    } else {
        p.text
    };
    glyph(
        painter,
        Rect::from_center_size(
            Pos2::new(rect.left() + 6. + m.row_glyph / 2., rect.center().y),
            Vec2::splat(m.row_glyph),
        ),
        icon,
        color,
    );
    let (name, detail) = icon.text();
    painter.text(
        Pos2::new(rect.left() + 16. + m.row_glyph, rect.center().y),
        egui::Align2::LEFT_CENTER,
        name,
        theme::sans(13.),
        color,
    );
    let key = icon.shortcut();
    if !key.is_empty() {
        painter.text(
            Pos2::new(rect.right() - 10., rect.center().y),
            egui::Align2::RIGHT_CENTER,
            key,
            theme::mono(12.),
            if enabled {
                p.muted
            } else {
                p.muted.gamma_multiply(0.45)
            },
        );
    }
    if hovered {
        ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
    }
    let response = match why_off {
        Some(why) => response.on_hover_text(why),
        None if detail.is_empty() => response,
        None => response.on_hover_text(detail),
    };
    enabled && response.clicked()
}

fn glyph(p: &egui::Painter, r: Rect, icon: Icon, c: Color32) {
    glyph_sized(p, r, icon, c, 1.8, 1.4);
}

/// Draws `icon` in `r` with the given stroke widths (they grow with the toolbar size).
fn glyph_sized(p: &egui::Painter, r: Rect, icon: Icon, c: Color32, stroke: f32, thin: f32) {
    p.extend(shapes(r, icon, c, stroke, thin));
}

/// `icon` in `r` as egui shapes (for drawing elsewhere than a painter, such as the plate
/// strips' textures on the ground).
pub fn shapes(r: Rect, icon: Icon, c: Color32, stroke: f32, thin: f32) -> Vec<Shape> {
    let shapes = Shapes::default();
    glyph_shapes(&shapes, r, icon, c, stroke, thin);
    shapes.0.into_inner()
}

/// Collects the shapes of a glyph, with the painter calls the glyphs use.
#[derive(Default)]
struct Shapes(std::cell::RefCell<Vec<Shape>>);
impl Shapes {
    fn add(&self, s: impl Into<Shape>) {
        self.0.borrow_mut().push(s.into());
    }
    fn line_segment(&self, points: [Pos2; 2], stroke: Stroke) {
        self.add(Shape::line_segment(points, stroke));
    }
    fn circle_filled(&self, center: Pos2, radius: f32, color: Color32) {
        self.add(Shape::circle_filled(center, radius, color));
    }
    fn circle_stroke(&self, center: Pos2, radius: f32, stroke: Stroke) {
        self.add(Shape::circle_stroke(center, radius, stroke));
    }
    fn rect_filled(&self, rect: Rect, radius: f32, color: Color32) {
        self.add(Shape::rect_filled(rect, radius, color));
    }
    fn rect_stroke(&self, rect: Rect, radius: f32, stroke: Stroke, kind: egui::StrokeKind) {
        self.add(Shape::rect_stroke(rect, radius, stroke, kind));
    }
}

fn glyph_shapes(p: &Shapes, r: Rect, icon: Icon, c: Color32, stroke: f32, thin: f32) {
    let s = Stroke::new(stroke, c);
    let thin = Stroke::new(thin, c);
    let cx = r.center();
    let w = r.width();
    // Mockup icons are drawn on a 24-unit grid.
    let at = |x: f32, y: f32| Pos2::new(r.left() + x / 24. * w, r.top() + y / 24. * r.height());
    match icon {
        Icon::Add => {
            p.line_segment([at(12., 5.), at(12., 19.)], s);
            p.line_segment([at(5., 12.), at(19., 12.)], s);
        }
        Icon::Arrange => {
            for (x, y) in [(4., 4.), (14., 4.), (4., 14.), (14., 14.)] {
                p.rect_stroke(
                    Rect::from_min_max(at(x, y), at(x + 6., y + 6.)),
                    0.5,
                    s,
                    egui::StrokeKind::Inside,
                );
            }
        }
        Icon::Orient => {
            // Star (auto-orient).
            p.add(Shape::closed_line(
                vec![
                    at(12., 3.),
                    at(14.5, 8.5),
                    at(20., 9.),
                    at(16., 13.),
                    at(17., 19.),
                    at(12., 16.),
                    at(7., 19.),
                    at(8., 13.),
                    at(4., 9.),
                    at(9.5, 8.5),
                ],
                s,
            ));
        }
        Icon::Move => {
            p.line_segment([at(12., 3.), at(12., 21.)], s);
            p.line_segment([at(3., 12.), at(21., 12.)], s);
            for (a, b, c2) in [
                (at(12., 3.), at(9., 6.), at(15., 6.)),
                (at(12., 21.), at(9., 18.), at(15., 18.)),
                (at(3., 12.), at(6., 9.), at(6., 15.)),
                (at(21., 12.), at(18., 9.), at(18., 15.)),
            ] {
                p.line_segment([a, b], s);
                p.line_segment([a, c2], s);
            }
        }
        Icon::Rotate => {
            // M20 12a8 8 0 1 1-3-6.2M20 4v5h-5
            let rad = w * 8. / 24.;
            let pts: Vec<Pos2> = (0..=24)
                .map(|i| {
                    let a = -i as f32 / 24. * (std::f32::consts::TAU - 0.7);
                    cx + Vec2::new(a.cos(), a.sin()) * rad
                })
                .collect();
            p.add(Shape::line(pts, s));
            p.add(Shape::line(vec![at(20., 4.), at(20., 9.), at(15., 9.)], s));
        }
        Icon::Scale => {
            p.rect_stroke(
                Rect::from_min_max(at(4., 10.), at(14., 20.)),
                0.5,
                s,
                egui::StrokeKind::Inside,
            );
            p.add(Shape::line(vec![at(14., 4.), at(20., 4.), at(20., 10.)], s));
            p.line_segment([at(20., 4.), at(12., 12.)], s);
        }
        Icon::LayOnFace => {
            p.line_segment([at(3., 20.), at(21., 20.)], s);
            p.add(Shape::closed_line(
                vec![at(7., 16.), at(12., 6.), at(17., 16.)],
                s,
            ));
        }
        Icon::PaintSupport
        | Icon::PaintSeam
        | Icon::PaintColour
        | Icon::PaintFuzzy
        | Icon::PaintTexture
        | Icon::Paint => {
            // A brush handle from the top right down to a tip at the bottom left…
            p.line_segment([at(20., 4.), at(11., 13.)], s);
            p.add(Shape::closed_line(
                vec![at(11., 13.), at(6., 14.), at(4., 20.), at(10., 18.)],
                thin,
            ));
            // …and the painted mark that says which tool it is.
            match icon {
                Icon::PaintSupport => {
                    p.line_segment([at(15., 20.), at(21., 20.)], s);
                    p.line_segment([at(18., 14.), at(18., 20.)], s);
                }
                Icon::PaintSeam => {
                    p.add(Shape::line(
                        vec![at(14., 20.), at(17., 15.), at(21., 18.)],
                        s,
                    ));
                }
                Icon::PaintColour => {
                    p.circle_filled(at(18., 17.5), w * 2.6 / 24., c);
                }
                Icon::PaintTexture => {
                    // A small knurled patch.
                    p.add(Shape::closed_line(
                        vec![at(14., 15.), at(21., 15.), at(21., 21.), at(14., 21.)],
                        thin,
                    ));
                    p.line_segment([at(14., 15.), at(21., 21.)], thin);
                    p.line_segment([at(21., 15.), at(14., 21.)], thin);
                }
                Icon::Paint => {
                    // The bare brush of the group: its tip filled in.
                    p.add(Shape::convex_polygon(
                        vec![at(11., 13.), at(10., 18.), at(4., 20.), at(6., 14.)],
                        c.gamma_multiply(0.6),
                        Stroke::NONE,
                    ));
                }
                _ => {
                    p.add(Shape::line(
                        vec![
                            at(13., 20.),
                            at(15., 17.),
                            at(17., 20.),
                            at(19., 17.),
                            at(21., 20.),
                        ],
                        thin,
                    ));
                }
            }
        }
        Icon::LayerHeights => {
            // Layers from thick at the bottom to thin at the top, beside a profile curve.
            for (y, h) in [(17., 3.), (12.5, 2.), (9.5, 1.2), (7.5, 0.8)] {
                p.rect_filled(Rect::from_min_max(at(3., y), at(13., y + h)), 0.5, c);
            }
            p.add(Shape::line(
                vec![at(20., 20.), at(19., 15.), at(16.5, 10.), at(16., 5.)],
                s,
            ));
        }
        Icon::BrimEars => {
            // A square's corner with a disc on it.
            p.add(Shape::line(vec![at(4., 9.), at(15., 9.), at(15., 20.)], s));
            p.circle_stroke(at(15., 9.), w * 5. / 24., thin);
            p.circle_filled(at(15., 9.), w * 1.6 / 24., c);
        }
        Icon::Cut => {
            p.circle_stroke(at(6., 18.), w * 3. / 24., s);
            p.circle_stroke(at(6., 6.), w * 3. / 24., s);
            p.line_segment([at(8., 8.), at(20., 18.)], s);
            p.line_segment([at(8., 16.), at(20., 6.)], s);
        }
        Icon::Measure => {
            // A ruler lying diagonally with five ticks.
            let a = at(3., 15.);
            let b = at(17., 3.);
            let d = (b - a).normalized();
            let n = Vec2::new(d.y, -d.x) * (w * 5. / 24.);
            p.add(Shape::closed_line(vec![a, b, b - n, a - n], thin));
            for k in 1..6 {
                let t = a + (b - a) * (k as f32 / 6.);
                let len = if k % 2 == 0 { 0.55 } else { 0.32 };
                p.line_segment([t, t - n * len], thin);
            }
        }
        Icon::Section => {
            // A box with its front half cut away by a slanted plane.
            p.rect_stroke(
                Rect::from_min_max(at(5., 5.), at(19., 19.)),
                1.,
                thin,
                egui::StrokeKind::Inside,
            );
            p.add(Shape::convex_polygon(
                vec![at(5., 19.), at(5., 12.), at(19., 6.), at(19., 19.)],
                c.gamma_multiply(0.35),
                Stroke::NONE,
            ));
            p.line_segment([at(3., 13.), at(21., 5.)], s);
        }
        Icon::Boolean => {
            p.rect_stroke(
                Rect::from_min_max(at(4., 4.), at(15., 15.)),
                1.5,
                s,
                egui::StrokeKind::Middle,
            );
            p.rect_filled(
                Rect::from_min_max(at(9., 9.), at(20., 20.)),
                1.5,
                c.gamma_multiply(0.35),
            );
            p.rect_stroke(
                Rect::from_min_max(at(9., 9.), at(20., 20.)),
                1.5,
                s,
                egui::StrokeKind::Middle,
            );
        }
        Icon::Simplify => {
            p.add(Shape::closed_line(
                vec![at(12., 4.), at(20., 19.), at(4., 19.)],
                s,
            ));
            p.line_segment([at(12., 4.), at(9., 19.)], thin);
            p.line_segment([at(12., 4.), at(15., 19.)], thin);
            p.line_segment([at(6.5, 14.5), at(12., 19.)], thin);
        }
        Icon::Repair => {
            // A sticking plaster across the icon, its pad in the middle.
            let o = at(12., 12.);
            let d = Vec2::new(1., -1.).normalized() * (w * 9.5 / 24.);
            let n = Vec2::new(1., 1.).normalized() * (w * 4. / 24.);
            p.add(Shape::closed_line(
                vec![o - d - n, o + d - n, o + d + n, o - d + n],
                s,
            ));
            let pad = d * 0.32;
            p.add(Shape::closed_line(
                vec![o - pad - n, o + pad - n, o + pad + n, o - pad + n],
                thin,
            ));
            for k in [-0.62f32, 0.62] {
                p.circle_filled(o + d * k, w * 0.9 / 24., c);
            }
        }
        Icon::Mesh => {
            // A triangulated square.
            p.rect_stroke(
                Rect::from_min_max(at(4., 4.), at(20., 20.)),
                1.,
                s,
                egui::StrokeKind::Middle,
            );
            p.line_segment([at(4., 12.), at(20., 12.)], thin);
            p.line_segment([at(12., 4.), at(12., 20.)], thin);
            p.line_segment([at(4., 20.), at(20., 4.)], thin);
            p.line_segment([at(4., 12.), at(12., 4.)], thin);
            p.line_segment([at(12., 20.), at(20., 12.)], thin);
        }
        Icon::More => {
            for x in [6., 12., 18.] {
                p.circle_filled(at(x, 12.), w * 1.7 / 24., c);
            }
        }
        Icon::Text => {
            p.line_segment([at(5., 6.), at(19., 6.)], s);
            p.line_segment([at(12., 6.), at(12., 19.)], s);
            p.line_segment([at(5., 6.), at(5., 8.5)], thin);
            p.line_segment([at(19., 6.), at(19., 8.5)], thin);
            p.line_segment([at(9.5, 19.), at(14.5, 19.)], thin);
        }
        Icon::Lock | Icon::Unlock => {
            p.rect_filled(Rect::from_min_max(at(5., 11.), at(19., 21.)), 1.5, c);
            let pts: Vec<Pos2> = (0..=12)
                .map(|i| {
                    let a = std::f32::consts::PI + i as f32 / 12. * std::f32::consts::PI;
                    at(12., 10.) + Vec2::new(a.cos(), a.sin()) * w * 5. / 24.
                })
                .collect();
            if icon == Icon::Lock {
                p.add(Shape::line(pts, s));
            } else {
                p.add(Shape::line(pts[..9].to_vec(), s));
            }
        }
        Icon::Settings => {
            p.circle_stroke(cx, w * 3.5 / 24., s);
            for i in 0..8 {
                let a = i as f32 / 8. * std::f32::consts::TAU;
                let d = Vec2::new(a.cos(), a.sin());
                p.line_segment([cx + d * w * 7. / 24., cx + d * w * 10.5 / 24.], s);
            }
        }
        Icon::Left => {
            p.add(Shape::line(vec![at(15., 5.), at(8., 12.), at(15., 19.)], s));
        }
        Icon::Right => {
            p.add(Shape::line(vec![at(9., 5.), at(16., 12.), at(9., 19.)], s));
        }
        Icon::AddPlate => {
            p.rect_stroke(
                Rect::from_min_max(at(3., 10.), at(16., 20.)),
                0.5,
                thin,
                egui::StrokeKind::Inside,
            );
            p.line_segment([at(18., 3.), at(18., 10.)], s);
            p.line_segment([at(14.5, 6.5), at(21.5, 6.5)], s);
        }
        Icon::Rename => {
            // The pencil of the panels' rename buttons (`widgets::Glyph::Pencil`).
            p.add(Shape::closed_line(
                vec![
                    at(4., 20.),
                    at(8., 20.),
                    at(19., 9.),
                    at(15., 5.),
                    at(4., 16.),
                ],
                s,
            ));
            p.line_segment([at(13., 7.), at(17., 11.)], thin);
        }
    }
}
