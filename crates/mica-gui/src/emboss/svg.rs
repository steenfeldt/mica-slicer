// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/NSVGUtils.cpp :: create_shape_with_ids
// Source: prusaslicer@b028299c77 src/libslic3r/NSVGUtils.cpp :: linearize_path
// Source: prusaslicer@b028299c77 src/libslic3r/NSVGUtils.cpp :: flatten_cubic_bez
// Source: prusaslicer@b028299c77 src/libslic3r/NSVGUtils.cpp :: need_flattening
// Source: prusaslicer@b028299c77 src/libslic3r/NSVGUtils.cpp :: fill_to_expolygons
// Source: prusaslicer@b028299c77 src/slic3r/GUI/Gizmos/GLGizmoSVG.cpp :: get_tesselation_tolerance
// Changes: usvg (resolved transforms, units, CSS, `use`) replaces nanosvg; each visible filled path is flattened with nanosvg's flatness test at a tolerance in mm (0.1 mm by default, as GLGizmoSVG), Y flipped, healed with its own fill rule (non-zero / even-odd) and unioned; strokes, images and text nodes are reported and skipped; the shape is centred and can be mirrored. Coordinates are Mica's scaled millimetres.
// Upstream attribution (NSVGUtils.cpp, GLGizmoSVG.cpp):
// Copyright (c) Prusa Research 2021 - 2023 Filip Sykala @Jony01, Lukáš Matěna @lukasmatena, Vojtěch Bubník @bubnikv
//
// PrusaSlicer is released under the terms of the AGPLv3 or higher

//! SVG fills → healed outline in the text frame (mm, Y up, centred).

use mica_core::emboss::SvgSource;
use mica_core::geometry::{ExPolygon, Point, Polygon, clip};
use usvg::tiny_skia_path::PathSegment;

/// An SVG's outline and its size before scaling.
#[derive(Debug, Default)]
pub struct SvgOutline {
    pub shape: Vec<ExPolygon>,
    /// Width and height of the filled area in SVG user units.
    pub size_units: [f64; 2],
    pub warnings: Vec<String>,
}

/// nanosvg's flatness test: the control points' distance from the chord against `tol`.
fn need_flattening(tol: f64, p1: [f64; 2], p2: [f64; 2], p3: [f64; 2], p4: [f64; 2]) -> bool {
    let det = |f: [f64; 2], s: [f64; 2]| (f[0] * s[1] - f[1] * s[0]).abs();
    let pd = [p4[0] - p1[0], p4[1] - p1[1]];
    let d2 = det([p2[0] - p4[0], p2[1] - p4[1]], pd);
    let d3 = det([p3[0] - p4[0], p3[1] - p4[1]], pd);
    let d23 = d2 + d3;
    d23 * d23 >= tol * tol * (pd[0] * pd[0] + pd[1] * pd[1])
}

fn flatten_cubic(out: &mut Vec<[f64; 2]>, tol: f64, p: [[f64; 2]; 4], level: u32) {
    let [p1, p2, p3, p4] = p;
    if level == 0 || !need_flattening(tol, p1, p2, p3, p4) {
        out.push(p4);
        return;
    }
    let mid = |a: [f64; 2], b: [f64; 2]| [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5];
    let p12 = mid(p1, p2);
    let p23 = mid(p2, p3);
    let p34 = mid(p3, p4);
    let p123 = mid(p12, p23);
    let p234 = mid(p23, p34);
    let p1234 = mid(p123, p234);
    flatten_cubic(out, tol, [p1, p12, p123, p1234], level - 1);
    flatten_cubic(out, tol, [p1234, p234, p34, p4], level - 1);
}

