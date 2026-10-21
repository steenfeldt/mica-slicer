// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: prusaslicer@b028299c77 src/libslic3r/MultiMaterialSegmentation.cpp :: extract_colored_segments, cell_range_to_polygon, split_convex_polygon_to_two_polygons_with_equal_area
// Changes: Index-based Rust Voronoi graph; bounded traversals and recoverable errors instead of pointer assertions.
// Copyright (c) Prusa Research 2021 - 2023 Vojtěch Bubník @bubnikv, Lukáš Hejl @hejllukas
// PrusaSlicer is released under the terms of the AGPLv3 or higher.

use crate::geometry::clip::{closing_ex, union_ex};
use crate::geometry::voronoi::{self, Diagram, Line};
use crate::geometry::{ExPolygon, Point, Polygon, signed_area};

pub(super) fn partition(
    rings: &[Vec<(Point, u8)>],
    count: usize,
) -> Result<Vec<Vec<ExPolygon>>, String> {
    // Like upstream, construct in object-local coordinates. Bed translations and
    // multi-plate offsets otherwise amplify cancellation in Voronoi predicates.
    let Some(bounds) =
        crate::geometry::BoundingBox::from_points(rings.iter().flatten().map(|(p, _)| p))
    else {
        return Ok(vec![vec![]; count]);
    };
    let origin = Point::new(
        (bounds.min.x + bounds.max.x) / 2,
        (bounds.min.y + bounds.max.y) / 2,
    );
    let local: Vec<Vec<_>> = rings
        .iter()
        .map(|r| {
            r.iter()
                .map(|&(p, c)| (Point::new(p.x - origin.x, p.y - origin.y), c))
                .collect()
        })
        .collect();
    let mut result = partition_local(&local, count)?;
    for p in result.iter_mut().flatten() {
        for v in p.contour.iter_mut().chain(p.holes.iter_mut().flatten()) {
            v.x += origin.x;
            v.y += origin.y;
        }
    }
    Ok(result)
}

