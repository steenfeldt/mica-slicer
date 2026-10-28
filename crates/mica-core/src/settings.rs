// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: orca@8500fcdcca src/libslic3r/PrintConfig.cpp :: PrintConfigDef::init_fff_params
// Source: orca@8500fcdcca src/libslic3r/Slicing.cpp :: generate_object_layers
// Source: orca@8500fcdcca src/libslic3r/PerimeterGenerator.cpp :: PerimeterGenerator::process_arachne
// Source: orca@8500fcdcca src/libslic3r/Fill/Fill.cpp :: Layer::make_fills
// Source: orca@8500fcdcca src/libslic3r/LayerRegion.cpp :: LayerRegion::process_external_surfaces
// Changes: Independent internal bridge thickness, densities and source bridge policy.
// Source: prusaslicer@b028299c77 src/libslic3r/PrintConfig.cpp :: PrintConfigDef::init_fff_params, s_keys_map_EnsureVerticalShellThickness
// Changes: Resolved normal support parameters; exact CORE One profile defaults.
// Changes: Orca fixed-height layers stop at the last midpoint below the mesh; retain Mica's
// Changes: existing partial final layer policy for Prusa profiles.
// Changes: Resolve Arachne, top-surface wall count and bridge-wall options for the first extruder.
// Changes: EnsureVerticalShellThickness keeps PrusaSlicer's names and its legacy 0/1 mapping.
// Source: prusaslicer@b028299c77 src/libslic3r/PrintConfig.cpp :: DynamicPrintConfig::normalize_fdm
// Changes: The 1 µm gcode_resolution floor is applied where the value is read, not by rewriting the configuration.

//! Resolved print settings.
//!
//! One flat struct, resolved from the canonical settings schema, profile inheritance and
//! importers (see `config` and the `mica-profiles` crate).

use crate::arachne::ArachneSettings;
use crate::config::Config;
use crate::fill::Pattern;

/// The smallest geometric simplification tolerance, mm. PrusaSlicer (`gcode_resolution`) and
/// Orca (`resolution`) raise smaller values to 1 µm when they normalise a print configuration
/// (`DynamicPrintConfig::normalize_fdm`): "Resolution will be above 1um." The option table
/// allows 0, which would keep every vertex of a dense mesh's slices.
pub const MIN_GCODE_RESOLUTION: f64 = 0.001;

/// `gcode_resolution` in mm with the upstream floor, `default` when the option is missing.
pub fn gcode_resolution(c: &Config, default: f64) -> f64 {
    c.float("gcode_resolution")
        .unwrap_or(default)
        .max(MIN_GCODE_RESOLUTION)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PerimeterGenerator {
    #[default]
    Classic,
    Arachne,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TopOnePerimeter {
    #[default]
    None,
    TopSurfaces,
    TopmostOnly,
}

/// A length that is either absolute (mm) or a percentage of something else.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FloatOrPercent {
    Mm(f64),
    Percent(f64),
}

impl FloatOrPercent {
    /// Resolves against `base` (in the same unit the caller works in); absolute values are converted
    /// with `mm_to_unit` (e.g. `geometry::SCALE` for scaled coordinates, 1.0 for mm).
    pub fn resolve(self, base: f64, mm_to_unit: f64) -> f64 {
        match self {
            FloatOrPercent::Mm(mm) => mm * mm_to_unit,
            FloatOrPercent::Percent(p) => base * p / 100.0,
        }
    }
}

/// PrusaSlicer's `ensure_vertical_shell_thickness`: how solid infill is added where sloping or vertical
/// walls would otherwise be too thin.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EnsureVerticalShellThickness {
    /// Only the horizontal top/bottom shells (`discover_horizontal_shells`).
    Disabled,
    /// Top/bottom surfaces of neighbouring layers are projected, but walls of neighbouring layers are not.
    Partial,
    /// Also make solid where the neighbouring layers' walls sit over or under this layer's infill.
    Enabled,
}

