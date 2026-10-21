// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: orca@8500fcdcca src/libslic3r/Arachne/WallToolPaths.cpp :: make_paths_params
// Source: prusaslicer@b028299c77 src/libslic3r/Arachne/WallToolPaths.cpp :: WallToolPaths::generate, separateOutInnerContour, removeSmallAreas, simplify
// Source: prusaslicer@b028299c77 src/libslic3r/PerimeterGenerator.cpp :: process_arachne, traverse_extrusions
// Source: prusaslicer@b028299c77 src/libslic3r/LayerRegion.cpp :: LayerRegion::bridging_flow
// Source: prusaslicer@b028299c77 src/libslic3r/Flow.cpp :: Flow::with_cross_section
// Source: orca@8500fcdcca src/libslic3r/PerimeterGenerator.cpp :: PerimeterGenerator::process_arachne
// Changes: Precise outer-wall outline and first-bead offset; configurable single-wall top width filter and Orca's 0.85-wall extension.
// Changes: Rust graph/strategy modules; Clipper2 outline cleanup; variable paths retain their widths through seam clipping.
// Changes: Allow immutable neighbour/support geometry to be reused across a compatible wall group.
// Changes: Prusa profiles round the outline and infill-area offsets like Clipper 6.4 (`FRound`, halves upwards).
// Changes: Walls on a textured (displaced) mesh simplify their inner contour by 0.01 mm before its offsets.
// Copyright (c) 2022 Ultimaker B.V.
// CuraEngine is released under the terms of the AGPLv3 or higher.

//! Arachne variable-width wall generation. All junction widths are line *spacings* in scaled units.

mod beading;
pub mod clip;
mod graph;
mod order;
mod prepare;
mod simplify;
#[cfg(test)]
mod tests;
mod toolpaths;
mod transitions;

use crate::geometry::clip::{
    Join, diff_ex, intersection_ex, offset_ex, offset2_ex, to_polygons, union_ex, union_ex_even_odd,
};
use crate::geometry::medial_axis::ThickPolyline;
use crate::geometry::{ExPolygon, Point, Polygon, SCALE};
use crate::perimeters::{IslandWalls, WallParams, thick_polyline_to_multi_path};
use crate::settings::{FloatOrPercent, TopOnePerimeter};
use crate::toolpath::Role;
use beading::Strategy;
use graph::Graph;

/// How far (scaled) the inner contour of walls on a textured mesh may move when simplified.
const INNER_CONTOUR_TOLERANCE: f64 = 0.01 * SCALE;

