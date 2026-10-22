// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original
// Source (original): Mica CLI argument parsing and profile selection.
// Changes: Track explicit profile flags so project printer detection preserves user choices.

//! `mica`: the Mica Slicer command line, as the library the `mica` binary wraps.

mod calibrate;
mod custom_gcode;
pub mod mcp;
mod overrides;
mod preview;
mod printer_cmd;
mod profile_cmd;
mod project_export;
mod project_settings;
mod validate;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use mica_core::geometry::BoundingBox;
use mica_core::print::LayerSource;
use mica_core::print_job::PreparedPrint;
use mica_core::settings::PrintSettings;
use mica_profiles::orca;
use mica_profiles::prusa::{Bundle, Kind};

const USAGE: &str = "\
Mica Slicer

Usage:
  mica slice <model.stl|model.3mf> [more models...] [options]   (slice --help: this text)
  mica calibrate <kind> [options] -o out.gcode (calibrate --help for kinds)
  mica validate <file.gcode|file.bgcode> [profile options] [--json] [--estimate]
  mica profiles [printer|print|filament] [--vendor <name>] [options]
  mica profiles fetch   download the Prusa printer profiles from Prusa (once; checked by SHA-256)
  mica profile …   (your own profiles; see below)
  mica print <file.gcode|file.bgcode> --printer-host <name> [--upload-only | --queue]
  mica printers status <name>
  mica mcp [--live|--headless] [model] [profile options]  Mica MCP server on stdio (the open Mica window when one answers); first capability: color painting (docs/mcp.md)
  mica --version
  mica --help

Slice options:
  --printer   printer preset   (default: Prusa CORE One HF0.4 nozzle)
  --print     print preset     (default: 0.20mm SPEED @COREONE HF0.4)
  --filament  filament preset  (default: Prusament PLA @COREONE HF0.4)
  --vendor    Prusa (default), or an Orca vendor index name, e.g. Snapmaker; Orca:Prusa is
              OrcaSlicer's own Prusa profiles (CORE One, XL 5T, …)
  --slicing-rules prusaslicer|orcaslicer  which slicer's behavior to follow (walls, seams,
              travel, cooling, tool changes); default: PrusaSlicer for Prusa bundle printers,
              OrcaSlicer for Orca profiles. Same as --set mica_slicing_rules=…
  --profiles-dir Orca resources/profiles directory (default: installed OrcaSlicer, then checkout)
  --bed       Orca plate type, e.g. \"High Temp Plate\" (default: machine default_bed_type)
  --bundle    Prusa vendor bundle (default: the one `mica profiles fetch` downloaded from Prusa,
              else your PrusaSlicer installation's)
  --gcode-identity mica|orca-compatible  generator identity (U1 defaults to orca-compatible)
  -o          output file; .bgcode writes Prusa binary G-code (default: <model>.gcode or .bgcode,
              as the printer profile's binary_gcode setting says, next to the model)
  --center    place model center at x,y, mm (STL default: bed center; 3MF: stored placement)
  --all-objects slice all 3MF objects/instances (default; --first-object selects the first)
  --plate N   slice one plate, one based (default: plate 1 in multi-plate 3MFs)
  --all-plates retain all plates in one file at their global placement
  --each-plate one file per plate into the folder -o names, named as Mica's Export all plates
              names them (first object and plate name: cube20_plate_2.bgcode)
  --extruder  default tool, one based (default: 1)
  --object-extruder i=n override object i with tool n, one based (repeatable)
  --perimeter-generator classic|arachne (overrides the profile)
  --preview   write PNG images of the chosen layers to <dir> (layer numbers start at 1)
  --layers    which layers to preview, e.g. 1,2,50
  --set       override a profile setting, e.g. --set support_material=1 (repeatable)
  --texture-image FILE  a grayscale PNG as the surface texture (mica_texture_skin_texture);
              --export-3mf embeds it in the project
  --export-3mf save the loaded project to this path (also slice when -o is supplied)
  --project-settings use|update|ignore  a 3MF's own settings (default: use for a matching
              printer, otherwise ignore). use: slice with the project's saved values, as
              OrcaSlicer/PrusaSlicer do; update: today's presets plus only the values the
              project marks as changed. --print/--filament/--bed naming other presets than
              the project's replace the project's values for that scope
  --flavour   orca (default) or prusa; requires --export-3mf
{CUSTOM_GCODE_USAGE}
  --adaptive-layers Q  with --export-3mf: give every object an adaptive layer height
              profile, quality 0 (fine) to 1 (fast), as the variable layer height tool
  --smooth-layers R    smooth that profile over R points (1-10)
  --brim-ear X,Y,R     with --export-3mf: a hand-placed brim ear (bed mm), for
              brim_type=painted (repeatable)

Your profiles (stored in Mica's settings folder, selectable with --print/--filament/--printer;
Prusa-format profiles for now):
{PROFILE_USAGE}

G-code output: Marlin 2 / Prusa and Klipper; multiple tools with wipe/prime towers.
Snapmaker defaults: U1 0.4 nozzle, 0.20 Standard, Generic PLA @System.
profiles also accepts --vendor, --profiles-dir, --printer and --print for compatibility filtering.";

const DEFAULT_PRINTER: &str = "Prusa CORE One HF0.4 nozzle";
const DEFAULT_PRINT: &str = "0.20mm SPEED @COREONE HF0.4";
const DEFAULT_FILAMENT: &str = "Prusament PLA @COREONE HF0.4";

/// Prusa's vendor profiles (Mica does not redistribute them, see PROVENANCE.md): `--bundle`,
/// `MICA_PRUSA_BUNDLE`, a development checkout's pinned copy, the copy `mica profiles fetch`
/// downloaded from Prusa, else the user's PrusaSlicer installation
/// (`mica_profiles::vendor_profiles::prusa::find_bundle`).
fn find_bundle(explicit: Option<&Path>) -> Result<PathBuf, String> {
    mica_profiles::vendor_profiles::prusa::find_bundle(explicit).ok_or_else(|| {
        "the Prusa printer profiles are not on this computer yet: run `mica profiles fetch` \
         to download them from Prusa (once), or pass --bundle"
            .to_string()
    })
}

fn load_bundle(explicit: Option<&Path>) -> Result<(Bundle, PathBuf), String> {
    let path = find_bundle(explicit)?;
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut bundle = Bundle::parse(&text);
    mica_profiles::user::Store::open(mica_profiles::user::Store::default_root())
        .register_all(&mut bundle)
        .map_err(|e| e.to_string())?;
    Ok((bundle, path))
}

/// `--project-settings`: how a 3MF's own settings are used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProjectMode {
    /// The project's saved values (its whole embedded configuration).
    Use,
    /// Today's presets plus the values the project lists as changed.
    Update,
    Ignore,
}

