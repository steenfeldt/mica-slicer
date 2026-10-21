// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: orca@8500fcdcca src/libslic3r/PerimeterGenerator.cpp :: PerimeterGenerator::process_arachne, reorderPerimetersByProximity
// Changes: Optional Orca batch B wall, bridge or scarf controls; existing Prusa defaults retained.
// Source: prusaslicer@b028299c77 src/libslic3r/Arachne/PerimeterOrder.cpp :: ordered_perimeter_extrusions, get_sorted_perimeter_extrusions_by_area, construct_perimeter_extrusions_adjacency_graph, assign_nearest_external_perimeter, ordered_perimeter_extrusions_to_minimize_distances, order_of_grouped_perimeter_extrusions_to_minimize_distances, extract_ordered_perimeter_extrusions
// Source: prusaslicer@b028299c77 src/libslic3r/Arachne/utils/ExtrusionLine.cpp :: ExtrusionLine::area, ExtrusionLine::is_contour
// Source: prusaslicer@b028299c77 src/clipper/clipper.cpp :: PointInPolygon
// Changes: Adjacency and nearest-external references are arena indices; preserve unassigned paths; contour flags computed once per line.
// Changes: Prusa profiles use the exact upstream order (libc++ std::sort, nearest-start greedy with
// Changes: its open/closed rule, border-inclusive containment); Orca profiles keep the earlier order.
use super::ExtrusionLine;
use crate::geometry::{Point, signed_area};

fn contains(poly: &[super::Junction], p: Point) -> bool {
    let mut inside = false;
    for w in poly.windows(2) {
        let a = w[0].p;
        let b = w[1].p;
        if (a.y > p.y) != (b.y > p.y)
            && (p.x as f64)
                < (b.x - a.x) as f64 * (p.y - a.y) as f64 / (b.y - a.y) as f64 + a.x as f64
        {
            inside = !inside;
        }
    }
    inside
}
fn bounds(line: &ExtrusionLine) -> (i64, i64, i64, i64) {
    line.junctions.iter().fold(
        (i64::MAX, i64::MAX, i64::MIN, i64::MIN),
        |(x0, y0, x1, y1), j| (x0.min(j.p.x), y0.min(j.p.y), x1.max(j.p.x), y1.max(j.p.y)),
    )
}
fn contour(line: &ExtrusionLine) -> bool {
    line.is_closed && signed_area(&line.polygon()) < 0.
}
fn nearest(mut ids: Vec<usize>, lines: &[ExtrusionLine], mut pos: Point) -> Vec<usize> {
    let mut out = vec![];
    while !ids.is_empty() {
        let best = (0..ids.len())
            .min_by_key(|&i| {
                (
                    lines[ids[i]].is_closed,
                    lines[ids[i]].junctions[0].p.distance_sq(pos),
                )
            })
            .unwrap();
        let i = ids.remove(best);
        pos = if lines[i].is_closed {
            lines[i].junctions[0].p
        } else {
            lines[i].junctions.last().unwrap().p
        };
        out.push(i);
    }
    out
}
/// `ClipperLib::PointInPolygon`: 0 outside, 1 inside, -1 on the boundary.
fn point_in_polygon(pt: Point, path: &[Point]) -> i32 {
    let cnt = path.len();
    if cnt < 3 {
        return 0;
    }
    let cross = |ip: Point, next: Point| {
        (ip.x - pt.x) as i128 * (next.y - pt.y) as i128
            - (next.x - pt.x) as i128 * (ip.y - pt.y) as i128
    };
    let mut result = 0;
    let mut ip = path[0];
    for i in 1..=cnt {
        let next = if i == cnt { path[0] } else { path[i] };
        if next.y == pt.y
            && (next.x == pt.x || (ip.y == pt.y && ((next.x > pt.x) == (ip.x < pt.x))))
        {
            return -1;
        }
        if (ip.y < pt.y) != (next.y < pt.y) {
            if ip.x >= pt.x {
                if next.x > pt.x {
                    result = 1 - result;
                } else {
                    let d = cross(ip, next);
                    if d == 0 {
                        return -1;
                    }
                    if (d > 0) == (next.y > ip.y) {
                        result = 1 - result;
                    }
                }
            } else if next.x > pt.x {
                let d = cross(ip, next);
                if d == 0 {
                    return -1;
                }
                if (d > 0) == (next.y > ip.y) {
                    result = 1 - result;
                }
            }
        }
        ip = next;
    }
    result
}

/// `ExtrusionLine::area`: shoelace in doubles, zero for open lines.
fn prusa_area(line: &ExtrusionLine) -> f64 {
    if !line.is_closed {
        return 0.;
    }
    let mut a = 0.;
    if line.junctions.len() >= 3 {
        let mut p1 = line.junctions.last().unwrap().p;
        for j in &line.junctions {
            let p2 = j.p;
            a += p1.x as f64 * p2.y as f64 - p1.y as f64 * p2.x as f64;
            p1 = p2;
        }
    }
    0.5 * a
}

