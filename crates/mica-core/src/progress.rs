// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Progress and cancellation for the long preparation stages. The engine stays GUI-free: a
//! caller hands in a [`Observer`] (a plain trait object, `Sync` so parallel loops can report)
//! and the stages call [`Observer::progress`] with their own fraction (layers done / total)
//! and check [`Observer::cancelled`] between layers, volumes, shells and batches, so a cancel
//! is honoured well before a stage finishes. [`Silent`] is the default for callers that do not
//! care (the CLI, tests); it never cancels, so engine output is unchanged.

use std::sync::atomic::{AtomicUsize, Ordering};

/// The preparation and output stages, in the order a print goes through them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Stage {
    /// Slicing the mesh into layer outlines.
    Slicing,
    /// Modifier, negative and painted volumes: regions and material segmentation.
    Regions,
    /// Walls and infill surface classification (shells, bridges, combined infill).
    Surfaces,
    /// Support contacts, columns or trees.
    Supports,
    /// Seam planning (boundaries, visibility, alignment).
    Seams,
    /// Toolpath generation, per layer batch, while the G-code is written.
    Toolpaths,
    /// Writing and packing the output file.
    Export,
}

impl Stage {
    /// Short lower-case label for progress read-outs ("seam planning").
    pub fn label(self) -> &'static str {
        match self {
            Self::Slicing => "slicing layers",
            Self::Regions => "painting and regions",
            Self::Surfaces => "walls and surfaces",
            Self::Supports => "supports",
            Self::Seams => "seam planning",
            Self::Toolpaths => "toolpaths",
            Self::Export => "export",
        }
    }
}

/// Receives stage progress and answers whether the work should stop.
pub trait Observer: Sync {
    /// `fraction` is the stage's own progress, 0 ..= 1.
    fn progress(&self, stage: Stage, fraction: f32);
    /// Checked between layers, volumes, shells and batches; a `true` ends the preparation
    /// with [`CANCELLED`] as the error text.
    fn cancelled(&self) -> bool {
        false
    }
}

/// Reports nothing and never cancels.
pub struct Silent;
impl Observer for Silent {
    fn progress(&self, _stage: Stage, _fraction: f32) {}
}

/// The error text of a cancelled preparation.
pub const CANCELLED: &str = "Canceled";

/// `Err(CANCELLED)` when the observer asked to stop.
pub fn check(observer: &dyn Observer) -> Result<(), String> {
    if observer.cancelled() {
        Err(CANCELLED.into())
    } else {
        Ok(())
    }
}

/// Counts finished items of one stage from any thread and reports the fraction about every
/// percent (and always at the end), so a parallel per-layer loop costs one atomic increment.
pub struct Ticker<'a> {
    observer: &'a dyn Observer,
    stage: Stage,
    total: usize,
    done: AtomicUsize,
    /// Fractions below `from` and above `to` belong to other parts of the same stage.
    from: f32,
    to: f32,
}

impl<'a> Ticker<'a> {
    pub fn new(observer: &'a dyn Observer, stage: Stage, total: usize) -> Self {
        Self::span(observer, stage, total, 0., 1.)
    }

    /// A ticker for the part `from ..= to` of the stage's progress.
    pub fn span(
        observer: &'a dyn Observer,
        stage: Stage,
        total: usize,
        from: f32,
        to: f32,
    ) -> Self {
        observer.progress(stage, from);
        Self {
            observer,
            stage,
            total: total.max(1),
            done: AtomicUsize::new(0),
            from,
            to,
        }
    }

    /// One item finished.
    pub fn tick(&self) {
        let done = self.done.fetch_add(1, Ordering::Relaxed) + 1;
        let step = (self.total / 100).max(1);
        if done.is_multiple_of(step) || done >= self.total {
            let f = (done as f32 / self.total as f32).min(1.);
            self.observer
                .progress(self.stage, self.from + (self.to - self.from) * f);
        }
    }

    pub fn cancelled(&self) -> bool {
        self.observer.cancelled()
    }

    /// Marks the span complete.
    pub fn finish(&self) {
        self.observer.progress(self.stage, self.to);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::sync::atomic::AtomicBool;

    struct Recorder {
        calls: Mutex<Vec<(Stage, f32)>>,
        stop: AtomicBool,
    }
    impl Observer for Recorder {
        fn progress(&self, stage: Stage, fraction: f32) {
            self.calls.lock().unwrap().push((stage, fraction));
        }
        fn cancelled(&self) -> bool {
            self.stop.load(Ordering::Relaxed)
        }
    }

    #[test]
    fn ticker_reports_about_every_percent_and_the_end() {
        let r = Recorder {
            calls: Mutex::new(vec![]),
            stop: AtomicBool::new(false),
        };
        let t = Ticker::new(&r, Stage::Seams, 1000);
        for _ in 0..1000 {
            t.tick();
        }
        let calls = r.calls.lock().unwrap();
        // The start, then one report per 10 items.
        assert_eq!(calls.len(), 101);
        assert_eq!(calls[0], (Stage::Seams, 0.));
        assert_eq!(calls.last().unwrap(), &(Stage::Seams, 1.));
        assert!(calls.windows(2).all(|w| w[0].1 <= w[1].1));
        drop(calls);
        // Small totals report every item; spans map into their part of the stage.
        r.calls.lock().unwrap().clear();
        let t = Ticker::span(&r, Stage::Supports, 2, 0.5, 1.);
        t.tick();
        t.tick();
        assert_eq!(
            *r.calls.lock().unwrap(),
            vec![
                (Stage::Supports, 0.5),
                (Stage::Supports, 0.75),
                (Stage::Supports, 1.)
            ]
        );
        assert!(check(&r).is_ok());
        r.stop.store(true, Ordering::Relaxed);
        assert!(t.cancelled());
        assert_eq!(check(&r), Err(CANCELLED.to_string()));
        assert!(check(&Silent).is_ok());
        assert_eq!(Stage::Seams.label(), "seam planning");
    }
}
