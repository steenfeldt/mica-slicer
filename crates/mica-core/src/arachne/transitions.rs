// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: prusaslicer@b028299c77 src/libslic3r/Arachne/SkeletalTrapezoidation.cpp :: updateIsCentral, updateBeadCount, filterNoncentralRegions, generateTransitioningRibs, generateExtraRibs
// Changes: Index-based graph; explicit visited sets bound traversal around degenerate cycles.
// Copyright (c) 2021 Ultimaker B.V.
// CuraEngine is released under the terms of the AGPLv3 or higher.

use super::Skeleton;
use super::graph::{EdgeType, End, Mid, lerp};
use std::collections::HashSet;

impl Skeleton {
    pub fn mark(&mut self) {
        let cap = (self.strategy.angle * 0.5).sin();
        for e in self.graph.edge_ids() {
            let edge = &self.graph.edges[e];
            if !edge.alive {
                continue;
            }
            let a = &self.graph.nodes[edge.from];
            let b = &self.graph.nodes[edge.to];
            let central = edge.kind != EdgeType::Extra
                && a.radius.max(b.radius) >= self.strategy.transition_thickness(0) / 2.
                && (b.radius - a.radius).abs() < self.graph.len(e) * cap;
            self.graph.edges[e].central = central;
        }
        // Upstream filterCentral's predicate is isLocalMaximum() && !isLocalMaximum(); it is a no-op.
        for e in self.graph.edge_ids() {
            if self.graph.edges[e].alive && self.graph.edges[e].central {
                let n = self.graph.edges[e].to;
                self.graph.nodes[n].count =
                    self.strategy.count(self.graph.nodes[n].radius * 2.) as i32;
            }
        }
        for n in 0..self.graph.nodes.len() {
            if self.graph.nodes[n].alive && self.graph.local_max(n, false) {
                self.graph.nodes[n].count =
                    self.strategy.count(self.graph.nodes[n].radius * 2.) as i32;
            }
        }
        for e in self.graph.edge_ids() {
            if self.graph.edges[e].alive && self.graph.end_central(e) {
                let count = self.graph.nodes[self.graph.edges[e].to].count;
                self.filter_noncentral(e, count, 0., &mut HashSet::new());
            }
        }
    }
    fn filter_noncentral(
        &mut self,
        e: usize,
        count: i32,
        traveled: f64,
        seen: &mut HashSet<usize>,
    ) -> bool {
        if !seen.insert(e) {
            return false;
        }
        let r = self.graph.nodes[self.graph.edges[e].to].radius;
        let next = self.graph.beyond(e).into_iter().find(|&x| {
            self.graph.nodes[self.graph.edges[x].to].radius >= r || self.graph.len(x) < 10_000.
        });
        let Some(next) = next else {
            return false;
        };
        let n = self.graph.edges[next].to;
        let length = self.graph.len(next);
        let current = self.graph.nodes[n].count;
        let dissolve = if current == count {
            true
        } else if current < 0 {
            self.filter_noncentral(next, count, traveled + length, seen)
        } else {
            traveled + length < 400_000. && (current - count).abs() == 1
        };
        if dissolve {
            let twin = self.graph.edges[next].twin;
            self.graph.edges[next].central = true;
            self.graph.edges[twin].central = true;
            self.graph.nodes[n].count = self.strategy.count(self.graph.nodes[n].radius * 2.) as i32;
            self.graph.nodes[n].ratio = 0.;
        }
        dissolve
    }
    pub fn transitions(&mut self) {
        self.generate_mids();
        self.filter_mids();
        for e in self.graph.edge_ids() {
            if !self.graph.edges[e].alive {
                continue;
            }
            for mid in self.graph.edges[e].mids.clone() {
                let length = self.strategy.length(mid.count);
                let anchor = self.strategy.anchor(mid.count);
                let half = anchor * length;
                let start = self.graph.len(e) - mid.pos;
                let twin = self.graph.edges[e].twin;
                self.transition_end(
                    twin,
                    start,
                    start + half,
                    half,
                    anchor,
                    0.,
                    mid.count,
                    &mut HashSet::new(),
                );
                let half = (1. - anchor) * length;
                self.transition_end(
                    e,
                    mid.pos,
                    mid.pos + half,
                    half,
                    anchor,
                    1.,
                    mid.count,
                    &mut HashSet::new(),
                );
            }
        }
        // Each pair has one owner. Move all ends there before changing graph topology.
        let edge_ids = self.graph.edge_ids();
        for &e in &edge_ids {
            if !self.graph.edges[e].alive {
                continue;
            }
            let twin = self.graph.edges[e].twin;
            if twin < e {
                continue;
            }
            let length = self.graph.len(e);
            let ends = std::mem::take(&mut self.graph.edges[twin].ends);
            self.graph.edges[e]
                .ends
                .extend(ends.into_iter().map(|end| End {
                    pos: length - end.pos,
                    ..end
                }));
        }
        for &e in &edge_ids {
            let mut ends = std::mem::take(&mut self.graph.edges[e].ends);
            if ends.is_empty() {
                continue;
            }
            ends.sort_by(|a, b| a.pos.total_cmp(&b.pos));
            let edge = self.graph.edges[e].clone();
            let from = self.graph.nodes[edge.from].p;
            let to = self.graph.nodes[edge.to].p;
            let length = self.graph.len(e);
            if length == 0. {
                continue;
            }
            let mut last = e;
            for end in ends {
                let count = (end.count + usize::from(!end.lower)) as i32;
                let close = if end.pos < length / 2. {
                    edge.from
                } else {
                    edge.to
                };
                if (end.pos < 20_000. || end.pos > length - 20_000.)
                    && self.graph.nodes[close].count == count
                {
                    self.graph.nodes[close].ratio = 0.;
                    continue;
                }
                let p = lerp(from, to, (end.pos / length).clamp(0., 1.));
                // Coincident inserted nodes make zero-length cyclic fans; reuse the existing node.
                let last_from = self.graph.edges[last].from;
                if self.graph.nodes[last_from].p == p {
                    self.graph.nodes[last_from].count = count;
                    self.graph.nodes[last_from].ratio = 0.;
                    continue;
                }
                last = self.graph.insert_node(last, p, count);
            }
        }
        for edge in &mut self.graph.edges {
            edge.mids.clear();
        }
        let edge_ids = self.graph.edge_ids();
        for &e in &edge_ids {
            let edge = &self.graph.edges[e];
            if !edge.alive || !edge.central || self.graph.len(e) < 800_000. {
                continue;
            }
            let a = &self.graph.nodes[edge.from];
            let b = &self.graph.nodes[edge.to];
            let r = self.strategy.min_width / 2.;
            if r <= a.radius || r >= b.radius {
                continue;
            }
            let count = a.count.min(b.count);
            let p = lerp(a.p, b.p, (r - a.radius) / (b.radius - a.radius));
            self.graph.insert_node(e, p, count);
        }
    }
    fn generate_mids(&mut self) {
        for e in self.graph.edge_ids() {
            let edge = &self.graph.edges[e];
            if !edge.alive || !edge.central {
                continue;
            }
            let a = &self.graph.nodes[edge.from];
            let b = &self.graph.nodes[edge.to];
            if a.radius >= b.radius || a.count == b.count {
                continue;
            }
            let mut mids = vec![];
            let length = self.graph.len(e);
            for count in a.count.max(0)..b.count {
                let radius = (self.strategy.transition_thickness(count as usize) / 2.)
                    .trunc()
                    .clamp(a.radius, b.radius);
                mids.push(Mid {
                    pos: (length * (radius - a.radius) / (b.radius - a.radius)).trunc(),
                    count: count as usize,
                    radius,
                });
            }
            self.graph.edges[e].mids = mids;
        }
    }
    fn filter_mids(&mut self) {
        for e in self.graph.edge_ids() {
            if !self.graph.edges[e].alive {
                continue;
            }
            for up in [true, false] {
                let Some(mid) = (if up {
                    self.graph.edges[e].mids.last()
                } else {
                    self.graph.edges[e].mids.first()
                })
                .copied() else {
                    break;
                };
                let start = if up { e } else { self.graph.edges[e].twin };
                let traveled = if up {
                    self.graph.len(e) - mid.pos
                } else {
                    mid.pos
                };
                let refs = self.nearby(start, mid, traveled, up, &mut HashSet::new());
                let mut dissolve = !refs.is_empty();
                let from = mid.count as i32 + i32::from(up);
                let to = mid.count as i32 + i32::from(!up);
                for (edge, index) in refs.into_iter().rev() {
                    self.dissolve_region(start, from, to);
                    if index < self.graph.edges[edge].mids.len() {
                        self.graph.edges[edge].mids.remove(index);
                    }
                }
                let half = (if up {
                    1. - self.strategy.anchor(mid.count)
                } else {
                    self.strategy.anchor(mid.count)
                }) * self.strategy.length(mid.count);
                dissolve |= self.filter_end(start, traveled, half, to, &mut HashSet::new());
                if dissolve {
                    let mids = &mut self.graph.edges[e].mids;
                    if let Some(i) = mids
                        .iter()
                        .position(|m| m.count == mid.count && m.pos == mid.pos)
                    {
                        mids.remove(i);
                    }
                }
            }
        }
    }
    fn nearby(
        &self,
        e: usize,
        origin: Mid,
        traveled: f64,
        up: bool,
        seen: &mut HashSet<usize>,
    ) -> Vec<(usize, usize)> {
        if traveled > 100_000_000. || !seen.insert(e) {
            return vec![];
        }
        let mut out = vec![];
        for next in self.graph.beyond(e) {
            if !self.graph.edges[next].central {
                continue;
            }
            let edge = &self.graph.edges[next];
            let aligned = self.graph.upward(next);
            let owner = if aligned { next } else { edge.twin };
            let length = self.graph.len(next);
            let deviation = (origin.radius - self.graph.nodes[edge.from].radius).abs() * 2.
                / if (origin.count % 2 == 1) == up {
                    1.
                } else {
                    2.
                };
            if deviation > self.filter_deviation {
                out.clear();
                break;
            }
            let mut found = false;
            for (i, mid) in self.graph.edges[owner].mids.iter().enumerate() {
                let pos = if aligned { mid.pos } else { length - mid.pos };
                if traveled + pos < 100_000_000. && mid.count == origin.count {
                    out.push((owner, i));
                    found = true;
                }
            }
            if !found {
                let more = self.nearby(next, origin, traveled + length, up, seen);
                if more.is_empty() {
                    out.clear();
                    break;
                }
                out.extend(more);
            }
        }
        seen.remove(&e);
        out.sort_unstable();
        out.dedup();
        out
    }
    fn dissolve_region(&mut self, e: usize, from: i32, to: i32) {
        let n = self.graph.edges[e].to;
        if self.graph.nodes[n].count != from {
            return;
        }
        self.graph.nodes[n].count = to;
        for next in self.graph.beyond(e) {
            if self.graph.edges[next].central {
                self.dissolve_region(next, from, to);
            }
        }
    }
    fn filter_end(
        &mut self,
        e: usize,
        traveled: f64,
        max: f64,
        replacing: i32,
        seen: &mut HashSet<usize>,
    ) -> bool {
        if traveled > max || !seen.insert(e) {
            return false;
        }
        let next: Vec<_> = self
            .graph
            .beyond(e)
            .into_iter()
            .filter(|&x| self.graph.edges[x].central)
            .collect();
        let mut dissolve = next.is_empty() && traveled < max;
        for next in next {
            dissolve |=
                self.filter_end(next, traveled + self.graph.len(next), max, replacing, seen);
        }
        if dissolve {
            self.graph.nodes[self.graph.edges[e].to].count = replacing;
        }
        seen.remove(&e);
        dissolve
    }
    #[allow(clippy::too_many_arguments)]
    fn transition_end(
        &mut self,
        e: usize,
        start: f64,
        end: f64,
        half: f64,
        start_rest: f64,
        end_rest: f64,
        count: usize,
        seen: &mut HashSet<usize>,
    ) -> bool {
        if !self.graph.edges[e].central || !seen.insert(e) {
            return false;
        }
        let length = self.graph.len(e);
        let up = end_rest > start_rest;
        let result = if end > length {
            let rest =
                (end_rest - (start_rest - end_rest) * (end - length) / (start - end)).clamp(0., 1.);
            let next: Vec<_> = self
                .graph
                .beyond(e)
                .into_iter()
                .filter(|&x| self.graph.edges[x].central)
                .collect();
            let multiple = next.len() > 1;
            let mut only_down = true;
            let mut recursed = false;
            for next in next {
                if multiple
                    && up
                    && self.going_down(next, 0., end - length + half, count, &mut HashSet::new())
                {
                    continue;
                }
                only_down &=
                    self.transition_end(next, 0., end - length, half, rest, end_rest, count, seen);
                recursed = true;
            }
            if !up || (recursed && !only_down) {
                let n = self.graph.edges[e].to;
                self.graph.nodes[n].ratio = rest;
                self.graph.nodes[n].count = count as i32;
            }
            only_down
        } else {
            let upward = self.graph.upward(e);
            let owner = if upward { e } else { self.graph.edges[e].twin };
            self.graph.edges[owner].ends.push(End {
                pos: if upward { end } else { length - end },
                count,
                lower: end_rest == 0.,
            });
            false
        };
        seen.remove(&e);
        result
    }
    fn going_down(
        &self,
        e: usize,
        traveled: f64,
        max: f64,
        count: usize,
        seen: &mut HashSet<usize>,
    ) -> bool {
        if !seen.insert(e) {
            return false;
        }
        let edge = &self.graph.edges[e];
        let to = &self.graph.nodes[edge.to];
        if to.radius == 0. {
            return true;
        }
        if to.count > count as i32 + 1 {
            return false;
        }
        let upward = to.radius >= self.graph.nodes[edge.from].radius;
        let owner = if upward { e } else { edge.twin };
        let length = self.graph.len(e);
        if let Some(mid) = if upward {
            self.graph.edges[owner].mids.first()
        } else {
            self.graph.edges[owner].mids.last()
        } {
            let pos = if upward { mid.pos } else { length - mid.pos };
            if mid.count == count && pos + traveled < max {
                return true;
            }
        }
        if traveled + length > max {
            return false;
        }
        if to.count <= count as i32 && !(to.count == count as i32 && to.ratio > 0.) {
            return true;
        }
        let next: Vec<_> = self
            .graph
            .beyond(e)
            .into_iter()
            .filter(|&x| self.graph.edges[x].central)
            .collect();
        let result = !next.is_empty()
            && next
                .into_iter()
                .all(|x| self.going_down(x, traveled + length, max, count, seen));
        seen.remove(&e);
        result
    }
}
