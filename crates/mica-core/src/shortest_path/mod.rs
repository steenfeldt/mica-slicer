// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: prusaslicer@b028299c77 src/libslic3r/ShortestPath.cpp :: chain_segments_closest_point, chain_segments_greedy_constrained_reversals_, update_end_point_in_queue, chain_segments_greedy_constrained_reversals2_
// Source: prusaslicer@b028299c77 src/libslic3r/ShortestPath.cpp :: chain_extrusion_entities, chain_extrusion_references, chain_extrusion_paths, chain_points, chain_expolygons, chain_layer_islands, chain_polylines
// Source: prusaslicer@b028299c77 src/libslic3r/ShortestPath.cpp :: minimum_crossover_cost, do_crossover, reorder_by_two_exchanges_with_segment_flipping, improve_ordering_by_two_exchanges_with_segment_flipping
// Changes: Rust port on index arenas (end points, chains and heap slots are indices instead of pointers).
// Changes: The KD tree, mutable priority queue and std::sort keep upstream's tie behaviour, so equal
// Changes: distances resolve as in the reference build. Extrusion entities are described by their end
// Changes: points and reversal flags ([`Entity`]) instead of the ExtrusionEntity class hierarchy.
// Upstream attribution (ShortestPath.cpp):
// Copyright (c) Prusa Research 2019 - 2023 Vojtěch Bubník @bubnikv, Lukáš Matěna @lukasmatena, Lukáš Hejl @hejllukas, David Kocík @kocikdav
//
// PrusaSlicer is released under the terms of the AGPLv3 or higher

//! Shortest-path ordering of extrusions: PrusaSlicer's greedy multi-fragment TSP chaining.
//!
//! [`chain_entities`] orders extrusion entities (infill paths, collections, loops) from a start point,
//! [`chain_points`] and [`chain_expolygons`] order islands, and [`chain_polylines`] orders fill or brim
//! lines with chain flipping plus the two-exchange improvement pass.

mod cxx_sort;
mod kdtree;
mod queue;
#[cfg(test)]
mod tests;

use crate::geometry::{ExPolygon, Point, Polyline};
use kdtree::KdTree;
use queue::{INVALID, Items, Queue};

const NONE: usize = usize::MAX;
/// PrusaSlicer's `EPSILON` and `SCALED_EPSILON` (`scale_(EPSILON)`, a double).
const EPSILON: f64 = 1e-4;
const SCALING_FACTOR: f64 = 0.000001;
const SCALED_EPSILON: f64 = EPSILON / SCALING_FACTOR;

fn pos(p: Point) -> [f64; 2] {
    [p.x as f64, p.y as f64]
}

fn squared_norm(a: [f64; 2], b: [f64; 2]) -> f64 {
    let (x, y) = (a[0] - b[0], a[1] - b[1]);
    x * x + y * y
}

fn norm(a: [f64; 2], b: [f64; 2]) -> f64 {
    squared_norm(a, b).sqrt()
}

/// End points of segment `i`: index `2 i` is the `first_point == true` end, `2 i + 1` the other.
fn end_points(end_point: &impl Fn(usize, bool) -> Point, n: usize) -> Vec<[f64; 2]> {
    let mut out = Vec::with_capacity(n * 2);
    for i in 0..n {
        out.push(pos(end_point(i, true)));
        out.push(pos(end_point(i, false)));
    }
    out
}

/// `chain_segments_closest_point`: nearest-neighbour fallback that honours reversal constraints.
fn chain_segments_closest_point(
    pts: &[[f64; 2]],
    kdtree: &KdTree,
    could_reverse: &impl Fn(usize) -> bool,
    first_point_idx: usize,
) -> Vec<(usize, bool)> {
    let num_segments = pts.len() / 2;
    let mut chain_id = vec![0usize; pts.len()];
    let mut out = Vec::with_capacity(num_segments);
    out.push((first_point_idx / 2, first_point_idx & 1 != 0));
    chain_id[first_point_idx] = 1;
    let mut this_idx = first_point_idx ^ 1;
    for _ in 0..num_segments.saturating_sub(1) {
        chain_id[this_idx] = 1;
        let next_idx = kdtree.find_closest(pts, pts[this_idx], |idx| {
            (idx ^ this_idx) > 1 && chain_id[idx] == 0 && (idx & 1 == 0 || could_reverse(idx >> 1))
        });
        chain_id[next_idx] = 1;
        out.push((next_idx / 2, next_idx & 1 != 0));
        this_idx = next_idx ^ 1;
    }
    out
}

