// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: bricklayers@59e581e364 bricklayers.py :: BrickLayersProcessor.process_gcode, calculate_loop_depth, generate_deffered_perimeters
// Changes: The idea only, re-implemented in the engine (no G-code parsing): walls are picked by
// Changes: their inset index, not by bounding-box loop trees; where walls are raised is decided per
// Changes: area from the outlines of the neighbouring layers instead of one global start and end
// Changes: layer; raised walls keep Mica's own seams, travels and retraction; every bead is at
// Changes: least as tall as a layer (starts are 1.5 layers tall, endings skip one layer and print
// Changes: the next one 1.5 layers tall), never half-height caps or reduced flow.

//! Brick layers: every other inner wall is printed half a layer higher, so the walls' layer
//! lines are staggered like bricks and no layer line runs straight through a wall.
//!
//! # Product rules
//!
//! These rules hold for every print and are checked on the G-code by `mica_gcode::brick_check`
//! (tests in `mica-cli/tests/brick_layers.rs`; docs/brick-layers.md):
//!
//! 1. **Every bead is at least as tall as a layer.** No extrusion is thinner than the nominal
//!    layer height, and no flow is ever reduced (or speed raised) to make a bead thinner. The only
//!    flow change is the optional extra flow ([`BrickSettings::flow_ratio`], never below 100 %).
//! 2. **Starts are taller, not shorter.** The first raised bead of a wall reaches from the layer
//!    below to half a layer above its own layer: 1.5 layers tall.
//! 3. **Endings rejoin the grid upward.** A raised wall stops being raised well below a top
//!    surface: it skips one layer, and the next layer's bead fills the 1.5-layer gap. The top
//!    surface layers, the layers next to bottom surfaces and the part's first and last layers
//!    print exactly as without brick layers.
//! 4. **Walls stay walls.** Only whole stretches of inner wall loops move up; no extra beads,
//!    caps or end pieces are added.
//!
//! # How it works
//!
//! Walls are numbered from the outside (outer wall 0). Inner walls with an odd number (1, 3, …)
//! are the *raised* walls. [`Plan::build`] decides once per object, layer by layer, the area in
//! which raised walls print raised: where the outlines (and, with modifiers, the regions) of the
//! layers around stay the same, so the walls stand on the same walls below and carry the same
//! walls above. The area leaves out the start layers, the bottom and top shells around every
//! bottom and top surface (plus a margin), and anything near an outline that changes.
//!
//! [`Plan::apply`] then cuts each raised wall of a layer at that area's borders:
//!
//! | below (previous layer) | in the area now | result |
//! |---|---|---|
//! | normal | yes | start: raised, from the layer below (1.5 layers) |
//! | raised | yes | raised, one layer tall |
//! | raised | no | skipped this layer |
//! | skipped | no | normal Z, from the raised bead two layers down (1.5 layers) |
//! | skipped | yes | raised, from the raised bead two layers down (2 layers) |
//! | normal | no | unchanged |
//!
//! "Raised below" is tested against the layer below's raised area widened by a quarter wall
//! width, so a wall that moved a little still counts as standing on its raised bead (and is
//! skipped rather than printed on top of it).
//!
//! A raised bead tops out half a layer above its layer; with variable layer heights, half the
//! object's thinnest layer, so a raised bead on a raised bead is exactly as tall as its own
//! layer and a start or an ending is taller by that half. Raised stretches print
//! after everything else of their object on the layer, so the nozzle never works next to a
//! bead that already stands higher than it. The G-code writer lifts travels over raised beads.
//!
//! Multi-tool prints (a wipe tower, several tools on a layer) slice without brick layers in this
//! version: [`Plan::suppress`].

use std::sync::atomic::{AtomicBool, Ordering};

use rayon::prelude::*;

use crate::config::Config;
use crate::flow::Flow;
use crate::geometry::clip::{
    Join, diff_ex, intersection_pl, offset_ex, opening_ex, simplify_ex, union_ex,
};
use crate::geometry::{BoundingBox, ExPolygon, Point, Polyline, scale};
use crate::toolpath::{BrickWall, ExtrusionPath, Role};

