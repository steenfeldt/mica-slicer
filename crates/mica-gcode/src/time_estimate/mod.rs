// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/GCodeProcessor.cpp :: apply_config, process_G1, process_G2_G3, process_G28, process_G92, process_M201, process_M203, process_M204, process_M205, process_M220, process_M221, process_M566, process_T, post_process
// Source: prusaslicer@b028299c77 src/libslic3r/Geometry/ArcWelder.hpp :: arc_discretization_steps, arc_center
// Source: orca@8500fcdcca src/libslic3r/GCode/GCodeProcessor.cpp :: process_SET_VELOCITY_LIMIT, process_SET_PRESSURE_ADVANCE, process_G2_G3, process_filament_change, get_extruder_change_time
// Changes: Two timing modes, bounded planner and per-tool explicit filament totals for tower jobs; disk-backed timeline replaces the unbounded move cache.
// Changes: The postprocess stage scans and rewinds text before rewriting, so totals are available to headers.
// Changes: The analysis also runs on a stream read once (`Analysis::from_stream`), for text analyzed while it is written.
// Changes: The analysis keeps its final layer count (upstream `m_layer_id` after processing) for the XL preheat pass.

// Changes: Independent Klipper tool changes include configured machine_tool_change_time in the shared preheat/progress timeline.
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/GCodeProcessor.cpp :: process_tags, store_move_vertex, process_custom_gcode_time, TimeMachine::calculate_time
// Changes: Colour-change and pause tags synchronise the planner and record printer stops; the rewrite pass emits
// Changes: upstream's remaining-time-to-stop lines (M73 C / D) from the stop times collected by the analysis pass.
//! Streaming G-code timing and M73 postprocessing. Memory is independent of print length:
//! at most 257 planner blocks per mode, one input line, and fixed-size buffered disk I/O.
//! The generation stage writes text; the postprocess stage analyzes then rewinds and rewrites it.
//! Heating, homing macros and dwell have no invented durations: the pinned Prusa processor ignores
//! G4/M140/M190, and M104/M109 only affect preview temperatures. Klipper macros are opaque too.

mod breakdown;
mod filament;
mod planner;
mod preheat;
pub use breakdown::{Breakdown, LayerBreakdown};

use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, BufReader, BufWriter, Read, Seek, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::writer::MoveStats;
use mica_core::config::Config;
use planner::{Machine, QUEUE, REFRESH};

pub const FIRST_M73: &str = ";_GP_FIRST_LINE_M73_PLACEHOLDER";
pub const LAST_M73: &str = ";_GP_LAST_LINE_M73_PLACEHOLDER";
pub const TIME_PLACEHOLDER: &str = ";_GP_ESTIMATED_PRINTING_TIME_PLACEHOLDER";
pub(crate) use filament::MARKER as FILAMENT_MARKER;

/// Times in seconds. The first layer includes the startup motion, as in PrusaSlicer.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Estimate {
    pub normal: f64,
    pub silent: Option<f64>,
    pub first_layer_normal: f64,
    pub first_layer_silent: Option<f64>,
}

impl Estimate {
    pub fn update_stats(self, stats: &mut MoveStats) {
        stats.time_s = self.normal;
        stats.silent_time_s = self.silent;
        stats.first_layer_time_s = self.first_layer_normal;
        stats.silent_first_layer_time_s = self.first_layer_silent;
    }

    fn modes(self) -> impl Iterator<Item = (usize, f64, f64)> {
        [
            (0, Some(self.normal), self.first_layer_normal),
            (1, self.silent, self.first_layer_silent.unwrap_or(0.0)),
        ]
        .into_iter()
        .filter_map(|(i, t, f)| t.map(|t| (i, t, f)))
    }
}

