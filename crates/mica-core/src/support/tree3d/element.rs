// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: prusaslicer@b028299c77 src/libslic3r/Support/TreeSupport.hpp :: AreaIncreaseSettings, SupportElementState, SupportElement, getEffectiveDTT, support_element_radius, support_element_collision_radius
// Source: prusaslicer@b028299c77 src/libslic3r/Support/TreeSupport.cpp :: LineStatus, LineInformation
// Source: orca@8500fcdcca src/libslic3r/Support/TreeSupport3D.hpp :: SupportElementState, set_pending_roof_recovery, propagate_down
// Source: orca@8500fcdcca src/libslic3r/Support/TreeSupportCommon.hpp :: LineStatus
// Changes: Owned polygons and parent indices; Option replaces invalid position sentinel; Orca metadata and recovery behind Rules.
// Copyright (c) Prusa Research 2022 - 2023 Vojtěch Bubník @bubnikv,
// Lukáš Matěna @lukasmatena, Tomáš Mészáros @tamasmeszaros
// PrusaSlicer is released under the terms of the AGPLv3 or higher.
// Tree supports by Thomas Rahm, losely based on Tree Supports by CuraEngine.
// Original source of Thomas Rahm's tree supports: https://github.com/ThomasRahm/CuraEngine
// Copyright (c) 2021 Ultimaker B.V.
// CuraEngine is released under the terms of the AGPLv3 or higher.

use super::{AvoidanceType, TreeSupportSettings};
use crate::{
    geometry::{ExPolygon, Point},
    rules::Rules,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AreaIncreaseSettings {
    pub increase_speed: i64,
    pub avoidance_type: AvoidanceType,
    pub increase_radius: bool,
    pub no_error: bool,
    pub use_min_distance: bool,
    pub move_area: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SupportElementState {
    pub to_buildplate: bool,
    pub to_model_gracious: bool,
    pub use_min_xy_dist: bool,
    pub supports_roof: bool,
    pub can_use_safe_radius: bool,
    pub skip_ovalisation: bool,
    pub lost: bool,
    pub verylost: bool,
    pub deleted: bool,
    pub marked: bool,
    pub target_height: i32,
    pub target_position: Point,
    pub next_position: Point,
    pub layer_idx: i32,
    pub effective_radius_height: u32,
    pub distance_to_top: u32,
    pub result_on_layer: Option<Point>,
    pub increased_to_model_radius: i64,
    pub elephant_foot_increases: f64,
    pub dont_move_until: u32,
    pub last_area_increase: AreaIncreaseSettings,
    pub missing_roof_layers: u32,
    /// Orca-only additions in TreeSupport3D.hpp. Inert under Prusa rules.
    pub orca: OrcaElementMetadata,
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OrcaElementMetadata {
    pub element_type: i32,
    /// Millimetres (upstream coordf_t), unlike scaled get_radius output.
    pub radius: f64,
    pub print_z: f32,
    pub roof_recovery_dtt: u32,
}
impl SupportElementState {
    pub fn propagate_down(&self, rules: Rules) -> Self {
        let mut dst = self.clone();
        dst.distance_to_top += 1;
        dst.layer_idx -= 1;
        // Orca decrements the pending roof/contact recovery count and advances
        // recovery depth; Prusa simply copies missing_roof_layers.
        if rules.is_orca() && dst.has_pending_roof_recovery() {
            dst.missing_roof_layers -= 1;
            dst.orca.roof_recovery_dtt += 1;
        }
        dst.result_on_layer = None;
        dst.skip_ovalisation = false;
        dst
    }
    pub fn locked(&self) -> bool {
        self.distance_to_top < self.dont_move_until
    }
    pub fn has_pending_roof_recovery(&self) -> bool {
        self.missing_roof_layers > 0
    }
    pub fn set_pending_roof_recovery(&mut self, pending: u32, depth: u32, rules: Rules) {
        self.missing_roof_layers = pending;
        if rules.is_orca() {
            self.orca.roof_recovery_dtt = if pending > 0 { depth } else { 0 };
        }
    }
    pub fn result_on_layer_is_set(&self) -> bool {
        self.result_on_layer.is_some()
    }
    pub fn result_on_layer_reset(&mut self) {
        self.result_on_layer = None;
    }
}

pub fn get_effective_dtt(settings: &TreeSupportSettings, elem: &SupportElementState) -> usize {
    if (elem.effective_radius_height as usize) < settings.increase_radius_until_layer {
        (elem.distance_to_top as usize).min(settings.increase_radius_until_layer)
    } else {
        elem.effective_radius_height as usize
    }
}
pub fn support_element_radius(settings: &TreeSupportSettings, elem: &SupportElementState) -> i64 {
    settings.get_radius(
        get_effective_dtt(settings, elem),
        elem.elephant_foot_increases,
    )
}
pub fn support_element_collision_radius(
    settings: &TreeSupportSettings,
    elem: &SupportElementState,
) -> i64 {
    settings.get_radius(
        elem.effective_radius_height as usize,
        elem.elephant_foot_increases,
    )
}
impl TreeSupportSettings {
    /// State overload of getRadius (getRadiusHeight in older Cura variants).
    pub fn get_radius_state(&self, elem: &SupportElementState) -> i64 {
        support_element_radius(self, elem)
    }
    pub fn get_collision_radius(&self, elem: &SupportElementState) -> i64 {
        support_element_collision_radius(self, elem)
    }
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SupportElement {
    pub state: SupportElementState,
    /// Stable indices into the layer immediately above, as in upstream.
    pub parents: Vec<i32>,
    pub influence_area: Vec<ExPolygon>,
}
impl SupportElement {
    pub fn radius(&self, settings: &TreeSupportSettings) -> i64 {
        support_element_radius(settings, &self.state)
    }
    pub fn collision_radius(&self, settings: &TreeSupportSettings) -> i64 {
        support_element_collision_radius(settings, &self.state)
    }
}
pub type SupportElements = std::collections::VecDeque<SupportElement>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineStatus {
    Invalid,
    ToModel,
    ToModelGracious,
    ToModelGraciousSafe,
    ToBp,
    ToBpSafe,
}
pub type LineInformation = Vec<(Point, LineStatus)>;
pub type LineInformations = Vec<LineInformation>;
pub const SUPPORT_TREE_CIRCLE_RESOLUTION: usize = 25;
pub const POLYGONS_STRICTLY_SIMPLE: bool = false;
pub fn tiny_area_threshold() -> f64 {
    (0.001 * crate::geometry::SCALE).powi(2)
}
