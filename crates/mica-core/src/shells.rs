// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: orca@8500fcdcca src/libslic3r/PrintObject.cpp :: PrintObject::detect_surfaces_type
// Changes: Optional Orca batch B wall, bridge or scarf controls; existing Prusa defaults retained.
// Changes: Orca configs (minimum_sparse_infill_area) keep small top/bottom cracks as top surfaces.
// Source: orca@8500fcdcca src/libslic3r/LayerRegion.cpp :: LayerRegion::prepare_fill_surfaces, LayerRegion::process_external_surfaces
// Changes: Orca's small sparse islands become solid after surface expansion, using its own area threshold.
// Source: prusaslicer@b028299c77 src/libslic3r/PrintObject.cpp :: PrintObject::prepare_infill, PrintObject::detect_surfaces_type, PrintObject::discover_vertical_shells, PrintObject::discover_horizontal_shells
// Source: prusaslicer@b028299c77 src/libslic3r/LayerRegion.cpp :: LayerRegion::slices_to_fill_surfaces_clipped, LayerRegion::prepare_fill_surfaces, LayerRegion::process_external_surfaces, expand_bridges_detect_orientations, expand_merge_surfaces, merge_bridges
// Source: prusaslicer@b028299c77 src/libslic3r/Algorithm/RegionExpansion.cpp :: RegionExpansionParameters::build, wave_seeds, propagate_waves, merge_expansions_into_expolygons
// Changes: Reuse immutable neighbour classification and expanded shells by exact scaled flow dimensions; shared object outlines/fill holes across regions; no interface_shells. Spiral vase types only the bottom layers (the last one top, every layer above internal) and limits vertical shells to them. Surface types live in per-type ExPolygon lists instead of a SurfaceCollection, and each
// Changes: pass works on whole-object per-layer arrays (see `classify`). Wave seeds come from clipping the
// Changes: slightly grown source contours with each boundary ExPolygon (no Clipper Z-tracking or split
// Changes: merging); bridges are grouped by overlapping anchors. Bridge direction and bridge_over_infill
// Changes: are implemented in bridge/; combined infill is in regions/combine.rs. No separate_infill_above_bridges.
// Changes: Offsets simplify their input by 0.5% of the offset instead of Clipper's ShortestEdgeLength. The
// Changes: tiny-drop filter in discover_vertical_shells tests "inside the model" for all candidates in one
// Changes: clipping and counts swallowed rings only on the internal ExPolygons near each drop (same result).
// Changes: Layers without internal surfaces skip the vertical-shell projection (its trimmed result is empty);
// Changes: regions sharing the neighbour cache share each layer's projection and its safety-grown outlines.
// Changes: Textured (displaced) meshes: wave growth cuts sources and boundaries to where they meet
// Changes: and grows the sources in parallel; projected shells are united once per layer.

//! Top, bottom and solid-shell classification of each layer's infill area (PrusaSlicer's `prepare_infill`).
//!
//! The passes need neighbouring layers, so they run over the whole object once, before any toolpath is made,
//! and keep only the typed fill areas per layer ([`FillSurfaces`]). Those are about as large as the layer
//! outlines, far smaller than toolpaths, so toolpaths can still be produced in batches afterwards.

use rayon::prelude::*;
use std::sync::OnceLock;

use crate::flow::Flow;
use crate::geometry::clip::{
    CLIPPER_SAFETY_OFFSET, DEFAULT_MITER_LIMIT, Join, diff_ex, diff_ex_safety, intersection_ex,
    intersection_ex_safety, intersection_pl, offset_ex, offset_ex_with, offset_polylines_round,
    simplify_ex, union_ex, union_safety_offset_ex,
};
use crate::geometry::{BoundingBox, ExPolygon, Point, Polyline, SCALE, signed_area};
use crate::settings::{EnsureVerticalShellThickness, PrintSettings};

/// PrusaSlicer's `EPSILON`, mm.
const EPSILON: f64 = 1e-4;
/// PrusaSlicer's `SCALED_EPSILON`, scaled units.
const SCALED_EPSILON: f64 = 100.0;

/// The typed infill areas of one layer: PrusaSlicer's `LayerRegion::fill_surfaces`, one list per type.
/// Together they cover the layer's infill area (the area inside the innermost walls).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FillSurfaces {
    /// Sparse areas printed at the combined thickness of several layers.
    pub combined: Vec<crate::regions::combine::CombinedFill>,
    /// `stTop`: seen from above.
    pub top: Vec<ExPolygon>,
    /// `stBottom`: resting on the bed (or on a supported surface).
    pub bottom: Vec<ExPolygon>,
    /// `stBottomBridge`: over air, expanded into neighbouring anchors.
    pub bottom_bridge: Vec<ExPolygon>,
    /// Expanded bottom bridges with extrusion directions (same geometry as `bottom_bridge`).
    pub bridges: Vec<crate::bridge::BridgeSurface>,
    /// `stInternalBridge`: thick bridges anchored on sparse infill.
    pub internal_bridge: Vec<crate::bridge::BridgeSurface>,
    /// `stInternal`: sparse infill.
    pub internal: Vec<ExPolygon>,
    /// `stInternalSolid`: solid infill inside the object (shells).
    pub internal_solid: Vec<ExPolygon>,
}

/// One layer's geometry and flows, as the shell passes need them.
#[derive(Clone, Copy, Debug)]
pub struct ShellLayer {
    /// Top of the layer, mm.
    pub print_z: f64,
    pub height: f64,
    pub external: Flow,
    pub perimeter: Flow,
    pub solid: Flow,
}

impl ShellLayer {
    fn bottom_z(&self) -> f64 {
        self.print_z - self.height
    }
}

/// Surface types of a layer's outlines including the wall area (PrusaSlicer's typed `LayerRegion::slices`).
/// Internal slices are not needed after classification and are not kept.
#[derive(Clone, Debug, Default)]
struct TypedSlices {
    top: Vec<ExPolygon>,
    bottom: Vec<ExPolygon>,
    bottom_bridge: Vec<ExPolygon>,
}

/// Classifies every layer's infill area into top, bottom, bridge, solid and sparse surfaces.
///
/// `outlines[i]` are the islands of layer `i`, `fill[i]` the area inside their innermost walls (PrusaSlicer's
/// `fill_expolygons`). Runs `detect_surfaces_type`, `prepare_fill_surfaces`, `discover_vertical_shells`,
/// `process_external_surfaces` and `discover_horizontal_shells`, in that order.
pub fn classify(
    outlines: &[Vec<ExPolygon>],
    fill: Vec<Vec<ExPolygon>>,
    layers: &[ShellLayer],
    s: &PrintSettings,
) -> Vec<FillSurfaces> {
    let origin = BoundingBox::from_points(outlines.iter().flatten().flat_map(|e| e.contour.iter()))
        .map(|b| Point::new((b.min.x + b.max.x) / 2, (b.min.y + b.max.y) / 2))
        .unwrap_or_default();
    classify_at(outlines, fill, layers, s, origin)
}

/// Use the same mesh-coordinate pattern origin as the eventual sparse infill paths.
pub(crate) fn classify_at(
    outlines: &[Vec<ExPolygon>],
    fill: Vec<Vec<ExPolygon>>,
    layers: &[ShellLayer],
    s: &PrintSettings,
    origin: Point,
) -> Vec<FillSurfaces> {
    classify_region_at(outlines, outlines, fill, None, layers, s, origin)
}

