// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: orca@8500fcdcca src/libslic3r/GCode.cpp :: GCode::_extrude
// Source: prusaslicer@b028299c77 src/libslic3r/GCode.cpp :: GCodeGenerator::_extrude
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/ExtrusionProcessor.cpp :: calculate_overhang_speed
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/CoolingBuffer.cpp :: split_linear_segment, split_arc_segment
// Source: orca@8500fcdcca src/libslic3r/Brim.cpp :: makeBrimInfillImpl
// Source: orca@8500fcdcca src/libslic3r/LayerRegion.cpp :: LayerRegion::simplify_path
// Source: orca@8500fcdcca src/libslic3r/GCode.cpp :: GCode::_extrude
// Changes: Scarf ramp speed caps share cooling, preview and emission section boundaries.
// Changes: Orca adhesion paths bypass layer-region arc fitting.
// Changes: Bounded parallel curve preparation; ordered per-range cooling/fan state in a layer-local plan.
// Source: orca@8500fcdcca src/libslic3r/GCode/CoolingBuffer.cpp :: CoolingBuffer::parse_layer_gcode
// Changes: Orca cooling slows every block (bridges and overhang walls too) and times feedrate-only lines with the last E word, as parse_layer_gcode does; fan marker decisions per section feed orca_fan.
// Changes: Engine choices read mica_slicing_rules (mica_core::rules), not gcode_flavor or the presence of Orca keys (docs/slicing-rules.md).
// Changes: Orca overhang sections (orca_overhang) print at their resolved `_extrude` speeds.
use crate::{
    cooling::{self, CoolingConfig, Move},
    extrusion_speed,
    smooth_path::ArcFitting,
    writer::Writer,
};
use mica_core::{
    config::Config,
    geometry::{
        SCALE,
        arc_welder::{self as arc, Path},
    },
    settings::PrintSettings,
    toolpath::{ExtrusionPath, LayerToolpaths, Role},
};

/// Upstream snaps a split within 10 XYZ output quanta to the existing vertex.
fn split_cooling_tail(curve: &Path, tail: f64) -> (Path, Path) {
    let length = arc::path_length(curve);
    let mut cut = (length - tail * SCALE).clamp(0., length);
    let mut at = 0.;
    for edge in curve.windows(2) {
        let end = at + arc::segment_length(&edge[0], &edge[1]);
        if cut <= end {
            if cut - at <= 0.01 * SCALE {
                cut = at;
            } else if end - cut <= 0.01 * SCALE {
                cut = end;
            }
            break;
        }
        at = end;
    }
    let mut head = curve.clone();
    let mut end = curve.clone();
    arc::clip_end(&mut head, length - cut);
    arc::clip_start(&mut end, cut);
    (head, end)
}

/// Split speed planning at scarf ramp boundaries as well as support-distance boundaries.
/// Ordinary paths retain exactly their existing sections.
fn ranges(
    path: &ExtrusionPath,
) -> Vec<(
    std::ops::Range<usize>,
    Option<mica_core::toolpath::OverhangSection>,
    Option<f64>,
)> {
    let last = path.points.len().saturating_sub(1);
    if last == 0 {
        return vec![];
    }
    let scarf = path
        .seam
        .as_ref()
        .and_then(|m| Some((m.scarf_ranges?, m.scarf_speed?)));
    let mut result = Vec::new();
    let mut append = |start: usize, end: usize, attr| {
        let mut cuts = vec![start, end];
        if let Some(([a, b], _)) = scarf {
            cuts.extend([a, b].into_iter().filter(|&i| i > start && i < end));
        }
        cuts.sort_unstable();
        cuts.dedup();
        for w in cuts.windows(2) {
            let limit = scarf.and_then(|([a, b], speed)| (w[0] < a || w[0] >= b).then_some(speed));
            result.push((w[0]..w[1], attr, limit));
        }
    };
    if let Some(attrs) = &path.overhang {
        let mut start = 0;
        for &attr in attrs.iter() {
            append(start, attr.end as usize, Some(attr));
            start = attr.end as usize;
        }
    } else {
        append(0, last, None);
    }
    result
}

