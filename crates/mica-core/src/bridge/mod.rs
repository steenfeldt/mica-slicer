// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/LayerRegion.cpp :: LayerRegion::bridging_flow
// Source: prusaslicer@b028299c77 src/libslic3r/Flow.cpp :: Flow::with_cross_section, Flow::bridge_extrusion_spacing
// Source: prusaslicer@b028299c77 src/libslic3r/Fill/Fill.cpp :: group_fills, Layer::make_fills
// Source: orca@8500fcdcca src/libslic3r/Fill/Fill.cpp :: Layer::make_fills
// Changes: Orca keeps bridge spacing and flow fixed; internal bridge thickness is independently selected.
// Source: orca@8500fcdcca src/libslic3r/LayerRegion.cpp :: LayerRegion::bridging_flow
// Changes: Orca bridge_line_width replaces the bridge base width and thick-bridge thread diameter.
// Changes: Single-region bridge surfaces; separate pattern spacing from the extrusion cross-section.
// Changes: Bridge fills are also returned as per-ExPolygon collections with their bridge angle.
// Attribution from src/libslic3r/LayerRegion.cpp:
// Copyright (c) Prusa Research 2016 - 2023 Vojtěch Bubník @bubnikv, Pavel Mikuš @Godrak, Lukáš Matěna @lukasmatena, Lukáš Hejl @hejllukas
// Copyright (c) Slic3r 2014 - 2016 Alessandro Ranellucci @alranel
//
// PrusaSlicer is released under the terms of the AGPLv3 or higher
//
// Attribution from src/libslic3r/Flow.cpp:
// Copyright (c) Prusa Research 2016 - 2023 Pavel Mikuš @Godrak, Oleksandra Iushchenko @YuSanka, Vojtěch Bubník @bubnikv, Tomáš Mészáros @tamasmeszaros
// Copyright (c) Slic3r 2014 - 2015 Alessandro Ranellucci @alranel
// Copyright (c) 2014 Petr Ledvina @ledvinap
//
// ported from lib/Slic3r/Flow.pm:
// Copyright (c) Prusa Research 2022 Vojtěch Bubník @bubnikv
// Copyright (c) Slic3r 2012 - 2014 Alessandro Ranellucci @alranel
//
// PrusaSlicer is released under the terms of the AGPLv3 or higher
//
// Attribution from src/libslic3r/Fill/Fill.cpp:
// Copyright (c) Prusa Research 2016 - 2023 Lukáš Matěna @lukasmatena, Vojtěch Bubník @bubnikv, Pavel Mikuš @Godrak, Lukáš Hejl @hejllukas
// Copyright (c) SuperSlicer 2023 Remi Durand @supermerill
// Copyright (c) 2016 Sakari Kapanen @Flannelhead
// Copyright (c) Slic3r 2011 - 2015 Alessandro Ranellucci @alranel
// Copyright (c) 2013 Mark Hindess
// Copyright (c) 2011 Michael Moon
//
// PrusaSlicer is released under the terms of the AGPLv3 or higher
//
//! Bridge geometry, direction detection, sparse-infill anchors and bridge toolpaths.

pub mod detector;
pub(crate) mod extra;
mod internal;
pub use extra::ExtraBridgeLayer;
#[cfg(test)]
mod tests;

