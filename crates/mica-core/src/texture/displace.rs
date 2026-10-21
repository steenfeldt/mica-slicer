// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Textures on every surface ("All surfaces"): the part's mesh is refined where it is textured
//! and its vertices move along the surface normal by the texture, so tops, slopes and bottoms
//! carry the texture too. It runs while slicing and never changes the project: the displaced
//! mesh is kept in a cache (memory, and on disk when the app names a folder) keyed by the mesh,
//! the texture settings and the texture paint. Same projections, depths, paint slots, soft
//! borders and modifiers as the wall texture ([`crate::texture_skin`]), so a texture looks the
//! same on walls either way. Inspired by CNC Kitchen's BumpMesh (subdivide, then displace
//! along smooth normals with mitred creases); Mica's own implementation.
//!
//! - Refinement: every edge longer than the target length on a textured facet is split at its
//!   midpoint, pass after pass (one, two or three split edges per triangle). The decision is
//!   made per edge and the midpoint is shared, so the neighbour splits the same edge and no
//!   crack can open, also where a textured area meets an untextured one.
//! - Displacement: one value per welded vertex, along the mitre of the normals around it
//!   (facets meeting at more than 30° keep their own normals, as BumpMesh's normal groups), so
//!   a cube's faces each move by the depth and its edges stay sharp.
//! - Limits: a vertex never moves further than 45 % of the way to the surface facing it (thin
//!   walls, narrow gaps) and never below the bed; the refinement stops at a triangle budget.
use super::{
    Mode, Surfaces, TextureSettings,
    heightmap::HeightMap,
    projection::{Bounds, Projector},
};
use crate::config::Config;
use crate::mesh::TriangleMesh;
use crate::regions::RegionModel;
use crate::threemf::PartKind;
use rayon::prelude::*;
use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

/// The most triangles the textured meshes of one object may have; past it the refinement is
/// coarser (and the slice result says so).
pub const TRIANGLE_BUDGET: usize = 2_500_000;
/// Changes whenever the displaced mesh for the same input changes (cache entries of other
/// versions are ignored).
const VERSION: u32 = 2;
/// Walls closer to vertical than this (sine of 3°) are left to the wall texture while slicing
/// (see [`Rims`]): on a vertical wall moving the slice outline is the same as moving the mesh.
pub const WALL_SIN: f64 = 0.0523;
/// Facets meeting at a sharper angle than this keep separate normals (cos 30°).
const CREASE_COS: f64 = 0.866;
/// A vertex moves at most this share of the distance to the surface it moves towards.
const CLEARANCE: f64 = 0.45;
/// Disk cache size cap, bytes.
const DISK_CAP: u64 = 1 << 30;

/// The displaced meshes of one object.
#[derive(Debug)]
pub struct Displaced {
    /// The object's model volumes, displaced, in volume order (the object mesh for slicing).
    pub mesh: TriangleMesh,
    /// Per volume of the object's region model (one entry for a plain object): the displaced
    /// mesh of a textured model volume, `None` where the volume is sliced as it is.
    pub volumes: Vec<Option<TriangleMesh>>,
    /// What the slice result says (depth limited, budget reached).
    pub warnings: Vec<String>,
    /// Target edge length the refinement used, mm.
    pub edge: f64,
    /// Set when the steep walls were left to the wall texture (slicing only, never baked).
    pub walls: Option<Arc<Rims>>,
}

/// Where steep walls left to the wall texture meet surfaces textured in the mesh.
///
/// Slicing a refined, displaced wall costs far more than texturing the walls of a plain one,
/// and on a vertical wall both give the same surface. So while slicing, walls within 3° of
/// vertical ([`WALL_SIN`]) stay as loaded and the wall texture ([`crate::texture_skin`]) moves
/// their outer walls by the same texture function, while tops, slopes and bottoms are displaced
/// here. Near a rim (an edge between such a wall and a surface textured here) the wall is refined
/// and displaced in the mesh too, so the corner is mitred as before; over a band the mesh's
/// share hands over to the wall texture's: the shares add up to one everywhere.
#[derive(Debug)]
pub struct Rims {
    /// Edges, bed frame, mm.
    segs: Vec<[[f64; 3]; 2]>,
    /// The mesh carries the whole texture up to `near` from a rim, none from `near + blend`.
    pub near: f64,
    pub blend: f64,
    cell: f64,
    grid: HashMap<[i64; 3], Vec<u32>>,
}

impl PartialEq for Rims {
    fn eq(&self, o: &Self) -> bool {
        let bits = |v: &[[[f64; 3]; 2]]| {
            v.iter()
                .flat_map(|s| s.iter().flatten().map(|c| c.to_bits()))
                .collect::<Vec<_>>()
        };
        bits(&self.segs) == bits(&o.segs)
            && self.near.to_bits() == o.near.to_bits()
            && self.blend.to_bits() == o.blend.to_bits()
    }
}

fn segment_distance(p: [f64; 3], [a, b]: [[f64; 3]; 2]) -> f64 {
    let ab = sub(b, a);
    let l2 = dot(ab, ab);
    let t = if l2 > 0. {
        (dot(sub(p, a), ab) / l2).clamp(0., 1.)
    } else {
        0.
    };
    length(sub(p, add(a, scale(ab, t))))
}

impl Rims {
    pub(crate) fn new(segs: Vec<[[f64; 3]; 2]>, near: f64, blend: f64) -> Self {
        let cell = (near + blend).max(0.1);
        let mut grid: HashMap<[i64; 3], Vec<u32>> = HashMap::new();
        for (i, s) in segs.iter().enumerate() {
            let lo: [i64; 3] =
                std::array::from_fn(|k| (s[0][k].min(s[1][k]) / cell).floor() as i64);
            let hi: [i64; 3] =
                std::array::from_fn(|k| (s[0][k].max(s[1][k]) / cell).floor() as i64);
            for x in lo[0]..=hi[0] {
                for y in lo[1]..=hi[1] {
                    for z in lo[2]..=hi[2] {
                        grid.entry([x, y, z]).or_default().push(i as u32);
                    }
                }
            }
        }
        Self {
            segs,
            near,
            blend,
            cell,
            grid,
        }
    }

    /// Distance from `p` to the nearest rim, at most `near + blend`.
    pub fn distance(&self, p: [f64; 3]) -> f64 {
        let reach = self.near + self.blend;
        let c: [i64; 3] = std::array::from_fn(|k| (p[k] / self.cell).floor() as i64);
        let mut best = reach;
        for x in c[0] - 1..=c[0] + 1 {
            for y in c[1] - 1..=c[1] + 1 {
                for z in c[2] - 1..=c[2] + 1 {
                    for &i in self.grid.get(&[x, y, z]).into_iter().flatten() {
                        best = best.min(segment_distance(p, self.segs[i as usize]));
                    }
                }
            }
        }
        best
    }

    /// The wall texture's share of the texture at `p` on a steep wall (the mesh has the rest).
    pub fn wall_share(&self, p: [f64; 3]) -> f64 {
        smoothstep(self.near, self.near + self.blend, self.distance(p))
    }

    /// Whether a box (bed frame) comes within `margin` of a rim's band.
    fn touches(&self, lo: [f64; 3], hi: [f64; 3], margin: f64) -> bool {
        let r = self.near + self.blend + margin;
        let near = |s: &[[f64; 3]; 2]| {
            (0..3).all(|k| s[0][k].min(s[1][k]) <= hi[k] + r && s[0][k].max(s[1][k]) >= lo[k] - r)
        };
        let a: [i64; 3] = std::array::from_fn(|k| ((lo[k] - r) / self.cell).floor() as i64);
        let b: [i64; 3] = std::array::from_fn(|k| ((hi[k] + r) / self.cell).floor() as i64);
        let cells: i64 = (0..3).map(|k| b[k] - a[k] + 1).product();
        if cells > self.segs.len() as i64 {
            return self.segs.iter().any(near);
        }
        for x in a[0]..=b[0] {
            for y in a[1]..=b[1] {
                for z in a[2]..=b[2] {
                    for &i in self.grid.get(&[x, y, z]).into_iter().flatten() {
                        if near(&self.segs[i as usize]) {
                            return true;
                        }
                    }
                }
            }
        }
        false
    }
}

fn same_mesh(a: &TriangleMesh, b: &TriangleMesh) -> bool {
    a.faces == b.faces
        && a.vertices.len() == b.vertices.len()
        && a.vertices
            .iter()
            .zip(&b.vertices)
            .all(|(p, q)| p.map(f32::to_bits) == q.map(f32::to_bits))
}
/// Bit for bit.
impl PartialEq for Displaced {
    fn eq(&self, o: &Self) -> bool {
        same_mesh(&self.mesh, &o.mesh)
            && self.volumes.len() == o.volumes.len()
            && self
                .volumes
                .iter()
                .zip(&o.volumes)
                .all(|(a, b)| match (a, b) {
                    (Some(a), Some(b)) => same_mesh(a, b),
                    (None, None) => true,
                    _ => false,
                })
            && self.warnings == o.warnings
            && self.edge.to_bits() == o.edge.to_bits()
            && match (&self.walls, &o.walls) {
                (Some(a), Some(b)) => a == b,
                (None, None) => true,
                _ => false,
            }
    }
}

