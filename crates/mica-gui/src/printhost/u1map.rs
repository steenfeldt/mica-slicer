// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The Print dialog's Snapmaker U1 part: which printer slot feeds each tool the job uses
//! (proposed by type and nearest color, editable), and the start options (bed leveling,
//! flow calibration for chosen tools, timelapse). Defaults are the printer's current
//! `print_task_config`, or the choices last sent to this printer (remembered in `mica.toml`).

use super::explain::JobTool;
use mica_printhost::{
    U1Slot, U1StartOptions, U1TaskConfig,
    u1_mapping::{MapIssue, MapTool, assess, propose},
};
use serde::{Deserialize, Serialize};

/// The start options last sent to one U1, by printer name.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Remembered {
    pub bed_leveling: bool,
    pub flow_calibration: bool,
    pub timelapse: bool,
}

/// One used tool and the slot it prints from.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub job: JobTool,
    /// Zero based; `None` only when no slot holds filament.
    pub slot: Option<u32>,
    pub issues: Vec<MapIssue>,
    /// Flow calibration for this tool (when the option is on).
    pub calibrate: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    pub slots: Vec<U1Slot>,
    pub rows: Vec<Row>,
    pub bed_leveling: bool,
    pub flow_calibration: bool,
    pub timelapse: bool,
    pub firmware: Option<String>,
}

fn map_tool(t: &JobTool) -> MapTool {
    MapTool {
        tool: t.tool,
        material: t.material.clone(),
        colour: t.colour,
    }
}

impl Plan {
    pub fn new(
        config: &U1TaskConfig,
        firmware: Option<String>,
        tools: &[JobTool],
        remembered: Option<Remembered>,
    ) -> Self {
        let mapped: Vec<MapTool> = tools.iter().map(map_tool).collect();
        let proposal = propose(&mapped, &config.slots);
        let rows = tools
            .iter()
            .zip(proposal)
            .map(|(t, c)| Row {
                job: t.clone(),
                slot: c.slot,
                issues: c.issues,
                // The printer's per-slot choice for the proposed slot (on when unknown).
                calibrate: c
                    .slot
                    .and_then(|s| config.flow_calib_slots.get(s as usize).copied())
                    .unwrap_or(true),
            })
            .collect();
        let r = remembered.unwrap_or(Remembered {
            bed_leveling: config.bed_leveling,
            flow_calibration: config.flow_calibrate,
            timelapse: config.timelapse,
        });
        Self {
            slots: config.slots.clone(),
            rows,
            bed_leveling: r.bed_leveling,
            flow_calibration: r.flow_calibration,
            timelapse: r.timelapse,
            firmware,
        }
    }
    /// The user picks `slot` for row `row`.
    pub fn choose(&mut self, row: usize, slot: u32) {
        if let Some(r) = self.rows.get_mut(row) {
            r.slot = Some(slot);
            r.issues = assess(&map_tool(&r.job), r.slot, &self.slots);
        }
    }
    /// Errors that block the start (an empty or missing slot).
    pub fn errors(&self) -> Vec<String> {
        self.rows
            .iter()
            .flat_map(|r| {
                r.issues
                    .iter()
                    .filter(|i| i.is_error())
                    .map(|i| i.text(r.job.tool, r.slot))
            })
            .collect()
    }
    pub fn warnings(&self) -> Vec<String> {
        self.rows
            .iter()
            .flat_map(|r| {
                r.issues
                    .iter()
                    .filter(|i| !i.is_error())
                    .map(|i| i.text(r.job.tool, r.slot))
            })
            .collect()
    }
    /// What to send; `None` while an error blocks it.
    pub fn options(&self) -> Option<U1StartOptions> {
        if !self.errors().is_empty() {
            return None;
        }
        Some(U1StartOptions {
            map: self
                .rows
                .iter()
                .map(|r| r.slot.map(|s| (r.job.tool, s)))
                .collect::<Option<Vec<_>>>()?,
            bed_leveling: self.bed_leveling,
            flow_calibration: self.flow_calibration,
            flow_tools: self
                .rows
                .iter()
                .filter(|r| r.calibrate)
                .map(|r| r.job.tool)
                .collect(),
            timelapse: self.timelapse,
        })
    }
    pub fn remembered(&self) -> Remembered {
        Remembered {
            bed_leveling: self.bed_leveling,
            flow_calibration: self.flow_calibration,
            timelapse: self.timelapse,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A real U1 (firmware 1.5.2.13) as read on 2026-10-20.
    fn sample_u1() -> U1TaskConfig {
        mica_printhost::parse_task_config(&json!({
            "filament_type": ["PLA","PLA","PLA","PLA"],
            "filament_color_rgba": ["F8F81CFF","FFFFFFFF","6F4C2FFF","000000FF"],
            "time_lapse_camera": false, "auto_bed_leveling": false, "flow_calibrate": false,
            "flow_calib_extruders": [true,true,true,true]
        }))
    }
    fn tool(tool: u32, material: &str, colour: [u8; 3]) -> JobTool {
        JobTool {
            tool,
            preset: format!("Snapmaker {material}"),
            material: Some(material.into()),
            colour: Some(colour),
        }
    }

    #[test]
    fn plan_proposes_follows_printer_defaults_and_remembers() {
        let tools = [tool(0, "PLA", [0x70, 0x4D, 0x30]), tool(1, "PLA", [255; 3])];
        let mut plan = Plan::new(&sample_u1(), Some("1.5.2.13".into()), &tools, None);
        assert_eq!(
            plan.rows.iter().map(|r| r.slot).collect::<Vec<_>>(),
            [Some(2), Some(1)]
        );
        assert!(!plan.bed_leveling && !plan.flow_calibration && !plan.timelapse);
        assert!(plan.rows.iter().all(|r| r.calibrate));
        // The review case: T0 → slot 3, T1 → slot 1, flow calibration on T0 only.
        plan.choose(1, 0);
        plan.flow_calibration = true;
        plan.rows[1].calibrate = false;
        let o = plan.options().unwrap();
        assert_eq!(o.map, [(0, 2), (1, 0)]);
        assert_eq!(o.flow_slots(), [2]);
        assert_eq!(
            plan.remembered(),
            Remembered {
                bed_leveling: false,
                flow_calibration: true,
                timelapse: false
            }
        );
        // A remembered choice wins over the printer's current flags.
        let plan = Plan::new(
            &sample_u1(),
            None,
            &tools,
            Some(Remembered {
                bed_leveling: true,
                flow_calibration: false,
                timelapse: true,
            }),
        );
        assert!(plan.bed_leveling && plan.timelapse && !plan.flow_calibration);
    }

    #[test]
    fn empty_slots_block_and_type_mismatches_warn() {
        let mut cfg = sample_u1();
        cfg.slots[3].exists = Some(false);
        cfg.slots[1].material = Some("PETG".into());
        let tools = [tool(0, "PLA", [0, 0, 0]), tool(1, "TPU", [255; 3])];
        let mut plan = Plan::new(&cfg, None, &tools, None);
        // Black is in the empty slot 4, so T1 goes elsewhere; TPU falls back to nearest white.
        assert_ne!(plan.rows[0].slot, Some(3));
        assert_eq!(plan.rows[1].slot, Some(1));
        assert_eq!(plan.warnings(), ["T2: wants TPU, slot 2 holds PETG"]);
        assert!(plan.options().is_some());
        plan.choose(0, 3);
        assert_eq!(plan.errors(), ["T1: slot 4 is empty"]);
        assert!(plan.options().is_none());
    }
}
