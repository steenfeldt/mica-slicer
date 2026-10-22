// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: orca@8500fcdcca src/libslic3r/PrintObjectSlice.cpp :: PrintObject::slice
// Source: orca@8500fcdcca src/libslic3r/Support/SupportCommon.cpp :: generate_support_toolpaths
// Changes: Support islands (the union of a layer's support areas) are derived on request from the support plan.
// Changes: Supports read the first layer without the elephant foot compensation (PrintObject::slice_volumes stores the uncompensated lslices back).
// Source: orca@8500fcdcca src/libslic3r/GCode/RetractWhenCrossingPerimeters.cpp :: RetractWhenCrossingPerimeters::travel_inside_internal_regions
// Changes: Per-region layer slices (grouped by region and extruder) for Orca's internal-region retraction test.
// Source: orca@8500fcdcca src/libslic3r/PerimeterGenerator.cpp :: detect_steep_overhang, reorient_perimeters, PerimeterGenerator::process_classic
// Changes: Optional Orca batch B wall, bridge or scarf controls; existing Prusa defaults retained.
// Changes: Classic wall loops split into overhang sections at the lower layer grown by half a nozzle.
// Changes: Prusa profiles slice a plain object (an STL, a one-volume 3MF object) in the float object frame of its
// Changes: model frame (mesh as loaded, instance transform), then shift it to the bed by `origin`.
// Source: prusaslicer@b028299c77 src/libslic3r/PrintObjectSlice.cpp :: PrintObject::slice_volumes
// Source: prusaslicer@b028299c77 src/libslic3r/Print.cpp :: Print::first_layer_islands, Print::finalize_first_layer_convex_hull
// Source: prusaslicer@b028299c77 src/libslic3r/GCode.cpp :: GCode::process_layer
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/SeamPlacer.cpp :: Placer::init, Placer::place_seam
// Source: orca@8500fcdcca src/libslic3r/Fill/Fill.cpp :: Layer::make_fills
// Changes: Delegate optional narrow solid fill to fill/orca_solid, retaining Prusa defaults.
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/SmoothPath.cpp :: SmoothPathCache::resolve_or_fit_split_with_seam
// Source: prusaslicer@b028299c77 src/libslic3r/Geometry/ArcWelder.cpp :: split_at
// Changes: Prusa profiles split multi-width Arachne loops at the projected seam (merged with a vertex within 1.5 um) instead of the nearest vertex; the loaded STL mesh is passed to seam visibility.
// Changes: Delegate seam planning and optional scarf/wipe handling to dedicated seam modules.
// Changes: Merge support and object Z schedules; support paths precede object paths at equal Z.
// Changes: Expose outlines by output-layer index, including empty slots for support-only layers.
// Changes: Object-wide XY and elephant-foot compensation in Mica's batched pipeline; retain
// Changes: first-layer slices before elephant-foot compensation; adhesion plan supplies final start bounds.
// Changes: Optional indexed region pipeline shares object outlines, layer scheduling and support masks.
// Source: prusaslicer@b028299c77 src/libslic3r/Layer.cpp :: Layer::make_slices
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/ExtrusionOrder.cpp :: get_slices_extrusions, extract_island_extrusions, extract_infill_ranges
// Changes: Prusa sources print islands in chain_expolygons order and an island's fills (gap fill
// Changes: included) as one shortest-path chained set; Orca sources keep the nearest-island order.
// Changes: Prusa `infill_first` prints an island's fills before its perimeters.
// Source: orca@8500fcdcca src/libslic3r/GCode.cpp :: GCode::process_layer, GCode::extrude_infill
// Changes: Orca island fills: one collection per filled ExPolygon (bridges and narrow concentric
// Changes: fills included), all chained together from the last position.
// Changes: Orca ironing is a separate per-island pass after fills, chained from their endpoint.
// Changes: Brick layers: a per-object plan of raised walls, applied to each finished layer.
// Changes: An object without regions classifies its surfaces beside the seams and keeps those walls for its first toolpaths; generated supports let region preparation go on to the prepass layers.
// Source: prusaslicer@b028299c77 src/libslic3r/PrintObjectSlice.cpp :: slice_volumes
// Source: prusaslicer@b028299c77 src/libslic3r/LayerRegion.cpp :: LayerRegion::make_perimeters
// Source: prusaslicer@b028299c77 src/libslic3r/GCode.cpp :: SmoothPathGenerator::operator()
// Changes: Spiral vase: layers above the bottom sliced to their largest outline, classic walls on spiral layers, no seam gap on spiral layers, and bottom layers starting where the layer below ended (the last position carried from layer to layer).

//! The slicing pipeline for one object: mesh → layers → walls → top/bottom → infill → ordered toolpaths.
//!
//! [`prepare_object`] keeps layer outlines and a compact seam plan; toolpaths are computed per batch of layers on
//! demand ([`LayerSource::toolpaths`]), so a whole print's toolpaths never have to exist at once.

use rayon::prelude::*;

use crate::fill::{FillParams, Pattern};
use crate::flow::Flow;
use crate::geometry::clip::{Join, intersection_ex, offset_ex, union_ex};
use crate::geometry::elephant_foot::elephant_foot_compensation;
use crate::geometry::{BoundingBox, ExPolygon, Point, Polygon, Polyline, scale};
use crate::mesh::TriangleMesh;
use crate::perimeters::{IslandWalls, WallParams};
use crate::settings::PrintSettings;
use crate::shells::{FillSurfaces, ShellLayer, classify_at as classify};
use crate::toolpath::{ExtrusionPath, LayerToolpaths, Role};

#[derive(Clone, Debug, Default)]
pub struct SliceReport {
    pub layers: usize,
    /// Chains of triangle cuts that never closed into a loop (a sign of a broken mesh).
    pub dropped_chains: usize,
}

pub struct SlicedObject {
    pub adhesion: crate::brim::AdhesionPlan,
    /// Model outlines retained for the same obstacle-aware travel as batched export.
    pub outlines: Vec<Vec<ExPolygon>>,
    pub layers: Vec<LayerToolpaths>,
    /// Startup footprint: uncompensated model/support contact plus actual skirt/brim envelopes.
    pub first_layer_bbox: Option<BoundingBox>,
    pub report: SliceReport,
}

/// Flows used on one layer.
pub(crate) struct LayerFlows {
    pub(crate) external: Flow,
    pub(crate) perimeter: Flow,
    pub(crate) infill: Flow,
    pub(crate) solid: Flow,
    pub(crate) top: Flow,
}

impl LayerFlows {
    pub(crate) fn new(s: &PrintSettings, layer: usize, height: f64) -> Self {
        let w = |width: f64| {
            Flow::new(
                if layer == 0 && s.raft_layers == 0 {
                    s.first_layer_extrusion_width
                } else {
                    width
                },
                height,
            )
        };
        Self {
            external: w(s.external_perimeter_extrusion_width),
            perimeter: w(s.perimeter_extrusion_width),
            infill: w(s.infill_extrusion_width),
            solid: w(s.solid_infill_extrusion_width),
            top: w(s.top_infill_extrusion_width),
        }
    }
}

/// Where finished layers come from: either all computed up front ([`SlicedObject`]) or computed in
/// batches on demand ([`PreparedObject`]), so consumers never need every layer's toolpaths at once.
/// What the exporter actually emitted for one stretch of a path: the final feedrate after
/// cooling slowdown and the fan it set. Plain data, so previews can show what the printer
/// will do without the exporter knowing about any GUI.
#[derive(Clone, Debug, PartialEq)]
pub struct PathEmission {
    /// Index into the scheduled layer's `paths`.
    pub path: usize,
    /// Point range of the path this applies to (`start..end`, end inclusive as a segment end).
    pub points: std::ops::Range<usize>,
    pub speed_mm_s: f64,
    pub fan_percent: f64,
}

pub trait LayerSource: Sync {
    /// Calibration changes at an original path boundary; ordinary slicing has none.
    fn calibration_control(&self, _layer: usize, _path: usize) -> Option<crate::calib::Control> {
        None
    }
    /// Cooperative interruption checked by streaming consumers between layers.
    fn is_cancelled(&self) -> bool {
        false
    }
    /// Called by the exporter once a layer's speeds and fans are final, before its G-code is
    /// written. `layer_time_s` is the layer's estimated print time after cooling slowdown.
    fn emitted(&self, _layer: usize, _paths: &[PathEmission], _layer_time_s: f64) {}
    /// Called by the exporter, before [`Self::emitted`], for a layer it prints as a spiral vase
    /// spiral (Z rising over the layer), with the layer as it prints it (its loop starts where
    /// the previous layer ended); `last`: the print's last layer, which also prints a
    /// finishing loop at its own height.
    fn spiral_layer(
        &self,
        _layer: usize,
        _scheduled: &crate::print_job::ScheduledLayer,
        _last: bool,
    ) {
    }
    fn print_job(&self) -> Option<&crate::print_job::PreparedPrint> {
        None
    }
    /// The plate's per-layer custom G-code (colour changes, pauses, …); none by default.
    fn custom_gcode(&self) -> Option<&crate::custom_gcode::Info> {
        None
    }
    fn scheduled_toolpaths(
        &self,
        range: std::ops::Range<usize>,
    ) -> Vec<crate::print_job::ScheduledLayer> {
        self.toolpaths(range)
            .into_iter()
            .map(|layer| crate::print_job::ScheduledLayer {
                layer,
                assignments: Vec::new(),
            })
            .collect()
    }
    /// Actual first-layer footprint including adhesion; absent for custom legacy sources.
    fn first_layer_hull(&self) -> Option<&[Point]> {
        None
    }