/// Whether `s` (a region's settings) puts a texture on all surfaces, from its own settings or
/// (with `painted`) from texture paint.
pub fn uses_all_surfaces(s: &TextureSettings, painted: bool) -> bool {
    let all = |v: Option<&String>| v.is_some_and(|v| v == "walls_and_tops");
    (s.surfaces == Surfaces::WallsAndTops && (s.enabled() || painted))
        || (painted
            && s.slots
                .values()
                .any(|o| all(o.get("mica_texture_skin_surfaces"))))
}

/// The texture settings of a chain of configurations (first wins), over the object's.
fn chain(configs: &[&Config]) -> Result<TextureSettings, String> {
    TextureSettings::from_lookup(|k| configs.iter().find_map(|c| c.raw(k)))
}

fn has_texture_keys(c: &Config) -> bool {
    super::KEYS
        .iter()
        .any(|k| c.get(k).is_some() || c.extra(k).is_some())
}

/// What [`for_object`] needs: the settings, the volumes, and where the texture is anchored.
struct Input<'a> {
    /// Model volumes to displace: (region volume index, mesh, part config).
    parts: Vec<(usize, &'a TriangleMesh, Option<&'a Config>)>,
    volume_count: usize,
    /// The object's own settings: resolved (plain object) or its configuration (regions).
    base: Base<'a>,
    ranges: Vec<(f64, f64, &'a Config)>,
    /// Modifiers that set texture options.
    modifiers: Vec<(&'a TriangleMesh, &'a Config)>,
    model: Option<&'a RegionModel>,
    bounds: Bounds,
    /// No texture below this height unless the settings texture the first layer.
    floor: f64,
    bed: f64,
    settings: &'a crate::settings::PrintSettings,
    /// Leave steep walls to the wall texture ([`Rims`]).
    walls: bool,
}
#[derive(Clone, Copy)]
enum Base<'a> {
    Settings(&'a TextureSettings),
    Config(&'a Config),
}

/// The displaced meshes of an object, when any of its settings puts a texture on all surfaces;
/// from the cache when the mesh, settings and paint are unchanged.
pub fn for_object(
    mesh: &TriangleMesh,
    s: &crate::settings::PrintSettings,
    model: Option<(&RegionModel, &Config)>,
    observer: &dyn crate::progress::Observer,
) -> Result<Option<Arc<Displaced>>, String> {
    for_object_with(mesh, s, model, observer, true)
}

/// [`for_object`]; `walls`: leave steep walls to the wall texture where it can ([`Rims`]).
fn for_object_with(
    mesh: &TriangleMesh,
    s: &crate::settings::PrintSettings,
    model: Option<(&RegionModel, &Config)>,
    observer: &dyn crate::progress::Observer,
    walls: bool,
) -> Result<Option<Arc<Displaced>>, String> {
    let Some(mut input) = plan(mesh, s, model)? else {
        return Ok(None);
    };
    input.walls = walls && walls_left_to_wall_texture(&input)?;
    let _timer = crate::timing::Span::new(crate::timing::Stage::TextureDisplace);
    let key = cache_key(&input);
    if let Some(hit) = cache_get(&key) {
        return Ok(Some(hit));
    }
    let out = Arc::new(displace(&input, observer)?);
    cache_put(&key, &out);
    Ok(Some(out))
}

/// What [`for_object`] works from, `None` when nothing has a texture on all surfaces.
fn plan<'a>(
    mesh: &'a TriangleMesh,
    s: &'a crate::settings::PrintSettings,
    model: Option<(&'a RegionModel, &'a Config)>,
) -> Result<Option<Input<'a>>, String> {
    let painted = model.is_some_and(|(m, _)| {
        m.volumes
            .iter()
            .any(|v| v.kind == PartKind::Model && !v.painting.texture.is_empty())
    });
    // Cheap test first: with textures off (the default) nothing below runs.
    let mut any = uses_all_surfaces(&s.texture_skin, painted);
    if let Some((m, c)) = model {
        let base = chain(&[c])?;
        any |= uses_all_surfaces(&base, painted);
        for cfg in m
            .volumes
            .iter()
            .map(|v| &v.config)
            .chain(m.height_ranges.iter().map(|r| &r.config))
            .filter(|c| has_texture_keys(c))
        {
            any |= uses_all_surfaces(&chain(&[cfg, c])?, painted);
        }
    }
    if !any {
        return Ok(None);
    }
    let (min, max) = mesh.bounds();
    let floor_height = if s.raft_layers > 0 {
        s.layer_height
    } else {
        s.first_layer_height
    };
    let input = match model {
        None => Input {
            parts: vec![(0, mesh, None)],
            volume_count: 1,
            base: Base::Settings(&s.texture_skin),
            ranges: Vec::new(),
            modifiers: Vec::new(),
            model: None,
            bounds: Bounds {
                min: min.map(f64::from),
                max: max.map(f64::from),
            },
            floor: f64::from(min[2]) + floor_height - 1e-6,
            bed: f64::from(min[2]),
            settings: s,
            walls: false,
        },
        Some((m, c)) => Input {
            parts: m
                .volumes
                .iter()
                .enumerate()
                .filter(|(_, v)| v.kind == PartKind::Model)
                .map(|(i, v)| (i, &v.mesh, Some(&v.config)))
                .collect(),
            volume_count: m.volumes.len(),
            base: Base::Config(c),
            ranges: m
                .height_ranges
                .iter()
                .map(|r| (r.min_z, r.max_z, &r.config))
                .collect(),
            modifiers: m
                .volumes
                .iter()
                .filter(|v| v.kind == PartKind::Modifier && has_texture_keys(&v.config))
                .map(|v| (&v.mesh, &v.config))
                .collect(),
            model: Some(m),
            bounds: Bounds {
                min: min.map(f64::from),
                max: max.map(f64::from),
            },
            floor: f64::from(min[2]) + floor_height - 1e-6,
            bed: f64::from(min[2]),
            settings: s,
            walls: false,
        },
    };
    Ok(Some(input))
}

