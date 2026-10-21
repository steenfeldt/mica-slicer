// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/slic3r/GUI/GLCanvas3D.cpp :: GLCanvas3D::on_mouse, GLCanvas3D::Mouse::Drag::MoveThresholdPx
// Source: prusaslicer@b028299c77 src/slic3r/GUI/GLCanvas3D.hpp :: GLCanvas3D::Mouse::is_move_threshold_met
// Source: orca@8500fcdcca src/slic3r/GUI/GLCanvas3D.cpp :: GLCanvas3D::on_mouse, GLCanvas3D::Mouse::Drag::MoveThresholdPx
// Changes: The selection rules and the 5 px start threshold of a press on a model, as pure functions the viewport's egui drag handling calls; the threshold applies to every press (upstream skips it for a model that was selected already), so a click never nudges; Shift-press keeps Mica's own meaning (toggle on click, no move); the selection change and the move are editor actions (one undo step per drag).

//! Pressing on a model and dragging moves it, as in PrusaSlicer and Orca: the press selects
//! the model (alone, unless it is part of the selection already, which then moves as a
//! whole), and the press becomes a move once the pointer has left it by more than
//! [`MOVE_THRESHOLD_PX`] along either axis. A plain click therefore only selects.

use eframe::egui::Pos2;

/// How far (screen px, along either axis) the pointer must travel from the press before a
/// press on a model becomes a move (`GLCanvas3D::Mouse::Drag::MoveThresholdPx`).
pub const MOVE_THRESHOLD_PX: f32 = 5.;

/// Whether the pointer at `p` has left `press` far enough to start a move
/// (`GLCanvas3D::Mouse::is_move_threshold_met`: either axis beyond the threshold).
pub fn move_threshold_met(press: Pos2, p: Pos2) -> bool {
    (p.x - press.x).abs() > MOVE_THRESHOLD_PX || (p.y - press.y).abs() > MOVE_THRESHOLD_PX
}

/// The model to select when a press on model `pressed` starts a move: `None` when it is
/// part of `selection` already (the whole selection moves), else the model alone
/// (`Selection::add(volume_idx, as_single_selection = !shift)` on `LeftDown`).
pub fn press_selects(selection: &[usize], pressed: usize) -> Option<usize> {
    (!selection.contains(&pressed)).then_some(pressed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn threshold_is_five_pixels_along_either_axis() {
        let press = Pos2::new(100., 100.);
        assert!(!move_threshold_met(press, press));
        assert!(!move_threshold_met(press, Pos2::new(105., 95.)));
        assert!(move_threshold_met(press, Pos2::new(105.5, 100.)));
        assert!(move_threshold_met(press, Pos2::new(100., 94.)));
    }

    #[test]
    fn a_press_selects_alone_unless_already_selected() {
        assert_eq!(press_selects(&[], 2), Some(2));
        assert_eq!(press_selects(&[0, 1], 2), Some(2));
        assert_eq!(press_selects(&[0, 2], 2), None);
    }
}
