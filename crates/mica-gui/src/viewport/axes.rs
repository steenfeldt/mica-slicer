// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Corner axis triad: shows the camera orientation; clicking an axis tip snaps to that view.

use crate::scene::camera::Camera;
use eframe::egui::{self, Align2, Color32, FontId, Rect, Sense, Stroke, Vec2};
use glam::Vec3;

const COLORS: [Color32; 3] = [
    Color32::from_rgb(232, 86, 78),
    Color32::from_rgb(96, 196, 96),
    Color32::from_rgb(78, 140, 240),
];
/// Screen offset of a world direction for the given camera, in pixels of `radius`.
pub fn screen_dir(camera: &Camera, axis: Vec3, radius: f32) -> (Vec2, f32) {
    let (right, up) = camera.basis();
    let depth = axis.dot(camera.direction());
    (Vec2::new(axis.dot(right), -axis.dot(up)) * radius, depth)
}
/// Draws the triad centred in `rect`; returns a camera preset index when a tip is clicked.
pub fn triad(ui: &mut egui::Ui, rect: Rect, camera: &Camera) -> Option<usize> {
    let center = rect.center();
    let radius = rect.width().min(rect.height()) * 0.36;
    let painter = ui.painter_at(rect);
    let dark = ui.visuals().dark_mode;
    painter.circle_filled(
        center,
        radius + 14.,
        if dark {
            Color32::from_rgba_unmultiplied(20, 22, 26, 110)
        } else {
            Color32::from_rgba_unmultiplied(255, 255, 255, 120)
        },
    );
    // Positive and negative tips: (axis, preset when clicked, label).
    let tips = [
        (Vec3::X, 6, "X"),
        (Vec3::Y, 4, "Y"),
        (Vec3::Z, 1, "Z"),
        (-Vec3::X, 5, ""),
        (-Vec3::Y, 3, ""),
        (-Vec3::Z, 2, ""),
    ];
    let mut items: Vec<(Vec2, f32, usize, &str, Color32, bool)> = tips
        .iter()
        .enumerate()
        .map(|(i, (axis, preset, label))| {
            let (d, depth) = screen_dir(camera, *axis, radius);
            (d, depth, *preset, *label, COLORS[i % 3], i < 3)
        })
        .collect();
    // Far tips first so near ones draw on top.
    items.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
    let mut clicked = None;
    for (d, _, preset, label, color, positive) in items {
        let tip = center + d;
        let r = if positive { 9. } else { 6. };
        let id = ui.scope_id().with(("axis tip", preset));
        let response = ui.interact(
            Rect::from_center_size(tip, Vec2::splat(r * 2.2)),
            id,
            Sense::click(),
        );
        let fill = if response.hovered() {
            color.gamma_multiply(1.3)
        } else if positive {
            color
        } else {
            color.gamma_multiply(0.45)
        };
        if positive {
            painter.line_segment([center, tip], Stroke::new(2., color));
        }
        painter.circle_filled(tip, r, fill);
        if positive {
            painter.text(
                tip,
                Align2::CENTER_CENTER,
                label,
                FontId::proportional(11.),
                Color32::from_gray(if dark { 20 } else { 255 }),
            );
        }
        if response.clicked() {
            clicked = Some(preset);
        }
        response.on_hover_text(format!("View {}", Camera::preset_name(preset)));
    }
    clicked
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn top_view_puts_x_right_and_y_up() {
        let mut c = Camera::default();
        c.preset(1);
        let (x, _) = screen_dir(&c, Vec3::X, 10.);
        let (y, _) = screen_dir(&c, Vec3::Y, 10.);
        let (z, depth) = screen_dir(&c, Vec3::Z, 10.);
        assert!(x.x > 9.9 && x.y.abs() < 1e-3);
        assert!(y.y < -9.9 && y.x.abs() < 1e-3);
        assert!(z.length() < 1e-3 && depth > 0.99);
    }
}
