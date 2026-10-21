// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/Travels.cpp :: get_first_crossed_line_distance, get_elevated_traval_params, generate_travel_to_extrusion
// Source: prusaslicer@b028299c77 src/libslic3r/GCode.cpp :: GCodeGenerator::travel_to
// Source: prusaslicer@b028299c77 src/libslic3r/GCode.cpp :: GCodeGenerator::travel_to_first_position
// Changes: travel_lift is shared with the wipe tower's first travel.
// Source: prusaslicer@b028299c77 src/libslic3r/GCode.cpp :: GCodeGenerator::generate_travel_gcode
// Changes: Waypoints go through the writer's travel insert (the object label change).
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/GCodeWriter.cpp :: travel_to_xyz_force
// Source: prusaslicer@b028299c77 src/libslic3r/GCode.cpp :: get_ramping_layer_change_params, GCodeGenerator::change_layer, line_distancer_is_required
// Source: orca@8500fcdcca src/libslic3r/GCode.cpp :: GCode::needs_retraction
// Changes: Source-selected Orca motion delegates to orca_motion, retaining Prusa defaults.
// Source: orca@8500fcdcca src/libslic3r/Print.cpp :: Print::layers_sorted_for_object
// Changes: Orca's lift overhang layers start at the float print_z - 0.4f; detect_overhangs_for_lift's line_width; the travel's support islands.
// Changes: Routed island and external plate travels, linear boundary intersection scans instead of AABB trees.
// Changes: No legacy-Marlin parabola (upstream disables it for Marlin2/Klipper); enabled obstacle checks without outlines use a vertical lift.
// Source: prusaslicer@b028299c77 src/libslic3r/GCode.cpp :: GCodeGenerator::generate_travel_xy_path, GCodeGenerator::get_layer_change_xy_path
// Changes: The Prusa path routes with the ported AvoidCrossingPerimeters (prusa_avoid_crossing.rs) only for travels that
// Changes: need retraction, and plans the layer change on the previous layer; the adapted router remains for Orca.
// Changes: Layer neighbours come from the source, so a sequential object's first layer has none below it.
// Changes: Engine choices read mica_slicing_rules (mica_core::rules), not gcode_flavor or the presence of Orca keys (docs/slicing-rules.md).
// Changes: The Orca travel geometry can be computed ahead of the planner from the layer's height and first external wall width (`orca_travel_geometry`).
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/Travels.cpp :: slice_xy_path, generate_elevated_travel, ElevatedTravelFormula, get_smoothing_params, linspace
// Source: prusaslicer@b028299c77 src/libslic3r/GCode.cpp :: get_ramping_layer_change_params, generate_ramping_layer_change_gcode
// Changes: The Prusa path builds elevated travels on scaled integer points with upstream's truncating
// Changes: scaled() Z (float m_last_layer_z for in-layer travels), inserting the slope-end point wherever it does not
// Changes: coincide with a path point, including at the very end, and keeps Marlin 2's parabola blend.
// Source: prusaslicer@b028299c77 src/libslic3r/GCode.cpp :: GCodeGenerator::needs_retraction
// Changes: Normal Prusa support travels inside the union of support islands skip retraction;
// Changes: interface travels still retract. Organic retains its previously validated motion.

//! Ramped lift reaches full clearance before entering an obstacle, and restores Z at destination.
use mica_core::{
    config::Config,
    geometry::{ExPolygon, Point, Polygon, SCALE},
    toolpath::{LayerToolpaths, Role},
};

pub struct TravelPlanner<'a> {
    layer: &'a LayerToolpaths,
    router: crate::avoid_crossing::AvoidCrossingPerimeters,
    previous: Option<&'a [ExPolygon]>,
    current: Option<&'a [ExPolygon]>,
    ramping: bool,
    obstacles: bool,
    vase: bool,
    lift: f64,
    slope: f64,
    orca: Option<crate::orca_motion::Travel>,
    normal_support: bool,
    support_islands: Vec<ExPolygon>,
    /// PrusaSlicer's avoid-crossing-perimeters planner (Prusa motion path only).
    prusa_acp: Option<crate::prusa_avoid_crossing::Planner>,
    acp_current: Option<crate::prusa_avoid_crossing::LayerGeometry<'a>>,
    acp_previous: Option<crate::prusa_avoid_crossing::LayerGeometry<'a>>,
    /// Marlin 2 machine limits for `get_smoothing_params` (PrusaSlicer rules only).
    smoothing: Option<SmoothingLimits>,
}
fn extruder_float(c: &Config, key: &str) -> f64 {
    c.float(&format!("filament_{key}"))
        .or_else(|| c.float(key))
        .unwrap_or(0.0)
}
fn ramping(c: &Config) -> bool {
    c.bool("filament_travel_ramping_lift")
        .or_else(|| c.bool("travel_ramping_lift"))
        .unwrap_or(false)
        && !mica_core::rules::Rules::of(c).is_orca()
}
/// The travel lift of the active tool at print height `z` (PrusaSlicer
/// `GCodeGenerator::travel_to_first_position`, and the ramping travel's maximum): the ramping
/// lift's maximum when ramping, else `retract_lift`; none outside the "Only lift Z above/below"
/// limits.
pub(crate) fn travel_lift(c: &Config, z: f64) -> f64 {
    let above = extruder_float(c, "retract_lift_above");
    let below = extruder_float(c, "retract_lift_below");
    if (above > 0.0 && z < above) || (below > 0.0 && z > below) {
        0.0
    } else {
        extruder_float(
            c,
            if ramping(c) {
                "travel_max_lift"
            } else {
                "retract_lift"
            },
        )
    }
}
/// The width Orca's travel geometry uses when `detect_overhangs_for_lift` has no configured
/// line width: the layer's first external wall, else 0.4 mm.
fn external_width(layer: &LayerToolpaths) -> f64 {
    layer
        .paths
        .iter()
        .find(|p| p.role == Role::ExternalPerimeter)
        .map_or(0.4, |p| p.flow.width)
}