use crate::fill::{FillParams, Pattern};
use crate::flow::Flow;
use crate::geometry::clip::intersection_ex;
use crate::geometry::{ExPolygon, Point};
use crate::settings::PrintSettings;
use crate::toolpath::{ExtrusionPath, Role};
pub(crate) use internal::{
    bridge_over_infill, bridge_over_infill_prepared, bridge_over_infill_supported,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BridgeStyle {
    #[default]
    Prusa,
    Orca,
}

/// A connected bridge with its extrusion direction measured counterclockwise from X, radians.
#[derive(Clone, Debug, PartialEq)]
pub struct BridgeSurface {
    pub polygon: ExPolygon,
    pub angle: f64,
}

/// Bridge spacing is independent of its deposited cross-section, including for thin bridges.
#[derive(Clone, Copy, Debug)]
pub struct BridgeFlow {
    pub extrusion: Flow,
    pub spacing: f64,
}

impl BridgeFlow {
    /// Support discovery tests clearance for a thick strand even when the emitted internal bridge
    /// uses normal layer height (Orca's bridge_over_infill calls bridging_flow with true).
    pub(crate) fn for_support(s: &PrintSettings, normal: Flow) -> Self {
        let mut settings = s.clone();
        settings.thick_internal_bridges = true;
        Self::new(&settings, normal, true)
    }

    pub fn new(s: &PrintSettings, normal: Flow, internal: bool) -> Self {
        // Orca LayerRegion::bridging_flow: a configured bridge_line_width replaces the width of
        // the base flow, and the nozzle diameter of a thick bridge thread.
        let bridge_width = s
            .orca_walls
            .bridge_line_width
            .map(|w| w.resolve(s.nozzle_diameter, 1.0));
        let normal = bridge_width.map_or(normal, |w| Flow::new(w, normal.height));
        let ratio = s.bridge_flow_ratio;
        assert!(
            ratio.is_finite() && ratio > 0.0,
            "bridge_flow_ratio must be positive"
        );
        if if internal {
            s.thick_internal_bridges
        } else {
            s.thick_bridges
        } {
            let diameter = bridge_width.unwrap_or(s.nozzle_diameter) * ratio.sqrt();
            // A rounded rectangle whose width equals its height is exactly a circle.
            Self {
                extrusion: Flow::new(diameter, diameter),
                spacing: diameter + 0.05,
            }
        } else {
            let area = normal.mm3_per_mm() * ratio;
            let extrusion = if ratio > 1.0 {
                let height = area / normal.spacing();
                Flow::new(
                    normal.spacing() + height * (1.0 - std::f64::consts::FRAC_PI_4),
                    height,
                )
            } else {
                let width = normal.width - (normal.mm3_per_mm() - area) / normal.height;
                if width > normal.height {
                    Flow::new(width, normal.height)
                } else {
                    let diameter = 2.0 * (area / std::f64::consts::PI).sqrt();
                    Flow::new(diameter, diameter)
                }
            };
            Self {
                extrusion,
                spacing: normal.spacing(),
            }
        }
    }
}

/// Generate bridges before ordinary infill, with upstream spacing adjustment and no layer alternation.
#[cfg(test)]
pub(crate) fn toolpaths(
    surfaces: &[BridgeSurface],
    island: &[ExPolygon],
    s: &PrintSettings,
    normal: Flow,
    origin: Point,
    internal: bool,
) -> Vec<ExtrusionPath> {
    collections(surfaces, island, s, normal, origin, internal)
        .into_iter()
        .flat_map(|(_, c)| c.entities.into_iter().flatten())
        .collect()
}

/// The bridge fills as `Layer::make_fills` collections (one per filled ExPolygon, unordered), each with
/// its bridge angle in radians.
pub(crate) fn collections(
    surfaces: &[BridgeSurface],
    island: &[ExPolygon],
    s: &PrintSettings,
    normal: Flow,
    origin: Point,
    internal: bool,
) -> Vec<(f64, crate::fill::FillCollection)> {
    let flow = BridgeFlow::new(s, normal, internal);
    let pattern = if matches!(
        s.top_fill_pattern,
        Pattern::Monotonic | Pattern::MonotonicLines
    ) {
        Pattern::Monotonic
    } else {
        Pattern::Rectilinear
    };
    let role = if internal {
        Role::InternalBridgeInfill
    } else {
        Role::BridgeInfill
    };
    let mut result = Vec::new();
    for surface in surfaces {
        let area = intersection_ex(std::slice::from_ref(&surface.polygon), island);
        let params = FillParams {
            spacing: flow.spacing,
            density: if internal {
                s.internal_bridge_density
            } else {
                s.bridge_density
            },
            angle: surface.angle.to_degrees(),
            layer_id: 0,
            z: 0.0,
            anchor_length: 1000.0,
            anchor_length_max: 1000.0,
            dont_adjust: s.bridge_style == BridgeStyle::Orca,
            link_max_length: 0.0,
            origin,
        };
        for filled in crate::fill::fill(pattern, &area, &params) {
            let extrusion = if s.bridge_style == BridgeStyle::Orca {
                flow.extrusion
            } else if if internal {
                s.thick_internal_bridges
            } else {
                s.thick_bridges
            } {
                let diameter = filled.spacing - 0.05;
                Flow::new(diameter, diameter)
            } else {
                Flow::new(
                    flow.extrusion.width + filled.spacing - flow.spacing,
                    flow.extrusion.height,
                )
            };
            let entities: Vec<_> = filled
                .polylines
                .into_iter()
                .map(|points| {
                    vec![ExtrusionPath {
                        role,
                        points,
                        flow: extrusion,
                        seam: None,
                        overhang: None,
                        perimeter_index: None,
                        brick: None,
                    }]
                })
                .collect();
            if !entities.is_empty() {
                result.push((
                    surface.angle,
                    crate::fill::FillCollection {
                        entities,
                        no_sort: pattern.no_sort(),
                        multi: false,
                    },
                ));
            }
        }
    }
    result
}
