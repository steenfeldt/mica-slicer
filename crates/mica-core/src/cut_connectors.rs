// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: prusaslicer@b028299c77 src/libslic3r/Model.hpp :: CutConnectorType, CutConnectorStyle, CutConnectorShape, CutConnectorAttributes, CutConnector
// Source: prusaslicer@b028299c77 src/libslic3r/TriangleMesh.cpp :: its_make_cylinder, its_make_frustum, its_make_snap
// Source: prusaslicer@b028299c77 src/libslic3r/CutUtils.cpp :: apply_tolerance, process_connector_cut
// Source: prusaslicer@b028299c77 src/slic3r/GUI/Gizmos/GLGizmoCut.cpp :: get_connector_mesh, apply_cut_connectors, apply_connectors_in_model, is_outside_of_cut_contour, is_conflict_for_connector
// Changes: Connector geometry only, as data for the GUI's cut tool: the unit meshes are centred on their bounding box (upstream's add_volume centring) and placed by a scale and an offset along the plane normal instead of a ModelVolume transformation; a dowel is always a prism (upstream's UI resets the style for dowels, so its_make_frustum_dowel is not ported); the contour test samples the outline (rotated by the connector's angle) against the plane section of the mesh (crossing segments, parity test) instead of the object clipper's contours; the snap's side steps are counted (2 × sectors) instead of compared with is_approx; the bounding-box test checks the connector's far end against the object box. Snap bulge and space proportions keep upstream's defaults. "Place automatically" (`Section::auto_place`, `spread`) is original Mica code.
// Upstream attribution (Model.hpp, TriangleMesh.cpp):
// Copyright (c) Prusa Research 2016 - 2023 Tomáš Mészáros @tamasmeszaros, Oleksandra Iushchenko @YuSanka, David Kocík @kocikdav, Enrico Turri @enricoturri1966, Lukáš Matěna @lukasmatena, Vojtěch Bubník @bubnikv, Filip Sykala @Jony01, Lukáš Hejl @hejllukas
// Copyright (c) Slic3r 2014 - 2016 Alessandro Ranellucci @alranel
// Upstream attribution (CutUtils.cpp, GLGizmoCut.cpp):
// Copyright (c) Prusa Research 2022 - 2023 Oleksandra Iushchenko @YuSanka
//
// PrusaSlicer is released under the terms of the AGPLv3 or higher
//

//! Cut connectors: plugs, dowels and snaps placed on a cut plane so the two halves of a cut
//! object fit back together. A plug or snap becomes a solid part of the lower half and a
//! slightly larger hole (negative volume) in the upper half; a dowel becomes a hole in both
//! halves and a separate printed pin.

use crate::mesh::TriangleMesh;

/// What the connector is (upstream `CutConnectorType`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ConnectorType {
    /// A peg on the lower half that fits a hole in the upper half.
    #[default]
    Plug,
    /// A separate pin; both halves get a hole.
    Dowel,
    /// A split peg with a bulge that clicks into a plain hole.
    Snap,
}
impl ConnectorType {
    pub const ALL: [Self; 3] = [Self::Plug, Self::Dowel, Self::Snap];
    pub fn label(self) -> &'static str {
        match self {
            Self::Plug => "Plug",
            Self::Dowel => "Dowel",
            Self::Snap => "Snap",
        }
    }
}

/// Prism (straight sides) or frustum (tapering to half the radius); plugs only.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ConnectorStyle {
    #[default]
    Prism,
    Frustum,
}
impl ConnectorStyle {
    pub const ALL: [Self; 2] = [Self::Prism, Self::Frustum];
    pub fn label(self) -> &'static str {
        match self {
            Self::Prism => "Prism",
            Self::Frustum => "Frustum",
        }
    }
}

/// The cross-section (upstream `CutConnectorShape`); snaps are always round.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ConnectorShape {
    Triangle,
    Square,
    Hexagon,
    #[default]
    Circle,
}
impl ConnectorShape {
    pub const ALL: [Self; 4] = [Self::Triangle, Self::Square, Self::Hexagon, Self::Circle];
    pub fn label(self) -> &'static str {
        match self {
            Self::Triangle => "Triangle",
            Self::Square => "Square",
            Self::Hexagon => "Hexagon",
            Self::Circle => "Circle",
        }
    }
    /// Sides of the mesh (`get_connector_mesh`).
    pub fn sectors(self) -> usize {
        match self {
            Self::Triangle => 3,
            Self::Square => 4,
            Self::Hexagon => 6,
            Self::Circle => 360,
        }
    }
    /// Outline points for the contour test (60 for a circle, as upstream).
    fn outline_sectors(self) -> usize {
        match self {
            Self::Circle => 60,
            s => s.sectors(),
        }
    }
}

/// Snap geometry proportions, relative to the radius (upstream's defaults).
pub const SNAP_BULGE: f32 = 0.15;
pub const SNAP_SPACE: f32 = 0.3;
/// Upstream's small shift that keeps a hole's cap off the cut face ("better preview").
pub const PREVIEW_SHIFT: f32 = 0.05;