struct Section {
    range: std::ops::Range<usize>,
    curve: Path,
    prepared: Option<crate::smooth_path::Prepared>,
    cooling: usize,
    fan: Option<f64>,
    original_speed: f64,
    flow_model: Option<mica_core::small_features::FlowCompensation>,
    /// Orca's role-based fan marker decisions at this section's start.
    orca_markers: Option<crate::orca_fan::Markers>,
}
/// Geometry has no writer, fan or cooling history. Prepared for a bounded batch and
/// consumed in output order, with the original section boundaries and fit tolerances.
#[derive(Default)]
pub(crate) struct LayerCurves(Vec<Vec<Curve>>);
struct Curve {
    path: Path,
    prepared: Option<crate::smooth_path::Prepared>,
}
impl LayerCurves {
    pub fn new<'a>(
        layer: &LayerToolpaths,
        settings: impl Fn(usize) -> (&'a Config, &'a PrintSettings, usize) + Sync,
        tolerance: f64,
        mode: ArcFitting,
    ) -> Self {
        let _timer = mica_core::timing::Span::new(mica_core::timing::Stage::ExportPlan);
        Self(mica_core::parallel::map(&layer.paths, |i, path| {
            let (c, s, tool) = settings(i);
            let offset = c
                .points("extruder_offset")
                .and_then(|v| v.get(tool).or(v.first()).copied())
                .unwrap_or_default();
            let filament_area = std::f64::consts::PI * (s.filament_diameter / 2.).powi(2);
            let e_per_mm = path.mm3_per_mm() * s.extrusion_multiplier / filament_area;
            let prepare = |curve: Path| {
                let prepared = path
                    .seam
                    .as_ref()
                    .is_none_or(|seam| seam.samples.is_empty())
                    .then(|| crate::smooth_path::Prepared::new(&curve, e_per_mm, mode, offset));
                Curve {
                    path: curve,
                    prepared,
                }
            };
            let tolerance = tolerance
                * if path.role == Role::InternalInfill {
                    3.
                } else {
                    1.
                };
            let length_tolerance = if mode == ArcFitting::Disabled
                || (s.adhesion.orca && matches!(path.role, Role::Brim | Role::Skirt))
            {
                0.
            } else {
                arc::LENGTH_TOLERANCE
            };
            ranges(path)
                .into_iter()
                .map(|(range, _, _)| {
                    prepare(arc::fit_path(
                        &path.points[range.start..=range.end],
                        tolerance,
                        length_tolerance,
                    ))
                })
                .collect()
        }))
    }
}
fn last_segment_length(curve: &Path) -> f64 {
    curve.len().checked_sub(2).map_or(0., |i| {
        arc::segment_length(&curve[i], &curve[i + 1]) / SCALE
    })
}

/// Orca `CoolingBuffer::parse_layer_gcode` resets the relative E accumulator after copying the
/// position, so a feedrate-only `G1 F…;_EXTRUDE_SET_SPEED` line is timed as an extruder move of
/// the last E word seen. `GCode::_extrude` writes one such line before each path, and in a
/// variable-speed (overhang) path one per segment printed at the nominal speed and one per
/// speed change. Adds those phantom lengths after the first line to the sections' cooling
/// blocks; returns the path's last segment E and the count of lines timed with the E before it.
fn orca_phantoms(
    moves: &mut [Move],
    sections: &[Section],
    nominal: f64,
    e_per_mm: f64,
) -> (f64, f64) {
    let speeds: Vec<f64> = sections.iter().map(|s| moves[s.cooling].speed).collect();
    let variable = speeds.iter().any(|v| (v - nominal).abs() > 1.);
    let mut leading = 1.;
    if variable {
        for (k, section) in sections.iter().enumerate() {
            let length = arc::path_length(&section.curve) / SCALE;
            let phantom = if (speeds[k] - nominal).abs() <= 1. {
                if k == 0 {
                    // The first segment repeats the starting feedrate line.
                    leading = 2.;
                }
                // One line per segment, each timed with the E of the segment before it.
                e_per_mm * (length - last_segment_length(&section.curve))
                    + if k > 0 {
                        e_per_mm * last_segment_length(&sections[k - 1].curve)
                    } else {
                        0.
                    }
            } else if k > 0 && (speeds[k] - speeds[k - 1]).abs() > 1. {
                e_per_mm * last_segment_length(&sections[k - 1].curve)
            } else {
                0.
            };
            moves[section.cooling].length_mm += phantom;
        }
    }
    let last = sections
        .last()
        .map_or(0., |s| e_per_mm * last_segment_length(&s.curve));
    (last, leading)
}

