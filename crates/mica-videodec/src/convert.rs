// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Pixel conversions from what the system decoders hand out to tightly packed RGBA.
//! Not every backend needs every conversion.
#![allow(dead_code)]

/// BGRA rows of `stride` bytes → RGBA. `None` when `src` is too short.
pub(crate) fn bgra_to_rgba(
    src: &[u8],
    width: usize,
    height: usize,
    stride: usize,
) -> Option<Vec<u8>> {
    let row = width.checked_mul(4)?;
    if stride < row || src.len() < stride.checked_mul(height.checked_sub(1)?)? + row {
        return None;
    }
    let mut out = Vec::with_capacity(row * height);
    for y in 0..height {
        for p in src[y * stride..y * stride + row].as_chunks::<4>().0 {
            out.extend_from_slice(&[p[2], p[1], p[0], 255]);
        }
    }
    Some(out)
}

/// NV12 (a Y plane, then interleaved Cb/Cr at half resolution), video range BT.601 → RGBA.
/// `None` when the planes are too short.
pub(crate) fn nv12_to_rgba(
    y_plane: &[u8],
    y_stride: usize,
    uv_plane: &[u8],
    uv_stride: usize,
    width: usize,
    height: usize,
) -> Option<Vec<u8>> {
    if width == 0 || height == 0 || y_stride < width || uv_stride < width.div_ceil(2) * 2 {
        return None;
    }
    let ch = height.div_ceil(2);
    if y_plane.len() < y_stride * (height - 1) + width
        || uv_plane.len() < uv_stride * (ch - 1) + width.div_ceil(2) * 2
    {
        return None;
    }
    let mut out = Vec::with_capacity(width * height * 4);
    for y in 0..height {
        let yr = &y_plane[y * y_stride..];
        let uvr = &uv_plane[(y / 2) * uv_stride..];
        for x in 0..width {
            let c = (i32::from(yr[x]) - 16).max(0) * 298;
            let d = i32::from(uvr[x / 2 * 2]) - 128;
            let e = i32::from(uvr[x / 2 * 2 + 1]) - 128;
            let clamp = |v: i32| ((v + 128) >> 8).clamp(0, 255) as u8;
            out.extend_from_slice(&[
                clamp(c + 409 * e),
                clamp(c - 100 * d - 208 * e),
                clamp(c + 516 * d),
                255,
            ]);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bgra_rows_lose_their_padding() {
        let src = [
            1, 2, 3, 4, 5, 6, 7, 8, 0, 0, 9, 10, 11, 12, 13, 14, 15, 16, 0, 0,
        ];
        let out = bgra_to_rgba(&src, 2, 2, 10).unwrap();
        assert_eq!(
            out,
            [3, 2, 1, 255, 7, 6, 5, 255, 11, 10, 9, 255, 15, 14, 13, 255]
        );
        assert!(bgra_to_rgba(&src[..15], 2, 2, 10).is_none());
    }

    #[test]
    fn nv12_greys_and_colours() {
        // 2×2: video-range white, black, and a strongly red chroma pair.
        let y = [235, 16, 81, 81];
        let uv = [128, 128];
        let out = nv12_to_rgba(&y, 2, &uv, 2, 2, 1).unwrap();
        assert_eq!(&out[..4], &[255, 255, 255, 255]);
        assert_eq!(&out[4..8], &[0, 0, 0, 255]);
        // BT.601 red is Y 81, Cb 90, Cr 240.
        let red = nv12_to_rgba(&y[2..], 2, &[90, 240], 2, 2, 1).unwrap();
        assert!(red[0] > 240 && red[1] < 20 && red[2] < 20, "{red:?}");
        assert!(nv12_to_rgba(&y, 2, &[], 2, 2, 2).is_none());
    }
}
