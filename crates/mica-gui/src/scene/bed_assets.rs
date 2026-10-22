// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/slic3r/GUI/3DBed.cpp :: Bed3D::init_internal_model_from_file
// Source: prusaslicer@b028299c77 src/slic3r/GUI/3DBed.cpp :: Bed3D::init_triangles
// Source: prusaslicer@b028299c77 src/slic3r/GUI/3DBed.cpp :: Bed3D::render_system
// Source: prusaslicer@b028299c77 src/slic3r/GUI/3DBed.cpp :: Bed3D::render_texture
// Source: prusaslicer@b028299c77 src/libslic3r/Preset.cpp :: PresetUtils::system_printer_bed_model
// Source: prusaslicer@b028299c77 resources/shaders/110/printbed.fs
// Changes: Rust; the printer's bed model (STL) and texture (SVG) are looked up at runtime in
// the pinned upstream checkouts and the installed PrusaSlicer / OrcaSlicer apps; the SVG is
// rasterised once with resvg instead of nanosvg + GL compression; the
// texture quad carries wgpu UVs (v runs down the image) over the bed outline's bounding
// box; the see-through rules for a camera below the bed return alpha factors for the
// renderer instead of GL state; a sheet whose top surface would reach the texture quad
// (OrcaSlicer's U1 model) is lowered so the top sits just under it; dark line art is
// inverted for the dark theme.
// Changes: The home folder comes from mica_core::user_data (none in tests and test tools).
// Changes: Also the Orca subset a packaged Mica ships and the Prusa assets it downloaded.

//! The printer's real bed sheet: PrusaSlicer and OrcaSlicer ship a bed model (`*_bed.stl`)
//! and a texture (`*.svg`) per printer model. Mica reads them from those installations (or
//! the pinned upstream checkouts) and places them like PrusaSlicer does: the model's origin
//! goes to the centre of the `bed_shape` bounding box, 0.03 mm below the bed plane, and the
//! texture is stretched over the bed outline's bounding box at `GROUND_Z`.

use glam::{Vec2, Vec3};
use mica_core::mesh::TriangleMesh;
use std::path::{Path, PathBuf};

/// Height of the texture quad (PrusaSlicer `GROUND_Z`).
pub const GROUND_Z: f32 = -0.02;
/// The bed model sits slightly lower so it never fights the texture quad.
pub const MODEL_Z: f32 = -0.03;
/// Longest side of the rasterised texture, in pixels.
pub const TEXTURE_SIDE: u32 = 2048;
/// Alpha of the texture line art (and the fallback grid) when the camera is below the bed.
pub const SEE_THROUGH_ALPHA: f32 = 0.5;

/// Resolved asset files of the selected printer model, if they were found.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct AssetFiles {
    pub model: Option<PathBuf>,
    pub texture: Option<PathBuf>,
}
impl AssetFiles {
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.model.is_none() && self.texture.is_none()
    }
}

/// Directories searched for a vendor's bed assets, in order: an explicit override
/// (`MICA_BED_ASSETS`), the pinned upstream data of a development checkout (never in a
/// packaged build), the Orca profiles a packaged Mica ships and the Prusa assets it downloaded
/// (`mica_profiles::vendor_profiles`), then the installed slicers.
pub fn asset_roots(vendor: &str) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(p) = std::env::var_os("MICA_BED_ASSETS") {
        roots.push(PathBuf::from(p));
    }
    if let Some(upstream) = mica_core::user_data::upstream_dir() {
        for id in ["prusaslicer", "orca", "snapmaker-orca"] {
            roots.push(upstream.join(id).join("resources/profiles").join(vendor));
        }
        // The pinned Prusa vendor profiles repository holds the same assets.
        roots.push(upstream.join("prusa-profiles").join(vendor));
    }
    // What a packaged Mica ships (the Orca subset) or downloaded (the Prusa bundle's assets).
    if let Some(orca) = mica_profiles::vendor_profiles::bundled_orca() {
        roots.push(orca.join(vendor));
    }
    if vendor == mica_profiles::vendor_profiles::prusa::VENDOR {
        roots.push(mica_profiles::vendor_profiles::prusa::cache_dir());
    }
    let home = mica_core::user_data::home();
    if let Some(home) = &home {
        // PrusaSlicer downloads updated vendor assets next to the bundle it updated.
        roots.push(
            home.join("Library/Application Support/PrusaSlicer/vendor")
                .join(vendor),
        );
        roots.push(home.join(".config/PrusaSlicer/vendor").join(vendor));
    }
    for app in [
        "/Applications/Original Prusa Drivers/PrusaSlicer.app/Contents/Resources/profiles",
        "/Applications/PrusaSlicer.app/Contents/Resources/profiles",
        "/Applications/OrcaSlicer.app/Contents/Resources/profiles",
        "/Applications/Snapmaker Orca.app/Contents/Resources/profiles",
        "/usr/share/PrusaSlicer/profiles",
        "/usr/share/OrcaSlicer/profiles",
    ] {
        roots.push(Path::new(app).join(vendor));
    }
    if let Some(home) = &home {
        for flatpak in [
            ".local/share/flatpak/app/com.prusa3d.PrusaSlicer/current/active/files/share/PrusaSlicer/profiles",
            ".local/share/flatpak/app/io.github.softfever.OrcaSlicer/current/active/files/share/OrcaSlicer/profiles",
        ] {
            roots.push(home.join(flatpak).join(vendor));
        }
    }
    roots
}

