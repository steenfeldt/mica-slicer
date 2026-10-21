// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: prusaslicer@b028299c77 src/libslic3r/Support/TreeSupportCommon.hpp :: TreeSupportMeshGroupSettings, TreeSupportSettings
// Source: prusaslicer@b028299c77 src/libslic3r/Support/TreeSupportCommon.cpp :: TreeSupportMeshGroupSettings, TreeSupportSettings
// Source: orca@8500fcdcca src/libslic3r/Support/TreeSupportCommon.hpp :: TreeSupportMeshGroupSettings, TreeSupportSettings, layer_idx_ceil, layer_idx_floor
// Source: orca@8500fcdcca src/libslic3r/Support/TreeSupport3D.cpp :: group_meshes
// Source: prusaslicer@b028299c77 src/libslic3r/Flow.cpp :: support_material_flow, support_material_interface_flow
// Source: orca@8500fcdcca src/libslic3r/PrintConfig.hpp :: SupportMaterialPattern, SupportMaterialInterfacePattern
// Source: orca@8500fcdcca src/libslic3r/Flow.cpp :: support_material_flow, support_material_interface_flow
// Source: prusaslicer@b028299c77 src/libslic3r/Slicing.cpp :: SlicingParameters::create_from_config
// Source: orca@8500fcdcca src/libslic3r/Slicing.cpp :: SlicingParameters::create_from_config
// Source: orca@8500fcdcca src/libslic3r/Support/SupportParameters.hpp :: number_of_support_interface_bottom_layers
// Changes: Integer-scaled Rust settings; explicit per-pass gap state and slicing grid; Orca differences selected by Rules. All upstream defaults retained.
// Copyright (c) Prusa Research 2023 Vojtěch Bubník @bubnikv
// Copyright (c) Prusa Research 2016 - 2023 Pavel Mikuš @Godrak, Oleksandra Iushchenko @YuSanka,
// Vojtěch Bubník @bubnikv, Tomáš Mészáros @tamasmeszaros, Enrico Turri @enricoturri1966,
// David Kocík @kocikdav, Lukáš Matěna @lukasmatena
// Copyright (c) Slic3r 2014 - 2015 Alessandro Ranellucci @alranel
// Copyright (c) 2014 Petr Ledvina @ledvinap
// Copyright (c) Prusa Research 2022 Vojtěch Bubník @bubnikv
// Copyright (c) Slic3r 2012 - 2014 Alessandro Ranellucci @alranel
// PrusaSlicer is released under the terms of the AGPLv3 or higher.
// Tree supports by Thomas Rahm, losely based on Tree Supports by CuraEngine.
// Original source of Thomas Rahm's tree supports: https://github.com/ThomasRahm/CuraEngine
// Copyright (c) 2021 Ultimaker B.V.
// CuraEngine is released under the terms of the AGPLv3 or higher.

use super::super::{InterfacePattern, SupportPattern};
use crate::{config::Config, geometry::SCALE, rules::Rules, settings::PrintSettings};
use std::f64::consts::PI;

