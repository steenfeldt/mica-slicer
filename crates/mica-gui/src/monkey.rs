// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The GUI monkey test: a headless `App` with the real profiles runs thousands of random but
//! valid user actions from the editor's own vocabulary (viewport `Action`s, object `Command`s,
//! settings edits, printer and filament switches, plates, cut, text / SVG emboss, painting, the prime tower,
//! slicing with cancels, preview scrubbing and colour schemes, the measure tool and the section
//! view, project save/load round trips, undo/redo bursts, the model library over a temporary folder)
//! and random pointer/keyboard frames through the whole window, checking invariants after every
//! step:
//!
//! - no panic on any thread (a panic hook records it; each step runs under `catch_unwind`),
//! - no step and no slice hangs, and a cancelled job stops within a timeout,
//! - undo all then redo all returns to the same scene,
//! - a slice a plate shows was made from its current inputs (colours aside), and a slice the
//!   cache gave back is byte for byte what a fresh slice of that state writes,
//! - the scene stays valid (finite transforms, indices within their plates),
//! - process memory (macOS physical footprint, else resident size) stays bounded over any
//!   1,000 steps,
//! - every completed slice passes `mica validate` (zero errors),
//! - a failed slice always shows: until its plate changes, the plate keeps the error, shows
//!   no slice and carries the error marker, and its notification is shown (or was dismissed),
//!   so the header reads "Slice failed" instead of old totals,
//! - no slice temp folder outlives the app.
//!
//! The random generator is seeded, so a failure is reproduced by its seed; the report carries
//! the trace of actions that led to it.
//!
//! ```sh
//! mica-gui --monkey [--seeds 20] [--first 0] [--steps 2000] [--jobs 4] [--quick]
//! mica-gui --monkey --seed 7 [--steps 2000] [--verbose]     # one seed, in this process
//! mica-gui --monkey --seed 3 --steps 198 --save-at 197      # also save the scene before step 197
//! ```
//! Without `--seed` every seed runs in its own child process (crashes that `catch_unwind`
//! cannot stop, such as aborts or stack overflows, and hangs are reported per seed).

use crate::{
    app::{App, Options, Snapshot},
    commands::{Command, Shape},
    overlays::Target,
    panels::editor::Edit,
    scene::{Delta, Sub},
    settings::Tab,
    viewport::Action,
};
use eframe::egui;
use glam::Vec3;
use mica_core::{
    config::{Config, OptType, option_defs},
    painting::PaintKind,
};
use mica_profiles::user::Store;
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

/// SplitMix64: small, fast and good enough for choosing actions; no dependency.
#[derive(Clone)]
pub struct Rng(u64);
impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed ^ 0x6d69_6361_6d6f_6e6b)
    }
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
    /// Uniform in `0..n` (0 when `n` is 0).
    pub fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next_u64() % n as u64) as usize
        }
    }
    pub fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
    pub fn chance(&mut self, p: f64) -> bool {
        self.unit() < p
    }
    pub fn range(&mut self, a: f64, b: f64) -> f64 {
        a + (b - a) * self.unit()
    }
    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
    fn vec3(&mut self, r: f64) -> Vec3 {
        Vec3::new(
            self.range(-r, r) as f32,
            self.range(-r, r) as f32,
            self.range(-r, r) as f32,
        )
    }
}

/// How long and how hard one seed runs.
#[derive(Clone, Debug)]
pub struct Limits {
    pub steps: usize,
    /// A wait for a slice gives up (and cancels) after this long.
    pub slice_wait: Duration,
    /// A cancelled job must have stopped within this long.
    pub cancel_timeout: Duration,
    /// One step (excluding slice waits) taking longer than this is a hang.
    pub step_timeout: Duration,
    /// Allowed RSS growth over any window of 1,000 steps.
    pub rss_growth_mb: f64,
    /// Steps between full undo-all / redo-all consistency checks.
    pub undo_check_every: usize,
    /// Every wait for a slice cancels it after this long (`--cancel-after MS`).
    pub cancel_after: Option<Duration>,
    /// Steps between memory samples (`--rss-every N`).
    pub rss_every: usize,
    /// Save the scene as an Orca project before this step (`--save-at STEP`, one seed).
    pub save_at: Option<usize>,
    /// Fail when slice folders outlive the app (off when other tests share the process).
    pub check_leaks: bool,
    pub verbose: bool,
    /// Draw the actions of `monkey/features.rs` too (`--no-new-features`: the action sequence
    /// of runs before them, to reproduce an older seed).
    pub new_features: bool,
}
impl Limits {
    pub fn new(steps: usize) -> Self {
        // Unoptimised builds (which check integer overflow) run the engine some 20× slower.
        let slow = if cfg!(debug_assertions) { 10 } else { 1 };
        Self {
            steps,
            slice_wait: Duration::from_secs(25 * slow),
            cancel_timeout: Duration::from_secs(15 * slow),
            step_timeout: Duration::from_secs(180 * slow),
            // One slice of a painted Benchy scaled 3× at 0.07 mm layers (1,916 layers, two
            // instances) on the XL holds 1.7 GB of prepared walls and peaks near 2.7 GB; the
            // allocator keeps some of it after the job. Not a
            // leak: the live heap returns to the scene's size.
            rss_growth_mb: 3000.,
            undo_check_every: 100,
            cancel_after: None,
            rss_every: 50,
            save_at: None,
            check_leaks: true,
            verbose: false,
            new_features: true,
        }
    }
}

/// What one seed did.
#[derive(Clone, Debug, Default)]
pub struct Stats {
    pub steps: usize,
    pub slices_started: usize,
    pub slices_completed: usize,
    pub slices_validated: usize,
    pub slice_errors: usize,
    pub cancels: usize,
    pub undo_checks: usize,
    /// Slices the cache gave back that were compared with a fresh slice of the same state.
    pub cache_checks: usize,
    pub round_trips: usize,
    pub ui_frames: usize,
    pub rss_start_mb: f64,
    pub rss_peak_mb: f64,
    pub rss_max_growth_mb: f64,
    pub seconds: f64,
    /// Validator warnings over every completed slice.
    pub validator_warnings: usize,
    /// Completed slices with validator errors whose settings were implausible.
    pub implausible_invalid: usize,
    /// Longest time a cancelled job took to stop (seconds, as seen between steps).
    pub cancel_max_s: f64,
    /// Mica MCP live calls sent and answered.
    pub mcp_calls: usize,
    pub mcp_answered: usize,
}

/// Why a seed failed, with the trace that got there.
#[derive(Debug)]
pub struct Failure {
    pub seed: u64,
    pub step: usize,
    pub kind: &'static str,
    pub message: String,
    pub trace: Vec<String>,
}
impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(
            f,
            "MONKEY FAILURE seed {} step {}: {}\n{}",
            self.seed, self.step, self.kind, self.message
        )?;
        writeln!(f, "Trace (last {} actions):", self.trace.len())?;
        for line in &self.trace {
            writeln!(f, "  {line}")?;
        }
        write!(
            f,
            "Reproduce: mica-gui --monkey --seed {} --steps {}",
            self.seed,
            self.step + 1
        )
    }
}

// ----- panic capture ------------------------------------------------------------------------

/// Panics seen on any thread since the last `take_panics` (message, location, backtrace).
static PANICS: Mutex<Vec<String>> = Mutex::new(Vec::new());
static HOOK: std::sync::Once = std::sync::Once::new();

fn install_panic_hook() {
    HOOK.call_once(|| {
        // The default hook still prints (so test failures and contained panics show).
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let message = info
                .payload()
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| info.payload().downcast_ref::<&str>().copied())
                .unwrap_or("non-text panic");
            let location = info
                .location()
                .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
                .unwrap_or_default();
            let thread = std::thread::current()
                .name()
                .unwrap_or("unnamed")
                .to_string();
            // Under `cargo test`, another test's own thread (named by its path) panicking on
            // purpose (`should_panic`) is not the monkey's: Mica's threads have no `::`.
            if cfg!(test) && thread.contains("::") && !thread.starts_with("monkey::") {
                previous(info);
                return;
            }
            let backtrace = std::backtrace::Backtrace::force_capture().to_string();
            // Only the frames of this workspace, to keep reports readable.
            let frames: Vec<&str> = backtrace
                .lines()
                .filter(|l| l.contains("mica") && !l.contains("monkey::install_panic_hook"))
                .take(24)
                .collect();
            let text = format!(
                "panic on thread '{thread}' at {location}: {message}\n    {}",
                frames.join("\n    ")
            );
            // Under `cargo test` other tests run beside the short monkey run on threads named
            // after them (`printhost::keychain::tests::…`), some panicking on purpose: only
            // the monkey's own thread and the threads it starts count.
            let other_test = thread.contains("::") && !thread.contains("monkey");
            if !other_test && let Ok(mut p) = PANICS.lock() {
                p.push(text);
            }
            previous(info);
        }));
    });
}

fn take_panics() -> Vec<String> {
    PANICS
        .lock()
        .map(|mut p| std::mem::take(&mut *p))
        .unwrap_or_default()
}

// ----- fixtures -----------------------------------------------------------------------------

const MODELS: &[&str] = &[
    "cube20.stl",
    "3DBenchy.stl",
    "hole_block.stl",
    "painted-cube.3mf",
    "plates-orca.3mf",
];

fn models_dir() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/models"))
}

/// Copies the fixtures into `dir`: a project opened from there becomes the document, and a
/// scripted Save (⌘S) then writes the copy, never the repository's file.
fn copy_models(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let src = models_dir();
    MODELS
        .iter()
        .map(|m| {
            let to = dir.join(m);
            std::fs::copy(src.join(m), &to)
                .map_err(|e| format!("{}: {e}", src.join(m).display()))?;
            Ok(to)
        })
        .collect()
}

/// Every editable key of a settings tab, from the generated page layout.
fn tab_keys(tab: Tab) -> Vec<&'static str> {
    let mut keys: Vec<&'static str> = crate::settings::pages(tab)
        .iter()
        .flat_map(|p| p.groups.iter())
        .flat_map(|g| g.lines.iter())
        .flat_map(|l| l.keys.iter().copied())
        .filter(|k| option_defs().contains_key(k))
        .collect();
    keys.sort_unstable();
    keys.dedup();
    keys
}

/// A value the editor could produce for `key`: an enum choice, a toggle, or a number inside
/// the option table's range (often at its ends, sometimes far out where it is unbounded).
fn random_value(rng: &mut Rng, key: &str) -> Option<String> {
    let def = option_defs().get(key)?;
    let text = crate::settings::text().get(key);
    let element = match def.ty {
        OptType::Floats => OptType::Float,
        OptType::Ints => OptType::Int,
        OptType::Bools => OptType::Bool,
        OptType::Percents => OptType::Percent,
        OptType::FloatsOrPercents => OptType::FloatOrPercent,
        OptType::Strings => OptType::String,
        OptType::Enums => OptType::Enum,
        OptType::Points => OptType::Point,
        t => t,
    };
    let default: f64 = def
        .default
        .split(',')
        .next()
        .unwrap_or("")
        .trim_end_matches('%')
        .parse()
        .unwrap_or(1.);
    let (min, max) = text.map(|t| (t.min, t.max)).unwrap_or((None, None));
    let number = |rng: &mut Rng| -> f64 {
        let lo = min.unwrap_or(0f64.min(default));
        let hi = max.unwrap_or((default.abs() * 2.).max(10.));
        match rng.below(10) {
            0 => lo,
            1 => hi,
            // Unbounded fields accept anything: far beyond the usual range.
            2 if max.is_none() => *rng.pick(&[1e3, 1e4, 1e6]),
            3 if min.is_none() => *rng.pick(&[-1., -100., 0.]),
            4 => default,
            _ => rng.range(lo, hi.max(lo)),
        }
    };
    Some(match element {
        OptType::Bool => rng.pick(&["0", "1"]).to_string(),
        OptType::Int => format!("{}", number(rng).round() as i64),
        OptType::Float => format!("{}", round(number(rng))),
        OptType::Percent => format!("{}%", round(number(rng))),
        OptType::FloatOrPercent => {
            if rng.chance(0.5) {
                format!("{}%", round(number(rng)))
            } else {
                format!("{}", round(number(rng)))
            }
        }
        OptType::Enum => {
            let choices = text.map(|t| t.enums.clone()).unwrap_or_default();
            if choices.is_empty() {
                return None;
            }
            rng.pick(&choices).0.clone()
        }
        OptType::String => {
            // Custom G-code and free text: empty, a comment, or a broken template.
            rng.pick(&[
                "",
                "; monkey",
                "{if layer_num == 1}M117 x{endif}",
                "{",
                "[unknown_placeholder]",
            ])
            .to_string()
        }
        OptType::Point => {
            if key != "bed_shape" {
                return None;
            }
            rng.pick(&[
                "0x0,250x0,250x220,0x220",
                "0x0,120x0,120x120,0x120",
                "-20x-20,300x-20,300x300,-20x300",
                "0x0,1x0,1x1",
                "0x0",
            ])
            .to_string()
        }
        _ => return None,
    })
}

fn round(v: f64) -> f64 {
    (v * 1000.).round() / 1000.
}

/// Whether a numeric setting is far outside anything printable: beyond the option table's
/// range, or (where the table sets no bound) beyond 20× its default or below zero for a
/// non-negative option. Such values must not crash or hang anything, but G-code made with
/// them (a 10 m retraction, a 1000 mm Z hop) is expected to fail validation, so the monkey
/// counts those slices instead of failing on them.
fn implausible(key: &str, raw: &str) -> bool {
    let Some(def) = option_defs().get(key) else {
        return false;
    };
    // Custom G-code edited by hand (a comment for a start script): heating and homing are
    // then the user's, and Klipper printers get no automatic temperature commands.
    if key.ends_with("_gcode") && matches!(def.ty, OptType::String | OptType::Strings) {
        return true;
    }
    if !matches!(
        def.ty,
        OptType::Float
            | OptType::Floats
            | OptType::Int
            | OptType::Ints
            | OptType::Percent
            | OptType::Percents
            | OptType::FloatOrPercent
            | OptType::FloatsOrPercents
    ) {
        return false;
    }
    let text = crate::settings::text().get(key);
    let (min, max) = text.map(|t| (t.min, t.max)).unwrap_or((None, None));
    let default: f64 = def
        .default
        .split(',')
        .next()
        .unwrap_or("")
        .trim_end_matches('%')
        .parse()
        .unwrap_or(0.);
    // Temperatures: the validator's limits, which are stricter than the option tables for
    // beds (they allow 300 °C, the validator 150 °C; GUI monkey test, seed 3015).
    let celsius = text.is_some_and(|t| t.unit.contains("°C"));
    let mut hi = max.unwrap_or(if celsius {
        mica_gcode::validate::MAX_HOTEND_TEMPERATURE
    } else {
        (default.abs() * 20.).max(20.)
    });
    if celsius {
        let limit = if is_bed_temperature(key) {
            mica_gcode::validate::MAX_BED_TEMPERATURE
        } else {
            mica_gcode::validate::MAX_HOTEND_TEMPERATURE
        };
        hi = hi.min(limit);
    }
    let lo = min.unwrap_or(if default >= 0. { 0. } else { f64::NEG_INFINITY });
    raw.split(',').any(|v| {
        v.trim()
            .trim_end_matches('%')
            .parse::<f64>()
            .is_ok_and(|v| !v.is_finite() || v > hi + 1e-9 || v < lo - 1e-9)
    })
}

/// A bed temperature (`bed_temperature`, `first_layer_bed_temperature`; Orca's plate
/// temperatures map onto them).
fn is_bed_temperature(key: &str) -> bool {
    key.ends_with("bed_temperature")
}

/// The implausible values (see `implausible`) in the app's settings and object overrides.
fn implausible_settings(app: &App) -> Vec<String> {
    let mut out = vec![];
    let mut scan = |what: &str, map: &std::collections::BTreeMap<String, String>| {
        for (k, v) in map {
            if implausible(k, v) {
                out.push(format!("{what} {k} = {v}"));
            }
        }
    };
    scan("print", &app.overlays.print);
    scan("printer", &app.overlays.printer);
    for (t, f) in app.overlays.filaments.iter().enumerate() {
        scan(&format!("filament {}", t + 1), f);
    }
    let mut imported = vec![];
    // Profiles imported from the fake home hold extreme values on purpose (a 999 °C PLA;
    // GUI monkey test, seed 409): in use, they count like edits.
    let in_use = |name: &str| {
        app.profiles.process == name
            || app.profiles.printer == name
            || app.profiles.filaments.iter().any(|f| f == name)
    };
    for p in app.profiles.imported() {
        if !in_use(&p.name) {
            continue;
        }
        scan(&p.name, &p.settings);
        for (k, v) in p.orca.iter().flatten() {
            let text = match v {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Array(a) => a
                    .iter()
                    .filter_map(|x| x.as_str())
                    .collect::<Vec<_>>()
                    .join(","),
                _ => continue,
            };
            let hot = k.contains("temperature")
                && text.split(',').any(|t| {
                    t.trim().parse::<f64>().is_ok_and(|t| {
                        !(0. ..=mica_gcode::validate::MAX_HOTEND_TEMPERATURE).contains(&t)
                    })
                });
            if hot || implausible(k, &text) {
                imported.push(format!("{} {k} = {text}", p.name));
            }
        }
    }
    for p in &app.plates.plates {
        for o in &p.objects {
            scan(&o.name, &o.overrides);
            for v in &o.volumes {
                scan(&v.name, &v.overrides);
            }
            for r in &o.height_ranges {
                scan("range", &r.overrides);
            }
        }
    }
    out.extend(imported);
    out
}

