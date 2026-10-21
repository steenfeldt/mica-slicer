// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/BuildVolume.cpp :: BuildVolume::BuildVolume, BuildVolume::all_paths_inside
// Source: prusaslicer@b028299c77 src/libslic3r/BuildVolume.hpp :: BuildVolume::BedEpsilon
// Source: prusaslicer@b028299c77 src/libslic3r/Geometry/Circle.cpp :: circle_ransac
// Source: orca@8500fcdcca src/libslic3r/BuildVolume.cpp :: BuildVolume::all_paths_inside
// Source: orca@8500fcdcca src/slic3r/GUI/PartPlate.cpp :: PartPlate::set_shape, PartPlate::check_outside
// Changes: The bed classification (rectangle, circle, convex, custom) and the toolpath test (extrusion centerlines, no width, `BedEpsilon`, max print height) follow BuildVolume. A custom (concave) bed is tested against its real outline, not the convex hull upstream uses as a stand-in (its FIXME), so nothing prints over a notch; the circle RANSAC samples evenly spaced point triples instead of std::sample with mt19937. Exclusion areas follow Orca's plate: each group of four `bed_exclude_area` points is a rectangle (its bounding box), used only under Orca rules. Mica adds the words for where a point is (edge and distance).

//! The printable area of a printer: the bed outline, its exclusion areas and the maximum print
//! height, and whether a toolpath point or an object stays inside it. The G-code export refuses
//! extrusions outside it; the placement preflight refuses objects outside it.

use crate::config::Config;

/// PrusaSlicer's `EPSILON`.
const EPSILON: f64 = 1e-4;
/// Tolerance of the toolpath test (`BuildVolume::BedEpsilon`), mm.
pub const BED_EPSILON: f64 = 3. * EPSILON;

/// How the bed outline is tested (`BuildVolume::Type`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    /// Its bounding box.
    Rectangle,
    /// A circle fitted to the outline (delta printers).
    Circle,
    /// A convex polygon.
    Convex,
    /// Any other polygon (tested against its real outline).
    Custom,
}

/// A bed exclusion area: an axis-aligned rectangle, bed mm.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Zone {
    pub min: [f64; 2],
    pub max: [f64; 2],
}

/// The printable volume of a printer.
#[derive(Clone, Debug)]
pub struct BuildVolume {
    shape: Shape,
    /// The outline, counter-clockwise.
    polygon: Vec<[f64; 2]>,
    pub min: [f64; 2],
    pub max: [f64; 2],
    center: [f64; 2],
    radius: f64,
    /// 0 for no limit.
    pub max_print_height: f64,
    /// Exclusion areas (Orca rules only).
    pub zones: Vec<Zone>,
}

/// Why a point is outside the printable volume.
#[derive(Clone, Debug, PartialEq)]
pub enum Outside {
    /// Off the bed: the words for where ("2.1 mm past the left edge of the bed").
    Bed(String),
    /// In exclusion area `zone`.
    Zone(Zone),
    /// Above the maximum print height.
    Height(f64),
}

impl Outside {
    /// "2.1 mm past the left edge of the bed", "in the excluded area X 0–18, Y 0–28 mm".
    pub fn describe(&self) -> String {
        match self {
            Outside::Bed(w) => w.clone(),
            Outside::Zone(z) => format!("in the excluded area {}", z.describe()),
            Outside::Height(h) => format!("above the maximum print height of {} mm", num(*h)),
        }
    }
}

impl Zone {
    /// "X 0–18, Y 0–28 mm"
    pub fn describe(&self) -> String {
        format!(
            "X {}–{}, Y {}–{} mm",
            num(self.min[0]),
            num(self.max[0]),
            num(self.min[1]),
            num(self.max[1])
        )
    }

    /// Whether `(x, y)` lies inside the area by more than `eps` (its edges are printable).
    pub fn contains(&self, x: f64, y: f64, eps: f64) -> bool {
        x > self.min[0] + eps
            && x < self.max[0] - eps
            && y > self.min[1] + eps
            && y < self.max[1] - eps
    }