/// Every brick-layers option.
pub const KEYS: [&str; 5] = [
    "mica_brick_layers",
    "mica_brick_layers_start_layer",
    "mica_brick_layers_flow_ratio",
    "mica_brick_layers_travel_lift",
    "mica_brick_layers_end_margin_layers",
];

/// The brick-layers options of a print.
#[derive(Clone, Debug, PartialEq)]
pub struct BrickSettings {
    pub enabled: bool,
    /// First layer (counted from 0) whose walls may be raised.
    pub start_layer: usize,
    /// Flow multiplier on raised walls and the inner walls between them, at least 1.
    pub flow_ratio: f64,
    /// Lift for travels over raised walls, mm; 0 picks `max(retract_lift, 0.2 mm)`.
    pub travel_lift: f64,
    /// Extra layers kept unraised next to top and bottom shells.
    pub end_margin_layers: usize,
}

impl Default for BrickSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            start_layer: 2,
            flow_ratio: 1.05,
            travel_lift: 0.0,
            end_margin_layers: 1,
        }
    }
}

impl BrickSettings {
    pub fn from_config(c: &Config) -> Result<Self, crate::resolve::ResolveError> {
        let d = Self::default();
        let ratio = c
            .float("mica_brick_layers_flow_ratio")
            .map_or(d.flow_ratio, |p| p / 100.0);
        Ok(Self {
            // Vase mode prints a single wall: nothing to raise.
            enabled: c.bool("mica_brick_layers").unwrap_or(false)
                && !c.bool("spiral_vase").unwrap_or(false),
            start_layer: c
                .int("mica_brick_layers_start_layer")
                .map_or(d.start_layer, |v| v.max(1) as usize),
            // Product rule 1: extra flow only, never less.
            flow_ratio: if ratio.is_finite() {
                ratio.clamp(1.0, 1.5)
            } else {
                1.0
            },
            travel_lift: c
                .float("mica_brick_layers_travel_lift")
                .map_or(d.travel_lift, |v| v.clamp(0.0, 5.0)),
            end_margin_layers: c
                .int("mica_brick_layers_end_margin_layers")
                .map_or(d.end_margin_layers, |v| v.clamp(0, 50) as usize),
        })
    }
}

/// Whether a configuration turns brick layers on.
pub fn enabled_in(c: &Config) -> bool {
    c.bool("mica_brick_layers").unwrap_or(false)
}

/// Where an object's raised walls are raised, per object layer.
#[derive(Debug)]
pub struct Plan {
    /// The area in which raised walls are raised, per object layer (placed coordinates).
    raised: Vec<Vec<ExPolygon>>,
    /// Print Z of each object layer, mm.
    tops: Vec<f64>,
    /// How far a raised bead of each layer stands above the layer, mm: half the object's thinnest
    /// layer (0 on the last layer).
    raise: Vec<f64>,
    /// `raised` widened by a quarter wall width: a raised wall that moved a little, or met the
    /// area's border at a shallow angle, still counts as standing on (or next to) the raised
    /// bead below it, so it never prints at layer height on top of it.
    widened: Vec<Vec<ExPolygon>>,
    /// Where raised walls were skipped on each layer (the layer below's widened raised area
    /// minus this layer's raised area), widened likewise.
    skipped: Vec<Vec<ExPolygon>>,
    flow_ratio: f64,
    /// What the slice result says about brick layers on this object.
    pub warnings: Vec<String>,
    /// Set when the print cannot use brick layers ([`Self::suppress`]).
    suppressed: AtomicBool,
}

/// One layer's region slices: (region id, slices).
pub type LayerRegions<'a> = Vec<(usize, &'a [ExPolygon])>;

