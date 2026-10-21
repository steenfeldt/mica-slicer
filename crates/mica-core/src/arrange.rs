// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c770b8380ee81c921a2867d522f288123 src/slic3r-arrange/src/NFP/NFP.cpp :: nfp_convex_convex, ifp_convex
// Source: prusaslicer@b028299c770b8380ee81c921a2867d522f288123 src/slic3r-arrange/include/arrange/NFP/Kernels/TMArrangeKernel.hpp :: placement_fitness
// Source: prusaslicer@b028299c770b8380ee81c921a2867d522f288123 src/slic3r-arrange/include/arrange/NFP/PackStrategyNFP.hpp :: pick_best_spot_on_nfp
// Source: prusaslicer@b028299c770b8380ee81c921a2867d522f288123 src/slic3r-arrange-wrapper/include/arrange-wrapper/Scene.hpp :: SceneBuilderBase::build_scene
// Source: prusaslicer@b028299c770b8380ee81c921a2867d522f288123 src/slic3r-arrange-wrapper/src/SceneBuilder.cpp :: get_skirt_inset, brim_offset
// Source: prusaslicer@b028299c770b8380ee81c921a2867d522f288123 src/libslic3r/Print.cpp :: Print::skirt_flow
// Source: prusaslicer@b028299c770b8380ee81c921a2867d522f288123 src/libslic3r/PrintConfig.cpp :: min_object_distance
// Changes: Convex Rust adaptation; translation-space NFPs, Clipper2 free boundaries and deterministic edge search replace NLopt/TBB; separate bed margin, fixed obstacles and bounded spill reporting.
// Copyright (c) Prusa Research 2023 Tomáš Mészáros @tamasmeszaros

//! Convex NFP arrangement adapted from PrusaSlicer's arrange2.
//!
//! Largest hulls are packed first. Obstacle ⊕ (-moving hull) is the no-fit
//! polygon in translation space; subtracting these from the inner-fit polygon
//! gives all free boundary segments. Candidates include vertices, centroid
//! projections, equal-size neighbour alignments and a deterministic scalar
//! search along each segment. Prusa's TMArrangeKernel scores pile-centre
//! distance and neighbour alignment, with small items pulled around big ones.
//!
//! Half the object spacing inflates each collision hull. Bed containment uses
//! the original hull with a separate margin (Prusa's spacing compensation),
//! so an exactly bed-sized object still fits. Forbidden regions are fixed;
//! fixed items never move. Rotations are opt-in. Empty free regions terminate
//! immediately, leaving oversized items unplaced for the caller to report.

use std::f64::consts::{FRAC_PI_2, FRAC_PI_4, PI};

/// Tolerance for "touching" tests (mm). Overlaps smaller than this are ignored.
const EPS: f64 = 1e-9;
/// Grid step (mm) of the fallback scan used when no skyline candidate fits.
const GRID_STEP: f64 = 5.0;
/// Rotations tried when [`Settings::rotations`] is on.
const ROTATIONS: [f64; 4] = [0.0, FRAC_PI_2, FRAC_PI_4, 3.0 * FRAC_PI_4];

/// One object to arrange.
#[derive(Clone, Debug)]
pub struct Item {
    /// Convex hull (or any outline; its convex hull is taken) of the object's XY
    /// footprint in bed coordinates, at its current position.
    pub footprint: Vec<[f64; 2]>,
    /// Point the rotation is applied about (the object's pivot, in the same coordinates).
    pub pivot: [f64; 2],
    /// Items with `fixed = true` stay where they are and others are packed around
    /// them (e.g. a locked object); they still count as obstacles.
    pub fixed: bool,
}

/// The printable area.
#[derive(Clone, Debug)]
pub struct Bed {
    /// Bed outline, CCW or CW, possibly non-rectangular.
    pub polygon: Vec<[f64; 2]>,
    /// Forbidden regions: exclusion zones and the wipe tower footprint. Each is
    /// treated as its convex hull.
    pub forbidden: Vec<Vec<[f64; 2]>>,
}

/// Arrangement parameters.
#[derive(Clone, Debug)]
pub struct Settings {
    /// Minimum gap between object footprints, mm. Half is kept to forbidden regions.
    pub spacing: f64,
    /// Independent clearance to the bed edge, including brim/skirt allowance.
    pub bed_margin: f64,
    /// Try rotated placements (0°, 90°, 45°, 135°) and keep the best.
    pub rotations: bool,
    /// Prefer positions near the bed centre (like PrusaSlicer); `false` packs
    /// towards the bottom-left corner.
    pub align_center: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            spacing: 6.0,
            bed_margin: 0.0,
            rotations: false,
            align_center: true,
        }
    }
}

impl Settings {
    /// Prusa's min_object_distance, with Orca's optional arrange-distance
    /// override. A combined brim/skirt reserves bed margin, not object gap.
    pub fn from_config(c: &crate::config::Config) -> Self {
        let number = |key: &str, default: f64| {
            c.extra(key)
                .and_then(|v| v.parse::<f64>().ok())
                .or_else(|| c.float(key))
                .filter(|v| v.is_finite() && *v >= 0.0)
                .unwrap_or(default)
        };
        let mut spacing = number(
            "bed_arrange_distance",
            number("min_object_distance", number("duplicate_distance", 6.0)),
        );
        // Objects printed one after another keep the print head's clearance apart
        // (min_object_distance; also Orca's "by object").
        if crate::sequential::enabled(c) {
            spacing = spacing.max(crate::sequential::min_object_distance(c));
        }
        let bed_distance = number("min_bed_distance", 0.0);
        let text = |key: &str| c.extra(key).or_else(|| c.string(key));
        let brim = match text("brim_type").unwrap_or("outer_only") {
            "outer_only" | "outer_and_inner" | "auto_brim" | "brim_ears" => {
                number("brim_width", 0.0) + number("brim_separation", 0.0)
            }
            _ => 0.0,
        };
        let skirts = number("skirts", 0.0);
        let has_skirt = number("skirt_height", 1.0) > 0.0 && skirts > 0.0
            || text("draft_shield") == Some("enabled");
        // Same first-layer -> perimeter -> global -> automatic width fallback
        // as Mica's Print::skirt_flow adaptation, resolved without requiring
        // unrelated slicing options to be supported by PrintSettings.
        let height = c
            .abs_value("first_layer_height", number("layer_height", 0.2))
            .unwrap_or(0.2);
        let width = [
            "first_layer_extrusion_width",
            "perimeter_extrusion_width",
            "extrusion_width",
        ]
        .into_iter()
        .filter_map(|key| c.abs_value(key, height))
        .find(|v| v.is_finite() && *v > 0.0)
        .unwrap_or_else(|| 1.125 * number("nozzle_diameter", 0.4));
        let skirt = if has_skirt {
            skirts * width + number("skirt_distance", 6.0)
        } else {
            0.0
        };
        let adhesion_margin = brim + skirt;
        Self {
            spacing,
            bed_margin: bed_distance.max(adhesion_margin),
            ..Self::default()
        }
    }
}

/// Where an item ended up.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    /// Translation applied after the rotation, mm.
    pub translation: [f64; 2],
    /// Rotation about Z in radians, applied about the item's pivot BEFORE the translation.
    pub rotation: f64,
}

impl Placement {
    /// The identity placement: the item stays where it is.
    pub const IDENTITY: Placement = Placement {
        translation: [0.0, 0.0],
        rotation: 0.0,
    };
}

/// Outcome of [`arrange`]. Both lists are sorted by item index.
#[derive(Clone, Debug, Default)]
pub struct Result {
    /// Items that fit, with their placement. Fixed items appear with [`Placement::IDENTITY`].
    pub placed: Vec<(usize, Placement)>,
    /// Items that could not be placed anywhere on the bed.
    pub unplaced: Vec<usize>,
}

/// Applies `placement` to a footprint: rotates about `pivot`, then translates.
pub fn apply_placement(
    footprint: &[[f64; 2]],
    pivot: [f64; 2],
    placement: &Placement,
) -> Vec<[f64; 2]> {
    footprint
        .iter()
        .map(|&p| {
            let r = rotate(p, pivot, placement.rotation);
            [
                r[0] + placement.translation[0],
                r[1] + placement.translation[1],
            ]
        })
        .collect()
}

