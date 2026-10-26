// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: prusaslicer@b028299c77 src/libslic3r/Fill/FillRectilinear.cpp :: FillRectilinear::fill_surface_by_lines
// Source: prusaslicer@b028299c77 src/libslic3r/Fill/FillRectilinear.cpp :: FillRectilinear::fill_surface_by_multilines
// Source: prusaslicer@b028299c77 src/libslic3r/Fill/FillRectilinear.cpp :: chain_monotonic_regions
// Source: prusaslicer@b028299c77 src/libslic3r/Fill/FillRectilinear.hpp :: FillRectilinear
// Source: prusaslicer@b028299c77 src/libslic3r/Fill/FillRectilinear.cpp :: FillSupportBase::fill_surface
// Changes: Rust port. Intersection pointers become indices into their vertical line; the ant-colony
// Changes: path matrix is a lazily filled hash map instead of a dense n²×4 matrix; the random generator
// Changes: is mt19937_64 with libc++ bounded draws (including singleton draws). The unused 3-opt stub and
// Changes: the debug SVG output are left out. InfillFailedException becomes `None`.
// Changes: Support base fills (normal and organic, Prusa and Orca rules) use connect_base_support.
// Source: orca@8500fcdcca src/libslic3r/Fill/FillRectilinear.cpp :: FillRectilinear::fill_surface_by_lines, traverse_graph_generate_polylines
// Changes: Zig-zag family (has_consistent_pattern): object-box line layout, alternating start ends,
// Changes: next-line-only links, and Orca's single-precision horiz_move shift of the first line.

//! Line-based fills: rectilinear, monotonic, monotonic lines, the zig-zag family (see `zag`), and the
//! multi-sweep patterns (grid, triangles, stars, cubic).
//!
//! The area is rotated so the infill lines are vertical. Vertical lines are intersected with the
//! area's outline shrunk by a little ("outer") and by half a line ("inner"). Line ends are linked
//! along the inner outline into zig-zags, and the monotonic variants additionally split the area
//! into regions that can each be printed strictly left to right.

use std::collections::HashMap;

use super::base::{
    EPSILON, SCALED_EPSILON, adjust_solid_spacing, align_point_to_grid, bbox_of, chain_polylines,
    merge_point, remove_duplicate_points, remove_duplicate_points_polygon, remove_small,
    remove_sticks, remove_sticks_all, rotate_point, rotate_points,
};
use super::connect::connect_infill;
use crate::geometry::clip::{DEFAULT_MITER_LIMIT, Join, offset, offset_expolygon, to_polygons};
use crate::geometry::{BoundingBox, Coord, ExPolygon, Point, Polygon, Polyline, SCALE, scale};

/// Shrink the input a bit first to not push the infill lines out of the perimeters.
const INFILL_OVERLAP_OVER_SPACING: f64 = 0.45;

/// Parameters of one line-based fill of one ExPolygon (the subset of `Fill` + `FillParams` used here).
#[derive(Clone, Copy, Debug)]
pub(crate) struct LineFillParams {
    /// Line spacing at 100%, mm. May be adjusted for solid fills.
    pub spacing: f64,
    /// 0–1.
    pub density: f64,
    /// Rotation of the pattern, radians (`_infill_direction().first`).
    pub angle: f64,
    pub dont_adjust: bool,
    pub monotonic: bool,
    /// mm.
    pub anchor_length: f64,
    /// mm. Below 0.05 the lines are not connected at all.
    pub anchor_length_max: f64,
    /// Longest perimeter segment linking two lines, scaled; 0 = unlimited.
    pub link_max_length: Coord,
    /// Zig-zag patterns (`has_consistent_pattern`): the lines are laid out over `object_box`
    /// instead of the area's own box, every other line is walked downwards and lines only link
    /// to the next one, so the zig-zags line up from layer to layer.
    pub consistent: bool,
    /// The object's extended bounding box (`extended_object_bounding_box`), rotation-proof.
    pub object_box: Option<BoundingBox>,
    /// Orca's `horiz_move`: scaled shift of the first line (Cross Zag, Locked Zag), computed in
    /// single precision like Orca. `None` keeps PrusaSlicer's line positions.
    pub horiz_move: Option<f32>,
}

