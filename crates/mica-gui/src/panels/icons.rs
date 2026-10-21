// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: orca@8500fcdcca resources/images/param_gyroid.svg :: fill pattern preview icons
// Source: orca@8500fcdcca resources/images/param_crosshatch.svg :: fill pattern preview icons
// Source: orca@8500fcdcca resources/images/param_tpmsd.svg :: fill pattern preview icons
// Source: prusaslicer@b028299c77 src/libslic3r/Fill/FillRectilinear.cpp :: FillStars::fill_surface
// Changes: Only the look of each fill pattern is taken (Orca's per-pattern preview icons and
// the line directions of PrusaSlicer's fills); every icon is drawn from scratch with egui
// shapes on a unit square, in the theme's colours, at 14–16 px.

//! Tiny vector icons for enum choices where a picture says more than a word: seam position,
//! infill and support patterns, brim type, support style. Drawn with the egui painter so they
//! follow the theme and need no image assets. Every fill pattern has its own picture, so the
//! sparse, top, bottom, support and interface dropdowns can be scanned by shape.

use eframe::egui::{self, Color32, Pos2, Rect, Shape, Stroke, pos2};
use std::f32::consts::{FRAC_PI_2, TAU};

/// Whether `key` has icons for its values.
pub fn has_icons(key: &str) -> bool {
    matches!(
        key,
        "seam_position"
            | "fill_pattern"
            | "top_fill_pattern"
            | "bottom_fill_pattern"
            | "support_material_pattern"
            | "support_material_interface_pattern"
            | "brim_type"
            | "support_material_style"
    )
}

/// Every fill pattern value with a picture: PrusaSlicer's names (`fill::Pattern`), Orca's
/// (sparse, surface and support vocabularies) and the support / interface choices.
pub const FILL_PATTERNS: &[&str] = &[
    "rectilinear",
    "alignedrectilinear",
    "monotonic",
    "monotoniclines",
    "monotonicline",
    "line",
    "zigzag",
    "zig-zag",
    "crosszag",
    "lockedzag",
    "grid",
    "rectilinear-grid",
    "rectilinear_interlaced",
    "triangles",
    "stars",
    "tri-hexagon",
    "cubic",
    "adaptivecubic",
    "supportcubic",
    "quartercubic",
    "honeycomb",
    "3dhoneycomb",
    "lateral-honeycomb",
    "lateral-lattice",
    "crosshatch",
    "tpmsd",
    "tpmsfk",
    "gyroid",
    "concentric",
    "lightning",
    "hilbertcurve",
    "archimedeanchords",
    "octagramspiral",
    "auto",
    "default",
    "hollow",
];

