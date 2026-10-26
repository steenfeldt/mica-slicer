// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Thumbnails Mica renders itself, for models that carry none (STL, OBJ, 3MFs without an
//! embedded picture), and the meshes the Library's 3D preview shows.
//!
//! The picture comes from the CPU rasterizer that also draws G-code thumbnails
//! (`mica_gcode::thumbnails::render_orbit`): one fixed, slightly raised three-quarter view,
//! framed to the model, in the theme's violet with soft camera-fixed lighting on a
//! transparent background. The same mesh always gives the same PNG bytes. Very detailed
//! meshes are simplified first (vertex clustering finer than a pixel); meshes above the
//! triangle budget are not rendered at all and keep a placeholder.

use crate::extract::{first_plate_mesh, parse_obj, plate_mesh, plate_tool_mesh, project_colours};
use crate::model::{Info, Kind};
use mica_core::mesh::TriangleMesh;
use mica_gcode::thumbnails::{Fit, Orbit};
use std::collections::HashMap;
use std::path::Path;

/// Bumped when the look of rendered thumbnails changes: older ones are rendered again.
pub const RENDER_VERSION: u32 = 1;
/// Rendered thumbnails are this many pixels square (sharp on a Retina card).
pub const THUMB_PX: u16 = 384;
/// The thumbnail colour for rendered models: the theme's violet accent.
pub const VIOLET: [u8; 3] = [0x8b, 0x7c, 0xf6];
/// The thumbnail camera (and the 3D preview's starting view): front right, a little above.
pub const VIEW: Orbit = Orbit {
    azimuth: 35.,
    elevation: 28.,
    zoom: 1.,
    fit: Fit::Box,
};
/// Meshes with more triangles are simplified before rendering a thumbnail.
pub const SIMPLIFY_ABOVE: usize = 600_000;
/// Thumbnails simplify to this many cells along the largest side (finer than a pixel of the
/// supersampled picture, so the simplification does not show).
const THUMB_CELLS: u32 = 1024;
/// Default triangle budget: meshes above it are not rendered (a placeholder shows).
pub const DEFAULT_BUDGET: u64 = 12_000_000;
/// Files larger than this are not read for a thumbnail or a preview.
pub const MAX_FILE: u64 = 384 << 20;

/// Whether Mica should render a thumbnail for this content: a readable mesh without an
/// embedded picture whose thumbnail (or failed attempt) is from an older renderer.
pub fn needs_render(kind: Kind, info: &Info) -> bool {
    let rendered_kind = matches!(kind, Kind::Stl | Kind::Obj);
    (rendered_kind || kind == Kind::ThreeMf)
        && info.readable
        && info.render_version < RENDER_VERSION
        && (info.thumbnail.is_none() || info.thumbnail_rendered || rendered_kind)
}

/// Whether a known model's filament colours were read by an older reader
/// (`crate::model::COLOURS_VERSION`) and should be read again.
pub fn needs_colours(kind: Kind, info: &Info) -> bool {
    matches!(kind, Kind::ThreeMf | Kind::Gcode | Kind::Bgcode)
        && info.readable
        && info.colours_version < crate::model::COLOURS_VERSION
}

/// What the Library's 3D preview draws: the mesh, and for a project with filament colours
/// the colour of every face (its part's tool, or the tool painted on it), as in Prepare.
#[derive(Clone, Debug)]
pub struct PreviewMesh {
    pub mesh: TriangleMesh,
    /// One colour per face of `mesh`; `None`: no colours known (drawn in the accent).
    pub face_colours: Option<Vec<[u8; 3]>>,
    /// Triangles of the model itself (painted facets split into leaves are counted once).
    pub triangles: usize,
}

