// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/Support/TreeModelVolumes.cpp :: calculateCollision, calculateAvoidance, calculatePlaceables, calculateWallRestrictions
// Source: prusaslicer@b028299c77 src/libslic3r/Support/TreeModelVolumes.hpp :: TreeModelVolumes
// Source: prusaslicer@b028299c77 src/libslic3r/Support/TreeSupport.cpp :: safe_offset_inc
// Changes: Single-object physical-Z slabs; exact offset reuse and parallel upstream radius buckets; two avoidance speeds,
// Changes: no hole-free cache or multi-object/build-volume masks. Sorted caches are deterministic.
// Copyright (c) Prusa Research 2022 - 2023 Vojtěch Bubník @bubnikv, Oleksandra Iushchenko @YuSanka
// PrusaSlicer is released under the terms of the AGPLv3 or higher.
// Tree supports by Thomas Rahm, losely based on Tree Supports by CuraEngine.
// Original source of Thomas Rahm's tree supports:
// https://github.com/ThomasRahm/CuraEngine
//
// Original CuraEngine copyright:
// Copyright (c) 2021 Ultimaker B.V.
// CuraEngine is released under the terms of the AGPLv3 or higher.

use super::tree_common::TreeSettings;
use super::tree_common::{offset_ex, simplify};
use super::{EPS, SupportInput, SupportLayer, SupportParameters};
use crate::geometry::clip::{Join, diff_ex, union_ex};
use crate::geometry::{ExPolygon, SCALE};
use rayon::prelude::*;
use std::collections::BTreeMap;