pub(crate) fn classify_region_at(
    outlines: &[Vec<ExPolygon>],
    neighbours: &[Vec<ExPolygon>],
    fill: Vec<Vec<ExPolygon>>,
    shared: Option<(&[Vec<ExPolygon>], &SharedShells)>,
    layers: &[ShellLayer],
    s: &PrintSettings,
    origin: Point,
) -> Vec<FillSurfaces> {
    let n = outlines.len();
    assert_eq!(fill.len(), n);
    assert_eq!(layers.len(), n);

    // detect_surfaces_type, slices_to_fill_surfaces_clipped, prepare_fill_surfaces.
    let (mut slices, mut surfaces): (Vec<TypedSlices>, Vec<FillSurfaces>) = (0..n)
        .into_par_iter()
        .map(|i| {
            // With a shared cache, the region's own typed slices are never read again (the
            // cache's are used below), so a layer without fill area needs no classification.
            // Where the region covers the whole layer, the cache already holds its typed slices:
            // the same `detect_surfaces_type` of the same outlines, widths and crack rule.
            let orca_cracks = s.minimum_sparse_infill_area.is_some();
            let (slices, mut surfaces) = match shared {
                Some(_) if fill[i].is_empty() => Default::default(),
                Some((_, cache)) => detect_surfaces_type_with(
                    outlines,
                    neighbours,
                    &fill[i],
                    layers,
                    i,
                    orca_cracks,
                    (outlines[i] == neighbours[i] && cache.orca_cracks == orca_cracks)
                        .then_some(&cache.slices[i]),
                    Some(&cache.grown),
                ),
                None => {
                    detect_surfaces_type(outlines, neighbours, &fill[i], layers, i, orca_cracks)
                }
            };
            if s.support_material && s.support_material_contact_distance == 0.0 && i > 0 {
                surfaces.bottom.append(&mut surfaces.bottom_bridge);
            }
            (slices, surfaces)
        })
        .unzip();
    // Spiral vase: only the bottom layers are classified; the last of them prints as a top
    // surface, every layer above is internal (detect_surfaces_type).
    let spiral = s.spiral_vase;
    let spiral_bottom = s.bottom_solid_layers.min(n);
    if spiral {
        if let Some((_, cache)) = shared {
            for (own, known) in slices.iter_mut().zip(&cache.slices) {
                *own = known.clone();
            }
        }
        spiral_vase_types(&mut slices, &mut surfaces, outlines, &fill, spiral_bottom);
    }
    crate::bridge::extra::external(&mut surfaces, layers, s);
    surfaces
        .par_iter_mut()
        .for_each(|surfaces| prepare_fill_surfaces(surfaces, s));
    let shared_fill = shared.map(|(fill, _)| fill);
    let extra_external = matches!(
        s.extra_bridge_layer,
        crate::bridge::ExtraBridgeLayer::External | crate::bridge::ExtraBridgeLayer::All
    );
    if extra_external {
        // The added bridge also seeds the bottom-shell stack, as Orca's typed slices do.
        if let Some((_, cache)) = shared.filter(|_| !spiral) {
            slices = cache.slices.clone();
        }
        for (typed, fill) in slices.iter_mut().zip(&surfaces) {
            typed
                .bottom_bridge
                .extend(fill.bottom_bridge.iter().cloned());
            typed.bottom_bridge = union_ex(&typed.bottom_bridge);
        }
    }
    let slices = if extra_external || spiral {
        &slices
    } else {
        shared.map_or(&slices, |(_, cache)| &cache.slices)
    };
    if s.ensure_vertical_shell_thickness != EnsureVerticalShellThickness::Disabled {
        let owned_cache;
        let (cache, projected) =
            if let Some((_, cache)) = shared.filter(|_| !extra_external && !spiral) {
                (&cache.vertical, Some(cache.projected.as_slice()))
            } else {
                owned_cache = vertical_cache(slices, layers);
                (&owned_cache, None)
            };
        let mut shelled = discover_vertical_shells(
            neighbours,
            shared_fill.unwrap_or(&fill),
            cache,
            projected,
            &surfaces,
            layers,
            s,
        );
        if spiral {
            // Spiral vase: vertical shells only for the bottom layers.
            for (new, old) in shelled.iter_mut().zip(&mut surfaces).skip(spiral_bottom) {
                *new = std::mem::take(old);
            }
        }
        surfaces = shelled;
    }
    drop(fill);

    surfaces
        .par_iter_mut()
        .zip(layers)
        .for_each(|(surfaces, layer)| process_external_surfaces(surfaces, layer, s));
    discover_horizontal_shells(slices, &mut surfaces, layers, s);
    surfaces.par_iter_mut().for_each(|f| {
        f.bridges = f
            .bridges
            .iter()
            .flat_map(|b| {
                intersection_ex(std::slice::from_ref(&b.polygon), &f.bottom_bridge)
                    .into_iter()
                    .map(|polygon| crate::bridge::BridgeSurface {
                        polygon,
                        angle: b.angle,
                    })
            })
            .collect();
    });
    if shared_fill.is_none() {
        crate::bridge::bridge_over_infill(&mut surfaces, layers, s, origin);
    }
    surfaces
}

/// `PrintObject::detect_surfaces_type` for layer `i` of a single-region object, followed by
/// `LayerRegion::slices_to_fill_surfaces_clipped`.
fn detect_surfaces_type(
    outlines: &[Vec<ExPolygon>],
    neighbours: &[Vec<ExPolygon>],
    fill: &[ExPolygon],
    layers: &[ShellLayer],
    i: usize,
    orca_cracks: bool,
) -> (TypedSlices, FillSurfaces) {
    detect_surfaces_type_with(
        outlines,
        neighbours,
        fill,
        layers,
        i,
        orca_cracks,
        None,
        None,
    )
}

/// [`detect_surfaces_type`], taking the typed slices from `known` when they were already computed
/// for the same outlines, neighbours and external width, and the neighbours grown by the safety
/// offset (`diff_ex_safety`'s clip) from `grown` when given.
#[allow(clippy::too_many_arguments)]
fn detect_surfaces_type_with(
    outlines: &[Vec<ExPolygon>],
    neighbours: &[Vec<ExPolygon>],
    fill: &[ExPolygon],
    layers: &[ShellLayer],
    i: usize,
    orca_cracks: bool,
    known: Option<&TypedSlices>,
    grown: Option<&[Vec<ExPolygon>]>,
) -> (TypedSlices, FillSurfaces) {
    let here = &outlines[i];
    if let Some(known) = known {
        let bottom = if i > 0 {
            known.bottom_bridge.clone()
        } else {
            known.bottom.clone()
        };
        return clip_typed_slices(here, known.clone(), &bottom, fill);
    }
    // `diff_ex_safety(here, neighbours[j])`, reusing the grown neighbour when there is one.
    let diff_safety = |j: usize| match grown {
        Some(grown) if !here.is_empty() && !neighbours[j].is_empty() => diff_ex(here, &grown[j]),
        _ => diff_ex_safety(here, &neighbours[j]),
    };
    // Collapse very narrow parts (using the safety offset in the diff is not enough).
    let offset = layers[i].external.scaled_width() as f64 / 10.0;
    // Find top surfaces (difference between this layer and the one above).
    let mut top = if i + 1 < neighbours.len() {
        opening(&diff_safety(i + 1), offset)
    } else {
        here.clone()
    };
    // Bottom surfaces: any surface lying on the void is a bottom bridge (an overhang). Without support
    // touching the object (contact distance 0), there is no plain stBottom above the first layer.
    let mut slices = TypedSlices::default();
    let mut bottom = if i > 0 {
        opening(&diff_safety(i - 1), offset)
    } else {
        slices.bottom = here.clone();
        here.clone()
    };
    // A thin membrane can be top and bottom at once: it counts as bottom (for bridge detection).
    if !top.is_empty() && !bottom.is_empty() {
        if orca_cracks && i > 0 {
            // Orca: a small top/bottom overlap (a "crack") stays top unless it is part of a large
            // bottom surface.
            let threshold = layers[i].external.scaled_width() as f64 * 1.5;
            for crack in intersection_ex(&top, &bottom) {
                let one = std::slice::from_ref(&crack);
                if !crate::geometry::clip::offset_ex(one, -threshold, Join::Miter).is_empty() {
                    continue;
                }
                let part_of_large = bottom.iter().any(|b| {
                    let b = std::slice::from_ref(b);
                    diff_ex_safety(one, b).is_empty()
                        && b[0].area_mm2() > crack.area_mm2() * 2.0
                        && !crate::geometry::clip::offset_ex(
                            &diff_ex(b, one),
                            -threshold,
                            Join::Miter,
                        )
                        .is_empty()
                });
                if !part_of_large {
                    bottom = diff_ex(
                        &bottom,
                        &crate::geometry::clip::offset_ex(one, threshold, Join::Miter),
                    );
                }
            }
        }
        top = diff_ex(&top, &bottom);
    }
    if i > 0 {
        slices.bottom_bridge = bottom.clone();
    }
    slices.top = top;
    clip_typed_slices(here, slices, &bottom, fill)
}

