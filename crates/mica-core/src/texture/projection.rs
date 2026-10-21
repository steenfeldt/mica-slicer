// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: bumpmesh@a6d6d6e6df js/mapping.js :: computeUV, applyTransform, getCubicBlendWeights, getDominantCubicAxis, getScaleReferenceLengths
// Source: bumpmesh@a6d6d6e6df js/displacement.js :: _projectedGrey
// Changes: Works in millimetres (tile size, offset) instead of bounding-box-normalised UVs; the cylinder may run along X, Y or Z; cylindrical and spherical U snap to a whole number of tiles around the part, so the atan2 wrap needs no cross-fade; returns the weighted grey value directly instead of UV sample lists. Pure function of a 3-D point and normal, shared by the wall texture and the Prepare preview.
// Copyright (c) 2026 CNCKitchen (Stefan Hermann) and contributors (BumpMesh, AGPL-3.0-only)

//! How a texture is laid onto a part: from a point on the surface and its normal to a grey value.
use super::heightmap::HeightMap;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Projection {
    #[default]
    Triplanar,
    Cubic,
    Cylindrical,
    Spherical,
    PlanarXy,
    PlanarXz,
    PlanarYz,
}
impl Projection {
    pub const ALL: [Self; 7] = [
        Self::Triplanar,
        Self::Cubic,
        Self::Cylindrical,
        Self::Spherical,
        Self::PlanarXy,
        Self::PlanarXz,
        Self::PlanarYz,
    ];
    pub fn key(self) -> &'static str {
        match self {
            Self::Triplanar => "triplanar",
            Self::Cubic => "cubic",
            Self::Cylindrical => "cylindrical",
            Self::Spherical => "spherical",
            Self::PlanarXy => "planar_xy",
            Self::PlanarXz => "planar_xz",
            Self::PlanarYz => "planar_yz",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.key() == s)
    }
    /// The shader's projection index (render/texture_preview.wgsl).
    pub fn index(self) -> u32 {
        Self::ALL.iter().position(|&p| p == self).unwrap_or(0) as u32
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Axis {
    X,
    Y,
    #[default]
    Z,
}
impl Axis {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "x" => Some(Self::X),
            "y" => Some(Self::Y),
            "z" => Some(Self::Z),
            _ => None,
        }
    }
    pub fn key(self) -> &'static str {
        match self {
            Self::X => "x",
            Self::Y => "y",
            Self::Z => "z",
        }
    }
    /// Rotates coordinates so the cylinder axis becomes Z (a cyclic permutation keeps handedness).
    fn to_z(self, p: [f64; 3]) -> [f64; 3] {
        match self {
            Self::X => [p[1], p[2], p[0]],
            Self::Y => [p[2], p[0], p[1]],
            Self::Z => p,
        }
    }
}

/// Placement of the texture on the part, in millimetres and degrees.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mapping {
    pub projection: Projection,
    /// Tile size along U, mm.
    pub scale_u: f64,
    /// Tile size along V, mm; 0 keeps the image's proportions.
    pub scale_v: f64,
    pub offset_u: f64,
    pub offset_v: f64,
    pub rotation: f64,
    /// Blend between projections: cube faces (cubic) and side to cap (cylindrical), 0–1.
    pub blend: f64,
    pub cap_angle: f64,
    pub axis: Axis,
}
impl Default for Mapping {
    fn default() -> Self {
        Self {
            projection: Projection::Triplanar,
            scale_u: 5.,
            scale_v: 0.,
            offset_u: 0.,
            offset_v: 0.,
            rotation: 0.,
            blend: 1.,
            cap_angle: 20.,
            axis: Axis::Z,
        }
    }
}

/// The part's bounding box, which anchors the texture so it moves with the part.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds {
    pub min: [f64; 3],
    pub max: [f64; 3],
}
impl Bounds {
    fn center(&self) -> [f64; 3] {
        std::array::from_fn(|k| 0.5 * (self.min[k] + self.max[k]))
    }
    fn size(&self) -> [f64; 3] {
        std::array::from_fn(|k| (self.max[k] - self.min[k]).max(0.))
    }
}