/// Convex hull of a point set (Andrew's monotone chain), counter-clockwise, no
/// duplicate or collinear vertices. Fewer than three distinct points are
/// returned as they are (a point or a segment).
pub fn convex_hull(points: &[[f64; 2]]) -> Vec<[f64; 2]> {
    let mut pts: Vec<[f64; 2]> = points
        .iter()
        .copied()
        .filter(|p| p[0].is_finite() && p[1].is_finite())
        .collect();
    pts.sort_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
    pts.dedup_by(|a, b| (a[0] - b[0]).abs() <= 1e-12 && (a[1] - b[1]).abs() <= 1e-12);
    if pts.len() < 3 {
        return pts;
    }
    let mut lower: Vec<[f64; 2]> = Vec::with_capacity(pts.len());
    let mut upper: Vec<[f64; 2]> = Vec::with_capacity(pts.len());
    for (&p, &q) in pts.iter().zip(pts.iter().rev()) {
        for (chain, v) in [(&mut lower, p), (&mut upper, q)] {
            while chain.len() >= 2
                && cross(chain[chain.len() - 2], chain[chain.len() - 1], v) <= 1e-12
            {
                chain.pop();
            }
            chain.push(v);
        }
    }
    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

/// Separating-axis overlap test for two convex polygons. Touching (gap 0)
/// counts as not overlapping. Degenerate inputs (points, segments) are handled.
pub fn polygons_overlap(a: &[[f64; 2]], b: &[[f64; 2]]) -> bool {
    sat_overlap(a, [0.0, 0.0], b)
}

/// Packs `items` onto `bed`. See the module docs for the algorithm.
pub fn arrange(items: &[Item], bed: &Bed, settings: &Settings) -> Result {
    let half = (settings.spacing * 0.5).max(0.0);
    let margin = settings.bed_margin.max(0.0);
    let mut result = Result::default();

    let Some(geom) = BedGeom::new(&bed.polygon) else {
        // No usable bed: fixed items stay, nothing else fits.
        for (i, item) in items.iter().enumerate() {
            if item.fixed {
                result.placed.push((i, Placement::IDENTITY));
            } else {
                result.unplaced.push(i);
            }
        }
        return result;
    };

    // Static obstacles: forbidden regions first, then fixed items (inflated).
    let mut statics: Vec<Hull> = bed
        .forbidden
        .iter()
        .map(|poly| convex_hull(poly))
        .filter(|h| !h.is_empty())
        .map(Hull::new)
        .collect();
    let mut fixed_pile: Option<Bbox> = None;
    for (i, item) in items.iter().enumerate() {
        if !item.fixed {
            continue;
        }
        result.placed.push((i, Placement::IDENTITY));
        let hull = convex_hull(&item.footprint);
        if hull.is_empty() {
            continue;
        }
        let h = Hull::new(inflate(&hull, half));
        fixed_pile = Some(fixed_pile.map_or(h.bbox, |p| p.union(&h.bbox)));
        statics.push(h);
    }

    // Movable items, largest hull first (index breaks ties), with their
    // inflated hulls at every rotation to try.
    let angles: &[f64] = if settings.rotations {
        &ROTATIONS
    } else {
        &ROTATIONS[..1]
    };
    let mut movable: Vec<Movable> = items
        .iter()
        .enumerate()
        .filter(|(_, it)| !it.fixed)
        .map(|(index, it)| {
            let hull = convex_hull(&it.footprint);
            let variants = angles
                .iter()
                .filter_map(|&angle| {
                    let rotated: Vec<[f64; 2]> = it
                        .footprint
                        .iter()
                        .map(|&p| rotate(p, it.pivot, angle))
                        .collect();
                    let hull = convex_hull(&rotated);
                    (!hull.is_empty()).then(|| Variant {
                        rotation: angle,
                        hull: Hull::new(inflate(&hull, half)),
                        bed_hull: Hull::new(inflate(&hull, margin)),
                    })
                })
                .collect();
            Movable {
                index,
                area: polygon_area(&hull),
                variants,
            }
        })
        .collect();
    movable.sort_by(|a, b| b.area.total_cmp(&a.area).then(a.index.cmp(&b.index)));

    let mut packed = pack(&movable, &geom, &statics, fixed_pile, settings.align_center);
    if settings.align_center && !packed.unplaced.is_empty() {
        // Growing from the centre can waste the bed's margins; a bottom-left
        // pack is denser, and the pile is centred afterwards anyway.
        let dense = pack(&movable, &geom, &statics, fixed_pile, false);
        if dense.unplaced.len() < packed.unplaced.len() {
            packed = dense;
        }
    }

    // Centre the finished pile on the bed when that keeps everything valid.
    let pile = packed
        .pile
        .filter(|_| settings.align_center && !packed.placed.is_empty());
    if let Some(p) = pile {
        let c = p.centre();
        let delta = [geom.centre[0] - c[0], geom.centre[1] - c[1]];
        if delta[0].abs() > EPS || delta[1].abs() > EPS {
            let valid = packed.placed.iter().all(|(i, pl, h)| {
                let raw = apply_placement(&items[*i].footprint, items[*i].pivot, pl);
                geom.contains(&inflate(&convex_hull(&raw), margin), delta)
                    && !statics.iter().any(|s| h.overlaps(delta, s))
            });
            if valid {
                for (_, pl, _) in &mut packed.placed {
                    pl.translation[0] += delta[0];
                    pl.translation[1] += delta[1];
                }
            }
        }
    }

    result
        .placed
        .extend(packed.placed.into_iter().map(|(i, pl, _)| (i, pl)));
    result.unplaced = packed.unplaced;
    result.placed.sort_by_key(|(i, _)| *i);
    result.unplaced.sort_unstable();
    result
}

// ---------------------------------------------------------------------------
// Internals
// ---------------------------------------------------------------------------

/// A movable item prepared for packing.
struct Movable {
    index: usize,
    area: f64,
    variants: Vec<Variant>,
}

/// Outcome of one packing pass.
struct Packed {
    /// Item index, placement, and the placed inflated hull.
    placed: Vec<(usize, Placement, Hull)>,
    unplaced: Vec<usize>,
    /// Bounding box of everything placed (including fixed items).
    pile: Option<Bbox>,
}

/// Greedy one-pass packing of `movable` (already sorted) around `statics`.
fn pack(
    movable: &[Movable],
    geom: &BedGeom,
    statics: &[Hull],
    fixed_pile: Option<Bbox>,
    align_center: bool,
) -> Packed {
    let mut obstacles: Vec<Hull> = statics.to_vec();
    let mut grid = ObstacleGrid::new(&geom.bbox);
    for (i, o) in obstacles.iter().enumerate() {
        grid.insert(i, &o.bbox);
    }
    let mut pile = fixed_pile;
    let mut packed = Packed {
        placed: Vec::new(),
        unplaced: Vec::new(),
        pile,
    };
    let mut corners: Vec<[f64; 2]> = Vec::new();
    let mut scratch: Vec<Candidate> = Vec::new();
    for (mi, m) in movable.iter().enumerate() {
        if m.variants.is_empty() {
            packed.unplaced.push(m.index);
            continue;
        }
        let mut search = Search {
            geom,
            obstacles: &obstacles,
            grid: &grid,
            pile,
            align_center,
            remaining_ratio: ((movable.len() - mi - 1) as f64 / movable.len() as f64).cbrt(),
            candidates: std::mem::take(&mut scratch),
        };
        for (vi, v) in m.variants.iter().enumerate() {
            corners.clear();
            search.nfp_candidates(vi, v);
        }
        let mut best = search.pick(&m.variants);
        if best.is_none() {
            for (vi, v) in m.variants.iter().enumerate() {
                corners.clear();
                grid_candidates(&mut corners, &v.bed_hull.bbox, geom);
                for &bl in &corners {
                    search.consider_translation(
                        vi,
                        v,
                        [
                            bl[0] - v.bed_hull.bbox.min[0],
                            bl[1] - v.bed_hull.bbox.min[1],
                        ],
                    );
                }
            }
            best = search.pick(&m.variants);
        }
        scratch = search.candidates;
        match best {
            None => packed.unplaced.push(m.index),
            Some(best) => {
                let v = &m.variants[best.variant];
                let placed = v.hull.translated(best.translation);
                pile = Some(pile.map_or(placed.bbox, |p| p.union(&placed.bbox)));
                grid.insert(obstacles.len(), &placed.bbox);
                obstacles.push(placed.clone());
                packed.placed.push((
                    m.index,
                    Placement {
                        translation: best.translation,
                        rotation: v.rotation,
                    },
                    placed,
                ));
            }
        }
    }
    packed.pile = pile;
    packed
}

/// Axis-aligned bounding box.
#[derive(Clone, Copy, Debug)]
struct Bbox {
    min: [f64; 2],
    max: [f64; 2],
}

impl Bbox {
    fn of(points: &[[f64; 2]]) -> Self {
        let mut b = Bbox {
            min: [f64::INFINITY, f64::INFINITY],
            max: [f64::NEG_INFINITY, f64::NEG_INFINITY],
        };
        for p in points {
            b.min[0] = b.min[0].min(p[0]);
            b.min[1] = b.min[1].min(p[1]);
            b.max[0] = b.max[0].max(p[0]);
            b.max[1] = b.max[1].max(p[1]);
        }
        b
    }

    fn translated(&self, t: [f64; 2]) -> Self {
        Bbox {
            min: [self.min[0] + t[0], self.min[1] + t[1]],
            max: [self.max[0] + t[0], self.max[1] + t[1]],
        }
    }

    fn width(&self) -> f64 {
        self.max[0] - self.min[0]
    }

    fn height(&self) -> f64 {
        self.max[1] - self.min[1]
    }

    fn centre(&self) -> [f64; 2] {
        [
            0.5 * (self.min[0] + self.max[0]),
            0.5 * (self.min[1] + self.max[1]),
        ]
    }

    fn union(&self, o: &Bbox) -> Self {
        Bbox {
            min: [self.min[0].min(o.min[0]), self.min[1].min(o.min[1])],
            max: [self.max[0].max(o.max[0]), self.max[1].max(o.max[1])],
        }
    }

    /// Strict overlap: touching boxes do not overlap.
    fn overlaps(&self, o: &Bbox) -> bool {
        self.min[0] < o.max[0] - EPS
            && o.min[0] < self.max[0] - EPS
            && self.min[1] < o.max[1] - EPS
            && o.min[1] < self.max[1] - EPS
    }

    /// `self` lies inside `o` (touching allowed).
    fn within(&self, o: &Bbox) -> bool {
        self.min[0] >= o.min[0] - EPS
            && self.min[1] >= o.min[1] - EPS
            && self.max[0] <= o.max[0] + EPS
            && self.max[1] <= o.max[1] + EPS
    }
}

/// A convex polygon with its bounding box.
#[derive(Clone, Debug)]
struct Hull {
    points: Vec<[f64; 2]>,
    bbox: Bbox,
    area: f64,
    /// Area centroid (vertex mean for degenerate hulls).
    inner: [f64; 2],
}

impl Hull {
    fn new(points: Vec<[f64; 2]>) -> Self {
        let bbox = Bbox::of(&points);
        let n = points.len().max(1) as f64;
        let sum = points
            .iter()
            .fold([0.0, 0.0], |s, p| [s[0] + p[0], s[1] + p[1]]);
        let mut inner = [sum[0] / n, sum[1] / n];
        let mut twice_area = 0.0;
        let mut weighted = [0.0, 0.0];
        for i in 0..points.len() {
            let (a, b) = (points[i], points[(i + 1) % points.len()]);
            let c = a[0] * b[1] - b[0] * a[1];
            twice_area += c;
            weighted[0] += (a[0] + b[0]) * c;
            weighted[1] += (a[1] + b[1]) * c;
        }
        if twice_area.abs() > EPS {
            inner = [
                weighted[0] / (3.0 * twice_area),
                weighted[1] / (3.0 * twice_area),
            ];
        }
        Hull {
            area: polygon_area(&points),
            points,
            bbox,
            inner,
        }
    }

    fn translated(&self, t: [f64; 2]) -> Self {
        Hull {
            area: self.area,
            points: self
                .points
                .iter()
                .map(|p| [p[0] + t[0], p[1] + t[1]])
                .collect(),
            bbox: self.bbox.translated(t),
            inner: [self.inner[0] + t[0], self.inner[1] + t[1]],
        }
    }

    /// Does this hull, translated by `t`, overlap `other`? Bounding boxes are
    /// checked first; then a cheap "one hull's inner point lies strictly inside
    /// the other" test settles the common deep-overlap case before the full
    /// separating-axis test runs.
    fn overlaps(&self, t: [f64; 2], other: &Hull) -> bool {
        if !self.bbox.translated(t).overlaps(&other.bbox) {
            return false;
        }
        let mine = [self.inner[0] + t[0], self.inner[1] + t[1]];
        let theirs = [other.inner[0] - t[0], other.inner[1] - t[1]];
        strictly_inside_convex(mine, &other.points)
            || strictly_inside_convex(theirs, &self.points)
            || sat_overlap(&self.points, t, &other.points)
    }
}

/// Point strictly inside a CCW convex polygon (a proper polygon only; points
/// and segments contain nothing).
fn strictly_inside_convex(p: [f64; 2], poly: &[[f64; 2]]) -> bool {
    let n = poly.len();
    if n < 3 {
        return false;
    }
    (0..n).all(|i| cross(poly[i], poly[(i + 1) % n], p) > EPS)
}

/// Uniform grid over the bed's bounding box indexing obstacles by the cells
/// their bounding boxes cover, so a candidate only tests nearby obstacles.
struct ObstacleGrid {
    origin: [f64; 2],
    inv_cell: [f64; 2],
    nx: usize,
    ny: usize,
    cells: Vec<Vec<usize>>,
}

impl ObstacleGrid {
    const N: usize = 12;

    fn new(bed: &Bbox) -> Self {
        let n = Self::N;
        ObstacleGrid {
            origin: bed.min,
            inv_cell: [
                n as f64 / bed.width().max(EPS),
                n as f64 / bed.height().max(EPS),
            ],
            nx: n,
            ny: n,
            cells: vec![Vec::new(); n * n],
        }
    }

    /// Cell index range `(x0, x1, y0, y1)` (inclusive) covered by `b`, clamped to the grid.
    fn range(&self, b: &Bbox) -> (usize, usize, usize, usize) {
        let cell = |v: f64, axis: usize, n: usize| {
            (((v - self.origin[axis]) * self.inv_cell[axis])
                .floor()
                .max(0.0) as usize)
                .min(n - 1)
        };
        (
            cell(b.min[0], 0, self.nx),
            cell(b.max[0], 0, self.nx),
            cell(b.min[1], 1, self.ny),
            cell(b.max[1], 1, self.ny),
        )
    }

    fn insert(&mut self, index: usize, b: &Bbox) {
        let (x0, x1, y0, y1) = self.range(b);
        for y in y0..=y1 {
            for x in x0..=x1 {
                self.cells[y * self.nx + x].push(index);
            }
        }
    }

    /// True if `pred` holds for any obstacle whose cells `b` touches. An
    /// obstacle spanning several cells may be tested more than once; `pred`
    /// must therefore be pure.
    fn any(&self, b: &Bbox, mut pred: impl FnMut(usize) -> bool) -> bool {
        let (x0, x1, y0, y1) = self.range(b);
        for y in y0..=y1 {
            for x in x0..=x1 {
                if self.cells[y * self.nx + x].iter().any(|&i| pred(i)) {
                    return true;
                }
            }
        }
        false
    }
}

/// One rotation of an item: its inflated hull in the item's original position.
struct Variant {
    rotation: f64,
    hull: Hull,
    bed_hull: Hull,
}

/// Bed outline with precomputed helpers.
struct BedGeom {
    polygon: Vec<[f64; 2]>,
    bbox: Bbox,
    centre: [f64; 2],
    /// The outline is exactly its bounding box, so the bbox test is sufficient.
    is_rect: bool,
    is_convex: bool,
}

impl BedGeom {
    fn new(polygon: &[[f64; 2]]) -> Option<Self> {
        let mut poly: Vec<[f64; 2]> = polygon
            .iter()
            .copied()
            .filter(|p| p[0].is_finite() && p[1].is_finite())
            .collect();
        poly.dedup_by(|a, b| (a[0] - b[0]).abs() <= EPS && (a[1] - b[1]).abs() <= EPS);
        if poly.len() > 1 {
            let (f, l) = (poly[0], poly[poly.len() - 1]);
            if (f[0] - l[0]).abs() <= EPS && (f[1] - l[1]).abs() <= EPS {
                poly.pop();
            }
        }
        if poly.len() < 3 {
            return None;
        }
        let bbox = Bbox::of(&poly);
        if bbox.width() <= EPS || bbox.height() <= EPS {
            return None;
        }
        let is_rect = poly.len() == 4
            && poly.iter().all(|p| {
                ((p[0] - bbox.min[0]).abs() <= EPS || (p[0] - bbox.max[0]).abs() <= EPS)
                    && ((p[1] - bbox.min[1]).abs() <= EPS || (p[1] - bbox.max[1]).abs() <= EPS)
            });
        let signed: f64 = (0..poly.len())
            .map(|i| {
                poly[i][0] * poly[(i + 1) % poly.len()][1]
                    - poly[(i + 1) % poly.len()][0] * poly[i][1]
            })
            .sum();
        if signed.abs() <= EPS {
            return None;
        }
        if signed < 0.0 {
            poly.reverse();
        }
        let is_convex = (0..poly.len()).all(|i| {
            cross(
                poly[i],
                poly[(i + 1) % poly.len()],
                poly[(i + 2) % poly.len()],
            ) >= -EPS
        });
        Some(BedGeom {
            centre: bbox.centre(),
            polygon: poly,
            bbox,
            is_rect,
            is_convex,
        })
    }

    /// `hull` translated by `t` lies inside the bed outline (touching allowed).
    fn contains(&self, hull: &[[f64; 2]], t: [f64; 2]) -> bool {
        if !Bbox::of(hull).translated(t).within(&self.bbox) {
            return false;
        }
        if self.is_rect {
            return true;
        }
        let moved = |p: &[f64; 2]| [p[0] + t[0], p[1] + t[1]];
        if !hull
            .iter()
            .all(|p| point_in_polygon(moved(p), &self.polygon))
        {
            return false;
        }
        // Non-convex outline: every vertex can be inside while an edge still
        // crosses the boundary (across a notch), so check for proper crossings.
        let n = hull.len();
        let m = self.polygon.len();
        for i in 0..n {
            let a = moved(&hull[i]);
            let b = moved(&hull[(i + 1) % n]);
            for j in 0..m {
                if segments_cross(a, b, self.polygon[j], self.polygon[(j + 1) % m]) {
                    return false;
                }
            }
            if !self.is_convex {
                let d = [b[0] - a[0], b[1] - a[1]];
                let axis = usize::from(d[1].abs() > d[0].abs());
                if d[axis].abs() > EPS {
                    let mut cuts = vec![0.0, 1.0];
                    for j in 0..m {
                        if let Some(p) =
                            segment_intersection(a, b, self.polygon[j], self.polygon[(j + 1) % m])
                        {
                            cuts.push(((p[axis] - a[axis]) / d[axis]).clamp(0.0, 1.0));
                        }
                    }
                    cuts.sort_by(f64::total_cmp);
                    for pair in cuts.windows(2) {
                        let t = (pair[0] + pair[1]) * 0.5;
                        if !point_in_polygon([a[0] + t * d[0], a[1] + t * d[1]], &self.polygon) {
                            return false;
                        }
                    }
                }
            }
        }
        true
    }
}

/// A chosen placement: which rotation variant, and where.
#[derive(Clone, Copy)]
struct Best {
    variant: usize,
    translation: [f64; 2],
}

/// A candidate placement with its cost, before validation.
#[derive(Clone, Copy)]
struct Candidate {
    cost: f64,
    variant: usize,
    translation: [f64; 2],
}

/// Candidate evaluation for one item: candidates are collected with their
/// cost, sorted, and validated cheapest-first; the first valid one wins, so
/// the expensive overlap tests only run until a hit.
struct Search<'a> {
    geom: &'a BedGeom,
    obstacles: &'a [Hull],
    grid: &'a ObstacleGrid,
    pile: Option<Bbox>,
    align_center: bool,
    remaining_ratio: f64,
    candidates: Vec<Candidate>,
}