impl EnsureVerticalShellThickness {
    /// Parses a PrusaSlicer value, including the legacy booleans (`1` → enabled, `0` → partial).
    pub fn from_config_name(name: &str) -> Option<Self> {
        Some(match name {
            "disabled" => Self::Disabled,
            "partial" | "0" => Self::Partial,
            "enabled" | "1" => Self::Enabled,
            _ => return None,
        })
    }
}

/// Everything the engine and G-code writer need for one print, already resolved.
#[derive(Clone, Debug, PartialEq)]
pub struct PrintSettings {
    /// Skirt and brim geometry, prepared once per plate.
    pub adhesion: crate::brim::AdhesionSettings,
    // Layers
    pub layer_height: f64,
    pub first_layer_height: f64,
    /// Orca keeps the final layer at full height when its midpoint is inside the model.
    pub complete_layer_heights: bool,
    /// First-layer outline shrink, in mm, limited to preserve printable thin features.
    pub elefant_foot_compensation: f64,
    /// Offset every layer's outline in mm (positive grows material, negative shrinks it).
    pub xy_size_compensation: f64,
    /// Optional Orca dimension and small-feature controls.
    pub small_features: crate::small_features::SmallFeatureSettings,
    /// Number of raft layers below the object.
    pub raft_layers: usize,

    // Normal supports and raft. Distances are mm; density is 0–1.
    pub support_material: bool,
    pub support_material_auto: bool,
    pub support_material_threshold: f64,
    pub tree3d: Option<crate::support::tree3d::Tree3dConfig>,
    pub support_material_style: crate::support::SupportStyle,
    pub support_material_contact_distance: f64,
    pub support_material_bottom_contact_distance: f64,
    pub support_material_xy_spacing: FloatOrPercent,
    pub support_material_spacing: f64,
    pub support_material_angle: f64,
    pub support_material_interface_layers: usize,
    pub support_material_bottom_interface_layers: i32,
    pub support_material_interface_spacing: f64,
    pub support_material_pattern: crate::support::SupportPattern,
    pub support_material_interface_pattern: crate::support::InterfacePattern,
    pub support_material_buildplate_only: bool,
    pub support_material_closing_radius: f64,
    pub support_material_with_sheath: bool,
    /// Orca `support_remove_small_overhang`: drop small automatic overhang clusters.
    pub support_remove_small_overhang: bool,
    /// Orca `SupportParameters` semantics (an Orca config sets the option above): interface
    /// counts exclude the contact layer, plus Orca's normal-support overhang rules.
    pub support_orca_semantics: bool,
    pub support_material_synchronize_layers: bool,
    pub support_material_enforce_layers: usize,
    pub dont_support_bridges: bool,
    pub raft_contact_distance: f64,
    pub raft_expansion: f64,
    pub raft_first_layer_density: f64,
    pub raft_first_layer_expansion: f64,
    pub support_material_extrusion_width: f64,
    /// Support-specific first-layer fallback (first width → support width → global width).
    pub support_material_first_layer_extrusion_width: f64,
    pub support_material_interface_contact_loops: bool,
    pub support_material_speed: f64,
    pub support_material_interface_speed: f64,
    pub min_layer_height: f64,
    pub max_layer_height: f64,

    /// Seam planning, clipping, scarf ramps and seam hiding.
    pub seam: crate::seam::SeamSettings,
    // Tree supports. Angles in degrees, lengths in mm, top rate as a fraction.
    pub support_tree_angle: f64,
    pub support_tree_angle_slow: f64,
    pub support_tree_branch_diameter: f64,
    pub support_tree_branch_diameter_angle: f64,
    pub support_tree_branch_diameter_double_wall: f64,
    pub support_tree_branch_distance: f64,
    pub support_tree_tip_diameter: f64,
    pub support_tree_top_rate: f64,

