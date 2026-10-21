// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/slic3r/Utils/UndoRedo.cpp :: StackImpl::StackImpl, StackImpl::release_least_recently_used
// Changes: Only the memory policy is taken: a byte limit of min(RAM / 10, 1 GiB), here a fixed 1 GiB, over what the history alone keeps alive, enforced by dropping the oldest snapshots; Mica's stack stores whole Arc-sharing snapshots, not per-object histories.

//! A snapshot undo stack with gestures. Each entry is a labelled copy of the editable state
//! taken *before* a change; redo stores the state that was undone. Snapshots are cheap because
//! meshes are shared (`Arc`) and settings are small maps.
//!
//! A **gesture** is a continuous change (a gizmo drag, a slider drag, a typed edit): the state
//! is captured once when it begins and committed as one entry when it ends, provided
//! something changed. Cancelling a gesture hands the start state back and records nothing.
//!
//! Besides its entry count, the history is bounded in memory like PrusaSlicer's
//! (`release_least_recently_used`): meshes and paintings that only old snapshots still share
//! (a cut, a split or a paint stroke replaces them) count against [`MEMORY_LIMIT`], and the
//! oldest entries go first.

use std::collections::HashSet;

/// Bytes the undo and redo history may keep alive beyond the current scene. PrusaSlicer's
/// stack allows `min(total_physical_memory / 10, 1 GiB)`.
pub const MEMORY_LIMIT: usize = 1 << 30;

/// Heap a state keeps alive. Shared buffers are counted once: an implementation adds a
/// buffer's bytes only when `seen` did not hold its address yet.
pub trait Retained {
    fn retained(&self, seen: &mut HashSet<usize>) -> usize;
}

#[derive(Clone, Debug)]
pub struct Entry<T> {
    pub label: String,
    pub state: T,
}

#[derive(Debug)]
pub struct Undo<T> {
    past: Vec<Entry<T>>,
    future: Vec<Entry<T>>,
    limit: usize,
    /// The gesture in progress: label and the state before it began.
    pending: Option<Entry<T>>,
}

impl<T: Clone + PartialEq> Default for Undo<T> {
    fn default() -> Self {
        Self::new(100)
    }
}

impl<T: Clone + PartialEq> Undo<T> {
    pub fn new(limit: usize) -> Self {
        Self {
            past: Vec::new(),
            future: Vec::new(),
            limit: limit.max(1),
            pending: None,
        }
    }

    fn push_entry(&mut self, entry: Entry<T>) {
        self.future.clear();
        self.past.push(entry);
        if self.past.len() > self.limit {
            self.past.remove(0);
        }
    }

    /// Records `before` as the state to return to when undoing `label`: one discrete step.
    /// A gesture still in progress is committed first, with `before` as its end state.
    pub fn push(&mut self, label: impl Into<String>, before: T) {
        self.end(&before);
        self.push_entry(Entry {
            label: label.into(),
            state: before,
        });
    }

    /// Starts a gesture from `start`. A second `begin` while one is pending is ignored, so
    /// several widgets can call it defensively during the same drag.
    pub fn begin(&mut self, label: impl Into<String>, start: T) {
        if self.pending.is_none() {
            self.pending = Some(Entry {
                label: label.into(),
                state: start,
            });
        }
    }

    pub fn in_gesture(&self) -> bool {
        self.pending.is_some()
    }

    /// Ends the gesture: one entry if `current` differs from the start, nothing otherwise.
    /// Returns whether an entry was recorded.
    pub fn end(&mut self, current: &T) -> bool {
        match self.pending.take() {
            Some(p) if p.state != *current => {
                self.push_entry(p);
                true
            }
            _ => false,
        }
    }

    /// Abandons the gesture and returns the state it started from, to be restored.
    pub fn cancel(&mut self) -> Option<T> {
        self.pending.take().map(|p| p.state)
    }

    /// Returns the state to restore, storing `current` for redo.
    pub fn undo(&mut self, current: T) -> Option<T> {
        self.end(&current);
        let entry = self.past.pop()?;
        self.future.push(Entry {
            label: entry.label,
            state: current,
        });
        Some(entry.state)
    }

    pub fn redo(&mut self, current: T) -> Option<T> {
        self.end(&current);
        let entry = self.future.pop()?;
        self.past.push(Entry {
            label: entry.label,
            state: current,
        });
        Some(entry.state)
    }

