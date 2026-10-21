// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Estimate external text G-code using its Prusa-style config footer (or a separate INI config).
//! cargo run --offline -p mica-gcode --example estimate_time -- input.gcode [config.ini] [output.gcode|-] [layers.csv]
use mica_core::config::Config;
use mica_gcode::{time_estimate::Analysis, writer::MoveStats};
use std::io::{BufRead, BufReader, Seek};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let input = args
        .first()
        .ok_or("usage: estimate_time input.gcode [config.ini] [output.gcode|-] [layers.csv]")?;
    let mut config = Config::defaults();
    let config_path = args.get(1).unwrap_or(input);
    for line in BufReader::new(std::fs::File::open(config_path)?).lines() {
        let line = line?;
        if let Some((key, value)) = line.trim_start_matches(';').trim().split_once(" = ") {
            let _ = config.set_raw(key.trim(), value.trim());
        }
    }
    let mut src = BufReader::new(std::fs::File::open(input)?);
    let mut analysis = Analysis::new(&mut src, &config)?;
    println!("{:?}", analysis.estimate);
    let report = analysis.breakdown(&mut src, &config)?;
    eprintln!("total: {:?}", report.total);
    if let Some(path) = args.get(3) {
        use std::io::Write;
        let mut csv = std::io::BufWriter::new(std::fs::File::create(path)?);
        let roles: Vec<_> = report.total.roles.keys().collect();
        writeln!(
            csv,
            "layer,seconds,travel,retract,unretract,other,extrusion_moves,arcs,retracts,z_moves,z_only_moves,z_only_seconds,{}",
            roles
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(",")
        )?;
        for (layer, s) in &report.layers {
            write!(
                csv,
                "{layer},{:.6},{:.6},{:.6},{:.6},{:.6},{},{},{},{},{},{:.6}",
                s.seconds,
                s.travel_seconds,
                s.retract_seconds,
                s.unretract_seconds,
                s.other_seconds,
                s.extrusion_moves,
                s.arcs,
                s.retracts,
                s.z_moves,
                s.z_only_moves,
                s.z_only_seconds
            )?;
            for role in &roles {
                write!(csv, ",{:.6}", s.roles.get(*role).copied().unwrap_or(0.0))?;
            }
            writeln!(csv)?;
        }
    }
    if let Some(path) = args.get(2).filter(|p| p.as_str() != "-") {
        src.rewind()?;
        analysis.write(
            &mut src,
            &mut std::fs::File::create(path)?,
            MoveStats::default(),
        )?;
    }
    Ok(())
}
