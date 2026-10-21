// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Completed plate slices, kept by what determines their G-code, so that an undo, a redo or
//! any edit that returns a plate to a state it was sliced in gets that slice back at once
//! instead of slicing again.
//!
//! **The key** is everything a slice job reads ([`Inputs`]): the plate's objects (meshes,
//! transforms, tools, painting, volumes, ranges and their overrides, the variable layer
//! height profile and brim ears; meshes compare by
//! identity, and the entry keeps them alive so an address is never reused for other
//! geometry) with the plate's per-layer custom G-code (colour changes, pauses…), or the
//! calibration test's parameters, and the job's resolved configuration
//! (presets, edits, project colours, the plate's overrides and tower placement). A hash of
//! the cheap parts is the first filter; equality decides.
//!
//! **Colours** do not change toolpaths, only the thumbnails and the colour list in the G-code
//! header: a slice whose inputs differ only in `filament_colour` / `extruder_colour` is still
//! the right preview ([`Fit::Colours`]); Export and Print slice again first (see
//! `App::reslice_for_colours`), unless an exact entry exists.
//!
//! **Memory:** a slice is a temp folder on disk (the preview segments and the G-code) plus a
//! small summary, so entries are bounded by count and by bytes on disk. Slices a plate still
//! shows are always kept (they are alive anyway); of the others the newest
//! [`MAX_SPARE`] stay while their folders total at most [`MAX_SPARE_BYTES`].

use crate::scene::Object;
use crate::worker::Finished;
use mica_core::config::Config;
use std::{
    hash::{Hash, Hasher},
    sync::Arc,
};

/// Slices kept beyond those the plates show.
pub const MAX_SPARE: usize = 4;
/// Disk the spare slices may use together.
pub const MAX_SPARE_BYTES: u64 = 2 << 30;

/// The options that only change how the print looks, not where it goes.
pub const COLOUR_KEYS: [&str; 2] = ["filament_colour", "extruder_colour"];

/// What a slice job prints.
#[derive(Clone, Debug, PartialEq)]
pub enum Job {
    /// The plate's objects and its per-layer custom G-code.
    Objects(Vec<Object>, mica_core::custom_gcode::Info),
    Calibration(mica_core::calib::Parameters),
}

/// Everything that determines a plate's G-code.
#[derive(Clone, Debug)]
pub struct Inputs {
    pub job: Job,
    pub config: Config,
    /// Filter for lookups: the object identities and transforms, without the configuration.
    quick: u64,
}

impl PartialEq for Inputs {
    fn eq(&self, other: &Self) -> bool {
        self.quick == other.quick && self.job == other.job && self.config == other.config
    }
}

impl Job {
    /// The worker's job for these inputs.
    pub fn to_worker(&self) -> crate::worker::Job {
        match self {
            Job::Objects(o, c) => crate::worker::Job::Objects(o.clone(), c.clone()),
            Job::Calibration(p) => crate::worker::Job::Calibration(p.clone()),
        }
    }
}

/// How well a slice fits the current inputs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fit {
    /// The G-code is exactly what slicing now would write.
    Exact,
    /// Same toolpaths; only filament colours differ (thumbnails and header lag).
    Colours,
}

impl Inputs {
    pub fn new(job: Job, config: Config) -> Self {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        match &job {
            Job::Objects(objects, custom) => {
                0u8.hash(&mut h);
                format!("{:?}", custom.mode).hash(&mut h);
                for item in &custom.items {
                    item.print_z.to_bits().hash(&mut h);
                    item.kind.hash(&mut h);
                    item.extruder.hash(&mut h);
                    item.color.hash(&mut h);
                    item.extra.hash(&mut h);
                }
                for o in objects {
                    o.id.hash(&mut h);
                    (Arc::as_ptr(&o.mesh) as usize).hash(&mut h);
                    for v in o.transform().to_cols_array() {
                        v.to_bits().hash(&mut h);
                    }
                    o.painting.fingerprint().hash(&mut h);
                    o.overrides.hash(&mut h);
                    // Variable layer heights and brim ears change the G-code too (equality
                    // compares them as well, through `Object`'s `PartialEq`).
                    for v in &o.layer_profile {
                        v.to_bits().hash(&mut h);
                    }
                    for v in o.brim_ears.iter().flatten() {
                        v.to_bits().hash(&mut h);
                    }
                }
            }
            Job::Calibration(_) => 1u8.hash(&mut h),
        }
        Self {
            job,
            config,
            quick: h.finish(),
        }
    }

    /// How a slice made from `self` fits `now`, if at all.
    /// What differs between two inputs (the monkey's report of a stale slice).
    pub fn difference(&self, now: &Inputs) -> String {
        let mut out = vec![];
        if self.quick != now.quick {
            out.push("object identities or transforms".to_string());
        }
        match (&self.job, &now.job) {
            (Job::Objects(a, ca), Job::Objects(b, cb)) => {
                if a.len() != b.len() {
                    out.push(format!("{} objects -> {}", a.len(), b.len()));
                }
                for (k, (x, y)) in a.iter().zip(b).enumerate() {
                    let d = x.differences(y);
                    if !d.is_empty() {
                        out.push(format!("object {k} ({}): {}", x.name, d.join(", ")));
                    }
                }
                if ca != cb {
                    out.push("custom G-code".to_string());
                }
            }
            (a, b) if a != b => out.push("the job".to_string()),
            _ => {}
        }
        let keys: std::collections::BTreeSet<&str> = self
            .config
            .values()
            .map(|(k, _)| k)
            .chain(now.config.values().map(|(k, _)| k))
            .chain(self.config.extras().map(|(k, _)| k))
            .chain(now.config.extras().map(|(k, _)| k))
            .collect();
        for k in keys {
            let (a, b) = (self.config.raw(k), now.config.raw(k));
            if a != b {
                out.push(format!("{k}: {a:?} -> {b:?}"));
            }
        }
        out.join("; ")
    }