impl LineFillParams {
    fn full_infill(&self) -> bool {
        self.density > 0.9999
    }
    fn dont_connect(&self) -> bool {
        self.anchor_length_max < 0.05
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum IType {
    OuterLow,
    OuterHigh,
    InnerLow,
    InnerHigh,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LinkType {
    Horizontal,
    Up,
    Down,
    Phony,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LinkQuality {
    Invalid,
    Valid,
    TooLong,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Side {
    Left,
    Right,
}

/// Intersection point of a vertical line with a polygon segment.
#[derive(Clone, Copy, Debug)]
struct SegmentIntersection {
    /// Contour index in `ExPolygonWithOffset`.
    i_contour: usize,
    /// Segment index in that contour (the segment ends at this point index).
    i_segment: usize,
    /// y position as a rational number pos_p / pos_q.
    pos_p: i64,
    pos_q: i64,
    prev_on_contour: i32,
    next_on_contour: i32,
    ty: IType,
    prev_on_contour_type: LinkType,
    next_on_contour_type: LinkType,
    prev_on_contour_quality: LinkQuality,
    next_on_contour_quality: LinkQuality,
    /// Was the segment along the vertical line (going up from here) consumed?
    consumed_vertical_up: bool,
    /// Was the segment of the inner perimeter to the right consumed?
    consumed_perimeter_right: bool,
}

impl SegmentIntersection {
    fn new(i_contour: usize, i_segment: usize) -> Self {
        Self {
            i_contour,
            i_segment,
            pos_p: 0,
            pos_q: 1,
            prev_on_contour: 0,
            next_on_contour: 0,
            ty: IType::OuterLow,
            prev_on_contour_type: LinkType::Horizontal,
            next_on_contour_type: LinkType::Horizontal,
            prev_on_contour_quality: LinkQuality::Valid,
            next_on_contour_quality: LinkQuality::Valid,
            consumed_vertical_up: false,
            consumed_perimeter_right: false,
        }
    }

    fn phony_outer(ty: IType, pos: Coord) -> Self {
        Self {
            pos_p: pos,
            ty,
            prev_on_contour: -1,
            next_on_contour: -1,
            prev_on_contour_type: LinkType::Phony,
            next_on_contour_type: LinkType::Phony,
            prev_on_contour_quality: LinkQuality::Invalid,
            next_on_contour_quality: LinkQuality::Invalid,
            ..Self::new(usize::MAX, usize::MAX)
        }
    }

    /// Rounded y position.
    fn pos(&self) -> Coord {
        let half = self.pos_q >> 1;
        let p = if self.pos_p < 0 {
            self.pos_p - half
        } else {
            self.pos_p + half
        };
        p / self.pos_q
    }

    fn cmp_pos(&self, other: &Self) -> std::cmp::Ordering {
        (self.pos_p as i128 * other.pos_q as i128).cmp(&(other.pos_p as i128 * self.pos_q as i128))
    }

    fn is_inner(&self) -> bool {
        matches!(self.ty, IType::InnerLow | IType::InnerHigh)
    }
    fn is_outer(&self) -> bool {
        matches!(self.ty, IType::OuterLow | IType::OuterHigh)
    }
    fn is_low(&self) -> bool {
        matches!(self.ty, IType::InnerLow | IType::OuterLow)
    }

    fn has_left_horizontal(&self) -> bool {
        self.prev_on_contour_type == LinkType::Horizontal
    }
    fn has_right_horizontal(&self) -> bool {
        self.next_on_contour_type == LinkType::Horizontal
    }
    fn has_left_vertical_up(&self) -> bool {
        self.prev_on_contour_type == LinkType::Up
    }
    fn has_left_vertical_down(&self) -> bool {
        self.prev_on_contour_type == LinkType::Down
    }
    fn has_left_vertical(&self) -> bool {
        self.has_left_vertical_up() || self.has_left_vertical_down()
    }
    fn has_right_vertical_up(&self) -> bool {
        self.next_on_contour_type == LinkType::Up
    }
    fn has_right_vertical_down(&self) -> bool {
        self.next_on_contour_type == LinkType::Down
    }
    fn has_right_vertical(&self) -> bool {
        self.has_right_vertical_up() || self.has_right_vertical_down()
    }
    fn has_vertical(&self, side: Side) -> bool {
        match side {
            Side::Left => self.has_left_vertical(),
            Side::Right => self.has_right_vertical(),
        }
    }

    fn left_horizontal(&self) -> i32 {
        if self.has_left_horizontal() {
            self.prev_on_contour
        } else {
            -1
        }
    }
    fn right_horizontal(&self) -> i32 {
        if self.has_right_horizontal() {
            self.next_on_contour
        } else {
            -1
        }
    }
    fn horizontal(&self, side: Side) -> i32 {
        match side {
            Side::Left => self.left_horizontal(),
            Side::Right => self.right_horizontal(),
        }
    }
    fn horizontal_quality(&self, side: Side) -> LinkQuality {
        match side {
            Side::Left => self.prev_on_contour_quality,
            Side::Right => self.next_on_contour_quality,
        }
    }
    fn left_vertical(&self) -> i32 {
        if self.has_left_vertical() {
            self.prev_on_contour
        } else {
            -1
        }
    }
    fn right_vertical(&self) -> i32 {
        if self.has_right_vertical() {
            self.next_on_contour
        } else {
            -1
        }
    }
    fn vertical_up(&self) -> i32 {
        if self.has_left_vertical_up() {
            self.prev_on_contour
        } else if self.has_right_vertical_up() {
            self.next_on_contour
        } else {
            -1
        }
    }
    fn vertical_up_quality(&self) -> LinkQuality {
        if self.has_left_vertical_up() {
            self.prev_on_contour_quality
        } else {
            self.next_on_contour_quality
        }
    }
    fn vertical_down(&self) -> i32 {
        if self.has_left_vertical_down() {
            self.prev_on_contour
        } else if self.has_right_vertical_down() {
            self.next_on_contour
        } else {
            -1
        }
    }
    fn vertical_down_quality(&self) -> LinkQuality {
        if self.has_left_vertical_down() {
            self.prev_on_contour_quality
        } else {
            self.next_on_contour_quality
        }
    }
    fn vertical_outside(&self) -> i32 {
        if self.is_low() {
            self.vertical_down()
        } else {
            self.vertical_up()
        }
    }
    fn vertical_outside_quality(&self) -> LinkQuality {
        if self.is_low() {
            self.vertical_down_quality()
        } else {
            self.vertical_up_quality()
        }
    }
}

/// A vertical line with its intersection points, sorted by y.
#[derive(Clone, Debug)]
struct SegmentedIntersectionLine {
    pos: Coord,
    intersections: Vec<SegmentIntersection>,
}

type Segs = Vec<SegmentedIntersectionLine>;

/// An ExPolygon with its outer (slightly shrunk) and inner (shrunk by half a line) offsets.
#[derive(Clone, Debug)]
struct ExPolygonWithOffset {
    polygons_src: ExPolygon,
    polygons_outer: Vec<Polygon>,
    polygons_inner: Vec<Polygon>,
    n_contours_outer: usize,
    n_contours_inner: usize,
    n_contours: usize,
}

fn rotate_expolygon(ex: &mut ExPolygon, angle: f64) {
    rotate_points(&mut ex.contour, angle);
    for h in &mut ex.holes {
        rotate_points(h, angle);
    }
}

impl ExPolygonWithOffset {
    fn new(expolygon: &ExPolygon, angle: f64, aoffset1: Coord, aoffset2: Coord) -> Self {
        let mut src = expolygon.clone();
        if angle != 0.0 {
            rotate_expolygon(&mut src, angle);
        }
        // Upstream short-circuits: the holes are only cleaned when the contour had no sticks.
        if !remove_sticks(&mut src.contour) {
            remove_sticks_all(&mut src.holes);
        }
        let mut outer = if aoffset1 == 0 {
            to_polygons(std::slice::from_ref(&src))
        } else {
            offset_expolygon(&src, aoffset1 as f64, Join::Miter, DEFAULT_MITER_LIMIT)
        };
        let mut inner = if aoffset2 < 0 {
            offset(
                &outer,
                -((aoffset1 - aoffset2) as f64),
                Join::Miter,
                DEFAULT_MITER_LIMIT,
            )
        } else {
            Vec::new()
        };
        let min_area_threshold = 0.01 * aoffset2 as f64 * aoffset2 as f64;
        remove_small(&mut outer, min_area_threshold);
        remove_small(&mut inner, min_area_threshold);
        remove_sticks_all(&mut outer);
        remove_sticks_all(&mut inner);
        for p in outer.iter_mut().chain(inner.iter_mut()) {
            remove_duplicate_points_polygon(p);
        }
        let (n_outer, n_inner) = (outer.len(), inner.len());
        Self {
            polygons_src: src,
            polygons_outer: outer,
            polygons_inner: inner,
            n_contours_outer: n_outer,
            n_contours_inner: n_inner,
            n_contours: n_outer + n_inner,
        }
    }

    /// A copy rotated by `angle`.
    fn rotated(&self, angle: f64) -> Self {
        let mut out = self.clone();
        if angle != 0.0 {
            rotate_expolygon(&mut out.polygons_src, angle);
            for p in out
                .polygons_outer
                .iter_mut()
                .chain(out.polygons_inner.iter_mut())
            {
                rotate_points(p, angle);
            }
        }
        out
    }

    fn is_contour_outer(&self, idx: usize) -> bool {
        idx < self.n_contours_outer
    }

    fn contour(&self, idx: usize) -> &Polygon {
        if self.is_contour_outer(idx) {
            &self.polygons_outer[idx]
        } else {
            &self.polygons_inner[idx - self.n_contours_outer]
        }
    }

    fn bounding_box_src(&self) -> BoundingBox {
        bbox_of(self.polygons_src.polygons().flatten()).unwrap_or(BoundingBox {
            min: Point::default(),
            max: Point::default(),
        })
    }
}

/// Euclidean length of a piece of a closed polygon from `p1` on segment `seg1` to `p2` on `seg2`
/// (segment indices name the end point of an edge in the forward direction).
fn segment_length(poly: &Polygon, seg1: usize, p1: Point, seg2: usize, p2: Point) -> f64 {
    let mut prev = p1;
    let mut len = 0.0;
    let mut step = |pt: Point| {
        len += prev.distance_to(pt);
        prev = pt;
    };
    if seg1 <= seg2 {
        for &pt in &poly[seg1..seg2] {
            step(pt);
        }
    } else {
        for &pt in poly[seg1..].iter().chain(&poly[..seg2]) {
            step(pt);
        }
    }
    len + prev.distance_to(p2)
}

/// Appends the polygon points between `seg1` and `seg2` (forward, `seg2` itself excluded).
fn polygon_segment_append(out: &mut Polyline, poly: &Polygon, seg1: usize, seg2: usize) {
    if seg1 == seg2 {
    } else if seg1 < seg2 {
        out.extend_from_slice(&poly[seg1..seg2]);
    } else {
        out.extend_from_slice(&poly[seg1..]);
        out.extend_from_slice(&poly[..seg2]);
    }
}

/// Appends the polygon points between `seg1` and `seg2` going backwards.
fn polygon_segment_append_reversed(out: &mut Polyline, poly: &Polygon, seg1: usize, seg2: usize) {
    if seg1 >= seg2 {
        for i in (seg2 + 1..=seg1).rev() {
            out.push(poly[i - 1]);
        }
    } else {
        for i in (1..=seg1).rev() {
            out.push(poly[i - 1]);
        }
        for i in (seg2 + 1..=poly.len()).rev() {
            out.push(poly[i - 1]);
        }
    }
}

fn distance_of_segments(poly: &Polygon, seg1: usize, seg2: usize, forward: bool) -> i64 {
    let mut d = seg2 as i64 - seg1 as i64;
    if !forward {
        d = -d;
    }
    if d < 0 {
        d += poly.len() as i64;
    }
    d
}

/// Can the perimeter be followed from this intersection to the neighbouring vertical line?
fn intersection_on_prev_next_vertical_line_valid(
    segs: &Segs,
    i_vline: usize,
    i_intersection: usize,
    side: Side,
) -> bool {
    let it_this = &segs[i_vline].intersections[i_intersection];
    if it_this.has_vertical(side) {
        return false;
    }
    let i_other = it_this.horizontal(side);
    if i_other == -1 {
        return false;
    }
    let i_other = i_other as usize;
    let vline_other = &segs[if side == Side::Right {
        i_vline + 1
    } else {
        i_vline - 1
    }];
    let it_other = &vline_other.intersections[i_other];
    let it_other2 = &vline_other.intersections[if it_other.is_low() {
        i_other - 1
    } else {
        i_other + 1
    }];
    if it_other2.is_inner() {
        // Only perimeter segments connecting to the end of a vertical segment are followed.
        return false;
    }
    if it_this.horizontal_quality(side) != LinkQuality::Valid {
        return false;
    }
    if if side == Side::Right {
        it_this.consumed_perimeter_right
    } else {
        it_other.consumed_perimeter_right
    } {
        return false;
    }
    if if it_other.is_low() {
        it_other.consumed_vertical_up
    } else {
        vline_other.intersections[i_other - 1].consumed_vertical_up
    } {
        return false;
    }
    true
}

/// Length of the perimeter from intersection `i1` on `i_vline` to `i2` on `i_vline + 1`.
fn measure_perimeter_horizontal_segment_length(
    pwo: &ExPolygonWithOffset,
    segs: &Segs,
    i_vline: usize,
    i1: usize,
    i2: usize,
) -> f64 {
    let vline = &segs[i_vline];
    let it = &vline.intersections[i1];
    let vline2 = &segs[i_vline + 1];
    let it2 = &vline2.intersections[i2];
    let poly = pwo.contour(it.i_contour);
    let p1 = Point::new(vline.pos, it.pos());
    let p2 = Point::new(vline2.pos, it2.pos());
    if it.is_low() {
        segment_length(poly, it.i_segment, p1, it2.i_segment, p2)
    } else {
        segment_length(poly, it2.i_segment, p2, it.i_segment, p1)
    }
}

/// Appends the perimeter from `i1` on `i_vline` to `i2` on the next (or previous) vertical line,
/// without the first point and with the last point.
#[allow(clippy::too_many_arguments)]
fn emit_perimeter_prev_next_segment(
    pwo: &ExPolygonWithOffset,
    segs: &Segs,
    i_vline: usize,
    i_inner_contour: usize,
    i1: usize,
    i2: usize,
    out: &mut Polyline,
    dir_is_next: bool,
) {
    let i_vline_other = if dir_is_next {
        i_vline + 1
    } else {
        i_vline - 1
    };
    let itsct = &segs[i_vline].intersections[i1];
    let il2 = &segs[i_vline_other];
    let itsct2 = &il2.intersections[i2];
    let poly = pwo.contour(i_inner_contour);
    let forward = itsct.is_low() == dir_is_next;
    if forward {
        polygon_segment_append(out, poly, itsct.i_segment, itsct2.i_segment);
    } else {
        polygon_segment_append_reversed(out, poly, itsct.i_segment, itsct2.i_segment);
    }
    out.push(Point::new(il2.pos, itsct2.pos()));
}

fn measure_perimeter_segment_on_vertical_line_length(
    pwo: &ExPolygonWithOffset,
    segs: &Segs,
    i_vline: usize,
    i1: usize,
    i2: usize,
    forward: bool,
) -> f64 {
    let il = &segs[i_vline];
    let itsct = &il.intersections[i1];
    let itsct2 = &il.intersections[i2];
    let poly = pwo.contour(itsct.i_contour);
    let p1 = Point::new(il.pos, itsct.pos());
    let p2 = Point::new(il.pos, itsct2.pos());
    if forward {
        segment_length(poly, itsct.i_segment, p1, itsct2.i_segment, p2)
    } else {
        segment_length(poly, itsct2.i_segment, p2, itsct.i_segment, p1)
    }
}

/// Appends the perimeter from `i1` to `i2` on the same vertical line.
#[allow(clippy::too_many_arguments)]
fn emit_perimeter_segment_on_vertical_line(
    pwo: &ExPolygonWithOffset,
    segs: &Segs,
    i_vline: usize,
    i_inner_contour: usize,
    i1: usize,
    i2: usize,
    out: &mut Polyline,
    forward: bool,
) {
    let il = &segs[i_vline];
    let itsct = &il.intersections[i1];
    let itsct2 = &il.intersections[i2];
    let poly = pwo.contour(i_inner_contour);
    if forward {
        polygon_segment_append(out, poly, itsct.i_segment, itsct2.i_segment);
    } else {
        polygon_segment_append_reversed(out, poly, itsct.i_segment, itsct2.i_segment);
    }
    out.push(Point::new(il.pos, itsct2.pos()));
}

/// Intersects equally spaced vertical lines with the contours. `None` if the result is inconsistent
/// (upstream throws `InfillFailedException`).
fn slice_region_by_vertical_lines(
    pwo: &ExPolygonWithOffset,
    n_vlines: usize,
    x0: Coord,
    line_spacing: Coord,
) -> Option<Segs> {
    let mut segs: Segs = (0..n_vlines)
        .map(|i| SegmentedIntersectionLine {
            pos: x0 + i as Coord * line_spacing,
            intersections: Vec::new(),
        })
        .collect();
    for i_contour in 0..pwo.n_contours {
        let contour = pwo.contour(i_contour);
        let n = contour.len();
        if n < 2 {
            continue;
        }
        for i_segment in 0..n {
            let i_prev = if i_segment == 0 { n } else { i_segment } - 1;
            let p1 = contour[i_prev];
            let p2 = contour[i_segment];
            let (mut l, mut r) = (p1.x, p2.x);
            if l > r {
                std::mem::swap(&mut l, &mut r);
            }
            let mut il = (l - x0) / line_spacing;
            while il * line_spacing + x0 < l {
                il += 1;
            }
            il = il.max(0);
            let mut ir = (r - x0 + line_spacing) / line_spacing;
            while ir * line_spacing + x0 > r {
                ir -= 1;
            }
            ir = ir.min(segs.len() as Coord - 1);
            if il > ir {
                continue;
            }
            for i in il..=ir {
                let this_x = segs[i as usize].pos;
                let mut is = SegmentIntersection::new(i_contour, i_segment);
                if p1.x == this_x {
                    if p2.x == this_x {
                        // Ignore strictly vertical segments.
                        continue;
                    }
                    let p0 = contour[if i_prev == 0 { n } else { i_prev } - 1];
                    if (p0.x - p1.x) as i128 * (p2.x - p1.x) as i128 > 0 {
                        // A contour point touching the line from one side.
                        continue;
                    }
                    is.pos_p = p1.y;
                    is.pos_q = 1;
                } else if p2.x == this_x {
                    let p3 = contour[(i_segment + 1) % n];
                    if (p3.x - p2.x) as i128 * (p1.x - p2.x) as i128 > 0 {
                        continue;
                    }
                    is.pos_p = p2.y;
                    is.pos_q = 1;
                } else {
                    if p2.x > p1.x {
                        is.pos_p = this_x - p1.x;
                        is.pos_q = p2.x - p1.x;
                    } else {
                        is.pos_p = p1.x - this_x;
                        is.pos_q = p1.x - p2.x;
                    }
                    is.pos_p *= p2.y - p1.y;
                    is.pos_p += p1.y * is.pos_q;
                }
                segs[i as usize].intersections.push(is);
            }
        }
    }

    // Sort the intersections along their lines, classify them, remove duplicates where a contour
    // vertex touches a line.
    for sil in &mut segs {
        let v = &mut sil.intersections;
        v.sort_by(|a, b| a.cmp_pos(b));
        let mut j = 0;
        for i in 0..v.len() {
            let contour = pwo.contour(v[i].i_contour);
            let i_segment = v[i].i_segment;
            let i_prev = if i_segment == 0 {
                contour.len()
            } else {
                i_segment
            } - 1;
            let low = contour[i_segment].x - contour[i_prev].x > 0;
            v[i].ty = match (pwo.is_contour_outer(v[i].i_contour), low) {
                (true, true) => IType::OuterLow,
                (true, false) => IType::OuterHigh,
                (false, true) => IType::InnerLow,
                (false, false) => IType::InnerHigh,
            };
            let mut take_next = true;
            if j > 0 {
                let is2 = v[j - 1];
                let is = v[i];
                if is.i_contour == is2.i_contour && is.pos_q == 1 && is2.pos_q == 1 {
                    if is.pos_p == is2.pos_p {
                        // Two successive segments meet exactly at the vertical line.
                        take_next = false;
                    } else if is.ty == is2.ty {
                        // A Z shaped path with its centre segment on this line: keep the longer run.
                        if !low {
                            v[j - 1] = v[i];
                        }
                        take_next = false;
                    }
                }
            }
            if take_next {
                if j < i {
                    v[j] = v[i];
                }
                j += 1;
            }
        }
        v.truncate(j);
    }

    // Verify the segments. If something is wrong, give up.
    for sil in &segs {
        let v = &sil.intersections;
        if v.len() & 1 != 0 {
            return None;
        }
        let mut i = 0;
        while i < v.len() {
            if v[i].ty != IType::OuterLow {
                return None;
            }
            let mut j = i + 1;
            if j >= v.len() || !matches!(v[j].ty, IType::InnerLow | IType::OuterHigh) {
                return None;
            }
            while j < v.len() && v[j].is_inner() {
                j += 1;
            }
            if j >= v.len() || j & 1 != 1 || v[j].ty != IType::OuterHigh {
                return None;
            }
            if !(i + 1 == j || v[j - 1].ty == IType::InnerHigh) {
                return None;
            }
            i = j + 1;
        }
    }
    Some(segs)
}

/// Links each intersection to its two neighbours along its contour, either on the same vertical line
/// or on the line left / right of it, and grades the links.
fn connect_segment_intersections_by_contours(
    pwo: &ExPolygonWithOffset,
    segs: &mut Segs,
    dont_connect: bool,
    link_max_length: Coord,
) {
    for i_vline in 0..segs.len() {
        for i_intersection in 0..segs[i_vline].intersections.len() {
            let itsct = segs[i_vline].intersections[i_intersection];
            let poly = pwo.contour(itsct.i_contour);
            let forward = itsct.is_low();

            // 1) Closest intersection of the same contour and orientation on the previous / next line.
            let mut iprev: i32 = -1;
            let mut d_prev = i64::MAX;
            if i_vline > 0 {
                for (i, it2) in segs[i_vline - 1].intersections.iter().enumerate() {
                    if itsct.i_contour == it2.i_contour && itsct.ty == it2.ty {
                        let d = distance_of_segments(poly, it2.i_segment, itsct.i_segment, forward);
                        if d < d_prev {
                            iprev = i as i32;
                            d_prev = d;
                        }
                    }
                }
            }
            let mut inext: i32 = -1;
            let mut d_next = i64::MAX;
            if i_vline + 1 < segs.len() {
                for (i, it2) in segs[i_vline + 1].intersections.iter().enumerate() {
                    if itsct.i_contour == it2.i_contour && itsct.ty == it2.ty {
                        let d = distance_of_segments(poly, itsct.i_segment, it2.i_segment, forward);
                        if d < d_next {
                            inext = i as i32;
                            d_next = d;
                        }
                    }
                }
            }

            // 2) Possible connections on the same vertical line.
            let mut same_prev = false;
            let mut same_next = false;
            for (i, it2) in segs[i_vline].intersections.iter().enumerate() {
                if i != i_intersection && it2.i_contour == itsct.i_contour && it2.ty != itsct.ty {
                    let d = distance_of_segments(poly, it2.i_segment, itsct.i_segment, forward);
                    if d < d_prev {
                        iprev = i as i32;
                        d_prev = d;
                        same_prev = true;
                    }
                    let d = distance_of_segments(poly, itsct.i_segment, it2.i_segment, forward);
                    if d < d_next {
                        inext = i as i32;
                        d_next = d;
                        same_next = true;
                    }
                }
            }

            let ii = i_intersection as i32;
            let mut out = itsct;
            out.prev_on_contour = iprev;
            out.prev_on_contour_type = if same_prev {
                if iprev < ii {
                    LinkType::Down
                } else {
                    LinkType::Up
                }
            } else {
                LinkType::Horizontal
            };
            out.next_on_contour = inext;
            out.next_on_contour_type = if same_next {
                if inext < ii {
                    LinkType::Down
                } else {
                    LinkType::Up
                }
            } else {
                LinkType::Horizontal
            };

            let il = &segs[i_vline].intersections;
            // Only follow a vertical perimeter segment if it skips just the outer intersections.
            let skips_inner = |other: i32| {
                let (a, b) = if (other as usize) < i_intersection {
                    (other as usize, i_intersection)
                } else {
                    (i_intersection, other as usize)
                };
                il[a + 1..b].iter().any(|it| it.is_inner())
            };
            if same_prev && skips_inner(iprev) {
                out.prev_on_contour_quality = LinkQuality::Invalid;
            }
            if same_next && skips_inner(inext) {
                out.next_on_contour_quality = LinkQuality::Invalid;
            }
            // Both closest intersections on this line and at the same side: don't trace this contour.
            if same_prev && same_next && ((iprev > ii) == (inext > ii)) {
                out.prev_on_contour_quality = LinkQuality::Invalid;
                out.next_on_contour_quality = LinkQuality::Invalid;
            }

            if dont_connect {
                if out.prev_on_contour_quality == LinkQuality::Valid {
                    out.prev_on_contour_quality = LinkQuality::TooLong;
                }
                if out.next_on_contour_quality == LinkQuality::Valid {
                    out.next_on_contour_quality = LinkQuality::TooLong;
                }
            } else if link_max_length > 0 {
                let max = link_max_length as f64;
                if out.prev_on_contour_quality == LinkQuality::Valid
                    && iprev >= 0
                    && (if same_prev {
                        measure_perimeter_segment_on_vertical_line_length(
                            pwo,
                            segs,
                            i_vline,
                            iprev as usize,
                            i_intersection,
                            forward,
                        )
                    } else {
                        measure_perimeter_horizontal_segment_length(
                            pwo,
                            segs,
                            i_vline - 1,
                            iprev as usize,
                            i_intersection,
                        )
                    }) > max
                {
                    out.prev_on_contour_quality = LinkQuality::TooLong;
                }
                if out.next_on_contour_quality == LinkQuality::Valid
                    && inext >= 0
                    && (if same_next {
                        measure_perimeter_segment_on_vertical_line_length(
                            pwo,
                            segs,
                            i_vline,
                            i_intersection,
                            inext as usize,
                            forward,
                        )
                    } else {
                        measure_perimeter_horizontal_segment_length(
                            pwo,
                            segs,
                            i_vline,
                            i_intersection,
                            inext as usize,
                        )
                    }) > max
                {
                    out.next_on_contour_quality = LinkQuality::TooLong;
                }
            }
            segs[i_vline].intersections[i_intersection] = out;
        }

        // Make LinkQuality::Invalid symmetric on vertical connections.
        let il = &mut segs[i_vline].intersections;
        for i in 0..il.len() {
            let it = il[i];
            if it.has_left_vertical() && it.prev_on_contour_quality == LinkQuality::Invalid {
                il[it.left_vertical() as usize].prev_on_contour_quality = LinkQuality::Invalid;
            }
            if it.has_right_vertical() && it.next_on_contour_quality == LinkQuality::Invalid {
                il[it.right_vertical() as usize].next_on_contour_quality = LinkQuality::Invalid;
            }
        }
    }
}

/// Where the outer contour squeezes the inner contour into two loops along one vertical line, inserts a
/// phony OUTER_HIGH / OUTER_LOW pair so the monotonic region detection sees two separate runs.
fn pinch_contours_insert_phony_outer_intersections(segs: &mut Segs) {
    for i_vline in 1..segs.len() {
        if segs[i_vline].intersections.is_empty() {
            continue;
        }
        let mut insert_after = Vec::new();
        {
            let il = &segs[i_vline].intersections;
            let end = il.len() - 1;
            let mut it = 1;
            while it < end {
                if il[it].ty == IType::OuterHigh {
                    it += 2;
                } else {
                    let hi = it + 1;
                    let lo2 = it + 2;
                    it = lo2;
                    if il[lo2].ty == IType::InnerLow {
                        let up = il[hi].vertical_up();
                        let dn = il[lo2].vertical_down();
                        if dn + 1 != up {
                            insert_after.push(hi);
                        }
                    }
                }
            }
        }
        if insert_after.is_empty() {
            continue;
        }
        let old = std::mem::take(&mut segs[i_vline].intersections);
        let mut map = Vec::with_capacity(old.len());
        let mut temp = Vec::with_capacity(old.len() + 2 * insert_after.len());
        let mut i = 0;
        for &after in &insert_after {
            while i <= after {
                map.push(temp.len() as i32);
                temp.push(old[i]);
                i += 1;
            }
            let pos = (temp.last().map_or(0, SegmentIntersection::pos) + old[i].pos()) / 2;
            temp.push(SegmentIntersection::phony_outer(IType::OuterHigh, pos));
            temp.push(SegmentIntersection::phony_outer(IType::OuterLow, pos));
        }
        while i < old.len() {
            map.push(temp.len() as i32);
            temp.push(old[i]);
            i += 1;
        }
        for ip in &mut temp {
            if ip.has_left_vertical() {
                ip.prev_on_contour = map[ip.prev_on_contour as usize];
            }
            if ip.has_right_vertical() {
                ip.next_on_contour = map[ip.next_on_contour as usize];
            }
        }
        segs[i_vline].intersections = temp;
        for ip in &mut segs[i_vline - 1].intersections {
            if ip.has_right_horizontal() {
                ip.next_on_contour = map[ip.next_on_contour as usize];
            }
        }
        if i_vline + 1 < segs.len() {
            for ip in &mut segs[i_vline + 1].intersections {
                if ip.has_left_horizontal() {
                    ip.prev_on_contour = map[ip.prev_on_contour as usize];
                }
            }
        }
    }
}

fn end_of_vertical_run_raw(il: &[SegmentIntersection], start: usize) -> usize {
    let mut it = start;
    loop {
        it += 1;
        if il[it].ty == IType::OuterHigh {
            break;
        }
    }
    if il[it - 1].is_inner() {
        it -= 1;
    }
    it
}

/// The last INNER_HIGH of the vertical run starting at the INNER_LOW `start`, following valid vertical
/// links up along the contour.
fn end_of_vertical_run(il: &[SegmentIntersection], start: usize) -> usize {
    let mut end = end_of_vertical_run_raw(il, start);
    loop {
        let e = &il[end];
        let up = e.vertical_up();
        let quality = if e.has_left_vertical_up() {
            e.prev_on_contour_quality
        } else {
            e.next_on_contour_quality
        };
        if up == -1 || quality != LinkQuality::Valid {
            break;
        }
        end = end_of_vertical_run_raw(il, up as usize);
    }
    end
}

/// Traverses the intersection graph, producing zig-zag polylines connected along the inner contour.
fn traverse_graph_generate_polylines(
    pwo: &ExPolygonWithOffset,
    segs: &mut Segs,
    consistent: bool,
    polylines_out: &mut Vec<Polyline>,
) {
    // Outer-only chords are marked consumed.
    for vline in segs.iter_mut() {
        let v = &mut vline.intersections;
        for i in 0..v.len().saturating_sub(1) {
            if v[i].ty == IType::OuterLow && v[i + 1].ty == IType::OuterHigh {
                v[i].consumed_vertical_up = true;
            }
        }
    }

    // A simple "sweep left to right" creates longer continuous paths than shortest-distance chaining.
    let mut i_vline = 0usize;
    let mut i_intersection: Option<usize> = None;
    loop {
        if i_intersection.is_none() {
            let mut found = None;
            'search: for (i_vline2, vline) in segs.iter().enumerate() {
                // Consistent patterns (zig-zag) start every other line from its top end.
                let forward = !consistent || i_vline2.is_multiple_of(2);
                let n = vline.intersections.len();
                for k in 0..n {
                    let i = if forward { k } else { n - k - 1 };
                    let it = &vline.intersections[i];
                    if it.is_outer() {
                        let consumed = if it.is_low() {
                            it.consumed_vertical_up
                        } else {
                            vline.intersections[i - 1].consumed_vertical_up
                        };
                        if !consumed {
                            found = Some((i_vline2, i));
                            break 'search;
                        }
                    }
                }
            }
            let Some((v, i)) = found else { break };
            i_vline = v;
            i_intersection = Some(i);
            polylines_out.push(vec![Point::new(
                segs[v].pos,
                segs[v].intersections[i].pos(),
            )]);
        }
        let cur = polylines_out.len() - 1;
        let mut it = i_intersection.unwrap();
        let going_up = segs[i_vline].intersections[it].is_low();
        let mut try_connect = false;
        {
            let v = &mut segs[i_vline].intersections;
            if going_up {
                if v[it].is_inner() {
                    it -= 1;
                }
                loop {
                    v[it].consumed_vertical_up = true;
                    it += 1;
                    if v[it].ty == IType::OuterHigh {
                        break;
                    }
                }
                if v[it - 1].is_inner() {
                    it -= 1;
                    try_connect = true;
                }
            } else {
                if v[it].is_inner() {
                    v[it].consumed_vertical_up = true;
                }
                loop {
                    it -= 1;
                    v[it].consumed_vertical_up = true;
                    if v[it].ty == IType::OuterLow {
                        break;
                    }
                }
                if v[it + 1].is_inner() {
                    it += 1;
                    try_connect = true;
                }
            }
        }
        if try_connect {
            let here = segs[i_vline].intersections[it];
            let pos = segs[i_vline].pos;
            let i_prev = here.left_horizontal();
            let i_next = here.right_horizontal();
            // Consistent patterns always continue to the next line, never the previous one.
            let prev_valid = !consistent
                && intersection_on_prev_next_vertical_line_valid(segs, i_vline, it, Side::Left);
            let next_valid =
                intersection_on_prev_next_vertical_line_valid(segs, i_vline, it, Side::Right);
            if i_prev != -1 {
                segs[i_vline - 1].intersections[i_prev as usize].consumed_perimeter_right = true;
            }
            if i_next != -1 {
                segs[i_vline].intersections[it].consumed_perimeter_right = true;
            }

            // Zig-zag to the previous or next vertical line along the perimeter.
            if prev_valid || next_valid {
                let mut take_next = next_valid;
                if prev_valid && next_valid {
                    // Take the shorter segment.
                    let dist_prev = measure_perimeter_horizontal_segment_length(
                        pwo,
                        segs,
                        i_vline - 1,
                        i_prev as usize,
                        it,
                    );
                    let dist_next = measure_perimeter_horizontal_segment_length(
                        pwo,
                        segs,
                        i_vline,
                        it,
                        i_next as usize,
                    );
                    take_next = dist_next < dist_prev;
                }
                let pl = &mut polylines_out[cur];
                pl.push(Point::new(pos, here.pos()));
                let other = if take_next { i_next } else { i_prev } as usize;
                emit_perimeter_prev_next_segment(
                    pwo,
                    segs,
                    i_vline,
                    here.i_contour,
                    it,
                    other,
                    pl,
                    take_next,
                );
                if take_next {
                    i_vline += 1;
                } else {
                    i_vline -= 1;
                }
                i_intersection = Some(other);
                continue;
            }

            // Connect to a previous or next point on the same vertical line.
            let i_vertical = here.vertical_outside();
            let quality = if i_vertical == -1
                || segs[i_vline].intersections
                    [(i_vertical + if going_up { 0 } else { -1 }) as usize]
                    .consumed_vertical_up
            {
                LinkQuality::Invalid
            } else {
                here.vertical_outside_quality()
            };
            if quality != LinkQuality::Invalid {
                let i_vertical = i_vertical as usize;
                polylines_out[cur].push(Point::new(pos, here.pos()));
                if quality == LinkQuality::Valid {
                    let forward = if going_up {
                        here.has_left_vertical_up()
                    } else {
                        here.has_right_vertical_down()
                    };
                    emit_perimeter_segment_on_vertical_line(
                        pwo,
                        segs,
                        i_vline,
                        here.i_contour,
                        it,
                        i_vertical,
                        &mut polylines_out[cur],
                        forward,
                    );
                } else {
                    // Skip the connecting contour and start a new path.
                    polylines_out.push(vec![Point::new(
                        pos,
                        segs[i_vline].intersections[i_vertical].pos(),
                    )]);
                }
                let v = &mut segs[i_vline].intersections;
                let range = if going_up {
                    it..i_vertical
                } else {
                    i_vertical..it
                };
                for k in range {
                    v[k].consumed_vertical_up = true;
                }
                v[it].consumed_perimeter_right = true;
                let k = if going_up { it + 1 } else { it - 1 };
                v[k].consumed_perimeter_right = true;
                i_intersection = Some(i_vertical);
                continue;
            }

            // No way to continue: take the rest of the line up to the outer contour.
            it = if going_up { it + 1 } else { it - 1 };
        }

        // Finish the current vertical line.
        let cur = polylines_out.len() - 1;
        let p = Point::new(segs[i_vline].pos, segs[i_vline].intersections[it].pos());
        let pl = &mut polylines_out[cur];
        pl.push(p);
        remove_duplicate_points(pl);
        if pl.len() <= 1
            || (pl.len() == 2
                && (pl[0].x - pl[1].x).abs() < SCALED_EPSILON
                && (pl[0].y - pl[1].y).abs() < SCALED_EPSILON)
        {
            polylines_out.pop();
        }
        i_intersection = None;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Boundary {
    vline: usize,
    low: usize,
    high: usize,
}

/// A part of the area that can be filled by consecutive vertical runs, strictly left to right.
#[derive(Clone, Debug)]
struct MonotonicRegion {
    left: Boundary,
    right: Boundary,
    /// Path length difference when starting at left.low (len1) or left.high (len2).
    len1: f64,
    len2: f64,
    /// When starting at left.low, end at right.high (and vice versa).
    flips: bool,
    left_neighbors: Vec<usize>,
    right_neighbors: Vec<usize>,
}

impl MonotonicRegion {
    fn length(&self, flipped: bool) -> f64 {
        if flipped { self.len2 } else { self.len1 }
    }
    fn left_intersection_point(&self, flipped: bool) -> usize {
        if flipped {
            self.left.high
        } else {
            self.left.low
        }
    }
    fn right_intersection_point(&self, flipped: bool) -> usize {
        if flipped == self.flips {
            self.right.low
        } else {
            self.right.high
        }
    }
}

fn vertical_run_bottom(il: &[SegmentIntersection], start: usize) -> usize {
    let mut it = start;
    loop {
        while il[it].ty != IType::InnerLow {
            it -= 1;
        }
        if il[it - 1].ty == IType::InnerHigh {
            it -= 1;
        } else {
            let down = il[it].vertical_down();
            if down == -1 || il[it].vertical_down_quality() != LinkQuality::Valid {
                break;
            }
            it = down as usize;
        }
    }
    it
}

fn vertical_run_top(il: &[SegmentIntersection], start: usize) -> usize {
    let mut it = start;
    loop {
        while il[it].ty != IType::InnerHigh {
            it += 1;
        }
        if il[it + 1].ty == IType::InnerLow {
            it += 1;
        } else {
            let up = il[it].vertical_up();
            if up == -1 || il[it].vertical_up_quality() != LinkQuality::Valid {
                break;
            }
            it = up as usize;
        }
    }
    it
}

fn overlap_bottom(
    start: usize,
    end: usize,
    this: &[SegmentIntersection],
    other: &[SegmentIntersection],
    side: Side,
) -> Option<usize> {
    let mut found = None;
    let mut it = start;
    loop {
        if this[it].is_inner() {
            let i = this[it].horizontal(side);
            if i != -1 {
                found = Some(i as usize);
                break;
            }
            if it == end {
                break;
            }
        }
        if this[it].ty != IType::InnerHigh || this[it + 1].ty == IType::InnerLow {
            it += 1;
        } else {
            let up = this[it].vertical_up();
            if up == -1 || this[it].vertical_up_quality() != LinkQuality::Valid {
                break;
            }
            it = up as usize;
        }
    }
    found.map(|o| vertical_run_bottom(other, o))
}

fn overlap_top(
    start: usize,
    end: usize,
    this: &[SegmentIntersection],
    other: &[SegmentIntersection],
    side: Side,
) -> Option<usize> {
    let mut found = None;
    let mut it = end;
    loop {
        if this[it].is_inner() {
            let i = this[it].horizontal(side);
            if i != -1 {
                found = Some(i as usize);
                break;
            }
            if it == start {
                break;
            }
        }
        if this[it].ty != IType::InnerLow || this[it - 1].ty == IType::InnerHigh {
            it -= 1;
        } else {
            let down = this[it].vertical_down();
            if down == -1 || this[it].vertical_down_quality() != LinkQuality::Valid {
                break;
            }
            it = down as usize;
        }
    }
    found.map(|o| vertical_run_top(other, o))
}

/// The vertical run on a neighbouring line overlapping the run `start..=end`, as (bottom, top).
fn side_overlap(
    start: usize,
    end: usize,
    this: &[SegmentIntersection],
    other: &[SegmentIntersection],
    side: Side,
) -> Option<(usize, usize)> {
    let first = overlap_bottom(start, end, this, other, side)?;
    let second = overlap_top(start, end, this, other, side)?;
    Some((first, second))
}

fn generate_monotonic_regions(segs: &mut Segs) -> Vec<MonotonicRegion> {
    let mut regions = Vec::new();
    for i_vline_seed in 0..segs.len() {
        let mut i_seed = 1;
        while i_seed + 1 < segs[i_vline_seed].intersections.len() {
            let len = segs[i_vline_seed].intersections.len();
            while i_seed < len && segs[i_vline_seed].intersections[i_seed].ty != IType::InnerLow {
                i_seed += 1;
            }
            if i_seed == len {
                break;
            }
            let start = i_seed;
            let end = end_of_vertical_run(&segs[i_vline_seed].intersections, start);
            if !segs[i_vline_seed].intersections[start].consumed_vertical_up {
                // A new monotonic region, extended while there is a single right neighbour.
                let mut i_vline = i_vline_seed;
                let mut left = (start, end);
                let boundary = Boundary {
                    vline: i_vline,
                    low: start,
                    high: end,
                };
                let mut region = MonotonicRegion {
                    left: boundary,
                    right: boundary,
                    len1: 0.0,
                    len2: 0.0,
                    flips: false,
                    left_neighbors: Vec::new(),
                    right_neighbors: Vec::new(),
                };
                segs[i_vline_seed].intersections[start].consumed_vertical_up = true;
                let mut num_lines = 1;
                loop {
                    i_vline += 1;
                    if i_vline >= segs.len() {
                        break;
                    }
                    let vline_left = &segs[i_vline - 1].intersections;
                    let vline_right = &segs[i_vline].intersections;
                    let Some(right) =
                        side_overlap(left.0, left.1, vline_left, vline_right, Side::Right)
                    else {
                        break;
                    };
                    if vertical_run_top(vline_right, right.0) != right.1 {
                        // Overlaps with multiple segments at its right side.
                        break;
                    }
                    if side_overlap(right.0, right.1, vline_right, vline_left, Side::Left)
                        != Some(left)
                    {
                        // The right neighbour overlaps with multiple segments at its left.
                        break;
                    }
                    region.right = Boundary {
                        vline: i_vline,
                        low: right.0,
                        high: right.1,
                    };
                    segs[i_vline].intersections[right.0].consumed_vertical_up = true;
                    num_lines += 1;
                    left = right;
                }
                // An odd number of lines exits the zig-zag on the other side than it starts.
                region.flips = num_lines & 1 != 0;
                regions.push(region);
            }
            i_seed = end + 1;
        }
    }
    regions
}

/// Length of the connecting moves of a region's zig-zag, for the path optimisation.
fn monotonic_region_path_length(
    region: &MonotonicRegion,
    dir: bool,
    pwo: &ExPolygonWithOffset,
    segs: &Segs,
) -> f64 {
    let mut i_intersection = region.left_intersection_point(dir);
    let mut i_vline = region.left.vline;
    let mut total = 0.0_f32;
    let mut no_perimeter = false;
    let mut last_point = (0.0, 0.0);
    loop {
        let vline = &segs[i_vline];
        let v = &vline.intersections;
        let mut it = i_intersection;
        let going_up = v[it].is_low();
        if no_perimeter {
            let y = v[if going_up { it - 1 } else { it + 1 }].pos() as f32;
            total +=
                ((last_point.0 - vline.pos as f32).powi(2) + (last_point.1 - y).powi(2)).sqrt();
        }
        let mut iright = v[it].right_horizontal();
        if going_up {
            loop {
                loop {
                    it += 1;
                    iright = iright.max(v[it].right_horizontal());
                    if v[it].ty == IType::InnerHigh && v[it + 1].ty == IType::OuterHigh {
                        break;
                    }
                }
                let inext = v[it].vertical_up();
                if inext == -1 || v[it].vertical_up_quality() != LinkQuality::Valid {
                    break;
                }
                it = inext as usize;
            }
        } else {
            loop {
                loop {
                    it -= 1;
                    let r = v[it].right_horizontal();
                    if r != -1 {
                        iright = r;
                    }
                    if v[it].ty == IType::InnerLow && v[it - 1].ty == IType::OuterLow {
                        break;
                    }
                }
                let inext = v[it].vertical_down();
                if inext == -1 || v[it].vertical_down_quality() != LinkQuality::Valid {
                    break;
                }
                it = inext as usize;
            }
        }
        if i_vline == region.right.vline || iright == -1 {
            break;
        }
        let inext = v[it].right_horizontal();
        let vline_right = &segs[i_vline + 1].intersections;
        let right = if going_up {
            vertical_run_top(vline_right, iright as usize)
        } else {
            vertical_run_bottom(vline_right, iright as usize)
        };
        i_intersection = right;
        if inext == right as i32 && v[it].next_on_contour_quality == LinkQuality::Valid {
            // Half the length of the connection along the perimeter.
            total += 0.5
                * measure_perimeter_horizontal_segment_length(
                    pwo,
                    segs,
                    i_vline,
                    it,
                    inext as usize,
                ) as f32;
            no_perimeter = false;
        } else {
            let k = if going_up { it + 1 } else { it - 1 };
            last_point = (vline.pos as f32, v[k].pos() as f32);
            no_perimeter = true;
        }
        i_vline += 1;
    }
    f64::from((f64::from(total) / SCALE) as f32)
}

fn connect_monotonic_regions(
    regions: &mut [MonotonicRegion],
    pwo: &ExPolygonWithOffset,
    segs: &Segs,
) {
    let mut by_start: HashMap<(usize, usize), usize> = HashMap::new();
    let mut by_end: HashMap<(usize, usize), usize> = HashMap::new();
    for (i, r) in regions.iter().enumerate() {
        by_start.insert((r.left.vline, r.left.low), i);
        by_end.insert((r.right.vline, r.right.low), i);
    }

    let mut left_links = Vec::new();
    let mut right_links = Vec::new();
    for (ri, region) in regions.iter().enumerate() {
        if region.left.vline > 0 {
            let vline = &segs[region.left.vline].intersections;
            let vline_left = &segs[region.left.vline - 1].intersections;
            if let Some((mut lbegin, lend)) = side_overlap(
                region.left.low,
                region.left.high,
                vline,
                vline_left,
                Side::Left,
            ) {
                loop {
                    if let Some(&other) = by_end.get(&(region.left.vline - 1, lbegin)) {
                        // `other` is left of this region.
                        right_links.push((other, ri));
                    }
                    let mut lnext = vertical_run_top(vline_left, lbegin);
                    if lnext == lend {
                        break;
                    }
                    while vline_left[lnext].ty != IType::InnerLow {
                        lnext += 1;
                    }
                    lbegin = lnext;
                }
            }
        }
        if region.right.vline + 1 < segs.len() {
            let vline = &segs[region.right.vline].intersections;
            let vline_right = &segs[region.right.vline + 1].intersections;
            if let Some((mut rbegin, rend)) = side_overlap(
                region.right.low,
                region.right.high,
                vline,
                vline_right,
                Side::Right,
            ) {
                loop {
                    if let Some(&other) = by_start.get(&(region.right.vline + 1, rbegin)) {
                        left_links.push((other, ri));
                    }
                    let mut rnext = vertical_run_top(vline_right, rbegin);
                    if rnext == rend {
                        break;
                    }
                    while vline_right[rnext].ty != IType::InnerLow {
                        rnext += 1;
                    }
                    rbegin = rnext;
                }
            }
        }
    }
    // Symmetric closure of the neighbour relation.
    for (a, b) in right_links {
        regions[a].right_neighbors.push(b);
        regions[b].left_neighbors.push(a);
    }
    for (a, b) in left_links {
        regions[a].left_neighbors.push(b);
        regions[b].right_neighbors.push(a);
    }
    for r in regions.iter_mut() {
        r.left_neighbors.sort_unstable();
        r.left_neighbors.dedup();
        r.right_neighbors.sort_unstable();
        r.right_neighbors.dedup();
    }

    for r in regions.iter_mut() {
        let len1 = f64::from(monotonic_region_path_length(r, false, pwo, segs) as f32);
        let len2 = f64::from(monotonic_region_path_length(r, true, pwo, segs) as f32);
        // Keep only the positive difference of the two.
        if len1 > len2 {
            r.len1 = len1 - len2;
            r.len2 = 0.0;
        } else {
            r.len2 = len2 - len1;
            r.len1 = 0.0;
        }
    }
}

/// The Mersenne Twister `std::mt19937_64` with its default seed, so the ant colony is deterministic.
struct Mt19937_64 {
    mt: [u64; 312],
    idx: usize,
}

impl Mt19937_64 {
    fn new() -> Self {
        let mut mt = [0u64; 312];
        mt[0] = 5489;
        for i in 1..312 {
            mt[i] = 6_364_136_223_846_793_005u64
                .wrapping_mul(mt[i - 1] ^ (mt[i - 1] >> 62))
                .wrapping_add(i as u64);
        }
        Self { mt, idx: 312 }
    }

    fn next(&mut self) -> u64 {
        if self.idx >= 312 {
            for i in 0..312 {
                let x =
                    (self.mt[i] & 0xFFFF_FFFF_8000_0000) | (self.mt[(i + 1) % 312] & 0x7FFF_FFFF);
                let mut xa = x >> 1;
                if x & 1 != 0 {
                    xa ^= 0xB502_6F5A_A966_19E9;
                }
                self.mt[i] = self.mt[(i + 156) % 312] ^ xa;
            }
            self.idx = 0;
        }
        let mut y = self.mt[self.idx];
        self.idx += 1;
        y ^= (y >> 29) & 0x5555_5555_5555_5555;
        y ^= (y << 17) & 0x71D6_7FFF_EDA6_0000;
        y ^= (y << 37) & 0xFFF7_EEE0_0000_0000;
        y ^= y >> 43;
        y
    }

    fn unit(&mut self) -> f64 {
        self.next() as f64 / u64::MAX as f64
    }

    // libc++ uniform_int_distribution: a singleton consumes no random value;
    // otherwise draw the necessary low bits and reject values outside the range.
    fn index(&mut self, count: usize) -> usize {
        if count == 1 {
            return 0;
        }
        let mask = u64::MAX >> (count - 1).leading_zeros();
        loop {
            let value = (self.next() & mask) as usize;
            if value < count {
                return value;
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct AntPath {
    /// Length of the link to the next region, mm.
    length: f64,
    visibility: f64,
    pheromone: f64,
}

/// Lazily evaluated links between region ends (upstream's `AntPathMatrix`).
struct AntPathMatrix<'a> {
    regions: &'a [MonotonicRegion],
    segs: &'a Segs,
    paths: HashMap<usize, AntPath>,
    initial_pheromone: f64,
}

impl<'a> AntPathMatrix<'a> {
    fn key(&self, from: usize, flipped_from: bool, to: usize, flipped_to: bool) -> usize {
        let row = 2 * from + flipped_from as usize;
        let col = 2 * to + flipped_to as usize;
        row * self.regions.len() * 2 + col
    }

    /// The link from the end of `from` to the start of `to`; returns its key.
    fn link(&mut self, from: usize, flipped_from: bool, to: usize, flipped_to: bool) -> usize {
        let key = self.key(from, flipped_from, to, flipped_to);
        if !self.paths.contains_key(&key) {
            let rf = &self.regions[from];
            let rt = &self.regions[to];
            let i_from = rf.right_intersection_point(flipped_from);
            let i_to = rt.left_intersection_point(flipped_to);
            let vf = &self.segs[rf.right.vline];
            let vt = &self.segs[rt.left.vline];
            // Upstream compares region_from.right.vline + 1 with region_from.left.vline here, which never
            // holds, so the length along the contour is never used: links are straight distances.
            let dx = (vt.pos - vf.pos) as f64;
            let dy = (vt.intersections[i_to].pos() - vf.intersections[i_from].pos()) as f64;
            let length = (dx * dx + dy * dy).sqrt() / SCALE;
            self.paths.insert(
                key,
                AntPath {
                    length,
                    visibility: 1.0 / (length + EPSILON),
                    pheromone: self.initial_pheromone,
                },
            );
        }
        key
    }

    fn get(&self, key: usize) -> AntPath {
        self.paths[&key]
    }

    fn get_mut(&mut self, key: usize) -> &mut AntPath {
        self.paths.get_mut(&key).unwrap()
    }

    fn update_initial_pheromone(&mut self, pheromone: f64) {
        self.initial_pheromone = pheromone;
        for p in self.paths.values_mut() {
            p.pheromone = pheromone;
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct MonotonicRegionLink {
    region: usize,
    flipped: bool,
    next: Option<usize>,
}

/// Finds an order of the monotonic regions (respecting left-to-right precedence) with short
/// connecting moves, using an "ant colony" optimisation.
fn chain_monotonic_regions(
    regions: &[MonotonicRegion],
    segs: &Segs,
    rng: &mut Mt19937_64,
) -> Vec<MonotonicRegionLink> {
    // Number of unprocessed left neighbours + self.
    let mut lnu_initial = vec![1i32; regions.len()];
    let mut queue_initial = Vec::new();
    for (i, r) in regions.iter().enumerate() {
        if r.left_neighbors.is_empty() {
            queue_initial.push(i);
        } else {
            lnu_initial[i] += r.left_neighbors.len() as i32;
        }
    }

    const NUM_ROUNDS: usize = 25;
    const NUM_ROUNDS_NO_CHANGE_EXIT: usize = 8;
    let num_ants = regions.len().min(10);
    const PHEROMONE_EVAPORATION: f64 = 0.1;
    const PHEROMONE_DIVERSIFICATION: f64 = 0.1;
    const PROBABILITY_TAKE_BEST: f64 = 0.9;

    let mut matrix = AntPathMatrix {
        regions,
        segs,
        paths: HashMap::new(),
        initial_pheromone: 0.5,
    };

    #[derive(Clone, Copy)]
    struct Candidate {
        region: usize,
        link: usize,
        probability: f64,
        dir: bool,
    }

    // A greedy path sets the initial pheromone level to 10% of its cost.
    let pheromone_initial_deposit;
    {
        let mut queue = queue_initial.clone();
        let mut lnu = lnu_initial.clone();
        let Some(first) = queue.pop() else {
            return Vec::new();
        };
        let mut path_end = (first, false);
        lnu[first] -= 1;
        let mut total_length = regions[first].length(false);
        while !queue.is_empty() || !regions[path_end.0].right_neighbors.is_empty() {
            let (region, dir) = path_end;
            let mut best: Option<Candidate> = None;
            let consider =
                |next: usize, matrix: &mut AntPathMatrix, best: &mut Option<Candidate>| {
                    for flip in [false, true] {
                        let link = matrix.link(region, dir, next, flip);
                        let vis = matrix.get(link).visibility;
                        if vis > best.map_or(0.0, |c| c.probability) {
                            *best = Some(Candidate {
                                region: next,
                                link,
                                probability: vis,
                                dir: flip,
                            });
                        }
                    }
                };
            for &next in &regions[region].right_neighbors {
                if lnu[next] == 2 {
                    consider(next, &mut matrix, &mut best);
                }
            }
            let from_queue = best.is_none();
            if from_queue {
                for &next in &queue {
                    consider(next, &mut matrix, &mut best);
                }
            }
            let Some(next) = best else { break };
            for &n in &regions[region].right_neighbors {
                lnu[n] -= 1;
                if lnu[n] == 1 && next.region != n {
                    queue.push(n);
                }
            }
            if from_queue && let Some(pos) = queue.iter().position(|&q| q == next.region) {
                queue.swap_remove(pos);
            }
            total_length += regions[next.region].length(next.dir) + matrix.get(next.link).length;
            path_end = (next.region, next.dir);
            lnu[next.region] = 0;
        }
        pheromone_initial_deposit = 0.1 / total_length;
        matrix.update_initial_pheromone(pheromone_initial_deposit);
    }

    let path_probability = |p: AntPath| p.pheromone * p.visibility * p.visibility;

    let mut path: Vec<MonotonicRegionLink> = Vec::with_capacity(regions.len());
    let mut best_path: Vec<MonotonicRegionLink> = Vec::with_capacity(regions.len());
    let mut best_path_length = f64::MAX;
    let mut candidates: Vec<Candidate> = Vec::new();
    let mut num_rounds_no_change = 0;
    'rounds: for _round in 0..NUM_ROUNDS {
        if num_rounds_no_change >= NUM_ROUNDS_NO_CHANGE_EXIT {
            break;
        }
        let mut improved = false;
        for _ant in 0..num_ants {
            path.clear();
            let mut queue = queue_initial.clone();
            let mut lnu = lnu_initial.clone();
            let first_idx = rng.index(queue.len());
            let flipped = rng.next() > u64::MAX / 2;
            path.push(MonotonicRegionLink {
                region: queue[first_idx],
                flipped,
                next: None,
            });
            queue.swap_remove(first_idx);
            lnu[path[0].region] -= 1;

            while !queue.is_empty()
                || !regions[path.last().unwrap().region]
                    .right_neighbors
                    .is_empty()
            {
                let last = *path.last().unwrap();
                let (region, dir) = (last.region, last.flipped);
                candidates.clear();
                let add = |next: usize, matrix: &mut AntPathMatrix, out: &mut Vec<Candidate>| {
                    let p1 = matrix.link(region, dir, next, false);
                    let p2 = matrix.link(region, dir, next, true);
                    // Upstream also evaluates the "flipped" variants of these links; they are never used.
                    out.push(Candidate {
                        region: next,
                        link: p1,
                        probability: path_probability(matrix.get(p1)),
                        dir: false,
                    });
                    out.push(Candidate {
                        region: next,
                        link: p2,
                        probability: path_probability(matrix.get(p2)),
                        dir: true,
                    });
                };
                for &next in &regions[region].right_neighbors {
                    lnu[next] -= 1;
                    if lnu[next] == 1 {
                        add(next, &mut matrix, &mut candidates);
                    }
                }
                let num_direct = candidates.len();
                if num_direct == 0 {
                    for &next in &queue {
                        add(next, &mut matrix, &mut candidates);
                    }
                }
                if candidates.is_empty() {
                    break;
                }
                let take = if rng.unit() < PROBABILITY_TAKE_BEST {
                    // The first candidate with the highest probability.
                    let mut best = 0;
                    for (i, c) in candidates.iter().enumerate() {
                        if c.probability > candidates[best].probability {
                            best = i;
                        }
                    }
                    best
                } else {
                    let total: f64 = candidates.iter().map(|c| c.probability).sum();
                    let mut threshold = rng.next() as f64 * total / u64::MAX as f64;
                    let mut take = candidates.len() - 1;
                    for (i, c) in candidates.iter().enumerate() {
                        threshold -= c.probability;
                        if threshold <= 0.0 {
                            take = i;
                            break;
                        }
                    }
                    take
                };
                let chosen = candidates[take];
                // Move the other right neighbours with satisfied constraints to the queue.
                for c in &candidates[..num_direct] {
                    if queue.last() != Some(&c.region) && c.region != chosen.region {
                        queue.push(c.region);
                    }
                }
                if take >= num_direct
                    && let Some(pos) = queue.iter().position(|&q| q == chosen.region)
                {
                    queue.swap_remove(pos);
                }
                path.last_mut().unwrap().next = Some(chosen.link);
                path.push(MonotonicRegionLink {
                    region: chosen.region,
                    flipped: chosen.dir,
                    next: None,
                });
                lnu[chosen.region] = 0;
                // Lower the pheromone on the taken link to diversify the paths of this batch.
                let p = matrix.get_mut(chosen.link);
                p.pheromone = (1.0 - PHEROMONE_DIVERSIFICATION) * p.pheromone
                    + PHEROMONE_DIVERSIFICATION * pheromone_initial_deposit;
            }

            let last = path.last().unwrap();
            let mut len = regions[last.region].length(last.flipped);
            for w in path.windows(2) {
                let (r, next) = (w[0], w[1]);
                let link = matrix.link(r.region, r.flipped, next.region, next.flipped);
                len += regions[r.region].length(r.flipped) + matrix.get(link).length;
            }
            if len < best_path_length {
                best_path_length = len;
                std::mem::swap(&mut best_path, &mut path);
                if len == 0.0 {
                    break 'rounds;
                }
                improved = true;
            }
        }

        // Reinforce the pheromones (upstream walks `path`, which after an improvement holds the
        // previous best path).
        let total_cost = best_path_length + EPSILON;
        for link in path.iter().take(path.len().saturating_sub(1)) {
            if let Some(k) = link.next {
                let p = matrix.get_mut(k);
                p.pheromone = (1.0 - PHEROMONE_EVAPORATION) * p.pheromone
                    + PHEROMONE_EVAPORATION / total_cost;
            }
        }
        if improved {
            num_rounds_no_change = 0;
        } else {
            num_rounds_no_change += 1;
        }
    }
    best_path
}

/// Emits the zig-zag polylines of the chained monotonic regions.
fn polylines_from_paths(
    path: &[MonotonicRegionLink],
    regions: &[MonotonicRegion],
    pwo: &ExPolygonWithOffset,
    segs: &Segs,
    out: &mut Vec<Polyline>,
) {
    let mut open = false;
    let finish = |out: &mut Vec<Polyline>| {
        let n = out.len();
        let pl = &mut out[n - 1];
        remove_duplicate_points(pl);
        let close = |a: Point, b: Point| {
            (a.x - b.x).abs() < SCALED_EPSILON && (a.y - b.y).abs() < SCALED_EPSILON
        };
        if pl.len() <= 1 || (pl.len() == 2 && close(pl[0], pl[1])) {
            out.pop();
        } else if n >= 2 && close(out[n - 1][0], *out[n - 2].last().unwrap()) {
            // Merge with the previous polyline: an extrusion may have been split by phony outer points.
            let pl = out.pop().unwrap();
            let prev = out.last_mut().unwrap();
            let a = *prev.last().unwrap();
            *prev.last_mut().unwrap() = Point::new((a.x + pl[0].x) / 2, (a.y + pl[0].y) / 2);
            prev.extend_from_slice(&pl[1..]);
        }
    };

    for (k, seg) in path.iter().enumerate() {
        let region = &regions[seg.region];
        let dir = seg.flipped;
        let mut i_intersection = region.left_intersection_point(dir);
        let mut i_vline = region.left.vline;

        if open && k > 0 {
            // Connect the previous region with this one.
            let prev = path[k - 1];
            let region_prev = &regions[prev.region];
            let i_vline_prev = region_prev.right.vline;
            let vline_prev = &segs[i_vline_prev];
            let i_prev = region_prev.right_intersection_point(prev.flipped);
            let ip_prev = vline_prev.intersections[i_prev];
            let mut extended = false;
            if i_vline_prev + 1 == i_vline
                && ip_prev.right_horizontal() == i_intersection as i32
                && ip_prev.next_on_contour_quality == LinkQuality::Valid
            {
                let pl = out.last_mut().unwrap();
                emit_perimeter_prev_next_segment(
                    pwo,
                    segs,
                    i_vline_prev,
                    ip_prev.i_contour,
                    i_prev,
                    i_intersection,
                    pl,
                    true,
                );
                extended = true;
            }
            if !extended {
                // Finish the current vertical line at the outer contour.
                let k = if ip_prev.is_low() {
                    i_prev - 1
                } else {
                    i_prev + 1
                };
                *out.last_mut().unwrap().last_mut().unwrap() =
                    Point::new(vline_prev.pos, vline_prev.intersections[k].pos());
                finish(out);
                open = false;
            }
        }

        loop {
            let vline = &segs[i_vline];
            let v = &vline.intersections;
            let mut it = i_intersection;
            let going_up = v[it].is_low();
            if !open {
                // Extend the infill line up to the outer contour.
                let k = if going_up { it - 1 } else { it + 1 };
                out.push(vec![Point::new(vline.pos, v[k].pos())]);
                open = true;
            } else {
                out.last_mut()
                    .unwrap()
                    .push(Point::new(vline.pos, v[it].pos()));
            }
            let mut iright = v[it].right_horizontal();
            if going_up {
                loop {
                    loop {
                        it += 1;
                        iright = iright.max(v[it].right_horizontal());
                        if v[it].ty == IType::InnerHigh && v[it + 1].ty == IType::OuterHigh {
                            break;
                        }
                    }
                    out.last_mut()
                        .unwrap()
                        .push(Point::new(vline.pos, v[it].pos()));
                    let inext = v[it].vertical_up();
                    if inext == -1 || v[it].vertical_up_quality() != LinkQuality::Valid {
                        break;
                    }
                    emit_perimeter_segment_on_vertical_line(
                        pwo,
                        segs,
                        i_vline,
                        v[it].i_contour,
                        it,
                        inext as usize,
                        out.last_mut().unwrap(),
                        v[it].has_left_vertical_up(),
                    );
                    it = inext as usize;
                }
            } else {
                loop {
                    loop {
                        it -= 1;
                        let r = v[it].right_horizontal();
                        if r != -1 {
                            iright = r;
                        }
                        if v[it].ty == IType::InnerLow && v[it - 1].ty == IType::OuterLow {
                            break;
                        }
                    }
                    out.last_mut()
                        .unwrap()
                        .push(Point::new(vline.pos, v[it].pos()));
                    let inext = v[it].vertical_down();
                    if inext == -1 || v[it].vertical_down_quality() != LinkQuality::Valid {
                        break;
                    }
                    emit_perimeter_segment_on_vertical_line(
                        pwo,
                        segs,
                        i_vline,
                        v[it].i_contour,
                        it,
                        inext as usize,
                        out.last_mut().unwrap(),
                        v[it].has_right_vertical_down(),
                    );
                    it = inext as usize;
                }
            }

            if i_vline == region.right.vline || iright == -1 {
                break;
            }
            let inext = v[it].right_horizontal();
            let vline_right = &segs[i_vline + 1].intersections;
            let right = if going_up {
                vertical_run_top(vline_right, iright as usize)
            } else {
                vertical_run_bottom(vline_right, iright as usize)
            };
            i_intersection = right;
            if inext == right as i32 && v[it].next_on_contour_quality == LinkQuality::Valid {
                emit_perimeter_prev_next_segment(
                    pwo,
                    segs,
                    i_vline,
                    v[it].i_contour,
                    it,
                    inext as usize,
                    out.last_mut().unwrap(),
                    true,
                );
            } else {
                let k = if going_up { it + 1 } else { it - 1 };
                *out.last_mut().unwrap().last_mut().unwrap() = Point::new(vline.pos, v[k].pos());
                finish(out);
                open = false;
            }
            i_vline += 1;
        }
    }

    if open && let Some(last) = path.last() {
        let region = &regions[last.region];
        let vline = &segs[region.right.vline];
        let ip = region.right_intersection_point(last.flipped);
        let k = if vline.intersections[ip].is_low() {
            ip - 1
        } else {
            ip + 1
        };
        *out.last_mut().unwrap().last_mut().unwrap() =
            Point::new(vline.pos, vline.intersections[k].pos());
        finish(out);
    }
}

/// `FillRectilinear::fill_surface_by_lines`: fills one ExPolygon with lines in one direction.
/// Returns the polylines and the (possibly adjusted) spacing in mm.
pub(crate) fn fill_surface_by_lines(
    expolygon: &ExPolygon,
    params: &LineFillParams,
) -> (Vec<Polyline>, f64) {
    let mut spacing = params.spacing;
    let mut out = Vec::new();
    let angle = params.angle;
    let mut line_spacing = (scale_f(spacing) / params.density) as Coord;
    let pwo = ExPolygonWithOffset::new(
        expolygon,
        -angle,
        scale_f(-(0.5 - INFILL_OVERLAP_OVER_SPACING) * spacing) as f32 as Coord,
        scale_f(-0.5 * spacing) as f32 as Coord,
    );
    if pwo.n_contours_inner == 0 || line_spacing <= 0 {
        // Not a single infill line fits.
        return (out, spacing);
    }
    let bb_src = pwo.bounding_box_src();
    let mut bb = match params.object_box {
        Some(b) if params.consistent => b,
        _ => bb_src,
    };
    if params.full_infill() && !params.dont_adjust {
        line_spacing = adjust_solid_spacing(bb_src.max.x - bb_src.min.x, line_spacing);
        spacing = line_spacing as f64 / SCALE;
    } else {
        // Align the pattern with the other layers: the reference point is the object centre (origin).
        let aligned = align_point_to_grid(
            bb.min,
            Point::new(line_spacing, line_spacing),
            Point::default(),
        );
        merge_point(&mut bb, aligned);
    }
    let mut n_vlines = ((bb.max.x - bb.min.x + line_spacing - 1) / line_spacing).max(0) as usize;
    let mut x0 = bb.min.x;
    if params.full_infill() {
        x0 += (line_spacing + SCALED_EPSILON) / 2;
    }
    if let Some(horiz_move) = params.horiz_move {
        // Orca: shift by whole pairs of lines plus the remainder, so the zig-zag keeps its phase.
        let gap_line = (horiz_move / line_spacing as f32) as i64;
        let back = if gap_line % 2 == 0 {
            gap_line
        } else {
            n_vlines += 1;
            gap_line - 1
        };
        x0 = (x0 as f32 + (horiz_move - (back * line_spacing) as f32)) as Coord;
    }
    let Some(mut segs) = slice_region_by_vertical_lines(&pwo, n_vlines, x0, line_spacing) else {
        return (out, spacing);
    };
    connect_segment_intersections_by_contours(
        &pwo,
        &mut segs,
        params.dont_connect(),
        params.link_max_length,
    );
    if params.monotonic {
        pinch_contours_insert_phony_outer_intersections(&mut segs);
        let mut regions = generate_monotonic_regions(&mut segs);
        connect_monotonic_regions(&mut regions, &pwo, &segs);
        if !regions.is_empty() {
            let mut rng = Mt19937_64::new();
            let path = chain_monotonic_regions(&regions, &segs, &mut rng);
            polylines_from_paths(&path, &regions, &pwo, &segs, &mut out);
        }
    } else {
        traverse_graph_generate_polylines(&pwo, &mut segs, params.consistent, &mut out);
    }
    // Rotate back.
    for pl in &mut out {
        rotate_points(pl, angle);
        remove_duplicate_points(pl);
    }
    (out, spacing)
}

fn scale_f(mm: f64) -> f64 {
    mm * SCALE
}

/// `make_fill_lines`: straight lines between the outer contour intersections of each vertical line,
/// skipping lines within `x_margin` of the bounding box sides, rotated back by `angle`.
fn make_fill_lines(
    pwo: &ExPolygonWithOffset,
    angle: f64,
    x_margin: Coord,
    line_spacing: Coord,
    pattern_shift: Coord,
    fill_lines: &mut Vec<Polyline>,
) -> Option<()> {
    let mut bb = pwo.bounding_box_src();
    let x_min = bb.min.x + x_margin;
    let x_max = bb.max.x - x_margin;
    let shift = pattern_shift % line_spacing;
    let refpt = Point::new(
        -(if shift >= 0 {
            shift
        } else {
            line_spacing + shift
        }),
        0,
    );
    let aligned = align_point_to_grid(bb.min, Point::new(line_spacing, line_spacing), refpt);
    merge_point(&mut bb, aligned);
    let n_vlines = ((bb.max.x - bb.min.x + line_spacing - 1) / line_spacing).max(0) as usize;
    let (sin, cos) = angle.sin_cos();
    for vline in slice_region_by_vertical_lines(pwo, n_vlines, bb.min.x, line_spacing)? {
        if vline.pos < x_min {
            continue;
        }
        if vline.pos > x_max {
            break;
        }
        for pair in vline.intersections.chunks(2) {
            if let [low, high] = pair
                && low.ty == IType::OuterLow
                && high.ty == IType::OuterHigh
            {
                let a = Point::new(vline.pos, low.pos());
                let b = Point::new(vline.pos, high.pos());
                fill_lines.push(if angle == 0.0 {
                    vec![a, b]
                } else {
                    vec![rotate_point(a, cos, sin), rotate_point(b, cos, sin)]
                });
            }
        }
    }
    Some(())
}

/// One sweep of a multi-line pattern: extra angle (radians) and shift (mm).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Sweep {
    pub angle_base: f64,
    pub pattern_shift: f64,
}

/// `FillRectilinear::fill_surface_by_multilines`: several sweeps of straight lines (grid, triangles,
/// stars, cubic), joined along the boundary by `connect_infill`.
pub(crate) fn fill_surface_by_multilines(
    expolygon: &ExPolygon,
    params: &LineFillParams,
    sweeps: &[Sweep],
) -> Vec<Polyline> {
    let mut params = *params;
    params.density /= sweeps.len() as f64;
    let base = ExPolygonWithOffset::new(
        expolygon,
        0.0,
        scale_f(-0.5 * params.spacing) as f32 as Coord,
        0,
    );
    if base.n_contours == 0 {
        return Vec::new();
    }
    let line_width = scale_f(params.spacing) as Coord;
    let line_spacing = (scale_f(params.spacing) / params.density) as Coord;
    if line_spacing <= 0 {
        return Vec::new();
    }
    let mut fill_lines = Vec::new();
    for sweep in sweeps {
        // Upstream accumulates sweep angles as float, then rotates with double trig. Keeping
        // exact f64 symmetry changes micron-rounded endpoints and the shortest-arc connection
        // order around holes (merging paths which upstream leaves separate).
        let angle = f64::from(params.angle as f32 + sweep.angle_base as f32);
        if make_fill_lines(
            &base.rotated(-angle),
            angle,
            line_width + SCALED_EPSILON,
            line_spacing,
            scale(sweep.pattern_shift),
            &mut fill_lines,
        )
        .is_none()
        {
            return Vec::new();
        }
    }
    if params.dont_connect() || fill_lines.len() <= 1 {
        chain_polylines(fill_lines)
    } else {
        let bbox = bbox_of(expolygon.contour.iter()).unwrap();
        connect_infill(
            fill_lines,
            &base.polygons_outer,
            bbox,
            params.spacing,
            params.anchor_length,
            params.anchor_length_max,
        )
    }
}

/// `FillSupportBase::fill_surface`: fixed-angle scanlines aligned to the support grid, joined by
/// `connect_base_support` (Prusa and Orca share it); unlike object infill there is no one-line
/// margin in X.
pub(crate) fn fill_support_base(ex: &ExPolygon, params: &LineFillParams) -> Vec<Polyline> {
    let base = ExPolygonWithOffset::new(
        ex,
        -params.angle,
        scale_f(-0.5 * params.spacing) as f32 as Coord,
        0,
    );
    if base.n_contours == 0 {
        return Vec::new();
    }
    let line_spacing = (scale_f(params.spacing) / params.density) as Coord;
    if line_spacing <= 0 {
        return Vec::new();
    }
    let mut lines = Vec::new();
    if make_fill_lines(&base, 0.0, 0, line_spacing, 0, &mut lines).is_none() {
        return Vec::new();
    }
    let bb = bbox_of(base.polygons_outer.iter().flat_map(|p| p.iter())).unwrap();
    let mut out = super::connect::connect_base_support(
        lines,
        &base.polygons_outer,
        bb,
        params.spacing,
        params.density,
    );
    let (sin, cos) = params.angle.sin_cos();
    for line in &mut out {
        for point in line {
            *point = rotate_point(*point, cos, sin);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::clip::union_ex_positive;

    fn square(size: f64) -> ExPolygon {
        union_ex_positive(&[vec![
            Point::from_mm(0.0, 0.0),
            Point::from_mm(size, 0.0),
            Point::from_mm(size, size),
            Point::from_mm(0.0, size),
        ]])
        .remove(0)
    }

    #[test]
    fn rational_positions_compare_exactly() {
        let mut a = SegmentIntersection::new(0, 0);
        a.pos_p = 1;
        a.pos_q = 3;
        let mut b = a;
        b.pos_p = 2;
        b.pos_q = 6;
        assert_eq!(a.cmp_pos(&b), std::cmp::Ordering::Equal);
        b.pos_p = 3;
        assert_eq!(a.cmp_pos(&b), std::cmp::Ordering::Less);
        b.pos_p = -7;
        b.pos_q = 2;
        assert_eq!(b.pos(), -4);
    }

    #[test]
    fn mersenne_twister_matches_std() {
        // The 10000th output of a default-constructed std::mt19937_64 (C++ standard, [rand.predef]).
        let mut rng = Mt19937_64::new();
        let mut v = 0;
        for _ in 0..10000 {
            v = rng.next();
        }
        assert_eq!(v, 9_981_545_732_273_789_042);
    }

    #[test]
    fn square_slices_into_runs() {
        let pwo = ExPolygonWithOffset::new(&square(10.0), 0.0, -20_000, -200_000);
        assert_eq!(pwo.n_contours_outer, 1);
        assert_eq!(pwo.n_contours_inner, 1);
        let segs = slice_region_by_vertical_lines(&pwo, 10, 500_000, 1_000_000).unwrap();
        for s in &segs {
            let t: Vec<_> = s.intersections.iter().map(|i| i.ty).collect();
            assert_eq!(
                t,
                [
                    IType::OuterLow,
                    IType::InnerLow,
                    IType::InnerHigh,
                    IType::OuterHigh
                ]
            );
        }
    }
}