    /// Model outlines for obstacle-aware travel; missing outlines force vertical lift when obstacle checks are enabled.
    fn layer_outlines(&self, _index: usize) -> Option<&[ExPolygon]> {
        None
    }
    /// Top solid fill surfaces (`stTop`) of the layer, which avoid-crossing travel keeps off.
    fn layer_top_surfaces(&self, _index: usize) -> Option<&[ExPolygon]> {
        None
    }
    /// Orca's `SupportLayer::support_islands`: the union of every support area printed at this
    /// height, placed like the outlines. A support travel inside one needs no retraction.
    fn support_islands(&self, _index: usize) -> Option<Vec<ExPolygon>> {
        None
    }
    /// Each print region's slices on this layer (Orca `LayerRegion::slices`), placed like the
    /// outlines; `None` when the layer is a single region.
    fn region_slices(&self, _index: usize) -> Option<Vec<Vec<ExPolygon>>> {
        None
    }
    /// Placed centre of object `object` (PrusaSlicer's instance shift, the G-code origin while it
    /// prints); `None` when unknown.
    fn object_origin(&self, _object: usize) -> Option<Point> {
        None
    }
    fn layer_count(&self) -> usize;
    /// (print_z, height) of layer `i`, mm.
    fn layer_z(&self, i: usize) -> (f64, f64);
    /// The layer printed under layer `i`: `i - 1`, except at the first layer of an object
    /// printed sequentially (one object after another), which stands on the bed.
    fn previous_layer(&self, i: usize) -> Option<usize> {
        match self.print_job() {
            Some(job) => job.previous_layer(i),
            None => i.checked_sub(1),
        }
    }
    /// The layer printed over layer `i` (see [`Self::previous_layer`]).
    fn next_layer(&self, i: usize) -> Option<usize> {
        match self.print_job() {
            Some(job) => job.next_layer(i),
            None => (i + 1 < self.layer_count()).then_some(i + 1),
        }
    }
    /// Highest print Z of any layer (the last layer's, unless objects print one after another).
    fn max_print_z(&self) -> f64 {
        match self.print_job() {
            Some(job) => job.max_print_z(),
            None => self
                .layer_count()
                .checked_sub(1)
                .map_or(0., |i| self.layer_z(i).0),
        }
    }
    /// Startup footprint: uncompensated model/support contact plus actual skirt/brim envelopes.
    fn first_layer_bbox(&self) -> Option<BoundingBox>;
    /// Finished toolpaths for the layers in `range`.
    fn toolpaths(&self, range: std::ops::Range<usize>) -> Vec<LayerToolpaths>;
}

/// An object sliced into outlines, with toolpaths computed per batch of layers when asked for.
/// Outlines and compact seam records are kept for the whole object; walls, solid areas and toolpaths of a
/// layer exist only while its batch is being processed.
pub struct PreparedObject {
    pub adhesion: crate::brim::AdhesionPlan,
    pub(crate) settings: PrintSettings,
    pub(crate) regions: Option<crate::regions::PreparedRegions>,
    infill: crate::fill::PreparedFill,
    /// Compact seam records, shared by every toolpath batch.
    pub seams: crate::seam::SeamPlan,
    pub(crate) origin: Point,
    pub(crate) tops: Vec<f64>,
    heights: Vec<f64>,
    pub(crate) outlines: Vec<Vec<ExPolygon>>,
    /// XY compensation applied, elephant-foot compensation not applied (PrusaSlicer's `lslices`).
    pub uncompensated_first_layer: Vec<ExPolygon>,
    pub(crate) supports: crate::support::SupportPlan,
    pub(crate) print_layers: Vec<crate::support::schedule::PrintLayer>,
    /// Typed infill areas per layer (top, bottom, solid, sparse), from [`classify`].
    surfaces: Vec<FillSurfaces>,
    first_layer_bbox: Option<BoundingBox>,
    pub report: SliceReport,
    /// Hand-placed brim ears (bed coordinates of this prepared instance).
    pub(crate) brim_ears: Vec<crate::brim::BrimEar>,
    /// Texture skin of the outer walls (`mica_texture_skin`, texture paint). Regional objects
    /// carry it in their regions' painting too.
    texture: Option<std::sync::Arc<crate::texture_skin::TextureSkin>>,
    /// Walls generated while classifying the surfaces of an object without regions, per
    /// layer, until [`PreparedObject::walls`] takes them.
    plain_walls: Vec<PlainWalls>,
    /// What the texture on all surfaces says (depth limited, triangle budget reached).
    displacement_warnings: Vec<String>,
    /// Brick layers: where this object's raised inner walls print raised.
    brick: Option<crate::brick_layers::Plan>,
    /// Spiral vase: where each bottom layer's toolpaths start, the end of the layer below (as
    /// both slicers carry the last position from layer to layer, and spiral vase splits every
    /// loop at the point nearest it). The exporter carries it on over the spiral layers.
    spiral_starts: Vec<Point>,
}

/// Slices a mesh that is already placed on the bed (resting on z = 0) into outlines.
pub fn prepare_object(
    mesh: &TriangleMesh,
    s: &PrintSettings,
) -> Result<PreparedObject, crate::support::SupportError> {
    try_prepare_object(mesh, s)
}

/// [`prepare_object`] for a placed mesh with the mesh as loaded (`source`), as the CLI prepares an
/// STL: PrusaSlicer profiles slice the loaded mesh in PrusaSlicer's float object frame.
pub fn prepare_object_from_source(
    mesh: &TriangleMesh,
    source: &TriangleMesh,
    s: &PrintSettings,
) -> Result<PreparedObject, crate::support::SupportError> {
    prepare_with_source(
        mesh,
        s,
        &crate::seam::painting::NoPainting,
        None,
        Some(&crate::input::Frame {
            source: std::sync::Arc::new(source.clone()),
            trafo: crate::threemf::Transform::IDENTITY,
        }),
        &crate::progress::Silent,
    )
}

/// Fallible preparation, including normal supports and a raft when configured.
pub fn try_prepare_object(
    mesh: &TriangleMesh,
    s: &PrintSettings,
) -> Result<PreparedObject, crate::support::SupportError> {
    try_prepare_object_with_seam_painting(mesh, s, &crate::seam::painting::NoPainting)
}

/// Preparation with bed-coordinate seam enforcers/blockers (3MF facet decoding is separate).
pub fn try_prepare_object_with_seam_painting(
    mesh: &TriangleMesh,
    s: &PrintSettings,
    painting: &dyn crate::seam::painting::SeamPainting,
) -> Result<PreparedObject, crate::support::SupportError> {
    prepare_with_regions(mesh, s, painting, None, &crate::progress::Silent)
}

/// Preparation reporting its stages to `observer`, which may also cancel it (the error text
/// is then [`crate::progress::CANCELLED`]).
pub fn try_prepare_object_observed(
    mesh: &TriangleMesh,
    s: &PrintSettings,
    observer: &dyn crate::progress::Observer,
) -> Result<PreparedObject, crate::support::SupportError> {
    prepare_with_regions(mesh, s, &crate::seam::painting::NoPainting, None, observer)
}

pub(crate) fn prepare_with_regions(
    mesh: &TriangleMesh,
    s: &PrintSettings,
    painting: &dyn crate::seam::painting::SeamPainting,
    model: Option<(&crate::regions::RegionModel, &crate::config::Config)>,
    observer: &dyn crate::progress::Observer,
) -> Result<PreparedObject, crate::support::SupportError> {
    prepare_with_source(mesh, s, painting, model, None, observer)
}

/// The object's texture context: from its own texture settings, its modifiers' and height
/// ranges' (`mica_texture_skin*` overrides) and its texture paint.
fn prepare_texture(
    mesh: &TriangleMesh,
    s: &PrintSettings,
    model: Option<(&crate::regions::RegionModel, &crate::config::Config)>,
    zs: &[f64],
    walls: Option<std::sync::Arc<crate::texture::displace::Rims>>,
) -> Result<Option<std::sync::Arc<crate::texture_skin::TextureSkin>>, String> {
    let mut list = vec![s.texture_skin.clone()];
    if let Some((m, c)) = model {
        let configs = m
            .volumes
            .iter()
            .map(|v| &v.config)
            .chain(m.height_ranges.iter().map(|r| &r.config));
        for cfg in configs {
            if crate::texture::KEYS.iter().any(|k| cfg.get(k).is_some()) {
                list.push(crate::texture::TextureSettings::from_lookup(|k| {
                    cfg.raw(k).or_else(|| c.raw(k))
                })?);
            }
        }
    }
    let refs: Vec<_> = list.iter().collect();
    crate::texture_skin::TextureSkin::prepare_with_walls(
        mesh,
        &refs,
        model.map(|(m, _)| m),
        zs,
        walls,
    )
}

