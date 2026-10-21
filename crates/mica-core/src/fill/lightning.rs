// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/Fill/Lightning/Generator.cpp :: Generator::generateInitialInternalOverhangs, Generator::generateTrees
// Source: prusaslicer@b028299c77 src/libslic3r/Fill/Lightning/DistanceField.cpp :: DistanceField::DistanceField, DistanceField::update
// Source: prusaslicer@b028299c77 src/libslic3r/Fill/Lightning/Layer.cpp :: Layer::generateNewTrees, Layer::getBestGroundingLocation, Layer::reconnectRoots
// Source: prusaslicer@b028299c77 src/libslic3r/Fill/Lightning/TreeNode.cpp :: Node::prune, Node::straighten, Node::realign, Node::convertToPolylines
// Source: prusaslicer@b028299c77 src/libslic3r/Fill/FillRectilinear.cpp :: sample_grid_pattern
// Changes: Compact parent-index records retained per layer; one mutable forest while preparing.
// Changes: Deterministic branch order, stable merge ordering for the distance heuristic, Clipper2 visibility and realignment.
// Copyright (c) 2021 Ultimaker B.V. CuraEngine is released under the terms of the AGPLv3 or higher.

use super::{base::lerp, concentric::clip_end};
use crate::{
    geometry::clip::{Join, diff_ex, intersection_pl, offset_ex},
    geometry::{BoundingBox, ExPolygon, Point, Polyline, SCALE},
    settings::PrintSettings,
    shells::FillSurfaces,
};
#[derive(Clone, Debug)]
struct Node {
    p: Point,
    children: Vec<Node>,
}
#[derive(Clone, Copy, Debug)]
struct Record {
    p: Point,
    parent: u32,
}
/// Trees are prepared once, retaining points and parent indices rather than extrusion paths.
#[derive(Clone, Debug, Default)]
pub struct Generator {
    layers: Vec<Vec<Record>>,
}
fn closest(p: Point, a: Point, b: Point) -> Point {
    let dx = (b.x - a.x) as f64;
    let dy = (b.y - a.y) as f64;
    let len = dx * dx + dy * dy;
    if len == 0. {
        return a;
    }
    lerp(
        a,
        b,
        (((p.x - a.x) as f64 * dx + (p.y - a.y) as f64 * dy) / len).clamp(0., 1.),
    )
}
fn boundary(p: Point, area: &[ExPolygon]) -> Point {
    area.iter()
        .flat_map(|ex| std::iter::once(&ex.contour).chain(&ex.holes))
        .flat_map(|poly| {
            poly.iter()
                .zip(poly.iter().cycle().skip(1))
                .take(poly.len())
        })
        .map(|(&a, &b)| closest(p, a, b))
        .min_by_key(|q| p.distance_sq(*q))
        .unwrap_or(p)
}
fn visible(a: Point, b: Point, area: &[ExPolygon]) -> bool {
    if a == b {
        return true;
    }
    let len: f64 = intersection_pl(&[vec![a, b]], area)
        .iter()
        .map(|l| crate::geometry::polyline_length(l))
        .sum();
    len + 100. >= a.distance_to(b)
}
fn contains(p: Point, area: &[ExPolygon]) -> bool {
    let inside = |poly: &[Point]| {
        let mut inside = false;
        for (&a, &b) in poly
            .iter()
            .zip(poly.iter().cycle().skip(1))
            .take(poly.len())
        {
            if (a.y > p.y) != (b.y > p.y)
                && ((p.x - a.x) as f64)
                    < (b.x - a.x) as f64 * (p.y - a.y) as f64 / (b.y - a.y) as f64
            {
                inside = !inside;
            }
        }
        inside
    };
    area.iter()
        .any(|ex| inside(&ex.contour) && !ex.holes.iter().any(|h| inside(h)))
}
impl Node {
    fn flatten(&self, parent: u32, out: &mut Vec<Record>) {
        let id = out.len() as u32;
        out.push(Record { p: self.p, parent });
        for c in &self.children {
            c.flatten(id, out);
        }
    }
    fn prune(&mut self, d: f64) -> f64 {
        let mut max: f64 = 0.;
        self.children.retain_mut(|c| {
            let cut = c.prune(d);
            let len = self.p.distance_to(c.p);
            if cut >= d {
                max = max.max(cut);
                true
            } else if cut + len <= d {
                max = max.max(cut + len);
                false
            } else {
                c.p = lerp(c.p, self.p, (d - cut) / len);
                max = d;
                true
            }
        });
        max
    }
    fn remove_collinear_child(parent: Point, child: &mut Node) {
        if child.children.len() == 1 {
            let below = child.children[0].p;
            if parent.distance_to(below) < 2. * SCALE
                && child.p.distance_to(closest(child.p, parent, below)) < 10.
            {
                *child = child.children.remove(0);
            }
        }
    }
    fn straighten(
        &mut self,
        above: Point,
        acc: f64,
        magnitude: f64,
        is_root: bool,
    ) -> (Point, f64) {
        if self.children.len() == 1 {
            let c = &mut self.children[0];
            let below = c.straighten(above, acc + self.p.distance_to(c.p), magnitude, false);
            Self::remove_collinear_child(self.p, c);
            let dest = lerp(above, below.0, acc / below.1.max(1.));
            let len = self.p.distance_to(dest);
            if len > 0. {
                self.p = lerp(self.p, dest, (magnitude / len).min(1.));
            }
            below
        } else {
            let unit = |a: Point, b: Point| {
                let len = a.distance_to(b);
                if len > 0. {
                    (
                        (b.x - a.x) as f64 / len * 1000.,
                        (b.y - a.y) as f64 / len * 1000.,
                    )
                } else {
                    (0., 0.)
                }
            };
            let mut dir = unit(self.p, above);
            let mut prevent = false;
            for child in &mut self.children {
                let below = child.straighten(self.p, self.p.distance_to(child.p), magnitude, false);
                Self::remove_collinear_child(self.p, child);
                let v = unit(self.p, below.0);
                dir.0 += v.0;
                dir.1 += v.1;
                prevent |= below.1 < magnitude;
            }
            let len = dir.0.hypot(dir.1);
            if !is_root && !prevent && !self.children.is_empty() && len > 0. {
                let factor = (magnitude * 0.75 / len).min(1.);
                self.p.x += (dir.0 * factor) as i64;
                self.p.y += (dir.1 * factor) as i64;
            }
            (self.p, acc)
        }
    }
    #[allow(clippy::too_many_arguments)]
    fn best(
        &self,
        p: Point,
        radius: f64,
        area: &[ExPolygon],
        root: bool,
        path: &mut Vec<usize>,
        best: &mut (f64, Option<Vec<usize>>, (i64, i64)),
        within: f64,
    ) {
        let len = p.distance_to(self.p);
        let valence = self.children.len() + usize::from(!root);
        let score = len.floor()
            - if valence > 0 && valence < 4 {
                4. * radius
            } else {
                0.
            };
        let bb = BoundingBox::from_points(area.iter().flat_map(|e| &e.contour)).unwrap();
        let cell = 4_000_000;
        let gx = (self.p.x - bb.min.x) / cell;
        let gy = (self.p.y - bb.min.y) / cell;
        let reach = within as i64;
        let in_grid = gx >= (p.x - reach - bb.min.x) / cell
            && gx < (p.x + reach + cell - bb.min.x) / cell
            && gy >= (p.y - reach - bb.min.y) / cell
            && gy < (p.y + reach + cell - bb.min.y) / cell;
        // Grid cells are visited row-first. Within a cell the upstream multimap visits
        // later insertions first; reverse preorder supplies a deterministic tie rule.
        let better = score < best.0 || (score == best.0 && (gy, gx) <= best.2);
        if len > 0. && in_grid && better && visible(p, self.p, area) {
            best.0 = score;
            best.1 = Some(path.clone());
            best.2 = (gy, gx);
        }
        for (i, c) in self.children.iter().enumerate() {
            path.push(i);
            c.best(p, radius, area, false, path, best, within);
            path.pop();
        }
    }
    fn attach(&mut self, path: &[usize], child: Node) {
        if let Some((&i, rest)) = path.split_first() {
            self.children[i].attach(rest, child);
        } else {
            self.children.push(child);
        }
    }
    fn at(&self, path: &[usize]) -> Point {
        if let Some((&i, rest)) = path.split_first() {
            self.children[i].at(rest)
        } else {
            self.p
        }
    }
    fn nearest(&self, p: Point, path: &mut Vec<usize>, best: &mut (i128, Vec<usize>)) {
        let d = self.p.distance_sq(p);
        if d < best.0 {
            *best = (d, path.clone());
        }
        for (i, c) in self.children.iter().enumerate() {
            path.push(i);
            c.nearest(p, path, best);
            path.pop();
        }
    }
    fn reroot_near(self, p: Point) -> Node {
        let mut best = (i128::MAX, Vec::new());
        self.nearest(p, &mut Vec::new(), &mut best);
        let mut current = self;
        let mut parent = None;
        for i in best.1 {
            let child = current.children.remove(i);
            if let Some(parent) = parent {
                current.children.push(parent);
            }
            parent = Some(current);
            current = child;
        }
        if let Some(parent) = parent {
            current.children.push(parent);
        }
        current
    }
    fn realign(mut self, area: &[ExPolygon], out: &mut Vec<Node>) -> Option<Node> {
        let inside = contains(self.p, area) || self.p.distance_to(boundary(self.p, area)) < 10.;
        let mut kept = Vec::new();
        for child in std::mem::take(&mut self.children) {
            if let Some(child) = child.realign(area, out) {
                if inside && visible(self.p, child.p, area) {
                    kept.push(child);
                } else {
                    let clipped = intersection_pl(&[vec![child.p, self.p]], area);
                    let ground = clipped
                        .iter()
                        .flat_map(|l| l.iter())
                        .min_by_key(|q| self.p.distance_sq(**q))
                        .copied()
                        .unwrap_or_else(|| boundary(child.p, area));
                    out.push(Node {
                        p: ground,
                        children: vec![child],
                    });
                }
            }
        }
        self.children = kept;
        if inside { Some(self) } else { None }
    }
}
impl Generator {
    pub(crate) fn build(surfaces: &[FillSurfaces], s: &PrintSettings, origin: Point) -> Self {
        let local = |area: &[ExPolygon]| {
            let mut area = area.to_vec();
            for ex in &mut area {
                super::translate(&mut ex.contour, -origin.x, -origin.y);
                for h in &mut ex.holes {
                    super::translate(h, -origin.x, -origin.y);
                }
            }
            area
        };
        let radius = s.infill_extrusion_width / s.fill_density * SCALE;
        let cell = (radius / 6.) as i64;
        let step = s.layer_height * SCALE;
        let mut layers = vec![Vec::new(); surfaces.len()];
        let mut trees: Vec<Node> = Vec::new();
        for i in (0..surfaces.len()).rev() {
            let area = local(&surfaces[i].internal);
            let area = area.as_slice();
            if area.is_empty() {
                trees.clear();
                continue;
            }
            let above = surfaces
                .get(i + 1)
                .map(|f| local(&f.internal))
                .unwrap_or_default();
            let overhang = diff_ex(&offset_ex(area, -step, Join::Miter), &above);
            let inherited_count = trees.len();
            let mut samples = Vec::new();
            if let Some(bb) = BoundingBox::from_points(overhang.iter().flat_map(|e| &e.contour)) {
                let mut x = bb.min.x;
                while x < bb.max.x {
                    let cuts = intersection_pl(
                        &[vec![
                            Point::new(x, bb.min.y - 1),
                            Point::new(x, bb.max.y + 1),
                        ]],
                        &overhang,
                    );
                    for cut in cuts {
                        let lo = cut.iter().map(|p| p.y).min().unwrap();
                        let hi = cut.iter().map(|p| p.y).max().unwrap();
                        let mut y = (lo.div_euclid(cell) + 1) * cell;
                        while y < hi {
                            let p = Point::new(x, y);
                            let dist = p.distance_to(boundary(p, &overhang));
                            let hash = ((89i64 * 31 + x) * 31 + y) as i32 as usize % 191;
                            // The upstream vertical scan excludes points on the polygon boundary;
                            // Clipper2 also returns coincident boundary segments, so drop those here.
                            if dist > 0. {
                                samples.push((p, dist as i64, hash));
                            }
                            y += cell;
                        }
                    }
                    x += cell;
                }
            }
            sort_samples(&mut samples, radius as i64, false);
            let mut supported: std::collections::HashMap<(i64, i64), Vec<Point>> =
                std::collections::HashMap::new();
            let support_cell = radius as i64;
            for (p, _, _) in samples {
                let bucket = (p.x.div_euclid(support_cell), p.y.div_euclid(support_cell));
                if (-1..=1).any(|dy| {
                    (-1..=1).any(|dx| {
                        supported
                            .get(&(bucket.0 + dx, bucket.1 + dy))
                            .is_some_and(|points| {
                                points.iter().any(|q| p.distance_to(*q) <= radius)
                            })
                    })
                }) {
                    continue;
                }
                let ground = boundary(p, area);
                let within = p.distance_to(ground).floor();
                let mut best = (within, None, (i64::MIN, i64::MIN));
                if within >= step {
                    for (j, tree) in trees.iter().enumerate() {
                        tree.best(p, radius, area, true, &mut vec![j], &mut best, within);
                    }
                }
                let child = Node {
                    p,
                    children: Vec::new(),
                };
                if let Some(path) = best.1 {
                    trees[path[0]].attach(&path[1..], child);
                } else {
                    trees.push(Node {
                        p: ground,
                        children: vec![child],
                    });
                }
                supported.entry(bucket).or_default().push(p);
            }
            // Reattach propagated roots as the internal boundary moves. Without this pass roots
            // inside an expanding lower layer would float, and overhang support would terminate early.
            for j in (0..inherited_count).rev() {
                let tree = trees.remove(j);
                if tree.children.is_empty() {
                    continue;
                }
                let ground = boundary(tree.p, area);
                let within = tree.p.distance_to(ground).floor();
                if within < step - 100. {
                    trees.insert(j, tree);
                    continue;
                }
                let mut best = (within, None, (i64::MIN, i64::MIN));
                for (k, other) in trees.iter().enumerate() {
                    other.best(tree.p, radius, area, true, &mut vec![k], &mut best, within);
                }
                if let Some(path) = best.1 {
                    let target = trees[path[0]].at(&path[1..]);
                    let tree = tree.reroot_near(target);
                    trees[path[0]].attach(&path[1..], tree);
                } else {
                    let tree = tree.reroot_near(ground);
                    trees.insert(
                        j,
                        Node {
                            p: ground,
                            children: vec![tree],
                        },
                    );
                }
            }
            for tree in &trees {
                tree.flatten(u32::MAX, &mut layers[i]);
            }
            if i > 0 {
                let below = local(&surfaces[i - 1].internal);
                let mut next = Vec::new();
                for mut tree in trees {
                    tree.prune(step);
                    let p = tree.p;
                    tree.straighten(p, 0., step, true);
                    if let Some(tree) = tree.realign(&below, &mut next) {
                        next.push(tree);
                    }
                }
                trees = next;
            }
        }
        for layer in &mut layers {
            for n in layer {
                n.p.x += origin.x;
                n.p.y += origin.y;
            }
        }
        Self { layers }
    }
    pub fn node_count(&self) -> usize {
        self.layers.iter().map(Vec::len).sum()
    }
    pub(super) fn lines(&self, layer: usize, overlap: f64) -> Vec<Polyline> {
        let Some(nodes) = self.layers.get(layer) else {
            return Vec::new();
        };
        let mut children = vec![Vec::new(); nodes.len()];
        for (i, n) in nodes.iter().enumerate() {
            if n.parent != u32::MAX {
                children[n.parent as usize].push(i);
            }
        }
        fn visit(
            i: usize,
            line: usize,
            nodes: &[Record],
            children: &[Vec<usize>],
            out: &mut Vec<Polyline>,
        ) {
            if let Some((&first, rest)) = children[i].split_first() {
                visit(first, line, nodes, children, out);
                out[line].push(nodes[i].p);
                for &c in rest {
                    let k = out.len();
                    out.push(Vec::new());
                    visit(c, k, nodes, children, out);
                    out[k].push(nodes[i].p);
                }
            } else {
                out[line].push(nodes[i].p);
            }
        }
        let mut out = Vec::new();
        for (i, n) in nodes.iter().enumerate() {
            if n.parent == u32::MAX {
                let k = out.len();
                out.push(Vec::new());
                visit(i, k, nodes, &children, &mut out);
            }
        }
        for line in &mut out {
            clip_end(line, overlap.max(0.));
        }
        out.retain(|l| l.len() > 1);
        out
    }
}

