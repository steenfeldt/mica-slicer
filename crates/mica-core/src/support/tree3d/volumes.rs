// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: prusaslicer@b028299c77 src/libslic3r/Support/TreeModelVolumes.hpp :: TreeModelVolumes, RadiusLayerPolygonCache, ceilRadius, getRadiusNextCeil
// Source: prusaslicer@b028299c77 src/libslic3r/Support/TreeModelVolumes.cpp :: TreeModelVolumes, precalculate, calculateCollision, calculateCollisionHolefree, calculateAvoidance, calculatePlaceables, calculateWallRestrictions, ceilRadius
// Source: orca@8500fcdcca src/libslic3r/Support/TreeModelVolumes.hpp :: TreeModelVolumes
// Source: orca@8500fcdcca src/libslic3r/Support/TreeModelVolumes.cpp :: TreeModelVolumes, calculateAvoidance
// Changes: Owned single-object slices as in active upstream constructor; deterministic BTreeMap caches; Clipper 6.4 offsets/intersections and polygons_simplify as upstream; lazy evaluation plus rayon-parallel precalculation.
// Changes: Precalculation computes each outline offset of the collisions once (neighbouring layers share them), the hole-free and placeable phases together and the wall restrictions beside the avoidance chains.
// Copyright (c) Prusa Research 2022 - 2023 Vojtěch Bubník @bubnikv, Pavel Mikuš @Godrak,
// Oleksandra Iushchenko @YuSanka
// PrusaSlicer is released under the terms of the AGPLv3 or higher.
// Tree supports by Thomas Rahm, losely based on Tree Supports by CuraEngine.
// Original source of Thomas Rahm's tree supports: https://github.com/ThomasRahm/CuraEngine
// Copyright (c) 2021 Ultimaker B.V.
// CuraEngine is released under the terms of the AGPLv3 or higher.

use super::{
    TreeSupportSettings,
    geometry::{diff64, intersection64, union64},
    polygons_simplify, scaled,
};
use crate::geometry::{ExPolygon, clip::Join, clip64};
use rayon::prelude::*;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, RwLock, RwLockReadGuard};

pub type Areas = Vec<ExPolygon>;
type Cache = Shared<(i64, usize)>;

