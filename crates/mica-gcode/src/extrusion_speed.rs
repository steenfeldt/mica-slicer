// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/GCode.cpp :: GCodeGenerator::extrude_perimeters, GCodeGenerator::_extrude, cap_speed
// Source: orca@8500fcdcca src/libslic3r/GCode.cpp :: GCode::_extrude
// Source: orca@8500fcdcca src/libslic3r/Flow.cpp :: Flow::mm3_per_mm
// Changes: Orca volumetric caps use float32 path volume and the filament flow ratio, also used by role-based wipe speed. Independent Orca internal bridge speed, resolved against external bridge speed; first-layer non-wall roles use infill speed.
// Changes: Existing Mica roles, first-layer infill override, small perimeters and positive-only volumetric caps.
// Changes: Engine choices read mica_slicing_rules (mica_core::rules), not gcode_flavor or the presence of Orca keys (docs/slicing-rules.md).
use mica_core::{
    config::Config,
    geometry::{SCALE, polyline_length},
    settings::PrintSettings,
    toolpath::{ExtrusionPath, Role},
};

pub(crate) fn speed(c: &Config, s: &PrintSettings, path: &ExtrusionPath, first: bool) -> f64 {
    cap(c, s, path, nominal_speed(c, s, path, first))
}

pub(crate) fn nominal_speed(
    c: &Config,
    s: &PrintSettings,
    path: &ExtrusionPath,
    first: bool,
) -> f64 {
    let mut speed = match path.role {
        Role::Skirt if s.adhesion.orca && s.adhesion.skirt_speed > 0.0 => s.adhesion.skirt_speed,
        Role::Skirt | Role::Brim => s.support_material_speed,
        Role::ExternalPerimeter => s.external_perimeter_speed,
        Role::Perimeter => s.perimeter_speed,
        Role::InternalInfill => s.infill_speed,
        Role::SolidInfill => s.solid_infill_speed,
        Role::TopSolidInfill => s.top_solid_infill_speed,
        Role::Ironing => s.ironing.speed,
        Role::GapFill => s.gap_fill_speed,
        Role::InternalBridgeInfill => c
            .abs_value("internal_bridge_speed", s.bridge_speed)
            .unwrap_or(s.bridge_speed),
        Role::OverhangPerimeter | Role::BridgeInfill => s.bridge_speed,
        Role::SupportMaterial => s.support_material_speed,
        Role::SupportMaterialInterface => s.support_material_interface_speed,
    };
    let threshold = s.small_features.small_perimeter_threshold;
    if threshold > 0. && matches!(path.role, Role::ExternalPerimeter | Role::Perimeter) {
        let orca = mica_core::rules::Rules::of(c).is_orca();
        let configured = orca || threshold != 6.5;
        let loop_length = path.seam.as_ref().and_then(|s| s.loop_length_mm);
        let length = if configured {
            loop_length.unwrap_or_else(|| polyline_length(&path.points) / SCALE)
        } else {
            polyline_length(&path.points) / SCALE
        };
        let is_loop =
            !configured || loop_length.is_some() || path.points.first() == path.points.last();
        if is_loop && length <= threshold * std::f64::consts::TAU {
            let base = if orca {
                s.external_perimeter_speed
            } else {
                s.perimeter_speed
            };
            speed = match c.abs_value("small_perimeter_speed", base) {
                Some(0.) if orca => 0.5 * base,
                Some(v) if v > 0. => v,
                _ => speed,
            };
        }
    }
    if first && !(path.role == Role::Skirt && s.adhesion.orca && s.adhesion.skirt_speed > 0.0) {
        let infill = c
            .abs_value("first_layer_infill_speed", speed)
            .unwrap_or(0.0);
        let orca_infill = mica_core::rules::Rules::of(c).is_orca()
            && !matches!(
                path.role,
                Role::Perimeter | Role::ExternalPerimeter | Role::OverhangPerimeter
            );
        speed = if (path.role == Role::SolidInfill || orca_infill) && infill > 0.0 {
            infill
        } else {
            c.abs_value("first_layer_speed", speed)
                .unwrap_or(s.first_layer_speed)
        };
    }
    speed
}

pub(crate) fn cap(c: &Config, s: &PrintSettings, path: &ExtrusionPath, mut speed: f64) -> f64 {
    let flow = if mica_core::rules::Rules::of(c).is_orca() {
        // Orca's Flow::mm3_per_mm returns a float32 result through its double API.
        // Wipe::calculateWipeRetractionLengths uses the resulting pre-cooling speed.
        f64::from(path.mm3_per_mm() as f32) * s.extrusion_multiplier
    } else {
        path.mm3_per_mm()
    };
    for cap in [
        c.float("max_volumetric_speed").unwrap_or(0.0),
        s.filament_max_volumetric_speed,
    ] {
        if cap > 0.0 && flow > 0.0 {
            speed = speed.min(cap / flow);
        }
    }
    if path.role == Role::InternalInfill {
        use mica_core::fill::Pattern;
        let crossing = matches!(
            s.fill_pattern,
            Pattern::Grid
                | Pattern::Triangles
                | Pattern::Stars
                | Pattern::Cubic
                | Pattern::AdaptiveCubic
                | Pattern::SupportCubic
        );
        let key = if crossing {
            "filament_infill_max_crossing_speed"
        } else {
            "filament_infill_max_speed"
        };
        if let Some(limit) = c.float(key).filter(|v| *v > 0.) {
            speed = speed.min(limit);
        }
    }
    speed
}

