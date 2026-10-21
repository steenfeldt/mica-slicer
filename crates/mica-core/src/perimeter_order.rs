// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/PerimeterGenerator.cpp :: process_classic, traverse_loops_classic
// Changes: Index-based nesting tree; original ray crossing and nearest-sibling ordering instead of
// Changes: upstream ShortestPath multifragment ordering. Thin walls remain a separate collection.

use crate::{geometry::Point, perimeters::WallLoop};

fn contains(polygon: &[Point], point: Point) -> bool {
    let mut inside = false;
    for (a, b) in polygon.iter().zip(polygon.iter().cycle().skip(1)) {
        if (a.y > point.y) != (b.y > point.y) {
            let cross = (b.x - a.x) as i128 * (point.y - a.y) as i128
                - (b.y - a.y) as i128 * (point.x - a.x) as i128;
            if (cross > 0) == (b.y > a.y) {
                inside = !inside;
            }
        }
    }
    inside
}

/// Nest holes from small to large, then contours from inside to outside. A hole is emitted before
/// its children; a contour after its children. Reversing the result implements external-first.
pub(crate) fn order(loops: &[WallLoop], origin: Point, external_first: bool) -> Vec<&WallLoop> {
    let mut children = vec![Vec::new(); loops.len()];
    let mut roots = Vec::new();
    for (i, wall) in loops.iter().enumerate() {
        let candidates = || {
            loops.iter().enumerate().filter(|(j, other)| {
                i != *j
                    && wall
                        .polygon
                        .first()
                        .is_some_and(|&p| contains(&other.polygon, p))
            })
        };
        let parent = if wall.is_contour {
            candidates()
                .filter(|(_, p)| p.is_contour && p.depth < wall.depth)
                .max_by_key(|(_, p)| p.depth)
        } else {
            candidates()
                .filter(|(_, p)| !p.is_contour && p.depth > wall.depth)
                .min_by_key(|(_, p)| p.depth)
                .or_else(|| {
                    candidates()
                        .filter(|(_, p)| p.is_contour)
                        .max_by_key(|(_, p)| p.depth)
                })
        };
        if let Some((parent, _)) = parent {
            children[parent].push(i);
        } else {
            roots.push(i);
        }
    }
    fn visit<'a>(
        mut siblings: Vec<usize>,
        loops: &'a [WallLoop],
        children: &[Vec<usize>],
        origin: Point,
        result: &mut Vec<&'a WallLoop>,
    ) {
        let mut from = origin;
        while !siblings.is_empty() {
            let next = siblings
                .iter()
                .enumerate()
                .min_by_key(|(_, i)| {
                    loops[**i]
                        .polygon
                        .first()
                        .map_or(i128::MAX, |p| p.distance_sq(from))
                })
                .map(|(i, _)| i)
                .unwrap();
            let i = siblings.remove(next);
            let wall = &loops[i];
            if !wall.is_contour {
                result.push(wall);
            }
            visit(children[i].clone(), loops, children, origin, result);
            if wall.is_contour {
                result.push(wall);
            }
            from = wall.polygon.first().copied().unwrap_or(from);
        }
    }
    let mut result = Vec::with_capacity(loops.len());
    visit(roots, loops, &children, origin, &mut result);
    if external_first {
        result.reverse();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hole_loops_precede_the_enclosing_contours() {
        let square = |lo, hi, depth, is_contour| WallLoop {
            polygon: [(lo, lo), (hi, lo), (hi, hi), (lo, hi)]
                .map(|(x, y)| Point::new(x, y))
                .to_vec(),
            depth,
            is_contour,
        };
        let walls = [
            square(0, 20, 0, true),
            square(1, 19, 1, true),
            square(8, 12, 0, false),
            square(7, 13, 1, false),
        ];
        let order = order(&walls, Point::new(10, 10), false);
        let actual: Vec<_> = order.iter().map(|w| (w.is_contour, w.depth)).collect();
        assert_eq!(actual, [(false, 1), (false, 0), (true, 1), (true, 0)]);
    }
}
