// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The project's plates, as in OrcaSlicer: a project has several plates, each with its own
//! objects, optional setting overrides and slice result. Object positions are stored in
//! **plate-local** coordinates (the plate's bed at the origin); `Plates::offset` gives the
//! world offset of a plate for drawing them side by side.
//!
//! The viewport consumes this through [`crate::scene::plates::PlateLayout`]:
//! `count`, `current` and `origin(i)`; `plates.offset(i)` is the same origin as a `Vec3`.

use crate::scene::plates::{PlateLayout, grid_origin};
use crate::scene::{Bed, Object};
use crate::worker::Finished;
use glam::{Vec2, Vec3};
use std::collections::BTreeMap;

/// Gap between neighbouring plates: the bed size divided by this (OrcaSlicer's
/// `LOGICAL_PART_PLATE_GAP`, a fifth), far enough apart that each plate reads as its own.
pub const GAP_DIVISOR: f32 = 5.;

static NEXT_PLATE_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

pub struct Plate {
    /// Identity for the life of the plate (unique in the process): a slice job names the
    /// plate it slices by id, so removing or reordering plates can never send its result to
    /// another plate.
    pub id: u64,
    pub name: String,
    pub objects: Vec<Object>,
    /// Per-plate setting overrides (PrusaSlicer option names → text, plus `curr_bed_type`),
    /// applied on top of the global configuration when this plate is sliced; edited in the
    /// sidebar's "This plate" scope (see `plate_settings`).
    pub overrides: BTreeMap<String, String>,
    /// A locked plate keeps its layout: no arrange, no moving or deleting objects.
    pub locked: bool,
    /// A project's raw per-plate metadata (Orca's `filament_maps`, thumbnails…), kept for the
    /// plate settings page and a future project export; not applied.
    pub metadata: BTreeMap<String, String>,
    /// Wipe/prime tower placement loaded from a project, if any.
    pub tower_placement: Option<[f64; 3]>,
    /// A calibration plate: the test is regenerated from these parameters when the plate is
    /// sliced (the objects are its preview), and the card reads the result (see
    /// `crate::calibration`).
    pub calibration: Option<crate::calibration::PlateJob>,
    /// Per-layer custom G-code of this plate: colour changes, pauses, custom and template
    /// G-code, tool switches (PrusaSlicer's `custom_gcode_per_print_z`, Orca's per-plate
    /// `custom_gcode_per_layer`).
    pub custom_gcode: mica_core::custom_gcode::Info,
    /// The last slice of this plate, if its contents have not changed since.
    /// The last slice of this plate, if its contents have not changed since (apart from
    /// filament colours: see `slice_cache`).
    pub finished: Option<Finished>,
    /// What `finished` was sliced from (the slice cache's key).
    pub sliced: Option<std::sync::Arc<crate::slice_cache::Inputs>>,
    /// Totals of the slice before the latest change, to show its consequences.
    pub previous: Option<crate::worker::Summary>,
    /// Why the latest slice of this plate failed; cleared by the next slice or edit. While it
    /// is set the plate shows its error (header, notification, plate label) and no totals.
    pub slice_error: Option<SliceError>,
    /// Counts the plate's invalidations (edits and new slices), so a check can tell whether
    /// anything happened to the plate since a moment it remembers (the monkey test's failed
    /// slices).
    pub epoch: u64,
}

/// A failed slice, as the plate shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct SliceError {
    /// The whole message, as the CLI prints it.
    pub message: String,
    /// The reason in a few words, for the header: "brim doesn't fit on the bed".
    pub short: String,
}

impl SliceError {
    pub fn new(message: String) -> Self {
        let short = short_reason(&message);
        Self { message, short }
    }
}

/// A few words for a slice error: the kind of a skirt or brim that does not fit, else the
/// message's first clause.
pub fn short_reason(message: &str) -> String {
    if message.starts_with("The brim of") {
        return "brim doesn't fit on the bed".into();
    }
    if message.starts_with("The skirt around") {
        return "skirt doesn't fit on the bed".into();
    }
    if message.contains("outside the printable volume") {
        if message.contains("in the excluded area") {
            return "a toolpath enters an excluded area".into();
        }
        return "a toolpath leaves the printable area".into();
    }
    if message.contains("reaches into the excluded area") {
        if message.contains("wipe tower") {
            return "the wipe tower is in an excluded area".into();
        }
        return "an object is in an excluded area".into();
    }
    if message.contains("outside the printer's build volume") {
        return "an object is outside the bed".into();
    }
    let first = message.split(['\n', ';']).next().unwrap_or(message).trim();
    if first.chars().count() > 48 {
        let cut: String = first.chars().take(46).collect();
        format!("{}…", cut.trim_end())
    } else {
        first.to_string()
    }
}