/// Walks the `edge_out` links from `first_point`; `None` when a segment would need a forbidden reversal.
fn walk(
    first_point: usize,
    edge_out: impl Fn(usize) -> usize,
    could_reverse: &impl Fn(usize) -> bool,
    check: bool,
    n: usize,
) -> Option<Vec<(usize, bool)>> {
    let mut out = Vec::with_capacity(n);
    let mut first_point = first_point;
    loop {
        let segment_id = first_point >> 1;
        let reverse = first_point & 1 != 0;
        if check && reverse && !could_reverse(segment_id) {
            return None;
        }
        out.push((segment_id, reverse));
        first_point = edge_out(first_point ^ 1);
        if first_point == NONE {
            return Some(out);
        }
    }
}

/// `EquivalentChains` of `chain_segments_greedy_constrained_reversals_`.
struct EquivalentChains {
    last: usize,
    with: Vec<usize>,
}

impl EquivalentChains {
    fn next(&mut self) -> usize {
        self.last += 1;
        self.with.push(self.last);
        self.last
    }
    fn get(&mut self, chain_id: usize) -> usize {
        if chain_id == 0 {
            return 0;
        }
        let mut last = chain_id;
        loop {
            let lower = self.with[last];
            if lower == last {
                self.with[chain_id] = lower;
                return lower;
            }
            last = lower;
        }
    }
    fn merge(&mut self, a: usize, b: usize) -> usize {
        let id = self.get(a).min(self.get(b));
        self.with[a] = id;
        self.with[b] = id;
        id
    }
}

struct EndPoint1 {
    chain_id: usize,
    edge_out: usize,
    distance_out: f64,
    heap_idx: usize,
}

struct Store1(Vec<EndPoint1>);

impl Items for Store1 {
    fn less(&self, a: usize, b: usize) -> bool {
        self.0[a].distance_out < self.0[b].distance_out
    }
    fn set_index(&mut self, item: usize, idx: usize) {
        self.0[item].heap_idx = idx;
    }
}

/// `chain_segments_greedy_constrained_reversals_`: greedy multi-fragment chaining of segments whose
/// end points are given by `end_point(i, first)`. With `could_fail`, segments that may not be reversed
/// are honoured (REVERSE_COULD_FAIL), falling back to [`chain_segments_closest_point`].
fn chain_segments_greedy_constrained_reversals_(
    end_point: impl Fn(usize, bool) -> Point,
    could_reverse: impl Fn(usize) -> bool,
    num_segments: usize,
    start_near: Option<Point>,
    could_fail: bool,
) -> Vec<(usize, bool)> {
    if num_segments == 0 {
        return Vec::new();
    }
    if num_segments == 1 {
        let reverse = could_reverse(0)
            && start_near.is_some_and(|s| {
                squared_norm(pos(end_point(0, false)), pos(s))
                    < squared_norm(pos(end_point(0, true)), pos(s))
            });
        return vec![(0, reverse)];
    }
    let pts = end_points(&end_point, num_segments);
    let kdtree = KdTree::build(&pts);
    let mut eq = EquivalentChains {
        last: 0,
        with: {
            let mut v = Vec::with_capacity(num_segments);
            v.push(0);
            v
        },
    };
    let mut eps = Store1(
        (0..pts.len())
            .map(|_| EndPoint1 {
                chain_id: 0,
                edge_out: NONE,
                distance_out: f64::MAX,
                heap_idx: usize::MAX,
            })
            .collect(),
    );

    let mut first_point = NONE;
    if let Some(s) = start_near {
        let idx = kdtree.find_closest(&pts, pos(s), |idx| idx & 1 == 0 || could_reverse(idx >> 1));
        first_point = idx;
        eps.0[idx].distance_out = 0.;
        eps.0[idx].chain_id = eq.next();
    }
    let initial_point = first_point;
    let first_point_idx = first_point;
    let last_point;

    for this_idx in 0..pts.len() {
        if this_idx != first_point {
            let next_idx = kdtree.find_closest(&pts, pts[this_idx], |idx| {
                idx != first_point_idx && (idx ^ this_idx) > 1
            });
            eps.0[this_idx].edge_out = next_idx;
            eps.0[this_idx].distance_out = squared_norm(pts[next_idx], pts[this_idx]);
        }
    }

    let mut queue = Queue::new(pts.len() * 2 - 1, false);
    for i in 0..pts.len() {
        if i != first_point {
            queue.push(i, &mut eps);
        }
    }

    let mut iter = num_segments as isize - 2;
    loop {
        let ep1 = queue.top();
        let ep2 = eps.0[ep1].edge_out;
        let mut valid = true;
        let mut other1 = 0;
        let mut other2 = 0;
        if eps.0[ep2].chain_id > 0 {
            valid = false;
        } else {
            other1 = eq.get(eps.0[ep1 ^ 1].chain_id);
            other2 = eq.get(eps.0[ep2 ^ 1].chain_id);
            if other1 == other2 && other1 != 0 {
                valid = false;
            }
        }
        if valid {
            queue.pop(&mut eps);
            let h = eps.0[ep2].heap_idx;
            queue.remove(h, &mut eps);
            eps.0[ep2].edge_out = ep1;
            eps.0[ep2].distance_out = eps.0[ep1].distance_out;
            let chain_id = if other1 == 0 {
                if other2 == 0 { eq.next() } else { other2 }
            } else if other2 == 0 || other1 == other2 {
                other1
            } else {
                eq.merge(other1, other2)
            };
            eps.0[ep1].chain_id = chain_id;
            eps.0[ep2].chain_id = chain_id;
            if iter == 0 {
                if first_point == NONE {
                    first_point = queue.top();
                    queue.pop(&mut eps);
                    eps.0[first_point].edge_out = NONE;
                }
                last_point = queue.top();
                eps.0[last_point].edge_out = NONE;
                queue.pop(&mut eps);
                break;
            }
        } else {
            iter += 1;
            eps.0[ep1].edge_out = NONE;
            let this_idx = ep1;
            let next_idx = {
                let eps_ro = &eps.0;
                kdtree.find_closest(&pts, pts[ep1], |idx| {
                    if (idx ^ this_idx) <= 1 || eps_ro[idx].chain_id != 0 {
                        return false;
                    }
                    let chain1 = eq.get(eps_ro[this_idx ^ 1].chain_id);
                    let chain2 = eq.get(eps_ro[idx ^ 1].chain_id);
                    chain1 != chain2 || chain1 == 0
                })
            };
            eps.0[ep1].edge_out = next_idx;
            eps.0[ep1].distance_out = squared_norm(pts[next_idx], pts[ep1]);
            let h = eps.0[ep1].heap_idx;
            queue.update(h, &mut eps);
        }
        iter -= 1;
    }

    let edge_out = |i: usize| eps.0[i].edge_out;
    if let Some(out) = walk(
        first_point,
        edge_out,
        &could_reverse,
        could_fail,
        num_segments,
    ) {
        return out;
    }
    if start_near.is_none()
        && let Some(out) = walk(last_point, edge_out, &could_reverse, true, num_segments)
    {
        return out;
    }
    chain_segments_closest_point(
        &pts,
        &kdtree,
        &could_reverse,
        if initial_point != NONE {
            initial_point
        } else {
            0
        },
    )
}

