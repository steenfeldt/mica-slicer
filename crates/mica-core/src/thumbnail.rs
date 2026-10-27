// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/slic3r/GUI/GLCanvas3D.cpp :: GLCanvas3D::_render_thumbnail_internal
// Source: prusaslicer@b028299c77 src/slic3r/GUI/Camera.cpp :: Camera::set_default_orientation, Camera::calc_zoom_to_bounding_box_factor
// Source: prusaslicer@b028299c77 resources/shaders/110/gouraud_light.vs :: main
// Source: orca@8500fcdcca src/slic3r/GUI/GLCanvas3D.cpp :: GLCanvas3D::render_thumbnail_internal
// Source: snapmaker-orca@b1831e5dcb src/slic3r/GUI/GLCanvas3D.cpp :: GLCanvas3D::render_thumbnail_internal
// Changes: Original CPU z-buffer rasterizer; upstream ortho camera/framing and lighting, flat normals,
// Changes: parallel viewing rays for specular, 2x supersampling, transparent background, one placed mesh;
// Changes: no bed, modifiers, painting, GPU or perspective. U1 uses current Orca's 2% Z padding.
// Changes: An original orbit camera (any azimuth/elevation, box or sphere fit) for the model library.
// Changes: Optional per-face colours (the library preview's filament colours).
// Source: orca@8500fcdcca resources/shaders/110/thumbnail.vs :: main
// Source: orca@8500fcdcca resources/shaders/110/thumbnail.fs :: main
// Changes: Moved from mica-gcode into mica-core so the 3MF writer renders project thumbnails;
// Changes: the z-buffer is shared (`Raster`, `Frame`): several draws into one picture, any
// Changes: per-sample value, and Orca's thumbnail-shader emission term as an option.
// Upstream attribution (AGPLv3 or higher):
// Copyright (c) Prusa Research 2018-2023 Enrico Turri, Oleksandra Iushchenko, Tomáš Mészáros,
// Lukáš Matěna, Vojtěch Bubník, David Kocík, Filip Sykala, Lukáš Hejl, Vojtěch Král.
// Copyright (c) BambuStudio 2023 manch1n; SuperSlicer 2023 Remi Durand.
// Copyright (c) 2020 Benjamin Greiner; 2019 John Drake, BeldrothTheGold, Thomas Moore.

//! The CPU thumbnail renderer: G-code thumbnails, 3MF project thumbnails
//! ([`crate::threemf`]) and the model library's pictures.

use std::io;

use crate::mesh::TriangleMesh;

/// Both use the default 45-degree azimuth/zenith orthographic camera. Orca expands the model box.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Camera {
    Prusa,
    Orca,
}

/// Top-to-bottom, straight (not premultiplied) RGBA8 pixels.
#[derive(Clone, Debug)]
pub struct Image {
    pub width: u16,
    pub height: u16,
    pub rgba: Vec<u8>,
}

const S: f32 = std::f32::consts::FRAC_1_SQRT_2;

// Rows of Rx(-45 degrees) * Rz(45 degrees), as in Camera::set_default_orientation.
fn view([x, y, z]: [f32; 3]) -> [f32; 3] {
    [S * (x - y), 0.5 * (x + y) + S * z, -0.5 * (x + y) + S * z]
}

/// Render a bed-positioned mesh. Sizes follow Prusa's thumbnail limit: 1..=999 pixels per side.
/// Memory is 12 bytes per source vertex plus bounded image/depth buffers; no per-face allocation.
pub fn render(
    mesh: &TriangleMesh,
    width: u16,
    height: u16,
    color: [u8; 3],
    camera: Camera,
) -> io::Result<Image> {
    if width == 0 || height == 0 || width >= 1000 || height >= 1000 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "thumbnail dimensions must be between 1 and 999",
        ));
    }
    if mesh.vertices.is_empty() || mesh.faces.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "cannot render an empty mesh",
        ));
    }
    let (lo, hi) = mesh.bounds();
    let frame = Frame::iso(lo, hi, width, height, camera)?;
    rasterize(
        mesh,
        width,
        height,
        |_| color,
        view,
        frame.center,
        frame.zoom,
    )
}

/// How [`render_orbit`] frames the mesh.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fit {
    /// The bounding box as seen from the camera fills the picture (thumbnails).
    Box,
    /// The bounding sphere fills it: the scale stays put while the view turns.
    Sphere,
}

