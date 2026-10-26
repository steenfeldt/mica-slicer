// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! An opened G-code file (`.gcode` or `.bgcode`) as a read-only document: File › Open, drag
//! and drop and the Library's Open load it on a thread ([`Loading`]) into the same disk-backed
//! preview a slice makes (`parse.rs`), and the window shows it in Preview until it is closed,
//! with the scene left exactly as it was underneath. The file itself is only ever read: Print
//! sends its bytes as they are, Save copy… copies them.
//!
//! [`Origin`] is what the file says about the printer it was made for (PrusaSlicer's and
//! Orca's configuration block, the binary file's printer metadata, `M862.3` checks, Mica's own
//! Snapmaker header); the Print dialog pre-selects a printer from it and warns when the chosen
//! one is a different model. Mica's validator runs on the file after it opens ([`Check`]).

pub(crate) mod app;
pub mod parse;

use crate::preview::legend;
use crate::worker::{Finished, Summary, TempJob, ToolUsage};
use mica_core::config::Config;
use mica_gcode::validate::{Report, Severity};
use parse::{Invalid, Scan};
use std::{
    io::{BufReader, Read},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
};

/// Whether a path names a G-code file by its extension.
pub fn is_gcode_path(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        matches!(
            e.to_ascii_lowercase().as_str(),
            "gcode" | "bgcode" | "gco" | "g"
        )
    })
}

/// The printer a file was made for, as far as it says.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Origin {
    /// `PrusaSlicer 2.9.6 on 2026-10-15 at 19:30:58 UTC`, `Mica Slicer 0.1`.
    pub generator: Option<String>,
    /// `Prusa` or `Snapmaker` (the printer host families Mica knows), when it can tell.
    pub vendor: Option<&'static str>,
    /// `printer_settings_id`: the printer preset it was sliced with.
    pub preset: Option<String>,
    /// `printer_model` (`COREONE`, `XL5IS`, `Snapmaker U1`) or the `M862.3` check.
    pub model: Option<String>,
    /// Nozzle diameter (`0.4`), from the configuration or the `M862.1` check.
    pub nozzle: Option<String>,
}

fn unquote(v: &str) -> String {
    v.trim().trim_matches('"').trim().to_string()
}

impl Origin {
    pub fn detect(scan: &Scan) -> Self {
        let value = |k: &str| {
            scan.values
                .get(k)
                .map(|v| unquote(v))
                .filter(|v| !v.is_empty())
        };
        let generator = scan.generator.clone();
        let preset = value("printer_settings_id");
        let mut model = value("printer_model").or_else(|| scan.printer_check.clone());
        // Mica's U1 output carries Snapmaker's compatibility header and nothing else.
        if model.is_none()
            && generator
                .as_deref()
                .is_some_and(|g| g.contains("Snapmaker Orca") && g.contains("Mica Slicer"))
        {
            model = Some("Snapmaker U1".into());
        }
        let nozzle = value("printer_variant")
            .or_else(|| {
                value("nozzle_diameter")
                    .and_then(|v| v.split([',', ';']).next().map(|s| s.trim().to_string()))
            })
            .or_else(|| scan.nozzle_check.clone());
        let says = |needle: &str| {
            [&generator, &preset, &model]
                .into_iter()
                .flatten()
                .any(|t| t.to_ascii_lowercase().contains(needle))
        };
        let vendor = if says("snapmaker") || model.as_deref().is_some_and(|m| m.contains("U1")) {
            Some("Snapmaker")
        } else if says("prusa") || scan.printer_check.is_some() {
            Some("Prusa")
        } else {
            None
        };
        Self {
            generator,
            vendor,
            preset,
            model,
            nozzle,
        }
    }
    /// Whether the file names its printer at all.
    pub fn known(&self) -> bool {
        self.preset.is_some() || self.model.is_some()
    }
}

/// The printer model a file names, in words, with the preset and model name it resolves to
/// among the installed profiles (`App::gcode_printer`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Printer {
    /// `Original Prusa CORE One`, `Snapmaker U1`: the model part of the preset name, or the
    /// bundle's name for a model code.
    pub model_name: Option<String>,
    /// The installed printer preset that matches, `(vendor index, name)`.
    pub preset: Option<(usize, String)>,
}

