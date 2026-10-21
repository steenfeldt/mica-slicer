// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Splits one input triangle along the intersection curves that cross it: the points that
//! lie on it are inserted one by one (splitting the triangle, or the edge, they fall in),
//! then every constraint segment is forced into the triangulation by edge flips (Sloan's
//! algorithm). All decisions use the exact predicates of [`super::exact`], so the result is
//! a valid triangulation of the original triangle whatever the configuration (collinear
//! points, points on edges, segments along edges).

use super::exact::{HPoint, P2, cmp2, orient2d};
use std::collections::{HashMap, VecDeque};

struct Tri {
    tris: Vec<Option<[usize; 3]>>,
    /// Directed edge → triangle holding it.
    edges: HashMap<(usize, usize), usize>,
    p: Vec<P2>,
    sign: i32,
}

impl Tri {
    fn orient(&self, a: usize, b: usize, c: usize) -> i32 {
        self.sign * orient2d(&self.p[a], &self.p[b], &self.p[c])
    }
    fn add(&mut self, t: [usize; 3]) {
        let i = self.tris.len();
        for k in 0..3 {
            self.edges.insert((t[k], t[(k + 1) % 3]), i);
        }
        self.tris.push(Some(t));
    }
    fn remove(&mut self, i: usize) -> [usize; 3] {
        let t = self.tris[i].take().expect("live triangle");
        for k in 0..3 {
            self.edges.remove(&(t[k], t[(k + 1) % 3]));
        }
        t
    }
    /// The vertex of the triangle on directed edge (a, b) that is neither.
    fn apex(&self, a: usize, b: usize) -> Option<(usize, usize)> {
        let i = *self.edges.get(&(a, b))?;
        let t = self.tris[i]?;
        let c = t.into_iter().find(|&v| v != a && v != b)?;
        Some((i, c))
    }

    fn insert_point(&mut self, p: usize) -> Result<(), String> {
        for i in 0..self.tris.len() {
            let Some([a, b, c]) = self.tris[i] else {
                continue;
            };
            let o = [
                self.orient(b, c, p),
                self.orient(c, a, p),
                self.orient(a, b, p),
            ];
            if o.iter().any(|&s| s < 0) {
                continue;
            }
            let zeros = o.iter().filter(|&&s| s == 0).count();
            match zeros {
                0 => {
                    self.remove(i);
                    self.add([a, b, p]);
                    self.add([b, c, p]);
                    self.add([c, a, p]);
                }
                1 => {
                    // On the edge opposite the zero's vertex.
                    let (x, y, z) = if o[0] == 0 {
                        (b, c, a)
                    } else if o[1] == 0 {
                        (c, a, b)
                    } else {
                        (a, b, c)
                    };
                    // Triangle (x, y, z) with p on x→y; the neighbour holds y→x.
                    let other = self.apex(y, x);
                    self.remove(i);
                    self.add([x, p, z]);
                    self.add([p, y, z]);
                    if let Some((j, m)) = other {
                        self.remove(j);
                        self.add([y, p, m]);
                        self.add([p, x, m]);
                    }
                }
                _ => return Err("two intersection points coincide".into()),
            }
            return Ok(());
        }
        Err("an intersection point lies outside its triangle".into())
    }

    /// Whether `x`, `y` properly cross segment `u`, `v` (no shared endpoint, no touching).
    fn crosses(&self, u: usize, v: usize, x: usize, y: usize) -> bool {
        if x == u || x == v || y == u || y == v {
            return false;
        }
        self.orient(u, v, x) * self.orient(u, v, y) < 0
            && self.orient(x, y, u) * self.orient(x, y, v) < 0
    }

    /// Strictly between `u` and `v` on their (common) line.
    fn between(&self, u: usize, v: usize, w: usize) -> bool {
        let axis = if cmp2(&self.p[u], &self.p[v], 0).is_ne() {
            0
        } else {
            1
        };
        let a = cmp2(&self.p[u], &self.p[w], axis);
        let b = cmp2(&self.p[w], &self.p[v], axis);
        a.is_ne() && a == b
    }