/// The mock printers (`crates/mica-printhost/examples/mock_printer.rs`, built next to this
/// binary) on free loopback ports: a PrusaLink XL 5T, a PrusaLink CORE One and a Snapmaker
/// U1, plus the mock's RTSP camera and "IP camera" (ports 3 and 4). Killed with the run.
struct MockPrinters {
    child: std::process::Child,
    ports: [u16; 5],
}
impl Drop for MockPrinters {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
impl MockPrinters {
    fn start() -> Option<Self> {
        let exe = std::env::current_exe().ok()?;
        let dir = exe.parent()?;
        let name = format!("mock_printer{}", std::env::consts::EXE_SUFFIX);
        // Next to a release/debug binary, or one level up from a test binary in deps/.
        let mock = [
            dir.join("examples").join(&name),
            dir.join("../examples").join(&name),
        ]
        .into_iter()
        .find(|p| p.exists())?;
        let free = || {
            std::net::TcpListener::bind("127.0.0.1:0")
                .ok()
                .and_then(|l| l.local_addr().ok())
                .map(|a| a.port())
        };
        let ports = [free()?, free()?, free()?, free()?, free()?];
        let child = std::process::Command::new(mock)
            .args(["--xl", &ports[0].to_string()])
            .args(["--coreone", &ports[1].to_string()])
            .args(["--u1", &ports[2].to_string()])
            .args(["--rtsp", &ports[3].to_string()])
            .args(["--ipcam", &ports[4].to_string()])
            .args(["--job-seconds", "40"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .ok()?;
        let mock = Self { child, ports };
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if ports
                .iter()
                .all(|p| std::net::TcpStream::connect(("127.0.0.1", *p)).is_ok())
            {
                return Some(mock);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        None
    }
}

// ----- the driver ---------------------------------------------------------------------------

mod features;

struct Monkey {
    seed: u64,
    rng: Rng,
    ctx: egui::Context,
    app: App,
    limits: Limits,
    trace: Arc<Mutex<VecDeque<String>>>,
    step: usize,
    stats: Stats,
    models: Vec<PathBuf>,
    dir: PathBuf,
    printers: Vec<(usize, String)>,
    print_keys: Vec<&'static str>,
    filament_keys: Vec<&'static str>,
    printer_keys: Vec<&'static str>,
    object_keys: Vec<&'static str>,
    /// Since when the cancelled job has been stopping.
    stopping_since: Option<Instant>,
    /// The configuration each running request was started with (validation profile).
    job_configs: Vec<(u64, Config, Vec<String>)>,
    /// Completed slices still to validate: (job folder, plate, the job's configuration).
    to_validate: Vec<(PathBuf, usize, Option<JobConfig>)>,
    /// Invariant violations found inside an action (reported by the next check).
    problems: Vec<String>,
    /// The loopback mock printers, when built (`None`: printer actions only see none).
    _mock: Option<MockPrinters>,
    /// How many of `app.printers` are the mock's (they come first and stay).
    mock_printers: usize,
    /// RSS samples (step, MB).
    rss: Vec<(usize, f64)>,
    /// Frames for the UI pass still to deliver (a scripted pointer gesture).
    time: f64,
    /// A home folder with other slicers' presets for the profile import.
    home: PathBuf,
    /// Mica MCP live mode, in process.
    mcp: features::Mcp,
}

/// The configuration a job started with and the implausible settings in it.
type JobConfig = (Config, Vec<String>);

const TRACE_LEN: usize = 60;

impl Monkey {
    fn new(seed: u64, limits: Limits, trace: Arc<Mutex<VecDeque<String>>>) -> Result<Self, String> {
        let dir = std::env::temp_dir().join(format!(
            "mica-monkey-{}-{seed}-{}",
            std::process::id(),
            Instant::now().elapsed().as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("store")).map_err(|e| e.to_string())?;
        let models = copy_models(&dir)?;
        let ctx = egui::Context::default();
        let app = App::create(
            &ctx,
            Options {
                prefs_path: None,
                store: Some(Store::open(dir.join("store"))),
                files: vec![],
                headless: true,
            },
        );
        let all = app.profiles.all_printers();
        let mut printers = vec![];
        for want in [
            |n: &str| n == "Prusa CORE One HF0.4 nozzle",
            |n: &str| {
                n.contains("XL") && n.contains("5T") && n.contains("0.4") && !n.contains("HF")
            },
            |n: &str| n.contains("Snapmaker U1") && n.contains("0.4"),
        ] {
            if let Some((v, _, n)) = all.iter().find(|(_, _, n)| want(n)) {
                printers.push((*v, n.clone()));
            }
        }
        if printers.is_empty() {
            return Err("no printer profiles found".into());
        }
        let object_keys = crate::settings::OBJECT_OVERRIDES
            .iter()
            .flat_map(|(_, keys)| keys.iter().copied())
            .collect();
        let mut monkey = Self {
            seed,
            rng: Rng::new(seed),
            ctx,
            app,
            limits,
            trace,
            step: 0,
            stats: Stats::default(),
            models,
            dir,
            printers,
            print_keys: tab_keys(Tab::Print),
            filament_keys: tab_keys(Tab::Filament),
            printer_keys: tab_keys(Tab::Printer),
            object_keys,
            stopping_since: None,
            job_configs: vec![],
            to_validate: vec![],
            problems: vec![],
            _mock: None,
            mock_printers: 0,
            rss: vec![],
            time: 0.,
            home: PathBuf::new(),
            mcp: features::Mcp::default(),
        };
        monkey.home = features::fake_home(&monkey.dir);
        // Mica MCP live mode without a socket: calls queue for the window like the socket's.
        let (host, tx) = crate::mcp_live::Host::in_process();
        monkey.app.mcp = host;
        monkey.mcp.tx = Some(tx);
        // Printers: this process talks to loopback addresses only; the mock answers there.
        crate::printhost::restrict_to_loopback();
        if let Some(mock) = MockPrinters::start() {
            use crate::printhost::config::{Kind, Printer};
            for (name, kind, port) in [
                ("Mock XL", Kind::PrusaLink, mock.ports[0]),
                ("Mock CORE One", Kind::PrusaLink, mock.ports[1]),
                ("Mock U1", Kind::SnapmakerU1, mock.ports[2]),
            ] {
                let mut p = Printer::new(kind);
                p.name = name.into();
                p.address = format!("127.0.0.1:{port}");
                if monkey.app.printers.put(None, p, None).is_ok() {
                    monkey.mock_printers += 1;
                }
            }
            monkey._mock = Some(mock);
        } else {
            eprintln!("monkey: mock_printer not built; printer actions run without printers");
        }
        // Start on a random one of the three printers.
        let p = monkey.rng.below(monkey.printers.len());
        monkey.switch_printer(p);
        Ok(monkey)
    }

    fn note(&mut self, text: String) {
        if self.limits.verbose {
            eprintln!("[{} #{}] {text}", self.seed, self.step);
        }
        if let Ok(mut t) = self.trace.lock() {
            t.push_back(format!("#{} {text}", self.step));
            while t.len() > TRACE_LEN {
                t.pop_front();
            }
        }
    }

    fn fail(&self, kind: &'static str, message: String) -> Failure {
        Failure {
            seed: self.seed,
            step: self.step,
            kind,
            message,
            trace: self
                .trace
                .lock()
                .map(|t| t.iter().cloned().collect())
                .unwrap_or_default(),
        }
    }

    fn map(&self) -> Vec<(usize, usize)> {
        self.app.scene_objects().1
    }

    fn handle(&mut self, action: Action) {
        let map = self.map();
        let ctx = self.ctx.clone();
        self.app.handle(action, &map, &ctx);
    }

    fn objects(&self) -> usize {
        self.app.plates.current().objects.len()
    }

    fn total_objects(&self) -> usize {
        self.app.plates.plates.iter().map(|p| p.objects.len()).sum()
    }

    fn random_object(&mut self) -> Option<usize> {
        let n = self.objects();
        (n > 0).then(|| self.rng.below(n))
    }

    fn switch_printer(&mut self, i: usize) {
        let (vendor, name) = self.printers[i].clone();
        self.note(format!("printer {name}"));
        // As the printer menus do (GUI monkey test, seed 3004: without it, a bed shape edited
        // on the XL stayed on the CORE One). Choosing the current printer changes nothing.
        if vendor != self.app.profiles.vendor || name != self.app.profiles.printer {
            self.app.switch_printer(vendor, &name);
        }
    }

    // ----- one step -----------------------------------------------------------------------

    fn step(&mut self) {
        let crowded = self.total_objects() > 14;
        let roll = self
            .rng
            .below(if self.limits.new_features { 108 } else { 100 });
        match roll {
            0..=3 if !crowded => self.open_model(),
            0..=3 => self.delete_some(),
            4..=9 => self.select(),
            10..=17 => self.transform(),
            18..=21 => self.tool_card(),
            22..=31 => self.command(crowded),
            32..=37 => self.undo_redo(),
            38..=45 => self.settings(),
            46..=47 => {
                let p = self.rng.below(self.printers.len());
                self.switch_printer(p);
            }
            48..=49 => self.filament(),
            50..=54 => self.plates(),
            55 => self.cut(),
            56 => self.emboss(),
            57 => self.mesh_tools(),
            58..=60 => self.paint(),
            61 => self.layer_heights(),
            62 => self.brim_ears(),
            63..=65 => self.tower(),
            66..=72 => self.slice(),
            73..=75 => self.preview(),
            76 => self.round_trip(),
            77..=85 => self.ui_frames(),
            86..=87 => self.view_tools(),
            88..=89 => self.library(),
            90..=91 => self.gesture_cancel(),
            92..=94 => self.dialogs(),
            95 => self.object_settings(),
            96..=98 => self.printers(),
            100..=107 => self.new_features(),
            _ => self.keys(),
        }
    }

    fn open_model(&mut self) {
        let path = self.rng.pick(&self.models).clone();
        self.note(format!(
            "open {}",
            path.file_name().unwrap_or_default().display()
        ));
        let result = crate::app::load_files(vec![path], &self.app.config, self.app.bed.center());
        if let Err(e) = &result {
            self.note(format!("  load error: {e}"));
        }
        self.app.finish_load(result);
        // A project's settings card: use, keep or ignore it.
        match self.rng.below(3) {
            0 if self.app.project.is_some() => {
                self.note("  use project settings".into());
                self.app.use_project_settings();
            }
            1 if self.app.project.is_some() => {
                self.note("  keep my settings".into());
                self.app.keep_my_settings();
            }
            _ => {}
        }
    }

    /// The Library tab over a temporary library (synthetic models in the monkey's folder):
    /// folders added, removed, taken offline and back, files added and deleted, search,
    /// filters, sorting, grid / list, selection, tags, favourites, opening models onto the
    /// plate. Checks the snapshot stays consistent and the worker alive.
    fn library(&mut self) {
        use crate::panels::library::Act;
        use mica_library::{Colours, Kind, Sliced, Sort};
        let root = self.dir.join("library").join("models");
        let second = self.dir.join("library").join("second");
        if self.app.library.lib.is_none() {
            let _ = mica_library::testkit::synthetic_library(&root, 24);
            let _ = mica_library::testkit::synthetic_range(&second, 500..508);
            let _ = mica_library::testkit::synthetic_downloads(&root, 0);
            let watch = self.rng.chance(0.5);
            self.app.library.open(
                &self.dir.join("library").join("data"),
                mica_library::Options {
                    watch,
                    threads: 2,
                    offline_check: Duration::from_millis(300),
                    debounce: Duration::from_millis(100),
                    ..Default::default()
                },
            );
            let ctx = self.ctx.clone();
            self.app.library_act(Act::AddFolder(root.clone()), &ctx);
            self.note(format!("library: open (watch {watch})"));
        }
        let ctx = self.ctx.clone();
        let visible: Vec<String> = {
            self.app.library.refresh();
            self.app.library.visible().map(|m| m.id.clone()).collect()
        };
        let pick = |rng: &mut Rng| (!visible.is_empty()).then(|| rng.pick(&visible).clone());
        let what = match self.rng.below(17) {
            0 => {
                self.app.library_act(Act::Rescan(None), &ctx);
                "rescan".to_string()
            }
            1 => {
                let t = *self
                    .rng
                    .pick(&["", "box", "gear 00", "toys", "3mf", "zzz", "prusa", "gifts"]);
                self.app.library.filter.text = t.into();
                format!("search {t:?}")
            }
            2 => {
                let v = &mut self.app.library;
                match self.rng.below(9) {
                    0 => v.toggle_kind(*self.rng.pick(&Kind::ALL)),
                    1 => {
                        v.filter.colours =
                            *self
                                .rng
                                .pick(&[Colours::Any, Colours::Some, Colours::Multi])
                    }
                    2 => v.filter.sliced = *self.rng.pick(&[Sliced::Any, Sliced::Yes, Sliced::No]),
                    3 => {
                        let s = *self.rng.pick(&crate::library::SIZES);
                        (v.filter.min_size_mm, v.filter.max_size_mm) = (s.1, s.2);
                    }
                    4 => v.filter.within_days = self.rng.pick(&crate::library::DATES).1,
                    5 => v.filter.favourites = !v.filter.favourites,
                    6 => v.filter.tag = self.rng.chance(0.5).then(|| "Workshop".to_string()),
                    7 => {
                        v.filter.printer = self
                            .rng
                            .chance(0.5)
                            .then(|| "Prusa CORE One HF0.4 nozzle".to_string())
                    }
                    _ => v.filter = Default::default(),
                }
                "filter".into()
            }
            3 => {
                self.app.library.sort = *self.rng.pick(&Sort::ALL);
                self.app.library.descending = self.rng.chance(0.5);
                format!("sort {:?}", self.app.library.sort)
            }
            4 => {
                self.app.library.list = !self.app.library.list;
                "grid / list".into()
            }
            5 => {
                let id = if self.rng.chance(0.8) {
                    pick(&mut self.rng)
                } else {
                    None
                };
                self.app.library_act(Act::Select(id), &ctx);
                "select".into()
            }
            6 => match pick(&mut self.rng) {
                Some(id) => {
                    self.app.library_act(Act::Favourite(id), &ctx);
                    "favourite".into()
                }
                None => "favourite (nothing shown)".into(),
            },
            7 => match pick(&mut self.rng) {
                Some(id) => {
                    let tag = self
                        .rng
                        .pick(&["Workshop", "gifts", " Gifts ", ""])
                        .to_string();
                    if self.rng.chance(0.6) {
                        self.app.library_act(Act::AddTag(id, tag.clone()), &ctx);
                    } else {
                        self.app.library_act(Act::RemoveTag(id, tag.clone()), &ctx);
                    }
                    format!("tag {tag:?}")
                }
                None => "tag (nothing shown)".into(),
            },
            8 if self.total_objects() <= 14 => match pick(&mut self.rng) {
                Some(id) => {
                    self.app.library_act(Act::Open(id), &ctx);
                    if self.app.library.open_prompt.is_some() {
                        let p = self.app.library.open_prompt.clone().unwrap().path;
                        let a = if self.rng.chance(0.6) {
                            Act::AddToPlate(p)
                        } else {
                            Act::ClosePrompt
                        };
                        self.app.library_act(a, &ctx);
                    }
                    "open from the library".into()
                }
                None => "open (nothing shown)".into(),
            },
            9 => {
                if let Some(id) = pick(&mut self.rng)
                    && let Some(m) = self.app.library.model(&id)
                {
                    let p = m.primary().path.clone();
                    self.app.library_act(Act::Reveal(p), &ctx);
                }
                "show in Finder".into()
            }
            10 => {
                // Change the (temporary) library folder itself.
                match self.rng.below(4) {
                    0 => {
                        let n = self.rng.below(1000);
                        let _ = std::fs::copy(
                            models_dir().join("cube20.stl"),
                            root.join(format!("Toys/added-{n}.stl")),
                        );
                        "library: file added".into()
                    }
                    1 => {
                        let victim = self
                            .app
                            .library
                            .snapshot()
                            .models
                            .first()
                            .map(|m| m.primary().path.clone());
                        if let Some(v) = victim.filter(|v| v.starts_with(&root)) {
                            let _ = std::fs::remove_file(v);
                        }
                        "library: file deleted".into()
                    }
                    2 => {
                        let away = self.dir.join("library").join("second-away");
                        if second.exists() {
                            let _ = std::fs::rename(&second, &away);
                            "library: second folder unmounted".into()
                        } else {
                            let _ = std::fs::rename(&away, &second);
                            "library: second folder back".into()
                        }
                    }
                    _ => {
                        self.app.library_act(Act::Rescan(None), &ctx);
                        "library: rescan".into()
                    }
                }
            }
            11 => {
                let folders: Vec<PathBuf> = self
                    .app
                    .library
                    .snapshot()
                    .folders
                    .iter()
                    .map(|f| f.path.clone())
                    .collect();
                if folders.len() > 1 && self.rng.chance(0.5) {
                    let f = self.rng.pick(&folders[1..]).clone();
                    self.app.library_act(Act::RemoveFolder(f), &ctx);
                    "library: folder removed".into()
                } else {
                    // A valid one, one inside another, a relative path and the disk root
                    // (refused in headless runs: only temporary folders are indexed).
                    let p = match self.rng.below(4) {
                        0 => second.clone(),
                        1 => root.join("Toys"),
                        2 => PathBuf::from("relative/folder"),
                        _ => PathBuf::from("/"),
                    };
                    self.app.library_act(Act::AddFolder(p.clone()), &ctx);
                    format!(
                        "library: add folder {} ({:?})",
                        p.display(),
                        self.app.library.notice
                    )
                }
            }
            13 => {
                let g = *self.rng.pick(&mica_library::GroupBy::ALL);
                self.app.library.group_by = g;
                format!("group by {g:?}")
            }
            14 => {
                // Into a project and out again.
                let folders: Vec<PathBuf> = self
                    .app
                    .library
                    .projects()
                    .list
                    .iter()
                    .map(|p| p.folder.clone())
                    .collect();
                let f = if self.app.library.open_project.is_some() || folders.is_empty() {
                    None
                } else {
                    Some(self.rng.pick(&folders).clone())
                };
                let what = format!("open project {f:?}");
                self.app.library_act(Act::OpenFolder(f), &ctx);
                if self.rng.chance(0.3)
                    && let Some(p) = self.app.library.current_project().cloned()
                {
                    let models = self.app.library.snapshot().models.clone();
                    let parts: Vec<PathBuf> = p
                        .plate_parts(&models)
                        .iter()
                        .map(|f| p.path(&models, *f).to_path_buf())
                        .collect();
                    if self.total_objects() + parts.len() <= 14 {
                        self.app.library_act(Act::AddAllToPlate(parts), &ctx);
                    }
                }
                what
            }
            15 => {
                // Split a project, make a folder one, or go back to automatic.
                let snap = self.app.library.snapshot().clone();
                let mut folders: Vec<PathBuf> = self
                    .app
                    .library
                    .projects()
                    .list
                    .iter()
                    .map(|p| p.folder.clone())
                    .collect();
                folders.extend(snap.grouping.keys().cloned());
                folders.extend(
                    snap.models
                        .iter()
                        .filter_map(|m| m.primary().path.parent().map(Path::to_path_buf)),
                );
                if folders.is_empty() {
                    "grouping (nothing)".into()
                } else {
                    let f = self.rng.pick(&folders).clone();
                    let g = *self.rng.pick(&[
                        Some(mica_library::Grouping::Split),
                        Some(mica_library::Grouping::Project),
                        None,
                    ]);
                    self.app.library_act(Act::SetGrouping(f.clone(), g), &ctx);
                    format!("grouping {} {g:?}", f.display())
                }
            }
            16 if self.total_objects() <= 14 => {
                // Drag one or two models onto the plate, or onto an object as parts.
                use crate::library_drop::{Target, part_kinds};
                let mut items = vec![];
                for _ in 0..1 + self.rng.below(2) {
                    if let Some(id) = pick(&mut self.rng)
                        && let Some(m) = self.app.library.model(&id).cloned()
                    {
                        items.extend(self.app.library.drag_items(&m));
                    }
                }
                let plate = self.app.plates.current;
                let target = match self.app.plates.current().objects.first() {
                    Some(o) if self.rng.chance(0.5) => {
                        let (a, b) = o.bounds();
                        Target::Part {
                            plate,
                            object: o.id,
                            at: (a + b) * 0.5,
                            screen: egui::pos2(600., 400.),
                        }
                    }
                    _ => Target::Plate {
                        plate,
                        at: self.rng.chance(0.5).then(|| Vec3::new(60., 70., 0.)),
                    },
                };
                self.app.library_drop(items, target, &ctx);
                if self.app.library.popover.is_some() && self.rng.chance(0.6) {
                    let kinds = part_kinds();
                    let kind = self.rng.pick(&kinds).0.clone();
                    self.app.set_dropped_part_kind(kind);
                }
                "drop from the library".into()
            }
            _ => {
                let f = self
                    .app
                    .library
                    .snapshot()
                    .folders
                    .first()
                    .map(|f| f.path.clone());
                self.app.library_act(Act::FolderFilter(f), &ctx);
                "library: folder filter".into()
            }
        };
        self.note(format!("library: {what}"));
        // Through the whole window, with the tab showing.
        self.app.library.shown = true;
        self.frame(vec![]);
        self.frame(vec![]);
        if let Err(e) = self.check_library() {
            self.problems.push(e);
        }
        if self.rng.chance(0.4) {
            self.app.library.shown = false;
        }
    }

    /// The library's snapshot is consistent and its worker is alive.
    fn check_library(&mut self) -> Result<(), String> {
        let Some(lib) = &self.app.library.lib else {
            return Ok(());
        };
        if !lib.alive() {
            return Err("library: the worker thread stopped".into());
        }
        if let Some(e) = lib.error() {
            return Err(format!("library: {e}"));
        }
        let snap = lib.snapshot();
        // The library stores canonical paths (`/private/var/…` for `/var/…`).
        let dir = std::fs::canonicalize(&self.dir).unwrap_or_else(|_| self.dir.clone());
        let mut paths = std::collections::HashSet::new();
        let mut ids = std::collections::HashSet::new();
        for m in &snap.models {
            if m.locations.is_empty() {
                return Err(format!("library: model {} has no location", m.name));
            }
            if !ids.insert(m.id.clone()) {
                return Err(format!("library: model {} twice", m.id));
            }
            for l in &m.locations {
                if !paths.insert(l.path.clone()) {
                    return Err(format!("library: {} in two models", l.path.display()));
                }
                if !l.path.starts_with(&dir) {
                    return Err(format!(
                        "library: indexed outside the test folder: {}",
                        l.path.display()
                    ));
                }
            }
            if let Some(t) = &m.thumbnail
                && !t.starts_with(dir.join("library").join("data"))
            {
                return Err(format!(
                    "library: thumbnail outside the data folder: {}",
                    t.display()
                ));
            }
        }
        if snap.files != paths.len() {
            return Err(format!(
                "library: {} files but {} locations",
                snap.files,
                paths.len()
            ));
        }
        self.app.library.refresh();
        if self.app.library.visible().count() > snap.models.len() {
            return Err("library: more shown than indexed".into());
        }
        // Projects: every file a real location, in at most one project, mapped back.
        let shown = self.app.library.snapshot().clone();
        let projects = self.app.library.projects();
        let mut in_project = std::collections::HashSet::new();
        for (i, p) in projects.list.iter().enumerate() {
            if p.files.is_empty() {
                return Err(format!("library: empty project {}", p.folder.display()));
            }
            for f in &p.files {
                let Some(m) = shown.models.get(f.model) else {
                    return Err("library: project file out of range".into());
                };
                let Some(l) = m.locations.get(f.location) else {
                    return Err("library: project location out of range".into());
                };
                if !l.path.starts_with(&p.folder) {
                    return Err(format!(
                        "library: {} outside its project {}",
                        l.path.display(),
                        p.folder.display()
                    ));
                }
                if !in_project.insert(l.path.clone()) {
                    return Err(format!("library: {} in two projects", l.path.display()));
                }
                if !projects.of_model[f.model].contains(&i) {
                    return Err("library: project missing from its model".into());
                }
            }
        }
        Ok(())
    }

    fn delete_some(&mut self) {
        self.note("crowded: select all + delete".into());
        let all: Vec<usize> = (0..self.objects()).collect();
        self.app.select_many(&all, false);
        self.app.delete_selected();
        if self.total_objects() > 14 {
            self.note("crowded: clear every plate".into());
            for i in (0..self.app.plates.len()).rev() {
                self.app.select_plate(i);
                self.app.plates.current_mut().locked = false;
                self.app.clear_plate();
            }
        }
    }

    fn select(&mut self) {
        let n = self.app.scene_objects().0.len();
        match self.rng.below(6) {
            0 => {
                self.note("select none".into());
                self.handle(Action::Select {
                    index: None,
                    toggle: false,
                });
            }
            1 | 2 => {
                let index = (n > 0).then(|| self.rng.below(n));
                let toggle = self.rng.chance(0.4);
                self.note(format!("select {index:?} toggle {toggle}"));
                self.handle(Action::Select { index, toggle });
            }
            3 => {
                let k = self.rng.below(4) + 1;
                let indices: Vec<usize> = (0..k).map(|_| self.rng.below(n.max(1))).collect();
                let toggle = self.rng.chance(0.3);
                self.note(format!("box select {indices:?} toggle {toggle}"));
                self.handle(Action::BoxSelect { indices, toggle });
            }
            4 => {
                self.note("select all".into());
                let ctx = self.ctx.clone();
                self.app.run_command(Command::SelectAll, &ctx);
            }
            _ => {
                // A volume or a height range of an object.
                let Some(i) = self.random_object() else {
                    return;
                };
                let o = &self.app.plates.current().objects[i];
                let mut subs: Vec<Option<Sub>> = vec![None];
                subs.extend(o.volumes.iter().map(|v| Some(Sub::Volume(v.id))));
                subs.extend(o.height_ranges.iter().map(|r| Some(Sub::Range(r.id))));
                let sub = *self.rng.pick(&subs);
                self.note(format!("select sub {i} {sub:?}"));
                self.handle(Action::SelectSub { index: i, sub });
            }
        }
    }

    fn random_delta(&mut self) -> Delta {
        match self.rng.below(3) {
            0 => Delta::Translate(self.rng.vec3(60.).with_z(if self.rng.chance(0.2) {
                self.rng.range(-10., 10.) as f32
            } else {
                0.
            })),
            1 => {
                let axis = match self.rng.below(4) {
                    0 => Vec3::X,
                    1 => Vec3::Y,
                    2 => Vec3::Z,
                    _ => self.rng.vec3(1.).normalize_or(Vec3::Z),
                };
                Delta::Rotate {
                    axis,
                    angle: self.rng.range(-4., 4.) as f32,
                }
            }
            _ => {
                let f = self.rng.range(0.4, 2.2) as f32;
                Delta::Scale(if self.rng.chance(0.6) {
                    Vec3::splat(f)
                } else {
                    Vec3::new(
                        self.rng.range(0.5, 1.8) as f32,
                        self.rng.range(0.5, 1.8) as f32,
                        self.rng.range(0.5, 1.8) as f32,
                    )
                })
            }
        }
    }

    /// Gizmo drags: a gesture with several deltas, or one bare nudge.
    fn transform(&mut self) {
        let n = self.app.scene_objects().0.len();
        if n == 0 {
            return;
        }
        let index = self.rng.below(n);
        self.app.viewport.mode = *self.rng.pick(&[
            crate::viewport::gizmo::Mode::Move,
            crate::viewport::gizmo::Mode::Rotate,
            crate::viewport::gizmo::Mode::Scale,
            crate::viewport::gizmo::Mode::None,
        ]);
        match self.rng.below(4) {
            0 => {
                let delta = self.random_delta();
                self.note(format!("nudge {index} {delta:?}"));
                self.handle(Action::Transform { index, delta });
            }
            1 => {
                let euler = self.rng.vec3(std::f64::consts::PI * 2.);
                self.note(format!("set rotation {index} {euler:?}"));
                self.handle(Action::SetRotation { index, euler });
                self.handle(Action::Edited);
            }
            2 => {
                let faces = self
                    .app
                    .scene_objects()
                    .0
                    .get(index)
                    .map_or(0, |o| o.mesh.faces.len());
                let face = self.rng.below(faces + 2);
                // Half the time one of the object's flat planes, as the tool offers them.
                let object = self.app.scene_objects().0.get(index).cloned();
                let plane = object.filter(|_| self.rng.below(2) == 0).and_then(|o| {
                    self.app.viewport.planes.blocking = true;
                    let planes = self.app.viewport.planes.planes(&o, &self.ctx)?;
                    let pick = self.rng.below(planes.len() + 1);
                    let p = planes.get(pick)?;
                    let n = o.rotation() * glam::Vec3::from(p.normal.map(|c| c as f32));
                    Some((pick, n))
                });
                match plane {
                    Some((pick, normal)) => {
                        self.note(format!("lay {index} on plane {pick} {normal:?}"));
                        self.handle(Action::LayOnPlane { index, normal });
                    }
                    None => {
                        self.note(format!("lay {index} on face {face}"));
                        self.handle(Action::LayOnFace { index, face });
                    }
                }
            }
            _ => {
                let frames = self.rng.below(6) + 1;
                let delta = self.random_delta();
                self.note(format!("drag {index} {frames}× {delta:?}"));
                self.handle(Action::GestureBegin("Move"));
                for _ in 0..frames {
                    self.handle(Action::Transform { index, delta });
                }
                if self.rng.chance(0.15) {
                    self.note("  Esc".into());
                    self.handle(Action::GestureCancel);
                } else {
                    self.handle(Action::Edited);
                }
            }
        }
    }

    /// Typed or dragged tool-card values, up to the fields' limits.
    fn tool_card(&mut self) {
        use crate::panels::objects::Transform;
        let Some(i) = self.app.selected.filter(|&i| i < self.objects()) else {
            if let Some(i) = self.random_object() {
                self.app.select(Some(i), false);
            }
            return;
        };
        let before = Transform::of(&self.app.plates.current().objects[i]);
        let mut t = before;
        let axis = self.rng.below(3);
        let extreme = self.rng.chance(0.15);
        let mut keep_on_bed = false;
        let what = match self.rng.below(6) {
            0 => {
                let v = if extreme {
                    *self.rng.pick(&[-1e6, 1e6, 0.])
                } else {
                    self.rng.range(-50., 300.) as f32
                };
                t.set_position(axis, v);
                format!("position[{axis}] = {v}")
            }
            1 => {
                let v = if extreme {
                    *self.rng.pick(&[-1e6, 1e6, 720.])
                } else {
                    self.rng.range(-180., 180.) as f32
                };
                t.set_rotation_degrees(axis, v);
                keep_on_bed = true;
                format!("rotation[{axis}] = {v}°")
            }
            2 => {
                let v = if extreme {
                    *self.rng.pick(&[0.01, 1e6, 0.5])
                } else {
                    self.rng.range(2., 200.) as f32
                };
                let uniform = self.rng.chance(0.6);
                t.set_size(axis, v, uniform);
                keep_on_bed = true;
                format!("size[{axis}] = {v} mm uniform {uniform}")
            }
            3 => {
                let v = if extreme {
                    *self.rng.pick(&[0.1, 1e6, 1.])
                } else {
                    self.rng.range(20., 300.) as f32
                };
                let uniform = self.rng.chance(0.6);
                t.set_scale_percent(axis, v, uniform);
                keep_on_bed = true;
                format!("scale[{axis}] = {v} % uniform {uniform}")
            }
            4 => {
                t.toggle_mirror(axis);
                keep_on_bed = true;
                format!("mirror[{axis}]")
            }
            _ => {
                t.reset();
                keep_on_bed = true;
                "reset transform".into()
            }
        };
        self.note(format!("tool card {i}: {what}"));
        if t != before {
            self.app.apply_card_transform(i, before, t, keep_on_bed);
        }
        // The pointer is released: the field's gesture ends.
        self.app.end_gesture();
    }

    fn command(&mut self, crowded: bool) {
        let shape = *self.rng.pick(&Shape::ALL);
        let tools = self.app.tool_count();
        let plates = self.app.plates.len();
        let mut commands = vec![
            Command::Copy,
            Command::Delete,
            Command::DeleteSub,
            Command::RemoveInstance,
            Command::Centre,
            Command::DropToBed,
            Command::Mirror(self.rng.below(3)),
            Command::Rotate90,
            Command::ChangeTool(self.rng.below(tools + 2)),
            Command::AddModifier(shape),
            Command::AddNegative(shape),
            Command::AddSupportEnforcer(shape),
            Command::AddSupportBlocker(shape),
            Command::AddHeightRange,
            Command::SplitToParts,
            Command::Rename,
            Command::ObjectSettings,
            Command::Arrange,
            Command::ArrangeAll,
            Command::SelectAll,
            Command::AutoOrient,
            Command::ExportStl,
            Command::MeshBoolean,
            Command::Simplify,
            Command::Repair,
        ];
        if !crowded {
            commands.extend([
                Command::Paste,
                Command::Duplicate,
                Command::AddInstance,
                Command::SplitToObjects,
                Command::MoveToPlate(self.rng.below(plates + 1)),
            ]);
        }
        let cmd = self.rng.pick(&commands).clone();
        let index = if self.rng.chance(0.5) {
            let n = self.app.scene_objects().0.len();
            (n > 0).then(|| self.rng.below(n))
        } else {
            None
        };
        self.note(format!("command {cmd:?} on {index:?}"));
        self.handle(Action::Command {
            index,
            command: cmd,
        });
        if self.app.renaming.is_some() && self.rng.chance(0.5) {
            self.app.renaming = None;
        }
    }

    fn undo_redo(&mut self) {
        let n = self.rng.below(8) + 1;
        let undo = self.rng.chance(0.6);
        self.note(format!("{} ×{n}", if undo { "undo" } else { "redo" }));
        for _ in 0..n {
            if undo {
                self.app.undo();
            } else {
                self.app.redo();
            }
        }
    }

    fn settings(&mut self) {
        let tools = self.app.tool_count();
        let (target, keys) = match self.rng.below(10) {
            0..=5 => (Target::Print, &self.print_keys),
            6..=7 => (Target::Filament(self.rng.below(tools)), &self.filament_keys),
            _ => (Target::Printer, &self.printer_keys),
        };
        if keys.is_empty() {
            return;
        }
        let key = *self.rng.pick(keys);
        if let Target::Filament(t) = target
            && self.rng.chance(0.15)
        {
            // A colour: project state that keeps the slices (see `slice_cache`).
            let raw = format!("#{:06X}", self.rng.below(0x100_0000));
            self.note(format!("colour T{} = {raw}", t + 1));
            self.app.apply_edit(
                Some(target),
                Edit::Set {
                    key: "filament_colour".into(),
                    raw,
                },
            );
            self.app.end_gesture();
            return;
        }
        if self.rng.chance(0.15) {
            self.note(format!("reset {target:?} {key}"));
            self.app.apply_edit(Some(target), Edit::Reset(key.into()));
        } else {
            let Some(raw) = random_value(&mut self.rng, key) else {
                return;
            };
            if matches!(target, Target::Printer) {
                self.app.editor.printer_extruder = self.rng.below(tools);
            }
            self.note(format!("set {target:?} {key} = {raw:?}"));
            self.app.apply_edit(
                Some(target),
                Edit::Set {
                    key: key.into(),
                    raw,
                },
            );
        }
        self.app.end_gesture();
    }

    /// Settings of the current plate (`plate_settings`): its plate options or any setting.
    fn plate_settings(&mut self) {
        let key = if self.rng.chance(0.5) {
            *self.rng.pick(&crate::plate_settings::PLATE_OPTIONS)
        } else {
            *self.rng.pick(&self.object_keys)
        };
        let raw = if key == crate::plate_settings::BED_TYPE {
            Some(
                self.rng
                    .pick(&crate::panels::profiles::SNAPMAKER_BEDS)
                    .to_string(),
            )
        } else {
            random_value(&mut self.rng, key)
        };
        let Some(raw) = raw else {
            return;
        };
        self.app.editor.plate_mode = true;
        if self.rng.chance(0.25) {
            self.note(format!("plate setting reset {key}"));
            self.app.apply_plate_edit(Edit::Reset(key.into()));
        } else {
            self.note(format!("plate setting {key} = {raw:?}"));
            self.app.apply_plate_edit(Edit::Set {
                key: key.into(),
                raw,
            });
        }
        self.app.end_gesture();
    }

    /// Settings of the selected object, volume or height range.
    fn object_settings(&mut self) {
        if self.rng.chance(0.25) {
            self.plate_settings();
            return;
        }
        if self.app.selected.is_none() {
            return;
        }
        let key = *self.rng.pick(&self.object_keys);
        let Some(raw) = random_value(&mut self.rng, key) else {
            return;
        };
        self.note(format!(
            "object setting {key} = {raw:?} (sub {:?})",
            self.app.sub
        ));
        if self.rng.chance(0.2) {
            self.app.apply_edit(None, Edit::Reset(key.into()));
        } else {
            self.app.apply_edit(
                None,
                Edit::Set {
                    key: key.into(),
                    raw,
                },
            );
        }
        self.app.end_gesture();
    }

    fn filament(&mut self) {
        let materials = self.app.profiles.materials.clone();
        if materials.is_empty() || self.app.profiles.filaments.is_empty() {
            return;
        }
        let slot = self.rng.below(self.app.profiles.filaments.len());
        let m = self.rng.pick(&materials).clone();
        self.note(format!("filament {} = {m}", slot + 1));
        self.app.profiles.filaments[slot] = m;
        self.app.profiles.refresh();
        self.app.settings_changed();
    }

    fn plates(&mut self) {
        let n = self.app.plates.len();
        let i = self.rng.below(n + 1);
        use crate::plate_menu::PlateCommand as C;
        let action = match self.rng.below(11) {
            0 if n < 6 => Action::AddPlate,
            1 => Action::RemovePlate(i),
            2 | 3 => Action::SelectPlate(i),
            4 => Action::PlateArrange(i),
            5 => Action::PlateOrient(i),
            // The plate menu: rename (empty: the number again), move, duplicate.
            6 => Action::RenamePlate(
                i,
                self.rng
                    .pick(&["", "Gift", "Plate 9", "  Brackets / PETG  "])
                    .to_string(),
            ),
            7 => Action::MovePlate {
                from: i,
                to: self.rng.below(n + 1),
            },
            8 if n < 6 => Action::Plate(i, C::Duplicate),
            9 => Action::Plate(
                i,
                if self.rng.below(2) == 0 {
                    C::MoveLeft
                } else {
                    C::MoveRight
                },
            ),
            _ => Action::PlateLock(i),
        };
        self.note(format!("plates: {action:?}"));
        self.handle(action);
    }

    /// Mesh tools: the boolean and simplify cards (open, run, close) and repair, on small
    /// meshes so a step stays quick.
    fn mesh_tools(&mut self) {
        use crate::mesh_tools::{MeshTool, Other, SimplifyParams};
        use mica_core::mesh_boolean::BooleanOp;
        const SMALL: usize = 30_000;
        if self.app.selected.is_none()
            && let Some(i) = self.random_object()
        {
            self.app.select(Some(i), false);
        }
        let Some(i) = self.app.selected else { return };
        let faces = |app: &App, k: usize| app.plates.current().objects[k].mesh.faces.len();
        match self.rng.below(6) {
            0 => {
                self.note("mesh boolean toggle".into());
                self.handle(Action::Command {
                    index: None,
                    command: Command::MeshBoolean,
                });
            }
            1 | 2 => {
                let choices = self.app.boolean_choices(i);
                if choices.is_empty() || faces(&self.app, i) > SMALL {
                    return;
                }
                let (other, _) = choices[self.rng.below(choices.len())];
                if let Other::Object(id) = other
                    && let Some(k) = self
                        .app
                        .plates
                        .current()
                        .objects
                        .iter()
                        .position(|o| o.id == id)
                    && faces(&self.app, k) > SMALL
                {
                    return;
                }
                let op = *self.rng.pick(&BooleanOp::ALL);
                let (keep_original, keep_other) = (self.rng.chance(0.3), self.rng.chance(0.5));
                self.note(format!(
                    "mesh boolean {op:?} with {other:?} keep original {keep_original} other {keep_other}"
                ));
                if let Err(e) = self.app.boolean_now(op, other, keep_original, keep_other) {
                    self.note(format!("  boolean refused: {e}"));
                }
            }
            3 => {
                if faces(&self.app, i) > SMALL * 4 {
                    return;
                }
                let ratio = self.rng.range(5., 95.) as f32;
                self.note(format!("simplify {ratio:.0} %"));
                self.app.simplify_now(ratio);
            }
            4 => {
                if faces(&self.app, i) > SMALL * 4 {
                    return;
                }
                self.note("simplify card".into());
                self.app.toggle_simplify();
                if let Some(MeshTool::Simplify(s)) = &mut self.app.mesh_tool {
                    s.params = SimplifyParams {
                        use_count: self.rng.chance(0.6),
                        level: self.rng.below(5),
                        ratio: self.rng.range(0., 99.) as f32,
                    };
                }
                let ctx = egui::Context::default();
                for _ in 0..3 {
                    self.app.poll_simplify(&ctx);
                }
                if self.rng.chance(0.6) {
                    self.note("  apply".into());
                    self.app.apply_simplify();
                } else {
                    self.app.mesh_tool = None;
                }
            }
            _ => {
                self.note("repair mesh".into());
                self.handle(Action::Command {
                    index: None,
                    command: Command::Repair,
                });
            }
        }
    }

    /// The Text / SVG tool: open / close, place on the model, edit text and style, change the
    /// type, swap in an SVG, wrap onto the surface, delete.
    fn emboss(&mut self) {
        // Open the tool (or toggle it), then a few edits on what it opened.
        if self.app.emboss_target().is_none() || self.rng.chance(0.15) {
            match self.rng.below(3) {
                0 => self.app.select(None, false),
                _ => {
                    if let Some(i) = self.random_object() {
                        self.app.select(Some(i), false);
                        // Sometimes one of its texts: the tool opens on it again.
                        let texts: Vec<u64> = self.app.plates.current().objects[i]
                            .volumes
                            .iter()
                            .filter(|v| v.emboss.is_some())
                            .map(|v| v.id)
                            .collect();
                        if !texts.is_empty() && self.rng.chance(0.5) {
                            let id = texts[self.rng.below(texts.len())];
                            self.note(format!("emboss reopen text {id}"));
                            self.app.sub = Some(crate::scene::Sub::Volume(id));
                        }
                    }
                }
            }
            self.note("emboss tool toggle".into());
            self.handle(Action::Emboss);
        }
        for _ in 0..1 + self.rng.below(3) {
            if self.app.emboss_target().is_none() {
                break;
            }
            let roll = 1 + self.rng.below(8);
            self.emboss_edit(roll);
        }
    }

    fn emboss_edit(&mut self, roll: usize) {
        use mica_core::emboss::{EmbossSource, HAlign};
        let Some((i, _)) = self.app.emboss_target() else {
            self.app.close_emboss();
            return;
        };
        match roll {
            1 => {
                let (min, max) = self.app.plates.current().objects[i].bounds();
                let (point, normal) = match self.rng.below(3) {
                    0 => (
                        Vec3::new((min.x + max.x) / 2., (min.y + max.y) / 2., max.z),
                        Vec3::Z,
                    ),
                    1 => (
                        Vec3::new((min.x + max.x) / 2., min.y, (min.z + max.z) / 2.),
                        -Vec3::Y,
                    ),
                    _ => (
                        Vec3::new(max.x, (min.y + max.y) / 2., (min.z + max.z) / 2.),
                        Vec3::X,
                    ),
                };
                self.note(format!("emboss place {point} {normal}"));
                self.handle(Action::EmbossPlace {
                    index: i,
                    point,
                    normal,
                });
            }
            2 => {
                let text = self
                    .rng
                    .pick(&[
                        "Mica",
                        "Hello\nworld",
                        "Åé%&@ 3D",
                        "",
                        "  ",
                        "A\u{10FFFD}B",
                        "x",
                    ])
                    .to_string();
                self.note(format!("emboss text {text:?}"));
                self.app.edit_emboss(
                    move |c| {
                        if let EmbossSource::Text(t) = &mut c.source {
                            t.text = text;
                        }
                    },
                    false,
                );
            }
            3 => {
                let (size, depth, gap) = (
                    self.rng.range(0.5, 40.),
                    self.rng.range(0.05, 6.),
                    self.rng.range(-200., 400.) as i32,
                );
                let align = *self
                    .rng
                    .pick(&[HAlign::Left, HAlign::Center, HAlign::Right]);
                let bold = self.rng.chance(0.5);
                self.note(format!(
                    "emboss style size {size:.2} depth {depth:.2} gap {gap} {align:?} bold {bold}"
                ));
                self.app.edit_emboss(
                    move |c| {
                        c.depth = depth;
                        if let EmbossSource::Text(t) = &mut c.source {
                            t.size_mm = size;
                            t.char_gap = gap;
                            t.line_gap = gap / 2;
                            t.h_align = align;
                            t.bold = bold;
                        }
                    },
                    true,
                );
            }
            4 => {
                let kind = *self.rng.pick(&[
                    crate::emboss::Kind::Raised,
                    crate::emboss::Kind::Engraved,
                    crate::emboss::Kind::Modifier,
                ]);
                self.note(format!("emboss kind {kind:?}"));
                self.app.set_emboss_kind(kind);
            }
            5 => {
                self.note("emboss svg".into());
                self.app.emboss_svg_data(
                    "monkey.svg",
                    r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 40 40"><path fill-rule="evenodd" d="M0 0h40v40H0z M10 10h20v20H10z"/><circle cx="20" cy="20" r="5"/></svg>"#.into(),
                );
            }
            6 => {
                let on = self.rng.chance(0.6);
                self.note(format!("emboss wrap {on}"));
                self.app.edit_emboss(move |c| c.use_surface = on, false);
            }
            7 => {
                let mirror = [self.rng.chance(0.5), self.rng.chance(0.5)];
                self.note(format!("emboss svg mirror {mirror:?}"));
                self.app.edit_emboss(
                    move |c| {
                        if let EmbossSource::Svg(s) = &mut c.source {
                            s.mirror = mirror;
                            s.tolerance_mm = if mirror[0] { 0.02 } else { 0.1 };
                        }
                    },
                    false,
                );
            }
            _ => {
                if self.rng.chance(0.5) {
                    self.note("emboss done".into());
                    self.app.close_emboss();
                } else {
                    self.note("emboss cancel".into());
                    self.app.cancel_emboss();
                }
            }
        }
    }

    fn cut(&mut self) {
        match self.rng.below(5) {
            3 => self.cut_connector(),
            0 => {
                if self.app.selected.is_none()
                    && let Some(i) = self.random_object()
                {
                    self.app.select(Some(i), false);
                }
                self.note("cut tool toggle".into());
                self.handle(Action::Cut);
            }
            1 => {
                let delta = match self.rng.below(2) {
                    0 => Delta::Translate(Vec3::Z * self.rng.range(-15., 15.) as f32),
                    _ => Delta::Rotate {
                        axis: if self.rng.chance(0.5) {
                            Vec3::X
                        } else {
                            Vec3::Y
                        },
                        angle: self.rng.range(-1., 1.) as f32,
                    },
                };
                self.note(format!("cut plane {delta:?}"));
                self.handle(Action::CutPlane(delta));
            }
            2 => {
                if let Some(c) = &mut self.app.cut {
                    let z = self.rng.range(-5., 80.) as f32;
                    c.set_z(z);
                    self.note(format!("cut z = {z}"));
                }
            }
            _ => {
                if self.app.cut.is_some() {
                    self.note("cut apply".into());
                    self.app.apply_cut();
                }
            }
        }
    }

    /// Cut connectors: place one by a click on the plane (on or off the cut face), change
    /// the type, shape and sizes (degenerate ones too), delete, clear, toggle the mode.
    fn cut_connector(&mut self) {
        use mica_core::cut_connectors::{
            Attributes, ConnectorShape, ConnectorStyle, ConnectorType, Sizes,
        };
        let Some(c) = self.app.cut.clone() else {
            return;
        };
        match self.rng.below(5) {
            0 | 1 => {
                let (x, y) = c.axes();
                let r = c.extent;
                let p = c.center
                    + x * self.rng.range(-r as f64, r as f64) as f32
                    + y * self.rng.range(-r as f64, r as f64) as f32;
                self.note(format!("cut connector click {p:?}"));
                self.handle(Action::CutConnector(crate::cut::ConnectorEdit::Click(p)));
            }
            2 => {
                let a = Attributes {
                    kind: *self.rng.pick(&ConnectorType::ALL),
                    style: *self.rng.pick(&ConnectorStyle::ALL),
                    shape: *self.rng.pick(&ConnectorShape::ALL),
                };
                let s = Sizes {
                    radius: self.rng.range(0.1, 30.) as f32,
                    height: self.rng.range(0.1, 60.) as f32,
                    radius_tolerance: self.rng.range(0., 2.) as f32,
                    height_tolerance: self.rng.range(0., 2.) as f32,
                    angle: self.rng.range(0., 3.2) as f32,
                };
                self.note(format!("cut connector set {a:?} {s:?}"));
                if let Some(c) = &mut self.app.cut {
                    c.set_connector(a, s);
                }
            }
            3 => {
                let edit = if self.rng.chance(0.7) {
                    crate::cut::ConnectorEdit::DeleteSelected
                } else {
                    crate::cut::ConnectorEdit::Clear
                };
                self.note(format!("cut connector {edit:?}"));
                self.handle(Action::CutConnector(edit));
            }
            _ => {
                self.note("cut connector mode toggle".into());
                if let Some(c) = &mut self.app.cut {
                    c.editing = !c.editing;
                }
            }
        }
    }

    /// A paint stroke of a random kind and brush at a face hit by a ray from a random side.
    fn paint(&mut self) {
        let Some(i) = self.random_object() else {
            return;
        };
        let kinds = [
            PaintKind::Supports,
            PaintKind::Seam,
            PaintKind::Material,
            PaintKind::Fuzzy,
            PaintKind::Texture,
        ];
        let kind = *self.rng.pick(&kinds);
        match self.rng.below(8) {
            0 => {
                self.note(format!("paint tool {kind:?}"));
                self.app.select(Some(i), false);
                self.app.viewport.paint = Some(kind);
                self.handle(Action::PaintTool(Some(kind)));
                return;
            }
            1 => {
                self.note("paint tool off".into());
                self.app.viewport.paint = None;
                self.handle(Action::PaintTool(None));
                return;
            }
            2 => {
                self.note("paint overhangs".into());
                self.app.select(Some(i), false);
                self.app.paint.overhang_angle = self.rng.range(0., 90.) as f32;
                self.app.paint_overhangs();
                return;
            }
            3 if self.app.viewport.paint.is_some() => {
                self.note("clear painting".into());
                self.app.clear_painting();
                return;
            }
            _ => {}
        }
        let (objects, map) = self.app.scene_objects();
        let Some(index) = map
            .iter()
            .position(|&(p, o)| p == self.app.plates.current && o == i)
        else {
            return;
        };
        let o = &objects[index];
        let (min, max) = o.bounds();
        let centre = (min + max) * 0.5;
        let dir = self.rng.vec3(1.).normalize_or(Vec3::NEG_Z);
        let reach = (max - min).length().max(1.) * 2.;
        let camera = centre - dir * reach;
        let Some(hit) = crate::scene::picking::pick_face(std::slice::from_ref(o), camera, dir)
        else {
            return;
        };
        let point = camera + dir * hit.distance;
        self.app.viewport.paint = Some(kind);
        self.app.paint.brush = *self.rng.pick(&crate::paint::BrushKind::ALL);
        let r = self.rng.range(0., 30.) as f32;
        self.app.paint.set_radius(r);
        self.app.paint.tool = self.rng.below(6);
        self.app.paint.enforce = self.rng.chance(0.7);
        self.app.paint.fill_angle = self.rng.range(0., 90.) as f32;
        let spread = self.rng.range(-5., 40.) as f32;
        self.app.paint.set_fill_spread(spread);
        self.app.paint.height = self.rng.range(0.1, 10.) as f32;
        self.app.paint.show_overhangs = self.rng.chance(0.3);
        self.app.paint.only_overhangs = self.rng.chance(0.3);
        let erase = self.rng.chance(0.2);
        let frames = self.rng.below(5) + 1;
        self.note(format!(
            "paint {kind:?} {:?} r {r:.1} on {i} face {} ×{frames} erase {erase}",
            self.app.paint.brush, hit.face
        ));
        self.handle(Action::GestureBegin(crate::paint::tool_name(kind)));
        // Later frames are fast-drag segments: a capsule from the previous frame's point.
        let mut previous: Option<crate::paint::Hit> = None;
        for k in 0..frames {
            let jitter = self.rng.vec3(2.) * k as f32;
            let hit = crate::paint::Hit {
                face: hit.face,
                point: point + jitter,
                camera,
            };
            self.handle(Action::Paint {
                index,
                hit,
                trail: previous.into_iter().collect(),
                erase,
            });
            previous = Some(hit);
        }
        self.handle(Action::Edited);
    }

    /// The variable layer height tool: switching it, its buttons and drags on the object.
    fn layer_heights(&mut self) {
        use crate::viewport::ObjectTool;
        let Some(i) = self.random_object() else {
            return;
        };
        self.app.select(Some(i), false);
        let Some(index) = self.scene_index(i) else {
            return;
        };
        match self.rng.below(8) {
            0 => {
                self.note("layer heights tool".into());
                self.app.viewport.object_tool = Some(ObjectTool::LayerHeights);
                self.handle(Action::ObjectTool(Some(ObjectTool::LayerHeights)));
            }
            1 => {
                self.note("object tool off".into());
                self.app.viewport.object_tool = None;
                self.handle(Action::ObjectTool(None));
            }
            2 => {
                let q = self.rng.unit() as f32;
                self.note(format!("adaptive layers {q:.2} on {i}"));
                self.app.layer_edit.quality = q;
                self.app.layer_adaptive();
            }
            3 => {
                let r = self.rng.below(12) as u32;
                let keep = self.rng.chance(0.5);
                self.note(format!("smooth layers r {r} keep {keep} on {i}"));
                self.app.layer_edit.smooth_radius = r.clamp(1, 10);
                self.app.layer_edit.keep_min = keep;
                self.app.layer_smooth();
            }
            4 => {
                self.note(format!("reset layers on {i}"));
                self.app.layer_reset();
            }
            5 => {
                let w = self.rng.range(0., 15.) as f32;
                self.note(format!("band width {w:.1}"));
                self.app.layer_edit.set_band_width(w);
            }
            _ => {
                let h = self.app.plates.current().objects[i].bounds();
                let height = f64::from(h.1.z - h.0.z);
                let z = self.rng.range(-2., height + 2.);
                let frames = self.rng.below(6) + 1;
                let (shift, alt) = (self.rng.chance(0.2), self.rng.chance(0.2));
                self.note(format!(
                    "layer drag on {i} at z {z:.1} ×{frames} shift {shift} alt {alt}"
                ));
                self.handle(Action::GestureBegin("Layer heights"));
                for _ in 0..frames {
                    let amount = self.rng.range(-0.08, 0.08);
                    self.handle(Action::LayerEdit {
                        index,
                        z,
                        amount,
                        shift,
                        alt,
                    });
                }
                self.handle(Action::Edited);
            }
        }
    }

    /// The brim ears tool: placing, removing, auto-placing and the painted brim type.
    fn brim_ears(&mut self) {
        use crate::viewport::ObjectTool;
        let Some(i) = self.random_object() else {
            return;
        };
        self.app.select(Some(i), false);
        let Some(index) = self.scene_index(i) else {
            return;
        };
        let (min, max) = self.app.plates.current().objects[i].bounds();
        match self.rng.below(7) {
            0 => {
                self.note("brim ears tool".into());
                self.app.viewport.object_tool = Some(ObjectTool::BrimEars);
                self.handle(Action::ObjectTool(Some(ObjectTool::BrimEars)));
            }
            1 => {
                self.app.brim_ear.max_angle = self.rng.range(0., 180.) as f32;
                self.app.brim_ear.detection = self.rng.range(0., 5.) as f32;
                self.note(format!(
                    "auto ears {:.0}° {:.1} mm on {i}",
                    self.app.brim_ear.max_angle, self.app.brim_ear.detection
                ));
                self.app.auto_place_ears();
            }
            2 => {
                self.note(format!("painted brim on {i}"));
                self.app.record("Use placed brim ears");
                self.app.plates.current_mut().objects[i]
                    .overrides
                    .insert("brim_type".into(), "painted".into());
                self.app.sync_instances(i);
                self.app.changed();
            }
            3 => {
                let n = self.app.plates.current().objects[i].brim_ears.len();
                if n > 0 {
                    let ear = self.rng.below(n + 1);
                    self.note(format!("remove ear {ear} of {i}"));
                    self.handle(Action::RemoveBrimEar { index, ear });
                }
            }
            _ => {
                let at = glam::Vec2::new(
                    self.rng.range(f64::from(min.x) - 5., f64::from(max.x) + 5.) as f32,
                    self.rng.range(f64::from(min.y) - 5., f64::from(max.y) + 5.) as f32,
                );
                let d = self.rng.range(1., 25.) as f32;
                self.app.brim_ear.set_diameter(d);
                self.note(format!("add ear on {i} at {at:?} ⌀ {d:.1}"));
                self.handle(Action::AddBrimEar { index, at });
            }
        }
    }

    /// The scene index of current-plate object `i` (the viewport's numbering).
    fn scene_index(&self, i: usize) -> Option<usize> {
        let (_, map) = self.app.scene_objects();
        map.iter()
            .position(|&(p, o)| p == self.app.plates.current && o == i)
    }

    fn tower(&mut self) {
        match self.rng.below(5) {
            0 => {
                let plate = self.app.plates.current;
                let p = [
                    self.rng.range(-50., 400.),
                    self.rng.range(-50., 400.),
                    self.rng.range(-90., 90.),
                ];
                self.note(format!("tower placement {p:?}"));
                self.app.set_tower_placement(plate, p);
                self.app.changed();
            }
            1 | 2 => {
                let delta = match self.rng.below(2) {
                    0 => Delta::Translate(self.rng.vec3(40.).with_z(0.)),
                    _ => Delta::Rotate {
                        axis: Vec3::Z,
                        angle: self.rng.range(-2., 2.) as f32,
                    },
                };
                let frames = self.rng.below(4) + 1;
                self.note(format!("tower drag {frames}× {delta:?}"));
                self.handle(Action::SelectTower);
                self.handle(Action::GestureBegin("Move prime tower"));
                for _ in 0..frames {
                    self.handle(Action::TowerTransform(delta));
                }
                self.handle(Action::Edited);
            }
            3 => {
                let axis = self.rng.below(3);
                let v = self.rng.range(-100., 400.);
                self.note(format!("tower value {axis} = {v}"));
                if self.app.tower().is_some() {
                    self.app.select_tower();
                    self.app.set_tower_value(axis, v);
                    self.app.end_gesture();
                }
            }
            _ => {
                let cmd = *self.rng.pick(&[
                    crate::tower::TowerCommand::FreeSpot,
                    crate::tower::TowerCommand::Disable,
                    crate::tower::TowerCommand::Settings,
                ]);
                self.note(format!("tower command {cmd:?}"));
                self.handle(Action::TowerCommand(cmd));
            }
        }
    }

    fn slice(&mut self) {
        let ctx = self.ctx.clone();
        match self.rng.below(8) {
            0..=4 => {
                if self.rng.chance(0.8) {
                    self.note("slice plate".into());
                    self.app.slice_current(&ctx);
                } else {
                    self.note("slice all".into());
                    self.app.slice_all(&ctx);
                }
                match self.rng.below(4) {
                    // Watch it finish, or cancel it part way.
                    0 | 1 => self.wait_for_slice(None),
                    2 => {
                        let after = Duration::from_millis(self.rng.below(3000) as u64);
                        self.wait_for_slice(Some(after));
                    }
                    _ => {}
                }
            }
            5 => {
                self.note("cancel slice".into());
                if self.app.worker.is_some() {
                    self.stats.cancels += 1;
                }
                self.app.cancel_slice();
            }
            _ => self.wait_for_slice(None),
        }
    }

    /// Waits for the running job like a user watching the progress bar: until it finishes,
    /// or cancels it after `cancel_after` (and always after the slice budget).
    fn wait_for_slice(&mut self, cancel_after: Option<Duration>) {
        let cancel_after = cancel_after.or(self.limits.cancel_after);
        self.note(match cancel_after {
            Some(d) => format!("  wait {} ms, then cancel", d.as_millis()),
            None => "  wait for the slice".into(),
        });
        let started = Instant::now();
        let limit = cancel_after.unwrap_or(self.limits.slice_wait);
        while self.app.worker.is_some()
            || self.app.pending_slice.is_some()
            || !self.app.slice_queue.is_empty()
        {
            self.poll();
            if started.elapsed() > limit {
                if cancel_after.is_none() {
                    self.note("  slow: cancel".into());
                }
                self.stats.cancels += 1;
                let stage = self.app.progress_stage.clone();
                self.app.cancel_slice();
                // How long the job takes to stop (the user waits for this before slicing
                // again); the invariant check fails it past `cancel_timeout`.
                let asked = Instant::now();
                while self.app.stopping.is_some() && asked.elapsed() < self.limits.cancel_timeout {
                    self.poll();
                    std::thread::sleep(Duration::from_millis(2));
                }
                let latency = asked.elapsed().as_secs_f64();
                self.stats.cancel_max_s = self.stats.cancel_max_s.max(latency);
                self.note(format!(
                    "  stopped {latency:.2} s after cancel (in {stage})"
                ));
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn preview(&mut self) {
        match self.rng.below(5) {
            3 | 4 => self.layer_gcode(),
            0 => {
                if self.app.finished().is_some() {
                    self.app.preview = !self.app.preview;
                    self.note(format!("preview {}", self.app.preview));
                } else {
                    self.app.preview = false;
                    self.note("preview: nothing sliced".into());
                }
            }
            _ => {
                let layers = self.app.finished().map_or(0, |f| f.preview.layers.len());
                let pick = |rng: &mut Rng| match rng.below(4) {
                    0 => 0,
                    1 => usize::MAX,
                    _ => rng.below(layers + 2),
                };
                self.app.view.high = pick(&mut self.rng);
                self.app.view.low = pick(&mut self.rng);
                self.app.view.moves = if self.rng.chance(0.3) {
                    u64::MAX
                } else {
                    self.rng.next_u64() % 5000
                };
                self.app.view.mask = self.rng.next_u64() as u32;
                self.app.view.tool_mask = self.rng.next_u64() as u32;
                self.app.view.color = *self.rng.pick(&crate::preview::ColorMode::ALL);
                self.note(format!(
                    "scrub low {} high {} moves {} {:?}",
                    self.app.view.low, self.app.view.high, self.app.view.moves, self.app.view.color
                ));
                if self.app.finished().is_some() {
                    self.app.preview = true;
                }
            }
        }
    }

    /// The view tools: measure clicks on random points (results must be finite), section planes
    /// in either mode (Prepare caps must lie on their plane), the tools' keys.
    fn view_tools(&mut self) {
        let rect = self.app.viewport_rect;
        let at = |rng: &mut Rng| {
            egui::pos2(
                rect.left() + rng.range(0., rect.width() as f64) as f32,
                rect.top() + rng.range(0., rect.height() as f64) as f32,
            )
        };
        match self.rng.below(6) {
            0 => {
                self.note("measure toggle (U)".into());
                for a in self.app.viewport.toggle_measure() {
                    self.handle(a);
                }
            }
            1 | 2 => {
                let (objects, _) = self.app.scene_objects();
                let aspect = rect.width() / rect.height().max(1.);
                for _ in 0..1 + self.rng.below(3) {
                    // Mostly on an object (near its centre), sometimes anywhere.
                    let on_object = objects
                        .get(self.rng.below(objects.len().max(1)))
                        .filter(|_| self.rng.chance(0.7))
                        .and_then(|o| {
                            let (lo, hi) = o.bounds();
                            let jitter = self.rng.vec3(0.4) * (hi - lo);
                            self.app
                                .viewport
                                .camera
                                .project((lo + hi) * 0.5 + jitter, aspect)
                        })
                        .map(|uv| {
                            egui::pos2(
                                rect.left() + uv.x * rect.width(),
                                rect.top() + uv.y * rect.height(),
                            )
                        });
                    let p = on_object.unwrap_or_else(|| at(&mut self.rng));
                    let shift = self.rng.chance(0.3);
                    let picked = self.app.viewport.measure_pick(&objects, rect, p, shift);
                    self.note(format!(
                        "measure click {:.0},{:.0} shift {shift}: {picked}",
                        p.x, p.y
                    ));
                }
                for row in self.app.viewport.measure.rows() {
                    if row.value.contains("NaN") || row.value.contains("inf") {
                        self.problems
                            .push(format!("measure {}: {}", row.label, row.value));
                    }
                }
                if self.rng.chance(0.2) {
                    self.note("measure unselect".into());
                    self.app.viewport.measure.unselect();
                }
            }
            3 => {
                let preview = self.app.preview && self.rng.chance(0.5);
                let dir = self.app.viewport.camera.direction();
                self.note(format!("section toggle (preview {preview})"));
                self.app.viewport.section.toggle(preview, dir);
            }
            4 => {
                let normal = self.rng.vec3(1.).normalize_or(Vec3::Z);
                let position = self.rng.range(-0.1, 1.1) as f32;
                let plane = crate::viewport::section::Plane { normal, position };
                self.note(format!("section plane {normal:?} at {position:.2}"));
                if self.app.preview {
                    self.app.viewport.section.preview = Some(plane);
                } else {
                    self.app.viewport.section.prepare = Some(plane);
                }
                // The caps of every object the plane crosses lie on it.
                let (objects, _) = self.app.scene_objects();
                if let Some(o) = objects.get(self.rng.below(objects.len().max(1))) {
                    let (lo, hi) = o.bounds();
                    let eq = plane.equation((lo, hi));
                    let cap = crate::viewport::section::object_cap(o, eq);
                    let tolerance = 1e-3 * (hi - lo).length().max(1.);
                    if cap.vertices.iter().any(|v| {
                        let v = Vec3::from(*v);
                        !v.is_finite() || (normal.dot(v) - eq[3]).abs() > tolerance
                    }) {
                        self.problems
                            .push(format!("section cap of {} off its plane", o.name));
                    }
                }
            }
            _ => {
                self.app.view.color = *self.rng.pick(&crate::preview::ColorMode::ALL);
                self.note(format!("colour by {:?}", self.app.view.color));
            }
        }
        // A few frames so the tools draw (hover, caps, cards).
        let p = at(&mut self.rng);
        self.frame(vec![egui::Event::PointerMoved(p)]);
        self.frame(vec![]);
    }

    /// The layer slider's custom G-code: add an item (what the "+" adds, a pause, custom
    /// G-code, a tool change) at a random layer, remove one, or open the menu or an editor
    /// for the next frames to draw.
    fn layer_gcode(&mut self) {
        use crate::preview::layer_gcode::{Dialog, Edit, LayerGCode};
        use mica_core::custom_gcode::{Item, Kind};
        let zs: Vec<f64> = self.app.finished().map_or_else(
            || (1..=60).map(|i| 0.2 * i as f64).collect(),
            |f| f.preview.layer_zs(),
        );
        let Some(&z) = zs.get(self.rng.below(zs.len().max(1))) else {
            return;
        };
        let info = self.app.plates.current().custom_gcode.clone();
        let tools = self.app.tool_count();
        let (object_tools, base_tool) = self.app.plate_object_tools(self.app.plates.current);
        let colors = self.app.tool_colors_f32();
        let g = LayerGCode {
            info: &info,
            tools,
            object_tools,
            base_tool,
            klipper: self
                .app
                .config
                .as_ref()
                .is_ok_and(|c| c.string("gcode_flavor") == Some("klipper")),
            has_template: true,
            tool_colors: &colors,
        };
        let edit = match self.rng.below(7) {
            0 | 1 => {
                let (item, label) = g.default_item(z);
                Some(Edit::Put {
                    item,
                    mode: g.mode(),
                    label,
                })
            }
            2 => Some(Edit::Put {
                item: Item {
                    extra: "Monkey pause".into(),
                    ..Item::new(z, Kind::PausePrint, base_tool + 1)
                },
                mode: g.mode(),
                label: "Add pause",
            }),
            3 => Some(Edit::Put {
                item: Item {
                    extra: "M117 monkey\nG4 P10".into(),
                    ..Item::new(z, Kind::Custom, base_tool + 1)
                },
                mode: g.mode(),
                label: "Add custom G-code",
            }),
            4 => Some(Edit::Put {
                item: g.tool_change(z, self.rng.below(tools.max(1))),
                mode: g.mode(),
                label: "Add tool change",
            }),
            5 if !info.items.is_empty() => Some(Edit::Remove {
                print_z: info.items[self.rng.below(info.items.len())].print_z,
            }),
            _ => {
                if self.rng.chance(0.5) {
                    self.app.view.layer_ui.menu = true;
                    self.note("layer G-code: menu".into());
                } else {
                    self.app.view.layer_ui.dialog = Some(Dialog {
                        kind: if self.rng.chance(0.5) {
                            Kind::PausePrint
                        } else {
                            Kind::Custom
                        },
                        print_z: z,
                        text: String::new(),
                    });
                    self.note("layer G-code: editor".into());
                }
                None
            }
        };
        if let Some(edit) = edit {
            self.note(format!("layer G-code at {z:.2}: {edit:?}"));
            let ctx = self.ctx.clone();
            self.app.edit_layer_gcode(edit, &ctx);
        }
    }

    /// Save the scene as a project (either flavour), open it again, and compare.
    fn round_trip(&mut self) {
        if self.app.plates.is_blank() {
            return;
        }
        let flavour = if self.rng.chance(0.5) {
            mica_core::threemf::Flavour::Orca
        } else {
            mica_core::threemf::Flavour::Prusa
        };
        let path = self
            .dir
            .join(format!("roundtrip-{}.3mf", self.stats.round_trips % 3));
        self.note(format!("save {} and reopen", flavour.name()));
        let bed = self.app.bed.clone();
        // Objects on their bed must come back on the same plate; one left beside every bed
        // is saved unassigned (as Orca keeps it) and comes back on the nearest plate.
        let on_bed: Vec<usize> = self
            .app
            .plates
            .plates
            .iter()
            .map(|p| {
                p.objects
                    .iter()
                    .filter(|o| {
                        let (min, max) = o.bounds();
                        min.x < bed.max.x
                            && max.x > bed.min.x
                            && min.y < bed.max.y
                            && max.y > bed.min.y
                    })
                    .count()
            })
            .collect();
        let total: usize = self.app.plates.plates.iter().map(|p| p.objects.len()).sum();
        let layer_gcode: Vec<_> = self
            .app
            .plates
            .plates
            .iter()
            .map(|p| p.custom_gcode.items.clone())
            .collect();
        if let Err(e) = self.app.save_project_to(&path, flavour) {
            self.note(format!("  save error: {e}"));
            return;
        }
        let result = crate::app::load_files(vec![path], &self.app.config, self.app.bed.center());
        match &result {
            Ok(scene) => {
                let mut after = vec![0usize; scene.plates.len().max(1)];
                for (plate, _) in &scene.objects {
                    if let Some(n) = after.get_mut(plate.unwrap_or(0)) {
                        *n += 1;
                    }
                }
                let lost = on_bed
                    .iter()
                    .enumerate()
                    .any(|(i, &n)| after.get(i).copied().unwrap_or(0) < n);
                // Per-layer custom G-code comes back on its plate.
                for (i, before) in layer_gcode.iter().enumerate() {
                    let after = match scene.plates.get(i) {
                        Some(p) => &p.custom_gcode.items,
                        None if i == 0 => &scene.custom_gcode.items,
                        None => continue,
                    };
                    if before != after {
                        self.problems.push(format!(
                            "round trip changed plate {i}'s layer G-code: {before:?} → {after:?}"
                        ));
                    }
                }
                if scene.objects.len() != total || lost {
                    self.problems.push(format!(
                        "round trip changed the objects: {total} objects, on their beds {on_bed:?} → {} objects, per plate {after:?}",
                        scene.objects.len()
                    ));
                }
            }
            Err(e) => {
                self.problems
                    .push(format!("a saved project does not open again: {e}"));
            }
        }
        self.app.finish_load(result);
        self.stats.round_trips += 1;
    }

    /// Dialogs, editors and view toggles, then a few random clicks into them.
    fn dialogs(&mut self) {
        let tools = self.app.tool_count();
        let what = match self.rng.below(14) {
            0 => {
                self.app.dialogs.printer_editor = true;
                self.app.editor.printer_page = self.rng.below(8);
                self.app.editor.printer_extruder = self.rng.below(tools);
                "printer editor".to_string()
            }
            1 => {
                // Sometimes a tool the printer does not have (a dialog left open over a
                // printer switch).
                let t = self.rng.below(tools + 2);
                self.app.dialogs.filament_editor = Some(t);
                self.app.editor.filament_page = self.rng.below(8);
                format!("filament editor {}", t + 1)
            }
            2 => {
                self.app.dialogs.all_printers = true;
                self.app.dialogs.all_printers_filter = self
                    .rng
                    .pick(&["", "XL", "u1", "(", "ñ", "0.4"])
                    .to_string();
                format!("all printers {:?}", self.app.dialogs.all_printers_filter)
            }
            3 => {
                self.app.save_as(None);
                "save project as".into()
            }
            4 => {
                self.app.dialogs.unsaved = Some(if self.rng.chance(0.5) {
                    crate::save::Pending::Close
                } else {
                    crate::save::Pending::Open(vec![self.rng.pick(&self.models).clone()])
                });
                "unsaved-changes prompt".into()
            }
            5 => {
                self.app.dialogs.print = true;
                "print dialog".into()
            }
            6 => {
                let s = self
                    .rng
                    .pick(&["", "infill", "temp", "[", ".*", "zzz", "°"])
                    .to_string();
                self.app.editor.search = s.clone();
                self.app.editor.printer_search = s.clone();
                self.app.editor.filament_search = s.clone();
                format!("search {s:?}")
            }
            7 => {
                let mode = if self.rng.chance(0.5) {
                    crate::theme::Mode::Light
                } else {
                    crate::theme::Mode::Dark
                };
                let ctx = self.ctx.clone();
                self.app.set_theme(&ctx, mode);
                format!("theme {}", mode.name())
            }
            8 => {
                self.app.prefs.advanced = !self.app.prefs.advanced;
                self.app.prefs.sidebar_right = self.rng.chance(0.7);
                "advanced / sidebar side".into()
            }
            9 => {
                self.app.editor.object_mode = self.rng.chance(0.5);
                self.app.editor.tower_mode = self.rng.chance(0.3);
                "object / tower settings mode".into()
            }
            10 => {
                if self.app.project.is_some() && self.rng.chance(0.5) {
                    self.app.update_project_to_current();
                    "update project to current profile".into()
                } else if self.app.project.is_some() {
                    self.app.revert_project_settings();
                    "revert project settings".into()
                } else {
                    "no project card".into()
                }
            }
            11 => self.preferences(),
            12 => self.new_project(),
            _ => {
                self.app.dialogs = Default::default();
                "close dialogs".into()
            }
        };
        self.note(format!("dialog: {what}"));
        for _ in 0..self.rng.below(3) {
            self.ui_frames();
        }
        self.frame(vec![]);
    }

    /// Preferences (⌘,): the window open, and every preference changed the way its control
    /// would (including values a hand-edited file could hold, which must be clamped).
    fn preferences(&mut self) -> String {
        let ctx = self.ctx.clone();
        // The next tab each time (no random draw, so runs keep their sequence): every tab
        // gets drawn with the changes below.
        let tab = self.app.preferences_tab().step(1);
        self.app.open_preferences(Some(tab));
        let mut changed = vec![format!("tab {}", tab.key())];
        if self.rng.chance(0.3) {
            let c = *self.rng.pick(&crate::theme::Choice::ALL);
            self.app.set_theme_choice(&ctx, c);
            changed.push(format!("theme {}", c.name()));
        }
        if self.rng.chance(0.3) {
            self.app.prefs.auto_slice = !self.app.prefs.auto_slice;
            self.app.prefs.auto_slice_delay_ms = *self.rng.pick(&[200, 500, 1300, 5000]);
            changed.push(format!(
                "auto-slice {} after {} ms",
                self.app.prefs.auto_slice, self.app.prefs.auto_slice_delay_ms
            ));
        }
        if self.rng.chance(0.3) {
            let n = *self.rng.pick(&[0, 1, 3, 10, 30, 500]);
            self.app.prefs.set_recent_limit(n);
            changed.push(format!("recent files {n}"));
        }
        if self.rng.chance(0.2) {
            // Textures and ColorMix flip together (one draw keeps the run's sequence).
            let on = !self.app.prefs.features.textures;
            self.app.prefs.features.textures = on;
            self.app.prefs.features.color_mix = on;
            self.app.settings_changed();
            changed.push(format!("textures and ColorMix features {on}"));
        }
        if self.rng.chance(0.3) {
            let prints = self.app.profiles.processes.clone();
            let materials = self.app.profiles.materials.clone();
            let d = &mut self.app.prefs.defaults;
            d.print = match self.rng.below(3) {
                0 => String::new(),
                1 if !prints.is_empty() => self.rng.pick(&prints).clone(),
                _ => "No such profile".into(),
            };
            d.filament = match self.rng.below(3) {
                0 => String::new(),
                1 if !materials.is_empty() => self.rng.pick(&materials).clone(),
                _ => "(".into(),
            };
            d.printer = if self.rng.chance(0.5) {
                String::new()
            } else {
                "Prusa/no such printer".into()
            };
            changed.push(format!("defaults {:?} / {:?}", d.print, d.filament));
        }
        if self.rng.chance(0.3) {
            self.app.viewport.camera.perspective = self.rng.chance(0.5);
            self.app.prefs.zoom_to_pointer = self.rng.chance(0.5);
            self.app.prefs.reverse_zoom = self.rng.chance(0.5);
            let sizes = crate::state::ToolbarSize::ALL;
            self.app.prefs.toolbar_size = sizes[self.rng.below(sizes.len())];
            changed.push("3D view options".into());
        }
        self.app.prefs_dirty = true;
        format!("preferences [{}]", changed.join(", "))
    }

    /// File › New project (⌘N): with unsaved changes it asks first; the answer is "Don't
    /// save" or Cancel (Save needs the file dialog).
    fn new_project(&mut self) -> String {
        let ctx = self.ctx.clone();
        self.app.new_project();
        if self.app.dialogs.unsaved == Some(crate::save::Pending::New) {
            if self.rng.chance(0.6) {
                self.app.dialogs.unsaved = None;
                self.app.proceed(crate::save::Pending::New, &ctx);
                "new project (don't save)".into()
            } else {
                self.app.dialogs.unsaved = None;
                "new project (cancelled)".into()
            }
        } else {
            "new project".into()
        }
    }

    /// The Add printer window: its screens, a random model set up with random choices and
    /// added (slice only, or connected to a loopback mock), the search and brand filters. A
    /// headless run never searches the network.
    fn add_printer(&mut self) {
        use crate::panels::add_printer::{Connect, Screen};
        self.app.open_add_printer();
        let screen = *self
            .rng
            .pick(&[Screen::Found, Screen::Gallery, Screen::Setup]);
        self.app.add_printer.screen = screen;
        self.app.add_printer.search = self
            .rng
            .pick(&["", "xl", "u1", "core one", "ñ", "("])
            .to_string();
        self.app.add_printer.brand = self
            .rng
            .pick(&[None, Some("Prusa"), Some("Snapmaker"), Some("Nobody")])
            .map(str::to_string);
        if screen == Screen::Setup {
            if self.app.add_printer.catalog.is_none() {
                self.app.add_printer.catalog = Some(crate::printhost::catalog::Catalog::build(
                    &self.app.profiles,
                ));
            }
            let n = self
                .app
                .add_printer
                .catalog
                .as_ref()
                .map_or(0, |c| c.models.len());
            if n == 0 {
                self.note("add printer: no models".into());
                return;
            }
            let mi = self.rng.below(n);
            let m = self.app.add_printer.catalog.as_ref().expect("built").models[mi].clone();
            let mut choice = m.default_choice();
            let tools = m.tool_options();
            choice.tools = self.rng.pick(&tools).clone();
            let nozzles = m.nozzle_options();
            choice.nozzle = self.rng.pick(&nozzles).raw.clone();
            let address = self
                .rng
                .pick(&["", "127.0.0.1:18081", "192.0.2.50", "u1.local:7125"])
                .to_string();
            self.app.begin_setup(
                mi,
                Some(choice),
                Some(Connect {
                    address: address.clone(),
                    ..Connect::default()
                }),
                Screen::Gallery,
            );
            let name = self
                .app
                .add_printer
                .setup
                .as_ref()
                .map(|s| s.name.clone())
                .unwrap_or_default();
            if self.rng.chance(0.5) {
                let before = self.app.printers.list.len();
                self.app.finish_setup();
                self.note(format!(
                    "add printer: {} ({}) at {address:?}: {}",
                    m.name,
                    name,
                    match &self.app.add_printer.error {
                        Some(e) => e.clone(),
                        None if self.app.printers.list.len() > before => "connected".into(),
                        None => "slice only".into(),
                    }
                ));
                return;
            }
            self.note(format!("add printer: setup {} ({name})", m.name));
            return;
        }
        self.note(format!("add printer: {screen:?}"));
    }

    /// The printer workflow against the loopback mock printers: My printers, connection tests,
    /// the Machine view, the Print dialog's check and upload, filament sync, adding and
    /// removing printers (an address typed at random must be refused, never contacted), and
    /// the calibration menu.
    fn printers(&mut self) {
        let ctx = self.ctx.clone();
        let n = self.app.printers.list.len();
        let pick = |rng: &mut Rng| (n > 0).then(|| rng.below(n));
        match self.rng.below(12) {
            11 => self.add_printer(),
            9 => self.dashboard(),
            10 if n > 0 => self.camera_url(),
            0 => {
                self.note("printers: My printers".into());
                self.app.dialogs.my_printers = true;
                self.app.printers_editor.selected = pick(&mut self.rng);
            }
            1 if n > 0 => {
                let i = self.rng.below(n);
                self.note(format!("printers: test {}", self.app.printers.list[i].name));
                self.app.printers.test(i, || {});
                self.wait_printers(Duration::from_secs(8));
                let name = self.app.printers.list[i].name.clone();
                let result = match self.app.printers.tests.get(&name) {
                    Some(crate::printhost::Test::Done(_)) => "ok".to_string(),
                    Some(crate::printhost::Test::Failed(e)) => e.headline.clone(),
                    _ => "unfinished".into(),
                };
                self.note(format!("  test: {result}"));
            }
            2 => {
                self.app.machine = self.rng.chance(0.8);
                self.app.machine_printer = if self.rng.chance(0.2) {
                    // A stale id (a printer removed meanwhile).
                    Some(u64::MAX - self.rng.below(3) as u64)
                } else {
                    pick(&mut self.rng).and_then(|i| self.app.printers.id_at(i))
                };
                self.note(format!(
                    "printers: machine view {} on {:?}",
                    self.app.machine, self.app.machine_printer
                ));
                for _ in 0..3 {
                    self.frame(vec![]);
                    std::thread::sleep(Duration::from_millis(30));
                }
            }
            3 => {
                self.note("printers: Print…".into());
                if self.app.finished().is_none() {
                    self.app.slice_current(&ctx);
                    self.wait_for_slice(None);
                }
                self.app.open_print_dialog();
                if self.app.print_job.is_some() {
                    self.app.start_check(&ctx);
                    self.wait_print_job(Duration::from_secs(10));
                    if self.rng.chance(0.5) {
                        self.note("  upload".into());
                        self.app.start_upload(&ctx);
                        self.wait_print_job(Duration::from_secs(20));
                    }
                }
                if self.rng.chance(0.5) {
                    self.app.print_job = None;
                    self.app.dialogs.print = false;
                }
            }
            4 => {
                self.note("printers: sync filaments".into());
                self.app.sync_filaments_from_printer(&ctx);
                self.wait_printers(Duration::from_secs(8));
                self.app.finish_pending_sync();
            }
            5 => {
                // A printer at an address typed at random: it must never be contacted.
                let address = self
                    .rng
                    .pick(&[
                        "printer.local",
                        "198.51.100.5",
                        "192.0.2.9:80",
                        "abc",
                        "0",
                        "http://example.com",
                        "[::1]:1",
                        "127.0.0.1:1",
                    ])
                    .to_string();
                let kind = *self.rng.pick(&crate::printhost::config::Kind::ALL);
                let mut p = crate::printhost::config::Printer::new(kind);
                p.name = format!("typed {}", self.step);
                p.address = address.clone();
                self.note(format!("printers: add {:?} at {address}", kind));
                if let Ok(i) = self.app.printers.put(None, p, None) {
                    self.app.printers.test(i, || {});
                    self.wait_printers(Duration::from_secs(8));
                    let name = self.app.printers.list[i].name.clone();
                    if !crate::printhost::is_loopback(&address)
                        && matches!(
                            self.app.printers.tests.get(&name),
                            Some(crate::printhost::Test::Done(_))
                        )
                    {
                        self.problems
                            .push(format!("a printer at {address} answered a connection test"));
                    }
                }
            }
            6 if n > self.mock_printers => {
                // Never the mock printers themselves (they come first).
                let i = self.mock_printers + self.rng.below(n - self.mock_printers);
                self.note(format!(
                    "printers: remove {}",
                    self.app.printers.list[i].name
                ));
                self.app.printers.remove(i);
            }
            7 => {
                let test = *self.rng.pick(&crate::calibration::Test::ALL);
                self.note(format!("calibration: {test:?}"));
                self.app.open_calibration(test);
                let tools = self.app.tool_count();
                if let Some(card) = &mut self.app.dialogs.calibration
                    && let Ok(c) = self.app.config.as_ref()
                {
                    card.set_tool(self.rng.below(tools), c);
                }
                if self.rng.chance(0.6) {
                    match self.app.create_calibration_plate(&ctx) {
                        Ok(i) => self.note(format!("  calibration plate {i}")),
                        Err(e) => self.note(format!("  calibration refused: {e}")),
                    }
                }
            }
            _ => {
                let input = self
                    .rng
                    .pick(&["", "3", "0.04", "abc", "-1", "1e9", "2,5"])
                    .to_string();
                if let Some(job) = &mut self.app.plates.current_mut().calibration {
                    job.input = input.clone();
                    self.note(format!("calibration: result {input:?}"));
                    if self.rng.chance(0.5) {
                        let saved = self.app.save_calibration_result();
                        self.note(format!("  save: {saved:?}"));
                    }
                }
            }
        }
        for _ in 0..self.rng.below(3) {
            self.ui_frames();
        }
        self.frame(vec![]);
    }

    /// The Printers dashboard: show it (every printer polled, a camera thumbnail per tile),
    /// sometimes open a tile, sometimes leave the Machine tab. Invariants: at most one
    /// thumbnail per printer plus the focused picture, every camera on this computer, and no
    /// stream left running outside the Machine tab.
    fn dashboard(&mut self) {
        self.app.machine = true;
        self.app.machine_printer = None;
        self.note("printers: dashboard".into());
        let frames = 2 + self.rng.below(6);
        for _ in 0..frames {
            self.frame(vec![]);
            std::thread::sleep(Duration::from_millis(40));
        }
        self.check_cameras();
        match self.rng.below(3) {
            0 => {
                let n = self.app.printers.list.len();
                if n > 0 {
                    let i = self.rng.below(n);
                    self.app.machine_printer = self.app.printers.id_at(i);
                    self.note(format!("  open {}", self.app.printers.list[i].name));
                    for _ in 0..3 {
                        self.frame(vec![]);
                        std::thread::sleep(Duration::from_millis(40));
                    }
                    self.check_cameras();
                }
            }
            1 => {
                self.note("  leave the Machine tab".into());
                self.app.machine = false;
                self.frame(vec![]);
                if !self.app.machine_view.cameras.is_empty() {
                    self.problems.push(format!(
                        "{} camera streams still run outside the Machine tab (Library shown: {})",
                        self.app.machine_view.cameras.len(),
                        self.app.library.shown
                    ));
                }
            }
            _ => {}
        }
    }

    /// A camera URL typed into a printer: the mock's own cameras, or addresses that must be
    /// refused (never contacted) because they are not on this computer.
    fn camera_url(&mut self) {
        let n = self.app.printers.list.len();
        let i = self.rng.below(n);
        let ports = self._mock.as_ref().map(|m| m.ports);
        let mut urls = vec![
            String::new(),
            "rtsp://192.0.2.9:554/stream1".to_string(),
            "http://example.com/snapshot.jpg".into(),
            "http://198.51.100.5/stream.mjpg".into(),
            "rtsp://user:pw@127.0.0.1:1/x".into(),
            "ftp://127.0.0.1/x".into(),
            "http://127.0.0.1:1/stream.mjpg".into(),
            "nonsense".into(),
        ];
        if let Some(p) = ports {
            urls.push(format!("rtsp://127.0.0.1:{}/coreone", p[3]));
            urls.push(format!("http://127.0.0.1:{}/snapshot.jpg", p[4]));
            urls.push(format!("http://127.0.0.1:{}/stream.mjpg", p[4]));
            urls.push(format!("http://127.0.0.1:{}/webcam/?action=stream", p[2]));
        }
        let url = self.rng.pick(&urls).clone();
        let mut printer = self.app.printers.list[i].clone();
        printer.camera = (!url.is_empty()).then(|| url.clone());
        let result = self.app.printers.put(Some(i), printer, None);
        self.app
            .machine_view
            .cameras
            .forget(self.app.printers.list[i].id);
        self.note(format!(
            "printers: camera of {} = {url:?}: {}",
            self.app.printers.list[i].name,
            if result.is_ok() { "saved" } else { "refused" }
        ));
        if result.is_ok() && !url.is_empty() && !mica_camera::is_loopback(&url) {
            self.problems.push(format!(
                "a camera URL off this computer was accepted: {url}"
            ));
        }
        self.dashboard();
    }

    fn check_cameras(&mut self) {
        let views = self.app.machine_view.cameras.len();
        let limit = self.app.printers.list.len() + 1;
        if views > limit {
            self.problems
                .push(format!("{views} camera players for {} printers", limit - 1));
        }
        for url in self.app.machine_view.cameras.urls() {
            if !mica_camera::is_loopback(&url) {
                self.problems
                    .push(format!("a camera off this computer was opened: {url}"));
            }
        }
    }

    /// Lets running connection tests finish (bounded).
    fn wait_printers(&mut self, limit: Duration) {
        let started = Instant::now();
        while self
            .app
            .printers
            .tests
            .values()
            .any(|t| matches!(t, crate::printhost::Test::Running(_)))
            && started.elapsed() < limit
        {
            self.app.printers.drain();
            std::thread::sleep(Duration::from_millis(10));
        }
        self.app.printers.drain();
    }

    /// Lets the Print dialog's check or upload finish (bounded).
    fn wait_print_job(&mut self, limit: Duration) {
        let started = Instant::now();
        while self.app.print_job.as_ref().is_some_and(|d| d.busy()) && started.elapsed() < limit {
            if let Some(d) = &mut self.app.print_job {
                d.poll();
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        if let Some(d) = &self.app.print_job {
            self.note(format!("  dialog: {:?}", d.stage));
        }
    }

    /// Esc during a gizmo drag.
    fn gesture_cancel(&mut self) {
        let Some(index) = self.random_object() else {
            return;
        };
        let delta = self.random_delta();
        self.note(format!("drag {index} {delta:?} then Esc"));
        self.handle(Action::GestureBegin("Move"));
        self.handle(Action::Transform { index, delta });
        self.handle(Action::GestureCancel);
    }

    /// Keyboard shortcuts through a real frame.
    fn keys(&mut self) {
        // Every key the shortcut table binds (keymap.rs), plus a few it does not.
        let mut keys = crate::keymap::keys();
        keys.extend([egui::Key::Enter, egui::Key::Q, egui::Key::W, egui::Key::E]);
        let key = *self.rng.pick(&keys);
        let modifiers = match self.rng.below(6) {
            0 => egui::Modifiers::COMMAND,
            1 => egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
            2 => egui::Modifiers::SHIFT,
            3 => egui::Modifiers::ALT,
            _ => egui::Modifiers::NONE,
        };
        self.note(format!("key {key:?} {modifiers:?}"));
        let press = |pressed| egui::Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers,
        };
        self.frame(vec![egui::Event::ModifiersChanged(modifiers), press(true)]);
        self.frame(vec![
            press(false),
            egui::Event::ModifiersChanged(egui::Modifiers::NONE),
        ]);
    }

    /// Random pointer gestures through the whole window: clicks, drags, right-click menus,
    /// typing into whatever field took focus.
    fn ui_frames(&mut self) {
        let size = egui::vec2(1440., 900.);
        let at = |rng: &mut Rng| {
            egui::pos2(
                rng.range(0., size.x as f64) as f32,
                rng.range(0., size.y as f64) as f32,
            )
        };
        let button = |pos, button, pressed, modifiers| egui::Event::PointerButton {
            pos,
            button,
            pressed,
            modifiers,
        };
        let primary = egui::PointerButton::Primary;
        let none = egui::Modifiers::NONE;
        match self.rng.below(6) {
            0 | 1 => {
                let p = at(&mut self.rng);
                self.note(format!("ui click {:.0},{:.0}", p.x, p.y));
                self.frame(vec![egui::Event::PointerMoved(p)]);
                self.frame(vec![button(p, primary, true, none)]);
                self.frame(vec![button(p, primary, false, none)]);
                self.frame(vec![]);
            }
            2 => {
                let a = at(&mut self.rng);
                let b = at(&mut self.rng);
                let shift = self.rng.chance(0.3);
                let m = if shift { egui::Modifiers::SHIFT } else { none };
                self.note(format!(
                    "ui drag {:.0},{:.0} → {:.0},{:.0} shift {shift}",
                    a.x, a.y, b.x, b.y
                ));
                self.frame(vec![
                    egui::Event::ModifiersChanged(m),
                    egui::Event::PointerMoved(a),
                ]);
                self.frame(vec![button(a, primary, true, m)]);
                for k in 1..=5 {
                    self.frame(vec![egui::Event::PointerMoved(a.lerp(b, k as f32 / 5.))]);
                }
                self.frame(vec![button(b, primary, false, m)]);
                self.frame(vec![egui::Event::ModifiersChanged(none)]);
            }
            3 => {
                // A context menu, then a click a little below and right of it (an item).
                let p = at(&mut self.rng);
                let q = p + egui::vec2(
                    self.rng.range(5., 120.) as f32,
                    self.rng.range(5., 300.) as f32,
                );
                self.note(format!(
                    "ui right-click {:.0},{:.0} then {:.0},{:.0}",
                    p.x, p.y, q.x, q.y
                ));
                let secondary = egui::PointerButton::Secondary;
                self.frame(vec![egui::Event::PointerMoved(p)]);
                self.frame(vec![button(p, secondary, true, none)]);
                self.frame(vec![button(p, secondary, false, none)]);
                self.frame(vec![egui::Event::PointerMoved(q)]);
                self.frame(vec![button(q, primary, true, none)]);
                self.frame(vec![button(q, primary, false, none)]);
                self.frame(vec![]);
            }
            4 => {
                // Click (often into a field), type, Enter.
                let p = at(&mut self.rng);
                let text = self
                    .rng
                    .pick(&[
                        "0",
                        "-1",
                        "1e6",
                        "nan",
                        "inf",
                        "-inf",
                        "999999999",
                        "0.0001",
                        "abc",
                        "",
                        "12,5",
                        "50%",
                        "1e400",
                        "-0",
                    ])
                    .to_string();
                self.note(format!("ui type {text:?} at {:.0},{:.0}", p.x, p.y));
                self.frame(vec![egui::Event::PointerMoved(p)]);
                self.frame(vec![button(p, primary, true, none)]);
                self.frame(vec![button(p, primary, false, none)]);
                self.frame(vec![]);
                let select_all = egui::Event::Key {
                    key: egui::Key::A,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::COMMAND,
                };
                self.frame(vec![select_all, egui::Event::Text(text)]);
                let enter = |pressed| egui::Event::Key {
                    key: egui::Key::Enter,
                    physical_key: None,
                    pressed,
                    repeat: false,
                    modifiers: none,
                };
                self.frame(vec![enter(true)]);
                self.frame(vec![enter(false)]);
                self.frame(vec![]);
            }
            _ => {
                self.note("ui idle frames".into());
                self.frame(vec![]);
                self.frame(vec![]);
            }
        }
    }

    /// One frame of the whole window with these input events (it polls the worker too).
    fn frame(&mut self, events: Vec<egui::Event>) {
        self.time += 0.05;
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1440., 900.),
            )),
            time: Some(self.time),
            predicted_dt: 0.05,
            events,
            ..Default::default()
        };
        self.observe(|m| {
            let app = &mut m.app;
            let mut output = m.ctx.run_ui(input, |ui| app.show(ui));
            // Nothing renders headless: the texture uploads are dropped on purpose.
            output.textures_delta.clear();
        });
        self.stats.ui_frames += 1;
    }

    fn poll(&mut self) {
        self.observe(|m| {
            let ctx = m.ctx.clone();
            m.app.poll(&ctx);
        });
    }

    /// Runs `f` (which may poll the worker) and notes what the running job did: a completed
    /// slice is queued for validation with the configuration its job started with.
    fn observe(&mut self, f: impl FnOnce(&mut Self)) {
        let before: Vec<Option<*const crate::preview::Preview>> = self
            .app
            .plates
            .plates
            .iter()
            .map(|p| p.finished.as_ref().map(|f| Arc::as_ptr(&f.preview)))
            .collect();
        let running = self.app.worker.as_ref().map(|(_, w)| w.request);
        f(self);
        if let Some(r) = running
            && self.app.finished_request == Some(r)
            && self.app.worker.as_ref().map(|(_, w)| w.request) != Some(r)
        {
            let config = self
                .job_configs
                .iter()
                .find(|(q, _, _)| *q == r)
                .map(|(_, c, odd)| (c.clone(), odd.clone()));
            let mut completed = false;
            for (i, p) in self.app.plates.plates.iter().enumerate() {
                if let Some(f) = &p.finished
                    && Some(Arc::as_ptr(&f.preview)) != before.get(i).copied().flatten()
                {
                    completed = true;
                    self.to_validate
                        .push((f.preview.job.path.clone(), i, config.clone()));
                }
            }
            if !completed {
                self.stats.slice_errors += 1;
                let status = self
                    .app
                    .plates
                    .current()
                    .slice_error
                    .as_ref()
                    .map_or_else(|| self.app.status.clone(), |e| e.message.clone());
                self.note(format!("  slice error: {status}"));
            }
        }
        self.track_jobs();
        self.track_stopping();
    }

    /// A cancelled job must stop within `cancel_timeout`, also while a step waits for slices.
    fn track_stopping(&mut self) {
        match (&self.app.stopping, self.stopping_since) {
            (Some(_), None) => self.stopping_since = Some(Instant::now()),
            (Some(_), Some(t)) if t.elapsed() > self.limits.cancel_timeout => {
                if !self.problems.iter().any(|p| p.contains("still running")) {
                    self.problems.push(format!(
                        "a cancelled slice job is still running after {:.0} s (last stage: {})",
                        t.elapsed().as_secs_f64(),
                        self.app.progress_stage
                    ));
                }
            }
            (None, Some(t)) => {
                self.stats.cancel_max_s = self.stats.cancel_max_s.max(t.elapsed().as_secs_f64());
                self.stopping_since = None;
            }
            _ => {}
        }
    }

    /// Remembers the configuration of every job that starts (the validation profile).
    fn track_jobs(&mut self) {
        if let Some((plate, w)) = &self.app.worker
            && !self.job_configs.iter().any(|(r, _, _)| *r == w.request)
        {
            let request = w.request;
            let plate = self.app.plates.index_of(*plate);
            self.stats.slices_started += 1;
            // The configuration the job slices with (`App::job_config`).
            if let Some(Ok(c)) = plate.map(|p| self.app.job_config(p)) {
                let odd = implausible_settings(&self.app);
                self.job_configs.push((request, c, odd));
            }
            if self.job_configs.len() > 8 {
                self.job_configs.remove(0);
            }
        }
    }

    // ----- invariants ---------------------------------------------------------------------

    fn check(&mut self) -> Result<(), Failure> {
        let panics = take_panics();
        if !panics.is_empty() {
            return Err(self.fail("panic", panics.join("\n")));
        }
        // Errors land in notifications and on their plate, not in the hint line.
        let errors: Vec<String> = self
            .app
            .notifications
            .list
            .iter()
            .map(|n| n.text.clone())
            .chain(
                self.app
                    .plates
                    .plates
                    .iter()
                    .filter_map(|p| p.slice_error.as_ref().map(|e| e.message.clone())),
            )
            .collect();
        for text in [&self.app.status]
            .into_iter()
            .chain(self.app.notices.iter())
            .chain(errors.iter())
        {
            if text.contains("Unexpected failure") || text.contains("stopped unexpectedly") {
                return Err(self.fail("contained crash", text.clone()));
            }
        }
        self.check_scene()
            .map_err(|e| self.fail("invalid scene", e))?;
        self.collect_mcp();
        self.track_stopping();
        if !self.problems.is_empty() {
            let problems = std::mem::take(&mut self.problems);
            let kind = if problems.iter().any(|p| p.contains("still running")) {
                "hang"
            } else {
                "invariant"
            };
            return Err(self.fail(kind, problems.join("\n")));
        }
        self.validate_slices()?;
        self.check_cached_slices()?;
        self.check_failed_slices()
            .map_err(|e| self.fail("failed slice not shown", e))?;
        Ok(())
    }

    /// After a failed slice the GUI shows an error state until the plate changes: the plate
    /// keeps its error and no slice, its label carries the error marker and its notification
    /// is shown (or the user dismissed it).
    fn check_failed_slices(&mut self) -> Result<(), String> {
        self.app.sync_notifications();
        for (id, epoch, message) in self.app.failed_slices.clone() {
            let Some(i) = self.app.plates.index_of(id) else {
                continue;
            };
            let p = self.app.plates.plate(i);
            if p.epoch != epoch {
                continue;
            }
            if p.slice_error.as_ref().map(|e| &e.message) != Some(&message) {
                return Err(format!(
                    "plate {i}: its slice failed ({message}) but it no longer says so"
                ));
            }
            if p.finished.is_some() {
                return Err(format!(
                    "plate {i}: shows a slice although its last one failed ({message})"
                ));
            }
            if self.app.plate_alert(i) != Some(crate::notifications::Level::Error) {
                return Err(format!("plate {i}: failed slice without the error marker"));
            }
            if !self
                .app
                .notifications
                .shown_or_dismissed(Some(id), crate::notifications::SLICE_ERROR)
            {
                return Err(format!(
                    "plate {i}: failed slice without its notification ({message})"
                ));
            }
        }
        Ok(())
    }

    /// The slice cache's invariants: a slice a plate shows was made from the plate's current
    /// inputs (colours aside); and a slice the cache gave back (an undo, a redo, an edit put
    /// back) is byte for byte the G-code a fresh slice of that state writes. One plate per
    /// check is sliced again, synchronously.
    fn check_cached_slices(&mut self) -> Result<(), Failure> {
        let restored = std::mem::take(&mut self.app.restored);
        for i in 0..self.app.plates.len() {
            let p = self.app.plates.plate(i);
            if p.finished.is_some() && p.sliced.is_some() && self.app.slice_fit(i).is_none() {
                let difference = match (&p.sliced, self.app.slice_inputs(i)) {
                    (Some(was), Ok(now)) => was.difference(&now),
                    (_, Err(e)) => format!("its inputs do not build: {e}"),
                    _ => String::new(),
                };
                let message = format!(
                    "plate {i} shows a slice of other inputs than it has now (restored: {}): {difference}",
                    restored.contains(&p.id)
                );
                return Err(self.fail("slice cache", message));
            }
        }
        let candidate = (0..self.app.plates.len()).find(|&i| {
            let p = self.app.plates.plate(i);
            restored.contains(&p.id)
                && p.finished
                    .as_ref()
                    .is_some_and(|f| f.preview.count < 4_000_000)
                && self.app.slice_fit(i) == Some(crate::slice_cache::Fit::Exact)
        });
        let Some(i) = candidate else {
            return Ok(());
        };
        let Ok(inputs) = self.app.slice_inputs(i) else {
            return Ok(());
        };
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let fresh = match &inputs.job {
            crate::slice_cache::Job::Objects(o, custom) => crate::worker::slice_with_custom_gcode(
                o.clone(),
                custom.clone(),
                inputs.config.clone(),
                &cancel,
                &|_| {},
            ),
            crate::slice_cache::Job::Calibration(p) => {
                crate::worker::slice_calibration(p, inputs.config.clone(), &cancel, &|_| {})
            }
        };
        let fresh = match fresh {
            Ok(f) => f,
            Err(e) => {
                let message = format!("plate {i}: the cached slice's state no longer slices: {e}");
                return Err(self.fail("slice cache", message));
            }
        };
        let cached = self.app.plates.plate(i).finished.clone().expect("checked");
        let read = |f: &crate::worker::Finished| {
            std::fs::read(f.preview.job.path.join("print.gcode")).unwrap_or_default()
        };
        let (a, b) = (read(&cached), read(&fresh));
        self.stats.cache_checks += 1;
        self.note(format!(
            "  cached slice of plate {i} vs a fresh one: {} / {} bytes",
            a.len(),
            b.len()
        ));
        if a != b || a.is_empty() {
            let kept = self.dir.join(format!("cached-{}.gcode", self.step));
            let _ = std::fs::write(&kept, &a);
            let line = a
                .split(|&c| c == b'\n')
                .zip(b.split(|&c| c == b'\n'))
                .position(|(x, y)| x != y)
                .map_or(0, |n| n + 1);
            let message = format!(
                "plate {i}: the restored slice differs from a fresh slice of the same state \
                 (first difference at line {line}; cached G-code kept at {})",
                kept.display()
            );
            return Err(self.fail("slice cache", message));
        }
        Ok(())
    }

    fn check_scene(&self) -> Result<(), String> {
        let app = &self.app;
        let plates = app.plates.len();
        if plates == 0 || app.plates.current >= plates {
            return Err(format!("current plate {} of {plates}", app.plates.current));
        }
        let len = app.plates.current().objects.len();
        if let Some(i) = app.selected
            && i >= len
        {
            return Err(format!("selected object {i} of {len}"));
        }
        if let Some(i) = app.multi.iter().find(|&&i| i >= len) {
            return Err(format!("multi-selection holds object {i} of {len}"));
        }
        if let Some(sub) = app.sub {
            let o = app
                .selected
                .and_then(|i| app.plates.current().objects.get(i));
            let exists = o.is_some_and(|o| match sub {
                Sub::Volume(id) => o.volume(id).is_some(),
                Sub::Range(id) => o.range(id).is_some(),
            });
            if !exists {
                return Err(format!(
                    "the selected part {sub:?} is not on the selected object {:?}",
                    app.selected
                ));
            }
        }
        if let Some(c) = &app.cut
            && c.selected_connector
                .is_some_and(|i| i >= c.connectors.len())
        {
            return Err(format!(
                "the selected cut connector {:?} is past the {} connectors",
                c.selected_connector,
                c.connectors.len()
            ));
        }
        if let Some(c) = &app.cut
            && app.cut_target().is_none()
        {
            return Err(format!(
                "the cut tool is open on object {} (id {}) of {len}, which is not the selected object any more",
                c.object, c.object_id
            ));
        }
        let gone = |id: u64| app.plates.index_of(id).is_none();
        if let Some((id, _)) = &app.worker
            && gone(*id)
        {
            return Err(format!("the running job slices plate #{id}, which is gone"));
        }
        if let Some((id, _, _)) = app.pending_slice
            && gone(id)
        {
            return Err(format!("a waiting slice is for plate #{id}, which is gone"));
        }
        if let Some(id) = app.slice_queue.iter().find(|&&id| gone(id)) {
            return Err(format!("the slice queue holds plate #{id}, which is gone"));
        }
        for (pi, p) in app.plates.plates.iter().enumerate() {
            for (oi, o) in p.objects.iter().enumerate() {
                let finite = o.position.is_finite()
                    && o.angle.is_finite()
                    && o.tilt.is_finite()
                    && o.scale.is_finite()
                    && o.scale.min_element() > 0.
                    && o.mirror.abs() == Vec3::ONE;
                if !finite {
                    return Err(format!(
                        "plate {pi} object {oi} ({}) has a broken transform: {o:?}",
                        o.name
                    ));
                }
                let (min, max) = o.bounds();
                if !min.is_finite() || !max.is_finite() {
                    return Err(format!(
                        "plate {pi} object {oi} ({}) has non-finite bounds",
                        o.name
                    ));
                }
                for v in &o.volumes {
                    if !(v.position.is_finite() && v.rotation.is_finite() && v.scale.is_finite()) {
                        return Err(format!("plate {pi} object {oi} volume {} broke", v.name));
                    }
                }
                for r in &o.height_ranges {
                    if !(r.min_z.is_finite() && r.max_z.is_finite()) || r.min_z > r.max_z {
                        return Err(format!(
                            "plate {pi} object {oi} has height range {}–{}",
                            r.min_z, r.max_z
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    /// Every slice that completed since the last check passes the validator.
    fn validate_slices(&mut self) -> Result<(), Failure> {
        for (job, plate, config) in std::mem::take(&mut self.to_validate) {
            self.stats.slices_completed += 1;
            let Some((config, odd)) = config else {
                self.note("  completed slice without a recorded configuration".into());
                continue;
            };
            let gcode = job.join("print.gcode");
            if !gcode.exists() {
                // The plate changed again and its slice (and folder) went before this check.
                continue;
            }
            let profile = match mica_gcode::validate::Profile::from_config(&config) {
                Ok(p) => p,
                Err(e) => {
                    self.note(format!("  no validation profile: {e}"));
                    continue;
                }
            };
            let report = mica_gcode::validate::validate_file(&gcode, &profile)
                .map_err(|e| self.fail("validation", e.to_string()))?;
            self.stats.slices_validated += 1;
            self.stats.validator_warnings += report.warnings;
            self.note(format!(
                "  validated plate {plate}: {} layers, {} errors, {} warnings",
                report.layers, report.errors, report.warnings
            ));
            if report.errors > 0 && !odd.is_empty() {
                // Far-out settings (see `implausible`): no crash is all that is asked.
                self.stats.implausible_invalid += 1;
                let rules: std::collections::BTreeSet<&str> = report
                    .diagnostics
                    .iter()
                    .filter(|d| matches!(d.severity, mica_gcode::validate::Severity::Error))
                    .map(|d| d.rule)
                    .collect();
                let text = format!(
                    "{} validator errors ({}) with implausible settings: {}",
                    report.errors,
                    rules.into_iter().collect::<Vec<_>>().join(", "),
                    odd.join(", ")
                );
                // Always in the run log, so a summary can show what was waived.
                eprintln!("MONKEY-NOTE seed {} step {}: {text}", self.seed, self.step);
                self.note(format!("  {text}"));
            } else if report.errors > 0 {
                let kept = self.dir.join(format!("invalid-{}.gcode", self.step));
                let _ = std::fs::copy(&gcode, &kept);
                let errors: Vec<String> = report
                    .diagnostics
                    .iter()
                    .filter(|d| matches!(d.severity, mica_gcode::validate::Severity::Error))
                    .take(8)
                    .map(|d| format!("line {}: {} {}", d.line, d.rule, d.message))
                    .collect();
                return Err(self.fail(
                    "validator errors",
                    format!(
                        "{} errors on plate {plate} (G-code kept at {}; temperatures {:?}, first layer {:?}):\n{}",
                        report.errors,
                        kept.display(),
                        config.raw("temperature"),
                        (config.raw("first_layer_temperature"), config.raw("layer_height"), config.raw("first_layer_height"), config.raw("z_offset"), config.raw("raft_layers")),
                        errors.join("\n")



                    ),
                ));
            }
        }
        Ok(())
    }

    /// Undo everything, redo everything: the scene must come back exactly.
    fn check_undo(&mut self) -> Result<(), Failure> {
        self.app.end_gesture();
        // From the top of the history (earlier undo bursts may have left redo steps).
        let mut guard = 0;
        while self.app.undo.can_redo() && guard < 1000 {
            self.app.redo();
            guard += 1;
        }
        let top: Snapshot = self.app.snapshot();
        let mut undone = 0;
        while self.app.undo.can_undo() && undone < 1000 {
            self.app.undo();
            undone += 1;
        }
        let mut redone = 0;
        while self.app.undo.can_redo() && redone < 1000 {
            self.app.redo();
            redone += 1;
        }
        self.stats.undo_checks += 1;
        if undone != redone {
            return Err(self.fail(
                "undo/redo",
                format!("{undone} undo steps but {redone} redo steps"),
            ));
        }
        let now = self.app.snapshot();
        if now != top {
            return Err(self.fail(
                "undo/redo",
                format!(
                    "undo all + redo all ({undone} steps) changed the scene: {}",
                    top.diff(&now).join("; ")
                ),
            ));
        }
        Ok(())
    }

    fn sample_rss(&mut self) -> Result<(), Failure> {
        let Some(mb) = crate::worker::memory_mb() else {
            return Ok(());
        };
        if self.rss.is_empty() {
            self.stats.rss_start_mb = mb;
        }
        self.stats.rss_peak_mb = self.stats.rss_peak_mb.max(mb);
        self.rss.push((self.step, mb));
        self.note(format!("memory {mb:.0} MB"));
        // Growth over the last 1,000 steps: from the smallest sample in that window.
        let floor = self
            .rss
            .iter()
            .filter(|(s, _)| s + 1000 >= self.step)
            .map(|(_, m)| *m)
            .fold(f64::INFINITY, f64::min);
        let growth = mb - floor;
        self.stats.rss_max_growth_mb = self.stats.rss_max_growth_mb.max(growth);
        if growth > self.limits.rss_growth_mb {
            return Err(self.fail(
                "memory",
                format!(
                    "Memory grew {growth:.0} MB within 1,000 steps (now {mb:.0} MB, limit {:.0} MB)",
                    self.limits.rss_growth_mb
                ),
            ));
        }
        Ok(())
    }
}

/// Runs one seed in this process.
pub fn run_seed(
    seed: u64,
    limits: &Limits,
    trace: Arc<Mutex<VecDeque<String>>>,
) -> Result<Stats, Failure> {
    install_panic_hook();
    take_panics();
    let started = Instant::now();
    // Slice folders named after this process's pid that exist before the seed starts belong to
    // an earlier process with the same (recycled) pid that was killed before it could clean up;
    // the leak check only counts folders this seed made.
    let stale = slice_folders();
    let fail0 = |message: String| Failure {
        seed,
        step: 0,
        kind: "setup",
        message,
        trace: vec![],
    };
    let mut m = Monkey::new(seed, limits.clone(), trace).map_err(fail0)?;
    if let Some(panic) = take_panics().first() {
        return Err(m.fail("panic", panic.clone()));
    }
    m.sample_rss()?;
    for step in 0..limits.steps {
        m.step = step;
        if limits.save_at == Some(step) {
            // The scene and settings before this step, to reproduce a slice with `mica slice`.
            let path = std::env::current_dir()
                .unwrap_or_default()
                .join(format!("monkey-seed{seed}-step{step}.3mf"));
            match m
                .app
                .save_project_to(&path, mica_core::threemf::Flavour::Orca)
            {
                Ok(()) => eprintln!("MONKEY-SAVED {}", path.display()),
                Err(e) => eprintln!("MONKEY-SAVE failed: {e}"),
            }
        }
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            m.step();
            m.poll();
            if m.rng.chance(0.5) {
                m.frame(vec![]);
            }
        }));
        if let Err(payload) = result {
            let message = payload
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_default();
            let mut details = take_panics();
            if details.is_empty() {
                details.push(message);
            }
            return Err(m.fail("panic", details.join("\n")));
        }
        m.check()?;
        if (step + 1) % limits.undo_check_every == 0 {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| m.check_undo()));
            match result {
                Ok(r) => r?,
                Err(_) => return Err(m.fail("panic", take_panics().join("\n"))),
            }
            m.check()?;
        }
        if (step + 1) % limits.rss_every == 0 {
            m.sample_rss()?;
        }
        m.stats.steps = step + 1;
    }
    m.check_undo()?;
    m.check()?;
    // Wind down: no job and no slice folder outlives the app.
    m.app.cancel_slice();
    let deadline = Instant::now() + limits.cancel_timeout;
    while m.app.stopping.is_some() && Instant::now() < deadline {
        m.poll();
        std::thread::sleep(Duration::from_millis(10));
    }
    if m.app.stopping.is_some() {
        return Err(m.fail("hang", "the last cancelled job never stopped".into()));
    }
    let mut stats = m.stats.clone();
    let dir = m.dir.clone();
    drop(m);
    let deadline = Instant::now() + limits.cancel_timeout;
    let leftovers = loop {
        let mut left = slice_folders();
        left.retain(|p| !stale.contains(p));
        if left.is_empty() || Instant::now() > deadline {
            break left;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    if limits.check_leaks && !leftovers.is_empty() {
        return Err(Failure {
            seed,
            step: limits.steps,
            kind: "leak",
            message: format!("slice folders left behind: {leftovers:?}"),
            trace: vec![],
        });
    }
    let _ = std::fs::remove_dir_all(dir);
    stats.seconds = started.elapsed().as_secs_f64();
    Ok(stats)
}

/// The slice job folders in the temporary directory named after this process
/// (`worker::TempJob`: `mica-gui-<pid>-…`).
fn slice_folders() -> Vec<PathBuf> {
    let prefix = format!("mica-gui-{}-", std::process::id());
    std::fs::read_dir(std::env::temp_dir())
        .map(|d| {
            d.flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .is_some_and(|n| n.to_string_lossy().starts_with(&prefix))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn stats_line(seed: u64, s: &Stats) -> String {
    format!(
        "MONKEY-OK seed={seed} steps={} seconds={:.1} slices_started={} completed={} validated={} errors={} cancels={} undo_checks={} cache_checks={} round_trips={} ui_frames={} rss_start={:.0} rss_peak={:.0} rss_growth={:.0} warnings={} implausible={} cancel_max={:.1} mcp_calls={} mcp_answered={}",
        s.steps,
        s.seconds,
        s.slices_started,
        s.slices_completed,
        s.slices_validated,
        s.slice_errors,
        s.cancels,
        s.undo_checks,
        s.cache_checks,
        s.round_trips,
        s.ui_frames,
        s.rss_start_mb,
        s.rss_peak_mb,
        s.rss_max_growth_mb,
        s.validator_warnings,
        s.implausible_invalid,
        s.cancel_max_s,
        s.mcp_calls,
        s.mcp_answered
    )
}

/// `mica-gui --monkey …` (see the module documentation).
pub fn run(args: &[String]) -> Result<(), String> {
    // The monkey test never reads or writes the user's settings, profiles, library or other
    // slicers' data (its seeds pass the same to their processes).
    mica_core::user_data::isolate();
    let value = |flag: &str| args.windows(2).find(|w| w[0] == flag).map(|w| w[1].clone());
    let number = |flag: &str| -> Result<Option<u64>, String> {
        value(flag)
            .map(|v| {
                v.parse::<u64>()
                    .map_err(|_| format!("{flag} takes a number"))
            })
            .transpose()
    };
    let quick = args.iter().any(|a| a == "--quick");
    let steps = number("--steps")?.unwrap_or(if quick { 200 } else { 2000 }) as usize;
    let mut limits = Limits::new(steps);
    limits.verbose = args.iter().any(|a| a == "--verbose");
    limits.new_features = !args.iter().any(|a| a == "--no-new-features");
    limits.cancel_after = number("--cancel-after")?.map(Duration::from_millis);
    if let Some(n) = number("--rss-every")? {
        limits.rss_every = (n as usize).max(1);
    }
    if let Some(n) = number("--rss-growth-mb")? {
        limits.rss_growth_mb = n as f64;
    }
    limits.save_at = number("--save-at")?.map(|n| n as usize);
    if quick {
        limits.slice_wait = Duration::from_secs(10);
    }
    // Crash logs of contained panics stay with the run (children inherit the variable).
    let logs = std::env::var_os("MICA_CRASH_LOG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("mica-monkey-crash-logs"));
    if let Some(seed) = number("--seed")? {
        let trace = Arc::new(Mutex::new(VecDeque::new()));
        spawn_watchdog(seed, limits.step_timeout, trace.clone());
        return match run_seed(seed, &limits, trace) {
            Ok(stats) => {
                println!("{}", stats_line(seed, &stats));
                Ok(())
            }
            Err(f) => {
                println!("{f}");
                Err(format!("seed {seed} failed"))
            }
        };
    }
    let seeds = number("--seeds")?.unwrap_or(if quick { 3 } else { 20 });
    let first = number("--first")?.unwrap_or(0);
    let jobs = number("--jobs")?
        .map(|j| j as usize)
        .unwrap_or_else(|| {
            std::thread::available_parallelism().map_or(2, |n| (n.get() / 3).clamp(1, 4))
        })
        .max(1);
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let threads_per_child = std::thread::available_parallelism()
        .map_or(2, |n| n.get() / jobs)
        .max(2);
    let started = Instant::now();
    let queue = Arc::new(Mutex::new(
        (first..first + seeds).collect::<VecDeque<u64>>(),
    ));
    let results = Arc::new(Mutex::new(Vec::<(u64, Result<String, String>)>::new()));
    let slow = if cfg!(debug_assertions) { 10 } else { 1 };
    let seed_timeout = Duration::from_secs((60 + steps as u64 * 2) * slow);
    let threads: Vec<_> = (0..jobs)
        .map(|_| {
            let queue = queue.clone();
            let results = results.clone();
            let exe = exe.clone();
            let logs = logs.clone();
            let verbose = limits.verbose;
            let new_features = limits.new_features;
            let cancel_after = limits.cancel_after;
            std::thread::spawn(move || {
                while let Some(seed) = queue.lock().ok().and_then(|mut q| q.pop_front()) {
                    let mut cmd = std::process::Command::new(&exe);
                    cmd.env("MICA_CRASH_LOG_DIR", &logs)
                        .env(mica_core::user_data::ISOLATED_VAR, "1");
                    // Seeds run side by side: share the cores instead of oversubscribing them
                    // (which would stretch cancel latencies past the hang timeout).
                    if std::env::var_os("RAYON_NUM_THREADS").is_none() {
                        cmd.env("RAYON_NUM_THREADS", threads_per_child.to_string());
                    }
                    cmd.args(["--monkey", "--seed", &seed.to_string()])
                        .args(["--steps", &steps.to_string()]);
                    if quick {
                        cmd.arg("--quick");
                    }
                    if let Some(d) = cancel_after {
                        cmd.args(["--cancel-after", &d.as_millis().to_string()]);
                    }
                    if verbose {
                        cmd.arg("--verbose");
                    }
                    if !new_features {
                        cmd.arg("--no-new-features");
                    }
                    let outcome = run_child(cmd, seed_timeout);
                    match &outcome {
                        Ok(line) => eprintln!("{line}"),
                        Err(report) => eprintln!("{report}"),
                    }
                    if let Ok(mut r) = results.lock() {
                        r.push((seed, outcome));
                    }
                }
            })
        })
        .collect();
    for t in threads {
        let _ = t.join();
    }
    let results = results.lock().map_err(|e| e.to_string())?;
    let failed: Vec<u64> = results
        .iter()
        .filter(|(_, r)| r.is_err())
        .map(|(s, _)| *s)
        .collect();
    let total = |key: &str| -> f64 {
        results
            .iter()
            .filter_map(|(_, r)| r.as_ref().ok())
            .filter_map(|line| {
                line.split_whitespace()
                    .find_map(|kv| kv.strip_prefix(&format!("{key}=")))
                    .and_then(|v| v.parse::<f64>().ok())
            })
            .sum()
    };
    let peak = results
        .iter()
        .filter_map(|(_, r)| r.as_ref().ok())
        .filter_map(|line| {
            line.split_whitespace()
                .find_map(|kv| kv.strip_prefix("rss_peak="))
                .and_then(|v| v.parse::<f64>().ok())
        })
        .fold(0., f64::max);
    eprintln!(
        "Monkey: {} seeds × {steps} steps in {:.0} s · {} failed · {:.0} steps · {:.0} slices started, {:.0} completed, {:.0} validated (0 errors), {:.0} slice errors, {:.0} cancels · {:.0} undo checks ({:.0} cached slices compared with fresh ones) · {:.0} save/load round trips · {:.0} UI frames · {:.0}/{:.0} Mica MCP calls answered · peak RSS {peak:.0} MB",
        results.len(),
        started.elapsed().as_secs_f64(),
        failed.len(),
        total("steps"),
        total("slices_started"),
        total("completed"),
        total("validated"),
        total("errors"),
        total("cancels"),
        total("undo_checks"),
        total("cache_checks"),
        total("round_trips"),
        total("ui_frames"),
        total("mcp_answered"),
        total("mcp_calls"),
    );
    if failed.is_empty() {
        Ok(())
    } else {
        // One line that names each failure (gate scripts often keep only the last line).
        let reasons: Vec<String> = results
            .iter()
            .filter_map(|(seed, r)| r.as_ref().err().map(|report| (seed, report)))
            .map(|(seed, report)| {
                let line = report
                    .lines()
                    .find(|l| l.starts_with("MONKEY FAILURE"))
                    .or_else(|| report.lines().next())
                    .unwrap_or_default();
                format!("seed {seed}: {}", line.trim_end_matches(':'))
            })
            .collect();
        Err(format!(
            "monkey seeds failed: {failed:?} ({})",
            reasons.join("; ")
        ))
    }
}

/// Runs one seed's child process with a wall-clock limit; returns its result line or the
/// failure report (including hangs and crashes `catch_unwind` cannot contain).
fn run_child(mut cmd: std::process::Command, timeout: Duration) -> Result<String, String> {
    use std::io::Read;
    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit());
    let mut child = cmd.spawn().map_err(|e| e.to_string())?;
    let mut stdout = child.stdout.take().ok_or("no stdout")?;
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stdout.read_to_string(&mut s);
        s
    });
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break Some(status);
        }
        if started.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let out = reader.join().unwrap_or_default();
    match status {
        Some(s) if s.success() => out
            .lines()
            .find(|l| l.starts_with("MONKEY-OK"))
            .map(str::to_string)
            .ok_or_else(|| format!("no result line:\n{out}")),
        Some(s) => Err(format!("child exited with {s}:\n{out}")),
        None => Err(format!(
            "seed timed out after {:.0} s (hang):\n{out}",
            timeout.as_secs_f64()
        )),
    }
}

/// Aborts the process with the trace when the trace stops moving for longer than `limit` (a
/// hang on the UI thread, which no `catch_unwind` can see).
fn spawn_watchdog(seed: u64, limit: Duration, trace: Arc<Mutex<VecDeque<String>>>) {
    std::thread::spawn(move || {
        let newest = |t: &Arc<Mutex<VecDeque<String>>>| {
            t.lock()
                .ok()
                .and_then(|t| t.back().cloned())
                .unwrap_or_default()
        };
        let mut last = (newest(&trace), Instant::now());
        loop {
            std::thread::sleep(Duration::from_secs(1));
            let now = newest(&trace);
            if now != last.0 {
                last = (now, Instant::now());
            } else if last.1.elapsed() > limit {
                let lines: Vec<String> = trace
                    .lock()
                    .map(|t| t.iter().cloned().collect())
                    .unwrap_or_default();
                println!(
                    "MONKEY FAILURE seed {seed}: hang\nno progress for {:.0} s after:\n  {}",
                    last.1.elapsed().as_secs_f64(),
                    lines.join("\n  ")
                );
                std::process::exit(3);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Same seed, same actions.
    #[test]
    fn rng_is_reproducible() {
        let mut a = Rng::new(7);
        let mut b = Rng::new(7);
        for _ in 0..100 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
        assert!(Rng::new(1).below(10) < 10);
    }

    /// Generated values parse for every editable key of every tab.
    #[test]
    fn random_values_parse() {
        let mut rng = Rng::new(3);
        for tab in [Tab::Print, Tab::Filament, Tab::Printer] {
            for key in tab_keys(tab) {
                for _ in 0..4 {
                    if let Some(v) = random_value(&mut rng, key) {
                        let mut c = Config::defaults();
                        // Unparseable values are allowed (the editor reports them); the
                        // generator must not produce empty keys or panic.
                        let _ = c.set_raw(key, &v);
                    }
                }
            }
        }
    }

    /// The plausible range of a temperature ends where the validator's does: a 300 °C bed
    /// (allowed by the option table) is implausible, as the validator rejects it (seed 3015).
    #[test]
    fn plausible_temperatures_follow_the_validator() {
        for key in ["first_layer_bed_temperature", "bed_temperature"] {
            assert!(implausible(key, "300"), "{key}");
            assert!(implausible(key, "60,151"), "{key}");
            assert!(!implausible(key, "150"), "{key}");
        }
        assert!(!implausible("temperature", "300"));
        assert!(implausible("temperature", "501"));
    }

    /// A short run of the monkey (debug builds are slow: few steps). The full run is
    /// `cargo xtask monkey` (see docs/quality-safety.md).
    #[test]
    fn monkey_short_run() {
        let mut limits = Limits::new(40);
        limits.undo_check_every = 20;
        limits.slice_wait = Duration::from_secs(60);
        // Other tests of this process slice too: their folders are not this run's leaks.
        limits.check_leaks = false;
        let trace = Arc::new(Mutex::new(VecDeque::new()));
        if let Err(f) = run_seed(1, &limits, trace) {
            panic!("{f}");
        }
    }

    /// The long run, for `cargo test -- --ignored` (prefer `cargo xtask monkey`, release).
    #[test]
    #[ignore]
    fn monkey_long_run() {
        let mut limits = Limits::new(500);
        limits.check_leaks = false;
        for seed in 0..4 {
            let trace = Arc::new(Mutex::new(VecDeque::new()));
            if let Err(f) = run_seed(seed, &limits, trace) {
                panic!("{f}");
            }
        }
    }
}
