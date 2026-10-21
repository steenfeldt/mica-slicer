// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: orca@8500fcdcca src/slic3r/GUI/PartPlate.cpp :: PartPlateList::compute_shape_position, plate_stride_x, plate_stride_y
// Source: orca@8500fcdcca src/slic3r/GUI/PartPlate.hpp :: compute_colum_count
// Changes: Select imported instances before preparation; translate one plate using the archive bed dimensions (selected bed fallback). All retains the legacy global layout. Each keeps every instance and moves each plate to its own bed-local coordinates (unassigned instances join the nearest grid cell) for an editor that shows all plates.

use crate::{config::Config, threemf::Project};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PlateSelection {
    /// Plate 1 for multi-plate archives; retain legacy behavior for a single plate or STL.
    #[default]
    Auto,
    /// One-based position in the archive's plate list (not its arbitrary stored plate ID).
    Plate(usize),
    /// All printable instances, including unassigned ones, at their stored global placement.
    All,
    /// Every plate, each moved to its own bed-local coordinates; instances keep their plate
    /// (unassigned ones are given the plate whose grid cell is nearest). For editors that
    /// show all plates side by side.
    Each,
}

/// Orca's global grid origin of plate `index` among `count` plates: a square grid with a 20 %
/// bed-size gap, rows along negative Y.
pub fn grid_origin(index: usize, count: usize, width: f64, depth: f64) -> [f64; 2] {
    let cols = (count.max(1) as f64).sqrt().ceil() as usize;
    [
        (index % cols) as f64 * width * 1.2,
        -((index / cols) as f64) * depth * 1.2,
    ]
}

/// The plate whose bed centre is nearest to a global position (for unassigned instances,
/// which Orca shows outside every plate; they stay outside the bed they are given).
pub fn nearest_plate(position: [f64; 2], count: usize, width: f64, depth: f64) -> usize {
    (0..count.max(1))
        .min_by(|&a, &b| {
            let distance = |i: usize| {
                let o = grid_origin(i, count, width, depth);
                let (dx, dy) = (
                    position[0] - o[0] - width / 2.,
                    position[1] - o[1] - depth / 2.,
                );
                dx * dx + dy * dy
            };
            distance(a).total_cmp(&distance(b))
        })
        .unwrap_or(0)
}

pub(super) fn select(
    project: &mut Project,
    selection: PlateSelection,
    config: &Config,
    dimensions: Option<(f64, f64)>,
) -> Result<Option<usize>, String> {
    let number = match selection {
        PlateSelection::All => return Ok(None),
        PlateSelection::Auto if project.plates.len() <= 1 => return Ok(None),
        PlateSelection::Auto => 1,
        PlateSelection::Plate(n) => n,
        PlateSelection::Each => {
            if project.plates.is_empty() {
                return Ok(None);
            }
            let (width, depth) = bed_dimensions(config, dimensions)?;
            let count = project.plates.len();
            let ids: Vec<u32> = project.plates.iter().map(|p| p.id).collect();
            for object in &mut project.objects {
                for instance in &mut object.instances {
                    let position = [instance.transform.0[0][3], instance.transform.0[1][3]];
                    let index = match instance
                        .plate
                        .and_then(|id| ids.iter().position(|&p| p == id))
                    {
                        Some(i) => i,
                        None => {
                            let i = nearest_plate(position, count, width, depth);
                            instance.plate = Some(ids[i]);
                            i
                        }
                    };
                    let origin = grid_origin(index, count, width, depth);
                    instance.transform.0[0][3] -= origin[0];
                    instance.transform.0[1][3] -= origin[1];
                }
            }
            return Ok(None);
        }
    };
    let count = project.plates.len().max(1);
    if number == 0 || number > count {
        return Err(format!("plate {number} is outside 1..={count}"));
    }
    if project.plates.is_empty() {
        return Ok(Some(0));
    }
    let index = number - 1;
    let id = project.plates[index].id;
    let (width, depth) = bed_dimensions(config, dimensions)?;
    let origin = grid_origin(index, count, width, depth);
    for object in &mut project.objects {
        for instance in &mut object.instances {
            instance.printable &= instance.plate == Some(id);
            if instance.printable {
                instance.transform.0[0][3] -= origin[0];
                instance.transform.0[1][3] -= origin[1];
            }
        }
    }
    Ok(Some(index))
}

