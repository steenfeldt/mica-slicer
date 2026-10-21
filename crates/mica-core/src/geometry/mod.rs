// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original
// Source (original): Mica geometry module organization.
// Changes: Expose the scoped Clipper cache job API.

//! 2D geometry in scaled integer coordinates.
//!
//! One coordinate unit is one nanometre (1e-6 mm), the same resolution PrusaSlicer uses
//! (`SCALING_FACTOR`), so ported algorithms keep their tolerances unchanged.

pub mod arc_welder;
pub mod clip;
pub mod clip64;
pub mod clip_cache;
pub mod elephant_foot;
pub mod medial_axis;
pub mod simplify;
pub mod voronoi;
mod voronoi_cache;
mod voronoi_probe;

/// Scaled coordinate: one unit is 1e-6 mm.
pub type Coord = i64;

/// Coordinate units per millimetre.
pub const SCALE: f64 = 1_000_000.0;

/// Converts millimetres to scaled coordinates.
pub fn scale(mm: f64) -> Coord {
    (mm * SCALE).round() as Coord
}

/// Converts scaled coordinates to millimetres.
pub fn unscale(v: Coord) -> f64 {
    v as f64 / SCALE
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Point {
    pub x: Coord,
    pub y: Coord,
}

impl Point {
    pub const fn new(x: Coord, y: Coord) -> Self {
        Self { x, y }
    }

    pub fn from_mm(x: f64, y: f64) -> Self {
        Self::new(scale(x), scale(y))
    }

    pub fn distance_to(self, other: Point) -> f64 {
        let dx = (other.x - self.x) as f64;
        let dy = (other.y - self.y) as f64;
        (dx * dx + dy * dy).sqrt()
    }

    pub fn distance_sq(self, other: Point) -> i128 {
        let dx = (other.x - self.x) as i128;
        let dy = (other.y - self.y) as i128;
        dx * dx + dy * dy
    }
}

/// Closed polygon. Counter-clockwise for outer contours, clockwise for holes.
pub type Polygon = Vec<Point>;

/// Open polyline.
pub type Polyline = Vec<Point>;

/// A polygon with holes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ExPolygon {
    pub contour: Polygon,
    pub holes: Vec<Polygon>,
}

impl ExPolygon {
    /// Net area in mm², holes subtracted.
    pub fn area_mm2(&self) -> f64 {
        let scaled =
            signed_area(&self.contour) + self.holes.iter().map(|h| signed_area(h)).sum::<f64>();
        scaled / (SCALE * SCALE)
    }

    /// The contour followed by the holes.
    pub fn polygons(&self) -> impl Iterator<Item = &Polygon> {
        std::iter::once(&self.contour).chain(self.holes.iter())
    }
}

/// Signed area in scaled units²; positive for counter-clockwise polygons.
pub fn signed_area(poly: &[Point]) -> f64 {
    let n = poly.len();
    if n < 3 {
        return 0.0;
    }
    let mut twice = 0i128;
    for i in 0..n {
        let a = poly[i];
        let b = poly[(i + 1) % n];
        twice += a.x as i128 * b.y as i128 - b.x as i128 * a.y as i128;
    }
    twice as f64 / 2.0
}

/// Length of an open polyline, in scaled units.
pub fn polyline_length(line: &[Point]) -> f64 {
    line.windows(2).map(|w| w[0].distance_to(w[1])).sum()
}

/// Length of a closed polygon, in scaled units.
pub fn polygon_length(poly: &[Point]) -> f64 {
    match (poly.first(), poly.last()) {
        (Some(&first), Some(&last)) if poly.len() > 1 => {
            polyline_length(poly) + last.distance_to(first)
        }
        _ => 0.0,
    }
}

/// Axis-aligned bounding box.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BoundingBox {
    pub min: Point,
    pub max: Point,
}

impl BoundingBox {
    pub fn from_points<'a>(points: impl IntoIterator<Item = &'a Point>) -> Option<Self> {
        let mut it = points.into_iter();
        let first = *it.next()?;
        let mut bb = Self {
            min: first,
            max: first,
        };
        for p in it {
            bb.min.x = bb.min.x.min(p.x);
            bb.min.y = bb.min.y.min(p.y);
            bb.max.x = bb.max.x.max(p.x);
            bb.max.y = bb.max.y.max(p.y);
        }
        Some(bb)
    }

    pub fn merge(self, other: Self) -> Self {
        Self {
            min: Point::new(self.min.x.min(other.min.x), self.min.y.min(other.min.y)),
            max: Point::new(self.max.x.max(other.max.x), self.max.y.max(other.max.y)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(size_mm: f64) -> Polygon {
        let s = scale(size_mm);
        vec![
            Point::new(0, 0),
            Point::new(s, 0),
            Point::new(s, s),
            Point::new(0, s),
        ]
    }

    #[test]
    fn area_and_orientation() {
        let sq = square(10.0);
        assert_eq!(signed_area(&sq) / (SCALE * SCALE), 100.0);
        let mut cw = sq.clone();
        cw.reverse();
        assert_eq!(signed_area(&cw) / (SCALE * SCALE), -100.0);
    }

    #[test]
    fn expolygon_area_subtracts_holes() {
        let mut hole = square(2.0);
        hole.reverse();
        let ex = ExPolygon {
            contour: square(10.0),
            holes: vec![hole],
        };
        assert_eq!(ex.area_mm2(), 96.0);
    }

    #[test]
    fn lengths() {
        assert_eq!(unscale(polygon_length(&square(10.0)) as Coord), 40.0);
        assert_eq!(unscale(polyline_length(&square(10.0)) as Coord), 30.0);
    }
}
