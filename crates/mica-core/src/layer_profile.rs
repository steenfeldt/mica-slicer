// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: prusaslicer@b028299c77 src/libslic3r/Slicing.cpp :: SlicingParameters::create_from_config, layer_height_profile_from_ranges, layer_height_profile_adaptive, smooth_height_profile, adjust_layer_height_profile, generate_object_layers
// Source: prusaslicer@b028299c77 src/libslic3r/SlicingAdaptive.cpp :: SlicingAdaptive::prepare, SlicingAdaptive::next_layer_height
// Source: prusaslicer@b028299c77 src/libslic3r/PrintObject.cpp :: PrintObject::update_layer_height_profile, PrintObject::slicing_parameters
// Source: orca@8500fcdcca src/libslic3r/Slicing.cpp :: SlicingParameters::create_from_config
// Changes: Rust port on placed bed-coordinate meshes; only the parameters the profile needs (no raft heights or support gaps); no Z shrinkage and no Orca precise_z_height (off by default); Orca's extruder-index quirk kept behind a flag.
// Upstream attribution (Slicing.cpp, SlicingAdaptive.cpp):
// Copyright (c) Prusa Research 2016 - 2023 Vojtěch Bubník @bubnikv, Enrico Turri @enricoturri1966, David Kocík @kocikdav, Lukáš Matěna @lukasmatena, Oleksandra Iushchenko @YuSanka
// Adaptive slicing based on the work of Florens Wasserfall (@platsch).
//
// PrusaSlicer is released under the terms of the AGPLv3 or higher
//
// Changes: Engine choices read mica_slicing_rules (mica_core::rules), not gcode_flavor or the presence of Orca keys (docs/slicing-rules.md).

//! Variable layer height: the layer height profile of an object and the layers it produces.
//!
//! A profile is a flat list of `(z, height)` pairs, `z` measured from the object's bottom and
//! the last `z` at the object's top, exactly as PrusaSlicer stores `ModelObject::layer_height_profile`
//! (and writes it to `Metadata/Slic3r_PE_layer_heights_profile.txt`). Heights between two
//! entries are interpolated linearly.

use crate::config::Config;
use crate::mesh::TriangleMesh;

/// libslic3r's `EPSILON`.
const EPSILON: f64 = 1e-4;
const MIN_LAYER_HEIGHT: f64 = 0.01;
const MIN_LAYER_HEIGHT_DEFAULT: f64 = 0.07;

/// What the profile algorithms need of upstream's `SlicingParameters`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SlicingParams {
    pub layer_height: f64,
    /// The first object layer (the print's first layer, or a normal layer over a raft).
    pub first_object_layer_height: f64,
    /// False over a raft: the first object layer may then vary too.
    pub first_layer_fixed: bool,
    pub min_layer_height: f64,
    pub max_layer_height: f64,
    /// Object height in mm (top of the object above its bottom).
    pub object_height: f64,
}

/// Adjustment of [`adjust`], as upstream's `LayerHeightEditActionType`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditAction {
    /// Thicker layers in the band.
    Increase,
    /// Thinner layers in the band (more detail).
    Decrease,
    /// Back towards the base layer height.
    Reduce,
    /// Smooth the profile in the band.
    Smooth,
}

fn lerp(a: f64, b: f64, t: f64) -> f64 {
    (1. - t) * a + t * b
}

fn is_approx(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-10
}

