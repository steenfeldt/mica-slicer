// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: prusaslicer@b028299c77 src/libslic3r/Fill/FillEnsuring.cpp :: filter_vibrating_extrusions, propagate_line_node_remove
// Changes: Index-based graph and queue instead of pointers; preserve upstream propagation rules.
// Copyright (c) Prusa Research 2023 Vojtěch Bubník @bubnikv, Pavel Mikuš @Godrak, Lukáš Hejl @hejllukas
// PrusaSlicer is released under the terms of the AGPLv3 or higher

use crate::geometry::{Point, SCALE};
use std::collections::VecDeque;

#[derive(Clone, Copy, Default)]
struct State {
    skips: usize,
    short: usize,
    touches_long: bool,
    initialized: bool,
}
struct Node {
    line: [Point; 2],
    previous: Vec<usize>,
    next: Vec<usize>,
    removed: bool,
    state: State,
}
impl Node {
    fn short(&self) -> bool {
        self.line[0].distance_to(self.line[1]) < 4. * SCALE
    }
}
fn removable(nodes: &[Node], i: usize) -> bool {
    let n = &nodes[i];
    n.short()
        && (n.state.short > 5
            || (!n.previous.iter().any(|&p| nodes[p].state.touches_long)
                && !n.next.iter().any(|&p| !nodes[p].removed)))
}
pub(super) fn filter(sections: &mut [Vec<[Point; 2]>]) {
    let mut nodes = Vec::<Node>::new();
    let mut ranges = Vec::new();
    for section in sections.iter() {
        let start = nodes.len();
        nodes.extend(section.iter().map(|&line| Node {
            line,
            previous: Vec::new(),
            next: Vec::new(),
            removed: false,
            state: State::default(),
        }));
        ranges.push(start..nodes.len());
    }
    for pair in ranges.windows(2) {
        for i in pair[0].clone() {
            for j in pair[1].clone() {
                if nodes[i].line[0].y <= nodes[j].line[1].y
                    && nodes[j].line[0].y <= nodes[i].line[1].y
                {
                    nodes[i].next.push(j);
                    nodes[j].previous.push(i);
                }
            }
        }
    }
    for initial in 0..ranges.len() {
        for i in ranges[initial].clone() {
            if !nodes[i].removed && nodes[i].short() {
                nodes[i].state = State {
                    short: 1,
                    touches_long: nodes[i]
                        .previous
                        .iter()
                        .any(|&p| !nodes[p].removed && !nodes[p].short()),
                    initialized: true,
                    skips: 0,
                };
            }
        }
        for section in initial..ranges.len() {
            if section + 1 < ranges.len() {
                for i in ranges[section + 1].clone() {
                    nodes[i].state = State::default();
                }
            }
            for i in ranges[section].clone() {
                if nodes[i].removed || !nodes[i].state.initialized {
                    continue;
                }
                let state = nodes[i].state;
                for j in nodes[i].next.clone() {
                    if nodes[j].removed {
                        continue;
                    }
                    let short = nodes[j].short();
                    if !short && state.skips >= 2 {
                        continue;
                    }
                    let next = State {
                        short: state.short + usize::from(short),
                        skips: state.skips + usize::from(!short),
                        touches_long: state.touches_long,
                        initialized: true,
                    };
                    if nodes[j].state.initialized {
                        // This apparently surprising assignment is also in the pinned upstream.
                        nodes[j].state.skips = nodes[j].state.skips.max(next.short).min(next.skips);
                        nodes[j].state.touches_long &= next.touches_long;
                    } else {
                        nodes[j].state = next;
                    }
                }
                if removable(&nodes, i) {
                    nodes[i].removed = true;
                    let mut queue = VecDeque::from(nodes[i].previous.clone());
                    while let Some(p) = queue.pop_front() {
                        if !nodes[p].removed && removable(&nodes, p) {
                            nodes[p].removed = true;
                            queue.extend(nodes[p].previous.iter().copied());
                        }
                    }
                }
            }
        }
    }
    for (section, range) in sections.iter_mut().zip(ranges) {
        *section = range
            .filter(|&i| !nodes[i].removed)
            .map(|i| nodes[i].line)
            .collect();
    }
}
