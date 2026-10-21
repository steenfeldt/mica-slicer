// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The Calibrate menu's model: the tests the engine offers (`mica_core::calib`), sensible
//! default ranges taken from the selected filament and printer, a quick time / weight estimate
//! for the card, the calibration plate's job, and reading a result back into the one filament
//! setting it belongs to (per firmware). The panels in `panels/calibration.rs` draw it.

use mica_core::{
    calib::{self, Kind, Parameters},
    config::Config,
    print::LayerSource,
    toolpath::Role,
};

/// The entries of the Calibrate menu, in menu order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Test {
    FlowPass1,
    FlowPass2,
    PaLine,
    PaPattern,
    PaTower,
    Temperature,
    Retraction,
    MaxVolumetric,
}

impl Test {
    pub const ALL: [Test; 8] = [
        Test::FlowPass1,
        Test::FlowPass2,
        Test::PaLine,
        Test::PaPattern,
        Test::PaTower,
        Test::Temperature,
        Test::Retraction,
        Test::MaxVolumetric,
    ];

    pub fn kind(self) -> Kind {
        match self {
            Test::FlowPass1 | Test::FlowPass2 => Kind::FlowRate,
            Test::PaLine => Kind::PressureAdvanceLine,
            Test::PaPattern => Kind::PressureAdvancePattern,
            Test::PaTower => Kind::PressureAdvanceTower,
            Test::Temperature => Kind::Temperature,
            Test::Retraction => Kind::Retraction,
            Test::MaxVolumetric => Kind::MaxVolumetricSpeed,
        }
    }

    /// The `--calibrate` name, which [`Self::parse`] reads back.
    pub fn name(self) -> &'static str {
        match self {
            Test::FlowPass1 => "flow1",
            Test::FlowPass2 => "flow2",
            Test::PaLine => "pa-line",
            Test::PaPattern => "pa-pattern",
            Test::PaTower => "pa-tower",
            Test::Temperature => "temperature",
            Test::Retraction => "retraction",
            Test::MaxVolumetric => "volumetric",
        }
    }

    /// The `--calibrate` names.
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "flow1" | "flow-1" | "flow" => Test::FlowPass1,
            "flow2" | "flow-2" => Test::FlowPass2,
            "pa-line" => Test::PaLine,
            "pa-pattern" => Test::PaPattern,
            "pa-tower" => Test::PaTower,
            "temperature" => Test::Temperature,
            "retraction" => Test::Retraction,
            "volumetric" | "max-volumetric-speed" => Test::MaxVolumetric,
            _ => return None,
        })
    }

    /// Menu label.
    pub fn title(self) -> &'static str {
        match self {
            Test::FlowPass1 => "Flow rate · pass 1",
            Test::FlowPass2 => "Flow rate · pass 2",
            Test::PaLine => "Pressure advance · line",
            Test::PaPattern => "Pressure advance · pattern",
            Test::PaTower => "Pressure advance · tower",
            Test::Temperature => "Temperature tower",
            Test::Retraction => "Retraction test",
            Test::MaxVolumetric => "Max volumetric speed",
        }
    }

    /// Short name for the plate: `PA tower · Extrudr PLA`.
    pub fn short(self) -> &'static str {
        match self {
            Test::FlowPass1 => "Flow pass 1",
            Test::FlowPass2 => "Flow pass 2",
            Test::PaLine => "PA line",
            Test::PaPattern => "PA pattern",
            Test::PaTower => "PA tower",
            Test::Temperature => "Temperature tower",
            Test::Retraction => "Retraction test",
            Test::MaxVolumetric => "Max volumetric speed",
        }
    }

    pub fn is_pa(self) -> bool {
        matches!(self, Test::PaLine | Test::PaPattern | Test::PaTower)
    }
    pub fn is_flow(self) -> bool {
        matches!(self, Test::FlowPass1 | Test::FlowPass2)
    }

    /// One plain paragraph: what the test does and when to run it.
    pub fn explanation(self) -> &'static str {
        match self {
            Test::FlowPass1 => {
                "Flow rate decides how much plastic comes out for every millimeter the nozzle \
                 moves. Nine small patches print with −20 % to +20 % of the filament's current \
                 flow; pick the one whose top surface is smooth, with the lines touching but no \
                 ridges. Run this first for a new filament (after the temperature tower), then \
                 pass 2 to narrow it down. Pressure advance comes after."
            }
            Test::FlowPass2 => {
                "The fine pass: ten patches from −9 % to 0 % of the flow you saved after pass 1, \
                 in 1 % steps. Pick the smoothest top surface again. Run it with the filament \
                 profile that already holds the pass 1 result."
            }
            Test::PaLine => {
                "Pressure advance compensates for the extruder lagging behind speed changes, so \
                 corners and line ends stop bulging. Each row prints slow – fast – slow with a \
                 different value, front to back. Look for the row whose line stays even where \
                 the speed changes. One layer, quick to print; narrow the range around the best \
                 row and run it again for a finer value."
            }
            Test::PaPattern => {
                "The pattern test prints nested corners at full speed for each value, left to \
                 right. Pick the group whose corners are crisp without a bulge, a gap or a \
                 rounded tip. Slower to read than the line test, but closer to a real print."
            }
            Test::PaTower => {
                "A hollow tower whose pressure advance rises with height: the start value at the \
                 bottom, plus one step for every millimeter. Look along the corners and find the \
                 height where they turn sharp without the walls thinning; read that height in \
                 the preview or measure it from the bed. Good when you prefer one print you can \
                 hold instead of comparing lines."
            }
            Test::Temperature => {
                "A tower of blocks with the nozzle temperature embossed on the front, hottest at \
                 the bottom. Compare the bridges, the stringing between the posts, the surface \
                 shine and how well the layers bond, then pick the temperature that balances \
                 these for what you print. Run this first for a new filament."
            }
            Test::Retraction => {
                "Two thin posts far apart, so the nozzle travels and retracts hundreds of times. \
                 The retraction length rises with height; find the lowest height where the \
                 strings between the posts disappear without gaps after a travel. Run it after \
                 the temperature tower."
            }
            Test::MaxVolumetric => {
                "How fast the hotend can melt plastic. A single-wall box asks for more flow with \
                 every millimeter of height; look for where the wall first turns rough, dull or \
                 thin, read that height, and Mica saves a value with a safety margin. Useful \
                 once flow and temperature are settled."
            }
        }
    }

    /// Under the pictogram on the plate's card.
    pub fn look_for(self) -> &'static str {
        match self {
            Test::FlowPass1 | Test::FlowPass2 => {
                "Patches count left to right, front row first (bed +Y away from you). Too little \
                 flow leaves gaps between the top lines; too much piles up ridges. The best patch \
                 is smooth with the lines just touching."
            }
            Test::PaLine => {
                "Rows count from the front. Too little pressure advance bulges where the fast \
                 stretch starts and ends; too much thins the line there. The best row is even \
                 along its whole length."
            }
            Test::PaPattern => {
                "Groups count from the left. Too little pressure advance rounds and fattens the \
                 corner tips; too much leaves gaps at the tips. The best group has crisp corners."
            }
            Test::PaTower => {
                "Low bands have blobby corners (too little advance), high bands thin out at the \
                 corners (too much). Read the height where the corners turn sharp."
            }
            Test::Temperature => {
                "Hot blocks sag their bridge and string between the posts; cold blocks look dull \
                 and bond poorly. Read the number embossed on the best block."
            }
            Test::Retraction => {
                "Short retractions leave strings between the posts low down. Read the height \
                 where the strings stop; the band above it is the value to save."
            }
            Test::MaxVolumetric => {
                "The wall is smooth low down and turns rough, dull or thin higher up, where the \
                 hotend cannot keep up. Read the height where that starts."
            }
        }
    }

    /// What the user types after printing: `(label, unit, whole numbers)`.
    pub fn result_prompt(self) -> (&'static str, &'static str, bool) {
        match self {
            Test::FlowPass1 => ("Best patch (1–9)", "", true),
            Test::FlowPass2 => ("Best patch (1–10)", "", true),
            Test::PaLine => ("Best row (1 = front)", "", true),
            Test::PaPattern => ("Best group (1 = left)", "", true),
            Test::PaTower => ("Height of sharp corners", "mm", false),
            Test::Temperature => ("Best block", "°C", true),
            Test::Retraction => ("Lowest clean height", "mm", false),
            Test::MaxVolumetric => ("Height where it turns rough", "mm", false),
        }
    }

    /// The unit of the range fields, and whether the step is per millimetre of height.
    pub fn range_unit(self) -> (&'static str, bool) {
        match self {
            Test::FlowPass1 | Test::FlowPass2 => ("%", false),
            Test::PaLine | Test::PaPattern => ("s", false),
            Test::PaTower => ("s", true),
            Test::Temperature => ("°C", false),
            Test::Retraction => ("mm", true),
            Test::MaxVolumetric => ("mm³/s", true),
        }
    }

    /// Decimals shown for the range fields.
    pub fn decimals(self) -> usize {
        match self {
            Test::PaLine | Test::PaPattern | Test::PaTower => 3,
            Test::Temperature => 0,
            Test::Retraction | Test::MaxVolumetric => 1,
            Test::FlowPass1 | Test::FlowPass2 => 0,
        }
    }
}

