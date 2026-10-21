// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 resources/shaders/110/gouraud_light.vs :: main
// Changes: Only the two-light diffuse/specular weights (as in Mica's thumbnail renderer, mica-gcode thumbnails/render.rs); the rest is original: a CPU z-buffer rasterizer for `mica mcp` with an orbit/orthographic camera, painted leaves coloured by filament, a facet-ID buffer for picking, 2x supersampling, silhouette/crease/paint-boundary lines and a labelled pixel grid.

//! Offscreen views of the scene for `mica mcp`: what the AI looks at before it paints. The
//! renderer runs on the CPU (no GPU, window or display needed, so it works over SSH and in
//! tests) and keeps, next to the picture, the original facet under every pixel, so a pixel
//! the AI points at maps back to (object, part, facet) exactly.

use mica_core::painting::selector::TriangleSelector;

type V3 = [f32; 3];

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn normalize(a: V3) -> V3 {
    let n = dot(a, a).sqrt();
    if n > 0. { a.map(|v| v / n) } else { a }
}

/// Largest image side, pixels (keeps a view's PNG well below a megabyte).
pub const MAX_SIZE: u32 = 1024;
pub const MIN_SIZE: u32 = 128;

/// Named views: azimuth (0 = front, looking from -Y towards +Y; 90 = from the right, +X) and
/// elevation (90 = from above), degrees.
pub const NAMED: &[(&str, f32, f32)] = &[
    ("front", 0., 0.),
    ("back", 180., 0.),
    ("left", 270., 0.),
    ("right", 90., 0.),
    ("top", 0., 90.),
    ("bottom", 0., -90.),
    ("iso", 45., 30.),
    ("iso_front_right", 45., 30.),
    ("iso_front_left", 315., 30.),
    ("iso_back_right", 135., 30.),
    ("iso_back_left", 225., 30.),
];

/// An orthographic camera: screen x along `right`, y along `up`, looking along `forward`.
#[derive(Clone, Copy, Debug)]
pub struct Camera {
    pub azimuth: f32,
    pub elevation: f32,
    pub center: V3,
    pub right: V3,
    pub up: V3,
    pub forward: V3,
    /// Pixels per millimetre.
    pub scale: f32,
    pub width: u32,
    pub height: u32,
}

impl Camera {
    /// Frames `bounds` (bed millimetres) from the given angles; `zoom` > 1 magnifies around
    /// `target` (default: the centre of `bounds`).
    pub fn frame(
        azimuth: f32,
        elevation: f32,
        bounds: (V3, V3),
        width: u32,
        height: u32,
        zoom: f32,
        target: Option<V3>,
    ) -> Self {
        let (az, el) = (
            azimuth.to_radians(),
            elevation.clamp(-90., 90.).to_radians(),
        );
        let eye = [az.sin() * el.cos(), -az.cos() * el.cos(), el.sin()];
        let forward = eye.map(|v| -v);
        let right = [az.cos(), az.sin(), 0.];
        let up = normalize(cross(right, forward));
        let (lo, hi) = bounds;
        let center = target.unwrap_or(std::array::from_fn(|i| 0.5 * (lo[i] + hi[i])));
        let box_center: V3 = std::array::from_fn(|i| 0.5 * (lo[i] + hi[i]));
        let (mut w, mut h) = (0f32, 0f32);
        for k in 0..8 {
            let p = [
                if k & 1 == 0 { lo[0] } else { hi[0] },
                if k & 2 == 0 { lo[1] } else { hi[1] },
                if k & 4 == 0 { lo[2] } else { hi[2] },
            ];
            let d = sub(p, box_center);
            w = w.max(dot(d, right).abs());
            h = h.max(dot(d, up).abs());
        }
        let fit =
            (width as f32 / (2. * w.max(0.5) * 1.08)).min(height as f32 / (2. * h.max(0.5) * 1.08));
        Self {
            azimuth,
            elevation,
            center,
            right,
            up,
            forward,
            scale: fit * zoom.clamp(0.1, 50.),
            width,
            height,
        }
    }

    /// Screen position (pixels, y down) and depth (mm, larger is farther).
    pub fn project(&self, p: V3) -> V3 {
        let d = sub(p, self.center);
        [
            self.width as f32 * 0.5 + dot(d, self.right) * self.scale,
            self.height as f32 * 0.5 - dot(d, self.up) * self.scale,
            dot(d, self.forward),
        ]
    }

