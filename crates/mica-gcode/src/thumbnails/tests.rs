// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 bundled_deps/qoi/qoi.h :: Data Format
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/Thumbnails.hpp :: export_thumbnails_to_file
// Changes: Independent test decoders and renderer/format/oracle regressions, no upstream implementation.
// QOI documentation: Copyright (c) 2021 Dominic Szablewski (MIT; see THIRD_PARTY.md).

use super::*;
use std::io::Read;

pub(crate) fn decode_png(bytes: &[u8]) -> (u32, u32, Vec<u8>) {
    assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
    let be = |b: &[u8]| u32::from_be_bytes(b.try_into().unwrap());
    let (mut w, mut h, mut channels) = (0, 0, 0);
    let mut zlib = Vec::new();
    let mut pos = 8;
    let mut ended = false;
    while pos < bytes.len() {
        let len = be(&bytes[pos..pos + 4]) as usize;
        let body = &bytes[pos + 4..pos + 8 + len];
        assert_eq!(
            crate::bgcode::crc32::update(0, body),
            be(&bytes[pos + 8 + len..pos + 12 + len])
        );
        match &body[..4] {
            b"IHDR" => {
                assert_eq!(len, 13);
                w = be(&body[4..8]);
                h = be(&body[8..12]);
                assert_eq!(body[12], 8);
                channels = match body[13] {
                    2 => 3,
                    6 => 4,
                    _ => panic!("unexpected color type"),
                };
                assert_eq!(&body[14..], &[0, 0, 0]);
            }
            b"IDAT" => zlib.extend(&body[4..]),
            b"IEND" => {
                assert_eq!(len, 0);
                ended = true;
            }
            _ => panic!("unexpected PNG chunk"),
        }
        pos += len + 12;
    }
    assert!(ended);
    let mut raw = Vec::new();
    flate2::read::ZlibDecoder::new(zlib.as_slice())
        .read_to_end(&mut raw)
        .unwrap();
    let stride = w as usize * channels;
    assert_eq!(raw.len(), (stride + 1) * h as usize);
    let mut pixels = Vec::new();
    for row in raw.chunks_exact(stride + 1) {
        let mut decoded = row[1..].to_vec();
        match row[0] {
            0 => {}
            1 => {
                for x in channels..stride {
                    decoded[x] = decoded[x].wrapping_add(decoded[x - channels]);
                }
            }
            _ => panic!("unexpected PNG filter"),
        }
        pixels.extend(decoded);
    }
    (w, h, pixels)
}

pub(crate) fn decode_qoi(bytes: &[u8]) -> (u32, u32, Vec<u8>) {
    assert_eq!(&bytes[..4], b"qoif");
    assert_eq!(&bytes[12..14], &[4, 0]);
    let w = u32::from_be_bytes(bytes[4..8].try_into().unwrap());
    let h = u32::from_be_bytes(bytes[8..12].try_into().unwrap());
    let mut table = [[0u8; 4]; 64];
    let mut pixel = [0, 0, 0, 255];
    let mut pixels = Vec::new();
    let mut pos = 14;
    while pixels.len() < w as usize * h as usize * 4 {
        let op = bytes[pos];
        pos += 1;
        let mut repeat = 1;
        match op {
            0xfe => {
                pixel[..3].copy_from_slice(&bytes[pos..pos + 3]);
                pos += 3;
            }
            0xff => {
                pixel.copy_from_slice(&bytes[pos..pos + 4]);
                pos += 4;
            }
            _ => match op >> 6 {
                0 => pixel = table[op as usize],
                1 => {
                    for (i, shift) in [4, 2, 0].into_iter().enumerate() {
                        pixel[i] = pixel[i].wrapping_add((op >> shift & 3).wrapping_sub(2));
                    }
                }
                2 => {
                    let dg = (op & 63).wrapping_sub(32);
                    pixel[0] = pixel[0]
                        .wrapping_add(dg)
                        .wrapping_add((bytes[pos] >> 4).wrapping_sub(8));
                    pixel[1] = pixel[1].wrapping_add(dg);
                    pixel[2] = pixel[2]
                        .wrapping_add(dg)
                        .wrapping_add((bytes[pos] & 15).wrapping_sub(8));
                    pos += 1;
                }
                3 => repeat = usize::from(op & 63) + 1,
                _ => unreachable!(),
            },
        }
        let hash: usize = pixel
            .iter()
            .zip([3, 5, 7, 11])
            .map(|(&c, m)| usize::from(c) * m)
            .sum();
        table[hash % 64] = pixel;
        for _ in 0..repeat {
            pixels.extend(pixel);
        }
    }
    assert_eq!(pixels.len(), w as usize * h as usize * 4);
    assert_eq!(&bytes[pos..], &[0, 0, 0, 0, 0, 0, 0, 1]);
    (w, h, pixels)
}

