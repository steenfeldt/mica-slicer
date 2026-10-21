// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: orca@8500fcdcca src/slic3r/GUI/PartPlate.cpp :: PartPlate::estimate_wipe_tower_polygon
// Source: orca@8500fcdcca src/libslic3r/GCode/WipeTower2.cpp :: WipeTower2::WipeTower2
// Changes: Import per-plate bed-local X/Y and shared rotation without applying embedded presets; reject malformed placement values.

use mica_core::threemf::RawConfig;
use serde_json::Value;
use std::collections::BTreeMap;

pub(super) fn placements(
    config: Option<&RawConfig>,
    plates: usize,
) -> Result<BTreeMap<u32, [f64; 3]>, String> {
    let mut out = BTreeMap::new();
    let Some(config) = config.filter(|c| c.path == "Metadata/project_settings.config") else {
        return Ok(out);
    };
    let json: Value =
        serde_json::from_str(&config.text).map_err(|e| format!("3MF project settings: {e}"))?;
    if json.get("wipe_tower_x").is_none() && json.get("wipe_tower_y").is_none() {
        return Ok(out);
    }
    let values = |key: &str, default: f64| -> Result<Vec<f64>, String> {
        let Some(value) = json.get(key) else {
            return Ok(vec![default]);
        };
        let items = value
            .as_array()
            .map_or_else(|| vec![value], |a| a.iter().collect());
        if items.is_empty() {
            return Err(format!("3MF {key}: empty tower placement vector"));
        }
        items
            .into_iter()
            .map(|v| {
                v.as_str()
                    .and_then(|s| s.trim().parse::<f64>().ok())
                    .or_else(|| v.as_f64())
                    .filter(|v| v.is_finite())
                    .ok_or_else(|| format!("3MF {key}: tower placement must be finite"))
            })
            .collect()
    };
    let xs = values("wipe_tower_x", 15.)?;
    let ys = values("wipe_tower_y", 220.)?;
    let rotation = values("wipe_tower_rotation_angle", 0.)?;
    if rotation.len() != 1 {
        return Err("3MF wipe_tower_rotation_angle: expected a shared scalar rotation".into());
    }
    for i in 0..plates.max(xs.len()).max(ys.len()) {
        // ConfigOptionVector::get_at falls back to the first element for older projects
        // whose placement vectors have fewer entries than plates.
        out.insert(
            i as u32,
            [
                *xs.get(i).unwrap_or(&xs[0]),
                *ys.get(i).unwrap_or(&ys[0]),
                rotation[0],
            ],
        );
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str, plates: usize) -> Result<BTreeMap<u32, [f64; 3]>, String> {
        placements(
            Some(&RawConfig {
                path: "Metadata/project_settings.config".into(),
                text: text.into(),
            }),
            plates,
        )
    }

    #[test]
    fn bed_local_positions_rotation_and_short_vectors() {
        let p = parse(r#"{"wipe_tower_x":["165","201.433"],"wipe_tower_y":["233.362","221.855"],"wipe_tower_rotation_angle":"90"}"#, 3).unwrap();
        assert_eq!(p[&0], [165., 233.362, 90.]);
        assert_eq!(p[&1], [201.433, 221.855, 90.]);
        assert_eq!(p[&2], p[&0]);
        assert!(parse("{}", 5).unwrap().is_empty());
    }

    #[test]
    fn malformed_coordinates_are_errors() {
        for text in [
            r#"{"wipe_tower_x":["NaN"]}"#,
            r#"{"wipe_tower_y":[]}"#,
            r#"{"wipe_tower_rotation_angle":"inf","wipe_tower_x":["165"]}"#,
        ] {
            assert!(parse(text, 1).is_err(), "{text}");
        }
    }
}
