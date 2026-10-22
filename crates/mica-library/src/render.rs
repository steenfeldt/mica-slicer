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

use crate::extract::{first_plate_mesh, first_plate_tool_mesh, parse_obj, project_colours};
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
    /// Simplified by vertex clustering (see [`simplify`]), keeping each face's colour.
    pub fn simplified(self, cells: u32) -> Self {
        let (mesh, kept) = simplify_faces(&self.mesh, cells);
        let face_colours = self
            .face_colours
            .map(|c| kept.iter().map(|&f| c[f]).collect());
        Self {
            mesh,
            face_colours,
            triangles: self.triangles,
        }
    }
}

fn hex_rgb(s: &str) -> Option<[u8; 3]> {
    let h = s.strip_prefix('#')?;
    let byte = |i: usize| u8::from_str_radix(h.get(i..i + 2)?, 16).ok();
    Some([byte(0)?, byte(2)?, byte(4)?])
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
    let project =
        mica_core::threemf::read_3mf(std::io::Cursor::new(data)).map_err(|e| e.to_string())?;
    let colours: Vec<Option<[u8; 3]>> = project_colours(&project)
        .iter()
        .map(|c| c.as_deref().and_then(hex_rgb))
        .collect();
    if colours.iter().all(Option::is_none) {
        let mesh = first_plate_mesh(&project);
        if mesh.faces.is_empty() {
            return Err("the file holds no triangles".into());
        }
        let triangles = mesh.faces.len();
        return Ok(PreviewMesh {
            mesh,
            face_colours: None,
            triangles,
        });
    }
    let triangles = crate::extract::first_plate_triangles(&project);
    let (mesh, tools) = first_plate_tool_mesh(&project);
    drop(project);
    if mesh.faces.is_empty() {
        return Err("the file holds no triangles".into());
    }
    // A tool without a colour of its own keeps the accent.
    let face_colours = tools
        .iter()
        .map(|&t| {
            t.checked_sub(1)
                .and_then(|i| colours.get(i).copied().flatten())
                .unwrap_or(VIOLET)
        })
        .collect();
    Ok(PreviewMesh {
        mesh,
        face_colours: Some(face_colours),
        triangles,
    })
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
        // all tool 2.
        assert_eq!(count([0xCB, 0x8F, 0x29]), 8);
        assert_eq!(count([0, 0, 0]), 2 + 12);
        assert_eq!(count([0xFF, 0xFF, 0xFF]), 2);
        assert!(!c.contains(&VIOLET) && !c.contains(&[0xFC, 0xE9, 0x4F]));
        // Simplifying keeps each face's colour.
        let s = p.clone().simplified(1024);
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
