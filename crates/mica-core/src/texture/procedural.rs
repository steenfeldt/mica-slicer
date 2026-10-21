// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Mica's own textures, generated from a few parameters. Every pattern is a periodic function on
//! the unit tile (period 1 in U and V), so the generated maps tile without a seam.
use super::heightmap::HeightMap;
use std::f64::consts::{PI, TAU};

/// Width of a generated map, px; the height follows the pattern's proportions.
pub const SIZE: usize = 512;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Pattern {
    KnurlStraight,
    KnurlDiamond,
    Hexagons,
    Diamonds,
    Waves,
    Ribs,
    Flutes,
    Scales,
    Dots,
    CarbonWeave,
    Twill,
    WoodGrain,
}

/// Parameters every pattern understands: `ratio` (0–1) is the pattern's own proportion (land
/// width, groove width, dot size, wave height, gap between strands, grain wobble) and
/// `softness` (0–1) rounds its profile.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Params {
    pub ratio: f64,
    pub softness: f64,
}
impl Default for Params {
    fn default() -> Self {
        Self {
            ratio: 0.5,
            softness: 0.3,
        }
    }
}

pub struct Info {
    pub pattern: Pattern,
    pub id: &'static str,
    pub name: &'static str,
    /// What `ratio` controls, for the settings label.
    pub ratio_label: &'static str,
    /// Suggested tile size, mm.
    pub tile: f64,
    /// Tile height over width.
    pub aspect: f64,
}

pub const ALL: &[Info] = &[
    info(
        Pattern::KnurlStraight,
        "knurl_straight",
        "Knurling, straight",
        "Flat land",
        1.5,
        1.,
    ),
    info(
        Pattern::KnurlDiamond,
        "knurl_diamond",
        "Knurling, diamond",
        "Flat land",
        2.,
        1.,
    ),
    info(
        Pattern::Hexagons,
        "hexagons",
        "Hexagons",
        "Groove width",
        4.,
        SQRT3,
    ),
    info(
        Pattern::Diamonds,
        "diamonds",
        "Diamonds",
        "Groove width",
        5.,
        1.,
    ),
    info(Pattern::Waves, "waves", "Waves", "Wave height", 6., 1.),
    info(Pattern::Ribs, "ribs", "Ribs", "Flat land", 2., 1.),
    info(Pattern::Flutes, "flutes", "Flutes", "Ridge width", 3., 1.),
    info(Pattern::Scales, "scales", "Scales", "Overlap", 4., 1.),
    info(Pattern::Dots, "dots", "Dots", "Dot size", 3., 1.),
    info(
        Pattern::CarbonWeave,
        "carbon_weave",
        "Carbon weave",
        "Gap",
        4.,
        1.,
    ),
    info(Pattern::Twill, "twill", "Twill weave", "Gap", 6., 1.),
    info(
        Pattern::WoodGrain,
        "wood_grain",
        "Wood grain",
        "Wobble",
        12.,
        1.,
    ),
];
const SQRT3: f64 = 1.732_050_807_568_877_2;
const fn info(
    pattern: Pattern,
    id: &'static str,
    name: &'static str,
    ratio_label: &'static str,
    tile: f64,
    aspect: f64,
) -> Info {
    Info {
        pattern,
        id,
        name,
        ratio_label,
        tile,
        aspect,
    }
}
pub fn by_id(id: &str) -> Option<&'static Info> {
    ALL.iter().find(|i| i.id == id)
}

fn frac(x: f64) -> f64 {
    x - x.floor()
}
/// Distance to the nearest integer, 0–0.5.
fn to_int(x: f64) -> f64 {
    let f = frac(x);
    f.min(1. - f)
}
/// Triangle wave, 0 at integers, 1 halfway.
fn tri(x: f64) -> f64 {
    2. * to_int(x)
}
fn smoothstep(a: f64, b: f64, x: f64) -> f64 {
    let t = ((x - a) / (b - a)).clamp(0., 1.);
    t * t * (3. - 2. * t)
}
/// Rounds a 0–1 profile towards a cosine bump.
fn soften(h: f64, softness: f64) -> f64 {
    let h = h.clamp(0., 1.);
    h + (0.5 - 0.5 * (PI * h).cos() - h) * softness
}
/// A ridge with a flat top of `land` (0–1 of the ridge).
fn ridge(t: f64, land: f64) -> f64 {
    (t / (1. - land * 0.8)).min(1.)
}
fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

