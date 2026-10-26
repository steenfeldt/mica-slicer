// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/ExtrusionOrder.cpp :: extract_infill_extrusions, sort_fill_extrusions
// Source: prusaslicer@b028299c77 src/libslic3r/Fill/Fill.cpp :: SurfaceFillParams::operator<, Layer::make_fills
// Changes: Works on one island's fill collections (Mica keeps one region per island here), described by
// Changes: [`FillCollection`] instead of ExtrusionEntityCollection pointers; the collections arrive in
// Changes: the order of their `group_fills` sort keys, approximated by [`FillRank`] (bridge angle,
// Changes: pattern, spacing, density) because Mica does not merge equal surface parameters.
// Source: orca@8500fcdcca src/libslic3r/GCode.cpp :: GCode::extrude_infill, GCode::ObjectByExtruder::Island::Region::append
// Changes: Orca chains one collection per filled ExPolygon (bridges included) of the island in one set.

//! PrusaSlicer's print order of an island's infill: sortable fill collections are flattened into their
//! paths, no-sort collections (monotonic, concentric, ensuring) stay whole, and everything is chained by
//! the shortest-path greedy from the previous position.

use crate::fill::{FillCollection, FillParams, Pattern};
use crate::geometry::Point;
use crate::settings::PrintSettings;
use crate::shortest_path::{Entity, chain_entities};
use crate::toolpath::ExtrusionPath;

/// The position of a fill collection in `group_fills`' `std::set<SurfaceFillParams>`, plus the
/// thin-fill collection `Layer::make_fills` appends after an island's fills.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct FillRank {
    /// 0 bridges (sorted by decreasing bridge angle), 1 other surfaces, 2 thin fills.
    group: u8,
    key: [i64; 4],
}

/// `InfillPattern` enum value (PrintConfig.hpp); patterns PrusaSlicer lacks sort last.
fn pattern_index(p: Pattern) -> i64 {
    match p {
        Pattern::Rectilinear => 0,
        Pattern::Monotonic => 1,
        Pattern::MonotonicLines => 2,
        Pattern::AlignedRectilinear => 3,
        Pattern::Grid => 4,
        Pattern::Triangles => 5,
        Pattern::Stars => 6,
        Pattern::Cubic => 7,
        Pattern::Concentric => 9,
        Pattern::Honeycomb => 10,
        Pattern::Honeycomb3D => 11,
        Pattern::Gyroid => 12,
        Pattern::HilbertCurve => 13,
        Pattern::ArchimedeanChords => 14,
        Pattern::OctagramSpiral => 15,
        Pattern::AdaptiveCubic => 16,
        Pattern::SupportCubic => 17,
        Pattern::SupportBase => 18,
        Pattern::Lightning => 19,
        Pattern::Ensuring => 20,
        Pattern::ZigZag => 21,
        Pattern::CrossHatch => 22,
        Pattern::CrossZag => 23,
        Pattern::LockedZag => 24,
    }
}

fn nano(v: f64) -> i64 {
    (v * 1e9).round() as i64
}

impl FillRank {
    /// A bridge fill with its bridge angle (radians); larger angles come first.
    pub(crate) fn bridge(angle: f64) -> Self {
        Self {
            group: 0,
            key: [-nano(angle), 0, 0, 0],
        }
    }

    /// A surface fill. Internal solid surfaces are sorted before `group_fills` switches them to
    /// the ensuring pattern, i.e. as monotonic (or rectilinear) fills.
    pub(crate) fn surface(pattern: Pattern, s: &PrintSettings, p: &FillParams) -> Self {
        let sorted_as = if pattern == Pattern::Ensuring {
            if matches!(
                s.top_fill_pattern,
                Pattern::Monotonic | Pattern::MonotonicLines
            ) {
                Pattern::Monotonic
            } else {
                Pattern::Rectilinear
            }
        } else {
            pattern
        };
        Self {
            group: 1,
            key: [
                pattern_index(sorted_as),
                nano(p.spacing),
                nano(p.angle),
                nano(p.density),
            ],
        }
    }

    pub(crate) fn thin_fills() -> Self {
        Self {
            group: 2,
            key: [0; 4],
        }
    }
}

fn first(entity: &[ExtrusionPath]) -> Point {
    entity[0].points[0]
}

fn last(entity: &[ExtrusionPath]) -> Point {
    *entity.last().unwrap().points.last().unwrap()
}

/// An entity of a collection: a path (`ExtrusionPath`, or `ExtrusionPathOriented` when clockwise
/// movements are preferred), a multi-path or a closed multi-path (`ExtrusionLoop`).
fn entity(paths: &[ExtrusionPath], multi: bool, prefer_clockwise: bool) -> Entity {
    let (a, b) = (first(paths), last(paths));
    let is_loop = multi && a == b;
    Entity {
        first: a,
        last: b,
        is_loop,
        can_reverse: if multi { !is_loop } else { !prefer_clockwise },
    }
}

