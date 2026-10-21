// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Encoded pictures → RGBA, downscaled for thumbnails and turned the way the camera is
//! mounted (Moonraker's `rotation`, `flip_horizontal`, `flip_vertical`).

use image::{RgbaImage, imageops};

/// How the picture is turned: clockwise degrees (0, 90, 180, 270), then mirrored.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Orientation {
    pub rotation: u16,
    pub flip_horizontal: bool,
    pub flip_vertical: bool,
}

/// At most this many bytes of encoded picture are decoded (a 4K JPEG is far below it).
pub const MAX_ENCODED: usize = 16 * 1024 * 1024;
/// Pictures larger than this in either direction are refused (decoder memory).
const MAX_SIDE: u32 = 8192;

pub fn decode(bytes: &[u8], o: Orientation, max_width: u32) -> Result<RgbaImage, String> {
    if bytes.len() > MAX_ENCODED {
        return Err("the camera picture is too large".into());
    }
    let reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| "the camera sent an unreadable picture".to_string())?;
    let (w, h) = reader
        .into_dimensions()
        .map_err(|_| "the camera sent a picture Mica cannot decode".to_string())?;
    if w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE {
        return Err("the camera picture has an unsupported size".into());
    }
    let image = image::load_from_memory(bytes)
        .map_err(|_| "the camera sent a picture Mica cannot decode".to_string())?
        .to_rgba8();
    Ok(finish(image, o, max_width))
}

/// Downscales (keeping the aspect) and orients a decoded picture.
pub fn finish(image: RgbaImage, o: Orientation, max_width: u32) -> RgbaImage {
    // Mirror and turn first, so `max_width` applies to the picture as shown.
    let mut image = match o.rotation {
        90 => imageops::rotate90(&image),
        180 => imageops::rotate180(&image),
        270 => imageops::rotate270(&image),
        _ => image,
    };
    if o.flip_horizontal {
        imageops::flip_horizontal_in_place(&mut image);
    }
    if o.flip_vertical {
        imageops::flip_vertical_in_place(&mut image);
    }
    let (w, h) = image.dimensions();
    if max_width > 0 && w > max_width {
        let nh = ((h as u64 * max_width as u64) / w as u64).max(1) as u32;
        image = imageops::thumbnail(&image, max_width, nh);
    }
    image
}
