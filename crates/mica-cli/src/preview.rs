// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Renders a layer's toolpaths to a PNG, coloured by role, for quick visual checks.
//! Uses the shared compressed PNG encoder in mica-gcode and a line rasteriser.

use mica_core::geometry::{BoundingBox, unscale};
use mica_core::toolpath::{LayerToolpaths, Role};

fn color(role: Role) -> [u8; 3] {
    // Same hues as PrusaSlicer's preview.
    match role {
        Role::Skirt | Role::Brim => [70, 190, 90],
        Role::ExternalPerimeter => [255, 125, 56],
        Role::Perimeter => [255, 230, 77],
        Role::OverhangPerimeter => [60, 100, 255],
        Role::BridgeInfill => [80, 150, 255],
        Role::InternalBridgeInfill => [40, 210, 235],
        Role::InternalInfill => [176, 48, 41],
        Role::SolidInfill => [150, 84, 204],
        Role::TopSolidInfill => [240, 64, 64],
        Role::Ironing => [60, 210, 220],
        // PrusaSlicer's gap fill colour.
        Role::GapFill => [255, 255, 255],
        Role::SupportMaterial => [75, 180, 105],
        Role::SupportMaterialInterface => [75, 210, 220],
    }
}

/// Draws `layer` into an RGB image `px_per_mm` pixels per millimetre, framed on `frame` (scaled coords).
pub fn render(layer: &LayerToolpaths, frame: BoundingBox, px_per_mm: f64) -> (u32, u32, Vec<u8>) {
    let margin = 2.0;
    let w = ((unscale(frame.max.x - frame.min.x) + 2.0 * margin) * px_per_mm).ceil() as u32;
    let h = ((unscale(frame.max.y - frame.min.y) + 2.0 * margin) * px_per_mm).ceil() as u32;
    let mut img = vec![24u8; (w * h * 3) as usize];
    let to_px = |p: mica_core::geometry::Point| {
        (
            (unscale(p.x - frame.min.x) + margin) * px_per_mm,
            // Image rows grow downwards; flip Y so the rear of the bed is at the top.
            h as f64 - (unscale(p.y - frame.min.y) + margin) * px_per_mm,
        )
    };
    for path in &layer.paths {
        let c = color(path.role);
        let radius = path.flow.width / 2.0 * px_per_mm;
        for seg in path.points.windows(2) {
            let (a, b) = (to_px(seg[0]), to_px(seg[1]));
            draw_thick_line(&mut img, w, h, a, b, radius, c);
        }
    }
    // Cyan markers show the starts of opened wall loops, including scarf starts.
    for path in layer.paths.iter().filter(|p| p.seam.is_some()) {
        if let Some(&start) = path.points.first() {
            let point = to_px(start);
            draw_thick_line(&mut img, w, h, point, point, 2.5, [70, 235, 245]);
        }
    }
    (w, h, img)
}

fn draw_thick_line(
    img: &mut [u8],
    w: u32,
    h: u32,
    a: (f64, f64),
    b: (f64, f64),
    r: f64,
    c: [u8; 3],
) {
    let (minx, maxx) = (a.0.min(b.0) - r, a.0.max(b.0) + r);
    let (miny, maxy) = (a.1.min(b.1) - r, a.1.max(b.1) + r);
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len2 = dx * dx + dy * dy;
    for py in (miny.floor().max(0.0) as u32)..=(maxy.ceil().min(h as f64 - 1.0) as u32) {
        for px in (minx.floor().max(0.0) as u32)..=(maxx.ceil().min(w as f64 - 1.0) as u32) {
            let (x, y) = (px as f64 + 0.5, py as f64 + 0.5);
            let t = if len2 > 0.0 {
                (((x - a.0) * dx + (y - a.1) * dy) / len2).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let (qx, qy) = (a.0 + t * dx, a.1 + t * dy);
            if (x - qx).powi(2) + (y - qy).powi(2) <= r * r {
                let i = ((py * w + px) * 3) as usize;
                img[i..i + 3].copy_from_slice(&c);
            }
        }
    }
}

/// Encodes an RGB image with the shared compressed PNG writer.
pub fn encode_png(w: u32, h: u32, rgb: &[u8]) -> Vec<u8> {
    mica_gcode::thumbnails::encode_png_rgb(w, h, rgb).expect("valid rendered RGB image")
}