    /// Whether the convex polygon `hull` reaches into the area by more than `eps`
    /// (separating axes: the rectangle's two and the hull's edge normals).
    pub fn overlaps_convex(&self, hull: &[[f64; 2]], eps: f64) -> bool {
        if hull.is_empty() {
            return false;
        }
        let (lo, hi) = bounds(hull);
        if hi[0] <= self.min[0] + eps
            || lo[0] >= self.max[0] - eps
            || hi[1] <= self.min[1] + eps
            || lo[1] >= self.max[1] - eps
        {
            return false;
        }
        if hull.len() < 3 {
            return true;
        }
        let corners = [
            self.min,
            [self.max[0], self.min[1]],
            self.max,
            [self.min[0], self.max[1]],
        ];
        let ccw = signed_area(hull) >= 0.;
        for i in 0..hull.len() {
            let (a, b) = (hull[i], hull[(i + 1) % hull.len()]);
            let len = (b[0] - a[0]).hypot(b[1] - a[1]);
            if len == 0. {
                continue;
            }
            // Signed distance to the right of the edge (outside for a counter-clockwise hull).
            let outside = |p: [f64; 2]| {
                let d = ((b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0])) / len;
                if ccw { -d } else { d }
            };
            if corners.iter().all(|&c| outside(c) >= -eps) {
                return false;
            }
        }
        true
    }
}

impl BuildVolume {
    /// The printable volume of `c` (`bed_shape`, `max_print_height`, and under Orca rules
    /// `bed_exclude_area`); `None` without a bed outline of at least three corners.
    pub fn from_config(c: &Config) -> Option<Self> {
        let bed: Vec<[f64; 2]> = c
            .points("bed_shape")?
            .into_iter()
            .map(|(x, y)| [x, y])
            .collect();
        let zones = if crate::rules::Rules::of(c).is_orca() {
            zones(&c.points("bed_exclude_area").unwrap_or_default())
        } else {
            Vec::new()
        };
        let mut v = Self::new(&bed, c.float("max_print_height").unwrap_or(0.))?;
        v.zones = zones;
        Some(v)
    }

    /// The volume of a bed outline and height (`BuildVolume::BuildVolume`).
    pub fn new(bed: &[[f64; 2]], max_print_height: f64) -> Option<Self> {
        if bed.len() < 3 || bed.iter().any(|p| !p[0].is_finite() || !p[1].is_finite()) {
            return None;
        }
        let mut polygon = bed.to_vec();
        let area = signed_area(&polygon);
        if area < 0. {
            polygon.reverse();
        }
        let area = area.abs();
        let (min, max) = bounds(&polygon);
        let hull = convex_hull(polygon.clone());
        let bbox_area = (max[0] - min[0]) * (max[1] - min[1]);
        // SCALED_EPSILON squared, in mm².
        let tiny = 1e-10;
        let mut shape = None;
        let mut center = [(min[0] + max[0]) / 2., (min[1] + max[1]) / 2.];
        let mut radius = 0.;
        if bed.len() >= 4 && (area - bbox_area).abs() < tiny {
            shape = Some(Shape::Rectangle);
        } else if bed.len() > 3
            && let Some((c, r)) = circle_ransac(bed)
        {
            let mut prev = bed[bed.len() - 1];
            let is_circle = bed.iter().all(|&p| {
                let mid = [(prev[0] + p[0]) / 2. - c[0], (prev[1] + p[1]) / 2. - c[1]];
                prev = p;
                ((p[0] - c[0]).hypot(p[1] - c[1]) - r).abs() <= 0.005
                    && r - mid[0].hypot(mid[1]) <= 3.
            });
            if is_circle {
                shape = Some(Shape::Circle);
                center = c;
                radius = r;
            }
        }
        let shape = shape.unwrap_or(if signed_area(&hull) - area < tiny {
            Shape::Convex
        } else {
            Shape::Custom
        });
        if shape == Shape::Convex {
            polygon = hull;
        }
        Some(Self {
            shape,
            polygon,
            min,
            max,
            center,
            radius,
            max_print_height: if max_print_height.is_finite() {
                max_print_height.max(0.)
            } else {
                0.
            },
            zones: Vec::new(),
        })
    }

    pub fn shape(&self) -> Shape {
        self.shape
    }