/// The first root that holds `file`. Names with path separators are refused: the profile
/// names a file in the vendor directory, nothing else.
pub fn find_in(roots: &[PathBuf], file: &str) -> Option<PathBuf> {
    let file = file.trim();
    if file.is_empty()
        || Path::new(file)
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return None;
    }
    roots.iter().map(|r| r.join(file)).find(|p| p.is_file())
}

/// Resolves the model and texture named by a printer model section. Either may be missing;
/// a texture that is not an SVG/PNG, or a model that is not an STL, is ignored like PrusaSlicer
/// ignores it.
pub fn resolve(vendor: &str, model: Option<&str>, texture: Option<&str>) -> AssetFiles {
    let roots = asset_roots(vendor);
    resolve_in(&roots, model, texture)
}
pub fn resolve_in(roots: &[PathBuf], model: Option<&str>, texture: Option<&str>) -> AssetFiles {
    let ext_is = |name: &str, exts: &[&str]| {
        Path::new(name)
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| exts.iter().any(|x| e.eq_ignore_ascii_case(x)))
    };
    AssetFiles {
        model: model
            .filter(|m| ext_is(m, &["stl"]))
            .and_then(|m| find_in(roots, m)),
        texture: texture
            .filter(|t| ext_is(t, &["svg"]))
            .and_then(|t| find_in(roots, t)),
    }
}

/// Where the model's origin goes: the centre of the bed outline's bounding box, a little
/// below the bed plane.
pub fn model_offset(bed_min: Vec2, bed_max: Vec2) -> Vec3 {
    ((bed_min + bed_max) * 0.5).extend(MODEL_Z)
}

/// Texture coordinate of a bed point: the texture is stretched over the outline's bounding
/// box, image top at the back of the bed (+Y).
pub fn texture_uv(p: Vec2, bed_min: Vec2, bed_max: Vec2) -> [f32; 2] {
    let size = (bed_max - bed_min).max(Vec2::splat(1e-6));
    [(p.x - bed_min.x) / size.x, 1. - (p.y - bed_min.y) / size.y]
}

/// The textured bed quad: the outline polygon triangulated at `GROUND_Z`, each vertex
/// `[x, y, z, u, v]`.
pub fn texture_quad(
    polygon: &[[f32; 2]],
    bed_min: Vec2,
    bed_max: Vec2,
) -> (Vec<[f32; 5]>, Vec<[u32; 3]>) {
    let vertices = polygon
        .iter()
        .map(|p| {
            let [u, v] = texture_uv(Vec2::from(*p), bed_min, bed_max);
            [p[0], p[1], GROUND_Z, u, v]
        })
        .collect();
    (vertices, super::bed::triangulate(polygon))
}

/// A rasterised SVG, straight (non-premultiplied) RGBA8 rows from the top.
#[derive(Clone, Debug, PartialEq)]
pub struct Texture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Texture {
    /// Mean luminance (0..1) of the pixels that are not transparent.
    pub fn mean_luminance(&self) -> f32 {
        let (mut sum, mut n) = (0f64, 0u64);
        for px in self.rgba.as_chunks::<4>().0 {
            if px[3] > 8 {
                sum +=
                    (0.2126 * px[0] as f64 + 0.7152 * px[1] as f64 + 0.0722 * px[2] as f64) / 255.;
                n += 1;
            }
        }
        if n == 0 { 1. } else { (sum / n as f64) as f32 }
    }
    /// Whether the line art is dark (drawn for a light plate, like OrcaSlicer's).
    pub fn dark_marks(&self) -> bool {
        self.mean_luminance() < 0.5
    }
    /// Inverts the colour of every pixel, keeping alpha: dark marks become light ones so a
    /// texture drawn for a light plate stays readable on the dark theme.
    pub fn inverted(mut self) -> Self {
        for px in self.rgba.as_chunks_mut::<4>().0 {
            px[0] = 255 - px[0];
            px[1] = 255 - px[1];
            px[2] = 255 - px[2];
        }
        self
    }
    /// The texture as the theme needs it: dark line art is inverted for the dark theme.
    pub fn for_theme(self, dark: bool) -> Self {
        if dark && self.dark_marks() {
            self.inverted()
        } else {
            self
        }
    }
}

