// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: orca@8500fcdcca src/libslic3r/PerimeterGenerator.cpp :: PerimeterGenerator::process_classic
// Source: prusaslicer@b028299c77 src/libslic3r/PerimeterGenerator.cpp :: PerimeterGenerator::process_classic, PerimeterGenerator::thick_polyline_to_multi_path, variable_width_classic
// Source: prusaslicer@b028299c77 src/libslic3r/ExtrusionEntity.cpp :: ExtrusionPath::polygons_covered_by_width
// Source: prusaslicer@b028299c77 src/libslic3r/ExPolygon.cpp :: ExPolygon::simplify_p
// Changes: Offsets, wall spacing (ext_perimeter_spacing2), thin walls (offset2 + medial axis), gap collection
// Changes: between loops, gap fill (opening / offset2 width filter, medial axis, variable width split by
// Changes: 0.05 mm width steps) and the infill boundary follow process_classic; thick_polyline_to_multi_path is
// Changes: a direct port. Not yet ported: extra perimeters on overhangs, Orca narrow-loop smaller width,
// Changes: closed gap fill
// Changes: is printed as an open path (no loop seam handling).
// Changes: Classic loop nesting/traversal is applied by perimeter_order.rs before seam placement.
// Source: orca@8500fcdcca src/libslic3r/PerimeterGenerator.cpp :: PerimeterGenerator::split_top_surfaces, traverse_loops
// Changes: Classic single wall on top surfaces (Orca and Prusa variants), Orca offset2 inner walls,
// Changes: precise_outer_wall spacing, fill_no_overlap, and overhang splitting of closed classic loops.

//! Wall loops ("perimeters"), thin walls and gap fill for one island.

use crate::flow::Flow;
use crate::geometry::clip::{
    CLIPPER_SAFETY_OFFSET, Join, diff_ex, offset_ex, offset_polylines, offset2_ex, opening_ex,
    union_ex,
};
use crate::geometry::medial_axis::{SCALED_EPSILON, ThickPolyline, medial_axis};
use crate::geometry::{ExPolygon, Point, Polygon, SCALE, scale, signed_area};
use crate::settings::FloatOrPercent;
use crate::toolpath::{ExtrusionPath, Role};

/// PrusaSlicer's `INSET_OVERLAP_TOLERANCE`: how much narrower than a full spacing a collapsing part may be
/// before it is left to the medial axis instead of being squeezed into a loop.
const INSET_OVERLAP_TOLERANCE: f64 = 0.4;

/// Granularity of variable-width extrusions: a new path starts whenever the width changes by more than
/// this (mm). G-code can't vary the width within a move.
const VARIABLE_WIDTH_TOLERANCE_MM: f64 = 0.05;

/// One closed wall loop.
#[derive(Clone, Debug, PartialEq)]
pub struct WallLoop {
    /// Counter-clockwise for contours, clockwise for holes.
    pub polygon: Polygon,
    /// 0 is the outer wall.
    pub depth: usize,
    pub is_contour: bool,
}

#[derive(Clone, Debug, Default)]
pub struct IslandWalls {
    pub loops: Vec<WallLoop>,
    /// Arachne walls, ordered as continuous runs before splitting by width.
    pub variable_walls: Vec<Vec<ExtrusionPath>>,
    /// Thin walls: features too narrow for a wall loop, extruded along their centreline as external
    /// perimeter. Each entry is one continuous run of paths whose widths vary along it.
    pub thin_walls: Vec<Vec<ExtrusionPath>>,
    /// Gap fill between the wall loops, as runs of variable-width paths (role [`Role::GapFill`]).
    pub gap_fill: Vec<Vec<ExtrusionPath>>,
    /// Area left for infill inside the innermost wall (including the overlap into it), without the gap fill.
    pub infill_area: Vec<ExPolygon>,
    /// Arachne infill boundary before wall overlap, used by Orca narrow solid fill.
    pub infill_no_overlap: Vec<ExPolygon>,
}

pub struct WallParams {
    pub perimeters: usize,
    pub external: Flow,
    pub internal: Flow,
    pub solid_infill: Flow,
    /// Percentages are of (inset + half the solid infill spacing).
    pub infill_overlap: FloatOrPercent,
    pub nozzle_diameter: f64,
    /// PrusaSlicer's `thin_walls`.
    pub thin_walls: bool,
    /// `gap_fill_enabled` and a non-zero `gap_fill_speed`.
    pub gap_fill: bool,
    /// Minimum continuous gap-fill length, mm; zero retains every gap.
    pub filter_out_gap_fill: f64,
    /// Sparse infill density 0–1; with none, no gaps are looked for inside the innermost loop.
    pub fill_density: f64,
}

/// Select the configured wall generator. The original `make_walls` API remains explicitly classic.
pub fn make_walls_selected(
    island: &ExPolygon,
    p: &WallParams,
    s: &crate::settings::PrintSettings,
    lower_slices: Option<&[ExPolygon]>,
    upper_slices: Option<&[ExPolygon]>,
    layer_index: usize,
) -> IslandWalls {
    make_walls_selected_with_painting(
        island,
        p,
        s,
        lower_slices,
        upper_slices,
        layer_index,
        &crate::fuzzy_skin::NoPainting,
    )
}