/// The fill surfaces of `detect_surfaces_type`: the typed slices and the internal rest of the layer,
/// trimmed to the infill area (`slices_to_fill_surfaces_clipped`).
fn clip_typed_slices(
    here: &[ExPolygon],
    slices: TypedSlices,
    bottom: &[ExPolygon],
    fill: &[ExPolygon],
) -> (TypedSlices, FillSurfaces) {
    // Shared-neighbour callers need only typed outlines. Empty regional fill also has no
    // clipped surfaces: avoid the unused whole-layer internal difference in both cases.
    if fill.is_empty() {
        return (slices, FillSurfaces::default());
    }
    let internal = diff_ex(here, &[slices.top.as_slice(), bottom].concat());

    // Fill surfaces: the typed slices trimmed to the infill area.
    let clip = |ex: &[ExPolygon]| {
        if ex.is_empty() {
            Vec::new()
        } else {
            intersection_ex(ex, fill)
        }
    };
    let surfaces = FillSurfaces {
        top: clip(&slices.top),
        bottom: clip(&slices.bottom),
        bottom_bridge: clip(&slices.bottom_bridge),
        internal: clip(&internal),
        internal_solid: Vec::new(),
        ..FillSurfaces::default()
    };
    (slices, surfaces)
}

/// `LayerRegion::prepare_fill_surfaces`: no top/bottom surfaces when no such layers are wanted, and small
/// sparse areas become solid.
fn prepare_fill_surfaces(surfaces: &mut FillSurfaces, s: &PrintSettings) {
    // Spiral vase keeps the top it gave the last bottom layer.
    if s.top_solid_layers == 0 && !s.spiral_vase {
        let top = std::mem::take(&mut surfaces.top);
        surfaces.internal.extend(top);
    }
    if s.bottom_solid_layers == 0 {
        let bottom = std::mem::take(&mut surfaces.bottom);
        let bridge = std::mem::take(&mut surfaces.bottom_bridge);
        surfaces.internal.extend(bottom);
        surfaces.internal.extend(bridge);
    }
    if s.minimum_sparse_infill_area.is_some() && s.fill_density >= 1.0 {
        surfaces.internal_solid.append(&mut surfaces.internal);
    } else if s.minimum_sparse_infill_area.is_none() && s.fill_density > 0.0 && !s.spiral_vase {
        let min_area = s.solid_infill_below_area * SCALE * SCALE;
        let (small, large): (Vec<_>, Vec<_>) = std::mem::take(&mut surfaces.internal)
            .into_iter()
            .partition(|ex| area(ex) <= min_area);
        surfaces.internal = large;
        surfaces.internal_solid.extend(small);
    }
}

/// `PrintObject::detect_surfaces_type` in spiral vase mode: of the `bottom` layers only the ones
/// below the last keep their detected types; the last bottom layer (when there are several) is a
/// top surface, so it prints with the top pattern, and every layer above it is internal.
fn spiral_vase_types(
    slices: &mut [TypedSlices],
    surfaces: &mut [FillSurfaces],
    outlines: &[Vec<ExPolygon>],
    fill: &[Vec<ExPolygon>],
    bottom: usize,
) {
    let detected = if bottom > 1 { bottom - 1 } else { bottom };
    for i in detected..slices.len() {
        let clipped = if fill[i].is_empty() {
            Vec::new()
        } else {
            intersection_ex(&outlines[i], &fill[i])
        };
        if bottom > 1 && i == bottom - 1 {
            slices[i] = TypedSlices {
                top: outlines[i].clone(),
                ..TypedSlices::default()
            };
            surfaces[i] = FillSurfaces {
                top: clipped,
                ..FillSurfaces::default()
            };
        } else {
            slices[i] = TypedSlices::default();
            surfaces[i] = FillSurfaces {
                internal: clipped,
                ..FillSurfaces::default()
            };
        }
    }
}

/// Net area of an ExPolygon in scaled units², whatever the orientation of its rings.
fn area(ex: &ExPolygon) -> f64 {
    signed_area(&ex.contour).abs() - ex.holes.iter().map(|h| signed_area(h).abs()).sum::<f64>()
}

/// Number of rings (contours and holes), PrusaSlicer's `Polygons::size()`.
fn ring_count(ex: &[ExPolygon]) -> usize {
    ex.iter().map(|e| 1 + e.holes.len()).sum()
}

/// PrusaSlicer's Clipper fork drops edges shorter than this fraction of the offset before offsetting
/// (`ClipperOffsetShortestEdgeFactor`). Clipper2 has no such option; simplifying the input by the same
/// tolerance serves the same purpose: clipping leaves long runs of nearly collinear vertices, which make
/// Clipper2's offsets very slow.
const SHORTEST_EDGE_FACTOR: f64 = 0.005;

fn simplified(ex: &[ExPolygon], delta: f64) -> Vec<ExPolygon> {
    simplify_ex(ex, delta.abs() * SHORTEST_EDGE_FACTOR)
}

/// PrusaSlicer's `offset_ex(expolygons, delta, join, miter_limit)`.
fn offset_with(ex: &[ExPolygon], delta: f64, join: Join, miter_limit: f64) -> Vec<ExPolygon> {
    offset_ex_with(&simplified(ex, delta), delta, join, miter_limit, 0.0)
}

/// `offset_ex` with the default miter join (`expand` / `shrink`).
fn offset(ex: &[ExPolygon], delta: f64) -> Vec<ExPolygon> {
    offset_with(ex, delta, Join::Miter, DEFAULT_MITER_LIMIT)
}

/// `opening_ex(expolygons, delta)`: drops parts narrower than `2 * delta`.
fn opening(ex: &[ExPolygon], delta: f64) -> Vec<ExPolygon> {
    offset(&offset(ex, -delta), delta)
}

/// `opening(polygons, delta1, delta2, jtMiter, miter_limit)`.
fn opening2(ex: &[ExPolygon], d1: f64, d2: f64, miter_limit: f64) -> Vec<ExPolygon> {
    offset_with(
        &offset_with(ex, -d1, Join::Miter, miter_limit),
        d2,
        Join::Miter,
        miter_limit,
    )
}

/// `closing_ex(expolygons, delta)`: fills gaps narrower than `2 * delta`.
fn closing(ex: &[ExPolygon], delta: f64) -> Vec<ExPolygon> {
    offset(&offset(ex, delta), -delta)
}

/// Per-layer data `discover_vertical_shells` looks up in neighbouring layers.
struct VerticalShellsCache {
    top_surfaces: Vec<ExPolygon>,
    bottom_surfaces: Vec<ExPolygon>,
}

/// Immutable neighbour classification shared by regions with exactly the same scaled
/// external width and solid spacing. Scoped to one object's outline column.
pub(crate) struct SharedShells {
    dimensions: Vec<(i64, i64)>,
    /// The crack rule the typed slices were classified with.
    orca_cracks: bool,
    /// The outlines grown by the safety offset: the clip of every region's `diff_ex_safety`
    /// against a neighbouring layer.
    grown: Vec<Vec<ExPolygon>>,
    slices: Vec<TypedSlices>,
    vertical: Vec<VerticalShellsCache>,
    /// Per layer: the projected shell and hole unions of `discover_vertical_shells`, which depend
    /// only on this shared cache, the shared fill holes and the projection settings ([`ShellKey`]),
    /// not on the region. The first region to need a layer computes it (a concurrent region waits
    /// for that result); later regions with the same key reuse it, other keys compute their own.
    projected: Vec<ProjectedMemo>,
}

