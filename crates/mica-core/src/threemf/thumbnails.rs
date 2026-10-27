// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: orca@8500fcdcca src/libslic3r/Format/bbs_3mf.cpp :: _BBS_3MF_Exporter::_add_thumbnail_file_to_archive
// Source: orca@8500fcdcca src/slic3r/GUI/Plater.cpp :: Plater::export_3mf
// Source: orca@8500fcdcca src/slic3r/GUI/GLCanvas3D.cpp :: GLCanvas3D::render_thumbnail_internal
// Source: orca@8500fcdcca src/slic3r/GUI/3DScene.cpp :: adjust_color_for_rendering
// Source: prusaslicer@b028299c77 src/slic3r/GUI/Plater.cpp :: Plater::export_3mf
// Source: prusaslicer@b028299c77 src/slic3r/GUI/GLCanvas3D.cpp :: GLCanvas3D::_render_thumbnail_internal
// Changes: The pictures a project 3MF carries, drawn by Mica's CPU thumbnail renderer instead of
// Changes: OpenGL: Orca's per-plate 512 px iso picture (thumbnail shader, emission 0.1), its
// Changes: 128 px block-averaged `_small` copy and the unlit picture whose alpha encodes the
// Changes: extruder; Prusa's 256 px picture of the first bed. Model parts only, flat normals,
// Changes: colour painting, mixed (virtual) tools, one translucent layer blended over the opaque
// Changes: scene; no bed, top-view or ID-colour picking pictures. Plates render in parallel.
// Changes: `plate_picture` draws the Orca plate picture of loose parts for the GUI's plate lists.
// Upstream attribution (AGPLv3 or higher):
// Copyright (c) Prusa Research 2018-2023 Enrico Turri, Oleksandra Iushchenko, Tomáš Mészáros,
// Lukáš Matěna, Vojtěch Bubník, David Kocík, Filip Sykala, Lukáš Hejl, Vojtěch Král.
// Copyright (c) 2021 BambuStudio (Bambu Lab); OrcaSlicer contributors.

//! The pictures of a saved project: what Finder, Quick Look, Printables and the slicers'
//! project lists show.
//!
//! OrcaSlicer flavour (`Plater::export_3mf`, `_BBS_3MF_Exporter`): for every plate
//! `Metadata/plate_N.png` (512 x 512, the plate's printable model parts from the iso camera,
//! lit by Orca's thumbnail shader), `Metadata/plate_N_small.png` (128 x 128, averaged 4 x 4
//! blocks of it) and `Metadata/plate_no_light_N.png` (unlit filament colours, the alpha holding
//! `255 - (extruder - 1)` as Bambu's send dialog reads it). Orca's `top_N.png` / `pick_N.png`
//! (a top view and an object-ID picking image for Bambu printers' skip-object feature) are not
//! written: Orca and Bambu Studio draw them again when they slice the plate.
//!
//! PrusaSlicer flavour (`Plater::export_3mf`): `Metadata/thumbnail.png`, 256 x 256, the first
//! bed's model parts in their extruder colours (non-printable instances grey), no bed.
//!
//! Both flavours carry the standard package thumbnail `Metadata/thumbnail.png` (Orca's
//! flavour: plate 1's picture) for Finder, Quick Look and Printables.

use super::write::Flavour;
use super::*;
use crate::thumbnail::{Camera, Frame, Raster, iso_view, shade};
use rayon::prelude::*;

/// Orca's `THUMBNAIL_SIZE_3MF` (Plater.cpp).
pub const ORCA_PX: u16 = 512;
/// Orca's `PLATE_THUMBNAIL_SMALL_WIDTH` / `_HEIGHT` (bbs_3mf.hpp).
pub const ORCA_SMALL_PX: u16 = 128;
/// PrusaSlicer's `THUMBNAIL_SIZE_3MF` (Plater.cpp).
pub const PRUSA_PX: u16 = 256;
/// The 3MF package thumbnail (the target of the OPC thumbnail relationship).
pub const PACKAGE: &str = "Metadata/thumbnail.png";