#[derive(Clone, Copy, Default)]
struct Chain {
    num_segments: usize,
    cost: f64,
    cost_flipped: f64,
    begin: usize,
    end: usize,
    equivalent_with: usize,
}

struct EndPoint2 {
    edge_candidate: usize,
    distance_out: f64,
    heap_idx: usize,
    chain_id: usize,
    edge_out: usize,
}

struct Store2(Vec<EndPoint2>);

impl Items for Store2 {
    fn less(&self, a: usize, b: usize) -> bool {
        self.0[a].distance_out < self.0[b].distance_out
    }
    fn set_index(&mut self, item: usize, idx: usize) {
        self.0[item].heap_idx = idx;
    }
}

struct Chains {
    chains: Vec<Chain>,
    last: usize,
}

impl Chains {
    fn next_id(&mut self) -> usize {
        self.last += 1;
        self.chains.push(Chain {
            begin: NONE,
            end: NONE,
            equivalent_with: self.last,
            ..Default::default()
        });
        self.last
    }
    fn equivalent(&mut self, chain_id: usize) -> usize {
        if chain_id == 0 {
            return 0;
        }
        let mut last = chain_id;
        loop {
            let lower = self.chains[last].equivalent_with;
            if lower == last {
                self.chains[chain_id].equivalent_with = lower;
                return lower;
            }
            last = lower;
        }
    }
    fn merge(&mut self, a: usize, b: usize) -> usize {
        if a == 0 {
            return if b == 0 { self.next_id() } else { b };
        }
        if b == 0 {
            return a;
        }
        let id = a.min(b);
        self.chains[a].equivalent_with = id;
        self.chains[b].equivalent_with = id;
        id
    }
    fn flip_penalty(&mut self, chain_id: usize) -> f64 {
        let id = self.equivalent(chain_id);
        self.chains[id].cost_flipped - self.chains[id].cost
    }
    /// `Chain::flip`: reverses the chain's linked end points and swaps its costs and end chain ids.
    fn flip(&mut self, id: usize, eps: &mut [EndPoint2]) {
        let c = &mut self.chains[id];
        let mut ept = c.begin;
        let mut ept_prev = NONE;
        loop {
            let ept_end = ept ^ 1;
            let ept_next = eps[ept_end].edge_out;
            eps[ept_end].edge_out = ept_prev;
            if ept_prev != NONE {
                eps[ept_prev].edge_out = ept_end;
            }
            ept_prev = ept;
            ept = ept_next;
            if ept == NONE {
                break;
            }
        }
        eps[ept_prev].edge_out = NONE;
        std::mem::swap(&mut c.cost, &mut c.cost_flipped);
        let new_begin = c.begin ^ 1;
        let new_end = c.end ^ 1;
        let t = eps[c.begin].chain_id;
        eps[c.begin].chain_id = eps[new_begin].chain_id;
        eps[new_begin].chain_id = t;
        let t = eps[c.end].chain_id;
        eps[c.end].chain_id = eps[new_end].chain_id;
        eps[new_end].chain_id = t;
        c.begin = new_begin;
        c.end = new_end;
    }
}

