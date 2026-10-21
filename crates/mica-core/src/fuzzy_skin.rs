// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: prusaslicer@b028299c77 src/libslic3r/Feature/FuzzySkin/FuzzySkin.cpp :: fuzzy_polyline, fuzzy_extrusion_line, should_fuzzify
// Changes: Reproducible per-loop PRNG, independent of Rayon batch order; spatial painting callback sampled along wall segments.

//! Fuzzy displacement before seam selection and variable-width extrusion splitting.
use crate::{
    arachne::{ExtrusionLine, Junction},
    geometry::{Point, Polygon, SCALE, signed_area},
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    None,
    External,
    All,
}
#[derive(Clone, Debug, PartialEq)]
pub struct FuzzySkinSettings {
    pub mode: Mode,
    pub thickness: f64,
    pub point_distance: f64,
    pub seed: u64,
}
impl Default for FuzzySkinSettings {
    fn default() -> Self {
        Self {
            mode: Mode::None,
            thickness: 0.3,
            point_distance: 0.8,
            seed: 0,
        }
    }
}
/// Optional local override, in bed coordinates. Returning None uses the object's settings.
/// A 3MF painting decoder can supply this without changing either wall generator.
pub trait FuzzySkinPainting: Sync {
    fn needs_sampling(&self) -> bool {
        true
    }
    fn settings_at(&self, point: Point, layer: usize) -> Option<FuzzySkinSettings>;
    /// The object's texture skin (Mica's own wall texture), if it has one.
    fn texture_skin(&self) -> Option<crate::texture_skin::Surface<'_>> {
        None
    }
}
pub struct NoPainting;
impl FuzzySkinPainting for NoPainting {
    fn needs_sampling(&self) -> bool {
        false
    }
    fn settings_at(&self, _: Point, _: usize) -> Option<FuzzySkinSettings> {
        None
    }
}
impl FuzzySkinSettings {
    pub fn should_fuzzify(&self, layer: usize, depth: usize, contour: bool) -> bool {
        layer > 0
            && depth == 0
            && self.thickness.is_finite()
            && self.point_distance.is_finite()
            && self.thickness > 0.
            && self.point_distance > 0.
            && (self.mode == Mode::All || (self.mode == Mode::External && contour))
    }
    pub fn from_config(c: &crate::config::Config) -> Result<Self, crate::resolve::ResolveError> {
        let mode = match c.string("fuzzy_skin").unwrap_or("none") {
            "none" => Mode::None,
            "external" => Mode::External,
            "all" => Mode::All,
            v => {
                return Err(crate::resolve::ResolveError(format!(
                    "unsupported fuzzy_skin: {v}"
                )));
            }
        };
        let thickness = c.float("fuzzy_skin_thickness").unwrap_or(0.3);
        let point_distance = c.float("fuzzy_skin_point_dist").unwrap_or(0.8);
        if !thickness.is_finite()
            || thickness < 0.
            || !point_distance.is_finite()
            || (mode != Mode::None && point_distance <= 0.)
        {
            return Err(crate::resolve::ResolveError(
                "fuzzy skin requires nonnegative thickness and positive point distance".into(),
            ));
        }
        Ok(Self {
            mode,
            thickness,
            point_distance,
            seed: 0,
        })
    }
}
struct Random(u64);
impl Random {
    fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_add(0x9e3779b97f4a7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
        ((z ^ (z >> 31)) >> 11) as f64 / (1u64 << 53) as f64
    }
}
fn random(s: &FuzzySkinSettings, layer: usize, first: Point) -> Random {
    Random(
        s.seed
            ^ (layer as u64).wrapping_mul(0x517cc1b727220a95)
            ^ (first.x as u64).rotate_left(23)
            ^ first.y as u64,
    )
}

