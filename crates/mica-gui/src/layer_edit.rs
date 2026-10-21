// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/slic3r/GUI/GLCanvas3D.cpp :: GLCanvas3D::LayersEditing::render_overlay, GLCanvas3D::LayersEditing::adjust_layer_height_profile, GLCanvas3D::_perform_layer_editing_action
// Source: prusaslicer@b028299c77 src/libslic3r/Slicing.cpp :: generate_layer_height_texture
// Changes: Edits come from pointer drags (on the object or the graph) instead of a repeating timer; the colour gradient is a Z-banded overlay mesh instead of a texture shader; same palette, band width limits and Adaptive / Smooth / Reset semantics.
// Upstream attribution (GLCanvas3D.cpp, Slicing.cpp):
// Copyright (c) Prusa Research 2016 - 2023 Vojtěch Bubník @bubnikv, Enrico Turri @enricoturri1966, Lukáš Matěna @lukasmatena, Oleksandra Iushchenko @YuSanka
//
// PrusaSlicer is released under the terms of the AGPLv3 or higher
//

//! The variable layer height tool: the settings of its card, the profile of the selected
//! object, the edits and the layer-height colour gradient drawn over the object.
//!
//! The profile lives on `scene::Object::layer_profile` (an undo snapshot carries it) and goes
//! to the slicer through `Object::placed_region`. An object without one edits the profile
//! its plate's layer height and height ranges give (upstream's `layer_height_profile_from_ranges`).

use crate::scene::Object;
use crate::scene::paint::{PaintVertex, Pattern};
use glam::Vec3;
use mica_core::config::Config;
use mica_core::layer_profile::{self as lp, EditAction, SlicingParams};

/// Band width limits and default (upstream: 2 mm, the mouse wheel scales it within 1.5–10 mm).
pub const BAND_MIN: f32 = 1.5;
pub const BAND_MAX: f32 = 10.;
/// Layer height change per dragged pixel. Upstream steps 0.005 mm per mouse event while the
/// button is held; a drag of 20 px makes one such step here, 200 px about 0.05 mm.
pub const MM_PER_PIXEL: f64 = 0.00025;

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    /// Width of the edited band (mm).
    pub band_width: f32,
    /// Adaptive quality: 0 = fine detail, 1 = fast (upstream's "Quality / Speed").
    pub quality: f32,
    /// Smoothing radius in profile points (1–10) and whether thin layers stay thin.
    pub smooth_radius: u32,
    pub keep_min: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            band_width: 2.,
            quality: 0.5,
            smooth_radius: 5,
            keep_min: false,
        }
    }
}

impl Settings {
    pub fn set_band_width(&mut self, w: f32) {
        self.band_width = w.clamp(BAND_MIN, BAND_MAX);
    }
}

/// What a drag does, from the modifiers: upstream's left / right / Shift+left / Shift+right
/// buttons become the drag direction, Shift and Alt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Thicker layers in the band (dragging up, or right in the graph).
    Thicker,
    /// Thinner layers, more detail (dragging down, or left in the graph).
    Thinner,
    /// Shift: back towards the base layer height.
    Base,
    /// Alt: smooth the band.
    Smooth,
}

impl Mode {
    /// The mode of a drag step: `amount` > 0 thickens; Shift resets, Alt smooths.
    pub fn of(amount: f64, shift: bool, alt: bool) -> Self {
        if alt {
            Mode::Smooth
        } else if shift {
            Mode::Base
        } else if amount >= 0. {
            Mode::Thicker
        } else {
            Mode::Thinner
        }
    }
    fn action(self) -> EditAction {
        match self {
            Mode::Thicker => EditAction::Increase,
            Mode::Thinner => EditAction::Decrease,
            Mode::Base => EditAction::Reduce,
            Mode::Smooth => EditAction::Smooth,
        }
    }
}

/// Height of the object (its top above its lowest point), what the profile spans.
pub fn object_height(o: &Object) -> f64 {
    let (min, max) = o.bounds();
    f64::from(max.z - min.z)
}

/// The slicing parameters of `o` under the plate configuration `config`.
pub fn params(config: &Config, o: &Object) -> SlicingParams {
    let c = crate::worker::object_config(config, o).unwrap_or_else(|_| config.clone());
    let mut tools: Vec<usize> = o.tools.0[..3].iter().flatten().copied().collect();
    tools.sort_unstable();
    tools.dedup();
    SlicingParams::from_config(&c, &tools, object_height(o))
}