/// Periodic value noise on an `l`×`l` lattice over the unit tile.
fn noise(u: f64, v: f64, l: i64, seed: u64) -> f64 {
    let hash = |i: i64, j: i64| {
        let (i, j) = (i.rem_euclid(l) as u64, j.rem_euclid(l) as u64);
        let mut z = seed
            ^ (i.wrapping_mul(0x9e37_79b9_7f4a_7c15))
            ^ (j.wrapping_mul(0xc2b2_ae3d_27d4_eb4f));
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        ((z ^ (z >> 31)) >> 11) as f64 / (1u64 << 53) as f64
    };
    let (x, y) = (frac(u) * l as f64, frac(v) * l as f64);
    let (i, j) = (x.floor() as i64, y.floor() as i64);
    let (fx, fy) = (x - i as f64, y - j as f64);
    let (sx, sy) = (fx * fx * (3. - 2. * fx), fy * fy * (3. - 2. * fy));
    lerp(
        lerp(hash(i, j), hash(i + 1, j), sx),
        lerp(hash(i, j + 1), hash(i + 1, j + 1), sx),
        sy,
    )
}

/// Height 0–1 of `pattern` at tile coordinates (`u`, `v`); `v` spans the tile's height, so
/// patterns with an aspect other than 1 see a stretched unit square.
pub fn height(pattern: Pattern, u: f64, v: f64, p: Params) -> f64 {
    let (r, s) = (p.ratio.clamp(0., 1.), p.softness.clamp(0., 1.));
    let h = match pattern {
        Pattern::KnurlStraight => soften(ridge(tri(u), r * 0.6), s),
        Pattern::KnurlDiamond => soften(ridge(tri(u + v).min(tri(u - v)), r * 0.6), s),
        Pattern::Hexagons => {
            // Centres one unit apart on a hexagonal lattice spanning 1 × √3.
            let (x, y) = (frac(u), frac(v) * SQRT3);
            let mut best = f64::INFINITY;
            for j in -1..=2 {
                for i in -1..=2 {
                    for (ox, oy) in [(0., 0.), (0.5, SQRT3 / 2.)] {
                        let (cx, cy) = (i as f64 + ox, j as f64 * SQRT3 + oy);
                        let (dx, dy) = (x - cx, y - cy);
                        // Hexagon "radius" over its three edge directions (pointy top).
                        let d = dx
                            .abs()
                            .max((0.5 * dx + SQRT3 / 2. * dy).abs())
                            .max((0.5 * dx - SQRT3 / 2. * dy).abs());
                        best = best.min(d);
                    }
                }
            }
            let edge = 0.5 - best;
            let groove = 0.02 + 0.2 * r;
            soften(smoothstep(0., groove * (0.3 + s), edge), s * 0.5)
        }
        Pattern::Diamonds => {
            let e = to_int(u + v).min(to_int(u - v)) / std::f64::consts::SQRT_2;
            let groove = 0.015 + 0.12 * r;
            soften(smoothstep(0., groove * (0.3 + s), e), s * 0.5)
        }
        Pattern::Waves => {
            let a = 0.05 + 0.3 * r;
            let t = v + a * (TAU * u).sin();
            lerp(tri(t), 0.5 - 0.5 * (TAU * t).cos(), s.max(0.3))
        }
        Pattern::Ribs => soften(ridge(tri(u), r), s),
        Pattern::Flutes => {
            let x = 2. * frac(u) - 1.;
            let w = 1. - 0.6 * r;
            let x = (x / w).clamp(-1., 1.);
            let channel = 1. - (1. - x * x).max(0.).sqrt();
            soften(channel, s * 0.5)
        }
        Pattern::Scales => {
            // Rows half a tile apart, every other one shifted half a scale; later rows lie on top.
            let radius = 0.5 + 0.3 * r;
            let (x, y) = (frac(u), frac(v));
            let mut out = 0.;
            let mut top = i64::MIN;
            for j in -2i64..=4 {
                let cy = j as f64 * 0.5;
                let shift = if j.rem_euclid(2) == 1 { 0.5 } else { 0. };
                for i in -1..=2 {
                    let cx = i as f64 + shift;
                    let d = ((x - cx).powi(2) + (y - cy).powi(2)).sqrt() / radius;
                    // The lowest scale wins: rows further down lie on top, rims pointing down.
                    if d < 1. && -j > top {
                        top = -j;
                        out = d.powf(1. + 2. * s);
                    }
                }
            }
            out
        }
        Pattern::Dots => {
            let radius = 0.12 + 0.25 * r;
            let (x, y) = (frac(u), frac(v));
            let mut d = f64::INFINITY;
            for (cx, cy) in [(0., 0.), (1., 0.), (0., 1.), (1., 1.), (0.5, 0.5)] {
                d = d.min(((x - cx).powi(2) + (y - cy).powi(2)).sqrt());
            }
            let t = (d / radius).min(1.);
            let dome = (1. - t * t).max(0.).sqrt();
            let flat = 1. - smoothstep(0.85, 1., t);
            lerp(flat, dome, s)
        }
        Pattern::CarbonWeave | Pattern::Twill => {
            let n = if pattern == Pattern::CarbonWeave {
                2
            } else {
                4
            };
            let (x, y) = (frac(u) * n as f64, frac(v) * n as f64);
            let (i, j) = (x.floor() as i64, y.floor() as i64);
            let (a, b) = (x - i as f64, y - j as f64);
            let over = if n == 2 {
                (i + j) % 2 == 0
            } else {
                (i + j).rem_euclid(4) < 2
            };
            let gap = 0.05 + 0.3 * r;
            let cross = |t: f64| {
                let x = ((2. * t - 1.) / (1. - gap)).clamp(-1., 1.);
                lerp(1. - x * x, (1. - x * x).max(0.).sqrt(), s)
            };
            // Warp runs along V, weft along U; the strand on top shows its full arch.
            let (top, under) = if over {
                (cross(a) * (0.75 + 0.25 * (PI * b).sin()), cross(b))
            } else {
                (cross(b) * (0.75 + 0.25 * (PI * a).sin()), cross(a))
            };
            (0.35 + 0.65 * top).max(0.35 * under)
        }
        Pattern::WoodGrain => {
            let wobble = 0.03 + 0.12 * r;
            let n = noise(u, v, 3, 7) * 0.6 + noise(u, v, 6, 11) * 0.3 + noise(u, v, 12, 13) * 0.1;
            let x = u + wobble * (n - 0.5) * 2.;
            let lines = 9.;
            let ring = frac(x * lines);
            // Late wood: a sharp rise and a long fall, softened.
            let profile = if ring < 0.2 {
                ring / 0.2
            } else {
                1. - (ring - 0.2) / 0.8
            };
            let fine = noise(u * 1., v, 64, 17);
            soften(profile * 0.85 + fine * 0.15, s)
        }
    };
    h.clamp(0., 1.)
}

