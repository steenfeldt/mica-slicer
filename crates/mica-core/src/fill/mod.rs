// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: prusaslicer@b028299c77 src/libslic3r/Fill/FillBase.cpp :: Fill::fill_surface, Fill::_infill_direction
// Source: prusaslicer@b028299c77 src/libslic3r/Fill/FillBase.hpp :: FillParams
// Source: prusaslicer@b028299c77 src/libslic3r/Fill/FillRectilinear.cpp :: FillMonotonic::fill_surface, FillMonotonicLines::fill_surface, FillGrid::fill_surface
// Source: prusaslicer@b028299c77 src/libslic3r/Fill/FillRectilinear.cpp :: FillTriangles::fill_surface, FillStars::fill_surface, FillCubic::fill_surface
// Source: prusaslicer@b028299c77 src/libslic3r/Fill/FillHoneycomb.hpp :: FillHoneycomb::_layer_angle
// Changes: One entry point instead of the Fill class hierarchy. The object centre is passed in and the area
// Changes: is moved so that it sits at the origin, like PrusaSlicer's object coordinates. Bridge filling is
// Changes: in bridge/; narrow internal solids use ensuring.rs and the existing Arachne implementation.
// Source: prusaslicer@b028299c77 src/libslic3r/Fill/FillBase.cpp :: Fill::connect_infill
// Changes: Unconnected clipped lines are chained with chain_polylines (dont_connect); before
// Changes: connect_infill they are still pre-ordered greedily (upstream passes them unchained).

//! Infill patterns.
//!
//! [`fill`] fills ExPolygons (the area inside the innermost wall, including the infill/wall overlap)
//! with one of PrusaSlicer's patterns:
//!
//! - **rectilinear**: parallel lines, alternating 90° per layer, linked into zig-zags along the boundary;
//! - **monotonic** / **monotoniclines**: the same lines, printed strictly in one direction so neighbouring
//!   lines are always laid next to a line printed just before (smooth top surfaces). `monotoniclines`
//!   doesn't link the lines along the boundary;
//! - **grid**, **triangles**, **stars**, **cubic**: two or three crossing sweeps of straight lines, anchored
//!   to the boundary by [`connect`];
//! - **gyroid**, **concentric**, **honeycomb**, **3dhoneycomb**;
//! - **hilbertcurve**, **archimedeanchords**, **octagramspiral**;
//! - Orca **crosshatch** (single-line);
//! - **zigzag** (PrusaSlicer and Orca) and Orca **crosszag** ([`zag`]).
//!
//! [`toolpaths`] additionally handles variable-width solid concentric and ensuring fill.
//! Adaptive cubic, support cubic and lightning require [`PreparedFill`], created once by
//! [`crate::print::prepare_object`] and shared by all layer batches. Passing these patterns
//! to [`fill`] without an object context panics.

pub mod adaptive;
pub mod lightning;
mod object;
pub(crate) mod orca_solid;
pub use object::{FillCollection, PreparedFill, collections, toolpaths};
mod base;
mod concentric;
mod connect;
mod crosshatch;
pub(crate) mod ensuring;
mod ensuring_filter;
mod gyroid;
mod honeycomb;
mod plane_path;
mod rectilinear;
mod zag;
pub use zag::ZagSettings;

use std::f64::consts::{FRAC_PI_2, PI};

use crate::geometry::clip::{Join, offset_ex};
use crate::geometry::{ExPolygon, Point, Polyline, SCALE};

use rectilinear::{LineFillParams, Sweep};

/// Infill patterns, named as in PrusaSlicer's `fill_pattern` option.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Pattern {
    Rectilinear,
    AlignedRectilinear,
    Monotonic,
    MonotonicLines,
    Grid,
    Triangles,
    Stars,
    Cubic,
    Gyroid,
    Concentric,
    Honeycomb,
    Honeycomb3D,
    HilbertCurve,
    ArchimedeanChords,
    OctagramSpiral,
    CrossHatch,
    AdaptiveCubic,
    SupportCubic,
    Lightning,
    /// Arachne-assisted internal solid fill (Prusa default).
    Ensuring,
    /// Sparse support lines with fixed layer angle and boundary anchoring.
    SupportBase,
    /// Rectilinear lines laid out consistently across layers (PrusaSlicer and Orca `zigzag`).
    ZigZag,
    /// Orca `crosszag`: Zig Zag shifted sideways every two layers.
    CrossZag,
}

