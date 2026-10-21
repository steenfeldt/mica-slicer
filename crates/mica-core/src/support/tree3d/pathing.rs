// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: prusaslicer@b028299c77 src/libslic3r/Support/TreeSupport.cpp :: increase_single_area, increase_areas_one_layer, merge_support_element_states, merge_influence_areas, create_layer_pathing, create_nodes_from_area
// Source: orca@8500fcdcca src/libslic3r/Support/TreeSupport3D.cpp :: increase_single_area, increase_areas_one_layer, merge_support_element_states, merge_influence_areas, create_layer_pathing, create_nodes_from_area
// Source: prusaslicer@b028299c77 src/libslic3r/AABBTreeIndirect.hpp :: build_recursive, partition_input
// Changes: Rust owned geometry, deterministic scheduling (parallel per parent and per merge round, fixed bucket schedule) and Clipper2 substitutions.
// Copyright (c) Prusa Research 2022 - 2023 Vojtěch Bubník @bubnikv, Lukáš Matěna @lukasmatena, Tomáš Mészáros @tamasmeszaros
// Copyright (c) Prusa Research 2020 - 2023 Vojtěch Bubník @bubnikv, Pavel Mikuš @Godrak, Tomáš Mészáros @tamasmeszaros, Lukáš Matěna @lukasmatena, Lukáš Hejl @hejllukas
// AABB tree balancing adapted upstream from libigl AABB.{cpp,hpp}, Copyright (C) 2015 Alec Jacobson <alecjacobson@gmail.com>.
// PrusaSlicer is released under the terms of the AGPLv3 or higher.
// Tree supports by Thomas Rahm, losely based on Tree Supports by CuraEngine.
// Original source of Thomas Rahm's tree supports: https://github.com/ThomasRahm/CuraEngine
// Copyright (c) 2021 Ultimaker B.V.
// CuraEngine is released under the terms of the AGPLv3 or higher.

use super::{geometry::*, *};
use crate::geometry::{BoundingBox, Point};
use rayon::prelude::*;

#[derive(Clone, Debug, Default)]
pub struct SupportElementInfluenceAreas {
    pub influence_areas: Areas,
    pub to_bp_areas: Areas,
    pub to_model_areas: Areas,
}
#[derive(Clone, Debug, Default)]
pub struct SupportElementMerging {
    pub state: SupportElementState,
    pub parents: Vec<i32>,
    pub areas: SupportElementInfluenceAreas,
}
impl SupportElementMerging {
    fn bbox(&self) -> BoundingBox {
        let a = &self.areas;
        bbox(&a.influence_areas)
            .into_iter()
            .chain(bbox(&a.to_bp_areas))
            .chain(bbox(&a.to_model_areas))
            .reduce(BoundingBox::merge)
            .expect("nonempty merging element")
    }
    fn into_element(self) -> SupportElement {
        SupportElement {
            state: self.state,
            parents: self.parents,
            influence_area: self.areas.influence_areas,
        }
    }
}
fn safe_movement(c: &TreeSupportSettings, min: bool) -> i64 {
    let step = if min {
        c.xy_min_distance
    } else {
        c.xy_distance
    } + if c.z_distance_top_layers.min(c.z_distance_bottom_layers) > 0 {
        c.min_feature_size
    } else {
        0
    };
    if c.settings.rules.is_orca() && step == 0 {
        scaled(0.1)
    } else {
        step
    }
}
fn reachable(
    v: &TreeModelVolumes,
    c: &TreeSupportSettings,
    e: &SupportElementState,
    s: AreaIncreaseSettings,
    increased: &[crate::geometry::ExPolygon],
    radius: i64,
    layer: usize,
) -> Areas {
    let forbidden = if e.to_buildplate || e.to_model_gracious {
        v.get_avoidance(
            radius,
            layer,
            s.avoidance_type,
            !e.to_buildplate,
            s.use_min_distance,
        )
    } else if c.support_rests_on_model {
        v.get_collision(radius, layer, s.use_min_distance)
    } else {
        return vec![];
    };
    diff64(increased, &forbidden)
}