impl Printer {
    /// What to call the file's printer in messages.
    pub fn label(&self, origin: &Origin) -> String {
        self.model_name
            .clone()
            .or_else(|| origin.preset.clone())
            .or_else(|| origin.model.clone())
            .unwrap_or_else(|| "an unknown printer".into())
    }
}

/// How a physical printer fits the file: the profile ranking of `printhost::config::fit` with
/// the file's printer as the profile.
pub fn fit(
    printer: &crate::printhost::config::Printer,
    origin: &Origin,
    resolved: &Printer,
) -> Option<crate::printhost::config::Fit> {
    let vendor = origin.vendor?;
    let model = resolved
        .model_name
        .clone()
        .or_else(|| origin.model.clone())?;
    let preset = resolved
        .preset
        .as_ref()
        .map(|(_, n)| n.clone())
        .or_else(|| origin.preset.clone())
        .unwrap_or_else(|| model.clone());
    Some(crate::printhost::config::fit(
        printer, vendor, &preset, &model,
    ))
}

/// A warning when `printer` is not the model the file was made for (`None`: it is, or
/// nothing tells).
pub fn mismatch(
    printer: &crate::printhost::config::Printer,
    origin: &Origin,
    resolved: &Printer,
) -> Option<String> {
    use crate::printhost::config::Fit;
    let fit = fit(printer, origin, resolved)?;
    let label = resolved.label(origin);
    match fit {
        Fit::Linked | Fit::Model => None,
        Fit::Other => Some(format!(
            "This file was made for {label}; {} is a {} printer. Printing it there may damage the printer or fail.",
            printer.name,
            printer.kind.label()
        )),
        // Same family: only a printer that says what it is can be told apart.
        Fit::Family => {
            let theirs = printer.model.clone().or_else(|| {
                printer
                    .profile
                    .as_deref()
                    .and_then(|p| p.split_once('/'))
                    .map(|(_, preset)| mica_profiles::project::split_printer_name(preset).0)
            })?;
            Some(format!(
                "This file was made for {label}, but {} is set up as {theirs}. Check that the file fits this printer before printing.",
                printer.name
            ))
        }
    }
}

/// The validator's verdict on the opened file.
#[derive(Clone, Debug)]
pub enum Check {
    Running,
    Done {
        errors: usize,
        warnings: usize,
        /// The first diagnostics (`line: message`), errors first.
        details: Vec<(Severity, String)>,
        /// The profile it was checked against.
        against: String,
    },
    Failed(String),
}

impl Check {
    pub fn from_report(report: &Report, against: String) -> Self {
        let mut details: Vec<(Severity, String)> = report
            .diagnostics
            .iter()
            .map(|d| (d.severity, format!("Line {}: {}", d.line, d.message)))
            .collect();
        details.sort_by_key(|(s, _)| *s != Severity::Error);
        details.truncate(40);
        Check::Done {
            errors: report.errors,
            warnings: report.warnings,
            details,
            against,
        }
    }
    pub fn errors(&self) -> usize {
        match self {
            Check::Done { errors, .. } => *errors,
            _ => 0,
        }
    }
}

/// An opened G-code file.
pub struct Doc {
    pub path: PathBuf,
    /// The file name as shown (`benchy.gcode`).
    pub name: String,
    /// The preview, totals and format, shaped as a slice's so Preview shows it unchanged.
    pub finished: Finished,
    pub scan: Scan,
    pub origin: Origin,
    pub printer: Printer,
    /// Tool colours: the file's, else Mica's defaults.
    pub colours: Vec<[u8; 3]>,
    /// `filament_type` per tool, when the file says.
    pub materials: Vec<String>,
    /// `filament_settings_id` per tool, when the file says.
    pub filament_names: Vec<String>,
    pub check: Check,
    pub check_rx: Option<mpsc::Receiver<Check>>,
    /// The bed of the file's printer when it is not the selected one (else the scene's).
    pub bed: Option<crate::scene::bed::Bed>,
    /// What the window showed before, restored on close.
    pub restore: Restore,
}

/// The one plate an opened G-code file shows on: at the origin, current.
pub fn single_plate() -> Vec<crate::scene::plates::Plate> {
    vec![crate::scene::plates::Plate {
        index: 0,
        origin: glam::Vec2::ZERO,
        current: true,
        name: None,
        locked: false,
        objects: 0,
        alert: 0,
        notes: Vec::new(),
        status: crate::plate_batch::PlateStatus::Empty,
    }]
}