/// Change in time and weight between two slices of a plate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Delta {
    pub seconds: f64,
    pub grams: f64,
}

impl Delta {
    pub fn between(before: &crate::worker::Summary, after: &crate::worker::Summary) -> Self {
        Self {
            seconds: after.time_s - before.time_s,
            grams: after.grams - before.grams,
        }
    }
    /// `+2 min · +0.8 g` style text; `None` when nothing changed noticeably. Grams carry one
    /// decimal while the change is small.
    pub fn text(&self) -> Option<String> {
        let minutes = self.seconds / 60.;
        if minutes.abs() < 0.5 && self.grams.abs() < 0.05 {
            return None;
        }
        let sign = |v: f64| if v >= 0. { "+" } else { "−" };
        let time = if minutes.abs() >= 60. {
            format!("{}{:.1} h", sign(minutes), minutes.abs() / 60.)
        } else if minutes.abs() < 0.5 {
            format!("{}{:.0} s", sign(self.seconds), self.seconds.abs())
        } else {
            format!("{}{:.0} min", sign(minutes), minutes.abs())
        };
        let grams = if self.grams.abs() < 10. {
            format!("{}{:.1} g", sign(self.grams), self.grams.abs())
        } else {
            format!("{}{:.0} g", sign(self.grams), self.grams.abs())
        };
        Some(format!("{time} · {grams}"))
    }
}

impl Plate {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: NEXT_PLATE_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            name: name.into(),
            objects: vec![],
            overrides: BTreeMap::new(),
            locked: false,
            metadata: BTreeMap::new(),
            tower_placement: None,
            calibration: None,
            custom_gcode: Default::default(),
            finished: None,
            sliced: None,
            previous: None,
            slice_error: None,
            epoch: 0,
        }
    }

    /// A plate imported from a project: its own name when it has one, else `Plate N`.
    pub fn imported(index: usize, imported: &mica_core::input::LoadedPlate) -> Self {
        let mut p = Self::new(if imported.name.trim().is_empty() {
            format!("Plate {}", index + 1)
        } else {
            imported.name.trim().to_string()
        });
        p.locked = imported.locked;
        p.metadata = imported.metadata.clone();
        p.overrides = crate::plate_settings::take_overrides(&mut p.metadata);
        p.tower_placement = imported.tower;
        p.custom_gcode = imported.custom_gcode.clone();
        // A calibration plate saved by Mica: its job comes back from Mica's own metadata.
        if let Some(text) = p.metadata.remove(crate::calibration::PLATE_METADATA_KEY) {
            p.calibration = crate::calibration::PlateJob::from_metadata(&text);
        }
        p
    }

    /// The name shown beside the plate number, when it is not just `Plate N`.
    pub fn custom_name(&self, index: usize) -> Option<String> {
        (self.name != default_name(index)).then(|| self.name.clone())
    }

    /// Forgets the slice but keeps its totals for a before/after comparison.
    pub fn invalidate(&mut self) {
        self.sliced = None;
        self.slice_error = None;
        self.epoch += 1;
        if let Some(f) = self.finished.take() {
            self.previous = Some(f.summary);
        }
    }

    /// The consequence of the latest change, once the plate has been re-sliced.
    pub fn delta(&self) -> Option<Delta> {
        Some(Delta::between(
            self.previous.as_ref()?,
            &self.finished.as_ref()?.summary,
        ))
    }
}

/// The name of an unnamed plate at `index`: `Plate 1`, `Plate 2`, …
pub fn default_name(index: usize) -> String {
    format!("Plate {}", index + 1)
}