/// Whether the object's own profile still fits its height (a scaled or turned object's does
/// not; the slicer then ignores it, as upstream does).
pub fn fits(p: &SlicingParams, o: &Object) -> bool {
    o.layer_profile.is_empty() || lp::valid(p, &o.layer_profile)
}

/// The profile the tool shows and edits: the object's own, or the one its layer height and
/// height ranges give.
pub fn profile(p: &SlicingParams, o: &Object) -> Vec<f64> {
    if !o.layer_profile.is_empty() && lp::valid(p, &o.layer_profile) {
        return o.layer_profile.clone();
    }
    let mut ranges: Vec<(f64, f64, f64)> = o
        .height_ranges
        .iter()
        .filter_map(|r| {
            let h = r.overrides.get("layer_height")?.parse::<f64>().ok()?;
            Some((f64::from(r.min_z), f64::from(r.max_z), h))
        })
        .collect();
    ranges.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
    lp::from_ranges(p, &ranges)
}

/// One drag step at `z` (mm above the object's bottom); `amount` in mm of layer height.
/// Returns whether the profile changed.
pub fn edit(p: &SlicingParams, o: &mut Object, z: f64, amount: f64, band: f32, mode: Mode) -> bool {
    let mut profile = profile(p, o);
    let before = profile.clone();
    lp::adjust(
        p,
        &mut profile,
        z,
        amount.abs(),
        f64::from(band),
        mode.action(),
    );
    if profile == before {
        return false;
    }
    o.layer_profile = profile;
    true
}

/// "Adaptive": upstream's cusp-height profile for the object's model parts.
pub fn adaptive(p: &SlicingParams, o: &mut Object, quality: f32) {
    let mesh = o.placed_mesh();
    let bottom = o.bounds().0.z;
    o.layer_profile = lp::adaptive(p, &mesh, bottom, quality.clamp(0., 1.));
}

/// "Smooth" over the whole profile.
pub fn smooth(p: &SlicingParams, o: &mut Object, radius: u32, keep_min: bool) {
    let profile = profile(p, o);
    o.layer_profile = lp::smooth(&profile, p, radius.clamp(1, 10), keep_min);
}

/// Layer count and the thinnest / thickest layer the profile prints.
pub fn summary(p: &SlicingParams, profile: &[f64]) -> (usize, f64, f64) {
    let layers = lp::object_layers(p, profile);
    let heights = layers
        .chunks(2)
        .filter(|l| l.len() == 2)
        .map(|l| l[1] - l[0]);
    let (mut lo, mut hi) = (f64::INFINITY, 0f64);
    let mut n = 0;
    for h in heights {
        n += 1;
        lo = lo.min(h);
        hi = hi.max(h);
    }
    (n, if n == 0 { 0. } else { lo }, hi)
}

/// Upstream's layer-height palette (ColorBrewer, green = thin … red = thick).
const PALETTE: [[u8; 3]; 8] = [
    [0x1A, 0x98, 0x50],
    [0x66, 0xBD, 0x63],
    [0xA6, 0xD9, 0x6A],
    [0xD9, 0xF1, 0xEB],
    [0xFE, 0xE6, 0xEB],
    [0xFD, 0xAE, 0x61],
    [0xF4, 0x6D, 0x43],
    [0xD7, 0x30, 0x27],
];

/// The colour of a layer `h` mm thick (upstream's `generate_layer_height_texture` mapping).
pub fn color(p: &SlicingParams, h: f64) -> [f32; 3] {
    let mut hscale =
        2. * (p.max_layer_height - p.layer_height).max(p.layer_height - p.min_layer_height);
    if hscale == 0. {
        hscale = p.layer_height;
    }
    let n = PALETTE.len() - 1;
    let idxf = (0.5 * hscale + (h - p.layer_height)) * n as f64 / hscale;
    let i1 = (idxf.floor() as i64).clamp(0, n as i64) as usize;
    let i2 = (i1 + 1).min(n);
    let t = (idxf - i1 as f64).clamp(0., 1.) as f32;
    std::array::from_fn(|c| {
        let a = f32::from(PALETTE[i1][c]);
        let b = f32::from(PALETTE[i2][c]);
        (a + (b - a) * t) / 255.
    })
}

