// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The prime / wipe tower as a thing on the bed: a translucent box of its real footprint and
//! height that is selected, dragged, nudged and turned like an object, with its position kept
//! per plate (`Plate::tower_placement`). Before a plate is sliced the box is the planner's
//! estimate for the tools the plate uses and its tallest object; after slicing it is the
//! sliced job's exact plan. The tower stays inside the printable area and outside exclusion
//! zones; overlapping an object is a warning that also blocks slicing.

use crate::{
    app::App,
    overlays::Target,
    panels::editor::Edit,
    scene::{Bed, Delta, Object},
};
use glam::{Mat4, Quat, Vec2, Vec3};
use mica_core::{
    arrange::polygons_overlap,
    config::Config,
    mesh::TriangleMesh,
    wipe_tower::{TowerKind, TowerShape},
};

/// Height of the drawn footprint slab (brim, ribs, cone), mm.
pub const FOOTPRINT_Z: f32 = 0.4;
/// Clearance kept to objects and the bed edge when the tower is placed automatically, mm.
pub const MARGIN: f64 = 2.;

/// What keeps a tower from printing where it stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Problem {
    Outside,
    Excluded,
    Overlaps(String),
}

impl Problem {
    pub fn text(&self) -> String {
        match self {
            Problem::Outside => "Outside the printable area".into(),
            Problem::Excluded => "In an exclusion zone".into(),
            Problem::Overlaps(name) => format!("Overlaps {name}"),
        }
    }
}

/// A plate's tower as the viewport draws it.
#[derive(Clone, Debug, PartialEq)]
pub struct TowerView {
    pub plate: usize,
    /// World offset of the plate.
    pub origin: Vec2,
    /// Plate-local shape (the engine's coordinates).
    pub shape: TowerShape,
    pub name: &'static str,
    pub problems: Vec<Problem>,
}

/// Snapmaker / Orca say "Prime tower", Prusa says "Wipe tower".
pub fn name_for(kind: TowerKind) -> &'static str {
    match kind {
        TowerKind::Orca => "Prime tower",
        TowerKind::Prusa => "Wipe tower",
    }
}

/// Whether the printer's tower can be turned (`wipe_tower_rotation_angle`): Prusa towers
/// and Orca's `WipeTower2` (every non-Bambu printer, the U1 included) can; Bambu's type1
/// tower, which Mica does not plan, cannot.
pub fn rotation_supported(c: &Config) -> bool {
    let kind = c
        .string("wipe_tower_type")
        .or_else(|| c.extra("wipe_tower_type"));
    !mica_core::rules::Rules::of(c).is_orca() || kind != Some("type1")
}

impl TowerView {
    /// The tower's name for the printer (`Prime tower` / `Wipe tower`).
    pub fn label(&self) -> &'static str {
        self.name
    }
    /// Footprint outline in world coordinates.
    pub fn world_hull(&self) -> Vec<[f32; 2]> {
        self.shape
            .hull
            .iter()
            .map(|p| [p[0] as f32 + self.origin.x, p[1] as f32 + self.origin.y])
            .collect()
    }
    /// World centre of the body's base.
    pub fn centre(&self) -> Vec3 {
        let c = self.shape.centre();
        Vec3::new(c[0] as f32 + self.origin.x, c[1] as f32 + self.origin.y, 0.)
    }
    /// World bounds of the footprint and height.
    pub fn bounds(&self) -> (Vec3, Vec3) {
        let (min, max) = self.shape.bbox();
        (
            Vec3::new(
                min[0] as f32 + self.origin.x,
                min[1] as f32 + self.origin.y,
                0.,
            ),
            Vec3::new(
                max[0] as f32 + self.origin.x,
                max[1] as f32 + self.origin.y,
                self.shape.height as f32,
            ),
        )
    }
    /// Radius of the footprint about the body centre, for the rotate ring.
    pub fn radius(&self) -> f32 {
        let c = self.centre().truncate();
        self.world_hull()
            .iter()
            .map(|p| Vec2::new(p[0], p[1]).distance(c))
            .fold(0., f32::max)
    }
    /// Transform placing the unit cube as the body box.
    pub fn body_transform(&self) -> Mat4 {
        let s = &self.shape;
        let centre = self.centre().with_z(s.height as f32 * 0.5);
        Mat4::from_scale_rotation_translation(
            Vec3::new(s.width as f32, s.depth as f32, s.height.max(0.1) as f32),
            Quat::from_rotation_z(s.rotation.to_radians() as f32),
            centre,
        )
    }
    /// Thin prism over the footprint outline (world coordinates), for the brim / rib slab.
    pub fn footprint_mesh(&self) -> TriangleMesh {
        let hull = self.world_hull();
        let n = hull.len();
        let mut vertices = Vec::with_capacity(n * 2);
        for p in &hull {
            vertices.push([p[0], p[1], 0.02]);
        }
        for p in &hull {
            vertices.push([p[0], p[1], FOOTPRINT_Z]);
        }
        let mut faces = Vec::new();
        if n >= 3 {
            let n32 = n as u32;
            for i in 1..n32 - 1 {
                faces.push([0, i + 1, i]);
                faces.push([n32, n32 + i, n32 + i + 1]);
            }
            for i in 0..n32 {
                let j = (i + 1) % n32;
                faces.push([i, j, n32 + j]);
                faces.push([i, n32 + j, n32 + i]);
            }
        }
        TriangleMesh { vertices, faces }
    }
    /// A GPU mesh key for the footprint that changes with its outline.
    pub fn footprint_id(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.plate.hash(&mut h);
        for p in &self.shape.hull {
            p[0].to_bits().hash(&mut h);
            p[1].to_bits().hash(&mut h);
        }
        self.origin.x.to_bits().hash(&mut h);
        self.origin.y.to_bits().hash(&mut h);
        (h.finish() >> 2) | (1 << 62)
    }
    /// Distance along a world ray to the body box or the footprint slab, if it hits.
    pub fn pick(&self, origin: Vec3, dir: Vec3) -> Option<f32> {
        let s = &self.shape;
        // Body: a slab test in the tower's own frame.
        let (sin, cos) = (s.rotation.to_radians() as f32).sin_cos();
        let anchor = Vec3::new(
            s.position[0] as f32 + self.origin.x,
            s.position[1] as f32 + self.origin.y,
            0.,
        );
        let local = |p: Vec3| {
            let d = p - anchor;
            Vec3::new(cos * d.x + sin * d.y, -sin * d.x + cos * d.y, d.z)
        };
        let o = local(origin);
        let d = Vec3::new(cos * dir.x + sin * dir.y, -sin * dir.x + cos * dir.y, dir.z);
        let size = Vec3::new(s.width as f32, s.depth as f32, s.height.max(0.1) as f32);
        let mut t0 = 0f32;
        let mut t1 = f32::INFINITY;
        let mut body = true;
        for i in 0..3 {
            if d[i].abs() < 1e-9 {
                if o[i] < 0. || o[i] > size[i] {
                    body = false;
                    break;
                }
                continue;
            }
            let a = (0. - o[i]) / d[i];
            let b = (size[i] - o[i]) / d[i];
            t0 = t0.max(a.min(b));
            t1 = t1.min(a.max(b));
            if t0 > t1 {
                body = false;
                break;
            }
        }
        let mut best = body.then_some(t0);
        // Footprint: where the ray meets the bed, inside the outline.
        if dir.z.abs() > 1e-6 {
            let t = (FOOTPRINT_Z - origin.z) / dir.z;
            if t > 0. {
                let p = origin + dir * t;
                let hull = self.world_hull();
                if crate::scene::bed::point_in_polygon(&hull, p.truncate())
                    && best.is_none_or(|b| t < b)
                {
                    best = Some(t);
                }
            }
        }
        best
    }
}

