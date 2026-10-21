// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: snapmaker-orca@b1831e5dcb src/libslic3r/GCode.cpp :: DoExport::update_print_stats_and_format_filament_stats, GCode::_do_export
// Source: snapmaker-orca@b1831e5dcb src/libslic3r/GCode/GCodeProcessor.cpp :: GCodeProcessor::run_post_process
// Source: orca@8500fcdcca src/libslic3r/GCode.cpp :: DoExport::update_print_stats_and_format_filament_stats, GCode::_do_export
// Source: orca@8500fcdcca src/libslic3r/GCode/GCodeProcessor.cpp :: GCodeProcessor::run_post_process
// Changes: Format Orca-rule footer statistics from Mica's existing bounded per-tool measurement pass; no separate tower accounting or MMU correction. Preserve positive-weight/cost gates and final postprocessor's full slot vectors.

use super::{Filament, Tool};

impl Tool {
    fn orca_values(&self) -> [f64; 4] {
        let cm3 = self.used * self.area * 0.001;
        let g = cm3 * self.density;
        [self.used, cm3, g, g * self.cost * 0.001]
    }
}

impl Filament {
    pub(super) fn replace_orca(&self, line: &str) -> Option<String> {
        let (tag, _) = line.split_once(" =")?;
        let (index, total, conditional) = match tag {
            "; filament used [mm]" => (0, false, false),
            "; filament used [cm3]" => (1, false, false),
            "; filament used [g]" => (2, false, true),
            "; filament cost" => (3, false, true),
            "; total filament used [g]" => (2, true, false),
            "; total filament cost" => (3, true, false),
            _ => return None,
        };
        // The generator emits weight only when positive, and cost only inside that gate.
        // Orca's postprocessor then rewrites those lines with ALL slots, including unused
        // leading, intermediate and trailing tools, each with exactly two decimal places.
        if conditional
            && !self.tools.iter().any(|t| {
                let v = t.orca_values();
                v[2] > 0. && v[index] > 0.
            })
        {
            return Some(String::new());
        }
        let values = self.tools.iter().map(|t| t.orca_values()[index]);
        let formatted = if total {
            format!("{:.2}", values.sum::<f64>())
        } else {
            values
                .map(|v| format!("{v:.2}"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        Some(format!("{tag} = {formatted}\n"))
    }
}

#[cfg(test)]
mod tests {
    use crate::{export::postprocess, time_estimate::FILAMENT_MARKER, writer::MoveStats};
    use mica_core::{config::Config, rules::Rules};
    use std::io::Cursor;

    const FOOTER: &str = "; filament used [mm] = 0\n; filament used [cm3] = 0\n; filament used [g] = 0\n; filament cost = 0\n; total filament used [g] = 0\n; total filament cost = 0\n";

    fn config() -> Config {
        let mut c = Config::defaults();
        Rules::OrcaSlicer.set(&mut c);
        c.set_raw("nozzle_diameter", "0.4,0.4,0.4,0.4").unwrap();
        c.set_raw("filament_diameter", "1.75,1.75,2.85,1.75")
            .unwrap();
        c.set_raw("filament_density", "1.24,1.24,1.1,1.24").unwrap();
        c.set_raw("filament_cost", "60,60,100,60").unwrap();
        c
    }

    fn processed(input: String, c: &Config) -> String {
        let mut out = Vec::new();
        postprocess(&mut Cursor::new(input), &mut out, MoveStats::default(), c).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn nonconsecutive_slots_startup_purge_arcs_and_retractions() {
        let c = config();
        let input = format!(
            "M83\nT0\nG1 E15 F600\nG1 E-5\nT2\nG1 E10\nG1 E-2\nG1 E2\nG1 X10 E4\nG2 X20 I5 E6\nT0\nG1 E5\n{FILAMENT_MARKER}\n{FOOTER}"
        );
        let out = processed(input, &c);
        assert!(out.ends_with(concat!(
            "; filament used [mm] = 15.00, 0.00, 20.00, 0.00\n",
            "; filament used [cm3] = 0.04, 0.00, 0.13, 0.00\n",
            "; filament used [g] = 0.04, 0.00, 0.14, 0.00\n",
            "; filament cost = 0.00, 0.00, 0.01, 0.00\n",
            "; total filament used [g] = 0.19\n",
            "; total filament cost = 0.02\n",
        )));
        assert_eq!(processed(out.clone(), &c), out);
    }

    #[test]
    fn weight_and_cost_gates_use_unrounded_positive_values() {
        let mut c = config();
        for (density, cost, weight_line, cost_line) in [
            ("0", "60", false, false),
            ("1.24", "0", true, false),
            ("1.24", "60", true, true),
        ] {
            c.set_raw("filament_density", density).unwrap();
            c.set_raw("filament_cost", cost).unwrap();
            let out = processed(
                format!("M83\nG1 E0.00001 F600\n{FILAMENT_MARKER}\n{FOOTER}"),
                &c,
            );
            assert_eq!(out.contains("; filament used [g] ="), weight_line);
            assert_eq!(out.contains("; filament cost ="), cost_line);
            assert!(out.contains("; total filament used [g] = 0.00\n"));
            assert!(out.contains("; total filament cost = 0.00\n"));
            assert_eq!(processed(out.clone(), &c), out);
        }
    }

    #[test]
    fn statistics_follow_rules_independently_of_firmware() {
        for flavor in ["klipper", "marlin2"] {
            let mut c = config();
            c.set_raw("gcode_flavor", flavor).unwrap();
            let input = format!("M83\nG1 E15 F600\n{FILAMENT_MARKER}\n{FOOTER}");
            assert!(processed(input.clone(), &c).contains("15.00, 0.00, 0.00, 0.00"));
            Rules::PrusaSlicer.set(&mut c);
            assert!(processed(input, &c).contains("; filament used [mm] = 15.00\n"));
        }
    }
}