impl PreviewMesh {
    /// Simplified by vertex clustering (see [`simplify`]), keeping each face's colour. The
    /// mesh itself is left as it is (the preview keeps it to simplify again when zoomed).
    pub fn simplified(&self, cells: u32) -> Self {
        let (mesh, kept) = simplify_faces(&self.mesh, cells);
        let face_colours = self
            .face_colours
            .as_ref()
            .map(|c| kept.iter().map(|&f| c[f]).collect());
        Self {
            mesh,
            face_colours,
            triangles: self.triangles,
        }
    }
}

/// Rendered pixels per millimetre of the 3D preview's picture (`picture` pixels, framed to
/// the bounding sphere of a model of `size` mm as `Fit::Sphere` in
/// `mica_gcode::thumbnails::render_orbit` does, with its 2.5 % margin) at `zoom`.
pub fn preview_px_per_mm(size: [f32; 3], picture: [u16; 2], zoom: f32) -> f32 {
    let d = size.iter().map(|v| v * v).sum::<f32>().sqrt();
    f32::from(picture[0].min(picture[1])) / (1.025 * d) * zoom
}

/// The clustering grid (cells along the largest side, as [`simplify`] takes it) whose cells
/// span at most one rendered pixel of the 3D preview at `zoom`: finer than that, clustering
/// does not show (coarser, a cell mixing a flat face with its neighbour tilts the face).
pub fn preview_cells(size: [f32; 3], picture: [u16; 2], zoom: f32) -> u32 {
    let extent = size.iter().copied().fold(0f32, f32::max);
    let cells = (extent * preview_px_per_mm(size, picture, zoom)).ceil();
    if cells.is_finite() {
        (cells as u32).max(1)
    } else {
        1
    }
}

/// The preview mesh of a model file's content: a 3MF's first plate in its filament colours
/// (when its settings name them), anything else as [`load_mesh`] without colours.
pub fn load_preview(kind: Kind, data: &[u8]) -> Result<PreviewMesh, String> {
    if kind != Kind::ThreeMf {
        let mesh = load_mesh(kind, data)?;
        let triangles = mesh.faces.len();
        return Ok(PreviewMesh {
            mesh,
            face_colours: None,
            triangles,
        });
    }
    PreviewProject::read(data)?.plate(0)
}

/// A project read for the 3D preview: its plates, and what any plate's [`PreviewMesh`] is
/// made from, so another plate is shown without reading the file again.
pub struct PreviewProject {
    project: mica_core::threemf::Project,
    /// The tools' display colours (one slot per tool); empty or all `None`: no colours.
    colours: Vec<Option<[u8; 3]>>,
    names: Vec<String>,
}

impl PreviewProject {
    /// Reads a 3MF's content.
    pub fn read(data: &[u8]) -> Result<Self, String> {
        let project =
            mica_core::threemf::read_3mf(std::io::Cursor::new(data)).map_err(|e| e.to_string())?;
        let colours = project_colours(&project)
            .iter()
            .map(|c| c.as_deref().and_then(mica_core::color::parse_hex))
            .collect();
        let names = crate::extract::plate_names(&project);
        Ok(Self {
            project,
            colours,
            names,
        })
    }

    /// Reads a 3MF file (read-only).
    pub fn read_file(path: &Path) -> Result<Self, String> {
        let size = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
        if size > MAX_FILE {
            return Err("too large to preview".into());
        }
        Self::read(&std::fs::read(path).map_err(|e| e.to_string())?)
    }

    /// The plates' names in order (empty for a plate without one); at least one.
    pub fn plate_names(&self) -> &[String] {
        &self.names
    }