pub(crate) fn make_walls_selected_with_painting(
    island: &ExPolygon,
    p: &WallParams,
    s: &crate::settings::PrintSettings,
    lower_slices: Option<&[ExPolygon]>,
    upper_slices: Option<&[ExPolygon]>,
    layer_index: usize,
    painting: &dyn crate::fuzzy_skin::FuzzySkinPainting,
) -> IslandWalls {
    match s.perimeter_generator {
        crate::settings::PerimeterGenerator::Classic => {
            // process_classic simplifies every input surface before offsetting. Retaining
            // mesh-triangle intersections creates tiny segments and prevents long arc fits.
            let simplified = simplify_classic(island, s.gcode_resolution * SCALE);
            let mut walls = IslandWalls::default();
            let top = ClassicTop::new(s, p, lower_slices, upper_slices, layer_index);
            for input in &simplified {
                let part = make_walls_classic(input, p, Some(&top));
                walls.loops.extend(part.loops);
                walls.thin_walls.extend(part.thin_walls);
                walls.gap_fill.extend(part.gap_fill);
                walls.infill_area.extend(part.infill_area);
                walls.infill_no_overlap.extend(part.infill_no_overlap);
            }
            for wall in &mut walls.loops {
                crate::fuzzy_skin::fuzzy_polygon(
                    &mut wall.polygon,
                    &s.fuzzy_skin,
                    layer_index,
                    wall.depth,
                    wall.is_contour,
                    painting,
                );
                if let Some(t) = painting.texture_skin() {
                    t.displace_polygon(
                        &mut wall.polygon,
                        layer_index,
                        wall.depth,
                        wall.is_contour,
                        &s.texture_skin,
                        &p.external,
                    );
                }
            }
            walls
        }
        crate::settings::PerimeterGenerator::Arachne => crate::arachne::make_walls_with_painting(
            island,
            p,
            s,
            lower_slices,
            upper_slices,
            layer_index,
            painting,
        ),
    }
}

fn simplify_classic(island: &ExPolygon, tolerance: f64) -> Vec<ExPolygon> {
    let ring = |points: &Polygon| {
        let Some(&first) = points.first() else {
            return Vec::new();
        };
        let mut closed = points.clone();
        closed.push(first);
        let mut result = crate::geometry::simplify::douglas_peucker(&closed, tolerance);
        result.pop();
        result
    };
    let contour = ring(&island.contour);
    if contour.len() < 3 {
        return Vec::new();
    }
    union_ex(&[ExPolygon {
        contour,
        holes: island
            .holes
            .iter()
            .map(ring)
            .filter(|p| p.len() >= 3)
            .collect(),
    }])
}

fn oriented(mut poly: Polygon, ccw: bool) -> Polygon {
    if (signed_area(&poly) > 0.0) != ccw {
        poly.reverse();
    }
    poly
}

/// Single-wall top handling and top/bottom infill overlap of the classic generator, resolved from
/// the print settings. Orca's variant (`split_top_surfaces`) is selected by its source option
/// `top_bottom_infill_wall_overlap`; Prusa's otherwise.
pub(crate) struct ClassicTop<'a> {
    lower: Option<&'a [ExPolygon]>,
    upper: Option<&'a [ExPolygon]>,
    /// Topmost-layer / first-layer reduction to one wall applies.
    one_wall: bool,
    /// Split exposed top surfaces from the inner walls after the first loop.
    split_top: bool,
    orca: bool,
    /// Orca `min_width_top_surface`, scaled.
    min_width_top: f64,
    /// Solid / sparse infill extrusion width, scaled (half of it pulls the clip inwards).
    infill_width: f64,
    nozzle_diameter: f64,
    /// Orca `top_bottom_infill_wall_overlap` on the first and topmost layers and for top fills.
    top_overlap: Option<FloatOrPercent>,
    first_or_topmost: bool,
    /// Orca `precise_outer_wall` (inner/outer sequence): the outer-to-inner distance uses widths.
    precise_outer_wall: bool,
}

impl<'a> ClassicTop<'a> {
    pub(crate) fn new(
        s: &crate::settings::PrintSettings,
        p: &WallParams,
        lower: Option<&'a [ExPolygon]>,
        upper: Option<&'a [ExPolygon]>,
        layer_index: usize,
    ) -> Self {
        use crate::settings::TopOnePerimeter;
        let orca = s.top_bottom_infill_overlap.is_some();
        let width = p.internal.scaled_width() as f64;
        Self {
            lower,
            upper,
            one_wall: (s.top_one_perimeter != TopOnePerimeter::None && upper.is_none())
                || (s.only_one_perimeter_first_layer && layer_index == 0),
            split_top: s.top_one_perimeter == TopOnePerimeter::TopSurfaces && upper.is_some(),
            orca,
            min_width_top: s.top_surface_min_width.resolve(width / SCALE, SCALE),
            infill_width: if orca {
                s.infill_extrusion_width
            } else {
                s.solid_infill_extrusion_width
            } * SCALE,
            nozzle_diameter: p.nozzle_diameter,
            top_overlap: s.top_bottom_infill_overlap,
            first_or_topmost: layer_index == 0 || upper.is_none(),
            precise_outer_wall: s.precise_outer_wall,
        }
    }

    /// Split `last` (the area inside the outer wall) into exposed top fill and the area that still
    /// receives inner walls. Returns `(top_fills, fill_clip, not_top)`; `None` keeps all walls.
    /// Orca `PerimeterGenerator::split_top_surfaces` / Prusa `process_classic`'s TopSurfaces block.
    fn split(
        &self,
        last: &[ExPolygon],
        p: &WallParams,
    ) -> Option<(Vec<ExPolygon>, Vec<ExPolygon>, Vec<ExPolygon>)> {
        let upper = self.upper?;
        let perimeter_width = p.internal.scaled_width() as f64;
        let perimeter_spacing = p.internal.scaled_spacing() as f64;
        let ext_width = p.external.scaled_width() as f64;
        let ext_spacing = p.external.scaled_spacing() as f64;
        let loops = p.perimeters as f64;
        let threshold = if p.perimeters <= 1 {
            0.0
        } else {
            0.9 * perimeter_spacing * (loops - 1.0)
        };
        let mut offset_top = if p.perimeters == 0 {
            0.0
        } else if self.orca {
            0.9 * (ext_width + perimeter_spacing * (loops - 1.0))
        } else {
            1.5 * (ext_width + perimeter_spacing * (loops - 1.0))
        };
        offset_top = if offset_top > threshold {
            offset_top - threshold
        } else {
            0.0
        };
        let min_width = if self.orca {
            (ext_spacing / 2.0 + 10.0).max(self.min_width_top)
        } else {
            (ext_spacing / 2.0 + 0.00001 * SCALE).max(perimeter_width)
        };
        let without_bridges = match self.lower {
            Some(lower) => {
                let grow = if self.orca {
                    // SoftFever: bridge margin of BRIDGE_INFILL_MARGIN (1 mm at a 0.4 mm nozzle).
                    let margin = SCALE * 1f64.min(self.nozzle_diameter / 0.4);
                    1.5 * ext_spacing.max(perimeter_width) + margin + perimeter_spacing / 2.0
                } else {
                    1.5 * ext_spacing.max(perimeter_width)
                };
                let bridges = offset_ex(&diff_ex(last, lower), grow, Join::Miter);
                diff_ex(last, &bridges)
            }
            None => last.to_vec(),
        };
        let top = diff_ex(&without_bridges, &offset_ex(upper, min_width, Join::Miter));
        if top.is_empty() && !self.orca {
            return None;
        }
        let fill_clip = offset_ex(last, -ext_spacing, Join::Miter);
        let not_top = diff_ex(
            last,
            &offset_ex(
                &top,
                offset_top + min_width - ext_spacing / 2.0,
                Join::Miter,
            ),
        );
        let gap = diff_ex(&top, &fill_clip);
        let top_fills = diff_ex(&fill_clip, &not_top);
        let fill_clip = offset_ex(
            last,
            ext_spacing / 2.0 - self.infill_width / 2.0,
            Join::Miter,
        );
        let mut rest = crate::geometry::clip::intersection_ex(&not_top, last);
        if p.gap_fill {
            rest = union_ex(&[rest, gap].concat());
        }
        Some((top_fills, fill_clip, rest))
    }
}