impl SlicingParams {
    /// Parameters for an object `height` mm tall printed with `config` (Mica's PrusaSlicer
    /// option names; `mica_slicing_rules` picks PrusaSlicer or Orca semantics).
    /// `extruders` are the zero-based tools printing the object's model parts.
    pub fn from_config(c: &Config, extruders: &[usize], height: f64) -> Self {
        let orca = crate::rules::Rules::of(c).is_orca();
        let layer_height = c.float("layer_height").unwrap_or(0.2);
        let first = c
            .float("first_layer_height")
            .filter(|h| *h > 0.)
            .unwrap_or(layer_height);
        // Upstream reads these vectors at a one-based extruder number minus one (out of range,
        // as for "current tool" 0, falls back to the first entry). Orca passes the zero-based
        // object tool there, so it reads the previous tool's value (the first for tool 0).
        let min_from = |index: usize| {
            let v = c.float_at("min_layer_height", index).unwrap_or(0.);
            if v == 0. {
                MIN_LAYER_HEIGHT_DEFAULT
            } else {
                v.max(MIN_LAYER_HEIGHT)
            }
        };
        let max_from = |index: usize| {
            let min = min_from(index);
            let v = c.float_at("max_layer_height", index).unwrap_or(0.);
            let nozzle = c.float_at("nozzle_diameter", index).unwrap_or(0.4);
            min.max(if v == 0. { 0.75 * nozzle } else { v })
        };
        let object_index = |tool: usize| if orca { tool.saturating_sub(1) } else { tool };
        let raft = c.int("raft_layers").unwrap_or(0) > 0;
        let mut min = MIN_LAYER_HEIGHT;
        let mut max = f64::MAX;
        if c.bool("support_material").unwrap_or(false)
            || raft
            || c.int("support_material_enforce_layers").unwrap_or(0) > 0
        {
            let support = c.int("support_material_extruder").unwrap_or(0).max(0) as usize;
            let interface = c
                .int("support_material_interface_extruder")
                .unwrap_or(0)
                .max(0) as usize;
            let (support, interface) = (support.saturating_sub(1), interface.saturating_sub(1));
            min = min_from(support).max(min_from(interface));
            max = max_from(support).min(max_from(interface));
        }
        if extruders.is_empty() {
            min = min.max(min_from(0));
            max = max.min(max_from(0));
        } else {
            for &e in extruders {
                min = min.max(min_from(object_index(e)));
                max = max.min(max_from(object_index(e)));
            }
        }
        Self {
            layer_height,
            first_object_layer_height: if raft { layer_height } else { first },
            first_layer_fixed: !raft,
            min_layer_height: min.min(layer_height),
            max_layer_height: max.max(layer_height),
            object_height: height,
        }
    }
}

/// Whether `profile` applies to an object of `p.object_height` (upstream's check in
/// `PrintObject::update_layer_height_profile`: an even list ending at the object's top).
pub fn valid(p: &SlicingParams, profile: &[f64]) -> bool {
    profile.len() >= 2
        && profile.len().is_multiple_of(2)
        && (profile[profile.len() - 2] - p.object_height).abs() <= 1e-3
}

/// The profile of an object without one: constant layer height (and the first layer), with
/// `ranges` (`min_z`, `max_z`, layer height) sorted by their bounds.
pub fn from_ranges(p: &SlicingParams, ranges: &[(f64, f64, f64)]) -> Vec<f64> {
    let mut non_overlapping: Vec<(f64, f64, f64)> = Vec::new();
    if p.first_layer_fixed {
        non_overlapping.push((0., p.first_object_layer_height, p.first_object_layer_height));
    }
    for &(lo, hi, height) in ranges {
        let mut lo = lo;
        let hi = hi.min(p.object_height);
        if let Some(last) = non_overlapping.last() {
            lo = lo.max(last.1);
        }
        if lo + EPSILON < hi {
            non_overlapping.push((lo, hi, height));
        }
    }
    let mut profile: Vec<f64> = Vec::new();
    fn last_z(profile: &[f64]) -> f64 {
        if profile.is_empty() {
            0.
        } else {
            profile[profile.len() - 2]
        }
    }
    fn append(profile: &mut Vec<f64>, z: f64, h: f64) {
        if !profile.is_empty() {
            let n = profile.len();
            let last_z_matches = is_approx(profile[n - 2], z);
            let last_h_matches = is_approx(profile[n - 1], h);
            if last_h_matches {
                if last_z_matches {
                    return;
                }
                if n >= 4 && is_approx(profile[n - 3], h) {
                    profile[n - 2] = z;
                    return;
                }
            }
        }
        profile.push(z);
        profile.push(h);
    }
    for (lo, hi, height) in non_overlapping {
        let z = last_z(&profile);
        if lo > z + EPSILON {
            append(&mut profile, z, p.layer_height);
            append(&mut profile, lo, p.layer_height);
        }
        append(&mut profile, lo, height);
        append(&mut profile, hi, height);
    }
    let z = last_z(&profile);
    if z < p.object_height {
        append(&mut profile, z, p.layer_height);
        append(&mut profile, p.object_height, p.layer_height);
    }
    profile
}

