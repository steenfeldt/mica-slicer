// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/Emboss.cpp :: Emboss::text2vshapes
// Source: prusaslicer@b028299c77 src/libslic3r/Emboss.cpp :: letter2shapes
// Source: prusaslicer@b028299c77 src/libslic3r/Emboss.cpp :: get_glyph
// Source: prusaslicer@b028299c77 src/libslic3r/Emboss.cpp :: align_shape
// Source: prusaslicer@b028299c77 src/libslic3r/Emboss.cpp :: get_align_y_offset
// Source: prusaslicer@b028299c77 src/libslic3r/Emboss.cpp :: Emboss::get_line_height
// Changes: Glyph outlines come from skrifa and placement from harfrust shaping (kerning, ligatures, marks) instead of stb_truetype advances; curves are flattened adaptively to PrusaSlicer's 0.0125 mm resolution; char_gap / line_gap, boldness (outline offset), skew, the alignment offsets and the 1/3-ascent vertical centre follow upstream; a variable font's weight axis gives bold; '\t' is four spaces; letters the font lacks are reported. Coordinates are Mica's scaled millimetres (1e-6 mm).
// Upstream attribution (Emboss.cpp):
// Copyright (c) Prusa Research 2021 - 2023 Filip Sykala @Jony01, Lukáš Matěna @lukasmatena, Vojtěch Bubník @bubnikv
//
// PrusaSlicer is released under the terms of the AGPLv3 or higher

//! Text → healed outline in the text frame (mm, Y up).

use super::fonts::FontFace;
use mica_core::emboss::{HAlign, TextSource, VAlign};
use mica_core::geometry::{ExPolygon, Point, Polygon, clip};
use skrifa::{
    MetadataProvider, Tag,
    instance::{LocationRef, Size},
    outline::{DrawSettings, OutlinePen},
};

/// PrusaSlicer's curve resolution for glyphs (mm).
const RESOLUTION_MM: f64 = 0.0125;
/// PrusaSlicer's vertical centre of a line: a third of the ascent above the baseline.
const ASCENT_CENTER: f64 = 1. / 3.;

/// A text's outline and what could not be drawn.
#[derive(Debug, Default)]
pub struct TextOutline {
    pub shape: Vec<ExPolygon>,
    /// Letters the font has no glyph for.
    pub missing: Vec<char>,
}

/// The font's units per em (1000 when it cannot be read).
pub fn units_per_em(face: &FontFace, data: &[u8]) -> u16 {
    use skrifa::raw::TableProvider;
    skrifa::FontRef::from_index(data, face.index)
        .ok()
        .and_then(|f| f.head().ok().map(|h| h.units_per_em()))
        .filter(|&u| u >= 16)
        .unwrap_or(1000)
}

/// Collects a glyph's contours in font units, flattening curves to `tol` font units.
struct Pen {
    tol: f64,
    contours: Vec<Vec<[f64; 2]>>,
    cur: Vec<[f64; 2]>,
}
impl Pen {
    fn last(&self) -> [f64; 2] {
        self.cur.last().copied().unwrap_or([0., 0.])
    }
    fn segments(dd: f64, tol: f64) -> usize {
        ((dd / tol).sqrt().ceil() as usize).clamp(1, 64)
    }
    fn finish(&mut self) {
        let mut c = std::mem::take(&mut self.cur);
        if c.len() > 1 && c.first() == c.last() {
            c.pop();
        }
        if c.len() >= 3 {
            self.contours.push(c);
        }
    }
}
impl OutlinePen for Pen {
    fn move_to(&mut self, x: f32, y: f32) {
        self.finish();
        self.cur.push([f64::from(x), f64::from(y)]);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.cur.push([f64::from(x), f64::from(y)]);
    }
    fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        let p0 = self.last();
        let (p1, p2) = ([f64::from(cx), f64::from(cy)], [f64::from(x), f64::from(y)]);
        let dd = ((p0[0] - 2. * p1[0] + p2[0]).hypot(p0[1] - 2. * p1[1] + p2[1])) / 4.;
        let n = Self::segments(dd, self.tol);
        for i in 1..=n {
            let t = i as f64 / n as f64;
            let u = 1. - t;
            self.cur.push([
                u * u * p0[0] + 2. * u * t * p1[0] + t * t * p2[0],
                u * u * p0[1] + 2. * u * t * p1[1] + t * t * p2[1],
            ]);
        }
    }
    fn curve_to(&mut self, c0x: f32, c0y: f32, c1x: f32, c1y: f32, x: f32, y: f32) {
        let p0 = self.last();
        let p1 = [f64::from(c0x), f64::from(c0y)];
        let p2 = [f64::from(c1x), f64::from(c1y)];
        let p3 = [f64::from(x), f64::from(y)];
        let d1 = (p0[0] - 2. * p1[0] + p2[0]).hypot(p0[1] - 2. * p1[1] + p2[1]);
        let d2 = (p1[0] - 2. * p2[0] + p3[0]).hypot(p1[1] - 2. * p2[1] + p3[1]);
        let n = Self::segments(0.75 * d1.max(d2), self.tol);
        for i in 1..=n {
            let t = i as f64 / n as f64;
            let u = 1. - t;
            let (a, b, c, d) = (u * u * u, 3. * u * u * t, 3. * u * t * t, t * t * t);
            self.cur.push([
                a * p0[0] + b * p1[0] + c * p2[0] + d * p3[0],
                a * p0[1] + b * p1[1] + c * p2[1] + d * p3[1],
            ]);
        }
    }
    fn close(&mut self) {
        self.finish();
    }
}

