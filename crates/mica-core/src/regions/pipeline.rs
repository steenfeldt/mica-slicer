// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/Layer.cpp :: has_compatible_layer_regions, Layer::make_perimeters
// Source: prusaslicer@b028299c77 src/libslic3r/Fill/Fill.cpp :: group_fills
// Source: prusaslicer@b028299c77 src/libslic3r/PrintObject.cpp :: PrintObject::detect_surfaces_type, PrintObject::discover_vertical_shells
// Changes: Indexed region geometry, whole-object neighbour masks and single-material shell inset, compatible wall/fill grouping, fuzzy facet hooks, texture skin settings in the wall grouping and batched paths.
// Changes: Parallel per-layer compensation and shared fill preparation, preserving indexed layer order.
// Changes: Regions are classified in parallel, one shared neighbour cache per flow-dimension set.
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/ExtrusionOrder.cpp :: get_slices_extrusions, extract_island_extrusions, extract_infill_ranges
// Source: prusaslicer@b028299c77 src/libslic3r/Layer.cpp :: Layer::make_slices, Layer::sort_perimeters_into_islands
// Source: prusaslicer@b028299c77 src/libslic3r/ShortestPath.cpp :: chain_expolygons, chain_layer_islands, chain_and_reorder_layer_islands
// Changes: Prusa profiles print island by island (lslices chained, islands chained from the last
// Changes: position, each island's perimeters then its fills per owning region, all fill types
// Changes: of one region chained together); islands are matched to lslices by containment.
// Source: orca@8500fcdcca src/libslic3r/GCode.cpp :: GCode::process_layer, GCode::extrude_perimeters, GCode::extrude_infill
// Source: orca@8500fcdcca src/libslic3r/Layer.cpp :: Layer::make_slices
// Changes: Orca profiles print by lslices island too (lslices in chain order, no chaining from the
// Changes: last position): every region's perimeters, then every region's fills, each region's fills
// Changes: chained together; fills are assigned to the island they lie in.
// Changes: Each island's ironing follows all of its regions' fills, in region order, chained
// Changes: from the preceding extrusion endpoint; Prusa retains the layer-end ironing pass.
// Changes: Layer toolpaths may be generated ahead of the tool-usage prepass while the object's supports still run (bounded, moved out on first use).
// Upstream attribution (Layer.cpp):
// Copyright (c) Prusa Research 2016 - 2023 Vojtěch Bubník @bubnikv, Pavel Mikuš @Godrak, Lukáš Hejl @hejllukas
// Copyright (c) Slic3r 2014 - 2015 Alessandro Ranellucci @alranel
// Copyright (c) 2015 Maksim Derbasov @ntfshard
// Changes: Spiral vase: walls know their layer's print Z (LayerRegion::make_perimeters), and bottom layers start where the layer below ended.
//
// ported from lib/Slic3r/Layer.pm:
// Copyright (c) Prusa Research 2016 - 2022 Vojtěch Bubník @bubnikv
// Copyright (c) Slic3r 2011 - 2016 Alessandro Ranellucci @alranel
//
// PrusaSlicer is released under the terms of the AGPLv3 or higher
//
// Upstream attribution (Fill/Fill.cpp):
// Copyright (c) Prusa Research 2016 - 2023 Lukáš Matěna @lukasmatena, Vojtěch Bubník @bubnikv, Pavel Mikuš @Godrak, Lukáš Hejl @hejllukas
// Copyright (c) SuperSlicer 2023 Remi Durand @supermerill
// Copyright (c) 2016 Sakari Kapanen @Flannelhead
// Copyright (c) Slic3r 2011 - 2015 Alessandro Ranellucci @alranel
// Copyright (c) 2013 Mark Hindess
// Copyright (c) 2011 Michael Moon
//
// PrusaSlicer is released under the terms of the AGPLv3 or higher
//

use super::{LayerRegion, PrintRegion, RegionSlices};
use crate::fill::PreparedFill;
use crate::geometry::clip::{Join, intersection_ex, offset_ex, union_ex};
use crate::geometry::{ExPolygon, Point};
use crate::mesh::TriangleMesh;
use crate::perimeters::IslandWalls;
use crate::print::{
    FillAreas, LayerFlows, LayerInfo, island_toolpaths, make_region_walls_with_painting,
};
use crate::shells::{FillSurfaces, ShellLayer};
use crate::toolpath::LayerToolpaths;
use rayon::prelude::*;

pub(crate) struct PreparedRegions {
    pub table: Vec<PrintRegion>,
    pub layers: Vec<Vec<LayerRegion>>,
    infill: Vec<PreparedFill>,
    fuzzy: crate::painting::query::FuzzyPainting,
    wall_groups: Vec<Vec<usize>>,
    fill_groups: Vec<(FillKind, Vec<usize>)>,
    /// Walls of every layer, per wall group, generated once while preparing the fill areas and
    /// reused by every toolpath pass (the tool-usage prepass and each export batch previously
    /// regenerated them). With Orca profiles their `infill_area` has been moved into the fill
    /// surfaces; Prusa profiles keep it to clip each island's fills. `None` marks a
    /// group with no active region on that layer; a missing layer (cancelled preparation) is
    /// generated on demand.
    wall_cache: Vec<LayerWalls>,
    /// Layer toolpaths generated ahead of the tool-usage prepass ([`PreparedRegions::warm`]),
    /// with their retained bytes; [`PreparedRegions::layer`] moves them out on first use.
    warm: Vec<std::sync::Mutex<Option<(WarmLayer, usize)>>>,
    /// Spiral vase: where each bottom layer's toolpaths start (the previous layer's end), as
    /// `PreparedObject::spiral_starts`.
    pub(crate) starts: Vec<Point>,
}

