// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Exact geometric predicates for the mesh boolean.
//!
//! Input vertices are snapped to an integer grid (|coordinate| < 2^26), so every predicate
//! on input vertices is an integer determinant that fits `i128`. Intersection points are
//! stored as homogeneous integer points (x, y, z, w) with w > 0, all four below 2^112; the
//! predicates on them need up to ~2^311, computed in [`Wide`] (512-bit two's complement)
//! behind an interval filter that answers almost every query in `f64`.

use std::cmp::Ordering;

/// Integer grid coordinates must stay below this magnitude (bits) for the bounds above.
pub const COORD_BITS: u32 = 26;

/// A homogeneous point with integer coordinates; `w` > 0. Input vertices have `w` = 1.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct HPoint {
    pub x: i128,
    pub y: i128,
    pub z: i128,
    pub w: i128,
}

impl HPoint {
    pub fn int(p: [i64; 3]) -> Self {
        Self {
            x: p[0] as i128,
            y: p[1] as i128,
            z: p[2] as i128,
            w: 1,
        }
    }
    pub fn coord(&self, k: usize) -> i128 {
        match k {
            0 => self.x,
            1 => self.y,
            _ => self.z,
        }
    }
    /// The point in grid units, rounded to f64.
    pub fn to_f64(self) -> [f64; 3] {
        let w = self.w as f64;
        [self.x as f64 / w, self.y as f64 / w, self.z as f64 / w]
    }
}

/// 512-bit two's complement integer: just enough arithmetic for determinant signs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Wide([u64; 8]);

impl Wide {
    pub fn from_i128(v: i128) -> Self {
        let fill = if v < 0 { u64::MAX } else { 0 };
        let u = v as u128;
        let mut l = [fill; 8];
        l[0] = u as u64;
        l[1] = (u >> 64) as u64;
        Wide(l)
    }
    pub fn add(self, o: Wide) -> Wide {
        let mut out = [0u64; 8];
        let mut carry = 0u64;
        for (i, slot) in out.iter_mut().enumerate() {
            let (s1, c1) = self.0[i].overflowing_add(o.0[i]);
            let (s2, c2) = s1.overflowing_add(carry);
            *slot = s2;
            carry = (c1 as u64) + (c2 as u64);
        }
        Wide(out)
    }
    pub fn neg(self) -> Wide {
        let mut inv = [0u64; 8];
        for (i, slot) in inv.iter_mut().enumerate() {
            *slot = !self.0[i];
        }
        Wide(inv).add(Wide::from_i128(1))
    }
    pub fn sub(self, o: Wide) -> Wide {
        self.add(o.neg())
    }
    /// Product modulo 2^512, which is the exact product whenever it fits (all callers).
    pub fn mul(self, o: Wide) -> Wide {
        let mut out = [0u64; 8];
        for i in 0..8 {
            if self.0[i] == 0 {
                continue;
            }
            let mut carry = 0u128;
            for j in 0..(8 - i) {
                let cur = out[i + j] as u128 + (self.0[i] as u128) * (o.0[j] as u128) + carry;
                out[i + j] = cur as u64;
                carry = cur >> 64;
            }
        }
        Wide(out)
    }
    pub fn signum(&self) -> i32 {
        if self.0[7] >> 63 == 1 {
            -1
        } else if self.0.iter().all(|&l| l == 0) {
            0
        } else {
            1
        }
    }
}

fn w(v: i128) -> Wide {
    Wide::from_i128(v)
}

/// A conservative f64 interval: every operation rounds outwards by one ulp.
#[derive(Clone, Copy, Debug)]
struct Iv {
    lo: f64,
    hi: f64,
}

impl Iv {
    fn of(v: i128) -> Iv {
        let f = v as f64;
        if (f as i128) == v && f.abs() < 9.0e15 {
            Iv { lo: f, hi: f }
        } else {
            Iv {
                lo: f.next_down(),
                hi: f.next_up(),
            }
        }
    }
    fn add(self, o: Iv) -> Iv {
        Iv {
            lo: (self.lo + o.lo).next_down(),
            hi: (self.hi + o.hi).next_up(),
        }
    }
    fn sub(self, o: Iv) -> Iv {
        Iv {
            lo: (self.lo - o.hi).next_down(),
            hi: (self.hi - o.lo).next_up(),
        }
    }
    fn mul(self, o: Iv) -> Iv {
        let p = [
            self.lo * o.lo,
            self.lo * o.hi,
            self.hi * o.lo,
            self.hi * o.hi,
        ];
        let lo = p.iter().copied().fold(f64::INFINITY, f64::min);
        let hi = p.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        Iv {
            lo: lo.next_down(),
            hi: hi.next_up(),
        }
    }
    fn sign(self) -> Option<i32> {
        if self.lo > 0. {
            Some(1)
        } else if self.hi < 0. {
            Some(-1)
        } else if self.lo == 0. && self.hi == 0. {
            Some(0)
        } else {
            None
        }
    }
}