/// The firmware the result is written for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Firmware {
    /// Prusa Buddy (CORE One, XL): `M572` in the filament start G-code owns pressure advance.
    Buddy,
    /// Klipper (Snapmaker U1): the `pressure_advance` filament setting.
    Klipper,
    Other,
}

impl Firmware {
    pub fn of(config: &Config) -> Self {
        match config.string("gcode_flavor") {
            Some("marlin2") => Firmware::Buddy,
            Some("klipper") => Firmware::Klipper,
            _ => Firmware::Other,
        }
    }
}

/// Rounds `v` to a multiple of `unit` (`0.005`, `5`).
fn round_to(v: f64, unit: f64) -> f64 {
    (v / unit).round() * unit
}

/// `0.042`, `215`, `1.2`: a number without trailing noise, at most `decimals` places.
pub fn trim_num(v: f64, decimals: usize) -> String {
    let s = format!("{v:.decimals$}");
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s
    }
}

/// The pressure advance the selected filament prints with now, per firmware (`None` when the
/// profile does not set one).
pub fn current_pressure_advance(config: &Config, tool: usize) -> Option<f64> {
    match Firmware::of(config) {
        Firmware::Klipper => config.float_at("pressure_advance", tool),
        Firmware::Buddy => {
            let nozzle = config.float_at("nozzle_diameter", tool)?;
            m572_value(config.string_at("start_filament_gcode", tool)?, nozzle)
        }
        Firmware::Other => None,
    }
}

