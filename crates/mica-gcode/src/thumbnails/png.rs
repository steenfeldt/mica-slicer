// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Shared PNG writer for mesh thumbnails and the CLI's toolpath preview.

use std::io::{self, Write};

/// Encode top-to-bottom, unpremultiplied RGBA8 pixels as a compressed PNG.
pub fn encode_png(width: u32, height: u32, rgba: &[u8]) -> io::Result<Vec<u8>> {
    encode(width, height, rgba, 4)
}

/// Encode top-to-bottom RGB8 pixels (used by the CLI layer preview).
pub fn encode_png_rgb(width: u32, height: u32, rgb: &[u8]) -> io::Result<Vec<u8>> {
    encode(width, height, rgb, 3)
}

pub(super) fn check_pixels(
    width: u32,
    height: u32,
    pixels: &[u8],
    channels: usize,
) -> io::Result<()> {
    let len = (width as usize)
        .checked_mul(height as usize)
        .and_then(|n| n.checked_mul(channels));
    if width == 0 || height == 0 || len != Some(pixels.len()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid image dimensions or pixel buffer length",
        ));
    }
    Ok(())
}

fn encode(width: u32, height: u32, pixels: &[u8], channels: usize) -> io::Result<Vec<u8>> {
    check_pixels(width, height, pixels, channels)?;
    let mut deflate = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    // Sub filtering makes flat shaded triangles and antialiased edges compress well.
    let stride = width as usize * channels;
    let mut filtered = vec![0; stride + 1];
    filtered[0] = 1;
    for row in pixels.chunks_exact(stride) {
        for (x, &v) in row.iter().enumerate() {
            filtered[x + 1] = v.wrapping_sub(if x < channels { 0 } else { row[x - channels] });
        }
        deflate.write_all(&filtered)?;
    }
    let compressed = deflate.finish()?;
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend(width.to_be_bytes());
    ihdr.extend(height.to_be_bytes());
    ihdr.extend([8, if channels == 4 { 6 } else { 2 }, 0, 0, 0]);
    chunk(&mut png, b"IHDR", &ihdr);
    chunk(&mut png, b"IDAT", &compressed);
    chunk(&mut png, b"IEND", &[]);
    Ok(png)
}

fn chunk(png: &mut Vec<u8>, kind: &[u8; 4], bytes: &[u8]) {
    png.extend((bytes.len() as u32).to_be_bytes());
    let start = png.len();
    png.extend(kind);
    png.extend(bytes);
    // IEEE CRC32 over the chunk type and payload, as in the original preview writer.
    let mut crc = !0u32;
    for &b in &png[start..] {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & 0u32.wrapping_sub(crc & 1));
        }
    }
    png.extend((!crc).to_be_bytes());
}