/// Rasterises an SVG so that its longer side is `max_side` pixels.
pub fn rasterize_svg(path: &Path, max_side: u32) -> Result<Texture, String> {
    let data = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    rasterize_svg_data(&data, max_side).map_err(|e| format!("{}: {e}", path.display()))
}
pub fn rasterize_svg_data(data: &[u8], max_side: u32) -> Result<Texture, String> {
    let tree = resvg::usvg::Tree::from_data(data, &resvg::usvg::Options::default())
        .map_err(|e| e.to_string())?;
    let size = tree.size();
    let (w, h) = (size.width().max(1.), size.height().max(1.));
    let scale = max_side.max(16) as f32 / w.max(h);
    let (pw, ph) = (
        (w * scale).round().max(1.) as u32,
        (h * scale).round().max(1.) as u32,
    );
    let mut pixmap =
        resvg::tiny_skia::Pixmap::new(pw, ph).ok_or_else(|| "texture too large".to_string())?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    let mut rgba = Vec::with_capacity((pw * ph * 4) as usize);
    for p in pixmap.pixels() {
        let c = p.demultiply();
        rgba.extend_from_slice(&[c.red(), c.green(), c.blue(), c.alpha()]);
    }
    Ok(Texture {
        width: pw,
        height: ph,
        rgba,
    })
}

/// Height of the model's dominant upward-facing surface (the sheet's top): the 0.1 mm bin of
/// upward face heights with the most area. PrusaSlicer's sheets have it at 0; OrcaSlicer's
/// `Snapmaker U1_bed.stl` has it at +1.35, which would hide a texture at `GROUND_Z`.
pub fn top_surface_z(mesh: &TriangleMesh) -> f32 {
    let mut bins: std::collections::BTreeMap<i64, (f32, f32)> = Default::default();
    for f in &mesh.faces {
        let [a, b, c] = f.map(|i| Vec3::from(mesh.vertices[i as usize]));
        let n = (b - a).cross(c - a);
        let area = n.length() * 0.5;
        if area <= 0. || n.z / n.length() < 0.9 {
            continue;
        }
        let z = (a.z + b.z + c.z) / 3.;
        let e = bins.entry((z * 10.).round() as i64).or_insert((0., z));
        e.0 += area;
        e.1 = e.1.max(a.z.max(b.z).max(c.z));
    }
    bins.values()
        .max_by(|x, y| x.0.total_cmp(&y.0))
        .map_or(0., |(_, z)| *z)
}

/// Where the model goes: `model_offset`, lowered when its top surface would otherwise reach
/// the texture quad, so the sheet always sits just under the texture.
pub fn model_offset_for(mesh: &TriangleMesh, bed_min: Vec2, bed_max: Vec2) -> Vec3 {
    let mut offset = model_offset(bed_min, bed_max);
    let top = top_surface_z(mesh);
    if top + offset.z > MODEL_Z {
        offset.z = MODEL_Z - top;
    }
    offset
}