/// A line parser for motion words and Klipper named parameters. Comments never become commands.
struct Line<'a> {
    command: String,
    args: &'a str,
}
impl<'a> Line<'a> {
    fn new(text: &'a str) -> Self {
        let mut code = text.split(';').next().unwrap_or("").trim();
        if code.starts_with(['N', 'n']) {
            code = code.trim_start_matches(['N', 'n']);
            code = code
                .trim_start_matches(|c: char| c.is_ascii_digit())
                .trim_start();
        }
        let end = if code.starts_with(['G', 'g', 'M', 'm', 'T', 't'])
            && code
                .as_bytes()
                .get(1)
                .is_some_and(|c| c.is_ascii_digit() || *c == b'-')
        {
            code.char_indices()
                .skip(1)
                .find(|(_, c)| !c.is_ascii_digit() && *c != '.' && *c != '-')
                .map_or(code.len(), |(i, _)| i)
        } else {
            code.find(char::is_whitespace).unwrap_or(code.len())
        };
        Self {
            command: code[..end].to_ascii_uppercase(),
            args: &code[end..],
        }
    }
    fn word(&self, key: u8) -> Option<Option<f64>> {
        let bytes = self.args.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'*' {
                break;
            }
            if bytes[i] == b'(' || bytes[i] == b'"' {
                let end = if bytes[i] == b'(' { b')' } else { b'"' };
                i += 1;
                while i < bytes.len() && bytes[i] != end {
                    i += 1;
                }
            } else if bytes[i].to_ascii_uppercase() == key {
                i += 1;
                while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                    i += 1;
                }
                let start = i;
                while i < bytes.len()
                    && (bytes[i].is_ascii_digit() || matches!(bytes[i], b'+' | b'-' | b'.'))
                {
                    i += 1;
                }
                return Some(
                    self.args[start..i]
                        .parse::<f64>()
                        .ok()
                        .filter(|v| v.is_finite()),
                );
            }
            i += 1;
        }
        None
    }
    fn get(&self, key: u8) -> Option<f64> {
        self.word(key).flatten()
    }
    fn has(&self, key: u8) -> bool {
        self.word(key).is_some()
    }
    fn named(&self, key: &str) -> Option<f32> {
        let mut rest = self.args;
        while !rest.trim_start().is_empty() {
            rest = rest.trim_start();
            let n = rest
                .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                .unwrap_or(rest.len());
            let name = &rest[..n];
            rest = rest[n..].trim_start();
            if let Some(value) = rest.strip_prefix('=') {
                rest = value.trim_start();
                let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
                let value = &rest[..end];
                rest = &rest[end..];
                if name.eq_ignore_ascii_case(key) {
                    return value
                        .parse::<f32>()
                        .ok()
                        .filter(|v| v.is_finite() && *v >= 0.0);
                }
            } else {
                let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
                rest = &rest[end..];
            }
        }
        None
    }
}

/// Incremental estimator. Call `process_line` for each physical line, then `finish`.
/// The optional timeline sink receives fixed-size internal records, consumed by `Analysis`.
pub struct Estimator {
    filament: filament::Filament,
    machines: [Machine; 2],
    position: [f64; 4],
    origin: [f64; 4],
    saved_position: [f64; 4],
    relative: bool,
    relative_e: bool,
    units: f64,
    feed: f32,
    feed_factor: f32,
    saved_factor: f32,
    layer: u64,
    line: u64,
    g1: u64,
    envelope: bool,
    wiping: bool,
    klipper: bool,
    xl: bool,
    tool: usize,
    tool_selected: bool,
    unloaded: bool,
    load: Vec<f32>,
    unload: Vec<f32>,
    toolchange_time: f32,
    areas: Vec<f64>,
    volumetric: bool,
    /// Orca tracks pressure advance for statistics/preview; it does not change its time planner.
    pub pressure_advance: f32,
    /// Printer stops (colour changes and pauses): the G1 counter at the stop and the elapsed
    /// time of the last move before it per mode (upstream `TimeMachine::stop_times`, float).
    stops: Vec<(u64, [f32; 2])>,
}

impl Estimator {
    pub fn new(config: &Config) -> Self {
        let klipper = config.string("gcode_flavor") == Some("klipper");
        let vector = |key: &str| {
            (0..config.vector_len(key))
                .map(|i| config.float_at(key, i).unwrap_or(0.0) as f32)
                .collect()
        };
        Self {
            filament: filament::Filament::new(config),
            machines: std::array::from_fn(|i| Machine::new(config, i, klipper)),
            position: [0.0; 4],
            origin: [0.0; 4],
            saved_position: [0.0; 4],
            relative: false,
            relative_e: false,
            units: 1.0,
            feed: 0.0,
            feed_factor: 1.0,
            saved_factor: 1.0,
            layer: 0,
            line: 0,
            g1: 0,
            envelope: false,
            wiping: false,
            klipper,
            xl: config
                .string("printer_notes")
                .unwrap_or("")
                .contains("PRINTER_MODEL_XL"),
            tool: 0,
            tool_selected: false,
            unloaded: true,
            load: vector("filament_load_time"),
            unload: vector("filament_unload_time"),
            toolchange_time: if klipper {
                config
                    .float("machine_tool_change_time")
                    .or_else(|| {
                        config
                            .extra("machine_tool_change_time")
                            .and_then(|s| s.parse().ok())
                    })
                    .unwrap_or(0.)
                    .max(0.) as f32
            } else {
                0.
            },
            areas: (0..config
                .vector_len("filament_diameter")
                .max(config.vector_len("nozzle_diameter"))
                .max(1))
                .map(|i| {
                    std::f64::consts::PI
                        * (config.float_at("filament_diameter", i).unwrap_or(1.75) / 2.0).powi(2)
                })
                .collect(),
            volumetric: config.bool("use_volumetric_e").unwrap_or(false),
            pressure_advance: 0.0,
            stops: Vec::new(),
        }
    }

