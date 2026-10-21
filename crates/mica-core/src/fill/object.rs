// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/PrintObject.cpp :: PrintObject::prepare_adaptive_infill_data
// Source: prusaslicer@b028299c77 src/libslic3r/Fill/Fill.cpp :: Layer::make_fills
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/ExtrusionOrder.cpp :: sort_fill_extrusions
// Changes: Immutable once-per-object context with a compact octree or lightning forest; dispatch for streamed batches.
// Changes: Chain surface collections before their paths, preserving monotonic/concentric order inside each collection.
// Changes: `collections` returns the unordered per-ExPolygon collections (no_sort, multi-path) for the
// Changes: Prusa island ordering in extrusion_order.rs; `toolpaths` keeps the Orca-path order.

use super::{FillParams, Pattern, adaptive::Octree, lightning::Generator};
use crate::{
    flow::Flow,
    geometry::{BoundingBox, ExPolygon, Point, SCALE},
    mesh::TriangleMesh,
    settings::{PerimeterGenerator, PrintSettings},
    shells::FillSurfaces,
    toolpath::{ExtrusionPath, Role},
};
#[derive(Clone, Debug, Default)]
pub struct PreparedFill {
    pub bounds: Option<BoundingBox>,
    /// Height added below the mesh by a raft.
    pub z_offset: f64,
    pub adaptive: Option<Octree>,
    pub lightning: Option<Generator>,
}
impl PreparedFill {
    pub(crate) fn finish_surfaces(
        &mut self,
        surfaces: &mut [FillSurfaces],
        tops: &[f64],
        heights: &[f64],
        s: &PrintSettings,
        origin: Point,
    ) {
        self.z_offset =
            tops.first().copied().unwrap_or(0.) - heights.first().copied().unwrap_or(0.);
        if !s.fill_pattern.needs_object() {
            return;
        }
        let layers: Vec<_> = tops
            .iter()
            .zip(heights)
            .enumerate()
            .map(|(i, (&print_z, &height))| {
                let flow = |w| {
                    Flow::new(
                        if i == 0 && s.raft_layers == 0 {
                            s.first_layer_extrusion_width
                        } else {
                            w
                        },
                        height,
                    )
                };
                crate::shells::ShellLayer {
                    print_z,
                    height,
                    external: flow(s.external_perimeter_extrusion_width),
                    perimeter: flow(s.perimeter_extrusion_width),
                    solid: flow(s.solid_infill_extrusion_width),
                }
            })
            .collect();
        crate::bridge::bridge_over_infill_prepared(surfaces, &layers, s, origin, Some(self));
    }
    pub fn prepare(
        mesh: &TriangleMesh,
        s: &PrintSettings,
        surfaces: &[FillSurfaces],
        origin: Point,
    ) -> Self {
        let (min, max) = mesh.bounds();
        let bounds = Some(BoundingBox {
            min: Point::from_mm(f64::from(min[0]), f64::from(min[1])),
            max: Point::from_mm(f64::from(max[0]), f64::from(max[1])),
        });
        Self {
            bounds,
            z_offset: 0.,
            adaptive: if s.fill_density > 0.
                && matches!(
                    s.fill_pattern,
                    Pattern::AdaptiveCubic | Pattern::SupportCubic
                ) {
                Some(Octree::build(
                    mesh,
                    origin,
                    3. * s.infill_extrusion_width / s.fill_density,
                    s.fill_pattern == Pattern::SupportCubic,
                ))
            } else {
                None
            },
            lightning: if s.fill_density > 0. && s.fill_pattern == Pattern::Lightning {
                Some(Generator::build(surfaces, s, origin))
            } else {
                None
            },
        }
    }
}
/// Generate extrusion paths, preserving variable widths for solid concentric fill.
#[allow(clippy::too_many_arguments)]
pub fn toolpaths(
    pattern: Pattern,
    area: &[ExPolygon],
    p: &FillParams,
    flow: Flow,
    role: Role,
    s: &PrintSettings,
    context: Option<&PreparedFill>,
    from: Point,
) -> Vec<ExtrusionPath> {
    if area.is_empty() || p.density <= 0. {
        return Vec::new();
    }
    if pattern == Pattern::Ensuring {
        return super::ensuring::fill(area, p, flow, s);
    }
    if arachne_concentric(pattern, p, s) {
        return super::concentric::arachne(area, p, flow, role, s);
    }
    let mut generated = generate(pattern, area, p, flow, s, context);
    let mut out = Vec::new();
    let mut pos = from;
    while !generated.is_empty() {
        // A no-sort fill preserves its scan order, not the arbitrary order of disconnected
        // surfaces returned by clipping. Select the next collection from the current nozzle.
        let next = generated
            .iter()
            .enumerate()
            .min_by_key(|(_, filled)| {
                if pattern.no_sort() {
                    filled
                        .polylines
                        .first()
                        .and_then(|p| p.first())
                        .map_or(i128::MAX, |p| p.distance_sq(pos))
                } else {
                    filled
                        .polylines
                        .iter()
                        .flat_map(|p| p.first().into_iter().chain(p.last()))
                        .map(|p| p.distance_sq(pos))
                        .min()
                        .unwrap_or(i128::MAX)
                }
            })
            .map(|(i, _)| i)
            .unwrap();
        let filled = generated.remove(next);
        let lines = if pattern.no_sort() {
            filled.polylines
        } else {
            super::order_nearest(filled.polylines, pos)
        };
        for path in filled_paths(pattern, lines, filled.spacing, flow, role, s) {
            pos = *path.points.last().unwrap();
            out.push(path);
        }
    }
    out
}

