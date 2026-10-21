// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The models that come with Mica ("Add shape"): the original 3DBenchy and basic shapes made
//! here, procedurally.
//!
//! - **3DBenchy** is the unmodified single-part STL of the official repository
//!   (`CreativeTools/3DBenchy`, `Single-part/3DBenchy.stl`, sha256 `6ab57f1c…`), stored
//!   gzip-compressed in `assets/models/3DBenchy.stl.gz`; it is CC0 1.0 since 2025 (see
//!   THIRD_PARTY.md). Decompressed it is byte for byte the official file (a test checks the
//!   hash).
//! - **Shapes** are generated from their dimensions: closed, consistently oriented meshes
//!   with shared vertices (watertight and manifold), standing on z = 0, centred on the XY
//!   origin. Round shapes get enough segments to keep the chord error under
//!   [`CHORD_TOLERANCE`] (smooth at print resolution without huge meshes).
//!
//! Adding a model: a variant in [`BuiltinModel`] with its entry in each `match` (and in
//! [`BuiltinModel::ALL`], which is the gallery order); a file-based one goes in
//! `assets/models/` with a credit and a THIRD_PARTY.md row.

use crate::mesh::TriangleMesh;
use crate::mesh_edit;
use std::f64::consts::{PI, TAU};

/// Largest distance (mm) between a curved surface and its facets.
pub const CHORD_TOLERANCE: f64 = 0.01;

const BENCHY_GZ: &[u8] = include_bytes!("../assets/models/3DBenchy.stl.gz");

/// A model the user can put on the plate without a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BuiltinModel {
    Benchy,
    Cube,
    Cylinder,
    Cone,
    Sphere,
    Torus,
    Pyramid,
    Wedge,
    Tube,
    Disc,
}

impl BuiltinModel {
    /// Every model, in the gallery's order.
    pub const ALL: [Self; 10] = [
        Self::Benchy,
        Self::Cube,
        Self::Cylinder,
        Self::Cone,
        Self::Sphere,
        Self::Torus,
        Self::Pyramid,
        Self::Wedge,
        Self::Tube,
        Self::Disc,
    ];