/// Default parameters for a test, from the selected filament and printer: ranges centred on
/// the current value where there is one, otherwise the engine's defaults.
pub fn defaults(test: Test, config: &Config, tool: usize) -> Parameters {
    let mut p = Parameters::new(test.kind());
    p.tool = tool;
    match test {
        Test::FlowPass1 => p.pass = 1,
        Test::FlowPass2 => p.pass = 2,
        Test::PaLine | Test::PaPattern | Test::PaTower => {
            let (span, step) = match test {
                Test::PaLine => (0.08, 0.004),
                Test::PaPattern => (0.04, 0.002),
                _ => (0.1, 0.002),
            };
            if let Some(pa) = current_pressure_advance(config, tool).filter(|v| *v > 0.) {
                p.start = round_to((pa - span / 2.).max(0.), 0.005);
                p.end = round_to(p.start + span, 0.005);
                p.step = step;
            } else {
                p.start = 0.;
                p.end = span;
                p.step = step;
            }
        }
        Test::Temperature => {
            let normal = config.float_at("temperature", tool).unwrap_or(215.).max(1.);
            let first = config
                .float_at("first_layer_temperature", tool)
                .unwrap_or(normal);
            let low = config
                .float_at("nozzle_temperature_range_low", tool)
                .unwrap_or(normal.min(first) - 30.)
                .max(150.);
            let high = config
                .float_at("nozzle_temperature_range_high", tool)
                .unwrap_or(normal.max(first) + 30.)
                .min(350.);
            p.start = round_to(normal + 15., 5.).min(round_to(high, 5.)).max(150.);
            p.end = round_to(normal - 20., 5.)
                .max(round_to(low, 5.))
                .min(p.start);
            p.step = 5.;
        }
        Test::Retraction => {
            let current = config
                .float_at("filament_retract_length", tool)
                .filter(|v| *v > 0.)
                .or_else(|| config.float_at("retract_length", tool))
                .unwrap_or(1.);
            p.start = 0.;
            p.end = round_to((current * 2.).clamp(1., 6.), 0.5);
            p.step = round_to(p.end / 20., 0.05).max(0.05);
        }
        Test::MaxVolumetric => {
            let current = config
                .float_at("filament_max_volumetric_speed", tool)
                .filter(|v| *v > 0.)
                .unwrap_or(15.);
            p.start = round_to((current * 0.5).max(2.), 1.);
            p.end = round_to((current * 1.6).max(p.start + 10.), 1.).min(200.);
            p.step = 0.5;
            // At most 100 samples.
            while (p.end - p.start) / p.step > 99. {
                p.step *= 2.;
            }
        }
    }
    p
}

/// The card's live summary.
#[derive(Clone, Debug, PartialEq)]
pub struct Estimate {
    pub samples: usize,
    pub seconds: f64,
    pub grams: f64,
    pub height_mm: f64,
    pub layers: usize,
}

impl Estimate {
    /// `9 patches · about 18 min · 6 g`
    pub fn text(&self, test: Test) -> String {
        let what = match test {
            Test::FlowPass1 | Test::FlowPass2 => "patches",
            Test::PaLine => "rows",
            Test::PaPattern => "groups",
            Test::PaTower | Test::Retraction | Test::MaxVolumetric => "bands",
            Test::Temperature => "blocks",
        };
        let minutes = (self.seconds / 60.).round().max(1.);
        let time = if minutes >= 90. {
            format!("{:.1} h", minutes / 60.)
        } else {
            format!("{minutes:.0} min")
        };
        let grams = if self.grams < 10. {
            format!("{:.1} g", self.grams)
        } else {
            format!("{:.0} g", self.grams)
        };
        format!(
            "Prints {} {what} · about {time} · {grams} · {:.0} mm tall",
            self.samples, self.height_mm
        )
    }
}

/// A quick estimate from the generated toolpaths: lengths over the speeds the engine will ask
/// for, plus a little travel; weight from the bead volume and the filament density.
pub fn estimate(params: &Parameters, config: &Config) -> Result<Estimate, String> {
    let generated = calib::generate(params, config)?;
    let print = generated.prepare(config)?;
    let s = &print.settings;
    let n = print.layer_count();
    let mut seconds = 0.;
    let mut mm3 = 0.;
    let mut height = 0.;
    for (li, layer) in print.toolpaths(0..n).into_iter().enumerate() {
        height = layer.print_z;
        seconds += 0.5;
        for (pi, path) in layer.paths.iter().enumerate() {
            let control = print.calibration_control(li, pi);
            let role_speed = if li == 0 {
                s.first_layer_speed
            } else {
                match path.role {
                    Role::ExternalPerimeter => s.external_perimeter_speed,
                    Role::InternalInfill => s.infill_speed,
                    Role::SolidInfill => s.solid_infill_speed,
                    Role::TopSolidInfill => s.top_solid_infill_speed,
                    Role::BridgeInfill | Role::InternalBridgeInfill => s.bridge_speed,
                    _ => s.perimeter_speed,
                }
            };
            let speed = control
                .and_then(|c| c.speed_mm_s)
                .unwrap_or(role_speed)
                .max(1.);
            let length: f64 = path
                .points
                .windows(2)
                .map(|w| w[0].distance_to(w[1]))
                .sum::<f64>()
                / mica_core::geometry::SCALE;
            let ratio = control.and_then(|c| c.flow_ratio).unwrap_or(1.);
            seconds += length / speed + 0.15;
            mm3 += length * path.mm3_per_mm() * s.extrusion_multiplier * ratio;
        }
    }
    let density = config
        .float_at("filament_density", params.tool)
        .unwrap_or(1.24);
    Ok(Estimate {
        samples: generated.samples.len(),
        seconds,
        grams: mm3 / 1000. * density,
        height_mm: height,
        layers: n,
    })
}

/// The open test card.
#[derive(Clone, Debug, PartialEq)]
pub struct Card {
    pub test: Test,
    pub params: Parameters,
    pub tool: usize,
    /// `Some(speed)` when the user set a PA speed; the engine suggests one otherwise.
    pub estimate: Result<Estimate, String>,
    /// What the estimate was computed for.
    pub estimate_for: Option<(Parameters, usize)>,
}

impl Card {
    pub fn new(test: Test, config: &Config, tool: usize) -> Self {
        let mut c = Self {
            test,
            params: defaults(test, config, tool),
            tool,
            estimate: Err(String::new()),
            estimate_for: None,
        };
        c.refresh(config);
        c
    }

    /// Resets the ranges to the defaults for `tool`.
    pub fn set_tool(&mut self, tool: usize, config: &Config) {
        self.tool = tool;
        self.params = defaults(self.test, config, tool);
        self.refresh(config);
    }