/// Generates wall loops, thin walls and gap fill for `island`, and the area left for infill.
/// PrusaSlicer's `PerimeterGenerator::process_classic`.
pub fn make_walls(island: &ExPolygon, p: &WallParams) -> IslandWalls {
    make_walls_classic(island, p, None)
}

fn make_walls_classic(
    island: &ExPolygon,
    p: &WallParams,
    top: Option<&ClassicTop<'_>>,
) -> IslandWalls {
    let perimeter_width = p.internal.scaled_width() as f64;
    let perimeter_spacing = p.internal.scaled_spacing() as f64;
    let ext_width = p.external.scaled_width() as f64;
    let ext_spacing = p.external.scaled_spacing() as f64;
    // Distance between the outer wall and the first inner wall: average of the two spacings.
    let ext_spacing2 = if top.is_some_and(|t| t.precise_outer_wall) {
        (0.5 * (p.external.width + p.internal.width) * SCALE).round()
    } else {
        (0.5 * (p.external.spacing() + p.internal.spacing()) * SCALE).round()
    };
    let solid_spacing = p.solid_infill.scaled_spacing() as f64;
    // Minimum spacing of two adjacent traces before a collapsing part is left to the medial axis.
    let min_spacing = (perimeter_spacing * (1.0 - INSET_OVERLAP_TOLERANCE)).trunc();
    let ext_min_spacing = (ext_spacing * (1.0 - INSET_OVERLAP_TOLERANCE)).trunc();

    let mut out = IslandWalls::default();
    let mut last = vec![island.clone()];
    let mut gaps: Vec<ExPolygon> = Vec::new();
    let mut thin_walls: Vec<ThickPolyline> = Vec::new();
    // 0-based index of the innermost loop; -1 for none.
    let mut loop_number = p.perimeters as isize - 1;
    if loop_number > 0 && top.is_some_and(|t| t.one_wall) {
        loop_number = 0;
    }
    let mut top_fills: Vec<ExPolygon> = Vec::new();
    let mut fill_clip: Vec<ExPolygon> = Vec::new();

    if loop_number >= 0 {
        // One iteration more than there are loops, to find the gaps inside the last one.
        let mut i: isize = 0;
        loop {
            let offsets = if i == 0 {
                let offsets = if p.thin_walls {
                    offset2_ex(
                        &last,
                        -(ext_width / 2.0 + ext_min_spacing / 2.0 - 1.0),
                        ext_min_spacing / 2.0 - 1.0,
                        Join::Miter,
                    )
                } else {
                    offset_ex(&last, -ext_width / 2.0, Join::Miter)
                };
                if p.thin_walls {
                    // The opening keeps (almost) nothing narrower than min_width.
                    let min_width = scale(p.nozzle_diameter / 3.0) as f64;
                    let covered = offset_ex(
                        &offsets,
                        ext_width / 2.0 + CLIPPER_SAFETY_OFFSET,
                        Join::Miter,
                    );
                    let narrow = opening_ex(&diff_ex(&last, &covered), min_width / 2.0);
                    // The widest thin wall is the narrowest part a single loop fits in.
                    for ex in &narrow {
                        thin_walls.extend(medial_axis(ex, min_width, ext_width + ext_spacing2));
                    }
                }
                offsets
            } else {
                let distance = if i == 1 {
                    ext_spacing2
                } else {
                    perimeter_spacing
                };
                // Orca (BBS) always collapses inner walls with offset2, so narrow inner regions
                // become gap fill even when thin-wall detection is off.
                let offsets = if p.thin_walls || top.is_some_and(|t| t.orca) {
                    offset2_ex(
                        &last,
                        -(distance + min_spacing / 2.0 - 1.0),
                        min_spacing / 2.0 - 1.0,
                        Join::Miter,
                    )
                } else {
                    offset_ex(&last, -distance, Join::Miter)
                };
                if p.gap_fill {
                    // The safety offset keeps very narrow gaps, which gap fill can't fill, out of the
                    // area subtracted from infill.
                    gaps.extend(diff_ex(
                        &offset_ex(&last, -0.5 * distance, Join::Miter),
                        &offset_ex(
                            &offsets,
                            0.5 * distance + CLIPPER_SAFETY_OFFSET,
                            Join::Miter,
                        ),
                    ));
                }
                offsets
            };
            if offsets.is_empty() {
                loop_number = i - 1;
                last.clear();
                break;
            } else if i > loop_number {
                // This round only looked for gaps.
                break;
            }
            let depth = i as usize;
            for ex in &offsets {
                out.loops.push(WallLoop {
                    polygon: oriented(ex.contour.clone(), true),
                    depth,
                    is_contour: true,
                });
                for hole in &ex.holes {
                    out.loops.push(WallLoop {
                        polygon: oriented(hole.clone(), false),
                        depth,
                        is_contour: false,
                    });
                }
            }
            last = offsets;
            if i == 0
                && i != loop_number
                && let Some(t) = top.filter(|t| t.split_top)
                && let Some((fills, clip, rest)) = t.split(&last, p)
            {
                top_fills.extend(fills);
                fill_clip = clip;
                last = rest;
            }
            if i == loop_number && (!p.gap_fill || p.fill_density == 0.0) {
                break;
            }
            i += 1;
        }
        out.thin_walls = variable_width_classic(&thin_walls, Role::ExternalPerimeter, p.external);
    }

    // Gap fill.
    if !gaps.is_empty() {
        let min = 0.2 * perimeter_width * (1.0 - INSET_OVERLAP_TOLERANCE);
        let max = 2.0 * perimeter_spacing;
        let gaps_ex = diff_ex(
            &opening_ex(&gaps, min / 2.0),
            &offset2_ex(
                &gaps,
                -max / 2.0,
                max / 2.0 + CLIPPER_SAFETY_OFFSET,
                Join::Miter,
            ),
        );
        let polylines: Vec<ThickPolyline> = gaps_ex
            .iter()
            .flat_map(|ex| medial_axis(ex, min, max))
            .filter(|line| line.length() >= p.filter_out_gap_fill * SCALE)
            .collect();
        if !polylines.is_empty() {
            let gap_fill = variable_width_classic(&polylines, Role::GapFill, p.solid_infill);
            // Don't infill what is gap-filled. Growing the actual extrusions keeps gaps the medial axis
            // skipped (too short) in the infill area.
            last = diff_ex(&last, &polygons_covered_by_width(&gap_fill, 10.0));
            out.gap_fill = gap_fill;
        }
    }

    // Infill boundary: half a spacing in from the innermost wall's centreline, minus the overlap,
    // then an opening that drops slivers too narrow for an infill line.
    let inset = match loop_number {
        n if n < 0 => 0.0,
        0 => (ext_spacing / 2.0).trunc(),
        _ => (perimeter_spacing / 2.0).trunc(),
    };
    let base = inset + solid_spacing / 2.0;
    let top_overlap = top.and_then(|t| t.top_overlap);
    let overlap = if inset > 0.0 {
        match top_overlap {
            Some(o) if top.is_some_and(|t| t.first_or_topmost) => o,
            _ => p.infill_overlap,
        }
        .resolve(base, SCALE)
        .trunc()
    } else {
        0.0
    };
    let min_infill_spacing = (solid_spacing * (1.0 - INSET_OVERLAP_TOLERANCE)).trunc();
    let not_filled = union_ex(&last);
    out.infill_area = offset2_ex(
        &not_filled,
        -(inset - overlap) - min_infill_spacing / 2.0,
        min_infill_spacing / 2.0,
        Join::Miter,
    );
    // Orca's fill_no_overlap: the infill boundary without the wall overlap (narrow solid fill).
    out.infill_no_overlap = if min_infill_spacing / 2.0 > overlap {
        offset2_ex(
            &not_filled,
            -(inset - overlap) - min_infill_spacing / 2.0,
            min_infill_spacing / 2.0 - overlap,
            Join::Miter,
        )
    } else {
        offset_ex(&not_filled, -inset, Join::Miter)
    };
    if !top_fills.is_empty() {
        // Grow the top fills to the virtual second wall, then by the top/bottom overlap.
        let exposed = crate::geometry::clip::intersection_ex(
            &fill_clip,
            &offset_ex(
                &top_fills,
                p.external.scaled_spacing() as f64 / 2.0,
                Join::Miter,
            ),
        );
        // Prusa grows by the ordinary overlap; Orca by top_bottom_infill_wall_overlap except on
        // the first and topmost layers, whose ordinary overlap already is that value.
        let grow = match top_overlap {
            None => overlap,
            Some(o) if inset > 0.0 && !top.is_some_and(|t| t.first_or_topmost) => {
                o.resolve(base, SCALE).trunc()
            }
            Some(_) => 0.0,
        };
        out.infill_area =
            union_ex(&[out.infill_area, offset_ex(&exposed, grow, Join::Miter)].concat());
        out.infill_no_overlap =
            union_ex(&[std::mem::take(&mut out.infill_no_overlap), exposed].concat());
    }
    out
}