/// One layer's projection and the settings it was made with.
type ProjectedMemo = OnceLock<(ShellKey, Projected)>;

/// Everything besides the shared inputs that [`project_shells`] reads.
#[derive(Clone, Copy, PartialEq)]
struct ShellKey {
    top_solid_layers: usize,
    bottom_solid_layers: usize,
    top_solid_min_thickness: u64,
    bottom_solid_min_thickness: u64,
    partial: bool,
    external_spacing: i64,
}

impl ShellKey {
    fn new(s: &PrintSettings, layer: &ShellLayer) -> Self {
        Self {
            top_solid_layers: s.top_solid_layers,
            bottom_solid_layers: s.bottom_solid_layers,
            top_solid_min_thickness: s.top_solid_min_thickness.to_bits(),
            bottom_solid_min_thickness: s.bottom_solid_min_thickness.to_bits(),
            partial: s.ensure_vertical_shell_thickness == EnsureVerticalShellThickness::Partial,
            external_spacing: layer.external.scaled_spacing(),
        }
    }
}

/// The union of the top / bottom surfaces projected onto a layer, and the holes common to it.
struct Projected {
    shell: Vec<ExPolygon>,
    holes: Vec<ExPolygon>,
}
impl SharedShells {
    pub fn new(outlines: &[Vec<ExPolygon>], layers: &[ShellLayer], orca_cracks: bool) -> Self {
        let grown: Vec<_> = outlines
            .par_iter()
            .map(|o| {
                if o.is_empty() {
                    Vec::new()
                } else {
                    offset_ex(o, CLIPPER_SAFETY_OFFSET, Join::Miter)
                }
            })
            .collect();
        let slices: Vec<_> = (0..outlines.len())
            .into_par_iter()
            .map(|i| {
                detect_surfaces_type_with(
                    outlines,
                    outlines,
                    &[],
                    layers,
                    i,
                    orca_cracks,
                    None,
                    Some(&grown),
                )
                .0
            })
            .collect();
        let vertical = vertical_cache(&slices, layers);
        Self {
            dimensions: layers.iter().map(Self::dimensions).collect(),
            orca_cracks,
            projected: (0..slices.len()).map(|_| OnceLock::new()).collect(),
            grown,
            slices,
            vertical,
        }
    }
    fn dimensions(layer: &ShellLayer) -> (i64, i64) {
        (layer.external.scaled_width(), layer.solid.scaled_spacing())
    }
    pub fn matches(&self, layers: &[ShellLayer]) -> bool {
        self.dimensions
            .iter()
            .copied()
            .eq(layers.iter().map(Self::dimensions))
    }
}

fn vertical_cache(slices: &[TypedSlices], layers: &[ShellLayer]) -> Vec<VerticalShellsCache> {
    // Just a tiny fraction of an infill extrusion width to merge neighbour regions reliably.
    const TOP_BOTTOM_EXPANSION_COEFF: f64 = 0.05;
    let n = slices.len();
    (0..n)
        .into_par_iter()
        .map(|i| {
            let expansion = layers[i].solid.scaled_spacing() as f64 * TOP_BOTTOM_EXPANSION_COEFF;
            let bottoms = [slices[i].bottom.as_slice(), &slices[i].bottom_bridge].concat();
            VerticalShellsCache {
                top_surfaces: offset(&slices[i].top, expansion),
                bottom_surfaces: offset(&bottoms, expansion),
            }
        })
        .collect()
}

/// The top surfaces of the layers above and the bottom surfaces of the layers below `idx` projected
/// onto it, and the fill holes common to all of them (`discover_vertical_shells`' gathering loops).
fn project_shells(
    outlines: &[Vec<ExPolygon>],
    holes: &[Vec<ExPolygon>],
    cache: &[VerticalShellsCache],
    layers: &[ShellLayer],
    s: &PrintSettings,
    idx: usize,
) -> Projected {
    let n = outlines.len();
    let layer = &layers[idx];
    let partial = s.ensure_vertical_shell_thickness == EnsureVerticalShellThickness::Partial;
    // Union of the top / bottom surfaces above / below, to guarantee a minimum shell thickness.
    let mut shell: Vec<ExPolygon> = Vec::new();
    let mut holes_here: Vec<ExPolygon> = holes[idx].clone();
    let combine_holes = |holes_here: &mut Vec<ExPolygon>, other: &[ExPolygon]| {
        *holes_here = if holes_here.is_empty() || other.is_empty() {
            Vec::new()
        } else {
            intersection_ex(holes_here, other)
        };
    };
    // Textured meshes: the many small surfaces of every layer are united once, at the end.
    let lazy = s.textured_mesh;
    let combine_shells = |shell: &mut Vec<ExPolygon>, other: &[ExPolygon]| {
        if shell.is_empty() {
            *shell = other.to_vec();
        } else if !other.is_empty() {
            shell.extend_from_slice(other);
            if !lazy {
                *shell = union_ex(shell);
            }
        }
    };
    if s.top_solid_layers > 0 {
        // Gather top regions projected to this layer.
        let mut i = idx + 1;
        let itop = idx + s.top_solid_layers;
        let mut at_least_one = false;
        while i < n
            && (i < itop || layers[i].print_z - layer.print_z < s.top_solid_min_thickness - EPSILON)
        {
            at_least_one = true;
            if !partial {
                combine_holes(&mut holes_here, &holes[i]);
            }
            combine_shells(&mut shell, &cache[i].top_surfaces);
            i += 1;
        }
        if !at_least_one && i < n {
            // With a single top layer and no minimum thickness, anchor the solid layer over the
            // walls by at least one wall width.
            let anchor = intersection_ex(
                &offset(
                    &cache[idx].top_surfaces,
                    layer.external.scaled_spacing() as f64,
                ),
                &outlines[i],
            );
            combine_shells(&mut shell, &anchor);
        }
    }
    if s.bottom_solid_layers > 0 {
        // Gather bottom regions projected to this layer.
        let bottom_z = layer.bottom_z();
        let ibottom = idx as isize - s.bottom_solid_layers as isize;
        let mut i = idx as isize - 1;
        let mut at_least_one = false;
        while i >= 0
            && (i > ibottom
                || bottom_z - layers[i as usize].bottom_z()
                    < s.bottom_solid_min_thickness - EPSILON)
        {
            at_least_one = true;
            if !partial {
                combine_holes(&mut holes_here, &holes[i as usize]);
            }
            combine_shells(&mut shell, &cache[i as usize].bottom_surfaces);
            i -= 1;
        }
        if !at_least_one && i >= 0 {
            let anchor = intersection_ex(
                &offset(
                    &cache[idx].bottom_surfaces,
                    layer.external.scaled_spacing() as f64,
                ),
                &outlines[i as usize],
            );
            combine_shells(&mut shell, &anchor);
        }
    }
    if lazy && !shell.is_empty() {
        shell = union_ex(&shell);
    }
    Projected {
        shell,
        holes: holes_here,
    }
}