/// One attempt from the ordered movement/avoidance/radius list. Carries both
/// restricted destination regions AND the unrestricted reachable region.
#[allow(clippy::too_many_arguments)]
pub fn increase_single_area(
    v: &TreeModelVolumes,
    c: &TreeSupportSettings,
    s: AreaIncreaseSettings,
    layer: usize,
    parent: &SupportElement,
    relevant_offset: &[crate::geometry::ExPolygon],
    overspeed: i64,
    merge_layer: bool,
) -> Option<SupportElementMerging> {
    assert!(layer > 0);
    let mut e = parent.state.propagate_down(c.settings.rules);
    if s.increase_radius {
        e.effective_radius_height += 1;
    }
    let mut radius = c.get_collision_radius(&e);
    let mut increased = if s.move_area {
        relevant_offset.to_vec()
    } else {
        parent.influence_area.clone()
    };
    // Both pins call polygons_simplify(increased, 0.025 mm) after this but discard its
    // return value, so `increased` is NOT simplified upstream.
    if s.move_area && overspeed > 0 {
        let safe = safe_movement(c, e.use_min_xy_dist);
        increased = safe_offset_inc(
            &increased,
            overspeed,
            &v.get_wall_restriction(
                parent.collision_radius(c),
                layer,
                parent.state.use_min_xy_dist,
            ),
            safe,
            safe + radius,
            1,
        );
    }
    let mut bp = vec![];
    let mut model = vec![];
    if merge_layer || e.to_buildplate {
        bp = safe_union(
            &diff64(
                &increased,
                &v.get_avoidance(
                    radius,
                    layer - 1,
                    s.avoidance_type,
                    false,
                    s.use_min_distance,
                ),
            ),
            &[],
        );
        if !e.to_buildplate && area(&bp) > tiny_area_threshold() {
            e.to_buildplate = true;
        }
    }
    if c.support_rests_on_model {
        if merge_layer || e.to_model_gracious {
            model = safe_union(
                &diff64(
                    &increased,
                    &v.get_avoidance(
                        radius,
                        layer - 1,
                        s.avoidance_type,
                        true,
                        s.use_min_distance,
                    ),
                ),
                &[],
            );
        }
        if !e.to_model_gracious {
            if merge_layer && area(&model) >= tiny_area_threshold() {
                e.to_model_gracious = true;
            } else {
                model = safe_union(
                    &diff64(
                        &increased,
                        &v.get_collision(radius, layer - 1, s.use_min_distance),
                    ),
                    &[],
                );
            }
        }
    }
    let mut check = if e.to_buildplate { &bp } else { &model };
    if s.increase_radius && area(check) > tiny_area_threshold() {
        let before = v.ceil_radius(radius, s.use_min_distance);
        if radius < c.increase_radius_until_radius && radius < c.get_radius_state(&e) {
            let target = c.get_radius_state(&e).min(c.increase_radius_until_radius);
            let mut current = v.get_radius_next_ceil(radius, s.use_min_distance);
            while current < target {
                let next = v.get_radius_next_ceil(current + 1, s.use_min_distance);
                if v.ceil_radius(next, s.use_min_distance)
                    > v.ceil_radius(radius, s.use_min_distance)
                    && area(&reachable(v, c, &e, s, &increased, next, layer - 1))
                        <= tiny_area_threshold()
                {
                    break;
                }
                current = next;
            }
            while e.effective_radius_height + 1 < e.distance_to_top
                && c.get_radius(
                    e.effective_radius_height as usize + 1,
                    e.elephant_foot_increases,
                ) <= current
                && c.get_radius(
                    e.effective_radius_height as usize + 1,
                    e.elephant_foot_increases,
                ) <= c.get_radius_state(&e)
            {
                e.effective_radius_height += 1;
            }
        }
        radius = c.get_collision_radius(&e);
        // coord_t conversion in upstream intentionally truncates the foot delta.
        let foot =
            (c.bp_radius_increase_per_layer - c.branch_radius_increase_per_layer).max(0.) as i64;
        let planned = ((c.recommended_min_radius(layer as i32 - 1) - c.get_radius_state(&e))
            as f64
            / foot as f64)
            .min(1.);
        if planned > 0.
            && e.to_buildplate
            && c.get_radius_state(&e) >= c.branch_radius
            && c.get_radius_state(&e) >= c.increase_radius_until_radius
        {
            let next = c.get_radius(
                e.effective_radius_height as usize,
                e.elephant_foot_increases + planned,
            );
            if v.ceil_radius(next, s.use_min_distance) <= v.ceil_radius(radius, s.use_min_distance)
                || area(&reachable(v, c, &e, s, &increased, next, layer - 1))
                    > tiny_area_threshold()
            {
                e.elephant_foot_increases += planned;
                radius = c.get_collision_radius(&e);
            }
        }
        if before != v.ceil_radius(radius, s.use_min_distance) {
            if e.to_buildplate {
                bp = safe_union(
                    &diff64(
                        &increased,
                        &v.get_avoidance(
                            radius,
                            layer - 1,
                            s.avoidance_type,
                            false,
                            s.use_min_distance,
                        ),
                    ),
                    &[],
                );
            }
            if c.support_rests_on_model && (!e.to_buildplate || merge_layer) {
                let forbidden = if e.to_model_gracious {
                    v.get_avoidance(
                        radius,
                        layer - 1,
                        s.avoidance_type,
                        true,
                        s.use_min_distance,
                    )
                } else {
                    v.get_collision(radius, layer - 1, s.use_min_distance)
                };
                model = safe_union(&diff64(&increased, &forbidden), &[]);
            }
            check = if e.to_buildplate { &bp } else { &model };
        }
    }
    (area(check) > tiny_area_threshold()).then_some(SupportElementMerging {
        state: e,
        parents: vec![],
        areas: SupportElementInfluenceAreas {
            influence_areas: increased,
            to_bp_areas: bp,
            to_model_areas: model,
        },
    })
}