    /// Upstream `store_move_vertex` for a colour change or pause, then `simulate_st_synchronize`.
    fn stop(&mut self, sink: &mut dyn Write) -> io::Result<()> {
        self.stops.push((self.g1, [0.0; 2]));
        self.flush(0, 0.0, sink)
    }

    fn target(&self, line: &Line<'_>) -> [f64; 4] {
        std::array::from_fn(|i| {
            line.get(b"XYZE"[i]).map_or(self.position[i], |v| {
                let value = v * self.units
                    / if i == 3 && self.volumetric {
                        self.areas[self.tool.min(self.areas.len() - 1)]
                    } else {
                        1.0
                    };
                value
                    + if self.relative || (i == 3 && self.relative_e) {
                        self.position[i]
                    } else {
                        self.origin[i]
                    }
            })
        })
    }

    fn flush(&mut self, keep: usize, additional: f32, sink: &mut dyn Write) -> io::Result<()> {
        let normal = self.machines[0].calculate(keep, additional);
        let silent = self.machines[1].calculate(keep, additional);
        if !self.stops.is_empty() {
            // Each stop keeps the time of the last block at or before it.
            for (mode, records) in [&normal, &silent].into_iter().enumerate() {
                for &(line, time) in records {
                    let i = self.stops.partition_point(|s| s.0 < line);
                    if let Some(stop) = self.stops.get_mut(i) {
                        stop.1[mode] = time as f32;
                    }
                }
            }
        }
        for (i, (line, normal)) in normal.into_iter().enumerate() {
            sink.write_all(&line.to_le_bytes())?;
            sink.write_all(&normal.to_le_bytes())?;
            sink.write_all(&silent.get(i).map_or(0.0, |(_, v)| *v).to_le_bytes())?;
        }
        Ok(())
    }

    fn motion(&mut self, target: [f64; 4], sink: &mut dyn Write) -> io::Result<()> {
        self.envelope = true;
        self.g1 += 1;
        let delta = std::array::from_fn(|i| target[i] - self.position[i]);
        self.filament.motion(self.tool, delta[3]);
        self.position = target;
        for machine in &mut self.machines {
            machine.push(delta, self.feed, self.g1, self.layer <= 1, self.wiping);
        }
        if self.machines[0].len() > REFRESH {
            self.flush(QUEUE, 0.0, sink)?;
        }
        Ok(())
    }