fn walk(group: &usvg::Group, tol: f64, shapes: &mut Vec<ExPolygon>, warnings: &mut Vec<String>) {
    for node in group.children() {
        match node {
            usvg::Node::Group(g) => walk(g, tol, shapes, warnings),
            usvg::Node::Path(path) => {
                if !path.is_visible() {
                    continue;
                }
                if path.stroke().is_some() {
                    push_once(
                        warnings,
                        "Outlines (strokes) are skipped; only filled shapes are used",
                    );
                }
                let Some(fill) = path.fill() else {
                    continue;
                };
                let t = path.abs_transform();
                let map = |p: usvg::tiny_skia_path::Point| {
                    let (x, y) = (f64::from(p.x), f64::from(p.y));
                    [
                        f64::from(t.sx) * x + f64::from(t.kx) * y + f64::from(t.tx),
                        f64::from(t.ky) * x + f64::from(t.sy) * y + f64::from(t.ty),
                    ]
                };
                let mut rings: Vec<Vec<[f64; 2]>> = Vec::new();
                let mut cur: Vec<[f64; 2]> = Vec::new();
                for seg in path.data().segments() {
                    match seg {
                        PathSegment::MoveTo(p) => {
                            if cur.len() >= 3 {
                                rings.push(std::mem::take(&mut cur));
                            }
                            cur.clear();
                            cur.push(map(p));
                        }
                        PathSegment::LineTo(p) => cur.push(map(p)),
                        PathSegment::QuadTo(c, p) => {
                            let p0 = cur.last().copied().unwrap_or([0., 0.]);
                            let (c, p) = (map(c), map(p));
                            let c1 = [
                                p0[0] + 2. / 3. * (c[0] - p0[0]),
                                p0[1] + 2. / 3. * (c[1] - p0[1]),
                            ];
                            let c2 = [
                                p[0] + 2. / 3. * (c[0] - p[0]),
                                p[1] + 2. / 3. * (c[1] - p[1]),
                            ];
                            flatten_cubic(&mut cur, tol, [p0, c1, c2, p], 10);
                        }
                        PathSegment::CubicTo(c1, c2, p) => {
                            let p0 = cur.last().copied().unwrap_or([0., 0.]);
                            flatten_cubic(&mut cur, tol, [p0, map(c1), map(c2), map(p)], 10);
                        }
                        PathSegment::Close => {
                            if cur.len() >= 3 {
                                rings.push(std::mem::take(&mut cur));
                            }
                            cur.clear();
                        }
                    }
                }
                // Open subpaths of a fill are closed, as SVG fills them.
                if cur.len() >= 3 {
                    rings.push(cur);
                }
                let polys: Vec<Polygon> = rings
                    .iter()
                    .map(|r| r.iter().map(|p| Point::from_mm(p[0], -p[1])).collect())
                    .collect();
                let non_zero = fill.rule() == usvg::FillRule::NonZero;
                shapes.extend(mica_core::emboss::heal(&polys, non_zero));
            }
            usvg::Node::Image(_) => push_once(warnings, "Embedded images are skipped"),
            usvg::Node::Text(_) => {
                push_once(warnings, "Text in the SVG is skipped; convert it to paths")
            }
        }
    }
}

fn push_once(w: &mut Vec<String>, s: &str) {
    if !w.iter().any(|x| x == s) {
        w.push(s.into());
    }
}

/// The healed, centred outline of the SVG's fills in millimetres (`mm_per_unit` applied, mirrored
/// as asked).
pub fn outline(src: &SvgSource) -> Result<SvgOutline, String> {
    let tree = usvg::Tree::from_str(&src.data, &usvg::Options::default())
        .map_err(|e| format!("Not a readable SVG: {e}"))?;
    let scale = src.mm_per_unit.clamp(1e-6, 1e6);
    // Tolerance in user units, so the flattening error is `tolerance_mm` at the final size.
    let tol = (src.tolerance_mm.clamp(0.001, 10.) / scale).max(1e-9);
    let mut shapes = Vec::new();
    let mut warnings = Vec::new();
    walk(tree.root(), tol, &mut shapes, &mut warnings);
    let mut shape = clip::union_ex(&shapes);
    if shape.is_empty() {
        return Err("The SVG has no filled shapes".into());
    }
    let (mut lo, mut hi) = ([i64::MAX; 2], [i64::MIN; 2]);
    for p in shape.iter().flat_map(|e| e.contour.iter()) {
        lo = [lo[0].min(p.x), lo[1].min(p.y)];
        hi = [hi[0].max(p.x), hi[1].max(p.y)];
    }
    let size_units = [
        (hi[0] - lo[0]) as f64 / mica_core::geometry::SCALE,
        (hi[1] - lo[1]) as f64 / mica_core::geometry::SCALE,
    ];
    let c = [(lo[0] + hi[0]) as f64 / 2., (lo[1] + hi[1]) as f64 / 2.];
    let [mx, my] = src.mirror;
    for e in &mut shape {
        for p in e.contour.iter_mut().chain(e.holes.iter_mut().flatten()) {
            let x = (p.x as f64 - c[0]) * scale * if mx { -1. } else { 1. };
            let y = (p.y as f64 - c[1]) * scale * if my { -1. } else { 1. };
            *p = Point::new(x.round() as i64, y.round() as i64);
        }
        if mx != my {
            e.contour.reverse();
            for h in &mut e.holes {
                h.reverse();
            }
        }
    }
    Ok(SvgOutline {
        shape,
        size_units,
        warnings,
    })
}

