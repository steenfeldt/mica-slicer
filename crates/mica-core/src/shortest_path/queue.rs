// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: prusaslicer@b028299c77 src/libslic3r/MutablePriorityQueue.hpp :: MutablePriorityQueue
// Changes: Heap of item indices; the "less" predicate and index setter act on a caller-owned store passed
// Changes: to every operation. Swap-on-tie sifting and the full-height sift-up are kept, so equal keys
// Changes: leave the heap in the same order as upstream.
// Upstream attribution (MutablePriorityQueue.hpp):
// Copyright (c) Prusa Research 2017 - 2022 Tomáš Mészáros @tamasmeszaros, Vojtěch Bubník @bubnikv, Filip Sykala @Jony01
//
// PrusaSlicer is released under the terms of the AGPLv3 or higher

//! Binary min-heap whose items know their heap position, so they can be removed or updated.

/// `InvalidQueueID`.
pub(crate) const INVALID: usize = usize::MAX;

/// The items a [`Queue`] orders: their keys and their stored heap positions.
pub(crate) trait Items {
    fn less(&self, a: usize, b: usize) -> bool;
    fn set_index(&mut self, item: usize, idx: usize);
}

pub(crate) struct Queue {
    heap: Vec<usize>,
    /// `ResetIndexWhenRemoved`.
    reset: bool,
}

impl Queue {
    pub(crate) fn new(capacity: usize, reset: bool) -> Self {
        Self {
            heap: Vec::with_capacity(capacity),
            reset,
        }
    }

    pub(crate) fn top(&self) -> usize {
        self.heap[0]
    }

    pub(crate) fn push(&mut self, item: usize, d: &mut impl Items) {
        let idx = self.heap.len();
        self.heap.push(item);
        d.set_index(item, idx);
        self.update_heap_up(0, idx, d);
    }

    pub(crate) fn pop(&mut self, d: &mut impl Items) {
        if self.reset {
            d.set_index(self.heap[0], INVALID);
        }
        if self.heap.len() > 1 {
            let last = self.heap.pop().unwrap();
            self.heap[0] = last;
            d.set_index(last, 0);
            let bottom = self.heap.len() - 1;
            self.update_heap_down(0, bottom, d);
        } else {
            self.heap.clear();
        }
    }

    pub(crate) fn remove(&mut self, idx: usize, d: &mut impl Items) {
        if self.reset {
            d.set_index(self.heap[idx], INVALID);
        }
        if idx + 1 == self.heap.len() {
            self.heap.pop();
            return;
        }
        let last = *self.heap.last().unwrap();
        self.heap[idx] = last;
        d.set_index(last, idx);
        self.heap.pop();
        let bottom = self.heap.len() - 1;
        self.update_heap_down(idx, bottom, d);
        self.update_heap_up(0, idx, d);
    }

    pub(crate) fn update(&mut self, idx: usize, d: &mut impl Items) {
        let item = self.heap[idx];
        self.remove(idx, d);
        self.push(item, d);
    }

    fn swap(&mut self, a: usize, b: usize, d: &mut impl Items) {
        let (ia, ib) = (self.heap[a], self.heap[b]);
        d.set_index(ia, b);
        d.set_index(ib, a);
        self.heap[a] = ib;
        self.heap[b] = ia;
    }

    fn update_heap_up(&mut self, top: usize, bottom: usize, d: &mut impl Items) {
        let mut child = bottom;
        while child != 0 {
            let parent = (child - 1) >> 1;
            if parent < top {
                break;
            }
            if !d.less(self.heap[parent], self.heap[child]) {
                self.swap(parent, child, d);
            }
            child = parent;
        }
    }

    fn update_heap_down(&mut self, top: usize, bottom: usize, d: &mut impl Items) {
        let mut parent = top;
        loop {
            let mut child = (parent << 1) + 1;
            if child > bottom {
                break;
            }
            let child2 = child + 1;
            if child2 <= bottom && !d.less(self.heap[child], self.heap[child2]) {
                child = child2;
            }
            if d.less(self.heap[parent], self.heap[child]) {
                return;
            }
            self.swap(parent, child, d);
            parent = child;
        }
    }
}