    /// Processes a line, ignoring existing M73 and estimated-time comments.
    pub fn process_line(&mut self, text: &str, sink: &mut dyn Write) -> io::Result<()> {
        self.line += 1;
        let tag = text.trim();
        if tag == FILAMENT_MARKER {
            self.filament.enabled = true;
        }
        if matches!(tag, ";LAYER_CHANGE" | "; CHANGE_LAYER") {
            self.layer += 1;
        }
        if tag == ";WIPE_START" {
            self.wiping = true;
        }
        if tag == ";WIPE_END" {
            self.wiping = false;
        }
        if let Some(rest) = tag.strip_prefix(";COLOR_CHANGE") {
            // `;COLOR_CHANGE,T<extruder>,<colour>`: a stop for the extruder printing now.
            let mut extruder = Some(0usize);
            if let Some(t) = rest.split(',').find(|t| !t.is_empty())
                && let Some(id) = t.strip_prefix('T')
            {
                extruder = id.parse::<usize>().ok().filter(|e| *e <= 255);
            }
            if extruder == Some(self.tool) {
                self.stop(sink)?;
            }
            return Ok(());
        }
        if tag == ";PAUSE_PRINT" {
            self.stop(sink)?;
            return Ok(());
        }
        let line = Line::new(text);
        match line.command.as_str() {
            "G0" | "G1" => {
                if let Some(f) = line.get(b'F') {
                    self.feed = self.feed_factor * f as f32 / 60.0;
                }
                self.motion(self.target(&line), sink)?;
            }
            "G2" | "G3" => self.arc(&line, sink)?,
            "G20" => self.units = 25.4,
            "G21" => self.units = 1.0,
            "G28" => {
                let any = b"XYZ".iter().any(|a| line.has(*a));
                let mut target = self.position;
                for (i, target) in target.iter_mut().enumerate().take(3) {
                    if !any || line.has(b"XYZ"[i]) {
                        *target = if self.relative {
                            self.position[i]
                        } else {
                            self.origin[i]
                        };
                    }
                }
                self.motion(target, sink)?;
            }
            "G60" => self.saved_position = self.position,
            "G61" => {
                let any = b"XYZE".iter().any(|a| line.has(*a));
                for i in 0..4 {
                    if !any || line.has(b"XYZE"[i]) {
                        self.position[i] = self.saved_position[i];
                    }
                }
                if let Some(f) = line.get(b'F') {
                    self.feed = self.feed_factor * f as f32;
                }
            }
            "G90" => self.relative = false,
            "G91" => self.relative = true,
            "M82" => self.relative_e = false,
            "M83" => self.relative_e = true,
            "G92" => {
                for i in 0..3 {
                    if let Some(v) = line.get(b"XYZ"[i]) {
                        self.origin[i] = self.position[i] - v * self.units;
                    }
                }
                if let Some(e) = line.get(b'E') {
                    self.position[3] = e * self.units;
                } else {
                    self.flush(0, 0.0, sink)?;
                }
                if line.args.trim().is_empty() {
                    self.origin = self.position;
                }
            }
            "M1" => {
                self.flush(0, 0.0, sink)?;
            }
            "M201" | "M203" | "M204" | "M205" | "M566" => {
                for (mode, m) in self.machines.iter_mut().enumerate() {
                    if mode != 0 && !self.envelope && line.command != "M566" {
                        continue;
                    }
                    match line.command.as_str() {
                        "M201" | "M203" | "M566" => {
                            let factor = match line.command.as_str() {
                                "M201" => self.units as f32,
                                "M203" if !self.klipper => 1.0,
                                _ => 1.0 / 60.0,
                            };
                            let values = match line.command.as_str() {
                                "M201" => &mut m.axis_accel,
                                "M203" => &mut m.axis_speed,
                                _ => &mut m.jerk,
                            };
                            for (i, value) in values.iter_mut().enumerate() {
                                if let Some(v) = line.get(b"XYZE"[i]) {
                                    *value = v as f32 * factor;
                                }
                            }
                        }
                        "M204" => {
                            if let Some(s) = line.get(b'S') {
                                m.set_acceleration(0, s as f32);
                                m.set_acceleration(2, s as f32);
                                if let Some(t) = line.get(b'T') {
                                    m.set_acceleration(1, t as f32);
                                }
                            } else {
                                for (i, key) in b"PRT".iter().enumerate() {
                                    if let Some(v) = line.get(*key) {
                                        m.set_acceleration(i, v as f32);
                                    }
                                }
                            }
                        }
                        "M205" => {
                            if let Some(x) = line.get(b'X') {
                                m.jerk[1] = x as f32;
                            }
                            for i in 0..4 {
                                if let Some(v) = line.get(b"XYZE"[i]) {
                                    m.jerk[i] = v as f32;
                                }
                            }
                            for i in 0..2 {
                                if let Some(v) = line.get(b"ST"[i]) {
                                    m.min_rate[i] = v as f32;
                                }
                            }
                            // The pinned processor reads junction deviation from config, not M205 J.
                        }
                        _ => unreachable!(),
                    }
                }
            }
            "M220" => {
                if line.has(b'B') {
                    self.saved_factor = self.feed_factor;
                }
                if let Some(s) = line.get(b'S') {
                    self.feed_factor = s as f32 * 0.01;
                }
                if line.has(b'R') {
                    self.feed_factor = self.saved_factor;
                }
            }
            "M221" => {
                if !line.has(b'T')
                    && let Some(s) = line.get(b'S')
                {
                    for m in &mut self.machines {
                        m.flow = s as f32 * 0.01;
                    }
                }
            }
            "SET_VELOCITY_LIMIT" if self.klipper => {
                for m in &mut self.machines {
                    if let Some(v) = line.named("SQUARE_CORNER_VELOCITY") {
                        m.jerk[..2].fill(v);
                    }
                    if let Some(v) = line.named("ACCEL") {
                        m.set_acceleration(0, v);
                        m.set_acceleration(2, v);
                    }
                    if let Some(v) = line.named("VELOCITY") {
                        m.axis_speed[..2].fill(v);
                    }
                }
                // Orca does not simulate ACCEL_TO_DECEL or MINIMUM_CRUISE_RATIO.
            }
            "SET_PRESSURE_ADVANCE" if self.klipper => {
                if let Some(v) = line.named("ADVANCE") {
                    self.pressure_advance = v;
                }
            }
            "M702" if line.has(b'C') => {
                self.unloaded = true;
                self.flush(0, 0.0, sink)?;
            }
            cmd if cmd.starts_with('T') => {
                if let Ok(tool) = cmd[1..].parse::<usize>()
                    && tool < self.areas.len()
                {
                    if tool == self.tool {
                        self.tool_selected = true;
                        return Ok(());
                    }
                    let unload = if self.unloaded || self.xl {
                        0.0
                    } else {
                        self.unload
                            .get(self.tool)
                            .or(self.unload.first())
                            .copied()
                            .unwrap_or(0.0)
                    };
                    let load = if self.xl {
                        4.5
                    } else {
                        self.load
                            .get(tool)
                            .or(self.load.first())
                            .copied()
                            .unwrap_or(0.0)
                    };
                    let change_time = if self.klipper && !self.tool_selected {
                        0.
                    } else {
                        self.toolchange_time
                    };
                    self.tool_selected = true;
                    self.tool = tool;
                    self.unloaded = false;
                    self.flush(0, unload + load + change_time, sink)?;
                }
            }
            // G4 and heater waits contribute no time in pinned PrusaSlicer. Unknown macros stay opaque.
            _ => (),
        }
        Ok(())
    }