impl Search<'_> {
    fn consider_translation(&mut self, vi: usize, v: &Variant, t: [f64; 2]) {
        if !v.bed_hull.bbox.translated(t).within(&self.geom.bbox) {
            return;
        }
        self.candidates.push(Candidate {
            cost: self.cost(v, t),
            variant: vi,
            translation: t,
        });
    }

    /// Prusa's IFP minus the union of obstacle NFPs. Raw vertices are also
    /// retained: polygon clipping drops zero-area exact-fit slots.
    fn nfp_candidates(&mut self, vi: usize, v: &Variant) {
        let ifp = inner_fit(self.geom, &v.bed_hull);
        if ifp.is_empty() {
            return;
        }
        let mut nfps: Vec<Vec<[f64; 2]>> = self
            .obstacles
            .iter()
            .map(|o| no_fit(&o.points, &v.hull.points))
            .collect();
        if !self.geom.is_rect && !self.geom.is_convex {
            // Crossing a concave bed edge is forbidden. The final containment
            // test discards disconnected regions outside the bed.
            for i in 0..self.geom.polygon.len() {
                nfps.push(no_fit(
                    &[
                        self.geom.polygon[i],
                        self.geom.polygon[(i + 1) % self.geom.polygon.len()],
                    ],
                    &v.bed_hull.points,
                ));
            }
        }
        for poly in std::iter::once(&ifp).chain(nfps.iter()) {
            for &p in poly {
                self.consider_translation(vi, v, p);
            }
        }
        let paths = |polys: &[Vec<[f64; 2]>]| -> clipper2_rust::PathsD {
            polys
                .iter()
                .map(|p| {
                    p.iter()
                        .map(|v| clipper2_rust::PointD::new(v[0], v[1]))
                        .collect()
                })
                .collect()
        };
        let free = clipper2_rust::difference_d(
            &paths(std::slice::from_ref(&ifp)),
            &paths(&nfps),
            clipper2_rust::FillRule::NonZero,
            6,
        );
        let sink = self.pile.map_or(self.geom.centre, |b| b.centre());
        let target = [sink[0] - v.hull.inner[0], sink[1] - v.hull.inner[1]];
        self.consider_translation(vi, v, target);
        self.consider_translation(
            vi,
            v,
            [
                self.geom.centre[0] - v.hull.inner[0],
                self.geom.centre[1] - v.hull.inner[1],
            ],
        );
        for poly in &free {
            for i in 0..poly.len() {
                let a = [poly[i].x, poly[i].y];
                let b = [poly[(i + 1) % poly.len()].x, poly[(i + 1) % poly.len()].y];
                self.edge_candidates(vi, v, a, b, target);
            }
        }
        // Exact-fit IFPs can be a point or segment, which Clipper omits.
        if polygon_area(&ifp) < EPS {
            for i in 0..ifp.len() {
                self.edge_candidates(vi, v, ifp[i], ifp[(i + 1) % ifp.len()], target);
                for poly in &nfps {
                    for j in 0..poly.len() {
                        if let Some(p) = segment_intersection(
                            ifp[i],
                            ifp[(i + 1) % ifp.len()],
                            poly[j],
                            poly[(j + 1) % poly.len()],
                        ) {
                            self.consider_translation(vi, v, p);
                        }
                    }
                }
            }
        }
    }

    fn edge_candidates(
        &mut self,
        vi: usize,
        v: &Variant,
        a: [f64; 2],
        b: [f64; 2],
        target: [f64; 2],
    ) {
        let d = [b[0] - a[0], b[1] - a[1]];
        let len2 = d[0] * d[0] + d[1] * d[1];
        let at = |u: f64| [a[0] + u * d[0], a[1] + u * d[1]];
        let mut positions = vec![0.0, 1.0];
        if len2 > EPS {
            positions.push(
                (((target[0] - a[0]) * d[0] + (target[1] - a[1]) * d[1]) / len2).clamp(0.0, 1.0),
            );
            // Alignment score changes at neighbour box coordinates. Include
            // all breakpoints rather than relying on numerical optimisation.
            for o in self.obstacles {
                for axis in 0..2 {
                    if d[axis].abs() > EPS {
                        for edge in [o.bbox.min[axis], o.bbox.max[axis]] {
                            for own in [v.hull.bbox.min[axis], v.hull.bbox.max[axis]] {
                                let u = (edge - own - a[axis]) / d[axis];
                                if (0.0..=1.0).contains(&u) {
                                    positions.push(u);
                                }
                            }
                        }
                    }
                }
            }
            // Fixed-iteration golden search replaces upstream's NLopt edge
            // optimiser; vertices/breakpoints above cover non-smooth minima.
            let (mut lo, mut hi) = (0.0, 1.0);
            for _ in 0..24 {
                let u = lo + (hi - lo) * 0.3819660112501051;
                let w = hi - (hi - lo) * 0.3819660112501051;
                if self.cost(v, at(u)) <= self.cost(v, at(w)) {
                    hi = w;
                } else {
                    lo = u;
                }
            }
            positions.push((lo + hi) * 0.5);
        }
        let len = len2.sqrt().max(EPS);
        // Clipper rounds to microns. Try the free-side normal too, then
        // validate against the original f64 hulls (never relax clearance).
        let normal = [-d[1] / len * 2e-6, d[0] / len * 2e-6];
        for u in positions {
            let p = at(u);
            self.consider_translation(vi, v, p);
            self.consider_translation(vi, v, [p[0] + normal[0], p[1] + normal[1]]);
        }
    }

    /// Validates candidates cheapest-first and returns the first that fits.
    /// Ties are broken by variant, then x, then y, so the result is deterministic.
    fn pick(&mut self, variants: &[Variant]) -> Option<Best> {
        self.candidates.sort_by(|a, b| {
            a.cost
                .total_cmp(&b.cost)
                .then(a.variant.cmp(&b.variant))
                .then(a.translation[0].total_cmp(&b.translation[0]))
                .then(a.translation[1].total_cmp(&b.translation[1]))
        });
        self.candidates.dedup_by(|a, b| {
            a.variant == b.variant
                && (a.translation[0] - b.translation[0]).abs() <= EPS
                && (a.translation[1] - b.translation[1]).abs() <= EPS
        });
        let found = self.candidates.iter().find(|c| {
            let v = &variants[c.variant];
            let tb = v.hull.bbox.translated(c.translation);
            !self
                .grid
                .any(&tb, |i| v.hull.overlaps(c.translation, &self.obstacles[i]))
                && self.geom.contains(&v.bed_hull.points, c.translation)
        });
        let best = found.map(|c| Best {
            variant: c.variant,
            translation: c.translation,
        });
        self.candidates.clear();
        best
    }

    /// Prusa arrange2 TMArrangeKernel::placement_fitness (negated to minimise).
    fn cost(&self, v: &Variant, t: [f64; 2]) -> f64 {
        let tb = v.hull.bbox.translated(t);
        if !self.align_center {
            return tb.min[0] + tb.min[1];
        }
        let area = polygon_area(&self.geom.polygon);
        let norm = area.sqrt();
        let big = |h: &Hull| h.area / area > 0.02;
        let bigbb = self
            .obstacles
            .iter()
            .filter(|o| big(o))
            .map(|o| o.bbox)
            .reduce(|a, b| a.union(&b));
        let centre = [v.hull.inner[0] + t[0], v.hull.inner[1] + t[1]];
        let sink = if let Some(bb) = bigbb.filter(|_| !big(&v.hull)) {
            bb.centre()
        } else {
            self.pile.map_or(self.geom.centre, |p| {
                p.union(&Bbox {
                    min: self.geom.centre,
                    max: self.geom.centre,
                })
                .centre()
            })
        };
        let dist = ((centre[0] - sink[0]).powi(2) + (centre[1] - sink[1]).powi(2)).sqrt() / norm;
        if !big(&v.hull) && bigbb.is_some() {
            return dist;
        }
        let item_area = v.hull.area;
        let mut alignment: f64 = 1.0;
        for o in self.obstacles.iter().filter(|o| !big(&v.hull) || big(o)) {
            if o.bbox.min[0] > tb.max[0] + EPS
                || o.bbox.max[0] < tb.min[0] - EPS
                || o.bbox.min[1] > tb.max[1] + EPS
                || o.bbox.max[1] < tb.min[1] - EPS
            {
                continue;
            }
            if item_area > EPS && (1.0 - o.area / item_area).abs() < 1e-6 {
                let bb = tb.union(&o.bbox);
                let score = 1.0
                    - (tb.width() * tb.height() + o.bbox.width() * o.bbox.height())
                        / (bb.width() * bb.height());
                alignment = alignment.min(score);
            }
        }
        let r = self.remaining_ratio;
        0.6 * dist + 0.1 * alignment + (1.0 - r) * 0.3 * dist + r * 0.3 * alignment
    }
}

