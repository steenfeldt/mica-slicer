// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: snapmaker-orca@b1831e5dcb src/slic3r/GUI/filamentsync/FilamentSyncAlgorithm.cpp :: compute_color_match, rgb_to_lab, srgb_to_linear, lab_f; src/slic3r/Utils/ColorSpaceConvert.cpp :: DeltaE00; src/slic3r/GUI/filamentsync/FilamentData.hpp :: is_none_filament, getMainColor
// Changes: maps the job's used tools (not every project filament) onto the U1's slots; types compare trimmed and case-insensitively; a slot the printer reports as empty counts as NONE; per-tool verdicts (type mismatch = warning, empty slot = error) for the proposal and for manual overrides; f64 math.

//! Proposes which U1 slot feeds each tool of a sliced job: a slot with the same filament
//! type and the perceptually nearest color (CIEDE2000), else the nearest color among all
//! loaded slots, as Snapmaker Orca's filament sync does.

use crate::U1Slot;

/// One tool the job uses, as the project describes its filament.
#[derive(Clone, Debug, PartialEq)]
pub struct MapTool {
    /// Zero based (`T0` = 0).
    pub tool: u32,
    pub material: Option<String>,
    pub colour: Option<[u8; 3]>,
}

/// How a tool → slot choice looks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MapIssue {
    /// A warning: the slot holds another filament type.
    TypeMismatch { wanted: String, loaded: String },
    /// An error: the printer reports the slot empty.
    SlotEmpty,
    /// An error: no slot holds filament, so nothing could be proposed.
    NoSlot,
}
impl MapIssue {
    pub fn is_error(&self) -> bool {
        matches!(self, Self::SlotEmpty | Self::NoSlot)
    }
    pub fn text(&self, tool: u32, slot: Option<u32>) -> String {
        let slot = slot.map(|s| format!("slot {}", s + 1)).unwrap_or_default();
        match self {
            Self::TypeMismatch { wanted, loaded } => {
                format!("T{}: wants {wanted}, {slot} holds {loaded}", tool + 1)
            }
            Self::SlotEmpty => format!("T{}: {slot} is empty", tool + 1),
            Self::NoSlot => format!("T{}: no slot holds filament", tool + 1),
        }
    }
}

/// The proposal (or a user's choice) for one tool.
#[derive(Clone, Debug, PartialEq)]
pub struct MapChoice {
    pub tool: u32,
    /// Zero based slot index; `None` only when nothing is loaded at all.
    pub slot: Option<u32>,
    pub issues: Vec<MapIssue>,
}

/// Upstream `is_none_filament`, plus a slot the printer reports as not present.
fn is_none(slot: &U1Slot) -> bool {
    slot.exists == Some(false)
        || slot
            .material
            .as_deref()
            .is_none_or(|m| m.trim().is_empty() || m.trim().eq_ignore_ascii_case("NONE"))
}

