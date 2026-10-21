// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/AvoidCrossingPerimeters.cpp :: get_boundary, get_boundary_external, avoid_perimeters_inner, simplify_travel, travel_to
// Changes: Island inset routing and plate-wide external hole boundaries with visibility shortcuts. No top-surface exclusion; narrow regions fall back to a smaller inset.
// Copyright (c) Prusa Research 2020 - 2022 Lukáš Hejl @hejllukas, Vojtěch Bubník @bubnikv

//! Follow the shorter boundary walk, then remove visible waypoints inside the selected region.
use crate::boundary::{Line, Lines, XY, distance, lerp, xy};
use mica_core::{
    config::Config,
    geometry::{
        BoundingBox, ExPolygon, Point, SCALE,
        clip::{Join, diff_ex, offset_ex},
    },
};

struct Island {
    outline: Lines,
    inner: Vec<Region>,
}
struct Region {
    boundary: Lines,
    polygons: Vec<Vec<XY>>,
}
impl Region {
    fn new(p: &ExPolygon) -> Self {
        Self {
            boundary: Lines::outlines(std::slice::from_ref(p)),
            polygons: p
                .polygons()
                .map(|p| p.iter().copied().map(xy).collect())
                .collect(),
        }
    }
    fn project(&self, p: XY) -> XY {
        if self.boundary.contains(p) {
            p
        } else {
            self.boundary
                .nearest(p)
                .map_or(p, |(i, _)| self.boundary.lines[i].projection(p).1)
        }
    }
    fn route(&self, a: XY, b: XY) -> Option<Vec<XY>> {
        if inside_segment(&self.boundary, a, b) {
            return Some(vec![a, b]);
        }
        // Each direct-line excursion around a hole or concavity follows its shortest boundary arc.
        let line = Line { a, b };
        let mut hits = Vec::new();
        for (pi, poly) in self.polygons.iter().enumerate() {
            for i in 0..poly.len() {
                if let Some(t) = line.intersection(Line {
                    a: poly[i],
                    b: poly[(i + 1) % poly.len()],
                }) {
                    hits.push((t, pi, i, lerp(a, b, t)));
                }
            }
        }
        hits.sort_by(|a, b| a.0.total_cmp(&b.0));
        hits.dedup_by(|a, b| a.1 == b.1 && (a.0 - b.0).abs() < 1e-8);
        let mut path = vec![a];
        let mut i = 0;
        while i < hits.len() {
            let (_, pi, edge, p) = hits[i];
            let j = (i + 1..hits.len()).rev().find(|&j| hits[j].1 == pi);
            let Some(j) = j else {
                i += 1;
                continue;
            };
            let (_, _, end_edge, q) = hits[j];
            // Only bypass excursions that leave the printable region. Tangencies stay straight.
            if inside_segment(&self.boundary, p, q) {
                i = j + 1;
                continue;
            }
            let poly = &self.polygons[pi];
            let n = poly.len();
            let walk = |forward: bool| {
                let mut v = vec![p];
                let mut k = edge;
                if edge == end_edge {
                    v.push(q);
                    return v;
                }
                while k != end_edge {
                    if forward {
                        k = (k + 1) % n;
                        v.push(poly[k]);
                    } else {
                        v.push(poly[k]);
                        k = (k + n - 1) % n;
                    }
                }
                v.push(q);
                v
            };
            let forward = walk(true);
            let backward = walk(false);
            let chosen = if length(&forward) <= length(&backward) {
                forward
            } else {
                backward
            };
            path.extend(chosen);
            i = j + 1;
        }
        path.push(b);
        path.dedup_by(|a, b| distance(*a, *b) < 1e-7);
        if !path
            .windows(2)
            .all(|p| inside_segment(&self.boundary, p[0], p[1]))
        {
            return None;
        }
        // The upstream visibility simplifier skips vertices only when the replacement cannot cross.
        let mut result = vec![path[0]];
        let mut i = 0;
        while i + 1 < path.len() {
            let next = (i + 1..path.len())
                .rev()
                .find(|&j| inside_segment(&self.boundary, path[i], path[j]))
                .unwrap();
            result.push(path[next]);
            i = next;
        }
        Some(result)
    }
}
fn length(points: &[XY]) -> f64 {
    points.windows(2).map(|p| distance(p[0], p[1])).sum()
}
fn contains_or_boundary(lines: &Lines, p: XY) -> bool {
    lines.contains(p) || lines.nearest(p).is_some_and(|(_, d)| d <= 2e-6)
}
fn inside_segment(lines: &Lines, a: XY, b: XY) -> bool {
    if !contains_or_boundary(lines, a) || !contains_or_boundary(lines, b) {
        return false;
    }
    let mut ts = lines.intersections(Line { a, b });
    ts.extend([0., 1.]);
    ts.sort_by(f64::total_cmp);
    ts.windows(2)
        .all(|t| t[1] - t[0] < 1e-8 || contains_or_boundary(lines, lerp(a, b, (t[0] + t[1]) * 0.5)))
}