/// What [`Plan::build`] reads of an object.
pub struct PlanInput<'a> {
    /// Outlines per object layer, placed.
    pub outlines: &'a [Vec<ExPolygon>],
    /// The first layer without elephant-foot compensation (which is no outline change).
    pub first_layer: Option<&'a [ExPolygon]>,
    /// With modifiers or painting: each layer's region slices (any order, region ids alike
    /// across layers).
    pub regions: Option<Vec<LayerRegions<'a>>>,
    pub tops: &'a [f64],
    pub heights: &'a [f64],
    /// The object's settings and those of its regions.
    pub settings: Vec<&'a crate::settings::PrintSettings>,
    /// Layers printed with supports that are not on the object's layers.
    pub independent_support_layers: bool,
}

impl Plan {
    /// The plan of an object, `None` when its settings leave brick layers off.
    pub fn build(input: PlanInput<'_>) -> Option<Self> {
        let s = input.settings.first()?;
        if !s.brick.enabled {
            return None;
        }
        let _timing = crate::timing::Span::new(crate::timing::Stage::BrickPlan);
        let n = input.outlines.len();
        let mut warnings = Vec::new();
        let ratio = s.brick.flow_ratio;
        let empty = |warnings: Vec<String>| {
            Some(Self {
                raised: vec![Vec::new(); n],
                widened: vec![Vec::new(); n],
                skipped: vec![Vec::new(); n],
                tops: input.tops.to_vec(),
                raise: vec![0.0; n],
                flow_ratio: ratio,
                warnings,
                suppressed: AtomicBool::new(false),
            })
        };
        if input.independent_support_layers {
            warnings.push(
                "Brick layers are off for this object: its supports print on their own layer \
                 heights."
                    .into(),
            );
            return empty(warnings);
        }
        let perimeters = input
            .settings
            .iter()
            .map(|s| s.perimeters)
            .max()
            .unwrap_or(0);
        if perimeters < 2 {
            warnings
                .push("Brick layers need at least 2 walls; with fewer, no wall is raised.".into());
            return empty(warnings);
        }
        let min_h = input
            .heights
            .iter()
            .skip(1)
            .copied()
            .fold(f64::INFINITY, f64::min)
            .min(s.layer_height)
            .max(1e-3);
        let max_h = input
            .heights
            .iter()
            .skip(1)
            .copied()
            .fold(s.layer_height, f64::max);
        // The tallest bead: a start on the thickest layer.
        let start_span = max_h + min_h / 2.0;
        if start_span > 0.75 * s.nozzle_diameter + 1e-9 {
            warnings.push(format!(
                "Brick layers: where a wall starts to be raised its bead is {start_span:.2} mm \
                 tall, more than 75 % of the nozzle diameter. A lower layer height prints it \
                 better."
            ));
        }
        let margin = s.brick.end_margin_layers;
        let heights = input.heights;
        // Layers a shell of at least `layers` layers and `thickness` mm takes, counted from
        // `from` upward (`up`) or downward from `from - 1`; `None` past the object.
        let shell = |from: usize, up: bool, layers: usize, thickness: f64| -> Option<usize> {
            let mut k = 0;
            let mut sum = 0.0;
            while k < layers || sum < thickness - 1e-9 {
                let i = if up {
                    from + k
                } else {
                    from.checked_sub(k + 1)?
                };
                sum += *heights.get(i)?;
                k += 1;
            }
            Some(k)
        };
        // Per layer: the layers above a raised bead that must not change (the skipped layer,
        // the 1.5-layer bead, the margin, then the top shell) and below it (the margin, then
        // the bottom shell).
        let window = |layer: usize| -> Option<(usize, usize)> {
            let mut ahead = 0;
            let mut behind = 0;
            for s in &input.settings {
                let top = shell(
                    layer + 3 + margin,
                    true,
                    s.top_solid_layers,
                    s.top_solid_min_thickness,
                )?;
                ahead = ahead.max(top + 2 + margin);
                let below = layer.checked_sub(margin)?;
                let bottom = shell(
                    below,
                    false,
                    s.bottom_solid_layers,
                    s.bottom_solid_min_thickness,
                )?;
                behind = behind.max(bottom + margin);
            }
            Some((ahead, behind))
        };
        let width = input
            .settings
            .iter()
            .map(|s| {
                s.perimeter_extrusion_width
                    .max(s.external_perimeter_extrusion_width)
            })
            .fold(0.0, f64::max);
        // How far an outline change reaches into the walls, mm.
        let reach = width * (perimeters as f64 + 1.5);
        // Outlines that move less than a quarter wall width per layer count as unchanged (twice
        // the opening): the walls of the next layer then stand on the same walls, never on a
        // neighbour's raised bead.
        let tolerance = 0.125 * width;

        // Per layer pair (j, j + 1): where outlines or regions differ by more than a sliver.
        let signatures: Vec<LayerRegions<'_>> = match &input.regions {
            Some(r) => r.clone(),
            None => input
                .outlines
                .iter()
                .map(|o| vec![(0, o.as_slice())])
                .collect(),
        };
        let outline = |j: usize| match (j, input.first_layer) {
            (0, Some(first)) => first,
            _ => input.outlines[j].as_slice(),
        };
        let changed: Vec<Vec<ExPolygon>> = (0..n.saturating_sub(1))
            .into_par_iter()
            .map(|j| {
                let mut out = xor(outline(j), outline(j + 1));
                if input.regions.is_some() {
                    let ids: std::collections::BTreeSet<usize> = signatures[j]
                        .iter()
                        .chain(&signatures[j + 1])
                        .map(|(id, _)| *id)
                        .collect();
                    for id in ids {
                        let a = gather(&signatures[j], id);
                        let b = gather(&signatures[j + 1], id);
                        out.extend(xor(&a, &b));
                    }
                }
                let out = opening_ex(&union_ex(&out), scale(tolerance) as f64);
                if out.is_empty() {
                    return out;
                }
                // Everything within reach of the change (square corners: a little more).
                let out = simplify_ex(&out, scale(0.02) as f64);
                offset_ex(&out, scale(reach) as f64, Join::Square)
            })
            .collect();
        let start = s.brick.start_layer.max(1);
        let raised: Vec<Vec<ExPolygon>> = (0..n)
            .into_par_iter()
            .map(|layer| {
                let Some((ahead, behind)) = window(layer) else {
                    return Vec::new();
                };
                if layer < start || layer + ahead >= n || layer < behind {
                    return Vec::new();
                }
                let near: Vec<ExPolygon> = changed[layer - behind..layer + ahead]
                    .iter()
                    .flatten()
                    .cloned()
                    .collect();
                let area = if near.is_empty() {
                    input.outlines[layer].clone()
                } else {
                    diff_ex(&input.outlines[layer], &near)
                };
                simplify_ex(&area, scale(0.01) as f64)
            })
            .collect();
        let snap = scale(0.25 * width) as f64;
        let widen = |a: &[ExPolygon]| {
            if a.is_empty() {
                Vec::new()
            } else {
                offset_ex(a, snap, Join::Miter)
            }
        };
        let widened: Vec<Vec<ExPolygon>> = raised.par_iter().map(|a| widen(a)).collect();
        // Skipped on each layer: the layer below's widened raised area minus this layer's.
        let skipped: Vec<Vec<ExPolygon>> = (0..n)
            .into_par_iter()
            .map(|i| match i.checked_sub(1) {
                Some(j) if !widened[j].is_empty() => widen(&diff_ex(&widened[j], &raised[i])),
                _ => Vec::new(),
            })
            .collect();
        // Raised walls stand half the object's thinnest layer above their layer, on every
        // layer: a raised bead on a raised bead is then exactly as tall as its own layer, with
        // variable layer heights too (product rule 1), and always below the next layer.
        let raise = (0..n)
            .map(|i| if i + 1 < n { min_h / 2.0 } else { 0.0 })
            .collect();
        Some(Self {
            raised,
            widened,
            skipped,
            tops: input.tops.to_vec(),
            raise,
            flow_ratio: ratio,
            warnings,
            suppressed: AtomicBool::new(false),
        })
    }

    /// The print cannot use brick layers (several tools); later toolpaths are plain. Called
    /// before any toolpath of the object is generated.
    pub fn suppress(&self) {
        self.suppressed.store(true, Ordering::SeqCst);
    }

    pub fn suppressed(&self) -> bool {
        self.suppressed.load(Ordering::SeqCst)
    }

    /// Whether any layer has walls to raise.
    pub fn raises_anything(&self) -> bool {
        self.raised.iter().any(|r| !r.is_empty())
    }

    /// The area in which raised walls of object layer `layer` print raised.
    pub fn raised_area(&self, layer: usize) -> &[ExPolygon] {
        self.raised.get(layer).map_or(&[], Vec::as_slice)
    }

    /// Changes object layer `layer`'s paths: raised stretches move to the end, skipped ones
    /// go, spans and flows follow the table in the module documentation. `ids` (one per path,
    /// e.g. region ids) follow the paths.
    pub fn apply<T: Clone>(
        &self,
        layer: usize,
        paths: &mut Vec<ExtrusionPath>,
        mut ids: Option<&mut Vec<T>>,
    ) {
        if self.suppressed() || layer < 1 || layer >= self.raised.len() {
            return;
        }
        let here = &self.raised[layer];
        let below = &self.widened[layer - 1];
        let held_below = &self.skipped[layer - 1];
        if here.is_empty() && below.is_empty() && held_below.is_empty() {
            return;
        }
        let _timing = crate::timing::Span::new(crate::timing::Stage::BrickApply);
        let z = self.tops[layer];
        let raised_top = |j: usize| self.tops[j] + self.raise[j];
        let areas = [Area::new(here), Area::new(below), Area::new(held_below)];
        // Inner walls by parity: odd ones are raised, even ones get the extra flow.
        let inner = |p: &ExtrusionPath| {
            p.perimeter_index
                .filter(|&k| k > 0 && p.role == Role::Perimeter && p.points.len() >= 2)
        };
        let odd: Vec<usize> = (0..paths.len())
            .filter(|&i| inner(&paths[i]).is_some_and(|k| k % 2 == 1))
            .collect();
        let lines: Vec<&[Point]> = odd.iter().map(|&i| paths[i].points.as_slice()).collect();
        let cums: Vec<Vec<f64>> = lines.iter().map(|l| arc_lengths(l)).collect();
        let per_area: Vec<Vec<Vec<(f64, f64)>>> = areas
            .iter()
            .map(|a| a.intervals_many(&lines, &cums))
            .collect();
        let mut pieces_of: Vec<Option<Vec<(f64, f64, Class)>>> = vec![None; paths.len()];
        for (k, &i) in odd.iter().enumerate() {
            let sets = [0, 1, 2].map(|a| per_area[a][k].clone());
            pieces_of[i] = Some(classify_sets(*cums[k].last().unwrap(), &sets));
        }
        let even: Vec<usize> = if self.flow_ratio > 1.0 {
            (0..paths.len())
                .filter(|&i| inner(&paths[i]).is_some_and(|k| k % 2 == 0))
                .collect()
        } else {
            Vec::new()
        };
        let even_lines: Vec<&[Point]> = even.iter().map(|&i| paths[i].points.as_slice()).collect();
        let mut extra = vec![false; paths.len()];
        for (k, touches) in areas[0].touching_many(&even_lines).into_iter().enumerate() {
            extra[even[k]] = touches;
        }
        let mut out = Vec::with_capacity(paths.len());
        let mut out_ids = Vec::new();
        let mut last = Vec::new();
        let mut last_ids = Vec::new();
        for (index, path) in std::mem::take(paths).into_iter().enumerate() {
            let id = ids.as_ref().map(|v| v[index].clone());
            let Some(pieces) = pieces_of[index].take() else {
                // Not a raised wall; an inner wall between raised ones gets the extra flow.
                let mut path = path;
                if extra[index] {
                    path.brick = Some(BrickWall {
                        raise: 0.0,
                        flow_ratio: self.flow_ratio,
                    });
                }
                out.push(path);
                out_ids.extend(id);
                continue;
            };
            if pieces.len() == 1 && pieces[0].2 == Class::Keep {
                out.push(path);
                out_ids.extend(id);
                continue;
            }
            let split = pieces.len() > 1;
            let count = pieces.len();
            for (n, (from, to, class)) in pieces.into_iter().enumerate() {
                let (span, raise) = match class {
                    Class::Keep => (None, 0.0),
                    Class::Skip => continue,
                    Class::Tall => (Some(z - raised_top(layer - 2)), 0.0),
                    Class::Raised(bottom) => {
                        let floor = match bottom {
                            Below::Normal => self.tops[layer - 1],
                            Below::Raised => raised_top(layer - 1),
                            Below::Held => raised_top(layer - 2),
                        };
                        (Some(raised_top(layer) - floor), self.raise[layer])
                    }
                };
                let mut piece = if split {
                    match cut(&path, from, to, n + 1 == count) {
                        Some(p) => p,
                        None => continue,
                    }
                } else {
                    path.clone()
                };
                if let Some(seam) = piece.seam.as_mut() {
                    // A changed wall has no scarf ramp: its Z is the brick layer's.
                    if class != Class::Keep || split {
                        seam.samples.clear();
                        seam.scarf_ranges = None;
                        seam.scarf_speed = None;
                    }
                }
                if let Some(span) = span {
                    piece.flow = Flow::new(path.flow.width, span);
                    piece.overhang = None;
                    piece.brick = Some(BrickWall {
                        raise,
                        flow_ratio: self.flow_ratio,
                    });
                }
                if matches!(class, Class::Raised(_)) {
                    last.push(piece);
                    last_ids.extend(id.clone());
                } else {
                    out.push(piece);
                    out_ids.extend(id.clone());
                }
            }
        }
        out.append(&mut last);
        out_ids.append(&mut last_ids);
        *paths = out;
        if let Some(ids) = ids.as_mut() {
            **ids = out_ids;
        }
    }
}