/// The object's layers for `profile`, as (bottom, top) pairs flattened like upstream's
/// `generate_object_layers` (no raft offset).
pub fn object_layers(p: &SlicingParams, profile: &[f64]) -> Vec<f64> {
    let mut print_z = 0.;
    let mut out = Vec::new();
    if p.first_layer_fixed {
        out.push(0.);
        print_z = p.first_object_layer_height;
        out.push(print_z);
    }
    let mut idx = 0usize;
    let mut slice_z = print_z + 0.5 * p.min_layer_height;
    while slice_z < p.object_height {
        let mut height = p.min_layer_height;
        if idx < profile.len() {
            let mut next = idx + 2;
            loop {
                if next >= profile.len() || slice_z < profile[next] {
                    break;
                }
                idx = next;
                next += 2;
            }
            let (z1, h1) = (profile[idx], profile[idx + 1]);
            height = h1;
            if next < profile.len() {
                let (z2, h2) = (profile[next], profile[next + 1]);
                height = lerp(h1, h2, (slice_z - z1) / (z2 - z1));
            }
        }
        slice_z = print_z + 0.5 * height;
        if slice_z >= p.object_height {
            break;
        }
        out.push(print_z);
        print_z += height;
        slice_z = print_z + 0.5 * p.min_layer_height;
        out.push(print_z);
    }
    out
}

/// Layer tops for `profile` (each layer's print Z above the object's bottom).
pub fn tops(p: &SlicingParams, profile: &[f64]) -> Vec<f64> {
    object_layers(p, profile)
        .as_chunks::<2>()
        .0
        .iter()
        .map(|l| l[1])
        .collect()
}

/// One facet's Z span and the cosine/sine of its normal against the Z axis.
#[derive(Clone, Copy, Debug)]
struct FaceZ {
    z_span: (f32, f32),
    n_cos: f32,
    n_sin: f32,
}

fn layer_height_from_slope(face: &FaceZ, max_surface_deviation: f32) -> f32 {
    (max_surface_deviation / 0.184).min(if face.n_cos > 1e-5 {
        (1.44 * f64::from(max_surface_deviation) * f64::from(face.n_sin / face.n_cos).sqrt()) as f32
    } else {
        f32::MAX
    })
}

/// Upstream's `SlicingAdaptive`: facets sorted by their Z span.
struct Adaptive {
    params: SlicingParams,
    faces: Vec<FaceZ>,
}

impl Adaptive {
    fn new(params: SlicingParams, mesh: &TriangleMesh, bottom: f32) -> Self {
        let mut faces: Vec<FaceZ> = mesh
            .faces
            .iter()
            .map(|f| {
                let v = f.map(|i| {
                    let p = mesh.vertices[i as usize];
                    [p[0], p[1], p[2] - bottom]
                });
                let a = [v[1][0] - v[0][0], v[1][1] - v[0][1], v[1][2] - v[0][2]];
                let b = [v[2][0] - v[0][0], v[2][1] - v[0][1], v[2][2] - v[0][2]];
                let mut n = [
                    a[1] * b[2] - a[2] * b[1],
                    a[2] * b[0] - a[0] * b[2],
                    a[0] * b[1] - a[1] * b[0],
                ];
                let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
                if len > 0. {
                    n = n.map(|c| c / len);
                }
                FaceZ {
                    z_span: (
                        v[0][2].min(v[1][2]).min(v[2][2]),
                        v[0][2].max(v[1][2]).max(v[2][2]),
                    ),
                    n_cos: n[2].abs(),
                    n_sin: (n[0] * n[0] + n[1] * n[1]).sqrt(),
                }
            })
            .collect();
        faces.sort_by(|a, b| {
            a.z_span
                .0
                .total_cmp(&b.z_span.0)
                .then(a.z_span.1.total_cmp(&b.z_span.1))
        });
        Self { params, faces }
    }