/// Orca travel geometry computed ahead of the layer's planner, with the inputs it was computed
/// for: (external wall width, print_z) as bits.
pub(crate) struct ReadyGeometry {
    key: (u64, u64),
    geometry: std::sync::Arc<crate::orca_motion::TravelGeometry>,
}

/// The Orca travel geometry of layer `index` as [`TravelPlanner::with_source`] builds it, for a
/// layer whose paths are `layer` (only its height and first external wall width are read).
/// `None` without Orca motion.
pub(crate) fn orca_travel_geometry(
    c: &Config,
    source: &dyn mica_core::print::LayerSource,
    index: usize,
    layer: &LayerToolpaths,
    pairs: Option<&crate::orca_motion::OverhangPairs>,
) -> Option<ReadyGeometry> {
    let mut orca = crate::orca_motion::Travel::new(c)?;
    let width = orca.lift_width.unwrap_or_else(|| external_width(layer));
    let previous = source
        .previous_layer(index)
        .and_then(|i| source.layer_outlines(i));
    let current = source.layer_outlines(index);
    fill_geometry(
        &mut orca,
        source,
        index,
        (previous, current),
        layer.print_z,
        width,
        pairs,
    );
    Some(ReadyGeometry {
        key: (width.to_bits(), layer.print_z.to_bits()),
        geometry: orca.geometry,
    })
}

/// The source-dependent part of [`TravelPlanner::with_source`] for Orca motion.
fn fill_geometry(
    orca: &mut crate::orca_motion::Travel,
    source: &dyn mica_core::print::LayerSource,
    index: usize,
    (previous, current): (Option<&[ExPolygon]>, Option<&[ExPolygon]>),
    print_z: f64,
    width: f64,
    pairs: Option<&crate::orca_motion::OverhangPairs>,
) {
    let above = source
        .next_layer(index)
        .and_then(|i| source.layer_outlines(i));
    if let Some(islands) = source.support_islands(index) {
        orca.set_support_islands(islands);
    }
    let regions = source.region_slices(index);
    orca.geometry(
        previous,
        current,
        above,
        None,
        width,
        pairs.map(|p| (p, index)),
        regions.as_deref(),
    );
    // Orca protects overhangs in the preceding 0.4 mm, independent of layer height:
    // `Print::layers_sorted_for_object` takes the layers from the float
    // `print_z - 0.4f` up, so a layer exactly 0.4 mm below is in only when that float
    // rounds down. Within the same object: a sequential object's layers stop at its
    // first one.
    let start = f64::from((print_z as f32 - 0.4f32).max(0.));
    let mut upper = source.previous_layer(index);
    while let Some(i) = upper {
        let Some(below) = source.previous_layer(i) else {
            break;
        };
        if source.layer_z(i).0 < start {
            break;
        }
        if let (Some(upper), Some(lower)) = (source.layer_outlines(i), source.layer_outlines(below))
        {
            orca.add_overhangs(upper, lower, width, pairs.map(|p| (p, i)));
        }
        upper = Some(below);
    }
}