    pub fn fit(&self, now: &Inputs) -> Option<Fit> {
        if self == now {
            return Some(Fit::Exact);
        }
        (self.quick == now.quick
            && self.job == now.job
            && same_but_colours(&self.config, &now.config))
        .then_some(Fit::Colours)
    }
}

/// Whether two configurations differ in nothing but colours.
pub fn same_but_colours(a: &Config, b: &Config) -> bool {
    let mut b = b.clone();
    for key in COLOUR_KEYS {
        match a.raw(key) {
            Some(v) => {
                if b.set_raw(key, &v).is_err() {
                    return false;
                }
            }
            None if b.raw(key).is_some() => return false,
            None => {}
        }
    }
    *a == b
}

struct Entry {
    inputs: Arc<Inputs>,
    finished: Finished,
    bytes: u64,
    /// Insertion / use order (newest highest).
    used: u64,
}

#[derive(Default)]
pub struct SliceCache {
    entries: Vec<Entry>,
    clock: u64,
    /// Lookups answered from the cache (the monkey test and tests read it).
    pub hits: u64,
}

/// Bytes of a slice's temp folder (preview segments, G-code, thumbnails).
fn folder_bytes(f: &Finished) -> u64 {
    std::fs::read_dir(&f.preview.job.path)
        .map(|dir| {
            dir.flatten()
                .filter_map(|e| e.metadata().ok())
                .map(|m| m.len())
                .sum()
        })
        .unwrap_or(0)
}

impl SliceCache {
    /// Remembers a completed slice made from `inputs`.
    pub fn insert(&mut self, inputs: Arc<Inputs>, finished: &Finished) {
        self.clock += 1;
        if let Some(e) = self.entries.iter_mut().find(|e| *e.inputs == *inputs) {
            e.inputs = inputs;
            e.finished = finished.clone();
            e.bytes = folder_bytes(finished);
            e.used = self.clock;
        } else {
            self.entries.push(Entry {
                inputs,
                bytes: folder_bytes(finished),
                finished: finished.clone(),
                used: self.clock,
            });
        }
        self.trim();
    }

    /// The best slice for `now`: an exact one, else one that differs only in colours.
    pub fn find(&mut self, now: &Inputs) -> Option<(Arc<Inputs>, Finished, Fit)> {
        let mut best: Option<(usize, Fit)> = None;
        for (i, e) in self.entries.iter().enumerate() {
            match e.inputs.fit(now) {
                Some(Fit::Exact) => {
                    best = Some((i, Fit::Exact));
                    break;
                }
                Some(Fit::Colours) if best.is_none() => best = Some((i, Fit::Colours)),
                _ => {}
            }
        }
        let (i, fit) = best?;
        self.clock += 1;
        self.hits += 1;
        let e = &mut self.entries[i];
        e.used = self.clock;
        Some((e.inputs.clone(), e.finished.clone(), fit))
    }

    /// Whether any kept slice printed this job (a cheap test before the configuration is
    /// resolved for a full lookup).
    pub fn has_job(&self, job: &Job) -> bool {
        self.entries.iter().any(|e| e.inputs.job == *job)
    }

    /// Drops spare slices beyond the bounds, oldest first. A slice a plate shows holds a
    /// second reference to its preview and is never counted or dropped here.
    pub fn trim(&mut self) {
        let shown = |e: &Entry| Arc::strong_count(&e.finished.preview) > 1;
        let mut spare: Vec<(u64, u64)> = self
            .entries
            .iter()
            .filter(|e| !shown(e))
            .map(|e| (e.used, e.bytes))
            .collect();
        spare.sort_by_key(|a| std::cmp::Reverse(a.0));
        let mut bytes = 0;
        let mut keep = std::collections::HashSet::new();
        for (n, (used, size)) in spare.into_iter().enumerate() {
            bytes += size;
            if n < MAX_SPARE && bytes <= MAX_SPARE_BYTES {
                keep.insert(used);
            }
        }
        self.entries.retain(|e| shown(e) || keep.contains(&e.used));
    }

    /// Forgets everything (a new project).
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// The spare slices' disk use.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn spare_bytes(&self) -> u64 {
        self.entries
            .iter()
            .filter(|e| Arc::strong_count(&e.finished.preview) == 1)
            .map(|e| e.bytes)
            .sum()
    }
}

#[cfg(test)]
mod layer_and_ear_tests {
    use super::*;

    /// A layer height edit or a brim ear is a different slice: undoing it must not bring back
    /// the edited slice, and redoing it must not bring back the plain one.
    #[test]
    fn layer_profile_and_brim_ears_are_part_of_the_key() {
        let o = crate::layer_edit::tests::cube();
        let config = Config::defaults();
        let plain = Inputs::new(
            Job::Objects(vec![o.clone()], Default::default()),
            config.clone(),
        );
        let mut profiled = o.clone();
        profiled.layer_profile = vec![0., 0.2, 10., 0.1, 20., 0.2];
        let mut eared = o.clone();
        eared.brim_ears = vec![[0., 0., -0.0001, 4.]];
        for changed in [profiled, eared] {
            let other = Inputs::new(
                Job::Objects(vec![changed], Default::default()),
                config.clone(),
            );
            assert_eq!(plain.fit(&other), None);
            assert_eq!(other.fit(&plain), None);
        }
        let again = Inputs::new(Job::Objects(vec![o], Default::default()), config);
        assert_eq!(plain.fit(&again), Some(Fit::Exact));
    }
}
