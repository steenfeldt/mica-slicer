// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: orca@8500fcdcca src/libslic3r/Fill/FillRectilinear.cpp :: FillRectilinear::extended_object_bounding_box, FillRectilinear::fill_surface_by_lines
// Source: orca@8500fcdcca src/libslic3r/Fill/FillRectilinear.hpp :: FillZigZag, FillCrossZag
// Source: orca@8500fcdcca src/libslic3r/Fill/Fill.cpp :: Layer::make_fills
// Source: orca@8500fcdcca src/libslic3r/PrintConfig.cpp :: PrintConfigDef::init_fff_params
// Changes: The zig-zag family's parameters as one typed settings struct; `horiz_move` is computed
// Changes: per layer here instead of in make_fills. The Y-axis mirror is applied around the fill of
// Changes: one area (the polygons are re-oriented after mirroring, as Clipper does for Orca).

//! Orca's zig-zag infill family: **Zig Zag** and **Cross Zag**.
//!
//! Both are rectilinear lines with a *consistent* layout: the lines are placed on a grid
//! over the object's whole (rotation-proof) bounding box and every other line is walked
//! downwards, so the zig-zags of one layer sit exactly on those two layers below.
//! Cross Zag additionally shifts the lines sideways by `infill_shift_step` per pair of layers.

use super::Pattern;
use super::rectilinear::{self, LineFillParams};
use crate::config::Config;
use crate::geometry::{BoundingBox, Coord, ExPolygon, Point, Polyline};
use crate::resolve::ResolveError;

/// Settings of the zig-zag patterns (Orca names and defaults).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ZagSettings {
    /// `infill_shift_step`, mm: Cross Zag and Locked Zag move their lines by this much per pair of layers.
    pub infill_shift_step: f64,
    /// `symmetric_infill_y_axis`: the pattern is mirrored about the object's Y axis.
    pub symmetric_infill_y_axis: bool,
    /// Orca rules: line positions are computed in single precision, as Orca does.
    pub orca: bool,
}

impl Default for ZagSettings {
    fn default() -> Self {
        Self {
            infill_shift_step: 0.4,
            symmetric_infill_y_axis: false,
            orca: false,
        }
    }
}

impl ZagSettings {
    pub fn from_config(c: &Config) -> Result<Self, ResolveError> {
        let d = Self::default();
        let number = |key: &str, default: f64, max: f64| {
            let v = c.float(key).unwrap_or(default);
            if v.is_finite() && (0.0..=max).contains(&v) {
                Ok(v)
            } else {
                Err(ResolveError(format!("invalid {key}: expected 0 to {max}")))
            }
        };
        Ok(Self {
            infill_shift_step: number("infill_shift_step", d.infill_shift_step, 10.)?,
            symmetric_infill_y_axis: c
                .bool("symmetric_infill_y_axis")
                .unwrap_or(d.symmetric_infill_y_axis),
            orca: crate::rules::Rules::of(c).is_orca(),
        })
    }
}

/// `FillRectilinear::extended_object_bounding_box`: the object's bounding box (centred on the
/// object, as Orca's `PrintObject::bounding_box`), made square and grown by √2 so it covers the
/// object at any rotation. `bounds` is relative to the object centre.
pub(super) fn extended_object_bounding_box(bounds: BoundingBox) -> BoundingBox {
    let hx = (bounds.max.x - bounds.min.x) / 2;
    let hy = (bounds.max.y - bounds.min.y) / 2;
    let h = hx.max(hy);
    // `Point::operator*=(double)` truncates.
    let scale = |v: Coord| (v as f64 * std::f64::consts::SQRT_2) as Coord;
    BoundingBox {
        min: Point::new(scale(-h), scale(-h)),
        max: Point::new(scale(h), scale(h)),
    }
}

/// Orca's `FillParams::horiz_move` of a layer, scaled: Cross Zag moves
/// even layers one way and odd layers the other, one step further every two layers. Zig Zag
/// with Orca rules has a zero move (Orca still places its lines in single precision).
pub(super) fn horiz_move(pattern: Pattern, layer_id: usize, zag: &ZagSettings) -> Option<f32> {
    match pattern {
        Pattern::CrossZag => {
            // `scale_` divides by SCALING_FACTOR.
            let step = zag.infill_shift_step / 0.000001 * (layer_id / 2) as f64;
            Some(if layer_id.is_multiple_of(2) {
                -step
            } else {
                step
            } as f32)
        }
        _ => zag.orca.then_some(0.0),
    }
}

fn mirror(points: &mut [Point], axis: Coord) {
    for p in points {
        p.x = 2 * axis - p.x;
    }
}

/// `fill_surface_by_lines` of one area, mirrored about the vertical line `x = axis` first (and the
/// lines mirrored back) when the pattern is symmetric about the Y axis.
pub(super) fn fill_lines(
    ex: &ExPolygon,
    params: &LineFillParams,
    axis: Option<Coord>,
) -> (Vec<Polyline>, f64) {
    let Some(axis) = axis else {
        return rectilinear::fill_surface_by_lines(ex, params);
    };
    let mut ex = ex.clone();
    for poly in std::iter::once(&mut ex.contour).chain(ex.holes.iter_mut()) {
        mirror(poly, axis);
        poly.reverse();
    }
    let (mut lines, spacing) = rectilinear::fill_surface_by_lines(&ex, params);
    for line in &mut lines {
        mirror(line, axis);
    }
    (lines, spacing)
}

/// The mirror axis of a symmetric zig-zag fill: the centre of the object's extended box
/// (`symmetric_y_axis`).
pub(super) fn mirror_axis(zag: &ZagSettings, object_box: Option<BoundingBox>) -> Option<Coord> {
    zag.symmetric_infill_y_axis
        .then(|| object_box.map_or(0, |b| (b.min.x + b.max.x) / 2))
}

#[cfg(test)]
#[path = "zag_tests.rs"]
mod tests;