pub(crate) fn decode_base64(s: &str) -> Vec<u8> {
    assert_eq!(s.len() % 4, 0);
    let mut out = Vec::new();
    for group in s.as_bytes().as_chunks::<4>().0.iter() {
        let mut n = 0u32;
        for &b in group {
            n = (n << 6)
                | match b {
                    b'A'..=b'Z' => u32::from(b - b'A'),
                    b'a'..=b'z' => u32::from(b - b'a' + 26),
                    b'0'..=b'9' => u32::from(b - b'0' + 52),
                    b'+' => 62,
                    b'/' => 63,
                    b'=' => 0,
                    _ => panic!("invalid base64"),
                };
        }
        out.push((n >> 16) as u8);
        if group[2] != b'=' {
            out.push((n >> 8) as u8);
        }
        if group[3] != b'=' {
            out.push(n as u8);
        }
    }
    out
}

pub(crate) fn cube() -> TriangleMesh {
    let mut mesh = TriangleMesh::load_stl(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/models/cube20.stl"
    ))
    .unwrap();
    mesh.place_on_bed(125.0, 110.0);
    mesh
}

#[test]
fn png_and_qoi_round_trip_all_channels() {
    let mut pixels = Vec::new();
    // Long runs, initial opaque black, alpha changes, index reuse, differences, luma, RGB.
    for p in [
        [0, 0, 0, 255],
        [0, 0, 0, 0],
        [200, 100, 50, 255],
        [201, 101, 51, 255],
        [211, 112, 63, 255],
        [200, 100, 50, 255],
    ] {
        pixels.extend(p);
    }
    pixels.extend([42, 75, 220, 128].repeat(200));
    let mut state = 7u32;
    for _ in 0..4096 {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        pixels.extend(state.to_le_bytes());
    }
    let w = (pixels.len() / 4) as u32;
    assert_eq!(
        decode_png(&encode_png(w, 1, &pixels).unwrap()),
        (w, 1, pixels.clone())
    );
    assert_eq!(
        decode_qoi(&encode_qoi(w, 1, &pixels).unwrap()),
        (w, 1, pixels)
    );
    let rgb = [255, 128, 0, 30, 40, 50].repeat(5);
    assert_eq!(
        decode_png(&encode_png_rgb(2, 5, &rgb).unwrap()),
        (2, 5, rgb)
    );
    for encode in [encode_png, encode_qoi] {
        assert!(encode(0, 1, &[]).is_err());
        assert!(encode(u32::MAX, u32::MAX, &[]).is_err());
        assert!(encode(2, 1, &[0; 4]).is_err());
    }
}

#[test]
fn qoi_known_opcodes_and_wraparound() {
    let pixels: Vec<u8> = [
        [255, 255, 255, 255],
        [0, 0, 0, 255],
        [10, 12, 14, 255],
        [200, 100, 50, 255],
        [10, 12, 14, 255],
        [1, 2, 3, 4],
        [1, 2, 3, 4],
    ]
    .into_iter()
    .flatten()
    .collect();
    let encoded = encode_qoi(7, 1, &pixels).unwrap();
    assert_eq!(
        &encoded[14..encoded.len() - 8],
        &[
            0x55, 0x7f, 0xac, 0x6a, 0xfe, 200, 100, 50, 49, 0xff, 1, 2, 3, 4, 0xc0
        ]
    );
    assert_eq!(decode_qoi(&encoded).2, pixels);
}