    fn next_layer_height(&self, print_z: f32, quality: f32, current: &mut usize) -> f32 {
        let p = &self.params;
        let mut height = p.max_layer_height as f32;
        let (delta_min, delta_mid, delta_max) =
            (p.min_layer_height, p.layer_height, p.max_layer_height);
        let deviation = if quality < 0.5 {
            lerp(delta_min, delta_mid, 2. * f64::from(quality))
        } else {
            lerp(delta_max, delta_mid, 2. * (1. - f64::from(quality)))
        } as f32;
        let mut id = *current;
        let mut first_hit = false;
        while id < self.faces.len() {
            let span = self.faces[id].z_span;
            if span.0 >= print_z {
                break;
            }
            if span.1 > print_z {
                if !first_hit {
                    first_hit = true;
                    *current = id;
                }
                if f64::from(span.1) < f64::from(print_z) + EPSILON {
                    id += 1;
                    continue;
                }
                height = height.min(layer_height_from_slope(&self.faces[id], deviation));
            }
            id += 1;
        }
        height = height.max(p.min_layer_height as f32);
        if height > p.min_layer_height as f32 {
            while id < self.faces.len() {
                let span = self.faces[id].z_span;
                if span.0 >= print_z + height {
                    break;
                }
                if f64::from(span.1) < f64::from(print_z) + EPSILON {
                    id += 1;
                    continue;
                }
                let reduced = layer_height_from_slope(&self.faces[id], deviation);
                let z_diff = span.0 - print_z;
                if reduced < z_diff {
                    height = z_diff;
                } else if reduced < height {
                    height = reduced;
                }
                id += 1;
            }
            height = height.max(p.min_layer_height as f32);
        }
        height
    }
}

/// Upstream's "Adaptive" button: heights keeping a cusp error, `quality` 0 (fine) to 1 (fast).
/// `mesh` is the object's model parts, placed; Z is measured from `bottom`.
pub fn adaptive(p: &SlicingParams, mesh: &TriangleMesh, bottom: f32, quality: f32) -> Vec<f64> {
    let a = Adaptive::new(*p, mesh, bottom);
    let mut profile = vec![0., p.first_object_layer_height];
    if p.first_layer_fixed {
        profile.push(p.first_object_layer_height);
        profile.push(p.first_object_layer_height);
    }
    let mut print_z = p.first_object_layer_height;
    let mut current = 0usize;
    while print_z + EPSILON < p.object_height {
        let mut height = p.max_layer_height as f32;
        let cusp = a.next_layer_height(print_z as f32, quality, &mut current);
        height = cusp.min(height);
        profile.push(print_z);
        profile.push(f64::from(height));
        print_z += f64::from(height);
    }
    let z_gap = p.object_height - profile[profile.len() - 2];
    if z_gap > 0. {
        profile.push(p.object_height);
        profile.push(z_gap.clamp(p.min_layer_height, p.max_layer_height));
    }
    profile
}