    /// Stable lowercase id (MCP arguments, cache keys).
    pub fn id(self) -> &'static str {
        match self {
            Self::Benchy => "benchy",
            Self::Cube => "cube",
            Self::Cylinder => "cylinder",
            Self::Cone => "cone",
            Self::Sphere => "sphere",
            Self::Torus => "torus",
            Self::Pyramid => "pyramid",
            Self::Wedge => "wedge",
            Self::Tube => "tube",
            Self::Disc => "disc",
        }
    }

    /// The object's name on the plate and in saved projects.
    pub fn name(self) -> &'static str {
        match self {
            Self::Benchy => "3DBenchy",
            Self::Cube => "Cube",
            Self::Cylinder => "Cylinder",
            Self::Cone => "Cone",
            Self::Sphere => "Sphere",
            Self::Torus => "Torus",
            Self::Pyramid => "Pyramid",
            Self::Wedge => "Wedge",
            Self::Tube => "Tube",
            Self::Disc => "Disc",
        }
    }

    /// The short label under a gallery tile.
    pub fn label(self) -> &'static str {
        match self {
            Self::Benchy => "Benchy",
            other => other.name(),
        }
    }

    /// What it is, for tooltips.
    pub fn description(self) -> &'static str {
        match self {
            Self::Benchy => {
                "The classic test boat: overhangs, bridges, small details and smooth curves"
            }
            Self::Cube => "20 mm cube",
            Self::Cylinder => "Ø 20 × 20 mm",
            Self::Cone => "Ø 20 × 20 mm, pointed top",
            Self::Sphere => "Ø 20 mm",
            Self::Torus => "Ring, Ø 30 × 10 mm",
            Self::Pyramid => "Square base 20 mm, 20 mm tall",
            Self::Wedge => "Ramp, 20 × 20 × 20 mm (a 45° slope)",
            Self::Tube => "Ø 20 × 20 mm, 3 mm wall",
            Self::Disc => "Ø 40 × 2 mm: first layer and flow checks",
        }
    }

    /// Who made it, when it is not Mica's own.
    pub fn credit(self) -> Option<&'static str> {
        match self {
            Self::Benchy => Some(
                "#3DBenchy by Creative Tools (designed by Daniel Norée and Paulo Kiefe), \
                 3dbenchy.com. Public domain (CC0 1.0).",
            ),
            _ => None,
        }
    }

    /// Finds a model by id, name or a common alias (any case).
    pub fn parse(s: &str) -> Option<Self> {
        let key: String = s
            .trim()
            .to_ascii_lowercase()
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .collect();
        if let Some(m) = Self::ALL
            .into_iter()
            .find(|m| m.id() == key || m.name().eq_ignore_ascii_case(&key))
        {
            return Some(m);
        }
        Some(match key.as_str() {
            "3dbenchy" | "boat" => Self::Benchy,
            "box" | "cuboid" => Self::Cube,
            "ball" => Self::Sphere,
            "ring" | "donut" => Self::Torus,
            "ramp" => Self::Wedge,
            "pipe" | "hollowcylinder" => Self::Tube,
            "calibrationdisc" | "disk" => Self::Disc,
            _ => return None,
        })
    }

    /// Its size as it comes (bounding box, mm).
    pub fn default_size(self) -> [f32; 3] {
        match self {
            Self::Benchy => {
                static SIZE: std::sync::OnceLock<[f32; 3]> = std::sync::OnceLock::new();
                *SIZE.get_or_init(|| {
                    let (lo, hi) = benchy()
                        .map(|m| m.bounds())
                        .unwrap_or(([0.; 3], [60., 31., 48.]));
                    std::array::from_fn(|k| hi[k] - lo[k])
                })
            }
            Self::Torus => [30., 30., 10.],
            Self::Disc => [40., 40., 2.],
            _ => [20., 20., 20.],
        }
    }

    /// The model at its default size, standing on z = 0, centred on the XY origin.
    pub fn mesh(self) -> Result<TriangleMesh, String> {
        self.mesh_sized(self.default_size())
    }

    /// The model with the bounding box `size` (mm, each at least 0.1), standing on z = 0,
    /// centred on the XY origin. Shapes are generated at that size (their tessellation
    /// follows); the 3DBenchy is scaled.
    pub fn mesh_sized(self, size: [f32; 3]) -> Result<TriangleMesh, String> {
        if size
            .iter()
            .any(|s| !s.is_finite() || *s < 0.1 || *s > 10_000.)
        {
            return Err(format!(
                "a size between 0.1 and 10000 mm is needed on every axis, not {size:?}"
            ));
        }
        let [sx, sy, sz] = size.map(f64::from);
        let round = sx.max(sy) / 2.;
        let mut mesh = match self {
            Self::Benchy => benchy()?,
            Self::Cube => mesh_edit::cuboid(size),
            Self::Cylinder | Self::Disc => cylinder(round, sz),
            Self::Cone => cone(round, sz),
            Self::Sphere => sphere(sx.max(sy).max(sz) / 2.),
            Self::Torus => {
                let minor = sz / 2.;
                torus((round - minor).max(minor * 1.05), minor)
            }
            Self::Pyramid => pyramid(round, sz),
            Self::Wedge => wedge(size),
            Self::Tube => {
                let wall = 3f64.min(round * 0.45);
                tube(round, round - wall, sz)
            }
        };
        fit(&mut mesh, size);
        Ok(mesh)
    }
}

/// The official 3DBenchy, decompressed and parsed.
fn benchy() -> Result<TriangleMesh, String> {
    TriangleMesh::from_stl_bytes(&benchy_stl()?).map_err(|e| format!("3DBenchy: {e}"))
}

/// The bytes of the official `3DBenchy.stl`.
pub fn benchy_stl() -> Result<Vec<u8>, String> {
    use std::io::Read;
    let mut out = Vec::with_capacity(11_300_000);
    flate2::read::GzDecoder::new(BENCHY_GZ)
        .read_to_end(&mut out)
        .map_err(|e| format!("3DBenchy: {e}"))?;
    Ok(out)
}