/// [`prepare_with_regions`] with the mesh as loaded before placement, for seam visibility.
pub(crate) fn prepare_with_source(
    mesh: &TriangleMesh,
    s: &PrintSettings,
    painting: &dyn crate::seam::painting::SeamPainting,
    model: Option<(&crate::regions::RegionModel, &crate::config::Config)>,
    frame: Option<&crate::input::Frame>,
    observer: &dyn crate::progress::Observer,
) -> Result<PreparedObject, crate::support::SupportError> {
    let check = |o: &dyn crate::progress::Observer| {
        crate::progress::check(o).map_err(crate::support::SupportError)
    };
    crate::safety::mesh(mesh).map_err(crate::support::SupportError)?;
    crate::safety::settings(s).map_err(crate::support::SupportError)?;
    // Textures on all surfaces: the object is sliced from its displaced mesh (cached), while
    // painting, the wall texture's normals and the texture anchor keep the mesh as loaded.
    let displaced = crate::texture::displace::for_object(mesh, s, model, observer)
        .map_err(crate::support::SupportError)?;
    let loaded = mesh;
    let flagged;
    let s = if displaced.is_some() {
        flagged = PrintSettings {
            textured_mesh: true,
            ..s.clone()
        };
        &flagged
    } else {
        s
    };
    let (mesh, frame) = match &displaced {
        Some(d) => (&d.mesh, None),
        None => (mesh, frame),
    };
    let displaced_volumes = displaced.as_ref().map(|d| d.volumes.as_slice());
    let (_, bounds) = mesh.bounds();
    if f64::from(bounds[2]) / s.layer_height > 100_000. {
        return Err(crate::support::SupportError(
            "print exceeds 100,000 layers; check units/layer height".into(),
        ));
    }
    let support_config = crate::support::SupportConfig::from(s);
    let support_params = crate::support::SupportParameters::new(&support_config)?;
    let (min, max) = mesh.bounds();
    // Infill patterns are aligned to the object's centre, like PrusaSlicer's object coordinates.
    let origin = Point::from_mm(
        (f64::from(min[0]) + f64::from(max[0])) / 2.0,
        (f64::from(min[1]) + f64::from(max[1])) / 2.0,
    );
    let mut slicing_settings = s.clone();
    if s.raft_layers > 0 {
        slicing_settings.first_layer_height = s.layer_height;
    }
    let mut tops = if let Some((model, c)) = model {
        crate::regions::layer_tops(model, c, &slicing_settings, f64::from(max[2]))
            .map_err(crate::support::SupportError)?
    } else {
        slicing_settings.layer_tops(f64::from(max[2]))
    };
    let mut heights: Vec<f64> = tops
        .iter()
        .enumerate()
        .map(|(i, &z)| if i == 0 { z } else { z - tops[i - 1] })
        .collect();
    let mut mid_zs: Vec<f64> = tops
        .iter()
        .zip(&heights)
        // Rounded to the nanometre grid, so planes sit exactly where meant (0.2·22 + 0.1 would
        // otherwise be 4.499999999999999, a hair below vertices at 4.5).
        .map(|(z, h)| ((z - h / 2.0) * 1e9).round() / 1e9)
        .collect();

    let painted_multimaterial = model.is_some_and(|(m, c)| {
        c.vector_len("filament_diameter") > 1
            && m.volumes.iter().any(|v| !v.painting.material.is_empty())
    });
    // The supports need only the object outlines and support masks, which do not depend on
    // the material segmentation. Unless small-feature compensation reshapes outlines and
    // regions together, the outlines are sliced first and the regions are sliced next to
    // the supports (same outlines and regions as slicing everything at once).
    let deferred =
        model.filter(|_| painted_multimaterial || !s.small_features.has_xy_compensation());
    let region_zs = deferred.map(|_| mid_zs.clone());
    let mut regional = model
        .map(|(m, c)| {
            if deferred.is_some() {
                m.slice_outlines(&mid_zs, c, observer, displaced_volumes)
            } else {
                m.slice(&mid_zs, c, observer, displaced_volumes)
            }
        })
        .transpose()
        .map_err(crate::support::SupportError)?;
    let (mut outlines, dropped_chains) = if let Some(r) = &mut regional {
        (std::mem::take(&mut r.outlines), r.dropped_chains)
    } else {
        // PrusaSlicer profiles slice the mesh as loaded, in the float object frame, and
        // Mica moves the result back to the bed by the same `origin` the walls subtract again.
        let prusa_frame = s
            .seam
            .prusa_parity
            .then(|| frame.and_then(|f| crate::slice::prusa_object_mesh(f, mesh)))
            .flatten();
        let slices = if let Some(object_mesh) = &prusa_frame {
            let mut slices = crate::slice::slice_mesh_observed(
                object_mesh,
                &mid_zs,
                s.slice_closing_radius,
                crate::slice::SliceRounding::PrusaScaled,
                observer,
            );
            for layer in &mut slices {
                for island in &mut layer.islands {
                    for p in island
                        .contour
                        .iter_mut()
                        .chain(island.holes.iter_mut().flatten())
                    {
                        p.x += origin.x;
                        p.y += origin.y;
                    }
                }
            }
            slices
        } else {
            if displaced.is_some() {
                crate::texture::displace::slice(mesh, &mid_zs, s.slice_closing_radius, observer)
            } else {
                crate::slice::slice_mesh_observed(
                    mesh,
                    &mid_zs,
                    s.slice_closing_radius,
                    // Without the loaded mesh Mica's vertices are not PrusaSlicer's, and its
                    // edge-crossing rounding (`floor(p + 1)`) would flip on near-integer noise.
                    crate::slice::SliceRounding::Nearest,
                    observer,
                )
            }
        };
        check(observer)?;
        let dropped = slices.iter().map(|l| l.dropped_chains).sum();
        let mut outlines: Vec<Vec<ExPolygon>> = slices.into_iter().map(|l| l.islands).collect();
        // Spiral vase: above the bottom layers the object is sliced to its largest outline,
        // holes closed (slice_volumes' PositiveLargestContour).
        if s.spiral_vase {
            let from = crate::spiral_vase::slice_from_layer(s, &mid_zs);
            for islands in outlines.iter_mut().skip(from) {
                *islands = crate::spiral_vase::largest_contour(islands);
            }
        }
        (outlines, dropped)
    };
    // Negative volumes can remove the top of a model entirely.
    while outlines.last().is_some_and(Vec::is_empty) {
        outlines.pop();
        tops.pop();
        heights.pop();
        mid_zs.pop();
        if let Some(r) = &mut regional {
            r.layers.pop();
            r.enforcers.pop();
            r.blockers.pop();
        }
    }
    let report = SliceReport {
        layers: outlines.len(),
        dropped_chains,
    };
    if s.xy_size_compensation < 0.0 || (regional.is_none() && s.xy_size_compensation > 0.0) {
        outlines.par_iter_mut().for_each(|islands| {
            *islands = offset_ex(islands, scale(s.xy_size_compensation) as f64, Join::Miter);
        });
    }
    if !painted_multimaterial && s.small_features.has_xy_compensation() {
        for (i, islands) in outlines.iter_mut().enumerate() {
            let regions = regional.as_mut().map(|r| r.layers[i].as_mut_slice());
            crate::small_features::compensate_layer(islands, regions, &s.small_features);
        }
    }
    let uncompensated_first_layer = outlines.first().cloned().unwrap_or_default();
    let foot_compensated = s.raft_layers == 0 && s.elefant_foot_compensation > 0.0;
    if foot_compensated && let Some(first) = outlines.first_mut() {
        let external = LayerFlows::new(s, 0, heights[0]).external;
        *first = union_ex(&elephant_foot_compensation(
            first,
            external,
            s.elefant_foot_compensation,
        ));
    }
    let seam_input = crate::seam::PlanInput {
        // Seam visibility from the mesh as loaded: a displaced mesh has many times the facets
        // and the same visibility at the scale seams are chosen.
        mesh: loaded,
        source: frame.map(|f| f.source.as_ref()),
        first_layer_uncompensated: &uncompensated_first_layer,
        outlines: &outlines,
        first_layer_compensation: if s.raft_layers == 0 {
            s.elefant_foot_compensation
        } else {
            0.0
        },
        slice_zs: &mid_zs,
        heights: &heights,
        settings: &s.seam,
        painting,
    };
    for z in &mut tops {
        *z += support_params.object_z_offset;
    }
    // Supports read the outlines and the support painting masks; seam planning reads the
    // outlines and the painting; region preparation reads the region slices, outlines and
    // layer heights. None depends on another's result, so all three share the pool instead of
    // running back to back (the tree supports used to hold the walls back by seconds).
    let mut regional = regional.take();
    let masks = regional.as_mut().map(|r| {
        (
            std::mem::take(&mut r.enforcers),
            std::mem::take(&mut r.blockers),
        )
    });
    let fuzzy_zs = mid_zs.clone();
    // Supports see the first layer without the elephant foot compensation: PrusaSlicer
    // (PrintObject::slice_volumes) and Orca store the uncompensated slices back into the
    // first layer's lslices, which the support generators read.
    let support_outlines: std::borrow::Cow<'_, [Vec<ExPolygon>]> = if foot_compensated
        && !outlines.is_empty()
        && (support_config.enabled || support_config.enforce_layers > 0)
    {
        let mut o = outlines.clone();
        o[0] = uncompensated_first_layer.clone();
        std::borrow::Cow::Owned(o)
    } else {
        std::borrow::Cow::Borrowed(&outlines)
    };
    // While generated supports still run, region preparation may go on to the layer
    // toolpaths the tool-usage prepass generates next (`PreparedRegions::warm`).
    let generated_supports = support_config.enabled || support_config.enforce_layers > 0;
    let supports_busy = std::sync::atomic::AtomicBool::new(generated_supports);
    struct Finished<'a>(&'a std::sync::atomic::AtomicBool);
    impl Drop for Finished<'_> {
        fn drop(&mut self) {
            self.0.store(false, std::sync::atomic::Ordering::Relaxed);
        }
    }
    let (supported, (seams, regions)) = rayon::join(
        || {
            let _finished = Finished(&supports_busy);
            let _timer = crate::timing::Span::new(crate::timing::Stage::Supports);
            let supports = || {
                crate::support::schedule::prepare_with_masks_observed(
                    &tops,
                    &heights,
                    &support_outlines,
                    &support_config,
                    origin,
                    masks
                        .as_ref()
                        .map(|(enforcers, blockers)| (enforcers.as_slice(), blockers.as_slice())),
                    observer,
                )
            };
            // Generated supports get their own workers (see `on_side_pool`).
            if generated_supports {
                crate::geometry::clip_cache::on_side_pool(supports)
            } else {
                supports()
            }
        },
        || {
            let (seams, mut regions) = rayon::join(
                || crate::seam::SeamPlan::prepare_observed(seam_input, observer),
                || {
                    // The texture context (surface normals per layer, texture paint) is built
                    // next to the supports and seams; the regions take it along.
                    let texture = prepare_texture(
                        loaded,
                        s,
                        model,
                        &fuzzy_zs,
                        displaced.as_ref().and_then(|d| d.walls.clone()),
                    )?;
                    let regional = match (deferred, &region_zs) {
                        (Some((m, c)), Some(zs)) => {
                            let mut r = m.slice(zs, c, observer, displaced_volumes)?;
                            // As slicing everything at once: outlines and masks were taken
                            // above, and the layers the outlines lost at the top are dropped.
                            r.outlines = Vec::new();
                            r.enforcers = Vec::new();
                            r.blockers = Vec::new();
                            r.layers.truncate(outlines.len());
                            Some(r)
                        }
                        _ => regional,
                    };
                    let prepared = regional.map(|mut r| {
                        if displaced.is_some() {
                            for region in &mut r.table {
                                region.settings.textured_mesh = true;
                                for (_, by_tool) in &mut region.by_tool {
                                    by_tool.textured_mesh = true;
                                }
                            }
                        }
                        crate::regions::PreparedRegions::prepare(
                            r,
                            mesh,
                            &outlines,
                            &tops,
                            &heights,
                            origin,
                            crate::painting::query::FuzzyPainting {
                                query: model
                                    .map(|(m, _)| {
                                        crate::painting::query::PaintedQuery::new(m, true)
                                    })
                                    .unwrap_or_default(),
                                zs: fuzzy_zs.clone(),
                                texture: texture.clone(),
                            },
                            observer,
                        )
                    });
                    // An object without regions classifies its surfaces here, beside the
                    // seams, which they do not depend on.
                    let plain = prepared.is_none().then(|| {
                        PlainParts {
                            settings: s,
                            outlines: &outlines,
                            tops: &tops,
                            heights: &heights,
                            origin,
                            texture: texture.as_ref(),
                        }
                        .surfaces(mesh, observer)
                    });
                    Ok::<_, String>((texture, prepared, plain))
                },
            );
            if let Ok((_, Some(prepared), _)) = &mut regions {
                prepared.warm(
                    &outlines,
                    &tops,
                    &heights,
                    origin,
                    &seams,
                    &supports_busy,
                    observer,
                );
            }
            (seams, regions)
        },
    );
    // A region slicing error comes first, as when the regions were sliced before supports.
    let (texture, regions, plain) = regions.map_err(crate::support::SupportError)?;
    let (supports, print_layers) = supported?;
    check(observer)?;
    let brim_ears = model.map_or_else(Vec::new, |(m, _)| m.brim_ears.clone());
    let adhesion = crate::brim::prepare::object_plan(
        &outlines,
        &uncompensated_first_layer,
        &tops,
        &supports,
        &print_layers,
        s,
        f64::from(max[2] - min[2]),
        &brim_ears,
    );
    let first_layer_bbox = adhesion.first_layer_bbox();
    check(observer)?;
    let mut obj = PreparedObject {
        adhesion,
        seams,
        supports,
        print_layers,
        settings: s.clone(),
        regions,
        infill: crate::fill::PreparedFill::default(),
        origin,
        tops,
        heights,
        outlines,
        surfaces: Vec::new(),
        uncompensated_first_layer,
        first_layer_bbox,
        report,
        brim_ears,
        texture,
        displacement_warnings: displaced
            .as_ref()
            .map(|d| d.warnings.clone())
            .unwrap_or_default(),
        brick: None,
        plain_walls: Vec::new(),
        spiral_starts: Vec::new(),
    };
    if let Some(plain) = plain {
        (obj.surfaces, obj.infill, obj.plain_walls) = plain?;
    }
    obj.brick = obj.brick_plan();
    obj.chain_spiral_starts();
    Ok(obj)
}