/// A lazily filled cache shared by parallel queries (upstream guards its caches with mutexes
/// too). Every entry is a pure function of its key, so which thread computes it first, or
/// computing it twice, cannot change a value.
#[derive(Debug)]
struct Shared<K>(RwLock<BTreeMap<K, Arc<Areas>>>);
impl<K> Default for Shared<K> {
    fn default() -> Self {
        Self(RwLock::new(BTreeMap::new()))
    }
}
impl<K: Ord + Clone> Clone for Shared<K> {
    fn clone(&self) -> Self {
        Self(RwLock::new(self.read().clone()))
    }
}
impl<K: Ord> Shared<K> {
    fn new() -> Self {
        Self::default()
    }
    fn read(&self) -> RwLockReadGuard<'_, BTreeMap<K, Arc<Areas>>> {
        self.0.read().unwrap_or_else(|p| p.into_inner())
    }
    fn map(&mut self) -> &mut BTreeMap<K, Arc<Areas>> {
        self.0.get_mut().unwrap_or_else(|p| p.into_inner())
    }
    fn get(&self, key: &K) -> Option<Arc<Areas>> {
        self.read().get(key).cloned()
    }
    fn contains_key(&self, key: &K) -> bool {
        self.read().contains_key(key)
    }
    /// The entry for `key`, which must have been computed.
    fn at(&self, key: &K) -> Arc<Areas> {
        self.get(key)
            .expect("tree support cache entry computed first")
    }
    /// Keeps an entry another thread stored meanwhile (it is the same value).
    fn insert(&self, key: K, value: Areas) -> Arc<Areas> {
        let mut map = self.0.write().unwrap_or_else(|p| p.into_inner());
        map.entry(key).or_insert_with(|| Arc::new(value)).clone()
    }
    fn clear(&mut self) {
        self.map().clear();
    }
    fn extend(&mut self, values: impl IntoIterator<Item = (K, Areas)>) {
        let map = self.map();
        for (k, v) in values {
            map.entry(k).or_insert_with(|| Arc::new(v));
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum AvoidanceType {
    Slow,
    FastSafe,
    #[default]
    Fast,
}

pub const SUPPORT_TREE_EXPONENTIAL_FACTOR: f64 = 1.5;
pub const SUPPORT_TREE_EXPONENTIAL_THRESHOLD: i64 = 1_500_000;
pub const SUPPORT_TREE_COLLISION_RESOLUTION: i64 = 500_000;
pub const SUPPORT_TREE_AVOID_SUPPORT_BLOCKER: bool = true;

/// Cached forbidden regions. Like both pinned upstreams, this operates on ONE
/// object's outlines; their multi-object constructor branch is #if 0. Outline
/// and blocker indices are layer indices (raft padding is added to outlines).
///
/// Geometry: offsets are ClipperUtils' per-path `offset(Polygons, …)` on the Clipper 6.4
/// offsetter port (jtMiter limits, jtRound arc tolerance = resolution), booleans run with
/// Clipper 6.4 intersection rounding, and `polygons_simplify` is the Douglas-Peucker +
/// SimplifyPolygons port, as in both pins.
#[derive(Clone, Debug)]
pub struct TreeModelVolumes {
    pub settings: TreeSupportSettings,
    outlines: Vec<Areas>,
    anti_overhang: Vec<Areas>,
    /// Orca stores the bed polygon for later drawing. Both upstream collision
    /// implementations ignore the machine border (calculateMachineBorderCollision).
    pub bed_area: Areas,
    collision_cache: Cache,
    holefree_cache: Cache,
    avoidance_cache: Shared<(i64, usize, AvoidanceType, bool)>,
    placeable_cache: Cache,
    wall_cache: Shared<(i64, usize, bool)>,
    ignorable_radii: BTreeSet<i64>,
    pub precalculated: bool,
}
impl TreeModelVolumes {
    pub fn new(
        settings: TreeSupportSettings,
        object_outlines: Vec<Areas>,
        anti_overhang: Vec<Areas>,
        bed_area: Areas,
    ) -> Self {
        let mut outlines = vec![Vec::new(); settings.raft_layers.len()];
        outlines.par_extend(
            object_outlines
                .into_par_iter()
                .map(|p| polygons_simplify(&p, settings.resolution as f64)),
        );
        let bed_area = if settings.settings.rules.is_orca() {
            bed_area
        } else {
            Vec::new()
        };
        Self {
            settings,
            outlines,
            anti_overhang,
            // Orca-only foundation state; its later drawing stage clips to bed.
            bed_area,
            collision_cache: Cache::new(),
            holefree_cache: Cache::new(),
            avoidance_cache: Shared::new(),
            placeable_cache: Cache::new(),
            wall_cache: Shared::new(),
            ignorable_radii: BTreeSet::new(),
            precalculated: false,
        }
    }
    pub fn layer_count(&self) -> usize {
        self.outlines.len()
    }
    pub fn min_xy_dist_delta(&self) -> i64 {
        self.settings.xy_distance - self.settings.xy_min_distance
    }
    pub fn clear(&mut self) {
        self.clear_all_but_object_collision();
        self.collision_cache.clear();
        self.placeable_cache.clear();
    }
    pub fn clear_all_but_object_collision(&mut self) {
        // Upstream retains ALL collision buckets, not just radius zero.
        self.holefree_cache.clear();
        self.avoidance_cache.clear();
        self.wall_cache.clear();
        self.placeable_cache
            .map()
            .retain(|(radius, _), _| *radius == 0);
    }
    pub fn ceil_radius_raw(&self, radius: i64) -> i64 {
        assert!(radius >= 0);
        if radius == 0 {
            return 0;
        }
        let mut out = self.settings.min_radius;
        if radius > out {
            let initial_delta = SUPPORT_TREE_EXPONENTIAL_THRESHOLD - out;
            if initial_delta > SUPPORT_TREE_COLLISION_RESOLUTION {
                let steps = (initial_delta + SUPPORT_TREE_EXPONENTIAL_THRESHOLD - 1)
                    / SUPPORT_TREE_EXPONENTIAL_THRESHOLD;
                let step_size = initial_delta / steps;
                out += step_size;
                for _ in 0..steps {
                    if out >= radius && !self.ignorable_radii.contains(&out) {
                        return out;
                    }
                    out += step_size;
                }
            } else {
                out += SUPPORT_TREE_COLLISION_RESOLUTION;
            }
            while out < radius || self.ignorable_radii.contains(&out) {
                out = (out as f64 * SUPPORT_TREE_EXPONENTIAL_FACTOR) as i64;
            }
        }
        out
    }
    pub fn ceil_radius(&self, radius: i64, min_xy_dist: bool) -> i64 {
        assert!(radius >= 0);
        if min_xy_dist {
            self.ceil_radius_raw(radius)
        } else if radius > 0 {
            self.ceil_radius_raw(radius + self.min_xy_dist_delta())
        } else {
            self.min_xy_dist_delta()
        }
    }
    pub fn get_radius_next_ceil(&self, radius: i64, min_xy_dist: bool) -> i64 {
        assert!(radius > 0);
        if min_xy_dist {
            self.ceil_radius_raw(radius)
        } else {
            self.ceil_radius_raw(radius + self.min_xy_dist_delta()) - self.min_xy_dist_delta()
        }
    }
    /// Upstream uses sorted per-layer radius maps; lower-bound queries must not
    /// round the input, since Organic's final optimization asks for a true bound.
    pub fn get_collision_lower_bound_area(
        &self,
        layer: usize,
        max_radius: i64,
    ) -> Option<(i64, Arc<Areas>)> {
        self.collision_cache
            .read()
            .range(..=(max_radius, layer))
            .rev()
            .find(|((_, l), _)| *l == layer)
            .map(|((r, _), a)| (*r, a.clone()))
    }
    pub fn get_collision(&self, radius: i64, layer: usize, min_xy_dist: bool) -> Arc<Areas> {
        let radius = self.ceil_radius(radius, min_xy_dist);
        if let Some(hit) = self.collision_cache.get(&(radius, layer)) {
            return hit;
        }
        self.calculate_collision(radius, layer);
        self.collision_cache.at(&(radius, layer))
    }
    fn miter(&self, area: &[ExPolygon], delta: i64) -> Areas {
        if delta == 0 {
            union64(area)
        } else {
            clip64::offset_polygons_ex(area, delta as f64, Join::Miter, 1.2)
        }
    }
    fn round(&self, area: &[ExPolygon], delta: f64) -> Areas {
        clip64::offset_polygons_ex(area, delta, Join::Round, self.settings.resolution as f64)
    }
    fn simplify(&self, area: &[ExPolygon]) -> Areas {
        polygons_simplify(area, self.settings.resolution as f64)
    }
    /// The outline offsets one layer of calculateCollision unites, in upstream's order, as
    /// (outline layer, offset).
    fn collision_parts(&self, radius: i64, layer: usize) -> Vec<(usize, i64)> {
        let top = self.settings.z_distance_top_layers;
        let bottom = self.settings.z_distance_bottom_layers;
        let xy = self.settings.xy_min_distance;
        let mut parts: Vec<_> = (layer.saturating_sub(bottom)..=layer)
            .map(|j| (j, radius + xy))
            .collect();
        for i in 1..=top.min(self.outlines.len() - layer - 1) {
            // Upstream tapers XY clearance across the top Z gap; when the gap
            // is one layer it retains HALF the XY gap to avoid diagonal fusion.
            let range = (xy as f64
                - (i as f64 - if top == 1 { 0.5 } else { 0.0 }) * xy as f64 / top as f64)
                as i64;
            parts.push((layer + i, radius + range));
        }
        parts
    }
    /// One outline offset of calculateCollision.
    fn collision_part(&self, (layer, delta): (usize, i64)) -> Areas {
        self.miter(&self.outlines[layer], delta)
    }
    /// One layer of calculateCollision: (collision, radius-0 placeable when applicable).
    fn compute_collision(&self, radius: i64, layer: usize) -> (Areas, Option<Areas>) {
        self.compute_collision_from(radius, layer, |part| {
            std::borrow::Cow::Owned(self.collision_part(part))
        })
    }
    /// [`Self::compute_collision`] with the outline offsets from `part` (the same values,
    /// e.g. computed once for the neighbouring layers that share them).
    fn compute_collision_from<'a>(
        &self,
        radius: i64,
        layer: usize,
        part: impl Fn((usize, i64)) -> std::borrow::Cow<'a, Areas>,
    ) -> (Areas, Option<Areas>) {
        let bottom = self.settings.z_distance_bottom_layers;
        let xy = self.settings.xy_min_distance;
        let mut collisions = Vec::new();
        for key in self.collision_parts(radius, layer) {
            collisions.extend_from_slice(&part(key));
        }
        // Faithful active-source quirk (BOTH rules): processing_last_mesh is
        // outline_idx == layer_outline_indices.size(), hence always false.
        // Thus anti_overhang is NOT merged into collision here, nor is the
        // final simplify executed. additional_excluded_areas is also #if 0.
        let collisions = union64(&collisions);
        let placeable = (radius == 0 && self.settings.support_rests_on_model).then(|| {
            if layer > bottom {
                let below = layer - bottom - 1;
                let surface =
                    clip64::offset_polygons_ex(&self.outlines[below], xy as f64, Join::Miter, 3.0);
                let mut current = self.miter(&self.outlines[layer], xy);
                if let Some(blockers) = self.anti_overhang.get(below) {
                    current.extend_from_slice(blockers);
                }
                diff64(&surface, &union64(&current))
            } else {
                Vec::new()
            }
        });
        (collisions, placeable)
    }
    fn insert_collision(&self, radius: i64, layer: usize, value: (Areas, Option<Areas>)) {
        // Placeable first: a reader that finds the collision entry may expect it.
        if let Some(placeable) = value.1 {
            self.placeable_cache.insert((0, layer), placeable);
        }
        self.collision_cache.insert((radius, layer), value.0);
    }
    fn calculate_collision(&self, radius: i64, max_layer: usize) {
        assert!(max_layer < self.outlines.len());
        let missing: Vec<usize> = {
            let cache = self.collision_cache.read();
            (0..=max_layer)
                .filter(|&layer| !cache.contains_key(&(radius, layer)))
                .collect()
        };
        for layer in missing {
            if !self.collision_cache.contains_key(&(radius, layer)) {
                let value = self.compute_collision(radius, layer);
                self.insert_collision(radius, layer, value);
            }
        }
    }
    /// Input is an already bucketed radius (matching upstream's private API).
    pub fn get_collision_holefree(&self, radius: i64, layer: usize) -> Arc<Areas> {
        assert_eq!(radius, self.ceil_radius_raw(radius));
        assert!(radius < self.settings.increase_radius_until_radius + self.min_xy_dist_delta());
        if let Some(hit) = self.holefree_cache.get(&(radius, layer)) {
            return hit;
        }
        let increase = self.settings.increase_radius_until_radius;
        self.get_collision(increase, layer, false);
        let area = self.compute_holefree(radius, layer);
        self.holefree_cache.insert((radius, layer), area)
    }
    /// calculateCollisionHolefree for one layer; the source collision must be cached.
    fn compute_holefree(&self, radius: i64, layer: usize) -> Areas {
        let increase = self.settings.increase_radius_until_radius;
        let bucket = self.ceil_radius(increase, false);
        let delta = bucket - radius;
        let collision = self.collision_cache.at(&(bucket, layer));
        // The +5 scaled-unit bias is upstream, NOT 5 micrometres.
        self.simplify(&self.round(&union64(&collision), (5 - delta) as f64))
    }
    /// calculatePlaceables for radius > 0; the radius-0 entry must be cached.
    fn compute_placeable(&self, radius: i64, layer: usize) -> Areas {
        if layer == 0 {
            // Upstream diff(machine_border, collision) with an empty border.
            Vec::new()
        } else {
            let zero = self
                .placeable_cache
                .get(&(0, layer))
                .unwrap_or_else(|| self.collision_cache.at(&(0, layer)));
            self.miter(&union64(&zero), -(radius + self.settings.xy_distance))
        }
    }
    pub fn get_placeable_areas(&self, radius: i64, layer: usize) -> Arc<Areas> {
        let radius = self.ceil_radius_raw(radius);
        if let Some(hit) = self.placeable_cache.get(&(radius, layer)) {
            return hit;
        }
        if radius == 0 {
            self.calculate_collision(0, layer);
            // With buildplate-only upstream falls back to collision if zero
            // placeables don't exist. Avoidance-to-model isn't then requested.
            self.placeable_cache
                .get(&(0, layer))
                .unwrap_or_else(|| self.collision_cache.at(&(0, layer)))
        } else {
            if layer > 0 {
                self.get_placeable_areas(0, layer);
            }
            let area = self.compute_placeable(radius, layer);
            self.placeable_cache.insert((radius, layer), area)
        }
    }
    pub fn get_avoidance(
        &self,
        radius: i64,
        layer: usize,
        kind: AvoidanceType,
        to_model: bool,
        min_xy_dist: bool,
    ) -> Arc<Areas> {
        if layer == 0 {
            return self.get_collision(radius, layer, min_xy_dist);
        }
        let radius = self.ceil_radius(radius, min_xy_dist);
        let threshold = self.settings.increase_radius_until_radius + self.min_xy_dist_delta();
        let kind = if kind == AvoidanceType::FastSafe && radius >= threshold {
            AvoidanceType::Fast
        } else {
            kind
        };
        let key = (radius, layer, kind, to_model);
        if let Some(hit) = self.avoidance_cache.get(&key) {
            return hit;
        }
        self.calculate_avoidance(radius, layer, kind, to_model);
        self.avoidance_cache.at(&key)
    }
    /// Offset steps of calculateAvoidance: (move_steps, move_step, last_move_step).
    fn avoidance_steps(&self, radius: i64, kind: AvoidanceType) -> (usize, f32, f32) {
        // Upstream stores max_move and offset steps as float, not double.
        let movement = if kind == AvoidanceType::Slow {
            self.settings.maximum_move_distance_slow
        } else {
            self.settings.maximum_move_distance
        };
        let max_move = movement.saturating_sub(2).max(0) as f32;
        let mut step = (1.9 * radius.max(self.settings.xy_min_distance) as f64) as f32;
        if step < 1e-4 && self.settings.settings.rules.is_orca() {
            // Orca returns without a cache entry here. Its lazy getter would then
            // retry recursively forever; expose the invalid request explicitly.
            panic!("Orca avoidance requires a positive radius or minimum XY gap");
        }
        // Prusa's round_up_divide<int> converts both float arguments to int.
        // Its zero-step/zero-move debug assertion is undefined in release. For
        // zero movement, union with current collision is the continuous limit.
        assert!(
            step >= 1.0,
            "avoidance requires a positive radius or minimum XY gap"
        );
        // i64 avoids overflow near 90 degree angles (upstream's int conversion is UB there).
        let mut steps = ((max_move as i64 + step as i64 - 1) / step as i64).max(1) as usize;
        let mut last = max_move - (steps - 1) as f32 * step;
        if last < scaled(0.05) as f32 && steps > 1 {
            steps -= 1;
            step = max_move / steps as f32;
            last = step;
        }
        (steps, step, last)
    }
    /// One layer of the serial avoidance recurrence from the layer below.
    fn avoidance_layer(
        &self,
        previous: &[ExPolygon],
        collision: &Areas,
        placeable: Option<&Areas>,
        (steps, step, last): (usize, f32, f32),
    ) -> Areas {
        let mut previous = previous.to_vec();
        for i in 0..steps {
            let amount = if i + 1 == steps { last } else { step };
            previous =
                union64(&[collision.clone(), self.round(&previous, -f64::from(amount))].concat());
        }
        if let Some(placeable) = placeable {
            previous = diff64(&previous, placeable);
        }
        self.simplify(&previous)
    }
    fn calculate_avoidance(
        &self,
        radius: i64,
        max_layer: usize,
        kind: AvoidanceType,
        to_model: bool,
    ) {
        let threshold = self.settings.increase_radius_until_radius + self.min_xy_dist_delta();
        let holefree = kind != AvoidanceType::Fast && radius < threshold;
        let steps = self.avoidance_steps(radius, kind);
        let mut previous = self.get_collision(radius, 0, true);
        for layer in 1..=max_layer {
            let key = (radius, layer, kind, to_model);
            if let Some(cached) = self.avoidance_cache.get(&key) {
                previous = cached;
                continue;
            }
            let collision = if holefree {
                self.get_collision_holefree(radius, layer)
            } else {
                self.get_collision(radius, layer, true)
            };
            let placeable = to_model.then(|| self.get_placeable_areas(radius, layer));
            let next = self.avoidance_layer(&previous, &collision, placeable.as_deref(), steps);
            previous = self.avoidance_cache.insert(key, next);
        }
    }
    /// A complete avoidance chain from cached collision/holefree/placeable inputs.
    fn compute_avoidance_chain(
        &self,
        radius: i64,
        max_layer: usize,
        kind: AvoidanceType,
        to_model: bool,
    ) -> Vec<Areas> {
        let threshold = self.settings.increase_radius_until_radius + self.min_xy_dist_delta();
        let holefree = kind != AvoidanceType::Fast && radius < threshold;
        let steps = self.avoidance_steps(radius, kind);
        let mut previous = (*self.collision_cache.at(&(radius, 0))).clone();
        let mut out = Vec::with_capacity(max_layer);
        for layer in 1..=max_layer {
            let collision = if holefree {
                self.holefree_cache.at(&(radius, layer))
            } else {
                self.collision_cache.at(&(radius, layer))
            };
            let placeable = to_model.then(|| self.placeable_cache.at(&(radius, layer)));
            previous = self.avoidance_layer(&previous, &collision, placeable.as_deref(), steps);
            out.push(previous.clone());
        }
        out
    }
    pub fn get_wall_restriction(&self, radius: i64, layer: usize, min_xy_dist: bool) -> Arc<Areas> {
        if layer == 0 {
            return self.get_collision(radius, layer, min_xy_dist);
        }
        let radius = self.ceil_radius_raw(radius);
        let min_xy_dist = min_xy_dist && self.min_xy_dist_delta() > 0;
        let key = (radius, layer, min_xy_dist);
        if let Some(hit) = self.wall_cache.get(&key) {
            return hit;
        }
        self.get_collision(0, layer, min_xy_dist);
        self.get_collision(radius, layer - 1, true);
        let area = self.compute_wall_restriction(radius, layer, min_xy_dist);
        self.wall_cache.insert(key, area)
    }
    /// calculateWallRestrictions for one bucketed key; both collisions must be cached.
    fn compute_wall_restriction(&self, radius: i64, layer: usize, min_xy_dist: bool) -> Areas {
        let upper = self
            .collision_cache
            .at(&(self.ceil_radius(0, min_xy_dist), layer));
        let lower = self.collision_cache.at(&(radius, layer - 1));
        self.simplify(&intersection64(&upper, &lower))
    }
    pub fn precalculate(&mut self, max_layer: usize) {
        self.precalculate_with_cancel(max_layer, || Ok(()))
            .expect("infallible cancel callback");
    }
    /// Deterministic equivalent of upstream's parallel radius jobs. Cache keys and
    /// polygon construction order never depend on hash iteration or task scheduling.
    pub fn precalculate_with_cancel(
        &mut self,
        max_layer: usize,
        mut cancel: impl FnMut() -> Result<(), String>,
    ) -> Result<(), String> {
        assert!(max_layer < self.outlines.len());
        // Recompute from an unfiltered bucket grid if called after lazy queries.
        self.ignorable_radii.clear();
        let delta = self.min_xy_dist_delta();
        let mut tips = BTreeSet::new();
        for dtt in 0..=self.settings.tip_layers {
            let r = self.settings.get_radius(dtt, 0.0);
            tips.insert(self.ceil_radius_raw(r));
            tips.insert(self.ceil_radius_raw(r + delta));
        }
        let mut radius = self.settings.min_radius;
        while radius <= self.settings.branch_radius {
            if !tips.contains(&radius) {
                self.ignorable_radii.insert(radius);
            }
            radius = self.ceil_radius_raw(radius + 1);
        }
        cancel()?;
        let mut avoidance = BTreeMap::<i64, usize>::new();
        for dtt in 0..=max_layer {
            let layer = max_layer - dtt;
            for r in [
                self.settings.get_radius(dtt, 0.0) + delta,
                self.settings.get_radius(dtt, 0.0),
                self.settings.recommended_min_radius(layer as i32) + delta,
            ] {
                avoidance.entry(self.ceil_radius_raw(r)).or_insert(layer);
            }
        }
        cancel()?;
        let mut collision = avoidance.clone();
        collision.insert(
            self.ceil_radius(self.settings.increase_radius_until_radius, false),
            max_layer,
        );
        collision.insert(0, max_layer);
        if delta != 0 {
            collision.insert(delta, max_layer);
        }
        // Upstream runs these jobs on TBB. Every entry is a pure function of the slices
        // and earlier stages, so parallel evaluation yields the same caches.
        let jobs: Vec<(i64, usize)> = collision
            .iter()
            .flat_map(|(&r, &max)| (0..=max).map(move |l| (r, l)))
            .filter(|key| !self.collision_cache.contains_key(key))
            .collect();
        // Neighbouring layers share outline offsets (a layer's offset for its own collision
        // is also part of the collisions of the layers it lies below): each is computed once.
        let parts: BTreeSet<(usize, i64)> = jobs
            .iter()
            .flat_map(|&(r, l)| self.collision_parts(r, l))
            .collect();
        let parts: Vec<_> = parts.into_iter().collect();
        let part_values: Vec<Areas> = parts
            .par_iter()
            .map(|&part| self.collision_part(part))
            .collect();
        let part_values: BTreeMap<(usize, i64), Areas> =
            parts.into_iter().zip(part_values).collect();
        let values: Vec<_> = jobs
            .par_iter()
            .map(|&(r, l)| {
                self.compute_collision_from(r, l, |part| {
                    std::borrow::Cow::Borrowed(&part_values[&part])
                })
            })
            .collect();
        drop(part_values);
        for (&(r, l), value) in jobs.iter().zip(values) {
            self.insert_collision(r, l, value);
        }
        cancel()?;
        // Hole-free collisions and placeable areas only read the collisions: one phase.
        let threshold = self.settings.increase_radius_until_radius + delta;
        let holefree_jobs: Vec<(i64, usize)> = avoidance
            .iter()
            .filter(|(r, _)| **r < threshold)
            .flat_map(|(&r, &max)| (0..=max).map(move |l| (r, l)))
            .filter(|key| !self.holefree_cache.contains_key(key))
            .collect();
        let placeable_jobs: Vec<(i64, usize)> = if self.settings.support_rests_on_model {
            avoidance
                .iter()
                .filter(|(r, _)| **r > 0)
                .flat_map(|(&r, &max)| (0..=max).map(move |l| (r, l)))
                .filter(|key| !self.placeable_cache.contains_key(key))
                .collect()
        } else {
            Vec::new()
        };
        let (holefree_values, placeable_values): (Vec<_>, Vec<_>) = rayon::join(
            || {
                holefree_jobs
                    .par_iter()
                    .map(|&(r, l)| self.compute_holefree(r, l))
                    .collect()
            },
            || {
                placeable_jobs
                    .par_iter()
                    .map(|&(r, l)| self.compute_placeable(r, l))
                    .collect()
            },
        );
        self.holefree_cache
            .extend(holefree_jobs.into_iter().zip(holefree_values));
        self.placeable_cache
            .extend(placeable_jobs.into_iter().zip(placeable_values));
        cancel()?;
        let mut chains = BTreeSet::new();
        for (&radius, &layer) in &avoidance {
            if layer == 0 {
                continue;
            }
            for kind in [
                AvoidanceType::Slow,
                AvoidanceType::FastSafe,
                AvoidanceType::Fast,
            ] {
                let kind = if kind == AvoidanceType::FastSafe && radius >= threshold {
                    AvoidanceType::Fast
                } else {
                    kind
                };
                for to_model in [false, true] {
                    if (!to_model || self.settings.support_rests_on_model)
                        && !self
                            .avoidance_cache
                            .contains_key(&(radius, layer, kind, to_model))
                    {
                        chains.insert((radius, layer, kind, to_model));
                    }
                }
            }
        }
        let chains: Vec<_> = chains.into_iter().collect();
        let min_variants: &[bool] = if delta > 0 { &[false, true] } else { &[false] };
        let wall_jobs: Vec<(i64, usize, bool)> = avoidance
            .iter()
            .flat_map(|(&r, &max)| {
                (1..=max).flat_map(move |l| min_variants.iter().map(move |&m| (r, l, m)))
            })
            .filter(|key| !self.wall_cache.contains_key(key))
            .collect();
        // The wall restrictions only read collisions: they run while the avoidance chains
        // (serial over the layers) do.
        let (values, wall_values): (Vec<_>, Vec<_>) = rayon::join(
            || {
                chains
                    .par_iter()
                    .map(|&(r, max, kind, to_model)| {
                        self.compute_avoidance_chain(r, max, kind, to_model)
                    })
                    .collect()
            },
            || {
                wall_jobs
                    .par_iter()
                    .map(|&(r, l, m)| self.compute_wall_restriction(r, l, m))
                    .collect()
            },
        );
        for (&(r, _, kind, to_model), chain) in chains.iter().zip(values) {
            self.avoidance_cache.extend(
                chain
                    .into_iter()
                    .enumerate()
                    .map(|(i, area)| ((r, i + 1, kind, to_model), area)),
            );
        }
        self.wall_cache
            .extend(wall_jobs.into_iter().zip(wall_values));
        cancel()?;
        self.precalculated = true;
        Ok(())
    }
}
