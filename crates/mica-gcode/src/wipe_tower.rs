// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/WipeTowerIntegration.cpp :: WipeTowerIntegration::append_tcr, tool_change, finalize
// Source: snapmaker-orca@b1831e5dcb src/libslic3r/GCode/WipeTower2.cpp :: WipeTowerWriter2::speed_override_backup, speed_override_restore, disable_linear_advance_value
// Changes: Structured volume moves replace G-code placeholder substitution; existing Mica writer owns offsets, retraction, stats and templates. U1 backs up M220; pressure advance is reset only for ramming.
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/WipeTower.cpp :: WipeTower::toolchange_Change, WipeTower::toolchange_Unload
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/CoolingBuffer.cpp :: CoolingBuffer::calculate_layer_slowdown
// Changes: Prusa towers: travel (object travel planner) before the tower G-code, forced XY/Z restore and deretraction after the toolchange, planner flushes; detour time in the cooling estimate.
// Changes: The Prusa wipe state follows append_tcr: wiped before the travel, reset for ramming, set to the tower's wipe_path afterwards at the object Z.
// Source: orca@8500fcdcca src/libslic3r/GCode.cpp :: WipeTowerIntegration::append_tcr2
// Changes: Orca (WipeTower2) visits with Orca motion settings go to orca_toolchange, with the Orca wipe state threaded through tower visits.
// Source: prusaslicer@b028299c77 src/libslic3r/GCode.cpp :: GCodeGenerator::travel_to_first_position
// Changes: The first travel to the tower (no known position) lifts by the ramping lift's maximum or retract_lift.
// Source: orca@8500fcdcca src/libslic3r/GCode/CoolingBuffer.cpp :: CoolingBuffer::parse_layer_gcode
// Changes: Orca towers time only their moves for cooling; machine_tool_change_time (a print time estimate setting) stays out.
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/WipeTower.cpp :: WipeTower::tool_change, WipeTower::finish_layer, merge_tcr
// Changes: Prusa tower exit: the toolchange trailer (M220 R, travel feed rate, G4 S0, G92 E0) precedes the joined finish_layer, finish-only visits have no speed-override/flush/E reset, the final unload does not re-enable filament monitoring, a zero wipe_tower_acceleration sets none and the toolchange keeps the writer's acceleration.

use crate::{
    export::ExportError,
    multi::Multi,
    travels::TravelPlanner,
    writer::{Writer, num},
};
use mica_core::{
    config::Config,
    wipe_tower::{TowerBlock, TowerKind, TowerMove, TowerVisit},
};
use mica_template::Context;
use std::collections::VecDeque;