    /// Whether `(x, y)` is on the bed, `eps` outside its outline included.
    pub fn contains_xy(&self, x: f64, y: f64, eps: f64) -> bool {
        match self.shape {
            Shape::Rectangle => {
                x >= self.min[0] - eps
                    && x <= self.max[0] + eps
                    && y >= self.min[1] - eps
                    && y <= self.max[1] + eps
            }
            Shape::Circle => (x - self.center[0]).hypot(y - self.center[1]) <= self.radius + eps,
            Shape::Convex => {
                let p = &self.polygon;
                (0..p.len()).all(|i| {
                    let (a, b) = (p[i], p[(i + 1) % p.len()]);
                    let len = (b[0] - a[0]).hypot(b[1] - a[1]);
                    len == 0.
                        || ((b[0] - a[0]) * (y - a[1]) - (b[1] - a[1]) * (x - a[0])) / len >= -eps
                })
            }
            Shape::Custom => {
                point_in_polygon(&self.polygon, x, y)
                    || distance_to_outline(&self.polygon, x, y) <= eps
            }
        }
    }

    /// The exclusion area `(x, y)` lies in, if any.
    pub fn zone_at(&self, x: f64, y: f64, eps: f64) -> Option<Zone> {
        self.zones.iter().copied().find(|z| z.contains(x, y, eps))
    }

    /// The toolpath test of one extrusion point (centerline) at height `z`
    /// (`BuildVolume::all_paths_inside`, plus Orca's exclusion areas).
    pub fn check_path_point(&self, x: f64, y: f64, z: f64) -> Result<(), Outside> {
        if !self.contains_xy(x, y, BED_EPSILON) {
            return Err(Outside::Bed(self.where_off(x, y)));
        }
        if let Some(zone) = self.zone_at(x, y, BED_EPSILON) {
            return Err(Outside::Zone(zone));
        }
        if self.max_print_height > 0. && z > self.max_print_height + BED_EPSILON {
            return Err(Outside::Height(self.max_print_height));
        }
        Ok(())
    }

    /// "2.1 mm past the left edge of the bed" for a point off the bed.
    pub fn where_off(&self, x: f64, y: f64) -> String {
        match self.shape {
            Shape::Rectangle => {
                let mut past = [
                    ("left", self.min[0] - x),
                    ("right", x - self.max[0]),
                    ("front", self.min[1] - y),
                    ("back", y - self.max[1]),
                ];
                past.sort_by(|a, b| b.1.total_cmp(&a.1));
                let parts: Vec<String> = past
                    .iter()
                    .filter(|(_, d)| *d > 0.)
                    .take(2)
                    .map(|(side, d)| format!("{d:.1} mm past the {side} edge"))
                    .collect();
                if parts.is_empty() {
                    "past the edge of the bed".into()
                } else {
                    format!("{} of the bed", parts.join(" and "))
                }
            }
            Shape::Circle => {
                let d = (x - self.center[0]).hypot(y - self.center[1]) - self.radius;
                format!("{:.1} mm outside the round bed", d.max(0.))
            }
            Shape::Convex | Shape::Custom => format!(
                "{:.1} mm outside the bed outline",
                distance_to_outline(&self.polygon, x, y)
            ),
        }
    }
}

/// Orca's exclusion areas: each group of four points is one rectangle (`PartPlate` takes the
/// bounding box of each group); an incomplete trailing group and empty rectangles are ignored.
pub fn zones(points: &[(f64, f64)]) -> Vec<Zone> {
    points
        .as_chunks::<4>()
        .0
        .iter()
        .map(|g| {
            let pts: Vec<[f64; 2]> = g.iter().map(|&(x, y)| [x, y]).collect();
            let (min, max) = bounds(&pts);
            Zone { min, max }
        })
        .filter(|z| z.max[0] - z.min[0] > EPSILON && z.max[1] - z.min[1] > EPSILON)
        .collect()
}