/// Scales and moves `mesh` so its bounding box is `size`, on z = 0, centred on the XY origin.
fn fit(mesh: &mut TriangleMesh, size: [f32; 3]) {
    let (lo, hi) = mesh.bounds();
    let scale: [f64; 3] = std::array::from_fn(|k| {
        let d = f64::from(hi[k] - lo[k]);
        if d > 0. { f64::from(size[k]) / d } else { 1. }
    });
    let centre: [f64; 3] = std::array::from_fn(|k| f64::from(lo[k] + hi[k]) / 2.);
    for v in &mut mesh.vertices {
        for k in 0..3 {
            let c = (f64::from(v[k]) - centre[k]) * scale[k];
            v[k] = (if k == 2 {
                c + f64::from(size[2]) / 2.
            } else {
                c
            }) as f32;
        }
    }
}

/// Segments around a circle of `radius` for [`CHORD_TOLERANCE`]: a multiple of 4 between
/// 24 and 256.
pub fn segments(radius: f64) -> usize {
    let n = if radius <= CHORD_TOLERANCE {
        24.
    } else {
        (PI / (1. - CHORD_TOLERANCE / radius).acos()).ceil()
    };
    ((n as usize).clamp(24, 256)).div_ceil(4) * 4
}

/// A surface of revolution about Z: `profile` is a polyline in the (r, z) half-plane,
/// counter-clockwise (r to the right, z up), so the surface faces outward. Points on the axis
/// (r = 0) become single pole vertices; with `closed` the last point joins the first.
/// `phase` turns the first meridian (radians).
fn revolve(profile: &[(f64, f64)], segments: usize, closed: bool, phase: f64) -> TriangleMesh {
    let n = segments.max(3);
    let mut vertices: Vec<[f32; 3]> = vec![];
    // Index of point i at meridian j.
    let mut first: Vec<(u32, bool)> = vec![];
    for &(r, z) in profile {
        let pole = r.abs() < 1e-12;
        first.push((vertices.len() as u32, pole));
        if pole {
            vertices.push([0., 0., z as f32]);
        } else {
            for j in 0..n {
                let (s, c) = (phase + TAU * j as f64 / n as f64).sin_cos();
                vertices.push([(r * c) as f32, (r * s) as f32, z as f32]);
            }
        }
    }
    let at = |i: usize, j: usize| {
        let (base, pole) = first[i];
        if pole { base } else { base + (j % n) as u32 }
    };
    let spans = if closed {
        profile.len()
    } else {
        profile.len() - 1
    };
    let mut faces = Vec::with_capacity(spans * n * 2);
    for i in 0..spans {
        let k = (i + 1) % profile.len();
        for j in 0..n {
            let (a, b, c, d) = (at(i, j), at(k, j), at(k, j + 1), at(i, j + 1));
            if a != d {
                faces.push([a, d, c]);
            }
            if b != c {
                faces.push([a, c, b]);
            }
        }
    }
    TriangleMesh { vertices, faces }
}

fn cylinder(r: f64, h: f64) -> TriangleMesh {
    revolve(
        &[(0., 0.), (r, 0.), (r, h), (0., h)],
        segments(r),
        false,
        0.,
    )
}

fn cone(r: f64, h: f64) -> TriangleMesh {
    revolve(&[(0., 0.), (r, 0.), (0., h)], segments(r), false, 0.)
}

fn sphere(r: f64) -> TriangleMesh {
    let n = segments(r);
    let rings = n / 2;
    let profile: Vec<(f64, f64)> = (0..=rings)
        .map(|k| {
            let phi = -PI / 2. + PI * k as f64 / rings as f64;
            let (s, c) = phi.sin_cos();
            // Exactly on the axis at the poles.
            let r_k = if k == 0 || k == rings { 0. } else { r * c };
            (r_k, r + r * s)
        })
        .collect();
    revolve(&profile, n, false, 0.)
}

fn torus(major: f64, minor: f64) -> TriangleMesh {
    let m = segments(minor);
    let profile: Vec<(f64, f64)> = (0..m)
        .map(|k| {
            let (s, c) = (TAU * k as f64 / m as f64 - PI / 2.).sin_cos();
            (major + minor * c, minor + minor * s)
        })
        .collect();
    revolve(&profile, segments(major + minor), true, 0.)
}

fn tube(outer: f64, inner: f64, h: f64) -> TriangleMesh {
    revolve(
        &[(inner, 0.), (outer, 0.), (outer, h), (inner, h)],
        segments(outer),
        true,
        0.,
    )
}