/// `PrintObject::discover_vertical_shells` for a single region: adds solid infill so that sloping walls and
/// the shells under top / over bottom surfaces keep the configured thickness.
///
/// A layer without internal surfaces is returned unchanged before anything is projected onto it: its
/// trimmed shell is empty anyway (`intersection(shell, {})` and `diff({}, holes)` upstream). With
/// `projected`, regions with equal [`ShellKey`]s share each layer's projection.
fn discover_vertical_shells(
    outlines: &[Vec<ExPolygon>],
    holes: &[Vec<ExPolygon>],
    cache: &[VerticalShellsCache],
    projected: Option<&[ProjectedMemo]>,
    surfaces: &[FillSurfaces],
    layers: &[ShellLayer],
    s: &PrintSettings,
) -> Vec<FillSurfaces> {
    let n = outlines.len();

    (0..n)
        .into_par_iter()
        .map(|idx| {
            let layer = &layers[idx];
            let current = &surfaces[idx];
            if current.internal.is_empty() && current.internal_solid.is_empty() {
                return current.clone();
            }
            let infill_line_spacing = layer.solid.scaled_spacing() as f64;
            let min_perimeter_infill_spacing = infill_line_spacing * 1.05;
            let project = || project_shells(outlines, holes, cache, layers, s, idx);
            let owned;
            let projection = match projected {
                Some(memo) => {
                    let key = ShellKey::new(s, layer);
                    match memo[idx].get_or_init(|| (key, project())) {
                        (k, p) if *k == key => p,
                        _ => {
                            owned = project();
                            &owned
                        }
                    }
                }
                None => {
                    owned = project();
                    &owned
                }
            };
            let (shell, holes_here) = (&projection.shell, &projection.holes);
            // Trim the shells by the internal surfaces (sparse, void, solid).
            let polygons_internal: Vec<ExPolygon> =
                [current.internal.as_slice(), &current.internal_solid].concat();
            let mut shell = intersection_ex_safety(shell, &polygons_internal);
            shell.extend(diff_ex(&polygons_internal, holes_here));
            if shell.is_empty() {
                return current.clone();
            }
            // Merge with the existing internal solids.
            shell.extend_from_slice(&current.internal_solid);

            // Filled by a rectilinear solid infill, which only fills regions that fit a line: avoid gaps in
            // the sparse infill by leaving no parts narrower than the infill spacing.
            // Open to remove regions narrower than a bit less than an infill line, close gaps narrower than
            // 1.2 lines (hard to fill with sparse infill), then grow a bit to remove tiny gaps between the
            // solid infill and the other regions.
            let narrow_ensure = 0.5 * 0.65 * min_perimeter_infill_spacing;
            let narrow_sparse = 0.5 * 1.2 * min_perimeter_infill_spacing;
            let tiny_overlap = 0.2 * min_perimeter_infill_spacing;
            let square =
                |ex: &[ExPolygon], delta| offset_with(ex, delta, Join::Square, DEFAULT_MITER_LIMIT);
            let opened = square(
                &square(&union_ex(&shell), -narrow_ensure),
                narrow_ensure + narrow_sparse,
            );
            let mut regularized = square(&opened, -(narrow_sparse - tiny_overlap));
            // The regularization may leave tiny drops on smooth parts of the model; drop a region when it is
            // very small, or quite small and fully inside the model (e.g. the sloping top of a Benchy's hull),
            // unless it covers a whole internal polygon (a very thin part would lose its solid layers).
            let tiny = min_perimeter_infill_spacing * 1.5 * SCALE;
            let quite_small = min_perimeter_infill_spacing * 8.0 * SCALE;
            // "Fully wrapped in the model": inside both neighbouring layers. Tested for all candidates in one
            // clipping operation; a candidate is inside when no part of it is left over.
            let candidates: Vec<usize> = (0..regularized.len())
                .filter(|&k| {
                    let a = area(&regularized[k]);
                    (tiny..quite_small).contains(&a)
                })
                .collect();
            let mut inside = vec![false; regularized.len()];
            if !candidates.is_empty()
                && let (Some(below), Some(above)) = (idx.checked_sub(1), outlines.get(idx + 1))
            {
                let object_volume = intersection_ex(&outlines[below], above);
                let pieces: Vec<ExPolygon> =
                    candidates.iter().map(|&k| regularized[k].clone()).collect();
                let outside = diff_ex(&pieces, &object_volume);
                // Each left-over part belongs to exactly one (disjoint) candidate.
                for &k in &candidates {
                    inside[k] = true;
                }
                for o in &outside {
                    let one = std::slice::from_ref(o);
                    let Some(ob) = bbox_of(one) else { continue };
                    let owner = candidates.iter().copied().find(|&k| {
                        let piece = std::slice::from_ref(&regularized[k]);
                        bbox_of(piece).is_some_and(|pb| overlaps(&pb, &ob))
                            && !intersection_ex(piece, one).is_empty()
                    });
                    if let Some(k) = owner {
                        inside[k] = false;
                    }
                }
            }
            // Whether growing the region swallows a whole internal ring: only the internal ExPolygons near
            // the region can lose a ring, so only those are clipped.
            let mut internal_volume: Option<Vec<ExPolygon>> = None;
            let mut keep = Vec::with_capacity(regularized.len());
            for (k, p) in regularized.iter().enumerate() {
                let a = area(p);
                if !(a < tiny || inside[k]) {
                    keep.push(true);
                    continue;
                }
                let volume = internal_volume
                    .get_or_insert_with(|| closing(&polygons_internal, SCALED_EPSILON));
                let grown = offset(std::slice::from_ref(p), min_perimeter_infill_spacing);
                let near: Vec<ExPolygon> = match bbox_of(&grown) {
                    Some(gb) => volume
                        .iter()
                        .filter(|v| {
                            bbox_of(std::slice::from_ref(v)).is_some_and(|vb| overlaps(&vb, &gb))
                        })
                        .cloned()
                        .collect(),
                    None => Vec::new(),
                };
                let rest = diff_ex(&near, &grown);
                keep.push(ring_count(&rest) < ring_count(&near));
            }
            let mut keep = keep.into_iter();
            regularized.retain(|_| keep.next().unwrap_or(true));
            if regularized.is_empty() {
                return current.clone();
            }

            FillSurfaces {
                combined: current.combined.clone(),
                top: current.top.clone(),
                bottom: current.bottom.clone(),
                bottom_bridge: current.bottom_bridge.clone(),
                bridges: current.bridges.clone(),
                internal_bridge: current.internal_bridge.clone(),
                internal: diff_ex(&current.internal, &regularized),
                internal_solid: intersection_ex(&polygons_internal, &regularized),
            }
        })
        .collect()
}

/// PrusaSlicer's `RegionExpansionParameters`, scaled units.
#[derive(Clone, Copy, Debug)]
struct ExpansionParams {
    tiny_expansion: f64,
    initial_step: f64,
    other_step: f64,
    num_other_steps: usize,
    /// Furthest the waves can get from the seeds; the boundary is trimmed to that for speed.
    max_inflation: f64,
    /// Cut the boundary to each source's neighbourhood with rectangle clipping first (textured
    /// meshes: thousands of small sources in one large boundary). The same regions; points on
    /// the cut may round differently, so plain objects keep the sweep over the whole boundary.
    crop: bool,
}

impl ExpansionParams {
    /// `RegionExpansionParameters::build`.
    fn build(full_expansion: f64, expansion_step: f64, max_nr_expansion_steps: usize) -> Self {
        // The C++ works in floats here.
        let full = full_expansion as f32;
        let step = expansion_step as f32;
        let mut tiny = (0.25 * full).min(0.05 * SCALE as f32);
        let mut nsteps = ((full - tiny) / step).ceil() as usize;
        nsteps = nsteps.min(max_nr_expansion_steps);
        let mut initial = (full - tiny) / nsteps as f32;
        if nsteps > 1 && 0.25 * initial < tiny {
            // Decrease the step size by lowering the number of steps.
            nsteps = (((full - tiny) as f64 / (4.0 * tiny as f64)).floor() as usize).max(1);
            initial = (full - tiny) / nsteps as f32;
        }
        if 0.25 * initial < tiny || nsteps == 1 {
            tiny = 0.2 * full;
            initial = 0.8 * full;
        }
        Self {
            tiny_expansion: tiny as f64,
            initial_step: initial as f64,
            other_step: initial as f64,
            num_other_steps: nsteps - 1,
            max_inflation: (tiny as f64 + nsteps as f64 * initial as f64) * 1.1,
            crop: false,
        }
    }
}

/// Arc tolerance of the wave offsets: PrusaSlicer uses 0.1 mm, which ClipperLib caps at a quarter of the
/// offset.
fn wave_arc_tolerance(delta: f64) -> f64 {
    (0.1 * SCALE).min(0.25 * delta.abs())
}

fn bbox_of(ex: &[ExPolygon]) -> Option<BoundingBox> {
    BoundingBox::from_points(ex.iter().flat_map(|e| e.contour.iter()))
}