/// Convex Minkowski difference in translation coordinates (no reference-vertex
/// shift is needed). Merge the angularly sorted edge vectors of fixed and
/// reflected movable hulls, as in nfp_convex_convex.
fn no_fit(fixed: &[[f64; 2]], moving: &[[f64; 2]]) -> Vec<[f64; 2]> {
    let reflected: Vec<_> = moving.iter().map(|p| [-p[0], -p[1]]).collect();
    let start = |p: &[[f64; 2]]| {
        *p.iter()
            .min_by(|a, b| a[1].total_cmp(&b[1]).then(a[0].total_cmp(&b[0])))
            .unwrap()
    };
    let a = start(fixed);
    let b = start(&reflected);
    let mut p = [a[0] + b[0], a[1] + b[1]];
    let mut edges = Vec::with_capacity(fixed.len() + moving.len());
    for poly in [fixed, &reflected] {
        for i in 0..poly.len() {
            let d = [
                poly[(i + 1) % poly.len()][0] - poly[i][0],
                poly[(i + 1) % poly.len()][1] - poly[i][1],
            ];
            if d[0].abs() + d[1].abs() > EPS {
                edges.push((d[1].atan2(d[0]).rem_euclid(2.0 * PI), d));
            }
        }
    }
    edges.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut out = vec![p];
    for (_, d) in edges {
        p = [p[0] + d[0], p[1] + d[1]];
        out.push(p);
    }
    convex_hull(&out)
}