/// Whether the wall texture can carry the steep walls exactly as the mesh would: Arachne
/// walls (classic walls cap outward depth at half a line), no part that changes the walls, and
/// no inward depth past the wall texture's cap (half the outer wall's spacing).
fn walls_left_to_wall_texture(input: &Input<'_>) -> Result<bool, String> {
    let s = input.settings;
    if s.perimeter_generator != crate::settings::PerimeterGenerator::Arachne {
        return Ok(false);
    }
    const WALL_KEYS: &[&str] = &[
        "perimeter_generator",
        "wall_generator",
        "external_perimeter_extrusion_width",
        "outer_wall_line_width",
        "extrusion_width",
        "line_width",
        "layer_height",
        "first_layer_height",
        "initial_layer_print_height",
        "nozzle_diameter",
    ];
    if let Some(m) = input.model {
        let changes_walls = |c: &Config| {
            WALL_KEYS
                .iter()
                .any(|k| c.get(k).is_some() || c.extra(k).is_some())
        };
        if m.volumes.iter().map(|v| &v.config).any(changes_walls)
            || m.height_ranges.iter().map(|r| &r.config).any(changes_walls)
        {
            return Ok(false);
        }
    }
    let cap = (0..2)
        .map(|i| {
            let h = if i == 0 {
                s.first_layer_height
            } else {
                s.layer_height
            };
            0.5 * crate::print::LayerFlows::new(s, i, h).external.spacing()
        })
        .fold(f64::INFINITY, f64::min);
    // The settings of every combination (no paint field or modifier solids needed for that).
    let painted = input.model.is_some_and(|m| {
        m.volumes
            .iter()
            .any(|v| v.kind == PartKind::Model && !v.painting.texture.is_empty())
    });
    let field = Field {
        input,
        paint: None,
        solids: Vec::new(),
        combos: Mutex::new(HashMap::new()),
    };
    for key in field.candidate_keys() {
        for a in Resolved::new(field.settings(&key)?, painted)?.acts() {
            let (_, inward) = a.s.direction.reach(a.s.amplitude);
            if inward > cap * 0.999 {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

// ---------------------------------------------------------------------------------------------
// Settings at a point.

/// One region's texture on all surfaces, ready to sample.
struct Act {
    s: TextureSettings,
    map: Arc<HeightMap>,
}
impl Act {
    fn new(s: TextureSettings) -> Result<Option<Self>, String> {
        if s.surfaces != Surfaces::WallsAndTops {
            return Ok(None);
        }
        let map = s.map()?;
        Ok(Some(Self { s, map }))
    }

    /// How much of the texture this surface direction gets (bottom limit, overhangs).
    fn fade(&self, n: [f64; 3], outward: bool) -> f64 {
        if n[2] >= 0. {
            return 1.;
        }
        let a = surface_angle(n);
        let mut f = if self.s.bottom_angle > 0. {
            smoothstep(self.s.bottom_angle, self.s.bottom_angle + 10., a)
        } else {
            1.
        };
        if outward && self.s.overhang_clamp {
            f *= smoothstep(35., 60., a);
        }
        f
    }

    /// Displacement (mm, positive outward) at `p` with unit normal `n`.
    fn offset(&self, bounds: Bounds, floor: f64, p: [f64; 3], n: [f64; 3]) -> f64 {
        if !self.s.first_layer && p[2] < floor {
            return 0.;
        }
        let g = Projector::new(&self.map, self.s.mapping, bounds).grey(p, n);
        let d = self.s.direction.offset(g, self.s.amplitude);
        d * self.fade(n, d > 0.)
    }

    /// Whether a facet with normal `n` reaching up to `top` can get any texture.
    fn may_texture(&self, n: [f64; 3], top: f64, floor: f64) -> bool {
        if !self.s.first_layer && top < floor {
            return false;
        }
        let (out, inn) = self.s.direction.reach(self.s.amplitude);
        (out > 0. && self.fade(n, true) > 0.) || (inn > 0. && self.fade(n, false) > 0.)
    }
}

/// A painted slot: its soft border, and its texture when it is on all surfaces.
struct SlotAct {
    falloff: f64,
    curve: super::Curve,
    act: Option<Act>,
}

/// The settings that apply at a point: the region's texture and its paint slots.
struct Resolved {
    base: Option<Act>,
    slots: Vec<Option<SlotAct>>,
}
impl Resolved {
    fn new(s: TextureSettings, painted: bool) -> Result<Self, String> {
        let slots = if painted {
            (0..16u8)
                .map(|n| {
                    if n == 0 {
                        return Ok(None);
                    }
                    let Ok(ss) = s.slot(n) else { return Ok(None) };
                    Ok(Some(SlotAct {
                        falloff: ss.falloff,
                        curve: ss.falloff_curve,
                        act: Act::new(ss)?,
                    }))
                })
                .collect::<Result<_, String>>()?
        } else {
            Vec::new()
        };
        let base = if s.mode != Mode::None {
            Act::new(s)?
        } else {
            None
        };
        Ok(Self { base, slots })
    }
    fn acts(&self) -> impl Iterator<Item = &Act> {
        self.base
            .iter()
            .chain(self.slots.iter().flatten().filter_map(|s| s.act.as_ref()))
    }
}

/// Which settings apply at a point: part, height range and modifiers containing it.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct ComboKey {
    part: usize,
    range: Option<usize>,
    modifiers: Vec<usize>,
}

/// A closed mesh for inside tests (vertical ray parity), gridded in XY.
struct Solid {
    tris: Vec<[[f64; 3]; 3]>,
    min: [f64; 3],
    max: [f64; 3],
    cell: f64,
    nx: usize,
    ny: usize,
    cells: Vec<Vec<u32>>,
}
impl Solid {
    fn new(mesh: &TriangleMesh) -> Self {
        let tris: Vec<[[f64; 3]; 3]> = mesh
            .faces
            .iter()
            .map(|f| f.map(|i| mesh.vertices[i as usize].map(f64::from)))
            .collect();
        let (lo, hi) = mesh.bounds();
        let (min, max) = (lo.map(f64::from), hi.map(f64::from));
        let span = (max[0] - min[0]).max(max[1] - min[1]).max(1e-3);
        let cell = (span / 64.).max(0.5);
        let nx = ((max[0] - min[0]) / cell) as usize + 1;
        let ny = ((max[1] - min[1]) / cell) as usize + 1;
        let mut cells = vec![Vec::new(); nx * ny];
        for (i, t) in tris.iter().enumerate() {
            let lx = t.iter().map(|p| p[0]).fold(f64::INFINITY, f64::min);
            let hx = t.iter().map(|p| p[0]).fold(f64::NEG_INFINITY, f64::max);
            let ly = t.iter().map(|p| p[1]).fold(f64::INFINITY, f64::min);
            let hy = t.iter().map(|p| p[1]).fold(f64::NEG_INFINITY, f64::max);
            let cx = |x: f64| (((x - min[0]) / cell) as usize).min(nx - 1);
            let cy = |y: f64| (((y - min[1]) / cell) as usize).min(ny - 1);
            for y in cy(ly)..=cy(hy) {
                for x in cx(lx)..=cx(hx) {
                    cells[y * nx + x].push(i as u32);
                }
            }
        }
        Self {
            tris,
            min,
            max,
            cell,
            nx,
            ny,
            cells,
        }
    }
    fn overlaps(&self, lo: [f64; 3], hi: [f64; 3]) -> bool {
        (0..3).all(|k| lo[k] <= self.max[k] + 1e-6 && hi[k] >= self.min[k] - 1e-6)
    }
    fn contains(&self, p: [f64; 3]) -> bool {
        if (0..3).any(|k| p[k] < self.min[k] || p[k] > self.max[k]) {
            return false;
        }
        // A slightly skewed vertical ray, so it does not run exactly through shared edges.
        let (x, y) = (p[0] + 1.37e-7, p[1] + 0.91e-7);
        let cx = (((x - self.min[0]) / self.cell) as usize).min(self.nx - 1);
        let cy = (((y - self.min[1]) / self.cell) as usize).min(self.ny - 1);
        let mut inside = false;
        for &i in &self.cells[cy * self.nx + cx] {
            let [a, b, c] = self.tris[i as usize];
            let d = (b[0] - a[0]) * (c[1] - a[1]) - (c[0] - a[0]) * (b[1] - a[1]);
            if d.abs() < 1e-18 {
                continue;
            }
            let u = ((x - a[0]) * (c[1] - a[1]) - (c[0] - a[0]) * (y - a[1])) / d;
            let v = ((b[0] - a[0]) * (y - a[1]) - (x - a[0]) * (b[1] - a[1])) / d;
            if u < 0. || v < 0. || u + v > 1. {
                continue;
            }
            let z = a[2] + u * (b[2] - a[2]) + v * (c[2] - a[2]);
            if z > p[2] {
                inside = !inside;
            }
        }
        inside
    }
}

/// Everything that decides the texture at a point of the object.
struct Field<'a> {
    input: &'a Input<'a>,
    paint: Option<crate::texture_skin::PaintField>,
    solids: Vec<Solid>,
    combos: Mutex<HashMap<ComboKey, Arc<Resolved>>>,
}
impl<'a> Field<'a> {
    fn new(input: &'a Input<'a>) -> Result<Self, String> {
        let painted = input.model.is_some_and(|m| {
            m.volumes
                .iter()
                .any(|v| v.kind == PartKind::Model && !v.painting.texture.is_empty())
        });
        let mut f = Self {
            input,
            paint: None,
            solids: input.modifiers.iter().map(|(m, _)| Solid::new(m)).collect(),
            combos: Mutex::new(HashMap::new()),
        };
        if painted {
            // The widest soft border of any slot, as the wall texture does.
            let mut falloff: f64 = 0.;
            for key in f.candidate_keys() {
                let s = f.settings(&key)?;
                falloff = falloff.max(s.falloff);
                for &n in s.slots.keys() {
                    if let Ok(ss) = s.slot(n) {
                        falloff = falloff.max(ss.falloff);
                    }
                }
            }
            f.paint = input
                .model
                .and_then(|m| crate::texture_skin::PaintField::new(m, falloff.max(0.5)));
        }
        Ok(f)
    }

    /// The texture settings of one combination.
    fn settings(&self, key: &ComboKey) -> Result<TextureSettings, String> {
        match self.input.base {
            Base::Settings(s) => Ok(s.clone()),
            Base::Config(c) => {
                let mut list: Vec<&Config> = key
                    .modifiers
                    .iter()
                    .rev()
                    .map(|&i| self.input.modifiers[i].1)
                    .collect();
                if let Some(part) = self.input.parts[key.part].2 {
                    list.push(part);
                }
                if let Some(r) = key.range {
                    list.push(self.input.ranges[r].2);
                }
                list.push(c);
                chain(&list)
            }
        }
    }

    fn resolved(&self, key: &ComboKey) -> Result<Arc<Resolved>, String> {
        if let Some(r) = self
            .combos
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(key)
        {
            return Ok(r.clone());
        }
        let r = Arc::new(Resolved::new(self.settings(key)?, self.paint.is_some())?);
        self.combos
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key.clone(), r.clone());
        Ok(r)
    }

    /// Every part × range × single-modifier combination (for the facet test).
    fn candidate_keys(&self) -> Vec<ComboKey> {
        let mut out = Vec::new();
        for part in 0..self.input.parts.len() {
            for range in std::iter::once(None).chain((0..self.input.ranges.len()).map(Some)) {
                for m in std::iter::once(None).chain((0..self.input.modifiers.len()).map(Some)) {
                    out.push(ComboKey {
                        part,
                        range,
                        modifiers: m.into_iter().collect(),
                    });
                }
            }
        }
        out
    }

    fn key_at(&self, part: usize, p: [f64; 3]) -> ComboKey {
        ComboKey {
            part,
            range: self
                .input
                .ranges
                .iter()
                .position(|r| p[2] >= r.0 && p[2] < r.1),
            modifiers: self
                .solids
                .iter()
                .enumerate()
                .filter(|(_, s)| s.contains(p))
                .map(|(i, _)| i)
                .collect(),
        }
    }

    /// Displacement (mm, positive outward) at `p` with unit normal `n`, as the wall texture
    /// computes it: the region's texture, blended into a painted slot's over its soft border.
    fn value(&self, r: &Resolved, p: [f64; 3], n: [f64; 3]) -> f64 {
        let (bounds, floor) = (self.input.bounds, self.input.floor);
        let mut d = r
            .base
            .as_ref()
            .map_or(0., |a| a.offset(bounds, floor, p, n));
        if let Some(paint) = &self.paint {
            let slot = paint.slot(p, 0.01);
            if let Some(Some(sa)) = r.slots.get(slot as usize).filter(|_| slot > 0) {
                let w = if sa.falloff > 0. {
                    sa.curve
                        .apply(paint.edge_distance(p, sa.falloff) / sa.falloff)
                } else {
                    1.
                };
                let v = sa
                    .act
                    .as_ref()
                    .map_or(0., |a| a.offset(bounds, floor, p, n));
                d = d * (1. - w) + v * w;
            }
        }
        d
    }
}

fn surface_angle(n: [f64; 3]) -> f64 {
    n[2].abs().clamp(0., 1.).acos().to_degrees()
}
fn smoothstep(a: f64, b: f64, x: f64) -> f64 {
    let t = ((x - a) / (b - a)).clamp(0., 1.);
    t * t * (3. - 2. * t)
}

// ---------------------------------------------------------------------------------------------
// Vector helpers.

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn scale(a: [f64; 3], k: f64) -> [f64; 3] {
    [a[0] * k, a[1] * k, a[2] * k]
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn length(a: [f64; 3]) -> f64 {
    dot(a, a).sqrt()
}
fn unit(a: [f64; 3]) -> [f64; 3] {
    let l = length(a);
    if l > 1e-30 { scale(a, 1. / l) } else { a }
}

// ---------------------------------------------------------------------------------------------
// The refined mesh.

/// A welded triangle mesh with per-corner smooth normals and a "textured" flag per facet.
struct Work {
    pos: Vec<[f64; 3]>,
    faces: Vec<[u32; 3]>,
    corners: Vec<[[f64; 3]; 3]>,
    textured: Vec<bool>,
    /// A steep wall left to the wall texture (textured here only near a rim).
    wall: Vec<bool>,
}

/// Welds `mesh` by position and gives every facet corner its smooth normal (facets within
/// 30° of each other share one; sharper creases keep their own).
fn weld(mesh: &TriangleMesh) -> Work {
    let mut ids: HashMap<[i64; 3], u32> = HashMap::new();
    let mut pos = Vec::new();
    let welded: Vec<u32> = mesh
        .vertices
        .iter()
        .map(|v| {
            let key = v.map(|c| (f64::from(c) * 1e4).round() as i64);
            *ids.entry(key).or_insert_with(|| {
                pos.push(v.map(f64::from));
                (pos.len() - 1) as u32
            })
        })
        .collect();
    let faces: Vec<[u32; 3]> = mesh
        .faces
        .iter()
        .map(|f| f.map(|i| welded[i as usize]))
        .filter(|f| f[0] != f[1] && f[1] != f[2] && f[0] != f[2])
        .collect();
    let normals: Vec<[f64; 3]> = faces
        .iter()
        .map(|f| {
            let [a, b, c] = f.map(|i| pos[i as usize]);
            cross(sub(b, a), sub(c, a))
        })
        .collect();
    let mut start = vec![0u32; pos.len() + 1];
    for f in &faces {
        for &v in f {
            start[v as usize + 1] += 1;
        }
    }
    for i in 1..start.len() {
        start[i] += start[i - 1];
    }
    let mut fill = start.clone();
    let mut around = vec![0u32; *start.last().unwrap_or(&0) as usize];
    for (fi, f) in faces.iter().enumerate() {
        for &v in f {
            around[fill[v as usize] as usize] = fi as u32;
            fill[v as usize] += 1;
        }
    }
    let corners = faces
        .iter()
        .enumerate()
        .map(|(fi, f)| {
            let own = unit(normals[fi]);
            f.map(|v| {
                let mut acc = [0.; 3];
                for &g in &around[start[v as usize] as usize..start[v as usize + 1] as usize] {
                    let n = normals[g as usize];
                    if dot(unit(n), own) >= CREASE_COS {
                        acc = add(acc, n);
                    }
                }
                if length(acc) > 1e-30 { unit(acc) } else { own }
            })
        })
        .collect();
    let textured = vec![false; faces.len()];
    let wall = vec![false; faces.len()];
    Work {
        pos,
        faces,
        corners,
        textured,
        wall,
    }
}

fn area(w: &Work, f: usize) -> f64 {
    let [a, b, c] = w.faces[f].map(|i| w.pos[i as usize]);
    0.5 * length(cross(sub(b, a), sub(c, a)))
}

/// How a refinement pass ended.
#[derive(PartialEq)]
enum Pass {
    Split,
    Done,
    /// The budget is reached: only the longest edges were split.
    Budget,
}

/// One refinement pass: the longest edge of every textured facet, when longer than `edge`, is
/// split at its midpoint, which the facet on the other side of the edge shares (longest-edge
/// bisection keeps the triangles well shaped).
fn refine_pass(w: &mut Work, edge: f64, budget: usize) -> Pass {
    let limit = edge * edge;
    let len2 = |w: &Work, a: u32, b: u32| {
        let d = sub(w.pos[a as usize], w.pos[b as usize]);
        dot(d, d)
    };
    let mut marked: Vec<(f64, (u32, u32))> = (0..w.faces.len())
        .into_par_iter()
        .filter(|&fi| w.textured[fi])
        .filter_map(|fi| {
            let f = w.faces[fi];
            let (k, l) = (0..3)
                .map(|k| (k, len2(w, f[k], f[(k + 1) % 3])))
                .fold((0, -1.), |best, x| if x.1 > best.1 { x } else { best });
            (l > limit).then(|| {
                let (a, b) = (f[k], f[(k + 1) % 3]);
                (l, (a.min(b), a.max(b)))
            })
        })
        .collect();
    if marked.is_empty() {
        return Pass::Done;
    }
    // Each split edge adds two triangles (one on each side). Past the budget only the longest
    // edges are split, so the detail stays even.
    let mut result = Pass::Split;
    let room = budget.saturating_sub(w.faces.len()) / 2;
    if marked.len() > room {
        marked.par_sort_unstable_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        marked.truncate(room);
        result = Pass::Budget;
        if marked.is_empty() {
            return result;
        }
    }
    // The split edges, sorted: a midpoint's index follows its edge's place (the same for any
    // number of threads).
    let mut edges: Vec<(u32, u32)> = marked.into_iter().map(|m| m.1).collect();
    edges.par_sort_unstable();
    edges.dedup();
    let first = w.pos.len() as u32;
    let mids_pos: Vec<[f64; 3]> = edges
        .par_iter()
        .map(|&(a, b)| scale(add(w.pos[a as usize], w.pos[b as usize]), 0.5))
        .collect();
    w.pos.extend(mids_pos);
    let mid = |a: u32, b: u32| {
        edges
            .binary_search(&(a.min(b), a.max(b)))
            .ok()
            .map(|i| first + i as u32)
    };
    type Piece = ([u32; 3], [[f64; 3]; 3], bool, bool);
    // Split in chunks of faces, in parallel, then joined in order.
    const CHUNK: usize = 4096;
    let n = w.faces.len();
    let pieces: Vec<Vec<Piece>> = (0..n.div_ceil(CHUNK))
        .into_par_iter()
        .map(|chunk| {
            let mut out: Vec<Piece> = Vec::with_capacity(CHUNK * 2);
            for fi in chunk * CHUNK..((chunk + 1) * CHUNK).min(n) {
                let f = w.faces[fi];
                let c = w.corners[fi];
                let t = w.textured[fi];
                let wl = w.wall[fi];
                let m: [Option<u32>; 3] = std::array::from_fn(|k| mid(f[k], f[(k + 1) % 3]));
                let mn: [[f64; 3]; 3] = std::array::from_fn(|k| unit(add(c[k], c[(k + 1) % 3])));
                let mut push = |v: [u32; 3], n: [[f64; 3]; 3]| out.push((v, n, t, wl));
                match m.iter().filter(|x| x.is_some()).count() {
                    0 => push(f, c),
                    1 => {
                        // Rotate so the split edge is 0 → 1.
                        let k = m.iter().position(Option::is_some).unwrap();
                        let (a, b, cc) = (k, (k + 1) % 3, (k + 2) % 3);
                        let mid = m[k].unwrap();
                        push([f[a], mid, f[cc]], [c[a], mn[k], c[cc]]);
                        push([mid, f[b], f[cc]], [mn[k], c[b], c[cc]]);
                    }
                    2 => {
                        // The unsplit edge is k → k+1; the split ones are k+1 → k+2 and k+2 → k.
                        let k = m.iter().position(Option::is_none).unwrap();
                        let (a, b, cc) = (k, (k + 1) % 3, (k + 2) % 3);
                        let (m1, m2) = (m[b].unwrap(), m[cc].unwrap());
                        let (n1, n2) = (mn[b], mn[cc]);
                        // The corner triangle at cc, then the quad a, b, m1, m2 cut along its
                        // shorter diagonal.
                        push([m1, f[cc], m2], [n1, c[cc], n2]);
                        let d1 = sub(w.pos[f[a] as usize], w.pos[m1 as usize]);
                        let d2 = sub(w.pos[f[b] as usize], w.pos[m2 as usize]);
                        if dot(d1, d1) <= dot(d2, d2) {
                            push([f[a], f[b], m1], [c[a], c[b], n1]);
                            push([f[a], m1, m2], [c[a], n1, n2]);
                        } else {
                            push([f[a], f[b], m2], [c[a], c[b], n2]);
                            push([f[b], m1, m2], [c[b], n1, n2]);
                        }
                    }
                    _ => {
                        let (m0, m1, m2) = (m[0].unwrap(), m[1].unwrap(), m[2].unwrap());
                        push([f[0], m0, m2], [c[0], mn[0], mn[2]]);
                        push([m0, f[1], m1], [mn[0], c[1], mn[1]]);
                        push([m2, m1, f[2]], [mn[2], mn[1], c[2]]);
                        push([m0, m1, m2], [mn[0], mn[1], mn[2]]);
                    }
                }
            }
            out
        })
        .collect();
    let total: usize = pieces.iter().map(|p| p.len()).sum();
    let mut faces = Vec::with_capacity(total);
    let mut corners = Vec::with_capacity(total);
    let mut textured = Vec::with_capacity(total);
    let mut wall = Vec::with_capacity(total);
    for (v, n, t, wl) in pieces.into_iter().flatten() {
        faces.push(v);
        corners.push(n);
        textured.push(t);
        wall.push(wl);
    }
    w.faces = faces;
    w.corners = corners;
    w.textured = textured;
    w.wall = wall;
    result
}

/// A node: bounds, the triangle range it covers and its children.
type RayNode = ([f64; 3], [f64; 3], usize, usize, Option<(usize, usize)>);

/// A bounding volume tree of triangles for "how far to the next surface" rays.
struct RayTree {
    tris: Vec<[[f64; 3]; 3]>,
    /// (min, max, start, end, children)
    nodes: Vec<RayNode>,
}
impl RayTree {
    fn new(mut tris: Vec<[[f64; 3]; 3]>) -> Self {
        let mut nodes = Vec::new();
        if !tris.is_empty() {
            let n = tris.len();
            Self::build(&mut tris, &mut nodes, 0, n);
        }
        Self { tris, nodes }
    }
    fn build(
        tris: &mut [[[f64; 3]; 3]],
        nodes: &mut Vec<RayNode>,
        start: usize,
        end: usize,
    ) -> usize {
        let mut lo = [f64::INFINITY; 3];
        let mut hi = [f64::NEG_INFINITY; 3];
        for t in &tris[start..end] {
            for p in t {
                for k in 0..3 {
                    lo[k] = lo[k].min(p[k]);
                    hi[k] = hi[k].max(p[k]);
                }
            }
        }
        let index = nodes.len();
        nodes.push((lo, hi, start, end, None));
        if end - start > 6 {
            let axis = (0..3)
                .max_by(|&a, &b| (hi[a] - lo[a]).total_cmp(&(hi[b] - lo[b])))
                .unwrap_or(0);
            let mid = (start + end) / 2;
            tris[start..end].select_nth_unstable_by(mid - start, |a, b| {
                let c = |t: &[[f64; 3]; 3]| t[0][axis] + t[1][axis] + t[2][axis];
                c(a).total_cmp(&c(b))
            });
            let l = Self::build(tris, nodes, start, mid);
            let r = Self::build(tris, nodes, mid, end);
            nodes[index].4 = Some((l, r));
        }
        index
    }
    /// Distance along unit `dir` from `o` to the first surface further than `skip` (up to `max`).
    fn hit(&self, o: [f64; 3], dir: [f64; 3], skip: f64, max: f64) -> Option<f64> {
        if self.nodes.is_empty() {
            return None;
        }
        let inv = dir.map(|d| if d.abs() > 1e-300 { 1. / d } else { 1e300 });
        let mut best = max;
        let mut found = false;
        let mut stack = vec![0usize];
        while let Some(i) = stack.pop() {
            let (lo, hi, start, end, children) = self.nodes[i];
            let (mut t0, mut t1) = (0f64, best);
            let mut miss = false;
            for k in 0..3 {
                let (a, b) = ((lo[k] - o[k]) * inv[k], (hi[k] - o[k]) * inv[k]);
                t0 = t0.max(a.min(b));
                t1 = t1.min(a.max(b));
                if t0 > t1 {
                    miss = true;
                    break;
                }
            }
            if miss {
                continue;
            }
            if let Some((l, r)) = children {
                stack.extend([l, r]);
                continue;
            }
            for t in &self.tris[start..end] {
                // Möller–Trumbore.
                let e1 = sub(t[1], t[0]);
                let e2 = sub(t[2], t[0]);
                let pv = cross(dir, e2);
                let det = dot(e1, pv);
                if det.abs() < 1e-14 {
                    continue;
                }
                let s = sub(o, t[0]);
                let u = dot(s, pv) / det;
                if !(0. ..=1.).contains(&u) {
                    continue;
                }
                let q = cross(s, e1);
                let v = dot(dir, q) / det;
                if v < 0. || u + v > 1. {
                    continue;
                }
                let d = dot(e2, q) / det;
                if d > skip && d < best {
                    best = d;
                    found = true;
                }
            }
        }
        found.then_some(best)
    }
}

/// The direction that moves every facet plane around a vertex by one unit along its own
/// normal (the crease mitre), at most twice as long as a normal.
#[cfg(test)]
fn mitre(groups: &[[f64; 3]]) -> [f64; 3] {
    mitre_to(groups, &vec![1.; groups.len()])
}

/// The direction that moves the facet plane of each normal group by its target (one for a
/// surface textured here, the mesh's share near a rim for a wall left to the wall texture),
/// at most twice as long as a normal.
fn mitre_to(groups: &[[f64; 3]], targets: &[f64]) -> [f64; 3] {
    let m = match groups {
        [] => return [0.; 3],
        [n] => scale(*n, targets[0]),
        [a, b] => {
            let (ta, tb) = (targets[0], targets[1]);
            let c = dot(*a, *b);
            if 1. + c < 1e-6 {
                return scale(unit(add(*a, *b)), ta.max(tb));
            }
            if ta == tb {
                scale(add(*a, *b), ta / (1. + c))
            } else {
                // m = α a + β b with a·m = ta, b·m = tb.
                let k = 1. - c * c;
                add(scale(*a, (ta - c * tb) / k), scale(*b, (tb - c * ta) / k))
            }
        }
        _ => {
            // Least squares: (Gᵀ G) m = Gᵀ t.
            let mut a = [[0f64; 3]; 3];
            let mut r = [0f64; 3];
            for (g, &t) in groups.iter().zip(targets) {
                for i in 0..3 {
                    r[i] += g[i] * t;
                    for j in 0..3 {
                        a[i][j] += g[i] * g[j];
                    }
                }
            }
            let det = a[0][0] * (a[1][1] * a[2][2] - a[1][2] * a[2][1])
                - a[0][1] * (a[1][0] * a[2][2] - a[1][2] * a[2][0])
                + a[0][2] * (a[1][0] * a[2][1] - a[1][1] * a[2][0]);
            if det.abs() < 1e-6 {
                let avg = unit(groups.iter().fold([0.; 3], |s, g| add(s, *g)));
                let c = groups
                    .iter()
                    .map(|g| dot(*g, avg))
                    .fold(f64::INFINITY, f64::min);
                let t = targets.iter().sum::<f64>() / targets.len() as f64;
                scale(if c < 1e-3 { avg } else { scale(avg, 1. / c) }, t)
            } else {
                let col = |k: usize| {
                    let mut m = a;
                    for i in 0..3 {
                        m[i][k] = r[i];
                    }
                    m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
                        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
                        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
                };
                [col(0) / det, col(1) / det, col(2) / det]
            }
        }
    };
    let l = length(m);
    if l > 2. { scale(m, 2. / l) } else { m }
}

/// Moves every vertex of the refined part by the texture. Returns the vertices, how many were
/// limited so the surface does not run into itself, and how many moved.
fn displace_vertices(
    w: &Work,
    field: &Field<'_>,
    part: usize,
    rays: &RayTree,
    rims: Option<&Rims>,
) -> Result<(Vec<[f32; 3]>, usize, usize), String> {
    let nv = w.pos.len();
    let mut start = vec![0u32; nv + 1];
    for f in &w.faces {
        for &v in f {
            start[v as usize + 1] += 1;
        }
    }
    for i in 1..start.len() {
        start[i] += start[i - 1];
    }
    let mut fill = start.clone();
    let mut around = vec![(0u32, 0u8); *start.last().unwrap_or(&0) as usize];
    for (fi, f) in w.faces.iter().enumerate() {
        for (k, &v) in f.iter().enumerate() {
            around[fill[v as usize] as usize] = (fi as u32, k as u8);
            fill[v as usize] += 1;
        }
    }
    // Which settings apply at each vertex that a textured facet touches.
    let touched: Vec<bool> = (0..nv)
        .into_par_iter()
        .map(|v| {
            around[start[v] as usize..start[v + 1] as usize]
                .iter()
                .any(|&(f, _)| w.textured[f as usize])
        })
        .collect();
    let keys: Vec<Option<ComboKey>> = (0..nv)
        .into_par_iter()
        .map(|v| touched[v].then(|| field.key_at(part, w.pos[v])))
        .collect();
    let mut unique: BTreeSet<&ComboKey> = BTreeSet::new();
    unique.extend(keys.iter().flatten());
    let mut resolved: HashMap<&ComboKey, Arc<Resolved>> = HashMap::new();
    for k in unique {
        resolved.insert(k, field.resolved(k)?);
    }
    let floor_z = field.input.bed;
    let moved: Vec<([f32; 3], bool, bool)> = (0..nv)
        .into_par_iter()
        .map(|v| {
            let p = w.pos[v];
            let keep = (p.map(|c| c as f32), false, false);
            let Some(key) = &keys[v] else { return keep };
            let r = &resolved[key];
            // Normal groups around the vertex, area weighted.
            let mut groups: Vec<[f64; 3]> = Vec::new();
            let mut sums: Vec<[f64; 3]> = Vec::new();
            // Whether every facet of the group is a wall left to the wall texture.
            let mut walls: Vec<bool> = Vec::new();
            let mut all = [0.; 3];
            for &(f, k) in &around[start[v] as usize..start[v + 1] as usize] {
                let n = w.corners[f as usize][k as usize];
                let a = area(w, f as usize).max(1e-12);
                let wall = w.wall[f as usize];
                all = add(all, scale(n, a));
                match groups.iter().position(|g| dot(*g, n) >= CREASE_COS) {
                    Some(i) => {
                        sums[i] = add(sums[i], scale(n, a));
                        groups[i] = unit(sums[i]);
                        walls[i] &= wall;
                    }
                    None => {
                        groups.push(n);
                        sums.push(scale(n, a));
                        walls.push(wall);
                    }
                }
            }
            let n = unit(all);
            if length(n) < 0.5 {
                return keep;
            }
            let d = field.value(r, p, n);
            if d == 0. || !d.is_finite() {
                return keep;
            }
            // A wall left to the wall texture moves by the mesh's share only (none away from
            // the rims), the other surfaces by the whole texture.
            let share = match rims {
                Some(r) if walls.iter().any(|&w| w) => 1. - r.wall_share(p),
                _ => 1.,
            };
            let targets: Vec<f64> = walls.iter().map(|&w| if w { share } else { 1. }).collect();
            let m = mitre_to(&groups, &targets);
            let mut delta = scale(m, d);
            let reach = length(delta);
            let mut limited = false;
            if reach > 0.05 {
                let dir = scale(delta, 1. / reach);
                if let Some(t) = rays.hit(p, dir, 1e-4, reach / CLEARANCE + 1e-3) {
                    let allowed = CLEARANCE * t;
                    if reach > allowed {
                        delta = scale(dir, allowed);
                        limited = true;
                    }
                }
            }
            let mut q = add(p, delta);
            if q[2] < floor_z {
                q[2] = floor_z;
            }
            (q.map(|c| c as f32), limited, true)
        })
        .collect();
    let limited = moved.iter().filter(|m| m.1).count();
    let count = moved.iter().filter(|m| m.2).count();
    Ok((moved.into_iter().map(|m| m.0).collect(), limited, count))
}

/// Refines and displaces every textured volume of the object.
fn displace(
    input: &Input<'_>,
    observer: &dyn crate::progress::Observer,
) -> Result<Displaced, String> {
    let field = Field::new(input)?;
    // Every texture that can reach a facet (for the facet test) and the finest point distance.
    let keys = field.candidate_keys();
    // Per part: (height range, modifier, settings) of every combination.
    type Candidates = Vec<(Option<usize>, Option<usize>, Arc<Resolved>)>;
    let mut by_part: Vec<Candidates> = vec![Vec::new(); input.parts.len()];
    let mut edge = f64::INFINITY;
    for k in &keys {
        let r = field.resolved(k)?;
        for a in r.acts() {
            edge = edge.min(a.s.point_distance);
        }
        by_part[k.part].push((k.range, k.modifiers.first().copied(), r));
    }
    // Refined edges end up between half and the whole split length: splitting at 4/3 of the
    // point distance gives edges of about the point distance.
    let edge = if edge.is_finite() {
        edge.max(0.05)
    } else {
        0.25
    } * 4.
        / 3.;
    let painted_faces = |part: usize| -> Vec<bool> {
        let (vi, mesh, _) = input.parts[part];
        let mut out = vec![false; mesh.faces.len()];
        if let Some(m) = input.model {
            for (t, _) in m.volumes[vi].painting.texture.entries() {
                if let Some(o) = out.get_mut(t as usize) {
                    *o = true;
                }
            }
        }
        out
    };
    let mut works = Vec::with_capacity(input.parts.len());
    let mut textured_area = 0.;
    // Edges between a steep wall left to the wall texture and a surface textured here.
    let mut rim_segs: Vec<[[f64; 3]; 2]> = Vec::new();
    for (part, &(_, mesh, _)) in input.parts.iter().enumerate() {
        crate::progress::check(observer)?;
        // Facet flags are decided on the mesh as loaded, then carried through the weld.
        let painted = painted_faces(part);
        let mut flags = Vec::with_capacity(mesh.faces.len());
        let mut walls = Vec::with_capacity(mesh.faces.len());
        for (fi, f) in mesh.faces.iter().enumerate() {
            let t = f.map(|i| mesh.vertices[i as usize].map(f64::from));
            let n = unit(cross(sub(t[1], t[0]), sub(t[2], t[0])));
            let lo: [f64; 3] = std::array::from_fn(|k| t[0][k].min(t[1][k]).min(t[2][k]));
            let hi: [f64; 3] = std::array::from_fn(|k| t[0][k].max(t[1][k]).max(t[2][k]));
            let floor = input.floor;
            let flag = by_part[part].iter().any(|(range, modifier, r)| {
                let in_range = range.is_none_or(|i| {
                    let (a, b, _) = input.ranges[i];
                    hi[2] >= a && lo[2] < b
                });
                let in_modifier = modifier.is_none_or(|i| field.solids[i].overlaps(lo, hi));
                if !in_range || !in_modifier {
                    return false;
                }
                let base = r
                    .base
                    .as_ref()
                    .is_some_and(|a| a.may_texture(n, hi[2], floor));
                let slot = painted[fi]
                    && r.slots
                        .iter()
                        .flatten()
                        .filter_map(|s| s.act.as_ref())
                        .any(|a| a.may_texture(n, hi[2], floor));
                base || slot
            });
            flags.push(flag);
            walls.push(input.walls && flag && n[2].abs() < WALL_SIN);
        }
        // Weld, keeping each facet's flag (faces dropped by the weld are degenerate).
        let mut w = weld(mesh);
        let kept: Vec<(bool, bool)> = mesh
            .faces
            .iter()
            .zip(flags.iter().zip(&walls))
            .filter(|(f, _)| {
                let key =
                    |i: u32| mesh.vertices[i as usize].map(|c| (f64::from(c) * 1e4).round() as i64);
                let (a, b, c) = (key(f[0]), key(f[1]), key(f[2]));
                a != b && b != c && a != c
            })
            .map(|(_, (&t, &wl))| (t && !wl, wl))
            .collect();
        (w.textured, w.wall) = kept.into_iter().unzip();
        if w.wall.iter().any(|&x| x) {
            // Rims: edges a wall shares with a surface textured here.
            let mut edges: HashMap<(u32, u32), (bool, bool)> = HashMap::new();
            for (fi, f) in w.faces.iter().enumerate() {
                if !w.wall[fi] && !w.textured[fi] {
                    continue;
                }
                for k in 0..3 {
                    let (a, b) = (f[k], f[(k + 1) % 3]);
                    let e = edges.entry((a.min(b), a.max(b))).or_default();
                    if w.wall[fi] {
                        e.0 = true;
                    } else {
                        e.1 = true;
                    }
                }
            }
            let mut rims: Vec<_> = edges
                .into_iter()
                .filter(|(_, (wl, t))| *wl && *t)
                .map(|(e, _)| e)
                .collect();
            rims.sort_unstable();
            rim_segs.extend(
                rims.into_iter()
                    .map(|(a, b)| [w.pos[a as usize], w.pos[b as usize]]),
            );
        }
        textured_area += (0..w.faces.len())
            .filter(|&f| w.textured[f])
            .map(|f| area(&w, f))
            .sum::<f64>();
        works.push(w);
    }
    // The walls are refined near the rims only: a band of `near + blend` (about five edges).
    textured_area += rim_segs
        .iter()
        .map(|s| length(sub(s[1], s[0])) * 6. * edge)
        .sum::<f64>();
    // A refined triangle has edges between half and the whole target length: about 0.2·L².
    let mut warnings = Vec::new();
    let untextured: usize = works
        .iter()
        .map(|w| w.textured.iter().filter(|t| !**t).count())
        .sum();
    let room = TRIANGLE_BUDGET.saturating_sub(untextured).max(1) as f64;
    let needed = (textured_area / (0.2 * room)).sqrt();
    let edge = if needed > edge {
        warnings.push(format!(
            "Texture: the textured surface is large; Mica refined it to {needed:.2} mm instead of {edge:.2} mm to stay within {} triangles, so the finest detail may be softer.",
            TRIANGLE_BUDGET
        ));
        needed
    } else {
        edge
    };
    let rims = input
        .walls
        .then(|| Arc::new(Rims::new(rim_segs, edge, 4. * edge)));
    // A wall facet is refined (and textured here) while it is near a rim.
    let near_rim = |w: &Work, f: usize, rims: &Rims| {
        let t = w.faces[f].map(|i| w.pos[i as usize]);
        let lo: [f64; 3] = std::array::from_fn(|k| t[0][k].min(t[1][k]).min(t[2][k]));
        let hi: [f64; 3] = std::array::from_fn(|k| t[0][k].max(t[1][k]).max(t[2][k]));
        rims.touches(lo, hi, edge)
    };
    let mark_walls = |w: &mut Work| {
        if let Some(r) = &rims {
            let marks: Vec<(usize, bool)> = (0..w.faces.len())
                .into_par_iter()
                .filter(|&f| w.wall[f])
                .map(|f| (f, near_rim(w, f, r)))
                .collect();
            for (f, t) in marks {
                w.textured[f] = t;
            }
        }
    };
    let mut budget_hit = false;
    let mut used: usize = works.iter().map(|w| w.faces.len()).sum();
    for w in &mut works {
        mark_walls(w);
        for _ in 0..60 {
            crate::progress::check(observer)?;
            let before = w.faces.len();
            let room = TRIANGLE_BUDGET.saturating_sub(used - before);
            let pass = refine_pass(w, edge, room);
            used += w.faces.len() - before;
            mark_walls(w);
            match pass {
                Pass::Split => {}
                Pass::Done => break,
                Pass::Budget => {
                    budget_hit = true;
                    break;
                }
            }
        }
    }
    if budget_hit && warnings.is_empty() {
        // Say so only when a noticeable share of the surface stayed coarse.
        let (coarse, total) = works
            .iter()
            .flat_map(|w| {
                w.faces
                    .iter()
                    .zip(&w.textured)
                    .filter(|(_, t)| **t)
                    .map(move |(f, _)| (w, f))
            })
            .fold((0usize, 0usize), |(c, t), (w, f)| {
                let long = (0..3).any(|k| {
                    length(sub(w.pos[f[k] as usize], w.pos[f[(k + 1) % 3] as usize])) > 1.5 * edge
                });
                (c + usize::from(long), t + 1)
            });
        if coarse * 20 > total {
            warnings.push(format!(
                "Texture: the textured surface reached {TRIANGLE_BUDGET} triangles; Mica stopped refining it, so the finest detail may be softer."
            ));
        }
    }
    // Rays against the object as loaded find thin walls and narrow gaps.
    let rays = RayTree::new(
        input
            .parts
            .iter()
            .flat_map(|(_, m, _)| {
                m.faces
                    .iter()
                    .map(|f| f.map(|i| m.vertices[i as usize].map(f64::from)))
            })
            .collect(),
    );
    let mut volumes: Vec<Option<TriangleMesh>> = vec![None; input.volume_count];
    let mut mesh = TriangleMesh::default();
    let (mut limited, mut moved) = (0, 0);
    for (part, w) in works.iter().enumerate() {
        crate::progress::check(observer)?;
        let (vi, original, _) = input.parts[part];
        let out = if w.textured.iter().any(|t| *t) {
            let (vertices, l, m) = displace_vertices(w, &field, part, &rays, rims.as_deref())?;
            limited += l;
            moved += m;
            TriangleMesh {
                vertices,
                faces: w.faces.clone(),
            }
        } else {
            original.clone()
        };
        let base = mesh.vertices.len() as u32;
        mesh.vertices.extend_from_slice(&out.vertices);
        mesh.faces
            .extend(out.faces.iter().map(|f| f.map(|i| i + base)));
        volumes[vi] = Some(out);
    }
    // A few points in narrow corners are normal; say so when it changes the look.
    if limited * 200 > moved {
        warnings.push(format!(
            "Texture: Mica made the texture shallower at {limited} points where the surface would have run into itself (thin walls or narrow gaps)."
        ));
    }
    Ok(Displaced {
        mesh,
        volumes,
        warnings,
        edge,
        walls: rims,
    })
}

/// Points of a displaced outline closer than this (mm) to the line through their neighbours
/// are dropped after slicing: far below what a nozzle prints (as the wall texture's own
/// simplification), it keeps the walls of refined flat areas and gentle slopes light.
pub const SLICE_TOLERANCE: f64 = 0.05;

/// Slices a displaced mesh: its loops are simplified by half the [`SLICE_TOLERANCE`] before the
/// closing offsets (a refined mesh crosses each plane at every few tenths of a millimetre), and
/// the islands by the whole tolerance after.
pub fn slice(
    mesh: &TriangleMesh,
    zs: &[f64],
    closing_radius: f64,
    observer: &dyn crate::progress::Observer,
) -> Vec<crate::slice::LayerSlice> {
    let mut layers = crate::slice::slice_mesh_simplified(
        mesh,
        zs,
        closing_radius,
        SLICE_TOLERANCE / 2.,
        observer,
    );
    let eps = SLICE_TOLERANCE * crate::geometry::SCALE;
    layers.par_iter_mut().for_each(|l| {
        l.islands = crate::geometry::clip::simplify_ex(&l.islands, eps);
    });
    layers
}

// ---------------------------------------------------------------------------------------------
// Cache.

fn cache_key(input: &Input<'_>) -> String {
    use sha2::Digest;
    let mut h = sha2::Sha256::new();
    h.update(b"mica-texture-displace");
    h.update(VERSION.to_le_bytes());
    for v in input.bounds.min.iter().chain(&input.bounds.max) {
        h.update(v.to_le_bytes());
    }
    h.update(input.floor.to_le_bytes());
    h.update(input.bed.to_le_bytes());
    h.update([u8::from(input.walls)]);
    let mesh_bytes = |h: &mut sha2::Sha256, m: &TriangleMesh| {
        h.update((m.vertices.len() as u64).to_le_bytes());
        for v in &m.vertices {
            for c in v {
                h.update(c.to_le_bytes());
            }
        }
        h.update((m.faces.len() as u64).to_le_bytes());
        for f in &m.faces {
            for i in f {
                h.update(i.to_le_bytes());
            }
        }
    };
    let config_bytes = |h: &mut sha2::Sha256, c: &Config| {
        for k in super::KEYS {
            if let Some(v) = c.raw(k) {
                h.update(k.as_bytes());
                h.update(b"=");
                h.update(v.as_bytes());
                h.update(b";");
            }
        }
        h.update(b"|");
    };
    match input.base {
        Base::Settings(t) => h.update(t.fingerprint().as_bytes()),
        Base::Config(c) => config_bytes(&mut h, c),
    }
    for (vi, mesh, cfg) in &input.parts {
        h.update((*vi as u64).to_le_bytes());
        mesh_bytes(&mut h, mesh);
        if let Some(c) = cfg {
            config_bytes(&mut h, c);
        }
        if let Some(m) = input.model {
            let p = &m.volumes[*vi].painting.texture;
            h.update([u8::from(p.reversed())]);
            for (t, codes) in p.entries() {
                h.update(t.to_le_bytes());
                h.update((codes.len() as u64).to_le_bytes());
                h.update(codes);
            }
        }
        h.update(b"#");
    }
    for (a, b, c) in &input.ranges {
        h.update(a.to_le_bytes());
        h.update(b.to_le_bytes());
        config_bytes(&mut h, c);
    }
    for (m, c) in &input.modifiers {
        mesh_bytes(&mut h, m);
        config_bytes(&mut h, c);
    }
    // Custom images are named by their content hash in the settings already.
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

type MemoryCache = Mutex<Vec<(String, Arc<Displaced>)>>;
fn memory() -> &'static MemoryCache {
    static CACHE: OnceLock<MemoryCache> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

fn disk_dir() -> &'static Mutex<Option<PathBuf>> {
    static DIR: OnceLock<Mutex<Option<PathBuf>>> = OnceLock::new();
    DIR.get_or_init(Default::default)
}

/// Keeps displaced meshes in `dir` too (the app's cache folder), so they survive a restart.
/// Without it (tests, library use) the cache is in memory only.
pub fn set_disk_cache(dir: Option<PathBuf>) {
    *disk_dir().lock().unwrap_or_else(|e| e.into_inner()) = dir;
}

/// Drops one displaced mesh kept in memory (tests).
#[cfg(test)]
fn forget(key: &str) {
    memory()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .retain(|(k, _)| k != key);
}

fn cache_get(key: &str) -> Option<Arc<Displaced>> {
    {
        let mut mem = memory().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(i) = mem.iter().position(|(k, _)| k == key) {
            let hit = mem.remove(i);
            let out = hit.1.clone();
            mem.push(hit);
            return Some(out);
        }
    }
    let dir = disk_dir()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()?;
    let bytes = std::fs::read(dir.join(format!("{key}.bin"))).ok()?;
    let d = Arc::new(decode(&bytes)?);
    remember(key, &d);
    Some(d)
}

fn remember(key: &str, d: &Arc<Displaced>) {
    let mut mem = memory().lock().unwrap_or_else(|e| e.into_inner());
    mem.retain(|(k, _)| k != key);
    mem.push((key.to_string(), d.clone()));
    // A few objects' worth, at most about 1 GB.
    let bytes = |d: &Displaced| {
        d.mesh.heap_bytes()
            + d.volumes
                .iter()
                .flatten()
                .map(|m| m.heap_bytes())
                .sum::<usize>()
    };
    while mem.len() > 8
        || (mem.len() > 1 && mem.iter().map(|(_, d)| bytes(d)).sum::<usize>() > 1 << 30)
    {
        mem.remove(0);
    }
}

fn cache_put(key: &str, d: &Arc<Displaced>) {
    remember(key, d);
    let Some(dir) = disk_dir().lock().unwrap_or_else(|e| e.into_inner()).clone() else {
        return;
    };
    // The disk cache is a convenience: failures to write it are ignored.
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    // One temporary name per writer: two jobs of one process (two plates, a slice and a
    // bake) may write the same entry at once.
    static WRITER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let tmp = dir.join(format!(
        "{key}.tmp{}-{}",
        std::process::id(),
        WRITER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    if std::fs::write(&tmp, encode(d)).is_ok() {
        let _ = std::fs::rename(&tmp, dir.join(format!("{key}.bin")));
    } else {
        let _ = std::fs::remove_file(&tmp);
    }
    trim_disk(&dir, DISK_CAP);
}

/// Deletes the least recently written entries until the folder is under `cap` bytes.
fn trim_disk(dir: &std::path::Path, cap: u64) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<(std::time::SystemTime, u64, PathBuf)> = read
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "bin"))
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            Some((m.modified().ok()?, m.len(), e.path()))
        })
        .collect();
    files.sort();
    let mut total: u64 = files.iter().map(|f| f.1).sum();
    for (_, len, path) in files {
        if total <= cap {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total = total.saturating_sub(len);
        }
    }
}