fn overlaps(a: &BoundingBox, b: &BoundingBox) -> bool {
    a.min.x <= b.max.x && a.max.x >= b.min.x && a.min.y <= b.max.y && a.max.y >= b.min.y
}

/// One source region grown into one boundary region.
struct Expansion {
    src: usize,
    polygons: Vec<ExPolygon>,
}

/// `wave_seeds` + `propagate_waves`: grows each source ExPolygon into the boundary ExPolygons it touches, in
/// waves of `params` steps that stay inside the boundary (so the growth follows the boundary's shape instead
/// of jumping across narrow gaps).
fn propagate_waves(
    src: &[ExPolygon],
    boundary: &[ExPolygon],
    params: &ExpansionParams,
) -> Vec<Expansion> {
    if src.is_empty() || boundary.is_empty() {
        return Vec::new();
    }
    let boundary_boxes: Vec<Option<BoundingBox>> = boundary
        .iter()
        .map(|b| bbox_of(std::slice::from_ref(b)))
        .collect();
    let croppers: Vec<Option<crate::geometry::clip::Cropper>> = boundary
        .iter()
        .map(|b| {
            params
                .crop
                .then(|| crate::geometry::clip::Cropper::new(std::slice::from_ref(b)))
        })
        .collect();
    // Margin around a source's seeds rings: the rings never reach the cut.
    let margin = |bb: BoundingBox, m: i64| BoundingBox {
        min: Point::new(bb.min.x - m, bb.min.y - m),
        max: Point::new(bb.max.x + m, bb.max.y + m),
    };
    let grow = |(si, s): (usize, &ExPolygon)| -> Vec<Expansion> {
        let mut out = Vec::new();
        // Expand the source slightly to intersect the boundaries; its rings, opened, are the seeds.
        let grown = offset_with(
            std::slice::from_ref(s),
            params.tiny_expansion,
            Join::Square,
            DEFAULT_MITER_LIMIT,
        );
        let Some(grown_bb) = bbox_of(&grown) else {
            return out;
        };
        let rings: Vec<Polyline> = grown
            .iter()
            .flat_map(|e| e.polygons())
            .filter(|r| r.len() >= 2)
            .map(|r| {
                let mut line = r.clone();
                line.push(r[0]);
                line
            })
            .collect();
        let ring_boxes: Vec<Option<BoundingBox>> = if params.crop {
            rings
                .iter()
                .map(|r| BoundingBox::from_points(r.iter()))
                .collect()
        } else {
            Vec::new()
        };
        for (bi, b) in boundary.iter().enumerate() {
            match &boundary_boxes[bi] {
                Some(bb) if overlaps(bb, &grown_bb) => {}
                _ => continue,
            }
            let seeds = match (&croppers[bi], &boundary_boxes[bi]) {
                (Some(c), Some(bb)) => {
                    // Only where this source and this boundary meet (a large top with
                    // thousands of holes meets thousands of small boundaries, or the other way
                    // round): the rings and the boundary are both cut to that box.
                    let meet = margin(
                        BoundingBox {
                            min: Point::new(
                                bb.min.x.max(grown_bb.min.x),
                                bb.min.y.max(grown_bb.min.y),
                            ),
                            max: Point::new(
                                bb.max.x.min(grown_bb.max.x),
                                bb.max.y.min(grown_bb.max.y),
                            ),
                        },
                        1000,
                    );
                    let near: Vec<&Polyline> = rings
                        .iter()
                        .zip(&ring_boxes)
                        .filter(|(_, rb)| rb.is_some_and(|rb| overlaps(&rb, &meet)))
                        .map(|(r, _)| r)
                        .collect();
                    if near.is_empty() {
                        Vec::new()
                    } else {
                        let near = crate::geometry::clip::crop_lines(&near, meet);
                        intersection_pl(&near, &c.crop(meet))
                    }
                }
                _ => intersection_pl(&rings, std::slice::from_ref(b)),
            };
            if seeds.is_empty() {
                continue;
            }
            // Only the part of the boundary the waves can reach takes part in the clipping.
            let Some(seed_bb) = BoundingBox::from_points(seeds.iter().flatten()) else {
                continue;
            };
            let m = params.max_inflation.ceil() as i64;
            let cropped;
            let one = match &croppers[bi] {
                Some(c) => {
                    cropped = c.crop(margin(seed_bb, m));
                    cropped.as_slice()
                }
                None => {
                    let reach = ExPolygon {
                        contour: vec![
                            Point::new(seed_bb.min.x - m, seed_bb.min.y - m),
                            Point::new(seed_bb.max.x + m, seed_bb.min.y - m),
                            Point::new(seed_bb.max.x + m, seed_bb.max.y + m),
                            Point::new(seed_bb.min.x - m, seed_bb.max.y + m),
                        ],
                        holes: Vec::new(),
                    };
                    cropped = intersection_ex(std::slice::from_ref(b), &[reach]);
                    cropped.as_slice()
                }
            };
            let mut wave = intersection_ex(
                &offset_polylines_round(
                    &seeds,
                    params.initial_step,
                    wave_arc_tolerance(params.initial_step),
                ),
                one,
            );
            for _ in 0..params.num_other_steps {
                if wave.is_empty() {
                    break;
                }
                wave = intersection_ex(
                    &offset_ex_with(
                        &simplified(&wave, params.other_step),
                        params.other_step,
                        Join::Round,
                        DEFAULT_MITER_LIMIT,
                        wave_arc_tolerance(params.other_step),
                    ),
                    one,
                );
            }
            if !wave.is_empty() {
                out.push(Expansion {
                    src: si,
                    polygons: wave,
                });
            }
        }
        out
    };
    // Sources are independent; many small ones (textured tops) are grown in parallel, in order.
    if params.crop {
        src.par_iter().enumerate().flat_map_iter(grow).collect()
    } else {
        src.iter().enumerate().flat_map(grow).collect()
    }
}

/// Whether `pt` lies inside `ex` (even-odd over all rings).
pub(crate) fn contains(ex: &ExPolygon, pt: Point) -> bool {
    let mut inside = false;
    for ring in ex.polygons() {
        let n = ring.len();
        for k in 0..n {
            let a = ring[k];
            let b = ring[(k + 1) % n];
            if (a.y > pt.y) != (b.y > pt.y) {
                let x = a.x as f64 + (pt.y - a.y) as f64 * (b.x - a.x) as f64 / (b.y - a.y) as f64;
                if (pt.x as f64) < x {
                    inside = !inside;
                }
            }
        }
    }
    inside
}

/// An infill area into which external surfaces may grow, with how far they grow into it.
struct ExpansionZone {
    expolygons: Vec<ExPolygon>,
    params: ExpansionParams,
    expanded_into: bool,
}

/// `expand_merge_surfaces`: grows `src` into the zones, merges each source with its growth, and trims the
/// zones by the result.
fn expand_merge_surfaces(
    src: Vec<ExPolygon>,
    zones: &mut [ExpansionZone],
    closing_radius: f64,
) -> Vec<ExPolygon> {
    if src.is_empty() {
        return Vec::new();
    }
    let mut per_src: Vec<Vec<ExPolygon>> = vec![Vec::new(); src.len()];
    for zone in zones.iter_mut() {
        let expansions = propagate_waves(&src, &zone.expolygons, &zone.params);
        zone.expanded_into = !expansions.is_empty();
        for e in expansions {
            per_src[e.src].extend(e.polygons);
        }
    }
    // merge_expansions_into_expolygons: growing one ExPolygon should not change its connectivity; if it
    // does, keep the part that contains the source.
    let mut expanded = Vec::with_capacity(src.len());
    for (s, grown) in src.into_iter().zip(per_src) {
        if grown.is_empty() {
            expanded.push(s);
            continue;
        }
        let sample = s.contour[0];
        let mut acc = grown;
        acc.push(s);
        let merged = union_safety_offset_ex(&acc);
        if merged.len() > 1 {
            if let Some(m) = merged.into_iter().find(|m| contains(m, sample)) {
                expanded.push(m);
            }
        } else {
            expanded.extend(merged);
        }
    }
    // Regularization of the shells can leave small unassigned regions; close them.
    let expanded = closing(&expanded, closing_radius);
    for zone in zones.iter_mut() {
        if zone.expanded_into {
            zone.expolygons = diff_ex(&zone.expolygons, &expanded);
        }
    }
    expanded
}

