// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: orca@8500fcdcca src/libslic3r/GCode.cpp :: GCode::extrude_loop
// Changes: Optional Orca batch B wall, bridge or scarf controls; existing Prusa defaults retained.
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/SeamPlacer.cpp :: Placer::init, precalculate_seams, project_to_extrusion_loop, finalize_seam_position
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/ExtrusionOrder.cpp :: extract_perimeter_extrusions
// Changes: Once-per-object compact immutable plan for batched Rust toolpaths; nearest retains candidate boundaries because its choice depends on travel order.
// Changes: Restore first-layer seam geometry by upstream elephant-foot expansion; single-region boundary matching; boundary-projected scarf endpoints, wall-distance offsets when staggering inner seams.
// Changes: Respect configured printing direction, retaining source winding for curl estimation.
// Source: prusaslicer@b028299c77 src/libslic3r/PrintObjectSlice.cpp :: PrintObject::slice_volumes
// Source: prusaslicer@b028299c77 src/libslic3r/ClipperUtils.cpp :: raw_offset
// Changes: Prusa profiles expand each first-layer polygon on its own (holes shrink), classify layer 1 against the uncompensated first layer (upstream keeps it in lslices) and use decimated visibility.
// Copyright (c) Prusa Research 2020 - 2023 Vojtěch Bubník @bubnikv, Lukáš Matěna @lukasmatena, Pavel Mikuš @Godrak.
// Copyright (c) SuperSlicer 2023 Remi Durand @supermerill.

//! Cross-layer seams. Preparation releases visibility samples, the ray index and alignment candidates.
//! Aligned, rear and random retain one record per boundary per layer; all inner walls project that
//! boundary's seam. Nearest alone keeps classified candidates for travel-dependent selection.
//! Painting accepts bed-coordinate facets or a custom query; 3MF decoding is deliberately separate.

mod aligned;
mod choice;
mod decimate;
mod geometry;
mod index;
mod model_visibility;
pub mod painting;
mod perimeters;
mod random;
mod scarf;
mod settings;
mod shared_vertices;
#[cfg(test)]
mod tests;
mod variable;
mod visibility;
pub(crate) use variable::split as split_variable;

use crate::flow::Flow;
use crate::geometry::{BoundingBox, ExPolygon, Point, Polygon, SCALE, polygon_length};
use crate::mesh::TriangleMesh;
use crate::perimeters::WallLoop;
use crate::settings::PrintSettings;
use crate::toolpath::{ExtrusionPath, Role};
use geometry::{Choice, V};
use painting::SeamPainting;
use perimeters::Boundary;
use rayon::prelude::*;
pub use settings::{ScarfPlacement, ScarfSettings, SeamPosition, SeamSettings};