fn attempt(
    kind: AvoidanceType,
    speed: i64,
    grow: bool,
    min: bool,
    movement: bool,
) -> AreaIncreaseSettings {
    AreaIncreaseSettings {
        increase_speed: speed,
        avoidance_type: kind,
        increase_radius: grow,
        no_error: true,
        use_min_distance: min,
        move_area: movement,
    }
}
/// Upstream's ordered list, including reuse of last_area_increase, hole escape,
/// XY-priority fallback, emergency overspeed, and locked tips.
pub fn area_increase_order(
    v: &TreeModelVolumes,
    c: &TreeSupportSettings,
    parent: &SupportElement,
    layer: usize,
    merge_layer: bool,
    slow: i64,
    fast: i64,
) -> Vec<AreaIncreaseSettings> {
    use AvoidanceType::{Fast, FastSafe, Slow};
    let e = parent.state.propagate_down(c.settings.rules);
    let mut order = vec![];
    let mut push = |s| {
        if !order.contains(&s) {
            order.push(s);
        }
    };
    let last = e.last_area_increase;
    let parent_slow = last.increase_speed < c.maximum_move_distance;
    let mismatch = parent_slow && last.avoidance_type != Slow;
    if last.move_area
        && last.no_error
        && e.can_use_safe_radius
        && !merge_layer
        && !mismatch
        && (e.distance_to_top as usize >= c.tip_layers || parent_slow)
    {
        for grow in [true, false] {
            push(attempt(
                last.avoidance_type,
                if parent_slow { slow } else { fast },
                grow,
                false,
                last.move_area,
            ));
        }
    }
    if !e.can_use_safe_radius {
        push(attempt(Slow, slow, true, false, false));
        if (e.distance_to_top as usize) < c.tip_layers.div_ceil(2) {
            push(attempt(Fast, slow, true, false, false));
        }
        push(attempt(FastSafe, fast, true, false, false));
        push(attempt(FastSafe, fast, false, false, true));
        push(attempt(Fast, fast, false, false, true));
    } else {
        push(attempt(Slow, slow, true, false, true));
        push(attempt(Slow, slow, false, false, true));
        if (e.distance_to_top as usize) < c.tip_layers {
            push(attempt(FastSafe, slow, true, false, true));
        }
        push(attempt(FastSafe, fast, true, false, true));
        push(attempt(FastSafe, fast, false, false, true));
    }
    if e.use_min_xy_dist {
        order = order
            .into_iter()
            .flat_map(|s| {
                [
                    s,
                    AreaIncreaseSettings {
                        use_min_distance: true,
                        ..s
                    },
                ]
            })
            .collect();
    }
    if e.to_buildplate
        || (e.to_model_gracious
            && intersection64(
                &parent.influence_area,
                &v.get_placeable_areas(c.get_collision_radius(&e), layer),
            )
            .is_empty())
    {
        let s = AreaIncreaseSettings {
            no_error: false,
            ..attempt(Fast, fast, false, e.use_min_xy_dist, true)
        };
        if !order.contains(&s) {
            order.push(s);
        }
    }
    if e.locked() && e.can_use_safe_radius {
        let s = attempt(Slow, 0, true, false, false);
        if !order.contains(&s) {
            order.insert(0, s);
        }
    }
    order
}

pub fn increase_areas_one_layer(
    v: &TreeModelVolumes,
    c: &TreeSupportSettings,
    layer: usize,
    parents: &mut SupportElements,
    merge_layer: bool,
) -> Vec<SupportElementMerging> {
    increase_areas_one_layer_with_cancel(v, c, layer, parents, merge_layer, || {
        Ok::<(), std::convert::Infallible>(())
    })
    .unwrap()
}

