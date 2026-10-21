// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/Slicing.cpp :: layer_height_profile_from_ranges, generate_object_layers
// Changes: Piecewise constant range heights; preserve Mica's existing constant-height schedule without overrides; a variable layer height profile goes through layer_profile::tops; no Z shrinkage.
// Upstream attribution (Slicing.cpp):
// Copyright (c) Prusa Research 2016 - 2023 Vojtěch Bubník @bubnikv, Enrico Turri @enricoturri1966, David Kocík @kocikdav, Lukáš Matěna @lukasmatena, Oleksandra Iushchenko @YuSanka
//
// PrusaSlicer is released under the terms of the AGPLv3 or higher
//

use super::RegionModel;
use crate::settings::PrintSettings;

pub(crate) fn layer_tops(
    model: &RegionModel,
    c: &crate::config::Config,
    s: &PrintSettings,
    max_z: f64,
) -> Result<Vec<f64>, String> {
    model.validate()?;
    if !model.layer_height_profile.is_empty() {
        let extruders: Vec<usize> = [
            "perimeter_extruder",
            "infill_extruder",
            "solid_infill_extruder",
        ]
        .iter()
        .filter_map(|k| c.int(k))
        .filter(|t| *t > 0)
        .flat_map(|t| model.full_spectrum.expand(t as usize))
        .map(|t| t - 1)
        .collect();
        let mut extruders = extruders;
        extruders.sort_unstable();
        extruders.dedup();
        let p = crate::layer_profile::SlicingParams::from_config(c, &extruders, max_z);
        if crate::layer_profile::valid(&p, &model.layer_height_profile) {
            return Ok(crate::layer_profile::tops(&p, &model.layer_height_profile));
        }
    }
    if !model.height_ranges.iter().any(|r| {
        r.config
            .float("layer_height")
            .is_some_and(|h| (h - s.layer_height).abs() > 1e-9)
    }) {
        return Ok(s.layer_tops(max_z));
    }
    let minimum = if s.min_layer_height > 0. {
        s.min_layer_height
    } else {
        0.07
    };
    let maximum = if s.max_layer_height > 0. {
        s.max_layer_height
    } else {
        0.75 * s.nozzle_diameter
    };
    for r in &model.height_ranges {
        if let Some(h) = r.config.float("layer_height")
            && (!h.is_finite() || h < minimum - 1e-6 || h > maximum + 1e-6)
        {
            return Err(format!(
                "range layer_height {h} outside {minimum}..={maximum}"
            ));
        }
    }
    let mut z = s.first_layer_height;
    let mut out = vec![z];
    while z + 0.5 * minimum < max_z {
        let sample = z + 0.5 * minimum;
        let h = model
            .height_ranges
            .iter()
            .find(|r| sample >= r.min_z && sample < r.max_z)
            .and_then(|r| r.config.float("layer_height"))
            .unwrap_or(s.layer_height);
        if z + 0.5 * h >= max_z {
            break;
        }
        z = ((z + h) * 1e9).round() / 1e9;
        out.push(z);
    }
    Ok(out)
}
