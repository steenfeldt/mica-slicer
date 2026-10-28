// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Line icons of the process settings: one per topic in the rail (Essentials, Quality, …) and
//! one per section inside a topic (Layer height, Seam, Infill, …). Drawn with the egui painter
//! on a 24-unit grid as thin strokes, so they follow the theme and need no image assets. Every
//! picture is Mica's own.

use eframe::egui::{self, Color32, Pos2, Rect, Shape, Stroke};
use std::f32::consts::{PI, TAU};

/// A stroke pen on a 24 × 24 grid mapped onto `rect`.
struct Pen<'a> {
    painter: &'a egui::Painter,
    rect: Rect,
    stroke: Stroke,
}

impl Pen<'_> {
    fn at(&self, x: f32, y: f32) -> Pos2 {
        let s = self.rect.width().min(self.rect.height()) / 24.;
        let c = self.rect.center();
        Pos2::new(c.x + (x - 12.) * s, c.y + (y - 12.) * s)
    }
    fn scale(&self) -> f32 {
        self.rect.width().min(self.rect.height()) / 24.
    }
    fn line(&self, pts: &[(f32, f32)]) {
        let pts: Vec<Pos2> = pts.iter().map(|&(x, y)| self.at(x, y)).collect();
        self.painter.add(Shape::line(pts, self.stroke));
    }
    fn closed(&self, pts: &[(f32, f32)]) {
        let pts: Vec<Pos2> = pts.iter().map(|&(x, y)| self.at(x, y)).collect();
        self.painter.add(Shape::closed_line(pts, self.stroke));
    }
    fn rect(&self, x: f32, y: f32, w: f32, h: f32, r: f32) {
        let rect = Rect::from_min_max(self.at(x, y), self.at(x + w, y + h));
        self.painter.rect_stroke(
            rect,
            r * self.scale(),
            self.stroke,
            egui::StrokeKind::Middle,
        );
    }
    fn circle(&self, x: f32, y: f32, r: f32) {
        self.painter
            .circle_stroke(self.at(x, y), r * self.scale(), self.stroke);
    }
    fn dot(&self, x: f32, y: f32, r: f32) {
        self.painter
            .circle_filled(self.at(x, y), r * self.scale(), self.stroke.color);
    }
    /// An arc from angle `a0` to `a1` (radians, screen orientation: 0 = right, π/2 = down).
    fn arc(&self, x: f32, y: f32, r: f32, a0: f32, a1: f32) {
        let n = ((a1 - a0).abs() / 0.2).ceil().max(2.) as usize;
        let pts: Vec<(f32, f32)> = (0..=n)
            .map(|i| {
                let a = a0 + (a1 - a0) * i as f32 / n as f32;
                (x + r * a.cos(), y + r * a.sin())
            })
            .collect();
        self.line(&pts);
    }
    fn dashed(&self, pts: &[(f32, f32)]) {
        let pts: Vec<Pos2> = pts.iter().map(|&(x, y)| self.at(x, y)).collect();
        let s = self.scale();
        self.painter
            .add(Shape::dashed_line(&pts, self.stroke, 2.5 * s, 2. * s));
    }
}

fn pen(painter: &egui::Painter, rect: Rect, color: Color32) -> Pen<'_> {
    // About 1.5 px at 16 px, a little bolder in the larger rail icons.
    let width = (rect.width().min(rect.height()) / 11.).clamp(1.3, 1.7);
    Pen {
        painter,
        rect,
        stroke: Stroke::new(width, color),
    }
}