fn end_position(line: &ExtrusionLine) -> Point {
    if line.is_closed {
        line.junctions[0].p
    } else {
        line.junctions.last().unwrap().p
    }
}

/// The nearest-start choice of `ordered_perimeter_extrusions_to_minimize_distances` and
/// `order_of_grouped_perimeter_extrusions_to_minimize_distances`: a closer closed line always wins,
/// a closer open one only while no closed line is the nearest so far.
fn nearest_start(candidates: impl Iterator<Item = (usize, Point, bool)>, pos: Point) -> usize {
    let mut nearest_distance = f64::MAX;
    let mut nearest = 0;
    let mut nearest_closed = false;
    for (k, start, closed) in candidates {
        let (dx, dy) = ((pos.x - start.x) as f64, (pos.y - start.y) as f64);
        let distance = dx * dx + dy * dy;
        if distance < nearest_distance
            && (closed || nearest_distance == f64::MAX || !nearest_closed)
        {
            nearest = k;
            nearest_distance = distance;
            nearest_closed = closed;
        }
    }
    nearest
}

/// PrusaSlicer's `Arachne::PerimeterOrder::ordered_perimeter_extrusions`.
fn order_prusa(lines: &mut Vec<ExtrusionLine>, external_first: bool) {
    // get_sorted_perimeter_extrusions_by_area (open lines have zero area and come first).
    let mut sorted: Vec<(ExtrusionLine, f64)> = std::mem::take(lines)
        .into_iter()
        .filter(|l| !l.junctions.is_empty())
        .map(|l| {
            let area = prusa_area(&l).abs();
            (l, area)
        })
        .collect();
    crate::cxx_sort::sort_by(&mut sorted, |a, b| a.1 < b.1);
    let sorted: Vec<ExtrusionLine> = sorted.into_iter().map(|(l, _)| l).collect();
    let n = sorted.len();
    let boxes: Vec<_> = sorted.iter().map(bounds).collect();
    // to_polygon drops the closing junction.
    let polygons: Vec<Vec<Point>> = sorted
        .iter()
        .map(|l| {
            if l.is_closed {
                l.junctions[..l.junctions.len() - 1]
                    .iter()
                    .map(|j| j.p)
                    .collect()
            } else {
                vec![]
            }
        })
        .collect();
    let is_contour: Vec<bool> = sorted.iter().map(contour).collect();
    // construct_perimeter_extrusions_adjacency_graph
    let mut adjacency = vec![vec![]; n];
    let mut roots = vec![false; n];
    for i in 0..n {
        if !sorted[i].is_closed {
            roots[i] = true;
            continue;
        }
        for j in 0..n {
            if !roots[j] {
                continue;
            }
            let (a, b) = (boxes[i], boxes[j]);
            if a.0 <= b.0
                && a.1 <= b.1
                && a.2 >= b.2
                && a.3 >= b.3
                && point_in_polygon(sorted[j].junctions[0].p, &polygons[i]) != 0
            {
                adjacency[i].push(j);
                adjacency[j].push(i);
                roots[j] = false;
            }
        }
        roots[i] = true;
    }
    // assign_nearest_external_perimeter
    let mut depth = vec![usize::MAX; n];
    let mut owner = vec![usize::MAX; n];
    let mut stack = vec![];
    for i in 0..n {
        if sorted[i].inset == 0 {
            depth[i] = 0;
            owner[i] = i;
            stack.push(i);
        }
    }
    while let Some(i) = stack.pop() {
        for &j in &adjacency[i] {
            let d = depth[i] + 1;
            if d < depth[j] {
                owner[j] = owner[i];
                depth[j] = d;
                stack.push(j);
            } else if d == depth[j] && !is_contour[owner[j]] && is_contour[i] {
                owner[j] = owner[i];
                stack.push(j);
            }
        }
    }
    // extract_ordered_perimeter_extrusions
    let mut groups: Vec<(usize, Vec<usize>)> = vec![];
    let mut included = vec![false; n];
    for root in 0..n {
        if sorted[root].inset != 0 {
            continue;
        }
        let mut group = vec![];
        let mut visited = vec![false; n];
        let mut stack = vec![root];
        while let Some(i) = stack.pop() {
            visited[i] = true;
            if owner[i] == root {
                group.push(i);
                included[i] = true;
            }
            let mut candidates: Vec<usize> = adjacency[i]
                .iter()
                .copied()
                .filter(|&j| !visited[j] && sorted[j].inset != 0 && owner[j] == root)
                .collect();
            if candidates.len() == 1 {
                stack.push(candidates[0]);
            } else if candidates.len() > 1 {
                // ordered_perimeter_extrusions_to_minimize_distances from Point::Zero().
                crate::cxx_sort::sort_by(&mut candidates, |&a, &b| {
                    !sorted[a].is_closed && sorted[b].is_closed
                });
                let mut selected = vec![false; candidates.len()];
                let mut ordered = vec![];
                let mut pos = Point::new(0, 0);
                while ordered.len() < candidates.len() {
                    let k = nearest_start(
                        (0..candidates.len()).filter(|&k| !selected[k]).map(|k| {
                            let l = &sorted[candidates[k]];
                            (k, l.junctions[0].p, l.is_closed)
                        }),
                        pos,
                    );
                    selected[k] = true;
                    pos = end_position(&sorted[candidates[k]]);
                    ordered.push(candidates[k]);
                }
                stack.extend(ordered.into_iter().rev());
            }
        }
        if !external_first {
            group.reverse();
        }
        groups.push((root, group));
    }
    // order_of_grouped_perimeter_extrusions_to_minimize_distances from Point::Zero(): holes (and
    // open lines) first.
    let mut by_kind: Vec<usize> = (0..groups.len()).collect();
    crate::cxx_sort::sort_by(&mut by_kind, |&l, &r| {
        let (l, r) = (groups[l].0, groups[r].0);
        (!is_contour[l] && is_contour[r])
            || (is_contour[l] == is_contour[r] && !sorted[l].is_closed && sorted[r].is_closed)
    });
    let holes = groups.iter().filter(|(root, _)| !is_contour[*root]).count();
    let mut selected = vec![false; groups.len()];
    let mut order = vec![];
    let mut pos = Point::new(0, 0);
    while order.len() < groups.len() {
        let end = if order.len() < holes {
            holes
        } else {
            by_kind.len()
        };
        let g = nearest_start(
            by_kind[..end]
                .iter()
                .copied()
                .filter(|&g| !selected[g])
                .map(|g| {
                    let root = &sorted[groups[g].0];
                    (g, root.junctions[0].p, root.is_closed)
                }),
            pos,
        );
        order.push(g);
        selected[g] = true;
        if let Some(&last) = groups[g].1.last() {
            pos = end_position(&sorted[last]);
        }
    }
    let mut indices: Vec<usize> = order
        .iter()
        .flat_map(|&g| groups[g].1.iter().copied())
        .collect();
    indices.extend((0..n).filter(|&i| !included[i]));
    let mut old: Vec<_> = sorted.into_iter().map(Some).collect();
    lines.extend(indices.into_iter().map(|i| old[i].take().unwrap()));
}

