// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: orca@8500fcdcca src/libslic3r/PrintObject.cpp :: PrintObject::bridge_over_infill
// Source: prusaslicer@b028299c77 src/libslic3r/PrintObject.cpp :: PrintObject::bridge_over_infill
// Changes: Bridge support crosses boundaries of geometry-compatible speed regions; candidate shells
// Changes: and output ownership remain per region. Different infill geometries keep independent contexts.

// Attribution from Prusa PrintObject.cpp:
// Copyright (c) Prusa Research 2016 - 2023 Lukáš Hejl @hejllukas, Pavel Mikuš @Godrak, Lukáš Matěna @lukasmatena, Vojtěch Bubník @bubnikv, Enrico Turri @enricoturri1966, Oleksandra Iushchenko @YuSanka, David Kocík @kocikdav, Roman Beránek @zavorka
// Copyright (c) 2021 Justin Schuh @jschuh
// Copyright (c) 2021 Ilya @xorza
// Copyright (c) 2016 Joseph Lenox @lordofhyphens
// Copyright (c) Slic3r 2014 - 2016 Alessandro Ranellucci @alranel
// Copyright (c) 2015 Maksim Derbasov @ntfshard
//
// PrusaSlicer is released under the terms of the AGPLv3 or higher
//

use super::{LayerRegion, PrintRegion};
use crate::geometry::{Point, clip::union_ex};
use crate::settings::PrintSettings;
use crate::shells::FillSurfaces;

/// Speeds do not change shell support. Keep every other resolved parameter in the comparison,
/// including flows, tools, bridge settings, sparse patterns and shell thicknesses.
fn geometry_settings(s: &PrintSettings) -> PrintSettings {
    let mut s = s.clone();
    s.perimeter_speed = 0.;
    s.external_perimeter_speed = 0.;
    s.infill_speed = 0.;
    s.solid_infill_speed = 0.;
    s.top_solid_infill_speed = 0.;
    s.gap_fill_speed = 0.;
    s
}

pub(super) fn groups(table: &[PrintRegion]) -> Vec<Vec<usize>> {
    let settings: Vec<_> = table
        .iter()
        .map(|r| geometry_settings(&r.settings))
        .collect();
    let mut groups: Vec<Vec<usize>> = vec![];
    for (id, s) in settings.iter().enumerate() {
        // Object-dependent infill retains its independently prepared region context.
        if let Some(group) = groups
            .iter_mut()
            .find(|g| !s.fill_pattern.needs_object() && settings[g[0]] == *s)
        {
            group.push(id);
        } else {
            groups.push(vec![id]);
        }
    }
    groups
}

pub(super) fn bridge_and_combine(
    table: &[PrintRegion],
    layers: &mut [Vec<LayerRegion>],
    tops: &[f64],
    heights: &[f64],
    origin: Point,
    cancelled: &(dyn Fn() -> bool + Sync),
) {
    for group in groups(table) {
        if cancelled() {
            return;
        }
        // Immutable lower-layer support, gathered before any candidate is replaced with bridging.
        let support: Vec<_> = layers
            .iter()
            .map(|layer| {
                let mut merged = FillSurfaces::default();
                for r in layer.iter().filter(|r| group.contains(&r.region)) {
                    merged.top.extend_from_slice(&r.surfaces.top);
                    merged.bottom.extend_from_slice(&r.surfaces.bottom);
                    merged
                        .bottom_bridge
                        .extend_from_slice(&r.surfaces.bottom_bridge);
                    merged.internal.extend_from_slice(&r.surfaces.internal);
                    merged
                        .internal_solid
                        .extend_from_slice(&r.surfaces.internal_solid);
                    merged
                        .internal_bridge
                        .extend_from_slice(&r.surfaces.internal_bridge);
                }
                merged.internal = union_ex(&merged.internal);
                merged
            })
            .collect();
        for id in group {
            let s = &table[id].settings;
            let mut surfaces: Vec<_> = layers
                .iter_mut()
                .map(|l| {
                    l.iter_mut()
                        .find(|r| r.region == id)
                        .map(|r| std::mem::take(&mut r.surfaces))
                        .unwrap_or_default()
                })
                .collect();
            if !s.fill_pattern.needs_object() {
                let shell_layers: Vec<_> = tops
                    .iter()
                    .zip(heights)
                    .enumerate()
                    .map(|(i, (&print_z, &height))| {
                        let f = crate::print::LayerFlows::new(s, i, height);
                        crate::shells::ShellLayer {
                            print_z,
                            height,
                            external: f.external,
                            perimeter: f.perimeter,
                            solid: f.solid,
                        }
                    })
                    .collect();
                crate::bridge::bridge_over_infill_supported(
                    &mut surfaces,
                    &shell_layers,
                    s,
                    origin,
                    None,
                    Some(&support),
                    cancelled,
                );
            }
            super::combine::combine_infill(&mut surfaces, heights, s);
            for (layer, surfaces) in layers.iter_mut().zip(surfaces) {
                if let Some(r) = layer.iter_mut().find(|r| r.region == id) {
                    r.surfaces = surfaces;
                }
            }
        }
    }
}
