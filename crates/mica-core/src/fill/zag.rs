// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: orca@8500fcdcca src/libslic3r/Fill/FillRectilinear.cpp :: FillRectilinear::extended_object_bounding_box, FillRectilinear::fill_surface_by_lines
// Source: orca@8500fcdcca src/libslic3r/Fill/FillRectilinear.hpp :: FillZigZag, FillCrossZag, FillLockedZag
// Source: orca@8500fcdcca src/libslic3r/Fill/FillRectilinear.cpp :: FillLockedZag::fill_surface_locked_zag, FillLockedZag::fill_surface_extrusion
// Source: orca@8500fcdcca src/libslic3r/Flow.cpp :: Flow::new_from_config_width, Flow::auto_extrusion_width
// Source: orca@8500fcdcca src/libslic3r/Fill/Fill.cpp :: Layer::make_fills
// Source: orca@8500fcdcca src/libslic3r/PrintConfig.cpp :: PrintConfigDef::init_fff_params
// Changes: The zig-zag family's parameters as one typed settings struct; `horiz_move` is computed
// Changes: per layer here instead of in make_fills. The Y-axis mirror is applied around the fill of
// Changes: one area (the polygons are re-oriented after mirroring, as Clipper does for Orca).
// Changes: Locked Zag uses the densities and line widths of the area's own region instead of the
// Changes: layer-wide LockRegionParam maps; the skeleton is cut from the unmirrored area.

//! Orca's zig-zag infill family: **Zig Zag**, **Cross Zag** and **Locked Zag**.
//!
//! All three are rectilinear lines with a *consistent* layout: the lines are placed on a grid
//! over the object's whole (rotation-proof) bounding box and every other line is walked
//! downwards, so the zig-zags of one layer sit exactly on those two layers below.
//! Cross Zag additionally shifts the lines sideways by `infill_shift_step` per pair of layers.
//! Locked Zag prints a Cross Zag skin along the walls and a Zig Zag skeleton turned by 90° in the
//! core, reaching into the skin so the two lock together, each with its own density and width.

use std::f64::consts::FRAC_PI_2;

use super::Pattern;
use super::rectilinear::{self, LineFillParams};
use crate::config::{Config, ConfigValue, Value};
use crate::geometry::clip::{
    Join, diff_ex, intersection_ex, intersection_pl, offset_ex, union_safety_offset_ex,
};
use crate::geometry::{BoundingBox, Coord, ExPolygon, Point, Polyline};
use crate::resolve::ResolveError;
use crate::settings::FloatOrPercent;

/// Settings of the zig-zag patterns (Orca names and defaults).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ZagSettings {
    /// `infill_shift_step`, mm: Cross Zag and Locked Zag move their lines by this much per pair of layers.
    pub infill_shift_step: f64,
    /// `symmetric_infill_y_axis`: the pattern is mirrored about the object's Y axis.
    pub symmetric_infill_y_axis: bool,
    /// `skin_infill_density`, 0–1 (Locked Zag).
    pub skin_infill_density: f64,
    /// `skeleton_infill_density`, 0–1 (Locked Zag).
    pub skeleton_infill_density: f64,
    /// `skin_infill_depth`, mm (Locked Zag): width of the skin band inside the walls.
    pub skin_infill_depth: f64,
    /// `infill_lock_depth`, mm (Locked Zag): how far the skeleton reaches into the skin.
    pub infill_lock_depth: f64,
    /// `skin_infill_line_width` (Locked Zag); percentages are of the nozzle diameter.
    pub skin_infill_line_width: FloatOrPercent,
    /// `skeleton_infill_line_width` (Locked Zag); percentages are of the nozzle diameter.
    pub skeleton_infill_line_width: FloatOrPercent,
    /// Orca rules: line positions are computed in single precision, as Orca does.
    pub orca: bool,
}