#[derive(Clone)]
struct SliceArgs {
    printer_explicit: bool,
    vendor_explicit: bool,
    print_explicit: bool,
    filament_explicit: bool,
    project_settings: Option<ProjectMode>,
    export_3mf: Option<PathBuf>,
    flavour: Option<mica_core::threemf::Flavour>,
    model: PathBuf,
    extra_models: Vec<PathBuf>,
    extruder: usize,
    object_extruders: std::collections::BTreeMap<usize, usize>,
    output: Option<PathBuf>,
    center: Option<(f32, f32)>,
    all_objects: bool,
    plate: mica_core::input::PlateSelection,
    /// `--each-plate`: one file per plate into the folder `output` names.
    each_plate: bool,
    /// Set by `--each-plate` for each plate's run: the folder, the project's plate count, this
    /// plate's name and the file stems already written (`mica_core::export_name`).
    each: Option<EachPlate>,
    preview: Option<PathBuf>,
    layers: Vec<usize>,
    printer: String,
    print: String,
    filament: String,
    bundle: Option<PathBuf>,
    vendor: String,
    profiles_dir: Option<PathBuf>,
    bed: Option<String>,
    overrides: Vec<(String, String)>,
    perimeter_generator: Option<String>,
    /// Per-layer custom G-code from the command line (added to the project's).
    custom_gcode: Vec<mica_core::custom_gcode::Item>,
    adaptive_layers: Option<f32>,
    smooth_layers: Option<u32>,
    brim_ears: Vec<[f64; 3]>,
    /// `--slicing-rules`.
    slicing_rules: Option<mica_core::rules::Rules>,
    /// Settings › Features of Mica's settings folder (`[features]` in mica.toml), as the
    /// GUI slices with them.
    features: mica_profiles::features::Features,
}

impl SliceArgs {
    fn vendor_defaults(&mut self) {
        if self.vendor == "Orca:Prusa" {
            if !self.printer_explicit {
                self.printer = "Prusa CORE One HF 0.4 nozzle".into();
            }
            if !self.print_explicit {
                self.print = "0.20mm SPEED @CORE One HF 0.4".into();
            }
            if !self.filament_explicit {
                self.filament = "Prusament PLA @CORE One HF 0.4".into();
            }
        }
        if self.vendor == "Snapmaker" {
            if !self.printer_explicit {
                self.printer = "Snapmaker U1 (0.4 nozzle)".into();
            }
            if !self.print_explicit {
                self.print = "0.20 Standard @Snapmaker U1 (0.4 nozzle)".into();
            }
            if !self.filament_explicit {
                self.filament = "Generic PLA @System".into();
            }
        }
    }
}

