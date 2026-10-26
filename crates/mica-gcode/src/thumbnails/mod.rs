// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/Thumbnails.cpp :: make_and_check_thumbnail_list
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/Thumbnails.hpp :: export_thumbnails_to_file, generate_binary_thumbnails
// Source: orca@8500fcdcca src/libslic3r/GCode/Thumbnails.hpp :: export_thumbnails_to_file
// Source: snapmaker-orca@b1831e5dcb src/libslic3r/GCode/Thumbnails.hpp :: export_thumbnails_to_file
// Changes: Rust CPU renderer/encoders; honor legacy thumbnails_format; reject incomplete numeric tokens
// Changes: and sub-one-pixel dimensions, trim format whitespace; report JPG as unsupported at generation.
// Upstream: Copyright (c) Prusa Research 2022 Enrico Turri, Vojtěch Bubník, Lukáš Hejl (AGPLv3 or higher).

//! Profile-driven mesh previews for text G-code and binary thumbnail blocks.
//! Text export without a source mesh omits previews, just like an absent upstream thumbnail callback.

mod png;
mod qoi;
mod render;

pub use png::{encode_png, encode_png_rgb};
pub use qoi::encode_qoi;
pub use render::{Camera, Fit, Image, Orbit, render, render_orbit, render_orbit_colored};

use std::io::{self, Write};

use mica_core::{config::Config, mesh::TriangleMesh};

use crate::bgcode::{Thumbnail, ThumbnailFormat};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ThumbnailSpec {
    pub width: u16,
    pub height: u16,
    pub format: ThumbnailFormat,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ThumbnailErrors {
    pub invalid_value: bool,
    pub out_of_range: bool,
    pub invalid_format: bool,
}

impl std::fmt::Display for ThumbnailErrors {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid thumbnails setting:")?;
        if self.invalid_value {
            write!(f, " expected WxH/FORMAT entries separated by commas;")?;
        }
        if self.out_of_range {
            write!(f, " dimensions must be at least 1 and less than 1000;")?;
        }
        if self.invalid_format {
            write!(f, " format must be PNG, QOI or JPG;")?;
        }
        Ok(())
    }
}

/// Prusa's list parser: keep order/duplicates, accept legacy sizes without an extension, collect
/// all error categories, and substitute PNG for unknown extensions while reporting the error.
/// Fractional dimensions are truncated to pixels, as in the upstream renderer. Unlike C++ stream
/// extraction we require the entire numeric token to parse, so e.g. `16oopsx16` is rejected.
pub fn make_and_check_thumbnail_list(
    value: &str,
    default_format: &str,
) -> (Vec<ThumbnailSpec>, ThumbnailErrors) {
    let mut list = Vec::new();
    let mut errors = ThumbnailErrors::default();
    if value.is_empty() {
        return (list, errors);
    }
    for entry in value.split_terminator(',') {
        let Some((x, rest)) = entry.split_once('x') else {
            errors.invalid_value = true;
            continue;
        };
        let (y, ext) = rest.split_once('/').unwrap_or((rest, ""));
        if x.is_empty() || y.is_empty() {
            errors.invalid_value = true;
            continue;
        }
        let dimensions = (x.trim().parse::<f64>(), y.trim().parse::<f64>());
        let (Ok(width), Ok(height)) = dimensions else {
            errors.invalid_value = true;
            continue;
        };
        if !(1.0..1000.0).contains(&width) || !(1.0..1000.0).contains(&height) {
            errors.out_of_range = true;
            continue;
        }
        let ext = if ext.trim().is_empty() {
            default_format.trim()
        } else {
            ext.trim()
        };
        let format = match ext.to_ascii_uppercase().as_str() {
            "" | "PNG" => ThumbnailFormat::Png,
            "QOI" => ThumbnailFormat::Qoi,
            "JPG" => ThumbnailFormat::Jpg,
            _ => {
                errors.invalid_format = true;
                ThumbnailFormat::Png
            }
        };
        list.push(ThumbnailSpec {
            width: width as u16,
            height: height as u16,
            format,
        });
    }
    (list, errors)
}

/// Color of the first tool: explicit extruder override, then filament color, then orange.
pub fn filament_color(config: &Config) -> [u8; 3] {
    for key in ["extruder_colour", "filament_colour"] {
        if let Some(rgb) = config
            .string_at(key, 0)
            .and_then(mica_core::color::parse_hex)
        {
            return rgb;
        }
    }
    [255, 128, 0]
}

/// Render and encode the profile's requested thumbnails once, ready for either output container.
/// Invalid settings and unsupported formats fail explicitly; an empty list disables thumbnails.
pub fn generate(config: &Config, mesh: &TriangleMesh) -> io::Result<Vec<Thumbnail>> {
    let (list, errors) = make_and_check_thumbnail_list(
        config.string("thumbnails").unwrap_or(""),
        config.string("thumbnails_format").unwrap_or("PNG"),
    );
    if errors != ThumbnailErrors::default() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            errors.to_string(),
        ));
    }
    if list.iter().any(|s| s.format == ThumbnailFormat::Jpg) {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "JPG thumbnails are not supported; select PNG or QOI",
        ));
    }
    let camera = if config.string("gcode_flavor") == Some("klipper") {
        Camera::Orca
    } else {
        Camera::Prusa
    };
    let color = filament_color(config);
    list.into_iter()
        .map(|spec| {
            let image = render(mesh, spec.width, spec.height, color, camera)?;
            let encode = if spec.format == ThumbnailFormat::Qoi {
                encode_qoi
            } else {
                encode_png
            };
            Ok(Thumbnail {
                format: spec.format,
                width: spec.width,
                height: spec.height,
                data: encode(u32::from(image.width), u32::from(image.height), &image.rgba)?,
            })
        })
        .collect()
}

/// Write upstream-compatible comment blocks. `len` is the base64 character count (not byte size).
/// Orca/U1 wraps each image in THUMBNAIL_BLOCK markers; Prusa adds a trailing semicolon line.
pub fn write_text(out: &mut impl Write, thumbnails: &[Thumbnail], orca: bool) -> io::Result<()> {
    for thumbnail in thumbnails {
        let tag = match thumbnail.format {
            ThumbnailFormat::Png => "thumbnail",
            ThumbnailFormat::Qoi => "thumbnail_QOI",
            ThumbnailFormat::Jpg => "thumbnail_JPG",
        };
        if orca {
            writeln!(out, "; THUMBNAIL_BLOCK_START")?;
        }
        let encoded = base64(&thumbnail.data);
        write!(
            out,
            "\n;\n; {tag} begin {}x{} {}\n",
            thumbnail.width,
            thumbnail.height,
            encoded.len()
        )?;
        for line in encoded.as_bytes().chunks(78) {
            out.write_all(b"; ")?;
            out.write_all(line)?;
            out.write_all(b"\n")?;
        }
        writeln!(out, "; {tag} end")?;
        if orca {
            writeln!(out, "; THUMBNAIL_BLOCK_END\n")?;
        } else {
            writeln!(out, ";")?;
        }
    }
    Ok(())
}

fn base64(data: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for b in data.chunks(3) {
        let word = (u32::from(b[0]) << 16)
            | (u32::from(*b.get(1).unwrap_or(&0)) << 8)
            | u32::from(*b.get(2).unwrap_or(&0));
        out.push(ALPHABET[(word >> 18) as usize] as char);
        out.push(ALPHABET[((word >> 12) & 63) as usize] as char);
        out.push(if b.len() > 1 {
            ALPHABET[((word >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if b.len() > 2 {
            ALPHABET[(word & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
pub(crate) mod tests;