/// The scene's Preview state while a G-code file covers it.
#[derive(Clone, Debug)]
pub struct Restore {
    pub preview: bool,
    pub view: crate::preview::View,
}

impl Doc {
    /// The tools the file prints with (extrusion records), in order.
    pub fn used_tools(&self) -> Vec<usize> {
        let tools: Vec<usize> = (0..16)
            .filter(|&t| self.finished.preview.stats.tools[t].count > 0)
            .collect();
        if tools.is_empty() { vec![0] } else { tools }
    }
}

/// `#RRGGBB` lists (`;` or `,` separated; empty entries stay `None`).
pub fn colours_of(value: &str) -> Vec<Option<[u8; 3]>> {
    unquote(value)
        .split([';', ','])
        .map(|c| mica_profiles::project::parse_colour(unquote(c).trim()))
        .collect()
}

/// A `;`-separated string list (`filament_type = PLA;PETG`, `"Prusament PLA";"Generic PETG"`).
pub fn strings_of(value: &str) -> Vec<String> {
    value.split(';').map(unquote).collect()
}

/// Tool colours of a file: `extruder_colour` where set, else `filament_colour`, else the
/// preview's default per tool (16 entries).
pub fn file_colours(scan: &Scan) -> Vec<[u8; 3]> {
    let get = |k: &str| {
        scan.values
            .get(k)
            .map(|v| colours_of(v))
            .unwrap_or_default()
    };
    let extruder = get("extruder_colour");
    let filament = get("filament_colour");
    let default = get("default_filament_colour");
    (0..16)
        .map(|t| {
            extruder
                .get(t)
                .copied()
                .flatten()
                .or_else(|| filament.get(t).copied().flatten())
                .or_else(|| default.get(t).copied().flatten())
                .unwrap_or(legend::TOOLS[t % legend::TOOLS.len()])
        })
        .collect()
}

/// Numbers of a footer value: `1.2, 3.4` → [1.2, 3.4].
fn numbers(v: &str) -> Vec<f64> {
    v.split([',', ';'])
        .filter_map(|n| n.trim().parse().ok())
        .collect()
}

/// The file's totals as the slice summary has them: time, layers, filament per tool (the
/// footer's figures where it has them, else the moves'), weight and cost.
pub fn summary(scan: &Scan, parsed: &parse::Parsed, layers: usize, time_s: f64) -> Summary {
    let total_mm = scan
        .footer("filament used [mm]")
        .map(|v| numbers(v).iter().sum::<f64>())
        .filter(|v| *v > 0.)
        .unwrap_or_else(|| parsed.extruded_mm.iter().filter(|v| **v > 0.).sum());
    let tail = scan.tail_text();
    let mut per_tool = if tail.contains("; filament used T") {
        crate::worker::per_tool_filament(&tail, total_mm)
    } else {
        Vec::new()
    };
    if per_tool.is_empty() {
        let used: Vec<(usize, f64)> = parsed
            .extruded_mm
            .iter()
            .enumerate()
            .filter(|(_, mm)| **mm > 0.01)
            .map(|(t, mm)| (t, *mm))
            .collect();
        // The footer's per-extruder list (PrusaSlicer) wins over the moves when it fits.
        let listed = scan
            .footer("filament used [mm]")
            .map(numbers)
            .unwrap_or_default();
        per_tool = if listed.len() > 1 {
            listed
                .into_iter()
                .enumerate()
                .filter(|(_, mm)| *mm > 0.)
                .collect()
        } else if used.len() == 1 {
            vec![(used[0].0, total_mm)]
        } else {
            used
        };
    }
    let value_list = |k: &str| scan.values.get(k).map(|v| numbers(v)).unwrap_or_default();
    let density = value_list("filament_density");
    let diameter = value_list("filament_diameter");
    let grams_listed = scan
        .footer("filament used [g]")
        .map(numbers)
        .unwrap_or_default();
    let names = scan
        .values
        .get("filament_settings_id")
        .map(|v| strings_of(v))
        .unwrap_or_default();
    let tools: Vec<ToolUsage> = per_tool
        .iter()
        .map(|&(tool, mm)| {
            let exact = scan
                .footer(&format!("filament used T{tool} [g]"))
                .and_then(|v| v.parse::<f64>().ok());
            let listed = (grams_listed.len() > 1)
                .then(|| grams_listed.get(tool).copied())
                .flatten();
            let estimated = || {
                let d = diameter
                    .get(tool)
                    .or(diameter.first())
                    .copied()
                    .unwrap_or(1.75);
                let rho = density.get(tool).or(density.first()).copied().unwrap_or(0.);
                mm * std::f64::consts::PI * (d / 2.).powi(2) / 1000. * rho
            };
            ToolUsage {
                tool,
                filament_mm: mm,
                grams: exact.or(listed).unwrap_or_else(estimated),
                cost: 0.,
                filament: names.get(tool).cloned().unwrap_or_default(),
            }
        })
        .collect();
    let grams = scan
        .footer("total filament used [g]")
        .and_then(|v| v.parse::<f64>().ok())
        .or_else(|| (grams_listed.len() == 1).then(|| grams_listed[0]))
        .unwrap_or_else(|| tools.iter().map(|t| t.grams).sum());
    let cost = scan
        .footer("total filament cost")
        .or_else(|| scan.footer("filament cost"))
        .map(|v| numbers(v).iter().sum())
        .unwrap_or(0.);
    Summary {
        time_s,
        silent_time_s: None,
        layers,
        filament_mm: total_mm,
        grams,
        cost,
        tools,
    }
}

