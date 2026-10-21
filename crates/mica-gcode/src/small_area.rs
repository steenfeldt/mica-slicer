// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: orca@8500fcdcca src/libslic3r/GCode.cpp :: GCode::_needSAFC, GCode::_extrude
// Source: orca@8500fcdcca src/libslic3r/GCode/SmallAreaInfillFlowCompensator.cpp :: modify_flow
// Changes: Mica role/pattern eligibility; apply one multiplier per fitted source line/arc before cooling subdivision; writer retains actual extrusion accounting.

use crate::{
    smooth_path::{self, ArcFitting},
    writer::Writer,
};
use mica_core::{
    fill::Pattern,
    geometry::{SCALE, arc_welder as arc},
    settings::PrintSettings,
    small_features::FlowCompensation,
    toolpath::Role,
};

pub(crate) fn model(s: &PrintSettings, role: Role, first: bool) -> Option<&FlowCompensation> {
    if !matches!(role, Role::SolidInfill | Role::TopSolidInfill) {
        return None;
    }
    let supported = |p| {
        matches!(
            p,
            Pattern::Rectilinear
                | Pattern::AlignedRectilinear
                | Pattern::Monotonic
                | Pattern::MonotonicLines
        )
    };
    ((first && supported(s.bottom_fill_pattern))
        || (role == Role::SolidInfill && supported(s.internal_solid_fill_pattern))
        || (role == Role::TopSolidInfill && supported(s.top_fill_pattern)))
    .then_some(s.small_features.flow_compensation.as_ref())
    .flatten()
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn emit(
    w: &mut Writer,
    curve: &arc::Path,
    model: &FlowCompensation,
    e_per_mm: f64,
    speed: f64,
    tail: f64,
    original_speed: f64,
    mode: ArcFitting,
) {
    let split = arc::path_length(curve) / SCALE - tail;
    let mut elapsed = 0.;
    for edge in curve.windows(2) {
        let length = arc::segment_length(&edge[0], &edge[1]) / SCALE;
        let flow = e_per_mm * model.multiplier(length);
        if tail > 0. && split > elapsed + 0.01 && split < elapsed + length - 0.01 {
            let mut head = edge.to_vec();
            let mut end = edge.to_vec();
            let cut = (split - elapsed) * SCALE;
            arc::clip_end(&mut head, length * SCALE - cut);
            arc::clip_start(&mut end, cut);
            smooth_path::emit(w, &head, flow, speed, mode);
            smooth_path::emit(w, &end, flow, original_speed, mode);
        } else {
            smooth_path::emit(
                w,
                edge,
                flow,
                if tail > 0. && elapsed >= split - 0.01 {
                    original_speed
                } else {
                    speed
                },
                mode,
            );
        }
        elapsed += length;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mica_core::{config::Config, geometry::Point};
    #[test]
    fn source_length_survives_cooling_split_and_accounts_for_material() {
        let model = FlowCompensation::new(vec![(0., 0.), (10., 1.)]).unwrap();
        let curve = arc::fit_path(&[Point::from_mm(0., 0.), Point::from_mm(4., 0.)], 0., 0.);
        for tail in [0., 1., 3.] {
            let mut w = Writer::new();
            w.travel_to(0., 0., 100.);
            emit(
                &mut w,
                &curve,
                &model,
                0.1,
                20.,
                tail,
                40.,
                ArcFitting::Disabled,
            );
            assert!((w.stats.filament_mm - 0.16).abs() < 1e-10);
        }
    }
    #[test]
    fn fitted_arcs_keep_one_flow_factor_when_cooling_subdivides_them() {
        let model = FlowCompensation::new(vec![(0., 0.), (10., 1.)]).unwrap();
        let curve = vec![
            arc::Segment::line(Point::from_mm(1., 0.)),
            arc::Segment {
                point: Point::from_mm(-1., 0.),
                radius: SCALE,
                ccw: true,
            },
        ];
        for tail in [0., 1.] {
            let mut w = Writer::new();
            w.travel_to(1., 0., 100.);
            emit(
                &mut w,
                &curve,
                &model,
                0.1,
                20.,
                tail,
                40.,
                ArcFitting::EmitCenter,
            );
            assert!(w.as_str().contains("G3"));
            assert!((w.stats.filament_mm - std::f64::consts::PI.powi(2) / 100.).abs() < 0.0001);
        }
    }

    #[test]
    fn only_eligible_solid_patterns_are_modified() {
        let mut c = Config::defaults();
        c.set_raw("small_area_infill_flow_compensation", "1")
            .unwrap();
        let mut s = PrintSettings::from_config(&c).unwrap();
        s.internal_solid_fill_pattern = Pattern::Rectilinear;
        assert!(model(&s, Role::SolidInfill, false).is_some());
        for role in [
            Role::InternalInfill,
            Role::BridgeInfill,
            Role::GapFill,
            Role::Perimeter,
            Role::Ironing,
        ] {
            assert!(model(&s, role, true).is_none());
        }
        s.top_fill_pattern = Pattern::Concentric;
        assert!(model(&s, Role::TopSolidInfill, false).is_none());
    }
}