/// Flow of an overhanging wall section: a circular thick bridge, or the inner-wall flow with the
/// bridge flow ratio applied (as the Arachne overhang split).
pub(crate) fn overhang_flow(s: &crate::settings::PrintSettings, internal: Flow) -> Flow {
    if s.thick_bridges {
        let diameter = s.nozzle_diameter * s.bridge_flow_ratio.sqrt();
        return Flow::new(diameter, diameter);
    }
    let mut flow = internal;
    let area = flow.mm3_per_mm() * s.bridge_flow_ratio;
    if s.bridge_flow_ratio > 1.0 {
        flow.height = area / flow.spacing();
    } else if s.bridge_flow_ratio < 1.0
        && flow.width - (flow.mm3_per_mm() - area) / flow.height <= flow.height
    {
        flow.height = 2.0 * (area / std::f64::consts::PI).sqrt();
    }
    flow
}

fn inside_area(area: &[ExPolygon], p: (f64, f64)) -> bool {
    // Even-odd over all rings: contours and holes of non-overlapping ExPolygons.
    let mut inside = false;
    for ring in area.iter().flat_map(ExPolygon::polygons) {
        let n = ring.len();
        for k in 0..n {
            let (a, b) = (ring[k], ring[(k + 1) % n]);
            let (ax, ay, bx, by) = (a.x as f64, a.y as f64, b.x as f64, b.y as f64);
            if (ay > p.1) != (by > p.1) && p.0 < ax + (p.1 - ay) / (by - ay) * (bx - ax) {
                inside = !inside;
            }
        }
    }
    inside
}