/// Disabled by default. Missing outlines or an excessive detour keep straight travel.
type CachedRoute = Option<([(f64, f64); 2], Vec<(f64, f64)>)>;

pub struct AvoidCrossingPerimeters {
    // Retraction, lift and cooling query the same route consecutively. Keep just that route.
    cached: std::cell::RefCell<CachedRoute>,
    islands: Vec<Island>,
    external: Vec<Region>,
    max_detour: f64,
    percent: bool,
}
impl AvoidCrossingPerimeters {
    pub fn new(config: &Config, outlines: Option<&[ExPolygon]>, spacing: f64) -> Self {
        let enabled = config.bool("avoid_crossing_perimeters").unwrap_or(false);
        let islands = if enabled {
            outlines
                .unwrap_or_default()
                .iter()
                .map(|p| {
                    let mut inner =
                        offset_ex(std::slice::from_ref(p), -1.5 * spacing * SCALE, Join::Miter);
                    // Preserve passages that disappear under the normal inset. Validity is checked against
                    // the original island on every leg; this does not join disconnected islands.
                    inner.extend(offset_ex(
                        std::slice::from_ref(p),
                        -0.5 * spacing * SCALE,
                        Join::Miter,
                    ));
                    Island {
                        outline: Lines::outlines(std::slice::from_ref(p)),
                        inner: inner.iter().map(Region::new).collect(),
                    }
                })
                .collect()
        } else {
            Vec::new()
        };
        let percent = config
            .raw("avoid_crossing_perimeters_max_detour")
            .is_some_and(|s| s.ends_with('%'));
        Self {
            cached: std::cell::RefCell::new(None),
            islands,
            external: if enabled {
                external_regions(outlines.unwrap_or_default(), spacing)
            } else {
                Vec::new()
            },
            max_detour: config
                .float("avoid_crossing_perimeters_max_detour")
                .unwrap_or(0.),
            percent,
        }
    }
    /// Waypoints include both endpoints. Retract/lift decisions must use the resulting length.
    pub fn route(&self, from: (f64, f64), to: (f64, f64)) -> Vec<(f64, f64)> {
        if let Some((ends, route)) = self.cached.borrow().as_ref()
            && *ends == [from, to]
        {
            return route.clone();
        }
        let route = self.compute_route(from, to);
        *self.cached.borrow_mut() = Some(([from, to], route.clone()));
        route
    }