pub(crate) struct LayerPlan {
    pub moves: Vec<Move>,
    paths: Vec<Vec<Section>>,
    /// Orca CoolingBuffer timing only: per path, the E of its last segment and how many
    /// feedrate-only lines start it (see `orca_phantom`).
    orca: Vec<Option<(f64, f64)>>,
}
impl LayerPlan {
    /// Calibration requests replace profile speeds before cooling and time accounting.
    pub fn calibrate(&mut self, source: &dyn mica_core::print::LayerSource, layer: usize) {
        for (i, sections) in self.paths.iter_mut().enumerate() {
            if let Some(speed) = source
                .calibration_control(layer, i)
                .and_then(|c| c.speed_mm_s)
            {
                for section in sections {
                    section.original_speed = speed;
                    self.moves[section.cooling].speed = speed;
                }
            }
        }
    }
    /// The smoothed geometry path `index` is emitted with, one curve per section.
    pub fn curves(&self, index: usize) -> Vec<Path> {
        self.paths[index].iter().map(|s| s.curve.clone()).collect()
    }
    /// Orca's phantom timing of path `index`'s starting feedrate line(s), with `previous_e` the
    /// last E word before it (the deretraction, or the previous path's last segment). Returns the
    /// E of this path's last segment, or `None` outside Orca cooling.
    pub fn orca_phantom(&mut self, index: usize, previous_e: f64) -> Option<f64> {
        let (last, leading) = self.orca.get(index).copied().flatten()?;
        if let Some(first) = self.paths[index].first() {
            self.moves[first.cooling].length_mm += leading * previous_e.abs();
        }
        Some(last)
    }
    pub fn final_original_speed(&self, index: usize) -> f64 {
        self.paths[index].last().map_or(0., |s| s.original_speed)
    }
    /// `settings(i)` gives the config and settings of path `i`'s tool.
    pub fn new<'a>(
        layer: &LayerToolpaths,
        settings: impl Fn(usize) -> (&'a Config, &'a PrintSettings, usize),
        curves: LayerCurves,
        current_fan: &mut Option<f64>,
    ) -> Self {
        let mut plan = Self {
            moves: Vec::new(),
            paths: Vec::with_capacity(layer.paths.len()),
            orca: Vec::with_capacity(layer.paths.len()),
        };
        // CoolingBuffer resets to the layer fan before consuming SET/RESET markers. The
        // generator's last dynamic request survives that reset, so unchanged requests emit no marker.
        let mut requested_fan = None;
        for (i, (path, curves)) in layer.paths.iter().zip(curves.0).enumerate() {
            let (c, s, tool) = settings(i);
            // Orca's CoolingBuffer slows every `;_EXTRUDE_SET_SPEED` block, bridges included.
            let orca = mica_core::rules::Rules::of(c).is_orca();
            let default = extrusion_speed::nominal_speed(c, s, path, layer.index == 0);
            let reference = extrusion_speed::cap(c, s, path, s.external_perimeter_speed);
            let mut sections = Vec::new();
            let mut curves = curves.into_iter();
            let mut append = |start, end, speed, fan| {
                let curve = curves.next().expect("one prepared curve per section");
                let cooling = plan.moves.len();
                plan.moves.push(Move {
                    length_mm: arc::path_length(&curve.path) / SCALE,
                    speed,
                    external: path.role == Role::ExternalPerimeter,
                    adjustable: orca || !path.role.is_bridge(),
                    perimeter_index: path.perimeter_index,
                    tool,
                    non_adjustable_length: 0.,
                });
                sections.push(Section {
                    range: start..end,
                    curve: curve.path,
                    prepared: curve.prepared,
                    cooling,
                    fan,
                    original_speed: speed,
                    flow_model: crate::small_area::model(s, path.role, layer.index == 0).cloned(),
                    orca_markers: None,
                });
            };
            // Supported fraction of the line width at each section's ends (ExtrusionProcessor's
            // `overlap`), from the distances to the lower layer.
            let mut overlaps = Vec::new();
            for (range, attr, limit) in ranges(path) {
                overlaps.push(attr.map_or((1., 1.), |a| {
                    let overlap = |d: f32| 1. - (d as f64 / path.flow.width).clamp(0., 1.);
                    (overlap(a.start_distance), overlap(a.end_distance))
                }));
                let speed = if let Some(attr) = attr.filter(|a| a.speed > 0.) {
                    // Orca's resolved segment speed; Orca's part fan follows its markers.
                    *current_fan = None;
                    requested_fan = None;
                    let nominal = extrusion_speed::speed(c, s, path, layer.index == 0);
                    let speed = f64::from(attr.speed);
                    // The path's own speed, without the f32 rounding of the section.
                    if (speed - nominal).abs() < 1e-3 {
                        nominal
                    } else {
                        speed
                    }
                } else if let Some(attr) = attr {
                    let (speed, fan) = crate::overhang::calculate_overhang_speed(
                        c,
                        path.flow.width,
                        attr,
                        reference,
                        default,
                        *current_fan,
                    );
                    let next_fan = fan.map(f64::trunc);
                    if next_fan != *current_fan {
                        requested_fan = next_fan;
                    }
                    *current_fan = next_fan;
                    extrusion_speed::cap(c, s, path, speed.unwrap_or(default))
                } else {
                    *current_fan = None;
                    requested_fan = None;
                    extrusion_speed::speed(c, s, path, layer.index == 0)
                };
                append(
                    range.start,
                    range.end,
                    speed.min(limit.unwrap_or(f64::INFINITY)),
                    requested_fan,
                );
            }
            if orca {
                // `_extrude` evaluates the fan markers once per path, or per segment of a path
                // printed with variable overhang speeds.
                let nominal = extrusion_speed::speed(c, s, path, layer.index == 0);
                let variable = sections
                    .iter()
                    .any(|section| (plan.moves[section.cooling].speed - nominal).abs() > 1.);
                for (k, section) in sections.iter_mut().enumerate() {
                    if variable || k == 0 {
                        section.orca_markers = Some(crate::orca_fan::markers(
                            c,
                            tool,
                            path.role,
                            variable,
                            overlaps[k],
                        ));
                    }
                }
            }
            plan.orca.push(orca.then(|| {
                let e_per_mm = path.mm3_per_mm() * s.extrusion_multiplier
                    / (std::f64::consts::PI * (s.filament_diameter / 2.).powi(2));
                orca_phantoms(
                    &mut plan.moves,
                    &sections,
                    extrusion_speed::speed(c, s, path, layer.index == 0),
                    e_per_mm,
                )
            }));
            plan.paths.push(sections);
        }
        plan
    }
    /// The final speed and fan of every section, for preview sinks. `fan(path, requested)`
    /// resolves a section's dynamic fan request against the layer fan.
    pub fn emissions(
        &self,
        fan: impl Fn(usize, Option<f64>) -> f64,
    ) -> Vec<mica_core::print::PathEmission> {
        let mut out = Vec::new();
        for (path, sections) in self.paths.iter().enumerate() {
            for section in sections {
                out.push(mica_core::print::PathEmission {
                    path,
                    points: section.range.clone(),
                    speed_mm_s: self.moves[section.cooling].speed,
                    fan_percent: fan(path, section.fan),
                });
            }
        }
        out
    }
    #[allow(clippy::too_many_arguments)]
    pub fn emit(
        &self,
        w: &mut Writer,
        index: usize,
        path: &ExtrusionPath,
        layer: &LayerToolpaths,
        cooling: &CoolingConfig,
        time: f64,
        bridge_fan: f64,
        e_per_mm: f64,
        z_speed: f64,
        mode: ArcFitting,
        orca_fan: bool,
    ) {
        for section in &self.paths[index] {
            if orca_fan {
                if let Some(markers) = section.orca_markers {
                    w.with_orca_fan(|fan, w| fan.apply(w, markers));
                }
            } else {
                w.set_fan(cooling::requested_fan_speed(
                    layer.index,
                    time,
                    cooling,
                    section.fan,
                    path.role.is_bridge().then_some(bridge_fan),
                ));
            }
            let speed = self.moves[section.cooling].speed;
            let speed = if speed != section.original_speed {
                (60. * speed).round() / 60.
            } else {
                speed
            };
            let tail = self.moves[section.cooling].non_adjustable_length;
            if !crate::seam::extrude_range_with_cooling(
                w,
                path,
                layer,
                section.range.clone(),
                [speed, e_per_mm, z_speed],
                [tail, section.original_speed],
            ) {
                if let Some(model) = &section.flow_model {
                    crate::small_area::emit(
                        w,
                        &section.curve,
                        model,
                        e_per_mm,
                        speed,
                        tail,
                        section.original_speed,
                        mode,
                    );
                } else if tail > 0.
                    && crate::smooth_path::emit_split_tail(
                        w,
                        &section.curve,
                        e_per_mm,
                        speed,
                        section.original_speed,
                        tail,
                        mode,
                    )
                {
                } else if tail > 0. {
                    let (head, end) = split_cooling_tail(&section.curve, tail);
                    crate::smooth_path::emit(w, &head, e_per_mm, speed, mode);
                    crate::smooth_path::emit(w, &end, e_per_mm, section.original_speed, mode);
                } else if !section
                    .prepared
                    .as_ref()
                    .is_some_and(|prepared| prepared.emit(w, speed, e_per_mm, mode))
                {
                    crate::smooth_path::emit(w, &section.curve, e_per_mm, speed, mode);
                }
            }
        }
        if !orca_fan
            && path.role.is_bridge()
            && self.paths[index].last().is_some_and(|s| s.fan.is_none())
        {
            // Consume BRIDGE_FAN_END before the next travel, restoring the layer's fan.
            w.set_fan(cooling::fan_speed(layer.index, time, cooling));
        }
    }
}