/// One layer's toolpaths and the region of each path, as [`PreparedRegions::layer`] returns them.
type WarmLayer = (LayerToolpaths, Vec<Option<usize>>);

/// Bytes of warmed layer toolpaths retained by all prepared objects of the process.
static WARM_BYTES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
/// No layer is warmed while the warmed layers of all objects hold this many bytes.
const WARM_BUDGET: usize = 128 << 20;
/// Layers warmed in one parallel step between checks of the stop condition.
const WARM_CHUNK: usize = 16;

impl Drop for PreparedRegions {
    fn drop(&mut self) {
        self.drop_warm();
    }
}

/// One layer's walls per wall group: the owning region and every island's walls, or `None` when
/// the group has no active region on that layer.
type LayerWalls = Vec<Option<(usize, Vec<(ExPolygon, IslandWalls)>)>>;
/// One layer's fill areas per region.
type LayerFills = Vec<(usize, Vec<ExPolygon>)>;

const WALL_KEYS: &[&str] = &[
    "perimeter_extruder",
    "perimeters",
    "perimeter_speed",
    "external_perimeter_speed",
    "small_perimeter_speed",
    "small_perimeter_threshold",
    "gap_fill_enabled",
    "gap_fill_speed",
    "filter_out_gap_fill",
    "initial_layer_min_bead_width",
    "overhangs",
    "perimeter_extrusion_width",
    "external_perimeter_extrusion_width",
    "thin_walls",
    "external_perimeters_first",
    "wall_sequence",
    "overhang_reverse",
    "overhang_reverse_internal_only",
    "overhang_reverse_threshold",
    "infill_overlap",
    "perimeter_generator",
    "wall_distribution_count",
    "wall_transition_length",
    "wall_transition_angle",
    "wall_transition_filter_deviation",
    "wall_transition_filter_distance",
    "min_bead_width",
    "min_feature_size",
    "fuzzy_skin",
    "fuzzy_skin_thickness",
    "fuzzy_skin_point_dist",
    "seam_position",
    "seam_gap_distance",
    "enable_dynamic_overhang_speeds",
    "overhang_speed_0",
    "overhang_speed_1",
    "overhang_speed_2",
    "overhang_speed_3",
    "solid_infill_extrusion_width",
    "extrusion_width",
    "first_layer_extrusion_width",
    "first_layer_speed",
    "default_acceleration",
    "first_layer_acceleration",
    "external_perimeter_acceleration",
    "perimeter_acceleration",
    "bridge_speed",
    "bridge_acceleration",
    "bridge_flow_ratio",
    "top_one_perimeter_type",
    "min_width_top_surface",
    "only_one_perimeter_first_layer",
    "staggered_inner_seams",
    "scarf_seam_placement",
    "seam_slope_type",
    "seam_slope_conditional",
    "seam_slope_entire_loop",
    "seam_slope_inner_walls",
    "seam_slope_min_length",
    "seam_slope_start_height",
    "seam_slope_steps",
    "scarf_angle_threshold",
    "scarf_overhang_threshold",
    "scarf_joint_speed",
    "scarf_joint_flow_ratio",
    "scarf_seam_only_on_smooth",
    "scarf_seam_length",
    "scarf_seam_entire_loop",
    "scarf_seam_max_segment_length",
    "scarf_seam_start_height",
    "scarf_seam_on_inner_perimeters",
];
// Fill compatibility is per surface type: changing sparse density must not split a top skin.
#[derive(Clone, Copy)]
enum FillKind {
    Bridge,
    InternalBridge,
    Solid,
    Bottom,
    Sparse,
    Top,
}
impl FillKind {
    fn keys(self) -> &'static [&'static str] {
        match self {
            Self::Bridge | Self::InternalBridge => &[
                "solid_infill_extruder",
                "solid_infill_extrusion_width",
                "top_fill_pattern",
                "bridge_speed",
                "bridge_acceleration",
                "bridge_flow_ratio",
                "thick_bridges",
                "thick_internal_bridges",
                "bridge_density",
                "internal_bridge_density",
                "internal_bridge_speed",
            ],
            Self::Solid => &[
                "filter_out_gap_fill",
                "small_area_infill_flow_compensation",
                "small_area_infill_flow_compensation_model",
                "solid_infill_extruder",
                "solid_infill_extrusion_width",
                "internal_solid_infill_pattern",
                "solid_infill_speed",
                "solid_infill_acceleration",
            ],
            Self::Bottom => &[
                "filter_out_gap_fill",
                "small_area_infill_flow_compensation",
                "small_area_infill_flow_compensation_model",
                "solid_infill_extruder",
                "solid_infill_extrusion_width",
                "bottom_fill_pattern",
                "solid_infill_speed",
                "solid_infill_acceleration",
            ],
            Self::Sparse => &[
                "infill_extruder",
                "fill_density",
                "fill_pattern",
                "infill_anchor",
                "infill_anchor_max",
                "infill_extrusion_width",
                "infill_speed",
                "infill_acceleration",
                "infill_every_layers",
                "infill_shift_step",
                "symmetric_infill_y_axis",
            ],
            Self::Top => &[
                "filter_out_gap_fill",
                "small_area_infill_flow_compensation",
                "small_area_infill_flow_compensation_model",
                "solid_infill_extruder",
                "top_infill_extrusion_width",
                "top_fill_pattern",
                "top_solid_infill_speed",
                "top_solid_infill_acceleration",
            ],
        }
    }
    fn append(self, to: &mut FillSurfaces, from: &FillSurfaces) {
        match self {
            Self::Bridge => {
                to.bottom_bridge.extend_from_slice(&from.bottom_bridge);
                to.bridges.extend_from_slice(&from.bridges);
            }
            Self::InternalBridge => to.internal_bridge.extend_from_slice(&from.internal_bridge),
            Self::Solid => to.internal_solid.extend_from_slice(&from.internal_solid),
            Self::Bottom => to.bottom.extend_from_slice(&from.bottom),
            Self::Sparse => {
                to.internal.extend_from_slice(&from.internal);
                to.combined.extend_from_slice(&from.combined);
            }
            Self::Top => to.top.extend_from_slice(&from.top),
        }
    }
}
const COMMON_FILL_KEYS: &[&str] = &[
    "fill_angle",
    "extrusion_width",
    "first_layer_extrusion_width",
    "first_layer_speed",
    "first_layer_acceleration",
    "default_acceleration",
    "gcode_resolution",
    "perimeter_generator",
    "infill_overlap",
    // Percentage speeds resolve against these base speeds.
    "perimeter_speed",
    "infill_speed",
    "solid_infill_speed",
];
fn groups(table: &[PrintRegion], keys: &[&str]) -> Vec<Vec<usize>> {
    let mut groups: Vec<Vec<usize>> = vec![];
    for i in 0..table.len() {
        if let Some(g) = groups.iter_mut().find(|g| {
            keys.iter()
                .all(|k| table[g[0]].config.raw(k) == table[i].config.raw(k))
        }) {
            g.push(i);
        } else {
            groups.push(vec![i]);
        }
    }
    groups
}