impl Pattern {
    /// Parses a PrusaSlicer `fill_pattern` value; `None` for patterns Mica doesn't have yet.
    pub fn from_config_name(name: &str) -> Option<Self> {
        Some(match name {
            "rectilinear" => Pattern::Rectilinear,
            "alignedrectilinear" => Pattern::AlignedRectilinear,
            "monotonic" => Pattern::Monotonic,
            "monotoniclines" => Pattern::MonotonicLines,
            "grid" => Pattern::Grid,
            "triangles" => Pattern::Triangles,
            "stars" => Pattern::Stars,
            "cubic" => Pattern::Cubic,
            "gyroid" => Pattern::Gyroid,
            "concentric" => Self::Concentric,
            "honeycomb" => Self::Honeycomb,
            "3dhoneycomb" => Self::Honeycomb3D,
            "hilbertcurve" => Self::HilbertCurve,
            "archimedeanchords" => Self::ArchimedeanChords,
            "octagramspiral" => Self::OctagramSpiral,
            "crosshatch" => Self::CrossHatch,
            "adaptivecubic" => Self::AdaptiveCubic,
            "supportcubic" => Self::SupportCubic,
            "lightning" => Self::Lightning,
            "ensuring" => Self::Ensuring,
            "monotonicline" => Self::MonotonicLines,
            "zigzag" => Self::ZigZag,
            "crosszag" => Self::CrossZag,
            _ => return None,
        })
    }

    pub fn is_surface_pattern(self) -> bool {
        matches!(
            self,
            Self::Rectilinear
                | Self::AlignedRectilinear
                | Self::Monotonic
                | Self::MonotonicLines
                | Self::Concentric
                | Self::HilbertCurve
                | Self::ArchimedeanChords
                | Self::OctagramSpiral
        )
    }
    /// One of the zig-zag family (`has_consistent_pattern`).
    pub fn is_zag(self) -> bool {
        matches!(self, Self::ZigZag | Self::CrossZag)
    }
    pub fn needs_object(self) -> bool {
        matches!(
            self,
            Self::AdaptiveCubic | Self::SupportCubic | Self::Lightning
        )
    }

    /// The fill's own order must be kept: monotonic patterns are printed as generated.
    pub fn no_sort(self) -> bool {
        matches!(
            self,
            Pattern::Monotonic | Pattern::MonotonicLines | Pattern::Concentric
        )
    }

    /// PrusaSlicer's `Fill::_layer_angle`: rectilinear and monotonic fills turn by 90° every layer.
    fn layer_angle(self, layer_id: usize) -> f64 {
        if self == Pattern::Honeycomb {
            return PI / 3. * (layer_id % 3) as f64;
        }
        let alternates = matches!(
            self,
            Pattern::Rectilinear
                | Pattern::Monotonic
                | Pattern::MonotonicLines
                | Pattern::ZigZag
                | Pattern::CrossZag
        );
        if alternates && layer_id & 1 == 1 {
            FRAC_PI_2
        } else {
            0.0
        }
    }
}

/// Parameters of one fill (PrusaSlicer's `FillParams` plus the `Fill` members the patterns use).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FillParams {
    /// Line spacing at 100% density, mm (the flow spacing).
    pub spacing: f64,
    /// Density, 0–1.
    pub density: f64,
    /// Base angle (`fill_angle`), degrees.
    pub angle: f64,
    /// Object layer index; drives the angle alternation.
    pub layer_id: usize,
    /// Print z of the layer, mm (gyroid and cubic follow it).
    pub z: f64,
    /// Longest anchor of an unconnected line end along the boundary, mm.
    pub anchor_length: f64,
    /// Line ends closer than this along the boundary are connected, mm. Below 0.05 nothing is connected.
    pub anchor_length_max: f64,
    /// Keep the spacing even for solid fills instead of widening it to fit the area.
    pub dont_adjust: bool,
    /// Longest boundary piece linking two rectilinear lines, mm; 0 = unlimited.
    pub link_max_length: f64,
    /// Centre of the object's bounding box: the pattern is aligned to it, as PrusaSlicer works in
    /// object coordinates centred on the object.
    pub origin: Point,
}

/// The fill of one ExPolygon.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Filled {
    pub polylines: Vec<Polyline>,
    /// The line spacing actually used, mm: solid fills widen it to fit the area evenly.
    pub spacing: f64,
}

fn translate(points: &mut [Point], dx: i64, dy: i64) {
    for p in points {
        p.x += dx;
        p.y += dy;
    }
}

