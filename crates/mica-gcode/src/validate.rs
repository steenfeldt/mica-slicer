// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Bounded-memory validation of finished G-code. This is a static simulator, not firmware:
//! temperatures are commanded targets and waits, and custom macro motion cannot be inferred.
//! Errors are definite violations; warnings identify uncertain flow/firmware assumptions.
//! Binary line numbers refer to the concatenated decoded G-code, not physical blocks.
mod image;
mod motion;
#[cfg(test)]
mod tests;

use crate::bgcode::{self, stream::Block};
use mica_core::config::Config;
use mica_core::settings::PrintSettings;
use std::collections::BTreeMap;
use std::io::{BufRead, Read};
use std::path::Path;

/// The highest hotend target (°C) the validator accepts (no firmware takes more).
pub const MAX_HOTEND_TEMPERATURE: f64 = 500.;
/// The highest bed target (°C) the validator accepts. The option tables allow up to 300 °C,
/// but no supported bed heats beyond 150 °C.
pub const MAX_BED_TEMPERATURE: f64 = 150.;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}
#[derive(Clone, Debug)]
pub struct Diagnostic {
    pub severity: Severity,
    pub line: usize,
    pub rule: &'static str,
    pub message: String,
}
#[derive(Clone, Debug, Default)]
pub struct Report {
    pub lines: usize,
    pub layers: usize,
    pub errors: usize,
    pub warnings: usize,
    /// First 20 occurrences of each rule are retained; counts include suppressed occurrences.
    pub diagnostics: Vec<Diagnostic>,
    pub filament_mm: Vec<f64>,
    pub role_filament_mm: BTreeMap<String, f64>,
    /// `role_filament_mm` per tool (which material prints the supports, the tower, ...).
    pub tool_role_filament_mm: Vec<BTreeMap<String, f64>>,
    pub toolchanges: usize,
}

#[derive(Clone, Debug)]
pub struct ToolProfile {
    pub diameter: f64,
    pub flow_ratio: f64,
    pub print_temperature: f64,
    pub first_temperature: f64,
    /// Lowest/highest commanded temperature accepted while extruding: the filament's
    /// recommended range, widened to the profile's own print temperatures.
    pub min_temperature: f64,
    pub max_temperature: f64,
    /// The filament's recommended nozzle range (`nozzle_temperature_range_low/high`). A
    /// profile that prints outside it gets a warning, as OrcaSlicer's filament tab shows one.
    pub recommended_temperature: (f64, f64),
    pub retract_limit: f64,
    /// First-layer bed temperature of this tool's filament (later layers' when the first
    /// layer has none). Multi-tool start G-code heats the bed for the tool that prints first,
    /// so that tool's value is what printing must have waited for.
    pub bed_temperature: f64,
}
#[derive(Clone, Debug)]
pub struct Profile {
    pub bed: Vec<(f64, f64)>,
    pub exclusions: Vec<Vec<(f64, f64)>>,
    pub max_z: f64,
    pub min_layer: f64,
    pub max_layer: f64,
    pub width: f64,
    pub height: f64,
    /// Bed temperature printing waits for when no tool's own value applies (see
    /// `ToolProfile::bed_temperature`).
    pub bed_temperature: f64,
    pub tools: Vec<ToolProfile>,
    pub klipper: bool,
    pub xl: bool,
    pub progress_required: bool,
    /// Optional known tower XY bounds (including brim). Omit when the footprint is unknown.
    pub tower: Option<[(f64, f64); 2]>,
    /// Spiral vase: Z rises within a layer, the first spiral layer ramps its flow in and the
    /// last layer prints once more at its own height while its flow ramps out
    /// (`mica_gcode::spiral_vase`). Also turned on by the file's own configuration.
    pub spiral_vase: bool,
}
impl Profile {
    pub fn from_config(c: &Config) -> Result<Self, String> {
        let s = PrintSettings::from_config(c).map_err(|e| e.to_string())?;
        let bed = c.points("bed_shape").ok_or("printer has no bed_shape")?;
        if bed.len() < 3 || bed.iter().any(|p| !p.0.is_finite() || !p.1.is_finite()) {
            return Err("invalid printer bed_shape".into());
        }
        let tools = (0..c.vector_len("nozzle_diameter").max(1))
            .map(|i| {
                let value = |key: &str, fallback| c.float_at(key, i).unwrap_or(fallback);
                let temperature = value("temperature", 210.);
                let first = value("first_layer_temperature", temperature);
                let low = value("nozzle_temperature_range_low", temperature.min(first) - 30.);
                let high = value(
                    "nozzle_temperature_range_high",
                    temperature.max(first) + 30.,
                );
                ToolProfile {
                    diameter: value("filament_diameter", 1.75),
                    flow_ratio: value("extrusion_multiplier", 1.),
                    print_temperature: temperature,
                    first_temperature: first,
                    min_temperature: low.min(temperature).min(first),
                    max_temperature: high.max(temperature).max(first),
                    recommended_temperature: (low, high),
                    // The filament's own retraction lengths override the printer's (the
                    // XL's filaments park their tool 20 mm retracted).
                    retract_limit: value("retract_length", 1.)
                        .max(value("retract_length_toolchange", 10.))
                        .max(value("filament_retract_length", 0.))
                        .max(value("filament_retract_length_toolchange", 0.))
                        .max(10.)
                        * 2.,
                    bed_temperature: Some(value("first_layer_bed_temperature", 0.))
                        .filter(|t| *t > 0.)
                        .unwrap_or_else(|| value("bed_temperature", 0.)),
                }
            })
            .collect::<Vec<_>>();
        if tools.iter().any(|t| {
            !t.diameter.is_finite()
                || t.diameter <= 0.
                || !t.flow_ratio.is_finite()
                || t.flow_ratio <= 0.
        }) {
            return Err("invalid filament diameter or flow ratio".into());
        }
        let exclusions = c
            .points("bed_exclude_area")
            .filter(|p| p.len() >= 3)
            .into_iter()
            .collect();
        Ok(Self {
            bed,
            exclusions,
            max_z: c.float("max_print_height").unwrap_or(250.),
            min_layer: s.min_layer_height,
            max_layer: s.max_layer_height.max(s.layer_height),
            width: s.perimeter_extrusion_width,
            height: s.layer_height,
            bed_temperature: c
                .float("first_layer_bed_temperature")
                .unwrap_or(0.)
                .max(c.float("bed_temperature").unwrap_or(0.)),
            tools,
            klipper: c.string("gcode_flavor") == Some("klipper"),
            xl: c
                .string("printer_notes")
                .is_some_and(|v| v.contains("PRINTER_MODEL_XL")),
            progress_required: !c.bool("disable_m73").unwrap_or(false),
            spiral_vase: mica_core::spiral_vase::enabled(c),
            tower: None,
        })
    }
}