/// One parent of [`increase_areas_one_layer_with_cancel`]: the first successful attempt, or
/// `None` after resetting the parent's placement.
fn increase_parent(
    v: &TreeModelVolumes,
    c: &TreeSupportSettings,
    layer: usize,
    idx: usize,
    parent: &mut SupportElement,
    merge_layer: bool,
) -> Option<SupportElementMerging> {
    let e = parent.state.propagate_down(c.settings.rules);
    let radius = c.get_collision_radius(&e);
    let wall = v
        .get_wall_restriction(
            parent.collision_radius(c),
            layer,
            parent.state.use_min_xy_dist,
        )
        .clone();
    let mut extra = 5;
    let mut extra_slow = 0;
    let ceiled = v.ceil_radius(parent.collision_radius(c), parent.state.use_min_xy_dist);
    let projected = c.get_radius(
        parent.state.effective_radius_height as usize + 1,
        parent.state.elephant_foot_increases,
    );
    let delta = projected - parent.collision_radius(c);
    let safe = safe_movement(c, e.use_min_xy_dist);
    assert!(
        safe > 0,
        "Prusa pathing requires positive XY clearance; Orca supplies a zero-clearance fallback"
    );
    if ceiled == v.ceil_radius(projected, parent.state.use_min_xy_dist)
        || projected < c.increase_radius_until_radius
    {
        extra += delta;
    } else {
        extra_slow +=
            delta.min(c.maximum_move_distance + extra - c.maximum_move_distance_slow - extra_slow);
    }
    if c.layer_start_bp_radius > layer as i32
        && c.recommended_min_radius(layer as i32 - 1)
            < c.get_radius(
                e.effective_radius_height as usize + 1,
                e.elephant_foot_increases,
            )
    {
        if ceiled
            == v.ceil_radius(
                c.get_radius(
                    parent.state.effective_radius_height as usize + 1,
                    parent.state.elephant_foot_increases + 1.,
                ),
                parent.state.use_min_xy_dist,
            )
        {
            extra += c.bp_radius_increase_per_layer as i64;
        } else {
            extra_slow += (c.bp_radius_increase_per_layer as i64)
                .min(c.maximum_move_distance - c.maximum_move_distance_slow - extra_slow);
        }
    }
    let fast = c.maximum_move_distance + extra;
    let slow = c.maximum_move_distance_slow + extra + extra_slow;
    let independent = radius / safe - i64::from(fast < radius + safe) > (slow + safe - 1) / safe;
    let order = area_increase_order(v, c, parent, layer, merge_layer, slow, fast);
    let mut offset_slow = vec![];
    let mut offset_fast = vec![];
    for s in order {
        if s.move_area {
            if offset_slow.is_empty() && (s.increase_speed == slow || !independent) {
                offset_slow = safe_offset_inc(
                    &parent.influence_area,
                    slow,
                    &wall,
                    safe,
                    if independent { safe + radius } else { 0 },
                    2,
                );
            }
            if offset_fast.is_empty() && s.increase_speed != slow {
                offset_fast = if independent {
                    safe_offset_inc(&parent.influence_area, fast, &wall, safe, safe + radius, 1)
                } else {
                    safe_offset_inc(
                        &offset_slow,
                        c.maximum_move_distance - c.maximum_move_distance_slow - extra_slow,
                        &wall,
                        safe,
                        safe + radius,
                        1,
                    )
                };
            }
        }
        let error_area;
        let relevant = if !s.no_error {
            error_area = safe_union(
                &parent.influence_area,
                &offset_lines_miter(&outlines(&parent.influence_area), scaled(0.005) as f64),
            );
            &error_area
        } else if s.increase_speed == slow {
            &offset_slow
        } else {
            &offset_fast
        };
        if let Some(mut result) = increase_single_area(
            v,
            c,
            s,
            layer,
            parent,
            relevant,
            if s.no_error {
                0
            } else {
                (fast as f64 * 1.5) as i64
            },
            merge_layer,
        ) {
            let e = &mut result.state;
            e.last_area_increase = s;
            let bypass =
                !s.move_area || (s.use_min_distance && (e.distance_to_top as usize) < c.tip_layers);
            if s.move_area {
                e.dont_move_until = 0;
            } else {
                e.result_on_layer = parent.state.result_on_layer;
            }
            e.can_use_safe_radius = s.avoidance_type != AvoidanceType::Fast;
            if !s.use_min_distance {
                e.use_min_xy_dist = false;
            }
            let a = &mut result.areas;
            a.influence_areas = safe_union(
                &diff64(
                    &a.influence_areas,
                    &v.get_collision(c.get_collision_radius(e), layer - 1, e.use_min_xy_dist),
                ),
                &safe_union(&a.to_bp_areas, &a.to_model_areas),
            );
            if bypass || !e.to_buildplate {
                a.to_bp_areas.clear();
            }
            if bypass || !c.support_rests_on_model {
                a.to_model_areas.clear();
            }
            result.parents = vec![idx as i32];
            return Some(result);
        }
    }
    {
        parent.state.result_on_layer_reset();
        parent.state.to_model_gracious = false;
    }
    None
}

/// Fallible cancellation is checked between upstream work units.
pub fn increase_areas_one_layer_with_cancel<E>(
    v: &TreeModelVolumes,
    c: &TreeSupportSettings,
    layer: usize,
    parents: &mut SupportElements,
    merge_layer: bool,
    mut cancel: impl FnMut() -> Result<(), E>,
) -> Result<Vec<SupportElementMerging>, E> {
    // Upstream runs this loop as a tbb::parallel_for over the parents: each result depends
    // only on its parent and the volume caches (pure per key). Collected in parent order.
    cancel()?;
    let output: Vec<SupportElementMerging> = parents
        .par_iter_mut()
        .enumerate()
        .filter_map(|(idx, parent)| increase_parent(v, c, layer, idx, parent, merge_layer))
        .collect();
    Ok(output)
}