/// A texture placed on a part, ready to sample.
pub struct Projector<'a> {
    map: &'a HeightMap,
    m: Mapping,
    bounds: Bounds,
    /// Effective tile sizes (mm).
    su: f64,
    sv: f64,
    cos: f64,
    sin: f64,
    /// Cylinder centre (in the axis-to-Z frame), radius and snapped U tile size.
    cyl: ([f64; 2], f64, f64),
    /// Sphere radius and snapped U tile size.
    sph: (f64, f64),
}

/// Effective tile sizes and the parameters every projection shares, also used by the GPU preview.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub su: f64,
    pub sv: f64,
    pub cyl_center: [f64; 2],
    pub cyl_radius: f64,
    pub cyl_su: f64,
    pub sph_radius: f64,
    pub sph_su: f64,
}

impl<'a> Projector<'a> {
    pub fn new(map: &'a HeightMap, m: Mapping, bounds: Bounds) -> Self {
        let p = placement(&m, bounds, map.aspect());
        let (sin, cos) = m.rotation.to_radians().sin_cos();
        Self {
            map,
            m,
            bounds,
            su: p.su,
            sv: p.sv,
            cos,
            sin,
            cyl: (p.cyl_center, p.cyl_radius, p.cyl_su),
            sph: (p.sph_radius, p.sph_su),
        }
    }

    /// Tile coordinates (mm / tile size, plus offset), rotated about the tile centre, then sampled.
    fn tile(&self, u_mm: f64, v_mm: f64, su: f64, sv: f64) -> f64 {
        let mut u = u_mm / su + self.m.offset_u / su;
        let mut v = v_mm / sv + self.m.offset_v / sv;
        if self.sin != 0. || self.cos != 1. {
            let (du, dv) = (u - 0.5, v - 0.5);
            u = self.cos * du - self.sin * dv + 0.5;
            v = self.sin * du + self.cos * dv + 0.5;
        }
        self.map.sample(u, v)
    }

    /// Grey value 0–1 at `pos` (mm) for the unit surface normal `n`.
    pub fn grey(&self, pos: [f64; 3], n: [f64; 3]) -> f64 {
        let min = self.bounds.min;
        let rel = [pos[0] - min[0], pos[1] - min[1], pos[2] - min[2]];
        let (su, sv) = (self.su, self.sv);
        match self.m.projection {
            Projection::PlanarXy => self.tile(rel[0], rel[1], su, sv),
            Projection::PlanarXz => self.tile(rel[0], rel[2], su, sv),
            Projection::PlanarYz => self.tile(rel[1], rel[2], su, sv),
            Projection::Triplanar => {
                let [bx, by, bz] = n.map(|c| c.abs().powi(4));
                let sum = bx + by + bz + 1e-6;
                self.three_planes(rel, n, [bx / sum, by / sum, bz / sum])
            }
            Projection::Cubic => {
                let w = cubic_weights(n, self.m.blend, 0.35);
                self.three_planes(rel, n, w)
            }
            Projection::Cylindrical => {
                let axis = self.m.axis;
                let p = axis.to_z(pos);
                let nz = axis.to_z(n)[2];
                let zmin = axis.to_z(self.bounds.min)[2];
                let ([cx, cy], r, csu) = self.cyl;
                let (rx, ry) = (p[0] - cx, p[1] - cy);
                let theta = ry.atan2(rx);
                let side = self.tile(
                    (theta / std::f64::consts::TAU + 0.5) * r * std::f64::consts::TAU,
                    p[2] - zmin,
                    csu,
                    sv,
                );
                if self.m.blend <= 0.001 {
                    return side;
                }
                let threshold = self.m.cap_angle.to_radians().cos();
                let half = 0.25;
                let raw = ((nz.abs() - (threshold - half)) / (2. * half + 1e-6)).clamp(0., 1.);
                let cap = raw * raw * (3. - 2. * raw);
                if cap <= 0. {
                    return side;
                }
                let top = self.tile(rx + r, ry + r, su, sv);
                side * (1. - cap) + top * cap
            }
            Projection::Spherical => {
                let c = self.bounds.center();
                let (rx, ry, rz) = (pos[0] - c[0], pos[1] - c[1], pos[2] - c[2]);
                let r = (rx * rx + ry * ry + rz * rz).sqrt();
                let phi = (rz / r.max(1e-6)).clamp(-1., 1.).acos();
                let theta = ry.atan2(rx);
                let (big_r, ssu) = self.sph;
                self.tile(
                    (theta / std::f64::consts::TAU + 0.5) * std::f64::consts::TAU * big_r,
                    phi * big_r,
                    ssu,
                    sv,
                )
            }
        }
    }

