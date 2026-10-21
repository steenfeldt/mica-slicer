// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/PrintObject.cpp :: PrintObject::PrintObject
// Source: prusaslicer@b028299c77 src/libslic3r/PerimeterGenerator.cpp :: process_arachne
// Changes: Generate Arachne walls in object coordinates, then restore placed coordinates; classic retains its existing frame.
// Changes: Reuse transformed neighbours and their support offset across a layer wall group; on textured meshes each island sees only the neighbours near it.

use crate::{
    geometry::{ExPolygon, Point},
    perimeters::{IslandWalls, WallParams},
    settings::PrintSettings,
};

fn translate(ex: &mut ExPolygon, delta: Point) {
    for point in ex.contour.iter_mut().chain(ex.holes.iter_mut().flatten()) {
        point.x += delta.x;
        point.y += delta.y;
    }
}

struct LocalPainting<'a> {
    inner: &'a dyn crate::fuzzy_skin::FuzzySkinPainting,
    origin: Point,
}
impl crate::fuzzy_skin::FuzzySkinPainting for LocalPainting<'_> {
    fn needs_sampling(&self) -> bool {
        self.inner.needs_sampling()
    }
    fn settings_at(&self, p: Point, layer: usize) -> Option<crate::fuzzy_skin::FuzzySkinSettings> {
        self.inner
            .settings_at(Point::new(p.x + self.origin.x, p.y + self.origin.y), layer)
    }
    fn texture_skin(&self) -> Option<crate::texture_skin::Surface<'_>> {
        self.inner
            .texture_skin()
            .map(|t| crate::texture_skin::Surface {
                offset: Point::new(t.offset.x + self.origin.x, t.offset.y + self.origin.y),
                ..t
            })
    }
}

/// A layer's local neighbours and support offset are invariant across its islands.
/// Keep them only for the current wall group, rather than cloning and offsetting
/// the entire neighbouring layer again for each small painted island.
pub(crate) struct LayerFrame<'a> {
    origin: Point,
    neighbors: [Option<std::borrow::Cow<'a, [ExPolygon]>>; 2],
    supported: Option<Vec<ExPolygon>>,
    local: bool,
    /// Textured meshes: the bounding box of every neighbour island, so each island's walls
    /// see only the neighbours near it (thousands of small islands above a textured top).
    boxes: Option<[Vec<Option<crate::geometry::BoundingBox>>; 2]>,
}
impl<'a> LayerFrame<'a> {
    pub fn new(
        neighbors: [Option<&'a [ExPolygon]>; 2],
        params: &WallParams,
        settings: &PrintSettings,
        origin: Point,
    ) -> Self {
        let local = settings.perimeter_generator == crate::settings::PerimeterGenerator::Arachne;
        let shift = Point::new(-origin.x, -origin.y);
        let neighbors = neighbors.map(|polygons| {
            polygons.map(|polygons| {
                if local {
                    std::borrow::Cow::Owned(
                        polygons
                            .iter()
                            .map(|ex| {
                                let mut ex = ex.clone();
                                translate(&mut ex, shift);
                                ex
                            })
                            .collect::<Vec<_>>(),
                    )
                } else {
                    std::borrow::Cow::Borrowed(polygons)
                }
            })
        });
        let supported = neighbors[0]
            .as_deref()
            .filter(|_| local && settings.overhangs && !settings.supports_touching)
            .map(|lower| {
                crate::geometry::clip::offset_ex(
                    lower,
                    params.nozzle_diameter * crate::geometry::SCALE / 2.,
                    crate::geometry::clip::Join::Miter,
                )
            });
        let boxes = (local && settings.textured_mesh).then(|| {
            std::array::from_fn(|k| {
                neighbors[k].as_deref().map_or_else(Vec::new, |n| {
                    n.iter()
                        .map(|e| crate::geometry::BoundingBox::from_points(e.contour.iter()))
                        .collect()
                })
            })
        });
        Self {
            origin,
            neighbors,
            supported,
            local,
            boxes,
        }
    }

    /// The neighbour islands whose bounding box comes within `margin` of `bb` (all of them
    /// unless the layer keeps boxes).
    fn near(
        &self,
        k: usize,
        bb: Option<crate::geometry::BoundingBox>,
        margin: i64,
    ) -> Option<std::borrow::Cow<'_, [ExPolygon]>> {
        let all = self.neighbors[k].as_deref()?;
        let (Some(boxes), Some(bb)) = (&self.boxes, bb) else {
            return Some(std::borrow::Cow::Borrowed(all));
        };
        Some(std::borrow::Cow::Owned(
            all.iter()
                .zip(&boxes[k])
                .filter(|(_, b)| {
                    b.is_some_and(|b| {
                        b.min.x <= bb.max.x + margin
                            && b.max.x >= bb.min.x - margin
                            && b.min.y <= bb.max.y + margin
                            && b.max.y >= bb.min.y - margin
                    })
                })
                .map(|(e, _)| e.clone())
                .collect(),
        ))
    }

    pub fn walls(
        &self,
        island: &ExPolygon,
        params: &WallParams,
        settings: &PrintSettings,
        index: usize,
        painting: &dyn crate::fuzzy_skin::FuzzySkinPainting,
    ) -> IslandWalls {
        if !self.local {
            return crate::perimeters::make_walls_selected_with_painting(
                island,
                params,
                settings,
                self.neighbors[0].as_deref(),
                self.neighbors[1].as_deref(),
                index,
                painting,
            );
        }
        let origin = self.origin;
        let mut local = island.clone();
        translate(&mut local, Point::new(-origin.x, -origin.y));
        // Only the neighbours near this island change its walls (a millimetre around it covers
        // every offset the walls take of them).
        let bb = self
            .boxes
            .as_ref()
            .and_then(|_| crate::geometry::BoundingBox::from_points(local.contour.iter()));
        let margin = crate::geometry::scale(1.0);
        let (lower, upper) = (self.near(0, bb, margin), self.near(1, bb, margin));
        let mut result = crate::arachne::make_walls_with_neighbors(
            &local,
            params,
            settings,
            crate::arachne::WallNeighbors {
                lower: lower.as_deref(),
                upper: upper.as_deref(),
                supported: self.supported.as_deref(),
            },
            index,
            &LocalPainting {
                inner: painting,
                origin,
            },
        );
        for wall in &mut result.loops {
            for p in &mut wall.polygon {
                p.x += origin.x;
                p.y += origin.y;
            }
        }
        for path in result
            .variable_walls
            .iter_mut()
            .chain(&mut result.thin_walls)
            .chain(&mut result.gap_fill)
            .flatten()
        {
            for p in &mut path.points {
                p.x += origin.x;
                p.y += origin.y;
            }
        }
        for ex in result
            .infill_area
            .iter_mut()
            .chain(&mut result.infill_no_overlap)
        {
            translate(ex, origin);
        }
        result
    }
}
