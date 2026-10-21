// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/Support/SupportParameters.cpp :: SupportParameters::SupportParameters
// Source: prusaslicer@b028299c77 src/libslic3r/Slicing.cpp :: SlicingParameters::create_from_config
// Source: prusaslicer@b028299c77 src/libslic3r/Flow.cpp :: support_material_flow, support_material_interface_flow
// Changes: Explicit config subset; the base interface layer counts of mixed soluble/non-soluble tools come from the resolved tree3d config.
// Copyright (c) Prusa Research 2016–2023 Vojtěch Bubník, Enrico Turri, David Kocík,
// Lukáš Matěna, Oleksandra Iushchenko

use super::SupportError;
use crate::{flow::Flow, settings::PrintSettings};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SupportStyle {
    Grid,
    Snug,
    Tree,
    Organic,
}
impl SupportStyle {
    pub fn is_tree(self) -> bool {
        matches!(self, Self::Tree | Self::Organic)
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "grid" => Self::Grid,
            "snug" => Self::Snug,
            "tree" => Self::Tree,
            "organic" => Self::Organic,
            _ => return None,
        })
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SupportPattern {
    Rectilinear,
    RectilinearGrid,
    Honeycomb,
}
impl SupportPattern {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "rectilinear" => Self::Rectilinear,
            "rectilinear-grid" => Self::RectilinearGrid,
            "honeycomb" => Self::Honeycomb,
            _ => return None,
        })
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InterfacePattern {
    Auto,
    Rectilinear,
    Concentric,
}
impl InterfacePattern {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "auto" => Self::Auto,
            "rectilinear" => Self::Rectilinear,
            "concentric" => Self::Concentric,
            _ => return None,
        })
    }
}

/// Resolved lengths in mm, angles in degrees, density as a fraction. This is the entire config
/// needed by the generator; it does not depend on `PreparedObject` or object toolpath batches.
#[derive(Clone, Debug)]
pub struct SupportConfig {
    pub tree3d: Option<super::tree3d::Tree3dConfig>,
    pub tree: super::tree_common::TreeConfig,
    pub enabled: bool,
    pub automatic: bool,
    pub threshold: f64,
    pub style: SupportStyle,
    pub top_gap: f64,
    pub bottom_gap: f64,
    pub xy_gap: f64,
    pub spacing: f64,
    pub angle: f64,
    pub top_interface_layers: usize,
    pub bottom_interface_layers: i32,
    pub interface_spacing: f64,
    pub pattern: SupportPattern,
    pub interface_pattern: InterfacePattern,
    pub buildplate_only: bool,
    pub closing_radius: f64,
    pub with_sheath: bool,
    /// Orca: remove small automatic overhang clusters (normal supports).
    pub remove_small_overhang: bool,
    /// Orca support semantics (Orca configs): interface layer counts exclude the contact layer
    /// (`num_top/bottom_interface_layers` without Prusa's -1), and normal supports use Orca's
    /// sharp-tail/cantilever/small-cluster overhang rules.
    pub orca_semantics: bool,
    pub contact_loops: bool,
    pub synchronize_layers: bool,
    /// Dense base-tool layers under the top / above the bottom interface layers
    /// (`SupportParameters::num_top/bottom_base_interface_layers`).
    pub top_base_interface_layers: usize,
    pub bottom_base_interface_layers: usize,
    pub enforce_layers: usize,
    pub dont_support_bridges: bool,
    /// External bridge direction override, degrees; zero selects automatic.
    pub bridge_angle: f64,
    pub raft_layers: usize,
    pub raft_gap: f64,
    pub raft_expansion: f64,
    pub first_layer_density: f64,
    pub first_layer_expansion: f64,
    pub width: f64,
    pub first_layer_width: f64,
    pub external_width: f64,
    pub nozzle_diameter: f64,
    pub layer_height: f64,
    pub first_layer_height: f64,
    pub min_layer_height: f64,
    pub max_layer_height: f64,
}