pub fn fuzzy_polygon(
    poly: &mut Polygon,
    s: &FuzzySkinSettings,
    layer: usize,
    depth: usize,
    contour: bool,
    painting: &dyn FuzzySkinPainting,
) {
    if poly.len() < 3 {
        return;
    }
    if painting.needs_sampling() {
        let mut sampled = Vec::new();
        for i in 0..poly.len() {
            let a = poly[i];
            let b = poly[(i + 1) % poly.len()];
            let n = (a.distance_to(b) / (0.1 * SCALE)).ceil().max(1.) as usize;
            for j in 0..n {
                let t = j as f64 / n as f64;
                sampled.push(Point::new(
                    a.x + ((b.x - a.x) as f64 * t).round() as i64,
                    a.y + ((b.y - a.y) as f64 * t).round() as i64,
                ));
            }
        }
        *poly = sampled;
    }
    let mut rng = random(s, layer, poly[0]);
    let mut left = rng.next() * s.point_distance * SCALE * 0.375;
    let mut out = Vec::new();
    let mut a = *poly.last().unwrap();
    for &b in poly.iter() {
        let local = painting.settings_at(a, layer);
        let cfg = local.as_ref().unwrap_or(s);
        let len = a.distance_to(b);
        if !cfg.should_fuzzify(layer, depth, contour) {
            out.push(b);
            left = 0.;
            a = b;
            continue;
        }
        while left < len {
            let r = (rng.next() * 2. - 1.) * cfg.thickness * SCALE;
            let dx = (b.x - a.x) as f64;
            let dy = (b.y - a.y) as f64;
            out.push(Point::new(
                a.x + (dx * left / len - dy / len * r) as i64,
                a.y + (dy * left / len + dx / len * r) as i64,
            ));
            left += (0.75 + rng.next() * 0.5) * cfg.point_distance * SCALE;
        }
        left -= len;
        a = b;
    }
    if out.len() >= 3 {
        *poly = out;
    }
}

pub fn fuzzy_extrusion_line(
    line: &mut ExtrusionLine,
    s: &FuzzySkinSettings,
    layer: usize,
    painting: &dyn FuzzySkinPainting,
) {
    if line.junctions.len() < 2 {
        return;
    }
    if painting.needs_sampling() {
        let mut sampled = vec![line.junctions[0]];
        for pair in line.junctions.windows(2) {
            let a = pair[0];
            let b = pair[1];
            let n = (a.p.distance_to(b.p) / (0.1 * SCALE)).ceil().max(1.) as usize;
            for j in 1..=n {
                let t = j as f64 / n as f64;
                sampled.push(Junction {
                    p: Point::new(
                        a.p.x + ((b.p.x - a.p.x) as f64 * t).round() as i64,
                        a.p.y + ((b.p.y - a.p.y) as f64 * t).round() as i64,
                    ),
                    width: a.width + (b.width - a.width) * t,
                    ..b
                });
            }
        }
        line.junctions = sampled;
    }
    let contour = !line.is_closed || signed_area(&line.polygon()) > 0.;
    let mut rng = random(s, layer, line.junctions[0].p);
    let mut left = rng.next() * s.point_distance * SCALE * 0.375;
    let mut out = vec![line.junctions[0]];
    for pair in line.junctions.windows(2) {
        let a = pair[0].p;
        let b = pair[1].p;
        let len = a.distance_to(b);
        let local = painting.settings_at(a, layer);
        let cfg = local.as_ref().unwrap_or(s);
        if !cfg.should_fuzzify(layer, line.inset, contour) {
            out.push(pair[1]);
            left = 0.;
            continue;
        }
        while left < len {
            let r = (rng.next() * 2. - 1.) * cfg.thickness * SCALE;
            let dx = (b.x - a.x) as f64;
            let dy = (b.y - a.y) as f64;
            out.push(Junction {
                p: Point::new(
                    a.x + (dx * left / len - dy / len * r) as i64,
                    a.y + (dy * left / len + dx / len * r) as i64,
                ),
                ..pair[1]
            });
            left += (0.75 + rng.next() * 0.5) * cfg.point_distance * SCALE;
        }
        left -= len;
    }
    if out.len() >= 3 {
        if line.is_closed {
            out[0].p = out.last().unwrap().p;
        }
        line.junctions = out;
    }
}