    /// The three axis-aligned planes, weighted; U flips with the normal's sign so opposite faces
    /// read the right way round.
    fn three_planes(&self, rel: [f64; 3], n: [f64; 3], [wx, wy, wz]: [f64; 3]) -> f64 {
        let (su, sv) = (self.su, self.sv);
        let flip = |neg: bool, a: f64| if neg { -a } else { a };
        let mut g = 0.;
        if wz > 1e-9 {
            g += self.tile(flip(n[2] < 0., rel[0]), rel[1], su, sv) * wz;
        }
        if wy > 1e-9 {
            g += self.tile(flip(n[1] > 0., rel[0]), rel[2], su, sv) * wy;
        }
        if wx > 1e-9 {
            g += self.tile(flip(n[0] < 0., rel[1]), rel[2], su, sv) * wx;
        }
        g / (wx + wy + wz).max(1e-9)
    }
}

/// Tile sizes after the image's proportions and, around cylinders and spheres, after snapping
/// to a whole number of tiles.
pub fn placement(m: &Mapping, bounds: Bounds, aspect: f64) -> Placement {
    let su = m.scale_u.max(1e-3);
    let sv = if m.scale_v > 0. {
        m.scale_v
    } else {
        su * aspect
    }
    .max(1e-3);
    let size = m.axis.to_z(bounds.size());
    let center = m.axis.to_z(bounds.center());
    let r = (size[0].max(size[1]) * 0.5).max(1e-3);
    let snap = |circumference: f64| circumference / (circumference / su).round().max(1.);
    let size3 = bounds.size();
    let sr = (0.5 * size3[0].max(size3[1]).max(size3[2])).max(1e-3);
    Placement {
        su,
        sv,
        cyl_center: [center[0], center[1]],
        cyl_radius: r,
        cyl_su: snap(std::f64::consts::TAU * r),
        sph_radius: sr,
        sph_su: snap(std::f64::consts::TAU * sr),
    }
}