/// `update_end_point_in_queue`.
#[allow(clippy::too_many_arguments)]
fn update_end_point_in_queue(
    queue: &mut Queue,
    kdtree: &KdTree,
    pts: &[[f64; 2]],
    chains: &mut Chains,
    eps: &mut Store2,
    this_idx: usize,
    first_point_idx: usize,
    first_point: usize,
) {
    eps.0[this_idx].edge_candidate = NONE;
    if first_point_idx == this_idx
        || (eps.0[this_idx].chain_id > 0 && first_point_idx == (this_idx ^ 1))
    {
        if eps.0[this_idx].heap_idx != INVALID {
            let h = eps.0[this_idx].heap_idx;
            queue.remove(h, eps);
        }
        return;
    }
    let chain1a = eps.0[this_idx].chain_id;
    let chain1b = eps.0[this_idx ^ 1].chain_id;
    let this_chain = chains.equivalent(chain1a.max(chain1b));
    let next_idx = {
        let e = &eps.0;
        kdtree.find_closest(pts, pts[this_idx], |idx| {
            if (idx ^ this_idx) <= 1 || idx == first_point_idx {
                return false;
            }
            let chain2a = e[idx].chain_id;
            let chain2b = e[idx ^ 1].chain_id;
            if chain2a > 0 && chain2b > 0 {
                return false;
            }
            let chain2 = chains.equivalent(chain2a.max(chain2b));
            if this_chain == chain2 {
                return this_chain == 0;
            }
            if chain2a > 0 {
                let chain = &chains.chains[chain2];
                if chain.begin == first_point || chain.end == first_point {
                    return false;
                }
            }
            true
        })
    };
    eps.0[this_idx].edge_candidate = next_idx;
    eps.0[this_idx].distance_out = norm(pts[next_idx], pts[this_idx]);
    if eps.0[this_idx].chain_id > 0 {
        eps.0[this_idx].distance_out += chains.flip_penalty(this_chain);
    }
    if eps.0[next_idx].chain_id > 0 {
        let c = eps.0[next_idx].chain_id;
        eps.0[this_idx].distance_out += chains.flip_penalty(c);
    }
    if eps.0[this_idx].heap_idx == INVALID {
        queue.push(this_idx, eps);
    } else {
        let h = eps.0[this_idx].heap_idx;
        queue.update(h, eps);
    }
}