    // Walls
    pub perimeter_generator: PerimeterGenerator,
    pub arachne: ArachneSettings,
    pub top_one_perimeter: TopOnePerimeter,
    /// Minimum top-surface width for single-wall filtering (relative to inner wall width).
    pub top_surface_min_width: FloatOrPercent,
    /// Extension into adjoining walls: Prusa 1.0, Orca 0.85.
    pub top_surface_wall_extension: f64,
    pub only_one_perimeter_first_layer: bool,
    pub overhangs: bool,
    /// Zero contact gap with enabled supports disables bridge wall classification.
    pub supports_touching: bool,
    /// Override external bridge direction in degrees; zero means automatic.
    pub bridge_angle: f64,
    pub bridge_fan_speed: f64,
    pub bridge_flow_ratio: f64,
    pub thick_bridges: bool,
    pub thick_internal_bridges: bool,
    /// Source-selected bridge spacing, anchoring and flow policy.
    pub bridge_style: crate::bridge::BridgeStyle,
    pub bridge_density: f64,
    pub internal_bridge_density: f64,
    pub bridge_speed: f64,
    pub bridge_acceleration: f64,
    pub perimeters: usize,
    /// Geometric simplification tolerance in mm.
    pub gcode_resolution: f64,
    /// Gap-closing radius of the mesh slicer, mm (`slice_closing_radius`).
    pub slice_closing_radius: f64,
    /// Inner walls first, outer wall last (PrusaSlicer `external_perimeters_first = 0`).
    pub external_perimeters_first: bool,
    /// PrusaSlicer `infill_first`: each island's fills before its perimeters.
    pub infill_first: bool,
    pub orca_walls: crate::wall_options::WallOptions,
    pub extra_bridge_layer: crate::bridge::ExtraBridgeLayer,
    /// Degrees; zero selects automatic internal bridge direction.
    pub internal_bridge_angle: f64,
    /// Orca spacing correction for the inner/outer wall sequence.
    pub precise_outer_wall: bool,
    /// Fill the gaps between wall loops with variable-width extrusions.
    pub gap_fill_enabled: bool,
    /// Detect features too thin for a wall loop and print them as single variable-width lines.
    pub thin_walls: bool,
    /// Spiral vase (PrusaSlicer `spiral_vase`, Orca `spiral_mode`): above the solid bottom, one
    /// wall around the largest outline, raised continuously (`crate::spiral_vase`).
    pub spiral_vase: bool,

    // Top / bottom
    pub top_solid_layers: usize,
    pub bottom_solid_layers: usize,
    /// Minimum thickness of the top shell, mm; adds layers beyond `top_solid_layers` when needed.
    pub top_solid_min_thickness: f64,
    /// Minimum thickness of the bottom shell, mm.
    pub bottom_solid_min_thickness: f64,
    pub ensure_vertical_shell_thickness: EnsureVerticalShellThickness,
    /// Sparse areas smaller than this (mm²) are filled solid.
    pub solid_infill_below_area: f64,
    /// Orca promotes small sparse islands after external-surface expansion, in mm².
    /// None retains Prusa's earlier solid_infill_below_area pass.
    pub minimum_sparse_infill_area: Option<f64>,
    /// Combine sparse infill over this many layers, limited by nozzle diameter.
    pub infill_every_layers: usize,
    /// Every n-th layer gets solid infill instead of sparse; 0 disables.
    pub solid_infill_every_layers: usize,

    // Infill
    /// Sparse infill density, 0.0–1.0.
    pub fill_density: f64,
    /// Base infill angle in degrees.
    pub fill_angle: f64,
    /// Overlap of infill with the innermost wall; percentages are of (half the wall spacing + half the
    /// infill spacing), as in PrusaSlicer.
    pub infill_overlap: FloatOrPercent,
    /// Orca top/bottom overlap, including exposed single-wall top regions.
    pub top_bottom_infill_overlap: Option<FloatOrPercent>,
    /// Sparse infill pattern.
    pub fill_pattern: Pattern,
    /// Settings of the zig-zag infill family (Orca `infill_shift_step`, `symmetric_infill_y_axis`, ...).
    pub zag: crate::fill::ZagSettings,
    /// Pattern of top and bottom surfaces.
    pub top_fill_pattern: Pattern,
    pub bottom_fill_pattern: Pattern,
    pub internal_solid_fill_pattern: Pattern,
    /// Orca replaces vibrating narrow solid sweeps with variable-width concentric fill.
    pub detect_narrow_internal_solid_infill: bool,
    pub ironing: crate::ironing::IroningSettings,
    pub fuzzy_skin: crate::fuzzy_skin::FuzzySkinSettings,
    pub texture_skin: crate::texture::TextureSettings,
    /// Set while an object is sliced from its displaced mesh (textures on all surfaces), never
    /// from a configuration: its layers have dense outlines and thousands of small top islands,
    /// so walls, shells and bridges take shortcuts that keep the geometry (to well below a
    /// micrometre, or a hundredth of a millimetre on the infill side of the walls) but not every
    /// bit of the plain code's output. Untextured objects never see it.
    pub textured_mesh: bool,
    /// Brick layers: every other inner wall raised half a layer (`mica_brick_layers*`).
    pub brick: crate::brick_layers::BrickSettings,
    /// Longest hook of an unconnected sparse infill line end along the walls; percentages are of the
    /// infill line spacing.
    pub infill_anchor: FloatOrPercent,
    /// Sparse infill line ends closer than this along the walls are connected; percentages as above.
    pub infill_anchor_max: FloatOrPercent,