pub(crate) fn order(
    lines: &mut Vec<ExtrusionLine>,
    external_first: bool,
    sandwich: bool,
    prusa: bool,
) {
    if prusa && !sandwich {
        order_prusa(lines, external_first);
        return;
    }
    lines.sort_by(|a, b| {
        let area = |l: &ExtrusionLine| {
            if l.is_closed {
                signed_area(&l.polygon()).abs()
            } else {
                0.
            }
        };
        area(a).total_cmp(&area(b))
    });
    let n = lines.len();
    let boxes: Vec<_> = lines.iter().map(bounds).collect();
    // Whether each line is a contour, once (the grouping below asks for it per comparison).
    let is_contour: Vec<bool> = lines.iter().map(contour).collect();
    let mut adjacency = vec![vec![]; n];
    let mut roots = vec![false; n];
    for i in 0..n {
        if lines[i].is_closed {
            for j in 0..i {
                let a = boxes[i];
                let b = boxes[j];
                if roots[j]
                    && a.0 <= b.0
                    && a.1 <= b.1
                    && a.2 >= b.2
                    && a.3 >= b.3
                    && contains(&lines[i].junctions, lines[j].junctions[0].p)
                {
                    adjacency[i].push(j);
                    adjacency[j].push(i);
                    roots[j] = false;
                }
            }
        }
        roots[i] = true;
    }
    let mut depth = vec![usize::MAX; n];
    let mut owner = vec![usize::MAX; n];
    let mut stack = vec![];
    for i in 0..n {
        if lines[i].inset == 0 {
            depth[i] = 0;
            owner[i] = i;
            stack.push(i);
        }
    }
    while let Some(i) = stack.pop() {
        for &j in &adjacency[i] {
            if depth[i] + 1 < depth[j]
                || (depth[i] + 1 == depth[j]
                    && owner[j] != usize::MAX
                    && !is_contour[owner[j]]
                    && is_contour[i])
            {
                depth[j] = depth[i] + 1;
                owner[j] = owner[i];
                stack.push(j);
            }
        }
    }
    let mut groups = vec![];
    let mut included = vec![false; n];
    for root in 0..n {
        if lines[root].inset != 0 {
            continue;
        }
        let mut group = vec![];
        let mut visited = vec![false; n];
        let mut stack = vec![root];
        while let Some(i) = stack.pop() {
            if visited[i] {
                continue;
            }
            visited[i] = true;
            if owner[i] == root {
                group.push(i);
                included[i] = true;
            }
            let candidates: Vec<_> = adjacency[i]
                .iter()
                .copied()
                .filter(|&j| !visited[j] && lines[j].inset != 0 && owner[j] == root)
                .collect();
            stack.extend(
                nearest(candidates, lines, Point::new(0, 0))
                    .into_iter()
                    .rev(),
            );
        }
        if sandwich {
            crate::wall_options::sandwich(&mut group, |i| lines[*i].inset);
        } else if !external_first {
            group.reverse();
        }
        groups.push((root, group));
    }
    let mut indices = vec![];
    let mut pos = Point::new(0, 0);
    while !groups.is_empty() {
        let best = (0..groups.len())
            .min_by_key(|&i| {
                let root = groups[i].0;
                (
                    is_contour[root],
                    lines[root].is_closed,
                    lines[root].junctions[0].p.distance_sq(pos),
                )
            })
            .unwrap();
        let (_, group) = groups.remove(best);
        if let Some(&last) = group.last() {
            pos = if lines[last].is_closed {
                lines[last].junctions[0].p
            } else {
                lines[last].junctions.last().unwrap().p
            };
        }
        indices.extend(group);
    }
    indices.extend((0..n).filter(|&i| !included[i]));
    let mut old: Vec<_> = std::mem::take(lines).into_iter().map(Some).collect();
    lines.extend(indices.into_iter().map(|i| old[i].take().unwrap()));
}