impl PreparedRegions {
    #[allow(clippy::too_many_arguments)]
    pub fn prepare(
        mut sliced: RegionSlices,
        mesh: &TriangleMesh,
        outlines: &[Vec<ExPolygon>],
        tops: &[f64],
        heights: &[f64],
        origin: Point,
        fuzzy: crate::painting::query::FuzzyPainting,
        observer: &dyn crate::progress::Observer,
    ) -> Self {
        let _timing = crate::timing::Span::new(crate::timing::Stage::RegionPreparation);
        use crate::progress::{Stage, Ticker};
        // Whole-object compensation trims regions, never moves an internal boundary twice.
        sliced
            .layers
            .par_iter_mut()
            .zip(outlines)
            .for_each(|(layer, outline)| {
                for r in layer {
                    r.slices = intersection_ex(&r.slices, outline);
                }
            });
        let mut out = Self {
            // Regions with other texture settings (a modifier) get their own walls too.
            wall_groups: groups(&sliced.table, &[WALL_KEYS, crate::texture::KEYS].concat()),
            fill_groups: [
                FillKind::Bridge,
                FillKind::InternalBridge,
                FillKind::Solid,
                FillKind::Bottom,
                FillKind::Sparse,
                FillKind::Top,
            ]
            .into_iter()
            .flat_map(|kind| {
                let keys = [COMMON_FILL_KEYS, kind.keys()].concat();
                let table = &sliced.table;
                groups(&sliced.table, &keys)
                    .into_iter()
                    .flat_map(move |g| {
                        // Lightning's forest is prepared from one region's shell surfaces.
                        if matches!(kind, FillKind::Sparse)
                            && table[g[0]].settings.fill_pattern == crate::fill::Pattern::Lightning
                        {
                            g.into_iter().map(|id| vec![id]).collect()
                        } else {
                            vec![g]
                        }
                    })
                    .map(move |g| (kind, g))
            })
            .collect(),
            infill: vec![PreparedFill::default(); sliced.table.len()],
            table: sliced.table,
            layers: sliced.layers,
            fuzzy,
            wall_cache: Vec::new(),
            warm: Vec::new(),
            starts: Vec::new(),
        };
        let walls_timer = crate::timing::Span::new(crate::timing::Stage::RegionWalls);
        // Walls of every layer first (0 .. 0.6 of the stage), then one step per region.
        let ticker = Ticker::span(observer, Stage::Surfaces, tops.len(), 0., 0.6);
        let (fills, wall_cache): (Vec<LayerFills>, Vec<LayerWalls>) = (0..tops.len())
            .into_par_iter()
            .map(|i| {
                let mut fills = vec![];
                let mut cached = Vec::with_capacity(out.wall_groups.len());
                if ticker.cancelled() {
                    return (fills, cached);
                }
                ticker.tick();
                for group in &out.wall_groups {
                    let (id, mut walls) =
                        out.generate_walls(group, i, outlines, (tops[i], heights[i]), origin);
                    let Some(id) = id else {
                        cached.push(None);
                        continue;
                    };
                    // Orca's toolpath passes never read the infill area again (they cleared it);
                    // the Prusa order clips each island's fills to it.
                    let keep = out.prusa_order();
                    let fill: Vec<_> = walls
                        .iter_mut()
                        .flat_map(|(_, w)| {
                            if keep {
                                w.infill_area.clone()
                            } else {
                                std::mem::take(&mut w.infill_area)
                            }
                        })
                        .collect();
                    for r in out.layers[i].iter().filter(|r| group.contains(&r.region)) {
                        fills.push((r.region, intersection_ex(&fill, &r.slices)));
                    }
                    cached.push(Some((id, walls)));
                }
                (fills, cached)
            })
            .unzip();
        out.wall_cache = wall_cache;
        drop(walls_timer);
        let shared_timer = crate::timing::Span::new(crate::timing::Stage::SharedFill);
        let shared_fill: Vec<_> = fills
            .par_iter()
            .enumerate()
            .map(|(i, l)| {
                let mut holes = l.iter().flat_map(|(_, f)| f.clone()).collect::<Vec<_>>();
                // Prusa's multi-region shell cache includes a simulated single-material
                // perimeter inset. Internal material boundaries must not create solid shells.
                if out.table.len() > 1 {
                    let mut inset: f64 = 0.;
                    let mut spacing = f64::INFINITY;
                    for r in &out.layers[i] {
                        let s = &out.table[r.region].settings;
                        let f = LayerFlows::new(s, i, heights[i]);
                        if s.perimeters > 0 {
                            inset = inset.max(
                                0.5 * (f.external.width + f.external.spacing())
                                    + (s.perimeters - 1) as f64 * f.perimeter.spacing(),
                            );
                            spacing = spacing.min(f.external.spacing().min(f.perimeter.spacing()));
                        }
                    }
                    if inset > 0. {
                        holes.extend(offset_ex(
                            &offset_ex(
                                &outlines[i],
                                0.3 * spacing * crate::geometry::SCALE,
                                Join::Miter,
                            ),
                            -(inset + 0.3 * spacing) * crate::geometry::SCALE,
                            Join::Miter,
                        ));
                    }
                }
                union_ex(&holes)
            })
            .collect();
        drop(shared_timer);
        let _shells_timer = crate::timing::Span::new(crate::timing::Stage::Shells);
        let regions = Ticker::span(observer, Stage::Surfaces, out.table.len(), 0.6, 1.);
        let shell_layers: Vec<Vec<ShellLayer>> = out
            .table
            .iter()
            .map(|region| {
                (0..tops.len())
                    .map(|i| {
                        let f = LayerFlows::new(&region.settings, i, heights[i]);
                        ShellLayer {
                            print_z: tops[i],
                            height: heights[i],
                            external: f.external,
                            perimeter: f.perimeter,
                            solid: f.solid,
                        }
                    })
                    .collect()
            })
            .collect();
        // The neighbour caches the sequential loop used: a new one (from this region's layers
        // and crack rule) whenever the previous cache's flow dimensions do not match. Built up
        // front, so the regions themselves can run in parallel.
        let mut shell_caches: Vec<crate::shells::SharedShells> = Vec::new();
        let cache_of: Vec<usize> = shell_layers
            .iter()
            .zip(&out.table)
            .map(|(layers, region)| {
                if shell_caches
                    .last()
                    .is_none_or(|cache| !cache.matches(layers))
                {
                    shell_caches.push(crate::shells::SharedShells::new(
                        outlines,
                        layers,
                        region.settings.minimum_sparse_infill_area.is_some(),
                    ));
                }
                shell_caches.len() - 1
            })
            .collect();
        // Regions are independent here: each reads only its own slices and fill, the shared
        // fill holes and the shared caches. They run in parallel and are stored in region order.
        let classified: Vec<Option<(Vec<FillSurfaces>, PreparedFill)>> = (0..out.table.len())
            .into_par_iter()
            .map(|id| {
                if regions.cancelled() {
                    return None;
                }
                regions.tick();
                let s = &out.table[id].settings;
                let region_outlines: Vec<_> = out
                    .layers
                    .iter()
                    .map(|l| {
                        l.iter()
                            .find(|r| r.region == id)
                            .map(|r| r.slices.clone())
                            .unwrap_or_default()
                    })
                    .collect();
                let fill = fills
                    .iter()
                    .map(|l| {
                        l.iter()
                            .find(|(r, _)| *r == id)
                            .map(|(_, f)| f.clone())
                            .unwrap_or_default()
                    })
                    .collect();
                let mut surfaces = crate::shells::classify_region_at(
                    &region_outlines,
                    outlines,
                    fill,
                    Some((&shared_fill, &shell_caches[cache_of[id]])),
                    &shell_layers[id],
                    s,
                    origin,
                );
                let mut infill = PreparedFill::prepare(mesh, s, &surfaces, origin);
                infill.finish_surfaces(&mut surfaces, tops, heights, s, origin);
                Some((surfaces, infill))
            })
            .collect();
        if regions.cancelled() || classified.iter().any(Option::is_none) {
            // The caller checks the observer and drops this result.
            return out;
        }
        for (id, (surfaces, infill)) in classified.into_iter().flatten().enumerate() {
            out.infill[id] = infill;
            for (l, f) in out.layers.iter_mut().zip(surfaces) {
                if let Some(r) = l.iter_mut().find(|r| r.region == id) {
                    r.surfaces = f;
                }
            }
        }
        // Cancel checks per layer: bridging a large model can take most of a minute.
        super::surfaces::bridge_and_combine(
            &out.table,
            &mut out.layers,
            tops,
            heights,
            origin,
            &|| regions.cancelled(),
        );
        out
    }