/// The filled area's size in user units (cached per file: the card asks every frame).
pub fn size_units(src: &SvgSource) -> [f64; 2] {
    use std::sync::{Mutex, OnceLock};
    type Cache = Mutex<Vec<(std::sync::Arc<str>, [f64; 2])>>;
    static CACHE: OnceLock<Cache> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Ok(c) = cache.lock()
        && let Some((_, s)) = c.iter().find(|(d, _)| std::sync::Arc::ptr_eq(d, &src.data))
    {
        return *s;
    }
    let size = outline(src).map(|o| o.size_units).unwrap_or([1., 1.]);
    if let Ok(mut c) = cache.lock() {
        c.insert(0, (src.data.clone(), size));
        c.truncate(8);
    }
    size
}

/// Millimetres per SVG user unit for the file's own size (CSS: 96 user units per inch).
pub const MM_PER_PX: f64 = 25.4 / 96.;

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn src(data: &str) -> SvgSource {
        SvgSource {
            path: String::new(),
            path_in_3mf: "3D/t.svg".into(),
            data: Arc::from(data),
            mm_per_unit: 1.,
            mirror: [false, false],
            tolerance_mm: 0.1,
        }
    }

    #[test]
    fn fills_become_centred_polygons_with_their_fill_rule() {
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="20">
            <rect x="0" y="0" width="20" height="10"/>
            <path fill-rule="evenodd" d="M30 0 h10 v10 h-10 z M32 2 h6 v6 h-6 z"/>
            <circle cx="10" cy="15" r="4"/>
            <line x1="0" y1="0" x2="5" y2="5" stroke="black"/>
        </svg>"#;
        let out = outline(&src(svg)).unwrap();
        assert_eq!(out.shape.len(), 3);
        assert_eq!(out.shape.iter().map(|e| e.holes.len()).sum::<usize>(), 1);
        let area: f64 = out.shape.iter().map(|e| e.area_mm2()).sum();
        let circle = std::f64::consts::PI * 16.;
        assert!((area - (200. + 64. + circle)).abs() < 1.0, "{area}");
        assert_eq!(out.size_units, [40., 19.]);
        assert!(!out.warnings.is_empty());
        // Centred on the origin.
        let xs: Vec<i64> = out
            .shape
            .iter()
            .flat_map(|e| e.contour.iter().map(|p| p.x))
            .collect();
        assert!((xs.iter().max().unwrap() + xs.iter().min().unwrap()).abs() < 10);
    }

    #[test]
    fn tolerance_scale_and_mirror() {
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg"><circle cx="0" cy="0" r="10"/><rect x="20" y="-2" width="4" height="4"/></svg>"#;
        let mut s = src(svg);
        let coarse = outline(&s).unwrap();
        s.tolerance_mm = 0.005;
        let fine = outline(&s).unwrap();
        let n = |o: &SvgOutline| o.shape.iter().map(|e| e.contour.len()).sum::<usize>();
        assert!(n(&fine) > n(&coarse) * 2, "{} {}", n(&fine), n(&coarse));
        s.mm_per_unit = 0.5;
        let half = outline(&s).unwrap();
        let area = |o: &SvgOutline| o.shape.iter().map(|e| e.area_mm2()).sum::<f64>();
        assert!((area(&half) * 4. - area(&fine)).abs() < 0.5);
        s.mirror = [true, false];
        let mirrored = outline(&s).unwrap();
        assert!((area(&mirrored) - area(&half)).abs() < 1e-6);
        // The square sits right of the circle; mirrored, it is on the left.
        let square = |o: &SvgOutline| {
            o.shape
                .iter()
                .find(|e| e.contour.len() == 4)
                .unwrap()
                .contour[0]
                .x
        };
        assert!(square(&half) > 0 && square(&mirrored) < 0);
    }
}
