// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: orca@8500fcdcca src/libslic3r/PrintObjectSlice.cpp :: PrintObject::_shrink_contour_holes, PrintObject::slice
// Source: orca@8500fcdcca src/libslic3r/GCode/SmallAreaInfillFlowCompensator.cpp :: SmallAreaInfillFlowCompensator, flow_comp_model, modify_flow
// Source: orca@8500fcdcca src/libslic3r/GCode/PchipInterpolatorHelper.cpp :: computePCHIP, interpolate
// Source: orca@8500fcdcca src/libslic3r/PrintConfig.cpp :: PrintConfigDef::init_fff_params
// Changes: Typed optional settings, Clipper2 geometry, layer-local region redistribution and validated Rust PCHIP model.
// Attribution: Small-area flow compensation is based on Alexander Þór's GPLv3 Small-Area-Flow-Comp, as credited by Orca.

//! Orca's independent dimension controls and opt-in small-feature treatment.
use crate::{
    config::{Config, ConfigValue, Value},
    geometry::{
        ExPolygon, SCALE,
        clip::{Join, diff_ex, intersection_ex, offset_ex, union_ex},
        signed_area,
    },
    regions::LayerRegion,
    resolve::ResolveError,
    settings::FloatOrPercent,
};

#[derive(Clone, Debug, PartialEq)]
pub struct SmallFeatureSettings {
    pub xy_hole_compensation: f64,
    pub xy_contour_compensation: f64,
    /// Radius in mm: the loop length limit is 2π times this value. Zero disables.
    pub small_perimeter_threshold: f64,
    /// None preserves Prusa's shared bead minimum; percent is of nozzle diameter.
    pub initial_layer_min_bead_width: Option<FloatOrPercent>,
    /// Minimum continuous thick polyline length in mm, before splitting into width steps.
    pub filter_out_gap_fill: f64,
    pub flow_compensation: Option<FlowCompensation>,
}
impl Default for SmallFeatureSettings {
    fn default() -> Self {
        Self {
            xy_hole_compensation: 0.,
            xy_contour_compensation: 0.,
            small_perimeter_threshold: 6.5,
            initial_layer_min_bead_width: None,
            filter_out_gap_fill: 0.,
            flow_compensation: None,
        }
    }
}
impl SmallFeatureSettings {
    pub fn from_config(c: &Config) -> Result<Self, ResolveError> {
        let number = |key: &str, default: f64, nonnegative: bool| {
            let n = c.float(key).unwrap_or(default);
            if !n.is_finite() || (nonnegative && n < 0.) {
                Err(ResolveError(format!(
                    "invalid {key}: expected a finite {}number",
                    if nonnegative { "nonnegative " } else { "" }
                )))
            } else {
                Ok(n)
            }
        };
        let bead = number("initial_layer_min_bead_width", 0., true)?;
        let flow_compensation = if c
            .bool("small_area_infill_flow_compensation")
            .unwrap_or(false)
        {
            let Some(ConfigValue::Vector(values)) =
                c.get("small_area_infill_flow_compensation_model")
            else {
                return Err(ResolveError(
                    "missing small_area_infill_flow_compensation_model".into(),
                ));
            };
            let mut points = Vec::new();
            for value in values {
                let Some(Value::String(line)) = value else {
                    return Err(ResolveError("invalid flow model point".into()));
                };
                // Orca JSON stores each point as a string, sometimes with a leading newline.
                for line in line.lines().filter(|s| !s.trim().is_empty()) {
                    let (x, y) = line.split_once(',').ok_or_else(|| {
                        ResolveError("flow model requires length,multiplier pairs".into())
                    })?;
                    let parse = |v: &str| {
                        v.trim()
                            .parse::<f64>()
                            .map_err(|_| ResolveError("invalid flow model number".into()))
                    };
                    points.push((parse(x)?, parse(y)?));
                }
            }
            Some(FlowCompensation::new(points).map_err(ResolveError)?)
        } else {
            None
        };
        Ok(Self {
            xy_hole_compensation: number("xy_hole_compensation", 0., false)?,
            xy_contour_compensation: number("xy_contour_compensation", 0., false)?,
            small_perimeter_threshold: number("small_perimeter_threshold", 6.5, true)?,
            initial_layer_min_bead_width: (bead > 0.).then_some(FloatOrPercent::Percent(bead)),
            filter_out_gap_fill: number("filter_out_gap_fill", 0., true)?,
            flow_compensation,
        })
    }
    pub fn has_xy_compensation(&self) -> bool {
        self.xy_hole_compensation != 0. || self.xy_contour_compensation != 0.
    }
}