/// `MICATEX3`: the entry ends with the SHA-256 of everything before it.
const MAGIC: &[u8; 8] = b"MICATEX3";
const CHECKSUM: usize = 32;

fn encode(d: &Displaced) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&d.edge.to_le_bytes());
    let mesh = |out: &mut Vec<u8>, m: &TriangleMesh| {
        out.extend_from_slice(&(m.vertices.len() as u64).to_le_bytes());
        for v in &m.vertices {
            for c in v {
                out.extend_from_slice(&c.to_le_bytes());
            }
        }
        out.extend_from_slice(&(m.faces.len() as u64).to_le_bytes());
        for f in &m.faces {
            for i in f {
                out.extend_from_slice(&i.to_le_bytes());
            }
        }
    };
    mesh(&mut out, &d.mesh);
    out.extend_from_slice(&(d.volumes.len() as u64).to_le_bytes());
    for v in &d.volumes {
        match v {
            Some(m) => {
                out.push(1);
                mesh(&mut out, m);
            }
            None => out.push(0),
        }
    }
    out.extend_from_slice(&(d.warnings.len() as u64).to_le_bytes());
    for w in &d.warnings {
        out.extend_from_slice(&(w.len() as u64).to_le_bytes());
        out.extend_from_slice(w.as_bytes());
    }
    match &d.walls {
        Some(r) => {
            out.push(1);
            out.extend_from_slice(&r.near.to_le_bytes());
            out.extend_from_slice(&r.blend.to_le_bytes());
            out.extend_from_slice(&(r.segs.len() as u64).to_le_bytes());
            for c in r.segs.iter().flatten().flatten() {
                out.extend_from_slice(&c.to_le_bytes());
            }
        }
        None => out.push(0),
    }
    let sum = <sha2::Sha256 as sha2::Digest>::digest(&out);
    out.extend_from_slice(&sum);
    out
}