/// `chain_segments_greedy_constrained_reversals2_` with every segment reversible (`chain_segments_greedy2`):
/// greedy chaining that may flip already connected chains.
fn chain_segments_greedy2(
    end_point: impl Fn(usize, bool) -> Point,
    num_segments: usize,
    start_near: Option<Point>,
) -> Vec<(usize, bool)> {
    if num_segments == 0 {
        return Vec::new();
    }
    if num_segments == 1 {
        // Upstream compares the first point against the last here (the reverse of the constrained
        // variant); kept as is.
        let reverse = start_near.is_some_and(|s| {
            squared_norm(pos(end_point(0, true)), pos(s))
                < squared_norm(pos(end_point(0, false)), pos(s))
        });
        return vec![(0, reverse)];
    }
    let pts = end_points(&end_point, num_segments);
    let kdtree = KdTree::build(&pts);
    let mut chains = Chains {
        chains: {
            let mut v = Vec::with_capacity(num_segments / 2 + 1);
            v.push(Chain {
                begin: NONE,
                end: NONE,
                ..Default::default()
            });
            v
        },
        last: 0,
    };
    let mut eps = Store2(
        (0..pts.len())
            .map(|_| EndPoint2 {
                edge_candidate: NONE,
                distance_out: f64::MAX,
                heap_idx: INVALID,
                chain_id: 0,
                edge_out: NONE,
            })
            .collect(),
    );

    let mut first_point = NONE;
    if let Some(s) = start_near {
        let idx = kdtree.find_closest(&pts, pos(s), |_| true);
        first_point = idx;
        eps.0[idx].distance_out = 0.;
        let id = chains.next_id();
        eps.0[idx].chain_id = id;
        chains.chains[id].begin = idx;
        chains.chains[id].end = idx ^ 1;
    }
    let first_point_idx = first_point;

    for this_idx in 0..pts.len() {
        if this_idx != first_point {
            let next_idx = kdtree.find_closest(&pts, pts[this_idx], |idx| {
                idx != first_point_idx && (idx ^ this_idx) > 1
            });
            eps.0[this_idx].edge_candidate = next_idx;
            eps.0[this_idx].distance_out = norm(pts[next_idx], pts[this_idx]);
        }
    }

    let mut queue = Queue::new(pts.len() * 2, true);
    for i in 0..pts.len() {
        if i != first_point {
            queue.push(i, &mut eps);
        }
    }

    let mut num_connections_to_end = num_segments - 1;
    let mut num_iter = num_segments * 16;
    while num_iter > 0 {
        num_iter -= 1;
        let ep1 = queue.top();
        let ep1o = ep1 ^ 1;
        let chain1_flip = eps.0[ep1].chain_id > 0;
        let ep2 = eps.0[ep1].edge_candidate;
        let ep2o = ep2 ^ 1;
        let chain2_flip = eps.0[ep2].chain_id > 0;
        let mut valid = true;
        let mut c1 = 0;
        let mut c2 = 0;
        if eps.0[ep2].chain_id > 0 && eps.0[ep2o].chain_id > 0 {
            valid = false;
        } else {
            c1 = chains.equivalent(eps.0[if chain1_flip { ep1 } else { ep1o }].chain_id);
            c2 = chains.equivalent(eps.0[if chain2_flip { ep2 } else { ep2o }].chain_id);
            if c1 == c2 && c1 != 0 {
                valid = false;
            } else {
                let mut dist = norm(pts[ep2], pts[ep1]);
                if chain1_flip {
                    dist += chains.flip_penalty(c1);
                }
                if chain2_flip {
                    dist += chains.flip_penalty(c2);
                }
                if (dist - eps.0[ep1].distance_out).abs() > SCALED_EPSILON {
                    valid = false;
                }
            }
            if valid && first_point != NONE {
                if chain1_flip {
                    let ch = &chains.chains[c1];
                    if ch.begin == first_point || ch.end == first_point {
                        valid = false;
                    }
                }
                if valid && chain2_flip {
                    let ch = &chains.chains[c2];
                    if ch.begin == first_point || ch.end == first_point {
                        valid = false;
                    }
                }
            }
        }
        if !valid {
            update_end_point_in_queue(
                &mut queue,
                &kdtree,
                &pts,
                &mut chains,
                &mut eps,
                ep1,
                first_point_idx,
                first_point,
            );
            continue;
        }
        queue.pop(&mut eps);
        let h = eps.0[ep2].heap_idx;
        queue.remove(h, &mut eps);
        eps.0[ep1].edge_candidate = NONE;
        if chain1_flip {
            chains.flip(c1, &mut eps.0);
        }
        if chain2_flip {
            chains.flip(c2, &mut eps.0);
        }
        let chain_id = chains.merge(c1, c2);
        let chain1 = (c1 != 0).then(|| chains.chains[c1]);
        let chain2 = (c2 != 0).then(|| chains.chains[c2]);
        let chain_dst = Chain {
            begin: match chain1 {
                None => ep1o,
                Some(c) if c.begin == ep1 => c.end,
                Some(c) => c.begin,
            },
            end: match chain2 {
                None => ep2o,
                Some(c) if c.begin == ep2 => c.end,
                Some(c) => c.begin,
            },
            cost: chain1.map_or(0., |c| c.cost)
                + chain2.map_or(0., |c| c.cost)
                + norm(pts[ep2], pts[ep1]),
            cost_flipped: chain1.map_or(0., |c| c.cost_flipped)
                + chain2.map_or(0., |c| c.cost_flipped)
                + norm(pts[ep2o], pts[ep1o]),
            num_segments: chain1.map_or(1, |c| c.num_segments)
                + chain2.map_or(1, |c| c.num_segments),
            equivalent_with: chain_id,
        };
        chains.chains[chain_id] = chain_dst;
        let chain = chain_dst;
        if chain.begin != ep1o && eps.0[ep1o].heap_idx != INVALID {
            let h = eps.0[ep1o].heap_idx;
            queue.remove(h, &mut eps);
        }
        if chain.end != ep2o && eps.0[ep2o].heap_idx != INVALID {
            let h = eps.0[ep2o].heap_idx;
            queue.remove(h, &mut eps);
        }
        eps.0[ep1].edge_out = ep2;
        eps.0[ep2].edge_out = ep1;
        for e in [ep1, ep2, ep1o, ep2o] {
            eps.0[e].chain_id = chain_id;
        }
        if chain.begin != first_point {
            eps.0[chain.begin].chain_id = 0;
        }
        if chain.end != first_point {
            eps.0[chain.end].chain_id = 0;
        }
        num_connections_to_end -= 1;
        if num_connections_to_end == 0 {
            if first_point == NONE {
                loop {
                    first_point = queue.top();
                    queue.pop(&mut eps);
                    if eps.0[first_point].edge_out == NONE {
                        break;
                    }
                }
            }
            // Upstream then pops the last free end point; only the reverse walk of the
            // constrained variant uses it.
            break;
        }
        let upd = |e: usize, queue: &mut Queue, chains: &mut Chains, eps: &mut Store2| {
            update_end_point_in_queue(
                queue,
                &kdtree,
                &pts,
                chains,
                eps,
                e,
                first_point_idx,
                first_point,
            )
        };
        upd(chain.begin ^ 1, &mut queue, &mut chains, &mut eps);
        upd(chain.end ^ 1, &mut queue, &mut chains, &mut eps);
        if chain1_flip {
            upd(chain.begin, &mut queue, &mut chains, &mut eps);
        }
        if chain2_flip {
            upd(chain.end, &mut queue, &mut chains, &mut eps);
        }
    }
    if first_point == NONE {
        // Upstream would dereference a null end point here (iteration limit exhausted).
        return (0..num_segments).map(|i| (i, false)).collect();
    }
    walk(
        first_point,
        |i| eps.0[i].edge_out,
        &|_| true,
        false,
        num_segments,
    )
    .unwrap_or_default()
}