/// Positive hole compensation expands the hole; positive contour compensation grows material.
/// Offsets are applied in two passes like Orca: material growth, then material removal.
pub fn compensate(polys: &[ExPolygon], contour: f64, hole: f64) -> Vec<ExPolygon> {
    if contour == 0. && hole == 0. {
        return polys.to_vec();
    }
    let pass = |polys: &[ExPolygon], contour: f64, hole: f64| {
        let mut result = Vec::new();
        for poly in polys {
            let outer = offset_ex(
                &[ExPolygon {
                    contour: poly.contour.clone(),
                    holes: vec![],
                }],
                contour * SCALE,
                Join::Miter,
            );
            let holes: Vec<_> = poly
                .holes
                .iter()
                .flat_map(|p| {
                    let mut p = p.clone();
                    if signed_area(&p) < 0. {
                        p.reverse();
                    }
                    offset_ex(
                        &[ExPolygon {
                            contour: p,
                            holes: vec![],
                        }],
                        hole * SCALE,
                        Join::Miter,
                    )
                })
                .collect();
            result.extend(diff_ex(&outer, &union_ex(&holes)));
        }
        union_ex(&result)
    };
    // Orca offsets clockwise holes by -hole_delta, which expands them for positive values.
    // Clipper2 needs redundant slice-closing vertices within 5 microns removed before miter
    // offsets; otherwise nearly reversed nanometre edges can grow into visible spikes.
    // This cleanup only runs for the explicitly enabled Orca dimension options.
    let clean = clean_dimensions(polys);
    let grown = pass(&clean, contour.max(0.), hole.max(0.));
    clean_dimensions(&pass(&grown, contour.min(0.), hole.min(0.)))
}

fn clean_dimensions(polys: &[ExPolygon]) -> Vec<ExPolygon> {
    let ring = |p: &crate::geometry::Polygon| {
        if p.len() < 3 {
            return p.clone();
        }
        let mut closed = p.clone();
        closed.push(p[0]);
        let mut simplified = crate::geometry::simplify::douglas_peucker(&closed, 0.005 * SCALE);
        simplified.pop();
        simplified
    };
    polys
        .iter()
        .filter_map(|p| {
            let contour = ring(&p.contour);
            (contour.len() >= 3).then(|| ExPolygon {
                contour,
                holes: p.holes.iter().map(ring).filter(|h| h.len() >= 3).collect(),
            })
        })
        .collect()
}

pub(crate) fn compensate_layer(
    outline: &mut Vec<ExPolygon>,
    regions: Option<&mut [LayerRegion]>,
    s: &SmallFeatureSettings,
) {
    *outline = compensate(outline, s.xy_contour_compensation, s.xy_hole_compensation);
    if let Some(regions) = regions {
        let growth = s
            .xy_contour_compensation
            .max(-s.xy_hole_compensation)
            .max(0.);
        let mut covered = Vec::new();
        for region in regions {
            let grown = if growth > 0. {
                offset_ex(&region.slices, growth * SCALE, Join::Miter)
            } else {
                region.slices.clone()
            };
            region.slices = diff_ex(&intersection_ex(&grown, outline), &covered);
            covered.extend(region.slices.clone());
        }
    }
}