/// Paints the icon into an existing `rect` (no space is allocated).
pub fn paint(ui: &egui::Ui, rect: Rect, key: &str, value: &str) -> bool {
    if !has_icons(key) {
        return false;
    }
    let painter = ui.painter_at(rect);
    let fg = ui.visuals().text_color();
    let weak = ui.visuals().weak_text_color();
    let accent = ui.visuals().hyperlink_color;
    let r = rect.shrink(1.5);
    let thin = Stroke::new(1., weak);
    let bold = Stroke::new(1.4, fg);
    let line = |a: Pos2, b: Pos2, s: Stroke| painter.line_segment([a, b], s);
    let frame = |s: Stroke| painter.rect_stroke(r, 1., s, egui::StrokeKind::Inside);
    let dot = |p: Pos2| painter.circle_filled(p, 2., accent);
    let lerp = |t: f32, u: f32| pos2(r.left() + r.width() * t, r.top() + r.height() * u);
    // Unknown values (Orca-only vocabulary) get no picture and no frame.
    let known = match key {
        "seam_position" => matches!(value, "random" | "nearest" | "aligned" | "rear"),
        "brim_type" => matches!(
            value,
            "no_brim" | "outer_only" | "inner_only" | "outer_and_inner"
        ),
        "support_material_style" => matches!(value, "grid" | "snug" | "organic" | "tree"),
        _ => FILL_PATTERNS.contains(&value),
    };
    if !known {
        return false;
    }
    match key {
        "seam_position" => {
            frame(bold);
            match value {
                "random" => {
                    dot(lerp(0.2, 0.0));
                    dot(lerp(1.0, 0.6));
                    dot(lerp(0.55, 1.0));
                    dot(lerp(0.0, 0.35));
                }
                "nearest" => {
                    dot(lerp(1.0, 0.0));
                    dot(lerp(0.0, 1.0));
                }
                "aligned" => {
                    line(lerp(1.0, 0.0), lerp(1.0, 1.0), Stroke::new(2.5, accent));
                }
                "rear" => {
                    dot(lerp(0.5, 0.0));
                }
                _ => return false,
            }
        }
        "brim_type" => {
            let inner = r.shrink(r.width() * 0.3);
            painter.rect_stroke(inner, 0., bold, egui::StrokeKind::Inside);
            let ring = Stroke::new(1.5, accent);
            match value {
                "no_brim" => {}
                "outer_only" => {
                    painter.rect_stroke(
                        inner.expand(r.width() * 0.18),
                        0.,
                        ring,
                        egui::StrokeKind::Inside,
                    );
                }
                "inner_only" => {
                    painter.rect_stroke(
                        inner.shrink(r.width() * 0.12),
                        0.,
                        ring,
                        egui::StrokeKind::Inside,
                    );
                }
                "outer_and_inner" => {
                    painter.rect_stroke(
                        inner.expand(r.width() * 0.18),
                        0.,
                        ring,
                        egui::StrokeKind::Inside,
                    );
                    painter.rect_stroke(
                        inner.shrink(r.width() * 0.12),
                        0.,
                        ring,
                        egui::StrokeKind::Inside,
                    );
                }
                _ => return false,
            }
        }
        "support_material_style" => {
            // An overhang on top, supports below it.
            line(lerp(0.0, 0.2), lerp(1.0, 0.2), bold);
            match value {
                "grid" => {
                    for t in [0.2, 0.5, 0.8] {
                        line(lerp(t, 0.2), lerp(t, 1.0), thin);
                    }
                    line(lerp(0.1, 0.6), lerp(0.9, 0.6), thin);
                }
                "snug" => {
                    for t in [0.35, 0.5, 0.65] {
                        line(lerp(t, 0.2), lerp(t, 1.0), thin);
                    }
                }
                "organic" | "tree" => {
                    line(lerp(0.5, 1.0), lerp(0.5, 0.6), Stroke::new(1.6, accent));
                    line(lerp(0.5, 0.6), lerp(0.2, 0.2), Stroke::new(1.2, accent));
                    line(lerp(0.5, 0.6), lerp(0.8, 0.2), Stroke::new(1.2, accent));
                    line(lerp(0.5, 0.75), lerp(0.5, 0.2), Stroke::new(1.0, accent));
                }
                _ => return false,
            }
        }
        _ => {
            frame(thin);
            let pad = Pad {
                p: painter.with_clip_rect(r),
                r,
                fg,
                weak,
                accent,
            };
            return pad.fill(value);
        }
    }
    true
}

/// A unit square (`r`, y down) to draw one fill pattern in, clipped to the square.
struct Pad {
    p: egui::Painter,
    r: Rect,
    fg: Color32,
    weak: Color32,
    accent: Color32,
}

type Pt = (f32, f32);