/// A square pyramid: four meridians turned 45°, so the base edges run along X and Y.
fn pyramid(half_side: f64, h: f64) -> TriangleMesh {
    let corner = half_side * std::f64::consts::SQRT_2;
    revolve(&[(0., 0.), (corner, 0.), (0., h)], 4, false, PI / 4.)
}

/// A ramp: full height at the back (+Y), down to the bed at the front.
fn wedge(size: [f32; 3]) -> TriangleMesh {
    let [x, y, _] = size.map(|s| s / 2.);
    let h = size[2];
    // 0..2: left side (front bottom, back bottom, back top); 3..5: right side.
    let vertices = vec![
        [-x, -y, 0.],
        [-x, y, 0.],
        [-x, y, h],
        [x, -y, 0.],
        [x, y, 0.],
        [x, y, h],
    ];
    let faces = vec![
        // Left (-X) and right (+X) triangles.
        [0, 2, 1],
        [3, 4, 5],
        // Bottom.
        [0, 1, 4],
        [0, 4, 3],
        // Back (+Y).
        [1, 2, 5],
        [1, 5, 4],
        // Slope.
        [0, 3, 5],
        [0, 5, 2],
    ];
    TriangleMesh { vertices, faces }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Every edge used exactly once in each direction (closed, consistently oriented, no
    /// edge shared by more than two triangles), no degenerate faces, positive volume.
    fn assert_manifold(m: &TriangleMesh, what: &str) {
        let mut edges: HashMap<(u32, u32), u32> = HashMap::new();
        for f in &m.faces {
            assert!(
                f[0] != f[1] && f[1] != f[2] && f[0] != f[2],
                "{what}: degenerate face"
            );
            for k in 0..3 {
                *edges.entry((f[k], f[(k + 1) % 3])).or_default() += 1;
            }
        }
        for (&(a, b), &n) in &edges {
            assert_eq!(n, 1, "{what}: edge {a}-{b} used {n} times one way");
            assert_eq!(
                edges.get(&(b, a)),
                Some(&1),
                "{what}: edge {a}-{b} has no twin"
            );
        }
        assert!(crate::mesh_repair::check(m).is_clean(), "{what}");
        // No unused or duplicate vertices.
        let mut used = vec![false; m.vertices.len()];
        for f in &m.faces {
            for &i in f {
                used[i as usize] = true;
            }
        }
        assert!(used.iter().all(|u| *u), "{what}: unused vertices");
    }

    fn analytic_volume(m: BuiltinModel) -> f64 {
        let (r, h) = (10., 20.);
        match m {
            BuiltinModel::Benchy => unreachable!(),
            BuiltinModel::Cube => 8000.,
            BuiltinModel::Cylinder => PI * r * r * h,
            BuiltinModel::Cone => PI * r * r * h / 3.,
            BuiltinModel::Sphere => 4. / 3. * PI * r * r * r,
            BuiltinModel::Torus => 2. * PI * PI * 10. * 25.,
            BuiltinModel::Pyramid => 400. * h / 3.,
            BuiltinModel::Wedge => 8000. / 2.,
            BuiltinModel::Tube => PI * (100. - 49.) * h,
            BuiltinModel::Disc => PI * 400. * 2.,
        }
    }

    #[test]
    fn shapes_are_closed_manifold_and_match_their_volume() {
        for m in BuiltinModel::ALL {
            if m == BuiltinModel::Benchy {
                continue;
            }
            let mesh = m.mesh().unwrap();
            assert_manifold(&mesh, m.name());
            let v = mesh.volume();
            let want = analytic_volume(m);
            assert!(
                (v - want).abs() / want < 0.005,
                "{}: volume {v:.1} vs {want:.1}",
                m.name()
            );
            // Standing on the bed, centred, at its stated size.
            let (lo, hi) = mesh.bounds();
            let size = m.default_size();
            for k in 0..3 {
                assert!(((hi[k] - lo[k]) - size[k]).abs() < 1e-3, "{}", m.name());
            }
            assert!(lo[2].abs() < 1e-5);
            assert!((lo[0] + hi[0]).abs() < 1e-3 && (lo[1] + hi[1]).abs() < 1e-3);
            // Smooth but small.
            assert!(
                mesh.faces.len() < 20_000,
                "{}: {}",
                m.name(),
                mesh.faces.len()
            );
        }
    }

    #[test]
    fn round_shapes_keep_the_chord_error_small() {
        assert!(segments(10.) >= 64);
        for r in [0.5, 2., 10., 20., 100., 1000.] {
            let n = segments(r) as f64;
            let sag = r * (1. - (PI / n).cos());
            assert!(sag <= CHORD_TOLERANCE + 1e-9 || n >= 256., "r {r}: {sag}");
        }
    }

    #[test]
    fn shapes_slice_into_clean_layers() {
        let zs: Vec<f64> = (0..100).map(|k| 0.1 + 0.2 * k as f64).collect();
        for m in BuiltinModel::ALL {
            if m == BuiltinModel::Benchy {
                continue;
            }
            let mesh = m.mesh().unwrap();
            let top = f64::from(mesh.bounds().1[2]);
            let layers = crate::slice::slice_mesh(&mesh, &zs, 0.);
            for (z, l) in zs.iter().zip(&layers) {
                assert_eq!(l.dropped_chains, 0, "{} at {z}", m.name());
                if *z < top - 1e-3 {
                    assert!(!l.islands.is_empty(), "{} at {z}: empty", m.name());
                    let area: f64 = l.islands.iter().map(|i| i.area_mm2()).sum();
                    assert!(area > 0., "{} at {z}", m.name());
                }
            }
        }
        // The tube's layers have a hole, the torus's mid layer too.
        let tube = BuiltinModel::Tube.mesh().unwrap();
        let l = &crate::slice::slice_mesh(&tube, &[10.], 0.)[0];
        assert_eq!(l.islands.len(), 1);
        assert_eq!(l.islands[0].holes.len(), 1);
        let a = l.islands[0].area_mm2();
        assert!((a - PI * 51.).abs() / (PI * 51.) < 0.005, "{a}");
    }

    #[test]
    fn sized_shapes_follow_the_size_and_stay_closed() {
        for m in BuiltinModel::ALL {
            let size = [35., 25., 12.];
            let mesh = m.mesh_sized(size).unwrap();
            let (lo, hi) = mesh.bounds();
            for k in 0..3 {
                assert!(
                    ((hi[k] - lo[k]) - size[k]).abs() < 1e-2,
                    "{} axis {k}",
                    m.name()
                );
            }
            if m != BuiltinModel::Benchy {
                assert_manifold(&mesh, m.name());
            }
        }
        assert!(BuiltinModel::Cube.mesh_sized([0., 1., 1.]).is_err());
        assert!(BuiltinModel::Cube.mesh_sized([f32::NAN, 1., 1.]).is_err());
    }

    #[test]
    fn names_parse_back() {
        for m in BuiltinModel::ALL {
            assert_eq!(BuiltinModel::parse(m.id()), Some(m));
            assert_eq!(BuiltinModel::parse(m.name()), Some(m));
            assert_eq!(BuiltinModel::parse(&m.label().to_uppercase()), Some(m));
        }
        assert_eq!(BuiltinModel::parse("box"), Some(BuiltinModel::Cube));
        assert_eq!(BuiltinModel::parse("Ramp"), Some(BuiltinModel::Wedge));
        assert_eq!(BuiltinModel::parse("3D Benchy"), Some(BuiltinModel::Benchy));
        assert_eq!(BuiltinModel::parse("teapot"), None);
    }

    #[test]
    fn the_benchy_is_the_official_file_unmodified() {
        use sha2::{Digest, Sha256};
        let stl = benchy_stl().unwrap();
        assert_eq!(stl.len(), 11_285_384);
        let hash: String = Sha256::digest(&stl)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(
            hash,
            "6ab57f1c3f8e86bc3cbd302c6fa6270acf06277c6335454e922419c25d42e97e"
        );
        let mesh = BuiltinModel::Benchy.mesh().unwrap();
        // 225,706 records; the loader drops the degenerate ones.
        assert!(mesh.faces.len() > 225_000);
        assert!(crate::mesh_repair::check(&mesh).is_clean());
        let [x, y, z] = BuiltinModel::Benchy.default_size();
        assert!((x - 60.).abs() < 0.5 && (y - 31.).abs() < 0.5 && (z - 48.).abs() < 0.5);
        assert!(BuiltinModel::Benchy.credit().unwrap().contains("CC0"));
    }
}
