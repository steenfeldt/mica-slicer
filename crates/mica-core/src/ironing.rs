// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/Fill/Fill.cpp :: Layer::make_ironing
// Source: orca@8500fcdcca src/libslic3r/Fill/Fill.cpp :: Layer::make_ironing
// Source: orca@8500fcdcca src/libslic3r/GCode.cpp :: GCode::process_layer, GCode::extrude_infill
// Changes: Retain one no-sort collection per ironing ExPolygon; Orca prints and chains these per island
// Changes: from the preceding extrusion endpoint. Prusa keeps the original flattened layer-end pass.
// Changes: Orca pattern, angle and inset resolved from retained options.

//! Iron solid faces with a small flow proportional to line spacing and nominal layer height.
use crate::{
    fill::{FillCollection, FillParams, Pattern},
    flow::Flow,
    geometry::clip::{Join, diff_ex, intersection_ex, offset_ex, union_ex},
    geometry::{ExPolygon, Point, SCALE},
    settings::PrintSettings,
    shells::FillSurfaces,
    toolpath::{ExtrusionPath, Role},
};
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum IroningType {
    #[default]
    Top,
    Topmost,
    Solid,
}
#[derive(Clone, Debug, PartialEq)]
pub struct IroningSettings {
    pub enabled: bool,
    pub kind: IroningType,
    pub flowrate: f64,
    pub spacing: f64,
    pub speed: f64,
    pub pattern: Pattern,
    pub angle: Option<f64>,
    pub inset: f64,
    pub fixed_angle: bool,
}
impl Default for IroningSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            kind: IroningType::Top,
            flowrate: 0.15,
            spacing: 0.1,
            speed: 15.,
            pattern: Pattern::Monotonic,
            angle: None,
            inset: 0.,
            fixed_angle: false,
        }
    }
}
impl IroningSettings {
    pub fn from_config(c: &crate::config::Config) -> Result<Self, crate::resolve::ResolveError> {
        let kind = match c.string("ironing_type").unwrap_or("top") {
            "top" => IroningType::Top,
            "topmost" => IroningType::Topmost,
            "solid" => IroningType::Solid,
            v => {
                return Err(crate::resolve::ResolveError(format!(
                    "unknown ironing_type {v}"
                )));
            }
        };
        let number = |key: &str, default: f64| -> Result<f64, crate::resolve::ResolveError> {
            crate::fill::config_number(c, key, default)
        };
        let pattern = crate::fill::config_pattern(c, "ironing_pattern", Pattern::Monotonic)?;
        if !matches!(
            pattern,
            Pattern::Rectilinear
                | Pattern::Monotonic
                | Pattern::MonotonicLines
                | Pattern::Concentric
        ) {
            return Err(crate::resolve::ResolveError(
                "ironing_pattern requires rectilinear, monotonic, monotoniclines or concentric"
                    .into(),
            ));
        }
        let angle = number("ironing_angle", -1.)?;
        let s = Self {
            enabled: c.bool("ironing").unwrap_or(false),
            kind,
            flowrate: c.float("ironing_flowrate").unwrap_or(15.) / 100.,
            spacing: c.float("ironing_spacing").unwrap_or(0.1),
            speed: c.float("ironing_speed").unwrap_or(15.),
            pattern: if pattern == Pattern::Rectilinear {
                Pattern::Monotonic
            } else {
                pattern
            },
            angle: if angle < 0. { None } else { Some(angle) },
            inset: number("ironing_inset", 0.)?,
            fixed_angle: number("ironing_angle_fixed", 0.)? != 0.,
        };
        if !s.flowrate.is_finite()
            || !(0. ..=1.).contains(&s.flowrate)
            || !s.spacing.is_finite()
            || s.spacing <= 0.
            || !s.speed.is_finite()
            || s.speed <= 0.
            || s.inset < 0.
        {
            return Err(crate::resolve::ResolveError(
                "ironing needs positive spacing/speed, nonnegative inset, and flowrate in 0..100%"
                    .into(),
            ));
        }
        Ok(s)
    }
}
#[allow(clippy::too_many_arguments)]
pub(crate) fn paths(
    outline: &[ExPolygon],
    upper: Option<&[ExPolygon]>,
    lower: Option<&[ExPolygon]>,
    surfaces: &FillSurfaces,
    index: usize,
    z: f64,
    origin: Point,
    s: &PrintSettings,
) -> Vec<ExtrusionPath> {
    collections(outline, upper, lower, surfaces, index, z, origin, s)
        .into_iter()
        .flat_map(|c| c.entities.into_iter().flatten())
        .collect()
}