impl PreparedObject {
    /// Drops layer toolpaths generated ahead of the tool-usage prepass that it did not use.
    pub(crate) fn drop_warm_layers(&self) {
        if let Some(regions) = &self.regions {
            regions.drop_warm();
        }
    }

    /// The brick-layers plan of this object (`None` with brick layers off).
    fn brick_plan(&self) -> Option<crate::brick_layers::Plan> {
        if !self.settings.brick.enabled {
            return None;
        }
        let mut settings = vec![&self.settings];
        if let Some(r) = &self.regions {
            settings.extend(r.table.iter().map(|t| &t.settings));
        }
        // Supports on their own layer heights print between the object's layers.
        let mut seen_object = false;
        let independent_support_layers = self.print_layers.iter().any(|l| {
            seen_object |= l.object.is_some();
            seen_object && l.object.is_none() && l.support.is_some()
        });
        let first_layer = (self.settings.raft_layers == 0 && !self.outlines.is_empty())
            .then_some(self.uncompensated_first_layer.as_slice());
        crate::brick_layers::Plan::build(crate::brick_layers::PlanInput {
            outlines: &self.outlines,
            first_layer,
            regions: self.regions.as_ref().map(|r| {
                r.layers
                    .iter()
                    .map(|l| l.iter().map(|r| (r.region, r.slices.as_slice())).collect())
                    .collect()
            }),
            tops: &self.tops,
            heights: &self.heights,
            settings,
            independent_support_layers,
        })
    }

    /// What the slice result says about brick layers on this object.
    pub fn brick_warnings(&self) -> Vec<String> {
        self.brick
            .as_ref()
            .filter(|b| !b.suppressed())
            .map_or_else(Vec::new, |b| b.warnings.clone())
    }

    /// Slices this object without brick layers from now on (a print with several tools);
    /// whether brick layers would have raised any wall.
    pub fn suppress_brick_layers(&self) -> bool {
        self.brick.as_ref().is_some_and(|b| {
            b.suppress();
            b.raises_anything()
        })
    }

    /// The brick-layers plan, for tests and diagnostics.
    pub fn brick_layers(&self) -> Option<&crate::brick_layers::Plan> {
        self.brick.as_ref()
    }

    /// Applies the brick-layers plan to object layer `i`'s paths (and their region ids).
    fn apply_brick<T: Clone>(
        &self,
        i: usize,
        layer: &mut LayerToolpaths,
        ids: Option<&mut Vec<T>>,
    ) {
        if let Some(plan) = &self.brick {
            plan.apply(i, &mut layer.paths, ids);
        }
    }

    /// What the slice result says about this object's textures (depths Mica limited or widens).
    pub fn texture_warnings(&self) -> Vec<String> {
        if self.texture.is_none() {
            return self.displacement_warnings.clone();
        }
        let mut settings = vec![&self.settings];
        if let Some(r) = &self.regions {
            settings.extend(r.table.iter().map(|t| &t.settings));
        }
        let height = self
            .heights
            .get(1)
            .copied()
            .unwrap_or(self.settings.layer_height);
        let mut out: Vec<String> = settings
            .into_iter()
            .flat_map(|s| {
                crate::texture_skin::warnings(
                    &s.texture_skin,
                    &LayerFlows::new(s, 1, height).external,
                    s.perimeter_generator == crate::settings::PerimeterGenerator::Arachne,
                    s.perimeters,
                )
            })
            .collect();
        out.extend(self.displacement_warnings.iter().cloned());
        out.sort();
        out.dedup();
        out
    }

    pub fn has_regions(&self) -> bool {
        self.regions.is_some()
    }

    /// Layer regions by output index. Support-only layers have no model regions.
    pub fn layer_regions(&self, index: usize) -> Option<&[crate::regions::LayerRegion]> {
        let i = self.print_layers.get(index)?.object?;
        Some(&self.regions.as_ref()?.layers[i])
    }

    pub fn settings(&self) -> &PrintSettings {
        &self.settings
    }

    pub fn region(&self, id: usize) -> Option<&crate::regions::PrintRegion> {
        self.regions.as_ref()?.table.get(id)
    }

    pub fn region_count(&self) -> usize {
        self.regions.as_ref().map_or(1, |r| r.table.len())
    }

    /// Region identity per path, computed with the same bounded batch as the geometry.
    pub fn regional_toolpaths(
        &self,
        range: std::ops::Range<usize>,
    ) -> Vec<(LayerToolpaths, Vec<Option<usize>>)> {
        let Some(regions) = &self.regions else {
            return self
                .toolpaths(range)
                .into_iter()
                .map(|l| (l, vec![]))
                .collect();
        };
        range
            .into_par_iter()
            .map(|i| {
                let pl = self.print_layers[i];
                let mut ids = vec![];
                let mut layer = crate::support::schedule::toolpaths(i, pl, &self.supports, |j| {
                    let (mut l, mut r) = regions.layer(
                        j,
                        &self.outlines,
                        &self.tops,
                        &self.heights,
                        self.origin,
                        &self.seams,
                    );
                    self.apply_brick(j, &mut l, Some(&mut r));
                    ids = r;
                    l
                });
                let prefix = layer.paths.len() - ids.len();
                ids.splice(0..0, std::iter::repeat_n(None, prefix));
                let before = layer.paths.len();
                crate::brim::prepare::prepend(&self.adhesion, &self.print_layers, &mut layer);
                ids.splice(0..0, std::iter::repeat_n(None, layer.paths.len() - before));
                (layer, ids)
            })
            .collect()
    }

    /// Tree graph diagnostics before first-layer flange expansion.
    pub fn tree_support_statistics(&self) -> Option<crate::support::TreeStatistics> {
        self.supports.tree_statistics
    }

    /// Walls of every island of layer `i`: those the surface classification generated, on
    /// first use, otherwise generated again (the same walls).
    fn walls(&self, i: usize) -> Vec<(ExPolygon, IslandWalls)> {
        if let Some(walls) = self
            .plain_walls
            .get(i)
            .and_then(|w| w.lock().unwrap_or_else(|p| p.into_inner()).take())
        {
            return walls;
        }
        PlainParts {
            settings: &self.settings,
            outlines: &self.outlines,
            tops: &self.tops,
            heights: &self.heights,
            origin: self.origin,
            texture: self.texture.as_ref(),
        }
        .walls(i)
    }
}

/// One layer's walls kept from the surface classification of an object without regions,
/// until its toolpaths take them.
type PlainWalls = std::sync::Mutex<Option<Vec<(ExPolygon, IslandWalls)>>>;

/// What the walls and surfaces of an object without regions are made from, borrowed before
/// the object is assembled (they are prepared beside the seams).
struct PlainParts<'a> {
    settings: &'a PrintSettings,
    outlines: &'a [Vec<ExPolygon>],
    tops: &'a [f64],
    heights: &'a [f64],
    origin: Point,
    texture: Option<&'a std::sync::Arc<crate::texture_skin::TextureSkin>>,
}