/// Split a closed classic wall loop at the boundary of `supported` into an ordered, closed run of
/// paths: supported sections keep `role`/`flow`, the rest become [`Role::OverhangPerimeter`] with
/// `overhang`. Returns `None` when the whole loop is supported (or has no usable geometry).
/// Classic `traverse_loops`: intersection_pl / diff_pl of the loop with the grown lower slices.
pub(crate) fn split_classic_overhangs(
    loop_polygon: &[Point],
    supported: &[ExPolygon],
    role: Role,
    flow: Flow,
    overhang: Flow,
) -> Option<Vec<ExtrusionPath>> {
    let n = loop_polygon.len();
    if n < 3 {
        return None;
    }
    // Rings near the loop only.
    let bb = crate::geometry::BoundingBox::from_points(loop_polygon.iter())?;
    let near: Vec<ExPolygon> = supported
        .iter()
        .filter(|e| {
            crate::geometry::BoundingBox::from_points(e.contour.iter()).is_some_and(|b| {
                b.min.x <= bb.max.x
                    && b.max.x >= bb.min.x
                    && b.min.y <= bb.max.y
                    && b.max.y >= bb.min.y
            })
        })
        .cloned()
        .collect();
    let edges: Vec<(Point, Point)> = near
        .iter()
        .flat_map(ExPolygon::polygons)
        .flat_map(|r| (0..r.len()).map(move |k| (r[k], r[(k + 1) % r.len()])))
        .collect();
    // Points along the closed loop with their classification of the following sub-segment.
    let mut pieces: Vec<(Point, bool)> = Vec::new();
    for k in 0..n {
        let (a, b) = (loop_polygon[k], loop_polygon[(k + 1) % n]);
        let (ax, ay) = (a.x as f64, a.y as f64);
        let (dx, dy) = (b.x as f64 - ax, b.y as f64 - ay);
        let mut ts = vec![0.0];
        for &(c, d) in &edges {
            let (cx, cy) = (c.x as f64, c.y as f64);
            let (ex, ey) = (d.x as f64 - cx, d.y as f64 - cy);
            let den = dx * ey - dy * ex;
            if den.abs() < 1e-9 {
                continue;
            }
            let t = ((cx - ax) * ey - (cy - ay) * ex) / den;
            let u = ((cx - ax) * dy - (cy - ay) * dx) / den;
            if t > 1e-9 && t < 1.0 - 1e-9 && (-1e-9..=1.0 + 1e-9).contains(&u) {
                ts.push(t);
            }
        }
        ts.sort_by(f64::total_cmp);
        ts.push(1.0);
        for w in ts.windows(2) {
            if w[1] - w[0] < 1e-9 {
                continue;
            }
            let m = 0.5 * (w[0] + w[1]);
            let start = Point::new(
                (ax + w[0] * dx).round() as i64,
                (ay + w[0] * dy).round() as i64,
            );
            pieces.push((start, inside_area(&near, (ax + m * dx, ay + m * dy))));
        }
    }
    if pieces.iter().all(|p| p.1) {
        return None;
    }
    // Start the run at a transition so the first and last paths meet at the loop's closing point.
    let first = (0..pieces.len())
        .find(|&i| pieces[i].1 != pieces[(i + pieces.len() - 1) % pieces.len()].1)
        .unwrap_or(0);
    let mut run: Vec<ExtrusionPath> = Vec::new();
    for j in 0..pieces.len() {
        let (pt, ok) = pieces[(first + j) % pieces.len()];
        let next = pieces[(first + j + 1) % pieces.len()].0;
        let (r, f) = if ok {
            (role, flow)
        } else {
            (Role::OverhangPerimeter, overhang)
        };
        match run.last_mut() {
            Some(last) if last.role == r && last.points.last() == Some(&pt) => {
                last.points.push(next)
            }
            _ => run.push(ExtrusionPath {
                role: r,
                points: vec![pt, next],
                flow: f,
                overhang: None,
                perimeter_index: None,
                seam: None,
                brick: None,
            }),
        }
    }
    run.retain(|p| p.points.len() >= 2);
    Some(run)
}

/// The area covered by the extrusions' full width, grown by `epsilon` (scaled).
fn polygons_covered_by_width(runs: &[Vec<ExtrusionPath>], epsilon: f64) -> Vec<ExPolygon> {
    let covered: Vec<ExPolygon> = runs
        .iter()
        .flatten()
        .flat_map(|path| {
            offset_polylines(
                std::slice::from_ref(&path.points),
                scale(path.flow.width / 2.0) as f64 + epsilon,
            )
        })
        .collect();
    union_ex(&covered)
}

/// Turns thick polylines into runs of constant-width paths (PrusaSlicer's `variable_width_classic`).
fn variable_width_classic(
    polylines: &[ThickPolyline],
    role: Role,
    flow: Flow,
) -> Vec<Vec<ExtrusionPath>> {
    // Only affects the number of segments; pruning happens before this.
    let tolerance = scale(VARIABLE_WIDTH_TOLERANCE_MM) as f64;
    polylines
        .iter()
        .map(|p| thick_polyline_to_multi_path(p, role, flow, tolerance, tolerance))
        .filter(|run| !run.is_empty())
        .collect()
}

/// `flow` with the extrusion width that gives a line spacing of `spacing_mm` (rounded-rectangle model).
fn flow_for_spacing(flow: Flow, spacing_mm: f64) -> Flow {
    Flow {
        width: spacing_mm + flow.height * (1.0 - 0.25 * std::f64::consts::PI),
        height: flow.height,
    }
}