/// Upstream scaled<coord_t> truncates toward zero; do not round these values.
pub fn scaled(mm: f64) -> i64 {
    (mm / 0.000_001) as i64
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InterfacePreference {
    InterfaceAreaOverwritesSupport,
    SupportAreaOverwritesInterface,
    InterfaceLinesOverwriteSupport,
    SupportLinesOverwriteInterface,
    Nothing,
}

/// Complete upstream pattern vocabulary. Extra Orca values are retained here;
/// drawing them belongs to step 3, without expanding the old support generator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TreeSupportPattern {
    Default,
    Rectilinear,
    RectilinearGrid,
    Honeycomb,
    Lightning,
    Hollow,
}
impl TreeSupportPattern {
    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "default" => Self::Default,
            "rectilinear" => Self::Rectilinear,
            "rectilinear-grid" => Self::RectilinearGrid,
            "honeycomb" => Self::Honeycomb,
            "lightning" => Self::Lightning,
            "hollow" => Self::Hollow,
            _ => return None,
        })
    }
}
impl From<SupportPattern> for TreeSupportPattern {
    fn from(value: SupportPattern) -> Self {
        match value {
            SupportPattern::Rectilinear => Self::Rectilinear,
            SupportPattern::RectilinearGrid => Self::RectilinearGrid,
            SupportPattern::Honeycomb => Self::Honeycomb,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TreeInterfacePattern {
    Auto,
    Rectilinear,
    Concentric,
    RectilinearInterlaced,
    Grid,
}
impl TreeInterfacePattern {
    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "auto" => Self::Auto,
            "rectilinear" => Self::Rectilinear,
            "concentric" => Self::Concentric,
            "rectilinear_interlaced" => Self::RectilinearInterlaced,
            "grid" => Self::Grid,
            _ => return None,
        })
    }
}
impl From<InterfacePattern> for TreeInterfacePattern {
    fn from(value: InterfacePattern) -> Self {
        match value {
            InterfacePattern::Auto => Self::Auto,
            InterfacePattern::Rectilinear => Self::Rectilinear,
            InterfacePattern::Concentric => Self::Concentric,
        }
    }
}

/// All distances use Mica's scaled coordinates; angles are radians, rates percent.
#[derive(Clone, Debug, PartialEq)]
pub struct TreeSupportMeshGroupSettings {
    pub layer_height: i64,
    pub resolution: i64,
    pub min_feature_size: i64,
    pub support_angle: f64,
    pub support_line_width: i64,
    pub support_roof_line_width: i64,
    pub support_bottom_enable: bool,
    pub support_bottom_height: i64,
    pub support_material_buildplate_only: bool,
    pub support_xy_distance: i64,
    pub support_xy_distance_overhang: i64,
    pub support_top_distance: i64,
    pub support_bottom_distance: i64,
    pub support_interface_skip_height: i64,
    pub support_roof_enable: bool,
    pub support_roof_layers: i64,
    pub support_floor_enable: bool,
    pub support_floor_layers: i64,
    pub minimum_roof_area: f64,
    pub support_roof_angles: Vec<f64>,
    pub support_roof_pattern: TreeInterfacePattern,
    pub support_pattern: TreeSupportPattern,
    pub support_line_spacing: i64,
    pub support_bottom_offset: i64,
    pub support_wall_count: i32,
    pub support_roof_line_distance: i64,
    pub minimum_support_area: i64,
    pub minimum_bottom_area: i64,
    pub support_offset: i64,
    pub support_tree_angle: f64,
    pub support_tree_branch_diameter_angle: f64,
    pub support_tree_branch_distance: i64,
    pub support_tree_branch_diameter: i64,
    pub support_tree_angle_slow: f64,
    pub support_tree_max_diameter_increase_by_merges_when_support_to_model: i64,
    pub support_tree_min_height_to_model: i64,
    pub support_tree_bp_diameter: i64,
    pub support_tree_top_rate: f64,
    pub support_tree_tip_diameter: i64,
    pub support_xy_distance_1st_layer: i64,
    pub rules: Rules,
    /// SupportParameters drawing threshold, in scaled area units (extension to
    /// upstream MeshGroupSettings so step 3 needn't reinterpret the option).
    pub tree_branch_diameter_double_wall_area_scaled: f64,
}