    /// Recomputes the estimate when the parameters changed. `config` is the plate's
    /// configuration; the test prints on the card's tool.
    pub fn refresh(&mut self, config: &Config) {
        self.params.tool = self.tool;
        let key = (self.params.clone(), self.tool);
        if self.estimate_for.as_ref() == Some(&key) {
            return;
        }
        self.estimate = estimate(&self.params, config);
        self.estimate_for = Some(key);
    }
}

/// A calibration plate's job: what was generated and what the user read off the print.
#[derive(Clone, Debug, PartialEq)]
pub struct PlateJob {
    pub test: Test,
    pub params: Parameters,
    pub tool: usize,
    /// The filament preset the test was made for (the result goes to a user profile of it).
    pub filament: String,
    pub samples: Vec<f64>,
    /// The engine's overrides, applied to this plate's slice only (for the card's count).
    pub overrides: Vec<(String, String)>,
    /// The typed observation (patch number, height…).
    pub input: String,
    /// The user filament profile the result is written to.
    pub profile_name: String,
    /// Temperature: also move the first-layer temperature, keeping its offset.
    pub first_layer_too: bool,
    /// Confirmation after a save.
    pub saved: Option<String>,
}

/// The plate metadata key under which a calibration plate's job is saved in a project. It
/// is written only to Mica's own `Metadata/mica.json` (the native plate metadata skips
/// `mica_` keys), so OrcaSlicer and PrusaSlicer never see it.
pub const PLATE_METADATA_KEY: &str = "mica_calibration";

/// The saved form of a [`PlateJob`]: plain JSON with the test's `--calibrate` name. Numbers
/// are kept as Rust's shortest exact text, so every value reads back bit for bit.
#[derive(serde::Serialize, serde::Deserialize)]
struct StoredJob {
    version: u32,
    test: String,
    tool: usize,
    start: String,
    end: String,
    step: String,
    pass: u8,
    speed: Option<String>,
    filament: String,
    samples: Vec<String>,
    overrides: Vec<(String, String)>,
    input: String,
    profile_name: String,
    first_layer_too: bool,
    saved: Option<String>,
}

impl PlateJob {
    /// The job as the project's plate metadata value (see [`PLATE_METADATA_KEY`]).
    pub fn to_metadata(&self) -> String {
        let p = &self.params;
        serde_json::to_string(&StoredJob {
            version: 1,
            test: self.test.name().into(),
            tool: self.tool,
            start: p.start.to_string(),
            end: p.end.to_string(),
            step: p.step.to_string(),
            pass: p.pass,
            speed: p.speed.map(|v| v.to_string()),
            filament: self.filament.clone(),
            samples: self.samples.iter().map(f64::to_string).collect(),
            overrides: self.overrides.clone(),
            input: self.input.clone(),
            profile_name: self.profile_name.clone(),
            first_layer_too: self.first_layer_too,
            saved: self.saved.clone(),
        })
        .expect("a calibration job serializes")
    }

    /// Reads a job saved by [`Self::to_metadata`]; `None` for an unknown or damaged value.
    pub fn from_metadata(text: &str) -> Option<Self> {
        let s: StoredJob = serde_json::from_str(text).ok()?;
        let test = Test::parse(&s.test).filter(|_| s.version == 1)?;
        let number = |v: &str| v.parse::<f64>().ok().filter(|v| v.is_finite());
        let mut params = Parameters::new(test.kind());
        params.start = number(&s.start)?;
        params.end = number(&s.end)?;
        params.step = number(&s.step)?;
        params.speed = match &s.speed {
            Some(v) => Some(number(v)?),
            None => None,
        };
        (params.pass, params.tool) = (s.pass, s.tool);
        Some(Self {
            test,
            params,
            tool: s.tool,
            filament: s.filament,
            samples: s.samples.iter().map(|v| number(v)).collect::<Option<_>>()?,
            overrides: s.overrides,
            input: s.input,
            profile_name: s.profile_name,
            first_layer_too: s.first_layer_too,
            saved: s.saved,
        })
    }
}

/// The reading of a result: the value, how it was computed, and the profile changes.
#[derive(Clone, Debug, PartialEq)]
pub struct Outcome {
    pub value: f64,
    /// `Pressure advance 0.042`
    pub headline: String,
    /// `start 0 + 21 mm × 0.002 per mm`
    pub detail: String,
    /// Which setting changes and why, per firmware.
    pub explanation: String,
    /// Canonical key → PrusaSlicer text, for the user profile.
    pub changes: Vec<(String, String)>,
    /// Where the reading sits along the test's samples (0 = first, 1 = last), for the pictogram.
    pub pick: f32,
}

/// The position of sample `i` of `n` along the test's axis.
fn pick_index(i: usize, n: usize) -> f32 {
    (i as f32 + 0.5) / n.max(1) as f32
}

/// The sample nearest to `v`, as a position along the test's axis.
fn pick_value(samples: &[f64], v: f64) -> f32 {
    let i = samples
        .iter()
        .enumerate()
        .min_by(|a, b| (a.1 - v).abs().total_cmp(&(b.1 - v).abs()))
        .map_or(0, |(i, _)| i);
    pick_index(i, samples.len())
}

