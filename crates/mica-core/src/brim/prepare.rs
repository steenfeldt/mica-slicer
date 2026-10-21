// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/Print.cpp :: Print::_make_skirt, Print::first_layer_islands
// Changes: Adapter for Mica's single-object schedule; retained plan also accepts plate-level inputs.

use super::*;
use crate::{
    settings::PrintSettings,
    support::{SupportPlan, schedule::PrintLayer},
};

#[allow(clippy::too_many_arguments)]
pub fn object_plan(
    outlines: &[Vec<ExPolygon>],
    uncompensated: &[ExPolygon],
    tops: &[f64],
    supports: &SupportPlan,
    schedule: &[PrintLayer],
    settings: &PrintSettings,
    object_height: f64,
    ears: &[BrimEar],
) -> AdhesionPlan {
    let s = &settings.adhesion;
    let count = s.skirt_layers(tops.len());
    let skirt_z = tops.get(count.saturating_sub(1)).copied().unwrap_or(0.0);
    let first_z = schedule
        .first()
        .map_or(settings.first_layer_height, |p| p.print_z);
    let on_bed = tops.first().is_some_and(|z| (*z - first_z).abs() < 1e-6);
    let base = if on_bed {
        if s.orca && !s.brim_efc {
            uncompensated
        } else {
            outlines.first().map_or(&[][..], Vec::as_slice)
        }
    } else {
        &[]
    };
    let islands: Vec<_> = base
        .iter()
        .map(|polygon| FirstLayerIsland {
            object_id: 0,
            polygon,
            brim: &s.brim,
            object_height,
            ears,
        })
        .collect();
    let support_base: Vec<_> = supports
        .layers
        .iter()
        .filter(|p| (p.print_z - first_z).abs() < 1e-6)
        .flat_map(|p| p.regions.iter())
        .flat_map(|r| r.polygons.iter().cloned())
        .collect();
    let mut skirt_points: Vec<_> = outlines
        .iter()
        .zip(tops)
        .take_while(|(_, z)| **z <= skirt_z + 1e-6)
        .enumerate()
        .flat_map(|(i, (p, _))| if i == 0 { uncompensated } else { p.as_slice() })
        .flat_map(|p| p.contour.iter().copied())
        .collect();
    skirt_points.extend(
        supports
            .layers
            .iter()
            .filter(|p| p.print_z <= skirt_z + 1e-6)
            .flat_map(|p| p.regions.iter())
            .flat_map(|r| r.polygons.iter())
            .flat_map(|p| p.contour.iter().copied()),
    );
    generate(&AdhesionInput {
        islands: &islands,
        supports: &support_base,
        skirt_hulls: &[SkirtHull {
            object_id: 0,
            points: skirt_points,
        }],
        first_layer: if on_bed { uncompensated } else { &[] },
        settings: s,
        flow: Flow::new(
            s.extrusion_width,
            schedule
                .first()
                .map_or(settings.first_layer_height, |p| p.height),
        ),
        resolution: settings.gcode_resolution,
        tools: &[SkirtTool {
            id: 0,
            filament_diameter: settings.filament_diameter,
            extrusion_multiplier: settings.extrusion_multiplier,
        }],
        max_speed: settings
            .perimeter_speed
            .max(settings.external_perimeter_speed)
            .max(settings.infill_speed)
            .max(settings.solid_infill_speed)
            .max(settings.top_solid_infill_speed)
            .max(settings.support_material_speed),
        filament_type: &settings.filament_type,
    })
}

/// Prefix only object/raft layers, never intervening support-only layers. Single-tool integration;
/// a plate scheduler should call `paths_for_tool` with its actual layer tool order instead.
pub fn prepend(
    plan: &AdhesionPlan,
    schedule: &[PrintLayer],
    layer: &mut crate::toolpath::LayerToolpaths,
) {
    let is_skirt_layer = |p: &PrintLayer| {
        p.object.is_some()
            || p.print_z
                <= schedule
                    .iter()
                    .find(|p| p.object.is_some())
                    .map_or(f64::INFINITY, |p| p.print_z)
    };
    if !is_skirt_layer(&schedule[layer.index]) {
        return;
    }
    let previous = schedule[..layer.index].iter().rposition(is_skirt_layer);
    let height = layer.print_z - previous.map_or(0.0, |i| schedule[i].print_z);
    let index = schedule[..layer.index]
        .iter()
        .filter(|p| is_skirt_layer(p))
        .count();
    let mut paths = plan.paths_for_tool(index, height, &[0], 0, 0);
    for group in &mut layer.support_groups {
        group.start += paths.len();
        group.end += paths.len();
    }
    paths.append(&mut layer.paths);
    layer.paths = paths;
}
