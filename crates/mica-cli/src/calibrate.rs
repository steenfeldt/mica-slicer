// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! CLI adapter for the engine's headless calibration API.
use mica_core::calib::{Kind, Parameters};
use std::path::PathBuf;

pub const USAGE: &str = "\
Usage: mica calibrate <kind> [options] -o out.gcode [profile options]
Kinds: flow, pa-line, pa-pattern, pa-tower, temperature, retraction, max-volumetric-speed
Options:
  --start N --end N --step N  range (temperature descends in 5°C steps; others ascend)
  --pass 1|2                  flow: coarse (-20..+20%) or fine (-9..0%)
  --speed N                   PA fast speed in mm/s (default: Orca suggested speed)
  --tool N                    tool that prints the test, one based (default: 1); on the XL
                              and U1 only that tool is picked and heated
Profile options: --printer, --print, --filament, --vendor, --bed, --bundle, --profiles-dir
Output extension must be .gcode or .bgcode.
See docs/calibration.md for defaults, height formulas and reading results.";

pub fn run(args: &[String]) -> Result<(), String> {
    if args.is_empty() || args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{USAGE}");
        return Ok(());
    }
    let kind = Kind::parse(&args[0])?;
    let mut parameters = Parameters::new(kind);
    // Reuse the normal profile parser and discovery, with a placeholder model never opened.
    let mut profile = vec!["calibration.stl".to_string()];
    let mut it = args[1..].iter();
    while let Some(arg) = it.next() {
        let value = it
            .next()
            .ok_or_else(|| format!("missing value after {arg}"))?;
        if kind == Kind::FlowRate && matches!(arg.as_str(), "--start" | "--end" | "--step") {
            return Err("flow uses --pass 1|2 rather than --start/--end/--step".into());
        }
        let number = || {
            value
                .parse::<f64>()
                .map_err(|_| format!("{arg} expects a number"))
        };
        match arg.as_str() {
            "--start" => parameters.start = number()?,
            "--end" => parameters.end = number()?,
            "--step" => parameters.step = number()?,
            "--pass" => {
                if kind != Kind::FlowRate {
                    return Err("--pass is only for flow".into());
                }
                parameters.pass = value.parse().map_err(|_| "--pass expects 1 or 2")?;
            }
            "--speed" => {
                if !matches!(
                    kind,
                    Kind::PressureAdvanceLine
                        | Kind::PressureAdvancePattern
                        | Kind::PressureAdvanceTower
                ) {
                    return Err("--speed is only for PA".into());
                }
                parameters.speed = Some(number()?);
            }
            "--tool" => {
                parameters.tool = value
                    .parse::<usize>()
                    .ok()
                    .and_then(|n| n.checked_sub(1))
                    .ok_or("--tool expects a tool number starting at 1")?;
            }
            "--printer" | "--print" | "--filament" | "--vendor" | "--bed" | "--bundle"
            | "--profiles-dir" | "-o" => {
                profile.push(arg.clone());
                profile.push(value.clone());
            }
            _ => return Err(format!("unknown calibration option {arg}\n{USAGE}")),
        }
    }
    let a = super::parse_slice(&profile)?;
    let output: PathBuf = a
        .output
        .ok_or("calibration requires -o out.gcode (or .bgcode)")?;
    if !output
        .extension()
        .is_some_and(|e| e == "gcode" || e == "bgcode")
    {
        return Err("calibration output must end in .gcode or .bgcode".into());
    }
    let base = if a.vendor == "Prusa" {
        let (bundle, path) = super::load_bundle(a.bundle.as_deref())?;
        bundle
            .full_config(&a.printer, &a.print, &a.filament)
            .map_err(|e| format!("{}: {e}", path.display()))?
    } else {
        let (bundle, path) =
            mica_profiles::orca::discovery::load_installed(a.profiles_dir.as_deref(), &a.vendor)
                .map_err(|e| e.to_string())?;
        bundle
            .single_tool_config(&a.printer, &a.print, &a.filament, a.bed.as_deref())
            .map_err(|e| format!("{}: {e}", path.display()))?
    };
    if !matches!(base.string("gcode_flavor"), Some("marlin2" | "klipper")) {
        return Err("calibration supports Prusa Buddy (marlin2) and Klipper printers".into());
    }
    let plan = mica_core::calib::generate(&parameters, &base)?;
    let print = plan.prepare(&base)?;
    let mesh = plan.combined_mesh();
    mica_gcode::export::export_file(
        &print,
        &print.config,
        &print.settings,
        &format!("calibration {kind:?}"),
        Some(&mesh),
        &output,
    )
    .map_err(|e| e.to_string())?;
    eprintln!(
        "Wrote {}: {:?} on tool {}, samples {:?}. See docs/calibration.md.",
        output.display(),
        kind,
        print.tool() + 1,
        plan.samples
    );
    Ok(())
}