/// Reads the typed observation into a setting value. `config` is the plate's configuration;
/// current values come from the tested tool's entries, the firmware from the printer.
pub fn evaluate(job: &PlateJob, config: &Config) -> Result<Outcome, String> {
    let p = &job.params;
    let tool = job.tool;
    let (label, _, whole) = job.test.result_prompt();
    let input: f64 = job
        .input
        .trim()
        .replace(',', ".")
        .parse()
        .map_err(|_| format!("Type the {}", label.to_lowercase()))?;
    if !input.is_finite() || input < 0. {
        return Err(format!("Type the {}", label.to_lowercase()));
    }
    let n = job.samples.len();
    let index = || -> Result<usize, String> {
        let i = input.round();
        if whole && (i - input).abs() > 1e-9 || !(1. ..=n as f64).contains(&i) {
            return Err(format!("Enter a number from 1 to {n}"));
        }
        Ok(i as usize - 1)
    };
    let firmware = Firmware::of(config);
    let nozzle = config.float_at("nozzle_diameter", tool).unwrap_or(0.4);
    match job.test {
        Test::FlowPass1 | Test::FlowPass2 => {
            let i = index()?;
            let pct = job.samples[i];
            let old = config.float_at("extrusion_multiplier", tool).unwrap_or(1.);
            let new = old * (100. + pct) / 100.;
            Ok(Outcome {
                value: new,
                headline: format!("Flow ratio {}", trim_num(new, 4)),
                detail: format!(
                    "patch {} is {pct:+.0} % → {} × {} = {}",
                    i + 1,
                    trim_num(old, 4),
                    trim_num((100. + pct) / 100., 4),
                    trim_num(new, 4)
                ),
                explanation: format!(
                    "Changes the filament's flow ratio (extrusion_multiplier) from {} to {}. \
                     Every extrusion in later prints scales by it.",
                    trim_num(old, 4),
                    trim_num(new, 4)
                ),
                changes: vec![("extrusion_multiplier".into(), trim_num(new, 4))],
                pick: pick_index(i, n),
            })
        }
        Test::PaLine | Test::PaPattern | Test::PaTower => {
            let (pa, detail) = if job.test == Test::PaTower {
                let last = job.samples.last().copied().unwrap_or(p.end);
                let v = (p.start + input.floor() * p.step).min(last);
                (
                    v,
                    format!(
                        "{} + {} mm × {} per mm = {}",
                        trim_num(p.start, 4),
                        input.floor(),
                        trim_num(p.step, 4),
                        trim_num(v, 4)
                    ),
                )
            } else {
                let i = index()?;
                let v = job.samples[i];
                (
                    v,
                    format!(
                        "{} {} of {n}: {} + {} × {}",
                        if job.test == Test::PaLine {
                            "row"
                        } else {
                            "group"
                        },
                        i + 1,
                        trim_num(p.start, 4),
                        i,
                        trim_num(p.step, 4)
                    ),
                )
            };
            let value = trim_num(pa, 4);
            let (explanation, changes) = match firmware {
                Firmware::Klipper => (
                    format!(
                        "Klipper printers (the U1) take pressure advance from the filament's \
                         pressure_advance setting and send it as SET_PRESSURE_ADVANCE at the \
                         start of every print. It becomes {value}, and pressure advance is \
                         switched on for this filament."
                    ),
                    vec![
                        ("pressure_advance".to_string(), value.clone()),
                        ("enable_pressure_advance".to_string(), "1".to_string()),
                    ],
                ),
                Firmware::Buddy => {
                    let current = config.string_at("start_filament_gcode", tool).unwrap_or("");
                    let edited = set_m572(current, nozzle, pa);
                    (
                        format!(
                            "Prusa Buddy printers (CORE One, XL) take pressure advance from the \
                             M572 S… line in the filament's start G-code. The S value for the \
                             {nozzle} mm nozzle becomes {value}; other nozzle sizes keep theirs. \
                             (The profile's pressure_advance field is not used by Buddy.)"
                        ),
                        vec![(
                            "start_filament_gcode".to_string(),
                            mica_core::config::escape_string(&edited),
                        )],
                    )
                }
                Firmware::Other => {
                    return Err(
                        "Pressure advance can be saved for Prusa Buddy and Klipper printers only"
                            .into(),
                    );
                }
            };
            Ok(Outcome {
                value: pa,
                headline: format!("Pressure advance {value}"),
                detail,
                explanation,
                changes,
                pick: pick_value(&job.samples, pa),
            })
        }
        Test::Temperature => {
            let t = job
                .samples
                .iter()
                .copied()
                .min_by(|a, b| (a - input).abs().total_cmp(&(b - input).abs()))
                .ok_or("no samples")?;
            if (t - input).abs() > 2.5 {
                return Err(format!(
                    "The tower has {}–{} °C in 5 °C steps",
                    trim_num(p.end, 0),
                    trim_num(p.start, 0)
                ));
            }
            let old = config.float_at("temperature", tool).unwrap_or(t);
            let old_first = config
                .float_at("first_layer_temperature", tool)
                .unwrap_or(old);
            let offset = old_first - old;
            let mut changes = vec![("temperature".to_string(), trim_num(t, 0))];
            let mut explanation = format!(
                "Nozzle temperature for layers after the first (temperature) changes from {} to \
                 {} °C.",
                trim_num(old, 0),
                trim_num(t, 0)
            );
            if job.first_layer_too {
                let first = (t + offset).clamp(150., 350.);
                changes.push(("first_layer_temperature".to_string(), trim_num(first, 0)));
                explanation.push_str(&format!(
                    " The first layer keeps its {offset:+.0} °C offset: {} °C.",
                    trim_num(first, 0)
                ));
            } else {
                explanation.push_str(&format!(
                    " The first-layer temperature stays at {} °C.",
                    trim_num(old_first, 0)
                ));
            }
            Ok(Outcome {
                value: t,
                headline: format!("Nozzle temperature {} °C", trim_num(t, 0)),
                detail: format!("block {} °C", trim_num(t, 0)),
                explanation,
                changes,
                pick: pick_value(&job.samples, t),
            })
        }
        Test::Retraction => {
            let last = job.samples.last().copied().unwrap_or(p.end);
            let v = (p.start + (input - 0.4).max(0.).floor() * p.step).min(last);
            let printer = config.float_at("retract_length", tool).unwrap_or(0.);
            Ok(Outcome {
                value: v,
                headline: format!("Retraction length {} mm", trim_num(v, 2)),
                detail: format!(
                    "{} + ⌊{} − 0.4⌉ mm × {} per mm = {}",
                    trim_num(p.start, 2),
                    trim_num(input, 1),
                    trim_num(p.step, 3),
                    trim_num(v, 2)
                ),
                explanation: format!(
                    "Saved as the filament's retraction length override \
                     (filament_retract_length), which wins over the printer's {} mm for this \
                     filament. Retraction speed stays as it is.",
                    trim_num(printer, 2)
                ),
                changes: vec![("filament_retract_length".to_string(), trim_num(v, 2))],
                pick: pick_value(&job.samples, v),
            })
        }
        Test::MaxVolumetric => {
            let q = p.start + input * p.step;
            let saved = ((q - (q * 0.1).max(1.)) * 2.).floor() / 2.;
            let saved = saved.max(p.start.min(q));
            Ok(Outcome {
                value: saved,
                headline: format!("Max volumetric speed {} mm³/s", trim_num(saved, 1)),
                detail: format!(
                    "{} + {} mm × {} per mm = {} mm³/s at that height; saved with a 10 % margin",
                    trim_num(p.start, 1),
                    trim_num(input, 1),
                    trim_num(p.step, 2),
                    trim_num(q, 1)
                ),
                explanation: format!(
                    "The filament's max volumetric speed (filament_max_volumetric_speed) becomes \
                     {} mm³/s; the slicer then caps every speed so the flow never exceeds it.",
                    trim_num(saved, 1)
                ),
                changes: vec![(
                    "filament_max_volumetric_speed".to_string(),
                    trim_num(saved, 1),
                )],
                pick: if p.end > p.start {
                    ((q - p.start) / (p.end - p.start)).clamp(0., 1.) as f32
                } else {
                    0.5
                },
            })
        }
    }
}