/// `expand_bridges_detect_orientations`: grows the bridges into the zones, merges
/// bridges whose anchors overlap, and trims the zones by the result.
fn expand_bridges(
    bridges: Vec<ExPolygon>,
    zones: &mut [ExpansionZone],
    closing_radius: f64,
) -> Vec<crate::bridge::BridgeSurface> {
    if bridges.is_empty() {
        return Vec::new();
    }
    let mut per_src: Vec<Vec<ExPolygon>> = vec![Vec::new(); bridges.len()];
    for zone in zones.iter_mut() {
        let expansions = propagate_waves(&bridges, &zone.expolygons, &zone.params);
        zone.expanded_into = !expansions.is_empty();
        for e in expansions {
            per_src[e.src].extend(e.polygons);
        }
    }
    // Group bridges whose anchors overlap (union-find, lower id wins).
    let mut group: Vec<usize> = (0..bridges.len()).collect();
    fn find(group: &mut [usize], mut i: usize) -> usize {
        while group[i] != i {
            group[i] = group[group[i]];
            i = group[i];
        }
        i
    }
    let boxes: Vec<Option<BoundingBox>> = per_src.iter().map(|p| bbox_of(p)).collect();
    for i in 0..bridges.len() {
        for j in i + 1..bridges.len() {
            let (Some(a), Some(b)) = (&boxes[i], &boxes[j]) else {
                continue;
            };
            if overlaps(a, b) && !intersection_ex(&per_src[i], &per_src[j]).is_empty() {
                let (gi, gj) = (find(&mut group, i), find(&mut group, j));
                let (lo, hi) = (gi.min(gj), gi.max(gj));
                group[hi] = lo;
            }
        }
    }
    let mut out = Vec::new();
    let mut bridges: Vec<Option<ExPolygon>> = bridges.into_iter().map(Some).collect();
    for head in 0..bridges.len() {
        if find(&mut group, head) != head {
            continue;
        }
        let mut acc = Vec::new();
        let mut anchors = Vec::new();
        for k in head..bridges.len() {
            if find(&mut group, k) == head {
                acc.extend(bridges[k].take());
                anchors.extend(std::mem::take(&mut per_src[k]));
            }
        }
        acc.extend_from_slice(&anchors);
        for polygon in closing(&acc, closing_radius) {
            let angle = crate::bridge::detector::direction_from_anchors(
                std::slice::from_ref(&polygon),
                &anchors,
            );
            out.push(crate::bridge::BridgeSurface { polygon, angle });
        }
    }
    let polygons: Vec<_> = out.iter().map(|b| b.polygon.clone()).collect();
    for zone in zones.iter_mut() {
        if zone.expanded_into {
            zone.expolygons = diff_ex(&zone.expolygons, &polygons);
        }
    }
    out
}

/// `LayerRegion::process_external_surfaces`: grows top, bottom and bridge surfaces into the neighbouring
/// solid and sparse infill, so they are anchored under / over the walls.
fn process_external_surfaces(surfaces: &mut FillSurfaces, layer: &ShellLayer, s: &PrintSettings) {
    // Width of the walls.
    let (shell_width, expansion_min) = if s.perimeters > 0 {
        let ext = layer.external;
        let per = layer.perimeter;
        (
            0.5 * ext.scaled_width() as f64
                + ext.scaled_spacing() as f64
                + per.scaled_spacing() as f64 * (s.perimeters - 1) as f64,
            per.scaled_spacing() as f64,
        )
    } else {
        (SCALED_EPSILON, SCALED_EPSILON)
    };
    // The C++ computes these in floats.
    let expansion = (shell_width as f32 * std::f32::consts::SQRT_2) as f64;
    const EXPANSION_STEP: f64 = 0.1 * SCALE;
    const MAX_NR_EXPANSION_STEPS: usize = 5;
    // Absorbs empty regions left by the regularization in discover_vertical_shells.
    let closing_radius = 0.55 * 0.65 * 1.05 * layer.solid.scaled_spacing() as f64;

    let into_solid = ExpansionParams {
        crop: s.textured_mesh,
        ..ExpansionParams::build(expansion, EXPANSION_STEP, MAX_NR_EXPANSION_STEPS)
    };
    let into_sparse = ExpansionParams {
        crop: s.textured_mesh,
        ..ExpansionParams::build(expansion_min, EXPANSION_STEP, MAX_NR_EXPANSION_STEPS)
    };
    let zone = |ex: &[ExPolygon], params| ExpansionZone {
        expolygons: union_ex(ex),
        params,
        expanded_into: false,
    };
    let mut zones = vec![
        zone(&surfaces.internal_solid, into_solid),
        zone(&surfaces.internal, into_sparse),
        zone(&surfaces.top, into_solid),
    ];
    let bridges = expand_bridges(
        union_ex(&std::mem::take(&mut surfaces.bottom_bridge)),
        &mut zones,
        closing_radius,
    );
    // The top surfaces, trimmed by the bridges' anchors, then grow like the bottoms.
    let top = zones.pop().map(|z| z.expolygons).unwrap_or_default();
    let bottoms = expand_merge_surfaces(
        union_ex(&std::mem::take(&mut surfaces.bottom)),
        &mut zones,
        closing_radius,
    );
    let tops = expand_merge_surfaces(union_ex(&top), &mut zones, closing_radius);
    let mut zones = zones.into_iter();
    surfaces.internal_solid = zones.next().map(|z| z.expolygons).unwrap_or_default();
    surfaces.internal = zones.next().map(|z| z.expolygons).unwrap_or_default();
    surfaces.bottom_bridge = bridges.iter().map(|b| b.polygon.clone()).collect();
    surfaces.bridges = bridges;
    if s.bridge_angle > 0.0 {
        for bridge in &mut surfaces.bridges {
            bridge.angle = s.bridge_angle.to_radians();
        }
    }
    surfaces.bottom = bottoms;
    surfaces.top = tops;
    if let Some(min_area) = s.minimum_sparse_infill_area.filter(|_| s.fill_density > 0.) {
        let (small, large): (Vec<_>, Vec<_>) = std::mem::take(&mut surfaces.internal)
            .into_iter()
            .partition(|ex| ex.area_mm2() <= min_area);
        surfaces.internal = large;
        surfaces.internal_solid.extend(small);
        surfaces.internal_solid = union_ex(&surfaces.internal_solid);
    }
}

