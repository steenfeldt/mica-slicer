// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: orca@8500fcdcca src/libslic3r/GCode.cpp :: GCode::extrude_support
// Changes: The bounded preparation is original. Orca's support chaining runs on each layer's support paths before planning, from the end of the path prepared before them.
// Source: orca@8500fcdcca src/libslic3r/GCode.cpp :: GCode::process_layer
// Changes: Orca rules estimate overhang speeds (`ExtrusionQualityEstimator`) with each path's region and tool configuration.
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/ExtrusionOrder.cpp :: get_support_extrusions
// Changes: Prusa rules chain supports through support_order before fitting; normal supports use
// Changes: upstream's unseeded outer chain, organic carries the preceding endpoint across batches.
// Changes: The Orca travel geometry of a batch is computed while its in-order steps run, and used when the layer's height and first external wall width are unchanged.
// Source: prusaslicer@b028299c77 src/libslic3r/GCode.cpp :: SmoothPathGenerator::operator()
// Source: orca@8500fcdcca src/libslic3r/GCode.cpp :: GCode::extrude_loop
// Changes: Spiral vase: a spiral layer's loop starts at the point nearest where the previous layer ended, carried across batches.

//! Bounded preparation ahead of the ordered writer. Curl history is advanced in order;
//! immutable curve fits and travel geometry use the engine's shared worker pool.

use crate::{overhang_plan::LayerCurves, smooth_path::ArcFitting};
use mica_core::{
    config::Config, geometry::Point, print::LayerSource, print_job::ScheduledLayer,
    settings::PrintSettings, toolpath::Role,
};
use std::sync::Arc;

#[derive(Default)]
pub(crate) struct Prepared {
    pub curves: LayerCurves,
    pub travel: Option<Arc<crate::orca_motion::TravelGeometry>>,
}

/// State the in-order preparation hands from one batch to the next.
#[derive(Default)]
pub(crate) struct Carry {
    overhangs: crate::overhang::OverhangProcessor,
    /// End of the last path prepared, for support chaining.
    last: Option<Point>,
    /// Spiral vase: where the previous layer's extrusions ended.
    spiral_last: Option<Point>,
}

/// Orca `GCode::extrude_support`: `chain_and_reorder_extrusion_entities` of each run of support
/// paths printed together (same object and tool), starting near the last position.
fn chain_orca_support(scheduled: &mut ScheduledLayer, last: &mut Option<Point>) {
    let support = |r: Role| matches!(r, Role::SupportMaterial | Role::SupportMaterialInterface);
    let key = |s: &ScheduledLayer, k: usize| s.assignments.get(k).map(|a| (a.object, a.tool));
    let mut i = 0;
    while i < scheduled.layer.paths.len() {
        if !support(scheduled.layer.paths[i].role) {
            *last = scheduled.layer.paths[i].points.last().copied().or(*last);
            i += 1;
            continue;
        }
        let mut j = i + 1;
        while j < scheduled.layer.paths.len()
            && support(scheduled.layer.paths[j].role)
            && key(scheduled, j) == key(scheduled, i)
        {
            j += 1;
        }
        let entities: Vec<_> = scheduled.layer.paths[i..j]
            .iter()
            .map(|p| {
                let (first, end) = (p.points[0], *p.points.last().unwrap());
                mica_core::shortest_path::Entity {
                    first,
                    last: end,
                    is_loop: p.points.len() > 2 && first == end,
                    can_reverse: true,
                }
            })
            .collect();
        let order = mica_core::shortest_path::chain_entities(&entities, *last, false);
        let paths = scheduled.layer.paths[i..j].to_vec();
        let assignments = scheduled
            .assignments
            .get(i..j)
            .map(<[_]>::to_vec)
            .unwrap_or_default();
        for (k, &(from, reverse)) in order.iter().enumerate() {
            let mut path = paths[from].clone();
            if reverse {
                path.points.reverse();
            }
            scheduled.layer.paths[i + k] = path;
            if let Some(a) = assignments.get(from) {
                scheduled.assignments[i + k] = *a;
            }
        }
        *last = scheduled.layer.paths[j - 1]
            .points
            .last()
            .copied()
            .or(*last);
        i = j;
    }
}

pub(crate) struct Parameters<'a> {
    pub source: &'a dyn LayerSource,
    pub config: &'a Config,
    pub settings: &'a PrintSettings,
    pub tolerance: f64,
    pub mode: ArcFitting,
    pub overhang_pairs: &'a crate::orca_motion::OverhangPairs,
}