/// Solid concentric fill with Arachne: variable-width loops (`Fill::fill_surface_arachne`).
fn arachne_concentric(pattern: Pattern, p: &FillParams, s: &PrintSettings) -> bool {
    pattern == Pattern::Concentric
        && s.perimeter_generator == PerimeterGenerator::Arachne
        && p.density > 0.9999
        && !p.dont_adjust
}

/// The extrusion paths of one filled ExPolygon's polylines (`Layer::make_fills`): solid fills take
/// the flow of the spacing the pattern settled on, concentric loops are clipped at their ends.
fn filled_paths(
    pattern: Pattern,
    lines: Vec<crate::geometry::Polyline>,
    spacing: f64,
    flow: Flow,
    role: Role,
    s: &PrintSettings,
) -> impl Iterator<Item = ExtrusionPath> + '_ {
    let flow = if role == Role::InternalInfill {
        flow
    } else {
        flow.with_spacing(spacing)
    };
    lines.into_iter().filter_map(move |mut points| {
        if pattern == Pattern::Concentric {
            super::concentric::clip_end(&mut points, s.nozzle_diameter * 0.15 * SCALE);
        }
        (points.len() >= 2).then_some(ExtrusionPath {
            points,
            role,
            flow,
            seam: None,
            overhang: None,
            perimeter_index: None,
            brick: None,
        })
    })
}

/// One `ExtrusionEntityCollection` of `Layer::make_fills`: the fill of one ExPolygon of a surface
/// group. Each entity is a path, or a multi-path / loop made from a variable-width line (`multi`).
#[derive(Clone, Debug, Default)]
pub struct FillCollection {
    pub entities: Vec<Vec<ExtrusionPath>>,
    /// `Fill::no_sort()`: the entities keep their generated order (monotonic, concentric, ensuring).
    pub no_sort: bool,
    /// Entities are `ExtrusionMultiPath`s or `ExtrusionLoop`s rather than single `ExtrusionPath`s.
    pub multi: bool,
}

/// The fill collections of `area` (one per filled ExPolygon) in generation order, unordered.
#[allow(clippy::too_many_arguments)]
pub fn collections(
    pattern: Pattern,
    area: &[ExPolygon],
    p: &FillParams,
    flow: Flow,
    role: Role,
    s: &PrintSettings,
    context: Option<&PreparedFill>,
) -> Vec<FillCollection> {
    if area.is_empty() || p.density <= 0. {
        return Vec::new();
    }
    if pattern == Pattern::Ensuring {
        return super::ensuring::entities(area, p, flow, s)
            .into_iter()
            .filter(|e| !e.is_empty())
            .map(|entities| FillCollection {
                entities,
                no_sort: true,
                multi: true,
            })
            .collect();
    }
    if arachne_concentric(pattern, p, s) {
        let entities: Vec<_> = super::concentric::arachne(area, p, flow, role, s)
            .into_iter()
            .map(|path| vec![path])
            .collect();
        return if entities.is_empty() {
            Vec::new()
        } else {
            vec![FillCollection {
                entities,
                no_sort: true,
                multi: true,
            }]
        };
    }
    generate(pattern, area, p, flow, s, context)
        .into_iter()
        .map(|filled| FillCollection {
            entities: filled_paths(pattern, filled.polylines, filled.spacing, flow, role, s)
                .map(|path| vec![path])
                .collect(),
            no_sort: pattern.no_sort(),
            multi: false,
        })
        .filter(|c| !c.entities.is_empty())
        .collect()
}

/// The pattern's polylines per filled ExPolygon, in generation order (`Fill::fill_surface`).
fn generate(
    pattern: Pattern,
    area: &[ExPolygon],
    p: &FillParams,
    flow: Flow,
    s: &PrintSettings,
    context: Option<&PreparedFill>,
) -> Vec<super::Filled> {
    match pattern {
        Pattern::AdaptiveCubic | Pattern::SupportCubic | Pattern::Lightning => {
            let context = context.expect("object-dependent infill requires PreparedFill");
            let mut lines = if pattern == Pattern::Lightning {
                context
                    .lightning
                    .as_ref()
                    .expect("lightning not prepared")
                    .lines(
                        p.layer_id,
                        0.5 * p.spacing * SCALE
                            - s.infill_overlap.resolve(flow.spacing() * SCALE, SCALE),
                    )
            } else {
                let mut lines = context
                    .adaptive
                    .as_ref()
                    .expect("adaptive infill not prepared")
                    .lines(p.z - context.z_offset);
                for line in &mut lines {
                    super::translate(line, p.origin.x, p.origin.y);
                }
                lines
            };
            lines.retain(|l| l.len() > 1);
            area.iter()
                .flat_map(|ex| super::concentric::shrunk(ex, p))
                .map(|ex| super::Filled {
                    polylines: super::clipped_connected(lines.clone(), &ex, p),
                    spacing: p.spacing,
                })
                .collect()
        }
        _ => {
            let bounds = context.and_then(|c| c.bounds).map(|b| BoundingBox {
                min: Point::new(b.min.x - p.origin.x, b.min.y - p.origin.y),
                max: Point::new(b.max.x - p.origin.x, b.max.y - p.origin.y),
            });
            super::fill_with_bounds(pattern, area, p, bounds)
        }
    }
}