fn parse_slice(args: &[String]) -> Result<SliceArgs, String> {
    let mut it = args.iter();
    let mut a = SliceArgs {
        printer_explicit: false,
        vendor_explicit: false,
        print_explicit: false,
        filament_explicit: false,
        project_settings: None,
        export_3mf: None,
        flavour: None,
        model: PathBuf::new(),
        extra_models: Vec::new(),
        extruder: 1,
        object_extruders: std::collections::BTreeMap::new(),
        output: None,
        center: None,
        all_objects: true,
        plate: mica_core::input::PlateSelection::Auto,
        each_plate: false,
        each: None,
        preview: None,
        layers: vec![],
        printer: DEFAULT_PRINTER.into(),
        print: DEFAULT_PRINT.into(),
        filament: DEFAULT_FILAMENT.into(),
        bundle: None,
        vendor: "Prusa".into(),
        profiles_dir: None,
        bed: None,
        overrides: Vec::new(),
        perimeter_generator: None,
        custom_gcode: Vec::new(),
        adaptive_layers: None,
        smooth_layers: None,
        brim_ears: Vec::new(),
        slicing_rules: None,
        features: load_features(),
    };
    let mut model = None;
    while let Some(arg) = it.next() {
        let mut value = || it.next().cloned().ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "--project-settings" => {
                a.project_settings = Some(match value()?.as_str() {
                    "use" => ProjectMode::Use,
                    "update" => ProjectMode::Update,
                    "ignore" => ProjectMode::Ignore,
                    _ => return Err("--project-settings expects use, update or ignore".into()),
                })
            }
            "--export-3mf" => a.export_3mf = Some(value()?.into()),
            "--adaptive-layers" => {
                let q = value()?
                    .parse::<f32>()
                    .ok()
                    .filter(|q| (0.0..=1.0).contains(q))
                    .ok_or("--adaptive-layers expects a quality from 0 to 1")?;
                a.adaptive_layers = Some(q);
            }
            "--smooth-layers" => {
                let r = value()?
                    .parse::<u32>()
                    .ok()
                    .filter(|r| (1..=10).contains(r))
                    .ok_or("--smooth-layers expects 1 to 10")?;
                a.smooth_layers = Some(r);
            }
            "--brim-ear" => {
                let v = value()?;
                let n: Vec<f64> = v
                    .split(',')
                    .map(|n| n.trim().parse::<f64>().ok().filter(|n| n.is_finite()))
                    .collect::<Option<Vec<f64>>>()
                    .filter(|n| n.len() == 3 && n[2] > 0.)
                    .ok_or("--brim-ear expects X,Y,R in mm")?;
                a.brim_ears.push([n[0], n[1], n[2]]);
            }
            "--gcode-identity" => {
                let identity = match value()?.as_str() {
                    "mica" => "mica",
                    "orca-compatible" => "orca_compatible",
                    _ => return Err("--gcode-identity expects mica or orca-compatible".into()),
                };
                a.overrides
                    .push(("mica_gcode_identity".into(), identity.into()));
            }
            "--slicing-rules" => {
                a.slicing_rules = Some(
                    mica_core::rules::Rules::parse(&value()?)
                        .ok_or("--slicing-rules expects prusaslicer or orcaslicer")?,
                )
            }
            "--flavour" => {
                a.flavour = Some(match value()?.as_str() {
                    "orca" => mica_core::threemf::Flavour::Orca,
                    "prusa" => mica_core::threemf::Flavour::Prusa,
                    _ => return Err("--flavour expects orca or prusa".into()),
                })
            }
            "-o" | "--output" => a.output = Some(value()?.into()),
            "--center" => {
                let v = value()?;
                let (x, y) = v.split_once(',').ok_or("--center expects x,y")?;
                a.center = Some((
                    x.trim().parse().map_err(|_| "bad --center x")?,
                    y.trim().parse().map_err(|_| "bad --center y")?,
                ));
            }
            "--plate" => {
                if a.plate != mica_core::input::PlateSelection::Auto || a.each_plate {
                    return Err(PLATE_ONCE.into());
                }
                let n = value()?
                    .parse::<usize>()
                    .map_err(|_| "--plate expects a positive plate number")?;
                if n == 0 {
                    return Err("--plate starts at 1".into());
                }
                a.plate = mica_core::input::PlateSelection::Plate(n);
            }
            "--all-plates" => {
                if a.plate != mica_core::input::PlateSelection::Auto || a.each_plate {
                    return Err(PLATE_ONCE.into());
                }
                a.plate = mica_core::input::PlateSelection::All;
            }
            "--each-plate" => {
                if a.plate != mica_core::input::PlateSelection::Auto || a.each_plate {
                    return Err(PLATE_ONCE.into());
                }
                a.each_plate = true;
            }
            "--all-objects" => a.all_objects = true,
            "--first-object" => a.all_objects = false,
            "--extruder" => {
                a.extruder = value()?
                    .parse()
                    .map_err(|_| "--extruder expects a positive tool number")?
            }
            "--object-extruder" => {
                let v = value()?;
                let (i, n) = v
                    .split_once('=')
                    .ok_or("--object-extruder expects object=tool (one based)")?;
                let i = i.parse().map_err(|_| "invalid object index")?;
                let n = n.parse().map_err(|_| "invalid tool number")?;
                if i == 0 || n == 0 {
                    return Err("object and tool numbers start at 1".into());
                }
                a.object_extruders.insert(i, n);
            }
            "--texture-image" => {
                // A custom texture for this run: stored by content hash (the 3MF export embeds
                // it) and chosen as the texture unless a --set names another one.
                let path = value()?;
                let bytes = std::fs::read(&path).map_err(|e| format!("{path}: {e}"))?;
                let texture = mica_core::texture::register_png(&bytes)
                    .map_err(|e| format!("{path}: {e} (PNG images only)"))?;
                a.overrides
                    .insert(0, ("mica_texture_skin_texture".into(), texture));
            }
            "--set" => {
                let v = value()?;
                let (key, value) = v.split_once('=').ok_or("--set expects key=value")?;
                a.overrides.push((key.into(), value.into()));
            }
            "--color-change" | "--tool-change" | "--pause" | "--custom-gcode"
            | "--template-gcode" => {
                let v = value()?;
                a.custom_gcode
                    .push(custom_gcode::parse(arg, &v).expect("a custom G-code option")?);
            }
            "--preview" => a.preview = Some(value()?.into()),
            "--printer" => {
                a.printer = value()?;
                a.printer_explicit = true;
            }
            "--print" => {
                a.print = value()?;
                a.print_explicit = true;
            }
            "--filament" => {
                a.filament = value()?;
                a.filament_explicit = true;
            }
            "--bundle" => a.bundle = Some(value()?.into()),
            "--vendor" => {
                a.vendor = value()?;
                a.vendor_explicit = true;
            }
            "--profiles-dir" => a.profiles_dir = Some(value()?.into()),
            "--bed" => a.bed = Some(value()?),
            "--perimeter-generator" => {
                let name = value()?;
                if !matches!(name.as_str(), "classic" | "arachne") {
                    return Err("--perimeter-generator expects classic or arachne".into());
                }
                a.perimeter_generator = Some(name);
            }
            "--layers" => {
                a.layers = value()?
                    .split(',')
                    .map(|n| {
                        n.trim()
                            .parse::<usize>()
                            .map_err(|_| format!("bad layer number '{n}'"))
                    })
                    .collect::<Result<_, _>>()?
            }
            s if s.starts_with('-') => return Err(format!("unknown option '{s}'")),
            s => {
                if model.is_none() {
                    model = Some(PathBuf::from(s));
                } else {
                    a.extra_models.push(PathBuf::from(s));
                }
            }
        }
    }
    a.vendor_defaults();
    if a.vendor != "Prusa" && a.bundle.is_some() {
        return Err("--bundle is for Prusa; use --profiles-dir for Orca".into());
    }
    if a.extruder == 0 {
        return Err("--extruder starts at 1".into());
    }
    if a.flavour.is_some() && a.export_3mf.is_none() {
        return Err("--flavour requires --export-3mf".into());
    }
    if (a.adaptive_layers.is_some() || a.smooth_layers.is_some() || !a.brim_ears.is_empty())
        && a.export_3mf.is_none()
    {
        return Err(
            "--adaptive-layers, --smooth-layers and --brim-ear require --export-3mf".into(),
        );
    }
    if a.smooth_layers.is_some() && a.adaptive_layers.is_none() {
        return Err("--smooth-layers requires --adaptive-layers".into());
    }
    if a.export_3mf.is_some() && a.export_3mf == a.output {
        return Err("3MF and G-code output paths must differ".into());
    }
    a.model = model.ok_or("no model given")?;
    if a.each_plate {
        if a.output.is_none() {
            return Err("--each-plate needs -o DIR, the folder for the plates' files".into());
        }
        if !a.extra_models.is_empty() {
            return Err("--each-plate takes one model or project".into());
        }
        if a.export_3mf.is_some() || a.preview.is_some() {
            return Err("--each-plate does not combine with --export-3mf or --preview".into());
        }
    }
    Ok(a)
}