/// The rail icon of a settings topic.
pub fn topic(painter: &egui::Painter, rect: Rect, name: &str, color: Color32) {
    let p = pen(painter, rect, color);
    match name {
        "Essentials" => sliders(&p),
        "Quality" => {
            // A cut gem.
            p.closed(&[(12., 20.), (3., 9.), (7., 4.), (17., 4.), (21., 9.)]);
            p.line(&[(3., 9.), (21., 9.)]);
            p.line(&[(9.5, 9.), (12., 20.), (14.5, 9.)]);
        }
        "Strength" => {
            // A brick wall.
            p.rect(3., 5., 18., 14., 1.5);
            p.line(&[(3., 9.7), (21., 9.7)]);
            p.line(&[(3., 14.3), (21., 14.3)]);
            p.line(&[(12., 5.), (12., 9.7)]);
            p.line(&[(8., 9.7), (8., 14.3)]);
            p.line(&[(16., 9.7), (16., 14.3)]);
            p.line(&[(12., 14.3), (12., 19.)]);
        }
        "Speed" => gauge(&p),
        "Support" => pillars(&p),
        "Cooling & temperatures" => thermometer(&p),
        "Multimaterial" => two_spools(&p),
        "Texture" => knurl(&p),
        "Preview" => {
            // A legend: three swatches, each with its label line.
            for y in [4., 10.5, 17.] {
                p.rect(3., y, 4., 4., 1.);
                p.line(&[(10., y + 2.), (21., y + 2.)]);
            }
        }
        _ => cog(&p),
    }
}

/// The icon of a Settings window tab (`general`, `slicing`, `files`, `features`).
pub fn settings_tab(painter: &egui::Painter, rect: Rect, key: &str, color: Color32) {
    let p = pen(painter, rect, color);
    match key {
        "slicing" => {
            // A stack of layers.
            p.closed(&[(3., 8.), (12., 4.), (21., 8.), (12., 12.)]);
            p.line(&[(3., 12.), (12., 16.), (21., 12.)]);
            p.line(&[(3., 16.), (12., 20.), (21., 16.)]);
        }
        "files" => {
            // A folder.
            p.closed(&[
                (3., 6.),
                (9., 6.),
                (11., 8.5),
                (21., 8.5),
                (21., 19.),
                (3., 19.),
            ]);
            p.line(&[(3., 11.), (21., 11.)]);
        }
        "profiles" => {
            // Two stacked profile cards, the front one with lines of settings.
            p.line(&[
                (6.5, 6.5),
                (6.5, 3.5),
                (20.5, 3.5),
                (20.5, 18.5),
                (17.5, 18.5),
            ]);
            p.rect(3.5, 6.5, 14., 15., 2.);
            p.line(&[(6.5, 11.), (14.5, 11.)]);
            p.line(&[(6.5, 14.5), (14.5, 14.5)]);
            p.line(&[(6.5, 18.), (11.5, 18.)]);
        }
        "features" => {
            // A switch, turned on.
            p.rect(2.5, 7., 19., 10., 5.);
            p.dot(16.5, 12., 3.);
        }
        "about" => {
            // An "i" in a circle.
            p.circle(12., 12., 9.);
            p.dot(12., 7.8, 1.3);
            p.line(&[(12., 10.8), (12., 16.8)]);
        }
        "credits" => {
            // A heart.
            p.closed(&[
                (12., 19.5),
                (4.5, 12.),
                (3.5, 8.5),
                (5.5, 5.5),
                (8.5, 4.8),
                (12., 7.5),
                (15.5, 4.8),
                (18.5, 5.5),
                (20.5, 8.5),
                (19.5, 12.),
            ]);
        }
        _ => cog(&p),
    }
}

/// Diamond knurling: the Texture topic and section.
fn knurl(p: &Pen<'_>) {
    p.rect(3.5, 3.5, 17., 17., 2.);
    for k in [0., 6., 12.] {
        p.line(&[(3.5 + k, 3.5), (20.5, 20.5 - k)]);
        p.line(&[(20.5 - k, 3.5), (3.5, 20.5 - k)]);
    }
}

