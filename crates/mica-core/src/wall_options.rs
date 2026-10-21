// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: orca@8500fcdcca src/libslic3r/PerimeterGenerator.cpp :: detect_steep_overhang, reorient_perimeters, PerimeterGenerator::process_classic, PerimeterGenerator::process_arachne
// Source: orca@8500fcdcca src/libslic3r/LayerRegion.cpp :: LayerRegion::bridging_flow
// Changes: Resolve optional controls once (incl. bridge_line_width for classic overhang flow); classify closed wall runs against a borrowed lower layer, with separate contour/hole flags; preserve Mica seam placement.
// Changes: Engine choices read mica_slicing_rules (mica_core::rules), not gcode_flavor or the presence of Orca keys (docs/slicing-rules.md).

use crate::{
    config::{Config, ConfigValue, Value},
    geometry::{
        ExPolygon, Point, SCALE,
        clip::{Join, diff_pl, offset_ex},
    },
    perimeters::IslandWalls,
    resolve::ResolveError,
    settings::{FloatOrPercent, PrintSettings},
};

#[derive(Clone, Debug, PartialEq)]
pub struct WallOptions {
    pub inner_outer_inner: bool,
    pub overhang_reverse: bool,
    pub internal_only: bool,
    pub threshold: FloatOrPercent,
    /// Orca `bridge_line_width` (percent of the nozzle diameter); classic overhang walls use it.
    pub bridge_line_width: Option<FloatOrPercent>,
}
impl Default for WallOptions {
    fn default() -> Self {
        Self {
            inner_outer_inner: false,
            overhang_reverse: false,
            internal_only: false,
            threshold: FloatOrPercent::Percent(50.),
            bridge_line_width: None,
        }
    }
}
impl WallOptions {
    pub fn from_config(c: &Config) -> Result<Self, ResolveError> {
        let threshold = match c.get("overhang_reverse_threshold") {
            Some(ConfigValue::Scalar(Value::FloatOrPercent { value, percent }))
                if value.is_finite() && *value >= 0. =>
            {
                if *percent {
                    FloatOrPercent::Percent(*value)
                } else {
                    FloatOrPercent::Mm(*value)
                }
            }
            None => FloatOrPercent::Percent(50.),
            _ => return Err(ResolveError("invalid overhang_reverse_threshold".into())),
        };
        // Orca's bridge line width belongs to Orca's overhang flow; PrusaSlicer rules ignore it.
        let orca = crate::rules::Rules::of(c).is_orca();
        let bridge_line_width = match c.get("bridge_line_width").filter(|_| orca) {
            Some(ConfigValue::Scalar(Value::FloatOrPercent { value, percent }))
                if value.is_finite() && *value >= 0. =>
            {
                (*value > 0.).then_some(if *percent {
                    FloatOrPercent::Percent(*value)
                } else {
                    FloatOrPercent::Mm(*value)
                })
            }
            None => None,
            _ => return Err(ResolveError("invalid bridge_line_width".into())),
        };
        Ok(Self {
            bridge_line_width,
            inner_outer_inner: c.string("wall_sequence") == Some("inner-outer-inner wall"),
            overhang_reverse: c.bool("overhang_reverse").unwrap_or(false),
            internal_only: c.bool("overhang_reverse_internal_only").unwrap_or(false),
            threshold,
        })
    }
    /// Orca propagates one detection to every loop of the same contour/hole class in an island.
    pub(crate) fn reversal(
        &self,
        walls: &IslandWalls,
        lower: Option<&[ExPolygon]>,
        layer: usize,
        s: &PrintSettings,
    ) -> [bool; 2] {
        let mut flags = [false; 2];
        if !self.overhang_reverse || layer.is_multiple_of(2) {
            return flags;
        }
        let Some(lower) = lower else {
            return flags;
        };
        let mut detect = |points: &[Point], width: f64, contour: bool, close: bool| {
            let threshold = self.threshold.resolve(width, 1.);
            let mut closed = points.to_vec();
            if close && let Some(&p) = closed.first() {
                closed.push(p);
            }
            if threshold < 1e-6
                || !diff_pl(
                    &[closed],
                    &offset_ex(lower, (threshold - width * 0.5) * SCALE, Join::Miter),
                )
                .is_empty()
            {
                flags[usize::from(contour)] = true;
            }
        };
        for wall in &walls.loops {
            detect(
                &wall.polygon,
                if wall.depth == 0 {
                    s.external_perimeter_extrusion_width
                } else {
                    s.perimeter_extrusion_width
                },
                wall.is_contour,
                true,
            );
        }
        for run in &walls.variable_walls {
            if let (Some(first), Some(last)) = (run.first(), run.last()) {
                if first.points.first() != last.points.last() {
                    continue;
                }
                let points: Vec<_> = run.iter().flat_map(|p| p.points.iter().copied()).collect();
                let contour = crate::geometry::signed_area(&points) > 0.;
                for p in run {
                    detect(&p.points, p.flow.width, contour, false);
                }
            }
        }
        flags
    }
}

/// Orca starts from outside-in groups, emits inset >=2 inside-out, then inset 0 and 1.
pub(crate) fn sandwich<T>(items: &mut Vec<T>, depth: impl Fn(&T) -> usize) {
    let mut start = 0;
    while start < items.len() {
        let end = (start + 1..items.len())
            .find(|&i| depth(&items[i]) == 0)
            .unwrap_or(items.len());
        if depth(&items[start]) == 0
            && items[start..end].iter().any(|p| depth(p) == 1)
            && items[start..end].iter().any(|p| depth(p) >= 2)
        {
            // Stable partition avoids printing disconnected first-inner fragments before the outer.
            let mut group: Vec<_> = items.drain(start..end).collect();
            let mut shallow = Vec::new();
            let mut deep = Vec::new();
            for item in group.drain(..) {
                if depth(&item) < 2 {
                    shallow.push(item);
                } else {
                    deep.push(item);
                }
            }
            deep.reverse();
            deep.extend(shallow);
            items.splice(start..start, deep);
        }
        start = end;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sandwich_keeps_disconnected_families_and_two_wall_fallback() {
        let mut depths = vec![0, 1, 2, 3, 0, 1, 2, 0, 1];
        sandwich(&mut depths, |d| *d);
        assert_eq!(depths, [3, 2, 0, 1, 2, 0, 1, 0, 1]);
    }
}