const PLATE_ONCE: &str = "--plate, --all-plates and --each-plate may only be specified once";

/// One plate's run of `--each-plate`.
#[derive(Clone)]
struct EachPlate {
    dir: PathBuf,
    plates: usize,
    name: String,
    /// File stems already used in `dir` by earlier plates.
    taken: Vec<String>,
}

/// `mica slice --each-plate -o DIR`: every plate sliced on its own (as `--plate N`) into
/// `DIR/<first object>_<plate name>.<gcode|bgcode>`, the names and bytes Mica's Export all
/// plates writes.
fn slice_each(a: SliceArgs) -> Result<(), String> {
    let dir = a.output.clone().ok_or("--each-plate needs -o DIR")?;
    if dir.is_file() {
        return Err(format!(
            "{}: --each-plate writes into a folder, not a file",
            dir.display()
        ));
    }
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let names = plate_names(&a.model)?;
    let mut taken = vec![];
    let mut written = vec![];
    for (i, name) in names.iter().enumerate() {
        let mut p = a.clone();
        p.each_plate = false;
        p.plate = if names.len() > 1 {
            mica_core::input::PlateSelection::Plate(i + 1)
        } else {
            mica_core::input::PlateSelection::Auto
        };
        p.output = None;
        p.each = Some(EachPlate {
            dir: dir.clone(),
            plates: names.len(),
            name: name.clone(),
            taken: taken.clone(),
        });
        let job = mica_core::job::Boundary::new("profile resolution");
        let sliced = job
            .run(|| slice_job(p, &job))
            .map_err(|e| format!("{name}: {e}"))?
            .ok_or("the slicer wrote no G-code")?;
        if let Some(stem) = sliced.out.file_stem() {
            taken.push(stem.to_string_lossy().into_owned());
        }
        written.push(sliced.out);
    }
    eprintln!("{} files → {}", written.len(), dir.display());
    Ok(())
}

