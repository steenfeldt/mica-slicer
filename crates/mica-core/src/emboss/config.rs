// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/TextConfiguration.hpp :: FontProp
// Source: prusaslicer@b028299c77 src/libslic3r/EmbossShape.hpp :: EmbossShape
// Source: prusaslicer@b028299c77 src/libslic3r/Format/3mf.cpp :: TextConfigurationSerialization::to_xml
// Source: prusaslicer@b028299c77 src/libslic3r/Format/3mf.cpp :: read_emboss_shape
// Source: orca@8500fcdcca src/libslic3r/Format/bbs_3mf.cpp :: TextConfigurationSerialization::to_xml
// Changes: One Rust struct for the text style (FontProp + EmbossStyle) and one for the SVG source, with bold / italic flags for Mica's face picker; serialised as flat `slic3rpe:text:<attr>` / `slic3rpe:shape:<attr>` part metadata that the 3MF writer turns back into PrusaSlicer's (and Orca's identical) `<slic3rpe:text …/>` and `<slic3rpe:shape …/>` elements. Font descriptors are file names only (PrusaSlicer's privacy rule for `file_name`); Mica-only extras carry a `mica_` prefix.
// Upstream attribution (TextConfiguration.hpp, EmbossShape.hpp, 3mf.cpp):
// Copyright (c) Prusa Research 2021 - 2023 Filip Sykala @Jony01, Oleksandra Iushchenko @YuSanka, Lukáš Matěna @lukasmatena, Vojtěch Bubník @bubnikv, Enrico Turri @enricoturri1966, Lukáš Hejl @hejllukas, Tomáš Mészáros @tamasmeszaros, David Kocík @kocikdav
//
// PrusaSlicer is released under the terms of the AGPLv3 or higher

//! The editable source of an embossed volume and its 3MF metadata form.

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::threemf::Transform;

/// Part metadata keys `slic3rpe:text:<attribute>` hold the `<slic3rpe:text>` element.
pub const TEXT_PREFIX: &str = "slic3rpe:text:";
/// Part metadata keys `slic3rpe:shape:<attribute>` hold the `<slic3rpe:shape>` element.
pub const SHAPE_PREFIX: &str = "slic3rpe:shape:";
/// The SVG file's text, kept with the part so the archive carries it at `filepath3mf`.
pub const SVG_DATA_KEY: &str = "mica_svg_data";