#[test]
fn thumbnail_list_legacy_formats_errors_and_order() {
    let (list, errors) =
        make_and_check_thumbnail_list("16x16/qoi, 313x173/QOI,480x240/QOI,380x285/PNG", "PNG");
    assert_eq!(errors, ThumbnailErrors::default());
    assert_eq!(
        list.iter().map(|s| (s.width, s.height)).collect::<Vec<_>>(),
        [(16, 16), (313, 173), (480, 240), (380, 285)]
    );
    assert_eq!(list[0].format, ThumbnailFormat::Qoi);
    assert_eq!(list[3].format, ThumbnailFormat::Png);
    assert_eq!(
        make_and_check_thumbnail_list("48x48,300x300/", "QOI").0[1].format,
        ThumbnailFormat::Qoi
    );
    assert_eq!(
        make_and_check_thumbnail_list("2.9x3.5/ png ", "").0[0].width,
        2
    );
    assert_eq!(
        make_and_check_thumbnail_list("", "bad"),
        (vec![], ThumbnailErrors::default())
    );
    let (list, errors) = make_and_check_thumbnail_list(
        "bad,0x1,1000x1,NaNx3,1x2/GIF,1x1/JPG,16oopsx16,1x1/PNG,",
        "",
    );
    assert!(errors.invalid_value && errors.out_of_range && errors.invalid_format);
    assert_eq!(list.len(), 3);
    assert_eq!(list[0].format, ThumbnailFormat::Png);
    let mut config = Config::defaults();
    config.set_raw("thumbnails", "16x16/JPG").unwrap();
    assert_eq!(
        generate(&config, &cube()).unwrap_err().kind(),
        io::ErrorKind::Unsupported
    );
    config.set_raw("thumbnails", "oops").unwrap();
    assert_eq!(
        generate(&config, &cube()).unwrap_err().kind(),
        io::ErrorKind::InvalidInput
    );
}

#[test]
fn colors_and_legacy_config() {
    let mut config = Config::defaults();
    config.set_raw("filament_colour", "invalid").unwrap();
    assert_eq!(filament_color(&config), [255, 128, 0]);
    config.set_raw("filament_colour", "#0123aB").unwrap();
    assert_eq!(filament_color(&config), [1, 35, 171]);
    config.set_raw("extruder_colour", "#abcdef").unwrap();
    assert_eq!(filament_color(&config), [171, 205, 239]);
    config.set_raw("extruder_colour", "bad").unwrap();
    assert_eq!(filament_color(&config), [1, 35, 171]);
    config.set_raw("thumbnails", "16x16").unwrap();
    config.set_raw("thumbnails_format", "QOI").unwrap();
    assert_eq!(
        generate(&config, &cube()).unwrap()[0].format,
        ThumbnailFormat::Qoi
    );
    config.set_raw("thumbnails", "").unwrap();
    assert!(generate(&config, &cube()).unwrap().is_empty());
}

#[test]
fn text_blocks_have_exact_lengths_wrapping_and_payloads() {
    for format in [ThumbnailFormat::Png, ThumbnailFormat::Qoi] {
        for len in [1usize, 2, 3, 57, 58, 59, 60, 117, 118, 256] {
            let data: Vec<u8> = (0..len).map(|i| i as u8).collect();
            let thumbnail = Thumbnail {
                format,
                width: 48,
                height: 48,
                data: data.clone(),
            };
            for orca in [false, true] {
                let mut out = Vec::new();
                write_text(&mut out, std::slice::from_ref(&thumbnail), orca).unwrap();
                let text = String::from_utf8(out).unwrap();
                let tag = if format == ThumbnailFormat::Png {
                    "thumbnail"
                } else {
                    "thumbnail_QOI"
                };
                let mut lines = text.lines().skip_while(|l| !l.contains(" begin "));
                assert_eq!(
                    lines.next().unwrap(),
                    format!("; {tag} begin 48x48 {}", len.div_ceil(3) * 4)
                );
                let payload: Vec<&str> = lines
                    .by_ref()
                    .take_while(|l| !l.ends_with(" end"))
                    .map(|l| l.strip_prefix("; ").unwrap())
                    .collect();
                for line in &payload[..payload.len() - 1] {
                    assert_eq!(line.len(), 78);
                }
                assert!(payload.last().unwrap().len() <= 78);
                assert_eq!(decode_base64(&payload.concat()), data);
                assert!(text.contains(&format!("; {tag} end\n")));
                assert_eq!(text.starts_with("; THUMBNAIL_BLOCK_START\n"), orca);
                assert_eq!(text.contains("; THUMBNAIL_BLOCK_END\n"), orca);
            }
        }
    }
    assert_eq!(base64(b"f"), "Zg==");
    assert_eq!(base64(b"fo"), "Zm8=");
    assert_eq!(base64(b"foo"), "Zm9v");
}

