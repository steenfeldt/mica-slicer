// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: prusaslicer@b028299c77 src/libslic3r/Arachne/SkeletalTrapezoidation.cpp :: generateSegments, propagateBeadingsUpward, propagateBeadingsDownward, generateJunctions, connectJunctions, generateLocalMaximaSingleBeads
// Source: prusaslicer@b028299c77 src/libslic3r/Arachne/utils/PolylineStitcher.hpp :: PolylineStitcher::stitch
// Changes: Arena indices and owned beadings; endpoint grid instead of SparsePointGrid; deterministic traversal order.
// Copyright (c) 2021-2022 Ultimaker B.V.
// CuraEngine is released under the terms of the AGPLv3 or higher.

use super::beading::{Beading, interpolate};
use super::graph::{NONE, Propagation, lerp};
use super::{ExtrusionLine, Junction, Skeleton};
use crate::geometry::Point;
use std::collections::{HashMap, HashSet};

impl Skeleton {
    fn beading(&mut self, n: usize) -> Propagation {
        if let Some(b) = &self.graph.nodes[n].beading {
            return b.clone();
        }
        if self.graph.nodes[n].count < 0 {
            let mut queue: Vec<_> = self
                .graph
                .outgoing(n)
                .into_iter()
                .map(|e| (self.graph.len(e), e))
                .collect();
            let mut seen = HashSet::new();
            for _ in 0..1000 {
                let Some((idx, _)) = queue
                    .iter()
                    .enumerate()
                    .min_by(|(_, a), (_, b)| a.0.total_cmp(&b.0))
                else {
                    break;
                };
                let (dist, e) = queue.swap_remove(idx);
                if dist > 100_000. {
                    break;
                }
                if !seen.insert(e) {
                    continue;
                }
                let to = self.graph.edges[e].to;
                if let Some(b) = &self.graph.nodes[to].beading {
                    return b.clone();
                }
                queue.extend(
                    self.graph
                        .beyond(e)
                        .into_iter()
                        .map(|e| (dist + self.graph.len(e), e)),
                );
            }
            let dist = self
                .graph
                .outgoing(n)
                .iter()
                .map(|&e| self.graph.nodes[self.graph.edges[e].to].radius + self.graph.len(e))
                .min_by(f64::total_cmp)
                .unwrap_or(self.graph.nodes[n].radius);
            self.graph.nodes[n].count = self.strategy.count(dist * 2.) as i32;
        }
        let bead: Propagation = self
            .strategy
            .compute(
                self.graph.nodes[n].radius * 2.,
                self.graph.nodes[n].count.max(0) as usize,
            )
            .into();
        self.graph.nodes[n].beading = Some(bead.clone());
        bead
    }
    pub fn segments(&mut self) -> Vec<ExtrusionLine> {
        let mut upward: Vec<_> = (0..self.graph.edges.len())
            .filter(|&e| {
                let edge = &self.graph.edges[e];
                edge.alive && edge.prev != NONE && edge.next != NONE && self.graph.upward(e)
            })
            .collect();
        upward.sort_by(|&a, &b| {
            let ea = &self.graph.edges[a];
            let eb = &self.graph.edges[b];
            let ra = self.graph.nodes[ea.to].radius;
            let rb = self.graph.nodes[eb.to].radius;
            rb.total_cmp(&ra).then_with(|| {
                let flat_a = self.graph.nodes[ea.from].radius == ra;
                let flat_b = self.graph.nodes[eb.from].radius == rb;
                flat_b.cmp(&flat_a).then_with(|| {
                    if flat_a && flat_b {
                        let dist = |e: usize| {
                            self.graph.dist_up(e).unwrap_or(f64::MAX).min(
                                self.graph
                                    .dist_up(self.graph.edges[e].twin)
                                    .unwrap_or(f64::MAX),
                            ) - self.graph.len(e)
                        };
                        dist(a).total_cmp(&dist(b))
                    } else {
                        a.cmp(&b)
                    }
                })
            })
        });
        for node in &mut self.graph.nodes {
            if !node.alive || node.count <= 0 {
                continue;
            }
            let lower = self.strategy.compute(node.radius * 2., node.count as usize);
            let beading = if node.ratio == 0. {
                lower
            } else {
                interpolate(
                    &lower,
                    1. - node.ratio,
                    &self
                        .strategy
                        .compute(node.radius * 2., node.count as usize + 1),
                )
            };
            node.beading = Some(beading.into());
        }
        for &e in upward.iter().rev() {
            let edge = &self.graph.edges[e];
            let from = edge.from;
            let to = edge.to;
            if self.graph.nodes[to].count >= 0 || self.graph.nodes[to].beading.is_some() {
                continue;
            }
            if let Some(mut lower) = self.graph.nodes[from].beading.clone() {
                lower.bottom += self.graph.len(e);
                lower.upward_only = true;
                self.graph.nodes[to].beading = Some(lower);
            }
        }
        for &edge in &upward {
            if self.graph.edges[edge].central {
                continue;
            }
            let mut e = edge;
            let from = self.graph.edges[e].from;
            let to = self.graph.edges[e].to;
            if self.graph.nodes[from].radius == self.graph.nodes[to].radius
                && self.graph.nodes[from].beading.is_some()
                && self.graph.nodes[to].beading.is_none()
            {
                e = self.graph.edges[e].twin;
            }
            let from = self.graph.edges[e].from;
            let to = self.graph.edges[e].to;
            let length = self.graph.len(e);
            let mut top = self.beading(to);
            let result = if let Some(bottom) = self.graph.nodes[from].beading.clone() {
                let total = top.top + length + bottom.bottom;
                let ratio = if total.min(self.strategy.transition_length) > 0. {
                    bottom.bottom / total.min(self.strategy.transition_length)
                } else {
                    0.
                };
                if ratio >= 1. {
                    top.top += length;
                    top
                } else {
                    let merged = interpolate_switch(
                        &top.beading,
                        ratio,
                        &bottom.beading,
                        self.graph.nodes[from].radius,
                    );
                    Propagation::from(merged)
                }
            } else {
                top.top += length;
                top
            };
            self.graph.nodes[from].beading = Some(result);
        }
        for e in 0..self.graph.edges.len() {
            let edge = &self.graph.edges[e];
            if !edge.alive {
                continue;
            }
            let from = edge.from;
            let to = edge.to;
            let start = self.graph.nodes[to].radius;
            let end = self.graph.nodes[from].radius;
            if end >= start
                || (self.graph.nodes[from].count == self.graph.nodes[to].count
                    && self.graph.nodes[from].count >= 0)
            {
                continue;
            }
            let bead = self.beading(to).beading;
            let n = bead.widths.len();
            if n == 0 {
                continue;
            }
            let a = self.graph.nodes[to].p;
            let b = self.graph.nodes[from].p;
            let mut joints = vec![];
            for i in (0..=(n - 1) / 2).rev() {
                let r = bead.locations[i];
                if r > start + 1. {
                    continue;
                }
                if r < end {
                    break;
                }
                let p = if r > start - 5_000. {
                    a
                } else {
                    lerp(a, b, (r - start) / (end - start))
                };
                joints.push(Junction {
                    p,
                    width: bead.widths[i],
                    perimeter: i,
                });
            }
            self.graph.edges[e].junctions = joints;
        }
        self.connect()
    }
    fn connect(&mut self) -> Vec<ExtrusionLine> {
        let mut output: Vec<ExtrusionLine> = vec![];
        let mut last_for_depth = HashMap::new();
        let mut starts: HashSet<_> = (0..self.graph.edges.len())
            .filter(|&e| self.graph.edges[e].alive && self.graph.edges[e].prev == NONE)
            .collect();
        let mut odd_seen = HashSet::new();
        while let Some(&domain) = starts.iter().min() {
            let mut start = domain;
            let mut new_domain = true;
            loop {
                if !starts.remove(&start) {
                    break;
                }
                let mut chain = vec![start];
                let mut e = start;
                while self.graph.edges[e].next != NONE {
                    e = self.graph.edges[e].next;
                    chain.push(e);
                }
                let end = e;
                let peak_index = chain
                    .iter()
                    .enumerate()
                    .max_by(|&(i, &a), &(j, &b)| {
                        self.graph.nodes[self.graph.edges[a].to]
                            .radius
                            .total_cmp(&self.graph.nodes[self.graph.edges[b].to].radius)
                            .then_with(|| j.cmp(&i))
                    })
                    .map(|(i, _)| i)
                    .unwrap();
                let peak_index = peak_index.min(chain.len().saturating_sub(2));
                let mut from = vec![];
                let mut to = vec![];
                for &e in chain[..=peak_index].iter().rev() {
                    append_junctions(&mut from, &self.graph.edges[e].junctions);
                }
                for &e in &chain[peak_index + 1..] {
                    append_junctions(
                        &mut to,
                        &self.graph.edges[self.graph.edges[e].twin].junctions,
                    );
                }
                let count = from.len().min(to.len());
                for i in 0..count {
                    let from = from[from.len() - 1 - i];
                    let to = to[to.len() - 1 - i];
                    if from.perimeter != to.perimeter || from.p == to.p {
                        continue;
                    }
                    let a = self.graph.edges[start].to;
                    let b = self.graph.edges[end].from;
                    let is_odd = |n: usize, p: Point| {
                        self.graph.nodes[n].count > 0
                            && self.graph.nodes[n].count % 2 == 1
                            && self.graph.nodes[n].ratio == 0.
                            && i + 1 == count
                            && p.distance_to(self.graph.nodes[n].p) < 5_000.
                    };
                    let odd_a = is_odd(a, from.p);
                    let odd_b = is_odd(b, to.p);
                    let odd = odd_a && odd_b;
                    let middle = self.graph.edges[start].next;
                    if odd && odd_seen.contains(&self.graph.edges[middle].twin) {
                        continue;
                    }
                    odd_seen.insert(middle);
                    let mut appended = false;
                    if !new_domain && let Some(&idx) = last_for_depth.get(&from.perimeter) {
                        let line: &mut ExtrusionLine = &mut output[idx];
                        if line.is_odd == odd {
                            let last = *line.junctions.last().unwrap();
                            if !(odd_a && self.graph.multi(a))
                                && last.p.distance_to(from.p) < 10_000.
                                && (last.width - from.width).abs() < 10_000.
                            {
                                line.junctions.push(to);
                                appended = true;
                            } else if odd
                                && !(odd_b && self.graph.multi(b))
                                && last.p.distance_to(to.p) < 10_000.
                                && (last.width - to.width).abs() < 10_000.
                            {
                                line.junctions.push(from);
                                appended = true;
                            }
                        }
                    }
                    if !appended {
                        last_for_depth.insert(from.perimeter, output.len());
                        output.push(ExtrusionLine {
                            inset: from.perimeter,
                            is_odd: odd,
                            is_closed: false,
                            junctions: vec![from, to],
                        });
                    }
                }
                new_domain = false;
                start = self.graph.edges[end].twin;
                if start == domain {
                    break;
                }
            }
        }
        for n in 0..self.graph.nodes.len() {
            let node = &self.graph.nodes[n];
            if !node.alive || !self.graph.local_max(n, true) || self.graph.central_node(n) {
                continue;
            }
            let Some(bead) = &node.beading else {
                continue;
            };
            let bead = &bead.beading;
            if bead.widths.len() % 2 != 1 {
                continue;
            }
            let inset = bead.widths.len() / 2;
            let width = bead.widths[inset];
            let r = width / 8.;
            let junctions = (0..6)
                .map(|i| {
                    let angle = std::f64::consts::TAU * i as f64 / 6.;
                    Junction {
                        p: Point::new(
                            node.p.x + (r * angle.cos()) as i64,
                            node.p.y + (r * angle.sin()) as i64,
                        ),
                        width,
                        perimeter: inset,
                    }
                })
                .collect();
            output.push(ExtrusionLine {
                inset,
                is_odd: true,
                is_closed: false,
                junctions,
            });
        }
        output
    }
}
fn append_junctions(to: &mut Vec<Junction>, from: &[Junction]) {
    if let Some(first) = from.first() {
        while to.last().is_some_and(|j| j.perimeter <= first.perimeter) {
            to.pop();
        }
    }
    to.extend_from_slice(from);
}
fn interpolate_switch(left: &Beading, ratio: f64, right: &Beading, radius: f64) -> Beading {
    let ret = interpolate(left, ratio, right);
    if let Some(i) = left.locations.iter().rposition(|&r| r < radius)
        && i + 1 < left.locations.len()
        && i < right.locations.len()
        && i < ret.locations.len()
        && ret.locations[i] > radius
    {
        let divisor = left.locations[i] - right.locations[i];
        if divisor != 0. {
            return interpolate(
                left,
                ((radius - right.locations[i]) / divisor + 0.1).clamp(0., 1.),
                right,
            );
        }
    }
    ret
}