/// Fills each ExPolygon of `area` with `pattern`. Results come one per filled ExPolygon.
pub fn fill(pattern: Pattern, area: &[ExPolygon], params: &FillParams) -> Vec<Filled> {
    fill_with_bounds(pattern, area, params, None, &ZagSettings::default())
}

/// `fill` with the object's bounding box (relative to `params.origin`) and the zig-zag settings.
fn fill_with_bounds(
    pattern: Pattern,
    area: &[ExPolygon],
    params: &FillParams,
    bounds: Option<crate::geometry::BoundingBox>,
    zag: &ZagSettings,
) -> Vec<Filled> {
    if !params.density.is_finite()
        || !params.spacing.is_finite()
        || params.density <= 0.0
        || params.spacing < 0.000001
    {
        return Vec::new();
    }
    let density = f64::from(params.density.min(1.0) as f32);
    let (ox, oy) = (params.origin.x, params.origin.y);
    let base_angle = params.angle.to_radians();
    // `_infill_direction`: base angle + per-layer alternation + 90° (lines are generated vertically).
    let direction = f64::from(
        base_angle as f32 + pattern.layer_angle(params.layer_id) as f32 + FRAC_PI_2 as f32,
    );
    let line_params = LineFillParams {
        spacing: params.spacing,
        density,
        angle: direction,
        dont_adjust: params.dont_adjust,
        monotonic: matches!(pattern, Pattern::Monotonic | Pattern::MonotonicLines),
        anchor_length: params.anchor_length.min(params.anchor_length_max),
        anchor_length_max: if pattern == Pattern::MonotonicLines {
            0.0
        } else {
            params.anchor_length_max
        },
        link_max_length: (params.link_max_length * SCALE).round() as i64,
        consistent: false,
        object_box: None,
        horiz_move: None,
    };
    // The zig-zag family lays its lines out over the object's extended box.
    let object_box = bounds
        .filter(|_| pattern.is_zag())
        .map(zag::extended_object_bounding_box);
    let zag_lines = LineFillParams {
        consistent: true,
        object_box,
        horiz_move: zag::horiz_move(pattern, params.layer_id, zag),
        ..line_params
    };
    let mirror_axis = zag::mirror_axis(zag, object_box);

    let mut out = Vec::new();
    for ex in area.iter().filter(|ex| ex.contour.len() >= 3) {
        let mut ex = ex.clone();
        translate(&mut ex.contour, -ox, -oy);
        for h in &mut ex.holes {
            translate(h, -ox, -oy);
        }
        let mut spacing = params.spacing;
        let mut polylines = match pattern {
            Pattern::Rectilinear
            | Pattern::AlignedRectilinear
            | Pattern::Monotonic
            | Pattern::MonotonicLines => {
                let (pl, s) = rectilinear::fill_surface_by_lines(&ex, &line_params);
                spacing = s;
                pl
            }
            Pattern::ZigZag | Pattern::CrossZag => {
                let (pl, s) = zag::fill_lines(&ex, &zag_lines, mirror_axis);
                spacing = s;
                pl
            }
            Pattern::Grid | Pattern::Triangles | Pattern::Stars | Pattern::Cubic => {
                let sweep = |angle_base: f64, pattern_shift: f64| Sweep {
                    angle_base,
                    pattern_shift,
                };
                let sweeps = match pattern {
                    Pattern::Grid => vec![sweep(0.0, 0.0), sweep(FRAC_PI_2, 0.0)],
                    Pattern::Triangles => vec![
                        sweep(0.0, 0.0),
                        sweep(PI / 3.0, 0.0),
                        sweep(2.0 * PI / 3.0, 0.0),
                    ],
                    Pattern::Stars => vec![
                        sweep(0.0, 0.0),
                        sweep(PI / 3.0, 0.0),
                        sweep(2.0 * PI / 3.0, 1.5 * params.spacing / density),
                    ],
                    _ => {
                        let dx = 0.5f64.sqrt() * params.z;
                        vec![
                            sweep(0.0, dx),
                            sweep(PI / 3.0, -dx),
                            sweep(2.0 * PI / 3.0, dx),
                        ]
                    }
                };
                rectilinear::fill_surface_by_multilines(&ex, &line_params, &sweeps)
            }
            Pattern::Concentric => concentric::shrunk(&ex, params)
                .iter()
                .flat_map(|part| {
                    let filled = concentric::fill(part, params);
                    spacing = filled.spacing;
                    filled.polylines
                })
                .collect(),
            Pattern::Honeycomb
            | Pattern::Honeycomb3D
            | Pattern::HilbertCurve
            | Pattern::ArchimedeanChords
            | Pattern::OctagramSpiral
            | Pattern::CrossHatch => concentric::shrunk(&ex, params)
                .iter()
                .flat_map(|part| match pattern {
                    Pattern::Honeycomb => honeycomb::honeycomb(part, params, direction),
                    Pattern::Honeycomb3D => honeycomb::honeycomb_3d(part, params),
                    Pattern::CrossHatch => crosshatch::fill(part, params),
                    _ => plane_path::fill(part, params, pattern, bounds),
                })
                .collect(),
            Pattern::AdaptiveCubic
            | Pattern::SupportCubic
            | Pattern::Lightning
            | Pattern::Ensuring => {
                panic!("use fill::toolpaths with PreparedFill for object-dependent patterns")
            }
            Pattern::SupportBase => rectilinear::fill_support_base(&ex, &line_params),
            Pattern::Gyroid => {
                // `Fill::fill_surface`: shrink by half a line, then fill each part.
                let shrunk = offset_ex(
                    std::slice::from_ref(&ex),
                    -0.5 * params.spacing * SCALE,
                    Join::Miter,
                );
                let gp = gyroid::GyroidParams {
                    spacing: params.spacing,
                    density,
                    angle: base_angle,
                    z: params.z,
                    anchor_length: line_params.anchor_length,
                    anchor_length_max: params.anchor_length_max,
                };
                shrunk
                    .iter()
                    .flat_map(|part| gyroid::fill_gyroid(part, &gp))
                    .collect()
            }
        };
        for pl in &mut polylines {
            translate(pl, ox, oy);
        }
        polylines.retain(|p| p.len() >= 2);
        if !polylines.is_empty() {
            out.push(Filled { polylines, spacing });
        }
    }
    out
}