/// Axis-aligned bounds of a polygon.
fn bounds(poly: &[[f64; 2]]) -> ([f64; 2], [f64; 2]) {
    let mut min = [f64::INFINITY; 2];
    let mut max = [f64::NEG_INFINITY; 2];
    for p in poly {
        for i in 0..2 {
            min[i] = min[i].min(p[i]);
            max[i] = max[i].max(p[i]);
        }
    }
    (min, max)
}

fn bed_box(bed: &Bed) -> ([f64; 2], [f64; 2]) {
    (
        [bed.min.x as f64, bed.min.y as f64],
        [bed.max.x as f64, bed.max.y as f64],
    )
}

fn zone_box(zone: &[[f32; 2]]) -> ([f64; 2], [f64; 2]) {
    bounds(
        &zone
            .iter()
            .map(|p| [p[0] as f64, p[1] as f64])
            .collect::<Vec<_>>(),
    )
}

/// The shift that brings the footprint inside the bed's bounds (none when it is wider than
/// the bed in that axis) and then out of any exclusion zone it overlaps, by the shortest
/// axis-aligned push that stays on the bed. Plate-local coordinates.
pub fn clamp_shift(shape: &TowerShape, bed: &Bed) -> [f64; 2] {
    let (bmin, bmax) = bed_box(bed);
    let (mut min, mut max) = shape.bbox();
    let mut shift = [0f64; 2];
    for i in 0..2 {
        if max[i] - min[i] > bmax[i] - bmin[i] {
            continue;
        }
        let d = if min[i] < bmin[i] {
            bmin[i] - min[i]
        } else if max[i] > bmax[i] {
            bmax[i] - max[i]
        } else {
            0.
        };
        shift[i] += d;
        min[i] += d;
        max[i] += d;
    }
    // Exclusion zones: push along the axis that needs the least, if that stays on the bed.
    for _ in 0..4 {
        let eps = 1e-6;
        let Some((zmin, zmax)) = bed
            .exclusions
            .iter()
            .map(|z| zone_box(z))
            .find(|(zmin, zmax)| {
                min[0] < zmax[0] - eps
                    && max[0] > zmin[0] + eps
                    && min[1] < zmax[1] - eps
                    && max[1] > zmin[1] + eps
            })
        else {
            break;
        };
        let mut moves: Vec<(usize, f64)> = vec![
            (0, zmax[0] - min[0]),
            (0, zmin[0] - max[0]),
            (1, zmax[1] - min[1]),
            (1, zmin[1] - max[1]),
        ];
        moves.retain(|(i, d)| min[*i] + d >= bmin[*i] - eps && max[*i] + d <= bmax[*i] + eps);
        moves.sort_by(|a, b| a.1.abs().total_cmp(&b.1.abs()));
        let Some((i, d)) = moves.first().copied() else {
            break;
        };
        shift[i] += d;
        min[i] += d;
        max[i] += d;
    }
    shift
}

/// Names of the objects whose footprints the tower overlaps (plate-local objects).
pub fn overlapping(shape: &TowerShape, objects: &[Object]) -> Vec<String> {
    let mut seen = Vec::new();
    for o in objects {
        if polygons_overlap(&shape.hull, &o.footprint_hull()) {
            let short = std::path::Path::new(&o.name)
                .file_stem()
                .map_or(o.name.clone(), |s| s.to_string_lossy().into_owned());
            if !seen.contains(&short) {
                seen.push(short);
            }
        }
    }
    seen
}