/// Inner-fit polygon: translations keeping the bed hull inside a convex bed.
/// For concave beds the rectangular IFP is trimmed by boundary-edge NFPs.
fn inner_fit(geom: &BedGeom, hull: &Hull) -> Vec<[f64; 2]> {
    let min = [
        geom.bbox.min[0] - hull.bbox.min[0],
        geom.bbox.min[1] - hull.bbox.min[1],
    ];
    let max = [
        geom.bbox.max[0] - hull.bbox.max[0],
        geom.bbox.max[1] - hull.bbox.max[1],
    ];
    if min[0] > max[0] + EPS || min[1] > max[1] + EPS {
        return vec![];
    }
    let mut poly = vec![min, [max[0], min[1]], max, [min[0], max[1]]];
    if geom.is_rect || !geom.is_convex {
        return poly;
    }
    for i in 0..geom.polygon.len() {
        let a = geom.polygon[i];
        let b = geom.polygon[(i + 1) % geom.polygon.len()];
        let d = [b[0] - a[0], b[1] - a[1]];
        let bound = hull
            .points
            .iter()
            .map(|p| cross(a, b, *p))
            .fold(f64::INFINITY, f64::min);
        let value = |p: [f64; 2]| bound + d[0] * p[1] - d[1] * p[0];
        let mut next = vec![];
        for j in 0..poly.len() {
            let p = poly[j];
            let q = poly[(j + 1) % poly.len()];
            let (vp, vq) = (value(p), value(q));
            if vp >= -EPS {
                next.push(p);
            }
            if (vp >= 0.0) != (vq >= 0.0) {
                let u = vp / (vp - vq);
                next.push([p[0] + u * (q[0] - p[0]), p[1] + u * (q[1] - p[1])]);
            }
        }
        poly = next;
    }
    poly
}

fn segment_intersection(a: [f64; 2], b: [f64; 2], c: [f64; 2], d: [f64; 2]) -> Option<[f64; 2]> {
    let ab = [b[0] - a[0], b[1] - a[1]];
    let cd = [d[0] - c[0], d[1] - c[1]];
    let det = ab[0] * cd[1] - ab[1] * cd[0];
    if det.abs() < EPS {
        return None;
    }
    let ca = [c[0] - a[0], c[1] - a[1]];
    let u = (ca[0] * cd[1] - ca[1] * cd[0]) / det;
    let v = (ca[0] * ab[1] - ca[1] * ab[0]) / det;
    ((-EPS..=1.0 + EPS).contains(&u) && (-EPS..=1.0 + EPS).contains(&v))
        .then(|| [a[0] + u * ab[0], a[1] + u * ab[1]])
}

/// Coarse grid of corners over the bed, used when no skyline candidate fits.
fn grid_candidates(out: &mut Vec<[f64; 2]>, b: &Bbox, geom: &BedGeom) {
    let bb = &geom.bbox;
    let (x_end, y_end) = (bb.max[0] - b.width(), bb.max[1] - b.height());
    let mut y = bb.min[1];
    while y <= y_end + EPS {
        let mut x = bb.min[0];
        while x <= x_end + EPS {
            out.push([x, y]);
            x += GRID_STEP;
        }
        out.push([x_end, y]);
        y += GRID_STEP;
    }
    let mut x = bb.min[0];
    while x <= x_end + EPS {
        out.push([x, y_end]);
        x += GRID_STEP;
    }
    out.push([x_end, y_end]);
}