/// Splits a thick polyline into paths of constant width. Segments whose ends differ by more than
/// `tolerance` (scaled) are subdivided; consecutive segments within `merge_tolerance` of the current
/// path's width are merged into it. Widths in the polyline are spacings; each path's flow width is the
/// matching extrusion width. PrusaSlicer's `PerimeterGenerator::thick_polyline_to_multi_path`.
pub fn thick_polyline_to_multi_path(
    thick_polyline: &ThickPolyline,
    role: Role,
    flow: Flow,
    tolerance: f64,
    merge_tolerance: f64,
) -> Vec<ExtrusionPath> {
    let mut paths: Vec<ExtrusionPath> = Vec::new();
    let mut path: Option<ExtrusionPath> = None;
    let mut lines = thick_polyline.thicklines();

    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let line_len = line.a.distance_to(line.b);
        if line_len < SCALED_EPSILON {
            // Too short to care about its width: connect it to a neighbour, or drop it.
            if let Some(path) = path.as_mut() {
                *path.points.last_mut().unwrap() = line.b;
            } else if i + 1 < lines.len() {
                lines[i + 1].a = line.a;
            } else if let Some(prev) = paths.last_mut() {
                *prev.points.last_mut().unwrap() = line.b;
            }
            i += 1;
            continue;
        }

        let thickness_delta = (line.a_width - line.b_width).abs();
        if thickness_delta > tolerance {
            // Split into segments whose widths differ by at most the tolerance.
            let segments = (thickness_delta / tolerance).ceil() as usize;
            let seg_len = line_len / segments as f64;
            let (dx, dy) = (
                (line.b.x - line.a.x) as f64 / line_len,
                (line.b.y - line.a.y) as f64 / line_len,
            );
            let mut pp = vec![line.a];
            let mut width = vec![line.a_width];
            for j in 1..segments {
                let d = j as f64 * seg_len;
                pp.push(Point::new(
                    (line.a.x as f64 + dx * d) as i64,
                    (line.a.y as f64 + dy * d) as i64,
                ));
                let w = line.a_width + d * (line.b_width - line.a_width) / line_len;
                width.push(w);
                width.push(w);
            }
            pp.push(line.b);
            width.push(line.b_width);
            let new_lines = (0..segments).map(|j| crate::geometry::medial_axis::ThickLine {
                a: pp[j],
                b: pp[j + 1],
                a_width: width[2 * j],
                b_width: width[2 * j + 1],
            });
            lines.splice(i..=i, new_lines);
            continue;
        }

        let w = line.a_width.max(line.b_width);
        let new_flow = flow_for_spacing(flow, w / SCALE);
        match path.as_mut() {
            None => {
                path = Some(ExtrusionPath {
                    overhang: None,
                    perimeter_index: None,
                    seam: None,
                    role,
                    points: vec![line.a, line.b],
                    flow: new_flow,
                    brick: None,
                });
            }
            Some(current) => {
                let delta = (current.flow.width - new_flow.width).abs() * SCALE;
                if delta <= merge_tolerance {
                    current.points.push(line.b);
                } else {
                    // Start a new path with this line.
                    paths.push(path.take().unwrap());
                    continue;
                }
            }
        }
        i += 1;
    }
    if let Some(path) = path
        && path.points.len() >= 2
    {
        paths.push(path);
    }
    paths
}

