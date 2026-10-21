// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: orca@8500fcdcca src/libslic3r/GCode.cpp :: GCode::set_extruder, GCode::retract, OozePrevention::pre_toolchange, OozePrevention::post_toolchange
// Source: orca@8500fcdcca src/libslic3r/GCode.cpp :: WipeTowerIntegration::append_tcr2, WipeTowerIntegration::post_process_wipe_tower_moves
// Source: orca@8500fcdcca src/libslic3r/GCodeWriter.cpp :: GCodeWriter::eager_lift, GCodeWriter::retract_for_toolchange
// Source: orca@8500fcdcca src/libslic3r/GCode/WipeTower2.cpp :: WipeTower2::tool_change, WipeTower2::toolchange_Unload, WipeTower2::toolchange_Change, WipeTower2::finish_layer
// Source: orca@8500fcdcca src/libslic3r/GCode/CoolingBuffer.cpp :: CoolingBuffer::apply_layer_cooldown
// Source: snapmaker-orca@b1831e5dcb src/libslic3r/GCode/WipeTower2.cpp :: WipeTowerWriter2::speed_override_backup, speed_override_restore
// Changes: Orca toolchanges and prime-tower visits on Mica's writer: the toolchange retraction wipes along the last path, the eager spiral lift precedes change_filament_gcode, and the tower's post-processed travels, Z restore, deretraction and wipe path follow append_tcr2. Fans resume at ;_FORCE_RESUME_FAN_SPEED. Mica keeps its tower markers and ends object labels before the tower.
// Source: orca@8500fcdcca src/libslic3r/GCode/CoolingBuffer.cpp :: CoolingBuffer::parse_layer_gcode
// Changes: change_extruder_set_fan writes the new filament's fans before the first line selecting another tool (TYPE_SET_TOOL).

use crate::{
    export::ExportError,
    multi::{Multi, run},
    orca_motion::Wipe,
    wipe_tower::Route,
    writer::{Writer, num},
};
use mica_core::{config::Config, wipe_tower::TowerVisit};
use mica_template::{Context, Var};

/// CoolingBuffer::parse_layer_gcode's `TYPE_SET_TOOL`: the byte offset of the first line that
/// starts with `T` and a tool number other than `current`, below `tools`.
fn tool_line(text: &str, current: usize, tools: usize) -> Option<usize> {
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let start = offset;
        offset += line.len();
        let Some(rest) = line.strip_prefix('T') else {
            continue;
        };
        let digits = rest.len() - rest.trim_start_matches(|c: char| c.is_ascii_digit()).len();
        if let Ok(tool) = rest[..digits].parse::<usize>()
            && tool != current
            && tool < tools
        {
            return Some(start);
        }
    }
    None
}

fn z_speed(s: &mica_core::settings::PrintSettings) -> f64 {
    if s.travel_speed_z > 0. {
        s.travel_speed_z
    } else {
        s.travel_speed
    }
}