impl<'a> TravelPlanner<'a> {
    pub fn new(
        c: &Config,
        layer: &'a LayerToolpaths,
        previous: Option<&'a [ExPolygon]>,
        current: Option<&'a [ExPolygon]>,
    ) -> Self {
        // Orca rules lift by z_hop_types (orca_motion); Prusa's ramping switch is a PrusaSlicer rule.
        let ramping = ramping(c);
        let lift = travel_lift(c, layer.print_z);
        let lift =
            lift.min((c.float("max_print_height").unwrap_or(f64::MAX) - layer.print_z).max(0.0));
        let orca = crate::orca_motion::Travel::new(c);
        let smoothing = (orca.is_none() && c.string("gcode_flavor") == Some("marlin2"))
            .then(|| SmoothingLimits::new(c));
        Self {
            layer,
            // The Prusa path plans avoidance with the ported upstream planner instead.
            router: crate::avoid_crossing::AvoidCrossingPerimeters::new(
                c,
                current.filter(|_| orca.is_some()),
                layer
                    .paths
                    .iter()
                    .find(|p| p.role == Role::Perimeter)
                    .or_else(|| layer.paths.first())
                    .map_or(0.4, |p| p.flow.spacing()),
            ),
            previous,
            current,
            ramping,
            obstacles: c
                .bool("filament_travel_lift_before_obstacle")
                .or_else(|| c.bool("travel_lift_before_obstacle"))
                .unwrap_or(false),
            vase: c.bool("spiral_vase").unwrap_or(false),
            lift,
            slope: extruder_float(c, "travel_slope"),
            orca,
            normal_support: mica_core::rules::Rules::of(c).is_prusa()
                && matches!(c.string("support_material_style"), Some("grid" | "snug")),
            support_islands: Vec::new(),
            prusa_acp: None,
            acp_current: None,
            acp_previous: None,
            smoothing,
        }
    }
    /// Enable PrusaSlicer's avoid crossing perimeters for this layer (`init_layer`). `previous`
    /// is the layer GCodeGenerator is still on while it plans the layer-change travel.
    pub fn with_prusa_avoid_crossing(
        mut self,
        c: &Config,
        current: Option<crate::prusa_avoid_crossing::LayerGeometry<'a>>,
        previous: Option<crate::prusa_avoid_crossing::LayerGeometry<'a>>,
        external_width: f32,
    ) -> Self {
        if self.orca.is_none()
            && c.bool("avoid_crossing_perimeters").unwrap_or(false)
            && let Some(current) = current
        {
            let z_lift = extruder_float(c, "travel_max_lift") > 0.0;
            self.prusa_acp = Some(crate::prusa_avoid_crossing::Planner::new(
                c,
                current,
                external_width,
                z_lift,
            ));
            self.acp_current = Some(current);
            self.acp_previous = previous;
        }
        self
    }
    pub(crate) fn has_prusa_avoid_crossing(&self) -> bool {
        self.prusa_acp.is_some()
    }
    /// `AvoidCrossingPerimeters::travel_to` from `from` (the generator's last position) to `to`.
    /// `None` without avoidance or when no previous layer exists for a layer change.
    pub fn prusa_route(
        &self,
        from: (f64, f64),
        to: (f64, f64),
        use_external: bool,
        layer_change: bool,
    ) -> Option<(Vec<(f64, f64)>, bool)> {
        let acp = self.prusa_acp.as_ref()?;
        let generator = if layer_change {
            self.acp_previous?
        } else {
            self.acp_current?
        };
        let route = acp.travel_to(
            Point::from_mm(from.0, from.1),
            Point::from_mm(to.0, to.1),
            use_external,
            &generator,
        );
        let mut points: Vec<(f64, f64)> = route
            .points
            .iter()
            .map(|p| (p.x as f64 / SCALE, p.y as f64 / SCALE))
            .collect();
        // Keep the exact (unscaled) ends, like result_pl.front()/back() = start/end.
        points[0] = from;
        *points.last_mut().unwrap() = to;
        Some((points, route.wipe_disabled))
    }
    pub(crate) fn with_source(
        mut self,
        source: &dyn mica_core::print::LayerSource,
        index: usize,
        pairs: Option<&crate::orca_motion::OverhangPairs>,
    ) -> Self {
        if self.normal_support {
            self.support_islands = source.support_islands(index).unwrap_or_default();
        }
        if let Some(orca) = &mut self.orca {
            let width = orca
                .lift_width
                .unwrap_or_else(|| external_width(self.layer));
            fill_geometry(
                orca,
                source,
                index,
                (self.previous, self.current),
                self.layer.print_z,
                width,
                pairs,
            );
        }
        self
    }
    /// Reuse only within the same source layer. The geometry depends on that layer's
    /// outlines and first external-wall width, not on per-tool motion settings.
    pub(crate) fn with_source_cached(
        mut self,
        source: &dyn mica_core::print::LayerSource,
        index: usize,
        cache: &mut Option<std::sync::Arc<crate::orca_motion::TravelGeometry>>,
        pairs: Option<&crate::orca_motion::OverhangPairs>,
    ) -> Self {
        if let (Some(orca), Some(geometry)) = (&mut self.orca, cache.as_ref()) {
            orca.geometry = std::sync::Arc::clone(geometry);
            return self;
        }
        self = self.with_source(source, index, pairs);
        if let Some(orca) = &self.orca {
            *cache = Some(std::sync::Arc::clone(&orca.geometry));
        }
        self
    }
    /// [`Self::with_source_cached`] for the first planner of a layer, with the travel geometry
    /// computed ahead by [`orca_travel_geometry`]: used when it was computed for this layer's
    /// external wall width and height (the same inputs, so the same geometry).
    pub(crate) fn with_source_ready(
        mut self,
        source: &dyn mica_core::print::LayerSource,
        index: usize,
        cache: &mut Option<std::sync::Arc<crate::orca_motion::TravelGeometry>>,
        pairs: Option<&crate::orca_motion::OverhangPairs>,
        ready: Option<ReadyGeometry>,
    ) -> Self {
        if cache.is_none()
            && !self.normal_support
            && let Some(orca) = &mut self.orca
            && let Some(ready) = ready
        {
            let width = orca
                .lift_width
                .unwrap_or_else(|| external_width(self.layer));
            if ready.key == (width.to_bits(), self.layer.print_z.to_bits()) {
                orca.geometry = std::sync::Arc::clone(&ready.geometry);
                *cache = Some(ready.geometry);
                return self;
            }
        }
        self = self.with_source(source, index, pairs);
        if let Some(orca) = &self.orca {
            *cache = Some(std::sync::Arc::clone(&orca.geometry));
        }
        self
    }

    pub fn is_orca(&self) -> bool {
        self.orca.is_some()
    }
    pub fn needs_retraction(
        &self,
        from: (f64, f64),
        to: (f64, f64),
        role: Role,
        previous: Option<Role>,
        minimum: f64,
    ) -> bool {
        (if self.normal_support
            && matches!(role, Role::SupportMaterial | Role::SupportMaterialInterface)
        {
            self.distance(from, to) >= minimum
        } else {
            self.distance(from, to) > minimum
        }) && !(role == Role::SupportMaterial
            && !self.support_islands.is_empty()
            && mica_core::geometry::clip::diff_pl_is_empty(
                &[vec![
                    Point::from_mm(from.0, from.1),
                    Point::from_mm(to.0, to.1),
                ]],
                &self.support_islands,
            ))
            && self
                .orca
                .as_ref()
                .is_none_or(|o| o.needs_retraction(&self.router.route(from, to), role, previous))
    }
    pub fn emit(
        &self,
        w: &mut crate::writer::Writer,
        to: (f64, f64),
        printed: usize,
        retract: bool,
        speed: f64,
        z_speed: f64,
    ) {
        self.emit_probed(w, to, None, printed, retract, speed, z_speed);
    }
    /// `emit`, with `probe_from` the position the travel was planned from before a wipe moved
    /// the nozzle: Orca's Auto Lift tests that travel for overhangs (`GCode::travel_to`).
    #[allow(clippy::too_many_arguments)]
    pub fn emit_probed(
        &self,
        w: &mut crate::writer::Writer,
        to: (f64, f64),
        probe_from: Option<(f64, f64)>,
        printed: usize,
        retract: bool,
        speed: f64,
        z_speed: f64,
    ) {
        if let Some(from) = w.position() {
            if let Some(orca) = &self.orca {
                let probe = probe_from.and_then(|p| {
                    let route = self.router.route(p, to);
                    Some((*route.first()?, *route.get(1)?))
                });
                orca.emit(
                    w,
                    &self.router.route(from, to),
                    probe,
                    self.layer.print_z,
                    if retract { self.lift } else { 0. },
                    speed,
                    z_speed,
                );
            } else {
                w.travel_points(&self.plan(from, to, printed, retract), speed, z_speed);
            }
        }
    }
    /// Emit a travel along an explicit XY route (its first point is the current position).
    pub(crate) fn emit_route(
        &self,
        w: &mut crate::writer::Writer,
        route: &[(f64, f64)],
        printed: usize,
        retract: bool,
        speed: f64,
        z_speed: f64,
    ) {
        w.travel_points(&self.plan_route(route, printed, retract), speed, z_speed);
    }
    pub fn lift_height(&self) -> f64 {
        self.lift
    }