/// Orca assigns a fill collection to the island containing its first point. `None` is the
/// final catch-all island. Chain whole ironing patches from the last extrusion, preserving
/// the monotonic order inside each patch, and carry its endpoint into the next island.
pub(crate) fn append_orca_island(
    collections: &mut Vec<FillCollection>,
    island: Option<&ExPolygon>,
    paths: &mut Vec<ExtrusionPath>,
    pos: &mut Point,
) {
    let here = collections
        .extract_if(.., |c| {
            island.is_none_or(|ex| crate::shells::contains(ex, c.entities[0][0].points[0]))
        })
        .map(|c| (c, false))
        .collect();
    for path in crate::extrusion_order::chain_orca_fills(here, *pos) {
        *pos = path.end_position().unwrap_or(*path.points.last().unwrap());
        paths.push(path);
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn collections(
    outline: &[ExPolygon],
    upper: Option<&[ExPolygon]>,
    lower: Option<&[ExPolygon]>,
    surfaces: &FillSurfaces,
    index: usize,
    z: f64,
    origin: Point,
    s: &PrintSettings,
) -> Vec<FillCollection> {
    let cfg = &s.ironing;
    if !cfg.enabled
        || cfg.flowrate <= 0.
        || (cfg.kind != IroningType::Solid
            && (s.top_solid_layers == 0 || (cfg.kind == IroningType::Topmost && upper.is_some())))
    {
        return Vec::new();
    }
    let mut area = if let Some(upper) = upper {
        diff_ex(outline, upper)
    } else {
        outline.to_vec()
    };
    if cfg.kind == IroningType::Solid {
        if (surfaces.internal.is_empty() || s.fill_density > 0.95)
            && surfaces.internal_bridge.is_empty()
            && surfaces.bridges.is_empty()
        {
            area = outline.to_vec();
        } else {
            let bottom = if let Some(lower) = lower {
                intersection_ex(
                    &diff_ex(outline, lower),
                    &offset_ex(lower, s.nozzle_diameter * SCALE / 2., Join::Miter),
                )
            } else {
                outline.to_vec()
            };
            area.extend(bottom);
            area.extend(surfaces.internal_solid.clone());
            area = union_ex(&area);
        }
    }
    let inset = if cfg.inset == 0. {
        s.nozzle_diameter / 2.
    } else {
        cfg.inset
    };
    let area = intersection_ex(&area, &offset_ex(outline, -inset * SCALE, Join::Miter));
    let height = s.layer_height * cfg.flowrate * cfg.spacing / s.nozzle_diameter;
    let flow = Flow::new(
        s.nozzle_diameter + height * (1. - std::f64::consts::PI / 4.),
        height,
    );
    let p = FillParams {
        spacing: cfg.spacing,
        density: 1.,
        angle: cfg.angle.unwrap_or(s.fill_angle + 45.),
        layer_id: if cfg.fixed_angle { 0 } else { index },
        z,
        anchor_length: 1000.,
        anchor_length_max: 1000.,
        dont_adjust: false,
        link_max_length: 3. * cfg.spacing,
        origin,
    };
    crate::fill::fill(cfg.pattern, &area, &p)
        .into_iter()
        .filter(|f| !f.polylines.is_empty())
        .map(|f| FillCollection {
            no_sort: true,
            multi: false,
            entities: f
                .polylines
                .into_iter()
                .map(|points| {
                    vec![ExtrusionPath {
                        role: Role::Ironing,
                        points,
                        flow,
                        seam: None,
                        overhang: None,
                        perimeter_index: None,
                        brick: None,
                    }]
                })
                .collect(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orca_chains_whole_ironing_patches_from_the_preceding_extrusion() {
        let patch = |x: f64| FillCollection {
            no_sort: true,
            multi: false,
            entities: [0., 1.]
                .into_iter()
                .map(|y| {
                    vec![ExtrusionPath {
                        role: Role::Ironing,
                        points: vec![Point::from_mm(x, y), Point::from_mm(x + 1., y)],
                        flow: Flow::new(0.4, 0.01),
                        seam: None,
                        overhang: None,
                        perimeter_index: None,
                        brick: None,
                    }]
                })
                .collect(),
        };
        let near = patch(90.);
        let far = patch(0.);
        let expected: Vec<_> = near
            .entities
            .iter()
            .chain(&far.entities)
            .flatten()
            .cloned()
            .collect();
        let mut collections = vec![far, near];
        let mut paths = vec![];
        let mut pos = Point::from_mm(100., 0.);
        append_orca_island(&mut collections, None, &mut paths, &mut pos);
        assert_eq!(paths, expected);
        assert_eq!(pos, *expected.last().unwrap().points.last().unwrap());
        assert!(collections.is_empty());
    }
}