    /// Plate `plate` (zero-based) in its filament colours: each part's tool, colour-painted
    /// facets split into their painted leaves, near-black lifted ([`display_colour`]).
    pub fn plate(&self, plate: usize) -> Result<PreviewMesh, String> {
        let empty = || {
            if self.names.len() > 1 {
                "nothing on this plate".to_string()
            } else {
                "the file holds no triangles".to_string()
            }
        };
        let project = &self.project;
        if self.colours.iter().all(Option::is_none) {
            let mesh = plate_mesh(project, plate);
            if mesh.faces.is_empty() {
                return Err(empty());
            }
            let triangles = mesh.faces.len();
            return Ok(PreviewMesh {
                mesh,
                face_colours: None,
                triangles,
            });
        }
        let triangles = crate::extract::plate_triangles(project, plate);
        let (mesh, tools) = plate_tool_mesh(project, plate);
        if mesh.faces.is_empty() {
            return Err(empty());
        }
        // A tool without a colour of its own keeps the accent.
        let face_colours = tools
            .iter()
            .map(|&t| {
                t.checked_sub(1)
                    .and_then(|i| self.colours.get(i).copied().flatten())
                    .map(display_colour)
                    .unwrap_or(VIOLET)
            })
            .collect();
        Ok(PreviewMesh {
            mesh,
            face_colours: Some(face_colours),
            triangles,
        })
    }
}

/// Reads a model file (read-only) and returns its preview mesh.
pub fn load_preview_file(kind: Kind, path: &Path) -> Result<PreviewMesh, String> {
    let size = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
    if size > MAX_FILE {
        return Err("too large to preview".into());
    }
    let data = std::fs::read(path).map_err(|e| e.to_string())?;
    load_preview(kind, &data)
}

/// The mesh of a model file's content: an STL or OBJ as it is, a 3MF's assembled first plate.
pub fn load_mesh(kind: Kind, data: &[u8]) -> Result<TriangleMesh, String> {
    let mesh = match kind {
        Kind::Stl => TriangleMesh::from_stl_bytes(data).map_err(|e| e.to_string())?,
        Kind::Obj => parse_obj(data)?,
        Kind::ThreeMf => first_plate_mesh(
            &mica_core::threemf::read_3mf(std::io::Cursor::new(data)).map_err(|e| e.to_string())?,
        ),
        _ => return Err(format!("{} files have no 3D shape", kind.label())),
    };
    if mesh.faces.is_empty() {
        return Err("the file holds no triangles".into());
    }
    Ok(mesh)
}

/// Reads a model file (read-only) and returns its mesh.
pub fn load_mesh_file(kind: Kind, path: &Path) -> Result<TriangleMesh, String> {
    let size = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
    if size > MAX_FILE {
        return Err("too large to preview".into());
    }
    let data = std::fs::read(path).map_err(|e| e.to_string())?;
    load_mesh(kind, &data)
}

/// Simplifies a mesh by vertex clustering: vertices in the same cell of a grid with `cells`
/// cells along the largest side become one (the first one seen), and triangles that collapse
/// are dropped. Deterministic; linear in the mesh size.
pub fn simplify(mesh: &TriangleMesh, cells: u32) -> TriangleMesh {
    simplify_faces(mesh, cells).0
}

/// [`simplify`], also returning the source face of every kept face.
pub fn simplify_faces(mesh: &TriangleMesh, cells: u32) -> (TriangleMesh, Vec<usize>) {
    let (lo, hi) = mesh.bounds();
    let extent = (0..3).map(|i| hi[i] - lo[i]).fold(0f32, f32::max);
    if !(extent.is_finite() && extent > 0.) || cells == 0 {
        return (mesh.clone(), (0..mesh.faces.len()).collect());
    }
    let cell = extent / cells as f32;
    let key = |v: [f32; 3]| -> [u32; 3] {
        std::array::from_fn(|i| (((v[i] - lo[i]) / cell).floor().max(0.) as u32).min(cells))
    };
    let mut index: HashMap<[u32; 3], u32> = HashMap::with_capacity(mesh.vertices.len() / 2);
    let mut vertices = Vec::new();
    let remap: Vec<u32> = mesh
        .vertices
        .iter()
        .map(|&v| {
            *index.entry(key(v)).or_insert_with(|| {
                vertices.push(v);
                vertices.len() as u32 - 1
            })
        })
        .collect();
    let mut kept = Vec::new();
    let faces = mesh
        .faces
        .iter()
        .enumerate()
        .filter_map(|(n, f)| {
            let g = f.map(|i| remap.get(i as usize).copied().unwrap_or(0));
            (g[0] != g[1] && g[1] != g[2] && g[0] != g[2]).then(|| {
                kept.push(n);
                g
            })
        })
        .collect();
    (TriangleMesh { vertices, faces }, kept)
}