#[cfg(test)]
mod tests {
    use super::super::Junction;
    use super::*;

    fn line(inset: usize, closed: bool, points: &[(i64, i64)]) -> ExtrusionLine {
        let mut junctions: Vec<Junction> = points
            .iter()
            .map(|&(x, y)| Junction {
                p: Point::new(x, y),
                width: 400_000.,
                perimeter: inset,
            })
            .collect();
        if closed {
            junctions.push(junctions[0]);
        }
        ExtrusionLine {
            inset,
            is_odd: false,
            is_closed: closed,
            junctions,
        }
    }

    /// Arachne contours run clockwise, holes counter-clockwise.
    fn square(inset: usize, c: (i64, i64), r: i64, hole: bool) -> ExtrusionLine {
        let mut pts = vec![
            (c.0 - r, c.1 - r),
            (c.0 - r, c.1 + r),
            (c.0 + r, c.1 + r),
            (c.0 + r, c.1 - r),
        ];
        if hole {
            pts.reverse();
        }
        line(inset, true, &pts)
    }

    #[test]
    fn prusa_order_takes_the_nearest_hole_before_a_farther_open_line() {
        let contour = square(0, (0, 0), 20_000_000, false);
        let near_hole = square(0, (-6_000_000, 0), 2_000_000, true);
        let far_open = line(
            0,
            false,
            &[(15_000_000, 15_000_000), (16_000_000, 15_000_000)],
        );
        let mut lines = vec![contour, near_hole, far_open];
        order(&mut lines, false, false, true);
        // Holes and open lines come before the contour; among them the nearest start wins,
        // not the open line (ordered_perimeter_extrusions_to_minimize_distances).
        assert!(lines[0].is_closed && signed_area(&lines[0].polygon()) > 0.);
        assert!(!lines[1].is_closed);
        assert!(lines[2].is_closed && signed_area(&lines[2].polygon()) < 0.);
    }

    #[test]
    fn prusa_order_prints_inner_walls_of_a_group_first() {
        let outer = square(0, (0, 0), 10_000_000, false);
        let inner = square(1, (0, 0), 9_500_000, false);
        let mut lines = vec![outer, inner];
        order(&mut lines, false, false, true);
        assert_eq!(lines[0].inset, 1);
        assert_eq!(lines[1].inset, 0);
        order(&mut lines, true, false, true);
        assert_eq!(lines[0].inset, 0);
    }

    #[test]
    fn point_in_polygon_reports_the_border() {
        let poly = [
            Point::new(0, 0),
            Point::new(10, 0),
            Point::new(10, 10),
            Point::new(0, 10),
        ];
        assert_eq!(point_in_polygon(Point::new(5, 5), &poly), 1);
        assert_eq!(point_in_polygon(Point::new(10, 5), &poly), -1);
        assert_eq!(point_in_polygon(Point::new(15, 5), &poly), 0);
    }
}
