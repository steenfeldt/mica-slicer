// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: orca@8500fcdcca src/libslic3r/GCode.cpp :: Wipe::calculateWipeRetractionLengths, Wipe::wipe, GCode::needs_retraction, GCode::retract
// Source: orca@8500fcdcca src/libslic3r/GCodeWriter.cpp :: GCodeWriter::travel_to_xyz, GCodeWriter::_spiral_travel_to_z
// Source: orca@8500fcdcca src/libslic3r/GCode/RetractWhenCrossingPerimeters.cpp :: travel_inside_internal_regions
// Source: orca@8500fcdcca src/libslic3r/Polyline.cpp :: Polyline::clip_end
// Source: orca@8500fcdcca src/libslic3r/PrintObject.cpp :: PrintObject::detect_overhangs_for_lift
// Changes: Retain original segment endpoints until the actual wipe start is known; split retraction from the configured total and use the unrounded wipe-distance denominator. Bounded head/tail wipe cache; polygon clipping instead of AABB queries. Layer outlines provide Auto Lift overhang protection; no firmware retraction.
// Changes: Each layer pair's lift-overhang slivers are clipped once per export; containment queries take the exact no-contact fast path.
// Source: orca@8500fcdcca src/libslic3r/GCodeWriter.cpp :: GCodeWriter::eager_lift, GCodeWriter::_retract
// Source: orca@8500fcdcca src/libslic3r/GCode.cpp :: WipeTowerIntegration::append_tcr2, GCode::travel_to
// Changes: Retraction of a given total (toolchange length), the tower's wipe path, the eager in-place helix, the lifted Z on the first travel after a lift, no zero-remainder retraction, the filament retraction speed as the wipe bound.
// Source: orca@8500fcdcca src/libslic3r/Print.cpp :: Print::layers_sorted_for_object
// Source: orca@8500fcdcca src/libslic3r/PrintObject.cpp :: PrintObject::get_certain_layers
// Changes: Slope and lifted XYZ travels at the travel speed, not limited by travel_speed_z (as travel_to_xyz).
// Changes: Auto Lift probes the pre-wipe travel over the largest z_hop; support travels inside a support island keep the filament, interfaces always retract.
// Source: orca@8500fcdcca src/libslic3r/GCodeWriter.cpp :: GCodeWriter::lazy_lift
// Changes: Spiral vase prints lift at once, straight up, whatever the lift type.

use crate::writer::Writer;
use mica_core::{
    config::Config,
    geometry::clip::{
        Join, diff_ex, diff_pl_is_empty, intersection_pl_is_empty, offset_ex, opening_ex,
    },
    geometry::{BoundingBox, ExPolygon, Point, SCALE},
    settings::PrintSettings,
    toolpath::{ExtrusionPath, Role},
};
use std::sync::{Arc, OnceLock};

// Per-tool configs project typed vectors; retained raw vectors still describe the whole plate.
// Scalar text overrides may replace those values without losing per-tool selection.
fn scalar_extra<'a>(c: &'a Config, key: &str) -> Option<&'a str> {
    c.extra(key).filter(|s| !s.contains(','))
}
fn numeric(c: &Config, key: &str) -> Option<f64> {
    scalar_extra(c, key)
        .and_then(|s| s.trim_end_matches('%').parse().ok())
        .or_else(|| c.float(key))
}
fn number(c: &Config, key: &str, fallback: f64) -> f64 {
    numeric(c, &format!("filament_{key}"))
        .or_else(|| numeric(c, key))
        .unwrap_or(fallback)
}
fn boolean(c: &Config, key: &str) -> Option<bool> {
    match scalar_extra(c, key) {
        Some("1" | "true") => Some(true),
        Some("0" | "false") => Some(false),
        _ => c.bool(key),
    }
}
fn flag(c: &Config, key: &str) -> bool {
    boolean(c, &format!("filament_{key}"))
        .or_else(|| boolean(c, key))
        .unwrap_or(false)
}
fn text<'a>(c: &'a Config, key: &str) -> Option<&'a str> {
    scalar_extra(c, key)
        .or_else(|| c.string(key))
        .filter(|s| *s != "nil")
}
/// Orca's travel, lift, wipe and toolchange motion (under Orca rules).
pub(crate) fn enabled(c: &Config) -> bool {
    mica_core::rules::Rules::of(c).is_orca()
}
fn mm(p: Point) -> (f64, f64) {
    (p.x as f64 / SCALE, p.y as f64 / SCALE)
}
fn prefix(points: &[Point], mut length: f64) -> Vec<Point> {
    let mut out = Vec::new();
    if let Some(&p) = points.first() {
        out.push(p);
    }
    for pair in points.windows(2) {
        let d = pair[0].distance_to(pair[1]) / SCALE;
        if d > length {
            let t = length / d;
            out.push(Point::new(
                (pair[0].x as f64 + (pair[1].x - pair[0].x) as f64 * t) as i64,
                (pair[0].y as f64 + (pair[1].y - pair[0].y) as f64 * t) as i64,
            ));
            break;
        }
        out.push(pair[1]);
        length -= d;
        if length <= 0. {
            break;
        }
    }
    out
}
// Keep the whole segment crossing the bound: clipping it here would change its slope
// when Wipe::wipe replaces the first vertex with the actual (seam-clipped) position.
fn cache_prefix(points: &mut Vec<Point>, mut distance: f64) {
    for i in 1..points.len() {
        distance -= points[i - 1].distance_to(points[i]) / SCALE;
        if distance <= 0. {
            points.truncate(i + 1);
            return;
        }
    }
}