pub fn merge_support_element_states(
    c: &TreeSupportSettings,
    first: &SupportElementState,
    second: &SupportElementState,
    next: Point,
    layer: i32,
) -> SupportElementState {
    let target = if first.target_height > second.target_height {
        first
    } else {
        second
    };
    let mut out = SupportElementState {
        next_position: next,
        layer_idx: layer,
        use_min_xy_dist: first.use_min_xy_dist || second.use_min_xy_dist,
        supports_roof: first.supports_roof || second.supports_roof,
        dont_move_until: first.dont_move_until.max(second.dont_move_until),
        can_use_safe_radius: first.can_use_safe_radius || second.can_use_safe_radius,
        missing_roof_layers: first.missing_roof_layers.min(second.missing_roof_layers),
        target_height: target.target_height,
        target_position: target.target_position,
        effective_radius_height: first
            .effective_radius_height
            .max(second.effective_radius_height),
        distance_to_top: first.distance_to_top.max(second.distance_to_top),
        to_buildplate: first.to_buildplate && second.to_buildplate,
        to_model_gracious: first.to_model_gracious && second.to_model_gracious,
        last_area_increase: AreaIncreaseSettings {
            avoidance_type: first
                .last_area_increase
                .avoidance_type
                .min(second.last_area_increase.avoidance_type),
            increase_speed: first
                .last_area_increase
                .increase_speed
                .min(second.last_area_increase.increase_speed),
            increase_radius: first.last_area_increase.increase_radius
                || second.last_area_increase.increase_radius,
            no_error: first.last_area_increase.no_error || second.last_area_increase.no_error,
            use_min_distance: first.last_area_increase.use_min_distance
                && second.last_area_increase.use_min_distance,
            move_area: first.last_area_increase.move_area || second.last_area_increase.move_area,
        },
        ..Default::default()
    };
    if c.settings.rules.is_orca() {
        out.set_pending_roof_recovery(
            first.missing_roof_layers.max(second.missing_roof_layers),
            first
                .orca
                .roof_recovery_dtt
                .max(second.orca.roof_recovery_dtt),
            c.settings.rules,
        );
    }
    if c.bp_radius_increase_per_layer > 0. {
        let foot = (c
            .get_collision_radius(first)
            .max(c.get_collision_radius(second))
            - c.get_collision_radius(&out))
        .abs();
        out.elephant_foot_increases =
            foot as f64 / (c.bp_radius_increase_per_layer - c.branch_radius_increase_per_layer);
    }
    out
}

pub fn merge_influence_areas_two_elements(
    v: &TreeModelVolumes,
    c: &TreeSupportSettings,
    layer: usize,
    dst: &mut SupportElementMerging,
    src: &mut SupportElementMerging,
) -> bool {
    if dst.state.to_model_gracious != src.state.to_model_gracious
        || dst.state.use_min_xy_dist != src.state.use_min_xy_dist
    {
        return false;
    }
    let (small, big) = if c.get_collision_radius(&dst.state) > c.get_collision_radius(&src.state) {
        (&*src, &*dst)
    } else {
        (&*dst, &*src)
    };
    let delta = (c.get_radius_state(&big.state) - c.get_radius_state(&small.state)).abs();
    let sb = small.bbox();
    let bb = big.bbox();
    if sb.min.x - delta - EPS > bb.max.x + EPS
        || sb.max.x + delta + EPS < bb.min.x - EPS
        || sb.min.y - delta - EPS > bb.max.y + EPS
        || sb.max.y + delta + EPS < bb.min.y - EPS
    {
        return false;
    }
    let to_bp = dst.state.to_buildplate && src.state.to_buildplate;
    let mut model_increase = 0;
    if !to_bp {
        if dst.state.to_buildplate != src.state.to_buildplate {
            let rd = c.get_radius_state(&dst.state);
            let rs = c.get_radius_state(&src.state);
            if dst.state.to_buildplate && rs < rd {
                model_increase = src.state.increased_to_model_radius + rd - rs;
            } else if !dst.state.to_buildplate && rs > rd {
                model_increase = dst.state.increased_to_model_radius + rs - rd;
            }
            if model_increase > c.max_to_model_radius_increase {
                return false;
            }
        }
        if !dst.state.supports_roof
            && !src.state.supports_roof
            && (dst.state.distance_to_top.max(src.state.distance_to_top) as usize)
                < c.min_dtt_to_model
        {
            return false;
        }
    }
    if !big.state.can_use_safe_radius && small.state.can_use_safe_radius {
        return false;
    }
    let radius = c.get_collision_radius(&small.state);
    let collision = v.get_collision(
        radius,
        layer - 1,
        big.state.use_min_xy_dist && small.state.use_min_xy_dist,
    );
    let intersect_areas = |a: &[crate::geometry::ExPolygon], b: &[crate::geometry::ExPolygon]| {
        intersection64(
            &safe_offset_inc(a, delta, &collision, 2 * (c.xy_distance + radius - 3), 0, 0),
            b,
        )
    };
    let intersect = intersect_areas(
        if to_bp {
            &small.areas.to_bp_areas
        } else {
            &small.areas.to_model_areas
        },
        if to_bp {
            &big.areas.to_bp_areas
        } else {
            &big.areas.to_model_areas
        },
    );
    if area(&intersect) <= tiny_area_threshold()
        || area(&miter(&intersect, -scaled(0.025) as f64)) <= tiny_area_threshold()
    {
        return false;
    }
    let next = move_inside_if_outside(&intersect, dst.state.next_position);
    let mut state = merge_support_element_states(c, &dst.state, &src.state, next, layer as i32 - 1);
    state.increased_to_model_radius = if model_increase == 0 {
        dst.state
            .increased_to_model_radius
            .max(src.state.increased_to_model_radius)
    } else {
        model_increase
    };
    let influence = safe_union(
        &intersect_areas(&small.areas.influence_areas, &big.areas.influence_areas),
        &intersect,
    );
    let model = if to_bp && c.support_rests_on_model {
        if state.to_model_gracious {
            safe_union(
                &intersect_areas(&small.areas.to_model_areas, &big.areas.to_model_areas),
                &intersect,
            )
        } else {
            influence.clone()
        }
    } else if !to_bp {
        intersect.clone()
    } else {
        vec![]
    };
    dst.parents.append(&mut src.parents);
    dst.state = state;
    dst.areas = SupportElementInfluenceAreas {
        influence_areas: influence,
        to_bp_areas: if to_bp { intersect } else { vec![] },
        to_model_areas: model,
    };
    src.areas = SupportElementInfluenceAreas::default();
    true
}