/// Orca's `THUMBNAIL_FILE_FORMAT` (bbs_3mf.hpp), for plate `n` (one-based).
pub fn orca_plate(n: usize) -> String {
    format!("Metadata/plate_{n}.png")
}
/// Orca's small copy of [`orca_plate`] (`_add_thumbnail_file_to_archive`).
pub fn orca_small(n: usize) -> String {
    format!("Metadata/plate_{n}_small.png")
}
/// Orca's `NO_LIGHT_THUMBNAIL_FILE_FORMAT` (bbs_3mf.hpp).
pub fn orca_no_light(n: usize) -> String {
    format!("Metadata/plate_no_light_{n}.png")
}

/// Every picture of the project, `(archive path, PNG)` in archive order. `config` is the
/// settings text the save writes (its filament colours colour the parts).
pub(super) fn pictures(
    p: &Project,
    flavour: Flavour,
    config: &str,
) -> Result<Vec<(String, Vec<u8>)>, ThreeMfError> {
    let palette = Palette::new(p, flavour, config);
    let mut out = Vec::new();
    match flavour {
        Flavour::Orca => {
            let plates: Vec<Vec<(usize, usize)>> = if p.plates.is_empty() {
                vec![all_instances(p)]
            } else {
                p.plates
                    .iter()
                    .map(|plate| {
                        plate
                            .instances
                            .iter()
                            .map(|i| (i.object, i.instance))
                            .collect()
                    })
                    .collect()
            };
            let rendered: Vec<Result<[Vec<u8>; 3], ThreeMfError>> = plates
                .par_iter()
                .map(|members| {
                    // Orca draws printable instances only (`is_visible`).
                    let parts = scene(p, members, true);
                    let (lit, flat) = draw_or_blank(&parts, &palette, ORCA_PX, Camera::Orca, 0.1);
                    let small = orca_small_pixels(&lit, ORCA_PX, ORCA_SMALL_PX);
                    Ok([
                        png(&lit, ORCA_PX)?,
                        png(&small, ORCA_SMALL_PX)?,
                        png(&flat, ORCA_PX)?,
                    ])
                })
                .collect();
            for (i, r) in rendered.into_iter().enumerate() {
                let [lit, small, flat] = r?;
                if i == 0 {
                    out.push((PACKAGE.to_string(), lit.clone()));
                }
                out.push((orca_plate(i + 1), lit));
                out.push((orca_small(i + 1), small));
                out.push((orca_no_light(i + 1), flat));
            }
        }
        Flavour::Prusa => {
            // The first bed, as the Prusa flavour prints it; non-printable instances grey.
            let members = p.plates.first().map_or_else(
                || all_instances(p),
                |plate| {
                    plate
                        .instances
                        .iter()
                        .map(|i| (i.object, i.instance))
                        .collect()
                },
            );
            let parts = scene(p, &members, false);
            let (lit, _) = draw_or_blank(&parts, &palette, PRUSA_PX, Camera::Prusa, 0.);
            out.push((PACKAGE.to_string(), png(&lit, PRUSA_PX)?));
        }
    }
    Ok(out)
}

/// One model part of a plate for [`plate_picture`]: its mesh placed by `transform`, its colour
/// painting, and its one-based tool.
pub struct PicturePart<'a> {
    pub mesh: &'a crate::mesh::TriangleMesh,
    pub painting: &'a crate::painting::FacetPainting,
    pub transform: Transform,
    pub tool: usize,
}

/// A plate's picture as a saved Orca-flavour project carries it (`Metadata/plate_N.png`: iso
/// camera, thumbnail-shader lighting, translucent filaments blended), `px` square, straight
/// RGBA rows, top first; transparent where nothing is drawn. `colours` are the tools' filament
/// colours in one-based tool order minus one (physical tools, then mixed colours).
pub fn plate_picture(parts: &[PicturePart], colours: &[[u8; 4]], px: u16) -> Vec<u8> {
    let palette = Palette {
        physical: colours.to_vec(),
        mixes: None,
        flavour: Flavour::Orca,
    };
    let placed: Vec<Placed> = parts
        .iter()
        .map(|p| Placed {
            transform: p.transform,
            mesh: p.mesh,
            painting: p.painting,
            tool: p.tool.max(1),
            grey: false,
        })
        .collect();
    draw_or_blank(&placed, &palette, px, Camera::Orca, 0.1).0
}