/// The bed model mesh in printer coordinates (already moved by `model_offset_for`).
pub fn load_model(path: &Path, bed_min: Vec2, bed_max: Vec2) -> Result<TriangleMesh, String> {
    let mut mesh = TriangleMesh::load_stl(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let offset = model_offset_for(&mesh, bed_min, bed_max);
    for v in &mut mesh.vertices {
        v[0] += offset.x;
        v[1] += offset.y;
        v[2] += offset.z;
    }
    Ok(mesh)
}

/// Alpha for the texture's line art and the fallback grid: opaque from above, half from
/// below (PrusaSlicer's `transparent_background`), where the bed model is not drawn at all.
pub fn surface_alpha(from_below: bool) -> f32 {
    if from_below { SEE_THROUGH_ALPHA } else { 1. }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("mica-bed-assets-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn lookup_follows_root_order_and_refuses_paths() {
        let a = temp_dir("a");
        let b = temp_dir("b");
        std::fs::write(b.join("coreone_bed.stl"), b"x").unwrap();
        std::fs::write(a.join("coreone.svg"), b"<svg/>").unwrap();
        std::fs::write(b.join("coreone.svg"), b"<svg/>").unwrap();
        let roots = vec![a.clone(), b.clone()];
        let files = resolve_in(&roots, Some("coreone_bed.stl"), Some("coreone.svg"));
        assert_eq!(files.model, Some(b.join("coreone_bed.stl")));
        // The first root wins when both have the file.
        assert_eq!(files.texture, Some(a.join("coreone.svg")));
        assert!(find_in(&roots, "../coreone.svg").is_none());
        assert!(find_in(&roots, "/etc/passwd").is_none());
        assert!(find_in(&roots, "").is_none());
        // Wrong extensions and missing files are ignored, not errors.
        let none = resolve_in(&roots, Some("coreone.svg"), Some("coreone_bed.stl"));
        assert!(none.is_empty());
        assert!(resolve_in(&roots, Some("missing.stl"), None).is_empty());
        let _ = std::fs::remove_dir_all(a);
        let _ = std::fs::remove_dir_all(b);
    }

    #[test]
    fn roots_prefer_upstream_then_installed_apps() {
        let roots = asset_roots("PrusaResearch");
        // One separator, so the Windows form (`upstream\prusaslicer\…`) matches too.
        let s: Vec<String> = roots
            .iter()
            .map(|p| p.display().to_string().replace('\\', "/"))
            .collect();
        let upstream = s
            .iter()
            .position(|p| p.contains("upstream/prusaslicer/resources/profiles/PrusaResearch"))
            .unwrap();
        let orca = s
            .iter()
            .position(|p| p.contains("upstream/orca/resources/profiles/PrusaResearch"))
            .unwrap();
        let app = s
            .iter()
            .position(|p| p.contains("Original Prusa Drivers/PrusaSlicer.app"))
            .unwrap();
        assert!(upstream < orca && orca < app);
        assert!(s.iter().all(|p| p.ends_with("PrusaResearch")));
    }

    #[test]
    fn placement_matches_prusaslicer() {
        // CORE One: bed_shape 0x0 … 250x220 → model origin at the bed centre, 0.03 mm down.
        let (min, max) = (Vec2::ZERO, Vec2::new(250., 220.));
        assert_eq!(model_offset(min, max), Vec3::new(125., 110., -0.03));
        // XL: 360 × 360 → 180, 180.
        assert_eq!(
            model_offset(Vec2::ZERO, Vec2::new(360., 360.)).truncate(),
            Vec2::new(180., 180.)
        );
        // Texture: image top (v = 0) at the back of the bed, left at x = min.
        assert_eq!(texture_uv(Vec2::new(0., 220.), min, max), [0., 0.]);
        assert_eq!(texture_uv(Vec2::new(250., 0.), min, max), [1., 1.]);
        assert_eq!(texture_uv(Vec2::new(125., 110.), min, max), [0.5, 0.5]);
        // A centre-origin bed maps the same way.
        let (cmin, cmax) = (Vec2::new(-125., -110.), Vec2::new(125., 110.));
        assert_eq!(texture_uv(Vec2::new(-125., 110.), cmin, cmax), [0., 0.]);
        let (v, f) = texture_quad(&[[0., 0.], [250., 0.], [250., 220.], [0., 220.]], min, max);
        assert_eq!(v.len(), 4);
        assert_eq!(f.len(), 2);
        assert_eq!(v[2], [250., 220., GROUND_Z, 1., 0.]);
    }

    #[test]
    fn svg_rasterises_to_the_requested_side() {
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100" viewBox="0 0 200 100"><rect x="0" y="0" width="100" height="100" fill="#ffffff"/></svg>"##;
        let t = rasterize_svg_data(svg, 64).unwrap();
        assert_eq!((t.width, t.height), (64, 32));
        assert_eq!(t.rgba.len(), 64 * 32 * 4);
        // Left half white and opaque, right half transparent.
        assert_eq!(&t.rgba[0..4], &[255, 255, 255, 255]);
        let right = ((31 * 64 + 63) * 4) as usize;
        assert_eq!(t.rgba[right + 3], 0);
        assert!(rasterize_svg_data(b"not svg", 64).is_err());
        // White marks stay as they are on the dark theme; black marks get inverted.
        assert!(!t.dark_marks());
        let white = t.clone().for_theme(true);
        assert_eq!(&white.rgba[0..4], &[255, 255, 255, 255]);
        let black = br##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20"><rect width="10" height="20"/></svg>"##;
        let b = rasterize_svg_data(black, 20).unwrap();
        assert!(b.dark_marks());
        let flipped = b.clone().for_theme(true);
        assert_eq!(&flipped.rgba[0..4], &[255, 255, 255, 255]);
        assert_eq!(
            flipped.rgba[(19 * 4) + 3],
            0,
            "transparent pixels stay transparent"
        );
        assert_eq!(&b.for_theme(false).rgba[0..4], &[0, 0, 0, 255]);
    }

    #[test]
    fn model_is_moved_to_the_bed_centre() {
        let dir = temp_dir("stl");
        let path = dir.join("bed.stl");
        // One ASCII triangle around the origin.
        std::fs::write(
            &path,
            "solid bed\nfacet normal 0 0 1\nouter loop\nvertex -1 -1 0\nvertex 1 -1 0\nvertex 0 1 0\nendloop\nendfacet\nendsolid bed\n",
        )
        .unwrap();
        let mesh = load_model(&path, Vec2::ZERO, Vec2::new(250., 220.)).unwrap();
        let (lo, hi) = mesh.bounds();
        assert!((lo[0] - 124.).abs() < 1e-4 && (hi[0] - 126.).abs() < 1e-4);
        assert!((lo[1] - 109.).abs() < 1e-4 && (hi[1] - 111.).abs() < 1e-4);
        assert!((lo[2] - MODEL_Z).abs() < 1e-5);
        assert_eq!(surface_alpha(true), SEE_THROUGH_ALPHA);
        assert_eq!(surface_alpha(false), 1.);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_sheet_whose_top_is_above_the_texture_is_lowered_under_it() {
        // A 20 × 20 slab from z = -2 to z = +1.35 (like the U1 sheet) with a small raised tab.
        let slab = |z0: f32, z1: f32, s: f32| TriangleMesh {
            vertices: vec![
                [-s, -s, z0],
                [s, -s, z0],
                [s, s, z0],
                [-s, s, z0],
                [-s, -s, z1],
                [s, -s, z1],
                [s, s, z1],
                [-s, s, z1],
            ],
            faces: vec![
                [0, 2, 1],
                [0, 3, 2],
                [4, 5, 6],
                [4, 6, 7],
                [0, 1, 5],
                [0, 5, 4],
                [1, 2, 6],
                [1, 6, 5],
                [2, 3, 7],
                [2, 7, 6],
                [3, 0, 4],
                [3, 4, 7],
            ],
        };
        let mut mesh = slab(-2., 1.35, 10.);
        // A tiny tab higher up must not win: the dominant top is the big surface.
        let tab = slab(1.35, 3., 0.5);
        let base = mesh.vertices.len() as u32;
        mesh.vertices.extend(tab.vertices);
        mesh.faces
            .extend(tab.faces.iter().map(|f| f.map(|i| i + base)));
        assert!((top_surface_z(&mesh) - 1.35).abs() < 1e-5);
        let offset = model_offset_for(&mesh, Vec2::ZERO, Vec2::new(270., 270.));
        assert!((offset.z - (MODEL_Z - 1.35)).abs() < 1e-5, "{offset}");
        assert_eq!(offset.truncate(), Vec2::new(135., 135.));
        // PrusaSlicer's sheets (top at 0) keep PrusaSlicer's offset.
        let prusa = slab(-3.5, 0., 100.);
        assert_eq!(top_surface_z(&prusa), 0.);
        assert_eq!(
            model_offset_for(&prusa, Vec2::ZERO, Vec2::new(250., 220.)),
            model_offset(Vec2::ZERO, Vec2::new(250., 220.))
        );
    }

    /// The CORE One's bed assets from the pinned Prusa vendor profiles.
    #[test]
    fn installed_assets_when_available() {
        if !mica_testkit::require_fixture(
            mica_testkit::upstream("prusa-profiles/PrusaResearch/coreone_bed.stl"),
            "Prusa vendor profiles: scripts/fetch-upstream.sh prusa-profiles",
        ) {
            return;
        }
        let files = resolve(
            "PrusaResearch",
            Some("coreone_bed.stl"),
            Some("coreone.svg"),
        );
        assert!(!files.is_empty(), "no PrusaResearch bed assets found");
        let texture = rasterize_svg(files.texture.as_deref().unwrap(), 256).unwrap();
        assert_eq!(texture.width.max(texture.height), 256);
        let mesh = load_model(
            files.model.as_deref().unwrap(),
            Vec2::ZERO,
            Vec2::new(250., 220.),
        )
        .unwrap();
        let (lo, hi) = mesh.bounds();
        // The sheet is centred on the bed and a few millimetres thick.
        assert!((lo[0] + hi[0]) * 0.5 > 100. && (lo[0] + hi[0]) * 0.5 < 150.);
        assert!(hi[2] - lo[2] < 40.);
    }
}