    /// Actual routed XY length, used for retraction and cooling decisions.
    pub fn distance(&self, from: (f64, f64), to: (f64, f64)) -> f64 {
        self.router
            .route(from, to)
            .windows(2)
            .map(|p| (p[1].0 - p[0].0).hypot(p[1].1 - p[0].1))
            .sum()
    }

    /// Waypoints after `from`, including the final unlift. Zero retraction means no lift.
    pub fn plan(
        &self,
        from: (f64, f64),
        to: (f64, f64),
        printed: usize,
        retract: bool,
    ) -> Vec<[f64; 3]> {
        self.plan_route(&self.router.route(from, to), printed, retract)
    }

    /// `plan` along a given route.
    pub fn plan_route(&self, route: &[(f64, f64)], printed: usize, retract: bool) -> Vec<[f64; 3]> {
        let z = self.layer.print_z;
        let route = route.to_vec();
        let length = route
            .windows(2)
            .map(|p| (p[1].0 - p[0].0).hypot(p[1].1 - p[0].1))
            .sum::<f64>();
        if !retract || self.lift <= 0.0 || length <= 1e-9 {
            return route.iter().skip(1).map(|p| [p.0, p.1, z]).collect();
        }
        let slope_end = self.orca.as_ref().map_or_else(
            || self.route_slope_end(&route, printed),
            |o| self.lift / o.slope.tan(),
        );
        if self.orca.is_none() {
            // generate_travel_to_extrusion: from z = scaled(float m_last_layer_z), back down to
            // the end point's scaled(m_last_layer_z) (computed in double).
            let layer_z = z as f32;
            let xy = scaled_path(&route);
            let params = self.elevation(self.lift, slope_end, scaled_length(&xy));
            let mut points = prusa_elevated_travel(&xy, scaled_f32_z(layer_z), &params);
            if let Some(last) = route.last() {
                points.push([last.0, last.1, scaled_z(f64::from(layer_z))]);
            }
            // Waypoints after `from`: the start is only a move for a vertical lift there.
            if params.height(0.) == 0. {
                points.remove(0);
            }
            return points;
        }
        routed_elevation(&route, z, self.lift, slope_end, z)
    }

    /// `get_elevated_traval_params` with `slope_end` already limited by obstacles.
    fn elevation(&self, lift: f64, slope_end: f64, length: f64) -> ElevatedTravel {
        let (blend, count) = self
            .smoothing
            .as_ref()
            .map_or((0., 1), |s| s.params(lift, slope_end, length));
        ElevatedTravel {
            lift,
            slope_end,
            blend,
            count,
        }
    }

    fn route_slope_end(&self, route: &[(f64, f64)], printed: usize) -> f64 {
        let mut limit = f64::INFINITY;
        let mut travelled = 0.;
        for edge in route.windows(2) {
            let length = (edge[1].0 - edge[0].0).hypot(edge[1].1 - edge[0].1);
            limit = limit.min(travelled + self.slope_end(edge[0], edge[1], printed, length));
            travelled += length;
        }
        limit
    }

    fn slope_end(&self, from: (f64, f64), to: (f64, f64), printed: usize, length: f64) -> f64 {
        let mut slope_end = if self.ramping
            && self.slope > 0.0
            && self.slope < 90.0
            && (!self.obstacles
                || (self.current.is_some() && (self.layer.index == 0 || self.previous.is_some())))
        {
            self.lift / self.slope.to_radians().tan()
        } else {
            0.0
        };
        if slope_end > 0.0 && self.obstacles {
            let a = Point::from_mm(from.0, from.1);
            let b = Point::from_mm(to.0, to.1);
            if let Some(previous) = self.previous {
                let crossings = previous
                    .iter()
                    .flat_map(|p| p.polygons())
                    .flat_map(|p| edges(p, true))
                    .filter_map(|(p, q)| intersection(a, b, p, q).map(|t| (t, true)))
                    .collect();
                slope_end =
                    slope_end.min(first_obstacle(crossings, contains(previous, a)) * length);
            }
            let crossings = self
                .layer
                .paths
                .iter()
                .enumerate()
                .filter(|(_, p)| p.role == Role::ExternalPerimeter)
                .flat_map(|(i, p)| {
                    edges(
                        &p.points,
                        p.points
                            .first()
                            .zip(p.points.last())
                            .is_some_and(|(a, b)| a.distance_to(*b) <= p.flow.width * SCALE),
                    )
                    .map(move |(p, q)| (i, p, q))
                })
                .filter_map(|(i, p, q)| intersection(a, b, p, q).map(|t| (t, i < printed)))
                .collect();
            slope_end = slope_end.min(
                first_obstacle(crossings, self.current.is_some_and(|p| contains(p, a))) * length,
            );
        }
        slope_end
    }