fn xor(a: &[ExPolygon], b: &[ExPolygon]) -> Vec<ExPolygon> {
    match (a.is_empty(), b.is_empty()) {
        (true, true) => Vec::new(),
        (true, false) => b.to_vec(),
        (false, true) => a.to_vec(),
        _ => {
            let mut d = diff_ex(a, b);
            d.extend(diff_ex(b, a));
            d
        }
    }
}

fn gather(layer: &[(usize, &[ExPolygon])], id: usize) -> Vec<ExPolygon> {
    let parts: Vec<ExPolygon> = layer
        .iter()
        .filter(|(i, _)| *i == id)
        .flat_map(|(_, s)| s.iter().cloned())
        .collect();
    if parts.len() > 1 {
        union_ex(&parts)
    } else {
        parts
    }
}

/// An area with its bounding box, for quick rejection.
struct Area<'a> {
    polygons: &'a [ExPolygon],
    bbox: Option<BoundingBox>,
}

impl<'a> Area<'a> {
    fn new(polygons: &'a [ExPolygon]) -> Self {
        Self {
            polygons,
            bbox: BoundingBox::from_points(polygons.iter().flat_map(|e| e.contour.iter())),
        }
    }

    fn may_touch(&self, line: &[Point]) -> bool {
        let (Some(a), Some(b)) = (self.bbox, BoundingBox::from_points(line)) else {
            return false;
        };
        a.min.x <= b.max.x && b.min.x <= a.max.x && a.min.y <= b.max.y && b.min.y <= a.max.y
    }