/// Upstream's "Smooth" button: a biased Gaussian blur over `radius` profile points.
pub fn smooth(profile: &[f64], p: &SlicingParams, radius: u32, keep_min: bool) -> Vec<f64> {
    let skip = if p.first_layer_fixed { 4 } else { 0 };
    if (profile.len() as i64) - (skip as i64) < 6 {
        return profile.to_vec();
    }
    let radius = radius.max(1);
    let kernel: Vec<f64> = {
        let size = 2 * radius + 1;
        let sigma = 0.3 * (f64::from(radius) - 1.) + 0.8;
        let two_sq_sigma = 2. * sigma * sigma;
        let inv = 1. / (std::f64::consts::PI * two_sq_sigma).sqrt();
        (0..size)
            .map(|i| {
                let x = f64::from(i) - f64::from(radius);
                inv * (-x * x / two_sq_sigma).exp()
            })
            .collect()
    };
    let two_radius = 2 * radius as i64;
    let size = profile.len();
    let mut ret = Vec::with_capacity(size);
    ret.extend_from_slice(&profile[..skip]);
    let delta_h = p.max_layer_height - p.min_layer_height;
    let inv_delta_h = if delta_h != 0. { 1. / delta_h } else { 1. };
    let max_dz_band = f64::from(radius) * p.layer_height;
    let mut i = skip;
    while i < size {
        let zi = profile[i];
        let hi = profile[i + 1];
        ret.push(zi);
        let begin = (i as i64 - two_radius).max(skip as i64);
        let end = (i as i64 + two_radius).min(size as i64 - 2);
        let mut height = 0.;
        let mut weight_total = 0.;
        let mut j = begin;
        while j <= end {
            let kernel_id = (i64::from(radius) + (j - i as i64) / 2) as usize;
            let dz = (zi - profile[j as usize]).abs();
            if dz * p.layer_height <= max_dz_band {
                let dh = (p.max_layer_height - profile[j as usize + 1]).abs();
                let weight = kernel[kernel_id] * (dh * inv_delta_h).sqrt();
                height += weight * profile[j as usize + 1];
                weight_total += weight;
            }
            j += 2;
        }
        height = if weight_total == 0. {
            hi
        } else {
            height / weight_total
        }
        .clamp(p.min_layer_height, p.max_layer_height);
        if keep_min {
            height = height.min(hi);
        }
        ret.push(height);
        i += 2;
    }
    ret
}

/// Upstream's interactive edit: change the profile around `z` within `band_width` by
/// `delta` (mm per step; upstream steps 0.005 mm per mouse event).
pub fn adjust(
    p: &SlicingParams,
    profile: &mut Vec<f64>,
    z: f64,
    delta: f64,
    band_width: f64,
    action: EditAction,
) {
    let span = (
        if p.first_layer_fixed {
            p.first_object_layer_height
        } else {
            0.
        },
        p.object_height,
    );
    if z < span.0 || z > span.1 || profile.len() < 4 {
        return;
    }
    // 1) The current layer thickness at z.
    let mut current = p.layer_height;
    let mut i = 0;
    while i < profile.len() {
        if i + 2 == profile.len() {
            current = profile[i + 1];
            break;
        } else if profile[i + 2] > z {
            let (z1, h1, z2, h2) = (profile[i], profile[i + 1], profile[i + 2], profile[i + 3]);
            current = lerp(h1, h2, (z - z1) / (z2 - z1));
            break;
        }
        i += 2;
    }
    // 2) Can the delta be applied?
    let mut delta = delta;
    match action {
        EditAction::Increase | EditAction::Decrease => {
            if action == EditAction::Decrease {
                delta = -delta;
            }
            if delta > 0. {
                if current >= p.max_layer_height - EPSILON {
                    return;
                }
                delta = delta.min(p.max_layer_height - current);
            } else {
                if current <= p.min_layer_height + EPSILON {
                    return;
                }
                delta = delta.max(p.min_layer_height - current);
            }
        }
        EditAction::Reduce | EditAction::Smooth => {
            delta = delta.abs().min((p.layer_height - current).abs());
            if delta < EPSILON {
                return;
            }
        }
    }
    // 3) Densify the profile inside z ± band_width/2.
    let lo = span.0.max(z - 0.5 * band_width);
    let hi = z + 0.5 * band_width;
    let z_step = 0.1;
    let mut idx = 0usize;
    while idx < profile.len() && profile[idx] < lo {
        idx += 2;
    }
    idx = idx.saturating_sub(2);
    let mut new: Vec<f64> = Vec::with_capacity(profile.len());
    new.extend_from_slice(&profile[..idx + 2]);
    let mut zz = lo;
    let mut resampled_start = new.len();
    while zz < hi {
        let next = idx + 2;
        let (z1, h1) = (profile[idx], profile[idx + 1]);
        let mut height = h1;
        if next < profile.len() {
            let (z2, h2) = (profile[next], profile[next + 1]);
            height = lerp(h1, h2, (zz - z1) / (z2 - z1));
        }
        let weight = if (zz - z).abs() < 0.5 * band_width {
            0.5 + 0.5 * (2. * std::f64::consts::PI * (zz - z) / band_width).cos()
        } else {
            0.
        };
        match action {
            EditAction::Increase | EditAction::Decrease => height += weight * delta,
            EditAction::Reduce => {
                let d = height - p.layer_height;
                let step = weight * delta;
                height += if d.abs() > step {
                    if d > 0. { -step } else { step }
                } else {
                    -d
                };
            }
            EditAction::Smooth => {}
        }
        height = height.clamp(p.min_layer_height, p.max_layer_height);
        if zz == span.1 {
            if new[new.len() - 2] + EPSILON > zz {
                new.pop();
                new.pop();
            }
            new.push(zz);
            new.push(height);
            idx = profile.len();
            break;
        }
        if new[new.len() - 2] + EPSILON < zz {
            new.push(zz);
            new.push(height);
        }
        zz = (zz + z_step).min(span.1);
        idx = next;
        while idx < profile.len() && profile[idx] < zz {
            idx += 2;
        }
        idx = idx.saturating_sub(2);
    }
    idx += 2;
    let mut resampled_end = new.len();
    if idx < profile.len() {
        new.extend_from_slice(&profile[idx..]);
    } else if new[new.len() - 2] + 0.5 * EPSILON < span.1 {
        new.extend_from_slice(&profile[profile.len() - 2..]);
    }
    *profile = new;
    if action == EditAction::Smooth {
        if resampled_start == 0 {
            resampled_start += 1;
        }
        if resampled_end == profile.len() {
            resampled_end -= 2;
        }
        for _ in 0..6 {
            let old = profile.clone();
            let mut i = resampled_start;
            while i < resampled_end {
                let zz = old[i];
                let t = if (zz - z).abs() < 0.5 * band_width {
                    0.25 + 0.25 * (2. * std::f64::consts::PI * (zz - z) / band_width).cos()
                } else {
                    0.
                };
                profile[i + 1] = if i == 0 {
                    (1. - t) * old[i + 1] + t * old[i + 3]
                } else if i + 1 == old.len() {
                    (1. - t) * old[i + 1] + t * old[i - 1]
                } else {
                    (1. - t) * old[i + 1] + 0.5 * t * (old[i - 1] + old[i + 3])
                };
                i += 2;
            }
        }
    }
}