/// The kind and shape of a connector.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Attributes {
    pub kind: ConnectorType,
    pub style: ConnectorStyle,
    pub shape: ConnectorShape,
}
impl Attributes {
    /// The attributes as they are built: dowels are prisms, snaps round prisms.
    pub fn effective(self) -> Self {
        match self.kind {
            ConnectorType::Plug => self,
            ConnectorType::Dowel => Self {
                style: ConnectorStyle::Prism,
                ..self
            },
            ConnectorType::Snap => Self {
                style: ConnectorStyle::Prism,
                shape: ConnectorShape::Circle,
                ..self
            },
        }
    }
}

/// Connector sizes, millimetres (upstream `CutConnector`; the UI's size is the diameter).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sizes {
    pub radius: f32,
    /// Depth into the other half (plug, snap) or into each half (dowel).
    pub height: f32,
    /// Added to the hole's radius.
    pub radius_tolerance: f32,
    /// Added to the hole's depth.
    pub height_tolerance: f32,
    /// Turn about the plane normal, radians.
    pub angle: f32,
}
impl Default for Sizes {
    /// Upstream's defaults: 2.5 mm across, 3 mm deep, 0.1 mm depth tolerance.
    fn default() -> Self {
        Self {
            radius: 1.25,
            height: 3.,
            radius_tolerance: 0.,
            height_tolerance: 0.1,
            angle: 0.,
        }
    }
}

/// A unit-sized connector body (radius 1, height 1) centred on its bounding box, axis +Z.
pub fn unit_mesh(a: Attributes) -> TriangleMesh {
    let a = a.effective();
    let step = std::f64::consts::TAU / a.shape.sectors() as f64;
    let mesh = match (a.kind, a.style) {
        (ConnectorType::Snap, _) => make_snap(1., 1., SNAP_SPACE, SNAP_BULGE),
        (_, ConnectorStyle::Prism) => make_cylinder(1., 1., step),
        (_, ConnectorStyle::Frustum) => make_frustum(1., 1., step),
    };
    centred(mesh)
}

/// The unit hole a connector cuts into the other half: the body itself, but a plain round
/// prism for a snap (upstream's `its_make_cylinder(1.0, 1.0, PI / 180.)`).
pub fn unit_hole_mesh(a: Attributes) -> TriangleMesh {
    let a = a.effective();
    if a.kind == ConnectorType::Snap {
        centred(make_cylinder(1., 1., std::f64::consts::PI / 180.))
    } else {
        unit_mesh(a)
    }
}

/// How a unit mesh is placed: scaled by `scale` (X, Y, Z of the connector frame) with its
/// centre `offset` mm along the plane normal from the connector's point on the plane.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fit {
    pub scale: [f32; 3],
    pub offset: f32,
}

/// The connector's body as placed before the cut (`apply_connectors_in_model`): a plug or
/// snap stands on the plane on the upper side; a dowel's prism is twice as tall and centred.
pub fn body(a: Attributes, s: &Sizes) -> Fit {
    if a.kind == ConnectorType::Dowel {
        Fit {
            scale: [s.radius, s.radius, 2. * s.height],
            offset: 0.,
        }
    } else {
        Fit {
            scale: [s.radius, s.radius, s.height],
            offset: 0.5 * s.height,
        }
    }
}

/// The hole: the body widened and deepened by the tolerances (`apply_tolerance`); plugs and
/// snaps shift back by the preview offset so the hole opens through the cut face.
pub fn hole(a: Attributes, s: &Sizes) -> Fit {
    let b = body(a, s);
    let mut offset = b.offset + 0.5 * s.height_tolerance;
    if a.kind != ConnectorType::Dowel {
        offset -= PREVIEW_SHIFT;
    }
    Fit {
        scale: [
            b.scale[0] + s.radius_tolerance,
            b.scale[1] + s.radius_tolerance,
            b.scale[2] + s.height_tolerance,
        ],
        offset,
    }
}

/// Why a connector cannot be cut (upstream `is_conflict_for_connector`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Conflict {
    /// Its outline leaves the cut face.
    OutsideContour,
    /// It reaches out of the object's bounding box.
    OutsideObject,
    /// It overlaps another connector.
    Overlap,
}
impl Conflict {
    pub fn message(self) -> &'static str {
        match self {
            Self::OutsideContour => "A connector is outside the cut face",
            Self::OutsideObject => "A connector reaches out of the object",
            Self::Overlap => "Connectors overlap",
        }
    }
}

/// One connector for the conflict test, in world coordinates.
pub struct Placed {
    pub point: [f32; 3],
    pub attributes: Attributes,
    pub sizes: Sizes,
}