impl Parameters<'_> {
    /// The configuration, settings and tool of path `i` of a scheduled layer.
    fn path_settings(
        &self,
        assignments: &[mica_core::print_job::PathAssignment],
        i: usize,
    ) -> (&Config, &PrintSettings, usize) {
        self.source
            .print_job()
            .map_or((self.config, self.settings, 0), |job| {
                let a = assignments[i];
                let (config, settings) = job.objects[a.object].path_config(a.region, a.tool);
                (config, settings, a.tool)
            })
    }

    pub fn prepare(
        &self,
        batch: &mut [ScheduledLayer],
        start: usize,
        carry: &mut Carry,
    ) -> Vec<Prepared> {
        let orca = crate::orca_motion::enabled(self.config);
        let Carry {
            overhangs,
            last,
            spiral_last,
        } = carry;
        // The travel geometry only reads the source and each layer's height and first external
        // wall width, which the in-order steps below do not change (checked when it is used):
        // it is computed meanwhile, on the shared pool.
        let ahead: Vec<_> = batch
            .iter()
            .map(|s| mica_core::toolpath::LayerToolpaths {
                print_z: s.layer.print_z,
                paths: s
                    .layer
                    .paths
                    .iter()
                    .find(|p| p.role == Role::ExternalPerimeter)
                    .cloned()
                    .into_iter()
                    .collect(),
                ..Default::default()
            })
            .collect();
        let mut ready = Vec::new();
        mica_core::parallel::join(
            || self.prepare_in_order(batch, start, overhangs, (last, spiral_last), orca),
            || {
                ready = mica_core::parallel::map(&ahead, |offset, layer| {
                    let _timer =
                        mica_core::timing::Span::new(mica_core::timing::Stage::ExportTravel);
                    crate::travels::orca_travel_geometry(
                        self.config,
                        self.source,
                        start + offset,
                        layer,
                        Some(self.overhang_pairs),
                    )
                })
            },
        );
        let ready = std::sync::Mutex::new(ready.into_iter().map(Some).collect::<Vec<_>>());
        mica_core::parallel::map(batch, |offset, scheduled| {
            let curves = LayerCurves::new(
                &scheduled.layer,
                |i| self.path_settings(&scheduled.assignments, i),
                self.tolerance,
                self.mode,
            );
            let _timer = mica_core::timing::Span::new(mica_core::timing::Stage::ExportTravel);
            let index = start + offset;
            let mut travel = None;
            let ahead = ready.lock().unwrap_or_else(|p| p.into_inner())[offset]
                .take()
                .flatten();
            crate::travels::TravelPlanner::new(
                self.config,
                &scheduled.layer,
                self.source
                    .previous_layer(index)
                    .and_then(|i| self.source.layer_outlines(i)),
                self.source.layer_outlines(index),
            )
            .with_source_ready(
                self.source,
                index,
                &mut travel,
                Some(self.overhang_pairs),
                ahead,
            );
            Prepared { curves, travel }
        })
    }

    /// The steps that carry state from layer to layer (support chaining, curl history), in
    /// layer order.
    fn prepare_in_order(
        &self,
        batch: &mut [ScheduledLayer],
        start: usize,
        overhangs: &mut crate::overhang::OverhangProcessor,
        (last, spiral_last): (&mut Option<Point>, &mut Option<Point>),
        orca: bool,
    ) {
        let spiral = mica_core::spiral_vase::enabled(self.config);
        for (offset, scheduled) in batch.iter_mut().enumerate() {
            if spiral {
                // Spiral vase: a spiral layer's loop starts where the previous layer ended
                // (the loop's split at the previous position, across layers).
                if let Some(from) = *spiral_last
                    && mica_core::spiral_vase::spiral_layer(self.settings, scheduled.layer.index)
                    && let Some(sources) = crate::spiral_vase::start_loop_near(
                        &mut scheduled.layer.paths,
                        from,
                        mica_core::rules::Rules::of(self.config).is_orca(),
                    )
                    && !scheduled.assignments.is_empty()
                {
                    scheduled.assignments =
                        sources.iter().map(|&k| scheduled.assignments[k]).collect();
                }
                if let Some(end) = scheduled.layer.paths.last().and_then(|p| p.points.last()) {
                    *spiral_last = Some(*end);
                }
            }
            if orca {
                chain_orca_support(scheduled, last);
            }
            // A sequential object stands on the bed: no curl history from the one before.
            if start + offset > 0 && self.source.previous_layer(start + offset).is_none() {
                *overhangs = crate::overhang::OverhangProcessor::default();
            }
            crate::adhesion::prepare_paths(&mut scheduled.layer, self.settings);
            if !orca {
                let normal = matches!(
                    self.config.string("support_material_style"),
                    Some("grid" | "snug")
                );
                crate::support_order::chain_support(scheduled, last, normal);
            }
            let outlines = self.source.layer_outlines(start + offset);
            if mica_core::rules::Rules::of(self.config).is_orca() {
                let first = scheduled.layer.index == 0;
                let assignments = &scheduled.assignments;
                overhangs.process_orca(&mut scheduled.layer, outlines, |i, path| {
                    let (config, settings, tool) = self.path_settings(assignments, i);
                    crate::orca_overhang::speeds(config, settings, path, tool, first)
                });
            } else {
                overhangs.process(&mut scheduled.layer, outlines, self.config);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mica_core::{
        flow::Flow,
        toolpath::{ExtrusionPath, LayerToolpaths},
    };
    fn line(role: Role, a: (f64, f64), b: (f64, f64)) -> ExtrusionPath {
        ExtrusionPath {
            points: vec![Point::from_mm(a.0, a.1), Point::from_mm(b.0, b.1)],
            flow: Flow::new(0.42, 0.2),
            role,
            perimeter_index: None,
            overhang: None,
            seam: None,
            brick: None,
        }
    }
    #[test]
    fn orca_support_runs_chain_from_the_last_position() {
        let mut scheduled = ScheduledLayer {
            layer: LayerToolpaths {
                paths: vec![
                    line(Role::SupportMaterial, (50., 0.), (51., 0.)),
                    line(Role::SupportMaterial, (0., 0.), (1., 0.)),
                    line(Role::SupportMaterialInterface, (3., 0.), (2., 0.)),
                    line(Role::Perimeter, (60., 0.), (70., 0.)),
                ],
                ..Default::default()
            },
            assignments: Vec::new(),
        };
        let mut last = Some(Point::from_mm(-1., 0.));
        chain_orca_support(&mut scheduled, &mut last);
        let starts: Vec<_> = scheduled
            .layer
            .paths
            .iter()
            .map(|p| p.points[0].x as f64 / mica_core::geometry::SCALE)
            .collect();
        // Nearest first, the interface reversed to continue forward; the walls stay last.
        assert_eq!(starts, [0., 2., 50., 60.]);
        assert_eq!(last, Some(Point::from_mm(70., 0.)));
    }
}