impl Default for TreeSupportMeshGroupSettings {
    fn default() -> Self {
        Self {
            layer_height: scaled(0.15),
            resolution: scaled(0.025),
            min_feature_size: scaled(0.1),
            support_angle: 50. * PI / 180.,
            support_line_width: scaled(0.4),
            support_roof_line_width: scaled(0.4),
            support_bottom_enable: false,
            support_bottom_height: scaled(1.),
            support_material_buildplate_only: false,
            support_xy_distance: scaled(0.7),
            support_xy_distance_overhang: scaled(0.2),
            support_top_distance: scaled(0.1),
            support_bottom_distance: scaled(0.1),
            support_interface_skip_height: scaled(0.3),
            support_roof_enable: false,
            support_roof_layers: 2,
            support_floor_enable: false,
            support_floor_layers: 2,
            minimum_roof_area: SCALE * SCALE,
            support_roof_angles: Vec::new(),
            support_roof_pattern: TreeInterfacePattern::Auto,
            support_pattern: TreeSupportPattern::Rectilinear,
            support_line_spacing: scaled(2.66 - 0.4),
            support_bottom_offset: scaled(0.),
            support_wall_count: 1,
            support_roof_line_distance: scaled(0.4),
            minimum_support_area: scaled(0.),
            minimum_bottom_area: scaled(1.0),
            support_offset: scaled(0.),
            support_tree_angle: 60. * PI / 180.,
            support_tree_branch_diameter_angle: 5. * PI / 180.,
            support_tree_branch_distance: scaled(1.),
            support_tree_branch_diameter: scaled(2.),
            support_tree_angle_slow: 50. * PI / 180.,
            support_tree_max_diameter_increase_by_merges_when_support_to_model: scaled(1.0),
            support_tree_min_height_to_model: scaled(1.0),
            support_tree_bp_diameter: scaled(7.5),
            support_tree_top_rate: 15.,
            support_tree_tip_diameter: scaled(0.4),
            support_xy_distance_1st_layer: scaled(0.7),
            rules: Rules::PrusaSlicer,
            tree_branch_diameter_double_wall_area_scaled: f64::MAX,
        }
    }
}

