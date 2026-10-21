// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: orca@8500fcdcca src/libslic3r/GCodeWriter.cpp :: apply_print_config, set_accel_and_jerk, set_acceleration_internal, set_jerk_xy, set_pressure_advance, set_fan, set_additional_fan
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/GCodeWriter.cpp :: extrude_to_xy_G2G3IJ, travel_to_xyz_force, travel_to_xy_force, travel_to_z_force
// Source: orca@8500fcdcca src/libslic3r/GCodeWriter.cpp :: GCodeWriter::_spiral_travel_to_z
// Changes: One-turn helical travel and retraction bookkeeping for wipes.
// Changes: Preformatted geometry preserves sequential feedrate, position, tool and floating-point totals.
// Changes: I/J and signed-radius arc emission with caller-computed analytic extrusion length.
// Source: prusaslicer@b028299c77 src/libslic3r/GCode.cpp :: GCodeGenerator::preamble, GCodeGenerator::change_layer
// Changes: z_offset is added to every emitted Z (the writer keeps print Z; templates see the machine Z), as both slicers' writer positions include it.
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/GCodeWriter.cpp :: GCodeWriter::extrude_to_xyz
// Changes: Simultaneous XYZ extrusion for scarf seams.
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/GCodeWriter.cpp :: GCodeWriter::toolchange, GCodeWriter::retract_for_toolchange
// Changes: Per-tool retraction state, toolchange top-up and XY offsets.
// Changes: The Prusa tower path keeps the last acceleration over a toolchange (GCodeWriter::toolchange leaves m_last_acceleration alone); feed rate only when changed (WipeTowerWriter::feedrate).
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/GCodeWriter.cpp :: GCodeWriter::retract, GCodeWriter::_retract
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/CoolingBuffer.cpp :: emit_extrusion, emit_feedrate
// Changes: Partial retraction top-up around wipes; float-split extrusion parts with only the axes that differ.
// Changes: Shared Mica motion writer; preserve Marlin output, add Klipper limits and fan commands.
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/GCodeWriter.cpp :: GCodeWriter::set_acceleration_internal, GCodeWriter::apply_print_config
// Source: prusaslicer@b028299c77 src/libslic3r/Extruder.cpp :: Extruder::extrude
// Changes: Prusa rules: M204 T state starting unknown, accelerations clamped to emitted machine limits; extruded
// Changes: filament counts the quantized E; a travel insert written before the second-to-last waypoint.
// Source: orca@8500fcdcca src/libslic3r/GCode.cpp :: WipeTowerIntegration::post_process_wipe_tower_moves
// Source: orca@8500fcdcca src/libslic3r/GCode/CoolingBuffer.cpp :: CoolingBuffer::apply_layer_cooldown
// Source: orca@8500fcdcca src/libslic3r/Extruder.cpp :: Extruder::retract
// Changes: Never-skipped tower travel with inline feed rate, forced fan resume, retraction state below EPSILON without a move.
// Changes: The writer holds Orca's CoolingBuffer fan state (orca_fan::State) and writes a forced auxiliary fan.
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/CoolingBuffer.cpp :: CoolingBuffer::apply_layer_cooldown
// Changes: A retraction's F counts as the current feed rate (Orca GCodeWriter::retract, PrusaSlicer CoolingBuffer current_feedrate), so the next travel writes its own.
// Changes: PrusaSlicer rules: CoolingBuffer's integer current_feedrate (reset per layer) drops a retraction's repeated F and a zero-length layer arrival at the current F.
// Source: prusaslicer@b028299c77 bundled_deps/localesutils/LocalesUtils.cpp :: float_to_string_decimal_point
// Changes: Six significant digits (a stream's default precision, as %g) in Mica's number style, for the layer markers.
// Changes: Brick layers: travels keep a lift above raised walls until a layer above them starts.

//! Low-level G-code emission with state tracking (position, feedrate, retraction, lift).
//!
//! Numbers are formatted like PrusaSlicer's output (no trailing zeros, no leading zero before the
//! decimal point), so Mica's G-code can be compared with PrusaSlicer's line by line.

use mica_core::config::Config;
use std::fmt::Write as _;

/// Formats `v` with at most `decimals` decimals, trimming trailing zeros and a leading `0`.
pub fn num(v: f64, decimals: usize) -> String {
    let mut s = format!("{v:.decimals$}");
    if s.contains('.') {
        while s.ends_with('0') {
            s.pop();
        }
        if s.ends_with('.') {
            s.pop();
        }
    }
    if s == "-0" {
        s = "0".into();
    }
    if let Some(rest) = s.strip_prefix("0.") {
        s = format!(".{rest}");
    } else if let Some(rest) = s.strip_prefix("-0.") {
        s = format!("-.{rest}");
    }
    s
}

/// `GCodeFormatter::quantize_e`: an E delta at the G-code's 5 decimals, as `Extruder::extrude`
/// counts it into the extruded volume (`extruded_weight_total` and friends).
fn quantize_e(v: f64) -> f64 {
    (v * 1e5).round() * 1e-5
}

/// `v` with six significant digits (a C++ stream's default precision, as PrusaSlicer's
/// `float_to_string_decimal_point` and `%g` write it), in `num`'s style.
pub fn num_g6(v: f64) -> String {
    if !v.is_finite() {
        return num(v, 3);
    }
    num(format!("{v:.5e}").parse().unwrap_or(v), 9)
}

/// `num_g6` with the leading zero Orca keeps.
pub(crate) fn decimal_g6(v: f64) -> String {
    let s = num_g6(v);
    if s.starts_with('.') {
        format!("0{s}")
    } else if let Some(rest) = s.strip_prefix("-.") {
        format!("-0.{rest}")
    } else {
        s
    }
}