/// Height of the bands the overlay splits large facets into (mm).
const BAND: f64 = 0.4;
/// The most slabs the overlay splits an object's facets into; past it the bands get taller.
/// GUI monkey test, seed 112: a model scaled to 1 km with the tool open split every wall into
/// millions of 0.4 mm slabs on the UI thread (no frame for minutes, gigabytes of vertices).
const MAX_SLABS: f64 = 1_000_000.;

/// The gradient over the object (mesh-local, `Pattern::Solid`): every facet coloured by the
/// layer height at its height, facets taller than a band split along horizontal planes.
pub fn overlay_vertices(p: &SlicingParams, o: &Object, profile: &[f64]) -> Vec<PaintVertex> {
    let t = o.transform();
    let bottom = f64::from(o.bounds().0.z);
    let world_z = |v: [f32; 3]| f64::from(t.transform_point3(Vec3::from(v)).z) - bottom;
    // Colour per band of the object's height, looked up instead of per vertex.
    let bands = ((p.object_height / (BAND / 4.)).ceil() as usize).clamp(1, 20_000);
    let step = p.object_height.max(1e-6) / bands as f64;
    let table: Vec<[f32; 3]> = (0..=bands)
        .map(|i| {
            let z = i as f64 * step;
            color(p, lp::height_at(profile, z).unwrap_or(p.layer_height))
        })
        .collect();
    let colour_at = |z: f64| table[((z / step).round().max(0.) as usize).min(bands)];
    let extent = |f: &[u32; 3]| {
        let z = f.map(|i| world_z(o.mesh.vertices[i as usize]));
        z[0].max(z[1]).max(z[2]) - z[0].min(z[1]).min(z[2])
    };
    let total: f64 = o.mesh.faces.iter().map(extent).sum();
    let band = BAND.max(total / MAX_SLABS);
    let mut out = Vec::with_capacity(o.mesh.faces.len() * 3);
    let mut push = |v: [f32; 3], z: f64| {
        let c = colour_at(z);
        out.push(PaintVertex {
            position: v,
            color: [c[0], c[1], c[2], 1.],
            pattern: Pattern::Solid as u32,
        });
    };
    for f in &o.mesh.faces {
        let v = f.map(|i| o.mesh.vertices[i as usize]);
        let z = v.map(world_z);
        let lo = z[0].min(z[1]).min(z[2]);
        let hi = z[0].max(z[1]).max(z[2]);
        if hi - lo <= band || !(hi - lo).is_finite() {
            for k in 0..3 {
                push(v[k], z[k]);
            }
            continue;
        }
        // Clip the facet into slabs [k·band, (k+1)·band], fan-triangulated.
        let first = (lo / band).floor() as i64;
        let last = (hi / band).ceil() as i64;
        for k in first..last {
            let (z0, z1) = (k as f64 * band, (k + 1) as f64 * band);
            let mut poly: Vec<([f32; 3], f64)> = (0..3).map(|i| (v[i], z[i])).collect();
            for (plane, keep_above) in [(z0, true), (z1, false)] {
                let inside = |zz: f64| if keep_above { zz >= plane } else { zz <= plane };
                let mut next = Vec::with_capacity(poly.len() + 2);
                for i in 0..poly.len() {
                    let (a, za) = poly[i];
                    let (b, zb) = poly[(i + 1) % poly.len()];
                    if inside(za) {
                        next.push((a, za));
                    }
                    if inside(za) != inside(zb) {
                        let s = ((plane - za) / (zb - za)) as f32;
                        let at = std::array::from_fn(|c| a[c] + (b[c] - a[c]) * s);
                        next.push((at, plane));
                    }
                }
                poly = next;
                if poly.len() < 3 {
                    break;
                }
            }
            for i in 1..poly.len().saturating_sub(1) {
                for (q, zq) in [poly[0], poly[i], poly[i + 1]] {
                    push(q, zq);
                }
            }
        }
    }
    out
}