    fn compute_route(&self, from: (f64, f64), to: (f64, f64)) -> Vec<(f64, f64)> {
        let a = [from.0, from.1];
        let b = [to.0, to.1];
        let direct = distance(a, b);
        let straight = || vec![from, to];
        let Some(island) = self.islands.iter().find(|island| {
            contains_or_boundary(&island.outline, a) && contains_or_boundary(&island.outline, b)
        }) else {
            // The pinned external planner permits crossing object exteriors when changing
            // objects, but routes around every object's holes in placed plate coordinates.
            for region in &self.external {
                if let Some(path) = region.route(a, b) {
                    return self.accept(path, direct).unwrap_or_else(straight);
                }
            }
            return straight();
        };
        if inside_segment(&island.outline, a, b) {
            return straight();
        }
        for region in &island.inner {
            let start = region.project(a);
            let end = region.project(b);
            if !inside_segment(&island.outline, a, start)
                || !inside_segment(&island.outline, end, b)
            {
                continue;
            }
            if let Some(route) = region.route(start, end) {
                let mut path = vec![a];
                path.extend(route);
                path.push(b);
                path.dedup_by(|a, b| distance(*a, *b) < 1e-7);
                // Snap to the emitted XY grid before validation. Otherwise a rounded waypoint
                // can land on the wrong side of a narrow hole or concave corner.
                for p in &mut path[1..] {
                    for coord in p {
                        *coord = (*coord * 1000.).round() / 1000.;
                    }
                }
                if !path
                    .windows(2)
                    .all(|p| inside_segment(&island.outline, p[0], p[1]))
                {
                    continue;
                }
                return self.accept(path, direct).unwrap_or_else(straight);
            }
        }
        straight()
    }

    fn accept(&self, mut path: Vec<XY>, direct: f64) -> Option<Vec<(f64, f64)>> {
        for p in &mut path[1..] {
            for coord in p {
                *coord = (*coord * 1000.).round() / 1000.;
            }
        }
        let allowed = if self.percent {
            direct * self.max_detour * 0.01
        } else {
            self.max_detour
        };
        (self.max_detour <= 0. || length(&path) - direct <= allowed)
            .then(|| path.into_iter().map(|p| (p[0], p[1])).collect())
    }
}