// Wipe::calculateWipeRetractionLengths starts from the configured total, even if
// GCodeWriter already holds a partial retraction. Its before amount is a target.
fn wipe_retraction(total: f64, fraction: f64, rate: f64, distance: f64, speed: f64) -> (f64, f64) {
    let before = total * fraction.clamp(0., 1.);
    let remaining = total - before;
    let capacity = rate * distance / speed;
    if remaining <= 1e-4 || capacity <= 1e-4 {
        (before, 0.)
    } else if capacity - remaining > 1e-4 {
        (before, remaining)
    } else {
        (before + remaining - capacity, capacity)
    }
}

fn length(points: &[Point]) -> f64 {
    points
        .windows(2)
        .map(|p| p[0].distance_to(p[1]) / SCALE)
        .sum()
}

#[derive(Clone)]
pub(crate) struct Wipe {
    head: Vec<Point>,
    tail: Vec<Point>,
    pub role: Option<Role>,
    tool: usize,
    loop_gap: f64,
    speed: f64,
}
impl Default for Wipe {
    fn default() -> Self {
        Self {
            head: Vec::new(),
            tail: Vec::new(),
            role: None,
            tool: 0,
            loop_gap: 0.,
            // GCodeWriter::m_current_speed starts at F3600.
            speed: 60.,
        }
    }
}
impl Wipe {
    pub fn remember(&mut self, path: &ExtrusionPath, c: &Config, tool: usize, speed: f64) {
        self.speed = speed;
        if !enabled(c) || !flag(c, "wipe") {
            self.head.clear();
            self.tail.clear();
            self.role = Some(path.role);
            return;
        }
        let distance = number(c, "wipe_distance", 0.) + path.flow.width;
        let continuous = self.tool == tool
            && self.role == Some(path.role)
            && self
                .tail
                .last()
                .zip(path.points.first())
                .is_some_and(|(a, b)| a.distance_to(*b) < 2_000.);
        if !continuous {
            self.head.clear();
            self.tail.clear();
        }
        self.tool = tool;
        self.role = Some(path.role);
        let skip = usize::from(continuous);
        if length(&self.head) < distance {
            self.head.extend_from_slice(&path.points[skip..]);
            cache_prefix(&mut self.head, distance);
        }
        self.tail.extend_from_slice(&path.points[skip..]);
        self.tail.reverse();
        cache_prefix(&mut self.tail, distance);
        self.tail.reverse();
        self.loop_gap = if path.seam.is_some()
            || matches!(
                path.role,
                Role::Brim
                    | Role::Skirt
                    | Role::Perimeter
                    | Role::ExternalPerimeter
                    | Role::OverhangPerimeter
            ) {
            path.flow.width
        } else {
            0.
        };
    }
    pub fn reset(&mut self) {
        self.head.clear();
        self.tail.clear();
    }
    /// `m_wipe.path = tcr.wipe_path`: the prime tower's wipe, followed forward from the
    /// current position. The wipe speed stays the writer's last model speed, as upstream.
    pub fn set_path(&mut self, points: &[[f64; 2]], tool: usize) {
        self.head.clear();
        self.tail = points
            .iter()
            .rev()
            .map(|p| Point::from_mm(p[0], p[1]))
            .collect();
        self.loop_gap = 0.;
        self.tool = tool;
    }
    pub fn retract(&mut self, w: &mut Writer, c: &Config, s: &PrintSettings) {
        if !enabled(c) {
            w.retract(s.retract_length, s.retract_speed);
            return;
        }
        self.retract_length(w, c, s, s.retract_length);
    }
    /// `GCode::retract` with `length` to retract in total (`retract_length_toolchange` for a
    /// toolchange): before/during the wipe, with any shortfall retracted afterwards.
    pub fn retract_length(&mut self, w: &mut Writer, c: &Config, s: &PrintSettings, total: f64) {
        let remaining = (total - w.template_state().1).max(0.);
        if remaining <= 0. {
            return;
        }
        let mut points = if self.loop_gap > 0.
            && self
                .head
                .first()
                .zip(self.tail.last())
                .is_some_and(|(a, b)| a.distance_to(*b) / SCALE < self.loop_gap)
        {
            self.head.clone()
        } else {
            self.tail.iter().rev().copied().collect()
        };
        if !flag(c, "wipe") || self.tool != w.active_tool() {
            points.clear();
        }
        if let (Some(first), Some((x, y))) = (points.first_mut(), w.position()) {
            *first = Point::from_mm(x, y);
        }
        let distance = length(&points).min(number(c, "wipe_distance", 0.));
        points = prefix(&points, distance);
        if distance > 0.0001 {
            let speed = if flag(c, "role_based_wipe_speed") {
                // Wipes are generated before CoolingBuffer adjusts extrusion feedrates.
                self.speed
            } else {
                c.abs_value("wipe_speed", s.travel_speed)
                    .unwrap_or(s.travel_speed * 0.8)
            }
            .max(10.);
            // config.retraction_speed: Orca's print config carries the filament override
            // (Snapmaker PLA @U1: 60 mm/s over the machine's 40), as the retraction itself.
            let wipe_rate = s.retract_speed;
            let (before, during) = wipe_retraction(
                total,
                number(c, "retract_before_wipe", 0.) / 100.,
                wipe_rate,
                distance,
                speed,
            );
            w.retract_to(before, s.retract_speed);
            w.begin_wipe();
            for pair in points.windows(2) {
                let e = during * pair[0].distance_to(pair[1]) / SCALE / distance;
                let (x, y) = mm(pair[1]);
                w.wipe_to(x, y, e, speed);
            }
            w.raw(";WIPE_END");
        }
        // GCodeWriter::_retract writes nothing for a zero (EPSILON) remainder.
        if total - w.retracted() >= 1e-4 {
            w.retract_to(total, s.retract_speed);
        } else {
            w.assume_retracted(total);
        }
        self.head.clear();
        self.tail.clear();
    }
}