    /// The clipped pieces of `lines` inside the area, each with the index of its line.
    fn pieces(&self, lines: &[&[Point]]) -> Vec<(usize, Polyline)> {
        let candidates: Vec<usize> = (0..lines.len())
            .filter(|&k| self.may_touch(lines[k]))
            .collect();
        if candidates.is_empty() {
            return Vec::new();
        }
        let input: Vec<Polyline> = candidates.iter().map(|&k| lines[k].to_vec()).collect();
        let boxes: Vec<Option<BoundingBox>> = candidates
            .iter()
            .map(|&k| BoundingBox::from_points(lines[k]))
            .collect();
        intersection_pl(&input, self.polygons)
            .into_iter()
            .filter(|p| p.len() >= 2)
            .map(|p| {
                // The line the piece came from: the nearest one to its first segment.
                let mid = (
                    (p[0].x as f64 + p[1].x as f64) / 2.0,
                    (p[0].y as f64 + p[1].y as f64) / 2.0,
                );
                let owner = candidates
                    .iter()
                    .zip(&boxes)
                    .filter(|(_, b)| {
                        b.is_some_and(|b| {
                            b.min.x as f64 - 2.0 <= mid.0
                                && mid.0 <= b.max.x as f64 + 2.0
                                && b.min.y as f64 - 2.0 <= mid.1
                                && mid.1 <= b.max.y as f64 + 2.0
                        })
                    })
                    .map(|(&k, _)| {
                        let d = lines[k]
                            .windows(2)
                            .map(|w| distance_to_segment(mid, w[0], w[1]))
                            .fold(f64::INFINITY, f64::min);
                        (d, k)
                    })
                    .min_by(|a, b| a.0.total_cmp(&b.0))
                    .map_or(candidates[0], |(_, k)| k);
                (owner, p)
            })
            .collect()
    }

