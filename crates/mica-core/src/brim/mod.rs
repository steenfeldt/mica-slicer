// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/Brim.cpp :: make_brim, make_inner_brim
// Source: prusaslicer@b028299c77 src/libslic3r/Print.cpp :: Print::_make_skirt, Print::finalize_first_layer_convex_hull
// Source: prusaslicer@b028299c77 src/libslic3r/GCode.cpp :: Skirt::skirt_loops_per_extruder_all_printing
// Source: orca@8500fcdcca src/libslic3r/Print.cpp :: Print::_make_skirt
// Changes: Object-agnostic borrowed inputs and one retained adhesion plan; Clipper2; explicit tool allocation.
// Copyright (c) Prusa Research 2021 - 2023 Vojtěch Bubník, Lukáš Matěna, Lukáš Hejl
// Copyright (c) SuperSlicer 2023 Remi Durand

//! Bed adhesion, prepared once for the whole plate. All coordinates are placed bed coordinates.
//! Callers with multiple objects must generate one plan from all islands, then emit its paths before
//! support/model paths. Skirt layer indices count object/raft layers, excluding interlaced support Zs.

mod geometry;
mod orca;
pub(crate) mod plate;
pub mod prepare;
mod settings;
#[cfg(test)]
mod tests;

use crate::{
    flow::Flow,
    geometry::{BoundingBox, ExPolygon, Point, Polygon},
    toolpath::{ExtrusionPath, Role},
};
pub use geometry::convex_hull;
use geometry::{inner_brim, outer_brim, skirt};
pub use orca::ear_points;
pub use settings::{AdhesionSettings, BrimSettings, BrimType, DraftShield};

/// One first-layer island, after XY compensation (and EFC for Prusa). An object's islands share
/// settings and height. Keep separate object IDs even when their contours touch or nest.
pub struct FirstLayerIsland<'a> {
    pub object_id: u64,
    pub polygon: &'a ExPolygon,
    pub brim: &'a BrimSettings,
    pub object_height: f64,
    /// The object's hand-placed ears (`brim_type = painted`), placed bed coordinates.
    pub ears: &'a [BrimEar],
}

/// A hand-placed brim ear (Orca's `BrimPoint`): a disc of `radius` mm at `x`, `y` on the bed.
/// Ears whose `z` is above the bed (0) are ignored, as upstream does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BrimEar {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub radius: f64,
}

/// Points from object and support layers up to the skirt's height, before EFC.
pub struct SkirtHull {
    /// IDs group parts of an instance. `u64::MAX` reserves tower area without its own skirt.
    pub object_id: u64,
    pub points: Vec<Point>,
}

#[derive(Clone, Copy, Debug)]
pub struct SkirtTool {
    pub id: usize,
    pub filament_diameter: f64,
    pub extrusion_multiplier: f64,
}

pub struct AdhesionInput<'a> {
    pub islands: &'a [FirstLayerIsland<'a>],
    /// Support/raft bed-contact regions. Orca excludes them from object brims.
    pub supports: &'a [ExPolygon],
    pub skirt_hulls: &'a [SkirtHull],
    /// Uncompensated first-layer footprint for probing when adhesion is absent.
    pub first_layer: &'a [ExPolygon],
    pub settings: &'a AdhesionSettings,
    pub flow: Flow,
    pub resolution: f64,
    pub tools: &'a [SkirtTool],
    pub max_speed: f64,
    pub filament_type: &'a str,
}

#[derive(Clone, Debug, Default)]
pub struct AdhesionPlan {
    pub brim: Vec<ExtrusionPath>,
    /// Orca object IDs, parallel to `brim`. Prusa emits its combined brim on the first tool.
    pub brim_objects: Vec<u64>,
    pub skirt: Vec<ExtrusionPath>,
    /// Convex hull of first-layer geometry including the actual brim and skirt envelopes.
    pub first_layer_hull: Polygon,
    pub skirt_height: usize,
    pub infinite_skirt: bool,
    pub orca: bool,
}