pub type V3 = [i128; 3];

pub fn sub3(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
pub fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
pub fn dot(a: V3, b: V3) -> i128 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
pub fn v3(p: [i64; 3]) -> V3 {
    [p[0] as i128, p[1] as i128, p[2] as i128]
}

/// Sign of the orientation of `d` against the plane through `a`, `b`, `c` (positive when
/// `d` is on the side the normal (b - a) × (c - a) points to). Exact for grid points.
pub fn orient3d(a: [i64; 3], b: [i64; 3], c: [i64; 3], d: [i64; 3]) -> i32 {
    let (a, b, c, d) = (v3(a), v3(b), v3(c), v3(d));
    dot(cross(sub3(b, a), sub3(c, a)), sub3(d, a)).signum() as i32
}

/// A plane n · p = d through three grid points.
#[derive(Clone, Copy, Debug)]
pub struct Plane {
    pub n: V3,
    pub d: i128,
}

impl Plane {
    pub fn through(a: [i64; 3], b: [i64; 3], c: [i64; 3]) -> Plane {
        let n = cross(sub3(v3(b), v3(a)), sub3(v3(c), v3(a)));
        Plane {
            n,
            d: dot(n, v3(a)),
        }
    }
    pub fn is_degenerate(&self) -> bool {
        self.n == [0, 0, 0]
    }
    /// Index of the normal's largest component (the axis dropped when projecting to 2D).
    pub fn dominant_axis(&self) -> usize {
        let a = self.n.map(i128::abs);
        if a[0] >= a[1] && a[0] >= a[2] {
            0
        } else if a[1] >= a[2] {
            1
        } else {
            2
        }
    }
    /// Sign of n · p - d for a homogeneous point (w > 0).
    pub fn side(&self, p: &HPoint) -> i32 {
        if p.w == 1 && p.x.abs() < 1 << 40 && p.y.abs() < 1 << 40 && p.z.abs() < 1 << 40 {
            return (dot(self.n, [p.x, p.y, p.z]) - self.d).signum() as i32;
        }
        let f = Iv::of(self.n[0])
            .mul(Iv::of(p.x))
            .add(Iv::of(self.n[1]).mul(Iv::of(p.y)))
            .add(Iv::of(self.n[2]).mul(Iv::of(p.z)))
            .sub(Iv::of(self.d).mul(Iv::of(p.w)));
        if let Some(s) = f.sign() {
            return s;
        }
        w(self.n[0])
            .mul(w(p.x))
            .add(w(self.n[1]).mul(w(p.y)))
            .add(w(self.n[2]).mul(w(p.z)))
            .sub(w(self.d).mul(w(p.w)))
            .signum()
    }
}

/// The intersection of the line through grid points `p`, `q` with `plane`; `None` when the
/// line is parallel to it.
pub fn line_plane(p: [i64; 3], q: [i64; 3], plane: &Plane) -> Option<HPoint> {
    let (p, q) = (v3(p), v3(q));
    let dir = sub3(q, p);
    let mut den = dot(plane.n, dir);
    if den == 0 {
        return None;
    }
    let mut num = plane.d - dot(plane.n, p);
    if den < 0 {
        den = -den;
        num = -num;
    }
    Some(HPoint {
        x: p[0] * den + dir[0] * num,
        y: p[1] * den + dir[1] * num,
        z: p[2] * den + dir[2] * num,
        w: den,
    })
}

/// The crossing point of the lines through `p`, `q` and through `r`, `s` (which must meet
/// in one point): the first line against a plane containing the second and one axis.
pub fn line_line(p: [i64; 3], q: [i64; 3], r: [i64; 3], s: [i64; 3]) -> Option<HPoint> {
    let dir = sub3(v3(q), v3(p));
    let rs = sub3(v3(s), v3(r));
    let mut best: Option<(i128, Plane)> = None;
    for k in 0..3 {
        let mut e = [0i128; 3];
        e[k] = 1;
        let n = cross(rs, e);
        let den = dot(n, dir).abs();
        if den != 0 && best.as_ref().is_none_or(|(b, _)| den > *b) {
            best = Some((
                den,
                Plane {
                    n,
                    d: dot(n, v3(r)),
                },
            ));
        }
    }
    best.and_then(|(_, plane)| line_plane(p, q, &plane))
}

/// A point projected to 2D by dropping axis `k`, keeping homogeneous `w`.
#[derive(Clone, Copy, Debug)]
pub struct P2 {
    pub u: i128,
    pub v: i128,
    pub w: i128,
}

impl P2 {
    pub fn project(p: &HPoint, k: usize) -> P2 {
        let (a, b) = match k {
            0 => (1, 2),
            1 => (2, 0),
            _ => (0, 1),
        };
        P2 {
            u: p.coord(a),
            v: p.coord(b),
            w: p.w,
        }
    }
}

/// Sign of the 2D orientation of `a`, `b`, `c` (positive counter-clockwise).
pub fn orient2d(a: &P2, b: &P2, c: &P2) -> i32 {
    if a.w == 1 && b.w == 1 && c.w == 1 {
        let small = |p: &P2| p.u.abs() < 1 << 60 && p.v.abs() < 1 << 60;
        if small(a) && small(b) && small(c) {
            let d = (b.u - a.u) * (c.v - a.v) - (b.v - a.v) * (c.u - a.u);
            return d.signum() as i32;
        }
    }
    // det | a.u a.v a.w ; b.u b.v b.w ; c.u c.v c.w |, the sign of which (w > 0) is the
    // orientation of the dehomogenised points.
    let f = Iv::of(a.u)
        .mul(
            Iv::of(b.v)
                .mul(Iv::of(c.w))
                .sub(Iv::of(c.v).mul(Iv::of(b.w))),
        )
        .sub(
            Iv::of(a.v).mul(
                Iv::of(b.u)
                    .mul(Iv::of(c.w))
                    .sub(Iv::of(c.u).mul(Iv::of(b.w))),
            ),
        )
        .add(
            Iv::of(a.w).mul(
                Iv::of(b.u)
                    .mul(Iv::of(c.v))
                    .sub(Iv::of(c.u).mul(Iv::of(b.v))),
            ),
        );
    if let Some(s) = f.sign() {
        return s;
    }
    let m = |x: i128, y: i128, z: i128, t: i128| w(x).mul(w(y)).sub(w(z).mul(w(t)));
    w(a.u)
        .mul(m(b.v, c.w, c.v, b.w))
        .sub(w(a.v).mul(m(b.u, c.w, c.u, b.w)))
        .add(w(a.w).mul(m(b.u, c.v, c.u, b.v)))
        .signum()
}

/// Compares the dehomogenised `u` (`axis` 0) or `v` (1) coordinates of two points.
pub fn cmp2(a: &P2, b: &P2, axis: usize) -> Ordering {
    let (x, y) = if axis == 0 { (a.u, b.u) } else { (a.v, b.v) };
    if a.w == b.w {
        return x.cmp(&y);
    }
    let f = Iv::of(x).mul(Iv::of(b.w)).sub(Iv::of(y).mul(Iv::of(a.w)));
    let s = f
        .sign()
        .unwrap_or_else(|| w(x).mul(w(b.w)).sub(w(y).mul(w(a.w))).signum());
    s.cmp(&0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_arithmetic_matches_i128() {
        let vals = [
            0i128,
            1,
            -1,
            12345,
            -98765,
            i64::MAX as i128,
            -(1i128 << 100),
        ];
        for &a in &vals {
            for &b in &vals {
                assert_eq!(w(a).add(w(b)), w(a.wrapping_add(b)));
                assert_eq!(w(a).sub(w(b)), w(a.wrapping_sub(b)));
                if a.checked_mul(b).is_some() {
                    assert_eq!(w(a).mul(w(b)), w(a * b));
                }
                assert_eq!(w(a).mul(w(b)).signum(), (a.signum() * b.signum()) as i32);
            }
        }
        // 2^110 * 2^110 * 2^90 = 2^310, and its negative.
        let big = w(1 << 110).mul(w(1 << 110)).mul(w(1 << 90));
        assert_eq!(big.signum(), 1);
        assert_eq!(big.neg().signum(), -1);
        assert_eq!(big.sub(big).signum(), 0);
    }

    #[test]
    fn orient2d_exact_on_nearly_collinear_homogeneous_points() {
        // Three collinear points on y = x, two of them homogeneous with huge weights.
        let wa = (1i128 << 80) + 7;
        let a = P2 {
            u: 3 * wa,
            v: 3 * wa,
            w: wa,
        };
        let b = P2 { u: 5, v: 5, w: 1 };
        let wc = (1i128 << 83) - 3;
        let c = P2 {
            u: -11 * wc,
            v: -11 * wc,
            w: wc,
        };
        assert_eq!(orient2d(&a, &b, &c), 0);
        let c2 = P2 {
            u: -11 * wc,
            v: -11 * wc + 1,
            w: wc,
        };
        assert_ne!(orient2d(&a, &b, &c2), 0);
        assert_eq!(orient2d(&a, &b, &c2), -orient2d(&b, &a, &c2));
    }

    #[test]
    fn line_plane_lands_on_the_plane() {
        let plane = Plane::through([0, 0, 7], [10, 0, 7], [0, 10, 7]);
        let p = line_plane([1, 2, 0], [3, 5, 9], &plane).unwrap();
        assert_eq!(plane.side(&p), 0);
        let q = line_line([0, 0, 0], [10, 10, 0], [10, 0, 0], [0, 10, 0]).unwrap();
        assert_eq!(q.to_f64(), [5., 5., 0.]);
    }
}