/// The library thumbnail of a mesh as PNG bytes (the same bytes for the same mesh).
pub fn thumbnail_png(mesh: &TriangleMesh) -> Result<Vec<u8>, String> {
    let simplified;
    let mesh = if mesh.faces.len() > SIMPLIFY_ABOVE {
        simplified = simplify(mesh, THUMB_CELLS);
        &simplified
    } else {
        mesh
    };
    let img = mica_gcode::thumbnails::render_orbit(mesh, THUMB_PX, THUMB_PX, VIOLET, VIEW)
        .map_err(|e| e.to_string())?;
    mica_gcode::thumbnails::encode_png(u32::from(img.width), u32::from(img.height), &img.rgba)
        .map_err(|e| e.to_string())
}

/// What rendering one file's thumbnail gave.
#[derive(Debug)]
pub enum Rendered {
    Png(Vec<u8>),
    /// No thumbnail for this content (too detailed, no triangles): a placeholder shows, and
    /// it is not tried again.
    Skipped(String),
    /// The file could not be read right now (gone, offline): tried again later.
    Unavailable,
}

/// Renders the thumbnail of one file. `triangles` is what the index counted (checked against
/// `budget` before anything is read).
pub fn render_file(kind: Kind, path: &Path, triangles: u64, budget: u64) -> Rendered {
    let too_detailed = |n: u64| format!("too detailed to preview ({n} triangles)");
    if triangles > budget {
        return Rendered::Skipped(too_detailed(triangles));
    }
    let data = match std::fs::metadata(path) {
        Ok(m) if m.len() > MAX_FILE => return Rendered::Skipped("too large to preview".into()),
        Ok(_) => match std::fs::read(path) {
            Ok(d) => d,
            Err(_) => return Rendered::Unavailable,
        },
        Err(_) => return Rendered::Unavailable,
    };
    let mesh = match load_mesh(kind, &data) {
        Ok(m) => m,
        Err(e) => return Rendered::Skipped(e),
    };
    drop(data);
    if mesh.faces.len() as u64 > budget {
        return Rendered::Skipped(too_detailed(mesh.faces.len() as u64));
    }
    match thumbnail_png(&mesh) {
        Ok(png) => Rendered::Png(png),
        Err(e) => Rendered::Skipped(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cube() -> TriangleMesh {
        let mut m = TriangleMesh::default();
        let s = 20.;
        for z in 0..2 {
            for y in 0..2 {
                for x in 0..2 {
                    m.vertices.push([x as f32 * s, y as f32 * s, z as f32 * s]);
                }
            }
        }
        let quads = [
            [0, 2, 3, 1],
            [4, 5, 7, 6],
            [0, 1, 5, 4],
            [2, 6, 7, 3],
            [0, 4, 6, 2],
            [1, 3, 7, 5],
        ];
        for q in &quads {
            m.faces.push([q[0], q[1], q[2]]);
            m.faces.push([q[0], q[2], q[3]]);
        }
        m
    }

    #[test]
    fn preview_cells_match_the_rendered_pixels() {
        // Seen straight from the front, the 20 mm cube is 20 mm wide on the picture.
        let size = [20.; 3];
        for (picture, zoom) in [([400u16, 300u16], 1f32), ([300, 500], 1.5)] {
            let orbit = Orbit {
                azimuth: 0.,
                elevation: 0.,
                zoom,
                fit: Fit::Sphere,
            };
            let img = mica_gcode::thumbnails::render_orbit(
                &cube(),
                picture[0],
                picture[1],
                VIOLET,
                orbit,
            )
            .unwrap();
            let row = usize::from(img.height / 2) * usize::from(img.width) * 4;
            let opaque = (0..usize::from(img.width))
                .filter(|x| img.rgba[row + x * 4 + 3] > 127)
                .count() as f32;
            let px = 20. * preview_px_per_mm(size, picture, zoom);
            assert!((opaque - px).abs() <= 1.5, "{opaque} vs {px}");
            // One cell of that grid spans at most one pixel, and not much less.
            let cells = preview_cells(size, picture, zoom);
            let cell_px = 20. / cells as f32 * preview_px_per_mm(size, picture, zoom);
            assert!(
                cell_px <= 1. && cell_px > 0.95,
                "{cells} cells: {cell_px} px"
            );
        }
    }

    #[test]
    fn same_mesh_same_bytes() {
        let benchy =
            std::fs::read(crate::testkit::repo_root().join("tests/models/3DBenchy.stl")).unwrap();
        let mesh = load_mesh(Kind::Stl, &benchy).unwrap();
        let a = thumbnail_png(&mesh).unwrap();
        let b = thumbnail_png(&load_mesh(Kind::Stl, &benchy).unwrap()).unwrap();
        assert!(a.starts_with(b"\x89PNG"));
        assert_eq!(a, b, "the same mesh renders to the same bytes");
        // Real compression: far below the raw 384 x 384 RGBA.
        assert!(a.len() < 384 * 384 * 4 / 8, "{} bytes", a.len());
    }

    #[test]
    fn simplify_keeps_the_shape_and_is_deterministic() {
        let benchy =
            std::fs::read(crate::testkit::repo_root().join("tests/models/3DBenchy.stl")).unwrap();
        let mesh = load_mesh(Kind::Stl, &benchy).unwrap();
        let s = simplify(&mesh, 64);
        assert!(s.faces.len() < mesh.faces.len() / 2, "{}", s.faces.len());
        assert!(!s.faces.is_empty());
        let (a, b) = (mesh.bounds(), s.bounds());
        for i in 0..3 {
            assert!((a.0[i] - b.0[i]).abs() < 2. && (a.1[i] - b.1[i]).abs() < 2.);
        }
        let again = simplify(&mesh, 64);
        assert_eq!((again.vertices, again.faces), (s.vertices, s.faces));
        // A plain box stays a box.
        assert_eq!(simplify(&cube(), 1024).faces.len(), 12);
    }

    #[test]
    fn project_preview_is_in_its_filament_colours() {
        let p = load_preview(Kind::ThreeMf, &crate::testkit::coloured_3mf()).unwrap();
        // Plate 1: two cubes of 12 triangles (the painted sides split into their leaves).
        assert_eq!(p.triangles, 24);
        let c = p.face_colours.as_ref().expect("coloured");
        assert_eq!(c.len(), p.mesh.faces.len());
        let count = |rgb: [u8; 3]| c.iter().filter(|&&x| x == rgb).count();
        // Cube 1: 8 unpainted faces in tool 1, one side tool 2, one side tool 3; cube 2
        // all tool 2 (black, drawn lifted to dark grey like Prepare).
        assert_eq!(count([0xCB, 0x8F, 0x29]), 8);
        assert_eq!(count([51, 51, 51]), 2 + 12);
        assert_eq!(count([0xFF, 0xFF, 0xFF]), 2);
        assert!(!c.contains(&VIOLET) && !c.contains(&[0xFC, 0xE9, 0x4F]));
        // Simplifying keeps each face's colour.
        let s = p.simplified(1024);
        assert_eq!(s.face_colours.unwrap().len(), s.mesh.faces.len());
        // A bare STL has no colours (drawn in the accent, as before).
        let stl =
            std::fs::read(crate::testkit::repo_root().join("tests/models/cube20.stl")).unwrap();
        let p = load_preview(Kind::Stl, &stl).unwrap();
        assert!(p.face_colours.is_none());
        assert_eq!(p.triangles, 12);
        // Known models read by the older colour reader are read again.
        let old = Info {
            readable: true,
            ..Default::default()
        };
        assert!(needs_colours(Kind::ThreeMf, &old) && !needs_colours(Kind::Stl, &old));
    }

    #[test]
    fn each_plate_of_a_project_in_its_colours() {
        let p = PreviewProject::read(&crate::testkit::four_plates_3mf()).unwrap();
        assert_eq!(p.plate_names(), ["", "Lid", "Loose", ""]);
        // Plate 1 as the first-plate preview shows it.
        let first = load_preview(Kind::ThreeMf, &crate::testkit::four_plates_3mf()).unwrap();
        let one = p.plate(0).unwrap();
        assert_eq!(one.triangles, first.triangles);
        assert_eq!(one.face_colours, first.face_colours);
        // The lid: tool 3, white; the loose cube: tool 4, red.
        let only = |m: &PreviewMesh, rgb: [u8; 3]| {
            let c = m.face_colours.as_ref().unwrap();
            !c.is_empty() && c.iter().all(|&x| x == rgb)
        };
        let lid = p.plate(1).unwrap();
        assert!(only(&lid, [0xFF, 0xFF, 0xFF]), "{:?}", lid.face_colours);
        let loose = p.plate(2).unwrap();
        assert!(only(&loose, [0xFB, 0x02, 0x07]), "{:?}", loose.face_colours);
        assert_eq!((lid.triangles, loose.triangles), (12, 12));
        // An empty plate, and one the project does not have, say so.
        assert_eq!(p.plate(3).unwrap_err(), "nothing on this plate");
        assert!(p.plate(9).is_err());
        // One plate: one unnamed plate; the original two-plate file names its second.
        let two = std::fs::read(crate::testkit::repo_root().join("tests/models/plates-orca.3mf"))
            .unwrap();
        assert_eq!(
            PreviewProject::read(&two).unwrap().plate_names(),
            ["", "Lid"]
        );
        let one = std::fs::read(crate::testkit::repo_root().join("tests/models/painted-cube.3mf"))
            .unwrap();
        let one = PreviewProject::read(&one).unwrap();
        assert_eq!(one.plate_names(), [""]);
        assert!(one.plate(0).is_ok());
    }

    #[test]
    fn budget_skips_before_reading() {
        let dir = crate::testkit::TempDir::new("render-budget");
        let path = dir.path().join("cube.stl");
        std::fs::copy(
            crate::testkit::repo_root().join("tests/models/cube20.stl"),
            &path,
        )
        .unwrap();
        assert!(matches!(
            render_file(Kind::Stl, &path, 12, 11),
            Rendered::Skipped(why) if why.contains("too detailed")
        ));
        assert!(matches!(
            render_file(Kind::Stl, &path, 12, DEFAULT_BUDGET),
            Rendered::Png(_)
        ));
        assert!(matches!(
            render_file(Kind::Stl, &dir.path().join("gone.stl"), 12, DEFAULT_BUDGET),
            Rendered::Unavailable
        ));
    }
}

/// A filament colour as the preview draws it: near-black lifted so its shading stays visible on
/// the dark panel, keeping the hue (the same rule as Prepare's `display_colour` in mica-gui).
pub fn display_colour(c: [u8; 3]) -> [u8; 3] {
    const FLOOR: u8 = 51; // 0.2 of 255
    let max = c[0].max(c[1]).max(c[2]);
    if max >= FLOOR {
        return c;
    }
    let lift = FLOOR - max;
    c.map(|v| v + lift)
}

#[cfg(test)]
mod display_colour_tests {
    #[test]
    fn near_black_is_lifted_and_keeps_its_hue() {
        assert_eq!(super::display_colour([0, 0, 0]), [51, 51, 51]);
        assert_eq!(super::display_colour([5, 8, 30]), [26, 29, 51]);
        assert_eq!(super::display_colour([253, 128, 8]), [253, 128, 8]);
    }
}