/// The conflicts of each connector, in order (`check_and_update_connectors_state`).
/// `axes` are the plane's in-plane X and Y and its normal; `inside` says whether a point on
/// the plane lies within the cut face; `bounds` is the object's world box.
pub fn conflicts(
    connectors: &[Placed],
    axes: [[f32; 3]; 3],
    bounds: ([f32; 3], [f32; 3]),
    inside: impl Fn([f32; 3]) -> bool,
) -> Vec<Option<Conflict>> {
    let [ax, ay, n] = axes;
    let at = |p: [f32; 3], x: f32, y: f32, z: f32| {
        std::array::from_fn(|k| p[k] + ax[k] * x + ay[k] * y + n[k] * z)
    };
    connectors
        .iter()
        .enumerate()
        .map(|(i, c)| {
            if !inside(c.point) {
                return Some(Conflict::OutsideContour);
            }
            let shape = c.attributes.effective().shape;
            let sectors = shape.outline_sectors();
            for k in 0..sectors {
                // Upstream's outline starts on +Y and turns counter-clockwise; the mesh is
                // turned by -angle.
                let t = std::f32::consts::TAU * k as f32 / sectors as f32 - c.sizes.angle;
                let (sin, cos) = t.sin_cos();
                let p = at(c.point, -sin * c.sizes.radius, cos * c.sizes.radius, 0.);
                if !inside(p) {
                    return Some(Conflict::OutsideContour);
                }
            }
            let fit = body(c.attributes, &c.sizes);
            let half = fit.scale[2] * 0.5;
            let ends = [
                at(c.point, 0., 0., fit.offset + half),
                at(c.point, 0., 0., fit.offset - half),
            ];
            let (lo, hi) = bounds;
            let within = |p: [f32; 3]| (0..3).all(|k| p[k] >= lo[k] - 1e-3 && p[k] <= hi[k] + 1e-3);
            if !ends.iter().all(|&p| within(p)) {
                return Some(Conflict::OutsideObject);
            }
            let overlap = connectors.iter().enumerate().any(|(j, o)| {
                j != i && {
                    let d: f32 = (0..3).map(|k| (o.point[k] - c.point[k]).powi(2)).sum();
                    d.sqrt() < o.sizes.radius + c.sizes.radius
                }
            });
            overlap.then_some(Conflict::Overlap)
        })
        .collect()
}

/// The cut face of a mesh: the segments where its triangles cross the plane, in the plane's
/// own X / Y, for point-in-face tests (the role of upstream's object clipper contours).
pub struct Section {
    segments: Vec<[[f64; 2]; 2]>,
}
impl Section {
    /// `point` lies on the plane; `axes` are its X, Y and normal.
    pub fn new(mesh: &TriangleMesh, point: [f32; 3], axes: [[f32; 3]; 3]) -> Self {
        let dot = |a: [f64; 3], b: [f32; 3]| {
            a[0] * f64::from(b[0]) + a[1] * f64::from(b[1]) + a[2] * f64::from(b[2])
        };
        let local = |v: [f32; 3]| -> [f64; 3] {
            let d = std::array::from_fn(|k| f64::from(v[k]) - f64::from(point[k]));
            [dot(d, axes[0]), dot(d, axes[1]), dot(d, axes[2])]
        };
        let mut segments = vec![];
        for f in &mesh.faces {
            let p = f.map(|v| local(mesh.vertices[v as usize]));
            let mut ends = [[0.; 2]; 2];
            let mut n = 0;
            for k in 0..3 {
                let (a, b) = (p[k], p[(k + 1) % 3]);
                if (a[2] >= 0.) != (b[2] >= 0.) && n < 2 {
                    let t = a[2] / (a[2] - b[2]);
                    ends[n] = [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
                    n += 1;
                }
            }
            if n == 2 {
                segments.push(ends);
            }
        }
        Self { segments }
    }
    /// Whether the plane point (X, Y) lies inside the cut face (crossing parity).
    pub fn contains(&self, x: f32, y: f32) -> bool {
        let (x, y) = (f64::from(x), f64::from(y));
        let mut inside = false;
        for [a, b] in &self.segments {
            if (a[1] > y) != (b[1] > y) {
                let cx = a[0] + (y - a[1]) / (b[1] - a[1]) * (b[0] - a[0]);
                if cx > x {
                    inside = !inside;
                }
            }
        }
        inside
    }
    pub fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }
    /// Distance from the plane point (X, Y) to the nearest edge of the cut face.
    pub fn distance_to_outline(&self, x: f32, y: f32) -> f32 {
        self.outline_distance([f64::from(x), f64::from(y)]) as f32
    }
    fn outline_distance(&self, p: [f64; 2]) -> f64 {
        self.segments
            .iter()
            .map(|&[a, b]| segment_distance(p, a, b))
            .fold(f64::INFINITY, f64::min)
    }