/// Inflates a convex hull by `d` (Minkowski sum with an octagon that contains
/// the disc of radius `d`, so the true clearance is at least `d`).
fn inflate(hull: &[[f64; 2]], d: f64) -> Vec<[f64; 2]> {
    if d <= 0.0 || hull.is_empty() {
        return hull.to_vec();
    }
    let r = d / (PI / 8.0).cos();
    let octagon: Vec<[f64; 2]> = (0..8)
        .map(|k| {
            let a = PI / 8.0 + k as f64 * FRAC_PI_4;
            [r * a.cos(), r * a.sin()]
        })
        .collect();
    let mut pts = Vec::with_capacity(hull.len() * 8);
    for p in hull {
        for o in &octagon {
            pts.push([p[0] + o[0], p[1] + o[1]]);
        }
    }
    convex_hull(&pts)
}

fn rotate(p: [f64; 2], pivot: [f64; 2], angle: f64) -> [f64; 2] {
    let (s, c) = exact_sin_cos(angle);
    let (dx, dy) = (p[0] - pivot[0], p[1] - pivot[1]);
    [pivot[0] + c * dx - s * dy, pivot[1] + s * dx + c * dy]
}

/// `sin_cos` that is exact at multiples of 90° so rectangles stay rectangles.
fn exact_sin_cos(angle: f64) -> (f64, f64) {
    let quarter = angle / FRAC_PI_2;
    let k = quarter.round();
    if (quarter - k).abs() < 1e-12 {
        match k.rem_euclid(4.0) as u8 {
            0 => (0.0, 1.0),
            1 => (1.0, 0.0),
            2 => (0.0, -1.0),
            _ => (-1.0, 0.0),
        }
    } else {
        angle.sin_cos()
    }
}

fn cross(o: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
}

fn polygon_area(poly: &[[f64; 2]]) -> f64 {
    let n = poly.len();
    if n < 3 {
        return 0.0;
    }
    let mut s = 0.0;
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        s += a[0] * b[1] - b[0] * a[1];
    }
    0.5 * s.abs()
}

/// Separating-axis test with `a` translated by `ta`. Touching is not overlap.
fn sat_overlap(a: &[[f64; 2]], ta: [f64; 2], b: &[[f64; 2]]) -> bool {
    if a.is_empty() || b.is_empty() {
        return false;
    }
    let mut any_axis = false;
    for (poly, is_a) in [(a, true), (b, false)] {
        let n = poly.len();
        if n < 2 {
            continue;
        }
        for i in 0..n {
            let (p, q) = (poly[i], poly[(i + 1) % n]);
            let axis = [q[1] - p[1], p[0] - q[0]];
            let len = (axis[0] * axis[0] + axis[1] * axis[1]).sqrt();
            if len <= 1e-12 {
                continue;
            }
            any_axis = true;
            let axis = [axis[0] / len, axis[1] / len];
            let _ = is_a;
            let (a_min, a_max) = project(a, ta, axis);
            let (b_min, b_max) = project(b, [0.0, 0.0], axis);
            if a_max <= b_min + EPS || b_max <= a_min + EPS {
                return false;
            }
        }
    }
    if !any_axis {
        // Two points: they never "overlap" in the touching-allowed sense.
        return false;
    }
    true
}

fn project(poly: &[[f64; 2]], t: [f64; 2], axis: [f64; 2]) -> (f64, f64) {
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;
    for p in poly {
        let d = (p[0] + t[0]) * axis[0] + (p[1] + t[1]) * axis[1];
        lo = lo.min(d);
        hi = hi.max(d);
    }
    (lo, hi)
}

/// Even-odd point-in-polygon; points on the boundary count as inside.
fn point_in_polygon(p: [f64; 2], poly: &[[f64; 2]]) -> bool {
    let n = poly.len();
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (a, b) = (poly[i], poly[j]);
        if point_segment_distance(p, a, b) <= 1e-7 {
            return true;
        }
        if (a[1] > p[1]) != (b[1] > p[1]) {
            let x = b[0] + (p[1] - b[1]) * (a[0] - b[0]) / (a[1] - b[1]);
            if p[0] < x {
                inside = !inside;
            }
        }
        j = i;
    }
    inside
}

fn point_segment_distance(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let len2 = dx * dx + dy * dy;
    let t = if len2 <= 1e-24 {
        0.0
    } else {
        (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / len2).clamp(0.0, 1.0)
    };
    let (cx, cy) = (a[0] + t * dx - p[0], a[1] + t * dy - p[1]);
    (cx * cx + cy * cy).sqrt()
}