    // Extrusion widths, mm
    pub nozzle_diameter: f64,
    /// Thickest combined sparse infill (`infill_every_layers`): the smaller nozzle of the sparse
    /// and solid infill tools (`PrintObject::combine_infill`).
    pub infill_combine_nozzle: f64,
    pub first_layer_extrusion_width: f64,
    pub external_perimeter_extrusion_width: f64,
    pub perimeter_extrusion_width: f64,
    pub infill_extrusion_width: f64,
    pub solid_infill_extrusion_width: f64,
    pub top_infill_extrusion_width: f64,

    // Speeds, mm/s
    pub first_layer_speed: f64,
    pub external_perimeter_speed: f64,
    pub perimeter_speed: f64,
    pub infill_speed: f64,
    pub solid_infill_speed: f64,
    pub top_solid_infill_speed: f64,
    /// 0 disables gap fill.
    pub gap_fill_speed: f64,
    pub travel_speed: f64,
    pub travel_speed_z: f64,

    // Accelerations, mm/s²
    pub default_acceleration: f64,
    pub first_layer_acceleration: f64,
    pub external_perimeter_acceleration: f64,
    pub perimeter_acceleration: f64,
    pub infill_acceleration: f64,
    pub solid_infill_acceleration: f64,
    pub top_solid_infill_acceleration: f64,

    // Filament
    pub filament_diameter: f64,
    pub extrusion_multiplier: f64,
    pub filament_max_volumetric_speed: f64,
    pub filament_type: String,
    pub first_layer_temperature: f64,
    pub temperature: f64,
    pub first_layer_bed_temperature: f64,
    pub bed_temperature: f64,
    pub chamber_temperature: f64,
    /// Pressure advance (Prusa `M572 S`).
    pub pressure_advance: f64,
    /// Nozzle temperature while waiting (`idle_temperature`); `None` when not set.
    pub idle_temperature: Option<f64>,
    pub filament_abrasive: bool,
    pub nozzle_high_flow: bool,

    // Retraction
    pub retract_length: f64,
    pub retract_speed: f64,
    pub deretract_speed: f64,
    pub retract_lift: f64,
    /// Travels longer than this (mm) retract.
    pub retract_before_travel: f64,

    // Cooling
    pub min_fan_speed: f64,
    pub max_fan_speed: f64,
    pub disable_fan_first_layers: usize,
    pub full_fan_speed_layer: usize,
    pub fan_below_layer_time: f64,
    pub slowdown_below_layer_time: f64,
    pub min_print_speed: f64,
}