impl PlainParts<'_> {
    /// Walls of every island of layer `i`.
    fn walls(&self, i: usize) -> Vec<(ExPolygon, IslandWalls)> {
        let s = self.settings;
        if let Some(t) = self.texture {
            return make_region_walls_with_painting(
                &self.outlines[i],
                self.outlines,
                s,
                i,
                self.tops[i],
                self.heights[i],
                self.origin,
                &crate::texture_skin::Painting(t),
            );
        }
        make_region_walls(
            &self.outlines[i],
            self.outlines,
            s,
            i,
            self.tops[i],
            self.heights[i],
            self.origin,
        )
    }

    /// Typed surfaces and prepared infill of every layer: [`PlainParts::classify_surfaces`],
    /// then the infill preparation and combination, as the object's surfaces.
    fn surfaces(
        &self,
        mesh: &TriangleMesh,
        observer: &dyn crate::progress::Observer,
    ) -> Result<
        (
            Vec<FillSurfaces>,
            crate::fill::PreparedFill,
            Vec<PlainWalls>,
        ),
        crate::support::SupportError,
    > {
        use crate::progress::Stage;
        let check = |o: &dyn crate::progress::Observer| {
            crate::progress::check(o).map_err(crate::support::SupportError)
        };
        let s = self.settings;
        let (mut surfaces, walls) = self.classify_surfaces(observer);
        check(observer)?;
        observer.progress(Stage::Surfaces, 0.7);
        let mut infill = crate::fill::PreparedFill::prepare(mesh, s, &surfaces, self.origin);
        check(observer)?;
        observer.progress(Stage::Surfaces, 0.9);
        infill.finish_surfaces(&mut surfaces, self.tops, self.heights, s, self.origin);
        crate::regions::combine::combine_infill(&mut surfaces, self.heights, s);
        observer.progress(Stage::Surfaces, 1.);
        Ok((surfaces, infill, walls))
    }

    /// Top / bottom / solid / sparse classification of every layer's infill area (needs all layers'
    /// walls once), and those walls, kept for the layer's toolpaths.
    fn classify_surfaces(
        &self,
        observer: &dyn crate::progress::Observer,
    ) -> (Vec<FillSurfaces>, Vec<PlainWalls>) {
        let s = self.settings;
        let n = self.outlines.len();
        // Walls of every layer are the long part (0 .. 0.6 of the stage); layers after a
        // cancel are skipped, the caller discards the result.
        let ticker =
            crate::progress::Ticker::span(observer, crate::progress::Stage::Surfaces, n, 0., 0.6);
        let (fill, walls): (Vec<Vec<ExPolygon>>, Vec<_>) = (0..n)
            .into_par_iter()
            .map(|i| {
                if ticker.cancelled() {
                    return (Vec::new(), std::sync::Mutex::new(None));
                }
                ticker.tick();
                let walls = self.walls(i);
                let fill = walls
                    .iter()
                    .flat_map(|(_, w)| w.infill_area.iter().cloned())
                    .collect();
                (fill, std::sync::Mutex::new(Some(walls)))
            })
            .unzip();
        let layers: Vec<ShellLayer> = (0..n)
            .map(|i| {
                let f = LayerFlows::new(s, i, self.heights[i]);
                ShellLayer {
                    print_z: self.tops[i],
                    height: self.heights[i],
                    external: f.external,
                    perimeter: f.perimeter,
                    solid: f.solid,
                }
            })
            .collect();
        (
            classify(self.outlines, fill, &layers, s, self.origin),
            walls,
        )
    }
}

impl PreparedObject {
    /// Spiral vase: the start of each bottom layer's toolpaths, generated in order from the
    /// end of the one below ([`PreparedObject::spiral_starts`]). A raft is not accounted for.
    fn chain_spiral_starts(&mut self) {
        let s = &self.settings;
        if !s.spiral_vase || s.raft_layers > 0 {
            return;
        }
        let n = self.tops.len();
        let bottom = (0..n)
            .find(|&i| crate::spiral_vase::spiral_layer(s, i))
            .unwrap_or(n);
        let mut pos = Point::default();
        for i in 0..bottom {
            self.spiral_starts.push(pos);
            if let Some(regions) = &mut self.regions {
                regions.starts.push(pos);
            }
            let layer = self.layer(i);
            if let Some(end) = layer.paths.last().and_then(|p| p.points.last()) {
                pos = *end;
            }
        }
    }

    /// Infill areas of layer `i` by type, indexed for lookup per island.
    fn fill_areas(&self, i: usize) -> FillAreas {
        FillAreas::new(&self.surfaces[i])
    }

    /// Finished toolpaths of layer `i`.
    fn layer(&self, i: usize) -> LayerToolpaths {
        if let Some(regions) = &self.regions {
            let mut layer = regions
                .layer(
                    i,
                    &self.outlines,
                    &self.tops,
                    &self.heights,
                    self.origin,
                    &self.seams,
                )
                .0;
            self.apply_brick::<()>(i, &mut layer, None);
            return layer;
        }
        let s = &self.settings;
        let walls = self.walls(i);
        let areas = self.fill_areas(i);
        let f = LayerFlows::new(s, i, self.heights[i]);
        let mut paths = Vec::new();
        let mut pos = self.spiral_starts.get(i).copied().unwrap_or_default();
        let mut remaining: Vec<&(ExPolygon, IslandWalls)> = walls.iter().collect();
        if !s.adhesion.orca {
            // Layer::make_slices: lslice_indices_sorted_by_print_order = chain_expolygons(lslices).
            let firsts: Vec<Point> = walls
                .iter()
                .map(|(e, _)| e.contour.first().copied().unwrap_or_default())
                .collect();
            // Reversed: islands are taken from the back.
            remaining = crate::shortest_path::chain_points(&firsts, None)
                .into_iter()
                .rev()
                .map(|k| &walls[k])
                .collect();
        }
        let mut ironing = if s.adhesion.orca {
            crate::ironing::collections(
                &self.outlines[i],
                self.outlines.get(i + 1).map(Vec::as_slice),
                i.checked_sub(1).map(|j| self.outlines[j].as_slice()),
                &self.surfaces[i],
                i,
                self.tops[i],
                self.origin,
                s,
            )
        } else {
            vec![]
        };
        while !remaining.is_empty() {
            let (island, island_walls) = if s.adhesion.orca {
                // Next island: the one whose outline starts closest to where we are.
                let idx = (0..remaining.len())
                    .min_by_key(|&k| {
                        remaining[k]
                            .0
                            .contour
                            .iter()
                            .map(|p| p.distance_sq(pos))
                            .min()
                            .unwrap_or(i128::MAX)
                    })
                    .unwrap();
                remaining.swap_remove(idx)
            } else {
                remaining.pop().unwrap()
            };
            let layer = LayerInfo {
                lower: i.checked_sub(1).map(|j| self.outlines[j].as_slice()),
                index: i,
                print_z: self.tops[i],
                origin: self.origin,
                seams: &self.seams,
                infill: &self.infill,
            };
            island_toolpaths(island_walls, &areas, &layer, s, &f, &mut paths, &mut pos);
            if s.adhesion.orca {
                crate::ironing::append_orca_island(
                    &mut ironing,
                    Some(island),
                    &mut paths,
                    &mut pos,
                );
            }
        }
        if s.adhesion.orca {
            crate::ironing::append_orca_island(&mut ironing, None, &mut paths, &mut pos);
        } else {
            paths.extend(crate::ironing::paths(
                &self.outlines[i],
                self.outlines.get(i + 1).map(Vec::as_slice),
                i.checked_sub(1).map(|j| self.outlines[j].as_slice()),
                &self.surfaces[i],
                i,
                self.tops[i],
                self.origin,
                s,
            ));
        }
        let mut layer = LayerToolpaths {
            support_groups: Vec::new(),
            index: i,
            print_z: self.tops[i],
            height: self.heights[i],
            paths,
        };
        self.apply_brick::<()>(i, &mut layer, None);
        layer
    }

    /// Computes every layer at once (tests, small prints).
    pub fn into_sliced(mut self) -> SlicedObject {
        let layers = self.toolpaths(0..self.layer_count());
        if self.settings.raft_layers == 0 && !self.outlines.is_empty() {
            self.outlines[0] = std::mem::take(&mut self.uncompensated_first_layer);
        }
        SlicedObject {
            adhesion: self.adhesion,
            outlines: self
                .print_layers
                .iter()
                .map(|layer| {
                    layer
                        .object
                        .map(|i| std::mem::take(&mut self.outlines[i]))
                        .unwrap_or_default()
                })
                .collect(),
            layers,
            first_layer_bbox: self.first_layer_bbox,
            report: self.report,
        }
    }
}

impl LayerSource for PreparedObject {
    fn first_layer_hull(&self) -> Option<&[Point]> {
        (!self.adhesion.first_layer_hull.is_empty())
            .then_some(self.adhesion.first_layer_hull.as_slice())
    }
    fn layer_outlines(&self, index: usize) -> Option<&[ExPolygon]> {
        let object = self.print_layers.get(index)?.object?;
        // Prusa's lslices retain the uncompensated first-layer boundary. Dynamic
        // overhang speeds must not interpret elephant-foot compensation as an overhang.
        if object == 0 && self.settings.raft_layers == 0 {
            Some(&self.uncompensated_first_layer)
        } else {
            self.outlines.get(object).map(Vec::as_slice)
        }
    }
    fn layer_top_surfaces(&self, index: usize) -> Option<&[ExPolygon]> {
        let object = self.print_layers.get(index)?.object?;
        self.surfaces.get(object).map(|s| s.top.as_slice())
    }
    fn support_islands(&self, index: usize) -> Option<Vec<ExPolygon>> {
        let layer = self
            .supports
            .layers
            .get(self.print_layers.get(index)?.support?)?;
        let areas: Vec<ExPolygon> = layer
            .regions
            .iter()
            .flat_map(|r| r.polygons.iter().cloned())
            .collect();
        (!areas.is_empty()).then(|| crate::geometry::clip::union_ex(&areas))
    }
    fn region_slices(&self, index: usize) -> Option<Vec<Vec<ExPolygon>>> {
        let regions = self.layer_regions(index)?;
        let table = &self.regions.as_ref()?.table;
        // Orca's print regions are per configuration and extruder: paint in a region's own
        // extruder does not split it.
        let mut keys = Vec::new();
        let mut groups: Vec<Vec<ExPolygon>> = Vec::new();
        for r in regions {
            let tool = r
                .painted_tool
                .map(|t| usize::from(t).saturating_sub(1))
                .or_else(|| table.get(r.region)?.tools.tool(Role::Perimeter));
            let key = (r.region, tool);
            match keys.iter().position(|k| *k == key) {
                Some(i) => groups[i].extend_from_slice(&r.slices),
                None => {
                    keys.push(key);
                    groups.push(r.slices.clone());
                }
            }
        }
        (groups.len() > 1).then(|| {
            groups
                .iter()
                .map(|slices| crate::geometry::clip::union_ex(slices))
                .collect()
        })
    }
    fn object_origin(&self, _object: usize) -> Option<Point> {
        Some(self.origin)
    }
    fn layer_count(&self) -> usize {
        self.print_layers.len()
    }

    fn layer_z(&self, i: usize) -> (f64, f64) {
        (self.print_layers[i].print_z, self.print_layers[i].height)
    }

    fn first_layer_bbox(&self) -> Option<BoundingBox> {
        self.first_layer_bbox
    }

    fn toolpaths(&self, range: std::ops::Range<usize>) -> Vec<LayerToolpaths> {
        range
            .into_par_iter()
            .map(|i| {
                let mut layer = crate::support::schedule::toolpaths(
                    i,
                    self.print_layers[i],
                    &self.supports,
                    |object| self.layer(object),
                );
                crate::brim::prepare::prepend(&self.adhesion, &self.print_layers, &mut layer);
                layer
            })
            .collect()
    }
}