/// The small icon in front of a section title. `name` is the section's group name; a
/// per-tool suffix (`Cooling · tool 2`) is ignored. Returns false when the section has no
/// picture of its own (a plain ring is drawn).
pub fn section(painter: &egui::Painter, rect: Rect, name: &str, color: Color32) -> bool {
    let p = pen(painter, rect, color);
    let base = name.split(" · ").next().unwrap_or(name);
    match base {
        // The Preview page's sections (`preview_side.rs`).
        "Color by" => {
            // A scale bar in three bands with ticks under it.
            p.rect(3., 6., 18., 7., 1.5);
            p.line(&[(9., 6.), (9., 13.)]);
            p.line(&[(15., 6.), (15., 13.)]);
            for x in [3.5, 12., 20.5] {
                p.line(&[(x, 16.), (x, 19.)]);
            }
        }
        "Totals" => {
            // A stopwatch.
            p.circle(12., 13.5, 7.5);
            p.line(&[(12., 13.5), (12., 9.)]);
            p.line(&[(10., 3.5), (14., 3.5)]);
            p.line(&[(12., 3.5), (12., 6.)]);
        }
        "Layer" => {
            // One slab lifted out of a stack.
            p.closed(&[(3., 9.), (12., 5.), (21., 9.), (12., 13.)]);
            p.line(&[(3., 15.), (12., 19.), (21., 15.)]);
        }
        "Layer height" => {
            p.closed(&[(3., 8.), (12., 4.), (21., 8.), (12., 12.)]);
            p.line(&[(3., 12.), (12., 16.), (21., 12.)]);
            p.line(&[(3., 16.), (12., 20.), (21., 16.)]);
        }
        "Line width" => {
            p.line(&[(4., 5.), (4., 19.)]);
            p.line(&[(20., 5.), (20., 19.)]);
            p.line(&[(7., 12.), (17., 12.)]);
            p.line(&[(10., 9.), (7., 12.), (10., 15.)]);
            p.line(&[(14., 9.), (17., 12.), (14., 15.)]);
        }
        "Seam" => {
            // A wall loop seen from above, its seam a short stitch where it starts and ends.
            p.rect(3.5, 3.5, 17., 17., 5.);
            p.dot(9., 3.5, 2.6);
            p.line(&[(9., 8.), (9., 12.)]);
        }
        "Precision" => {
            p.circle(12., 12., 6.);
            p.line(&[(12., 2.5), (12., 8.)]);
            p.line(&[(12., 16.), (12., 21.5)]);
            p.line(&[(2.5, 12.), (8., 12.)]);
            p.line(&[(16., 12.), (21.5, 12.)]);
        }
        "Ironing" => {
            p.closed(&[(3., 18.), (21., 18.), (20., 13.), (15.5, 9.5), (7., 9.5)]);
            p.line(&[(9., 9.5), (9., 6.), (16., 6.)]);
        }
        "Wall generator" => {
            p.rect(3., 3., 18., 18., 4.);
            p.rect(7.5, 7.5, 9., 9., 2.);
        }
        "Walls and surfaces" => {
            p.rect(3., 3., 18., 18., 1.5);
            p.line(&[(3., 8.), (21., 8.)]);
            p.line(&[(8., 8.), (8., 21.)]);
        }
        "Overhangs" | "Overhang speed" => {
            p.line(&[(3., 20.), (9., 20.), (9., 11.), (21., 11.)]);
            p.dashed(&[(21., 11.), (21., 20.)]);
            if base == "Overhang speed" {
                p.line(&[(12., 5.), (20., 5.)]);
                p.line(&[(17., 2.5), (20., 5.), (17., 7.5)]);
            }
        }
        "Bridges" => {
            p.line(&[(2.5, 8.), (21.5, 8.)]);
            p.line(&[(4., 8.), (4., 20.)]);
            p.line(&[(20., 8.), (20., 20.)]);
            p.arc(12., 20., 8., PI, TAU);
        }
        "Walls" => {
            p.rect(3., 3., 18., 18., 1.5);
            p.rect(6.5, 6.5, 11., 11., 1.);
            p.rect(10., 10., 4., 4., 0.5);
        }
        "Brick layers" => {
            // Two columns of beads, one shifted half a bead up.
            for y in [4., 10., 16.] {
                p.rect(4., y, 7., 5., 1.);
            }
            for y in [7., 13.] {
                p.rect(13., y, 7., 5., 1.);
            }
        }
        "Top/bottom shells" => {
            p.rect(3., 3., 18., 18., 1.5);
            p.line(&[(3., 7.), (21., 7.)]);
            p.line(&[(3., 17.), (21., 17.)]);
            p.dashed(&[(7., 12.), (17., 12.)]);
        }
        "Infill" => {
            p.rect(3., 3., 18., 18., 1.5);
            p.line(&[(3., 12.), (12., 3.)]);
            p.line(&[(3., 21.), (21., 3.)]);
            p.line(&[(12., 21.), (21., 12.)]);
        }
        "First layer speed" => {
            p.line(&[(3., 20.), (21., 20.)]);
            p.line(&[(4., 14.), (18., 14.)]);
            p.line(&[(14., 10.), (18., 14.), (14., 18.)]);
        }
        "Other layers speed" | "Speed" => {
            p.line(&[(5., 6.), (11., 12.), (5., 18.)]);
            p.line(&[(12., 6.), (18., 12.), (12., 18.)]);
        }
        "Travel speed" => {
            p.dashed(&[(3., 12.), (18., 12.)]);
            p.line(&[(16., 8.), (20., 12.), (16., 16.)]);
            p.circle(4., 12., 1.2);
        }
        "Acceleration" => {
            let curve: Vec<(f32, f32)> = (0..=10)
                .map(|i| {
                    let t = i as f32 / 10.;
                    (4. + 15. * t, 20. - 15. * t * t)
                })
                .collect();
            p.line(&curve);
            p.line(&[(14.5, 5.), (19., 5.), (19., 9.5)]);
            p.line(&[(3., 21.), (21., 21.)]);
        }
        "Support" | "Supports" => pillars(&p),
        "Raft" => {
            p.rect(7., 4., 10., 8., 1.);
            p.line(&[(3., 15.), (21., 15.)]);
            p.line(&[(3., 18.), (21., 18.)]);
            p.line(&[(3., 21.), (21., 21.)]);
        }
        "Support filament" => {
            p.circle(12., 12., 8.5);
            p.circle(12., 12., 2.5);
            p.arc(12., 12., 5.5, -0.6, 1.2);
        }
        "Tree supports" => {
            p.line(&[(12., 21.), (12., 11.)]);
            p.line(&[(12., 15.), (6., 9.), (6., 4.)]);
            p.line(&[(12., 13.), (18., 7.), (18., 4.)]);
            p.line(&[(12., 11.), (12., 4.)]);
            p.line(&[(4., 21.), (20., 21.)]);
        }
        "Prime tower" => {
            p.rect(8., 4., 8., 16., 1.);
            p.line(&[(8., 8.), (16., 8.)]);
            p.line(&[(8., 12.), (16., 12.)]);
            p.line(&[(8., 16.), (16., 16.)]);
            p.line(&[(3., 20.), (21., 20.)]);
        }
        "Filament for Features" => two_spools(&p),
        // Purge into the part: an arrow down into a wall with zigzag infill.
        "Flush options" => {
            p.rect(4., 10., 16., 11., 1.);
            p.line(&[(6., 19.), (9., 13.), (12., 19.), (15., 13.), (18., 19.)]);
            p.line(&[(12., 2.), (12., 8.)]);
            p.line(&[(9.5, 5.5), (12., 8.), (14.5, 5.5)]);
        }
        "Ooze prevention" => {
            let mut drop: Vec<(f32, f32)> = vec![(12., 3.)];
            let (a0, a1) = (-0.6_f32, PI + 0.6);
            for i in 0..=12 {
                let a = a0 + (a1 - a0) * i as f32 / 12.;
                drop.push((12. + 6. * a.cos(), 14. + 6. * a.sin()));
            }
            p.closed(&drop);
        }
        "Skirt" => {
            p.rect(8., 8., 8., 8., 1.);
            p.dashed(&[(3., 3.), (21., 3.), (21., 21.), (3., 21.), (3., 3.)]);
        }
        "Brim" | "Bed adhesion" => {
            p.rect(8., 8., 8., 8., 1.);
            p.rect(3.5, 3.5, 17., 17., 3.);
        }
        "Special mode" => {
            let spiral: Vec<(f32, f32)> = (0..=60)
                .map(|i| {
                    let t = i as f32 / 60.;
                    let a = t * 2.6 * TAU;
                    let r = 1. + 8. * t;
                    (12. + r * a.cos(), 12. + r * a.sin())
                })
                .collect();
            p.line(&spiral);
        }
        "Texture" => knurl(&p),
        "Texture placement" => {
            // A tile with its repeat marks and a turn arrow.
            p.rect(4., 4., 11., 11., 1.);
            p.dashed(&[(15., 4.), (20., 4.), (20., 9.)]);
            p.dashed(&[(4., 15.), (4., 20.), (9., 20.)]);
            p.arc(15., 15., 5., 0.3, 1.9);
        }
        "Texture detail" => {
            // A magnifier over a wavy line.
            p.circle(10., 10., 6.);
            p.line(&[(14.5, 14.5), (20., 20.)]);
            p.line(&[(6., 11.), (8., 8.5), (10., 11.), (12., 8.5), (14., 11.)]);
        }
        "Texture limits" => {
            // A slope with its angle.
            p.line(&[(3., 19.), (21., 19.)]);
            p.line(&[(3., 19.), (19., 5.)]);
            p.arc(3., 19., 9., -0.72, 0.);
        }
        "Texture painting" => {
            // A brush over a soft-edged patch.
            p.line(&[(19., 4.), (11., 12.)]);
            p.closed(&[(11., 12.), (8., 15.), (7., 18.), (10., 17.), (13., 14.)]);
            p.dashed(&[(3., 21.), (12., 21.)]);
        }
        "Fuzzy Skin" => {
            p.line(&[
                (4., 20.),
                (4., 9.),
                (6., 6.),
                (8., 9.),
                (10., 6.),
                (12., 9.),
                (14., 6.),
                (16., 9.),
                (18., 6.),
                (20., 9.),
                (20., 20.),
                (4., 20.),
            ]);
        }
        "G-code output" => {
            p.closed(&[(6., 3.), (15., 3.), (19., 7.), (19., 21.), (6., 21.)]);
            p.line(&[(15., 3.), (15., 7.), (19., 7.)]);
            p.line(&[(11., 11.), (9., 13.5), (11., 16.)]);
            p.line(&[(14., 11.), (16., 13.5), (14., 16.)]);
        }
        "Post-processing Scripts" => {
            p.rect(3., 5., 18., 14., 2.);
            p.line(&[(7., 10.), (10., 12.5), (7., 15.)]);
            p.line(&[(12., 15.), (16., 15.)]);
        }
        "Notes" => {
            p.rect(5., 3., 14., 18., 1.5);
            p.line(&[(8., 8.), (16., 8.)]);
            p.line(&[(8., 12.), (16., 12.)]);
            p.line(&[(8., 16.), (13., 16.)]);
        }
        "Temperatures" => thermometer(&p),
        "Cooling" => fan(&p),
        "Multiple objects" => {
            p.rect(3., 8., 8., 8., 1.);
            p.rect(13., 8., 8., 8., 1.);
            p.line(&[(2., 19.), (22., 19.)]);
        }
        "Advanced" => sliders(&p),
        "More settings" => {
            p.circle(6., 12., 1.4);
            p.circle(12., 12., 1.4);
            p.circle(18., 12., 1.4);
        }
        _ => {
            p.circle(12., 12., 8.);
            p.circle(12., 12., 1.4);
            return false;
        }
    }
    true
}