impl Default for ZagSettings {
    fn default() -> Self {
        Self {
            infill_shift_step: 0.4,
            symmetric_infill_y_axis: false,
            skin_infill_density: 0.25,
            skeleton_infill_density: 0.25,
            skin_infill_depth: 2.0,
            infill_lock_depth: 1.0,
            skin_infill_line_width: FloatOrPercent::Percent(100.),
            skeleton_infill_line_width: FloatOrPercent::Percent(100.),
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
        let width = |key: &str, default: FloatOrPercent| match c.get(key) {
            Some(ConfigValue::Scalar(Value::FloatOrPercent { value, percent }))
                if value.is_finite() && *value >= 0. =>
            {
                Ok(if *percent {
                    FloatOrPercent::Percent(*value)
                } else {
                    FloatOrPercent::Mm(*value)
                })
            }
            None => Ok(default),
            _ => Err(ResolveError(format!("invalid {key}"))),
        };
        Ok(Self {
            infill_shift_step: number("infill_shift_step", d.infill_shift_step, 10.)?,
            symmetric_infill_y_axis: c
                .bool("symmetric_infill_y_axis")
                .unwrap_or(d.symmetric_infill_y_axis),
            skin_infill_density: number("skin_infill_density", 25., 100.)? / 100.,
            skeleton_infill_density: number("skeleton_infill_density", 25., 100.)? / 100.,
            skin_infill_depth: number("skin_infill_depth", d.skin_infill_depth, 100.)?,
            infill_lock_depth: number("infill_lock_depth", d.infill_lock_depth, 100.)?,
            skin_infill_line_width: width("skin_infill_line_width", d.skin_infill_line_width)?,
            skeleton_infill_line_width: width(
                "skeleton_infill_line_width",
                d.skeleton_infill_line_width,
            )?,
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

/// Orca's `FillParams::horiz_move` of a layer, scaled: Cross Zag (and Locked Zag's skin) move
/// even layers one way and odd layers the other, one step further every two layers. Zig Zag
/// with Orca rules has a zero move (Orca still places its lines in single precision).
pub(super) fn horiz_move(pattern: Pattern, layer_id: usize, zag: &ZagSettings) -> Option<f32> {
    match pattern {
        Pattern::CrossZag | Pattern::LockedZag => {
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

/// The lines of one Locked Zag area: the skeleton's first, then the skin's.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct LockedFill {
    pub skeleton: Vec<Polyline>,
    pub skin: Vec<Polyline>,
    /// The line spacing used last (a 100 % density adjusts it, as Orca's filler does).
    pub spacing: f64,
}

/// A density in Orca's single precision (`float(0.01 * percent)`), or `None` for no lines.
fn density(fraction: f64) -> Option<f64> {
    let d = f64::from(fraction.min(1.0) as f32);
    (d > 0.0001).then_some(d)
}

/// `FillLockedZag::fill_surface_locked_zag` for one area of one region: a band of
/// `skin_infill_depth` inside the area's outline is the skin, filled like Cross Zag at the skin
/// density; the rest is the skeleton, grown by `infill_lock_depth` back into the skin and filled
/// with Zig Zag lines turned by 90° at the skeleton density. `skin` carries the skin's line
/// parameters (the layer's direction and shift); the skeleton never shifts.
pub(super) fn locked(
    ex: &ExPolygon,
    skin: &LineFillParams,
    axis: Option<Coord>,
    zag: &ZagSettings,
) -> LockedFill {
    let src = std::slice::from_ref(ex);
    // `scale_` divides by SCALING_FACTOR.
    let depth = zag.skin_infill_depth / 0.000001;
    let lock = zag.infill_lock_depth / 0.000001;
    let zig = offset_ex(src, -depth, Join::Miter);
    let cross = diff_ex(src, &zig);
    // Orca collects the surfaces of every region with the same density and line width; one
    // region's own area stands for them here (see `generate_for_different_flow`).
    let region = union_safety_offset_ex(src);
    let mut out = LockedFill {
        spacing: skin.spacing,
        ..LockedFill::default()
    };
    if let Some(density) = density(zag.skeleton_infill_density) {
        let params = LineFillParams {
            density,
            // `locked_zag`: the skeleton runs at 90° to the skin.
            angle: f64::from(skin.angle as f32 + FRAC_PI_2 as f32),
            horiz_move: Some(0.0),
            ..*skin
        };
        let parts = intersection_ex(&region, &zig);
        let parts = intersection_ex(&offset_ex(&parts, lock, Join::Miter), src);
        let mut lines = Vec::new();
        for part in &parts {
            let (l, spacing) = fill_lines(
                part,
                &LineFillParams {
                    spacing: out.spacing,
                    ..params
                },
                axis,
            );
            out.spacing = spacing;
            lines.extend(l);
        }
        out.skeleton = intersection_pl(&lines, &region);
    }
    if let Some(density) = density(zag.skin_infill_density) {
        let params = LineFillParams { density, ..*skin };
        let parts = intersection_ex(&region, &cross);
        let mut lines = Vec::new();
        for part in &parts {
            let (l, spacing) = fill_lines(
                part,
                &LineFillParams {
                    spacing: out.spacing,
                    ..params
                },
                axis,
            );
            out.spacing = spacing;
            lines.extend(l);
        }
        out.skin = intersection_pl(&lines, &region);
    }
    out
}

/// Width of a Locked Zag line setting (`Flow::new_from_config_width` for sparse infill):
/// percentages are of the nozzle, 0 is Orca's automatic 112.5 % of the nozzle.
pub fn line_width(width: FloatOrPercent, nozzle_diameter: f64) -> f64 {
    let w = match width {
        FloatOrPercent::Mm(v) if v <= 0. => f64::from(1.125f32 * nozzle_diameter as f32),
        w => w.resolve(nozzle_diameter, 1.0),
    };
    f64::from(w as f32)
}

#[cfg(test)]
#[path = "zag_tests.rs"]
mod tests;