/// The plates of `model` as the GUI shows them: a project's plates with their names (`Plate N`
/// when unnamed), else one plate.
fn plate_names(model: &Path) -> Result<Vec<String>, String> {
    let is_3mf = model
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("3mf"));
    let plates: Vec<String> = if is_3mf {
        mica_core::threemf::load_3mf(model)
            .map_err(|e| format!("{}: {e}", model.display()))?
            .plates
            .into_iter()
            .map(|p| p.name)
            .collect()
    } else {
        vec![]
    };
    let mut names: Vec<String> = plates
        .into_iter()
        .enumerate()
        .map(|(i, n)| {
            if n.trim().is_empty() {
                format!("Plate {}", i + 1)
            } else {
                n
            }
        })
        .collect();
    if names.is_empty() {
        names.push("Plate 1".into());
    }
    Ok(names)
}

/// The `[features]` switches of Mica's settings folder; a broken file is said and gives the
/// defaults, as the GUI does.
fn load_features() -> mica_profiles::features::Features {
    let (features, problem) = mica_profiles::features::Features::load();
    if let Some(p) = problem {
        eprintln!("warning: {p}");
    }
    features
}

/// What `mica slice` says about a project that uses a feature turned off in Preferences.
fn feature_off_warning(f: mica_profiles::features::Feature) -> String {
    if f == mica_profiles::features::Feature::BrickLayers {
        // Brick layers has no switch in Preferences unless it was turned on by hand.
        return "warning: This project uses Brick layers, which are off in this Mica; slicing \
                without them (the project file keeps them)"
            .into();
    }
    format!(
        "warning: This project uses {}, which is turned off in Settings › Features; \
         slicing without it (the project file keeps it)",
        f.title()
    )
}

/// `mica slice` with these feature switches instead of the settings folder's (tests compare
/// it with the GUI under the same switches).
pub fn slice_with_features(
    args: &[String],
    features: mica_profiles::features::Features,
) -> Result<(), String> {
    let mut a = parse_slice(args)?;
    a.features = features;
    slice(a)
}

/// `mica mcp` starts only with Mica MCP on in Settings › Features (`--help` always works).
fn mcp_allowed(args: &[String]) -> Result<(), String> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        return Ok(());
    }
    let features = load_features();
    if mica_profiles::features::mcp_enabled(&features) {
        Ok(())
    } else {
        Err(mica_profiles::features::mcp_off_message(
            &mica_profiles::features::Features::default_path(),
        ))
    }
}

fn slice(a: SliceArgs) -> Result<(), String> {
    let job = mica_core::job::Boundary::new("profile resolution");
    job.run(|| slice_job(a, &job)).map(drop)
}

/// What a slice wrote, for callers that report on it (`mica mcp`'s slice summary).
pub(crate) struct Sliced {
    pub out: PathBuf,
    pub layers: usize,
    pub stats: mica_gcode::writer::MoveStats,
    /// The print's warnings (also printed to stderr).
    pub warnings: Vec<String>,
    /// The configuration the G-code was written with.
    pub config: mica_core::config::Config,
}