fn same_type(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

/// Upstream `getMainColor`: an unreadable color is `#26A69A`.
const DEFAULT_COLOUR: [u8; 3] = [0x26, 0xA6, 0x9A];

pub(crate) fn slot_rgb(slot: &U1Slot) -> [u8; 3] {
    slot.colour
        .as_deref()
        .and_then(mica_core::color::parse_hex)
        .unwrap_or(DEFAULT_COLOUR)
}

/// The verdicts for tool `tool` printed from `slot`.
pub fn assess(tool: &MapTool, slot: Option<u32>, slots: &[U1Slot]) -> Vec<MapIssue> {
    let Some(s) = slot.and_then(|s| slots.iter().find(|x| x.index == s)) else {
        return vec![MapIssue::NoSlot];
    };
    let mut issues = vec![];
    if is_none(s) {
        issues.push(MapIssue::SlotEmpty);
        return issues;
    }
    if let (Some(w), Some(l)) = (&tool.material, &s.material)
        && !w.trim().is_empty()
        && !same_type(w, l)
    {
        issues.push(MapIssue::TypeMismatch {
            wanted: w.trim().into(),
            loaded: l.trim().into(),
        });
    }
    issues
}

/// Upstream `compute_color_match` over the job's tools and the printer's slots.
pub fn propose(tools: &[MapTool], slots: &[U1Slot]) -> Vec<MapChoice> {
    let lab: Vec<[f64; 3]> = slots.iter().map(|s| rgb_to_lab(slot_rgb(s))).collect();
    tools
        .iter()
        .map(|t| {
            let design = rgb_to_lab(t.colour.unwrap_or(DEFAULT_COLOUR));
            let pick = |same: bool| {
                let mut best: Option<(f64, usize)> = None;
                for (j, s) in slots.iter().enumerate() {
                    if is_none(s) {
                        continue;
                    }
                    if same {
                        match (&t.material, &s.material) {
                            (Some(w), Some(l)) if same_type(w, l) => {}
                            _ => continue,
                        }
                    }
                    let d = delta_e00(design, lab[j]);
                    if best.is_none_or(|(b, _)| d < b) {
                        best = Some((d, j));
                    }
                }
                best.map(|(_, j)| slots[j].index)
            };
            let slot = pick(true).or_else(|| pick(false));
            MapChoice {
                tool: t.tool,
                slot,
                issues: assess(t, slot, slots),
            }
        })
        .collect()
}

fn srgb_to_linear(c: u8) -> f64 {
    let c = c as f64 / 255.;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn lab_f(t: f64) -> f64 {
    let d: f64 = 6. / 29.;
    if t > d * d * d {
        t.cbrt()
    } else {
        t / (3. * d * d) + 4. / 29.
    }
}

pub(crate) fn rgb_to_lab([r, g, b]: [u8; 3]) -> [f64; 3] {
    let (lr, lg, lb) = (srgb_to_linear(r), srgb_to_linear(g), srgb_to_linear(b));
    let x = 0.4124564 * lr + 0.3575761 * lg + 0.1804375 * lb;
    let y = 0.2126729 * lr + 0.7151522 * lg + 0.0721750 * lb;
    let z = 0.0193339 * lr + 0.1191920 * lg + 0.9503041 * lb;
    let (fx, fy, fz) = (lab_f(x / 0.95047), lab_f(y), lab_f(z / 1.08883));
    [116. * fy - 16., 500. * (fx - fy), 200. * (fy - fz)]
}

/// Upstream `DeltaE00` (CIEDE2000, kL = kC = kH = 1).
pub(crate) fn delta_e00([l1, a1, b1]: [f64; 3], [l2, a2, b2]: [f64; 3]) -> f64 {
    use std::f64::consts::PI;
    let rad2deg = |r: f64| 360. * r / (2. * PI);
    let deg2rad = |d: f64| 2. * PI * d / 360.;
    let avg_l = (l1 + l2) / 2.;
    let c1 = (a1 * a1 + b1 * b1).sqrt();
    let c2 = (a2 * a2 + b2 * b2).sqrt();
    let avg_c = (c1 + c2) / 2.;
    let g = (1. - (avg_c.powi(7) / (avg_c.powi(7) + 25f64.powi(7))).sqrt()) / 2.;
    let a1p = a1 * (1. + g);
    let a2p = a2 * (1. + g);
    let c1p = (a1p * a1p + b1 * b1).sqrt();
    let c2p = (a2p * a2p + b2 * b2).sqrt();
    let avg_cp = (c1p + c2p) / 2.;
    let mut h1p = rad2deg(b1.atan2(a1p));
    if h1p < 0. {
        h1p += 360.;
    }
    let mut h2p = rad2deg(b2.atan2(a2p));
    if h2p < 0. {
        h2p += 360.;
    }
    let avg_hp = if (h1p - h2p).abs() > 180. {
        (h1p + h2p + 360.) / 2.
    } else {
        (h1p + h2p) / 2.
    };
    let t = 1. - 0.17 * deg2rad(avg_hp - 30.).cos()
        + 0.24 * deg2rad(2. * avg_hp).cos()
        + 0.32 * deg2rad(3. * avg_hp + 6.).cos()
        - 0.2 * deg2rad(4. * avg_hp - 63.).cos();
    let mut delta_hp = h2p - h1p;
    if delta_hp.abs() > 180. {
        if h2p <= h1p {
            delta_hp += 360.;
        } else {
            delta_hp -= 360.;
        }
    }
    let delta_lp = l2 - l1;
    let delta_cp = c2p - c1p;
    let delta_hp = 2. * (c1p * c2p).sqrt() * (deg2rad(delta_hp) / 2.).sin();
    let sl = 1. + (0.015 * (avg_l - 50.).powi(2)) / (20. + (avg_l - 50.).powi(2)).sqrt();
    let sc = 1. + 0.045 * avg_cp;
    let sh = 1. + 0.015 * avg_cp * t;
    let delta_ro = 30. * (-((avg_hp - 275.) / 25.).powi(2)).exp();
    let rc = 2. * (avg_cp.powi(7) / (avg_cp.powi(7) + 25f64.powi(7))).sqrt();
    let rt = -rc * (2. * deg2rad(delta_ro)).sin();
    ((delta_lp / sl).powi(2)
        + (delta_cp / sc).powi(2)
        + (delta_hp / sh).powi(2)
        + rt * (delta_cp / sc) * (delta_hp / sh))
        .sqrt()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::u1::parse_task_config;
    use serde_json::json;

    /// A real U1 as read on 2026-10-20 (firmware 1.5.2.13): four PLA spools, yellow, white,
    /// brown and black; the object reports no `filament_exist`.
    pub(crate) fn sample_u1() -> serde_json::Value {
        json!({
            "filament_type": ["PLA","PLA","PLA","PLA"],
            "filament_color_rgba": ["F8F81CFF","FFFFFFFF","6F4C2FFF","000000FF"],
            "extruder_map_table": [0,1,2,3,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],
            "extruders_used": [false,false,false,false],
            "extruders_replenished": [0,1,2,3],
            "time_lapse_camera": false,
            "auto_bed_leveling": false,
            "flow_calibrate": false,
            "shaper_calibrate": false,
            "auto_replenish_filament": true,
            "flow_calib_extruders": [true,true,true,true],
            "filament_color_multi": [
                {"nums":1,"alpha":255,"mode":0,"colors":["F8F81CFF"]},
                {"nums":1,"alpha":255,"mode":0,"colors":["FFFFFFFF"]},
                {"nums":1,"alpha":255,"mode":0,"colors":["6F4C2FFF"]},
                {"nums":1,"alpha":255,"mode":0,"colors":["000000FF"]}
            ],
            "replenish_ignore_color": false
        })
    }
    fn tool(tool: u32, material: &str, colour: [u8; 3]) -> MapTool {
        MapTool {
            tool,
            material: Some(material.into()),
            colour: Some(colour),
        }
    }

    #[test]
    fn delta_e00_matches_the_published_test_pairs() {
        // Sharma, Wu & Dalal (2005), pairs 1 and 17.
        let d = delta_e00([50., 2.6772, -79.7751], [50., 0., -82.7485]);
        assert!((d - 2.0425).abs() < 1e-3, "{d}");
        let d = delta_e00([50., 2.5, 0.], [73., 25., -18.]);
        assert!((d - 27.1492).abs() < 1e-3, "{d}");
        assert!(delta_e00(rgb_to_lab([10, 20, 30]), rgb_to_lab([10, 20, 30])) < 1e-9);
    }

    #[test]
    fn sample_u1_proposals_follow_nearest_colour() {
        let cfg = parse_task_config(&sample_u1());
        assert_eq!(cfg.slots.len(), 4);
        assert!(cfg.slots.iter().all(|s| s.exists.is_none()));
        let tools = [
            tool(0, "PLA", [0x00, 0x00, 0x00]), // black → slot 4
            tool(1, "PLA", [0xFF, 0xFF, 0x00]), // yellow → slot 1
            tool(2, "PLA", [0x80, 0x55, 0x33]), // brown → slot 3
            tool(3, "PLA", [0xF0, 0xF0, 0xF0]), // off-white → slot 2
        ];
        let p = propose(&tools, &cfg.slots);
        assert_eq!(
            p.iter().map(|c| c.slot).collect::<Vec<_>>(),
            [Some(3), Some(0), Some(2), Some(1)]
        );
        assert!(p.iter().all(|c| c.issues.is_empty()));
    }

    #[test]
    fn type_comes_before_colour_and_mismatches_are_flagged() {
        let mut v = sample_u1();
        v["filament_type"] = json!(["PLA", "PETG", "pla", "PLA"]);
        let cfg = parse_task_config(&v);
        // A white PETG part goes to the PETG slot (2, white) even with whiter PLA elsewhere.
        let p = propose(&[tool(0, "PETG", [0xFF, 0x00, 0x00])], &cfg.slots);
        assert_eq!(p[0].slot, Some(1));
        assert!(p[0].issues.is_empty());
        // Case-insensitive type match: brown PLA picks the "pla" brown slot.
        let p = propose(&[tool(0, "PLA", [0x6F, 0x4C, 0x2F])], &cfg.slots);
        assert_eq!(p[0].slot, Some(2));
        // No ABS loaded: nearest colour across all slots, flagged as a type mismatch.
        let p = propose(&[tool(1, "ABS", [0x10, 0x10, 0x10])], &cfg.slots);
        assert_eq!(p[0].slot, Some(3));
        assert_eq!(
            p[0].issues,
            [MapIssue::TypeMismatch {
                wanted: "ABS".into(),
                loaded: "PLA".into()
            }]
        );
        assert!(!p[0].issues[0].is_error());
        assert_eq!(
            p[0].issues[0].text(1, Some(3)),
            "T2: wants ABS, slot 4 holds PLA"
        );
    }

    #[test]
    fn empty_slots_are_skipped_and_flagged_when_chosen() {
        let mut v = sample_u1();
        v["filament_exist"] = json!([true, false, true, true]);
        let cfg = parse_task_config(&v);
        // White is in slot 2, but slot 2 is empty: the proposal avoids it.
        let p = propose(&[tool(0, "PLA", [0xFF, 0xFF, 0xFF])], &cfg.slots);
        assert_ne!(p[0].slot, Some(1));
        // Choosing it anyway is an error.
        let issues = assess(&tool(0, "PLA", [0xFF, 0xFF, 0xFF]), Some(1), &cfg.slots);
        assert_eq!(issues, [MapIssue::SlotEmpty]);
        assert!(issues[0].is_error());
        // Nothing loaded at all.
        v["filament_exist"] = json!([false, false, false, false]);
        let cfg = parse_task_config(&v);
        let p = propose(&[tool(0, "PLA", [1, 2, 3])], &cfg.slots);
        assert_eq!(p[0].slot, None);
        assert_eq!(p[0].issues, [MapIssue::NoSlot]);
        // A slot typed "NONE" or blank counts as empty too (upstream is_none_filament).
        v["filament_exist"] = json!([true, true, true, true]);
        v["filament_type"] = json!(["NONE", "", "PLA", "PLA"]);
        let cfg = parse_task_config(&v);
        assert_eq!(
            assess(&tool(0, "PLA", [0; 3]), Some(0), &cfg.slots),
            [MapIssue::SlotEmpty]
        );
    }

    #[test]
    fn unknown_project_colour_uses_upstream_default() {
        let cfg = parse_task_config(&sample_u1());
        let t = MapTool {
            tool: 0,
            material: None,
            colour: None,
        };
        // #26A69A (teal) is nearest to… whichever slot; the point is a stable, typed answer.
        let p = propose(std::slice::from_ref(&t), &cfg.slots);
        assert!(p[0].slot.is_some());
        assert!(p[0].issues.is_empty(), "no type means no mismatch");
        let parse_hex = mica_core::color::parse_hex;
        assert_eq!(parse_hex("#F8F81CFF"), Some([0xF8, 0xF8, 0x1C]));
        assert_eq!(parse_hex("F8F81C"), Some([0xF8, 0xF8, 0x1C]));
        assert_eq!(parse_hex("F8F"), None);
    }
}