/// Orca's PCHIP uses secant slopes at the endpoints (not the alternate SciPy endpoint rule).
#[derive(Clone, Debug, PartialEq)]
pub struct FlowCompensation {
    points: Vec<(f64, f64)>,
    slopes: Vec<f64>,
}
impl FlowCompensation {
    pub fn new(points: Vec<(f64, f64)>) -> Result<Self, String> {
        let invalid = || {
            "small_area_infill_flow_compensation_model: need at least two finite, strictly increasing length/flow pairs, starting at length 0 and ending at flow 1".to_string()
        };
        if points.len() < 2
            || points[0].0 != 0.
            || points.last().unwrap().1 != 1.
            || points
                .iter()
                .any(|(x, y)| !x.is_finite() || !y.is_finite() || *x < 0. || *y < 0.)
            || points
                .windows(2)
                .any(|p| p[1].0 <= p[0].0 || p[1].1 <= p[0].1)
        {
            return Err(invalid());
        }
        let h: Vec<_> = points.windows(2).map(|p| p[1].0 - p[0].0).collect();
        let delta: Vec<_> = points
            .windows(2)
            .zip(&h)
            .map(|(p, h)| (p[1].1 - p[0].1) / h)
            .collect();
        let mut slopes = vec![delta[0]];
        for i in 1..points.len() - 1 {
            let w1 = 2. * h[i] + h[i - 1];
            let w2 = h[i] + 2. * h[i - 1];
            slopes.push((w1 + w2) / (w1 / delta[i - 1] + w2 / delta[i]));
        }
        slopes.push(*delta.last().unwrap());
        Ok(Self { points, slopes })
    }
    pub fn multiplier(&self, length: f64) -> f64 {
        if !length.is_finite() || length <= 0. || length > self.points.last().unwrap().0 {
            return 1.;
        }
        let i = self
            .points
            .partition_point(|p| p.0 < length)
            .saturating_sub(1);
        let (x, y) = self.points[i];
        let h = self.points[i + 1].0 - x;
        let t = (length - x) / h;
        let t2 = t * t;
        let t3 = t2 * t;
        (2. * t3 - 3. * t2 + 1.) * y
            + (t3 - 2. * t2 + t) * h * self.slopes[i]
            + (-2. * t3 + 3. * t2) * self.points[i + 1].1
            + (t3 - t2) * h * self.slopes[i + 1]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::{BoundingBox, Point};
    fn rect(a: f64, b: f64, c: f64, d: f64) -> Vec<Point> {
        vec![
            Point::from_mm(a, b),
            Point::from_mm(c, b),
            Point::from_mm(c, d),
            Point::from_mm(a, d),
        ]
    }
    #[test]
    fn independent_signed_offsets_preserve_topology() {
        let mut hole = rect(5., 5., 8., 8.);
        hole.reverse();
        let original = vec![ExPolygon {
            contour: rect(0., 0., 20., 20.),
            holes: vec![hole],
        }];
        assert_eq!(compensate(&original, 0., 0.), original);
        for (contour, hole) in [(0.2, 0.1), (-0.2, -0.1), (0.2, -0.1), (-0.2, 0.1)] {
            let p = compensate(&original, contour, hole);
            assert_eq!(p.len(), 1);
            assert_eq!(p[0].holes.len(), 1);
            let outer = BoundingBox::from_points(p[0].contour.iter()).unwrap();
            let inner = BoundingBox::from_points(p[0].holes[0].iter()).unwrap();
            assert!((outer.min.x as f64 / SCALE + contour).abs() < 1e-5);
            assert!((inner.min.x as f64 / SCALE - (5. - hole)).abs() < 1e-5);
        }
        assert!(compensate(&original, -11., 0.).is_empty());
        assert!(compensate(&original, 0., -2.)[0].holes.is_empty());
    }
    #[test]
    fn circular_holes_remain_dimensionally_stable_through_wall_offsets() {
        use crate::geometry::clip::closing_ex;
        for diameter in [3., 5., 8.] {
            let mut hole = Vec::new();
            for i in 0..192 {
                let angle = std::f64::consts::TAU * i as f64 / 192.;
                let next = angle + std::f64::consts::TAU / 192.;
                let vertex = |a: f64| {
                    (
                        (119.5 + diameter / 2. * a.cos()) as f32 as f64,
                        (136. + diameter / 2. * a.sin()) as f32 as f64,
                    )
                };
                let (x, y) = vertex(angle);
                let (u, v) = vertex(next);
                hole.push(Point::from_mm(x, y));
                hole.push(Point::from_mm(
                    x + (u - x) * 0.95 / 3.,
                    y + (v - y) * 0.95 / 3.,
                ));
            }
            hole.reverse();
            let outline = closing_ex(
                &[ExPolygon {
                    contour: rect(110., 126., 130., 146.),
                    holes: vec![hole],
                }],
                0.049 * SCALE,
            );
            let compensated = compensate(&outline, -0.05, 0.1);
            let walls = offset_ex(&compensated, -0.21 * SCALE, Join::Miter);
            assert_eq!(walls[0].holes.len(), 1);
            let bb = BoundingBox::from_points(walls[0].holes[0].iter()).unwrap();
            for extent in [bb.max.x - bb.min.x, bb.max.y - bb.min.y] {
                assert!((extent as f64 / SCALE - 0.42 - (diameter + 0.2)).abs() < 0.02);
            }
        }
    }

    #[test]
    fn resolve_neutral_defaults_and_orca_model_validation() {
        let nonlinear = FlowCompensation::new(vec![(0., 0.), (1., 0.75), (3., 1.)]).unwrap();
        assert!((nonlinear.multiplier(0.5) - 51. / 116.).abs() < 1e-12);
        let mut c = Config::defaults();
        assert_eq!(
            SmallFeatureSettings::from_config(&c).unwrap(),
            SmallFeatureSettings::default()
        );
        c.set_raw("small_area_infill_flow_compensation", "1")
            .unwrap();
        let s = SmallFeatureSettings::from_config(&c).unwrap();
        let m = s.flow_compensation.unwrap();
        assert_eq!(m.multiplier(0.), 1.);
        assert_eq!(m.multiplier(11.), 1.);
        assert!((m.multiplier(0.2) - 0.4444).abs() < 1e-12);
        assert!((m.multiplier(10.) - 1.).abs() < 1e-12);
        for bad in [
            "1,0;10,1",
            "0,0;0,1",
            "0,1;10,0",
            "0,0;10,0.9",
            "0,0;NaN,1",
            "0,0",
        ] {
            c.set_raw("small_area_infill_flow_compensation_model", bad)
                .unwrap();
            assert!(SmallFeatureSettings::from_config(&c).is_err(), "{bad}");
        }
        assert!(
            (FlowCompensation::new(vec![(0., 0.), (1., 0.5), (2., 1.)])
                .unwrap()
                .multiplier(0.5)
                - 0.25)
                .abs()
                < 1e-12
        );
    }
}