#[derive(Clone, Default)]
struct Tool {
    target: f64,
    waited: f64,
    preheated: bool,
    retracted: f64,
    used: bool,
}
struct TextThumbnail {
    format: String,
    expected: usize,
    dimensions: (usize, usize),
    data: Vec<u8>,
}
struct Simulator<'a> {
    p: &'a Profile,
    report: Report,
    counts: BTreeMap<&'static str, usize>,
    tools: Vec<Tool>,
    tool: usize,
    initial_tool: Option<usize>,
    /// The tool that extrudes the first slicer feature: the one the start G-code heated the
    /// bed for (`initial_tool` may be a tool the start G-code purges first).
    first_layer_tool: Option<usize>,
    xyz: [f64; 3],
    known: [bool; 3],
    absolute: bool,
    relative_e: bool,
    e: f64,
    unit: f64,
    bed_target: f64,
    bed_waited: f64,
    layer_z: Option<f64>,
    floor_z: f64,
    printed: bool,
    /// Sequential printing: highest Z extruded so far, and the head's state since the last
    /// extrusion (highest Z reached; whether it then moved in XY below that printed height).
    printed_z: f64,
    since_extrusion_z: f64,
    dipped: bool,
    /// XY travel since the last extrusion below the printed height, before clearing it.
    low_travel: f64,
    pending_layer: bool,
    height: f64,
    width: f64,
    role: String,
    custom: bool,
    wipe: bool,
    tower: bool,
    tower_z: f64,
    tower_bounds: Option<[(f64, f64); 2]>,
    tower_markers: Vec<String>,
    expected_tools: BTreeMap<usize, f64>,
    object: Option<String>,
    progress: [Option<f64>; 2],
    metadata: bool,
    header: bool,
    executable: bool,
    preextrude_pending: bool,
    /// After a filament change (M600) the profile may prime the new filament in place.
    filament_change_prime: bool,
    totals: Option<Vec<f64>>,
    thumbnail: Option<TextThumbnail>,
    /// Spiral vase checks (`Profile::spiral_vase` or the file's configuration).
    vase: bool,
    /// The current and the previous layer extruded below their layer marker (a spiral).
    spiral_layer: bool,
    previous_spiral_layer: bool,
    /// The finishing loop of a spiral vase: the last layer once more at the same Z.
    vase_finish: bool,
}
impl<'a> Simulator<'a> {
    fn new(p: &'a Profile) -> Self {
        Self {
            p,
            report: Report {
                filament_mm: vec![0.; p.tools.len().max(1)],
                tool_role_filament_mm: vec![BTreeMap::new(); p.tools.len().max(1)],
                ..Report::default()
            },
            counts: BTreeMap::new(),
            tools: vec![Tool::default(); p.tools.len().max(1)],
            tool: 0,
            initial_tool: None,
            first_layer_tool: None,
            xyz: [0.; 3],
            known: [false; 3],
            absolute: true,
            relative_e: false,
            e: 0.,
            unit: 1.,
            bed_target: 0.,
            bed_waited: 0.,
            layer_z: None,
            floor_z: 0.,
            printed: false,
            printed_z: 0.,
            since_extrusion_z: 0.,
            dipped: false,
            low_travel: 0.,
            pending_layer: false,
            height: p.height,
            width: p.width,
            role: "Unknown".into(),
            custom: true,
            wipe: false,
            tower: false,
            tower_z: 0.,
            tower_bounds: None,
            tower_markers: Vec::new(),
            expected_tools: BTreeMap::new(),
            object: None,
            progress: [None; 2],
            metadata: false,
            header: false,
            executable: false,
            preextrude_pending: false,
            filament_change_prime: false,
            totals: None,
            thumbnail: None,
            vase: p.spiral_vase,
            spiral_layer: false,
            previous_spiral_layer: false,
            vase_finish: false,
        }
    }
    fn issue(&mut self, severity: Severity, rule: &'static str, message: impl Into<String>) {
        match severity {
            Severity::Error => self.report.errors += 1,
            Severity::Warning => self.report.warnings += 1,
        }
        let count = self.counts.entry(rule).or_default();
        *count += 1;
        if *count <= 20 {
            self.report.diagnostics.push(Diagnostic {
                severity,
                line: self.report.lines,
                rule,
                message: message.into(),
            });
        }
    }
    fn error(&mut self, rule: &'static str, message: impl Into<String>) {
        self.issue(Severity::Error, rule, message);
    }
    fn warn(&mut self, rule: &'static str, message: impl Into<String>) {
        self.issue(Severity::Warning, rule, message);
    }
    /// A Klipper start macro's temperature parameters count as set and waited for (one value
    /// per tool, comma-separated for IDEX printers); what the macro does is not visible here.
    fn start_macro(&mut self, code: &str) {
        let first = |keys: &[&str]| keys.iter().find_map(|k| motion::named(code, k));
        let mut assumed = false;
        if let Some(list) = first(&["EXTRUDER_TEMP", "HOTEND_TEMP", "HOTEND", "EXTRUDER"]) {
            for (i, v) in list.split(',').enumerate() {
                if let (Ok(temp), Some(t)) = (v.trim().parse::<f64>(), self.tools.get_mut(i)) {
                    t.target = temp;
                    t.preheated = temp > 0.;
                    t.waited = temp;
                    assumed = true;
                }
            }
        }
        if let Some(temp) = first(&["BED_TEMP", "BED"]).and_then(|v| v.trim().parse::<f64>().ok()) {
            self.bed_target = temp;
            self.bed_waited = temp;
            assumed = true;
        }
        if assumed {
            self.warn(
                "start-macro",
                "temperatures assumed reached by the printer's start macro",
            );
        }
    }
    fn comment(&mut self, text: &str) {
        let text = text.trim();
        if text.starts_with("generated by") {
            self.metadata = true;
        }
        if text == "HEADER_BLOCK_START" {
            self.header = true;
        }
        if text == "EXECUTABLE_BLOCK_START" {
            self.executable = true;
        }
        // The file's own configuration (a binary file's metadata comes before its G-code).
        if matches!(text, "spiral_vase = 1" | "spiral_mode = 1") {
            self.vase = true;
        }
        if text == "LAYER_CHANGE" || text == "CHANGE_LAYER" {
            if self.pending_layer {
                self.error("layer-marker", "layer change has no Z marker");
            }
            self.previous_spiral_layer = self.spiral_layer;
            self.spiral_layer = false;
            self.vase_finish = false;
            self.pending_layer = true;
            self.report.layers += 1;
            self.custom = false;
        }
        if let Some(v) = text
            .strip_prefix("Z:")
            .or_else(|| text.strip_prefix("Z_HEIGHT:"))
        {
            if let Ok(z) = v.trim().parse::<f64>() {
                if !z.is_finite() || z <= 0. || z > self.p.max_z + 0.01 {
                    self.error("layer-z", format!("invalid layer Z {z}"));
                }
                if self.vase && self.layer_z.is_some_and(|old| (z - old).abs() < 0.0001) {
                    // Spiral vase: the finishing loop repeats the last layer's marker.
                    self.vase_finish = true;
                } else if self.layer_z.is_some_and(|old| z < old + 0.0001) {
                    // A sequentially printed object starts again from the bed. That is only
                    // safe once the head rose above everything printed and stayed there.
                    // A wipe may stay low; a travel to the next object may not.
                    if self.printed_z <= 0.
                        || self.since_extrusion_z <= self.printed_z + 0.01
                        || self.dipped
                        || self.low_travel > 10.
                    {
                        self.error(
                            "layer-order",
                            "layer Z did not rise, and the head did not first clear the printed objects",
                        );
                    }
                    self.floor_z = z;
                }
                if !self.pending_layer {
                    self.warn("layer-marker", "Z marker without a layer change");
                }
                self.layer_z = Some(z);
                self.pending_layer = false;
            } else {
                self.error("layer-z", "invalid Z marker");
            }
        }
        for (prefix, width) in [
            ("WIDTH:", true),
            ("LINE_WIDTH:", true),
            ("HEIGHT:", false),
            ("LAYER_HEIGHT:", false),
        ] {
            if let Some(v) = text.strip_prefix(prefix) {
                match v.trim().parse::<f64>() {
                    Ok(v) if v.is_finite() && v > 0. => {
                        if width {
                            self.width = v;
                        } else {
                            self.height = v;
                            // A bridge's height is its flow's, not a layer height: a thick
                            // bridge (OrcaSlicer's `thick_bridges` / `thick_internal_bridges`,
                            // `Flow::bridging_flow(dmr, dmr)`) and tree support's transition
                            // write the round extrusion's diameter, e.g. 0.4 for a 0.4 nozzle.
                            let role = self.role.to_ascii_lowercase();
                            let bridge_flow =
                                role.contains("bridge") || role.contains("support transition");
                            // Brick layers: an inner wall's bead may span up to two layers
                            // (a raised wall starting, or rejoining the layer grid).
                            let max = if matches!(self.role.as_str(), "Perimeter" | "Inner wall") {
                                2. * self.p.max_layer
                            } else {
                                self.p.max_layer
                            };
                            if !bridge_flow && (v < self.p.min_layer - 0.002 || v > max + 0.002) {
                                self.warn("layer-height", format!("path height {v:.4} outside profile range {:.3}–{:.3}; supports/bridges may override it", self.p.min_layer, self.p.max_layer));
                            }
                        }
                    }
                    _ => self.error("flow-metadata", "nonpositive or invalid width/height"),
                }
            }
        }
        if let Some(role) = text
            .strip_prefix("TYPE:")
            .or_else(|| text.strip_prefix("FEATURE:"))
        {
            if role.len() > 100
                || (self.report.role_filament_mm.len() >= 64
                    && !self.report.role_filament_mm.contains_key(role.trim()))
            {
                self.error("metadata-limit", "too many or oversized role names");
                self.role = "Unknown".into();
            } else {
                self.role = role.trim().to_string();
            }
            self.custom = self.role.eq_ignore_ascii_case("custom");
            self.tower = self.role.to_lowercase().contains("tower");
        }
        for name in [
            "CP TOOLCHANGE",
            "CP WIPE TOWER",
            "WIPE_TOWER",
            "WIPE_TOWER_BRIM",
        ] {
            let separator = if name.starts_with("CP ") { " " } else { "_" };
            if text == format!("{name}{separator}START") {
                if self.tower_markers.len() >= 16 {
                    self.error("tower-sequence", "tower marker nesting exceeds limit");
                } else {
                    self.tower_markers.push(name.into());
                }
            }
            if text == format!("{name}{separator}END")
                && self.tower_markers.pop().as_deref() != Some(name)
            {
                self.error("tower-sequence", "tower end has no matching entry marker");
            }
        }
        if let Some(rest) = text.strip_prefix("filament used T")
            && let Some((tool, value)) = rest.split_once(" [mm] = ")
            && let (Ok(tool), Ok(value)) = (tool.parse::<usize>(), value.parse::<f64>())
        {
            self.expected_tools.insert(tool, value);
        }
        if text == "WIPE_START" {
            self.wipe = true;
        }
        if text == "WIPE_END" {
            self.wipe = false;
        }
        if (text.contains("filament used [mm]") || text.starts_with("total filament length [mm]"))
            && let Some((_, values)) = text.split_once('=').or_else(|| text.split_once(':'))
        {
            let totals: Result<Vec<f64>, _> =
                values.split([',', ';']).map(|v| v.trim().parse()).collect();
            if let Ok(totals) = totals {
                self.totals = Some(totals);
            }
        }
        if text.starts_with("thumbnail") {
            let fields: Vec<_> = text.split_whitespace().collect();
            if fields.get(1) == Some(&"begin") {
                if self.thumbnail.is_some() {
                    self.error("thumbnail", "nested thumbnail");
                }
                let expected = fields.get(3).and_then(|v| v.parse().ok()).unwrap_or(0);
                let dimensions = fields
                    .get(2)
                    .and_then(|s| s.split_once('x'))
                    .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))
                    .unwrap_or((0, 0));
                self.thumbnail = Some(TextThumbnail {
                    format: fields[0].to_string(),
                    expected,
                    dimensions,
                    data: Vec::new(),
                });
                return;
            }
            if fields.get(1) == Some(&"end") {
                if let Some(TextThumbnail {
                    format,
                    expected,
                    dimensions,
                    data: encoded,
                }) = self.thumbnail.take()
                {
                    if expected != encoded.len() {
                        self.error("thumbnail", "base64 length disagrees with thumbnail header");
                    }
                    match image::base64(&encoded).and_then(|bytes| {
                        image::check(&bytes, &format)?;
                        image::dimensions(&bytes, dimensions)
                    }) {
                        Ok(()) => {}
                        Err(e) => self.error("thumbnail", e),
                    }
                } else {
                    self.error("thumbnail", "thumbnail end without begin");
                }
                return;
            }
        }
        if let Some(TextThumbnail { data, .. }) = &mut self.thumbnail {
            if data.len() + text.len() > 16 * 1024 * 1024 {
                self.thumbnail = None;
                self.error("thumbnail", "thumbnail exceeds 16 MiB limit");
            } else {
                data.extend_from_slice(text.as_bytes());
            }
        }
    }
    fn line(&mut self, text: &str) {
        self.report.lines += 1;
        let (code, comment) = text.split_once(';').unwrap_or((text, ""));
        if !comment.is_empty() {
            self.comment(comment);
        }
        let code = code.trim();
        if code.is_empty() {
            return;
        }
        let (command, args) = motion::command(code);
        if matches!(
            command.as_str(),
            "G0" | "G1"
                | "G2"
                | "G3"
                | "G92"
                | "M104"
                | "M104.1"
                | "M109"
                | "M140"
                | "M190"
                | "M106"
                | "M73"
        ) && args
            .iter()
            // Prusa firmware's `M106 … R` (restore the fan) is a flag without a number: the
            // CORE One L start G-code turns the bed fans on with `M106 P5 R A125 B10` (GUI
            // monkey test, seed 311).
            .any(|(&k, v)| !v.is_finite() && !(command == "M106" && k == 'R'))
        {
            self.error("number", "nonfinite command parameter");
            return;
        }
        let get = |key| args.get(&key).copied();
        match command.as_str() {
            "G0" | "G00" | "G1" | "G01" | "G2" | "G02" | "G3" | "G03" => {
                self.movement(&command, &args)
            }
            "G90" => {
                self.absolute = true;
            }
            "G91" => {
                self.absolute = false;
            }
            "M82" => self.relative_e = false,
            "M83" => self.relative_e = true,
            "G20" => self.unit = 25.4,
            "G21" => self.unit = 1.,
            "G92" => {
                if let Some(e) = get('E') {
                    self.e = e * self.unit;
                }
                for (i, axis) in ['X', 'Y', 'Z'].iter().enumerate() {
                    if let Some(v) = get(*axis) {
                        self.xyz[i] = v * self.unit;
                        self.known[i] = true;
                    }
                }
            }
            "G28" => {
                self.known = [false; 3];
            }
            "M104" | "M104.1" | "M109" => {
                let tool = get('T').unwrap_or(self.tool as f64);
                if tool < 0. || tool.fract() != 0. || tool >= self.tools.len() as f64 {
                    self.error(
                        "tool",
                        format!("heater tool {tool} outside printer profile"),
                    );
                    return;
                }
                if let Some(temp) = get('S').or_else(|| get('R')) {
                    if !(0. ..=MAX_HOTEND_TEMPERATURE).contains(&temp) {
                        self.error("temperature", format!("invalid hotend target {temp}"));
                    }
                    let printing_temperature = self
                        .p
                        .tools
                        .get(tool as usize)
                        .map_or(f64::INFINITY, |p| p.min_temperature);
                    let t = &mut self.tools[tool as usize];
                    t.target = temp;
                    t.preheated = temp > 0.;
                    if command == "M109" {
                        t.waited = temp;
                    } else if command == "M104.1" && t.waited >= printing_temperature {
                        // The XL's timed preheat for the tool's next pick. PrusaSlicer 2.9.6 places
                        // it by time only, so it can land in the tool's own active segment, even
                        // before its first extrusion, and with `temperature` on the first layer
                        // (`m_layer_id` is the final layer count). The firmware applies it like
                        // M104 and keeps printing while it heats: no wait is owed by a tool that
                        // already waited at a printing temperature (GUI monkey test, seed 703).
                        t.waited = t.waited.max(temp);
                    }
                }
            }
            "M140" | "M190" => {
                if let Some(temp) = get('S').or_else(|| get('R')) {
                    if !(0. ..=MAX_BED_TEMPERATURE).contains(&temp) {
                        self.error("bed-temperature", format!("invalid bed target {temp}"));
                    }
                    self.bed_target = temp;
                    if command == "M190" {
                        self.bed_waited = temp;
                    }
                }
            }
            "M106" => {
                if get('S').is_some_and(|s| !(0. ..=255.).contains(&s)) {
                    self.error("fan", "fan PWM outside 0–255 (0–100%)");
                }
            }
            "M73" => {
                for (i, key) in ['P', 'Q'].into_iter().enumerate() {
                    if let Some(v) = get(key) {
                        if !(0. ..=100.).contains(&v) || self.progress[i].is_some_and(|old| v < old)
                        {
                            self.error("progress", "M73 progress decreases or is outside 0–100");
                        }
                        self.progress[i] = Some(v);
                    }
                }
            }
            "M486" => {
                if code
                    .strip_prefix("M486")
                    .is_some_and(|s| s.trim_start().starts_with('A'))
                {
                    return;
                }
                if let Some(id) = get('S') {
                    if id < 0. {
                        self.end_object(None);
                    } else {
                        self.start_object(id.to_string());
                    }
                }
            }
            "EXCLUDE_OBJECT_START" => {
                self.start_object(motion::named(code, "NAME").unwrap_or("?").to_string())
            }
            "EXCLUDE_OBJECT_END" => self.end_object(motion::named(code, "NAME")),
            // Klipper start macros heat and wait (OrcaSlicer writes no bed commands of its own
            // for Klipper): the profile passes the temperatures (`START_PRINT EXTRUDER_TEMP=220
            // BED_TEMP=60`, `PRINT_START BED=60 HOTEND=220`; the U1 0.2/0.6/0.8 nozzle profiles'
            // generic toolchanger start G-code, GUI monkey test seed 1010; RatRig, Voron,
            // Creality K…). On a Marlin printer such a line is not a heating command.
            "PRINT_START" | "START_PRINT" if self.p.klipper => {
                let bed = ["BED_TEMP", "BED"]
                    .iter()
                    .find_map(|k| motion::named(code, k))
                    .and_then(|v| v.parse::<f64>().ok());
                if let Some(temp) = bed
                    && !(0. ..=MAX_BED_TEMPERATURE).contains(&temp)
                {
                    self.error("bed-temperature", format!("invalid bed target {temp}"));
                }
                self.start_macro(code);
            }
            "SM_PRINT_PREEXTRUDE_FILAMENT" => self.preextrude_pending = false,
            "G10" if args.is_empty() => {
                self.tools[self.tool].retracted = 1.;
            }
            // A filament change (M600) reloads and primes the nozzle: nothing is left retracted,
            // as PrusaSlicer's writer assumes after a colour change.
            "M600" => {
                self.tools[self.tool].retracted = 0.;
                self.filament_change_prime = true;
            }
            "G11" => {
                self.tools[self.tool].retracted = 0.;
            }
            _ if command.starts_with('T') && command[1..].parse::<usize>().is_ok() => {
                let tool = command[1..].parse::<usize>().unwrap_or(usize::MAX);
                if tool >= self.tools.len() {
                    self.error(
                        "tool",
                        format!("T{tool} outside printer tool count {}", self.tools.len()),
                    );
                    return;
                }
                if tool != self.tool {
                    self.report.toolchanges += 1;
                    if self.layer_z.is_some() {
                        if !self.tools[tool].preheated {
                            self.warn(
                                "tool-preheat",
                                format!("T{tool} has no preceding M104/M104.1/M109 preheat"),
                            );
                        }
                        if self.p.xl && get('S') != Some(1.) {
                            self.warn("tool-sequence", "XL selection lacks profile S1 parameter");
                        }
                        if self.p.klipper {
                            self.preextrude_pending = self.initial_tool != Some(tool);
                        }
                    }
                }
                self.tool = tool;
            }
            _ => {}
        }
    }
    fn start_object(&mut self, name: String) {
        if self.object.is_some() {
            self.error("object-label", "nested object start");
        }
        self.object = Some(name);
    }
    fn end_object(&mut self, name: Option<&str>) {
        match self.object.take() {
            Some(old) if name.is_some_and(|n| n != old) => {
                self.error("object-label", "object end name does not match start")
            }
            None => self.warn(
                "object-label",
                "object end without active object (startup reset is permitted)",
            ),
            _ => {}
        }
    }
    fn compare_total(&mut self, name: &str, actual: f64, expected: f64) {
        if !expected.is_finite() || expected < 0. {
            self.error("filament-total", "invalid filament total");
        } else if (actual - expected).abs() > 2.0_f64.max(expected * 0.02) {
            self.warn("filament-total",format!("{name}: simulated {actual:.2} mm, header {expected:.2} mm (macros/purge accounting can differ)"));
        }
    }
    fn finish(mut self) -> Report {
        if !self.tower_markers.is_empty() {
            self.error("tower-sequence", "unclosed wipe/prime tower markers");
        }
        if self.object.is_some() {
            self.error("object-label", "object label is not closed at end of file");
        }
        if self.pending_layer {
            self.error("layer-marker", "last layer has no Z marker");
        }
        if self.thumbnail.is_some() {
            self.error("thumbnail", "unterminated thumbnail");
        }
        if !self.metadata {
            self.warn("metadata", "no generator metadata");
        }
        for i in 0..self.p.tools.len() {
            let t = &self.p.tools[i];
            let (low, high) = t.recommended_temperature;
            if self.tools.get(i).is_some_and(|t| t.used)
                && [t.print_temperature, t.first_temperature]
                    .iter()
                    .any(|v| *v < low || *v > high)
            {
                let message = format!(
                    "T{i} filament profile prints at {:.0}/{:.0}°C, outside its recommended \
                     {low:.0}–{high:.0}°C",
                    t.first_temperature, t.print_temperature
                );
                self.warn("filament-range", message);
            }
        }
        if self.report.layers == 0 {
            self.error(
                "layers",
                "no layer markers found; file is empty, truncated or unsupported",
            );
        }
        if self.p.progress_required && self.progress[0] != Some(100.) {
            self.warn("progress-final", "M73 P does not reach 100");
        }
        if self.p.klipper && (!self.header || !self.executable) {
            self.warn(
                "klipper-header",
                "missing Orca HEADER/EXECUTABLE block markers",
            );
        }
        if let Some(totals) = self.totals.clone() {
            if totals.len() == 1 && self.tools.len() > 1 {
                self.compare_total("all tools", self.report.filament_mm.iter().sum(), totals[0]);
            } else {
                for (i, expected) in totals.into_iter().enumerate() {
                    self.compare_total(
                        &format!("T{i}"),
                        self.report.filament_mm.get(i).copied().unwrap_or(0.),
                        expected,
                    );
                }
            }
        } else {
            self.warn("filament-total", "no filament length metadata to compare");
        }
        for (i, expected) in self.expected_tools.clone() {
            self.compare_total(
                &format!("T{i}"),
                self.report.filament_mm.get(i).copied().unwrap_or(0.),
                expected,
            );
        }
        for i in 0..self.tools.len() {
            // The tolerance absorbs the rounding of the written wipe-and-retract moves: each
            // E value is rounded to 5 decimals, and OrcaSlicer counts the unrounded amounts,
            // so a wipe written as .69943 + .04252 + .03681 + .02125 counts as 0.8 mm and the
            // toolchange tops it up to 20 mm by 19.2 (GUI monkey test, seed 3011).
            if self.tools[i].retracted > self.p.tools.get(i).map_or(20., |t| t.retract_limit) + 1e-3
            {
                self.error(
                    "retraction-balance",
                    format!("T{i}: {:.2} mm remains retracted", self.tools[i].retracted),
                );
            }
        }
        self.report
    }
}