/// Orca command arguments and metadata retain the leading zero.
pub(crate) fn decimal(v: f64, decimals: usize) -> String {
    let s = num(v, decimals);
    if s.starts_with('.') {
        format!("0{s}")
    } else if let Some(rest) = s.strip_prefix("-.") {
        format!("-0.{rest}")
    } else {
        s
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MoveStats {
    /// Filament pushed, mm (net of retractions).
    pub filament_mm: f64,
    /// Seconds. The writer accumulates constant-speed motion; export replaces it with planned time.
    pub time_s: f64,
    pub silent_time_s: Option<f64>,
    pub first_layer_time_s: f64,
    pub silent_first_layer_time_s: Option<f64>,
}

use crate::Flavor;

#[derive(Default)]
struct KlipperLimits {
    acceleration: f64,
    jerk: f64,
    accel_to_decel: Option<f64>,
    comments: bool,
    fan_minimum: f64,
}

/// G2/G3 geometry. Negative R selects the major arc.
#[derive(Clone, Copy, Debug)]
pub enum ArcCommand {
    Center { ij: (f64, f64), ccw: bool },
    Radius { radius: f64, ccw: bool },
}

/// Accumulates G-code text and tracks the machine state needed to write minimal, correct moves.
pub struct Writer {
    out: String,
    flavor: Flavor,
    limits: KlipperLimits,
    jerk: Option<f64>,
    fan: Option<f64>,
    auxiliary_fan: Option<f64>,
    x: Option<f64>,
    y: Option<f64>,
    /// The nozzle's Z in print coordinates (the layers' Z, without `z_offset`).
    z: f64,
    /// The printer's `z_offset`, added to every Z the writer emits (PrusaSlicer's and Orca's
    /// GCodeWriter positions include it: `GCode::change_layer`, `GCode::preamble`).
    z_offset: f64,
    feedrate: Option<f64>,
    acceleration: Option<f64>,
    /// PrusaSlicer rules: the travel acceleration last written (`M204 T`,
    /// `GCodeWriter::m_last_travel_acceleration`, unknown at the start like PrusaSlicer's).
    travel_acceleration: Option<f64>,
    /// PrusaSlicer rules: `GCodeWriter::m_max_acceleration` / `m_max_travel_acceleration`, the
    /// machine limits the print and travel accelerations are clamped to (0 = no clamp).
    prusa_max_acceleration: (f64, f64),
    /// G-code a travel writes before its second-to-last point (`GCodeGenerator::travel_to`'s
    /// `insert_gcode`, PrusaSlicer's object label change).
    travel_insert: Option<String>,
    /// Retracted filament length, mm (0 when primed).
    retracted: f64,
    restart_extra: f64,
    /// Current Z lift above the layer, mm.
    lifted: f64,
    pub stats: MoveStats,
    tool: usize,
    tools: Vec<(f64, f64)>,
    tool_filament: Vec<f64>,
    offset: (f64, f64),
    /// PrusaSlicer rules: CoolingBuffer::apply_layer_cooldown removes a repeated F word and drops
    /// a zero-length move whose F is the current one (Orca's writer does its own bookkeeping);
    /// travels follow GCodeWriter::travel_to_xyz (XYZ_EPSILON) and carry PrusaSlicer's
    /// truncated scaled Z values, so Z equality tolerates their 1 µm quantization.
    prusa_rules: bool,
    /// Orca's CoolingBuffer fan state (Orca cooling only), kept with the output it shapes.
    orca_fan: crate::orca_fan::State,
    /// Brick layers: the top of the highest raised wall still level with or above the nozzle's
    /// layer, the height travels keep above it, and the Z speed of that lift.
    raised: Option<[f64; 3]>,
}

impl Default for Writer {
    fn default() -> Self {
        Self::new()
    }
}

impl Writer {
    pub fn new() -> Self {
        Self {
            out: String::with_capacity(1 << 20),
            flavor: Flavor::PrusaBuddy,
            limits: KlipperLimits::default(),
            jerk: None,
            fan: None,
            auxiliary_fan: None,
            orca_fan: crate::orca_fan::State::default(),
            x: None,
            y: None,
            z: 0.0,
            z_offset: 0.0,
            feedrate: None,
            acceleration: None,
            travel_acceleration: None,
            prusa_max_acceleration: (0.0, 0.0),
            travel_insert: None,
            retracted: 0.0,
            restart_extra: 0.0,
            lifted: 0.0,
            stats: MoveStats::default(),
            tool: 0,
            tools: vec![(0., 0.)],
            tool_filament: vec![0.],
            offset: (0., 0.),
            prusa_rules: false,
            raised: None,
        }
    }

    pub fn with_config(flavor: Flavor, c: &Config) -> Self {
        let mut w = Self::new();
        w.flavor = flavor;
        w.prusa_rules = !mica_core::rules::Rules::of(c).is_orca();
        w.z_offset = c.float("z_offset").filter(|v| v.is_finite()).unwrap_or(0.0);
        let f = |k| c.float(k).unwrap_or(0.0);
        // GCodeWriter::apply_print_config: limits emitted to G-code also clamp the accelerations.
        if w.prusa_rules
            && flavor != Flavor::Klipper
            && c.string("machine_limits_usage") == Some("emit_to_gcode")
        {
            w.prusa_max_acceleration = (
                f("machine_max_acceleration_extruding").round(),
                f("machine_max_acceleration_travel").round(),
            );
        }
        let mut acceleration = f("machine_max_acceleration_extruding").round();
        for key in ["machine_max_acceleration_x", "machine_max_acceleration_y"] {
            let limit = f(key).round();
            if limit > 0.0 {
                acceleration = acceleration.min(limit);
            }
        }
        let jerk = [
            f("machine_max_jerk_x").round(),
            f("machine_max_jerk_y").round(),
        ]
        .into_iter()
        .filter(|v| *v > 0.0)
        .reduce(f64::min)
        .unwrap_or(0.0);
        w.limits = KlipperLimits {
            acceleration,
            jerk,
            accel_to_decel: c
                .bool("accel_to_decel_enable")
                .unwrap_or(false)
                .then(|| f("accel_to_decel_factor") / 100.0),
            comments: c.bool("gcode_comments").unwrap_or(false),
            fan_minimum: f("part_cooling_fan_min_pwm"),
        };
        w
    }

    pub(crate) fn filament_for_tool(&self, tool: usize) -> f64 {
        self.tool_filament.get(tool).copied().unwrap_or(0.)
    }
    pub(crate) fn active_tool(&self) -> usize {
        self.tool
    }
    pub(crate) fn tool_states(&self, n: usize) -> Vec<(f64, f64)> {
        let mut states = self.tools.clone();
        states.resize(n, (0., 0.));
        states[self.tool] = (self.retracted, self.restart_extra);
        states
    }
    pub(crate) fn apply_tool_states(&mut self, states: Vec<(f64, f64)>) {
        (self.retracted, self.restart_extra) = states[self.tool];
        self.tools = states;
    }
    pub(crate) fn select_tool(&mut self, tool: usize, c: &Config) {
        let n = c.vector_len("nozzle_diameter").max(1);
        self.tools = self.tool_states(n);
        self.tool_filament.resize(n, 0.);
        self.tool = tool;
        (self.retracted, self.restart_extra) = self.tools[tool];
        let previous_offset = self.offset;
        self.offset = c
            .points("extruder_offset")
            .and_then(|v| v.get(tool).or(v.first()).copied())
            .unwrap_or_default();
        if let Some(x) = &mut self.x {
            *x += self.offset.0 - previous_offset.0;
        }
        if let Some(y) = &mut self.y {
            *y += self.offset.1 - previous_offset.1;
        }
        self.feedrate = None;
        self.acceleration = None;
        self.jerk = None;
        self.forget_fan();
    }
    pub(crate) fn set_restart_extra(&mut self, value: f64) {
        self.restart_extra = value;
    }
    /// Toolchange retraction tops up an existing ordinary retraction.
    pub(crate) fn retract_to(&mut self, length: f64, speed: f64) {
        if length <= self.retracted {
            return;
        }
        let previous = self.retracted;
        self.retracted = 0.;
        self.retract(length - previous, speed);
        self.retracted = length;
    }

    /// Orca combines changed acceleration and square-corner velocity in one Klipper command.
    /// Zero means leave firmware state alone. Limits apply even if envelope emission is disabled.
    pub fn set_accel_and_jerk(&mut self, accel: f64, jerk: f64) {
        if self.flavor == Flavor::PrusaBuddy {
            self.set_acceleration(accel);
            return;
        }
        let accel = if self.limits.acceleration > 0.0 {
            accel.round().min(self.limits.acceleration)
        } else {
            accel.round()
        };
        let jerk = if self.limits.jerk > 0.0 {
            jerk.min(self.limits.jerk)
        } else {
            jerk
        };
        let mut line = String::from("SET_VELOCITY_LIMIT");
        if accel > 0.0 && self.acceleration != Some(accel) {
            self.acceleration = Some(accel);
            let _ = write!(line, " ACCEL={}", num(accel, 0));
            if let Some(factor) = self.limits.accel_to_decel {
                let _ = write!(line, " ACCEL_TO_DECEL={}", decimal(accel * factor, 6));
            }
        }
        if jerk > 0.01 && self.jerk.is_none_or(|j| (j - jerk).abs() > 1e-6) {
            self.jerk = Some(jerk);
            let _ = write!(line, " SQUARE_CORNER_VELOCITY={}", decimal(jerk, 6));
        }
        if line != "SET_VELOCITY_LIMIT" {
            if self.limits.comments {
                line.push_str(" ; adjust VELOCITY_LIMIT(accel/jerk)");
            }
            self.raw(&line);
        }
    }

    pub fn set_pressure_advance(&mut self, pa: f64) {
        if self.flavor == Flavor::Klipper && pa >= 0.0 {
            let decimals = if pa > 0.0 {
                (3 - pa.log10().floor() as i32).max(0) as usize
            } else {
                0
            };
            self.raw(&format!(
                "SET_PRESSURE_ADVANCE ADVANCE={}; Override pressure advance value",
                decimal(pa, decimals)
            ));
        }
    }

    pub fn set_auxiliary_fan(&mut self, percent: f64) {
        if self.auxiliary_fan != Some(percent) {
            self.auxiliary_fan = Some(percent);
            self.raw(&format!(
                "M106 P2 S{}",
                num((255.0 * percent.clamp(0.0, 100.0) / 100.0).floor(), 0)
            ));
        }
    }

    /// Orca `GCodeWriter::set_additional_fan`: emitted even when the speed is unchanged.
    pub fn force_auxiliary_fan(&mut self, percent: f64) {
        self.auxiliary_fan = None;
        self.set_auxiliary_fan(percent);
    }

    /// Run `f` on Orca's CoolingBuffer fan state and this writer.
    pub(crate) fn with_orca_fan<R>(
        &mut self,
        f: impl FnOnce(&mut crate::orca_fan::State, &mut Self) -> R,
    ) -> R {
        let mut state = std::mem::take(&mut self.orca_fan);
        let result = f(&mut state, self);
        self.orca_fan = state;
        result
    }

    pub fn forget_fan(&mut self) {
        self.fan = None;
        self.auxiliary_fan = None;
    }

    pub fn into_string(self) -> String {
        self.out
    }

    /// Writes the G-code produced so far to `sink` and empties the buffer, so a whole print never has
    /// to be held in memory.
    pub fn drain_to(&mut self, sink: &mut dyn std::io::Write) -> std::io::Result<()> {
        sink.write_all(self.out.as_bytes())?;
        self.out.clear();
        Ok(())
    }

    pub fn as_str(&self) -> &str {
        &self.out
    }

    pub fn position(&self) -> Option<(f64, f64)> {
        Some((self.x?, self.y?))
    }

    pub fn z(&self) -> f64 {
        self.z
    }

    /// The printer's `z_offset` (added to every emitted Z).
    pub fn z_offset(&self) -> f64 {
        self.z_offset
    }

    /// Templates see the machine position (`position[2]` includes `z_offset`, as upstream's
    /// writer position does).
    pub(crate) fn template_state(&self) -> ([f64; 3], f64, f64, f64) {
        (
            [
                self.x.unwrap_or(0.0),
                self.y.unwrap_or(0.0),
                self.z + self.z_offset,
            ],
            self.retracted,
            self.restart_extra,
            self.lifted,
        )
    }

    /// Templates describe the state left by their own moves; this emits no motion.
    pub(crate) fn apply_template_state(
        &mut self,
        position: Option<[f64; 3]>,
        retracted: f64,
        restart_extra: f64,
    ) {
        if let Some([x, y, z]) = position {
            self.x = Some(x);
            self.y = Some(y);
            self.z = z - self.z_offset;
            self.feedrate = None;
        }
        self.retracted = retracted;
        self.restart_extra = restart_extra;
    }

    /// Appends raw G-code (one or more lines; a trailing newline is added if missing).
    /// `raw`, for text that may be empty (then nothing, not an empty line).
    pub(crate) fn raw_nonempty(&mut self, text: &str) {
        if !text.is_empty() {
            self.raw(text);
        }
    }
    pub fn raw(&mut self, text: &str) {
        if self.flavor == Flavor::PrusaBuddy
            && text
                .lines()
                .any(|line| matches!(line.split_whitespace().next(), Some("M106" | "M107")))
        {
            self.forget_fan();
        }
        self.out.push_str(text);
        if !text.ends_with('\n') {
            self.out.push('\n');
        }
    }

    /// PrusaSlicer's CoolingBuffer processes each layer on its own, starting with
    /// `current_feedrate = 0`: the layer's first feed rate is always written.
    pub fn begin_cooling_layer(&mut self) {
        if self.prusa_rules {
            self.feedrate = None;
        }
    }

    /// CoolingBuffer's `current_feedrate` (an `int` parsed from the G-code text) equals the
    /// integer F word `f`, so the F word (or, for a zero-length move, the line) is dropped.
    fn cooling_repeats_feedrate(&self, f: f64) -> bool {
        self.prusa_rules
            && self.feedrate.is_some_and(|cur| {
                num(cur, 3).parse::<f64>().map(f64::trunc).ok() == num(f, 0).parse::<f64>().ok()
            })
    }

    pub fn comment(&mut self, text: &str) {
        let _ = writeln!(self.out, ";{text}");
    }

    /// The machine state is unknown after custom G-code that moves the head (e.g. start G-code).
    pub fn forget_position(&mut self, z: f64) {
        self.x = None;
        self.y = None;
        self.z = z;
        self.feedrate = None;
    }

    pub fn set_acceleration(&mut self, accel: f64) {
        if self.flavor == Flavor::Klipper {
            self.set_accel_and_jerk(accel, 0.0);
            return;
        }
        let max = self.prusa_max_acceleration.0;
        let accel = if max > 0.0 { accel.min(max) } else { accel };
        if self.acceleration != Some(accel) {
            self.acceleration = Some(accel);
            let _ = writeln!(self.out, "M204 P{}", num(accel, 0));
        }
    }

    /// `GCodeWriter::set_travel_acceleration` (Marlin 2): `M204 T` when the value changes; 0 keeps
    /// the current one.
    pub fn set_travel_acceleration(&mut self, accel: f64) {
        let accel = (accel + 0.5).floor();
        let max = self.prusa_max_acceleration.1;
        let accel = if max > 0.0 { accel.min(max) } else { accel };
        if accel <= 0.0 || self.travel_acceleration == Some(accel) {
            return;
        }
        self.travel_acceleration = Some(accel);
        let _ = writeln!(self.out, "M204 T{}", num(accel, 0));
    }

    /// Fan speed in percent, written even when unchanged (Orca's `GCodeWriter::set_fan`).
    pub fn force_fan(&mut self, percent: f64) {
        self.fan = None;
        self.set_fan(percent);
    }

    /// Fan speed in percent.
    pub fn set_fan(&mut self, percent: f64) {
        if self.flavor == Flavor::Klipper {
            if self.fan != Some(percent) {
                self.fan = Some(percent);
                let percent = if percent > 0.0 {
                    percent.max(self.limits.fan_minimum)
                } else {
                    0.0
                };
                self.raw(&format!(
                    "M106 S{}",
                    num((255.5 * percent.clamp(0.0, 100.0) / 100.0).floor(), 0)
                ));
            }
            return;
        }
        if self.fan == Some(percent) {
            return;
        }
        self.fan = Some(percent);
        if percent <= 0.0 {
            self.out.push_str("M107\n");
        } else {
            let _ = writeln!(
                self.out,
                "M106 S{}",
                num(255.0 * percent.min(100.0) / 100.0, 3)
            );
        }
    }

    pub(crate) fn retract_with_extra(&mut self, length: f64, speed: f64, extra: f64) {
        if self.retracted <= 0. && length > 0. {
            self.restart_extra = extra;
        }
        self.retract(length, speed);
    }

    pub(crate) fn begin_wipe(&mut self) {
        self.raw(";WIPE_START");
        self.feedrate = None;
    }
    pub(crate) fn wipe_to(&mut self, x: f64, y: f64, retract: f64, speed: f64) {
        self.extrude_to(x, y, -retract, speed);
        self.retracted += retract;
    }
    pub(crate) fn wipe_arc_to(
        &mut self,
        xy: (f64, f64),
        arc: ArcCommand,
        retract: f64,
        speed: f64,
        length: f64,
    ) {
        self.extrude_to_xy_g2g3(xy, arc, -retract, speed, length);
        self.retracted += retract;
    }
    /// `GCodeWriter::retract` with an already quantized amount still to go: adds to an existing
    /// (partial) retraction instead of skipping it.
    pub(crate) fn retract_top_up(&mut self, amount: f64, speed: f64) {
        if amount <= 0.0 {
            return;
        }
        let before = self.retracted;
        self.retracted = 0.0;
        self.retract(amount, speed);
        self.retracted = before + amount;
    }
    /// `Extruder::retract` below `EPSILON`: the state follows, but no move is written.
    pub(crate) fn assume_retracted(&mut self, length: f64) {
        if length > self.retracted {
            let delta = length - self.retracted;
            self.stats.filament_mm -= delta;
            self.tool_filament[self.tool] -= quantize_e(delta);
            self.retracted = length;
        }
    }
    pub(crate) fn retracted(&self) -> f64 {
        self.retracted
    }
    /// `WipeTowerWriter::feedrate`: a feed rate line only when it changes the current one.
    pub(crate) fn feedrate_if_changed(&mut self, speed: f64) {
        if self.feedrate != Some(speed * 60.0) {
            self.feedrate_line(speed);
        }
    }
    /// G-code the next travel writes before its second-to-last point (empty text: none).
    pub(crate) fn set_travel_insert(&mut self, text: String) {
        self.travel_insert = (!text.is_empty()).then_some(text);
    }
    /// Write the pending travel insert now (the travel reached its insertion point, or wrote
    /// no point at all).
    pub(crate) fn flush_travel_insert(&mut self) {
        if let Some(text) = self.travel_insert.take() {
            self.raw(&text);
        }
    }
    /// Travel waypoints with the pending insert before the second-to-last one
    /// (`GCodeGenerator::generate_travel_gcode`; the waypoints exclude the start, which
    /// PrusaSlicer's list includes, so the index from the end is the same).
    pub(crate) fn travel_points(&mut self, points: &[[f64; 3]], speed: f64, z_speed: f64) {
        let n = points.len();
        for (i, &point) in points.iter().enumerate() {
            if n - i <= 2 {
                self.flush_travel_insert();
            }
            self.travel_to_xyz(point, speed, z_speed);
        }
        self.flush_travel_insert();
    }
    /// The print acceleration last written (`GCodeWriter::m_last_acceleration`).
    pub(crate) fn acceleration_state(&self) -> Option<f64> {
        self.acceleration
    }
    /// `GCodeWriter::toolchange` keeps `m_last_acceleration`; restores it after `select_tool`.
    pub(crate) fn restore_acceleration(&mut self, accel: Option<f64>) {
        self.acceleration = accel;
    }
    /// `emit_feedrate`: an explicit feed rate line.
    pub(crate) fn feedrate_line(&mut self, speed: f64) {
        let f = speed * 60.0;
        let _ = writeln!(self.out, "G1 F{}", num(f, 3));
        self.feedrate = Some(f);
    }
    /// CoolingBuffer's `emit_extrusion`: one part of a split line, with every axis that differs
    /// in float (`za`, `zb`: the machine Z as written, with z_offset). Machine position and
    /// totals follow the part's end.
    pub(crate) fn extrude_split_part(
        &mut self,
        a: [f32; 2],
        b: [f32; 2],
        za: f32,
        zb: f32,
        e: f32,
        speed: f64,
    ) {
        let f = speed * 60.0;
        if self.feedrate.is_none_or(|cur| (cur - f).abs() > 1e-6) {
            let _ = writeln!(self.out, "G1 F{}", num(f, 3));
            self.feedrate = Some(f);
        }
        self.out.push_str("G1");
        if a[0] != b[0] {
            let _ = write!(self.out, " X{}", num(f64::from(b[0]) - self.offset.0, 3));
        }
        if a[1] != b[1] {
            let _ = write!(self.out, " Y{}", num(f64::from(b[1]) - self.offset.1, 3));
        }
        if za != zb {
            let _ = write!(self.out, " Z{}", num(f64::from(zb), 3));
        }
        if e != 0.0 {
            let _ = write!(self.out, " E{}", num(f64::from(e), 5));
        }
        self.out.push('\n');
        let (x, y) = (f64::from(b[0]), f64::from(b[1]));
        if let Some((cx, cy)) = self.position() {
            self.stats.time_s += (x - cx).hypot(y - cy) / speed;
        }
        self.stats.filament_mm += f64::from(e);
        self.tool_filament[self.tool] += quantize_e(f64::from(e));
        self.x = Some(x);
        self.y = Some(y);
    }
    pub(crate) fn tool_offset(&self) -> (f64, f64) {
        self.offset
    }
    pub(crate) fn set_position(&mut self, xy: (f64, f64)) {
        self.x = Some(xy.0);
        self.y = Some(xy.1);
    }
    pub(crate) fn spiral_lift(&mut self, z: f64, ij: (f64, f64), speed: f64) {
        self.raw("G17");
        let _ = writeln!(
            self.out,
            "G3 Z{} I{} J{} P1 F{}",
            num(z + self.z_offset, 3),
            num(ij.0, 3),
            num(ij.1, 3),
            num(speed * 60., 0)
        );
        self.stats.time_s += (std::f64::consts::TAU * ij.0.hypot(ij.1)).hypot(z - self.z) / speed;
        self.z = z;
        self.feedrate = Some(speed * 60.);
    }

    pub fn retract(&mut self, length: f64, speed: f64) {
        if self.retracted > 0.0 || length <= 0.0 {
            return;
        }
        self.retracted = length;
        self.stats.filament_mm -= length;
        self.tool_filament[self.tool] -= quantize_e(length);
        self.stats.time_s += length / speed;
        let _ = write!(self.out, "G1 E-{}", num(length, 5).trim_start_matches('-'));
        // CoolingBuffer removes the F word when it repeats current_feedrate (e.g. a first-layer
        // retraction right after a 45 mm/s extrusion on the CORE One).
        if !self.cooling_repeats_feedrate(speed * 60.0) {
            let _ = write!(self.out, " F{}", num(speed * 60.0, 0));
        }
        self.out.push('\n');
        // The retraction's F is the current feed rate, so the next move states its own: Orca's
        // writer remembers it (GCodeWriter::retract), PrusaSlicer's CoolingBuffer counts it as
        // `current_feedrate` (apply_layer_cooldown), e.g. on a layer change's first ramp move.
        self.feedrate = Some(speed * 60.0);
    }

    pub fn unretract(&mut self, speed: f64) {
        if self.retracted <= 0.0 {
            return;
        }
        let length = std::mem::take(&mut self.retracted) + std::mem::take(&mut self.restart_extra);
        self.stats.filament_mm += length;
        self.tool_filament[self.tool] += quantize_e(length);
        self.stats.time_s += length / speed;
        let _ = writeln!(self.out, "G1 E{} F{}", num(length, 5), num(speed * 60.0, 0));
        self.feedrate = None;
    }

    /// Custom G-code (an `M600` colour change) left the extruder primed: count the pending
    /// retraction as repaid without writing a move (PrusaSlicer discards that unretract's text).
    pub(crate) fn assume_unretracted(&mut self) {
        let length = std::mem::take(&mut self.retracted) + std::mem::take(&mut self.restart_extra);
        self.tool_filament[self.tool] += quantize_e(length);
    }

    /// Moves Z to `z` (absolute), keeping any lift on top.
    pub fn move_z(&mut self, z: f64, speed: f64) {
        let target = z + self.lifted;
        // PrusaSlicer's Z values are truncated to whole micrometres (scaled coord_t), so the
        // same layer Z can differ by a few 1e-6 mm between code paths.
        let same = if self.prusa_rules { 1e-5 } else { 1e-9 };
        if (target - self.z).abs() < same {
            return;
        }
        self.stats.time_s += (target - self.z).abs() / speed;
        self.z = target;
        let _ = writeln!(
            self.out,
            "G1 Z{} F{}",
            num(target + self.z_offset, 3),
            num(speed * 60.0, 0)
        );
        self.feedrate = None;
    }

    /// `GCodeWriter::travel_to_z_force`: always writes the Z move (even to the current Z) and
    /// leaves no lift pending.
    pub(crate) fn travel_to_z_force(&mut self, z: f64, speed: f64) {
        self.stats.time_s += (z - self.z).abs() / speed;
        self.z = z;
        self.lifted = 0.0;
        let _ = writeln!(
            self.out,
            "G1 Z{} F{}",
            num(z + self.z_offset, 3),
            num(speed * 60.0, 0)
        );
        self.feedrate = None;
    }

    /// `travel_to_first_position` after a ramping layer change: `travel_to_xy_force` to the
    /// current position, then `travel_to_z_force(z)`, even if rounding puts it at the same Z.
    pub(crate) fn confirm_layer_arrival(
        &mut self,
        to: (f64, f64),
        speed: f64,
        z_speed: f64,
        z: f64,
    ) {
        let Some((x, y)) = self.position() else {
            return;
        };
        let text = |x: f64, y: f64| {
            format!(
                "X{} Y{}",
                num(x - self.offset.0, 3),
                num(y - self.offset.1, 3)
            )
        };
        let (from, xy) = (text(x, y), text(to.0, to.1));
        // CoolingBuffer reads the written text: at the current F a move that does not change
        // it is dropped and any other loses its F word.
        if !self.cooling_repeats_feedrate(speed * 60.) {
            let _ = writeln!(self.out, "G1 {xy} F{}", num(speed * 60., 0));
        } else if xy != from {
            let _ = writeln!(self.out, "G1 {xy}");
        }
        self.stats.time_s += (to.0 - x).hypot(to.1 - y) / speed + (z - self.z).abs() / z_speed;
        self.x = Some(to.0);
        self.y = Some(to.1);
        self.z = z;
        self.lifted = 0.0;
        let _ = writeln!(
            self.out,
            "G1 Z{} F{}",
            num(z + self.z_offset, 3),
            num(z_speed * 60., 0)
        );
        self.feedrate = None;
    }

    /// `GCodeWriter::travel_to_xyz_force`'s feed rate: the travel speed, slowed so that its Z
    /// component is at most `z_speed` (Prusa rules use upstream's exact arithmetic).
    fn capped_travel_speed(&self, p: [f64; 3], speed: f64, z_speed: f64) -> f64 {
        let (x, y) = self.position().unwrap_or((p[0], p[1]));
        let dz = (p[2] - self.z).abs();
        if self.prusa_rules {
            if z_speed == 0.0 {
                return speed;
            }
            // In machine coordinates (with z_offset), as GCodeWriter computes it.
            let mz = (p[2] + self.z_offset) - (self.z + self.z_offset);
            let (mx, my) = (p[0] - x, p[1] - y);
            let distance = (mx * mx + my * my + mz * mz).sqrt();
            let unit_z = mz.abs() / distance;
            // A zero-length move gives NaN here, which never exceeds the cap (upstream too).
            if unit_z * speed > z_speed {
                return z_speed / unit_z;
            }
            return speed;
        }
        let length = (p[0] - x).hypot(p[1] - y).hypot(dz);
        if z_speed > 0.0 && dz > 0.0 {
            speed.min(z_speed * length / dz)
        } else {
            speed
        }
    }

    /// Brick layers: a raised wall reaching up to `top` was printed; until the nozzle's layer
    /// is above it, travels keep `lift` above it.
    pub(crate) fn printed_raised(&mut self, top: f64, lift: f64, z_speed: f64) {
        let [old_top, old_lift, _] = self.raised.unwrap_or([f64::NEG_INFINITY, 0.0, 0.0]);
        self.raised = Some([top.max(old_top), lift.max(old_lift), z_speed]);
    }

    /// Brick layers: a path of the layer at `layer_z` starts; raised walls below that layer
    /// no longer limit travels.
    pub(crate) fn settle_raised(&mut self, layer_z: f64) {
        if self.raised.is_some_and(|[top, ..]| layer_z > top + 1e-6) {
            self.raised = None;
        }
    }

    /// Brick layers: before a travel that moves in XY, rises straight up to the clearance
    /// over raised walls if the nozzle is below it, and returns `target_z` kept at or above
    /// that clearance; a travel straight down stops at the nozzle's Z. Without raised walls
    /// nothing changes.
    fn clear_raised(&mut self, moves_xy: bool, target_z: f64) -> f64 {
        let Some([top, lift, z_speed]) = self.raised else {
            return target_z;
        };
        let floor = top + lift;
        if !moves_xy {
            // Straight down where a raised wall may have just ended: not below the nozzle
            // (the path's own start Z follows with `move_z`).
            return target_z.max(self.z.min(floor));
        }
        if self.z < floor - 1e-6 {
            let up = floor - self.z;
            self.stats.time_s += up / z_speed.max(1.0);
            if self.lifted > 0.0 {
                self.lifted += up;
            }
            self.z = floor;
            let _ = writeln!(
                self.out,
                "G1 Z{} F{}",
                num(floor + self.z_offset, 3),
                num(z_speed.max(1.0) * 60.0, 0)
            );
            self.feedrate = None;
        }
        target_z.max(floor)
    }

    /// Whether a travel to (`x`, `y`) leaves the nozzle's spot; joints between pieces of one
    /// wall (rounded to the output's 1 µm) do not.
    fn moves_xy(&self, x: f64, y: f64) -> bool {
        self.position()
            .is_none_or(|(cx, cy)| (cx - x).hypot(cy - y) > 0.02)
    }

    pub fn lift(&mut self, amount: f64, speed: f64) {
        if self.lifted > 0.0 || amount <= 0.0 {
            return;
        }
        let base = self.z;
        self.lifted = amount;
        self.z = base; // move_z adds the lift
        self.move_z(base, speed);
    }

    pub fn unlift(&mut self, speed: f64) {
        if self.lifted <= 0.0 {
            return;
        }
        let base = self.z - self.lifted;
        self.lifted = 0.0;
        self.move_z(base, speed);
    }

    /// Drops a pending lift by moving straight to `z` (one Z move instead of down to the lifted
    /// base, then to `z`). Without a lift nothing is written.
    pub(crate) fn unlift_to(&mut self, z: f64, speed: f64) {
        if self.lifted <= 0.0 {
            return;
        }
        self.lifted = 0.0;
        self.move_z(z, speed);
    }

    /// XYZ travel with a Z-component velocity cap; final waypoint may restore the layer Z.
    pub fn travel_to_xyz(&mut self, mut p: [f64; 3], speed: f64, z_speed: f64) {
        if self.raised.is_some() {
            p[2] = self.clear_raised(self.moves_xy(p[0], p[1]), p[2]);
        }
        if self.prusa_rules {
            // GCodeWriter::travel_to_xyz: XYZ_EPSILON (0.001 mm) decides which axes move.
            const XYZ_EPSILON: f64 = 0.001;
            let (x, y) = self.position().unwrap_or((f64::NAN, f64::NAN));
            let same_xy = (p[0] - x).abs() < XYZ_EPSILON && (p[1] - y).abs() < XYZ_EPSILON;
            let same_z = (p[2] - self.z).abs() < XYZ_EPSILON;
            if same_xy && same_z {
                return;
            }
            if same_xy {
                self.move_z_force(p[2], if z_speed > 0.0 { z_speed } else { speed });
            } else if same_z {
                self.travel_to(p[0], p[1], speed);
            } else {
                self.travel_to_xyz_force(p, speed, z_speed);
            }
            return;
        }
        if self
            .position()
            .is_some_and(|(x, y)| (x - p[0]).abs() < 0.0005 && (y - p[1]).abs() < 0.0005)
        {
            self.move_z(p[2], if z_speed > 0.0 { z_speed } else { speed });
        } else if (p[2] - self.z).abs() < 0.0005 {
            self.travel_to(p[0], p[1], speed);
        } else {
            let (x, y) = self.position().unwrap_or((p[0], p[1]));
            let dz = (p[2] - self.z).abs();
            let length = (p[0] - x).hypot(p[1] - y).hypot(dz);
            let speed = if z_speed > 0.0 {
                speed.min(z_speed * length / dz)
            } else {
                speed
            };
            let _ = writeln!(
                self.out,
                "G1 X{} Y{} Z{} F{}",
                num(p[0] - self.offset.0, 3),
                num(p[1] - self.offset.1, 3),
                num(p[2] + self.z_offset, 3),
                num(speed * 60., 0)
            );
            self.stats.time_s += length / speed;
            self.x = Some(p[0]);
            self.y = Some(p[1]);
            self.z = p[2];
            self.feedrate = Some(speed * 60.);
        }
    }

    /// Layer-change ramps retain every XYZ waypoint, including the starting point.
    pub fn travel_to_xyz_force(&mut self, mut p: [f64; 3], speed: f64, z_speed: f64) {
        if self.raised.is_some() {
            p[2] = self.clear_raised(self.moves_xy(p[0], p[1]), p[2]);
        }
        let (x, y) = self.position().unwrap_or((p[0], p[1]));
        let dz = (p[2] - self.z).abs();
        let length = (p[0] - x).hypot(p[1] - y).hypot(dz);
        let speed = self.capped_travel_speed(p, speed, z_speed);
        let feedrate = speed * 60.;
        let _ = write!(
            self.out,
            "G1 X{} Y{} Z{}",
            num(p[0] - self.offset.0, 3),
            num(p[1] - self.offset.1, 3),
            num(p[2] + self.z_offset, 3)
        );
        if self.feedrate != Some(feedrate) {
            let _ = write!(self.out, " F{}", num(feedrate, 3));
        }
        self.out.push('\n');
        self.stats.time_s += length / speed;
        self.x = Some(p[0]);
        self.y = Some(p[1]);
        self.z = p[2];
        self.feedrate = Some(feedrate);
    }

    /// `GCodeWriter::travel_to_xy_force`-like: always emits the XY travel (the wipe tower's
    /// never-skip move after custom toolchange G-code), with the feedrate on its own line.
    pub(crate) fn travel_to_xy_force(&mut self, x: f64, y: f64, speed: f64) {
        if self.raised.is_some() {
            let z = self.z;
            self.clear_raised(self.moves_xy(x, y), z);
        }
        if let Some((cx, cy)) = self.position() {
            self.stats.time_s += (x - cx).hypot(y - cy) / speed;
        }
        let f = speed * 60.0;
        let _ = writeln!(self.out, "G1 F{}", num(f, 0));
        let _ = writeln!(
            self.out,
            "G1 X{} Y{}",
            num(x - self.offset.0, 3),
            num(y - self.offset.1, 3)
        );
        self.x = Some(x);
        self.y = Some(y);
        self.feedrate = Some(f);
    }

    /// Orca's post-processed wipe-tower travel (`never_skip_tag`): always written, with the
    /// feed rate on the same line when it changes.
    pub(crate) fn travel_to_xy_never_skip(&mut self, x: f64, y: f64, speed: f64) {
        if self.raised.is_some() {
            let z = self.z;
            self.clear_raised(self.moves_xy(x, y), z);
        }
        if let Some((cx, cy)) = self.position() {
            self.stats.time_s += (x - cx).hypot(y - cy) / speed;
        }
        let f = speed * 60.0;
        let _ = write!(
            self.out,
            "G1 X{} Y{}",
            num(x - self.offset.0, 3),
            num(y - self.offset.1, 3)
        );
        if self.feedrate != Some(f) {
            let _ = write!(self.out, " F{}", num(f, 0));
        }
        self.out.push('\n');
        self.x = Some(x);
        self.y = Some(y);
        self.feedrate = Some(f);
    }

    /// `GCodeWriter::travel_to_z_force`: always emits the Z move and drops any lift, because
    /// custom G-code may have moved Z (e.g. the XL toolchange's `position[2]` hop).
    pub(crate) fn move_z_force(&mut self, z: f64, speed: f64) {
        self.stats.time_s += (z - self.z).abs() / speed;
        self.lifted = 0.0;
        self.z = z;
        let _ = writeln!(
            self.out,
            "G1 Z{} F{}",
            num(z + self.z_offset, 3),
            num(speed * 60.0, 0)
        );
        self.feedrate = None;
    }

    pub fn travel_to(&mut self, x: f64, y: f64, speed: f64) {
        if self.raised.is_some() {
            let z = self.z;
            self.clear_raised(self.moves_xy(x, y), z);
        }
        if let Some((cx, cy)) = self.position() {
            if (cx - x).abs() < 1e-9 && (cy - y).abs() < 1e-9 {
                return;
            }
            self.stats.time_s += (x - cx).hypot(y - cy) / speed;
        }
        let f = speed * 60.0;
        let _ = write!(
            self.out,
            "G1 X{} Y{}",
            num(x - self.offset.0, 3),
            num(y - self.offset.1, 3)
        );
        if self.feedrate != Some(f) {
            let _ = write!(self.out, " F{}", num(f, 0));
        }
        self.out.push('\n');
        self.x = Some(x);
        self.y = Some(y);
        self.feedrate = Some(f);
    }

    /// Emit an XY arc; `e` and `length` are based on its analytic length, not its chord.
    pub fn extrude_to_xy_g2g3(
        &mut self,
        xy: (f64, f64),
        arc: ArcCommand,
        e: f64,
        speed: f64,
        length: f64,
    ) {
        let f = speed * 60.0;
        if self.feedrate.is_none_or(|cur| (cur - f).abs() > 1e-6) {
            let _ = writeln!(self.out, "G1 F{}", num(f, 3));
            self.feedrate = Some(f);
        }
        extrusion_line(&mut self.out, xy, Some(arc), e, self.offset);
        self.stats.time_s += length / speed;
        self.stats.filament_mm += e;
        self.tool_filament[self.tool] += quantize_e(e);
        self.x = Some(xy.0);
        self.y = Some(xy.1);
    }

    /// Simultaneous XYZ extrusion, used for scarf ramps. The feed is the 3D move speed.
    pub fn extrude_to_xyz(&mut self, x: f64, y: f64, z: f64, e: f64, speed: f64) {
        let f = speed * 60.0;
        if self.feedrate.is_none_or(|cur| (cur - f).abs() > 1e-6) {
            let _ = writeln!(self.out, "G1 F{}", num(f, 3));
            self.feedrate = Some(f);
        }
        if let Some((cx, cy)) = self.position() {
            self.stats.time_s += (x - cx).hypot(y - cy).hypot(z - self.z) / speed;
        }
        self.stats.filament_mm += e;
        self.tool_filament[self.tool] += quantize_e(e);
        let _ = writeln!(
            self.out,
            "G1 X{} Y{} Z{} E{}",
            num(x - self.offset.0, 3),
            num(y - self.offset.1, 3),
            num(z + self.z_offset, 3),
            num(e, 5)
        );
        self.x = Some(x);
        self.y = Some(y);
        self.z = z;
    }

    /// Extrudes in a straight line to (`x`, `y`), pushing `e` mm of filament, at `speed` mm/s.
    pub fn extrude_to(&mut self, x: f64, y: f64, e: f64, speed: f64) {
        let f = speed * 60.0;
        if self.feedrate.is_none_or(|cur| (cur - f).abs() > 1e-6) {
            let _ = writeln!(self.out, "G1 F{}", num(f, 3));
            self.feedrate = Some(f);
        }
        if let Some((cx, cy)) = self.position() {
            self.stats.time_s += (x - cx).hypot(y - cy) / speed;
        }
        self.stats.filament_mm += e;
        self.tool_filament[self.tool] += quantize_e(e);
        extrusion_line(&mut self.out, (x, y), None, e, self.offset);
        self.x = Some(x);
        self.y = Some(y);
    }
    /// Append preformatted geometry while updating machine state and floating-point totals
    /// in the original segment order. A different live tool offset takes the normal path.
    pub(crate) fn prepared_extrusions(
        &mut self,
        moves: &[crate::smooth_path::Motion],
        text: &str,
        offset: (f64, f64),
        speed: f64,
    ) -> bool {
        if self.offset != offset {
            return false;
        }
        if moves.is_empty() {
            return true;
        }
        let f = speed * 60.;
        if self.feedrate.is_none_or(|cur| (cur - f).abs() > 1e-6) {
            let _ = writeln!(self.out, "G1 F{}", num(f, 3));
            self.feedrate = Some(f);
        }
        for m in moves {
            if m.command.is_some() {
                self.stats.time_s += m.length / speed;
            } else if let Some((cx, cy)) = self.position() {
                self.stats.time_s += (m.xy.0 - cx).hypot(m.xy.1 - cy) / speed;
            }
            self.stats.filament_mm += m.e;
            self.tool_filament[self.tool] += quantize_e(m.e);
            self.x = Some(m.xy.0);
            self.y = Some(m.xy.1);
        }
        self.out.push_str(text);
        true
    }
}

/// Shared formatting for streaming moves and bounded parallel preparation.
pub(crate) fn extrusion_line(
    out: &mut String,
    xy: (f64, f64),
    arc: Option<ArcCommand>,
    e: f64,
    offset: (f64, f64),
) {
    if let Some(arc) = arc {
        let (ccw, geometry) = match arc {
            ArcCommand::Center { ij, ccw } => (ccw, format!("I{} J{}", num(ij.0, 3), num(ij.1, 3))),
            ArcCommand::Radius { radius, ccw } => (ccw, format!("R{}", num(radius, 5))),
        };
        let _ = writeln!(
            out,
            "G{} X{} Y{} {} E{}",
            if ccw { 3 } else { 2 },
            num(xy.0 - offset.0, 3),
            num(xy.1 - offset.1, 3),
            geometry,
            num(e, 5)
        );
    } else {
        let _ = writeln!(
            out,
            "G1 X{} Y{} E{}",
            num(xy.0 - offset.0, 3),
            num(xy.1 - offset.1, 3),
            num(e, 5)
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Layer markers: six significant digits as PrusaSlicer and `%g` write them (GUI monkey
    /// test, seed 3007: layers at Z 0.799707 and 0.8 both read `.8`).
    #[test]
    fn six_significant_digits() {
        assert_eq!(num_g6(0.799707), ".799707");
        assert_eq!(num_g6(0.6000000000000001), ".6");
        assert_eq!(num_g6(0.2), ".2");
        assert_eq!(num_g6(48.), "48");
        assert_eq!(num_g6(12.3456789), "12.3457");
        assert_eq!(num_g6(123.4564), "123.456");
        assert_eq!(decimal_g6(0.849800001), "0.8498");
        assert_eq!(decimal_g6(1.0), "1");
    }

    #[test]
    fn prepared_extrusions_preserve_text_state_and_accumulation() {
        use crate::smooth_path::{self, ArcFitting, Prepared};
        use mica_core::geometry::{Point, SCALE, arc_welder::Segment};
        let path = vec![
            Segment::line(Point::from_mm(10., 0.)),
            Segment {
                point: Point::from_mm(0., 10.),
                radius: 10. * SCALE,
                ccw: true,
            },
            Segment::line(Point::from_mm(0., 10.)),
            Segment::line(Point::from_mm(2.3456, 12.1234)),
        ];
        for mode in [
            ArcFitting::Disabled,
            ArcFitting::EmitCenter,
            ArcFitting::EmitRadius,
        ] {
            for offset in [(0., 0.), (1.25, -2.5)] {
                let mut serial = Writer::new();
                let mut prepared = Writer::new();
                for writer in [&mut serial, &mut prepared] {
                    writer.offset = offset;
                    writer.travel_to(9.9876, 0.0012, 100.);
                }
                let moves = Prepared::new(&path, 0.035, mode, offset);
                for speed in [40., 40., 17.35] {
                    smooth_path::emit(&mut serial, &path, 0.035, speed, mode);
                    assert!(moves.emit(&mut prepared, speed, 0.035, mode));
                }
                assert_eq!(serial.out, prepared.out);
                assert_eq!(serial.stats, prepared.stats);
                assert_eq!(serial.tool_filament, prepared.tool_filament);
                assert_eq!(serial.position(), prepared.position());
                assert_eq!(serial.feedrate, prepared.feedrate);
                assert!(!moves.emit(&mut prepared, 40., 0.036, mode));
                prepared.offset.0 += 1.;
                assert!(!moves.emit(&mut prepared, 40., 0.035, mode));
                assert_eq!(serial.out, prepared.out);
            }
        }
    }

    #[test]
    fn prusa_accelerations_dedup_and_clamp_to_emitted_limits() {
        let mut c = Config::defaults();
        for (key, value) in [
            ("gcode_flavor", "marlin2"),
            ("machine_limits_usage", "emit_to_gcode"),
            ("machine_max_acceleration_extruding", "4000"),
            ("machine_max_acceleration_travel", "5000"),
        ] {
            c.set_raw(key, value).unwrap();
        }
        let mut w = Writer::with_config(Flavor::PrusaBuddy, &c);
        // GCodeWriter::m_last_travel_acceleration starts unknown: the first travel writes it.
        w.set_travel_acceleration(4000.);
        w.set_travel_acceleration(4000.);
        w.set_travel_acceleration(9000.);
        w.set_travel_acceleration(0.);
        w.set_acceleration(6000.);
        w.set_acceleration(4000.);
        assert_eq!(w.as_str(), "M204 T4000\nM204 T5000\nM204 P4000\n");
    }

    #[test]
    fn travel_insert_lands_before_the_second_to_last_point() {
        let mut w = Writer::new();
        w.travel_to_xyz([0., 0., 0.2], 100., 10.);
        let start = w.as_str().len();
        w.set_travel_insert("M486 S0\n".into());
        w.travel_points(&[[1., 0., 0.6], [2., 0., 0.6], [3., 0., 0.2]], 100., 10.);
        let text = &w.as_str()[start..];
        let lines: Vec<_> = text.lines().collect();
        assert_eq!(
            lines.iter().position(|l| *l == "M486 S0"),
            Some(1),
            "{text}"
        );
        // An empty insert writes nothing; a travel without points still flushes it.
        w.set_travel_insert(String::new());
        w.set_travel_insert("M486 S-1\n".into());
        w.travel_points(&[], 100., 10.);
        assert!(w.as_str().ends_with("M486 S-1\n"));
    }

    #[test]
    fn extruded_filament_counts_quantized_e() {
        // Extruder::extrude adds GCodeFormatter::quantize_e(dE), the E the G-code carries.
        let mut w = Writer::new();
        w.travel_to_xyz([0., 0., 0.2], 100., 10.);
        for i in 1..=3 {
            w.extrude_to(f64::from(i), 0., 0.333_334_4, 40.);
        }
        assert!((w.filament_for_tool(0) - 3. * 0.33333).abs() < 1e-12);
    }

    #[test]
    fn toolchange_retraction_tops_up_and_restores_each_tool() {
        let mut c = Config::defaults();
        c.set_raw("nozzle_diameter", "0.4,0.4").unwrap();
        let mut w = Writer::new();
        w.select_tool(0, &c);
        w.retract(0.8, 40.);
        w.retract_to(3., 40.);
        assert_eq!(w.tool_states(2), [(3., 0.), (0., 0.)]);
        w.select_tool(1, &c);
        w.retract_to(5., 50.);
        w.select_tool(0, &c);
        w.unretract(30.);
        assert_eq!(w.tool_states(2), [(0., 0.), (5., 0.)]);
        assert!(w.as_str().contains("G1 E-2.2 F2400"));
        assert!(w.as_str().contains("G1 E3 F1800"));
    }

    #[test]
    fn klipper_limits_zero_settings_and_pressure_advance() {
        use mica_core::config::{ConfigValue, Value};
        let mut c = Config::defaults();
        for (key, value) in [
            ("machine_max_acceleration_extruding", "12000"),
            ("machine_max_acceleration_x", "8000"),
            ("machine_max_acceleration_y", "6000"),
            ("machine_max_jerk_x", "8"),
            ("machine_max_jerk_y", "5"),
        ] {
            c.set_raw(key, value).unwrap();
        }
        c.set(
            "accel_to_decel_enable",
            ConfigValue::Scalar(Value::Bool(true)),
        );
        c.set(
            "accel_to_decel_factor",
            ConfigValue::Scalar(Value::Percent(50.0)),
        );
        let mut w = Writer::with_config(Flavor::Klipper, &c);
        w.set_accel_and_jerk(10000.0, 9.0);
        w.set_accel_and_jerk(10000.0, 9.0); // clamped values deduplicate
        w.set_accel_and_jerk(0.0, 0.0); // firmware defaults remain untouched
        w.set_accel_and_jerk(500.0, 3.0);
        w.set_pressure_advance(0.02);
        w.set_pressure_advance(-1.0);
        w.set_auxiliary_fan(70.0);
        assert_eq!(
            w.as_str(),
            "SET_VELOCITY_LIMIT ACCEL=6000 ACCEL_TO_DECEL=3000 SQUARE_CORNER_VELOCITY=5\nSET_VELOCITY_LIMIT ACCEL=500 ACCEL_TO_DECEL=250 SQUARE_CORNER_VELOCITY=3\nSET_PRESSURE_ADVANCE ADVANCE=0.02; Override pressure advance value\nM106 P2 S178\n"
        );
        c.set(
            "accel_to_decel_enable",
            ConfigValue::Scalar(Value::Bool(false)),
        );
        let mut w = Writer::with_config(Flavor::Klipper, &c);
        w.set_acceleration(3000.0);
        assert_eq!(w.as_str(), "SET_VELOCITY_LIMIT ACCEL=3000\n");
    }

    #[test]
    fn number_formatting_matches_prusaslicer() {
        assert_eq!(num(0.69118, 5), ".69118");
        assert_eq!(num(134.093, 3), "134.093");
        assert_eq!(num(0.2, 3), ".2");
        assert_eq!(num(2700.0, 0), "2700");
        assert_eq!(num(-0.7, 5), "-.7");
        assert_eq!(num(20.0, 3), "20");
        assert_eq!(num(-0.0000001, 3), "0");
        assert_eq!(num(127.5, 3), "127.5");
    }

    #[test]
    fn retract_lift_travel_restore() {
        let mut w = Writer::new();
        w.forget_position(0.2);
        w.retract(0.7, 45.0);
        w.lift(0.2, 12.0);
        w.travel_to(10.0, 20.0, 350.0);
        w.unlift(12.0);
        w.unretract(25.0);
        w.extrude_to(11.0, 20.0, 0.05, 50.0);
        assert_eq!(
            w.as_str(),
            "G1 E-.7 F2700\nG1 Z.4 F720\nG1 X10 Y20 F21000\nG1 Z.2 F720\nG1 E.7 F1500\nG1 F3000\nG1 X11 Y20 E.05\n"
        );
        assert!((w.stats.filament_mm - 0.05).abs() < 1e-12);
    }

    /// `z_offset` is added to every Z the writer emits, and templates see the machine Z
    /// (`position[2]`), while the writer's own Z stays the layers' Z.
    #[test]
    fn z_offset_moves_emitted_z_only() {
        let mut c = Config::defaults();
        c.set_raw("z_offset", "-0.1").unwrap();
        let mut w = Writer::with_config(Flavor::PrusaBuddy, &c);
        w.forget_position(0.2);
        w.lift(0.6, 12.);
        w.travel_to_xyz_force([10., 10., 0.4], 350., 0.);
        w.unlift_to(0.4, 12.);
        w.move_z_force(0.6, 12.);
        assert_eq!(
            w.as_str(),
            "G1 Z.7 F720\nG1 X10 Y10 Z.3 F21000\nG1 Z.5 F720\n"
        );
        assert_eq!(w.z(), 0.6);
        assert!((w.template_state().0[2] - 0.5).abs() < 1e-12);
        w.apply_template_state(Some([10., 10., 1.5]), 0., 0.);
        assert!((w.z() - 1.6).abs() < 1e-12);
    }

    /// PrusaSlicer's CoolingBuffer counts a retraction's F as `current_feedrate`: a layer
    /// change's ramp right after a travel at the same speed still states F21000.
    #[test]
    fn travel_after_retraction_states_its_feedrate() {
        let mut w = Writer::new();
        w.forget_position(19.2);
        w.travel_to(119.088, 115.593, 350.0);
        w.retract(0.7, 45.0);
        w.travel_to_xyz_force([119.088, 115.593, 19.0], 350.0, 0.0);
        w.travel_to_xyz_force([147.543, 116.292, 19.497], 350.0, 0.0);
        assert_eq!(
            w.as_str(),
            "G1 X119.088 Y115.593 F21000\nG1 E-.7 F2700\nG1 X119.088 Y115.593 Z19 F21000\nG1 X147.543 Y116.292 Z19.497\n"
        );
    }

    /// CoolingBuffer::apply_layer_cooldown: a retraction at the current F loses its F word (a
    /// CORE One first-layer retraction after a 45 mm/s extrusion); each layer starts afresh.
    #[test]
    fn prusa_retraction_drops_a_repeated_feedrate() {
        let mut w = Writer::with_config(Flavor::PrusaBuddy, &Config::defaults());
        w.forget_position(0.2);
        w.travel_to(10., 10., 350.);
        w.extrude_to(20., 10., 0.5, 45.);
        w.retract(0.7, 45.);
        w.unretract(25.);
        w.extrude_to(30., 10., 0.5, 45.);
        w.begin_cooling_layer();
        w.retract(0.7, 45.);
        assert_eq!(
            w.as_str(),
            "G1 X10 Y10 F21000\nG1 F2700\nG1 X20 Y10 E.5\nG1 E-.7\nG1 E.7 F1500\nG1 F2700\nG1 X30 Y10 E.5\nG1 E-.7 F2700\n"
        );
    }

    /// travel_to_first_position after a ramp: the zero-length XY move survives only with a new
    /// F (CoolingBuffer drops it at the current one); the Z move is always written.
    #[test]
    fn layer_arrival_drops_a_zero_length_move_at_the_current_feedrate() {
        let mut w = Writer::with_config(Flavor::PrusaBuddy, &Config::defaults());
        w.forget_position(0.4);
        w.travel_to(133.878, 117.789, 350.);
        w.travel_to_xyz_force([134.368, 119.368, 0.6], 350., 12.);
        w.confirm_layer_arrival((134.368, 119.368), 350., 12., 0.6);
        w.travel_to(110., 117.789, 350.);
        w.travel_to_xyz_force([137.953, 109.949, 0.8], 350., 12.);
        w.confirm_layer_arrival((137.953, 109.949), 350., 12., 0.8);
        let lines: Vec<&str> = w.as_str().lines().collect();
        assert!(lines[1].starts_with("G1 X134.368 Y119.368 Z.6 F5"));
        assert_eq!(&lines[2..4], ["G1 X134.368 Y119.368 F21000", "G1 Z.6 F720"]);
        assert_eq!(
            &lines[4..],
            [
                "G1 X110 Y117.789 F21000",
                "G1 X137.953 Y109.949 Z.8",
                "G1 Z.8 F720"
            ]
        );
    }
}