impl PrintSettings {
    /// Prusa CORE One, HF0.4 nozzle, "0.20mm SPEED", Prusament PLA, as resolved by PrusaSlicer 2.9.6
    /// from vendor profiles 2.5.12. A fixed baseline for tests; `from_config` resolves real profiles.
    pub fn core_one_hf04_speed_pla() -> Self {
        Self {
            adhesion: crate::brim::AdhesionSettings {
                min_skirt_length: 4.0,
                brim: crate::brim::BrimSettings {
                    separation: 0.1,
                    ..Default::default()
                },
                ..Default::default()
            },
            layer_height: 0.2,
            first_layer_height: 0.2,
            complete_layer_heights: false,
            elefant_foot_compensation: 0.2,
            xy_size_compensation: 0.0,
            small_features: crate::small_features::SmallFeatureSettings::default(),
            raft_layers: 0,
            support_material: true,
            support_material_auto: false,
            support_material_threshold: 35.0,
            tree3d: None,
            support_material_style: crate::support::SupportStyle::Snug,
            support_material_contact_distance: 0.2,
            support_material_bottom_contact_distance: 0.0,
            support_material_xy_spacing: FloatOrPercent::Percent(80.0),
            support_material_spacing: 2.0,
            support_material_angle: 0.0,
            support_material_interface_layers: 3,
            support_material_bottom_interface_layers: 0,
            support_material_interface_spacing: 0.2,
            support_material_pattern: crate::support::SupportPattern::Rectilinear,
            support_material_interface_pattern: crate::support::InterfacePattern::Auto,
            support_material_buildplate_only: false,
            support_material_closing_radius: 2.0,
            support_material_with_sheath: false,
            support_remove_small_overhang: false,
            support_orca_semantics: false,
            support_material_synchronize_layers: false,
            support_material_enforce_layers: 0,
            dont_support_bridges: false,
            raft_contact_distance: 0.15,
            raft_expansion: 1.5,
            raft_first_layer_density: 0.9,
            raft_first_layer_expansion: 3.0,
            support_material_extrusion_width: 0.4,
            support_material_first_layer_extrusion_width: 0.5,
            support_material_interface_contact_loops: false,
            support_material_speed: 120.0,
            support_material_interface_speed: 60.0,
            min_layer_height: 0.07,
            max_layer_height: 0.3,
            perimeter_generator: PerimeterGenerator::Arachne,
            arachne: ArachneSettings::default(),
            top_one_perimeter: TopOnePerimeter::None,
            top_surface_min_width: FloatOrPercent::Percent(100.),
            top_surface_wall_extension: 1.0,
            only_one_perimeter_first_layer: false,
            overhangs: true,
            supports_touching: false,
            bridge_angle: 0.0,
            bridge_fan_speed: 100.0,
            bridge_flow_ratio: 1.0,
            thick_bridges: false,
            thick_internal_bridges: true,
            bridge_style: crate::bridge::BridgeStyle::Prusa,
            bridge_density: 1.,
            internal_bridge_density: 1.,
            bridge_speed: 50.0,
            bridge_acceleration: 1500.0,
            seam: crate::seam::SeamSettings::default(),
            support_tree_angle: 40.0,
            support_tree_angle_slow: 25.0,
            support_tree_branch_diameter: 2.0,
            support_tree_branch_diameter_angle: 5.0,
            support_tree_branch_diameter_double_wall: 8.0,
            support_tree_branch_distance: 1.0,
            support_tree_tip_diameter: 0.8,
            support_tree_top_rate: 0.3,
            perimeters: 2,
            gcode_resolution: 0.008,
            slice_closing_radius: crate::slice::DEFAULT_CLOSING_RADIUS,
            external_perimeters_first: false,
            infill_first: false,
            orca_walls: Default::default(),
            extra_bridge_layer: Default::default(),
            internal_bridge_angle: 0.,
            precise_outer_wall: false,
            gap_fill_enabled: true,
            thin_walls: false,
            spiral_vase: false,
            top_solid_layers: 5,
            bottom_solid_layers: 3,
            top_solid_min_thickness: 0.7,
            bottom_solid_min_thickness: 0.5,
            ensure_vertical_shell_thickness: EnsureVerticalShellThickness::Enabled,
            solid_infill_below_area: 0.0,
            minimum_sparse_infill_area: None,
            infill_every_layers: 1,
            solid_infill_every_layers: 0,
            fill_density: 0.15,
            fill_angle: 45.0,
            infill_overlap: FloatOrPercent::Percent(15.0),
            top_bottom_infill_overlap: None,
            fill_pattern: Pattern::Grid,
            zag: crate::fill::ZagSettings::default(),
            top_fill_pattern: Pattern::MonotonicLines,
            bottom_fill_pattern: Pattern::Monotonic,
            internal_solid_fill_pattern: Pattern::Ensuring,
            detect_narrow_internal_solid_infill: false,
            ironing: crate::ironing::IroningSettings::default(),
            fuzzy_skin: crate::fuzzy_skin::FuzzySkinSettings::default(),
            texture_skin: crate::texture::TextureSettings::default(),
            textured_mesh: false,
            brick: crate::brick_layers::BrickSettings::default(),
            infill_anchor: FloatOrPercent::Mm(2.0),
            infill_anchor_max: FloatOrPercent::Mm(12.0),
            nozzle_diameter: 0.4,
            infill_combine_nozzle: 0.4,
            first_layer_extrusion_width: 0.5,
            external_perimeter_extrusion_width: 0.45,
            perimeter_extrusion_width: 0.45,
            infill_extrusion_width: 0.45,
            solid_infill_extrusion_width: 0.45,
            top_infill_extrusion_width: 0.42,
            first_layer_speed: 45.0,
            external_perimeter_speed: 200.0,
            perimeter_speed: 250.0,
            infill_speed: 300.0,
            solid_infill_speed: 250.0,
            top_solid_infill_speed: 100.0,
            gap_fill_speed: 120.0,
            travel_speed: 350.0,
            travel_speed_z: 12.0,
            default_acceleration: 3000.0,
            first_layer_acceleration: 500.0,
            external_perimeter_acceleration: 3000.0,
            perimeter_acceleration: 6000.0,
            infill_acceleration: 7000.0,
            solid_infill_acceleration: 6000.0,
            top_solid_infill_acceleration: 2000.0,
            filament_diameter: 1.75,
            extrusion_multiplier: 1.0,
            filament_max_volumetric_speed: 24.0,
            filament_type: "PLA".into(),
            first_layer_temperature: 230.0,
            temperature: 230.0,
            first_layer_bed_temperature: 60.0,
            bed_temperature: 60.0,
            chamber_temperature: 20.0,
            pressure_advance: 0.036,
            idle_temperature: Some(170.0),
            filament_abrasive: false,
            nozzle_high_flow: true,
            retract_length: 0.7,
            retract_speed: 45.0,
            deretract_speed: 25.0,
            retract_lift: 0.2,
            retract_before_travel: 1.5,
            min_fan_speed: 85.0,
            max_fan_speed: 100.0,
            disable_fan_first_layers: 1,
            full_fan_speed_layer: 3,
            fan_below_layer_time: 40.0,
            slowdown_below_layer_time: 8.0,
            min_print_speed: 20.0,
        }
    }

