// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The printer's bed as the profile describes it: outline polygon, printable height,
//! exclusion zones, name and size. Geometry helpers build what the renderer draws.

use glam::{Vec2, Vec3};
use mica_core::config::Config;

/// Grid steps in millimetres; the shader draws the minor grid, the renderer the major lines.
pub const MINOR_STEP: f32 = 10.;
pub const MAJOR_STEP: f32 = 50.;

#[derive(Clone, Debug, PartialEq)]
pub struct Bed {
    /// Bed outline in printer coordinates, counter-clockwise.
    pub polygon: Vec<[f32; 2]>,
    pub min: Vec3,
    pub max: Vec3,
    /// `max_print_height` (Orca `printable_height`).
    pub height: f32,
    /// Printer model or preset name for the plate label.
    pub name: String,
    /// `bed_exclude_area` zones (Orca and Prusa spell it the same); each is a polygon.
    pub exclusions: Vec<Vec<[f32; 2]>>,
    /// Where models are placed: `mica_core::input::bed_center`, the CLI's rule.
    placement: [f32; 2],
    /// The printer model's bed sheet (STL) and texture (SVG), when the profile names them
    /// and an installed slicer or checkout has them. Set by the app, not by `from_config`.
    pub assets: super::bed_assets::AssetFiles,
}

/// Where an object sits relative to the printable volume.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    Inside,
    /// Partly or fully beyond the outline or above the printable height.
    Outside,
    /// Inside the outline but overlapping an exclusion zone.
    Excluded,
}

