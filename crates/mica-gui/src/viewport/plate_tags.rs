// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: orca@8500fcdcca src/slic3r/GUI/PartPlate.cpp :: PartPlate::calc_vertex_for_number, PartPlate::generate_plate_name_texture, PartPlate::calc_vertex_for_plate_name_edit_icon
// Changes: Placement and sizing kept: both tags lie flat on the ground in plate space, sized by factor = bed depth / 200 mm (Mica grows them up to 26 units, still inside the gap, when zoomed out so they stay readable); the number right of the bed's front-right corner (ICON_GAP_LEFT into the gap, TEXT_OFFSET_Y above the front edge), the name behind the back edge from the left corner (TEXT_OFFSET_Y). Mica draws each as a rounded chip with an alert mark before the number and "locked" after the name, elides the name to the bed width with an ellipsis (full name on hover) and caps the number chip to the gap; text is rasterised from egui's font atlas into coverage masks coloured in the shader; tags whose projected chip is too small to read are left out; hit testing uses the projected quad and lets objects in front win. No name edit icon (rename lives in the plate menu and bar).

//! Plate tags: the plate number and name lying on the ground next to each plate, as Orca
//! draws them, so they foreshorten with the plates and sit in the gap beside their own
//! plate rather than over objects. This module lays them out (ground rectangles, texture
//! masks, colours, which are readable) and hit-tests them; `render::tags` draws them.
use crate::render::{TagDraw, TagImage};
use crate::scene::{bed::Bed, plates::Plate};
use crate::theme;
use eframe::egui::{self, Color32, Pos2, Vec2 as EVec2};
use glam::{Vec2, Vec3};
use std::{
    collections::HashMap,
    hash::{Hash, Hasher},
    sync::Arc,
};

// Orca's PartPlate constants, in units of `factor` (bed depth / 200 mm).
/// `PARTPLATE_ICON_GAP_LEFT`: from the bed's right edge to the number.
const ICON_GAP_LEFT: f32 = 3.;
/// `PARTPLATE_TEXT_OFFSET_Y`: from the bed's front (number) or back (name) edge.
const TEXT_OFFSET_Y: f32 = 1.;
/// The chips' smallest heights: Orca's number square less its offsets top and bottom
/// (`PARTPLATE_ICON_SIZE` 16 − 2) and, for the name, Orca's name text height
/// (`PARTPLATE_EDIT_PLATE_NAME_ICON_SIZE` 9) plus a margin around it.
const NUMBER_HEIGHT: f32 = 14.;
const NAME_HEIGHT: f32 = 12.;
/// Their largest heights. Zoomed out, the chips grow towards these (Mica) to stay readable;
/// the gap is a fifth of the bed (40 units deep), so even then each tag lies nearer its own
/// plate (1–3 units) than the next one (at least 11).
const NUMBER_MAX_HEIGHT: f32 = 26.;
const NAME_MAX_HEIGHT: f32 = 26.;
/// Projected chip heights (screen points) the growth aims at.
const NUMBER_TARGET_PX: f32 = 22.;
const NAME_TARGET_PX: f32 = 20.;

/// Texture rows of a chip; the text's em is a fraction of it.
const TEXELS: f32 = 64.;
/// Smallest projected chip height (screen points) at which the text still reads; smaller
/// chips are left out rather than drawn as mush (the name first, then the number).
const NAME_MIN_PX: f32 = 13.;
const NUMBER_MIN_PX: f32 = 10.;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TagKind {
    Number,
    Name,
}