/// A key for the overlay cache: the object, its placement and the profile.
pub fn overlay_key(o: &Object, profile: &[f64], p: &SlicingParams) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    o.id.hash(&mut h);
    for v in o.transform().to_cols_array() {
        v.to_bits().hash(&mut h);
    }
    for v in profile {
        v.to_bits().hash(&mut h);
    }
    for v in [p.layer_height, p.min_layer_height, p.max_layer_height] {
        v.to_bits().hash(&mut h);
    }
    // Distinct from the paint overlay keys (`scene::paint::overlay_key`) by a tag.
    "layer-heights".hash(&mut h);
    h.finish()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn cube() -> Object {
        let mut mesh = mica_core::mesh::TriangleMesh::load_stl(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/models/cube20.stl"),
        )
        .unwrap();
        mesh.place_on_bed(100., 100.);
        Object::new(mica_core::input::InputObject::plain(
            "cube",
            0,
            0,
            mesh,
            mica_core::print_job::ToolAssignments([Some(0); 5]),
            Config::defaults(),
        ))
    }
    fn config() -> Config {
        let mut c = Config::defaults();
        c.set_raw("layer_height", "0.2").unwrap();
        c.set_raw("first_layer_height", "0.2").unwrap();
        c
    }

    #[test]
    fn edits_adaptive_smooth_and_reset_on_an_object() {
        let c = config();
        let mut o = cube();
        let p = params(&c, &o);
        assert!((p.object_height - 20.).abs() < 1e-4);
        // Without a profile the tool edits the constant one.
        assert_eq!(profile(&p, &o), vec![0., 0.2, p.object_height, 0.2]);
        for _ in 0..40 {
            edit(&p, &mut o, 10., 0.005, 2., Mode::Thinner);
        }
        assert!(!o.layer_profile.is_empty());
        assert!(lp::height_at(&o.layer_profile, 10.).unwrap() < 0.1);
        assert!(lp::valid(&p, &o.layer_profile));
        let (n, lo, hi) = summary(&p, &o.layer_profile);
        assert!(n > 100 && lo < 0.1 && (hi - 0.2).abs() < 1e-9);
        // Thin at the band, base colour elsewhere: the gradient differs along Z.
        let v = overlay_vertices(&p, &o, &o.layer_profile);
        assert!(v.len() > o.mesh.faces.len() * 3, "large facets are split");
        let greens = v.iter().filter(|v| v.color[1] > v.color[0] + 0.2).count();
        assert!(greens > 0);
        smooth(&p, &mut o, 5, false);
        assert!(lp::valid(&p, &o.layer_profile));
        adaptive(&p, &mut o, 0.5);
        assert!(lp::valid(&p, &o.layer_profile));
        // A cube's walls are vertical: adaptive prints the thickest layers.
        assert!(
            o.layer_profile
                .chunks(2)
                .skip(2)
                .all(|c| c[1] >= 0.2 - 1e-9)
        );
        // Scaling the object leaves the profile unfitting.
        o.scale = Vec3::new(1., 1., 1.5);
        let p2 = params(&c, &o);
        assert!(!fits(&p2, &o));
        assert_eq!(profile(&p2, &o).len(), 4);
    }

    /// GUI monkey test, seed 112: the overlay of a model scaled to 1 km stays small.
    #[test]
    fn the_overlay_of_a_huge_object_stays_bounded() {
        let c = config();
        let mut o = cube();
        o.scale = Vec3::new(1., 1., 50_000.);
        let p = params(&c, &o);
        assert!(p.object_height > 999_000.);
        let started = std::time::Instant::now();
        let v = overlay_vertices(&p, &o, &profile(&p, &o));
        assert!(v.len() < 20_000_000, "{} vertices", v.len());
        assert!(v.len() > o.mesh.faces.len() * 3, "still split into bands");
        assert!(started.elapsed().as_secs() < 10, "{:?}", started.elapsed());
    }

    #[test]
    fn palette_ends_and_modes() {
        let p = SlicingParams {
            layer_height: 0.2,
            first_object_layer_height: 0.2,
            first_layer_fixed: true,
            min_layer_height: 0.07,
            max_layer_height: 0.3,
            object_height: 10.,
        };
        let thin = color(&p, 0.07);
        let thick = color(&p, 0.3);
        assert!(thin[1] > thin[0], "thin layers are green");
        assert!(thick[0] > thick[1], "thick layers are red");
        assert_eq!(Mode::of(1., false, false), Mode::Thicker);
        assert_eq!(Mode::of(-1., false, false), Mode::Thinner);
        assert_eq!(Mode::of(-1., true, false), Mode::Base);
        assert_eq!(Mode::of(1., true, true), Mode::Smooth);
        let mut s = Settings::default();
        s.set_band_width(50.);
        assert_eq!(s.band_width, BAND_MAX);
    }
}