/// `chain_segments_greedy` (every segment reversible).
fn chain_segments_greedy(
    end_point: impl Fn(usize, bool) -> Point,
    num_segments: usize,
    start_near: Option<Point>,
) -> Vec<(usize, bool)> {
    chain_segments_greedy_constrained_reversals_(
        end_point,
        |_| true,
        num_segments,
        start_near,
        false,
    )
}

/// An extrusion entity as the chaining sees it: `ExtrusionEntity::first_point / last_point`,
/// `is_loop()` and `can_reverse()`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entity {
    pub first: Point,
    pub last: Point,
    pub is_loop: bool,
    pub can_reverse: bool,
}

/// `chain_extrusion_entities` / `chain_extrusion_references`: the visiting order of `entities` from
/// `start_near`, with a reversal flag per entity. `reversed` treats the input as already reversed.
pub fn chain_entities(
    entities: &[Entity],
    start_near: Option<Point>,
    reversed: bool,
) -> Vec<(usize, bool)> {
    let mut out = chain_segments_greedy_constrained_reversals_(
        |idx, first_point| {
            if first_point == reversed {
                entities[idx].last
            } else {
                entities[idx].first
            }
        },
        |idx| entities[idx].is_loop || entities[idx].can_reverse,
        entities.len(),
        start_near,
        true,
    );
    for segment in &mut out {
        let e = &entities[segment.0];
        if e.is_loop {
            segment.1 = false;
        } else if reversed {
            segment.1 = !segment.1;
        }
    }
    out
}

/// `chain_extrusion_paths`: orders open paths given by their end points (all reversible).
pub fn chain_paths(ends: &[(Point, Point)], start_near: Option<Point>) -> Vec<(usize, bool)> {
    chain_segments_greedy(
        |idx, first| if first { ends[idx].0 } else { ends[idx].1 },
        ends.len(),
        start_near,
    )
}

/// `chain_points`: visiting order of single points.
pub fn chain_points(points: &[Point], start_near: Option<Point>) -> Vec<usize> {
    chain_segments_greedy(|idx, _| points[idx], points.len(), start_near)
        .into_iter()
        .map(|(i, _)| i)
        .collect()
}

/// `chain_expolygons`: print order of islands by the first point of their contours.
pub fn chain_expolygons(expolygons: &[ExPolygon]) -> Vec<usize> {
    let points: Vec<Point> = expolygons
        .iter()
        .map(|e| e.contour.first().copied().unwrap_or_default())
        .collect();
    chain_points(&points, None)
}

#[derive(Clone, Copy)]
struct FlipEdge {
    p1: [f64; 2],
    p2: [f64; 2],
    source_index: usize,
}