pub(crate) fn stitch(lines: Vec<ExtrusionLine>, distance: f64) -> Vec<ExtrusionLine> {
    if lines.is_empty() {
        return lines;
    }
    let distance = distance.max(1.);
    let cell = |p: Point| {
        (
            (p.x as f64 / distance).floor() as i64,
            (p.y as f64 / distance).floor() as i64,
        )
    };
    let mut grid: HashMap<(usize, i64, i64), Vec<(usize, bool)>> = HashMap::new();
    for (i, line) in lines.iter().enumerate() {
        for (p, end) in [
            (line.junctions[0].p, false),
            (line.junctions.last().unwrap().p, true),
        ] {
            let (x, y) = cell(p);
            grid.entry((line.inset, x, y)).or_default().push((i, end));
        }
    }
    let mut used = vec![false; lines.len()];
    let mut out = vec![];
    for i in 0..lines.len() {
        if used[i] {
            continue;
        }
        used[i] = true;
        let mut chain = lines[i].clone();
        let mut should_close = chain.is_odd;
        for reversed in [false, true] {
            if reversed {
                chain.junctions.reverse();
            }
            let mut length = chain.length();
            loop {
                let from = chain.junctions.last().unwrap().p;
                let first = chain.junctions[0].p;
                let (x, y) = cell(from);
                let mut closest: Option<(f64, usize, bool, bool)> = None;
                for xx in x - 1..=x + 1 {
                    for yy in y - 1..=y + 1 {
                        if let Some(candidates) = grid.get(&(chain.inset, xx, yy)) {
                            for &(j, end) in candidates {
                                let line = &lines[j];
                                if line.is_odd != chain.is_odd || (!line.is_odd && end != reversed)
                                {
                                    continue;
                                }
                                let p = if end {
                                    line.junctions.last().unwrap().p
                                } else {
                                    line.junctions[0].p
                                };
                                let mut dist = p.distance_to(from);
                                if dist > distance {
                                    continue;
                                }
                                let closing = p.distance_to(first) < 10_000.;
                                if closing {
                                    if length + dist < 3. * distance || chain.junctions.len() <= 2 {
                                        continue;
                                    }
                                    dist += if should_close { -10_000. } else { 10_000. };
                                } else if used[j] {
                                    continue;
                                }
                                if closest.is_none_or(|c| dist < c.0) {
                                    closest = Some((dist, j, end, closing));
                                }
                            }
                        }
                    }
                }
                let Some((_, j, end, closing)) = closest else {
                    break;
                };
                if closing {
                    chain.is_closed = true;
                    break;
                }
                let mut points = lines[j].junctions.clone();
                if end {
                    points.reverse();
                }
                let dist = from.distance_to(points[0].p);
                let start = usize::from(dist < 10_000.);
                let old = chain.junctions.len();
                chain.junctions.extend_from_slice(&points[start..]);
                for k in old..chain.junctions.len() {
                    length += chain.junctions[k - 1].p.distance_to(chain.junctions[k].p);
                }
                used[j] = true;
                should_close &= !lines[j].is_odd;
            }
            if chain.is_closed {
                if reversed {
                    chain.junctions.reverse();
                }
                break;
            }
        }
        if !chain.is_closed && !chain.is_odd {
            chain.junctions.reverse();
        }
        if chain.is_closed && chain.junctions[0].p != chain.junctions.last().unwrap().p {
            chain.junctions.push(chain.junctions[0]);
        }
        let min_width = chain
            .junctions
            .iter()
            .map(|j| j.width)
            .fold(f64::MAX, f64::min);
        // shorterThan includes the closing segment, including for open lines.
        if chain.is_odd
            && !chain.is_closed
            && chain.length()
                + chain.junctions[0]
                    .p
                    .distance_to(chain.junctions.last().unwrap().p)
                < min_width / 2.
        {
            continue;
        }
        out.push(chain);
    }
    out
}