fn sliders(p: &Pen<'_>) {
    p.line(&[(3., 7.), (7., 7.)]);
    p.line(&[(11., 7.), (21., 7.)]);
    p.circle(9., 7., 2.);
    p.line(&[(3., 12.), (13., 12.)]);
    p.line(&[(17., 12.), (21., 12.)]);
    p.circle(15., 12., 2.);
    p.line(&[(3., 17.), (5., 17.)]);
    p.line(&[(9., 17.), (21., 17.)]);
    p.circle(7., 17., 2.);
}

fn gauge(p: &Pen<'_>) {
    p.arc(12., 15., 9., PI, TAU);
    p.line(&[(3., 15.), (5., 15.)]);
    p.line(&[(19., 15.), (21., 15.)]);
    p.line(&[(12., 15.), (16.5, 9.)]);
    p.circle(12., 15., 1.3);
}

fn pillars(p: &Pen<'_>) {
    p.line(&[(3., 5.), (21., 5.)]);
    p.line(&[(7., 5.), (7., 20.)]);
    p.line(&[(12., 5.), (12., 20.)]);
    p.line(&[(17., 5.), (17., 20.)]);
    p.line(&[(3., 20.), (21., 20.)]);
}

fn thermometer(p: &Pen<'_>) {
    p.circle(11., 17.5, 3.5);
    p.line(&[(9., 14.6), (9., 5.)]);
    p.line(&[(13., 14.6), (13., 5.)]);
    p.arc(11., 5., 2., PI, TAU);
    p.line(&[(11., 16.), (11., 9.)]);
    p.line(&[(16., 6.), (19., 6.)]);
    p.line(&[(16., 9.5), (19., 9.5)]);
}