#[derive(Default)]
pub(crate) struct State {
    layer: Option<usize>,
    visits: VecDeque<TowerVisit>,
}
impl Multi<'_> {
    pub(crate) fn tower_prime(
        &mut self,
        ctx: &mut Context,
        w: &mut Writer,
        c: &Config,
    ) -> Result<(), ExportError> {
        if let Some(plan) = &self.job.wipe_tower {
            let visits = plan.priming_visits();
            let mut prime_bounds: Option<mica_core::geometry::BoundingBox> = None;
            for v in visits {
                let points: Vec<_> = v
                    .after
                    .moves
                    .iter()
                    .filter(|m| m.volume > 0.)
                    .map(|m| mica_core::geometry::Point::from_mm(m.xy[0], m.xy[1]))
                    .collect();
                if let Some(b) = mica_core::geometry::BoundingBox::from_points(&points) {
                    if let Some(a) = &mut prime_bounds {
                        a.min.x = a.min.x.min(b.min.x);
                        a.min.y = a.min.y.min(b.min.y);
                        a.max.x = a.max.x.max(b.max.x);
                        a.max.y = a.max.y.max(b.max.y);
                    } else {
                        prime_bounds = Some(b);
                    }
                }
                let z = v.z;
                // Priming wipe paths are not carried into the first layer.
                let mut wipe = crate::prusa_wipe::Wipe::default();
                let mut orca_wipe = crate::orca_motion::Wipe::default();
                self.emit_tower(
                    v,
                    0,
                    z,
                    Route::default(),
                    &mut wipe,
                    ctx,
                    w,
                    c,
                    &mut orca_wipe,
                )?;
            }
            if let Some(prime) = prime_bounds
                && c.string("gcode_flavor") == Some("marlin2")
            {
                use mica_core::print::LayerSource;
                let overlap = self.job.first_layer_bbox().is_some_and(|b| {
                    prime.min.x <= b.max.x
                        && prime.max.x >= b.min.x
                        && prime.min.y <= b.max.y
                        && prime.max.y >= b.min.y
                });
                self.retract_current(w);
                w.raw("M300 S800 P500");
                w.raw(if overlap {
                    "M1 Remove priming towers and click button."
                } else {
                    "M1 S10"
                });
            }
        }
        Ok(())
    }
    /// Tower moves cannot be slowed with object extrusion, but do contribute to cooling time.
    /// `paths` lists the layer's model paths in print order as (tool, start, end), so the
    /// detours to and from the tower count like CoolingBuffer's non-adjustable travel time.
    pub(crate) fn tower_fixed_time(
        &mut self,
        layer: usize,
        c: &Config,
        paths: &[(usize, [f64; 2], [f64; 2])],
    ) -> f64 {
        let Some(plan) = &self.job.wipe_tower else {
            return 0.;
        };
        self.tower_state.layer = Some(layer);
        self.tower_state.visits = plan.visits(layer).into();
        let mut seconds = 0.;
        for v in &self.tower_state.visits {
            for (b, tool) in v
                .before
                .iter()
                .map(|b| (b, v.old_tool))
                .chain(std::iter::once((&v.after, v.tool)))
            {
                let mut pos = b.start;
                let s = &self.tools[tool].settings;
                for m in &b.moves {
                    let duration = (m.xy[0] - pos[0]).hypot(m.xy[1] - pos[1]) / m.speed;
                    seconds += if !m.ramming && s.filament_max_volumetric_speed > 0. {
                        duration.max(m.volume / s.filament_max_volumetric_speed)
                    } else {
                        duration
                    };
                    pos = m.xy;
                }
            }
            if v.change {
                // Orca's CoolingBuffer times only the layer's moves and dwells:
                // machine_tool_change_time is a print time estimate setting.
                if plan.settings.kind == TowerKind::Prusa {
                    seconds += c.float("machine_tool_change_time").unwrap_or(0.);
                }
                seconds += c
                    .float_at("retract_length_toolchange", v.old_tool)
                    .unwrap_or(0.)
                    / self.tools[v.old_tool].settings.retract_speed;
                seconds += c
                    .float_at("retract_length_toolchange", v.tool)
                    .unwrap_or(0.)
                    / self.tools[v.tool].settings.deretract_speed;
            }
        }
        if plan.settings.kind == TowerKind::Prusa {
            seconds += self.tower_detour_time(layer, paths);
        }
        seconds
    }
    /// Travel from the model to each tower visit and back, minus the direct travel the model
    /// estimate already counts, plus the retraction around the travel to the tower.
    fn tower_detour_time(&self, layer: usize, paths: &[(usize, [f64; 2], [f64; 2])]) -> f64 {
        let tools = &self.job.ordering.layers[layer].extruders;
        let rank = |t: usize| tools.iter().position(|x| *x == t).unwrap_or(usize::MAX);
        let ends = |v: &TowerVisit| {
            let start = v.before.as_ref().map_or(v.after.start, |b| b.start);
            let end = v
                .after
                .moves
                .last()
                .or_else(|| v.before.as_ref().and_then(|b| b.moves.last()))
                .map_or(start, |m| m.xy);
            (start, end)
        };
        let dist = |a: [f64; 2], b: [f64; 2]| (a[0] - b[0]).hypot(a[1] - b[1]);
        let mut visits = self.tower_state.visits.iter().peekable();
        let mut active = self.active;
        let mut position: Option<[f64; 2]> = None;
        let mut seconds = 0.;
        let mut detour = |from: Option<[f64; 2]>,
                          to: Option<[f64; 2]>,
                          visits: &mut dyn Iterator<Item = &TowerVisit>,
                          active: &mut usize| {
            let mut pos = from;
            let mut length = 0.;
            let mut any = false;
            for v in visits {
                any = true;
                let (start, end) = ends(v);
                let s = &self.tools[*active].settings;
                if let Some(p) = pos {
                    length += dist(p, start);
                    seconds +=
                        s.retract_length / s.retract_speed + s.retract_length / s.deretract_speed;
                }
                pos = Some(end);
                *active = v.tool;
            }
            if any && let (Some(p), Some(q)) = (pos, to) {
                length += dist(p, q) - from.map_or(0., |f| dist(f, q));
            }
            seconds += length.max(0.) / self.tools[*active].settings.travel_speed;
        };
        for &(tool, start, end) in paths {
            let mut due = Vec::new();
            // Same queue rule as tower_before_tool.
            while let Some(v) = visits
                .next_if(|v| tools.iter().position(|x| *x == v.tool).unwrap_or(0) <= rank(tool))
            {
                due.push(v);
            }
            if !due.is_empty() {
                detour(position, Some(start), &mut due.into_iter(), &mut active);
            }
            position = Some(end);
        }
        detour(position, None, &mut visits, &mut active);
        seconds
    }
    /// Emit pending tower-only tools as well as the visit belonging to the next model tool.
    /// `wipe` is the Prusa wipe path (`GCodeGenerator::m_wipe`).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn tower_before_tool(
        &mut self,
        tool: usize,
        layer: usize,
        z: f64,
        route: Route<'_, '_>,
        wipe: &mut crate::prusa_wipe::Wipe,
        ctx: &mut Context,
        w: &mut Writer,
        c: &Config,
        orca_wipe: &mut crate::orca_motion::Wipe,
    ) -> Result<ToolSwitch, ExportError> {
        let Some(plan) = &self.job.wipe_tower else {
            let changed = tool != self.active;
            self.change(tool, layer, z, wipe, ctx, w, c, orca_wipe)?;
            return Ok(ToolSwitch {
                tower: false,
                changed,
            });
        };
        if self.tower_state.layer != Some(layer) {
            self.tower_state.layer = Some(layer);
            self.tower_state.visits = plan.visits(layer).into();
        }
        let tools = &self.job.ordering.layers[layer].extruders;
        let rank = tools.iter().position(|t| *t == tool).unwrap_or(usize::MAX);
        let mut emitted = false;
        while self
            .tower_state
            .visits
            .front()
            .is_some_and(|v| tools.iter().position(|t| *t == v.tool).unwrap_or(0) <= rank)
        {
            let v = self.tower_state.visits.pop_front().unwrap();
            self.emit_tower(v, layer, z, route, wipe, ctx, w, c, orca_wipe)?;
            emitted = true;
        }
        let changed = tool != self.active;
        self.change(tool, layer, z, wipe, ctx, w, c, orca_wipe)?;
        Ok(ToolSwitch {
            tower: emitted,
            changed,
        })
    }
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn tower_finish_layer(
        &mut self,
        layer: usize,
        z: f64,
        route: Route<'_, '_>,
        wipe: &mut crate::prusa_wipe::Wipe,
        ctx: &mut Context,
        w: &mut Writer,
        c: &Config,
        orca_wipe: &mut crate::orca_motion::Wipe,
    ) -> Result<bool, ExportError> {
        let Some(plan) = &self.job.wipe_tower else {
            return Ok(false);
        };
        if self.tower_state.layer != Some(layer) {
            self.tower_state.layer = Some(layer);
            self.tower_state.visits = plan.visits(layer).into();
        }
        let mut emitted = false;
        while let Some(v) = self.tower_state.visits.pop_front() {
            self.emit_tower(v, layer, z, route, wipe, ctx, w, c, orca_wipe)?;
            emitted = true;
        }
        Ok(emitted)
    }
    pub(crate) fn tower_finalize(
        &mut self,
        z: f64,
        h: f64,
        ctx: &mut Context,
        w: &mut Writer,
        c: &Config,
    ) -> Result<(), ExportError> {
        if let Some(plan) = &self.job.wipe_tower {
            let final_z = if plan
                .layers
                .last()
                .is_some_and(|l| (l.print_z - z).abs() < 1e-4)
            {
                z + h
            } else {
                z
            };
            if let Some(v) = plan.final_ram(self.active, final_z, h) {
                let layer = self.job.ordering.layers.len() - 1;
                // The final purge has no wipe path; the object's was wiped before.
                let mut wipe = crate::prusa_wipe::Wipe::default();
                let mut orca_wipe = crate::orca_motion::Wipe::default();
                self.emit_tower(
                    v,
                    layer,
                    final_z,
                    Route::default(),
                    &mut wipe,
                    ctx,
                    w,
                    c,
                    &mut orca_wipe,
                )?;
            }
        }
        Ok(())
    }
    #[allow(clippy::too_many_arguments)]
    fn emit_tower(
        &mut self,
        v: TowerVisit,
        layer: usize,
        object_z: f64,
        route: Route<'_, '_>,
        wipe: &mut crate::prusa_wipe::Wipe,
        ctx: &mut Context,
        w: &mut Writer,
        c: &Config,
        orca_wipe: &mut crate::orca_motion::Wipe,
    ) -> Result<(), ExportError> {
        let plan = self.job.wipe_tower.as_ref().unwrap();
        let orca = plan.settings.kind == TowerKind::Orca;
        if orca && crate::orca_motion::enabled(c) {
            return self.emit_tower_orca(v, layer, object_z, route, ctx, w, c, orca_wipe);
        }
        let backup = !orca
            || c.string("printer_model")
                .is_some_and(|s| s.contains("Snapmaker") && s.contains("U1"));
        let monitor = !orca
            && c.string("printer_notes")
                .is_some_and(|s| s.contains("PRINTER_MODEL_XL"));
        let change_pressure = orca
            && mica_core::wipe_tower::number(c, "enable_change_pressure_when_wiping", 0.) != 0.;
        self.end_label(w, c);
        // WipeTowerIntegration::append_tcr: the travel to the tower (with the current tool's
        // retraction, ramping lift and deretraction) precedes the tower's own G-code. Without
        // ramming, a toolchange at the same Z happens in place and the tower travels afterwards.
        let travels_first = !orca
            && (v.before.is_some() || !v.change || v.z < object_z - 1e-5)
            && (v.before.is_some() || !v.after.moves.is_empty());
        if travels_first {
            let start = v.before.as_ref().map_or(v.after.start, |b| b.start);
            self.travel_to_tower(w, route, wipe, start, v.z);
        }
        if v.before
            .as_ref()
            .is_some_and(|b| b.moves.iter().any(|m| m.ramming))
        {
            // "We don't want wiping on the ramming lines."
            wipe.reset();
        }
        let tower_accel = c.float("wipe_tower_acceleration").filter(|v| *v > 0.);
        if !orca
            && self.tools[self.active].settings.default_acceleration > 0.
            && let Some(accel) = tower_accel
        {
            // append_tcr sets the tower acceleration before the tower G-code; zero sets none.
            w.set_acceleration(accel);
        }
        w.raw(&format!(
            ";HEIGHT:{}\n;TYPE:{}\n; CP WIPE TOWER START",
            num(v.height, 6),
            if orca { "Prime tower" } else { "Wipe tower" }
        ));
        // Prusa: only WipeTower::tool_change results back up and restore the speed override,
        // flush and reset E; a finish_layer result alone has none of it. merge_tcr appends
        // the finish_layer G-code after the toolchange's (or the toolchange's after it).
        let toolchange = orca || v.change || v.before.is_some();
        let finish_first = v
            .before
            .as_ref()
            .and_then(|b| b.joined)
            .filter(|_| !orca && v.change)
            .map_or(0, |k| k + 1);
        let header = |w: &mut Writer| {
            if backup {
                w.raw("M220 B");
            }
            w.raw("M220 S100");
        };
        if toolchange && finish_first == 0 {
            header(w);
        }
        if orca {
            let accel =
                tower_accel.unwrap_or(self.tools[self.active].settings.default_acceleration);
            w.set_acceleration(accel);
        }
        if let Some(b) = &v.before {
            let mut moves = &b.moves[..];
            if finish_first > 0 {
                // merge_tcr(finish_layer, tool_change): the finish and the travel to the
                // toolchange's start come first.
                block(
                    self,
                    w,
                    &TowerBlock {
                        start: b.start,
                        moves: moves[..finish_first].to_vec(),
                        ..TowerBlock::default()
                    },
                    orca,
                );
                moves = &moves[finish_first..];
                header(w);
            }
            if orca {
                travel(self, w, b.start, v.z);
            } else if let Some(first) = moves.first().filter(|m| m.volume <= 0.) {
                // WipeTower::toolchange_Unload travels to the ramming start before
                // disabling linear advance.
                w.travel_to(first.xy[0], first.xy[1], first.speed);
                moves = &moves[1..];
            }
            if b.moves.iter().any(|m| m.ramming) {
                if !orca {
                    w.raw("M900 K0");
                } else if change_pressure {
                    w.set_pressure_advance(mica_core::wipe_tower::number(
                        c,
                        "ramming_pressure_advance_value",
                        0.,
                    ));
                }
                if monitor {
                    w.raw("G4 S0\nM591 S0");
                }
            }
            block(
                self,
                w,
                &TowerBlock {
                    start: b.start,
                    moves: moves.to_vec(),
                    ..TowerBlock::default()
                },
                orca,
            );
            if !orca {
                // toolchange_Unload ends with flush_planner_queue.
                w.raw("G4 S0");
            }
        } else if orca && v.z < object_z - 1e-5 {
            // Move outside the objects before descending to a sparse-free tower.
            travel(self, w, v.after.start, v.z);
        }
        if v.change {
            let accel = w.acceleration_state();
            self.change(v.tool, layer, object_z, wipe, ctx, w, c, orca_wipe)?;
            if !orca {
                // GCodeWriter::toolchange keeps the last acceleration, so append_tcr's
                // set_print_acceleration after the tower writes nothing when it is unchanged.
                w.restore_acceleration(accel);
            }
        }
        if monitor && v.change {
            // toolchange_Change re-enables filament monitoring; the final unload does not.
            w.raw("G4 S0\nM591 R");
        }
        if !orca && v.change {
            // WipeTower::toolchange_Change: the toolchange G-code may have moved the head, so
            // travel (never skipped) to where the tower expects it, then restore the layer Z
            // and deretract the new tool ([deretraction_from_wipe_tower_generator]) and flush.
            let s = &self.tools[self.active].settings;
            w.travel_to_xy_force(v.after.start[0], v.after.start[1], s.travel_speed);
            w.move_z_force(v.z, z_speed(s));
            w.unretract(s.deretract_speed);
            w.raw("G4 S0");
        }
        // WipeTower::tool_change ends with speed_override_restore, feedrate(travel speed),
        // flush_planner_queue and reset_extruder, before a joined finish_layer.
        let travel_speed = self.tools[self.active].settings.travel_speed;
        let trailer = |w: &mut Writer| {
            if backup {
                w.raw("M220 R");
            }
            if !orca {
                w.feedrate_if_changed(travel_speed);
            }
            w.raw("G4 S0\nG92 E0");
        };
        let joined = v.after.joined.filter(|_| !orca && toolchange);
        if !v.after.moves.is_empty() {
            if orca {
                travel(self, w, v.after.start, v.z);
            }
            match joined {
                Some(k) => {
                    let (change, finish) = v.after.moves.split_at(k);
                    let part = |moves: &[TowerMove]| TowerBlock {
                        start: v.after.start,
                        moves: moves.to_vec(),
                        ..TowerBlock::default()
                    };
                    block(self, w, &part(change), orca);
                    trailer(w);
                    block(self, w, &part(finish), orca);
                }
                None => block(self, w, &v.after, orca),
            }
        }
        if toolchange && joined.is_none() {
            trailer(w);
        }
        w.raw("; CP WIPE TOWER END");
        w.set_acceleration(self.tools[self.active].settings.default_acceleration);
        if change_pressure
            && c.bool_at("enable_pressure_advance", self.active)
                .unwrap_or(false)
        {
            w.set_pressure_advance(c.float_at("pressure_advance", self.active).unwrap_or(0.));
        }
        if (v.z - object_z).abs() > 1e-5 {
            self.retract_current(w);
            w.move_z(object_z, z_speed(&self.tools[self.active].settings));
            wipe.reset();
        } else if !orca {
            // append_tcr: "Prepare a future wipe" along the tower's wipe path.
            wipe.set_path(&v.wipe_path);
        }
        Ok(())
    }
}
/// The object layer's travel planners (one per tool) and the number of paths printed so far,
/// so tower travels use the same ramping lift and routing as object travels.
#[derive(Clone, Copy, Default)]
pub(crate) struct Route<'p, 'l> {
    pub planners: Option<&'p [TravelPlanner<'l>]>,
    pub printed: usize,
}
/// What `tower_before_tool` did.
pub(crate) struct ToolSwitch {
    /// Tower G-code was emitted (the position is the tower's end).
    pub tower: bool,
    /// The tool changed (`set_extruder`), either inside a tower visit or in place.
    pub changed: bool,
}
impl Multi<'_> {
    /// `append_tcr`: retract_and_wipe, `travel_to` the tower start, travel down to a lower
    /// tower layer if needed, and unretract.
    fn travel_to_tower(
        &self,
        w: &mut Writer,
        route: Route<'_, '_>,
        wipe: &mut crate::prusa_wipe::Wipe,
        xy: [f64; 2],
        z: f64,
    ) {
        let t = &self.tools[self.active];
        let s = &t.settings;
        let speed_z = z_speed(s);
        // retract_and_wipe, then Wipe::wipe forgets the path.
        if let Some(plan) = crate::prusa_wipe::Params::new(&t.config, s)
            .filter(|_| wipe.has_path())
            .zip(w.position())
            .map(|(p, from)| wipe.plan(from, w.retracted(), s.retract_length, &p))
        {
            crate::prusa_wipe::emit(w, &plan, s.retract_speed);
            wipe.reset();
        }
        self.retract_current(w);
        // append_tcr travels with use_external_mp_once: avoidance plans between objects.
        let external = route
            .planners
            .and_then(|p| p.get(self.active))
            .zip(w.position())
            .and_then(|(planner, from)| planner.prusa_route(from, (xy[0], xy[1]), true, false));
        match (
            route.planners.and_then(|p| p.get(self.active)),
            w.position(),
        ) {
            (Some(planner), Some(_)) if external.is_some() => planner.emit_route(
                w,
                &external.unwrap().0,
                route.printed,
                s.retract_length > 0.,
                s.travel_speed,
                speed_z,
            ),
            (Some(planner), Some(_)) => planner.emit(
                w,
                (xy[0], xy[1]),
                route.printed,
                s.retract_length > 0.,
                s.travel_speed,
                speed_z,
            ),
            _ => {
                // travel_to_first_position: lift by the ramping lift's maximum (or
                // retract_lift), across, then down.
                let lift = if s.retract_length > 0. {
                    crate::travels::travel_lift(&t.config, z)
                } else {
                    0.
                };
                let safe = w.z().max(z) + lift;
                w.unlift(speed_z);
                w.move_z(safe, speed_z);
                w.travel_to(xy[0], xy[1], s.travel_speed);
            }
        }
        w.unlift(speed_z);
        // "Travel down to the last wipe tower layer."
        w.move_z(z, speed_z);
        w.unretract(s.deretract_speed);
    }
}
fn travel(m: &Multi<'_>, w: &mut Writer, xy: [f64; 2], z: f64) {
    let s = &m.tools[m.active].settings;
    let speed_z = z_speed(s);
    m.retract_current(w);
    // Lift first, travel in XY, then descend; tool templates may have changed actual XYZ.
    let safe = w.z().max(z) + s.retract_lift;
    w.unlift(speed_z);
    w.move_z(safe, speed_z);
    w.travel_to(xy[0], xy[1], s.travel_speed);
    w.move_z(z, speed_z);
    w.unretract(s.deretract_speed);
}
fn z_speed(s: &mica_core::settings::PrintSettings) -> f64 {
    if s.travel_speed_z > 0. {
        s.travel_speed_z
    } else {
        s.travel_speed
    }
}
pub(crate) fn block(m: &mut Multi<'_>, w: &mut Writer, b: &TowerBlock, orca: bool) {
    let s = &m.tools[m.active].settings;
    let filament_area = std::f64::consts::PI * (s.filament_diameter / 2.).powi(2);
    let mut width = None;
    let mut used = 0.;
    for mov in &b.moves {
        if mov.volume > 0. {
            if width != Some(mov.width) {
                w.raw(&format!(";WIDTH:{}", num(mov.width, 6)));
                width = Some(mov.width);
            }
            let mut speed = mov.speed;
            if !mov.ramming
                && s.filament_max_volumetric_speed > 0.
                && let Some(pos) = w.position()
            {
                let len = (mov.xy[0] - pos.0).hypot(mov.xy[1] - pos.1);
                speed = speed.min(s.filament_max_volumetric_speed * len / mov.volume);
            }
            let e = mov.volume / filament_area;
            w.extrude_to(mov.xy[0], mov.xy[1], e, speed.max(0.01));
            used += e;
        } else {
            w.travel_to(mov.xy[0], mov.xy[1], mov.speed);
        }
    }
    m.record_tower(if orca { "Prime tower" } else { "Wipe tower" }, used);
}
