// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/SpiralVase.cpp :: SpiralVase::process_layer, get_layer_distancer
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/SpiralVase.hpp :: SpiralVase::SpiralVase, SpiralVase::enable
// Source: prusaslicer@b028299c77 src/libslic3r/GCodeReader.cpp :: GCodeReader::parse_line_internal, GCodeReader::update_coordinates, GCodeReader::GCodeLine::set
// Source: orca@8500fcdcca src/libslic3r/GCode/SpiralVase.cpp :: SpiralVase::process_layer, SpiralVaseHelpers::nearest_point_on_line, SpiralVaseHelpers::nearest_point_on_lines
// Source: orca@8500fcdcca src/libslic3r/GCode/SpiralVase.hpp :: SpiralVase::SpiralVase, SpiralVase::enable
// Source: orca@8500fcdcca src/libslic3r/GCode.cpp :: GCode::_do_export
// Changes: One post-processor for both rules (`Rules`): PrusaSlicer always smooths XY over 2× the largest nozzle and skips a line whose quantized point does not move; Orca smooths only with spiral_mode_smooth (spiral_mode_max_xy_smoothing), drops retractions and segments shorter than 2× resolution, and ramps flow from/to its starting/finishing ratios.
// Changes: The reader keeps upstream's float arithmetic and fixed-decimal number rewriting; a line missing X or Y takes the reader's position for it (Mica's cooling may split a move into one axis). The nearest point on the previous layer is searched in a grid of the smoothing distance instead of an AABB tree (same nearest segment within it).

//! Spiral vase G-code post-processor: on a spiral layer, Z rises with every extrusion move in
//! proportion to the length printed, so the wall becomes one continuous spiral without a seam.
//!
//! As upstream, it reads every layer's G-code text in print order (to follow the position),
//! rewrites the layers [`SpiralVase::enable`]d for it, ramps the flow in over the first spiral
//! layer, blends each point with the nearest point of the previous spiral layer (XY smoothing),
//! and adds a last loop at the final height whose flow ramps out.

use mica_core::config::Config;
use mica_core::geometry::Point;
use mica_core::toolpath::{ExtrusionPath, Role};

/// The axes `GCodeReader` tracks.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Axis {
    X = 0,
    Y = 1,
    Z = 2,
    E = 3,
    F = 4,
}
const AXES: [Axis; 5] = [Axis::X, Axis::Y, Axis::Z, Axis::E, Axis::F];
const UNKNOWN_AXIS: u32 = 1 << 5;

impl Axis {
    fn letter(self) -> char {
        match self {
            Axis::X => 'X',
            Axis::Y => 'Y',
            Axis::Z => 'Z',
            Axis::E => 'E',
            Axis::F => 'F',
        }
    }
}

/// `GCodeReader::GCodeLine`: the raw text and the parsed axis values (floats, as upstream).
#[derive(Clone)]
struct Line {
    raw: String,
    axis: [f32; 5],
    mask: u32,
    /// The command word ("G1", "M106", …; empty for a comment or blank line).
    command_end: usize,
    command_start: usize,
}