pub struct PlateTag {
    pub plate: usize,
    pub kind: TagKind,
    /// The quad on screen: front-left, front-right, back-right, back-left.
    pub screen: [Pos2; 4],
    pub draw: TagDraw,
    /// The pointer is on it and no object in front of it.
    pub hovered: bool,
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

/// Orca's `factor`: tags scale with the bed's depth (200 mm = 1).
fn factor(min: Vec2, max: Vec2) -> f32 {
    ((max.y - min.y) / 200.).max(0.05)
}

/// The number's ground rectangle `[x0, y0, x1, y1]` for a chip `width` × `height` units (Orca
/// `calc_vertex_for_number`, the "in the bottom" branch: `bed_ext[1]` is the front-right
/// corner, so the number lies in the gap right of the bed, its bottom on the front edge).
fn number_rect(min: Vec2, max: Vec2, width: f32, height: f32) -> [f32; 4] {
    let f = factor(min, max);
    let x0 = max.x + ICON_GAP_LEFT * f;
    let y0 = min.y + TEXT_OFFSET_Y * f;
    [x0, y0, x0 + width * f, y0 + height * f]
}

/// The name's ground rectangle for a chip `width` × `height` units (Orca
/// `generate_plate_name_texture`: `bed_ext[3]` is the back-left corner, so the name lies
/// behind the back edge, starting at the left edge).
fn name_rect(min: Vec2, max: Vec2, width: f32, height: f32) -> [f32; 4] {
    let f = factor(min, max);
    let y0 = max.y + TEXT_OFFSET_Y * f;
    [min.x, y0, min.x + width * f, y0 + height * f]
}

/// How wide (in `factor` units) each chip may grow: the name to the bed's width (then it is
/// elided), the number to the gap beside the bed less 11 units towards the next plate (then
/// it is drawn smaller: a number is never cut).
fn max_widths(min: Vec2, max: Vec2) -> (f32, f32) {
    let f = factor(min, max);
    let width = (max.x - min.x) / f;
    let gap = width / crate::plates::GAP_DIVISOR;
    ((gap - ICON_GAP_LEFT - 11.).max(NUMBER_HEIGHT), width)
}

/// What a plate's tags say: the number with its alert mark, and the name (or nothing).
fn texts(plate: &Plate) -> (String, Option<String>) {
    let mut number = format!("{}", plate.index + 1);
    if plate.alert > 0 {
        number = format!(
            "{}\u{2009}{number}",
            if plate.alert > 1 { "✕" } else { "!" }
        );
    }
    let name = match (&plate.name, plate.locked) {
        (Some(n), true) => Some(format!("{n} · locked")),
        (Some(n), false) => Some(n.clone()),
        (None, true) => Some("locked".to_string()),
        (None, false) => None,
    };
    (number, name)
}

/// Linear RGBA for the renderer.
fn rgba(c: Color32, a: f32) -> [f32; 4] {
    let [r, g, b] = theme::c3_of(c);
    [r, g, b, a]
}

/// Chip fill, text and rim colours for a plate's tags (the old screen labels' scheme: the
/// current plate in the accent colour and outlined, alerts in warning or danger).
fn colors(plate: &Plate, dark: bool, hovered: bool) -> ([f32; 4], [f32; 4], [f32; 4]) {
    let text = if plate.alert > 1 {
        theme::DANGER
    } else if plate.alert == 1 && !plate.current {
        theme::WARNING
    } else if plate.current {
        if dark {
            theme::ACCENT_BRIGHT
        } else {
            theme::ACCENT_DIM
        }
    } else if dark {
        Color32::from_gray(170)
    } else {
        Color32::from_gray(80)
    };
    let fill = if dark {
        rgba(
            Color32::from_rgb(20, 22, 26),
            if hovered { 0.92 } else { 0.8 },
        )
    } else {
        rgba(Color32::WHITE, if hovered { 0.98 } else { 0.85 })
    };
    let rim = if plate.current {
        rgba(theme::ACCENT, 1.)
    } else {
        rgba(text, if hovered { 0.6 } else { 0.2 })
    };
    (fill, rgba(text, 1.), rim)
}

/// Rasterises a chip with `text` into coverage masks (r chip, g text, b rim) from egui's font
/// atlas: `TEXELS` rows high, at least `min_width` and at most `max_width` chip heights wide
/// (a longer text ends in "…"). Returns the image and its width in chip heights.
fn rasterize(
    ctx: &egui::Context,
    text: &str,
    em: f32,
    pad: f32,
    min_width: f32,
    max_width: f32,
) -> TagImage {
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
    let text_w = galley.size().x * ppp;
    let width = (text_w + 2. * pad).max(min_width * h).ceil().max(1.) as u32;
    let height = h as u32;
    let mut texels = vec![0u8; (width * height * 4) as usize];
    // The chip: a rounded rectangle, antialiased, with a rim band inside its edge.
    let (w, hh) = (width as f32, h);
    let radius = 0.3 * h;
    let rim = 0.075 * h;
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
    // The text, centred on the chip's height (its line box) after the padding (or centred
    // on a chip wider than the text).
    let origin = EVec2::new(
        ((w - text_w) / 2.).max(pad),
        (hh - galley.size().y * ppp) / 2.,
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
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (
        text,
        em.to_bits(),
        min_width.to_bits(),
        max_width.to_bits(),
        ppp.to_bits(),
    )
        .hash(&mut h);
    TagImage {
        key: h.finish(),
        width,
        height,
        texels,
    }
}

impl TagCache {
    fn image(
        &mut self,
        ctx: &egui::Context,
        used: &mut Vec<u64>,
        text: &str,
        kind: TagKind,
        max_width: f32,
    ) -> Arc<TagImage> {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (
            text,
            kind,
            max_width.to_bits(),
            ctx.pixels_per_point().to_bits(),
        )
            .hash(&mut h);
        let key = h.finish();
        used.push(key);
        self.images
            .entry(key)
            .or_insert_with(|| {
                Arc::new(match kind {
                    TagKind::Number => rasterize(ctx, text, 0.6, 0.28, 1., max_width),
                    TagKind::Name => rasterize(ctx, text, 0.56, 0.4, 1., max_width),
                })
            })
            .clone()
    }
}

/// Lays out every plate's tags for this frame (none for a single plate). `project` maps world
/// to screen points; `pointer` with `occluded` (whether an object lies in front of the ground
/// under the pointer) decides which tag is hovered.
#[allow(clippy::too_many_arguments)]
pub fn layout(
    ctx: &egui::Context,
    cache: &mut TagCache,
    bed: &Bed,
    plates: &[Plate],
    dark: bool,
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
    let (number_max, name_max) = max_widths(min, max);
    // How tall the chips are this frame: Orca's size, grown (in whole units, up to the
    // maximum) until a typical plate's projected chip reaches the target height. "Typical"
    // is the upper quartile of the plates in view (the nearer ones, which are looked at),
    // measured at their number's place.
    let f = factor(min, max);
    let mut per_unit: Vec<f32> = plates
        .iter()
        .filter_map(|p| {
            let a = (Vec2::new(max.x, min.y) + p.origin).extend(0.);
            let (pa, pb) = (project(a)?, project(a + Vec3::Y * (10. * f))?);
            view.contains(pa).then(|| pa.distance(pb) / 10.)
        })
        .collect();
    per_unit.sort_by(f32::total_cmp);
    let unit_px = per_unit
        .get(per_unit.len() * 3 / 4)
        .copied()
        .unwrap_or(1.)
        .max(1e-3);
    let grown = |base: f32, max: f32, target: f32| (target / unit_px).ceil().clamp(base, max);
    let number_h = grown(NUMBER_HEIGHT, NUMBER_MAX_HEIGHT, NUMBER_TARGET_PX);
    let name_h = grown(NAME_HEIGHT, NAME_MAX_HEIGHT, NAME_TARGET_PX);
    for plate in plates {
        let (number, name) = texts(plate);
        let (o_min, o_max) = (min + plate.origin, max + plate.origin);
        let mut push = |kind: TagKind, text: &str, max_w: f32, min_px: f32, height: f32| {
            let image = cache.image(ctx, &mut used, text, kind, max_w);
            let aspect = image.width as f32 / image.height as f32;
            let height = match kind {
                TagKind::Number => height.min(number_max / aspect),
                TagKind::Name => height,
            };
            let width = aspect * height;
            let rect = match kind {
                TagKind::Number => number_rect(o_min, o_max, width, height),
                TagKind::Name => name_rect(o_min, o_max, width, height),
            };
            let [x0, y0, x1, y1] = rect;
            let corners = [
                Vec3::new(x0, y0, 0.),
                Vec3::new(x1, y0, 0.),
                Vec3::new(x1, y1, 0.),
                Vec3::new(x0, y1, 0.),
            ];
            let Some(screen) = corners
                .iter()
                .map(|c| project(*c))
                .collect::<Option<Vec<_>>>()
            else {
                return;
            };
            let screen = [screen[0], screen[1], screen[2], screen[3]];
            if !view.intersects(egui::Rect::from_points(&screen)) {
                return;
            }
            // Readable? The chip's projected height through its middle (foreshortened).
            let mid = (x0 + x1) / 2.;
            let (Some(a), Some(b)) = (
                project(Vec3::new(mid, y0, 0.)),
                project(Vec3::new(mid, y1, 0.)),
            ) else {
                return;
            };
            if a.distance(b) < min_px {
                return;
            }
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
            });
        };
        push(
            TagKind::Number,
            &number,
            // Laid out unbounded; a wide number is drawn smaller instead.
            8.,
            NUMBER_MIN_PX,
            number_h,
        );
        if let Some(name) = &name {
            push(TagKind::Name, name, name_max / name_h, NAME_MIN_PX, name_h);
        }
    }
    cache.images.retain(|k, _| used.contains(k));
    // The hovered tag: under the pointer, unless an object stands in front of it there.
    if let Some(p) = pointer
        && let Some(i) = tags.iter().rposition(|t| contains(&t.screen, p))
        && !occluded()
    {
        tags[i].hovered = true;
    }
    for t in &mut tags {
        let Some(plate) = plates.iter().find(|p| p.index == t.plate) else {
            continue;
        };
        let (fill, text, rim) = colors(plate, dark, t.hovered);
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

    /// Orca's placement on a 270 × 270 bed (factor 1.35): the number right of the front-right
    /// corner, the name behind the back edge from the left corner.
    #[test]
    fn tags_sit_where_orca_puts_them() {
        let (min, max) = (Vec2::ZERO, Vec2::new(270., 270.));
        let n = number_rect(min, max, NUMBER_HEIGHT, NUMBER_HEIGHT);
        assert!((n[0] - (270. + 3. * 1.35)).abs() < 1e-3);
        assert!((n[1] - 1.35).abs() < 1e-3);
        assert!((n[3] - n[1] - NUMBER_HEIGHT * 1.35).abs() < 1e-3);
        let m = name_rect(min, max, 20., NAME_HEIGHT);
        assert_eq!(m[0], 0.);
        assert!((m[1] - (270. + 1.35)).abs() < 1e-3);
        assert!((m[2] - 20. * 1.35).abs() < 1e-3);
        // Both stay in the gap: closer to their own plate than to the next one.
        let gap = 270. / crate::plates::GAP_DIVISOR;
        let (number_max, _) = max_widths(min, max);
        assert!(n[0] + number_max * 1.35 <= 270. + gap - 11. * 1.35 + 1e-3);
        let m = name_rect(min, max, 20., NAME_MAX_HEIGHT);
        assert!(m[3] <= 270. + gap - 11. * 1.35);
    }

    #[test]
    fn texts_carry_alerts_and_lock() {
        let mut p = Plate {
            index: 8,
            origin: Vec2::ZERO,
            current: false,
            name: None,
            locked: false,
            objects: 1,
            alert: 1,
            notes: vec![],
            status: crate::plate_batch::PlateStatus::NotSliced,
        };
        assert_eq!(texts(&p), ("!\u{2009}9".into(), None));
        p.alert = 2;
        p.locked = true;
        p.name = Some("Lid".into());
        assert_eq!(
            texts(&p),
            ("✕\u{2009}9".into(), Some("Lid · locked".into()))
        );
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
}
