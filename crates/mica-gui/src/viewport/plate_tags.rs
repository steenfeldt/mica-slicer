// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: orca@8500fcdcca src/slic3r/GUI/PartPlate.cpp :: PartPlate::calc_vertex_for_number, PartPlate::generate_plate_name_texture, PartPlate::calc_vertex_for_plate_name_edit_icon
// Changes: Only Orca's ideas are kept: plate labels (number, name texture) are textured quads lying flat on the ground in plate space, sized by factor = bed depth / 200 mm, and an edit pencil follows the name. The layout is Mica's own: one strip along each plate's front edge in the gap in front of it (number badge with alert mark and "locked" on the left, the plate tools as icons on the right) and the name on its own line in front of the strip with a rename pencil; all of it measured from the front rim of the plate's drawn bed sheet, with the plate rows spaced for the sheets and labels (`row_gap`) so the labels sit clear of both sheets and clearly nearer their own. Text and icons are rasterised from egui's font atlas and icon shapes into coverage masks coloured in the shader; parts too small to read are left out (icons first, then the name line, then the strip); hit testing uses the projected quads and lets objects in front win.

//! Plate strips: each plate's number, tools and name lying flat on the ground in the gap in
//! front of the plate, so they foreshorten with the plates and never cover objects. A strip
//! along the front edge holds the number badge (alert mark, "locked") and the plate tools as
//! icons; in front of it the plate's name has a line of its own, with a rename pencil. This
//! module lays them out (ground rectangles, texture masks, colours, which are readable) and
//! hit-tests them; `render::tags` draws them.
use super::icons::{self, Icon};
use crate::render::{TagDraw, TagImage};
use crate::scene::{bed::Bed, bed_assets::Overhang, plates::Plate};
use crate::theme;
use eframe::egui::{self, Color32, Pos2, Vec2 as EVec2};
use glam::{Vec2, Vec3};
use std::{
    collections::HashMap,
    hash::{Hash, Hasher},
    sync::Arc,
};

// Sizes in units of `factor` (bed depth / 200 mm, as Orca sizes its plate labels), measured
// from the front rim of the plate's drawn bed sheet (`Bed::overhang`), which reaches past the
// printable area. The rows of plates are spaced for them (`row_gap`): the sheet in front, a
// clear distance, the strip, the name line, then more ground before the next row's sheet
// than between the strip and its own sheet.
/// From the sheet's front rim to the strip.
const SHEET_CLEAR: f32 = 4.;
/// The strip's depth.
const STRIP_H: f32 = 16.;
/// Between the strip and the name line.
const LINE_GAP: f32 = 1.5;
/// The name line's depth (and its pencil's size).
const NAME_H: f32 = 9.;
/// How far in front of the sheet's rim the name line ends.
const DEPTH: f32 = SHEET_CLEAR + STRIP_H + LINE_GAP + NAME_H;
/// From the name line to the next row's sheet: three times the strip's distance from its own
/// sheet, so the labels clearly belong to the plate behind them.
const NEXT_CLEAR: f32 = 3. * SHEET_CLEAR;
/// Inside the strip: the margin around the badge and icons.
const PAD: f32 = 2.5;
/// The number badge's height and a tool icon's size.
const BADGE_H: f32 = STRIP_H - 2. * PAD;
const ICON: f32 = BADGE_H;
/// Between two icons.
const ICON_GAP: f32 = 0.8;

/// Texture rows of a quad; the text's em is a fraction of it.
const TEXELS: f32 = 64.;
/// The smallest projected sizes (screen points) at which each part still reads; smaller
/// parts are left out rather than drawn as mush: the icons first, then the name line (the
/// strip then gives way to a bigger number alone), then the number.
const ICON_MIN_PX: f32 = 13.;
const NAME_MIN_PX: f32 = 10.;
const NUMBER_MIN_PX: f32 = 9.;
/// Zoomed out (number only), the number grows towards this projected height.
const NUMBER_TARGET_PX: f32 = 16.;