/// Validate a text stream. The line cap also bounds malformed files without newlines.
pub fn validate_text(reader: impl BufRead, profile: &Profile) -> Report {
    let mut simulator = Simulator::new(profile);
    read_text(reader, &mut simulator);
    simulator.finish()
}
fn read_text(mut reader: impl BufRead, simulator: &mut Simulator<'_>) {
    loop {
        let mut line = Vec::new();
        match reader
            .by_ref()
            .take(1024 * 1024 + 1)
            .read_until(b'\n', &mut line)
        {
            Ok(0) => break,
            Ok(_) if line.len() > 1024 * 1024 => {
                simulator.error("format", "G-code line exceeds 1 MiB");
                break;
            }
            Ok(_) => match std::str::from_utf8(&line) {
                Ok(line) => simulator.line(line),
                Err(_) => {
                    simulator.error("format", "G-code is not UTF-8 text");
                    break;
                }
            },
            Err(e) => {
                simulator.error("io", e.to_string());
                break;
            }
        }
    }
}
/// Whether the configuration block at the end of a text G-code file turns spiral vase on
/// (PrusaSlicer `; spiral_vase = 1`, OrcaSlicer `; spiral_mode = 1`).
fn declares_spiral_vase(path: &Path) -> std::io::Result<bool> {
    use std::io::{Seek, SeekFrom};
    let mut file = std::fs::File::open(path)?;
    let length = file.seek(SeekFrom::End(0))?;
    file.seek(SeekFrom::Start(length.saturating_sub(1 << 20)))?;
    let mut tail = Vec::new();
    file.read_to_end(&mut tail)?;
    Ok(String::from_utf8_lossy(&tail)
        .lines()
        .any(|l| matches!(l.trim(), "; spiral_vase = 1" | "; spiral_mode = 1")))
}