    /// Spots for connectors of `radius` spread over the cut face ("Place automatically"):
    /// each island of the face gets 1 to 4, each at least `radius` + [`AUTO_MARGIN`] from
    /// the outline, as far from the `existing` connectors' edges (centre, radius), and two
    /// such margins from each other. The face is rasterised (scanline fill of the crossing
    /// segments, the parity rule of [`Section::contains`]), inset by the margin with a
    /// distance transform that carries each cell's nearest segment, and each island's inset
    /// area is split into clusters (k-means seeded by farthest points): one spot for a small
    /// inset, two along the principal axis for an elongated one, three or four for a large
    /// one. Every spot is checked exactly against the whole outline. Original Mica code.
    pub fn auto_place(&self, radius: f32, existing: &[([f32; 2], f32)]) -> AutoPlacement {
        let mut out = AutoPlacement::default();
        if self.segments.is_empty() || radius <= 0. {
            return out;
        }
        let margin = f64::from(radius) + f64::from(AUTO_MARGIN);
        let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
        for p in self.segments.iter().flatten() {
            for k in 0..2 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        let span = (hi[0] - lo[0]).max(hi[1] - lo[1]);
        let h = (span / 160.).max(0.05);
        let nx = ((hi[0] - lo[0]) / h).ceil() as usize + 1;
        let ny = ((hi[1] - lo[1]) / h).ceil() as usize + 1;
        let centre = |k: usize| {
            [
                lo[0] + ((k % nx) as f64 + 0.5) * h,
                lo[1] + ((k / nx) as f64 + 0.5) * h,
            ]
        };
        // Inside: between pairs of sorted crossings on each row of cell centres.
        let mut inside = vec![false; nx * ny];
        let mut xs = vec![];
        for j in 0..ny {
            let y = centre(j * nx)[1];
            xs.clear();
            for [a, b] in &self.segments {
                if (a[1] > y) != (b[1] > y) {
                    xs.push(a[0] + (y - a[1]) / (b[1] - a[1]) * (b[0] - a[0]));
                }
            }
            xs.sort_by(f64::total_cmp);
            for &[enter, leave] in xs.as_chunks::<2>().0 {
                let first = ((enter - lo[0]) / h - 0.5).ceil().max(0.) as usize;
                let last = ((leave - lo[0]) / h - 0.5).floor();
                if last < 0. {
                    continue;
                }
                for i in first..=(last as usize).min(nx - 1) {
                    inside[j * nx + i] = true;
                }
            }
        }
        // Distance to the outline: exact next to each segment, then sweeps that offer each
        // cell its neighbours' nearest segment.
        let mut near: Vec<Option<usize>> = vec![None; nx * ny];
        let mut dist = vec![f64::INFINITY; nx * ny];
        let offer = |dist: &mut [f64], near: &mut [Option<usize>], k: usize, s: usize| {
            let [a, b] = self.segments[s];
            let d = segment_distance(centre(k), a, b);
            if d < dist[k] {
                dist[k] = d;
                near[k] = Some(s);
            }
        };
        let neighbour = |k: usize, di: i64, dj: i64| {
            let (i, j) = ((k % nx) as i64 + di, (k / nx) as i64 + dj);
            (i >= 0 && j >= 0 && (i as usize) < nx && (j as usize) < ny)
                .then(|| j as usize * nx + i as usize)
        };
        for (s, &[a, b]) in self.segments.iter().enumerate() {
            let len = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
            let steps = (len / (0.5 * h)).ceil() as usize + 1;
            for t in 0..=steps {
                let f = t as f64 / steps as f64;
                let i =
                    (((a[0] + (b[0] - a[0]) * f - lo[0]) / h).floor().max(0.) as usize).min(nx - 1);
                let j =
                    (((a[1] + (b[1] - a[1]) * f - lo[1]) / h).floor().max(0.) as usize).min(ny - 1);
                for dj in -1..=1 {
                    for di in -1..=1 {
                        if let Some(k) = neighbour(j * nx + i, di, dj) {
                            offer(&mut dist, &mut near, k, s);
                        }
                    }
                }
            }
        }
        for _ in 0..2 {
            for forward in [true, false] {
                let sign = if forward { -1 } else { 1 };
                for step in 0..nx * ny {
                    let k = if forward { step } else { nx * ny - 1 - step };
                    for (di, dj) in [(-1, 1), (0, 1), (1, 1), (1, 0)] {
                        if let Some(s) = neighbour(k, di * sign, dj * sign).and_then(|n| near[n]) {
                            offer(&mut dist, &mut near, k, s);
                        }
                    }
                }
            }
        }
        let clear_of_existing = |p: [f64; 2]| {
            existing.iter().all(|&(c, r)| {
                let d =
                    ((p[0] - f64::from(c[0])).powi(2) + (p[1] - f64::from(c[1])).powi(2)).sqrt();
                d >= f64::from(r) + margin
            })
        };
        // Islands: 4-connected inside cells, in row order.
        let mut seen = vec![false; nx * ny];
        let mut stack = vec![];
        for start in 0..nx * ny {
            if !inside[start] || seen[start] {
                continue;
            }
            seen[start] = true;
            stack.push(start);
            let mut inset = vec![];
            while let Some(k) = stack.pop() {
                if dist[k] >= margin && clear_of_existing(centre(k)) {
                    inset.push(centre(k));
                }
                for (di, dj) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
                    if let Some(n) = neighbour(k, di, dj)
                        && inside[n]
                        && !seen[n]
                    {
                        seen[n] = true;
                        stack.push(n);
                    }
                }
            }
            let spots: Vec<[f64; 2]> = spread(&inset, 2. * margin)
                .into_iter()
                .filter(|&p| {
                    self.outline_distance(p) >= margin - 1e-6
                        && self.contains(p[0] as f32, p[1] as f32)
                })
                .collect();
            if spots.is_empty() {
                out.too_small += 1;
            }
            out.points
                .extend(spots.into_iter().map(|p| [p[0] as f32, p[1] as f32]));
        }
        out
    }
}