    /// Whether each of `lines` has a part inside the area.
    fn touching_many(&self, lines: &[&[Point]]) -> Vec<bool> {
        let mut out = vec![false; lines.len()];
        for (k, _) in self.pieces(lines) {
            out[k] = true;
        }
        out
    }

    /// Arc-length intervals (scaled units) of each of `lines` inside the area; `cums` are the
    /// lines' arc lengths at their points.
    fn intervals_many(&self, lines: &[&[Point]], cums: &[Vec<f64>]) -> Vec<Vec<(f64, f64)>> {
        let mut out = vec![Vec::new(); lines.len()];
        for (k, p) in self.pieces(lines) {
            let (line, cum) = (lines[k], &cums[k]);
            let a = locate(line, cum, p[0], p[1]);
            let b = locate(line, cum, p[p.len() - 1], p[p.len() - 2]);
            out[k].push((a.min(b), a.max(b)));
        }
        for v in &mut out {
            v.sort_by(|a, b| a.0.total_cmp(&b.0));
        }
        out
    }
}

/// Arc length along `line` of `p`, an end of a clipped piece whose next point is `q` (the
/// piece's first segment pins down which segment of `line` it lies on).
fn locate(line: &[Point], cum: &[f64], p: Point, q: Point) -> f64 {
    let mid = (
        (p.x as f64 + q.x as f64) / 2.0,
        (p.y as f64 + q.y as f64) / 2.0,
    );
    let mut best = (f64::INFINITY, 0usize);
    for k in 0..line.len() - 1 {
        let d = distance_to_segment(mid, line[k], line[k + 1]);
        if d < best.0 {
            best = (d, k);
        }
    }
    let k = best.1;
    let (a, b) = (line[k], line[k + 1]);
    let (dx, dy) = ((b.x - a.x) as f64, (b.y - a.y) as f64);
    let len2 = dx * dx + dy * dy;
    let t = if len2 > 0.0 {
        (((p.x - a.x) as f64 * dx + (p.y - a.y) as f64 * dy) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    cum[k] + t * (cum[k + 1] - cum[k])
}

fn distance_to_segment(p: (f64, f64), a: Point, b: Point) -> f64 {
    let (ax, ay, bx, by) = (a.x as f64, a.y as f64, b.x as f64, b.y as f64);
    let (dx, dy) = (bx - ax, by - ay);
    let len2 = dx * dx + dy * dy;
    let t = if len2 > 0.0 {
        (((p.0 - ax) * dx + (p.1 - ay) * dy) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (p.0 - ax - t * dx).hypot(p.1 - ay - t * dy)
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Below {
    /// On the layer grid.
    Normal,
    /// Raised on the layer below.
    Raised,
    /// Skipped on the layer below, raised on the one before.
    Held,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Class {
    Keep,
    Raised(Below),
    Skip,
    Tall,
}

fn inside(intervals: &[(f64, f64)], s: f64) -> bool {
    intervals.iter().any(|&(a, b)| a <= s && s <= b)
}

/// The stretches of `line` (arc-length ranges in scaled units) and what happens to each.
/// `areas`: raised here, raised on the layer below, skipped on the layer below.
#[cfg(test)]
fn classify(line: &[Point], areas: &[Area<'_>; 3]) -> Vec<(f64, f64, Class)> {
    let cum = arc_lengths(line);
    let cums = std::slice::from_ref(&cum);
    let sets = [0, 1, 2].map(|a| areas[a].intervals_many(&[line], cums).remove(0));
    classify_sets(*cum.last().unwrap(), &sets)
}

/// Arc length at each point of `line` (scaled units).
fn arc_lengths(line: &[Point]) -> Vec<f64> {
    let mut cum = Vec::with_capacity(line.len());
    cum.push(0.0);
    for w in line.windows(2) {
        cum.push(cum.last().unwrap() + w[0].distance_to(w[1]));
    }
    cum
}

/// The stretches of a line `length` long and their class, from its intervals inside the
/// three areas (raised here, raised below, skipped below).
fn classify_sets(length: f64, sets: &[Vec<(f64, f64)>; 3]) -> Vec<(f64, f64, Class)> {
    let mut cuts: Vec<f64> = vec![0.0, length];
    for set in sets {
        for &(a, b) in set {
            cuts.push(a.clamp(0.0, length));
            cuts.push(b.clamp(0.0, length));
        }
    }
    cuts.sort_by(f64::total_cmp);
    // Stretches shorter than 1 µm are merged into their neighbours.
    cuts.dedup_by(|b, a| (*b - *a).abs() < 1.0);
    if let Some(last) = cuts.last_mut() {
        *last = length;
    }
    let mut out: Vec<(f64, f64, Class)> = Vec::new();
    for w in cuts.windows(2) {
        let mid = (w[0] + w[1]) / 2.0;
        let [here, below, held] = [0, 1, 2].map(|i| inside(&sets[i], mid));
        let class = match (here, below, held) {
            (true, true, _) => Class::Raised(Below::Raised),
            (true, false, true) => Class::Raised(Below::Held),
            (true, false, false) => Class::Raised(Below::Normal),
            (false, true, _) => Class::Skip,
            (false, false, true) => Class::Tall,
            (false, false, false) => Class::Keep,
        };
        match out.last_mut() {
            Some(prev) if prev.2 == class => prev.1 = w[1],
            _ => out.push((w[0], w[1], class)),
        }
    }
    if out.is_empty() {
        out.push((0.0, length, Class::Keep));
    }
    out
}

/// The stretch `from..to` (arc length, scaled units) of `path` as its own path. Only the
/// stretch that ends the path keeps the seam's inward wipe.
fn cut(path: &ExtrusionPath, from: f64, to: f64, keeps_end: bool) -> Option<ExtrusionPath> {
    let line = &path.points;
    let mut points: Polyline = Vec::new();
    let mut walked = 0.0;
    let at = |a: Point, b: Point, t: f64| {
        Point::new(
            a.x + ((b.x - a.x) as f64 * t).round() as i64,
            a.y + ((b.y - a.y) as f64 * t).round() as i64,
        )
    };
    let push = |p: Point, points: &mut Polyline| {
        if points.last() != Some(&p) {
            points.push(p);
        }
    };
    for w in line.windows(2) {
        let len = w[0].distance_to(w[1]);
        let (s0, s1) = (walked, walked + len);
        walked = s1;
        if s1 < from || s0 > to || len <= 0.0 {
            continue;
        }
        let start = if s0 >= from {
            w[0]
        } else {
            at(w[0], w[1], (from - s0) / len)
        };
        let end = if s1 <= to {
            w[1]
        } else {
            at(w[0], w[1], (to - s0) / len)
        };
        push(start, &mut points);
        push(end, &mut points);
    }
    if points.len() < 2 {
        return None;
    }
    let mut seam = path.seam.clone();
    if let Some(s) = seam.as_mut()
        && !keeps_end
    {
        s.wipe = None;
    }
    Some(ExtrusionPath {
        role: path.role,
        perimeter_index: path.perimeter_index,
        points,
        flow: path.flow,
        seam,
        overhang: None,
        brick: path.brick,
    })
}

#[cfg(test)]
mod tests;