/// Proper (interior) crossing of two segments; shared endpoints or touching do not count.
fn segments_cross(p1: [f64; 2], p2: [f64; 2], q1: [f64; 2], q2: [f64; 2]) -> bool {
    let d1 = cross(q1, q2, p1);
    let d2 = cross(q1, q2, p2);
    let d3 = cross(p1, p2, q1);
    let d4 = cross(p1, p2, q2);
    let opposite = |u: f64, v: f64| (u > EPS && v < -EPS) || (u < -EPS && v > EPS);
    opposite(d1, d2) && opposite(d3, d4)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn rect(x: f64, y: f64, w: f64, h: f64) -> Vec<[f64; 2]> {
        vec![[x, y], [x + w, y], [x + w, y + h], [x, y + h]]
    }

    fn rect_item(w: f64, h: f64) -> Item {
        Item {
            footprint: rect(0.0, 0.0, w, h),
            pivot: [w * 0.5, h * 0.5],
            fixed: false,
        }
    }

    fn bed(w: f64, h: f64) -> Bed {
        Bed {
            polygon: rect(0.0, 0.0, w, h),
            forbidden: Vec::new(),
        }
    }

    /// Deterministic xorshift for test sizes.
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> f64 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.0 = x;
            (x >> 11) as f64 / (1u64 << 53) as f64
        }
        fn range(&mut self, lo: f64, hi: f64) -> f64 {
            lo + (hi - lo) * self.next()
        }
    }

    fn random_items(n: usize, seed: u64) -> Vec<Item> {
        let mut rng = Rng(seed);
        (0..n)
            .map(|_| rect_item(rng.range(10.0, 40.0), rng.range(10.0, 40.0)))
            .collect()
    }

    /// Placed footprint of item `i`, inflated by half the spacing.
    fn placed_hull(items: &[Item], res: &Result, i: usize, spacing: f64) -> Vec<[f64; 2]> {
        let (_, pl) = res.placed.iter().find(|(k, _)| *k == i).expect("placed");
        let pts = apply_placement(&items[i].footprint, items[i].pivot, pl);
        inflate(&convex_hull(&pts), spacing * 0.5)
    }

    /// Asserts every placed item is inside the bed, clear of forbidden regions
    /// and at least `spacing` away from every other placed item.
    fn check_valid(items: &[Item], bed: &Bed, settings: &Settings, res: &Result) {
        let geom = BedGeom::new(&bed.polygon).expect("bed");
        let hulls: Vec<(usize, Vec<[f64; 2]>)> = res
            .placed
            .iter()
            .map(|(i, _)| (*i, placed_hull(items, res, *i, settings.spacing)))
            .collect();
        let forbidden: Vec<Vec<[f64; 2]>> = bed.forbidden.iter().map(|f| convex_hull(f)).collect();
        for (a, (i, ha)) in hulls.iter().enumerate() {
            if !items[*i].fixed {
                assert!(
                    geom.contains(
                        &placed_hull(items, res, *i, settings.bed_margin * 2.0),
                        [0.0, 0.0]
                    ),
                    "item {i} outside bed: {ha:?}"
                );
                for f in &forbidden {
                    assert!(
                        !polygons_overlap(ha, f),
                        "item {i} overlaps forbidden region"
                    );
                }
            }
            for (j, hb) in &hulls[a + 1..] {
                assert!(!polygons_overlap(ha, hb), "items {i} and {j} overlap");
            }
        }
        for (i, _) in &res.placed {
            assert!(!res.unplaced.contains(i));
        }
        assert_eq!(res.placed.len() + res.unplaced.len(), items.len());
    }

    #[test]
    fn nfp_matches_minkowski_vertex_oracle() {
        let fixed = convex_hull(&[[2., 3.], [20., 1.], [30., 12.], [10., 25.], [0., 10.]]);
        let moving = convex_hull(&[[-4., 2.], [8., -2.], [12., 9.], [3., 16.]]);
        let sums: Vec<_> = fixed
            .iter()
            .flat_map(|a| moving.iter().map(move |b| [a[0] - b[0], a[1] - b[1]]))
            .collect();
        let expected = convex_hull(&sums);
        let nfp = no_fit(&fixed, &moving);
        assert_eq!(nfp.len(), expected.len());
        for (a, b) in nfp.iter().zip(&expected) {
            assert!((a[0] - b[0]).abs() < 1e-9 && (a[1] - b[1]).abs() < 1e-9);
        }
        for x in -20..40 {
            for y in -20..40 {
                let t = [x as f64 + 0.123, y as f64 + 0.456];
                assert_eq!(
                    strictly_inside_convex(t, &nfp),
                    sat_overlap(&moving, t, &fixed)
                );
            }
        }
    }

    #[test]
    fn nfp_packs_against_diagonal_hull_not_bounding_box() {
        let items = vec![
            Item {
                footprint: vec![[0., 0.], [100., 0.], [0., 100.]],
                pivot: [0., 0.],
                fixed: true,
            },
            Item {
                footprint: vec![[0., 0.], [40., 0.], [0., 40.]],
                pivot: [0., 0.],
                fixed: false,
            },
        ];
        let bed = bed(100., 100.);
        let settings = Settings {
            spacing: 0.,
            ..Settings::default()
        };
        let result = arrange(&items, &bed, &settings);
        assert!(result.unplaced.is_empty());
        check_valid(&items, &bed, &settings, &result);
        let t = result.placed[1].1.translation;
        assert!(
            (t[0] + t[1] - 100.).abs() < 1e-5,
            "diagonal NFP contact: {t:?}"
        );
        let moving = Bbox::of(&placed_hull(&items, &result, 1, 0.));
        assert!(
            moving.overlaps(&Bbox::of(&items[0].footprint)),
            "bounding boxes must overlap"
        );
    }

    #[test]
    fn bed_distance_is_independent_of_object_spacing_and_exact_fit_terminates() {
        let items = vec![rect_item(100., 80.)];
        let bed = bed(100., 80.);
        let result = arrange(&items, &bed, &Settings::default());
        assert_eq!(result.placed.len(), 1);
        assert!(result.placed[0].1.translation.iter().all(|v| v.abs() < EPS));
        assert_eq!(result.placed[0].1.rotation, 0.0);
        let settings = Settings {
            bed_margin: 1.,
            ..Settings::default()
        };
        assert_eq!(arrange(&items, &bed, &settings).unplaced, vec![0]);
    }

    #[test]
    fn oversized_item_is_unplaced_without_mutating_input() {
        let mut items = vec![rect_item(1000., 1000.), rect_item(20., 20.)];
        items[0].footprint = rect(-400., 900., 1000., 1000.);
        let before = items[0].footprint.clone();
        let bed = bed(250., 220.);
        let result = arrange(&items, &bed, &Settings::default());
        assert_eq!(result.unplaced, vec![0]);
        assert_eq!(result.placed.len(), 1);
        assert_eq!(items[0].footprint, before);
        assert_eq!(
            result.placed,
            arrange(&items, &bed, &Settings::default()).placed
        );
    }

    #[test]
    fn spacing_config_reserves_brim_skirt_at_bed_and_sequential_clearance() {
        let mut c = crate::config::Config::defaults();
        for (key, value) in [
            ("duplicate_distance", "9"),
            ("complete_objects", "1"),
            ("extruder_clearance_radius", "30"),
            ("brim_type", "outer_only"),
            ("brim_width", "4"),
            ("brim_separation", "0.5"),
            ("skirts", "2"),
            ("skirt_height", "1"),
            ("skirt_distance", "3"),
            ("first_layer_extrusion_width", "0.5"),
        ] {
            c.set_raw(key, value).unwrap();
        }
        let settings = Settings::from_config(&c);
        assert_eq!(settings.spacing, 30.);
        assert_eq!(settings.bed_margin, 8.5);
        assert!(!settings.rotations);
        c.set_raw("complete_objects", "0").unwrap();
        c.set_raw("bed_arrange_distance", "0").unwrap();
        c.set_raw("brim_type", "inner_only").unwrap();
        assert_eq!(Settings::from_config(&c).spacing, 0.);
        assert_eq!(Settings::from_config(&c).bed_margin, 4.);
        c.set_raw("skirt_height", "0").unwrap();
        assert_eq!(Settings::from_config(&c).bed_margin, 0.);
    }

    #[test]
    fn concave_bed_notch_is_not_crossed_even_at_boundary_vertices() {
        let bed = Bed {
            polygon: vec![
                [0., 0.],
                [100., 0.],
                [100., 100.],
                [60., 100.],
                [60., 40.],
                [40., 40.],
                [40., 100.],
                [0., 100.],
            ],
            forbidden: vec![],
        };
        let geom = BedGeom::new(&bed.polygon).unwrap();
        assert!(!geom.contains(&rect(20., 60., 60., 40.), [0., 0.]));
        let items: Vec<_> = (0..8).map(|_| rect_item(20., 20.)).collect();
        let settings = Settings::default();
        let result = arrange(&items, &bed, &settings);
        assert!(result.unplaced.is_empty());
        check_valid(&items, &bed, &settings, &result);
    }

    #[test]
    fn fifty_outline_hulls_fast_valid_and_deterministic() {
        let items: Vec<_> = (0..50)
            .map(|i| Item {
                footprint: (0..16)
                    .map(|j| {
                        let a = j as f64 * 2. * PI / 16.;
                        [
                            ((i % 5) as f64 + 8.) * a.cos(),
                            ((i % 7) as f64 + 8.) * a.sin(),
                        ]
                    })
                    .collect(),
                pivot: [0., 0.],
                fixed: false,
            })
            .collect();
        let bed = bed(250., 220.);
        let settings = Settings::default();
        let start = Instant::now();
        let result = arrange(&items, &bed, &settings);
        let elapsed = start.elapsed();
        eprintln!("50 outline hulls arranged in {elapsed:?}");
        assert!(elapsed.as_secs_f64() < if cfg!(debug_assertions) { 10. } else { 1. });
        assert!(result.unplaced.is_empty());
        check_valid(&items, &bed, &settings, &result);
        assert_eq!(result.placed, arrange(&items, &bed, &settings).placed);
    }

    #[test]
    fn hull_is_ccw_without_duplicates() {
        let pts = [
            [0.0, 0.0],
            [10.0, 0.0],
            [10.0, 10.0],
            [0.0, 10.0],
            [5.0, 5.0],
            [5.0, 0.0],
            [10.0, 0.0],
        ];
        let h = convex_hull(&pts);
        assert_eq!(h.len(), 4);
        assert!(polygon_area(&h) > 0.0);
        let mut s = 0.0;
        for i in 0..4 {
            let (a, b) = (h[i], h[(i + 1) % 4]);
            s += a[0] * b[1] - b[0] * a[1];
        }
        assert!(s > 0.0, "hull must be counter-clockwise");
    }

    #[test]
    fn overlap_touching_is_not_overlap() {
        let a = rect(0.0, 0.0, 10.0, 10.0);
        let b = rect(10.0, 0.0, 10.0, 10.0);
        let c = rect(9.0, 5.0, 10.0, 10.0);
        let d = rect(20.0, 20.0, 1.0, 1.0);
        assert!(!polygons_overlap(&a, &b));
        assert!(polygons_overlap(&a, &c));
        assert!(!polygons_overlap(&a, &d));
        let tri = vec![[2.0, 2.0], [8.0, 2.0], [5.0, 8.0]];
        assert!(polygons_overlap(&a, &tri));
        assert!(polygons_overlap(&tri, &a));
    }

    #[test]
    fn twelve_rectangles_fit_with_spacing() {
        let sizes = [
            (60.0, 40.0),
            (30.0, 30.0),
            (50.0, 20.0),
            (25.0, 45.0),
            (40.0, 40.0),
            (15.0, 60.0),
            (35.0, 25.0),
            (20.0, 20.0),
            (55.0, 30.0),
            (10.0, 10.0),
            (45.0, 35.0),
            (30.0, 50.0),
        ];
        let items: Vec<Item> = sizes.iter().map(|&(w, h)| rect_item(w, h)).collect();
        let bed = bed(250.0, 220.0);
        let settings = Settings::default();
        let res = arrange(&items, &bed, &settings);
        assert!(res.unplaced.is_empty(), "unplaced: {:?}", res.unplaced);
        check_valid(&items, &bed, &settings, &res);
        // Spacing honoured: actual (un-inflated) hulls are at least `spacing` apart.
        for a in 0..items.len() {
            for b in a + 1..items.len() {
                let ha = placed_hull(&items, &res, a, 0.0);
                let hb = placed_hull(&items, &res, b, 0.0);
                let gap = min_distance(&ha, &hb);
                assert!(
                    gap >= settings.spacing - 1e-6,
                    "gap {gap} between {a} and {b}"
                );
            }
        }
    }

    fn min_distance(a: &[[f64; 2]], b: &[[f64; 2]]) -> f64 {
        let mut best = f64::INFINITY;
        for (p, q) in [(a, b), (b, a)] {
            let n = q.len();
            for v in p {
                for i in 0..n {
                    best = best.min(point_segment_distance(*v, q[i], q[(i + 1) % n]));
                }
            }
        }
        best
    }

    #[test]
    fn fifty_items_fast_and_valid() {
        let items = random_items(50, 0x9E37_79B9_7F4A_7C15);
        let bed = bed(250.0, 220.0);
        let settings = Settings::default();
        let start = Instant::now();
        let res = arrange(&items, &bed, &settings);
        let elapsed = start.elapsed();
        eprintln!("50 items arranged in {elapsed:?}");
        assert!(
            elapsed.as_secs_f64() < if cfg!(debug_assertions) { 10.0 } else { 1.0 },
            "arrange took {elapsed:?}"
        );
        assert!(res.unplaced.is_empty(), "unplaced: {:?}", res.unplaced);
        check_valid(&items, &bed, &settings, &res);

        let start = Instant::now();
        let rotated = arrange(
            &items,
            &bed,
            &Settings {
                rotations: true,
                ..Settings::default()
            },
        );
        let elapsed = start.elapsed();
        eprintln!("50 items with rotations arranged in {elapsed:?}");
        assert!(
            elapsed.as_secs_f64() < if cfg!(debug_assertions) { 10.0 } else { 1.0 },
            "arrange took {elapsed:?}"
        );
        assert!(rotated.unplaced.is_empty());
        check_valid(
            &items,
            &bed,
            &Settings {
                rotations: true,
                ..Settings::default()
            },
            &rotated,
        );
    }

    #[test]
    fn too_many_items_leaves_some_unplaced() {
        let items: Vec<Item> = (0..30).map(|_| rect_item(80.0, 80.0)).collect();
        let bed = bed(250.0, 220.0);
        let settings = Settings::default();
        let res = arrange(&items, &bed, &settings);
        assert!(!res.unplaced.is_empty());
        assert_eq!(res.placed.len(), 4, "two columns by two rows of 86 mm fit");
        check_valid(&items, &bed, &settings, &res);
    }

    #[test]
    fn exclusion_zone_and_tower_are_avoided() {
        let items: Vec<Item> = (0..8).map(|_| rect_item(40.0, 40.0)).collect();
        let bed = Bed {
            polygon: rect(0.0, 0.0, 250.0, 220.0),
            forbidden: vec![rect(0.0, 0.0, 250.0, 60.0), rect(200.0, 150.0, 40.0, 60.0)],
        };
        let settings = Settings::default();
        let res = arrange(&items, &bed, &settings);
        assert!(res.unplaced.is_empty(), "unplaced: {:?}", res.unplaced);
        check_valid(&items, &bed, &settings, &res);
        for (i, _) in &res.placed {
            let h = placed_hull(&items, &res, *i, 0.0);
            assert!(
                h.iter()
                    .all(|p| p[1] >= 60.0 + settings.spacing * 0.5 - 1e-6)
            );
        }
    }

    #[test]
    fn long_bar_only_fits_rotated() {
        let items = vec![rect_item(200.0, 30.0)];
        let bed = bed(100.0, 250.0);
        let flat = arrange(&items, &bed, &Settings::default());
        assert_eq!(flat.unplaced, vec![0]);
        let settings = Settings {
            rotations: true,
            ..Settings::default()
        };
        let res = arrange(&items, &bed, &settings);
        assert!(res.unplaced.is_empty());
        check_valid(&items, &bed, &settings, &res);
        let h = placed_hull(&items, &res, 0, 0.0);
        let b = Bbox::of(&h);
        assert!((b.width() - 30.0).abs() < 1e-9 && (b.height() - 200.0).abs() < 1e-9);
        assert!((res.placed[0].1.rotation - FRAC_PI_2).abs() < 1e-12);
    }

    #[test]
    fn octagonal_bed_keeps_items_inside() {
        let c = 50.0;
        let (w, h) = (250.0, 220.0);
        let bed = Bed {
            polygon: vec![
                [c, 0.0],
                [w - c, 0.0],
                [w, c],
                [w, h - c],
                [w - c, h],
                [c, h],
                [0.0, h - c],
                [0.0, c],
            ],
            forbidden: Vec::new(),
        };
        let items: Vec<Item> = (0..20).map(|_| rect_item(30.0, 30.0)).collect();
        let settings = Settings {
            bed_margin: 3.0,
            ..Settings::default()
        };
        let res = arrange(&items, &bed, &settings);
        assert!(res.unplaced.is_empty(), "unplaced: {:?}", res.unplaced);
        check_valid(&items, &bed, &settings, &res);
        for (i, _) in &res.placed {
            let hull = placed_hull(&items, &res, *i, 0.0);
            for p in &hull {
                assert!(point_in_polygon(*p, &bed.polygon), "{p:?} outside octagon");
                // Inside the cut corners: x + y >= c etc.
                assert!(p[0] + p[1] >= c - 1e-6);
                assert!((w - p[0]) + p[1] >= c - 1e-6);
                assert!(p[0] + (h - p[1]) >= c - 1e-6);
                assert!((w - p[0]) + (h - p[1]) >= c - 1e-6);
            }
        }
    }

    #[test]
    fn single_item_is_centred() {
        let items = vec![Item {
            footprint: rect(300.0, -20.0, 50.0, 30.0),
            pivot: [310.0, 0.0],
            fixed: false,
        }];
        let bed = bed(250.0, 220.0);
        let res = arrange(&items, &bed, &Settings::default());
        assert_eq!(res.placed.len(), 1);
        let h = placed_hull(&items, &res, 0, 0.0);
        let c = Bbox::of(&h).centre();
        assert!(
            (c[0] - 125.0).abs() < 1e-9 && (c[1] - 110.0).abs() < 1e-9,
            "{c:?}"
        );
    }

    #[test]
    fn fixed_item_stays_and_is_avoided() {
        let mut items = vec![Item {
            footprint: rect(100.0, 90.0, 50.0, 40.0),
            pivot: [125.0, 110.0],
            fixed: true,
        }];
        items.extend((0..6).map(|_| rect_item(40.0, 40.0)));
        let bed = bed(250.0, 220.0);
        let settings = Settings::default();
        let res = arrange(&items, &bed, &settings);
        assert!(res.unplaced.is_empty(), "unplaced: {:?}", res.unplaced);
        assert_eq!(res.placed[0], (0, Placement::IDENTITY));
        check_valid(&items, &bed, &settings, &res);
        let fixed = inflate(&convex_hull(&items[0].footprint), settings.spacing * 0.5);
        for i in 1..items.len() {
            let h = placed_hull(&items, &res, i, settings.spacing);
            assert!(!polygons_overlap(&h, &fixed));
        }
    }

    #[test]
    fn deterministic() {
        let items = random_items(40, 42);
        let bed = Bed {
            polygon: rect(0.0, 0.0, 250.0, 220.0),
            forbidden: vec![rect(0.0, 180.0, 60.0, 40.0)],
        };
        let settings = Settings {
            rotations: true,
            ..Settings::default()
        };
        let a = arrange(&items, &bed, &settings);
        let b = arrange(&items, &bed, &settings);
        assert_eq!(a.placed, b.placed);
        assert_eq!(a.unplaced, b.unplaced);
    }

    #[test]
    fn bottom_left_mode_packs_towards_origin() {
        let items = random_items(10, 7);
        let bed = bed(250.0, 220.0);
        let settings = Settings {
            align_center: false,
            ..Settings::default()
        };
        let res = arrange(&items, &bed, &settings);
        assert!(res.unplaced.is_empty());
        check_valid(&items, &bed, &settings, &res);
        let pile = res
            .placed
            .iter()
            .map(|(i, _)| Bbox::of(&placed_hull(&items, &res, *i, settings.bed_margin * 2.0)))
            .reduce(|a, b| a.union(&b))
            .unwrap();
        assert!(pile.min[0].abs() < 1e-9 && pile.min[1].abs() < 1e-9);
        assert!(pile.max[0] < 200.0 && pile.max[1] < 200.0);
    }
}