    fn arc(&mut self, line: &Line<'_>, sink: &mut dyn Write) -> io::Result<()> {
        let start = self.position;
        let end = self.target(line);
        let clockwise = line.command == "G2";
        let center = if let Some(r) = line.get(b'R') {
            let dx = (end[0] - start[0]) as f32;
            let dy = (end[1] - start[1]) as f32;
            let r = r as f32;
            let d2 = dx * dx + dy * dy;
            if r == 0.0 || d2 == 0.0 || 4.0 * r * r < d2 {
                return Ok(());
            }
            let h = (r * r / d2 - 0.25).sqrt() * if clockwise != (r < 0.0) { -1.0 } else { 1.0 };
            [
                (0.5 * dx - h * dy) * self.units as f32,
                (0.5 * dy + h * dx) * self.units as f32,
            ]
        } else if line.has(b'I') || line.has(b'J') {
            [
                line.get(b'I').unwrap_or(0.0) as f32 * self.units as f32,
                line.get(b'J').unwrap_or(0.0) as f32 * self.units as f32,
            ]
        } else {
            return Ok(());
        };
        let radius = (center[0] * center[0] + center[1] * center[1]).sqrt();
        if radius == 0.0 {
            return Ok(());
        }
        let a = [-f64::from(center[0]), -f64::from(center[1])];
        let b = [
            end[0] - start[0] - f64::from(center[0]),
            end[1] - start[1] - f64::from(center[1]),
        ];
        let angle = if (end[0] - start[0]).abs() < 1e-4 && (end[1] - start[1]).abs() < 1e-4 {
            std::f64::consts::TAU
        } else {
            let mut angle = (a[0] * b[1] - a[1] * b[0]).atan2(a[0] * b[0] + a[1] * b[1]);
            if angle < 0.0 {
                angle += std::f64::consts::TAU;
            }
            if clockwise {
                angle -= std::f64::consts::TAU;
            }
            angle
        };
        if (angle * f64::from(radius)).hypot(end[2] - start[2]) < 0.001 {
            return Ok(());
        }
        let arc_feed = line.get(b'F').map(|f| self.feed_factor * f as f32 / 60.0);
        let segment_mm = (8.0 * radius * 0.02)
            .sqrt()
            .min(arc_feed.unwrap_or(self.feed) / 50.0)
            .clamp(0.1, 2.0);
        let segments = if !self.klipper {
            ((radius * angle.abs() as f32 / segment_mm + 0.8) as usize).max(1)
        } else {
            let step = 2.0 * (1.0 - 0.0125 / f64::from(radius)).clamp(-1.0, 1.0).acos();
            (angle.abs() / step).ceil().max(1.0) as usize
        };
        // Preserve upstream's explicit-F arc path (it passes mm/s into process_G1's mm/min input).
        if let Some(f) = arc_feed {
            self.feed = if self.klipper {
                f
            } else {
                self.feed_factor * f / 60.0
            };
        }
        let theta = angle as f32 / segments as f32;
        let (sin, cos) = theta.sin_cos();
        let mut r = [-center[0], -center[1]];
        for i in 1..=segments {
            let target = if i == segments {
                end
            } else {
                if i % 25 == 0 {
                    let (s, c) = (i as f32 * theta).sin_cos();
                    r = [
                        -center[0] * c + center[1] * s,
                        -center[0] * s - center[1] * c,
                    ];
                } else {
                    r = [r[0] * cos - r[1] * sin, r[0] * sin + r[1] * cos];
                }
                let fraction = i as f64 / segments as f64;
                [
                    start[0] + f64::from(center[0]) + f64::from(r[0]),
                    start[1] + f64::from(center[1]) + f64::from(r[1]),
                    start[2] + fraction * (end[2] - start[2]),
                    start[3] + fraction * (end[3] - start[3]),
                ]
            };
            self.motion(target, sink)?;
        }
        Ok(())
    }

