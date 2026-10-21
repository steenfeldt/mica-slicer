// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Several build plates side by side, as OrcaSlicer shows them. The rendering only needs
//! how many plates exist, which one is current and where each plate's bed origin sits;
//! the editor's plate model implements [`PlateLayout`] to drive it.

use super::bed::Bed;
use glam::{Vec2, Vec3};

/// Grid of plates in rows of `columns` with `gap` millimetres between beds: the layout the
/// editor's plate model uses.
pub fn grid_origin(i: usize, columns: usize, bed_size: Vec2, gap: f32) -> Vec2 {
    let columns = columns.max(1);
    let col = (i % columns) as f32;
    let row = (i / columns) as f32;
    Vec2::new(col * (bed_size.x + gap), row * (bed_size.y + gap))
}

pub trait PlateLayout {
    fn count(&self) -> usize;
    fn current(&self) -> usize;
    /// World offset added to bed coordinates for plate `i`.
    fn origin(&self, i: usize) -> Vec2;
    /// The plate's own name (a project's `plater_name`), if it has one besides its number.
    fn name(&self, _i: usize) -> Option<String> {
        None
    }
    /// Whether the plate is locked against edits.
    fn locked(&self, _i: usize) -> bool {
        false
    }
    /// How many objects the plate holds.
    fn objects(&self, _i: usize) -> usize {
        0
    }
}

/// Snapshot handed to the renderer each frame.
#[derive(Clone, Debug, PartialEq)]
pub struct Plate {
    pub index: usize,
    pub origin: Vec2,
    pub current: bool,
    pub name: Option<String>,
    pub locked: bool,
    /// How many objects it holds (the plate list).
    pub objects: usize,
    /// The marker on its label: 0 none, 1 warnings about its objects, 2 a failed slice (set by
    /// the editor, which knows them).
    pub alert: u8,
    /// Its slice status (set by the editor): the plate bar and the plate list show it.
    pub status: crate::plate_batch::PlateStatus,
}
pub fn snapshot(layout: &dyn PlateLayout) -> Vec<Plate> {
    (0..layout.count())
        .map(|i| Plate {
            index: i,
            origin: layout.origin(i),
            current: i == layout.current(),
            name: layout.name(i),
            locked: layout.locked(i),
            objects: layout.objects(i),
            alert: 0,
            status: crate::plate_batch::PlateStatus::Empty,
        })
        .collect()
}
/// World origin of the current plate: where its slice preview is drawn. The G-code (and so the
/// preview's toolpaths) is in bed coordinates; the viewport adds this offset so plate N's
/// preview lands on plate N's bed. Taken from the layout's current position, so reordered or
/// deleted plates move their preview with them. No plates (or none current): the origin.
pub fn current_origin(plates: &[Plate]) -> Vec2 {
    plates
        .iter()
        .find(|p| p.current)
        .map_or(Vec2::ZERO, |p| p.origin)
}
/// World bounds of plate `i` including the printable height.
pub fn plate_bounds(bed: &Bed, origin: Vec2) -> (Vec3, Vec3) {
    (
        bed.min + origin.extend(0.),
        bed.max + origin.extend(bed.height),
    )
}
/// World bounds of all plates together.
pub fn all_bounds(bed: &Bed, plates: &[Plate]) -> (Vec3, Vec3) {
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = -min;
    for p in plates {
        let (a, b) = plate_bounds(bed, p.origin);
        min = min.min(a);
        max = max.max(b);
    }
    if plates.is_empty() {
        plate_bounds(bed, Vec2::ZERO)
    } else {
        (min, max)
    }
}
/// Which plate contains the XY point, if any.
pub fn plate_at(bed: &Bed, plates: &[Plate], p: Vec2) -> Option<usize> {
    plates
        .iter()
        .find(|plate| bed.contains(p - plate.origin))
        .map(|plate| plate.index)
}
/// Where an object with world bounds `min..max` sits relative to the printable volume of
/// **its own** plate (`plate` is a plate index). An object on another plate is checked
/// against that plate's bed, never against the current one.
pub fn placement_on(
    bed: &Bed,
    plates: &[Plate],
    plate: usize,
    min: Vec3,
    max: Vec3,
) -> super::Placement {
    let origin = plates
        .iter()
        .find(|p| p.index == plate)
        .map_or(Vec2::ZERO, |p| p.origin)
        .extend(0.);
    bed.placement(min - origin, max - origin)
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Three;
    impl PlateLayout for Three {
        fn count(&self) -> usize {
            3
        }
        fn current(&self) -> usize {
            1
        }
        fn origin(&self, i: usize) -> Vec2 {
            grid_origin(i, 2, Vec2::new(250., 220.), 20.)
        }
    }
    #[test]
    fn grid_layout_and_lookup() {
        let mut c = mica_core::config::Config::defaults();
        c.set_raw("bed_shape", "0x0,250x0,250x220,0x220").unwrap();
        let bed = Bed::from_config(&c);
        let layout = Three;
        assert_eq!(layout.origin(0), Vec2::ZERO);
        assert_eq!(layout.origin(1), Vec2::new(250. + 20., 0.));
        assert_eq!(layout.origin(2), Vec2::new(0., 220. + 20.));
        let plates = snapshot(&layout);
        assert!(plates[1].current && !plates[0].current);
        assert_eq!(plate_at(&bed, &plates, Vec2::new(300., 10.)), Some(1));
        assert_eq!(plate_at(&bed, &plates, Vec2::new(260., 10.)), None);
        let (min, max) = all_bounds(&bed, &plates);
        assert_eq!(min, Vec3::ZERO);
        assert_eq!(max, Vec3::new(520., 460., bed.height));
    }

    /// An object inside plate 2's bed is Inside when checked against plate 2, and only
    /// looks Outside when wrongly checked against plate 1 (the bug this guards against).
    #[test]
    fn placement_uses_the_objects_own_plate() {
        use crate::scene::Placement;
        let mut c = mica_core::config::Config::defaults();
        c.set_raw("bed_shape", "0x0,250x0,250x220,0x220").unwrap();
        c.set_raw("max_print_height", "250").unwrap();
        let bed = Bed::from_config(&c);
        let plates = snapshot(&Three);
        // World bounds of a 20 mm cube at plate-local (100, 100) on plate 1 (origin 270, 0).
        let min = Vec3::new(370., 100., 0.);
        let max = Vec3::new(390., 120., 20.);
        assert_eq!(placement_on(&bed, &plates, 1, min, max), Placement::Inside);
        assert_eq!(placement_on(&bed, &plates, 0, min, max), Placement::Outside);
        // The same cube on plate 2 (origin 0, 240).
        let min = Vec3::new(100., 340., 0.);
        let max = Vec3::new(120., 360., 20.);
        assert_eq!(placement_on(&bed, &plates, 2, min, max), Placement::Inside);
        // Too tall for the printer, on any plate.
        assert_eq!(
            placement_on(&bed, &plates, 2, min, max.with_z(300.)),
            Placement::Outside
        );
        // An unknown plate index falls back to the first plate's bed.
        assert_eq!(
            placement_on(
                &bed,
                &plates,
                7,
                Vec3::new(10., 10., 0.),
                Vec3::new(30., 30., 10.)
            ),
            Placement::Inside
        );
    }
}