impl AdhesionPlan {
    pub fn first_layer_bbox(&self) -> Option<BoundingBox> {
        BoundingBox::from_points(self.first_layer_hull.iter())
    }

    /// Allocate contiguous outer-to-inner loops to the tools in the actual layer tool order.
    /// Orca assigns the whole skirt to the first printing tool; Prusa divides it evenly.
    pub fn skirt_ranges(&self, tools: &[usize]) -> Vec<(usize, std::ops::Range<usize>)> {
        if tools.is_empty() || self.skirt.is_empty() {
            return Vec::new();
        }
        if self.orca {
            return vec![(tools[0], 0..self.skirt.len())];
        }
        let n = self.skirt.len().div_ceil(tools.len());
        (0..self.skirt.len())
            .step_by(n)
            .enumerate()
            .map(|(i, start)| (tools[i], start..(start + n).min(self.skirt.len())))
            .collect()
    }

    /// Materialize only this layer's paths. `brim_tool` comes from the first perimeter/raft tool.
    /// Heights must be measured from the previous skirt Z, not an interlaced support layer.
    pub fn paths_for_tool(
        &self,
        layer: usize,
        height: f64,
        tools: &[usize],
        tool: usize,
        brim_tool: usize,
    ) -> Vec<ExtrusionPath> {
        let mut paths = Vec::new();
        if self.infinite_skirt || layer < self.skirt_height {
            for (id, range) in self.skirt_ranges(tools) {
                if id == tool {
                    paths.extend(self.skirt[range].iter().cloned().map(|mut p| {
                        p.flow = Flow::new(p.flow.width, height);
                        p
                    }));
                }
            }
        }
        if layer == 0 && tool == brim_tool {
            paths.extend(self.brim.iter().cloned());
        }
        paths
    }
}

pub fn generate(input: &AdhesionInput<'_>) -> AdhesionPlan {
    let mut plan = AdhesionPlan {
        skirt_height: input.settings.skirt_height,
        infinite_skirt: input.settings.draft_shield == DraftShield::Enabled,
        orca: input.settings.orca,
        ..AdhesionPlan::default()
    };
    let (mut brim, brim_area, mut brim_objects) = if input.settings.orca {
        orca::brim(input)
    } else {
        let (mut paths, area) = outer_brim(input);
        paths.extend(inner_brim(input));
        (paths, area, Vec::new())
    };
    let (skirts, skirt_area) = skirt(input, &brim_area);
    // A draft shield stays at the requested distance; trim brim paths around its deposited band.
    if input.settings.draft_shield != DraftShield::Disabled && !skirts.is_empty() {
        let bands = crate::geometry::clip::offset_polylines_round(
            &skirts.iter().map(|p| p.points.clone()).collect::<Vec<_>>(),
            input.flow.scaled_spacing() as f64 * 0.5,
            10_000.0,
        );
        let mut trimmed_objects = Vec::new();
        brim = brim
            .into_iter()
            .enumerate()
            .flat_map(|(i, p)| {
                crate::geometry::clip::diff_pl(&[p.points], &bands)
                    .into_iter()
                    .map(|points| {
                        if let Some(&object) = brim_objects.get(i) {
                            trimmed_objects.push(object);
                        }
                        path(points, Role::Brim, input.flow)
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        brim_objects = trimmed_objects;
    }
    let mut points: Vec<_> = input
        .first_layer
        .iter()
        .chain(input.supports)
        .flat_map(|p| p.contour.iter().copied())
        .collect();
    points.extend(brim_area.iter().flat_map(|p| p.contour.iter().copied()));
    points.extend(skirt_area.iter().flat_map(|p| p.contour.iter().copied()));
    plan.first_layer_hull = convex_hull(points);
    plan.skirt = skirts;
    plan.brim = brim;
    plan.brim_objects = brim_objects;
    plan
}

fn path(points: Vec<Point>, role: Role, flow: Flow) -> ExtrusionPath {
    ExtrusionPath {
        points,
        role,
        flow,
        seam: None,
        overhang: None,
        perimeter_index: None,
        brick: None,
    }
}