fn all_instances(p: &Project) -> Vec<(usize, usize)> {
    p.objects
        .iter()
        .enumerate()
        .flat_map(|(oi, o)| (0..o.instances.len()).map(move |ii| (oi, ii)))
        .collect()
}

/// One model part placed by one instance.
struct Placed<'a> {
    transform: Transform,
    mesh: &'a crate::mesh::TriangleMesh,
    painting: &'a crate::painting::FacetPainting,
    /// One-based tool of the part (its own `extruder`, else the object's, else 1).
    tool: usize,
    /// PrusaSlicer draws non-printable instances grey.
    grey: bool,
}

fn scene<'a>(p: &'a Project, members: &[(usize, usize)], printable_only: bool) -> Vec<Placed<'a>> {
    let tool = |s: &Settings| {
        s.get("extruder")
            .and_then(|v| v.trim().parse::<usize>().ok())
            .filter(|&t| t > 0)
    };
    let mut out = Vec::new();
    for &(oi, ii) in members {
        let Some(o) = p.objects.get(oi) else { continue };
        let Some(inst) = o.instances.get(ii) else {
            continue;
        };
        if printable_only && !inst.printable {
            continue;
        }
        for part in o.parts.iter().filter(|part| part.kind == PartKind::Model) {
            out.push(Placed {
                transform: inst.transform,
                mesh: &part.mesh,
                painting: &part.painting,
                tool: tool(&part.settings).or(tool(&o.settings)).unwrap_or(1),
                grey: !inst.printable,
            });
        }
    }
    out
}

/// The filament colours a save draws with, by one-based tool.
struct Palette {
    physical: Vec<[u8; 4]>,
    mixes: Option<crate::full_spectrum::FullSpectrum>,
    flavour: Flavour,
}

impl Palette {
    /// Orca draws `filament_colour` (`extruder_colour` fills a slot without one); PrusaSlicer
    /// prefers a set `extruder_colour` (`get_extruder_colors_from_plater_config`).
    fn new(p: &Project, flavour: Flavour, config: &str) -> Self {
        let (filament, extruder): (Vec<String>, Vec<String>) = if flavour == Flavour::Orca {
            let list = |j: &json::Json, key: &str| -> Vec<String> {
                j.get(key)
                    .and_then(|v| v.array())
                    .map(|a| {
                        a.iter()
                            .map(|v| v.text().unwrap_or("").to_string())
                            .collect()
                    })
                    .unwrap_or_default()
            };
            match json::parse(config) {
                Ok(j) => (list(&j, "filament_colour"), list(&j, "extruder_colour")),
                Err(_) => Default::default(),
            }
        } else {
            let ini = super::write_config::ini(config);
            let list = |key: &str| -> Vec<String> {
                ini.get(key)
                    .map(|v| v.split(';').map(str::to_string).collect())
                    .unwrap_or_default()
            };
            (list("filament_colour"), list("extruder_colour"))
        };
        let (first, second) = if flavour == Flavour::Orca {
            (&filament, &extruder)
        } else {
            (&extruder, &filament)
        };
        let physical: Vec<[u8; 4]> = (0..first.len().max(second.len()))
            .map(|i| {
                first
                    .get(i)
                    .and_then(|c| crate::color::parse_hex_rgba(c))
                    .or_else(|| second.get(i).and_then(|c| crate::color::parse_hex_rgba(c)))
                    .unwrap_or([255, 128, 0, 255])
            })
            .collect();
        let mixes = p.full_spectrum_json.as_deref().and_then(|text| {
            crate::full_spectrum::FullSpectrum::from_json(text, physical.len()).ok()
        });
        Self {
            physical,
            mixes,
            flavour,
        }
    }