/// Detect binary magic (or .bgcode extension) and validate with bounded blocks and diagnostics.
pub fn validate_file(path: &Path, profile: &Profile) -> std::io::Result<Report> {
    let mut reader = std::io::BufReader::new(std::fs::File::open(path)?);
    let binary =
        reader.fill_buf()?.starts_with(b"GCDE") || path.extension().is_some_and(|s| s == "bgcode");
    if !binary {
        // A text file's configuration comes after its G-code: look there for spiral vase.
        if !profile.spiral_vase && declares_spiral_vase(path)? {
            let vase = Profile {
                spiral_vase: true,
                ..profile.clone()
            };
            return Ok(validate_text(reader, &vase));
        }
        return Ok(validate_text(reader, profile));
    }
    let mut sim = Simulator::new(profile);
    let mut pending = String::new();
    let result = bgcode::stream::visit(reader, 16 * 1024 * 1024, |block| {
        match block {
            Block::Metadata(_, metadata) => {
                sim.metadata = true;
                if let bgcode::Metadata::Ini(entries) = metadata {
                    for (k, v) in entries {
                        sim.comment(&format!("{k} = {v}"));
                    }
                }
            }
            Block::Thumbnail(t) => {
                let format = match t.format {
                    bgcode::ThumbnailFormat::Png => "thumbnail",
                    bgcode::ThumbnailFormat::Qoi => "thumbnail_QOI",
                    bgcode::ThumbnailFormat::Jpg => "thumbnail_JPG",
                };
                if let Err(e) = image::check(&t.data, format).and_then(|()| {
                    image::dimensions(&t.data, (usize::from(t.width), usize::from(t.height)))
                }) {
                    sim.error("thumbnail", e);
                }
            }
            Block::Gcode(text) => {
                pending.push_str(&text);
                if let Some(end) = pending.rfind('\n') {
                    for line in pending[..=end].lines() {
                        sim.line(line);
                    }
                    pending.drain(..=end);
                }
                if pending.len() > 1024 * 1024 {
                    return Err(bgcode::Error::Invalid("G-code line exceeds 1 MiB"));
                }
            }
        }
        Ok(())
    });
    if !pending.is_empty() {
        sim.line(&pending);
    }
    match result {
        Err(e) => sim.error("binary-format", e.to_string()),
        Ok(bgcode::Checksum::None) => {
            sim.warn("binary-checksum", "binary file has no block checksums")
        }
        _ => {}
    }
    Ok(sim.finish())
}