impl Multi<'_> {
    /// `OozePrevention::_get_temp`: the first layer (or a zero other-layers value) uses the
    /// first-layer temperature.
    fn orca_temperature(c: &Config, tool: usize, layer: usize) -> f64 {
        let temperature = c.float_at("temperature", tool).unwrap_or(0.);
        if layer == 0 || temperature == 0. {
            c.float_at("first_layer_temperature", tool).unwrap_or(0.)
        } else {
            temperature
        }
    }

    /// Orca `GCode::set_extruder` for independent tools. `z` is the toolchange print Z.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn change_orca(
        &mut self,
        next: usize,
        layer: usize,
        z: f64,
        ctx: &mut Context,
        w: &mut Writer,
        c: &Config,
        wipe: &mut Wipe,
    ) -> Result<(), ExportError> {
        self.end_label(w, c);
        let prev = self.active;
        // retract(true): wipe along the last path and retract_for_toolchange (`c` carries the
        // filament overrides). Extruder::retract replaces the restart extra.
        let length = c
            .float_at("retract_length_toolchange", prev)
            .unwrap_or(self.tools[prev].settings.retract_length);
        if length > w.retracted() {
            w.set_restart_extra(
                c.float_at("retract_restart_extra_toolchange", prev)
                    .unwrap_or(0.),
            );
        }
        {
            let t = &self.tools[prev];
            wipe.retract_length(w, &t.config, &t.settings, length);
        }
        // "Always reset the extrusion path, even if the tool change retract is set to zero."
        wipe.reset();
        ctx.set("filament_extruder_id", Var::int(prev as i64));
        let end = run(
            "end_filament_gcode",
            c.string_at("end_filament_gcode", prev).unwrap_or(""),
            ctx,
            w,
            c,
        )
        .map_err(ExportError::Template)?;
        if !end.is_empty() {
            w.raw(&end);
        }
        let ooze = c.bool("ooze_prevention").unwrap_or(false);
        let delta = c.float("standby_temperature_delta").unwrap_or(0.);
        if ooze {
            let idle = c.float_at("idle_temperature", prev).unwrap_or(0.);
            let standby = if idle != 0. {
                Some(idle)
            } else if delta != 0. {
                Some(Self::orca_temperature(c, prev, layer) + delta)
            } else {
                None
            };
            if let Some(t) = standby {
                // GCodeWriter::set_temperature plus OozePrevention's ";cooldown" marker.
                w.raw(&format!(
                    "M104 S{} T{prev} ; set nozzle temperature ;cooldown",
                    num(t.max(0.).trunc(), 0)
                ));
            }
        }
        self.changes += 1;
        for (key, value) in [
            ("previous_extruder", prev),
            ("next_extruder", next),
            ("toolchange_count", self.changes),
            ("current_extruder", prev),
        ] {
            ctx.set(key, Var::int(value as i64));
        }
        crate::templates::set_layer_vars(ctx, layer as i64, z, z);
        // "Move the lift gcode here which is in the change_filament_gcode originally":
        // retract(false, false, SpiralLift, true) is already retracted; it only lifts.
        {
            let t = &self.tools[prev];
            let above = c.float_at("retract_lift_above", prev).unwrap_or(0.);
            let below = c.float_at("retract_lift_below", prev).unwrap_or(0.);
            let lift = if w.z() >= above && (w.z() <= below || below == 0.) {
                t.settings.retract_lift
            } else {
                0.
            };
            let speed = if t.settings.travel_speed_z > 0. {
                t.settings.travel_speed_z
            } else if layer == 0 {
                c.float("initial_layer_travel_speed")
                    .unwrap_or(t.settings.travel_speed)
            } else {
                t.settings.travel_speed
            };
            crate::orca_motion::eager_spiral_lift(w, &t.config, lift, speed);
        }
        let tpl = c
            .string("toolchange_gcode")
            .filter(|s| !s.is_empty())
            .or_else(|| c.string("change_filament_gcode"))
            .unwrap_or("");
        let text = run("toolchange_gcode", tpl, ctx, w, c).map_err(ExportError::Template)?;
        let selects = text.lines().any(|l| {
            l.split(';').next().unwrap_or("").split_whitespace().next() == Some(&format!("T{next}"))
        });
        // CoolingBuffer's change_extruder_set_fan writes the new filament's fans before the
        // first line that selects another tool (Mica's own T line when the template has none).
        let fan = self.layer_fans.get(next).copied().unwrap_or(0.);
        let (tool_config, tools) = (&self.tools[next].config, self.tools.len());
        let set_fans = |w: &mut Writer| {
            w.with_orca_fan(|f, w| f.change_extruder(w, tool_config, next, layer, fan, false));
        };
        let split = tool_line(&text, prev, tools);
        match split {
            Some(at) => {
                if at > 0 {
                    w.raw(&text[..at]);
                }
                set_fans(w);
                w.raw(&text[at..]);
            }
            None if !text.is_empty() => w.raw(&text),
            None => {}
        }
        if !selects {
            if split.is_none() && next != prev {
                set_fans(w);
            }
            w.raw(&format!("T{next}"));
        }
        self.active = next;
        w.select_tool(next, c);
        if !text.is_empty() {
            // ;_FORCE_RESUME_FAN_SPEED: CoolingBuffer re-emits the auxiliary and part fans.
            w.with_orca_fan(|f, w| f.force_resume(w, c));
        }
        ctx.set_current_extruder(next);
        ctx.set("current_extruder", Var::int(next as i64));
        ctx.set("current_hotend", Var::int(next as i64));
        ctx.set("filament_extruder_id", Var::int(next as i64));
        let start = run(
            "start_filament_gcode",
            c.string_at("start_filament_gcode", next).unwrap_or(""),
            ctx,
            w,
            c,
        )
        .map_err(ExportError::Template)?;
        if !start.is_empty() {
            w.raw(&start);
        }
        if ooze && delta != 0. {
            w.raw(&format!(
                "M109 S{} T{next} ; set nozzle temperature and wait for it to be reached",
                num(Self::orca_temperature(c, next, layer).trunc(), 0)
            ));
        }
        if c.string("gcode_flavor") == Some("klipper")
            && c.bool_at("enable_pressure_advance", next).unwrap_or(false)
        {
            w.set_pressure_advance(c.float_at("pressure_advance", next).unwrap_or(0.));
        }
        Ok(())
    }

    /// One Orca (WipeTower2) tower visit, as `WipeTowerIntegration::append_tcr2` emits it.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn emit_tower_orca(
        &mut self,
        v: TowerVisit,
        layer: usize,
        object_z: f64,
        _route: Route<'_, '_>,
        ctx: &mut Context,
        w: &mut Writer,
        c: &Config,
        wipe: &mut Wipe,
    ) -> Result<(), ExportError> {
        let u1 = c
            .string("printer_model")
            .is_some_and(|s| s.contains("Snapmaker") && s.contains("U1"));
        let change_pressure =
            mica_core::wipe_tower::number(c, "enable_change_pressure_when_wiping", 0.) != 0.;
        self.end_label(w, c);
        let down = v.z < object_z - 1e-5;
        let ramming = v
            .before
            .as_ref()
            .is_some_and(|b| b.moves.iter().any(|m| m.ramming));
        // should_travel_to_tower: a finish-only visit, a finish merged before the toolchange
        // (force_travel), a lower tower layer, ramming or "Tool change on wipe tower".
        // Otherwise the independent tool changes in place and the tower G-code travels.
        let travel = !v.change
            || down
            || v.before.is_some()
            || ramming
            || c.bool("tool_change_on_wipe_tower").unwrap_or(false);
        if travel {
            let start = v.before.as_ref().map_or(v.after.start, |b| b.start);
            let t = &self.tools[self.active];
            let speed_z = z_speed(&t.settings);
            wipe.retract(w, &t.config, &t.settings);
            // GCode::retract()'s default NormalLift is the lazy lift that travel_to then
            // executes: Z up, XYZ at the lifted height, Z restored by unretract().
            let above = c.float_at("retract_lift_above", self.active).unwrap_or(0.);
            let below = c.float_at("retract_lift_below", self.active).unwrap_or(0.);
            let lift = if t.settings.retract_length > 0.
                && object_z >= above
                && (object_z <= below || below == 0.)
            {
                t.settings.retract_lift
            } else {
                0.
            };
            let speed = if layer == 0 {
                c.float("initial_layer_travel_speed")
                    .unwrap_or(t.settings.travel_speed)
            } else {
                t.settings.travel_speed
            };
            if lift > 0. && w.position().is_some() {
                w.move_z_force(object_z + lift, speed_z);
                w.travel_to_xyz_force([start[0], start[1], object_z + lift], speed, speed_z);
                w.move_z_force(object_z, speed_z);
            } else {
                w.travel_to(start[0], start[1], speed);
                w.move_z(object_z, speed_z);
            }
            w.unretract(t.settings.deretract_speed);
        }
        if down {
            // "Travel down to the last wipe tower layer."
            let t = &self.tools[self.active];
            w.retract(t.settings.retract_length, t.settings.retract_speed);
            w.move_z(v.z, z_speed(&t.settings));
            w.unretract(t.settings.deretract_speed);
        }
        w.raw(&format!(
            ";HEIGHT:{}\n;TYPE:Prime tower\n; CP WIPE TOWER START",
            num(v.height, 6)
        ));
        let accel = c
            .float("wipe_tower_acceleration")
            .filter(|v| *v > 0.)
            .unwrap_or(self.tools[self.active].settings.default_acceleration);
        w.set_acceleration(accel);
        let (after_change, joined) = match v.after.joined {
            Some(i) if v.change => (&v.after.moves[..i], &v.after.moves[i..]),
            _ if v.change => (&v.after.moves[..], &[][..]),
            _ => (&[][..], &v.after.moves[..]),
        };
        if let Some(b) = &v.before
            && !ramming
        {
            // merge_tcr(finish_layer, tool_change): the layer's finish comes first.
            crate::wipe_tower::block(self, w, b, true);
        }
        if v.change {
            if u1 {
                w.raw("M220 B");
            }
            w.raw("M220 S100");
            if let Some(b) = v.before.as_ref().filter(|_| ramming) {
                if change_pressure {
                    w.set_pressure_advance(mica_core::wipe_tower::number(
                        c,
                        "ramming_pressure_advance_value",
                        0.,
                    ));
                }
                crate::wipe_tower::block(self, w, b, true);
            }
            // toolchange_Unload ends with flush_planner_queue.
            w.raw("G4 S0");
            self.change_orca(v.tool, layer, object_z, ctx, w, c, wipe)?;
            // toolchange_Change: "Travel to where we assume we are", never skipped; the
            // post-processor first places the feedrate line at tool_change_start_pos.
            let s = &self.tools[self.active].settings;
            if let Some(entry) = v.after.entry {
                w.travel_to_xy_never_skip(entry[0], entry[1], s.travel_speed);
            }
            w.travel_to_xy_never_skip(v.after.start[0], v.after.start[1], s.travel_speed);
            // [deretraction_from_wipe_tower_generator]: force the layer Z, then deretract.
            w.move_z_force(v.z, z_speed(s));
            w.unretract(s.deretract_speed);
            w.raw("G4 S0");
            crate::wipe_tower::block(
                self,
                w,
                &mica_core::wipe_tower::TowerBlock {
                    start: v.after.start,
                    moves: after_change.to_vec(),
                    ..Default::default()
                },
                true,
            );
            if u1 {
                w.raw("M220 R");
            }
            w.raw("G4 S0\nG92 E0");
        }
        if !joined.is_empty() {
            crate::wipe_tower::block(
                self,
                w,
                &mica_core::wipe_tower::TowerBlock {
                    start: v.after.start,
                    moves: joined.to_vec(),
                    ..Default::default()
                },
                true,
            );
        }
        w.raw("; CP WIPE TOWER END");
        w.set_acceleration(self.tools[self.active].settings.default_acceleration);
        // append_tcr2 ("SoftFever: set new PA for new filament") after every tower result.
        if c.bool_at("enable_pressure_advance", self.active)
            .unwrap_or(false)
        {
            w.set_pressure_advance(c.float_at("pressure_advance", self.active).unwrap_or(0.));
        }
        if down {
            // "Travel back up to the topmost object layer."
            let t = &self.tools[self.active];
            w.retract(t.settings.retract_length, t.settings.retract_speed);
            w.move_z(object_z, z_speed(&t.settings));
            w.unretract(t.settings.deretract_speed);
        } else {
            // "Prepare a future wipe."
            wipe.set_path(&v.wipe_path, self.active);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::tool_line;

    #[test]
    fn the_first_line_selecting_another_tool_is_the_toolchange() {
        let text = "; Change Tool0 -> Tool2\nM109 S220 T2\nTIMELAPSE_TAKE_FRAME\nT0\nT9\nT2\nT1\n";
        // Not the M109 word, the TIMELAPSE macro, the current tool or a tool the printer lacks.
        assert_eq!(tool_line(text, 0, 4), text.find("T2\nT1"));
        assert_eq!(tool_line("M400\n", 0, 4), None);
        assert_eq!(tool_line("T1 ; select\n", 0, 4), Some(0));
    }
}