    /// The colour of a tool, as the flavour's slicer draws it in a thumbnail.
    fn colour(&self, tool: usize) -> [u8; 4] {
        let raw = match self.physical.get(tool.wrapping_sub(1)) {
            Some(&c) => c,
            None => match self.mixes.as_ref().and_then(|m| m.get(tool)) {
                Some(mix) => {
                    let rgb: Vec<[u8; 3]> = self
                        .physical
                        .iter()
                        .map(|&[r, g, b, _]| [r, g, b])
                        .collect();
                    let [r, g, b] = mix.predicted_rgb(&rgb);
                    [r, g, b, 255]
                }
                None => self.physical.last().copied().unwrap_or([255, 128, 0, 255]),
            },
        };
        if self.flavour == Flavour::Orca {
            orca_display(raw)
        } else {
            raw
        }
    }
}

/// OrcaSlicer's `adjust_color_for_rendering`: an almost fully transparent filament is drawn
/// white at 0.3 alpha, an almost black one dark grey (0.2).
fn orca_display([r, g, b, a]: [u8; 4]) -> [u8; 4] {
    // FullyTransparentMaterialThreshold 0.1, FullTransparentModdifiedToFixAlpha 0.3.
    if f32::from(a) / 255. < 0.1 {
        return [255, 255, 255, 77];
    }
    // FULL_BLACK_THRESHOLD 0.2 = 51 / 255.
    if r < 51 && g < 51 && b < 51 {
        return [51, 51, 51, a];
    }
    [r, g, b, a]
}

/// What one sample holds: the lit colour (alpha: the filament's, 0 for empty) and the unlit
/// colour with Orca's extruder-encoding alpha.
#[derive(Clone, Copy, Default)]
struct Sample {
    lit: [u8; 4],
    flat: [u8; 4],
}

/// Draws the parts square, `px` pixels, framed to their box: the lit picture and the unlit one
/// (RGBA rows, top first). An empty scene gives two transparent pictures (Orca writes one for
/// an empty plate too).
/// [`draw`], with a transparent picture where the geometry cannot be drawn (degenerate or
/// non-finite coordinates): a picture never stops a project from saving.
fn draw_or_blank(
    parts: &[Placed],
    palette: &Palette,
    px: u16,
    camera: Camera,
    emission: f32,
) -> (Vec<u8>, Vec<u8>) {
    draw(parts, palette, px, camera, emission).unwrap_or_else(|_| {
        let n = usize::from(px) * usize::from(px) * 4;
        (vec![0; n], vec![0; n])
    })
}