pub fn merge_influence_areas_leaves(
    v: &TreeModelVolumes,
    c: &TreeSupportSettings,
    layer: usize,
    a: &mut Vec<SupportElementMerging>,
) {
    let mut i = 0;
    while i + 1 < a.len() {
        let mut j = i + 1;
        let mut merged = false;
        while j < a.len() {
            let (left, right) = a.split_at_mut(j);
            if merge_influence_areas_two_elements(v, c, layer, &mut left[i], &mut right[0]) {
                a.swap_remove(j);
                merged = true;
                break;
            }
            j += 1;
        }
        if !merged {
            i += 1;
        }
    }
}
pub fn merge_influence_areas_two_sets(
    v: &TreeModelVolumes,
    c: &TreeSupportSettings,
    layer: usize,
    dst: &mut Vec<SupportElementMerging>,
    mut src: Vec<SupportElementMerging>,
) {
    // Match upstream's front-compaction of consumed source entries.
    let mut begin = 0;
    for si in 0..src.len() {
        let mut merged = None;
        for (di, d) in dst.iter_mut().enumerate() {
            if merge_influence_areas_two_elements(v, c, layer, d, &mut src[si]) {
                merged = Some(di);
                src.swap(si, begin);
                begin += 1;
                break;
            }
        }
        if let Some(di) = merged {
            let mut j = di + 1;
            while j < dst.len() {
                let (left, right) = dst.split_at_mut(j);
                if merge_influence_areas_two_elements(v, c, layer, &mut left[di], &mut right[0]) {
                    dst.swap_remove(j);
                } else {
                    j += 1;
                }
            }
        }
    }
    dst.extend(src.into_iter().skip(begin));
}

// AABBTreeIndirect's median-of-three quickselect. Stable scheduling alone is
// insufficient: its spatial ordering determines which eligible pair merges first.
fn partition(
    a: &mut [SupportElementMerging],
    dim: bool,
    mut left: usize,
    mut right: usize,
    k: usize,
) {
    let value = |e: &SupportElementMerging| {
        let b = e.bbox();
        if dim {
            (b.min.y + b.max.y) / 2
        } else {
            (b.min.x + b.max.x) / 2
        }
    };
    while left < right {
        let center = (left + right) / 2;
        let mut lv = value(&a[left]);
        let mut cv = value(&a[center]);
        let mut rv = value(&a[right]);
        if lv > cv {
            a.swap(left, center);
            std::mem::swap(&mut lv, &mut cv);
        }
        if lv > rv {
            a.swap(left, right);
            rv = lv;
        }
        if cv > rv {
            a.swap(center, right);
            cv = rv;
        }
        if right <= left + 2 {
            break;
        }
        let pivot = cv;
        let mut i = left;
        let mut j = right - 1;
        a.swap(center, j);
        loop {
            i += 1;
            while value(&a[i]) < pivot {
                i += 1;
            }
            j -= 1;
            while value(&a[j]) > pivot && i < j {
                j -= 1;
            }
            if i >= j {
                break;
            }
            a.swap(i, j);
        }
        a.swap(i, right - 1);
        if k < i {
            right = i - 1;
        } else if k == i {
            break;
        } else {
            left = i + 1;
        }
    }
}
fn spatial_order(a: &mut [SupportElementMerging]) {
    if a.len() < 2 {
        return;
    }
    let b = a
        .iter()
        .map(SupportElementMerging::bbox)
        .reduce(BoundingBox::merge)
        .unwrap();
    let center = (a.len() - 1) / 2;
    partition(
        a,
        b.max.y - b.min.y > b.max.x - b.min.x,
        0,
        a.len() - 1,
        center,
    );
    let (left, right) = a.split_at_mut(center + 1);
    spatial_order(left);
    spatial_order(right);
}
pub fn merge_influence_areas(
    v: &TreeModelVolumes,
    c: &TreeSupportSettings,
    layer: usize,
    a: &mut Vec<SupportElementMerging>,
) {
    merge_influence_areas_with_cancel(v, c, layer, a, || Ok::<(), std::convert::Infallible>(()))
        .unwrap();
}