impl LayerSource for SlicedObject {
    fn first_layer_hull(&self) -> Option<&[Point]> {
        (!self.adhesion.first_layer_hull.is_empty())
            .then_some(self.adhesion.first_layer_hull.as_slice())
    }
    fn layer_outlines(&self, index: usize) -> Option<&[ExPolygon]> {
        self.outlines
            .get(index)
            .filter(|o| !o.is_empty())
            .map(Vec::as_slice)
    }
    fn layer_count(&self) -> usize {
        self.layers.len()
    }

    fn layer_z(&self, i: usize) -> (f64, f64) {
        (self.layers[i].print_z, self.layers[i].height)
    }

    fn first_layer_bbox(&self) -> Option<BoundingBox> {
        self.first_layer_bbox
    }

    fn toolpaths(&self, range: std::ops::Range<usize>) -> Vec<LayerToolpaths> {
        self.layers[range].to_vec()
    }
}

/// Slices a mesh that is already placed on the bed (resting on z = 0), computing every layer.
pub fn slice_object(
    mesh: &TriangleMesh,
    s: &PrintSettings,
) -> Result<SlicedObject, crate::support::SupportError> {
    Ok(prepare_object(mesh, s)?.into_sliced())
}

/// Opens a closed Arachne wall run at the seam the seam plan chooses for it, and shortens its end by the
/// seam gap, so variable-width walls get the same seams as classic loops. Open runs pass through.
/// Scarf samples preserve the flow of each width run.
fn seam_arachne_run(
    run: &[ExtrusionPath],
    layer: &LayerInfo<'_>,
    s: &PrintSettings,
    pos: Point,
) -> Vec<ExtrusionPath> {
    let (Some(first), Some(last)) = (run.first(), run.last()) else {
        return Vec::new();
    };
    let closed = first.points.first() == last.points.last()
        && run.iter().map(|p| p.points.len()).sum::<usize>() > 3;
    if !closed {
        return run.to_vec();
    }
    // The loop's outline: every path's points, without the repeated joints and the closing point.
    let mut polygon: Polygon = Vec::new();
    for p in run {
        for &q in &p.points {
            if polygon.last() != Some(&q) {
                polygon.push(q);
            }
        }
    }
    polygon.pop();
    let wall = crate::perimeters::WallLoop {
        is_contour: crate::geometry::signed_area(&polygon) > 0.0,
        depth: if first.role == Role::ExternalPerimeter {
            0
        } else {
            1
        },
        polygon,
    };
    let Some(opened) =
        layer
            .seams
            .open_loop_supported(layer.index, &wall, first.flow, s, pos, layer.lower)
    else {
        return run.to_vec();
    };
    if run.len() == 1 {
        // A uniform loop can use the projected seam directly. Snapping to an existing
        // vertex changes the seam on curved walls and the subsequent travel path.
        let mut whole = first.clone();
        whole.points = opened.points;
        whole.seam = opened.seam;
        return vec![whole];
    }
    if opened.seam.as_ref().is_some_and(|m| !m.samples.is_empty()) {
        return crate::seam::split_variable(&opened, run);
    }
    let reversed_run;
    let run = if s.orca_walls.overhang_reverse
        && (crate::geometry::signed_area(&wall.polygon) < 0.) != s.seam.prefer_clockwise_movements
    {
        reversed_run = run
            .iter()
            .rev()
            .map(|p| {
                let mut p = p.clone();
                p.points.reverse();
                p
            })
            .collect::<Vec<_>>();
        &reversed_run
    } else {
        run
    };
    let seam = opened.points[0];
    let (best_path, head_points, tail_points) = if s.seam.prusa_parity {
        // SmoothPathCache::resolve_or_fit_split_with_seam: split the run at the seam's
        // projection, merging with a vertex closer than 1.5 µm.
        let (mut best, mut best_d) = ((0, 0, seam), f64::INFINITY);
        for (pi, p) in run.iter().enumerate() {
            for (qi, w) in p.points.windows(2).enumerate() {
                let (q, d) = closest_on_segment(w[0], w[1], seam);
                if d < best_d {
                    (best, best_d) = ((pi, qi, q), d);
                }
            }
        }
        let (pi, qi, q) = best;
        let points = &run[pi].points;
        let threshold = 1_500i128 * 1_500;
        let (start, end) = (points[qi], points[qi + 1]);
        let d_start = q.distance_sq(start);
        let vertex = if d_start < threshold {
            Some(if q.distance_sq(end) < d_start {
                qi + 1
            } else {
                qi
            })
        } else if q.distance_sq(end) < threshold {
            Some(qi + 1)
        } else {
            None
        };
        match vertex {
            Some(k) => (pi, points[k..].to_vec(), points[..=k].to_vec()),
            None => {
                let mut head = vec![q];
                head.extend_from_slice(&points[qi + 1..]);
                let mut tail = points[..=qi].to_vec();
                tail.push(q);
                (pi, head, tail)
            }
        }
    } else {
        // Rotate the run to start at its vertex nearest the chosen seam, splitting the path containing it.
        let (mut best_path, mut best_point, mut best_d) = (0, 0, i128::MAX);
        for (pi, p) in run.iter().enumerate() {
            for (qi, q) in p.points.iter().enumerate() {
                let d = q.distance_sq(seam);
                if d < best_d {
                    (best_path, best_point, best_d) = (pi, qi, d);
                }
            }
        }
        let points = &run[best_path].points;
        (
            best_path,
            points[best_point..].to_vec(),
            points[..=best_point].to_vec(),
        )
    };
    let split = &run[best_path];
    let mut out = Vec::with_capacity(run.len() + 1);
    let mut head = split.clone();
    head.points = head_points;
    if head.points.len() >= 2 {
        out.push(head);
    }
    out.extend(run[best_path + 1..].iter().cloned());
    out.extend(run[..best_path].iter().cloned());
    let mut tail = split.clone();
    tail.points = tail_points;
    if tail.points.len() >= 2 {
        out.push(tail);
    }
    // Seam gap: stop short of the start so the loop doesn't extrude onto itself (not on a
    // spiral vase layer, whose loop runs on into the next layer).
    let mut gap = if crate::spiral_vase::spiral_layer(s, layer.index) {
        0.0
    } else {
        scale(s.seam.gap.resolve(s.nozzle_diameter, 1.0)) as f64
    };
    while gap > 0.0 {
        let Some(p) = out.last_mut() else { break };
        let len = crate::geometry::polyline_length(&p.points);
        if len <= gap {
            gap -= len;
            out.pop();
        } else {
            clip_end(&mut p.points, gap);
            break;
        }
    }
    if s.small_features.small_perimeter_threshold != 6.5 || s.adhesion.orca {
        for path in &mut out {
            path.seam
                .get_or_insert_with(Default::default)
                .loop_length_mm = opened.seam.as_ref().and_then(|s| s.loop_length_mm);
        }
    }
    // PrusaSlicer's "move inwards before travel" after an external loop: the seam plan computed it on
    // the same outline, seam and gap, so carry it over to the run's last path.
    if let (Some(wipe), Some(last)) = (opened.seam.as_ref().and_then(|m| m.wipe), out.last_mut())
        && last.seam.as_ref().is_none_or(|m| m.wipe.is_none())
    {
        last.seam.get_or_insert_with(Default::default).wipe = Some(wipe);
    }
    out
}

/// Shortens a polyline by `length` (scaled) from its end.
fn clip_end(pts: &mut Polyline, mut length: f64) {
    while pts.len() >= 2 && length > 0.0 {
        let b = pts[pts.len() - 1];
        let a = pts[pts.len() - 2];
        let seg = a.distance_to(b);
        if seg > length {
            let t = (seg - length) / seg;
            let i = pts.len() - 1;
            pts[i] = Point::new(
                a.x + ((b.x - a.x) as f64 * t).round() as i64,
                a.y + ((b.y - a.y) as f64 * t).round() as i64,
            );
            return;
        }
        length -= seg;
        pts.pop();
    }
}

/// ExPolygons with their bounding boxes, so an island can pick out the ones near it cheaply.
pub(crate) struct Indexed {
    items: Vec<ExPolygon>,
    boxes: Vec<BoundingBox>,
}

impl Indexed {
    pub(crate) fn new(items: Vec<ExPolygon>) -> Self {
        let boxes = items
            .iter()
            .map(|e| {
                BoundingBox::from_points(e.contour.iter()).unwrap_or(BoundingBox {
                    min: Point::default(),
                    max: Point::default(),
                })
            })
            .collect();
        Self { items, boxes }
    }

    /// The polygons whose bounding box overlaps `bb`.
    fn near(&self, bb: BoundingBox) -> Vec<ExPolygon> {
        self.items
            .iter()
            .zip(&self.boxes)
            .filter(|(_, b)| {
                b.min.x <= bb.max.x
                    && b.max.x >= bb.min.x
                    && b.min.y <= bb.max.y
                    && b.max.y >= bb.min.y
            })
            .map(|(e, _)| e.clone())
            .collect()
    }
}

/// The infill areas of one layer by type (see [`FillSurfaces`]).
pub(crate) struct FillAreas {
    combined: Vec<crate::regions::combine::CombinedFill>,
    pub(crate) bridges: Vec<crate::bridge::BridgeSurface>,
    pub(crate) internal_bridges: Vec<crate::bridge::BridgeSurface>,
    pub(crate) bottom: Indexed,
    pub(crate) top: Indexed,
    /// Internal solid (shells).
    pub(crate) solid: Indexed,
    /// Sparse infill.
    pub(crate) sparse: Indexed,
}

pub(crate) struct LayerInfo<'a> {
    pub(crate) lower: Option<&'a [ExPolygon]>,
    pub(crate) infill: &'a crate::fill::PreparedFill,
    pub(crate) seams: &'a crate::seam::SeamPlan,
    pub(crate) index: usize,
    pub(crate) print_z: f64,
    /// Object centre, the reference point of the infill patterns.
    pub(crate) origin: Point,
}