fn draw(
    parts: &[Placed],
    palette: &Palette,
    px: u16,
    camera: Camera,
    emission: f32,
) -> Result<(Vec<u8>, Vec<u8>), ThreeMfError> {
    let n = usize::from(px) * usize::from(px) * 4;
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    // The slicers frame the union of the volumes' `transformed_bounding_box`: each part's own
    // box, placed (so a turned part frames a little loose, as there).
    for placed in parts.iter().filter(|p| !p.mesh.faces.is_empty()) {
        let (a, b) = placed.mesh.bounds();
        for corner in 0..8 {
            let v = std::array::from_fn(|i| if (corner >> i) & 1 == 1 { b[i] } else { a[i] });
            let w = placed.transform.apply(v);
            for a in 0..3 {
                lo[a] = lo[a].min(w[a]);
                hi[a] = hi[a].max(w[a]);
            }
        }
    }
    if parts.iter().all(|placed| placed.mesh.faces.is_empty()) || lo[0] > hi[0] {
        return Ok((vec![0; n], vec![0; n]));
    }
    let frame = Frame::iso(lo, hi, px, px, camera).map_err(ThreeMfError::Io)?;
    let colour = |placed: &Placed, tool: usize| -> [u8; 4] {
        if placed.grey {
            [128, 128, 128, 255]
        } else {
            palette.colour(tool)
        }
    };
    let translucent = |placed: &Placed, tool: usize| colour(placed, tool)[3] < 255;
    let mut opaque = Raster::new(&frame, Sample::default());
    let mut any_translucent = false;
    let mut vertices = Vec::new();
    let mut faces = Vec::new();
    let mut tools = Vec::new();
    // Opaque faces first; then the nearest translucent surface in front of them, blended once.
    for pass in 0..2 {
        let mut layer = (pass == 1).then(|| Raster {
            w: opaque.w,
            h: opaque.h,
            depth: opaque.depth.clone(),
            pixels: vec![Sample::default(); opaque.pixels.len()],
        });
        for placed in parts {
            let part = placed;
            let paint = &part.painting.material;
            if paint.is_empty() {
                let t = translucent(placed, placed.tool);
                any_translucent |= t;
                if t != (pass == 1) {
                    continue;
                }
            }
            let mirrored = placed.transform.determinant() < 0.;
            vertices.clear();
            faces.clear();
            tools.clear();
            if paint.is_empty() {
                vertices.extend(
                    part.mesh
                        .vertices
                        .iter()
                        .map(|&v| frame.project(iso_view, placed.transform.apply(v))),
                );
                faces.extend(
                    part.mesh
                        .faces
                        .iter()
                        .map(|&f| if mirrored { [f[0], f[2], f[1]] } else { f }),
                );
            } else {
                paint.visit(part.mesh, true, |state, t| {
                    let base = vertices.len() as u32;
                    for p in t {
                        let v = placed.transform.apply(p.map(|v| v as f32));
                        vertices.push(frame.project(iso_view, v));
                    }
                    faces.push(if mirrored {
                        [base, base + 2, base + 1]
                    } else {
                        [base, base + 1, base + 2]
                    });
                    tools.push(if state == 0 {
                        placed.tool
                    } else {
                        usize::from(state)
                    });
                });
            }
            let tool_of = |i: usize| tools.get(i).copied().unwrap_or(placed.tool);
            if pass == 0 && !paint.is_empty() {
                any_translucent |= tools.iter().any(|&t| translucent(placed, t));
            }
            // Orca's unlit picture: alpha 255 - (extruder - 1) of the part's own extruder.
            let code = 255u8.saturating_sub(u8::try_from(placed.tool - 1).unwrap_or(255));
            let skip = |i: usize| translucent(placed, tool_of(i)) != (pass == 1);
            let shade_face = |i: usize, normal: [f32; 3]| {
                let [r, g, b, a] = colour(placed, tool_of(i));
                let [lr, lg, lb, _] = shade(normal, [r, g, b], emission);
                Sample {
                    lit: [lr, lg, lb, a.max(1)],
                    flat: [r, g, b, code],
                }
            };
            let target = layer.as_mut().unwrap_or(&mut opaque);
            target
                .draw(&vertices, &faces, skip, shade_face)
                .map_err(ThreeMfError::Io)?;
        }
        if let Some(layer) = layer {
            return Ok(resolve(&opaque, Some(&layer), px));
        }
        if !any_translucent {
            break;
        }
    }
    Ok(resolve(&opaque, None, px))
}