/// PrusaSlicer's `SHAPE_SCALE`: glyph shapes are font units / 1000 in its integer space.
const SHAPE_SCALE: f64 = 0.001;
/// PrusaSlicer's `SCALING_FACTOR` (mm per integer coordinate unit).
const SCALING_FACTOR: f64 = 1e-6;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum HAlign {
    Left,
    #[default]
    Center,
    Right,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum VAlign {
    Top,
    #[default]
    Center,
    Bottom,
}

/// Text and its font style (PrusaSlicer's `TextConfiguration`: `EmbossStyle` + `FontProp`).
#[derive(Clone, Debug, PartialEq)]
pub struct TextSource {
    pub text: String,
    /// Font family (`Helvetica Neue`) and face (`Bold Italic`) names, as listed by the picker.
    pub family: String,
    pub face: String,
    /// The font file's name without directories (PrusaSlicer stores only the name).
    pub file_name: String,
    /// Face index inside a `.ttc` collection.
    pub collection: u32,
    /// Em size in mm (PrusaSlicer's `size_in_mm`, written as `line_height`).
    pub size_mm: f64,
    /// Extra advance per letter and per line, in font units (PrusaSlicer's `char_gap` / `line_gap`).
    pub char_gap: i32,
    pub line_gap: i32,
    /// Bold / italic as picked: the family's own faces, or a variable font's weight axis.
    pub bold: bool,
    pub italic: bool,
    /// Synthetic outline offset (mm) and slant (x per y) for fonts without such faces.
    pub boldness: Option<f64>,
    pub skew: Option<f64>,
    pub h_align: HAlign,
    pub v_align: VAlign,
    /// The font's units per em at the time of writing (to report `scale`); 1000 when unknown.
    pub units_per_em: u16,
}
impl Default for TextSource {
    fn default() -> Self {
        Self {
            text: "Mica".into(),
            family: String::new(),
            face: String::new(),
            file_name: String::new(),
            collection: 0,
            size_mm: 10.,
            char_gap: 0,
            line_gap: 0,
            bold: false,
            italic: false,
            boldness: None,
            skew: None,
            h_align: HAlign::Center,
            v_align: VAlign::Center,
            units_per_em: 1000,
        }
    }
}

/// An SVG file as the shape's source.
#[derive(Clone, Debug, PartialEq)]
pub struct SvgSource {
    /// Where it was imported from (for "reload"); may be empty.
    pub path: String,
    /// Where the project stores it (`3D/<name>.svg`).
    pub path_in_3mf: String,
    /// The file's text.
    pub data: Arc<str>,
    /// Millimetres per SVG user unit (the size slider scales this).
    pub mm_per_unit: f64,
    /// Mirror the shape across its vertical (X) / horizontal (Y) axis.
    pub mirror: [bool; 2],
    /// Curve flattening tolerance in mm (PrusaSlicer: 0.1 mm).
    pub tolerance_mm: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum EmbossSource {
    Text(TextSource),
    Svg(SvgSource),
}

/// Everything needed to rebuild an embossed volume.
#[derive(Clone, Debug, PartialEq)]
pub struct EmbossConfig {
    pub source: EmbossSource,
    /// Emboss / engrave depth in mm.
    pub depth: f64,
    /// Follow a curved surface instead of a flat prism.
    pub use_surface: bool,
}

impl EmbossConfig {
    pub fn text(&self) -> Option<&TextSource> {
        match &self.source {
            EmbossSource::Text(t) => Some(t),
            EmbossSource::Svg(_) => None,
        }
    }
    pub fn svg(&self) -> Option<&SvgSource> {
        match &self.source {
            EmbossSource::Svg(s) => Some(s),
            EmbossSource::Text(_) => None,
        }
    }

    /// The part metadata for a 3MF: `<slic3rpe:shape>` (always) and `<slic3rpe:text>` (text)
    /// attributes; `frame` is the text frame in the part's coordinates (PrusaSlicer's
    /// `fix_3mf_tr`: stored volume transform × fix = the frame before baking).
    pub fn to_metadata(&self, frame: Transform) -> BTreeMap<String, String> {
        let mut m = BTreeMap::new();
        let mut shape = |k: &str, v: String| {
            m.insert(format!("{SHAPE_PREFIX}{k}"), v);
        };
        match &self.source {
            EmbossSource::Text(t) => {
                shape(
                    "scale",
                    num(t.size_mm / f64::from(t.units_per_em.max(1)) * SHAPE_SCALE),
                );
            }
            EmbossSource::Svg(s) => {
                if !s.path.is_empty() {
                    shape("filepath", file_name(&s.path).to_string());
                }
                shape("filepath3mf", s.path_in_3mf.clone());
                shape("scale", num(s.mm_per_unit * SCALING_FACTOR));
                shape("mica_mm_per_unit", num(s.mm_per_unit));
                shape("mica_tolerance", num(s.tolerance_mm));
                let mirror = match s.mirror {
                    [true, true] => "xy",
                    [true, false] => "x",
                    [false, true] => "y",
                    [false, false] => "",
                };
                if !mirror.is_empty() {
                    shape("mica_mirror", mirror.into());
                }
            }
        }
        shape("depth", num(self.depth));
        if self.use_surface {
            shape("use_surface", "1".into());
        }
        shape("transform", format_transform(frame));
        if let EmbossSource::Svg(s) = &self.source {
            m.insert(SVG_DATA_KEY.into(), s.data.to_string());
        }
        if let EmbossSource::Text(t) = &self.source {
            let mut text = |k: &str, v: String| {
                m.insert(format!("{TEXT_PREFIX}{k}"), v);
            };
            text("text", t.text.clone());
            text(
                "style_name",
                format!("{} {}", t.family, t.face).trim().to_string(),
            );
            text("font_descriptor", file_name(&t.file_name).to_string());
            text("font_descriptor_type", "file_name".into());
            if t.char_gap != 0 {
                text("char_gap", t.char_gap.to_string());
            }
            if t.line_gap != 0 {
                text("line_gap", t.line_gap.to_string());
            }
            text("line_height", num(t.size_mm));
            if let Some(b) = t.boldness {
                text("boldness", num(b));
            }
            if let Some(s) = t.skew {
                text("skew", num(s));
            }
            text(
                "horizontal",
                match t.h_align {
                    HAlign::Left => "left",
                    HAlign::Center => "center",
                    HAlign::Right => "right",
                }
                .into(),
            );
            text(
                "vertical",
                match t.v_align {
                    VAlign::Top => "top",
                    VAlign::Center => "middle",
                    VAlign::Bottom => "bottom",
                }
                .into(),
            );
            if t.collection > 0 {
                text("collection", t.collection.to_string());
            }
            if !t.family.is_empty() {
                text("family", t.family.clone());
            }
            if !t.face.is_empty() {
                text("face_name", t.face.clone());
            }
            text("style", if t.italic { "italic" } else { "normal" }.into());
            text("weight", if t.bold { "bold" } else { "normal" }.into());
        }
        m
    }

    /// The configuration and stored frame from part metadata (`None` when the part carries no
    /// emboss source, or an SVG shape whose file is missing).
    pub fn from_metadata(m: &BTreeMap<String, String>) -> Option<(Self, Option<Transform>)> {
        let shape = |k: &str| m.get(&format!("{SHAPE_PREFIX}{k}")).map(String::as_str);
        let text = |k: &str| m.get(&format!("{TEXT_PREFIX}{k}")).map(String::as_str);
        let f = |v: Option<&str>| {
            v.and_then(|s| s.trim().parse::<f64>().ok())
                .filter(|v| v.is_finite())
        };
        let i = |v: Option<&str>| v.and_then(|s| s.trim().parse::<i64>().ok());
        let has_text = text("text").is_some();
        let has_shape = m.keys().any(|k| k.starts_with(SHAPE_PREFIX));
        if !has_text && !has_shape {
            return None;
        }
        let depth = f(shape("depth")).filter(|d| *d > 0.).unwrap_or(10.);
        let use_surface = i(shape("use_surface")) == Some(1);
        let frame = shape("transform").and_then(parse_transform);
        let source = if has_text {
            let size_mm = f(text("line_height")).filter(|s| *s > 0.).unwrap_or(10.);
            let scale = f(shape("scale")).filter(|s| *s > 0.);
            let units_per_em = scale
                .map(|s| (size_mm * SHAPE_SCALE / s).round())
                .filter(|u| (16. ..=16384.).contains(u))
                .map_or(1000, |u| u as u16);
            let style = text("style").unwrap_or("");
            let weight = text("weight").unwrap_or("");
            EmbossSource::Text(TextSource {
                text: text("text").unwrap_or("").to_string(),
                family: text("family").unwrap_or("").to_string(),
                face: text("face_name").unwrap_or("").to_string(),
                file_name: file_name(text("font_descriptor").unwrap_or("")).to_string(),
                collection: i(text("collection")).unwrap_or(0).clamp(0, 1 << 16) as u32,
                size_mm,
                char_gap: i(text("char_gap")).unwrap_or(0).clamp(-100_000, 100_000) as i32,
                line_gap: i(text("line_gap")).unwrap_or(0).clamp(-100_000, 100_000) as i32,
                bold: weight.eq_ignore_ascii_case("bold")
                    || weight.parse::<u32>().is_ok_and(|w| w >= 600),
                italic: style.eq_ignore_ascii_case("italic")
                    || style.eq_ignore_ascii_case("oblique")
                    || style.eq_ignore_ascii_case("slant"),
                boldness: f(text("boldness")).filter(|b| *b != 0.),
                skew: f(text("skew")).filter(|s| *s != 0.),
                h_align: match text("horizontal") {
                    Some("left") | Some("0") => HAlign::Left,
                    Some("right") | Some("2") => HAlign::Right,
                    _ => HAlign::Center,
                },
                v_align: match text("vertical") {
                    Some("top") | Some("0") => VAlign::Top,
                    Some("bottom") | Some("2") => VAlign::Bottom,
                    _ => VAlign::Center,
                },
                units_per_em,
            })
        } else {
            let data = m.get(SVG_DATA_KEY)?;
            let mm_per_unit = f(shape("mica_mm_per_unit"))
                .or_else(|| f(shape("scale")).map(|s| s / SCALING_FACTOR))
                .filter(|s| *s > 0.)
                .unwrap_or(1.);
            let mirror = shape("mica_mirror").unwrap_or("");
            EmbossSource::Svg(SvgSource {
                path: shape("filepath").unwrap_or("").to_string(),
                path_in_3mf: shape("filepath3mf").unwrap_or("").to_string(),
                data: Arc::from(data.as_str()),
                mm_per_unit,
                mirror: [mirror.contains('x'), mirror.contains('y')],
                tolerance_mm: f(shape("mica_tolerance"))
                    .filter(|t| *t > 0.)
                    .unwrap_or(0.1),
            })
        };
        Some((
            Self {
                source,
                depth,
                use_surface,
            },
            frame,
        ))
    }
}

/// Whether a part metadata key belongs to the emboss source (not a slicer setting).
pub fn is_emboss_key(k: &str) -> bool {
    k.starts_with(TEXT_PREFIX) || k.starts_with(SHAPE_PREFIX) || k == SVG_DATA_KEY
}

fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// Shortest round-trip decimal text (Rust never writes an exponent).
fn num(v: f64) -> String {
    if v == 0. { "0".into() } else { format!("{v}") }
}

/// 3MF matrix text: the 4 × 3 column-major values (`m00 m10 m20 m01 … m23`).
pub fn format_transform(t: Transform) -> String {
    let mut out = Vec::with_capacity(12);
    for c in 0..4 {
        for r in 0..3 {
            out.push(format!("{}", t.0[r][c]));
        }
    }
    out.join(" ")
}

pub fn parse_transform(s: &str) -> Option<Transform> {
    let v: Vec<f64> = s
        .split_whitespace()
        .map(|x| x.parse::<f64>().ok().filter(|v| v.is_finite()))
        .collect::<Option<_>>()?;
    // PrusaSlicer writes 12 values; a full 4 × 4 row-major form (16) is accepted too.
    let mut t = Transform::IDENTITY;
    match v.len() {
        12 => {
            for c in 0..4 {
                for r in 0..3 {
                    t.0[r][c] = v[c * 3 + r];
                }
            }
        }
        16 => {
            for r in 0..3 {
                for c in 0..4 {
                    t.0[r][c] = v[r * 4 + c];
                }
            }
        }
        _ => return None,
    }
    (t.determinant().abs() > 1e-12).then_some(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_round_trips_through_metadata() {
        let cfg = EmbossConfig {
            source: EmbossSource::Text(TextSource {
                text: "Hello \"Mica\"\nline two".into(),
                family: "IBM Plex Sans".into(),
                face: "Medium".into(),
                file_name: "/Library/Fonts/IBMPlexSans-Medium.ttf".into(),
                char_gap: 50,
                line_gap: -20,
                bold: true,
                italic: false,
                skew: Some(0.2),
                h_align: HAlign::Left,
                v_align: VAlign::Bottom,
                ..Default::default()
            }),
            depth: 1.5,
            use_surface: true,
        };
        let frame = crate::emboss::transform_onto_surface([1., 2., 3.], [0., -1., 0.], 0.9);
        let m = cfg.to_metadata(frame);
        assert_eq!(m["slic3rpe:text:font_descriptor"], "IBMPlexSans-Medium.ttf");
        assert_eq!(m["slic3rpe:shape:use_surface"], "1");
        let (back, t) = EmbossConfig::from_metadata(&m).unwrap();
        let mut expected = cfg.clone();
        if let EmbossSource::Text(t) = &mut expected.source {
            t.file_name = "IBMPlexSans-Medium.ttf".into();
        }
        assert_eq!(back, expected);
        let t = t.unwrap();
        for r in 0..3 {
            for c in 0..4 {
                assert!((t.0[r][c] - frame.0[r][c]).abs() < 1e-12);
            }
        }
    }

    #[test]
    fn svg_round_trips_and_needs_its_data() {
        let cfg = EmbossConfig {
            source: EmbossSource::Svg(SvgSource {
                path: "/tmp/logo.svg".into(),
                path_in_3mf: "3D/logo.svg".into(),
                data: Arc::from("<svg/>"),
                mm_per_unit: 0.264583,
                mirror: [true, false],
                tolerance_mm: 0.1,
            }),
            depth: 2.,
            use_surface: false,
        };
        let mut m = cfg.to_metadata(Transform::IDENTITY);
        let (back, _) = EmbossConfig::from_metadata(&m).unwrap();
        let mut expected = cfg.clone();
        if let EmbossSource::Svg(s) = &mut expected.source {
            s.path = "logo.svg".into();
        }
        assert_eq!(back, expected);
        m.remove(SVG_DATA_KEY);
        assert!(EmbossConfig::from_metadata(&m).is_none());
    }
}
