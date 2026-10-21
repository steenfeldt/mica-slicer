// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Small decoders for embedded thumbnails: Base64 (text G-code) and QOI (Prusa binary
//! G-code). Both formats are public specifications; PNG and JPEG are kept as they are.

/// Decodes standard Base64, ignoring whitespace. `None` on any other invalid character.
pub fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let mut acc = 0u32;
    let mut bits = 0;
    for b in text.bytes() {
        let v = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            b' ' | b'\t' | b'\r' | b'\n' => continue,
            _ => return None,
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// Decodes a QOI image to (width, height, RGBA8). Bounded to 4096 × 4096.
pub fn qoi_decode(data: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    if data.len() < 14 || &data[..4] != b"qoif" {
        return None;
    }
    let w = u32::from_be_bytes(data[4..8].try_into().ok()?);
    let h = u32::from_be_bytes(data[8..12].try_into().ok()?);
    if w == 0 || h == 0 || w > 4096 || h > 4096 {
        return None;
    }
    let n = (w * h) as usize;
    let mut out = Vec::with_capacity(n * 4);
    let mut index = [[0u8; 4]; 64];
    let mut px = [0u8, 0, 0, 255];
    let mut i = 14;
    let hash = |p: [u8; 4]| {
        (usize::from(p[0]) * 3
            + usize::from(p[1]) * 5
            + usize::from(p[2]) * 7
            + usize::from(p[3]) * 11)
            % 64
    };
    while out.len() < n * 4 {
        let b = *data.get(i)?;
        i += 1;
        let mut run = 1;
        match b {
            0xfe => {
                px[..3].copy_from_slice(data.get(i..i + 3)?);
                i += 3;
            }
            0xff => {
                px.copy_from_slice(data.get(i..i + 4)?);
                i += 4;
            }
            _ => match b >> 6 {
                0 => px = index[usize::from(b & 63)],
                1 => {
                    px[0] = px[0].wrapping_add((b >> 4) & 3).wrapping_sub(2);
                    px[1] = px[1].wrapping_add((b >> 2) & 3).wrapping_sub(2);
                    px[2] = px[2].wrapping_add(b & 3).wrapping_sub(2);
                }
                2 => {
                    let b2 = *data.get(i)?;
                    i += 1;
                    let dg = (b & 63).wrapping_sub(32);
                    px[0] = px[0].wrapping_add(dg.wrapping_sub(8).wrapping_add(b2 >> 4));
                    px[1] = px[1].wrapping_add(dg);
                    px[2] = px[2].wrapping_add(dg.wrapping_sub(8).wrapping_add(b2 & 15));
                }
                _ => run = usize::from(b & 63) + 1,
            },
        }
        index[hash(px)] = px;
        for _ in 0..run {
            if out.len() >= n * 4 {
                break;
            }
            out.extend_from_slice(&px);
        }
    }
    Some((w, h, out))
}

/// Normalises an embedded thumbnail to bytes the GUI can show: PNG and JPEG pass through,
/// QOI becomes PNG. Returns the bytes and their file extension.
pub fn normalise(data: Vec<u8>) -> Option<(Vec<u8>, &'static str)> {
    if data.starts_with(b"\x89PNG") {
        Some((data, "png"))
    } else if data.starts_with(&[0xff, 0xd8]) {
        Some((data, "jpg"))
    } else if data.starts_with(b"qoif") {
        let (w, h, rgba) = qoi_decode(&data)?;
        mica_gcode::thumbnails::encode_png(w, h, &rgba)
            .ok()
            .map(|p| (p, "png"))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round_trips_known_values() {
        assert_eq!(base64_decode("TWFu").unwrap(), b"Man");
        assert_eq!(base64_decode("TWE=").unwrap(), b"Ma");
        assert_eq!(base64_decode("TQ==\n").unwrap(), b"M");
        assert!(base64_decode("T*Q").is_none());
    }

    #[test]
    fn qoi_matches_the_encoder() {
        let (w, h) = (7u32, 5u32);
        let rgba: Vec<u8> = (0..w * h)
            .flat_map(|i| {
                [
                    (i * 9) as u8,
                    200,
                    (i % 3 * 60) as u8,
                    if i % 4 == 0 { 0 } else { 255 },
                ]
            })
            .collect();
        let qoi = mica_gcode::thumbnails::encode_qoi(w, h, &rgba).unwrap();
        let (dw, dh, back) = qoi_decode(&qoi).unwrap();
        assert_eq!((dw, dh), (w, h));
        assert_eq!(back, rgba);
        let (png, ext) = normalise(qoi).unwrap();
        assert_eq!(ext, "png");
        assert!(png.starts_with(b"\x89PNG"));
    }
}