fn num(v: f64) -> String {
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn bounds(points: &[[f64; 2]]) -> ([f64; 2], [f64; 2]) {
    let mut min = [f64::INFINITY; 2];
    let mut max = [f64::NEG_INFINITY; 2];
    for p in points {
        for i in 0..2 {
            min[i] = min[i].min(p[i]);
            max[i] = max[i].max(p[i]);
        }
    }
    (min, max)
}

fn signed_area(p: &[[f64; 2]]) -> f64 {
    (0..p.len())
        .map(|i| {
            let (a, b) = (p[i], p[(i + 1) % p.len()]);
            a[0] * b[1] - b[0] * a[1]
        })
        .sum::<f64>()
        / 2.
}

/// Convex hull (monotone chain, counter-clockwise).
pub fn convex_hull(mut points: Vec<[f64; 2]>) -> Vec<[f64; 2]> {
    points.retain(|p| p[0].is_finite() && p[1].is_finite());
    points.sort_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
    points.dedup();
    if points.len() < 3 {
        return points;
    }
    let cross = |o: [f64; 2], a: [f64; 2], b: [f64; 2]| {
        (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
    };
    let mut lower: Vec<[f64; 2]> = Vec::new();
    for &p in &points {
        while lower.len() >= 2 && cross(lower[lower.len() - 2], lower[lower.len() - 1], p) <= 0. {
            lower.pop();
        }
        lower.push(p);
    }
    let mut upper: Vec<[f64; 2]> = Vec::new();
    for &p in points.iter().rev() {
        while upper.len() >= 2 && cross(upper[upper.len() - 2], upper[upper.len() - 1], p) <= 0. {
            upper.pop();
        }
        upper.push(p);
    }
    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

fn point_in_polygon(p: &[[f64; 2]], x: f64, y: f64) -> bool {
    let mut inside = false;
    let mut j = p.len() - 1;
    for i in 0..p.len() {
        let (a, b) = (p[i], p[j]);
        if (a[1] > y) != (b[1] > y) && x < (b[0] - a[0]) * (y - a[1]) / (b[1] - a[1]) + a[0] {
            inside = !inside;
        }
        j = i;
    }
    inside
}

fn distance_to_outline(p: &[[f64; 2]], x: f64, y: f64) -> f64 {
    (0..p.len())
        .map(|i| {
            let (a, b) = (p[i], p[(i + 1) % p.len()]);
            let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
            let l2 = dx * dx + dy * dy;
            let t = if l2 == 0. {
                0.
            } else {
                (((x - a[0]) * dx + (y - a[1]) * dy) / l2).clamp(0., 1.)
            };
            (x - a[0] - t * dx).hypot(y - a[1] - t * dy)
        })
        .fold(f64::INFINITY, f64::min)
}

/// Circumcentre of three points (`Geometry::circle_center`); `None` when nearly collinear.
fn circle_center(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> Option<[f64; 2]> {
    let (b, c) = ([b[0] - a[0], b[1] - a[1]], [c[0] - a[0], c[1] - a[1]]);
    let d = 2. * (b[0] * c[1] - b[1] * c[0]);
    if d.abs() < EPSILON {
        return None;
    }
    let (b2, c2) = (b[0] * b[0] + b[1] * b[1], c[0] * c[0] + c[1] * c[1]);
    Some([
        a[0] + (c[1] * b2 - b[1] * c2) / d,
        a[1] + (b[0] * c2 - c[0] * b2) / d,
    ])
}

/// Best three-point circle (`circle_ransac`, 20 iterations as `BuildVolume` uses): centre and
/// mean radius of the one with the smallest largest deviation.
fn circle_ransac(points: &[[f64; 2]]) -> Option<([f64; 2], f64)> {
    const ITERATIONS: usize = 20;
    let n = points.len();
    let mut best: Option<([f64; 2], f64, f64)> = None;
    for k in 0..ITERATIONS {
        let offset = k * n / (3 * ITERATIONS);
        let pick = |j: usize| points[(offset + j * n / 3) % n];
        let Some(c) = circle_center(pick(0), pick(1), pick(2)) else {
            continue;
        };
        let r = points
            .iter()
            .map(|p| (p[0] - c[0]).hypot(p[1] - c[1]))
            .sum::<f64>()
            / n as f64;
        let err = points
            .iter()
            .map(|p| ((p[0] - c[0]).hypot(p[1] - c[1]) - r).abs())
            .fold(0., f64::max);
        if best.is_none_or(|(_, _, e)| err < e) {
            best = Some((c, r, err));
        }
    }
    best.map(|(c, r, _)| (c, r))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn circle(n: usize, r: f64) -> Vec<[f64; 2]> {
        (0..n)
            .map(|i| {
                let a = std::f64::consts::TAU * i as f64 / n as f64;
                [r * a.cos(), r * a.sin()]
            })
            .collect()
    }

    #[test]
    fn classifies_beds_like_buildvolume() {
        let rect = BuildVolume::new(&[[0., 0.], [250., 0.], [250., 220.], [0., 220.]], 0.).unwrap();
        assert_eq!(rect.shape(), Shape::Rectangle);
        let delta = BuildVolume::new(&circle(72, 100.), 300.).unwrap();
        assert_eq!(delta.shape(), Shape::Circle);
        let diamond =
            BuildVolume::new(&[[125., 0.], [250., 125.], [125., 250.], [0., 125.]], 0.).unwrap();
        assert_eq!(diamond.shape(), Shape::Convex);
        let ell = BuildVolume::new(
            &[
                [0., 0.],
                [200., 0.],
                [200., 100.],
                [100., 100.],
                [100., 200.],
                [0., 200.],
            ],
            0.,
        )
        .unwrap();
        assert_eq!(ell.shape(), Shape::Custom);
    }

    #[test]
    fn toolpath_points_use_centerlines_and_bed_epsilon() {
        let rect =
            BuildVolume::new(&[[0., 0.], [250., 0.], [250., 220.], [0., 220.]], 210.).unwrap();
        assert!(rect.check_path_point(0., 110., 0.2).is_ok());
        assert!(rect.check_path_point(-0.0002, 110., 0.2).is_ok());
        assert_eq!(
            rect.check_path_point(-2.1, 110., 0.2),
            Err(Outside::Bed("2.1 mm past the left edge of the bed".into()))
        );
        assert_eq!(
            rect.check_path_point(10., 10., 210.2),
            Err(Outside::Height(210.))
        );
        let delta = BuildVolume::new(&circle(72, 100.), 0.).unwrap();
        assert!(delta.check_path_point(70., 70., 1.).is_ok());
        // Inside the bounding box, outside the round bed.
        assert!(matches!(
            delta.check_path_point(90., 90., 1.),
            Err(Outside::Bed(_))
        ));
        let diamond =
            BuildVolume::new(&[[125., 0.], [250., 125.], [125., 250.], [0., 125.]], 0.).unwrap();
        assert!(diamond.check_path_point(125., 125., 1.).is_ok());
        assert!(diamond.check_path_point(30., 30., 1.).is_err());
        let ell = BuildVolume::new(
            &[
                [0., 0.],
                [200., 0.],
                [200., 100.],
                [100., 100.],
                [100., 200.],
                [0., 200.],
            ],
            0.,
        )
        .unwrap();
        assert!(ell.check_path_point(50., 150., 1.).is_ok());
        assert!(ell.check_path_point(150., 150., 1.).is_err());
        assert!(ell.check_path_point(100., 150., 1.).is_ok());
    }

    #[test]
    fn exclusion_areas_are_groups_of_four_points() {
        let z = zones(&[(0., 0.), (18., 0.), (18., 28.), (0., 28.)]);
        assert_eq!(
            z,
            [Zone {
                min: [0., 0.],
                max: [18., 28.]
            }]
        );
        assert!(zones(&[(0., 0.)]).is_empty());
        let zone = z[0];
        assert!(zone.contains(5., 5., BED_EPSILON));
        assert!(!zone.contains(18., 5., BED_EPSILON));
        let square = [[10., 10.], [30., 10.], [30., 30.], [10., 30.]];
        assert!(zone.overlaps_convex(&square, 0.02));
        let away = [[20., 10.], [40., 10.], [40., 30.], [20., 30.]];
        assert!(!zone.overlaps_convex(&away, 0.02));
        // A triangle whose bounding box overlaps the area but whose edge passes it by.
        let tri = [[40., 10.], [40., 50.], [0., 50.]];
        assert!(!zone.overlaps_convex(&tri, 0.02));
    }
}