type Areas = Vec<ExPolygon>;
pub(super) struct ModelVolumes {
    pub collision: Vec<Areas>,
    pub minimum_collision: Vec<Areas>,
    pub placeable: Vec<Areas>,
    heights: Vec<f64>,
    radii: BTreeMap<i64, Vec<Areas>>,
    avoidance: BTreeMap<(i64, bool), Vec<Areas>>,
    tip_radius: f64,
    fast: f64,
    slow: f64,
}
impl ModelVolumes {
    pub fn new(
        input: &SupportInput<'_>,
        layers: &[SupportLayer],
        p: &SupportParameters,
        t: &TreeSettings,
    ) -> Self {
        // A source slice can intersect several support slabs with the same clearance.
        // Key by the exact f64 offset, never a rounded approximation of the geometry.
        let mut offsets = BTreeMap::new();
        let mut jobs = Vec::new();
        let mut offset_id = |source: usize, distance: f64| {
            *offsets
                .entry((source, distance.to_bits()))
                .or_insert_with(|| {
                    let id = jobs.len();
                    jobs.push((source, distance));
                    id
                })
        };
        let slabs: Vec<_> = layers
            .iter()
            .map(|layer| {
                let bottom = layer.print_z - layer.height;
                let mut obstacles = Vec::new();
                let mut minimum_obstacles = Vec::new();
                let mut ground = Vec::new();
                for (i, obj) in input.layers.iter().enumerate() {
                    let obj_bottom = obj.print_z - obj.height;
                    if obj.print_z > bottom - p.bottom_gap + EPS
                        && obj_bottom < layer.print_z + p.config.top_gap - EPS
                    {
                        let xy = if obj_bottom >= layer.print_z - EPS && p.config.top_gap > EPS {
                            p.config.xy_gap
                                * (1.0 - (obj_bottom - layer.print_z) / p.config.top_gap)
                                    .clamp(0.0, 1.0)
                        } else {
                            p.config.xy_gap
                        };
                        obstacles.push(offset_id(i, xy * SCALE));
                        let min_xy = if p.config.xy_gap > EPS {
                            xy * p.config.xy_gap.min(0.5 * p.config.external_width)
                                / p.config.xy_gap
                        } else {
                            0.0
                        };
                        minimum_obstacles.push(offset_id(i, min_xy * SCALE));
                    }
                    if obj.print_z <= bottom - p.bottom_gap + EPS
                        && obj.print_z > bottom - p.bottom_gap - layer.height - EPS
                    {
                        ground.push(i);
                    }
                }
                (obstacles, minimum_obstacles, ground)
            })
            .collect();
        let offsets: Vec<_> = jobs
            .par_iter()
            .map(|&(source, distance)| {
                offset_ex(input.layers[source].outlines, distance, Join::Round)
            })
            .collect();
        let computed: Vec<_> = slabs
            .par_iter()
            .map(|(obstacles, minimum, ground)| {
                let gather = |ids: &[usize]| {
                    ids.iter()
                        .flat_map(|&i| offsets[i].iter().cloned())
                        .collect::<Vec<_>>()
                };
                let obstacles = union_ex(&gather(obstacles));
                let ground: Vec<_> = ground
                    .iter()
                    .flat_map(|&i| input.layers[i].outlines.iter().cloned())
                    .collect();
                let placeable = diff_ex(&union_ex(&ground), &obstacles);
                (obstacles, union_ex(&gather(minimum)), placeable)
            })
            .collect();
        let mut collision = Vec::with_capacity(layers.len());
        let mut minimum_collision = Vec::with_capacity(layers.len());
        let mut placeable = Vec::with_capacity(layers.len());
        for (c, m, p) in computed {
            collision.push(c);
            minimum_collision.push(m);
            placeable.push(p);
        }
        Self {
            collision,
            minimum_collision,
            placeable,
            heights: layers.iter().map(|l| l.height).collect(),
            radii: BTreeMap::new(),
            avoidance: BTreeMap::new(),
            tip_radius: t.tip_radius,
            fast: t.fast,
            slow: t.slow,
        }
    }
    fn key(&self, radius: f64) -> i64 {
        if radius == 0.0 {
            return 0;
        }
        let mut out = self.tip_radius;
        if radius > out + 1e-8 {
            let delta = 1.5 - out;
            if delta > 0.5 {
                let steps = (delta / 1.5).ceil() as usize;
                let step = delta / steps as f64;
                out += step;
                for _ in 0..steps {
                    if out + 1e-8 >= radius {
                        return (out * SCALE).round() as i64;
                    }
                    out += step;
                }
            } else {
                out += 0.5;
            }
            while out + 1e-8 < radius {
                out *= 1.5;
            }
        }
        (out * SCALE).round() as i64
    }
    /// Populate only requested radius buckets through this layer. Collision layers are
    /// independent; avoidance keeps its bottom-up recurrence, parallel across radius/speed.
    pub fn prepare(&mut self, requests: &[(f64, bool)], layer: usize) {
        let mut keys = BTreeMap::new();
        for &(radius, avoid) in requests {
            *keys.entry(self.key(radius)).or_insert(false) |= avoid;
        }
        let missing: Vec<_> = keys
            .keys()
            .filter_map(|&key| {
                let start = self.radii.get(&key).map_or(0, Vec::len);
                (start <= layer).then_some((key, start))
            })
            .collect();
        let radii: Vec<_> = missing
            .par_iter()
            .map(|&(key, start)| {
                let areas = (start..=layer)
                    .into_par_iter()
                    .map(|i| {
                        let obstacles = if key <= (self.tip_radius * SCALE).round() as i64 {
                            &self.minimum_collision[i]
                        } else {
                            &self.collision[i]
                        };
                        offset_ex(obstacles, key as f64, Join::Round)
                    })
                    .collect::<Vec<_>>();
                (key, areas)
            })
            .collect();
        for (key, areas) in radii {
            self.radii.entry(key).or_default().extend(areas);
        }
        let tasks: Vec<_> = keys
            .into_iter()
            .filter(|(_, avoid)| *avoid)
            .flat_map(|(key, _)| [(key, false), (key, true)])
            .filter(|key| self.avoidance.get(key).map_or(0, Vec::len) <= layer)
            .collect();
        let avoidance: Vec<_> = tasks
            .par_iter()
            .map(|&(key, slow)| {
                let cached = self.avoidance.get(&(key, slow));
                let start = cached.map_or(0, Vec::len);
                let mut previous = cached.and_then(|v| v.last()).cloned().unwrap_or_default();
                let mut areas = Vec::with_capacity(layer + 1 - start);
                for i in start..=layer {
                    let movement = self.heights[i] * if slow { self.slow } else { self.fast };
                    previous = simplify(union_ex(
                        &[
                            offset_ex(&previous, -movement * SCALE, Join::Round),
                            self.radii[&key][i].clone(),
                        ]
                        .concat(),
                    ));
                    areas.push(previous.clone());
                }
                ((key, slow), areas)
            })
            .collect();
        for (key, areas) in avoidance {
            self.avoidance.entry(key).or_default().extend(areas);
        }
    }
    pub fn collision(&self, radius: f64, layer: usize) -> &Areas {
        &self.radii[&self.key(radius)][layer]
    }
    /// Bottom-up inaccessible volumes, prepared before parallel branch work.
    pub fn avoidance(&self, radius: f64, layer: usize, slow: bool) -> &Areas {
        &self.avoidance[&(self.key(radius), slow)][layer]
    }
}

/// Bound offset steps by the obstacle thickness so expanded areas cannot jump through walls.
pub(super) fn safe_offset(
    area: &[ExPolygon],
    distance: f64,
    collision: &[ExPolygon],
    safe_step: f64,
) -> Areas {
    let steps = (distance / safe_step.max(0.05)).ceil().max(1.0) as usize;
    let mut out = area.to_vec();
    for _ in 0..steps {
        out = diff_ex(
            &offset_ex(&out, distance / steps as f64 * SCALE, Join::Round),
            collision,
        );
    }
    simplify(out)
}