    fn generate_walls(
        &self,
        group: &[usize],
        i: usize,
        outlines: &[Vec<ExPolygon>],
        (print_z, height): (f64, f64),
        origin: Point,
    ) -> (Option<usize>, Vec<(ExPolygon, IslandWalls)>) {
        let active: Vec<_> = self.layers[i]
            .iter()
            .filter(|r| group.contains(&r.region) && !r.slices.is_empty())
            .collect();
        let Some(best) = active.iter().max_by(|a, b| {
            self.table[a.region]
                .settings
                .fill_density
                .total_cmp(&self.table[b.region].settings.fill_density)
        }) else {
            return (None, vec![]);
        };
        let slices = union_ex(
            &active
                .iter()
                .flat_map(|r| r.slices.clone())
                .collect::<Vec<_>>(),
        );
        (
            Some(best.region),
            make_region_walls_with_painting(
                &slices,
                outlines,
                &self.table[best.region].settings,
                i,
                print_z,
                height,
                origin,
                &crate::painting::query::FuzzyLayer {
                    painting: &self.fuzzy,
                    settings: &self.table[best.region].settings.fuzzy_skin,
                    radius: LayerFlows::new(&self.table[best.region].settings, i, height)
                        .external
                        .width,
                    origin: Point::default(),
                },
            ),
        )
    }