impl Line {
    fn parse(raw: &str) -> Self {
        let bytes = raw.as_bytes();
        let mut line = Self {
            raw: raw.to_string(),
            axis: [0.; 5],
            mask: 0,
            command_start: 0,
            command_end: 0,
        };
        let is_space = |c: u8| c == b' ' || c == b'\t';
        let end_of_line = |c: u8| c == b';' || c == b'\r' || c == b'\n';
        let mut i = 0;
        while i < bytes.len() && is_space(bytes[i]) {
            i += 1;
        }
        line.command_start = i;
        while i < bytes.len() && !is_space(bytes[i]) && !end_of_line(bytes[i]) {
            i += 1;
        }
        line.command_end = i;
        loop {
            while i < bytes.len() && is_space(bytes[i]) {
                i += 1;
            }
            if i >= bytes.len() || end_of_line(bytes[i]) {
                break;
            }
            let letter = bytes[i];
            let word_end = (i + 1..bytes.len())
                .find(|&k| is_space(bytes[k]) || end_of_line(bytes[k]))
                .unwrap_or(bytes.len());
            let axis = AXES.iter().find(|a| a.letter() as u8 == letter).copied();
            if (axis.is_some() || letter.is_ascii_uppercase())
                && let Ok(v) = raw[i + 1..word_end].trim_start().parse::<f64>()
            {
                match axis {
                    Some(a) => {
                        line.axis[a as usize] = v as f32;
                        line.mask |= 1 << a as u32;
                    }
                    None => line.mask |= UNKNOWN_AXIS,
                }
            }
            i = word_end;
        }
        line
    }
    fn command(&self) -> &str {
        &self.raw[self.command_start..self.command_end]
    }
    fn cmd_is(&self, command: &str) -> bool {
        self.command() == command
    }
    fn has(&self, axis: Axis) -> bool {
        self.mask & (1 << axis as u32) != 0
    }
    fn value(&self, axis: Axis) -> f32 {
        self.axis[axis as usize]
    }
    /// `GCodeLine::set`: writes `value` with `decimals` fixed decimals in place of the axis'
    /// word, or inserts it after the command.
    fn set(&mut self, axis: Axis, value: f32, decimals: usize) {
        let text = format!("{:.*}", decimals, f64::from(value));
        let word = format!(" {}", axis.letter());
        if self.has(axis) {
            if let Some(found) = self.raw.find(&word) {
                let pos = found + 2;
                let end = self.raw[(pos + 1).min(self.raw.len())..]
                    .find(' ')
                    .map_or(self.raw.len(), |k| k + pos + 1);
                self.raw.replace_range(pos..end, &text);
            }
        } else {
            match self.raw.find(' ') {
                None => {
                    self.raw.push_str(&word);
                    self.raw.push_str(&text);
                }
                Some(pos) => self.raw.insert_str(pos, &format!("{word}{text}")),
            }
        }
        self.axis[axis as usize] = value;
        self.mask |= 1 << axis as u32;
    }
}

/// `GCodeReader`'s position (relative extrusion: E restarts at 0 on every line with E).
#[derive(Clone, Copy, Default)]
struct Reader {
    position: [f32; 5],
}

impl Reader {
    fn x(&self) -> f32 {
        self.position[0]
    }
    fn y(&self) -> f32 {
        self.position[1]
    }
    fn z(&self) -> f32 {
        self.position[2]
    }
    /// What `parse_line_internal` does before the callback: a relative E word counts from 0.
    fn before(&mut self, line: &Line) {
        if line.has(Axis::E) {
            self.position[Axis::E as usize] = 0.;
        }
    }
    /// `update_coordinates` with the line as read (the callback's edits do not count).
    fn after(&mut self, line: &Line, orca: bool) {
        let command = line.command();
        let moves =
            matches!(command, "G0" | "G1" | "G92") || (orca && matches!(command, "G2" | "G3"));
        if moves {
            for a in AXES {
                if line.has(a) {
                    self.position[a as usize] = line.value(a);
                }
            }
        }
    }
    fn new_x(&self, line: &Line) -> f32 {
        if line.has(Axis::X) {
            line.value(Axis::X)
        } else {
            self.x()
        }
    }
    fn new_y(&self, line: &Line) -> f32 {
        if line.has(Axis::Y) {
            line.value(Axis::Y)
        } else {
            self.y()
        }
    }
    fn dist_e(&self, line: &Line) -> f32 {
        if line.has(Axis::E) {
            line.value(Axis::E) - self.position[Axis::E as usize]
        } else {
            0.
        }
    }
    fn dist_z(&self, line: &Line) -> f32 {
        if line.has(Axis::Z) {
            line.value(Axis::Z) - self.z()
        } else {
            0.
        }
    }
    fn dist_xy(&self, line: &Line) -> f32 {
        let x = if line.has(Axis::X) {
            line.value(Axis::X) - self.x()
        } else {
            0.
        };
        let y = if line.has(Axis::Y) {
            line.value(Axis::Y) - self.y()
        } else {
            0.
        };
        (x * x + y * y).sqrt()
    }
    fn is_move(line: &Line, orca: bool) -> bool {
        line.cmd_is("G1") || (orca && (line.cmd_is("G2") || line.cmd_is("G3")))
    }
    fn extruding(&self, line: &Line, orca: bool) -> bool {
        Self::is_move(line, orca) && self.dist_e(line) > 0.
    }
    fn retracting(&self, line: &Line, orca: bool) -> bool {
        Self::is_move(line, orca) && self.dist_e(line) < 0.
    }
}