impl TreeSupportMeshGroupSettings {
    /// Reuse Mica's resolved support flow and canonical option mapping. Orca import also
    /// retains raw option names as extras: prefer those for the organic-specific settings,
    /// even when the old generator mapped classic-tree values to the canonical keys.
    pub fn from_config(c: &Config) -> Result<Self, crate::resolve::ResolveError> {
        let s = PrintSettings::from_config(c)?;
        Self::from_resolved(c, &s)
    }
    pub fn from_resolved(
        c: &Config,
        s: &PrintSettings,
    ) -> Result<Self, crate::resolve::ResolveError> {
        let rules = Rules::of(c);
        let orca = rules.is_orca();
        let number = |prusa: &str, orca_key: &str, fallback: f64| {
            (if orca {
                c.raw(orca_key)
                    .and_then(|v| v.trim_end_matches('%').parse().ok())
            } else {
                None
            })
            .or_else(|| c.float(prusa))
            .unwrap_or(fallback)
        };
        let flag = |prusa: &str, orca_key: &str, fallback: bool| {
            number(prusa, orca_key, if fallback { 1.0 } else { 0.0 }) != 0.0
        };
        let h = scaled(s.layer_height);
        let top = number(
            "support_material_interface_layers",
            "support_interface_top_layers",
            3.0,
        )
        .max(0.0) as i64;
        let bottom_raw = number(
            "support_material_bottom_interface_layers",
            "support_interface_bottom_layers",
            -1.0,
        ) as i64;
        // Orca resolves -1 before both floor enable and floor layer assignment. Prusa
        // resolves it only for bottom_height, and leaves floor_enable false for -1.
        let bottom = if bottom_raw < 0 { top } else { bottom_raw };
        let external_width = s.external_perimeter_extrusion_width as f32 as f64;
        let canonical_xy = s.support_material_xy_spacing.resolve(external_width, 1.0);
        let xy = if orca {
            c.raw("support_object_xy_distance")
                .and_then(|value| value.parse().ok())
                .unwrap_or(canonical_xy)
        } else {
            canonical_xy
        };
        let angle = number(
            "support_tree_angle",
            "tree_support_branch_angle_organic",
            40.0,
        )
        .to_radians()
        .clamp(0.0, PI / 2.0 - 1e-4);
        let slow = number("support_tree_angle_slow", "tree_support_angle_slow", 25.0)
            .to_radians()
            .max(0.0)
            .min((angle - 1e-4).max(0.0));
        let diameter = scaled(number(
            "support_tree_branch_diameter",
            "tree_support_branch_diameter_organic",
            2.0,
        ));
        let bottom_enable = top > 0 && if orca { bottom > 0 } else { bottom_raw != 0 };
        let mut top_gap = number(
            "support_material_contact_distance",
            "support_top_z_distance",
            0.2,
        );
        let bottom_gap = number(
            "support_material_bottom_contact_distance",
            "support_bottom_z_distance",
            0.0,
        );
        // SlicingParameters: zero top gap is soluble, otherwise a zero bottom option
        // inherits the top distance. Orca's gap_object_support is independent (see below).
        let mut bottom_gap = if orca {
            bottom_gap
        } else if top_gap == 0.0 {
            0.0
        } else if bottom_gap == 0.0 {
            top_gap
        } else {
            bottom_gap
        };
        if orca {
            // SlicingParameters::create_from_config: top zero gap forces bottom
            // zero only when a bottom interface exists. Non-independent gaps are
            // snapped to object layers with an EPSILON bias.
            if bottom > 0 && (bottom_gap == 0.0 || top_gap == 0.0) {
                bottom_gap = 0.0;
            }
            if !flag("", "independent_support_layer_height", true) {
                top_gap = (top_gap / s.layer_height + 1e-4).round() * s.layer_height;
                bottom_gap = (bottom_gap / s.layer_height + 1e-4).round() * s.layer_height;
            }
        }
        // Flow stores widths as float; scaled_width truncates that float promoted
        // to double. Preserve this (0.42 mm becomes 419999 scaled units).
        let flow_width = |interface: bool| {
            let tool = number(
                if interface {
                    "support_material_interface_extruder"
                } else {
                    "support_material_extruder"
                },
                if interface {
                    "support_interface_filament"
                } else {
                    "support_filament"
                },
                0.0,
            ) as i64;
            let nozzle = if tool <= 0 && !orca {
                c.raw("nozzle_diameter")
                    .and_then(|v| {
                        v.split(',')
                            .filter_map(|v| v.trim().parse::<f64>().ok())
                            .reduce(f64::min)
                    })
                    .unwrap_or(s.nozzle_diameter)
            } else {
                c.float_at(
                    "nozzle_diameter",
                    if tool > 0 { (tool - 1) as usize } else { 0 },
                )
                .unwrap_or(s.nozzle_diameter)
            };
            let option = if number(
                "support_material_extrusion_width",
                "support_line_width",
                0.0,
            ) > 0.0
            {
                if orca && c.raw("support_line_width").is_some() {
                    "support_line_width"
                } else {
                    "support_material_extrusion_width"
                }
            } else if orca && c.raw("line_width").is_some() {
                "line_width"
            } else {
                "extrusion_width"
            };
            let raw = c.raw(option).unwrap_or_default();
            let width = raw.trim_end_matches('%').parse::<f64>().unwrap_or(0.0);
            let width = if raw.ends_with('%') {
                width * s.layer_height / 100.0
            } else {
                width
            };
            scaled((if width == 0.0 { nozzle } else { width }) as f32 as f64)
        };
        let line_width = flow_width(false);
        let roof_width = flow_width(true);
        let double_wall_area = if orca {
            match number("", "tree_support_wall_count", 0.0) as i32 {
                0 => 0.25 * (5.0 * SCALE).powi(2) * PI,
                1 => f64::MAX,
                _ => 0.1,
            }
        } else {
            let diameter = c
                .float("support_tree_branch_diameter_double_wall")
                .unwrap_or(3.0);
            if diameter > 0.0 {
                0.25 * (diameter * SCALE).powi(2) * PI
            } else {
                f64::MAX
            }
        };
        Ok(Self {
            tree_branch_diameter_double_wall_area_scaled: double_wall_area,
            rules,
            layer_height: h,
            resolution: scaled(number("gcode_resolution", "resolution", 0.0125)),
            // Upstream reads .value, even for a percent option (25% becomes 25).
            min_feature_size: scaled(c.float("min_feature_size").unwrap_or(0.1)),
            support_angle: PI / 2.0
                - (number("support_material_threshold", "support_threshold_angle", 0.0) + 1.0)
                    .to_radians()
                    .clamp(0.0, PI / 2.0),
            support_line_width: line_width,
            support_roof_line_width: roof_width,
            support_bottom_enable: bottom_enable,
            support_bottom_height: if bottom_enable { bottom * h } else { 0 },
            support_material_buildplate_only: flag(
                "support_material_buildplate_only",
                "support_on_build_plate_only",
                false,
            ),
            support_xy_distance: scaled(xy),
            // Orca-only storage; the volume implementation, like upstream, doesn't consume it.
            support_xy_distance_1st_layer: if orca {
                scaled(number("", "support_object_first_layer_gap", 0.2))
            } else {
                Self::default().support_xy_distance_1st_layer
            },
            support_xy_distance_overhang: scaled(xy).min(scaled(0.5 * external_width)),
            support_top_distance: scaled(top_gap),
            support_bottom_distance: scaled(bottom_gap),
            support_roof_enable: top > 0,
            support_roof_layers: top,
            support_floor_enable: if orca {
                bottom > 0
            } else {
                top > 0 && bottom_raw > 0
            },
            support_floor_layers: if orca {
                bottom
            } else if top > 0 && bottom_raw > 0 {
                bottom_raw
            } else {
                0
            },
            support_roof_pattern: if orca {
                c.raw("support_interface_pattern")
                    .and_then(|v| TreeInterfacePattern::parse(&v))
                    .unwrap_or(s.support_material_interface_pattern.into())
            } else {
                s.support_material_interface_pattern.into()
            },
            support_pattern: if orca {
                c.raw("support_base_pattern")
                    .and_then(|v| TreeSupportPattern::parse(&v))
                    .unwrap_or(s.support_material_pattern.into())
            } else {
                s.support_material_pattern.into()
            },
            support_line_spacing: scaled(number(
                "support_material_spacing",
                "support_base_pattern_spacing",
                2.5,
            )),
            // Orca enforces at least one wall here, independent of its drawing-time
            // 0=auto / 1=single / 2=double area threshold in SupportParameters.
            support_wall_count: if orca {
                number("", "tree_support_wall_count", 0.0).max(1.0) as i32
            } else {
                1
            },
            support_roof_line_distance: scaled(number(
                "support_material_interface_spacing",
                "support_interface_spacing",
                0.0,
            )) + roof_width,
            support_tree_angle: angle,
            support_tree_angle_slow: slow,
            support_tree_branch_diameter: diameter,
            support_tree_branch_diameter_angle: number(
                "support_tree_branch_diameter_angle",
                "tree_support_branch_diameter_angle",
                5.0,
            )
            .to_radians()
            .clamp(0.0, PI / 2.0 - 1e-4),
            support_tree_branch_distance: scaled(number(
                "support_tree_branch_distance",
                "tree_support_branch_distance_organic",
                1.0,
            )),
            support_tree_top_rate: number(
                "support_tree_top_rate",
                "tree_support_top_rate",
                if orca { 30.0 } else { 15.0 },
            ),
            support_tree_tip_diameter: scaled(number(
                "support_tree_tip_diameter",
                "tree_support_tip_diameter",
                0.8,
            ))
            .clamp(0, diameter),
            ..Self::default()
        })
    }
}

