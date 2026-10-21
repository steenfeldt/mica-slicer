// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 bundled_deps/qoi/qoi.h :: Data Format
// Changes: Bounded structural QOI decoder; original PNG CRC/scanline and base64 validation. No image allocation.
// QOI format documentation: Copyright (c) 2021 Dominic Szablewski (MIT; see THIRD_PARTY.md).

use std::io::Read;
pub(super) fn base64(data: &[u8]) -> Result<Vec<u8>, String> {
    if !data.len().is_multiple_of(4) {
        return Err("thumbnail base64 is truncated".into());
    }
    let mut out = Vec::new();
    for (index, chunk) in data.as_chunks::<4>().0.iter().enumerate() {
        let mut n = 0u32;
        let mut padding = 0;
        for &c in chunk {
            let value = match c {
                b'A'..=b'Z' => c - b'A',
                b'a'..=b'z' => c - b'a' + 26,
                b'0'..=b'9' => c - b'0' + 52,
                b'+' => 62,
                b'/' => 63,
                b'=' => {
                    padding += 1;
                    0
                }
                _ => return Err("invalid thumbnail base64 character".into()),
            };
            if c != b'=' && padding > 0 {
                return Err("invalid base64 padding".into());
            }
            n = (n << 6) | u32::from(value);
        }
        if padding > 2 || (padding > 0 && index + 1 != data.len() / 4) {
            return Err("invalid base64 padding".into());
        }
        out.extend_from_slice(&n.to_be_bytes()[1..4 - padding]);
    }
    Ok(out)
}
fn u32be(data: &[u8]) -> Result<usize, String> {
    let bytes: [u8; 4] = data
        .get(..4)
        .ok_or("truncated image")?
        .try_into()
        .map_err(|_| "truncated image")?;
    Ok(u32::from_be_bytes(bytes) as usize)
}
pub(super) fn check(data: &[u8], format: &str) -> Result<(), String> {
    if format.to_ascii_uppercase().contains("QOI") || data.starts_with(b"qoif") {
        return qoi(data);
    }
    if !data.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err("thumbnail is not a supported PNG/QOI image".into());
    }
    let mut pos = 8;
    let mut header = None;
    let mut compressed = Vec::new();
    let mut end = false;
    while pos < data.len() {
        let len = u32be(&data[pos..])?;
        let finish = pos
            .checked_add(12)
            .and_then(|v| v.checked_add(len))
            .ok_or("PNG length overflow")?;
        if finish > data.len() {
            return Err("truncated PNG chunk".into());
        }
        let kind = &data[pos + 4..pos + 8];
        let payload = &data[pos + 8..pos + 8 + len];
        let expected = u32be(&data[finish - 4..finish])? as u32;
        if crate::bgcode::crc32::update(0, &data[pos + 4..finish - 4]) != expected {
            return Err("PNG checksum mismatch".into());
        }
        match kind {
            b"IHDR" if header.is_none() && pos == 8 && len == 13 => {
                let width = u32be(payload)?;
                let height = u32be(&payload[4..])?;
                let channels = match payload[9] {
                    0 | 3 => 1,
                    2 => 3,
                    4 => 2,
                    6 => 4,
                    _ => return Err("invalid PNG color type".into()),
                };
                let depth = payload[8] as usize;
                if ![1, 2, 4, 8, 16].contains(&depth) || payload[10..] != [0, 0, 0] {
                    return Err("unsupported PNG bit depth/compression/interlace".into());
                }
                let stride = width
                    .checked_mul(channels * depth)
                    .ok_or("PNG dimensions overflow")?
                    .div_ceil(8)
                    + 1;
                let size = stride
                    .checked_mul(height)
                    .ok_or("PNG dimensions overflow")?;
                if width == 0 || height == 0 || size > 64 * 1024 * 1024 {
                    return Err("PNG dimensions exceed thumbnail limit".into());
                }
                header = Some((stride, size));
            }
            b"IDAT" => compressed.extend_from_slice(payload),
            b"IEND" if len == 0 => {
                end = true;
                if finish != data.len() {
                    return Err("data after PNG end".into());
                }
            }
            _ => {}
        }
        pos = finish;
    }
    let (stride, size) = header.ok_or("missing PNG header")?;
    if !end {
        return Err("missing PNG end".into());
    }
    let mut decoder = flate2::read::ZlibDecoder::new(compressed.as_slice());
    let mut raw = Vec::new();
    decoder
        .by_ref()
        .take(size as u64 + 1)
        .read_to_end(&mut raw)
        .map_err(|e| format!("PNG decompression: {e}"))?;
    if raw.len() != size || decoder.total_in() != compressed.len() as u64 {
        return Err("PNG decompressed size/trailing data mismatch".into());
    }
    if raw.chunks_exact(stride).any(|row| row[0] > 4) {
        return Err("invalid PNG row filter".into());
    }
    Ok(())
}
fn qoi(data: &[u8]) -> Result<(), String> {
    if data.len() < 22
        || !data.starts_with(b"qoif")
        || data[data.len() - 8..] != [0, 0, 0, 0, 0, 0, 0, 1]
    {
        return Err("invalid QOI header/end marker".into());
    }
    let pixels = u32be(&data[4..])?
        .checked_mul(u32be(&data[8..])?)
        .ok_or("QOI dimensions overflow")?;
    if pixels == 0 || pixels > 16 * 1024 * 1024 || ![3, 4].contains(&data[12]) || data[13] > 1 {
        return Err("invalid QOI dimensions/channels".into());
    }
    let mut pos = 14;
    let mut count = 0;
    while pos < data.len() - 8 && count < pixels {
        let op = data[pos];
        pos += 1;
        let (extra, run) = match op {
            0xfe => (3, 1),
            0xff => (4, 1),
            v if v & 0xc0 == 0x80 => (1, 1),
            v if v & 0xc0 == 0xc0 => (0, usize::from(v & 0x3f) + 1),
            _ => (0, 1),
        };
        pos += extra;
        count += run;
    }
    if count != pixels || pos != data.len() - 8 {
        return Err("QOI pixel count/truncation mismatch".into());
    }
    Ok(())
}

pub(super) fn dimensions(data: &[u8], expected: (usize, usize)) -> Result<(), String> {
    let offset = if data.starts_with(b"qoif") { 4 } else { 16 };
    let actual = (
        u32be(data.get(offset..).ok_or("truncated image dimensions")?)?,
        u32be(data.get(offset + 4..).ok_or("truncated image dimensions")?)?,
    );
    if actual != expected {
        return Err(format!(
            "thumbnail dimensions {actual:?} differ from metadata {expected:?}"
        ));
    }
    Ok(())
}