    pub fn can_undo(&self) -> bool {
        !self.past.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.future.is_empty()
    }

    pub fn undo_label(&self) -> Option<&str> {
        self.past.last().map(|e| e.label.as_str())
    }

    pub fn redo_label(&self) -> Option<&str> {
        self.future.last().map(|e| e.label.as_str())
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.past.len()
    }
}

impl<T: Clone + PartialEq + Retained> Undo<T> {
    /// Drops the oldest undo entries while the history keeps more than `limit` bytes alive
    /// beyond `current` (what the scene shares costs the history nothing). Redo entries and
    /// the gesture in progress are counted first, then undo entries newest first, so an old
    /// entry pays for the buffers only it still holds. The newest undo entry always stays.
    /// Returns how many entries were dropped.
    pub fn trim_memory(&mut self, current: &T, limit: usize) -> usize {
        let mut seen = HashSet::new();
        current.retained(&mut seen);
        let mut total = 0usize;
        for e in self.future.iter().chain(&self.pending) {
            total = total.saturating_add(e.state.retained(&mut seen));
        }
        // Undo entries newest first; keep everything up to the first one past the limit.
        let mut keep = 0;
        for e in self.past.iter().rev() {
            total = total.saturating_add(e.state.retained(&mut seen));
            if total > limit && keep > 0 {
                break;
            }
            keep += 1;
        }
        let drop = self.past.len() - keep;
        self.past.drain(..drop);
        drop
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{Delta, Object};
    use glam::Vec3;

    #[test]
    fn undo_redo_and_limit() {
        let mut u: Undo<i32> = Undo::new(3);
        let mut state = 0;
        for next in 1..=5 {
            u.push(format!("set {next}"), state);
            state = next;
        }
        assert_eq!(u.undo_label(), Some("set 5"));
        assert_eq!(state, 5);
        state = u.undo(state).unwrap();
        assert_eq!(state, 4);
        state = u.undo(state).unwrap();
        assert_eq!(state, 3);
        assert_eq!(u.redo_label(), Some("set 4"));
        state = u.redo(state).unwrap();
        assert_eq!(state, 4);
        // Only three entries were kept, one of them has been redone.
        state = u.undo(state).unwrap();
        state = u.undo(state).unwrap();
        assert_eq!(state, 2);
        assert!(!u.can_undo());
        assert!(u.can_redo());
        // A new edit clears redo history.
        u.push("set 9", state);
        assert!(!u.can_redo());
    }

    /// A state that shares one buffer per value, like meshes behind `Arc`s.
    #[derive(Clone, PartialEq)]
    struct Mesh(std::sync::Arc<Vec<u8>>);
    impl Retained for Mesh {
        fn retained(&self, seen: &mut HashSet<usize>) -> usize {
            if seen.insert(std::sync::Arc::as_ptr(&self.0) as usize) {
                self.0.len()
            } else {
                0
            }
        }
    }

    #[test]
    fn history_memory_is_bounded_and_shared_buffers_count_once() {
        let mesh = |n| Mesh(std::sync::Arc::new(vec![0; n]));
        let mut u: Undo<Mesh> = Undo::default();
        let mut state = mesh(100);
        // Ten edits that each replace the mesh (a cut): ten 100-byte buffers only the history holds.
        for i in 0..10 {
            u.push(format!("cut {i}"), state.clone());
            state = mesh(100);
        }
        assert_eq!(u.trim_memory(&state, 1_000), 0);
        assert_eq!(u.trim_memory(&state, 450), 6, "the oldest six go");
        assert_eq!(u.undo_label(), Some("cut 9"));
        // Edits that keep sharing the scene's buffer cost nothing.
        for i in 0..20 {
            u.push(format!("move {i}"), state.clone());
        }
        assert_eq!(u.trim_memory(&state, 450), 0);
        // The newest entry stays even when it alone is over the limit.
        u.push("huge", mesh(10_000));
        let before = u.len();
        assert_eq!(u.trim_memory(&state, 450), before - 1);
        assert_eq!(u.len(), 1);
        assert_eq!(u.undo_label(), Some("huge"));
    }

    #[test]
    fn multi_frame_drag_is_one_entry() {
        let mut u: Undo<f32> = Undo::default();
        let mut x = 0.;
        u.begin("Move", x);
        for step in 1..=30 {
            // Every frame of the drag changes the state; none of them records anything.
            x = step as f32 * 0.37;
            u.begin("Move", x); // defensive re-begin from another widget: ignored
            assert!(!u.can_undo());
        }
        assert!(u.in_gesture());
        assert!(u.end(&x));
        assert!(!u.in_gesture());
        assert_eq!(u.len(), 1);
        assert_eq!(u.undo_label(), Some("Move"));
        assert_eq!(u.undo(x), Some(0.));
        assert_eq!(u.redo(0.), Some(x));
        // A gesture that changes nothing records nothing.
        u.begin("Move", x);
        assert!(!u.end(&x));
        assert_eq!(u.len(), 1);
    }

    #[test]
    fn cancelled_drag_records_nothing_and_returns_start() {
        let mut u: Undo<f32> = Undo::default();
        u.begin("Rotate", 10.);
        let mut x = 10.;
        for _ in 0..8 {
            x += 5.;
        }
        assert_eq!(u.cancel(), Some(10.));
        assert!(!u.in_gesture());
        assert!(!u.can_undo());
        assert_eq!(u.cancel(), None);
        assert_eq!(u.undo(x), None);
    }

    #[test]
    fn discrete_push_commits_a_pending_gesture_first() {
        let mut u: Undo<i32> = Undo::default();
        u.begin("Drag", 0);
        // Quick button pressed while a drag was still open: both end up as separate entries.
        u.push("Drop to bed", 5);
        assert_eq!(u.len(), 2);
        assert_eq!(u.undo_label(), Some("Drop to bed"));
        assert_eq!(u.undo(6), Some(5));
        assert_eq!(u.undo_label(), Some("Drag"));
        assert_eq!(u.undo(5), Some(0));
    }

    fn cube() -> Object {
        let mesh = mica_core::mesh::TriangleMesh {
            vertices: vec![
                [0., 0., 0.],
                [10., 0., 0.],
                [10., 10., 0.],
                [0., 10., 0.],
                [0., 0., 10.],
                [10., 0., 10.],
                [10., 10., 10.],
                [0., 10., 10.],
            ],
            faces: vec![
                [0, 2, 1],
                [0, 3, 2],
                [4, 5, 6],
                [4, 6, 7],
                [0, 1, 5],
                [0, 5, 4],
                [1, 2, 6],
                [1, 6, 5],
                [2, 3, 7],
                [2, 7, 6],
                [3, 0, 4],
                [3, 4, 7],
            ],
        };
        Object::new(mica_core::input::InputObject::plain(
            "cube",
            0,
            0,
            mesh,
            mica_core::print_job::ToolAssignments([Some(0); 5]),
            mica_core::config::Config::defaults(),
        ))
    }

    #[test]
    fn rotation_drag_undoes_and_redoes_as_a_whole() {
        let mut u: Undo<Object> = Undo::default();
        let mut o = cube();
        u.begin("Rotate", o.clone());
        // A ring drag arrives as many small increments: 18 × 5° = 90°.
        for _ in 0..18 {
            o.apply(Delta::Rotate {
                axis: Vec3::Z,
                angle: 5f32.to_radians(),
            });
        }
        assert!(u.end(&o));
        assert_eq!(u.len(), 1);
        assert!((o.angle - 90f32.to_radians()).abs() < 1e-4);
        let before = u.undo(o.clone()).unwrap();
        assert_eq!(before.angle, 0.);
        assert_eq!(before, cube_like(&before));
        let after = u.redo(before).unwrap();
        assert!((after.angle - 90f32.to_radians()).abs() < 1e-4);
        assert!(!u.can_redo());
        // Cancelling a second drag restores the 90° state and leaves the history alone.
        let mut o = after.clone();
        u.begin("Rotate", o.clone());
        o.apply(Delta::Rotate {
            axis: Vec3::Z,
            angle: 1.,
        });
        let restored = u.cancel().unwrap();
        assert_eq!(restored, after);
        assert_eq!(u.len(), 1);
    }

    /// The same object with the same transform compares equal (meshes by identity).
    fn cube_like(o: &Object) -> Object {
        o.clone()
    }
}