/// `PrintObject::discover_horizontal_shells`: solid infill every n layers, and, when
/// `ensure_vertical_shell_thickness` is disabled, solid layers under top and over bottom surfaces.
/// Serial: each layer's shells are scattered into its neighbours.
fn discover_horizontal_shells(
    slices: &[TypedSlices],
    surfaces: &mut [FillSurfaces],
    layers: &[ShellLayer],
    s: &PrintSettings,
) {
    let n = surfaces.len();
    for i in 0..n {
        if s.solid_infill_every_layers > 0
            && s.fill_density > 0.0
            && i % s.solid_infill_every_layers == 0
        {
            let internal = std::mem::take(&mut surfaces[i].internal);
            if s.fill_density >= 1.0 || s.solid_infill_every_layers == 1 {
                surfaces[i].internal_solid.extend(internal);
            } else {
                // Upstream explicitly marks periodic sparse-to-solid layers as internal bridges.
                let angle = (s.fill_angle + if i % 2 == 1 { 90. } else { 0. }).to_radians();
                surfaces[i].internal_bridge.extend(
                    internal
                        .into_iter()
                        .map(|polygon| crate::bridge::BridgeSurface { polygon, angle }),
                );
            }
        }
        // With ensure_vertical_shell_thickness, discover_vertical_shells has done the rest.
        if s.ensure_vertical_shell_thickness != EnsureVerticalShellThickness::Disabled {
            continue;
        }
        let print_z = layers[i].print_z;
        let bottom_z = layers[i].bottom_z();
        for kind in [Kind::Top, Kind::Bottom, Kind::BottomBridge] {
            let num_solid_layers = if kind == Kind::Top {
                s.top_solid_layers
            } else {
                s.bottom_solid_layers
            };
            if num_solid_layers == 0 {
                continue;
            }
            // Slices (including the wall area, which also needs shells) plus the grown fill surfaces.
            let mut solid: Vec<ExPolygon> =
                [kind.of_slices(&slices[i]), kind.of_surfaces(&surfaces[i])].concat();
            if solid.is_empty() {
                continue;
            }
            let neighbours: Box<dyn Iterator<Item = usize>> = if kind == Kind::Top {
                Box::new(
                    (0..i)
                        .rev()
                        .take_while(|&m| {
                            i - m < num_solid_layers
                                || print_z - layers[m].print_z < s.top_solid_min_thickness - EPSILON
                        })
                        .collect::<Vec<_>>()
                        .into_iter(),
                )
            } else {
                Box::new(
                    (i + 1..n)
                        .take_while(|&m| {
                            m - i < num_solid_layers
                                || layers[m].bottom_z() - bottom_z
                                    < s.bottom_solid_min_thickness - EPSILON
                        })
                        .collect::<Vec<_>>()
                        .into_iter(),
                )
            };
            for m in neighbours {
                let neighbour = &surfaces[m];
                // Solid shells on one layer are always a subset of those on the previous shell layer: a
                // hollow sloping vase gets bottom shells in its base but not in its walls.
                let internal: Vec<ExPolygon> =
                    [neighbour.internal.as_slice(), &neighbour.internal_solid].concat();
                let mut new_internal_solid = intersection_ex_safety(&solid, &internal);
                if new_internal_solid.is_empty() {
                    // No solid needed here; with the shells disabled the search stops.
                    break;
                }
                // Discard shells thinner than a wall (likely just crossing a sloping wall); with sparse
                // infill only those thinner than half a wall.
                let factor = if s.fill_density == 0.0 { 1.0 } else { 0.5 };
                let margin = layers[m].external.scaled_width() as f64 * factor;
                let too_narrow = diff_ex(
                    &new_internal_solid,
                    &opening2(
                        &new_internal_solid,
                        margin,
                        margin + CLIPPER_SAFETY_OFFSET,
                        5.0,
                    ),
                );
                if !too_narrow.is_empty() {
                    new_internal_solid = diff_ex(&new_internal_solid, &too_narrow);
                    solid = new_internal_solid.clone();
                }
                // Make sure the new solid is wide enough not to collapse when filled: grow the collapsing
                // parts within the neighbour's internal area.
                let margin = layers[i].solid.scaled_width() as f64;
                let too_narrow = diff_ex(
                    &new_internal_solid,
                    &opening2(
                        &new_internal_solid,
                        margin,
                        margin + CLIPPER_SAFETY_OFFSET,
                        5.0,
                    ),
                );
                if !too_narrow.is_empty() {
                    new_internal_solid
                        .extend(intersection_ex(&offset(&too_narrow, margin), &internal));
                }
                // The new internal solid joins the existing one; the rest of the internal stays sparse.
                let neighbour = &mut surfaces[m];
                new_internal_solid.extend(std::mem::take(&mut neighbour.internal_solid));
                let internal_solid = union_ex(&new_internal_solid);
                let internal = diff_ex_safety(&neighbour.internal, &internal_solid);
                let polygons_internal: Vec<ExPolygon> =
                    [internal_solid.as_slice(), &internal].concat();
                for list in [
                    &mut neighbour.top,
                    &mut neighbour.bottom,
                    &mut neighbour.bottom_bridge,
                ] {
                    if !list.is_empty() {
                        *list = diff_ex(list, &polygons_internal);
                    }
                }
                neighbour.internal_solid = internal_solid;
                neighbour.internal = internal;
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Top,
    Bottom,
    BottomBridge,
}

impl Kind {
    fn of_slices(self, s: &TypedSlices) -> &[ExPolygon] {
        match self {
            Kind::Top => &s.top,
            Kind::Bottom => &s.bottom,
            Kind::BottomBridge => &s.bottom_bridge,
        }
    }

    fn of_surfaces(self, s: &FillSurfaces) -> &[ExPolygon] {
        match self {
            Kind::Top => &s.top,
            Kind::Bottom => &s.bottom,
            Kind::BottomBridge => &s.bottom_bridge,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> ExPolygon {
        ExPolygon {
            contour: vec![
                Point::from_mm(x0, y0),
                Point::from_mm(x1, y0),
                Point::from_mm(x1, y1),
                Point::from_mm(x0, y1),
            ],
            holes: Vec::new(),
        }
    }

    fn total(ex: &[ExPolygon]) -> f64 {
        ex.iter().map(area).sum::<f64>() / (SCALE * SCALE)
    }

    #[test]
    fn orca_small_sparse_threshold_runs_after_surface_expansion() {
        let mut s = PrintSettings {
            solid_infill_below_area: 70.,
            minimum_sparse_infill_area: Some(15.),
            ..PrintSettings::core_one_hf04_speed_pla()
        };
        let input = FillSurfaces {
            internal: vec![rect(0., 0., 4., 3.), rect(10., 0., 15., 6.)],
            ..FillSurfaces::default()
        };
        let mut orca = input.clone();
        prepare_fill_surfaces(&mut orca, &s);
        assert!(orca.internal_solid.is_empty());
        let flow = Flow::new(0.45, 0.2);
        process_external_surfaces(
            &mut orca,
            &ShellLayer {
                print_z: 1.,
                height: 0.2,
                external: flow,
                perimeter: flow,
                solid: flow,
            },
            &s,
        );
        assert!((total(&orca.internal_solid) - 12.).abs() < 1e-5);
        assert!((total(&orca.internal) - 30.).abs() < 1e-5);
        s.minimum_sparse_infill_area = None;
        let mut prusa = input;
        prepare_fill_surfaces(&mut prusa, &s);
        assert!(prusa.internal.is_empty());
        assert!((total(&prusa.internal_solid) - 42.).abs() < 1e-5);
    }

    #[test]
    fn expansion_params_follow_prusaslicer() {
        // Perimeter spacing 0.407 mm into sparse: a single step of 80% after a 20% tiny expansion.
        let p = ExpansionParams::build(0.407 * SCALE, 0.1 * SCALE, 5);
        assert_eq!(p.num_other_steps, 0);
        assert!((p.tiny_expansion - 0.2 * 0.407 * SCALE).abs() < 1.0);
        assert!((p.initial_step - 0.8 * 0.407 * SCALE).abs() < 1.0);
        // 1.2 mm into solid: 5 steps of 0.23 mm after 0.05 mm.
        let p = ExpansionParams::build(1.2 * SCALE, 0.1 * SCALE, 5);
        assert_eq!(p.num_other_steps, 4);
        assert!((p.tiny_expansion - 0.05 * SCALE).abs() < 1.0);
        assert!((p.initial_step - 0.23 * SCALE).abs() < 1.0);
    }

    #[test]
    fn waves_stay_inside_the_boundary() {
        // A 2 x 10 source next to a 10 x 10 boundary with a wall in the middle: growth of 1 mm reaches
        // 1 mm into the boundary only where it touches the source.
        let src = vec![rect(0.0, 0.0, 2.0, 10.0)];
        let boundary = vec![rect(2.0, 0.0, 12.0, 10.0)];
        let p = ExpansionParams::build(1.0 * SCALE, 0.1 * SCALE, 5);
        let e = propagate_waves(&src, &boundary, &p);
        assert_eq!(e.len(), 1);
        let a = total(&e[0].polygons);
        assert!((a - 10.0).abs() < 0.3, "grown area {a}");
        // A boundary that doesn't touch the source isn't grown into.
        let far = vec![rect(5.0, 0.0, 12.0, 10.0)];
        assert!(propagate_waves(&src, &far, &p).is_empty());
    }
}
