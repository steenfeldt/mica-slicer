// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

pub(crate) fn run(args: &[String]) -> Result<(), String> {
    let json = args.iter().any(|a| a == "--json");
    let estimate = args.iter().any(|a| a == "--estimate");
    let mut args: Vec<_> = args
        .iter()
        .filter(|a| !matches!(a.as_str(), "--json" | "--estimate"))
        .cloned()
        .collect();
    let printer = args
        .windows(2)
        .find(|w| w[0] == "--printer")
        .map(|w| w[1].clone())
        .unwrap_or_default();
    if printer.contains("Snapmaker U1") && !args.iter().any(|a| a == "--vendor") {
        args.extend(["--vendor".into(), "Snapmaker".into()]);
    }
    if printer.contains("Prusa XL") {
        for (flag, value) in [
            ("--print", "0.20mm SPEED @XLIS 0.4"),
            ("--filament", "Prusament PLA @XLIS"),
        ] {
            if !args.iter().any(|a| a == flag) {
                args.extend([flag.into(), value.into()]);
            }
        }
    }
    let a = super::parse_slice(&args)?;
    if !a.extra_models.is_empty() {
        return Err("validate accepts one G-code file".into());
    }
    let mut c = if a.vendor == "Prusa" {
        let (bundle, _) = super::load_bundle(a.bundle.as_deref())?;
        bundle
            .full_config(&a.printer, &a.print, &a.filament)
            .map_err(|e| e.to_string())?
    } else {
        let (bundle, _) =
            mica_profiles::orca::discovery::load_installed(a.profiles_dir.as_deref(), &a.vendor)
                .map_err(|e| e.to_string())?;
        bundle
            .single_tool_config(&a.printer, &a.print, &a.filament, a.bed.as_deref())
            .map_err(|e| e.to_string())?
    };
    for (key, value) in &a.overrides {
        super::overrides::apply(&mut c, key, value)?;
    }
    if estimate {
        // Re-time the file with Mica's estimator, e.g. to compare with another slicer's own figure.
        let file = std::fs::File::open(&a.model).map_err(|e| e.to_string())?;
        let time = mica_gcode::time_estimate::estimate(&mut std::io::BufReader::new(file), &c)
            .map_err(|e| e.to_string())?;
        println!("estimated time: {:.1} s (normal mode)", time.normal);
    }
    let profile = mica_gcode::validate::Profile::from_config(&c)?;
    let report =
        mica_gcode::validate::validate_file(&a.model, &profile).map_err(|e| e.to_string())?;
    if json {
        let diagnostics = report
            .diagnostics
            .iter()
            .map(|d| {
                format!(
                    "{{\"severity\":{},\"line\":{},\"rule\":{},\"message\":{}}}",
                    quote(&format!("{:?}", d.severity)),
                    d.line,
                    quote(d.rule),
                    quote(&d.message)
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        let roles = report
            .role_filament_mm
            .iter()
            .map(|(k, v)| format!("{}:{v}", quote(k)))
            .collect::<Vec<_>>()
            .join(",");
        let tool_roles = report
            .tool_role_filament_mm
            .iter()
            .map(|roles| {
                let roles = roles
                    .iter()
                    .map(|(k, v)| format!("{}:{v}", quote(k)))
                    .collect::<Vec<_>>()
                    .join(",");
                format!("{{{roles}}}")
            })
            .collect::<Vec<_>>()
            .join(",");
        println!(
            "{{\"errors\":{},\"warnings\":{},\"lines\":{},\"layers\":{},\"filament_mm\":{:?},\"roles\":{{{roles}}},\"tool_roles\":[{tool_roles}],\"toolchanges\":{},\"diagnostics\":[{diagnostics}]}}",
            report.errors,
            report.warnings,
            report.lines,
            report.layers,
            report.filament_mm,
            report.toolchanges
        );
    } else {
        for d in &report.diagnostics {
            println!(
                "{}:{}: {:?} [{}] {}",
                a.model.display(),
                d.line,
                d.severity,
                d.rule,
                d.message
            );
        }
        println!(
            "{} errors, {} warnings; {} layers, {} lines",
            report.errors, report.warnings, report.layers, report.lines
        );
        for (role, mm) in &report.role_filament_mm {
            println!("  {role}: {mm:.3} mm");
        }
    }
    if report.errors == 0 {
        Ok(())
    } else {
        Err(format!(
            "G-code validation failed with {} errors",
            report.errors
        ))
    }
}

fn quote(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if c < ' ' => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