/// The byte range of the value expression after `M572 S` in a filament start G-code (the
/// first `M572` outside a comment), brace-aware so `{if …}0.036{else}0{endif}` is one token.
fn m572_span(gcode: &str) -> Option<std::ops::Range<usize>> {
    let bytes = gcode.as_bytes();
    let mut offset = 0;
    for line in gcode.split_inclusive('\n') {
        let code = line.split(';').next().unwrap_or("");
        if let Some(at) = code.find("M572")
            && (at == 0 || code.as_bytes()[at - 1].is_ascii_whitespace())
        {
            let mut i = at + 4;
            while i < code.len() && code.as_bytes()[i] == b' ' {
                i += 1;
            }
            if code.as_bytes().get(i) == Some(&b'S') {
                let start = i + 1;
                let mut depth = 0i32;
                let mut end = start;
                while end < code.len() {
                    let c = code.as_bytes()[end];
                    match c {
                        b'{' => depth += 1,
                        b'}' => depth -= 1,
                        _ if depth == 0 && (c.is_ascii_whitespace() || c == b';') => break,
                        _ => {}
                    }
                    end += 1;
                }
                let _ = bytes;
                return Some(offset + start..offset + end);
            }
        }
        offset += line.len();
    }
    None
}

/// Within an `M572 S` expression, the range of the number that applies to `nozzle`: the
/// number right after `=={nozzle}}` of PrusaSlicer's nozzle conditional, or the whole
/// expression when it is a plain number.
fn m572_number(expr: &str, nozzle: f64) -> Option<std::ops::Range<usize>> {
    let number_at = |from: usize| {
        let rest = &expr[from..];
        let len = rest
            .bytes()
            .take_while(|b| b.is_ascii_digit() || *b == b'.')
            .count();
        (len > 0).then_some(from..from + len)
    };
    if !expr.contains('{') {
        return number_at(0).filter(|r| r.end == expr.len());
    }
    let needle = format!("=={nozzle}}}");
    expr.find(&needle)
        .and_then(|at| number_at(at + needle.len()))
}

/// The pressure advance a Prusa filament start G-code sets for `nozzle`.
pub fn m572_value(gcode: &str, nozzle: f64) -> Option<f64> {
    let span = m572_span(gcode)?;
    let expr = &gcode[span.clone()];
    let number = m572_number(expr, nozzle)?;
    expr[number].parse().ok()
}