/// Slices as `mica slice` does; `None` when the arguments only export a project.
fn slice_job(a: SliceArgs, job: &mica_core::job::Boundary) -> Result<Option<Sliced>, String> {
    let _timings = mica_core::timing::Report::default();
    let t0 = std::time::Instant::now();
    let project_settings::Resolved {
        mut config,
        original,
        selected_orca,
    } = project_settings::resolve(&a)?;
    if let Some(generator) = &a.perimeter_generator {
        config
            .set_raw("perimeter_generator", generator)
            .map_err(|e| e.to_string())?;
    }
    apply_slicing_rules(&a, &mut config)?;
    for (key, value) in &a.overrides {
        let key = match key.as_str() {
            "skirt_loops" => "skirts",
            "brim_object_gap" => "brim_separation",
            key => key,
        };
        overrides::apply(&mut config, key, value)?;
    }
    for warning in mica_core::safety::config_warnings(&config) {
        eprintln!("warning: {warning}");
    }
    let mut settings = PrintSettings::from_config(&config).map_err(|e| e.to_string())?;
    // The GUI places models by the same rule (`Bed::center`).
    let center = a
        .center
        .unwrap_or_else(|| mica_core::input::bed_center(&config));
    let mut paths = vec![a.model.clone()];
    paths.extend(a.extra_models.clone());
    if a.export_3mf.is_some() {
        project_export::save(
            &a,
            &config,
            center,
            original.as_ref(),
            selected_orca.as_ref(),
        )?;
        if a.output.is_none() {
            return Ok(None);
        }
    }
    // Spiral vase forces its settings (normalize_fdm) for slicing; the project export above
    // kept the user's values.
    if mica_core::spiral_vase::normalize(&mut config) {
        settings = PrintSettings::from_config(&config).map_err(|e| e.to_string())?;
    }
    // Features turned off in Preferences slice without them; the project export above kept
    // their data.
    use mica_profiles::features::Feature;
    let config_textures = mica_core::features::uses_textures(&config, &[]);
    if !a.features.textures {
        mica_core::features::config_without_textures(&mut config);
        settings = PrintSettings::from_config(&config).map_err(|e| e.to_string())?;
    }
    let config_bricks = mica_core::features::uses_brick_layers(&config, &[]);
    if !mica_profiles::features::brick_layers_enabled(&a.features) {
        mica_core::features::config_without_brick_layers(&mut config);
        settings = PrintSettings::from_config(&config).map_err(|e| e.to_string())?;
    }
    job.stage("model import");
    let mut loaded = mica_core::input::load_with_plate(
        &paths,
        center,
        a.center.is_some(),
        a.all_objects,
        a.extruder - 1,
        &a.object_extruders,
        &config,
        &mica_profiles::project::Mapper::default(),
        a.plate,
    )?;
    for w in &loaded.warnings {
        eprintln!("{w}");
    }
    let first_object = loaded.objects.first().map(|o| o.name.clone());
    if !a.features.textures {
        if config_textures || mica_core::features::uses_textures(&config, &loaded.objects) {
            eprintln!("{}", feature_off_warning(Feature::Textures));
        }
        mica_core::features::without_textures(&mut config, &mut loaded.objects);
    }
    if !mica_profiles::features::brick_layers_enabled(&a.features) {
        if config_bricks || mica_core::features::uses_brick_layers(&config, &loaded.objects) {
            eprintln!("{}", feature_off_warning(Feature::BrickLayers));
        }
        mica_core::features::without_brick_layers(&mut config, &mut loaded.objects);
    }
    if !a.features.color_mix && mica_core::features::uses_color_mix(&loaded.objects) {
        eprintln!("{}", feature_off_warning(Feature::ColorMix));
        mica_core::features::without_color_mix(&mut loaded.objects);
    }
    if let Some(placement) = loaded.tower_placement {
        for (key, value) in ["wipe_tower_x", "wipe_tower_y", "wipe_tower_rotation_angle"]
            .into_iter()
            .zip(placement)
        {
            if !a.overrides.iter().any(|(k, _)| k == key) {
                overrides::apply(&mut config, key, &value.to_string())?;
            }
        }
    }
    // The print builder the GUI's slice worker uses too: placement checks, preparation, the
    // plate's custom G-code and the single-object or plate source.
    job.stage("mesh preparation, supports and seams");
    let plate = mica_core::input::prepare_plate(loaded.objects, &config, &|_| {
        Box::new(mica_core::progress::Silent)
    })?;
    let custom = custom_gcode::merge(
        loaded.custom_gcode.clone(),
        &a.custom_gcode,
        &config,
        config.vector_len("nozzle_diameter").max(1),
        PreparedPrint::object_tool_count(&plate.objects).max(1),
    );
    let print = plate.finish(&config, custom)?;
    for warning in &print.warnings {
        eprintln!("warning: {warning}");
    }
    for note in &print.notes {
        eprintln!("{note}");
    }
    let mesh = &print.mesh;
    let obj: &dyn LayerSource = &print;
    let name = &print.name;
    // Prusa profiles ask for binary G-code (binary_gcode = 1); `-o name.gcode` still writes text.
    let extension = if binary_output(&config) {
        "bgcode"
    } else {
        "gcode"
    };
    let out = match &a.each {
        Some(each) => {
            let stem =
                mica_core::export_name::stem(first_object.as_deref(), &each.name, each.plates);
            let mut taken = each.taken.clone();
            let stem = mica_core::export_name::unique(&stem, &mut taken);
            each.dir.join(format!("{stem}.{extension}"))
        }
        None => a
            .output
            .clone()
            .unwrap_or_else(|| a.model.with_extension(extension)),
    };
    job.stage("toolpaths and G-code export");
    let stats = mica_gcode::export::export_file(obj, &config, &settings, name, Some(mesh), &out)
        .map_err(|e| format!("{}: {e}", out.display()))?;
    eprintln!(
        "{} layers, {:.0} mm filament, ~{:.0} min → {} ({:.2?})",
        obj.layer_count(),
        stats.filament_mm,
        stats.time_s / 60.0,
        out.display(),
        t0.elapsed()
    );

    if let Some(dir) = &a.preview {
        write_previews(obj, dir, &a.layers)?;
    }
    Ok(Some(Sliced {
        out,
        layers: obj.layer_count(),
        stats,
        warnings: print.warnings.clone(),
        config,
    }))
}

/// Whether the printer's own format is Prusa binary G-code (`binary_gcode` on a Marlin 2
/// printer), as the GUI's Export all plates decides too.
pub fn binary_output(config: &mica_core::config::Config) -> bool {
    config.bool("binary_gcode").unwrap_or(false) && config.string("gcode_flavor") == Some("marlin2")
}

