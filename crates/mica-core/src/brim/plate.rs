// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/Print.cpp :: Print::_make_skirt, Print::first_layer_islands, Print::finalize_first_layer_convex_hull
// Source: prusaslicer@b028299c77 src/libslic3r/GCode.cpp :: Skirt::make_skirt_loops_per_extruder_1st_layer
// Source: orca@8500fcdcca src/libslic3r/Print.cpp :: Print::_make_skirt
// Changes: Gather placed parts and instances into one borrowed adhesion input, shared prepared objects at each instance's shift; retain compact layer eligibility and Orca brim ownership.

use super::*;
use crate::{
    config::Config, print::PreparedObject, print_job::PrintObject, settings::PrintSettings,
    tool_ordering::ToolOrdering, wipe_tower::TowerPlan,
};

pub(crate) fn skirt_layer(object: &PreparedObject, index: usize) -> bool {
    let p = &object.print_layers[index];
    p.object.is_some() || p.print_z <= object.tops.first().copied().unwrap_or(f64::INFINITY)
}

/// Model first-layer bounds, before skirt/brim/tower expansion (Orca's model-only center).
pub(crate) fn object_bbox(objects: &[PrintObject]) -> Option<BoundingBox> {
    let shifted: Vec<_> = objects
        .iter()
        .flat_map(|o| {
            BoundingBox::from_points(
                o.prepared
                    .uncompensated_first_layer
                    .iter()
                    .flat_map(|p| &p.contour),
            )
            .map(|b| {
                let s = o.shift;
                [
                    Point::new(b.min.x + s.x, b.min.y + s.y),
                    Point::new(b.max.x + s.x, b.max.y + s.y),
                ]
            })
        })
        .flatten()
        .collect();
    BoundingBox::from_points(&shifted)
}

pub(crate) fn generate_plate(
    objects: &[PrintObject],
    config: &Config,
    ordering: &ToolOrdering,
    tower: Option<&TowerPlan>,
) -> Result<AdhesionPlan, String> {
    let settings =
        PrintSettings::from_config(&crate::print_job::tool_config(config, ordering.first))
            .map_err(|e| e.to_string())?;
    let s = &settings.adhesion;
    let skirt_z = objects
        .iter()
        .map(|o| {
            let tops = &o.prepared.tops;
            tops.get(s.skirt_layers(tops.len()).saturating_sub(1))
                .copied()
                .unwrap_or(0.)
        })
        .fold(0., f64::max);
    let first_z = ordering
        .layers
        .first()
        .map_or(settings.first_layer_height, |l| l.print_z);
    let mut islands = Vec::new();
    let mut first_layer = Vec::new();
    let mut supports = Vec::new();
    let mut hulls = Vec::new();
    // Instances sharing a prepared object contribute its first layer at their own shift.
    let shifted = |o: &PrintObject, polygons: &[ExPolygon]| {
        crate::instances::shifted_polygons(polygons, o.shift)
    };
    let placed: Vec<_> = objects
        .iter()
        .map(|o| {
            let p = &o.prepared;
            (
                shifted(o, &p.uncompensated_first_layer),
                shifted(o, p.outlines.first().map_or(&[][..], Vec::as_slice)),
            )
        })
        .collect();
    let ears: Vec<Vec<BrimEar>> = objects
        .iter()
        .map(|o| {
            let (dx, dy) = (
                o.shift.x as f64 / crate::geometry::SCALE,
                o.shift.y as f64 / crate::geometry::SCALE,
            );
            o.prepared
                .brim_ears
                .iter()
                .map(|e| BrimEar {
                    x: e.x + dx,
                    y: e.y + dy,
                    ..*e
                })
                .collect()
        })
        .collect();
    // Parts of an instance share brim ownership. A per-object skirt covers all of its copies.
    let mut identities = std::collections::BTreeMap::new();
    for (oi, o) in objects.iter().enumerate() {
        let (uncompensated_first_layer, first_outlines) = &placed[oi];
        let id = *identities
            .entry((o.object_id, o.instance_id))
            .or_insert(oi as u64);
        let p = &o.prepared;
        let on_bed = p.tops.first().is_some_and(|z| (*z - first_z).abs() < 1e-4);
        let base = if !on_bed {
            &[][..]
        } else if s.orca && !p.settings.adhesion.brim_efc {
            uncompensated_first_layer.as_slice()
        } else {
            first_outlines.as_slice()
        };
        let (min, max) = o.mesh.bounds();
        islands.extend(base.iter().map(|polygon| FirstLayerIsland {
            object_id: id,
            polygon,
            brim: &p.settings.adhesion.brim,
            object_height: f64::from(max[2] - min[2]),
            ears: &ears[oi],
        }));
        if on_bed {
            first_layer.extend_from_slice(uncompensated_first_layer);
        }
        supports.extend(
            p.supports
                .layers
                .iter()
                .filter(|l| (l.print_z - first_z).abs() < 1e-4)
                .flat_map(|l| &l.regions)
                .flat_map(|r| shifted(o, &r.polygons)),
        );
        let mut points: Vec<_> = p
            .outlines
            .iter()
            .zip(&p.tops)
            .take_while(|(_, z)| **z <= skirt_z + 1e-4)
            .enumerate()
            .flat_map(|(i, (outlines, _))| {
                if i == 0 {
                    &p.uncompensated_first_layer
                } else {
                    outlines
                }
            })
            .flat_map(|p| p.contour.iter().copied())
            .collect();
        points.extend(
            p.supports
                .layers
                .iter()
                .filter(|l| l.print_z <= skirt_z + 1e-4)
                .flat_map(|l| &l.regions)
                .flat_map(|r| &r.polygons)
                .flat_map(|p| p.contour.iter().copied()),
        );
        for point in &mut points {
            point.x += o.shift.x;
            point.y += o.shift.y;
        }
        hulls.push(SkirtHull {
            object_id: o.object_id as u64,
            points,
        });
    }
    let tower_points = tower.map_or_else(Vec::new, TowerPlan::first_layer_hull);
    if !tower_points.is_empty() {
        hulls.push(SkirtHull {
            object_id: u64::MAX,
            points: tower_points.clone(),
        });
    }
    let tools: Vec<_> = ordering
        .used
        .iter()
        .map(|&id| SkirtTool {
            id,
            filament_diameter: config.float_at("filament_diameter", id).unwrap_or(1.75),
            extrusion_multiplier: config.float_at("extrusion_multiplier", id).unwrap_or(1.),
        })
        .collect();
    let mut plan = generate(&AdhesionInput {
        islands: &islands,
        supports: &supports,
        skirt_hulls: &hulls,
        first_layer: &first_layer,
        settings: s,
        flow: Flow::new(s.extrusion_width, first_z),
        resolution: settings.gcode_resolution,
        tools: &tools,
        max_speed: settings
            .perimeter_speed
            .max(settings.external_perimeter_speed)
            .max(settings.infill_speed)
            .max(settings.solid_infill_speed)
            .max(settings.top_solid_infill_speed)
            .max(settings.support_material_speed),
        filament_type: &settings.filament_type,
    });
    plan.first_layer_hull.extend(tower_points);
    plan.first_layer_hull = convex_hull(plan.first_layer_hull);
    Ok(plan)
}