/// One island's extrusions: perimeters, then fills (`extract_island_extrusions`); Prusa profiles
/// with `infill_first` print the fills first, both chained on from the previous position.
pub(crate) fn island_toolpaths(
    walls: &IslandWalls,
    areas: &FillAreas,
    layer: &LayerInfo<'_>,
    s: &PrintSettings,
    f: &LayerFlows,
    paths: &mut Vec<ExtrusionPath>,
    pos: &mut Point,
) {
    if s.infill_first && !s.adhesion.orca {
        island_fill_toolpaths(walls, areas, layer, s, f, paths, pos);
        island_wall_toolpaths(walls, layer, s, f, paths, pos);
    } else {
        island_wall_toolpaths(walls, layer, s, f, paths, pos);
        island_fill_toolpaths(walls, areas, layer, s, f, paths, pos);
    }
}

/// The island's perimeters and thin walls (and, with Orca profiles, its gap fill).
fn island_wall_toolpaths(
    walls: &IslandWalls,
    layer: &LayerInfo<'_>,
    s: &PrintSettings,
    f: &LayerFlows,
    paths: &mut Vec<ExtrusionPath>,
    pos: &mut Point,
) {
    let sandwich = s.orca_walls.inner_outer_inner && layer.index > 0;
    let mut loops = crate::perimeter_order::order(
        &walls.loops,
        layer.origin,
        s.external_perimeters_first || sandwich,
    );
    if sandwich {
        crate::wall_options::sandwich(&mut loops, |w| w.depth);
    }
    let reversed = s.orca_walls.reversal(walls, layer.lower, layer.index, s);
    let classic_supported = layer
        .lower
        .filter(|_| s.overhangs && !s.supports_touching && layer.index > 0 && !loops.is_empty())
        .map(|lower| offset_ex(lower, scale(s.nozzle_diameter / 2.0) as f64, Join::Miter));
    for l in loops {
        let flow = if l.depth == 0 {
            f.external
        } else {
            f.perimeter
        };
        let mut direction = s.clone();
        if reversed[usize::from(l.is_contour)] && (!s.orca_walls.internal_only || l.depth > 0) {
            direction.seam.prefer_clockwise_movements = !direction.seam.prefer_clockwise_movements;
        }
        // Classic overhang walls: sections outside the lower layer grown by half a nozzle.
        if let Some(supported) = &classic_supported {
            let role = if l.depth == 0 {
                Role::ExternalPerimeter
            } else {
                Role::Perimeter
            };
            // Orca's classic overhang flow is the bridging flow of bridge_line_width.
            let base = match s.orca_walls.bridge_line_width {
                Some(w) => Flow::new(w.resolve(s.nozzle_diameter, 1.0), f.perimeter.height),
                None => f.perimeter,
            };
            let overhang = crate::perimeters::overhang_flow(s, base);
            if let Some(run) = crate::perimeters::split_classic_overhangs(
                &l.polygon, supported, role, flow, overhang,
            ) {
                for mut path in seam_arachne_run(&run, layer, &direction, *pos) {
                    path.perimeter_index = Some(l.depth);
                    *pos = path.end_position().unwrap_or(*path.points.last().unwrap());
                    paths.push(path);
                }
                continue;
            }
        }
        if let Some(path) =
            layer
                .seams
                .open_loop_supported(layer.index, l, flow, &direction, *pos, layer.lower)
        {
            *pos = path.end_position().unwrap();
            paths.push(path);
        }
    }
    for run in &walls.variable_walls {
        let mut direction = s.clone();
        let polygon: Vec<_> = run.iter().flat_map(|p| p.points.iter().copied()).collect();
        let external = run.iter().any(|p| p.role == Role::ExternalPerimeter);
        if reversed[usize::from(crate::geometry::signed_area(&polygon) > 0.)]
            && (!s.orca_walls.internal_only || !external)
        {
            direction.seam.prefer_clockwise_movements = !direction.seam.prefer_clockwise_movements;
        }
        for path in seam_arachne_run(run, layer, &direction, *pos) {
            *pos = *path.points.last().unwrap();
            paths.push(path);
        }
    }
    // PrusaSlicer orders the island's fills, its gap fill included, as one set
    // (GCode/ExtrusionOrder.cpp); Orca keeps its own nearest-group order.
    let prusa_order = !s.adhesion.orca;
    // Thin walls are printed with the outer walls; gap fill after the walls, before the infill.
    let gap_fill_here: &[Vec<ExtrusionPath>] = if prusa_order { &[] } else { &walls.gap_fill };
    for run in [walls.thin_walls.as_slice(), gap_fill_here] {
        for path in crate::perimeters::chain_runs(run.to_vec(), *pos) {
            *pos = *path.points.last().unwrap();
            paths.push(path);
        }
    }
}

/// The island's fills; with Prusa profiles its gap fill is chained together with them.
fn island_fill_toolpaths(
    walls: &IslandWalls,
    areas: &FillAreas,
    layer: &LayerInfo<'_>,
    s: &PrintSettings,
    f: &LayerFlows,
    paths: &mut Vec<ExtrusionPath>,
    pos: &mut Point,
) {
    let prusa_order = !s.adhesion.orca;
    // Layer::make_fills packs an island's thin fills into one sortable collection after its fills.
    let thin_fills =
        (prusa_order && !walls.gap_fill.is_empty()).then(|| crate::fill::FillCollection {
            entities: walls.gap_fill.clone(),
            no_sort: false,
            multi: true,
        });

    let _timing = crate::timing::Span::new(crate::timing::Stage::Fills);
    // Infill: solid where the layer is near the top or bottom, sparse elsewhere.
    let fill = &walls.infill_area;
    if fill.is_empty() {
        if let Some(thin) = thin_fills {
            let prefer_cw = s.seam.prefer_clockwise_movements;
            for path in crate::extrusion_order::sort_fill_extrusions(vec![thin], *pos, prefer_cw) {
                *pos = path.end_position().unwrap_or(*path.points.last().unwrap());
                paths.push(path);
            }
        }
        return;
    }
    // Only the layer's polygons near this island take part (a layer may have hundreds of islands).
    let Some(fill_bb) = BoundingBox::from_points(fill.iter().flat_map(|e| e.contour.iter())) else {
        return;
    };
    let pick = |areas: &Indexed| {
        let near = areas.near(fill_bb);
        if near.is_empty() {
            near
        } else {
            intersection_ex(fill, &near)
        }
    };
    let bottom_here = pick(&areas.bottom);
    let top_here = pick(&areas.top);
    let internal_solid = pick(&areas.solid);
    let sparse = pick(&areas.sparse);

    // Prusa order: the island's fill collections with their `group_fills` sort keys.
    let mut ranked: Vec<(
        crate::extrusion_order::FillRank,
        crate::fill::FillCollection,
    )> = Vec::new();
    // Orca order: every filled ExPolygon of a surface is one collection of the island's region
    // (`Fill::fill_surface_extrusion`), all of them chained together from the last position
    // (`GCode::extrude_infill`); the flag is the paths' `can_reverse`.
    let mut orca_fills: Vec<(crate::fill::FillCollection, bool)> = Vec::new();
    for (bridges, internal) in [(&areas.bridges, false), (&areas.internal_bridges, true)] {
        for (angle, c) in
            crate::bridge::collections(bridges, fill, s, f.solid, layer.origin, internal)
        {
            if prusa_order {
                ranked.push((crate::extrusion_order::FillRank::bridge(angle), c));
            } else {
                orca_fills.push((c, true));
            }
        }
    }

    // PrusaSlicer's group_fills / make_fills parameters.
    let params = |spacing: f64, density: f64, anchor: f64, anchor_max: f64| FillParams {
        spacing,
        density,
        angle: s.fill_angle,
        layer_id: layer.index,
        z: layer.print_z,
        anchor_length: anchor.min(anchor_max),
        anchor_length_max: anchor_max,
        dont_adjust: false,
        // make_fills computes 3 × spacing before the filler's spacing is set, so this is 0 (unlimited).
        link_max_length: 0.0,
        origin: layer.origin,
    };
    // Solid fills don't limit their anchors.
    let solid = |flow: Flow| params(flow.spacing(), 1.0, 1000.0, 1000.0);
    let mut emit = |area: &[ExPolygon], pattern: Pattern, role: Role, flow: Flow, p: FillParams| {
        if prusa_order {
            let rank = crate::extrusion_order::FillRank::surface(pattern, s, &p);
            for c in crate::fill::collections(pattern, area, &p, flow, role, s, Some(layer.infill))
            {
                ranked.push((rank, c));
            }
            return;
        }
        // One collection per filled ExPolygon; `params.can_reverse = false` for grid infill.
        let reversible = pattern != Pattern::Grid;
        for c in crate::fill::collections(pattern, area, &p, flow, role, s, Some(layer.infill)) {
            orca_fills.push((c, reversible));
        }
    };
    let mut narrow_area = Vec::new();
    if s.detect_narrow_internal_solid_infill {
        let (normal, narrow) = crate::fill::orca_solid::split(
            &internal_solid,
            &solid(f.solid),
            s.internal_solid_fill_pattern,
        );
        emit(
            &normal,
            s.internal_solid_fill_pattern,
            Role::SolidInfill,
            f.solid,
            solid(f.solid),
        );
        // ConcentricInternal fills the no-overlap region, preserving the original wall spacing.
        let narrow = if walls.infill_no_overlap.is_empty() {
            narrow
        } else {
            intersection_ex(&narrow, &walls.infill_no_overlap)
        };
        narrow_area = narrow;
    } else {
        emit(
            &internal_solid,
            s.internal_solid_fill_pattern,
            Role::SolidInfill,
            f.solid,
            solid(f.solid),
        );
    }
    emit(
        &bottom_here,
        s.bottom_fill_pattern,
        Role::SolidInfill,
        f.solid,
        solid(f.solid),
    );

    if s.fill_density > 0.0 {
        // Sparse infill spacing ignores the first layer and the layer height, so the pattern lines up
        // across layers.
        let spacing = Flow::new(s.infill_extrusion_width, s.layer_height).spacing();
        let anchor = s.infill_anchor.resolve(spacing, 1.0);
        let anchor_max = s.infill_anchor_max.resolve(spacing, 1.0);
        emit(
            &sparse,
            s.fill_pattern,
            Role::InternalInfill,
            f.infill,
            params(spacing, s.fill_density.min(1.0), anchor, anchor_max),
        );
        for combined in &areas.combined {
            emit(
                &intersection_ex(fill, &combined.polygons),
                s.fill_pattern,
                Role::InternalInfill,
                Flow::new(s.infill_extrusion_width, combined.height),
                params(spacing, s.fill_density.min(1.0), anchor, anchor_max),
            );
        }
    }
    emit(
        &top_here,
        s.top_fill_pattern,
        Role::TopSolidInfill,
        f.top,
        solid(f.top),
    );
    let narrow =
        |area: &[ExPolygon]| crate::fill::orca_solid::concentric(area, &solid(f.solid), f.solid, s);
    if prusa_order {
        ranked.extend(thin_fills.map(|c| (crate::extrusion_order::FillRank::thin_fills(), c)));
        // group_fills: SurfaceFill order (a std::set by SurfaceFillParams), thin fills last.
        ranked.sort_by_key(|a| a.0);
        let collections = ranked.into_iter().map(|(_, c)| c).collect();
        let prefer_cw = s.seam.prefer_clockwise_movements;
        for path in crate::extrusion_order::sort_fill_extrusions(collections, *pos, prefer_cw) {
            *pos = path.end_position().unwrap_or(*path.points.last().unwrap());
            paths.push(path);
        }
        paths.extend(narrow(&narrow_area));
        return;
    }
    // Each narrow ExPolygon is its own ConcentricInternal surface: one no-sort collection.
    for ex in &narrow_area {
        let group = narrow(std::slice::from_ref(ex));
        if !group.is_empty() {
            let collection = crate::fill::FillCollection {
                entities: vec![group],
                no_sort: true,
                multi: true,
            };
            orca_fills.push((collection, true));
        }
    }
    for path in crate::extrusion_order::chain_orca_fills(orca_fills, *pos) {
        *pos = path.end_position().unwrap_or(*path.points.last().unwrap());
        paths.push(path);
    }
}