fn external_regions(outlines: &[ExPolygon], spacing: f64) -> Vec<Region> {
    // Upstream collects each placed object's holes before expanding their union.
    let holes: Vec<_> = outlines
        .iter()
        .flat_map(|p| &p.holes)
        .map(|h| {
            let mut contour = h.clone();
            contour.reverse();
            ExPolygon {
                contour,
                holes: vec![],
            }
        })
        .collect();
    if holes.is_empty() {
        return Vec::new();
    }
    // Half a spacing plus one XY output quantum keeps rounded legs outside the physical hole.
    let holes = offset_ex(&holes, (0.5 * spacing + 0.001) * SCALE, Join::Miter);
    let Some(bb) = BoundingBox::from_points(outlines.iter().flat_map(|p| &p.contour)) else {
        return Vec::new();
    };
    let margin = (1000. * SCALE) as i64;
    let min = Point::new(bb.min.x - margin, bb.min.y - margin);
    let max = Point::new(bb.max.x + margin, bb.max.y + margin);
    diff_ex(
        &[ExPolygon {
            contour: vec![min, Point::new(max.x, min.y), max, Point::new(min.x, max.y)],
            holes: vec![],
        }],
        &holes,
    )
    .iter()
    .map(Region::new)
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mica_core::geometry::Point;
    fn polygon(p: &[(f64, f64)]) -> Vec<Point> {
        p.iter().map(|p| Point::from_mm(p.0, p.1)).collect()
    }
    fn donut() -> ExPolygon {
        ExPolygon {
            contour: polygon(&[(0., 0.), (20., 0.), (20., 20.), (0., 20.)]),
            holes: vec![polygon(&[(8., 8.), (8., 12.), (12., 12.), (12., 8.)])],
        }
    }
    #[test]
    fn holes_concavity_and_detour_limits() {
        let mut c = Config::defaults();
        c.set_raw("avoid_crossing_perimeters", "1").unwrap();
        let outlines = vec![donut()];
        let router = AvoidCrossingPerimeters::new(&c, Some(&outlines), 0.4);
        let path = router.route((2., 10.), (18., 10.));
        assert!(path.len() > 2, "{path:?}");
        assert!(path.windows(2).all(|p| inside_segment(
            &router.islands[0].outline,
            [p[0].0, p[0].1],
            [p[1].0, p[1].1]
        )));
        for limit in ["0.1", "1%"] {
            c.set_raw("avoid_crossing_perimeters_max_detour", limit)
                .unwrap();
            assert_eq!(
                AvoidCrossingPerimeters::new(&c, Some(&outlines), 0.4)
                    .route((2., 10.), (18., 10.))
                    .len(),
                2
            );
        }
        c.set_raw("avoid_crossing_perimeters_max_detour", "100%")
            .unwrap();
        assert!(
            AvoidCrossingPerimeters::new(&c, Some(&outlines), 0.4)
                .route((2., 10.), (18., 10.))
                .len()
                > 2
        );
        let concave = vec![ExPolygon {
            contour: polygon(&[
                (0., 0.),
                (20., 0.),
                (20., 20.),
                (12., 20.),
                (12., 6.),
                (8., 6.),
                (8., 20.),
                (0., 20.),
            ]),
            holes: vec![],
        }];
        c.set_raw("avoid_crossing_perimeters_max_detour", "0")
            .unwrap();
        assert!(
            AvoidCrossingPerimeters::new(&c, Some(&concave), 0.4)
                .route((4., 16.), (16., 16.))
                .len()
                > 2
        );
    }
    #[test]
    fn off_missing_outlines_and_external_holes() {
        let mut c = Config::defaults();
        let outlines = vec![donut()];
        assert_eq!(
            AvoidCrossingPerimeters::new(&c, Some(&outlines), 0.4)
                .route((2., 10.), (18., 10.))
                .len(),
            2
        );
        c.set_raw("avoid_crossing_perimeters", "1").unwrap();
        assert_eq!(
            AvoidCrossingPerimeters::new(&c, None, 0.4)
                .route((2., 10.), (18., 10.))
                .len(),
            2
        );
        let router = AvoidCrossingPerimeters::new(&c, Some(&outlines), 0.4);
        let route = router.route((2., 10.), (30., 10.));
        assert!(route.len() > 2);
        assert!(route.windows(2).all(|p| inside_segment(
            &router.external[0].boundary,
            [p[0].0, p[0].1],
            [p[1].0, p[1].1]
        )));
        assert_eq!(router.route((2., 5.), (30., 5.)).len(), 2);
        c.set_raw("avoid_crossing_perimeters_max_detour", "0.1")
            .unwrap();
        assert_eq!(
            AvoidCrossingPerimeters::new(&c, Some(&outlines), 0.4)
                .route((2., 10.), (30., 10.))
                .len(),
            2
        );
    }

    #[test]
    fn external_route_avoids_holes_in_both_placed_objects() {
        let mut c = Config::defaults();
        c.set_raw("avoid_crossing_perimeters", "1").unwrap();
        let mut second = donut();
        for p in second
            .contour
            .iter_mut()
            .chain(second.holes.iter_mut().flatten())
        {
            p.x += Point::from_mm(30., 0.).x;
        }
        let outlines = [donut(), second];
        let router = AvoidCrossingPerimeters::new(&c, Some(&outlines), 0.4);
        let path = router.route((2., 10.), (48., 10.));
        assert!(path.len() > 2);
        assert_eq!(path.first(), Some(&(2., 10.)));
        assert_eq!(path.last(), Some(&(48., 10.)));
        assert!(path.windows(2).all(|p| inside_segment(
            &router.external[0].boundary,
            [p[0].0, p[0].1],
            [p[1].0, p[1].1]
        )));
        // Crossing the two outer contours is intentional for an external move.
        assert_eq!(router.route((2., 5.), (48., 5.)), [(2., 5.), (48., 5.)]);
    }
}