/// Fallible cancellation is checked between upstream work units.
pub fn merge_influence_areas_with_cancel<E>(
    v: &TreeModelVolumes,
    c: &TreeSupportSettings,
    layer: usize,
    a: &mut Vec<SupportElementMerging>,
    mut cancel: impl FnMut() -> Result<(), E>,
) -> Result<(), E> {
    if a.len() < 2 {
        return Ok(());
    }
    spatial_order(a);
    cancel()?;
    // Fix upstream's hardware-dependent bucket choice to its single-worker
    // schedule: four entries per bucket where possible, otherwise two.
    let size = if (a.len() + 2) / 4 >= 1 { 4 } else { 2 };
    let mut buckets = vec![];
    let mut it = std::mem::take(a).into_iter();
    loop {
        let b: Vec<_> = it.by_ref().take(size).collect();
        if b.is_empty() {
            break;
        }
        buckets.push(b);
    }
    // That schedule is a reduction tree: the buckets of one round touch disjoint elements,
    // so each round runs in parallel (upstream: tbb::parallel_reduce) with the same pairs.
    buckets
        .par_iter_mut()
        .for_each(|b| merge_influence_areas_leaves(v, c, layer, b));
    cancel()?;
    while buckets.len() > 1 {
        let mut pairs = vec![];
        let mut it = buckets.into_iter();
        while let Some(dst) = it.next() {
            pairs.push((dst, it.next()));
        }
        buckets = pairs
            .into_par_iter()
            .map(|(mut dst, src)| {
                if let Some(src) = src {
                    merge_influence_areas_two_sets(v, c, layer, &mut dst, src);
                }
                dst
            })
            .collect();
        cancel()?;
    }
    *a = buckets.pop().unwrap();

    Ok(())
}

/// Propagate tips down, retaining the full collision-free reachable area after
/// every merge. The avoidance subsets only decide validity and merge eligibility.
pub fn create_layer_pathing(
    v: &TreeModelVolumes,
    c: &TreeSupportSettings,
    bounds: &mut [SupportElements],
) {
    create_layer_pathing_with_cancel(v, c, bounds, || Ok::<(), std::convert::Infallible>(()))
        .unwrap();
}

/// Fallible cancellation is checked between upstream work units.
pub fn create_layer_pathing_with_cancel<E>(
    v: &TreeModelVolumes,
    c: &TreeSupportSettings,
    bounds: &mut [SupportElements],
    mut cancel: impl FnMut() -> Result<(), E>,
) -> Result<(), E> {
    let mut last = bounds.len();
    let mut new_element = false;
    let mut every = 1;
    // These are literal scaled-coordinate constants at BOTH pins, not mm.
    let max_every = (5000 / c.maximum_move_distance.max(100))
        .min(1000 / c.maximum_move_distance_slow.max(20))
        .min(3000 / c.layer_height) as usize;
    for layer in (1..bounds.len()).rev() {
        cancel()?;
        if bounds[layer].is_empty() {
            continue;
        }
        let had_new = new_element;
        let merge = had_new || last - layer >= every;
        if had_new {
            every = 1;
        }
        let increased = increase_areas_one_layer_with_cancel(
            v,
            c,
            layer,
            &mut bounds[layer],
            merge,
            &mut cancel,
        )?;
        let below = &mut bounds[layer - 1];
        let mut candidates = vec![];
        for e in increased {
            if e.areas.influence_areas.is_empty() {
                continue;
            }
            if e.areas.to_bp_areas.is_empty() && e.areas.to_model_areas.is_empty() {
                below.push_back(e.into_element());
            } else {
                candidates.push(e);
            }
        }
        new_element = !below.is_empty();
        if merge {
            merge_influence_areas_with_cancel(v, c, layer, &mut candidates, &mut cancel)?;
            last = layer;
            // Both pins compact pointer ranges but do not resize the source
            // vector. Their size-based reduced_by_merging is therefore always
            // false. We compact owned storage, retaining that schedule decision.
            if !had_new {
                every = max_every.min(every + 1);
            }
        }
        candidates
            .par_iter_mut()
            .for_each(|e| e.areas.influence_areas = safe_union(&e.areas.influence_areas, &[]));
        for e in candidates {
            if !e.areas.influence_areas.is_empty() {
                below.push_back(e.into_element());
            }
        }
    }

    Ok(())
}