/// An orbit camera around the mesh's bounding-box centre (orthographic). Azimuth 0 looks
/// from the front (-Y towards +Y), 90 from the right (+X); elevation 90 looks from above.
/// Degrees. `zoom` > 1 magnifies.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Orbit {
    pub azimuth: f32,
    pub elevation: f32,
    pub zoom: f32,
    pub fit: Fit,
}

/// Renders the mesh from an orbit camera, with the same rasterizer, lighting (fixed to the
/// camera) and transparent background as [`render`].
pub fn render_orbit(
    mesh: &TriangleMesh,
    width: u16,
    height: u16,
    color: [u8; 3],
    orbit: Orbit,
) -> io::Result<Image> {
    render_orbit_faces(mesh, width, height, |_| color, orbit)
}

/// [`render_orbit`] with a colour per face (`face_colors[i]` for `mesh.faces[i]`; faces
/// without an entry use the last colour, or grey for an empty list).
pub fn render_orbit_colored(
    mesh: &TriangleMesh,
    width: u16,
    height: u16,
    face_colors: &[[u8; 3]],
    orbit: Orbit,
) -> io::Result<Image> {
    let fallback = face_colors.last().copied().unwrap_or([0x80; 3]);
    render_orbit_faces(
        mesh,
        width,
        height,
        |i| face_colors.get(i).copied().unwrap_or(fallback),
        orbit,
    )
}

fn render_orbit_faces(
    mesh: &TriangleMesh,
    width: u16,
    height: u16,
    color: impl Fn(usize) -> [u8; 3],
    orbit: Orbit,
) -> io::Result<Image> {
    if width == 0 || height == 0 || width >= 1000 || height >= 1000 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "thumbnail dimensions must be between 1 and 999",
        ));
    }
    if mesh.vertices.is_empty() || mesh.faces.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "cannot render an empty mesh",
        ));
    }
    let (az, el) = (orbit.azimuth.to_radians(), orbit.elevation.to_radians());
    let right = [az.cos(), az.sin(), 0.0];
    // Towards the camera; `right`, `up` and `back` form a right-handed basis.
    let back = [az.sin() * el.cos(), -az.cos() * el.cos(), el.sin()];
    let up = [
        back[1] * right[2] - back[2] * right[1],
        back[2] * right[0] - back[0] * right[2],
        back[0] * right[1] - back[1] * right[0],
    ];
    let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let view = move |p: [f32; 3]| [dot(p, right), dot(p, up), dot(p, back)];
    let (lo, hi) = mesh.bounds();
    let center: [f32; 3] = std::array::from_fn(|i| lo[i] * 0.5 + hi[i] * 0.5);
    let (dx, dy) = match orbit.fit {
        Fit::Box => {
            let (mut mx, mut my) = (0f32, 0f32);
            for c in 0..8 {
                let corner: [f32; 3] = std::array::from_fn(|i| {
                    (if (c >> i) & 1 == 1 { hi[i] } else { lo[i] }) - center[i]
                });
                let [x, y, _] = view(corner);
                mx = mx.max(x.abs());
                my = my.max(y.abs());
            }
            (2.0 * mx, 2.0 * my)
        }
        Fit::Sphere => {
            let size: [f32; 3] = std::array::from_fn(|i| hi[i] - lo[i]);
            let d = dot(size, size).sqrt();
            (d, d)
        }
    };
    let (w, h) = (usize::from(width) * 2, usize::from(height) * 2);
    let zoom = (w as f32 / dx).min(h as f32 / dy) / 1.025 * orbit.zoom;
    rasterize(mesh, width, height, color, view, center, zoom)
}

/// Draws the mesh seen through `view` (a rotation into camera space: x right, y up, z towards
/// the camera) around `center`, `zoom` supersampled pixels per millimetre.
fn rasterize(
    mesh: &TriangleMesh,
    width: u16,
    height: u16,
    color: impl Fn(usize) -> [u8; 3],
    view: impl Fn([f32; 3]) -> [f32; 3],
    center: [f32; 3],
    zoom: f32,
) -> io::Result<Image> {
    let frame = Frame::new(width, height, center, zoom)?;
    let vertices: Vec<[f32; 3]> = mesh
        .vertices
        .iter()
        .map(|&v| frame.project(&view, v))
        .collect();
    let mut raster = Raster::new(&frame, [0u8; 4]);
    raster.draw(
        &vertices,
        &mesh.faces,
        |_| false,
        |index, normal| shade(normal, color(index), 0.),
    )?;
    Ok(raster.resolve(width, height))
}