/// Fractions at a polyline point. Extrusion applies to the segment ending at this point,
/// matching Prusa's SmoothPath. Height is relative to the layer bottom.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScarfSample {
    pub height: f64,
    pub extrusion: f64,
}
impl Default for ScarfSample {
    fn default() -> Self {
        Self {
            height: 1.0,
            extrusion: 1.0,
        }
    }
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PathSeam {
    /// Empty for ordinary seams, otherwise exactly one entry per path point.
    pub samples: Vec<ScarfSample>,
    /// Optional Orca speed cap on the sloped sections only, mm/s.
    pub scarf_speed: Option<f64>,
    /// End of the start ramp and start of the return ramp, as segment indices.
    pub scarf_ranges: Option<[usize; 2]>,
    /// Non-extruding seam-hiding move after this path; never retract before this move.
    pub wipe: Option<Point>,
    /// Winding of the generated perimeter, retained for curl estimation after print-direction reversal.
    pub original_clockwise: bool,
    /// Complete loop length before seam clipping/scarfing, mm.
    pub loop_length_mm: Option<f64>,
}

pub struct PlanInput<'a> {
    pub mesh: &'a TriangleMesh,
    /// `mesh` as loaded, before its placement translation (PrusaSlicer's model frame), when
    /// known. Aligned visibility then reproduces upstream's float arithmetic exactly.
    pub source: Option<&'a TriangleMesh>,
    pub outlines: &'a [Vec<ExPolygon>],
    /// Upstream expands first-layer seam geometry by this amount (zero with a raft).
    pub first_layer_compensation: f64,
    /// The first layer before elephant-foot compensation (PrusaSlicer keeps these as its
    /// `lslices`, so layer 1 classifies overhangs against them).
    pub first_layer_uncompensated: &'a [ExPolygon],
    /// Mesh slice coordinates, before any raft Z translation.
    pub slice_zs: &'a [f64],
    pub heights: &'a [f64],
    pub settings: &'a SeamSettings,
    pub painting: &'a dyn SeamPainting,
}
#[derive(Clone, Copy)]
struct Selected {
    point: V,
    normal: V,
    smooth: bool,
    overhang: bool,
    backward: f64,
    forward: f64,
}
impl Selected {
    fn new(b: &Boundary, c: Choice) -> Self {
        Self {
            point: c.point,
            normal: b.normal(c),
            smooth: b.smooth(c),
            overhang: b.overhang(c),
            backward: b.allowed_distance(c, false),
            forward: b.allowed_distance(c, true),
        }
    }
}
#[derive(Clone, Copy)]
struct Projected {
    point: V,
    normal: V,
}
impl Projected {
    fn new(b: &Boundary, c: Choice) -> Self {
        Self {
            point: c.point,
            normal: b.normal(c),
        }
    }
    fn onto(self, poly: &[Point]) -> V {
        perimeters::project(poly, self.point, self.normal).point
    }
}
#[derive(Clone, Copy)]
struct ScarfTargets {
    back: Option<Projected>,
    back2: Option<Projected>,
    forward: Option<Projected>,
    forward2: Option<Projected>,
}
impl ScarfTargets {
    fn new(b: &Boundary, c: Choice, settings: &ScarfSettings) -> Option<Self> {
        if settings.placement == ScarfPlacement::Nowhere || settings.entire_loop {
            return None;
        }
        let pick = |distance, forward| b.offset(c, distance, forward).map(|c| Projected::new(b, c));
        Some(Self {
            back: pick(settings.length, false),
            back2: pick(settings.length * 2.0, false),
            forward: pick(settings.length, true),
            forward2: pick(settings.length * 2.0, true),
        })
    }
}
struct Record {
    bbox: BoundingBox,
    hole: bool,
    selected: Selected,
    scarf: Option<Box<ScarfTargets>>,
    nearest: Option<Box<Boundary>>,
}
#[derive(Default)]
pub struct SeamPlan {
    layers: Vec<Vec<Record>>,
}
impl SeamPlan {
    pub fn prepare(input: PlanInput<'_>) -> Self {
        Self::prepare_observed(input, &crate::progress::Silent)
    }

