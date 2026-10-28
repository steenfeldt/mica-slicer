// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/PrintObject.cpp :: PrintObject::combine_infill
// Changes: Compact thickness-tagged areas; fixed interval combination only, bounded by the smaller nozzle of the
// Changes: region's sparse and solid infill tools (resolved with the region settings, print_job::region_settings).
// Upstream attribution (PrintObject.cpp):
// Copyright (c) Prusa Research 2016 - 2023 Lukáš Hejl @hejllukas, Pavel Mikuš @Godrak, Lukáš Matěna @lukasmatena, Vojtěch Bubník @bubnikv, Enrico Turri @enricoturri1966, Oleksandra Iushchenko @YuSanka, David Kocík @kocikdav, Roman Beránek @zavorka
// Copyright (c) 2021 Justin Schuh @jschuh
// Copyright (c) 2021 Ilya @xorza
// Copyright (c) 2016 Joseph Lenox @lordofhyphens
// Copyright (c) Slic3r 2014 - 2016 Alessandro Ranellucci @alranel
// Copyright (c) 2015 Maksim Derbasov @ntfshard
//
// PrusaSlicer is released under the terms of the AGPLv3 or higher
//

use crate::geometry::clip::{Join, diff_ex, intersection_ex, offset_ex};
use crate::geometry::{ExPolygon, SCALE, signed_area};
use crate::settings::PrintSettings;
use crate::shells::FillSurfaces;

#[derive(Clone, Debug, PartialEq)]
pub struct CombinedFill {
    pub polygons: Vec<ExPolygon>,
    pub height: f64,
}

pub(crate) fn combine_infill(surfaces: &mut [FillSurfaces], heights: &[f64], s: &PrintSettings) {
    if s.infill_every_layers < 2 || s.fill_density <= 0. || surfaces.len() < 2 {
        return;
    }
    let mut first = 1;
    while first < surfaces.len() {
        let mut end = first + 1;
        let mut height = heights[first];
        while end < surfaces.len()
            && end - first < s.infill_every_layers
            && height + heights[end] < s.infill_combine_nozzle + 1e-4
        {
            height += heights[end];
            end += 1;
        }
        if end - first > 1 {
            let mut common = surfaces[first].internal.clone();
            for f in &surfaces[first + 1..end] {
                common = intersection_ex(&common, &f.internal);
            }
            let threshold = crate::flow::Flow::new(s.solid_infill_extrusion_width, heights[first])
                .spacing()
                .powi(2)
                * SCALE
                * SCALE;
            common.retain(|p| {
                signed_area(&p.contour).abs()
                    - p.holes.iter().map(|h| signed_area(h).abs()).sum::<f64>()
                    > threshold
            });
            if !common.is_empty() {
                use crate::fill::Pattern;
                let factor = if matches!(
                    s.fill_pattern,
                    Pattern::Rectilinear | Pattern::Monotonic | Pattern::Grid | Pattern::Honeycomb
                ) {
                    1.5
                } else {
                    0.5
                };
                let clearance = offset_ex(
                    &common,
                    (0.5 * s.perimeter_extrusion_width + factor * s.solid_infill_extrusion_width)
                        * SCALE,
                    Join::Miter,
                );
                for f in &mut surfaces[first..end] {
                    f.internal = diff_ex(&f.internal, &clearance);
                }
                surfaces[end - 1].combined.push(CombinedFill {
                    polygons: common,
                    height,
                });
            }
        }
        first = end;
    }
}