/// Arachne's object settings. Percentages are resolved against the smallest nozzle diameter.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ArachneSettings {
    pub wall_transition_length: FloatOrPercent,
    pub wall_transition_filter_deviation: FloatOrPercent,
    pub wall_transition_angle: f64,
    pub wall_distribution_count: usize,
    pub min_feature_size: FloatOrPercent,
    pub min_bead_width: FloatOrPercent,
}
impl Default for ArachneSettings {
    fn default() -> Self {
        Self {
            wall_transition_length: FloatOrPercent::Percent(100.),
            wall_transition_filter_deviation: FloatOrPercent::Percent(25.),
            wall_transition_angle: 10.,
            wall_distribution_count: 1,
            min_feature_size: FloatOrPercent::Percent(25.),
            min_bead_width: FloatOrPercent::Percent(85.),
        }
    }
}
impl ArachneSettings {
    /// Validate the same ranges as PrusaSlicer's configuration schema.
    pub fn validate(&self) -> Result<(), &'static str> {
        for (name, value) in [
            ("wall_transition_length", self.wall_transition_length),
            (
                "wall_transition_filter_deviation",
                self.wall_transition_filter_deviation,
            ),
            ("min_feature_size", self.min_feature_size),
            ("min_bead_width", self.min_bead_width),
        ] {
            let value = value.resolve(1., 1.);
            if !value.is_finite() || value < 0. {
                return Err(name);
            }
        }
        if !(1. ..=59.).contains(&self.wall_transition_angle) {
            return Err("wall_transition_angle");
        }
        if self.wall_distribution_count == 0 {
            return Err("wall_distribution_count");
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Junction {
    pub p: Point,
    pub width: f64,
    pub perimeter: usize,
}
#[derive(Clone, Debug)]
pub struct ExtrusionLine {
    pub inset: usize,
    pub is_odd: bool,
    pub is_closed: bool,
    pub junctions: Vec<Junction>,
}
impl ExtrusionLine {
    pub fn length(&self) -> f64 {
        self.junctions
            .windows(2)
            .map(|w| w[0].p.distance_to(w[1].p))
            .sum()
    }
    pub fn polygon(&self) -> Polygon {
        self.junctions.iter().map(|j| j.p).collect()
    }
}
#[derive(Debug, Default)]
pub struct WallToolPaths {
    pub paths: Vec<ExtrusionLine>,
    pub inner_contour: Vec<ExPolygon>,
}
struct Skeleton {
    graph: Graph,
    strategy: Strategy,
    filter_deviation: f64,
}
/// A cleaned outline's pristine skeletal graph (see [`WallToolPaths::prepare_outline`]); `None`
/// when nothing remains after cleanup.
pub(crate) struct PreparedOutline {
    graph: Option<Graph>,
}

impl WallToolPaths {
    /// Build walls from a spacing outline (the caller accounts for extrusion width vs. spacing).
    /// `outer` and `inner` are scaled spacings; `height` and `nozzle` are in millimetres.
    ///
    /// Panics on invalid settings or an unrecoverable Voronoi graph, rather than dropping walls.
    pub fn generate(
        outline: &[ExPolygon],
        outer: f64,
        inner: f64,
        insets: usize,
        height: f64,
        nozzle: f64,
        settings: &ArachneSettings,
    ) -> Self {
        Self::generate_inset(
            outline,
            [outer, inner, 0.],
            insets,
            height,
            nozzle,
            settings,
        )
    }

    fn generate_inset(
        outline: &[ExPolygon],
        widths: [f64; 3],
        insets: usize,
        height: f64,
        nozzle: f64,
        settings: &ArachneSettings,
    ) -> Self {
        Self::generate_prepared(None, outline, widths, insets, height, nozzle, settings)
    }

    /// The outline cleanup and skeletal graph of `generate_inset`: everything that does not depend
    /// on the inset count, so one outline can be beaded twice (only-one-wall-top falls back from a
    /// single wall to the full count on the same outline) without rebuilding its Voronoi graph.
    pub(crate) fn prepare_outline(
        outline: &[ExPolygon],
        outer: f64,
        settings: &ArachneSettings,
    ) -> PreparedOutline {
        settings.validate().expect("invalid Arachne setting");
        assert!(
            outer.is_finite() && outer > 0.,
            "Arachne requires positive finite spacings, layer height and nozzle diameter"
        );
        let epsilon = 12_499.;
        let prepared = offset_ex(
            &offset2_ex(outline, -epsilon, 2. * epsilon, Join::Miter),
            -epsilon,
            Join::Miter,
        );
        let mut polygons = to_polygons(&prepared);
        for p in &mut polygons {
            simplify::outline(p);
        }
        let prepared = prepare::clean(polygons, (outer / 2.).powi(2));
        let polygons = to_polygons(&prepared);
        if polygons.is_empty() {
            return PreparedOutline { graph: None };
        }
        let _graph_timer = crate::timing::Span::new(crate::timing::Stage::WallGraph);
        let graph = Graph::construct(&polygons, settings.wall_transition_angle.to_radians())
            .expect("Arachne: Voronoi trapezoid graph has unpaired edges");
        PreparedOutline { graph: Some(graph) }
    }

    /// `generate_inset` on an outline whose graph was prepared by [`Self::prepare_outline`] with
    /// the same `outer` spacing and settings; `None` prepares it here.
    pub(crate) fn generate_prepared(
        prepared: Option<&PreparedOutline>,
        outline: &[ExPolygon],
        widths: [f64; 3],
        insets: usize,
        height: f64,
        nozzle: f64,
        settings: &ArachneSettings,
    ) -> Self {
        let [outer, inner, outer_inset] = widths;
        if insets == 0 {
            return Self {
                paths: vec![],
                inner_contour: outline.to_vec(),
            };
        }
        settings.validate().expect("invalid Arachne setting");
        assert!(
            [outer, inner, height, nozzle]
                .iter()
                .all(|v| v.is_finite() && *v > 0.),
            "Arachne requires positive finite spacings, layer height and nozzle diameter"
        );
        let resolve = |v: FloatOrPercent| v.resolve(nozzle * SCALE, SCALE).trunc();
        let min_width = resolve(settings.min_bead_width);
        let width_correction = height * SCALE * (1. - std::f64::consts::PI / 4.);
        let strategy = Strategy {
            outer,
            inner,
            min_feature: resolve(settings.min_feature_size),
            min_width,
            split: (2. * min_width / (outer + width_correction) - 1.).clamp(0.01, 0.99),
            add: (min_width / (inner + width_correction)).clamp(0.01, 0.99),
            max_beads: 2 * insets,
            distribution: settings.wall_distribution_count,
            transition_length: resolve(settings.wall_transition_length),
            angle: settings.wall_transition_angle.to_radians(),
            outer_inset,
        };
        let owned;
        let prepared = match prepared {
            Some(prepared) => prepared,
            None => {
                owned = Self::prepare_outline(outline, outer, settings);
                &owned
            }
        };
        let Some(graph) = prepared.graph.clone() else {
            return Self::default();
        };
        let transition_timer = crate::timing::Span::new(crate::timing::Stage::WallTransitions);
        let mut skeleton = Skeleton {
            graph,
            strategy,
            filter_deviation: resolve(settings.wall_transition_filter_deviation),
        };
        skeleton.mark();
        skeleton.transitions();
        let paths = toolpaths::stitch(skeleton.segments(), inner - 1.);
        drop(transition_timer);
        let mut walls = vec![];
        let mut contour = vec![];
        for line in paths {
            if line.junctions.iter().all(|j| j.width == 0.) {
                if !line.is_odd && line.is_closed {
                    contour.push(line.polygon());
                }
            } else if line.junctions.len() >= 2 {
                walls.push(line);
            }
        }
        for line in &mut walls {
            simplify::line(line);
        }
        walls.sort_by_key(|line| line.inset);
        Self {
            paths: walls,
            inner_contour: union_ex_even_odd(&contour),
        }
    }
}

/// `process_arachne`: inset the outline for the rounded-rectangle cross section and return the
/// marker contour grown by the configured infill overlap. Gap fill is part of the walls themselves.
#[cfg(test)]
fn make_walls(
    island: &ExPolygon,
    p: &WallParams,
    s: &crate::settings::PrintSettings,
    lower: Option<&[ExPolygon]>,
    upper: Option<&[ExPolygon]>,
    layer: usize,
) -> IslandWalls {
    make_walls_with_painting(
        island,
        p,
        s,
        lower,
        upper,
        layer,
        &crate::fuzzy_skin::NoPainting,
    )
}

pub(crate) fn make_walls_with_painting(
    island: &ExPolygon,
    p: &WallParams,
    s: &crate::settings::PrintSettings,
    lower_slices: Option<&[ExPolygon]>,
    upper_slices: Option<&[ExPolygon]>,
    layer_index: usize,
    painting: &dyn crate::fuzzy_skin::FuzzySkinPainting,
) -> IslandWalls {
    let supported = lower_slices
        .filter(|_| s.overhangs && !s.supports_touching)
        .map(|lower| offset_ex(lower, p.nozzle_diameter * SCALE / 2., Join::Miter));
    make_walls_with_neighbors(
        island,
        p,
        s,
        WallNeighbors {
            lower: lower_slices,
            upper: upper_slices,
            supported: supported.as_deref(),
        },
        layer_index,
        painting,
    )
}

/// Shared, immutable geometry for every island in a layer's compatible wall group.
pub(crate) struct WallNeighbors<'a> {
    pub lower: Option<&'a [ExPolygon]>,
    pub upper: Option<&'a [ExPolygon]>,
    pub supported: Option<&'a [ExPolygon]>,
}

pub(crate) fn make_walls_with_neighbors(
    island: &ExPolygon,
    p: &WallParams,
    s: &crate::settings::PrintSettings,
    neighbors: WallNeighbors<'_>,
    layer_index: usize,
    painting: &dyn crate::fuzzy_skin::FuzzySkinPainting,
) -> IslandWalls {
    let WallNeighbors {
        lower: lower_slices,
        upper: upper_slices,
        supported,
    } = neighbors;
    let mut first_layer_settings = s.arachne;
    if layer_index == 0
        && let Some(width) = s.small_features.initial_layer_min_bead_width
    {
        first_layer_settings.min_bead_width = width;
    }
    let settings = &first_layer_settings;
    let external_first = s.external_perimeters_first;
    let outer = p.external.scaled_spacing() as f64;
    let inner = p.internal.scaled_spacing() as f64;
    let simplified = ExPolygon {
        contour: simplify::resolution(&island.contour, s.gcode_resolution * SCALE),
        holes: island
            .holes
            .iter()
            .map(|p| simplify::resolution(p, s.gcode_resolution * SCALE))
            .collect(),
    };
    let wall_inset = if s.precise_outer_wall {
        -((p.external.scaled_width() as f64 - outer) / 2.).trunc()
    } else {
        0.
    };
    // PrusaSlicer's Clipper 6.4 rounds offset vertices on exact halves upwards (`FRound`).
    let prusa = s.seam.prusa_parity;
    let offset_rounded = |ex: &[ExPolygon], delta: f64| {
        if prusa {
            crate::geometry::clip::offset_ex_half_up(ex, delta, Join::Miter)
        } else {
            offset_ex(ex, delta, Join::Miter)
        }
    };
    let outline = offset_rounded(
        std::slice::from_ref(&simplified),
        -(p.external.scaled_width() as f64 - outer) / 2. + wall_inset,
    );
    let generate = |outline: &[ExPolygon], outer, count, inset| {
        WallToolPaths::generate_inset(
            outline,
            [outer, inner, inset],
            count,
            p.external.height,
            p.nozzle_diameter,
            settings,
        )
    };
    let count = if p.perimeters > 0
        && ((s.top_one_perimeter == TopOnePerimeter::TopmostOnly && upper_slices.is_none())
            || (s.only_one_perimeter_first_layer && layer_index == 0))
    {
        1
    } else {
        p.perimeters
    };
    let top_surfaces = s.top_one_perimeter == TopOnePerimeter::TopSurfaces;
    let mut exposed_top = Vec::new();
    // The cleaned outline and its skeletal graph do not depend on the inset count: the single-wall
    // attempt below and its full-count fallback bead the same prepared graph.
    let first_count = if top_surfaces { count.min(1) } else { count };
    let prepared =
        (first_count > 0).then(|| WallToolPaths::prepare_outline(&outline, outer, settings));
    let generate_outline = |count| {
        WallToolPaths::generate_prepared(
            prepared.as_ref(),
            &outline,
            [outer, inner, wall_inset],
            count,
            p.external.height,
            p.nozzle_diameter,
            settings,
        )
    };
    let mut result = generate_outline(first_count);
    if top_surfaces
        && count > 1
        && let Some(upper) = upper_slices
    {
        let mut top = diff_ex(&result.inner_contour, upper);
        if top.is_empty() {
            result = generate_outline(count);
        } else {
            let width = p.internal.scaled_width() as f64;
            if let Some(lower) = lower_slices {
                let bridges = offset_ex(&diff_ex(&top, lower), outer.max(width), Join::Miter);
                top = diff_ex(&top, &bridges);
            }
            let min_width =
                (outer / 4. + 10.).max(s.top_surface_min_width.resolve(width, SCALE) / 4.);
            top = offset2_ex(
                &top,
                -min_width,
                min_width + width * s.top_surface_wall_extension,
                Join::Miter,
            );
            let not_top = diff_ex(&result.inner_contour, &top);
            top = intersection_ex(&top, &result.inner_contour);
            let not_top = offset_ex(&not_top, wall_inset, Join::Miter);
            let mut inside = if s.textured_mesh && not_top.len() > 1 {
                // Under a textured top the rest is thousands of small pieces; their walls do
                // not depend on each other, so they are made piece by piece in parallel.
                use rayon::prelude::*;
                let pieces: Vec<WallToolPaths> = not_top
                    .par_iter()
                    .map(|piece| generate(std::slice::from_ref(piece), inner, count - 1, 0.))
                    .collect();
                let mut all = WallToolPaths::default();
                for piece in pieces {
                    all.paths.extend(piece.paths);
                    all.inner_contour.extend(piece.inner_contour);
                }
                all.paths.sort_by_key(|line| line.inset);
                all
            } else {
                generate(&not_top, inner, count - 1, 0.)
            };
            if !result.paths.is_empty() {
                for line in &mut inside.paths {
                    line.inset += 1;
                    for junction in &mut line.junctions {
                        junction.perimeter += 1;
                    }
                }
            }
            result.paths.extend(inside.paths);
            exposed_top = top.clone();
            top.extend(inside.inner_contour);
            result.inner_contour = union_ex(&top);
        }
    }
    if s.textured_mesh {
        // A textured outline's inner contour follows the skeleton's arcs around every bump
        // (some 30 points per outline point); shrinking those tiny arcs folds them onto
        // themselves, which made the offsets below cost more than the walls. A hundredth of a
        // millimetre on the infill side of the walls is far below what the infill shows.
        result.inner_contour =
            crate::geometry::clip::simplify_ex(&result.inner_contour, INNER_CONTOUR_TOLERANCE);
    }
    let depth_count = result.paths.iter().map(|l| l.inset + 1).max().unwrap_or(0);
    let spacing = if depth_count == 1 {
        (outer + inner) / 2.
    } else {
        inner
    };
    let vanishes = if s.textured_mesh {
        // Shrinking keeps separate pieces apart: the whole vanishes when every piece does, and
        // the largest pieces are the ones that stay.
        let mut pieces: Vec<&ExPolygon> = result.inner_contour.iter().collect();
        pieces.sort_by(|a, b| b.area_mm2().total_cmp(&a.area_mm2()));
        pieces
            .iter()
            .all(|p| offset_ex(std::slice::from_ref(*p), -spacing / 2., Join::Miter).is_empty())
    } else {
        offset_ex(&result.inner_contour, -spacing / 2., Join::Miter).is_empty()
    };
    if vanishes {
        result.inner_contour.clear();
    }
    let overlap_base = if s.top_bottom_infill_overlap.is_some() {
        if count == 0 {
            0.
        } else if top_surfaces || count == 1 {
            outer
        } else {
            inner
        }
    } else {
        match depth_count {
            0 => 0.,
            1 => outer,
            _ => inner,
        }
    };
    let overlap = if layer_index == 0 || upper_slices.is_none() {
        s.top_bottom_infill_overlap.unwrap_or(p.infill_overlap)
    } else {
        p.infill_overlap
    }
    .resolve(overlap_base, SCALE)
    .trunc();
    let half_min_spacing = (p.solid_infill.scaled_spacing() as f64 * 0.6).trunc() / 2.;
    for contour in &mut result.inner_contour {
        contour.contour = simplify::resolution(&contour.contour, s.gcode_resolution * SCALE);
        for hole in &mut contour.holes {
            *hole = simplify::resolution(hole, s.gcode_resolution * SCALE);
        }
    }
    let mut infill_area = offset_rounded(
        &offset_rounded(&result.inner_contour, -half_min_spacing),
        overlap + half_min_spacing,
    );
    if let Some(top_overlap) = s.top_bottom_infill_overlap {
        infill_area.extend(offset_ex(
            &exposed_top,
            top_overlap.resolve(overlap_base, SCALE).trunc(),
            Join::Miter,
        ));
        infill_area = union_ex(&infill_area);
    }
    // Ordering is implemented separately so closed loops remain intact across width splits.
    order::order(
        &mut result.paths,
        external_first,
        s.orca_walls.inner_outer_inner && layer_index > 0,
        !s.adhesion.orca,
    );
    let mut variable_walls = vec![];
    let clip_area = supported.map(if s.textured_mesh {
        clip::ClipArea::cropped
    } else {
        clip::ClipArea::new
    });
    for mut line in result.paths {
        if line.is_closed {
            // Arachne produces contours clockwise and holes counter-clockwise; like PerimeterGenerator's
            // traverse_extrusions, restore printing orientation (contours CCW, holes CW). The seam is
            // chosen later by the seam plan (print.rs, seam_arachne_run), which also applies the seam gap.
            line.junctions.reverse();
        }
        crate::fuzzy_skin::fuzzy_extrusion_line(&mut line, &s.fuzzy_skin, layer_index, painting);
        if let Some(t) = painting.texture_skin() {
            t.displace_line(&mut line, layer_index, &s.texture_skin, &p.external);
        }
        let flow = if line.inset == 0 {
            p.external
        } else {
            p.internal
        };
        let role = if line.inset == 0 {
            Role::ExternalPerimeter
        } else {
            Role::Perimeter
        };
        let mut pieces = if let Some(supported) = &clip_area {
            let normal = clip::clip_extrusion_with(&line, supported, true)
                .into_iter()
                .map(|l| (l, false));
            let overhang = clip::clip_extrusion_with(&line, supported, false)
                .into_iter()
                .map(|l| (l, true));
            normal.chain(overhang).collect::<Vec<_>>()
        } else {
            vec![(line.clone(), false)]
        };
        let mut run = vec![];
        let mut pos = line.junctions[0].p;
        while !pieces.is_empty() {
            let (idx, reverse) = pieces
                .iter()
                .enumerate()
                .flat_map(|(i, (l, _))| {
                    [
                        (i, false, l.junctions[0].p.distance_sq(pos)),
                        (i, true, l.junctions.last().unwrap().p.distance_sq(pos)),
                    ]
                })
                .min_by_key(|v| v.2)
                .map(|(i, r, _)| (i, r))
                .unwrap();
            let (mut piece, overhang) = pieces.remove(idx);
            if reverse {
                piece.junctions.reverse();
            }
            pos = piece.junctions.last().unwrap().p;
            if overhang && s.thick_bridges {
                // A circular bridge cross-section is the width==height special case of Flow.
                let diameter = s.nozzle_diameter * s.bridge_flow_ratio.sqrt();
                run.push(crate::toolpath::ExtrusionPath {
                    role: Role::OverhangPerimeter,
                    points: piece.polygon(),
                    flow: crate::flow::Flow::new(diameter, diameter),
                    overhang: None,
                    perimeter_index: None,
                    seam: None,
                    brick: None,
                });
            } else {
                let thick = ThickPolyline {
                    points: piece.polygon(),
                    width: piece
                        .junctions
                        .windows(2)
                        .flat_map(|w| [w[0].width, w[1].width])
                        .collect(),
                    endpoints: (!line.is_closed, !line.is_closed),
                };
                let (role, flow) = if overhang {
                    let mut flow = p.internal;
                    let area = flow.mm3_per_mm() * s.bridge_flow_ratio;
                    if s.bridge_flow_ratio > 1.0 {
                        flow.height = area / flow.spacing();
                    } else if s.bridge_flow_ratio < 1.0
                        && flow.width - (flow.mm3_per_mm() - area) / flow.height <= flow.height
                    {
                        flow.height = 2.0 * (area / std::f64::consts::PI).sqrt();
                    }
                    (Role::OverhangPerimeter, flow)
                } else {
                    (role, flow)
                };
                run.extend(thick_polyline_to_multi_path(
                    &thick, role, flow, 50_000., 100.,
                ));
            }
        }
        for path in &mut run {
            path.perimeter_index = Some(line.inset);
        }
        if !run.is_empty() {
            variable_walls.push(run);
        }
    }
    let mut infill_no_overlap = offset2_ex(
        &result.inner_contour,
        -half_min_spacing,
        half_min_spacing,
        Join::Miter,
    );
    infill_no_overlap.extend(exposed_top);
    IslandWalls {
        infill_no_overlap: union_ex(&infill_no_overlap),
        variable_walls,
        infill_area,
        ..IslandWalls::default()
    }
}

#[cfg(test)]
mod upstream_tests;
