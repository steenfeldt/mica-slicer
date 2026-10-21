// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The plate menu: every action on one plate in one list, offered the same way wherever a
//! plate shows (the "⋯" button and right-click on the plate bar, right-click on a plate's bed
//! or its label in the 3D view, right-click on the object list's plate header, the rows of
//! the plate list). The viewport and the object list hand the pick to
//! [`crate::app::App::plate_command`].

use crate::keymap::{Act, shortcut_text};
use eframe::egui;

/// What the plate menu does to its plate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlateCommand {
    /// Edit the plate's name in the plate bar.
    Rename,
    /// The sidebar's "This plate" settings (per-plate bed type, print sequence, overrides).
    Settings,
    /// Arrange the objects on this plate.
    ArrangeObjects,
    /// Auto-orient every object on this plate.
    Orient,
    /// Select every object on this plate.
    SelectAll,
    /// Lock or unlock the plate's layout.
    Lock,
    /// Copy the plate, with copies of its objects, right after it.
    Duplicate,
    /// Move the plate one place earlier (left in the plate bar) or later.
    MoveLeft,
    MoveRight,
    /// Open the plate list, where plates are reordered by dragging.
    Reorder,
    /// Delete the plate and its objects.
    Delete,
}

/// What the menu needs to know about its plate.
#[derive(Clone, Debug, PartialEq)]
pub struct PlateMenuInfo {
    pub index: usize,
    pub count: usize,
    /// The plate's name (`Plate N` when it has none of its own).
    pub name: String,
    pub locked: bool,
    pub objects: usize,
    /// Edits are possible now (not while loading or showing a slice).
    pub editable: bool,
}

/// Every label the menu can show, in order (UI tests look them up).
pub const LABELS: [&str; 11] = [
    "Rename plate…",
    "Plate settings…",
    "Arrange objects on this plate",
    "Auto-orient objects on this plate",
    "Select all objects on this plate",
    "Lock plate",
    "Duplicate plate",
    "Move plate left",
    "Move plate right",
    "Reorder plates…",
    "Delete plate",
];

/// The menu's rows; returns the pick (the menu closes on it).
pub fn plate_menu(ui: &mut egui::Ui, info: &PlateMenuInfo) -> Option<PlateCommand> {
    use PlateCommand as C;
    ui.set_min_width(250.);
    let edit = info.editable;
    let free = edit && !info.locked;
    let mut out = None;
    ui.label(
        egui::RichText::new(if info.name == crate::plates::default_name(info.index) {
            info.name.clone()
        } else {
            format!("Plate {} · {}", info.index + 1, info.name)
        })
        .small()
        .weak(),
    );
    let mut row = |ui: &mut egui::Ui, label: &str, key: &str, enabled: bool, c: C| {
        let r = ui.add_enabled(enabled, egui::Button::new(label).shortcut_text(key));
        record(ui, label, r.rect);
        if r.clicked() {
            out = Some(c);
            ui.close();
        }
    };
    row(ui, LABELS[0], "F2", edit, C::Rename);
    row(ui, LABELS[1], "", true, C::Settings);
    ui.separator();
    let has = info.objects > 0;
    row(
        ui,
        LABELS[2],
        &shortcut_text(Act::ArrangePlate),
        free && has,
        C::ArrangeObjects,
    );
    row(
        ui,
        LABELS[3],
        &shortcut_text(Act::OrientPlate),
        free && has,
        C::Orient,
    );
    row(ui, LABELS[4], "", has, C::SelectAll);
    ui.separator();
    row(
        ui,
        if info.locked {
            "Unlock plate"
        } else {
            LABELS[5]
        },
        "",
        edit,
        C::Lock,
    );
    row(ui, LABELS[6], "", edit, C::Duplicate);
    row(ui, LABELS[7], "", edit && info.index > 0, C::MoveLeft);
    row(
        ui,
        LABELS[8],
        "",
        edit && info.index + 1 < info.count,
        C::MoveRight,
    );
    row(ui, LABELS[9], "", info.count > 1, C::Reorder);
    ui.separator();
    row(
        ui,
        if info.count > 1 {
            LABELS[10]
        } else {
            "Clear plate"
        },
        "",
        edit,
        C::Delete,
    );
    out
}

/// Where each row of the plate menu was drawn this frame (for UI tests).
#[derive(Clone, Default)]
pub struct Probe(pub Vec<(String, egui::Rect)>);

fn record(ui: &egui::Ui, label: &str, rect: egui::Rect) {
    let id = egui::Id::unique("plate menu probe");
    ui.ctx().data_mut(|d| {
        let probe = d.get_temp_mut_or_default::<Probe>(id);
        probe.0.retain(|(l, _)| l != label);
        probe.0.push((label.to_string(), rect));
    });
}

/// The rect of the menu row labelled `label`, as last drawn.
#[cfg(test)]
pub fn probe(ctx: &egui::Context, label: &str) -> Option<egui::Rect> {
    ctx.data(|d| {
        d.get_temp::<Probe>(egui::Id::unique("plate menu probe"))
            .and_then(|p| p.0.iter().find(|(l, _)| l == label).map(|(_, r)| *r))
    })
}

/// Clears the probe (so a test sees only what the next frame draws).
#[cfg(test)]
pub fn clear_probe(ctx: &egui::Context) {
    ctx.data_mut(|d| d.remove::<Probe>(egui::Id::unique("plate menu probe")));
}
