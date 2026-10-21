// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/GCodeProcessor.cpp :: UsedFilaments::increase_caches
// Source: orca@8500fcdcca src/libslic3r/GCode/GCodeProcessor.cpp :: GCodeProcessor::run_post_process
// Changes: Independent-tool retraction debt and explicit G-code totals, integrated into the bounded timing pass. No MMU parking/loading correction; legacy single-tool statistics stay unchanged.
// Changes: Retain per-tool filament cost and select the Orca footer formatter by slicing rules; Prusa replacements stay unchanged.

use mica_core::config::Config;

mod orca;

/// Retained in final output so a subsequent analysis also refreshes these totals.
pub(crate) const MARKER: &str = "; filament statistics = explicit G-code, retractions excluded";

#[derive(Default)]
pub(super) struct Filament {
    pub enabled: bool,
    orca: bool,
    tools: Vec<Tool>,
}
struct Tool {
    used: f64,
    retracted: f64,
    area: f64,
    density: f64,
    cost: f64,
}
impl Filament {
    pub fn new(c: &Config) -> Self {
        Self {
            enabled: false,
            orca: mica_core::rules::Rules::of(c).is_orca(),
            tools: (0..c.vector_len("nozzle_diameter").max(1))
                .map(|i| Tool {
                    used: 0.,
                    retracted: 0.,
                    area: std::f64::consts::PI
                        * (c.float_at("filament_diameter", i).unwrap_or(1.75) / 2.).powi(2),
                    density: c.float_at("filament_density", i).unwrap_or(0.),
                    cost: c.float_at("filament_cost", i).unwrap_or(0.),
                })
                .collect(),
        }
    }
    pub fn motion(&mut self, tool: usize, delta_e: f64) {
        if let Some(t) = self.tools.get_mut(tool) {
            t.retracted -= delta_e;
            if t.retracted < 0. {
                t.used -= t.retracted;
                t.retracted = 0.;
            }
        }
    }
    pub fn total_mm(&self) -> f64 {
        self.tools.iter().map(|t| t.used).sum()
    }
    pub fn replace(&self, line: &str) -> Option<String> {
        if !self.enabled {
            return None;
        }
        if self.orca
            && let Some(replacement) = self.replace_orca(line)
        {
            return Some(replacement);
        }
        let value = if line.starts_with("; filament used [mm] =") {
            return Some(format!("; filament used [mm] = {:.2}\n", self.total_mm()));
        } else if line.starts_with("; filament used [cm3] =") {
            let volume: f64 = self.tools.iter().map(|t| t.used * t.area / 1000.).sum();
            return Some(format!("; filament used [cm3] = {volume:.2}\n"));
        } else {
            let rest = line.strip_prefix("; filament used T")?;
            let (tool, unit) = rest.split_once(' ')?;
            let t = self.tools.get(tool.parse::<usize>().ok()?)?;
            let cm3 = t.used * t.area / 1000.;
            match unit.split_once(" =")?.0 {
                "[mm]" => t.used,
                "[cm3]" => cm3,
                "[g]" => cm3 * t.density,
                _ => return None, // Per-role values describe generated paths only.
            }
        };
        Some(format!("{} = {value:.5}\n", line.split_once(" =")?.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{export::postprocess, writer::MoveStats};
    use std::io::Cursor;

    #[test]
    fn explicit_startup_purge_arcs_and_per_tool_retraction_debt() {
        let mut c = Config::defaults();
        c.set_raw("nozzle_diameter", "0.4,0.4").unwrap();
        c.set_raw("filament_diameter", "1.75,2.85").unwrap();
        c.set_raw("filament_density", "1.24,1.1").unwrap();
        let input = format!(
            "M83\nT1\nG1 E8 F60\nG1 E-5\nT0\nG1 E3\nG1 E-2\nT1\nG1 E5\nG1 X10 E2 F600\nG2 X20 I5 E1\nG1 E-5\nT0\nG1 E2\nM82\nG92 E0\nG1 X30 E4\n{MARKER}\n; filament used [mm] = 0\n; filament used [cm3] = 0\n; filament used T0 [mm] = 0\n; filament used T1 [mm] = 0\n; filament used T1 [g] = 0\n"
        );
        let mut output = Vec::new();
        let stats = postprocess(
            &mut Cursor::new(input),
            &mut output,
            MoveStats::default(),
            &c,
        )
        .unwrap();
        let gcode = String::from_utf8(output.clone()).unwrap();
        assert!((stats.filament_mm - 18.).abs() < 1e-8);
        assert!(gcode.contains("; filament used T0 [mm] = 7.00000"));
        assert!(gcode.contains("; filament used T1 [mm] = 11.00000"));
        let grams = 11. * std::f64::consts::PI * (2.85_f64 / 2.).powi(2) / 1000. * 1.1;
        assert!(gcode.contains(&format!("; filament used T1 [g] = {grams:.5}")));
        let mut repeated = Vec::new();
        postprocess(&mut Cursor::new(output), &mut repeated, stats, &c).unwrap();
        assert_eq!(gcode, String::from_utf8(repeated).unwrap());
    }
}