    /// Combine the layer rise with XY travel. Increase the slope when a short travel cannot
    /// reach the next layer, then lower only if the ramp rises above its target Z.
    pub fn layer_change(&self, from: [f64; 3], to: (f64, f64)) -> Option<Vec<[f64; 3]>> {
        self.layer_change_along(from, to, None)
    }
    /// `layer_change` along an explicit XY route from `from` to `to`. On the Prusa path
    /// `from[2]` is the previous layer's print_z (GCodeGenerator keeps it as a float).
    pub fn layer_change_along(
        &self,
        from: [f64; 3],
        to: (f64, f64),
        route: Option<Vec<(f64, f64)>>,
    ) -> Option<Vec<[f64; 3]>> {
        let from_z = if self.orca.is_none() {
            f64::from(from[2] as f32)
        } else {
            from[2]
        };
        let change = self.layer.print_z - from_z;
        if !self.ramping || self.vase || self.slope <= 0.0 || self.slope >= 90.0 || change <= 0.0 {
            return None;
        }
        let start = (from[0], from[1]);
        let route = route.unwrap_or_else(|| self.router.route(start, to));
        let length = route
            .windows(2)
            .map(|p| (p[1].0 - p[0].0).hypot(p[1].1 - p[0].1))
            .sum::<f64>();
        let mut slope_end = self.route_slope_end(&route, 0);
        if self.orca.is_none() {
            // get_layer_change_xy_path on scaled points (the start may be a quantized wipe end).
            let xy = scaled_path(&route);
            let length = scaled_length(&xy);
            // get_ramping_layer_change_params: the blend is planned for the configured lift.
            let mut params = self.elevation(self.lift, slope_end, length);
            params.lift = params.lift.max(change);
            let at_end = params.lift.min(params.lift / params.slope_end * length);
            if at_end < change {
                params.lift = change;
                params.slope_end = length;
            }
            let mut points = prusa_elevated_travel(&xy, from_z, &params);
            // travel_to_first_position: travel_to_z_force(scaled(print_z)).
            points.push([to.0, to.1, scaled_z(self.layer.print_z)]);
            return Some(points);
        }
        let mut lift = self.lift.max(change);
        if slope_end > 0.0 && lift / slope_end * length < change {
            lift = change;
            slope_end = length;
        }
        let mut points = vec![from];
        points.extend(routed_elevation(
            &route,
            from[2],
            lift,
            slope_end,
            self.layer.print_z,
        ));
        Some(points)
    }
}

fn routed_elevation(
    route: &[(f64, f64)],
    z: f64,
    lift: f64,
    slope_end: f64,
    final_z: f64,
) -> Vec<[f64; 3]> {
    if route.len() == 2 {
        return elevated_points(route[0], route[1], z, lift, slope_end, final_z);
    }
    let mut result = Vec::new();
    let mut travelled = 0.;
    if slope_end <= 1e-9 {
        result.push([route[0].0, route[0].1, z + lift]);
    }
    for edge in route.windows(2) {
        let length = (edge[1].0 - edge[0].0).hypot(edge[1].1 - edge[0].1);
        if travelled < slope_end && travelled + length > slope_end {
            let t = (slope_end - travelled) / length;
            result.push([
                edge[0].0 + t * (edge[1].0 - edge[0].0),
                edge[0].1 + t * (edge[1].1 - edge[0].1),
                z + lift,
            ]);
        }
        travelled += length;
        result.push([
            edge[1].0,
            edge[1].1,
            z + lift
                * if slope_end > 1e-9 {
                    (travelled / slope_end).min(1.)
                } else {
                    1.
                },
        ]);
    }
    if let Some(last) = route.last() {
        result.push([last.0, last.1, final_z]);
    }
    result
}

fn elevated_points(
    from: (f64, f64),
    to: (f64, f64),
    z: f64,
    lift: f64,
    slope_end: f64,
    final_z: f64,
) -> Vec<[f64; 3]> {
    let length = (to.0 - from.0).hypot(to.1 - from.1);
    if length <= 1e-9 {
        return vec![[to.0, to.1, final_z]];
    }
    let mut out = Vec::with_capacity(3);
    if slope_end <= 1e-9 {
        out.push([from.0, from.1, z + lift]);
        out.push([to.0, to.1, z + lift]);
    } else {
        if slope_end < length {
            let t = slope_end / length;
            out.push([
                from.0 + (to.0 - from.0) * t,
                from.1 + (to.1 - from.1) * t,
                z + lift,
            ]);
        }
        out.push([to.0, to.1, z + lift * (length / slope_end).min(1.0)]);
    }
    out.push([to.0, to.1, final_z]);
    out
}

/// `scaled<coord_t>(double)`: PrusaSlicer divides by SCALING_FACTOR and truncates.
fn scaled_trunc(v: f64) -> i64 {
    (v / 1e-6) as i64
}
/// A Z coordinate after `scaled()` and `unscaled()`: truncated to whole micrometres.
pub(crate) fn scaled_z(v: f64) -> f64 {
    scaled_trunc(v) as f64 * 1e-6
}
/// `scaled(float)`: the division runs in single precision.
fn scaled_f32_z(v: f32) -> f64 {
    (v / (1e-6_f64 as f32)) as i64 as f64 * 1e-6
}