    /// The viewing ray through a pixel position: origin far in front, direction `forward`.
    pub fn ray(&self, x: f32, y: f32) -> (V3, V3) {
        let u = (x - self.width as f32 * 0.5) / self.scale;
        let v = (self.height as f32 * 0.5 - y) / self.scale;
        let o = std::array::from_fn(|i| {
            self.center[i] + self.right[i] * u + self.up[i] * v - self.forward[i] * 100_000.
        });
        (o, self.forward)
    }
}

/// An affine map, rows of a 3 × 4 matrix (bed space of the object's first instance to
/// another instance's).
pub type Affine = [[f64; 4]; 3];

pub fn apply(t: &Affine, p: V3) -> V3 {
    t.map(|r| {
        (r[0] * f64::from(p[0]) + r[1] * f64::from(p[1]) + r[2] * f64::from(p[2]) + r[3]) as f32
    })
}

/// One part placement to draw: its painting selector (bed space of the object's first
/// instance), the tool of its unpainted surface (one based) and, for further instances, the
/// map from the first instance.
pub struct DrawPart<'a> {
    pub selector: &'a TriangleSelector,
    pub base_tool: u8,
    pub to_instance: Option<Affine>,
}

#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// Unpainted surfaces washed out and outlines between tools, to check painting.
    pub highlight_paint: bool,
    /// Faint pixel grid every 100 px with coordinate labels, to read positions off.
    pub grid: bool,
}

/// A rendered view: RGB pixels and, per pixel, `0` (background) or `1 + global facet`,
/// where the global facet numbering runs over `parts` in order (see [`Rendered::bases`]).
pub struct Rendered {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
    pub ids: Vec<u32>,
    /// First global facet of each part.
    pub bases: Vec<u32>,
    /// Pixels per tool (index = tool - 1) among the model's pixels.
    pub tool_pixels: Vec<usize>,
}

impl Rendered {
    /// `(part index, facet)` under a pixel.
    pub fn pick(&self, x: u32, y: u32) -> Option<(usize, usize)> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let id = self.ids[(y * self.width + x) as usize];
        let g = id.checked_sub(1)?;
        let part = self.bases.partition_point(|&b| b <= g) - 1;
        Some((part, (g - self.bases[part]) as usize))
    }
}

/// Supersampling factor per axis.
const SS: usize = 2;