/// The clearance "Place automatically" keeps between a connector's edge and the outline
/// (and between neighbouring connectors' keep-outs), mm.
pub const AUTO_MARGIN: f32 = 1.5;

/// The result of [`Section::auto_place`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AutoPlacement {
    /// Connector centres in the plane's X / Y.
    pub points: Vec<[f32; 2]>,
    /// Islands of the cut face with no room for a connector of that size.
    pub too_small: usize,
}

fn segment_distance(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let len2 = dx * dx + dy * dy;
    let t = if len2 > 0. {
        (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / len2).clamp(0., 1.)
    } else {
        0.
    };
    ((p[0] - a[0] - t * dx).powi(2) + (p[1] - a[1] - t * dy).powi(2)).sqrt()
}

/// 1 to 4 spots spread over `cells` (one island's usable inset area), at least `gap` apart,
/// each on a cell. How many comes from the inset's extent along and across its principal
/// axis: one when it is shorter than `gap`, two along the axis when it is elongated (or too
/// narrow for more), three or four when it is large both ways. Where comes from k-means
/// (Lloyd) seeded with farthest points; fewer spots when they would crowd.
fn spread(cells: &[[f64; 2]], gap: f64) -> Vec<[f64; 2]> {
    if cells.is_empty() {
        return vec![];
    }
    let n = cells.len() as f64;
    let mean = cells
        .iter()
        .fold([0.; 2], |m, p| [m[0] + p[0] / n, m[1] + p[1] / n]);
    let (mut sxx, mut sxy, mut syy) = (0., 0., 0.);
    for p in cells {
        let (dx, dy) = (p[0] - mean[0], p[1] - mean[1]);
        sxx += dx * dx;
        sxy += dx * dy;
        syy += dy * dy;
    }
    let angle = 0.5 * (2. * sxy).atan2(sxx - syy);
    let extent = |axis: [f64; 2]| {
        let (lo, hi) = cells
            .iter()
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), p| {
                let t = (p[0] - mean[0]) * axis[0] + (p[1] - mean[1]) * axis[1];
                (lo.min(t), hi.max(t))
            });
        hi - lo
    };
    let long = extent([angle.cos(), angle.sin()]);
    let short = extent([-angle.sin(), angle.cos()]);
    let (long, short) = (long.max(short), long.min(short));
    let wanted = if long < gap {
        1
    } else if long >= 2. * short || short < gap {
        2
    } else if short < 2.5 * gap {
        3
    } else {
        4
    };
    let d2 = |a: [f64; 2], b: [f64; 2]| (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2);
    let nearest_cell = |p: [f64; 2]| {
        cells
            .iter()
            .copied()
            .min_by(|a, b| d2(*a, p).total_cmp(&d2(*b, p)))
            .unwrap_or(p)
    };
    for k in (2..=wanted).rev() {
        // Farthest-point seeds, starting from the cell farthest from the centre.
        let mut seeds = vec![];
        let mut from = vec![f64::INFINITY; cells.len()];
        let mut next = cells
            .iter()
            .copied()
            .max_by(|a, b| d2(*a, mean).total_cmp(&d2(*b, mean)))
            .unwrap_or(mean);
        while seeds.len() < k {
            seeds.push(next);
            for (f, p) in from.iter_mut().zip(cells) {
                *f = f.min(d2(*p, next));
            }
            next = cells[(0..cells.len())
                .max_by(|&a, &b| from[a].total_cmp(&from[b]))
                .unwrap_or(0)];
        }
        for _ in 0..30 {
            let mut sums = vec![[0.; 3]; k];
            for p in cells {
                let c = (0..k)
                    .min_by(|&a, &b| d2(*p, seeds[a]).total_cmp(&d2(*p, seeds[b])))
                    .unwrap_or(0);
                sums[c] = [sums[c][0] + p[0], sums[c][1] + p[1], sums[c][2] + 1.];
            }
            for (s, sum) in seeds.iter_mut().zip(&sums) {
                if sum[2] > 0. {
                    *s = [sum[0] / sum[2], sum[1] / sum[2]];
                }
            }
        }
        let spots: Vec<[f64; 2]> = seeds.into_iter().map(nearest_cell).collect();
        let apart = spots
            .iter()
            .enumerate()
            .all(|(i, a)| spots[i + 1..].iter().all(|b| d2(*a, *b) >= gap * gap));
        if apart {
            return spots;
        }
    }
    vec![nearest_cell(mean)]
}

fn centred(mut m: TriangleMesh) -> TriangleMesh {
    let (lo, hi) = m.bounds();
    let c: [f32; 3] = std::array::from_fn(|k| 0.5 * (lo[k] + hi[k]));
    for v in &mut m.vertices {
        for k in 0..3 {
            v[k] -= c[k];
        }
    }
    m
}

/// `Eigen::Rotation2Df(angle) * (0, len)`.
fn rot(angle: f64, len: f64) -> (f32, f32) {
    let (s, c) = angle.sin_cos();
    ((-s * len) as f32, (c * len) as f32)
}