/// `GCodeFormatter::quantize` of a float point (3 decimals, in double).
fn quantize(p: [f32; 2]) -> [f64; 2] {
    p.map(|v| (f64::from(v) * 1000.).round() / 1000.)
}

/// The previous spiral layer's points as segments, bucketed in a grid whose cells are as large as
/// the smoothing distance: every segment closer than that to a point lies in the point's cell or
/// one of its eight neighbours.
struct Distancer {
    points: Vec<[f32; 2]>,
    cell: f64,
    grid: std::collections::HashMap<(i64, i64), Vec<usize>>,
}

impl Distancer {
    fn new(points: &[[f32; 2]], cell: f32) -> Self {
        let cell = f64::from(cell).max(1e-3);
        let mut grid: std::collections::HashMap<(i64, i64), Vec<usize>> = Default::default();
        for i in 1..points.len() {
            let (a, b) = (points[i - 1], points[i]);
            let lo = |k: usize| f64::from(a[k].min(b[k]));
            let hi = |k: usize| f64::from(a[k].max(b[k]));
            let (x0, x1) = ((lo(0) / cell).floor() as i64, (hi(0) / cell).floor() as i64);
            let (y0, y1) = ((lo(1) / cell).floor() as i64, (hi(1) / cell).floor() as i64);
            // A degenerate long segment would fill many cells; it still is one entry per cell.
            for gx in x0..=x1 {
                for gy in y0..=y1 {
                    grid.entry((gx, gy)).or_default().push(i - 1);
                }
            }
        }
        Self {
            points: points.to_vec(),
            cell,
            grid,
        }
    }

    /// PrusaSlicer: `distance_from_lines_extra<false>` in double: the distance to the nearest
    /// segment within the smoothing distance and the nearest point on it.
    fn nearest_prusa(&self, p: [f32; 2]) -> Option<(f64, [f64; 2])> {
        let q = [f64::from(p[0]), f64::from(p[1])];
        let mut best: Option<(f64, usize, [f64; 2])> = None;
        for i in self.candidates(p) {
            let a = self.points[i].map(f64::from);
            let b = self.points[i + 1].map(f64::from);
            let v = [b[0] - a[0], b[1] - a[1]];
            let va = [q[0] - a[0], q[1] - a[1]];
            let l2 = v[0] * v[0] + v[1] * v[1];
            let foot = if l2 == 0. {
                a
            } else {
                let t = va[0] * v[0] + va[1] * v[1];
                if t <= 0. {
                    a
                } else if t >= l2 {
                    b
                } else {
                    [a[0] + v[0] * t / l2, a[1] + v[1] * t / l2]
                }
            };
            let d2 = (q[0] - foot[0]).powi(2) + (q[1] - foot[1]).powi(2);
            if best.is_none_or(|(bd, bi, _)| d2 < bd || (d2 == bd && i < bi)) {
                best = Some((d2, i, foot));
            }
        }
        best.map(|(d2, _, foot)| (d2.sqrt(), foot))
    }

    /// Orca: `nearest_point_on_lines` in float, the first segment wins a tie.
    fn nearest_orca(&self, p: [f32; 2]) -> Option<(f32, [f32; 2])> {
        let mut best: Option<(f32, usize, [f32; 2])> = None;
        for i in self.candidates(p) {
            let (a, b) = (self.points[i], self.points[i + 1]);
            let ab = [b[0] - a[0], b[1] - a[1]];
            let ca = [p[0] - a[0], p[1] - a[1]];
            let mut t = (ca[0] * ab[0] + ca[1] * ab[1]) / (ab[0] * ab[0] + ab[1] * ab[1]);
            t = if t > 1. { 1. } else { t };
            t = if t < 0. { 0. } else { t };
            let closest = [a[0] + ab[0] * t, a[1] + ab[1] * t];
            let d = ((p[0] - closest[0]).powi(2) + (p[1] - closest[1]).powi(2)).sqrt();
            if best.is_none_or(|(bd, bi, _)| d < bd || (d == bd && i < bi)) {
                best = Some((d, i, closest));
            }
        }
        best.map(|(d, _, c)| (d, c))
    }