/// The pattern rasterised to a map `SIZE` px wide.
pub fn generate(info: &Info, p: Params) -> HeightMap {
    let w = SIZE;
    let h = ((SIZE as f64 * info.aspect).round() as usize).max(1);
    let mut px = Vec::with_capacity(w * h);
    for y in 0..h {
        // Row 0 is the top of the image (v near 1), as HeightMap::sample expects.
        let v = 1. - (y as f64 + 0.5) / h as f64;
        for x in 0..w {
            let u = (x as f64 + 0.5) / w as f64;
            px.push(height(info.pattern, u, v, p) as f32);
        }
    }
    HeightMap::new(w, h, px).expect("non-empty generated map")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_pattern_is_periodic_on_the_tile() {
        let p = Params::default();
        let mut seed = 0x1234_5678u64;
        let mut rnd = || {
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (seed >> 11) as f64 / (1u64 << 53) as f64
        };
        for info in ALL {
            for _ in 0..200 {
                let (u, v) = (rnd() * 0.98 + 0.01, rnd() * 0.98 + 0.01);
                let a = height(info.pattern, u, v, p);
                for (du, dv) in [(1., 0.), (0., 1.), (-2., 3.)] {
                    let b = height(info.pattern, u + du, v + dv, p);
                    assert!((a - b).abs() < 1e-6, "{} at {u},{v}+{du},{dv}", info.id);
                }
            }
        }
    }

    #[test]
    fn generated_maps_match_across_tile_edges() {
        // The jump across the wrap (last column to the first) is no larger than the map's
        // largest jump between neighbouring columns inside the tile.
        for info in ALL {
            let m = generate(info, Params::default());
            let (w, h) = (m.width(), m.height());
            let px = m.pixels();
            let mut inner = 0f32;
            let mut wrap = 0f32;
            for y in 0..h {
                for x in 0..w - 1 {
                    inner = inner.max((px[y * w + x] - px[y * w + x + 1]).abs());
                }
                wrap = wrap.max((px[y * w + w - 1] - px[y * w]).abs());
            }
            let mut inner_v = 0f32;
            let mut wrap_v = 0f32;
            for x in 0..w {
                for y in 0..h - 1 {
                    inner_v = inner_v.max((px[y * w + x] - px[(y + 1) * w + x]).abs());
                }
                wrap_v = wrap_v.max((px[(h - 1) * w + x] - px[x]).abs());
            }
            assert!(wrap <= inner + 1e-4, "{}: U wrap {wrap} > {inner}", info.id);
            assert!(
                wrap_v <= inner_v + 1e-4,
                "{}: V wrap {wrap_v} > {inner_v}",
                info.id
            );
        }
    }

    #[test]
    fn patterns_use_the_whole_range() {
        for info in ALL {
            let m = generate(info, Params::default());
            let (lo, hi) = m
                .pixels()
                .iter()
                .fold((1f32, 0f32), |(lo, hi), &v| (lo.min(v), hi.max(v)));
            assert!(hi - lo > 0.5, "{}: {lo}..{hi}", info.id);
        }
    }
}