pub fn render(
    parts: &[DrawPart],
    colors: &[[u8; 3]],
    camera: &Camera,
    options: Options,
) -> Rendered {
    let (w, h) = (camera.width as usize, camera.height as usize);
    let (sw, sh) = (w * SS, h * SS);
    let mut depth = vec![f32::INFINITY; sw * sh];
    let mut ids = vec![0u32; sw * sh];
    let mut tools = vec![0u8; sw * sh];
    let mut pixels = vec![[0u8; 3]; sw * sh];
    let mut bases = Vec::with_capacity(parts.len());
    let mut base = 0u32;
    // Light from the upper left of the viewer, plus a fill light (the weights of PrusaSlicer's
    // gouraud_light shader, as Mica's thumbnails use them).
    let key = normalize(std::array::from_fn(|i| {
        -camera.forward[i] * 0.76 + camera.up[i] * 0.46 - camera.right[i] * 0.46
    }));
    let fill = normalize(std::array::from_fn(|i| {
        -camera.forward[i] * 0.7 + camera.up[i] * 0.14 + camera.right[i] * 0.7
    }));
    let color_of = |tool: u8| {
        colors
            .get(usize::from(tool).wrapping_sub(1))
            .copied()
            .unwrap_or([200, 200, 200])
    };
    let scale = SS as f32;
    for part in parts {
        bases.push(base);
        let sel = part.selector;
        let facets = sel.facet_count() as u32;
        let id_base = base;
        base = base.saturating_add(facets);
        sel.world_leaves(true, |facet, state, tri| {
            let tri = match &part.to_instance {
                Some(t) => tri.map(|p| apply(t, p)),
                None => tri,
            };
            let tool = if state == 0 { part.base_tool } else { state };
            let n = normalize(cross(sub(tri[1], tri[0]), sub(tri[2], tri[0])));
            let toward = -dot(n, camera.forward);
            let mut c = color_of(tool).map(f32::from);
            if options.highlight_paint && state == 0 {
                let g = 0.3 * c[0] + 0.59 * c[1] + 0.11 * c[2];
                c = c.map(|v| 0.35 * v + 0.65 * (0.5 * g + 0.5 * 235.));
            }
            let shade = if toward >= 0. {
                let diffuse = 0.32 + 0.5 * dot(n, key).max(0.) + 0.2 * dot(n, fill).max(0.);
                let half = normalize(std::array::from_fn(|i| key[i] - camera.forward[i]));
                let spec = 0.08 * dot(n, half).max(0.).powi(24);
                (diffuse, spec)
            } else {
                // Inside of an open shell.
                (0.25, 0.)
            };
            // Dark filaments get a little lift so their shape stays readable.
            let rgb = c.map(|v| {
                ((v * shade.0 + 18. * shade.0 + 255. * shade.1).round()).clamp(0., 255.) as u8
            });
            let p = tri.map(|v| {
                let s = camera.project(v);
                [s[0] * scale, s[1] * scale, s[2]]
            });
            let area = (p[1][0] - p[0][0]) * (p[2][1] - p[0][1])
                - (p[1][1] - p[0][1]) * (p[2][0] - p[0][0]);
            if area.abs() < 1e-12 || !area.is_finite() {
                return;
            }
            let min_x = (p.iter().map(|v| v[0]).fold(f32::INFINITY, f32::min) - 0.5)
                .ceil()
                .max(0.);
            let max_x = (p.iter().map(|v| v[0]).fold(f32::NEG_INFINITY, f32::max) - 0.5)
                .floor()
                .min(sw as f32 - 1.);
            let min_y = (p.iter().map(|v| v[1]).fold(f32::INFINITY, f32::min) - 0.5)
                .ceil()
                .max(0.);
            let max_y = (p.iter().map(|v| v[1]).fold(f32::NEG_INFINITY, f32::max) - 0.5)
                .floor()
                .min(sh as f32 - 1.);
            if min_x > max_x || min_y > max_y {
                return;
            }
            let inv = 1. / area;
            let id = id_base + facet as u32 + 1;
            for y in min_y as usize..=max_y as usize {
                let py = y as f32 + 0.5;
                for x in min_x as usize..=max_x as usize {
                    let px = x as f32 + 0.5;
                    let w0 =
                        ((p[1][0] - px) * (p[2][1] - py) - (p[1][1] - py) * (p[2][0] - px)) * inv;
                    let w1 =
                        ((p[2][0] - px) * (p[0][1] - py) - (p[2][1] - py) * (p[0][0] - px)) * inv;
                    let w2 = 1. - w0 - w1;
                    if w0 < -1e-6 || w1 < -1e-6 || w2 < -1e-6 {
                        continue;
                    }
                    let z = w0 * p[0][2] + w1 * p[1][2] + w2 * p[2][2];
                    let i = y * sw + x;
                    if z < depth[i] {
                        depth[i] = z;
                        ids[i] = id;
                        tools[i] = tool;
                        pixels[i] = rgb;
                    }
                }
            }
        });
    }
    // Resolve: average the samples (background where none), one sample's facet per pixel.
    let mut rgb = vec![0u8; w * h * 3];
    let mut out_ids = vec![0u32; w * h];
    let mut out_depth = vec![f32::INFINITY; w * h];
    let mut out_tools = vec![0u8; w * h];
    let mut tool_pixels = vec![0usize; colors.len().max(1)];
    for y in 0..h {
        let t = y as f32 / h.max(1) as f32;
        let bg = [232. - 30. * t, 235. - 28. * t, 240. - 22. * t];
        for x in 0..w {
            let mut sum = [0f32; 3];
            let mut chosen = None;
            for (dx, dy) in [(1, 1), (0, 0), (1, 0), (0, 1)] {
                let i = (y * SS + dy) * sw + x * SS + dx;
                if ids[i] != 0 {
                    for c in 0..3 {
                        sum[c] += f32::from(pixels[i][c]);
                    }
                    if chosen.is_none() {
                        chosen = Some(i);
                    }
                } else {
                    for c in 0..3 {
                        sum[c] += bg[c];
                    }
                }
            }
            let o = y * w + x;
            for c in 0..3 {
                rgb[o * 3 + c] = (sum[c] / 4.).round() as u8;
            }
            if let Some(i) = chosen {
                out_ids[o] = ids[i];
                out_depth[o] = depth[i];
                out_tools[o] = tools[i];
                if let Some(n) = tool_pixels.get_mut(usize::from(tools[i]).wrapping_sub(1)) {
                    *n += 1;
                }
            }
        }
    }
    let rendered_normal = |id: u32| -> Option<V3> {
        let g = id.checked_sub(1)?;
        let part = bases.partition_point(|&b| b <= g) - 1;
        let sel = parts[part].selector;
        let n = sel.normal((g - bases[part]) as usize);
        Some(match &parts[part].to_instance {
            Some(t) => {
                let o = apply(t, [0.; 3]);
                normalize(sub(apply(t, n), o))
            }
            None => n,
        })
    };
    // Lines: silhouettes and depth steps (dark), creases (soft), paint boundaries (strong
    // when highlighting).
    let step = (3. / camera.scale).max(0.8);
    let crease = 40f32.to_radians().cos();
    let mut darken = vec![1f32; w * h];
    let mut boundary = vec![false; w * h];
    for y in 0..h {
        for x in 0..w {
            let o = y * w + x;
            for (nx, ny) in [(x + 1, y), (x, y + 1)] {
                if nx >= w || ny >= h {
                    continue;
                }
                let q = ny * w + nx;
                let (a, b) = (out_ids[o], out_ids[q]);
                if a == b {
                    continue;
                }
                let factor = if a == 0 || b == 0 || (out_depth[o] - out_depth[q]).abs() > step {
                    0.45
                } else {
                    match (rendered_normal(a), rendered_normal(b)) {
                        (Some(na), Some(nb)) if dot(na, nb) < crease => 0.7,
                        _ => 1.,
                    }
                };
                // The nearer pixel carries the line.
                let target = if a == 0 || (b != 0 && out_depth[q] < out_depth[o]) {
                    q
                } else {
                    o
                };
                darken[target] = darken[target].min(factor);
                if options.highlight_paint && a != 0 && b != 0 && out_tools[o] != out_tools[q] {
                    boundary[o] = true;
                    boundary[q] = true;
                }
            }
        }
    }
    for o in 0..w * h {
        if boundary[o] {
            rgb[o * 3..o * 3 + 3].copy_from_slice(&[20, 20, 20]);
        } else if darken[o] < 1. {
            for c in 0..3 {
                rgb[o * 3 + c] = (f32::from(rgb[o * 3 + c]) * darken[o]) as u8;
            }
        }
    }
    if options.grid {
        draw_grid(&mut rgb, w, h);
    }
    Rendered {
        width: camera.width,
        height: camera.height,
        rgb,
        ids: out_ids,
        bases,
        tool_pixels,
    }
}