fn partition_local(
    rings: &[Vec<(Point, u8)>],
    count: usize,
) -> Result<Vec<Vec<ExPolygon>>, String> {
    let mut lines = Vec::new();
    let mut colors = Vec::new();
    let mut next = Vec::new();
    for ring in rings {
        let base = lines.len();
        for i in 0..ring.len() {
            lines.push(Line::new(ring[i].0, ring[(i + 1) % ring.len()].0));
            colors.push(ring[i].1);
            next.push(base + (i + 1) % ring.len());
        }
    }
    let (vd, _valid) = voronoi::construct(&lines);
    // Prusa's colour extraction continues with recoverable cells after rotation
    // retries. Its global Voronoi validity check is stricter than this traversal:
    // missing contour vertices are skipped by segment_cell_range below.
    if vd.cells.is_empty() {
        return Err("could not construct painted-boundary Voronoi diagram".into());
    }
    let mut g = Graph {
        deleted: vec![false; vd.edges.len()],
        vertices: vec![0; vd.vertices.len()],
        vd,
    };
    let ranges: Vec<_> =
        g.vd.cells
            .iter()
            .filter(|c| c.contains_segment())
            .filter_map(|c| {
                voronoi::segment_cell_range(&g.vd, c, &lines).map(|(b, e)| (c.source_index, b, e))
            })
            .collect();
    for &(_, b, _) in &ranges {
        if let Some(v) = g.vd.edges[b].vertex0 {
            g.vertices[v] = 1;
        }
    }
    let min_x = lines.iter().map(|l| l.a.x).min().unwrap_or(0) as f64;
    let min_y = lines.iter().map(|l| l.a.y).min().unwrap_or(0) as f64;
    let max_x = lines.iter().map(|l| l.a.x).max().unwrap_or(0) as f64;
    let max_y = lines.iter().map(|l| l.a.y).max().unwrap_or(0) as f64;
    for v in 0..g.vd.vertices.len() {
        let p = g.vd.vertices[v];
        if g.vertices[v] == 0
            && (!p.x.is_finite()
                || !p.y.is_finite()
                || p.x < min_x
                || p.x > max_x
                || p.y < min_y
                || p.y > max_y)
        {
            g.delete_deep(v);
        }
    }
    for e in 0..g.vd.edges.len() {
        if !g.deleted[e] && !g.vd.is_finite(e) {
            g.deleted[e] = true;
            g.deleted[e ^ 1] = true;
            for v in [g.vd.edges[e].vertex0, g.vd.vertex1(e)]
                .into_iter()
                .flatten()
            {
                if g.can_delete(v) {
                    g.delete_deep(v);
                }
            }
        }
    }
    for &(i, b, _) in &ranges {
        let current = lines[i];
        let after = lines[next[i]];
        let normal = |l: Line| {
            let dx = (l.b.x - l.a.x) as f64;
            let dy = (l.b.y - l.a.y) as f64;
            let len = dx.hypot(dy);
            [-dy / len, dx / len]
        };
        let n1 = normal(current);
        let n2 = normal(after);
        for e in g.around(b) {
            if g.deleted[e] {
                continue;
            }
            if let Some(v) = g.vd.vertex1(e) {
                let p = g.vd.vertices[v];
                if (p.x - current.b.x as f64) * (n1[0] + n2[0])
                    + (p.y - current.b.y as f64) * (n1[1] + n2[1])
                    <= 0.
                {
                    g.deleted[e] = true;
                    g.deleted[e ^ 1] = true;
                    if g.vertices[v] == 0 {
                        g.delete_deep(v);
                    }
                }
            }
        }
    }
    for &(i, b, _) in &ranges {
        if colors[i] != colors[next[i]] {
            continue;
        }
        let edges: Vec<_> = g.around(b).into_iter().filter(|&e| !g.deleted[e]).collect();
        for &e in edges.iter().skip(1).rev() {
            g.deleted[e] = true;
            g.deleted[e ^ 1] = true;
            if let Some(v) = g.vd.vertex1(e)
                && g.can_delete(v)
            {
                g.delete_deep(v);
            }
        }
    }
    let mut result = vec![vec![]; count];
    for &(i, b, e) in &ranges {
        let Some(v) = g.vd.edges[b].vertex0 else {
            continue;
        };
        if g.vertices[v] != 1 {
            continue;
        }
        let Some(poly) = g.polygon(b, e) else {
            continue;
        };
        if let Some(point_begin) = g.around(b).into_iter().find(|&e| !g.deleted[e])
            && let Some(point_poly) = g.polygon(point_begin, b ^ 1)
            && let Some((left, right)) = split_equal(&point_poly)
        {
            add(&mut result[colors[i] as usize], left);
            add(&mut result[colors[next[i]] as usize], right);
        }
        g.vertices[v] = 2;
        add(&mut result[colors[i] as usize], poly);
    }
    for area in &mut result {
        *area = closing_ex(&union_ex(area), 0.0001 * crate::geometry::SCALE);
    }
    Ok(result)
}
fn add(out: &mut Vec<ExPolygon>, mut contour: Polygon) {
    contour.dedup();
    if contour.len() < 3 || signed_area(&contour).abs() < 1. {
        return;
    }
    if signed_area(&contour) < 0. {
        contour.reverse();
    }
    out.push(ExPolygon {
        contour,
        holes: vec![],
    });
}
struct Graph {
    vd: Diagram,
    deleted: Vec<bool>,
    vertices: Vec<u8>,
}
impl Graph {
    fn around(&self, begin: usize) -> Vec<usize> {
        let mut result = vec![];
        let mut e = begin;
        loop {
            result.push(e);
            e = self.vd.rot_next(e);
            if e == begin || result.len() > self.vd.edges.len() {
                break;
            }
        }
        result
    }
    fn can_delete(&self, v: usize) -> bool {
        self.vertices[v] == 0
            && self.vd.vertices[v].incident_edge.is_some_and(|e| {
                self.around(e)
                    .into_iter()
                    .filter(|&e| !self.deleted[e])
                    .count()
                    <= 1
            })
    }
    fn delete_deep(&mut self, v: usize) {
        let mut pending = vec![v];
        while let Some(v) = pending.pop() {
            if self.vertices[v] != 0 {
                continue;
            }
            self.vertices[v] = 2;
            if let Some(begin) = self.vd.vertices[v].incident_edge {
                for e in self.around(begin) {
                    self.deleted[e] = true;
                    self.deleted[e ^ 1] = true;
                    if let Some(other) = self.vd.vertex1(e)
                        && self.can_delete(other)
                    {
                        pending.push(other);
                    }
                }
            }
        }
    }
    fn point(&self, v: usize) -> Point {
        let p = self.vd.vertices[v];
        Point::new(p.x.round() as i64, p.y.round() as i64)
    }
    fn polygon(&mut self, begin: usize, end: usize) -> Option<Polygon> {
        let start = self.vd.edges[begin].vertex0?;
        if self.vertices[start] != 1 {
            return None;
        }
        let mut poly = vec![self.point(start)];
        let mut e = begin;
        let mut steps = 0;
        loop {
            if !self.deleted[e] {
                let v = self.vd.vertex1(e)?;
                poly.push(self.point(v));
                self.deleted[e] = true;
                if self.vertices[v] != 0 {
                    break;
                }
                e ^= 1;
            }
            e = self.vd.edges[e ^ 1].next;
            steps += 1;
            if e == begin || steps > self.vd.edges.len() {
                break;
            }
        }
        if self.vd.vertex1(e) != self.vd.vertex1(end) {
            return None;
        }
        if Some(start) == self.vd.vertex1(end) {
            poly.pop();
        }
        (poly.len() >= 3).then_some(poly)
    }
}
fn triangle_area(a: Point, b: Point, c: Point) -> f64 {
    ((b.x - a.x) as f64 * (c.y - a.y) as f64 - (b.y - a.y) as f64 * (c.x - a.x) as f64) * 0.5
}
fn split_equal(poly: &Polygon) -> Option<(Polygon, Polygon)> {
    let half = signed_area(poly) * 0.5;
    if half <= 0. || poly.len() < 3 {
        return None;
    }
    let mut left = vec![poly[0]];
    let mut right = vec![poly[0], poly[1]];
    let mut area = 0.;
    let mut i = 2;
    while i < poly.len() {
        let a = triangle_area(poly[0], poly[i - 1], poly[i]);
        if area + a <= half {
            area += a;
            right.push(poly[i]);
        } else {
            let t = (half - area) / a;
            let p = Point::new(
                poly[i - 1].x + (t * (poly[i].x - poly[i - 1].x) as f64) as i64,
                poly[i - 1].y + (t * (poly[i].y - poly[i - 1].y) as f64) as i64,
            );
            right.push(p);
            left.push(p);
            break;
        }
        i += 1;
    }
    left.extend_from_slice(&poly[i..]);
    Some((left, right))
}