/// Supersampled screen placement: 2x2 samples per pixel, `zoom` samples per millimetre around
/// `center`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Frame {
    pub(crate) w: usize,
    pub(crate) h: usize,
    center: [f32; 3],
    zoom: f32,
}

impl Frame {
    pub(crate) fn new(width: u16, height: u16, center: [f32; 3], zoom: f32) -> io::Result<Self> {
        if !zoom.is_finite() || zoom <= 0.0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "mesh has invalid or degenerate bounds",
            ));
        }
        Ok(Self {
            w: usize::from(width) * 2,
            h: usize::from(height) * 2,
            center,
            zoom,
        })
    }

    /// The default orthographic thumbnail camera of [`render`], framed to the box `lo..hi`.
    pub(crate) fn iso(
        mut lo: [f32; 3],
        mut hi: [f32; 3],
        width: u16,
        height: u16,
        camera: Camera,
    ) -> io::Result<Self> {
        if camera == Camera::Orca {
            lo[2] = -0.0001; // BuildVolume::SceneEpsilon; thumbnail models are on the bed.
            for axis in 0..3 {
                let margin = (hi[axis] - lo[axis]) * if axis == 2 { 0.02 } else { 0.01 };
                lo[axis] -= margin;
                hi[axis] += margin;
            }
        }
        let center: [f32; 3] = std::array::from_fn(|i| lo[i] * 0.5 + hi[i] * 0.5);
        let size: [f32; 3] = std::array::from_fn(|i| hi[i] - lo[i]);
        let dx = S * (size[0] + size[1]);
        let dy = 0.5 * (size[0] + size[1]) + S * size[2];
        let (w, h) = (usize::from(width) * 2, usize::from(height) * 2);
        let zoom = (w as f32 / dx).min(h as f32 / dy) / 1.025;
        Self::new(width, height, center, zoom)
    }

    /// Screen position (samples, y down) and depth (towards the camera) of a point seen
    /// through `view`.
    pub(crate) fn project(&self, view: impl Fn([f32; 3]) -> [f32; 3], v: [f32; 3]) -> [f32; 3] {
        let [x, y, z] = view(std::array::from_fn(|i| v[i] - self.center[i]));
        [
            self.w as f32 * 0.5 + x * self.zoom,
            self.h as f32 * 0.5 - y * self.zoom,
            z * self.zoom,
        ]
    }
}

/// The rotation of [`render`]'s default camera.
pub(crate) fn iso_view(v: [f32; 3]) -> [f32; 3] {
    view(v)
}

/// A supersampled z-buffer holding one value per sample.
pub(crate) struct Raster<P> {
    pub(crate) w: usize,
    pub(crate) h: usize,
    pub(crate) depth: Vec<f32>,
    pub(crate) pixels: Vec<P>,
}

impl<P: Copy> Raster<P> {
    pub(crate) fn new(frame: &Frame, empty: P) -> Self {
        Self {
            w: frame.w,
            h: frame.h,
            depth: vec![f32::NEG_INFINITY; frame.w * frame.h],
            pixels: vec![empty; frame.w * frame.h],
        }
    }