/// Orders runs of paths greedily by nearest start or end from `start`, reversing runs where that is
/// shorter, and flattens them.
pub fn chain_runs(mut runs: Vec<Vec<ExtrusionPath>>, start: Point) -> Vec<ExtrusionPath> {
    let mut out = Vec::new();
    let mut pos = start;
    while !runs.is_empty() {
        let (idx, reverse) = runs
            .iter()
            .enumerate()
            .flat_map(|(k, run)| {
                let first = run[0].points[0];
                let last = *run.last().unwrap().points.last().unwrap();
                [
                    (k, false, first.distance_sq(pos)),
                    (k, true, last.distance_sq(pos)),
                ]
            })
            .min_by_key(|&(_, _, d)| d)
            .map(|(k, r, _)| (k, r))
            .unwrap();
        let mut run = runs.swap_remove(idx);
        if reverse {
            run.reverse();
            for path in &mut run {
                path.points.reverse();
            }
        }
        pos = *run.last().unwrap().points.last().unwrap();
        out.extend(run);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::clip::union_ex_positive;
    use crate::geometry::{polygon_length, polyline_length, unscale};

    fn params() -> WallParams {
        WallParams {
            perimeters: 2,
            external: Flow::new(0.45, 0.2),
            internal: Flow::new(0.45, 0.2),
            solid_infill: Flow::new(0.45, 0.2),
            infill_overlap: FloatOrPercent::Percent(15.0),
            nozzle_diameter: 0.4,
            thin_walls: false,
            gap_fill: true,
            filter_out_gap_fill: 0.,
            fill_density: 0.15,
        }
    }

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Polygon {
        vec![
            Point::from_mm(x0, y0),
            Point::from_mm(x1, y0),
            Point::from_mm(x1, y1),
            Point::from_mm(x0, y1),
        ]
    }

    fn square(size: f64) -> ExPolygon {
        union_ex_positive(&[rect(0.0, 0.0, size, size)]).remove(0)
    }

    fn run_length_mm(runs: &[Vec<ExtrusionPath>]) -> f64 {
        runs.iter()
            .flatten()
            .map(|p| polyline_length(&p.points))
            .sum::<f64>()
            / SCALE
    }

    #[test]
    fn two_walls_on_a_square() {
        let w = make_walls(&square(20.0), &params());
        assert_eq!(w.loops.len(), 2);
        // Outer wall centreline is half a width (0.225 mm) inside the outline.
        let outer = &w.loops[0];
        assert_eq!(outer.depth, 0);
        assert!((unscale(polygon_length(&outer.polygon) as i64) - 4.0 * 19.55).abs() < 1e-3);
        // Inner wall is one spacing further in.
        let inner = &w.loops[1];
        let side = 19.55 - 2.0 * 0.407_08;
        assert!((unscale(polygon_length(&inner.polygon) as i64) - 4.0 * side).abs() < 1e-3);
        assert!(w.loops.iter().all(|l| signed_area(&l.polygon) > 0.0));
        assert_eq!(w.infill_area.len(), 1);
        // A square leaves no gaps.
        assert!(w.gap_fill.is_empty());
    }

    #[test]
    fn hole_walls_run_clockwise() {
        let mut hole = rect(8.0, 8.0, 12.0, 12.0);
        hole.reverse();
        let mut island = square(20.0);
        island.holes.push(hole);
        let w = make_walls(&island, &params());
        let holes: Vec<_> = w.loops.iter().filter(|l| !l.is_contour).collect();
        assert_eq!(holes.len(), 2);
        assert!(holes.iter().all(|l| signed_area(&l.polygon) < 0.0));
    }

    #[test]
    fn thin_island_gets_no_walls() {
        let w = make_walls(&square(0.3), &params());
        assert!(w.loops.is_empty());
        assert!(w.infill_area.is_empty());
        assert!(w.gap_fill.is_empty());
    }

    #[test]
    fn strip_between_walls_is_gap_filled() {
        // 1.1 mm wide strip: inside the outer loop's centreline 0.65 mm are left, too little for a second
        // loop (2 × 0.407 mm), so the middle 0.24 mm become gap fill along the strip and there is no infill.
        let island = union_ex_positive(&[rect(0.0, 0.0, 20.0, 1.1)]).remove(0);
        let w = make_walls(&island, &params());
        assert!(w.loops.iter().all(|l| l.depth == 0));
        assert!(!w.gap_fill.is_empty());
        assert!(w.infill_area.is_empty());
        let len = run_length_mm(&w.gap_fill);
        assert!(len > 18.0 && len < 20.5, "gap fill length {len}");
        assert!(
            w.gap_fill
                .iter()
                .flatten()
                .all(|p| p.role == Role::GapFill && p.flow.width > 0.1 && p.flow.width < 1.0)
        );
    }

    #[test]
    fn gap_length_filter_runs_before_width_splitting_and_coverage() {
        let island = union_ex_positive(&[rect(0., 0., 20., 1.1)]).remove(0);
        let mut p = params();
        let unfiltered = make_walls(&island, &p);
        assert!(!unfiltered.gap_fill.is_empty());
        p.filter_out_gap_fill = 100.;
        let filtered = make_walls(&island, &p);
        assert!(filtered.gap_fill.is_empty());
        assert_eq!(unfiltered.loops, filtered.loops);
        p.filter_out_gap_fill = 1.;
        assert_eq!(make_walls(&island, &p).gap_fill, unfiltered.gap_fill);
    }

    #[test]
    fn gap_fill_off_leaves_gaps_empty() {
        let island = union_ex_positive(&[rect(0.0, 0.0, 20.0, 1.1)]).remove(0);
        let p = WallParams {
            gap_fill: false,
            ..params()
        };
        assert!(make_walls(&island, &p).gap_fill.is_empty());
    }

    #[test]
    fn thin_wall_becomes_a_variable_width_external_perimeter() {
        // 0.6 mm wall: with thin walls on, the first offset (which needs room for the outer wall plus
        // 60% of a spacing) is empty, so it becomes one centreline extrusion along its length. Without,
        // it gets a squashed loop.
        let island = union_ex_positive(&[rect(0.0, 0.0, 10.0, 0.6)]).remove(0);
        let off = make_walls(&island, &params());
        assert_eq!(off.loops.len(), 1);
        assert!(off.thin_walls.is_empty());
        let p = WallParams {
            thin_walls: true,
            ..params()
        };
        let w = make_walls(&island, &p);
        assert!(w.loops.is_empty());
        assert_eq!(w.thin_walls.len(), 1);
        let len = run_length_mm(&w.thin_walls);
        assert!((len - 10.0).abs() < 0.05, "thin wall length {len}");
        for path in w.thin_walls.iter().flatten() {
            assert_eq!(path.role, Role::ExternalPerimeter);
            // Spacing 0.6 mm → width 0.6 + 0.2·(1 − π/4).
            assert!(
                (path.flow.width - 0.642_92).abs() < 1e-3,
                "{}",
                path.flow.width
            );
        }
    }

    #[test]
    fn multi_path_splits_by_width() {
        let tp = ThickPolyline {
            points: vec![Point::from_mm(0.0, 0.0), Point::from_mm(10.0, 0.0)],
            width: vec![scale(0.2) as f64, scale(0.6) as f64],
            endpoints: (true, true),
        };
        let flow = Flow::new(0.45, 0.2);
        let tol = scale(0.05) as f64;
        let paths = thick_polyline_to_multi_path(&tp, Role::GapFill, flow, tol, tol);
        // 0.4 mm of taper in 0.05 mm steps: 8 segments; neighbours within 0.05 mm share a path.
        assert!((4..=8).contains(&paths.len()), "{}", paths.len());
        let total: f64 = paths
            .iter()
            .map(|p| polyline_length(&p.points))
            .sum::<f64>()
            / SCALE;
        assert!((total - 10.0).abs() < 1e-5);
        assert!(paths.windows(2).all(|w| w[0].flow.width < w[1].flow.width));
        assert!(
            paths
                .windows(2)
                .all(|w| w[0].points.last() == w[1].points.first())
        );
        // A path takes the larger end width of its first segment as spacing.
        assert!((paths[0].flow.spacing() - 0.25).abs() < 1e-6);
        let last = paths.last().unwrap().flow.spacing();
        assert!((0.55 - 1e-6..=0.6 + 1e-6).contains(&last), "{last}");
    }

    #[test]
    fn chain_runs_reverses_to_the_nearer_end() {
        let flow = Flow::new(0.45, 0.2);
        let path = |a: (f64, f64), b: (f64, f64)| ExtrusionPath {
            overhang: None,
            perimeter_index: None,
            seam: None,
            role: Role::GapFill,
            points: vec![Point::from_mm(a.0, a.1), Point::from_mm(b.0, b.1)],
            flow,
            brick: None,
        };
        let runs = vec![
            vec![path((10.0, 0.0), (5.0, 0.0)), path((5.0, 0.0), (0.0, 0.0))],
            vec![path((20.0, 0.0), (30.0, 0.0))],
        ];
        let out = chain_runs(runs, Point::from_mm(-1.0, 0.0));
        let starts: Vec<f64> = out.iter().map(|p| p.points[0].x as f64 / SCALE).collect();
        assert_eq!(starts, vec![0.0, 5.0, 20.0]);
    }

    fn classic_settings(orca: bool) -> crate::settings::PrintSettings {
        let mut s = crate::settings::PrintSettings::core_one_hf04_speed_pla();
        s.perimeter_generator = crate::settings::PerimeterGenerator::Classic;
        s.top_one_perimeter = crate::settings::TopOnePerimeter::TopSurfaces;
        s.top_bottom_infill_overlap = orca.then_some(FloatOrPercent::Percent(25.0));
        s.top_surface_min_width = FloatOrPercent::Percent(300.0);
        s
    }

    fn inner_length_mm(w: &IslandWalls) -> f64 {
        w.loops
            .iter()
            .filter(|l| l.depth > 0)
            .map(|l| polygon_length(&l.polygon))
            .sum::<f64>()
            / SCALE
    }

    #[test]
    fn classic_single_wall_on_exposed_top_surfaces() {
        let island = square(20.0);
        let mut p = params();
        p.perimeters = 3;
        // Only a 6 mm column continues above the centre: the rest of the layer is top surface.
        let upper = vec![union_ex_positive(&[rect(7.0, 7.0, 13.0, 13.0)]).remove(0)];
        let lower = vec![square(20.0)];
        for orca in [false, true] {
            let s = classic_settings(orca);
            let full = make_walls(&island, &p);
            let split = make_walls_selected(&island, &p, &s, Some(&lower), Some(&upper), 5);
            assert_eq!(split.loops.iter().filter(|l| l.depth == 0).count(), 1);
            // Inner walls only surround the region below the column (Orca/Prusa top split).
            assert!(
                inner_length_mm(&split) < 0.6 * inner_length_mm(&full),
                "{orca}"
            );
            let area = |a: &[ExPolygon]| a.iter().map(ExPolygon::area_mm2).sum::<f64>();
            assert!(
                area(&split.infill_area) > area(&full.infill_area) + 20.0,
                "{orca}"
            );
            // The topmost layer gets a single wall.
            let top = make_walls_selected(&island, &p, &s, Some(&lower), None, 5);
            assert!(top.loops.iter().all(|l| l.depth == 0));
        }
    }

    #[test]
    fn orca_classic_collapses_narrow_inner_walls_into_gap_fill() {
        // 2.3 mm wide strip with three walls: the third inner loop would be 0.2 mm wide.
        let island = union_ex_positive(&[rect(0.0, 0.0, 2.3, 20.0)]).remove(0);
        let mut p = params();
        p.perimeters = 3;
        let lower = vec![island.clone()];
        let upper = lower.clone();
        let mut prusa = classic_settings(false);
        prusa.top_one_perimeter = crate::settings::TopOnePerimeter::None;
        let mut orca = classic_settings(true);
        orca.top_one_perimeter = crate::settings::TopOnePerimeter::None;
        let a = make_walls_selected(&island, &p, &prusa, Some(&lower), Some(&upper), 5);
        let b = make_walls_selected(&island, &p, &orca, Some(&lower), Some(&upper), 5);
        assert!(
            b.loops.len() < a.loops.len(),
            "{} {}",
            b.loops.len(),
            a.loops.len()
        );
        assert!(run_length_mm(&b.gap_fill) > run_length_mm(&a.gap_fill) + 10.0);
    }

    #[test]
    fn classic_precise_outer_wall_uses_widths_between_outer_and_inner() {
        let mut p = params();
        p.external = Flow::new(0.42, 0.2);
        let island = square(20.0);
        let mut s = classic_settings(true);
        s.top_one_perimeter = crate::settings::TopOnePerimeter::None;
        let upper = vec![island.clone()];
        let inner_side = |s: &crate::settings::PrintSettings| {
            let w = make_walls_selected(&island, &p, s, Some(&upper), Some(&upper), 5);
            let inner = w.loops.iter().find(|l| l.depth == 1).unwrap();
            unscale(polygon_length(&inner.polygon) as i64) / 4.0
        };
        let normal = inner_side(&s);
        s.precise_outer_wall = true;
        let precise = inner_side(&s);
        let spacing = |f: Flow| f.spacing();
        let expected =
            (0.5 * (0.42 + 0.45) - 0.5 * (spacing(p.external) + spacing(p.internal))) * 2.0;
        assert!(
            (normal - precise - expected).abs() < 1e-3,
            "{normal} {precise} {expected}"
        );
    }

    #[test]
    fn classic_overhang_split_is_an_ordered_closed_run() {
        let poly = rect(0.0, 0.0, 10.0, 10.0);
        // Only the left half is supported.
        let supported = vec![union_ex_positive(&[rect(-1.0, -1.0, 5.0, 11.0)]).remove(0)];
        let flow = Flow::new(0.45, 0.2);
        let bridge = Flow::new(0.4, 0.4);
        let run =
            split_classic_overhangs(&poly, &supported, Role::Perimeter, flow, bridge).unwrap();
        assert_eq!(run.len(), 2);
        assert_eq!(run[0].points.first(), run[1].points.last());
        assert!(
            run.windows(2)
                .all(|w| w[0].points.last() == w[1].points.first())
        );
        let len = |r: Role| {
            run.iter()
                .filter(|p| p.role == r)
                .map(|p| polyline_length(&p.points))
                .sum::<f64>()
                / SCALE
        };
        assert!((len(Role::Perimeter) - 20.0).abs() < 1e-3);
        assert!((len(Role::OverhangPerimeter) - 20.0).abs() < 1e-3);
        assert!(
            run.iter()
                .any(|p| p.role == Role::OverhangPerimeter && p.flow == bridge)
        );
        let full = vec![union_ex_positive(&[rect(-1.0, -1.0, 11.0, 11.0)]).remove(0)];
        assert!(split_classic_overhangs(&poly, &full, Role::Perimeter, flow, bridge).is_none());
    }
}