/// The archive's bed size for the plate grid, else the selected printer's `bed_shape` extent.
fn bed_dimensions(config: &Config, dimensions: Option<(f64, f64)>) -> Result<(f64, f64), String> {
    let bed = config.points("bed_shape").unwrap_or_default();
    let extent = |axis: fn(&(f64, f64)) -> f64| {
        bed.iter().map(axis).fold(f64::NEG_INFINITY, f64::max)
            - bed.iter().map(axis).fold(f64::INFINITY, f64::min)
    };
    let (width, depth) = dimensions.unwrap_or_else(|| (extent(|p| p.0), extent(|p| p.1)));
    if !(width.is_finite() && depth.is_finite()) {
        return Err("plate selection needs a finite bed_shape".into());
    }
    Ok((width, depth))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::threemf::{Instance, Object, Plate, ResourceId, Transform};

    fn project() -> Project {
        Project {
            plates: [7, 11, 18, 23, 42]
                .into_iter()
                .map(|id| Plate {
                    id,
                    name: String::new(),
                    instances: Vec::new(),
                    metadata: Default::default(),
                })
                .collect(),
            objects: vec![Object {
                height_ranges: Vec::new(),
                layer_height_profile: Vec::new(),
                brim_points: Vec::new(),
                name: String::new(),
                source: ResourceId {
                    path: String::new(),
                    id: 1,
                },
                parts: Vec::new(),
                settings: Default::default(),
                metadata: Default::default(),
                instances: [Some(7), Some(42), None]
                    .into_iter()
                    .map(|plate| {
                        let mut transform = Transform::IDENTITY;
                        transform.0[0][3] = 334.;
                        transform.0[1][3] = -304.;
                        Instance {
                            transform,
                            plate,
                            printable: true,
                        }
                    })
                    .collect(),
            }],
            ..Project::default()
        }
    }
    fn config() -> Config {
        let mut c = Config::defaults();
        c.set_raw("bed_shape", "0.5x1,270.5x1,270.5x271,0.5x271")
            .unwrap();
        c
    }
    #[test]
    fn explicit_plate_uses_list_order_and_moves_only_selected_instances() {
        let mut p = project();
        assert_eq!(
            select(&mut p, PlateSelection::Plate(5), &config(), None).unwrap(),
            Some(4)
        );
        let instances = &p.objects[0].instances;
        assert_eq!(
            instances.iter().map(|i| i.printable).collect::<Vec<_>>(),
            [false, true, false]
        );
        assert_eq!(instances[1].transform.apply([0.; 3]), [10., 20., 0.]);
        assert_eq!(instances[0].transform.apply([0.; 3]), [334., -304., 0.]);
    }
    #[test]
    fn defaults_legacy_and_invalid_selections() {
        let mut p = project();
        select(&mut p, PlateSelection::Auto, &config(), None).unwrap();
        assert_eq!(
            p.objects[0]
                .instances
                .iter()
                .map(|i| i.printable)
                .collect::<Vec<_>>(),
            [true, false, false]
        );
        let mut p = project();
        select(&mut p, PlateSelection::All, &config(), None).unwrap();
        assert!(p.objects[0].instances.iter().all(|i| i.printable));
        assert!(select(&mut p, PlateSelection::Plate(0), &config(), None).is_err());
        assert!(select(&mut p, PlateSelection::Plate(6), &config(), None).is_err());
        p.plates.clear();
        assert_eq!(
            select(&mut p, PlateSelection::Plate(1), &config(), None).unwrap(),
            Some(0)
        );
        assert!(p.objects[0].instances.iter().all(|i| i.printable));
    }
    #[test]
    fn each_moves_every_plate_to_its_own_bed_and_adopts_unassigned_instances() {
        let mut p = project();
        assert_eq!(
            select(&mut p, PlateSelection::Each, &config(), None).unwrap(),
            None
        );
        let instances = &p.objects[0].instances;
        assert!(instances.iter().all(|i| i.printable));
        // Plate 7 is the first grid cell, plate 42 the fifth (column 1, row 1).
        assert_eq!(instances[0].plate, Some(7));
        assert_eq!(instances[0].transform.apply([0.; 3]), [334., -304., 0.]);
        assert_eq!(instances[1].plate, Some(42));
        assert_eq!(instances[1].transform.apply([0.; 3]), [10., 20., 0.]);
        // The unassigned instance at (334, -304) is nearest to cell (1, 1) too.
        assert_eq!(instances[2].plate, Some(42));
        assert_eq!(instances[2].transform.apply([0.; 3]), [10., 20., 0.]);
        assert_eq!(grid_origin(2, 5, 270., 270.), [648., 0.]);
        assert_eq!(grid_origin(3, 5, 270., 270.), [0., -324.]);
        assert_eq!(nearest_plate([741., -136.], 5, 270., 270.), 2);
        assert_eq!(nearest_plate([-50., 900.], 5, 270., 270.), 0);
        assert_eq!(nearest_plate([900., -900.], 5, 270., 270.), 4);
        assert_eq!(nearest_plate([758., -222.], 5, 270., 270.), 4);
        let mut none = Project::default();
        assert_eq!(
            select(&mut none, PlateSelection::Each, &config(), None).unwrap(),
            None
        );
    }
}