/// Whether `name` is a plate's number rather than a name of its own (`Plate 3`): such names
/// follow the plate's position when plates are added, removed or reordered.
pub fn is_default_name(name: &str) -> bool {
    name.strip_prefix("Plate ")
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

pub struct Plates {
    pub plates: Vec<Plate>,
    pub current: usize,
    /// Bed size used to space plates; updated when the printer changes.
    pub bed_size: Vec3,
}

impl PlateLayout for Plates {
    fn count(&self) -> usize {
        self.plates.len()
    }
    fn current(&self) -> usize {
        self.current
    }
    fn origin(&self, i: usize) -> Vec2 {
        self.offset(i).truncate()
    }
    fn name(&self, i: usize) -> Option<String> {
        self.plates.get(i).and_then(|p| p.custom_name(i))
    }
    fn locked(&self, i: usize) -> bool {
        self.plates.get(i).is_some_and(|p| p.locked)
    }
    fn objects(&self, i: usize) -> usize {
        self.plates.get(i).map_or(0, |p| p.objects.len())
    }
}

impl Default for Plates {
    fn default() -> Self {
        Self::new(Vec3::new(250., 220., 0.))
    }
}

impl Plates {
    pub fn new(bed_size: Vec3) -> Self {
        Self {
            plates: vec![Plate::new("Plate 1")],
            current: 0,
            bed_size,
        }
    }

    pub fn set_bed(&mut self, bed: &Bed) {
        self.bed_size = bed.max - bed.min;
    }

    pub fn len(&self) -> usize {
        self.plates.len()
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.plates.is_empty()
    }

    /// The index of the plate with this id, if it still exists.
    pub fn index_of(&self, id: u64) -> Option<usize> {
        self.plates.iter().position(|p| p.id == id)
    }

    pub fn plate(&self, i: usize) -> &Plate {
        &self.plates[i]
    }

    pub fn current(&self) -> &Plate {
        &self.plates[self.current]
    }

    pub fn current_mut(&mut self) -> &mut Plate {
        &mut self.plates[self.current]
    }

    /// Plates per row: a near-square grid.
    pub fn columns(&self) -> usize {
        (self.plates.len().max(1) as f32).sqrt().ceil().max(1.) as usize
    }
    /// World offset of plate `i`: plates fill rows left to right, then the next row behind.
    pub fn offset(&self, i: usize) -> Vec3 {
        grid_origin(
            i,
            self.columns(),
            self.bed_size.truncate(),
            self.bed_size.truncate() / GAP_DIVISOR,
        )
        .extend(0.)
    }

    /// Objects of plate `i` moved to world coordinates (for drawing all plates at once).
    #[allow(dead_code)] // viewport API
    pub fn world_objects(&self, i: usize) -> Vec<Object> {
        let offset = self.offset(i);
        self.plates[i]
            .objects
            .iter()
            .cloned()
            .map(|mut o| {
                o.position += offset;
                o
            })
            .collect()
    }

    /// Whether no plate holds anything yet (a fresh window, or everything was cleared).
    pub fn is_blank(&self) -> bool {
        self.plates.iter().all(|p| p.objects.is_empty())
    }

    /// Takes a project's plates in: they replace the plates of a blank scene, otherwise they
    /// are added after the existing ones. Returns the index of the first imported plate.
    pub fn import(&mut self, plates: Vec<Plate>) -> usize {
        if plates.is_empty() {
            return self.current;
        }
        if self.is_blank() {
            self.plates = plates;
            self.current = 0;
            0
        } else {
            let base = self.plates.len();
            self.plates.extend(plates);
            self.renumber();
            base
        }
    }

    pub fn add(&mut self) -> usize {
        self.plates
            .push(Plate::new(default_name(self.plates.len())));
        self.current = self.plates.len() - 1;
        self.current
    }

    /// Removes a plate; the last plate is emptied instead so there is always one.
    pub fn remove(&mut self, i: usize) {
        if self.plates.len() <= 1 {
            self.plates[0] = Plate::new("Plate 1");
            self.current = 0;
            return;
        }
        self.plates.remove(i);
        // A plate before the current one shifts it down; the current plate stays current.
        if i < self.current {
            self.current -= 1;
        }
        self.current = self.current.min(self.plates.len() - 1);
        self.renumber();
    }

    fn renumber(&mut self) {
        for (n, p) in self.plates.iter_mut().enumerate() {
            if is_default_name(&p.name) {
                p.name = default_name(n);
            }
        }
    }

    /// Names plate `i`; an empty (or blank) name makes it `Plate N` again. Returns whether
    /// the name changed.
    pub fn rename(&mut self, i: usize, name: &str) -> bool {
        let Some(p) = self.plates.get_mut(i) else {
            return false;
        };
        let name = name.trim();
        let name = if name.is_empty() {
            default_name(i)
        } else {
            name.to_string()
        };
        if p.name == name {
            return false;
        }
        p.name = name;
        true
    }

    /// The plate's name as a project stores it: empty for an unnamed plate (`Plate N`), as
    /// OrcaSlicer writes `plater_name`, so the number follows the plate's position.
    pub fn stored_name(&self, i: usize) -> String {
        self.plates
            .get(i)
            .and_then(|p| p.custom_name(i))
            .unwrap_or_default()
    }

    /// Moves plate `from` to position `to` with everything on it (objects keep their places
    /// on the plate, settings, lock, tower, slice); the current plate stays current. Unnamed
    /// plates are renumbered by their new positions. Returns whether anything moved.
    pub fn move_plate(&mut self, from: usize, to: usize) -> bool {
        let len = self.plates.len();
        if from >= len || to >= len || from == to {
            return false;
        }
        let current = self.plates[self.current].id;
        let plate = self.plates.remove(from);
        self.plates.insert(to, plate);
        self.current = self.index_of(current).unwrap_or(0);
        self.renumber();
        true
    }

    /// A copy of plate `i` right after it, with independent copies of its objects (fresh
    /// identities; `object_id`s from `next_object_id` on), its settings, lock, tower and
    /// layer G-code; the copy becomes current. Returns its index.
    pub fn duplicate(&mut self, i: usize, mut next_object_id: usize) -> usize {
        let source = &self.plates[i];
        let name = match source.custom_name(i) {
            Some(name) => format!("{name} (copy)"),
            None => default_name(i + 1),
        };
        let mut copy = Plate::new(name);
        let mut ids = BTreeMap::new();
        copy.objects = source
            .objects
            .iter()
            .map(|o| {
                let mut c = o.fresh();
                // Instances of one object stay instances of each other on the copy.
                c.object_id = *ids.entry(o.object_id).or_insert_with(|| {
                    next_object_id += 1;
                    next_object_id - 1
                });
                c
            })
            .collect();
        copy.overrides = source.overrides.clone();
        copy.locked = source.locked;
        copy.metadata = source.metadata.clone();
        copy.tower_placement = source.tower_placement;
        copy.calibration = source.calibration.clone();
        copy.custom_gcode = source.custom_gcode.clone();
        self.plates.insert(i + 1, copy);
        self.current = i + 1;
        self.renumber();
        i + 1
    }

    /// Objects on every plate, with their plate index.
    #[allow(dead_code)] // viewport API
    pub fn all_objects(&self) -> impl Iterator<Item = (usize, &Object)> {
        self.plates
            .iter()
            .enumerate()
            .flat_map(|(i, p)| p.objects.iter().map(move |o| (i, o)))
    }

    /// Clears cached slice results (after a settings change affecting every plate).
    pub fn invalidate_all(&mut self) {
        for p in &mut self.plates {
            p.invalidate();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_and_lifecycle() {
        let mut p = Plates::new(Vec3::new(250., 220., 0.));
        assert_eq!(p.len(), 1);
        assert_eq!(p.offset(0), Vec3::ZERO);
        p.add();
        p.add();
        p.add();
        assert_eq!(p.current, 3);
        assert_eq!(p.columns(), 2);
        assert_eq!(p.offset(1), Vec3::new(300., 0., 0.));
        assert_eq!(p.offset(3), Vec3::new(300., 264., 0.));
        assert_eq!(p.origin(2), Vec2::new(0., 264.));
        assert_eq!(p.count(), 4);
        assert_eq!(p.plate(3).name, "Plate 4");
        p.remove(1);
        assert_eq!(p.len(), 3);
        assert_eq!(p.current, 2);
        assert_eq!(
            p.plates.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
            ["Plate 1", "Plate 2", "Plate 3"]
        );
        p.remove(0);
        p.remove(0);
        p.remove(0);
        assert_eq!(p.len(), 1);
        assert_eq!(p.current, 0);
    }

    #[test]
    fn imported_plates_replace_a_blank_scene_and_follow_a_used_one() {
        use mica_core::input::LoadedPlate;
        let imported = |name: &str, locked: bool| LoadedPlate {
            name: name.into(),
            locked,
            tower: None,
            metadata: Default::default(),
            custom_gcode: Default::default(),
        };
        let mut p = Plates::new(Vec3::new(270., 270., 0.));
        let base = p.import(vec![
            Plate::imported(0, &imported("", false)),
            Plate::imported(1, &imported("Simple Paint Job", true)),
        ]);
        assert_eq!(base, 0);
        assert_eq!(p.len(), 2);
        assert_eq!(p.plate(0).name, "Plate 1");
        assert_eq!(p.name(0), None);
        assert_eq!(p.name(1).as_deref(), Some("Simple Paint Job"));
        assert!(p.locked(1) && !p.locked(0));
        // With something on a plate, a second project goes behind.
        p.plates[0].objects.push(crate::scene::Object::new(
            mica_core::input::InputObject::plain(
                "x",
                0,
                0,
                Default::default(),
                mica_core::print_job::ToolAssignments([Some(0); 5]),
                mica_core::config::Config::defaults(),
            ),
        ));
        let base = p.import(vec![Plate::imported(0, &imported("", false))]);
        assert_eq!(base, 2);
        assert_eq!(p.plate(2).name, "Plate 3");
        assert_eq!(p.name(1).as_deref(), Some("Simple Paint Job"));
    }

    #[test]
    fn rename_move_and_duplicate() {
        let names =
            |p: &Plates| -> Vec<String> { p.plates.iter().map(|p| p.name.clone()).collect() };
        let mut p = Plates::new(Vec3::new(250., 220., 0.));
        p.add();
        p.add();
        assert!(p.rename(1, "  Plate for mom "));
        assert!(!p.rename(1, "Plate for mom"));
        assert_eq!(p.stored_name(0), "");
        assert_eq!(p.stored_name(1), "Plate for mom");
        // A name that merely starts with "Plate " is the user's: renumbering leaves it alone.
        assert!(p.move_plate(1, 0));
        assert_eq!(names(&p), ["Plate for mom", "Plate 2", "Plate 3"]);
        // The current plate (the last one added) stays current wherever it ends up.
        assert_eq!(p.current, 2);
        assert!(p.move_plate(2, 0));
        assert_eq!(p.current, 0);
        assert_eq!(names(&p), ["Plate 1", "Plate for mom", "Plate 3"]);
        assert!(!p.move_plate(0, 0) && !p.move_plate(0, 3));
        // An empty name is the number again.
        assert!(p.rename(1, " "));
        assert_eq!(names(&p), ["Plate 1", "Plate 2", "Plate 3"]);
        p.rename(0, "Bracket");
        p.plates[0].locked = true;
        assert_eq!(p.duplicate(0, 7), 1);
        assert_eq!(p.current, 1);
        assert_eq!(
            names(&p),
            ["Bracket", "Bracket (copy)", "Plate 3", "Plate 4"]
        );
        assert!(p.plate(1).locked);
        assert_ne!(p.plate(1).id, p.plate(0).id);
        assert_eq!(p.duplicate(2, 7), 3);
        assert_eq!(p.plate(3).name, "Plate 4");
        assert_eq!(p.plate(4).name, "Plate 5");
        assert!(
            is_default_name("Plate 12")
                && !is_default_name("Plate ")
                && !is_default_name("Plate 1b")
        );
    }

    #[test]
    fn delta_text() {
        use crate::worker::Summary;
        let before = Summary {
            time_s: 3600.,
            grams: 20.,
            ..Default::default()
        };
        let after = Summary {
            time_s: 3600. + 240.,
            grams: 23.,
            ..Default::default()
        };
        assert_eq!(
            Delta::between(&before, &after).text().as_deref(),
            Some("+4 min · +3.0 g")
        );
        assert_eq!(
            Delta::between(&after, &before).text().as_deref(),
            Some("−4 min · −3.0 g")
        );
        assert_eq!(Delta::between(&before, &before).text(), None);
        let small = Summary {
            time_s: 3600. + 125.,
            grams: 20.8,
            ..Default::default()
        };
        assert_eq!(
            Delta::between(&before, &small).text().as_deref(),
            Some("+2 min · +0.8 g")
        );
        let long = Summary {
            time_s: 3600. * 3.5,
            grams: 35.,
            ..Default::default()
        };
        assert_eq!(
            Delta::between(&before, &long).text().as_deref(),
            Some("+2.5 h · +15 g")
        );
    }
}