/// Cube-face weights: one face inside, a soft band of width `band` around the face boundaries.
pub fn cubic_weights(n: [f64; 3], blend: f64, band: f64) -> [f64; 3] {
    const EPS: f64 = 1e-4;
    let [ax, ay, az] = n.map(f64::abs);
    let axis = if ax >= ay - EPS && ax >= az - EPS {
        0
    } else if ay >= az - EPS {
        1
    } else {
        2
    };
    let one_hot: [f64; 3] = std::array::from_fn(|k| if k == axis { 1. } else { 0. });
    if blend <= 0.001 {
        return one_hot;
    }
    let a = [ax, ay, az];
    let primary = a[axis];
    let secondary = (0..3)
        .filter(|&k| k != axis)
        .map(|k| a[k])
        .fold(0., f64::max);
    let width = band.max(EPS * 2.);
    let raw = 1. - ((primary - secondary) / width).clamp(0., 1.);
    let mix = blend * raw * raw * (3. - 2. * raw);
    if mix <= 0.001 {
        return one_hot;
    }
    let power = 1. + (1. - mix) * 11.;
    let s = a.map(|c| c.powf(power));
    let total = s[0] + s[1] + s[2] + 1e-6;
    let m: [f64; 3] = std::array::from_fn(|k| one_hot[k] * (1. - mix) + s[k] / total * mix);
    let sum = m[0] + m[1] + m[2];
    m.map(|c| c / sum)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::FRAC_1_SQRT_2;

    fn checker() -> HeightMap {
        // 2×2 checker: tile coordinates map to quadrants.
        HeightMap::new(2, 2, vec![0., 1., 1., 0.]).unwrap()
    }
    fn unit_box() -> Bounds {
        Bounds {
            min: [0.; 3],
            max: [20.; 3],
        }
    }

    #[test]
    fn planar_repeats_every_tile() {
        let map = checker();
        let m = Mapping {
            projection: Projection::PlanarXy,
            scale_u: 4.,
            ..Mapping::default()
        };
        let p = Projector::new(&map, m, unit_box());
        for (x, y) in [(0.3, 0.7), (1.1, 2.9), (3.3, 0.2)] {
            let a = p.grey([x, y, 0.], [0., 0., 1.]);
            let b = p.grey([x + 4., y + 8., 5.], [0., 0., 1.]);
            assert!((a - b).abs() < 1e-9);
        }
    }

    #[test]
    fn triplanar_on_a_vertical_wall_uses_its_plane_only() {
        let map = checker();
        let m = Mapping {
            scale_u: 4.,
            ..Mapping::default()
        };
        let p = Projector::new(&map, m, unit_box());
        // +X wall: U = y, V = z; moving in x does not change it.
        let a = p.grey([20., 1.2, 3.1], [1., 0., 0.]);
        let b = p.grey([25., 1.2, 3.1], [1., 0., 0.]);
        assert!((a - b).abs() < 1e-9);
        let planar = Projector::new(
            &map,
            Mapping {
                projection: Projection::PlanarYz,
                ..m
            },
            unit_box(),
        );
        assert!((a - planar.grey([20., 1.2, 3.1], [1., 0., 0.])).abs() < 1e-6);
    }

    #[test]
    fn cylinder_wraps_without_a_seam() {
        let map = HeightMap::new(4, 1, vec![0., 0.3, 0.9, 0.5]).unwrap();
        let m = Mapping {
            projection: Projection::Cylindrical,
            scale_u: 7.,
            blend: 0.,
            ..Mapping::default()
        };
        let p = Projector::new(&map, m, unit_box());
        // Just either side of the atan2 wrap (θ = ±π, the −X side of the centre).
        let eps = 1e-6;
        let a = p.grey([0., 10. + eps, 5.], [-1., 0., 0.]);
        let b = p.grey([0., 10. - eps, 5.], [-1., 0., 0.]);
        assert!((a - b).abs() < 1e-3, "{a} vs {b}");
    }

    #[test]
    fn cylinder_axis_x_matches_z_rotated() {
        let map = HeightMap::new(3, 3, (0..9).map(|i| i as f32 / 9.).collect()).unwrap();
        let mk = |axis| Mapping {
            projection: Projection::Cylindrical,
            axis,
            blend: 0.,
            scale_u: 3.,
            ..Mapping::default()
        };
        let pz = Projector::new(&map, mk(Axis::Z), unit_box());
        let px = Projector::new(&map, mk(Axis::X), unit_box());
        // Point (x, y, z) around Z equals point (z, x, y) around X.
        let (x, y, z) = (3., 17., 6.);
        let a = pz.grey([x, y, z], [0., 1., 0.]);
        let b = px.grey([z, x, y], [0., 0., 1.]);
        assert!((a - b).abs() < 1e-9);
    }

    #[test]
    fn cubic_weights_are_one_hot_inside_faces_and_sum_to_one() {
        assert_eq!(cubic_weights([0., 0., 1.], 1., 0.35), [0., 0., 1.]);
        let w = cubic_weights([FRAC_1_SQRT_2, FRAC_1_SQRT_2, 0.], 1., 0.35);
        assert!((w[0] - 0.5).abs() < 1e-3 && (w[1] - 0.5).abs() < 1e-3);
        let w = cubic_weights([0.6, 0.5, 0.62], 0.5, 0.35);
        assert!((w.iter().sum::<f64>() - 1.).abs() < 1e-9);
    }

    #[test]
    fn rotation_turns_about_the_tile_centre() {
        let map = HeightMap::new(2, 1, vec![0., 1.]).unwrap();
        let base = Mapping {
            projection: Projection::PlanarXy,
            scale_u: 2.,
            scale_v: 2.,
            ..Mapping::default()
        };
        let p0 = Projector::new(&map, base, unit_box());
        let p90 = Projector::new(
            &map,
            Mapping {
                rotation: 90.,
                ..base
            },
            unit_box(),
        );
        // A 90° turn maps U to V: sampling along y now reads what x used to.
        let a = p0.grey([0.5, 1., 0.], [0., 0., 1.]);
        let b = p90.grey([1., 1.5, 0.], [0., 0., 1.]);
        assert!((a - b).abs() < 1e-9, "{a} {b}");
    }
}