#[test]
fn renderer_is_centered_shaded_transparent_and_translation_invariant() {
    let mut mesh = cube();
    for camera in [Camera::Prusa, Camera::Orca] {
        for (w, h) in [(16, 16), (48, 48), (300, 300), (480, 240)] {
            let image = render(&mesh, w, h, [255, 128, 0], camera).unwrap();
            let occupied: Vec<_> = image
                .rgba
                .as_chunks::<4>()
                .0
                .iter()
                .enumerate()
                .filter(|(_, p)| p[3] != 0)
                .collect();
            assert!(occupied.len() > usize::from(w) * usize::from(h) / 4);
            let xmin = occupied
                .iter()
                .map(|(i, _)| i % usize::from(w))
                .min()
                .unwrap();
            let xmax = occupied
                .iter()
                .map(|(i, _)| i % usize::from(w))
                .max()
                .unwrap();
            let ymin = occupied
                .iter()
                .map(|(i, _)| i / usize::from(w))
                .min()
                .unwrap();
            let ymax = occupied
                .iter()
                .map(|(i, _)| i / usize::from(w))
                .max()
                .unwrap();
            assert!((xmin + xmax).abs_diff(usize::from(w) - 1) <= 1);
            assert!((ymin + ymax).abs_diff(usize::from(h) - 1) <= 1);
            assert_eq!(image.rgba[3], 0);
            let colors: std::collections::BTreeSet<_> = occupied
                .iter()
                .filter(|(_, p)| p[3] == 255)
                .map(|(_, p)| p[0])
                .collect();
            assert!(colors.len() >= 3, "three differently lit visible faces");
            assert_eq!(
                decode_png(&encode_png(w.into(), h.into(), &image.rgba).unwrap()).2,
                image.rgba
            );
            assert_eq!(
                decode_qoi(&encode_qoi(w.into(), h.into(), &image.rgba).unwrap()).2,
                image.rgba
            );
        }
    }
    let before = render(&mesh, 48, 48, [255, 128, 0], Camera::Prusa).unwrap();
    mesh.translate([100., -500., 0.]);
    assert_eq!(
        render(&mesh, 48, 48, [255, 128, 0], Camera::Prusa)
            .unwrap()
            .rgba,
        before.rgba
    );
    mesh.faces.reverse();
    assert_eq!(
        render(&mesh, 48, 48, [255, 128, 0], Camera::Prusa)
            .unwrap()
            .rgba,
        before.rgba
    );
    assert!(render(&mesh, 0, 48, [0; 3], Camera::Prusa).is_err());
    mesh.faces[0][0] = u32::MAX;
    assert!(render(&mesh, 48, 48, [0; 3], Camera::Prusa).is_err());
}