/// A finished load, before the app shapes it into a [`Doc`].
pub struct Loaded {
    pub path: PathBuf,
    pub binary: bool,
    pub scan: Scan,
    pub parsed: parse::Parsed,
}

pub enum Event {
    /// Overall fraction and what is happening.
    Progress(f32, String),
    Done(Box<Result<Loaded, String>>),
}

/// A G-code file being opened on its thread.
pub struct Loading {
    pub path: PathBuf,
    pub rx: mpsc::Receiver<Event>,
    pub cancel: Arc<AtomicBool>,
    pub progress: f32,
    pub stage: String,
    pub started: std::time::Instant,
}

impl Drop for Loading {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// Starts opening `path`; `config` gives the machine limits for planned speeds.
pub fn start(path: PathBuf, config: Config, wake: impl Fn() + Send + 'static) -> Loading {
    let (tx, rx) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let token = cancel.clone();
    let p = path.clone();
    std::thread::spawn(move || {
        let report = |fraction: f32, stage: &str| {
            let _ = tx.send(Event::Progress(fraction, stage.to_string()));
            wake();
        };
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            load(&p, &config, &token, &report)
        }))
        .unwrap_or_else(|_| Err("Mica could not read this file (an internal error)".into()));
        let _ = tx.send(Event::Done(Box::new(result)));
        wake();
    });
    Loading {
        path,
        rx,
        cancel,
        progress: 0.,
        stage: "Opening".into(),
        started: std::time::Instant::now(),
    }
}

