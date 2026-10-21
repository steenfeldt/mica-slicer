// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: prusaslicer@b028299c77 src/libslic3r/PerimeterGenerator.cpp :: clip_extrusion
// Changes: Clipper2 Z callback; repair inherited zero-Z clip vertices even on two-point subjects; clip paths whose bounding box misses the line are left out (they cannot touch it), so a layer of many small extrusions is not clipped against the whole layer below each time; on textured meshes the clip paths are also cut to the line's neighbourhood.
use super::{ExtrusionLine, Junction};
use crate::geometry::{ExPolygon, Point};
use clipper2_rust::{ClipType, Clipper64, FillRule, Path64, Paths64, Point64};

/// Clip a width-bearing extrusion against material supported by the layer below. Width (spacing)
/// is carried in Clipper's Z coordinate and interpolated at every newly inserted intersection.
pub fn clip_extrusion(
    line: &ExtrusionLine,
    clip: &[ExPolygon],
    intersection: bool,
) -> Vec<ExtrusionLine> {
    clip_extrusion_with(line, &ClipArea::new(clip), intersection)
}

/// The clip polygons of a layer with their bounding boxes, built once for all its extrusions.
pub struct ClipArea {
    paths: Vec<([i64; 4], Path64)>,
    /// Cut the clip paths to each line's neighbourhood (rectangle clipping) before clipping:
    /// the same region near the line, for layers of thousands of small extrusions over a
    /// large, detailed layer below (textured meshes). Plain objects clip as upstream does.
    crop: bool,
}
impl ClipArea {
    pub fn new(clip: &[ExPolygon]) -> Self {
        Self {
            paths: clip
                .iter()
                .flat_map(|e| e.polygons())
                .map(|p| {
                    let path: Path64 = p.iter().map(|p| Point64::new(p.x, p.y)).collect();
                    (bounds(path.iter().map(|q| (q.x, q.y))), path)
                })
                .collect(),
            crop: false,
        }
    }

    /// [`Self::new`], cutting the clip paths to each line's neighbourhood.
    pub fn cropped(clip: &[ExPolygon]) -> Self {
        Self {
            crop: true,
            ..Self::new(clip)
        }
    }
}

fn bounds(points: impl Iterator<Item = (i64, i64)>) -> [i64; 4] {
    points.fold([i64::MAX, i64::MAX, i64::MIN, i64::MIN], |b, (x, y)| {
        [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)]
    })
}

/// [`clip_extrusion`] against a prepared [`ClipArea`]. A path whose bounding box misses the
/// line's can neither cross it nor contain any of its points, so it is left out.
pub fn clip_extrusion_with(
    line: &ExtrusionLine,
    clip: &ClipArea,
    intersection: bool,
) -> Vec<ExtrusionLine> {
    let _timing = crate::timing::Span::new(crate::timing::Stage::WallClipping);
    if line.junctions.len() < 2 {
        return vec![];
    }
    let subject: Path64 = line
        .junctions
        .iter()
        .map(|j| Point64::new_z(j.p.x, j.p.y, j.width.round() as i64))
        .collect();
    let b = bounds(line.junctions.iter().map(|j| (j.p.x, j.p.y)));
    let near = clip
        .paths
        .iter()
        .filter(|(c, _)| c[0] <= b[2] && c[2] >= b[0] && c[1] <= b[3] && c[3] >= b[1]);
    let paths: Paths64 = if clip.crop {
        // A margin keeps the cut away from the line: the region near the line is unchanged.
        const MARGIN: i64 = 1000;
        let rect =
            clipper2_rust::Rect64::new(b[0] - MARGIN, b[1] - MARGIN, b[2] + MARGIN, b[3] + MARGIN);
        let mut out = Paths64::new();
        for (c, p) in near {
            if c[0] >= rect.left && c[2] <= rect.right && c[1] >= rect.top && c[3] <= rect.bottom {
                out.push(p.clone());
                continue;
            }
            let ccw = clipper2_rust::area(p) > 0.;
            for mut piece in clipper2_rust::rect_clip_path_64(&rect, p) {
                if (clipper2_rust::area(&piece) > 0.) != ccw {
                    piece.reverse();
                }
                out.push(piece);
            }
        }
        out
    } else {
        near.map(|(_, p)| p.clone()).collect()
    };
    let mut c = Clipper64::new();
    c.set_z_callback(|a, b, c, d, p| {
        let (start, end) = if a.z > 0 || b.z > 0 { (a, b) } else { (c, d) };
        if start.z <= 0 && end.z <= 0 {
            p.z = 0;
            return;
        }
        let length = ((end.x - start.x) as f64).hypot((end.y - start.y) as f64);
        let t = if length > 0. {
            ((p.x - start.x) as f64).hypot((p.y - start.y) as f64) / length
        } else {
            0.
        };
        p.z = start.z + ((end.z - start.z) as f64 * t.clamp(0., 1.)) as i64;
    });
    c.add_open_subject(&vec![subject]);
    c.add_clip(&paths);
    let mut closed = Paths64::new();
    let mut open = Paths64::new();
    c.execute(
        if intersection {
            ClipType::Intersection
        } else {
            ClipType::Difference
        },
        FillRule::NonZero,
        &mut closed,
        Some(&mut open),
    );
    open.into_iter()
        .map(|path| {
            let junctions = path
                .into_iter()
                .map(|p| {
                    let point = Point::new(p.x, p.y);
                    let width = if p.z > 0 {
                        p.z as f64
                    } else {
                        line.junctions
                            .windows(2)
                            .map(|w| {
                                let q = super::graph::project(point, w[0].p, w[1].p);
                                let length = w[0].p.distance_to(w[1].p);
                                let t = if length > 0. {
                                    w[0].p.distance_to(q) / length
                                } else {
                                    0.
                                };
                                (
                                    point.distance_sq(q),
                                    w[0].width + t * (w[1].width - w[0].width),
                                )
                            })
                            .min_by_key(|v| v.0)
                            .unwrap()
                            .1
                    };
                    Junction {
                        p: point,
                        width,
                        perimeter: line.inset,
                    }
                })
                .collect();
            ExtrusionLine {
                junctions,
                is_closed: false,
                ..line.clone()
            }
        })
        .collect()
}