impl FlipEdge {
    fn flip(&mut self) {
        std::mem::swap(&mut self.p1, &mut self.p2);
    }
}

#[derive(Clone, Copy, Default)]
struct ConnectionCost {
    cost: f64,
    cost_flipped: f64,
}

impl std::ops::Sub for ConnectionCost {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self {
        Self {
            cost: self.cost - rhs.cost,
            cost_flipped: self.cost_flipped - rhs.cost_flipped,
        }
    }
}

type Span = (usize, usize);

#[allow(clippy::too_many_arguments)]
fn connection_cost(
    edges: &[FlipEdge],
    span1: Span,
    cost1: ConnectionCost,
    reversed1: bool,
    flipped1: bool,
    span2: Span,
    cost2: ConnectionCost,
    reversed2: bool,
    flipped2: bool,
    span3: Span,
    cost3: ConnectionCost,
    reversed3: bool,
    flipped3: bool,
) -> f64 {
    let first_point = |span: Span, flipped: bool| {
        if flipped {
            edges[span.0].p2
        } else {
            edges[span.0].p1
        }
    };
    let last_point = |span: Span, flipped: bool| {
        if flipped {
            edges[span.1 - 1].p1
        } else {
            edges[span.1 - 1].p2
        }
    };
    let point = |span: Span, start: bool, flipped: bool| {
        if start {
            first_point(span, flipped)
        } else {
            last_point(span, flipped)
        }
    };
    let cost = |c: ConnectionCost, flipped: bool| if flipped { c.cost_flipped } else { c.cost };
    let ignore = |span: Span, reversed: bool| span.0 + 1 == span.1 && reversed;
    if ignore(span1, reversed1) || ignore(span2, reversed2) || ignore(span3, reversed3) {
        return f64::MAX;
    }
    cost(cost1, flipped1)
        + cost(cost2, flipped2)
        + cost(cost3, flipped3)
        + norm(
            point(span2, !reversed2, flipped2),
            point(span1, reversed1, flipped1),
        )
        + norm(
            point(span3, !reversed3, flipped3),
            point(span2, reversed2, flipped2),
        )
}

/// `minimum_crossover_cost` for three spans.
fn minimum_crossover_cost(
    edges: &[FlipEdge],
    spans: [(Span, ConnectionCost); 3],
    cost_current: f64,
) -> (f64, usize) {
    let [(s1, k1), (s2, k2), (s3, k3)] = spans;
    let mut cost_min = cost_current;
    let mut flip_min = 0;
    for i in 0..(1usize << 6) {
        let b = |bit: usize| i & (1 << bit) != 0;
        let c1 = if i == 0 {
            cost_current
        } else {
            connection_cost(
                edges,
                s1,
                k1,
                b(0),
                b(1),
                s2,
                k2,
                b(2),
                b(3),
                s3,
                k3,
                b(4),
                b(5),
            )
        };
        let c2 = connection_cost(
            edges,
            s1,
            k1,
            b(0),
            b(1),
            s3,
            k3,
            b(2),
            b(3),
            s2,
            k2,
            b(4),
            b(5),
        );
        let c3 = connection_cost(
            edges,
            s2,
            k2,
            b(0),
            b(1),
            s1,
            k1,
            b(2),
            b(3),
            s3,
            k3,
            b(4),
            b(5),
        );
        if c1 < cost_min {
            cost_min = c1;
            flip_min = i;
        }
        if c2 < cost_min {
            cost_min = c2;
            flip_min = i + (1 << 6);
        }
        if c3 < cost_min {
            cost_min = c3;
            flip_min = i + (2 << 6);
        }
    }
    (cost_min, flip_min)
}

/// `do_crossover` for three spans.
fn do_crossover(edges_in: &[FlipEdge], edges_out: &mut [FlipEdge], spans: [Span; 3], i: usize) {
    let [s1, s2, s3] = spans;
    let order = match i >> 6 {
        0 => [s1, s2, s3],
        1 => [s1, s3, s2],
        _ => [s2, s1, s3],
    };
    let mut out = 0;
    for (k, span) in order.into_iter().enumerate() {
        let reversed = i & (1 << (2 * k)) != 0;
        let flipped = i & (1 << (2 * k + 1)) != 0;
        let start = out;
        let src = &edges_in[span.0..span.1];
        if reversed {
            for (o, e) in edges_out[out..out + src.len()]
                .iter_mut()
                .zip(src.iter().rev())
            {
                *o = *e;
            }
        } else {
            edges_out[out..out + src.len()].copy_from_slice(src);
        }
        out += src.len();
        if reversed != flipped {
            for e in &mut edges_out[start..out] {
                e.flip();
            }
        }
    }
}