/// The SlicingParameters subset used by the upstream tree support grid. Z values
/// are millimetres. Supplied explicitly so multi-object and raft callers retain
/// the original layer grid without going through the old support scheduler.
#[derive(Clone, Debug, PartialEq)]
pub struct TreeSlicingParameters {
    pub layer_height: f64,
    pub object_print_z_min: f64,
    pub first_object_layer_height: f64,
    pub first_print_layer_height: f64,
    pub base_raft_layers: usize,
    pub interface_raft_layers: usize,
    pub base_raft_layer_height: f64,
    pub interface_raft_layer_height: f64,
    pub raft_contact_top_z: f64,
    pub max_support_layer_height: f64,
    /// Object origin on the bed (PrusaSlicer's instance shift). Upstream draws the organic
    /// branches in object coordinates, as `float`s; the drawing works relative to it.
    pub origin: crate::geometry::Point,
}
impl TreeSlicingParameters {
    pub fn new(layer_height: f64, first_layer_height: f64) -> Self {
        Self {
            layer_height,
            object_print_z_min: 0.0,
            first_object_layer_height: first_layer_height,
            first_print_layer_height: first_layer_height,
            base_raft_layers: 0,
            interface_raft_layers: 0,
            base_raft_layer_height: layer_height,
            interface_raft_layer_height: layer_height,
            raft_contact_top_z: 0.0,
            max_support_layer_height: layer_height,
            origin: crate::geometry::Point::default(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct TreeSupportSettings {
    /// Per-generation equivalent of upstream's static soluble/zero_top_z_gap.
    pub zero_top_z_gap: bool,
    pub support_line_width: i64,
    pub layer_height: i64,
    pub branch_radius: i64,
    pub min_radius: i64,
    pub maximum_move_distance: i64,
    pub maximum_move_distance_slow: i64,
    pub support_bottom_layers: usize,
    pub tip_layers: usize,
    pub branch_radius_increase_per_layer: f64,
    pub max_to_model_radius_increase: i64,
    pub min_dtt_to_model: usize,
    pub increase_radius_until_radius: i64,
    pub increase_radius_until_layer: usize,
    pub support_rests_on_model: bool,
    pub xy_distance: i64,
    pub bp_radius: i64,
    pub layer_start_bp_radius: i32,
    pub bp_radius_increase_per_layer: f64,
    pub xy_min_distance: i64,
    pub z_distance_top_layers: usize,
    pub z_distance_bottom_layers: usize,
    pub support_roof_angles: Vec<f64>,
    pub roof_pattern: TreeInterfacePattern,
    pub support_pattern: TreeSupportPattern,
    pub support_roof_line_width: i64,
    pub support_line_spacing: i64,
    pub support_bottom_offset: i64,
    pub support_wall_count: i32,
    pub resolution: i64,
    pub support_roof_line_distance: i64,
    pub interface_preference: InterfacePreference,
    pub settings: TreeSupportMeshGroupSettings,
    pub min_feature_size: i64,
    pub raft_layers: Vec<f64>,
}
impl TreeSupportSettings {
    /// `zero_top_z_gap` is true if ANY object in this generation pass has a zero
    /// top gap, under both rules. Orca resets the upstream static flag each pass;
    /// explicit ownership here prevents the Prusa static flag leaking across jobs.
    pub fn new(
        m: &TreeSupportMeshGroupSettings,
        slicing: &TreeSlicingParameters,
        zero_top_z_gap: bool,
    ) -> Self {
        let layer_height = m.layer_height;
        assert!(layer_height > 0 && m.support_line_width >= 3);
        let branch_radius = m.support_tree_branch_diameter / 2;
        let min_radius = m.support_tree_tip_diameter / 2;
        let tip_layers = ((branch_radius - min_radius) / (m.support_line_width / 3))
            .max(branch_radius / layer_height) as usize;
        assert!(
            tip_layers > 0,
            "at least one tip layer is required upstream"
        );
        let growth = m.support_tree_branch_diameter_angle.tan() * layer_height as f64;
        let bp_growth =
            (0.7_f64.tan() * layer_height as f64).min(0.5 * m.support_line_width as f64);
        let bp_radius = m.support_tree_bp_diameter / 2;
        let xy_min_distance = m
            .support_xy_distance
            .min(m.support_xy_distance_overhang)
            .max(if zero_top_z_gap { scaled(0.1) } else { 0 });
        let movement = |angle: f64| {
            if angle < PI / 2.0 {
                (angle.tan() * layer_height as f64) as i64
            } else {
                i64::MAX
            }
        };
        let mut out = Self {
            zero_top_z_gap,
            support_line_width: m.support_line_width,
            layer_height,
            branch_radius,
            min_radius,
            maximum_move_distance: movement(m.support_tree_angle),
            maximum_move_distance_slow: movement(m.support_tree_angle_slow),
            support_bottom_layers: if m.support_bottom_enable {
                ((m.support_bottom_height + layer_height / 2) / layer_height) as usize
            } else {
                0
            },
            tip_layers,
            branch_radius_increase_per_layer: growth,
            max_to_model_radius_increase: m
                .support_tree_max_diameter_increase_by_merges_when_support_to_model
                / 2,
            min_dtt_to_model: ((m.support_tree_min_height_to_model + layer_height - 1)
                / layer_height) as usize,
            increase_radius_until_radius: branch_radius,
            increase_radius_until_layer: tip_layers,
            support_rests_on_model: !m.support_material_buildplate_only,
            xy_distance: m.support_xy_distance.max(xy_min_distance),
            xy_min_distance,
            bp_radius,
            layer_start_bp_radius: ((bp_radius - branch_radius) as f64 / bp_growth) as i32,
            bp_radius_increase_per_layer: bp_growth,
            z_distance_top_layers: (m.support_top_distance as f64 / layer_height as f64).round()
                as usize,
            z_distance_bottom_layers: (m.support_bottom_distance as f64 / layer_height as f64)
                .round() as usize,
            support_roof_angles: m.support_roof_angles.clone(),
            roof_pattern: m.support_roof_pattern,
            support_pattern: m.support_pattern,
            support_roof_line_width: m.support_roof_line_width,
            support_line_spacing: m.support_line_spacing,
            support_bottom_offset: m.support_bottom_offset,
            support_wall_count: m.support_wall_count,
            resolution: m.resolution,
            support_roof_line_distance: m.support_roof_line_distance,
            interface_preference: InterfacePreference::InterfaceAreaOverwritesSupport,
            settings: m.clone(),
            min_feature_size: m.min_feature_size,
            raft_layers: Vec::new(),
        };
        if slicing.base_raft_layers + slicing.interface_raft_layers > 0 {
            let mut z = slicing.first_print_layer_height;
            out.raft_layers.push(z);
            for _ in 1..slicing.base_raft_layers {
                z += slicing.base_raft_layer_height;
                out.raft_layers.push(z);
            }
            for _ in 0..slicing.interface_raft_layers.saturating_sub(1) {
                z += slicing.interface_raft_layer_height;
                out.raft_layers.push(z);
            }
            if slicing.base_raft_layers + slicing.interface_raft_layers > 1 {
                z = slicing.raft_contact_top_z;
                out.raft_layers.push(z);
            }
            let remaining = slicing.object_print_z_min - z;
            if remaining > 1e-4 {
                // Orca biases the ceil by EPSILON to avoid splitting an almost exact gap.
                let bias = if m.rules.is_orca() { 1e-4 } else { 0.0 };
                let steps = ((remaining - bias) / slicing.max_support_layer_height).ceil() as usize;
                let step = remaining / steps as f64;
                for _ in 0..steps {
                    z += step;
                    out.raft_layers.push(z);
                }
            }
        }
        out
    }
    pub fn get_radius(&self, distance_to_top: usize, elephant_foot_increases: f64) -> i64 {
        // C++ performs integer division in the tip BEFORE adding the floating foot term.
        let base = if distance_to_top <= self.tip_layers {
            self.min_radius
                + (self.branch_radius - self.min_radius) * distance_to_top as i64
                    / self.tip_layers as i64
        } else {
            self.branch_radius
        };
        let radius = if distance_to_top <= self.tip_layers {
            base as f64
        } else {
            self.branch_radius as f64
                + (distance_to_top - self.tip_layers) as f64 * self.branch_radius_increase_per_layer
        };
        (radius
            + elephant_foot_increases
                * (self.bp_radius_increase_per_layer - self.branch_radius_increase_per_layer)
                    .max(0.0)) as i64
    }
    pub fn recommended_min_radius(&self, layer_idx: i32) -> i64 {
        let layers = self.layer_start_bp_radius - layer_idx;
        if layers > 0 {
            (self.branch_radius as f64 + layers as f64 * self.bp_radius_increase_per_layer) as i64
        } else {
            0
        }
    }
    pub fn layer_z(&self, p: &TreeSlicingParameters, layer_idx: usize) -> f64 {
        if layer_idx < self.raft_layers.len() {
            self.raft_layers[layer_idx]
        } else {
            p.object_print_z_min
                + p.first_object_layer_height
                + (layer_idx - self.raft_layers.len()) as f64 * p.layer_height
        }
    }
    pub fn layer_idx_ceil(&self, p: &TreeSlicingParameters, z: f64) -> usize {
        self.layer_idx(p, z, true)
    }
    pub fn layer_idx_floor(&self, p: &TreeSlicingParameters, z: f64) -> usize {
        self.layer_idx(p, z, false)
    }
    fn layer_idx(&self, p: &TreeSlicingParameters, z: f64, ceil: bool) -> usize {
        let first = p.object_print_z_min + p.first_object_layer_height;
        // Orca reverses layer_z inside raft/contact/intermediate layers. Prusa
        // clamps all heights below the first object layer to the object layer.
        if self.settings.rules.is_orca() && !self.raft_layers.is_empty() && z < first - 1e-4 {
            return if ceil {
                self.raft_layers.partition_point(|v| *v < z - 1e-4)
            } else {
                self.raft_layers
                    .partition_point(|v| *v <= z + 1e-4)
                    .saturating_sub(1)
            };
        }
        let i = (z - first) / p.layer_height;
        self.raft_layers.len() + (if ceil { i.ceil() } else { i.floor() }).max(0.0) as usize
    }
    /// InterfacePlacer classification; actual polygon placement is step 2. Orca
    /// reserves a top interface when top base-interface layers also exist.
    pub fn roof_is_interface(
        &self,
        dtt: usize,
        top_layers_only: usize,
        top_base_layers: usize,
    ) -> bool {
        let threshold = top_layers_only
            - usize::from(
                self.settings.rules.is_orca() && top_layers_only > 0 && top_base_layers > 0,
            );
        dtt > 0 && dtt <= threshold
    }
}

impl PartialEq for TreeSupportSettings {
    fn eq(&self, other: &Self) -> bool {
        self.settings.rules == other.settings.rules
            && self.branch_radius == other.branch_radius
            && self.tip_layers == other.tip_layers
            && self.branch_radius_increase_per_layer == other.branch_radius_increase_per_layer
            && self.layer_start_bp_radius == other.layer_start_bp_radius
            && self.bp_radius == other.bp_radius
            && self.bp_radius_increase_per_layer == other.bp_radius_increase_per_layer
            && self.min_radius == other.min_radius
            && self.xy_min_distance == other.xy_min_distance
            && self.support_rests_on_model == other.support_rests_on_model
            && self.increase_radius_until_layer == other.increase_radius_until_layer
            && self.min_dtt_to_model == other.min_dtt_to_model
            && self.max_to_model_radius_increase == other.max_to_model_radius_increase
            && self.maximum_move_distance == other.maximum_move_distance
            && self.maximum_move_distance_slow == other.maximum_move_distance_slow
            && self.z_distance_bottom_layers == other.z_distance_bottom_layers
            && self.support_line_width == other.support_line_width
            && self.support_line_spacing == other.support_line_spacing
            && self.support_roof_line_width == other.support_roof_line_width
            && self.support_bottom_offset == other.support_bottom_offset
            && self.support_wall_count == other.support_wall_count
            && self.support_pattern == other.support_pattern
            && self.roof_pattern == other.roof_pattern
            && self.support_roof_angles == other.support_roof_angles
            && self.increase_radius_until_radius == other.increase_radius_until_radius
            && self.support_bottom_layers == other.support_bottom_layers
            && self.layer_height == other.layer_height
            && self.z_distance_top_layers == other.z_distance_top_layers
            && self.resolution == other.resolution
            && self.support_roof_line_distance == other.support_roof_line_distance
            && self.interface_preference == other.interface_preference
            && self.min_feature_size == other.min_feature_size
            && self.xy_distance == other.xy_distance
            && self.raft_layers == other.raft_layers
    }
}