fn decode(bytes: &[u8]) -> Option<Displaced> {
    struct R<'a>(&'a [u8]);
    impl R<'_> {
        fn take(&mut self, n: usize) -> Option<&[u8]> {
            if self.0.len() < n {
                return None;
            }
            let (a, b) = self.0.split_at(n);
            self.0 = b;
            Some(a)
        }
        fn u64(&mut self) -> Option<u64> {
            Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
        }
        fn len(&mut self, unit: usize) -> Option<usize> {
            let n = usize::try_from(self.u64()?).ok()?;
            (n.checked_mul(unit)? <= self.0.len()).then_some(n)
        }
        fn mesh(&mut self) -> Option<TriangleMesh> {
            let nv = self.len(12)?;
            let mut vertices = Vec::with_capacity(nv);
            for _ in 0..nv {
                let b = self.take(12)?;
                vertices.push(std::array::from_fn(|k| {
                    f32::from_le_bytes(b[4 * k..4 * k + 4].try_into().unwrap())
                }));
            }
            let nf = self.len(12)?;
            let mut faces = Vec::with_capacity(nf);
            for _ in 0..nf {
                let b = self.take(12)?;
                let f: [u32; 3] = std::array::from_fn(|k| {
                    u32::from_le_bytes(b[4 * k..4 * k + 4].try_into().unwrap())
                });
                if f.iter().any(|&i| i as usize >= nv) {
                    return None;
                }
                faces.push(f);
            }
            Some(TriangleMesh { vertices, faces })
        }
    }
    // A damaged entry (a disk error, a file changed by hand) is a miss, never a wrong mesh:
    // the texture CLI fuzz of 2026-10-21 sliced 64 scrambled bytes into a 100,000-layer model.
    let (bytes, sum) = bytes.split_at_checked(bytes.len().checked_sub(CHECKSUM)?)?;
    if <sha2::Sha256 as sha2::Digest>::digest(bytes).as_slice() != sum {
        return None;
    }
    let mut r = R(bytes);
    if r.take(8)? != MAGIC || u32::from_le_bytes(r.take(4)?.try_into().ok()?) != VERSION {
        return None;
    }
    let edge = f64::from_le_bytes(r.take(8)?.try_into().ok()?);
    let mesh = r.mesh()?;
    let nvol = r.len(1)?;
    let mut volumes = Vec::with_capacity(nvol);
    for _ in 0..nvol {
        volumes.push(match r.take(1)?[0] {
            1 => Some(r.mesh()?),
            _ => None,
        });
    }
    let nw = r.len(8)?;
    let mut warnings = Vec::with_capacity(nw);
    for _ in 0..nw {
        let n = r.len(1)?;
        warnings.push(String::from_utf8(r.take(n)?.to_vec()).ok()?);
    }
    let walls = match r.take(1)?[0] {
        0 => None,
        1 => {
            let mut f = || Some(f64::from_le_bytes(r.take(8)?.try_into().ok()?));
            let (near, blend) = (f()?, f()?);
            let n = r.len(48)?;
            let mut segs = Vec::with_capacity(n);
            for _ in 0..n {
                let mut c = [0f64; 6];
                for x in &mut c {
                    *x = f64::from_le_bytes(r.take(8)?.try_into().ok()?);
                }
                segs.push([[c[0], c[1], c[2]], [c[3], c[4], c[5]]]);
            }
            Some(Arc::new(Rims::new(segs, near, blend)))
        }
        _ => return None,
    };
    r.0.is_empty().then_some(Displaced {
        mesh,
        volumes,
        warnings,
        edge,
        walls,
    })
}

/// The object's meshes with the texture moved into them, for "Bake texture into mesh": the
/// textured model volumes (`None` where a volume has no texture on all surfaces).
pub fn bake(
    mesh: &TriangleMesh,
    s: &crate::settings::PrintSettings,
    model: Option<(&RegionModel, &Config)>,
) -> Result<Option<Arc<Displaced>>, String> {
    // A baked mesh carries the whole texture, walls included.
    for_object_with(mesh, s, model, &crate::progress::Silent, false)
}

#[cfg(test)]
mod tests;