/// The layer height at `z` (mm above the object's bottom), interpolated in `profile`.
pub fn height_at(profile: &[f64], z: f64) -> Option<f64> {
    if profile.len() < 4 {
        return profile.get(1).copied();
    }
    let mut i = 0;
    while i + 2 < profile.len() {
        let (z1, z2) = (profile[i], profile[i + 2]);
        if z <= z2 {
            let dz = z2 - z1;
            return Some(if dz != 0. {
                lerp(
                    profile[i + 1],
                    profile[i + 3],
                    ((z - z1) / dz).clamp(0., 1.),
                )
            } else {
                profile[i + 3]
            });
        }
        i += 2;
    }
    profile.last().copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(height: f64) -> SlicingParams {
        SlicingParams {
            layer_height: 0.2,
            first_object_layer_height: 0.2,
            first_layer_fixed: true,
            min_layer_height: 0.07,
            max_layer_height: 0.3,
            object_height: height,
        }
    }

    #[test]
    fn constant_profile_gives_constant_layers() {
        let p = params(20.);
        let profile = from_ranges(&p, &[]);
        // Upstream merges the equal first-layer step into the constant run.
        assert_eq!(profile, vec![0., 0.2, 20., 0.2]);
        let tops = tops(&p, &profile);
        assert_eq!(tops.len(), 100);
        assert!((tops[99] - 20.).abs() < 1e-9);
    }

    #[test]
    fn ranges_profile_steps() {
        let p = params(10.);
        let profile = from_ranges(&p, &[(2., 4., 0.1)]);
        assert_eq!(
            profile,
            vec![0., 0.2, 2., 0.2, 2., 0.1, 4., 0.1, 4., 0.2, 10., 0.2]
        );
        let tops = tops(&p, &profile);
        let thin = tops
            .windows(2)
            .filter(|w| (w[1] - w[0] - 0.1).abs() < 1e-6)
            .count();
        assert!((19..=21).contains(&thin), "{thin}");
    }

    #[test]
    fn linear_ramp_interpolates() {
        let p = params(10.);
        let profile = vec![0., 0.2, 0.2, 0.2, 10., 0.1];
        let t = tops(&p, &profile);
        let h: Vec<f64> = t.windows(2).map(|w| w[1] - w[0]).collect();
        assert!(h.windows(2).all(|w| w[1] <= w[0] + 1e-12));
        assert!(h.last().unwrap() < &0.11);
    }

    #[test]
    fn adjust_thins_a_band_and_keeps_the_ends() {
        let p = params(20.);
        let mut profile = from_ranges(&p, &[]);
        for _ in 0..20 {
            adjust(&p, &mut profile, 10., 0.005, 2., EditAction::Decrease);
        }
        assert_eq!(profile[0], 0.);
        assert!((profile[profile.len() - 2] - 20.).abs() < 1e-9);
        assert!(valid(&p, &profile));
        let at = height_at(&profile, 10.).unwrap();
        assert!((at - 0.1).abs() < 1e-6, "{at}");
        assert!((height_at(&profile, 5.).unwrap() - 0.2).abs() < 1e-9);
        // Reduce goes back to the base height.
        for _ in 0..40 {
            adjust(&p, &mut profile, 10., 0.005, 2., EditAction::Reduce);
        }
        assert!((height_at(&profile, 10.).unwrap() - 0.2).abs() < 1e-6);
        // Increase is limited to the maximum.
        for _ in 0..100 {
            adjust(&p, &mut profile, 10., 0.005, 2., EditAction::Increase);
        }
        assert!((height_at(&profile, 10.).unwrap() - 0.3).abs() < 1e-6);
        for _ in 0..10 {
            adjust(&p, &mut profile, 10., 0.005, 2., EditAction::Smooth);
        }
        assert!(height_at(&profile, 10.).unwrap() < 0.3);
        // Edits below the fixed first layer are ignored.
        let before = profile.clone();
        adjust(&p, &mut profile, 0.1, 0.005, 2., EditAction::Decrease);
        assert_eq!(before, profile);
    }

    #[test]
    fn adaptive_on_a_sphere_like_cone_varies() {
        // A cone: slopes everywhere, so heights lie strictly between the limits.
        let mut mesh = TriangleMesh::default();
        let n = 32;
        mesh.vertices.push([0., 0., 10.]);
        for i in 0..n {
            let a = std::f32::consts::TAU * i as f32 / n as f32;
            mesh.vertices.push([10. * a.cos(), 10. * a.sin(), 0.]);
        }
        mesh.vertices.push([0., 0., 0.]);
        for i in 0..n as u32 {
            let (a, b) = (1 + i, 1 + (i + 1) % n as u32);
            mesh.faces.push([0, a, b]);
            mesh.faces.push([n as u32 + 1, b, a]);
        }
        let p = params(10.);
        let fine = adaptive(&p, &mesh, 0., 0.);
        let fast = adaptive(&p, &mesh, 0., 1.);
        assert!(valid(&p, &fine) && valid(&p, &fast));
        assert!(fine.len() > fast.len());
        assert!(
            fine.chunks(2)
                .skip(2)
                .all(|c| c[1] >= p.min_layer_height - 1e-9 && c[1] <= p.max_layer_height + 1e-9)
        );
        let smoothed = smooth(&fine, &p, 5, false);
        assert_eq!(smoothed.len(), fine.len());
        let kept = smooth(&fine, &p, 5, true);
        assert!(
            kept.chunks(2)
                .zip(fine.chunks(2))
                .all(|(a, b)| a[1] <= b[1] + 1e-12)
        );
    }

    #[test]
    fn invalid_profile_is_detected() {
        let p = params(20.);
        assert!(!valid(&p, &[0., 0.2, 10., 0.2]));
        assert!(!valid(&p, &[0., 0.2, 20.]));
        assert!(valid(&p, &[0., 0.2, 20.0005, 0.2]));
    }
}