/// The start G-code with its pressure advance for `nozzle` set to `value`: the matching
/// branch of PrusaSlicer's nozzle conditional (other nozzles keep theirs), a plain number, or
/// (when the expression has no branch for this nozzle) an extra `M572` line after it that
/// wins. Without any `M572` the line is added at the top.
pub fn set_m572(gcode: &str, nozzle: f64, value: f64) -> String {
    let text = trim_num(value, 4);
    let added = format!("M572 S{text} ; pressure advance (Mica calibration)");
    let Some(span) = m572_span(gcode) else {
        return if gcode.is_empty() {
            added
        } else {
            format!("{added}\n{gcode}")
        };
    };
    let expr = &gcode[span.clone()];
    match m572_number(expr, nozzle) {
        Some(number) => {
            let mut out = String::with_capacity(gcode.len() + 8);
            out.push_str(&gcode[..span.start + number.start]);
            out.push_str(&text);
            out.push_str(&gcode[span.start + number.end..]);
            out
        }
        None => {
            // Keep the conditional; a later plain command overrides it.
            let line_end = gcode[span.end..]
                .find('\n')
                .map_or(gcode.len(), |i| span.end + i);
            format!("{}\n{added}{}", &gcode[..line_end], &gcode[line_end..])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> Config {
        let mut c = Config::defaults();
        for line in include_str!("../../mica-profiles/tests/fixtures/coreone_hf04_speed_pla.ini")
            .lines()
            .filter(|l| !l.starts_with('#'))
        {
            if let Some((k, v)) = line
                .split_once(" = ")
                .or_else(|| line.strip_suffix(" =").map(|k| (k, "")))
            {
                c.set_raw(k, v).unwrap();
            }
        }
        c
    }

    const PRUSA_START: &str = "M572 S{if nozzle_diameter[filament_extruder_id]==0.4}0.036{elsif nozzle_diameter[filament_extruder_id]==0.5}0.026{elsif nozzle_diameter[filament_extruder_id]==0.6}0.022{else}0{endif} ; Filament gcode\n\nM142 S36 ; set heatbreak target temp";

    #[test]
    fn m572_reads_and_edits_the_nozzle_branch_only() {
        assert_eq!(m572_value(PRUSA_START, 0.4), Some(0.036));
        assert_eq!(m572_value(PRUSA_START, 0.6), Some(0.022));
        assert_eq!(m572_value(PRUSA_START, 0.25), None);
        let edited = set_m572(PRUSA_START, 0.4, 0.042);
        assert_eq!(m572_value(&edited, 0.4), Some(0.042));
        assert_eq!(m572_value(&edited, 0.5), Some(0.026));
        assert!(edited.contains("{else}0{endif} ; Filament gcode\n\nM142 S36"));
        assert_eq!(edited.len(), PRUSA_START.len());
        // A plain command: the number is replaced in place.
        assert_eq!(
            set_m572("M572 S0.036 ; PA\nM142 S36", 0.4, 0.05),
            "M572 S0.05 ; PA\nM142 S36"
        );
        assert_eq!(m572_value("M572 S0.036 ; PA", 0.4), Some(0.036));
        // No branch for this nozzle: the conditional stays and a later command wins.
        let other = set_m572(PRUSA_START, 0.25, 0.1);
        assert!(other.starts_with(PRUSA_START.lines().next().unwrap()));
        assert!(
            other
                .lines()
                .nth(1)
                .unwrap()
                .starts_with("M572 S0.1 ; pressure advance")
        );
        // No M572 at all: added at the top. A commented M572 does not count.
        assert_eq!(
            set_m572("M142 S36", 0.4, 0.03),
            "M572 S0.03 ; pressure advance (Mica calibration)\nM142 S36"
        );
        assert_eq!(m572_value("; M572 S0.1 was here\nM104 S0", 0.4), None);
        assert_eq!(
            set_m572("", 0.4, 0.03),
            "M572 S0.03 ; pressure advance (Mica calibration)"
        );
    }

    #[test]
    fn defaults_follow_the_filament() {
        let c = config();
        assert_eq!(Firmware::of(&c), Firmware::Buddy);
        // The CORE One profile's start G-code sets 0.036 for the 0.4 mm nozzle.
        assert_eq!(current_pressure_advance(&c, 0), Some(0.036));
        let p = defaults(Test::PaTower, &c, 0);
        assert_eq!(p.kind, Kind::PressureAdvanceTower);
        assert!((p.start - 0.).abs() < 1e-9 || p.start < 0.036);
        assert!(p.end > 0.036 && p.end <= 0.1 + 1e-9);
        assert_eq!(p.step, 0.002);
        let t = defaults(Test::Temperature, &c, 0);
        assert_eq!(t.step, 5.);
        assert!(t.start > t.end);
        assert_eq!(t.start % 5., 0.);
        assert_eq!(t.end % 5., 0.);
        let f = defaults(Test::FlowPass2, &c, 0);
        assert_eq!((f.kind, f.pass), (Kind::FlowRate, 2));
        let r = defaults(Test::Retraction, &c, 0);
        assert!(r.end >= 1. && r.end <= 6. && r.step > 0.);
        let v = defaults(Test::MaxVolumetric, &c, 0);
        assert!((v.end - v.start) / v.step <= 99.);
        // Every default generates.
        for test in Test::ALL {
            let p = defaults(test, &c, 0);
            calib::generate(&p, &c).unwrap_or_else(|e| panic!("{test:?}: {e}"));
        }
    }

    #[test]
    fn estimate_is_plausible() {
        let c = config();
        let e = estimate(&defaults(Test::PaTower, &c, 0), &c).unwrap();
        assert_eq!(e.samples, 51);
        assert!((e.height_mm - 51.).abs() < 0.3, "{}", e.height_mm);
        assert!(e.seconds > 120. && e.seconds < 3600., "{}", e.seconds);
        assert!(e.grams > 0.5 && e.grams < 20., "{}", e.grams);
        assert!(
            e.text(Test::PaTower)
                .starts_with("Prints 51 bands · about ")
        );
        let f = estimate(&defaults(Test::FlowPass1, &c, 0), &c).unwrap();
        assert_eq!(f.samples, 9);
        assert!(f.grams > 2. && f.grams < 40., "{}", f.grams);
    }

    fn job(test: Test, c: &Config) -> PlateJob {
        let params = defaults(test, c, 0);
        let g = calib::generate(&params, c).unwrap();
        PlateJob {
            test,
            params,
            tool: 0,
            filament: "Prusament PLA".into(),
            samples: g.samples.clone(),
            overrides: g.overrides.clone(),
            input: String::new(),
            profile_name: "Prusament PLA tuned".into(),
            first_layer_too: true,
            saved: None,
        }
    }

    #[test]
    fn results_become_the_right_setting_per_firmware() {
        let c = config();
        // Flow pass 1, patch 4 = −5 %: multiplier × 0.95.
        let mut j = job(Test::FlowPass1, &c);
        j.input = "4".into();
        let o = evaluate(&j, &c).unwrap();
        let old = c.float("extrusion_multiplier").unwrap();
        assert!((o.value - old * 0.95).abs() < 1e-9);
        assert_eq!(o.changes[0].0, "extrusion_multiplier");
        j.input = "10".into();
        assert!(evaluate(&j, &c).is_err());
        j.input = "2.5".into();
        assert!(evaluate(&j, &c).is_err());
        // PA tower: start + floor(z) × step, on Buddy into the start G-code.
        let mut j = job(Test::PaTower, &c);
        j.input = "21.7".into();
        let o = evaluate(&j, &c).unwrap();
        let expected = j.params.start + 21. * j.params.step;
        assert!((o.value - expected).abs() < 1e-9);
        assert_eq!(
            o.headline,
            format!("Pressure advance {}", trim_num(expected, 4))
        );
        assert_eq!(o.changes.len(), 1);
        assert_eq!(o.changes[0].0, "start_filament_gcode");
        let gcode = mica_core::config::unescape_string(&o.changes[0].1).unwrap();
        assert_eq!(m572_value(&gcode, 0.4), Some(expected));
        assert_eq!(m572_value(&gcode, 0.6), Some(0.022));
        assert!(o.explanation.contains("M572"));
        // Beyond the top band the last sample is used.
        j.input = "500".into();
        let o = evaluate(&j, &c).unwrap();
        assert_eq!(o.value, *j.samples.last().unwrap());
        // PA line row 3 is the third sample.
        let mut j = job(Test::PaLine, &c);
        j.input = "3".into();
        assert_eq!(evaluate(&j, &c).unwrap().value, j.samples[2]);
        // Klipper: the pressure_advance setting, switched on.
        let mut k = c.clone();
        k.set_raw("gcode_flavor", "klipper").unwrap();
        let o = evaluate(&j, &k).unwrap();
        assert_eq!(
            o.changes,
            vec![
                ("pressure_advance".to_string(), trim_num(j.samples[2], 4)),
                ("enable_pressure_advance".to_string(), "1".to_string()),
            ]
        );
        assert!(o.explanation.contains("SET_PRESSURE_ADVANCE"));
        // Temperature snaps to the embossed block and keeps the first-layer offset.
        let mut j = job(Test::Temperature, &c);
        j.input = "214".into();
        let o = evaluate(&j, &c).unwrap();
        assert_eq!(o.value, 215.);
        let old = c.float("temperature").unwrap();
        let old_first = c.float("first_layer_temperature").unwrap();
        assert_eq!(o.changes[0], ("temperature".to_string(), "215".to_string()));
        assert_eq!(
            o.changes[1],
            (
                "first_layer_temperature".to_string(),
                trim_num(215. + old_first - old, 0)
            )
        );
        j.first_layer_too = false;
        assert_eq!(evaluate(&j, &c).unwrap().changes.len(), 1);
        j.input = "600".into();
        assert!(evaluate(&j, &c).is_err());
        // Retraction: Orca's schedule from the height.
        let mut j = job(Test::Retraction, &c);
        j.input = "5.2".into();
        let o = evaluate(&j, &c).unwrap();
        assert!((o.value - (j.params.start + 4. * j.params.step)).abs() < 1e-9);
        assert_eq!(o.changes[0].0, "filament_retract_length");
        // Max volumetric speed: Q at the height, saved with a 10 % margin on half steps.
        let mut j = job(Test::MaxVolumetric, &c);
        j.input = "20".into();
        let o = evaluate(&j, &c).unwrap();
        let q = j.params.start + 20. * j.params.step;
        assert!(o.value < q && o.value >= q * 0.85, "{} vs {q}", o.value);
        assert_eq!(o.value * 2., (o.value * 2.).round());
        assert_eq!(o.changes[0].0, "filament_max_volumetric_speed");
    }

    /// A test for tool 2 is generated, estimated and read on the plate's own configuration
    /// with tool 2's entries: its nozzle, filament and current values.
    #[test]
    fn a_second_tool_is_tested_with_its_own_settings() {
        let mut c = config();
        c.set_raw("nozzle_diameter", "0.4,0.4").unwrap();
        c.set_raw("temperature", "215,245").unwrap();
        c.set_raw("first_layer_temperature", "215,250").unwrap();
        c.set_raw("extrusion_multiplier", "1,0.95").unwrap();
        c.set_raw("filament_density", "1.24,2.48").unwrap();
        let t = defaults(Test::Temperature, &c, 1);
        assert_eq!(t.tool, 1);
        assert_eq!(t.start % 5., 0.);
        assert!(t.start > 245. && t.end < 245., "{t:?}");
        let mut card = Card::new(Test::PaTower, &c, 0);
        let first = card.estimate.clone().unwrap();
        card.set_tool(1, &c);
        assert_eq!(card.params.tool, 1);
        let second = card.estimate.clone().unwrap();
        // Twice the density at 0.95 flow.
        let ratio = second.grams / first.grams;
        assert!((ratio - 1.9).abs() < 0.01, "{first:?} {second:?}");
        let mut j = job(Test::FlowPass1, &c);
        j.tool = 1;
        j.params.tool = 1;
        j.input = "5".into();
        let o = evaluate(&j, &c).unwrap();
        // 0.95 (tool 2's flow) × (100 + 0) / 100.
        assert_eq!(
            o.changes,
            [("extrusion_multiplier".to_string(), "0.95".to_string())]
        );
        let mut j = job(Test::Temperature, &c);
        j.tool = 1;
        j.input = format!("{}", j.samples[0]);
        let o = evaluate(&j, &c).unwrap();
        assert!(o.explanation.contains("from 245 to"), "{}", o.explanation);
        assert!(o.explanation.contains("+5 °C offset"), "{}", o.explanation);
    }

    #[test]
    fn names_parse_and_format() {
        for t in Test::ALL {
            assert!(!t.explanation().is_empty() && !t.look_for().is_empty());
            assert!(t.explanation().len() < 520, "{t:?} explanation is long");
        }
        assert_eq!(Test::parse("pa-tower"), Some(Test::PaTower));
        assert_eq!(Test::parse("flow2"), Some(Test::FlowPass2));
        assert_eq!(Test::parse("nonsense"), None);
        assert_eq!(trim_num(0.0420, 4), "0.042");
        assert_eq!(trim_num(215., 0), "215");
        assert_eq!(trim_num(1.5, 2), "1.5");
        let e = Estimate {
            samples: 9,
            seconds: 18. * 60.,
            grams: 6.2,
            height_mm: 2.,
            layers: 10,
        };
        assert_eq!(
            e.text(Test::FlowPass1),
            "Prints 9 patches · about 18 min · 6.2 g · 2 mm tall"
        );
    }
}