/// Upstream `its_make_cylinder`: base at z = 0, `fa` the angle step.
fn make_cylinder(r: f64, h: f64, fa: f64) -> TriangleMesh {
    make_frustum_ratio(r, r, h, fa)
}

/// Upstream `its_make_frustum`: the top has half the base radius.
fn make_frustum(r: f64, h: f64, fa: f64) -> TriangleMesh {
    make_frustum_ratio(r, 0.5 * r, h, fa)
}

/// The shared body of `its_make_cylinder` and `its_make_frustum` (identical but for the top
/// radius), with upstream's vertex and facet order.
fn make_frustum_ratio(r_bottom: f64, r_top: f64, h: f64, fa: f64) -> TriangleMesh {
    let n_steps = (std::f64::consts::TAU / fa).ceil() as usize;
    let angle_step = std::f64::consts::TAU / n_steps as f64;
    let mut vertices = Vec::with_capacity(2 * n_steps + 2);
    let mut faces: Vec<[u32; 3]> = Vec::with_capacity(4 * n_steps);
    vertices.push([0., 0., 0.]);
    vertices.push([0., 0., h as f32]);
    let push = |vertices: &mut Vec<[f32; 3]>, angle: f64| {
        let (bx, by) = rot(angle, r_bottom);
        let (tx, ty) = rot(angle, r_top);
        vertices.push([bx, by, 0.]);
        vertices.push([tx, ty, h as f32]);
    };
    push(&mut vertices, 0.);
    for i in 1..n_steps {
        push(&mut vertices, angle_step * i as f64);
        let id = vertices.len() as u32 - 1;
        faces.push([0, id - 1, id - 3]);
        faces.push([id, 1, id - 2]);
        faces.push([id, id - 2, id - 3]);
        faces.push([id, id - 3, id - 1]);
    }
    let id = vertices.len() as u32 - 1;
    faces.push([0, 2, id - 1]);
    faces.push([3, 1, id]);
    faces.push([id, 2, 3]);
    faces.push([id, id - 1, 2]);
    TriangleMesh { vertices, faces }
}

