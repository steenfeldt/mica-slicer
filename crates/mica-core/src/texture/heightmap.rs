// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: bumpmesh@a6d6d6e6df js/displacement.js :: sampleBilinear
// Changes: f32 grey map instead of RGBA image data; wrapping separable Gaussian smoothing (sigma in pixels of a 512 px map, as BumpMesh's texture smoothing) instead of a canvas blur; PNG decoding and normalisation of custom images for projects.
// Copyright (c) 2026 CNCKitchen (Stefan Hermann) and contributors (BumpMesh, AGPL-3.0-only)

//! Grayscale height maps: 0 (black, low) to 1 (white, high), tiling in both directions.
use std::io::Cursor;

/// Largest side of a custom image kept in a project (BumpMesh keeps custom maps up to 2048 px).
pub const CUSTOM_MAX: usize = 2048;

#[derive(Clone, Debug, PartialEq)]
pub struct HeightMap {
    w: usize,
    h: usize,
    px: Vec<f32>,
}

impl HeightMap {
    /// A map from row-major values (row 0 is the top row of the image).
    pub fn new(w: usize, h: usize, px: Vec<f32>) -> Result<Self, String> {
        if w == 0 || h == 0 || px.len() != w * h {
            return Err("texture: the image is empty or its size does not match".into());
        }
        Ok(Self { w, h, px })
    }
    pub fn from_gray8(w: usize, h: usize, bytes: &[u8]) -> Result<Self, String> {
        Self::new(w, h, bytes.iter().map(|&b| f32::from(b) / 255.).collect())
    }
    pub fn width(&self) -> usize {
        self.w
    }
    pub fn height(&self) -> usize {
        self.h
    }
    /// Height over width, for keeping the image's proportions on the part.
    pub fn aspect(&self) -> f64 {
        self.h as f64 / self.w as f64
    }
    pub fn pixels(&self) -> &[f32] {
        &self.px
    }

    /// Any 8 or 16 bit PNG; colour images are converted to their brightness, alpha is ignored.
    pub fn from_png(bytes: &[u8]) -> Result<Self, String> {
        let mut decoder = png::Decoder::new(Cursor::new(bytes));
        decoder.set_transformations(png::Transformations::normalize_to_color8());
        let mut reader = decoder
            .read_info()
            .map_err(|e| format!("texture image: {e}"))?;
        let (w, h) = reader.info().size();
        let (w, h) = (w as usize, h as usize);
        if w == 0 || h == 0 || w > 16384 || h > 16384 {
            return Err("texture image: unsupported size".into());
        }
        let mut buf = vec![
            0;
            reader
                .output_buffer_size()
                .ok_or("texture image: too large")?
        ];
        let info = reader
            .next_frame(&mut buf)
            .map_err(|e| format!("texture image: {e}"))?;
        let channels = info.color_type.samples();
        let line = info.line_size;
        let mut px = Vec::with_capacity(w * h);
        for y in 0..h {
            let row = &buf[y * line..];
            for x in 0..w {
                let p = &row[x * channels..];
                let grey = match channels {
                    1 | 2 => f32::from(p[0]),
                    _ => {
                        0.299 * f32::from(p[0]) + 0.587 * f32::from(p[1]) + 0.114 * f32::from(p[2])
                    }
                };
                px.push(grey / 255.);
            }
        }
        Self::new(w, h, px)
    }