    /// Draws the front-facing `faces` of projected `vertices` (see [`Frame::project`]), except
    /// those `skip` names. `shade` gives a face's value from its index and its camera-space
    /// normal; it is asked once per face, and only when the face covers a sample.
    pub(crate) fn draw(
        &mut self,
        vertices: &[[f32; 3]],
        faces: &[[u32; 3]],
        skip: impl Fn(usize) -> bool,
        mut shade: impl FnMut(usize, [f32; 3]) -> P,
    ) -> io::Result<()> {
        if vertices.iter().flatten().any(|v| !v.is_finite()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "mesh has non-finite coordinates",
            ));
        }
        let (w, h) = (self.w, self.h);
        let (depth, pixels) = (&mut self.depth, &mut self.pixels);
        for (index, face) in faces.iter().enumerate() {
            let vertex = |i: u32| {
                vertices.get(i as usize).copied().ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "mesh face has an invalid vertex index",
                    )
                })
            };
            let [a, b, c] = [vertex(face[0])?, vertex(face[1])?, vertex(face[2])?];
            if skip(index) {
                continue;
            }
            let ab: [f32; 3] = std::array::from_fn(|i| b[i] - a[i]);
            let ac: [f32; 3] = std::array::from_fn(|i| c[i] - a[i]);
            let n = [
                ab[1] * ac[2] - ab[2] * ac[1],
                ab[2] * ac[0] - ab[0] * ac[2],
                ab[0] * ac[1] - ab[1] * ac[0],
            ];
            // Screen Y points down, so outward-facing triangles have negative signed area.
            if n[2] >= 0.0 {
                continue;
            }
            let min_x = (a[0].min(b[0]).min(c[0]) - 0.5).ceil().max(0.0) as usize;
            let max_x = (a[0].max(b[0]).max(c[0]) - 0.5).floor().min(w as f32 - 1.0) as usize;
            let min_y = (a[1].min(b[1]).min(c[1]) - 0.5).ceil().max(0.0) as usize;
            let max_y = (a[1].max(b[1]).max(c[1]) - 0.5).floor().min(h as f32 - 1.0) as usize;
            // Safe subpixel rejection: skip only if the bounds contain no sample centres.
            if min_x > max_x || min_y > max_y {
                continue;
            }
            let edges = [(a, b), (b, c), (c, a)];
            let step_x = edges.map(|(a, b)| a[1] - b[1]);
            let step_y = edges.map(|(a, b)| b[0] - a[0]);
            let mut row = edges.map(|(a, b)| {
                (b[0] - a[0]) * (min_y as f32 + 0.5 - a[1])
                    - (b[1] - a[1]) * (min_x as f32 + 0.5 - a[0])
            });
            let dzdx = -n[0] / n[2];
            let dzdy = -n[1] / n[2];
            let mut row_z =
                a[2] + dzdx * (min_x as f32 + 0.5 - a[0]) + dzdy * (min_y as f32 + 0.5 - a[1]);
            let mut shaded = None;
            for y in min_y..=max_y {
                let mut e = row;
                let mut z = row_z;
                for x in min_x..=max_x {
                    let i = y * w + x;
                    if e[0] <= 0.0 && e[1] <= 0.0 && e[2] <= 0.0 && z > depth[i] {
                        depth[i] = z;
                        // Undo the screen-space reflection for lighting in camera space.
                        pixels[i] =
                            *shaded.get_or_insert_with(|| shade(index, [-n[0], n[1], -n[2]]));
                    }
                    for k in 0..3 {
                        e[k] += step_x[k];
                    }
                    z += dzdx;
                }
                for k in 0..3 {
                    row[k] += step_y[k];
                }
                row_z += dzdy;
            }
        }
        Ok(())
    }
}

impl Raster<[u8; 4]> {
    /// Each pixel: the average of its 2x2 opaque samples (alpha 0: empty).
    fn resolve(&self, width: u16, height: u16) -> Image {
        let (w, pixels) = (self.w, &self.pixels);
        let mut rgba = Vec::with_capacity(usize::from(width) * usize::from(height) * 4);
        for y in 0..usize::from(height) {
            for x in 0..usize::from(width) {
                let i = 2 * y * w + 2 * x;
                let samples = [pixels[i], pixels[i + 1], pixels[i + w], pixels[i + w + 1]];
                let count = samples.iter().filter(|p| p[3] != 0).count() as u16;
                // Unpremultiply edge colors so transparent pixels don't produce dark fringes.
                for channel in 0..3 {
                    let sum: u16 = samples.iter().map(|p| u16::from(p[channel])).sum();
                    rgba.push((sum + count / 2).checked_div(count).unwrap_or(0) as u8);
                }
                rgba.push(((count * 255 + 2) / 4) as u8);
            }
        }
        Image {
            width,
            height,
            rgba,
        }
    }
}

/// Flat two-light shading of `color` for a camera-space `normal`. `emission` adds to the
/// diffuse term (the `emission_factor` of OrcaSlicer's thumbnail shader).
pub(crate) fn shade(normal: [f32; 3], color: [u8; 3], emission: f32) -> [u8; 4] {
    let len = normal.iter().map(|v| v * v).sum::<f32>().sqrt();
    let n = normal.map(|v| v / len);
    let dot = |l: [f32; 3]| (n[0] * l[0] + n[1] * l[1] + n[2] * l[2]).max(0.0);
    let top = [-0.4574957, 0.4574957, 0.7624929];
    let diffuse = 0.3 + 0.48 * dot(top) + 0.18 * dot([0.6985074, 0.1397015, 0.6985074]) + emission;
    let specular = 0.075 * (2.0 * dot(top) * n[2] - top[2]).max(0.0).powi(20);
    let [r, g, b] = color.map(|c| {
        (f32::from(c) * diffuse + 255.0 * specular)
            .round()
            .clamp(0.0, 255.0) as u8
    });
    [r, g, b, 255]
}
