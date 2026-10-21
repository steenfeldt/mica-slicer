// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Optional diagnostic aggregation of the existing planner timeline. Counts refer to physical
//! G-code moves; an arc's internal planner segments all belong to that same move and role.
use super::*;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default)]
pub struct LayerBreakdown {
    /// Layer zero contains startup; following layers are one-based.
    pub seconds: f64,
    pub roles: BTreeMap<String, f64>,
    pub travel_seconds: f64,
    pub retract_seconds: f64,
    pub unretract_seconds: f64,
    pub other_seconds: f64,
    pub extrusion_moves: u64,
    pub arcs: u64,
    pub retracts: u64,
    pub z_moves: u64,
    pub z_only_moves: u64,
    /// Subset of travel_seconds spent on vertical-only motion.
    pub z_only_seconds: f64,
}
#[derive(Clone, Debug, Default)]
pub struct Breakdown {
    pub layers: BTreeMap<u64, LayerBreakdown>,
    pub total: LayerBreakdown,
}
impl Analysis {
    /// Normal-mode role/layer accounting, using exactly the planner timeline used for M73.
    /// Pass the same config used to create this Analysis.
    /// Custom motion is counted under its TYPE; no durations are guessed for opaque macros.
    pub fn breakdown<R: BufRead + Seek>(
        &mut self,
        src: &mut R,
        config: &Config,
    ) -> io::Result<Breakdown> {
        src.seek(io::SeekFrom::Start(self.start))?;
        self.timeline.file.rewind()?;
        let mut timeline = BufReader::new(&mut self.timeline.file);
        let mut next = record(&mut timeline)?;
        let mut last_time = 0.0;
        let mut estimator = Estimator::new(config);
        let mut report = Breakdown::default();
        let mut role = "Custom".to_string();
        let mut line = String::new();
        loop {
            line.clear();
            if src.read_line(&mut line)? == 0 {
                break;
            }
            if let Some(name) = line
                .trim()
                .strip_prefix(";TYPE:")
                .or_else(|| line.trim().strip_prefix("; FEATURE: "))
            {
                role = name.to_string();
            }
            let before = estimator.position;
            let before_id = estimator.g1;
            estimator.process_line(&line, &mut io::sink())?;
            if estimator.g1 == before_id {
                continue;
            }
            let after = estimator.position;
            let xy = (after[0] - before[0]).hypot(after[1] - before[1]) > 1e-9;
            let z = (after[2] - before[2]).abs() > 1e-9;
            let de = after[3] - before[3];
            let is_arc = matches!(Line::new(&line).command.as_str(), "G2" | "G3");
            let extrude = de > 0.0 && (xy || z || is_arc);
            let retract = de < 0.0;
            let unretract = de > 0.0 && !extrude;
            let travel = !extrude && !retract && !unretract && (xy || z);
            let mut seconds = 0.0;
            while let Some((id, times)) = next {
                if id > estimator.g1 {
                    break;
                }
                seconds += times[0] - last_time;
                last_time = times[0];
                next = record(&mut timeline)?;
            }
            for stats in [
                &mut report.total,
                report.layers.entry(estimator.layer).or_default(),
            ] {
                stats.seconds += seconds;
                stats.z_moves += u64::from(z);
                stats.z_only_moves += u64::from(z && !xy);
                if travel && z && !xy {
                    stats.z_only_seconds += seconds;
                }
                stats.arcs += u64::from(is_arc);
                stats.retracts += u64::from(retract);
                stats.extrusion_moves += u64::from(extrude);
                if extrude {
                    *stats.roles.entry(role.clone()).or_default() += seconds;
                } else if retract {
                    stats.retract_seconds += seconds;
                } else if unretract {
                    stats.unretract_seconds += seconds;
                } else if travel {
                    stats.travel_seconds += seconds;
                } else {
                    stats.other_seconds += seconds;
                }
            }
        }
        // A final tool-change load/unload can add time without a following motion record.
        let residual = self.estimate.normal - report.total.seconds;
        if residual > 0.0 {
            report.total.seconds += residual;
            report.total.other_seconds += residual;
            let layer = report.layers.entry(estimator.layer).or_default();
            layer.seconds += residual;
            layer.other_seconds += residual;
        }
        src.seek(io::SeekFrom::Start(self.start))?;
        Ok(report)
    }
}