/// Everything wrong with the tower where it stands.
pub fn problems(shape: &TowerShape, bed: &Bed, objects: &[Object]) -> Vec<Problem> {
    let mut out = Vec::new();
    let (bmin, bmax) = bed_box(bed);
    let (min, max) = shape.bbox();
    let eps = 0.01;
    if (0..2).any(|i| min[i] < bmin[i] - eps || max[i] > bmax[i] + eps)
        || shape
            .hull
            .iter()
            .any(|p| !bed.contains(Vec2::new(p[0] as f32, p[1] as f32)) && bed.polygon.len() != 4)
    {
        out.push(Problem::Outside);
    } else if bed.exclusions.iter().any(|z| {
        let (zmin, zmax) = zone_box(z);
        min[0] < zmax[0] - eps
            && max[0] > zmin[0] + eps
            && min[1] < zmax[1] - eps
            && max[1] > zmin[1] + eps
    }) {
        out.push(Problem::Excluded);
    }
    out.extend(
        overlapping(shape, objects)
            .into_iter()
            .map(Problem::Overlaps),
    );
    out
}

/// A position (the tower's `wipe_tower_x/y`) in a free corner or along a free edge of the
/// bed, `MARGIN` from the edge and at least `spacing` from every object, nearest to where the
/// tower stands now; `None` when no candidate fits.
pub fn free_spot(
    shape: &TowerShape,
    bed: &Bed,
    objects: &[Object],
    spacing: f64,
) -> Option<[f64; 2]> {
    let (bmin, bmax) = bed_box(bed);
    let (hmin, hmax) = shape.bbox();
    let size = [hmax[0] - hmin[0], hmax[1] - hmin[1]];
    // Where the footprint's bottom-left corner may go.
    let lo = [bmin[0] + MARGIN, bmin[1] + MARGIN];
    let hi = [bmax[0] - MARGIN - size[0], bmax[1] - MARGIN - size[1]];
    if hi[0] < lo[0] || hi[1] < lo[1] {
        return None;
    }
    let hulls: Vec<Vec<[f64; 2]>> = objects.iter().map(Object::footprint_hull).collect();
    let boxes: Vec<([f64; 2], [f64; 2])> = hulls.iter().map(|h| bounds(h)).collect();
    let mid = [(lo[0] + hi[0]) / 2., (lo[1] + hi[1]) / 2.];
    // Corners first, then the edge middles, then a coarse grid so a crowded bed still finds
    // a gap; within a tier the spot nearest to where the tower stands wins.
    let corners = vec![
        [lo[0], lo[1]],
        [hi[0], lo[1]],
        [lo[0], hi[1]],
        [hi[0], hi[1]],
    ];
    let edges = vec![
        [mid[0], lo[1]],
        [mid[0], hi[1]],
        [lo[0], mid[1]],
        [hi[0], mid[1]],
    ];
    let mut grid = Vec::new();
    let step = 10.;
    let mut y = lo[1];
    while y <= hi[1] {
        let mut x = lo[0];
        while x <= hi[0] {
            grid.push([x, y]);
            x += step;
        }
        y += step;
    }
    let fits = |corner: [f64; 2]| {
        let dx = corner[0] - hmin[0];
        let dy = corner[1] - hmin[1];
        let moved = shape.translated(dx, dy);
        let (mmin, mmax) = moved.bbox();
        if bed.exclusions.iter().any(|z| {
            let (zmin, zmax) = zone_box(z);
            mmin[0] < zmax[0] + MARGIN
                && mmax[0] > zmin[0] - MARGIN
                && mmin[1] < zmax[1] + MARGIN
                && mmax[1] > zmin[1] - MARGIN
        }) {
            return None;
        }
        if bed.polygon.len() != 4
            && moved
                .hull
                .iter()
                .any(|p| !bed.contains(Vec2::new(p[0] as f32, p[1] as f32)))
        {
            return None;
        }
        for (h, (omin, omax)) in hulls.iter().zip(&boxes) {
            let near = mmin[0] < omax[0] + spacing
                && mmax[0] > omin[0] - spacing
                && mmin[1] < omax[1] + spacing
                && mmax[1] > omin[1] - spacing;
            if near
                && (spacing <= 0. || polygons_overlap(&moved.hull, h) || {
                    // Within the spacing band: keep the gap unless the boxes already clear it.
                    let gap_x = (omin[0] - mmax[0]).max(mmin[0] - omax[0]);
                    let gap_y = (omin[1] - mmax[1]).max(mmin[1] - omax[1]);
                    gap_x.max(gap_y) < spacing
                })
            {
                return None;
            }
        }
        Some([shape.position[0] + dx, shape.position[1] + dy])
    };
    let here = shape.position;
    for tier in [corners, edges, grid] {
        let mut best: Option<([f64; 2], f64)> = None;
        for c in tier {
            if let Some(p) = fits(c) {
                let d = (p[0] - here[0]).hypot(p[1] - here[1]);
                if best.is_none_or(|(_, bd)| d < bd) {
                    best = Some((p, d));
                }
            }
        }
        if let Some((p, _)) = best {
            return Some(p);
        }
    }
    None
}

/// The tower's `wipe_tower_x/y` for a given body centre and rotation (the anchor corner is
/// the centre moved back by half the body in the tower's frame).
pub fn position_for_centre(shape: &TowerShape, centre: [f64; 2], rotation: f64) -> [f64; 2] {
    let (sin, cos) = rotation.to_radians().sin_cos();
    let (hx, hy) = (shape.width / 2., shape.depth / 2.);
    [
        centre[0] - (cos * hx - sin * hy),
        centre[1] - (sin * hx + cos * hy),
    ]
}

/// What a plate's tower estimate depends on, besides where it stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TowerKey {
    tools: Vec<usize>,
    height: u64,
    rotation: u64,
    generation: u64,
    overrides: u64,
}

/// What the tower's context menu and card can do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TowerCommand {
    /// `wipe_tower = 0` in the process settings.
    Disable,
    /// Move to a free corner.
    FreeSpot,
    /// Show its settings in the sidebar.
    Settings,
}