fn fan(p: &Pen<'_>) {
    p.circle(12., 12., 1.8);
    for k in 0..3 {
        let a = k as f32 * TAU / 3. - PI / 2.;
        let tip = |da: f32, r: f32| (12. + r * (a + da).cos(), 12. + r * (a + da).sin());
        p.closed(&[tip(0., 2.5), tip(-0.45, 9.), tip(0.35, 9.)]);
    }
}

fn two_spools(p: &Pen<'_>) {
    p.circle(8.5, 12., 5.5);
    p.circle(15.5, 12., 5.5);
}

fn cog(p: &Pen<'_>) {
    p.circle(12., 12., 3.);
    p.circle(12., 12., 6.5);
    for k in 0..8 {
        let a = k as f32 * TAU / 8.;
        p.line(&[
            (12. + 6.5 * a.cos(), 12. + 6.5 * a.sin()),
            (12. + 9. * a.cos(), 12. + 9. * a.sin()),
        ]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every print section Mica shows has its own picture, not the fallback ring.
    #[test]
    fn every_print_section_has_an_icon() {
        let ctx = egui::Context::default();
        let mut missing = Vec::new();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let rect = Rect::from_min_size(Pos2::ZERO, egui::vec2(16., 16.));
            assert!(!section(
                ui.painter(),
                rect,
                "no such section",
                Color32::WHITE
            ));
            for page in crate::settings::pages(crate::settings::Tab::Print) {
                for group in &page.groups {
                    if !section(ui.painter(), rect, &group.name, Color32::WHITE) {
                        missing.push(format!("{} / {}", page.name, group.name));
                    }
                }
            }
            for (group, _) in crate::settings::ESSENTIALS_MORE {
                if !section(ui.painter(), rect, group, Color32::WHITE) {
                    missing.push(format!("Essentials / {group}"));
                }
            }
        });
        output.textures_delta.clear();
        assert!(missing.is_empty(), "sections without an icon: {missing:?}");
    }
}