fn travel_slope(c: &Config) -> f64 {
    let slope = number(c, "travel_slope", 3.);
    if slope > 0. && slope < 90. {
        slope.to_radians()
    } else {
        3f64.to_radians()
    }
}

/// `GCodeWriter::_spiral_travel_to_z`: one helix turn from the current position to `target`.
fn helix(w: &mut Writer, ij: (f64, f64), target: f64, z_speed: f64, arc: bool, resolution: f64) {
    if arc {
        w.spiral_lift(target, ij, z_speed);
    } else if let Some(from) = w.position() {
        let count = (8. * 0.01 / resolution.max(1e-6)).round().clamp(4., 16.) as usize;
        let start = w.z();
        let rise = target - start;
        for i in 1..=count {
            let t = i as f64 / count as f64;
            let a = std::f64::consts::TAU * t;
            let x = from.0 + ij.0 - ij.0 * a.cos() + ij.1 * a.sin();
            let y = from.1 + ij.1 - ij.0 * a.sin() - ij.1 * a.cos();
            w.travel_to_xyz_force([x, y, start + rise * t], z_speed, z_speed);
        }
    }
}

/// `GCodeWriter::eager_lift(SpiralLift)` before Orca's change_filament_gcode: a helix in place,
/// its centre one radius in +X; a normal lift when the position is unknown.
pub(crate) fn eager_spiral_lift(w: &mut Writer, c: &Config, lift: f64, speed: f64) {
    if lift <= 0. {
        return;
    }
    let target = w.z() + lift;
    if w.position().is_none() {
        w.move_z_force(target, speed);
        return;
    }
    let radius = lift / (std::f64::consts::TAU * travel_slope(c).atan());
    helix(
        w,
        (radius, 0.),
        target,
        speed,
        c.bool("enable_arc_fitting").unwrap_or(false),
        mica_core::settings::gcode_resolution(c, 0.0125),
    );
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Hop {
    Normal,
    Slope,
    Spiral,
    Auto,
}
/// The overhang slivers of each source layer over the one below it, as `Travel::add_overhangs`
/// clips them, computed at most once per export. Every layer's travel geometry adds the pairs of
/// its last 0.4 mm, so each pair used to be clipped again for three or four consecutive layers.
/// A pair is reused only for the same wall width; another width is clipped as before.
pub(crate) struct OverhangPairs(Vec<OnceLock<(u64, Vec<ExPolygon>)>>);
impl OverhangPairs {
    pub fn new(layers: usize) -> Self {
        Self((0..layers).map(|_| OnceLock::new()).collect())
    }
}

fn overhang_pair(upper: &[ExPolygon], lower: &[ExPolygon], width: f64) -> Vec<ExPolygon> {
    opening_ex(
        &diff_ex(upper, &offset_ex(lower, 0.3 * width * SCALE, Join::Miter)),
        0.1 * width * SCALE,
    )
}

/// Geometry is independent of the tool's hop/retraction settings. One immutable instance
/// is shared by the planners for a single output layer (and never across layer frames).
#[derive(Clone, Default)]
pub(crate) struct TravelGeometry {
    internal: Vec<ExPolygon>,
    bounds: Vec<Option<BoundingBox>>,
    overhangs: Vec<ExPolygon>,
    /// `SupportLayer::support_islands` and their bounds.
    support: Vec<ExPolygon>,
    support_bounds: Vec<Option<BoundingBox>>,
}

/// Whether `line` lies within one of `areas` (`ExPolygon::contains(Polyline)`).
fn inside_any(line: &[Point], areas: &[ExPolygon], bounds: &[Option<BoundingBox>]) -> bool {
    areas.iter().zip(bounds).any(|(p, bounds)| {
        bounds.is_some_and(|b| {
            line.iter()
                .all(|v| v.x >= b.min.x && v.x <= b.max.x && v.y >= b.min.y && v.y <= b.max.y)
        }) && diff_pl_is_empty(
            std::slice::from_ref(&line.to_vec()),
            std::slice::from_ref(p),
        )
    })
}

pub(crate) struct Travel {
    hop: Hop,
    pub slope: f64,
    reduce: bool,
    arc: bool,
    resolution: f64,
    /// `needs_retraction`'s overhang probe length: the largest `z_hop` of all filaments over
    /// the current filament's slope.
    probe: f64,
    /// `PrintObject::detect_overhangs_for_lift`'s `line_width`, when configured.
    pub lift_width: Option<f64>,
    pub geometry: Arc<TravelGeometry>,
    /// Spiral vase print: `GCodeWriter::lazy_lift(.., spiral_vase)` lifts at once, straight up.
    spiral_vase: bool,
}

/// The largest value of a per-tool option over all tools (retained plate-wide text first).
fn max_over_tools(c: &Config, key: &str) -> Option<f64> {
    c.extra(key)
        .filter(|s| s.contains(','))
        .map(|s| {
            s.split(',')
                .filter_map(|v| v.trim().parse::<f64>().ok())
                .fold(0., f64::max)
        })
        .or_else(|| {
            (0..c.vector_len(key).max(1))
                .filter_map(|i| c.float_at(key, i))
                .reduce(f64::max)
        })
}
impl Travel {
    pub fn new(c: &Config) -> Option<Self> {
        if !enabled(c) {
            return None;
        }
        let hop = text(c, "filament_z_hop_types")
            .or_else(|| text(c, "z_hop_types"))
            .unwrap_or("Normal Lift");
        let slope = number(c, "travel_slope", 3.);
        let hop = if slope.is_finite() && slope > 0. && slope < 90. {
            hop
        } else {
            "Normal Lift"
        };
        let slope_radians = if slope > 0. && slope < 90. {
            slope.to_radians()
        } else {
            3f64.to_radians()
        };
        let nozzle = c.float_at("nozzle_diameter", 0).unwrap_or(0.4);
        Some(Self {
            probe: max_over_tools(c, "retract_lift").unwrap_or(0.).max(number(
                c,
                "retract_lift",
                0.,
            )) / slope_radians.tan(),
            lift_width: c.abs_value("extrusion_width", nozzle).filter(|w| *w > 0.),
            hop: match hop {
                "Slope Lift" => Hop::Slope,
                "Spiral Lift" => Hop::Spiral,
                "Auto Lift" => Hop::Auto,
                _ => Hop::Normal,
            },
            slope: if slope > 0. && slope < 90. {
                slope.to_radians()
            } else {
                3f64.to_radians()
            },
            reduce: flag(c, "reduce_infill_retraction")
                && c.float("fill_density").unwrap_or(0.) > 0.,
            arc: c.bool("enable_arc_fitting").unwrap_or(false),
            resolution: mica_core::settings::gcode_resolution(c, 0.0125),
            geometry: Arc::default(),
            spiral_vase: mica_core::spiral_vase::enabled(c),
        })
    }
    /// `pairs` gives the shared pair cache and the source index of `here` (whose pair is
    /// `here` over `below`; `below` over `older` is the pair of the index before it).
    /// `regions`: the layer's print-region slices when it has several.
    #[allow(clippy::too_many_arguments)]
    pub fn geometry(
        &mut self,
        below: Option<&[ExPolygon]>,
        here: Option<&[ExPolygon]>,
        above: Option<&[ExPolygon]>,
        older: Option<&[ExPolygon]>,
        width: f64,
        pairs: Option<(&OverhangPairs, usize)>,
        regions: Option<&[Vec<ExPolygon>]>,
    ) {
        if let (Some(below), Some(here)) = (below, here) {
            self.add_overhangs(here, below, width, pairs);
            if let Some(older) = older {
                let previous = pairs.and_then(|(p, i)| Some((p, i.checked_sub(1)?)));
                self.add_overhangs(below, older, width, previous);
            }
            if let Some(above) = above {
                let mut external = opening_ex(&diff_ex(here, below), width * SCALE / 10.);
                external.extend(opening_ex(&diff_ex(here, above), width * SCALE / 10.));
                let geometry = Arc::make_mut(&mut self.geometry);
                // RetractWhenCrossingPerimeters tests each region's internal surfaces on their
                // own: a travel from one region (part, modifier or paint) into another retracts.
                geometry.internal = match regions {
                    Some(regions) => regions
                        .iter()
                        .flat_map(|slices| diff_ex(slices, &external))
                        .collect(),
                    None => diff_ex(here, &external),
                };
                geometry.bounds = geometry
                    .internal
                    .iter()
                    .map(|p| BoundingBox::from_points(&p.contour))
                    .collect();
            }
        }
    }
    /// `pairs`: the shared cache and the source index of `upper` (`lower` is the layer below).
    pub fn add_overhangs(
        &mut self,
        upper: &[ExPolygon],
        lower: &[ExPolygon],
        width: f64,
        pairs: Option<(&OverhangPairs, usize)>,
    ) {
        let owned;
        let cached = pairs
            .and_then(|(p, i)| p.0.get(i))
            .map(|cell| cell.get_or_init(|| (width.to_bits(), overhang_pair(upper, lower, width))));
        let slivers = match cached {
            Some((w, slivers)) if *w == width.to_bits() => slivers,
            _ => {
                owned = overhang_pair(upper, lower, width);
                &owned
            }
        };
        Arc::make_mut(&mut self.geometry)
            .overhangs
            .extend_from_slice(slivers);
    }
    pub fn needs_retraction(
        &self,
        route: &[(f64, f64)],
        role: Role,
        previous: Option<Role>,
    ) -> bool {
        let perimeter = |r| {
            matches!(
                r,
                Role::Perimeter | Role::ExternalPerimeter | Role::OverhangPerimeter
            )
        };
        // Force a retraction when leaving an external (or overhang) perimeter.
        if previous.is_some_and(|r| perimeter(r) && r != Role::Perimeter) {
            return true;
        }
        let line: Vec<_> = route.iter().map(|p| Point::from_mm(p.0, p.1)).collect();
        if role == Role::SupportMaterial {
            // A support travel inside a support island does not retract. Supports print with
            // the support layer as `m_layer`, which has no internal object regions.
            return !inside_any(&line, &self.geometry.support, &self.geometry.support_bounds);
        }
        if role == Role::SupportMaterialInterface {
            return true;
        }
        !(!perimeter(role)
            && self.reduce
            && inside_any(&line, &self.geometry.internal, &self.geometry.bounds))
    }
    pub fn set_support_islands(&mut self, islands: Vec<ExPolygon>) {
        let geometry = Arc::make_mut(&mut self.geometry);
        geometry.support_bounds = islands
            .iter()
            .map(|p| BoundingBox::from_points(&p.contour))
            .collect();
        geometry.support = islands;
    }
    /// `probe`: the first travel segment `needs_retraction` saw, which starts where the last
    /// extrusion ended (before any wipe); `None` probes the emitted route.
    #[allow(clippy::too_many_arguments)]
    pub fn emit(
        &self,
        w: &mut Writer,
        route: &[(f64, f64)],
        probe: Option<((f64, f64), (f64, f64))>,
        z: f64,
        lift: f64,
        speed: f64,
        z_speed: f64,
    ) {
        if route.len() < 2 {
            return;
        }
        let from = route[0];
        let to = route[1];
        let distance = (to.0 - from.0).hypot(to.1 - from.1);
        let layer_change = w.z() < z - 0.001;
        let target = z + lift;
        let rise = (target - w.z()).max(0.);
        let slope_distance = rise / self.slope.tan();
        // Auto Lift's collision query is only consumed by an actual rising hop.
        // Width-split Arachne joints usually arrive at the current position/Z.
        let hop = rise > 0. && lift > 0. && distance > 1e-9;
        if hop && self.spiral_vase {
            // GCodeWriter::lazy_lift in a spiral vase print: "lift Z" now, whatever the lift
            // type; the travel then moves in XY and the extrusion unlifts.
            w.move_z_force(target, z_speed);
        } else if hop {
            let spiral = match self.hop {
                Hop::Spiral => true,
                Hop::Auto => {
                    layer_change || {
                        let (a, b) = probe.unwrap_or((from, to));
                        let length = (b.0 - a.0).hypot(b.1 - a.1);
                        let ratio = if length > self.probe {
                            self.probe / length
                        } else {
                            1.
                        };
                        let line = vec![
                            Point::from_mm(a.0, a.1),
                            Point::from_mm(a.0 + (b.0 - a.0) * ratio, a.1 + (b.1 - a.1) * ratio),
                        ];
                        !intersection_pl_is_empty(&[line], &self.geometry.overhangs)
                    }
                }
                _ => false,
            };
            if spiral {
                // Upstream uses atan(slope), rather than tan, for its one-turn helix radius.
                // change_layer updates Orca's nominal writer Z before the lazy hop.
                // Its radius therefore uses the configured lift, including on layer changes.
                let radius = lift / (std::f64::consts::TAU * self.slope.atan());
                let ij = (
                    -(to.1 - from.1) / distance * radius,
                    (to.0 - from.0) / distance * radius,
                );
                helix(w, ij, w.z() + rise, z_speed, self.arc, self.resolution);
            } else if self.hop == Hop::Normal {
                // GCodeWriter::_travel_to_z ("normal lift Z").
                w.move_z_force(target, z_speed);
            } else if slope_distance < distance {
                let t = slope_distance / distance;
                // GCodeWriter::travel_to_xyz writes slope and XYZ travels at the travel speed:
                // Orca does not slow them for travel_speed_z (only pure Z moves use it).
                w.travel_to_xyz_force(
                    [
                        from.0 + (to.0 - from.0) * t,
                        from.1 + (to.1 - from.1) * t,
                        target,
                    ],
                    speed,
                    0.,
                );
            }
        }
        for (i, &(x, y)) in route.iter().enumerate().skip(1) {
            if hop && i == 1 && !self.spiral_vase {
                // travel_to_xyz after a lift always writes the lifted Z (emit_xyz(target)).
                w.travel_to_xyz_force([x, y, target], speed, 0.);
            } else {
                w.travel_to_xyz([x, y, target], speed, z_speed);
            }
        }
        w.travel_to_xyz(
            [route.last().unwrap().0, route.last().unwrap().1, z],
            speed,
            z_speed,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mica_core::{
        config::{ConfigValue, Value},
        flow::Flow,
    };
    fn config(hop: &str) -> Config {
        let mut c = Config::defaults();
        mica_core::rules::Rules::OrcaSlicer.set(&mut c);
        for (k, v) in [
            ("z_hop_types", Value::String(hop.into())),
            ("wipe_distance", Value::Float(2.)),
            ("enable_arc_fitting", Value::Bool(true)),
            ("role_based_wipe_speed", Value::Bool(true)),
            ("reduce_infill_retraction", Value::Bool(true)),
        ] {
            c.set(k, ConfigValue::Scalar(v));
        }
        for (k, v) in [
            ("travel_slope", "3"),
            ("wipe", "1"),
            ("retract_before_wipe", "25%"),
            ("fill_density", "15%"),
        ] {
            c.set_raw(k, v).unwrap();
        }
        c
    }
    fn square(x: f64, y: f64, w: f64) -> ExPolygon {
        ExPolygon {
            contour: vec![
                Point::from_mm(x, y),
                Point::from_mm(x + w, y),
                Point::from_mm(x + w, y + w),
                Point::from_mm(x, y + w),
            ],
            holes: vec![],
        }
    }
    #[test]
    fn wipes_respect_rate_total_retraction_and_follow_the_loop_forward() {
        let mut c = config("Auto Lift");
        c.set(
            "retraction_speed",
            ConfigValue::Vector(vec![Some(Value::Float(40.))]),
        );
        let s = PrintSettings {
            retract_length: 0.8,
            retract_speed: 60.,
            ..PrintSettings::core_one_hf04_speed_pla()
        };
        let mut w = Writer::new();
        w.travel_to_xyz_force([0., 0., 1.], 100., 10.);
        let mut wipe = Wipe::default();
        let path = ExtrusionPath {
            points: vec![
                Point::from_mm(0., 0.),
                Point::from_mm(10., 0.),
                Point::from_mm(10., 10.),
                Point::from_mm(0., 10.),
                Point::from_mm(0., 0.04),
            ],
            flow: Flow::new(0.45, 0.2),
            role: Role::Perimeter,
            perimeter_index: Some(1),
            overhang: None,
            seam: Some(Box::default()),
            brick: None,
        };
        w.extrude_to(0., 0.04, 1., 75.); // cooling lowered the emitted extrusion speed
        wipe.remember(&path, &c, 0, 300.);
        wipe.retract(&mut w, &c, &s);
        assert!((w.template_state().1 - 0.8).abs() < 1e-9);
        assert!(w.position().unwrap().0 > 1.9, "wipe follows loop forwards");
        assert!((w.stats.filament_mm - 0.2).abs() < 1e-9);
        let before = w.position();
        wipe.retract(&mut w, &c, &s);
        assert_eq!(before, w.position());
        w.unretract(35.);
        assert!((w.stats.filament_mm - 1.).abs() < 1e-9);
        assert!(w.as_str().contains(";WIPE_START"));
        assert!(w.as_str().contains("G1 E-.4 F3600")); // the (filament) retraction rate bounds the wipe
        assert!(w.as_str().contains("G1 F18000")); // wipe retains pre-cooling speed
    }
    #[test]
    fn retraction_split_uses_total_fraction_rate_and_short_path() {
        for (total, fraction, rate, distance, speed, before, during) in [
            (0.8, 0.25, 60., 2., 100., 0.2, 0.6),
            (0.8, 0.25, 60., 2., 300., 0.4, 0.4),
            (0.8, 0.25, 60., 0.1, 300., 0.78, 0.02),
            (0.8, 1., 60., 2., 100., 0.8, 0.),
            // Below EPSILON, the remainder is retracted after the zero-E wipe.
            (0.8, 0.25, 60., 0.0002, 300., 0.2, 0.),
        ] {
            let split = wipe_retraction(total, fraction, rate, distance, speed);
            assert!((split.0 - before).abs() < 1e-9, "{split:?}");
            assert!((split.1 - during).abs() < 1e-9, "{split:?}");
        }
    }

    #[test]
    fn negligible_wipe_capacity_leaves_remainder_after_wiping() {
        let mut c = config("Normal Lift");
        c.set_raw("wipe_distance", "0.001").unwrap();
        let s = PrintSettings {
            retract_length: 0.8,
            retract_speed: 60.,
            ..PrintSettings::core_one_hf04_speed_pla()
        };
        let mut w = Writer::new();
        w.apply_template_state(Some([0., 0., 1.]), 0., 0.);
        let mut wipe = Wipe {
            speed: 1000.,
            ..Wipe::default()
        };
        wipe.set_path(&[[0., 0.], [10., 0.]], 0);
        wipe.retract(&mut w, &c, &s);
        assert_eq!(
            w.as_str(),
            "G1 E-.2 F3600\n;WIPE_START\nG1 F60000\nG1 X.001 Y0 E0\n;WIPE_END\nG1 E-.6 F3600\n"
        );
        assert!((w.retracted() - 0.8).abs() < 1e-9);
    }

    #[test]
    fn partial_retraction_targets_the_configured_before_amount() {
        let c = config("Normal Lift");
        let s = PrintSettings {
            retract_length: 0.8,
            retract_speed: 60.,
            ..PrintSettings::core_one_hf04_speed_pla()
        };
        let mut w = Writer::new();
        w.apply_template_state(Some([0., 0., 1.]), 0.1, 0.);
        let mut wipe = Wipe {
            speed: 100.,
            ..Wipe::default()
        };
        wipe.set_path(&[[0., 0.], [10., 0.]], 0);
        wipe.retract(&mut w, &c, &s);
        assert!(
            w.as_str().contains("G1 E-.1 F3600\n;WIPE_START\n"),
            "{}",
            w.as_str()
        );
        assert!(w.as_str().contains("G1 X2 Y0 E-.6"), "{}", w.as_str());
        assert!((w.retracted() - 0.8).abs() < 1e-9);
    }

    #[test]
    fn seam_clipped_loop_retains_original_segment_for_native_u1_split() {
        let mut c = config("Normal Lift");
        c.set_raw("retract_before_wipe", "0%").unwrap();
        let s = PrintSettings {
            retract_length: 0.8,
            retract_speed: 60.,
            ..PrintSettings::core_one_hf04_speed_pla()
        };
        let path = ExtrusionPath {
            points: vec![
                Point::from_mm(145.29, 145.79),
                Point::from_mm(125.71, 145.79),
                Point::from_mm(125.71, 126.21),
                Point::from_mm(145.29, 126.21),
                Point::from_mm(145.29, 145.75),
            ],
            flow: Flow::new(0.45, 0.2),
            role: Role::Perimeter,
            perimeter_index: Some(1),
            overhang: None,
            seam: Some(Box::default()),
            brick: None,
        };
        let mut w = Writer::new();
        w.apply_template_state(Some([145.29, 145.75, 0.45]), 0., 0.);
        let mut wipe = Wipe::default();
        wipe.remember(&path, &c, 0, 11365.56 / 60.);
        wipe.retract(&mut w, &c, &s);
        // Native Orca 2.4.1, cube20's layer-2 outer-wall departure.
        assert_eq!(
            w.as_str(),
            "G1 E-.16651 F3600\n;WIPE_START\nG1 F11365.56\nG1 X143.29 Y145.754 E-.63349\n;WIPE_END\n"
        );
        // Integer XY clipping can leave sub-EPSILON excess, as in Extruder::extrude.
        assert!((w.retracted() - 0.8).abs() < 1e-6);
    }

    #[test]
    fn retained_vectors_do_not_override_projected_tool_values() {
        let mut c = config("Auto Lift");
        c.set_raw("z_hop_types", "Auto Lift,Auto Lift,Auto Lift,Auto Lift")
            .unwrap();
        c.set_raw("filament_z_hop_types", "nil,nil,nil,nil")
            .unwrap();
        assert_eq!(Travel::new(&c).unwrap().hop, Hop::Auto);
        c.set_raw("z_hop_types", "Slope Lift").unwrap();
        assert_eq!(Travel::new(&c).unwrap().hop, Hop::Slope);
    }
    #[test]
    fn hops_restore_z_and_zero_slope_is_finite() {
        for mode in ["Normal Lift", "Slope Lift", "Spiral Lift", "Auto Lift"] {
            let c = config(mode);
            let t = Travel::new(&c).unwrap();
            let mut w = Writer::new();
            w.apply_template_state(Some([10., 10., 1.]), 0., 0.);
            t.emit(
                &mut w,
                &[(10., 10.), (30., 10.)],
                None,
                1.2,
                0.4,
                500.,
                500.,
            );
            assert_eq!(w.position(), Some((30., 10.)));
            assert_eq!(w.z(), 1.2);
            let text = w.as_str();
            assert!(!text.contains("NaN") && !text.contains("inf"));
            assert_eq!(
                text.contains(" P1 "),
                matches!(mode, "Spiral Lift" | "Auto Lift")
            );
        }
        let mut c = config("Auto Lift");
        c.set_raw("travel_slope", "0").unwrap();
        let t = Travel::new(&c).unwrap();
        assert_eq!(t.hop, Hop::Normal);
        let mut w = Writer::new();
        w.apply_template_state(Some([0., 0., 1.]), 0., 0.);
        t.emit(&mut w, &[(0., 0.), (20., 0.)], None, 1.2, 0.4, 500., 10.);
        assert!(w.stats.time_s.is_finite());
        assert!(!w.as_str().contains("inf"));
    }
    #[test]
    fn internal_travel_respects_holes_and_external_walls() {
        let c = config("Auto Lift");
        let mut t = Travel::new(&c).unwrap();
        let mut p = square(0., 0., 20.);
        let mut hole = square(8., 8., 4.).contour;
        hole.reverse();
        p.holes.push(hole);
        let area = vec![p];
        t.geometry(Some(&area), Some(&area), Some(&area), None, 0.4, None, None);
        assert!(!t.needs_retraction(
            &[(1., 1.), (18., 1.)],
            Role::InternalInfill,
            Some(Role::InternalInfill)
        ));
        assert!(t.needs_retraction(
            &[(1., 10.), (18., 10.)],
            Role::InternalInfill,
            Some(Role::InternalInfill)
        ));
        assert!(t.needs_retraction(
            &[(1., 1.), (18., 1.)],
            Role::InternalInfill,
            Some(Role::ExternalPerimeter)
        ));
        assert!(t.needs_retraction(
            &[(1., 1.), (18., 1.)],
            Role::Perimeter,
            Some(Role::InternalInfill)
        ));
    }
    #[test]
    fn support_travel_inside_an_island_does_not_retract() {
        let mut c = config("Auto Lift");
        c.set_raw("reduce_infill_retraction", "0").unwrap();
        let mut t = Travel::new(&c).unwrap();
        t.set_support_islands(vec![square(0., 0., 20.)]);
        let inside = [(1., 1.), (18., 1.)];
        let outside = [(1., 1.), (25., 1.)];
        assert!(!t.needs_retraction(&inside, Role::SupportMaterial, Some(Role::SupportMaterial)));
        assert!(t.needs_retraction(&outside, Role::SupportMaterial, Some(Role::SupportMaterial)));
        // Interfaces always retract; so does leaving an external perimeter.
        assert!(t.needs_retraction(
            &inside,
            Role::SupportMaterialInterface,
            Some(Role::SupportMaterial)
        ));
        assert!(t.needs_retraction(
            &inside,
            Role::SupportMaterial,
            Some(Role::ExternalPerimeter)
        ));
    }
    #[test]
    fn auto_lift_probes_the_travel_planned_before_the_wipe() {
        let mut c = config("Auto Lift");
        c.set_raw("retract_lift", "0.4").unwrap();
        let mut t = Travel::new(&c).unwrap();
        // An overhang sliver across the line y = 0 at x 0.5..1.5.
        t.add_overhangs(&[square(0.5, -0.5, 1.)], &[], 0.4, None);
        let spiral = |probe| {
            let mut w = Writer::new();
            w.apply_template_state(Some([10., 0., 1.2]), 0., 0.);
            t.emit(&mut w, &[(10., 0.), (0., 0.)], probe, 1.2, 0.4, 500., 500.);
            w.as_str().contains(" P1 ")
        };
        // From the wipe end at x 10 the 7.6 mm probe (z_hop / tan 3°) stops short of the
        // sliver; from the extrusion end at x 6 it reaches it.
        assert!(!spiral(None));
        assert!(spiral(Some(((6., 0.), (0., 0.)))));
    }
}