/// `--slicing-rules` (or `--set mica_slicing_rules=…`), then the other engine's settings: a
/// PrusaSlicer bundle profile sliced like OrcaSlicer gets the Orca-only options from Orca's
/// matching profile, else Orca's defaults (`rules_fill`). Explicit `--set` values still win:
/// they are applied afterwards and the fill only adds options the profile lacks.
fn apply_slicing_rules(
    a: &SliceArgs,
    config: &mut mica_core::config::Config,
) -> Result<(), String> {
    use mica_core::rules::{KEY, Rules};
    let explicit = a.slicing_rules.or(a
        .overrides
        .iter()
        .rev()
        .find(|(k, _)| k == KEY)
        .map(|(_, v)| Rules::parse(v).ok_or(format!("{KEY} expects prusaslicer or orcaslicer")))
        .transpose()?);
    if let Some(rules) = explicit {
        rules.set(config);
    }
    if Rules::of(config).is_orca() && a.vendor == "Prusa" {
        let filled = mica_profiles::rules_fill::fill_orca_rules(config, None);
        if !filled.keys.is_empty() {
            eprintln!(
                "note: this profile is tuned for PrusaSlicer; G-code follows OrcaSlicer's rules \
                 ({} Orca-only settings from {})",
                filled.keys.len(),
                filled.source
            );
        }
    }
    Ok(())
}

fn write_previews(obj: &dyn LayerSource, dir: &Path, layers: &[usize]) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    // Only the requested layers are computed.
    let mut rendered = Vec::new();
    for &n in layers {
        let i = n.wrapping_sub(1);
        if i >= obj.layer_count() {
            return Err(format!("no layer {n}"));
        }
        rendered.push((n, obj.toolpaths(i..i + 1).remove(0)));
    }
    let frame = BoundingBox::from_points(
        rendered
            .iter()
            .flat_map(|(_, l)| l.paths.iter().flat_map(|p| p.points.iter())),
    )
    .ok_or("nothing to preview")?;
    for (n, layer) in &rendered {
        let (w, h, rgb) = preview::render(layer, frame, 20.0);
        let path = dir.join(format!("layer_{n:04}.png"));
        std::fs::write(&path, preview::encode_png(w, h, &rgb)).map_err(|e| e.to_string())?;
        eprintln!("preview: {}", path.display());
    }
    Ok(())
}

/// `mica profiles fetch`: downloads the pinned Prusa bundle (and its bed assets) into Mica's
/// settings folder, unless an intact copy is already there.
fn fetch_profiles(args: &[String]) -> Result<(), String> {
    use mica_profiles::vendor_profiles::prusa;
    if let Some(a) = args.first() {
        return Err(format!(
            "unknown argument '{a}' (mica profiles fetch takes none)"
        ));
    }
    eprintln!(
        "Downloading the Prusa printer profiles {} from {} …",
        prusa::VERSION,
        prusa::REPOSITORY
    );
    let mut shown = -1i32;
    let fetched = prusa::fetch(
        &mut |f| {
            let pct = (f * 100.) as i32;
            if pct / 10 != shown / 10 {
                shown = pct;
                eprintln!("{pct:3} %");
            }
        },
        &std::sync::atomic::AtomicBool::new(false),
    )?;
    println!(
        "Prusa printer profiles {}: {} (SHA-256 checked)",
        prusa::VERSION,
        fetched.bundle.display()
    );
    println!("Bed models and pictures: {}", fetched.assets);
    if !fetched.missing_assets.is_empty() {
        println!(
            "Not available ({}; those printers get a plain bed drawing): {}",
            fetched.missing_assets.len(),
            fetched.missing_assets.join(", ")
        );
    }
    Ok(())
}

fn list_profiles(args: &[String]) -> Result<(), String> {
    let mut kinds = Vec::new();
    let mut bundle = None;
    let mut vendor = "Prusa".to_string();
    let mut profiles_dir = None;
    let mut printer = None;
    let mut process = None;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "printer" => kinds.push(Kind::Printer),
            "print" => kinds.push(Kind::Print),
            "filament" => kinds.push(Kind::Filament),
            "--bundle" => bundle = Some(PathBuf::from(it.next().ok_or("--bundle needs a value")?)),
            "--vendor" => vendor = it.next().ok_or("--vendor needs a value")?.clone(),
            "--profiles-dir" => {
                profiles_dir = Some(PathBuf::from(
                    it.next().ok_or("--profiles-dir needs a value")?,
                ))
            }
            "--printer" => printer = Some(it.next().ok_or("--printer needs a value")?.as_str()),
            "--print" => process = Some(it.next().ok_or("--print needs a value")?.as_str()),
            other => return Err(format!("unknown argument '{other}'")),
        }
    }
    if kinds.is_empty() {
        kinds = vec![Kind::Printer, Kind::Print, Kind::Filament];
    }
    if vendor != "Prusa" {
        if bundle.is_some() {
            return Err("--bundle is for Prusa; use --profiles-dir for Orca".into());
        }
        let (bundle, path) = orca::discovery::load_installed(profiles_dir.as_deref(), &vendor)
            .map_err(|e| e.to_string())?;
        println!("# {} ({vendor})", path.display());
        for kind in kinds {
            let kind = match kind {
                Kind::Printer => orca::Kind::Machine,
                Kind::Print => orca::Kind::Process,
                Kind::Filament => orca::Kind::Filament,
            };
            println!("\n[{kind:?}]");
            let names = if kind != orca::Kind::Machine
                && let Some(printer) = printer
            {
                bundle.compatible_presets(kind, printer, process)
            } else {
                bundle.preset_names(kind)
            };
            for name in names {
                println!("{name}");
            }
        }
        return Ok(());
    }
    if profiles_dir.is_some() {
        return Err("--profiles-dir requires an Orca --vendor".into());
    }
    let (bundle, path) = load_bundle(bundle.as_deref())?;
    let version = bundle
        .vendor
        .get("config_version")
        .map_or("?", String::as_str);
    println!("# {} (profiles {version})", path.display());
    for kind in kinds {
        println!("\n[{kind:?}]");
        let names = if kind != Kind::Printer
            && let Some(printer) = printer
        {
            bundle.compatible_presets(kind, printer, process)
        } else {
            bundle.preset_names(kind)
        };
        for name in names {
            println!("{name}");
        }
    }
    Ok(())
}