/// Opens a G-code file into a new preview job folder (the work of [`start`]'s thread).
pub fn load(
    path: &Path,
    config: &Config,
    cancel: &AtomicBool,
    report: &dyn Fn(f32, &str),
) -> Result<Loaded, String> {
    let text = |e: Invalid| e.0;
    let mut file =
        std::fs::File::open(path).map_err(|e| format!("Mica could not open the file: {e}"))?;
    let size = file.metadata().map(|m| m.len()).unwrap_or(0);
    let mut magic = [0u8; 4];
    let got = file.read(&mut magic).unwrap_or(0);
    let binary = got == 4 && &magic == b"GCDE";
    let named_binary = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("bgcode"));
    if named_binary && !binary {
        return Err(
            "This is not a binary G-code file: it does not start with the bgcode signature".into(),
        );
    }
    drop(file);
    let job = Arc::new(
        TempJob::new().map_err(|e| format!("Mica could not make a temporary folder: {e}"))?,
    );
    let (source, metadata, span) = if binary {
        report(0., "Unpacking");
        let out = job.path.join("decoded.gcode");
        let meta = parse::decode_binary(path, &out, cancel, |read| {
            report(0.3 * read as f32 / size.max(1) as f32, "Unpacking")
        })
        .map_err(text)?;
        (out, meta, 0.3)
    } else {
        (path.to_path_buf(), Default::default(), 0.)
    };
    let text_size = std::fs::metadata(&source)
        .map(|m| m.len())
        .unwrap_or(1)
        .max(1);
    let open = || {
        std::fs::File::open(&source)
            .map(|f| BufReader::with_capacity(1 << 20, f))
            .map_err(|e| format!("Mica could not open the file: {e}"))
    };
    let scan_share = 0.15;
    let mut scan = parse::scan(open()?, cancel, |read| {
        report(
            span + (1. - span) * scan_share * read as f32 / text_size as f32,
            "Reading",
        )
    })
    .map_err(text)?;
    scan.binary = binary;
    for (k, v) in metadata {
        scan.values.entry(k).or_insert(v);
    }
    let options = parse::Options {
        filament_diameter: scan
            .values
            .get("filament_diameter")
            .and_then(|v| numbers(v).first().copied())
            .filter(|d| *d > 0.5)
            .unwrap_or(1.75),
        machine: crate::preview::attrs::Machine::from_config(config),
        config: config.clone(),
        unknown: parse::unknown_slot(&scan.types),
        layer_tags: scan.layer_tags > 0,
    };
    let start = span + (1. - span) * scan_share;
    // The file's own estimate (footer, M73), else Mica's time estimator over it.
    let time = scan.file_time_s().or_else(|| {
        report(start, "Estimating the print time");
        let mut reader = open().ok()?;
        mica_gcode::time_estimate::estimate(&mut reader, config)
            .ok()
            .map(|e| e.normal)
            .filter(|t| t.is_finite() && *t > 0.)
    });
    let parsed = parse::parse(open()?, &job, &options, time, cancel, |read| {
        report(
            start + (1. - start) * read as f32 / text_size as f32,
            "Building the preview",
        )
    })
    .map_err(text)?;
    report(1., "Done");
    Ok(Loaded {
        path: path.to_path_buf(),
        binary,
        scan,
        parsed,
    })
}

impl Loaded {
    /// The document: preview, totals and what the file says about itself.
    pub fn into_doc(self, restore: Restore) -> Doc {
        let Loaded {
            path,
            binary,
            scan,
            parsed,
        } = self;
        let preview = &parsed.preview;
        let time = preview.total_time();
        let layers = preview.layers.len();
        let summary = summary(&scan, &parsed, layers, time);
        let stats = mica_gcode::writer::MoveStats {
            filament_mm: summary.filament_mm,
            time_s: time,
            ..Default::default()
        };
        let origin = Origin::detect(&scan);
        let colours = file_colours(&scan);
        let list = |k: &str| {
            scan.values
                .get(k)
                .map(|v| strings_of(v))
                .unwrap_or_default()
        };
        let materials = list("filament_type");
        let filament_names = list("filament_settings_id");
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "G-code".into());
        let parsed_preview = parsed.preview;
        Doc {
            path,
            name,
            finished: Finished {
                preview: Arc::new(parsed_preview),
                stats,
                binary,
                summary,
                tower: None,
                warnings: Vec::new(),
            },
            scan,
            origin,
            printer: Printer::default(),
            colours,
            materials,
            filament_names,
            check: Check::Running,
            check_rx: None,
            bed: None,
            restore,
        }
    }
}

/// Runs Mica's validator on the file with `profile` on a thread.
pub fn start_check(
    path: PathBuf,
    profile: Result<mica_gcode::validate::Profile, String>,
    against: String,
    wake: impl Fn() + Send + 'static,
) -> mpsc::Receiver<Check> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let check = match profile {
            Err(e) => Check::Failed(format!("No printer profile to check against: {e}")),
            Ok(profile) => std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                mica_gcode::validate::validate_file(&path, &profile)
            }))
            .map_err(|_| "the validator stopped unexpectedly".to_string())
            .and_then(|r| r.map_err(|e| e.to_string()))
            .map_or_else(
                |e| Check::Failed(format!("The file could not be checked: {e}")),
                |report| Check::from_report(&report, against),
            ),
        };
        let _ = tx.send(check);
        wake();
    });
    rx
}

#[cfg(test)]
mod app_tests;
#[cfg(test)]
mod tests;