impl Default for Bed {
    fn default() -> Self {
        Self::from_config(&Config::defaults())
    }
}
impl Bed {
    pub fn from_config(c: &Config) -> Self {
        let polygon: Vec<_> = c
            .points("bed_shape")
            .filter(|points| {
                points.len() >= 3 && points.iter().all(|(x, y)| x.is_finite() && y.is_finite())
            })
            .unwrap_or_else(|| vec![(0., 0.), (250., 0.), (250., 220.), (0., 220.)])
            .into_iter()
            .map(|(x, y)| [x as f32, y as f32])
            .collect();
        let polygon = ccw(polygon);
        let (min, max) = bounds(&polygon);
        let height = c
            .float("max_print_height")
            .filter(|h| h.is_finite() && *h > 0.)
            .unwrap_or(200.) as f32;
        let name = ["printer_model", "printer_settings_id"]
            .into_iter()
            .filter_map(|k| c.string(k))
            .map(str::trim)
            .find(|s| !s.is_empty())
            .unwrap_or("Printer")
            .to_owned();
        let exclusions = c
            .points("bed_exclude_area")
            .or_else(|| c.extra("bed_exclude_area").and_then(parse_points))
            .map(|points| {
                // One flat point list; split into quads when the count allows, else one polygon.
                let points: Vec<[f32; 2]> = points
                    .into_iter()
                    .map(|(x, y)| [x as f32, y as f32])
                    .collect();
                if points.len() >= 8 && points.len().is_multiple_of(4) {
                    points.chunks(4).map(|c| ccw(c.to_vec())).collect()
                } else {
                    vec![ccw(points)]
                }
            })
            .unwrap_or_default()
            .into_iter()
            .filter(|zone| zone.len() >= 3 && polygon_area(zone).abs() > 1e-3)
            .collect();
        let (x, y) = mica_core::input::bed_center(c);
        Self {
            polygon,
            min,
            max,
            height,
            name,
            exclusions,
            placement: [x, y],
            assets: Default::default(),
        }
    }
    /// The bed's centre; in XY exactly where the CLI places a model
    /// (`mica_core::input::bed_center`), so an opened STL lands where `mica slice` puts it.
    pub fn center(&self) -> Vec3 {
        let [x, y] = self.placement;
        Vec3::new(x, y, (self.min.z + self.max.z) * 0.5)
    }
    pub fn size(&self) -> Vec2 {
        (self.max - self.min).truncate()
    }
    /// `250 × 220 × 270 mm`
    pub fn size_label(&self) -> String {
        let s = self.size();
        format!("{} × {} × {} mm", trim(s.x), trim(s.y), trim(self.height))
    }
    pub fn contains(&self, p: Vec2) -> bool {
        point_in_polygon(&self.polygon, p)
    }
    /// Classifies an object's world bounding box (already in bed coordinates).
    pub fn placement(&self, min: Vec3, max: Vec3) -> Placement {
        let corners = [
            Vec2::new(min.x, min.y),
            Vec2::new(max.x, min.y),
            Vec2::new(max.x, max.y),
            Vec2::new(min.x, max.y),
        ];
        let eps = 0.01;
        if max.z > self.height + eps
            || min.z < -eps
            || corners
                .iter()
                .any(|c| !self.contains_with_tolerance(*c, eps))
        {
            return Placement::Outside;
        }
        let overlaps = |zone: &Vec<[f32; 2]>| {
            let (zmin, zmax) = bounds(zone);
            min.x < zmax.x - eps
                && max.x > zmin.x + eps
                && min.y < zmax.y - eps
                && max.y > zmin.y + eps
        };
        if self.exclusions.iter().any(overlaps) {
            Placement::Excluded
        } else {
            Placement::Inside
        }
    }
    fn contains_with_tolerance(&self, p: Vec2, eps: f32) -> bool {
        self.contains(p)
            || (self.min.x - eps..=self.max.x + eps).contains(&p.x)
                && (self.min.y - eps..=self.max.y + eps).contains(&p.y)
                && self.polygon.len() == 4
    }
    /// Grid lines clipped to the outline's bounding box, split into minor and major sets.
    /// Lines are in bed coordinates; each is `[start, end]`.
    pub fn grid(&self) -> Grid {
        let mut minor = vec![];
        let mut major = vec![];
        let (min, max) = (self.min.truncate(), self.max.truncate());
        let push = |out: &mut Vec<[[f32; 2]; 2]>, x: f32, vertical: bool| {
            out.push(if vertical {
                [[x, min.y], [x, max.y]]
            } else {
                [[min.x, x], [max.x, x]]
            });
        };
        for vertical in [true, false] {
            let (lo, hi) = if vertical {
                (min.x, max.x)
            } else {
                (min.y, max.y)
            };
            let mut k = (lo / MINOR_STEP).ceil() as i64;
            while (k as f32) * MINOR_STEP <= hi + 1e-3 {
                let v = k as f32 * MINOR_STEP;
                let is_major = (v / MAJOR_STEP - (v / MAJOR_STEP).round()).abs() < 1e-4;
                push(if is_major { &mut major } else { &mut minor }, v, vertical);
                k += 1;
            }
        }
        Grid { minor, major }
    }
    /// Vertical edges and the top loop of the printable volume.
    pub fn volume_edges(&self) -> Vec<[Vec3; 2]> {
        let n = self.polygon.len();
        let mut out = Vec::with_capacity(2 * n);
        for (i, p) in self.polygon.iter().enumerate() {
            let a = Vec3::new(p[0], p[1], 0.);
            out.push([a, a.with_z(self.height)]);
            let q = self.polygon[(i + 1) % n];
            out.push([a.with_z(self.height), Vec3::new(q[0], q[1], self.height)]);
        }
        out
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Grid {
    pub minor: Vec<[[f32; 2]; 2]>,
    pub major: Vec<[[f32; 2]; 2]>,
}

fn trim(v: f32) -> String {
    if (v - v.round()).abs() < 0.05 {
        format!("{}", v.round() as i64)
    } else {
        format!("{v:.1}")
    }
}
fn parse_points(text: &str) -> Option<Vec<(f64, f64)>> {
    text.split(',')
        .filter(|s| !s.trim().is_empty())
        .map(|pair| {
            let (x, y) = pair.trim().split_once('x')?;
            Some((x.trim().parse().ok()?, y.trim().parse().ok()?))
        })
        .collect()
}
pub fn polygon_area(p: &[[f32; 2]]) -> f32 {
    0.5 * (0..p.len())
        .map(|i| Vec2::from(p[i]).perp_dot(Vec2::from(p[(i + 1) % p.len()])))
        .sum::<f32>()
}
fn ccw(mut p: Vec<[f32; 2]>) -> Vec<[f32; 2]> {
    if polygon_area(&p) < 0. {
        p.reverse();
    }
    p
}
fn bounds(p: &[[f32; 2]]) -> (Vec3, Vec3) {
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = -min;
    for q in p {
        let q = Vec3::new(q[0], q[1], 0.);
        min = min.min(q);
        max = max.max(q);
    }
    (min, max)
}
pub fn point_in_polygon(p: &[[f32; 2]], q: Vec2) -> bool {
    let mut inside = false;
    let n = p.len();
    let mut j = n - 1;
    for i in 0..n {
        let (a, b) = (Vec2::from(p[i]), Vec2::from(p[j]));
        if (a.y > q.y) != (b.y > q.y) && q.x < (b.x - a.x) * (q.y - a.y) / (b.y - a.y) + a.x {
            inside = !inside;
        }
        j = i;
    }
    inside
}
/// Ear clipping; supports concave bed outlines and exclusion polygons.
pub fn triangulate(p: &[[f32; 2]]) -> Vec<[u32; 3]> {
    let mut order: Vec<_> = (0..p.len()).collect();
    if polygon_area(p) < 0. {
        order.reverse();
    }
    let mut out = Vec::new();
    while order.len() >= 3 {
        let mut found = false;
        for j in 0..order.len() {
            let [a, b, c] = [
                order[(j + order.len() - 1) % order.len()],
                order[j],
                order[(j + 1) % order.len()],
            ];
            let [pa, pb, pc] = [a, b, c].map(|i| Vec2::from(p[i]));
            if (pb - pa).perp_dot(pc - pb) <= 1e-6 {
                continue;
            }
            if order.iter().any(|&i| {
                i != a && i != b && i != c && {
                    let q = Vec2::from(p[i]);
                    (pb - pa).perp_dot(q - pa) >= 0.
                        && (pc - pb).perp_dot(q - pb) >= 0.
                        && (pa - pc).perp_dot(q - pc) >= 0.
                }
            }) {
                continue;
            }
            out.push([a as u32, b as u32, c as u32]);
            order.remove(j);
            found = true;
            break;
        }
        if !found {
            break;
        }
    }
    out
}
/// Flat quads of width `w` along each closed polygon edge, for crisp outlines in 3D.
pub fn outline_strip(p: &[[f32; 2]], w: f32, z: f32) -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
    let n = p.len();
    let mut vertices = Vec::with_capacity(2 * n);
    let mut faces = Vec::with_capacity(2 * n);
    let inward = if polygon_area(p) >= 0. { 1. } else { -1. };
    for i in 0..n {
        let prev = Vec2::from(p[(i + n - 1) % n]);
        let cur = Vec2::from(p[i]);
        let next = Vec2::from(p[(i + 1) % n]);
        let d0 = (cur - prev).normalize_or_zero();
        let d1 = (next - cur).normalize_or_zero();
        let n0 = Vec2::new(-d0.y, d0.x) * inward;
        let n1 = Vec2::new(-d1.y, d1.x) * inward;
        let m = (n0 + n1).normalize_or(n1);
        let scale = 1. / m.dot(n1).max(0.3);
        let inner = cur + m * w * scale;
        vertices.push([cur.x, cur.y, z]);
        vertices.push([inner.x, inner.y, z]);
        let (a, b) = (2 * i as u32, 2 * ((i + 1) % n) as u32);
        faces.push([a, b, a + 1]);
        faces.push([a + 1, b, b + 1]);
    }
    (vertices, faces)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn core_one() -> Bed {
        let mut c = Config::defaults();
        c.set_raw("bed_shape", "0x0,250x0,250x220,0x220").unwrap();
        c.set_raw("max_print_height", "270").unwrap();
        c.set_raw("printer_model", "COREONE").unwrap();
        Bed::from_config(&c)
    }
    #[test]
    fn profile_values_and_label() {
        let bed = core_one();
        assert_eq!(bed.size(), Vec2::new(250., 220.));
        assert_eq!(bed.height, 270.);
        assert_eq!(bed.name, "COREONE");
        assert_eq!(bed.size_label(), "250 × 220 × 270 mm");
        assert_eq!(bed.volume_edges().len(), 8);
    }
    #[test]
    fn grid_from_bed_shape() {
        let g = core_one().grid();
        // X: 0,10,…,250 → 26 lines, 6 major (0,50,…,250). Y: 0..220 → 23 lines, 5 major.
        assert_eq!(g.minor.len() + g.major.len(), 26 + 23);
        assert_eq!(g.major.len(), 11);
        assert!(g.major.contains(&[[50., 0.], [50., 220.]]));
        assert!(g.minor.contains(&[[0., 10.], [250., 10.]]));
        // Centre-origin bed keeps lines on multiples of the step.
        let mut c = Config::defaults();
        c.set_raw("bed_shape", "-125x-110,125x-110,125x110,-125x110")
            .unwrap();
        let g = Bed::from_config(&c).grid();
        assert!(g.major.contains(&[[0., -110.], [0., 110.]]));
        assert!(
            g.minor
                .iter()
                .all(|l| (l[0][0] / 10.).fract() == 0. || (l[0][1] / 10.).fract() == 0.)
        );
    }
    #[test]
    fn exclusions_from_typed_points_and_extras() {
        let mut c = Config::defaults();
        c.set_raw("bed_shape", "0x0,230x0,230x250,0x250").unwrap();
        c.set_raw("bed_exclude_area", "0x238,230x238,230x250,0x250")
            .unwrap();
        let bed = Bed::from_config(&c);
        assert_eq!(bed.exclusions.len(), 1);
        assert_eq!(
            bed.placement(Vec3::new(10., 230., 0.), Vec3::new(30., 245., 10.)),
            Placement::Excluded
        );
        assert_eq!(
            bed.placement(Vec3::new(10., 10., 0.), Vec3::new(30., 45., 10.)),
            Placement::Inside
        );
        assert_eq!(
            bed.placement(Vec3::new(-1., 10., 0.), Vec3::new(30., 45., 10.)),
            Placement::Outside
        );
        assert_eq!(
            bed.placement(Vec3::new(10., 10., 0.), Vec3::new(30., 45., 300.)),
            Placement::Outside
        );
        assert_eq!(
            parse_points("0x238, 230x238").unwrap(),
            vec![(0., 238.), (230., 238.)]
        );
        assert!(parse_points("bogus").is_none());
    }
    #[test]
    fn polygon_helpers() {
        let square = [[0., 0.], [10., 0.], [10., 10.], [0., 10.]];
        assert!(point_in_polygon(&square, Vec2::new(5., 5.)));
        assert!(!point_in_polygon(&square, Vec2::new(15., 5.)));
        assert_eq!(triangulate(&square).len(), 2);
        assert_eq!(
            triangulate(&[[0., 0.], [10., 0.], [10., 10.], [5., 5.], [0., 10.]]).len(),
            3
        );
        let (v, f) = outline_strip(&square, 1., 0.);
        assert_eq!((v.len(), f.len()), (8, 8));
        assert!(
            v[1].iter()
                .zip([1., 1., 0.])
                .all(|(a, b)| (a - b).abs() < 1e-5)
        );
        // Clockwise input is normalised.
        let mut c = Config::defaults();
        c.set_raw("bed_shape", "0x0,0x10,10x10,10x0").unwrap();
        assert!(polygon_area(&Bed::from_config(&c).polygon) > 0.);
    }
}