    fn candidates(&self, p: [f32; 2]) -> Vec<usize> {
        let gx = (f64::from(p[0]) / self.cell).floor() as i64;
        let gy = (f64::from(p[1]) / self.cell).floor() as i64;
        let mut out = Vec::new();
        for dx in -1..=1 {
            for dy in -1..=1 {
                if let Some(list) = self.grid.get(&(gx + dx, gy + dy)) {
                    out.extend_from_slice(list);
                }
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }
}

/// `SpiralVase`: one per print, fed every layer's G-code in print order.
pub struct SpiralVase {
    orca: bool,
    reader: Reader,
    max_xy_smoothing: f32,
    smooth: bool,
    relative_e: bool,
    /// Orca `spiral_starting_flow_ratio` / `spiral_finishing_flow_ratio`.
    starting_flow: f32,
    finishing_flow: f32,
    /// Orca: extrusions shorter than this are dropped on spiral layers.
    min_segment_length: f32,
    enabled: bool,
    transition: bool,
    /// The previous spiral layer's points (Orca distinguishes "none yet" from "empty").
    previous: Option<Vec<[f32; 2]>>,
}

/// A FloatOrPercent of the nozzle diameter, as Orca's `get_abs_value` reads it.
fn float_or_percent(text: &str, base: f64) -> Option<f64> {
    let text = text.trim();
    match text.strip_suffix('%') {
        Some(p) => p.trim().parse::<f64>().ok().map(|p| p / 100. * base),
        None => text.parse::<f64>().ok(),
    }
}

impl SpiralVase {
    /// The post-processor for `c`, or `None` when the print is not a spiral vase.
    pub fn new(c: &Config) -> Option<Self> {
        if !mica_core::spiral_vase::enabled(c) {
            return None;
        }
        let orca = mica_core::rules::Rules::of(c).is_orca();
        let nozzles: Vec<f64> = (0..c.vector_len("nozzle_diameter").max(1))
            .filter_map(|i| c.float_at("nozzle_diameter", i))
            .collect();
        let text = |key: &str| c.raw(key);
        let number = |key: &str| text(key).and_then(|v| v.trim().parse::<f64>().ok());
        let (max_xy_smoothing, smooth) = if orca {
            // GCode::_do_export: of the current extruder's nozzle (the first one here).
            let nozzle = nozzles.first().copied().unwrap_or(0.4);
            (
                text("spiral_mode_max_xy_smoothing")
                    .and_then(|v| float_or_percent(&v, nozzle))
                    .unwrap_or(2. * nozzle) as f32,
                text("spiral_mode_smooth").is_some_and(|v| matches!(v.trim(), "1" | "true")),
            )
        } else {
            (
                (2. * nozzles.iter().copied().fold(0., f64::max)) as f32,
                true,
            )
        };
        let mut reader = Reader::default();
        reader.position[Axis::Z as usize] = c.float("z_offset").unwrap_or(0.) as f32;
        Some(Self {
            orca,
            reader,
            max_xy_smoothing,
            smooth,
            relative_e: c.bool("use_relative_e_distances").unwrap_or(true),
            starting_flow: number("spiral_starting_flow_ratio").unwrap_or(0.) as f32,
            finishing_flow: number("spiral_finishing_flow_ratio").unwrap_or(0.) as f32,
            min_segment_length: (1e-4f32).max(2. * c.float("resolution").unwrap_or(0.) as f32),
            enabled: false,
            transition: false,
            previous: None,
        })
    }

    /// `SpiralVase::enable`: whether the next layer is a spiral; the first one after a layer
    /// that was not is the transition layer.
    pub fn enable(&mut self, enable: bool) {
        self.transition = enable && !self.enabled;
        self.enabled = enable;
    }

    /// Whether the last [`Self::enable`] turned the spiral on.
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Feeds G-code that is not part of a layer (or precedes its layer change) to the reader.
    pub fn follow(&mut self, gcode: &str) {
        for raw in lines(gcode) {
            let line = Line::parse(raw);
            self.reader.before(&line);
            self.reader.after(&line, self.orca);
        }
    }

    /// `SpiralVase::process_layer`: one whole layer's G-code, starting at its layer change.
    pub fn process_layer(&mut self, gcode: &str, last_layer: bool) -> String {
        if !self.enabled {
            self.follow(gcode);
            return gcode.to_string();
        }
        let orca = self.orca;
        // Total XY length of the layer's extrusions, its height and the Z of its layer change.
        let mut total_layer_length = 0f32;
        let mut layer_height = 0f32;
        let mut z = 0f32;
        {
            let mut r = self.reader;
            let mut set_z = false;
            for raw in lines(gcode) {
                let line = Line::parse(raw);
                r.before(&line);
                if line.cmd_is("G1") {
                    if r.extruding(&line, orca) {
                        total_layer_length += r.dist_xy(&line);
                    } else if line.has(Axis::Z) {
                        layer_height += r.dist_z(&line);
                        if !set_z {
                            z = line.value(Axis::Z);
                            set_z = true;
                        }
                    }
                }
                r.after(&line, orca);
            }
        }
        // Remove the layer height from the initial Z.
        z -= layer_height;

        // Tapering and smoothing only with relative extrusion (upstream FIXME).
        let transition_in = self.transition && self.relative_e;
        let transition_out = last_layer && self.relative_e;
        let smooth = self.smooth && self.relative_e;
        let previous = self.previous.as_deref().unwrap_or(&[]);
        let distancer = Distancer::new(previous, self.max_xy_smoothing);
        let mut last_point = previous.last().copied().unwrap_or([0., 0.]);
        let mut current: Vec<[f32; 2]> = Vec::new();
        let mut len = 0f32;
        let mut new_gcode = String::with_capacity(gcode.len() + gcode.len() / 4);
        let mut transition_gcode = String::new();
        for raw in lines(gcode) {
            let mut line = Line::parse(raw);
            self.reader.before(&line);
            let r = self.reader;
            let emitted = self.rewrite(
                &mut line,
                &r,
                SpiralLayer {
                    z,
                    total_layer_length,
                    layer_height,
                    transition_in,
                    transition_out,
                    smooth,
                    previous_known: self.previous.is_some(),
                },
                &distancer,
                &mut len,
                &mut last_point,
                &mut current,
                &mut transition_gcode,
            );
            if let Some(text) = emitted {
                new_gcode.push_str(&text);
                new_gcode.push('\n');
            }
            self.reader.after(&Line::parse(raw), orca);
        }
        // Orca keeps the (possibly empty) list even without smoothing; PrusaSlicer's is empty
        // then too.
        self.previous = Some(current);
        new_gcode + &transition_gcode
    }

    /// The `parse_buffer` callback of `process_layer` for one line: the text to emit, if any.
    #[allow(clippy::too_many_arguments)]
    fn rewrite(
        &self,
        line: &mut Line,
        reader: &Reader,
        layer: SpiralLayer,
        distancer: &Distancer,
        len: &mut f32,
        last_point: &mut [f32; 2],
        current: &mut Vec<[f32; 2]>,
        transition_gcode: &mut String,
    ) -> Option<String> {
        let orca = self.orca;
        if line.cmd_is("G1") || (orca && Reader::is_move(line, orca)) {
            // Orca: no retractions and no tiny extrusions (deretractions included) in a spiral.
            if orca
                && (reader.retracting(line, orca)
                    || (reader.extruding(line, orca)
                        && reader.dist_xy(line) < self.min_segment_length))
            {
                return None;
            }
            let has_xy = line.has(Axis::X) || line.has(Axis::Y);
            let layer_change = if orca {
                line.has(Axis::Z) && !has_xy
            } else {
                line.has(Axis::Z)
            };
            if layer_change && line.cmd_is("G1") {
                // The layer's Z move goes to the previous layer's Z: the ramp starts there.
                line.set(Axis::Z, layer.z, 3);
                return Some(line.raw.clone());
            }
            if has_xy && line.cmd_is("G1") {
                let dist_xy = reader.dist_xy(line);
                if dist_xy > 0. && reader.extruding(line, orca) {
                    *len += dist_xy;
                    let factor = *len / layer.total_layer_length;
                    if layer.transition_in {
                        // The first spiral layer ramps its flow in.
                        let start = if orca { self.starting_flow } else { 0. };
                        line.set(
                            Axis::E,
                            line.value(Axis::E) * (start + factor * (1. - start)),
                            5,
                        );
                    } else if layer.transition_out {
                        // The last layer prints again at its full height while its flow ramps out.
                        let mut copy = line.clone();
                        let finish = if orca { self.finishing_flow } else { 0. };
                        copy.set(
                            Axis::E,
                            line.value(Axis::E) * (finish + (1. - factor) * (1. - finish)),
                            5,
                        );
                        transition_gcode.push_str(&copy.raw);
                        transition_gcode.push('\n');
                    }
                    // The core of the spiral: Z rises with the length printed.
                    line.set(Axis::Z, layer.z + factor * layer.layer_height, 3);
                    let mut emit = true;
                    if layer.smooth {
                        let p = [reader.new_x(line), reader.new_y(line)];
                        current.push(p);
                        if orca {
                            if layer.previous_known {
                                match distancer.nearest_orca(p) {
                                    Some((d, nearest)) if d < self.max_xy_smoothing => {
                                        let target = [
                                            nearest[0] * (1. - factor) + p[0] * factor,
                                            nearest[1] * (1. - factor) + p[1] * factor,
                                        ];
                                        let modified = ((last_point[0] - target[0]).powi(2)
                                            + (last_point[1] - target[1]).powi(2))
                                        .sqrt();
                                        if modified < self.min_segment_length {
                                            line.raw.clear();
                                        } else {
                                            line.set(Axis::X, target[0], 3);
                                            line.set(Axis::Y, target[1], 3);
                                            line.set(
                                                Axis::E,
                                                line.value(Axis::E) * modified / dist_xy,
                                                5,
                                            );
                                            *last_point = target;
                                        }
                                    }
                                    _ => *last_point = p,
                                }
                            }
                        } else {
                            match distancer.nearest_prusa(p) {
                                Some((d, nearest)) if d < f64::from(self.max_xy_smoothing) => {
                                    let nearest = nearest.map(|v| v as f32);
                                    let target = [
                                        nearest[0] * (1. - factor) + p[0] * factor,
                                        nearest[1] * (1. - factor) + p[1] * factor,
                                    ];
                                    // Only a line that moves once quantized is written.
                                    emit = quantize(*last_point) != quantize(target);
                                    line.set(Axis::X, target[0], 3);
                                    line.set(Axis::Y, target[1], 3);
                                    let modified = ((last_point[0] - target[0]).powi(2)
                                        + (last_point[1] - target[1]).powi(2))
                                    .sqrt();
                                    line.set(Axis::E, line.value(Axis::E) * modified / dist_xy, 5);
                                    *last_point = target;
                                }
                                _ => *last_point = p,
                            }
                        }
                    }
                    return emit.then(|| line.raw.clone());
                }
                // Travels (and wipes) are skipped: the spiral blends into its first point.
                return None;
            }
        }
        if layer.transition_out {
            transition_gcode.push_str(&line.raw);
            transition_gcode.push('\n');
        }
        Some(line.raw.clone())
    }
}

/// What `process_layer` measured of the layer before rewriting it.
#[derive(Clone, Copy)]
struct SpiralLayer {
    z: f32,
    total_layer_length: f32,
    layer_height: f32,
    transition_in: bool,
    transition_out: bool,
    smooth: bool,
    /// Orca: a previous spiral layer was processed (its list exists, even empty).
    previous_known: bool,
}

/// A spiral layer's wall loop (its paths, all walls, joined end to start) rotated to start at
/// the point nearest `from`, where the previous layer ended: PrusaSlicer's
/// `resolve_or_fit_split_with_seam` / Orca's `ExtrusionLoop::split_at` from the last position,
/// which carries over from layer to layer. The split merges with a vertex closer than 1.5 µm
/// (PrusaSlicer) or `SCALED_EPSILON` (Orca). Anything else is left alone. Returns, for a
/// rotated loop, the index of the original path each new path comes from.
pub fn start_loop_near(
    paths: &mut Vec<ExtrusionPath>,
    from: Point,
    orca: bool,
) -> Option<Vec<usize>> {
    let walls = |r: Role| {
        matches!(
            r,
            Role::ExternalPerimeter | Role::Perimeter | Role::OverhangPerimeter
        )
    };
    if paths.is_empty()
        || !paths.iter().all(|p| walls(p.role) && p.points.len() >= 2)
        || paths
            .windows(2)
            .any(|w| w[0].points.last() != w[1].points.first())
        || paths.last().unwrap().points.last() != paths[0].points.first()
    {
        return None;
    }
    // The nearest point over every segment of every path.
    let mut best: Option<(f64, usize, usize, [f64; 2])> = None;
    let p = [from.x as f64, from.y as f64];
    for (pi, path) in paths.iter().enumerate() {
        for (si, w) in path.points.windows(2).enumerate() {
            let (a, b) = (
                [w[0].x as f64, w[0].y as f64],
                [w[1].x as f64, w[1].y as f64],
            );
            let v = [b[0] - a[0], b[1] - a[1]];
            let l2 = v[0] * v[0] + v[1] * v[1];
            let t = if l2 > 0. {
                (((p[0] - a[0]) * v[0] + (p[1] - a[1]) * v[1]) / l2).clamp(0., 1.)
            } else {
                0.
            };
            let q = [a[0] + v[0] * t, a[1] + v[1] * t];
            let d = (q[0] - p[0]).powi(2) + (q[1] - p[1]).powi(2);
            if best.is_none_or(|(bd, ..)| d < bd) {
                best = Some((d, pi, si, q));
            }
        }
    }
    let (_, pi, si, q) = best?;
    let merge = if orca { 100. } else { 1_500. };
    let points = &paths[pi].points;
    let (a, b) = (points[si], points[si + 1]);
    let dist = |v: Point| ((v.x as f64 - q[0]).powi(2) + (v.y as f64 - q[1]).powi(2)).sqrt();
    let split = if dist(a) <= dist(b) && dist(a) < merge {
        a
    } else if dist(b) < merge {
        b
    } else {
        Point::new(q[0].round() as i64, q[1].round() as i64)
    };
    // Split path `pi` at `split` (after vertex `si`) into its head (from the split on) and tail.
    let mut head = paths[pi].clone();
    let mut tail = paths[pi].clone();
    let at = if split == a {
        si
    } else if split == b {
        si + 1
    } else {
        usize::MAX
    };
    if at == usize::MAX {
        head.points = std::iter::once(split)
            .chain(points[si + 1..].iter().copied())
            .collect();
        tail.points = points[..=si]
            .iter()
            .copied()
            .chain(std::iter::once(split))
            .collect();
    } else {
        head.points = points[at..].to_vec();
        tail.points = points[..=at].to_vec();
    }
    let mut out = Vec::with_capacity(paths.len() + 1);
    let mut sources = Vec::with_capacity(paths.len() + 1);
    if head.points.len() >= 2 {
        out.push(head);
        sources.push(pi);
    }
    out.extend(paths[pi + 1..].iter().cloned());
    sources.extend(pi + 1..paths.len());
    out.extend(paths[..pi].iter().cloned());
    sources.extend(0..pi);
    if tail.points.len() >= 2 {
        out.push(tail);
        sources.push(pi);
    }
    *paths = out;
    Some(sources)
}

/// Filament the extrusion moves (`G1`/`G2`/`G3` with a positive relative E) of `gcode` push.
pub fn extruded(gcode: &str) -> f64 {
    lines(gcode)
        .map(Line::parse)
        .filter(|l| {
            matches!(l.command(), "G1" | "G2" | "G3") && l.has(Axis::E) && l.value(Axis::E) > 0.
        })
        .map(|l| f64::from(l.value(Axis::E)))
        .sum()
}

/// The lines of a G-code buffer (`parse_buffer`): a final line without newline counts, an empty
/// buffer has none.
fn lines(gcode: &str) -> impl Iterator<Item = &str> {
    let body = gcode.strip_suffix('\n').unwrap_or(gcode);
    body.split('\n')
        .filter(move |_| !gcode.is_empty())
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(orca: bool) -> Config {
        let mut c = Config::defaults();
        c.set_raw("spiral_vase", "1").unwrap();
        c.set_raw("nozzle_diameter", "0.4").unwrap();
        c.set_raw("use_relative_e_distances", "1").unwrap();
        if orca {
            mica_core::rules::Rules::OrcaSlicer.set(&mut c);
            c.set_raw("resolution", "0.012").unwrap();
        }
        c
    }

    /// A square loop of side 10 at the layer's Z, extruding 1 mm of filament per side.
    fn square_layer(z: f64) -> String {
        format!(
            ";LAYER_CHANGE\n;Z:{z}\nG1 Z{z} F720\nG1 X10 Y0\nG1 F1200\nG1 X20 Y0 E1\nG1 X20 Y10 E1\nG1 X10 Y10 E1\nG1 X10 Y0 E1\n"
        )
    }

    fn values(text: &str, axis: char) -> Vec<f64> {
        text.lines()
            .filter(|l| l.starts_with("G1") && l.contains(" E"))
            .filter_map(|l| {
                l.split_whitespace()
                    .find_map(|w| w.strip_prefix(axis))
                    .and_then(|v| v.parse().ok())
            })
            .collect()
    }

    #[test]
    fn z_ramps_with_the_length_printed_from_the_previous_layer() {
        let mut spiral = SpiralVase::new(&config(false)).unwrap();
        spiral.enable(false);
        let bottom = spiral.process_layer(&square_layer(0.2), false);
        assert_eq!(bottom, square_layer(0.2), "a bottom layer passes through");
        spiral.enable(true);
        let out = spiral.process_layer(&square_layer(0.4), false);
        // The layer change goes to the previous Z, the travel to the start is dropped.
        assert!(out.contains("G1 Z0.200 F720\n"), "{out}");
        assert!(!out.contains("G1 X10 Y0\n"), "{out}");
        assert_eq!(values(&out, 'Z'), [0.25, 0.3, 0.35, 0.4]);
        // First spiral layer: the flow ramps in with the same factor.
        assert_eq!(values(&out, 'E'), [0.25, 0.5, 0.75, 1.]);
        // A full spiral layer on the same path (no smoothing needed) keeps its flow.
        spiral.enable(true);
        let out = spiral.process_layer(&square_layer(0.6), false);
        assert_eq!(values(&out, 'Z'), [0.45, 0.5, 0.55, 0.6]);
        assert_eq!(values(&out, 'E'), [1., 1., 1., 1.]);
    }

    #[test]
    fn last_layer_prints_again_at_its_height_with_the_flow_ramping_out() {
        let mut spiral = SpiralVase::new(&config(false)).unwrap();
        spiral.enable(true);
        spiral.process_layer(&square_layer(0.2), false);
        spiral.enable(true);
        let out = spiral.process_layer(&square_layer(0.4), true);
        let (ramp, finish) = out.split_at(out.rfind(";LAYER_CHANGE").unwrap());
        assert_eq!(values(ramp, 'Z'), [0.25, 0.3, 0.35, 0.4]);
        // The finishing loop has no Z (it stays at the top) and its flow falls to zero.
        assert!(values(finish, 'Z').is_empty());
        assert_eq!(values(finish, 'E'), [0.75, 0.5, 0.25, 0.]);
    }

    #[test]
    fn xy_smoothing_blends_towards_the_previous_layer() {
        let mut spiral = SpiralVase::new(&config(false)).unwrap();
        spiral.enable(true);
        spiral.process_layer(&square_layer(0.2), false);
        spiral.enable(true);
        // The next layer is 0.2 mm wider on the right: its points start on the previous wall
        // and reach their own position at the end of the layer.
        let layer = ";LAYER_CHANGE\nG1 Z0.4\nG1 X20.2 Y0 E1\nG1 X20.2 Y10 E1\nG1 X10 Y10 E1\nG1 X10 Y0 E1\n";
        let out = spiral.process_layer(layer, false);
        let x = values(&out, 'X');
        // Each corner is as far out as the share of the layer printed when it is reached.
        let total = 10.2 + 10. + 10.2 + 10.;
        assert!((x[0] - (20. + 0.2 * 10.2 / total)).abs() < 6e-4, "{out}");
        assert!((x[1] - 20.1).abs() < 6e-4, "{out}");
        assert_eq!(values(&out, 'Y')[1], 10.);
    }

    #[test]
    fn orca_drops_retractions_and_ramps_from_its_starting_flow() {
        let mut c = config(true);
        c.set_raw("spiral_starting_flow_ratio", "0.5").unwrap();
        let mut spiral = SpiralVase::new(&c).unwrap();
        spiral.enable(true);
        let layer = format!("{}G1 E-.8 F2100\nG1 E.8\n", square_layer(0.2));
        let out = spiral.process_layer(&layer, false);
        assert!(!out.contains("E-.8") && !out.contains("E.8\n"), "{out}");
        assert_eq!(values(&out, 'E'), [0.625, 0.75, 0.875, 1.]);
        // Orca writes the layer change only when it has no XY.
        assert!(out.contains("G1 Z0.000 F720"), "{out}");
    }
}