#[cfg(test)]
mod tests {
    use super::*;
    use mica_core::{flow::Flow, geometry::Point};
    #[test]
    fn orca_radius_limit_zero_auto_and_percentage_speed() {
        let mut c = Config::defaults();
        mica_core::rules::Rules::OrcaSlicer.set(&mut c);
        c.set_raw("outer_wall_speed", "100").unwrap();
        c.set_raw("small_perimeter_speed", "50%").unwrap();
        let mut s = PrintSettings::core_one_hf04_speed_pla();
        s.external_perimeter_speed = 100.;
        s.filament_max_volumetric_speed = 0.;
        let mut path = ExtrusionPath {
            role: Role::Perimeter,
            points: vec![Point::from_mm(0., 0.), Point::from_mm(1., 0.)],
            flow: Flow::new(0.45, 0.2),
            overhang: None,
            perimeter_index: Some(1),
            seam: Some(Box::new(mica_core::seam::PathSeam {
                loop_length_mm: Some(20.),
                ..Default::default()
            })),
            brick: None,
        };
        s.small_features.small_perimeter_threshold = 3.;
        assert_eq!(nominal_speed(&c, &s, &path, false), s.perimeter_speed);
        s.small_features.small_perimeter_threshold = 4.;
        assert_eq!(nominal_speed(&c, &s, &path, false), 50.);
        c.set_raw("small_perimeter_speed", "0").unwrap();
        assert_eq!(nominal_speed(&c, &s, &path, false), 50.);
        c.set_raw("small_perimeter_speed", "-1").unwrap();
        assert_eq!(nominal_speed(&c, &s, &path, false), s.perimeter_speed);
        s.small_features.small_perimeter_threshold = 0.;
        assert_eq!(nominal_speed(&c, &s, &path, false), s.perimeter_speed);
        c.set_raw("small_perimeter_speed", "50%").unwrap();
        path.seam = None;
        s.small_features.small_perimeter_threshold = 4.;
        assert_eq!(nominal_speed(&c, &s, &path, false), s.perimeter_speed);
        path.role = Role::BridgeInfill;
        s.small_features.small_perimeter_threshold = 10.;
        assert_eq!(nominal_speed(&c, &s, &path, false), s.bridge_speed);
    }

    #[test]
    fn orca_volumetric_cap_uses_flow_ratio_for_extrusion_and_wipe_speed() {
        let mut c = Config::defaults();
        let s = PrintSettings {
            extrusion_multiplier: 0.98,
            filament_max_volumetric_speed: 15.,
            ..PrintSettings::core_one_hf04_speed_pla()
        };
        let path = ExtrusionPath {
            points: vec![Point::from_mm(0., 0.), Point::from_mm(10., 0.)],
            flow: Flow::new(0.45, 0.2),
            role: Role::Perimeter,
            overhang: None,
            perimeter_index: Some(1),
            seam: None,
            brick: None,
        };
        let prusa = cap(&c, &s, &path, 300.);
        assert_eq!(prusa, 15. / path.mm3_per_mm());
        mica_core::rules::Rules::OrcaSlicer.set(&mut c);
        let orca = cap(&c, &s, &path, 300.);
        assert_eq!(orca, 15. / (f64::from(path.mm3_per_mm() as f32) * 0.98));
    }

    #[test]
    fn first_layer_infill_small_perimeters_and_zero_cap() {
        let mut c = Config::defaults();
        c.set_raw("first_layer_infill_speed", "100").unwrap();
        c.set_raw("first_layer_speed", "45").unwrap();
        c.set_raw("small_perimeter_speed", "50%").unwrap();
        let s = PrintSettings {
            filament_max_volumetric_speed: 0.0,
            ..PrintSettings::core_one_hf04_speed_pla()
        };
        let mut path = ExtrusionPath {
            role: Role::SolidInfill,
            points: vec![Point::from_mm(0., 0.), Point::from_mm(10., 0.)],
            flow: Flow::new(0.45, 0.2),
            overhang: None,
            perimeter_index: None,
            seam: None,
            brick: None,
        };
        assert_eq!(speed(&c, &s, &path, true), 100.);
        path.role = Role::Perimeter;
        assert_eq!(speed(&c, &s, &path, true), 45.);
        assert_eq!(speed(&c, &s, &path, false), s.perimeter_speed * 0.5);
    }
}