impl FillAreas {
    pub(crate) fn new(f: &FillSurfaces) -> Self {
        Self {
            combined: f.combined.clone(),
            bottom: Indexed::new(f.bottom.clone()),
            bridges: f.bridges.clone(),
            internal_bridges: f.internal_bridge.clone(),
            top: Indexed::new(f.top.clone()),
            solid: Indexed::new(f.internal_solid.clone()),
            sparse: Indexed::new(f.internal.clone()),
        }
    }
}

pub(crate) fn make_region_walls(
    here: &[ExPolygon],
    outlines: &[Vec<ExPolygon>],
    s: &PrintSettings,
    i: usize,
    print_z: f64,
    height: f64,
    origin: Point,
) -> Vec<(ExPolygon, IslandWalls)> {
    make_region_walls_with_painting(
        here,
        outlines,
        s,
        i,
        print_z,
        height,
        origin,
        &crate::fuzzy_skin::NoPainting,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn make_region_walls_with_painting(
    here: &[ExPolygon],
    outlines: &[Vec<ExPolygon>],
    s: &PrintSettings,
    i: usize,
    print_z: f64,
    height: f64,
    origin: Point,
    painting: &dyn crate::fuzzy_skin::FuzzySkinPainting,
) -> Vec<(ExPolygon, IslandWalls)> {
    let _timing = crate::timing::Span::new(crate::timing::Stage::Walls);
    // Spiral vase layers: LayerRegion::make_perimeters runs the classic generator (never
    // Arachne) with spiral_vase set.
    let spiral = crate::spiral_vase::wall_layer(s, i, print_z);
    let classic;
    let s = if spiral && s.perimeter_generator != crate::settings::PerimeterGenerator::Classic {
        classic = PrintSettings {
            perimeter_generator: crate::settings::PerimeterGenerator::Classic,
            ..s.clone()
        };
        &classic
    } else {
        s
    };
    let f = LayerFlows::new(s, i, height);
    let params = WallParams {
        perimeters: s.perimeters,
        external: f.external,
        internal: f.perimeter,
        solid_infill: f.solid,
        infill_overlap: s.infill_overlap,
        nozzle_diameter: s.nozzle_diameter,
        thin_walls: s.thin_walls,
        gap_fill: s.has_gap_fill(),
        filter_out_gap_fill: s.small_features.filter_out_gap_fill,
        fill_density: s.fill_density,
        spiral_vase: spiral,
    };
    if here.is_empty() {
        return vec![];
    }
    let frame = crate::perimeter_frame::LayerFrame::new(
        [
            i.checked_sub(1).map(|j| outlines[j].as_slice()),
            outlines.get(i + 1).map(Vec::as_slice),
        ],
        &params,
        s,
        origin,
    );
    let walls = |isl: &ExPolygon| (isl.clone(), frame.walls(isl, &params, s, i, painting));
    // A textured top's layers have thousands of islands: their walls are made in parallel
    // (in order, so the result is the same).
    if s.textured_mesh && here.len() > 1 {
        here.par_iter().map(walls).collect()
    } else {
        here.iter().map(walls).collect()
    }
}

/// The closest point of segment `a`-`b` to `p` on the integer grid and its squared distance.
fn closest_on_segment(a: Point, b: Point, p: Point) -> (Point, f64) {
    let (vx, vy) = ((b.x - a.x) as f64, (b.y - a.y) as f64);
    let (wx, wy) = ((p.x - a.x) as f64, (p.y - a.y) as f64);
    let l2 = vx * vx + vy * vy;
    let t = if l2 > 0.0 {
        ((wx * vx + wy * vy) / l2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let q = if t <= 0.0 {
        a
    } else if t >= 1.0 {
        b
    } else {
        Point::new(
            a.x + (vx * t).round() as crate::geometry::Coord,
            a.y + (vy * t).round() as crate::geometry::Coord,
        )
    };
    (q, q.distance_sq(p) as f64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::{SCALE, polyline_length};

    fn cube() -> TriangleMesh {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/models/");
        let mut m = TriangleMesh::load_stl(format!("{dir}cube20.stl")).unwrap();
        m.place_on_bed(125.0, 110.0);
        m
    }

    /// Filament (mm) used for `role` on `layer`.
    fn filament_mm(obj: &SlicedObject, s: &PrintSettings, layer: usize, role: Role) -> f64 {
        let area = std::f64::consts::PI * (s.filament_diameter / 2.0).powi(2);
        obj.layers[layer]
            .paths
            .iter()
            .filter(|p| p.role == role)
            .map(|p| polyline_length(&p.points) / SCALE * p.flow.mm3_per_mm() / area)
            .sum()
    }

    #[test]
    fn cube_layers_and_roles() {
        let s = PrintSettings::core_one_hf04_speed_pla();
        let obj = slice_object(&cube(), &s).unwrap();
        assert_eq!(obj.layers.len(), 100);
        assert_eq!(obj.report.dropped_chains, 0);
        let roles = |i: usize| {
            obj.layers[i]
                .paths
                .iter()
                .map(|p| p.role)
                .collect::<std::collections::HashSet<_>>()
        };
        // Bottom layers solid, middle sparse, top layer top-solid.
        assert!(roles(0).contains(&Role::SolidInfill) && !roles(0).contains(&Role::InternalInfill));
        assert!(
            roles(50).contains(&Role::InternalInfill) && !roles(50).contains(&Role::SolidInfill)
        );
        assert!(roles(99).contains(&Role::TopSolidInfill));
        assert!(
            roles(96).contains(&Role::SolidInfill) && !roles(96).contains(&Role::InternalInfill)
        );
        // Every layer has an outer wall.
        assert!(
            obj.layers
                .iter()
                .all(|l| l.paths.iter().any(|p| p.role == Role::ExternalPerimeter))
        );
    }

    #[test]
    fn infill_first_prints_each_island_fill_before_its_walls() {
        let mut s = PrintSettings::core_one_hf04_speed_pla();
        let normal = slice_object(&cube(), &s).unwrap();
        s.infill_first = true;
        let first = slice_object(&cube(), &s).unwrap();
        let is_wall = |r: Role| matches!(r, Role::Perimeter | Role::ExternalPerimeter);
        for (a, b) in normal.layers[1..].iter().zip(&first.layers[1..]) {
            assert!(is_wall(a.paths[0].role) && !is_wall(a.paths.last().unwrap().role));
            assert!(!is_wall(b.paths[0].role) && is_wall(b.paths.last().unwrap().role));
            assert_eq!(a.paths.len(), b.paths.len());
        }
    }

    #[test]
    fn cube_filament_matches_prusaslicer_per_layer() {
        // Reference: PrusaSlicer 2.9.6, Core One HF0.4, 0.20mm SPEED, Prusament PLA (profiles 2.5.12),
        // filament mm per layer and role, read from its G-code.
        let s = PrintSettings::core_one_hf04_speed_pla();
        let obj = slice_object(&cube(), &s).unwrap();
        let close = |got: f64, want: f64, tol: f64| (got - want).abs() <= want * tol;
        for layer in [1, 50, 98] {
            assert!(close(
                filament_mm(&obj, &s, layer, Role::ExternalPerimeter),
                2.6,
                0.03
            ));
            assert!(close(
                filament_mm(&obj, &s, layer, Role::Perimeter),
                2.5,
                0.03
            ));
        }
        // Internal solid: PrusaSlicer uses its Arachne-based "ensuring" fill here, Mica rectilinear at 100%
        // with the spacing widened to fit (as PrusaSlicer's rectilinear does); the amount is the same.
        for layer in [1, 2, 97, 98] {
            assert!(close(
                filament_mm(&obj, &s, layer, Role::SolidInfill),
                28.3,
                0.03
            ));
        }
        // Top: monotonic lines.
        assert!(close(
            filament_mm(&obj, &s, 99, Role::TopSolidInfill),
            28.2,
            0.03
        ));
        // Sparse infill: grid at 15%, its line ends joined and anchored along the walls (connect_infill,
        // infill_anchor 2 mm, infill_anchor_max 12 mm). Plain lines at the same density give only 4.2 mm.
        assert!(close(
            filament_mm(&obj, &s, 50, Role::InternalInfill),
            5.3,
            0.05
        ));
    }

    #[test]
    fn hole_block_bottom_layers_are_fully_solid() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/models/");
        let mut m = TriangleMesh::load_stl(format!("{dir}hole_block.stl")).unwrap();
        m.place_on_bed(125.0, 110.0);
        let s = PrintSettings::core_one_hf04_speed_pla();
        let obj = slice_object(&m, &s).unwrap();
        for layer in 0..s.bottom_solid_layers {
            assert!(
                obj.layers[layer]
                    .paths
                    .iter()
                    .all(|p| p.role != Role::InternalInfill),
                "layer {layer} has sparse infill"
            );
        }
    }
}