impl Pad {
    fn at(&self, (x, y): Pt) -> Pos2 {
        pos2(
            self.r.left() + self.r.width() * x,
            self.r.top() + self.r.height() * y,
        )
    }
    fn bold(&self) -> Stroke {
        Stroke::new(1.3, self.fg)
    }
    /// For patterns with many lines.
    fn fine(&self) -> Stroke {
        Stroke::new(1., self.fg)
    }
    fn thin(&self) -> Stroke {
        Stroke::new(1., self.weak)
    }
    fn acc(&self, w: f32) -> Stroke {
        Stroke::new(w, self.accent)
    }
    fn line(&self, pts: &[Pt], s: Stroke) {
        let pts = pts.iter().map(|&q| self.at(q)).collect();
        self.p.add(Shape::line(pts, s));
    }
    fn closed(&self, pts: &[Pt], s: Stroke) {
        let pts = pts.iter().map(|&q| self.at(q)).collect();
        self.p.add(Shape::closed_line(pts, s));
    }
    /// Parallel lines across the whole square in direction `deg` (0 = across, 90 = down),
    /// `gap` apart, the set moved `shift` gaps off the centre.
    fn hatch(&self, deg: f32, gap: f32, shift: f32, s: Stroke) {
        let a = deg.to_radians();
        let (dx, dy) = (a.cos(), a.sin());
        let k = (0.75 / gap).ceil() as i32 + 1;
        for i in -k..=k {
            let o = (i as f32 + shift) * gap;
            let (cx, cy) = (0.5 - dy * o, 0.5 + dx * o);
            self.line(&[(cx - dx, cy - dy), (cx + dx, cy + dy)], s);
        }
    }
    /// A regular `n`-gon around `c`, the first corner at `start` degrees.
    fn ngon(c: Pt, rad: Pt, n: usize, start: f32) -> Vec<Pt> {
        (0..n)
            .map(|i| {
                let a = (start + i as f32 * 360. / n as f32).to_radians();
                (c.0 + rad.0 * a.cos(), c.1 + rad.1 * a.sin())
            })
            .collect()
    }
    /// An isometric cube seen from above: a hexagon and the three edges meeting in front.
    /// Returns its corners (top first, clockwise) for further lines.
    fn cube(&self, c: Pt, rad: f32, s: Stroke) -> Vec<Pt> {
        let v = Self::ngon(c, (rad, rad), 6, -90.);
        self.closed(&v, s);
        for &k in &[1, 3, 5] {
            self.line(&[c, v[k]], s);
        }
        v
    }