fn push_oriented(out: &mut Vec<ExtrusionPath>, mut paths: Vec<ExtrusionPath>, reverse: bool) {
    if reverse {
        paths.reverse();
        for p in &mut paths {
            p.points.reverse();
        }
    }
    out.extend(paths);
}

/// `extract_infill_extrusions` + `sort_fill_extrusions`: the island's fills in print order from
/// `start_near` (the end of the previous extrusion).
pub(crate) fn sort_fill_extrusions(
    collections: Vec<FillCollection>,
    start_near: Point,
    prefer_clockwise: bool,
) -> Vec<ExtrusionPath> {
    let reversible = vec![!prefer_clockwise; collections.len()];
    chain_fills(collections, &reversible, start_near)
}

/// Orca `GCode::extrude_infill`: the island region's fill collections (`Island::Region::append`
/// flattens a sortable collection into its paths and keeps a no-sort one whole) chained by
/// `chain_and_reorder_extrusion_entities` from `start_near`. The flag is the paths' `can_reverse`
/// (false for grid infill, `params.can_reverse`).
pub(crate) fn chain_orca_fills(
    collections: Vec<(FillCollection, bool)>,
    start_near: Point,
) -> Vec<ExtrusionPath> {
    let (collections, reversible): (Vec<_>, Vec<_>) = collections.into_iter().unzip();
    chain_fills(collections, &reversible, start_near)
}

fn chain_fills(
    collections: Vec<FillCollection>,
    reversible: &[bool],
    start_near: Point,
) -> Vec<ExtrusionPath> {
    enum Item {
        Single(usize, usize),
        Whole(usize),
    }
    let mut items = Vec::new();
    let mut entities = Vec::new();
    for (ci, c) in collections.iter().enumerate() {
        if c.entities.is_empty() {
            continue;
        }
        if c.no_sort {
            items.push(Item::Whole(ci));
            entities.push(Entity {
                first: first(&c.entities[0]),
                last: last(c.entities.last().unwrap()),
                is_loop: false,
                can_reverse: false,
            });
        } else {
            // Flatten the infill collection for better path planning.
            for (ei, e) in c.entities.iter().enumerate() {
                items.push(Item::Single(ci, ei));
                entities.push(entity(e, c.multi, !reversible[ci]));
            }
        }
    }
    let order = chain_entities(&entities, Some(start_near), false);
    let multi: Vec<bool> = collections.iter().map(|c| c.multi).collect();
    let mut collections: Vec<Vec<Option<Vec<ExtrusionPath>>>> = collections
        .into_iter()
        .map(|c| c.entities.into_iter().map(Some).collect())
        .collect();
    let mut out = Vec::new();
    for (i, flipped) in order {
        match items[i] {
            Item::Single(ci, ei) => {
                let paths = collections[ci][ei].take().unwrap_or_default();
                push_oriented(&mut out, paths, flipped);
            }
            Item::Whole(ci) => {
                // chain_extrusion_references on a no-sort collection: keep the order, never
                // reverse a loop.
                for e in collections[ci].iter_mut() {
                    let paths = e.take().unwrap_or_default();
                    let is_loop = multi[ci] && first(&paths) == last(&paths);
                    push_oriented(&mut out, paths, flipped && !is_loop);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{flow::Flow, toolpath::Role};

    fn line(a: (f64, f64), b: (f64, f64)) -> ExtrusionPath {
        ExtrusionPath {
            role: Role::SolidInfill,
            perimeter_index: None,
            points: vec![Point::from_mm(a.0, a.1), Point::from_mm(b.0, b.1)],
            flow: Flow::new(0.45, 0.2),
            seam: None,
            overhang: None,
            brick: None,
        }
    }
    fn collection(paths: Vec<ExtrusionPath>, no_sort: bool) -> FillCollection {
        FillCollection {
            entities: paths.into_iter().map(|p| vec![p]).collect(),
            no_sort,
            multi: false,
        }
    }

    #[test]
    fn orca_fills_chain_the_nearest_collection_first() {
        // Two monotonic collections in generation order far, near: Orca starts with the near one.
        let far = collection(
            vec![line((50., 0.), (51., 0.)), line((51., 1.), (50., 1.))],
            true,
        );
        let near = collection(vec![line((1., 0.), (2., 0.))], true);
        let out = chain_orca_fills(vec![(far, true), (near, true)], Point::from_mm(0., 0.));
        let starts: Vec<f64> = out.iter().map(|p| p.points[0].x as f64 / 1e6).collect();
        assert_eq!(starts, [1., 50., 51.]);
    }

    #[test]
    fn orca_grid_paths_keep_their_direction() {
        let paths = || collection(vec![line((10., 0.), (0., 0.))], false);
        let start = Point::from_mm(0., 0.);
        let reversed = chain_orca_fills(vec![(paths(), true)], start);
        assert_eq!(reversed[0].points[0], start);
        let grid = chain_orca_fills(vec![(paths(), false)], start);
        assert_eq!(grid[0].points[0], Point::from_mm(10., 0.));
    }
}