    /// The toolpaths of layer `i`. Always called with the object's own outlines, layer
    /// heights, origin and seams, so a layer warmed beforehand is the same result.
    #[allow(clippy::too_many_arguments)]
    pub fn layer(
        &self,
        i: usize,
        outlines: &[Vec<ExPolygon>],
        tops: &[f64],
        heights: &[f64],
        origin: Point,
        seams: &crate::seam::SeamPlan,
    ) -> (LayerToolpaths, Vec<Option<usize>>) {
        if let Some(slot) = self.warm.get(i)
            && let Some((layer, bytes)) = slot.lock().unwrap_or_else(|p| p.into_inner()).take()
        {
            WARM_BYTES.fetch_sub(bytes, std::sync::atomic::Ordering::Relaxed);
            return layer;
        }
        self.generate_layer(i, outlines, tops, heights, origin, seams)
    }

    /// Generates layer toolpaths, lowest layers first, ahead of the tool-usage prepass that
    /// would generate them next, while `busy` stays set (another stage of the object, the
    /// supports, still runs and leaves workers idle). Layers are pure functions of the
    /// prepared object, so a warmed layer equals the one generated later; how many get
    /// warmed depends on timing only. Bounded by [`WARM_BUDGET`] over all objects.
    #[allow(clippy::too_many_arguments)]
    pub fn warm(
        &mut self,
        outlines: &[Vec<ExPolygon>],
        tops: &[f64],
        heights: &[f64],
        origin: Point,
        seams: &crate::seam::SeamPlan,
        busy: &std::sync::atomic::AtomicBool,
        observer: &dyn crate::progress::Observer,
    ) {
        use std::sync::atomic::Ordering::Relaxed;
        let n = self.layers.len().min(tops.len());
        self.warm = (0..n).map(|_| std::sync::Mutex::new(None)).collect();
        let this = &*self;
        let stop = || {
            !busy.load(Relaxed)
                || WARM_BYTES.load(Relaxed) >= WARM_BUDGET
                || crate::progress::check(observer).is_err()
        };
        for begin in (0..n).step_by(WARM_CHUNK) {
            if stop() {
                break;
            }
            (begin..(begin + WARM_CHUNK).min(n))
                .into_par_iter()
                .for_each(|i| {
                    if stop() {
                        return;
                    }
                    let layer = this.generate_layer(i, outlines, tops, heights, origin, seams);
                    let bytes = layer.0.heap_bytes()
                        + layer.1.capacity() * std::mem::size_of::<Option<usize>>();
                    WARM_BYTES.fetch_add(bytes, Relaxed);
                    *this.warm[i].lock().unwrap_or_else(|p| p.into_inner()) = Some((layer, bytes));
                });
        }
    }