#[cfg(test)]
mod scarf_tests {
    use super::*;
    use mica_core::{
        flow::Flow,
        geometry::Point,
        seam::{PathSeam, ScarfSample},
        toolpath::OverhangSection,
    };
    #[test]
    fn scarf_speed_sections_intersect_overhang_sections_and_leave_flat_speed() {
        let path = ExtrusionPath {
            role: Role::ExternalPerimeter,
            points: (0..=6).map(|i| Point::from_mm(i as f64, 0.)).collect(),
            flow: Flow::new(0.45, 0.2),
            perimeter_index: Some(0),
            seam: Some(Box::new(PathSeam {
                samples: vec![ScarfSample::default(); 7],
                scarf_ranges: Some([2, 4]),
                scarf_speed: Some(30.),
                ..Default::default()
            })),
            overhang: Some(
                vec![
                    OverhangSection {
                        end: 3,
                        start_distance: 0.,
                        end_distance: 0.,
                        curled_proximity: 0.,
                        speed: 0.,
                    },
                    OverhangSection {
                        end: 6,
                        start_distance: 0.,
                        end_distance: 0.,
                        curled_proximity: 0.,
                        speed: 0.,
                    },
                ]
                .into_boxed_slice(),
            ),
            brick: None,
        };
        let sections = ranges(&path);
        assert_eq!(
            sections
                .iter()
                .map(|(r, _, limit)| (r.clone(), *limit))
                .collect::<Vec<_>>(),
            [
                (0..2, Some(30.)),
                (2..3, None),
                (3..4, None),
                (4..6, Some(30.))
            ]
        );
        let mut c = Config::defaults();
        c.set_raw("external_perimeter_speed", "100").unwrap();
        c.set_raw("small_perimeter_speed", "100").unwrap();
        let s = PrintSettings::from_config(&c).unwrap();
        let layer = LayerToolpaths {
            support_groups: Vec::new(),
            index: 1,
            print_z: 0.4,
            height: 0.2,
            paths: vec![path],
        };
        let curves = LayerCurves::new(&layer, |_| (&c, &s, 0), 0., ArcFitting::Disabled);
        let plan = LayerPlan::new(&layer, |_| (&c, &s, 0), curves, &mut None);
        assert_eq!(plan.moves.len(), 4);
        assert_eq!(plan.moves[0].speed, 30.);
        assert!(plan.moves[1].speed > 30.);
        assert_eq!(plan.moves[3].speed, 30.);
    }

