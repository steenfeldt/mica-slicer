// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/WipeTower.cpp :: prime, toolchange_Unload, toolchange_Wipe, finish_layer, get_wipe_tower_cone_base, WipeTowerWriter
// Source: orca@8500fcdcca src/libslic3r/GCode/WipeTower2.cpp :: generate_support_rib_wall, generate_rib_polygon, rounding_polygon, toolchange_Wipe
// Changes: Physical-tool ramming and volume moves; geometry uses Mica's Clipper2 and monotonic filler. No MMU loading/cooling or tower ironing. One layer at a time.
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/WipeTower.cpp :: WipeTower::generate, WipeTower::save_on_last_wipe
// Changes: finish_layer starts in the corner m_left_to_right gives after the first non-soluble toolchange (state kept across layers;
// Changes: below the first toolchange layer, as save_on_last_wipe's last pass leaves it).
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/WipeTower.cpp :: WipeTowerWriter::add_wipe_point, WipeTower::construct_tcr, merge_tcr
// Changes: Visits carry the transformed wipe path of toolchange_Wipe or finish_layer (the merged result keeps the second's).
// Source: orca@8500fcdcca src/libslic3r/GCode/WipeTower2.cpp :: merge_tcr, WipeTower2::construct_tcr
// Changes: Blocks also record the merge point and tool_change_start_pos for Orca's post-processed tower G-code.

// Copyright (c) Prusa Research 2017–2023 Lukáš Matěna, Vojtěch Bubník, Enrico Turri.
// Copyright (c) SuperSlicer 2023 Remi Durand; 2020 Paul Arden; 2019 Thomas Moore.
// Upstream is AGPLv3 or higher; this port is AGPL-3.0-only.

use super::*;
use crate::fill::{FillParams, Pattern, fill};
use crate::geometry::clip::{Join, diff_ex, offset_ex, union_ex};
use crate::geometry::{ExPolygon, Polygon, scale, unscale};
use std::f64::consts::PI;

struct Builder<'a> {
    plan: &'a TowerPlan,
    layer: usize,
    shift: f64,
    pos: [f64; 2],
    block: TowerBlock,
    flow: f64,
    width: f64,
    speed: f64,
    priming: bool,
    /// `WipeTowerWriter::add_wipe_point`s, already transformed (`ToolChangeResult::wipe_path`).
    wipe: Vec<[f64; 2]>,
    /// The last polygon extruded by `polygon`, in tower coordinates.
    last_polygon: Vec<[f64; 2]>,
}
impl<'a> Builder<'a> {
    fn new(plan: &'a TowerPlan, layer: usize, start: [f64; 2], shift: f64) -> Self {
        let h = plan.layers[layer].height;
        let mut b = Self {
            plan,
            layer,
            shift,
            pos: start,
            block: TowerBlock::default(),
            flow: area(plan.settings.line_width, h),
            width: plan.settings.line_width,
            speed: plan.settings.travel_speed,
            priming: false,
            wipe: vec![],
            last_polygon: vec![],
        };
        b.block.start = b.transform(start);
        b
    }
    fn transform(&self, mut p: [f64; 2]) -> [f64; 2] {
        let s = &self.plan.settings;
        if self.priming {
            return [s.position[0] + p[0], s.position[1] + p[1]];
        }
        p[1] += self.shift;
        if self.layer.is_multiple_of(2) {
            p = [s.width - p[0], self.plan.depth - p[1]];
        }
        let (sin, cos) = s.rotation.to_radians().sin_cos();
        [
            s.position[0] + cos * p[0] - sin * p[1],
            s.position[1] + sin * p[0] + cos * p[1],
        ]
    }
    fn go(&mut self, p: [f64; 2], volume: f64, speed: f64, ramming: bool) {
        if speed > 0. {
            self.speed = speed;
        }
        if distance(self.pos, p) > 1e-8 || volume > 0. {
            self.block.moves.push(TowerMove {
                xy: self.transform(p),
                volume,
                speed: self.speed,
                width: self.width,
                ramming,
            });
        }
        self.pos = p;
    }
    fn travel(&mut self, p: [f64; 2], speed: f64) {
        self.go(p, 0., speed, false);
    }
    fn extrude(&mut self, p: [f64; 2], speed: f64) {
        self.go(p, distance(self.pos, p) * self.flow, speed, false);
    }
    fn polygon(&mut self, poly: &Polygon, speed: f64) {
        if poly.is_empty() {
            return;
        }
        let points: Vec<_> = poly.iter().map(|p| [unscale(p.x), unscale(p.y)]).collect();
        let i = (0..points.len())
            .min_by(|&i, &j| {
                distance(self.pos, points[i]).total_cmp(&distance(self.pos, points[j]))
            })
            .unwrap();
        self.travel(points[i], 0.);
        for j in 1..=points.len() {
            self.extrude(points[(i + j) % points.len()], speed);
        }
        self.last_polygon = points;
    }
    fn rectangle(&mut self, lo: [f64; 2], hi: [f64; 2], speed: f64) {
        let ps = [lo, [hi[0], lo[1]], hi, [lo[0], hi[1]]];
        let i = match (
            self.pos[0] - lo[0] > hi[0] - self.pos[0],
            self.pos[1] - lo[1] > hi[1] - self.pos[1],
        ) {
            (false, false) => 0,
            (true, false) => 1,
            (true, true) => 2,
            (false, true) => 3,
        };
        self.travel([ps[i][0], self.pos[1]], 0.);
        self.travel(ps[i], 0.);
        for j in 1..=4 {
            self.extrude(ps[(i + j) % 4], speed);
        }
    }
}
fn distance(a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}
fn poly(ps: impl IntoIterator<Item = [f64; 2]>) -> Polygon {
    ps.into_iter().map(|p| Point::from_mm(p[0], p[1])).collect()
}
fn ex(p: Polygon) -> ExPolygon {
    ExPolygon {
        contour: p,
        holes: vec![],
    }
}
fn rectangle(lo: [f64; 2], hi: [f64; 2]) -> Polygon {
    poly([lo, [hi[0], lo[1]], hi, [lo[0], hi[1]]])
}
fn base_shift(p: &TowerPlan, i: usize) -> f64 {
    (p.depth - p.layers[i].depth - p.settings.line_width) / 2.
}
fn reverse(p: &TowerPlan, i: usize) -> bool {
    p.settings.kind == TowerKind::Prusa && !i.is_multiple_of(2)
}