    pub fn finish(mut self, sink: &mut dyn Write) -> io::Result<Estimate> {
        self.flush(0, 0.0, sink)?;
        Ok(Estimate {
            normal: self.machines[0].time,
            silent: self.machines[1].enabled.then_some(self.machines[1].time),
            first_layer_normal: f64::from(self.machines[0].first_layer),
            first_layer_silent: self.machines[1]
                .enabled
                .then_some(f64::from(self.machines[1].first_layer)),
        })
    }
}

/// Estimate a stream without storing a timeline. Existing time comments are never used as input.
pub fn estimate(src: &mut dyn BufRead, config: &Config) -> io::Result<Estimate> {
    run(src, config, &mut io::sink(), &mut io::sink()).map(|(estimate, ..)| estimate)
}
/// Printer stops of a program: per stop, the elapsed time before it in each mode.
type Stops = Vec<[f32; 2]>;

fn run(
    src: &mut dyn BufRead,
    config: &Config,
    timeline: &mut dyn Write,
    mapping: &mut dyn Write,
) -> io::Result<(Estimate, filament::Filament, Stops, u64)> {
    let mut estimator = Estimator::new(config);
    let mut line = String::new();
    loop {
        line.clear();
        if src.read_line(&mut line)? == 0 {
            break;
        }
        estimator.process_line(&line, timeline)?;
        if matches!(
            Line::new(&line).command.as_str(),
            "G0" | "G1" | "G2" | "G3" | "G28"
        ) {
            // Prusa post_process starts its G1 counter at zero, while process_G1 starts at one.
            // For arcs it advances to the penultimate internal segment. Keep that placement.
            mapping.write_all(&estimator.line.to_le_bytes())?;
            mapping.write_all(&estimator.g1.saturating_sub(1).to_le_bytes())?;
        }
    }
    let filament = std::mem::take(&mut estimator.filament);
    estimator.flush(0, 0.0, timeline)?;
    let stops = estimator.stops.iter().map(|s| s.1).collect();
    let layers = estimator.layer;
    Ok((estimator.finish(timeline)?, filament, stops, layers))
}