/// The two pictures from the samples: lit (the translucent layer blended over the opaque one,
/// alpha-weighted 2x2 average) and unlit (the nearest surface, Orca's coded alpha averaged
/// with the empty background as multisampling does).
fn resolve(opaque: &Raster<Sample>, layer: Option<&Raster<Sample>>, px: u16) -> (Vec<u8>, Vec<u8>) {
    let (w, size) = (opaque.w, usize::from(px));
    let mut lit = Vec::with_capacity(size * size * 4);
    let mut flat = Vec::with_capacity(size * size * 4);
    for y in 0..size {
        for x in 0..size {
            let i = 2 * y * w + 2 * x;
            let (mut sum, mut weight) = ([0f32; 3], 0f32);
            let (mut flat_sum, mut flat_alpha, mut covered) = ([0u32; 3], 0u32, 0u32);
            for s in [i, i + 1, i + w, i + w + 1] {
                let o = opaque.pixels[s];
                let t = layer.map_or(Sample::default(), |l| l.pixels[s]);
                let (rgb, alpha) = if t.lit[3] > 0 {
                    let a = f32::from(t.lit[3]) / 255.;
                    let top = t.lit.map(f32::from);
                    if o.lit[3] > 0 {
                        let under = o.lit.map(f32::from);
                        (
                            std::array::from_fn(|c| top[c] * a + under[c] * (1. - a)),
                            1.,
                        )
                    } else {
                        ([top[0], top[1], top[2]], a)
                    }
                } else if o.lit[3] > 0 {
                    ([o.lit[0], o.lit[1], o.lit[2]].map(f32::from), 1.)
                } else {
                    ([0.; 3], 0.)
                };
                for c in 0..3 {
                    sum[c] += rgb[c] * alpha;
                }
                weight += alpha;
                let nearest = if t.lit[3] > 0 {
                    Some(t.flat)
                } else {
                    (o.lit[3] > 0).then_some(o.flat)
                };
                if let Some(f) = nearest {
                    covered += 1;
                    for c in 0..3 {
                        flat_sum[c] += u32::from(f[c]);
                    }
                    flat_alpha += u32::from(f[3]);
                }
            }
            for c in sum {
                lit.push(if weight > 0. {
                    (c / weight).round().clamp(0., 255.) as u8
                } else {
                    0
                });
            }
            lit.push((weight * 255. / 4.).round() as u8);
            for c in flat_sum {
                flat.push((c + covered / 2).checked_div(covered).unwrap_or(0) as u8);
            }
            flat.push(((flat_alpha + 2) / 4) as u8);
        }
    }
    (lit, flat)
}

/// Orca's small plate picture: plain averages of `size / small`-pixel square blocks (straight
/// RGBA, so edges darken against the transparent background as in Orca).
fn orca_small_pixels(rgba: &[u8], size: u16, small: u16) -> Vec<u8> {
    let (size, small) = (usize::from(size), usize::from(small));
    let step = size / small;
    let mut out = vec![0u8; small * small * 4];
    for by in 0..small {
        for bx in 0..small {
            let mut acc = [0u32; 4];
            for m in 0..step {
                for k in 0..step {
                    let i = 4 * ((by * step + m) * size + bx * step + k);
                    for c in 0..4 {
                        acc[c] += u32::from(rgba[i + c]);
                    }
                }
            }
            let o = 4 * (by * small + bx);
            for c in 0..4 {
                out[o + c] = (acc[c] / (step * step) as u32).min(255) as u8;
            }
        }
    }
    out
}

/// A square RGBA picture as PNG.
fn png(rgba: &[u8], px: u16) -> Result<Vec<u8>, ThreeMfError> {
    let mut out = Vec::new();
    {
        let mut e = png::Encoder::new(&mut out, u32::from(px), u32::from(px));
        e.set_color(png::ColorType::Rgba);
        e.set_depth(png::BitDepth::Eight);
        let mut w = e
            .write_header()
            .map_err(|e| ThreeMfError::Io(std::io::Error::other(e)))?;
        w.write_image_data(rgba)
            .map_err(|e| ThreeMfError::Io(std::io::Error::other(e)))?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orca_small_averages_blocks() {
        let mut big = vec![0u8; 8 * 8 * 4];
        // The top-left 4 x 4 block opaque red: its small pixel is red; the others empty.
        for y in 0..4 {
            for x in 0..4 {
                big[4 * (y * 8 + x)..4 * (y * 8 + x) + 4].copy_from_slice(&[255, 0, 0, 255]);
            }
        }
        let small = orca_small_pixels(&big, 8, 2);
        assert_eq!(&small[0..4], &[255, 0, 0, 255]);
        assert_eq!(&small[4..16], &[0; 12]);
    }

    #[test]
    fn orca_display_colours() {
        assert_eq!(orca_display([0, 0, 0, 255]), [51, 51, 51, 255]);
        assert_eq!(orca_display([10, 200, 0, 128]), [10, 200, 0, 128]);
        assert_eq!(orca_display([10, 200, 0, 20]), [255, 255, 255, 77]);
    }
}