impl App {
    /// `Prime tower` or `Wipe tower`, by the printer's slicing rules (Orca's or PrusaSlicer's tower).
    pub(crate) fn tower_name(&self) -> &'static str {
        match self.effective_config() {
            Some(c) if mica_core::rules::Rules::of(c).is_orca() => name_for(TowerKind::Orca),
            _ => name_for(TowerKind::Prusa),
        }
    }

    /// The tools a plate's objects print with: object and part tools, and the tools painted
    /// on them (one-based facet material states).
    fn plate_tools(&self, plate: usize) -> Vec<usize> {
        let mut tools: Vec<usize> = self
            .plates
            .plate(plate)
            .objects
            .iter()
            .flat_map(|o| {
                let painted = o
                    .painting
                    .get(mica_core::painting::PaintKind::Material)
                    .states()
                    .into_iter()
                    .map(|s| usize::from(s) - 1);
                o.tools
                    .0
                    .iter()
                    .flatten()
                    .copied()
                    .chain(painted)
                    .collect::<Vec<_>>()
            })
            .collect();
        let count = self.tool_count().max(1);
        for t in &mut tools {
            *t = (*t).min(count - 1);
        }
        tools.sort_unstable();
        tools.dedup();
        tools
    }

    /// The tower plate `plate` prints, if it prints one: the sliced job's exact plan while the
    /// slice is current, else the planner's estimate at the plate's placement. The estimate
    /// depends on the tools, the tallest object, the rotation and the configuration, not on
    /// where the tower stands, so a cached one is only translated during a drag.
    fn tower_shape_cached(
        &self,
        plate: usize,
        cache: Option<&mut std::collections::HashMap<usize, (TowerKey, TowerShape)>>,
    ) -> Option<TowerShape> {
        let p = self.plates.plate(plate);
        if p.objects.is_empty() {
            return None;
        }
        if let Some(f) = &p.finished
            && let Some(t) = &f.tower
        {
            return Some(t.clone());
        }
        let tools = self.plate_tools(plate);
        if tools.len() < 2 || self.tool_count() < 2 {
            return None;
        }
        let height = p
            .objects
            .iter()
            .map(|o| o.bounds().1.z as f64)
            .fold(0., f64::max);
        let placement = self.tower_placement(plate);
        let key = TowerKey {
            tools: tools.clone(),
            height: height.to_bits(),
            rotation: placement[2].to_bits(),
            generation: self.config_generation,
            overrides: {
                use std::hash::{Hash, Hasher};
                let mut h = std::collections::hash_map::DefaultHasher::new();
                p.overrides.hash(&mut h);
                h.finish()
            },
        };
        if let Some(cache) = &cache
            && let Some((k, shape)) = cache.get(&plate)
            && *k == key
        {
            return Some(shape.translated(
                placement[0] - shape.position[0],
                placement[1] - shape.position[1],
            ));
        }
        let c = self.plate_config(plate).ok()?;
        let shape = mica_core::wipe_tower::estimate::estimate(&c, &tools, height)
            .ok()
            .flatten()?;
        if let Some(cache) = cache {
            cache.insert(plate, (key, shape.clone()));
        }
        Some(shape)
    }

    /// Rebuilds the towers of every plate (called with the warnings, on every change).
    pub(crate) fn refresh_towers(&mut self) {
        let mut cache = std::mem::take(&mut self.tower_cache);
        cache.retain(|plate, _| *plate < self.plates.len());
        let mut towers = Vec::with_capacity(self.plates.len());
        for i in 0..self.plates.len() {
            let shape = self.tower_shape_cached(i, Some(&mut cache));
            towers.push(shape.map(|shape| {
                let p = self.plates.plate(i);
                TowerView {
                    plate: i,
                    origin: self.plates.offset(i).truncate(),
                    problems: problems(&shape, &self.bed, &p.objects),
                    name: name_for(shape.kind),
                    shape,
                }
            }));
        }
        self.towers = towers;
        self.tower_cache = cache;
        if self.tower_selected && self.tower().is_none() {
            self.tower_selected = false;
        }
    }

    /// The current plate's tower.
    pub(crate) fn tower(&self) -> Option<&TowerView> {
        self.towers.get(self.plates.current)?.as_ref()
    }

    /// The tower warnings of the current plate, for the list and the status line.
    pub(crate) fn tower_warnings(&self) -> Vec<String> {
        self.tower()
            .map(|t| {
                t.problems
                    .iter()
                    .map(|p| format!("{} {}", t.name, p.text().to_lowercase()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Why the current plate cannot be sliced because of its tower, if it cannot.
    pub(crate) fn tower_blocks_slicing(&self, plate: usize) -> Option<String> {
        let t = self.towers.get(plate)?.as_ref()?;
        let p = t.problems.first()?;
        Some(format!(
            "{} {}: move it before slicing",
            t.name,
            p.text().to_lowercase()
        ))
    }

    /// Selects the tower (and nothing else).
    pub(crate) fn select_tower(&mut self) {
        if self.tower().is_none() {
            return;
        }
        self.select(None, false);
        self.tower_selected = true;
        self.cut = None;
        self.editor.tower_mode = false;
    }

    /// The placement a plate slices with: `[x, y, rotation]`.
    pub(crate) fn tower_placement(&self, plate: usize) -> [f64; 3] {
        self.effective_tower(self.plates.plate(plate))
            .unwrap_or([180., 140., 0.])
    }

    /// Sets a plate's tower placement. A `wipe_tower_x/y/rotation_angle` edit in the process
    /// settings would otherwise win over it, so such edits are first written into every
    /// plate's placement, then dropped: the position lives on the plates from here on.
    pub(crate) fn set_tower_placement(&mut self, plate: usize, placement: [f64; 3]) {
        let keys = ["wipe_tower_x", "wipe_tower_y", "wipe_tower_rotation_angle"];
        if keys.iter().any(|k| self.overlays.print.contains_key(*k)) {
            let all: Vec<[f64; 3]> = (0..self.plates.len())
                .map(|i| self.tower_placement(i))
                .collect();
            for (p, t) in self.plates.plates.iter_mut().zip(all) {
                p.tower_placement = Some(t);
            }
            for k in keys {
                self.overlays.print.remove(k);
            }
            self.recompute_config();
        }
        self.plates.plates[plate].tower_placement = Some(placement);
        self.plates.plates[plate].invalidate();
    }

    /// Moves or turns the current plate's tower by a gizmo delta, keeping it on the bed.
    /// Scaling is ignored.
    pub(crate) fn move_tower(&mut self, delta: Delta) {
        let plate = self.plates.current;
        let Some(view) = self.tower().cloned() else {
            return;
        };
        if self.plates.current().locked {
            self.status = "This plate is locked; unlock it from its plate label".into();
            return;
        }
        let mut placement = self.tower_placement(plate);
        match delta {
            Delta::Translate(d) => {
                let moved = view.shape.translated(d.x as f64, d.y as f64);
                let shift = clamp_shift(&moved, &self.bed);
                placement[0] = moved.position[0] + shift[0];
                placement[1] = moved.position[1] + shift[1];
            }
            Delta::Rotate { axis, angle } => {
                if !axis.abs_diff_eq(Vec3::Z, 1e-4) {
                    return;
                }
                let Some(c) = self.effective_config() else {
                    return;
                };
                if !rotation_supported(c) {
                    self.status = "This printer's prime tower cannot be turned".into();
                    return;
                }
                let rotation = placement[2] + angle.to_degrees() as f64;
                let rotation = ((rotation % 360.) + 360.) % 360.;
                let rotation = if rotation > 359.9995 { 0. } else { rotation };
                // Turn about the body centre, as the ring suggests.
                let p = position_for_centre(&view.shape, view.shape.centre(), rotation);
                placement = [p[0], p[1], rotation];
            }
            Delta::Scale(_) => return,
        }
        self.set_tower_placement(plate, placement);
        self.changed();
        // A turned tower has a new footprint: settle it on the bed.
        if matches!(delta, Delta::Rotate { .. })
            && let Some(view) = self.tower().cloned()
        {
            let shift = clamp_shift(&view.shape, &self.bed);
            if shift != [0., 0.] {
                let mut p = self.tower_placement(plate);
                p[0] += shift[0];
                p[1] += shift[1];
                self.set_tower_placement(plate, p);
                self.changed();
            }
        }
    }

    /// Sets the tower's X / Y / rotation from the card (one undo step per edit).
    pub(crate) fn set_tower_value(&mut self, axis: usize, value: f64) {
        let plate = self.plates.current;
        if self.tower().is_none() || self.plates.current().locked {
            return;
        }
        self.begin_gesture(if axis == 2 {
            "Turn prime tower"
        } else {
            "Move prime tower"
        });
        let mut p = self.tower_placement(plate);
        p[axis] = value;
        if axis == 2 {
            p[2] = ((value % 360.) + 360.) % 360.;
        }
        self.set_tower_placement(plate, p);
        self.changed();
        if let Some(view) = self.tower().cloned() {
            let shift = clamp_shift(&view.shape, &self.bed);
            if shift != [0., 0.] {
                let mut p = self.tower_placement(plate);
                p[0] += shift[0];
                p[1] += shift[1];
                self.set_tower_placement(plate, p);
                self.changed();
            }
        }
    }

    /// Puts plate `plate`'s tower in a free corner (or along a free edge). Returns whether a
    /// spot was found; the caller records the undo step.
    pub(crate) fn tower_to_free_spot(&mut self, plate: usize) -> bool {
        let Some(view) = self.towers.get(plate).cloned().flatten() else {
            return false;
        };
        let spacing = self.arrange_spacing().max(MARGIN);
        let Some(p) = free_spot(
            &view.shape,
            &self.bed,
            &self.plates.plate(plate).objects,
            spacing,
        ) else {
            return false;
        };
        let mut placement = self.tower_placement(plate);
        placement[0] = p[0];
        placement[1] = p[1];
        self.set_tower_placement(plate, placement);
        true
    }

    pub(crate) fn run_tower_command(&mut self, cmd: TowerCommand) {
        match cmd {
            TowerCommand::Disable => {
                self.tower_selected = false;
                self.editor.tower_mode = false;
                self.apply_edit(
                    Some(Target::Print),
                    Edit::Set {
                        key: "wipe_tower".into(),
                        raw: "0".into(),
                    },
                );
                self.end_gesture();
                self.status = format!("{} disabled", self.tower_name());
            }
            TowerCommand::FreeSpot => {
                if self.plate_locked() {
                    return;
                }
                let plate = self.plates.current;
                self.record("Move prime tower");
                if self.tower_to_free_spot(plate) {
                    self.changed();
                    self.status = format!("{} moved to a free spot", self.tower_name());
                } else {
                    self.status =
                        format!("No free spot for the {}", self.tower_name().to_lowercase());
                }
            }
            TowerCommand::Settings => {
                self.select_tower();
                self.editor.tower_mode = true;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Options;
    use eframe::egui;
    use mica_core::{
        config::Config, input::InputObject, mesh_edit::cuboid, print_job::ToolAssignments,
    };
    use mica_profiles::user::Store;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    fn xl_app(ctx: &egui::Context) -> App {
        let dir = std::env::temp_dir().join(format!(
            "mica-tower-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let mut app = App::create(
            ctx,
            Options {
                prefs_path: None,
                store: Some(Store::open(dir)),
                files: vec![],
                headless: true,
            },
        );
        let (vi, _, name) = app
            .profiles
            .all_printers()
            .into_iter()
            .find(|(_, _, n)| {
                n.contains("XL") && n.contains("5T") && n.contains("0.4") && n.contains("Input")
            })
            .expect("XL 5T preset");
        app.profiles.select_printer(vi, &name);
        app.settings_changed();
        app
    }

    fn cube(name: &str, id: usize, tool: usize, at: [f32; 2]) -> Object {
        let mut o = Object::new(InputObject::plain(
            name,
            id,
            0,
            cuboid([20., 20., 20.]),
            // Supports on the layer's tool ("Default"): PrusaSlicer's wipe tower refuses
            // normal supports with their own tool and a gap (Print::validate).
            ToolAssignments([Some(tool), Some(tool), Some(tool), None, None]),
            Config::defaults(),
        ));
        o.position = Vec3::new(at[0], at[1], 0.);
        o.drop_to_bed();
        o
    }

    fn two_cubes(app: &mut App) {
        let plate = app.plates.current_mut();
        plate.objects.push(cube("a.stl", 0, 0, [60., 60.]));
        plate.objects.push(cube("b.stl", 1, 1, [120., 60.]));
        app.changed();
    }

    fn xl_shape() -> TowerShape {
        let mut c = Config::defaults();
        for (k, v) in [
            ("bed_shape", "0x0,360x0,360x360,0x360"),
            ("layer_height", "0.2"),
            ("first_layer_height", "0.2"),
            ("nozzle_diameter", "0.4,0.4,0.4,0.4,0.4"),
            ("wipe_tower", "1"),
            ("wipe_tower_width", "60"),
            ("wipe_tower_brim_width", "3"),
            ("wipe_tower_cone_angle", "25"),
            ("wipe_tower_extra_spacing", "150%"),
            ("filament_minimal_purge_on_wipe_tower", "15,15,15,15,15"),
        ] {
            c.set_raw(k, v).unwrap();
        }
        mica_core::wipe_tower::estimate::estimate(&c, &[0, 1], 20.)
            .unwrap()
            .unwrap()
    }

    fn bed(exclusion: Option<&str>) -> Bed {
        let mut c = Config::defaults();
        c.set_raw("bed_shape", "0x0,360x0,360x360,0x360").unwrap();
        if let Some(z) = exclusion {
            c.set_raw("bed_exclude_area", z).unwrap();
        }
        Bed::from_config(&c)
    }

    #[test]
    fn clamping_keeps_the_footprint_on_the_bed_and_out_of_exclusion_zones() {
        let s = xl_shape();
        let b = bed(None);
        assert_eq!(clamp_shift(&s, &b), [0., 0.]);
        // Off the right edge: the footprint's right end comes back to x = 360.
        let off = s.translated(150., 0.);
        let shift = clamp_shift(&off, &b);
        assert!(shift[0] < 0. && shift[1] == 0.);
        let back = off.translated(shift[0], shift[1]);
        assert!((back.bbox().1[0] - 360.).abs() < 1e-9, "{:?}", back.bbox());
        // Below the bed: back to y = 0.
        let low = s.translated(0., -200.);
        let shift = clamp_shift(&low, &b);
        assert!((low.translated(0., shift[1]).bbox().0[1]).abs() < 1e-9);
        // A zone under the tower: the shortest push that clears it.
        let zb = bed(Some("170x130,250x130,250x150,170x150"));
        let shift = clamp_shift(&s, &zb);
        let moved = s.translated(shift[0], shift[1]);
        let (min, max) = moved.bbox();
        assert!(
            max[0] <= 170. + 1e-6
                || min[0] >= 250. - 1e-6
                || max[1] <= 130. + 1e-6
                || min[1] >= 150. - 1e-6,
            "{min:?} {max:?}"
        );
        assert!(min[0] >= 0. && max[0] <= 360. && min[1] >= 0. && max[1] <= 360.);
        assert!(problems(&moved, &zb, &[]).is_empty());
        assert_eq!(problems(&s, &zb, &[]), [Problem::Excluded]);
        assert_eq!(problems(&off, &b, &[]), [Problem::Outside]);
    }

    #[test]
    fn overlap_detection_names_the_object_and_a_free_spot_avoids_it() {
        let s = xl_shape();
        let b = bed(None);
        // The XL tower sits at 180/140: a cube under it overlaps, one beside it does not.
        let under = cube("3DBenchy.stl", 0, 0, [170., 130.]);
        let beside = cube("cube20.stl", 1, 1, [40., 40.]);
        assert_eq!(
            overlapping(&s, &[under.clone(), beside.clone()]),
            ["3DBenchy"]
        );
        assert_eq!(
            problems(&s, &b, std::slice::from_ref(&under)),
            [Problem::Overlaps("3DBenchy".into())]
        );
        assert_eq!(
            Problem::Overlaps("3DBenchy".into()).text(),
            "Overlaps 3DBenchy"
        );
        assert!(overlapping(&s, std::slice::from_ref(&beside)).is_empty());
        let p = free_spot(&s, &b, &[under, beside], 6.).unwrap();
        let moved = s.translated(p[0] - s.position[0], p[1] - s.position[1]);
        assert!(problems(&moved, &b, &[cube("3DBenchy.stl", 0, 0, [170., 130.])]).is_empty());
        let (min, max) = moved.bbox();
        assert!(min[0] >= MARGIN - 1e-6 && min[1] >= MARGIN - 1e-6);
        assert!(max[0] <= 360. - MARGIN + 1e-6 && max[1] <= 360. - MARGIN + 1e-6);
        // A bed smaller than the footprint has no spot.
        let mut tiny = Config::defaults();
        tiny.set_raw("bed_shape", "0x0,50x0,50x50,0x50").unwrap();
        assert!(free_spot(&s, &Bed::from_config(&tiny), &[], 6.).is_none());
    }

    #[test]
    fn picking_hits_the_body_and_the_brim_and_misses_beside_them() {
        let view = TowerView {
            plate: 0,
            origin: Vec2::ZERO,
            shape: xl_shape(),
            name: "Wipe tower",
            problems: vec![],
        };
        let down = Vec3::NEG_Z;
        // Through the body's centre from above.
        let c = view.centre();
        let t = view.pick(c.with_z(100.), down).unwrap();
        assert!((t - (100. - view.shape.height as f32)).abs() < 1e-3, "{t}");
        // Through the brim only: the slab is hit at its top.
        let (min, _) = view.bounds();
        let brim = Vec3::new(min.x + 0.5, c.y, 100.);
        let t = view.pick(brim, down).unwrap();
        assert!((t - (100. - FOOTPRINT_Z)).abs() < 1e-3, "{t}");
        assert!(view.pick(Vec3::new(min.x - 5., c.y, 100.), down).is_none());
        // A turned tower is picked in its own frame.
        let mut turned = view.clone();
        turned.shape.rotation = 90.;
        turned.shape.hull = turned.shape.body_corners().to_vec();
        let c = turned.centre();
        assert!(turned.pick(c.with_z(100.), down).is_some());
        let far = Vec3::new(c.x + 20., c.y, 100.);
        assert!(
            turned.pick(far, down).is_none(),
            "the long side now runs along Y"
        );
        assert_eq!(turned.footprint_mesh().faces.len(), 2 * 2 + 4 * 2);
        assert_ne!(turned.footprint_id(), view.footprint_id());
    }

    #[test]
    fn rotation_support_follows_the_tower_type() {
        let mut c = Config::defaults();
        assert!(rotation_supported(&c));
        c.set_raw("gcode_flavor", "klipper").unwrap();
        c.set_raw("mica_slicing_rules", "orcaslicer").unwrap();
        assert!(rotation_supported(&c));
        c.set_raw("wipe_tower_type", "type2").unwrap();
        assert!(rotation_supported(&c));
        c.set_raw("wipe_tower_type", "type1").unwrap();
        assert!(!rotation_supported(&c));
        assert_eq!(name_for(TowerKind::Orca), "Prime tower");
        assert_eq!(name_for(TowerKind::Prusa), "Wipe tower");
    }

    /// The XL with two cubes on two tools shows a wipe tower; dragging it is one undo entry
    /// per drag, it stays on the bed, the plate keeps its own position, and an arrange moves
    /// the objects away from it.
    #[test]
    fn tower_moves_with_undo_and_stays_per_plate() {
        use crate::viewport::Action;
        let ctx = egui::Context::default();
        let mut app = xl_app(&ctx);
        assert_eq!(app.tower_name(), "Wipe tower");
        assert!(app.tower().is_none(), "no objects, no tower");
        two_cubes(&mut app);
        let t = app.tower().expect("two tools print a tower").clone();
        assert_eq!(t.name, "Wipe tower");
        assert!(!t.shape.exact);
        assert_eq!(t.shape.position, [180., 140.]);
        assert!(t.problems.is_empty(), "{:?}", t.problems);
        // Select and drag: one gesture, one undo entry.
        app.select_tower();
        assert!(app.tower_selected && app.selected.is_none());
        let entries = app.undo.len();
        app.handle(Action::GestureBegin("Move"), &[], &ctx);
        for _ in 0..5 {
            app.handle(
                Action::TowerTransform(Delta::Translate(Vec3::new(-10., 4., 0.))),
                &[],
                &ctx,
            );
        }
        app.handle(Action::Edited, &[], &ctx);
        assert_eq!(app.undo.len(), entries + 1);
        assert_eq!(app.tower_placement(0), [130., 160., 0.]);
        assert_eq!(app.tower().unwrap().shape.position, [130., 160.]);
        app.undo();
        assert_eq!(app.tower_placement(0), [180., 140., 0.]);
        app.redo();
        assert_eq!(app.tower_placement(0), [130., 160., 0.]);
        // Dragging off the bed clamps the footprint to the edge.
        app.handle(Action::GestureBegin("Move"), &[], &ctx);
        app.handle(
            Action::TowerTransform(Delta::Translate(Vec3::new(1000., 0., 0.))),
            &[],
            &ctx,
        );
        app.handle(Action::Edited, &[], &ctx);
        let t = app.tower().unwrap().clone();
        assert!(
            (t.shape.bbox().1[0] - 360.).abs() < 1e-6,
            "{:?}",
            t.shape.bbox()
        );
        assert!(t.problems.is_empty());
        // A typed value and a nudge are undo steps too.
        app.set_tower_value(1, 50.);
        app.end_gesture();
        assert_eq!(app.tower_placement(0)[1], 50.);
        let n = app.undo.len();
        app.handle(Action::GestureBegin("Move"), &[], &ctx);
        app.handle(
            Action::TowerTransform(Delta::Translate(Vec3::new(0., 1., 0.))),
            &[],
            &ctx,
        );
        app.handle(Action::Edited, &[], &ctx);
        assert_eq!(app.undo.len(), n + 1);
        assert_eq!(app.tower_placement(0)[1], 51.);
        // Rotation keeps the body centre.
        let before = app.tower().unwrap().shape.centre();
        app.handle(Action::GestureBegin("Rotate"), &[], &ctx);
        app.handle(
            Action::TowerTransform(Delta::Rotate {
                axis: Vec3::Z,
                angle: std::f32::consts::FRAC_PI_2,
            }),
            &[],
            &ctx,
        );
        app.handle(Action::Edited, &[], &ctx);
        let after = app.tower().unwrap().shape.clone();
        assert!((after.rotation - 90.).abs() < 1e-4, "{}", after.rotation);
        let c = after.centre();
        assert!(
            (c[0] - before[0]).abs() < 1e-3 && (c[1] - before[1]).abs() < 1e-3,
            "{c:?} vs {before:?}"
        );
        app.set_tower_value(2, 0.);
        app.end_gesture();
        // A second plate has its own position.
        app.handle(Action::AddPlate, &[], &ctx);
        assert_eq!(app.plates.current, 1);
        assert!(app.tower().is_none(), "an empty plate prints nothing");
        two_cubes(&mut app);
        assert_eq!(app.tower_placement(1), [180., 140., 0.]);
        assert_ne!(app.tower_placement(0)[1], 140.);
        app.select_plate(0);
        assert_eq!(
            app.tower().unwrap().shape.position[1],
            app.tower_placement(0)[1]
        );
        // Overlap: the tower over a cube warns, blocks slicing and "free spot" resolves it.
        let (min, _) = app.plates.current().objects[0].bounds();
        app.set_tower_value(0, min.x as f64 - 5.);
        app.set_tower_value(1, min.y as f64 - 5.);
        app.end_gesture();
        let t = app.tower().unwrap().clone();
        assert_eq!(t.problems, [Problem::Overlaps("a".into())]);
        assert!(
            app.warnings.iter().any(|w| w == "Wipe tower overlaps a"),
            "{:?}",
            app.warnings
        );
        assert!(app.tower_blocks_slicing(0).unwrap().contains("overlaps a"));
        app.slice_current(&ctx);
        assert!(app.worker.is_none(), "slicing is blocked: {}", app.status);
        let error = app
            .plates
            .current()
            .slice_error
            .clone()
            .expect("the plate shows why");
        assert!(error.message.contains("overlaps a"), "{error:?}");
        app.run_tower_command(TowerCommand::FreeSpot);
        assert!(
            app.tower().unwrap().problems.is_empty(),
            "{:?}",
            app.tower().unwrap().problems
        );
        assert!(app.tower_blocks_slicing(0).is_none());
        // Arrange keeps the objects clear of the tower's real footprint.
        app.arrange();
        let t = app.tower().unwrap().clone();
        assert!(t.problems.is_empty(), "{:?}", t.problems);
        // The disable command switches the tower off in the process settings.
        app.select_tower();
        app.run_tower_command(TowerCommand::Disable);
        assert_eq!(
            app.overlays.print.get("wipe_tower").map(String::as_str),
            Some("0")
        );
        assert!(app.tower().is_none() && !app.tower_selected);
        app.undo();
        assert!(app.tower().is_some());
    }

    /// The slice puts the tower where the box stands: the sliced job's exact shape has the
    /// plate's placement, the same width and footprint X extents as the estimate, and the
    /// plate then draws the exact shape.
    #[test]
    fn sliced_tower_stands_where_the_box_was_drawn() {
        let ctx = egui::Context::default();
        let mut app = xl_app(&ctx);
        two_cubes(&mut app);
        app.set_tower_value(0, 100.);
        app.set_tower_value(1, 60.);
        app.end_gesture();
        let estimate = app.tower().unwrap().shape.clone();
        assert_eq!(estimate.position, [100., 60.]);
        let config = app.plate_config(0).unwrap();
        let objects = app.plates.current().objects.clone();
        let finished = crate::worker::slice(
            objects,
            config,
            &std::sync::atomic::AtomicBool::new(false),
            &|_| {},
        )
        .unwrap();
        let exact = finished.tower.clone().expect("two tools print a tower");
        assert!(exact.exact);
        assert_eq!(exact.position, [100., 60.]);
        assert_eq!(exact.width, estimate.width);
        let (emin, emax) = estimate.bbox();
        let (xmin, xmax) = exact.bbox();
        assert!(
            (emin[0] - xmin[0]).abs() < 0.1 && (emax[0] - xmax[0]).abs() < 0.1,
            "{emin:?} {xmin:?}"
        );
        assert!(
            (exact.depth - estimate.depth).abs() <= 0.15 * exact.depth,
            "{} vs {}",
            exact.depth,
            estimate.depth
        );
        app.plates.current_mut().finished = Some(finished);
        app.refresh_towers();
        assert!(app.tower().unwrap().shape.exact);
        assert_eq!(app.tower().unwrap().shape, exact);
        // A move drops the slice and goes back to the estimate.
        app.set_tower_value(0, 110.);
        app.end_gesture();
        assert!(!app.tower().unwrap().shape.exact);
        assert_eq!(app.tower().unwrap().shape.position, [110., 60.]);
    }

    /// Painted colours count as used tools: the colour-painted cube prints on one object tool
    /// but two filaments, so it gets a tower before slicing.
    #[test]
    fn painted_tools_give_the_estimate_a_tower() {
        let ctx = egui::Context::default();
        let mut app = xl_app(&ctx);
        let path = std::path::PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/models/painted-cube.3mf"
        ));
        let result = crate::app::load_files(vec![path], &app.config, app.bed.center());
        app.finish_load(result);
        assert_eq!(app.plates.current().objects.len(), 1, "{}", app.status);
        assert_eq!(app.plate_tools(0).len(), 2, "{:?}", app.plate_tools(0));
        assert!(app.tower().is_some());
    }

    /// An explicit `wipe_tower_x/y` edit in the settings is folded into the plates when the
    /// tower is moved, so the plate's position governs from then on.
    #[test]
    fn a_settings_edit_of_the_position_becomes_the_plates_placement() {
        let ctx = egui::Context::default();
        let mut app = xl_app(&ctx);
        two_cubes(&mut app);
        app.overlays
            .print
            .insert("wipe_tower_x".into(), "100".into());
        app.overlays
            .print
            .insert("wipe_tower_y".into(), "100".into());
        app.settings_changed();
        assert_eq!(app.tower().unwrap().shape.position, [100., 100.]);
        app.set_tower_value(0, 120.);
        app.end_gesture();
        assert!(!app.overlays.print.contains_key("wipe_tower_x"));
        assert_eq!(app.plates.plate(0).tower_placement, Some([120., 100., 0.]));
        assert_eq!(app.tower().unwrap().shape.position, [120., 100.]);
        let c = app.plate_config(0).unwrap();
        assert_eq!(mica_core::wipe_tower::number(&c, "wipe_tower_x", 0.), 120.);
    }
}