    /// [`Self::prepare`] reporting boundaries (0 .. 0.5), visibility (0.5 .. 0.7) and the
    /// aligned planning (0.7 .. 1) as the seam stage; after a cancel the plan comes back
    /// incomplete and the caller discards it.
    pub fn prepare_observed(
        input: PlanInput<'_>,
        observer: &dyn crate::progress::Observer,
    ) -> Self {
        use crate::progress::{Stage, Ticker};
        let _timing = crate::timing::Span::new(crate::timing::Stage::Seams);
        let expanded_first = input
            .outlines
            .first()
            .map(|outlines| {
                if input.first_layer_compensation > 0.0 {
                    crate::geometry::clip::offset_ex(
                        outlines,
                        input.first_layer_compensation * SCALE,
                        crate::geometry::clip::Join::Miter,
                    )
                } else {
                    outlines.clone()
                }
            })
            .unwrap_or_default();
        // PrusaSlicer expands each first-layer boundary polygon on its own (`expand(polygon)`):
        // contours grow, holes shrink, orientation is kept and the first result is used.
        let first_polygons: Vec<Polygon> = if input.settings.prusa_parity {
            input
                .outlines
                .first()
                .into_iter()
                .flatten()
                .flat_map(ExPolygon::polygons)
                .map(|p| {
                    if input.first_layer_compensation <= 0.0 {
                        return p.clone();
                    }
                    let delta = input.first_layer_compensation * SCALE;
                    let hole = crate::geometry::signed_area(p) < 0.0;
                    let mut solid = p.clone();
                    if hole {
                        solid.reverse();
                    }
                    let grown = crate::geometry::clip::offset(
                        &[solid],
                        if hole { -delta } else { delta },
                        crate::geometry::clip::Join::Miter,
                        3.0,
                    );
                    match grown.into_iter().next() {
                        Some(mut q) => {
                            if hole {
                                q.reverse();
                            }
                            q
                        }
                        None => p.clone(),
                    }
                })
                .collect()
        } else {
            Vec::new()
        };
        let boundaries_timer = crate::timing::Span::new(crate::timing::Stage::SeamBoundaries);
        let ticker = Ticker::span(observer, Stage::Seams, input.outlines.len(), 0., 0.5);
        let boundaries: Vec<Vec<Boundary>> = input
            .outlines
            .par_iter()
            .enumerate()
            .map(|(layer, outlines)| {
                if ticker.cancelled() {
                    return Vec::new();
                }
                ticker.tick();
                let here = if layer == 0 {
                    &expanded_first
                } else {
                    outlines
                };
                let previous = if layer == 1 && input.settings.prusa_parity {
                    input.first_layer_uncompensated
                } else if layer > 0 {
                    &input.outlines[layer - 1][..]
                } else {
                    &[]
                };
                let polygons: Vec<&Polygon> = if layer == 0 && input.settings.prusa_parity {
                    first_polygons.iter().collect()
                } else {
                    here.iter().flat_map(ExPolygon::polygons).collect()
                };
                polygons
                    .into_iter()
                    .filter_map(|p| {
                        let new = if input.settings.prusa_parity {
                            Boundary::new_prusa
                        } else {
                            Boundary::new
                        };
                        let mut boundary = new(
                            p,
                            previous,
                            input.slice_zs[layer],
                            input.heights[layer],
                            input.painting,
                        )?;
                        // An absent lower layer after layer zero is an unsupported floating start.
                        if layer > 0 && previous.is_empty() {
                            boundary.mark_overhang();
                        }
                        Some(boundary)
                    })
                    .collect()
            })
            .collect();
        drop(boundaries_timer);
        let choices = if observer.cancelled() {
            boundaries
                .iter()
                .map(|l| vec![Choice::default(); l.len()])
                .collect()
        } else if input.settings.position == SeamPosition::Aligned {
            let visibility_timer = crate::timing::Span::new(crate::timing::Stage::SeamVisibility);
            let visibility: Box<dyn visibility::PointVisibility> = if !input.settings.prusa_parity {
                Box::new(visibility::Visibility::new_observed(input.mesh, observer))
            } else {
                Box::new(model_visibility::ModelVisibility::new(
                    input.mesh,
                    input.source,
                    observer,
                ))
            };
            drop(visibility_timer);
            aligned::plan(&boundaries, visibility.as_ref(), observer)
        } else {
            let mut random = choice::Random::new(1_653_710_332);
            boundaries
                .iter()
                .map(|layer| {
                    layer
                        .iter()
                        .map(|b| match input.settings.position {
                            SeamPosition::Rear => choice::rear(b),
                            SeamPosition::Random => random.choose(b),
                            _ => Choice::vertex(&b.points, 0),
                        })
                        .collect()
                })
                .collect()
        };
        let layers = boundaries
            .into_iter()
            .zip(choices)
            .map(|(layer, choices)| {
                layer
                    .into_iter()
                    .zip(choices)
                    .map(|(b, c)| Record {
                        bbox: b.bbox,
                        hole: b.hole,
                        selected: Selected::new(&b, c),
                        scarf: ScarfTargets::new(&b, c, &input.settings.scarf).map(Box::new),
                        nearest: (input.settings.position == SeamPosition::Nearest)
                            .then(|| Box::new(b)),
                    })
                    .collect()
            })
            .collect();
        Self { layers }
    }
    pub fn layer_count(&self) -> usize {
        self.layers.len()
    }
    pub fn seam_count(&self) -> usize {
        self.layers.iter().map(Vec::len).sum()
    }
    /// Boundary seam locations before projection onto wall centre lines. For nearest these are seeds.
    pub fn points(&self, layer: usize) -> impl Iterator<Item = Point> + '_ {
        self.layers
            .get(layer)
            .into_iter()
            .flatten()
            .map(|r| r.selected.point.point())
    }
    pub fn open_loop(
        &self,
        layer: usize,
        wall: &WallLoop,
        flow: Flow,
        s: &PrintSettings,
        last: Point,
    ) -> Option<ExtrusionPath> {
        self.open_loop_supported(layer, wall, flow, s, last, None)
    }
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn open_loop_supported(
        &self,
        layer: usize,
        wall: &WallLoop,
        flow: Flow,
        s: &PrintSettings,
        last: Point,
        lower: Option<&[ExPolygon]>,
    ) -> Option<ExtrusionPath> {
        let poly = &wall.polygon;
        if poly.len() < 3 {
            return None;
        }
        let bbox = BoundingBox::from_points(poly.iter())?;
        let record = self
            .layers
            .get(layer)?
            .iter()
            .filter(|r| r.hole != wall.is_contour)
            .min_by(|a, b| {
                geometry::bb_distance(a.bbox, bbox).total_cmp(&geometry::bb_distance(b.bbox, bbox))
            });
        let (mut selected, mut targets) = if let Some(r) = record {
            if let Some(b) = &r.nearest {
                let c = choice::nearest(b, V::from(last), 1.0, true);
                (Selected::new(b, c), ScarfTargets::new(b, c, &s.seam.scarf))
            } else {
                (r.selected, r.scarf.as_deref().copied())
            }
        } else {
            let b = Boundary::new(poly, &[], 0.0, flow.height, &painting::NoPainting)?;
            let c = choice::nearest(&b, V::from(last), 1.0, true);
            (
                Selected::new(&b, c),
                ScarfTargets::new(&b, c, &s.seam.scarf),
            )
        };
        // ExtrusionOrder selects the printing direction independently of whether this is a hole.
        // Retain the original winding separately for curl estimation and boundary classification.
        let reversed;
        let poly =
            if (crate::geometry::signed_area(poly) < 0.0) != s.seam.prefer_clockwise_movements {
                reversed = poly.iter().rev().copied().collect::<Vec<_>>();
                std::mem::swap(&mut selected.backward, &mut selected.forward);
                if let Some(targets) = &mut targets {
                    std::mem::swap(&mut targets.back, &mut targets.forward);
                    std::mem::swap(&mut targets.back2, &mut targets.forward2);
                }
                &reversed
            } else {
                poly
            };
        let mut at = perimeters::project(poly, selected.point, selected.normal).point;
        let total = polygon_length(poly) / SCALE;
        if wall.depth > 0 && s.seam.staggered_inner_seams {
            let offset = ((at - selected.point).norm() - flow.width * 0.5)
                .max(0.0)
                .min(selected.backward)
                .min(total);
            let opened = geometry::open(poly, at);
            at = V::from(geometry::along(&opened, total - offset).1);
            selected.backward = (selected.backward - offset).max(0.0);
        }
        let scarf = &s.seam.scarf;
        let enabled = layer > 0
            && scarf.length > 0.0
            && (scarf.orca || !selected.overhang)
            && (!scarf.only_on_smooth || selected.smooth)
            && match scarf.placement {
                ScarfPlacement::Nowhere => false,
                ScarfPlacement::Contours => wall.is_contour,
                ScarfPlacement::Everywhere => true,
            };
        let mut length = if scarf.entire_loop {
            total
        } else {
            scarf.length
        };
        let enabled = enabled
            && if scarf.orca {
                !scarf.conditional
                    || scarf::conditional(poly, at, lower, flow.width, s.nozzle_diameter, scarf)
            } else {
                length <= selected.backward
                    && (scarf.entire_loop || length < total)
                    && (!scarf.conditional
                        || scarf::conditional(
                            poly,
                            at,
                            lower,
                            flow.width,
                            s.nozzle_diameter,
                            scarf,
                        ))
            };
        let enabled = enabled
            && (scarf.orca
                || wall.depth == 0
                || scarf.entire_loop
                || if s.external_perimeters_first {
                    length <= selected.forward
                } else {
                    length * if scarf.on_inner_perimeters { 2.0 } else { 1.0 } <= selected.backward
                });
        let mut enabled = enabled;
        let mut meta = None;
        if enabled && !scarf.entire_loop && !scarf.orca {
            if let Some(targets) =
                targets.filter(|_| wall.depth == 0 || !s.seam.staggered_inner_seams)
            {
                let end = if wall.depth == 0 {
                    Some(Projected {
                        point: selected.point,
                        normal: selected.normal,
                    })
                } else if s.external_perimeters_first {
                    targets.forward
                } else {
                    targets.back
                };
                let start = if wall.depth == 0 {
                    targets.back
                } else if s.external_perimeters_first {
                    Some(Projected {
                        point: selected.point,
                        normal: selected.normal,
                    })
                } else {
                    targets.back2
                };
                if let Some(end) = end {
                    at = end.onto(poly);
                    if wall.depth == 0 || scarf.on_inner_perimeters {
                        if let Some(start) = start {
                            let start = start.onto(poly);
                            length =
                                total - geometry::distance_along(&geometry::open(poly, at), start);
                            enabled = length > 1e-6 && length < total - 1e-6;
                        } else {
                            enabled = false;
                        }
                    }
                } else {
                    enabled = false;
                }
            } else if wall.depth > 0 {
                let offset = if s.external_perimeters_first {
                    length.min(selected.forward)
                } else {
                    total - length
                };
                at = V::from(geometry::along(&geometry::open(poly, at), offset).1);
            }
        }
        let mut points = if enabled && (wall.depth == 0 || scarf.on_inner_perimeters) {
            let mut resolved = scarf.clone();
            if scarf.orca {
                length = length.min(total);
                resolved.max_segment_length = length / scarf.steps as f64;
                resolved.start_height = scarf
                    .absolute_start_height
                    .map_or(scarf.start_height, |h| h / flow.height)
                    .min(0.99);
            }
            let (points, mut m) = scarf::add(poly, at, length, &resolved);
            if scarf.orca || scarf.speed != crate::settings::FloatOrPercent::Percent(100.) {
                m.scarf_speed = Some(scarf.speed.resolve(
                    if wall.depth == 0 {
                        s.external_perimeter_speed
                    } else {
                        s.perimeter_speed
                    },
                    1.,
                ));
            }
            meta = Some(Box::new(m));
            points
        } else {
            geometry::open(poly, at)
        };
        scarf::gap(
            &mut points,
            &mut meta,
            s.seam.gap.resolve(s.nozzle_diameter, 1.0),
        );
        if points.len() < 2 {
            return None;
        }
        if !s.seam.wipe
            && wall.depth == 0
            && s.perimeters > 1
            && meta.is_none()
            && let Some(wipe) = scarf::wipe(&points, &wall.polygon, s.nozzle_diameter)
        {
            meta = Some(Box::new(PathSeam {
                wipe: Some(wipe),
                ..PathSeam::default()
            }));
        }
        let mut meta = meta.unwrap_or_default();
        meta.loop_length_mm = Some(total);
        meta.original_clockwise = crate::geometry::signed_area(&wall.polygon) < 0.0;
        Some(ExtrusionPath {
            role: if wall.depth == 0 {
                Role::ExternalPerimeter
            } else {
                Role::Perimeter
            },
            points,
            flow,
            overhang: None,
            perimeter_index: Some(wall.depth),
            seam: Some(meta),
            brick: None,
        })
    }
}

impl ExtrusionPath {
    /// Position after extrusion and an optional inward wipe, for subsequent travel-dependent seams.
    pub fn end_position(&self) -> Option<Point> {
        self.seam
            .as_ref()
            .and_then(|m| m.wipe)
            .or_else(|| self.points.last().copied())
    }
}