/// 3 × 5 digit glyphs, one row per three bits, top row first.
const DIGITS: [[u8; 5]; 10] = [
    [0b111, 0b101, 0b101, 0b101, 0b111],
    [0b010, 0b110, 0b010, 0b010, 0b111],
    [0b111, 0b001, 0b111, 0b100, 0b111],
    [0b111, 0b001, 0b111, 0b001, 0b111],
    [0b101, 0b101, 0b111, 0b001, 0b001],
    [0b111, 0b100, 0b111, 0b001, 0b111],
    [0b111, 0b100, 0b111, 0b101, 0b111],
    [0b111, 0b001, 0b010, 0b010, 0b010],
    [0b111, 0b101, 0b111, 0b101, 0b111],
    [0b111, 0b101, 0b111, 0b001, 0b111],
];

fn blend(rgb: &mut [u8], w: usize, x: usize, y: usize, c: [u8; 3], a: f32) {
    let o = (y * w + x) * 3;
    for k in 0..3 {
        rgb[o + k] = (f32::from(rgb[o + k]) * (1. - a) + f32::from(c[k]) * a).round() as u8;
    }
}

fn draw_text(rgb: &mut [u8], w: usize, h: usize, x0: usize, y0: usize, text: &str) {
    const S: usize = 2;
    for (k, ch) in text.bytes().enumerate() {
        let Some(glyph) = ch
            .checked_sub(b'0')
            .and_then(|d| DIGITS.get(usize::from(d)))
        else {
            continue;
        };
        let gx = x0 + k * 4 * S;
        // A light backing box keeps the digits readable over the model.
        for y in y0.saturating_sub(1)..(y0 + 5 * S + 1).min(h) {
            for x in gx.saturating_sub(1)..(gx + 3 * S + 1).min(w) {
                blend(rgb, w, x, y, [255, 255, 255], 0.75);
            }
        }
        for (row, bits) in glyph.iter().enumerate() {
            for col in 0..3 {
                if bits & (0b100 >> col) != 0 {
                    for dy in 0..S {
                        for dx in 0..S {
                            let (x, y) = (gx + col * S + dx, y0 + row * S + dy);
                            if x < w && y < h {
                                blend(rgb, w, x, y, [30, 30, 60], 1.);
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Faint lines every 100 px, labelled along the top and left edges.
fn draw_grid(rgb: &mut [u8], w: usize, h: usize) {
    for x in (100..w).step_by(100) {
        for y in 0..h {
            blend(rgb, w, x, y, [40, 60, 120], 0.18);
        }
        let label = x.to_string();
        if x + 3 + label.len() * 8 < w {
            draw_text(rgb, w, h, x + 3, 3, &label);
        }
    }
    for y in (100..h).step_by(100) {
        for x in 0..w {
            blend(rgb, w, x, y, [40, 60, 120], 0.18);
        }
        draw_text(rgb, w, h, 3, y + 3, &y.to_string());
    }
}

/// Standard base64 (for MCP image content).
pub fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for k in 0..4 {
            if k <= chunk.len() {
                out.push(T[((n >> (18 - 6 * k)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use mica_core::mesh::TriangleMesh;

    fn cube(s: f32) -> TriangleMesh {
        TriangleMesh {
            vertices: vec![
                [0., 0., 0.],
                [s, 0., 0.],
                [s, s, 0.],
                [0., s, 0.],
                [0., 0., s],
                [s, 0., s],
                [s, s, s],
                [0., s, s],
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
        }
    }

    const IDENTITY: [[f32; 4]; 4] = [
        [1., 0., 0., 0.],
        [0., 1., 0., 0.],
        [0., 0., 1., 0.],
        [0., 0., 0., 1.],
    ];

    #[test]
    fn base64_matches_rfc4648() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn views_pick_the_facing_side_and_paint_shows() {
        let mesh = cube(20.);
        let mut sel = TriangleSelector::new(&mesh, &IDENTITY, false);
        // Paint the front face (y = 0, facets 4 and 5) tool 2.
        sel.set_facet(4, 2);
        sel.set_facet(5, 2);
        let colors = [[240, 240, 240], [200, 30, 30]];
        let bounds = ([0.; 3], [20.; 3]);
        let parts = [DrawPart {
            selector: &sel,
            base_tool: 1,
            to_instance: None,
        }];
        let opts = Options {
            highlight_paint: false,
            grid: false,
        };
        let front = Camera::frame(0., 0., bounds, 200, 200, 1., None);
        let r = render(&parts, &colors, &front, opts);
        let (_, facet) = r.pick(100, 100).unwrap();
        assert!(facet == 4 || facet == 5, "{facet}");
        assert!(r.pick(2, 2).is_none(), "corner is background");
        let c = &r.rgb[(100 * 200 + 100) * 3..][..3];
        assert!(
            u16::from(c[0]) > u16::from(c[1]) + 50 && u16::from(c[0]) > u16::from(c[2]) + 50,
            "{c:?}"
        );
        assert!(r.tool_pixels[1] > r.tool_pixels[0]);
        let top = Camera::frame(0., 90., bounds, 200, 200, 1., None);
        let (_, facet) = render(&parts, &colors, &top, opts).pick(100, 100).unwrap();
        assert!(facet == 2 || facet == 3, "{facet}");
        let right = Camera::frame(90., 0., bounds, 200, 200, 1., None);
        let (_, facet) = render(&parts, &colors, &right, opts)
            .pick(100, 100)
            .unwrap();
        assert!(facet == 6 || facet == 7, "{facet}");
        // The ray through a pixel hits the front face plane (y = 0) at the expected x, z.
        let (o, d) = front.ray(100., 100.);
        let t = (0. - o[1]) / d[1];
        let hit: V3 = std::array::from_fn(|i| o[i] + d[i] * t);
        assert!(
            (hit[0] - 10.).abs() < 0.2 && (hit[2] - 10.).abs() < 0.2,
            "{hit:?}"
        );
    }
}