pub fn set_points_on_areas(elem: &SupportElement, above: &mut SupportElements) {
    assert!(!elem.state.deleted);
    if let Some(point) = elem.state.result_on_layer {
        for &index in &elem.parents {
            let parent = &mut above[index as usize];
            assert!(!parent.state.deleted);
            if parent.state.result_on_layer.is_none() {
                parent.state.result_on_layer =
                    Some(move_inside_if_outside(&parent.influence_area, point));
            }
            parent.state.marked = true;
        }
    }
}
pub fn set_to_model_contact_simple(elem: &mut SupportElement) {
    elem.state.result_on_layer = Some(move_inside_if_outside(
        &elem.influence_area,
        elem.state.next_position,
    ));
}
pub fn set_to_model_contact_to_model_gracious(
    v: &TreeModelVolumes,
    c: &TreeSupportSettings,
    bounds: &mut [SupportElements],
    layer: usize,
    index: usize,
) {
    let mut current = (layer, index);
    let mut last = None;
    loop {
        let e = &bounds[current.0][current.1];
        if intersection64(
            &e.influence_area,
            &v.get_placeable_areas(e.collision_radius(c), current.0),
        )
        .is_empty()
        {
            break;
        }
        last = Some(current);
        if e.parents.len() != 1 {
            break;
        }
        current = (current.0 + 1, e.parents[0] as usize);
    }
    if let Some(last) = last {
        let mut current = (layer, index);
        while current != last {
            let e = &mut bounds[current.0][current.1];
            e.state.deleted = true;
            current = (current.0 + 1, e.parents[0] as usize);
        }
        set_to_model_contact_simple(&mut bounds[last.0][last.1]);
    } else {
        let e = &mut bounds[layer][index];
        e.state.to_model_gracious = false;
        set_to_model_contact_simple(e);
    }
}
pub fn remove_deleted_elements(bounds: &mut [SupportElements]) {
    let mut parents: Vec<i32> = vec![];
    for layer in bounds.iter_mut().rev() {
        let mut map: Vec<i32> = vec![];
        let mut i = 0;
        while i < layer.len() {
            if layer[i].state.deleted {
                if map.is_empty() {
                    map = (0..layer.len() as i32).collect();
                }
                while i < layer.len() && layer.back().unwrap().state.deleted {
                    layer.pop_back();
                    map[layer.len()] = -1;
                }
                if i + 1 < layer.len() {
                    layer[i] = layer.pop_back().unwrap();
                    map[i] = -1;
                    map[layer.len()] = i as i32;
                }
            } else {
                if !parents.is_empty() {
                    for p in &mut layer[i].parents {
                        *p = parents[*p as usize];
                        assert!(*p >= 0);
                    }
                }
                i += 1;
            }
        }
        parents = map;
    }
}
pub fn create_nodes_from_area(
    v: &TreeModelVolumes,
    c: &TreeSupportSettings,
    bounds: &mut [SupportElements],
) {
    create_nodes_from_area_with_cancel(v, c, bounds, || Ok::<(), std::convert::Infallible>(()))
        .unwrap();
}

/// Fallible cancellation is checked between upstream work units.
pub fn create_nodes_from_area_with_cancel<E>(
    v: &TreeModelVolumes,
    c: &TreeSupportSettings,
    bounds: &mut [SupportElements],
    mut cancel: impl FnMut() -> Result<(), E>,
) -> Result<(), E> {
    if bounds.is_empty() {
        return Ok(());
    }
    if bounds.len() > 1 {
        for e in &mut bounds[1] {
            e.state.marked = false;
        }
    }
    for index in 0..bounds[0].len() {
        set_to_model_contact_simple(&mut bounds[0][index]);
        if bounds.len() > 1 {
            let (below, above) = bounds.split_at_mut(1);
            set_points_on_areas(&below[0][index], &mut above[0]);
        }
    }
    cancel()?;
    for layer in 1..bounds.len() {
        cancel()?;
        if layer + 1 < bounds.len() {
            for e in &mut bounds[layer + 1] {
                e.state.marked = false;
            }
        }
        for index in 0..bounds[layer].len() {
            let state = &bounds[layer][index].state;
            if state.result_on_layer.is_none() {
                if state.to_buildplate
                    || ((state.distance_to_top as usize) < c.min_dtt_to_model
                        && !state.supports_roof)
                {
                    bounds[layer][index].state.deleted = true;
                } else if state.to_model_gracious {
                    set_to_model_contact_to_model_gracious(v, c, bounds, layer, index);
                } else {
                    set_to_model_contact_simple(&mut bounds[layer][index]);
                }
            }
            let e = &mut bounds[layer][index];
            if !e.state.deleted && !e.state.marked && e.state.target_height == layer as i32 {
                e.state.deleted = true;
            }
            let (below, above) = bounds.split_at_mut(layer + 1);
            let e = &below[layer][index];
            if let Some(above) = above.first_mut() {
                if e.state.deleted {
                    for &p in &e.parents {
                        above[p as usize].state.result_on_layer_reset();
                    }
                } else {
                    set_points_on_areas(e, above);
                }
            } else {
                assert!(e.parents.is_empty());
            }
        }
    }
    remove_deleted_elements(bounds);

    Ok(())
}
