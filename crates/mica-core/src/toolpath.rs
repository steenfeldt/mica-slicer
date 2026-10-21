// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/ExtrusionEntity.hpp :: OverhangAttributes, ExtrusionAttributes
// Changes: Original Mica paths extended with compact end-indexed support/curl ranges and wall inset index.
// Source: orca@8500fcdcca src/libslic3r/GCode/ExtrusionProcessor.hpp :: ProcessedPoint
// Changes: Overhang sections also carry the resolved speed of Orca's overhang estimator.

//! Machine-independent toolpaths: what to extrude, where, and with which cross-section.

use crate::flow::Flow;
use crate::geometry::Polyline;

/// What an extrusion is for. Drives speed, acceleration and the `;TYPE:` comment in G-code.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Role {
    Skirt,
    Brim,
    ExternalPerimeter,
    /// Unsupported perimeter printed with bridge flow.
    OverhangPerimeter,
    Perimeter,
    InternalInfill,
    SolidInfill,
    BridgeInfill,
    InternalBridgeInfill,
    TopSolidInfill,
    Ironing,
    /// Variable-width extrusion along the centreline of gaps between walls.
    GapFill,
    SupportMaterial,
    SupportMaterialInterface,
}

impl Role {
    pub fn is_bridge(self) -> bool {
        matches!(
            self,
            Self::BridgeInfill | Self::InternalBridgeInfill | Self::OverhangPerimeter
        )
    }

    /// Name used in PrusaSlicer-style `;TYPE:` comments, so existing G-code viewers colour it correctly.
    pub fn name(self) -> &'static str {
        match self {
            Role::Skirt | Role::Brim => "Skirt/Brim",
            Role::ExternalPerimeter => "External perimeter",
            Role::OverhangPerimeter => "Overhang perimeter",
            Role::Perimeter => "Perimeter",
            Role::InternalInfill => "Internal infill",
            Role::SolidInfill => "Solid infill",
            Role::BridgeInfill => "Bridge infill",
            // PrusaSlicer has no separate internal-bridge role and labels both "Bridge infill".
            Role::InternalBridgeInfill => "Bridge infill",
            Role::TopSolidInfill => "Top solid infill",
            Role::Ironing => "Ironing",
            Role::GapFill => "Gap fill",
            Role::SupportMaterial => "Support material",
            Role::SupportMaterialInterface => "Support material interface",
        }
    }
}

/// One continuous extrusion. Closed loops repeat nothing: the writer just follows the points.
#[derive(Clone, Debug, PartialEq)]
pub struct ExtrusionPath {
    pub role: Role,
    /// Wall inset: external = 0, first inner = 1. Preserved through splitting for cooling.
    pub perimeter_index: Option<usize>,
    pub points: Polyline,
    pub flow: Flow,
    /// Closed-loop seam metadata, with optional scarf modulation and seam-hiding travel.
    pub seam: Option<Box<crate::seam::PathSeam>>,
    /// Optional support/curl attributes for contiguous point ranges; absent when disabled.
    pub overhang: Option<Box<[OverhangSection]>>,
    /// Brick layers: set only on inner walls the feature changed (see [`crate::brick_layers`]).
    pub brick: Option<BrickWall>,
}

/// How a brick-layers inner wall differs from an ordinary one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BrickWall {
    /// Height above the layer's print Z the nozzle prints this path at, mm; 0 for a wall that
    /// stays on the layer grid. The path's `flow.height` is the bead's full height.
    pub raise: f64,
    /// Flow multiplier on top of the flow of the bead's height, never below 1.
    pub flow_ratio: f64,
}

impl ExtrusionPath {
    /// Height above the layer's print Z this path is printed at, mm (brick layers).
    pub fn z_raise(&self) -> f64 {
        self.brick.map_or(0.0, |b| b.raise)
    }

    /// Volume per mm of path, mm³: the flow's, times the brick-layers flow ratio.
    pub fn mm3_per_mm(&self) -> f64 {
        match self.brick {
            Some(b) => self.flow.mm3_per_mm() * b.flow_ratio,
            None => self.flow.mm3_per_mm(),
        }
    }
}

/// Everything printed on one layer, in printing order.
#[derive(Clone, Debug, Default)]
pub struct LayerToolpaths {
    pub index: usize,
    /// Height of the top of this layer above the bed, mm.
    pub print_z: f64,
    /// Layer thickness, mm.
    pub height: f64,
    pub paths: Vec<ExtrusionPath>,
    /// Prusa support entities that must remain whole and retain their direction (organic
    /// anchors and inner/outer wall collections). Other support paths may be reversed.
    pub support_groups: Vec<std::ops::Range<usize>>,
}

impl LayerToolpaths {
    /// Approximate heap bytes the layer holds (paths, points, seam samples, overhang ranges),
    /// for bounding how many generated layers a caller keeps.
    pub fn heap_bytes(&self) -> usize {
        use std::mem::size_of;
        self.paths.capacity() * size_of::<ExtrusionPath>()
            + self.support_groups.capacity() * size_of::<std::ops::Range<usize>>()
            + self
                .paths
                .iter()
                .map(|p| {
                    p.points.capacity() * size_of::<crate::geometry::Point>()
                        + p.seam.as_ref().map_or(0, |s| {
                            size_of::<crate::seam::PathSeam>()
                                + s.samples.capacity() * size_of::<crate::seam::ScarfSample>()
                        })
                        + p.overhang
                            .as_ref()
                            .map_or(0, |o| o.len() * size_of::<OverhangSection>())
                })
                .sum::<usize>()
    }
}

/// Support attributes for edges ending at `end` (inclusive), starting at the previous section's end.
/// Distances are signed distances to the lower layer plus half the extrusion width, in mm.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OverhangSection {
    pub end: u32,
    pub start_distance: f32,
    pub end_distance: f32,
    pub curled_proximity: f32,
    /// Orca rules: the section's resolved speed in mm/s (`ExtrusionQualityEstimator`); 0 when
    /// the speed follows from the distances (PrusaSlicer's dynamic overhang speeds).
    pub speed: f32,
}