    /// Whether gap fill is generated (PrusaSlicer: `gap_fill_enabled` and a non-zero `gap_fill_speed`).
    pub fn has_gap_fill(&self) -> bool {
        self.gap_fill_enabled && self.gap_fill_speed > 0.0
    }

    /// Print height of each layer's top surface (mm), for an object `height` mm tall.
    pub fn layer_tops(&self, height: f64) -> Vec<f64> {
        if height < self.first_layer_height - 1e-9 {
            return Vec::new();
        }
        let above_first =
            ((height - self.first_layer_height) / self.layer_height + 1e-9).floor() as usize;
        let mut tops: Vec<f64> = (0..=above_first)
            .map(|i| self.first_layer_height + i as f64 * self.layer_height)
            .collect();
        // Keep a final partial layer only if it is at least half a layer thick.
        if let Some(&last) = tops.last()
            && height - last > self.layer_height / 2.0
        {
            tops.push(if self.complete_layer_heights {
                last + self.layer_height
            } else {
                height
            });
        }
        tops
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn twenty_mm_object_has_hundred_layers() {
        let s = PrintSettings::core_one_hf04_speed_pla();
        let tops = s.layer_tops(20.0);
        assert_eq!(tops.len(), 100);
        assert!((tops[99] - 20.0).abs() < 1e-9);
    }
}