/// The help text: commands, slice options and the profile commands.
fn usage() -> String {
    USAGE
        .replace("{PROFILE_USAGE}", profile_cmd::USAGE)
        .replace("{CUSTOM_GCODE_USAGE}", custom_gcode::USAGE)
}

/// Runs one `mica` command (the arguments after the program name). The binary is a thin
/// wrapper around this, so tests drive exactly the CLI's code path (argument parsing, profile
/// resolution, model import, preparation and export).
pub fn run(args: &[String]) -> Result<(), String> {
    match args.first().map(String::as_str) {
        Some("--version" | "-V") => {
            println!("mica {}", mica_core::VERSION);
            Ok(())
        }
        None | Some("--help" | "-h") => {
            println!("{}", usage());
            Ok(())
        }
        // `mica slice --help`: the same text, which lists the slice options.
        Some("slice") if args[1..].iter().any(|a| a == "--help" || a == "-h") => {
            println!("{}", usage());
            Ok(())
        }
        Some("print") => printer_cmd::print(&args[1..]),
        Some("printers") => printer_cmd::printers(&args[1..]),
        Some("validate") => validate::run(&args[1..]),
        Some("calibrate") => calibrate::run(&args[1..]),
        Some("slice") => parse_slice(&args[1..]).and_then(|a| {
            if a.each_plate {
                slice_each(a)
            } else {
                slice(a)
            }
        }),
        Some("profiles") if args.get(1).is_some_and(|a| a == "fetch") => fetch_profiles(&args[2..]),
        Some("profiles") => list_profiles(&args[1..]),
        Some("profile") => profile_cmd::run(&args[1..], || load_bundle(None)),
        Some("mcp") => mcp_allowed(&args[1..]).and_then(|()| mcp::run(&args[1..])),
        Some(other) => Err(format!("unknown command '{other}' (see mica --help)")),
    }
}

/// [`run`] with the exit status and error message the binary reports.
pub fn main(args: &[String]) -> ExitCode {
    match run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("mica: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn slice_help_prints_the_usage() {
        // It used to answer "unknown option '--help'".
        for flag in ["--help", "-h"] {
            run(&["slice".to_string(), flag.to_string()]).unwrap();
            run(&["slice".into(), "cube.stl".into(), flag.to_string()]).unwrap();
        }
        assert!(usage().contains("Slice options:"));
    }

    #[test]
    fn plate_arguments_are_one_based_and_mutually_exclusive() {
        let parse =
            |args: &[&str]| parse_slice(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(
            parse(&["cat.3mf"]).unwrap().plate,
            mica_core::input::PlateSelection::Auto
        );
        assert_eq!(
            parse(&["cat.3mf", "--plate", "2"]).unwrap().plate,
            mica_core::input::PlateSelection::Plate(2)
        );
        assert_eq!(
            parse(&["cat.3mf", "--all-plates"]).unwrap().plate,
            mica_core::input::PlateSelection::All
        );
        for args in [
            vec!["--plate", "0"],
            vec!["--plate", "x"],
            vec!["--plate"],
            vec!["--plate", "2", "--all-plates"],
            vec!["--all-plates", "--plate", "2"],
        ] {
            assert!(parse(&args).is_err());
        }
        let each = parse(&["cat.3mf", "--each-plate", "-o", "out"]).unwrap();
        assert!(each.each_plate);
        for args in [
            vec!["cat.3mf", "--each-plate"],
            vec!["cat.3mf", "--each-plate", "--plate", "2", "-o", "out"],
            vec!["cat.3mf", "--all-plates", "--each-plate", "-o", "out"],
            vec!["cat.3mf", "dog.stl", "--each-plate", "-o", "out"],
        ] {
            assert!(parse(&args).is_err(), "{args:?}");
        }
    }
}