    /// Drops the warmed layers nobody used (the prepass and export are done with them).
    pub fn drop_warm(&self) {
        for slot in &self.warm {
            if let Some((_, bytes)) = slot.lock().unwrap_or_else(|p| p.into_inner()).take() {
                WARM_BYTES.fetch_sub(bytes, std::sync::atomic::Ordering::Relaxed);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn generate_layer(
        &self,
        i: usize,
        outlines: &[Vec<ExPolygon>],
        tops: &[f64],
        heights: &[f64],
        origin: Point,
        seams: &crate::seam::SeamPlan,
    ) -> (LayerToolpaths, Vec<Option<usize>>) {
        if self.prusa_order() {
            return self.layer_prusa(i, outlines, tops, heights, origin, seams);
        }
        self.layer_orca(i, outlines, tops, heights, origin, seams)
    }

    /// Orca's print order of one layer (`GCode::process_layer`): the islands in `lslices` order
    /// (`Layer::make_slices` chains their first contour points); an extrusion belongs to the
    /// island holding it. Per island, the perimeters of every region in region order, then each
    /// region's fills chained together from the last position (`extrude_perimeters`,
    /// `extrude_infill`), then each region's ironing from the last extrusion endpoint.
    /// Printing per extruder keeps this order for each tool.
    fn layer_orca(
        &self,
        i: usize,
        outlines: &[Vec<ExPolygon>],
        tops: &[f64],
        heights: &[f64],
        origin: Point,
        seams: &crate::seam::SeamPlan,
    ) -> (LayerToolpaths, Vec<Option<usize>>) {
        // Walls of every wall group (cached, or generated when preparation stopped early).
        let generated: Vec<_> = (0..self.wall_groups.len())
            .map(
                |gi| match self.wall_cache.get(i).and_then(|layer| layer.get(gi)) {
                    Some(_) => None,
                    None => {
                        let (id, mut walls) = self.generate_walls(
                            &self.wall_groups[gi],
                            i,
                            outlines,
                            (tops[i], heights[i]),
                            origin,
                        );
                        for (_, w) in &mut walls {
                            w.infill_area.clear();
                        }
                        id.map(|id| (id, walls))
                    }
                },
            )
            .collect();
        let mut islands: Vec<(usize, &ExPolygon, &IslandWalls)> = vec![];
        for (gi, own) in generated.iter().enumerate() {
            let entry = match self.wall_cache.get(i).and_then(|layer| layer.get(gi)) {
                Some(cached) => cached.as_ref(),
                None => own.as_ref(),
            };
            if let Some((id, walls)) = entry {
                islands.extend(walls.iter().map(|(ex, w)| (*id, ex, w)));
            }
        }
        // `by_region`: the fills of one print region (a merged fill is stored in its first
        // region) are chained together.
        let mut owners: Vec<(usize, FillSurfaces)> = vec![];
        for &(kind, ref group) in &self.fill_groups {
            let Some((id, surfaces)) = self.group_surfaces(i, kind, group) else {
                continue;
            };
            match owners.iter_mut().find(|(o, _)| *o == id) {
                Some((_, to)) => kind.append(to, &surfaces),
                None => owners.push((id, surfaces)),
            }
        }
        owners.sort_by_key(|(id, _)| *id);
        let mut everything = vec![];
        let owners: Vec<(usize, FillAreas)> = owners
            .into_iter()
            .map(|(id, mut surfaces)| {
                merge_surfaces(&mut surfaces);
                everything.extend(fill_area(&surfaces));
                (id, FillAreas::new(&surfaces))
            })
            .collect();

        let lslices = &outlines[i];
        // Fills outside every island print with the last (catch-all) island.
        let outside = if everything.is_empty() {
            vec![]
        } else {
            crate::geometry::clip::diff_ex(&union_ex(&everything), lslices)
        };
        let firsts: Vec<Point> = lslices
            .iter()
            .map(|e| e.contour.first().copied().unwrap_or_default())
            .collect();
        let slice_order = crate::shortest_path::chain_points(&firsts, None);
        let mut per_slice: Vec<Vec<usize>> = vec![vec![]; lslices.len() + 1];
        for (k, (_, ex, _)) in islands.iter().enumerate() {
            per_slice[containing_slice(lslices, ex).unwrap_or(lslices.len())].push(k);
        }
        let info = |region: usize| LayerInfo {
            lower: i.checked_sub(1).map(|j| outlines[j].as_slice()),
            infill: &self.infill[region],
            seams,
            index: i,
            print_z: tops[i],
            origin,
        };
        let flows = |region: usize| LayerFlows::new(&self.table[region].settings, i, heights[i]);
        let no_fill = FillAreas::new(&FillSurfaces::default());

        let mut ironing: Vec<_> = self.layers[i]
            .iter()
            .map(|r| {
                (
                    r.region,
                    crate::ironing::collections(
                        &r.slices,
                        outlines.get(i + 1).map(Vec::as_slice),
                        i.checked_sub(1).map(|j| outlines[j].as_slice()),
                        &r.surfaces,
                        i,
                        tops[i],
                        origin,
                        &self.table[r.region].settings,
                    ),
                )
            })
            .collect();
        ironing.sort_by_key(|(id, _)| *id);

        let mut paths = vec![];
        let mut ids = vec![];
        let mut pos = self.starts.get(i).copied().unwrap_or_default();
        for slice in slice_order
            .into_iter()
            .chain(std::iter::once(lslices.len()))
        {
            let mut members = per_slice[slice].clone();
            // Regions print in region order, each region's perimeters as generated.
            members.sort_by_key(|&k| islands[k].0);
            for &k in &members {
                let (id, _, walls) = islands[k];
                let perimeters = IslandWalls {
                    infill_area: vec![],
                    ..walls.clone()
                };
                let s = &self.table[id].settings;
                island_toolpaths(
                    &perimeters,
                    &no_fill,
                    &info(id),
                    s,
                    &flows(id),
                    &mut paths,
                    &mut pos,
                );
                ids.resize(paths.len(), Some(id));
            }
            // The fills inside this island; anything outside every island goes with the last.
            let area = if slice < lslices.len() {
                vec![lslices[slice].clone()]
            } else {
                outside.clone()
            };
            for (owner, areas) in &owners {
                let fill_walls = IslandWalls {
                    infill_area: area.clone(),
                    ..Default::default()
                };
                let s = &self.table[*owner].settings;
                island_toolpaths(
                    &fill_walls,
                    areas,
                    &info(*owner),
                    s,
                    &flows(*owner),
                    &mut paths,
                    &mut pos,
                );
                ids.resize(paths.len(), Some(*owner));
            }
            for (region, collections) in &mut ironing {
                crate::ironing::append_orca_island(
                    collections,
                    lslices.get(slice),
                    &mut paths,
                    &mut pos,
                );
                ids.resize(paths.len(), Some(*region));
            }
        }
        (
            LayerToolpaths {
                support_groups: Vec::new(),
                index: i,
                print_z: tops[i],
                height: heights[i],
                paths,
            },
            ids,
        )
    }

    /// Prusa profiles print a layer island by island (GCode/ExtrusionOrder.cpp); Orca profiles
    /// by `lslices` island, see [`Self::layer_orca`].
    fn prusa_order(&self) -> bool {
        self.table
            .first()
            .is_some_and(|r| !r.settings.adhesion.orca)
    }

    /// One fill group's surfaces on layer `i`, owned by its first active region.
    fn group_surfaces(
        &self,
        i: usize,
        kind: FillKind,
        group: &[usize],
    ) -> Option<(usize, FillSurfaces)> {
        let active: Vec<_> = self.layers[i]
            .iter()
            .filter(|r| group.contains(&r.region))
            .collect();
        let id = active.first()?.region;
        let mut surfaces = FillSurfaces::default();
        for r in active {
            kind.append(&mut surfaces, &r.surfaces);
        }
        Some((id, surfaces))
    }

    /// PrusaSlicer's print order of one layer: `lslices` in `chain_expolygons` order; in each, its
    /// islands chained from the previous position (`chain_and_reorder_layer_islands`); per island
    /// the perimeters, then the fills of each region the island's fills are stored in, every
    /// region's fills (all surface types and the gap fill) chained as one set
    /// (`extract_island_extrusions`, `extract_infill_ranges`, `sort_fill_extrusions`).
    fn layer_prusa(
        &self,
        i: usize,
        outlines: &[Vec<ExPolygon>],
        tops: &[f64],
        heights: &[f64],
        origin: Point,
        seams: &crate::seam::SeamPlan,
    ) -> (LayerToolpaths, Vec<Option<usize>>) {
        // Walls of every wall group (cached, or generated when preparation stopped early).
        let generated: Vec<_> = (0..self.wall_groups.len())
            .map(
                |gi| match self.wall_cache.get(i).and_then(|layer| layer.get(gi)) {
                    Some(_) => None,
                    None => {
                        let (id, walls) = self.generate_walls(
                            &self.wall_groups[gi],
                            i,
                            outlines,
                            (tops[i], heights[i]),
                            origin,
                        );
                        id.map(|id| (id, walls))
                    }
                },
            )
            .collect();
        let mut islands: Vec<(usize, &ExPolygon, &IslandWalls)> = vec![];
        for (gi, own) in generated.iter().enumerate() {
            let entry = match self.wall_cache.get(i).and_then(|layer| layer.get(gi)) {
                Some(cached) => cached.as_ref(),
                None => own.as_ref(),
            };
            if let Some((id, walls)) = entry {
                islands.extend(walls.iter().map(|(ex, w)| (*id, ex, w)));
            }
        }
        // Fill surfaces per owning region: `group_fills` stores a merged fill in its first
        // region, and an island's fills of one region are chained together.
        let mut owners: Vec<(usize, FillSurfaces)> = vec![];
        for &(kind, ref group) in &self.fill_groups {
            let Some((id, surfaces)) = self.group_surfaces(i, kind, group) else {
                continue;
            };
            match owners.iter_mut().find(|(o, _)| *o == id) {
                Some((_, to)) => kind.append(to, &surfaces),
                None => owners.push((id, surfaces)),
            }
        }
        owners.sort_by_key(|(id, _)| *id);
        let owners: Vec<(usize, FillAreas)> = owners
            .into_iter()
            .map(|(id, mut surfaces)| {
                merge_surfaces(&mut surfaces);
                (id, FillAreas::new(&surfaces))
            })
            .collect();

        // Layer::make_slices: lslice_indices_sorted_by_print_order = chain_expolygons(lslices).
        let lslices = &outlines[i];
        let firsts: Vec<Point> = lslices
            .iter()
            .map(|e| e.contour.first().copied().unwrap_or_default())
            .collect();
        let slice_order = crate::shortest_path::chain_points(&firsts, None);
        // Layer::sort_perimeters_into_islands: each island belongs to the lslice containing it.
        let mut per_slice: Vec<Vec<usize>> = vec![vec![]; lslices.len() + 1];
        for (k, (_, ex, _)) in islands.iter().enumerate() {
            per_slice[containing_slice(lslices, ex).unwrap_or(lslices.len())].push(k);
        }

        let mut paths = vec![];
        let mut ids = vec![];
        let mut pos = self.starts.get(i).copied().unwrap_or_default();
        let no_fill = FillAreas::new(&FillSurfaces::default());
        for slice in slice_order
            .into_iter()
            .chain(std::iter::once(lslices.len()))
        {
            let members = &per_slice[slice];
            let firsts: Vec<Point> = members
                .iter()
                .map(|&k| islands[k].1.contour.first().copied().unwrap_or_default())
                .collect();
            for m in crate::shortest_path::chain_points(&firsts, Some(pos)) {
                let (id, _, walls) = islands[members[m]];
                let info = |region: usize| LayerInfo {
                    lower: i.checked_sub(1).map(|j| outlines[j].as_slice()),
                    infill: &self.infill[region],
                    seams,
                    index: i,
                    print_z: tops[i],
                    origin,
                };
                let flows =
                    |region: usize| LayerFlows::new(&self.table[region].settings, i, heights[i]);
                if owners.len() == 1 && owners[0].0 == id {
                    // One region: the island's walls and fills, exactly as a single-region object.
                    let s = &self.table[id].settings;
                    island_toolpaths(
                        walls,
                        &owners[0].1,
                        &info(id),
                        s,
                        &flows(id),
                        &mut paths,
                        &mut pos,
                    );
                    ids.resize(paths.len(), Some(id));
                    continue;
                }
                // Several regions: the perimeters, then each region's fills (or the other way
                // round with `infill_first`, as extract_island_extrusions does).
                let s = &self.table[id].settings;
                let owns_fill = owners.iter().any(|(o, _)| *o == id);
                for fills_now in if s.infill_first {
                    [true, false]
                } else {
                    [false, true]
                } {
                    if !fills_now {
                        let perimeters = IslandWalls {
                            infill_area: vec![],
                            gap_fill: if owns_fill {
                                vec![]
                            } else {
                                walls.gap_fill.clone()
                            },
                            ..walls.clone()
                        };
                        island_toolpaths(
                            &perimeters,
                            &no_fill,
                            &info(id),
                            s,
                            &flows(id),
                            &mut paths,
                            &mut pos,
                        );
                        ids.resize(paths.len(), Some(id));
                        continue;
                    }
                    for (owner, areas) in &owners {
                        let fill_walls = IslandWalls {
                            infill_area: walls.infill_area.clone(),
                            gap_fill: if *owner == id {
                                walls.gap_fill.clone()
                            } else {
                                vec![]
                            },
                            ..Default::default()
                        };
                        let s = &self.table[*owner].settings;
                        island_toolpaths(
                            &fill_walls,
                            areas,
                            &info(*owner),
                            s,
                            &flows(*owner),
                            &mut paths,
                            &mut pos,
                        );
                        ids.resize(paths.len(), Some(*owner));
                    }
                }
            }
        }
        for r in &self.layers[i] {
            paths.extend(crate::ironing::paths(
                &r.slices,
                outlines.get(i + 1).map(Vec::as_slice),
                i.checked_sub(1).map(|j| outlines[j].as_slice()),
                &r.surfaces,
                i,
                tops[i],
                origin,
                &self.table[r.region].settings,
            ));
            ids.resize(paths.len(), Some(r.region));
        }
        (
            LayerToolpaths {
                support_groups: Vec::new(),
                index: i,
                print_z: tops[i],
                height: heights[i],
                paths,
            },
            ids,
        )
    }
}

/// The lslice an island lies in: the one whose bounding box holds the island's and which holds
/// most of its contour vertices.
fn containing_slice(lslices: &[ExPolygon], island: &ExPolygon) -> Option<usize> {
    let bb = crate::geometry::BoundingBox::from_points(island.contour.iter())?;
    let tolerance = 10;
    let candidates: Vec<usize> = (0..lslices.len())
        .filter(|&k| {
            crate::geometry::BoundingBox::from_points(lslices[k].contour.iter()).is_some_and(|s| {
                s.min.x - tolerance <= bb.min.x
                    && s.min.y - tolerance <= bb.min.y
                    && s.max.x + tolerance >= bb.max.x
                    && s.max.y + tolerance >= bb.max.y
            })
        })
        .collect();
    if candidates.len() <= 1 {
        return candidates.first().copied();
    }
    candidates.into_iter().max_by_key(|&k| {
        island
            .contour
            .iter()
            .filter(|&&p| crate::shells::contains(&lslices[k], p))
            .count()
    })
}

/// Every fill surface of a group, unmerged.
fn fill_area(surfaces: &FillSurfaces) -> Vec<ExPolygon> {
    [
        surfaces.bottom.clone(),
        surfaces.top.clone(),
        surfaces.internal.clone(),
        surfaces.internal_solid.clone(),
        surfaces.bottom_bridge.clone(),
        surfaces
            .internal_bridge
            .iter()
            .map(|b| b.polygon.clone())
            .collect(),
        surfaces
            .combined
            .iter()
            .flat_map(|c| c.polygons.clone())
            .collect(),
    ]
    .concat()
}

/// Unions each surface type of a merged fill group, and merges bridges of equal angle.
fn merge_surfaces(surfaces: &mut FillSurfaces) {
    for area in [
        &mut surfaces.bottom,
        &mut surfaces.top,
        &mut surfaces.internal,
        &mut surfaces.internal_solid,
    ] {
        *area = union_ex(area);
    }
    merge_bridges(&mut surfaces.bridges);
    merge_bridges(&mut surfaces.internal_bridge);
    merge_combined(&mut surfaces.combined);
}

fn merge_bridges(surfaces: &mut Vec<crate::bridge::BridgeSurface>) {
    let mut groups: Vec<(f64, Vec<ExPolygon>)> = vec![];
    for surface in surfaces.drain(..) {
        if let Some((_, polygons)) = groups.iter_mut().find(|(angle, _)| *angle == surface.angle) {
            polygons.push(surface.polygon);
        } else {
            groups.push((surface.angle, vec![surface.polygon]));
        }
    }
    for (angle, polygons) in groups {
        surfaces.extend(
            union_ex(&polygons)
                .into_iter()
                .map(|polygon| crate::bridge::BridgeSurface { polygon, angle }),
        );
    }
}
fn merge_combined(surfaces: &mut Vec<super::combine::CombinedFill>) {
    let mut merged: Vec<super::combine::CombinedFill> = vec![];
    for surface in surfaces.drain(..) {
        if let Some(other) = merged
            .iter_mut()
            .find(|s| (s.height - surface.height).abs() < 1e-8)
        {
            other.polygons.extend(surface.polygons);
        } else {
            merged.push(surface);
        }
    }
    for surface in &mut merged {
        surface.polygons = union_ex(&surface.polygons);
    }
    *surfaces = merged;
}