#[test]
#[ignore = "manual image/performance oracle; writes PNGs into the system temp directory"]
fn render_oracle() {
    let dir = std::env::temp_dir().join("mica-thumbnail-oracle");
    std::fs::create_dir_all(&dir).unwrap();
    for name in ["cube20", "3DBenchy"] {
        let mut mesh = TriangleMesh::load_stl(format!(
            "{}/../../tests/models/{name}.stl",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap();
        mesh.place_on_bed(125.0, 110.0);
        for (camera, sizes) in [
            (
                Camera::Prusa,
                vec![(16, 16), (313, 173), (480, 240), (380, 285)],
            ),
            (Camera::Orca, vec![(48, 48), (300, 300)]),
        ] {
            for (w, h) in sizes {
                let start = std::time::Instant::now();
                let image = render(&mesh, w, h, [255, 128, 0], camera).unwrap();
                eprintln!("{name} {camera:?} {w}x{h}: {:?}", start.elapsed());
                let png = encode_png(w.into(), h.into(), &image.rgba).unwrap();
                std::fs::write(dir.join(format!("{name}-{camera:?}-{w}x{h}.png")), png).unwrap();
            }
        }
    }
    if let Ok(path) = std::env::var("MICA_THUMBNAIL_ORACLE_STL") {
        let mut mesh = TriangleMesh::load_stl(path).unwrap();
        mesh.place_on_bed(125.0, 110.0);
        for (w, h) in [(480, 240), (512, 512)] {
            let start = std::time::Instant::now();
            let image = render(&mesh, w, h, [255, 128, 0], Camera::Orca).unwrap();
            eprintln!(
                "external mesh: {} triangles {w}x{h}: {:?}",
                mesh.faces.len(),
                start.elapsed()
            );
            std::fs::write(
                dir.join(format!("external-Orca-{w}x{h}.png")),
                encode_png(w.into(), h.into(), &image.rgba).unwrap(),
            )
            .unwrap();
        }
    }
    // Subdivide every cube triangle: 4.32 million distinct triangles with small projected areas.
    let source = cube();
    let mut dense = TriangleMesh::default();
    let n = 600;
    for f in source.faces {
        let [a, b, c] = f.map(|i| source.vertices[i as usize]);
        let mut rows = Vec::new();
        for y in 0..=n {
            rows.push(dense.vertices.len() as u32);
            for x in 0..=n - y {
                dense.vertices.push(std::array::from_fn(|i| {
                    a[i] + (b[i] - a[i]) * x as f32 / n as f32 + (c[i] - a[i]) * y as f32 / n as f32
                }));
            }
        }
        for y in 0..n {
            for x in 0..n - y {
                let (i, j) = (rows[y] + x as u32, rows[y + 1] + x as u32);
                dense.faces.push([i, i + 1, j]);
                if x + 1 < n - y {
                    dense.faces.push([i + 1, j + 1, j]);
                }
            }
        }
    }
    let start = std::time::Instant::now();
    let image = render(&dense, 480, 240, [255, 128, 0], Camera::Prusa).unwrap();
    eprintln!(
        "{} triangles at 480x240: {:?}; images in {}",
        dense.faces.len(),
        start.elapsed(),
        dir.display()
    );
    std::fs::write(
        dir.join("dense-cube-480x240.png"),
        encode_png(480, 240, &image.rgba).unwrap(),
    )
    .unwrap();
}

#[test]
fn per_face_colours() {
    let mesh = cube();
    let orbit = Orbit {
        azimuth: 35.,
        elevation: 28.,
        zoom: 1.,
        fit: Fit::Box,
    };
    let plain = render_orbit(&mesh, 64, 64, [200, 40, 40], orbit).unwrap();
    let same = render_orbit_colored(&mesh, 64, 64, &[[200, 40, 40]; 12], orbit).unwrap();
    assert_eq!(
        plain.rgba, same.rgba,
        "one colour per face draws as one colour"
    );
    // The top in blue: blue shows, red stays on the sides.
    let top = mesh.bounds().1[2];
    let colours: Vec<[u8; 3]> = mesh
        .faces
        .iter()
        .map(|f| {
            if f.iter().all(|&v| mesh.vertices[v as usize][2] == top) {
                [40, 40, 200]
            } else {
                [200, 40, 40]
            }
        })
        .collect();
    assert_eq!(colours.iter().filter(|c| c[2] == 200).count(), 2);
    let two = render_orbit_colored(&mesh, 64, 64, &colours, orbit).unwrap();
    let px = |img: &Image, f: fn(&[u8]) -> bool| img.rgba.chunks(4).filter(|p| f(p)).count();
    assert!(px(&two, |p| p[3] == 255 && p[2] > p[0]) > 100);
    assert!(px(&two, |p| p[3] == 255 && p[0] > p[2]) > 100);
    assert_eq!(px(&plain, |p| p[3] == 255 && p[2] > p[0]), 0);
}