    /// Binary PGM (P5, 8 bit).
    pub fn from_pgm(bytes: &[u8]) -> Result<Self, String> {
        let mut fields = Vec::new();
        let mut i = 0;
        while fields.len() < 4 && i < bytes.len() {
            while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            if bytes.get(i) == Some(&b'#') {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
            let start = i;
            while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            fields.push(String::from_utf8_lossy(&bytes[start..i]).into_owned());
        }
        i += 1;
        let num = |k: usize| fields.get(k).and_then(|f| f.parse::<usize>().ok());
        let (Some(w), Some(h), Some(255)) = (num(1), num(2), num(3)) else {
            return Err("texture: expected an 8-bit binary PGM".into());
        };
        if fields[0] != "P5" || w * h > 4096 * 4096 {
            return Err("texture: expected an 8-bit binary PGM up to 4096 px".into());
        }
        let data = bytes
            .get(i..i + w * h)
            .ok_or("texture: truncated PGM data")?;
        Self::from_gray8(w, h, data)
    }

    /// The map as 8-bit grey values, row-major.
    pub fn to_gray8(&self) -> Vec<u8> {
        self.px
            .iter()
            .map(|v| (v.clamp(0., 1.) * 255.).round() as u8)
            .collect()
    }

    /// An 8-bit grey PNG of the map.
    pub fn to_png(&self) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut e = png::Encoder::new(&mut out, self.w as u32, self.h as u32);
            e.set_color(png::ColorType::Grayscale);
            e.set_depth(png::BitDepth::Eight);
            e.set_compression(png::Compression::High);
            let mut w = e.write_header().expect("PNG header into memory");
            w.write_image_data(&self.to_gray8())
                .expect("PNG data into memory");
        }
        out
    }

    /// Area-averaged copy whose longest side is at most `max` pixels.
    pub fn fit(&self, max: usize) -> Self {
        let longest = self.w.max(self.h);
        if longest <= max {
            return self.clone();
        }
        let s = max as f64 / longest as f64;
        let (nw, nh) = (
            ((self.w as f64 * s).round() as usize).max(1),
            ((self.h as f64 * s).round() as usize).max(1),
        );
        let (fx, fy) = (self.w as f64 / nw as f64, self.h as f64 / nh as f64);
        let mut px = Vec::with_capacity(nw * nh);
        for y in 0..nh {
            let (y0, y1) = (y as f64 * fy, (y + 1) as f64 * fy);
            for x in 0..nw {
                let (x0, x1) = (x as f64 * fx, (x + 1) as f64 * fx);
                let (mut sum, mut weight) = (0., 0.);
                let mut sy = y0.floor() as usize;
                while (sy as f64) < y1 && sy < self.h {
                    let wy = (y1.min(sy as f64 + 1.) - y0.max(sy as f64)).max(0.);
                    let mut sx = x0.floor() as usize;
                    while (sx as f64) < x1 && sx < self.w {
                        let wx = (x1.min(sx as f64 + 1.) - x0.max(sx as f64)).max(0.);
                        sum += f64::from(self.px[sy * self.w + sx]) * wx * wy;
                        weight += wx * wy;
                        sx += 1;
                    }
                    sy += 1;
                }
                px.push((sum / weight.max(1e-12)) as f32);
            }
        }
        Self { w: nw, h: nh, px }
    }

    /// Black and white swapped.
    pub fn inverted(&self) -> Self {
        Self {
            w: self.w,
            h: self.h,
            px: self.px.iter().map(|v| 1. - v).collect(),
        }
    }

    /// Gaussian smoothing that wraps around the edges, so the map keeps tiling. `sigma` is in
    /// pixels of a 512 px map (BumpMesh's unit), scaled to this map's width.
    pub fn smoothed(&self, sigma: f64) -> Self {
        let sigma = sigma * self.w as f64 / 512.;
        if sigma < 0.05 {
            return self.clone();
        }
        let radius = (sigma * 3.).ceil() as i64;
        let kernel: Vec<f64> = (-radius..=radius)
            .map(|i| (-(i * i) as f64 / (2. * sigma * sigma)).exp())
            .collect();
        let total: f64 = kernel.iter().sum();
        let kernel: Vec<f64> = kernel.iter().map(|k| k / total).collect();
        let (w, h) = (self.w as i64, self.h as i64);
        let mut tmp = vec![0f32; self.px.len()];
        for y in 0..h {
            for x in 0..w {
                let mut s = 0.;
                for (k, &kv) in kernel.iter().enumerate() {
                    let sx = (x + k as i64 - radius).rem_euclid(w);
                    s += f64::from(self.px[(y * w + sx) as usize]) * kv;
                }
                tmp[(y * w + x) as usize] = s as f32;
            }
        }
        let mut px = vec![0f32; self.px.len()];
        for y in 0..h {
            for x in 0..w {
                let mut s = 0.;
                for (k, &kv) in kernel.iter().enumerate() {
                    let sy = (y + k as i64 - radius).rem_euclid(h);
                    s += f64::from(tmp[(sy * w + x) as usize]) * kv;
                }
                px[(y * w + x) as usize] = s as f32;
            }
        }
        Self {
            w: self.w,
            h: self.h,
            px,
        }
    }

    /// Bilinear, wrapping, texel centres at (i + 0.5) / w; v = 0 is the bottom row (GL convention).
    pub fn sample(&self, u: f64, v: f64) -> f64 {
        let u = u.rem_euclid(1.);
        let v = 1. - v.rem_euclid(1.);
        let fx = u * self.w as f64 - 0.5;
        let fy = v * self.h as f64 - 0.5;
        let (x0, y0) = (fx.floor(), fy.floor());
        let (tx, ty) = (fx - x0, fy - y0);
        let wrap = |a: f64, n: usize| (a as i64).rem_euclid(n as i64) as usize;
        let (x0, x1) = (wrap(x0, self.w), wrap(x0 + 1., self.w));
        let (y0, y1) = (wrap(y0, self.h), wrap(y0 + 1., self.h));
        let p = |x: usize, y: usize| f64::from(self.px[y * self.w + x]);
        p(x0, y0) * (1. - tx) * (1. - ty)
            + p(x1, y0) * tx * (1. - ty)
            + p(x0, y1) * (1. - tx) * ty
            + p(x1, y1) * tx * ty
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp() -> HeightMap {
        let (w, h) = (8, 4);
        HeightMap::new(
            w,
            h,
            (0..w * h)
                .map(|i| ((i % w) as f32 / w as f32 + (i / w) as f32 * 0.1) % 1.)
                .collect(),
        )
        .unwrap()
    }

    #[test]
    fn sampling_wraps_at_tile_edges() {
        let m = ramp();
        for k in 0..16 {
            let t = k as f64 / 16.;
            assert!((m.sample(0., t) - m.sample(1., t)).abs() < 1e-12);
            assert!((m.sample(t, -1.) - m.sample(t, 0.)).abs() < 1e-12);
        }
    }

    #[test]
    fn png_round_trip_keeps_values() {
        let m = HeightMap::from_gray8(3, 2, &[0, 64, 128, 192, 255, 7]).unwrap();
        let back = HeightMap::from_png(&m.to_png()).unwrap();
        assert_eq!(back.to_gray8(), vec![0, 64, 128, 192, 255, 7]);
    }

    #[test]
    fn smoothing_keeps_the_mean_and_tiles() {
        let m = ramp().smoothed(200.);
        let mean = |m: &HeightMap| m.px.iter().map(|&v| f64::from(v)).sum::<f64>() / 32.;
        assert!((mean(&m) - mean(&ramp())).abs() < 1e-5);
        // Wrapping keeps the left and right columns as close as any two neighbours.
        let inner = (0..7)
            .map(|x| (m.px[x] - m.px[x + 1]).abs())
            .fold(0f32, f32::max);
        assert!((m.px[0] - m.px[7]).abs() <= inner + 1e-5);
    }

    #[test]
    fn fit_halves_by_averaging() {
        let m = HeightMap::from_gray8(4, 2, &[0, 255, 0, 255, 255, 0, 255, 0]).unwrap();
        let f = m.fit(2);
        assert_eq!((f.width(), f.height()), (2, 1));
        assert!(f.px.iter().all(|v| (v - 0.5).abs() < 1e-6));
    }
}