/// `reorder_by_two_exchanges_with_segment_flipping`.
fn reorder_by_two_exchanges_with_segment_flipping(edges: &mut Vec<FlipEdge>) {
    if edges.len() < 2 {
        return;
    }
    let n = edges.len();
    let mut connections = vec![ConnectionCost::default(); n];
    let mut edges_tmp = edges.clone();
    let mut connection_lengths = vec![(0f64, 0usize); n - 1];
    let mut connection_tried = vec![false; n];
    let max_iterations = n.min(100);
    for _ in 0..max_iterations {
        for i in 1..n {
            let (e1, e2) = (edges[i - 1], edges[i]);
            let mut c = connections[i - 1];
            let l = norm(e2.p1, e1.p2);
            c.cost += l;
            c.cost_flipped += norm(e2.p2, e1.p1);
            connections[i] = c;
            connection_lengths[i - 1] = (l, i);
        }
        cxx_sort::sort_by(&mut connection_lengths, |l, r| l.0 > r.0);
        connection_tried.fill(false);
        let total = connections[n - 1];
        let mut final_ = (usize::MAX, usize::MAX, 0usize);
        for &(_, longest) in &connection_lengths {
            connection_tried[longest] = true;
            let mut crossover_pos_min = usize::MAX;
            let mut crossover_cost_min = total.cost;
            let mut crossover_flip_min = 0;
            for (j, &tried) in connection_tried.iter().enumerate().skip(1) {
                if tried {
                    continue;
                }
                let (a, b) = if j > longest {
                    (longest, j)
                } else {
                    (j, longest)
                };
                let (cost, flip) = minimum_crossover_cost(
                    edges,
                    [
                        ((0, a), connections[a - 1]),
                        ((a, b), connections[b - 1] - connections[a]),
                        ((b, n), total - connections[b]),
                    ],
                    total.cost,
                );
                if flip > 0 && cost < crossover_cost_min {
                    crossover_pos_min = j;
                    crossover_cost_min = cost;
                    crossover_flip_min = flip;
                }
            }
            if crossover_cost_min < total.cost {
                final_ = (longest, crossover_pos_min, crossover_flip_min);
                break;
            }
        }
        let (mut p1, mut p2, flip) = final_;
        if flip == 0 {
            break;
        }
        if p1 > p2 {
            std::mem::swap(&mut p1, &mut p2);
        }
        do_crossover(edges, &mut edges_tmp, [(0, p1), (p1, p2), (p2, n)], flip);
        std::mem::swap(edges, &mut edges_tmp);
    }
}

/// `improve_ordering_by_two_exchanges_with_segment_flipping`.
fn improve_ordering_by_two_exchanges_with_segment_flipping(
    polylines: Vec<Polyline>,
) -> Vec<Polyline> {
    let mut edges: Vec<FlipEdge> = polylines
        .iter()
        .enumerate()
        .map(|(i, pl)| FlipEdge {
            p1: pos(pl[0]),
            p2: pos(*pl.last().unwrap()),
            source_index: i,
        })
        .collect();
    reorder_by_two_exchanges_with_segment_flipping(&mut edges);
    let mut src: Vec<Option<Polyline>> = polylines.into_iter().map(Some).collect();
    edges
        .iter()
        .map(|edge| {
            let mut pl = src[edge.source_index].take().unwrap_or_default();
            if edge.p2 == pos(pl[0]) {
                pl.reverse();
            }
            pl
        })
        .collect()
}

/// `chain_polylines`: orders (and reverses) fill or brim lines; without a start point the order is
/// improved further by two-exchanges with chain flipping. Empty polylines are kept out of the
/// chaining by callers (upstream never passes them).
pub fn chain_polylines(polylines: Vec<Polyline>, start_near: Option<Point>) -> Vec<Polyline> {
    if polylines.is_empty() {
        return polylines;
    }
    let ordered = chain_segments_greedy2(
        |idx, first| {
            if first {
                polylines[idx][0]
            } else {
                *polylines[idx].last().unwrap()
            }
        },
        polylines.len(),
        start_near,
    );
    let mut src: Vec<Option<Polyline>> = polylines.into_iter().map(Some).collect();
    let out: Vec<Polyline> = ordered
        .into_iter()
        .map(|(i, rev)| {
            let mut pl = src[i].take().unwrap_or_default();
            if rev {
                pl.reverse();
            }
            pl
        })
        .collect();
    if out.len() > 1 && start_near.is_none() {
        improve_ordering_by_two_exchanges_with_segment_flipping(out)
    } else {
        out
    }
}