pub(super) fn visits(p: &TowerPlan, i: usize) -> Vec<TowerVisit> {
    let Some(l) = p.layers.get(i) else {
        return vec![];
    };
    if l.skipped {
        return vec![];
    }
    let idx = l
        .changes
        .iter()
        .position(|t| !p.settings.tools[t.new_tool].soluble);
    // WipeTower::generate runs finish_layer right after the first toolchange to a non-soluble
    // tool (or first, if there is none), starting in the corner given by m_left_to_right at
    // that moment. That state persists over layers.
    let mut blocks = Vec::new();
    let mut traversed = 0.;
    let mut left_to_right = true;
    let mut finish_direction = None;
    for j in 0..l.changes.len() {
        blocks.push(change(p, i, j, traversed, &mut left_to_right));
        traversed += l.changes[j].required_depth;
        if idx == Some(j) {
            finish_direction = Some(after_wipe(l, j, left_to_right));
        }
    }
    let finish_direction = finish_direction.unwrap_or_else(|| previous_direction(p, i));
    let mut finished = Some(finish(p, i, finish_direction));
    let mut result = Vec::new();
    for ((j, t), (before, after, wipe_path)) in l.changes.iter().enumerate().zip(blocks) {
        let mut visit = TowerVisit {
            old_tool: t.old_tool,
            tool: t.new_tool,
            z: l.tower_z,
            height: l.height,
            before,
            after,
            change: true,
            wipe_path,
        };
        if idx == Some(j) {
            // merge_tcr(toolchange, finish_layer) keeps the second wipe path.
            let (block, wipe_path) = finished.take().unwrap();
            append(&mut visit.after, block);
            visit.wipe_path = wipe_path;
        }
        if idx.is_none() && j == 0 {
            let (mut f, _) = finished.take().unwrap();
            if let Some(before) = visit.before.take() {
                append(&mut f, before);
            }
            visit.before = Some(f);
        }
        result.push(visit);
    }
    if let Some((after, wipe_path)) = finished {
        result.push(TowerVisit {
            old_tool: l.start_tool,
            tool: l.start_tool,
            z: l.tower_z,
            height: l.height,
            before: None,
            after,
            change: false,
            wipe_path,
        });
    }
    result
}
/// `m_left_to_right` at the start of layer `i`: as the last wipe of the nearest lower
/// tower layer with toolchanges left it. Below the first such layer, PrusaSlicer's state is
/// what `WipeTower::save_on_last_wipe` left: its passes run `tool_change` over every layer
/// before `generate`, so the topmost layer with toolchanges decides (upstream initializes it
/// to true).
fn previous_direction(p: &TowerPlan, i: usize) -> bool {
    let below = (0..i).rev();
    let planned = (i..p.layers.len())
        .rev()
        .filter(|_| p.settings.kind == TowerKind::Prusa);
    for k in below.chain(planned) {
        let l = &p.layers[k];
        if l.skipped || l.changes.is_empty() {
            continue;
        }
        let mut left = true;
        let mut traversed = 0.;
        for (j, t) in l.changes.iter().enumerate() {
            change(p, k, j, traversed, &mut left);
            traversed += t.required_depth;
        }
        return after_wipe(l, l.changes.len() - 1, left);
    }
    true
}
/// The end of toolchange_Wipe flips m_left_to_right unless the new tool is the layer's last.
fn after_wipe(l: &TowerLayer, j: usize, left_to_right: bool) -> bool {
    if l.changes
        .last()
        .is_some_and(|t| t.new_tool != l.changes[j].new_tool)
    {
        !left_to_right
    } else {
        left_to_right
    }
}
fn append(a: &mut TowerBlock, b: TowerBlock) {
    // merge_tcr: the joined result travels to the second's start.
    a.joined = Some(a.moves.len());
    a.moves.push(TowerMove {
        xy: b.start,
        volume: 0.,
        speed: 120.,
        width: 0.,
        ramming: false,
    });
    a.moves.extend(b.moves);
}
fn change(
    p: &TowerPlan,
    i: usize,
    j: usize,
    traversed: f64,
    left: &mut bool,
) -> (Option<TowerBlock>, TowerBlock, Vec<[f64; 2]>) {
    let s = &p.settings;
    let l = &p.layers[i];
    let t = &l.changes[j];
    let pw = s.line_width;
    let shift = base_shift(p, i)
        + if reverse(p, i) {
            l.depth - l.changes_depth()
        } else {
            0.
        };
    let start = [pw / 2., pw / 2. + traversed];
    let mut b = Builder::new(p, i, start, shift);
    let ram = ram(&mut b, t.old_tool, traversed, Some(j), left);
    let before = ram.then(|| b.block.clone());
    let mut wipe = Builder::new(p, i, b.pos, shift);
    wipe.block.entry = Some(b.transform(start));
    wipe.travel([wipe.pos[0], wipe.pos[1] - pw], 0.);
    wipe_lines(&mut wipe, t.wipe_volume, t.required_depth + traversed, left);
    (before, wipe.block, wipe.wipe)
}
fn ram(
    b: &mut Builder<'_>,
    tool: usize,
    traversed: f64,
    change: Option<usize>,
    left: &mut bool,
) -> bool {
    let p = b.plan;
    let l = &p.layers[b.layer];
    let s = &p.settings;
    let pw = s.line_width;
    let t = &s.tools[tool];
    let line = 2. * pw;
    let step = line * s.extra_spacing;
    let xl = 1.5 * pw;
    let xr = s.width - 1.5 * pw;
    let start = [xl, pw / 2. + traversed + step / 2.];
    *left = true;
    if t.ram {
        b.travel(start, 0.);
    } else {
        b.pos = start;
    }
    let mut remaining = xr - xl;
    if t.ram
        && b.layer > 1
        && let Some(j) = change
    {
        let prev = &p.layers[b.layer - 1];
        let sparse = if reverse(p, b.layer) {
            prev.depth - prev.changes_depth() - (l.depth - l.changes_depth())
        } else {
            prev.changes_depth() + pw
        };
        let t = &l.changes[j];
        let end_y = traversed + t.ramming_depth - (line - pw) / 2.;
        if (reverse(p, b.layer) && end_y < sparse - pw / 2.)
            || (!reverse(p, b.layer) && end_y > sparse + pw / 2.)
        {
            let dx = t.first_wipe_line - pw;
            b.extrude([xl + dx, b.pos[1]], 0.);
            remaining -= dx;
        }
    }
    if t.ram {
        let length = t.ram_volume / area(line, l.height);
        let mut todo = length;
        b.width = line;
        while todo > 1e-6 {
            let d = todo.min(remaining).max(0.);
            let speed = t.ram_flow / area(line, l.height);
            b.go(
                [b.pos[0] + if *left { d } else { -d }, b.pos[1]],
                d * area(line, l.height),
                speed,
                true,
            );
            todo -= d;
            remaining -= d;
            if remaining < 1e-3 {
                b.travel([b.pos[0], b.pos[1] + step], 120.);
                *left = !*left;
                remaining = xr - xl;
            }
        }
        b.width = pw;
    }
    let end = [b.pos[0], b.pos[1] + (line - pw) / 2. + pw];
    if t.ram {
        b.travel(end, 40.);
    } else {
        b.pos = end;
    }
    t.ram
}
fn wipe_lines(b: &mut Builder<'_>, volume: f64, top: f64, left: &mut bool) {
    let s = &b.plan.settings;
    let h = b.plan.layers[b.layer].height;
    let pw = s.line_width;
    let xl = pw / 2.;
    let xr = s.width - pw / 2.;
    let line = pw * s.extra_flow;
    let first = b.layer == 0 && !b.priming;
    let dy = if first {
        s.extra_flow
    } else {
        s.extra_flow * s.extra_spacing
    } * pw;
    let mut length = (volume / area(pw, h)).max(0.) / s.extra_flow;
    b.flow = area(pw, h) * s.extra_flow;
    b.width = line;
    if first && s.kind == TowerKind::Prusa {
        let count = ((top - 0.5 * line - 1e-6 - (b.pos[1] + dy)) / dy).floor() + 1.;
        length = length.max((xr - xl) * count);
    }
    let target = if first { s.first_speed } else { s.infill_speed };
    let mut speed = 0.33 * target;
    if (if *left { xr - b.pos[0] } else { b.pos[0] - xl }) < 2.5 * line {
        b.travel(
            [if *left { xr - line } else { xl + line }, b.pos[1] + dy],
            0.,
        );
        *left = !*left;
    }
    for k in 0..100000 {
        if k > 0 {
            speed = if speed < 0.34 * target {
                0.375 * target
            } else if speed < 0.377 * target {
                0.458 * target
            } else if speed < 0.46 * target {
                0.875 * target
            } else {
                (speed + 50. / 60.).min(target)
            };
        }
        let x = b.pos[0];
        b.extrude(
            [
                if *left {
                    xr - if k % 4 == 0 { 0. } else { 1.5 * line }
                } else {
                    xl + if k % 4 == 1 { 0. } else { 1.5 * line }
                },
                b.pos[1],
            ],
            speed,
        );
        if b.pos[1] + 1e-6 > top - 0.5 * line {
            break;
        }
        length -= (x - b.pos[0]).abs();
        if length < 1e-3 {
            b.travel(
                [
                    if *left {
                        xl + 1.5 * line
                    } else {
                        xr - 1.5 * line
                    },
                    b.pos[1],
                ],
                120.,
            );
            break;
        }
        b.extrude(
            [
                b.pos[0]
                    + (match k % 4 {
                        0 => -1.,
                        1 => 1.,
                        _ => 0.,
                    }) * 1.5
                        * line,
                b.pos[1] + dy,
            ],
            0.,
        );
        *left = !*left;
    }
    // "We may be going back to the model - wipe the nozzle. If this is followed by
    // finish_layer, this wipe path will be overwritten."
    let (x, y) = (b.pos[0], b.pos[1]);
    b.wipe = vec![
        b.transform([x, y]),
        b.transform([x, y - dy]),
        b.transform([if *left { 0. } else { s.width }, y - dy]),
    ];
    b.width = pw;
    b.flow = area(pw, h);
}
fn finish(p: &TowerPlan, i: usize, left_to_right: bool) -> (TowerBlock, Vec<[f64; 2]>) {
    let l = &p.layers[i];
    let s = &p.settings;
    let pw = s.line_width;
    let cd = l.depth - l.changes_depth();
    let lo = [pw, l.changes_depth() + pw];
    let hi = [s.width - pw, l.depth];
    let shift = base_shift(p, i) - if reverse(p, i) { l.changes_depth() } else { 0. };
    // fill_box.ru or fill_box.lu, so there is never a diagonal travel.
    let start = if left_to_right { hi } else { [lo[0], hi[1]] };
    let mut b = Builder::new(p, i, start, shift);
    let speed = if i == 0 {
        s.first_speed
    } else {
        s.infill_speed
    };
    if cd - pw > pw - 1e-3 {
        b.rectangle(lo, hi, speed);
    }
    if b.pos[0] > lo[0] + 1e-6 {
        b.travel([lo[0], b.pos[1]], 0.);
    }
    if b.pos[1] > lo[1] + 1e-6 {
        b.travel([b.pos[0], lo[1]], 0.);
    }
    let dy = hi[1] - lo[1] - pw;
    let mut left = lo[0] + 2. * pw;
    let mut right = hi[0] - 2. * pw;
    if dy > pw {
        b.travel([left, lo[1]], 0.);
        let solid = i == 0
            || p.layers.get(i + 1).is_some_and(|l| {
                l.changes
                    .iter()
                    .any(|t| s.tools[t.old_tool].soluble || s.tools[t.new_tool].soluble)
            });
        if solid {
            let factor = if i == 0 {
                left -= pw;
                right += pw;
                1.
            } else {
                1.5
            };
            let n = (dy / (pw * factor)) as usize;
            let spacing = if n > 1 {
                (dy - pw) / (n - 1) as f64
            } else {
                0.
            };
            for j in 0..n {
                let y = lo[1] + pw + j as f64 * spacing;
                b.extrude([b.pos[0], y], speed);
                b.extrude([if j % 2 == 0 { right } else { left }, y], 0.);
            }
            b.extrude([b.pos[0], hi[1]], 0.);
        } else {
            b.extrude([left, hi[1]], speed);
            let n = 1 + ((right - left) / s.bridging) as usize;
            let dx = (right - left) / n as f64;
            for j in 1..=n {
                b.travel([left + dx * j as f64, b.pos[1]], 0.);
                b.extrude([b.pos[0], if j % 2 == 1 { lo[1] } else { hi[1] }], 0.);
            }
        }
    }
    let spacing = pw - l.height * (1. - std::f64::consts::FRAC_PI_4);
    let speed = if i == 0 {
        s.first_speed
    } else {
        s.perimeter_speed
    };
    let wt_y = if s.wall == Wall::Cone && reverse(p, i) {
        l.changes_depth()
    } else {
        0.
    };
    let lo = [0., wt_y];
    let hi = [s.width, wt_y + l.depth + pw];
    let mut boundary = match s.wall {
        Wall::Cone => cone(p, i, lo, hi),
        Wall::Rib => rib(p, i, lo, hi),
        Wall::Rectangle => rectangle(lo, hi),
    };
    b.polygon(&boundary, speed);
    if i == 0 && s.wall == Wall::Cone {
        let contour = offset_ex(
            &[ex(boundary.clone())],
            -scale(spacing / 2.) as f64,
            Join::Miter,
        );
        let rect = offset_ex(
            &[ex(rectangle(lo, hi))],
            -scale(spacing / 2.) as f64,
            Join::Miter,
        );
        let areas = diff_ex(&contour, &rect);
        if areas.len() == 2 {
            let params = FillParams {
                spacing,
                density: 1.,
                angle: 45.,
                layer_id: 0,
                z: l.tower_z,
                anchor_length: 0.,
                anchor_length_max: 0.,
                dont_adjust: false,
                link_max_length: 0.,
                origin: Point::new(0, 0),
            };
            for filled in fill(Pattern::MonotonicLines, &areas, &params) {
                for line in filled.polylines {
                    if let Some(a) = line.first() {
                        b.travel([unscale(a.x), unscale(a.y)], 0.);
                        for pt in &line[1..] {
                            b.extrude([unscale(pt.x), unscale(pt.y)], speed);
                        }
                    }
                }
            }
        }
    }
    if i == 0 {
        let loops = ((s.brim + spacing / 2.) / spacing) as usize;
        for _ in 0..loops {
            let Some(polygon) = offset_ex(&[ex(boundary)], scale(spacing) as f64, Join::Miter)
                .into_iter()
                .next()
            else {
                break;
            };
            boundary = polygon.contour;
            b.polygon(&boundary, speed);
        }
    }
    // "Now prepare future wipe": back along the outermost polygon, to the vertex before the
    // one closest to the end position.
    let points = &b.last_polygon;
    if let Some(i) = (0..points.len())
        .min_by(|&i, &j| distance(b.pos, points[i]).total_cmp(&distance(b.pos, points[j])))
    {
        let previous = points[if i == 0 { points.len() - 1 } else { i - 1 }];
        b.wipe = vec![b.transform(b.pos), b.transform(previous)];
    }
    (b.block, b.wipe)
}
fn cone(p: &TowerPlan, i: usize, lo: [f64; 2], hi: [f64; 2]) -> Polygon {
    let s = &p.settings;
    let r0 = (s.cone_angle.to_radians() / 2.).tan() * p.height;
    let fake_width = 0.66 * s.width;
    let diag = (fake_width / 2.).hypot(p.depth / 2.);
    let scale_x = if r0 > diag {
        let t = (r0 - diag) * 0.5 * p.depth / diag;
        let tan = p.depth / fake_width;
        (fake_width / 2. + t / tan + t * tan) / (fake_width / 2.)
    } else {
        1.
    };
    let r = (s.cone_angle.to_radians() / 2.).tan() * (p.height - p.layers[i].tower_z);
    let center = [(lo[0] + hi[0]) / 2., (lo[1] + hi[1]) / 2.];
    let half = (hi[1] - lo[1]) / 2.;
    let mut pts = vec![hi];
    if r > half + 0.01 {
        let a = (half / r).asin();
        for j in 0..=40 {
            let alpha = a + (PI - 2. * a) * j as f64 / 40.;
            pts.push([
                center[0] + r * alpha.cos() / scale_x,
                center[1] + r * alpha.sin(),
            ]);
        }
    }
    let arcs = pts.len() - 1;
    pts.push([lo[0], hi[1]]);
    pts.push(lo);
    for j in (1..=arcs).rev() {
        pts.push([pts[j][0], 2. * center[1] - pts[j][1]]);
    }
    pts.push([hi[0], lo[1]]);
    poly(pts)
}
fn rib(p: &TowerPlan, i: usize, lo: [f64; 2], hi: [f64; 2]) -> Polygon {
    let s = &p.settings;
    let width = s.rib_width.min(p.depth.min(s.width) / 2.) / 2.;
    let len = s.width.hypot(p.depth);
    let extra = s.rib_extra / 2. * (p.height - p.layers[i].tower_z).abs() / p.height;
    let yshift = base_shift(p, i);
    let mut shapes = vec![ex(rectangle(lo, hi))];
    for (a, b) in [
        ([0., 0.], [s.width, p.depth]),
        ([s.width, 0.], [0., p.depth]),
    ] {
        let u = [(b[0] - a[0]) / len, (b[1] - a[1]) / len];
        let v = [-u[1] * width, u[0] * width];
        let a = [a[0] - u[0] * extra, a[1] - u[1] * extra - yshift];
        let b = [b[0] + u[0] * extra, b[1] + u[1] * extra - yshift];
        shapes.push(ex(poly([
            [a[0] + v[0], a[1] + v[1]],
            [a[0] - v[0], a[1] - v[1]],
            [b[0] - v[0], b[1] - v[1]],
            [b[0] + v[0], b[1] + v[1]],
        ])));
    }
    let boundary = union_ex(&shapes).remove(0).contour;
    if s.fillet {
        union_ex(&[ex(rounding(&boundary)), ex(rectangle(lo, hi))])
            .remove(0)
            .contour
    } else {
        boundary
    }
}
fn rounding(p: &Polygon) -> Polygon {
    let ps: Vec<_> = p.iter().map(|p| [unscale(p.x), unscale(p.y)]).collect();
    let mut out = vec![];
    for i in 0..ps.len() {
        let a = ps[(i + ps.len() - 1) % ps.len()];
        let b = ps[i];
        let c = ps[(i + 1) % ps.len()];
        let ab = distance(a, b);
        let bc = distance(b, c);
        let u = [(b[0] - a[0]) / ab, (b[1] - a[1]) / ab];
        let v = [(c[0] - b[0]) / bc, (c[1] - b[1]) / bc];
        let cos = (u[0] * v[0] + u[1] * v[1]).clamp(-1., 1.);
        if cos.abs() < 30_f64.to_radians().cos() {
            let d = 2_f64.min(ab / 2.1).min(bc / 2.1);
            let left = [b[0] - u[0] * d, b[1] - u[1] * d];
            let right = [b[0] + v[0] * d, b[1] + v[1] * d];
            let sign = if u[0] * v[1] - u[1] * v[0] > 0. {
                1.
            } else {
                -1.
            };
            let len = distance(left, right);
            let dis = d / (cos.acos() / 2.).sin();
            let center = [
                b[0] - (right[1] - left[1]) / len * dis * sign,
                b[1] + (right[0] - left[0]) / len * dis * sign,
            ];
            let r = distance(left, center);
            let start = (left[1] - center[1]).atan2(left[0] - center[0]);
            let end = (right[1] - center[1]).atan2(right[0] - center[0]);
            let angle = if sign > 0. {
                (end - start).rem_euclid(2. * PI)
            } else {
                -(start - end).rem_euclid(2. * PI)
            };
            for j in 0..20 {
                let a = start + angle * j as f64 / 20.;
                out.push([center[0] + r * a.cos(), center[1] + r * a.sin()]);
            }
            out.push(right);
        } else {
            out.push(b);
        }
    }
    poly(out)
}
pub(super) fn finish_volume(p: &TowerPlan, i: usize) -> f64 {
    finish(p, i, true).0.moves.iter().map(|m| m.volume).sum()
}
pub(super) fn final_ram(p: &TowerPlan, tool: usize, z: f64, height: f64) -> Option<TowerVisit> {
    if !p.settings.tools[tool].ram {
        return None;
    }
    // Final unloading is above the completed tower; no fresh tool is selected.
    let mut temp = p.clone();
    temp.layers = vec![TowerLayer {
        print_z: z,
        tower_z: z,
        height,
        depth: p.depth - p.settings.line_width,
        changes: vec![],
        start_tool: tool,
        skipped: false,
    }];
    let pw = p.settings.line_width;
    let mut b = Builder::new(&temp, 0, [pw / 2., pw / 2.], 0.);
    ram(&mut b, tool, 0., None, &mut true);
    Some(TowerVisit {
        old_tool: tool,
        tool,
        z,
        height,
        before: Some(b.block.clone()),
        after: TowerBlock {
            start: b.transform(b.pos),
            ..TowerBlock::default()
        },
        change: false,
        // toolchange_Unload adds no wipe points.
        wipe_path: vec![],
    })
}