impl From<&PrintSettings> for SupportConfig {
    fn from(s: &PrintSettings) -> Self {
        Self {
            tree3d: s.tree3d.clone(),
            tree: super::tree_common::TreeConfig {
                angle: s.support_tree_angle,
                angle_slow: s.support_tree_angle_slow,
                branch_diameter: s.support_tree_branch_diameter,
                branch_diameter_angle: s.support_tree_branch_diameter_angle,
                branch_diameter_double_wall: s.tree3d.as_ref().map_or(
                    s.support_tree_branch_diameter_double_wall,
                    super::tree3d::Tree3dConfig::double_wall_diameter,
                ),
                branch_distance: s.support_tree_branch_distance,
                tip_diameter: s.support_tree_tip_diameter,
                top_rate: s.support_tree_top_rate,
            },
            enabled: s.support_material,
            automatic: s.support_material_auto,
            threshold: s.support_material_threshold,
            style: s.support_material_style,
            top_gap: s.support_material_contact_distance,
            bottom_gap: s.support_material_bottom_contact_distance,
            xy_gap: s
                .support_material_xy_spacing
                .resolve(s.external_perimeter_extrusion_width, 1.0),
            spacing: s.support_material_spacing,
            angle: s.support_material_angle,
            top_interface_layers: s.support_material_interface_layers,
            bottom_interface_layers: s.support_material_bottom_interface_layers,
            interface_spacing: s.support_material_interface_spacing,
            pattern: s.support_material_pattern,
            interface_pattern: s.support_material_interface_pattern,
            buildplate_only: s.support_material_buildplate_only,
            closing_radius: s.support_material_closing_radius,
            with_sheath: s.support_material_with_sheath,
            remove_small_overhang: s.support_remove_small_overhang,
            orca_semantics: s.support_orca_semantics,
            contact_loops: s.support_material_interface_contact_loops,
            synchronize_layers: s.support_material_synchronize_layers,
            top_base_interface_layers: s.tree3d.as_ref().map_or(0, |t| t.top_base_layers),
            bottom_base_interface_layers: s.tree3d.as_ref().map_or(0, |t| t.bottom_base_layers),
            enforce_layers: s.support_material_enforce_layers,
            dont_support_bridges: s.dont_support_bridges,
            bridge_angle: s.bridge_angle,
            raft_layers: s.raft_layers,
            raft_gap: s.raft_contact_distance,
            raft_expansion: s.raft_expansion,
            first_layer_density: s.raft_first_layer_density,
            first_layer_expansion: s.raft_first_layer_expansion,
            width: s.support_material_extrusion_width,
            first_layer_width: s.support_material_first_layer_extrusion_width,
            external_width: s.external_perimeter_extrusion_width,
            nozzle_diameter: s.nozzle_diameter,
            layer_height: s.layer_height,
            first_layer_height: s.first_layer_height,
            min_layer_height: s.min_layer_height,
            max_layer_height: s.max_layer_height,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SupportParameters {
    pub config: SupportConfig,
    pub flow: Flow,
    pub first_layer_flow: Flow,
    pub support_density: f64,
    pub interface_density: f64,
    pub bottom_gap: f64,
    pub bottom_interface_layers: usize,
    pub max_height: f64,
    pub min_height: f64,
    pub raft_base_layers: usize,
    pub raft_height: f64,
    pub raft_top_z: f64,
    pub object_z_offset: f64,
}

impl SupportParameters {
    pub fn new(c: &SupportConfig) -> Result<Self, SupportError> {
        for (name, value) in [
            ("layer height", c.layer_height),
            ("first layer height", c.first_layer_height),
            ("support width", c.width),
            ("first layer width", c.first_layer_width),
            ("nozzle diameter", c.nozzle_diameter),
            ("external width", c.external_width),
        ] {
            if !value.is_finite() || value <= 0.0 {
                return Err(SupportError(format!("{name} must be finite and positive")));
            }
        }
        for value in [
            c.top_gap,
            c.bottom_gap,
            c.xy_gap,
            c.spacing,
            c.interface_spacing,
            c.closing_radius,
            c.raft_gap,
            c.raft_expansion,
            c.first_layer_expansion,
            c.min_layer_height,
            c.max_layer_height,
        ] {
            if !value.is_finite() || value < 0.0 {
                return Err(SupportError(
                    "support gaps, spacings and height limits must be finite and nonnegative"
                        .into(),
                ));
            }
        }
        if !c.angle.is_finite()
            || !(0.0..=90.0).contains(&c.threshold)
            || !(0.0..=1.0).contains(&c.first_layer_density)
        {
            return Err(SupportError(
                "invalid support angle, threshold or first layer density".into(),
            ));
        }
        let normal = c.enabled && c.automatic || c.enforce_layers > 0;
        if c.style.is_tree() {
            c.tree.validate()?;
        }
        if (normal || c.raft_layers > 0) && c.pattern == SupportPattern::Honeycomb {
            return Err(SupportError(
                "honeycomb supports are not implemented; select rectilinear or rectilinear-grid"
                    .into(),
            ));
        }
        if (normal || c.raft_layers > 0) && c.contact_loops {
            return Err(SupportError(
                "support interface contact loops are not implemented".into(),
            ));
        }
        let rounding = 1.0 - std::f64::consts::PI / 4.0;
        if c.width <= c.layer_height.max(0.75 * c.nozzle_diameter) * rounding
            || c.first_layer_width <= c.first_layer_height * rounding
        {
            return Err(SupportError(
                "support widths are too small for the layer heights".into(),
            ));
        }
        let max_height = if c.max_layer_height == 0.0 {
            0.75 * c.nozzle_diameter
        } else {
            c.max_layer_height
        }
        .min(c.nozzle_diameter);
        let min_height = if c.min_layer_height == 0.0 {
            0.07
        } else {
            c.min_layer_height
        }
        .max(0.01)
        .min(c.layer_height);
        if max_height < min_height || c.width <= max_height * (1.0 - std::f64::consts::PI / 4.0) {
            return Err(SupportError(
                "support layer height limits are incompatible with the nozzle or width".into(),
            ));
        }
        let flow = Flow::new(c.width, c.layer_height);
        let first_layer_flow = Flow::new(c.first_layer_width, c.first_layer_height);
        let raft_height = c.layer_height.max(0.75 * c.nozzle_diameter);
        let raft_top_z = if c.raft_layers == 0 {
            0.0
        } else {
            c.first_layer_height + (c.raft_layers - 1) as f64 * raft_height
        };
        Ok(Self {
            config: c.clone(),
            flow,
            first_layer_flow,
            support_density: flow.spacing() / (c.spacing + flow.spacing()),
            interface_density: flow.spacing() / (c.interface_spacing + flow.spacing()),
            bottom_gap: if c.top_gap == 0.0 {
                0.0
            } else if c.bottom_gap == 0.0 {
                c.top_gap
            } else {
                c.bottom_gap
            },
            bottom_interface_layers: if c.bottom_interface_layers < 0 {
                c.top_interface_layers
            } else {
                c.bottom_interface_layers as usize
            },
            max_height,
            min_height,
            raft_base_layers: c.raft_layers / 2,
            raft_height,
            raft_top_z,
            object_z_offset: raft_top_z
                + if c.raft_layers > 0 && c.top_gap > 0.0 {
                    c.raft_gap
                } else {
                    0.0
                },
        })
    }
}