/// Orders polylines greedily by the nearest end point, starting near `from`, reversing them as needed.
pub fn order_nearest(lines: Vec<Polyline>, from: Point) -> Vec<Polyline> {
    base::greedy_chain(lines, Some(from))
}

#[cfg(test)]
mod tests;

/// Clip generated curves and connect their ends using the common boundary graph.
fn clipped_connected(lines: Vec<Polyline>, ex: &ExPolygon, p: &FillParams) -> Vec<Polyline> {
    let mut lines = crate::geometry::clip::intersection_pl(&lines, std::slice::from_ref(ex));
    lines.retain(|l| !l.is_empty());
    if p.anchor_length_max < 0.05 || lines.len() <= 1 {
        // chain_or_connect_infill / dont_connect(): chain_polylines.
        return base::chain_polylines(lines);
    }
    // Upstream connects the clipped lines unchained; Mica orders them first (kept: the U1
    // crosshatch output depends on it).
    let lines = base::greedy_chain(lines, None);
    let boundary = crate::geometry::clip::to_polygons(std::slice::from_ref(ex));
    connect::connect_infill(
        lines,
        &boundary,
        base::bbox_of(&ex.contour).unwrap(),
        p.spacing,
        p.anchor_length.min(p.anchor_length_max),
        p.anchor_length_max,
    )
}

/// Resolve a pattern from a canonical or retained Orca option, honoring CLI extras.
pub(crate) fn config_pattern(
    c: &crate::config::Config,
    key: &str,
    default: Pattern,
) -> Result<Pattern, crate::resolve::ResolveError> {
    let Some(name) = c.extra(key).or_else(|| c.string(key)) else {
        return Ok(default);
    };
    Pattern::from_config_name(name).ok_or_else(|| {
        crate::resolve::ResolveError(format!("{key} = {name}: unsupported infill pattern"))
    })
}
pub(crate) fn config_number(
    c: &crate::config::Config,
    key: &str,
    default: f64,
) -> Result<f64, crate::resolve::ResolveError> {
    let value = if let Some(raw) = c.extra(key) {
        raw.parse()
            .map_err(|_| crate::resolve::ResolveError(format!("invalid {key}: {raw}")))?
    } else {
        c.float(key).unwrap_or(default)
    };
    if !value.is_finite() {
        return Err(crate::resolve::ResolveError(format!(
            "{key} must be finite"
        )));
    }
    Ok(value)
}

#[cfg(test)]
mod m7_tests;