/// Upstream `its_make_snap`: two half-pegs either side of a slot, bulging at mid-height.
fn make_snap(r: f64, h: f64, space_proportion: f32, bulge_proportion: f32) -> TriangleMesh {
    let radius = r as f32;
    let height = h as f32;
    let sectors_cnt = 10usize;
    let half_pi = std::f32::consts::FRAC_PI_2;
    let space_len = space_proportion * radius;
    let b_len = radius;
    let m_len = (1. + bulge_proportion) * radius;
    let t_len = 0.5 * radius;
    let (b_height, m_height, t_height) = (0., 0.5 * height, height);
    let b_angle = (space_len / b_len).acos();
    let t_angle = (space_len / t_len).acos();
    let b_angle_step = b_angle / sectors_cnt as f32;
    let t_angle_step = t_angle / sectors_cnt as f32;
    let rot2 = |angle: f32, len: f32| {
        let (s, c) = angle.sin_cos();
        (-s * len, c * len)
    };
    let add_side_vertices = |v: &mut Vec<[f32; 3]>, b_angle: f32, t_angle: f32, m_len: f32| {
        let b = rot2(b_angle, b_len);
        let m = rot2(b_angle, m_len);
        let t = rot2(t_angle, t_len);
        v.push([b.0, b.1, b_height]);
        v.push([m.0, m.1, m_height]);
        v.push([t.0, t.1, t_height]);
    };
    let add_side_facets = |f: &mut Vec<[u32; 3]>, count: usize, first: u32, second: u32| {
        let id = count as u32 - 1;
        f.push([first, id - 2, id - 5]);
        f.push([id - 2, id - 1, id - 5]);
        f.push([id - 1, id - 4, id - 5]);
        f.push([id - 4, id - 1, id]);
        f.push([id, id - 3, id - 4]);
        f.push([id, second, id - 3]);
    };
    // Flattening of the ellipse the bulge follows.
    let flat = (b_len - m_len) / m_len;
    let get_m_len = |angle: f32| {
        let rad_sqr = b_len * b_len;
        let sin_sqr = angle.sin() * angle.sin();
        let f_sqr = (1. - flat) * (1. - flat);
        (rad_sqr / (1. + (1. / f_sqr - 1.) * sin_sqr)).sqrt()
    };
    let mut mesh = TriangleMesh::default();
    let add_sub_mesh = |mesh: &mut TriangleMesh, center_x: f32, angle_rotation: f32| {
        let first = mesh.vertices.len() as u32;
        let second = first + 1;
        mesh.vertices.push([center_x, 0., b_height]);
        mesh.vertices.push([center_x, 0., t_height]);
        let b_start = angle_rotation - b_angle;
        let t_start = angle_rotation - t_angle;
        add_side_vertices(&mut mesh.vertices, b_start, t_start, get_m_len(b_start));
        let id = mesh.vertices.len() as u32 - 1;
        mesh.faces.push([first, id - 2, id - 1]);
        mesh.faces.push([first, id - 1, id]);
        mesh.faces.push([first, id, second]);
        // Upstream adds steps until the angle is approximately the stop angle (start +
        // 2 * b_angle): exactly 2 * sectors_cnt steps, counted here instead of compared.
        for k in 1..=2 * sectors_cnt {
            let b = b_start + b_angle_step * k as f32;
            let t = t_start + t_angle_step * k as f32;
            add_side_vertices(&mut mesh.vertices, b, t, get_m_len(b));
            add_side_facets(&mut mesh.faces, mesh.vertices.len(), first, second);
        }
        let id = mesh.vertices.len() as u32 - 1;
        mesh.faces.push([first, second, id]);
        mesh.faces.push([first, id, id - 1]);
        mesh.faces.push([first, id - 1, id - 2]);
    };
    add_sub_mesh(&mut mesh, -space_len, half_pi);
    add_sub_mesh(&mut mesh, space_len, 3. * half_pi);
    mesh
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh_cut::is_watertight;

    #[test]
    fn unit_meshes_are_closed_centred_and_sized() {
        for kind in ConnectorType::ALL {
            for style in ConnectorStyle::ALL {
                for shape in ConnectorShape::ALL {
                    let a = Attributes { kind, style, shape };
                    for m in [unit_mesh(a), unit_hole_mesh(a)] {
                        assert!(is_watertight(&m), "{a:?}");
                        assert!(m.volume() > 0., "{a:?}: inside out");
                        let (lo, hi) = m.bounds();
                        for k in 0..3 {
                            assert!((lo[k] + hi[k]).abs() < 1e-4, "{a:?}: not centred");
                        }
                        assert!((hi[2] - lo[2] - 1.).abs() < 1e-5, "{a:?}");
                    }
                }
            }
        }
        // A unit round prism is close to π, a frustum about 7/12 of it, a square prism 2.
        let round = Attributes::default();
        assert!((unit_mesh(round).volume() - std::f64::consts::PI).abs() < 1e-3);
        let frustum = Attributes {
            style: ConnectorStyle::Frustum,
            ..round
        };
        let v = unit_mesh(frustum).volume();
        assert!((v - std::f64::consts::PI * 7. / 12.).abs() < 1e-3, "{v}");
        let square = Attributes {
            shape: ConnectorShape::Square,
            ..round
        };
        assert!((unit_mesh(square).volume() - 2.).abs() < 1e-4);
        // A dowel is always a prism; a snap is round and wider than its hole at the bulge.
        let dowel = Attributes {
            kind: ConnectorType::Dowel,
            style: ConnectorStyle::Frustum,
            shape: ConnectorShape::Hexagon,
        };
        assert_eq!(dowel.effective().style, ConnectorStyle::Prism);
        let snap = unit_mesh(Attributes {
            kind: ConnectorType::Snap,
            ..round
        });
        let (lo, hi) = snap.bounds();
        assert!(hi[0] - lo[0] > 2. && hi[0] - lo[0] < 2. * (1. + SNAP_BULGE) + 1e-3);
    }

    #[test]
    fn fits_follow_upstream_tolerances() {
        let s = Sizes {
            radius: 2.,
            height: 5.,
            radius_tolerance: 0.1,
            height_tolerance: 0.2,
            angle: 0.,
        };
        let plug = Attributes::default();
        assert_eq!(
            body(plug, &s),
            Fit {
                scale: [2., 2., 5.],
                offset: 2.5
            }
        );
        let h = hole(plug, &s);
        assert_eq!(h.scale, [2.1, 2.1, 5.2]);
        assert!((h.offset - (2.5 + 0.1 - PREVIEW_SHIFT)).abs() < 1e-6);
        // The hole reaches past the plug's top by the depth tolerance and opens below the
        // cut face by the preview shift.
        assert!((h.offset + h.scale[2] / 2. - 5.2 + PREVIEW_SHIFT).abs() < 1e-5);
        assert!((h.offset - h.scale[2] / 2. + PREVIEW_SHIFT).abs() < 1e-5);
        let dowel = Attributes {
            kind: ConnectorType::Dowel,
            ..plug
        };
        assert_eq!(body(dowel, &s).scale, [2., 2., 10.]);
        assert_eq!(body(dowel, &s).offset, 0.);
        assert_eq!(hole(dowel, &s).scale[2], 10.2);
    }

    #[test]
    fn conflicts_cover_contour_box_and_overlap() {
        let cube = crate::mesh_edit::cuboid([20., 20., 20.]);
        let axes = [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]];
        let section = Section::new(&cube, [0., 0., 0.], axes);
        let inside = |p: [f32; 3]| section.contains(p[0], p[1]);
        assert!(inside([0., 0., 0.]) && inside([9.9, -9.9, 0.]) && !inside([11., 0., 0.]));
        assert!(Section::new(&cube, [0., 0., 30.], axes).is_empty());
        let bounds = cube.bounds();
        let at = |x: f32, y: f32| Placed {
            point: [x, y, 0.],
            attributes: Attributes::default(),
            sizes: Sizes {
                radius: 2.,
                ..Sizes::default()
            },
        };
        let got = conflicts(
            &[at(0., 0.), at(9., 0.), at(-5., 5.), at(-4., 3.)],
            axes,
            bounds,
            inside,
        );
        assert_eq!(
            got,
            vec![
                None,
                Some(Conflict::OutsideContour),
                Some(Conflict::Overlap),
                Some(Conflict::Overlap)
            ]
        );
        // A plug deeper than the half it goes into reaches out of the object.
        let mut deep = at(0., 0.);
        deep.sizes.height = 15.;
        assert_eq!(
            conflicts(&[deep], axes, bounds, inside),
            vec![Some(Conflict::OutsideObject)]
        );
    }

    const FLAT: [[f32; 3]; 3] = [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]];

    /// Auto-placed connectors of `radius` on `mesh` cut at `z`: the spots, after checking
    /// that none conflicts and each keeps the margin from the outline and the others.
    fn auto(mesh: &TriangleMesh, z: f32, radius: f32) -> AutoPlacement {
        let section = Section::new(mesh, [0., 0., z], FLAT);
        let got = section.auto_place(radius, &[]);
        let sizes = Sizes {
            radius,
            ..Sizes::default()
        };
        let placed: Vec<Placed> = got
            .points
            .iter()
            .map(|p| Placed {
                point: [p[0], p[1], z],
                attributes: Attributes {
                    kind: ConnectorType::Dowel,
                    ..Attributes::default()
                },
                sizes,
            })
            .collect();
        let found = conflicts(&placed, FLAT, mesh.bounds(), |p| {
            section.contains(p[0], p[1])
        });
        assert!(found.iter().all(Option::is_none), "{found:?} at {got:?}");
        let margin = radius + AUTO_MARGIN;
        for (i, p) in got.points.iter().enumerate() {
            assert!(section.distance_to_outline(p[0], p[1]) >= margin - 1e-4);
            for q in &got.points[i + 1..] {
                let d = ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2)).sqrt();
                assert!(d >= 2. * margin - 1e-4, "{p:?} {q:?}");
            }
        }
        got
    }

    #[test]
    fn auto_place_puts_two_along_a_long_box() {
        let bar = crate::mesh_edit::cuboid([20., 40., 10.]);
        let got = auto(&bar, 0., 1.25);
        assert_eq!(got.too_small, 0);
        assert_eq!(got.points.len(), 2, "{got:?}");
        let [a, b] = [got.points[0], got.points[1]];
        // Along the long (Y) axis, centred across it, evenly either side of the middle.
        assert!(a[0].abs() < 0.5 && b[0].abs() < 0.5, "{got:?}");
        assert!((a[1] + b[1]).abs() < 0.5, "{got:?}");
        assert!((a[1] - b[1]).abs() > 12., "spread out: {got:?}");
    }

    #[test]
    fn auto_place_puts_three_or_four_on_a_large_round_face() {
        let rod = make_cylinder(15., 10., std::f64::consts::PI / 90.);
        let got = auto(&rod, 5., 1.25);
        assert!((3..=4).contains(&got.points.len()), "{got:?}");
        // Spread around the middle, not bunched on one side.
        let n = got.points.len() as f32;
        let mean = got
            .points
            .iter()
            .fold([0.; 2], |m, p| [m[0] + p[0] / n, m[1] + p[1] / n]);
        assert!(mean[0].hypot(mean[1]) < 1., "{got:?}");
        // A small round face takes one in the middle.
        let pin = make_cylinder(4., 10., std::f64::consts::PI / 90.);
        let got = auto(&pin, 5., 1.25);
        assert_eq!(got.points.len(), 1, "{got:?}");
        assert!(got.points[0][0].hypot(got.points[0][1]) < 0.5);
    }

    #[test]
    fn auto_place_skips_a_face_too_small_and_keeps_clear_of_existing_ones() {
        let thin = make_cylinder(2.5, 10., std::f64::consts::PI / 90.);
        let got = auto(&thin, 5., 1.25);
        assert_eq!(
            got,
            AutoPlacement {
                points: vec![],
                too_small: 1
            }
        );
        // A plane that misses the object: nothing at all, nothing to report.
        assert_eq!(auto(&thin, 50., 1.25), AutoPlacement::default());
        // Two islands (two separate bars), each gets its own; one already holds a connector
        // in its middle, so the new ones there keep clear of it.
        let mut left = crate::mesh_edit::cuboid([10., 40., 10.]);
        let right = left.clone();
        for v in &mut left.vertices {
            v[0] -= 20.;
        }
        let both = crate::mesh_edit::merge(&[&left, &right]);
        let got = auto(&both, 0., 1.25);
        assert_eq!(got.points.len(), 4, "{got:?}");
        let section = Section::new(&both, [0., 0., 0.], FLAT);
        let got = section.auto_place(1.25, &[([0., 0.], 2.)]);
        let right_side: Vec<_> = got.points.iter().filter(|p| p[0] > -10.).collect();
        assert_eq!(right_side.len(), 2, "{got:?}");
        assert!(
            right_side
                .iter()
                .all(|p| p[0].hypot(p[1]) >= 2. + 1.25 + AUTO_MARGIN)
        );
    }
}