/// The healed outline of `src.text` in `face` (whose file bytes are `data`). The text frame's
/// origin is the alignment anchor (PrusaSlicer's `align_shape`).
pub fn outline(src: &TextSource, face: &FontFace, data: &[u8]) -> Result<TextOutline, String> {
    let font = skrifa::FontRef::from_index(data, face.index)
        .map_err(|e| format!("{} {}: unreadable font ({e})", face.family, face.face))?;
    let upem = units_per_em(face, data);
    let size = src.size_mm.clamp(0.1, 1000.);
    let mm_per_unit = size / f64::from(upem);
    // A variable font's weight axis gives bold (and keeps a light default light).
    let mut variations: Vec<(Tag, f32)> = Vec::new();
    if let Some((lo, hi)) = face.weight_axis {
        let w = if src.bold {
            700f32.clamp(lo, hi)
        } else {
            face.weight.clamp(lo, hi)
        };
        variations.push((Tag::new(b"wght"), w));
    }
    let location = font
        .axes()
        .location(variations.iter().map(|&(t, v)| (t, v)));
    let loc = LocationRef::from(&location);
    let metrics = font.metrics(Size::unscaled(), loc);
    let ascent = f64::from(metrics.ascent);
    let line_height =
        f64::from(metrics.ascent - metrics.descent + metrics.leading) + f64::from(src.line_gap);
    let tol = (RESOLUTION_MM / mm_per_unit).max(RESOLUTION_MM);
    let shaper_data = harfrust::ShaperData::new(&font);
    let instance = harfrust::ShaperInstance::from_variations(
        &font,
        variations
            .iter()
            .map(|&(tag, value)| harfrust::Variation { tag, value }),
    );
    let shaper = shaper_data.shaper(&font).instance(Some(&instance)).build();
    let glyphs = font.outline_glyphs();
    let charmap = font.charmap();
    let text = src.text.replace('\r', "").replace('\t', "    ");
    let lines: Vec<&str> = text.split('\n').collect();
    let count = lines.len().max(1) as f64;
    // Vertical offset of the first baseline (font units, Y up).
    let y0 = match src.v_align {
        VAlign::Top => -ascent,
        VAlign::Center => -(ascent * ASCENT_CENTER).round() + line_height * (count - 1.) / 2.,
        VAlign::Bottom => line_height * (count - 1.),
    };
    let mut missing = Vec::new();
    let mut all: Vec<ExPolygon> = Vec::new();
    for (li, line) in lines.iter().enumerate() {
        for c in line.chars() {
            if !c.is_whitespace() && charmap.map(c).is_none() && !missing.contains(&c) {
                missing.push(c);
            }
        }
        if line.trim().is_empty() {
            continue;
        }
        let mut buffer = harfrust::UnicodeBuffer::new();
        buffer.push_str(line);
        buffer.guess_segment_properties();
        let shaped = shaper.shape(buffer, harfrust::ShapeOptions::new());
        let baseline = y0 - line_height * li as f64;
        let mut pen_x = 0f64;
        let mut line_shapes: Vec<ExPolygon> = Vec::new();
        for (info, pos) in shaped.glyph_infos().iter().zip(shaped.glyph_positions()) {
            let gid = skrifa::GlyphId::new(info.glyph_id);
            let gx = pen_x + f64::from(pos.x_offset);
            let gy = baseline + f64::from(pos.y_offset);
            pen_x += f64::from(pos.x_advance) + f64::from(src.char_gap);
            if info.glyph_id == 0 {
                continue;
            }
            let Some(glyph) = glyphs.get(gid) else {
                continue;
            };
            let mut pen = Pen {
                tol,
                contours: vec![],
                cur: vec![],
            };
            if glyph
                .draw(DrawSettings::unhinted(Size::unscaled(), loc), &mut pen)
                .is_err()
            {
                continue;
            }
            pen.finish();
            let skew = src.skew.unwrap_or(0.);
            let polys: Vec<Polygon> = pen
                .contours
                .iter()
                .map(|c| {
                    c.iter()
                        .map(|p| {
                            let x = p[0] + p[1] * skew;
                            Point::from_mm((gx + x) * mm_per_unit, (gy + p[1]) * mm_per_unit)
                        })
                        .collect()
                })
                .collect();
            // TrueType and CFF outlines use the non-zero winding rule.
            let mut shape = mica_core::emboss::heal(&polys, true);
            if let Some(b) = src.boldness.filter(|b| *b != 0.) {
                shape = clip::offset_ex(&shape, b * mica_core::geometry::SCALE, clip::Join::Miter);
            }
            line_shapes.extend(shape);
        }
        // Horizontal alignment line by line.
        if !line_shapes.is_empty() && src.h_align != HAlign::Left {
            let (lo, hi) = line_shapes
                .iter()
                .flat_map(|e| e.contour.iter())
                .fold((i64::MAX, i64::MIN), |(lo, hi), p| {
                    (lo.min(p.x), hi.max(p.x))
                });
            let dx = match src.h_align {
                HAlign::Center => -(lo + hi) / 2,
                HAlign::Right => -hi,
                HAlign::Left => 0,
            };
            for e in &mut line_shapes {
                for p in e.contour.iter_mut().chain(e.holes.iter_mut().flatten()) {
                    p.x += dx;
                }
            }
        }
        all.extend(line_shapes);
    }
    // Glyphs of one text may touch or overlap (script fonts): one healed union.
    let shape = if all.len() > 1 {
        clip::union_ex(&all)
    } else {
        all
    };
    Ok(TextOutline { shape, missing })
}