    fn fill(&self, v: &str) -> bool {
        let bold = self.bold();
        let fine = self.fine();
        match v {
            // Straight lines: rectilinear runs diagonally, line straight across; aligned keeps
            // the same direction on every layer (the bar), monotonic is one connected
            // back-and-forth path, monotonic lines are separate lines laid in one order.
            "rectilinear" => self.hatch(45., 0.3, 0., bold),
            "alignedrectilinear" => {
                let below = Rect::from_min_max(self.at((0., 0.26)), self.r.max);
                let p = self.p.with_clip_rect(below);
                Pad { p, ..*self }.hatch(45., 0.3, 0., bold);
                self.line(&[(0.1, 0.12), (0.9, 0.12)], self.acc(1.6));
            }
            "monotonic" => {
                let u = |x: f32, y: f32| (0.1 + 0.8 * x, 0.1 + 0.8 * y);
                let ends = |c: f32| {
                    if c <= 1. {
                        (u(0., c), u(c, 0.))
                    } else {
                        (u(c - 1., 1.), u(1., c - 1.))
                    }
                };
                let mut path = Vec::new();
                for (i, c) in [0.3, 0.75, 1.25, 1.7].into_iter().enumerate() {
                    let (a, b) = ends(c);
                    if i % 2 == 0 {
                        path.extend([a, b]);
                    } else {
                        path.extend([b, a]);
                    }
                }
                self.line(&path, bold);
            }
            "monotoniclines" | "monotonicline" => {
                self.hatch(-45., 0.3, 0., fine);
                let s = self.acc(1.5);
                self.line(&[(0.2, 0.2), (0.78, 0.78)], s);
                self.line(&[(0.48, 0.8), (0.8, 0.8), (0.8, 0.48)], s);
            }
            "line" => self.hatch(0., 0.33, 0., bold),
            // Zigzags: rows of teeth; crosszag crosses a row with a column; locked zigzags
            // meet tooth to tooth and lock at the points.
            "zigzag" | "zig-zag" => {
                for (lo, hi) in [(0.1, 0.4), (0.6, 0.9)] {
                    let row: Vec<Pt> = (0..=4)
                        .map(|i| (i as f32 * 0.25, if i % 2 == 0 { lo } else { hi }))
                        .collect();
                    self.line(&row, bold);
                }
            }
            "crosszag" => {
                let row: Vec<Pt> = (0..=4)
                    .map(|i| (i as f32 * 0.25, if i % 2 == 0 { 0.3 } else { 0.7 }))
                    .collect();
                self.line(&row, bold);
                let col: Vec<Pt> = row.iter().map(|&(x, y)| (y, x)).collect();
                self.line(&col, self.acc(1.2));
            }
            "lockedzag" => {
                let a: Vec<Pt> = (0..=4)
                    .map(|i| (i as f32 * 0.25, if i % 2 == 0 { 0.18 } else { 0.5 }))
                    .collect();
                let b: Vec<Pt> = a.iter().map(|&(x, y)| (x, 1. - y)).collect();
                self.line(&a, bold);
                self.line(&b, bold);
                for x in [0.25, 0.75] {
                    self.p.circle_filled(self.at((x, 0.5)), 1.6, self.accent);
                }
            }
            // Crossing lines: grid at 45°, the support grid square to the edges, interlaced
            // interface layers in two colours.
            "grid" => {
                self.hatch(45., 0.36, 0., fine);
                self.hatch(-45., 0.36, 0., fine);
            }
            "rectilinear-grid" => {
                self.hatch(0., 0.33, 0., fine);
                self.hatch(90., 0.33, 0., fine);
            }
            "rectilinear_interlaced" => {
                self.hatch(0., 0.3, 0.5, bold);
                self.hatch(90., 0.3, 0.5, self.acc(1.));
            }
            // Three directions: through common points (triangles), with every set moved half
            // a gap (hexagons between the triangles), and the six-pointed star it makes.
            "triangles" => {
                for d in [0., 60., 120.] {
                    self.hatch(d, 0.4, 0., fine);
                }
            }
            "tri-hexagon" => {
                for d in [0., 60., 120.] {
                    self.hatch(d, 0.52, 0.5, fine);
                }
            }
            "stars" => {
                self.closed(&Self::ngon((0.5, 0.5), (0.47, 0.47), 3, -90.), bold);
                self.closed(&Self::ngon((0.5, 0.5), (0.47, 0.47), 3, 90.), bold);
            }
            // Cubes: one cube; adaptive shrinks them toward the walls; support cubic builds
            // up under a top surface (the bar); quarter cubic splits every face.
            "cubic" => {
                self.cube((0.5, 0.5), 0.46, fine);
            }
            "adaptivecubic" => {
                self.cube((0.37, 0.39), 0.31, fine);
                self.cube((0.79, 0.81), 0.17, self.thin());
                self.cube((0.24, 0.87), 0.12, self.thin());
            }
            "supportcubic" => {
                self.cube((0.5, 0.64), 0.32, fine);
                self.line(&[(0.08, 0.12), (0.92, 0.12)], self.acc(1.6));
                self.line(&[(0.5, 0.32), (0.5, 0.12)], self.thin());
            }
            "quartercubic" => {
                let c = (0.5, 0.5);
                let v = self.cube(c, 0.46, fine);
                let mid = |a: Pt, b: Pt| ((a.0 + b.0) / 2., (a.1 + b.1) / 2.);
                for face in [
                    [c, v[5], v[0], v[1]],
                    [c, v[1], v[2], v[3]],
                    [c, v[3], v[4], v[5]],
                ] {
                    let [a, b, cc, d] = face;
                    self.line(&[mid(a, b), mid(d, cc)], self.thin());
                    self.line(&[mid(b, cc), mid(a, d)], self.thin());
                }
            }
            // Cells: regular hexagons; 3D honeycomb's octagons and squares; lateral
            // honeycomb's wide, flat-topped cells.
            "honeycomb" => {
                let rad = 0.27;
                let w = 3f32.sqrt() * rad;
                for j in -2..=2 {
                    for i in -2..=2 {
                        let c = (
                            0.5 + w * (i as f32 + j as f32 / 2.),
                            0.5 + 1.5 * rad * j as f32,
                        );
                        self.closed(&Self::ngon(c, (rad, rad), 6, -90.), fine);
                    }
                }
            }
            "3dhoneycomb" => {
                let pitch = 0.6;
                let rad = pitch / (1. + 2f32.sqrt()) / (2. * (22.5f32).to_radians().sin());
                for j in -1..=1 {
                    for i in -1..=1 {
                        let c = (0.5 + pitch * i as f32, 0.5 + pitch * j as f32);
                        self.closed(&Self::ngon(c, (rad, rad), 8, 22.5), fine);
                    }
                }
            }
            "lateral-honeycomb" => {
                let (rad, sx) = (0.22, 1.45);
                let h = 3f32.sqrt() * rad;
                for i in -2..=2 {
                    for j in -2..=2 {
                        let c = (
                            0.5 + 1.5 * rad * sx * i as f32,
                            0.5 + h * (j as f32 + if i % 2 == 0 { 0. } else { 0.5 }),
                        );
                        self.closed(&Self::ngon(c, (rad * sx, rad), 6, 0.), fine);
                    }
                }
            }
            "lateral-lattice" => {
                self.hatch(65., 0.36, 0., fine);
                self.hatch(115., 0.36, 0., fine);
            }
            // Cross hatch: blocks of lines that turn from block to block.
            "crosshatch" => {
                for (qx, qy, across) in [
                    (0., 0., true),
                    (0.5, 0., false),
                    (0., 0.5, false),
                    (0.5, 0.5, true),
                ] {
                    for t in [0.17, 0.33] {
                        if across {
                            self.line(&[(qx, qy + t), (qx + 0.5, qy + t)], bold);
                        } else {
                            self.line(&[(qx + t, qy), (qx + t, qy + 0.5)], bold);
                        }
                    }
                }
            }
            // Triply periodic surfaces: gyroid's waves, TPMS-D's wavy diamonds, TPMS-FK's
            // round cells.
            "gyroid" => {
                let wave = |flip: bool| -> Vec<Pt> {
                    (0..=16)
                        .map(|i| {
                            let t = i as f32 / 16.;
                            let y = 0.35 * (t * TAU).sin();
                            (t, 0.5 + if flip { -y } else { y })
                        })
                        .collect()
                };
                self.line(&wave(false), bold);
                self.line(&wave(true), self.thin());
            }
            "tpmsd" => {
                for (deg, s) in [(45f32, fine), (-45., self.thin())] {
                    let a = deg.to_radians();
                    let (dx, dy) = (a.cos(), a.sin());
                    for o in [-0.36, 0., 0.36] {
                        let pts: Vec<Pt> = (0..=24)
                            .map(|i| {
                                let t = -0.75 + 1.5 * i as f32 / 24.;
                                let w = o + 0.07 * (t * TAU / 0.5).sin();
                                (0.5 + dx * t - dy * w, 0.5 + dy * t + dx * w)
                            })
                            .collect();
                        self.line(&pts, s);
                    }
                }
            }
            "tpmsfk" => {
                for (c, rad) in [
                    ((0.5, 0.5), 0.17),
                    ((0., 0.), 0.27),
                    ((1., 0.), 0.27),
                    ((0., 1.), 0.27),
                    ((1., 1.), 0.27),
                ] {
                    let w = rad * self.r.width();
                    self.p.circle_stroke(self.at(c), w, bold);
                }
            }
            "concentric" => {
                for (a, b) in [(0.18, 0.82), (0.34, 0.66)] {
                    self.closed(&[(a, a), (b, a), (b, b), (a, b)], bold);
                }
            }
            "lightning" => {
                self.line(&[(0.5, 1.), (0.5, 0.5), (0.2, 0.1)], bold);
                self.line(&[(0.5, 0.5), (0.8, 0.2)], bold);
                self.line(&[(0.65, 0.35), (0.9, 0.5)], self.thin());
            }
            // Space-filling and spiral surface fills.
            "hilbertcurve" => {
                const H: [(u8, u8); 16] = [
                    (0, 0),
                    (1, 0),
                    (1, 1),
                    (0, 1),
                    (0, 2),
                    (0, 3),
                    (1, 3),
                    (1, 2),
                    (2, 2),
                    (2, 3),
                    (3, 3),
                    (3, 2),
                    (3, 1),
                    (2, 1),
                    (2, 0),
                    (3, 0),
                ];
                let pts: Vec<Pt> = H
                    .iter()
                    .map(|&(x, y)| ((x as f32 + 0.5) / 4., (y as f32 + 0.5) / 4.))
                    .collect();
                self.line(&pts, bold);
            }
            "archimedeanchords" => {
                // Straight chords along an Archimedean spiral, eight per turn.
                let pts: Vec<Pt> = (0..=20)
                    .map(|i| {
                        let t = i as f32 / 20.;
                        let a = t * 2.5 * TAU;
                        (0.5 + 0.46 * t * a.cos(), 0.5 + 0.46 * t * a.sin())
                    })
                    .collect();
                self.line(&pts, bold);
            }
            "octagramspiral" => {
                // Eight-pointed stars, one inside the other.
                let star = |out: f32, inner: f32| -> Vec<Pt> {
                    (0..16)
                        .map(|i| {
                            let a = i as f32 / 16. * TAU - FRAC_PI_2;
                            let rad = if i % 2 == 0 { out } else { inner };
                            (0.5 + rad * a.cos(), 0.5 + rad * a.sin())
                        })
                        .collect()
                };
                self.closed(&star(0.5, 0.3), fine);
                self.closed(&star(0.22, 0.12), self.thin());
            }
            // Support choices that are not one pattern: the default (a sparkle) and hollow
            // (only the dashed outline).
            "auto" | "default" => {
                let star: Vec<Pt> = (0..8)
                    .map(|i| {
                        let a = (i as f32 * 45. - 90.).to_radians();
                        let rad = if i % 2 == 0 { 0.4 } else { 0.13 };
                        (0.5 + rad * a.cos(), 0.5 + rad * a.sin())
                    })
                    .collect();
                self.closed(&star, self.acc(1.3));
            }
            "hollow" => {
                let pts: Vec<Pos2> = [(0.2, 0.2), (0.8, 0.2), (0.8, 0.8), (0.2, 0.8), (0.2, 0.2)]
                    .iter()
                    .map(|&q| self.at(q))
                    .collect();
                self.p
                    .extend(Shape::dashed_line(&pts, self.thin(), 1.5, 1.5));
            }
            _ => return false,
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PATTERN_KEYS: [&str; 5] = [
        "fill_pattern",
        "top_fill_pattern",
        "bottom_fill_pattern",
        "support_material_pattern",
        "support_material_interface_pattern",
    ];

    /// Whether `paint` knows `value`, and the shapes it draws for it.
    fn drawn(ctx: &egui::Context, key: &str, value: &str) -> (bool, String) {
        let mut ok = false;
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            let rect = Rect::from_min_size(pos2(10., 10.), egui::vec2(16., 16.));
            ok = paint(ui, rect, key, value);
        });
        out.textures_delta.clear();
        let shapes: Vec<_> = out.shapes.into_iter().map(|c| c.shape).collect();
        (ok, format!("{shapes:?}"))
    }

