// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 bundled_deps/qoi/qoi.h :: Data Format
// Changes: Independent safe Rust encoder from the format description; RGBA8 only, no C library.
// QOI format documentation: Copyright (c) 2021 Dominic Szablewski (MIT; see THIRD_PARTY.md).

use std::io;

/// Encode top-to-bottom, unpremultiplied RGBA8 pixels as QOI (sRGB, linear alpha).
pub fn encode_qoi(width: u32, height: u32, rgba: &[u8]) -> io::Result<Vec<u8>> {
    super::png::check_pixels(width, height, rgba, 4)?;
    let mut out = b"qoif".to_vec();
    out.extend(width.to_be_bytes());
    out.extend(height.to_be_bytes());
    out.extend([4, 0]);
    let mut index = [[0u8; 4]; 64];
    let mut previous = [0, 0, 0, 255];
    let mut run = 0u8;
    for &pixel in rgba.as_chunks::<4>().0 {
        if pixel == previous {
            run += 1;
            if run == 62 {
                out.push(0xc0 | (run - 1));
                run = 0;
            }
            continue;
        }
        if run != 0 {
            out.push(0xc0 | (run - 1));
            run = 0;
        }
        let [r, g, b, a] = pixel.map(usize::from);
        let slot = (r * 3 + g * 5 + b * 7 + a * 11) % 64;
        if index[slot] == pixel {
            out.push(slot as u8);
        } else if pixel[3] != previous[3] {
            out.push(0xff);
            out.extend(pixel);
        } else {
            let [dr, dg, db] =
                std::array::from_fn(|i| i16::from(pixel[i].wrapping_sub(previous[i]) as i8));
            let (rg, bg) = (dr - dg, db - dg);
            if [dr, dg, db].iter().all(|v| (-2..=1).contains(v)) {
                out.push(0x40 | ((dr + 2) << 4 | (dg + 2) << 2 | (db + 2)) as u8);
            } else if (-32..=31).contains(&dg) && (-8..=7).contains(&rg) && (-8..=7).contains(&bg) {
                out.push(0x80 | (dg + 32) as u8);
                out.push(((rg + 8) << 4 | (bg + 8)) as u8);
            } else {
                out.push(0xfe);
                out.extend(&pixel[..3]);
            }
        }
        index[slot] = pixel;
        previous = pixel;
    }
    if run != 0 {
        out.push(0xc0 | (run - 1));
    }
    out.extend([0, 0, 0, 0, 0, 0, 0, 1]);
    Ok(out)
}