/// A plate tool on the strip (the actions of the plate's former hover toolbar).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PlateTool {
    Arrange,
    Orient,
    Lock,
    Settings,
    More,
}
impl PlateTool {
    pub const ALL: [PlateTool; 5] = [
        PlateTool::Arrange,
        PlateTool::Orient,
        PlateTool::Lock,
        PlateTool::Settings,
        PlateTool::More,
    ];
    fn icon(self, locked: bool) -> Icon {
        match self {
            PlateTool::Arrange => Icon::Arrange,
            PlateTool::Orient => Icon::Orient,
            PlateTool::Lock if locked => Icon::Lock,
            PlateTool::Lock => Icon::Unlock,
            PlateTool::Settings => Icon::Settings,
            PlateTool::More => Icon::More,
        }
    }
    /// The tooltip.
    pub fn tip(self, locked: bool) -> &'static str {
        match self {
            PlateTool::Arrange => "Arrange this plate",
            PlateTool::Orient => "Auto-orient every object on this plate",
            PlateTool::Lock if locked => "Unlock plate",
            PlateTool::Lock => "Lock plate",
            PlateTool::Settings => "Plate settings",
            PlateTool::More => "Plate menu: rename, duplicate, move, delete…",
        }
    }
    /// Whether it changes the plate's objects (then a read-only scene has it off).
    pub fn edits(self) -> bool {
        matches!(
            self,
            PlateTool::Arrange | PlateTool::Orient | PlateTool::Lock
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TagKind {
    /// The strip's background (a click on it shows the plate).
    Strip,
    Number,
    Locked,
    Tool(PlateTool),
    Name,
    Rename,
}

pub struct PlateTag {
    pub plate: usize,
    pub kind: TagKind,
    /// The quad on screen: front-left, front-right, back-right, back-left.
    pub screen: [Pos2; 4],
    pub draw: TagDraw,
    /// The pointer is on it (the topmost tag there) and no object in front of it.
    pub hovered: bool,
    /// Off: a tool that edits in a read-only scene.
    pub enabled: bool,
}
impl PlateTag {
    pub fn bounds(&self) -> egui::Rect {
        egui::Rect::from_points(&self.screen)
    }
}

/// Rasterised tag masks by content key; images not used in a frame are dropped.
#[derive(Default)]
pub struct TagCache {
    images: HashMap<u64, Arc<TagImage>>,
}

/// Orca's `factor`: labels scale with the bed's depth (200 mm = 1).
fn factor(min: Vec2, max: Vec2) -> f32 {
    ((max.y - min.y) / 200.).max(0.05)
}

/// The gap between plate rows (mm) for a bed `min`..`max` whose sheet reaches `overhang`
/// past it: the sheet in front of the plate, its strip and name line with their clearances,
/// and the sheet behind the next row's plate.
pub fn row_gap(min: Vec2, max: Vec2, overhang: Overhang) -> f32 {
    overhang.front + overhang.back + (DEPTH + NEXT_CLEAR) * factor(min, max)
}

/// How far in front of the plate's printable area its labels reach (mm), for framing.
pub fn reach(min: Vec2, max: Vec2, overhang: Overhang) -> f32 {
    overhang.front + DEPTH * factor(min, max)
}

/// The ground rectangles (`[x0, y0, x1, y1]`, world mm) of a plate's strip and name line,
/// for the plate's bed corners `min` (front-left) and `max` and its sheet's `front`
/// overhang.
fn strip_rect(min: Vec2, max: Vec2, front: f32) -> [f32; 4] {
    let f = factor(min, max);
    let y1 = min.y - front - SHEET_CLEAR * f;
    [min.x, y1 - STRIP_H * f, max.x, y1]
}
fn name_line_rect(min: Vec2, max: Vec2, front: f32) -> [f32; 4] {
    let f = factor(min, max);
    let y1 = strip_rect(min, max, front)[1] - LINE_GAP * f;
    [
        min.x + PAD * f,
        min.y - front - DEPTH * f,
        max.x - PAD * f,
        y1,
    ]
}

/// The icon squares on the right of the strip, left to right.
fn tool_rects(min: Vec2, max: Vec2, front: f32) -> Vec<[f32; 4]> {
    let f = factor(min, max);
    let [_, y0, x1, y1] = strip_rect(min, max, front);
    let (cy, n) = ((y0 + y1) / 2., PlateTool::ALL.len());
    (0..n)
        .map(|k| {
            let right = x1 - PAD * f - (n - 1 - k) as f32 * (ICON + ICON_GAP) * f;
            [
                right - ICON * f,
                cy - ICON * f / 2.,
                right,
                cy + ICON * f / 2.,
            ]
        })
        .collect()
}

/// How wide (in name-line heights) the name may be: the line less its pencil.
fn name_max_width(min: Vec2, max: Vec2) -> f32 {
    let f = factor(min, max);
    let [x0, _, x1, _] = name_line_rect(min, max, 0.);
    ((x1 - x0) / f - NAME_H * 1.1) / NAME_H
}

/// What a plate's badge and name line say: the number with its alert mark, and the name
/// (`None`: no name, the line says "Untitled").
fn texts(plate: &Plate) -> (String, Option<String>) {
    let mut number = format!("{}", plate.index + 1);
    if plate.alert > 0 {
        number = format!(
            "{}\u{2009}{number}",
            if plate.alert > 1 { "✕" } else { "!" }
        );
    }
    (number, plate.name.clone())
}

/// Linear RGBA for the renderer.
fn rgba(c: Color32, a: f32) -> [f32; 4] {
    let [r, g, b] = theme::c3_of(c);
    [r, g, b, a]
}

/// What a tag's texture holds.
#[derive(Clone, Debug)]
enum Content {
    /// A rounded chip with a line of text (empty for the strip), at least `min_width` and at
    /// most `max_width` heights wide (a longer text ends in "…").
    Chip {
        text: String,
        em: f32,
        pad: f32,
        min_width: f32,
        max_width: f32,
    },
    /// A rounded square (shown on hover) with an icon.
    Icon(Icon),
}
impl Hash for Content {
    fn hash<H: Hasher>(&self, h: &mut H) {
        match self {
            Content::Chip {
                text,
                em,
                pad,
                min_width,
                max_width,
            } => {
                (0u8, text).hash(h);
                for v in [em, pad, min_width, max_width] {
                    v.to_bits().hash(h);
                }
            }
            Content::Icon(i) => (1u8, i).hash(h),
        }
    }
}

/// An empty mask image `width` × `TEXELS` with a rounded chip (r) and its rim band (b).
fn chip(width: u32, radius: f32) -> Vec<u8> {
    let height = TEXELS as u32;
    let mut texels = vec![0u8; (width * height * 4) as usize];
    let (w, hh) = (width as f32, TEXELS);
    let radius = radius * hh;
    let rim = 0.075 * hh;
    for y in 0..height {
        for x in 0..width {
            let p = EVec2::new(x as f32 + 0.5 - w / 2., y as f32 + 0.5 - hh / 2.);
            let q =
                EVec2::new(p.x.abs(), p.y.abs()) - EVec2::new(w / 2. - radius, hh / 2. - radius);
            let d = EVec2::new(q.x.max(0.), q.y.max(0.)).length() + q.x.max(q.y).min(0.) - radius;
            let chip = (0.5 - d).clamp(0., 1.);
            let band = (d + rim + 0.5).clamp(0., 1.) * chip;
            let i = ((y * width + x) * 4) as usize;
            texels[i] = (chip * 255.).round() as u8;
            texels[i + 2] = (band * 255.).round() as u8;
            texels[i + 3] = 255;
        }
    }
    texels
}

/// Rasterises a chip with `text` into coverage masks (r chip, g text, b rim) from egui's font
/// atlas: `TEXELS` rows high, at least `min_width` and at most `max_width` chip heights wide
/// (a longer text ends in "…"). The text starts after the padding (centred on a chip wider
/// than the text).
fn rasterize(
    ctx: &egui::Context,
    text: &str,
    em: f32,
    pad: f32,
    min_width: f32,
    max_width: f32,
) -> (u32, Vec<u8>) {
    let ppp = ctx.pixels_per_point();
    let h = TEXELS;
    let pad = pad * h;
    let mut job = egui::text::LayoutJob::single_section(
        text.to_string(),
        egui::TextFormat::simple(theme::medium(em * h / ppp), Color32::WHITE),
    );
    job.wrap = egui::text::TextWrapping {
        max_width: ((max_width * h - 2. * pad) / ppp).max(1.),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    let (galley, atlas) = ctx.fonts_mut(|f| {
        let g = f.layout_job(job);
        (g, f.image())
    });
    let text_w = if text.is_empty() {
        0.
    } else {
        galley.size().x * ppp
    };
    let width = (text_w + 2. * pad).max(min_width * h).ceil().max(1.) as u32;
    let height = h as u32;
    let mut texels = chip(width, 0.3);
    let w = width as f32;
    let origin = EVec2::new(
        ((w - text_w) / 2.).max(pad),
        (h - galley.size().y * ppp) / 2.,
    );
    let [aw, ah] = atlas.size;
    let alpha = |x: i32, y: i32| -> f32 {
        if x < 0 || y < 0 || x as usize >= aw || y as usize >= ah {
            return 0.;
        }
        atlas.pixels[y as usize * aw + x as usize].a() as f32 / 255.
    };
    for row in &galley.rows {
        for glyph in &row.row.glyphs {
            let uv = glyph.uv_rect;
            if uv.is_nothing() {
                continue;
            }
            let min = (row.pos + glyph.pos.to_vec2() + uv.offset).to_vec2() * ppp + origin;
            let size = uv.size * ppp;
            let (u0, v0) = (uv.min[0] as f32, uv.min[1] as f32);
            let (du, dv) = (
                (uv.max[0] - uv.min[0]) as f32 / size.x.max(1e-3),
                (uv.max[1] - uv.min[1]) as f32 / size.y.max(1e-3),
            );
            let x0 = min.x.floor().max(0.) as i32;
            let y0 = min.y.floor().max(0.) as i32;
            let x1 = ((min.x + size.x).ceil() as i32).min(width as i32);
            let y1 = ((min.y + size.y).ceil() as i32).min(height as i32);
            for y in y0..y1 {
                for x in x0..x1 {
                    // Bilinear sample of the atlas at this texel's centre.
                    let sx = u0 + (x as f32 + 0.5 - min.x) * du - 0.5;
                    let sy = v0 + (y as f32 + 0.5 - min.y) * dv - 0.5;
                    if sx < u0 - 0.5 || sy < v0 - 0.5 {
                        continue;
                    }
                    let (fx, fy) = (sx.floor(), sy.floor());
                    let (tx, ty) = (sx - fx, sy - fy);
                    let (ix, iy) = (fx as i32, fy as i32);
                    let inside = |x: i32, y: i32| {
                        x >= uv.min[0] as i32
                            && y >= uv.min[1] as i32
                            && x < uv.max[0] as i32
                            && y < uv.max[1] as i32
                    };
                    let s = |x, y| if inside(x, y) { alpha(x, y) } else { 0. };
                    let a = (s(ix, iy) * (1. - tx) + s(ix + 1, iy) * tx) * (1. - ty)
                        + (s(ix, iy + 1) * (1. - tx) + s(ix + 1, iy + 1) * tx) * ty;
                    let i = ((y as u32 * width + x as u32) * 4 + 1) as usize;
                    texels[i] = texels[i].max((a * 255.).round() as u8);
                }
            }
        }
    }
    (width, texels)
}

/// Rasterises `icon` (the toolbar's vector glyph, tessellated by egui and filled here) into
/// a square mask: r a rounded square behind it, g the glyph.
fn rasterize_icon(icon: Icon) -> (u32, Vec<u8>) {
    use egui::epaint::{Mesh, TessellationOptions, Tessellator};
    let h = TEXELS;
    let size = h as u32;
    let mut texels = chip(size, 0.25);
    // No rim on an icon's square.
    for t in texels.as_chunks_mut::<4>().0 {
        t[2] = 0;
    }
    let glyph = 0.68 * h;
    let r = egui::Rect::from_center_size(Pos2::new(h / 2., h / 2.), EVec2::splat(glyph));
    let shapes = icons::shapes(r, icon, Color32::WHITE, 0.09 * glyph, 0.07 * glyph);
    let mut tess = Tessellator::new(
        1.,
        TessellationOptions {
            prerasterized_discs: false,
            round_line_segments_to_pixels: false,
            round_rects_to_pixels: false,
            round_text_to_pixels: false,
            ..Default::default()
        },
        [1, 1],
        vec![],
    );
    let mut mesh = Mesh::default();
    for s in shapes {
        tess.tessellate_shape(s, &mut mesh);
    }
    for tri in mesh.indices.as_chunks::<3>().0 {
        let v = [0, 1, 2].map(|k| &mesh.vertices[tri[k] as usize]);
        let (a, b, c) = (v[0].pos, v[1].pos, v[2].pos);
        let area = (b - a).x * (c - a).y - (b - a).y * (c - a).x;
        if area.abs() < 1e-6 {
            continue;
        }
        let x0 = a.x.min(b.x).min(c.x).floor().max(0.) as u32;
        let y0 = a.y.min(b.y).min(c.y).floor().max(0.) as u32;
        let x1 = (a.x.max(b.x).max(c.x).ceil() as u32).min(size);
        let y1 = (a.y.max(b.y).max(c.y).ceil() as u32).min(size);
        for y in y0..y1 {
            for x in x0..x1 {
                let p = Pos2::new(x as f32 + 0.5, y as f32 + 0.5);
                let w0 = ((b - p).x * (c - p).y - (b - p).y * (c - p).x) / area;
                let w1 = ((c - p).x * (a - p).y - (c - p).y * (a - p).x) / area;
                let w2 = 1. - w0 - w1;
                if w0 < -1e-4 || w1 < -1e-4 || w2 < -1e-4 {
                    continue;
                }
                let alpha = w0 * v[0].color.a() as f32
                    + w1 * v[1].color.a() as f32
                    + w2 * v[2].color.a() as f32;
                let i = ((y * size + x) * 4 + 1) as usize;
                texels[i] = texels[i].max(alpha.round().clamp(0., 255.) as u8);
            }
        }
    }
    (size, texels)
}

impl TagCache {
    fn image(
        &mut self,
        ctx: &egui::Context,
        used: &mut Vec<u64>,
        content: Content,
    ) -> Arc<TagImage> {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        content.hash(&mut h);
        ctx.pixels_per_point().to_bits().hash(&mut h);
        let key = h.finish();
        used.push(key);
        self.images
            .entry(key)
            .or_insert_with(|| {
                let (width, texels) = match &content {
                    Content::Chip {
                        text,
                        em,
                        pad,
                        min_width,
                        max_width,
                    } => rasterize(ctx, text, *em, *pad, *min_width, *max_width),
                    Content::Icon(icon) => rasterize_icon(*icon),
                };
                Arc::new(TagImage {
                    key,
                    width,
                    height: TEXELS as u32,
                    texels,
                })
            })
            .clone()
    }
}

/// The colours (fill, text, rim) of a tag. `active`: the plate is current or hovered (its
/// tools at full strength, the others faint).
fn colors(
    plate: &Plate,
    kind: TagKind,
    dark: bool,
    active: bool,
    tag: &PlateTag,
) -> ([f32; 4], [f32; 4], [f32; 4]) {
    let none = [0.; 4];
    let gray = |d: u8, l: u8| Color32::from_gray(if dark { d } else { l });
    let accent_text = if dark {
        theme::ACCENT_BRIGHT
    } else {
        theme::ACCENT_DIM
    };
    let surface = if dark {
        Color32::from_rgb(20, 22, 26)
    } else {
        Color32::WHITE
    };
    match kind {
        TagKind::Strip => {
            let rim = if plate.current {
                rgba(theme::ACCENT, if active { 0.75 } else { 0.55 })
            } else {
                rgba(gray(170, 80), if active { 0.35 } else { 0.14 })
            };
            (
                rgba(
                    surface,
                    match (dark, active) {
                        (true, true) => 0.92,
                        (true, false) => 0.8,
                        (false, true) => 0.96,
                        (false, false) => 0.85,
                    },
                ),
                none,
                rim,
            )
        }
        TagKind::Number => {
            let alert = match plate.alert {
                0 => None,
                1 => Some(theme::WARNING),
                _ => Some(theme::DANGER),
            };
            if plate.current {
                let fill = if dark {
                    theme::ACCENT
                } else {
                    theme::ACCENT_DIM
                };
                let rim = alert.map_or(rgba(fill, 1.), |a| rgba(a, 1.));
                (rgba(fill, 1.), rgba(Color32::WHITE, 1.), rim)
            } else {
                let text = alert.unwrap_or(gray(200, 60));
                (
                    rgba(gray(52, 228), 1.),
                    rgba(text, 1.),
                    rgba(alert.unwrap_or(gray(52, 228)), 1.),
                )
            }
        }
        TagKind::Locked => (none, rgba(gray(150, 110), 1.), none),
        TagKind::Name => {
            let text = if plate.name.is_none() {
                rgba(gray(115, 150), 1.)
            } else if plate.current {
                rgba(accent_text, 1.)
            } else {
                rgba(gray(195, 55), 1.)
            };
            (none, text, none)
        }
        TagKind::Tool(_) | TagKind::Rename => {
            let glyph = if kind == TagKind::Tool(PlateTool::Lock) && plate.locked {
                accent_text
            } else {
                gray(215, 45)
            };
            let strength = if !tag.enabled {
                0.2
            } else if tag.hovered || active {
                0.95
            } else {
                0.35
            };
            let fill = if tag.hovered && tag.enabled {
                rgba(gray(62, 222), 1.)
            } else {
                none
            };
            (fill, rgba(glyph, strength), none)
        }
    }
}

/// What the layout needs to know besides the plates.
pub struct Options {
    pub dark: bool,
    /// The plate the pointer is over (its tools at full strength, as the current plate's).
    pub hovered_plate: Option<usize>,
    /// Show the plate tools and the rename pencil (Prepare).
    pub tools: bool,
    /// The scene can be edited (else the tools that edit are off).
    pub editable: bool,
}

/// Lays out every plate's strip and name line for this frame (none for a single plate).
/// `project` maps world to screen points; `pointer` with `occluded` (whether an object lies
/// in front of the ground under the pointer) decides which tag is hovered.
#[allow(clippy::too_many_arguments)]
pub fn layout(
    ctx: &egui::Context,
    cache: &mut TagCache,
    bed: &Bed,
    plates: &[Plate],
    options: &Options,
    view: egui::Rect,
    project: impl Fn(Vec3) -> Option<Pos2>,
    pointer: Option<Pos2>,
    occluded: impl FnOnce() -> bool,
) -> Vec<PlateTag> {
    let mut tags = vec![];
    if plates.len() < 2 {
        // A single plate needs no tag; the plate bar names it.
        cache.images.clear();
        return tags;
    }
    let mut used = vec![];
    let (min, max) = (bed.min.truncate(), bed.max.truncate());
    let front = bed.overhang().front;
    let f = factor(min, max);
    let name_max = name_max_width(min, max);
    let quad = |rect: [f32; 4]| -> Option<[Pos2; 4]> {
        let [x0, y0, x1, y1] = rect;
        Some([
            project(Vec3::new(x0, y0, 0.))?,
            project(Vec3::new(x1, y0, 0.))?,
            project(Vec3::new(x1, y1, 0.))?,
            project(Vec3::new(x0, y1, 0.))?,
        ])
    };
    for plate in plates {
        let (o_min, o_max) = (min + plate.origin, max + plate.origin);
        let strip = strip_rect(o_min, o_max, front);
        let line = name_line_rect(o_min, o_max, front);
        let Some(strip_q) = quad(strip) else {
            continue;
        };
        let Some(line_q) = quad(line) else {
            continue;
        };
        let all = egui::Rect::from_points(&strip_q).union(egui::Rect::from_points(&line_q));
        if !view.intersects(all) {
            continue;
        }
        // Screen points per unit (foreshortened): across the strip where it is largest in
        // view, so a strip reaching off screen reads as its visible part does.
        let across = |k: usize| {
            let x = strip[0] + (strip[2] - strip[0]) * k as f32 / 4.;
            let a = project(Vec3::new(x, strip[1], 0.))?;
            let b = project(Vec3::new(x, strip[3], 0.))?;
            Some((view.contains(a) || view.contains(b), a.distance(b)))
        };
        let samples: Vec<(bool, f32)> = (0..=4).filter_map(across).collect();
        let in_view = samples
            .iter()
            .filter(|(seen, _)| *seen)
            .map(|(_, d)| *d)
            .reduce(f32::max);
        // None in view: through the middle.
        let Some(span) = in_view.or_else(|| across(2).map(|(_, d)| d)) else {
            continue;
        };
        let unit_px = span / STRIP_H;
        let show_name = NAME_H * unit_px >= NAME_MIN_PX;
        let show_tools = show_name && options.tools && ICON * unit_px >= ICON_MIN_PX;
        let (number, name) = texts(plate);
        let number = cache.image(
            ctx,
            &mut used,
            Content::Chip {
                text: number,
                em: 0.62,
                pad: 0.3,
                min_width: 1.35,
                max_width: 8.,
            },
        );
        let aspect = number.width as f32 / TEXELS;
        let mut push = |kind: TagKind, rect: [f32; 4], image: Arc<TagImage>, enabled: bool| {
            if let Some(screen) = quad(rect) {
                tags.push(PlateTag {
                    plate: plate.index,
                    kind,
                    screen,
                    draw: TagDraw {
                        rect,
                        image,
                        fill: [0.; 4],
                        text: [0.; 4],
                        rim: [0.; 4],
                    },
                    hovered: false,
                    enabled,
                });
            }
        };
        if !show_name {
            // Zoomed out too far for the name line: only the number, grown into the room of
            // the strip and the name line so the plates stay told apart.
            let h = (NUMBER_TARGET_PX / unit_px).clamp(BADGE_H, DEPTH - SHEET_CLEAR);
            if h * unit_px >= NUMBER_MIN_PX {
                let [x0, _, _, y1] = strip;
                let rect = [x0, y1 - h * f, x0 + aspect * h * f, y1];
                push(TagKind::Number, rect, number, true);
            }
            continue;
        }
        // The strip.
        let strip_w = (strip[2] - strip[0]) / (STRIP_H * f);
        let image = cache.image(
            ctx,
            &mut used,
            Content::Chip {
                text: String::new(),
                em: 0.,
                pad: 0.,
                min_width: strip_w,
                max_width: strip_w,
            },
        );
        push(TagKind::Strip, strip, image, true);
        // The number badge on its left, "locked" after it.
        let cy = (strip[1] + strip[3]) / 2.;
        let (by0, by1) = (cy - BADGE_H * f / 2., cy + BADGE_H * f / 2.);
        let bx0 = strip[0] + PAD * f;
        let bx1 = bx0 + aspect * BADGE_H * f;
        push(TagKind::Number, [bx0, by0, bx1, by1], number, true);
        if plate.locked {
            let image = cache.image(
                ctx,
                &mut used,
                Content::Chip {
                    text: "locked".into(),
                    em: 0.58,
                    pad: 0.15,
                    min_width: 1.,
                    max_width: 8.,
                },
            );
            let lx0 = bx1 + 0.4 * f;
            let lx1 = lx0 + image.width as f32 / TEXELS * BADGE_H * f;
            push(TagKind::Locked, [lx0, by0, lx1, by1], image, true);
        }
        // The tools on its right.
        if show_tools {
            for (tool, rect) in PlateTool::ALL
                .into_iter()
                .zip(tool_rects(o_min, o_max, front))
            {
                let image = cache.image(ctx, &mut used, Content::Icon(tool.icon(plate.locked)));
                push(
                    TagKind::Tool(tool),
                    rect,
                    image,
                    options.editable || !tool.edits(),
                );
            }
        }
        // The name line in front of the strip: the name (or a dim "Untitled"), elided to
        // the plate's width less the pencil, and the pencil after it.
        if show_name {
            let image = cache.image(
                ctx,
                &mut used,
                Content::Chip {
                    text: name.unwrap_or_else(|| "Untitled".into()),
                    em: 0.72,
                    pad: 0.04,
                    min_width: 0.5,
                    max_width: name_max,
                },
            );
            let nx1 = line[0] + image.width as f32 / TEXELS * NAME_H * f;
            push(TagKind::Name, [line[0], line[1], nx1, line[3]], image, true);
            if options.tools {
                let image = cache.image(ctx, &mut used, Content::Icon(Icon::Rename));
                let px0 = nx1 + 0.3 * f;
                push(
                    TagKind::Rename,
                    [px0, line[1], px0 + NAME_H * f, line[3]],
                    image,
                    options.editable,
                );
            }
        }
    }
    cache.images.retain(|k, _| used.contains(k));
    // The hovered tag: the topmost under the pointer, unless an object stands in front of
    // the ground there.
    if let Some(p) = pointer
        && let Some(i) = tags.iter().rposition(|t| contains(&t.screen, p))
        && !occluded()
    {
        tags[i].hovered = true;
    }
    let hovered_plate = tags
        .iter()
        .find(|t| t.hovered)
        .map(|t| t.plate)
        .or(options.hovered_plate);
    for t in &mut tags {
        let Some(plate) = plates.iter().find(|p| p.index == t.plate) else {
            continue;
        };
        let active = plate.current || hovered_plate == Some(plate.index);
        let (fill, text, rim) = colors(plate, t.kind, options.dark, active, t);
        t.draw.fill = fill;
        t.draw.text = text;
        t.draw.rim = rim;
    }
    tags
}

/// Whether `p` lies in the convex quad `q` (either winding).
pub fn contains(q: &[Pos2; 4], p: Pos2) -> bool {
    let mut sign = 0.;
    for i in 0..4 {
        let (a, b) = (q[i], q[(i + 1) % 4]);
        let c = (b - a).x * (p - a).y - (b - a).y * (p - a).x;
        if c.abs() < 1e-6 {
            continue;
        }
        if sign == 0. {
            sign = c.signum();
        } else if c.signum() != sign {
            return false;
        }
    }
    sign != 0.
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plate(index: usize) -> Plate {
        Plate {
            index,
            origin: Vec2::ZERO,
            current: false,
            name: None,
            locked: false,
            objects: 1,
            alert: 0,
            notes: vec![],
            status: crate::plate_batch::PlateStatus::NotSliced,
        }
    }

    /// The common beds with their sheets' overhang (front, back; from the pinned bed models):
    /// U1 270 × 270, CORE One 250 × 220, XL 360 × 360.
    const BEDS: [(f32, f32, f32, f32); 3] = [
        (270., 270., 11.5, 11.5),
        (250., 220., 13.3, 35.7),
        (360., 360., 13.3, 7.5),
    ];

    /// The strip and the name line lie on the ground between the plate's sheet and the next
    /// row's sheet, across the plate's width, clear of both and clearly nearer their own.
    #[test]
    fn strip_and_name_line_sit_between_the_sheets_nearer_their_own() {
        for (w, d, front, back) in BEDS {
            let (min, max) = (Vec2::ZERO, Vec2::new(w, d));
            let overhang = Overhang {
                front,
                back,
                ..Overhang::default()
            };
            let gap = row_gap(min, max, overhang);
            // Never closer than before (a fifth of the bed), on these beds much wider.
            assert!(gap > d / crate::plates::GAP_DIVISOR, "{w}×{d}: {gap}");
            let own_rim = -front;
            let next_rim = -gap + back;
            let s = strip_rect(min, max, front);
            let n = name_line_rect(min, max, front);
            // In front of the own sheet with a clear distance, the name line in front of the
            // strip, and ground left before the next row's sheet.
            let to_own = own_rim - s[3];
            let to_next = n[1] - next_rim;
            assert!(to_own > 0.01 * d, "{w}×{d}: {to_own}");
            assert!(n[3] < s[1], "{w}×{d}");
            assert!(to_next >= 2.9 * to_own, "{w}×{d}: {to_own} vs {to_next}");
            assert!((reach(min, max, overhang) + n[1]).abs() < 1e-3);
            // Both across the plate's width, left-aligned.
            assert_eq!((s[0], s[2]), (0., w));
            assert!(n[0] > s[0] && n[2] < s[2]);
            // The icons fit the strip, right-aligned, and never overlap each other.
            let t = tool_rects(min, max, front);
            assert!(t[0][0] > s[0] + 0.5 * w, "{w}×{d}: icons crowd the badge");
            assert!(t.last().unwrap()[2] < s[2]);
            for r in &t {
                assert!(r[1] > s[1] && r[3] < s[3]);
            }
            for p in t.windows(2) {
                assert!(p[0][2] < p[1][0]);
            }
        }
    }

    #[test]
    fn the_name_and_pencil_fit_the_plate_width() {
        let (min, max) = (Vec2::ZERO, Vec2::new(270., 270.));
        let f = factor(min, max);
        let n = name_line_rect(min, max, 0.);
        let widest = n[0] + name_max_width(min, max) * NAME_H * f;
        // The pencil (0.3 + NAME_H units) still ends inside the line.
        assert!(widest + (0.3 + NAME_H) * f <= n[2] + 1e-3);
    }

    #[test]
    fn texts_carry_alerts() {
        let mut p = plate(8);
        p.alert = 1;
        assert_eq!(texts(&p), ("!\u{2009}9".into(), None));
        p.alert = 2;
        p.name = Some("Lid".into());
        assert_eq!(texts(&p), ("✕\u{2009}9".into(), Some("Lid".into())));
    }

    #[test]
    fn quad_contains_points_inside_only() {
        let q = [
            Pos2::new(0., 10.),
            Pos2::new(10., 10.),
            Pos2::new(8., 0.),
            Pos2::new(2., 0.),
        ];
        assert!(contains(&q, Pos2::new(5., 5.)));
        assert!(!contains(&q, Pos2::new(0.5, 1.)));
        assert!(!contains(&q, Pos2::new(5., 11.)));
    }

    /// Lays out two U1 plates seen from straight above at `px_per_mm`, the pointer at `at`
    /// (world mm on the ground) with nothing in front.
    fn top_view(
        ctx: &egui::Context,
        px_per_mm: f32,
        at: Option<Vec2>,
        name: Option<&str>,
    ) -> Vec<PlateTag> {
        let mut bed = Bed::default();
        (bed.min, bed.max) = (Vec3::ZERO, Vec3::new(270., 270., 270.));
        let mut a = plate(0);
        a.current = true;
        a.name = name.map(str::to_string);
        let mut b = plate(1);
        b.origin = Vec2::new(324., 0.);
        let project = |w: Vec3| Some(Pos2::new(w.x * px_per_mm, 1000. - w.y * px_per_mm));
        let to_screen = |v: Vec2| Pos2::new(v.x * px_per_mm, 1000. - v.y * px_per_mm);
        let mut cache = TagCache::default();
        layout(
            ctx,
            &mut cache,
            &bed,
            &[a, b],
            &Options {
                dark: true,
                hovered_plate: None,
                tools: true,
                editable: true,
            },
            egui::Rect::from_min_size(Pos2::new(-500., -500.), EVec2::splat(4000.)),
            project,
            at.map(to_screen),
            || false,
        )
    }

    fn ctx() -> egui::Context {
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(Default::default(), |_| {});
        output.textures_delta.clear();
        ctx
    }

    #[test]
    fn the_pointer_hits_the_icon_under_it() {
        let ctx = ctx();
        let (min, max) = (Vec2::ZERO, Vec2::new(270., 270.));
        let rects = tool_rects(min, max, 0.);
        for (tool, r) in PlateTool::ALL.into_iter().zip(&rects) {
            let centre = Vec2::new((r[0] + r[2]) / 2., (r[1] + r[3]) / 2.);
            let tags = top_view(&ctx, 3., Some(centre), None);
            let hovered: Vec<_> = tags.iter().filter(|t| t.hovered).collect();
            assert_eq!(hovered.len(), 1);
            assert_eq!(
                (hovered[0].plate, hovered[0].kind),
                (0, TagKind::Tool(tool))
            );
        }
        // Between two icons the strip itself is hit (a click there shows the plate).
        let between = Vec2::new(
            (rects[0][2] + rects[1][0]) / 2.,
            (rects[0][1] + rects[0][3]) / 2.,
        );
        let tags = top_view(&ctx, 3., Some(between), None);
        assert_eq!(
            tags.iter().find(|t| t.hovered).unwrap().kind,
            TagKind::Strip
        );
        // On the plate itself no tag is.
        let tags = top_view(&ctx, 3., Some(Vec2::new(100., 100.)), None);
        assert!(tags.iter().all(|t| !t.hovered));
    }

    #[test]
    fn a_long_name_is_elided_before_the_pencil_and_zoomed_out_parts_drop_in_order() {
        let ctx = ctx();
        let long = "A very long plate name that goes on and on past the edge of the plate";
        let tags = top_view(&ctx, 3., None, Some(long));
        let line = name_line_rect(Vec2::ZERO, Vec2::new(270., 270.), 0.);
        let name = tags
            .iter()
            .find(|t| t.plate == 0 && t.kind == TagKind::Name)
            .unwrap();
        let pencil = tags
            .iter()
            .find(|t| t.plate == 0 && t.kind == TagKind::Rename)
            .unwrap();
        assert!(name.draw.rect[2] < pencil.draw.rect[0]);
        assert!(pencil.draw.rect[2] <= line[2] + 1e-3);
        // Unnamed plates still have a name line ("Untitled") and a pencil.
        assert!(tags.iter().any(|t| t.plate == 1 && t.kind == TagKind::Name));
        assert!(
            tags.iter()
                .any(|t| t.plate == 1 && t.kind == TagKind::Rename)
        );
        // Zooming out: the icons go first, then the name line (with its pencil), then all.
        let kinds = |px: f32| -> Vec<TagKind> {
            top_view(&ctx, px, None, Some("Lid"))
                .into_iter()
                .filter(|t| t.plate == 0)
                .map(|t| t.kind)
                .collect()
        };
        let f = 1.35;
        let tools = |k: &Vec<TagKind>| k.iter().any(|k| matches!(k, TagKind::Tool(_)));
        let at = |unit_px: f32| kinds(unit_px / f + 1e-3);
        let k = at(ICON_MIN_PX / ICON);
        assert!(tools(&k) && k.contains(&TagKind::Name));
        let k = at(NAME_MIN_PX / NAME_H);
        assert!(!tools(&k) && k.contains(&TagKind::Name) && k.contains(&TagKind::Rename));
        let k = at(NAME_MIN_PX / NAME_H * 0.95);
        assert_eq!(k, vec![TagKind::Number], "only a grown number");
        let k = at(NUMBER_MIN_PX / (DEPTH - SHEET_CLEAR));
        assert_eq!(k, vec![TagKind::Number]);
        assert!(at(NUMBER_MIN_PX / (DEPTH - SHEET_CLEAR) * 0.9).is_empty());
    }
}
