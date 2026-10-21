// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Panels of the main window. Each file adds `impl App` methods for one area; shared small
//! widgets live here.

pub mod about;
pub mod add_printer;
pub mod brim_ears;
pub mod calibration;
pub mod colour;
pub mod dashboard;
pub mod dialogs;
pub mod editor;
pub mod export_all;
pub mod gcode;
pub mod icons;
pub mod import;
pub mod layer_edit;
pub mod library;
pub mod machine;
pub mod markdown;
pub mod mesh_tools;
pub mod objects;
pub mod paint;
pub mod plate_scope;
pub mod preferences;
pub mod preview_side;
pub mod print;
pub mod printers;
pub mod profiles;
pub mod scope;
pub mod settings_icons;
pub mod shapes;
pub mod shortcuts;
pub mod sidebar;
pub mod subcards;
pub mod sync;
pub mod topbar;
pub mod topics;
pub mod tower;
pub mod tree;
pub mod welcome;
pub mod whats_new;
pub mod widgets;

use eframe::egui;

/// A small square colour swatch.
pub fn swatch(ui: &mut egui::Ui, colour: [u8; 3], size: f32) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(
        rect,
        3.,
        egui::Color32::from_rgb(colour[0], colour[1], colour[2]),
    );
    painter.rect_stroke(
        rect,
        3.,
        egui::Stroke::new(1., ui.visuals().widgets.noninteractive.bg_stroke.color),
        egui::StrokeKind::Inside,
    );
    response
}

/// Human time: `1h 23m`, `12m 05s`.
pub fn duration(seconds: f64) -> String {
    let s = seconds.max(0.).round() as u64;
    let (h, m, sec) = (s / 3600, (s % 3600) / 60, s % 60);
    if h > 0 {
        format!("{h}h {m:02}m")
    } else if m > 0 {
        format!("{m}m {sec:02}s")
    } else {
        format!("{sec}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn helpers() {
        assert_eq!(duration(0.), "0s");
        assert_eq!(duration(65.), "1m 05s");
        assert_eq!(duration(5000.), "1h 23m");
    }
}