/// `get_smoothing_params` inputs (index 0 of the machine limits).
#[derive(Clone, Copy, Debug)]
struct SmoothingLimits {
    max_xy_velocity: f64,
    xy_acceleration: f64,
    max_z_velocity: f64,
    z_acceleration: f64,
    z_jerk: f64,
}
impl SmoothingLimits {
    fn new(c: &Config) -> Self {
        let f = |k| c.float(k).unwrap_or(0.);
        let (x, y) = (f("machine_max_feedrate_x"), f("machine_max_feedrate_y"));
        Self {
            max_xy_velocity: (x * x + y * y).sqrt(),
            xy_acceleration: f("machine_max_acceleration_travel"),
            max_z_velocity: f("machine_max_feedrate_z"),
            z_acceleration: f("machine_max_acceleration_z"),
            z_jerk: f("machine_max_jerk_z"),
        }
    }
    /// (blend_width, parabola_points_count).
    fn params(&self, lift: f64, slope_end: f64, travel_length: f64) -> (f64, u32) {
        let slope = lift / slope_end;
        let xy_time = self.max_xy_velocity / self.xy_acceleration;
        let xy_distance = 1.0 / 2.0 * self.xy_acceleration * (xy_time * xy_time);
        if travel_length < xy_distance {
            return (0., 1);
        }
        let max_z_velocity = (self.max_xy_velocity * slope).min(self.max_z_velocity);
        let deceleration_time = max_z_velocity / self.z_acceleration;
        let deceleration_xy_distance = deceleration_time * self.max_xy_velocity;
        let blend = if slope_end > deceleration_xy_distance / 2.0 {
            deceleration_xy_distance
        } else {
            slope_end * 2.0
        };
        let count = if blend > 0. {
            (max_z_velocity / self.z_jerk).ceil()
        } else {
            1.
        };
        // Upstream converts to unsigned: NaN or a huge count disables the blend.
        let usable = blend > 0. && count > 0. && count <= 6.;
        if !usable {
            return (0., 1);
        }
        (blend, count as u32)
    }
}

/// `ElevatedTravelParams` with `ElevatedTravelFormula`.
#[derive(Clone, Copy, Debug)]
struct ElevatedTravel {
    lift: f64,
    slope_end: f64,
    blend: f64,
    count: u32,
}
impl ElevatedTravel {
    fn height(&self, d: f64) -> f64 {
        let (mut from, mut to) = (
            self.slope_end - self.blend / 2.0,
            self.slope_end + self.blend / 2.0,
        );
        if from < 0. {
            from = self.slope_end;
            to = self.slope_end;
        }
        if d > from && d < to {
            let slope = self.lift / self.slope_end;
            let a = -slope / 2.0 / self.blend;
            let b = slope * to / self.blend;
            let c = self.lift + a * (to * to);
            return a * d * d + b * d + c;
        }
        if d < self.slope_end {
            d / self.slope_end * self.lift
        } else {
            self.lift
        }
    }
    /// `linspace(slope_end - blend / 2, slope_end + blend / 2, parabola_points_count)`.
    fn ensure_points(&self) -> Vec<f64> {
        let (from, to) = (
            self.slope_end - self.blend / 2.0,
            self.slope_end + self.blend / 2.0,
        );
        match self.count {
            0 => vec![],
            1 => vec![(from + to) / 2.0],
            n => {
                let step = (to - from) / f64::from(n);
                (0..n - 1)
                    .map(|i| from + f64::from(i) * step)
                    .chain([to])
                    .collect()
            }
        }
    }
}

/// A route in scaled coordinates. Route points come from scaled geometry (exact multiples of
/// 1e-6 mm); other positions, such as an unquantized wipe end, are truncated like gcode_to_point.
/// Upstream works in object coordinates; truncation there differs only for negative ones.
fn scaled_path(route: &[(f64, f64)]) -> Vec<(i64, i64)> {
    let to_scaled = |v: f64| {
        let units = v * SCALE;
        if (units - units.round()).abs() < 1e-3 {
            units.round() as i64
        } else {
            scaled_trunc(v)
        }
    };
    route
        .iter()
        .map(|p| (to_scaled(p.0), to_scaled(p.1)))
        .collect()
}
/// `unscaled(Polyline::length())`.
fn scaled_length(xy: &[(i64, i64)]) -> f64 {
    let length: f64 = xy
        .windows(2)
        .map(|w| {
            let (dx, dy) = ((w[1].0 - w[0].0) as f64, (w[1].1 - w[0].1) as f64);
            (dx * dx + dy * dy).sqrt()
        })
        .sum();
    length * 1e-6
}

/// `generate_elevated_travel(slice_xy_path(...))` on the scaled path `xy`, in G-code
/// millimetres. The first point is the path start at `initial` height.
fn prusa_elevated_travel(
    xy: &[(i64, i64)],
    initial: f64,
    params: &ElevatedTravel,
) -> Vec<[f64; 3]> {
    let unscaled = |p: (i64, i64)| (p.0 as f64 * 1e-6, p.1 as f64 * 1e-6);
    let distances = params.ensure_points();
    let mut sliced = vec![(xy[0], 0.)];
    let mut previous = xy[0];
    let mut total = 0.;
    let mut offset = 0;
    for &point in &xy[1..] {
        let (px, py) = unscaled(point);
        let (qx, qy) = unscaled(previous);
        let (dx, dy) = (px - qx, py - qy);
        let segment = (dx * dx + dy * dy).sqrt();
        for &d in &distances[offset..] {
            if d > total + segment {
                break;
            }
            // place_at_segment: Eigen's normalized() leaves a zero vector unchanged.
            let (ux, uy) = if segment > 0. {
                (dx / segment, dy / segment)
            } else {
                (dx, dy)
            };
            let t = d - total;
            let place = (scaled_trunc(qx + ux * t), scaled_trunc(qy + uy * t));
            // Upstream's rounding sometimes leaves a point 1 µm short of the path point it falls
            // on (about once per Benchy); Mica treats a distance at the point as the point.
            if place != previous && place != point && t < segment - 1e-9 {
                sliced.push((place, d));
            }
            offset += 1;
        }
        total += segment;
        sliced.push((point, total));
        previous = point;
    }
    sliced
        .into_iter()
        .map(|((x, y), d)| {
            [
                x as f64 / SCALE,
                y as f64 / SCALE,
                scaled_z(initial + params.height(d)),
            ]
        })
        .collect()
}