#[cfg(test)]
mod tests {
    use super::super::fonts;
    use super::*;

    fn plex() -> (FontFace, std::sync::Arc<[u8]>) {
        let lib = fonts::library();
        let f = fonts::default_face(&lib).clone();
        let d = fonts::load(&f).unwrap();
        (f, d)
    }

    #[test]
    fn letters_have_holes_size_and_alignment() {
        let (face, data) = plex();
        let src = TextSource {
            text: "Ob".into(),
            size_mm: 10.,
            ..Default::default()
        };
        let out = outline(&src, &face, &data).unwrap();
        assert!(out.missing.is_empty());
        assert_eq!(units_per_em(&face, &data), 1000);
        // "O" and "b" each have one counter.
        assert_eq!(out.shape.len(), 2);
        assert!(out.shape.iter().all(|e| e.holes.len() == 1));
        let pts: Vec<_> = out.shape.iter().flat_map(|e| e.contour.iter()).collect();
        let lo = pts.iter().map(|p| p.x).min().unwrap() as f64 * 1e-6;
        let hi = pts.iter().map(|p| p.x).max().unwrap() as f64 * 1e-6;
        let top = pts.iter().map(|p| p.y).max().unwrap() as f64 * 1e-6;
        // Centred horizontally; cap height of Plex ≈ 0.7 em above the baseline.
        assert!((lo + hi).abs() < 0.01, "{lo} {hi}");
        assert!(top > 3.5 && top < 6.5, "{top}");
    }

    #[test]
    fn spacing_lines_and_missing_glyphs() {
        let (face, data) = plex();
        let width = |src: &TextSource| {
            let out = outline(src, &face, &data).unwrap();
            let xs: Vec<i64> = out
                .shape
                .iter()
                .flat_map(|e| e.contour.iter().map(|p| p.x))
                .collect();
            let ys: Vec<i64> = out
                .shape
                .iter()
                .flat_map(|e| e.contour.iter().map(|p| p.y))
                .collect();
            (
                (xs.iter().max().unwrap() - xs.iter().min().unwrap()) as f64 * 1e-6,
                (ys.iter().max().unwrap() - ys.iter().min().unwrap()) as f64 * 1e-6,
            )
        };
        let base = TextSource {
            text: "HH".into(),
            ..Default::default()
        };
        let (w0, h0) = width(&base);
        let (w1, _) = width(&TextSource {
            char_gap: 500, // half an em → +5 mm between the two letters
            ..base.clone()
        });
        assert!((w1 - w0 - 5.).abs() < 0.05, "{w0} {w1}");
        let (_, h2) = width(&TextSource {
            text: "H\nH".into(),
            ..base.clone()
        });
        assert!(h2 > 2. * h0, "{h0} {h2}");
        let out = outline(
            &TextSource {
                text: "A\u{10FFFD}".into(),
                ..base
            },
            &face,
            &data,
        )
        .unwrap();
        assert_eq!(out.missing, vec!['\u{10FFFD}']);
    }
}