    #[test]
    fn orca_sections_print_at_their_resolved_speeds() {
        let section = |end, speed| OverhangSection {
            end,
            start_distance: 0.,
            end_distance: 0.,
            curled_proximity: 0.,
            speed,
        };
        let mut c = Config::defaults();
        mica_core::rules::Rules::OrcaSlicer.set(&mut c);
        c.set_raw("external_perimeter_speed", "100").unwrap();
        c.set_raw("small_perimeter_speed", "100").unwrap();
        let s = PrintSettings::from_config(&c).unwrap();
        let mut path = ExtrusionPath {
            role: Role::ExternalPerimeter,
            points: (0..=3).map(|i| Point::from_mm(i as f64, 0.)).collect(),
            flow: Flow::new(0.45, 0.2),
            perimeter_index: Some(0),
            seam: None,
            overhang: None,
            brick: None,
        };
        let nominal = extrusion_speed::speed(&c, &s, &path, false);
        path.overhang = Some(
            vec![section(1, nominal as f32), section(2, 30.), section(3, 10.)].into_boxed_slice(),
        );
        let layer = LayerToolpaths {
            support_groups: Vec::new(),
            index: 1,
            print_z: 0.4,
            height: 0.2,
            paths: vec![path],
        };
        let curves = LayerCurves::new(&layer, |_| (&c, &s, 0), 0., ArcFitting::Disabled);
        let plan = LayerPlan::new(&layer, |_| (&c, &s, 0), curves, &mut None);
        let speeds: Vec<f64> = plan.moves.iter().map(|m| m.speed).collect();
        assert_eq!(speeds, [nominal, 30., 10.]);
    }