fn edges(p: &[Point], close: bool) -> impl Iterator<Item = (Point, Point)> + '_ {
    p.windows(2)
        .map(|w| (w[0], w[1]))
        .chain((close && p.len() > 2).then(|| (*p.last().unwrap(), p[0])))
}
fn in_polygon(poly: &Polygon, p: Point) -> bool {
    let mut inside = false;
    for (a, b) in edges(poly, true) {
        if (a.y > p.y) != (b.y > p.y)
            && (p.x as f64)
                < (b.x - a.x) as f64 * (p.y - a.y) as f64 / (b.y - a.y) as f64 + a.x as f64
        {
            inside = !inside;
        }
    }
    inside
}
fn contains(polys: &[ExPolygon], p: Point) -> bool {
    polys
        .iter()
        .any(|ex| in_polygon(&ex.contour, p) && !ex.holes.iter().any(|h| in_polygon(h, p)))
}
fn intersection(a: Point, b: Point, p: Point, q: Point) -> Option<f64> {
    let v = ((b.x - a.x) as f64, (b.y - a.y) as f64);
    let w = ((q.x - p.x) as f64, (q.y - p.y) as f64);
    let d = v.0 * w.1 - v.1 * w.0;
    if d.abs() < 1e-6 {
        return None;
    }
    let u = ((p.x - a.x) as f64, (p.y - a.y) as f64);
    let t = (u.0 * w.1 - u.1 * w.0) / d;
    let s = (u.0 * v.1 - u.1 * v.0) / d;
    (t > 1e-9 && t <= 1.0 && (-1e-9..=1.0 + 1e-9).contains(&s)).then_some(t)
}
fn first_obstacle(mut crossings: Vec<(f64, bool)>, mut skip_exit: bool) -> f64 {
    crossings.sort_by(|a, b| a.0.total_cmp(&b.0));
    // Merge coincident crossings before testing the printed predicate.
    let mut groups: Vec<(f64, bool)> = Vec::new();
    for (t, printed) in crossings {
        if let Some(last) = groups.last_mut()
            && (t - last.0).abs() < 1e-9
        {
            last.1 |= printed;
        } else {
            groups.push((t, printed));
        }
    }
    for (t, printed) in groups {
        if skip_exit {
            skip_exit = false;
            continue;
        }
        if printed {
            return t;
        }
    }
    f64::INFINITY
}
/// XY length of a route, mm.
pub fn route_length(route: &[(f64, f64)]) -> f64 {
    route
        .windows(2)
        .map(|p| (p[1].0 - p[0].0).hypot(p[1].1 - p[0].1))
        .sum()
}
/// Constant-speed duration, including the configured Z velocity limit, used by cooling.
pub fn duration(from: [f64; 3], points: &[[f64; 3]], xy_speed: f64, z_speed: f64) -> f64 {
    let mut previous = from;
    points
        .iter()
        .map(|p| {
            let dz = (p[2] - previous[2]).abs();
            let length = (p[0] - previous[0]).hypot(p[1] - previous[1]).hypot(dz);
            previous = *p;
            (length / xy_speed).max(if z_speed > 0.0 { dz / z_speed } else { 0.0 })
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn normal_support_retraction_uses_support_islands_and_keeps_interface_retractions() {
        let mut c = Config::defaults();
        c.set_raw("support_material_style", "grid").unwrap();
        let layer = LayerToolpaths::default();
        let mut planner = TravelPlanner::new(&c, &layer, None, None);
        let mut island = square(0.);
        island.holes = vec![
            [(3., 3.), (3., 5.), (5., 5.), (5., 3.)]
                .map(|p| Point::from_mm(p.0, p.1))
                .to_vec(),
        ];
        planner.support_islands = vec![island, square(20.)];
        let base = Role::SupportMaterial;
        let interface = Role::SupportMaterialInterface;
        // An island-contained base travel skips retraction regardless of the previous role.
        assert!(!planner.needs_retraction((1., 1.), (9., 1.), base, Some(interface), 2.));
        assert!(planner.needs_retraction((1., 1.), (9., 1.), interface, Some(base), 2.));
        assert!(planner.needs_retraction((1., 4.), (9., 4.), base, Some(base), 2.)); // hole
        assert!(planner.needs_retraction((1., 1.), (21., 1.), base, Some(base), 2.)); // gap
        assert!(!planner.needs_retraction((9., 1.), (10.5, 1.), base, None, 2.));
        // The configured minimum is inclusive, as in PrusaSlicer needs_retraction.
        assert!(planner.needs_retraction((9., 1.), (11., 1.), base, None, 2.));
        // Neither avoidance nor only-retract-when-crossing controls the support-island rule.
        for (key, value) in [
            ("avoid_crossing_perimeters", "1"),
            ("only_retract_when_crossing_perimeters", "1"),
        ] {
            c.set_raw(key, value).unwrap();
        }
        let mut planner = TravelPlanner::new(&c, &layer, None, None);
        planner.support_islands = vec![square(0.)];
        assert!(!planner.needs_retraction((1., 1.), (9., 1.), base, None, 2.));
        c.set_raw("support_material_style", "organic").unwrap();
        assert!(!TravelPlanner::new(&c, &layer, None, None).normal_support);
        mica_core::rules::Rules::OrcaSlicer.set(&mut c);
        c.set_raw("support_material_style", "grid").unwrap();
        assert!(!TravelPlanner::new(&c, &layer, None, None).normal_support);
    }
    fn square(x: f64) -> ExPolygon {
        ExPolygon {
            contour: vec![
                Point::from_mm(x, 0.),
                Point::from_mm(x + 10., 0.),
                Point::from_mm(x + 10., 10.),
                Point::from_mm(x, 10.),
            ],
            holes: vec![],
        }
    }
    /// Prusa-path waypoints are truncated to whole micrometres like upstream's scaled points.
    fn close(a: [f64; 3], b: [f64; 3]) {
        assert!(
            a.iter().zip(&b).all(|(a, b)| (a - b).abs() < 2e-6),
            "{a:?} != {b:?}"
        );
    }
    #[test]
    fn slope_obstacles_limits_and_restore() {
        let mut c = Config::defaults();
        for (k, v) in [
            ("travel_ramping_lift", "1"),
            ("travel_lift_before_obstacle", "1"),
            ("travel_max_lift", "1"),
            ("travel_slope", "1"),
            ("max_print_height", "250"),
        ] {
            c.set_raw(k, v).unwrap();
        }
        let layer = LayerToolpaths {
            index: 1,
            print_z: 0.4,
            ..Default::default()
        };
        let outlines = vec![square(0.), square(12.)];
        let planner = TravelPlanner::new(&c, &layer, Some(&outlines), Some(&outlines));
        let points = planner.plan((5., 5.), (20., 5.), 0, true);
        // Ignore the source island's exit at x=10; reach full clearance at x=12.
        close(points[0], [12., 5., 1.4]);
        close(*points.last().unwrap(), [20., 5., 0.4]);
        let short = planner.plan((5., 5.), (6., 5.), 0, true);
        assert!((short[0][2] - 0.4 - 1_f64.to_radians().tan()).abs() < 2e-6);
        assert_eq!(
            planner.plan((5., 5.), (6., 5.), 0, false),
            vec![[6., 5., 0.4]]
        );
        c.set_raw("filament_travel_max_lift", "0.6").unwrap();
        let points = TravelPlanner::new(&c, &layer, None, None).plan((5., 5.), (6., 5.), 0, true);
        close(points[0], [5., 5., 1.0]); // missing outline information: conservative vertical lift
        c.set_raw("retract_lift_above", "1").unwrap();
        assert_eq!(
            TravelPlanner::new(&c, &layer, None, None).plan((5., 5.), (6., 5.), 0, true),
            vec![[6., 5., 0.4]]
        );
    }
    #[test]
    fn optional_obstacles_and_ramped_layer_change() {
        let mut c = Config::defaults();
        for (k, v) in [
            ("travel_ramping_lift", "1"),
            ("travel_max_lift", "0.6"),
            ("travel_slope", "1"),
            ("max_print_height", "250"),
        ] {
            c.set_raw(k, v).unwrap();
        }
        let layer = LayerToolpaths {
            index: 1,
            print_z: 0.4,
            ..Default::default()
        };
        // The CORE One profile leaves obstacle adjustment off: no outlines are needed.
        let planner = TravelPlanner::new(&c, &layer, None, None);
        let short = planner.plan((0., 0.), (1., 0.), 0, true);
        assert!((short[0][2] - 0.4 - 1_f64.to_radians().tan()).abs() < 2e-6);
        let change = planner.layer_change([0., 0., 0.2], (1., 0.)).unwrap();
        close(change[0], [0., 0., 0.2]); // emit the geometric start before the ramp
        close(change[1], [1., 0., 0.4]); // steeper ramp to reach the next layer
        let long = planner.layer_change([0., 0., 0.2], (100., 0.)).unwrap();
        close(long[0], [0., 0., 0.2]);
        assert!((long[1][2] - 0.8).abs() < 2e-6);
        close(*long.last().unwrap(), [100., 0., 0.4]);
        assert!(planner.layer_change([0., 0., 0.4], (100., 0.)).is_none());
        assert_eq!(first_obstacle(vec![(0.4, false), (0.4, true)], false), 0.4);
    }
    /// A CORE One Benchy layer change (0.4 -> 0.6, PrusaSlicer 2.9.6): Marlin 2's parabola blend
    /// around the slope end adds points at 29.0, 31.2 and 33.3 mm, then the arrival Z follows.
    #[test]
    fn ramped_layer_change_matches_upstream_segments() {
        let mut c = Config::defaults();
        for (k, v) in [
            ("gcode_flavor", "marlin2"),
            ("travel_ramping_lift", "1"),
            ("travel_max_lift", "0.6"),
            ("travel_slope", "1"),
            ("max_print_height", "250"),
            ("machine_max_feedrate_x", "350,160"),
            ("machine_max_feedrate_y", "350,160"),
            ("machine_max_feedrate_z", "12,12"),
            ("machine_max_acceleration_travel", "7000,2500"),
            ("machine_max_acceleration_z", "400,200"),
            ("machine_max_jerk_z", "2,2"),
        ] {
            c.set_raw(k, v).unwrap();
        }
        let layer = LayerToolpaths {
            index: 2,
            print_z: 0.2 + 0.2 + 0.2,
            ..Default::default()
        };
        let planner = TravelPlanner::new(&c, &layer, None, None);
        let points = planner
            .layer_change([102.704, 109.907, 0.2 + 0.2], (137.95, 109.95))
            .unwrap();
        // Upstream's points as written; this test starts from their rounded ends, so XY may
        // differ in the last digit.
        let expected = [
            (102.704, 109.907, ".4"),
            (131.733, 109.942, ".907"),
            (133.871, 109.945, ".94"),
            (136.009, 109.948, ".966"),
            (137.95, 109.95, ".984"),
            (137.95, 109.95, ".6"),
        ];
        assert_eq!(points.len(), expected.len());
        for (p, e) in points.iter().zip(expected) {
            assert!(
                (p[0] - e.0).abs() < 1.5e-3 && (p[1] - e.1).abs() < 1.5e-3,
                "{p:?}"
            );
            assert_eq!(crate::writer::num(p[2], 3), e.2);
        }
        // A steep short ramp (cube20 at 0.6 mm) ends on the first point without a second,
        // zero-length point at the slope end.
        let short = planner
            .layer_change(
                [133.877982, 117.788642, 0.2 + 0.2],
                (134.367923, 119.367923),
            )
            .unwrap();
        assert_eq!(short.len(), 3);
        assert!((short[1][2] - 0.6).abs() < 2e-6 && short[1][0] == 134.367923);
    }
}