    #[test]
    fn every_offered_pattern_has_an_icon() {
        let text = crate::settings::text();
        for key in PATTERN_KEYS {
            let enums = &text.get(key).expect(key).enums;
            assert!(!enums.is_empty(), "{key} has no choices");
            for (value, _) in enums {
                assert!(FILL_PATTERNS.contains(&value.as_str()), "{key} = {value}");
            }
        }
        // Every name `fill::Pattern` reads, bar the internal-only Ensuring.
        for name in [
            "rectilinear",
            "alignedrectilinear",
            "monotonic",
            "monotoniclines",
            "monotonicline",
            "grid",
            "triangles",
            "stars",
            "cubic",
            "gyroid",
            "concentric",
            "honeycomb",
            "3dhoneycomb",
            "hilbertcurve",
            "archimedeanchords",
            "octagramspiral",
            "crosshatch",
            "adaptivecubic",
            "supportcubic",
            "lightning",
        ] {
            assert!(
                mica_core::fill::Pattern::from_config_name(name).is_some(),
                "{name}"
            );
            assert!(FILL_PATTERNS.contains(&name), "{name}");
        }
    }

    #[test]
    fn every_pattern_icon_is_its_own() {
        let ctx = egui::Context::default();
        let mut seen: Vec<(&str, String)> = Vec::new();
        for &value in FILL_PATTERNS {
            let (ok, verts) = drawn(&ctx, "fill_pattern", value);
            assert!(ok, "{value} has no icon");
            // Aliases of one pattern share a picture.
            let alias = matches!(value, "monotonicline" | "zig-zag" | "default");
            if let Some((other, _)) = seen.iter().find(|(_, v)| *v == verts) {
                assert!(alias, "{value} looks the same as {other}");
            }
            seen.push((value, verts));
        }
        assert!(!drawn(&ctx, "fill_pattern", "no-such-pattern").0);
    }
}