/// Bed-front priming, in upstream order (the first printing tool is primed last).
pub(super) fn prime(p: &TowerPlan) -> Vec<TowerVisit> {
    if p.priming_tools.is_empty() {
        return vec![];
    }
    let bed_width = p.bed_bounds[1][0] - p.bed_bounds[0][0];
    let section = (0.9 * bed_width / p.priming_tools.len() as f64).min(60.);
    let pw = p.settings.line_width;
    let mut temp = TowerPlan {
        settings: p.settings.clone(),
        layers: vec![p.layers[0].clone()],
        depth: p.depth,
        height: p.height,
        priming_tools: vec![],
        bed_bounds: p.bed_bounds,
        job_layers: vec![Some(0)],
    };
    temp.settings.width = section + pw;
    let mut out = vec![];
    let mut previous = p.priming_tools[0];
    for (i, &tool) in p.priming_tools.iter().enumerate() {
        temp.settings.position = [
            p.bed_bounds[0][0] + 0.02 * bed_width + section * i as f64 - pw / 2.,
            p.bed_bounds[0][1] + 0.01,
        ];
        let start = [pw / 2., pw / 2.];
        let mut b = Builder::new(&temp, 0, start, 0.);
        b.priming = true;
        b.block.start = b.transform(start);
        let last = i + 1 == p.priming_tools.len();
        let volume = if last {
            if p.settings.kind == TowerKind::Prusa {
                p.settings.tools[tool].min_purge
            } else {
                p.settings.prime_volume
            }
        } else {
            20.
        };
        let mut left = true;
        wipe_lines(&mut b, volume, 100. + pw / 2., &mut left);
        if !last {
            let traversed = b.pos[1] + pw / 2.;
            ram(&mut b, tool, traversed, None, &mut left);
        }
        out.push(TowerVisit {
            old_tool: previous,
            tool,
            z: p.layers[0].height,
            height: p.layers[0].height,
            before: None,
            after: b.block,
            change: true,
            wipe_path: b.wipe,
        });
        previous = tool;
    }
    out
}