// The upstream comparison is a proximity heuristic, not a strict weak ordering. Use an explicit
// stable merge instead of passing it to Rust's sort (which requires a total order).
fn sort_samples(samples: &mut [(Point, i64, usize)], radius: i64, small_run: bool) {
    let less = |a: &(Point, i64, usize), b: &(Point, i64, usize)| {
        if (a.1 - b.1).abs() > radius {
            a.1 < b.1
        } else {
            a.2 < b.2
        }
    };
    if samples.len() <= 2 || (small_run && samples.len() <= 8) {
        for i in 1..samples.len() {
            let value = samples[i];
            let mut j = i;
            while j > 0 && less(&value, &samples[j - 1]) {
                samples[j] = samples[j - 1];
                j -= 1;
            }
            samples[j] = value;
        }
        return;
    }
    let mid = samples.len() / 2;
    sort_samples(&mut samples[..mid], radius, !small_run);
    sort_samples(&mut samples[mid..], radius, !small_run);
    let mut merged = Vec::with_capacity(samples.len());
    let (mut i, mut j) = (0, mid);
    while i < mid && j < samples.len() {
        if less(&samples[j], &samples[i]) {
            merged.push(samples[j]);
            j += 1;
        } else {
            merged.push(samples[i]);
            i += 1;
        }
    }
    merged.extend_from_slice(&samples[i..mid]);
    merged.extend_from_slice(&samples[j..]);
    samples.copy_from_slice(&merged);
}