struct Timeline {
    file: File,
    path: PathBuf,
}
impl Timeline {
    fn new() -> io::Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        loop {
            let path = std::env::temp_dir().join(format!(
                "mica-time-{}-{}.bin",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            match OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(file) => return Ok(Self { file, path }),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
    }
}
impl Drop for Timeline {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Prepared second-pass analysis. The timeline is deleted on drop, including on errors.
pub struct Analysis {
    pub estimate: Estimate,
    filament: filament::Filament,
    stops: Stops,
    /// Layer-change tags of the analyzed program (upstream's final `m_layer_id`).
    layers: u64,
    timeline: Timeline,
    mapping: Timeline,
    start: u64,
    progress: bool,
    config: Config,
}
impl Analysis {
    pub fn new<R: BufRead + Seek>(src: &mut R, config: &Config) -> io::Result<Self> {
        let start = src.stream_position()?;
        let analysis = Self::from_stream(src, config, start)?;
        src.seek(io::SeekFrom::Start(start))?;
        Ok(analysis)
    }

    /// Analyzes `src` read once to its end, for text that [`Analysis::write`] later reads
    /// from position `start` of a seekable copy (the export analyzes its text while writing
    /// it). The same as [`Analysis::new`] on that copy.
    pub fn from_stream(src: &mut dyn BufRead, config: &Config, start: u64) -> io::Result<Self> {
        let mut timeline = Timeline::new()?;
        let mut mapping = Timeline::new()?;
        let (estimate, filament, stops, layers) = {
            let mut sink = BufWriter::new(&mut timeline.file);
            let mut map_sink = BufWriter::new(&mut mapping.file);
            let estimate = run(src, config, &mut sink, &mut map_sink)?;
            map_sink.flush()?;
            sink.flush()?;
            estimate
        };
        Ok(Self {
            estimate,
            filament,
            stops,
            layers,
            timeline,
            mapping,
            start,
            config: config.clone(),
            progress: if config.string("gcode_flavor") == Some("klipper") {
                !config.bool("disable_m73").unwrap_or(false)
            } else {
                config.bool("remaining_times").unwrap_or(false)
            },
        })
    }

    pub fn update_stats(&self, stats: &mut MoveStats) {
        self.estimate.update_stats(stats);
        if self.filament.enabled {
            stats.filament_mm = self.filament.total_mm();
        }
    }

    pub fn write<R: BufRead + Seek>(
        &mut self,
        src: &mut R,
        dst: &mut dyn Write,
        mut stats: MoveStats,
    ) -> io::Result<MoveStats> {
        let mut preheat = preheat::Preheat::new(
            dst,
            &self.config,
            self.estimate.silent.is_some(),
            self.layers,
        );
        let dst = &mut preheat;
        self.update_stats(&mut stats);
        src.seek(io::SeekFrom::Start(self.start))?;
        self.timeline.file.rewind()?;
        let mut timeline = BufReader::new(&mut self.timeline.file);
        let mut next = record(&mut timeline)?;
        self.mapping.file.rewind()?;
        let mut mapping = BufReader::new(&mut self.mapping.file);
        let mut next_map = map_record(&mut mapping)?;
        let mut last = [
            (0, minutes(self.estimate.normal)),
            (0, minutes(self.estimate.silent.unwrap_or(0.0))),
        ];
        let mut last_stop = [i64::from(last[0].1), i64::from(last[1].1)];
        let mut line = String::new();
        let mut id = 0u64;
        let mut before_motion = true;
        loop {
            line.clear();
            if src.read_line(&mut line)? == 0 {
                break;
            }
            id += 1;
            if !line.ends_with('\n') {
                line.push('\n');
            }
            let mut elapsed = None;
            if let Some((physical, target)) = next_map
                && physical == id
            {
                while let Some((g1, _)) = next {
                    if g1 >= target {
                        break;
                    }
                    next = record(&mut timeline)?;
                }
                if let Some((g1, times)) = next
                    && g1 == target
                {
                    elapsed = Some(times);
                }
                // Progress is placed at the preceding G1 cache entry, while upstream's
                // preheat backtrace timestamps a motion with its own completed time.
                while let Some((g1, _)) = next {
                    if g1 > target {
                        break;
                    }
                    next = record(&mut timeline)?;
                }
                if let Some((g1, time)) = next
                    && g1 == target + 1
                {
                    dst.set_time(time);
                }
                next_map = map_record(&mut mapping)?;
            }
            if Line::new(&line).command == "G28" {
                elapsed = None;
            }
            let trimmed = line.trim();
            let parsed = Line::new(&line);
            if matches!(parsed.command.as_str(), "G0" | "G1" | "G2" | "G3" | "G28") {
                before_motion = false;
            }
            if let Some(replacement) = self.filament.replace(trimmed) {
                dst.write_all(replacement.as_bytes())?;
            } else if trimmed == FIRST_M73 || trimmed == LAST_M73 {
                if self.progress {
                    for (mode, total, _) in self.estimate.modes() {
                        write_m73(
                            dst,
                            mode,
                            if trimmed == FIRST_M73 {
                                (0, minutes(total))
                            } else {
                                (100, 0)
                            },
                        )?;
                        // Remaining time to the next printer stop.
                        if trimmed == FIRST_M73
                            && let Some(stop) = self.stops.first()
                        {
                            let to_stop = stop_minutes(stop[mode]);
                            write_m73_stop(dst, mode, &to_stop.to_string())?;
                            last_stop[mode] = to_stop;
                        }
                    }
                }
            } else if parsed.command == "M73" {
                // Reprocessing an exported file must also refresh its boundary M73 commands.
                if self.progress {
                    for (mode, total, _) in self.estimate.modes() {
                        let value = parsed.get(if mode == 0 { b'P' } else { b'Q' });
                        if value == Some(0.0) && before_motion {
                            write_m73(dst, mode, (0, minutes(total)))?;
                        } else if value == Some(100.0) {
                            write_m73(dst, mode, (100, 0))?;
                        }
                    }
                }
                if !parsed.has(b'P')
                    && !parsed.has(b'Q')
                    && !parsed.has(b'R')
                    && !parsed.has(b'S')
                    && !(self.progress && (parsed.has(b'C') || parsed.has(b'D')))
                {
                    dst.write_all(line.as_bytes())?;
                }
            } else if trimmed == TIME_PLACEHOLDER
                || trimmed.starts_with("; estimated printing time (normal mode)")
            {
                for (mode, total, _) in self.estimate.modes() {
                    dst.write_all(
                        format!(
                            "; estimated printing time ({} mode) = {}\n",
                            mode_name(mode),
                            crate::prusa_metadata::time_dhms(total)
                        )
                        .as_bytes(),
                    )?;
                }
                for (mode, _, first) in self.estimate.modes() {
                    dst.write_all(
                        format!(
                            "; estimated first layer printing time ({} mode) = {}\n",
                            mode_name(mode),
                            crate::prusa_metadata::time_dhms(first)
                        )
                        .as_bytes(),
                    )?;
                }
            } else if !trimmed.starts_with("; estimated printing time (silent mode)")
                && !trimmed.starts_with("; estimated first layer printing time (")
            {
                if line.contains("__MICA_") {
                    dst.write_all(crate::orca::replace_statistics(line.clone(), stats).as_bytes())?;
                } else {
                    dst.write_all(line.as_bytes())?;
                }
            }
            if self.progress
                && let Some(elapsed) = elapsed
            {
                for (mode, total, _) in self.estimate.modes() {
                    if total <= 0.0 {
                        continue;
                    }
                    // Upstream stores elapsed time as float in its G1 cache.
                    let t = f64::from(elapsed[mode] as f32);
                    let pair = (
                        ((f64::from(100.0f32 * elapsed[mode] as f32) / total) as u32).min(100),
                        minutes((total - t).max(0.0)),
                    );
                    if pair != last[mode] {
                        write_m73(dst, mode, pair)?;
                        last[mode] = pair;
                    }
                    stop_progress(
                        &self.stops,
                        dst,
                        mode,
                        elapsed[mode] as f32,
                        next,
                        &mut last_stop,
                    )?;
                }
            }
        }
        dst.finish()?;
        Ok(stats)
    }
}
/// Upstream's remaining time to the next printer stop after a move that finished at
/// `elapsed` (`next` is the following timeline entry): whole minutes while more than one is
/// left, then once the stop's last minute as a fraction (an integer for the last stop).
fn stop_progress(
    stops: &[[f32; 2]],
    dst: &mut dyn Write,
    mode: usize,
    elapsed: f32,
    next: Option<(u64, [f64; 2])>,
    last_stop: &mut [i64; 2],
) -> io::Result<()> {
    let upper = |t: f32| stops.partition_point(|s| t >= s[mode]);
    let index = upper(elapsed);
    let Some(stop) = stops.get(index) else {
        return Ok(());
    };
    let to_stop = stop_minutes(stop[mode] - elapsed);
    if last_stop[mode] == to_stop {
        return Ok(());
    }
    if to_stop > 0 {
        write_m73_stop(dst, mode, &to_stop.to_string())?;
        last_stop[mode] = to_stop;
        return Ok(());
    }
    let mut is_last = next.is_none();
    if let Some((_, times)) = next {
        let next_elapsed = times[mode] as f32;
        is_last |= upper(next_elapsed) != index;
        let here = format!("{:.2}", f64::from((stop[mode] - elapsed) / 60.0));
        let there = format!("{:.2}", f64::from((stop[mode] - next_elapsed) / 60.0));
        is_last |=
            here.parse::<f64>().unwrap_or(0.) > 0. && there.parse::<f64>().unwrap_or(0.) == 0.;
    }
    if is_last {
        if index + 1 == stops.len() {
            write_m73_stop(dst, mode, &to_stop.to_string())?;
        } else {
            write_m73_stop(
                dst,
                mode,
                &format!("{:.2}", f64::from((stop[mode] - elapsed) / 60.0)),
            )?;
        }
        last_stop[mode] = to_stop;
    }
    Ok(())
}
/// Upstream `time_in_minutes` on float seconds.
fn stop_minutes(seconds: f32) -> i64 {
    ((seconds + 0.5) / 60.0) as i64
}
fn write_m73_stop(dst: &mut dyn Write, mode: usize, value: &str) -> io::Result<()> {
    dst.write_all(format!("M73 {}{value}\n", if mode == 0 { 'C' } else { 'D' }).as_bytes())
}
fn mode_name(mode: usize) -> &'static str {
    if mode == 0 { "normal" } else { "silent" }
}
fn minutes(seconds: f64) -> u32 {
    ((seconds as f32 + 0.5) / 60.0) as u32
}
fn write_m73(dst: &mut dyn Write, mode: usize, (percent, remaining): (u32, u32)) -> io::Result<()> {
    if mode == 0 {
        dst.write_all(format!("M73 P{percent} R{remaining}\n").as_bytes())
    } else {
        dst.write_all(format!("M73 Q{percent} S{remaining}\n").as_bytes())
    }
}
fn record(src: &mut impl Read) -> io::Result<Option<(u64, [f64; 2])>> {
    let mut bytes = [0u8; 24];
    if src.read(&mut bytes[..1])? == 0 {
        return Ok(None);
    }
    src.read_exact(&mut bytes[1..])?;
    Ok(Some((
        u64::from_le_bytes(bytes[..8].try_into().unwrap()),
        [
            f64::from_le_bytes(bytes[8..16].try_into().unwrap()),
            f64::from_le_bytes(bytes[16..].try_into().unwrap()),
        ],
    )))
}

fn map_record(src: &mut impl Read) -> io::Result<Option<(u64, u64)>> {
    let mut bytes = [0u8; 16];
    if src.read(&mut bytes[..1])? == 0 {
        return Ok(None);
    }
    src.read_exact(&mut bytes[1..])?;
    Ok(Some((
        u64::from_le_bytes(bytes[..8].try_into().unwrap()),
        u64::from_le_bytes(bytes[8..].try_into().unwrap()),
    )))
}

#[cfg(test)]
mod tests;