    #[test]
    fn orca_cooling_slows_bridges_and_times_feedrate_only_lines() {
        let bridge = ExtrusionPath {
            role: Role::BridgeInfill,
            points: (0..=10).map(|i| Point::from_mm(i as f64, 0.)).collect(),
            flow: Flow::new(0.45, 0.2),
            perimeter_index: None,
            seam: None,
            overhang: None,
            brick: None,
        };
        let layer = LayerToolpaths {
            support_groups: Vec::new(),
            index: 1,
            print_z: 0.4,
            height: 0.2,
            paths: vec![bridge],
        };
        let prusa = Config::defaults();
        let mut orca = prusa.clone();
        // Orca rules select Orca's cooling; the Orca key alone no longer does.
        orca.set(
            "slow_down_for_layer_cooling",
            mica_core::config::ConfigValue::Scalar(mica_core::config::Value::Bool(true)),
        );
        let mut prusa = prusa;
        prusa.set(
            "slow_down_for_layer_cooling",
            mica_core::config::ConfigValue::Scalar(mica_core::config::Value::Bool(true)),
        );
        mica_core::rules::Rules::OrcaSlicer.set(&mut orca);
        for (c, adjustable) in [(&prusa, false), (&orca, true)] {
            let s = PrintSettings::from_config(c).unwrap();
            let curves = LayerCurves::new(&layer, |_| (c, &s, 0), 0., ArcFitting::Disabled);
            let mut plan = LayerPlan::new(&layer, |_| (c, &s, 0), curves, &mut None);
            assert_eq!(plan.moves[0].adjustable, adjustable);
            let before = plan.moves[0].length_mm;
            // The starting `G1 F` line after a 0.8 mm deretraction.
            let last = plan.orca_phantom(0, 0.8);
            assert_eq!(last.is_some(), adjustable);
            let expected = if adjustable { before + 0.8 } else { before };
            assert!((plan.moves[0].length_mm - expected).abs() < 1e-9);
        }
    }
}