    fn insert_segment(&mut self, u: usize, v: usize) -> Result<(), String> {
        if u == v {
            return Ok(());
        }
        // Vertices on the open segment split it.
        let mut on: Vec<usize> = (0..self.p.len())
            .filter(|&w| w != u && w != v)
            .filter(|&w| self.orient(u, v, w) == 0 && self.between(u, v, w))
            .collect();
        if !on.is_empty() {
            let axis = if cmp2(&self.p[u], &self.p[v], 0).is_ne() {
                0
            } else {
                1
            };
            let forward = cmp2(&self.p[u], &self.p[v], axis).is_lt();
            on.sort_by(|&a, &b| {
                let o = cmp2(&self.p[a], &self.p[b], axis);
                if forward { o } else { o.reverse() }
            });
            let mut prev = u;
            for w in on.into_iter().chain(std::iter::once(v)) {
                self.insert_segment(prev, w)?;
                prev = w;
            }
            return Ok(());
        }
        if self.edges.contains_key(&(u, v)) || self.edges.contains_key(&(v, u)) {
            return Ok(());
        }
        let mut queue: VecDeque<(usize, usize)> = VecDeque::new();
        for t in self.tris.iter().flatten() {
            for k in 0..3 {
                let (x, y) = (t[k], t[(k + 1) % 3]);
                if x < y && self.crosses(u, v, x, y) {
                    queue.push_back((x, y));
                }
            }
        }
        let mut guard = 0usize;
        let limit = 64 + queue.len() * queue.len() * 8;
        while let Some((x, y)) = queue.pop_front() {
            guard += 1;
            if guard > limit {
                return Err("constraint insertion did not converge".into());
            }
            let (Some((i, a)), Some((j, b))) = (self.apex(x, y), self.apex(y, x)) else {
                return Err("a crossing edge lies on the triangle's boundary".into());
            };
            // Quad x, b, y, a; convex when x and y lie strictly on either side of a-b.
            if self.orient(a, b, x) * self.orient(a, b, y) < 0 {
                self.remove(i);
                self.remove(j);
                self.add([x, b, a]);
                self.add([b, y, a]);
                if self.crosses(u, v, a, b) {
                    queue.push_back((a, b));
                }
            } else {
                queue.push_back((x, y));
            }
        }
        if self.edges.contains_key(&(u, v)) || self.edges.contains_key(&(v, u)) {
            Ok(())
        } else {
            Err("constraint missing after flips".into())
        }
    }
}

/// Triangulates triangle `corners` (counter-clockwise when its plane is projected along
/// `axis` and orientations are multiplied by `sign`) with the extra `points` (on it, none
/// equal to a corner) and the `constraints` (segments between corners and points). Returns
/// triangles of point ids with the original winding.
pub(super) fn subdivide(
    corners: [u32; 3],
    points: &[u32],
    constraints: &[(u32, u32)],
    axis: usize,
    sign: i32,
    pts: &[HPoint],
) -> Result<Vec<[u32; 3]>, String> {
    let mut ids: Vec<u32> = corners.to_vec();
    ids.extend(points.iter().copied().filter(|p| !corners.contains(p)));
    let local: HashMap<u32, usize> = ids.iter().enumerate().map(|(i, &id)| (id, i)).collect();
    let mut t = Tri {
        tris: Vec::new(),
        edges: HashMap::new(),
        p: ids
            .iter()
            .map(|&id| P2::project(&pts[id as usize], axis))
            .collect(),
        sign,
    };
    if t.orient(0, 1, 2) <= 0 {
        return Err("degenerate input triangle".into());
    }
    t.add([0, 1, 2]);
    for i in 3..ids.len() {
        t.insert_point(i)?;
    }
    for &(a, b) in constraints {
        let (Some(&u), Some(&v)) = (local.get(&a), local.get(&b)) else {
            return Err("constraint endpoint is not a point of the triangle".into());
        };
        t.insert_segment(u, v)?;
    }
    Ok(t.tris
        .iter()
        .flatten()
        .map(|tri| tri.map(|i| ids[i]))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hp(x: i64, y: i64) -> HPoint {
        HPoint::int([x, y, 0])
    }

    #[test]
    fn points_and_segments_triangulate_the_triangle() {
        let pts = vec![
            hp(0, 0),
            hp(100, 0),
            hp(0, 100),
            hp(20, 20),
            hp(40, 20),
            hp(50, 0),  // on edge 0-1
            hp(30, 30), // collinear with 3 and on the segment 3 → 7
            hp(40, 40),
        ];
        let tris = subdivide(
            [0, 1, 2],
            &[3, 4, 5, 6, 7],
            &[(3, 4), (3, 7), (5, 7)],
            2,
            1,
            &pts,
        )
        .unwrap();
        // Area is preserved and every triangle is counter-clockwise.
        let area = |t: &[u32; 3]| {
            let [a, b, c] = t.map(|i| pts[i as usize]);
            (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)
        };
        assert!(tris.iter().all(|t| area(t) > 0));
        assert_eq!(tris.iter().map(area).sum::<i128>(), 100 * 100);
        // Constraints are edges (3-7 through 6).
        let has = |a: u32, b: u32| {
            tris.iter()
                .any(|t| (0..3).any(|k| t[k] == a && t[(k + 1) % 3] == b))
        };
        for (a, b) in [(3, 4), (3, 6), (6, 7), (5, 7)] {
            assert!(has(a, b) || has(b, a), "missing {a}-{b}");
        }
    }
}
